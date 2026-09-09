#![cfg(feature = "recalc")]

use agent_spreadsheet_mcp::SpreadsheetServer;
use agent_spreadsheet_mcp::model::WorkbookId;
use agent_spreadsheet_mcp::tools::fork::{
    ApplyStagedChangeParams, CreateForkParams, ListStagedChangesParams, apply_staged_change,
    create_fork, list_staged_changes,
};
use agent_spreadsheet_mcp::tools::mutate_batch::{MutateBatchParams, MutateOpInput, mutate_batch};
use agent_spreadsheet_mcp::tools::param_enums::BatchMode;
use agent_spreadsheet_mcp::tools::write_normalize::{CellEditInput, EditBatchParamsInput};
use agent_spreadsheet_mcp::tools::{ListWorkbooksParams, list_workbooks};
use anyhow::Result;
use serde_json::json;

mod support;

fn recalc_state(
    workspace: &support::TestWorkspace,
) -> std::sync::Arc<agent_spreadsheet_mcp::state::AppState> {
    let config = workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
    });
    support::app_state_with_config(config)
}

async fn forked_workbook(
    state: &std::sync::Arc<agent_spreadsheet_mcp::state::AppState>,
) -> Result<String> {
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
    Ok(fork.fork_id)
}

fn ops(values: Vec<serde_json::Value>) -> Vec<MutateOpInput> {
    values.into_iter().map(MutateOpInput).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn mutate_batch_apply_routes_one_op_from_each_family() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("mutate_all_families.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(1);
        sheet.get_cell_mut("A2").set_value_number(2);
        sheet
            .get_cell_mut("E1")
            .set_formula("SUM(A1:A2)".to_string());
    });

    let state = recalc_state(&workspace);
    let fork_id = forked_workbook(&state).await?;

    let response = mutate_batch(
        state.clone(),
        MutateBatchParams {
            fork_id: fork_id.clone(),
            mode: Some(BatchMode::Apply),
            ops: ops(vec![
                // transform family
                json!({
                    "kind": "fill_range",
                    "sheet_name": "Sheet1",
                    "target": {"kind": "range", "range": "C1:C2"},
                    "value": "7"
                }),
                // structure family
                json!({
                    "kind": "create_sheet",
                    "name": "Extra"
                }),
                // rules family
                json!({
                    "kind": "set_data_validation",
                    "sheet_name": "Sheet1",
                    "target_range": "B1:B2",
                    "validation": {"kind": "list", "formula1": "\"yes,no\""}
                }),
                // style family
                json!({
                    "kind": "style",
                    "sheet_name": "Sheet1",
                    "target": {"kind": "range", "range": "A1:A1"},
                    "patch": {"font": {"bold": true}}
                }),
                // sheet layout family
                json!({
                    "kind": "freeze_panes",
                    "sheet_name": "Sheet1",
                    "freeze_rows": 1,
                    "freeze_cols": 0
                }),
                // column size family
                json!({
                    "kind": "column_size",
                    "sheet_name": "Sheet1",
                    "target": {"kind": "columns", "range": "A:A"},
                    "size": {"kind": "width", "width_chars": 20.0}
                }),
                // formula pattern (wraps apply_formula_pattern)
                json!({
                    "kind": "formula_pattern",
                    "sheet_name": "Sheet1",
                    "target_range": "D1:D2",
                    "anchor_cell": "D1",
                    "base_formula": "A1*2"
                }),
                // replace_in_formulas
                json!({
                    "kind": "replace_in_formulas",
                    "sheet_name": "Sheet1",
                    "find": "SUM",
                    "replace": "COUNT"
                }),
            ]),
            label: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(response.mode, "apply");
    assert_eq!(response.ops_applied, Some(8));
    assert_eq!(response.ops_staged, None);
    assert_eq!(response.results.len(), 8);
    assert!(response.recalc_needed);

    let families: Vec<&str> = response.results.iter().map(|r| r.family.as_str()).collect();
    assert_eq!(
        families,
        vec![
            "transform",
            "structure",
            "rules",
            "style",
            "sheet_layout",
            "column_size",
            "formula_pattern",
            "replace_in_formulas"
        ]
    );
    for (i, result) in response.results.iter().enumerate() {
        assert_eq!(result.index, i);
        assert_eq!(result.status, "applied");
        assert!(
            result.detail.is_some(),
            "each single-op group carries detail"
        );
        assert!(result.change_id.is_none(), "apply mode stages nothing");
    }

    // Serialized response must use `ops_applied` and never `ops_staged`.
    let value = serde_json::to_value(&response)?;
    assert!(value.get("ops_applied").is_some());
    assert!(value.get("ops_staged").is_none());

    // Spot-check the fork actually mutated per family routing.
    let fork_wb = state.open_workbook(&WorkbookId(fork_id.clone())).await?;
    let (c1, d2_formula, e1_formula) = fork_wb.with_sheet("Sheet1", |sheet| {
        (
            sheet
                .get_cell("C1")
                .map(|c| c.get_value().to_string())
                .unwrap_or_default(),
            sheet
                .get_cell("D2")
                .map(|c| c.get_formula().to_string())
                .unwrap_or_default(),
            sheet
                .get_cell("E1")
                .map(|c| c.get_formula().to_string())
                .unwrap_or_default(),
        )
    })?;
    assert_eq!(c1, "7");
    assert_eq!(d2_formula.replace(' ', ""), "A2*2");
    assert!(
        e1_formula.contains("COUNT"),
        "replace_in_formulas should have rewritten SUM -> COUNT, got {e1_formula}"
    );
    assert!(
        fork_wb.with_sheet("Extra", |_| ()).is_ok(),
        "create_sheet op should have added sheet 'Extra'"
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn mutate_batch_groups_consecutive_same_family_ops() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("mutate_grouping.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("x");
    });

    let state = recalc_state(&workspace);
    let fork_id = forked_workbook(&state).await?;

    let response = mutate_batch(
        state.clone(),
        MutateBatchParams {
            fork_id: fork_id.clone(),
            mode: None, // defaults to apply
            ops: ops(vec![
                json!({
                    "kind": "fill_range",
                    "sheet_name": "Sheet1",
                    "target": {"kind": "range", "range": "B1:B1"},
                    "value": "1"
                }),
                json!({
                    "kind": "clear_range",
                    "sheet_name": "Sheet1",
                    "target": {"kind": "range", "range": "A1:A1"}
                }),
            ]),
            label: None,
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(response.ops_applied, Some(2));
    assert_eq!(response.results.len(), 2);
    // Consecutive transform ops form one group: detail on first op only.
    assert!(response.results[0].detail.is_some());
    assert!(response.results[1].detail.is_none());

    let fork_wb = state.open_workbook(&WorkbookId(fork_id)).await?;
    let (a1, b1) = fork_wb.with_sheet("Sheet1", |sheet| {
        (
            sheet
                .get_cell("A1")
                .map(|c| c.get_value().to_string())
                .unwrap_or_default(),
            sheet
                .get_cell("B1")
                .map(|c| c.get_value().to_string())
                .unwrap_or_default(),
        )
    })?;
    assert_eq!(a1, "");
    assert_eq!(b1, "1");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn mutate_batch_preview_stages_and_does_not_mutate() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("mutate_preview.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("keep");
    });

    let state = recalc_state(&workspace);
    let fork_id = forked_workbook(&state).await?;

    let response = mutate_batch(
        state.clone(),
        MutateBatchParams {
            fork_id: fork_id.clone(),
            mode: Some(BatchMode::Preview),
            ops: ops(vec![json!({
                "kind": "fill_range",
                "sheet_name": "Sheet1",
                "target": {"kind": "range", "range": "A1:A1"},
                "value": "overwritten"
            })]),
            label: Some("what-if".to_string()),
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(response.mode, "preview");
    assert_eq!(response.ops_staged, Some(1));
    assert_eq!(response.ops_applied, None);
    assert_eq!(response.results[0].status, "staged");
    let change_id = response.results[0]
        .change_id
        .clone()
        .expect("staged op exposes change_id");

    // Serialized response must use `ops_staged`, never `ops_applied`.
    let value = serde_json::to_value(&response)?;
    assert!(value.get("ops_staged").is_some());
    assert!(value.get("ops_applied").is_none());

    // Fork untouched.
    let fork_wb = state.open_workbook(&WorkbookId(fork_id.clone())).await?;
    let a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet
            .get_cell("A1")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default()
    })?;
    assert_eq!(a1, "keep");

    // Staged change is listed and applies cleanly.
    let staged = list_staged_changes(
        state.clone(),
        ListStagedChangesParams {
            fork_id: fork_id.clone(),
        },
    )
    .await?;
    assert_eq!(staged.staged_changes.len(), 1);
    assert_eq!(staged.staged_changes[0].change_id, change_id);

    apply_staged_change(
        state.clone(),
        ApplyStagedChangeParams {
            fork_id: fork_id.clone(),
            change_id,
        },
    )
    .await?;

    let fork_wb = state.open_workbook(&WorkbookId(fork_id)).await?;
    let a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet
            .get_cell("A1")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default()
    })?;
    assert_eq!(a1, "overwritten");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn mutate_batch_rejects_unknown_kind_before_applying_anything() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("mutate_bad_kind.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("keep");
    });

    let state = recalc_state(&workspace);
    let fork_id = forked_workbook(&state).await?;

    let err = mutate_batch(
        state.clone(),
        MutateBatchParams {
            fork_id: fork_id.clone(),
            mode: Some(BatchMode::Apply),
            ops: ops(vec![
                json!({
                    "kind": "fill_range",
                    "sheet_name": "Sheet1",
                    "target": {"kind": "range", "range": "A1:A1"},
                    "value": "clobbered"
                }),
                json!({"kind": "bogus_kind"}),
            ]),
            label: None,
            formula_parse_policy: None,
        },
    )
    .await
    .expect_err("unknown kind must fail");

    let message = format!("{err:#}");
    assert!(
        message.contains("unknown op kind 'bogus_kind'"),
        "message should name the bad kind: {message}"
    );
    let invalid = err
        .downcast_ref::<agent_spreadsheet_mcp::errors::InvalidParamsError>()
        .expect("shape errors surface as InvalidParamsError");
    assert_eq!(invalid.path(), Some("ops[1]"));

    // Parse failures happen before any dispatch: op 0 must NOT have applied.
    let fork_wb = state.open_workbook(&WorkbookId(fork_id)).await?;
    let a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet
            .get_cell("A1")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default()
    })?;
    assert_eq!(a1, "keep");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn mutate_batch_runtime_failure_reports_index_and_prior_applied_state() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("mutate_runtime_fail.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("x");
    });

    let state = recalc_state(&workspace);
    let fork_id = forked_workbook(&state).await?;

    let err = mutate_batch(
        state.clone(),
        MutateBatchParams {
            fork_id: fork_id.clone(),
            mode: Some(BatchMode::Apply),
            ops: ops(vec![
                json!({
                    "kind": "fill_range",
                    "sheet_name": "Sheet1",
                    "target": {"kind": "range", "range": "B1:B1"},
                    "value": "42"
                }),
                // anchor cell outside target range -> deterministic runtime error
                json!({
                    "kind": "formula_pattern",
                    "sheet_name": "Sheet1",
                    "target_range": "C1:C2",
                    "anchor_cell": "Z9",
                    "base_formula": "A1*2"
                }),
            ]),
            label: None,
            formula_parse_policy: None,
        },
    )
    .await
    .expect_err("invalid anchor must fail");

    let message = format!("{err:#}");
    assert!(
        message.contains("op at index 1"),
        "message should report the failing op index: {message}"
    );
    assert!(
        message.contains("family 'formula_pattern'"),
        "message should report the failing family: {message}"
    );
    assert!(
        message.contains("Ops at indices 0..=0 were already applied"),
        "message should state whether prior ops were applied: {message}"
    );

    // Prior group really did apply.
    let fork_wb = state.open_workbook(&WorkbookId(fork_id)).await?;
    let b1 = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet
            .get_cell("B1")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default()
    })?;
    assert_eq!(b1, "42");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn edit_batch_preview_stages_and_does_not_mutate() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("edit_preview.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("keep");
    });

    let state = recalc_state(&workspace);
    let fork_id = forked_workbook(&state).await?;

    let response = agent_spreadsheet_mcp::tools::fork::edit_batch(
        state.clone(),
        EditBatchParamsInput {
            fork_id: fork_id.clone(),
            sheet_name: "Sheet1".to_string(),
            edits: vec![CellEditInput::Shorthand("A1=100".to_string())],
            mode: Some(BatchMode::Preview),
            formula_parse_policy: None,
        },
    )
    .await?;

    assert_eq!(response.mode, "preview");
    assert_eq!(response.edits_staged, Some(1));
    assert_eq!(response.edits_applied, None);
    let change_id = response.change_id.clone().expect("change_id in preview");

    // Serialized response must use `edits_staged`, never `edits_applied`.
    let value = serde_json::to_value(&response)?;
    assert!(value.get("edits_staged").is_some());
    assert!(value.get("edits_applied").is_none());

    let fork_wb = state.open_workbook(&WorkbookId(fork_id.clone())).await?;
    let a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet
            .get_cell("A1")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default()
    })?;
    assert_eq!(a1, "keep");

    apply_staged_change(
        state.clone(),
        ApplyStagedChangeParams {
            fork_id: fork_id.clone(),
            change_id,
        },
    )
    .await?;

    let fork_wb = state.open_workbook(&WorkbookId(fork_id)).await?;
    let a1 = fork_wb.with_sheet("Sheet1", |sheet| {
        sheet
            .get_cell("A1")
            .map(|c| c.get_value().to_string())
            .unwrap_or_default()
    })?;
    assert_eq!(a1, "100");

    Ok(())
}

