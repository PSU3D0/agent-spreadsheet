//! Private native transport and executable-independent bootstrap.
//! This module knows canonical operation names only through decode_operation.
use crate::{config::ServerConfig, native_resident::*, operations::CanonicalResponse};
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::post,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, Notify, Semaphore};

#[path = "native_host_auth.rs"]
mod auth;

const PROTOCOL: &str = concat!("resident/11/core-", env!("CARGO_PKG_VERSION"));
const HOST_ARGUMENT: &str = "--private-resident-host";
const MAX_OWNERS: usize = 32;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "control", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostRequest {
    Ping,
    Materialize {
        intent: crate::resident_export::FileExportIntent,
    },
    Configure {
        config: ServerConfig,
    },
    Shutdown,
    Diagnostics {
        resource_id: String,
    },
    Admission {
        resource_id: String,
    },
    Detach {
        resource_id: String,
    },
    Bind {
        resource_id: String,
        source: PathBuf,
        expected_sha256: String,
        config: ServerConfig,
    },
    Execute {
        resource_id: String,
        request_id: String,
        operation: String,
        payload: Value,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Discovery {
    protocol: String,
    port: u16,
    credential: String,
    instance: String,
}
struct HostState {
    discovery: Discovery,
    root: PathBuf,
    config: Mutex<Option<Arc<ServerConfig>>>,
    bindings: NativeBindings,
    owners: Mutex<HashMap<String, Arc<NativeOwnerLane>>>,
    owner_admission: Mutex<()>,
    closing: std::sync::atomic::AtomicBool,
    ingress: Semaphore,
    nonces: Mutex<HashMap<String, u64>>,
    shutdown: Arc<Notify>,
}

impl HostState {
    async fn prune_closed_lanes(&self) {
        // Terminal admission closes independently of response delivery. Reclaim
        // the slot even when the discard caller vanished before receiving it.
        self.owners.lock().await.retain(|_, lane| !lane.is_closed());
    }
    async fn owner(&self, resource: &str) -> Result<Arc<NativeOwnerLane>> {
        self.prune_closed_lanes().await;
        if let Some(lane) = self.owners.lock().await.get(resource).cloned() {
            return Ok(lane);
        }
        let _admission = self.owner_admission.lock().await;
        ensure!(
            !self.closing.load(std::sync::atomic::Ordering::Acquire),
            "host closing; attach not admitted"
        );
        if let Some(lane) = self.owners.lock().await.get(resource).cloned() {
            return Ok(lane);
        }
        ensure!(
            self.owners.lock().await.len() < MAX_OWNERS,
            "owner capacity reached; not admitted"
        );
        ensure!(
            self.bindings.terminal_journal(resource).await?.is_none(),
            "resource has been discarded"
        );
        let lane = Arc::new(self.bindings.attach(resource).await?);
        self.owners
            .lock()
            .await
            .insert(resource.into(), lane.clone());
        Ok(lane)
    }

    async fn configuration(&self) -> Result<Arc<ServerConfig>> {
        let mut config = self.config.lock().await;
        if let Some(value) = config.as_ref() {
            return Ok(value.clone());
        }
        let value: ServerConfig =
            serde_json::from_slice(&read_private(&self.root.join("config.json"), 1024 * 1024)?)?;
        sync_directory(&self.root)?;
        let value = Arc::new(value);
        *config = Some(value.clone());
        Ok(value)
    }

    async fn configure(&self, mut proposed: ServerConfig) -> Result<Arc<ServerConfig>> {
        proposed.workspace_root = proposed.workspace_root.canonicalize()?;
        let mut config = self.config.lock().await;
        let path = self.root.join("config.json");
        let existing = match config.as_ref() {
            Some(value) => Some(value.clone()),
            None if path.try_exists()? => Some(Arc::new(serde_json::from_slice::<ServerConfig>(
                &read_private(&path, 1024 * 1024)?,
            )?)),
            None => None,
        };
        if let Some(existing) = existing {
            ensure!(
                same_resident_authority(existing.as_ref(), &proposed)?,
                "native host workspace/configuration authority differs; use its pinned configuration"
            );
            sync_directory(&self.root)?;
            *config = Some(existing.clone());
            return Ok(existing);
        }
        proposed.ensure_workspace_root()?;
        let pending = self
            .root
            .join(format!("pending_config_{}", uuid::Uuid::new_v4().simple()));
        write_new_private(&pending, &serde_json::to_vec(&proposed)?)?;
        fs::rename(&pending, &path)?;
        sync_directory(&self.root).context("configuration publication outcome unknown")?;
        let proposed = Arc::new(proposed);
        *config = Some(proposed.clone());
        Ok(proposed)
    }
}

#[cfg(all(test, feature = "cli"))]
mod authority_tests {
    use super::*;

    #[test]
    fn transport_presentation_does_not_replace_pinned_authority() {
        let config = ServerConfig::from_args(crate::config::CliArgs::default()).unwrap();
        let mut adapter = config.clone();
        adapter.transport = crate::config::TransportKind::Http;
        adapter.http_bind_address = "127.0.0.1:43210".parse().unwrap();
        adapter.output_profile = crate::config::OutputProfile::Verbose;
        adapter.tool_timeout_ms = Some(1);
        adapter.max_response_bytes = Some(256);
        assert!(same_resident_authority(&config, &adapter).unwrap());
        adapter.workspace_root = config.workspace_root.join("different");
        assert!(!same_resident_authority(&config, &adapter).unwrap());
        adapter = config.clone();
        adapter.recalc_enabled = !config.recalc_enabled;
        assert!(!same_resident_authority(&config, &adapter).unwrap());
        adapter = config.clone();
        adapter.enabled_tools = Some(Default::default());
        adapter.slim_surface = !config.slim_surface;
        assert!(same_resident_authority(&config, &adapter).unwrap());
        adapter = config.clone();
        adapter.vba_enabled = !config.vba_enabled;
        adapter.recalc_backend = crate::config::RecalcBackendKind::Formualizer;
        assert!(same_resident_authority(&config, &adapter).unwrap());
        adapter.recalc_backend = crate::config::RecalcBackendKind::Libreoffice;
        assert!(!same_resident_authority(&config, &adapter).unwrap());
        adapter = config.clone();
        adapter.max_cells = Some(config.max_cells.unwrap_or(0) + 1);
        assert!(!same_resident_authority(&config, &adapter).unwrap());
    }
}

/// Transport presentation is adapter-owned, not workbook authority. Keep every
/// other setting pinned (including workbook admission/evaluation limits).
/// Enabled tools, legacy registration and response policy are enforced by the
/// requesting MCP adapter, never inherited as authority from another adapter.
fn same_resident_authority(existing: &ServerConfig, proposed: &ServerConfig) -> Result<bool> {
    let mut comparable = proposed.clone();
    comparable.transport = existing.transport;
    comparable.http_bind_address = existing.http_bind_address;
    comparable.output_profile = existing.output_profile;
    comparable.tool_timeout_ms = existing.tool_timeout_ms;
    comparable.max_response_bytes = existing.max_response_bytes;
    comparable.enabled_tools = existing.enabled_tools.clone();
    comparable.slim_surface = existing.slim_surface;
    comparable.vba_enabled = existing.vba_enabled;
    // Native residents always retain Formualizer; Auto resolves to that same
    // evaluator here. Never equate an explicit alternative engine with it.
    if matches!(
        existing.recalc_backend,
        crate::config::RecalcBackendKind::Auto | crate::config::RecalcBackendKind::Formualizer
    ) && matches!(
        proposed.recalc_backend,
        crate::config::RecalcBackendKind::Auto | crate::config::RecalcBackendKind::Formualizer
    ) {
        comparable.recalc_backend = existing.recalc_backend;
    }
    Ok(serde_json::to_value(existing)? == serde_json::to_value(comparable)?)
}

// Host policy only: source binding, publication and owner admission. The shared
// dispatcher and shared creation receipt construct all canonical semantics.
struct NativeCreationContext<'a> {
    host: &'a HostState,
    request_id: &'a str,
    files: Arc<crate::state::AppState>,
}
impl crate::execution_context::ExecutionContext for NativeCreationContext<'_> {
    type Reads<'a>
        = Arc<crate::state::AppState>
    where
        Self: 'a;
    fn reads(&self) -> Result<Self::Reads<'_>, crate::operations::CanonicalErrorEnvelope> {
        Ok(self.files.clone())
    }
    fn capabilities(&self) -> crate::operations::RuntimeCapabilities {
        crate::operations::RuntimeCapabilities::from_state(&self.files)
    }
    async fn identify(
        &self,
        resource: &crate::operations::ResourceId,
    ) -> Result<(crate::operations::ResourceId, String)> {
        if resource.as_str().starts_with("fork:") || resource.as_str().starts_with("session:") {
            let lane = self.host.owner(resource.as_str()).await?;
            let descriptor = lane
                .catalog_descriptor()
                .await?
                .context("parent resource has been discarded")?;
            return Ok((
                resource.clone(),
                descriptor.revision_id.context("parent requires recovery")?,
            ));
        }
        self.files.identify(resource).await
    }
    async fn verify_workbook(
        &mut self,
        request: crate::canonical_lifecycle::VerifyWorkbookRequest,
    ) -> Result<crate::canonical_lifecycle::VerifyWorkbookData> {
        let baseline =
            verification_snapshot(self.host, &self.files, &request.baseline_resource_id).await?;
        let current = verification_snapshot(self.host, &self.files, &request.resource_id).await?;
        crate::canonical_lifecycle::verify_workbook_snapshots(
            (*self.files.config()).clone(),
            request,
            baseline,
            current,
        )
        .await
    }
    async fn list_forks(
        &mut self,
        _request: crate::canonical_lifecycle::ListForksRequest,
    ) -> Result<crate::canonical_lifecycle::ListForksData> {
        // Prevent activation/detach races while selecting the catalog owner.
        let _admission = self.host.owner_admission.lock().await;
        let mut forks = Vec::new();
        for resource in self.host.bindings.catalog_resources()? {
            let lane = self.host.owners.lock().await.get(&resource).cloned();
            let descriptor = match lane {
                Some(lane) => lane.catalog_descriptor().await?,
                None => self.host.bindings.inactive_descriptor(&resource).await?,
            };
            if let Some(descriptor) = descriptor {
                forks.push(descriptor);
            }
        }
        Ok(crate::canonical_lifecycle::ListForksData {
            forks,
            warnings: Vec::new(),
        })
    }
    async fn create_fork(
        &mut self,
        request: crate::canonical_lifecycle::CreateForkRequest,
    ) -> Result<crate::canonical_lifecycle::CreateForkData> {
        let parent = if request.resource_id.as_str().starts_with("fork:")
            || request.resource_id.as_str().starts_with("session:")
        {
            Some(self.host.owner(request.resource_id.as_str()).await?)
        } else {
            None
        };
        // Serialize resource publication with memory-owner admission. Do not
        // publish a durable resource then discover there is no owner capacity.
        let _admission = self.host.owner_admission.lock().await;
        ensure!(
            !self.host.closing.load(std::sync::atomic::Ordering::Acquire),
            "host closing; creation not admitted"
        );
        if let Some(response) = self
            .host
            .bindings
            .creation_outcome(self.request_id, &request)
            .await?
        {
            return Ok(serde_json::from_value(response.data)?);
        }
        self.host.prune_closed_lanes().await;
        ensure!(
            self.host.owners.lock().await.len() < MAX_OWNERS,
            "owner capacity reached; creation not admitted"
        );
        if let Some(parent) = parent {
            let binding = self.host.bindings.binding(request.resource_id.as_str())?;
            ensure!(
                serde_json::to_value(&binding.config)?
                    == serde_json::to_value(self.files.config().as_ref())?,
                "parent configuration differs from pinned host authority"
            );
            let bytes = parent
                .capture_fork_base(request.expected_revision.clone())
                .await?;
            let resource =
                NativeBindings::creation_resource(self.request_id, &request.resource_id)?;
            let (lane, response) = self
                .host
                .bindings
                .create_child(self.request_id.into(), request, binding, bytes)
                .await?;
            self.host
                .owners
                .lock()
                .await
                .insert(resource, Arc::new(lane));
            return serde_json::from_value(response.data)
                .context("child creation response outcome unknown");
        }
        let book = self
            .files
            .open_workbook(&request.resource_id.to_workbook_id())
            .await?;
        ensure!(
            book.revision_id == request.expected_revision,
            "revision conflict: source changed"
        );
        let resource = NativeBindings::creation_resource(self.request_id, &request.resource_id)?;
        let (lane, response) = self
            .host
            .bindings
            .create_canonical(
                self.request_id.into(),
                request,
                book.path.clone(),
                book.revision_id.clone(),
                (*self.files.config()).clone(),
            )
            .await?;
        self.host
            .owners
            .lock()
            .await
            .insert(resource, Arc::new(lane));
        serde_json::from_value(response.data).context("creation response outcome unknown")
    }
}

