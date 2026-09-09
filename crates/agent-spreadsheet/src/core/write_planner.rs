use crate::config::{OutputProfile, RecalcBackendKind, ServerConfig, TransportKind};
use crate::formula::pattern::{RelativeMode, parse_base_formula, shift_formula_ast};
use crate::model::{FormulaParsePolicy, NamedItemKind};
use crate::tools::fork::{
    MatrixCell, StructureOp, TransformOp, apply_structure_ops_to_workbook,
    apply_transform_ops_to_workbook,
};
use crate::workbook::WorkbookContext;
use anyhow::{Context, Result, anyhow};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, to_value};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AppendFooterPolicy {
    Auto,
    BeforeFooter,
    AppendAtEnd,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClonePatchTargets {
    LikelyInputs,
    AllNonFormula,
    None,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloneMergePolicy {
    Safe,
    Strict,
}
fn invalid_argument(message: impl AsRef<str>) -> anyhow::Error {
    anyhow!("invalid argument: {}", message.as_ref())
}
fn unsafe_clone_template(message: impl AsRef<str>) -> anyhow::Error {
    anyhow!("unsafe clone template: {}", message.as_ref())
}

pub fn apply_append_rows(
    path: &Path,
    sheet_name: &str,
    region_id: Option<u32>,
    table_name: Option<&str>,
    footer_policy: AppendFooterPolicy,
    rows: Vec<Vec<Option<MatrixCell>>>,
) -> Result<Value> {
    let plan =
        build_append_region_plan(path, sheet_name, region_id, table_name, footer_policy, rows)?;
    apply_append_region_plan_to_file(path, &plan)?;
    Ok(to_value(plan)?)
}

pub(crate) fn apply_append_rows_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    region_id: Option<u32>,
    table_name: Option<&str>,
    footer_policy: AppendFooterPolicy,
    rows: Vec<Vec<Option<MatrixCell>>>,
) -> Result<Value> {
    let mut bytes = Vec::new();
    crate::xlsx_export::write_writer(book, &mut bytes)?;
    let plan = build_append_region_plan_from_bytes(
        &bytes,
        sheet_name,
        region_id,
        table_name,
        footer_policy,
        rows,
    )?;
    apply_append_region_plan_to_workbook(book, &plan)?;
    Ok(to_value(plan)?)
}

#[allow(clippy::too_many_arguments)]
pub fn apply_clone_row(
    path: &Path,
    sheet_name: &str,
    source_row: u32,
    before: Option<u32>,
    after: Option<u32>,
    insert_at: Option<u32>,
    count: u32,
    expand_adjacent_sums: bool,
    patch_targets: ClonePatchTargets,
    merge_policy: CloneMergePolicy,
) -> Result<Value> {
    let plan = build_clone_template_row_plan(
        path,
        sheet_name,
        source_row,
        before,
        after,
        insert_at,
        count,
        expand_adjacent_sums,
        patch_targets,
        merge_policy,
    )?;
    apply_clone_template_row_plan_to_file(path, &plan)?;
    Ok(to_value(plan)?)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_clone_row_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    source_row: u32,
    before: Option<u32>,
    after: Option<u32>,
    insert_at: Option<u32>,
    count: u32,
    expand_adjacent_sums: bool,
    patch_targets: ClonePatchTargets,
    merge_policy: CloneMergePolicy,
) -> Result<Value> {
    let plan = build_clone_template_row_plan_from_workbook(
        book,
        sheet_name,
        source_row,
        before,
        after,
        insert_at,
        count,
        expand_adjacent_sums,
        patch_targets,
        merge_policy,
    )?;
    apply_clone_template_row_plan_to_workbook(book, &plan)?;
    Ok(to_value(plan)?)
}

#[allow(clippy::too_many_arguments)]
pub fn apply_clone_row_band(
    path: &Path,
    sheet_name: &str,
    source_rows: &str,
    before: Option<u32>,
    after: Option<u32>,
    insert_at: Option<u32>,
    repeat: u32,
    expand_adjacent_sums: bool,
    patch_targets: ClonePatchTargets,
    merge_policy: CloneMergePolicy,
) -> Result<Value> {
    let plan = build_clone_row_band_plan(
        path,
        sheet_name,
        source_rows,
        before,
        after,
        insert_at,
        repeat,
        expand_adjacent_sums,
        patch_targets,
        merge_policy,
    )?;
    apply_clone_row_band_plan_to_file(path, &plan)?;
    Ok(to_value(plan)?)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_clone_row_band_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    source_rows: &str,
    before: Option<u32>,
    after: Option<u32>,
    insert_at: Option<u32>,
    repeat: u32,
    expand_adjacent_sums: bool,
    patch_targets: ClonePatchTargets,
    merge_policy: CloneMergePolicy,
) -> Result<Value> {
    let plan = build_clone_row_band_plan_from_workbook(
        book,
        sheet_name,
        source_rows,
        before,
        after,
        insert_at,
        repeat,
        expand_adjacent_sums,
        patch_targets,
        merge_policy,
    )?;
    apply_clone_row_band_plan_to_workbook(book, &plan)?;
    Ok(to_value(plan)?)
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AppendRegionTargetKind {
    DetectedRegion,
    Table,
}

#[derive(Debug, Clone, Serialize)]
struct AppendFooterCandidate {
    row: u32,
    matched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AppendRegionResponse {
    mode: String,
    file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_path: Option<String>,
    sheet_name: String,
    target_kind: AppendRegionTargetKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    region_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    table_name: Option<String>,
    region_bounds: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    header_row: Option<u32>,
    footer_policy: String,
    insert_at_row: u32,
    insert_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    footer_row: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    footer_detection: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    footer_candidates: Vec<AppendFooterCandidate>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    footer_formula_targets: Vec<String>,
    rows_appended: u32,
    columns_written: u32,
    target_anchor: String,
    target_range: String,
    expand_adjacent_sums: bool,
    confidence: String,
    confidence_reason: String,
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    would_change: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppendRegionPlan {
    sheet_name: String,
    target_kind: AppendRegionTargetKind,
    region_id: Option<u32>,
    table_name: Option<String>,
    region_bounds: String,
    header_row: Option<u32>,
    footer_policy: String,
    insert_at_row: u32,
    insert_reason: String,
    footer_row: Option<u32>,
    footer_detection: Option<String>,
    footer_candidates: Vec<AppendFooterCandidate>,
    footer_formula_targets: Vec<String>,
    rows_appended: u32,
    columns_written: u32,
    target_anchor: String,
    target_range: String,
    confidence: String,
    confidence_reason: String,
    warnings: Vec<String>,
    #[serde(skip)]
    rows: Vec<Vec<Option<MatrixCell>>>,
}

struct AppendFooterScan {
    footer_row: Option<u32>,
    footer_detection: Option<String>,
    footer_candidates: Vec<AppendFooterCandidate>,
    footer_formula_targets: Vec<String>,
}

struct AppendRegionTarget {
    sheet_name: String,
    target_kind: AppendRegionTargetKind,
    region_id: Option<u32>,
    table_name: Option<String>,
    bounds: AppendBounds,
    region_bounds: String,
    header_row: Option<u32>,
    headers_truncated: bool,
}

pub fn build_append_region_plan(
    source: &Path,
    sheet_name: &str,
    region_id: Option<u32>,
    table_name: Option<&str>,
    footer_policy: AppendFooterPolicy,
    rows: Vec<Vec<Option<MatrixCell>>>,
) -> Result<AppendRegionPlan> {
    if rows.is_empty() {
        return Err(invalid_argument(
            "append-region requires at least one row in the rows payload",
        ));
    }

    let config = Arc::new(local_workbook_config(source));
    let workbook = WorkbookContext::load(&config, source)?;
    let book = crate::xlsx_import::read(source)
        .with_context(|| format!("failed to read workbook '{}'", source.display()))?;
    build_append_region_plan_from_workbooks(
        &workbook,
        &book,
        source,
        sheet_name,
        region_id,
        table_name,
        footer_policy,
        rows,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_append_region_plan_from_workbooks(
    workbook: &WorkbookContext,
    book: &umya_spreadsheet::Workbook,
    source: &Path,
    sheet_name: &str,
    region_id: Option<u32>,
    table_name: Option<&str>,
    footer_policy: AppendFooterPolicy,
    rows: Vec<Vec<Option<MatrixCell>>>,
) -> Result<AppendRegionPlan> {
    let target = resolve_append_region_target(workbook, source, sheet_name, region_id, table_name)?;
    let bounds = target.bounds;

    let columns_written = rows.iter().map(Vec::len).max().unwrap_or(0) as u32;
    if columns_written == 0 {
        return Err(invalid_argument(
            "append-region rows payload must contain at least one non-empty column",
        ));
    }
    let region_width = bounds.end_col - bounds.start_col + 1;
    if columns_written > region_width {
        let target_label = target
            .table_name
            .clone()
            .map(|name| format!("table '{}'", name))
            .or_else(|| target.region_id.map(|id| format!("region {}", id)))
            .unwrap_or_else(|| "append target".to_string());
        return Err(invalid_argument(format!(
            "rows payload is wider than {} on sheet '{}': payload columns={}, region columns={}",
            target_label, target.sheet_name, columns_written, region_width
        )));
    }

    let footer_scan = detect_append_footer_on_workbook(
        book,
        &target.sheet_name,
        bounds.start_col,
        bounds.end_col,
        bounds.end_row,
    )?;
    let footer_policy_label = append_footer_policy_label(footer_policy).to_string();
    let (insert_at_row, insert_reason) = match footer_policy {
        AppendFooterPolicy::Auto => {
            if let Some(row) = footer_scan.footer_row {
                (
                    row,
                    format!("auto policy selected detected footer row {}", row),
                )
            } else {
                (
                    bounds.end_row + 1,
                    format!(
                        "auto policy found no footer row; appending after detected region end row {}",
                        bounds.end_row
                    ),
                )
            }
        }
        AppendFooterPolicy::BeforeFooter => {
            let row = footer_scan.footer_row.ok_or_else(|| {
                invalid_argument(
                    "footer policy 'before-footer' requires a detected footer/subtotal row; use --footer-policy auto or append-at-end to continue without one",
                )
            })?;
            (
                row,
                format!("before_footer policy selected detected footer row {}", row),
            )
        }
        AppendFooterPolicy::AppendAtEnd => {
            if let Some(row) = footer_scan.footer_row {
                (
                    bounds.end_row + 1,
                    format!(
                        "append_at_end policy bypassed detected footer row {} and appended after region end row {}",
                        row, bounds.end_row
                    ),
                )
            } else {
                (
                    bounds.end_row + 1,
                    format!(
                        "append_at_end policy appended after detected region end row {}",
                        bounds.end_row
                    ),
                )
            }
        }
    };
    let target_anchor = format!(
        "{}{}",
        column_number_to_name(bounds.start_col),
        insert_at_row
    );
    let target_range = format_a1_range(
        bounds.start_col,
        bounds.start_col + columns_written - 1,
        insert_at_row,
        insert_at_row + rows.len() as u32 - 1,
    );

    let mut warnings = Vec::new();
    if target.headers_truncated {
        warnings.push(
            "detected region headers were truncated; verify the append target carefully"
                .to_string(),
        );
    }
    match footer_policy {
        AppendFooterPolicy::Auto if footer_scan.footer_row.is_none() => {
            warnings.push("no footer row detected; appending at detected region end".to_string());
        }
        AppendFooterPolicy::AppendAtEnd if footer_scan.footer_row.is_some() => {
            warnings.push(format!(
                "footer policy '{}' ignored detected footer row {}",
                footer_policy_label,
                footer_scan.footer_row.unwrap_or_default()
            ));
        }
        _ => {}
    }

    let (confidence, confidence_reason) = append_plan_confidence(&target, &footer_scan);

    Ok(AppendRegionPlan {
        sheet_name: target.sheet_name,
        target_kind: target.target_kind,
        region_id: target.region_id,
        table_name: target.table_name,
        region_bounds: target.region_bounds,
        header_row: target.header_row,
        footer_policy: footer_policy_label,
        insert_at_row,
        insert_reason,
        footer_row: footer_scan.footer_row,
        footer_detection: footer_scan.footer_detection,
        footer_candidates: footer_scan.footer_candidates,
        footer_formula_targets: footer_scan.footer_formula_targets,
        rows_appended: rows.len() as u32,
        columns_written,
        target_anchor,
        target_range,
        confidence: confidence.to_string(),
        confidence_reason,
        warnings,
        rows,
    })
}

pub fn build_append_region_plan_from_bytes(
    bytes: &[u8],
    sheet_name: &str,
    region_id: Option<u32>,
    table_name: Option<&str>,
    footer_policy: AppendFooterPolicy,
    rows: Vec<Vec<Option<MatrixCell>>>,
) -> Result<AppendRegionPlan> {
    let source = Path::new("session.xlsx");
    let config = Arc::new(local_workbook_config(source));
    let workbook = WorkbookContext::load_from_bytes(
        &config,
        "session.xlsx",
        bytes,
        crate::model::WorkbookId("session".to_string()),
        "session".to_string(),
        None,
    )?;
    let book = crate::xlsx_import::read_reader(std::io::Cursor::new(bytes), true)?;
    build_append_region_plan_from_workbooks(
        &workbook,
        &book,
        source,
        sheet_name,
        region_id,
        table_name,
        footer_policy,
        rows,
    )
}

fn resolve_append_region_target(
    workbook: &WorkbookContext,
    source: &Path,
    sheet_name: &str,
    region_id: Option<u32>,
    table_name: Option<&str>,
) -> Result<AppendRegionTarget> {
    match (region_id, table_name) {
        (Some(_), Some(_)) => Err(invalid_argument(
            "--region-id and --table-name are mutually exclusive",
        )),
        (None, None) => Err(invalid_argument(
            "append-region requires exactly one of --region-id or --table-name",
        )),
        (Some(region_id), None) => {
            let region = workbook.detected_region(sheet_name, region_id).map_err(|_| {
                invalid_argument(format!(
                    "region {} was not found on sheet '{}'; run `asp sheet-overview {} {}` to inspect detected region ids",
                    region_id,
                    sheet_name,
                    source.display(),
                    sheet_name
                ))
            })?;
            let bounds = parse_append_region_bounds(&region.bounds).ok_or_else(|| {
                invalid_argument(format!(
                    "detected region {} on sheet '{}' has unsupported bounds '{}'",
                    region_id, sheet_name, region.bounds
                ))
            })?;
            Ok(AppendRegionTarget {
                sheet_name: sheet_name.to_string(),
                target_kind: AppendRegionTargetKind::DetectedRegion,
                region_id: Some(region_id),
                table_name: None,
                bounds,
                region_bounds: region.bounds,
                header_row: region.header_row,
                headers_truncated: region.headers_truncated,
            })
        }
        (None, Some(table_name)) => resolve_append_table_target(workbook, sheet_name, table_name),
    }
}

fn resolve_append_table_target(
    workbook: &WorkbookContext,
    sheet_name: &str,
    table_name: &str,
) -> Result<AppendRegionTarget> {
    let lower_name = table_name.to_ascii_lowercase();
    let items = workbook.named_items()?;
    let same_sheet = |item: &crate::model::NamedRangeDescriptor| {
        item.sheet_name
            .as_deref()
            .map(|item_sheet| item_sheet.eq_ignore_ascii_case(sheet_name))
            .unwrap_or(false)
    };

    let exact_matches: Vec<_> = items
        .iter()
        .filter(|item| item.kind == NamedItemKind::Table)
        .filter(|item| same_sheet(item))
        .filter(|item| item.name.eq_ignore_ascii_case(table_name))
        .cloned()
        .collect();
    let candidates = if !exact_matches.is_empty() {
        exact_matches
    } else {
        items
            .into_iter()
            .filter(|item| item.kind == NamedItemKind::Table)
            .filter(|item| same_sheet(item))
            .filter(|item| item.name.to_ascii_lowercase().contains(&lower_name))
            .collect()
    };

    let item = match candidates.len() {
        1 => candidates.into_iter().next().expect("one candidate"),
        0 => {
            return Err(invalid_argument(format!(
                "table '{}' was not found on sheet '{}'; run `asp named-ranges {}` to inspect available table names",
                table_name,
                sheet_name,
                workbook.path.display()
            )));
        }
        _ => {
            let matches = candidates
                .into_iter()
                .map(|item| item.name)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(invalid_argument(format!(
                "table '{}' matched multiple tables on sheet '{}': {}",
                table_name, sheet_name, matches
            )));
        }
    };

    let bounds = parse_append_named_item_bounds(&item.refers_to).ok_or_else(|| {
        invalid_argument(format!(
            "table '{}' on sheet '{}' has unsupported bounds '{}'",
            item.name, sheet_name, item.refers_to
        ))
    })?;

    Ok(AppendRegionTarget {
        sheet_name: sheet_name.to_string(),
        target_kind: AppendRegionTargetKind::Table,
        region_id: None,
        table_name: Some(item.name.clone()),
        region_bounds: format_a1_range(
            bounds.start_col,
            bounds.end_col,
            bounds.start_row,
            bounds.end_row,
        ),
        header_row: Some(bounds.start_row),
        headers_truncated: false,
        bounds,
    })
}

fn parse_append_named_item_bounds(raw: &str) -> Option<AppendBounds> {
    let refers_to = raw.trim().trim_start_matches('=');
    let range_part = refers_to
        .split_once('!')
        .map(|(_, rest)| rest)
        .unwrap_or(refers_to);
    parse_append_region_bounds(range_part)
}

fn append_plan_confidence(
    target: &AppendRegionTarget,
    footer_scan: &AppendFooterScan,
) -> (&'static str, String) {
    if let Some(reason) = footer_scan.footer_detection.as_deref() {
        if reason.starts_with("footer keyword") {
            return (
                "high",
                format!("explicit footer keyword detected: {}", reason),
            );
        }
        return (
            "medium",
            format!("formula-derived footer signal detected: {}", reason),
        );
    }

    if matches!(target.target_kind, AppendRegionTargetKind::Table) {
        return (
            "medium",
            format!(
                "resolved table target '{}' but found no explicit footer row",
                target.table_name.as_deref().unwrap_or_default()
            ),
        );
    }

    if target.header_row.is_some() {
        return (
            "medium",
            "detected region includes a header row but no explicit footer row was found"
                .to_string(),
        );
    }

    (
        "low",
        "no explicit header or footer cues were found; verify the append plan before apply"
            .to_string(),
    )
}

fn append_footer_policy_label(policy: AppendFooterPolicy) -> &'static str {
    match policy {
        AppendFooterPolicy::Auto => "auto",
        AppendFooterPolicy::BeforeFooter => "before_footer",
        AppendFooterPolicy::AppendAtEnd => "append_at_end",
    }
}

pub fn apply_append_region_plan_to_file(path: &Path, plan: &AppendRegionPlan) -> Result<()> {
    let mut book = crate::xlsx_import::read(path)
        .with_context(|| format!("failed to read workbook '{}'", path.display()))?;
    apply_append_region_plan_to_workbook(&mut book, plan)?;
    crate::xlsx_export::write(&book, path)
        .with_context(|| format!("failed to write workbook '{}'", path.display()))
}

pub fn apply_append_region_plan_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    plan: &AppendRegionPlan,
) -> Result<()> {
    let structure_ops = vec![StructureOp::InsertRows {
        sheet_name: plan.sheet_name.clone(),
        at_row: plan.insert_at_row,
        count: plan.rows_appended,
        expand_adjacent_sums: true,
    }];
    apply_structure_ops_to_workbook(book, &structure_ops, FormulaParsePolicy::Warn)?;
    let transform_ops = vec![TransformOp::WriteMatrix {
        sheet_name: plan.sheet_name.clone(),
        anchor: plan.target_anchor.clone(),
        rows: plan.rows.clone(),
        overwrite_formulas: false,
    }];
    apply_transform_ops_to_workbook(book, &transform_ops)?;
    if matches!(plan.target_kind, AppendRegionTargetKind::Table)
        && let Some(table_name) = plan.table_name.as_deref()
    {
        expand_table_target_on_workbook(book, &plan.sheet_name, table_name, plan.rows_appended)?;
    }
    Ok(())
}

fn expand_table_target_on_workbook(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    table_name: &str,
    appended_rows: u32,
) -> Result<()> {
    let sheet = book
        .get_sheet_by_name_mut(sheet_name).ok()
        .ok_or_else(|| invalid_argument(format!("sheet '{}' was not found", sheet_name)))?;
    let table = sheet
        .get_tables_mut()
        .iter_mut()
        .find(|table| {
            table.get_name().eq_ignore_ascii_case(table_name)
                || table.get_display_name().eq_ignore_ascii_case(table_name)
        })
        .ok_or_else(|| {
            invalid_argument(format!(
                "table '{}' was not found on sheet '{}' after append",
                table_name, sheet_name
            ))
        })?;
    let start_col = table.get_area().0.get_col_num();
    let start_row = table.get_area().0.get_row_num();
    let end_col = table.get_area().1.get_col_num();
    let end_row = table.get_area().1.get_row_num();
    table.set_area(((start_col, start_row), (end_col, end_row + appended_rows)));
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CloneHelperKind {
    CloneTemplateRow,
    CloneRowBand,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CloneAnchorKind {
    Before,
    After,
    InsertAt,
}

#[derive(Debug, Serialize, Clone)]
struct CloneTemplateSummary {
    non_empty_cell_count: u32,
    formula_cell_count: u32,
    style_cell_count: u32,
    validation_cell_count: u32,
    merged_ranges_fully_contained: Vec<String>,
    merged_ranges_crossing_boundary: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CloneTemplateRowResponse {
    mode: String,
    file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_path: Option<String>,
    sheet_name: String,
    helper_kind: CloneHelperKind,
    source_row: u32,
    source_row_range: String,
    anchor_kind: CloneAnchorKind,
    anchor_row: u32,
    insert_at_row: u32,
    count: u32,
    rows_inserted: u32,
    inserted_row_range: String,
    expand_adjacent_sums: bool,
    patch_target_mode: String,
    merge_policy: String,
    template_summary: CloneTemplateSummary,
    formula_targets: Vec<String>,
    likely_patch_targets: Vec<String>,
    adjacent_sum_targets: Vec<String>,
    warnings: Vec<String>,
    confidence: String,
    confidence_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    would_change: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CloneTemplateRowPlan {
    sheet_name: String,
    helper_kind: CloneHelperKind,
    source_row: u32,
    source_row_range: String,
    anchor_kind: CloneAnchorKind,
    anchor_row: u32,
    insert_at_row: u32,
    count: u32,
    rows_inserted: u32,
    inserted_row_range: String,
    expand_adjacent_sums: bool,
    patch_target_mode: String,
    merge_policy: String,
    template_summary: CloneTemplateSummary,
    formula_targets: Vec<String>,
    likely_patch_targets: Vec<String>,
    adjacent_sum_targets: Vec<String>,
    warnings: Vec<String>,
    confidence: String,
    confidence_reason: String,
    #[serde(skip)]
    contained_merges: Vec<CloneMergeSpan>,
    #[serde(skip)]
    contained_validations: Vec<CloneValidationSpec>,
    #[serde(skip)]
    row_dimension: Option<umya_spreadsheet::structs::Row>,
}

#[derive(Debug, Clone)]
struct CloneTemplateCellPreview {
    col: u32,
    value: String,
    is_formula: bool,
}

#[derive(Debug, Clone)]
struct CloneMergeSpan {
    start_col: u32,
    end_col: u32,
    range: String,
}

#[derive(Debug, Clone)]
struct CloneValidationSpec {
    data_validation: umya_spreadsheet::structs::DataValidation,
    start_col: u32,
    end_col: u32,
    start_row_offset: u32,
    end_row_offset: u32,
}

#[derive(Debug, Clone)]
struct CloneTemplateCellData {
    col: u32,
    value: String,
    formula: Option<String>,
    style: umya_spreadsheet::Style,
}

#[derive(Debug, Clone)]
struct CloneBandTemplateRow {
    source_row: u32,
    row_offset: u32,
    preview_cells: Vec<CloneTemplateCellPreview>,
    cell_data: Vec<CloneTemplateCellData>,
    row_dimension: Option<umya_spreadsheet::structs::Row>,
}

#[derive(Debug, Clone, Serialize)]
struct CloneInsertedBlock {
    block_index: u32,
    row_range: String,
}

#[derive(Debug, Serialize)]
pub struct CloneRowBandResponse {
    mode: String,
    file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_path: Option<String>,
    sheet_name: String,
    helper_kind: CloneHelperKind,
    source_row_range: String,
    source_row_count: u32,
    anchor_kind: CloneAnchorKind,
    anchor_row: u32,
    insert_at_row: u32,
    repeat: u32,
    rows_inserted: u32,
    inserted_row_range: String,
    inserted_blocks: Vec<CloneInsertedBlock>,
    expand_adjacent_sums: bool,
    patch_target_mode: String,
    merge_policy: String,
    template_summary: CloneTemplateSummary,
    formula_targets: Vec<String>,
    likely_patch_targets: Vec<String>,
    adjacent_sum_targets: Vec<String>,
    warnings: Vec<String>,
    confidence: String,
    confidence_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    would_change: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CloneRowBandPlan {
    sheet_name: String,
    helper_kind: CloneHelperKind,
    source_row_range: String,
    source_row_count: u32,
    anchor_kind: CloneAnchorKind,
    anchor_row: u32,
    insert_at_row: u32,
    repeat: u32,
    rows_inserted: u32,
    inserted_row_range: String,
    inserted_blocks: Vec<CloneInsertedBlock>,
    expand_adjacent_sums: bool,
    patch_target_mode: String,
    merge_policy: String,
    template_summary: CloneTemplateSummary,
    formula_targets: Vec<String>,
    likely_patch_targets: Vec<String>,
    adjacent_sum_targets: Vec<String>,
    warnings: Vec<String>,
    confidence: String,
    confidence_reason: String,
    #[serde(skip)]
    template_rows: Vec<CloneBandTemplateRow>,
    #[serde(skip)]
    contained_merges: Vec<CloneBandMergeSpan>,
    #[serde(skip)]
    contained_validations: Vec<CloneValidationSpec>,
}

#[derive(Debug, Clone)]
struct CloneBandMergeSpan {
    start_col: u32,
    end_col: u32,
    start_row_offset: u32,
    end_row_offset: u32,
    range: String,
}

#[allow(clippy::too_many_arguments)]
pub fn build_clone_template_row_plan(
    source: &Path,
    sheet_name: &str,
    source_row: u32,
    before: Option<u32>,
    after: Option<u32>,
    insert_at: Option<u32>,
    count: u32,
    expand_adjacent_sums: bool,
    patch_targets: ClonePatchTargets,
    merge_policy: CloneMergePolicy,
) -> Result<CloneTemplateRowPlan> {
    let book = crate::xlsx_import::read(source)
        .with_context(|| format!("failed to read workbook '{}'", source.display()))?;
    build_clone_template_row_plan_from_workbook(
        &book,
        sheet_name,
        source_row,
        before,
        after,
        insert_at,
        count,
        expand_adjacent_sums,
        patch_targets,
        merge_policy,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_clone_template_row_plan_from_workbook(
    book: &umya_spreadsheet::Workbook,
    sheet_name: &str,
    source_row: u32,
    before: Option<u32>,
    after: Option<u32>,
    insert_at: Option<u32>,
    count: u32,
    expand_adjacent_sums: bool,
    patch_targets: ClonePatchTargets,
    merge_policy: CloneMergePolicy,
) -> Result<CloneTemplateRowPlan> {
    if source_row == 0 {
        return Err(invalid_argument("--source-row must be at least 1"));
    }
    if count == 0 {
        return Err(invalid_argument("--count must be at least 1"));
    }
    let (anchor_kind, anchor_row, insert_at_row) = resolve_clone_anchor(before, after, insert_at)?;
    let sheet = book
        .get_sheet_by_name(sheet_name).ok()
        .ok_or_else(|| invalid_argument(format!("sheet '{}' was not found", sheet_name)))?;

    let template_cells = inspect_template_row_cells(sheet, source_row);
    let (contained_merges, crossing_merges) = inspect_clone_row_merges(sheet, source_row)?;
    let (contained_validations, crossing_validations, validation_cell_count) =
        inspect_clone_row_validations(sheet, source_row)?;

    if matches!(merge_policy, CloneMergePolicy::Strict) && !crossing_merges.is_empty() {
        return Err(unsafe_clone_template(format!(
            "source row {} intersects merged ranges that cross the clone boundary: {}",
            source_row,
            crossing_merges.join(", ")
        )));
    }

    let source_row_range = format!("{}:{}", source_row, source_row);
    let inserted_row_range = format!(
        "{}:{}",
        insert_at_row,
        insert_at_row + count.saturating_sub(1)
    );

    let formula_targets = build_clone_formula_targets(&template_cells, insert_at_row, count);
    let likely_patch_targets = build_clone_patch_targets(
        &template_cells,
        &contained_validations,
        insert_at_row,
        count,
        patch_targets,
    );
    let adjacent_sum_targets = if expand_adjacent_sums {
        preview_adjacent_sum_targets(sheet, insert_at_row, count)
    } else {
        Vec::new()
    };

    let non_empty_cell_count = template_cells
        .iter()
        .filter(|cell| cell.is_formula || !cell.value.trim().is_empty())
        .count() as u32;
    let formula_cell_count = template_cells.iter().filter(|cell| cell.is_formula).count() as u32;
    let style_cell_count = template_cells.len() as u32;

    let mut warnings = Vec::new();
    if template_cells.is_empty() {
        warnings.push(format!(
            "source row {} has no materialized cells; cloning will insert blank rows",
            source_row
        ));
    }
    if !crossing_merges.is_empty() {
        warnings.push(format!(
            "merge-policy '{}' will not reproduce boundary-crossing merged ranges: {}",
            clone_merge_policy_label(merge_policy),
            crossing_merges.join(", ")
        ));
    }
    if !crossing_validations.is_empty() {
        warnings.push(format!(
            "row-scoped validation cloning skipped boundary-crossing validation ranges: {}",
            crossing_validations.join(", ")
        ));
    }
    if expand_adjacent_sums && adjacent_sum_targets.is_empty() {
        warnings.push(
            "no adjacent SUM footer formulas qualified for expansion below the inserted rows"
                .to_string(),
        );
    }

    let (confidence, confidence_reason) = if template_cells.is_empty() {
        (
            "low",
            "template row has no materialized cells; verify that inserting blank rows is intended"
                .to_string(),
        )
    } else if !crossing_merges.is_empty() || !crossing_validations.is_empty() {
        (
            "medium",
            "clone can proceed, but boundary-crossing merges or validations will not be fully reproduced"
                .to_string(),
        )
    } else {
        (
            "high",
            "template row cloned cleanly with no merge or validation boundary conflicts"
                .to_string(),
        )
    };

    Ok(CloneTemplateRowPlan {
        sheet_name: sheet_name.to_string(),
        helper_kind: CloneHelperKind::CloneTemplateRow,
        source_row,
        source_row_range,
        anchor_kind,
        anchor_row,
        insert_at_row,
        count,
        rows_inserted: count,
        inserted_row_range,
        expand_adjacent_sums,
        patch_target_mode: clone_patch_targets_label(patch_targets).to_string(),
        merge_policy: clone_merge_policy_label(merge_policy).to_string(),
        template_summary: CloneTemplateSummary {
            non_empty_cell_count,
            formula_cell_count,
            style_cell_count,
            validation_cell_count,
            merged_ranges_fully_contained: contained_merges
                .iter()
                .map(|span| span.range.clone())
                .collect(),
            merged_ranges_crossing_boundary: crossing_merges,
        },
        formula_targets,
        likely_patch_targets,
        adjacent_sum_targets,
        warnings,
        confidence: confidence.to_string(),
        confidence_reason,
        contained_merges,
        contained_validations,
        row_dimension: sheet.get_row_dimension(&source_row).cloned(),
    })
}

fn resolve_clone_anchor(
    before: Option<u32>,
    after: Option<u32>,
    insert_at: Option<u32>,
) -> Result<(CloneAnchorKind, u32, u32)> {
    let selections = before.is_some() as u8 + after.is_some() as u8 + insert_at.is_some() as u8;
    if selections != 1 {
        return Err(invalid_argument(
            "choose exactly one of --before <ROW>, --after <ROW>, or --insert-at <ROW>",
        ));
    }

    if let Some(row) = before {
        if row == 0 {
            return Err(invalid_argument("--before must be at least 1"));
        }
        return Ok((CloneAnchorKind::Before, row, row));
    }
    if let Some(row) = after {
        if row == 0 {
            return Err(invalid_argument("--after must be at least 1"));
        }
        return Ok((CloneAnchorKind::After, row, row + 1));
    }
    let row = insert_at.expect("one anchor row required");
    if row == 0 {
        return Err(invalid_argument("--insert-at must be at least 1"));
    }
    Ok((CloneAnchorKind::InsertAt, row, row))
}

fn inspect_template_row_cells(
    sheet: &umya_spreadsheet::Worksheet,
    source_row: u32,
) -> Vec<CloneTemplateCellPreview> {
    let max_col = sheet.get_highest_column();
    let mut cells = Vec::new();
    for col in 1..=max_col {
        let Some(cell) = sheet.get_cell((col, source_row)) else {
            continue;
        };
        cells.push(CloneTemplateCellPreview {
            col,
            value: cell.get_value().to_string(),
            is_formula: cell.is_formula(),
        });
    }
    cells
}

fn inspect_clone_row_merges(
    sheet: &umya_spreadsheet::Worksheet,
    source_row: u32,
) -> Result<(Vec<CloneMergeSpan>, Vec<String>)> {
    let mut contained = Vec::new();
    let mut crossing = Vec::new();
    for range in sheet.get_merge_cells() {
        let raw = range.get_range();
        let Some(bounds) = parse_append_region_bounds(&raw) else {
            continue;
        };
        if !(bounds.start_row..=bounds.end_row).contains(&source_row) {
            continue;
        }
        if bounds.start_row == source_row && bounds.end_row == source_row {
            contained.push(CloneMergeSpan {
                start_col: bounds.start_col,
                end_col: bounds.end_col,
                range: raw,
            });
        } else {
            crossing.push(raw);
        }
    }
    Ok((contained, crossing))
}

fn inspect_clone_row_validations(
    sheet: &umya_spreadsheet::Worksheet,
    source_row: u32,
) -> Result<(Vec<CloneValidationSpec>, Vec<String>, u32)> {
    let mut contained = Vec::new();
    let mut crossing = Vec::new();
    let mut validation_cols = BTreeSet::new();

    let Some(validations) = sheet.get_data_validations() else {
        return Ok((contained, crossing, 0));
    };

    for data_validation in validations.get_data_validation_list() {
        let mut seen_ranges = BTreeSet::new();
        for range in data_validation
            .get_sequence_of_references()
            .get_range_collection()
        {
            let raw = range.get_range();
            if !seen_ranges.insert(raw.clone()) {
                continue;
            }
            let Some(bounds) = parse_append_region_bounds(&raw) else {
                continue;
            };
            if !(bounds.start_row..=bounds.end_row).contains(&source_row) {
                continue;
            }
            for col in bounds.start_col..=bounds.end_col {
                validation_cols.insert(col);
            }
            if bounds.start_row == source_row && bounds.end_row == source_row {
                let mut clone = data_validation.clone();
                clone
                    .get_sequence_of_references_mut()
                    .remove_range_collection()
                    .set_sqref(format_a1_range(
                        bounds.start_col,
                        bounds.end_col,
                        source_row,
                        source_row,
                    ));
                contained.push(CloneValidationSpec {
                    data_validation: clone,
                    start_col: bounds.start_col,
                    end_col: bounds.end_col,
                    start_row_offset: 0,
                    end_row_offset: 0,
                });
            } else {
                crossing.push(raw);
            }
        }
    }

    Ok((contained, crossing, validation_cols.len() as u32))
}

fn build_clone_formula_targets(
    template_cells: &[CloneTemplateCellPreview],
    insert_at_row: u32,
    count: u32,
) -> Vec<String> {
    let formula_cols: Vec<u32> = template_cells
        .iter()
        .filter(|cell| cell.is_formula)
        .map(|cell| cell.col)
        .collect();
    let mut targets = Vec::new();
    for row in insert_at_row..(insert_at_row + count) {
        for col in &formula_cols {
            targets.push(format!("{}{}", column_number_to_name(*col), row));
        }
    }
    targets
}

fn get_likely_input_cols_for_row(
    preview_cells: &[CloneTemplateCellPreview],
    validations: &[CloneValidationSpec],
    row_offset: u32,
    patch_targets: ClonePatchTargets,
) -> Vec<u32> {
    let mut target_cols = std::collections::BTreeSet::new();

    match patch_targets {
        ClonePatchTargets::None => {}
        ClonePatchTargets::AllNonFormula => {
            for cell in preview_cells {
                if !cell.is_formula {
                    target_cols.insert(cell.col);
                }
            }
        }
        ClonePatchTargets::LikelyInputs => {
            for cell in preview_cells {
                if cell.is_formula {
                    continue;
                }
                if looks_like_footer_label(&cell.value) {
                    continue;
                }
                let is_numeric = cell.value.trim().parse::<f64>().is_ok();
                let has_validation = validations.iter().any(|v| {
                    v.start_col <= cell.col
                        && v.end_col >= cell.col
                        && v.start_row_offset <= row_offset
                        && v.end_row_offset >= row_offset
                });
                if is_numeric || has_validation {
                    target_cols.insert(cell.col);
                }
            }

            // Add completely empty cells that have validation
            for v in validations {
                if v.start_row_offset <= row_offset && v.end_row_offset >= row_offset {
                    for col in v.start_col..=v.end_col {
                        target_cols.insert(col);
                    }
                }
            }
        }
    }

    target_cols.into_iter().collect()
}

fn build_clone_patch_targets(
    template_cells: &[CloneTemplateCellPreview],
    validations: &[CloneValidationSpec],
    insert_at_row: u32,
    count: u32,
    patch_targets: ClonePatchTargets,
) -> Vec<String> {
    let target_cols = get_likely_input_cols_for_row(template_cells, validations, 0, patch_targets);

    let mut targets = Vec::new();
    for row in insert_at_row..(insert_at_row + count) {
        for col in &target_cols {
            targets.push(format!("{}{}", column_number_to_name(*col), row));
        }
    }
    targets
}

fn looks_like_footer_label(value: &str) -> bool {
    let text = value.trim().to_ascii_lowercase();
    text.starts_with("total")
        || text.contains("grand total")
        || text.contains("subtotal")
        || text.contains("footer")
}

fn preview_adjacent_sum_targets(
    sheet: &umya_spreadsheet::Worksheet,
    insert_at_row: u32,
    count: u32,
) -> Vec<String> {
    let mut targets = Vec::new();
    let pre_shift_subtotal_row = insert_at_row;
    let post_shift_subtotal_row = insert_at_row + count;
    let sum_re = simple_sum_range_regex();
    let max_col = sheet.get_highest_column();
    for col in 1..=max_col {
        let Some(cell) = sheet.get_cell((col, pre_shift_subtotal_row)) else {
            continue;
        };
        if !cell.is_formula() {
            continue;
        }
        let formula_text = cell.get_formula().to_string();
        let formula_bare = formula_text.strip_prefix('=').unwrap_or(&formula_text);
        let Some(caps) = sum_re.captures(formula_bare) else {
            continue;
        };
        let col1 = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        let col2 = caps.get(3).map(|m| m.as_str()).unwrap_or_default();
        let row2: u32 = caps
            .get(4)
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        if col1 == col2 && row2 + 1 == insert_at_row {
            targets.push(format!(
                "{}{}",
                column_number_to_name(col),
                post_shift_subtotal_row
            ));
        }
    }
    targets
}

fn simple_sum_range_regex() -> Regex {
    Regex::new(r"(?i)^SUM\(([A-Z]{1,3})(\d+):([A-Z]{1,3})(\d+)\)$").expect("valid simple sum regex")
}

fn copy_supported_row_dimension(
    source: &umya_spreadsheet::structs::Row,
    destination: &mut umya_spreadsheet::structs::Row,
) {
    destination
        .set_height(source.get_height())
        .set_descent(source.get_descent())
        .set_thick_bot(source.get_thick_bot())
        .set_custom_height(source.get_custom_height())
        .set_hidden(source.get_hidden())
        .set_style(source.get_style().clone());
}

pub fn apply_clone_template_row_plan_to_file(
    path: &Path,
    plan: &CloneTemplateRowPlan,
) -> Result<()> {
    let structure_ops = vec![StructureOp::CloneRow {
        sheet_name: plan.sheet_name.clone(),
        source_row: plan.source_row,
        insert_at: plan.insert_at_row,
        count: plan.count,
        expand_adjacent_sums: plan.expand_adjacent_sums,
    }];
    let mut book = crate::xlsx_import::read(path)
        .with_context(|| format!("failed to read workbook '{}'", path.display()))?;
    apply_structure_ops_to_workbook(&mut book, &structure_ops, FormulaParsePolicy::Warn)?;
    apply_clone_template_row_postprocess(&mut book, plan)?;
    crate::xlsx_export::write(&book, path)
        .with_context(|| format!("failed to write workbook '{}'", path.display()))?;
    Ok(())
}

pub fn apply_clone_template_row_plan_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    plan: &CloneTemplateRowPlan,
) -> Result<()> {
    let structure_ops = vec![StructureOp::CloneRow {
        sheet_name: plan.sheet_name.clone(),
        source_row: plan.source_row,
        insert_at: plan.insert_at_row,
        count: plan.count,
        expand_adjacent_sums: plan.expand_adjacent_sums,
    }];
    apply_structure_ops_to_workbook(book, &structure_ops, FormulaParsePolicy::Warn)?;
    apply_clone_template_row_postprocess(book, plan)
}

fn apply_clone_template_row_postprocess(
    book: &mut umya_spreadsheet::Workbook,
    plan: &CloneTemplateRowPlan,
) -> Result<()> {
    if plan.contained_merges.is_empty()
        && plan.contained_validations.is_empty()
        && plan.row_dimension.is_none()
    {
        return Ok(());
    }
    let sheet = book
        .get_sheet_by_name_mut(&plan.sheet_name).ok()
        .ok_or_else(|| invalid_argument(format!("sheet '{}' was not found", plan.sheet_name)))?;

    for copy_idx in 0..plan.count {
        let dest_row = plan.insert_at_row + copy_idx;
        if let Some(source) = &plan.row_dimension {
            copy_supported_row_dimension(source, sheet.get_row_dimension_mut(&dest_row));
        }
        for merge in &plan.contained_merges {
            sheet.add_merge_cells(format_a1_range(
                merge.start_col,
                merge.end_col,
                dest_row,
                dest_row,
            ));
        }
    }

    if !plan.contained_validations.is_empty() {
        if sheet.get_data_validations().is_none() {
            sheet.set_data_validations(umya_spreadsheet::structs::DataValidations::default());
        }
        let validations = sheet
            .get_data_validations_mut()
            .expect("data validations exist after initialization");
        for copy_idx in 0..plan.count {
            let dest_row = plan.insert_at_row + copy_idx;
            for spec in &plan.contained_validations {
                let mut clone = spec.data_validation.clone();
                clone
                    .get_sequence_of_references_mut()
                    .remove_range_collection()
                    .set_sqref(format_a1_range(
                        spec.start_col,
                        spec.end_col,
                        dest_row,
                        dest_row,
                    ));
                validations.add_data_validation_list(clone);
            }
        }
    }

    Ok(())
}

fn clone_patch_targets_label(mode: ClonePatchTargets) -> &'static str {
    match mode {
        ClonePatchTargets::LikelyInputs => "likely_inputs",
        ClonePatchTargets::AllNonFormula => "all_non_formula",
        ClonePatchTargets::None => "none",
    }
}

fn clone_merge_policy_label(policy: CloneMergePolicy) -> &'static str {
    match policy {
        CloneMergePolicy::Safe => "safe",
        CloneMergePolicy::Strict => "strict",
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_clone_row_band_plan(
    source: &Path,
    sheet_name: &str,
    source_rows: &str,
    before: Option<u32>,
    after: Option<u32>,
    insert_at: Option<u32>,
    repeat: u32,
    expand_adjacent_sums: bool,
    patch_targets: ClonePatchTargets,
    merge_policy: CloneMergePolicy,
) -> Result<CloneRowBandPlan> {
    let book = crate::xlsx_import::read(source)
        .with_context(|| format!("failed to read workbook '{}'", source.display()))?;
    build_clone_row_band_plan_from_workbook(
        &book,
        sheet_name,
        source_rows,
        before,
        after,
        insert_at,
        repeat,
        expand_adjacent_sums,
        patch_targets,
        merge_policy,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_clone_row_band_plan_from_workbook(
    book: &umya_spreadsheet::Workbook,
    sheet_name: &str,
    source_rows: &str,
    before: Option<u32>,
    after: Option<u32>,
    insert_at: Option<u32>,
    repeat: u32,
    expand_adjacent_sums: bool,
    patch_targets: ClonePatchTargets,
    merge_policy: CloneMergePolicy,
) -> Result<CloneRowBandPlan> {
    if repeat == 0 {
        return Err(invalid_argument("--repeat must be at least 1"));
    }
    let (source_start_row, source_end_row) = parse_clone_row_band(source_rows)?;
    let source_row_count = source_end_row - source_start_row + 1;
    let (anchor_kind, anchor_row, insert_at_row) = resolve_clone_anchor(before, after, insert_at)?;
    let sheet = book
        .get_sheet_by_name(sheet_name).ok()
        .ok_or_else(|| invalid_argument(format!("sheet '{}' was not found", sheet_name)))?;

    let template_rows = inspect_clone_band_rows(sheet, source_start_row, source_end_row);
    let (contained_merges, crossing_merges) =
        inspect_clone_band_merges(sheet, source_start_row, source_end_row)?;
    let (contained_validations, crossing_validations, validation_cell_count) =
        inspect_clone_band_validations(sheet, source_start_row, source_end_row)?;

    if matches!(merge_policy, CloneMergePolicy::Strict) && !crossing_merges.is_empty() {
        return Err(unsafe_clone_template(format!(
            "source rows {}:{} intersect merged ranges that cross the clone boundary: {}",
            source_start_row,
            source_end_row,
            crossing_merges.join(", ")
        )));
    }

    let rows_inserted = source_row_count * repeat;
    let source_row_range = format!("{}:{}", source_start_row, source_end_row);
    let inserted_row_range = format!(
        "{}:{}",
        insert_at_row,
        insert_at_row + rows_inserted.saturating_sub(1)
    );
    let inserted_blocks = build_clone_inserted_blocks(insert_at_row, source_row_count, repeat);
    let formula_targets =
        build_clone_band_formula_targets(&template_rows, insert_at_row, source_row_count, repeat);
    let likely_patch_targets = build_clone_band_patch_targets(
        &template_rows,
        &contained_validations,
        insert_at_row,
        source_row_count,
        repeat,
        patch_targets,
    );
    let adjacent_sum_targets = if expand_adjacent_sums {
        preview_adjacent_sum_targets(sheet, insert_at_row, rows_inserted)
    } else {
        Vec::new()
    };

    let non_empty_cell_count = template_rows
        .iter()
        .flat_map(|row| row.preview_cells.iter())
        .filter(|cell| cell.is_formula || !cell.value.trim().is_empty())
        .count() as u32;
    let formula_cell_count = template_rows
        .iter()
        .flat_map(|row| row.preview_cells.iter())
        .filter(|cell| cell.is_formula)
        .count() as u32;
    let style_cell_count = template_rows
        .iter()
        .map(|row| row.cell_data.len() as u32)
        .sum();

    let mut warnings = Vec::new();
    if template_rows.iter().all(|row| row.cell_data.is_empty()) {
        warnings.push(format!(
            "source rows {}:{} have no materialized cells; cloning will insert blank rows",
            source_start_row, source_end_row
        ));
    }
    if !crossing_merges.is_empty() {
        warnings.push(format!(
            "merge-policy '{}' will not reproduce boundary-crossing merged ranges: {}",
            clone_merge_policy_label(merge_policy),
            crossing_merges.join(", ")
        ));
    }
    if !crossing_validations.is_empty() {
        warnings.push(format!(
            "band validation cloning skipped boundary-crossing validation ranges: {}",
            crossing_validations.join(", ")
        ));
    }
    if expand_adjacent_sums && adjacent_sum_targets.is_empty() {
        warnings.push(
            "no adjacent SUM footer formulas qualified for expansion below the inserted rows"
                .to_string(),
        );
    }

    let (confidence, confidence_reason) = if template_rows
        .iter()
        .all(|row| row.cell_data.is_empty())
    {
        (
            "low",
            "source row band has no materialized cells; verify that inserting blank rows is intended"
                .to_string(),
        )
    } else if !crossing_merges.is_empty() || !crossing_validations.is_empty() {
        (
            "medium",
            "clone can proceed, but boundary-crossing merges or validations will not be fully reproduced"
                .to_string(),
        )
    } else {
        (
            "high",
            "row band cloned cleanly with no merge or validation boundary conflicts".to_string(),
        )
    };

    Ok(CloneRowBandPlan {
        sheet_name: sheet_name.to_string(),
        helper_kind: CloneHelperKind::CloneRowBand,
        source_row_range,
        source_row_count,
        anchor_kind,
        anchor_row,
        insert_at_row,
        repeat,
        rows_inserted,
        inserted_row_range,
        inserted_blocks,
        expand_adjacent_sums,
        patch_target_mode: clone_patch_targets_label(patch_targets).to_string(),
        merge_policy: clone_merge_policy_label(merge_policy).to_string(),
        template_summary: CloneTemplateSummary {
            non_empty_cell_count,
            formula_cell_count,
            style_cell_count,
            validation_cell_count,
            merged_ranges_fully_contained: contained_merges
                .iter()
                .map(|span| span.range.clone())
                .collect(),
            merged_ranges_crossing_boundary: crossing_merges,
        },
        formula_targets,
        likely_patch_targets,
        adjacent_sum_targets,
        warnings,
        confidence: confidence.to_string(),
        confidence_reason,
        template_rows,
        contained_merges,
        contained_validations,
    })
}

fn parse_clone_row_band(raw: &str) -> Result<(u32, u32)> {
    let (start, end) = raw
        .split_once(':')
        .ok_or_else(|| invalid_argument("--source-rows must use START:END notation"))?;
    let start_row = start
        .trim()
        .parse::<u32>()
        .map_err(|_| invalid_argument("--source-rows start row must be a positive integer"))?;
    let end_row = end
        .trim()
        .parse::<u32>()
        .map_err(|_| invalid_argument("--source-rows end row must be a positive integer"))?;
    if start_row == 0 || end_row == 0 {
        return Err(invalid_argument(
            "--source-rows values must both be at least 1",
        ));
    }
    if start_row > end_row {
        return Err(invalid_argument(
            "--source-rows must be an ascending contiguous range like 12:14",
        ));
    }
    Ok((start_row, end_row))
}

fn inspect_clone_band_rows(
    sheet: &umya_spreadsheet::Worksheet,
    source_start_row: u32,
    source_end_row: u32,
) -> Vec<CloneBandTemplateRow> {
    let max_col = sheet.get_highest_column();
    let mut rows = Vec::new();
    for source_row in source_start_row..=source_end_row {
        let row_offset = source_row - source_start_row;
        let mut preview_cells = Vec::new();
        let mut cell_data = Vec::new();
        for col in 1..=max_col {
            let Some(cell) = sheet.get_cell((col, source_row)) else {
                continue;
            };
            let value = cell.get_value().to_string();
            let is_formula = cell.is_formula();
            preview_cells.push(CloneTemplateCellPreview {
                col,
                value: value.clone(),
                is_formula,
            });
            cell_data.push(CloneTemplateCellData {
                col,
                value,
                formula: if is_formula {
                    Some(cell.get_formula().to_string())
                } else {
                    None
                },
                style: cell.get_style().clone(),
            });
        }
        rows.push(CloneBandTemplateRow {
            source_row,
            row_offset,
            preview_cells,
            cell_data,
            row_dimension: sheet.get_row_dimension(&source_row).cloned(),
        });
    }
    rows
}

fn inspect_clone_band_merges(
    sheet: &umya_spreadsheet::Worksheet,
    source_start_row: u32,
    source_end_row: u32,
) -> Result<(Vec<CloneBandMergeSpan>, Vec<String>)> {
    let mut contained = Vec::new();
    let mut crossing = Vec::new();
    for range in sheet.get_merge_cells() {
        let raw = range.get_range();
        let Some(bounds) = parse_append_region_bounds(&raw) else {
            continue;
        };
        if bounds.end_row < source_start_row || bounds.start_row > source_end_row {
            continue;
        }
        if bounds.start_row >= source_start_row && bounds.end_row <= source_end_row {
            contained.push(CloneBandMergeSpan {
                start_col: bounds.start_col,
                end_col: bounds.end_col,
                start_row_offset: bounds.start_row - source_start_row,
                end_row_offset: bounds.end_row - source_start_row,
                range: raw,
            });
        } else {
            crossing.push(raw);
        }
    }
    Ok((contained, crossing))
}

fn inspect_clone_band_validations(
    sheet: &umya_spreadsheet::Worksheet,
    source_start_row: u32,
    source_end_row: u32,
) -> Result<(Vec<CloneValidationSpec>, Vec<String>, u32)> {
    let mut contained = Vec::new();
    let mut crossing = Vec::new();
    let mut validation_cells = BTreeSet::new();

    let Some(validations) = sheet.get_data_validations() else {
        return Ok((contained, crossing, 0));
    };

    for data_validation in validations.get_data_validation_list() {
        let mut seen_ranges = BTreeSet::new();
        for range in data_validation
            .get_sequence_of_references()
            .get_range_collection()
        {
            let raw = range.get_range();
            if !seen_ranges.insert(raw.clone()) {
                continue;
            }
            let Some(bounds) = parse_append_region_bounds(&raw) else {
                continue;
            };
            if bounds.end_row < source_start_row || bounds.start_row > source_end_row {
                continue;
            }
            let intersect_start = bounds.start_row.max(source_start_row);
            let intersect_end = bounds.end_row.min(source_end_row);
            for row in intersect_start..=intersect_end {
                for col in bounds.start_col..=bounds.end_col {
                    validation_cells.insert((row, col));
                }
            }
            if bounds.start_row >= source_start_row && bounds.end_row <= source_end_row {
                let mut clone = data_validation.clone();
                clone
                    .get_sequence_of_references_mut()
                    .remove_range_collection()
                    .set_sqref(format_a1_range(
                        bounds.start_col,
                        bounds.end_col,
                        bounds.start_row,
                        bounds.end_row,
                    ));
                contained.push(CloneValidationSpec {
                    data_validation: clone,
                    start_col: bounds.start_col,
                    end_col: bounds.end_col,
                    start_row_offset: bounds.start_row - source_start_row,
                    end_row_offset: bounds.end_row - source_start_row,
                });
            } else {
                crossing.push(raw);
            }
        }
    }

    Ok((contained, crossing, validation_cells.len() as u32))
}

fn build_clone_inserted_blocks(
    insert_at_row: u32,
    source_row_count: u32,
    repeat: u32,
) -> Vec<CloneInsertedBlock> {
    (0..repeat)
        .map(|block_index| {
            let start_row = insert_at_row + block_index * source_row_count;
            CloneInsertedBlock {
                block_index,
                row_range: format!(
                    "{}:{}",
                    start_row,
                    start_row + source_row_count.saturating_sub(1)
                ),
            }
        })
        .collect()
}

fn build_clone_band_formula_targets(
    template_rows: &[CloneBandTemplateRow],
    insert_at_row: u32,
    source_row_count: u32,
    repeat: u32,
) -> Vec<String> {
    let mut targets = Vec::new();
    for block_index in 0..repeat {
        let block_start = insert_at_row + block_index * source_row_count;
        for row in template_rows {
            let dest_row = block_start + row.row_offset;
            for cell in row.preview_cells.iter().filter(|cell| cell.is_formula) {
                targets.push(format!("{}{}", column_number_to_name(cell.col), dest_row));
            }
        }
    }
    targets
}

fn build_clone_band_patch_targets(
    template_rows: &[CloneBandTemplateRow],
    validations: &[CloneValidationSpec],
    insert_at_row: u32,
    source_row_count: u32,
    repeat: u32,
    patch_targets: ClonePatchTargets,
) -> Vec<String> {
    let mut targets = Vec::new();

    // precompute target cols per row_offset
    let mut cols_by_offset = std::collections::HashMap::new();
    for row in template_rows {
        let cols = get_likely_input_cols_for_row(
            &row.preview_cells,
            validations,
            row.row_offset,
            patch_targets,
        );
        cols_by_offset.insert(row.row_offset, cols);
    }

    for block_index in 0..repeat {
        let block_start = insert_at_row + block_index * source_row_count;
        for row in template_rows {
            let dest_row = block_start + row.row_offset;
            if let Some(cols) = cols_by_offset.get(&row.row_offset) {
                for col in cols {
                    targets.push(format!("{}{}", column_number_to_name(*col), dest_row));
                }
            }
        }
    }
    targets
}

pub fn apply_clone_row_band_plan_to_file(path: &Path, plan: &CloneRowBandPlan) -> Result<()> {
    let structure_ops = vec![StructureOp::InsertRows {
        sheet_name: plan.sheet_name.clone(),
        at_row: plan.insert_at_row,
        count: plan.rows_inserted,
        expand_adjacent_sums: plan.expand_adjacent_sums,
    }];
    let mut book = crate::xlsx_import::read(path)
        .with_context(|| format!("failed to read workbook '{}'", path.display()))?;
    apply_structure_ops_to_workbook(&mut book, &structure_ops, FormulaParsePolicy::Warn)?;
    apply_clone_row_band_postprocess(&mut book, plan)?;
    crate::xlsx_export::write(&book, path)
        .with_context(|| format!("failed to write workbook '{}'", path.display()))?;
    Ok(())
}

pub fn apply_clone_row_band_plan_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    plan: &CloneRowBandPlan,
) -> Result<()> {
    let structure_ops = vec![StructureOp::InsertRows {
        sheet_name: plan.sheet_name.clone(),
        at_row: plan.insert_at_row,
        count: plan.rows_inserted,
        expand_adjacent_sums: plan.expand_adjacent_sums,
    }];
    apply_structure_ops_to_workbook(book, &structure_ops, FormulaParsePolicy::Warn)?;
    apply_clone_row_band_postprocess(book, plan)
}

fn apply_clone_row_band_postprocess(
    book: &mut umya_spreadsheet::Workbook,
    plan: &CloneRowBandPlan,
) -> Result<()> {
    let sheet = book
        .get_sheet_by_name_mut(&plan.sheet_name).ok()
        .ok_or_else(|| invalid_argument(format!("sheet '{}' was not found", plan.sheet_name)))?;

    for block_index in 0..plan.repeat {
        let block_start = plan.insert_at_row + block_index * plan.source_row_count;
        for row in &plan.template_rows {
            let dest_row = block_start + row.row_offset;
            if let Some(source) = &row.row_dimension {
                copy_supported_row_dimension(source, sheet.get_row_dimension_mut(&dest_row));
            }
            for cell in &row.cell_data {
                let dest_cell = sheet.get_cell_mut((cell.col, dest_row));
                dest_cell.set_style(cell.style.clone());
                dest_cell.get_cell_value_mut().remove_formula();
                if let Some(formula) = &cell.formula {
                    let shifted = parse_base_formula(formula)
                        .and_then(|ast| {
                            shift_formula_ast(
                                &ast,
                                0,
                                dest_row as i32 - row.source_row as i32,
                                RelativeMode::Excel,
                            )
                        })
                        .ok()
                        .map(|value| value.strip_prefix('=').unwrap_or(&value).to_string())
                        .unwrap_or_else(|| formula.clone());
                    dest_cell.set_formula(shifted);
                    dest_cell.set_formula_result_default("");
                } else {
                    dest_cell.set_value(cell.value.clone());
                }
            }
        }
        for merge in &plan.contained_merges {
            sheet.add_merge_cells(format_a1_range(
                merge.start_col,
                merge.end_col,
                block_start + merge.start_row_offset,
                block_start + merge.end_row_offset,
            ));
        }
    }

    if !plan.contained_validations.is_empty() {
        if sheet.get_data_validations().is_none() {
            sheet.set_data_validations(umya_spreadsheet::structs::DataValidations::default());
        }
        let validations = sheet
            .get_data_validations_mut()
            .expect("data validations exist after initialization");
        for block_index in 0..plan.repeat {
            let block_start = plan.insert_at_row + block_index * plan.source_row_count;
            for spec in &plan.contained_validations {
                let mut clone = spec.data_validation.clone();
                clone
                    .get_sequence_of_references_mut()
                    .remove_range_collection()
                    .set_sqref(format_a1_range(
                        spec.start_col,
                        spec.end_col,
                        block_start + spec.start_row_offset,
                        block_start + spec.end_row_offset,
                    ));
                validations.add_data_validation_list(clone);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
fn detect_append_footer(
    source: &Path,
    sheet_name: &str,
    start_col: u32,
    end_col: u32,
    region_end_row: u32,
) -> Result<AppendFooterScan> {
    let book = crate::xlsx_import::read(source)
        .with_context(|| format!("failed to read workbook '{}'", source.display()))?;
    detect_append_footer_on_workbook(&book, sheet_name, start_col, end_col, region_end_row)
}

fn detect_append_footer_on_workbook(
    book: &umya_spreadsheet::Workbook,
    sheet_name: &str,
    start_col: u32,
    end_col: u32,
    region_end_row: u32,
) -> Result<AppendFooterScan> {
    let sheet = book
        .get_sheet_by_name(sheet_name).ok()
        .ok_or_else(|| invalid_argument(format!("sheet '{}' was not found", sheet_name)))?;

    let mut footer_row = None;
    let mut footer_detection = None;
    let mut footer_formula_targets = Vec::new();
    let mut footer_candidates = Vec::new();

    for row in [region_end_row, region_end_row + 1] {
        let reason = footer_reason_for_row(sheet, start_col, end_col, row);
        let matched = reason.is_some();
        if footer_row.is_none() && matched {
            footer_row = Some(row);
            footer_detection = reason.clone();
            footer_formula_targets = footer_formula_targets_for_row(sheet, start_col, end_col, row);
        }
        footer_candidates.push(AppendFooterCandidate {
            row,
            matched,
            reason,
        });
    }

    Ok(AppendFooterScan {
        footer_row,
        footer_detection,
        footer_candidates,
        footer_formula_targets,
    })
}

fn footer_formula_targets_for_row(
    sheet: &umya_spreadsheet::Worksheet,
    start_col: u32,
    end_col: u32,
    row: u32,
) -> Vec<String> {
    let mut addresses = Vec::new();
    for col in start_col..=end_col {
        let Some(cell) = sheet.get_cell((col, row)) else {
            continue;
        };
        if !cell.get_formula().trim().is_empty() {
            addresses.push(format!("{}{}", column_number_to_name(col), row));
        }
    }
    addresses
}

fn footer_reason_for_row(
    sheet: &umya_spreadsheet::Worksheet,
    start_col: u32,
    end_col: u32,
    row: u32,
) -> Option<String> {
    let mut saw_formula = false;
    let mut saw_non_formula_non_empty = false;
    let mut saw_footer_label = None;
    for col in start_col..=end_col {
        let Some(cell) = sheet.get_cell((col, row)) else {
            continue;
        };
        let value = cell.get_value().trim().to_string();
        let formula = cell.get_formula().trim().to_string();
        let has_formula = !formula.is_empty();
        if has_formula {
            saw_formula = true;
        } else if !value.is_empty() {
            if looks_like_footer_label(&value) {
                saw_footer_label = Some(value.clone());
            } else {
                saw_non_formula_non_empty = true;
            }
        }
    }

    if let Some(label) = saw_footer_label
        && saw_formula
    {
        return Some(format!(
            "footer keyword '{}' and formula on row {}",
            label, row
        ));
    }

    (saw_formula && !saw_non_formula_non_empty)
        .then(|| format!("formula-bearing summary row {}", row))
}

fn local_workbook_config(source: &Path) -> ServerConfig {
    let workspace_root = source
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    ServerConfig {
        workspace_root: workspace_root.clone(),
        screenshot_dir: workspace_root.join("screenshots"),
        path_mappings: Vec::new(),
        cache_capacity: 8,
        supported_extensions: vec![
            "xlsx".to_string(),
            "xlsm".to_string(),
            "xls".to_string(),
            "xlsb".to_string(),
        ],
        single_workbook: None,
        enabled_tools: None,
        transport: TransportKind::Http,
        http_bind_address: "127.0.0.1:8079".parse().expect("http bind address"),
        recalc_enabled: false,
        recalc_backend: RecalcBackendKind::Auto,
        vba_enabled: false,
        max_concurrent_recalcs: 2,
        tool_timeout_ms: Some(30_000),
        max_response_bytes: Some(1_000_000),
        output_profile: OutputProfile::TokenDense,
        max_payload_bytes: Some(65_536),
        max_cells: Some(10_000),
        max_items: Some(500),
        allow_overwrite: false,
        slim_surface: true,
    }
}

fn parse_append_region_bounds(raw: &str) -> Option<AppendBounds> {
    let (left, right) = raw.split_once(':').map_or((raw, raw), |(a, b)| (a, b));
    let (start_col, start_row) = parse_append_coord(left)?;
    let (end_col, end_row) = parse_append_coord(right)?;
    Some(AppendBounds {
        start_col: start_col.min(end_col),
        end_col: start_col.max(end_col),
        start_row: start_row.min(end_row),
        end_row: start_row.max(end_row),
    })
}

#[derive(Debug, Clone, Copy)]
struct AppendBounds {
    start_col: u32,
    end_col: u32,
    start_row: u32,
    end_row: u32,
}

fn parse_append_coord(raw: &str) -> Option<(u32, u32)> {
    let coord = raw.trim().trim_start_matches('$');
    if coord.is_empty() {
        return None;
    }

    let mut letters = String::new();
    let mut digits = String::new();
    for ch in coord.chars() {
        if ch == '$' {
            continue;
        }
        if ch.is_ascii_alphabetic() {
            if !digits.is_empty() {
                return None;
            }
            letters.push(ch.to_ascii_uppercase());
        } else if ch.is_ascii_digit() {
            digits.push(ch);
        } else {
            return None;
        }
    }

    if letters.is_empty() || digits.is_empty() {
        return None;
    }

    let mut col = 0u32;
    for ch in letters.bytes() {
        col = col
            .saturating_mul(26)
            .saturating_add((ch - b'A' + 1) as u32);
    }
    let row = digits.parse().ok()?;
    (col > 0 && row > 0).then_some((col, row))
}

fn column_number_to_name(mut col: u32) -> String {
    let mut chars = Vec::new();
    while col > 0 {
        let rem = ((col - 1) % 26) as u8;
        chars.push((b'A' + rem) as char);
        col = (col - 1) / 26;
    }
    chars.iter().rev().collect()
}

fn format_a1_range(start_col: u32, end_col: u32, start_row: u32, end_row: u32) -> String {
    let start = format!("{}{}", column_number_to_name(start_col), start_row);
    let end = format!("{}{}", column_number_to_name(end_col), end_row);
    if start == end {
        start
    } else {
        format!("{}:{}", start, end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn with_sheet<F>(configure: F) -> umya_spreadsheet::Workbook
    where
        F: FnOnce(&mut umya_spreadsheet::Worksheet),
    {
        let mut workbook = umya_spreadsheet::new_file();
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        configure(sheet);
        workbook
    }

    fn write_workbook_fixture<F>(name: &str, configure: F) -> (tempfile::TempDir, PathBuf)
    where
        F: FnOnce(&mut umya_spreadsheet::Worksheet),
    {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join(name);
        let workbook = with_sheet(configure);
        crate::xlsx_export::write(&workbook, &path).expect("write workbook");
        (tempdir, path)
    }

    fn seed_basic_region(sheet: &mut umya_spreadsheet::Worksheet) {
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Amount");
        sheet.get_cell_mut("A2").set_value("Alice");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("A3").set_value("Bob");
        sheet.get_cell_mut("B3").set_value_number(20.0);
    }

    fn set_formula(
        sheet: &mut umya_spreadsheet::Worksheet,
        address: &str,
        formula: &str,
        result: &str,
    ) {
        let cell = sheet.get_cell_mut(address);
        cell.set_formula(formula);
        cell.get_cell_value_mut().set_formula_result_default(result);
    }

    fn sample_append_rows() -> Vec<Vec<Option<MatrixCell>>> {
        vec![vec![
            Some(MatrixCell::Value(serde_json::json!("Cara"))),
            Some(MatrixCell::Value(serde_json::json!(30))),
        ]]
    }

    fn detect_primary_region_id(path: &Path, sheet_name: &str) -> u32 {
        let config = Arc::new(local_workbook_config(path));
        let workbook = WorkbookContext::load(&config, path).expect("load workbook");
        let entry = workbook
            .get_sheet_metrics(sheet_name)
            .expect("sheet metrics");
        entry
            .detected_regions()
            .into_iter()
            .find(|region| region.bounds.starts_with("A1:"))
            .or_else(|| entry.detected_regions().into_iter().next())
            .expect("detected region")
            .id
    }

    #[test]
    fn footer_detects_exact_total_keyword() {
        let workbook = with_sheet(|sheet| {
            sheet.get_cell_mut("A4").set_value("Total");
            set_formula(sheet, "B4", "SUM(B1:B3)", "100");
        });
        let sheet = workbook.get_sheet_by_name("Sheet1").ok().expect("sheet1");

        let reason = footer_reason_for_row(sheet, 1, 2, 4);
        assert!(
            reason
                .as_deref()
                .unwrap_or_default()
                .contains("footer keyword 'Total' and formula")
        );
    }

    #[test]
    fn footer_detects_grand_total_keyword() {
        let workbook = with_sheet(|sheet| {
            sheet.get_cell_mut("A4").set_value("Grand Total");
            set_formula(sheet, "B4", "SUM(B1:B3)", "100");
        });
        let sheet = workbook.get_sheet_by_name("Sheet1").ok().expect("sheet1");

        let reason = footer_reason_for_row(sheet, 1, 2, 4);
        assert!(
            reason
                .as_deref()
                .unwrap_or_default()
                .contains("footer keyword 'Grand Total' and formula")
        );
    }

    #[test]
    fn footer_detects_subtotal_keyword() {
        let workbook = with_sheet(|sheet| {
            sheet.get_cell_mut("A4").set_value("Subtotal");
            set_formula(sheet, "B4", "SUM(B1:B3)", "100");
        });
        let sheet = workbook.get_sheet_by_name("Sheet1").ok().expect("sheet1");

        let reason = footer_reason_for_row(sheet, 1, 2, 4);
        assert!(
            reason
                .as_deref()
                .unwrap_or_default()
                .contains("footer keyword 'Subtotal' and formula")
        );
    }

    #[test]
    fn footer_detects_footer_keyword() {
        let workbook = with_sheet(|sheet| {
            sheet.get_cell_mut("A4").set_value("Footer");
            set_formula(sheet, "B4", "SUM(B1:B3)", "100");
        });
        let sheet = workbook.get_sheet_by_name("Sheet1").ok().expect("sheet1");

        let reason = footer_reason_for_row(sheet, 1, 2, 4);
        assert!(
            reason
                .as_deref()
                .unwrap_or_default()
                .contains("footer keyword 'Footer' and formula")
        );
    }

    #[test]
    fn footer_detects_formula_summary_with_blank_label() {
        let workbook = with_sheet(|sheet| {
            set_formula(sheet, "B4", "SUM(B2:B3)", "30");
        });
        let sheet = workbook.get_sheet_by_name("Sheet1").ok().expect("sheet1");

        assert_eq!(
            footer_reason_for_row(sheet, 1, 2, 4).as_deref(),
            Some("formula-bearing summary row 4")
        );
    }

    #[test]
    fn footer_detects_sparse_late_column_formula_summary() {
        let workbook = with_sheet(|sheet| {
            set_formula(sheet, "D4", "SUM(D2:D3)", "30");
        });
        let sheet = workbook.get_sheet_by_name("Sheet1").ok().expect("sheet1");

        assert_eq!(
            footer_reason_for_row(sheet, 1, 4, 4).as_deref(),
            Some("formula-bearing summary row 4")
        );
    }

    #[test]
    fn footer_detection_trims_and_normalizes_case() {
        let workbook = with_sheet(|sheet| {
            sheet.get_cell_mut("A4").set_value("  ToTaL  ");
            set_formula(sheet, "B4", "SUM(B1:B3)", "100");
        });
        let sheet = workbook.get_sheet_by_name("Sheet1").ok().expect("sheet1");

        let reason = footer_reason_for_row(sheet, 1, 2, 4);
        assert!(
            reason
                .as_deref()
                .unwrap_or_default()
                .contains("footer keyword 'ToTaL' and formula")
        );
    }

    #[test]
    fn footer_ignores_non_footer_total_phrase_without_formula() {
        let workbook = with_sheet(|sheet| {
            sheet.get_cell_mut("A4").set_value("Total Revenue Plan");
        });
        let sheet = workbook.get_sheet_by_name("Sheet1").ok().expect("sheet1");

        assert!(footer_reason_for_row(sheet, 1, 2, 4).is_none());
    }

    #[test]
    fn footer_detects_starts_with_total_with_formula() {
        let workbook = with_sheet(|sheet| {
            sheet.get_cell_mut("A4").set_value("Total Revenue Plan");
            set_formula(sheet, "B4", "SUM(B1:B3)", "100");
        });
        let sheet = workbook.get_sheet_by_name("Sheet1").ok().expect("sheet1");

        let reason = footer_reason_for_row(sheet, 1, 2, 4);
        assert!(
            reason
                .as_deref()
                .unwrap_or_default()
                .contains("footer keyword 'Total Revenue Plan' and formula")
        );
    }

    #[test]
    fn footer_ignores_last_data_row_with_formula_and_label() {
        let workbook = with_sheet(|sheet| {
            sheet.get_cell_mut("A4").set_value("Alice");
            set_formula(sheet, "B4", "B2+B3", "30");
        });
        let sheet = workbook.get_sheet_by_name("Sheet1").ok().expect("sheet1");

        assert!(footer_reason_for_row(sheet, 1, 2, 4).is_none());
    }

    #[test]
    fn detect_append_footer_returns_none_when_no_footer_row_exists() {
        let (_tmp, path) = write_workbook_fixture("append-region-no-footer.xlsx", |sheet| {
            seed_basic_region(sheet);
        });

        let detection = detect_append_footer(&path, "Sheet1", 1, 2, 3).expect("detect footer");
        assert_eq!(detection.footer_row, None);
        assert_eq!(detection.footer_detection, None);
        assert!(detection.footer_formula_targets.is_empty());
        assert_eq!(detection.footer_candidates.len(), 2);
        assert!(!detection.footer_candidates[0].matched);
        assert!(!detection.footer_candidates[1].matched);
    }

    #[test]
    fn detect_append_footer_prefers_region_end_row_when_it_is_summary() {
        let (_tmp, path) = write_workbook_fixture("append-region-footer-at-end.xlsx", |sheet| {
            seed_basic_region(sheet);
            sheet.get_cell_mut("A4").set_value("Total");
            set_formula(sheet, "B4", "SUM(B2:B3)", "30");
        });

        let detection = detect_append_footer(&path, "Sheet1", 1, 2, 4).expect("detect footer");
        assert_eq!(detection.footer_row, Some(4));
        assert_eq!(
            detection.footer_detection.as_deref(),
            Some("footer keyword 'Total' and formula on row 4")
        );
        assert_eq!(detection.footer_formula_targets, vec!["B4"]);
        assert!(detection.footer_candidates[0].matched);
    }

    #[test]
    fn detect_append_footer_finds_summary_on_row_after_region_end() {
        let (_tmp, path) = write_workbook_fixture("append-region-footer-after-end.xlsx", |sheet| {
            seed_basic_region(sheet);
            sheet.get_cell_mut("A4").set_value("Total");
            set_formula(sheet, "B4", "SUM(B2:B3)", "30");
        });

        let detection = detect_append_footer(&path, "Sheet1", 1, 2, 3).expect("detect footer");
        assert_eq!(detection.footer_row, Some(4));
        assert_eq!(
            detection.footer_detection.as_deref(),
            Some("footer keyword 'Total' and formula on row 4")
        );
        assert!(!detection.footer_candidates[0].matched);
        assert!(detection.footer_candidates[1].matched);
    }

    #[test]
    fn build_append_region_plan_inserts_before_footer_and_sets_target_range() {
        let (_tmp, path) = write_workbook_fixture("append-region-plan-footer.xlsx", |sheet| {
            seed_basic_region(sheet);
            sheet.get_cell_mut("A4").set_value("Total");
            set_formula(sheet, "B4", "SUM(B2:B3)", "30");
        });
        let region_id = detect_primary_region_id(&path, "Sheet1");

        let plan = build_append_region_plan(
            &path,
            "Sheet1",
            Some(region_id),
            None,
            AppendFooterPolicy::Auto,
            sample_append_rows(),
        )
        .expect("build plan");
        assert_eq!(plan.target_kind, AppendRegionTargetKind::DetectedRegion);
        assert_eq!(plan.region_id, Some(region_id));
        assert_eq!(plan.footer_policy, "auto");
        assert_eq!(plan.footer_row, Some(4));
        assert_eq!(plan.insert_at_row, 4);
        assert_eq!(
            plan.insert_reason,
            "auto policy selected detected footer row 4"
        );
        assert_eq!(plan.footer_formula_targets, vec!["B4"]);
        assert_eq!(plan.target_anchor, "A4");
        assert_eq!(plan.target_range, "A4:B4");
        assert_eq!(plan.confidence, "high");
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn build_append_region_plan_warns_when_no_footer_is_detected() {
        let (_tmp, path) = write_workbook_fixture("append-region-plan-no-footer.xlsx", |sheet| {
            seed_basic_region(sheet);
        });
        let region_id = detect_primary_region_id(&path, "Sheet1");

        let plan = build_append_region_plan(
            &path,
            "Sheet1",
            Some(region_id),
            None,
            AppendFooterPolicy::Auto,
            sample_append_rows(),
        )
        .expect("build plan");
        assert_eq!(plan.footer_row, None);
        assert!(plan.insert_at_row >= 4);
        assert_eq!(plan.confidence, "low");
        assert!(
            plan.warnings
                .iter()
                .any(|warning| warning.contains("no footer row detected"))
        );
    }

    #[test]
    fn build_append_region_plan_does_not_treat_formula_data_row_as_footer() {
        let (_tmp, path) =
            write_workbook_fixture("append-region-plan-formula-data-row.xlsx", |sheet| {
                sheet.get_cell_mut("A1").set_value("Name");
                sheet.get_cell_mut("B1").set_value("Amount");
                sheet.get_cell_mut("A2").set_value("Alice");
                sheet.get_cell_mut("B2").set_value_number(10.0);
                sheet.get_cell_mut("A3").set_value("Bob");
                set_formula(sheet, "B3", "B2*2", "20");
            });
        let region_id = detect_primary_region_id(&path, "Sheet1");

        let plan = build_append_region_plan(
            &path,
            "Sheet1",
            Some(region_id),
            None,
            AppendFooterPolicy::Auto,
            sample_append_rows(),
        )
        .expect("build plan");
        assert_eq!(plan.footer_row, None);
        assert_eq!(plan.insert_at_row, 4);
    }

    #[test]
    fn build_append_region_plan_table_target_does_not_treat_formula_data_row_as_footer() {
        let (_tmp, path) =
            write_workbook_fixture("append-region-plan-table-formula-data-row.xlsx", |sheet| {
                sheet.get_cell_mut("A1").set_value("Name");
                sheet.get_cell_mut("B1").set_value("Amount");
                sheet.get_cell_mut("A2").set_value("Alice");
                sheet.get_cell_mut("B2").set_value_number(10.0);
                sheet.get_cell_mut("A3").set_value("Bob");
                set_formula(sheet, "B3", "B2*2", "20");
                let mut table = umya_spreadsheet::structs::Table::new("SalesTable", ("A1", "B3"));
                table.set_display_name("SalesTable");
                sheet.add_table(table);
            });

        let plan = build_append_region_plan(
            &path,
            "Sheet1",
            None,
            Some("SalesTable"),
            AppendFooterPolicy::Auto,
            sample_append_rows(),
        )
        .expect("build plan");
        assert_eq!(plan.footer_row, None);
        assert_eq!(plan.insert_at_row, 4);
    }

    #[test]
    fn build_append_region_plan_before_footer_fails_for_formula_data_row() {
        let (_tmp, path) = write_workbook_fixture(
            "append-region-plan-formula-data-row-before-footer.xlsx",
            |sheet| {
                sheet.get_cell_mut("A1").set_value("Name");
                sheet.get_cell_mut("B1").set_value("Amount");
                sheet.get_cell_mut("A2").set_value("Alice");
                sheet.get_cell_mut("B2").set_value_number(10.0);
                sheet.get_cell_mut("A3").set_value("Bob");
                set_formula(sheet, "B3", "B2*2", "20");
            },
        );
        let region_id = detect_primary_region_id(&path, "Sheet1");

        let error = build_append_region_plan(
            &path,
            "Sheet1",
            Some(region_id),
            None,
            AppendFooterPolicy::BeforeFooter,
            sample_append_rows(),
        )
        .expect_err("before-footer should fail for calculated data rows");
        assert!(
            error
                .to_string()
                .contains("requires a detected footer/subtotal row")
        );
    }

    #[test]
    fn build_append_region_plan_append_at_end_ignores_detected_footer() {
        let (_tmp, path) =
            write_workbook_fixture("append-region-plan-append-at-end.xlsx", |sheet| {
                seed_basic_region(sheet);
                sheet.get_cell_mut("A4").set_value("Total");
                set_formula(sheet, "B4", "SUM(B2:B3)", "30");
            });
        let region_id = detect_primary_region_id(&path, "Sheet1");

        let plan = build_append_region_plan(
            &path,
            "Sheet1",
            Some(region_id),
            None,
            AppendFooterPolicy::AppendAtEnd,
            sample_append_rows(),
        )
        .expect("build plan");
        assert_eq!(plan.footer_row, Some(4));
        assert_eq!(plan.insert_at_row, 5);
        assert!(
            plan.insert_reason
                .contains("append_at_end policy bypassed detected footer row 4")
        );
        assert!(
            plan.warnings
                .iter()
                .any(|warning| warning.contains("ignored detected footer row 4"))
        );
    }

    #[test]
    fn build_append_region_plan_before_footer_requires_detected_footer() {
        let (_tmp, path) =
            write_workbook_fixture("append-region-plan-before-footer.xlsx", |sheet| {
                seed_basic_region(sheet);
            });
        let region_id = detect_primary_region_id(&path, "Sheet1");

        let error = build_append_region_plan(
            &path,
            "Sheet1",
            Some(region_id),
            None,
            AppendFooterPolicy::BeforeFooter,
            sample_append_rows(),
        )
        .expect_err("before-footer should fail without a footer row");
        assert!(
            error
                .to_string()
                .contains("footer policy 'before-footer' requires a detected footer/subtotal row")
        );
    }

    #[test]
    fn build_append_region_plan_resolves_table_target() {
        let (_tmp, path) = write_workbook_fixture("append-region-plan-table.xlsx", |sheet| {
            sheet.get_cell_mut("A1").set_value("Name");
            sheet.get_cell_mut("B1").set_value("Amount");
            sheet.get_cell_mut("A2").set_value("Alice");
            sheet.get_cell_mut("B2").set_value_number(10.0);
            sheet.get_cell_mut("A3").set_value("Bob");
            sheet.get_cell_mut("B3").set_value_number(20.0);
            let mut table = umya_spreadsheet::structs::Table::new("SalesTable", ("A1", "B3"));
            table.set_display_name("SalesTable");
            sheet.add_table(table);
        });

        let plan = build_append_region_plan(
            &path,
            "Sheet1",
            None,
            Some("SalesTable"),
            AppendFooterPolicy::Auto,
            sample_append_rows(),
        )
        .expect("build plan");
        assert_eq!(plan.target_kind, AppendRegionTargetKind::Table);
        assert_eq!(plan.table_name.as_deref(), Some("SalesTable"));
        assert_eq!(plan.header_row, Some(1));
        assert_eq!(plan.region_bounds, "A1:B3");
    }

    #[test]
    fn build_append_region_plan_rejects_payload_wider_than_region() {
        let (_tmp, path) = write_workbook_fixture("append-region-plan-too-wide.xlsx", |sheet| {
            seed_basic_region(sheet);
        });
        let region_id = detect_primary_region_id(&path, "Sheet1");
        let rows = vec![vec![
            Some(MatrixCell::Value(serde_json::json!("Cara"))),
            Some(MatrixCell::Value(serde_json::json!(30))),
            Some(MatrixCell::Value(serde_json::json!("extra"))),
        ]];

        let error = build_append_region_plan(
            &path,
            "Sheet1",
            Some(region_id),
            None,
            AppendFooterPolicy::Auto,
            rows,
        )
        .expect_err("payload wider than region should fail");
        assert!(
            error
                .to_string()
                .contains("rows payload is wider than region 0")
                || error
                    .to_string()
                    .contains("rows payload is wider than region ")
        );
    }

    #[test]
    fn build_append_region_plan_rejects_zero_column_payload() {
        let (_tmp, path) =
            write_workbook_fixture("append-region-plan-empty-columns.xlsx", |sheet| {
                seed_basic_region(sheet);
            });
        let region_id = detect_primary_region_id(&path, "Sheet1");

        let error = build_append_region_plan(
            &path,
            "Sheet1",
            Some(region_id),
            None,
            AppendFooterPolicy::Auto,
            vec![Vec::new()],
        )
        .expect_err("zero-column payload should fail");
        assert!(
            error
                .to_string()
                .contains("append-region rows payload must contain at least one non-empty column")
        );
    }

    #[test]
    fn build_clone_template_row_plan_reports_targets_and_adjacent_sum_candidates() {
        let (_tmp, path) = write_workbook_fixture("clone-template-row-plan.xlsx", |sheet| {
            sheet.get_cell_mut("A1").set_value("Item");
            sheet.get_cell_mut("B1").set_value("Input");
            sheet.get_cell_mut("C1").set_value("Calc");
            sheet.get_cell_mut("A2").set_value("Alpha");
            sheet.get_cell_mut("B2").set_value_number(10.0);
            set_formula(sheet, "C2", "B2*2", "20");
            sheet.get_cell_mut("A3").set_value("Total");
            set_formula(sheet, "C3", "SUM(C2:C2)", "20");
        });

        let plan = build_clone_template_row_plan(
            &path,
            "Sheet1",
            2,
            None,
            Some(2),
            None,
            2,
            true,
            ClonePatchTargets::LikelyInputs,
            CloneMergePolicy::Safe,
        )
        .expect("build plan");
        assert_eq!(plan.anchor_kind, CloneAnchorKind::After);
        assert_eq!(plan.insert_at_row, 3);
        assert_eq!(plan.inserted_row_range, "3:4");
        assert_eq!(plan.formula_targets, vec!["C3", "C4"]);
        assert_eq!(plan.likely_patch_targets, vec!["B3", "B4"]);
        assert_eq!(plan.adjacent_sum_targets, vec!["C5"]);
        assert_eq!(plan.confidence, "high");
    }

    #[test]
    fn build_clone_template_row_plan_strict_merge_policy_fails_for_crossing_merge() {
        let (_tmp, path) =
            write_workbook_fixture("clone-template-row-strict-merge.xlsx", |sheet| {
                sheet.get_cell_mut("A1").set_value("Header");
                sheet.get_cell_mut("A2").set_value("Alpha");
                sheet.get_cell_mut("B2").set_value_number(10.0);
                sheet.add_merge_cells("A1:A2");
            });

        let error = build_clone_template_row_plan(
            &path,
            "Sheet1",
            2,
            Some(3),
            None,
            None,
            1,
            false,
            ClonePatchTargets::LikelyInputs,
            CloneMergePolicy::Strict,
        )
        .expect_err("strict merge policy should fail");
        assert!(error.to_string().contains("unsafe clone template"));
    }

    #[test]
    fn apply_clone_template_row_plan_preserves_horizontal_merges_and_row_validations() {
        let (_tmp, path) = write_workbook_fixture("clone-template-row-apply.xlsx", |sheet| {
            sheet.get_cell_mut("A1").set_value("Name");
            sheet.get_cell_mut("B1").set_value("Input");
            sheet.get_cell_mut("C1").set_value("Calc");
            sheet.get_cell_mut("A2").set_value("Alpha");
            sheet.get_cell_mut("B2").set_value_number(10.0);
            set_formula(sheet, "C2", "B2*2", "20");
            sheet.add_merge_cells("A2:B2");
            let mut row_style = umya_spreadsheet::Style::default();
            row_style.get_font_mut().set_bold(true);
            sheet
                .get_row_dimension_mut(&2)
                .set_height(27.5)
                .set_custom_height(true)
                .set_hidden(true)
                .set_descent(0.25)
                .set_thick_bot(true)
                .set_style(row_style);

            let mut dv = umya_spreadsheet::structs::DataValidation::default();
            dv.set_type(umya_spreadsheet::structs::DataValidationValues::List);
            dv.get_sequence_of_references_mut().set_sqref("B2 B2 C2:D2");
            dv.set_formula1("\"A,B,C\"");
            sheet.set_data_validations(umya_spreadsheet::structs::DataValidations::default());
            sheet
                .get_data_validations_mut()
                .unwrap()
                .add_data_validation_list(dv);
        });

        let plan = build_clone_template_row_plan(
            &path,
            "Sheet1",
            2,
            Some(3),
            None,
            None,
            2,
            false,
            ClonePatchTargets::AllNonFormula,
            CloneMergePolicy::Safe,
        )
        .expect("build plan");
        apply_clone_template_row_plan_to_file(&path, &plan).expect("apply plan");

        let book = crate::xlsx_import::read(&path).expect("read workbook");
        let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet1");
        assert_eq!(sheet.get_cell("A3").expect("A3").get_value(), "Alpha");
        assert_eq!(sheet.get_cell("B4").expect("B4").get_value(), "10");
        let merge_ranges: Vec<String> = sheet
            .get_merge_cells()
            .iter()
            .map(|range| range.get_range())
            .collect();
        assert!(merge_ranges.contains(&"A3:B3".to_string()));
        assert!(merge_ranges.contains(&"A4:B4".to_string()));
        let source_dimension = sheet.get_row_dimension(&2).expect("source row dimension");
        for row_number in [3, 4] {
            let cloned = sheet
                .get_row_dimension(&row_number)
                .expect("cloned row dimension");
            assert_eq!(cloned.get_height(), source_dimension.get_height());
            assert_eq!(
                cloned.get_custom_height(),
                source_dimension.get_custom_height()
            );
            assert_eq!(cloned.get_hidden(), source_dimension.get_hidden());
            assert_eq!(cloned.get_descent(), source_dimension.get_descent());
            assert_eq!(cloned.get_thick_bot(), source_dimension.get_thick_bot());
            assert_eq!(cloned.get_style(), source_dimension.get_style());
        }
        let validations = sheet.get_data_validations().expect("validations");
        let sqrefs: Vec<String> = validations
            .get_data_validation_list()
            .iter()
            .map(|dv| dv.get_sequence_of_references().get_sqref())
            .collect();
        assert_eq!(sqrefs, vec!["B2 C2:D2", "B3", "C3:D3", "B4", "C4:D4"]);
    }

    #[test]
    fn clone_template_row_before_source_relocates_original_and_adds_only_destination_sqrefs() {
        let (_tmp, path) =
            write_workbook_fixture("clone-row-before-source-validation.xlsx", |sheet| {
                sheet.get_cell_mut("A4").set_value("template");
                let mut validation = umya_spreadsheet::structs::DataValidation::default();
                validation
                    .get_sequence_of_references_mut()
                    .set_sqref("B4 C4:D4");
                sheet.set_data_validations(umya_spreadsheet::structs::DataValidations::default());
                sheet
                    .get_data_validations_mut()
                    .expect("validations")
                    .add_data_validation_list(validation);
            });

        let plan = build_clone_template_row_plan(
            &path,
            "Sheet1",
            4,
            Some(2),
            None,
            None,
            1,
            false,
            ClonePatchTargets::None,
            CloneMergePolicy::Safe,
        )
        .expect("build plan");
        apply_clone_template_row_plan_to_file(&path, &plan).expect("apply plan");

        let workbook = crate::xlsx_import::read(&path).expect("read workbook");
        let sheet = workbook.get_sheet_by_name("Sheet1").ok().expect("sheet1");
        let sqrefs = sheet
            .get_data_validations()
            .expect("validations")
            .get_data_validation_list()
            .iter()
            .map(|validation| validation.get_sequence_of_references().get_sqref())
            .collect::<Vec<_>>();
        assert_eq!(sqrefs, vec!["B5 C5:D5", "B2", "C2:D2"]);
    }

    #[test]
    fn build_clone_row_band_plan_reports_inserted_blocks_and_targets() {
        let (_tmp, path) = write_workbook_fixture("clone-row-band-plan.xlsx", |sheet| {
            sheet.get_cell_mut("A1").set_value("Item");
            sheet.get_cell_mut("B1").set_value("Input");
            sheet.get_cell_mut("C1").set_value("Calc");
            sheet.get_cell_mut("A2").set_value("Alpha");
            sheet.get_cell_mut("B2").set_value_number(10.0);
            set_formula(sheet, "C2", "B2*2", "20");
            sheet.get_cell_mut("A3").set_value("Beta");
            sheet.get_cell_mut("B3").set_value_number(20.0);
            set_formula(sheet, "C3", "B3*2", "40");
            sheet.get_cell_mut("A4").set_value("Total");
            set_formula(sheet, "C4", "SUM(C2:C3)", "60");
        });

        let plan = build_clone_row_band_plan(
            &path,
            "Sheet1",
            "2:3",
            None,
            Some(3),
            None,
            2,
            true,
            ClonePatchTargets::LikelyInputs,
            CloneMergePolicy::Safe,
        )
        .expect("build plan");
        assert_eq!(plan.helper_kind, CloneHelperKind::CloneRowBand);
        assert_eq!(plan.source_row_count, 2);
        assert_eq!(plan.rows_inserted, 4);
        assert_eq!(plan.inserted_row_range, "4:7");
        assert_eq!(plan.inserted_blocks.len(), 2);
        assert_eq!(plan.inserted_blocks[0].row_range, "4:5");
        assert_eq!(plan.inserted_blocks[1].row_range, "6:7");
        assert_eq!(plan.formula_targets, vec!["C4", "C5", "C6", "C7"]);
        assert_eq!(plan.likely_patch_targets, vec!["B4", "B5", "B6", "B7"]);
        assert_eq!(plan.adjacent_sum_targets, vec!["C8"]);
    }

    #[test]
    fn build_clone_row_band_plan_strict_merge_policy_fails_for_crossing_merge() {
        let (_tmp, path) = write_workbook_fixture("clone-row-band-strict-merge.xlsx", |sheet| {
            sheet.get_cell_mut("A1").set_value("Header");
            sheet.get_cell_mut("A2").set_value("Alpha");
            sheet.get_cell_mut("A3").set_value("Beta");
            sheet.add_merge_cells("A1:A2");
        });

        let error = build_clone_row_band_plan(
            &path,
            "Sheet1",
            "2:3",
            Some(4),
            None,
            None,
            1,
            false,
            ClonePatchTargets::LikelyInputs,
            CloneMergePolicy::Strict,
        )
        .expect_err("strict merge policy should fail");
        assert!(error.to_string().contains("unsafe clone template"));
    }

    #[test]
    fn apply_clone_row_band_plan_preserves_contained_merges_validations_and_row_heights() {
        let (_tmp, path) = write_workbook_fixture("clone-row-band-apply.xlsx", |sheet| {
            sheet.get_cell_mut("A1").set_value("Name");
            sheet.get_cell_mut("B1").set_value("Input");
            sheet.get_cell_mut("C1").set_value("Calc");
            sheet.get_cell_mut("A2").set_value("Alpha");
            sheet.get_cell_mut("B2").set_value_number(10.0);
            set_formula(sheet, "C2", "B2*2", "20");
            sheet.get_cell_mut("A3").set_value("Beta");
            sheet.get_cell_mut("B3").set_value_number(20.0);
            set_formula(sheet, "C3", "B3*2", "40");
            sheet.add_merge_cells("A2:A3");
            sheet
                .get_row_dimension_mut(&2)
                .set_height(28.0)
                .set_custom_height(true);
            sheet
                .get_row_dimension_mut(&3)
                .set_height(32.0)
                .set_custom_height(true);

            let mut dv = umya_spreadsheet::structs::DataValidation::default();
            dv.set_type(umya_spreadsheet::structs::DataValidationValues::List);
            dv.get_sequence_of_references_mut().set_sqref("B2:B3");
            dv.set_formula1("\"A,B,C\"");
            sheet.set_data_validations(umya_spreadsheet::structs::DataValidations::default());
            sheet
                .get_data_validations_mut()
                .unwrap()
                .add_data_validation_list(dv);
        });

        let plan = build_clone_row_band_plan(
            &path,
            "Sheet1",
            "2:3",
            Some(4),
            None,
            None,
            2,
            false,
            ClonePatchTargets::AllNonFormula,
            CloneMergePolicy::Safe,
        )
        .expect("build plan");
        apply_clone_row_band_plan_to_file(&path, &plan).expect("apply plan");

        let book = crate::xlsx_import::read(&path).expect("read workbook");
        let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet1");
        assert_eq!(sheet.get_cell("A4").expect("A4").get_value(), "Alpha");
        assert_eq!(sheet.get_cell("A5").expect("A5").get_value(), "Beta");
        assert_eq!(
            sheet
                .get_cell("C4")
                .expect("C4")
                .get_formula()
                .replace(' ', ""),
            "B4*2"
        );
        assert_eq!(
            sheet
                .get_cell("C7")
                .expect("C7")
                .get_formula()
                .replace(' ', ""),
            "B7*2"
        );
        let merge_ranges: Vec<String> = sheet
            .get_merge_cells()
            .iter()
            .map(|range| range.get_range())
            .collect();
        assert!(merge_ranges.contains(&"A4:A5".to_string()));
        assert!(merge_ranges.contains(&"A6:A7".to_string()));
        assert_eq!(
            sheet.get_row_dimension(&4).map(|row| row.get_height()),
            Some(28.0)
        );
        assert_eq!(
            sheet.get_row_dimension(&5).map(|row| row.get_height()),
            Some(32.0)
        );
        let validations = sheet.get_data_validations().expect("validations");
        let sqrefs: Vec<String> = validations
            .get_data_validation_list()
            .iter()
            .map(|dv| dv.get_sequence_of_references().get_sqref())
            .collect();
        assert!(
            sqrefs
                .iter()
                .any(|sqref| sqref.contains("B4") && sqref.contains("B5"))
        );
        assert!(
            sqrefs
                .iter()
                .any(|sqref| sqref.contains("B6") && sqref.contains("B7"))
        );
    }

    #[test]
    fn apply_clone_row_band_shifts_formulas_correctly_for_internal_external_and_absolute_refs() {
        let (_tmp, path) = write_workbook_fixture("clone-row-band-formulas.xlsx", |sheet| {
            sheet.get_cell_mut("A1").set_value("Rate");
            sheet.get_cell_mut("Z1").set_value_number(0.05);

            // Row 2
            set_formula(sheet, "A2", "A1", ""); // External relative
            set_formula(sheet, "B2", "$Z$1", ""); // Absolute
            sheet.get_cell_mut("C2").set_value_number(100.0);

            // Row 3
            set_formula(sheet, "A3", "A2", ""); // Internal relative
            set_formula(sheet, "B3", "$Z$1", ""); // Absolute
            set_formula(sheet, "D3", "SUM(C2:C3)", ""); // Internal range
        });

        let plan = build_clone_row_band_plan(
            &path,
            "Sheet1",
            "2:3",
            None,
            Some(3), // after row 3 -> inserts at row 4
            None,
            1, // repeat 1 time -> range 4:5
            false,
            ClonePatchTargets::None,
            CloneMergePolicy::Safe,
        )
        .expect("build plan");

        apply_clone_row_band_plan_to_file(&path, &plan).expect("apply plan");

        let book = crate::xlsx_import::read(&path).expect("read workbook");
        let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet1");

        // Row 4 (cloned from Row 2, offset +2)
        assert_eq!(
            sheet
                .get_cell("A4")
                .expect("A4")
                .get_formula()
                .replace(' ', ""),
            "A3"
        );
        assert_eq!(
            sheet
                .get_cell("B4")
                .expect("B4")
                .get_formula()
                .replace(' ', ""),
            "$Z$1"
        );

        // Row 5 (cloned from Row 3, offset +2)
        assert_eq!(
            sheet
                .get_cell("A5")
                .expect("A5")
                .get_formula()
                .replace(' ', ""),
            "A4"
        );
        assert_eq!(
            sheet
                .get_cell("B5")
                .expect("B5")
                .get_formula()
                .replace(' ', ""),
            "$Z$1"
        );
        assert_eq!(
            sheet
                .get_cell("D5")
                .expect("D5")
                .get_formula()
                .replace(' ', ""),
            "SUM(C4:C5)"
        );
    }

    #[test]
    fn apply_clone_row_band_safe_policy_drops_crossing_merges_but_keeps_contained_merges() {
        let (_tmp, path) = write_workbook_fixture("clone-row-band-safe-merges.xlsx", |sheet| {
            sheet.get_cell_mut("A1").set_value("A1");
            sheet.get_cell_mut("A2").set_value("A2");
            sheet.get_cell_mut("A3").set_value("A3");
            sheet.get_cell_mut("A4").set_value("A4");

            sheet.add_merge_cells("A2:A3"); // Fully contained in 2:3
            sheet.add_merge_cells("B3:B4"); // Crossing bottom boundary
            sheet.add_merge_cells("C1:C2"); // Crossing top boundary
        });

        let plan = build_clone_row_band_plan(
            &path,
            "Sheet1",
            "2:3",
            None,
            Some(3), // after row 3 -> inserts at row 4
            None,
            1, // repeat 1 time -> range 4:5
            false,
            ClonePatchTargets::None,
            CloneMergePolicy::Safe,
        )
        .expect("build plan");

        apply_clone_row_band_plan_to_file(&path, &plan).expect("apply plan");

        let book = crate::xlsx_import::read(&path).expect("read workbook");
        let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet1");

        let merge_ranges: Vec<String> = sheet
            .get_merge_cells()
            .iter()
            .map(|range| range.get_range())
            .collect();

        // Original merges should still exist (or be properly expanded)
        assert!(merge_ranges.contains(&"A2:A3".to_string()));
        assert!(merge_ranges.contains(&"C1:C2".to_string()));
        // B3:B4 crosses the insertion boundary at row 4, so inserting 2 rows expands it to B3:B6
        assert!(merge_ranges.contains(&"B3:B6".to_string()));

        // Cloned fully contained merge should exist
        assert!(merge_ranges.contains(&"A4:A5".to_string()));

        // Cloned crossing merges should NOT exist
        // shifted B3:B4 (+2 rows) = B5:B6 (does not exist, though B3:B6 covers the area)
        // shifted C1:C2 (+2 rows) = C3:C4
        assert!(!merge_ranges.contains(&"C3:C4".to_string()));
    }

    #[test]
    fn build_clone_template_row_plan_identifies_likely_inputs_correctly() {
        let (_tmp, path) = write_workbook_fixture("clone-likely-inputs.xlsx", |sheet| {
            sheet.get_cell_mut("A1").set_value("Expense"); // String label, skip
            sheet.get_cell_mut("B1").set_value_number(150.0); // Numeric, include
            // C1 is completely empty, no validation
            sheet.get_cell_mut("D1").set_value_number(200.0); // Numeric, include
            set_formula(sheet, "E1", "B1+D1", "350"); // Formula, skip

            // F1 is a string but has data validation, include
            sheet.get_cell_mut("F1").set_value("Select...");
            let mut dv = umya_spreadsheet::structs::DataValidation::default();
            dv.set_type(umya_spreadsheet::structs::DataValidationValues::List);
            dv.get_sequence_of_references_mut().set_sqref("F1:F1");
            dv.set_formula1("\"A,B,C\"");
            sheet.set_data_validations(umya_spreadsheet::structs::DataValidations::default());
            sheet
                .get_data_validations_mut()
                .unwrap()
                .add_data_validation_list(dv);

            // G1 is completely empty but has data validation, include
            let mut dv2 = umya_spreadsheet::structs::DataValidation::default();
            dv2.set_type(umya_spreadsheet::structs::DataValidationValues::List);
            dv2.get_sequence_of_references_mut().set_sqref("G1:G1");
            dv2.set_formula1("\"X,Y,Z\"");
            sheet
                .get_data_validations_mut()
                .unwrap()
                .add_data_validation_list(dv2);
        });

        let plan = build_clone_template_row_plan(
            &path,
            "Sheet1",
            1,
            None,
            Some(1),
            None,
            1,
            false,
            ClonePatchTargets::LikelyInputs,
            CloneMergePolicy::Safe,
        )
        .expect("build plan");

        let expected_targets = vec!["B2", "D2", "F2", "G2"];
        assert_eq!(plan.likely_patch_targets, expected_targets);
    }
}
