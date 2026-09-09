#![cfg(feature = "recalc")]

use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::diff::{Change, calculate_changeset, names::NameDiff, tables::TableDiff};
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use umya_spreadsheet::{Workbook as Spreadsheet, structs::Table};
use zip::write::FileOptions;
use zip::{ZipArchive, ZipWriter};

#[path = "./support/mod.rs"]
mod support;

struct DiffScenario {
    _temp_dir: tempfile::TempDir,
    base_path: PathBuf,
    fork_path: PathBuf,
}

impl DiffScenario {
    fn new() -> Self {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let base_path = temp_dir.path().join("base.xlsx");
        let fork_path = temp_dir.path().join("fork.xlsx");

        Self {
            _temp_dir: temp_dir,
            base_path,
            fork_path,
        }
    }

    fn setup<F1, F2>(&self, setup_base: F1, setup_fork: F2)
    where
        F1: FnOnce(&mut Spreadsheet),
        F2: FnOnce(&mut Spreadsheet),
    {
        let mut base_book = umya_spreadsheet::new_file();
        setup_base(&mut base_book);
        umya_spreadsheet::writer::xlsx::write(&base_book, &self.base_path)
            .expect("failed to write base");

        let mut fork_book = umya_spreadsheet::new_file();
        setup_fork(&mut fork_book);
        umya_spreadsheet::writer::xlsx::write(&fork_book, &self.fork_path)
            .expect("failed to write fork");
    }

    fn run_diff(&self, sheet_filter: Option<&str>) -> Vec<Change> {
        calculate_changeset(&self.base_path, &self.fork_path, sheet_filter).expect("diff failed")
    }
}

// Helper to inject defined names into an existing XLSX file
fn inject_defined_names(path: &PathBuf, names: &[(&str, &str)]) {
    let file = File::open(path).unwrap();
    let mut archive = ZipArchive::new(file).unwrap();

    let mut workbook_xml = String::new();

    // 1. Read all files into memory (simple for small test files)
    let mut files = std::collections::HashMap::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).unwrap();
        let name = file.name().to_string();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        if name == "xl/workbook.xml" {
            workbook_xml = String::from_utf8(buffer).unwrap();
        } else {
            files.insert(name, buffer);
        }
    }

    // 2. Modify workbook.xml
    // Insert <definedNames> before </workbook>
    let names_xml: String = names
        .iter()
        .map(|(n, f)| format!("<definedName name=\"{}\">{}</definedName>", n, f))
        .collect();

    let replacement = format!("<definedNames>{}</definedNames></workbook>", names_xml);
    workbook_xml = workbook_xml.replace("</workbook>", &replacement);

    files.insert("xl/workbook.xml".to_string(), workbook_xml.into_bytes());

    // 3. Write back
    let file = File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);

    for (name, content) in files {
        zip.start_file(name, FileOptions::default()).unwrap();
        zip.write_all(&content).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn test_defined_name_changes() {
    let scenario = DiffScenario::new();

    scenario.setup(|_| {}, |_| {});

    // Inject names manually
    inject_defined_names(&scenario.base_path, &[("Input", "Sheet1!$A$1")]);
    inject_defined_names(
        &scenario.fork_path,
        &[("Input", "Sheet1!$B$1"), ("Output", "Sheet1!$C$1")],
    );

    let diffs = scenario.run_diff(None);

    // Check for NameModified "Input"
    let input_mod = diffs.iter().find(
        |d| matches!(d, Change::Name(NameDiff::NameModified { name, .. }) if name == "Input"),
    );
    assert!(input_mod.is_some(), "Expected Input modification");
    if let Some(Change::Name(NameDiff::NameModified {
        old_formula,
        new_formula,
        ..
    })) = input_mod
    {
        assert_eq!(old_formula, "Sheet1!$A$1");
        assert_eq!(new_formula, "Sheet1!$B$1");
    }

    // Check for NameAdded "Output"
    let output_add = diffs
        .iter()
        .find(|d| matches!(d, Change::Name(NameDiff::NameAdded { name, .. }) if name == "Output"));
    assert!(output_add.is_some(), "Expected Output addition");
}

#[test]
fn test_table_changes() {
    let scenario = DiffScenario::new();

    // Base: Table "Sales" on Sheet1 A1:C5
    // Fork: Table "Sales" resized to A1:C10
    scenario.setup(
        |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("Sheet1");
            let mut table = Table::default();
            table.set_name("Table1");
            table.set_display_name("Sales");
            table.set_area(((1, 1), (3, 5))); // A1:C5
            sheet.add_table(table);
        },
        |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("Sheet1");
            let mut table = Table::default();
            table.set_name("Table1");
            table.set_display_name("Sales");
            table.set_area(((1, 1), (3, 10))); // A1:C10
            sheet.add_table(table);
        },
    );

    let diffs = scenario.run_diff(None);

    // Check for TableModified "Sales"
    let sales_mod = diffs.iter().find(|d| matches!(d, Change::Table(TableDiff::TableModified { display_name, .. }) if display_name == "Sales"));
    assert!(sales_mod.is_some(), "Expected Sales table modification");

    if let Some(Change::Table(TableDiff::TableModified {
        sheet,
        old_range,
        new_range,
        ..
    })) = sales_mod
    {
        assert_eq!(sheet, "Sheet1");
        assert_eq!(old_range, "A1:C5");
        assert_eq!(new_range, "A1:C10");
    }
}

#[test]
fn test_table_addition() {
    let scenario = DiffScenario::new();

    scenario.setup(
        |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("Sheet1");
        },
        |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("Sheet1");
            let mut table = Table::default();
            table.set_name("NewTable");
            table.set_display_name("NewTable");
            table.set_area(((1, 1), (2, 2)));
            sheet.add_table(table);
        },
    );

    let diffs = scenario.run_diff(None);

    let added = diffs.iter().find(|d| matches!(d, Change::Table(TableDiff::TableAdded { display_name, .. }) if display_name == "NewTable"));
    assert!(added.is_some());
}