// Capture foreign bindings outside an owner lane; both verification directions
// use the same file/native binding policy and shared verification algorithm.
async fn verification_snapshot(
    host: &HostState,
    files: &Arc<crate::state::AppState>,
    resource: &crate::operations::ResourceId,
) -> Result<crate::canonical_lifecycle::VerificationSnapshot> {
    if resource.as_str().starts_with("fork:") || resource.as_str().starts_with("session:") {
        let owner = host.owner(resource.as_str()).await?;
        let descriptor = owner
            .catalog_descriptor()
            .await?
            .context("verification resource discarded")?;
        let revision = descriptor
            .revision_id
            .context("verification resource requires recovery")?;
        let bytes = owner.capture_fork_base(revision.clone()).await?;
        Ok(crate::canonical_lifecycle::VerificationSnapshot { bytes, revision })
    } else {
        let workbook = files.open_workbook(&resource.to_workbook_id()).await?;
        let bytes = fs::read(&workbook.path)?;
        let revision = crate::utils::hash_bytes_sha256_hex(&bytes);
        Ok(crate::canonical_lifecycle::VerificationSnapshot { bytes, revision })
    }
}

async fn admission(
    State(state): State<Arc<HostState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let headers = request.headers();
    if headers.contains_key("origin")
        || request.method() != axum::http::Method::POST
        || request.uri() != "/"
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let (Some(nonce), Some(time), Some(signature)) = (
        header("x-asp-nonce"),
        header("x-asp-time"),
        header("x-asp-signature"),
    ) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let (Ok(time), Ok(now)) = (time.parse::<u64>(), auth::timestamp()) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if nonce.len() != 32 || !nonce.bytes().all(|b| b.is_ascii_hexdigit()) || now.abs_diff(time) > 60
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(_permit) = state.ingress.try_acquire() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let (parts, body) = request.into_parts();
    let Ok(body) = axum::body::to_bytes(body, MAX_REQUEST_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    if auth::verify(
        &state.discovery.credential,
        "request",
        &nonce,
        time,
        &body,
        &signature,
    )
    .is_err()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    {
        let mut nonces = state.nonces.lock().await;
        nonces.retain(|_, accepted| now.saturating_sub(*accepted) <= 120);
        if nonces.len() >= 65_536 || nonces.insert(nonce.clone(), now).is_some() {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(body));
    let response = next.run(request).await;
    let (mut parts, body) = response.into_parts();
    let Ok(body) = axum::body::to_bytes(body, 32 * 1024 * 1024).await else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let signature = auth::sign(
        &state.discovery.credential,
        "response",
        &nonce,
        parts.status.as_u16() as u64,
        &body,
    );
    parts.headers.insert(
        "x-asp-signature",
        signature.parse().expect("hex signature is an HTTP header"),
    );
    axum::response::Response::from_parts(parts, axum::body::Body::from(body))
}

fn recovery_error(operation: &str, error: anyhow::Error) -> Value {
    json!({"canonical_error":crate::operations::CanonicalErrorEnvelope::new(
        crate::operations::CanonicalErrorCode::RecoveryRequired, error.to_string(), Some(operation), None,
    )})
}

async fn terminal_response(
    journal: crate::core::resident_storage::native::NativeResidentJournal,
    resource: crate::operations::ResourceId,
    request_id: String,
    operation: crate::operations::SpreadsheetOperation,
) -> Result<Value> {
    tokio::task::spawn_blocking(move || -> Result<Value> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                Ok(
                    match crate::session::execute_terminal_session(
                        &journal,
                        &resource,
                        &request_id,
                        operation,
                    )
                    .await
                    {
                        Ok(response) => json!({"response":response}),
                        Err(error) => json!({"canonical_error":error}),
                    },
                )
            })
    })
    .await?
}

