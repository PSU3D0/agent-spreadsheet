//! MCP server-level tests for agent-safe read guardrails (ticket #4106).
//!
//! Verifies:
//! - inspect_cells is registered and callable via MCP tool handler
//! - inspect_cells rejects over-limit requests with actionable MCP error
//! - sheet_page budget signaling flows through MCP response pruning
//! - Budget metadata is machine-consumable in the JSON response

use anyhow::Result;
use rmcp::handler::server::wrapper::Parameters;
use std::sync::Arc;

use agent_spreadsheet_mcp::SpreadsheetServer;
use agent_spreadsheet_mcp::model::SheetPageFormat;
use agent_spreadsheet_mcp::tools::{InspectCellsParams, ListWorkbooksParams, SheetPageParams};

mod support;

fn build_data_workbook(book: &mut umya_spreadsheet::Workbook) {
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    for col in 1..=10u32 {
        sheet
            .get_cell_mut((col, 1))
            .set_value(format!("Header{}", col));
    }
    for row in 2..=51u32 {
        for col in 1..=10u32 {
            sheet
                .get_cell_mut((col, row))
                .set_value(format!("val_r{}_c{}", row, col));
        }
    }
}

async fn setup_server(
    workspace: &support::TestWorkspace,
) -> Result<(SpreadsheetServer, agent_spreadsheet_mcp::model::WorkbookId)> {
    workspace.create_workbook("data.xlsx", build_data_workbook);
    let server = workspace.server().await?;

    let list = server
        .list_workbooks(Parameters(ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        }))
        .await?
        .0;
    let workbook_id = list.0.workbooks[0].workbook_id.clone();
    Ok((server, workbook_id))
}

#[tokio::test(flavor = "current_thread")]
async fn mcp_inspect_cells_returns_budget_metadata() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let (server, workbook_id) = setup_server(&workspace).await?;

    let response = server
        .inspect_cells(Parameters(InspectCellsParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            targets: vec!["A1:B2".to_string()],
            include_empty: Some(true),
            budget: None,
        }))
        .await?
        .0;

    // Verify response structure
    assert_eq!(response.0.cells.len(), 4); // 2×2
    assert!(!response.0.truncated);

    // Verify budget is present
    let budget = response
        .0
        .budget
        .as_ref()
        .expect("budget metadata required");
    assert_eq!(budget.max_cells, Some(25));
    assert_eq!(budget.cells_returned, 4);
    assert_eq!(budget.rows_returned, 2);
    assert_eq!(budget.total_rows_available, Some(2));

    // Verify serializable as JSON (agent-consumable)
    let json = serde_json::to_value(&response.0)?;
    assert!(json.get("budget").is_some());
    assert!(json["budget"]["max_cells"].is_number());

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn mcp_inspect_cells_rejects_over_limit_with_mcp_error() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let (server, workbook_id) = setup_server(&workspace).await?;

    // Request 30 cells (> 25 limit)
    let result = server
        .inspect_cells(Parameters(InspectCellsParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            targets: vec!["A1:F5".to_string()], // 6×5 = 30
            include_empty: Some(false),
            budget: None,
        }))
        .await;

    let mcp_err = match result {
        Ok(_) => panic!("should reject over-limit"),
        Err(e) => e,
    };
    assert!(
        mcp_err.message.contains("detail view") || mcp_err.message.contains("25"),
        "error should mention limit: {}",
        mcp_err.message
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn mcp_sheet_page_budget_flows_through_response() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let config = workspace.config_with(|cfg| {
        cfg.max_cells = Some(30); // 3 rows of 10 cols
        cfg.max_payload_bytes = None;
    });
    workspace.create_workbook("budget_flow.xlsx", build_data_workbook);
    let server = SpreadsheetServer::new(Arc::new(config)).await?;

    let list = server
        .list_workbooks(Parameters(ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        }))
        .await?
        .0;
    let workbook_id = list.0.workbooks[0].workbook_id.clone();

    let response = server
        .sheet_page(Parameters(SheetPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            start_row: 1,
            page_size: 50,
            columns: None,
            columns_by_header: None,
            include_formulas: false,
            include_styles: false,
            include_header: true,
            format: Some(SheetPageFormat::Full),
        }))
        .await?
        .0;

    assert!(
        response.0.truncated,
        "should be truncated with 30-cell budget"
    );
    assert!(response.0.rows.len() <= 3, "max 3 rows at 10 cols/row");
    assert!(response.0.next_start_row.is_some());

    let budget = response.0.budget.as_ref().expect("budget metadata");
    assert_eq!(budget.max_cells, Some(30));
    assert!(budget.cells_returned <= 30);
    assert!(budget.continuation.is_some());

    // Verify JSON shape
    let json = serde_json::to_value(&response.0)?;
    assert_eq!(json["truncated"], true);
    assert!(json["budget"]["continuation"].is_string());

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn mcp_inspect_cells_empty_targets_returns_error() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let (server, workbook_id) = setup_server(&workspace).await?;

    let result = server
        .inspect_cells(Parameters(InspectCellsParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            targets: vec![],
            include_empty: None,
            budget: None,
        }))
        .await;

    let mcp_err = match result {
        Ok(_) => panic!("empty targets should error"),
        Err(e) => e,
    };
    assert!(mcp_err.message.contains("at least one"));

    Ok(())
}
