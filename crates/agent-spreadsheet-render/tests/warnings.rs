//! One test per [`Warning`] variant. The 5004 definition of done is that
//! nothing unsupported disappears silently, so every variant has to be
//! reachable and has to be proven reachable.

mod common;

use agent_spreadsheet_render::{
    Fidelity, RangeBounds, RasterOptions, RenderOptions, Warning, extract_scene, rasterize,
};
use common::render_fixture;

fn warnings_for(fixture: &str) -> Vec<Warning> {
    render_fixture(fixture).1.report.warnings
}

#[test]
fn conditional_format_omitted() {
    let warnings = warnings_for("cf_conditional_format");
    assert!(
        warnings.contains(&Warning::ConditionalFormatOmitted),
        "got {warnings:?}"
    );
}

#[test]
fn chart_omitted() {
    let warnings = warnings_for("chart_bar");
    assert!(
        warnings.contains(&Warning::ChartOmitted),
        "got {warnings:?}"
    );
}

#[test]
fn image_omitted_and_rotation_and_pattern_and_number_format() {
    let warnings = warnings_for("warnings_grabbag");
    for expected in [
        Warning::ImageOmitted,
        Warning::TextRotationOmitted,
        Warning::PatternFillApproximated,
        Warning::NumberFormatApproximated,
    ] {
        assert!(
            warnings.contains(&expected),
            "{expected:?} not in {warnings:?}"
        );
    }
}

#[test]
fn font_substituted_for_codepoints_outside_the_subset() {
    let warnings = warnings_for("unicode_wide");
    assert!(
        warnings.contains(&Warning::FontSubstituted),
        "got {warnings:?}"
    );
}

#[test]
fn formulas_unevaluated_when_no_cached_value() {
    let warnings = warnings_for("formulas_uncached");
    assert!(
        warnings.contains(&Warning::FormulasUnevaluated),
        "got {warnings:?}"
    );
    // And the cell renders empty rather than showing the formula text.
    let (scene, _) = render_fixture("formulas_uncached");
    let painted: Vec<&str> = scene
        .cmds
        .iter()
        .filter_map(|cmd| match cmd {
            agent_spreadsheet_render::Cmd::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !painted.iter().any(|t| t.contains("SUM")),
        "formula text leaked into the render: {painted:?}"
    );
}

#[test]
fn a_recalculated_fixture_does_not_raise_formulas_unevaluated() {
    // gen_11 goes through `asp workbook recalculate`, so its SUMs carry cached
    // values and the warning must not fire.
    let warnings = warnings_for("gen_11_dashboard");
    assert!(
        !warnings.contains(&Warning::FormulasUnevaluated),
        "got {warnings:?}"
    );
    let (scene, _) = render_fixture("gen_11_dashboard");
    let painted: Vec<&str> = scene
        .cmds
        .iter()
        .filter_map(|cmd| match cmd {
            agent_spreadsheet_render::Cmd::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        painted.contains(&"$29,010,000"),
        "cached SUM not rendered: {painted:?}"
    );
    // The bake-off's number-format regression, end to end.
    assert!(painted.contains(&"57.3%"), "got {painted:?}");
    assert!(painted.contains(&"-3.2%"), "got {painted:?}");
}

#[test]
fn rich_text_flattened() {
    // No corpus workbook carries rich text and openpyxl cannot author it, so
    // this one is built in memory.
    let mut book = umya_spreadsheet::new_file();
    let mut rich = umya_spreadsheet::RichText::default();
    let mut plain = umya_spreadsheet::TextElement::default();
    plain.set_text("plain ");
    let mut bold = umya_spreadsheet::TextElement::default();
    bold.set_text("bold");
    let mut font = umya_spreadsheet::Font::default();
    font.set_bold(true);
    bold.set_run_properties(font);
    rich.set_rich_text_elements(vec![plain, bold]);
    book.get_sheet_mut(&0).ok()
        .unwrap()
        .get_cell_mut("A1")
        .set_rich_text(rich);

    let sheet = book.get_sheet(&0).ok().unwrap();
    let scene = extract_scene(
        sheet,
        &book,
        &RangeBounds::new(1, 2, 1, 2),
        &RenderOptions::default(),
    )
    .unwrap();
    assert!(
        scene.warnings.contains(&Warning::RichTextFlattened),
        "got {:?}",
        scene.warnings
    );
    let output = rasterize(&scene, &RasterOptions::default()).unwrap();
    assert_eq!(output.report.fidelity, Fidelity::Partial);
}

#[test]
fn a_clean_fixture_reports_full_fidelity() {
    // gen_10 is plain text with gridlines turned off: nothing to warn about.
    let (_, output) = render_fixture("gen_10_gridlines");
    assert_eq!(
        output.report.fidelity,
        Fidelity::Full,
        "unexpected warnings: {:?}",
        output.report.warnings
    );
}

#[test]
fn warnings_are_sorted_and_deduplicated() {
    let (_, output) = render_fixture("warnings_grabbag");
    let mut sorted = output.report.warnings.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted, output.report.warnings);
}
