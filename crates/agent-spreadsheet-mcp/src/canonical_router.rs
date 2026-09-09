use crate::server::SpreadsheetServer;
use crate::state::AppState;
use agent_spreadsheet::operations::{
    CanonicalErrorCode, CanonicalErrorEnvelope, CanonicalResponse, OperationAdapter,
    OperationDescriptor, OperationRisk, RuntimeCapabilities, decode_operation, execute_operation,
    operation_registry,
};
use rmcp::{
    ErrorData as McpError,
    handler::server::{
        router::tool::{ToolRoute, ToolRouter},
        tool::ToolCallContext,
    },
    model::{CallToolResult, Content, Meta, Tool, ToolAnnotations},
};
use serde::Serialize;
use serde_json::{Value, to_value};
use std::{borrow::Cow, sync::Arc};

pub(crate) const CANONICAL_TOOL_META_KEY: &str = "agent-spreadsheet/canonical";
pub(crate) const CANONICAL_SCHEMA_VERSION: &str = "1";
pub(crate) const REQUEST_ID_META_KEY: &str = "agent-spreadsheet/request-id";

pub(crate) fn runtime_capabilities(state: &AppState) -> RuntimeCapabilities {
    let mut capabilities = RuntimeCapabilities::from_state(state);
    capabilities.vba = state.config().vba_enabled;
    capabilities.resident_history = cfg!(feature = "recalc-formualizer") && capabilities.workbook_write;
    capabilities
}

pub(crate) fn canonical_tool_router(
    capabilities: &RuntimeCapabilities,
) -> ToolRouter<SpreadsheetServer> {
    let mut router = ToolRouter::new();
    for descriptor in operation_registry()
        .iter()
        .filter(|descriptor| descriptor.is_available_for(OperationAdapter::Mcp, capabilities))
    {
        router.add_route(canonical_route(descriptor));
    }
    router
}

/// Titles are annotations only. Keep dialect selection and every validation
/// keyword (including unevaluatedProperties) unchanged in the MCP projection.
fn strip_schema_titles(schema: &mut Value) {
    if let Some(object) = schema.as_object_mut() { object.remove("title"); }
}

fn canonical_route(descriptor: &'static OperationDescriptor) -> ToolRoute<SpreadsheetServer> {
    let mut schema = (descriptor.input_schema)();
    strip_schema_titles(&mut schema);
    let input_schema = schema.as_object().cloned().expect("canonical operation input schemas are objects");
    let mut tool = Tool::new(
        descriptor.name,
        canonical_description(descriptor),
        Arc::new(input_schema),
    )
    .annotate(canonical_annotations(descriptor.risk_ceiling));
    tool.meta = Some(canonical_meta(descriptor.name));
    let operation = descriptor.name;

    ToolRoute::new_dyn(tool, move |context| {
        Box::pin(async move { call_canonical_operation(context, operation).await })
    })
}

fn canonical_meta(operation: &str) -> Meta {
    Meta(serde_json::Map::from_iter([(
        CANONICAL_TOOL_META_KEY.to_string(),
        serde_json::json!({
            "schema_version": CANONICAL_SCHEMA_VERSION,
            "operation": operation,
        }),
    )]))
}

fn canonical_description(descriptor: &OperationDescriptor) -> Cow<'static, str> {
    let risk = match descriptor.risk_ceiling {
        OperationRisk::Low => "Risk: low.",
        OperationRisk::Moderate => {
            "Risk <=moderate."
        }
        OperationRisk::High => {
            "Risk <=high."
        }
        OperationRisk::Destructive => {
            "Risk <=destructive."
        }
    };
    Cow::Owned(format!("{} {risk}", descriptor.description))
}

fn canonical_annotations(risk: OperationRisk) -> ToolAnnotations {
    match risk {
        OperationRisk::Low => ToolAnnotations::new()
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false),
        OperationRisk::Moderate => ToolAnnotations::new()
            .read_only(false)
            .destructive(false)
            .idempotent(false)
            .open_world(false),
        OperationRisk::High | OperationRisk::Destructive => ToolAnnotations::new()
            .read_only(false)
            .destructive(true)
            .idempotent(false)
            .open_world(false),
    }
}

