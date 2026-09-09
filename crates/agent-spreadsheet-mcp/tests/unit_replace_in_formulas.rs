//! MCP-level tests for replace_in_formulas fork tool.

#![cfg(feature = "recalc")]

use anyhow::Result;
use agent_spreadsheet_mcp::model::WorkbookId;
use agent_spreadsheet_mcp::tools::fork::{
    CreateForkParams, ReplaceInFormulasParams, create_fork, replace_in_formulas,
};
use agent_spreadsheet_mcp::tools::param_enums::BatchMode;
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

fn create_formula_workbook(workspace: &support::TestWorkspace, name: &str) -> std::path::PathBuf {
    workspace.create_workbook(name, |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Label");
        sheet.cell_mut("B1").set_value("Value");
        sheet
            .cell_mut("B2")
            .set_formula("SUM(C2:C10)".to_string());
        sheet
            .cell_mut("B3")
            .set_formula("AVERAGE(C2:C10)".to_string());
        sheet
            .cell_mut("B4")
            .set_formula("Sheet1!D5+Sheet1!D6".to_string());
        // literal value — should NOT be touched
        sheet.cell_mut("B5").set_value("SUM(C2:C10)");
    })
}

async fn fork_workbook(
    workspace: &support::TestWorkspace,
    state: &std::sync::Arc<agent_spreadsheet_mcp::state::AppState>,
    name: &str,
) -> Result<String> {
    create_formula_workbook(workspace, name);

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

    let search = name.trim_end_matches(".xlsx");
    let wb = list
        .workbooks
        .iter()
        .find(|w| w.slug.contains(search))
        .unwrap_or(&list.workbooks[0]);

    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: wb.workbook_id.clone(),
        },
    )
    .await?;

    Ok(fork.fork_id)
}

