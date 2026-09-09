use anyhow::Result;
use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::model::{SheetPageFormat, TableOutputFormat};
use agent_spreadsheet_mcp::tools::{
    FindContext, FindValueParams, ListSheetsParams, ListWorkbooksParams, RangeValuesParams,
    ReadTableParams, SheetPageParams, SheetStatisticsParams, SheetStylesParams, TableProfileParams,
    WorkbookStyleSummaryParams, WorkbookSummaryParams, find_value, list_sheets, list_workbooks,
    range_values, read_table, sheet_page, sheet_statistics, sheet_styles, table_profile,
    workbook_style_summary, workbook_summary,
};
use umya_spreadsheet::Workbook as Spreadsheet;

mod support;

#[tokio::test(flavor = "current_thread")]
async fn read_table_defaults_to_csv() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("defaults.xlsx", build_simple_workbook);
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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let table = read_table(
        state,
        ReadTableParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: Some("Sheet1".into()),
            header_row: Some(1),
            limit: Some(10),
            ..Default::default()
        },
    )
    .await?;

    let csv = table.csv.expect("csv output expected by default");
    assert!(
        csv.lines()
            .next()
            .unwrap_or_default()
            .contains("Name,Value,Flag")
    );
    assert!(table.rows.is_empty());
    assert!(table.values.is_none());
    assert!(table.headers.is_empty());
    assert!(table.next_offset.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn range_values_defaults_to_dense() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("defaults.xlsx", build_simple_workbook);
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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let ranges = range_values(
        state,
        RangeValuesParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".into(),
            ranges: vec!["A1:C2".into()],
            include_headers: Some(false),
            include_formulas: None,
            format: None,
            page_size: None,
        },
    )
    .await?;

    let entry = &ranges.values[0];
    let dense = entry
        .dense
        .as_ref()
        .expect("dense output expected by default");
    assert_eq!(dense.encoding, "dense_v1");
    assert_eq!(dense.col_count, 3);
    assert!(entry.values.is_none());
    assert!(entry.rows.is_none());
    assert!(entry.csv.is_none());
    assert!(entry.next_start_row.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_page_defaults_to_compact() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("defaults.xlsx", build_simple_workbook);
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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let page = sheet_page(
        state,
        SheetPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".into(),
            start_row: 1,
            page_size: 20,
            include_formulas: true,
            include_styles: true,
            format: None,
            ..Default::default()
        },
    )
    .await?;

    assert!(matches!(page.format, SheetPageFormat::Compact));
    assert!(page.compact.is_some());
    assert!(page.rows.is_empty());
    assert!(page.next_start_row.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn read_table_truncates_with_max_cells() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("truncate.xlsx", build_simple_workbook);
    let config = workspace.config_with(|config| {
        config.max_cells = Some(6);
        config.max_payload_bytes = None;
    });
    let state = support::app_state_with_config(config);

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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let table = read_table(
        state,
        ReadTableParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: Some("Sheet1".into()),
            header_row: Some(1),
            limit: Some(50),
            format: Some(TableOutputFormat::Json),
            ..Default::default()
        },
    )
    .await?;

    assert_eq!(table.rows.len(), 2);
    assert!(table.total_rows as usize > table.rows.len());
    assert!(table.next_offset.is_some());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn range_values_truncates_with_max_cells() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("truncate.xlsx", build_simple_workbook);
    let config = workspace.config_with(|config| {
        config.max_cells = Some(6);
        config.max_payload_bytes = None;
    });
    let state = support::app_state_with_config(config);

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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let ranges = range_values(
        state,
        RangeValuesParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".into(),
            ranges: vec!["A1:C6".into()],
            include_headers: Some(false),
            include_formulas: None,
            format: Some(TableOutputFormat::Values),
            page_size: None,
        },
    )
    .await?;

    let entry = &ranges.values[0];
    let values = entry.values.as_ref().expect("values output expected");
    assert_eq!(values.len(), 2);
    assert!(entry.next_start_row.is_some());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_page_truncates_with_max_cells() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("truncate.xlsx", build_simple_workbook);
    let config = workspace.config_with(|config| {
        config.max_cells = Some(6);
        config.max_payload_bytes = None;
    });
    let state = support::app_state_with_config(config);

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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let page = sheet_page(
        state,
        SheetPageParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".into(),
            start_row: 1,
            page_size: 50,
            include_formulas: false,
            include_styles: false,
            format: Some(SheetPageFormat::Full),
            ..Default::default()
        },
    )
    .await?;

    assert_eq!(page.rows.len(), 2);
    assert!(page.next_start_row.is_some());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn list_workbooks_defaults_hide_paths() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("one.xlsx", build_simple_workbook);
    let _path = workspace.create_workbook("two.xlsx", build_simple_workbook);
    let state = workspace.app_state();

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
    .await?;

    assert!(workbooks.next_offset.is_none());
    assert!(workbooks.workbooks.iter().all(|wb| wb.path.is_none()));
    assert!(workbooks.workbooks.iter().all(|wb| wb.caps.is_none()));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn list_workbooks_paginates() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("one.xlsx", build_simple_workbook);
    let _path = workspace.create_workbook("two.xlsx", build_simple_workbook);
    let state = workspace.app_state();

    let first_page = list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: Some(1),
            offset: Some(0),
            include_paths: Some(true),
        },
    )
    .await?;

    assert_eq!(first_page.workbooks.len(), 1);
    assert!(first_page.next_offset.is_some());

    let second_page = list_workbooks(
        state,
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: Some(1),
            offset: first_page.next_offset,
            include_paths: Some(true),
        },
    )
    .await?;

    assert_eq!(second_page.workbooks.len(), 1);
    assert!(second_page.next_offset.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn list_sheets_defaults_hide_bounds() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("multi.xlsx", build_two_sheet_workbook);
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

    let sheets = list_sheets(
        state,
        ListSheetsParams {
            workbook_or_fork_id: workbooks.workbooks[0].workbook_id.clone(),
            limit: None,
            offset: None,
            include_bounds: None,
        },
    )
    .await?;

    assert!(sheets.next_offset.is_none());
    assert!(sheets.sheets.iter().all(|sheet| sheet.row_count.is_none()));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn list_sheets_paginates_with_bounds() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("multi.xlsx", build_two_sheet_workbook);
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

    let first_page = list_sheets(
        state,
        ListSheetsParams {
            workbook_or_fork_id: workbooks.workbooks[0].workbook_id.clone(),
            limit: Some(1),
            offset: Some(0),
            include_bounds: Some(true),
        },
    )
    .await?;

    assert_eq!(first_page.sheets.len(), 1);
    assert!(first_page.next_offset.is_some());
    assert!(first_page.sheets[0].row_count.is_some());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn workbook_summary_defaults_to_summary_only() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("summary.xlsx", build_simple_workbook);
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

    let summary = workbook_summary(
        state,
        WorkbookSummaryParams {
            workbook_or_fork_id: workbooks.workbooks[0].workbook_id.clone(),
            summary_only: None,
            include_entry_points: None,
            include_named_ranges: None,
        },
    )
    .await?;

    assert!(summary.suggested_entry_points.is_empty());
    assert!(summary.key_named_ranges.is_empty());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn table_profile_defaults_to_summary_only() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("profile.xlsx", build_simple_workbook);
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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let profile = table_profile(
        state,
        TableProfileParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: Some("Sheet1".into()),
            summary_only: None,
            ..Default::default()
        },
    )
    .await?;

    assert!(profile.samples.is_empty());
    assert!(!profile.column_types.is_empty());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_statistics_defaults_to_summary_only() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("stats.xlsx", build_simple_workbook);
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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let stats = sheet_statistics(
        state,
        SheetStatisticsParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".into(),
            sample_rows: None,
            summary_only: None,
        },
    )
    .await?;

    let mut columns = stats
        .numeric_columns
        .iter()
        .chain(stats.text_columns.iter());
    let first = columns.next().expect("column");
    assert!(first.samples.is_empty());
    assert!(columns.all(|col| col.samples.is_empty()));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_styles_defaults_to_summary_only() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("styles.xlsx", build_styled_workbook);
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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let styles = sheet_styles(
        state,
        SheetStylesParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".into(),
            scope: None,
            granularity: None,
            max_items: None,
            summary_only: None,
            include_descriptor: None,
            include_ranges: None,
            include_example_cells: None,
        },
    )
    .await?;

    assert!(!styles.styles.is_empty());
    assert!(styles.styles.iter().all(|s| s.descriptor.is_none()));
    assert!(styles.styles.iter().all(|s| s.example_cells.is_empty()));
    assert!(styles.styles.iter().all(|s| s.cell_ranges.is_empty()));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn workbook_style_summary_defaults_to_summary_only() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("summary.xlsx", build_styled_workbook);
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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let summary = workbook_style_summary(
        state,
        WorkbookStyleSummaryParams {
            workbook_or_fork_id: workbook_id,
            max_styles: None,
            max_conditional_formats: None,
            max_cells_scan: None,
            summary_only: None,
            include_descriptor: None,
            include_example_cells: None,
            include_theme: None,
            include_conditional_formats: None,
        },
    )
    .await?;

    assert!(summary.theme.is_none());
    assert!(summary.styles.iter().all(|s| s.descriptor.is_none()));
    assert!(summary.styles.iter().all(|s| s.example_cells.is_empty()));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn table_profile_truncates_with_max_items() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("profile_trunc.xlsx", build_simple_workbook);
    let config = workspace.config_with(|config| {
        config.max_items = Some(1);
        config.max_payload_bytes = None;
    });
    let state = support::app_state_with_config(config);

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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let profile = table_profile(
        state,
        TableProfileParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: Some("Sheet1".into()),
            summary_only: None,
            ..Default::default()
        },
    )
    .await?;

    assert_eq!(profile.headers.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_statistics_truncates_with_max_items() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("stats_trunc.xlsx", build_simple_workbook);
    let config = workspace.config_with(|config| {
        config.max_items = Some(1);
        config.max_payload_bytes = None;
    });
    let state = support::app_state_with_config(config);

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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let stats = sheet_statistics(
        state,
        SheetStatisticsParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".into(),
            sample_rows: None,
            summary_only: None,
        },
    )
    .await?;

    assert_eq!(stats.text_columns.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_styles_truncates_with_max_items() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("styles_trunc.xlsx", build_styled_workbook);
    let config = workspace.config_with(|config| {
        config.max_items = Some(1);
        config.max_payload_bytes = None;
    });
    let state = support::app_state_with_config(config);

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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let styles = sheet_styles(
        state,
        SheetStylesParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".into(),
            scope: None,
            granularity: None,
            max_items: None,
            summary_only: Some(false),
            include_descriptor: None,
            include_ranges: None,
            include_example_cells: None,
        },
    )
    .await?;

    assert_eq!(styles.styles.len(), 1);
    assert!(styles.styles_truncated);
    assert!(styles.total_styles > 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn workbook_style_summary_truncates_with_max_items() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("summary_trunc.xlsx", build_styled_workbook);
    let config = workspace.config_with(|config| {
        config.max_items = Some(1);
        config.max_payload_bytes = None;
    });
    let state = support::app_state_with_config(config);

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
    let workbook_id = workbooks.workbooks[0].workbook_id.clone();

    let summary = workbook_style_summary(
        state,
        WorkbookStyleSummaryParams {
            workbook_or_fork_id: workbook_id,
            max_styles: None,
            max_conditional_formats: None,
            max_cells_scan: None,
            summary_only: Some(false),
            include_descriptor: None,
            include_example_cells: None,
            include_theme: None,
            include_conditional_formats: None,
        },
    )
    .await?;

    assert_eq!(summary.styles.len(), 1);
    assert!(summary.styles_truncated);
    Ok(())
}

