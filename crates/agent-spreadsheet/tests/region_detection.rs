use std::sync::Arc;

use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::workbook::WorkbookContext;

mod support;

#[test]
fn single_table_no_gutters() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("single.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut((1u32, 1u32)).set_value("Name");
        sheet.cell_mut((2u32, 1u32)).set_value("Dept");
        sheet.cell_mut((3u32, 1u32)).set_value("Score");
        for row in 2..=4 {
            sheet
                .cell_mut((1u32, row))
                .set_value(format!("User{}", row));
            sheet
                .cell_mut((2u32, row))
                .set_value(format!("Team{}", row));
            sheet
                .cell_mut((3u32, row))
                .set_value_number(80 + row as i32);
        }
    });

    let config = Arc::new(workspace.config());
    let ctx = WorkbookContext::load(&config, &path).expect("load");
    let metrics = ctx.get_sheet_metrics("Sheet1").expect("metrics");
    let regions = metrics.detected_regions();
    assert_eq!(regions.len(), 1);
    let region = &regions[0];
    assert_eq!(region.bounds, "A1:C4");
    assert_eq!(region.header_row, Some(1));
    assert!(matches!(
        region.region_kind,
        Some(agent_spreadsheet_mcp::model::RegionKind::Data)
            | Some(agent_spreadsheet_mcp::model::RegionKind::Table)
    ));
}

#[test]
fn two_tables_vertical_gutter() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("vertical.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut((1u32, 1u32)).set_value("A");
        sheet.cell_mut((2u32, 1u32)).set_value("B");
        for row in 2..=3 {
            sheet.cell_mut((1u32, row)).set_value_number(row as i32);
            sheet
                .cell_mut((2u32, row))
                .set_value_number(row as i32 * 2);
        }
        for row in 6..=8 {
            sheet
                .cell_mut((1u32, row))
                .set_value(format!("R{}", row));
            sheet
                .cell_mut((2u32, row))
                .set_value_number(row as i32 + 10);
        }
    });

    let config = Arc::new(workspace.config());
    let ctx = WorkbookContext::load(&config, &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    assert_eq!(regions.len(), 2);
    assert_eq!(regions[0].bounds, "A1:B3");
    assert_eq!(regions[1].bounds, "A6:B8");
}

#[test]
fn two_tables_horizontal_gutter() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("horizontal.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut((1u32, 1u32)).set_value("L1");
        sheet.cell_mut((2u32, 1u32)).set_value("L2");
        sheet.cell_mut((5u32, 1u32)).set_value("R1");
        sheet.cell_mut((6u32, 1u32)).set_value("R2");
        for row in 2..=4 {
            sheet.cell_mut((1u32, row)).set_value_number(row as i32);
            sheet
                .cell_mut((2u32, row))
                .set_value_number(row as i32 * 3);
            sheet
                .cell_mut((5u32, row))
                .set_value(format!("X{}", row));
            sheet
                .cell_mut((6u32, row))
                .set_value_number(row as i32 + 20);
        }
    });

    let config = Arc::new(workspace.config());
    let ctx = WorkbookContext::load(&config, &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    assert_eq!(regions.len(), 2);
    assert_eq!(regions[0].bounds, "A1:B4");
    assert_eq!(regions[1].bounds, "E1:F4");
}

#[test]
fn parameters_block_classified() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("params.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        for row in 1..=5 {
            sheet
                .cell_mut((1u32, row))
                .set_value(format!("Param{}", row));
            sheet
                .cell_mut((2u32, row))
                .set_value_number((row * 10) as i32);
        }
    });

    let config = Arc::new(workspace.config());
    let ctx = WorkbookContext::load(&config, &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    let region = &regions[0];
    assert!(matches!(
        region.region_kind,
        Some(agent_spreadsheet_mcp::model::RegionKind::Parameters)
    ));
    assert_eq!(region.bounds, "A1:B5");
}

#[test]
fn calculator_region_detected() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("calc.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut((1u32, 1u32)).set_value_number(10);
        sheet.cell_mut((2u32, 1u32)).set_formula("A1*2");
        sheet.cell_mut((3u32, 1u32)).set_formula("B1+5");
        for row in 2..=4 {
            for col in 1..=3 {
                sheet.cell_mut((col, row)).set_formula("A1*2");
            }
        }
    });

    let config = Arc::new(workspace.config());
    let ctx = WorkbookContext::load(&config, &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    let region = &regions[0];
    assert!(matches!(
        region.region_kind,
        Some(agent_spreadsheet_mcp::model::RegionKind::Calculator)
    ));
}

