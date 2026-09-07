use super::RecalcResult;
use crate::recalc::RecalcBackend;
use crate::utils::{column_number_to_name, hash_bytes_sha256_hex, hash_file_sha256_hex};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use formualizer::common::PackedSheetCell;
use formualizer::eval::engine::ingest::EngineLoadStream;
use formualizer::eval::engine::{Engine, EvalConfig, FormulaParsePolicy};
use formualizer::workbook::workbook::WBResolver;
use formualizer::workbook::{
    FormulaCacheUpdate, LiteralValue, SpreadsheetReader, SpreadsheetWriter, UmyaAdapter,
};
use std::collections::HashSet;
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
use web_time::Instant;

pub struct FormualizerBackend;

#[async_trait]
impl RecalcBackend for FormualizerBackend {
    async fn recalculate(
        &self,
        fork_work_path: &Path,
        timeout_ms: Option<u64>,
    ) -> Result<RecalcResult> {
        let path = fork_work_path.to_path_buf();
        // Use a dedicated thread with a 32 MiB stack instead of
        // tokio::task::spawn_blocking (which uses 2 MiB by default).
        // Deep formula chains (e.g. 30k cascading rows) can exceed 2 MiB
        // in debug builds.
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("formualizer-recalc".into())
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                let _ = tx.send(recalc_sync(&path, timeout_ms));
            })
            .map_err(|e| anyhow!("failed to spawn recalc thread: {e}"))?;
        rx.await.map_err(|_| anyhow!("recalc thread panicked"))?
    }

    fn is_available(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "formualizer"
    }
}

pub(crate) type FormualizerEngine = Engine<WBResolver>;

/// Deterministic lifecycle counters for retained-evaluator reuse assertions.
/// These count events, not time, and are also useful to adapter diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EvaluatorCounters {
    pub constructions: u64,
    pub ingests: u64,
    pub evaluations: u64,
    pub rebuilds: u64,
}

pub(crate) struct RetainedEvaluator {
    engine: FormualizerEngine,
    formula_cells: HashSet<(String, u32, u32)>,
    usable: bool,
}

pub(crate) struct EvaluatorEvaluation {
    pub cells_evaluated: u64,
    pub cache_updates: Vec<FormulaCacheUpdate>,
    pub eval_errors: Vec<String>,
    pub error_formula_cells: u64,
    pub formula_cells: u64,
}

impl RetainedEvaluator {
    pub(crate) fn ingest(adapter: &mut UmyaAdapter) -> Result<Self> {
        let eval_config = EvalConfig {
            defer_graph_building: true,
            formula_parse_policy: FormulaParsePolicy::CoerceToError,
            ..Default::default()
        };
        let mut engine = FormualizerEngine::new(WBResolver::default(), eval_config);
        adapter
            .stream_into_engine(&mut engine)
            .map_err(|e| anyhow!("failed to ingest workbook into formualizer engine: {e}"))?;
        Ok(Self {
            engine,
            formula_cells: adapter.formula_cells().into_iter().collect(),
            usable: true,
        })
    }

    #[cfg(test)]
    pub(crate) fn invalidate_for_test(&mut self) {
        self.usable = false;
    }

    pub(crate) fn set_value(
        &mut self,
        sheet: &str,
        row: u32,
        col: u32,
        value: LiteralValue,
    ) -> Result<()> {
        if !self.usable {
            return Err(anyhow!("evaluator requires rebuild"));
        }
        self.engine
            .set_cell_value(sheet, row, col, value)
            .map_err(|e| anyhow!("failed to synchronize evaluator value: {e}"))?;
        self.formula_cells.remove(&(sheet.to_string(), row, col));
        Ok(())
    }

    pub(crate) fn set_formula(
        &mut self,
        sheet: &str,
        row: u32,
        col: u32,
        formula: &str,
    ) -> Result<()> {
        if !self.usable {
            return Err(anyhow!("evaluator requires rebuild"));
        }
        let normalized = if formula.starts_with('=') {
            formula.to_string()
        } else {
            format!("={formula}")
        };
        let ast = formualizer_parse::parser::parse(&normalized)
            .map_err(|e| anyhow!("failed to parse formula for evaluator synchronization: {e}"))?;
        self.engine
            .set_cell_formula(sheet, row, col, ast)
            .map_err(|e| anyhow!("failed to synchronize evaluator formula: {e}"))?;
        self.formula_cells.insert((sheet.to_string(), row, col));
        Ok(())
    }