async fn handle(
    State(state): State<Arc<HostState>>,
    Json(request): Json<HostRequest>,
) -> (StatusCode, Json<Value>) {
    let result: Result<Value> = async {
        match request {
            HostRequest::Ping => Ok(json!({"protocol":PROTOCOL,"instance":state.discovery.instance,"pid":std::process::id()})),
            HostRequest::Materialize { intent } => {
                ensure!(!intent.request_id.is_empty() && intent.request_id.len() <= 256, "invalid materialize request identity");
                if let Some(outcome) = state.bindings.file_export_outcome(&intent).await? { return Ok(serde_json::to_value(outcome)?); }
                let owner = state.owner(&intent.resource_id).await?;
                Ok(serde_json::to_value(owner.materialize(intent).await?)?)
            }
            HostRequest::Configure { config } => {
                state.configure(config).await?;
                Ok(json!({"configured":true}))
            }
            HostRequest::Shutdown => { state.shutdown.notify_one(); Ok(json!({"shutting_down":true})) }
            HostRequest::Detach { resource_id } => {
                let _admission = state.owner_admission.lock().await;
                let mut owners = state.owners.lock().await;
                if let Some(lane) = owners.remove(&resource_id) {
                    match Arc::try_unwrap(lane) {
                        Ok(lane) => lane.detach().await?,
                        Err(lane) => { owners.insert(resource_id, lane); bail!("owner has active clients; detach not admitted"); }
                    }
                }
                Ok(json!({"detached":true}))
            }
            HostRequest::Admission { resource_id } => {
                let lane = state.owners.lock().await.get(&resource_id).cloned().context("owner not attached")?;
                Ok(lane.admission())
            }
            HostRequest::Diagnostics { resource_id } => {
                let lane = state.owners.lock().await.get(&resource_id).cloned().context("owner not attached")?;
                lane.diagnostics().await
            }
            HostRequest::Bind { resource_id, source, expected_sha256, config } => {
                let config = (*state.configure(config).await?).clone();
                let bindings = state.bindings.clone();
                // Cold ingestion is isolated from the HTTP executor. No owner or
                // evaluator ever crosses this blocking-worker boundary.
                let host = state.clone();
                let binding = tokio::task::spawn_blocking(move || {
                    let _admission = host.owner_admission.blocking_lock();
                    ensure!(!host.closing.load(std::sync::atomic::Ordering::Acquire), "host closing; binding not admitted");
                    bindings.create(&resource_id, &source, &expected_sha256, config)
                }).await??;
                Ok(serde_json::to_value(binding)?)
            }
            HostRequest::Execute { resource_id, request_id, operation, payload } => {
                state.prune_closed_lanes().await;
                // Validate the transport binding against the same canonical enum.
                let decoded = match crate::operations::decode_operation(&operation, payload.clone()) {
                    Ok(decoded) => decoded,
                    Err(error) => return Ok(json!({"canonical_error":error})),
                };
                if request_id.is_empty() || request_id.len() > 256
                    || decoded.resource_id().map(|id| id.as_str()).unwrap_or("") != resource_id.as_str() {
                    return Ok(json!({"canonical_error":crate::operations::CanonicalErrorEnvelope::new(
                        crate::operations::CanonicalErrorCode::InvalidRequest,
                        "invalid request identity or canonical resource differs from transport binding",
                        Some(&operation), None,
                    )}));
                }
                if matches!(decoded, crate::operations::SpreadsheetOperation::CreateFork(_) | crate::operations::SpreadsheetOperation::ListForks(_))
                    || matches!(&decoded, crate::operations::SpreadsheetOperation::VerifyWorkbook(request)
                        if request.resource_id.as_str().starts_with("wb:")) {
                    let state = state.clone();
                    return tokio::task::spawn_blocking(move || -> Result<Value> {
                    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async move {
                    if let crate::operations::SpreadsheetOperation::CreateFork(ref request) = decoded {
                    // Reconciliation must precede source lookup/CAS: the source may
                    // have changed or disappeared after the accepted creation.
                    match state.bindings.creation_outcome(&request_id, request).await {
                        Ok(Some(response)) => return Ok(json!({"response":response})),
                        Ok(None) => (),
                        Err(error) => return Ok(json!({"canonical_error":crate::operations::lifecycle_error("create_fork", error)})),
                    }
                    }
                    let config = state.configuration().await?;
                    let context = NativeCreationContext {
                        host: &state, request_id: &request_id,
                        files: Arc::new(crate::state::AppState::new(config)),
                    };
                    match crate::operations::execute_operation(context, decoded).await {
                        Ok(response) => Ok(json!({"response":response})),
                        Err(error) => Ok(json!({"canonical_error":error})),
                    }
                    })
                    }).await?;
                }
                let binding = match state.bindings.binding(&resource_id) {
                    Ok(binding) => binding,
                    Err(error) => {
                        let missing = error.chain().any(|cause| cause.downcast_ref::<std::io::Error>().is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound));
                        return Ok(json!({"canonical_error":crate::operations::CanonicalErrorEnvelope::new(
                            if missing { crate::operations::CanonicalErrorCode::ResourceNotFound } else { crate::operations::CanonicalErrorCode::RecoveryRequired },
                            error.to_string(), Some(&operation), None,
                        )}));
                    }
                };
                if state.root.join("config.json").try_exists()? {
                    let pinned = state.configuration().await?;
                    if serde_json::to_value(&binding.config)? != serde_json::to_value(pinned.as_ref())? {
                        return Ok(recovery_error(&operation, anyhow::anyhow!("resource configuration differs from pinned host authority")));
                    }
                }
                if !state.owners.lock().await.contains_key(&resource_id) {
                    let terminal = match state.bindings.terminal_journal(&resource_id).await {
                        Ok(value) => value, Err(error) => return Ok(recovery_error(&operation, error)),
                    };
                    if let Some(journal) = terminal {
                        let resource = serde_json::from_value(Value::String(resource_id.clone()))?;
                        return terminal_response(journal, resource, request_id, decoded).await;
                    }
                }
                // Foreign verification inputs are captured before entering the
                // current owner's lane, so reciprocal verification cannot deadlock.
                // Native identities never reach the legacy ForkRegistry.
                let verification_baseline = if let crate::operations::SpreadsheetOperation::VerifyWorkbook(ref request) = decoded {
                    let baseline = &request.baseline_resource_id;
                    if baseline.as_str() == resource_id { None } else {
                        let files = Arc::new(crate::state::AppState::new(state.configuration().await?));
                        let snapshot = verification_snapshot(&state, &files, baseline).await?;
                        Some((baseline.clone(), snapshot))
                    }
                } else { None };
                let existing = state.owners.lock().await.get(&resource_id).cloned();
                let lane = if let Some(lane) = existing { lane } else {
                    let _admission = state.owner_admission.lock().await;
                    ensure!(!state.closing.load(std::sync::atomic::Ordering::Acquire), "host closing; attach not admitted");
                    let existing = state.owners.lock().await.get(&resource_id).cloned();
                    if let Some(lane) = existing { lane } else {
                        let terminal = match state.bindings.terminal_journal(&resource_id).await {
                        Ok(value) => value, Err(error) => return Ok(recovery_error(&operation, error)),
                    };
                    if let Some(journal) = terminal {
                            let resource = serde_json::from_value(Value::String(resource_id.clone()))?;
                            return terminal_response(journal, resource, request_id, decoded).await;
                        }
                        ensure!(state.owners.lock().await.len() < MAX_OWNERS, "owner capacity reached; not admitted");
                        let lane = match state.bindings.attach(&resource_id).await {
                            Ok(lane) => Arc::new(lane), Err(error) => return Ok(recovery_error(&operation, error)),
                        };
                        state.owners.lock().await.insert(resource_id.clone(), lane.clone());
                        lane
                    }
                };
                match lane.execute_with_verification_baseline(request_id, operation, payload, verification_baseline).await? {
                    Ok(response) => {
                        if response.operation == "discard_fork" {
                            state.owners.lock().await.remove(&resource_id);
                        }
                        Ok(json!({"response":response}))
                    },
                    Err(error) => Ok(json!({"canonical_error":error})),
                }
            }
        }
    }.await;
    match result {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":error.to_string()})),
        ),
    }
}

