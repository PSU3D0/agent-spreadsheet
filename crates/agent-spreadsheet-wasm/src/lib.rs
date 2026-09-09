//! Byte-session bindings over the shared resident document, evaluator and journal.
mod compat;
mod runtime;
mod storage;
use agent_spreadsheet::{
    config::{OutputProfile, RecalcBackendKind, ServerConfig, TransportKind},
    core::session::{SessionApplySummary, SessionTransformOp},
    model::*,
    operations::{
        CanonicalErrorCode, CanonicalErrorEnvelope, CanonicalResponse, OperationAdapter,
        ResourceId, RuntimeCapabilities, SpreadsheetOperation, decode_operation,
        is_canonical_operation_name, operation_descriptor, operations_discovery_for,
    },
};
pub use compat::*;
use runtime::StoredSession;
#[cfg(target_arch = "wasm32")]
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    cell::{RefCell, RefMut},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

const MAX_TOTAL_WORKBOOK_BYTES: usize = 256 * 1024 * 1024;
struct ArtifactSlot {
    handle: String,
    bytes: Vec<u8>,
    last_used: u64,
}
#[derive(Default)]
struct SessionStore {
    sessions: HashMap<String, StoredSession>,
    workbook_bytes: HashMap<String, usize>,
    artifacts: HashMap<String, Vec<ArtifactSlot>>,
    artifact_bytes: usize,
    artifact_clock: u64,
}
impl SessionStore {
    fn tick(&mut self) -> u64 {
        self.artifact_clock += 1;
        self.artifact_clock
    }
    fn drop_artifacts(&mut self, session: &str) {
        if let Some(slots) = self.artifacts.remove(session) {
            self.artifact_bytes -= slots.iter().map(|slot| slot.bytes.len()).sum::<usize>();
        }
    }
    fn remove_artifact(&mut self, session: &str, handle: &str) -> bool {
        let Some(slots) = self.artifacts.get_mut(session) else {
            return false;
        };
        let Some(index) = slots.iter().position(|slot| slot.handle == handle) else {
            return false;
        };
        self.artifact_bytes -= slots.remove(index).bytes.len();
        if slots.is_empty() {
            self.artifacts.remove(session);
        }
        true
    }
    fn insert_artifact(&mut self, session: &str, handle: String, bytes: Vec<u8>) {
        let now = self.tick();
        let slots = self.artifacts.entry(session.to_string()).or_default();
        if let Some(slot) = slots.iter_mut().find(|slot| slot.handle == handle) {
            slot.last_used = now;
            return;
        }
        if slots.len() >= MAX_ARTIFACTS_PER_SESSION {
            let index = slots
                .iter()
                .enumerate()
                .min_by_key(|(_, slot)| slot.last_used)
                .unwrap()
                .0;
            self.artifact_bytes -= slots.remove(index).bytes.len();
        }
        while self.artifact_bytes.saturating_add(bytes.len()) > MAX_TOTAL_ARTIFACT_BYTES {
            let victim = self
                .artifacts
                .iter()
                .flat_map(|(session, slots)| {
                    slots
                        .iter()
                        .map(move |slot| (session.clone(), slot.handle.clone(), slot.last_used))
                })
                .min_by_key(|(_, _, clock)| *clock);
            let Some((session, handle, _)) = victim else {
                break;
            };
            self.remove_artifact(&session, &handle);
        }
        self.artifact_bytes += bytes.len();
        self.artifacts
            .entry(session.to_string())
            .or_default()
            .push(ArtifactSlot {
                handle,
                bytes,
                last_used: now,
            });
    }
    fn read_artifact(&mut self, session: &str, handle: &str) -> Option<Vec<u8>> {
        let now = self.tick();
        let slot = self
            .artifacts
            .get_mut(session)?
            .iter_mut()
            .find(|slot| slot.handle == handle)?;
        slot.last_used = now;
        Some(slot.bytes.clone())
    }
}

