#![cfg(feature = "recalc")]

use anyhow::Result;
use agent_spreadsheet_mcp::model::{FillDescriptor, WorkbookId};
use agent_spreadsheet_mcp::tools::fork::{
    ApplyStagedChangeParams, CreateForkParams, apply_staged_change, create_fork,
};
use agent_spreadsheet_mcp::tools::param_enums::BatchMode;
use agent_spreadsheet_mcp::tools::rules_batch::{
    ConditionalFormatOperator, ConditionalFormatRuleSpec, ConditionalFormatStyleSpec,
    RulesBatchParams, RulesOp, rules_batch,
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
async fn rules_batch_add_conditional_format_persists_and_is_idempotent() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("cf.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(1);
        sheet.get_cell_mut("A2").set_value_number(-1);
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

    let op = RulesOp::AddConditionalFormat {
        sheet_name: "Sheet1".to_string(),
        target_range: "A1:A3".to_string(),
        rule: ConditionalFormatRuleSpec::CellIs {
            operator: ConditionalFormatOperator::LessThan,
            formula: "0".to_string(),
        },
        style: ConditionalFormatStyleSpec {
            fill_color: Some("#12AB34".to_string()),
            font_color: Some("#123456".to_string()),
            bold: Some(true),
        },
    };

    let first = rules_batch(
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
    assert_eq!(
        first
            .summary
            .counts
            .get("conditional_formats_added")
            .copied(),
        Some(1)
    );

    let second = rules_batch(
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
    assert_eq!(
        second
            .summary
            .counts
            .get("conditional_formats_skipped")
            .copied(),
        Some(1)
    );

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    fork_wb.with_sheet("Sheet1", |sheet| {
        let cfs = sheet.get_conditional_formatting_collection();
        assert_eq!(cfs.len(), 1);
        assert_eq!(cfs[0].get_sequence_of_references().get_sqref(), "A1:A3");
        assert_eq!(cfs[0].get_conditional_collection().len(), 1);

        let rule = &cfs[0].get_conditional_collection()[0];
        assert_eq!(
            rule.get_type(),
            &umya_spreadsheet::ConditionalFormatValues::CellIs
        );
        assert_eq!(
            rule.get_operator(),
            &umya_spreadsheet::ConditionalFormattingOperatorValues::LessThan
        );

        let st = rule.get_style().expect("expected dxf-backed style");
        let desc = agent_spreadsheet_mcp::styles::descriptor_from_style(st);
        assert_eq!(desc.font.as_ref().and_then(|f| f.bold), Some(true));
        assert_eq!(
            desc.font.as_ref().and_then(|f| f.color.as_deref()),
            Some("FF123456")
        );
        match desc.fill {
            Some(FillDescriptor::Pattern(p)) => {
                assert_eq!(p.foreground_color.as_deref(), Some("FF12AB34"));
            }
            other => panic!("expected pattern fill in dxf style, got: {other:?}"),
        }
    })?;

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rules_batch_conditional_format_preview_then_apply_staged_change() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("cf_preview.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(1);
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
            ops: vec![RulesOp::AddConditionalFormat {
                sheet_name: "Sheet1".to_string(),
                target_range: "A1:A3".to_string(),
                rule: ConditionalFormatRuleSpec::Expression {
                    formula: "A1>0".to_string(),
                },
                style: ConditionalFormatStyleSpec {
                    fill_color: Some("FFF5F7FA".to_string()),
                    font_color: Some("FF111111".to_string()),
                    bold: Some(false),
                },
            }],
            mode: Some(BatchMode::Preview),
            label: Some("cf".to_string()),

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
        assert!(sheet.get_conditional_formatting_collection().is_empty());
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
        let cfs = sheet.get_conditional_formatting_collection();
        assert_eq!(cfs.len(), 1);
        assert_eq!(cfs[0].get_sequence_of_references().get_sqref(), "A1:A3");
    })?;

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rules_batch_set_and_clear_conditional_formats() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("cf_set_clear.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(1);
        sheet.get_cell_mut("A2").set_value_number(-1);
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

    // Seed an existing rule.
    rules_batch(
        state.clone(),
        RulesBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![RulesOp::AddConditionalFormat {
                sheet_name: "Sheet1".to_string(),
                target_range: "A1:A3".to_string(),
                rule: ConditionalFormatRuleSpec::CellIs {
                    operator: ConditionalFormatOperator::LessThan,
                    formula: "0".to_string(),
                },
                style: ConditionalFormatStyleSpec {
                    fill_color: Some("#FF0000".to_string()),
                    font_color: Some("#000000".to_string()),
                    bold: Some(true),
                },
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    // Ensure the seed rule persisted.
    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    fork_wb.with_sheet("Sheet1", |sheet| {
        let cfs = sheet.get_conditional_formatting_collection();
        assert_eq!(cfs.len(), 1);
        assert_eq!(cfs[0].get_sequence_of_references().get_sqref(), "A1:A3");
    })?;

    // Replace the rule (set ensures only one rule for the range).
    let set_resp = rules_batch(
        state.clone(),
        RulesBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![RulesOp::SetConditionalFormat {
                sheet_name: "Sheet1".to_string(),
                target_range: "A1:A3".to_string(),
                rule: ConditionalFormatRuleSpec::Expression {
                    formula: "A1<0".to_string(),
                },
                style: ConditionalFormatStyleSpec {
                    fill_color: Some("#12AB34".to_string()),
                    font_color: Some("#123456".to_string()),
                    bold: Some(false),
                },
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(
        set_resp
            .summary
            .counts
            .get("conditional_formats_set")
            .copied(),
        Some(1)
    );

    // Setting the same op again should skip.
    let set_again = rules_batch(
        state.clone(),
        RulesBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![RulesOp::SetConditionalFormat {
                sheet_name: "Sheet1".to_string(),
                target_range: "A1:A3".to_string(),
                rule: ConditionalFormatRuleSpec::Expression {
                    formula: "A1<0".to_string(),
                },
                style: ConditionalFormatStyleSpec {
                    fill_color: Some("#12AB34".to_string()),
                    font_color: Some("#123456".to_string()),
                    bold: Some(false),
                },
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;
    assert_eq!(
        set_again
            .summary
            .counts
            .get("conditional_formats_set_skipped")
            .copied(),
        Some(1)
    );

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    fork_wb.with_sheet("Sheet1", |sheet| {
        let cfs = sheet.get_conditional_formatting_collection();
        assert_eq!(cfs.len(), 1);
        assert_eq!(cfs[0].get_sequence_of_references().get_sqref(), "A1:A3");
        assert_eq!(cfs[0].get_conditional_collection().len(), 1);

        let rule = &cfs[0].get_conditional_collection()[0];
        assert_eq!(
            rule.get_type(),
            &umya_spreadsheet::ConditionalFormatValues::Expression
        );

        let st = rule.get_style().expect("expected dxf-backed style");
        let desc = agent_spreadsheet_mcp::styles::descriptor_from_style(st);
        assert!(!desc.font.as_ref().and_then(|f| f.bold).unwrap_or(false));
        assert_eq!(
            desc.font.as_ref().and_then(|f| f.color.as_deref()),
            Some("FF123456")
        );
        match desc.fill {
            Some(FillDescriptor::Pattern(p)) => {
                assert_eq!(p.foreground_color.as_deref(), Some("FF12AB34"));
            }
            other => panic!("expected pattern fill in dxf style, got: {other:?}"),
        }
    })?;

    let clear = rules_batch(
        state.clone(),
        RulesBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![RulesOp::ClearConditionalFormats {
                sheet_name: "Sheet1".to_string(),
                target_range: "A1:A3".to_string(),
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;
    assert_eq!(
        clear
            .summary
            .counts
            .get("conditional_formats_cleared")
            .copied(),
        Some(1)
    );

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    fork_wb.with_sheet("Sheet1", |sheet| {
        assert!(sheet.get_conditional_formatting_collection().is_empty());
    })?;

    Ok(())
}
