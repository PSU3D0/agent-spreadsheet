//! Canonical HTTP route (`/v1`) mounted beside the MCP streamable-HTTP service.
//!
//! This is a transport adapter only: it binds JSON to a canonical operation,
//! reuses the MCP dispatcher, and projects canonical envelopes onto HTTP status
//! codes. It contains no spreadsheet semantics. See
//! `docs/architecture/canonical-http-route.md`.

use crate::artifacts::{ArtifactError, resolve_artifact};
use crate::canonical_router;
use crate::server::SpreadsheetServer;
use crate::state::AppState;
use agent_spreadsheet::operations::{
    CanonicalErrorCode, CanonicalErrorEnvelope, OperationAdapter, RuntimeCapabilities,
    operation_descriptor, operations_discovery_for, registry_projection,
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

pub const CANONICAL_HTTP_PREFIX: &str = "/v1";

/// HTTP status for a canonical error code.
///
/// | code | status |
/// | --- | --- |
/// | `INVALID_REQUEST` | 400 |
/// | `STALE_CURSOR`, `CURSOR_MISMATCH`, `ROW_EXCEEDS_BUDGET` | 400 |
/// | `UNKNOWN_OPERATION` | 404 |
/// | `RESOURCE_NOT_FOUND` | 404 |
/// | `REVISION_CONFLICT` | 409 |
/// | `OPERATION_FAILED` | 500 |
/// | `CAPABILITY_UNAVAILABLE` | 501 |
pub fn status_for(code: CanonicalErrorCode) -> StatusCode {
    match code {
        CanonicalErrorCode::InvalidRequest
        | CanonicalErrorCode::StaleCursor
        | CanonicalErrorCode::CursorMismatch
        | CanonicalErrorCode::RowExceedsBudget => StatusCode::BAD_REQUEST,
        CanonicalErrorCode::UnknownOperation | CanonicalErrorCode::ResourceNotFound => {
            StatusCode::NOT_FOUND
        }
        CanonicalErrorCode::RevisionConflict => StatusCode::CONFLICT,
        CanonicalErrorCode::OperationFailed => StatusCode::INTERNAL_SERVER_ERROR,
        CanonicalErrorCode::CapabilityUnavailable => StatusCode::NOT_IMPLEMENTED,
        CanonicalErrorCode::OutcomeUnknown | CanonicalErrorCode::RecoveryRequired => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Canonical `/v1` router bound to the live server process.
pub fn canonical_http_router(state: Arc<AppState>) -> Router {
    let server = Arc::new(SpreadsheetServer::from_state(state));
    Router::new()
        .route("/v1/op/{operation}", post(execute_operation_route))
        .route("/v1/operations", get(operations_route))
        .route("/v1/registry", get(registry_route))
        .route("/v1/artifacts/{handle}", get(artifact_route))
        .with_state(server)
}

fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => (
            status,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            body,
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            format!(
                "{{\"schema_version\":\"1\",\"error\":{{\"code\":\"OPERATION_FAILED\",\"message\":\"failed to serialize response: {error}\"}}}}"
            ),
        )
            .into_response(),
    }
}

fn error_response(envelope: &CanonicalErrorEnvelope) -> Response {
    json_response(status_for(envelope.error.code), envelope)
}

fn live_capabilities(state: &AppState) -> RuntimeCapabilities {
    canonical_router::runtime_capabilities(state)
}

async fn operations_route(State(server): State<Arc<SpreadsheetServer>>) -> Response {
    let state = server.canonical_state();
    let discovery =
        operations_discovery_for(OperationAdapter::Mcp, &live_capabilities(state.as_ref()));
    json_response(StatusCode::OK, &discovery)
}

async fn registry_route() -> Response {
    json_response(StatusCode::OK, &registry_projection())
}

async fn execute_operation_route(
    State(server): State<Arc<SpreadsheetServer>>,
    Path(operation): Path<String>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    let Some(descriptor) = operation_descriptor(&operation) else {
        return error_response(&CanonicalErrorEnvelope::new(
            CanonicalErrorCode::UnknownOperation,
            format!("unknown operation '{operation}'"),
            Some(&operation),
            Some("$.operation".to_string()),
        ));
    };
    let operation = descriptor.name;

    if !server.canonical_tool_allowed(operation) {
        return error_response(&CanonicalErrorEnvelope::new(
            CanonicalErrorCode::CapabilityUnavailable,
            format!("operation '{operation}' is disabled on this server"),
            Some(operation),
            None,
        ));
    }

    let arguments = match parse_request_body(operation, &body) {
        Ok(arguments) => arguments,
        Err(envelope) => return error_response(&envelope),
    };

    let request_id = match headers.get("x-agent-spreadsheet-request-id") {
        Some(value) => match value.to_str() {
            Ok(value) => Some(value.to_owned()),
            Err(_) => return error_response(&CanonicalErrorEnvelope::new(CanonicalErrorCode::InvalidRequest, "invalid request identity header", Some(operation), None)),
        },
        None => None,
    };
    let request_id = match canonical_router::ingress_identity(request_id, operation) {
        Ok(identity) => identity,
        Err(error) => return error_response(&error),
    };
    let result = canonical_router::dispatch(
        server.canonical_state(),
        server.canonical_tool_timeout(),
        operation,
        arguments,
        request_id.clone(),
    )
    .await;

    let outgoing = match result {
        Ok(response) => {
            if let Some(limit) = server.canonical_response_limit() {
                let size = serde_json::to_vec(&response).map(|bytes| bytes.len()).ok();
                if let Some(size) = size
                    && size > limit
                {
                    return with_request_identity(error_response(&CanonicalErrorEnvelope::new(
                        CanonicalErrorCode::OperationFailed,
                        format!(
                            "operation '{operation}' completed but its response is {size} bytes, exceeding the {limit} byte limit; effects are not rolled back. Reconcile using the response request-identity header rather than blindly retrying"
                        ),
                        Some(operation),
                        None,
                    )), request_id.as_deref());
                }
            }
            json_response(StatusCode::OK, &response)
        }
        Err(envelope) => error_response(&envelope),
    };
    with_request_identity(outgoing, request_id.as_deref())
}

fn with_request_identity(mut response: Response, identity: Option<&str>) -> Response {
    if let Some(identity) = identity.and_then(|id| HeaderValue::from_str(id).ok()) {
        response.headers_mut().insert("x-agent-spreadsheet-request-id", identity);
    }
    response
}

fn parse_request_body(operation: &str, body: &Bytes) -> Result<Value, CanonicalErrorEnvelope> {
    if body.is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let value: Value = serde_json::from_slice(body).map_err(|error| {
        CanonicalErrorEnvelope::new(
            CanonicalErrorCode::InvalidRequest,
            format!("request body is not valid JSON: {error}"),
            Some(operation),
            Some("$".to_string()),
        )
    })?;
    if !value.is_object() {
        return Err(CanonicalErrorEnvelope::new(
            CanonicalErrorCode::InvalidRequest,
            "request body must be a canonical input object".to_string(),
            Some(operation),
            Some("$".to_string()),
        ));
    }
    Ok(value)
}

async fn artifact_route(
    State(server): State<Arc<SpreadsheetServer>>,
    Path(handle): Path<String>,
) -> Response {
    let workspace_root = server.canonical_state().config().workspace_root.clone();
    match resolve_artifact(&workspace_root, &handle) {
        Ok(artifact) => {
            let media_type = HeaderValue::from_str(artifact.media_type)
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
            let length = artifact.bytes.len();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, media_type)
                .header(header::CONTENT_LENGTH, length)
                .body(Body::from(artifact.bytes))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(error) => {
            let code = match error {
                ArtifactError::Malformed => CanonicalErrorCode::InvalidRequest,
                ArtifactError::NotFound => CanonicalErrorCode::ResourceNotFound,
                ArtifactError::TooLarge => CanonicalErrorCode::OperationFailed,
            };
            error_response(&CanonicalErrorEnvelope::new(
                code,
                error.message().to_string(),
                Some("artifacts"),
                None,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_table_matches_the_documented_mapping() {
        assert_eq!(
            status_for(CanonicalErrorCode::InvalidRequest),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for(CanonicalErrorCode::StaleCursor),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for(CanonicalErrorCode::CursorMismatch),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for(CanonicalErrorCode::RowExceedsBudget),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for(CanonicalErrorCode::UnknownOperation),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_for(CanonicalErrorCode::ResourceNotFound),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_for(CanonicalErrorCode::RevisionConflict),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_for(CanonicalErrorCode::OperationFailed),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_for(CanonicalErrorCode::CapabilityUnavailable),
            StatusCode::NOT_IMPLEMENTED
        );
    }
}