async fn call_canonical_operation(
    context: ToolCallContext<'_, SpreadsheetServer>,
    operation: &'static str,
) -> Result<CallToolResult, McpError> {
    let request_id = context.request_context.meta.0.get(REQUEST_ID_META_KEY)
        .map(|value| value.as_str().map(str::to_owned).ok_or_else(|| McpError::invalid_params("request identity metadata must be a string", None)))
        .transpose()?;
    let arguments = Value::Object(context.arguments.unwrap_or_default());
    context
        .service
        .execute_canonical_operation(operation, arguments, request_id)
        .await
}

pub(crate) fn canonical_result<T: Serialize>(
    value: &T,
    is_error: bool,
) -> Result<CallToolResult, McpError> {
    let structured =
        to_value(value).map_err(|error| McpError::internal_error(error.to_string(), None))?;
    let text = serde_json::to_string(value)
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
    Ok(CallToolResult {
        content: vec![Content::text(text)],
        structured_content: Some(structured),
        is_error: is_error.then_some(true),
        meta: None,
    })
}

/// Canonical name of the screenshot operation, whose artifact crosses adapter
/// boundaries as image content (MCP) or bytes (HTTP artifact route).
pub(crate) const SCREENSHOT_OPERATION: &str = "screenshot_sheet";

/// Adapter correlation only: absent IDs denote distinct calls, not safe retries.
pub(crate) fn ingress_identity(
    supplied: Option<String>,
    operation: &str,
) -> Result<Option<String>, CanonicalErrorEnvelope> {
    if supplied.as_ref().is_some_and(|id| id.is_empty() || id.len() > 256) {
        return Err(CanonicalErrorEnvelope::new(
            CanonicalErrorCode::InvalidRequest,
            "request identity must contain 1..256 bytes",
            Some(operation),
            None,
        ));
    }
    #[cfg(feature = "recalc-formualizer")]
    { Ok(supplied.or_else(|| Some(uuid::Uuid::new_v4().to_string()))) }
    #[cfg(not(feature = "recalc-formualizer"))]
    { Ok(supplied) }
}

