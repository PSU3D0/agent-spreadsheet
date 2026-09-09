#![cfg(feature = "recalc")]

use anyhow::Result;
use agent_spreadsheet_mcp::model::WorkbookId;
use agent_spreadsheet_mcp::tools::fork::{
    ApplyStagedChangeParams, CreateForkParams, TransformBatchParams, TransformOp, TransformTarget,
    apply_staged_change, create_fork, transform_batch,
};
use agent_spreadsheet_mcp::tools::param_enums::{BatchMode, ReplaceMatchMode};
use agent_spreadsheet_mcp::tools::{
    ListWorkbooksParams, SheetOverviewParams, list_workbooks, sheet_overview,
};

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
async fn transform_batch_clear_range_clears_values_keeps_formulas_by_default() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_clear.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        sheet.cell_mut("B1").set_formula("A1".to_string());
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

    transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ClearRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Range {
                    range: "A1:B1".to_string(),
                },
                clear_values: true,
                clear_formulas: false,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let (a1, b1_formula) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet
            .cell("A1")
            .map(|c| c.value().to_string())
            .unwrap_or_default();
        let b1 = sheet.cell("B1").expect("B1");
        (a1, b1.formula().to_string())
    })?;

    assert_eq!(a1, "");
    assert_eq!(b1_formula, "A1");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_preview_stages_and_apply() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_preview.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        sheet.cell_mut("B1").set_formula("A1".to_string());
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

    let preview = transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ClearRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Range {
                    range: "A1:B1".to_string(),
                },
                clear_values: true,
                clear_formulas: false,
            }],
            mode: Some(BatchMode::Preview),
            label: Some("blank inputs".to_string()),

            formula_parse_policy: None,
        },
    )
    .await?;
    let change_id = preview.change_id.clone().expect("change_id");

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let a1_before = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet
            .cell("A1")
            .map(|c| c.value().to_string())
            .unwrap_or_default()
    })?;
    assert_eq!(a1_before, "x");

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
    let (a1_after, b1_formula) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet
            .cell("A1")
            .map(|c| c.value().to_string())
            .unwrap_or_default();
        let b1_formula = sheet.cell("B1").expect("B1").formula().to_string();
        (a1, b1_formula)
    })?;

    assert_eq!(a1_after, "");
    assert_eq!(b1_formula, "A1");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_region_target_resolves() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_region.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        sheet.cell_mut("B2").set_value("y");
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

    let overview = sheet_overview(
        state.clone(),
        SheetOverviewParams {
            workbook_or_fork_id: WorkbookId(fork.fork_id.clone()),
            sheet_name: "Sheet1".to_string(),
            max_regions: None,
            max_headers: None,
            include_headers: None,
        },
    )
    .await?;
    let region_id = overview
        .detected_regions
        .iter()
        .find(|r| r.bounds.contains("A1"))
        .map(|r| r.id)
        .or_else(|| overview.detected_regions.first().map(|r| r.id))
        .expect("detected region");

    transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ClearRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Region { region_id },
                clear_values: true,
                clear_formulas: false,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let (a1, b2) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet
            .cell("A1")
            .map(|c| c.value().to_string())
            .unwrap_or_default();
        let b2 = sheet
            .cell("B2")
            .map(|c| c.value().to_string())
            .unwrap_or_default();
        (a1, b2)
    })?;

    assert_eq!(a1, "");
    assert_eq!(b2, "");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_cells_target_skips_missing_and_handles_duplicates() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_cells.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        sheet.cell_mut("C3").set_value("keep");
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

    let resp = transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ClearRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Cells {
                    cells: vec!["A1".to_string(), "A1".to_string(), "Z99".to_string()],
                },
                clear_values: true,
                clear_formulas: false,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(
        resp.summary.counts.get("cells_value_cleared").copied(),
        Some(1)
    );

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let (a1, c3, z99_exists) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet
            .cell("A1")
            .map(|c| c.value().to_string())
            .unwrap_or_default();
        let c3 = sheet
            .cell("C3")
            .map(|c| c.value().to_string())
            .unwrap_or_default();
        let z99_exists = sheet.cell("Z99").is_some();
        (a1, c3, z99_exists)
    })?;

    assert_eq!(a1, "");
    assert_eq!(c3, "keep");
    assert!(!z99_exists);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_accepts_reversed_range() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_reversed.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        sheet.cell_mut("C3").set_value("z");
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

    transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ClearRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Range {
                    range: "C3:A1".to_string(),
                },
                clear_values: true,
                clear_formulas: false,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let (a1, c3) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet
            .cell("A1")
            .map(|c| c.value().to_string())
            .unwrap_or_default();
        let c3 = sheet
            .cell("C3")
            .map(|c| c.value().to_string())
            .unwrap_or_default();
        (a1, c3)
    })?;

    assert_eq!(a1, "");
    assert_eq!(c3, "");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_noop_flags_do_not_change_cells() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_noop.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        sheet.cell_mut("B1").set_formula("A1".to_string());
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

    let resp = transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ClearRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Range {
                    range: "A1:B1".to_string(),
                },
                clear_values: false,
                clear_formulas: false,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(
        resp.summary.counts.get("cells_value_cleared").copied(),
        Some(0)
    );
    assert_eq!(
        resp.summary.counts.get("cells_formula_cleared").copied(),
        Some(0)
    );

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let (a1, b1_formula) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet
            .cell("A1")
            .map(|c| c.value().to_string())
            .unwrap_or_default();
        let b1_formula = sheet.cell("B1").expect("B1").formula().to_string();
        (a1, b1_formula)
    })?;

    assert_eq!(a1, "x");
    assert_eq!(b1_formula, "A1");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_counts_mixed_range() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_counts.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        sheet.cell_mut("B1").set_formula("A1".to_string());
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

    let resp = transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ClearRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Range {
                    range: "A1:B1".to_string(),
                },
                clear_values: true,
                clear_formulas: false,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(resp.summary.counts.get("cells_touched").copied(), Some(2));
    assert_eq!(
        resp.summary.counts.get("cells_value_cleared").copied(),
        Some(1)
    );
    assert_eq!(
        resp.summary.counts.get("cells_formula_cleared").copied(),
        Some(0)
    );
    assert_eq!(
        resp.summary
            .counts
            .get("cells_skipped_keep_formulas")
            .copied(),
        Some(1)
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_clear_formulas_only_removes_formula_keeps_literal_values() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_clear_formulas.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
        sheet.cell_mut("B1").set_formula("A1".to_string());
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

    let resp = transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ClearRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Range {
                    range: "A1:B1".to_string(),
                },
                clear_values: false,
                clear_formulas: true,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(
        resp.summary.counts.get("cells_formula_cleared").copied(),
        Some(1)
    );
    assert_eq!(
        resp.summary.counts.get("cells_value_cleared").copied(),
        Some(0)
    );

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let (a1, b1_formula) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet
            .cell("A1")
            .map(|c| c.value().to_string())
            .unwrap_or_default();
        let b1_formula = sheet.cell("B1").expect("B1").formula().to_string();
        (a1, b1_formula)
    })?;

    assert_eq!(a1, "x");
    assert!(b1_formula.is_empty());

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_fill_range_creates_cells_and_skips_formulas_by_default() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_fill.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_formula("1+1".to_string());
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

    let resp = transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::FillRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Range {
                    range: "A1:B2".to_string(),
                },
                value: "x".to_string(),
                is_formula: false,
                overwrite_formulas: false,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(resp.summary.counts.get("cells_value_set").copied(), Some(3));
    assert_eq!(
        resp.summary
            .counts
            .get("cells_skipped_keep_formulas")
            .copied(),
        Some(1)
    );

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let (a1_formula, b2_value) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet.cell("A1").expect("A1");
        let b2 = sheet.cell("B2").expect("B2");
        (a1.formula().to_string(), b2.value().to_string())
    })?;

    assert_eq!(a1_formula, "1+1");
    assert_eq!(b2_value, "x");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_replace_in_range_replaces_values_exact() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_replace_values.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Foo");
        sheet.cell_mut("B1").set_value("FooBar");
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

    let resp = transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ReplaceInRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Range {
                    range: "A1:B1".to_string(),
                },
                find: "Foo".to_string(),
                replace: "Bar".to_string(),
                match_mode: ReplaceMatchMode::Exact,
                case_sensitive: true,
                include_formulas: false,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(
        resp.summary.counts.get("cells_value_replaced").copied(),
        Some(1)
    );

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let (a1, b1) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet.cell("A1").expect("A1").value().to_string();
        let b1 = sheet.cell("B1").expect("B1").value().to_string();
        (a1, b1)
    })?;

    assert_eq!(a1, "Bar");
    assert_eq!(b1, "FooBar");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_replace_in_range_contains_case_sensitive() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_replace_contains.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("FooBarFoo");
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

    let resp = transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ReplaceInRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Cells {
                    cells: vec!["A1".to_string()],
                },
                find: "Foo".to_string(),
                replace: "Z".to_string(),
                match_mode: ReplaceMatchMode::Contains,
                case_sensitive: true,
                include_formulas: false,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(
        resp.summary.counts.get("cells_value_replaced").copied(),
        Some(1)
    );

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.cell("A1").expect("A1").value().to_string()
    })?;

    assert_eq!(a1, "ZBarZ");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_replace_in_range_skips_formulas_by_default() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_replace_formula_skip.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("B1").set_formula("A1".to_string());
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

    let resp = transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ReplaceInRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Cells {
                    cells: vec!["B1".to_string()],
                },
                find: "A1".to_string(),
                replace: "A2".to_string(),
                match_mode: ReplaceMatchMode::Exact,
                case_sensitive: true,
                include_formulas: false,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(
        resp.summary
            .counts
            .get("cells_skipped_keep_formulas")
            .copied(),
        Some(1)
    );

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let b1_formula = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.cell("B1").expect("B1").formula().to_string()
    })?;

    assert_eq!(b1_formula, "A1");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_replace_in_range_can_mutate_formulas_when_enabled() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_replace_formula.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("B1").set_formula("A1".to_string());
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

    let resp = transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ReplaceInRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Cells {
                    cells: vec!["B1".to_string()],
                },
                find: "A1".to_string(),
                replace: "A2".to_string(),
                match_mode: ReplaceMatchMode::Exact,
                case_sensitive: true,
                include_formulas: true,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(
        resp.summary.counts.get("cells_formula_replaced").copied(),
        Some(1)
    );

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let b1_formula = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.cell("B1").expect("B1").formula().to_string()
    })?;

    assert_eq!(b1_formula, "A2");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_fill_range_preview_stages_and_apply() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_fill_preview.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
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

    let preview = transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::FillRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Range {
                    range: "A1:B1".to_string(),
                },
                value: "y".to_string(),
                is_formula: false,
                overwrite_formulas: false,
            }],
            mode: Some(BatchMode::Preview),
            label: Some("fill".to_string()),

            formula_parse_policy: None,
        },
    )
    .await?;
    let change_id = preview.change_id.clone().expect("change_id");

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let a1_before = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.cell("A1").expect("A1").value().to_string()
    })?;
    assert_eq!(a1_before, "x");

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
    let (a1_after, b1_after) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet.cell("A1").expect("A1").value().to_string();
        let b1 = sheet.cell("B1").expect("B1").value().to_string();
        (a1, b1)
    })?;

    assert_eq!(a1_after, "y");
    assert_eq!(b1_after, "y");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_replace_in_range_preview_stages_and_apply() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_replace_preview.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Foo");
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

    let preview = transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ReplaceInRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Cells {
                    cells: vec!["A1".to_string()],
                },
                find: "Foo".to_string(),
                replace: "Bar".to_string(),
                match_mode: ReplaceMatchMode::Exact,
                case_sensitive: true,
                include_formulas: false,
            }],
            mode: Some(BatchMode::Preview),
            label: Some("replace".to_string()),

            formula_parse_policy: None,
        },
    )
    .await?;
    let change_id = preview.change_id.clone().expect("change_id");

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let a1_before = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.cell("A1").expect("A1").value().to_string()
    })?;
    assert_eq!(a1_before, "Foo");

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
    let a1_after = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.cell("A1").expect("A1").value().to_string()
    })?;

    assert_eq!(a1_after, "Bar");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_replace_in_range_exact_case_insensitive() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_replace_ci.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("FOO");
        sheet.cell_mut("A2").set_value("foo");
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

    transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ReplaceInRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Range {
                    range: "A1:A2".to_string(),
                },
                find: "foo".to_string(),
                replace: "bar".to_string(),
                match_mode: ReplaceMatchMode::Exact,
                case_sensitive: false,
                include_formulas: false,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let (a1, a2) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet.cell("A1").expect("A1").value().to_string();
        let a2 = sheet.cell("A2").expect("A2").value().to_string();
        (a1, a2)
    })?;

    assert_eq!(a1, "bar");
    assert_eq!(a2, "bar");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_replace_in_range_contains_replaces_all_occurrences() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_replace_all.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("aaaa");
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

    transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ReplaceInRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Cells {
                    cells: vec!["A1".to_string()],
                },
                find: "aa".to_string(),
                replace: "b".to_string(),
                match_mode: ReplaceMatchMode::Contains,
                case_sensitive: true,
                include_formulas: false,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.cell("A1").expect("A1").value().to_string()
    })?;

    assert_eq!(a1, "bb");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_multiple_ops_last_wins() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_order.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("x");
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

    transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                TransformOp::FillRange {
                    sheet_name: "Sheet1".to_string(),
                    target: TransformTarget::Cells {
                        cells: vec!["A1".to_string()],
                    },
                    value: "y".to_string(),
                    is_formula: false,
                    overwrite_formulas: false,
                },
                TransformOp::FillRange {
                    sheet_name: "Sheet1".to_string(),
                    target: TransformTarget::Cells {
                        cells: vec!["A1".to_string()],
                    },
                    value: "z".to_string(),
                    is_formula: false,
                    overwrite_formulas: false,
                },
            ],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.cell("A1").expect("A1").value().to_string()
    })?;

    assert_eq!(a1, "z");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_replace_in_range_contains_rejects_case_insensitive() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_replace_contains_ci.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Foo");
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

    let err = transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::ReplaceInRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Cells {
                    cells: vec!["A1".to_string()],
                },
                find: "Foo".to_string(),
                replace: "Bar".to_string(),
                match_mode: ReplaceMatchMode::Contains,
                case_sensitive: false,
                include_formulas: false,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("requires case_sensitive=true"));

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn transform_batch_fill_range_overwrite_formulas_removes_formula() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("transform_fill_overwrite.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_formula("1+1".to_string());
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

    transform_batch(
        state.clone(),
        TransformBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![TransformOp::FillRange {
                sheet_name: "Sheet1".to_string(),
                target: TransformTarget::Cells {
                    cells: vec!["A1".to_string()],
                },
                value: "x".to_string(),
                is_formula: false,
                overwrite_formulas: true,
            }],
            mode: Some(BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let (a1_formula, a1_value) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet.cell("A1").expect("A1");
        (a1.formula().to_string(), a1.value().to_string())
    })?;

    assert!(a1_formula.is_empty());
    assert_eq!(a1_value, "x");

    Ok(())
}
