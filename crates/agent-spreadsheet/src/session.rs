//! Portable owner backend for the one canonical dispatcher. Hosts serialize
//! this !Send value on its owning lane and supply asynchronous persistence.
#![cfg(feature = "recalc-formualizer")]
use crate::{
    canonical_lifecycle::*,
    canonical_outcome::OutcomeStorage,
    canonical_write::{self, ResidentWriteSession, WriteRequest, WriteResponseData},
    config::{RecalcBackendKind, ServerConfig},
    core::resident_storage::{
        PreparedResidentCommit, ResidentCheckpoint, ResidentCommitStorage, ResidentTransition,
    },
    execution_context::ExecutionContext,
    operations::{CanonicalErrorEnvelope, CanonicalResponse, ResourceId, RuntimeCapabilities},
    read_context::BorrowedReadContext,
};
use anyhow::{Result, anyhow, bail};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{cell::RefCell, sync::Arc};

/// Host-owned artifact publication; no workspace paths or filesystem policy in
/// the resident core. Implementations return only after their required barriers.
#[async_trait::async_trait(?Send)]
pub trait ResidentArtifactSink {
    async fn validate(&self, request: &ExportForkRequest) -> Result<()>;
    async fn publish(&self, request: &ExportForkRequest, bytes: &[u8]) -> Result<ArtifactMetadata>;
}

pub struct ResidentSessionRuntime<S> {
    pub owner: ResidentWriteSession,
    pub storage: S,
    pub config: Arc<ServerConfig>,
    resource: ResourceId,
    request_id: String,
    outcome: RefCell<Option<CanonicalResponse>>,
    poisoned_request_id: Option<String>,
    artifact_sink: Option<Box<dyn ResidentArtifactSink>>,
    discarded: Option<bool>,
    verification_baseline: Option<(ResourceId, VerificationSnapshot)>,
}
impl<S: ResidentCommitStorage> ResidentSessionRuntime<S> {
    pub fn new(
        owner: ResidentWriteSession,
        storage: S,
        config: Arc<ServerConfig>,
        resource: ResourceId,
    ) -> Result<Self> {
        if !resource.as_str().starts_with("session:") && !resource.as_str().starts_with("fork:") {
            bail!("resident binding requires a session: or fork: identity");
        }
        if owner.read_view()?.id != resource.to_workbook_id() {
            bail!("resident binding identity mismatch");
        }
        Ok(Self {
            owner,
            storage,
            config,
            resource,
            request_id: String::new(),
            outcome: RefCell::new(None),
            poisoned_request_id: None,
            artifact_sink: None,
            discarded: None,
            verification_baseline: None,
        })
    }
    pub fn with_artifact_sink(mut self, sink: impl ResidentArtifactSink + 'static) -> Self {
        self.artifact_sink = Some(Box::new(sink));
        self
    }

    /// Explicit optional-operation reparse. This state owns only a disposable
    /// snapshot; it never receives a native resource ID or mutation authority.
    #[cfg(feature = "native-fs")]
    fn optional_snapshot(&self) -> Result<(crate::hostfs::TempDir, Arc<crate::state::AppState>, ResourceId)> {
        self.owner.read_view()?;
        let directory = crate::hostfs::tempdir()?;
        std::fs::write(directory.path().join("session.xlsx"), self.owner.diagnostic_workbook().snapshot_bytes()?)?;
        let mut config = (*self.config).clone();
        config.workspace_root = directory.path().to_path_buf();
        config.screenshot_dir = directory.path().join("screenshots");
        config.single_workbook = None;
        config.path_mappings.clear();
        let state = Arc::new(crate::state::AppState::new(Arc::new(config)));
        let workbooks = state.list_workbooks(crate::tools::filters::WorkbookFilter::default())?;
        let id = workbooks.workbooks.first().ok_or_else(|| anyhow!("optional snapshot missing"))?.workbook_id.clone();
        let resource = ResourceId::bind_workbook(&id).map_err(anyhow::Error::msg)?;
        Ok((directory, state, resource))
    }

    /// Explicit cold child-fork capture at one coherent retained revision.
    pub async fn capture_fork_base(&self, expected: &str) -> Result<Vec<u8>> {
        if self.catalog_descriptor().await?.is_none() { bail!("resource has been discarded"); }
        self.owner.read_view()?;
        if self.owner.revision() != expected { bail!("revision conflict: parent fork changed"); }
        self.owner.diagnostic_workbook().snapshot_bytes()
    }

    pub fn is_discarded(&self) -> bool { self.discarded == Some(true) }

    pub async fn catalog_descriptor(&self) -> Result<Option<CanonicalForkDescriptor>> {
        let records = self.storage.load(&self.session_id()).await?;
        let live = self.owner.poison_reason().is_none().then(|| (self.owner.revision(), self.dirty()));
        resident_catalog_descriptor(&self.resource, &records, live)
    }

    /// Prepare the original creation response in the initial journal receipt.
    /// The host must publish the immutable binding and keep this owner alive
    /// before acknowledging it; this does not itself activate a host resource.
    pub async fn prepare_creation(
        &mut self,
        request_id: &str,
        request: CreateForkRequest,
    ) -> Result<CanonicalResponse> {
        let input_sha256 = fingerprint("create_fork", &request)?;
        let resource = self.resource.clone();
        let outcome = RefCell::new(None);
        let storage = OutcomeStorage {
            inner: &self.storage, request_id, input_sha256,
            outcome: &outcome,
            prepare: |record: &PreparedResidentCommit| envelope("create_fork", &resource,
                record, CreateForkData {
                    label: request.label.clone(),
                    base_resource_id: request.resource_id.clone(),
                    base_revision_id: request.expected_revision.clone(),
                    fork_resource_id: resource.clone(),
                    revision_id: record.state_revision.clone(),
                    ttl_seconds: None, warnings: Vec::new(),
                }),
        };
        canonical_write::commit_creation_receipt(&mut self.owner, &storage, request_id, &request).await?;
        outcome.into_inner().ok_or_else(|| anyhow!("creation outcome was not prepared"))
    }

    /// Adapter-supplied foreign baseline is scoped to this single cold call.
    pub async fn execute_with_verification_baseline(
        &mut self,
        request_id: &str,
        operation: crate::operations::SpreadsheetOperation,
        baseline: Option<(ResourceId, VerificationSnapshot)>,
    ) -> Result<CanonicalResponse, CanonicalErrorEnvelope> {
        self.verification_baseline = baseline;
        let result = self.execute(request_id, operation).await;
        self.verification_baseline = None;
        result
    }

