#![cfg(feature = "recalc")]

use anyhow::Result;
use serde_json::json;
use agent_spreadsheet_mcp::model::WorkbookId;
use agent_spreadsheet_mcp::tools::fork::{
    ApplyStagedChangeParams, ColumnSizeBatchParamsInput, CreateForkParams, apply_staged_change,
    column_size_batch, create_fork,
};
use agent_spreadsheet_mcp::tools::{ListWorkbooksParams, list_workbooks};

mod support;

fn app_state(
    workspace: &support::TestWorkspace,
) -> std::sync::Arc<agent_spreadsheet_mcp::state::AppState> {
    let config = workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
    });
    support::app_state_with_config(config)
}

async fn first_workbook_id(
    state: std::sync::Arc<agent_spreadsheet_mcp::state::AppState>,
) -> Result<WorkbookId> {
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

async fn widths_abc_after_apply(
    state: std::sync::Arc<agent_spreadsheet_mcp::state::AppState>,
    workbook_id: WorkbookId,
    range: &str,
    width_chars: f64,
) -> Result<(f64, f64, f64)> {
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let params: ColumnSizeBatchParamsInput = serde_json::from_value(json!({
        "fork_id": fork.fork_id,
        "sheet_name": "Data",
        "mode": "apply",
        "ops": [
            {"range": range, "size": {"kind":"width","width_chars": width_chars}}
        ]
    }))?;
    let resp = column_size_batch(state.clone(), params).await?;
    assert_eq!(resp.mode, "apply");

    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(resp.fork_id.clone()))
        .await?;
    let (a, b, c) = fork_wb.with_sheet("Data", |sheet| {
        let a = sheet
            .column_dimension("A")
            .expect("A column")
            .width();
        let b = sheet
            .column_dimension("B")
            .expect("B column")
            .width();
        let c = sheet
            .column_dimension("C")
            .expect("C column")
            .width();
        (a, b, c)
    })?;
    Ok((a, b, c))
}

#[tokio::test(flavor = "current_thread")]
async fn column_size_batch_sets_manual_width() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("cols.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.set_name("Data");
        sheet.cell_mut("A1").set_value("x");
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let params: ColumnSizeBatchParamsInput = serde_json::from_value(json!({
        "fork_id": fork.fork_id,
        "sheet_name": "Data",
        "mode": "apply",
        "ops": [
            {"range":"A:A", "size": {"kind":"width","width_chars": 22.0}}
        ]
    }))?;
    let resp = column_size_batch(state.clone(), params).await?;

    assert_eq!(resp.mode, "apply");
    assert_eq!(resp.ops_applied, 1);
    assert!(
        resp.summary
            .counts
            .get("columns_sized")
            .copied()
            .unwrap_or(0)
            >= 1
    );

    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(resp.fork_id.clone()))
        .await?;
    let width = fork_wb.with_sheet("Data", |sheet| {
        sheet
            .column_dimension("A")
            .expect("A column")
            .width()
    })?;
    assert!((width - 22.0).abs() < 0.001);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn column_size_batch_auto_width_increases_for_long_text() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("cols_auto.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.set_name("Data");
        sheet
            .cell_mut("A1")
            .set_value("this is a longish header");
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let params: ColumnSizeBatchParamsInput = serde_json::from_value(json!({
        "fork_id": fork.fork_id,
        "sheet_name": "Data",
        "mode": "apply",
        "ops": [
            {"range":"A:A", "size": {"kind":"auto"}}
        ]
    }))?;
    let resp = column_size_batch(state.clone(), params).await?;

    assert_eq!(resp.mode, "apply");
    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(resp.fork_id.clone()))
        .await?;
    let width = fork_wb.with_sheet("Data", |sheet| {
        sheet
            .column_dimension("A")
            .expect("A column")
            .width()
    })?;
    assert!(width > 8.38);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn column_size_batch_preview_can_be_applied() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("cols_preview.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.set_name("Data");
        sheet.cell_mut("B1").set_value("x");
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let params: ColumnSizeBatchParamsInput = serde_json::from_value(json!({
        "fork_id": fork.fork_id,
        "sheet_name": "Data",
        "mode": "preview",
        "label": "preview cols",
        "ops": [
            {"range":"B:B", "size": {"kind":"width","width_chars": 18.0}}
        ]
    }))?;
    let preview = column_size_batch(state.clone(), params).await?;

    let change_id = preview.change_id.clone().expect("change_id");
    assert_eq!(preview.mode, "preview");
    assert_eq!(preview.ops_applied, 1);

    let _applied = apply_staged_change(
        state.clone(),
        ApplyStagedChangeParams {
            fork_id: preview.fork_id.clone(),
            change_id,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&agent_spreadsheet_mcp::model::WorkbookId(preview.fork_id.clone()))
        .await?;
    let width = fork_wb.with_sheet("Data", |sheet| {
        sheet
            .column_dimension("B")
            .expect("B column")
            .width()
    })?;
    assert!((width - 18.0).abs() < 0.001);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn column_size_batch_warns_for_formula_without_cached_value() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("cols_formula.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.set_name("Data");
        sheet.cell_mut("A1").set_formula("1+1");
        // no cached formula result
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let params: ColumnSizeBatchParamsInput = serde_json::from_value(json!({
        "fork_id": fork.fork_id,
        "sheet_name": "Data",
        "mode": "apply",
        "ops": [
            {"range":"A:A", "size": {"kind":"auto"}}
        ]
    }))?;
    let resp = column_size_batch(state.clone(), params).await?;

    let warnings = resp.summary.warnings.join("\n");
    assert!(warnings.contains("WARN_AUTOWIDTH_FORMULA_NO_CACHED"));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn column_size_batch_accepts_reversed_column_spans() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("cols_reverse.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.set_name("Data");
        sheet.cell_mut("A1").set_value("a");
        sheet.cell_mut("B1").set_value("b");
        sheet.cell_mut("C1").set_value("c");
    });

    let state = app_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;

    let forward = widths_abc_after_apply(state.clone(), workbook_id.clone(), "A:C", 17.0).await?;
    let reversed_colon =
        widths_abc_after_apply(state.clone(), workbook_id.clone(), "C:A", 17.0).await?;
    let reversed_dash = widths_abc_after_apply(state.clone(), workbook_id, "C-A", 17.0).await?;

    assert_eq!(forward, reversed_colon);
    assert_eq!(forward, reversed_dash);
    Ok(())
}
