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

/// One portable resident workbook. It is deliberately not `Clone`, wrapped in
/// a global mutex, or marked Send/Sync: an adapter chooses an appropriate owner
/// lane and serializes commits around it.
pub struct ResidentWorkbook {
    document: WorkbookSession,
    evaluator: Option<RetainedEvaluator>,
    revisions: ResidentRevision,
    calculation: CalculationStamp,
    counters: EvaluatorCounters,
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
        let sheet_name = sheet_name.into();
        let anchor = anchor.into();
        // Record formula-preservation decisions before the authoritative mutation;
        // this is effect metadata, not a document copy.
        let (anchor_col, anchor_row) = parse_cell(&anchor)?;
        let mut skipped = std::collections::HashSet::new();
        if !overwrite_formulas {
            for (row_offset, values) in rows.iter().enumerate() {
                for (col_offset, value) in values.iter().enumerate() {
                    if value.is_some() {
                        let row = anchor_row + row_offset as u32;
                        let col = anchor_col + col_offset as u32;
                        if self.document.cell_is_formula(&sheet_name, col, row)? {
                            skipped.insert((row, col));
                        }
                    }
                }
            }
        }
        // WorkbookSession validates the complete operation before mutation.
        let summary = self.document.apply_write_matrix(
            sheet_name.clone(),
            anchor.clone(),
            rows.clone(),
            overwrite_formulas,
        )?;
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
            let sync = synchronize_matrix(
                &self.document,
                evaluator,
                &sheet_name,
                &anchor,
                &rows,
                &skipped,
            );
            if sync.is_err() {
                self.evaluator = None;
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
        let bytes = self.document.to_bytes()?;
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
        let bytes = self.document.to_bytes()?;
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
