use super::param_enums::{BatchMode, FillDirection, FormulaRelativeMode, ReplaceMatchMode};
use crate::config::RecalcBackendKind;
use crate::fork::{ChangeSummary, EditOp, StagedChange, StagedOp};
use crate::formula::pattern::{RelativeMode, parse_base_formula, shift_formula_ast};
use crate::model::{
    AlignmentPatch, BordersPatch, CommandClass, FORMULA_PARSE_FAILED_PREFIX, FillPatch, FontPatch,
    FormulaParseDiagnostics, FormulaParseDiagnosticsBuilder, FormulaParsePolicy, PatternFillPatch,
    StylePatch, Warning, WorkbookId, validate_formula,
};
use crate::recalc::RecalcBackend;
#[cfg(not(target_arch = "wasm32"))]
use crate::security::sanitize_filename_component;
use crate::state::AppState;
use crate::tools::write_normalize::{EditBatchParamsInput, normalize_edit_batch};
use crate::utils::make_short_random_id;
use anyhow::{Result, anyhow, bail};
use chrono::Utc;
use formualizer_parse::tokenizer::Tokenizer;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn set_recalc_needed_flag(summary: &mut ChangeSummary, recalc_needed: bool) {
    summary
        .flags
        .insert("recalc_needed".to_string(), recalc_needed);
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateForkParams {
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CreateForkResponse {
    pub fork_id: String,
    pub base_workbook: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_base_workbook: Option<String>,
    pub ttl_seconds: u64,
}

pub async fn create_fork(
    state: Arc<AppState>,
    params: CreateForkParams,
) -> Result<CreateForkResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available (recalc disabled?)"))?;

    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let base_path = &workbook.path;
    let config = state.config();
    let workspace_root = &config.workspace_root;

    let fork_id = registry.create_fork(base_path, workspace_root)?;

    Ok(CreateForkResponse {
        fork_id,
        base_workbook: base_path.display().to_string(),
        client_base_workbook: config
            .map_path_for_client(base_path)
            .map(|p| p.display().to_string()),
        ttl_seconds: registry.ttl().as_secs(),
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EditBatchParams {
    pub fork_id: String,
    pub sheet_name: String,
    pub edits: Vec<CellEdit>,
    #[serde(default)]
    pub mode: Option<BatchMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CellEdit {
    pub address: String,
    pub value: String,
    #[serde(default)]
    pub is_formula: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct EditBatchResponse {
    pub fork_id: String,
    /// "apply" or "preview".
    pub mode: String,
    /// Staged change id (preview mode only); apply via apply_staged_change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_id: Option<String>,
    /// Number of edits written to the fork (apply mode only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edits_applied: Option<usize>,
    /// Number of edits staged without mutating the fork (preview mode only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edits_staged: Option<usize>,
    pub total_edits: usize,
    pub recalc_needed: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
}

pub async fn edit_batch(
    state: Arc<AppState>,
    params: EditBatchParamsInput,
) -> Result<EditBatchResponse> {
    let policy =
        params
            .formula_parse_policy
            .unwrap_or(FormulaParsePolicy::default_for_command_class(
                CommandClass::BatchWrite,
            ));
    let (params, warnings) = normalize_edit_batch(params)?;

    let (edits_to_write, formula_parse_diagnostics) = if policy == FormulaParsePolicy::Off {
        (params.edits.clone(), None)
    } else {
        let mut builder = FormulaParseDiagnosticsBuilder::new(policy);
        let mut valid_edits = Vec::new();
        for edit in &params.edits {
            if edit.is_formula {
                match validate_formula(&edit.value) {
                    Ok(()) => valid_edits.push(edit.clone()),
                    Err(err_msg) => {
                        if policy == FormulaParsePolicy::Fail {
                            bail!(
                                "{}edit at {} failed: {}",
                                FORMULA_PARSE_FAILED_PREFIX,
                                edit.address,
                                err_msg
                            );
                        }
                        builder.record_error(
                            &params.sheet_name,
                            &edit.address,
                            &edit.value,
                            &err_msg,
                        );
                    }
                }
            } else {
                valid_edits.push(edit.clone());
            }
        }
        let diagnostics = if builder.has_errors() {
            Some(builder.build())
        } else {
            None
        };
        (valid_edits, diagnostics)
    };

    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;
    let work_path = fork_ctx.work_path.clone();

    let mode = params.mode.unwrap_or_default();

    if mode.is_preview() {
        let edit_count = edits_to_write.len();
        let change_id = make_short_random_id("chg", 12);
        let snapshot_path = stage_snapshot_path(&params.fork_id, &change_id);
        fs::create_dir_all(snapshot_path.parent().unwrap())?;
        fs::copy(&work_path, &snapshot_path)?;

        // Apply to the snapshot only: validates the edits without touching the fork.
        tokio::task::spawn_blocking({
            let sheet_name = params.sheet_name.clone();
            let edits = edits_to_write.clone();
            let snapshot_path = snapshot_path.clone();
            move || {
                let core_edits = edits
                    .into_iter()
                    .map(|edit| crate::core::types::CellEdit {
                        address: edit.address,
                        value: edit.value,
                        is_formula: edit.is_formula,
                    })
                    .collect::<Vec<_>>();
                crate::core::write::apply_edits_to_file(&snapshot_path, &sheet_name, &core_edits)
            }
        })
        .await??;

        let mut summary = ChangeSummary {
            op_kinds: vec!["edit_batch".to_string()],
            affected_sheets: vec![params.sheet_name.clone()],
            ..ChangeSummary::default()
        };
        summary
            .counts
            .insert("cells_edited".to_string(), edit_count as u64);
        set_recalc_needed_flag(&mut summary, fork_ctx.recalc_needed);

        let staged_op = StagedOp {
            kind: "edit_batch".to_string(),
            payload: serde_json::to_value(EditBatchStagedPayload {
                sheet_name: params.sheet_name.clone(),
                edits: edits_to_write.clone(),
            })?,
        };

        let staged = StagedChange {
            change_id: change_id.clone(),
            created_at: Utc::now(),
            label: None,
            ops: vec![staged_op],
            summary,
            fork_path_snapshot: Some(snapshot_path),
        };

        registry.add_staged_change(&params.fork_id, staged)?;

        let total = registry.get_fork(&params.fork_id)?.edits.len();

        return Ok(EditBatchResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: Some(change_id),
            edits_applied: None,
            edits_staged: Some(edit_count),
            total_edits: total,
            recalc_needed: fork_ctx.recalc_needed,
            warnings,
            formula_parse_diagnostics,
        });
    }

    let edits_to_apply: Vec<_> = edits_to_write
        .iter()
        .map(|e| EditOp {
            timestamp: Utc::now(),
            sheet: params.sheet_name.clone(),
            address: e.address.clone(),
            value: e.value.clone(),
            is_formula: e.is_formula,
        })
        .collect();

    let edit_count = edits_to_apply.len();

    tokio::task::spawn_blocking({
        let sheet_name = params.sheet_name.clone();
        let edits = edits_to_write.clone();
        move || {
            let core_edits = edits
                .into_iter()
                .map(|edit| crate::core::types::CellEdit {
                    address: edit.address,
                    value: edit.value,
                    is_formula: edit.is_formula,
                })
                .collect::<Vec<_>>();
            crate::core::write::apply_edits_to_file(&work_path, &sheet_name, &core_edits)
        }
    })
    .await??;

    let total = registry.with_fork_mut(&params.fork_id, |ctx| {
        ctx.edits.extend(edits_to_apply);
        ctx.recalc_needed = true;
        Ok(ctx.edits.len())
    })?;

    let fork_workbook_id = WorkbookId(params.fork_id.clone());
    let _ = state.close_workbook(&fork_workbook_id);

    Ok(EditBatchResponse {
        fork_id: params.fork_id,
        mode: mode.as_str().to_string(),
        change_id: None,
        edits_applied: Some(edit_count),
        edits_staged: None,
        total_edits: total,
        recalc_needed: true,
        warnings,
        formula_parse_diagnostics,
    })
}

fn default_clone_count() -> u32 {
    1
}

fn default_clear_values() -> bool {
    true
}

fn default_overwrite_formulas() -> bool {
    false
}

fn default_replace_case_sensitive() -> bool {
    true
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TransformBatchParams {
    pub fork_id: String,
    pub ops: Vec<TransformOp>,
    #[serde(default)]
    pub mode: Option<BatchMode>, // preview|apply (default apply)
    pub label: Option<String>,
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum MatrixCell {
    #[serde(rename = "v")]
    Value(serde_json::Value),
    #[serde(rename = "f")]
    Formula(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransformOp {
    ClearRange {
        sheet_name: String,
        target: TransformTarget,
        #[serde(default = "default_clear_values")]
        clear_values: bool,
        #[serde(default)]
        clear_formulas: bool,
    },
    FillRange {
        sheet_name: String,
        target: TransformTarget,
        value: String,
        #[serde(default)]
        is_formula: bool,
        #[serde(default = "default_overwrite_formulas")]
        overwrite_formulas: bool,
    },
    ReplaceInRange {
        sheet_name: String,
        target: TransformTarget,
        find: String,
        replace: String,
        #[serde(default)]
        match_mode: ReplaceMatchMode,
        #[serde(default = "default_replace_case_sensitive")]
        case_sensitive: bool,
        #[serde(default)]
        include_formulas: bool,
    },
    WriteMatrix {
        sheet_name: String,
        anchor: String,
        rows: Vec<Vec<Option<MatrixCell>>>,
        #[serde(default = "default_overwrite_formulas")]
        overwrite_formulas: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransformTarget {
    Range { range: String },
    Region { region_id: u32 },
    Cells { cells: Vec<String> },
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TransformBatchResponse {
    pub fork_id: String,
    pub mode: String,
    pub change_id: Option<String>,
    pub ops_applied: usize,
    pub summary: ChangeSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TransformBatchStagedPayload {
    ops: Vec<TransformOp>,
}

pub(crate) fn resolve_transform_ops_for_workbook(
    workbook: &crate::workbook::WorkbookContext,
    ops: &[TransformOp],
) -> Result<Vec<TransformOp>> {
    let mut resolved_ops = Vec::with_capacity(ops.len());

    for op in ops {
        match op {
            TransformOp::WriteMatrix { .. } => {
                resolved_ops.push(op.clone());
            }
            TransformOp::ClearRange {
                sheet_name, target, ..
            }
            | TransformOp::FillRange {
                sheet_name, target, ..
            }
            | TransformOp::ReplaceInRange {
                sheet_name, target, ..
            } => {
                let resolved_target = match target {
                    TransformTarget::Region { region_id } => {
                        let metrics = workbook.get_sheet_metrics(sheet_name)?;
                        let regions = metrics.detected_regions();
                        let region =
                            regions.iter().find(|r| r.id == *region_id).ok_or_else(|| {
                                anyhow!(
                                    "region_id {} not found on sheet '{}'",
                                    region_id,
                                    sheet_name
                                )
                            })?;
                        TransformTarget::Range {
                            range: region.bounds.clone(),
                        }
                    }
                    other => other.clone(),
                };

                match op {
                    TransformOp::ClearRange {
                        sheet_name,
                        clear_values,
                        clear_formulas,
                        ..
                    } => {
                        resolved_ops.push(TransformOp::ClearRange {
                            sheet_name: sheet_name.clone(),
                            target: resolved_target,
                            clear_values: *clear_values,
                            clear_formulas: *clear_formulas,
                        });
                    }
                    TransformOp::FillRange {
                        sheet_name,
                        value,
                        is_formula,
                        overwrite_formulas,
                        ..
                    } => {
                        resolved_ops.push(TransformOp::FillRange {
                            sheet_name: sheet_name.clone(),
                            target: resolved_target,
                            value: value.clone(),
                            is_formula: *is_formula,
                            overwrite_formulas: *overwrite_formulas,
                        });
                    }
                    TransformOp::ReplaceInRange {
                        sheet_name,
                        find,
                        replace,
                        match_mode,
                        case_sensitive,
                        include_formulas,
                        ..
                    } => {
                        resolved_ops.push(TransformOp::ReplaceInRange {
                            sheet_name: sheet_name.clone(),
                            target: resolved_target,
                            find: find.clone(),
                            replace: replace.clone(),
                            match_mode: *match_mode,
                            case_sensitive: *case_sensitive,
                            include_formulas: *include_formulas,
                        });
                    }
                    TransformOp::WriteMatrix { .. } => unreachable!(),
                }
            }
        }
    }

    Ok(resolved_ops)
}

pub async fn transform_batch(
    state: Arc<AppState>,
    params: TransformBatchParams,
) -> Result<TransformBatchResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;
    let work_path = fork_ctx.work_path.clone();

    let fork_workbook_id = WorkbookId(params.fork_id.clone());
    let workbook = state.open_workbook(&fork_workbook_id).await?;

    let resolved_ops = resolve_transform_ops_for_workbook(&workbook, &params.ops)?;

    let policy =
        params
            .formula_parse_policy
            .unwrap_or(FormulaParsePolicy::default_for_command_class(
                CommandClass::BatchWrite,
            ));

    let (ops_to_apply, formula_parse_diagnostics) = if policy == FormulaParsePolicy::Off {
        (resolved_ops, None)
    } else {
        let mut builder = FormulaParseDiagnosticsBuilder::new(policy);
        let mut valid_ops = Vec::new();
        for op in resolved_ops {
            match &op {
                TransformOp::FillRange {
                    sheet_name,
                    value,
                    is_formula,
                    ..
                } if *is_formula => match validate_formula(value) {
                    Ok(()) => valid_ops.push(op),
                    Err(err_msg) => {
                        if policy == FormulaParsePolicy::Fail {
                            bail!(
                                "{}FillRange formula failed: {}",
                                FORMULA_PARSE_FAILED_PREFIX,
                                err_msg
                            );
                        }
                        builder.record_error(sheet_name, "FillRange", value, &err_msg);
                    }
                },
                TransformOp::WriteMatrix {
                    sheet_name,
                    anchor,
                    rows,
                    overwrite_formulas,
                } => {
                    let mut has_errors = false;
                    let mut valid_rows = Vec::new();

                    let (anchor_col, anchor_row) = parse_cell_ref(anchor)?;

                    for (r_idx, row) in rows.iter().enumerate() {
                        let mut valid_row = Vec::new();
                        let r = anchor_row + r_idx as u32;

                        for (c_idx, cell_opt) in row.iter().enumerate() {
                            let c = anchor_col + c_idx as u32;
                            if let Some(MatrixCell::Formula(f)) = cell_opt {
                                match validate_formula(f) {
                                    Ok(()) => valid_row.push(cell_opt.clone()),
                                    Err(err_msg) => {
                                        if policy == FormulaParsePolicy::Fail {
                                            bail!(
                                                "{}WriteMatrix formula failed at {}: {}",
                                                FORMULA_PARSE_FAILED_PREFIX,
                                                crate::utils::cell_address(c, r),
                                                err_msg
                                            );
                                        }
                                        builder.record_error(
                                            sheet_name,
                                            &crate::utils::cell_address(c, r),
                                            f,
                                            &err_msg,
                                        );
                                        has_errors = true;
                                        valid_row.push(None); // drop the invalid formula cell if warn
                                    }
                                }
                            } else {
                                valid_row.push(cell_opt.clone());
                            }
                        }
                        valid_rows.push(valid_row);
                    }

                    if has_errors && policy == FormulaParsePolicy::Warn {
                        valid_ops.push(TransformOp::WriteMatrix {
                            sheet_name: sheet_name.clone(),
                            anchor: anchor.clone(),
                            rows: valid_rows,
                            overwrite_formulas: *overwrite_formulas,
                        });
                    } else {
                        valid_ops.push(op);
                    }
                }
                _ => valid_ops.push(op),
            }
        }
        let diagnostics = if builder.has_errors() {
            Some(builder.build())
        } else {
            None
        };
        (valid_ops, diagnostics)
    };

    let mode = params.mode.unwrap_or_default();

    if mode.is_preview() {
        let change_id = make_short_random_id("chg", 12);
        let snapshot_path = stage_snapshot_path(&params.fork_id, &change_id);
        fs::create_dir_all(snapshot_path.parent().unwrap())?;
        fs::copy(&work_path, &snapshot_path)?;

        let snapshot_for_apply = snapshot_path.clone();
        let apply_result = tokio::task::spawn_blocking({
            let ops = ops_to_apply.clone();
            move || apply_transform_ops_to_file(&snapshot_for_apply, &ops)
        })
        .await??;

        let mut summary = apply_result.summary;
        summary.op_kinds = vec!["transform_batch".to_string()];
        set_recalc_needed_flag(&mut summary, fork_ctx.recalc_needed);

        let staged_op = StagedOp {
            kind: "transform_batch".to_string(),
            payload: serde_json::to_value(TransformBatchStagedPayload {
                ops: ops_to_apply.clone(),
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

        Ok(TransformBatchResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: Some(change_id),
            ops_applied: apply_result.ops_applied,
            summary,
            formula_parse_diagnostics,
        })
    } else {
        let apply_result = tokio::task::spawn_blocking({
            let ops = ops_to_apply.clone();
            let work_path = work_path.clone();
            move || apply_transform_ops_to_file(&work_path, &ops)
        })
        .await??;

        let mut summary = apply_result.summary;
        summary.op_kinds = vec!["transform_batch".to_string()];

        registry.with_fork_mut(&params.fork_id, |ctx| {
            ctx.recalc_needed = true;
            Ok(())
        })?;
        set_recalc_needed_flag(&mut summary, true);

        let _ = state.close_workbook(&fork_workbook_id);

        Ok(TransformBatchResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: None,
            ops_applied: apply_result.ops_applied,
            summary,
            formula_parse_diagnostics,
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StyleBatchParams {
    pub fork_id: String,
    pub ops: Vec<StyleOp>,
    #[serde(default)]
    pub mode: Option<BatchMode>, // preview|apply (default apply)
    pub label: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StyleBatchParamsInput {
    pub fork_id: String,
    pub ops: Vec<StyleOpInput>,
    #[serde(default)]
    pub mode: Option<BatchMode>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StyleOpInput {
    op: StyleOp,
    shorthand_used: bool,
    fill_color_used: bool,
    color_alpha_defaulted: bool,
}

impl From<StyleOp> for StyleOpInput {
    fn from(op: StyleOp) -> Self {
        Self {
            op,
            shorthand_used: false,
            fill_color_used: false,
            color_alpha_defaulted: false,
        }
    }
}

impl<'de> Deserialize<'de> for StyleOpInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let Some(obj) = value.as_object_mut() else {
            return Err(de::Error::custom("style op must be an object"));
        };

        let mut shorthand_used = false;
        let mut fill_color_used = false;
        let mut color_alpha_defaulted = false;

        if obj.get("target").is_none()
            && let Some(range) = obj.remove("range")
        {
            shorthand_used = true;
            obj.insert(
                "target".to_string(),
                serde_json::json!({ "kind": "range", "range": range }),
            );
        }

        if obj.get("patch").is_none()
            && let Some(style) = obj.remove("style")
        {
            shorthand_used = true;
            obj.insert("patch".to_string(), style);
        }

        if let Some(patch_value) = obj.remove("patch") {
            let patch_input: StylePatchInput =
                serde_json::from_value(patch_value).map_err(de::Error::custom)?;
            let (patch, used_fill_color, alpha_defaulted) =
                normalize_style_patch_input(patch_input);
            if used_fill_color {
                fill_color_used = true;
            }
            if alpha_defaulted {
                color_alpha_defaulted = true;
            }
            obj.insert(
                "patch".to_string(),
                serde_json::to_value(patch).map_err(de::Error::custom)?,
            );
        }

        let op = serde_json::from_value(value).map_err(de::Error::custom)?;
        Ok(StyleOpInput {
            op,
            shorthand_used,
            fill_color_used,
            color_alpha_defaulted,
        })
    }
}

impl schemars::JsonSchema for StyleOpInput {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "StyleOp".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        StyleOp::json_schema(generator)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct StylePatchInput {
    #[serde(default)]
    pub font: Option<Option<FontPatch>>,
    #[serde(default)]
    pub fill: Option<Option<FillPatchInput>>,
    #[serde(default)]
    pub borders: Option<Option<BordersPatch>>,
    #[serde(default)]
    pub alignment: Option<Option<AlignmentPatch>>,
    #[serde(default)]
    pub number_format: Option<Option<NumberFormatPatchInput>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum NumberFormatPatchInput {
    FormatCode(String),
    Shorthand(NumberFormatShorthandInput),
}

#[derive(Debug, Clone, Deserialize)]
struct NumberFormatShorthandInput {
    pub kind: NumberFormatKind,
    #[serde(default)]
    pub format_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NumberFormatKind {
    Currency,
    Percent,
    DateIso,
    Accounting,
    Integer,
}

fn number_format_kind_to_format_code(kind: &NumberFormatKind) -> &'static str {
    match kind {
        NumberFormatKind::Currency => "$#,##0.00",
        NumberFormatKind::Percent => "0.00%",
        NumberFormatKind::DateIso => "yyyy-mm-dd",
        NumberFormatKind::Accounting => "_($* #,##0.00_)",
        NumberFormatKind::Integer => "0",
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum FillPatchInput {
    Canonical(FillPatch),
    Color(FillColorPatch),
}

#[derive(Debug, Clone, Deserialize)]
struct FillColorPatch {
    color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StyleOp {
    pub sheet_name: String,
    pub target: StyleTarget,
    pub patch: StylePatch,
    #[serde(default)]
    pub op_mode: Option<crate::styles::StylePatchMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StyleTarget {
    Range { range: String },
    Region { region_id: u32 },
    Cells { cells: Vec<String> },
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StyleBatchResponse {
    pub fork_id: String,
    pub mode: String,
    pub change_id: Option<String>,
    pub ops_applied: usize,
    pub summary: ChangeSummary,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ColumnSizeBatchParamsInput {
    pub fork_id: String,
    pub sheet_name: String,
    pub ops: Vec<ColumnSizeOpInput>,
    pub mode: Option<BatchMode>, // preview|apply (default apply)
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ColumnTarget {
    Columns { range: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ColumnSizeSpec {
    Auto {
        #[serde(default)]
        min_width_chars: Option<f64>,
        #[serde(default)]
        max_width_chars: Option<f64>,
    },
    Width {
        width_chars: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ColumnSizeOp {
    pub target: ColumnTarget,
    pub size: ColumnSizeSpec,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ColumnSizeOpInput {
    Canonical(ColumnSizeOp),
    Shorthand { range: String, size: ColumnSizeSpec },
}

impl From<ColumnSizeOp> for ColumnSizeOpInput {
    fn from(value: ColumnSizeOp) -> Self {
        Self::Canonical(value)
    }
}

#[derive(Debug, Clone)]
struct ColumnSizeBatchParams {
    fork_id: String,
    sheet_name: String,
    ops: Vec<ColumnSizeOp>,
    mode: Option<BatchMode>,
    label: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ColumnSizeBatchResponse {
    pub fork_id: String,
    pub sheet_name: String,
    pub mode: String,
    pub change_id: Option<String>,
    pub ops_applied: usize,
    pub summary: ChangeSummary,
}

#[derive(Debug, Serialize, Deserialize)]
struct ColumnSizeBatchStagedPayload {
    sheet_name: String,
    ops: Vec<ColumnSizeOp>,
}

fn normalize_column_size_batch(
    params: ColumnSizeBatchParamsInput,
) -> Result<(ColumnSizeBatchParams, Vec<crate::model::Warning>)> {
    let mut warnings = Vec::new();
    let mut ops = Vec::with_capacity(params.ops.len());

    for entry in params.ops {
        match entry {
            ColumnSizeOpInput::Canonical(op) => ops.push(op),
            ColumnSizeOpInput::Shorthand { range, size } => {
                warnings.push(crate::model::Warning {
                    code: "WARN_COLUMN_SHORTHAND_TARGET".to_string(),
                    message: "Used range shorthand; prefer target:{kind:'columns',range:'A:C'}"
                        .to_string(),
                });
                ops.push(ColumnSizeOp {
                    target: ColumnTarget::Columns { range },
                    size,
                });
            }
        }
    }

    Ok((
        ColumnSizeBatchParams {
            fork_id: params.fork_id,
            sheet_name: params.sheet_name,
            ops,
            mode: params.mode,
            label: params.label,
        },
        warnings,
    ))
}

pub(crate) fn normalize_column_size_payload(
    sheet_name: String,
    ops: Vec<ColumnSizeOpInput>,
) -> Result<(Vec<ColumnSizeOp>, Vec<Warning>)> {
    let (params, warnings) = normalize_column_size_batch(ColumnSizeBatchParamsInput {
        fork_id: String::new(),
        sheet_name,
        ops,
        mode: None,
        label: None,
    })?;
    Ok((params.ops, warnings))
}

pub async fn column_size_batch(
    state: Arc<AppState>,
    params: ColumnSizeBatchParamsInput,
) -> Result<ColumnSizeBatchResponse> {
    let (params, warnings) = normalize_column_size_batch(params)?;
    let warning_messages: Vec<String> = warnings
        .into_iter()
        .map(|warning| format!("{}: {}", warning.code, warning.message))
        .collect();
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;
    let work_path = fork_ctx.work_path.clone();

    let fork_workbook_id = WorkbookId(params.fork_id.clone());
    let workbook = state.open_workbook(&fork_workbook_id).await?;
    let _ = workbook.with_sheet(&params.sheet_name, |_| Ok::<_, anyhow::Error>(()))?;

    let mode = params.mode.unwrap_or_default();

    if mode.is_preview() {
        let change_id = make_short_random_id("chg", 12);
        let snapshot_path = stage_snapshot_path(&params.fork_id, &change_id);
        fs::create_dir_all(snapshot_path.parent().unwrap())?;
        fs::copy(&work_path, &snapshot_path)?;

        let snapshot_path_for_apply = snapshot_path.clone();
        let apply_result = tokio::task::spawn_blocking({
            let ops = params.ops.clone();
            let sheet_name = params.sheet_name.clone();
            move || apply_column_size_ops_to_file(&snapshot_path_for_apply, &sheet_name, &ops)
        })
        .await??;

        let mut summary = apply_result.summary;
        summary.op_kinds = vec!["column_size_batch".to_string()];
        summary.warnings.extend(warning_messages.clone());
        set_recalc_needed_flag(&mut summary, fork_ctx.recalc_needed);

        let staged_op = StagedOp {
            kind: "column_size_batch".to_string(),
            payload: serde_json::to_value(ColumnSizeBatchStagedPayload {
                sheet_name: params.sheet_name.clone(),
                ops: params.ops.clone(),
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

        Ok(ColumnSizeBatchResponse {
            fork_id: params.fork_id,
            sheet_name: params.sheet_name,
            mode: mode.as_str().to_string(),
            change_id: Some(change_id),
            ops_applied: apply_result.ops_applied,
            summary,
        })
    } else {
        let apply_result = tokio::task::spawn_blocking({
            let ops = params.ops.clone();
            let sheet_name = params.sheet_name.clone();
            let work_path = work_path.clone();
            move || apply_column_size_ops_to_file(&work_path, &sheet_name, &ops)
        })
        .await??;

        let mut summary = apply_result.summary;
        summary.op_kinds = vec!["column_size_batch".to_string()];
        summary.warnings.extend(warning_messages);
        set_recalc_needed_flag(&mut summary, fork_ctx.recalc_needed);

        let _ = state.close_workbook(&fork_workbook_id);

        Ok(ColumnSizeBatchResponse {
            fork_id: params.fork_id,
            sheet_name: params.sheet_name,
            mode: mode.as_str().to_string(),
            change_id: None,
            ops_applied: apply_result.ops_applied,
            summary,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StyleBatchStagedPayload {
    ops: Vec<StyleOp>,
}

pub fn normalize_style_batch(
    input: StyleBatchParamsInput,
) -> Result<(StyleBatchParams, Vec<Warning>)> {
    let mut warnings = Vec::new();
    let mut ops = Vec::with_capacity(input.ops.len());

    for op_input in input.ops {
        if op_input.shorthand_used {
            warnings.push(Warning {
                code: "WARN_STYLE_SHORTHAND".to_string(),
                message: "Normalized style op shorthand to canonical form".to_string(),
            });
        }
        if op_input.fill_color_used {
            warnings.push(Warning {
                code: "WARN_FILL_COLOR".to_string(),
                message: "Normalized fill color shorthand to pattern fill".to_string(),
            });
        }
        if op_input.color_alpha_defaulted {
            warnings.push(Warning {
                code: "WARN_COLOR_ALPHA_DEFAULT".to_string(),
                message: "Normalized RGB hex to ARGB with default alpha".to_string(),
            });
        }
        ops.push(op_input.op);
    }

    Ok((
        StyleBatchParams {
            fork_id: input.fork_id,
            ops,
            mode: input.mode,
            label: input.label,
        },
        warnings,
    ))
}

fn normalize_style_patch_input(input: StylePatchInput) -> (StylePatch, bool, bool) {
    let mut fill_color_used = false;
    let mut color_alpha_defaulted = false;
    let fill = match input.fill {
        None => None,
        Some(None) => Some(None),
        Some(Some(fill_input)) => {
            let normalized = match fill_input {
                FillPatchInput::Canonical(fill) => fill,
                FillPatchInput::Color(color) => {
                    fill_color_used = true;
                    FillPatch::Pattern(PatternFillPatch {
                        pattern_type: Some(Some("solid".to_string())),
                        foreground_color: Some(Some(color.color)),
                        background_color: None,
                    })
                }
            };
            Some(Some(normalized))
        }
    };

    let number_format: Option<Option<String>> = match input.number_format {
        None => None,
        Some(None) => Some(None),
        Some(Some(nf)) => match nf {
            NumberFormatPatchInput::FormatCode(code) => Some(Some(code)),
            NumberFormatPatchInput::Shorthand(sh) => {
                if let Some(code) = sh.format_code {
                    Some(Some(code))
                } else {
                    Some(Some(
                        number_format_kind_to_format_code(&sh.kind).to_string(),
                    ))
                }
            }
        },
    };

    let mut patch = StylePatch {
        font: input.font,
        fill,
        borders: input.borders,
        alignment: input.alignment,
        number_format,
    };
    normalize_style_patch_colors(&mut patch, &mut color_alpha_defaulted);

    (patch, fill_color_used, color_alpha_defaulted)
}

fn normalize_style_patch_colors(patch: &mut StylePatch, alpha_defaulted: &mut bool) {
    if let Some(Some(font)) = patch.font.as_mut() {
        normalize_color_option(&mut font.color, alpha_defaulted);
    }

    if let Some(Some(fill)) = patch.fill.as_mut() {
        normalize_fill_colors(fill, alpha_defaulted);
    }

    if let Some(Some(borders)) = patch.borders.as_mut() {
        normalize_border_side_color(&mut borders.left, alpha_defaulted);
        normalize_border_side_color(&mut borders.right, alpha_defaulted);
        normalize_border_side_color(&mut borders.top, alpha_defaulted);
        normalize_border_side_color(&mut borders.bottom, alpha_defaulted);
        normalize_border_side_color(&mut borders.diagonal, alpha_defaulted);
        normalize_border_side_color(&mut borders.vertical, alpha_defaulted);
        normalize_border_side_color(&mut borders.horizontal, alpha_defaulted);
    }
}

fn normalize_fill_colors(fill: &mut FillPatch, alpha_defaulted: &mut bool) {
    match fill {
        FillPatch::Pattern(pattern) => {
            normalize_color_option(&mut pattern.foreground_color, alpha_defaulted);
            normalize_color_option(&mut pattern.background_color, alpha_defaulted);
        }
        FillPatch::Gradient(gradient) => {
            if let Some(stops) = gradient.stops.as_mut() {
                for stop in stops {
                    if let Some((normalized, defaulted)) =
                        crate::styles::normalize_color_hex(&stop.color)
                    {
                        stop.color = normalized;
                        if defaulted {
                            *alpha_defaulted = true;
                        }
                    }
                }
            }
        }
    }
}

fn normalize_border_side_color(
    side: &mut Option<Option<crate::model::BorderSidePatch>>,
    alpha_defaulted: &mut bool,
) {
    if let Some(Some(side_patch)) = side.as_mut() {
        normalize_color_option(&mut side_patch.color, alpha_defaulted);
    }
}

fn normalize_color_option(value: &mut Option<Option<String>>, alpha_defaulted: &mut bool) {
    let Some(Some(color)) = value.as_mut() else {
        return;
    };
    if let Some((normalized, defaulted)) = crate::styles::normalize_color_hex(color) {
        *color = normalized;
        if defaulted {
            *alpha_defaulted = true;
        }
    }
}

pub(crate) fn resolve_style_ops_for_workbook(
    workbook: &crate::workbook::WorkbookContext,
    ops: &[StyleOp],
) -> Result<Vec<StyleOp>> {
    let mut resolved_ops = Vec::with_capacity(ops.len());
    for op in ops {
        let mut resolved = op.clone();
        if let StyleTarget::Region { region_id } = &op.target {
            let metrics = workbook.get_sheet_metrics(&op.sheet_name)?;
            let regions = metrics.detected_regions();
            let region = regions.iter().find(|r| r.id == *region_id).ok_or_else(|| {
                anyhow!(
                    "region_id {} not found on sheet '{}'",
                    region_id,
                    op.sheet_name
                )
            })?;
            resolved.target = StyleTarget::Range {
                range: region.bounds.clone(),
            };
        }
        resolved_ops.push(resolved);
    }
    Ok(resolved_ops)
}

pub async fn style_batch(
    state: Arc<AppState>,
    params: StyleBatchParamsInput,
) -> Result<StyleBatchResponse> {
    let (params, warnings) = normalize_style_batch(params)?;
    let warning_messages: Vec<String> = warnings
        .into_iter()
        .map(|warning| format!("{}: {}", warning.code, warning.message))
        .collect();
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;
    let work_path = fork_ctx.work_path.clone();

    // Resolve any region targets against current fork regions.
    let fork_workbook_id = WorkbookId(params.fork_id.clone());
    let workbook = state.open_workbook(&fork_workbook_id).await?;
    let resolved_ops = resolve_style_ops_for_workbook(&workbook, &params.ops)?;

    let mode = params.mode.unwrap_or_default();

    if mode.is_preview() {
        let change_id = make_short_random_id("chg", 12);
        let snapshot_path = stage_snapshot_path(&params.fork_id, &change_id);
        fs::create_dir_all(snapshot_path.parent().unwrap())?;
        fs::copy(&work_path, &snapshot_path)?;

        let snapshot_path_for_apply = snapshot_path.clone();
        let apply_result = tokio::task::spawn_blocking({
            let ops = resolved_ops.clone();
            move || apply_style_ops_to_file(&snapshot_path_for_apply, &ops)
        })
        .await??;

        let mut summary = apply_result.summary;
        summary.op_kinds = vec!["style_batch".to_string()];
        summary.warnings.extend(warning_messages.clone());
        set_recalc_needed_flag(&mut summary, fork_ctx.recalc_needed);

        let staged_op = StagedOp {
            kind: "style_batch".to_string(),
            payload: serde_json::to_value(StyleBatchStagedPayload {
                ops: resolved_ops.clone(),
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

        Ok(StyleBatchResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: Some(change_id),
            ops_applied: resolved_ops.len(),
            summary,
        })
    } else {
        let apply_result = tokio::task::spawn_blocking({
            let ops = resolved_ops.clone();
            let work_path = work_path.clone();
            move || apply_style_ops_to_file(&work_path, &ops)
        })
        .await??;

        let mut summary = apply_result.summary;
        summary.op_kinds = vec!["style_batch".to_string()];
        summary.warnings.extend(warning_messages);
        set_recalc_needed_flag(&mut summary, fork_ctx.recalc_needed);

        let _ = state.close_workbook(&fork_workbook_id);

        Ok(StyleBatchResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: None,
            ops_applied: apply_result.ops_applied,
            summary,
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ApplyFormulaPatternParams {
    pub fork_id: String,
    pub sheet_name: String,
    pub target_range: String,
    pub anchor_cell: String,
    pub base_formula: String,
    #[serde(default)]
    pub fill_direction: Option<FillDirection>, // down|right|both (default both)
    #[serde(default)]
    pub relative_mode: Option<FormulaRelativeMode>, // excel|abs_cols|abs_rows
    #[serde(default)]
    pub mode: Option<BatchMode>, // preview|apply (default apply)
    pub label: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ApplyFormulaPatternResponse {
    pub fork_id: String,
    pub sheet_name: String,
    pub target_range: String,
    pub mode: String,
    pub change_id: Option<String>,
    pub cells_filled: u64,
    pub summary: ChangeSummary,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApplyFormulaPatternStagedPayload {
    sheet_name: String,
    target_range: String,
    anchor_cell: String,
    base_formula: String,
    fill_direction: Option<FillDirection>,
    relative_mode: Option<FormulaRelativeMode>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ApplyFormulaPatternOpInput {
    pub sheet_name: String,
    pub target_range: String,
    pub anchor_cell: String,
    pub base_formula: String,
    #[serde(default)]
    pub fill_direction: Option<FillDirection>,
    #[serde(default)]
    pub relative_mode: Option<FormulaRelativeMode>,
}

pub async fn apply_formula_pattern(
    state: Arc<AppState>,
    params: ApplyFormulaPatternParams,
) -> Result<ApplyFormulaPatternResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;
    let work_path = fork_ctx.work_path.clone();

    let bounds = parse_range_bounds(&params.target_range)?;
    let (anchor_col, anchor_row) = parse_cell_ref(&params.anchor_cell)?;
    let fill_direction = params.fill_direction.unwrap_or_default();
    validate_formula_pattern_bounds(&bounds, anchor_col, anchor_row, fill_direction)?;

    let fork_workbook_id = WorkbookId(params.fork_id.clone());
    let workbook = state.open_workbook(&fork_workbook_id).await?;
    let _ = workbook.with_sheet(&params.sheet_name, |_| Ok::<_, anyhow::Error>(()))?;

    let relative_mode_param = params.relative_mode.unwrap_or_default();
    let relative_mode: RelativeMode = relative_mode_param.into();
    let mode = params.mode.unwrap_or_default();

    if mode.is_preview() {
        let change_id = make_short_random_id("chg", 12);
        let snapshot_path = stage_snapshot_path(&params.fork_id, &change_id);
        fs::create_dir_all(snapshot_path.parent().unwrap())?;
        fs::copy(&work_path, &snapshot_path)?;

        let sheet_name = params.sheet_name.clone();
        let target_range = params.target_range.clone();
        let anchor_cell = params.anchor_cell.clone();
        let base_formula = params.base_formula.clone();
        let fill_direction = Some(fill_direction);
        let relative_mode_param = Some(relative_mode_param);
        let snapshot_for_apply = snapshot_path.clone();
        let sheet_name_for_apply = sheet_name.clone();
        let target_range_for_apply = target_range.clone();
        let base_formula_for_apply = base_formula.clone();

        let apply_result = tokio::task::spawn_blocking(move || {
            apply_formula_pattern_to_file(
                &snapshot_for_apply,
                &sheet_name_for_apply,
                &target_range_for_apply,
                anchor_col,
                anchor_row,
                &base_formula_for_apply,
                relative_mode,
            )
        })
        .await??;

        let mut summary = apply_result.summary;
        summary.op_kinds = vec!["apply_formula_pattern".to_string()];
        set_recalc_needed_flag(&mut summary, fork_ctx.recalc_needed);

        let staged_op = StagedOp {
            kind: "apply_formula_pattern".to_string(),
            payload: serde_json::to_value(ApplyFormulaPatternStagedPayload {
                sheet_name: sheet_name.clone(),
                target_range: target_range.clone(),
                anchor_cell: anchor_cell.clone(),
                base_formula: base_formula.clone(),
                fill_direction,
                relative_mode: relative_mode_param,
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

        Ok(ApplyFormulaPatternResponse {
            fork_id: params.fork_id,
            sheet_name,
            target_range,
            mode: mode.as_str().to_string(),
            change_id: Some(change_id),
            cells_filled: apply_result.cells_filled,
            summary,
        })
    } else {
        let sheet_name = params.sheet_name.clone();
        let target_range = params.target_range.clone();
        let base_formula = params.base_formula.clone();
        let sheet_name_for_apply = sheet_name.clone();
        let target_range_for_apply = target_range.clone();
        let base_formula_for_apply = base_formula.clone();
        let apply_result = tokio::task::spawn_blocking(move || {
            apply_formula_pattern_to_file(
                &work_path,
                &sheet_name_for_apply,
                &target_range_for_apply,
                anchor_col,
                anchor_row,
                &base_formula_for_apply,
                relative_mode,
            )
        })
        .await??;

        let mut summary = apply_result.summary;
        summary.op_kinds = vec!["apply_formula_pattern".to_string()];

        registry.with_fork_mut(&params.fork_id, |ctx| {
            ctx.recalc_needed = true;
            Ok(())
        })?;
        set_recalc_needed_flag(&mut summary, true);

        let _ = state.close_workbook(&fork_workbook_id);

        Ok(ApplyFormulaPatternResponse {
            fork_id: params.fork_id,
            sheet_name,
            target_range,
            mode: mode.as_str().to_string(),
            change_id: None,
            cells_filled: apply_result.cells_filled,
            summary,
        })
    }
}

struct FormulaPatternApplyResult {
    cells_filled: u64,
    summary: ChangeSummary,
}

fn apply_formula_pattern_to_file(
    path: &Path,
    sheet_name: &str,
    target_range: &str,
    anchor_col: u32,
    anchor_row: u32,
    base_formula: &str,
    relative_mode: RelativeMode,
) -> Result<FormulaPatternApplyResult> {
    let mut book = crate::xlsx_import::read(path)?;
    let result = apply_formula_pattern_to_workbook(
        &mut book,
        sheet_name,
        target_range,
        anchor_col,
        anchor_row,
        base_formula,
        relative_mode,
    )?;
    crate::xlsx_export::write(&book, path)?;
    Ok(result)
}

fn apply_formula_pattern_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    target_range: &str,
    anchor_col: u32,
    anchor_row: u32,
    base_formula: &str,
    relative_mode: RelativeMode,
) -> Result<FormulaPatternApplyResult> {
    let ast = parse_base_formula(base_formula)?;
    let bounds = parse_range_bounds(target_range)?;
    let sheet = book
        .get_sheet_by_name_mut(sheet_name).ok()
        .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;

    let mut cells_filled: u64 = 0;
    for row in bounds.min_row..=bounds.max_row {
        for col in bounds.min_col..=bounds.max_col {
            let delta_col = col as i32 - anchor_col as i32;
            let delta_row = row as i32 - anchor_row as i32;
            let shifted = shift_formula_ast(&ast, delta_col, delta_row, relative_mode)?;
            let shifted_for_umya = shifted.strip_prefix('=').unwrap_or(&shifted);
            let addr = crate::utils::cell_address(col, row);
            let cell = sheet.get_cell_mut(addr.as_str());
            cell.set_formula(shifted_for_umya.to_string());
            cell.set_formula_result_default("");
            cells_filled += 1;
        }
    }

    let mut counts = BTreeMap::new();
    counts.insert("cells_filled".to_string(), cells_filled);

    let summary = ChangeSummary {
        op_kinds: vec!["apply_formula_pattern".to_string()],
        affected_sheets: vec![sheet_name.to_string()],
        affected_bounds: vec![target_range.to_string()],
        counts,
        warnings: Vec::new(),
        ..Default::default()
    };

    Ok(FormulaPatternApplyResult {
        cells_filled,
        summary,
    })
}

pub(crate) struct FormulaPatternBatchApplyResult {
    pub(crate) ops_applied: usize,
    pub(crate) summary: ChangeSummary,
}

pub(crate) fn apply_formula_pattern_ops_to_file(
    path: &Path,
    ops: &[ApplyFormulaPatternOpInput],
) -> Result<FormulaPatternBatchApplyResult> {
    let mut book = crate::xlsx_import::read(path)?;
    let result = apply_formula_pattern_ops_to_workbook(&mut book, ops)?;
    crate::xlsx_export::write(&book, path)?;
    Ok(result)
}

pub(crate) fn apply_formula_pattern_ops_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    ops: &[ApplyFormulaPatternOpInput],
) -> Result<FormulaPatternBatchApplyResult> {
    struct PreparedFormulaPatternOp {
        sheet_name: String,
        target_range: String,
        anchor_col: u32,
        anchor_row: u32,
        base_formula: String,
        relative_mode: RelativeMode,
    }

    let mut prepared_ops = Vec::with_capacity(ops.len());
    let mut affected_sheets: BTreeSet<String> = BTreeSet::new();
    let mut affected_bounds: Vec<String> = Vec::with_capacity(ops.len());

    for op in ops {
        let bounds = parse_range_bounds(&op.target_range)?;
        let (anchor_col, anchor_row) = parse_cell_ref(&op.anchor_cell)?;
        let fill_direction = op.fill_direction.unwrap_or_default();
        validate_formula_pattern_bounds(&bounds, anchor_col, anchor_row, fill_direction)?;
        parse_base_formula(&op.base_formula)?;

        let relative_mode: RelativeMode = op.relative_mode.unwrap_or_default().into();

        affected_sheets.insert(op.sheet_name.clone());
        affected_bounds.push(op.target_range.clone());

        prepared_ops.push(PreparedFormulaPatternOp {
            sheet_name: op.sheet_name.clone(),
            target_range: op.target_range.clone(),
            anchor_col,
            anchor_row,
            base_formula: op.base_formula.clone(),
            relative_mode,
        });
    }

    let mut cells_filled = 0u64;
    for op in prepared_ops {
        let result = apply_formula_pattern_to_workbook(
            book,
            &op.sheet_name,
            &op.target_range,
            op.anchor_col,
            op.anchor_row,
            &op.base_formula,
            op.relative_mode,
        )?;
        cells_filled += result.cells_filled;
    }

    let mut counts = BTreeMap::new();
    counts.insert("cells_filled".to_string(), cells_filled);

    Ok(FormulaPatternBatchApplyResult {
        ops_applied: ops.len(),
        summary: ChangeSummary {
            op_kinds: vec!["apply_formula_pattern".to_string()],
            affected_sheets: affected_sheets.into_iter().collect(),
            affected_bounds,
            counts,
            warnings: Vec::new(),
            ..Default::default()
        },
    })
}

fn validate_formula_pattern_bounds(
    bounds: &ScreenshotBounds,
    anchor_col: u32,
    anchor_row: u32,
    fill_direction: FillDirection,
) -> Result<()> {
    if anchor_col < bounds.min_col
        || anchor_col > bounds.max_col
        || anchor_row < bounds.min_row
        || anchor_row > bounds.max_row
    {
        let bounds_range = format!(
            "{}:{}",
            crate::utils::cell_address(bounds.min_col, bounds.min_row),
            crate::utils::cell_address(bounds.max_col, bounds.max_row)
        );
        bail!(
            "anchor_cell must be inside target_range (anchor {} not within {})",
            crate::utils::cell_address(anchor_col, anchor_row),
            bounds_range
        );
    }

    if bounds.min_col != anchor_col || bounds.min_row != anchor_row {
        bail!("target_range must start at anchor_cell (anchor should be top-left of fill range)");
    }

    match fill_direction {
        FillDirection::Down => {
            if bounds.min_col != bounds.max_col {
                bail!("fill_direction=down requires a single-column target_range");
            }
        }
        FillDirection::Right => {
            if bounds.min_row != bounds.max_row {
                bail!("fill_direction=right requires a single-row target_range");
            }
        }
        FillDirection::Both => {}
    }
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StructureBatchParams {
    pub fork_id: String,
    pub ops: Vec<StructureOp>,
    #[serde(default)]
    pub mode: Option<BatchMode>, // preview|apply (default apply)
    pub label: Option<String>,
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
    #[serde(default)]
    pub impact_report: Option<bool>,
    #[serde(default)]
    pub show_formula_delta: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StructureBatchParamsInput {
    pub fork_id: String,
    pub ops: Vec<StructureOpInput>,
    #[serde(default)]
    pub mode: Option<BatchMode>,
    pub label: Option<String>,
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
    /// Request a structural impact report (shifted spans, absolute-ref warnings).
    /// Only honoured in preview mode.
    #[serde(default)]
    pub impact_report: Option<bool>,
    /// Request before/after formula delta samples.
    /// Only honoured in preview mode.
    #[serde(default)]
    pub show_formula_delta: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct StructureOpInput {
    op: StructureOp,
    alias_used: bool,
}

impl From<StructureOp> for StructureOpInput {
    fn from(op: StructureOp) -> Self {
        Self {
            op,
            alias_used: false,
        }
    }
}

impl<'de> Deserialize<'de> for StructureOpInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let Some(obj) = value.as_object_mut() else {
            return Err(de::Error::custom("structure op must be an object"));
        };

        let mut alias_used = false;
        let kind_value = if let Some(kind) = obj.get("kind") {
            kind.clone()
        } else if let Some(op) = obj.remove("op") {
            alias_used = true;
            op
        } else {
            return Err(de::Error::custom("structure op requires 'kind' or 'op'"));
        };

        let Some(kind_str) = kind_value.as_str() else {
            return Err(de::Error::custom("structure op kind must be a string"));
        };

        let normalized_kind = if kind_str == "add_sheet" {
            alias_used = true;
            "create_sheet"
        } else {
            kind_str
        };

        obj.insert(
            "kind".to_string(),
            serde_json::Value::String(normalized_kind.to_string()),
        );

        let op = serde_json::from_value(value).map_err(de::Error::custom)?;
        Ok(StructureOpInput { op, alias_used })
    }
}

impl schemars::JsonSchema for StructureOpInput {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "StructureOp".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        StructureOp::json_schema(generator)
    }
}

pub fn normalize_structure_batch(
    input: StructureBatchParamsInput,
) -> Result<(StructureBatchParams, Vec<Warning>)> {
    let mut warnings = Vec::new();
    let mut ops = Vec::with_capacity(input.ops.len());

    for op_input in input.ops {
        if op_input.alias_used {
            warnings.push(Warning {
                code: "WARN_ALIAS_KIND".to_string(),
                message: "Normalized structure op alias to canonical kind".to_string(),
            });
        }
        ops.push(op_input.op);
    }

    Ok((
        StructureBatchParams {
            fork_id: input.fork_id,
            ops,
            mode: input.mode,
            label: input.label,
            formula_parse_policy: input.formula_parse_policy,
            impact_report: input.impact_report,
            show_formula_delta: input.show_formula_delta,
        },
        warnings,
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StructureOp {
    MergeCells {
        sheet_name: String,
        target_range: String,
    },
    UnmergeCells {
        sheet_name: String,
        target_range: String,
    },
    InsertRows {
        sheet_name: String,
        at_row: u32,
        count: u32,
        /// When true, simple SUM(Ax:Ay) formulas in the row directly adjacent
        /// (below) to the insertion band are expanded to include the new rows.
        /// Only unambiguous single-range SUM patterns are expanded; complex or
        /// ambiguous formulas produce a warning and are left untouched.
        #[serde(default)]
        expand_adjacent_sums: bool,
    },
    CloneRow {
        sheet_name: String,
        /// 1-based row to use as the template.
        source_row: u32,
        /// 1-based row position at which to insert new rows.
        insert_at: u32,
        /// Number of copies to insert (default 1).
        #[serde(default = "default_clone_count")]
        count: u32,
        /// When true, expand adjacent SUM formulas below the insertion band.
        #[serde(default)]
        expand_adjacent_sums: bool,
    },
    DeleteRows {
        sheet_name: String,
        start_row: u32,
        count: u32,
    },
    InsertCols {
        sheet_name: String,
        at_col: String,
        count: u32,
    },
    DeleteCols {
        sheet_name: String,
        start_col: String,
        count: u32,
    },
    RenameSheet {
        old_name: String,
        new_name: String,
    },
    CreateSheet {
        name: String,
        #[serde(default)]
        position: Option<u32>,
    },
    DeleteSheet {
        name: String,
    },
    CopyRange {
        sheet_name: String,
        #[serde(default)]
        dest_sheet_name: Option<String>,
        src_range: String,
        dest_anchor: String,
        include_styles: bool,
        include_formulas: bool,
    },
    MoveRange {
        sheet_name: String,
        #[serde(default)]
        dest_sheet_name: Option<String>,
        src_range: String,
        dest_anchor: String,
        include_styles: bool,
        include_formulas: bool,
    },
}

fn structure_ops_require_recalc(ops: &[StructureOp]) -> bool {
    ops.iter().any(|op| {
        matches!(
            op,
            StructureOp::InsertRows { .. }
                | StructureOp::DeleteRows { .. }
                | StructureOp::InsertCols { .. }
                | StructureOp::DeleteCols { .. }
                | StructureOp::RenameSheet { .. }
                | StructureOp::CloneRow { .. }
                | StructureOp::CopyRange {
                    include_formulas: true,
                    ..
                }
                | StructureOp::MoveRange {
                    include_formulas: true,
                    ..
                }
        )
    })
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StructureBatchResponse {
    pub fork_id: String,
    pub mode: String,
    pub change_id: Option<String>,
    pub ops_applied: usize,
    pub summary: ChangeSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
    /// Structural impact report (only present when explicitly requested in preview mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impact_report: Option<crate::tools::structure_impact::StructureImpactReport>,
    /// Formula delta preview (only present when explicitly requested in preview mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_delta_preview: Option<Vec<crate::tools::structure_impact::FormulaDeltaItem>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StructureBatchStagedPayload {
    ops: Vec<StructureOp>,
    #[serde(default)]
    formula_parse_policy: Option<FormulaParsePolicy>,
}

pub async fn structure_batch(
    state: Arc<AppState>,
    params: StructureBatchParamsInput,
) -> Result<StructureBatchResponse> {
    let want_impact = params.impact_report.unwrap_or(false);
    let want_delta = params.show_formula_delta.unwrap_or(false);
    let (params, warnings) = normalize_structure_batch(params)?;
    let policy =
        params
            .formula_parse_policy
            .unwrap_or(FormulaParsePolicy::default_for_command_class(
                CommandClass::BatchWrite,
            ));
    let alias_warnings: Vec<String> = warnings
        .into_iter()
        .map(|warning| format!("{}: {}", warning.code, warning.message))
        .collect();
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;
    let work_path = fork_ctx.work_path.clone();

    let will_need_recalc = fork_ctx.recalc_needed || structure_ops_require_recalc(&params.ops);

    let mode = params.mode.unwrap_or_default();

    if mode.is_preview() {
        let change_id = make_short_random_id("chg", 12);
        let snapshot_path = stage_snapshot_path(&params.fork_id, &change_id);
        fs::create_dir_all(snapshot_path.parent().unwrap())?;
        fs::copy(&work_path, &snapshot_path)?;

        let snapshot_for_apply = snapshot_path.clone();
        let ops_for_apply = params.ops.clone();

        let apply_result = tokio::task::spawn_blocking(move || {
            apply_structure_ops_to_file(&snapshot_for_apply, &ops_for_apply, policy)
        })
        .await??;

        let mut summary = apply_result.summary;
        summary.op_kinds = vec!["structure_batch".to_string()];
        summary.warnings.extend(alias_warnings);
        set_recalc_needed_flag(&mut summary, fork_ctx.recalc_needed);
        // Best-effort preview diff size: compare current fork to preview snapshot.
        // This is intentionally summarized as a count to avoid large payloads.
        if let Ok(change_count) = tokio::task::spawn_blocking({
            let base_path = work_path.clone();
            let preview_path = snapshot_path.clone();
            move || {
                crate::core::diff::calculate_changeset(&base_path, &preview_path, None)
                    .map(|changes| changes.len() as u64)
            }
        })
        .await?
        {
            summary
                .counts
                .insert("preview_change_items".to_string(), change_count);
        } else {
            summary.warnings.push(
                "Preview diff computation failed; run get_changeset after applying to inspect changes."
                    .to_string(),
            );
        }

        let staged_op = StagedOp {
            kind: "structure_batch".to_string(),
            payload: serde_json::to_value(StructureBatchStagedPayload {
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

        // Compute impact report / formula delta preview if requested in preview mode.
        let (ir, fdp) = if want_impact || want_delta {
            let ops_for_impact = params.ops.clone();
            let base_path = work_path.clone();
            let (report, delta) = tokio::task::spawn_blocking(move || {
                crate::tools::structure_impact::compute_structure_impact(
                    &base_path,
                    &ops_for_impact,
                    want_delta,
                )
            })
            .await??;
            (if want_impact { Some(report) } else { None }, delta)
        } else {
            (None, None)
        };

        Ok(StructureBatchResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: Some(change_id),
            ops_applied: apply_result.ops_applied,
            summary,
            formula_parse_diagnostics: apply_result.formula_parse_diagnostics,
            impact_report: ir,
            formula_delta_preview: fdp,
        })
    } else {
        let ops_for_apply = params.ops.clone();
        let apply_result = tokio::task::spawn_blocking(move || {
            apply_structure_ops_to_file(&work_path, &ops_for_apply, policy)
        })
        .await??;

        let mut summary = apply_result.summary;
        summary.op_kinds = vec!["structure_batch".to_string()];
        summary.warnings.extend(alias_warnings);

        if will_need_recalc {
            registry.with_fork_mut(&params.fork_id, |ctx| {
                ctx.recalc_needed = true;
                Ok(())
            })?;
        }
        set_recalc_needed_flag(&mut summary, will_need_recalc);

        let fork_workbook_id = WorkbookId(params.fork_id.clone());
        let _ = state.close_workbook(&fork_workbook_id);

        Ok(StructureBatchResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: None,
            ops_applied: apply_result.ops_applied,
            summary,
            formula_parse_diagnostics: apply_result.formula_parse_diagnostics,
            impact_report: None,
            formula_delta_preview: None,
        })
    }
}

pub(crate) struct StructureApplyResult {
    pub(crate) ops_applied: usize,
    pub(crate) summary: ChangeSummary,
    pub(crate) formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
}

pub(crate) fn apply_structure_ops_to_file(
    path: &Path,
    ops: &[StructureOp],
    policy: FormulaParsePolicy,
) -> Result<StructureApplyResult> {
    let mut book = crate::xlsx_import::read(path)?;
    let mut result = apply_structure_ops_to_workbook(&mut book, ops, policy)?;
    crate::xlsx_export::write(&book, path)?;
    // Umya exposes parsed defined names, but malformed legacy workbook.xml entries may
    // survive outside that model. Keep the ZIP-level native guardrail in the path wrapper.
    let clamped = sanitize_workbook_xml_defined_name_rows(path)?;
    if clamped > 0 {
        result.summary.warnings.push(format!(
            "Clamped {clamped} workbook.xml defined name reference(s) to Excel max row 1048576 after structural edits."
        ));
    }
    Ok(result)
}

pub(crate) fn apply_structure_ops_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    ops: &[StructureOp],
    policy: FormulaParsePolicy,
) -> Result<StructureApplyResult> {
    let mut formula_parse_diagnostics_builder = FormulaParseDiagnosticsBuilder::new(policy);

    let mut affected_sheets: BTreeSet<String> = BTreeSet::new();
    let affected_bounds: Vec<String> = Vec::new();
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut warnings: Vec<String> = vec![
        "Structural edits may not fully rewrite formulas/named ranges like Excel. After apply, run recalculate and review get_changeset.".to_string(),
    ];

    for op in ops {
        match op {
            StructureOp::MergeCells {
                sheet_name,
                target_range,
            } => {
                let sheet = book
                    .get_sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                sheet.add_merge_cells(target_range.clone());
                affected_sheets.insert(sheet_name.clone());
                *counts.entry("cells_merged".to_string()).or_insert(0) += 1;
            }
            StructureOp::UnmergeCells {
                sheet_name,
                target_range,
            } => {
                let sheet = book
                    .get_sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;

                let original_len = sheet.get_merge_cells().len();
                if let Ok(target_bounds) = parse_range_bounds(target_range) {
                    let merges = sheet.get_merge_cells_mut();
                    merges.retain(|m| {
                        let m_range = m.get_range();
                        if let Ok(m_bounds) = parse_range_bounds(&m_range) {
                            !(m_bounds.min_col <= target_bounds.max_col
                                && m_bounds.max_col >= target_bounds.min_col
                                && m_bounds.min_row <= target_bounds.max_row
                                && m_bounds.max_row >= target_bounds.min_row)
                        } else {
                            true
                        }
                    });
                }
                *counts.entry("cells_unmerged".to_string()).or_insert(0) +=
                    (original_len - sheet.get_merge_cells().len()) as u64;

                affected_sheets.insert(sheet_name.clone());
            }
            StructureOp::InsertRows {
                sheet_name,
                at_row,
                count,
                expand_adjacent_sums,
            } => {
                if *at_row == 0 || *count == 0 {
                    bail!("insert_rows requires at_row>=1 and count>=1");
                }
                {
                    let sheet = book
                        .get_sheet_by_name_mut(sheet_name).ok()
                        .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                    let relocated_validations = transform_data_validations_for_structure_change(
                        sheet,
                        StructureAxis::Row,
                        StructureEdit::Insert {
                            at: *at_row,
                            count: *count,
                        },
                    )?;
                    sheet.insert_new_row(*at_row, *count);
                    replace_data_validations(sheet, relocated_validations);
                }
                rewrite_formulas_for_sheet_row_insert(
                    book,
                    sheet_name,
                    *at_row,
                    *count,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;
                rewrite_defined_name_formulas_for_sheet_row_insert(
                    book,
                    sheet_name,
                    *at_row,
                    *count,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;
                if *expand_adjacent_sums {
                    let (expansion_warnings, expanded_count) =
                        expand_adjacent_sum_formulas(book, sheet_name, *at_row, *count)?;
                    warnings.extend(expansion_warnings);
                    if expanded_count > 0 {
                        counts
                            .entry("sums_expanded".to_string())
                            .and_modify(|v| *v += expanded_count)
                            .or_insert(expanded_count);
                    }
                }
                affected_sheets.insert(sheet_name.clone());
                counts
                    .entry("rows_inserted".to_string())
                    .and_modify(|v| *v += *count as u64)
                    .or_insert(*count as u64);
            }
            StructureOp::CloneRow {
                sheet_name,
                source_row,
                insert_at,
                count,
                expand_adjacent_sums,
            } => {
                if *source_row == 0 || *insert_at == 0 || *count == 0 {
                    bail!("clone_row requires source_row>=1, insert_at>=1, and count>=1");
                }
                // Step 1: Capture template row cells before insertion (pre-shift).
                let template_cells = {
                    let sheet = book
                        .get_sheet_by_name_mut(sheet_name).ok()
                        .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                    capture_row_template(sheet, *source_row)?
                };
                if template_cells.is_empty() {
                    warnings.push(format!(
                        "WARN_CLONE_TEMPLATE_EMPTY: {} row {} has no template cells to clone.",
                        sheet_name, source_row
                    ));
                }

                // Step 2: Insert blank rows.
                {
                    let sheet = book
                        .get_sheet_by_name_mut(sheet_name).ok()
                        .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                    let relocated_validations = transform_data_validations_for_structure_change(
                        sheet,
                        StructureAxis::Row,
                        StructureEdit::Insert {
                            at: *insert_at,
                            count: *count,
                        },
                    )?;
                    sheet.insert_new_row(*insert_at, *count);
                    replace_data_validations(sheet, relocated_validations);
                }
                rewrite_formulas_for_sheet_row_insert(
                    book,
                    sheet_name,
                    *insert_at,
                    *count,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;
                rewrite_defined_name_formulas_for_sheet_row_insert(
                    book,
                    sheet_name,
                    *insert_at,
                    *count,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;

                // Step 3: Fill inserted rows from the template.
                {
                    let sheet = book
                        .get_sheet_by_name_mut(sheet_name).ok()
                        .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                    let clone_warnings = stamp_template_rows(
                        sheet,
                        &template_cells,
                        *source_row,
                        *insert_at,
                        *count,
                    )?;
                    warnings.extend(clone_warnings);
                }

                // Step 4: Optionally expand adjacent SUMs.
                if *expand_adjacent_sums {
                    let (expansion_warnings, expanded_count) =
                        expand_adjacent_sum_formulas(book, sheet_name, *insert_at, *count)?;
                    warnings.extend(expansion_warnings);
                    if expanded_count > 0 {
                        counts
                            .entry("sums_expanded".to_string())
                            .and_modify(|v| *v += expanded_count)
                            .or_insert(expanded_count);
                    }
                }

                affected_sheets.insert(sheet_name.clone());
                counts
                    .entry("rows_inserted".to_string())
                    .and_modify(|v| *v += *count as u64)
                    .or_insert(*count as u64);
                counts
                    .entry("rows_cloned".to_string())
                    .and_modify(|v| *v += *count as u64)
                    .or_insert(*count as u64);
            }
            StructureOp::DeleteRows {
                sheet_name,
                start_row,
                count,
            } => {
                if *start_row == 0 || *count == 0 {
                    bail!("delete_rows requires start_row>=1 and count>=1");
                }
                {
                    let sheet = book
                        .get_sheet_by_name_mut(sheet_name).ok()
                        .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                    let relocated_validations = transform_data_validations_for_structure_change(
                        sheet,
                        StructureAxis::Row,
                        StructureEdit::Delete {
                            start: *start_row,
                            count: *count,
                        },
                    )?;
                    sheet.remove_row(*start_row, *count);
                    replace_data_validations(sheet, relocated_validations);
                }
                rewrite_formulas_for_sheet_row_delete(
                    book,
                    sheet_name,
                    *start_row,
                    *count,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;
                rewrite_defined_name_formulas_for_sheet_row_delete(
                    book,
                    sheet_name,
                    *start_row,
                    *count,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;
                affected_sheets.insert(sheet_name.clone());
                counts
                    .entry("rows_deleted".to_string())
                    .and_modify(|v| *v += *count as u64)
                    .or_insert(*count as u64);
            }
            StructureOp::InsertCols {
                sheet_name,
                at_col,
                count,
            } => {
                if at_col.trim().is_empty() || *count == 0 {
                    bail!("insert_cols requires at_col and count>=1");
                }
                let col_letters = normalize_col_letters(at_col)?;
                let root_col =
                    umya_spreadsheet::helper::coordinate::column_index_from_string(&col_letters);
                {
                    let sheet = book
                        .get_sheet_by_name_mut(sheet_name).ok()
                        .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                    sheet.insert_new_column(&col_letters, *count);
                }
                rewrite_formulas_for_sheet_col_insert(
                    book,
                    sheet_name,
                    root_col,
                    *count,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;
                rewrite_defined_name_formulas_for_sheet_col_insert(
                    book,
                    sheet_name,
                    root_col,
                    *count,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;
                affected_sheets.insert(sheet_name.clone());
                counts
                    .entry("cols_inserted".to_string())
                    .and_modify(|v| *v += *count as u64)
                    .or_insert(*count as u64);
            }
            StructureOp::DeleteCols {
                sheet_name,
                start_col,
                count,
            } => {
                if start_col.trim().is_empty() || *count == 0 {
                    bail!("delete_cols requires start_col and count>=1");
                }
                let col_letters = normalize_col_letters(start_col)?;
                let root_col =
                    umya_spreadsheet::helper::coordinate::column_index_from_string(&col_letters);
                {
                    let sheet = book
                        .get_sheet_by_name_mut(sheet_name).ok()
                        .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                    sheet.remove_column(&col_letters, *count);
                }
                rewrite_formulas_for_sheet_col_delete(
                    book,
                    sheet_name,
                    root_col,
                    *count,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;
                rewrite_defined_name_formulas_for_sheet_col_delete(
                    book,
                    sheet_name,
                    root_col,
                    *count,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;
                affected_sheets.insert(sheet_name.clone());
                counts
                    .entry("cols_deleted".to_string())
                    .and_modify(|v| *v += *count as u64)
                    .or_insert(*count as u64);
            }
            StructureOp::RenameSheet { old_name, new_name } => {
                let old_name = old_name.trim();
                let new_name = new_name.trim();
                if old_name.is_empty() || new_name.is_empty() {
                    bail!("rename_sheet requires non-empty old_name and new_name");
                }

                let sheet_index = book
                    .get_sheet_collection_no_check()
                    .iter()
                    .position(|s| s.get_name() == old_name)
                    .ok_or_else(|| anyhow!("sheet '{}' not found", old_name))?;
                book.set_sheet_name(sheet_index, new_name.to_string())
                    .map_err(|e| anyhow!("failed to rename sheet '{}': {}", old_name, e))?;

                rewrite_formulas_for_sheet_rename(
                    book,
                    old_name,
                    new_name,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;
                rewrite_defined_name_formulas_for_sheet_rename(
                    book,
                    old_name,
                    new_name,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;

                affected_sheets.insert(old_name.to_string());
                affected_sheets.insert(new_name.to_string());
                counts
                    .entry("sheets_renamed".to_string())
                    .and_modify(|v| *v += 1)
                    .or_insert(1);
            }
            StructureOp::CreateSheet { name, position } => {
                let name_trimmed = name.trim();
                if name_trimmed.is_empty() {
                    bail!("create_sheet requires non-empty name");
                }
                let requested_position = *position;
                book.new_sheet(name_trimmed.to_string())
                    .map_err(|e| anyhow!("failed to create sheet '{}': {}", name_trimmed, e))?;

                if let Some(pos) = requested_position {
                    let desired = pos as usize;
                    let len = book.get_sheet_collection_no_check().len();
                    if desired >= len {
                        warnings.push(format!(
                            "create_sheet position {} is out of range (sheet_count {}). Appended at end.",
                            desired, len
                        ));
                    } else if desired != len - 1 {
                        let sheets = book.get_sheet_collection_mut();
                        sheets[desired..len].rotate_right(1);
                    }
                }

                affected_sheets.insert(name_trimmed.to_string());
                counts
                    .entry("sheets_created".to_string())
                    .and_modify(|v| *v += 1)
                    .or_insert(1);
            }
            StructureOp::DeleteSheet { name } => {
                let name_trimmed = name.trim();
                if name_trimmed.is_empty() {
                    bail!("delete_sheet requires non-empty name");
                }
                if book.get_sheet_collection_no_check().len() <= 1 {
                    bail!("cannot delete the last remaining sheet");
                }
                book.remove_sheet_by_name(name_trimmed)
                    .map_err(|e| anyhow!("failed to delete sheet '{}': {}", name_trimmed, e))?;
                affected_sheets.insert(name_trimmed.to_string());
                counts
                    .entry("sheets_deleted".to_string())
                    .and_modify(|v| *v += 1)
                    .or_insert(1);
            }
            StructureOp::CopyRange {
                sheet_name,
                dest_sheet_name,
                src_range,
                dest_anchor,
                include_styles,
                include_formulas,
            } => {
                let dest_sheet_name = dest_sheet_name.as_deref().unwrap_or(sheet_name);
                let result = copy_or_move_range(
                    book,
                    sheet_name,
                    dest_sheet_name,
                    src_range,
                    dest_anchor,
                    *include_styles,
                    *include_formulas,
                    false,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;
                affected_sheets.insert(sheet_name.clone());
                affected_sheets.insert(dest_sheet_name.to_string());
                counts
                    .entry("cells_copied".to_string())
                    .and_modify(|v| *v += result.cells_written)
                    .or_insert(result.cells_written);
                counts
                    .entry("ranges_copied".to_string())
                    .and_modify(|v| *v += 1)
                    .or_insert(1);
                warnings.extend(result.warnings);
            }
            StructureOp::MoveRange {
                sheet_name,
                dest_sheet_name,
                src_range,
                dest_anchor,
                include_styles,
                include_formulas,
            } => {
                let dest_sheet_name = dest_sheet_name.as_deref().unwrap_or(sheet_name);
                let result = copy_or_move_range(
                    book,
                    sheet_name,
                    dest_sheet_name,
                    src_range,
                    dest_anchor,
                    *include_styles,
                    *include_formulas,
                    true,
                    policy,
                    &mut formula_parse_diagnostics_builder,
                )?;
                affected_sheets.insert(sheet_name.clone());
                affected_sheets.insert(dest_sheet_name.to_string());
                counts
                    .entry("cells_moved".to_string())
                    .and_modify(|v| *v += result.cells_written)
                    .or_insert(result.cells_written);
                counts
                    .entry("ranges_moved".to_string())
                    .and_modify(|v| *v += 1)
                    .or_insert(1);
                warnings.extend(result.warnings);
            }
        }
    }

    let clamped_defined_names = clamp_out_of_bounds_defined_name_rows(book);
    if clamped_defined_names > 0 {
        warnings.push(format!(
            "Clamped {} defined name reference(s) to Excel max row 1048576 after structural edits.",
            clamped_defined_names
        ));
    }

    let summary = ChangeSummary {
        op_kinds: vec!["structure_batch".to_string()],
        affected_sheets: affected_sheets.into_iter().collect(),
        affected_bounds,
        counts,
        warnings,
        ..Default::default()
    };

    let formula_parse_diagnostics = if formula_parse_diagnostics_builder.has_errors() {
        Some(formula_parse_diagnostics_builder.build())
    } else {
        None
    };

    Ok(StructureApplyResult {
        ops_applied: ops.len(),
        summary,
        formula_parse_diagnostics,
    })
}

fn normalize_col_letters(col: &str) -> Result<String> {
    let letters = col.trim().to_ascii_uppercase();
    if letters.is_empty() || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        bail!("invalid column reference: {}", col);
    }
    Ok(letters)
}

struct CopyMoveApplyResult {
    cells_written: u64,
    warnings: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn ranges_intersect(
    a_min_col: u32,
    a_min_row: u32,
    a_max_col: u32,
    a_max_row: u32,
    b_min_col: u32,
    b_min_row: u32,
    b_max_col: u32,
    b_max_row: u32,
) -> bool {
    !(a_max_col < b_min_col
        || b_max_col < a_min_col
        || a_max_row < b_min_row
        || b_max_row < a_min_row)
}

#[allow(clippy::too_many_arguments)]
fn copy_or_move_range(
    book: &mut umya_spreadsheet::Workbook,
    src_sheet_name: &str,
    dest_sheet_name: &str,
    src_range: &str,
    dest_anchor: &str,
    include_styles: bool,
    include_formulas: bool,
    clear_source: bool,
    policy: FormulaParsePolicy,
    builder: &mut FormulaParseDiagnosticsBuilder,
) -> Result<CopyMoveApplyResult> {
    let src_bounds = parse_range_bounds(src_range)?;
    let (dest_start_col, dest_start_row) = parse_cell_ref(dest_anchor)?;

    let width = src_bounds.cols;
    let height = src_bounds.rows;

    let dest_end_col = dest_start_col
        .checked_add(width.saturating_sub(1))
        .ok_or_else(|| anyhow!("destination range overflows column bounds"))?;
    let dest_end_row = dest_start_row
        .checked_add(height.saturating_sub(1))
        .ok_or_else(|| anyhow!("destination range overflows row bounds"))?;

    let same_sheet = src_sheet_name == dest_sheet_name;

    if same_sheet
        && ranges_intersect(
            src_bounds.min_col,
            src_bounds.min_row,
            src_bounds.max_col,
            src_bounds.max_row,
            dest_start_col,
            dest_start_row,
            dest_end_col,
            dest_end_row,
        )
    {
        let dest_range = if width == 1 && height == 1 {
            crate::utils::cell_address(dest_start_col, dest_start_row)
        } else {
            format!(
                "{}:{}",
                crate::utils::cell_address(dest_start_col, dest_start_row),
                crate::utils::cell_address(dest_end_col, dest_end_row)
            )
        };
        bail!(
            "copy/move destination overlaps source (src {}, dest {})",
            src_range,
            dest_range
        );
    }

    let delta_col = dest_start_col as i32 - src_bounds.min_col as i32;
    let delta_row = dest_start_row as i32 - src_bounds.min_row as i32;

    let mut warnings: Vec<String> = Vec::new();
    let mut formula_value_copies: u64 = 0;

    let (src_sheet_index, dest_sheet_index) = {
        let sheets = book.get_sheet_collection_no_check();
        let src = sheets
            .iter()
            .position(|s| s.get_name() == src_sheet_name)
            .ok_or_else(|| anyhow!("sheet '{}' not found", src_sheet_name))?;
        let dest = sheets
            .iter()
            .position(|s| s.get_name() == dest_sheet_name)
            .ok_or_else(|| anyhow!("sheet '{}' not found", dest_sheet_name))?;
        (src, dest)
    };

    let sheets = book.get_sheet_collection_mut();

    if src_sheet_index == dest_sheet_index {
        let sheet = &mut sheets[src_sheet_index];

        for row in 0..height {
            for col in 0..width {
                let src_col = src_bounds.min_col + col;
                let src_row = src_bounds.min_row + row;
                let dest_col = dest_start_col + col;
                let dest_row = dest_start_row + row;

                let Some(src_cell) = sheet.get_cell((src_col, src_row)) else {
                    sheet.remove_cell((dest_col, dest_row));
                    continue;
                };

                let mut set_value = true;
                let mut dest_formula: Option<String> = None;

                if include_formulas && src_cell.is_formula() {
                    let src_formula = src_cell.get_formula().to_string();
                    if policy == FormulaParsePolicy::Off {
                        dest_formula = Some(src_formula);
                        set_value = false;
                    } else {
                        match parse_base_formula(&src_formula).and_then(|ast| {
                            shift_formula_ast(&ast, delta_col, delta_row, RelativeMode::Excel)
                        }) {
                            Ok(shifted) => {
                                let shifted =
                                    shifted.strip_prefix('=').unwrap_or(&shifted).to_string();
                                dest_formula = Some(shifted);
                                set_value = false;
                            }
                            Err(err) => {
                                let src_address = crate::utils::cell_address(src_col, src_row);
                                let dest_address = crate::utils::cell_address(dest_col, dest_row);
                                if policy == FormulaParsePolicy::Fail {
                                    bail!(
                                        "{}formula shift failed in {}!{} -> {}!{}: {}",
                                        FORMULA_PARSE_FAILED_PREFIX,
                                        src_sheet_name,
                                        src_address,
                                        dest_sheet_name,
                                        dest_address,
                                        err
                                    );
                                }
                                builder.record_error(
                                    dest_sheet_name,
                                    &dest_address,
                                    &src_formula,
                                    &err.to_string(),
                                );
                                dest_formula = Some(src_formula);
                                set_value = false;
                            }
                        }
                    }
                } else if !include_formulas && src_cell.is_formula() {
                    formula_value_copies += 1;
                }

                let mut src_value = src_cell.get_cell_value().clone();
                src_value.remove_formula();
                let src_style = src_cell.get_style().clone();

                let dest_cell = sheet.get_cell_mut((dest_col, dest_row));
                if include_styles {
                    dest_cell.set_style(src_style);
                }

                dest_cell.get_cell_value_mut().remove_formula();
                if let Some(formula) = dest_formula {
                    dest_cell.set_formula(formula);
                    dest_cell.set_formula_result_default("");
                }
                if set_value {
                    dest_cell.set_cell_value(src_value);
                }
            }
        }

        if clear_source {
            for row in 0..height {
                for col in 0..width {
                    let src_col = src_bounds.min_col + col;
                    let src_row = src_bounds.min_row + row;
                    sheet.remove_cell((src_col, src_row));
                }
            }
        }
    } else {
        let (src_sheet, dest_sheet) = if src_sheet_index < dest_sheet_index {
            let (left, right) = sheets.split_at_mut(dest_sheet_index);
            (&mut left[src_sheet_index], &mut right[0])
        } else {
            let (left, right) = sheets.split_at_mut(src_sheet_index);
            (&mut right[0], &mut left[dest_sheet_index])
        };

        for row in 0..height {
            for col in 0..width {
                let src_col = src_bounds.min_col + col;
                let src_row = src_bounds.min_row + row;
                let dest_col = dest_start_col + col;
                let dest_row = dest_start_row + row;

                let Some(src_cell) = src_sheet.get_cell((src_col, src_row)) else {
                    dest_sheet.remove_cell((dest_col, dest_row));
                    continue;
                };

                let mut set_value = true;
                let mut dest_formula: Option<String> = None;

                if include_formulas && src_cell.is_formula() {
                    let src_formula = src_cell.get_formula().to_string();
                    if policy == FormulaParsePolicy::Off {
                        dest_formula = Some(src_formula);
                        set_value = false;
                    } else {
                        match parse_base_formula(&src_formula).and_then(|ast| {
                            shift_formula_ast(&ast, delta_col, delta_row, RelativeMode::Excel)
                        }) {
                            Ok(shifted) => {
                                let shifted =
                                    shifted.strip_prefix('=').unwrap_or(&shifted).to_string();
                                dest_formula = Some(shifted);
                                set_value = false;
                            }
                            Err(err) => {
                                let src_address = crate::utils::cell_address(src_col, src_row);
                                let dest_address = crate::utils::cell_address(dest_col, dest_row);
                                if policy == FormulaParsePolicy::Fail {
                                    bail!(
                                        "{}formula shift failed in {}!{} -> {}!{}: {}",
                                        FORMULA_PARSE_FAILED_PREFIX,
                                        src_sheet_name,
                                        src_address,
                                        dest_sheet_name,
                                        dest_address,
                                        err
                                    );
                                }
                                builder.record_error(
                                    dest_sheet_name,
                                    &dest_address,
                                    &src_formula,
                                    &err.to_string(),
                                );
                                dest_formula = Some(src_formula);
                                set_value = false;
                            }
                        }
                    }
                } else if !include_formulas && src_cell.is_formula() {
                    formula_value_copies += 1;
                }

                let mut src_value = src_cell.get_cell_value().clone();
                src_value.remove_formula();
                let src_style = src_cell.get_style().clone();

                let dest_cell = dest_sheet.get_cell_mut((dest_col, dest_row));
                if include_styles {
                    dest_cell.set_style(src_style);
                }

                dest_cell.get_cell_value_mut().remove_formula();
                if let Some(formula) = dest_formula {
                    dest_cell.set_formula(formula);
                    dest_cell.set_formula_result_default("");
                }
                if set_value {
                    dest_cell.set_cell_value(src_value);
                }
            }
        }

        if clear_source {
            for row in 0..height {
                for col in 0..width {
                    let src_col = src_bounds.min_col + col;
                    let src_row = src_bounds.min_row + row;
                    src_sheet.remove_cell((src_col, src_row));
                }
            }
        }
    }

    if !include_formulas && formula_value_copies > 0 {
        warnings.push(format!(
            "Copied cached values for {} formula cell(s) (include_formulas=false); run recalculate for fresh results.",
            formula_value_copies
        ));
    }

    Ok(CopyMoveApplyResult {
        cells_written: width as u64 * height as u64,
        warnings,
    })
}

fn rewrite_formulas_for_sheet_rename(
    book: &mut umya_spreadsheet::Workbook,
    old_name: &str,
    new_name: &str,
    policy: FormulaParsePolicy,
    builder: &mut FormulaParseDiagnosticsBuilder,
) -> Result<()> {
    let new_prefix = format_sheet_prefix_for_formula(new_name);

    for sheet in book.get_sheet_collection_mut().iter_mut() {
        let sheet_name = sheet.get_name().to_string();
        for cell in sheet.cells_mut() {
            if !cell.is_formula() {
                continue;
            }
            let formula_text = cell.get_formula();
            if formula_text.is_empty() {
                continue;
            }
            let formula_with_equals = if formula_text.starts_with('=') {
                formula_text.to_string()
            } else {
                format!("={}", formula_text)
            };

            if policy == FormulaParsePolicy::Off {
                continue;
            }

            let cell_address = cell.get_coordinate().get_coordinate().to_string();
            let context_description = format!("{}!{}", sheet_name, cell_address);

            let tokens = match Tokenizer::new(&formula_with_equals) {
                Ok(tokenizer) => tokenizer.items,
                Err(e) => {
                    if policy == FormulaParsePolicy::Fail {
                        bail!(
                            "{}tokenizer error in {}: {}",
                            FORMULA_PARSE_FAILED_PREFIX,
                            context_description,
                            e.message
                        );
                    }
                    builder.record_error(&sheet_name, &cell_address, formula_text, &e.message);
                    continue;
                }
            };
            let mut out = String::with_capacity(formula_with_equals.len());
            let mut cursor = 0usize;

            for token in &tokens {
                if token.start > cursor {
                    out.push_str(&formula_with_equals[cursor..token.start]);
                }

                let mut value = token.value.clone();
                if token.subtype == formualizer_parse::TokenSubType::Range
                    && value.contains('!')
                    && let Some((sheet_part, tail)) = value.split_once('!')
                    && sheet_part_matches(sheet_part, old_name)
                {
                    value = format!("{}{}", new_prefix, tail);
                }

                out.push_str(&value);
                cursor = token.end;
            }

            if cursor < formula_with_equals.len() {
                out.push_str(&formula_with_equals[cursor..]);
            }

            let new_formula = out.strip_prefix('=').unwrap_or(&out);
            cell.set_formula(new_formula.to_string());
            cell.set_formula_result_default("");
        }
    }

    Ok(())
}

fn rewrite_defined_name_formulas_for_sheet_rename(
    book: &mut umya_spreadsheet::Workbook,
    old_name: &str,
    new_name: &str,
    policy: FormulaParsePolicy,
    builder: &mut FormulaParseDiagnosticsBuilder,
) -> Result<()> {
    let new_prefix = format_sheet_prefix_for_formula(new_name);

    for defined in book.get_defined_names_mut() {
        let refers_to = defined.get_address();
        let trimmed = refers_to.trim();
        let had_equals = trimmed.starts_with('=');
        let looks_like_formula = had_equals || trimmed.contains('(');
        if !looks_like_formula {
            continue;
        }

        let formula_in = if had_equals {
            trimmed.to_string()
        } else {
            format!("={}", trimmed)
        };

        if policy == FormulaParsePolicy::Off {
            continue;
        }

        let defined_name = defined.get_name().to_string();
        let context_description = format!("defined name '{}'", defined_name);

        let tokens = match Tokenizer::new(&formula_in) {
            Ok(tokenizer) => tokenizer.items,
            Err(e) => {
                if policy == FormulaParsePolicy::Fail {
                    bail!(
                        "{}tokenizer error in {}: {}",
                        FORMULA_PARSE_FAILED_PREFIX,
                        context_description,
                        e.message
                    );
                }
                builder.record_error("[DefinedName]", &defined_name, trimmed, &e.message);
                continue;
            }
        };

        let mut out = String::with_capacity(formula_in.len());
        let mut cursor = 0usize;
        let mut changed = false;

        for token in &tokens {
            if token.start > cursor {
                out.push_str(&formula_in[cursor..token.start]);
            }

            let mut value = token.value.clone();
            if token.subtype == formualizer_parse::TokenSubType::Range
                && value.contains('!')
                && let Some((sheet_part, tail)) = value.split_once('!')
                && sheet_part_matches(sheet_part, old_name)
            {
                value = format!("{}{}", new_prefix, tail);
                changed = true;
            }

            out.push_str(&value);
            cursor = token.end;
        }

        if cursor < formula_in.len() {
            out.push_str(&formula_in[cursor..]);
        }

        if changed {
            let out_final = if had_equals {
                out
            } else {
                out.strip_prefix('=').unwrap_or(&out).to_string()
            };
            defined.set_address(out_final);
        }
    }

    Ok(())
}

fn rewrite_defined_name_formulas_for_sheet_col_insert(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    at_col: u32,
    count: u32,
    policy: FormulaParsePolicy,
    builder: &mut FormulaParseDiagnosticsBuilder,
) -> Result<()> {
    rewrite_defined_name_formulas_for_sheet_structure_change(
        book,
        sheet_name,
        StructureAxis::Col,
        StructureEdit::Insert { at: at_col, count },
        policy,
        builder,
    )
}

fn rewrite_defined_name_formulas_for_sheet_col_delete(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    start_col: u32,
    count: u32,
    policy: FormulaParsePolicy,
    builder: &mut FormulaParseDiagnosticsBuilder,
) -> Result<()> {
    rewrite_defined_name_formulas_for_sheet_structure_change(
        book,
        sheet_name,
        StructureAxis::Col,
        StructureEdit::Delete {
            start: start_col,
            count,
        },
        policy,
        builder,
    )
}

fn rewrite_defined_name_formulas_for_sheet_row_insert(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    at_row: u32,
    count: u32,
    policy: FormulaParsePolicy,
    builder: &mut FormulaParseDiagnosticsBuilder,
) -> Result<()> {
    rewrite_defined_name_formulas_for_sheet_structure_change(
        book,
        sheet_name,
        StructureAxis::Row,
        StructureEdit::Insert { at: at_row, count },
        policy,
        builder,
    )
}

fn rewrite_defined_name_formulas_for_sheet_row_delete(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    start_row: u32,
    count: u32,
    policy: FormulaParsePolicy,
    builder: &mut FormulaParseDiagnosticsBuilder,
) -> Result<()> {
    rewrite_defined_name_formulas_for_sheet_structure_change(
        book,
        sheet_name,
        StructureAxis::Row,
        StructureEdit::Delete {
            start: start_row,
            count,
        },
        policy,
        builder,
    )
}

fn rewrite_defined_name_formulas_for_sheet_structure_change(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    axis: StructureAxis,
    edit: StructureEdit,
    policy: FormulaParsePolicy,
    builder: &mut FormulaParseDiagnosticsBuilder,
) -> Result<()> {
    for defined in book.get_defined_names_mut() {
        let refers_to = defined.get_address();
        let trimmed = refers_to.trim();
        let had_equals = trimmed.starts_with('=');
        let looks_like_formula = had_equals || trimmed.contains('(');
        if !looks_like_formula {
            continue;
        }

        let formula_in = if had_equals {
            trimmed.to_string()
        } else {
            format!("={}", trimmed)
        };

        if policy == FormulaParsePolicy::Off {
            continue;
        }

        let defined_name = defined.get_name().to_string();
        let context_description = format!("defined name '{}'", defined_name);

        let tokens = match Tokenizer::new(&formula_in) {
            Ok(tokenizer) => tokenizer.items,
            Err(e) => {
                if policy == FormulaParsePolicy::Fail {
                    bail!(
                        "{}tokenizer error in {}: {}",
                        FORMULA_PARSE_FAILED_PREFIX,
                        context_description,
                        e.message
                    );
                }
                builder.record_error("[DefinedName]", &defined_name, trimmed, &e.message);
                continue;
            }
        };

        let mut out = String::with_capacity(formula_in.len());
        let mut cursor = 0usize;
        let mut changed = false;

        for token in &tokens {
            if token.start > cursor {
                out.push_str(&formula_in[cursor..token.start]);
            }

            let mut value = token.value.clone();
            if token.subtype == formualizer_parse::TokenSubType::Range
                && value.contains('!')
                && let Some((sheet_part, coord_part)) = value.split_once('!')
                && sheet_part_matches(sheet_part, sheet_name)
            {
                let adjusted = adjust_ref_coord_part(coord_part, axis, edit)?;
                value = format!("{sheet_part}!{adjusted}");
                changed = true;
            }

            out.push_str(&value);
            cursor = token.end;
        }

        if cursor < formula_in.len() {
            out.push_str(&formula_in[cursor..]);
        }

        if changed {
            let out_final = if had_equals {
                out
            } else {
                out.strip_prefix('=').unwrap_or(&out).to_string()
            };
            defined.set_address(out_final);
        }
    }

    Ok(())
}

fn clamp_defined_name_refers_to_max_row(refers_to: &str) -> Option<String> {
    const EXCEL_MAX_ROW: u32 = 1_048_576;

    let trimmed = refers_to.trim();
    if trimmed.is_empty() {
        return None;
    }

    let had_equals = trimmed.starts_with('=');
    let formula_in = if had_equals {
        trimmed.to_string()
    } else {
        format!("={trimmed}")
    };

    let tokens = Tokenizer::new(&formula_in).ok()?.items;

    let mut out = String::with_capacity(formula_in.len());
    let mut cursor = 0usize;
    let mut changed = false;

    for token in &tokens {
        if token.start > cursor {
            out.push_str(&formula_in[cursor..token.start]);
        }

        let mut value = token.value.clone();
        if token.subtype == formualizer_parse::TokenSubType::Range {
            let clamped = clamp_range_token_max_row(&value, EXCEL_MAX_ROW);
            if clamped != value {
                value = clamped;
                changed = true;
            }
        }

        out.push_str(&value);
        cursor = token.end;
    }

    if cursor < formula_in.len() {
        out.push_str(&formula_in[cursor..]);
    }

    if !changed {
        return None;
    }

    Some(if had_equals {
        out
    } else {
        out.strip_prefix('=').unwrap_or(&out).to_string()
    })
}

fn clamp_range_token_max_row(token_value: &str, max_row: u32) -> String {
    if token_value == "#REF!" {
        return token_value.to_string();
    }

    if let Some((sheet_part, coord_part)) = token_value.split_once('!') {
        let clamped = clamp_coord_part_max_row(coord_part, max_row);
        if clamped == coord_part {
            token_value.to_string()
        } else {
            format!("{sheet_part}!{clamped}")
        }
    } else {
        clamp_coord_part_max_row(token_value, max_row)
    }
}

fn clamp_coord_part_max_row(coord_part: &str, max_row: u32) -> String {
    let mut changed = false;
    let mut union_out = Vec::new();

    for union_piece in coord_part.split(',') {
        let mut range_out = Vec::new();
        let mut local_changed = false;

        for segment in union_piece.split(':') {
            let clamped = clamp_ref_segment_max_row(segment, max_row);
            if clamped != segment {
                local_changed = true;
            }
            range_out.push(clamped);
        }

        if local_changed {
            changed = true;
        }
        union_out.push(range_out.join(":"));
    }

    if changed {
        union_out.join(",")
    } else {
        coord_part.to_string()
    }
}

fn clamp_ref_segment_max_row(segment: &str, max_row: u32) -> String {
    use umya_spreadsheet::helper::coordinate::{
        coordinate_from_index_with_lock, index_from_coordinate, string_from_column_index,
    };

    let (col, row, col_lock, row_lock) = index_from_coordinate(segment);

    // Not a coordinate-like segment (e.g., structured reference); leave untouched.
    if col.is_none() && row.is_none() {
        return segment.to_string();
    }

    let row = row.map(|value| value.min(max_row));

    match (col, row) {
        (Some(c), Some(r)) => coordinate_from_index_with_lock(
            c,
            r,
            col_lock.unwrap_or(false),
            row_lock.unwrap_or(false),
        ),
        (Some(c), None) => {
            let col_str = string_from_column_index(c);
            format!(
                "{}{}",
                if col_lock.unwrap_or(false) { "$" } else { "" },
                col_str
            )
        }
        (None, Some(r)) => format!("{}{}", if row_lock.unwrap_or(false) { "$" } else { "" }, r),
        (None, None) => segment.to_string(),
    }
}

fn clamp_out_of_bounds_defined_name_rows(book: &mut umya_spreadsheet::Workbook) -> usize {
    #[derive(Debug)]
    struct Patch {
        idx: usize,
        name: String,
        clamped_address: String,
        hidden: bool,
        local_sheet_id: Option<u32>,
        sheet_hint: Option<String>,
    }

    fn extract_first_sheet_name(refers_to: &str) -> Option<String> {
        let first = refers_to.split(',').next()?.trim();
        let (sheet_part, _) = first.split_once('!')?;
        let sheet_part = sheet_part.trim();
        if let Some(inner) = sheet_part
            .strip_prefix('\'')
            .and_then(|value| value.strip_suffix('\''))
        {
            Some(inner.replace("''", "'"))
        } else {
            Some(sheet_part.to_string())
        }
    }

    let mut patches = Vec::new();
    for (idx, defined) in book.get_defined_names().iter().enumerate() {
        let original = defined.get_address();
        let trimmed = original.trim();

        // Keep formula-like names on the tokenizer path; clamp plain address unions only.
        if trimmed.starts_with('=') || trimmed.contains('(') || trimmed.is_empty() {
            continue;
        }

        if let Some(clamped_address) = clamp_defined_name_refers_to_max_row(trimmed) {
            patches.push(Patch {
                idx,
                name: defined.get_name().to_string(),
                clamped_address,
                hidden: defined.get_hidden(),
                local_sheet_id: if defined.has_local_sheet_id() {
                    Some(defined.get_local_sheet_id())
                } else {
                    None
                },
                sheet_hint: extract_first_sheet_name(trimmed),
            });
        }
    }

    let mut applied = 0usize;
    for patch in patches {
        let target_sheet = patch
            .sheet_hint
            .as_ref()
            .filter(|name| book.get_sheet_by_name(name).ok().is_some())
            .cloned()
            .or_else(|| {
                book.get_sheet_collection_no_check()
                    .first()
                    .map(|sheet| sheet.get_name().to_string())
            });

        let Some(target_sheet) = target_sheet else {
            continue;
        };

        let mut replacement = {
            let Some(sheet) = book.get_sheet_by_name_mut(&target_sheet).ok() else {
                continue;
            };
            let before = sheet.get_defined_names().len();
            if sheet
                .add_defined_name(patch.name.clone(), patch.clamped_address.clone())
                .is_err()
            {
                continue;
            }
            let Some(candidate) = sheet.get_defined_names().last().cloned() else {
                sheet.get_defined_names_mut().truncate(before);
                continue;
            };
            sheet.get_defined_names_mut().truncate(before);
            candidate
        };

        replacement.set_hidden(patch.hidden);
        if let Some(local_sheet_id) = patch.local_sheet_id {
            replacement.set_local_sheet_id(local_sheet_id);
        }

        if let Some(slot) = book.get_defined_names_mut().get_mut(patch.idx) {
            *slot = replacement;
            applied += 1;
        }
    }

    applied
}

fn clamp_defined_name_rows_in_workbook_xml(xml: &str) -> (String, usize) {
    let defined_name_re =
        Regex::new(r"(?s)(<definedName[^>]*>)([^<]*)(</definedName>)").expect("valid xml regex");

    let mut clamped_entries = 0usize;
    let rewritten = defined_name_re
        .replace_all(xml, |caps: &regex::Captures<'_>| {
            let full = caps.get(0).map(|m| m.as_str()).unwrap_or_default();
            let prefix = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let body = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
            let suffix = caps.get(3).map(|m| m.as_str()).unwrap_or_default();

            if body.trim_start().starts_with('=') || body.contains('(') {
                return full.to_string();
            }

            if let Some(clamped_body) = clamp_defined_name_refers_to_max_row(body) {
                clamped_entries += 1;
                format!("{prefix}{clamped_body}{suffix}")
            } else {
                full.to_string()
            }
        })
        .to_string();

    (rewritten, clamped_entries)
}

fn sanitize_workbook_xml_defined_name_rows(path: &Path) -> Result<usize> {
    use zip::{ZipArchive, ZipWriter, write::FileOptions};

    let input_file = fs::File::open(path)?;
    let mut archive = ZipArchive::new(input_file)?;

    #[derive(Debug)]
    struct ZipEntry {
        name: String,
        is_dir: bool,
        data: Vec<u8>,
        compression: zip::CompressionMethod,
        unix_mode: Option<u32>,
        modified: zip::DateTime,
    }

    let mut entries: Vec<ZipEntry> = Vec::with_capacity(archive.len());
    let mut clamped_defined_names = 0usize;

    for idx in 0..archive.len() {
        let mut file = archive.by_index(idx)?;
        let name = file.name().to_string();
        let is_dir = file.is_dir();
        let compression = file.compression();
        let unix_mode = file.unix_mode();
        let modified = file.last_modified();

        let mut data = Vec::new();
        if !is_dir {
            file.read_to_end(&mut data)?;
            if name == "xl/workbook.xml" {
                let xml = String::from_utf8_lossy(&data);
                let (rewritten, changed) = clamp_defined_name_rows_in_workbook_xml(&xml);
                clamped_defined_names = changed;
                if changed > 0 {
                    data = rewritten.into_bytes();
                }
            }
        }

        entries.push(ZipEntry {
            name,
            is_dir,
            data,
            compression,
            unix_mode,
            modified,
        });
    }

    if clamped_defined_names == 0 {
        return Ok(0);
    }

    let temp_path = path.with_extension("xlsx.tmp");
    let output_file = fs::File::create(&temp_path)?;
    let mut writer = ZipWriter::new(output_file);

    for entry in entries {
        let mut options = FileOptions::default()
            .compression_method(entry.compression)
            .last_modified_time(entry.modified);
        if let Some(mode) = entry.unix_mode {
            options = options.unix_permissions(mode);
        }

        if entry.is_dir {
            writer.add_directory(entry.name, options)?;
        } else {
            writer.start_file(entry.name, options)?;
            writer.write_all(&entry.data)?;
        }
    }

    writer.finish()?;
    fs::rename(temp_path, path)?;
    Ok(clamped_defined_names)
}

fn rewrite_formulas_for_sheet_col_insert(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    at_col: u32,
    count: u32,
    policy: FormulaParsePolicy,
    builder: &mut FormulaParseDiagnosticsBuilder,
) -> Result<()> {
    rewrite_formulas_for_sheet_structure_change(
        book,
        sheet_name,
        StructureAxis::Col,
        StructureEdit::Insert { at: at_col, count },
        policy,
        builder,
    )
}

fn rewrite_formulas_for_sheet_col_delete(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    start_col: u32,
    count: u32,
    policy: FormulaParsePolicy,
    builder: &mut FormulaParseDiagnosticsBuilder,
) -> Result<()> {
    rewrite_formulas_for_sheet_structure_change(
        book,
        sheet_name,
        StructureAxis::Col,
        StructureEdit::Delete {
            start: start_col,
            count,
        },
        policy,
        builder,
    )
}

fn rewrite_formulas_for_sheet_row_insert(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    at_row: u32,
    count: u32,
    policy: FormulaParsePolicy,
    builder: &mut FormulaParseDiagnosticsBuilder,
) -> Result<()> {
    rewrite_formulas_for_sheet_structure_change(
        book,
        sheet_name,
        StructureAxis::Row,
        StructureEdit::Insert { at: at_row, count },
        policy,
        builder,
    )
}

fn rewrite_formulas_for_sheet_row_delete(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    start_row: u32,
    count: u32,
    policy: FormulaParsePolicy,
    builder: &mut FormulaParseDiagnosticsBuilder,
) -> Result<()> {
    rewrite_formulas_for_sheet_structure_change(
        book,
        sheet_name,
        StructureAxis::Row,
        StructureEdit::Delete {
            start: start_row,
            count,
        },
        policy,
        builder,
    )
}

#[derive(Debug, Clone, Copy)]
enum StructureAxis {
    Row,
    Col,
}

#[derive(Debug, Clone, Copy)]
enum StructureEdit {
    Insert { at: u32, count: u32 },
    Delete { start: u32, count: u32 },
}

fn rewrite_formulas_for_sheet_structure_change(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    axis: StructureAxis,
    edit: StructureEdit,
    policy: FormulaParsePolicy,
    builder: &mut FormulaParseDiagnosticsBuilder,
) -> Result<()> {
    for sheet in book.get_sheet_collection_mut().iter_mut() {
        if sheet.get_name() == sheet_name {
            continue;
        }
        let current_sheet_name = sheet.get_name().to_string();
        for cell in sheet.cells_mut() {
            if !cell.is_formula() {
                continue;
            }
            let formula_text = cell.get_formula();
            if formula_text.is_empty() {
                continue;
            }
            let formula_with_equals = if formula_text.starts_with('=') {
                formula_text.to_string()
            } else {
                format!("={}", formula_text)
            };
            if policy == FormulaParsePolicy::Off {
                continue;
            }

            let cell_address = cell.get_coordinate().get_coordinate().to_string();
            let context_description = format!("{}!{}", current_sheet_name, cell_address);

            let tokens = match Tokenizer::new(&formula_with_equals) {
                Ok(tokenizer) => tokenizer.items,
                Err(e) => {
                    if policy == FormulaParsePolicy::Fail {
                        bail!(
                            "{}tokenizer error in {}: {}",
                            FORMULA_PARSE_FAILED_PREFIX,
                            context_description,
                            e.message
                        );
                    }
                    builder.record_error(
                        &current_sheet_name,
                        &cell_address,
                        formula_text,
                        &e.message,
                    );
                    continue;
                }
            };

            let mut out = String::with_capacity(formula_with_equals.len());
            let mut cursor = 0usize;
            let mut changed = false;

            for token in &tokens {
                if token.start > cursor {
                    out.push_str(&formula_with_equals[cursor..token.start]);
                }

                let mut value = token.value.clone();
                if token.subtype == formualizer_parse::TokenSubType::Range
                    && value.contains('!')
                    && let Some((sheet_part, coord_part)) = value.split_once('!')
                    && sheet_part_matches(sheet_part, sheet_name)
                {
                    let adjusted = adjust_ref_coord_part(coord_part, axis, edit)?;
                    value = format!("{sheet_part}!{adjusted}");
                    changed = true;
                }

                out.push_str(&value);
                cursor = token.end;
            }

            if cursor < formula_with_equals.len() {
                out.push_str(&formula_with_equals[cursor..]);
            }

            if changed {
                let new_formula = out.strip_prefix('=').unwrap_or(&out);
                cell.set_formula(new_formula.to_string());
                cell.set_formula_result_default("");
            }
        }
    }
    Ok(())
}

fn adjust_ref_coord_part(
    coord_part: &str,
    axis: StructureAxis,
    edit: StructureEdit,
) -> Result<String> {
    Ok(transform_a1_range(coord_part, axis, edit)?.unwrap_or_else(|| "#REF!".to_string()))
}

#[derive(Debug, Clone, Copy)]
struct A1RefSegment {
    col: Option<u32>,
    row: Option<u32>,
    col_lock: bool,
    row_lock: bool,
}

fn transform_a1_range(
    coord_part: &str,
    axis: StructureAxis,
    edit: StructureEdit,
) -> Result<Option<String>> {
    if coord_part == "#REF!" {
        return Ok(Some(coord_part.to_string()));
    }

    let mut transformed = Vec::new();
    for piece in coord_part.split(',') {
        if let Some(value) = transform_a1_range_piece(piece, axis, edit)? {
            transformed.push(value);
        }
    }
    Ok((!transformed.is_empty()).then(|| transformed.join(",")))
}

fn transform_a1_range_piece(
    piece: &str,
    axis: StructureAxis,
    edit: StructureEdit,
) -> Result<Option<String>> {
    if let Some((start_raw, end_raw)) = piece.split_once(':') {
        let mut start = parse_a1_ref_segment(start_raw)?;
        let mut end = parse_a1_ref_segment(end_raw)?;
        let (start_value, end_value) = match axis {
            StructureAxis::Row => (&mut start.row, &mut end.row),
            StructureAxis::Col => (&mut start.col, &mut end.col),
        };

        match (*start_value, *end_value) {
            (Some(first), Some(last)) if first <= last => {
                let Some((new_first, new_last)) = transform_a1_interval(first, last, axis, edit)
                else {
                    return Ok(None);
                };
                *start_value = Some(new_first);
                *end_value = Some(new_last);
            }
            (None, None) => {}
            _ => {
                *start_value = transform_a1_scalar(*start_value, axis, edit);
                *end_value = transform_a1_scalar(*end_value, axis, edit);
                if start_value.is_none() || end_value.is_none() {
                    return Ok(None);
                }
            }
        }

        Ok(Some(format!(
            "{}:{}",
            format_a1_ref_segment(start)?,
            format_a1_ref_segment(end)?
        )))
    } else {
        let mut segment = parse_a1_ref_segment(piece)?;
        let value = match axis {
            StructureAxis::Row => &mut segment.row,
            StructureAxis::Col => &mut segment.col,
        };
        *value = transform_a1_scalar(*value, axis, edit);
        if segment.col.is_none() && segment.row.is_none() {
            return Ok(None);
        }
        Ok(Some(format_a1_ref_segment(segment)?))
    }
}

fn parse_a1_ref_segment(segment: &str) -> Result<A1RefSegment> {
    use umya_spreadsheet::helper::coordinate::index_from_coordinate;

    let (col, row, col_lock, row_lock) = index_from_coordinate(segment);
    if col.is_none() && row.is_none() {
        bail!("invalid A1 reference segment '{segment}'");
    }
    Ok(A1RefSegment {
        col,
        row,
        col_lock: col_lock.unwrap_or(false),
        row_lock: row_lock.unwrap_or(false),
    })
}

fn format_a1_ref_segment(segment: A1RefSegment) -> Result<String> {
    use umya_spreadsheet::helper::coordinate::{
        coordinate_from_index_with_lock, string_from_column_index,
    };

    match (segment.col, segment.row) {
        (Some(col), Some(row)) => Ok(coordinate_from_index_with_lock(
            col,
            row,
            segment.col_lock,
            segment.row_lock,
        )),
        (Some(col), None) => Ok(format!(
            "{}{}",
            if segment.col_lock { "$" } else { "" },
            string_from_column_index(col)
        )),
        (None, Some(row)) => Ok(format!(
            "{}{}",
            if segment.row_lock { "$" } else { "" },
            row
        )),
        (None, None) => bail!("A1 reference was removed"),
    }
}

fn transform_a1_interval(
    first: u32,
    last: u32,
    axis: StructureAxis,
    edit: StructureEdit,
) -> Option<(u32, u32)> {
    let max_value = match axis {
        StructureAxis::Row => 1_048_576,
        StructureAxis::Col => 16_384,
    };
    match edit {
        StructureEdit::Insert { at, count } => Some((
            adjust_insert_bounded(first, at, count, max_value),
            adjust_insert_bounded(last, at, count, max_value),
        )),
        StructureEdit::Delete { start, count } => {
            let deleted_end = start.saturating_add(count.saturating_sub(1));
            if last < start {
                Some((first, last))
            } else if first > deleted_end {
                Some((first - count, last - count))
            } else {
                let remaining_before = first < start;
                let remaining_after = last > deleted_end;
                match (remaining_before, remaining_after) {
                    (false, false) => None,
                    (true, false) => Some((first, start - 1)),
                    (false, true) => Some((start, last - count)),
                    (true, true) => Some((first, last - count)),
                }
            }
        }
    }
}

fn transform_a1_scalar(
    value: Option<u32>,
    axis: StructureAxis,
    edit: StructureEdit,
) -> Option<u32> {
    let value = value?;
    let max_value = match axis {
        StructureAxis::Row => 1_048_576,
        StructureAxis::Col => 16_384,
    };
    match edit {
        StructureEdit::Insert { at, count } => {
            Some(adjust_insert_bounded(value, at, count, max_value))
        }
        StructureEdit::Delete { start, count } => adjust_delete(value, start, count),
    }
}

fn transform_data_validations_for_structure_change(
    sheet: &umya_spreadsheet::Worksheet,
    axis: StructureAxis,
    edit: StructureEdit,
) -> Result<Option<umya_spreadsheet::structs::DataValidations>> {
    let Some(source) = sheet.get_data_validations() else {
        return Ok(None);
    };
    let mut transformed = umya_spreadsheet::structs::DataValidations::default();
    for source_validation in source.get_data_validation_list() {
        let mut validation = source_validation.clone();
        let original_ranges = validation
            .get_sequence_of_references()
            .get_range_collection()
            .iter()
            .map(|range| range.get_range())
            .collect::<Vec<_>>();
        let references = validation.get_sequence_of_references_mut();
        references.remove_range_collection();
        let mut seen = BTreeSet::new();
        for original in original_ranges {
            if let Some(relocated) = transform_a1_range(&original, axis, edit)?
                && seen.insert(relocated.clone())
            {
                references.set_sqref(relocated);
            }
        }
        if !references.get_range_collection().is_empty() {
            transformed.add_data_validation_list(validation);
        }
    }
    Ok((!transformed.get_data_validation_list().is_empty()).then_some(transformed))
}

fn replace_data_validations(
    sheet: &mut umya_spreadsheet::Worksheet,
    validations: Option<umya_spreadsheet::structs::DataValidations>,
) {
    sheet.remove_data_validations();
    if let Some(validations) = validations {
        sheet.set_data_validations(validations);
    }
}

fn adjust_insert(value: u32, at: u32, count: u32) -> u32 {
    if value >= at {
        value.saturating_add(count)
    } else {
        value
    }
}

fn adjust_insert_bounded(value: u32, at: u32, count: u32, max_value: u32) -> u32 {
    adjust_insert(value, at, count).min(max_value)
}

fn adjust_delete(value: u32, start: u32, count: u32) -> Option<u32> {
    let end = start.saturating_add(count.saturating_sub(1));
    if value >= start && value <= end {
        None
    } else if value > end {
        Some(value - count)
    } else {
        Some(value)
    }
}

// ---------------------------------------------------------------------------
// Adjacent SUM expansion helpers (ticket 4104)
// ---------------------------------------------------------------------------

/// Regex for simple `SUM(ColRow:ColRow)` patterns (A1-style, no sheet prefix).
/// Captures: full match, col1, row1, col2, row2.
fn simple_sum_range_regex() -> Regex {
    Regex::new(r"(?i)^SUM\(([A-Z]{1,3})(\d+):([A-Z]{1,3})(\d+)\)$").expect("valid regex")
}

/// After inserting `count` rows at `at_row`, scan the subtotal row immediately
/// below the insertion band (row `at_row + count`) for simple `SUM(Ax:Ay)`
/// formulas whose range end was adjacent to the insertion point. Expand those
/// ranges to include the newly inserted rows.
///
/// **Important**: `insert_new_row` shifts cell positions but the same-sheet
/// formula rewriter is intentionally skipped (it only rewrites cross-sheet
/// references). So the formula text at the subtotal row still contains
/// pre-insert row references. If the range ended at `at_row - 1`, it was
/// adjacent and should be expanded to `subtotal_row - 1`.
///
/// Only unambiguous single-range `SUM(Ax:Ay)` patterns are touched.
/// Returns a list of warning strings for any skipped/ambiguous formulas.
fn expand_adjacent_sum_formulas(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    at_row: u32,
    count: u32,
) -> Result<(Vec<String>, u64)> {
    let mut warnings: Vec<String> = Vec::new();
    let mut expanded_count: u64 = 0;
    // The cell that was originally at `at_row` is now at `at_row + count`.
    let subtotal_row = at_row + count;
    let sum_re = simple_sum_range_regex();

    let sheet = book
        .get_sheet_by_name_mut(sheet_name).ok()
        .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;

    let max_col = sheet.get_highest_column();
    if max_col == 0 {
        return Ok((warnings, expanded_count));
    }

    for col in 1..=max_col {
        let Some(cell) = sheet.get_cell((col, subtotal_row)) else {
            continue;
        };
        if !cell.is_formula() {
            continue;
        }
        let formula_text = cell.get_formula().to_string();
        if formula_text.is_empty() {
            continue;
        }
        // Strip leading = if present (umya stores without it, but be safe).
        let formula_bare = formula_text.strip_prefix('=').unwrap_or(&formula_text);

        let Some(caps) = sum_re.captures(formula_bare) else {
            // Not a simple SUM(range) pattern.
            if formula_bare.to_ascii_uppercase().contains("SUM") {
                warnings.push(format!(
                    "WARN_SUM_EXPANSION_SKIPPED: {}!{} has complex SUM formula '{}'; skipped.",
                    sheet_name,
                    crate::utils::cell_address(col, subtotal_row),
                    formula_bare
                ));
            }
            continue;
        };

        let col1_str = caps.get(1).unwrap().as_str().to_ascii_uppercase();
        let row1: u32 = caps.get(2).unwrap().as_str().parse().unwrap_or(0);
        let col2_str = caps.get(3).unwrap().as_str().to_ascii_uppercase();
        let row2: u32 = caps.get(4).unwrap().as_str().parse().unwrap_or(0);

        // Only expand single-column SUM ranges.
        if col1_str != col2_str {
            warnings.push(format!(
                "WARN_SUM_EXPANSION_SKIPPED: {}!{} SUM spans columns {}:{} — not a single-column range; skipped.",
                sheet_name,
                crate::utils::cell_address(col, subtotal_row),
                col1_str,
                col2_str,
            ));
            continue;
        }

        // The formula text still has pre-insert references because the
        // same-sheet rewriter was skipped. Check adjacency: the range end
        // (row2) should be at_row - 1 (immediately above the insertion point).
        if at_row == 0 || row2 + 1 != at_row {
            // Range end wasn't immediately before the insertion point.
            continue;
        }
        if row1 > row2 || row1 == 0 {
            continue;
        }

        // Expand the range end to include the newly inserted rows.
        // New end = subtotal_row - 1 = at_row + count - 1.
        let desired_end = subtotal_row - 1;

        // Build expanded formula.
        let new_formula = format!("SUM({}{}:{}{})", col1_str, row1, col2_str, desired_end);

        let cell = sheet.get_cell_mut((col, subtotal_row));
        cell.set_formula(new_formula);
        cell.set_formula_result_default("");
        expanded_count += 1;
    }

    Ok((warnings, expanded_count))
}

// ---------------------------------------------------------------------------
// Clone-row helpers (ticket 4104)
// ---------------------------------------------------------------------------

/// Captured cell data from a template row.
#[derive(Debug, Clone)]
struct TemplateCellData {
    col: u32,
    value: String,
    formula: Option<String>,
    style: umya_spreadsheet::Style,
}

/// Capture every non-empty cell in `source_row` as template data.
fn capture_row_template(
    sheet: &umya_spreadsheet::Worksheet,
    source_row: u32,
) -> Result<Vec<TemplateCellData>> {
    let max_col = sheet.get_highest_column();
    let mut cells = Vec::new();
    for col in 1..=max_col {
        let Some(cell) = sheet.get_cell((col, source_row)) else {
            continue;
        };
        let value = cell.get_value().to_string();
        let formula = if cell.is_formula() {
            Some(cell.get_formula().to_string())
        } else {
            None
        };
        let style = cell.get_style().clone();
        cells.push(TemplateCellData {
            col,
            value,
            formula,
            style,
        });
    }
    Ok(cells)
}

/// Stamp template data into each inserted row, shifting formulas by row delta.
fn stamp_template_rows(
    sheet: &mut umya_spreadsheet::Worksheet,
    template: &[TemplateCellData],
    source_row: u32,
    insert_at: u32,
    count: u32,
) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    for copy_idx in 0..count {
        let dest_row = insert_at + copy_idx;
        let delta_row = dest_row as i32 - source_row as i32;
        for tpl in template {
            let dest_cell = sheet.get_cell_mut((tpl.col, dest_row));
            dest_cell.set_style(tpl.style.clone());

            if let Some(formula) = &tpl.formula {
                // Shift formula references by delta_row rows.
                match parse_base_formula(formula)
                    .and_then(|ast| shift_formula_ast(&ast, 0, delta_row, RelativeMode::Excel))
                {
                    Ok(shifted) => {
                        let shifted = shifted.strip_prefix('=').unwrap_or(&shifted).to_string();
                        dest_cell.set_formula(shifted);
                        dest_cell.set_formula_result_default("");
                    }
                    Err(err) => {
                        warnings.push(format!(
                            "WARN_CLONE_FORMULA_SHIFT: could not shift formula '{}' for row {}: {}; copied verbatim.",
                            formula, dest_row, err
                        ));
                        dest_cell.set_formula(formula.clone());
                        dest_cell.set_formula_result_default("");
                    }
                }
            } else {
                dest_cell.set_value(tpl.value.clone());
            }
        }
    }
    Ok(warnings)
}

fn sheet_part_matches(sheet_part: &str, old_name: &str) -> bool {
    let trimmed = sheet_part.trim();
    if let Some(stripped) = trimmed.strip_prefix('\'')
        && let Some(inner) = stripped.strip_suffix('\'')
    {
        return inner.replace("''", "'") == old_name;
    }
    trimmed == old_name
}

fn format_sheet_prefix_for_formula(sheet_name: &str) -> String {
    if sheet_name_needs_quoting_for_formula(sheet_name) {
        let escaped = sheet_name.replace('\'', "''");
        format!("'{escaped}'!")
    } else {
        format!("{sheet_name}!")
    }
}

fn sheet_name_needs_quoting_for_formula(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let bytes = name.as_bytes();
    if bytes[0].is_ascii_digit() {
        return true;
    }
    for &byte in bytes {
        match byte {
            b' ' | b'!' | b'"' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+'
            | b',' | b'-' | b'.' | b'/' | b':' | b';' | b'<' | b'=' | b'>' | b'?' | b'@' | b'['
            | b'\\' | b']' | b'^' | b'`' | b'{' | b'|' | b'}' | b'~' => return true,
            _ => {}
        }
    }
    let upper = name.to_uppercase();
    matches!(
        upper.as_str(),
        "TRUE" | "FALSE" | "NULL" | "REF" | "DIV" | "NAME" | "NUM" | "VALUE" | "N/A"
    )
}

pub(crate) struct StyleApplyResult {
    pub(crate) ops_applied: usize,
    pub(crate) summary: ChangeSummary,
}

pub(crate) fn stage_snapshot_path(fork_id: &str, change_id: &str) -> PathBuf {
    PathBuf::from("/tmp/mcp-staged").join(format!("{fork_id}_{change_id}.xlsx"))
}

pub(crate) struct ColumnSizeApplyResult {
    pub(crate) ops_applied: usize,
    pub(crate) summary: ChangeSummary,
}

fn parse_column_span(spec: &str) -> Result<(u32, u32)> {
    let raw = spec.trim();
    if raw.is_empty() {
        return Err(anyhow!("column range is empty"));
    }

    let raw = raw.replace(' ', "");
    let (start, end) = if let Some((a, b)) = raw.split_once(':') {
        (a, b)
    } else if let Some((a, b)) = raw.split_once('-') {
        (a, b)
    } else {
        (raw.as_str(), raw.as_str())
    };

    let start_idx = umya_spreadsheet::helper::coordinate::column_index_from_string(start);
    let end_idx = umya_spreadsheet::helper::coordinate::column_index_from_string(end);
    if start_idx == 0 || end_idx == 0 {
        return Err(anyhow!("invalid column span '{spec}'"));
    }
    let (min, max) = if start_idx <= end_idx {
        (start_idx, end_idx)
    } else {
        (end_idx, start_idx)
    };
    Ok((min, max))
}

pub(crate) fn apply_column_size_ops_to_file(
    path: &Path,
    sheet_name: &str,
    ops: &[ColumnSizeOp],
) -> Result<ColumnSizeApplyResult> {
    let mut book = crate::xlsx_import::read(path)?;
    let result = apply_column_size_ops_to_workbook(&mut book, sheet_name, ops)?;
    crate::xlsx_export::write(&book, path)?;
    Ok(result)
}

pub(crate) fn apply_column_size_ops_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    ops: &[ColumnSizeOp],
) -> Result<ColumnSizeApplyResult> {
    let sheet = book
        .get_sheet_by_name_mut(sheet_name).ok()
        .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;

    let mut affected_bounds: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    let mut columns_sized: u64 = 0;
    let mut auto_ops: u64 = 0;
    let mut width_ops: u64 = 0;

    for op in ops {
        let ColumnTarget::Columns { range } = &op.target;
        let (start_col, end_col) = parse_column_span(range)?;
        affected_bounds.push(range.clone());

        match &op.size {
            ColumnSizeSpec::Width { width_chars } => {
                width_ops += 1;
                for col in start_col..=end_col {
                    let col_dim = sheet.get_column_dimension_by_number_mut(&col);
                    col_dim.set_width(*width_chars);
                    col_dim.set_best_fit(false);
                    col_dim.set_auto_width(false);
                    columns_sized += 1;
                }
            }
            ColumnSizeSpec::Auto {
                min_width_chars,
                max_width_chars,
            } => {
                auto_ops += 1;

                let mut saw_formula_without_cached = false;
                for cell in sheet.cells() {
                    let col_num = cell.get_coordinate().get_col_num();
                    if col_num < start_col || col_num > end_col {
                        continue;
                    }
                    if cell.is_formula() && cell.get_value().is_empty() {
                        saw_formula_without_cached = true;
                        break;
                    }
                }
                if saw_formula_without_cached {
                    warnings.push(
                        "WARN_AUTOWIDTH_FORMULA_NO_CACHED: Autosize measured empty values for some formula cells; results may be too narrow. Recalc the sheet before autosize for best results."
                            .to_string(),
                    );
                }

                for col in start_col..=end_col {
                    sheet
                        .get_column_dimension_by_number_mut(&col)
                        .set_auto_width(true);
                }
                sheet.calculation_auto_width();

                for col in start_col..=end_col {
                    let col_dim = sheet.get_column_dimension_by_number_mut(&col);
                    col_dim.set_auto_width(false);
                    col_dim.set_best_fit(true);

                    let mut width = col_dim.get_width();
                    if let Some(min_width) = min_width_chars
                        && width < *min_width
                    {
                        width = *min_width;
                    }
                    if let Some(max_width) = max_width_chars
                        && width > *max_width
                    {
                        width = *max_width;
                    }
                    col_dim.set_width(width);
                    columns_sized += 1;
                }
            }
        }
    }

    let mut counts = BTreeMap::new();
    counts.insert("columns_sized".to_string(), columns_sized);
    counts.insert("auto_ops".to_string(), auto_ops);
    counts.insert("width_ops".to_string(), width_ops);

    Ok(ColumnSizeApplyResult {
        ops_applied: ops.len(),
        summary: ChangeSummary {
            op_kinds: vec!["column_size_batch".to_string()],
            affected_sheets: vec![sheet_name.to_string()],
            affected_bounds,
            counts,
            warnings,
            ..Default::default()
        },
    })
}

pub(crate) struct TransformApplyResult {
    pub(crate) ops_applied: usize,
    pub(crate) summary: ChangeSummary,
}

pub(crate) fn apply_transform_ops_to_file(
    path: &Path,
    ops: &[TransformOp],
) -> Result<TransformApplyResult> {
    let mut book = crate::xlsx_import::read(path)?;
    let result = apply_transform_ops_to_workbook(&mut book, ops)?;
    crate::xlsx_export::write(&book, path)?;
    Ok(result)
}

pub(crate) fn apply_transform_ops_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    ops: &[TransformOp],
) -> Result<TransformApplyResult> {
    let mut sheets: BTreeSet<String> = BTreeSet::new();
    let mut affected_bounds: Vec<String> = Vec::new();

    let mut cells_touched: u64 = 0;
    let mut cells_value_cleared: u64 = 0;
    let mut cells_formula_cleared: u64 = 0;
    let mut cells_skipped_keep_formulas: u64 = 0;

    let mut cells_value_set: u64 = 0;
    let mut cells_formula_set: u64 = 0;
    let mut cells_value_replaced: u64 = 0;
    let mut cells_formula_replaced: u64 = 0;

    for op in ops {
        match op {
            TransformOp::ClearRange {
                sheet_name,
                target,
                clear_values,
                clear_formulas,
            } => {
                let sheet = book
                    .get_sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                sheets.insert(sheet_name.clone());

                match target {
                    TransformTarget::Range { range } => {
                        let bounds = parse_range_bounds(range)?;
                        affected_bounds.push(range.clone());

                        for row in bounds.min_row..=bounds.max_row {
                            for col in bounds.min_col..=bounds.max_col {
                                let exists = sheet.get_cell((col, row)).is_some();
                                if !exists {
                                    continue;
                                }

                                let cell = sheet.get_cell_mut((col, row));
                                let was_formula = cell.is_formula();
                                cells_touched += 1;

                                if *clear_formulas && was_formula {
                                    cell.set_formula(String::new());
                                    cells_formula_cleared += 1;
                                }

                                if *clear_values {
                                    if was_formula && !*clear_formulas {
                                        cells_skipped_keep_formulas += 1;
                                    } else {
                                        if !cell.get_value().is_empty() {
                                            cells_value_cleared += 1;
                                        }
                                        cell.set_value(String::new());
                                    }
                                }
                            }
                        }
                    }
                    TransformTarget::Cells { cells } => {
                        affected_bounds.extend(cells.iter().cloned());
                        for addr in cells {
                            let exists = sheet.get_cell(addr.as_str()).is_some();
                            if !exists {
                                continue;
                            }

                            let cell = sheet.get_cell_mut(addr.as_str());
                            let was_formula = cell.is_formula();
                            cells_touched += 1;

                            if *clear_formulas && was_formula {
                                cell.set_formula(String::new());
                                cells_formula_cleared += 1;
                            }

                            if *clear_values {
                                if was_formula && !*clear_formulas {
                                    cells_skipped_keep_formulas += 1;
                                } else {
                                    if !cell.get_value().is_empty() {
                                        cells_value_cleared += 1;
                                    }
                                    cell.set_value(String::new());
                                }
                            }
                        }
                    }
                    TransformTarget::Region { .. } => {
                        return Err(anyhow!(
                            "region_id targets must be resolved before apply_transform_ops_to_file"
                        ));
                    }
                }
            }
            TransformOp::FillRange {
                sheet_name,
                target,
                value,
                is_formula,
                overwrite_formulas,
            } => {
                let sheet = book
                    .get_sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                sheets.insert(sheet_name.clone());

                match target {
                    TransformTarget::Range { range } => {
                        let bounds = parse_range_bounds(range)?;
                        affected_bounds.push(range.clone());

                        for row in bounds.min_row..=bounds.max_row {
                            for col in bounds.min_col..=bounds.max_col {
                                let cell = sheet.get_cell_mut((col, row));
                                cells_touched += 1;

                                if !*is_formula && cell.is_formula() {
                                    if !*overwrite_formulas {
                                        cells_skipped_keep_formulas += 1;
                                        continue;
                                    }
                                    cell.set_formula(String::new());
                                    cells_formula_cleared += 1;
                                }

                                if *is_formula {
                                    cell.set_formula(value.clone());
                                    cell.set_formula_result_default("");
                                    cells_formula_set += 1;
                                } else {
                                    cell.set_value(value.clone());
                                    cells_value_set += 1;
                                }
                            }
                        }
                    }
                    TransformTarget::Cells { cells } => {
                        affected_bounds.extend(cells.iter().cloned());
                        for addr in cells {
                            let cell = sheet.get_cell_mut(addr.as_str());
                            cells_touched += 1;

                            if !*is_formula && cell.is_formula() {
                                if !*overwrite_formulas {
                                    cells_skipped_keep_formulas += 1;
                                    continue;
                                }
                                cell.set_formula(String::new());
                                cells_formula_cleared += 1;
                            }

                            if *is_formula {
                                cell.set_formula(value.clone());
                                cell.set_formula_result_default("");
                                cells_formula_set += 1;
                            } else {
                                cell.set_value(value.clone());
                                cells_value_set += 1;
                            }
                        }
                    }
                    TransformTarget::Region { .. } => {
                        return Err(anyhow!(
                            "region_id targets must be resolved before apply_transform_ops_to_file"
                        ));
                    }
                }
            }
            TransformOp::ReplaceInRange {
                sheet_name,
                target,
                find,
                replace,
                match_mode,
                case_sensitive,
                include_formulas,
            } => {
                let sheet = book
                    .get_sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                sheets.insert(sheet_name.clone());

                if *match_mode == ReplaceMatchMode::Contains && !*case_sensitive {
                    return Err(anyhow!(
                        "match_mode 'contains' requires case_sensitive=true"
                    ));
                }

                let replace_value = |input: &str| -> Option<String> {
                    if *match_mode == ReplaceMatchMode::Exact {
                        if *case_sensitive {
                            (input == find).then(|| replace.clone())
                        } else {
                            input.eq_ignore_ascii_case(find).then(|| replace.clone())
                        }
                    } else if input.contains(find) {
                        Some(input.replace(find, replace))
                    } else {
                        None
                    }
                };

                match target {
                    TransformTarget::Range { range } => {
                        let bounds = parse_range_bounds(range)?;
                        affected_bounds.push(range.clone());

                        for row in bounds.min_row..=bounds.max_row {
                            for col in bounds.min_col..=bounds.max_col {
                                let exists = sheet.get_cell((col, row)).is_some();
                                if !exists {
                                    continue;
                                }

                                let cell = sheet.get_cell_mut((col, row));
                                cells_touched += 1;

                                if cell.is_formula() {
                                    if !*include_formulas {
                                        cells_skipped_keep_formulas += 1;
                                        continue;
                                    }

                                    let formula = cell.get_formula().to_string();
                                    if formula.is_empty() {
                                        continue;
                                    }
                                    if let Some(next) = replace_value(&formula) {
                                        cell.set_formula(next);
                                        cell.set_formula_result_default("");
                                        cells_formula_replaced += 1;
                                    }
                                    continue;
                                }

                                let value = cell.get_value().to_string();
                                if value.is_empty() {
                                    continue;
                                }
                                if let Some(next) = replace_value(&value) {
                                    cell.set_value(next);
                                    cells_value_replaced += 1;
                                }
                            }
                        }
                    }
                    TransformTarget::Cells { cells } => {
                        affected_bounds.extend(cells.iter().cloned());
                        for addr in cells {
                            let exists = sheet.get_cell(addr.as_str()).is_some();
                            if !exists {
                                continue;
                            }

                            let cell = sheet.get_cell_mut(addr.as_str());
                            cells_touched += 1;

                            if cell.is_formula() {
                                if !*include_formulas {
                                    cells_skipped_keep_formulas += 1;
                                    continue;
                                }

                                let formula = cell.get_formula().to_string();
                                if formula.is_empty() {
                                    continue;
                                }
                                if let Some(next) = replace_value(&formula) {
                                    cell.set_formula(next);
                                    cell.set_formula_result_default("");
                                    cells_formula_replaced += 1;
                                }
                                continue;
                            }

                            let value = cell.get_value().to_string();
                            if value.is_empty() {
                                continue;
                            }
                            if let Some(next) = replace_value(&value) {
                                cell.set_value(next);
                                cells_value_replaced += 1;
                            }
                        }
                    }
                    TransformTarget::Region { .. } => {
                        return Err(anyhow!(
                            "region_id targets must be resolved before apply_transform_ops_to_file"
                        ));
                    }
                }
            }
            TransformOp::WriteMatrix {
                sheet_name,
                anchor,
                rows,
                overwrite_formulas,
            } => {
                let sheet = book
                    .get_sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                sheets.insert(sheet_name.clone());

                let (anchor_col, anchor_row) = parse_cell_ref(anchor)?;

                let mut max_row = anchor_row;
                let mut max_col = anchor_col;

                for (r_idx, row) in rows.iter().enumerate() {
                    let r = anchor_row + r_idx as u32;
                    if r > max_row {
                        max_row = r;
                    }
                    for (c_idx, cell_opt) in row.iter().enumerate() {
                        let c = anchor_col + c_idx as u32;
                        if c > max_col {
                            max_col = c;
                        }

                        let Some(cell_data) = cell_opt else {
                            continue;
                        };

                        let cell = sheet.get_cell_mut((c, r));
                        cells_touched += 1;

                        if cell.is_formula() {
                            if !*overwrite_formulas {
                                cells_skipped_keep_formulas += 1;
                                continue;
                            }
                            cell.set_formula(String::new());
                            cells_formula_cleared += 1;
                        }

                        match cell_data {
                            MatrixCell::Value(v) => {
                                let val_str = match v {
                                    serde_json::Value::Null => String::new(),
                                    serde_json::Value::Bool(b) => b.to_string(),
                                    serde_json::Value::Number(n) => n.to_string(),
                                    serde_json::Value::String(s) => s.clone(),
                                    serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                                        v.to_string()
                                    }
                                };
                                cell.set_value(val_str);
                                cells_value_set += 1;
                            }
                            MatrixCell::Formula(f) => {
                                let f_str = f.strip_prefix('=').unwrap_or(f);
                                cell.set_formula(f_str);
                                cell.set_formula_result_default("");
                                cells_formula_set += 1;
                            }
                        }
                    }
                }

                affected_bounds.push(format!(
                    "{}:{}",
                    crate::utils::cell_address(anchor_col, anchor_row),
                    crate::utils::cell_address(max_col, max_row)
                ));
            }
        }
    }

    let mut counts = BTreeMap::new();
    counts.insert("cells_touched".to_string(), cells_touched);
    counts.insert("cells_value_cleared".to_string(), cells_value_cleared);
    counts.insert("cells_formula_cleared".to_string(), cells_formula_cleared);
    counts.insert(
        "cells_skipped_keep_formulas".to_string(),
        cells_skipped_keep_formulas,
    );

    counts.insert("cells_value_set".to_string(), cells_value_set);
    counts.insert("cells_formula_set".to_string(), cells_formula_set);
    counts.insert("cells_value_replaced".to_string(), cells_value_replaced);
    counts.insert("cells_formula_replaced".to_string(), cells_formula_replaced);

    let summary = ChangeSummary {
        op_kinds: vec!["transform_batch".to_string()],
        affected_sheets: sheets.into_iter().collect(),
        affected_bounds,
        counts,
        warnings: Vec::new(),
        ..Default::default()
    };

    Ok(TransformApplyResult {
        ops_applied: ops.len(),
        summary,
    })
}

// ── replace_in_formulas core ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReplaceInFormulasOp {
    pub sheet_name: String,
    pub find: String,
    pub replace: String,
    /// Optional A1 range to scope the replacement; defaults to the used range.
    #[serde(default)]
    pub range: Option<String>,
    /// Enable regex mode (default: false).
    #[serde(default)]
    pub regex: bool,
    /// Case-sensitive matching (default: true).
    #[serde(default = "default_replace_case_sensitive")]
    pub case_sensitive: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FormulaReplaceSample {
    pub address: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug)]
pub struct ReplaceInFormulasApplyResult {
    pub formulas_checked: u64,
    pub formulas_changed: u64,
    pub samples: Vec<FormulaReplaceSample>,
    pub warnings: Vec<String>,
    pub formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
}

const REPLACE_SAMPLE_LIMIT: usize = 20;

pub fn apply_replace_in_formulas_to_file(
    path: &Path,
    op: &ReplaceInFormulasOp,
    policy: FormulaParsePolicy,
) -> Result<ReplaceInFormulasApplyResult> {
    let mut book = crate::xlsx_import::read(path)?;
    let result = apply_replace_in_formulas_to_workbook(&mut book, op, policy)?;
    crate::xlsx_export::write(&book, path)?;
    Ok(result)
}

pub(crate) fn apply_replace_in_formulas_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    op: &ReplaceInFormulasOp,
    policy: FormulaParsePolicy,
) -> Result<ReplaceInFormulasApplyResult> {
    let sheet = book
        .get_sheet_by_name_mut(&op.sheet_name).ok()
        .ok_or_else(|| anyhow!("sheet '{}' not found", op.sheet_name))?;

    // Determine bounds (optional range or used range).
    let (min_col, min_row, max_col, max_row) = if let Some(range) = &op.range {
        let bounds = parse_range_bounds(range)?;
        (
            bounds.min_col,
            bounds.min_row,
            bounds.max_col,
            bounds.max_row,
        )
    } else {
        let (hc, hr) = sheet.get_highest_column_and_row();
        (1, 1, hc.max(1), hr.max(1))
    };

    // Build matcher.
    let compiled_regex: Option<Regex> = if op.regex {
        let pattern = if op.case_sensitive {
            op.find.clone()
        } else {
            format!("(?i){}", op.find)
        };
        Some(Regex::new(&pattern).map_err(|e| anyhow!("invalid regex pattern: {}", e))?)
    } else {
        None
    };

    let replace_formula = |formula: &str| -> Option<String> {
        if let Some(re) = &compiled_regex {
            let result = re.replace_all(formula, op.replace.as_str());
            if result != formula {
                Some(result.into_owned())
            } else {
                None
            }
        } else if op.case_sensitive {
            if formula.contains(&op.find) {
                Some(formula.replace(&op.find, &op.replace))
            } else {
                None
            }
        } else {
            // Case-insensitive plain text replacement.
            let find_lower = op.find.to_ascii_lowercase();
            let formula_lower = formula.to_ascii_lowercase();
            if !formula_lower.contains(&find_lower) {
                return None;
            }
            // Rebuild with original casing for non-matched parts.
            let mut result = String::with_capacity(formula.len());
            let mut cursor = 0usize;
            while let Some(pos) = formula_lower[cursor..].find(&find_lower) {
                result.push_str(&formula[cursor..cursor + pos]);
                result.push_str(&op.replace);
                cursor += pos + op.find.len();
            }
            result.push_str(&formula[cursor..]);
            Some(result)
        }
    };

    let mut formulas_checked: u64 = 0;
    let mut formulas_changed: u64 = 0;
    let mut samples: Vec<FormulaReplaceSample> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut formula_parse_diagnostics_builder = FormulaParseDiagnosticsBuilder::new(policy);

    for row in min_row..=max_row {
        for col in min_col..=max_col {
            let exists = sheet.get_cell((col, row)).is_some();
            if !exists {
                continue;
            }
            let cell = sheet.get_cell_mut((col, row));
            if !cell.is_formula() {
                continue;
            }
            let formula = cell.get_formula().to_string();
            if formula.is_empty() {
                continue;
            }
            formulas_checked += 1;

            if let Some(next) = replace_formula(&formula) {
                let address = crate::utils::cell_address(col, row);

                if policy != FormulaParsePolicy::Off
                    && let Err(err_msg) = validate_formula(&next)
                {
                    if policy == FormulaParsePolicy::Fail {
                        bail!(
                            "{}replaced formula at {} failed parse: {}",
                            FORMULA_PARSE_FAILED_PREFIX,
                            address,
                            err_msg
                        );
                    }
                    formula_parse_diagnostics_builder.record_error(
                        &op.sheet_name,
                        &address,
                        &next,
                        &err_msg,
                    );
                    // Warn mode: keep the original formula untouched.
                    continue;
                }

                if samples.len() < REPLACE_SAMPLE_LIMIT {
                    samples.push(FormulaReplaceSample {
                        address,
                        before: formula.clone(),
                        after: next.clone(),
                    });
                }
                cell.set_formula(next);
                cell.set_formula_result_default("");
                formulas_changed += 1;
            }
        }
    }

    if formulas_changed == 0 {
        warnings.push("WARN_NO_MATCH: no formula text matched the find pattern".to_string());
    }

    let formula_parse_diagnostics = if formula_parse_diagnostics_builder.has_errors() {
        Some(formula_parse_diagnostics_builder.build())
    } else {
        None
    };

    Ok(ReplaceInFormulasApplyResult {
        formulas_checked,
        formulas_changed,
        samples,
        warnings,
        formula_parse_diagnostics,
    })
}

// ── replace_in_formulas MCP fork tool ─────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplaceInFormulasParams {
    pub fork_id: String,
    pub sheet_name: String,
    pub find: String,
    pub replace: String,
    /// Optional A1 range; defaults to the used range.
    #[serde(default)]
    pub range: Option<String>,
    /// Enable regex mode (default: false).
    #[serde(default)]
    pub regex: bool,
    /// Case-sensitive matching (default: true).
    #[serde(default = "default_replace_case_sensitive")]
    pub case_sensitive: bool,
    #[serde(default)]
    pub mode: Option<BatchMode>,
    #[serde(default)]
    pub label: Option<String>,
    /// Formula parse policy: fail, warn (default), or off.
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ReplaceInFormulasResponse {
    pub fork_id: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_id: Option<String>,
    pub formulas_checked: u64,
    pub formulas_changed: u64,
    pub recalc_needed: bool,
    pub samples: Vec<FormulaReplaceSample>,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReplaceInFormulasStagedPayload {
    op: ReplaceInFormulasOp,
}

pub async fn replace_in_formulas(
    state: Arc<AppState>,
    params: ReplaceInFormulasParams,
) -> Result<ReplaceInFormulasResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;
    let work_path = fork_ctx.work_path.clone();

    let op = ReplaceInFormulasOp {
        sheet_name: params.sheet_name.clone(),
        find: params.find.clone(),
        replace: params.replace.clone(),
        range: params.range.clone(),
        regex: params.regex,
        case_sensitive: params.case_sensitive,
    };

    let policy =
        params
            .formula_parse_policy
            .unwrap_or(FormulaParsePolicy::default_for_command_class(
                CommandClass::BatchWrite,
            ));

    let mode = params.mode.unwrap_or_default();

    if mode.is_preview() {
        let change_id = make_short_random_id("chg", 12);
        let snapshot_path = stage_snapshot_path(&params.fork_id, &change_id);
        std::fs::create_dir_all(snapshot_path.parent().unwrap())?;
        std::fs::copy(&work_path, &snapshot_path)?;

        let snapshot_for_apply = snapshot_path.clone();
        let op_clone = op.clone();
        let result = tokio::task::spawn_blocking(move || {
            apply_replace_in_formulas_to_file(&snapshot_for_apply, &op_clone, policy)
        })
        .await??;

        let staged_op = StagedOp {
            kind: "replace_in_formulas".to_string(),
            payload: serde_json::to_value(ReplaceInFormulasStagedPayload { op: op.clone() })?,
        };

        let summary = ChangeSummary {
            op_kinds: vec!["replace_in_formulas".to_string()],
            affected_sheets: vec![op.sheet_name.clone()],
            affected_bounds: op.range.clone().into_iter().collect(),
            counts: {
                let mut c = BTreeMap::new();
                c.insert("formulas_checked".to_string(), result.formulas_checked);
                c.insert("formulas_changed".to_string(), result.formulas_changed);
                c
            },
            warnings: result.warnings.clone(),
            ..Default::default()
        };

        let staged = StagedChange {
            change_id: change_id.clone(),
            created_at: Utc::now(),
            label: params.label.clone(),
            ops: vec![staged_op],
            summary,
            fork_path_snapshot: Some(snapshot_path),
        };

        registry.add_staged_change(&params.fork_id, staged)?;

        Ok(ReplaceInFormulasResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: Some(change_id),
            formulas_checked: result.formulas_checked,
            formulas_changed: result.formulas_changed,
            recalc_needed: fork_ctx.recalc_needed || result.formulas_changed > 0,
            samples: result.samples,
            warnings: result.warnings,
            formula_parse_diagnostics: result.formula_parse_diagnostics,
        })
    } else {
        let prior_recalc_needed = fork_ctx.recalc_needed;
        let op_clone = op.clone();
        let work_path_for_apply = work_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            apply_replace_in_formulas_to_file(&work_path_for_apply, &op_clone, policy)
        })
        .await??;

        if result.formulas_changed > 0 {
            registry.with_fork_mut(&params.fork_id, |ctx| {
                ctx.recalc_needed = true;
                Ok(())
            })?;
            let fork_workbook_id = WorkbookId(params.fork_id.clone());
            let _ = state.close_workbook(&fork_workbook_id);
        }

        Ok(ReplaceInFormulasResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: None,
            formulas_checked: result.formulas_checked,
            formulas_changed: result.formulas_changed,
            recalc_needed: prior_recalc_needed || result.formulas_changed > 0,
            samples: result.samples,
            warnings: result.warnings,
            formula_parse_diagnostics: result.formula_parse_diagnostics,
        })
    }
}

pub(crate) fn apply_style_ops_to_file(path: &Path, ops: &[StyleOp]) -> Result<StyleApplyResult> {
    let mut book = crate::xlsx_import::read(path)?;
    let result = apply_style_ops_to_workbook(&mut book, ops)?;
    crate::xlsx_export::write(&book, path)?;
    Ok(result)
}

pub(crate) fn apply_style_ops_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    ops: &[StyleOp],
) -> Result<StyleApplyResult> {
    use crate::styles::{
        StylePatchMode, apply_style_patch, descriptor_from_style, stable_style_id,
    };

    let mut sheets: BTreeSet<String> = BTreeSet::new();
    let mut affected_bounds: Vec<String> = Vec::new();
    let mut cells_touched: u64 = 0;
    let mut cells_style_changed: u64 = 0;

    for op in ops {
        let sheet = book
            .get_sheet_by_name_mut(&op.sheet_name).ok()
            .ok_or_else(|| anyhow!("sheet '{}' not found", op.sheet_name))?;
        sheets.insert(op.sheet_name.clone());

        let op_mode = op.op_mode.unwrap_or(StylePatchMode::Merge);

        match &op.target {
            StyleTarget::Range { range } => {
                let bounds = parse_range_bounds(range)?;
                affected_bounds.push(range.clone());
                for row in bounds.min_row..=bounds.max_row {
                    for col in bounds.min_col..=bounds.max_col {
                        let addr = crate::utils::cell_address(col, row);
                        let cell = sheet.get_cell_mut(addr.as_str());
                        let before = stable_style_id(&descriptor_from_style(cell.get_style()));
                        let next_style = apply_style_patch(cell.get_style(), &op.patch, op_mode);
                        cell.set_style(next_style);
                        let after = stable_style_id(&descriptor_from_style(cell.get_style()));
                        cells_touched += 1;
                        if before != after {
                            cells_style_changed += 1;
                        }
                    }
                }
            }
            StyleTarget::Cells { cells } => {
                affected_bounds.extend(cells.iter().cloned());
                for addr in cells {
                    let cell = sheet.get_cell_mut(addr.as_str());
                    let before = stable_style_id(&descriptor_from_style(cell.get_style()));
                    let next_style = apply_style_patch(cell.get_style(), &op.patch, op_mode);
                    cell.set_style(next_style);
                    let after = stable_style_id(&descriptor_from_style(cell.get_style()));
                    cells_touched += 1;
                    if before != after {
                        cells_style_changed += 1;
                    }
                }
            }
            StyleTarget::Region { .. } => {
                return Err(anyhow!(
                    "region_id targets must be resolved before apply_style_ops_to_file"
                ));
            }
        }
    }

    let mut counts = BTreeMap::new();
    counts.insert("cells_touched".to_string(), cells_touched);
    counts.insert("cells_style_changed".to_string(), cells_style_changed);

    let summary = ChangeSummary {
        op_kinds: vec!["style_batch".to_string()],
        affected_sheets: sheets.into_iter().collect(),
        affected_bounds,
        counts,
        warnings: Vec::new(),
        ..Default::default()
    };

    Ok(StyleApplyResult {
        ops_applied: ops.len(),
        summary,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetEditsParams {
    pub fork_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GetEditsResponse {
    pub fork_id: String,
    pub edits: Vec<EditRecord>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct EditRecord {
    pub timestamp: String,
    pub sheet: String,
    pub address: String,
    pub value: String,
    pub is_formula: bool,
}

pub async fn get_edits(state: Arc<AppState>, params: GetEditsParams) -> Result<GetEditsResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;

    let edits: Vec<EditRecord> = fork_ctx
        .edits
        .iter()
        .map(|e| EditRecord {
            timestamp: e.timestamp.to_rfc3339(),
            sheet: e.sheet.clone(),
            address: e.address.clone(),
            value: e.value.clone(),
            is_formula: e.is_formula,
        })
        .collect();

    Ok(GetEditsResponse {
        fork_id: params.fork_id,
        edits,
    })
}

fn default_get_changeset_limit() -> u32 {
    200
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetChangesetParams {
    pub fork_id: String,
    pub sheet_name: Option<String>,
    #[serde(default = "default_get_changeset_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub summary_only: bool,
    #[serde(default)]
    pub include_types: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_types: Option<Vec<String>>,
    #[serde(default)]
    pub include_subtypes: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_subtypes: Option<Vec<String>>,
}

impl Default for GetChangesetParams {
    fn default() -> Self {
        Self {
            fork_id: String::new(),
            sheet_name: None,
            limit: default_get_changeset_limit(),
            offset: 0,
            summary_only: false,
            include_types: None,
            exclude_types: None,
            include_subtypes: None,
            exclude_subtypes: None,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ChangesetSummary {
    pub total_changes: u32,
    pub returned_changes: u32,
    pub truncated: bool,
    pub next_offset: Option<u32>,
    pub counts_by_kind: BTreeMap<String, u32>,
    pub counts_by_type: BTreeMap<String, u32>,
    pub counts_by_subtype: BTreeMap<String, u32>,
    pub affected_sheets: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GetChangesetResponse {
    pub fork_id: String,
    pub base_workbook: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_base_workbook: Option<String>,
    pub changes: Vec<crate::diff::Change>,
    pub summary: ChangesetSummary,
}

fn normalize_filter(values: &Option<Vec<String>>) -> Option<BTreeSet<String>> {
    values.as_ref().map(|items| {
        items
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect::<BTreeSet<_>>()
    })
}

fn change_kind_key(change: &crate::diff::Change) -> &'static str {
    match change {
        crate::diff::Change::Cell(_) => "cell",
        crate::diff::Change::Table(_) => "table",
        crate::diff::Change::Name(_) => "name",
    }
}

fn change_type_key(change: &crate::diff::Change) -> &'static str {
    use crate::diff::merge::CellDiff;
    match change {
        crate::diff::Change::Cell(cell) => match &cell.diff {
            CellDiff::Added { .. } => "added",
            CellDiff::Deleted { .. } => "deleted",
            CellDiff::Modified { .. } => "modified",
        },
        crate::diff::Change::Table(table) => match table {
            crate::diff::tables::TableDiff::TableAdded { .. } => "table_added",
            crate::diff::tables::TableDiff::TableDeleted { .. } => "table_deleted",
            crate::diff::tables::TableDiff::TableModified { .. } => "table_modified",
        },
        crate::diff::Change::Name(name) => match name {
            crate::diff::names::NameDiff::NameAdded { .. } => "name_added",
            crate::diff::names::NameDiff::NameDeleted { .. } => "name_deleted",
            crate::diff::names::NameDiff::NameModified { .. } => "name_modified",
        },
    }
}

fn change_subtype_key(change: &crate::diff::Change) -> Option<&'static str> {
    use crate::diff::merge::{CellDiff, ModificationType};
    match change {
        crate::diff::Change::Cell(cell) => match &cell.diff {
            CellDiff::Modified { subtype, .. } => Some(match subtype {
                ModificationType::FormulaEdit => "formula_edit",
                ModificationType::RecalcResult => "recalc_result",
                ModificationType::ValueEdit => "value_edit",
                ModificationType::StyleEdit => "style_edit",
            }),
            _ => None,
        },
        _ => None,
    }
}

fn change_sheet_name(change: &crate::diff::Change) -> Option<&str> {
    match change {
        crate::diff::Change::Cell(cell) => Some(cell.sheet.as_str()),
        crate::diff::Change::Table(table) => match table {
            crate::diff::tables::TableDiff::TableAdded { sheet, .. }
            | crate::diff::tables::TableDiff::TableDeleted { sheet, .. }
            | crate::diff::tables::TableDiff::TableModified { sheet, .. } => Some(sheet.as_str()),
        },
        crate::diff::Change::Name(name) => match name {
            crate::diff::names::NameDiff::NameAdded { scope_sheet, .. }
            | crate::diff::names::NameDiff::NameDeleted { scope_sheet, .. }
            | crate::diff::names::NameDiff::NameModified { scope_sheet, .. } => {
                scope_sheet.as_deref()
            }
        },
    }
}

fn change_passes_filters(
    change: &crate::diff::Change,
    include_types: &Option<BTreeSet<String>>,
    exclude_types: &Option<BTreeSet<String>>,
    include_subtypes: &Option<BTreeSet<String>>,
    exclude_subtypes: &Option<BTreeSet<String>>,
) -> bool {
    let type_key = change_type_key(change);
    let subtype_key = change_subtype_key(change);

    if let Some(include) = include_types
        && !include.contains(type_key)
    {
        return false;
    }
    if let Some(exclude) = exclude_types
        && exclude.contains(type_key)
    {
        return false;
    }

    if let Some(include) = include_subtypes
        && subtype_key.is_none_or(|subtype| !include.contains(subtype))
    {
        return false;
    }
    if let Some(exclude) = exclude_subtypes
        && subtype_key.is_some_and(|subtype| exclude.contains(subtype))
    {
        return false;
    }

    true
}

pub async fn get_changeset(
    state: Arc<AppState>,
    params: GetChangesetParams,
) -> Result<GetChangesetResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;

    let raw_changes = tokio::task::spawn_blocking({
        let base_path = fork_ctx.base_path.clone();
        let work_path = fork_ctx.work_path.clone();
        let sheet_filter = params.sheet_name.clone();
        move || {
            crate::core::diff::calculate_changeset(&base_path, &work_path, sheet_filter.as_deref())
        }
    })
    .await??;

    let include_types = normalize_filter(&params.include_types);
    let exclude_types = normalize_filter(&params.exclude_types);
    let include_subtypes = normalize_filter(&params.include_subtypes);
    let exclude_subtypes = normalize_filter(&params.exclude_subtypes);

    let mut affected_sheets: BTreeSet<String> = BTreeSet::new();
    let mut counts_by_kind: BTreeMap<String, u32> = BTreeMap::new();
    let mut counts_by_type: BTreeMap<String, u32> = BTreeMap::new();
    let mut counts_by_subtype: BTreeMap<String, u32> = BTreeMap::new();

    let mut filtered: Vec<crate::diff::Change> = Vec::new();
    for change in raw_changes {
        if !change_passes_filters(
            &change,
            &include_types,
            &exclude_types,
            &include_subtypes,
            &exclude_subtypes,
        ) {
            continue;
        }

        *counts_by_kind
            .entry(change_kind_key(&change).to_string())
            .or_default() += 1;
        *counts_by_type
            .entry(change_type_key(&change).to_string())
            .or_default() += 1;
        if let Some(subtype) = change_subtype_key(&change) {
            *counts_by_subtype.entry(subtype.to_string()).or_default() += 1;
        }
        if let Some(sheet) = change_sheet_name(&change) {
            affected_sheets.insert(sheet.to_string());
        }

        filtered.push(change);
    }

    let limit = params.limit.clamp(1, 2000) as usize;
    let offset = params.offset as usize;
    let total = filtered.len();

    let (returned_changes, changes, truncated, next_offset) = if params.summary_only {
        (0u32, Vec::new(), false, None)
    } else {
        let end = offset.saturating_add(limit);
        let truncated = end < total;
        let next_offset = truncated.then_some(end as u32);
        let changes: Vec<_> = filtered.into_iter().skip(offset).take(limit).collect();
        (changes.len() as u32, changes, truncated, next_offset)
    };

    let summary = ChangesetSummary {
        total_changes: total as u32,
        returned_changes,
        truncated,
        next_offset,
        counts_by_kind,
        counts_by_type,
        counts_by_subtype,
        affected_sheets: affected_sheets.into_iter().collect(),
    };

    Ok(GetChangesetResponse {
        fork_id: params.fork_id,
        base_workbook: fork_ctx.base_path.display().to_string(),
        client_base_workbook: state
            .config()
            .map_path_for_client(&fork_ctx.base_path)
            .map(|p| p.display().to_string()),
        changes,
        summary,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecalculateParams {
    pub fork_id: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub backend: Option<RecalcBackendKind>,
}

fn default_timeout() -> u64 {
    30_000
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RecalculateResponse {
    pub fork_id: String,
    pub duration_ms: u64,
    pub backend: String,
    pub state: crate::model::EvaluationState,
    pub evaluation_coverage: crate::model::EvaluationCoverage,
    /// "completed" or "completed_with_errors" — lets a skimming agent detect
    /// partial failure without inspecting eval_errors.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cells_evaluated: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eval_errors: Option<Vec<String>>,
}

pub async fn recalculate(
    state: Arc<AppState>,
    params: RecalculateParams,
) -> Result<RecalculateResponse> {
    let backend = state
        .recalc_backend(params.backend)
        .ok_or_else(|| anyhow!("requested recalc backend not available"))?;

    recalculate_with_backend(state, params, backend).await
}

pub async fn recalculate_with_backend(
    state: Arc<AppState>,
    params: RecalculateParams,
    backend: Arc<dyn RecalcBackend>,
) -> Result<RecalculateResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let semaphore = state
        .recalc_semaphore()
        .ok_or_else(|| anyhow!("recalc semaphore not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;

    let _permit = semaphore
        .0
        .acquire()
        .await
        .map_err(|e| anyhow!("failed to acquire recalc permit: {}", e))?;

    let timeout_ms = if params.timeout_ms == 0 {
        None
    } else {
        Some(params.timeout_ms)
    };
    let result =
        crate::core::recalc::execute_with_backend(&fork_ctx.work_path, timeout_ms, backend).await?;

    let evaluation_complete = result.evaluation_coverage.is_complete_and_fresh();
    registry.with_fork_mut(&params.fork_id, |ctx| {
        ctx.recalc_needed = !evaluation_complete;
        Ok(())
    })?;

    let fork_workbook_id = WorkbookId(params.fork_id.clone());
    let _ = state.close_workbook(&fork_workbook_id);

    let status = if result.state == crate::model::EvaluationState::Clean {
        "completed"
    } else {
        "completed_with_errors"
    };
    let error_count = result.eval_errors.as_ref().map(Vec::len);
    Ok(RecalculateResponse {
        fork_id: params.fork_id,
        duration_ms: result.duration_ms,
        backend: result.backend,
        state: result.state,
        evaluation_coverage: result.evaluation_coverage,
        status: status.to_string(),
        error_count,
        cells_evaluated: result.cells_evaluated,
        eval_errors: result.eval_errors,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListForksParams {}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListForksResponse {
    pub forks: Vec<ForkSummary>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ForkSummary {
    pub fork_id: String,
    pub base_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_base_path: Option<String>,
    pub age_seconds: u64,
    pub edit_count: usize,
    pub recalc_needed: bool,
}

pub async fn list_forks(
    state: Arc<AppState>,
    _params: ListForksParams,
) -> Result<ListForksResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let config = state.config();
    let forks: Vec<ForkSummary> = registry
        .list_forks()
        .into_iter()
        .map(|f| {
            let base_path = f.base_path;
            let client_base_path = if config.path_mappings.is_empty() {
                None
            } else {
                config
                    .map_path_for_client(PathBuf::from(&base_path))
                    .map(|p| p.display().to_string())
            };
            ForkSummary {
                fork_id: f.fork_id,
                base_path,
                client_base_path,
                age_seconds: f.created_at.elapsed().as_secs(),
                edit_count: f.edit_count,
                recalc_needed: f.recalc_needed,
            }
        })
        .collect();

    Ok(ListForksResponse { forks })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiscardForkParams {
    pub fork_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DiscardForkResponse {
    pub fork_id: String,
    pub discarded: bool,
}

pub async fn discard_fork(
    state: Arc<AppState>,
    params: DiscardForkParams,
) -> Result<DiscardForkResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    registry.discard_fork(&params.fork_id)?;

    Ok(DiscardForkResponse {
        fork_id: params.fork_id,
        discarded: true,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveForkParams {
    pub fork_id: String,
    /// Target path to save to. If omitted, saves to original location (requires --allow-overwrite).
    pub target_path: Option<String>,
    /// If true, discard the fork after saving. If false, fork remains active for further edits.
    #[serde(default = "default_drop_fork")]
    pub drop_fork: bool,
}

fn default_drop_fork() -> bool {
    true
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SaveForkResponse {
    pub fork_id: String,
    pub saved_to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_saved_to: Option<String>,
    pub fork_dropped: bool,
}

pub async fn save_fork(state: Arc<AppState>, params: SaveForkParams) -> Result<SaveForkResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;
    let config = state.config();
    let workspace_root = &config.workspace_root;

    let (target, is_overwrite) = match params.target_path {
        Some(p) => {
            let resolved = config.resolve_user_path(&p);
            let is_overwrite = if resolved.exists() {
                let base_canon = fork_ctx.base_path.canonicalize().map_err(|e| {
                    anyhow!("failed to canonicalize base_path for overwrite check: {e}")
                })?;
                let target_canon = resolved.canonicalize().map_err(|e| {
                    anyhow!("failed to canonicalize target_path for overwrite check: {e}")
                })?;
                target_canon == base_canon
            } else {
                false
            };
            (resolved, is_overwrite)
        }
        None => (fork_ctx.base_path.clone(), true),
    };

    if is_overwrite && !config.allow_overwrite {
        return Err(anyhow!(
            "overwriting original file is disabled. Use --allow-overwrite flag or specify a different target_path"
        ));
    }

    let base_path = fork_ctx.base_path.clone();
    registry.save_fork(&params.fork_id, &target, workspace_root, params.drop_fork)?;

    if is_overwrite {
        state.evict_by_path(&base_path);
    }

    Ok(SaveForkResponse {
        fork_id: params.fork_id,
        saved_to: target.display().to_string(),
        client_saved_to: config
            .map_path_for_client(&target)
            .map(|p| p.display().to_string()),
        fork_dropped: params.drop_fork,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckpointForkParams {
    pub fork_id: String,
    pub label: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CheckpointInfo {
    pub checkpoint_id: String,
    pub created_at: String,
    pub label: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CheckpointForkResponse {
    pub fork_id: String,
    pub checkpoint: CheckpointInfo,
    pub total_checkpoints: usize,
}

pub async fn checkpoint_fork(
    state: Arc<AppState>,
    params: CheckpointForkParams,
) -> Result<CheckpointForkResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    registry.get_fork(&params.fork_id)?;
    let checkpoint = registry.create_checkpoint(&params.fork_id, params.label.clone())?;
    let total = registry.list_checkpoints(&params.fork_id)?.len();

    Ok(CheckpointForkResponse {
        fork_id: params.fork_id,
        checkpoint: CheckpointInfo {
            checkpoint_id: checkpoint.checkpoint_id,
            created_at: checkpoint.created_at.to_rfc3339(),
            label: checkpoint.label,
        },
        total_checkpoints: total,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListCheckpointsParams {
    pub fork_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListCheckpointsResponse {
    pub fork_id: String,
    pub checkpoints: Vec<CheckpointInfo>,
}

pub async fn list_checkpoints(
    state: Arc<AppState>,
    params: ListCheckpointsParams,
) -> Result<ListCheckpointsResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let checkpoints = registry.list_checkpoints(&params.fork_id)?;
    let checkpoints = checkpoints
        .into_iter()
        .map(|cp| CheckpointInfo {
            checkpoint_id: cp.checkpoint_id,
            created_at: cp.created_at.to_rfc3339(),
            label: cp.label,
        })
        .collect();

    Ok(ListCheckpointsResponse {
        fork_id: params.fork_id,
        checkpoints,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RestoreCheckpointParams {
    pub fork_id: String,
    pub checkpoint_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RestoreCheckpointResponse {
    pub fork_id: String,
    pub restored_checkpoint: CheckpointInfo,
}

pub async fn restore_checkpoint(
    state: Arc<AppState>,
    params: RestoreCheckpointParams,
) -> Result<RestoreCheckpointResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let checkpoint = registry.restore_checkpoint(&params.fork_id, &params.checkpoint_id)?;
    let fork_workbook_id = WorkbookId(params.fork_id.clone());
    let _ = state.close_workbook(&fork_workbook_id);

    Ok(RestoreCheckpointResponse {
        fork_id: params.fork_id,
        restored_checkpoint: CheckpointInfo {
            checkpoint_id: checkpoint.checkpoint_id,
            created_at: checkpoint.created_at.to_rfc3339(),
            label: checkpoint.label,
        },
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteCheckpointParams {
    pub fork_id: String,
    pub checkpoint_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeleteCheckpointResponse {
    pub fork_id: String,
    pub checkpoint_id: String,
    pub deleted: bool,
}

pub async fn delete_checkpoint(
    state: Arc<AppState>,
    params: DeleteCheckpointParams,
) -> Result<DeleteCheckpointResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    registry.delete_checkpoint(&params.fork_id, &params.checkpoint_id)?;

    Ok(DeleteCheckpointResponse {
        fork_id: params.fork_id,
        checkpoint_id: params.checkpoint_id,
        deleted: true,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListStagedChangesParams {
    pub fork_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StagedChangeInfo {
    pub change_id: String,
    pub created_at: String,
    pub label: Option<String>,
    pub summary: ChangeSummary,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListStagedChangesResponse {
    pub fork_id: String,
    pub staged_changes: Vec<StagedChangeInfo>,
}

pub async fn list_staged_changes(
    state: Arc<AppState>,
    params: ListStagedChangesParams,
) -> Result<ListStagedChangesResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let staged = registry.list_staged_changes(&params.fork_id)?;
    let staged_changes = staged
        .into_iter()
        .map(|c| StagedChangeInfo {
            change_id: c.change_id,
            created_at: c.created_at.to_rfc3339(),
            label: c.label,
            summary: c.summary,
        })
        .collect();

    Ok(ListStagedChangesResponse {
        fork_id: params.fork_id,
        staged_changes,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ApplyStagedChangeParams {
    pub fork_id: String,
    pub change_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ApplyStagedChangeResponse {
    pub fork_id: String,
    pub change_id: String,
    pub ops_applied: usize,
    pub summary: ChangeSummary,
}

#[derive(Debug, Serialize, Deserialize)]
struct EditBatchStagedPayload {
    sheet_name: String,
    edits: Vec<CellEdit>,
}

pub async fn apply_staged_change(
    state: Arc<AppState>,
    params: ApplyStagedChangeParams,
) -> Result<ApplyStagedChangeResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let staged_list = registry.list_staged_changes(&params.fork_id)?;
    let staged = staged_list
        .iter()
        .find(|c| c.change_id == params.change_id)
        .cloned()
        .ok_or_else(|| anyhow!("staged change not found: {}", params.change_id))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;
    let work_path = fork_ctx.work_path.clone();

    let initial_recalc_needed = fork_ctx.recalc_needed;
    let mut recalc_triggered = false;

    let mut ops_applied = 0usize;

    for op in &staged.ops {
        match op.kind.as_str() {
            "canonical_write_bundle" => {
                let bundle: crate::canonical_write::CanonicalStagedBundle =
                    serde_json::from_value(op.payload.clone())
                        .map_err(|error| anyhow!("invalid canonical write bundle: {error}"))?;
                let applied = registry.with_fork_mut(&params.fork_id, |fork| {
                    fork.sync_revisions()?;
                    let applied = crate::canonical_write::apply_bundle_atomically_to_path(
                        &fork.work_path,
                        &bundle,
                    )?;
                    if applied > 0 {
                        fork.content_revision =
                            crate::utils::hash_file_sha256_hex(&fork.work_path)?;
                        fork.advance_state_revision();
                        fork.recalc_needed = true;
                    }
                    Ok(applied)
                })?;
                recalc_triggered = false;
                ops_applied += applied;
            }
            "edit_batch" => {
                recalc_triggered = true;
                let payload: EditBatchStagedPayload = serde_json::from_value(op.payload.clone())
                    .map_err(|e| anyhow!("invalid edit_batch payload: {}", e))?;

                let edits_to_apply: Vec<_> = payload
                    .edits
                    .iter()
                    .map(|e| EditOp {
                        timestamp: Utc::now(),
                        sheet: payload.sheet_name.clone(),
                        address: e.address.clone(),
                        value: e.value.clone(),
                        is_formula: e.is_formula,
                    })
                    .collect();

                tokio::task::spawn_blocking({
                    let sheet_name = payload.sheet_name.clone();
                    let edits = payload.edits.clone();
                    let work_path = work_path.clone();
                    move || {
                        let core_edits = edits
                            .into_iter()
                            .map(|edit| crate::core::types::CellEdit {
                                address: edit.address,
                                value: edit.value,
                                is_formula: edit.is_formula,
                            })
                            .collect::<Vec<_>>();
                        crate::core::write::apply_edits_to_file(
                            &work_path,
                            &sheet_name,
                            &core_edits,
                        )
                    }
                })
                .await??;

                registry.with_fork_mut(&params.fork_id, |ctx| {
                    ctx.edits.extend(edits_to_apply);
                    ctx.recalc_needed = true;
                    Ok(())
                })?;

                ops_applied += 1;
            }
            "style_batch" => {
                let payload: StyleBatchStagedPayload =
                    serde_json::from_value(op.payload.clone())
                        .map_err(|e| anyhow!("invalid style_batch payload: {}", e))?;

                tokio::task::spawn_blocking({
                    let ops = payload.ops.clone();
                    let work_path = work_path.clone();
                    move || apply_style_ops_to_file(&work_path, &ops)
                })
                .await??;

                ops_applied += 1;
            }
            "column_size_batch" => {
                let payload: ColumnSizeBatchStagedPayload =
                    serde_json::from_value(op.payload.clone())
                        .map_err(|e| anyhow!("invalid column_size_batch payload: {}", e))?;

                tokio::task::spawn_blocking({
                    let sheet_name = payload.sheet_name.clone();
                    let ops = payload.ops.clone();
                    let work_path = work_path.clone();
                    move || apply_column_size_ops_to_file(&work_path, &sheet_name, &ops)
                })
                .await??;

                ops_applied += 1;
            }
            "transform_batch" => {
                recalc_triggered = true;
                let payload: TransformBatchStagedPayload =
                    serde_json::from_value(op.payload.clone())
                        .map_err(|e| anyhow!("invalid transform_batch payload: {}", e))?;

                tokio::task::spawn_blocking({
                    let ops = payload.ops.clone();
                    let work_path = work_path.clone();
                    move || apply_transform_ops_to_file(&work_path, &ops)
                })
                .await??;

                ops_applied += 1;
            }
            "grid_import" => {
                recalc_triggered = true;
                let payload: GridImportStagedPayload =
                    serde_json::from_value(op.payload.clone())
                        .map_err(|e| anyhow!("invalid grid_import payload: {}", e))?;

                grid_import(
                    state.clone(),
                    GridImportParams {
                        fork_id: params.fork_id.clone(),
                        sheet_name: payload.sheet_name,
                        anchor: payload.anchor,
                        grid: payload.grid,
                        clear_target: payload.clear_target,
                        mode: Some(BatchMode::Apply),
                        label: None,
                        formula_parse_policy: None,
                    },
                )
                .await?;

                ops_applied += 1;
            }
            "apply_formula_pattern" => {
                recalc_triggered = true;
                let payload: ApplyFormulaPatternStagedPayload =
                    serde_json::from_value(op.payload.clone())
                        .map_err(|e| anyhow!("invalid apply_formula_pattern payload: {}", e))?;

                let bounds = parse_range_bounds(&payload.target_range)?;
                let (anchor_col, anchor_row) = parse_cell_ref(&payload.anchor_cell)?;
                let fill_direction = payload.fill_direction.unwrap_or_default();
                validate_formula_pattern_bounds(&bounds, anchor_col, anchor_row, fill_direction)?;
                let relative_mode: RelativeMode = payload.relative_mode.unwrap_or_default().into();

                tokio::task::spawn_blocking({
                    let sheet_name = payload.sheet_name.clone();
                    let target_range = payload.target_range.clone();
                    let base_formula = payload.base_formula.clone();
                    let work_path = work_path.clone();
                    move || {
                        apply_formula_pattern_to_file(
                            &work_path,
                            &sheet_name,
                            &target_range,
                            anchor_col,
                            anchor_row,
                            &base_formula,
                            relative_mode,
                        )
                    }
                })
                .await??;

                ops_applied += 1;
            }
            "structure_batch" => {
                let payload: StructureBatchStagedPayload =
                    serde_json::from_value(op.payload.clone())
                        .map_err(|e| anyhow!("invalid structure_batch payload: {}", e))?;

                if structure_ops_require_recalc(&payload.ops) {
                    recalc_triggered = true;
                }

                tokio::task::spawn_blocking({
                    let ops = payload.ops.clone();
                    let work_path = work_path.clone();
                    let policy = payload.formula_parse_policy.unwrap_or(
                        FormulaParsePolicy::default_for_command_class(CommandClass::BatchWrite),
                    );
                    move || apply_structure_ops_to_file(&work_path, &ops, policy)
                })
                .await??;

                ops_applied += 1;
            }
            "sheet_layout_batch" => {
                let payload: crate::tools::sheet_layout::SheetLayoutBatchStagedPayload =
                    serde_json::from_value(op.payload.clone())
                        .map_err(|e| anyhow!("invalid sheet_layout_batch payload: {}", e))?;

                tokio::task::spawn_blocking({
                    let ops = payload.ops.clone();
                    let work_path = work_path.clone();
                    move || {
                        crate::tools::sheet_layout::apply_sheet_layout_ops_to_file(&work_path, &ops)
                    }
                })
                .await??;

                ops_applied += 1;
            }
            "rules_batch" => {
                let payload: crate::tools::rules_batch::RulesBatchStagedPayload =
                    serde_json::from_value(op.payload.clone())
                        .map_err(|e| anyhow!("invalid rules_batch payload: {}", e))?;

                tokio::task::spawn_blocking({
                    let ops = payload.ops.clone();
                    let work_path = work_path.clone();
                    let policy = payload.formula_parse_policy.unwrap_or(
                        FormulaParsePolicy::default_for_command_class(CommandClass::BatchWrite),
                    );
                    move || {
                        crate::tools::rules_batch::apply_rules_ops_to_file(&work_path, &ops, policy)
                    }
                })
                .await??;

                ops_applied += 1;
            }
            other => {
                return Err(anyhow!("unsupported staged op kind: {}", other));
            }
        }
    }

    let recalc_needed_now = initial_recalc_needed || recalc_triggered;
    if recalc_needed_now {
        registry.with_fork_mut(&params.fork_id, |ctx| {
            ctx.recalc_needed = true;
            Ok(())
        })?;
    }

    registry.discard_staged_change(&params.fork_id, &params.change_id)?;
    let fork_workbook_id = WorkbookId(params.fork_id.clone());
    let _ = state.close_workbook(&fork_workbook_id);

    let mut summary = staged.summary;
    set_recalc_needed_flag(&mut summary, recalc_needed_now);

    Ok(ApplyStagedChangeResponse {
        fork_id: params.fork_id,
        change_id: params.change_id,
        ops_applied,
        summary,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiscardStagedChangeParams {
    pub fork_id: String,
    pub change_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DiscardStagedChangeResponse {
    pub fork_id: String,
    pub change_id: String,
    pub discarded: bool,
}

pub async fn discard_staged_change(
    state: Arc<AppState>,
    params: DiscardStagedChangeParams,
) -> Result<DiscardStagedChangeResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    registry.discard_staged_change(&params.fork_id, &params.change_id)?;

    Ok(DiscardStagedChangeResponse {
        fork_id: params.fork_id,
        change_id: params.change_id,
        discarded: true,
    })
}

#[cfg(not(target_arch = "wasm32"))]
const MAX_SCREENSHOT_ROWS: u32 = 100;
#[cfg(not(target_arch = "wasm32"))]
const MAX_SCREENSHOT_COLS: u32 = 30;
#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_SCREENSHOT_RANGE: &str = "A1:M40";
#[cfg(feature = "recalc-libreoffice")]
const DEFAULT_MAX_PNG_DIM_PX: u32 = 4096;
#[cfg(feature = "recalc-libreoffice")]
const DEFAULT_MAX_PNG_AREA_PX: u64 = 12_000_000;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScreenshotSheetParams {
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    pub sheet_name: String,
    #[serde(default)]
    pub range: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ScreenshotSheetResponse {
    pub workbook_id: String,
    pub sheet_name: String,
    pub range: String,
    pub output_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_output_path: Option<String>,
    pub size_bytes: u64,
    pub duration_ms: u64,
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn screenshot_sheet(
    state: Arc<AppState>,
    params: ScreenshotSheetParams,
) -> Result<ScreenshotSheetResponse> {
    let range = params.range.as_deref().unwrap_or(DEFAULT_SCREENSHOT_RANGE);
    let bounds = validate_screenshot_range(range)?;

    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let workbook_path = workbook.path.clone();

    let _ = workbook.with_sheet(&params.sheet_name, |_| Ok::<_, anyhow::Error>(()))?;

    let safe_range = sanitize_filename_component(&range.replace(':', "-"));
    let safe_sheet = sanitize_filename_component(&params.sheet_name).replace(' ', "_");
    let safe_slug = sanitize_filename_component(&workbook.slug);
    let filename = format!("{}_{}_{}.png", safe_slug, safe_sheet, safe_range);

    let config = state.config();
    let screenshot_dir = config.screenshot_dir.clone();

    let semaphore = state
        .screenshot_semaphore()
        .ok_or_else(|| anyhow!("screenshot semaphore not available"))?;

    // LibreOffice profile/macro export is not concurrency-safe. Serialize screenshot calls.
    let _permit = semaphore
        .0
        .acquire()
        .await
        .map_err(|e| anyhow!("failed to acquire screenshot permit: {}", e))?;
    tokio::fs::create_dir_all(&screenshot_dir).await?;
    let output_path = screenshot_dir.join(&filename);

    #[cfg(not(feature = "recalc-libreoffice"))]
    {
        let _ = workbook_path;
        let _ = output_path;
        let _ = bounds;
        Err(anyhow!(
            "screenshot backend unavailable (build without recalc-libreoffice feature)"
        ))
    }

    #[cfg(feature = "recalc-libreoffice")]
    {
        let executor =
            crate::recalc::ScreenshotExecutor::new(&crate::recalc::RecalcConfig::default());
        let result = executor
            .screenshot(
                &workbook_path,
                &output_path,
                &params.sheet_name,
                Some(range),
            )
            .await?;

        enforce_png_pixel_limits(&result.output_path, range, &bounds).await?;

        Ok(ScreenshotSheetResponse {
            workbook_id: params.workbook_or_fork_id.0,
            sheet_name: params.sheet_name,
            range: range.to_string(),
            output_path: format!("file://{}", result.output_path.display()),
            client_output_path: config
                .map_path_for_client(&result.output_path)
                .map(|p| format!("file://{}", p.display())),
            size_bytes: result.size_bytes,
            duration_ms: result.duration_ms,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct ScreenshotBounds {
    min_col: u32,
    max_col: u32,
    min_row: u32,
    max_row: u32,
    rows: u32,
    cols: u32,
}

#[cfg(not(target_arch = "wasm32"))]
fn validate_screenshot_range(range: &str) -> Result<ScreenshotBounds> {
    let bounds = parse_range_bounds(range)?;

    if bounds.rows > MAX_SCREENSHOT_ROWS || bounds.cols > MAX_SCREENSHOT_COLS {
        let row_tiles = div_ceil(bounds.rows, MAX_SCREENSHOT_ROWS);
        let col_tiles = div_ceil(bounds.cols, MAX_SCREENSHOT_COLS);
        let total_tiles = row_tiles * col_tiles;

        let display_limit = 50usize;
        let display_ranges = suggest_tiled_ranges(
            &bounds,
            MAX_SCREENSHOT_ROWS,
            MAX_SCREENSHOT_COLS,
            Some(display_limit),
        );

        let mut msg = format!(
            "Requested range {range} is too large for a single screenshot ({} rows x {} cols; max {} x {}). \
Split into {} tile(s) ({} row tiles x {} col tiles). Suggested ranges: {}",
            bounds.rows,
            bounds.cols,
            MAX_SCREENSHOT_ROWS,
            MAX_SCREENSHOT_COLS,
            total_tiles,
            row_tiles,
            col_tiles,
            display_ranges.join(", ")
        );
        if total_tiles as usize > display_limit {
            msg.push_str(&format!(
                " ... and {} more.",
                total_tiles as usize - display_limit
            ));
        }
        return Err(anyhow!(msg));
    }

    Ok(bounds)
}

fn parse_cell_ref(cell: &str) -> Result<(u32, u32)> {
    use umya_spreadsheet::helper::coordinate::index_from_coordinate;
    let (col, row, _, _) = index_from_coordinate(cell);
    match (col, row) {
        (Some(c), Some(r)) => Ok((c, r)),
        _ => Err(anyhow!("Invalid cell reference: {}", cell)),
    }
}

fn parse_range_bounds(range: &str) -> Result<ScreenshotBounds> {
    let parts: Vec<&str> = range.split(':').collect();
    if parts.is_empty() || parts.len() > 2 {
        return Err(anyhow!("Invalid range format. Expected 'A1' or 'A1:Z99'"));
    }

    let start = parse_cell_ref(parts[0])?;
    let end = if parts.len() == 2 {
        parse_cell_ref(parts[1])?
    } else {
        start
    };

    let min_col = start.0.min(end.0);
    let max_col = start.0.max(end.0);
    let min_row = start.1.min(end.1);
    let max_row = start.1.max(end.1);

    let rows = max_row - min_row + 1;
    let cols = max_col - min_col + 1;

    Ok(ScreenshotBounds {
        min_col,
        max_col,
        min_row,
        max_row,
        rows,
        cols,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn div_ceil(n: u32, d: u32) -> u32 {
    n.div_ceil(d)
}

#[cfg(not(target_arch = "wasm32"))]
fn suggest_tiled_ranges(
    bounds: &ScreenshotBounds,
    max_rows: u32,
    max_cols: u32,
    limit: Option<usize>,
) -> Vec<String> {
    use umya_spreadsheet::helper::coordinate::coordinate_from_index;

    let mut out = Vec::new();
    let mut row_start = bounds.min_row;
    while row_start <= bounds.max_row {
        let row_end = (row_start + max_rows - 1).min(bounds.max_row);
        let mut col_start = bounds.min_col;
        while col_start <= bounds.max_col {
            let col_end = (col_start + max_cols - 1).min(bounds.max_col);
            let start_cell = coordinate_from_index(col_start, row_start);
            let end_cell = coordinate_from_index(col_end, row_end);
            out.push(format!("{start_cell}:{end_cell}"));
            if let Some(lim) = limit
                && out.len() >= lim
            {
                return out;
            }
            col_start = col_end + 1;
        }
        row_start = row_end + 1;
        if let Some(lim) = limit
            && out.len() >= lim
        {
            return out;
        }
    }
    out
}

#[cfg(feature = "recalc-libreoffice")]
fn suggest_split_single_tile(bounds: &ScreenshotBounds) -> Vec<String> {
    use umya_spreadsheet::helper::coordinate::coordinate_from_index;

    if bounds.rows >= bounds.cols && bounds.rows > 1 {
        let mid_row = bounds.min_row + (bounds.rows / 2) - 1;
        let start1 = coordinate_from_index(&bounds.min_col, &bounds.min_row);
        let end1 = coordinate_from_index(&bounds.max_col, &mid_row);
        let start2 = coordinate_from_index(&bounds.min_col, &(mid_row + 1));
        let end2 = coordinate_from_index(&bounds.max_col, &bounds.max_row);
        vec![format!("{start1}:{end1}"), format!("{start2}:{end2}")]
    } else if bounds.cols > 1 {
        let mid_col = bounds.min_col + (bounds.cols / 2) - 1;
        let start1 = coordinate_from_index(&bounds.min_col, &bounds.min_row);
        let end1 = coordinate_from_index(&mid_col, &bounds.max_row);
        let start2 = coordinate_from_index(&(mid_col + 1), &bounds.min_row);
        let end2 = coordinate_from_index(&bounds.max_col, &bounds.max_row);
        vec![format!("{start1}:{end1}"), format!("{start2}:{end2}")]
    } else {
        vec![range_from_bounds(bounds)]
    }
}

#[cfg(feature = "recalc-libreoffice")]
fn range_from_bounds(bounds: &ScreenshotBounds) -> String {
    use umya_spreadsheet::helper::coordinate::coordinate_from_index;
    let start = coordinate_from_index(&bounds.min_col, &bounds.min_row);
    let end = coordinate_from_index(&bounds.max_col, &bounds.max_row);
    format!("{start}:{end}")
}

#[cfg(feature = "recalc-libreoffice")]
async fn enforce_png_pixel_limits(
    path: &std::path::Path,
    range: &str,
    bounds: &ScreenshotBounds,
) -> Result<()> {
    use image::GenericImageView;
    use image::ImageReader;

    let max_dim_px = std::env::var("SPREADSHEET_MCP_MAX_PNG_DIM_PX")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_PNG_DIM_PX);
    let max_area_px = std::env::var("SPREADSHEET_MCP_MAX_PNG_AREA_PX")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MAX_PNG_AREA_PX);

    let reader = ImageReader::open(path)
        .map_err(|e| anyhow!("failed to read png {}: {}", path.display(), e))?;
    let reader = reader
        .with_guessed_format()
        .map_err(|e| anyhow!("failed to sniff png {}: {}", path.display(), e))?;
    let img = reader
        .decode()
        .map_err(|e| anyhow!("failed to decode png {}: {}", path.display(), e))?;
    let (w, h) = img.dimensions();
    let area = (w as u64) * (h as u64);

    if w > max_dim_px || h > max_dim_px || area > max_area_px {
        let _ = tokio::fs::remove_file(path).await;

        let mut suggestions =
            suggest_tiled_ranges(bounds, MAX_SCREENSHOT_ROWS, MAX_SCREENSHOT_COLS, Some(50));
        let row_tiles = div_ceil(bounds.rows, MAX_SCREENSHOT_ROWS);
        let col_tiles = div_ceil(bounds.cols, MAX_SCREENSHOT_COLS);
        let total_tiles = row_tiles * col_tiles;
        if total_tiles == 1 {
            suggestions = suggest_split_single_tile(bounds);
        }

        return Err(anyhow!(
            "Rendered PNG for range {range} is {w}x{h}px (area {area}px), exceeding limits (max_dim={max_dim_px}px, max_area={max_area_px}px). \
Try smaller ranges. Suggested ranges: {}",
            suggestions.join(", ")
        ));
    }

    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GridImportParams {
    #[serde(alias = "workbook_or_fork_id")]
    pub fork_id: String,
    pub sheet_name: String,
    pub anchor: String,
    pub grid: crate::model::GridPayload,
    #[serde(default)]
    pub clear_target: bool,
    #[serde(default)]
    pub mode: Option<BatchMode>,
    pub label: Option<String>,
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GridImportResponse {
    pub fork_id: String,
    pub mode: String,
    pub change_id: Option<String>,
    pub summary: ChangeSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GridImportStagedPayload {
    sheet_name: String,
    anchor: String,
    clear_target: bool,
    grid: crate::model::GridPayload,
}

pub async fn grid_import(
    state: Arc<AppState>,
    params: GridImportParams,
) -> Result<GridImportResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;
    let work_path = fork_ctx.work_path.clone();

    let (anchor_col, anchor_row) = parse_cell_ref(&params.anchor)?;
    let mut max_col = anchor_col;
    let mut max_row = anchor_row;

    let mut write_rows: Vec<Vec<Option<MatrixCell>>> = Vec::new();
    let mut style_ops_map: BTreeMap<String, (StyleOp, Vec<String>)> = BTreeMap::new(); // keyed by json of style patch

    for grid_row in &params.grid.rows {
        for cell in &grid_row.cells {
            let r = anchor_row + cell.offset[0];
            let c = anchor_col + cell.offset[1];
            if r > max_row {
                max_row = r;
            }
            if c > max_col {
                max_col = c;
            }
        }
    }

    let footprint_range = format!(
        "{}:{}",
        crate::utils::cell_address(anchor_col, anchor_row),
        crate::utils::cell_address(max_col, max_row)
    );

    for grid_row in &params.grid.rows {
        for cell in &grid_row.cells {
            let row_idx = cell.offset[0] as usize;
            let col_idx = cell.offset[1] as usize;
            while write_rows.len() <= row_idx {
                write_rows.push(Vec::new());
            }
            while write_rows[row_idx].len() <= col_idx {
                write_rows[row_idx].push(None);
            }

            let mut mc = None;
            if let Some(f) = &cell.f {
                mc = Some(MatrixCell::Formula(f.clone()));
            } else if let Some(v) = &cell.v {
                mc = Some(MatrixCell::Value(v.clone()));
            }
            if mc.is_some() {
                write_rows[row_idx][col_idx] = mc;
            }

            let mut has_style = false;
            let mut style_patch = crate::model::StylePatch::default();
            if let Some(st) = &cell.style {
                style_patch = st.clone();
                has_style = true;
            }
            if let Some(fmt) = &cell.fmt {
                style_patch.number_format = Some(Some(fmt.clone()));
                has_style = true;
            }

            if has_style {
                let key = serde_json::to_string(&style_patch).unwrap();
                let addr = crate::utils::cell_address(
                    anchor_col + cell.offset[1],
                    anchor_row + cell.offset[0],
                );

                let entry = style_ops_map.entry(key).or_insert_with(|| {
                    (
                        StyleOp {
                            sheet_name: params.sheet_name.clone(),
                            target: StyleTarget::Cells { cells: Vec::new() }, // filled later
                            patch: style_patch.clone(),
                            op_mode: None,
                        },
                        Vec::new(),
                    )
                });
                entry.1.push(addr);
            }
        }
    }

    let mut style_ops = Vec::new();
    if params.clear_target {
        style_ops.push(StyleOp {
            sheet_name: params.sheet_name.clone(),
            target: StyleTarget::Range {
                range: footprint_range.clone(),
            },
            patch: crate::model::StylePatch {
                font: Some(None),
                fill: Some(None),
                borders: Some(None),
                alignment: Some(None),
                number_format: Some(None),
            },
            op_mode: None,
        });
    }
    for mut group in style_ops_map.into_values() {
        group.0.target = StyleTarget::Cells { cells: group.1 };
        style_ops.push(group.0);
    }

    let mut transform_ops = Vec::new();
    if params.clear_target {
        transform_ops.push(TransformOp::ClearRange {
            sheet_name: params.sheet_name.clone(),
            target: TransformTarget::Range {
                range: footprint_range.clone(),
            },
            clear_values: true,
            clear_formulas: true,
        });
    }
    transform_ops.push(TransformOp::WriteMatrix {
        sheet_name: params.sheet_name.clone(),
        anchor: params.anchor.clone(),
        rows: write_rows,
        overwrite_formulas: true,
    });

    let mut column_ops = Vec::new();
    for col in &params.grid.columns {
        let col_name = crate::utils::column_number_to_name(anchor_col + col.offset);
        column_ops.push(ColumnSizeOp {
            target: ColumnTarget::Columns {
                range: format!("{}:{}", col_name, col_name),
            },
            size: ColumnSizeSpec::Width {
                width_chars: col.width_chars,
            },
        });
    }

    let mut structure_ops = Vec::new();
    if !params.grid.merges.is_empty() {
        if params.clear_target {
            structure_ops.push(StructureOp::UnmergeCells {
                sheet_name: params.sheet_name.clone(),
                target_range: footprint_range.clone(),
            });
        }
        for merge in &params.grid.merges {
            structure_ops.push(StructureOp::MergeCells {
                sheet_name: params.sheet_name.clone(),
                target_range: merge.clone(),
            });
        }
    }

    // Prepare ops for applying (same formula parsing as transform_batch)
    let fork_workbook_id = WorkbookId(params.fork_id.clone());
    let workbook = state.open_workbook(&fork_workbook_id).await?;
    let resolved_transform_ops = resolve_transform_ops_for_workbook(&workbook, &transform_ops)?;
    let resolved_style_ops = resolve_style_ops_for_workbook(&workbook, &style_ops)?;

    let policy =
        params
            .formula_parse_policy
            .unwrap_or(FormulaParsePolicy::default_for_command_class(
                CommandClass::BatchWrite,
            ));
    let (ops_to_apply, formula_parse_diagnostics) = if policy == FormulaParsePolicy::Off {
        (resolved_transform_ops, None)
    } else {
        let mut builder = FormulaParseDiagnosticsBuilder::new(policy);
        let mut valid_ops = Vec::new();
        for op in resolved_transform_ops {
            match &op {
                TransformOp::WriteMatrix {
                    sheet_name,
                    anchor,
                    rows,
                    overwrite_formulas,
                } => {
                    let mut has_errors = false;
                    let mut valid_rows = Vec::new();
                    let (a_col, a_row) = parse_cell_ref(anchor)?;
                    for (r_idx, row) in rows.iter().enumerate() {
                        let mut valid_row = Vec::new();
                        let r = a_row + r_idx as u32;
                        for (c_idx, cell_opt) in row.iter().enumerate() {
                            let c = a_col + c_idx as u32;
                            if let Some(MatrixCell::Formula(f)) = cell_opt {
                                match validate_formula(f) {
                                    Ok(()) => valid_row.push(cell_opt.clone()),
                                    Err(err_msg) => {
                                        if policy == FormulaParsePolicy::Fail {
                                            bail!(
                                                "{}grid_import formula failed at {}: {}",
                                                FORMULA_PARSE_FAILED_PREFIX,
                                                crate::utils::cell_address(c, r),
                                                err_msg
                                            );
                                        }
                                        builder.record_error(
                                            sheet_name,
                                            &crate::utils::cell_address(c, r),
                                            f,
                                            &err_msg,
                                        );
                                        has_errors = true;
                                        valid_row.push(None);
                                    }
                                }
                            } else {
                                valid_row.push(cell_opt.clone());
                            }
                        }
                        valid_rows.push(valid_row);
                    }
                    if has_errors && policy == FormulaParsePolicy::Warn {
                        valid_ops.push(TransformOp::WriteMatrix {
                            sheet_name: sheet_name.clone(),
                            anchor: anchor.clone(),
                            rows: valid_rows,
                            overwrite_formulas: *overwrite_formulas,
                        });
                    } else {
                        valid_ops.push(op);
                    }
                }
                _ => valid_ops.push(op),
            }
        }
        let diagnostics = if builder.has_errors() {
            Some(builder.build())
        } else {
            None
        };
        (valid_ops, diagnostics)
    };

    let mode = params.mode.unwrap_or_default();

    if mode.is_preview() {
        let change_id = make_short_random_id("chg", 12);
        let snapshot_path = stage_snapshot_path(&params.fork_id, &change_id);
        fs::create_dir_all(snapshot_path.parent().unwrap())?;
        fs::copy(&work_path, &snapshot_path)?;

        let snapshot_for_apply = snapshot_path.clone();
        let sheet_name_for_col = params.sheet_name.clone();
        let apply_result = tokio::task::spawn_blocking(move || {
            let mut summary = ChangeSummary::default();
            summary.op_kinds.push("grid_import".to_string());

            if !structure_ops.is_empty() {
                let res = apply_structure_ops_to_file(
                    &snapshot_for_apply,
                    &structure_ops,
                    FormulaParsePolicy::Off,
                )?;
                merge_summary_counts(&mut summary, &res.summary);
            }
            if !column_ops.is_empty() {
                let res = apply_column_size_ops_to_file(
                    &snapshot_for_apply,
                    &sheet_name_for_col,
                    &column_ops,
                )?;
                merge_summary_counts(&mut summary, &res.summary);
            }
            if !ops_to_apply.is_empty() {
                let res = apply_transform_ops_to_file(&snapshot_for_apply, &ops_to_apply)?;
                merge_summary_counts(&mut summary, &res.summary);
            }
            if !resolved_style_ops.is_empty() {
                let res = apply_style_ops_to_file(&snapshot_for_apply, &resolved_style_ops)?;
                merge_summary_counts(&mut summary, &res.summary);
            }

            Ok::<_, anyhow::Error>(summary)
        })
        .await??;

        let mut summary = apply_result;
        set_recalc_needed_flag(&mut summary, fork_ctx.recalc_needed);

        let staged_op = StagedOp {
            kind: "grid_import".to_string(),
            payload: serde_json::to_value(GridImportStagedPayload {
                sheet_name: params.sheet_name.clone(),
                anchor: params.anchor.clone(),
                clear_target: params.clear_target,
                grid: params.grid.clone(),
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

        Ok(GridImportResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: Some(change_id),
            summary,
            formula_parse_diagnostics,
        })
    } else {
        let sheet_name_for_col = params.sheet_name.clone();
        let work_path_for_apply = work_path.clone();
        let apply_result = tokio::task::spawn_blocking(move || {
            let mut summary = ChangeSummary::default();
            summary.op_kinds.push("grid_import".to_string());

            if !structure_ops.is_empty() {
                let res = apply_structure_ops_to_file(
                    &work_path_for_apply,
                    &structure_ops,
                    FormulaParsePolicy::Off,
                )?;
                merge_summary_counts(&mut summary, &res.summary);
            }
            if !column_ops.is_empty() {
                let res = apply_column_size_ops_to_file(
                    &work_path_for_apply,
                    &sheet_name_for_col,
                    &column_ops,
                )?;
                merge_summary_counts(&mut summary, &res.summary);
            }
            if !ops_to_apply.is_empty() {
                let res = apply_transform_ops_to_file(&work_path_for_apply, &ops_to_apply)?;
                merge_summary_counts(&mut summary, &res.summary);
            }
            if !resolved_style_ops.is_empty() {
                let res = apply_style_ops_to_file(&work_path_for_apply, &resolved_style_ops)?;
                merge_summary_counts(&mut summary, &res.summary);
            }

            Ok::<_, anyhow::Error>(summary)
        })
        .await??;

        let mut summary = apply_result;
        registry.with_fork_mut(&params.fork_id, |ctx| {
            ctx.recalc_needed = true;
            Ok(())
        })?;
        set_recalc_needed_flag(&mut summary, true);

        let _ = state.close_workbook(&fork_workbook_id);

        Ok(GridImportResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: None,
            summary,
            formula_parse_diagnostics,
        })
    }
}

fn merge_summary_counts(dest: &mut ChangeSummary, src: &ChangeSummary) {
    for sheet in &src.affected_sheets {
        if !dest.affected_sheets.contains(sheet) {
            dest.affected_sheets.push(sheet.clone());
        }
    }
    for bounds in &src.affected_bounds {
        if !dest.affected_bounds.contains(bounds) {
            dest.affected_bounds.push(bounds.clone());
        }
    }
    for (k, v) in &src.counts {
        *dest.counts.entry(k.clone()).or_insert(0) += v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjust_ref_coord_part_row_insert_clamps_excel_max_row() {
        let adjusted = adjust_ref_coord_part(
            "$C$2:$P$1048576",
            StructureAxis::Row,
            StructureEdit::Insert { at: 1791, count: 7 },
        )
        .expect("adjust range");

        assert_eq!(adjusted, "$C$2:$P$1048576");
    }

    #[test]
    fn transform_a1_row_insert_handles_unaffected_crossing_multiple_and_bounds() {
        let adjusted = adjust_ref_coord_part(
            "A1:B2,C2:C4,$D$1048575:$D$1048576",
            StructureAxis::Row,
            StructureEdit::Insert { at: 3, count: 2 },
        )
        .expect("adjust ranges");

        assert_eq!(adjusted, "A1:B2,C2:C6,$D$1048576:$D$1048576");
    }

    #[test]
    fn transform_a1_row_delete_shrinks_crossing_ranges_and_drops_deleted_ranges() {
        let adjusted = transform_a1_range(
            "A2:A8,B4:B5,C4:C8,D2:D5,E10",
            StructureAxis::Row,
            StructureEdit::Delete { start: 4, count: 2 },
        )
        .expect("adjust ranges");

        assert_eq!(adjusted.as_deref(), Some("A2:A6,C4:C6,D2:D3,E8"));
    }

    #[test]
    fn structure_row_edits_relocate_data_validation_sqrefs() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("validation-row-shifts.xlsx");
        let mut workbook = umya_spreadsheet::new_file();
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").expect("sheet1");
        let mut validation = umya_spreadsheet::structs::DataValidation::default();
        validation
            .get_sequence_of_references_mut()
            .set_sqref("A1:A2 C2:C4 D10:D11");
        sheet.set_data_validations(umya_spreadsheet::structs::DataValidations::default());
        sheet
            .get_data_validations_mut()
            .expect("validations")
            .add_data_validation_list(validation);
        crate::xlsx_export::write(&workbook, &path).expect("write fixture");

        apply_structure_ops_to_file(
            &path,
            &[StructureOp::InsertRows {
                sheet_name: "Sheet1".to_string(),
                at_row: 3,
                count: 2,
                expand_adjacent_sums: false,
            }],
            FormulaParsePolicy::Warn,
        )
        .expect("insert rows");
        let workbook = crate::xlsx_import::read(&path).expect("read inserted");
        let sqref = workbook
            .get_sheet_by_name("Sheet1")
            .expect("sheet1")
            .get_data_validations()
            .expect("validations")
            .get_data_validation_list()[0]
            .get_sequence_of_references()
            .get_sqref();
        assert_eq!(sqref, "A1:A2 C2:C6 D12:D13");

        apply_structure_ops_to_file(
            &path,
            &[StructureOp::DeleteRows {
                sheet_name: "Sheet1".to_string(),
                start_row: 4,
                count: 2,
            }],
            FormulaParsePolicy::Warn,
        )
        .expect("delete rows");
        let workbook = crate::xlsx_import::read(&path).expect("read deleted");
        let sqref = workbook
            .get_sheet_by_name("Sheet1")
            .expect("sheet1")
            .get_data_validations()
            .expect("validations")
            .get_data_validation_list()[0]
            .get_sequence_of_references()
            .get_sqref();
        assert_eq!(sqref, "A1:A2 C2:C4 D10:D11");
    }

    #[test]
    fn clamp_out_of_bounds_defined_name_rows_clamps_plain_ranges() {
        let mut workbook = umya_spreadsheet::new_file();
        workbook
            .set_sheet_name(0, "GL Data".to_string())
            .expect("rename default sheet");

        let workbook_scoped = {
            let sheet = workbook
                .get_sheet_by_name_mut("GL Data")
                .expect("GL Data sheet");
            sheet
                .add_defined_name("GLDATA", "'GL Data'!$C$2:$P$1048583")
                .expect("add defined name");
            let cloned = sheet
                .get_defined_names()
                .first()
                .expect("sheet defined name")
                .clone();
            sheet.get_defined_names_mut().clear();
            cloned
        };
        {
            let defs = workbook.get_defined_names_mut();
            defs.clear();
            defs.push(workbook_scoped);
        }

        let changed = clamp_out_of_bounds_defined_name_rows(&mut workbook);
        assert!(changed >= 1, "expected at least one clamped defined name");

        let global_refers_to = workbook
            .get_defined_names()
            .iter()
            .find(|item| item.get_name() == "GLDATA")
            .map(|item| item.get_address().to_string())
            .expect("global GLDATA defined name");
        assert!(
            global_refers_to.contains("$P$1048576"),
            "expected clamped max-row bound, got: {global_refers_to}"
        );
        assert!(
            !global_refers_to.contains("104858"),
            "defined name should not overflow Excel max rows: {global_refers_to}"
        );
    }

    #[test]
    fn clamp_defined_name_rows_in_workbook_xml_clamps_overflow_rows() {
        let xml = r#"<workbook><definedNames><definedName name="GLDATA" hidden="0">'GL Data'!$C$2:$P$1048583</definedName><definedName name="Calc">=SUM(Sheet1!A1:A10)</definedName></definedNames></workbook>"#;
        let (rewritten, changed) = clamp_defined_name_rows_in_workbook_xml(xml);

        assert_eq!(changed, 1);
        assert!(rewritten.contains("'GL Data'!$C$2:$P$1048576"));
        assert!(!rewritten.contains("1048583"));
        assert!(
            rewritten.contains("<definedName name=\"Calc\">=SUM(Sheet1!A1:A10)</definedName>"),
            "formula-like defined names should not be rewritten"
        );
    }
}
