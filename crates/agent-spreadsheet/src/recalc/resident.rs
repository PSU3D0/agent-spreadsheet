//! Portable, in-process ownership for an authoritative Umya document and a
//! disposable retained Formualizer evaluator.
//!
//! Host paths, persistence, locking, transactions, and resource tables are
//! intentionally outside this type. This is the calculation/document seam used
//! by those later layers; XLSX bytes are import/export snapshots, never the
//! intermediate representation for common value/formula edits.

use super::formualizer_backend::{EvaluatorCounters, RetainedEvaluator};
use crate::core::session::{
    SessionApplySummary, SessionEvaluatorCell, SessionMatrixCell, WorkbookSession,
};
use crate::model::{EvaluationCoverage, EvaluationFreshness, EvaluationSource};
use crate::utils::hash_bytes_sha256_hex;
use anyhow::{Result, anyhow};
use formualizer::workbook::{SpreadsheetReader, UmyaAdapter};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum ResidentMaterializedValue {
    Empty,
    String(String),
    RichText(String),
    Lazy(String),
    Number(f64),
    Bool(bool),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ResidentMaterializedCell {
    pub value: ResidentMaterializedValue,
    pub formula: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ResidentPreparedCellEffect {
    pub sheet_name: String,
    pub column: u32,
    pub row: u32,
    pub source_op_indices: Vec<usize>,
    pub expected_before: Option<ResidentMaterializedCell>,
    pub after: ResidentMaterializedCell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResidentRevision {
    /// Resource-scoped epoch; hosts persist it or rotate it on recovery so a
    /// pre-restart token cannot ABA-match a different resident resource.
    pub epoch: String,
    /// Advances for committed document/history transitions only.
    pub document: u64,
    /// Opaque workbook CAS/cursor generation. Complete calculation publication
    /// advances it even though calculation is not a history edit.
    pub state: u64,
}

#[derive(Debug, Clone)]
pub enum CalculationStamp {
    NotEvaluated {
        document_revision: u64,
    },
    Dirty {
        document_revision: u64,
    },
    Current {
        document_revision: u64,
        coverage: EvaluationCoverage,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportStamp {
    pub epoch: String,
    pub document_revision: u64,
    pub state_revision: u64,
    pub content_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ResidentCalculationEffect {
    Preserve,
    Invalidate,
}

/// One portable resident workbook. It is deliberately not `Clone`, wrapped in
/// a global mutex, or marked Send/Sync: an adapter chooses an appropriate owner
/// lane and serializes commits around it.
pub struct ResidentWorkbook {
    document: WorkbookSession,
    evaluator: Option<RetainedEvaluator>,
    revisions: ResidentRevision,
    calculation: CalculationStamp,
    counters: EvaluatorCounters,
    serializations: std::cell::Cell<u64>,
    snapshot: std::cell::RefCell<Option<(ResidentRevision, std::sync::Arc<[u8]>)>>,
    last_export: Option<ExportStamp>,
}

impl ResidentWorkbook {
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Result<Self> {
        let bytes = bytes.as_ref();
        let document = WorkbookSession::from_bytes(bytes)?;
        let mut adapter = UmyaAdapter::open_bytes(bytes.to_vec())
            .map_err(|error| anyhow!("failed to open evaluator adapter: {error}"))?;
        let evaluator = RetainedEvaluator::ingest(&mut adapter)?;
        Ok(Self {
            document,
            evaluator: Some(evaluator),
            revisions: ResidentRevision {
                epoch: uuid::Uuid::new_v4().to_string(),
                document: 0,
                state: 0,
            },
            calculation: CalculationStamp::NotEvaluated {
                document_revision: 0,
            },
            counters: EvaluatorCounters {
                constructions: 1,
                ingests: 1,
                ..Default::default()
            },
            last_export: None,
            serializations: std::cell::Cell::new(0),
            snapshot: std::cell::RefCell::new(None),
        })
    }

    pub fn revisions(&self) -> &ResidentRevision {
        &self.revisions
    }
    pub fn calculation_stamp(&self) -> &CalculationStamp {
        &self.calculation
    }
    pub fn last_export(&self) -> Option<&ExportStamp> {
        self.last_export.as_ref()
    }

    pub fn evaluator_counters(&self) -> EvaluatorCounters {
        self.counters
    }

    /// Warm read using the existing WorkbookSession semantic helper. No XLSX
    /// serialization or reader reconstruction occurs.
    pub fn range_values(
        &self,
        sheet_name: &str,
        ranges: impl Into<crate::core::session::SessionRangeSelection>,
    ) -> Result<Vec<crate::model::RangeValuesEntry>> {
        self.document.range_values(sheet_name, ranges)
    }

    /// Warm page read using the shared session projection implementation.
    pub fn sheet_page(
        &self,
        params: crate::core::session::SessionSheetPageParams,
    ) -> Result<crate::model::SheetPageResponse> {
        self.document.sheet_page(params)
    }

    pub fn state_revision_id(&self) -> String {
        format!(
            "resident:{}:{}:{}",
            self.revisions.epoch, self.revisions.document, self.revisions.state
        )
    }

    pub fn serialization_count(&self) -> u64 {
        self.serializations.get()
    }

    pub fn snapshot_bytes(&self) -> Result<Vec<u8>> {
        // XLSX is a lazy derived export, never the live workbook authority.
        // Reuse an exact-revision capture: Umya's serialization tables contain
        // interior counters, so repeated writes need not be byte-identical.
        if let Some((revision, bytes)) = self.snapshot.borrow().as_ref()
            && revision == &self.revisions
        {
            return Ok(bytes.to_vec());
        }
        self.serializations.set(self.serializations.get() + 1);
        let bytes = self.document.to_bytes()?;
        *self.snapshot.borrow_mut() = Some((self.revisions.clone(), bytes.clone().into()));
        Ok(bytes)
    }

    /// Borrow the authoritative document for legacy read-only projections.
    /// Mutations still go through prepared resident transactions.
    pub fn legacy_read_session(&self) -> &WorkbookSession {
        &self.document
    }

    pub(crate) fn spreadsheet(&self) -> &umya_spreadsheet::Spreadsheet {
        self.document.spreadsheet()
    }

    pub(crate) fn materialized_cell(
        &self,
        sheet_name: &str,
        column: u32,
        row: u32,
    ) -> Result<Option<ResidentMaterializedCell>> {
        let sheet = self
            .document
            .spreadsheet()
            .get_sheet_by_name(sheet_name)
            .ok_or_else(|| anyhow!("sheet '{sheet_name}' not found"))?;
        Ok(sheet.get_cell((column, row)).map(materialize_umya_cell))
    }

    pub(crate) fn publish_prepared_cells(
        &mut self,
        effects: &[ResidentPreparedCellEffect],
    ) -> Result<String> {
        self.publish_prepared_cells_inner(effects, true)
    }

    pub(crate) fn recover_prepared_cells(
        &mut self,
        effects: &[ResidentPreparedCellEffect],
    ) -> Result<String> {
        self.publish_prepared_cells_inner(effects, false)
    }

    fn publish_prepared_cells_inner(
        &mut self,
        effects: &[ResidentPreparedCellEffect],
        check_before: bool,
    ) -> Result<String> {
        for effect in effects {
            let current = self.materialized_cell(&effect.sheet_name, effect.column, effect.row)?;
            let matches = if check_before {
                current == effect.expected_before
            } else {
                logical_predecessor_matches(current.as_ref(), effect.expected_before.as_ref())
            };
            if !matches {
                return Err(anyhow!(
                    "prepared cell logical precondition changed at {}!{}",
                    effect.sheet_name,
                    crate::utils::cell_address(effect.column, effect.row)
                ));
            }
        }
        for effect in effects {
            let sheet = self
                .document
                .spreadsheet_mut()
                .get_sheet_by_name_mut(&effect.sheet_name)
                .ok_or_else(|| anyhow!("sheet '{}' not found", effect.sheet_name))?;
            assign_materialized_cell(
                sheet.get_cell_mut((effect.column, effect.row)),
                &effect.after,
            )?;
        }
        if effects.is_empty() {
            return Ok(self.state_revision_id());
        }
        self.revisions.document += 1;
        self.revisions.state += 1;
        self.calculation = CalculationStamp::Dirty {
            document_revision: self.revisions.document,
        };
        if let Some(evaluator) = self.evaluator.as_mut() {
            for effect in effects {
                let update =
                    self.document
                        .evaluator_cell(&effect.sheet_name, effect.column, effect.row)?;
                let synchronized = match update {
                    SessionEvaluatorCell::Formula(formula) => evaluator.set_formula(
                        &effect.sheet_name,
                        effect.row,
                        effect.column,
                        &formula,
                    ),
                    SessionEvaluatorCell::Value(value) => {
                        evaluator.set_value(&effect.sheet_name, effect.row, effect.column, value)
                    }
                };
                if synchronized.is_err() {
                    self.evaluator = None;
                    break;
                }
            }
        }
        Ok(self.state_revision_id())
    }

    pub(crate) fn restore_revision_identity(&mut self, epoch: String, document: u64, state: u64) {
        self.revisions = ResidentRevision {
            epoch,
            document,
            state,
        };
        self.calculation = CalculationStamp::Dirty {
            document_revision: document,
        };
        self.evaluator = None;
    }

    #[cfg(test)]
    pub(crate) fn fail_next_evaluation_for_test(&mut self) {
        self.evaluator.as_mut().unwrap().invalidate_for_test();
    }

    pub(crate) fn prepare_calculation(
        &mut self,
        timeout_ms: Option<u64>,
    ) -> Result<(
        super::formualizer_backend::EvaluatorEvaluation,
        formualizer::eval::engine::DateSystem,
        u64,
    )> {
        self.ensure_evaluator()?;
        self.counters.evaluations += 1;
        let evaluator = self.evaluator.as_mut().expect("evaluator ensured");
        let evaluation_started = web_time::Instant::now();
        match evaluator.evaluate(timeout_ms) {
            Ok(evaluation) => Ok((evaluation, evaluator.date_system(), evaluation_started.elapsed().as_millis() as u64)),
            Err(error) => {
                // Derived partial work is disposable; publication/proof belongs to
                // the durable owner and must not precede its commit.
                self.evaluator = None;
                Err(error)
            }
        }
    }

    /// External engines are explicitly cold. Only their formula caches are
    /// imported; their rewritten OOXML never replaces the authoritative document.
    #[cfg(feature = "native-fs")]
    pub(crate) async fn prepare_external_calculation(
        &mut self,
        backend: std::sync::Arc<dyn super::RecalcBackend>,
        timeout_ms: Option<u64>,
    ) -> Result<(super::formualizer_backend::EvaluatorEvaluation,
        formualizer::eval::engine::DateSystem, crate::core::types::RecalculateOutcome)> {
        let directory = crate::hostfs::tempdir()?;
        let path = directory.path().join("calculation.xlsx");
        std::fs::write(&path, self.snapshot_bytes()?)?;
        self.counters.evaluations += 1;
        let mut outcome = crate::core::recalc::execute_with_backend(&path, timeout_ms, backend).await?;
        if !outcome.evaluation_coverage.is_complete_and_fresh() {
            anyhow::bail!("external calculation did not produce complete current coverage");
        }
        // Read external caches without constructing or evaluating a second engine.
        let mut evaluated = UmyaAdapter::open_path(&path)
            .map_err(|error| anyhow!("failed to read external calculation: {error}"))?;
        let mut updates = Vec::new();
        let mut errors = 0;
        for sheet in self.document.spreadsheet().get_sheet_collection() {
            let values = evaluated.read_sheet(sheet.get_name())
                .map_err(|error| anyhow!("external calculation sheet missing: {error}"))?;
            for cell in sheet.get_cell_collection().into_iter().filter(|cell| cell.is_formula()) {
                let coordinate = cell.get_coordinate();
                let row = *coordinate.get_row_num();
                let col = *coordinate.get_col_num();
                let value = values.cells.get(&(row, col)).and_then(|cell| cell.value.clone())
                    .ok_or_else(|| anyhow!("external calculation omitted cache {}!R{row}C{col}", sheet.get_name()))?;
                errors += u64::from(matches!(value, formualizer::workbook::LiteralValue::Error(_)));
                updates.push(formualizer::workbook::FormulaCacheUpdate {
                    sheet: sheet.get_name().to_owned(), row, col, value,
                });
            }
        }
        // Counts describe the original formulas, not formulas rewritten by the
        // external engine. Switching back to Formualizer requires a rebuild.
        self.evaluator = None;
        let count = updates.len() as u64;
        outcome.evaluation_coverage.formula_cells = count;
        outcome.evaluation_coverage.evaluated_formula_cells = count;
        outcome.evaluation_coverage.error_formula_cells = errors;
        outcome.state = outcome.evaluation_coverage.state();
        Ok((super::formualizer_backend::EvaluatorEvaluation {
            cells_evaluated: count, cache_updates: updates, eval_errors: vec![],
            error_formula_cells: errors, formula_cells: count,
        }, formualizer::eval::engine::DateSystem::Excel1900, outcome))
    }

    pub(crate) fn publish_prepared_calculation(
        &mut self,
        evaluation: super::formualizer_backend::EvaluatorEvaluation,
        date_system: formualizer::eval::engine::DateSystem,
        mut coverage: EvaluationCoverage,
    ) -> Result<String> {
        self.publish_formula_caches(&evaluation.cache_updates, date_system)?;
        self.revisions.state += 1;
        coverage.revision_id = self.state_revision_id();
        self.calculation = CalculationStamp::Current {
            document_revision: self.revisions.document,
            coverage,
        };
        Ok(self.state_revision_id())
    }

    pub(crate) fn advance_metadata_state(&mut self) -> String {
        self.revisions.state += 1;
        self.state_revision_id()
    }

    pub(crate) fn mark_restart_boundary(&mut self) {
        self.evaluator = None;
        self.revisions.state += 1;
        self.calculation = CalculationStamp::Dirty {
            document_revision: self.revisions.document,
        };
    }

    pub(crate) fn replace_from_bytes(
        &mut self,
        bytes: &[u8],
        effect: ResidentCalculationEffect,
    ) -> Result<String> {
        let replacement = WorkbookSession::from_bytes(bytes)?;
        self.document = replacement;
        self.revisions.document += 1;
        self.revisions.state += 1;
        let new_revision = self.state_revision_id();
        match effect {
            ResidentCalculationEffect::Preserve => match &mut self.calculation {
                CalculationStamp::Current {
                    document_revision,
                    coverage,
                } => {
                    *document_revision = self.revisions.document;
                    coverage.revision_id = new_revision.clone();
                }
                CalculationStamp::NotEvaluated { document_revision }
                | CalculationStamp::Dirty { document_revision } => {
                    *document_revision = self.revisions.document;
                }
            },
            ResidentCalculationEffect::Invalidate => {
                self.evaluator = None;
                self.calculation = CalculationStamp::Dirty {
                    document_revision: self.revisions.document,
                };
            }
        }
        Ok(new_revision)
    }

    /// Apply the common cell/matrix hot path to the authoritative document and
    /// synchronize each committed effect into the retained evaluator. If engine
    /// synchronization fails, the document remains committed and the evaluator
    /// is invalidated, matching the resident transaction contract.
    pub fn apply_write_matrix(
        &mut self,
        sheet_name: impl Into<String>,
        anchor: impl Into<String>,
        rows: Vec<Vec<Option<SessionMatrixCell>>>,
        overwrite_formulas: bool,
    ) -> Result<SessionApplySummary> {
        self.apply_write_matrices(&[crate::core::session::SessionTransformOp::WriteMatrix {
            sheet_name: sheet_name.into(),
            anchor: anchor.into(),
            rows,
            overwrite_formulas,
        }])
    }

    pub(crate) fn preflight_write_matrices(
        &self,
        ops: &[crate::core::session::SessionTransformOp],
    ) -> Result<()> {
        for op in ops {
            let crate::core::session::SessionTransformOp::WriteMatrix {
                sheet_name,
                anchor,
                rows,
                ..
            } = op;
            let (anchor_col, anchor_row) = parse_cell(anchor)?;
            if self.document.sheet_by_name(sheet_name).is_none() {
                return Err(anyhow!("sheet '{}' not found", sheet_name));
            }
            let max_width = rows.iter().map(Vec::len).max().unwrap_or(0) as u32;
            let end_row = anchor_row
                .checked_add(rows.len().saturating_sub(1) as u32)
                .ok_or_else(|| anyhow!("matrix row range overflows"))?;
            let end_col = anchor_col
                .checked_add(max_width.saturating_sub(1))
                .ok_or_else(|| anyhow!("matrix column range overflows"))?;
            if end_row > 1_048_576 || end_col > 16_384 {
                return Err(anyhow!("matrix exceeds XLSX worksheet bounds"));
            }
        }
        Ok(())
    }

    pub(crate) fn apply_write_matrices(
        &mut self,
        ops: &[crate::core::session::SessionTransformOp],
    ) -> Result<SessionApplySummary> {
        // Preflight the complete batch before mutating the authoritative document.
        self.preflight_write_matrices(ops)?;
        let mut skip_sets = Vec::with_capacity(ops.len());
        for op in ops {
            let crate::core::session::SessionTransformOp::WriteMatrix {
                sheet_name,
                anchor,
                rows,
                overwrite_formulas,
            } = op;
            let mut skipped = std::collections::HashSet::new();
            let (anchor_col, anchor_row) = parse_cell(anchor)?;
            if !overwrite_formulas {
                for (row_offset, values) in rows.iter().enumerate() {
                    for (col_offset, value) in values.iter().enumerate() {
                        if value.is_some() {
                            let row = anchor_row + row_offset as u32;
                            let col = anchor_col + col_offset as u32;
                            if self.document.cell_is_formula(sheet_name, col, row)? {
                                skipped.insert((row, col));
                            }
                        }
                    }
                }
            }
            skip_sets.push(skipped);
        }

        let summary = self.document.apply_ops(ops)?;
        let effects = summary.cells_value_set + summary.cells_formula_set;
        if effects == 0 {
            return Ok(summary);
        }

        self.revisions.document += 1;
        self.revisions.state += 1;
        self.calculation = CalculationStamp::Dirty {
            document_revision: self.revisions.document,
        };

        if let Some(evaluator) = self.evaluator.as_mut() {
            for (op, skipped) in ops.iter().zip(&skip_sets) {
                let crate::core::session::SessionTransformOp::WriteMatrix {
                    sheet_name,
                    anchor,
                    rows,
                    ..
                } = op;
                if synchronize_matrix(&self.document, evaluator, sheet_name, anchor, rows, skipped)
                    .is_err()
                {
                    self.evaluator = None;
                    break;
                }
            }
        }
        Ok(summary)
    }

    /// Mark a committed structural/name change as requiring a conservative
    /// evaluator rebuild. The canonical write backend remains the semantic owner.
    pub fn invalidate_after_unsupported_change(&mut self) {
        self.revisions.document += 1;
        self.revisions.state += 1;
        self.calculation = CalculationStamp::Dirty {
            document_revision: self.revisions.document,
        };
        self.evaluator = None;
    }

    /// Evaluate dependencies and publish caches only after complete success.
    /// A failed/partial evaluation publishes neither cache values nor coverage.
    pub fn recalculate(&mut self, timeout_ms: Option<u64>) -> Result<EvaluationCoverage> {
        self.ensure_evaluator()?;
        let evaluator = self.evaluator.as_mut().expect("evaluator ensured");
        self.counters.evaluations += 1;
        let evaluation = match evaluator.evaluate(timeout_ms) {
            Ok(evaluation) => evaluation,
            Err(error) => {
                // Retained partial state cannot authorize a later success.
                self.invalidate_calculation_proof();
                return Err(error);
            }
        };
        // Cache publication is performed against the Umya document itself, not
        // through Formualizer's XLSX exporter. Its complete target set is
        // preflighted before mutation; any failure invalidates derived state and
        // cannot retain a Current stamp.
        let date_system = evaluator.date_system();
        self.publish_formula_caches(&evaluation.cache_updates, date_system)?;
        self.revisions.state += 1;
        let coverage = EvaluationCoverage {
            formula_cells: evaluation.formula_cells,
            evaluated_formula_cells: evaluation.formula_cells,
            unsupported_formula_cells: 0,
            error_formula_cells: evaluation.error_formula_cells,
            source: EvaluationSource::Formualizer,
            freshness: EvaluationFreshness::CurrentRevision,
            revision_id: format!(
                "resident:{}:{}:{}",
                self.revisions.epoch, self.revisions.document, self.revisions.state
            ),
        };
        self.calculation = CalculationStamp::Current {
            document_revision: self.revisions.document,
            coverage: coverage.clone(),
        };
        Ok(coverage)
    }

    /// Export an immutable snapshot through Umya. Export does not edit logical
    /// state or trigger calculation.
    pub fn export_bytes(&mut self) -> Result<Vec<u8>> {
        let bytes = self.snapshot_bytes()?;
        self.last_export = Some(ExportStamp {
            epoch: self.revisions.epoch.clone(),
            document_revision: self.revisions.document,
            state_revision: self.revisions.state,
            content_sha256: hash_bytes_sha256_hex(&bytes),
        });
        Ok(bytes)
    }

    fn publish_formula_caches(
        &mut self,
        updates: &[formualizer::workbook::FormulaCacheUpdate],
        date_system: formualizer::eval::engine::DateSystem,
    ) -> Result<()> {
        if let Err(error) = self
            .document
            .apply_formula_cache_updates(updates, date_system)
        {
            self.invalidate_calculation_proof();
            return Err(error);
        }
        Ok(())
    }

    /// Revoking published proof is a workbook-state transition, but not a
    /// document edit or export. Repeated failure while already dirty is not.
    pub(crate) fn revoke_calculation_proof(&mut self) -> String {
        self.invalidate_calculation_proof();
        self.state_revision_id()
    }

    fn invalidate_calculation_proof(&mut self) {
        self.evaluator = None;
        if !matches!(
            self.calculation,
            CalculationStamp::Dirty { document_revision }
                if document_revision == self.revisions.document
        ) {
            self.revisions.state += 1;
            self.calculation = CalculationStamp::Dirty {
                document_revision: self.revisions.document,
            };
        }
    }

    fn ensure_evaluator(&mut self) -> Result<()> {
        if self.evaluator.is_some() {
            return Ok(());
        }
        let bytes = self.snapshot_bytes()?;
        let mut adapter = UmyaAdapter::open_bytes(bytes)
            .map_err(|error| anyhow!("failed to rebuild evaluator adapter: {error}"))?;
        self.evaluator = Some(RetainedEvaluator::ingest(&mut adapter)?);
        self.counters.constructions += 1;
        self.counters.ingests += 1;
        self.counters.rebuilds += 1;
        Ok(())
    }
}

fn synchronize_matrix(
    document: &WorkbookSession,
    evaluator: &mut RetainedEvaluator,
    sheet: &str,
    anchor: &str,
    rows: &[Vec<Option<SessionMatrixCell>>],
    skipped: &std::collections::HashSet<(u32, u32)>,
) -> Result<()> {
    let (anchor_col, anchor_row) = parse_cell(anchor)?;
    for (row_offset, values) in rows.iter().enumerate() {
        for (col_offset, value) in values.iter().enumerate() {
            let Some(_value) = value else { continue };
            let row = anchor_row + row_offset as u32;
            let col = anchor_col + col_offset as u32;
            if skipped.contains(&(row, col)) {
                continue;
            }
            match document.evaluator_cell(sheet, col, row)? {
                SessionEvaluatorCell::Formula(formula) => {
                    evaluator.set_formula(sheet, row, col, &formula)?
                }
                SessionEvaluatorCell::Value(value) => {
                    evaluator.set_value(sheet, row, col, value)?
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn materialize_umya_cell(cell: &umya_spreadsheet::Cell) -> ResidentMaterializedCell {
    use umya_spreadsheet::CellRawValue;
    let value = match cell.get_cell_value().get_raw_value() {
        CellRawValue::Empty => ResidentMaterializedValue::Empty,
        CellRawValue::String(value) => ResidentMaterializedValue::String(value.to_string()),
        CellRawValue::RichText(value) => {
            ResidentMaterializedValue::RichText(value.get_text().to_string())
        }
        CellRawValue::Lazy(value) => ResidentMaterializedValue::Lazy(value.to_string()),
        CellRawValue::Numeric(value) => ResidentMaterializedValue::Number(*value),
        CellRawValue::Bool(value) => ResidentMaterializedValue::Bool(*value),
        CellRawValue::Error(value) => ResidentMaterializedValue::Error(value.to_string()),
    };
    let formula = cell.get_formula();
    ResidentMaterializedCell {
        value,
        formula: (!formula.is_empty()).then(|| formula.to_string()),
    }
}

fn logical_predecessor_matches(
    current: Option<&ResidentMaterializedCell>,
    expected: Option<&ResidentMaterializedCell>,
) -> bool {
    match (current, expected) {
        (None, None) => true,
        (Some(current), Some(expected)) => match &expected.formula {
            Some(formula) => current.formula.as_ref() == Some(formula),
            None => current == expected,
        },
        _ => false,
    }
}

fn error_value(value: &str) -> Result<umya_spreadsheet::CellErrorType> {
    Ok(match value {
        "#DIV/0!" => umya_spreadsheet::CellErrorType::Div0,
        "#NAME?" => umya_spreadsheet::CellErrorType::Name,
        "#N/A" => umya_spreadsheet::CellErrorType::NA,
        "#NUM!" => umya_spreadsheet::CellErrorType::Num,
        "#VALUE!" => umya_spreadsheet::CellErrorType::Value,
        "#REF!" => umya_spreadsheet::CellErrorType::Ref,
        "#NULL!" => umya_spreadsheet::CellErrorType::Null,
        "#DATA!" => umya_spreadsheet::CellErrorType::Data,
        _ => return Err(anyhow!("unsupported prepared cell error '{value}'")),
    })
}

fn assign_materialized_cell(
    cell: &mut umya_spreadsheet::Cell,
    materialized: &ResidentMaterializedCell,
) -> Result<()> {
    use ResidentMaterializedValue as Value;
    if let Some(formula) = &materialized.formula {
        cell.set_formula(formula);
        match &materialized.value {
            Value::Empty => {
                cell.set_formula_result_blank();
            }
            Value::String(value) | Value::RichText(value) | Value::Lazy(value) => {
                cell.set_formula_result_string(value);
            }
            Value::Number(value) => {
                cell.set_formula_result_number(*value);
            }
            Value::Bool(value) => {
                cell.set_formula_result_bool(*value);
            }
            Value::Error(value) => {
                cell.set_formula_result_error(error_value(value)?);
            }
        }
    } else {
        match &materialized.value {
            Value::Empty => {
                cell.set_value("");
            }
            Value::String(value) | Value::RichText(value) | Value::Lazy(value) => {
                // This is already a normalized literal. Never run type guessing again.
                cell.set_value_string(value);
            }
            Value::Number(value) => {
                cell.set_value_number(*value);
            }
            Value::Bool(value) => {
                cell.set_value_bool(*value);
            }
            Value::Error(value) => {
                cell.set_value(value);
            }
        }
    }
    Ok(())
}

fn parse_cell(cell: &str) -> Result<(u32, u32)> {
    let (col, row, _, _) = umya_spreadsheet::helper::coordinate::index_from_coordinate(cell);
    col.zip(row)
        .ok_or_else(|| anyhow!("invalid cell reference: {cell}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use formualizer::common::LiteralValue;
    use formualizer::workbook::FormulaCacheUpdate;

    fn formula_fixture() -> Vec<u8> {
        let mut book = umya_spreadsheet::new_file();
        let sheet = book.get_sheet_by_name_mut("Sheet1").unwrap();
        sheet.get_cell_mut("A1").set_value_number(1.0);
        sheet.get_cell_mut("B1").set_formula("A1*2");
        let mut bytes = Vec::new();
        umya_spreadsheet::writer::xlsx::write_writer(&book, &mut bytes).unwrap();
        bytes
    }

    fn cached_b1(resident: &ResidentWorkbook) -> String {
        let bytes = resident.document.to_bytes().unwrap();
        let book =
            umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(bytes), true).unwrap();
        book.get_sheet_by_name("Sheet1")
            .unwrap()
            .get_cell("B1")
            .unwrap()
            .get_value()
            .to_string()
    }

    #[test]
    fn evaluation_failure_revokes_proof_without_editing_document_or_export() {
        for initially_current in [false, true] {
            let mut resident = ResidentWorkbook::from_bytes(formula_fixture()).unwrap();
            if initially_current {
                resident.recalculate(None).unwrap();
                resident.export_bytes().unwrap();
            }
            let before = resident.revisions.clone();
            let exported = resident.last_export.clone();
            let cache = cached_b1(&resident);
            resident.evaluator.as_mut().unwrap().invalidate_for_test();
            assert!(resident.recalculate(None).is_err());
            assert_eq!(resident.revisions.epoch, before.epoch);
            assert_eq!(resident.revisions.document, before.document);
            assert_eq!(resident.revisions.state, before.state + 1);
            assert_eq!(resident.last_export, exported);
            assert_eq!(cached_b1(&resident), cache);
            assert!(matches!(
                resident.calculation,
                CalculationStamp::Dirty { .. }
            ));

            // Another failed attempt cannot reuse proof, but it does not
            // invent a second transition from the identical dirty state.
            let dirty_state = resident.revisions.state;
            resident.ensure_evaluator().unwrap();
            resident.evaluator.as_mut().unwrap().invalidate_for_test();
            assert!(resident.recalculate(None).is_err());
            assert_eq!(resident.revisions.state, dirty_state);
            assert_eq!(resident.revisions.document, before.document);
            assert_eq!(resident.last_export, exported);
            assert_eq!(cached_b1(&resident), cache);
            resident.recalculate(None).unwrap();
            assert_eq!(resident.revisions.state, dirty_state + 1);
            assert_eq!(cached_b1(&resident), "2");
        }
    }

    #[test]
    fn cache_publication_preflights_all_targets_before_mutation() {
        let mut resident = ResidentWorkbook::from_bytes(formula_fixture()).unwrap();
        resident.recalculate(None).unwrap();
        assert_eq!(cached_b1(&resident), "2");
        let revisions = resident.revisions.clone();
        let date_system = resident.evaluator.as_ref().unwrap().date_system();
        let updates = vec![
            FormulaCacheUpdate {
                sheet: "Sheet1".into(),
                row: 1,
                col: 2,
                value: LiteralValue::Number(999.0),
            },
            FormulaCacheUpdate {
                sheet: "Missing".into(),
                row: 1,
                col: 1,
                value: LiteralValue::Number(1.0),
            },
        ];

        assert!(
            resident
                .publish_formula_caches(&updates, date_system)
                .is_err()
        );
        assert_eq!(cached_b1(&resident), "2");
        assert_eq!(resident.revisions.epoch, revisions.epoch);
        assert_eq!(resident.revisions.document, revisions.document);
        assert_eq!(resident.revisions.state, revisions.state + 1);
        let dirty_state = resident.revisions.state;
        assert!(
            resident
                .publish_formula_caches(&updates, date_system)
                .is_err()
        );
        assert_eq!(resident.revisions.state, dirty_state);
        assert_eq!(cached_b1(&resident), "2");
        assert!(resident.evaluator.is_none());
        assert!(matches!(
            resident.calculation,
            CalculationStamp::Dirty { .. }
        ));
    }
}