#[tokio::test(flavor = "current_thread")]
async fn replace_in_formulas_apply_changes_formulas() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let state = recalc_state(&workspace);
    let fork_id = fork_workbook(&workspace, &state, "apply_test.xlsx").await?;

    let response = replace_in_formulas(
        state.clone(),
        ReplaceInFormulasParams {
            fork_id: fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            find: "C2:C10".to_string(),
            replace: "D2:D20".to_string(),
            range: None,
            regex: false,
            case_sensitive: true,
            mode: Some(BatchMode::Apply),
            label: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(response.formulas_changed, 2);
    assert!(response.formulas_checked >= 3);
    assert!(response.recalc_needed);
    assert!(!response.samples.is_empty());
    assert_eq!(response.mode, "apply");
    assert!(response.change_id.is_none());

    // Verify the fork workbook has updated formulas
    let fork_wb = state.open_workbook(&WorkbookId(fork_id.clone())).await?;
    let (b2_formula, b3_formula) = fork_wb.with_sheet("Sheet1", |sheet| {
        let b2 = sheet.cell("B2").unwrap().formula().to_string();
        let b3 = sheet.cell("B3").unwrap().formula().to_string();
        (b2, b3)
    })?;
    assert_eq!(b2_formula, "SUM(D2:D20)");
    assert_eq!(b3_formula, "AVERAGE(D2:D20)");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn replace_in_formulas_preview_stages_change() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let state = recalc_state(&workspace);
    let fork_id = fork_workbook(&workspace, &state, "preview_test.xlsx").await?;

    let response = replace_in_formulas(
        state.clone(),
        ReplaceInFormulasParams {
            fork_id: fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            find: "Sheet1!".to_string(),
            replace: "Sheet2!".to_string(),
            range: None,
            regex: false,
            case_sensitive: true,
            mode: Some(BatchMode::Preview),
            label: Some("fix sheet refs".to_string()),
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(response.formulas_changed, 1);
    assert_eq!(response.mode, "preview");
    assert!(response.change_id.is_some());

    // Fork should NOT be modified yet (preview mode)
    let fork_wb = state.open_workbook(&WorkbookId(fork_id.clone())).await?;
    let b4_formula = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.cell("B4").unwrap().formula().to_string()
    })?;
    assert_eq!(b4_formula, "Sheet1!D5+Sheet1!D6");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn replace_in_formulas_regex_mode() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let state = recalc_state(&workspace);
    let fork_id = fork_workbook(&workspace, &state, "regex_test.xlsx").await?;

    let response = replace_in_formulas(
        state.clone(),
        ReplaceInFormulasParams {
            fork_id: fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            find: r"Sheet1!D(\d+)".to_string(),
            replace: "Sheet2!E$1".to_string(),
            range: None,
            regex: true,
            case_sensitive: true,
            mode: Some(BatchMode::Apply),
            label: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(response.formulas_changed, 1);

    let fork_wb = state.open_workbook(&WorkbookId(fork_id.clone())).await?;
    let b4_formula = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.cell("B4").unwrap().formula().to_string()
    })?;
    assert_eq!(b4_formula, "Sheet2!E5+Sheet2!E6");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn replace_in_formulas_range_scoped() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let state = recalc_state(&workspace);
    let fork_id = fork_workbook(&workspace, &state, "range_scope.xlsx").await?;

    let response = replace_in_formulas(
        state.clone(),
        ReplaceInFormulasParams {
            fork_id: fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            find: "C2:C10".to_string(),
            replace: "X1:X5".to_string(),
            range: Some("B2:B2".to_string()),
            regex: false,
            case_sensitive: true,
            mode: Some(BatchMode::Apply),
            label: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(response.formulas_changed, 1, "only B2 in range");

    let fork_wb = state.open_workbook(&WorkbookId(fork_id.clone())).await?;
    let (b2, b3) = fork_wb.with_sheet("Sheet1", |sheet| {
        let b2 = sheet.cell("B2").unwrap().formula().to_string();
        let b3 = sheet.cell("B3").unwrap().formula().to_string();
        (b2, b3)
    })?;
    assert_eq!(b2, "SUM(X1:X5)");
    assert_eq!(b3, "AVERAGE(C2:C10)"); // untouched

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn replace_in_formulas_no_op() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let state = recalc_state(&workspace);
    let fork_id = fork_workbook(&workspace, &state, "noop_test.xlsx").await?;

    let response = replace_in_formulas(
        state.clone(),
        ReplaceInFormulasParams {
            fork_id: fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            find: "NONEXISTENT".to_string(),
            replace: "WHATEVER".to_string(),
            range: None,
            regex: false,
            case_sensitive: true,
            mode: Some(BatchMode::Apply),
            label: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(response.formulas_changed, 0);
    assert!(!response.recalc_needed);
    assert!(
        response
            .warnings
            .iter()
            .any(|w| w.contains("WARN_NO_MATCH"))
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn replace_in_formulas_fail_policy_rejects_invalid_replacements() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let state = recalc_state(&workspace);
    let fork_id = fork_workbook(&workspace, &state, "fail_policy.xlsx").await?;

    let err = replace_in_formulas(
        state.clone(),
        ReplaceInFormulasParams {
            fork_id: fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            find: "SUM(".to_string(),
            replace: "SUM((".to_string(),
            range: None,
            regex: false,
            case_sensitive: true,
            mode: Some(BatchMode::Apply),
            label: None,
            formula_parse_policy: Some(agent_spreadsheet_mcp::model::FormulaParsePolicy::Fail),
        },
    )
    .await
    .expect_err("fail policy should reject invalid replacements");

    assert!(
        err.to_string().contains("failed parse"),
        "unexpected error: {err}"
    );

    // Ensure fork workbook was not mutated.
    let fork_wb = state.open_workbook(&WorkbookId(fork_id.clone())).await?;
    let b2_formula = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.cell("B2").unwrap().formula().to_string()
    })?;
    assert_eq!(b2_formula, "SUM(C2:C10)");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn replace_in_formulas_noop_preserves_prior_recalc_needed_state() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let state = recalc_state(&workspace);
    let fork_id = fork_workbook(&workspace, &state, "recalc_state.xlsx").await?;

    // First mutation sets recalc_needed=true on the fork.
    let first = replace_in_formulas(
        state.clone(),
        ReplaceInFormulasParams {
            fork_id: fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            find: "C2:C10".to_string(),
            replace: "D2:D20".to_string(),
            range: None,
            regex: false,
            case_sensitive: true,
            mode: Some(BatchMode::Apply),
            label: None,
            formula_parse_policy: None,
        },
    )
    .await?;
    assert!(first.recalc_needed);

    // No-op replacement should still report recalc_needed=true from prior fork state.
    let second = replace_in_formulas(
        state.clone(),
        ReplaceInFormulasParams {
            fork_id: fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            find: "NONEXISTENT".to_string(),
            replace: "WHATEVER".to_string(),
            range: None,
            regex: false,
            case_sensitive: true,
            mode: Some(BatchMode::Apply),
            label: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(second.formulas_changed, 0);
    assert!(
        second.recalc_needed,
        "prior fork recalc_needed state should be preserved"
    );

    Ok(())
}
