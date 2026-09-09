//! Integration tests for fork workflow: create_fork → edit_batch → get_edits → get_changeset → save_fork
//!
//! These tests exercise the full fork workflow without requiring LibreOffice for recalculation.

#![cfg(feature = "recalc")]

use std::sync::Arc;

use agent_spreadsheet_mcp::ServerConfig;
use agent_spreadsheet_mcp::diff::Change; // Add Change import
use agent_spreadsheet_mcp::diff::merge::{CellDiff, ModificationType};
use agent_spreadsheet_mcp::model::WorkbookId;
use agent_spreadsheet_mcp::state::AppState;
use agent_spreadsheet_mcp::tools::fork::{
    CreateForkParams, DiscardForkParams, GetChangesetParams, GetEditsParams, ListForksParams,
    SaveForkParams, TransformBatchParams, TransformOp, TransformTarget, create_fork, discard_fork,
    edit_batch, get_changeset, get_edits, list_forks, save_fork, transform_batch,
};
use agent_spreadsheet_mcp::tools::write_normalize::{
    CellEditInput, CellEditV2, EditBatchParamsInput,
};
use agent_spreadsheet_mcp::tools::{ListWorkbooksParams, list_workbooks};
use anyhow::Result;

#[path = "./support/mod.rs"]
mod support;

fn recalc_enabled_config(workspace: &support::TestWorkspace) -> ServerConfig {
    workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
    })
}

fn app_state_with_recalc(workspace: &support::TestWorkspace) -> Arc<AppState> {
    let config = Arc::new(recalc_enabled_config(workspace));
    Arc::new(AppState::new(config))
}

fn input_edit(address: &str, value: &str, is_formula: bool) -> CellEditInput {
    CellEditInput::Object(CellEditV2 {
        address: address.to_string(),
        value: Some(value.to_string()),
        formula: None,
        is_formula: Some(is_formula),
    })
}

async fn discover_workbook(state: Arc<AppState>) -> Result<WorkbookId> {
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
    assert_eq!(list.workbooks.len(), 1, "expected exactly 1 workbook");
    Ok(list.workbooks[0].workbook_id.clone())
}

#[tokio::test]
async fn test_create_fork_basic() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("source.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(100);
        sheet.get_cell_mut("B1").set_formula("A1*2");
    });

    let config = Arc::new(workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
        cfg.allow_overwrite = true;
    }));
    let state = Arc::new(AppState::new(config));
    let workbook_id = discover_workbook(state.clone()).await?;

    let response = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    assert!(!response.fork_id.is_empty());
    assert!(response.base_workbook.contains("source.xlsx"));
    assert_eq!(response.ttl_seconds, 0);

    Ok(())
}