    pub async fn execute(
        &mut self,
        request_id: &str,
        operation: crate::operations::SpreadsheetOperation,
    ) -> Result<CanonicalResponse, CanonicalErrorEnvelope> {
        if self.discarded.is_none() {
            let state = async {
                let records = self.storage.load(&self.session_id()).await?;
                if records.is_empty() { return Ok::<_, anyhow::Error>(false); }
                Ok(crate::core::resident_storage::PortableHistoryState::replay(&records)?.discarded)
            }.await.map_err(|error| CanonicalErrorEnvelope::new(
                crate::operations::CanonicalErrorCode::RecoveryRequired, error.to_string(), Some(operation.name()), None,
            ))?;
            self.discarded = Some(state);
        }
        if let crate::operations::SpreadsheetOperation::DiscardFork(ref request) = operation {
            if let Some(response) = reconcile_lifecycle_outcome(&self.storage, &self.resource, request_id, "discard_fork", request)
                .await.map_err(|error| crate::operations::lifecycle_error("discard_fork", error))? {
                self.discarded = Some(true);
                return Ok(response);
            }
        }
        let outcome_query = matches!(&operation, crate::operations::SpreadsheetOperation::SessionHistory(
            crate::session_history::SessionHistoryRequest::Outcome { .. }
        ));
        if self.discarded == Some(true) && !outcome_query {
            // Historical export success is still historical success; replaying
            // its receipt neither accesses the workbook nor republishes bytes.
            if let crate::operations::SpreadsheetOperation::ExportFork(ref request) = operation {
                if let Some(response) = reconcile_lifecycle_outcome(&self.storage, &self.resource, request_id, "export_fork", request)
                    .await.map_err(|error| crate::operations::lifecycle_error("export_fork", error))? {
                    return Ok(response);
                }
            }
            return Err(CanonicalErrorEnvelope::new(
                crate::operations::CanonicalErrorCode::ResourceNotFound,
                "resource has been discarded; only retained outcome reconciliation is available",
                Some(operation.name()), None,
            ));
        }
        if self.owner.poison_reason().is_some() && !operation.is_owner_diagnostic() {
            return Err(CanonicalErrorEnvelope::new(
                crate::operations::CanonicalErrorCode::RecoveryRequired,
                "resident session requires recovery",
                Some(operation.name()),
                None,
            ));
        }
        if request_id.is_empty() {
            return Err(CanonicalErrorEnvelope::operation_failed(
                operation.name(),
                "request identity is required".into(),
            ));
        }
        self.request_id = request_id.to_owned();
        *self.outcome.borrow_mut() = None;
        let result = crate::operations::execute_operation(&mut *self, operation).await;
        match result {
            Ok(response) => Ok(self.outcome.borrow().clone().unwrap_or(response)),
            Err(mut error) => {
                if let Some(reason) = self.owner.poison_reason() {
                    if self.poisoned_request_id.is_none() {
                        self.poisoned_request_id = Some(request_id.into());
                    }
                    error.error.code = if reason.contains("outcome unknown") {
                        crate::operations::CanonicalErrorCode::OutcomeUnknown
                    } else {
                        crate::operations::CanonicalErrorCode::RecoveryRequired
                    };
                    error.error.message = format!(
                        "{}; resident requires recovery: {reason}",
                        error.error.message
                    );
                }
                Err(error)
            }
        }
    }
    fn session_id(&self) -> String {
        self.resource.to_workbook_id().0
    }
    fn core_resource(&self) -> Result<ResourceId> {
        Ok(serde_json::from_value(serde_json::json!(format!(
            "session:{}",
            self.session_id()
        )))?)
    }
    async fn check_revision(&self, expected: &str) -> Result<()> {
        let records = self.storage.load(&self.session_id()).await?;
        if let Some((index, record)) = records
            .iter()
            .enumerate()
            .find(|(_, r)| r.request_id == self.request_id)
        {
            let before = record
                .effects
                .iter()
                .find_map(|e| e.pointer("/canonical_outcome/response/data/revision_before"))
                .and_then(Value::as_str)
                .or_else(|| {
                    record
                        .effects
                        .iter()
                        .find_map(|e| e.pointer("/prepared_transaction/response/revision_before"))
                        .and_then(Value::as_str)
                })
                .or_else(|| {
                    index
                        .checked_sub(1)
                        .map(|i| records[i].state_revision.as_str())
                });
            if before == Some(expected) {
                return Ok(());
            }
            bail!("request identity reuse with different expected revision");
        }
        if self.owner.revision() == expected {
            return Ok(());
        }
        bail!(
            "revision conflict: expected {expected}, current {}",
            self.owner.revision()
        )
    }
    fn dirty(&self) -> bool {
        !matches!(
            self.owner.diagnostic_workbook().calculation_stamp(),
            crate::recalc::CalculationStamp::Current { .. }
        )
    }
    async fn checkpoints(&self) -> Result<Vec<CheckpointDescriptor>> {
        Ok(
            canonical_write::list_checkpoints_durable(&self.owner, &self.storage)
                .await?
                .into_iter()
                .map(checkpoint_descriptor)
                .collect(),
        )
    }
    fn finish<T: DeserializeOwned>(&mut self) -> Result<T> {
        let data = self
            .outcome
            .borrow()
            .as_ref()
            .map(|outcome| outcome.data.clone());
        let result = data
            .ok_or_else(|| anyhow!("committed canonical outcome is unavailable"))
            .and_then(|data| serde_json::from_value(data).map_err(Into::into));
        if let Err(ref error) = result {
            self.owner
                .poison_after_committed_outcome(&error.to_string());
        }
        result
    }
}
/// Portable catalog projection. Hosts may supply a live owner revision, but
/// persisted state revisions are never promoted into current CAS tokens.
pub fn resident_catalog_descriptor(
    resource: &ResourceId, records: &[PreparedResidentCommit], live: Option<(String, bool)>,
) -> Result<Option<CanonicalForkDescriptor>> {
    let state = if records.is_empty() { None } else {
        let state = crate::core::resident_storage::PortableHistoryState::replay(records)?;
        if state.session_id != resource.to_workbook_id().0 { bail!("catalog resource identity mismatch"); }
        if state.discarded { return Ok(None); }
        Some(state)
    };
    let operation_count = records.iter().filter(|record| {
        record.transition == ResidentTransition::CalculationPublish || record.effects.iter().any(|effect| {
            effect.pointer("/prepared_transaction/response/mode").and_then(Value::as_str) == Some("apply")
        })
    }).count();
    Ok(Some(CanonicalForkDescriptor {
        resource_id: resource.clone(), revision_id: live.as_ref().map(|(revision, _)| revision.clone()),
        age_seconds: None, operation_count,
        staged_change_count: state.as_ref().map_or(0, |state| state.staged_change_count()),
        checkpoint_count: state.as_ref().map_or(0, |state| state.checkpoints.len()),
        recalc_needed: live.is_none_or(|(_, dirty)| dirty),
    }))
}

