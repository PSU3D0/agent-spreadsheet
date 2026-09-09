use anyhow::Result;
use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::tools::{
    ListWorkbooksParams, SheetOverviewParams, list_workbooks, sheet_overview,
};
use support::builders::{CellVal, fill_table};
use umya_spreadsheet::Workbook as Spreadsheet;

mod support;

#[tokio::test(flavor = "current_thread")]
async fn sheet_overview_truncates_regions_and_sets_counts() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("overview_truncate.xlsx", build_multi_region_workbook);
    let state = workspace.app_state();

    let workbook_id = list_workbooks(
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
    .await?
    .workbooks
    .remove(0)
    .workbook_id;

    let overview = sheet_overview(
        state,
        SheetOverviewParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            max_regions: Some(1),
            max_headers: None,
            include_headers: Some(true),
        },
    )
    .await?;

    assert_eq!(overview.detected_regions.len(), 1);
    assert!(overview.detected_regions_truncated);
    assert!(overview.detected_region_count > overview.detected_regions.len() as u32);
    assert!(
        overview
            .notes
            .iter()
            .any(|note| note.contains("Detected regions truncated"))
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_overview_truncates_headers_and_sets_flags() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("overview_headers.xlsx", build_wide_header_workbook);
    let state = workspace.app_state();

    let workbook_id = list_workbooks(
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
    .await?
    .workbooks
    .remove(0)
    .workbook_id;

    let overview = sheet_overview(
        state,
        SheetOverviewParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            max_regions: None,
            max_headers: Some(3),
            include_headers: Some(true),
        },
    )
    .await?;

    let region = overview.detected_regions.first().expect("region exists");
    assert_eq!(region.header_count, 8);
    assert_eq!(region.headers.len(), 3);
    assert!(region.headers_truncated);
    assert!(
        overview
            .notes
            .iter()
            .any(|note| note.contains("Region headers truncated"))
    );

    Ok(())
}

fn build_multi_region_workbook(book: &mut Spreadsheet) {
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();

    let headers = ["A", "B"];
    let rows = vec![
        vec![CellVal::Num(1.0), CellVal::Num(2.0)],
        vec![CellVal::Num(3.0), CellVal::Num(4.0)],
    ];
    fill_table(sheet, "A1", &headers, &rows);

    let rows_right = vec![
        vec![CellVal::Num(5.0), CellVal::Num(6.0)],
        vec![CellVal::Num(7.0), CellVal::Num(8.0)],
    ];
    fill_table(sheet, "F1", &headers, &rows_right);

    let rows_lower = vec![
        vec![CellVal::Num(9.0), CellVal::Num(10.0)],
        vec![CellVal::Num(11.0), CellVal::Num(12.0)],
    ];
    fill_table(sheet, "A7", &headers, &rows_lower);
}

fn build_wide_header_workbook(book: &mut Spreadsheet) {
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();

    let headers: Vec<String> = (1..=8).map(|i| format!("Col{}", i)).collect();
    let header_refs: Vec<&str> = headers.iter().map(|s| s.as_str()).collect();
    let row1: Vec<CellVal> = (1..=8).map(|i| CellVal::Num(i as f64)).collect();
    let row2: Vec<CellVal> = (1..=8).map(|i| CellVal::Num((i * 10) as f64)).collect();
    let rows = vec![row1, row2];
    fill_table(sheet, "A1", &header_refs, &rows);
}