#[tokio::test]
async fn test_create_fork_rejects_non_xlsx() -> Result<()> {
    let workspace = support::TestWorkspace::new();

    support::touch_file(&workspace.path("data.xls"));

    let config = Arc::new(workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
        cfg.allow_overwrite = true;
    }));
    let state = Arc::new(AppState::new(config));

    let list = list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: Some("*.xls".to_string()),
            limit: None,
            offset: None,
            include_paths: None,
        },
    )
    .await?;

    if list.workbooks.is_empty() {
        return Ok(());
    }

    let result = create_fork(
        state,
        CreateForkParams {
            workbook_or_fork_id: list.workbooks[0].workbook_id.clone(),
        },
    )
    .await;

    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_edit_batch_applies_values() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("editable.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value_number(10);
        sheet.get_cell_mut("A2").set_value_number(20);
    });

    let state = app_state_with_recalc(&workspace);
    let workbook_id = discover_workbook(state.clone()).await?;

    let fork_response = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let edit_response = edit_batch(
        state.clone(),
        EditBatchParamsInput {
            fork_id: fork_response.fork_id.clone(),
            sheet_name: "Data".to_string(),
            edits: vec![
                CellEditInput::Shorthand("A1=100".to_string()),
                CellEditInput::Shorthand("B2==SUM(A1:A2)".to_string()),
                input_edit("A3", "SUM(A1:A2)", true),
            ],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(edit_response.edits_applied, Some(3));
    assert_eq!(edit_response.total_edits, 3);
    // Successful shorthand edits must not emit noise warnings (they trained
    // agents to ignore warnings entirely).
    assert!(
        edit_response
            .warnings
            .iter()
            .all(|warning| !warning.code.starts_with("WARN_SHORTHAND")
                && warning.code != "WARN_FORMULA_PREFIX")
    );

    Ok(())
}

#[tokio::test]
async fn test_edit_batch_clears_cached_value_on_formula() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("formula_cache.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value("stale");
    });

    let state = app_state_with_recalc(&workspace);
    let workbook_id = discover_workbook(state.clone()).await?;

    let fork_response = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    edit_batch(
        state.clone(),
        EditBatchParamsInput {
            fork_id: fork_response.fork_id.clone(),
            sheet_name: "Data".to_string(),
            edits: vec![input_edit("A1", "A1*2", true)],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    let registry = state
        .fork_registry()
        .expect("fork registry should be available");
    let fork_path = registry
        .get_fork_path(&fork_response.fork_id)
        .expect("fork path");
    let book = umya_spreadsheet::reader::xlsx::read(&fork_path)?;
    let sheet = book
        .get_sheet_by_name("Data").ok()
        .expect("Data sheet should exist");
    let cell = sheet.get_cell("A1").expect("A1 should exist");

    assert!(cell.is_formula());
    assert_eq!(cell.get_formula(), "A1*2");
    assert!(cell.get_cell_value().get_raw_value().is_empty());
    assert_eq!(cell.get_value(), "");

    Ok(())
}

#[tokio::test]
async fn test_get_edits_returns_history() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("history.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Sheet1");
        sheet.get_cell_mut("A1").set_value_number(1);
    });

    let state = app_state_with_recalc(&workspace);
    let workbook_id = discover_workbook(state.clone()).await?;

    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    // Apply multiple batches
    edit_batch(
        state.clone(),
        EditBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            edits: vec![input_edit("A1", "10", false)],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    edit_batch(
        state.clone(),
        EditBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            edits: vec![input_edit("B1", "A1*2", true)],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    let edits = get_edits(
        state.clone(),
        GetEditsParams {
            fork_id: fork.fork_id.clone(),
        },
    )
    .await?;

    assert_eq!(edits.edits.len(), 2);

    let a1_edit = &edits.edits[0];
    assert_eq!(a1_edit.address, "A1");
    assert_eq!(a1_edit.value, "10");
    assert!(!a1_edit.is_formula);

    let b1_edit = &edits.edits[1];
    assert_eq!(b1_edit.address, "B1");
    assert_eq!(b1_edit.value, "A1*2");
    assert!(b1_edit.is_formula);

    Ok(())
}

#[tokio::test]
async fn test_get_changeset_detects_modifications() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("changeset.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Sheet1");
        sheet.get_cell_mut("A1").set_value_number(100);
        sheet.get_cell_mut("A2").set_value("original");
    });

    let state = app_state_with_recalc(&workspace);
    let workbook_id = discover_workbook(state.clone()).await?;

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
            edits: vec![
                input_edit("A1", "200", false),
                input_edit("A2", "modified", false),
            ],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    let changeset = get_changeset(
        state.clone(),
        GetChangesetParams {
            fork_id: fork.fork_id.clone(),
            sheet_name: None,
            ..Default::default()
        },
    )
    .await?;

    assert_eq!(changeset.changes.len(), 2);

    // Find A1 change
    let a1_change = changeset
        .changes
        .iter()
        .find(|c| {
            matches!(c, Change::Cell(cell) if matches!(&cell.diff, agent_spreadsheet_mcp::diff::merge::CellDiff::Modified { address, .. } if address == "A1"))
        })
        .expect("A1 change not found");

    if let Change::Cell(c) = a1_change {
        match &c.diff {
            agent_spreadsheet_mcp::diff::merge::CellDiff::Modified {
                subtype,
                old_value,
                new_value,
                ..
            } => {
                assert!(matches!(subtype, ModificationType::ValueEdit));
                assert_eq!(old_value.as_deref(), Some("100"));
                assert_eq!(new_value.as_deref(), Some("200"));
            }
            _ => panic!("Expected Modified diff"),
        }
    } else {
        panic!("Expected cell change");
    }

    Ok(())
}

