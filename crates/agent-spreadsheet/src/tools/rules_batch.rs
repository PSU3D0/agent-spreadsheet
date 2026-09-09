use crate::fork::{ChangeSummary, StagedChange, StagedOp};
use crate::model::diagnostics::{
    CommandClass, FORMULA_PARSE_FAILED_PREFIX, FormulaParseDiagnostics,
    FormulaParseDiagnosticsBuilder, FormulaParsePolicy, validate_formula,
};
use crate::model::{FillDescriptor, WorkbookId};
use crate::state::AppState;
use crate::styles::descriptor_from_style;
use crate::tools::param_enums::BatchMode;
use crate::utils::make_short_random_id;
use crate::{rules::conditional_format, styles::normalize_color_hex};
use anyhow::{Result, anyhow, bail};
use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use umya_spreadsheet::{
    ConditionalFormattingOperatorValues, DataValidation, DataValidationOperatorValues,
    DataValidationValues, DataValidations,
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RulesBatchParams {
    pub fork_id: String,
    pub ops: Vec<RulesOp>,
    #[serde(default)]
    pub mode: Option<BatchMode>, // preview|apply (default apply)
    pub label: Option<String>,
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RulesOp {
    SetDataValidation {
        sheet_name: String,
        target_range: String,
        validation: DataValidationSpec,
    },
    AddConditionalFormat {
        sheet_name: String,
        target_range: String,
        rule: ConditionalFormatRuleSpec,
        #[serde(default)]
        style: ConditionalFormatStyleSpec,
    },
    SetConditionalFormat {
        sheet_name: String,
        target_range: String,
        rule: ConditionalFormatRuleSpec,
        #[serde(default)]
        style: ConditionalFormatStyleSpec,
    },
    ClearConditionalFormats {
        sheet_name: String,
        target_range: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConditionalFormatRuleSpec {
    CellIs {
        operator: ConditionalFormatOperator,
        formula: String,
    },
    Expression {
        formula: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConditionalFormatOperator {
    #[serde(alias = "lessThan")]
    LessThan,
    #[serde(alias = "lessThanOrEqual")]
    LessThanOrEqual,
    #[serde(alias = "greaterThan")]
    GreaterThan,
    #[serde(alias = "greaterThanOrEqual")]
    GreaterThanOrEqual,
    #[serde(alias = "equal")]
    Equal,
    #[serde(alias = "notEqual")]
    NotEqual,
    #[serde(alias = "between")]
    Between,
    #[serde(alias = "notBetween")]
    NotBetween,
}

impl ConditionalFormatOperator {
    fn to_umya(self) -> ConditionalFormattingOperatorValues {
        match self {
            Self::LessThan => ConditionalFormattingOperatorValues::LessThan,
            Self::LessThanOrEqual => ConditionalFormattingOperatorValues::LessThanOrEqual,
            Self::GreaterThan => ConditionalFormattingOperatorValues::GreaterThan,
            Self::GreaterThanOrEqual => ConditionalFormattingOperatorValues::GreaterThanOrEqual,
            Self::Equal => ConditionalFormattingOperatorValues::Equal,
            Self::NotEqual => ConditionalFormattingOperatorValues::NotEqual,
            Self::Between => ConditionalFormattingOperatorValues::Between,
            Self::NotBetween => ConditionalFormattingOperatorValues::NotBetween,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ConditionalFormatStyleSpec {
    #[serde(default)]
    pub fill_color: Option<String>,
    #[serde(default)]
    pub font_color: Option<String>,
    #[serde(default)]
    pub bold: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DataValidationSpec {
    pub kind: DataValidationKind,
    pub formula1: String,
    #[serde(default)]
    pub formula2: Option<String>,
    #[serde(default)]
    pub allow_blank: Option<bool>,
    #[serde(default)]
    pub prompt: Option<ValidationMessage>,
    #[serde(default)]
    pub error: Option<ValidationMessage>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DataValidationKind {
    List,
    Whole,
    Decimal,
    Date,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ValidationMessage {
    pub title: String,
    pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RulesBatchResponse {
    pub fork_id: String,
    pub mode: String,
    pub change_id: Option<String>,
    pub ops_applied: usize,
    pub summary: ChangeSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct RulesBatchStagedPayload {
    pub(crate) ops: Vec<RulesOp>,
    #[serde(default)]
    pub(crate) formula_parse_policy: Option<FormulaParsePolicy>,
}

pub async fn rules_batch(
    state: Arc<AppState>,
    params: RulesBatchParams,
) -> Result<RulesBatchResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;
    let work_path = fork_ctx.work_path.clone();

    // Validate sheet names early against current fork snapshot.
    let fork_workbook_id = WorkbookId(params.fork_id.clone());
    let workbook = state.open_workbook(&fork_workbook_id).await?;
    for op in &params.ops {
        match op {
            RulesOp::SetDataValidation { sheet_name, .. } => {
                let _ = workbook.with_sheet(sheet_name, |_| Ok::<_, anyhow::Error>(()))?;
            }
            RulesOp::AddConditionalFormat { sheet_name, .. }
            | RulesOp::SetConditionalFormat { sheet_name, .. }
            | RulesOp::ClearConditionalFormats { sheet_name, .. } => {
                let _ = workbook.with_sheet(sheet_name, |_| Ok::<_, anyhow::Error>(()))?;
            }
        }
    }

    let policy =
        params
            .formula_parse_policy
            .unwrap_or(FormulaParsePolicy::default_for_command_class(
                CommandClass::BatchWrite,
            ));

    let mode = params.mode.unwrap_or_default();

    if mode.is_preview() {
        let change_id = make_short_random_id("chg", 12);
        let snapshot_path = crate::tools::fork::stage_snapshot_path(&params.fork_id, &change_id);
        fs::create_dir_all(snapshot_path.parent().unwrap())?;
        fs::copy(&work_path, &snapshot_path)?;

        let snapshot_for_apply = snapshot_path.clone();
        let ops_for_apply = params.ops.clone();
        let apply_result = tokio::task::spawn_blocking(move || {
            apply_rules_ops_to_file(&snapshot_for_apply, &ops_for_apply, policy)
        })
        .await??;

        let mut summary = apply_result.summary;
        summary
            .flags
            .insert("recalc_needed".to_string(), fork_ctx.recalc_needed);

        let staged_op = StagedOp {
            kind: "rules_batch".to_string(),
            payload: serde_json::to_value(RulesBatchStagedPayload {
                ops: params.ops.clone(),
                formula_parse_policy: Some(policy),
            })?,
        };
        let staged = StagedChange {
            change_id: change_id.clone(),
            created_at: Utc::now(),
            label: params.label.clone(),
            ops: vec![staged_op],
            summary: summary.clone(),
            fork_path_snapshot: Some(snapshot_path),
        };
        registry.add_staged_change(&params.fork_id, staged)?;

        Ok(RulesBatchResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: Some(change_id),
            ops_applied: apply_result.ops_applied,
            summary,
            formula_parse_diagnostics: apply_result.formula_parse_diagnostics,
        })
    } else {
        let work_path_for_apply = work_path.clone();
        let ops_for_apply = params.ops.clone();
        let apply_result = tokio::task::spawn_blocking(move || {
            apply_rules_ops_to_file(&work_path_for_apply, &ops_for_apply, policy)
        })
        .await??;

        let mut summary = apply_result.summary;
        summary
            .flags
            .insert("recalc_needed".to_string(), fork_ctx.recalc_needed);

        let _ = state.close_workbook(&fork_workbook_id);

        Ok(RulesBatchResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: None,
            ops_applied: apply_result.ops_applied,
            summary,
            formula_parse_diagnostics: apply_result.formula_parse_diagnostics,
        })
    }
}

pub(crate) struct RulesApplyResult {
    pub(crate) ops_applied: usize,
    pub(crate) summary: ChangeSummary,
    pub(crate) formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
}

/// Extract formula strings and field labels from a rules op for validation.
/// Returns tuples of (sheet_name, field_label, formula_text).
fn extract_rule_op_formulas(op: &RulesOp) -> Vec<(&str, &str, &str)> {
    match op {
        RulesOp::SetDataValidation {
            sheet_name,
            validation,
            ..
        } => {
            let mut formulas = vec![(
                sheet_name.as_str(),
                "formula1",
                validation.formula1.as_str(),
            )];
            if let Some(formula2) = &validation.formula2 {
                formulas.push((sheet_name.as_str(), "formula2", formula2.as_str()));
            }
            formulas
        }
        RulesOp::AddConditionalFormat {
            sheet_name, rule, ..
        }
        | RulesOp::SetConditionalFormat {
            sheet_name, rule, ..
        } => match rule {
            ConditionalFormatRuleSpec::CellIs { formula, .. }
            | ConditionalFormatRuleSpec::Expression { formula } => {
                vec![(sheet_name.as_str(), "rule.formula", formula.as_str())]
            }
        },
        RulesOp::ClearConditionalFormats { .. } => Vec::new(),
    }
}

pub(crate) fn apply_rules_ops_to_file(
    path: &Path,
    ops: &[RulesOp],
    policy: FormulaParsePolicy,
) -> Result<RulesApplyResult> {
    let mut book = crate::xlsx_import::read(path)?;
    let result = apply_rules_ops_to_workbook(&mut book, ops, policy)?;
    crate::xlsx_export::write(&book, path)?;
    Ok(result)
}

pub(crate) fn apply_rules_ops_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    ops: &[RulesOp],
    policy: FormulaParsePolicy,
) -> Result<RulesApplyResult> {
    let mut affected_sheets: BTreeSet<String> = BTreeSet::new();
    let mut affected_bounds: Vec<String> = Vec::new();
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();

    let mut validations_set: u64 = 0;
    let mut validations_replaced: u64 = 0;
    let mut conditional_formats_added: u64 = 0;
    let mut conditional_formats_skipped: u64 = 0;
    let mut conditional_formats_set: u64 = 0;
    let mut conditional_formats_replaced: u64 = 0;
    let mut conditional_formats_set_skipped: u64 = 0;
    let mut conditional_formats_cleared: u64 = 0;

    let mut formula_parse_diagnostics_builder = FormulaParseDiagnosticsBuilder::new(policy);
    let ops_to_apply: Vec<&RulesOp> = if policy == FormulaParsePolicy::Off {
        ops.iter().collect()
    } else {
        let mut valid_ops = Vec::new();
        for op in ops {
            let formulas = extract_rule_op_formulas(op);
            if formulas.is_empty() {
                valid_ops.push(op);
                continue;
            }

            let mut op_valid = true;
            for (sheet_name, field, formula_text) in formulas {
                let normalized = formula_text.trim();
                let to_validate = normalized.strip_prefix('=').unwrap_or(normalized);
                if to_validate.is_empty() {
                    continue;
                }

                if let Err(err_msg) = validate_formula(to_validate) {
                    if policy == FormulaParsePolicy::Fail {
                        bail!(
                            "{}{} in {}: {}",
                            FORMULA_PARSE_FAILED_PREFIX,
                            err_msg,
                            field,
                            formula_text
                        );
                    }
                    formula_parse_diagnostics_builder.record_error(
                        sheet_name,
                        field,
                        formula_text,
                        &err_msg,
                    );
                    op_valid = false;
                }
            }

            if op_valid {
                valid_ops.push(op);
            }
        }
        valid_ops
    };

    let ops_applied = ops_to_apply.len();

    let mut warned_not_parsed = false;
    let mut warned_cf_structure = false;

    for op in ops_to_apply {
        match op {
            RulesOp::SetDataValidation {
                sheet_name,
                target_range,
                validation,
            } => {
                let sheet = book
                    .sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;

                affected_sheets.insert(sheet_name.clone());
                affected_bounds.push(target_range.clone());

                if !warned_not_parsed && policy == FormulaParsePolicy::Off {
                    warnings.push(
                        "WARN_VALIDATION_FORMULA_NOT_PARSED: Validation formulas are applied verbatim (not parsed or validated)."
                            .to_string(),
                    );
                    warned_not_parsed = true;
                }

                let (set_inc, replaced_inc) =
                    set_data_validation(sheet, target_range, validation, &mut warnings)?;
                validations_set += set_inc;
                validations_replaced += replaced_inc;
            }
            RulesOp::AddConditionalFormat {
                sheet_name,
                target_range,
                rule,
                style,
            } => {
                let sheet = book
                    .sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;

                affected_sheets.insert(sheet_name.clone());
                affected_bounds.push(target_range.clone());

                if !warned_cf_structure {
                    warnings.push("WARN_CF_FORMULA_NOT_ADJUSTED_ON_STRUCTURE: Conditional format formulas are not automatically rewritten on structural edits; re-apply or review after row/col insertion/deletion.".to_string());
                    warned_cf_structure = true;
                }

                let (added, skipped) =
                    add_conditional_format(sheet, target_range, rule, style, &mut warnings)?;
                conditional_formats_added += added;
                conditional_formats_skipped += skipped;
            }
            RulesOp::SetConditionalFormat {
                sheet_name,
                target_range,
                rule,
                style,
            } => {
                let sheet = book
                    .sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;

                affected_sheets.insert(sheet_name.clone());
                affected_bounds.push(target_range.clone());

                if !warned_cf_structure {
                    warnings.push("WARN_CF_FORMULA_NOT_ADJUSTED_ON_STRUCTURE: Conditional format formulas are not automatically rewritten on structural edits; re-apply or review after row/col insertion/deletion.".to_string());
                    warned_cf_structure = true;
                }

                let (set, replaced, skipped) =
                    set_conditional_format(sheet, target_range, rule, style, &mut warnings)?;
                conditional_formats_set += set;
                conditional_formats_replaced += replaced;
                conditional_formats_set_skipped += skipped;
            }
            RulesOp::ClearConditionalFormats {
                sheet_name,
                target_range,
            } => {
                let sheet = book
                    .sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;

                affected_sheets.insert(sheet_name.clone());
                affected_bounds.push(target_range.clone());

                let cleared = clear_conditional_formats(sheet, target_range)?;
                conditional_formats_cleared += cleared;
            }
        }
    }

    counts.insert("validations_set".to_string(), validations_set);
    counts.insert("validations_replaced".to_string(), validations_replaced);
    counts.insert(
        "conditional_formats_added".to_string(),
        conditional_formats_added,
    );
    counts.insert(
        "conditional_formats_skipped".to_string(),
        conditional_formats_skipped,
    );
    counts.insert(
        "conditional_formats_set".to_string(),
        conditional_formats_set,
    );
    counts.insert(
        "conditional_formats_replaced".to_string(),
        conditional_formats_replaced,
    );
    counts.insert(
        "conditional_formats_set_skipped".to_string(),
        conditional_formats_set_skipped,
    );
    counts.insert(
        "conditional_formats_cleared".to_string(),
        conditional_formats_cleared,
    );

    let formula_parse_diagnostics = if formula_parse_diagnostics_builder.has_errors() {
        Some(formula_parse_diagnostics_builder.build())
    } else {
        None
    };

    Ok(RulesApplyResult {
        ops_applied,
        summary: ChangeSummary {
            op_kinds: vec!["rules_batch".to_string()],
            affected_sheets: affected_sheets.into_iter().collect(),
            affected_bounds,
            counts,
            warnings,
            ..Default::default()
        },
        formula_parse_diagnostics,
    })
}

fn normalize_sqref(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("target_range is required");
    }
    // DV sqref is space-separated list of ranges; v1 uses a single range.
    Ok(trimmed.replace(' ', "").to_ascii_uppercase())
}

fn normalize_cf_formula(field: &str, value: &str, warnings: &mut Vec<String>) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("{field} is required");
    }
    if let Some(stripped) = trimmed.strip_prefix('=') {
        warnings.push(format!(
            "WARN_CF_FORMULA_PREFIX: Stripped leading '=' from {field}"
        ));
        return Ok(stripped.to_string());
    }
    Ok(trimmed.to_string())
}

fn normalize_argb_color(field: &str, input: &str, warnings: &mut Vec<String>) -> Result<String> {
    let trimmed = input.trim();
    let Some((argb, defaulted_alpha)) = normalize_color_hex(trimmed) else {
        bail!("invalid color for {field}: expected #RGB/#RRGGBB/#AARRGGBB");
    };
    if defaulted_alpha {
        warnings.push(format!(
            "WARN_COLOR_ALPHA_DEFAULT: Defaulted alpha to FF for {field}"
        ));
    }
    Ok(argb)
}

fn add_conditional_format(
    sheet: &mut umya_spreadsheet::Worksheet,
    target_range: &str,
    rule: &ConditionalFormatRuleSpec,
    style: &ConditionalFormatStyleSpec,
    warnings: &mut Vec<String>,
) -> Result<(u64, u64)> {
    let sqref = normalize_sqref(target_range)?;

    let desired = match rule {
        ConditionalFormatRuleSpec::Expression { formula } => (
            umya_spreadsheet::ConditionalFormatValues::Expression,
            None,
            normalize_cf_formula("rule.formula", formula, warnings)?,
        ),
        ConditionalFormatRuleSpec::CellIs { operator, formula } => (
            umya_spreadsheet::ConditionalFormatValues::CellIs,
            Some(operator.to_umya()),
            normalize_cf_formula("rule.formula", formula, warnings)?,
        ),
    };

    // Defaults aim for determinism and readability.
    let fill = style.fill_color.as_deref().unwrap_or("FFFFE0E0");
    let font = style.font_color.as_deref().unwrap_or("FF000000");
    let bold = style.bold.unwrap_or(false);

    let fill_argb = normalize_argb_color("style.fill_color", fill, warnings)?;
    let font_argb = normalize_argb_color("style.font_color", font, warnings)?;

    // Deduplicate exact matches (sqref + kind/operator + formula).
    for existing in sheet.conditional_formatting_collection() {
        let existing_sqref = existing.sequence_of_references().get_sqref();
        let existing_norm = existing_sqref.replace(' ', "").to_ascii_uppercase();
        if existing_norm != sqref {
            continue;
        }
        for existing_rule in existing.conditional_collection() {
            if existing_rule.get_type() != &desired.0 {
                continue;
            }
            if let Some(ref op) = desired.1
                && existing_rule.operator() != op
            {
                continue;
            }
            let existing_formula = existing_rule
                .formula()
                .map(|f| f.address_str())
                .unwrap_or_default();
            if existing_formula == desired.2 {
                return Ok((0, 1));
            }
        }
    }

    let dxf_style = conditional_format::build_simple_dxf_style(&fill_argb, &font_argb, bold);

    match desired.0 {
        umya_spreadsheet::ConditionalFormatValues::Expression => {
            conditional_format::append_cf_expression_rule(sheet, &sqref, &desired.2, dxf_style);
        }
        umya_spreadsheet::ConditionalFormatValues::CellIs => {
            conditional_format::append_cf_cellis_rule(
                sheet,
                &sqref,
                desired
                    .1
                    .clone()
                    .unwrap_or(ConditionalFormattingOperatorValues::LessThan),
                &desired.2,
                dxf_style,
            );
        }
        _ => unreachable!("only expression and cellIs are supported"),
    }

    Ok((1, 0))
}

fn clear_conditional_formats(
    sheet: &mut umya_spreadsheet::Worksheet,
    target_range: &str,
) -> Result<u64> {
    let sqref = normalize_sqref(target_range)?;
    let before = sheet.conditional_formatting_collection().len();
    if before == 0 {
        return Ok(0);
    }

    let mut kept: Vec<umya_spreadsheet::ConditionalFormatting> = Vec::new();
    for cf in sheet.conditional_formatting_collection() {
        let existing = cf.sequence_of_references().get_sqref();
        let existing_norm = existing.replace(' ', "").to_ascii_uppercase();
        if existing_norm != sqref {
            kept.push(cf.clone());
        }
    }

    let removed = before.saturating_sub(kept.len()) as u64;
    if removed > 0 {
        sheet.set_conditional_formatting_collection(kept);
    }
    Ok(removed)
}

fn cf_rule_core_matches(
    existing: &umya_spreadsheet::ConditionalFormattingRule,
    desired_kind: &umya_spreadsheet::ConditionalFormatValues,
    desired_operator: Option<&ConditionalFormattingOperatorValues>,
    desired_formula: &str,
) -> bool {
    if existing.get_type() != desired_kind {
        return false;
    }
    if let Some(op) = desired_operator
        && existing.operator() != op
    {
        return false;
    }
    let existing_formula = existing
        .formula()
        .map(|f| f.address_str())
        .unwrap_or_default();
    existing_formula == desired_formula
}

fn cf_rule_style_matches(
    existing: &umya_spreadsheet::ConditionalFormattingRule,
    desired_fill_argb: &str,
    desired_font_argb: &str,
    desired_bold: bool,
) -> bool {
    let Some(style) = existing.style() else {
        return false;
    };

    let desc = descriptor_from_style(style);
    let existing_bold = desc.font.as_ref().and_then(|f| f.bold).unwrap_or(false);
    if existing_bold != desired_bold {
        return false;
    }
    if desc.font.as_ref().and_then(|f| f.color.as_deref()) != Some(desired_font_argb) {
        return false;
    }

    match &desc.fill {
        Some(FillDescriptor::Pattern(p)) => {
            p.foreground_color.as_deref() == Some(desired_fill_argb)
        }
        _ => false,
    }
}

fn set_conditional_format(
    sheet: &mut umya_spreadsheet::Worksheet,
    target_range: &str,
    rule: &ConditionalFormatRuleSpec,
    style: &ConditionalFormatStyleSpec,
    warnings: &mut Vec<String>,
) -> Result<(u64, u64, u64)> {
    let sqref = normalize_sqref(target_range)?;

    let desired_kind;
    let desired_operator: Option<ConditionalFormattingOperatorValues>;
    let desired_formula: String;
    match rule {
        ConditionalFormatRuleSpec::Expression { formula } => {
            desired_kind = umya_spreadsheet::ConditionalFormatValues::Expression;
            desired_operator = None;
            desired_formula = normalize_cf_formula("rule.formula", formula, warnings)?;
        }
        ConditionalFormatRuleSpec::CellIs { operator, formula } => {
            desired_kind = umya_spreadsheet::ConditionalFormatValues::CellIs;
            desired_operator = Some(operator.to_umya());
            desired_formula = normalize_cf_formula("rule.formula", formula, warnings)?;
        }
    }

    // Defaults aim for determinism and readability.
    let fill = style.fill_color.as_deref().unwrap_or("FFFFE0E0");
    let font = style.font_color.as_deref().unwrap_or("FF000000");
    let bold = style.bold.unwrap_or(false);
    let fill_argb = normalize_argb_color("style.fill_color", fill, warnings)?;
    let font_argb = normalize_argb_color("style.font_color", font, warnings)?;

    // If already exactly set (one cf block, one rule, matches core + style), skip.
    let matches: Vec<&umya_spreadsheet::ConditionalFormatting> = sheet
        .conditional_formatting_collection()
        .iter()
        .filter(|cf| {
            let existing = cf.sequence_of_references().get_sqref();
            let existing_norm = existing.replace(' ', "").to_ascii_uppercase();
            existing_norm == sqref
        })
        .collect();
    if matches.len() == 1 {
        let rules = matches[0].conditional_collection();
        if rules.len() == 1 {
            let existing = &rules[0];
            if cf_rule_core_matches(
                existing,
                &desired_kind,
                desired_operator.as_ref(),
                &desired_formula,
            ) && cf_rule_style_matches(existing, &fill_argb, &font_argb, bold)
            {
                return Ok((0, 0, 1));
            }
        }
    }

    // Remove all existing CF blocks targeting the same sqref.
    let mut replaced: u64 = 0;
    if !sheet.conditional_formatting_collection().is_empty() {
        let mut kept: Vec<umya_spreadsheet::ConditionalFormatting> = Vec::new();
        for cf in sheet.conditional_formatting_collection() {
            let existing = cf.sequence_of_references().get_sqref();
            let existing_norm = existing.replace(' ', "").to_ascii_uppercase();
            if existing_norm == sqref {
                replaced += 1;
            } else {
                kept.push(cf.clone());
            }
        }
        if replaced > 0 {
            sheet.set_conditional_formatting_collection(kept);
        }
    }

    let dxf_style = conditional_format::build_simple_dxf_style(&fill_argb, &font_argb, bold);
    match desired_kind {
        umya_spreadsheet::ConditionalFormatValues::Expression => {
            conditional_format::append_cf_expression_rule(
                sheet,
                &sqref,
                &desired_formula,
                dxf_style,
            );
        }
        umya_spreadsheet::ConditionalFormatValues::CellIs => {
            conditional_format::append_cf_cellis_rule(
                sheet,
                &sqref,
                desired_operator
                    .clone()
                    .unwrap_or(ConditionalFormattingOperatorValues::LessThan),
                &desired_formula,
                dxf_style,
            );
        }
        _ => unreachable!("only expression and cellIs are supported"),
    }

    Ok((1, replaced, 0))
}

fn normalize_dv_formula(field: &str, value: &str, warnings: &mut Vec<String>) -> String {
    let trimmed = value.trim();
    if let Some(stripped) = trimmed.strip_prefix('=') {
        warnings.push(format!(
            "WARN_VALIDATION_FORMULA_PREFIX: Stripped leading '=' from {field}"
        ));
        stripped.to_string()
    } else {
        trimmed.to_string()
    }
}

fn set_data_validation(
    sheet: &mut umya_spreadsheet::Worksheet,
    target_range: &str,
    spec: &DataValidationSpec,
    warnings: &mut Vec<String>,
) -> Result<(u64, u64)> {
    let sqref = normalize_sqref(target_range)?;

    if sheet.data_validations_mut().is_none() {
        sheet.set_data_validations(DataValidations::default());
    }
    let dvs = sheet
        .data_validations_mut()
        .ok_or_else(|| anyhow!("failed to initialize data validations"))?;

    // Remove any existing validations targeting the same sqref.
    let list = dvs.data_validation_list_mut();
    let before = list.len();
    list.retain(|dv| {
        let existing = dv.sequence_of_references().get_sqref();
        let existing_norm = existing.replace(' ', "").to_ascii_uppercase();
        existing_norm != sqref
    });
    let removed = before.saturating_sub(list.len());

    let mut dv = DataValidation::default();
    dv.set_type(spec.kind.to_umya());
    dv.sequence_of_references_mut().set_sqref(sqref.clone());

    if let Some(allow_blank) = spec.allow_blank {
        dv.set_allow_blank(allow_blank);
    }

    // Excel stores DV formulas without a leading '=' in OOXML.
    let formula1 = normalize_dv_formula("formula1", &spec.formula1, warnings);
    dv.set_formula1(formula1);
    if let Some(f2) = spec.formula2.as_ref() {
        let formula2 = normalize_dv_formula("formula2", f2, warnings);
        if !formula2.is_empty() {
            dv.set_formula2(formula2);
        }
    }

    // Operator: keep surface minimal; default to between when formula2 is provided.
    match spec.kind {
        DataValidationKind::Whole | DataValidationKind::Decimal | DataValidationKind::Date => {
            let op = if spec.formula2.as_ref().is_some_and(|s| !s.trim().is_empty()) {
                DataValidationOperatorValues::Between
            } else {
                DataValidationOperatorValues::Equal
            };
            dv.set_operator(op);
        }
        DataValidationKind::List | DataValidationKind::Custom => {}
    }

    if let Some(prompt) = spec.prompt.as_ref() {
        dv.set_show_input_message(true);
        if !prompt.title.is_empty() {
            dv.set_prompt_title(prompt.title.clone());
        }
        if !prompt.message.is_empty() {
            dv.set_prompt(prompt.message.clone());
        }
    }

    if let Some(error) = spec.error.as_ref() {
        dv.set_show_error_message(true);
        if !error.title.is_empty() {
            dv.set_error_title(error.title.clone());
        }
        if !error.message.is_empty() {
            dv.set_error_message(error.message.clone());
        }
    }

    dvs.add_data_validation_list(dv);

    Ok((1, if removed > 0 { 1 } else { 0 }))
}

impl DataValidationKind {
    fn to_umya(self) -> DataValidationValues {
        match self {
            DataValidationKind::List => DataValidationValues::List,
            DataValidationKind::Whole => DataValidationValues::Whole,
            DataValidationKind::Decimal => DataValidationValues::Decimal,
            DataValidationKind::Date => DataValidationValues::Date,
            DataValidationKind::Custom => DataValidationValues::Custom,
        }
    }
}
