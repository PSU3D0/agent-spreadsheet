#![cfg(feature = "recalc")]

use anyhow::Result;
use agent_spreadsheet_mcp::model::WorkbookId;
use agent_spreadsheet_mcp::tools::fork::{
    ApplyFormulaPatternParams, ApplyStagedChangeParams, CreateForkParams, apply_formula_pattern,
    apply_staged_change, create_fork,
};
use agent_spreadsheet_mcp::tools::param_enums::{BatchMode, FillDirection};
use agent_spreadsheet_mcp::tools::{ListWorkbooksParams, list_workbooks};

mod support;

fn recalc_state(
    workspace: &support::TestWorkspace,
) -> std::sync::Arc<agent_spreadsheet_mcp::state::AppState> {
    let config = workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
    });
    support::app_state_with_config(config)
}

#[tokio::test(flavor = "current_thread")]
async fn apply_formula_pattern_preview_stages_and_apply() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("pattern.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value_number(1);
        sheet.cell_mut("B1").set_value_number(2);
    });

    let state = recalc_state(&workspace);
    let list = list_workbooks(
        state.clone(),
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
    let workbook_id = list.workbooks[0].workbook_id.clone();
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let preview = apply_formula_pattern(
        state.clone(),
        ApplyFormulaPatternParams {
            fork_id: fork.fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            target_range: "C1:C3".to_string(),
            anchor_cell: "C1".to_string(),
            base_formula: "A1+B1".to_string(),
            fill_direction: Some(FillDirection::Down),
            relative_mode: None,
            mode: Some(BatchMode::Preview),
            label: Some("fill sums".to_string()),
        },
    )
    .await?;
    let change_id = preview.change_id.clone().expect("change_id");

    // Preview should not mutate the fork.
    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let formula_c2 = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.cell("C2").map(|c| c.formula().to_string())
    })?;
    assert!(formula_c2.is_none());

    apply_staged_change(
        state.clone(),
        ApplyStagedChangeParams {
            fork_id: fork.fork_id.clone(),
            change_id,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let formulas = fork_wb.with_sheet("Sheet1", |sheet| {
        vec![
            sheet.cell("C1").unwrap().formula().to_string(),
            sheet.cell("C2").unwrap().formula().to_string(),
            sheet.cell("C3").unwrap().formula().to_string(),
        ]
    })?;

    assert_eq!(formulas[0], "A1 + B1");
    assert_eq!(formulas[1], "A2 + B2");
    assert_eq!(formulas[2], "A3 + B3");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn apply_formula_pattern_validates_anchor_and_direction() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("invalid.xlsx", |_| {});

    let state = recalc_state(&workspace);
    let list = list_workbooks(
        state.clone(),
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
    let workbook_id = list.workbooks[0].workbook_id.clone();
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let err = apply_formula_pattern(
        state.clone(),
        ApplyFormulaPatternParams {
            fork_id: fork.fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            target_range: "C1:C3".to_string(),
            anchor_cell: "C2".to_string(),
            base_formula: "A1+B1".to_string(),
            fill_direction: Some(FillDirection::Down),
            relative_mode: None,
            mode: Some(BatchMode::Apply),
            label: None,
        },
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("target_range must start at anchor_cell")
    );

    Ok(())
}