    pub(crate) fn evaluate(&mut self, timeout_ms: Option<u64>) -> Result<EvaluatorEvaluation> {
        if !self.usable {
            return Err(anyhow!("evaluator requires rebuild"));
        }
        let (cells_evaluated, cycle_errors, changed_cells) =
            match evaluate_with_optional_timeout(&mut self.engine, timeout_ms) {
                Ok(result) => result,
                Err(error) if timeout_ms.is_some() => {
                    // A cancelled engine may contain partial derived state. Never reuse or
                    // publish it as coverage; the owner must rebuild before another attempt.
                    self.usable = false;
                    return Err(anyhow!(
                        "evaluation interrupted before complete coverage: {error}"
                    ));
                }
                Err(error) => return Err(anyhow!("formualizer evaluate_all failed: {error}")),
            };
        let mut eval_errors = Vec::new();
        if cycle_errors > 0 {
            eval_errors.push(format!(
                "Detected {} circular reference cycle(s). Cells in cycles are reported as #CIRC! by this backend; workbooks built with Excel's iterative calculation need an iterative backend.",
                cycle_errors
            ));
        }
        let _changed_cells = changed_cells;
        let mut cache_updates = Vec::with_capacity(self.formula_cells.len());
        let mut error_formula_cells = 0;
        for (sheet_name, row, col) in &self.formula_cells {
            let value = self
                .engine
                .get_cell_value(sheet_name, *row, *col)
                .unwrap_or(LiteralValue::Empty);
            if let LiteralValue::Error(err) = &value {
                error_formula_cells += 1;
                if eval_errors.len() < 200 {
                    eval_errors.push(format!(
                        "{}!{}{}: {}",
                        sheet_name,
                        column_number_to_name(*col),
                        row,
                        err
                    ));
                }
            }
            // Complete calculation publishes a coherent cache snapshot. Delta metrics are
            // retained separately; they are not a safe filter for newly replaced formulas.
            cache_updates.push(FormulaCacheUpdate {
                sheet: sheet_name.clone(),
                row: *row,
                col: *col,
                value,
            });
        }
        Ok(EvaluatorEvaluation {
            cells_evaluated,
            cache_updates,
            eval_errors,
            error_formula_cells,
            formula_cells: self.formula_cells.len() as u64,
        })
    }

    pub(crate) fn date_system(&self) -> formualizer::common::DateSystem {
        self.engine.config.date_system
    }
}

fn recalc_sync(path: &Path, timeout_ms: Option<u64>) -> Result<RecalcResult> {
    let start = Instant::now();
    let open_start = Instant::now();
    let adapter = UmyaAdapter::open_path(path)
        .map_err(|e| anyhow!("failed to open workbook adapter {:?}: {e}", path))?;
    let open_ms = open_start.elapsed().as_millis() as u64;
    let (result, _) = recalculate_adapter_sync(
        adapter,
        timeout_ms,
        start,
        open_ms,
        RecalcPersistence::Path(path),
    )?;
    Ok(result)
}

pub fn recalculate_bytes_sync(
    bytes: &[u8],
    timeout_ms: Option<u64>,
) -> Result<(RecalcResult, Vec<u8>)> {
    let start = Instant::now();
    let open_start = Instant::now();
    let adapter = UmyaAdapter::open_bytes(bytes.to_vec())
        .map_err(|e| anyhow!("failed to open workbook adapter from bytes: {e}"))?;
    let open_ms = open_start.elapsed().as_millis() as u64;
    let (result, evaluated) = recalculate_adapter_sync(
        adapter,
        timeout_ms,
        start,
        open_ms,
        RecalcPersistence::Bytes,
    )?;
    Ok((result, evaluated.expect("bytes persistence returns bytes")))
}

enum RecalcPersistence<'a> {
    Path(&'a Path),
    Bytes,
}