/// Decode, check capabilities, route the binding and bound response waiting.
/// Tool-enable/response-size policy remains with the requesting adapter;
/// spreadsheet semantics remain with the shared canonical dispatcher.
pub(crate) async fn dispatch(
    state: Arc<AppState>,
    timeout: Option<std::time::Duration>,
    operation: &str,
    arguments: Value,
    request_id: Option<String>,
) -> Result<CanonicalResponse, CanonicalErrorEnvelope> {
    let decoded = decode_operation(operation, arguments.clone())?;
    if !agent_spreadsheet::operations::operation_descriptor(operation).expect("decoded operation")
        .is_available_for(OperationAdapter::Mcp, &runtime_capabilities(&state)) {
        return Err(CanonicalErrorEnvelope::new(CanonicalErrorCode::CapabilityUnavailable,
            format!("operation '{operation}' is unavailable in this adapter"), Some(operation), None));
    }
    #[cfg(feature = "recalc-formualizer")]
    {
        use agent_spreadsheet::{native_host::{NativeHostClient, HostRequest}, operations::SpreadsheetOperation};
        let resident = matches!(&decoded, SpreadsheetOperation::CreateFork(_) | SpreadsheetOperation::ListForks(_))
            || decoded.resource_id().is_some_and(|id| id.as_str().starts_with("fork:") || id.as_str().starts_with("session:"))
            || matches!(&decoded, SpreadsheetOperation::VerifyWorkbook(request)
                if request.baseline_resource_id.as_str().starts_with("fork:")
                    || request.baseline_resource_id.as_str().starts_with("session:"));
        if resident {
            let error = |code, message: String| CanonicalErrorEnvelope::new(code, message, Some(operation), None);
            let identity = match request_id {
                Some(id) if !id.is_empty() && id.len() <= 256 => id,
                Some(_) => return Err(error(CanonicalErrorCode::InvalidRequest, "request identity must contain 1..256 bytes".into())),
                None => uuid::Uuid::new_v4().to_string(),
            };
            let root = agent_spreadsheet::native_host::workspace_root(&state.config().workspace_root)
                .map_err(|e| error(CanonicalErrorCode::OperationFailed, e.to_string()))?;
            let resource = decoded.resource_id().map(|id| id.as_str()).unwrap_or("").to_owned();
            let operation = operation.to_owned();
            let config = (*state.config()).clone();
            // Deadline applies only to waiting, never to the independently
            // owned operation. Dropping JoinHandle detaches (does not abort).
            let reconciliation = format!("resource {resource:?}, request identity {identity:?}");
            let transport_reconciliation = reconciliation.clone();
            let response_operation = operation.clone();
            let task_operation = operation.clone();
            let pending = tokio::spawn(async move {
                let client = NativeHostClient::connect(&root).await.map_err(|e| CanonicalErrorEnvelope::new(
                    CanonicalErrorCode::OperationFailed, format!("native request not submitted: {e}"), Some(&operation), None))?;
                client.request(&HostRequest::Configure { config }).await.map_err(|e| CanonicalErrorEnvelope::new(
                    CanonicalErrorCode::OperationFailed, format!("native operation not submitted; configuration rejected: {e}"), Some(&operation), None))?;
                match client.execute(resource, identity, task_operation, arguments).await {
                    Ok(Err(mut error)) if error.error.code == CanonicalErrorCode::OutcomeUnknown => {
                        error.error.message.push_str(&format!("; reconcile {transport_reconciliation}"));
                        Err(error)
                    }
                    Ok(result) => result,
                    Err(error) => Err(CanonicalErrorEnvelope::new(CanonicalErrorCode::OutcomeUnknown,
                        format!("native transport failed; reconcile {transport_reconciliation}: {error}"), Some(&operation), None)),
                }
            });
            let completed = match timeout {
                Some(limit) => tokio::time::timeout(limit, pending).await.map_err(|_| CanonicalErrorEnvelope::new(
                    CanonicalErrorCode::OutcomeUnknown,
                    format!("response deadline elapsed; accepted work is not cancelled; reconcile {reconciliation}"),
                    Some(&response_operation), None))?,
                None => pending.await,
            };
            return completed.map_err(|e| CanonicalErrorEnvelope::new(CanonicalErrorCode::OutcomeUnknown,
                format!("response unavailable ({e}); reconcile {reconciliation}"), Some(&response_operation), None))?;
        }
    }
    #[cfg(not(feature = "recalc-formualizer"))]
    let _ = request_id;
    match timeout {
        Some(limit) => tokio::time::timeout(limit, execute_operation(state, decoded))
            .await
            .unwrap_or_else(|_| {
                Err(CanonicalErrorEnvelope::new(
                    CanonicalErrorCode::OperationFailed,
                    format!(
                        "operation '{operation}' timed out after {}ms",
                        limit.as_millis()
                    ),
                    Some(operation),
                    None,
                ))
            }),
        None => execute_operation(state, decoded).await,
    }
}

/// Attach `Content::image` for a successful canonical `screenshot_sheet` result.
///
/// `structured_content` is left untouched; the image is appended after the text
/// content. The base64 payload is charged against the adapter response-size
/// limit. A result without a resolvable artifact handle is left unchanged.
#[cfg(feature = "recalc")]
pub(crate) fn attach_screenshot_image(
    result: &mut CallToolResult,
    workspace_root: &std::path::Path,
    response_limit: Option<usize>,
) -> Result<(), McpError> {
    use base64::Engine;

    let Some(structured) = result.structured_content.as_ref() else {
        return Ok(());
    };
    let artifact = &structured["data"]["artifact"];
    let Some(handle) = artifact["handle"].as_str() else {
        return Ok(());
    };
    let resolved = crate::artifacts::resolve_artifact(workspace_root, handle).map_err(|error| {
        McpError::internal_error(
            format!("screenshot artifact is unavailable: {}", error.message()),
            None,
        )
    })?;
    let media_type = artifact["media_type"]
        .as_str()
        .unwrap_or(resolved.media_type)
        .to_string();
    let encoded = base64::engine::general_purpose::STANDARD.encode(&resolved.bytes);
    if let Some(limit) = response_limit
        && encoded.len() > limit
    {
        return Err(McpError::internal_error(
            format!(
                "screenshot image content is {} bytes, exceeding the {limit} byte response limit",
                encoded.len()
            ),
            None,
        ));
    }
    result.content.push(Content::image(encoded, media_type));
    Ok(())
}