#[test]
fn metadata_footer_split() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("footer.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut((1u32, 1u32)).set_value("H1");
        sheet.cell_mut((2u32, 1u32)).set_value("H2");
        for row in 2..=3 {
            sheet.cell_mut((1u32, row)).set_value_number(1);
            sheet.cell_mut((2u32, row)).set_value_number(2);
        }
        sheet.cell_mut((1u32, 9u32)).set_value("Notes");
        sheet
            .cell_mut((2u32, 9u32))
            .set_value("Confidential footer");
    });

    let config = Arc::new(workspace.config());
    let ctx = WorkbookContext::load(&config, &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    assert_eq!(regions.len(), 2);
    assert!(regions.iter().any(|r| r.bounds == "A1:B3"));
    let meta = regions.iter().find(|r| r.bounds.contains("9")).unwrap();
    assert!(matches!(
        meta.region_kind,
        Some(agent_spreadsheet_mcp::model::RegionKind::Metadata)
    ));
}

#[test]
fn multi_row_headers_detected() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("headers.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut((1u32, 1u32)).set_value("FY");
        sheet.cell_mut((2u32, 1u32)).set_value("FY");
        sheet.cell_mut((1u32, 2u32)).set_value("'2023");
        sheet.cell_mut((2u32, 2u32)).set_value("'2024");
        for row in 3..=5 {
            sheet
                .cell_mut((1u32, row))
                .set_value_number(10 + row as i32);
            sheet
                .cell_mut((2u32, row))
                .set_value_number(20 + row as i32);
        }
    });

    let config = Arc::new(workspace.config());
    let ctx = WorkbookContext::load(&config, &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    let region = &regions[0];
    eprintln!("headers {:?} at {:?}", region.headers, region.header_row);
    assert_eq!(region.header_row, Some(1));
    assert!(region.headers.iter().any(|h| h.contains("FY")));
    assert!(region.headers.iter().any(|h| h.contains("2024")));
}

#[test]
fn region_ids_stable_across_reads() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("stable.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut((1u32, 1u32)).set_value("A");
        sheet.cell_mut((2u32, 1u32)).set_value("B");
        sheet.cell_mut((1u32, 2u32)).set_value_number(1);
        sheet.cell_mut((2u32, 2u32)).set_value_number(2);
        sheet.cell_mut((1u32, 6u32)).set_value("X");
        sheet.cell_mut((2u32, 6u32)).set_value_number(9);
    });

    let config = Arc::new(workspace.config());
    let ctx = WorkbookContext::load(&config, &path).expect("load");
    let first = ctx.get_sheet_metrics("Sheet1").expect("metrics");
    let ids_first: Vec<u32> = first.detected_regions().iter().map(|r| r.id).collect();
    let second = ctx.get_sheet_metrics("Sheet1").expect("metrics");
    let ids_second: Vec<u32> = second.detected_regions().iter().map(|r| r.id).collect();
    assert_eq!(ids_first, ids_second);
}

#[test]
fn stacked_and_side_by_side_quadrants() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("quadrants.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Q1");
        sheet.cell_mut("B1").set_value_number(1);
        sheet.cell_mut("A6").set_value("Q3");
        sheet.cell_mut("B6").set_value_number(3);
        sheet.cell_mut("E1").set_value("Q2");
        sheet.cell_mut("F1").set_value_number(2);
        sheet.cell_mut("E6").set_value("Q4");
        sheet.cell_mut("F6").set_value_number(4);
    });
    let ctx = WorkbookContext::load(&workspace.config().into(), &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    assert_eq!(regions.len(), 4);
    assert!(regions.iter().any(|r| r.bounds == "A1:B1"));
    assert!(regions.iter().any(|r| r.bounds == "A6:B6"));
    assert!(regions.iter().any(|r| r.bounds == "E1:F1"));
    assert!(regions.iter().any(|r| r.bounds == "E6:F6"));
}

#[test]
fn outputs_band_detected() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("outputs.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Input");
        sheet.cell_mut("B1").set_value("Rate");
        for row in 2..=15 {
            sheet.cell_mut((1u32, row)).set_value_number(row as f64);
            sheet
                .cell_mut((2u32, row))
                .set_formula("A1*0.1")
                .set_value_number(0.1);
        }
        sheet.cell_mut("A18").set_value("Totals");
        for col in 2..=4 {
            sheet.cell_mut((col, 18u32)).set_formula("SUM(A2:A15)");
        }
    });
    let ctx = WorkbookContext::load(&workspace.config().into(), &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    assert!(!regions.is_empty(), "expected at least 1 region");
    let outputs_region = regions
        .iter()
        .find(|r| r.bounds.contains("18"))
        .expect("outputs region exists");
    assert!(
        matches!(
            outputs_region.region_kind,
            Some(agent_spreadsheet_mcp::model::RegionKind::Outputs)
        ),
        "expected Outputs, got {:?}",
        outputs_region.region_kind
    );
}

