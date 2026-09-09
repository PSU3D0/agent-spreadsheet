use std::sync::Arc;

use anyhow::Result;
use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::model::TableOutputFormat;
use agent_spreadsheet_mcp::repository::{VirtualWorkbookInput, VirtualWorkspaceRepository};
use agent_spreadsheet_mcp::state::AppState;
use agent_spreadsheet_mcp::tools::{
    ListSheetsParams, ListWorkbooksParams, RangeValuesParams, list_sheets, list_workbooks,
    range_values,
};

mod support;

#[tokio::test]
async fn path_and_virtual_readflows_match() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("parity.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.set_name("Data");
        sheet.cell_mut("A1").set_value("item");
        sheet.cell_mut("B1").set_value("amount");
        sheet.cell_mut("A2").set_value("rent");
        sheet.cell_mut("B2").set_value_number(1000);
        sheet.cell_mut("A3").set_value("food");
        sheet.cell_mut("B3").set_value_number(500);
    });

    let config = Arc::new(workspace.config());
    let path_state = Arc::new(AppState::new(config.clone()));

    let path_list = list_workbooks(
        path_state.clone(),
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
    let path_id = path_list.workbooks[0].workbook_id.clone();

    let path_sheets = list_sheets(
        path_state.clone(),
        ListSheetsParams {
            workbook_or_fork_id: path_id.clone(),
            limit: None,
            offset: None,
            include_bounds: Some(true),
        },
    )
    .await?;

    let path_ranges = range_values(
        path_state.clone(),
        RangeValuesParams {
            workbook_or_fork_id: path_id,
            sheet_name: "Data".to_string(),
            ranges: vec!["A1:B3".to_string()],
            include_headers: Some(false),
            include_formulas: None,
            format: Some(TableOutputFormat::Values),
            page_size: None,
        },
    )
    .await?;

    let virtual_repo = Arc::new(VirtualWorkspaceRepository::new(config.clone()));
    let virtual_id = virtual_repo.register(VirtualWorkbookInput {
        key: "parity.xlsx".to_string(),
        slug: Some("parity".to_string()),
        bytes: std::fs::read(path)?,
    });
    let virtual_state = Arc::new(AppState::new_with_repository(config, virtual_repo));

    let virtual_sheets = list_sheets(
        virtual_state.clone(),
        ListSheetsParams {
            workbook_or_fork_id: virtual_id.clone(),
            limit: None,
            offset: None,
            include_bounds: Some(true),
        },
    )
    .await?;

    let virtual_ranges = range_values(
        virtual_state,
        RangeValuesParams {
            workbook_or_fork_id: virtual_id,
            sheet_name: "Data".to_string(),
            ranges: vec!["A1:B3".to_string()],
            include_headers: Some(false),
            include_formulas: None,
            format: Some(TableOutputFormat::Values),
            page_size: None,
        },
    )
    .await?;

    let path_sheet_names: Vec<_> = path_sheets.sheets.iter().map(|s| s.name.clone()).collect();
    let virtual_sheet_names: Vec<_> = virtual_sheets
        .sheets
        .iter()
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(path_sheet_names, virtual_sheet_names);

    assert_eq!(
        serde_json::to_value(&path_ranges.values)?,
        serde_json::to_value(&virtual_ranges.values)?
    );

    Ok(())
}
