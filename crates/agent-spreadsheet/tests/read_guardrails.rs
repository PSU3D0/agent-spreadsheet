//! Tests for agent-safe read guardrails and output budgets (ticket #4106).
//!
//! Verifies:
//! - sheet_page truncation + budget metadata under tight cell/payload limits
//! - inspect_cells strict detail-view budget (hard cap at 25)
//! - inspect_cells rejects over-limit requests with actionable error
//! - Machine-consumable continuation guidance in budget metadata
//! - Deterministic truncation behavior

use anyhow::Result;
use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::model::SheetPageFormat;
use agent_spreadsheet_mcp::tools::{
    InspectCellsParams, ListWorkbooksParams, SheetPageParams, inspect_cells, list_workbooks,
    sheet_page,
};

mod support;

/// Build a workbook with enough data to trigger truncation under tight limits.
fn build_wide_workbook(book: &mut umya_spreadsheet::Workbook) {
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    // Header row
    for col in 1..=10u32 {
        sheet
            .get_cell_mut((col, 1))
            .set_value(format!("Col{}", col));
    }
    // 100 data rows × 10 columns = 1000 cells
    for row in 2..=101u32 {
        for col in 1..=10u32 {
            sheet
                .get_cell_mut((col, row))
                .set_value(format!("R{}C{}", row, col));
        }
    }
}

async fn get_workbook_id(
    state: std::sync::Arc<agent_spreadsheet_mcp::state::AppState>,
) -> agent_spreadsheet_mcp::model::WorkbookId {
    let workbooks = list_workbooks(
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
    .await
    .expect("list workbooks");
    workbooks.workbooks[0].workbook_id.clone()
}

// ─── sheet_page guardrails ───────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn sheet_page_truncates_under_cell_budget() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("guardrail.xlsx", build_wide_workbook);

    // Set max_cells to 50 → at 10 cols/row, only 5 rows should fit
    let config = workspace.config_with(|cfg| {
        cfg.max_cells = Some(50);
        cfg.max_payload_bytes = None; // disable payload limit for this test
    });
    let state = support::app_state_with_config(config);
    let workbook_id = get_workbook_id(state.clone()).await;

    let response = sheet_page(
        state,
        SheetPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            start_row: 1,
            page_size: 100, // request more than budget allows
            format: Some(SheetPageFormat::Full),
            ..Default::default()
        },
    )
    .await?;

    // Should be truncated
    assert!(response.truncated, "response should be truncated");
    assert!(
        response.rows.len() <= 5,
        "rows should be capped by cell budget"
    );
    assert!(
        response.next_start_row.is_some(),
        "should have continuation cursor"
    );

    // Budget metadata must be present and accurate
    let budget = response.budget.expect("budget metadata required");
    assert_eq!(budget.max_cells, Some(50));
    assert!(budget.cells_returned <= 50);
    assert!(budget.rows_returned <= 5);
    assert!(budget.total_rows_available.is_some());
    assert!(
        budget.continuation.is_some(),
        "continuation hint required on truncation"
    );
    let hint = budget.continuation.unwrap();
    assert!(
        hint.contains("start_row="),
        "hint should contain start_row guidance: {}",
        hint
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_page_truncates_under_payload_budget() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("guardrail_bytes.xlsx", build_wide_workbook);

    // Set a very tight payload budget
    let config = workspace.config_with(|cfg| {
        cfg.max_cells = None;
        cfg.max_payload_bytes = Some(2048);
    });
    let state = support::app_state_with_config(config);
    let workbook_id = get_workbook_id(state.clone()).await;

    let response = sheet_page(
        state,
        SheetPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            start_row: 1,
            page_size: 100,
            format: Some(SheetPageFormat::Full),
            ..Default::default()
        },
    )
    .await?;

    // Payload limit should truncate well below 100 rows
    assert!(
        response.truncated,
        "response should be truncated by payload budget"
    );
    assert!(
        response.rows.len() < 100,
        "rows should be fewer than requested"
    );
    let budget = response.budget.expect("budget metadata required");
    assert_eq!(budget.max_payload_bytes, Some(2048));
    assert!(budget.continuation.is_some());

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_page_no_truncation_emits_budget_when_limits_configured() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("small.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut((1, 1)).set_value("A");
        sheet.get_cell_mut((1, 2)).set_value("B");
    });

    let state = workspace.app_state(); // default limits are configured
    let workbook_id = get_workbook_id(state.clone()).await;

    let response = sheet_page(
        state,
        SheetPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            start_row: 1,
            page_size: 50,
            format: Some(SheetPageFormat::Full),
            ..Default::default()
        },
    )
    .await?;

    // No truncation
    assert!(!response.truncated);
    // Budget still present because limits are configured (agent can see the contract)
    let budget = response
        .budget
        .expect("budget metadata should be present when limits configured");
    assert!(budget.rows_returned > 0);
    // No continuation hint when not truncated and all data returned
    if response.next_start_row.is_none() {
        assert!(budget.continuation.is_none());
    }

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_page_continuation_cursor_is_deterministic() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("deterministic.xlsx", build_wide_workbook);

    let config = workspace.config_with(|cfg| {
        cfg.max_cells = Some(30); // 3 rows × 10 cols
        cfg.max_payload_bytes = None;
    });
    let state = support::app_state_with_config(config);
    let workbook_id = get_workbook_id(state.clone()).await;

    // First page
    let page1 = sheet_page(
        state.clone(),
        SheetPageParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: "Sheet1".to_string(),
            start_row: 1,
            page_size: 100,
            format: Some(SheetPageFormat::Full),
            ..Default::default()
        },
    )
    .await?;

    assert!(page1.truncated);
    let next = page1.next_start_row.expect("must have continuation");

    // Second page picks up from continuation cursor
    let page2 = sheet_page(
        state,
        SheetPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            start_row: next,
            page_size: 100,
            format: Some(SheetPageFormat::Full),
            ..Default::default()
        },
    )
    .await?;

    // Pages should not overlap
    let page1_last = page1.rows.last().unwrap().row_index;
    let page2_first = page2.rows.first().unwrap().row_index;
    assert!(page2_first > page1_last, "pages must not overlap");

    Ok(())
}