pub(crate) async fn execute(
    server: &SpreadsheetServer,
    operation: &'static str,
    arguments: Value,
    request_id: Option<String>,
) -> Result<CallToolResult, McpError> {
    server.ensure_canonical_tool_enabled(operation)?;
    let request_id = ingress_identity(request_id, operation)
        .map_err(|error| McpError::invalid_params(error.error.message, None))?;
    let requested_resource = arguments.get("resource_id").and_then(Value::as_str).map(str::to_owned);

    let result = dispatch(
        server.canonical_state(),
        server.canonical_tool_timeout(),
        operation,
        arguments,
        request_id.clone(),
    )
    .await;
    project_execution_result(server, operation, requested_resource.as_deref(), request_id, result)
}

fn project_execution_result(
    server: &SpreadsheetServer,
    operation: &str,
    requested_resource: Option<&str>,
    request_id: Option<String>,
    result: Result<CanonicalResponse, CanonicalErrorEnvelope>,
) -> Result<CallToolResult, McpError> {
    // Projection happens after execution. Its failure cannot retroactively turn
    // a completed operation into an invalid request or erase its retry identity.
    let projected = (|| match &result {
        Ok(response) => {
            server.ensure_canonical_response_size(operation, &response)?;
            #[cfg_attr(not(feature = "recalc"), allow(unused_mut))]
            let mut call_result = canonical_result(&response, false)?;
            #[cfg(feature = "recalc")]
            if operation == SCREENSHOT_OPERATION {
                attach_screenshot_image(
                    &mut call_result,
                    &server.canonical_state().config().workspace_root,
                    server.canonical_response_limit(),
                )?;
            }
            Ok(call_result)
        }
        Err(error) => {
            server.ensure_canonical_response_size(operation, &error)?;
            canonical_result(&error, true)
        }
    })();
    let mut result = projected.map_err(|projection_error| {
        let response = result.as_ref().ok();
        McpError::internal_error(
            format!(
                "operation '{operation}' response could not be delivered; execution is not rolled back. Reconcile request identity {:?} rather than blindly retrying: {projection_error}",
                request_id
            ),
            Some(serde_json::json!({
                REQUEST_ID_META_KEY: request_id,
                "resource_id": response.and_then(|r| r.resource_id.as_ref().map(|id| id.as_str()))
                    .or(requested_resource),
                "revision_id": response.and_then(|r| r.revision_id.as_deref()),
                "operation_completed": result.is_ok(),
                "canonical_error_code": result.as_ref().err().map(|e| e.error.code),
            })),
        )
    })?;
    if let Some(identity) = request_id {
        result.meta = Some(Meta(serde_json::Map::from_iter([(REQUEST_ID_META_KEY.into(), Value::String(identity))])));
    }
    Ok(result)
}

#[cfg(all(test, feature = "recalc"))]
mod tests {
    use super::*;
    use rmcp::model::RawContent;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    #[test]
    fn supplied_id_validation_is_independent_of_binding() {
        for id in [String::new(), "x".repeat(257), "é".repeat(129)] {
            assert_eq!(ingress_identity(Some(id), "describe_workbook").unwrap_err().error.code,
                CanonicalErrorCode::InvalidRequest);
        }
        assert_eq!(ingress_identity(Some("x".repeat(256)), "describe_workbook").unwrap(), Some("x".repeat(256)));
    }

    fn size_limited_server() -> SpreadsheetServer {
        use agent_spreadsheet::config::{ServerConfig, TransportKind, RecalcBackendKind, OutputProfile};
        let config = ServerConfig {
            workspace_root: ".".into(), screenshot_dir: "screenshots".into(),
            path_mappings: vec![], cache_capacity: 8, supported_extensions: vec!["xlsx".into()],
            single_workbook: None, enabled_tools: None, transport: TransportKind::Stdio,
            http_bind_address: "127.0.0.1:0".parse().unwrap(), recalc_enabled: false,
            recalc_backend: RecalcBackendKind::Auto, vba_enabled: false, max_concurrent_recalcs: 1,
            tool_timeout_ms: None, max_response_bytes: Some(1), output_profile: OutputProfile::TokenDense,
            max_payload_bytes: None, max_cells: None, max_items: None, allow_overwrite: false, slim_surface: true,
        };
        SpreadsheetServer::from_state(Arc::new(AppState::new(Arc::new(config))))
    }

