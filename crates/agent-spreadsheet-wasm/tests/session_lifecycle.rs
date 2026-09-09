#![cfg(not(target_arch = "wasm32"))]

use agent_spreadsheet::core::session::{SessionMatrixCell, SessionTransformOp, WorkbookSession};
use agent_spreadsheet::model::CellValue;
use agent_spreadsheet_wasm::{
    FindValueParams, GridExportParams, RangeSelectionInput, RangeValuesParams, ReadTableParams,
    SessionApi, SessionApiError, SheetOverviewParams, SheetPageParams, TransformBatchOptions,
};

fn workbook_bytes(setup: impl FnOnce(&mut umya_spreadsheet::Workbook)) -> Vec<u8> {
    let mut book = umya_spreadsheet::new_file();
    setup(&mut book);

    let mut bytes = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&book, &mut bytes).expect("write workbook");
    bytes
}

#[test]
fn session_lifecycle_reads_and_disposes() {
    let bytes = workbook_bytes(|book| {
        book.sheet_by_name_mut("Sheet1").ok()
            .expect("sheet")
            .cell_mut("A1")
            .set_value("hello");
    });

    let api = SessionApi::new();
    let session_id = api.create_session(&bytes).expect("create session");

    let sheets = api.list_sheets(&session_id).expect("list sheets");
    assert_eq!(sheets, vec!["Sheet1"]);

    let desc = api
        .describe_workbook(&session_id)
        .expect("describe workbook");
    assert_eq!(desc.workbook_id.as_str(), session_id);

    let named = api.named_ranges(&session_id).expect("named ranges");
    assert_eq!(named.workbook_id.as_str(), session_id);

    let overview = api
        .sheet_overview(
            &session_id,
            SheetOverviewParams {
                sheet_name: "Sheet1".to_string(),
                max_regions: Some(1),
                max_headers: Some(1),
                include_headers: Some(true),
            },
        )
        .expect("sheet overview");
    assert_eq!(overview.workbook_id.as_str(), session_id);
    assert_eq!(overview.sheet_name, "Sheet1");

    let find = api
        .find_value(
            &session_id,
            FindValueParams {
                query: "hello".to_string(),
                sheet_name: Some("Sheet1".to_string()),
                case_sensitive: Some(false),
                limit: Some(10),
                offset: Some(0),
            },
        )
        .expect("find value");
    assert_eq!(find.workbook_id.as_str(), session_id);
    assert_eq!(find.matches.len(), 1);

    let table = api
        .read_table(
            &session_id,
            ReadTableParams {
                sheet_name: Some("Sheet1".to_string()),
                range: Some("A1:A1".to_string()),
                columns: None,
                limit: Some(10),
                offset: Some(0),
                format: Some(agent_spreadsheet::model::TableOutputFormat::Json),
                include_headers: Some(true),
                include_types: Some(false),
            },
        )
        .expect("read table");
    assert_eq!(table.workbook_id.as_str(), session_id);

    let values = api
        .range_values(
            &session_id,
            RangeValuesParams {
                sheet_name: "Sheet1".to_string(),
                ranges: RangeSelectionInput::Single("A1:A1".to_string()),
            },
        )
        .expect("range values");
    let rows = values.values[0].rows.as_ref().expect("rows");
    assert!(matches!(
        rows[0][0],
        Some(CellValue::Text(ref value)) if value == "hello"
    ));

    let grid = api
        .grid_export(
            &session_id,
            GridExportParams {
                sheet_name: "Sheet1".to_string(),
                range: "A1:A1".to_string(),
            },
        )
        .expect("grid export");
    assert_eq!(grid.rows.len(), 1);

    assert!(api.dispose_session(&session_id).expect("dispose"));
    assert!(
        !api.dispose_session(&session_id)
            .expect("dispose second time")
    );

    let err = api.list_sheets(&session_id).expect_err("session removed");
    assert!(matches!(err, SessionApiError::SessionNotFound { .. }));
    assert_eq!(err.code(), "SESSION_NOT_FOUND");
}