/// Called before any CLI/MCP argv parser. All shipped entrypoints embed this
/// function; standalone MCP does not search for or spawn an adjacent CLI.
pub async fn run_if_requested() -> Result<bool> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_none_or(|value| value != HOST_ARGUMENT) {
        return Ok(false);
    }
    ensure!(
        args.len() == 3,
        "private host requires exactly one root path"
    );
    serve(PathBuf::from(&args[2])).await?;
    Ok(true)
}

pub async fn serve(root: PathBuf) -> Result<()> {
    check_private(&root, true)?;
    let lock_path = root.join("host.lock");
    let lock = private_options()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    check_private(&lock_path, false)?;
    lock.try_lock_exclusive()
        .context("native host already running")?;
    let bindings = NativeBindings::open(root.join("resources"))?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let discovery = Discovery {
        protocol: PROTOCOL.into(),
        port: listener.local_addr()?.port(),
        credential: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        instance: uuid::Uuid::new_v4().to_string(),
    };
    let pending = root.join(format!("discovery_{}.tmp", discovery.instance));
    write_new_private(&pending, &serde_json::to_vec(&discovery)?)?;
    let discovery_path = root.join("discovery.json");
    if discovery_path.try_exists()? {
        check_private(&discovery_path, false)?;
        fs::remove_file(&discovery_path)?;
    }
    fs::rename(&pending, &discovery_path)?;
    sync_directory(&root)?;
    let shutdown = Arc::new(Notify::new());
    let state = Arc::new(HostState {
        discovery,
        root: root.clone(),
        config: Mutex::new(None),
        bindings,
        owners: Mutex::new(HashMap::new()),
        owner_admission: Mutex::new(()),
        closing: std::sync::atomic::AtomicBool::new(false),
        ingress: Semaphore::new(32),
        nonces: Mutex::new(HashMap::new()),
        shutdown: shutdown.clone(),
    });
    let app = Router::new()
        .route("/", post(handle))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            admission,
        ))
        .with_state(state.clone());
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.notified().await })
        .await?;
    // Detached accepted cold creation may outlive its HTTP receiver. Fence new
    // admission and wait for its publication before releasing the lifetime lock.
    state
        .closing
        .store(true, std::sync::atomic::Ordering::Release);
    let _admission = state.owner_admission.lock().await;
    // Graceful HTTP shutdown has drained handlers. Detach only releases memory;
    // immutable bases, journal receipts and catalogs remain durable authority.
    for (_, lane) in std::mem::take(&mut *state.owners.lock().await) {
        match Arc::try_unwrap(lane) {
            Ok(lane) => lane.detach().await?,
            Err(_) => bail!("host shutdown retained a live lane"),
        }
    }
    fs::remove_file(discovery_path)?;
    sync_directory(&root)?;
    drop(lock);
    Ok(())
}

