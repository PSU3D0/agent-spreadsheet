//! Read-only canonical execution context. Borrowed resident projections and
//! ordinary file readers execute the same semantic helpers.
use crate::{
    config::ServerConfig,
    model::{CalculationMetadata, WorkbookId, WorkbookListResponse},
    state::AppState,
    tools::filters::WorkbookFilter,
    workbook::{WorkbookContext, WorkbookReadSource},
};
use anyhow::Result;
use std::{future::Future, sync::Arc};

pub trait ReadContext: Clone + Send + Sync {
    type Source: WorkbookReadSource + Send + Sync;
    fn config(&self) -> Arc<ServerConfig>;
    fn with_workbook<
        T: Send + 'static,
        F: FnOnce(Arc<WorkbookContext<Self::Source>>) -> Result<T> + Send + 'static,
    >(
        &self,
        workbook: Arc<WorkbookContext<Self::Source>>,
        f: F,
    ) -> impl Future<Output = Result<T>> + Send;
    fn open_workbook<'a>(
        &'a self,
        id: &'a WorkbookId,
    ) -> impl Future<Output = Result<Arc<WorkbookContext<Self::Source>>>> + Send + 'a;
    fn list_workbooks(&self, filter: WorkbookFilter) -> Result<WorkbookListResponse>;
    fn close_workbook(&self, id: &WorkbookId) -> Result<()>;
    fn calculation_metadata(
        &self,
        id: &WorkbookId,
        revision: &str,
        fallback: CalculationMetadata,
    ) -> CalculationMetadata;
    fn recalc_needed(&self, id: &WorkbookId) -> bool;
}

impl ReadContext for Arc<AppState> {
    type Source = Arc<parking_lot::RwLock<umya_spreadsheet::Spreadsheet>>;
    fn config(&self) -> Arc<ServerConfig> {
        self.as_ref().config()
    }
    async fn with_workbook<
        T: Send + 'static,
        F: FnOnce(Arc<WorkbookContext<Self::Source>>) -> Result<T> + Send + 'static,
    >(
        &self,
        workbook: Arc<WorkbookContext<Self::Source>>,
        f: F,
    ) -> Result<T> {
        crate::runtime::maybe_blocking(move || f(workbook)).await?
    }
    async fn open_workbook(&self, id: &WorkbookId) -> Result<Arc<WorkbookContext>> {
        self.as_ref().open_workbook(id).await
    }
    fn list_workbooks(&self, filter: WorkbookFilter) -> Result<WorkbookListResponse> {
        self.as_ref().list_workbooks(filter)
    }
    fn close_workbook(&self, id: &WorkbookId) -> Result<()> {
        self.as_ref().close_workbook(id)
    }
    fn calculation_metadata(
        &self,
        id: &WorkbookId,
        revision: &str,
        fallback: CalculationMetadata,
    ) -> CalculationMetadata {
        self.as_ref().calculation_metadata(id, revision, fallback)
    }
    fn recalc_needed(&self, id: &WorkbookId) -> bool {
        #[cfg(not(feature = "recalc"))]
        let _ = id;
        #[cfg(feature = "recalc")]
        if let Some(registry) = self.fork_registry() {
            return registry
                .get_fork(&id.0)
                .is_ok_and(|fork| fork.recalc_needed);
        }
        false
    }
}

#[derive(Clone)]
pub struct BorrowedReadContext<'a> {
    pub view: Arc<WorkbookContext<&'a umya_spreadsheet::Spreadsheet>>,
    pub config: Arc<ServerConfig>,
}
impl<'a> ReadContext for BorrowedReadContext<'a> {
    type Source = &'a umya_spreadsheet::Spreadsheet;
    fn config(&self) -> Arc<ServerConfig> {
        self.config.clone()
    }
    async fn with_workbook<
        T: Send + 'static,
        F: FnOnce(Arc<WorkbookContext<Self::Source>>) -> Result<T> + Send + 'static,
    >(
        &self,
        workbook: Arc<WorkbookContext<Self::Source>>,
        f: F,
    ) -> Result<T> {
        f(workbook)
    }
    async fn open_workbook(&self, id: &WorkbookId) -> Result<Arc<WorkbookContext<Self::Source>>> {
        if *id != self.view.id {
            anyhow::bail!("resource does not belong to this resident owner");
        }
        Ok(self.view.clone())
    }
    fn list_workbooks(&self, _filter: WorkbookFilter) -> Result<WorkbookListResponse> {
        anyhow::bail!("resource listing requires the runtime catalog");
    }
    fn close_workbook(&self, _id: &WorkbookId) -> Result<()> {
        anyhow::bail!("detach requires the runtime lifecycle");
    }
    fn calculation_metadata(
        &self,
        _id: &WorkbookId,
        _revision: &str,
        _fallback: CalculationMetadata,
    ) -> CalculationMetadata {
        self.view.calculation_metadata()
    }
    fn recalc_needed(&self, _id: &WorkbookId) -> bool {
        !self
            .view
            .imported_evaluation_coverage()
            .is_complete_and_fresh()
    }
}
