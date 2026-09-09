use anyhow::Result;
use agent_spreadsheet_mcp::model::FillDescriptor;
use agent_spreadsheet_mcp::tools::{
    ListWorkbooksParams, SheetStylesParams, SheetStylesScope, StyleGranularity, list_workbooks,
    sheet_styles,
};
use umya_spreadsheet::{
    GradientStop, HorizontalAlignmentValues, NumberingFormat, PatternValues,
    VerticalAlignmentValues,
};

mod support;

#[tokio::test(flavor = "current_thread")]
async fn sheet_styles_reports_full_descriptors() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("styled.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();

        sheet.cell_mut("A1").set_value("Header");
        sheet.cell_mut("A2").set_value_number(123.0);

        let style_a1 = sheet.style_mut("A1");
        style_a1.font_mut().set_bold(true);
        style_a1
            .fill_mut()
            .pattern_fill_mut()
            .set_pattern_type(PatternValues::Solid)
            .foreground_color_mut()
            .set_argb_str("FF0000FF");
        {
            let borders = style_a1.borders_mut();
            borders.left_border_mut().set_border_style("thin");
            borders.right_border_mut().set_border_style("thin");
            borders.top_border_mut().set_border_style("thin");
            borders.bottom_border_mut().set_border_style("thin");
        }
        style_a1
            .alignment_mut()
            .set_horizontal(HorizontalAlignmentValues::Center);
        style_a1
            .alignment_mut()
            .set_vertical(VerticalAlignmentValues::Top);
        style_a1.alignment_mut().set_wrap_text(true);
        style_a1
            .number_format_mut()
            .set_format_code(NumberingFormat::FORMAT_NUMBER_00);

        // A2 shares some style (number format only).
        sheet
            .style_mut("A2")
            .number_format_mut()
            .set_format_code(NumberingFormat::FORMAT_NUMBER_00);

        // B1 italic text.
        sheet.cell_mut("B1").set_value("Italic");
        sheet.style_mut("B1").font_mut().set_italic(true);
    });

    let config = workspace.config_with(|config| {
        config.max_payload_bytes = None;
    });
    let state = support::app_state_with_config(config);
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
    let workbook_id = list.workbooks[0].workbook_id.clone();

    let styles = sheet_styles(
        state,
        SheetStylesParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
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

    assert!(styles.total_styles >= 2);
    assert!(!styles.styles.is_empty());

    let a1_style = styles
        .styles
        .iter()
        .find(|s| s.example_cells.iter().any(|c| c == "A1"))
        .expect("A1 style present");

    let descriptor = a1_style.descriptor.as_ref().expect("descriptor");
    let font = descriptor.font.as_ref().expect("font");
    assert_eq!(font.bold, Some(true));

    let fill = descriptor.fill.as_ref().expect("fill");
    match fill {
        FillDescriptor::Pattern(p) => {
            assert_eq!(p.foreground_color.as_deref(), Some("FF0000FF"));
        }
        _ => panic!("expected pattern fill"),
    }

    let alignment = descriptor.alignment.as_ref().expect("alignment");
    assert_eq!(alignment.horizontal.as_deref(), Some("center"));
    assert_eq!(alignment.vertical.as_deref(), Some("top"));
    assert_eq!(alignment.wrap_text, Some(true));

    assert_eq!(descriptor.number_format.as_deref(), Some("0.00"));
    assert!(a1_style.tags.iter().any(|t| t == "header"));

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_styles_runs_respect_scope() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("style_overview.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("a");
        sheet.cell_mut("B1").set_value("b");
        sheet.cell_mut("C1").set_value("c");

        sheet.style_mut("A1").font_mut().set_bold(true);
        sheet.style_mut("B1").font_mut().set_bold(true);
        sheet.style_mut("C1").font_mut().set_italic(true);
    });

    let config = workspace.config_with(|config| {
        config.max_payload_bytes = None;
    });
    let state = support::app_state_with_config(config);
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
    let workbook_id = list.workbooks[0].workbook_id.clone();

    let styles = sheet_styles(
        state,
        SheetStylesParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            scope: Some(SheetStylesScope::Range {
                range: "A1:C1".to_string(),
            }),
            granularity: Some(StyleGranularity::Runs),
            max_items: Some(50),
            summary_only: Some(false),
            include_descriptor: None,
            include_ranges: None,
            include_example_cells: None,
        },
    )
    .await?;

    let bold_style = styles
        .styles
        .iter()
        .find(|s| {
            s.occurrences == 2
                && s.descriptor
                    .as_ref()
                    .and_then(|d| d.font.as_ref())
                    .and_then(|f| f.bold)
                    == Some(true)
        })
        .expect("expected a bold style");

    assert!(bold_style.cell_ranges.iter().any(|r| r == "A1:B1"));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_styles_cells_truncates() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("style_overview_cells.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("a");
        sheet.cell_mut("B1").set_value("b");
        sheet.cell_mut("C1").set_value("c");
    });

    let config = workspace.config_with(|config| {
        config.max_payload_bytes = None;
    });
    let state = support::app_state_with_config(config);
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
    let workbook_id = list.workbooks[0].workbook_id.clone();

    let styles = sheet_styles(
        state,
        SheetStylesParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
            scope: Some(SheetStylesScope::Range {
                range: "A1:C1".to_string(),
            }),
            granularity: Some(StyleGranularity::Cells),
            max_items: Some(2),
            summary_only: Some(false),
            include_descriptor: None,
            include_ranges: None,
            include_example_cells: None,
        },
    )
    .await?;

    assert_eq!(styles.styles.len(), 1);
    assert_eq!(styles.styles[0].occurrences, 3);
    assert_eq!(styles.styles[0].cell_ranges.len(), 2);
    assert!(styles.styles[0].ranges_truncated);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_styles_truncates_large_style_counts() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("many_styles.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        for i in 0..205u32 {
            let row = i + 1;
            let addr = format!("A{row}");
            sheet.cell_mut(addr.as_str()).set_value_number(i as i32);
            let color = format!("FF{:02X}0000", (i % 256) as u8);
            sheet
                .style_mut(addr.as_str())
                .font_mut()
                .color_mut()
                .set_argb_str(color);
        }
    });

    let config = workspace.config_with(|config| {
        config.max_payload_bytes = None;
    });
    let state = support::app_state_with_config(config);
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
    let workbook_id = list.workbooks[0].workbook_id.clone();

    let styles = sheet_styles(
        state,
        SheetStylesParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
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

    assert!(styles.total_styles >= 205);
    assert!(styles.styles_truncated);
    assert_eq!(styles.styles.len(), 200);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_styles_truncates_ranges_for_disjoint_runs() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("many_runs.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        for i in 0..51u32 {
            let row = 1 + i * 2;
            let addr = format!("A{row}");
            sheet.cell_mut(addr.as_str()).set_value("x");
            sheet
                .style_mut(addr.as_str())
                .font_mut()
                .set_bold(true);
        }
    });

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
    let workbook_id = list.workbooks[0].workbook_id.clone();

    let styles = sheet_styles(
        state,
        SheetStylesParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
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

    let header_style = styles
        .styles
        .iter()
        .find(|s| s.occurrences == 51 && s.tags.iter().any(|t| t == "header"))
        .expect("bold style present");

    assert!(header_style.ranges_truncated);
    assert_eq!(header_style.cell_ranges.len(), 50);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_styles_maps_gradient_pattern_underline_borders_rotation() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("breadth.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("Breadth");
        sheet.cell_mut("B1").set_value("Pattern");

        let style_a1 = sheet.style_mut("A1");
        style_a1.font_mut().set_bold(true);
        style_a1.font_mut().set_underline("double");
        style_a1.alignment_mut().set_text_rotation(45);

        {
            let borders = style_a1.borders_mut();
            borders.diagonal_border_mut().set_border_style("thick");
            borders.set_diagonal_up(true);
            borders.set_diagonal_down(true);
        }

        {
            let grad = style_a1.fill_mut().gradient_fill_mut();
            grad.set_degree(45.0);
            let mut stop1 = GradientStop::default();
            stop1.set_position(0.0);
            stop1.color_mut().set_argb_str("FFFF0000");
            grad.set_gradient_stop(stop1);
            let mut stop2 = GradientStop::default();
            stop2.set_position(1.0);
            stop2.color_mut().set_argb_str("FF00FF00");
            grad.set_gradient_stop(stop2);
        }

        let style_b1 = sheet.style_mut("B1");
        style_b1
            .fill_mut()
            .pattern_fill_mut()
            .set_pattern_type(PatternValues::Gray125);
    });

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
    let workbook_id = list.workbooks[0].workbook_id.clone();

    let styles = sheet_styles(
        state,
        SheetStylesParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
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

    let a1_style = styles
        .styles
        .iter()
        .find(|s| s.example_cells.iter().any(|c| c == "A1"))
        .expect("A1 style present");
    let descriptor = a1_style.descriptor.as_ref().expect("descriptor");

    let font = descriptor.font.as_ref().expect("font");
    assert_eq!(font.underline.as_deref(), Some("double"));

    let alignment = descriptor.alignment.as_ref().expect("alignment");
    assert_eq!(alignment.text_rotation, Some(45));

    let borders = descriptor.borders.as_ref().expect("borders");
    let diagonal = borders.diagonal.as_ref().expect("diagonal border");
    assert_eq!(diagonal.style.as_deref(), Some("thick"));
    assert_eq!(borders.diagonal_up, Some(true));
    assert_eq!(borders.diagonal_down, Some(true));

    match descriptor.fill.as_ref().expect("fill") {
        FillDescriptor::Gradient(g) => {
            assert_eq!(g.degree, Some(45.0));
            assert_eq!(g.stops.len(), 2);
        }
        _ => panic!("expected gradient fill"),
    }

    let b1_style = styles
        .styles
        .iter()
        .find(|s| s.example_cells.iter().any(|c| c == "B1"))
        .expect("B1 style present");
    let b1_fill = b1_style
        .descriptor
        .as_ref()
        .and_then(|d| d.fill.as_ref())
        .expect("B1 fill present");
    match b1_fill {
        FillDescriptor::Pattern(p) => {
            assert_eq!(p.pattern_type.as_deref(), Some("gray125"));
        }
        _ => panic!("expected pattern fill for B1"),
    }

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn sheet_styles_dedupes_identical_visible_formats() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("stable.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut("A1").set_value("A");
        sheet.cell_mut("B1").set_value("B");
        sheet.style_mut("A1").font_mut().set_bold(true);
        sheet.style_mut("B1").font_mut().set_bold(true);
    });

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
    let workbook_id = list.workbooks[0].workbook_id.clone();

    let styles = sheet_styles(
        state,
        SheetStylesParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Sheet1".to_string(),
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

    let header_style = styles
        .styles
        .iter()
        .find(|s| s.tags.iter().any(|t| t == "header"))
        .expect("header style present");
    assert_eq!(header_style.occurrences, 2);
    Ok(())
}

#[test]
fn compress_positions_merges_runs() {
    let positions = vec![(1, 1), (1, 2), (1, 3), (2, 1), (2, 2)];
    let (mut ranges, truncated) =
        agent_spreadsheet_mcp::styles::compress_positions_to_ranges(&positions, 50);
    ranges.sort();
    assert!(!truncated);
    assert_eq!(ranges, vec!["A1:C1".to_string(), "A2:B2".to_string()]);
}
