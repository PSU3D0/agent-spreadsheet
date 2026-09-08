//! Shared durable capture/receipt protocol for explicit host filesystem exports.
//! Paths and filesystem authorization are interpreted exclusively by the host sink.
use crate::canonical_lifecycle::ArtifactMetadata;
#[cfg(feature = "recalc-formualizer")]
use crate::{
    canonical_write::ResidentWriteSession,
    core::resident_storage::{PortableHistoryState, ResidentCommitStorage},
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileExportIntent {
    pub resource_id: String,
    pub request_id: String,
    pub expected_revision: Option<String>,
    /// None means explicit replacement of the bound source.
    pub destination: Option<String>,
    pub force: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileExportPlan {
    pub head: Option<String>,
    pub events_replayed: usize,
    pub intent: FileExportIntent,
    pub revision_id: String,
    pub artifact: ArtifactMetadata,
    pub source_sha256: String,
    pub destination: String,
    /// First-admission destination content generation; absent on older plans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_generation: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileExportResult {
    pub head: Option<String>,
    pub events_replayed: usize,
    pub request_id: String,
    pub resource_id: String,
    pub revision_id: String,
    pub output_path: String,
    pub artifact: ArtifactMetadata,
}
impl FileExportPlan {
    pub fn result(&self) -> FileExportResult {
        FileExportResult {
            head: self.head.clone(),
            events_replayed: self.events_replayed,
            request_id: self.intent.request_id.clone(),
            resource_id: self.intent.resource_id.clone(),
            revision_id: self.revision_id.clone(),
            output_path: self.destination.clone(),
            artifact: self.artifact.clone(),
        }
    }
    pub fn validate(&self, session_id: &str) -> Result<()> {
        ensure!(
            self.intent
                .resource_id
                .split_once(':')
                .is_some_and(|(kind, key)| matches!(kind, "fork" | "session") && key == session_id),
            "file export resource mismatch"
        );
        ensure!(
            !self.intent.request_id.is_empty() && self.intent.request_id.len() <= 256,
            "invalid file export request identity"
        );
        ensure!(
            self.intent
                .expected_revision
                .as_ref()
                .is_none_or(|revision| revision == &self.revision_id),
            "file export named revision mismatch"
        );
        ensure!(
            self.destination_generation
                .as_deref()
                .is_none_or(|generation| generation == "missing"
                    || generation.strip_prefix("sha256:").is_some_and(
                        |hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
                    )),
            "invalid export destination generation"
        );
        ensure!(
            !self.destination.is_empty() && self.destination.len() <= 4096,
            "invalid export destination"
        );
        ensure!(
            self.intent
                .destination
                .as_ref()
                .is_none_or(|path| path == &self.destination),
            "export destination differs from explicit intent"
        );
        ensure!(
            self.artifact.bytes <= 64 * 1024 * 1024
                && self.artifact.sha256.len() == 64
                && self.artifact.sha256.bytes().all(|b| b.is_ascii_hexdigit())
                && self.artifact.artifact_id == format!("artifact-{}", self.artifact.sha256),
            "invalid capture metadata"
        );
        ensure!(
            self.source_sha256.len() == 64
                && self.source_sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid source binding"
        );
        ensure!(
            self.artifact.media_type
                == "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "invalid export media type"
        );
        Ok(())
    }
}
pub fn plan_id(identity: &str) -> String {
    format!(
        "file_export_plan_{}",
        crate::utils::hash_bytes_sha256_hex(identity.as_bytes())
    )
}

#[async_trait::async_trait(?Send)]
pub trait FileExportSink {
    /// Validate explicit authority and current destination before capture.
    async fn authorize(&self, intent: &FileExportIntent) -> Result<(String, String, String)>;
    /// Durably preserve immutable captured bytes before committing the plan.
    async fn retain(&self, bytes: &[u8]) -> Result<ArtifactMetadata>;
    /// Read retained bytes, verify hash/length, publish or reconcile prior IO.
    async fn publish(&self, plan: &FileExportPlan) -> Result<()>;
}

#[cfg(feature = "recalc-formualizer")]
pub async fn export_file<S: ResidentCommitStorage>(
    owner: &mut ResidentWriteSession,
    storage: &S,
    intent: FileExportIntent,
    sink: &dyn FileExportSink,
) -> Result<FileExportResult> {
    let session_id = intent
        .resource_id
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid resource"))?
        .1;
    ensure!(
        !intent.request_id.is_empty() && intent.request_id.len() <= 256,
        "invalid request identity"
    );
    let records = storage.load(session_id).await?;
    let state =
        PortableHistoryState::replay_bound(&records, session_id, owner.immutable_base_sha256())?;
    let plan = if let Some(plan) = state.file_export_plans.get(&intent.request_id) {
        ensure!(
            plan.intent == intent,
            "file export request identity reuse with different input"
        );
        if let Some(result) = state.file_export_results.get(&intent.request_id) {
            let record = records
                .iter()
                .find(|record| record.request_id == intent.request_id)
                .ok_or_else(|| anyhow!("export result lacks receipt"))?;
            ensure!(
                matches!(
                    storage
                        .reconcile(session_id, &intent.request_id, &record.request_fingerprint)
                        .await?,
                    crate::core::resident_storage::ReconcileOutcome::Committed(_)
                ),
                "export receipt outcome unknown"
            );
            return Ok(result.clone());
        }
        plan.clone()
    } else {
        let preparation_id = plan_id(&intent.request_id);
        ensure!(
            !records
                .iter()
                .any(|record| record.request_id == intent.request_id
                    || record.request_id == preparation_id)
                && !state.file_export_plans.contains_key(&preparation_id),
            "file export public or preparation request identity belongs to another operation; export not submitted"
        );
        ensure!(
            state.file_export_plans.len() < 128,
            "file export plan lifetime quota reached; acknowledged captures are not expired"
        );
        owner.read_view()?;
        if let Some(expected) = &intent.expected_revision {
            ensure!(
                *expected == owner.revision(),
                "revision conflict: named export revision is not the retained current revision"
            );
        }
        let (destination, source_sha256, destination_generation) = sink.authorize(&intent).await?;
        let bytes = owner.diagnostic_workbook().snapshot_bytes()?;
        let artifact = sink.retain(&bytes).await?;
        ensure!(
            artifact.bytes == bytes.len() as u64
                && artifact.sha256 == crate::utils::hash_bytes_sha256_hex(&bytes),
            "capture metadata mismatch"
        );
        let plan = FileExportPlan {
            head: state.head.clone(),
            events_replayed: state.active_ancestry()?.len(),
            intent: intent.clone(),
            revision_id: owner.revision(),
            artifact,
            source_sha256,
            destination,
            destination_generation: Some(destination_generation),
        };
        plan.validate(session_id)?;
        crate::canonical_write::commit_file_export_receipt(
            owner,
            storage,
            &plan_id(&intent.request_id),
            json!({"file_export_plan":plan}),
        )
        .await
        .context("export preparation outcome unknown; reconcile this request before retry")?;
        plan
    };
    if state.discarded {
        bail!("discarded resource cannot publish a new filesystem export");
    }
    owner.read_view()?; // Never publish pending IO through a poisoned owner.
    let plan_records = storage.load(session_id).await?;
    let preparation_id = plan_id(&intent.request_id);
    let preparation = plan_records
        .iter()
        .find(|record| record.request_id == preparation_id)
        .ok_or_else(|| anyhow!("export plan is not readable"))?;
    ensure!(
        matches!(
            storage
                .reconcile(
                    session_id,
                    &preparation_id,
                    &preparation.request_fingerprint
                )
                .await?,
            crate::core::resident_storage::ReconcileOutcome::Committed(_)
        ),
        "export preparation outcome unknown"
    );
    // The durable plan pins revision and bytes. Retry never captures newer work.
    sink.publish(&plan).await.context("filesystem export outcome unknown after durable plan; retain request identity for reconciliation")?;
    let result = plan.result();
    crate::canonical_write::commit_file_export_receipt(
        owner,
        storage,
        &intent.request_id,
        json!({"file_export_result":result}),
    )
    .await
    .context("filesystem IO may have committed; export receipt outcome unknown")?;
    Ok(result)
}
