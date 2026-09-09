#![cfg(feature = "recalc")]

use anyhow::Result;
use agent_spreadsheet_mcp::model::WorkbookId;
use agent_spreadsheet_mcp::tools::fork::{
    ApplyStagedChangeParams, CreateForkParams, apply_staged_change, create_fork,
};
use agent_spreadsheet_mcp::tools::param_enums::BatchMode;
use agent_spreadsheet_mcp::tools::rules_batch::{
    DataValidationKind, DataValidationSpec, RulesBatchParams, RulesOp, ValidationMessage,
    rules_batch,
};
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
async fn rules_batch_set_data_validation_list_persists_and_is_idempotent() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("dv.xlsx", |book| {
        let _ = book.new_sheet("Lists");
        let lists = book.get_sheet_by_name_mut("Lists").ok().unwrap();
        lists.get_cell_mut("A1").set_value("A");
        lists.get_cell_mut("A2").set_value("B");
        lists.get_cell_mut("A3").set_value("C");
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

    let op = RulesOp::SetDataValidation {
        sheet_name: "Sheet1".to_string(),
        target_range: "B3:B10".to_string(),
        validation: DataValidationSpec {
            kind: DataValidationKind::List,
            formula1: "=Lists!$A$1:$A$3".to_string(),
            formula2: None,
            allow_blank: Some(false),
            prompt: Some(ValidationMessage {
                title: "Choose".to_string(),
                message: "Pick one".to_string(),
            }),
            error: Some(ValidationMessage {
                title: "Invalid".to_string(),
                message: "Use the dropdown".to_string(),
            }),
        },
    };

    rules_batch(
        state.clone(),
        RulesBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![op.clone()],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    // Apply twice; should not duplicate validations.
    rules_batch(
        state.clone(),
        RulesBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![op],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    fork_wb.with_sheet("Sheet1", |sheet| {
        let dvs = sheet.get_data_validations().expect("data validations");
        let list = dvs.get_data_validation_list();
        assert_eq!(list.len(), 1);
        let dv = &list[0];

        assert_eq!(dv.get_sequence_of_references().get_sqref(), "B3:B10");
        assert_eq!(dv.get_type(), &umya_spreadsheet::DataValidationValues::List);
        assert_eq!(dv.get_formula1(), "Lists!$A$1:$A$3");
        assert_eq!(dv.get_prompt_title(), "Choose");
        assert_eq!(dv.get_prompt(), "Pick one");
        assert_eq!(dv.get_error_title(), "Invalid");
        assert_eq!(dv.get_error_message(), "Use the dropdown");
    })?;

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rules_batch_preview_then_apply_staged_change() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("dv_preview.xlsx", |book| {
        let _ = book.new_sheet("Lists");
        let lists = book.get_sheet_by_name_mut("Lists").ok().unwrap();
        lists.get_cell_mut("A1").set_value("A");
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

    let preview = rules_batch(
        state.clone(),
        RulesBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![RulesOp::SetDataValidation {
                sheet_name: "Sheet1".to_string(),
                target_range: "B3:B10".to_string(),
                validation: DataValidationSpec {
                    kind: DataValidationKind::List,
                    formula1: "=Lists!$A$1:$A$1".to_string(),
                    formula2: None,
                    allow_blank: None,
                    prompt: None,
                    error: None,
                },
            }],
            mode: Some(BatchMode::Preview),
            label: Some("add dropdown".to_string()),

            formula_parse_policy: None,
        },
    )
    .await?;
    let change_id = preview.change_id.clone().expect("change_id");

    // Preview should not mutate the fork.
    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    fork_wb.with_sheet("Sheet1", |sheet| {
        assert!(sheet.get_data_validations().is_none());
    })?;

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
    fork_wb.with_sheet("Sheet1", |sheet| {
        let dvs = sheet.get_data_validations().expect("data validations");
        let list = dvs.get_data_validation_list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].get_sequence_of_references().get_sqref(), "B3:B10");
    })?;

    Ok(())
}