/// Read the original creation outcome through the same validated journal boundary.
pub async fn reconcile_creation<S: ResidentCommitStorage>(
    storage: &S, resource: &ResourceId, base_sha256: &str, request_id: &str, request: &CreateForkRequest,
) -> Result<CanonicalResponse> {
    let outcome = RefCell::new(None);
    let retained = OutcomeStorage {
        inner: storage, request_id, input_sha256: fingerprint("create_fork", request)?,
        outcome: &outcome,
        prepare: |_: &PreparedResidentCommit| -> Result<CanonicalResponse> { bail!("read-only creation reconciliation") },
    };
    let session_id = resource.to_workbook_id().0;
    let records = retained.load(&session_id).await?;
    crate::core::resident_storage::PortableHistoryState::replay_bound(&records, &session_id, base_sha256)?;
    let record = records.iter().find(|record| record.request_id == request_id)
        .ok_or_else(|| anyhow!("creation outcome missing; recovery required"))?;
    if !matches!(retained.reconcile(&session_id, request_id, &record.request_fingerprint).await?,
        crate::core::resident_storage::ReconcileOutcome::Committed(_)) {
        bail!("creation outcome unknown during reconciliation");
    }
    let response = outcome.into_inner().ok_or_else(|| anyhow!("creation outcome missing; recovery required"))?;
    if response.operation != "create_fork" || response.resource_id.as_ref() != Some(resource) {
        bail!("creation outcome binding mismatch");
    }
    Ok(response)
}

/// Reconcile a lifecycle receipt without borrowing or resurrecting a workbook.
pub async fn reconcile_lifecycle_outcome<S: ResidentCommitStorage, T: Serialize>(
    storage: &S, resource: &ResourceId, request_id: &str, operation: &str, request: &T,
) -> Result<Option<CanonicalResponse>> {
    let outcome = RefCell::new(None);
    let retained = OutcomeStorage {
        inner: storage, request_id, input_sha256: fingerprint(operation, request)?, outcome: &outcome,
        prepare: |_: &PreparedResidentCommit| -> Result<CanonicalResponse> { bail!("read-only lifecycle reconciliation") },
    };
    let session_id = resource.to_workbook_id().0;
    let records = retained.load(&session_id).await?;
    let Some(record) = records.iter().find(|record| record.request_id == request_id) else { return Ok(None); };
    if !matches!(retained.reconcile(&session_id, request_id, &record.request_fingerprint).await?,
        crate::core::resident_storage::ReconcileOutcome::Committed(_)) {
        bail!("lifecycle outcome unknown during reconciliation");
    }
    let response = outcome.into_inner().ok_or_else(|| anyhow!("lifecycle outcome missing; recovery required"))?;
    if response.operation != operation || response.resource_id.as_ref() != Some(resource) {
        bail!("request identity reuse with different lifecycle operation");
    }
    Ok(Some(response))
}

