use anyhow::Result;
use agent_spreadsheet::model::{LayoutMode, LayoutRender, WorkbookId};
use agent_spreadsheet::tools::{LayoutPageParams, ListWorkbooksParams, layout_page, list_workbooks};
use std::sync::Arc;

mod support;

fn app_state(workspace: &support::TestWorkspace) -> Arc<agent_spreadsheet::state::AppState> {
    workspace.app_state()
}

async fn first_workbook_id(state: Arc<agent_spreadsheet::state::AppState>) -> Result<WorkbookId> {
    let list = list_workbooks(
        state,
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        },
    )
    .await?;
    Ok(list.workbooks[0].workbook_id.clone())
}

// ── column widths ─────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn layout_page_returns_column_widths() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("widths.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Label");
        sheet.cell_mut("B1").set_value("Value");
        sheet.column_dimension_mut("A").set_width(28.0);
        sheet.column_dimension_mut("B").set_width(12.0);
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:B1".to_string()),
            mode: None,
            max_col_width: None,
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: None,
        },
    )
    .await?;

    assert_eq!(resp.columns.len(), 2);
    assert_eq!(resp.columns[0].col, "A");
    // 28.0 > default max_col_width of 20, so capped at 20
    assert_eq!(resp.columns[0].width_chars, 20.0);
    assert!(!resp.columns[0].is_default_width);
    assert_eq!(resp.columns[1].col, "B");
    assert_eq!(resp.columns[1].width_chars, 12.0);
    assert!(!resp.columns[1].is_default_width);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn layout_page_default_width_for_unset_column() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("default_width.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        // No explicit column width set
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:A1".to_string()),
            mode: None,
            max_col_width: None,
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: None,
        },
    )
    .await?;

    assert_eq!(resp.columns.len(), 1);
    // Default Excel column width is 8.43 chars — well under max_col_width cap of 20
    // is_default_width may be true or false depending on xlsx round-trip,
    // but the width should always be at or near 8.43.
    assert!(
        resp.columns[0].width_chars <= 20.0,
        "width should not exceed max_col_width: {}",
        resp.columns[0].width_chars
    );
    // No explicit wide width was set
    assert!(
        resp.columns[0].width_chars <= 12.0,
        "default width should be under 12 chars, got: {}",
        resp.columns[0].width_chars
    );
    Ok(())
}

// ── bold and italic ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn layout_page_detects_bold_and_italic() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("font.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Bold");
        sheet.style_mut("A1").font_mut().set_bold(true);

        sheet.cell_mut("B1").set_value("Italic");
        sheet.style_mut("B1").font_mut().set_italic(true);

        sheet.cell_mut("C1").set_value("Plain");
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:C1".to_string()),
            mode: None,
            max_col_width: None,
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: None,
        },
    )
    .await?;

    assert_eq!(resp.rows.len(), 1);
    let cells = &resp.rows[0].cells;
    assert_eq!(cells[0].bold, Some(true), "A1 should be bold");
    assert_eq!(cells[0].italic, None);
    assert_eq!(cells[1].bold, None);
    assert_eq!(cells[1].italic, Some(true), "B1 should be italic");
    assert_eq!(cells[2].bold, None);
    assert_eq!(cells[2].italic, None);
    Ok(())
}

// ── borders ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn layout_page_captures_border_styles() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("borders.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Total");
        sheet
            .style_mut("A1")
            .borders_mut()
            .bottom_border_mut()
            .set_border_style("medium");
        sheet
            .style_mut("A1")
            .borders_mut()
            .top_border_mut()
            .set_border_style("thin");
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:A1".to_string()),
            mode: None,
            max_col_width: None,
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: None,
        },
    )
    .await?;

    let cell = &resp.rows[0].cells[0];
    let borders = cell.borders.as_ref().expect("borders should be present");
    assert_eq!(borders.bottom.as_deref(), Some("medium"));
    assert_eq!(borders.top.as_deref(), Some("thin"));
    assert!(borders.left.is_none());
    assert!(borders.right.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn layout_page_no_borders_on_plain_cell() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("no_borders.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("plain");
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:A1".to_string()),
            mode: None,
            max_col_width: None,
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: None,
        },
    )
    .await?;

    let cell = &resp.rows[0].cells[0];
    assert!(cell.borders.is_none(), "plain cell should have no borders");
    Ok(())
}