fn recalculate_adapter_sync(
    mut adapter: UmyaAdapter,
    timeout_ms: Option<u64>,
    start: Instant,
    open_ms: u64,
    persistence: RecalcPersistence<'_>,
) -> Result<(RecalcResult, Option<Vec<u8>>)> {
    let stream_start = Instant::now();
    let mut evaluator = RetainedEvaluator::ingest(&mut adapter)?;
    let stream_ms = stream_start.elapsed().as_millis() as u64;

    let eval_start = Instant::now();
    let evaluation = match evaluator.evaluate(timeout_ms) {
        Ok(evaluation) => evaluation,
        Err(error) if timeout_ms.is_some() => {
            // Stateless compatibility: interruption is a partial result, but no cache or
            // current coverage is ever published.
            let (unchanged_bytes, revision_id) = match persistence {
                RecalcPersistence::Path(path) => (None, hash_file_sha256_hex(path)?),
                RecalcPersistence::Bytes => {
                    let bytes = adapter
                        .save_to_bytes()
                        .map_err(|e| anyhow!("failed to serialize unchanged workbook: {e}"))?;
                    let revision = hash_bytes_sha256_hex(&bytes);
                    (Some(bytes), revision)
                }
            };
            return Ok((
                RecalcResult {
                    duration_ms: start.elapsed().as_millis() as u64,
                    was_warm: false,
                    backend_name: "formualizer",
                    cells_evaluated: Some(0),
                    eval_errors: Some(vec![error.to_string()]),
                    evaluation_coverage: crate::model::EvaluationCoverage {
                        formula_cells: evaluator.formula_cells.len() as u64,
                        evaluated_formula_cells: 0,
                        unsupported_formula_cells: 0,
                        error_formula_cells: 0,
                        source: crate::model::EvaluationSource::None,
                        freshness: crate::model::EvaluationFreshness::Unknown,
                        revision_id,
                    },
                    incomplete: true,
                },
                unchanged_bytes,
            ));
        }
        Err(error) => return Err(error),
    };
    let evaluate_ms = eval_start.elapsed().as_millis() as u64;
    let updates_len = evaluation.cache_updates.len();
    let write_start = Instant::now();
    if !evaluation.cache_updates.is_empty() {
        adapter
            .write_formula_caches_batch(&evaluation.cache_updates, evaluator.date_system())
            .map_err(|e| anyhow!("failed to write formula caches in batch: {e}"))?;
    }
    let write_formula_caches_batch_ms = write_start.elapsed().as_millis() as u64;
    let save_start = Instant::now();
    let (evaluated_bytes, revision_id) = match persistence {
        RecalcPersistence::Path(path) => {
            if !evaluation.cache_updates.is_empty() {
                adapter
                    .save_as_path(path)
                    .map_err(|e| anyhow!("failed to save recalculated workbook {:?}: {e}", path))?;
            }
            (None, hash_file_sha256_hex(path)?)
        }
        RecalcPersistence::Bytes => {
            let bytes = adapter
                .save_to_bytes()
                .map_err(|e| anyhow!("failed to serialize recalculated workbook: {e}"))?;
            let revision = hash_bytes_sha256_hex(&bytes);
            (Some(bytes), revision)
        }
    };
    let save_as_path_ms = save_start.elapsed().as_millis() as u64;
    let total_ms = start.elapsed().as_millis() as u64;
    tracing::trace!(target: "asp::recalc::timing", open_ms,
        stream_into_engine_ms = stream_ms, evaluate_ms,
        write_formula_caches_batch_ms, save_as_path_ms,
        formula_cells_len = evaluation.formula_cells, updates_len, total_ms,
        "formualizer recalc timing");

    Ok((
        RecalcResult {
            duration_ms: total_ms,
            // This API creates and ingests an ephemeral evaluator for every invocation.
            was_warm: false,
            backend_name: "formualizer",
            cells_evaluated: Some(evaluation.cells_evaluated),
            eval_errors: (!evaluation.eval_errors.is_empty()).then_some(evaluation.eval_errors),
            evaluation_coverage: crate::model::EvaluationCoverage {
                formula_cells: evaluation.formula_cells,
                evaluated_formula_cells: evaluation.formula_cells,
                unsupported_formula_cells: 0,
                error_formula_cells: evaluation.error_formula_cells,
                source: crate::model::EvaluationSource::Formualizer,
                freshness: crate::model::EvaluationFreshness::CurrentRevision,
                revision_id,
            },
            incomplete: false,
        },
        evaluated_bytes,
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn evaluate_with_optional_timeout(
    engine: &mut FormualizerEngine,
    timeout_ms: Option<u64>,
) -> Result<(u64, u64, Option<HashSet<PackedSheetCell>>)> {
    let Some(timeout_ms) = timeout_ms else {
        let (eval, delta) = engine.evaluate_all_with_delta()?;
        let changed = delta.changed_cells.into_iter().collect::<HashSet<_>>();
        return Ok((
            eval.computed_vertices as u64,
            eval.cycle_errors as u64,
            Some(changed),
        ));
    };

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let done_flag = Arc::new(AtomicBool::new(false));
    let cancel_for_thread = cancel_flag.clone();
    let done_for_thread = done_flag.clone();

    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        // Relaxed is sufficient: flag is monotonic false->true, no data synchronized.
        while !done_for_thread.load(Ordering::Relaxed) {
            if Instant::now() >= deadline {
                cancel_for_thread.store(true, Ordering::Relaxed);
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
    });

    let result = engine.evaluate_all_cancellable(cancel_flag.into());
    done_flag.store(true, Ordering::Relaxed);
    let _ = handle.join();

    let eval = result?;
    Ok((
        eval.computed_vertices as u64,
        eval.cycle_errors as u64,
        None,
    ))
}

#[cfg(target_arch = "wasm32")]
fn evaluate_with_optional_timeout(
    engine: &mut FormualizerEngine,
    _timeout_ms: Option<u64>,
) -> Result<(u64, u64, Option<HashSet<PackedSheetCell>>)> {
    // Browser and Node wasm32 have no portable preemptive thread primitive. Evaluation remains
    // synchronous and in-memory; callers can terminate a Web Worker for a hard deadline.
    let (eval, delta) = engine.evaluate_all_with_delta()?;
    let changed = delta.changed_cells.into_iter().collect::<HashSet<_>>();
    Ok((
        eval.computed_vertices as u64,
        eval.cycle_errors as u64,
        Some(changed),
    ))
}