fn fingerprint<T: Serialize>(operation: &str, request: &T) -> Result<String> {
    Ok(crate::utils::hash_bytes_sha256_hex(&serde_json::to_vec(
        &(operation, request),
    )?))
}
fn envelope<T: Serialize>(
    operation: &str,
    resource: &ResourceId,
    record: &PreparedResidentCommit,
    data: T,
) -> Result<CanonicalResponse> {
    Ok(CanonicalResponse {
        schema_version: crate::operations::CANONICAL_SCHEMA_VERSION.into(),
        operation: operation.into(),
        resource_id: Some(resource.clone()),
        revision_id: Some(record.state_revision.clone()),
        data: serde_json::to_value(data)?,
    })
}
fn prepared_response(record: &PreparedResidentCommit) -> Result<WriteResponseData> {
    serde_json::from_value(
        record
            .effects
            .iter()
            .find_map(|e| e.pointer("/prepared_transaction/response"))
            .cloned()
            .ok_or_else(|| anyhow!("prepared canonical write outcome missing"))?,
    )
    .map_err(Into::into)
}
fn checkpoint_descriptor(checkpoint: ResidentCheckpoint) -> CheckpointDescriptor {
    CheckpointDescriptor {
        checkpoint_id: checkpoint.id,
        created_at: None,
        label: checkpoint.label,
        snapshot_revision: checkpoint.state_revision,
        recalc_needed: true,
    }
}
impl<S: ResidentCommitStorage> ExecutionContext for &mut ResidentSessionRuntime<S> {
    type Reads<'a>
        = BorrowedReadContext<'a>
    where
        Self: 'a;
    fn reads(&self) -> Result<Self::Reads<'_>, CanonicalErrorEnvelope> {
        let view = self
            .owner
            .read_view()
            .map_err(|e| CanonicalErrorEnvelope::operation_failed("read", e.to_string()))?;
        Ok(BorrowedReadContext {
            view: Arc::new(view),
            config: self.config.clone(),
        })
    }
    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities {
            resident_history: true,
            workbook_discovery: false,
            workbook_read: true,
            workbook_write: true,
            screenshot_rendering: cfg!(feature = "native-fs") && RuntimeCapabilities::native().screenshot_rendering,
            sheetport: cfg!(all(feature = "native-fs", feature = "sheetport")),
            vba: cfg!(feature = "native-fs"),
        }
    }
    async fn identify(&self, resource: &ResourceId) -> Result<(ResourceId, String)> {
        self.owner.read_view()?;
        if *resource != self.resource {
            bail!("resource does not belong to this resident binding");
        }
        Ok((resource.clone(), self.owner.revision()))
    }
    async fn identify_diagnostic(
        &self,
        resource: &ResourceId,
    ) -> Result<(ResourceId, Option<String>)> {
        if *resource != self.resource {
            bail!("resource does not belong to this resident binding");
        }
        Ok((
            resource.clone(),
            self.owner
                .poison_reason()
                .is_none()
                .then(|| (self.discarded != Some(true)).then(|| self.owner.revision()))
                .flatten(),
        ))
    }

    #[cfg(feature = "native-fs")]
    async fn screenshot_sheet(&mut self, mut request: crate::canonical_optional::ScreenshotSheetRequest) -> Result<crate::canonical_optional::ScreenshotSheetData> {
        self.identify(&request.resource_id).await?;
        let calculation = self.owner.read_view()?.calculation_metadata();
        let (directory, state, resource) = self.optional_snapshot()?;
        request.resource_id = resource;
        let mut data = crate::canonical_optional::screenshot_sheet(state, request).await?;
        let hash = data.artifact.hash.strip_prefix("sha256:").ok_or_else(|| anyhow!("invalid snapshot artifact hash"))?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) { bail!("invalid snapshot artifact hash"); }
        let bytes = std::fs::read(directory.path().join("artifacts").join(format!("{hash}.png")))?;
        data.artifact = crate::canonical_optional::persist_png_artifact(&self.config.workspace_root, &bytes)?;
        data.calculation = calculation;
        Ok(data)
    }

    #[cfg(feature = "native-fs")]
    async fn sheetport_manifest(&mut self, mut request: crate::canonical_optional::SheetportManifestRequest) -> Result<crate::canonical_optional::SheetportManifestData> {
        use crate::canonical_optional::SheetportManifestRequest as Request;
        if let Some(resource) = request.resource_id() { self.identify(resource).await?; }
        let (_directory, state, resource) = self.optional_snapshot()?;
        match &mut request {
            Request::Candidates { resource_id, .. } | Request::BindCheck { resource_id, .. } => *resource_id = resource,
            _ => (),
        }
        crate::canonical_optional::execute_sheetport_manifest_action(state, request).await
    }

    #[cfg(feature = "native-fs")]
    async fn execute_sheetport(&mut self, mut request: crate::canonical_optional::ExecuteSheetportRequest) -> Result<crate::canonical_optional::ExecuteSheetportData> {
        self.identify(&request.resource_id).await?;
        let (_directory, state, resource) = self.optional_snapshot()?;
        request.resource_id = resource;
        crate::canonical_optional::execute_sheetport(state, request).await
    }

    #[cfg(feature = "native-fs")]
    async fn inspect_vba(&mut self, request: crate::canonical_optional::InspectVbaRequest, revision: &str) -> Result<crate::canonical_optional::InspectVbaData> {
        self.identify(request.resource_id()).await?;
        let (_directory, state, resource) = self.optional_snapshot()?;
        crate::canonical_optional::inspect_vba_bound(state, request, revision, Some(resource.to_workbook_id())).await
    }

    async fn verify_workbook(&mut self, request: VerifyWorkbookRequest) -> Result<VerifyWorkbookData> {
        if request.resource_id != self.resource {
            bail!("resource does not belong to this resident binding");
        }
        self.owner.read_view()?;
        let current = VerificationSnapshot {
            bytes: self.owner.diagnostic_workbook().snapshot_bytes()?,
            revision: self.owner.revision(),
        };
        let baseline = if request.baseline_resource_id == self.resource {
            VerificationSnapshot { bytes: current.bytes.clone(), revision: current.revision.clone() }
        } else {
            let (resource, snapshot) = self.verification_baseline.as_ref()
                .ok_or_else(|| anyhow!("verification baseline snapshot binding is required"))?;
            if *resource != request.baseline_resource_id {
                bail!("verification baseline differs from bound snapshot");
            }
            VerificationSnapshot { bytes: snapshot.bytes.clone(), revision: snapshot.revision.clone() }
        };
        verify_workbook_snapshots((*self.config).clone(), request, baseline, current).await
    }

    async fn write(&mut self, mut request: WriteRequest) -> Result<WriteResponseData> {
        let hash = fingerprint("write", &request)?;
        let resource = self.resource.clone();
        request.resource_id = self.core_resource()?;
        let storage = OutcomeStorage {
            inner: &self.storage,
            request_id: &self.request_id,
            input_sha256: hash,
            outcome: &self.outcome,
            prepare: move |record: &PreparedResidentCommit| {
                envelope("write", &resource, record, prepared_response(record)?)
            },
        };
        let response = canonical_write::execute_durable_write_on_resident(
            &mut self.owner,
            &storage,
            &self.request_id,
            request,
        )
        .await?;
        Ok(response)
    }
    async fn recalculate(&mut self, request: RecalculateRequest) -> Result<RecalculateData> {
        let selected = request.backend.unwrap_or(self.config.recalc_backend);
        let backend = if matches!(selected, RecalcBackendKind::Auto | RecalcBackendKind::Formualizer) {
            None
        } else {
            #[cfg(feature = "native-fs")]
            {
                let adapters = crate::state::AppState::new(self.config.clone());
                Some(adapters.recalc_backend(Some(selected))
                    .ok_or_else(|| anyhow!("requested recalc backend not available"))?)
            }
            #[cfg(not(feature = "native-fs"))]
            { bail!("requested recalc backend requires a native filesystem host"); }
        };
        self.check_revision(&request.expected_revision).await?;
        let hash = fingerprint("recalculate", &request)?;
        let resource = self.resource.clone();
        let before = request.expected_revision.clone();
        let storage = OutcomeStorage {
            inner: &self.storage,
            request_id: &self.request_id,
            input_sha256: hash,
            outcome: &self.outcome,
            prepare: move |record: &PreparedResidentCommit| {
                let proof = record
                    .effects
                    .iter()
                    .find_map(|e| e.get("calculation_proof"))
                    .ok_or_else(|| anyhow!("calculation proof missing"))?;
                if let Some(external) = proof.get("external_result") {
                    let data: RecalculateData = serde_json::from_value(external.clone())?;
                    return envelope("recalculate", &resource, record, data);
                }
                let coverage: crate::model::EvaluationCoverage =
                    serde_json::from_value(proof["coverage"].clone())?;
                let eval_errors: Vec<String> = serde_json::from_value(
                    proof.get("eval_errors").cloned().unwrap_or_else(|| serde_json::json!([])),
                )?;
                envelope(
                    "recalculate",
                    &resource,
                    record,
                    RecalculateData {
                        revision_before: before.clone(),
                        revision_after: record.state_revision.clone(),
                        duration_ms: proof["evaluation_duration_ms"]
                            .as_u64()
                            .ok_or_else(|| anyhow!("evaluation duration missing"))?,
                        backend: "formualizer".into(),
                        state: coverage.state(),
                        status: if coverage.error_formula_cells > 0 || !eval_errors.is_empty() {
                            "completed_with_errors"
                        } else {
                            "success"
                        }.into(),
                        error_count: Some(coverage.error_formula_cells as usize),
                        cells_evaluated: Some(proof["cells_evaluated"].as_u64()
                            .ok_or_else(|| anyhow!("evaluation count missing"))?),
                        eval_errors: (!eval_errors.is_empty()).then_some(eval_errors),
                        evaluation_coverage: coverage,
                        warnings: vec![],
                    },
                )
            },
        };
        canonical_write::recalculate_durable_with_backend(
            &mut self.owner,
            &storage,
            &self.request_id,
            (request.timeout_ms != 0).then_some(request.timeout_ms),
            backend,
        )
        .await?;
        self.finish()
    }
    async fn session_history(
        &mut self,
        request: crate::session_history::SessionHistoryRequest,
    ) -> Result<crate::session_history::SessionHistoryData> {
        use crate::core::resident_storage::PortableHistoryState;
        use crate::session_history::{
            SessionHealth, SessionHistoryData as Data, SessionHistoryEntry,
            SessionHistoryRequest as Request,
        };
        match &request {
            Request::Status { .. } => {
                return Ok(Data::Status {
                    health: if self.owner.poison_reason().is_some() {
                        SessionHealth::Poisoned
                    } else {
                        SessionHealth::Usable
                    },
                    revision_id: self
                        .owner
                        .poison_reason()
                        .is_none()
                        .then(|| self.owner.revision()),
                    poisoned_request_id: self.poisoned_request_id.clone(),
                    reason: self.owner.poison_reason().map(str::to_owned),
                });
            }
            Request::Outcome { request_id, .. } => {
                return reconcile_request_outcome(&self.storage, &self.session_id(), request_id).await;
            }
            Request::List { offset, limit, .. } => {
                let records = self.storage.load(&self.session_id()).await?;
                let state = PortableHistoryState::replay_bound(
                    &records,
                    &self.session_id(),
                    self.owner.immutable_base_sha256(),
                )?;
                let total = records.len();
                let start = (*offset as usize).min(total);
                let limit = (*limit).clamp(1, 2000) as usize;
                let records = records
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(limit)
                    .map(|(index, record)| SessionHistoryEntry {
                        sequence: index as u64 + 1,
                        commit_id: record.commit_id.clone(),
                        request_id: record.request_id.clone(),
                        transition: record.transition.clone(),
                        op_kinds: record.effects.iter().find_map(|effect| effect.pointer("/prepared_transaction/response/impact/op_kinds"))
                            .and_then(|value| serde_json::from_value(value.clone()).ok()).unwrap_or_default(),
                        history_parent_commit_id: record.history_parent_commit_id.clone(),
                        resulting_head: record.resulting_head.clone(),
                        branch: record.resulting_branch.clone(),
                        revision_id: record.state_revision.clone(),
                    })
                    .collect::<Vec<_>>();
                let next = start + records.len();
                return Ok(Data::List {
                    revision_id: self.owner.revision(),
                    head: state.head,
                    branch: state.current_branch,
                    branches: state.branches,
                    branch_labels: state.branch_labels,
                    records,
                    total,
                    next_offset: (next < total).then_some(next as u32),
                });
            }
            _ => {}
        }
        let hash = fingerprint("session_history", &request)?;
        let expected = match &request {
            Request::Undo {
                expected_revision, ..
            }
            | Request::Redo {
                expected_revision, ..
            }
            | Request::Checkout {
                expected_revision, ..
            }
            | Request::CreateBranch {
                expected_revision, ..
            }
            | Request::SwitchBranch {
                expected_revision, ..
            } => expected_revision.clone(),
            _ => unreachable!("read-only history returned above"),
        };
        self.check_revision(&expected).await?;
        let resource = self.resource.clone();
        let storage = OutcomeStorage {
            inner: &self.storage,
            request_id: &self.request_id,
            input_sha256: hash,
            outcome: &self.outcome,
            prepare: move |record: &PreparedResidentCommit| {
                let transition = if record.transition == ResidentTransition::Receipt {
                    serde_json::from_value(
                        record
                            .effects
                            .iter()
                            .find_map(|e| e.get("receipt_operation"))
                            .cloned()
                            .ok_or_else(|| anyhow!("history receipt lacks action"))?,
                    )?
                } else {
                    record.transition.clone()
                };
                envelope(
                    "session_history",
                    &resource,
                    record,
                    Data::Mutation {
                        transition,
                        revision_before: expected.clone(),
                        revision_after: record.state_revision.clone(),
                        head: record.resulting_head.clone(),
                        branch: record.resulting_branch.clone(),
                    },
                )
            },
        };
        let base = self.owner.immutable_base();
        match request {
            Request::Undo { .. } => {
                canonical_write::undo_durable(&mut self.owner, &storage, &self.request_id, &base)
                    .await?;
            }
            Request::Redo { .. } => {
                canonical_write::redo_durable(&mut self.owner, &storage, &self.request_id, &base)
                    .await?;
            }
            Request::Checkout {
                target_commit_id, ..
            } => {
                canonical_write::checkout_durable(
                    &mut self.owner,
                    &storage,
                    &self.request_id,
                    &target_commit_id,
                    &base,
                )
                .await?;
            }
            Request::CreateBranch { name, target_commit_id, label, .. } => {
                canonical_write::create_branch_at_durable(
                    &mut self.owner,
                    &storage,
                    &self.request_id,
                    &name,
                    target_commit_id.as_deref(),
                    label.as_deref(),
                )
                .await?;
            }
            Request::SwitchBranch { name, .. } => {
                canonical_write::switch_branch_durable(
                    &mut self.owner,
                    &storage,
                    &self.request_id,
                    &name,
                    &base,
                )
                .await?;
            }
            _ => unreachable!("read-only history returned above"),
        }
        self.finish()
    }

    async fn discard_fork(&mut self, request: DiscardForkRequest) -> Result<DiscardForkData> {
        self.check_revision(&request.expected_revision).await?;
        let resource = self.resource.clone();
        let storage = OutcomeStorage {
            inner: &self.storage, request_id: &self.request_id,
            input_sha256: fingerprint("discard_fork", &request)?, outcome: &self.outcome,
            prepare: |record: &PreparedResidentCommit| {
                let terminal = format!("discarded:{}", record.commit_id);
                let mut response = envelope("discard_fork", &resource, record, DiscardForkData {
                    revision_before: record.state_revision.clone(), revision_after: terminal.clone(),
                    discarded: true, warnings: Vec::new(),
                })?;
                response.revision_id = Some(terminal);
                Ok(response)
            },
        };
        canonical_write::commit_discard_receipt(&mut self.owner, &storage, &self.request_id, &request).await?;
        self.discarded = Some(true);
        self.finish()
    }
    async fn export_fork(&mut self, request: ExportForkRequest) -> Result<ExportForkData> {
        self.check_revision(&request.expected_revision).await?;
        let name = export_destination_name(&request.destination)?;
        let input_sha256 = fingerprint("export_fork", &request)?;
        let records = self.storage.load(&self.session_id()).await?;
        if let Some(record) = records.iter().find(|record| record.request_id == self.request_id) {
            let retained = OutcomeStorage {
                inner: &self.storage, request_id: &self.request_id, input_sha256,
                outcome: &self.outcome,
                prepare: |_: &PreparedResidentCommit| -> Result<CanonicalResponse> { bail!("read-only export reconciliation") },
            };
            if !matches!(retained.reconcile(&self.session_id(), &self.request_id, &record.request_fingerprint).await?,
                crate::core::resident_storage::ReconcileOutcome::Committed(_)) {
                bail!("export outcome unknown during reconciliation");
            }
            return self.finish();
        }
        self.owner.read_view()?;
        let sink = self.artifact_sink.as_ref().ok_or_else(|| anyhow!("artifact publication unavailable for this binding"))?;
        sink.validate(&request).await?;
        let bytes = self.owner.diagnostic_workbook().snapshot_bytes()?;
        let artifact = sink.publish(&request, &bytes).await?;
        if artifact.bytes != bytes.len() as u64 || artifact.sha256 != crate::utils::hash_bytes_sha256_hex(&bytes) {
            bail!("export outcome unknown: artifact sink returned metadata for a different snapshot");
        }
        let resource = self.resource.clone();
        let storage = OutcomeStorage {
            inner: &self.storage, request_id: &self.request_id, input_sha256,
            outcome: &self.outcome,
            prepare: |record: &PreparedResidentCommit| envelope("export_fork", &resource, record, ExportForkData {
                revision_before: record.state_revision.clone(), revision_after: record.state_revision.clone(),
                destination: ExportedDestination::Workspace {name:name.clone()}, artifact:artifact.clone(), warnings:Vec::new(),
            }),
        };
        canonical_write::commit_export_receipt(&mut self.owner, &storage, &self.request_id, &request, &artifact).await
            .map_err(|error| error.context("export outcome unknown after artifact publication"))?;
        self.finish()
    }

    async fn get_changes(&mut self, request: GetChangesRequest) -> Result<GetChangesData> {
        let (offset, limit) = match request.view {
            ChangesView::Operations { offset, limit } => (offset, limit),
            ChangesView::NetDiff { sheet_name, offset, limit } => {
                self.owner.read_view()?; // Never bypass poisoning for cold work.
                let revision = self.owner.revision();
                let base = self.owner.immutable_base();
                // Explicit cold comparison only; ordinary reads never snapshot.
                let current = self.owner.diagnostic_workbook().snapshot_bytes()?;
                let changes = crate::diff::calculate_changeset_bytes(&base, &current, sheet_name.as_deref())?;
                return Ok(net_diff_page(revision, self.owner.immutable_base_sha256().into(), changes, offset, limit));
            }
        };
        let records = self.storage.load(&self.session_id()).await?;
        let mut operations = Vec::new();
        for (index, record) in records.iter().enumerate() {
            let prepared = record
                .effects
                .iter()
                .find_map(|e| e.pointer("/prepared_transaction/response"));
            let (kind, op_kinds) = if record.transition == ResidentTransition::CalculationPublish {
                ("recalculate", vec![])
            } else if prepared.is_some_and(|r| r["mode"] == "apply") {
                (
                    "write",
                    serde_json::from_value(prepared.unwrap()["impact"]["op_kinds"].clone())?,
                )
            } else {
                continue;
            };
            let before = record
                .effects
                .iter()
                .find_map(|e| e.pointer("/canonical_outcome/response/data/revision_before"))
                .and_then(Value::as_str)
                .or_else(|| prepared.and_then(|r| r["revision_before"].as_str()))
                .or_else(|| {
                    index
                        .checked_sub(1)
                        .map(|i| records[i].state_revision.as_str())
                })
                .map(str::to_owned);
            let revision_before = match before {
                Some(before) => before,
                None => {
                    let (prefix, state) = record
                        .state_revision
                        .rsplit_once(':')
                        .ok_or_else(|| anyhow!("invalid calculation revision"))?;
                    let previous = state
                        .parse::<u64>()?
                        .checked_sub(1)
                        .ok_or_else(|| anyhow!("invalid first calculation revision"))?;
                    format!("{prefix}:{previous}")
                }
            };
            operations.push(crate::fork::CanonicalOperationRecord {
                sequence: operations.len() as u64 + 1,
                timestamp: None,
                kind: kind.into(),
                op_kinds,
                revision_before,
                revision_after: record.state_revision.clone(),
            });
        }
        Ok(operation_history_page(
            self.owner.revision(),
            &operations,
            offset,
            limit,
        ))
    }

    async fn checkpoint(&mut self, request: CheckpointRequest) -> Result<CheckpointData> {
        let hash = fingerprint("checkpoint", &request)?;
        let resource = self.resource.clone();
        match request {
            CheckpointRequest::List { .. } => Ok(CheckpointData::List {
                revision_id: self.owner.revision(),
                checkpoints: self.checkpoints().await?,
                warnings: vec![],
            }),
            CheckpointRequest::Create {
                expected_revision,
                label,
                ..
            } => {
                self.check_revision(&expected_revision).await?;
                let total_checkpoints = self.checkpoints().await?.len() + 1;
                let saved_label = label.clone();
                let storage = OutcomeStorage {
                    inner: &self.storage,
                    request_id: &self.request_id,
                    input_sha256: hash,
                    outcome: &self.outcome,
                    prepare: move |record: &PreparedResidentCommit| {
                        envelope(
                            "checkpoint",
                            &resource,
                            record,
                            CheckpointData::Create {
                                revision_before: expected_revision.clone(),
                                revision_after: record.state_revision.clone(),
                                checkpoint: CheckpointDescriptor {
                                    checkpoint_id: record.request_id.clone(),
                                    created_at: None,
                                    label: saved_label.clone(),
                                    snapshot_revision: record.state_revision.clone(),
                                    recalc_needed: true,
                                },
                                total_checkpoints,
                                warnings: vec![],
                            },
                        )
                    },
                };
                canonical_write::checkpoint_durable(
                    &mut self.owner,
                    &storage,
                    &self.request_id,
                    label.as_deref(),
                )
                .await?;
                self.finish()
            }
            CheckpointRequest::Delete {
                expected_revision,
                checkpoint_id,
                ..
            } => {
                self.check_revision(&expected_revision).await?;
                let id = checkpoint_id.clone();
                let storage = OutcomeStorage {
                    inner: &self.storage,
                    request_id: &self.request_id,
                    input_sha256: hash,
                    outcome: &self.outcome,
                    prepare: move |record: &PreparedResidentCommit| {
                        envelope(
                            "checkpoint",
                            &resource,
                            record,
                            CheckpointData::Delete {
                                revision_before: expected_revision.clone(),
                                revision_after: record.state_revision.clone(),
                                checkpoint_id: id.clone(),
                                deleted: true,
                                warnings: vec![],
                            },
                        )
                    },
                };
                canonical_write::delete_checkpoint_durable(
                    &mut self.owner,
                    &storage,
                    &self.request_id,
                    &checkpoint_id,
                )
                .await?;
                self.finish()
            }
            CheckpointRequest::Restore {
                expected_revision,
                checkpoint_id,
                ..
            } => {
                self.check_revision(&expected_revision).await?;
                // A retry may follow deletion of the restored checkpoint. Its
                // original response is already journal authority, not today's list.
                let records = self.storage.load(&self.session_id()).await?;
                let historical = records
                    .iter()
                    .find(|r| r.request_id == self.request_id)
                    .and_then(|r| {
                        r.effects.iter().find_map(|e| {
                            e.pointer("/canonical_outcome/response/data/restored_checkpoint")
                        })
                    })
                    .cloned();
                let checkpoints = self.checkpoints().await?;
                let restored_checkpoint = match historical {
                    Some(value) => serde_json::from_value(value)?,
                    None => checkpoints
                        .iter()
                        .find(|c| c.checkpoint_id == checkpoint_id)
                        .cloned()
                        .ok_or_else(|| anyhow!("unknown checkpoint"))?,
                };
                let retained_checkpoint_ids = checkpoints
                    .iter()
                    .map(|c| c.checkpoint_id.clone())
                    .collect::<Vec<_>>();
                let storage = OutcomeStorage {
                    inner: &self.storage,
                    request_id: &self.request_id,
                    input_sha256: hash,
                    outcome: &self.outcome,
                    prepare: move |record: &PreparedResidentCommit| {
                        envelope(
                            "checkpoint",
                            &resource,
                            record,
                            CheckpointData::Restore {
                                revision_before: expected_revision.clone(),
                                revision_after: record.state_revision.clone(),
                                restored_checkpoint: restored_checkpoint.clone(),
                                operations_removed: 0,
                                staged_changes_discarded: 0,
                                retained_checkpoint_ids: retained_checkpoint_ids.clone(),
                                invalidated_checkpoint_ids: vec![],
                                recalc_needed: true,
                                warnings: vec![],
                            },
                        )
                    },
                };
                canonical_write::restore_checkpoint_durable(
                    &mut self.owner,
                    &storage,
                    &self.request_id,
                    &checkpoint_id,
                )
                .await?;
                self.finish()
            }
        }
    }
    async fn staged_change(&mut self, request: StagedChangeRequest) -> Result<StagedChangeData> {
        let hash = fingerprint("staged_change", &request)?;
        let resource = self.resource.clone();
        match request {
            StagedChangeRequest::List { .. } => {
                let records = self.storage.load(&self.session_id()).await?;
                let mut staged_changes = vec![];
                for (id, bundle) in self.owner.staged_bundles()? {
                    let response = records
                        .iter()
                        .find_map(|r| {
                            r.effects
                                .iter()
                                .find(|e| {
                                    e.pointer("/prepared_transaction/staged/0")
                                        .and_then(Value::as_str)
                                        == Some(id.as_str())
                                })
                                .and_then(|e| e.pointer("/prepared_transaction/response"))
                        })
                        .ok_or_else(|| anyhow!("staged catalog lacks its authoritative outcome"))?;
                    let changes = response
                        .pointer("/diff/change_count")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| anyhow!("staged outcome lacks canonical diff"))?
                        as usize;
                    let summary = canonical_write::staged_summary(
                        bundle.ops.iter().map(|o| o.kind().into()).collect(),
                        bundle.ops.len(),
                        changes,
                    );
                    staged_changes.push(StagedChangeDescriptor {
                        change_id: id.clone(),
                        created_at: None,
                        label: bundle.label.clone(),
                        base_revision: bundle.base_revision.clone(),
                        summary,
                    });
                }
                Ok(StagedChangeData::List {
                    revision_id: self.owner.revision(),
                    staged_changes,
                    warnings: vec![],
                })
            }
            StagedChangeRequest::Apply {
                expected_revision,
                change_id,
                ..
            } => {
                self.check_revision(&expected_revision).await?;
                let id = change_id.clone();
                let prior_dirty = self.dirty();
                let storage = OutcomeStorage {
                    inner: &self.storage,
                    request_id: &self.request_id,
                    input_sha256: hash,
                    outcome: &self.outcome,
                    prepare: move |record: &PreparedResidentCommit| {
                        let publication = record
                            .effects
                            .iter()
                            .find_map(|e| e.pointer("/prepared_transaction/publication"))
                            .ok_or_else(|| anyhow!("prepared publication missing"))?;
                        let recalc_needed = if publication["kind"] == "none"
                            || publication["calculation_effect"] == "Preserve"
                        {
                            prior_dirty
                        } else {
                            true
                        };
                        match prepared_response(record)? {
                            WriteResponseData::Applied {
                                revision_before,
                                revision_after,
                                ops_applied,
                                impact,
                                ..
                            } => envelope(
                                "staged_change",
                                &resource,
                                record,
                                StagedChangeData::Apply {
                                    head: record.resulting_head.clone(),
                                    revision_before,
                                    revision_after,
                                    change_id: id.clone(),
                                    ops_applied,
                                    op_kinds: impact.op_kinds,
                                    recalc_needed,
                                    warnings: vec![],
                                },
                            ),
                            _ => bail!("staged application did not prepare a success"),
                        }
                    },
                };
                let result = canonical_write::apply_staged_durable(
                    &mut self.owner,
                    &storage,
                    &self.request_id,
                    &change_id,
                    &expected_revision,
                )
                .await?;
                if !matches!(result, WriteResponseData::Applied { .. }) {
                    bail!(
                        "staged application did not commit: {}",
                        serde_json::to_string(&result)?
                    );
                }
                self.finish()
            }
            StagedChangeRequest::Discard {
                expected_revision,
                change_id,
                ..
            } => {
                self.check_revision(&expected_revision).await?;
                let id = change_id.clone();
                let storage = OutcomeStorage {
                    inner: &self.storage,
                    request_id: &self.request_id,
                    input_sha256: hash,
                    outcome: &self.outcome,
                    prepare: move |record: &PreparedResidentCommit| {
                        envelope(
                            "staged_change",
                            &resource,
                            record,
                            StagedChangeData::Discard {
                                revision_before: expected_revision.clone(),
                                revision_after: record.state_revision.clone(),
                                change_id: id.clone(),
                                discarded: record.transition == ResidentTransition::CatalogDiscard,
                                warnings: vec![],
                            },
                        )
                    },
                };
                canonical_write::discard_staged_durable(
                    &mut self.owner,
                    &storage,
                    &self.request_id,
                    &change_id,
                )
                .await?;
                self.finish()
            }
        }
    }
}