const LEGACY_013_TOOLS: &[&str] = &[
    "apply_formula_pattern",
    "apply_staged_change",
    "checkpoint_fork",
    "close_workbook",
    "column_size_batch",
    "create_fork",
    "define_name",
    "delete_checkpoint",
    "delete_name",
    "describe_workbook",
    "discard_fork",
    "discard_staged_change",
    "edit_batch",
    "execute_manifest",
    "find_formula",
    "find_value",
    "formula_trace",
    "get_changeset",
    "get_edits",
    "get_manifest_stub",
    "grid_export",
    "grid_import",
    "inspect_cells",
    "layout_page",
    "list_checkpoints",
    "list_forks",
    "list_sheets",
    "list_staged_changes",
    "list_workbooks",
    "mutate_batch",
    "named_ranges",
    "range_values",
    "read_table",
    "recalculate",
    "replace_in_formulas",
    "restore_checkpoint",
    "rules_batch",
    "save_fork",
    "scan_volatiles",
    "screenshot_sheet",
    "sheet_formula_map",
    "sheet_layout_batch",
    "sheet_overview",
    "sheet_page",
    "sheet_statistics",
    "sheet_styles",
    "structure_batch",
    "style_batch",
    "table_profile",
    "transform_batch",
    "update_name",
    "vba_module_source",
    "vba_project_summary",
    "verify_workbook",
    "workbook_style_summary",
    "workbook_summary",
];