#[test]
fn sheet_page_reads_real_session_data() {
    let bytes = workbook_bytes(|book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
        sheet.cell_mut("A1").set_value("Name");
        sheet.cell_mut("B1").set_value("Score");
        sheet.cell_mut("A2").set_value("alpha");
        sheet.cell_mut("B2").set_value_number(42.0);
        sheet.cell_mut("A3").set_value("beta");
        sheet.cell_mut("B3").set_value_number(7.0);
    });

    let api = SessionApi::new();
    let session_id = api.create_session(&bytes).expect("create session");

    let page = api
        .sheet_page(
            &session_id,
            SheetPageParams {
                sheet_name: "Sheet1".to_string(),
                start_row: Some(2),
                page_size: Some(1),
                columns: None,
                columns_by_header: Some(vec!["score".to_string()]),
                include_formulas: Some(false),
                include_styles: Some(false),
                include_header: Some(true),
                format: Some(agent_spreadsheet::model::SheetPageFormat::Compact),
            },
        )
        .expect("sheet page");

    assert_eq!(page.sheet_name, "Sheet1");
    assert_eq!(page.workbook_id.as_str(), session_id);
    assert_eq!(page.next_start_row, Some(3));

    let compact = page.compact.expect("compact payload");
    assert_eq!(compact.headers, vec!["Row", "Score"]);
    assert_eq!(compact.rows.len(), 1);
    assert!(matches!(
        compact.rows[0][1],
        Some(CellValue::Number(n)) if (n - 42.0).abs() < f64::EPSILON
    ));
}

#[test]
fn transform_batch_roundtrip_and_dry_run() {
    let bytes = workbook_bytes(|book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
        sheet.cell_mut("A1").set_value("before");
        sheet.cell_mut("B1").set_formula("1+1");
    });

    let api = SessionApi::new();
    let session_id = api.create_session(&bytes).expect("create session");

    let dry_run_summary = api
        .transform_batch(
            &session_id,
            vec![SessionTransformOp::WriteMatrix {
                sheet_name: "Sheet1".to_string(),
                anchor: "A1".to_string(),
                rows: vec![vec![Some(SessionMatrixCell::Value(serde_json::json!(
                    "preview"
                )))]],
                overwrite_formulas: false,
            }],
            TransformBatchOptions { dry_run: true },
        )
        .expect("dry run summary");
    assert_eq!(dry_run_summary.ops_applied, 1);

    let before_apply = api
        .range_values(
            &session_id,
            RangeValuesParams {
                sheet_name: "Sheet1".to_string(),
                ranges: RangeSelectionInput::Single("A1:A1".to_string()),
            },
        )
        .expect("before apply");
    let before_rows = before_apply.values[0].rows.as_ref().expect("rows");
    assert!(matches!(
        before_rows[0][0],
        Some(CellValue::Text(ref value)) if value == "before"
    ));

    let summary = api
        .transform_batch(
            &session_id,
            vec![SessionTransformOp::WriteMatrix {
                sheet_name: "Sheet1".to_string(),
                anchor: "A1".to_string(),
                rows: vec![vec![
                    Some(SessionMatrixCell::Value(serde_json::json!("after"))),
                    Some(SessionMatrixCell::Value(serde_json::json!(99))),
                ]],
                overwrite_formulas: false,
            }],
            TransformBatchOptions::default(),
        )
        .expect("write matrix");
    assert_eq!(summary.cells_skipped_keep_formulas, 1);

    api.transform_batch(
        &session_id,
        vec![SessionTransformOp::WriteMatrix {
            sheet_name: "Sheet1".to_string(),
            anchor: "B1".to_string(),
            rows: vec![vec![Some(SessionMatrixCell::Formula(
                "=SUM(1,2)".to_string(),
            ))]],
            overwrite_formulas: true,
        }],
        TransformBatchOptions::default(),
    )
    .expect("set formula");

    let exported = api.export_workbook(&session_id).expect("export bytes");
    let reopened = WorkbookSession::from_bytes(exported).expect("reopen bytes");
    let payload = reopened.grid_export("Sheet1", "A1:B1").expect("grid");

    let a1 = payload
        .rows
        .iter()
        .flat_map(|row| row.cells.iter())
        .find(|cell| cell.offset == [0, 0])
        .expect("A1 cell");
    let b1 = payload
        .rows
        .iter()
        .flat_map(|row| row.cells.iter())
        .find(|cell| cell.offset == [0, 1])
        .expect("B1 cell");

    assert_eq!(a1.v, Some(serde_json::json!("after")));
    assert_eq!(b1.f.as_deref(), Some("=SUM(1,2)"));
}
