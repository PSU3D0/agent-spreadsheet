#![cfg(feature = "recalc")]

use anyhow::Result;
use agent_spreadsheet_mcp::model::WorkbookId;
use agent_spreadsheet_mcp::tools::ListWorkbooksParams;
use agent_spreadsheet_mcp::tools::fork::{
    ApplyStagedChangeParams, CreateForkParams, apply_staged_change, create_fork,
};
use agent_spreadsheet_mcp::tools::list_workbooks;
use agent_spreadsheet_mcp::tools::param_enums::BatchMode;
use agent_spreadsheet_mcp::tools::sheet_layout::{
    SheetLayoutBatchParams, SheetLayoutOp, sheet_layout_batch,
};
use umya_spreadsheet::EnumTrait;

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
async fn sheet_layout_freeze_panes_persists_and_infers_top_left() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("layout_freeze.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("hdr");
        sheet.get_cell_mut("A2").set_value("x");
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

    sheet_layout_batch(
        state.clone(),
        SheetLayoutBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![SheetLayoutOp::FreezePanes {
                sheet_name: "Sheet1".to_string(),
                freeze_rows: 1,
                freeze_cols: 1,
                top_left_cell: None,
            }],
            mode: Some(BatchMode::Apply),
            label: None,
        },
    )
    .await?;

    let work_path = state
        .fork_registry()
        .unwrap()
        .get_fork(&fork.fork_id)?
        .work_path
        .clone();
    let book = umya_spreadsheet::reader::xlsx::read(&work_path)?;
    let sheet = book.get_sheet_by_name("Sheet1").ok().unwrap();
    let views = sheet.get_sheets_views().get_sheet_view_list();
    assert!(!views.is_empty());
    let pane = views[0].get_pane().expect("pane");

    assert_eq!(pane.get_horizontal_split(), 1.0);
    assert_eq!(pane.get_vertical_split(), 1.0);
    assert_eq!(pane.get_state().value_string(), "frozen");
    assert_eq!(pane.get_top_left_cell().to_string(), "B2");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_layout_print_area_defined_name_written_and_scoped() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("layout_print_area.xlsx", |_| {});

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

    sheet_layout_batch(
        state.clone(),
        SheetLayoutBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![SheetLayoutOp::SetPrintArea {
                sheet_name: "Sheet1".to_string(),
                range: "A1:G30".to_string(),
            }],
            mode: Some(BatchMode::Apply),
            label: None,
        },
    )
    .await?;

    let work_path = state
        .fork_registry()
        .unwrap()
        .get_fork(&fork.fork_id)?
        .work_path
        .clone();
    let book = umya_spreadsheet::reader::xlsx::read(&work_path)?;
    let sheet = book.get_sheet_by_name("Sheet1").ok().unwrap();

    let print_names: Vec<_> = sheet
        .get_defined_names()
        .iter()
        .filter(|d| d.get_name() == "_xlnm.Print_Area")
        .collect();
    assert_eq!(print_names.len(), 1);
    let dn = print_names[0];
    assert!(dn.has_local_sheet_id());
    assert_eq!(dn.get_local_sheet_id(), 0);
    // umya may normalize sheet names with quotes.
    assert_eq!(dn.get_address(), "Sheet1!$A$1:$G$30");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_layout_preview_then_apply_staged_change() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("layout_preview.xlsx", |book| {
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

    let preview = sheet_layout_batch(
        state.clone(),
        SheetLayoutBatchParams {
            fork_id: fork.fork_id.clone(),
            ops: vec![SheetLayoutOp::FreezePanes {
                sheet_name: "Sheet1".to_string(),
                freeze_rows: 1,
                freeze_cols: 0,
                top_left_cell: None,
            }],
            mode: Some(BatchMode::Preview),
            label: Some("freeze headers".to_string()),
        },
    )
    .await?;
    let change_id = preview.change_id.clone().expect("change_id");

    // Preview should NOT modify the fork workbook.
    let work_path = state
        .fork_registry()
        .unwrap()
        .get_fork(&fork.fork_id)?
        .work_path
        .clone();
    let book_before = umya_spreadsheet::reader::xlsx::read(&work_path)?;
    let sheet_before = book_before.get_sheet_by_name("Sheet1").ok().unwrap();
    let views_before = sheet_before.get_sheets_views().get_sheet_view_list();
    let pane_before = views_before.first().and_then(|v| v.get_pane());
    assert!(pane_before.is_none());

    apply_staged_change(
        state.clone(),
        ApplyStagedChangeParams {
            fork_id: fork.fork_id.clone(),
            change_id,
        },
    )
    .await?;

    let book_after = umya_spreadsheet::reader::xlsx::read(&work_path)?;
    let sheet_after = book_after.get_sheet_by_name("Sheet1").ok().unwrap();
    let views_after = sheet_after.get_sheets_views().get_sheet_view_list();
    let pane_after = views_after[0].get_pane().expect("pane after apply");
    assert_eq!(pane_after.get_state().value_string(), "frozen");
    assert_eq!(pane_after.get_top_left_cell().to_string(), "A2");

    // Also confirm via AppState cache that the fork can be opened post-apply.
    let _ = state
        .open_workbook(&WorkbookId(fork.fork_id.clone()))
        .await?;
    Ok(())
}
