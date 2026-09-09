#![cfg(feature = "recalc-formualizer")]

use agent_spreadsheet::verification::ProofStatus;
use agent_spreadsheet_mcp::model::EvaluationState;
use agent_spreadsheet_mcp::state::AppState;
use agent_spreadsheet_mcp::tools::{
    ListWorkbooksParams, VerifyWorkbookParams, list_workbooks, verify_workbook,
};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod support;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../agent-spreadsheet/tests/fixtures/f1")
        .join(name)
}

#[tokio::test]
async fn verify_workbook_evaluates_both_mcp_resources() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.copy_workbook(&fixture("unevaluated_broken.xlsx"), "broken.xlsx");
    let state = Arc::new(AppState::new(Arc::new(workspace.config())));
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

    let response = verify_workbook(
        state,
        VerifyWorkbookParams {
            baseline_workbook_or_fork_id: workbook_id.clone(),
            current_workbook_or_fork_id: workbook_id,
            targets: Vec::new(),
            sheet_name: None,
            include_named_range_deltas: false,
            errors_only: true,
            targets_only: false,
        },
    )
    .await?;

    assert_eq!(response.proof_status, ProofStatus::Proved);
    assert_eq!(response.baseline_state, EvaluationState::ErrorsFound);
    assert_eq!(response.current_state, EvaluationState::ErrorsFound);
    assert_eq!(
        response
            .baseline_evaluation_coverage
            .evaluated_formula_cells,
        2
    );
    assert_eq!(
        response.current_evaluation_coverage.evaluated_formula_cells,
        2
    );
    assert_eq!(response.preexisting_errors.len(), 2);
    Ok(())
}

#[tokio::test]
async fn verify_workbook_detects_changed_error_at_the_same_address() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.copy_workbook(&fixture("real_errors.xlsx"), "baseline-errors.xlsx");
    let changed_path = workspace.copy_workbook(&fixture("real_errors.xlsx"), "changed-errors.xlsx");
    let mut changed = umya_spreadsheet::reader::xlsx::read(&changed_path)?;
    changed
        .sheet_by_name_mut("Sheet1").ok()
        .unwrap()
        .cell_mut("A1")
        .set_formula("UNKNOWNFN(2)")
        .set_formula_result_default("#NAME?");
    umya_spreadsheet::writer::xlsx::write(&changed, &changed_path)?;

    let state = Arc::new(AppState::new(Arc::new(workspace.config())));
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
    let baseline_id = list
        .workbooks
        .iter()
        .find(|workbook| workbook.slug == "baseline-errors")
        .unwrap()
        .workbook_id
        .clone();
    let changed_id = list
        .workbooks
        .iter()
        .find(|workbook| workbook.slug == "changed-errors")
        .unwrap()
        .workbook_id
        .clone();

    let response = verify_workbook(
        state,
        VerifyWorkbookParams {
            baseline_workbook_or_fork_id: baseline_id,
            current_workbook_or_fork_id: changed_id,
            targets: Vec::new(),
            sheet_name: None,
            include_named_range_deltas: false,
            errors_only: true,
            targets_only: false,
        },
    )
    .await?;

    assert_eq!(response.proof_status, ProofStatus::DifferencesFound);
    assert!(response.new_errors.is_empty());
    assert_eq!(response.preexisting_errors.len(), 3);
    let changed_a1 = response
        .preexisting_errors
        .iter()
        .find(|delta| delta.address == "Sheet1!A1")
        .unwrap();
    assert_ne!(changed_a1.before_error, changed_a1.after_error);
    assert_ne!(changed_a1.before_formula, changed_a1.after_formula);
    Ok(())
}
