//! Canonical rich-document fixture with the style table used by ordinary editing.
pub fn input() -> Vec<u8> {
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.sheet_by_name_mut("Sheet1").unwrap();
    sheet.cell_mut("A1").set_value_number(21);
    sheet
        .cell_mut("B1")
        .set_formula("A1*2")
        .set_formula_result_number(42);
    let mut bytes = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&book, &mut bytes).unwrap();
    bytes
}
