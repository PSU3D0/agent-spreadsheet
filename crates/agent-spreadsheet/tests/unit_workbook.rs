use std::sync::Arc;

use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::tools::filters::WorkbookFilter;
use agent_spreadsheet_mcp::workbook::{WorkbookContext, build_workbook_list};

mod support;

#[test]
fn build_workbook_list_respects_filters() {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("reports/summary.xlsx", |_| {});
    workspace.create_workbook("ops/dashboard.xlsb", |_| {});
    support::touch_file(&workspace.path("other/notes.txt"));

    let config = Arc::new(workspace.config());
    let filter = WorkbookFilter::new(Some("sum".to_string()), Some("reports".to_string()), None)
        .expect("filter");

    let response = build_workbook_list(&config, &filter).expect("list workbooks");
    assert_eq!(response.workbooks.len(), 1);
    let descriptor = &response.workbooks[0];
    assert_eq!(descriptor.slug, "summary");
    assert_eq!(descriptor.folder.as_deref(), Some("reports"));
    assert_eq!(descriptor.path.as_deref(), Some("reports/summary.xlsx"));
    assert!(descriptor.bytes > 0);
    assert!(descriptor.last_modified.is_some());
}

#[test]
fn workbook_context_caches_sheet_metrics() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("metrics.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        for row in 1..=3 {
            for col in 1..=3 {
                sheet
                    .cell_mut((col, row))
                    .set_value_number((row * 10 + col) as i32);
            }
        }
        sheet.cell_mut("A4").set_formula("SUM(A1:A3)");
    });

    let config = Arc::new(workspace.config());
    let context = WorkbookContext::load(&config, &path).expect("load workbook");

    let metrics_first = context.get_sheet_metrics("Sheet1").expect("metrics");
    let metrics_second = context.get_sheet_metrics("Sheet1").expect("metrics");
    assert!(Arc::ptr_eq(&metrics_first, &metrics_second));

    assert_eq!(metrics_first.metrics.non_empty_cells, 10);
    assert_eq!(metrics_first.metrics.formula_cells, 1);
    assert_eq!(metrics_first.metrics.cached_values, 0);

    let summary = context.describe();
    assert_eq!(summary.sheet_count, 1);
    assert_eq!(summary.slug, "metrics");
    assert!(summary.caps.supports_styles);
    assert!(summary.caps.supports_formula_graph);
    assert!(
        summary
            .bytes
            .expect("file-backed descriptor has an exact artifact size")
            > 0
    );
}

#[test]
fn build_workbook_list_single_mode_filters_properly() {
    let workspace = support::TestWorkspace::new();
    let focus_path = workspace.create_workbook("focus/only.xlsx", |_| {});
    workspace.create_workbook("other/ignored.xlsx", |_| {});

    let config = Arc::new(workspace.config_with(|cfg| {
        cfg.single_workbook = Some(focus_path.clone());
    }));
    let filter = WorkbookFilter::default();

    let response = build_workbook_list(&config, &filter).expect("list workbooks");
    assert_eq!(response.workbooks.len(), 1);
    let descriptor = &response.workbooks[0];
    assert_eq!(descriptor.slug, "only");
    assert_eq!(descriptor.folder.as_deref(), Some("focus"));
    assert_eq!(descriptor.path.as_deref(), Some("focus/only.xlsx"));
}

