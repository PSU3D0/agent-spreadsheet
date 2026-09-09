use umya_spreadsheet::{
    ConditionalFormatValues, ConditionalFormatting, ConditionalFormattingOperatorValues,
    ConditionalFormattingRule, Formula, PatternValues, Style, Worksheet,
};

/// Build a minimal style intended for conditional formatting (OOXML `dxf`).
///
/// Note: umya-spreadsheet materializes/deduplicates `dxf` records at write time based on
/// the style hash. Keeping this style deterministic helps reuse the same `dxf`.
pub fn build_simple_dxf_style(fill_argb: &str, font_argb: &str, bold: bool) -> Style {
    let mut style = Style::default();

    crate::styles::set_color_hex(
        style.get_fill_mut().get_pattern_fill_mut()
            .set_pattern_type(PatternValues::Solid).get_foreground_color_mut(),
        fill_argb,
    );

    let font = style.get_font_mut();
    font.set_bold(bold);
    crate::styles::set_color_hex(font.get_color_mut(), font_argb);

    style
}

/// Compute the next conditional-formatting rule priority for a worksheet.
///
/// Excel expects priorities to be unique within a worksheet.
pub fn next_cf_priority(sheet: &Worksheet) -> i32 {
    let mut max_priority: i32 = 0;
    for cf in sheet.get_conditional_formatting_collection() {
        for rule in cf.get_conditional_collection() {
            max_priority = max_priority.max(rule.get_priority());
        }
    }

    max_priority.saturating_add(1).max(1)
}

/// Append an `expression` conditional formatting rule for `sqref`.
///
/// Returns the priority assigned to the new rule.
pub fn append_cf_expression_rule(
    sheet: &mut Worksheet,
    sqref: &str,
    expression: &str,
    dxf_style: Style,
) -> i32 {
    let mut cf = ConditionalFormatting::default();
    cf.get_sequence_of_references_mut().set_sqref(sqref);

    let priority = next_cf_priority(sheet);
    let mut rule = ConditionalFormattingRule::default();
    rule.set_type(ConditionalFormatValues::Expression);
    rule.set_priority(priority);

    let mut formula = Formula::default();
    formula.set_string_value(expression);
    rule.set_formula(formula);
    rule.set_style(dxf_style);

    cf.add_conditional_collection(rule);
    sheet.add_conditional_formatting_collection(cf);

    priority
}

/// Append a `cellIs` conditional formatting rule for `sqref`.
///
/// Returns the priority assigned to the new rule.
pub fn append_cf_cellis_rule(
    sheet: &mut Worksheet,
    sqref: &str,
    operator: ConditionalFormattingOperatorValues,
    formula_value: &str,
    dxf_style: Style,
) -> i32 {
    let mut cf = ConditionalFormatting::default();
    cf.get_sequence_of_references_mut().set_sqref(sqref);

    let priority = next_cf_priority(sheet);
    let mut rule = ConditionalFormattingRule::default();
    rule.set_type(ConditionalFormatValues::CellIs);
    rule.set_operator(operator);
    rule.set_priority(priority);

    let mut formula = Formula::default();
    formula.set_string_value(formula_value);
    rule.set_formula(formula);
    rule.set_style(dxf_style);

    cf.add_conditional_collection(rule);
    sheet.add_conditional_formatting_collection(cf);

    priority
}
