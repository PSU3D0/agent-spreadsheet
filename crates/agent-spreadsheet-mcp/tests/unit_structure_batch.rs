#![cfg(feature = "recalc")]

use anyhow::Result;
use serde_json::json;
use agent_spreadsheet_mcp::model::WorkbookId;
use agent_spreadsheet_mcp::styles::descriptor_from_style;
use agent_spreadsheet_mcp::tools::fork::{
    ApplyStagedChangeParams, CreateForkParams, StructureBatchParamsInput, StructureOp,
    apply_staged_change, create_fork, normalize_structure_batch, structure_batch,
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

#[tokio::test(flavor = "current_thread")]
async fn structure_batch_insert_rows_moves_cells() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("structure_rows.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("keep");
        sheet.get_cell_mut("A2").set_value("move");
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

    structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::InsertRows {
                    sheet_name: "Sheet1".to_string(),
                    at_row: 2,
                    count: 1,
                    expand_adjacent_sums: false,
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,

            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let values = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet.get_cell("A1").unwrap().get_value().to_string();
        let a2 = sheet
            .get_cell("A2")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default();
        let a3 = sheet
            .get_cell("A3")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default();
        (a1, a2, a3)
    })?;

    assert_eq!(values.0, "keep");
    assert_eq!(values.1, "");
    assert_eq!(values.2, "move");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn structure_batch_copy_range_shifts_formulas_and_copies_style() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("structure_copy_range.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(1);
        sheet.get_cell_mut("B1").set_value_number(10);
        sheet.get_cell_mut("A2").set_value_number(2);
        sheet.get_cell_mut("B2").set_value_number(20);

        sheet.get_cell_mut("C1").set_formula("A1+B1".to_string());
        sheet.get_style_mut("C1").get_font_mut().set_bold(true);
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

    structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::CopyRange {
                    sheet_name: "Sheet1".to_string(),
                    dest_sheet_name: None,
                    src_range: "C1:C1".to_string(),
                    dest_anchor: "D1".to_string(),
                    include_styles: true,
                    include_formulas: true,
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,

            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let (src_formula, dest_formula, dest_bold) = fork_wb.with_sheet("Sheet1", |sheet| {
        let src = sheet.get_cell("C1").expect("C1").get_formula().to_string();
        let dest = sheet.get_cell("D1").expect("D1").get_formula().to_string();
        let desc = descriptor_from_style(sheet.get_cell("D1").expect("D1").get_style());
        (src, dest, desc.font.and_then(|f| f.bold).unwrap_or(false))
    })?;

    assert_eq!(src_formula, "A1+B1");
    assert_eq!(dest_formula.replace(' ', ""), "B1+C1");
    assert!(dest_bold);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn structure_batch_move_range_moves_and_clears_source() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("structure_move_range.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("x");
        sheet.get_style_mut("A1").get_font_mut().set_bold(true);
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

    structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::MoveRange {
                    sheet_name: "Sheet1".to_string(),
                    dest_sheet_name: None,
                    src_range: "A1:A1".to_string(),
                    dest_anchor: "C3".to_string(),
                    include_styles: true,
                    include_formulas: false,
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,

            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let (a1_val, c3_val, c3_bold) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a1 = sheet
            .get_cell("A1")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default();
        let c3 = sheet.get_cell("C3").expect("C3");
        let desc = descriptor_from_style(c3.get_style());
        (
            a1,
            c3.get_value().to_string(),
            desc.font.and_then(|f| f.bold).unwrap_or(false),
        )
    })?;

    assert_eq!(a1_val, "");
    assert_eq!(c3_val, "x");
    assert!(c3_bold);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn structure_batch_copy_range_rejects_overlap() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("structure_copy_overlap.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("x");
        sheet.get_cell_mut("B2").set_value("y");
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

    let err = structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::CopyRange {
                    sheet_name: "Sheet1".to_string(),
                    dest_sheet_name: None,
                    src_range: "A1:B2".to_string(),
                    dest_anchor: "B2".to_string(),
                    include_styles: false,
                    include_formulas: false,
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,

            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("overlaps source"));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn structure_batch_preview_stages_and_apply() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("structure_preview.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("B1").set_value("move");
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

    let preview = structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::InsertCols {
                    sheet_name: "Sheet1".to_string(),
                    at_col: "B".to_string(),
                    count: 1,
                }
                .into(),
            ],
            mode: Some(BatchMode::Preview),
            label: Some("insert col".to_string()),

            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;
    let change_id = preview.change_id.clone().expect("change_id");

    // Preview should not mutate the fork.
    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let b1 = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.get_cell("B1").unwrap().get_value().to_string()
    })?;
    assert_eq!(b1, "move");

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
    let moved = fork_wb.with_sheet("Sheet1", |sheet| {
        (
            sheet
                .get_cell("B1")
                .map(|c| c.get_value().to_string())
                .unwrap_or_default(),
            sheet.get_cell("C1").unwrap().get_value().to_string(),
        )
    })?;
    assert_eq!(moved.0, "");
    assert_eq!(moved.1, "move");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn structure_batch_preview_includes_change_count() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("structure_preview_count.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("x");
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

    let preview = structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::InsertCols {
                    sheet_name: "Sheet1".to_string(),
                    at_col: "A".to_string(),
                    count: 1,
                }
                .into(),
            ],
            mode: Some(BatchMode::Preview),
            label: None,

            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert!(
        preview.summary.counts.contains_key("preview_change_items"),
        "preview should include preview_change_items"
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn structure_batch_rename_sheet_handles_quoted_sheet_names() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("structure_rename_quoted.xlsx", |book| {
        let inputs = book.get_sheet_mut(&0).ok().unwrap();
        inputs.set_name("My Sheet");
        inputs.get_cell_mut("A1").set_value_number(3);

        book.new_sheet("Calc").unwrap();
        let calc = book.get_sheet_by_name_mut("Calc").ok().unwrap();
        calc.get_cell_mut("A1")
            .set_formula("'My Sheet'!A1".to_string());
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

    structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::RenameSheet {
                    old_name: "My Sheet".to_string(),
                    new_name: "Data".to_string(),
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,

            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let formula = fork_wb.with_sheet("Calc", |sheet| {
        sheet.get_cell("A1").unwrap().get_formula().to_string()
    })?;
    assert_eq!(formula, "Data!A1");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn structure_batch_create_sheet_inserts_at_position() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("structure_create_sheet.xlsx", |_| {});

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

    structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::CreateSheet {
                    name: "First".to_string(),
                    position: Some(0),
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,

            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let sheets = fork_wb.sheet_names();
    assert_eq!(sheets[0], "First");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn structure_batch_delete_sheet_guard_prevents_last_sheet() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("structure_delete_last.xlsx", |_| {});

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

    let err = structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::DeleteSheet {
                    name: "Sheet1".to_string(),
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,

            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("last remaining sheet"));

    Ok(())
}

#[test]
fn structure_batch_accepts_op_and_add_sheet_alias() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            { "op": "add_sheet", "name": "Inputs" },
            { "kind": "create_sheet", "name": "Accounts" }
        ]
    });

    let params: StructureBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, warnings) = normalize_structure_batch(params).unwrap();

    assert!(matches!(normalized.ops[0], StructureOp::CreateSheet { .. }));
    assert!(warnings.iter().any(|w| w.code == "WARN_ALIAS_KIND"));
}

#[test]
fn structure_batch_does_not_warn_when_no_alias() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            { "kind": "create_sheet", "name": "Inputs" }
        ]
    });

    let params: StructureBatchParamsInput = serde_json::from_value(input).unwrap();
    let (_normalized, warnings) = normalize_structure_batch(params).unwrap();

    assert!(warnings.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn structure_batch_surfaces_alias_warnings_in_summary() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("structure_alias_warning.xlsx", |_| {});

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

    let preview_fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id.clone(),
        },
    )
    .await?;
    let preview_params: StructureBatchParamsInput = serde_json::from_value(json!({
        "fork_id": preview_fork.fork_id,
        "ops": [{ "op": "add_sheet", "name": "Inputs" }],
        "mode": "preview"
    }))
    .unwrap();
    let preview = structure_batch(state.clone(), preview_params).await?;

    assert!(
        preview
            .summary
            .warnings
            .iter()
            .any(|w| w == "WARN_ALIAS_KIND: Normalized structure op alias to canonical kind")
    );

    let apply_fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;
    let apply_params: StructureBatchParamsInput = serde_json::from_value(json!({
        "fork_id": apply_fork.fork_id,
        "ops": [{ "op": "add_sheet", "name": "Inputs" }],
        "mode": "apply"
    }))
    .unwrap();
    let apply = structure_batch(state.clone(), apply_params).await?;

    assert!(
        apply
            .summary
            .warnings
            .iter()
            .any(|w| w == "WARN_ALIAS_KIND: Normalized structure op alias to canonical kind")
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Ticket 4104 – expand_adjacent_sums + clone_row
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn insert_rows_expand_adjacent_sums_single_row() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("expand_sum_single.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        // Rows 1-3: detail data
        sheet.get_cell_mut("A1").set_value_number(10);
        sheet.get_cell_mut("A2").set_value_number(20);
        sheet.get_cell_mut("A3").set_value_number(30);
        // Row 4: subtotal SUM adjacent to detail rows
        sheet.get_cell_mut("A4").set_formula("SUM(A1:A3)");
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

    // Insert 1 row at row 4 (between detail and subtotal), with expand_adjacent_sums
    let resp = structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::InsertRows {
                    sheet_name: "Sheet1".to_string(),
                    at_row: 4,
                    count: 1,
                    expand_adjacent_sums: true,
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert!(resp.summary.counts.contains_key("sums_expanded"));

    // Subtotal is now at row 5 (shifted from 4). Formula should be SUM(A1:A4).
    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let formula = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.get_cell("A5").unwrap().get_formula().to_string()
    })?;
    assert_eq!(formula.to_uppercase().replace(' ', ""), "SUM(A1:A4)");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn insert_rows_expand_adjacent_sums_multi_row() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("expand_sum_multi.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("B1").set_value_number(1);
        sheet.get_cell_mut("B2").set_value_number(2);
        sheet.get_cell_mut("B3").set_value_number(3);
        sheet.get_cell_mut("B4").set_formula("SUM(B1:B3)");
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

    // Insert 3 rows at row 4 with expansion
    structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::InsertRows {
                    sheet_name: "Sheet1".to_string(),
                    at_row: 4,
                    count: 3,
                    expand_adjacent_sums: true,
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    // Subtotal now at row 7. Formula should be SUM(B1:B6).
    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let formula = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.get_cell("B7").unwrap().get_formula().to_string()
    })?;
    assert_eq!(formula.to_uppercase().replace(' ', ""), "SUM(B1:B6)");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn insert_rows_expand_adjacent_sums_counts_all_expanded_formulas() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("expand_sum_count.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(1);
        sheet.get_cell_mut("A2").set_value_number(2);
        sheet.get_cell_mut("A3").set_value_number(3);
        sheet.get_cell_mut("B1").set_value_number(10);
        sheet.get_cell_mut("B2").set_value_number(20);
        sheet.get_cell_mut("B3").set_value_number(30);
        sheet.get_cell_mut("A4").set_formula("SUM(A1:A3)");
        sheet.get_cell_mut("B4").set_formula("SUM(B1:B3)");
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

    let resp = structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::InsertRows {
                    sheet_name: "Sheet1".to_string(),
                    at_row: 4,
                    count: 2,
                    expand_adjacent_sums: true,
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(resp.summary.counts.get("sums_expanded").copied(), Some(2));

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let (a6, b6) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a6 = sheet
            .get_cell("A6")
            .unwrap()
            .get_formula()
            .to_string()
            .to_uppercase()
            .replace(' ', "");
        let b6 = sheet
            .get_cell("B6")
            .unwrap()
            .get_formula()
            .to_string()
            .to_uppercase()
            .replace(' ', "");
        (a6, b6)
    })?;
    assert_eq!(a6, "SUM(A1:A5)");
    assert_eq!(b6, "SUM(B1:B5)");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn insert_rows_no_expansion_when_flag_absent() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("no_expand.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(10);
        sheet.get_cell_mut("A2").set_value_number(20);
        sheet.get_cell_mut("A3").set_formula("SUM(A1:A2)");
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

    // Insert without expand_adjacent_sums (default false)
    structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::InsertRows {
                    sheet_name: "Sheet1".to_string(),
                    at_row: 3,
                    count: 1,
                    expand_adjacent_sums: false,
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    // Subtotal now at row 4 with formula rewritten to SUM(A1:A2) (no expansion).
    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let formula = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.get_cell("A4").unwrap().get_formula().to_string()
    })?;
    // Formula rewriter shifts row refs >= at_row, but the range SUM(A1:A2) has
    // row2=2 which is < at_row=3, so it stays SUM(A1:A2).
    assert_eq!(formula.to_uppercase().replace(' ', ""), "SUM(A1:A2)");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn insert_rows_ambiguous_formula_produces_warning() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("ambiguous_sum.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(10);
        sheet.get_cell_mut("A2").set_value_number(20);
        // Complex formula – not a simple SUM(Ax:Ay)
        sheet
            .get_cell_mut("A3")
            .set_formula("SUM(A1:A2)+SUM(B1:B2)");
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

    let resp = structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::InsertRows {
                    sheet_name: "Sheet1".to_string(),
                    at_row: 3,
                    count: 1,
                    expand_adjacent_sums: true,
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    // Should have a warning about the skipped complex formula.
    assert!(
        resp.summary
            .warnings
            .iter()
            .any(|w| w.contains("WARN_SUM_EXPANSION_SKIPPED")),
        "expected ambiguous SUM warning, got: {:?}",
        resp.summary.warnings
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn clone_row_copies_template_and_expands_sums() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("clone_row.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        // Row 1: header
        sheet.get_cell_mut("A1").set_value("Label");
        sheet.get_cell_mut("B1").set_value("Amount");
        // Row 2: template detail row
        sheet.get_cell_mut("A2").set_value("Item");
        sheet.get_cell_mut("B2").set_value_number(100);
        // Row 3: subtotal
        sheet.get_cell_mut("A3").set_value("Total");
        sheet.get_cell_mut("B3").set_formula("SUM(B2:B2)");
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

    let resp = structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::CloneRow {
                    sheet_name: "Sheet1".to_string(),
                    source_row: 2,
                    insert_at: 3,
                    count: 2,
                    expand_adjacent_sums: true,
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert!(resp.summary.counts.contains_key("rows_cloned"));
    assert_eq!(*resp.summary.counts.get("rows_cloned").unwrap(), 2);

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;

    let (a3, b3_val, a4, b4_val, a5, b5_formula) = fork_wb.with_sheet("Sheet1", |sheet| {
        let a3 = sheet
            .get_cell("A3")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default();
        let b3 = sheet
            .get_cell("B3")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default();
        let a4 = sheet
            .get_cell("A4")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default();
        let b4 = sheet
            .get_cell("B4")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default();
        let a5 = sheet
            .get_cell("A5")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default();
        let b5_formula = sheet
            .get_cell("B5")
            .map(|c| c.get_formula().to_string())
            .unwrap_or_default();
        (a3, b3, a4, b4, a5, b5_formula)
    })?;

    // Cloned rows at 3 and 4 should have the template's value.
    assert_eq!(a3, "Item");
    assert_eq!(b3_val, "100");
    assert_eq!(a4, "Item");
    assert_eq!(b4_val, "100");
    // Subtotal moved to row 5 and its SUM should now span B2:B4.
    assert_eq!(a5, "Total");
    assert_eq!(b5_formula.to_uppercase().replace(' ', ""), "SUM(B2:B4)");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn clone_row_source_below_insert_point_shifts_formula_to_new_row() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("clone_row_source_below.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A5").set_value_number(7);
        sheet.get_cell_mut("B5").set_formula("A5+1");
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

    structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::CloneRow {
                    sheet_name: "Sheet1".to_string(),
                    source_row: 5,
                    insert_at: 2,
                    count: 1,
                    expand_adjacent_sums: false,
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let cloned_formula = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet
            .get_cell("B2")
            .unwrap()
            .get_formula()
            .to_string()
            .to_uppercase()
            .replace(' ', "")
    })?;
    assert_eq!(cloned_formula, "A2+1");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn clone_row_without_expansion_keeps_original_sum() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("clone_row_no_expand.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(10);
        sheet.get_cell_mut("A2").set_formula("SUM(A1:A1)");
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

    structure_batch(
        state.clone(),
        StructureBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            ops: vec![
                StructureOp::CloneRow {
                    sheet_name: "Sheet1".to_string(),
                    source_row: 1,
                    insert_at: 2,
                    count: 1,
                    expand_adjacent_sums: false,
                }
                .into(),
            ],
            mode: Some(BatchMode::Apply),
            label: None,
            impact_report: None,
            show_formula_delta: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    // Subtotal moved from row 2 to row 3. Without expansion, the rewritten SUM
    // stays SUM(A1:A1) (no change since row1=1 < at_row=2).
    let fork_wb = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    let formula = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet.get_cell("A3").unwrap().get_formula().to_string()
    })?;
    assert_eq!(formula.to_uppercase().replace(' ', ""), "SUM(A1:A1)");

    Ok(())
}

#[test]
fn structure_batch_deserialize_expand_adjacent_sums_default() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            { "kind": "insert_rows", "sheet_name": "Sheet1", "at_row": 2, "count": 1 }
        ]
    });
    let params: StructureBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, _) = normalize_structure_batch(params).unwrap();
    match &normalized.ops[0] {
        StructureOp::InsertRows {
            expand_adjacent_sums,
            ..
        } => {
            assert!(!expand_adjacent_sums, "default should be false");
        }
        _ => panic!("expected InsertRows"),
    }
}

