use anyhow::Result;
use std::fs::File;
use std::io::Read;

use agent_spreadsheet::model::FillDescriptor;
use agent_spreadsheet::rules::conditional_format::{
    append_cf_expression_rule, build_simple_dxf_style,
};

fn read_zip_entry(path: &std::path::Path, entry_name: &str) -> Result<String> {
    let file = File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut entry = zip.by_name(entry_name)?;
    let mut out = String::new();
    entry.read_to_string(&mut out)?;
    Ok(out)
}

#[test]
fn conditional_format_helpers_persist_dxf_and_sheet_blocks() -> Result<()> {
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.cell_mut("A1").set_value_number(1);

    let fill_argb = "FF12AB34";
    let font_argb = "FF123456";
    let style = build_simple_dxf_style(fill_argb, font_argb, true);

    let priority = append_cf_expression_rule(sheet, "A1:A3", "A1>0", style);
    assert_eq!(priority, 1);

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("cf_helpers.xlsx");
    umya_spreadsheet::writer::xlsx::write(&book, &path)?;

    // Raw OOXML checks: conditionalFormatting exists and rule references a dxf.
    let sheet_xml = read_zip_entry(&path, "xl/worksheets/sheet1.xml")?;
    assert!(sheet_xml.contains("<conditionalFormatting"));
    assert!(sheet_xml.contains("sqref=\"A1:A3\""));
    assert!(sheet_xml.contains("dxfId=\""));

    let styles_xml = read_zip_entry(&path, "xl/styles.xml")?;
    let styles_xml_upper = styles_xml.to_ascii_uppercase();
    assert!(styles_xml_upper.contains("<DXFS"));
    assert!(styles_xml_upper.contains(fill_argb));
    assert!(styles_xml_upper.contains(font_argb));

    // Read-back via umya: conditionalFormatting blocks and style colors persist.
    let book2 = umya_spreadsheet::reader::xlsx::read(&path)?;
    let sheet2 = book2.sheet_by_name("Sheet1").ok().unwrap();
    let cfs = sheet2.conditional_formatting_collection();
    assert_eq!(cfs.len(), 1);
    assert_eq!(cfs[0].sequence_of_references().get_sqref(), "A1:A3");
    assert_eq!(cfs[0].conditional_collection().len(), 1);

    let rule = &cfs[0].conditional_collection()[0];
    let st = rule.style().expect("expected dxf-backed style");
    let desc = agent_spreadsheet::styles::descriptor_from_style(st);

    assert_eq!(desc.font.as_ref().and_then(|f| f.bold), Some(true));
    assert_eq!(
        desc.font.as_ref().and_then(|f| f.color.as_deref()),
        Some(font_argb)
    );
    match desc.fill {
        Some(FillDescriptor::Pattern(p)) => {
            assert_eq!(p.foreground_color.as_deref(), Some(fill_argb));
        }
        other => panic!("expected pattern fill in dxf style, got: {other:?}"),
    }

    Ok(())
}