    #[test]
    fn oversized_completed_response_retains_identity_and_actual_resource() {
        let server = size_limited_server();
        let response = CanonicalResponse {
            schema_version: "1".into(), operation: "write".into(),
            resource_id: Some(serde_json::from_value(json!("fork:created_result")).unwrap()),
            revision_id: Some("committed-revision".into()), data: json!({"ops_applied": 1}),
        };
        let error = project_execution_result(&server, "write", Some("wb:source"),
            Some("stable-id".into()), Ok(response)).unwrap_err();
        let data = error.data.unwrap();
        assert_eq!(data[REQUEST_ID_META_KEY], "stable-id");
        assert_eq!(data["resource_id"], "fork:created_result");
        assert_eq!(data["revision_id"], "committed-revision");
        assert_eq!(data["operation_completed"], true);
        assert!(error.message.contains("not rolled back"));
    }

    #[test]
    fn oversized_error_retains_unknown_outcome_classification() {
        let server = size_limited_server();
        let outcome = CanonicalErrorEnvelope::new(CanonicalErrorCode::OutcomeUnknown,
            "response lost", Some("write"), None);
        let error = project_execution_result(&server, "write", Some("fork:target"),
            Some("retry-id".into()), Err(outcome)).unwrap_err();
        let data = error.data.unwrap();
        assert_eq!(data[REQUEST_ID_META_KEY], "retry-id");
        assert_eq!(data["canonical_error_code"], "OUTCOME_UNKNOWN");
        assert_eq!(data["operation_completed"], false);
    }

    fn stub_artifact(workspace: &std::path::Path, bytes: &[u8]) -> Value {
        let root = workspace.join("artifacts");
        std::fs::create_dir_all(&root).unwrap();
        let hex = format!("{:x}", Sha256::digest(bytes));
        std::fs::write(root.join(format!("{hex}.png")), bytes).unwrap();
        json!({
            "handle": format!("artifact:sha256:{hex}"),
            "hash": format!("sha256:{hex}"),
            "bytes": bytes.len(),
            "media_type": "image/png",
        })
    }

    fn screenshot_envelope(artifact: Value) -> Value {
        json!({
            "schema_version": "1",
            "operation": SCREENSHOT_OPERATION,
            "resource_id": "workbook:stub",
            "revision_id": "rev-1",
            "data": {
                "sheet_name": "Sheet1",
                "range": "A1:M40",
                "artifact": artifact,
                "duration_ms": 12,
            },
        })
    }

    #[test]
    fn screenshot_results_carry_one_text_and_one_image_item() {
        let workspace = tempfile::tempdir().unwrap();
        let png = b"\x89PNG\r\n\x1a\nstub-bytes";
        let envelope = screenshot_envelope(stub_artifact(workspace.path(), png));
        let before = envelope.clone();

        let mut result = canonical_result(&envelope, false).unwrap();
        attach_screenshot_image(&mut result, workspace.path(), None).unwrap();

        assert_eq!(result.content.len(), 2);
        assert!(matches!(&result.content[0].raw, RawContent::Text(_)));
        let RawContent::Image(image) = &result.content[1].raw else {
            panic!("second content item must be an image");
        };
        assert_eq!(image.mime_type, "image/png");
        use base64::Engine;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&image.data)
                .unwrap(),
            png
        );
        assert_eq!(result.structured_content.as_ref().unwrap(), &before);
        assert!(result.is_error.is_none());
    }

    #[test]
    fn screenshot_image_is_charged_against_the_response_limit() {
        let workspace = tempfile::tempdir().unwrap();
        let envelope = screenshot_envelope(stub_artifact(workspace.path(), b"stub-bytes"));
        let mut result = canonical_result(&envelope, false).unwrap();
        let error = attach_screenshot_image(&mut result, workspace.path(), Some(4)).unwrap_err();
        assert!(error.message.contains("response limit"), "{error:?}");
        assert_eq!(result.content.len(), 1);
    }

    #[test]
    fn results_without_an_artifact_handle_are_unchanged() {
        let workspace = tempfile::tempdir().unwrap();
        let envelope = json!({"schema_version": "1", "operation": "list_sheets", "data": {}});
        let mut result = canonical_result(&envelope, false).unwrap();
        attach_screenshot_image(&mut result, workspace.path(), None).unwrap();
        assert_eq!(result.content.len(), 1);
    }
}
