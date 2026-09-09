#![cfg(all(feature = "recalc", feature = "recalc-formualizer"))]

use agent_spreadsheet_mcp::model::WorkbookId;
use agent_spreadsheet_mcp::tools::fork::{
    CreateForkParams, RecalculateParams, create_fork, edit_batch, recalculate,
};
use agent_spreadsheet_mcp::tools::write_normalize::{CellEditInput, EditBatchParamsInput};
use agent_spreadsheet_mcp::tools::{ListWorkbooksParams, list_workbooks};
use agent_spreadsheet_mcp::{RecalcBackendKind, state::AppState};
use anyhow::Result;
use std::sync::Arc;

mod support;

async fn first_workbook_id(state: Arc<AppState>) -> Result<WorkbookId> {
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

#[tokio::test(flavor = "current_thread")]
async fn recalculate_uses_formualizer_backend_and_updates_formula_cache() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("formualizer_recalc.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value_number(10);
        let out = sheet.cell_mut("A2");
        out.set_formula("A1*2");
        out.cell_value_mut().set_formula_result_default("0");
    });

    let config = Arc::new(workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
        cfg.recalc_backend = RecalcBackendKind::Formualizer;
    }));
    let state = Arc::new(AppState::new(config));

    let workbook_id = first_workbook_id(state.clone()).await?;
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    edit_batch(
        state.clone(),
        EditBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            edits: vec![CellEditInput::Shorthand("A1=11".to_string())],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    let recalc = recalculate(
        state.clone(),
        RecalculateParams {
            fork_id: fork.fork_id.clone(),
            timeout_ms: 30_000,
            backend: Some(RecalcBackendKind::Formualizer),
        },
    )
    .await?;

    assert_eq!(recalc.backend, "formualizer");
    assert!(recalc.cells_evaluated.unwrap_or_default() > 0);

    let fork_ctx = state
        .fork_registry()
        .expect("fork registry")
        .get_fork(&fork.fork_id)?;
    let saved = umya_spreadsheet::reader::xlsx::read(&fork_ctx.work_path)?;
    let sheet = saved.sheet_by_name("Sheet1").ok().expect("Sheet1 exists");
    assert_eq!(sheet.cell("A2").expect("A2 exists").value(), "22");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn recalculate_one_pass_updates_non_last_sheet_formula_chains() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("formualizer_one_pass_multisheet.xlsx", |book| {
        let sheet1 = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet1.cell_mut("A1").set_value_number(10);
        for (addr, formula) in [
            ("A2", "A1+10"),
            ("A3", "A2+10"),
            ("A4", "A3+1"),
            ("A5", "SUM(A3:A4)"),
        ] {
            let cell = sheet1.cell_mut(addr);
            cell.set_formula(formula);
            cell.cell_value_mut().set_formula_result_default("0");
        }

        book.new_sheet("Sheet2").expect("add Sheet2");
        let sheet2 = book.sheet_by_name_mut("Sheet2").ok().unwrap();
        sheet2.cell_mut("A1").set_value_number(10);
        for (addr, formula) in [
            ("A2", "A1+10"),
            ("A3", "A2+10"),
            ("A4", "A3+1"),
            ("A5", "SUM(A3:A4)"),
        ] {
            let cell = sheet2.cell_mut(addr);
            cell.set_formula(formula);
            cell.cell_value_mut().set_formula_result_default("0");
        }
    });

    let config = Arc::new(workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
        cfg.recalc_backend = RecalcBackendKind::Formualizer;
    }));
    let state = Arc::new(AppState::new(config));

    let workbook_id = first_workbook_id(state.clone()).await?;
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let recalc = recalculate(
        state.clone(),
        RecalculateParams {
            fork_id: fork.fork_id.clone(),
            timeout_ms: 30_000,
            backend: Some(RecalcBackendKind::Formualizer),
        },
    )
    .await?;

    assert_eq!(recalc.backend, "formualizer");
    assert!(recalc.cells_evaluated.unwrap_or_default() >= 8);

    let fork_ctx = state
        .fork_registry()
        .expect("fork registry")
        .get_fork(&fork.fork_id)?;
    let saved = umya_spreadsheet::reader::xlsx::read(&fork_ctx.work_path)?;

    for sheet_name in ["Sheet1", "Sheet2"] {
        let sheet = saved
            .sheet_by_name(sheet_name).ok()
            .unwrap_or_else(|| panic!("{sheet_name} exists"));
        assert_eq!(sheet.cell("A3").expect("A3 exists").value(), "30");
        assert_eq!(sheet.cell("A4").expect("A4 exists").value(), "31");
        assert_eq!(sheet.cell("A5").expect("A5 exists").value(), "61");
    }

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn recalculate_populates_eval_errors() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("formualizer_eval_errors.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_formula("UNKNOWNFN(1)");
        sheet.cell_mut("A2").set_formula("A3+1");
        sheet.cell_mut("A3").set_formula("A2+1");
    });

    let config = Arc::new(workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
        cfg.recalc_backend = RecalcBackendKind::Formualizer;
    }));
    let state = Arc::new(AppState::new(config));
    let workbook_id = first_workbook_id(state.clone()).await?;
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let recalc = recalculate(
        state,
        RecalculateParams {
            fork_id: fork.fork_id,
            timeout_ms: 30_000,
            backend: Some(RecalcBackendKind::Formualizer),
        },
    )
    .await?;

    let errors = recalc.eval_errors.unwrap_or_default();
    assert!(!errors.is_empty());
    assert!(errors.iter().any(|e| {
        let lower = e.to_ascii_lowercase();
        lower.contains("circular") || lower.contains("name") || lower.contains("unknown")
    }));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn recalculate_timeout_can_cancel_long_eval() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("formualizer_timeout.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value_number(1);
        for row in 2..=30_000u32 {
            sheet
                .cell_mut((1, row))
                .set_formula(format!("A{}+1", row - 1));
        }
    });

    let config = Arc::new(workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
        cfg.recalc_backend = RecalcBackendKind::Formualizer;
    }));
    let state = Arc::new(AppState::new(config));
    let workbook_id = first_workbook_id(state.clone()).await?;
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let result = recalculate(
        state,
        RecalculateParams {
            fork_id: fork.fork_id,
            timeout_ms: 1,
            backend: Some(RecalcBackendKind::Formualizer),
        },
    )
    .await;

    let response = result?;
    assert_eq!(
        response.state,
        agent_spreadsheet_mcp::model::EvaluationState::Partial
    );
    assert!(
        response.evaluation_coverage.evaluated_formula_cells
            < response.evaluation_coverage.formula_cells
    );
    assert!(
        response
            .eval_errors
            .unwrap_or_default()
            .iter()
            .any(|error| error.contains("interrupted"))
    );
    Ok(())
}
