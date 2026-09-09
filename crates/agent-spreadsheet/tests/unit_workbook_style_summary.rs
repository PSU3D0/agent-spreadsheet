use anyhow::Result;
use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::tools::{
    ListWorkbooksParams, WorkbookStyleSummaryParams, list_workbooks, workbook_style_summary,
};
use umya_spreadsheet::structs::drawing::Color2Type;
use umya_spreadsheet::{
    ConditionalFormatValues, ConditionalFormatting, ConditionalFormattingRule, Formula,
};

mod support;

#[tokio::test(flavor = "current_thread")]
async fn workbook_style_summary_reports_theme_and_infers_default_style() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("wb_styles.xlsx", |book| {
        let sheet1 = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        for col in ['A', 'B', 'C', 'D', 'E'] {
            let addr = format!("{col}1");
            sheet1.get_cell_mut(addr.as_str()).set_value("x");
        }

        sheet1.get_cell_mut("F1").set_value("Header");
        sheet1.get_style_mut("F1").get_font_mut().set_bold(true);

        book.new_sheet("Sheet2").unwrap();
        let sheet2 = book.get_sheet_by_name_mut("Sheet2").ok().unwrap();
        sheet2.get_cell_mut("A1").set_value_number(1);

        let mut cf = ConditionalFormatting::default();
        cf.get_sequence_of_references_mut().set_sqref("A1:A3");

        let mut rule = ConditionalFormattingRule::default();
        rule.set_type(ConditionalFormatValues::Expression);
        rule.set_priority(1);
        let mut formula = Formula::default();
        formula.set_string_value("A1>0");
        rule.set_formula(formula);
        cf.add_conditional_collection(rule);
        sheet2.add_conditional_formatting_collection(cf);
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

    let theme = summary.theme.as_ref().expect("theme present");
    assert!(!theme.colors.is_empty());

    assert!(!summary.styles.is_empty());
    let default_style_id = summary
        .inferred_default_style_id
        .clone()
        .expect("default style id");
    let default_style = summary
        .styles
        .iter()
        .find(|s| s.style_id == default_style_id)
        .expect("default style usage");
    let inferred_font = summary
        .inferred_default_font
        .as_ref()
        .expect("default font");
    let default_font_name = default_style
        .descriptor
        .as_ref()
        .and_then(|d| d.font.as_ref())
        .and_then(|f| f.name.clone());
    if default_font_name.is_some() {
        assert_eq!(default_font_name, inferred_font.name);
    } else {
        assert!(inferred_font.name.is_some());
    }

    assert_eq!(summary.conditional_formats.len(), 1);
    let cf_summary = &summary.conditional_formats[0];
    assert_eq!(cf_summary.sheet_name, "Sheet2");
    assert_eq!(cf_summary.range, "A1:A3");
    assert!(cf_summary.rule_types.iter().any(|t| t == "expression"));

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn workbook_style_summary_truncates_large_style_counts() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("many_styles.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        for i in 0..205u32 {
            let row = i + 1;
            let addr = format!("A{row}");
            sheet.get_cell_mut(addr.as_str()).set_value_number(i as i32);
            let color = format!("FF{:02X}0000", (i % 256) as u8);
            sheet
                .get_style_mut(addr.as_str())
                .get_font_mut()
                .get_color_mut()
                .set_argb_str(color);
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

    assert!(summary.total_styles >= 205);
    assert!(summary.styles_truncated);
    assert_eq!(summary.styles.len(), 200);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn workbook_style_summary_handles_empty_workbook() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("empty.xlsx", |_book| {});

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

    assert_eq!(summary.total_styles, 0);
    assert!(summary.styles.is_empty());
    assert!(summary.inferred_default_style_id.is_none());
    assert!(summary.theme.is_some());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn workbook_style_summary_omits_empty_theme_colors() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("theme.xlsx", |book| {
        let scheme = book
            .get_theme_mut()
            .get_theme_elements_mut()
            .get_color_scheme_mut();
        scheme.set_accent1(Color2Type::default());
        scheme.set_accent2(Color2Type::default());
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

    let theme = summary.theme.expect("theme present");
    assert!(!theme.colors.contains_key("accent1"));
    assert!(!theme.colors.contains_key("accent2"));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn workbook_style_summary_sets_scan_truncated_when_limit_exceeded() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("scan.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        for i in 0..1100u32 {
            let row = i + 1;
            let addr = format!("A{row}");
            sheet.get_cell_mut(addr.as_str()).set_value_number(i as i32);
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

    let summary = workbook_style_summary(
        state,
        WorkbookStyleSummaryParams {
            workbook_or_fork_id: workbook_id,
            max_styles: None,
            max_conditional_formats: None,
            max_cells_scan: Some(1000),
            summary_only: Some(false),
            include_descriptor: None,
            include_example_cells: None,
            include_theme: None,
            include_conditional_formats: None,
        },
    )
    .await?;

    assert!(summary.scan_truncated);
    assert!(summary.notes.iter().any(|n| n.contains("Stopped scanning")));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn workbook_style_summary_aggregates_multiple_cf_rules_and_sheets() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("cf_multi.xlsx", |book| {
        let sheet1 = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        let mut cf1 = ConditionalFormatting::default();
        cf1.get_sequence_of_references_mut().set_sqref("A1:A3");
        let mut rule1 = ConditionalFormattingRule::default();
        rule1.set_type(ConditionalFormatValues::Expression);
        rule1.set_priority(1);
        let mut formula = Formula::default();
        formula.set_string_value("A1>0");
        rule1.set_formula(formula);
        cf1.add_conditional_collection(rule1);
        let mut rule2 = ConditionalFormattingRule::default();
        rule2.set_type(ConditionalFormatValues::DuplicateValues);
        rule2.set_priority(2);
        cf1.add_conditional_collection(rule2);
        sheet1.add_conditional_formatting_collection(cf1);

        book.new_sheet("Sheet2").unwrap();
        let sheet2 = book.get_sheet_by_name_mut("Sheet2").ok().unwrap();
        let mut cf2 = ConditionalFormatting::default();
        cf2.get_sequence_of_references_mut().set_sqref("B1:B2");
        let mut rule3 = ConditionalFormattingRule::default();
        rule3.set_type(ConditionalFormatValues::Expression);
        rule3.set_priority(1);
        let mut formula = Formula::default();
        formula.set_string_value("B1>0");
        rule3.set_formula(formula);
        cf2.add_conditional_collection(rule3);
        sheet2.add_conditional_formatting_collection(cf2);
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

    assert_eq!(summary.conditional_formats.len(), 2);
    let sheet1_cf = summary
        .conditional_formats
        .iter()
        .find(|c| c.sheet_name == "Sheet1")
        .expect("Sheet1 CF");
    assert_eq!(sheet1_cf.range, "A1:A3");
    assert_eq!(sheet1_cf.rule_count, 2);
    assert!(sheet1_cf.rule_types.iter().any(|t| t == "expression"));
    assert!(sheet1_cf.rule_types.iter().any(|t| t == "duplicateValues"));

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn workbook_style_summary_truncates_conditional_formats() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("cf_trunc.xlsx", |book| {
        let sheet1 = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        let mut cf1 = ConditionalFormatting::default();
        cf1.get_sequence_of_references_mut().set_sqref("A1:A2");
        let mut rule1 = ConditionalFormattingRule::default();
        rule1.set_type(ConditionalFormatValues::Expression);
        rule1.set_priority(1);
        let mut formula = Formula::default();
        formula.set_string_value("A1>0");
        rule1.set_formula(formula);
        cf1.add_conditional_collection(rule1);
        sheet1.add_conditional_formatting_collection(cf1);

        let mut cf2 = ConditionalFormatting::default();
        cf2.get_sequence_of_references_mut().set_sqref("B1:B2");
        let mut rule2 = ConditionalFormattingRule::default();
        rule2.set_type(ConditionalFormatValues::Expression);
        rule2.set_priority(1);
        let mut formula = Formula::default();
        formula.set_string_value("B1>0");
        rule2.set_formula(formula);
        cf2.add_conditional_collection(rule2);
        sheet1.add_conditional_formatting_collection(cf2);
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

    let summary = workbook_style_summary(
        state,
        WorkbookStyleSummaryParams {
            workbook_or_fork_id: workbook_id,
            max_styles: None,
            max_conditional_formats: Some(1),
            max_cells_scan: None,
            summary_only: Some(false),
            include_descriptor: None,
            include_example_cells: None,
            include_theme: None,
            include_conditional_formats: None,
        },
    )
    .await?;

    assert_eq!(summary.conditional_formats.len(), 1);
    assert!(summary.conditional_formats_truncated);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn workbook_style_summary_aggregates_identical_styles_across_sheets() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("style_invariant.xlsx", |book| {
        let sheet1 = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet1.get_cell_mut("A1").set_value("x");
        sheet1.get_style_mut("A1").get_font_mut().set_bold(true);

        book.new_sheet("Sheet2").unwrap();
        let sheet2 = book.get_sheet_by_name_mut("Sheet2").ok().unwrap();
        sheet2.get_cell_mut("A1").set_value("x");
        sheet2.get_style_mut("A1").get_font_mut().set_bold(true);
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

    let invariant = summary
        .styles
        .iter()
        .find(|s| s.occurrences == 2)
        .expect("aggregated style");
    assert!(invariant.example_cells.iter().any(|c| c == "Sheet1!A1"));
    assert!(invariant.example_cells.iter().any(|c| c == "Sheet2!A1"));
    Ok(())
}
