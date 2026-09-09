use crate::model::{
    CellValue, EvaluationCoverage, EvaluationState, NamedItemKind, NamedRangeDescriptor,
    NamedRangeScope,
};
use crate::workbook::{WorkbookContext, cell_to_value, is_spreadsheet_error};
use anyhow::{Result, anyhow, bail};
use schemars::JsonSchema;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, JsonSchema, Default)]
pub struct TargetClassificationCounts {
    pub unchanged: u32,
    pub direct_edit: u32,
    pub recalc_result: u32,
    pub formula_shift: u32,
    pub new_error: u32,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct VerifySummary {
    pub target_count: u32,
    pub changed_targets: u32,
    pub new_error_count: u32,
    pub resolved_error_count: u32,
    pub preexisting_error_count: u32,
    pub named_range_delta_count: u32,
    pub target_classification_counts: TargetClassificationCounts,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TargetDelta {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<CellValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<CellValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_formula: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_formula: Option<String>,
    pub classification: String,
    pub changed: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ErrorDelta {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_formula: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_formula: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct NamedRangeDelta {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_kind: Option<NamedRangeScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_sheet_name: Option<String>,
    pub change: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_refers_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_refers_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_kind: Option<NamedItemKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_kind: Option<NamedItemKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProofStatus {
    Proved,
    DifferencesFound,
    InconclusiveUnevaluated,
    Failed,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct VerifyResponse {
    pub baseline: String,
    pub current: String,
    pub proof_status: ProofStatus,
    pub baseline_state: EvaluationState,
    pub current_state: EvaluationState,
    pub baseline_evaluation_coverage: EvaluationCoverage,
    pub current_evaluation_coverage: EvaluationCoverage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    pub target_deltas: Vec<TargetDelta>,
    pub new_errors: Vec<ErrorDelta>,
    pub resolved_errors: Vec<ErrorDelta>,
    pub preexisting_errors: Vec<ErrorDelta>,
    pub named_range_deltas: Vec<NamedRangeDelta>,
    pub summary: VerifySummary,
}

#[derive(Debug, Clone, Default)]
pub struct VerifyOptions {
    pub targets: Vec<String>,
    pub sheet_filter: Option<String>,
    pub include_named_range_deltas: bool,
    pub errors_only: bool,
    pub targets_only: bool,
}

#[derive(Debug, Clone)]
struct TargetCellSnapshot {
    value: Option<CellValue>,
    formula: Option<String>,
}

#[derive(Debug, Clone)]
struct ErrorCellSnapshot {
    error: String,
    formula: Option<String>,
}

pub struct EvaluatedWorkbook {
    _temp_dir: crate::hostfs::TempDir,
    pub workbook: WorkbookContext,
    pub coverage: EvaluationCoverage,
}

pub struct EvaluatedWorkbookPair {
    baseline: EvaluatedWorkbook,
    current: Option<EvaluatedWorkbook>,
}

impl EvaluatedWorkbookPair {
    pub fn baseline(&self) -> &EvaluatedWorkbook {
        &self.baseline
    }

    pub fn current(&self) -> &EvaluatedWorkbook {
        self.current.as_ref().unwrap_or(&self.baseline)
    }

    pub fn evaluations_performed(&self) -> u8 {
        if self.current.is_some() { 2 } else { 1 }
    }
}

pub fn can_reuse_verification_evaluation(
    baseline: &WorkbookContext,
    current: &WorkbookContext,
) -> bool {
    baseline.id == current.id && baseline.revision_id == current.revision_id
}

pub async fn evaluate_workbook_pair_for_verification(
    baseline_config: &std::sync::Arc<crate::config::ServerConfig>,
    baseline: &WorkbookContext,
    current_config: &std::sync::Arc<crate::config::ServerConfig>,
    current: &WorkbookContext,
) -> Result<EvaluatedWorkbookPair> {
    if can_reuse_verification_evaluation(baseline, current) {
        return Ok(EvaluatedWorkbookPair {
            baseline: evaluate_workbook_for_verification(baseline_config, baseline).await?,
            current: None,
        });
    }

    let (baseline, current) = tokio::try_join!(
        evaluate_workbook_for_verification(baseline_config, baseline),
        evaluate_workbook_for_verification(current_config, current),
    )?;
    Ok(EvaluatedWorkbookPair {
        baseline,
        current: Some(current),
    })
}

#[cfg(feature = "recalc-formualizer")]
pub async fn evaluate_workbook_for_verification(
    config: &std::sync::Arc<crate::config::ServerConfig>,
    workbook: &WorkbookContext,
) -> Result<EvaluatedWorkbook> {
    use crate::recalc::{FormualizerBackend, RecalcBackend};

    let temp_dir = crate::hostfs::tempdir()?;
    let path = temp_dir.path().join("verification.xlsx");
    workbook.save_copy(&path)?;

    let result = FormualizerBackend.recalculate(&path, None).await?;
    let mut coverage = result.evaluation_coverage;
    coverage.revision_id = workbook.revision_id.clone();
    let evaluated = WorkbookContext::load_from_path(
        config,
        &path,
        workbook.id.clone(),
        workbook.short_id.clone(),
        Some(workbook.revision_id.clone()),
    )?;

    Ok(EvaluatedWorkbook {
        _temp_dir: temp_dir,
        workbook: evaluated,
        coverage,
    })
}

#[cfg(not(feature = "recalc-formualizer"))]
pub async fn evaluate_workbook_for_verification(
    _config: &std::sync::Arc<crate::config::ServerConfig>,
    _workbook: &WorkbookContext,
) -> Result<EvaluatedWorkbook> {
    bail!("verification requires the recalc-formualizer feature")
}

#[cfg(feature = "recalc-formualizer")]
pub fn verify_workbook_bytes(
    baseline_label: &str,
    baseline_bytes: &[u8],
    baseline_revision: &str,
    current_label: &str,
    current_bytes: &[u8],
    current_revision: &str,
    options: &VerifyOptions,
) -> Result<VerifyResponse> {
    let (baseline_result, baseline_evaluated) =
        crate::recalc::recalculate_bytes_sync(baseline_bytes, None)?;
    let (current_result, current_evaluated) =
        crate::recalc::recalculate_bytes_sync(current_bytes, None)?;
    let config = std::sync::Arc::new(crate::config::ServerConfig {
        workspace_root: std::path::PathBuf::new(),
        screenshot_dir: std::path::PathBuf::new(),
        path_mappings: Vec::new(),
        cache_capacity: 2,
        supported_extensions: vec!["xlsx".to_string(), "xlsm".to_string()],
        single_workbook: None,
        enabled_tools: None,
        transport: crate::config::TransportKind::Stdio,
        http_bind_address: "127.0.0.1:0".parse().expect("static address"),
        recalc_enabled: true,
        recalc_backend: crate::config::RecalcBackendKind::Formualizer,
        vba_enabled: false,
        max_concurrent_recalcs: 1,
        tool_timeout_ms: None,
        max_response_bytes: None,
        output_profile: crate::config::OutputProfile::TokenDense,
        max_payload_bytes: None,
        max_cells: None,
        max_items: None,
        allow_overwrite: false,
        slim_surface: true,
    });
    let baseline = WorkbookContext::load_from_bytes(
        &config,
        "baseline.xlsx",
        &baseline_evaluated,
        crate::model::WorkbookId("baseline".to_string()),
        "baseline".to_string(),
        Some(baseline_revision.to_string()),
    )?;
    let current = WorkbookContext::load_from_bytes(
        &config,
        "current.xlsx",
        &current_evaluated,
        crate::model::WorkbookId("current".to_string()),
        "current".to_string(),
        Some(current_revision.to_string()),
    )?;
    let baseline_named = options
        .include_named_range_deltas
        .then(|| baseline.named_items())
        .transpose()?;
    let current_named = options
        .include_named_range_deltas
        .then(|| current.named_items())
        .transpose()?;
    let mut baseline_coverage = baseline_result.evaluation_coverage;
    baseline_coverage.revision_id = baseline_revision.to_string();
    let mut current_coverage = current_result.evaluation_coverage;
    current_coverage.revision_id = current_revision.to_string();
    compare_workbooks_with_coverage(
        baseline_label,
        current_label,
        &baseline,
        &current,
        options,
        baseline_named.as_deref(),
        current_named.as_deref(),
        baseline_coverage,
        current_coverage,
    )
}

impl VerifyOptions {
    pub fn validate(&self) -> Result<()> {
        if self.errors_only && self.targets_only {
            bail!("invalid argument: --errors-only and --targets-only cannot be combined");
        }
        if self.errors_only && !self.targets.is_empty() {
            bail!(
                "invalid argument: --errors-only cannot be combined with explicit --targets; drop --targets or use default verify mode"
            );
        }
        if self.errors_only && self.include_named_range_deltas {
            bail!(
                "invalid argument: --errors-only cannot be combined with --named-ranges; use default verify mode to include named-range deltas"
            );
        }
        if self.targets_only && self.targets.is_empty() {
            bail!(
                "invalid argument: --targets-only requires --targets Sheet!A1,... to define the proof scope"
            );
        }
        if self.targets_only && self.include_named_range_deltas {
            bail!(
                "invalid argument: --targets-only cannot be combined with --named-ranges; use default verify mode to include named-range deltas"
            );
        }
        Ok(())
    }
}

pub fn compare_workbooks(
    baseline_label: impl Into<String>,
    current_label: impl Into<String>,
    baseline: &WorkbookContext,
    current: &WorkbookContext,
    options: &VerifyOptions,
    baseline_named_ranges: Option<&[NamedRangeDescriptor]>,
    current_named_ranges: Option<&[NamedRangeDescriptor]>,
) -> Result<VerifyResponse> {
    let baseline_coverage = baseline.imported_evaluation_coverage();
    let current_coverage = current.imported_evaluation_coverage();
    compare_workbooks_with_coverage(
        baseline_label,
        current_label,
        baseline,
        current,
        options,
        baseline_named_ranges,
        current_named_ranges,
        baseline_coverage,
        current_coverage,
    )
}

#[allow(clippy::too_many_arguments)] // Public parity API keeps both sides explicit.
pub fn compare_workbooks_with_coverage(
    baseline_label: impl Into<String>,
    current_label: impl Into<String>,
    baseline: &WorkbookContext,
    current: &WorkbookContext,
    options: &VerifyOptions,
    baseline_named_ranges: Option<&[NamedRangeDescriptor]>,
    current_named_ranges: Option<&[NamedRangeDescriptor]>,
    baseline_evaluation_coverage: EvaluationCoverage,
    current_evaluation_coverage: EvaluationCoverage,
) -> Result<VerifyResponse> {
    options.validate()?;

    let target_deltas = if options.errors_only {
        Vec::new()
    } else {
        collect_target_deltas(baseline, current, options.targets.clone())?
    };

    let (new_errors, resolved_errors, preexisting_errors) = if options.targets_only {
        (Vec::new(), Vec::new(), Vec::new())
    } else {
        let baseline_errors = collect_error_cells(baseline, options.sheet_filter.as_deref())?;
        let current_errors = collect_error_cells(current, options.sheet_filter.as_deref())?;
        compare_error_maps(&baseline_errors, &current_errors)
    };

    let named_range_deltas = if options.include_named_range_deltas {
        let baseline_named_ranges = baseline_named_ranges.ok_or_else(|| {
            anyhow!("internal error: baseline named ranges were not loaded for verification")
        })?;
        let current_named_ranges = current_named_ranges.ok_or_else(|| {
            anyhow!("internal error: current named ranges were not loaded for verification")
        })?;
        compare_named_ranges(baseline_named_ranges, current_named_ranges)
    } else {
        Vec::new()
    };

    let target_classification_counts = count_target_classifications(&target_deltas);
    let baseline_state = baseline_evaluation_coverage.state();
    let current_state = current_evaluation_coverage.state();
    let complete_and_fresh = baseline_evaluation_coverage.is_complete_and_fresh()
        && current_evaluation_coverage.is_complete_and_fresh();
    let changed_preexisting_error = preexisting_errors.iter().any(|delta| {
        delta.before_error != delta.after_error || delta.before_formula != delta.after_formula
    });
    let has_differences = target_deltas.iter().any(|delta| delta.changed)
        || !new_errors.is_empty()
        || !resolved_errors.is_empty()
        || changed_preexisting_error
        || !named_range_deltas.is_empty();
    let proof_status = if !complete_and_fresh {
        ProofStatus::InconclusiveUnevaluated
    } else if has_differences {
        ProofStatus::DifferencesFound
    } else {
        ProofStatus::Proved
    };

    Ok(VerifyResponse {
        baseline: baseline_label.into(),
        current: current_label.into(),
        proof_status,
        baseline_state,
        current_state,
        baseline_evaluation_coverage,
        current_evaluation_coverage,
        failure: None,
        summary: VerifySummary {
            target_count: target_deltas.len() as u32,
            changed_targets: target_deltas.iter().filter(|d| d.changed).count() as u32,
            new_error_count: new_errors.len() as u32,
            resolved_error_count: resolved_errors.len() as u32,
            preexisting_error_count: preexisting_errors.len() as u32,
            named_range_delta_count: named_range_deltas.len() as u32,
            target_classification_counts,
        },
        target_deltas,
        new_errors,
        resolved_errors,
        preexisting_errors,
        named_range_deltas,
    })
}

pub fn failed_verification_response(
    baseline_label: impl Into<String>,
    current_label: impl Into<String>,
    baseline: &WorkbookContext,
    current: &WorkbookContext,
    failure: impl Into<String>,
) -> VerifyResponse {
    let baseline_evaluation_coverage = baseline.imported_evaluation_coverage();
    let current_evaluation_coverage = current.imported_evaluation_coverage();
    VerifyResponse {
        baseline: baseline_label.into(),
        current: current_label.into(),
        proof_status: ProofStatus::Failed,
        baseline_state: baseline_evaluation_coverage.state(),
        current_state: current_evaluation_coverage.state(),
        baseline_evaluation_coverage,
        current_evaluation_coverage,
        failure: Some(failure.into()),
        target_deltas: Vec::new(),
        new_errors: Vec::new(),
        resolved_errors: Vec::new(),
        preexisting_errors: Vec::new(),
        named_range_deltas: Vec::new(),
        summary: VerifySummary {
            target_count: 0,
            changed_targets: 0,
            new_error_count: 0,
            resolved_error_count: 0,
            preexisting_error_count: 0,
            named_range_delta_count: 0,
            target_classification_counts: TargetClassificationCounts::default(),
        },
    }
}

fn count_target_classifications(target_deltas: &[TargetDelta]) -> TargetClassificationCounts {
    let mut counts = TargetClassificationCounts::default();
    for delta in target_deltas {
        match delta.classification.as_str() {
            "unchanged" => counts.unchanged += 1,
            "direct_edit" => counts.direct_edit += 1,
            "recalc_result" => counts.recalc_result += 1,
            "formula_shift" => counts.formula_shift += 1,
            "new_error" => counts.new_error += 1,
            _ => {}
        }
    }
    counts
}

fn collect_target_deltas(
    baseline: &WorkbookContext,
    current: &WorkbookContext,
    targets: Vec<String>,
) -> Result<Vec<TargetDelta>> {
    let mut deltas = Vec::new();
    for target in targets {
        let (sheet_name, cell_ref) = parse_sheet_cell_ref(&target)?;
        let before = read_target_cell(baseline, &sheet_name, &cell_ref)?;
        let after = read_target_cell(current, &sheet_name, &cell_ref)?;
        let changed = !cell_values_equal(before.value.as_ref(), after.value.as_ref())
            || before.formula != after.formula;
        let classification = classify_target_delta(&before, &after, changed).to_string();
        deltas.push(TargetDelta {
            address: target,
            before: before.value,
            after: after.value,
            before_formula: before.formula,
            after_formula: after.formula,
            classification,
            changed,
        });
    }
    Ok(deltas)
}

fn parse_sheet_cell_ref(raw: &str) -> Result<(String, String)> {
    let (sheet_name, cell_ref) = raw.rsplit_once('!').ok_or_else(|| {
        anyhow!(
            "invalid argument: target '{}' must use Sheet!A1 notation",
            raw
        )
    })?;
    if sheet_name.trim().is_empty() || cell_ref.trim().is_empty() {
        bail!(
            "invalid argument: target '{}' must use Sheet!A1 notation",
            raw
        );
    }

    let sheet_name = extract_sheet_name(sheet_name);
    let cell_ref = parse_target_cell_ref(raw, cell_ref)?;
    Ok((sheet_name, cell_ref))
}

fn extract_sheet_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(stripped) = trimmed.strip_prefix('\'')
        && let Some(inner) = stripped.strip_suffix('\'')
    {
        return inner.replace("''", "'");
    }
    trimmed.to_string()
}

fn parse_target_cell_ref(target: &str, raw_cell_ref: &str) -> Result<String> {
    let cell_ref = raw_cell_ref.trim();
    let (col, row, _, _) = umya_spreadsheet::helper::coordinate::index_from_coordinate(cell_ref);
    match (col, row) {
        (Some(c), Some(r)) if c > 0 && r > 0 => Ok(cell_ref.to_string()),
        _ => bail!(
            "invalid argument: target '{}' must use Sheet!A1 notation with a single A1 cell reference",
            target
        ),
    }
}

fn read_target_cell(
    workbook: &WorkbookContext,
    sheet_name: &str,
    cell_ref: &str,
) -> Result<TargetCellSnapshot> {
    workbook.with_sheet(sheet_name, |sheet| {
        if let Some(cell) = sheet.get_cell(cell_ref) {
            let formula = non_empty_formula(cell.get_formula());
            TargetCellSnapshot {
                value: cell_to_value(cell),
                formula,
            }
        } else {
            TargetCellSnapshot {
                value: None,
                formula: None,
            }
        }
    })
}

fn collect_error_cells(
    workbook: &WorkbookContext,
    sheet_filter: Option<&str>,
) -> Result<BTreeMap<String, ErrorCellSnapshot>> {
    let mut out = BTreeMap::new();
    let sheet_names = if let Some(sheet_name) = sheet_filter {
        vec![sheet_name.to_string()]
    } else {
        workbook.sheet_names()
    };

    for sheet_name in sheet_names {
        let sheet_errors = workbook.with_sheet(&sheet_name, |sheet| {
            let mut items = Vec::new();
            for cell in sheet.cells() {
                let raw = cell.get_value();
                let is_typed_error =
                    cell.get_data_type() == "e" || (cell.is_formula() && is_error_text(&raw));
                if !is_typed_error {
                    continue;
                }
                let address = format!("{}!{}", sheet_name, cell.get_coordinate().get_coordinate());
                items.push((
                    address,
                    ErrorCellSnapshot {
                        error: raw.to_string(),
                        formula: non_empty_formula(cell.get_formula()),
                    },
                ));
            }
            items
        })?;
        for (address, snapshot) in sheet_errors {
            out.insert(address, snapshot);
        }
    }
    Ok(out)
}

fn compare_error_maps(
    baseline: &BTreeMap<String, ErrorCellSnapshot>,
    current: &BTreeMap<String, ErrorCellSnapshot>,
) -> (Vec<ErrorDelta>, Vec<ErrorDelta>, Vec<ErrorDelta>) {
    let mut new_errors = Vec::new();
    let mut resolved_errors = Vec::new();
    let mut preexisting_errors = Vec::new();

    for (address, after) in current {
        if let Some(before) = baseline.get(address) {
            preexisting_errors.push(ErrorDelta {
                address: address.clone(),
                before_error: Some(before.error.clone()),
                after_error: Some(after.error.clone()),
                before_formula: before.formula.clone(),
                after_formula: after.formula.clone(),
            });
        } else {
            new_errors.push(ErrorDelta {
                address: address.clone(),
                before_error: None,
                after_error: Some(after.error.clone()),
                before_formula: None,
                after_formula: after.formula.clone(),
            });
        }
    }

    for (address, before) in baseline {
        if !current.contains_key(address) {
            resolved_errors.push(ErrorDelta {
                address: address.clone(),
                before_error: Some(before.error.clone()),
                after_error: None,
                before_formula: before.formula.clone(),
                after_formula: None,
            });
        }
    }

    (new_errors, resolved_errors, preexisting_errors)
}

fn compare_named_ranges(
    baseline: &[NamedRangeDescriptor],
    current: &[NamedRangeDescriptor],
) -> Vec<NamedRangeDelta> {
    let base_map: BTreeMap<String, &NamedRangeDescriptor> = baseline
        .iter()
        .map(|item| (named_range_key(item), item))
        .collect();
    let current_map: BTreeMap<String, &NamedRangeDescriptor> = current
        .iter()
        .map(|item| (named_range_key(item), item))
        .collect();

    let keys: BTreeSet<String> = base_map
        .keys()
        .cloned()
        .chain(current_map.keys().cloned())
        .collect();

    let mut deltas = Vec::new();
    for key in keys {
        let before = base_map.get(&key).copied();
        let after = current_map.get(&key).copied();
        match (before, after) {
            (Some(b), Some(a)) if b.refers_to != a.refers_to || b.kind != a.kind => {
                deltas.push(NamedRangeDelta {
                    name: a.name.clone(),
                    scope_kind: a.scope_kind,
                    scope_sheet_name: a.scope_sheet_name.clone(),
                    change: "changed".to_string(),
                    before_refers_to: Some(b.refers_to.clone()),
                    after_refers_to: Some(a.refers_to.clone()),
                    before_kind: Some(b.kind.clone()),
                    after_kind: Some(a.kind.clone()),
                });
            }
            (Some(b), None) => {
                deltas.push(NamedRangeDelta {
                    name: b.name.clone(),
                    scope_kind: b.scope_kind,
                    scope_sheet_name: b.scope_sheet_name.clone(),
                    change: "removed".to_string(),
                    before_refers_to: Some(b.refers_to.clone()),
                    after_refers_to: None,
                    before_kind: Some(b.kind.clone()),
                    after_kind: None,
                });
            }
            (None, Some(a)) => {
                deltas.push(NamedRangeDelta {
                    name: a.name.clone(),
                    scope_kind: a.scope_kind,
                    scope_sheet_name: a.scope_sheet_name.clone(),
                    change: "added".to_string(),
                    before_refers_to: None,
                    after_refers_to: Some(a.refers_to.clone()),
                    before_kind: None,
                    after_kind: Some(a.kind.clone()),
                });
            }
            _ => {}
        }
    }

    deltas
}

fn named_range_key(item: &NamedRangeDescriptor) -> String {
    format!(
        "{}|{:?}|{}|{:?}",
        item.name,
        item.scope_kind,
        item.scope_sheet_name.as_deref().unwrap_or(""),
        item.kind
    )
}

fn classify_target_delta(
    before: &TargetCellSnapshot,
    after: &TargetCellSnapshot,
    changed: bool,
) -> &'static str {
    if !changed {
        return "unchanged";
    }
    if is_error_value(after.value.as_ref()) && !is_error_value(before.value.as_ref()) {
        return "new_error";
    }
    if before.formula != after.formula {
        return "formula_shift";
    }
    if before.formula.is_none() && after.formula.is_none() {
        return "direct_edit";
    }
    "recalc_result"
}

fn is_error_value(value: Option<&CellValue>) -> bool {
    match value {
        Some(CellValue::Error(_)) => true,
        Some(other) => serde_json::to_value(other)
            .ok()
            .and_then(|json| {
                json.get("value")
                    .and_then(|v| v.as_str())
                    .map(is_error_text)
            })
            .unwrap_or(false),
        None => false,
    }
}

fn cell_values_equal(left: Option<&CellValue>, right: Option<&CellValue>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(l), Some(r)) => serde_json::to_value(l).ok() == serde_json::to_value(r).ok(),
        _ => false,
    }
}

fn non_empty_formula(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn is_error_text(raw: &str) -> bool {
    is_spreadsheet_error(raw)
}