#[tokio::test]
async fn test_get_changeset_with_sheet_filter() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("multi_sheet.xlsx", |book| {
        let sheet1 = book.get_sheet_mut(&0).ok().unwrap();
        sheet1.set_name("Sheet1");
        sheet1.get_cell_mut("A1").set_value_number(1);

        let sheet2 = book.new_sheet("Sheet2").unwrap();
        sheet2.get_cell_mut("A1").set_value_number(2);
    });

    let state = app_state_with_recalc(&workspace);
    let workbook_id = discover_workbook(state.clone()).await?;

    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    // Edit both sheets
    edit_batch(
        state.clone(),
        EditBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            edits: vec![input_edit("A1", "10", false)],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    edit_batch(
        state.clone(),
        EditBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            sheet_name: "Sheet2".to_string(),
            edits: vec![input_edit("A1", "20", false)],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    // Get changeset filtered to Sheet1 only
    let changeset = get_changeset(
        state.clone(),
        GetChangesetParams {
            fork_id: fork.fork_id.clone(),
            sheet_name: Some("Sheet1".to_string()),
            ..Default::default()
        },
    )
    .await?;

    assert_eq!(changeset.changes.len(), 1);

    if let Change::Cell(c) = &changeset.changes[0] {
        assert_eq!(c.sheet, "Sheet1");
    } else {
        panic!("Expected cell change");
    }

    Ok(())
}

#[tokio::test]
async fn test_list_forks() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("listable.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(1);
    });

    let state = app_state_with_recalc(&workspace);
    let workbook_id = discover_workbook(state.clone()).await?;

    let list = list_forks(state.clone(), ListForksParams {}).await?;
    assert!(list.forks.is_empty());

    let fork1 = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id.clone(),
        },
    )
    .await?;

    let fork2 = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let list = list_forks(state.clone(), ListForksParams {}).await?;
    assert_eq!(list.forks.len(), 2);

    let fork_ids: Vec<_> = list.forks.iter().map(|f| f.fork_id.as_str()).collect();
    assert!(fork_ids.contains(&fork1.fork_id.as_str()));
    assert!(fork_ids.contains(&fork2.fork_id.as_str()));

    Ok(())
}

#[tokio::test]
async fn test_discard_fork() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("discardable.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(1);
    });

    let state = app_state_with_recalc(&workspace);
    let workbook_id = discover_workbook(state.clone()).await?;

    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    // Verify fork exists
    let list = list_forks(state.clone(), ListForksParams {}).await?;
    assert_eq!(list.forks.len(), 1);

    // Discard it
    let discard_response = discard_fork(
        state.clone(),
        DiscardForkParams {
            fork_id: fork.fork_id.clone(),
        },
    )
    .await?;

    assert!(discard_response.discarded);
    assert_eq!(discard_response.fork_id, fork.fork_id);

    // Verify fork is gone
    let list = list_forks(state.clone(), ListForksParams {}).await?;
    assert!(list.forks.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_save_fork_overwrites_original() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("saveable.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value_number(1);
    });

    let config = Arc::new(workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
        cfg.allow_overwrite = true;
    }));
    let state = Arc::new(AppState::new(config));
    let workbook_id = discover_workbook(state.clone()).await?;

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
            sheet_name: "Data".to_string(),
            edits: vec![input_edit("A1", "999", false)],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    let save_response = save_fork(
        state.clone(),
        SaveForkParams {
            fork_id: fork.fork_id.clone(),
            target_path: None, // Overwrite original
            drop_fork: true,
        },
    )
    .await?;

    assert!(save_response.saved_to.contains("saveable.xlsx"));

    // Verify the original file was updated
    let book = umya_spreadsheet::reader::xlsx::read(&path)?;
    let sheet = book.get_sheet_by_name("Data").ok().unwrap();
    let value = sheet.get_cell("A1").unwrap().get_value();
    assert_eq!(value, "999");

    // Fork should be removed after save
    let list = list_forks(state.clone(), ListForksParams {}).await?;
    assert!(list.forks.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_save_fork_to_new_path() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _original = workspace.create_workbook("original.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value_number(1);
    });

    let state = app_state_with_recalc(&workspace);
    let workbook_id = discover_workbook(state.clone()).await?;

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
            sheet_name: "Data".to_string(),
            edits: vec![input_edit("A1", "modified", false)],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    let save_response = save_fork(
        state.clone(),
        SaveForkParams {
            fork_id: fork.fork_id.clone(),
            target_path: Some("copy.xlsx".to_string()),
            drop_fork: true,
        },
    )
    .await?;

    assert!(save_response.saved_to.contains("copy.xlsx"));

    // Verify original is unchanged
    let original_book = umya_spreadsheet::reader::xlsx::read(workspace.path("original.xlsx"))?;
    let original_value = original_book
        .get_sheet_by_name("Data").ok()
        .unwrap()
        .get_cell("A1")
        .unwrap()
        .get_value();
    assert_eq!(original_value, "1");

    // Verify copy has changes
    let copy_book = umya_spreadsheet::reader::xlsx::read(workspace.path("copy.xlsx"))?;
    let copy_value = copy_book
        .get_sheet_by_name("Data").ok()
        .unwrap()
        .get_cell("A1")
        .unwrap()
        .get_value();
    assert_eq!(copy_value, "modified");

    Ok(())
}