#[test]
fn structure_batch_deserialize_clone_row() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            {
                "kind": "clone_row",
                "sheet_name": "Sheet1",
                "source_row": 5,
                "insert_at": 6,
                "count": 2,
                "expand_adjacent_sums": true
            }
        ]
    });
    let params: StructureBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, _) = normalize_structure_batch(params).unwrap();
    match &normalized.ops[0] {
        StructureOp::CloneRow {
            sheet_name,
            source_row,
            insert_at,
            count,
            expand_adjacent_sums,
        } => {
            assert_eq!(sheet_name, "Sheet1");
            assert_eq!(*source_row, 5);
            assert_eq!(*insert_at, 6);
            assert_eq!(*count, 2);
            assert!(*expand_adjacent_sums);
        }
        _ => panic!("expected CloneRow"),
    }
}

#[test]
fn structure_batch_deserialize_clone_row_default_count() {
    let input = json!({
        "fork_id": "f1",
        "ops": [
            {
                "kind": "clone_row",
                "sheet_name": "Sheet1",
                "source_row": 2,
                "insert_at": 3
            }
        ]
    });
    let params: StructureBatchParamsInput = serde_json::from_value(input).unwrap();
    let (normalized, _) = normalize_structure_batch(params).unwrap();
    match &normalized.ops[0] {
        StructureOp::CloneRow { count, .. } => {
            assert_eq!(*count, 1, "default count should be 1");
        }
        _ => panic!("expected CloneRow"),
    }
}
