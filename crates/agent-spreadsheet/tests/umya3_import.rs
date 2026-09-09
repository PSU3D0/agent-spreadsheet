#![cfg(feature = "recalc-formualizer")]
use agent_spreadsheet::{core::session::WorkbookSession, recalc::ResidentWorkbook};

#[test]
#[cfg(feature = "render")]
fn dashboard_styles_survive_export() {
    let source = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../agent-spreadsheet-render/tests/fixtures/gen_11_dashboard.xlsx")).unwrap();
    let original = formualizer_workbook::backends::umya3::read_document(&source).unwrap();
    let exported = WorkbookSession::from_bytes(&source).unwrap().to_bytes().unwrap();
    let reopened = formualizer_workbook::backends::umya3::read_document(&exported).unwrap();
    for address in ["A9", "B9", "E9"] {
        let before = original.sheet(0).unwrap().cell(address).unwrap().style();
        let after = reopened.sheet(0).unwrap().cell(address).unwrap().style();
        assert_eq!(before.font(), after.font(), "{address} font");
    }
}

#[test]
fn resident_and_file_documents_export_repaired_colours_without_hot_serialization() {
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.sheet_mut(0).unwrap();
    sheet.cell_mut("A1").set_value_number(21.0);
    sheet.cell_mut("B1").set_formula("A1*2");
    let border = sheet.cell_mut("B1").style_mut().borders_mut().left_mut();
    border.set_border_style(umya_spreadsheet::Border::BORDER_THIN);
    border.set_color(
        umya_spreadsheet::Color::default()
            .set_argb_str("FFC00000")
            .clone(),
    );
    let mut source = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&book, &mut source).unwrap();
    let file_document = WorkbookSession::from_bytes(&source).unwrap();
    let mut resident = ResidentWorkbook::from_bytes(&source).unwrap();
    resident.recalculate(None).unwrap();
    resident.recalculate(None).unwrap();
    assert_eq!(resident.evaluator_counters().ingests, 1);
    assert_eq!(resident.serialization_count(), 0);
    let exported = resident.export_bytes().unwrap();
    for bytes in [file_document.to_bytes().unwrap(), exported.clone()] {
        let reopened = formualizer_workbook::backends::umya3::read_document(&bytes).unwrap();
        let cell = reopened.sheet(0).unwrap().cell("B1").unwrap();
        assert_eq!(
            cell.style()
                .borders()
                .unwrap()
                .left()
                .color()
                .unwrap()
                .argb_str(),
            "FFC00000"
        );
        assert_eq!(cell.formula(), "A1*2");
    }
    let reopened = formualizer_workbook::backends::umya3::read_document(&exported).unwrap();
    assert_eq!(
        reopened
            .sheet(0)
            .unwrap()
            .cell("B1")
            .unwrap()
            .value_number(),
        Some(42.0)
    );
    assert_eq!(resident.serialization_count(), 1);
}