#[test]
fn date_cells_return_iso_format() {
    use agent_spreadsheet_mcp::workbook::cell_to_value;

    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("dates.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        let cell = sheet.cell_mut("A1");
        cell.set_value_number(45597.0);
        cell.style_mut()
            .number_format_mut()
            .set_format_code("yyyy-mm-dd");

        let cell2 = sheet.cell_mut("B1");
        cell2.set_value_number(44986.0);
        cell2
            .style_mut()
            .number_format_mut()
            .set_format_code("mm/dd/yyyy");

        let cell3 = sheet.cell_mut("C1");
        cell3.set_value_number(12345.0);
    });

    let book = umya_spreadsheet::reader::xlsx::read(&path).expect("read");
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();

    let val_a1 = cell_to_value(sheet.cell("A1").unwrap());
    assert!(
        matches!(&val_a1, Some(agent_spreadsheet_mcp::model::CellValue::Date(d)) if d == "2024-11-01"),
        "expected Date(2024-11-01), got {:?}",
        val_a1
    );

    let val_b1 = cell_to_value(sheet.cell("B1").unwrap());
    assert!(
        matches!(&val_b1, Some(agent_spreadsheet_mcp::model::CellValue::Date(d)) if d == "2023-03-01"),
        "expected Date(2023-03-01), got {:?}",
        val_b1
    );

    let val_c1 = cell_to_value(sheet.cell("C1").unwrap());
    assert!(
        matches!(val_c1, Some(agent_spreadsheet_mcp::model::CellValue::Number(_))),
        "expected Number, got {:?}",
        val_c1
    );
}

#[test]
fn excel_serial_date_conversion_edge_cases() {
    use agent_spreadsheet_mcp::workbook::cell_to_value;

    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("edge_dates.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();

        let cell = sheet.cell_mut("A1");
        cell.set_value_number(1.0);
        cell.style_mut()
            .number_format_mut()
            .set_format_code("yyyy-mm-dd");

        let cell2 = sheet.cell_mut("B1");
        cell2.set_value_number(60.0);
        cell2
            .style_mut()
            .number_format_mut()
            .set_format_code("yyyy-mm-dd");

        let cell3 = sheet.cell_mut("C1");
        cell3.set_value_number(61.0);
        cell3
            .style_mut()
            .number_format_mut()
            .set_format_code("yyyy-mm-dd");
    });

    let book = umya_spreadsheet::reader::xlsx::read(&path).expect("read");
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();

    let val_a1 = cell_to_value(sheet.cell("A1").unwrap());
    assert!(
        matches!(&val_a1, Some(agent_spreadsheet_mcp::model::CellValue::Date(d)) if d == "1900-01-01"),
        "serial 1 should be 1900-01-01, got {:?}",
        val_a1
    );

    let val_c1 = cell_to_value(sheet.cell("C1").unwrap());
    assert!(
        matches!(&val_c1, Some(agent_spreadsheet_mcp::model::CellValue::Date(d)) if d == "1900-03-01"),
        "serial 61 should be 1900-03-01, got {:?}",
        val_c1
    );
}

#[test]
fn formula_graph_extracts_precedents() {
    use agent_spreadsheet_mcp::analysis::formula::{FormulaAtlas, FormulaGraph};

    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("formulas.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value_number(10.0);
        sheet.cell_mut("A2").set_value_number(20.0);
        sheet.cell_mut("B1").set_formula("A1+A2");
        sheet.cell_mut("C1").set_formula("SUM(A1:A2)");
    });

    let ctx = WorkbookContext::load(&workspace.config().into(), &path).unwrap();
    let atlas = FormulaAtlas::default();
    let graph = ctx
        .with_sheet("Sheet1", |sheet| {
            FormulaGraph::build(
                sheet,
                &atlas,
                agent_spreadsheet_mcp::model::FormulaParsePolicy::Warn,
                None,
            )
        })
        .unwrap()
        .unwrap();

    let b1_precedents = graph.precedents("B1");
    let c1_precedents = graph.precedents("C1");

    assert!(
        !b1_precedents.is_empty(),
        "B1 formula A1+A2 should have precedents"
    );
    assert!(b1_precedents.contains(&"A1".to_string()));
    assert!(b1_precedents.contains(&"A2".to_string()));

    assert!(
        !c1_precedents.is_empty(),
        "C1 formula SUM(A1:A2) should have precedents"
    );
    assert!(
        c1_precedents.contains(&"A1".to_string()),
        "C1 should have A1 as precedent (range expanded)"
    );
    assert!(
        c1_precedents.contains(&"A2".to_string()),
        "C1 should have A2 as precedent (range expanded)"
    );

    let a1_dependents = graph.dependents("A1");
    assert!(
        a1_dependents.contains(&"B1".to_string()),
        "A1 should have B1 as dependent"
    );
    assert!(
        a1_dependents.contains(&"C1".to_string()),
        "A1 should have C1 as dependent (from expanded range)"
    );
}