#[test]
fn slim_surface_is_exactly_the_available_canonical_registry() {
    use agent_spreadsheet::operations::{RuntimeCapabilities, operation_registry};

    let workspace = support::TestWorkspace::new();
    let config = workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
        assert!(cfg.slim_surface);
    });
    let state = support::app_state_with_config(config);
    let mut capabilities = RuntimeCapabilities::from_state(&state);
    capabilities.vba = false;
    capabilities.resident_history = cfg!(feature = "recalc-formualizer");
    let mut expected = operation_registry()
        .iter()
        .filter(|descriptor| descriptor.is_available(&capabilities))
        .map(|descriptor| descriptor.name.to_string())
        .collect::<Vec<_>>();
    let server = SpreadsheetServer::from_state(state);
    let mut names = server.tool_names();
    expected.sort();
    names.sort();

    assert_eq!(names, expected);
    assert!(!names.iter().any(|name| name == "close_workbook"));
    assert!(!names.iter().any(|name| name == "mutate_batch"));
}

#[test]
fn compat_mode_registers_every_013_tool_without_duplicate_names() {
    let workspace = support::TestWorkspace::new();
    let config = workspace.config_with(|cfg| {
        cfg.recalc_enabled = true;
        cfg.vba_enabled = true;
        cfg.slim_surface = false;
    });
    let server = SpreadsheetServer::from_state(support::app_state_with_config(config));
    let names = server.tool_names();

    for tool in LEGACY_013_TOOLS {
        assert!(
            names.iter().any(|name| name == tool),
            "compat mode must register legacy tool '{tool}'"
        );
    }
    let unique = names.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(
        unique.len(),
        names.len(),
        "router must not double-register names"
    );
}