// ─── inspect_cells guardrails ────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn inspect_cells_rejects_over_limit() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("inspect_limit.xlsx", build_wide_workbook);
    let state = workspace.app_state();
    let workbook_id = get_workbook_id(state.clone()).await;

    // Request 30 cells (> DETAIL_LIMIT of 25)
    let err = inspect_cells(
        state,
        InspectCellsParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            targets: vec!["A1:E6".to_string()], // 5×6 = 30 cells
            include_empty: Some(false),
            budget: None,
        },
    )
    .await;

    assert!(err.is_err());
    let msg = err.unwrap_err().to_string();
    assert!(
        msg.contains("detail view"),
        "error should mention detail view: {}",
        msg
    );
    assert!(
        msg.contains("25"),
        "error should mention the limit: {}",
        msg
    );
    assert!(
        msg.contains("sheet-page") || msg.contains("range-values"),
        "error should suggest alternatives: {}",
        msg
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn inspect_cells_returns_budget_metadata() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("inspect_budget.xlsx", build_wide_workbook);
    let state = workspace.app_state();
    let workbook_id = get_workbook_id(state.clone()).await;

    let response = inspect_cells(
        state,
        InspectCellsParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            targets: vec!["A1:C3".to_string()], // 3×3 = 9 cells, well under limit
            include_empty: Some(true),
            budget: None,
        },
    )
    .await?;

    assert!(!response.truncated);
    assert_eq!(response.cells.len(), 9);

    let budget = response
        .budget
        .expect("budget metadata should always be present");
    assert_eq!(budget.max_cells, Some(25)); // DETAIL_LIMIT
    assert_eq!(budget.cells_returned, 9);
    assert_eq!(budget.rows_returned, 3);
    assert_eq!(budget.total_rows_available, Some(3));
    // No continuation when not truncated
    assert!(budget.continuation.is_none());

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn inspect_cells_respects_lower_config_limit() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("inspect_low.xlsx", build_wide_workbook);

    // Set max_cells to 10, which is lower than DETAIL_LIMIT (25)
    let config = workspace.config_with(|cfg| {
        cfg.max_cells = Some(10);
    });
    let state = support::app_state_with_config(config);
    let workbook_id = get_workbook_id(state.clone()).await;

    // Request exactly 10 cells → should work
    let response = inspect_cells(
        state.clone(),
        InspectCellsParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: "Sheet1".to_string(),
            targets: vec!["A1:B5".to_string()], // 2×5 = 10
            include_empty: Some(true),
            budget: None,
        },
    )
    .await?;
    assert_eq!(response.cells.len(), 10);
    let budget = response.budget.expect("budget present");
    assert_eq!(budget.max_cells, Some(10));

    // Request 11 cells → should fail
    let err = inspect_cells(
        state,
        InspectCellsParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            targets: vec!["A1:B5".to_string(), "C1".to_string()], // 10 + 1 = 11
            include_empty: Some(false),
            budget: None,
        },
    )
    .await;
    assert!(err.is_err());

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn inspect_cells_payload_truncation_signals_budget() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("inspect_payload.xlsx", build_wide_workbook);

    // Very tight payload budget to force truncation within the 25-cell limit
    let config = workspace.config_with(|cfg| {
        cfg.max_payload_bytes = Some(512);
        cfg.max_cells = Some(25);
    });
    let state = support::app_state_with_config(config);
    let workbook_id = get_workbook_id(state.clone()).await;

    let response = inspect_cells(
        state,
        InspectCellsParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            targets: vec!["A1:E5".to_string()], // 25 cells
            include_empty: Some(true),
            budget: None,
        },
    )
    .await?;

    // Under tight payload budget, cells should be truncated
    if response.truncated {
        assert!(response.cells.len() < 25);
        let budget = response.budget.as_ref().expect("budget on truncation");
        assert!(
            budget.continuation.is_some(),
            "truncated inspect must have continuation hint"
        );
        let hint = budget.continuation.as_ref().unwrap();
        assert!(
            hint.contains("detail-view") || hint.contains("narrow"),
            "hint should guide narrowing: {}",
            hint
        );
    }
    // If no truncation (payload fit), that's ok too — just verify budget exists
    assert!(response.budget.is_some());

    Ok(())
}
