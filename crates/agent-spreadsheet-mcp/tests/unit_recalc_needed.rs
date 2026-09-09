#![cfg(feature = "recalc")]

use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

use agent_spreadsheet_mcp::model::WorkbookId;
use agent_spreadsheet_mcp::recalc::{RecalcBackend, RecalcResult};
use agent_spreadsheet_mcp::state::AppState;
use agent_spreadsheet_mcp::tools::fork::{
    CreateForkParams, RecalculateParams, create_fork, edit_batch, recalculate_with_backend,
};
use agent_spreadsheet_mcp::tools::write_normalize::{CellEditInput, EditBatchParamsInput};
use agent_spreadsheet_mcp::tools::{
    ListWorkbooksParams, RangeValuesParams, list_workbooks, range_values,
};

mod support;

struct TestRecalcBackend;

#[async_trait]
impl RecalcBackend for TestRecalcBackend {
    async fn recalculate(
        &self,
        _fork_work_path: &Path,
        _timeout_ms: Option<u64>,
    ) -> Result<RecalcResult> {
        Ok(RecalcResult {
            duration_ms: 1,
            was_warm: true,
            backend_name: "test",
            cells_evaluated: None,
            eval_errors: Some(vec!["backend diagnostic".to_string()]),
            evaluation_coverage: agent_spreadsheet_mcp::model::EvaluationCoverage {
                formula_cells: 0,
                evaluated_formula_cells: 0,
                unsupported_formula_cells: 0,
                error_formula_cells: 0,
                source: agent_spreadsheet_mcp::model::EvaluationSource::None,
                freshness: agent_spreadsheet_mcp::model::EvaluationFreshness::CurrentRevision,
                revision_id: "test-revision".to_string(),
            },
            incomplete: false,
        })
    }

    fn is_available(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "test"
    }
}

fn recalc_state(workspace: &support::TestWorkspace) -> Arc<AppState> {
    let config = workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
    });
    support::app_state_with_config(config)
}

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
async fn formula_edit_sets_recalc_needed_true() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("recalc_needed_edit.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(10);
        let out = sheet.get_cell_mut("A2");
        out.set_formula("A1*2");
        out.get_cell_value_mut().set_formula_result_default("0");
    });

    let state = recalc_state(&workspace);
    let workbook_id = first_workbook_id(state.clone()).await?;
    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let resp = edit_batch(
        state.clone(),
        EditBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            edits: vec![CellEditInput::Shorthand("B1==SUM(1,2)".to_string())],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert!(resp.recalc_needed);

    let registry = state.fork_registry().unwrap().clone();
    let fork_ctx = registry.get_fork(&fork.fork_id)?;
    assert!(fork_ctx.recalc_needed);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn recalculate_clears_recalc_needed() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("recalc_needed_recalc.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(10);
        let out = sheet.get_cell_mut("A2");
        out.set_formula("A1*2");
        out.get_cell_value_mut().set_formula_result_default("0");
    });

    let state = recalc_state(&workspace);
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

    let registry = state.fork_registry().unwrap().clone();
    assert!(registry.get_fork(&fork.fork_id)?.recalc_needed);

    let response = recalculate_with_backend(
        state.clone(),
        RecalculateParams {
            fork_id: fork.fork_id.clone(),
            timeout_ms: 1000,
            backend: None,
        },
        Arc::new(TestRecalcBackend),
    )
    .await?;

    assert_eq!(response.error_count, Some(1));
    assert_eq!(response.evaluation_coverage.error_formula_cells, 0);
    assert!(!registry.get_fork(&fork.fork_id)?.recalc_needed);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn range_values_warns_when_stale_formulas_present() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("recalc_needed_read.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(10);
        let out = sheet.get_cell_mut("A2");
        out.set_formula("A1*2");
        out.get_cell_value_mut().set_formula_result_default("0");
    });

    let state = recalc_state(&workspace);
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

    let resp = range_values(
        state,
        RangeValuesParams {
            workbook_or_fork_id: WorkbookId(fork.fork_id),
            sheet_name: "Sheet1".to_string(),
            ranges: vec!["A2:A2".to_string()],
            include_headers: Some(false),
            format: None,
            page_size: None,

            include_formulas: None,
        },
    )
    .await?;

    assert!(
        resp.warnings
            .iter()
            .any(|w| w.code == "WARN_STALE_FORMULAS"),
        "expected WARN_STALE_FORMULAS warning"
    );
    Ok(())
}
