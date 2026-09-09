use anyhow::Result;
use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::model::TableOutputFormat;
use agent_spreadsheet_mcp::tools::{
    FindValueParams, ListWorkbooksParams, ReadTableParams, SheetOverviewParams, find_value,
    list_workbooks, read_table, sheet_overview,
};
use umya_spreadsheet::Workbook as Spreadsheet;

mod support;

#[tokio::test(flavor = "current_thread")]
async fn sheet_overview_reports_regions_and_tools_scope_to_region() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("regions.xlsx", build_regioned_workbook);
    let state = workspace.app_state();

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
    let descriptor = &list.workbooks[0];
    let workbook_id = descriptor.workbook_id.clone();

    let overview = sheet_overview(
        state.clone(),
        SheetOverviewParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: "Sheet1".to_string(),
            max_regions: None,
            max_headers: None,
            include_headers: None,
        },
    )
    .await?;
    assert_eq!(overview.detected_regions.len(), 2);
    let left = overview
        .detected_regions
        .iter()
        .find(|r| r.bounds == "A1:B4")
        .expect("left region");
    let right = overview
        .detected_regions
        .iter()
        .find(|r| r.bounds == "E1:F3")
        .expect("right region");
    assert!(left.confidence > 0.3);
    assert!(right.confidence > 0.3);

    // read_table scoped to left region returns only left data
    let table = read_table(
        state.clone(),
        ReadTableParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: Some("Sheet1".to_string()),
            region_id: Some(left.id),
            format: Some(TableOutputFormat::Json),
            ..Default::default()
        },
    )
    .await?;
    let values: Vec<String> = table
        .rows
        .iter()
        .filter_map(|row| {
            row.get("Month").and_then(|v| match v {
                Some(agent_spreadsheet_mcp::model::CellValue::Text(s)) => Some(s.clone()),
                Some(agent_spreadsheet_mcp::model::CellValue::Number(n)) => Some(n.to_string()),
                Some(agent_spreadsheet_mcp::model::CellValue::Bool(b)) => Some(b.to_string()),
                Some(agent_spreadsheet_mcp::model::CellValue::Date(d)) => Some(d.clone()),
                Some(agent_spreadsheet_mcp::model::CellValue::Error(e)) => Some(e.clone()),
                None => None,
            })
        })
        .collect();
    assert_eq!(
        values,
        vec!["Jan".to_string(), "Feb".to_string(), "Mar".to_string()]
    );

    // find_value scoped to right region should only see labels/values there
    let find = find_value(
        state.clone(),
        FindValueParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: Some("Sheet1".to_string()),
            region_id: Some(right.id),
            query: "Target".to_string(),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(find.matches.len(), 1);
    assert!(find.matches[0].address.starts_with("E"));

    Ok(())
}

fn build_regioned_workbook(book: &mut Spreadsheet) {
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    // Left table
    sheet.get_cell_mut("A1").set_value("Month");
    sheet.get_cell_mut("B1").set_value("Value");
    sheet.get_cell_mut("A2").set_value("Jan");
    sheet.get_cell_mut("B2").set_value_number(10);
    sheet.get_cell_mut("A3").set_value("Feb");
    sheet.get_cell_mut("B3").set_value_number(20);
    sheet.get_cell_mut("A4").set_value("Mar");
    sheet.get_cell_mut("B4").set_value_number(30);

    // Right parameters block separated by gutter at column C
    sheet.get_cell_mut("E1").set_value("Target");
    sheet.get_cell_mut("F1").set_value("Value");
    sheet.get_cell_mut("E2").set_value("North");
    sheet.get_cell_mut("F2").set_value_number(5);
    sheet.get_cell_mut("E3").set_value("South");
    sheet.get_cell_mut("F3").set_value_number(7);
}
