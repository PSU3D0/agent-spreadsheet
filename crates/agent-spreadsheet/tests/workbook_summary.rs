use anyhow::Result;
use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::tools::{
    ListWorkbooksParams, WorkbookSummaryParams, list_workbooks, workbook_summary,
};
use umya_spreadsheet::Workbook as Spreadsheet;

mod support;

#[tokio::test(flavor = "current_thread")]
async fn workbook_summary_reports_regions_and_entry_points() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("summary.xlsx", build_summary_workbook);
    let state = workspace.app_state();

    let workbooks = list_workbooks(
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
    let descriptor = workbooks.workbooks.first().expect("workbook exists");

    let summary = workbook_summary(
        state.clone(),
        WorkbookSummaryParams {
            workbook_or_fork_id: descriptor.workbook_id.clone(),
            summary_only: Some(false),
            include_entry_points: None,
            include_named_ranges: None,
        },
    )
    .await?;

    assert_eq!(summary.sheet_count, 1);
    assert!(summary.total_cells > 0);
    assert!(summary.total_formulas > 0);
    let total_regions = summary.region_counts.data
        + summary.region_counts.parameters
        + summary.region_counts.outputs
        + summary.region_counts.calculator
        + summary.region_counts.metadata
        + summary.region_counts.other;
    assert!(total_regions >= 1);
    assert!(!summary.suggested_entry_points.is_empty());
    assert!(
        summary
            .notes
            .iter()
            .any(|note| note.contains("Region counts and entry points are inferred"))
    );

    Ok(())
}

fn build_summary_workbook(book: &mut Spreadsheet) {
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.get_cell_mut("A1").set_value("Name");
    sheet.get_cell_mut("B1").set_value("Value");
    sheet.get_cell_mut("A2").set_value("Alpha");
    sheet.get_cell_mut("B2").set_value_number(10);
    sheet.get_cell_mut("C4").set_formula("SUM(B2:B2)");
    sheet.get_cell_mut("B3").set_formula("B2*2");
    sheet
        .add_defined_name("KeyValue", "Sheet1!$B$2")
        .expect("define name");
}
