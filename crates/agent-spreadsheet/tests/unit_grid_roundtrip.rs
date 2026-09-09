#![cfg(feature = "recalc")]

use anyhow::Result;
use serde_json::json;
use agent_spreadsheet::model::{FormulaParsePolicy, WorkbookId};
use agent_spreadsheet::tools::fork::{MatrixCell, TransformBatchParams, TransformOp};
use std::sync::Arc;

mod support;

async fn first_workbook_id(state: Arc<agent_spreadsheet::state::AppState>) -> Result<WorkbookId> {
    let mut resp = agent_spreadsheet::tools::list_workbooks(
        state,
        agent_spreadsheet::tools::ListWorkbooksParams {
            limit: None,
            offset: None,
            include_paths: None,
            folder: None,
            path_glob: None,
            slug_prefix: None,
        },
    )
    .await?;
    Ok(resp.workbooks.remove(0).workbook_id)
}

#[tokio::test(flavor = "current_thread")]
async fn test_write_matrix_applies_correctly() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("test.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Old");
        sheet.cell_mut("B1").set_formula("=SUM(1,2)");
    });

    let config = workspace.config_with(|c| {
        c.recalc_enabled = true;
    });
    let state = support::app_state_with_config(config);
    let workbook_id = first_workbook_id(state.clone()).await?;

    let create_fork = agent_spreadsheet::tools::fork::create_fork(
        state.clone(),
        agent_spreadsheet::tools::fork::CreateForkParams {
            workbook_or_fork_id: workbook_id.clone(),
        },
    )
    .await?;

    let fork_id = create_fork.fork_id;

    let op = TransformOp::WriteMatrix {
        sheet_name: "Sheet1".to_string(),
        anchor: "A1".to_string(),
        rows: vec![
            vec![
                Some(MatrixCell::Value(json!("New1"))),
                Some(MatrixCell::Value(json!("New2"))),
                None, // skipped cell
                Some(MatrixCell::Formula("=A1+1".to_string())),
            ],
            vec![
                Some(MatrixCell::Value(json!(42.0))),
                Some(MatrixCell::Value(json!(true))),
            ],
        ],
        overwrite_formulas: true,
    };

    let result = agent_spreadsheet::tools::fork::transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork_id.clone(),
            ops: vec![op],
            mode: None,
            label: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(result.ops_applied, 1);
    assert_eq!(result.summary.counts.get("cells_value_set"), Some(&4));
    assert_eq!(result.summary.counts.get("cells_formula_set"), Some(&1));

    let fork_ctx = state.fork_registry().unwrap().get_fork(&fork_id).unwrap();
    let book = umya_spreadsheet::reader::xlsx::read(&fork_ctx.work_path).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();

    assert_eq!(sheet.cell("A1").unwrap().value(), "New1");
    assert_eq!(sheet.cell("B1").unwrap().value(), "New2");
    assert!(!sheet.cell("B1").unwrap().is_formula()); // Overwritten
    assert_eq!(sheet.cell("D1").unwrap().formula(), "A1+1");
    assert!(sheet.cell("D1").unwrap().is_formula());
    assert_eq!(sheet.cell("A2").unwrap().value(), "42");
    assert_eq!(sheet.cell("B2").unwrap().value(), "TRUE");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn test_write_matrix_respects_overwrite_formulas() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("test2.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_formula("SUM(1,2)");
    });

    let config = workspace.config_with(|c| {
        c.recalc_enabled = true;
    });
    let state = support::app_state_with_config(config);
    let workbook_id = first_workbook_id(state.clone()).await?;

    let create_fork = agent_spreadsheet::tools::fork::create_fork(
        state.clone(),
        agent_spreadsheet::tools::fork::CreateForkParams {
            workbook_or_fork_id: workbook_id.clone(),
        },
    )
    .await?;

    let fork_id = create_fork.fork_id;

    let op = TransformOp::WriteMatrix {
        sheet_name: "Sheet1".to_string(),
        anchor: "A1".to_string(),
        rows: vec![vec![Some(MatrixCell::Value(json!("New1")))]],
        overwrite_formulas: false,
    };

    let result = agent_spreadsheet::tools::fork::transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork_id.clone(),
            ops: vec![op],
            mode: None,
            label: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(
        result.summary.counts.get("cells_skipped_keep_formulas"),
        Some(&1)
    );

    let fork_ctx = state.fork_registry().unwrap().get_fork(&fork_id).unwrap();
    let book = umya_spreadsheet::reader::xlsx::read(&fork_ctx.work_path).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    assert_eq!(sheet.cell("A1").unwrap().formula(), "SUM(1,2)");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn test_write_matrix_formula_parse_policy() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("test3.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Old");
    });

    let config = workspace.config_with(|c| {
        c.recalc_enabled = true;
    });
    let state = support::app_state_with_config(config);
    let workbook_id = first_workbook_id(state.clone()).await?;

    let create_fork = agent_spreadsheet::tools::fork::create_fork(
        state.clone(),
        agent_spreadsheet::tools::fork::CreateForkParams {
            workbook_or_fork_id: workbook_id.clone(),
        },
    )
    .await?;

    let fork_id = create_fork.fork_id;

    let op = TransformOp::WriteMatrix {
        sheet_name: "Sheet1".to_string(),
        anchor: "A1".to_string(),
        rows: vec![vec![Some(MatrixCell::Formula("=BAD_FORMULA(".to_string()))]],
        overwrite_formulas: true,
    };

    let result = agent_spreadsheet::tools::fork::transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork_id.clone(),
            ops: vec![op.clone()],
            mode: None,
            label: None,
            formula_parse_policy: Some(FormulaParsePolicy::Warn),
        },
    )
    .await?;

    assert!(result.formula_parse_diagnostics.is_some());
    assert_eq!(result.formula_parse_diagnostics.unwrap().total_errors, 1);

    // Warn mode drops the invalid formula cell, skipping it.
    let fork_ctx = state.fork_registry().unwrap().get_fork(&fork_id).unwrap();
    let book = umya_spreadsheet::reader::xlsx::read(&fork_ctx.work_path).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    assert_eq!(sheet.cell("A1").unwrap().value(), "Old");

    // Fail mode error
    let err = agent_spreadsheet::tools::fork::transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork_id.clone(),
            ops: vec![op],
            mode: None,
            label: None,
            formula_parse_policy: Some(FormulaParsePolicy::Fail),
        },
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("formula failed at A1"));

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn test_grid_export_and_import_roundtrip_core_data() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("grid_roundtrip.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Revenue");
        sheet.cell_mut("B1").set_formula("A1&\"!\"");
    });

    let config = workspace.config_with(|c| {
        c.recalc_enabled = true;
    });
    let state = support::app_state_with_config(config);
    let workbook_id = first_workbook_id(state.clone()).await?;

    let grid = agent_spreadsheet::tools::grid_export(
        state.clone(),
        agent_spreadsheet::tools::GridExportParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: "Sheet1".to_string(),
            range: "A1:B1".to_string(),
        },
    )
    .await?;

    assert_eq!(grid.anchor, "A1");
    assert!(!grid.rows.is_empty());
    assert!(grid.rows[0].cells.iter().any(|c| c.f.is_some()));

    let create_fork = agent_spreadsheet::tools::fork::create_fork(
        state.clone(),
        agent_spreadsheet::tools::fork::CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let fork_id = create_fork.fork_id;
    let import_resp = agent_spreadsheet::tools::fork::grid_import(
        state.clone(),
        agent_spreadsheet::tools::fork::GridImportParams {
            fork_id: fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            anchor: "C3".to_string(),
            grid,
            clear_target: false,
            mode: None,
            label: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(import_resp.mode, "apply");

    let fork_ctx = state.fork_registry().unwrap().get_fork(&fork_id).unwrap();
    let book = umya_spreadsheet::reader::xlsx::read(&fork_ctx.work_path).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();

    assert_eq!(sheet.cell("C3").unwrap().value(), "Revenue");
    assert!(sheet.cell("D3").unwrap().is_formula());

    Ok(())
}