/// Bootstrap is serialized by an OS lock. A live host lock with unreachable
/// discovery is an error, never permission to start a second owner or steal it.
#[derive(Clone)]
pub struct NativeHostClient {
    http: reqwest::Client,
    discovery: Discovery,
}
impl NativeHostClient {
    pub async fn connect(root: &Path) -> Result<Self> {
        check_private(root, true)?;
        let lock_path = root.join("bootstrap.lock");
        let lock = private_options()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        check_private(&lock_path, false)?;
        // The bootstrap lock is acquired on a blocking worker, not the runtime.
        let lock = tokio::task::spawn_blocking(move || -> Result<_> {
            lock.lock_exclusive()?;
            Ok(lock)
        })
        .await??;
        if let Ok(client) = Self::discover(root) {
            if client.ping().await.is_ok() {
                return Ok(client);
            }
        }
        let host_lock_path = root.join("host.lock");
        let host_lock = private_options()
            .create(true)
            .read(true)
            .write(true)
            .open(&host_lock_path)?;
        check_private(&host_lock_path, false)?;
        host_lock
            .try_lock_exclusive()
            .context("live native host is unreachable; do not steal ownership")?;
        FileExt::unlock(&host_lock)?;
        let mut child = Command::new(std::env::current_exe()?)
            .arg(HOST_ARGUMENT)
            .arg(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let mut connected = None;
        for _ in 0..200 {
            if let Some(status) = child.try_wait()? {
                bail!("native host exited during bootstrap: {status}");
            }
            if let Ok(client) = Self::discover(root) {
                if client.ping().await.is_ok() {
                    connected = Some(client);
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        drop(lock);
        match connected {
            Some(client) => Ok(client),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("native host startup failed");
            }
        }
    }

    pub fn discover(root: &Path) -> Result<Self> {
        let discovery: Discovery =
            serde_json::from_slice(&read_private(&root.join("discovery.json"), 16 * 1024)?)?;
        ensure!(
            discovery.protocol == PROTOCOL,
            "incompatible resident core protocol"
        );
        ensure!(
            discovery.credential.len() == 64
                && discovery.credential.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid discovery credential"
        );
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .build()?;
        Ok(Self { http, discovery })
    }
    pub async fn request(&self, request: &HostRequest) -> Result<Value> {
        // No mutation response timeout: accepted lane work is never represented
        // as cancelled. A transport failure is uncertain and includes identity.
        let identity = match request {
            HostRequest::Execute { request_id, .. } => request_id.as_str(),
            HostRequest::Materialize { intent } => intent.request_id.as_str(),
            _ => "host-control",
        };
        let body = serde_json::to_vec(request)?;
        ensure!(
            body.len() <= MAX_REQUEST_BYTES,
            "request exceeds admission limit"
        );
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let time = auth::timestamp()?;
        let signature = auth::sign(&self.discovery.credential, "request", &nonce, time, &body);
        let mut response = self
            .http
            .post(format!("http://127.0.0.1:{}/", self.discovery.port))
            .header("content-type", "application/json")
            .header("x-asp-nonce", &nonce)
            .header("x-asp-time", time)
            .header("x-asp-signature", signature)
            .body(body)
            .send()
            .await
            .with_context(|| {
                format!("transport outcome unknown for {identity}; reconnect and reconcile")
            })?;
        let status = response.status();
        let signature = response
            .headers()
            .get("x-asp-signature")
            .and_then(|value| value.to_str().ok())
            .context("unauthenticated host response; outcome unknown")?
            .to_owned();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .with_context(|| format!("response outcome unknown for {identity}"))?
        {
            ensure!(
                bytes.len() + chunk.len() <= 32 * 1024 * 1024,
                "host response exceeds limit; outcome unknown for {identity}"
            );
            bytes.extend_from_slice(&chunk);
        }
        auth::verify(
            &self.discovery.credential,
            "response",
            &nonce,
            status.as_u16() as u64,
            &bytes,
            &signature,
        )
        .with_context(|| format!("untrusted host response; outcome unknown for {identity}"))?;
        let value: Value = serde_json::from_slice(&bytes)?;
        ensure!(status.is_success(), "native host rejected request: {value}");
        Ok(value)
    }
    pub async fn ping(&self) -> Result<Value> {
        let value = tokio::time::timeout(Duration::from_secs(2), self.request(&HostRequest::Ping))
            .await??;
        ensure!(
            value["protocol"] == PROTOCOL && value["instance"] == self.discovery.instance,
            "native host identity mismatch"
        );
        Ok(value)
    }
    pub async fn execute(
        &self,
        resource_id: String,
        request_id: String,
        operation: String,
        payload: Value,
    ) -> Result<std::result::Result<CanonicalResponse, crate::operations::CanonicalErrorEnvelope>>
    {
        let value = self
            .request(&HostRequest::Execute {
                resource_id,
                request_id,
                operation,
                payload,
            })
            .await?;
        if let Some(error) = value.get("canonical_error") {
            return Ok(Err(serde_json::from_value(error.clone())?));
        }
        Ok(Ok(serde_json::from_value(value["response"].clone())?))
    }
}

/// Resolve the per-workspace host. An explicit override is intended for managed
/// deployments/tests and must already satisfy the private-root contract.
pub fn workspace_root(workspace: &Path) -> Result<PathBuf> {
    if let Some(root) = std::env::var_os("ASP_RESIDENT_ROOT") {
        let root = PathBuf::from(root);
        check_private(&root, true)?;
        NativeBindings::open(root.join("resources"))?;
        return Ok(root);
    }
    let home = PathBuf::from(std::env::var_os("HOME").context("private user home unavailable")?);
    let parent = home.join(".agent-spreadsheet-resident");
    match create_private_directory(&parent) {
        Ok(()) => (),
        Err(error) if parent.is_dir() => check_private(&parent, true).context(error)?,
        Err(error) => return Err(error),
    }
    let canonical = workspace.canonicalize()?;
    let hash = crate::utils::hash_bytes_sha256_hex(canonical.to_string_lossy().as_bytes());
    provision_root(&parent, &hash.replace(':', "_"))
}

/// Provision under an existing private parent. The caller must select a trusted
/// user-owned parent; arbitrary workspace roots are not treated as private.
pub fn provision_root(parent: &Path, name: &str) -> Result<PathBuf> {
    check_private(parent, true)?;
    ensure!(
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "invalid host root name"
    );
    let root = parent.join(name);
    match create_private_directory(&root) {
        Ok(()) => (),
        Err(error) if root.is_dir() => {
            check_private(&root, true).context(error)?;
        }
        Err(error) => return Err(error),
    }
    let resources = root.join("resources");
    match create_private_directory(&resources) {
        Ok(()) => (),
        Err(error) if resources.is_dir() => {
            check_private(&resources, true).context(error)?;
        }
        Err(error) => return Err(error),
    }
    sync_directory(&root)?;
    Ok(root)
}