/// Receipt-only execution context for a validated terminal journal. No workbook
/// or evaluator is constructed, and normal dispatcher envelope rules still apply.
pub struct TerminalSessionContext<'a, S> {
    pub storage: &'a S,
    pub resource: &'a ResourceId,
}
impl<S: ResidentCommitStorage> ExecutionContext for TerminalSessionContext<'_, S> {
    type Reads<'a> = Arc<crate::state::AppState> where Self: 'a;
    fn reads(&self) -> Result<Self::Reads<'_>, CanonicalErrorEnvelope> {
        Err(CanonicalErrorEnvelope::new(crate::operations::CanonicalErrorCode::ResourceNotFound, "resource has been discarded", None, None))
    }
    fn capabilities(&self) -> RuntimeCapabilities { let mut value = RuntimeCapabilities::native(); value.resident_history = true; value }
    async fn identify(&self, _: &ResourceId) -> Result<(ResourceId, String)> { bail!("resource has been discarded") }
    async fn identify_diagnostic(&self, resource: &ResourceId) -> Result<(ResourceId, Option<String>)> {
        if resource != self.resource { bail!("terminal resource binding mismatch"); }
        Ok((resource.clone(), None))
    }
    async fn session_history(&mut self, request: crate::session_history::SessionHistoryRequest) -> Result<crate::session_history::SessionHistoryData> {
        if let crate::session_history::SessionHistoryRequest::Outcome { request_id, .. } = request {
            reconcile_request_outcome(self.storage, &self.resource.to_workbook_id().0, &request_id).await
        } else { bail!("resource has been discarded; only outcome reconciliation is available") }
    }
}

