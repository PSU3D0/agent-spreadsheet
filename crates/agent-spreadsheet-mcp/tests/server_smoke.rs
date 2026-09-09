use anyhow::Result;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::ErrorCode;
use std::collections::HashSet;
use std::sync::Arc;

use agent_spreadsheet_mcp::model::SheetPageFormat;
use agent_spreadsheet_mcp::tools::{ListWorkbooksParams, SheetPageParams};
use agent_spreadsheet_mcp::{SpreadsheetServer, startup_scan};

mod support;

#[test]
fn startup_scan_discovers_workspace_workbooks() {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("reports/summary.xlsx", |_| {});

    let state = workspace.app_state();
    let response = startup_scan(&state).expect("startup scan");

    assert_eq!(response.workbooks.len(), 1);
    assert_eq!(response.workbooks[0].slug, "summary");
}

#[tokio::test(flavor = "current_thread")]
async fn server_tool_handlers_return_json() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("simple.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut((1, 1)).set_value("Name".to_string());
        sheet.cell_mut((2, 1)).set_value("Value".to_string());
        sheet.cell_mut((1, 2)).set_value("Alpha".to_string());
        sheet.cell_mut((2, 2)).set_value_number(10_f64);
        sheet
            .cell_mut((2, 3))
            .set_formula("B2*2")
            .set_formula_result_default("20");
    });

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
        .await
        .expect("list workbooks")
        .0;
    assert_eq!(list.0.workbooks.len(), 1);
    let workbook_id = list.0.workbooks[0].workbook_id.clone();

    let error = match server
        .sheet_page(Parameters(SheetPageParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: "Missing".to_string(),
            start_row: 1,
            page_size: 10,
            columns: None,
            columns_by_header: None,
            include_formulas: true,
            include_styles: false,
            include_header: true,
            format: None,
        }))
        .await
    {
        Ok(_) => panic!("missing sheet should error"),
        Err(err) => err,
    };
    assert!(error.message.contains("sheet Missing"));

    let page = server
        .sheet_page(Parameters(SheetPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            start_row: 1,
            page_size: 10,
            columns: None,
            include_formulas: true,
            include_styles: false,
            columns_by_header: None,
            include_header: true,
            format: Some(SheetPageFormat::Full),
        }))
        .await
        .expect("page fetch")
        .0;
    assert_eq!(page.0.rows.len(), 3);
    let contains_b3 = page
        .0
        .rows
        .iter()
        .flat_map(|row| row.cells.iter())
        .any(|cell| cell.address == "B3");
    assert!(contains_b3);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn response_size_guard_rejects_large_payloads() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("oversize.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        for row in 1..=25u32 {
            sheet
                .cell_mut((1u32, row))
                .set_value(format!("Row{}", row));
            sheet.cell_mut((2u32, row)).set_value_number(row as i32);
        }
    });

    let config = workspace.config_with(|cfg| {
        cfg.max_response_bytes = Some(1);
    });
    let server = SpreadsheetServer::new(Arc::new(config)).await?;

    let err = match server
        .list_workbooks(Parameters(ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        }))
        .await
    {
        Ok(_) => panic!("expected response size error"),
        Err(err) => err,
    };

    assert_eq!(err.code, ErrorCode::INVALID_REQUEST);
    assert!(err.message.contains("response too large"));

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn disabled_tools_return_invalid_request() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let workbook_path = workspace.create_workbook("locked.xlsx", |_| {});

    let mut enabled = HashSet::new();
    enabled.insert("list_workbooks".to_string());

    let config = workspace.config_with(|cfg| {
        cfg.enabled_tools = Some(enabled);
        cfg.single_workbook = Some(workbook_path.clone());
    });
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
    assert_eq!(list.0.workbooks.len(), 1);
    let workbook_id = list.0.workbooks[0].workbook_id.clone();

    let error = match server
        .sheet_page(Parameters(SheetPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            start_row: 1,
            page_size: 5,
            columns: None,
            include_formulas: true,
            include_styles: false,
            columns_by_header: None,
            include_header: true,
            format: None,
        }))
        .await
    {
        Ok(_) => panic!("sheet_page should be disabled"),
        Err(err) => err,
    };

    assert_eq!(error.code, ErrorCode::INVALID_REQUEST);
    assert!(
        error
            .message
            .contains("tool 'sheet_page' is disabled by server configuration")
    );

    Ok(())
}
