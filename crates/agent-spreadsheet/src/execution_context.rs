//! Typed backend seam for the single canonical operation dispatcher.
//! Transport adapters bind resources; implementations do not own a registry.
use crate::{
    operations::{ResourceId, RuntimeCapabilities},
    read_context::ReadContext,
    state::AppState,
};
use anyhow::Result;
use std::sync::Arc;

#[allow(async_fn_in_trait)]
pub trait ExecutionContext {
    type Reads<'a>: ReadContext
    where
        Self: 'a;
    fn reads(&self) -> Result<Self::Reads<'_>, crate::operations::CanonicalErrorEnvelope>;
    fn capabilities(&self) -> RuntimeCapabilities;
    async fn identify(&self, resource: &ResourceId) -> Result<(ResourceId, String)>;
    async fn identify_diagnostic(
        &self,
        resource: &ResourceId,
    ) -> Result<(ResourceId, Option<String>)> {
        self.identify(resource)
            .await
            .map(|(resource, revision)| (resource, Some(revision)))
    }
    #[cfg(feature = "recalc")]
    async fn session_history(
        &mut self,
        _request: crate::session_history::SessionHistoryRequest,
    ) -> Result<crate::session_history::SessionHistoryData> {
        anyhow::bail!("resident history unavailable for this binding")
    }

    #[cfg(feature = "recalc")]
    async fn write(
        &mut self,
        _request: crate::canonical_write::WriteRequest,
    ) -> Result<crate::canonical_write::WriteResponseData> {
        anyhow::bail!("write unavailable for this binding")
    }
    #[cfg(feature = "recalc")]
    async fn create_fork(
        &mut self,
        _request: crate::canonical_lifecycle::CreateForkRequest,
    ) -> Result<crate::canonical_lifecycle::CreateForkData> {
        anyhow::bail!("create_fork unavailable for this binding")
    }
    #[cfg(feature = "recalc")]
    async fn list_forks(
        &mut self,
        _request: crate::canonical_lifecycle::ListForksRequest,
    ) -> Result<crate::canonical_lifecycle::ListForksData> {
        anyhow::bail!("list_forks unavailable for this binding")
    }
    #[cfg(feature = "recalc")]
    async fn recalculate(
        &mut self,
        _request: crate::canonical_lifecycle::RecalculateRequest,
    ) -> Result<crate::canonical_lifecycle::RecalculateData> {
        anyhow::bail!("recalculate unavailable for this binding")
    }
    #[cfg(feature = "recalc")]
    async fn verify_workbook(
        &mut self,
        _request: crate::canonical_lifecycle::VerifyWorkbookRequest,
    ) -> Result<crate::canonical_lifecycle::VerifyWorkbookData> {
        anyhow::bail!("verify unavailable for this binding")
    }
    #[cfg(feature = "recalc")]
    async fn export_fork(
        &mut self,
        _request: crate::canonical_lifecycle::ExportForkRequest,
    ) -> Result<crate::canonical_lifecycle::ExportForkData> {
        anyhow::bail!("export unavailable for this binding")
    }
    #[cfg(feature = "recalc")]
    async fn discard_fork(
        &mut self,
        _request: crate::canonical_lifecycle::DiscardForkRequest,
    ) -> Result<crate::canonical_lifecycle::DiscardForkData> {
        anyhow::bail!("discard unavailable for this binding")
    }
    #[cfg(feature = "recalc")]
    async fn get_changes(
        &mut self,
        _request: crate::canonical_lifecycle::GetChangesRequest,
    ) -> Result<crate::canonical_lifecycle::GetChangesData> {
        anyhow::bail!("history unavailable for this binding")
    }
    #[cfg(feature = "recalc")]
    async fn checkpoint(
        &mut self,
        _request: crate::canonical_lifecycle::CheckpointRequest,
    ) -> Result<crate::canonical_lifecycle::CheckpointData> {
        anyhow::bail!("checkpoint unavailable for this binding")
    }
    #[cfg(feature = "recalc")]
    async fn staged_change(
        &mut self,
        _request: crate::canonical_lifecycle::StagedChangeRequest,
    ) -> Result<crate::canonical_lifecycle::StagedChangeData> {
        anyhow::bail!("staging unavailable for this binding")
    }
    #[cfg(feature = "recalc")]
    async fn screenshot_sheet(
        &mut self,
        _request: crate::canonical_optional::ScreenshotSheetRequest,
    ) -> Result<crate::canonical_optional::ScreenshotSheetData> {
        anyhow::bail!("screenshot unavailable for this binding")
    }
    #[cfg(feature = "recalc-formualizer")]
    async fn sheetport_manifest(
        &mut self,
        _request: crate::canonical_optional::SheetportManifestRequest,
    ) -> Result<crate::canonical_optional::SheetportManifestData> {
        anyhow::bail!("sheetport unavailable for this binding")
    }
    #[cfg(feature = "recalc-formualizer")]
    async fn execute_sheetport(
        &mut self,
        _request: crate::canonical_optional::ExecuteSheetportRequest,
    ) -> Result<crate::canonical_optional::ExecuteSheetportData> {
        anyhow::bail!("sheetport unavailable for this binding")
    }
    async fn inspect_vba(
        &mut self,
        _request: crate::canonical_optional::InspectVbaRequest,
        _revision: &str,
    ) -> Result<crate::canonical_optional::InspectVbaData> {
        anyhow::bail!("VBA unavailable for this binding")
    }
}