// ── merged cells ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn layout_page_reports_merged_cells() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("merged.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("B1").set_value("Header");
        sheet.add_merge_cells("B1:D1");
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:E1".to_string()),
            mode: None,
            max_col_width: None,
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: None,
        },
    )
    .await?;

    assert!(
        resp.merged_cells.contains(&"B1:D1".to_string()),
        "merged_cells should contain B1:D1, got: {:?}",
        resp.merged_cells
    );

    let b1 = resp.rows[0]
        .cells
        .iter()
        .find(|c| c.address == "B1")
        .unwrap();
    assert_eq!(
        b1.merge_start,
        Some(true),
        "B1 should be flagged merge_start"
    );

    let a1 = resp.rows[0]
        .cells
        .iter()
        .find(|c| c.address == "A1")
        .unwrap();
    assert_eq!(a1.merge_start, None, "A1 should not be merge_start");
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn layout_page_filters_merges_outside_range() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("merge_filter.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        // Merge far outside the render range
        sheet.add_merge_cells("Z1:AB1");
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:B2".to_string()),
            mode: None,
            max_col_width: None,
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: None,
        },
    )
    .await?;

    assert!(
        resp.merged_cells.is_empty(),
        "out-of-range merge should not appear: {:?}",
        resp.merged_cells
    );
    Ok(())
}

// ── formula mode ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn layout_page_formula_mode_returns_formula_text() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("formulas.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value_number(10.0_f64);
        sheet.cell_mut("A2").set_value_number(20.0_f64);
        sheet.cell_mut("A3").set_formula("SUM(A1:A2)");
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:A3".to_string()),
            mode: Some(LayoutMode::Formulas),
            max_col_width: None,
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: None,
        },
    )
    .await?;

    // A1 and A2 are plain values
    assert_eq!(resp.rows[0].cells[0].value.as_deref(), Some("10"));
    assert_eq!(resp.rows[1].cells[0].value.as_deref(), Some("20"));
    // A3 is a formula — should start with '='
    let a3_val = resp.rows[2].cells[0].value.as_deref().unwrap_or("");
    assert!(
        a3_val.starts_with('='),
        "formula mode cell should start with '=', got: {a3_val}"
    );
    assert!(a3_val.contains("SUM"), "should contain SUM, got: {a3_val}");
    Ok(())
}

// ── range cap ─────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn layout_page_caps_oversized_range() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("big.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    // AX = column 50, 200 rows — both beyond the 80×25 cap
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:AX200".to_string()),
            mode: None,
            max_col_width: None,
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: None,
        },
    )
    .await?;

    assert!(resp.truncated, "should be marked truncated");
    assert!(
        resp.rows.len() <= 80,
        "rows should be capped: {}",
        resp.rows.len()
    );
    assert!(
        resp.columns.len() <= 25,
        "cols should be capped: {}",
        resp.columns.len()
    );
    Ok(())
}

// ── ASCII render ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn layout_page_ascii_render_present_when_requested() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("ascii.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Revenue");
        sheet.style_mut("A1").font_mut().set_bold(true);
        sheet.cell_mut("B1").set_value_number(1_000_000.0_f64);
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:B1".to_string()),
            mode: None,
            max_col_width: None,
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: Some(LayoutRender::Ascii),
        },
    )
    .await?;

    let ascii = resp.ascii_render.expect("ascii_render should be present");
    // Bold cell should produce * markers
    assert!(
        ascii.contains('*'),
        "bold cell should use * markers:\n{ascii}"
    );
    // Should use box-drawing chars for the grid frame
    assert!(
        ascii.contains('─') || ascii.contains('│'),
        "should contain box-drawing chars:\n{ascii}"
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn layout_page_json_render_no_ascii_by_default() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("no_ascii.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:A1".to_string()),
            mode: None,
            max_col_width: None,
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: None, // default = json
        },
    )
    .await?;

    assert!(
        resp.ascii_render.is_none(),
        "ascii_render should be None for default json render"
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn layout_page_both_render_returns_json_and_ascii() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("both.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:A1".to_string()),
            mode: None,
            max_col_width: None,
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: Some(LayoutRender::Both),
        },
    )
    .await?;

    assert!(
        resp.ascii_render.is_some(),
        "ascii_render should be present for render=both"
    );
    assert!(!resp.columns.is_empty());
    assert!(!resp.rows.is_empty());
    Ok(())
}

// ── max_col_width cap ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn layout_page_max_col_width_caps_wide_columns() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("wide.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        sheet.column_dimension_mut("A").set_width(60.0);
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:A1".to_string()),
            mode: None,
            max_col_width: Some(15),
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: None,
        },
    )
    .await?;

    assert_eq!(
        resp.columns[0].width_chars, 15.0,
        "width should be capped at 15"
    );
    Ok(())
}

// ── alignment ─────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn layout_page_captures_explicit_alignment() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("align.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("centered");
        sheet
            .style_mut("A1")
            .alignment_mut()
            .set_horizontal(umya_spreadsheet::HorizontalAlignmentValues::Center);
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let resp = layout_page(
        state,
        LayoutPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            range: Some("A1:A1".to_string()),
            mode: None,
            max_col_width: None,
            fit_columns: None,
            trim_empty_columns: Some(false),
            render: None,
        },
    )
    .await?;

    let cell = &resp.rows[0].cells[0];
    assert_eq!(cell.align_h.as_deref(), Some("center"));
    Ok(())
}
