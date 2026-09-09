use anyhow::Result;
use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::model::{SheetPageFormat, TableOutputFormat};
use agent_spreadsheet_mcp::tools::{
    FindValueParams, InspectCellsParams, ListSheetsParams, RangeValuesParams, ReadTableParams,
    SheetOverviewParams, SheetPageParams, TableProfileParams, find_value, inspect_cells,
    list_workbooks, range_values, read_table, sheet_overview, sheet_page, table_profile,
};
use umya_spreadsheet::Workbook as Spreadsheet;

mod support;

#[tokio::test(flavor = "current_thread")]
async fn new_tools_cover_navigation_and_reads() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("inputs.xlsx", build_inputs_workbook);
    let state = workspace.app_state();

    let workbooks = list_workbooks(
        state.clone(),
        agent_spreadsheet_mcp::tools::ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        },
    )
    .await?;
    let descriptor = workbooks.workbooks.first().expect("workbook exists");
    let workbook_id = descriptor.workbook_id.clone();

    let sheets = agent_spreadsheet_mcp::tools::list_sheets(
        state.clone(),
        ListSheetsParams {
            workbook_or_fork_id: workbook_id.clone(),
            limit: None,
            offset: None,
            include_bounds: Some(true),
        },
    )
    .await?;
    let sheets_json = serde_json::to_value(&sheets)?;
    assert!(sheets_json.get("workbook_short_id").is_none());
    let target_summary = sheets
        .sheets
        .iter()
        .find(|s| s.name == "Inputs")
        .expect("inputs sheet exists");
    println!(
        "inputs rows {}, cols {}",
        target_summary.row_count.unwrap_or(0),
        target_summary.column_count.unwrap_or(0)
    );
    let target = target_summary.name.clone();

    let overview = sheet_overview(
        state.clone(),
        SheetOverviewParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: target.clone(),
            max_regions: None,
            max_headers: None,
            include_headers: None,
        },
    )
    .await?;
    assert!(
        !overview.detected_regions.is_empty(),
        "sheet_overview should include detected regions"
    );
    let _region_id = overview.detected_regions[0].id;

    let label_matches = find_value(
        state.clone(),
        FindValueParams {
            workbook_or_fork_id: workbook_id.clone(),
            query: "".into(),
            label: Some("Comp Rate".into()),
            mode: Some(agent_spreadsheet_mcp::model::FindMode::Label),
            sheet_name: Some(target.clone()),
            region_id: None,
            direction: Some(agent_spreadsheet_mcp::model::LabelDirection::Right),
            ..Default::default()
        },
    )
    .await?;
    assert!(
        label_matches.matches.iter().any(|m| {
            m.value
                .as_ref()
                .map(|v| matches!(v, agent_spreadsheet_mcp::model::CellValue::Number(n) if (*n - 175.5).abs() < 0.01))
                .unwrap_or(false)
        }),
        "label mode should return adjacent value, got: {:?}",
        label_matches.matches
    );

    let value_matches = find_value(
        state.clone(),
        FindValueParams {
            workbook_or_fork_id: workbook_id.clone(),
            query: "Widget".into(),
            sheet_name: Some(target.clone()),
            mode: Some(agent_spreadsheet_mcp::model::FindMode::Value),
            ..Default::default()
        },
    )
    .await?;
    assert!(
        value_matches.matches.iter().any(|m| m.sheet_name == target),
        "value mode should find matching cell"
    );

    let table = read_table(
        state.clone(),
        ReadTableParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: Some("Data".into()),
            header_row: Some(1),
            limit: Some(5),
            format: Some(TableOutputFormat::Json),
            ..Default::default()
        },
    )
    .await?;
    let table_json = serde_json::to_value(&table)?;
    assert!(table_json.get("workbook_short_id").is_none());
    assert_eq!(table.headers, vec!["Date", "Revenue", "Cost"]);
    assert_eq!(table.total_rows, 3);
    assert_eq!(table.rows.len(), 3);

    let profile = table_profile(
        state.clone(),
        TableProfileParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: Some("Data".into()),
            region_id: None,
            sample_size: Some(2),
            ..Default::default()
        },
    )
    .await?;
    assert!(!profile.column_types.is_empty());
    assert!(profile.row_count >= 3);

    let ranges = range_values(
        state.clone(),
        RangeValuesParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: "Inputs".into(),
            ranges: vec!["B2".into(), "B3:C3".into()],
            include_headers: Some(true),
            include_formulas: None,
            format: None,
            page_size: None,
        },
    )
    .await?;
    let ranges_json = serde_json::to_value(&ranges)?;
    assert!(ranges_json.get("workbook_short_id").is_none());
    assert_eq!(ranges.values.len(), 2);

    let inspected = inspect_cells(
        state.clone(),
        InspectCellsParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: "Inputs".into(),
            targets: vec!["B2:C3".into()],
            include_empty: Some(true),
            budget: None,
        },
    )
    .await?;
    assert_eq!(inspected.range, "B2:C3");
    assert_eq!(inspected.cells.len(), 4);
    assert!(
        inspected
            .cells
            .iter()
            .any(|cell| cell.address == "B2" && cell.value.is_some())
    );

    let values_only = sheet_page(
        state,
        SheetPageParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: "Data".into(),
            start_row: 1,
            page_size: 10,
            include_formulas: false,
            include_styles: false,
            format: Some(SheetPageFormat::ValuesOnly),
            ..Default::default()
        },
    )
    .await?;
    let values = values_only
        .values_only
        .as_ref()
        .expect("values_only payload present");
    assert!(values.rows.len() >= 3);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn find_value_search_headers_only() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("headers_search.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Name");
        sheet.cell_mut("B1").set_value("Value");
        sheet.cell_mut("A2").set_value("Name");
        sheet.cell_mut("B2").set_value_number(100);
        sheet.cell_mut("A3").set_value("Other");
        sheet.cell_mut("B3").set_value_number(200);
    });
    let state = workspace.app_state();
    let workbooks = list_workbooks(
        state.clone(),
        agent_spreadsheet_mcp::tools::ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: None,
        },
    )
    .await?;
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let all_matches = find_value(
        state.clone(),
        FindValueParams {
            workbook_or_fork_id: workbook_id.clone(),
            query: "Name".into(),
            search_headers_only: false,
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        all_matches.matches.len(),
        2,
        "should find 'Name' in both row 1 and row 2"
    );

    let header_only_matches = find_value(
        state,
        FindValueParams {
            workbook_or_fork_id: workbook_id,
            query: "Name".into(),
            search_headers_only: true,
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        header_only_matches.matches.len(),
        1,
        "should only find 'Name' in header row"
    );
    assert_eq!(header_only_matches.matches[0].address, "A1");

    Ok(())
}

fn build_inputs_workbook(book: &mut Spreadsheet) {
    let inputs = book.sheet_by_name_mut("Sheet1").ok().unwrap();
    inputs.set_name("Inputs");
    inputs.cell_mut("A1").set_value("Label");
    inputs.cell_mut("B1").set_value("Value");
    inputs.cell_mut("A2").set_value("Comp Rate");
    inputs.cell_mut("B2").set_value_number(175.5);
    inputs.cell_mut("A3").set_value("Widget");
    inputs.cell_mut("B3").set_value("Blue");
    inputs.cell_mut("C3").set_value("Note");

    let data = book.new_sheet("Data").expect("data sheet");
    data.cell_mut("A1").set_value("Date");
    data.cell_mut("B1").set_value("Revenue");
    data.cell_mut("C1").set_value("Cost");
    data.cell_mut("A2").set_value("2024-01-01");
    data.cell_mut("B2").set_value_number(100.0);
    data.cell_mut("C2").set_value_number(30.0);
    data.cell_mut("A3").set_value("2024-01-02");
    data.cell_mut("B3").set_value_number(120.0);
    data.cell_mut("C3").set_value_number(40.0);
    data.cell_mut("A4").set_value("2024-01-03");
    data.cell_mut("B4").set_value_number(140.0);
    data.cell_mut("C4").set_value_number(50.0);
}
