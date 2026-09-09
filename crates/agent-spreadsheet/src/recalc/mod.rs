#[cfg(feature = "recalc")]
mod backend;
#[cfg(feature = "recalc")]
mod executor;
#[cfg(feature = "recalc-libreoffice")]
mod fire_and_forget;
#[cfg(feature = "recalc-formualizer")]
mod formualizer_backend;
#[cfg(feature = "recalc")]
pub mod macro_uri;
#[cfg(feature = "recalc-libreoffice")]
mod pooled;
#[cfg(feature = "recalc-formualizer")]
mod resident;
#[cfg(feature = "recalc-libreoffice")]
mod screenshot;

#[cfg(feature = "recalc-libreoffice")]
pub use backend::LibreOfficeBackend;
#[cfg(feature = "recalc")]
pub use backend::RecalcBackend;
#[cfg(feature = "recalc-libreoffice")]
pub use executor::RecalcExecutor;
#[cfg(feature = "recalc")]
pub use executor::RecalcResult;
#[cfg(feature = "recalc-libreoffice")]
pub use fire_and_forget::FireAndForgetExecutor;
#[cfg(feature = "recalc-formualizer")]
pub use formualizer_backend::{EvaluatorCounters, FormualizerBackend, recalculate_bytes_sync};
#[cfg(feature = "recalc-formualizer")]
pub use resident::{CalculationStamp, ExportStamp, ResidentRevision, ResidentWorkbook};
#[cfg(feature = "recalc-formualizer")]
pub(crate) use resident::{
    ResidentCalculationEffect, ResidentMaterializedCell, ResidentMaterializedValue,
    ResidentPreparedCellEffect, materialize_umya_cell,
};
#[cfg(feature = "recalc-libreoffice")]
pub use screenshot::{ScreenshotExecutor, ScreenshotResult};

use std::path::PathBuf;
#[cfg(feature = "recalc")]
use std::sync::Arc;
#[cfg(feature = "recalc")]
use tokio::sync::Semaphore;

#[cfg(feature = "recalc")]
#[derive(Clone)]
pub struct GlobalRecalcLock(pub Arc<Semaphore>);

#[cfg(feature = "recalc")]
impl GlobalRecalcLock {
    pub fn new(permits: usize) -> Self {
        Self(Arc::new(Semaphore::new(permits)))
    }
}

#[cfg(feature = "recalc")]
#[derive(Clone)]
pub struct GlobalScreenshotLock(pub Arc<Semaphore>);

#[cfg(feature = "recalc")]
impl GlobalScreenshotLock {
    pub fn new() -> Self {
        Self(Arc::new(Semaphore::new(1)))
    }
}

#[cfg(feature = "recalc")]
impl Default for GlobalScreenshotLock {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum ExecutorStrategy {
    #[default]
    FireAndForget,
    Pooled,
}

#[derive(Debug, Clone)]
pub struct RecalcConfig {
    pub soffice_path: Option<PathBuf>,
    pub timeout_ms: Option<u64>,
    pub strategy: ExecutorStrategy,
}

impl Default for RecalcConfig {
    fn default() -> Self {
        Self {
            soffice_path: None,
            timeout_ms: Some(30_000),
            strategy: ExecutorStrategy::FireAndForget,
        }
    }
}

#[cfg(feature = "recalc-libreoffice")]
pub fn create_executor(config: &RecalcConfig) -> Arc<dyn RecalcExecutor> {
    match config.strategy {
        ExecutorStrategy::FireAndForget => Arc::new(FireAndForgetExecutor::new(config)),
        ExecutorStrategy::Pooled => {
            tracing::warn!("Pooled executor not yet implemented, falling back to fire-and-forget");
            Arc::new(FireAndForgetExecutor::new(config))
        }
    }
}
