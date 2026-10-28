//! Synthetic public source packages; shared by core and byte-adapter tests.
#![allow(dead_code)]
use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Write},
};
use zip::{ZipArchive, ZipWriter, write::FileOptions};

pub const SHEET: &str = "xl/worksheets/sheet1.xml";
const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const OFFICE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

pub fn parts(rows: &str) -> BTreeMap<String, String> {
    [
        ("[Content_Types].xml", "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"bin\" ContentType=\"application/octet-stream\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>".to_owned()),
        ("_rels/.rels", format!("<Relationships xmlns=\"{RELS}\"><Relationship Id=\"rId1\" Type=\"{OFFICE}/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>")),
        ("xl/workbook.xml", format!("<workbook xmlns=\"{MAIN}\" xmlns:r=\"{OFFICE}\"><sheets><sheet name=\"Sheet1\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>")),
        ("xl/_rels/workbook.xml.rels", format!("<Relationships xmlns=\"{RELS}\"><Relationship Id=\"rId1\" Type=\"{OFFICE}/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>")),
        (SHEET, format!("<worksheet xmlns=\"{MAIN}\"><sheetData>{rows}</sheetData></worksheet>")),
        ("custom/opaque.bin", "unrelated package member: preserve me".to_owned()),
    ].into_iter().map(|(name, body)| (name.to_owned(), body)).collect()
}

pub fn pack(parts: &BTreeMap<String, String>) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.set_comment("original-source-comment");
    let options = FileOptions::default()
        .unix_permissions(0o640)
        .last_modified_time(zip::DateTime::from_date_and_time(2020, 1, 2, 3, 4, 6).unwrap());
    for (name, body) in parts {
        zip.start_file(name, options).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

pub fn member(bytes: &[u8], name: &str) -> String {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut result = String::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_string(&mut result)
        .unwrap();
    result
}

pub fn stale_parts() -> BTreeMap<String, String> {
    parts("<row r=\"1\"><c r=\"A1\"><v>21</v></c><c r=\"B1\"><f>A1*2</f><v>0</v></c></row>")
}

pub fn errors(count: usize) -> Vec<u8> {
    let rows: String = (1..=count)
        .map(|row| format!("<row r=\"{row}\"><c r=\"A{row}\"><f>1/0</f><v>0</v></c></row>"))
        .collect();
    pack(&parts(&rows))
}

pub fn external() -> Vec<u8> {
    let mut p = parts("<row r=\"1\"><c r=\"A1\"><f>[1]Sheet1!A1*2</f><v>0</v></c></row>");
    let workbook = p.get_mut("xl/workbook.xml").unwrap();
    *workbook = workbook.replace(
        "</sheets>",
        "</sheets><externalReferences><externalReference r:id=\"rIdLink1\"/></externalReferences>",
    );
    let relationships = p.get_mut("xl/_rels/workbook.xml.rels").unwrap();
    *relationships = relationships.replace("</Relationships>", &format!(
        "<Relationship Id=\"rIdLink1\" Type=\"{OFFICE}/externalLink\" Target=\"externalLinks/externalLink1.xml\"/></Relationships>"
    ));
    let types = p.get_mut("[Content_Types].xml").unwrap();
    *types = types.replace("</Types>", "<Override PartName=\"/xl/externalLinks/externalLink1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml\"/></Types>");
    p.insert("xl/externalLinks/externalLink1.xml".into(), format!(
        "<externalLink xmlns=\"{MAIN}\"><externalBook xmlns:r=\"{OFFICE}\" r:id=\"rId1\"><sheetNames><sheetName val=\"Sheet1\"/></sheetNames><sheetDataSet><sheetData sheetId=\"0\"><row r=\"1\"><cell r=\"A1\"><v>21</v></cell></row></sheetData></sheetDataSet></externalBook></externalLink>"
    ));
    p.insert("xl/externalLinks/_rels/externalLink1.xml.rels".into(), format!(
        "<Relationships xmlns=\"{RELS}\"><Relationship Id=\"rId1\" Type=\"{OFFICE}/externalLinkPath\" Target=\"file:///synthetic/Source.xlsx\" TargetMode=\"External\"/></Relationships>"
    ));
    pack(&p)
}
