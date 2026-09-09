use crate::fork::{ChangeSummary, StagedChange, StagedOp};
use crate::model::WorkbookId;
use crate::state::AppState;
use crate::tools::param_enums::{BatchMode, PageOrientation};
use crate::utils::make_short_random_id;
use anyhow::{Result, anyhow, bail};
use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use umya_spreadsheet::{
    Break, Coordinate, Pane, PaneStateValues, PaneValues, Selection, SheetView, SheetViews,
    Worksheet,
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetLayoutBatchParams {
    pub fork_id: String,
    pub ops: Vec<SheetLayoutOp>,
    #[serde(default)]
    pub mode: Option<BatchMode>, // preview|apply (default apply)
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SheetLayoutOp {
    FreezePanes {
        sheet_name: String,
        #[serde(default)]
        freeze_rows: u32,
        #[serde(default)]
        freeze_cols: u32,
        #[serde(default)]
        top_left_cell: Option<String>,
    },
    SetZoom {
        sheet_name: String,
        zoom_percent: u32,
    },
    SetGridlines {
        sheet_name: String,
        show: bool,
    },
    SetPageMargins {
        sheet_name: String,
        left: f64,
        right: f64,
        top: f64,
        bottom: f64,
        #[serde(default)]
        header: Option<f64>,
        #[serde(default)]
        footer: Option<f64>,
    },
    SetPageSetup {
        sheet_name: String,
        orientation: PageOrientation,
        #[serde(default)]
        fit_to_width: Option<u32>,
        #[serde(default)]
        fit_to_height: Option<u32>,
        #[serde(default)]
        scale_percent: Option<u32>,
    },
    SetPrintArea {
        sheet_name: String,
        range: String,
    },
    SetPageBreaks {
        sheet_name: String,
        #[serde(default)]
        row_breaks: Vec<u32>,
        #[serde(default)]
        col_breaks: Vec<u32>,
    },
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SheetLayoutBatchResponse {
    pub fork_id: String,
    pub mode: String,
    pub change_id: Option<String>,
    pub ops_applied: usize,
    pub summary: ChangeSummary,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SheetLayoutBatchStagedPayload {
    pub(crate) ops: Vec<SheetLayoutOp>,
}

pub async fn sheet_layout_batch(
    state: Arc<AppState>,
    params: SheetLayoutBatchParams,
) -> Result<SheetLayoutBatchResponse> {
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;

    let fork_ctx = registry.get_fork(&params.fork_id)?;
    let work_path = fork_ctx.work_path.clone();

    // Validate sheet existence up-front.
    let fork_workbook_id = WorkbookId(params.fork_id.clone());
    let workbook = state.open_workbook(&fork_workbook_id).await?;
    {
        let mut seen = BTreeSet::new();
        for op in &params.ops {
            let sheet_name = op_sheet_name(op);
            if seen.insert(sheet_name.to_string()) {
                let _ = workbook.with_sheet(sheet_name, |_| Ok::<_, anyhow::Error>(()))?;
            }
        }
    }

    let mode = params.mode.unwrap_or_default();

    if mode.is_preview() {
        let change_id = make_short_random_id("chg", 12);
        let snapshot_path = stage_snapshot_path(&params.fork_id, &change_id);
        fs::create_dir_all(snapshot_path.parent().unwrap())?;
        fs::copy(&work_path, &snapshot_path)?;

        let snapshot_for_apply = snapshot_path.clone();
        let ops_for_apply = params.ops.clone();
        let apply_result = tokio::task::spawn_blocking(move || {
            apply_sheet_layout_ops_to_file(&snapshot_for_apply, &ops_for_apply)
        })
        .await??;

        let mut summary = apply_result.summary;
        summary.op_kinds = vec!["sheet_layout_batch".to_string()];
        set_recalc_needed_flag(&mut summary, fork_ctx.recalc_needed);

        let staged_op = StagedOp {
            kind: "sheet_layout_batch".to_string(),
            payload: serde_json::to_value(SheetLayoutBatchStagedPayload {
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

        Ok(SheetLayoutBatchResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: Some(change_id),
            ops_applied: apply_result.ops_applied,
            summary,
        })
    } else {
        let work_path_for_apply = work_path.clone();
        let ops_for_apply = params.ops.clone();
        let apply_result = tokio::task::spawn_blocking(move || {
            apply_sheet_layout_ops_to_file(&work_path_for_apply, &ops_for_apply)
        })
        .await??;

        let mut summary = apply_result.summary;
        summary.op_kinds = vec!["sheet_layout_batch".to_string()];
        set_recalc_needed_flag(&mut summary, fork_ctx.recalc_needed);

        let _ = state.close_workbook(&fork_workbook_id);

        Ok(SheetLayoutBatchResponse {
            fork_id: params.fork_id,
            mode: mode.as_str().to_string(),
            change_id: None,
            ops_applied: apply_result.ops_applied,
            summary,
        })
    }
}

fn op_sheet_name(op: &SheetLayoutOp) -> &str {
    match op {
        SheetLayoutOp::FreezePanes { sheet_name, .. }
        | SheetLayoutOp::SetZoom { sheet_name, .. }
        | SheetLayoutOp::SetGridlines { sheet_name, .. }
        | SheetLayoutOp::SetPageMargins { sheet_name, .. }
        | SheetLayoutOp::SetPageSetup { sheet_name, .. }
        | SheetLayoutOp::SetPrintArea { sheet_name, .. }
        | SheetLayoutOp::SetPageBreaks { sheet_name, .. } => sheet_name,
    }
}

fn stage_snapshot_path(fork_id: &str, change_id: &str) -> PathBuf {
    PathBuf::from("/tmp/mcp-staged").join(format!("{fork_id}_{change_id}.xlsx"))
}

fn set_recalc_needed_flag(summary: &mut ChangeSummary, recalc_needed: bool) {
    summary
        .flags
        .insert("recalc_needed".to_string(), recalc_needed);
}

pub(crate) struct SheetLayoutApplyResult {
    pub(crate) ops_applied: usize,
    pub(crate) summary: ChangeSummary,
}

pub(crate) fn apply_sheet_layout_ops_to_file(
    path: &Path,
    ops: &[SheetLayoutOp],
) -> Result<SheetLayoutApplyResult> {
    let mut book = crate::xlsx_import::read(path)?;
    let result = apply_sheet_layout_ops_to_workbook(&mut book, ops)?;
    crate::xlsx_export::write(&book, path)?;
    Ok(result)
}

pub(crate) fn apply_sheet_layout_ops_to_workbook(
    book: &mut umya_spreadsheet::Workbook,
    ops: &[SheetLayoutOp],
) -> Result<SheetLayoutApplyResult> {
    let mut affected_sheets: BTreeSet<String> = BTreeSet::new();
    let mut affected_bounds: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();

    let mut freeze_ops: u64 = 0;
    let mut zoom_ops: u64 = 0;
    let mut grid_ops: u64 = 0;
    let mut margin_ops: u64 = 0;
    let mut setup_ops: u64 = 0;
    let mut print_area_ops: u64 = 0;
    let mut page_break_ops: u64 = 0;

    for op in ops {
        match op {
            SheetLayoutOp::FreezePanes {
                sheet_name,
                freeze_rows,
                freeze_cols,
                top_left_cell,
            } => {
                freeze_ops += 1;
                affected_sheets.insert(sheet_name.clone());
                let sheet = book
                    .sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;

                apply_freeze_panes(
                    sheet,
                    *freeze_rows,
                    *freeze_cols,
                    top_left_cell.as_deref(),
                    &mut warnings,
                )?;
            }
            SheetLayoutOp::SetZoom {
                sheet_name,
                zoom_percent,
            } => {
                zoom_ops += 1;
                affected_sheets.insert(sheet_name.clone());
                if *zoom_percent < 10 || *zoom_percent > 400 {
                    bail!("zoom_percent must be between 10 and 400");
                }
                let sheet = book
                    .sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                let view = primary_sheet_view_mut(sheet);
                view.set_zoom_scale(*zoom_percent);
                view.set_zoom_scale_normal(*zoom_percent);
            }
            SheetLayoutOp::SetGridlines { sheet_name, show } => {
                grid_ops += 1;
                affected_sheets.insert(sheet_name.clone());
                let sheet = book
                    .sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                let view = primary_sheet_view_mut(sheet);
                view.set_show_grid_lines(*show);
            }
            SheetLayoutOp::SetPageMargins {
                sheet_name,
                left,
                right,
                top,
                bottom,
                header,
                footer,
            } => {
                margin_ops += 1;
                affected_sheets.insert(sheet_name.clone());
                validate_margin_value("left", *left)?;
                validate_margin_value("right", *right)?;
                validate_margin_value("top", *top)?;
                validate_margin_value("bottom", *bottom)?;
                if let Some(h) = header {
                    validate_margin_value("header", *h)?;
                }
                if let Some(f) = footer {
                    validate_margin_value("footer", *f)?;
                }
                let sheet = book
                    .sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                let margins = sheet.page_margins_mut();
                margins.set_left(*left);
                margins.set_right(*right);
                margins.set_top(*top);
                margins.set_bottom(*bottom);
                if let Some(h) = header {
                    margins.set_header(*h);
                }
                if let Some(f) = footer {
                    margins.set_footer(*f);
                }
            }
            SheetLayoutOp::SetPageSetup {
                sheet_name,
                orientation,
                fit_to_width,
                fit_to_height,
                scale_percent,
            } => {
                setup_ops += 1;
                affected_sheets.insert(sheet_name.clone());
                let orientation_value = orientation.to_umya();
                if let Some(v) = fit_to_width
                    && *v < 1
                {
                    bail!("fit_to_width must be >= 1");
                }
                if let Some(v) = fit_to_height
                    && *v < 1
                {
                    bail!("fit_to_height must be >= 1");
                }
                if let Some(v) = scale_percent
                    && (*v < 10 || *v > 400)
                {
                    bail!("scale_percent must be between 10 and 400");
                }

                let sheet = book
                    .sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                let setup = sheet.page_setup_mut();
                setup.set_orientation(orientation_value);
                if let Some(v) = fit_to_width {
                    setup.set_fit_to_width(*v);
                }
                if let Some(v) = fit_to_height {
                    setup.set_fit_to_height(*v);
                }
                if let Some(v) = scale_percent {
                    setup.set_scale(*v);
                }
            }
            SheetLayoutOp::SetPrintArea { sheet_name, range } => {
                print_area_ops += 1;
                affected_sheets.insert(sheet_name.clone());
                affected_bounds.push(range.clone());
                set_print_area_defined_name(book, sheet_name, range)?;
            }
            SheetLayoutOp::SetPageBreaks {
                sheet_name,
                row_breaks,
                col_breaks,
            } => {
                page_break_ops += 1;
                affected_sheets.insert(sheet_name.clone());
                for b in row_breaks {
                    if *b < 1 {
                        bail!("row_breaks entries must be >= 1");
                    }
                }
                for b in col_breaks {
                    if *b < 1 {
                        bail!("col_breaks entries must be >= 1");
                    }
                }
                let sheet = book
                    .sheet_by_name_mut(sheet_name).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                apply_page_breaks(sheet, row_breaks, col_breaks);
            }
        }
    }

    counts.insert("ops".to_string(), ops.len() as u64);
    if freeze_ops > 0 {
        counts.insert("freeze_panes_ops".to_string(), freeze_ops);
    }
    if zoom_ops > 0 {
        counts.insert("set_zoom_ops".to_string(), zoom_ops);
    }
    if grid_ops > 0 {
        counts.insert("set_gridlines_ops".to_string(), grid_ops);
    }
    if margin_ops > 0 {
        counts.insert("set_page_margins_ops".to_string(), margin_ops);
    }
    if setup_ops > 0 {
        counts.insert("set_page_setup_ops".to_string(), setup_ops);
    }
    if print_area_ops > 0 {
        counts.insert("set_print_area_ops".to_string(), print_area_ops);
    }
    if page_break_ops > 0 {
        counts.insert("set_page_breaks_ops".to_string(), page_break_ops);
    }

    let summary = ChangeSummary {
        op_kinds: vec!["sheet_layout_batch".to_string()],
        affected_sheets: affected_sheets.into_iter().collect(),
        affected_bounds,
        counts,
        warnings,
        ..Default::default()
    };

    Ok(SheetLayoutApplyResult {
        ops_applied: ops.len(),
        summary,
    })
}

fn primary_sheet_view_mut(sheet: &mut Worksheet) -> &mut SheetView {
    let views = sheet.sheet_views_mut().sheet_view_list_mut();
    if views.is_empty() {
        let mut view = SheetView::default();
        view.set_workbook_view_id(0);
        let mut sheet_views = SheetViews::default();
        sheet_views.add_sheet_view_list_mut(view);
        sheet.set_sheets_views(sheet_views);
    }
    &mut sheet.sheet_views_mut().sheet_view_list_mut()[0]
}

fn apply_freeze_panes(
    sheet: &mut Worksheet,
    freeze_rows: u32,
    freeze_cols: u32,
    top_left_cell: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    if freeze_rows == 0 && freeze_cols == 0 {
        bail!("freeze_rows and freeze_cols cannot both be 0");
    }

    let view = primary_sheet_view_mut(sheet);

    let inferred = if let Some(tlc) = top_left_cell {
        tlc.trim().to_string()
    } else {
        warnings.push(
            "WARN_FREEZE_PANES_TOPLEFT_DEFAULTED: top_left_cell inferred from freeze_rows/freeze_cols"
                .to_string(),
        );
        let col = freeze_cols.saturating_add(1).max(1);
        let row = freeze_rows.saturating_add(1).max(1);
        umya_spreadsheet::helper::coordinate::coordinate_from_index(col, row)
    };

    // Pane.topLeftCell is stored as a Coordinate (no $ locks).
    let mut coord = Coordinate::default();
    coord.set_coordinate(&inferred);

    let mut pane = Pane::default();
    if freeze_cols > 0 {
        pane.set_horizontal_split(freeze_cols as f64);
    }
    if freeze_rows > 0 {
        pane.set_vertical_split(freeze_rows as f64);
    }
    pane.set_top_left_cell(coord);
    pane.set_state(PaneStateValues::Frozen);
    let active_pane = active_pane_for_freeze(freeze_rows, freeze_cols);
    pane.set_active_pane(active_pane.clone());

    // LibreOffice interop: clear sheetView@topLeftCell so pane.topLeftCell is authoritative.
    // Some files arrive with a pre-existing view topLeftCell; keeping both can cause viewport
    // quirks in LO.
    view.set_top_left_cell("");
    view.set_pane(pane);

    // Keep selection aligned with the frozen active pane.
    view.selection_mut().clear();
    let mut selection = Selection::default();
    selection.set_pane(active_pane);

    let mut active_cell = Coordinate::default();
    active_cell.set_coordinate(&inferred);
    selection.set_active_cell(active_cell);
    selection
        .sequence_of_references_mut()
        .set_sqref(inferred.as_str());
    view.set_selection(selection);

    Ok(())
}

fn active_pane_for_freeze(freeze_rows: u32, freeze_cols: u32) -> PaneValues {
    match (freeze_rows > 0, freeze_cols > 0) {
        (true, true) => PaneValues::BottomRight,
        (true, false) => PaneValues::BottomLeft,
        (false, true) => PaneValues::BottomRight, // avoid umya "TopRight" string quirk
        (false, false) => PaneValues::BottomRight,
    }
}

fn validate_margin_value(field: &str, value: f64) -> Result<()> {
    if !value.is_finite() {
        bail!("{field} margin must be finite");
    }
    if value < 0.0 {
        bail!("{field} margin must be >= 0");
    }
    Ok(())
}

fn set_print_area_defined_name(
    book: &mut umya_spreadsheet::Workbook,
    sheet_name: &str,
    range: &str,
) -> Result<()> {
    let sheet_index = resolve_sheet_index(book, sheet_name)?;
    let (start, end) = parse_a1_range(range)?;

    let start_abs = umya_spreadsheet::helper::coordinate::coordinate_from_index_with_lock(
        start.0, start.1, true, true,
    );
    let end_abs = umya_spreadsheet::helper::coordinate::coordinate_from_index_with_lock(
        end.0, end.1, true, true,
    );
    let sheet_prefix = format_sheet_prefix(sheet_name);
    let refers_to = format!("{sheet_prefix}{start_abs}:{end_abs}");

    // Remove any workbook-scoped print area entries for this sheet to avoid duplicates.
    {
        let defined = book.defined_names_mut();
        defined.retain(|d| {
            if d.name() != "_xlnm.Print_Area" {
                return true;
            }
            if d.has_local_sheet_id() {
                return d.local_sheet_id() != sheet_index;
            }
            true
        });
    }

    let sheet = book
        .sheet_by_name_mut(sheet_name).ok()
        .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;

    // If present on the sheet, update in place; otherwise create.
    let mut found = false;
    {
        let names = sheet.defined_names_mut();
        for defined in names.iter_mut() {
            if defined.name() == "_xlnm.Print_Area" {
                defined.set_address(refers_to.clone());
                defined.set_local_sheet_id(sheet_index);
                found = true;
            }
        }
        // Deduplicate within the sheet scope.
        if found {
            let mut kept = false;
            names.retain(|d| {
                if d.name() != "_xlnm.Print_Area" {
                    return true;
                }
                if !kept {
                    kept = true;
                    true
                } else {
                    false
                }
            });
        }
    }

    if !found {
        sheet
            .add_defined_name("_xlnm.Print_Area".to_string(), refers_to)
            .map_err(|e| anyhow!("failed to add defined name: {e}"))?;
        // Set local sheet id on the just-added entry.
        if let Some(last) = sheet.defined_names_mut().last_mut()
            && last.name() == "_xlnm.Print_Area"
        {
            last.set_local_sheet_id(sheet_index);
        }
    }

    Ok(())
}

fn resolve_sheet_index(book: &umya_spreadsheet::Workbook, sheet_name: &str) -> Result<u32> {
    for (idx, sheet) in book.sheet_collection().iter().enumerate() {
        if sheet.name() == sheet_name {
            return Ok(idx as u32);
        }
    }
    bail!("sheet '{}' not found", sheet_name)
}

fn parse_a1_range(range: &str) -> Result<((u32, u32), (u32, u32))> {
    let trimmed = range.trim();
    if trimmed.is_empty() {
        bail!("range is empty");
    }
    let range_part = if let Some((_, tail)) = trimmed.rsplit_once('!') {
        tail
    } else {
        trimmed
    };
    let mut parts = range_part.split(':');
    let a = parts.next().unwrap_or("").trim();
    let b = parts.next().unwrap_or(a).trim();
    if a.is_empty() {
        bail!("range is empty");
    }
    let (ac, ar, _, _) = umya_spreadsheet::helper::coordinate::index_from_coordinate(a);
    let (bc, br, _, _) = umya_spreadsheet::helper::coordinate::index_from_coordinate(b);
    let (Some(ac), Some(ar), Some(bc), Some(br)) = (ac, ar, bc, br) else {
        bail!("invalid range: {range}");
    };
    Ok(((ac.min(bc), ar.min(br)), (ac.max(bc), ar.max(br))))
}

fn format_sheet_prefix(sheet_name: &str) -> String {
    if sheet_name_needs_quoting(sheet_name) {
        let escaped = sheet_name.replace('\'', "''");
        format!("'{escaped}'!")
    } else {
        format!("{sheet_name}!")
    }
}

fn sheet_name_needs_quoting(name: &str) -> bool {
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

fn apply_page_breaks(sheet: &mut Worksheet, row_breaks: &[u32], col_breaks: &[u32]) {
    let rb = sheet.row_breaks_mut().break_list_mut();
    rb.clear();
    for &id in row_breaks {
        let mut brk = Break::default();
        brk.set_id(id).set_manual_page_break(true);
        rb.push(brk);
    }

    let cb = sheet.column_breaks_mut().break_list_mut();
    cb.clear();
    for &id in col_breaks {
        let mut brk = Break::default();
        brk.set_id(id).set_manual_page_break(true);
        cb.push(brk);
    }
}
