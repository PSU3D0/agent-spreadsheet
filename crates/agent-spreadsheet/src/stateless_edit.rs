//! Atomic file-in/edit/optional-source-recalc/file-out, without resident ownership.
use crate::canonical_write::{
    WriteMode, WriteOp, WriteOpResult, WriteOpStatus, WriteRequest, WriteResponseData,
    calculation_effect_for_ops, execute_stateless_write_on_bytes,
};
use crate::model::FormulaParsePolicy;
use crate::operations::ResourceId;
use crate::recalc::{
    ResidentCalculationEffect,
    source_xlsx::{
        IoError, XlsxRecalculateOptions, XlsxRecalculateResult, recalculate_source_xlsx_bytes,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EditRecalculate {
    #[default]
    Auto,
    Never,
    Always,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatelessEditRequest {
    pub ops: Vec<WriteOp>,
    #[serde(default)]
    pub recalculate: EditRecalculate,
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
    /// Computes the canonical diff summary and returns all existing per-op typed facts.
    #[serde(default)]
    pub details: bool,
}

#[derive(Debug, Serialize)]
pub struct EditCalculationReceipt {
    pub requested: EditRecalculate,
    pub performed: bool,
    /// Auto is a batch-category heuristic; it never proves incoming cache freshness.
    pub reason: &'static str,
    pub recommendation: &'static str,
    pub cache_state: &'static str,
    pub attempted: bool,
    pub completed: bool,
    pub artifact_available: bool,
}

#[derive(Debug, Serialize)]
pub struct StatelessEditReceipt {
    pub ops_applied: usize,
    pub op_kinds: Vec<String>,
    pub change_count: Option<usize>,
    pub diff_exact: bool,
    pub calculation: EditCalculationReceipt,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<WriteOpResult>>,
}

#[derive(Debug)]
pub enum EditedArtifact {
    Document(Vec<u8>),
    Recalculated(XlsxRecalculateResult),
    Deferred {
        bytes: Vec<u8>,
        reason: crate::recalc::source_xlsx::CalculationDeferredReason,
        context: String,
    },
}

#[derive(Debug)]
pub struct StatelessEditResult {
    pub artifact: EditedArtifact,
    pub receipt: StatelessEditReceipt,
}
impl StatelessEditResult {
    pub fn bytes(&self) -> &[u8] {
        match &self.artifact {
            EditedArtifact::Document(bytes) | EditedArtifact::Deferred { bytes, .. } => bytes,
            EditedArtifact::Recalculated(result) => &result.bytes,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StatelessEditError {
    #[error("invalid edit request: {0}")]
    InvalidRequest(String),
    #[error("edit operation {index} ({kind}) failed: {message}")]
    Operation {
        index: usize,
        kind: String,
        code: String,
        message: String,
    },
    #[error(transparent)]
    Recalculate(#[from] IoError),
}

/// Apply existing canonical ops atomically in a fresh rich document, optionally
/// repairing its caches with the selected source profile. No file publication or CAS.
///
/// Rich editing may rewrite/drop unsupported original content. Strict package
/// preservation applies only to subsequent recalculation of the edited bytes.
pub fn edit_xlsx_bytes(
    input: &[u8],
    request: StatelessEditRequest,
    recalc_options: XlsxRecalculateOptions,
) -> Result<StatelessEditResult, StatelessEditError> {
    if let Some(policy) = &recalc_options.calculation {
        policy.validate()?;
    }
    let allow_deferred = recalc_options.profile
        == crate::recalc::source_xlsx::XlsxRecalculateProfile::Workbook
        && !recalc_options.require_complete;
    let output_limit = recalc_options.limits.max_output_bytes;
    let result = edit_with_recalculator_policy(input, request, allow_deferred, |edited| {
        recalculate_source_xlsx_bytes(edited, recalc_options)
    })?;
    if result.bytes().len() > output_limit {
        return Err(IoError::LoadBudgetExceeded {
            backend: "xlsx".into(),
            sheet: String::new(),
            message: "edited output exceeds maxOutputBytes".into(),
        }
        .into());
    }
    Ok(result)
}

#[cfg(test)]
fn edit_with_recalculator(
    input: &[u8],
    request: StatelessEditRequest,
    recalculate: impl FnOnce(&[u8]) -> Result<XlsxRecalculateResult, IoError>,
) -> Result<StatelessEditResult, StatelessEditError> {
    edit_with_recalculator_policy(input, request, false, recalculate)
}

fn edit_with_recalculator_policy(
    input: &[u8],
    request: StatelessEditRequest,
    allow_deferred: bool,
    recalculate: impl FnOnce(&[u8]) -> Result<XlsxRecalculateResult, IoError>,
) -> Result<StatelessEditResult, StatelessEditError> {
    let preserving = matches!(
        calculation_effect_for_ops(request.ops.iter()),
        ResidentCalculationEffect::Preserve
    );
    let selected = request.recalculate == EditRecalculate::Always
        || (request.recalculate == EditRecalculate::Auto && !preserving);
    let reason = match request.recalculate {
        EditRecalculate::Always => "always_requested",
        EditRecalculate::Never => "never_requested",
        EditRecalculate::Auto if preserving => "auto_preserving_batch",
        EditRecalculate::Auto => "auto_invalidating_batch",
    };
    // ByteSessionBackend compatibility is private and derived from this input.
    // These handles never escape and are not an addressable session service.
    let digest = crate::utils::hash_bytes_sha256_hex(input);
    let revision = format!("state:{digest}");
    let resource_id: ResourceId =
        serde_json::from_value(serde_json::json!(format!("session:stateless-{digest}")))
            .map_err(|error| StatelessEditError::InvalidRequest(error.to_string()))?;
    let canonical = WriteRequest {
        resource_id,
        expected_revision: revision.clone(),
        mode: WriteMode::Apply,
        atomic: true,
        ops: request.ops,
        formula_parse_policy: request.formula_parse_policy,
        label: None,
    };
    let (response, committed) =
        execute_stateless_write_on_bytes(input, &revision, canonical, request.details)
            .map_err(|error| StatelessEditError::InvalidRequest(error.to_string()))?;
    let (ops_applied, diff, impact, results) = match response {
        WriteResponseData::Applied {
            ops_applied,
            diff,
            impact,
            results,
            ..
        } => (ops_applied, diff, impact, results),
        WriteResponseData::Failed { results, .. }
        | WriteResponseData::RolledBack { results, .. } => {
            let failed = results
                .iter()
                .find(|result| matches!(result.status, WriteOpStatus::Failed));
            return Err(match failed {
                Some(result) => StatelessEditError::Operation {
                    index: result.index,
                    kind: result.kind.clone(),
                    code: result
                        .error
                        .as_ref()
                        .map(|error| error.code.clone())
                        .unwrap_or_else(|| "EDIT_FAILED".into()),
                    message: result
                        .error
                        .as_ref()
                        .map(|error| error.message.clone())
                        .unwrap_or_else(|| "atomic batch failed".into()),
                },
                None => StatelessEditError::InvalidRequest("atomic batch was not applied".into()),
            });
        }
        _ => {
            return Err(StatelessEditError::InvalidRequest(
                "unexpected non-atomic write outcome".into(),
            ));
        }
    };
    let edited = committed.unwrap_or_else(|| input.to_vec());
    let artifact = if selected {
        match recalculate(&edited) {
            Ok(result) => EditedArtifact::Recalculated(result),
            Err(IoError::CalculationDeferred { reason, context }) if allow_deferred => {
                EditedArtifact::Deferred {
                    bytes: edited,
                    reason,
                    context,
                }
            }
            Err(error) => return Err(error.into()),
        }
    } else {
        EditedArtifact::Document(edited)
    };
    let partial = matches!(&artifact, EditedArtifact::Recalculated(result)
        if result.coverage == crate::recalc::source_xlsx::SourceCalculationCoverage::Partial);
    let completed = matches!(&artifact, EditedArtifact::Recalculated(_));
    let deferred = matches!(&artifact, EditedArtifact::Deferred { .. });
    Ok(StatelessEditResult {
        artifact,
        receipt: StatelessEditReceipt {
            ops_applied,
            op_kinds: impact.op_kinds,
            change_count: request.details.then_some(diff.change_count),
            diff_exact: request.details && diff.exact,
            calculation: EditCalculationReceipt {
                requested: request.recalculate,
                performed: completed,
                attempted: selected,
                completed,
                artifact_available: true,
                reason,
                recommendation: if preserving { "unknown" } else { "recalculate" },
                cache_state: if partial {
                    "partial"
                } else if completed {
                    "recalculated"
                } else if deferred {
                    "deferred"
                } else {
                    "unknown"
                },
            },
            details: request.details.then_some(results),
        },
    })
}

#[cfg(test)]
pub(crate) mod construction_guard {
    use std::cell::Cell;
    thread_local! { static FORBIDDEN: Cell<bool> = const { Cell::new(false) }; }
    pub(crate) struct Guard(bool);
    impl Drop for Guard {
        fn drop(&mut self) {
            FORBIDDEN.with(|flag| flag.set(self.0));
        }
    }
    pub(crate) fn forbid() -> Guard {
        Guard(FORBIDDEN.with(|flag| flag.replace(true)))
    }
    pub(crate) fn assert_allowed() {
        FORBIDDEN.with(|flag| {
            assert!(
                !flag.get(),
                "resident/evaluator construction forbidden in document-only test"
            )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<u8> {
        let mut book = umya_spreadsheet::new_file();
        book.sheet_by_name_mut("Sheet1")
            .unwrap()
            .cell_mut("B1")
            .set_formula("A1*2");
        let mut bytes = Vec::new();
        umya_spreadsheet::writer::xlsx::write_writer(&book, &mut bytes).unwrap();
        bytes
    }
    #[test]
    fn never_and_auto_presentation_edits_forbid_known_engine_paths() {
        for (mode, ops) in [
            (
                "never",
                serde_json::json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":30}}}]),
            ),
            (
                "auto",
                serde_json::json!([{"kind":"style","sheet_name":"Sheet1","target":{"kind":"range","range":"A1"},"patch":{"font":{"bold":true}}},{"kind":"column_size","sheet_name":"Sheet1","target":{"kind":"columns","range":"A:A"},"size":{"kind":"width","width_chars":16.0}}]),
            ),
        ] {
            let _guard = construction_guard::forbid();
            let _diff_guard = crate::canonical_write::diff_test_guard::forbid();
            let request =
                serde_json::from_value(serde_json::json!({"ops":ops,"recalculate":mode})).unwrap();
            let output = edit_with_recalculator(&fixture(), request, |_| {
                panic!("strict recalculator must not run")
            })
            .unwrap();
            assert!(output.receipt.change_count.is_none());
            assert!(!output.receipt.diff_exact);
            assert!(!output.receipt.calculation.performed);
            assert_eq!(output.receipt.calculation.cache_state, "unknown");
        }
    }
    #[test]
    fn never_keeps_formula_name_import_rules_and_structure_document_only() {
        let request = serde_json::from_value(serde_json::json!({
            "recalculate":"never", "ops":[
                {"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":30},"C1":{"kind":"formula","formula":"A1+1"}}},
                {"kind":"define_name","name":"Rate","refers_to":"Sheet1!$A$1","scope":"workbook"},
                {"kind":"import_csv","sheet_name":"Sheet1","anchor":"D1","csv":"header\n2\n"},
                {"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"A1:A1","validation":{"kind":"list","formula1":"\"a,b\""}},
                {"kind":"insert_rows","sheet_name":"Sheet1","at_row":2,"count":1}
            ]
        })).unwrap();
        let _guard = construction_guard::forbid();
        let _diff_guard = crate::canonical_write::diff_test_guard::forbid();
        let output = edit_with_recalculator(&fixture(), request, |_| {
            panic!("no calculation in never mode")
        })
        .unwrap();
        assert_eq!(output.receipt.ops_applied, 5);
        assert!(!output.receipt.calculation.performed);
    }

    #[test]
    fn diff_guard_restores_on_unwind_and_explicit_details_use_full_diff() {
        assert!(std::panic::catch_unwind(|| {
            let _guard = crate::canonical_write::diff_test_guard::forbid();
            let request = serde_json::from_value(serde_json::json!({
                "ops":[{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":30}}}],
                "recalculate":"never","details":true
            })).unwrap();
            edit_with_recalculator(&fixture(), request, |_| panic!("no calculation")).unwrap();
        }).is_err());
        crate::canonical_write::diff_test_guard::assert_allowed();
        construction_guard::assert_allowed();
    }

    #[test]
    fn only_typed_deferred_can_preserve_edited_bytes() {
        use crate::recalc::source_xlsx::CalculationDeferredReason;
        for allowed in [false, true] {
            let request = serde_json::from_value(serde_json::json!({
                "recalculate":"always","ops":[{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":30}}}]
            })).unwrap();
            let result = edit_with_recalculator_policy(&fixture(), request, allowed, |_| {
                Err(IoError::CalculationDeferred {
                    reason: CalculationDeferredReason::IterativeInitialStateUnverified,
                    context: "test cycle".into(),
                })
            });
            if allowed {
                let result = result.unwrap();
                assert!(matches!(result.artifact, EditedArtifact::Deferred { .. }));
                assert!(result.receipt.calculation.attempted);
                assert!(!result.receipt.calculation.completed);
                assert!(!result.receipt.calculation.performed);
                assert_eq!(result.receipt.calculation.cache_state, "deferred");
                let book = umya_spreadsheet::reader::xlsx::read_reader(
                    std::io::Cursor::new(result.bytes()),
                    true,
                )
                .unwrap();
                assert_eq!(
                    book.sheet_by_name("Sheet1")
                        .unwrap()
                        .cell("A1")
                        .unwrap()
                        .value(),
                    "30"
                );
            } else {
                assert!(result.is_err());
            }
        }
        for error in [
            IoError::InvalidOptions {
                field: "calculation".into(),
                message: "bad option".into(),
            },
            IoError::Backend {
                backend: "xlsx".into(),
                message: "corrupt package".into(),
            },
            IoError::Unsupported {
                feature: "output failure".into(),
                context: "refused".into(),
            },
            IoError::LoadBudgetExceeded {
                backend: "xlsx".into(),
                sheet: String::new(),
                message: "resource limit".into(),
            },
            IoError::Engine(formualizer::common::ExcelError::new(
                formualizer::common::ExcelErrorKind::Cancelled,
            )),
            IoError::Io(std::io::Error::other("output write failed")),
        ] {
            let request = serde_json::from_value(serde_json::json!({
                "recalculate":"always","ops":[{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":30}}}]
            })).unwrap();
            assert!(
                edit_with_recalculator_policy(&fixture(), request, true, |_| Err(error)).is_err()
            );
        }
    }

    #[test]
    fn skipped_edits_validate_policy_without_constructing_engine() {
        let _guard = construction_guard::forbid();
        let mut options = XlsxRecalculateOptions::workbook();
        options.calculation.as_mut().unwrap().max_elapsed_ms = Some(0);
        for mode in ["never", "auto"] {
            let request = serde_json::from_value(serde_json::json!({
                "recalculate":mode,"ops":[{"kind":"style","sheet_name":"Sheet1","target":{"kind":"range","range":"A1"},"patch":{"font":{"bold":true}}}]
            })).unwrap();
            assert!(matches!(
                edit_xlsx_bytes(&fixture(), request, options.clone()),
                Err(StatelessEditError::Recalculate(
                    IoError::InvalidOptions { .. }
                ))
            ));
        }
    }

    #[test]
    fn constructor_guard_restores_on_unwind() {
        assert!(
            std::panic::catch_unwind(|| {
                let _guard = construction_guard::forbid();
                construction_guard::assert_allowed();
            })
            .is_err()
        );
        construction_guard::assert_allowed();
    }
}