#[test]
fn large_range_dependents_found_via_containment() {
    use agent_spreadsheet_mcp::analysis::formula::{FormulaAtlas, FormulaGraph};

    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("large_range.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        for row in 1..=600 {
            sheet.cell_mut((1, row)).set_value_number(row as f64);
        }
        sheet.cell_mut("B1").set_formula("SUM(A1:A600)");
    });

    let ctx = WorkbookContext::load(&workspace.config().into(), &path).unwrap();
    let atlas = FormulaAtlas::default();
    let graph = ctx
        .with_sheet("Sheet1", |sheet| {
            FormulaGraph::build(
                sheet,
                &atlas,
                agent_spreadsheet_mcp::model::FormulaParsePolicy::Warn,
                None,
            )
        })
        .unwrap()
        .unwrap();

    let a300_dependents = graph.dependents("A300");
    assert!(
        a300_dependents.contains(&"B1".to_string()),
        "A300 should have B1 as dependent via large range containment check"
    );

    let a1_dependents = graph.dependents("A1");
    assert!(
        a1_dependents.contains(&"B1".to_string()),
        "A1 should have B1 as dependent"
    );

    let a600_dependents = graph.dependents("A600");
    assert!(
        a600_dependents.contains(&"B1".to_string()),
        "A600 should have B1 as dependent"
    );

    let a601_dependents = graph.dependents("A601");
    assert!(
        !a601_dependents.contains(&"B1".to_string()),
        "A601 should NOT have B1 as dependent (outside range)"
    );
}

#[test]
fn cross_sheet_dependents_traced() {
    use agent_spreadsheet_mcp::analysis::formula::{FormulaAtlas, FormulaGraph};

    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("cross_sheet.xlsx", |book| {
        let data_sheet = book.new_sheet("Data").unwrap();
        data_sheet.cell_mut("A1").set_value_number(100.0);
        data_sheet.cell_mut("A2").set_value_number(200.0);

        let calc_sheet = book.new_sheet("Calc").unwrap();
        calc_sheet.cell_mut("B1").set_formula("Data!A1+Data!A2");
        calc_sheet.cell_mut("B2").set_formula("SUM(Data!A1:A2)");
    });

    let ctx = WorkbookContext::load(&workspace.config().into(), &path).unwrap();
    let atlas = FormulaAtlas::default();

    let calc_graph = ctx
        .with_sheet("Calc", |sheet| {
            FormulaGraph::build(
                sheet,
                &atlas,
                agent_spreadsheet_mcp::model::FormulaParsePolicy::Warn,
                None,
            )
        })
        .unwrap()
        .unwrap();

    let b1_precs = calc_graph.precedents("B1");
    assert!(
        b1_precs
            .iter()
            .any(|p| p.contains("Data") && p.contains("A1")),
        "B1 should have Data!A1 as precedent: {:?}",
        b1_precs
    );

    let b2_precs = calc_graph.precedents("B2");
    assert!(
        b2_precs
            .iter()
            .any(|p| p.contains("Data") && p.contains("A1")),
        "B2 should have Data!A1 as precedent (expanded): {:?}",
        b2_precs
    );
}

