use anyhow::Result;
use serde_json::json;
use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::model::{CellValue, TableOutputFormat};
use agent_spreadsheet_mcp::tools::{FilterOp, SampleMode, TableFilter};
use agent_spreadsheet_mcp::tools::{ListWorkbooksParams, ReadTableParams, list_workbooks, read_table};

mod support;

#[tokio::test(flavor = "current_thread")]
async fn read_table_uses_region_header_hint_and_range_offsets() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("region_header.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        // leave some blank rows to force trim; header on row 5
        sheet.get_cell_mut("A5").set_value("ColA");
        sheet.get_cell_mut("B5").set_value("ColB");
        sheet.get_cell_mut("A6").set_value("R1");
        sheet.get_cell_mut("B6").set_value_number(1);
        sheet.get_cell_mut("A7").set_value("R2");
        sheet.get_cell_mut("B7").set_value_number(2);
    });
    let state = workspace.app_state();
    let descriptor = list_workbooks(
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
    .remove(0);
    let workbook_id = descriptor.workbook_id;

    // get region id via sheet_overview to ensure detection ran
    let overview = agent_spreadsheet_mcp::tools::sheet_overview(
        state.clone(),
        agent_spreadsheet_mcp::tools::SheetOverviewParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: "Sheet1".into(),
            max_regions: None,
            max_headers: None,
            include_headers: None,
        },
    )
    .await?;
    let region_id = overview.detected_regions[0].id;

    let table = read_table(
        state.clone(),
        ReadTableParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: Some("Sheet1".into()),
            region_id: Some(region_id),
            limit: Some(10),
            format: Some(TableOutputFormat::Json),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(table.headers, vec!["ColA", "ColB"]);
    assert_eq!(table.total_rows, 2);

    // Explicit offset header row should still work
    let ranged = read_table(
        state,
        ReadTableParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: Some("Sheet1".into()),
            range: Some("A5:B7".into()),
            header_row: Some(5),
            limit: Some(10),
            format: Some(TableOutputFormat::Json),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(ranged.headers, vec!["ColA", "ColB"]);
    assert_eq!(ranged.total_rows, 2);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn read_table_handles_multi_row_headers_and_filters() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("multi_headers.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("Group");
        sheet.get_cell_mut("B1").set_value("Group");
        sheet.get_cell_mut("A2").set_value("X");
        sheet.get_cell_mut("B2").set_value("Y");
        sheet.get_cell_mut("A3").set_value("foo");
        sheet.get_cell_mut("B3").set_value_number(10);
        sheet.get_cell_mut("A4").set_value("bar");
        sheet.get_cell_mut("B4").set_value_number(20);
    });
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

    let table = read_table(
        state.clone(),
        ReadTableParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: Some("Sheet1".into()),
            header_rows: Some(2),
            filters: Some(vec![TableFilter {
                column: "Group / Y".into(),
                op: FilterOp::Gt,
                value: json!(15),
            }]),
            sample_mode: Some(SampleMode::Distributed),
            limit: Some(2),
            format: Some(TableOutputFormat::Json),
            ..Default::default()
        },
    )
    .await?;
    eprintln!(
        "headers {:?}, total_rows {}, rows {:?}",
        table.headers, table.total_rows, table.rows
    );
    assert_eq!(table.headers, vec!["Group / X", "Group / Y"]);
    assert_eq!(table.total_rows, 1);
    assert_eq!(table.rows.len(), 1);
    let only = table.rows.first().unwrap();
    assert!(matches!(
        only.get("Group / Y").and_then(|v| v.as_ref()),
        Some(agent_spreadsheet_mcp::model::CellValue::Number(n)) if (*n - 20.0).abs() < 0.01
    ));

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn read_table_expands_merged_headers_and_in_filters() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("merged.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("Q1");
        sheet.add_merge_cells("A1:B1");
        sheet.get_cell_mut("A2").set_value("Name");
        sheet.get_cell_mut("B2").set_value("Value");
        sheet.get_cell_mut("A3").set_value("alpha");
        sheet.get_cell_mut("B3").set_value_number(1);
        sheet.get_cell_mut("A4").set_value("beta");
        sheet.get_cell_mut("B4").set_value_number(2);
    });
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

    let table = read_table(
        state.clone(),
        ReadTableParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: Some("Sheet1".into()),
            header_rows: Some(2),
            filters: Some(vec![TableFilter {
                column: "Q1 / Value".into(),
                op: FilterOp::In,
                value: json!([1, "3"]),
            }]),
            limit: Some(5),
            format: Some(TableOutputFormat::Json),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(table.headers, vec!["Q1 / Name", "Q1 / Value"]);
    assert_eq!(table.total_rows, 1);

    let neq = read_table(
        state,
        ReadTableParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: Some("Sheet1".into()),
            header_rows: Some(2),
            filters: Some(vec![TableFilter {
                column: "Q1 / Name".into(),
                op: FilterOp::Neq,
                value: json!("alpha"),
            }]),
            limit: Some(5),
            format: Some(TableOutputFormat::Json),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(neq.total_rows, 1);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn read_table_large_range_stops_after_limit_and_counts() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("large.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("X");
        sheet.get_cell_mut("B1").set_value("Y");
        for row in 2..=200 {
            sheet.get_cell_mut((1u32, row)).set_value(format!("r{row}"));
            sheet.get_cell_mut((2u32, row)).set_value_number(row as i32);
        }
    });
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

    let table = read_table(
        state,
        ReadTableParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: Some("Sheet1".into()),
            header_row: Some(1),
            filters: Some(vec![TableFilter {
                column: "Y".into(),
                op: FilterOp::Gt,
                value: json!(50),
            }]),
            limit: Some(5),
            offset: Some(1),
            sample_mode: Some(SampleMode::First),
            format: Some(TableOutputFormat::Json),
            ..Default::default()
        },
    )
    .await?;

    assert_eq!(table.headers, vec!["X", "Y"]);
    assert!(table.total_rows >= 148);
    assert_eq!(table.rows.len(), 5);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn read_table_handles_huge_sheet_sampling() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("huge.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("Idx");
        sheet.get_cell_mut("B1").set_value("Value");
        for row in 2..=10001 {
            sheet
                .get_cell_mut((1u32, row))
                .set_value_number((row - 1) as i32);
            sheet
                .get_cell_mut((2u32, row))
                .set_value_number(((row - 1) * 2) as i32);
        }
    });
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

    let table = read_table(
        state,
        ReadTableParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: Some("Sheet1".into()),
            header_row: Some(1),
            limit: Some(10),
            offset: Some(5),
            sample_mode: Some(SampleMode::First),
            format: Some(TableOutputFormat::Json),
            ..Default::default()
        },
    )
    .await?;

    assert_eq!(table.headers, vec!["Idx", "Value"]);
    assert_eq!(table.rows.len(), 10);
    assert!(table.total_rows >= 9990);
    assert!(table.next_offset.is_some());

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn read_table_handles_empty_header_cells_in_multi_row() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("empty_headers.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("Category");
        sheet.get_cell_mut("A2").set_value("Sub");
        sheet.get_cell_mut("B1").set_value("Values");
        sheet.get_cell_mut("C1").set_value("Values");
        sheet.get_cell_mut("C2").set_value("Amt");
        sheet.get_cell_mut("A3").set_value("foo");
        sheet.get_cell_mut("B3").set_value_number(1);
        sheet.get_cell_mut("C3").set_value_number(100);
    });
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

    let table = read_table(
        state,
        ReadTableParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: Some("Sheet1".into()),
            header_rows: Some(2),
            limit: Some(10),
            format: Some(TableOutputFormat::Json),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(table.headers.len(), 3);
    assert!(table.headers[0].contains("Category"));
    assert!(table.headers[0].contains("Sub"));
    assert!(table.headers[1].contains("Values"));
    assert!(table.headers[2].contains("Values"));
    assert!(table.headers[2].contains("Amt"));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn read_table_filter_contains_case_insensitive() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("contains.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("A2").set_value("Apple Pie");
        sheet.get_cell_mut("A3").set_value("Banana Bread");
        sheet.get_cell_mut("A4").set_value("Cherry Cake");
    });
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

    let table = read_table(
        state,
        ReadTableParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: Some("Sheet1".into()),
            filters: Some(vec![TableFilter {
                column: "Name".into(),
                op: FilterOp::Contains,
                value: json!("BREAD"),
            }]),
            limit: Some(10),
            format: Some(TableOutputFormat::Json),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(table.total_rows, 1);
    let row = table.rows.first().unwrap();
    assert!(matches!(
        row.get("Name").and_then(|v| v.as_ref()),
        Some(CellValue::Text(s)) if s.contains("Banana")
    ));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn read_table_resolves_excel_table_by_name() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let _path = workspace.create_workbook("with_table.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("B2").set_value("ID");
        sheet.get_cell_mut("C2").set_value("Amount");
        sheet.get_cell_mut("B3").set_value_number(1);
        sheet.get_cell_mut("C3").set_value_number(100);
        sheet.get_cell_mut("B4").set_value_number(2);
        sheet.get_cell_mut("C4").set_value_number(200);
        let mut table = umya_spreadsheet::structs::Table::new("SalesData", ("B2", "C4"));
        table.set_display_name("SalesData");
        sheet.add_table(table);
    });
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

    let table = read_table(
        state,
        ReadTableParams {
            workbook_or_fork_id: workbook_id,
            table_name: Some("SalesData".into()),
            limit: Some(10),
            format: Some(TableOutputFormat::Json),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(table.table_name, Some("SalesData".into()));
    assert_eq!(table.headers, vec!["ID", "Amount"]);
    assert_eq!(table.total_rows, 2);
    Ok(())
}