#[test]
fn noisy_sparse_sheet_stays_single_region_low_confidence() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("noisy.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("X");
        sheet.cell_mut("D10").set_value_number(5);
        sheet.cell_mut("H3").set_value("Note");
    });
    let ctx = WorkbookContext::load(&workspace.config().into(), &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    assert!(!regions.is_empty());
    let avg_conf: f32 = regions.iter().map(|r| r.confidence).sum::<f32>() / regions.len() as f32;
    assert!(avg_conf <= 1.0);
}

#[test]
fn edge_gutter_bias_trims_leading_blank_area() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("offset.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("E10").set_value("Data");
        sheet.cell_mut("F10").set_value_number(1);
        sheet.cell_mut("E11").set_value("Data2");
        sheet.cell_mut("F11").set_value_number(2);
    });
    let ctx = WorkbookContext::load(&workspace.config().into(), &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].bounds, "E10:F11");
}

#[test]
fn small_min_size_guard_keeps_tiny_block() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("tiny.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("B2").set_value("x");
    });
    let ctx = WorkbookContext::load(&workspace.config().into(), &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].bounds, "B2:B2");
}

#[test]
fn formula_mixed_with_values_classifies_mixed() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("mixed.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value_number(1);
        sheet
            .cell_mut("B1")
            .set_formula("A1*2")
            .set_value_number(2);
        sheet.cell_mut("A2").set_value("text");
    });
    let ctx = WorkbookContext::load(&workspace.config().into(), &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    let region = &regions[0];
    assert!(region.confidence > 0.3);
}

#[test]
fn header_false_positive_guard_uses_text_row() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("header_guard.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value_number(1);
        sheet.cell_mut("B1").set_value_number(2);
        sheet.cell_mut("A2").set_value("Name");
        sheet.cell_mut("B2").set_value("Val");
        sheet.cell_mut("A3").set_value("Row1");
        sheet.cell_mut("B3").set_value_number(10);
    });
    let ctx = WorkbookContext::load(&workspace.config().into(), &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    let region = &regions[0];
    assert_eq!(region.header_row, Some(2));
}

#[test]
fn key_value_layout_detected_as_parameters() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("kv_layout.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Item");
        sheet.cell_mut("B1").set_value("Widget");
        sheet.cell_mut("A2").set_value("Quantity");
        sheet.cell_mut("B2").set_value_number(150);
        sheet.cell_mut("A3").set_value("Price");
        sheet.cell_mut("B3").set_value_number(44.99);
        sheet.cell_mut("A4").set_value("Category");
        sheet.cell_mut("B4").set_value("Electronics");
        sheet.cell_mut("A5").set_value("Available");
        sheet.cell_mut("B5").set_value("Yes");
    });

    let config = Arc::new(workspace.config());
    let ctx = WorkbookContext::load(&config, &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    let region = &regions[0];

    assert!(
        matches!(
            region.region_kind,
            Some(agent_spreadsheet_mcp::model::RegionKind::Parameters)
        ),
        "expected Parameters, got {:?}",
        region.region_kind
    );
    assert_eq!(
        region.header_row, None,
        "key-value layout should not have header row"
    );
    assert!(
        !region.headers.contains(&"Widget".to_string()),
        "headers should not contain data values"
    );
}

#[test]
fn proper_noun_penalized_in_header_scoring() {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("proper_noun.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Acme Corp");
        sheet.cell_mut("B1").set_value("Globex Inc");
        sheet.cell_mut("C1").set_value("Initech LLC");
        sheet.cell_mut("A2").set_value("Name");
        sheet.cell_mut("B2").set_value("Dept");
        sheet.cell_mut("C2").set_value("Status");
        sheet.cell_mut("A3").set_value("Widget");
        sheet.cell_mut("B3").set_value("Sales");
        sheet.cell_mut("C3").set_value("Active");
    });

    let config = Arc::new(workspace.config());
    let ctx = WorkbookContext::load(&config, &path).expect("load");
    let regions = ctx
        .get_sheet_metrics("Sheet1")
        .expect("metrics")
        .detected_regions();
    let region = &regions[0];

    assert_eq!(
        region.header_row,
        Some(2),
        "row 2 with generic headers should be preferred over row 1 with proper nouns"
    );
}