fn build_simple_workbook(book: &mut Spreadsheet) {
    let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("Sheet1");
    sheet.cell_mut("A1").set_value("Name");
    sheet.cell_mut("B1").set_value("Value");
    sheet.cell_mut("C1").set_value("Flag");

    for i in 0..5 {
        let row = i + 2;
        sheet
            .cell_mut(format!("A{row}").as_str())
            .set_value(format!("Item {}", i + 1));
        sheet
            .cell_mut(format!("B{row}").as_str())
            .set_value_number((i + 1) as f64);
        sheet
            .cell_mut(format!("C{row}").as_str())
            .set_value("Y");
    }
}

fn build_two_sheet_workbook(book: &mut Spreadsheet) {
    build_simple_workbook(book);
    let _ = book.new_sheet("Sheet2");
}

fn build_styled_workbook(book: &mut Spreadsheet) {
    build_simple_workbook(book);
    let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("Sheet1");
    sheet.style_mut("A2").font_mut().set_bold(true);
    sheet.style_mut("B2").font_mut().set_italic(true);
}

#[tokio::test(flavor = "current_thread")]
async fn find_value_context_defaults_to_none() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("find_context.xlsx", build_simple_workbook);
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

    let result = find_value(
        state,
        FindValueParams {
            workbook_or_fork_id: workbooks.workbooks[0].workbook_id.clone(),
            query: "Item 1".to_string(),
            context: None, // default should be "none"
            ..Default::default()
        },
    )
    .await?;

    assert!(!result.matches.is_empty());
    // With context=none (default), neighbors and row_context should be None
    assert!(result.matches[0].neighbors.is_none());
    assert!(result.matches[0].row_context.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn find_value_context_neighbors() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("find_neighbors.xlsx", build_simple_workbook);
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

    let result = find_value(
        state,
        FindValueParams {
            workbook_or_fork_id: workbooks.workbooks[0].workbook_id.clone(),
            query: "Item 1".to_string(),
            context: Some(FindContext::Neighbors),
            ..Default::default()
        },
    )
    .await?;

    assert!(!result.matches.is_empty());
    // With context=neighbors, neighbors should be Some
    assert!(result.matches[0].neighbors.is_some());
    // row_context should still be None
    assert!(result.matches[0].row_context.is_none());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn find_value_context_row() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("find_row.xlsx", build_simple_workbook);
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

    let result = find_value(
        state,
        FindValueParams {
            workbook_or_fork_id: workbooks.workbooks[0].workbook_id.clone(),
            query: "Item 1".to_string(),
            context: Some(FindContext::Row),
            ..Default::default()
        },
    )
    .await?;

    assert!(!result.matches.is_empty());
    // With context=row, neighbors should be None but row_context populated
    assert!(result.matches[0].neighbors.is_none());
    assert!(result.matches[0].row_context.is_some());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn find_value_context_both() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("find_both.xlsx", build_simple_workbook);
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

    let result = find_value(
        state,
        FindValueParams {
            workbook_or_fork_id: workbooks.workbooks[0].workbook_id.clone(),
            query: "Item 1".to_string(),
            context: Some(FindContext::Both),
            ..Default::default()
        },
    )
    .await?;

    assert!(!result.matches.is_empty());
    // With context=both, both neighbors and row_context should be Some
    assert!(result.matches[0].neighbors.is_some());
    assert!(result.matches[0].row_context.is_some());
    Ok(())
}