#[test]
fn dependents_are_deduplicated() {
    use agent_spreadsheet_mcp::analysis::formula::{FormulaAtlas, FormulaGraph};

    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("dedup.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        for row in 1..=10 {
            sheet.cell_mut((1, row)).set_value_number(row as f64);
        }
        // Formula references A1:A10 THREE times - should only appear once in dependents
        sheet
            .cell_mut("B1")
            .set_formula("SUM(A1:A10)+SUM(A1:A10)+SUM(A1:A10)");
    });

    let ctx = WorkbookContext::load(&workspace.config().into(), &path).unwrap();
    let atlas = FormulaAtlas::default();
    let graph = ctx
        .with_sheet("Sheet1", |sheet| {
            FormulaGraph::build(
                sheet,
                &atlas,
                agent_spreadsheet_mcp::model::FormulaParsePolicy::Warn,
                None,
            )
        })
        .unwrap()
        .unwrap();

    let a5_dependents = graph.dependents("A5");
    let b1_count = a5_dependents.iter().filter(|d| *d == "B1").count();
    assert_eq!(
        b1_count, 1,
        "B1 should appear exactly once in A5's dependents, not {} times. Dependents: {:?}",
        b1_count, a5_dependents
    );

    let a1_dependents = graph.dependents("A1");
    let b1_count_a1 = a1_dependents.iter().filter(|d| *d == "B1").count();
    assert_eq!(
        b1_count_a1, 1,
        "B1 should appear exactly once in A1's dependents, not {} times",
        b1_count_a1
    );
}

mod date_conversion {
    use agent_spreadsheet::workbook::{excel_serial_to_iso, excel_serial_to_iso_with_leap_bug};

    #[test]
    fn excel_1900_system_basic_dates() {
        assert_eq!(excel_serial_to_iso(1.0, false), "1900-01-01");
        assert_eq!(excel_serial_to_iso(2.0, false), "1900-01-02");
        assert_eq!(excel_serial_to_iso(59.0, false), "1900-02-28");
        assert_eq!(excel_serial_to_iso(61.0, false), "1900-03-01");
    }

    #[test]
    fn excel_1900_leap_year_bug_serial_60() {
        assert_eq!(
            excel_serial_to_iso(60.0, false),
            "1900-02-28",
            "Serial 60 compensated to Feb 28 (skipping Excel's fake Feb 29)"
        );
    }

    #[test]
    fn excel_1900_leap_year_bug_compensation_disabled() {
        assert_eq!(
            excel_serial_to_iso_with_leap_bug(60.0, false, false),
            "1900-03-01",
            "Without compensation, serial 60 is March 1"
        );
        assert_eq!(
            excel_serial_to_iso_with_leap_bug(61.0, false, false),
            "1900-03-02",
            "Without compensation, serial 61 is March 2"
        );
    }

    #[test]
    fn excel_1900_modern_dates() {
        assert_eq!(excel_serial_to_iso(44197.0, false), "2021-01-01");
        assert_eq!(excel_serial_to_iso(45292.0, false), "2024-01-01");
        assert_eq!(excel_serial_to_iso(45658.0, false), "2025-01-01");
    }

    #[test]
    fn excel_1904_system_basic_dates() {
        assert_eq!(excel_serial_to_iso(0.0, true), "1904-01-01");
        assert_eq!(excel_serial_to_iso(1.0, true), "1904-01-02");
        assert_eq!(excel_serial_to_iso(366.0, true), "1905-01-01");
    }

    #[test]
    fn excel_1904_modern_dates() {
        assert_eq!(excel_serial_to_iso(42735.0, true), "2021-01-01");
        assert_eq!(excel_serial_to_iso(43830.0, true), "2024-01-01");
    }

    #[test]
    fn dates_before_leap_bug_serial() {
        assert_eq!(excel_serial_to_iso(59.0, false), "1900-02-28");
        assert_eq!(
            excel_serial_to_iso_with_leap_bug(59.0, false, true),
            "1900-02-28"
        );
        assert_eq!(
            excel_serial_to_iso_with_leap_bug(59.0, false, false),
            "1900-02-28"
        );
    }

    #[test]
    fn fractional_serials_truncated() {
        assert_eq!(excel_serial_to_iso(44197.5, false), "2021-01-01");
        assert_eq!(excel_serial_to_iso(44197.999, false), "2021-01-01");
    }
}