#[tokio::test]
async fn test_full_workflow_without_recalc() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("workflow.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Budget");
        sheet.get_cell_mut("A1").set_value("Item");
        sheet.get_cell_mut("B1").set_value("Amount");
        sheet.get_cell_mut("A2").set_value("Rent");
        sheet.get_cell_mut("B2").set_value_number(1000);
        sheet.get_cell_mut("A3").set_value("Food");
        sheet.get_cell_mut("B3").set_value_number(500);
        sheet.get_cell_mut("A4").set_value("Total");
        let cell = sheet.get_cell_mut("B4");
        cell.set_formula("SUM(B2:B3)");
        cell.set_formula_result_default("1500");
    });

    let state = app_state_with_recalc(&workspace);
    let workbook_id = discover_workbook(state.clone()).await?;

    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;
    assert!(!fork.fork_id.is_empty());

    // Step 2: Apply edits (update rent amount)
    let edit_result = edit_batch(
        state.clone(),
        EditBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            sheet_name: "Budget".to_string(),
            edits: vec![input_edit("B2", "1200", false)],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await?;
    assert_eq!(edit_result.edits_applied, Some(1));

    // Step 3: Review edits
    let edits = get_edits(
        state.clone(),
        GetEditsParams {
            fork_id: fork.fork_id.clone(),
        },
    )
    .await?;
    assert_eq!(edits.edits.len(), 1);
    assert_eq!(edits.edits[0].address, "B2");
    assert_eq!(edits.edits[0].value, "1200");

    // Step 4: Get changeset (without recalc - just value change)
    let changeset = get_changeset(
        state.clone(),
        GetChangesetParams {
            fork_id: fork.fork_id.clone(),
            sheet_name: None,
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(changeset.changes.len(), 1);

    // Step 5: Save fork
    let save_result = save_fork(
        state.clone(),
        SaveForkParams {
            fork_id: fork.fork_id.clone(),
            target_path: Some("workflow_updated.xlsx".to_string()),
            drop_fork: true,
        },
    )
    .await?;
    assert!(save_result.saved_to.contains("workflow_updated.xlsx"));

    // Verify the saved file
    let saved_book = umya_spreadsheet::reader::xlsx::read(workspace.path("workflow_updated.xlsx"))?;
    let saved_value = saved_book
        .get_sheet_by_name("Budget").ok()
        .unwrap()
        .get_cell("B2")
        .unwrap()
        .get_value();
    assert_eq!(saved_value, "1200");

    Ok(())
}

#[tokio::test]
async fn test_fork_not_found_error() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("dummy.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(1);
    });

    let state = app_state_with_recalc(&workspace);

    let result = get_edits(
        state.clone(),
        GetEditsParams {
            fork_id: "nonexistent-fork-id".to_string(),
        },
    )
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not found"),
        "error should mention not found: {}",
        err
    );

    Ok(())
}

#[tokio::test]
async fn test_edit_nonexistent_sheet_error() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("single_sheet.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("RealSheet");
        sheet.get_cell_mut("A1").set_value_number(1);
    });

    let state = app_state_with_recalc(&workspace);
    let workbook_id = discover_workbook(state.clone()).await?;

    let fork = create_fork(
        state.clone(),
        CreateForkParams {
            workbook_or_fork_id: workbook_id,
        },
    )
    .await?;

    let result = edit_batch(
        state.clone(),
        EditBatchParamsInput {
            fork_id: fork.fork_id.clone(),
            sheet_name: "FakeSheet".to_string(),
            edits: vec![input_edit("A1", "test", false)],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not found") || err.contains("FakeSheet"),
        "error should mention sheet not found: {}",
        err
    );

    Ok(())
}

fn first_cell_address(changes: &[Change]) -> Option<&str> {
    for change in changes {
        if let Change::Cell(cell) = change {
            match &cell.diff {
                CellDiff::Added { address, .. }
                | CellDiff::Deleted { address, .. }
                | CellDiff::Modified { address, .. } => {
                    return Some(address.as_str());
                }
            }
        }
    }
    None
}