impl ExecutionContext for Arc<AppState> {
    type Reads<'a> = Arc<AppState>;
    fn reads(&self) -> Result<Self::Reads<'_>, crate::operations::CanonicalErrorEnvelope> {
        Ok(self.clone())
    }
    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::from_state(self)
    }
    async fn identify(&self, resource: &ResourceId) -> Result<(ResourceId, String)> {
        let id = resource.to_workbook_id();
        #[allow(unused_mut)]
        let mut workbook = self.open_workbook(&id).await?;
        #[allow(unused_mut)]
        let mut revision = workbook.revision_id.clone();
        #[cfg(all(not(target_arch = "wasm32"), feature = "native-fs", feature = "recalc"))]
        if resource.as_str().starts_with("fork:") {
            let registry = self
                .fork_registry()
                .ok_or_else(|| anyhow::anyhow!("fork registry not available"))?;
            let (state_revision, content_revision) = registry.sync_fork_revisions(id.as_str())?;
            if workbook.revision_id != content_revision {
                self.evict_by_path(&workbook.path);
                workbook = self.open_workbook(&id).await?;
            }
            revision = state_revision;
        }
        Ok((
            ResourceId::bind_workbook(&workbook.id).map_err(anyhow::Error::msg)?,
            revision,
        ))
    }
    #[cfg(all(not(target_arch = "wasm32"), feature = "native-fs", feature = "recalc"))]
    async fn write(
        &mut self,
        request: crate::canonical_write::WriteRequest,
    ) -> Result<crate::canonical_write::WriteResponseData> {
        crate::canonical_write::execute_write(self.clone(), request).await
    }
    #[cfg(feature = "recalc")]
    async fn create_fork(
        &mut self,
        request: crate::canonical_lifecycle::CreateForkRequest,
    ) -> Result<crate::canonical_lifecycle::CreateForkData> {
        crate::canonical_lifecycle::create_fork(self.clone(), request).await
    }
    #[cfg(feature = "recalc")]
    async fn list_forks(
        &mut self,
        request: crate::canonical_lifecycle::ListForksRequest,
    ) -> Result<crate::canonical_lifecycle::ListForksData> {
        crate::canonical_lifecycle::list_forks(self.clone(), request)
    }
    #[cfg(feature = "recalc")]
    async fn recalculate(
        &mut self,
        request: crate::canonical_lifecycle::RecalculateRequest,
    ) -> Result<crate::canonical_lifecycle::RecalculateData> {
        crate::canonical_lifecycle::recalculate(self.clone(), request).await
    }
    #[cfg(all(feature = "recalc", feature = "native-fs"))]
    async fn verify_workbook(
        &mut self,
        request: crate::canonical_lifecycle::VerifyWorkbookRequest,
    ) -> Result<crate::canonical_lifecycle::VerifyWorkbookData> {
        crate::canonical_lifecycle::verify_workbook(self.clone(), request).await
    }
    #[cfg(feature = "recalc")]
    async fn export_fork(
        &mut self,
        request: crate::canonical_lifecycle::ExportForkRequest,
    ) -> Result<crate::canonical_lifecycle::ExportForkData> {
        crate::canonical_lifecycle::export_fork(self.clone(), request)
    }
    #[cfg(feature = "recalc")]
    async fn discard_fork(
        &mut self,
        request: crate::canonical_lifecycle::DiscardForkRequest,
    ) -> Result<crate::canonical_lifecycle::DiscardForkData> {
        crate::canonical_lifecycle::discard_fork(self.clone(), request)
    }
    #[cfg(feature = "recalc")]
    async fn get_changes(
        &mut self,
        request: crate::canonical_lifecycle::GetChangesRequest,
    ) -> Result<crate::canonical_lifecycle::GetChangesData> {
        crate::canonical_lifecycle::get_changes(self.clone(), request).await
    }
    #[cfg(feature = "recalc")]
    async fn checkpoint(
        &mut self,
        request: crate::canonical_lifecycle::CheckpointRequest,
    ) -> Result<crate::canonical_lifecycle::CheckpointData> {
        crate::canonical_lifecycle::checkpoint(self.clone(), request)
    }
    #[cfg(feature = "recalc")]
    async fn staged_change(
        &mut self,
        request: crate::canonical_lifecycle::StagedChangeRequest,
    ) -> Result<crate::canonical_lifecycle::StagedChangeData> {
        crate::canonical_lifecycle::staged_change(self.clone(), request)
    }
    #[cfg(feature = "recalc")]
    async fn screenshot_sheet(
        &mut self,
        request: crate::canonical_optional::ScreenshotSheetRequest,
    ) -> Result<crate::canonical_optional::ScreenshotSheetData> {
        crate::canonical_optional::screenshot_sheet(self.clone(), request).await
    }
    #[cfg(feature = "recalc-formualizer")]
    async fn sheetport_manifest(
        &mut self,
        request: crate::canonical_optional::SheetportManifestRequest,
    ) -> Result<crate::canonical_optional::SheetportManifestData> {
        crate::canonical_optional::execute_sheetport_manifest_action(self.clone(), request).await
    }
    #[cfg(feature = "recalc-formualizer")]
    async fn execute_sheetport(
        &mut self,
        request: crate::canonical_optional::ExecuteSheetportRequest,
    ) -> Result<crate::canonical_optional::ExecuteSheetportData> {
        crate::canonical_optional::execute_sheetport(self.clone(), request).await
    }
    async fn inspect_vba(
        &mut self,
        request: crate::canonical_optional::InspectVbaRequest,
        revision: &str,
    ) -> Result<crate::canonical_optional::InspectVbaData> {
        crate::canonical_optional::inspect_vba(self.clone(), request, revision).await
    }
}