/// Cloneable within one owner thread. It is intentionally neither Send nor Sync.
#[derive(Clone, Default)]
pub struct SessionApi {
    store: Rc<RefCell<SessionStore>>,
}
fn wasm_config() -> Arc<ServerConfig> {
    Arc::new(ServerConfig {
        workspace_root: PathBuf::new(),
        screenshot_dir: PathBuf::new(),
        path_mappings: Vec::new(),
        cache_capacity: 1,
        supported_extensions: vec!["xlsx".into(), "xlsm".into()],
        single_workbook: None,
        enabled_tools: None,
        transport: TransportKind::Stdio,
        http_bind_address: "127.0.0.1:0".parse().unwrap(),
        recalc_enabled: false,
        recalc_backend: RecalcBackendKind::Formualizer,
        vba_enabled: false,
        max_concurrent_recalcs: 1,
        tool_timeout_ms: None,
        max_response_bytes: Some(MAX_PARAMS_JSON_BYTES as u64),
        output_profile: OutputProfile::TokenDense,
        max_payload_bytes: Some(MAX_PARAMS_JSON_BYTES as u64),
        max_cells: Some(10_000),
        max_items: Some(500),
        allow_overwrite: false,
        slim_surface: true,
    })
}
#[cfg(all(feature = "render", target_arch = "wasm32"))]
fn now_ms() -> f64 {
    js_sys::Date::now()
}
#[cfg(all(feature = "render", not(target_arch = "wasm32")))]
fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}
fn adapter_capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities {
        // Memory journals retain history for the owner's lifetime, not across reloads.
        resident_history: true,
        workbook_discovery: false,
        workbook_read: true,
        workbook_write: true,
        screenshot_rendering: cfg!(feature = "render"),
        sheetport: false,
        vba: false,
    }
}
fn invalid(error: impl std::fmt::Display) -> SessionApiError {
    SessionApiError::InvalidArgument {
        message: error.to_string(),
    }
}
fn canonical_error(
    code: CanonicalErrorCode,
    operation: &str,
    message: impl Into<String>,
) -> CanonicalErrorEnvelope {
    CanonicalErrorEnvelope::new(code, message, Some(operation), None)
}
fn operation_error(operation: &str, error: impl std::fmt::Display) -> CanonicalErrorEnvelope {
    canonical_error(
        CanonicalErrorCode::OperationFailed,
        operation,
        error.to_string(),
    )
}
macro_rules! legacy_projection {
    ($name:ident($($arg:ident: $ty:ty),*) -> $out:ty) => {
        pub fn $name(&self, session_id: &str, $($arg: $ty),*) -> SessionResult<$out> {
            let mut response = self.session(session_id)?.$name($($arg.into()),*).map_err(invalid)?;
            response.workbook_id = WorkbookId(session_id.to_string());
            Ok(response)
        }
    };
}
impl SessionApi {
    pub fn new() -> Self {
        Self::default()
    }
    fn lock_store(&self) -> SessionResult<RefMut<'_, SessionStore>> {
        self.store
            .try_borrow_mut()
            .map_err(|_| SessionApiError::Internal {
                message: "session registry is busy".into(),
            })
    }
    fn session(&self, id: &str) -> SessionResult<StoredSession> {
        self.lock_store()?.sessions.get(id).cloned().ok_or_else(|| {
            SessionApiError::SessionNotFound {
                session_id: id.into(),
            }
        })
    }
    pub fn create_session(&self, bytes: &[u8]) -> SessionResult<String> {
        let mut store = self.lock_store()?;
        if bytes.len() > MAX_WORKBOOK_BYTES {
            return Err(invalid(format!(
                "workbook exceeds the {MAX_WORKBOOK_BYTES}-byte session limit"
            )));
        }
        if store.sessions.len() >= MAX_SESSIONS {
            return Err(invalid(format!("session limit of {MAX_SESSIONS} reached")));
        }
        if store
            .workbook_bytes
            .values()
            .sum::<usize>()
            .saturating_add(bytes.len())
            > MAX_TOTAL_WORKBOOK_BYTES
        {
            return Err(invalid("total workbook session memory limit exceeded"));
        }
        let id = format!(
            "session:{}",
            agent_spreadsheet::utils::make_short_random_id("wasm", 24)
        );
        let resource: ResourceId = serde_json::from_value(json!(id)).map_err(invalid)?;
        let owner = StoredSession::new(resource, bytes).map_err(invalid)?;
        store.sessions.insert(id.clone(), owner);
        store.workbook_bytes.insert(id.clone(), bytes.len());
        Ok(id)
    }
    pub fn session_metadata(&self, id: &str) -> SessionResult<Value> {
        self.session(id)?.metadata().map_err(invalid)
    }
    pub fn operations_json(&self) -> SessionResult<String> {
        serde_json::to_string(&operations_discovery_for(
            OperationAdapter::Wasm,
            &adapter_capabilities(),
        ))
        .map_err(invalid)
    }
    pub async fn execute_operation(
        &self,
        session_id: &str,
        operation: &str,
        params: &str,
    ) -> Result<String, CanonicalErrorEnvelope> {
        let request_id = agent_spreadsheet::utils::make_short_random_id("wasm-request", 24);
        self.execute_operation_with_request_id(session_id, operation, params, &request_id)
            .await
    }
    pub async fn execute_operation_with_request_id(
        &self,
        session_id: &str,
        name: &str,
        params_json: &str,
        request_id: &str,
    ) -> Result<String, CanonicalErrorEnvelope> {
        if request_id.is_empty()
            || request_id.len() > 256
            || params_json.len() > MAX_PARAMS_JSON_BYTES
        {
            return Err(canonical_error(
                CanonicalErrorCode::InvalidRequest,
                name,
                "request ID must be 1–256 bytes and params must fit the session limit",
            ));
        }
        if (is_canonical_operation_name(name) && operation_descriptor(name).is_none())
            || operation_descriptor(name).is_some_and(|descriptor| {
                !descriptor.is_available_for(OperationAdapter::Wasm, &adapter_capabilities())
            })
        {
            return Err(canonical_error(
                CanonicalErrorCode::CapabilityUnavailable,
                name,
                format!("operation '{name}' is unavailable in this runtime"),
            ));
        }
        let mut params: Value = serde_json::from_str(params_json).map_err(|error| {
            canonical_error(CanonicalErrorCode::InvalidRequest, name, error.to_string())
        })?;
        let object = params.as_object_mut().ok_or_else(|| {
            canonical_error(
                CanonicalErrorCode::InvalidRequest,
                name,
                "params JSON must be an object",
            )
        })?;
        match object.get("resource_id") {
            None => {
                object.insert("resource_id".into(), json!(session_id));
            }
            Some(Value::String(id)) if id == session_id => {}
            _ => {
                return Err(canonical_error(
                    CanonicalErrorCode::InvalidRequest,
                    name,
                    "params resource_id must match sessionId",
                ));
            }
        }
        let operation = decode_operation(name, params)?;
        let owner = self.session(session_id).map_err(|error| {
            canonical_error(
                CanonicalErrorCode::ResourceNotFound,
                name,
                error.to_string(),
            )
        })?;
        #[cfg(feature = "render")]
        if let SpreadsheetOperation::ScreenshotSheet(request) = operation {
            return self
                .screenshot(session_id, request)
                .await
                .and_then(|response| {
                    serde_json::to_string(&response).map_err(|error| operation_error(name, error))
                });
        }
        let baseline = if let SpreadsheetOperation::VerifyWorkbook(ref request) = operation {
            if request.baseline_resource_id.as_str() != session_id {
                let (revision, bytes) = self
                    .session(request.baseline_resource_id.as_str())
                    .map_err(|error| {
                        canonical_error(
                            CanonicalErrorCode::ResourceNotFound,
                            name,
                            error.to_string(),
                        )
                    })?
                    .capture()
                    .map_err(|error| operation_error(name, error))?;
                Some((
                    request.baseline_resource_id.clone(),
                    agent_spreadsheet::canonical_lifecycle::VerificationSnapshot {
                        revision,
                        bytes,
                    },
                ))
            } else {
                None
            }
        } else {
            None
        };
        let response = owner
            .execute_with_baseline(request_id, operation, baseline)
            .await?;
        serde_json::to_string(&response).map_err(|error| operation_error(name, error))
    }
    pub fn list_sheets(&self, id: &str) -> SessionResult<Vec<String>> {
        self.session(id)?.list_sheets().map_err(invalid)
    }
    legacy_projection!(describe_workbook() -> WorkbookDescription);
    legacy_projection!(named_ranges() -> NamedRangesResponse);
    legacy_projection!(sheet_overview(params: SheetOverviewParams) -> SheetOverviewResponse);
    legacy_projection!(find_value(params: FindValueParams) -> FindValueResponse);
    legacy_projection!(read_table(params: ReadTableParams) -> ReadTableResponse);
    legacy_projection!(sheet_page(params: SheetPageParams) -> SheetPageResponse);
    pub fn range_values(
        &self,
        id: &str,
        params: RangeValuesParams,
    ) -> SessionResult<RangeValuesResult> {
        let values = self
            .session(id)?
            .range_values(
                &params.sheet_name,
                agent_spreadsheet::core::session::SessionRangeSelection::from(params.ranges),
            )
            .map_err(invalid)?;
        Ok(RangeValuesResult {
            sheet_name: params.sheet_name,
            values,
        })
    }
    pub fn grid_export(&self, id: &str, params: GridExportParams) -> SessionResult<GridPayload> {
        self.session(id)?
            .grid_export(&params.sheet_name, &params.range)
            .map_err(invalid)
    }
    pub fn define_name(
        &self,
        id: &str,
        name: &str,
        refers_to: &str,
        scope: Option<&str>,
        scope_sheet_name: Option<&str>,
    ) -> SessionResult<DefineNameResponse> {
        let mut response = self
            .session(id)?
            .define_name(name, refers_to, scope, scope_sheet_name)
            .map_err(invalid)?;
        response.workbook_id = WorkbookId(id.into());
        Ok(response)
    }
    pub fn update_name(
        &self,
        id: &str,
        name: &str,
        refers_to: Option<&str>,
        scope: Option<&str>,
        scope_sheet_name: Option<&str>,
    ) -> SessionResult<UpdateNameResponse> {
        let mut response = self
            .session(id)?
            .update_name(name, refers_to, scope, scope_sheet_name)
            .map_err(invalid)?;
        response.workbook_id = WorkbookId(id.into());
        Ok(response)
    }
    pub fn delete_name(
        &self,
        id: &str,
        name: &str,
        scope: Option<&str>,
        scope_sheet_name: Option<&str>,
    ) -> SessionResult<DeleteNameResponse> {
        let mut response = self
            .session(id)?
            .delete_name(name, scope, scope_sheet_name)
            .map_err(invalid)?;
        response.workbook_id = WorkbookId(id.into());
        Ok(response)
    }
    pub fn transform_batch(
        &self,
        id: &str,
        ops: Vec<SessionTransformOp>,
        options: TransformBatchOptions,
    ) -> SessionResult<SessionApplySummary> {
        if ops.is_empty() {
            return Err(invalid("at least one transform op is required"));
        }
        self.session(id)?
            .apply_ops(&ops, options.dry_run)
            .map_err(invalid)
    }
    pub fn export_workbook(&self, id: &str) -> SessionResult<Vec<u8>> {
        let (_, bytes) = self.session(id)?.capture().map_err(invalid)?;
        if bytes.len() > MAX_WORKBOOK_BYTES {
            return Err(invalid("export exceeds workbook size limit"));
        }
        Ok(bytes)
    }
    pub fn dispose_session(&self, id: &str) -> SessionResult<bool> {
        let mut store = self.lock_store()?;
        let removed = store.sessions.remove(id).is_some();
        store.workbook_bytes.remove(id);
        store.drop_artifacts(id);
        Ok(removed)
    }
    pub fn read_artifact(&self, id: &str, handle: &str) -> Result<Vec<u8>, CanonicalErrorEnvelope> {
        self.session(id).map_err(|error| {
            canonical_error(
                CanonicalErrorCode::ResourceNotFound,
                "read_artifact",
                error.to_string(),
            )
        })?;
        self.lock_store()
            .map_err(|error| operation_error("read_artifact", error))?
            .read_artifact(id, handle)
            .ok_or_else(|| {
                canonical_error(
                    CanonicalErrorCode::ResourceNotFound,
                    "read_artifact",
                    "artifact is missing, evicted, or belongs to another session",
                )
            })
    }
    pub fn dispose_artifact(&self, id: &str, handle: &str) -> Result<bool, CanonicalErrorEnvelope> {
        Ok(self
            .lock_store()
            .map_err(|error| operation_error("dispose_artifact", error))?
            .remove_artifact(id, handle))
    }
    #[cfg(feature = "render")]
    async fn screenshot(
        &self,
        id: &str,
        request: agent_spreadsheet::canonical_optional::ScreenshotSheetRequest,
    ) -> Result<CanonicalResponse, CanonicalErrorEnvelope> {
        use agent_spreadsheet::{
            canonical_optional::*,
            repository::{VirtualWorkbookInput, VirtualWorkspaceRepository},
            state::AppState,
        };
        let name = "screenshot_sheet";
        let started = now_ms();
        validate_screenshot_request(&request).map_err(|error| operation_error(name, error))?;
        if matches!(request.backend, Some(ScreenshotBackend::Libreoffice)) {
            return Err(canonical_error(
                CanonicalErrorCode::CapabilityUnavailable,
                name,
                "LibreOffice requires a native process host",
            ));
        }
        // Explicit optional cold snapshot; ordinary reads/calculations never use it.
        let (revision, bytes, coverage) = self
            .session(id)
            .map_err(|error| operation_error(name, error))?
            .capture_with_coverage()
            .map_err(|error| operation_error(name, error))?;
        let styles = agent_spreadsheet::render::styles_xml_from_bytes(&bytes);
        let config = wasm_config();
        let repository = Arc::new(VirtualWorkspaceRepository::new(config.clone()));
        let workbook_id = repository.register(VirtualWorkbookInput {
            key: "render.xlsx".into(),
            slug: None,
            bytes,
        });
        let state = Arc::new(AppState::new_with_repository(config, repository));
        let workbook = state
            .open_workbook(&workbook_id)
            .await
            .map_err(|error| operation_error(name, error))?;
        let range = request
            .range
            .as_deref()
            .unwrap_or(DEFAULT_SCREENSHOT_RANGE)
            .to_string();
        let level = request.png_level.unwrap_or(ScreenshotPngLevel::Balanced);
        let rendered = agent_spreadsheet::render::render_sheet_with_styles(
            &workbook,
            &request.sheet_name,
            &range,
            map_png_level(level),
            styles.as_deref(),
        )
        .map_err(|error| operation_error(name, error))?;
        if rendered.png.len() > max_screenshot_bytes()
            || rendered.png.len() > MAX_TOTAL_ARTIFACT_BYTES
        {
            return Err(canonical_error(
                CanonicalErrorCode::CapabilityUnavailable,
                name,
                "screenshot artifact exceeds the size limit",
            ));
        }
        let hash = format!(
            "sha256:{}",
            agent_spreadsheet::utils::hash_bytes_sha256_hex(&rendered.png)
        );
        let artifact = ArtifactHandle {
            handle: format!("artifact:{hash}"),
            hash,
            bytes: rendered.png.len() as u64,
            media_type: "image/png".into(),
        };
        self.lock_store()
            .map_err(|error| operation_error(name, error))?
            .insert_artifact(id, artifact.handle.clone(), rendered.png.clone());
        let mut data = screenshot_data_from_render(
            request.sheet_name,
            range,
            artifact,
            (now_ms() - started).max(0.0) as u64,
            level,
            &rendered,
            calculation_for(&state, &workbook),
        );
        data.calculation.revision_id = revision.clone();
        data.calculation.state = coverage.state();
        Ok(CanonicalResponse {
            schema_version: "1".into(),
            operation: name.into(),
            resource_id: Some(
                serde_json::from_value(json!(id)).map_err(|error| operation_error(name, error))?,
            ),
            revision_id: Some(revision),
            data: serde_json::to_value(data).map_err(|error| operation_error(name, error))?,
        })
    }
}
include!("bindings.rs");