#[tokio::test]
async fn test_get_changeset_paging_limit_offset_and_summary_only() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("changeset_paging.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Sheet1");
    });

    let state = app_state_with_recalc(&workspace);
    let workbook_id = discover_workbook(state.clone()).await?;

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
                target: TransformTarget::Range {
                    range: "A1:T20".to_string(),
                },
                value: "x".to_string(),
                is_formula: false,
                overwrite_formulas: false,
            }],
            mode: Some(agent_spreadsheet_mcp::tools::param_enums::BatchMode::Apply),
            label: None,

            formula_parse_policy: None,
        },
    )
    .await?;

    let page1 = get_changeset(
        state.clone(),
        GetChangesetParams {
            fork_id: fork.fork_id.clone(),
            sheet_name: Some("Sheet1".to_string()),
            limit: 5,
            offset: 0,
            ..Default::default()
        },
    )
    .await?;

    assert_eq!(page1.changes.len(), 5);
    assert!(page1.summary.total_changes > 5);
    assert!(page1.summary.truncated);
    assert_eq!(page1.summary.next_offset, Some(5));
    assert_eq!(page1.summary.returned_changes, 5);

    let page2 = get_changeset(
        state.clone(),
        GetChangesetParams {
            fork_id: fork.fork_id.clone(),
            sheet_name: Some("Sheet1".to_string()),
            limit: 5,
            offset: page1.summary.next_offset.unwrap(),
            ..Default::default()
        },
    )
    .await?;

    assert_eq!(page2.changes.len(), 5);
    assert_ne!(
        first_cell_address(&page1.changes),
        first_cell_address(&page2.changes)
    );

    let summary_only = get_changeset(
        state.clone(),
        GetChangesetParams {
            fork_id: fork.fork_id.clone(),
            sheet_name: Some("Sheet1".to_string()),
            summary_only: true,
            ..Default::default()
        },
    )
    .await?;

    assert!(summary_only.changes.is_empty());
    assert_eq!(summary_only.summary.returned_changes, 0);
    assert_eq!(
        summary_only.summary.total_changes,
        page1.summary.total_changes
    );

    Ok(())
}

#[tokio::test]
async fn test_get_changeset_exclude_recalc_result() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("changeset_filter.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Sheet1");
        sheet.get_cell_mut("A1").set_value_number(1);
        sheet
            .get_cell_mut("B1")
            .set_formula("A1*2")
            .set_formula_result_default("2");
    });

    let state = app_state_with_recalc(&workspace);
    let workbook_id = discover_workbook(state.clone()).await?;

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
            edits: vec![input_edit("A1", "2", false)],

            mode: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    let registry = state.fork_registry().expect("fork registry");
    let fork_ctx = registry.get_fork(&fork.fork_id)?;
    let work_path = fork_ctx.work_path.clone();

    let mut book = umya_spreadsheet::reader::xlsx::read(&work_path)?;
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.get_cell_mut("B1").set_formula_result_default("999");
    umya_spreadsheet::writer::xlsx::write(&book, &work_path)?;

    let unfiltered = get_changeset(
        state.clone(),
        GetChangesetParams {
            fork_id: fork.fork_id.clone(),
            sheet_name: Some("Sheet1".to_string()),
            ..Default::default()
        },
    )
    .await?;

    let has_recalc_result = unfiltered.changes.iter().any(|c| match c {
        Change::Cell(cell) => matches!(
            &cell.diff,
            CellDiff::Modified {
                address,
                subtype: ModificationType::RecalcResult,
                ..
            } if address == "B1"
        ),
        _ => false,
    });
    assert!(has_recalc_result);

    let filtered = get_changeset(
        state.clone(),
        GetChangesetParams {
            fork_id: fork.fork_id.clone(),
            sheet_name: Some("Sheet1".to_string()),
            exclude_subtypes: Some(vec!["recalc_result".to_string()]),
            ..Default::default()
        },
    )
    .await?;

    let filtered_has_recalc_result = filtered.changes.iter().any(|c| match c {
        Change::Cell(cell) => matches!(
            &cell.diff,
            CellDiff::Modified {
                subtype: ModificationType::RecalcResult,
                ..
            }
        ),
        _ => false,
    });
    assert!(!filtered_has_recalc_result);

    let filtered_has_a1 = filtered.changes.iter().any(|c| match c {
        Change::Cell(cell) => matches!(
            &cell.diff,
            CellDiff::Modified {
                address,
                subtype: ModificationType::ValueEdit,
                ..
            } if address == "A1"
        ),
        _ => false,
    });
    assert!(filtered_has_a1);

    Ok(())
}
