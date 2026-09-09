//! Same-record canonical outcomes. Only shared canonical code prepares these;
//! hosts supply storage and never reconstruct semantic responses after commit.
use crate::{core::resident_storage::*, operations::CanonicalResponse};
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CanonicalOutcome {
    pub input_sha256: String,
    pub core_fingerprint: String,
    pub response: CanonicalResponse,
}
pub(crate) struct OutcomeStorage<'a, S, F> {
    pub inner: &'a S,
    pub request_id: &'a str,
    pub input_sha256: String,
    pub prepare: F,
    pub outcome: &'a RefCell<Option<CanonicalResponse>>,
}
impl<S, F> OutcomeStorage<'_, S, F> {
    fn capture(&self, record: &PreparedResidentCommit) -> Result<()> {
        let value = record.effects.iter().find_map(|e| e.get("canonical_outcome")).ok_or_else(|| anyhow!("request predates canonical outcome retention; outcome requires operator reconciliation"))?;
        let outcome: CanonicalOutcome = serde_json::from_value(value.clone())?;
        if outcome.input_sha256 != self.input_sha256 {
            bail!("request identity reuse with different canonical input");
        }
        *self.outcome.borrow_mut() = Some(outcome.response);
        Ok(())
    }
}
#[async_trait::async_trait(?Send)]
impl<S: ResidentCommitStorage, F: Fn(&PreparedResidentCommit) -> Result<CanonicalResponse>>
    ResidentCommitStorage for OutcomeStorage<'_, S, F>
{
    fn outcome_retention(&self) -> OutcomeRetention {
        self.inner.outcome_retention()
    }
    async fn load(&self, session_id: &str) -> Result<Vec<PreparedResidentCommit>> {
        let records = self.inner.load(session_id).await?;
        if records
            .iter()
            .flat_map(|record| &record.effects)
            .any(|effect| {
                effect
                    .pointer("/file_export_plan/intent/request_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(self.request_id)
            })
        {
            bail!(
                "request identity reserved by filesystem export; canonical operation was not submitted"
            );
        }
        if let Some(record) = records.iter().find(|r| r.request_id == self.request_id) {
            self.capture(record)?;
        }
        Ok(records)
    }
    async fn commit(&self, prepared: &PreparedResidentCommit) -> Result<DurableCommitOutcome> {
        let response = (self.prepare)(prepared)?;
        let mut record = prepared.clone();
        record.effects.push(serde_json::json!({"canonical_outcome":CanonicalOutcome {input_sha256:self.input_sha256.clone(),core_fingerprint:record.request_fingerprint.clone(),response:response.clone()}}));
        let committed = self.inner.commit(&record).await?;
        if committed.commit_id != record.commit_id {
            bail!("commit identity changed; canonical outcome reconciliation required");
        }
        *self.outcome.borrow_mut() = Some(response);
        Ok(committed)
    }
    async fn reconcile(
        &self,
        session_id: &str,
        request_id: &str,
        fingerprint: &str,
    ) -> Result<ReconcileOutcome> {
        let outcome = self
            .inner
            .reconcile(session_id, request_id, fingerprint)
            .await?;
        if let ReconcileOutcome::Committed(ref committed) = outcome {
            let records = self.inner.load(session_id).await?;
            let record = records
                .iter()
                .find(|r| r.commit_id == committed.commit_id)
                .ok_or_else(|| anyhow!("committed canonical outcome missing"))?;
            self.capture(record)?;
        }
        Ok(outcome)
    }
}

pub(crate) fn validate(
    record: &PreparedResidentCommit,
    state: &PortableHistoryState,
) -> Result<()> {
    let outcomes = record
        .effects
        .iter()
        .filter_map(|e| e.get("canonical_outcome"))
        .collect::<Vec<_>>();
    if outcomes.is_empty() {
        return Ok(());
    }
    if outcomes.len() != 1 {
        bail!("duplicate canonical outcome metadata");
    }
    if record
        .effects
        .iter()
        .any(|e| e.get("canonical_outcome").is_some() && e.as_object().is_none_or(|o| o.len() != 1))
    {
        bail!("canonical metadata must be a separate typed effect");
    }
    let outcome: CanonicalOutcome = serde_json::from_value(outcomes[0].clone())?;
    let response = &outcome.response;
    if response.resource_id.as_ref().is_none_or(|resource| {
        !resource.as_str().starts_with("session:") && !resource.as_str().starts_with("fork:")
    }) || outcome.core_fingerprint != record.request_fingerprint
        || outcome.input_sha256.len() != 64
        || !outcome.input_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        || response.schema_version != crate::operations::CANONICAL_SCHEMA_VERSION
        || response.revision_id.as_deref()
            != Some(if response.operation == "discard_fork" {
                response.data["revision_after"].as_str().unwrap_or("")
            } else {
                record.state_revision.as_str()
            })
        || response
            .resource_id
            .as_ref()
            .map(|r| r.to_workbook_id().0)
            .as_deref()
            != Some(record.session_id.as_str())
    {
        bail!("canonical outcome identity binding mismatch");
    }
    let effect = |key| record.effects.iter().find_map(|e| e.get(key));
    if record
        .effects
        .iter()
        .any(|e| e.get("resource_export").is_some() && e.as_object().is_none_or(|o| o.len() != 1))
    {
        bail!("export metadata must be a separate typed effect");
    }
    if record
        .effects
        .iter()
        .any(|e| e.get("resource_discard").is_some() && e.as_object().is_none_or(|o| o.len() != 1))
    {
        bail!("discard metadata must be a separate typed effect");
    }
    if effect("resource_discard").is_some() && response.operation != "discard_fork" {
        bail!("discard receipt requires a discard outcome");
    }
    if effect("resource_export").is_some() && response.operation != "export_fork" {
        bail!("export receipt requires an export outcome");
    }
    if effect("resource_creation").is_some() && response.operation != "create_fork" {
        bail!("creation receipt requires a creation outcome");
    }
    if record
        .effects
        .iter()
        .any(|e| e.get("resource_creation").is_some() && e.as_object().is_none_or(|o| o.len() != 1))
    {
        bail!("creation metadata must be a separate typed effect");
    }
    if let Some(before) = response
        .data
        .get("revision_before")
        .and_then(serde_json::Value::as_str)
    {
        let expected = effect("prepared_transaction")
            .and_then(|p| p.pointer("/response/revision_before"))
            .and_then(serde_json::Value::as_str)
            .or(state.state_revision.as_deref());
        let initial_before;
        let expected = if let Some(expected) = expected {
            expected
        } else if matches!(
            record.transition,
            ResidentTransition::CalculationPublish | ResidentTransition::BranchCreate
        ) {
            let (prefix, counter) = record
                .state_revision
                .rsplit_once(':')
                .ok_or_else(|| anyhow!("invalid calculation revision"))?;
            initial_before = format!(
                "{prefix}:{}",
                counter
                    .parse::<u64>()?
                    .checked_sub(1)
                    .ok_or_else(|| anyhow!("invalid calculation counter"))?
            );
            &initial_before
        } else {
            &record.state_revision
        };
        if before != expected {
            bail!("canonical outcome before revision mismatch");
        }
    }
    if response.operation != "write"
        && response.operation != "session_history"
        && response.data["warnings"] != serde_json::json!([])
    {
        bail!("canonical outcome contains unprepared warnings");
    }
    match response.operation.as_str() {
        "create_fork" => {
            let request: crate::canonical_lifecycle::CreateForkRequest = serde_json::from_value(
                effect("resource_creation")
                    .cloned()
                    .ok_or_else(|| anyhow!("creation lacks prepared request"))?,
            )?;
            let data: crate::canonical_lifecycle::CreateForkData =
                serde_json::from_value(response.data.clone())?;
            let expected_input = crate::utils::hash_bytes_sha256_hex(&serde_json::to_vec(&(
                "create_fork",
                &request,
            ))?);
            if record.transition != ResidentTransition::Receipt
                || record.parent_commit_id.is_some()
                || record.effects.len() != 2
                || !(request.resource_id.as_str().starts_with("wb:")
                    || request.resource_id.as_str().starts_with("fork:")
                    || request.resource_id.as_str().starts_with("session:"))
                || request.expected_revision.is_empty()
                || (request.resource_id.as_str().starts_with("wb:")
                    && request.expected_revision != record.base_sha256)
                || effect("receipt_operation").is_some()
                || effect("prepared_transaction").is_some()
                || data.label != request.label
                || request
                    .label
                    .as_ref()
                    .is_some_and(|label| label.len() > 1024)
                || data.base_resource_id != request.resource_id
                || data.base_revision_id != request.expected_revision
                || data.fork_resource_id != *response.resource_id.as_ref().unwrap()
                || data.revision_id != record.state_revision
                || data.ttl_seconds.is_some()
                || outcome.input_sha256 != expected_input
                || !data.warnings.is_empty()
            {
                bail!("canonical creation outcome differs from prepared activation");
            }
        }
        "discard_fork" => {
            let request: crate::canonical_lifecycle::DiscardForkRequest = serde_json::from_value(
                effect("resource_discard")
                    .cloned()
                    .ok_or_else(|| anyhow!("discard lacks prepared request"))?,
            )?;
            let data: crate::canonical_lifecycle::DiscardForkData =
                serde_json::from_value(response.data.clone())?;
            let expected_input = crate::utils::hash_bytes_sha256_hex(&serde_json::to_vec(&(
                "discard_fork",
                &request,
            ))?);
            if record.transition != ResidentTransition::Receipt
                || record.effects.len() != 2
                || request.resource_id != *response.resource_id.as_ref().unwrap()
                || request.expected_revision != record.state_revision
                || data.revision_before != record.state_revision
                || data.revision_after != format!("discarded:{}", record.commit_id)
                || !data.discarded
                || outcome.input_sha256 != expected_input
            {
                bail!("canonical discard outcome differs from prepared tombstone");
            }
        }
        "export_fork" => {
            let prepared = effect("resource_export")
                .ok_or_else(|| anyhow!("export lacks prepared artifact"))?;
            let request: crate::canonical_lifecycle::ExportForkRequest =
                serde_json::from_value(prepared["request"].clone())?;
            let data: crate::canonical_lifecycle::ExportForkData =
                serde_json::from_value(response.data.clone())?;
            let expected_input = crate::utils::hash_bytes_sha256_hex(&serde_json::to_vec(&(
                "export_fork",
                &request,
            ))?);
            let name = crate::canonical_lifecycle::export_destination_name(&request.destination)?;
            if record.transition != ResidentTransition::Receipt
                || record.effects.len() != 2
                || prepared.as_object().is_none_or(|o| o.len() != 2)
                || request.resource_id != *response.resource_id.as_ref().unwrap()
                || request.expected_revision != record.state_revision
                || data.revision_before != record.state_revision
                || data.revision_after != record.state_revision
                || prepared["artifact"] != serde_json::to_value(&data.artifact)?
                || serde_json::to_value(&data.destination)?
                    != serde_json::json!({"kind":"workspace","name":name})
                || data.artifact.sha256.len() != 64
                || !data.artifact.sha256.bytes().all(|b| b.is_ascii_hexdigit())
                || data.artifact.artifact_id != format!("artifact-{}", data.artifact.sha256)
                || data.artifact.media_type
                    != "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                || outcome.input_sha256 != expected_input
            {
                bail!("canonical export outcome differs from prepared artifact");
            }
        }
        "write" => {
            if effect("receipt_operation").is_some()
                || !matches!(
                    record.transition,
                    ResidentTransition::Mutation
                        | ResidentTransition::CatalogStage
                        | ResidentTransition::Receipt
                )
                || effect("prepared_transaction").and_then(|p| p.get("response"))
                    != Some(&response.data)
            {
                bail!("canonical write outcome differs from prepared transaction");
            }
        }
        "recalculate" => {
            let data: crate::canonical_lifecycle::RecalculateData =
                serde_json::from_value(response.data.clone())?;
            let proof = effect("calculation_proof");
            let external = proof.and_then(|p| p.get("external_result"));
            let backend_fields_valid = if let Some(external) = external {
                external == &response.data
                    && data.backend == "libreoffice"
                    && proof
                        .and_then(|p| p.get("backend"))
                        .and_then(serde_json::Value::as_str)
                        == Some("libreoffice")
                    && data.evaluation_coverage.source
                        == crate::model::EvaluationSource::TrustedCache
                    && data.status
                        == if data.state == crate::model::EvaluationState::Clean {
                            "completed"
                        } else {
                            "completed_with_errors"
                        }
                    && data.eval_errors.is_none()
                    && data.error_count.is_none()
                    && data.cells_evaluated.is_none()
            } else {
                let errors: Vec<String> = serde_json::from_value(
                    proof
                        .and_then(|p| p.get("eval_errors"))
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([])),
                )?;
                let status =
                    if data.evaluation_coverage.error_formula_cells > 0 || !errors.is_empty() {
                        "completed_with_errors"
                    } else {
                        "success"
                    };
                data.backend == "formualizer"
                    && data.status == status
                    && data.eval_errors == (!errors.is_empty()).then_some(errors)
                    && data.error_count
                        == Some(data.evaluation_coverage.error_formula_cells as usize)
                    && data.cells_evaluated.is_some()
                    && data.cells_evaluated
                        == proof
                            .and_then(|p| p.get("cells_evaluated"))
                            .and_then(serde_json::Value::as_u64)
            };
            if record.transition != ResidentTransition::CalculationPublish
                || effect("calculation_proof").and_then(|p| p.get("coverage"))
                    != Some(&serde_json::to_value(&data.evaluation_coverage)?)
                || data.revision_after != record.state_revision
                || data.state != data.evaluation_coverage.state()
                || !backend_fields_valid
                || effect("calculation_proof")
                    .and_then(|p| p.get("evaluation_duration_ms"))
                    .and_then(serde_json::Value::as_u64)
                    != Some(data.duration_ms)
                || !data.warnings.is_empty()
            {
                bail!("canonical calculation outcome differs from proof");
            }
        }
        "checkpoint" => {
            use crate::canonical_lifecycle::CheckpointData;
            let data: CheckpointData = serde_json::from_value(response.data.clone())?;
            match data {
                CheckpointData::Create {
                    checkpoint,
                    total_checkpoints,
                    revision_after,
                    ..
                } if record.transition == ResidentTransition::Checkpoint => {
                    let expected = state.checkpoints.get(&record.request_id).ok_or_else(|| {
                        anyhow!("checkpoint outcome references missing checkpoint")
                    })?;
                    if checkpoint.checkpoint_id != expected.id
                        || checkpoint.snapshot_revision != expected.state_revision
                        || checkpoint.label != expected.label
                        || checkpoint.created_at.is_some()
                        || !checkpoint.recalc_needed
                        || total_checkpoints != state.checkpoints.len()
                        || revision_after != record.state_revision
                    {
                        bail!("canonical checkpoint outcome mismatch");
                    }
                }
                CheckpointData::Delete {
                    checkpoint_id,
                    deleted,
                    revision_after,
                    ..
                } if record.transition == ResidentTransition::CheckpointDelete => {
                    if effect("checkpoint_id").and_then(serde_json::Value::as_str)
                        != Some(&checkpoint_id)
                        || !deleted
                        || revision_after != record.state_revision
                    {
                        bail!("canonical checkpoint deletion mismatch");
                    }
                }
                CheckpointData::Restore {
                    restored_checkpoint,
                    revision_after,
                    operations_removed,
                    staged_changes_discarded,
                    retained_checkpoint_ids,
                    invalidated_checkpoint_ids,
                    recalc_needed,
                    ..
                } if record.transition == ResidentTransition::Checkout => {
                    let expected = state
                        .checkpoints
                        .get(&restored_checkpoint.checkpoint_id)
                        .ok_or_else(|| {
                            anyhow!("restored checkpoint outcome references missing checkpoint")
                        })?;
                    if restored_checkpoint.snapshot_revision != expected.state_revision
                        || restored_checkpoint.label != expected.label
                        || restored_checkpoint.created_at.is_some()
                        || !restored_checkpoint.recalc_needed
                        || effect("checkpoint_id").and_then(serde_json::Value::as_str)
                            != Some(&restored_checkpoint.checkpoint_id)
                        || revision_after != record.state_revision
                        || operations_removed != 0
                        || staged_changes_discarded != 0
                        || !invalidated_checkpoint_ids.is_empty()
                        || !recalc_needed
                        || retained_checkpoint_ids
                            != state.checkpoints.keys().cloned().collect::<Vec<_>>()
                    {
                        bail!("canonical checkpoint restoration mismatch");
                    }
                }
                _ => bail!("canonical checkpoint action/transition mismatch"),
            }
        }
        "staged_change" => {
            use crate::canonical_lifecycle::StagedChangeData;
            let data: StagedChangeData = serde_json::from_value(response.data.clone())?;
            match data {
                StagedChangeData::Apply {
                    head,
                    change_id,
                    revision_before,
                    revision_after,
                    ops_applied,
                    op_kinds,
                    recalc_needed,
                    ..
                } if record.transition == ResidentTransition::StageApply => {
                    let prepared = effect("prepared_transaction")
                        .ok_or_else(|| anyhow!("stage apply lacks prepared transaction"))?;
                    if head.is_some() && head != record.resulting_head {
                        bail!("staged apply head differs from committed history");
                    }
                    if recalc_needed == state.calculation_current
                        || prepared["consume_staged"].as_str() != Some(&change_id)
                        || prepared["response"]["revision_before"].as_str()
                            != Some(&revision_before)
                        || revision_after != record.state_revision
                        || prepared["response"]["ops_applied"].as_u64() != Some(ops_applied as u64)
                        || prepared["response"]["impact"]["op_kinds"]
                            != serde_json::to_value(op_kinds)?
                    {
                        bail!("canonical staged apply outcome mismatch");
                    }
                }
                StagedChangeData::Discard {
                    change_id,
                    revision_after,
                    discarded,
                    ..
                } if matches!(
                    record.transition,
                    ResidentTransition::CatalogDiscard | ResidentTransition::Receipt
                ) =>
                {
                    if (record.transition == ResidentTransition::Receipt
                        && effect("receipt_operation")
                            != Some(&serde_json::to_value(ResidentTransition::CatalogDiscard)?))
                        || effect("discard_staged_change_id").and_then(serde_json::Value::as_str)
                            != Some(&change_id)
                        || revision_after != record.state_revision
                        || discarded != (record.transition == ResidentTransition::CatalogDiscard)
                    {
                        bail!("canonical staged discard mismatch");
                    }
                }
                _ => bail!("canonical staging action/transition mismatch"),
            }
        }
        "session_history" => {
            let data: crate::session_history::SessionHistoryData =
                serde_json::from_value(response.data.clone())?;
            let crate::session_history::SessionHistoryData::Mutation {
                transition,
                revision_after,
                head,
                branch,
                ..
            } = data
            else {
                bail!("read-only history cannot authorize a transition");
            };
            let expected = if record.transition == ResidentTransition::Receipt {
                serde_json::from_value(
                    effect("receipt_operation")
                        .cloned()
                        .ok_or_else(|| anyhow!("history receipt lacks action"))?,
                )?
            } else {
                record.transition.clone()
            };
            if !matches!(
                expected,
                ResidentTransition::Undo
                    | ResidentTransition::Redo
                    | ResidentTransition::Checkout
                    | ResidentTransition::BranchCreate
                    | ResidentTransition::BranchSwitch
            ) || transition != expected
                || revision_after != record.state_revision
                || head != record.resulting_head
                || branch != record.resulting_branch
            {
                bail!("canonical history outcome/transition mismatch");
            }
        }
        _ => bail!("canonical outcome cannot authorize this transition"),
    }
    Ok(())
}