pub async fn execute_terminal_session<S: ResidentCommitStorage>(
    storage: &S, resource: &ResourceId, request_id: &str, operation: crate::operations::SpreadsheetOperation,
) -> Result<CanonicalResponse, CanonicalErrorEnvelope> {
    use crate::operations::SpreadsheetOperation as Op;
    let result = match &operation {
        Op::DiscardFork(request) => reconcile_lifecycle_outcome(storage, resource, request_id, "discard_fork", request).await,
        Op::ExportFork(request) => reconcile_lifecycle_outcome(storage, resource, request_id, "export_fork", request).await,
        _ => Ok(None),
    }.map_err(|error| crate::operations::lifecycle_error(operation.name(), error))?;
    if let Some(response) = result { return Ok(response); }
    if matches!(&operation, Op::SessionHistory(crate::session_history::SessionHistoryRequest::Outcome { .. })) {
        return crate::operations::execute_operation(TerminalSessionContext { storage, resource }, operation).await;
    }
    Err(CanonicalErrorEnvelope::new(crate::operations::CanonicalErrorCode::ResourceNotFound,
        "resource has been discarded; only retained outcome reconciliation is available", Some(operation.name()), None))
}

pub async fn reconcile_request_outcome<S: ResidentCommitStorage>(storage: &S, session_id: &str, request_id: &str) -> Result<crate::session_history::SessionHistoryData> {
    use crate::session_history::{SessionHistoryData as Data, RequestOutcomeState};
    use crate::core::resident_storage::ReconcileOutcome;
    let records = match storage.load(session_id).await {
        Ok(records) => records,
        Err(_) => {
            return Ok(Data::Outcome {
                request_id: request_id.to_owned(),
                state: RequestOutcomeState::Unknown,
                response: None,
            });
        }
    };
    let record = records.iter().find(|r| r.request_id == request_id);
    let fingerprint = record
        .map(|r| r.request_fingerprint.as_str())
        .unwrap_or("absent-canonical-request");
    let reconciled = storage
        .reconcile(session_id, request_id, fingerprint)
        .await;
    let response = record
        .and_then(|r| {
            r.effects
                .iter()
                .find_map(|e| e.pointer("/canonical_outcome/response"))
        })
        .cloned();
    let (state, response) = match (reconciled, response) {
        (Ok(ReconcileOutcome::NotFound), _) => (RequestOutcomeState::NotFound, None),
        (Ok(ReconcileOutcome::Committed(_)), Some(response)) => (
            RequestOutcomeState::Committed,
            Some(serde_json::from_value(response)?),
        ),
        _ => (RequestOutcomeState::Unknown, None),
    };
    return Ok(Data::Outcome {
        request_id: request_id.to_owned(),
        state,
        response,
    });
}
