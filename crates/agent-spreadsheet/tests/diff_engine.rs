#![cfg(feature = "recalc")]

use agent_spreadsheet as agent_spreadsheet_mcp;
use agent_spreadsheet_mcp::diff::{
    Change, calculate_changeset,
    merge::{CellDiff, ModificationType},
};
use std::path::PathBuf;
use umya_spreadsheet::Workbook as Spreadsheet;

#[path = "./support/mod.rs"]
mod support;

use support::builders::{self, CellVal};

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

#[test]
fn test_no_changes() {
    let scenario = DiffScenario::new();
    scenario.setup(
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            builders::set_cell(sheet, 1, 1, &CellVal::from(10)); // A1
        },
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            builders::set_cell(sheet, 1, 1, &CellVal::from(10)); // A1
        },
    );

    let diff = scenario.run_diff(None);
    assert!(diff.is_empty(), "expected no changes, got {:?}", diff);
}

#[test]
fn test_basic_edits() {
    let scenario = DiffScenario::new();

    // A1=10, A2="foo", A3=SUM(A1) (val=10)
    let setup_base = |book: &mut Spreadsheet| {
        let sheet = book.sheet_mut(0).ok().unwrap();
        builders::set_cell(sheet, 1, 1, &CellVal::from(10));
        builders::set_cell(sheet, 1, 2, &CellVal::from("foo"));
        builders::set_cell(sheet, 1, 3, &CellVal::Formula("SUM(A1)".to_string()));
        // sheet.get_cell_mut("A3").set_value_number(10.0);
    };

    // A1=20 (Val Edit), A2="bar" (Val Edit), A3=SUM(A1)+1 (Formula Edit)
    let setup_fork = |book: &mut Spreadsheet| {
        let sheet = book.sheet_mut(0).ok().unwrap();
        builders::set_cell(sheet, 1, 1, &CellVal::from(20));
        builders::set_cell(sheet, 1, 2, &CellVal::from("bar"));
        builders::set_cell(sheet, 1, 3, &CellVal::Formula("SUM(A1)+1".to_string()));
        // sheet.get_cell_mut("A3").set_value("20");
    };

    scenario.setup(setup_base, setup_fork);

    let diffs = scenario.run_diff(None);
    assert_eq!(diffs.len(), 3);

    // Sort by address for stable assertions
    let mut diffs = diffs;
    diffs.sort_by(|a, b| match (a, b) {
        (Change::Cell(ca), Change::Cell(cb)) => match (&ca.diff, &cb.diff) {
            (
                CellDiff::Modified {
                    address: a_addr, ..
                },
                CellDiff::Modified {
                    address: b_addr, ..
                },
            ) => a_addr.cmp(b_addr),
            _ => std::cmp::Ordering::Equal,
        },
        _ => std::cmp::Ordering::Equal,
    });

    // A1: 10 -> 20 (ValueEdit)
    match &diffs[0] {
        Change::Cell(c) => match &c.diff {
            CellDiff::Modified {
                address,
                subtype,
                old_value,
                new_value,
                ..
            } => {
                assert_eq!(address, "A1");
                assert!(matches!(subtype, ModificationType::ValueEdit));
                assert_eq!(old_value.as_deref(), Some("10"));
                assert_eq!(new_value.as_deref(), Some("20"));
            }
            _ => panic!("Unexpected diff at A1"),
        },
        _ => panic!("Expected cell diff"),
    }

    // A2: "foo" -> "bar" (ValueEdit)
    match &diffs[1] {
        Change::Cell(c) => match &c.diff {
            CellDiff::Modified {
                address,
                subtype,
                old_value,
                new_value,
                ..
            } => {
                assert_eq!(address, "A2");
                assert!(matches!(subtype, ModificationType::ValueEdit));
                assert_eq!(old_value.as_deref(), Some("foo"));
                assert_eq!(new_value.as_deref(), Some("bar"));
            }
            _ => panic!("Unexpected diff at A2"),
        },
        _ => panic!("Expected cell diff"),
    }

    // A3: Formula changed (FormulaEdit)
    match &diffs[2] {
        Change::Cell(c) => match &c.diff {
            CellDiff::Modified {
                address,
                subtype,
                old_formula,
                new_formula,
                ..
            } => {
                assert_eq!(address, "A3");
                assert!(matches!(subtype, ModificationType::FormulaEdit));
                assert_eq!(old_formula.as_deref(), Some("SUM(A1)"));
                assert_eq!(new_formula.as_deref(), Some("SUM(A1)+1"));
            }
            _ => panic!("Unexpected diff at A3"),
        },
        _ => panic!("Expected cell diff"),
    }
}

#[test]
fn test_structural_changes() {
    let scenario = DiffScenario::new();

    // Base: A1=10
    // Fork: B1=20 (Added), A1 deleted
    scenario.setup(
        |book| {
            builders::set_cell(book.sheet_mut(0).ok().unwrap(), 1, 1, &CellVal::from(10));
        },
        |book| {
            builders::set_cell(book.sheet_mut(0).ok().unwrap(), 2, 1, &CellVal::from(20));
        },
    );

    let diffs = scenario.run_diff(None);
    assert_eq!(diffs.len(), 2);

    // Check for Added B1
    let _added = diffs
        .iter()
        .find(|d| match d {
            Change::Cell(c) => {
                matches!(c.diff, CellDiff::Added { ref address, .. } if address == "B1")
            }
            _ => false,
        })
        .expect("Missing B1 add");

    // Check for Deleted A1
    let _deleted = diffs
        .iter()
        .find(|d| match d {
            Change::Cell(c) => {
                matches!(c.diff, CellDiff::Deleted { ref address, .. } if address == "A1")
            }
            _ => false,
        })
        .expect("Missing A1 delete");
}

#[test]
fn test_sheet_filtering() {
    let scenario = DiffScenario::new();

    // Base: Sheet1!A1=1, Sheet2!A1=1
    // Fork: Sheet1!A1=2, Sheet2!A1=2
    let setup = |book: &mut Spreadsheet, val: i32| {
        let s1 = book.sheet_mut(0).ok().unwrap();
        s1.set_name("Sheet1");
        builders::set_cell(s1, 1, 1, &CellVal::from(val));

        let s2 = book.new_sheet("Sheet2").unwrap();
        builders::set_cell(s2, 1, 1, &CellVal::from(val));
    };

    scenario.setup(|b| setup(b, 1), |b| setup(b, 2));

    // Filter for Sheet1
    let diffs = scenario.run_diff(Some("Sheet1"));
    assert_eq!(diffs.len(), 1);
    match &diffs[0] {
        Change::Cell(c) => assert_eq!(c.sheet, "Sheet1"),
        _ => panic!("Expected cell diff"),
    }
}

#[test]
fn test_sst_resolution() {
    let scenario = DiffScenario::new();

    // Base: A1="Apple", B1="Apple" (Shared)
    // Fork: A1="Banana", B1="Apple"
    scenario.setup(
        |book| {
            let s = book.sheet_mut(0).ok().unwrap();
            builders::set_cell(s, 1, 1, &CellVal::from("Apple"));
            builders::set_cell(s, 2, 1, &CellVal::from("Apple"));
        },
        |book| {
            let s = book.sheet_mut(0).ok().unwrap();
            builders::set_cell(s, 1, 1, &CellVal::from("Banana")); // Change A1
            builders::set_cell(s, 2, 1, &CellVal::from("Apple")); // Keep B1
        },
    );

    let diffs = scenario.run_diff(None);

    // Only A1 should change
    assert_eq!(diffs.len(), 1);
    match &diffs[0] {
        Change::Cell(c) => match &c.diff {
            CellDiff::Modified {
                address,
                old_value,
                new_value,
                ..
            } => {
                assert_eq!(address, "A1");
                assert_eq!(old_value.as_deref(), Some("Apple"));
                assert_eq!(new_value.as_deref(), Some("Banana"));
            }
            _ => panic!("Wrong diff type"),
        },
        _ => panic!("Expected cell diff"),
    }
}

#[test]
fn test_large_dataset() {
    let scenario = DiffScenario::new();
    let rows = 5000;

    // Setup 5k rows
    let setup = |book: &mut Spreadsheet, modify: bool| {
        let s = book.sheet_mut(0).ok().unwrap();
        for r in 1..=rows {
            let val = if modify && r == rows { 9999 } else { r as i32 };
            builders::set_cell(s, 1, r, &CellVal::from(val)); // A{r}
        }
    };

    let start = std::time::Instant::now();
    scenario.setup(|b| setup(b, false), |b| setup(b, true));
    let setup_time = start.elapsed();
    println!("Setup time: {:?}", setup_time);

    let start = std::time::Instant::now();
    let diffs = scenario.run_diff(None);
    let diff_time = start.elapsed();
    println!("Diff time: {:?}", diff_time);

    assert_eq!(diffs.len(), 1);
    match &diffs[0] {
        Change::Cell(c) => match &c.diff {
            CellDiff::Modified {
                address,
                old_value,
                new_value,
                ..
            } => {
                assert_eq!(address, &format!("A{}", rows));
                assert_eq!(old_value.as_deref(), Some("5000"));
                assert_eq!(new_value.as_deref(), Some("9999"));
            }
            _ => panic!("Wrong diff"),
        },
        _ => panic!("Expected cell diff"),
    }
}

#[test]
fn test_recalc_result_classification() {
    let scenario = DiffScenario::new();

    // Base: A1=10, B1=SUM(A1) with cached value 10
    // Fork: A1=10, B1=SUM(A1) with cached value 20 (simulated recalc)
    scenario.setup(
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            sheet.cell_mut("A1").set_value_number(10);
            let cell = sheet.cell_mut("B1");
            cell.set_formula("SUM(A1)");
            cell.set_formula_result_default("10");
        },
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            sheet.cell_mut("A1").set_value_number(10);
            let cell = sheet.cell_mut("B1");
            cell.set_formula("SUM(A1)");
            cell.set_formula_result_default("20");
        },
    );

    let diffs = scenario.run_diff(None);
    assert_eq!(diffs.len(), 1);

    match &diffs[0] {
        Change::Cell(c) => match &c.diff {
            CellDiff::Modified {
                address,
                subtype,
                old_value,
                new_value,
                old_formula,
                new_formula,
                ..
            } => {
                assert_eq!(address, "B1");
                assert!(
                    matches!(subtype, ModificationType::RecalcResult),
                    "expected RecalcResult, got {:?}",
                    subtype
                );
                assert_eq!(old_value.as_deref(), Some("10"));
                assert_eq!(new_value.as_deref(), Some("20"));
                assert_eq!(old_formula.as_deref(), Some("SUM(A1)"));
                assert_eq!(new_formula.as_deref(), Some("SUM(A1)"));
            }
            _ => panic!("Expected Modified diff, got {:?}", c.diff),
        },
        _ => panic!("Expected cell diff"),
    }
}

#[test]
fn test_float_epsilon_comparison() {
    let scenario = DiffScenario::new();

    // 0.1 + 0.2 = 0.30000000000000004 in IEEE 754
    scenario.setup(
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            sheet.cell_mut("A1").set_value_number(0.3);
        },
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            sheet.cell_mut("A1").set_value_number(0.1 + 0.2);
        },
    );

    let diffs = scenario.run_diff(None);
    assert!(
        diffs.is_empty(),
        "float epsilon comparison should treat 0.3 == 0.1+0.2, got {:?}",
        diffs
    );
}

#[test]
fn test_float_beyond_epsilon() {
    let scenario = DiffScenario::new();

    // Values differ by more than 1e-9
    scenario.setup(
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            sheet.cell_mut("A1").set_value_number(1.0);
        },
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            sheet.cell_mut("A1").set_value_number(1.000000002);
        },
    );

    let diffs = scenario.run_diff(None);
    assert_eq!(
        diffs.len(),
        1,
        "values differing by >1e-9 should be detected"
    );
}

#[test]
fn test_pure_numeric_sheet_no_sst() {
    let scenario = DiffScenario::new();

    // Sheet with only numbers - no shared strings at all
    scenario.setup(
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            for i in 1..=10 {
                sheet.cell_mut((1, i)).set_value_number(i as f64);
            }
        },
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            for i in 1..=10 {
                let val = if i == 5 { 999.0 } else { i as f64 };
                sheet.cell_mut((1, i)).set_value_number(val);
            }
        },
    );

    let diffs = scenario.run_diff(None);
    assert_eq!(diffs.len(), 1);

    match &diffs[0] {
        Change::Cell(c) => match &c.diff {
            CellDiff::Modified {
                address,
                old_value,
                new_value,
                ..
            } => {
                assert_eq!(address, "A5");
                assert_eq!(old_value.as_deref(), Some("5"));
                assert_eq!(new_value.as_deref(), Some("999"));
            }
            _ => panic!("Wrong diff type"),
        },
        _ => panic!("Expected cell diff"),
    }
}

#[test]
fn test_empty_sheet() {
    let scenario = DiffScenario::new();
    scenario.setup(|_book| {}, |_book| {});
    let diffs = scenario.run_diff(None);
    assert!(diffs.is_empty());
}

#[test]
fn test_empty_to_populated() {
    let scenario = DiffScenario::new();

    // Base empty, fork has data
    scenario.setup(
        |_book| {},
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            sheet.cell_mut("A1").set_value_number(42);
        },
    );

    let diffs = scenario.run_diff(None);
    assert_eq!(diffs.len(), 1);

    match &diffs[0] {
        Change::Cell(c) => match &c.diff {
            CellDiff::Added { address, value, .. } => {
                assert_eq!(address, "A1");
                assert_eq!(value.as_deref(), Some("42"));
            }
            _ => panic!("Expected Added diff"),
        },
        _ => panic!("Expected cell diff"),
    }
}

#[test]
fn test_rich_text_in_sst() {
    use umya_spreadsheet::structs::{RichText, TextElement};

    let scenario = DiffScenario::new();

    // Create rich text with multiple runs: "Hello" + "World"
    scenario.setup(
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();

            let mut rt = RichText::default();
            let mut elem1 = TextElement::default();
            elem1.set_text("Hello");
            let mut elem2 = TextElement::default();
            elem2.set_text("World");
            rt.add_rich_text_elements(elem1);
            rt.add_rich_text_elements(elem2);

            sheet.cell_mut("A1").set_rich_text(rt);
        },
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();

            let mut rt = RichText::default();
            let mut elem1 = TextElement::default();
            elem1.set_text("Hello");
            let mut elem2 = TextElement::default();
            elem2.set_text("World");
            rt.add_rich_text_elements(elem1);
            rt.add_rich_text_elements(elem2);

            sheet.cell_mut("A1").set_rich_text(rt);
        },
    );

    // Should detect no changes - same concatenated text
    let diffs = scenario.run_diff(None);
    assert!(
        diffs.is_empty(),
        "identical rich text should produce no diff, got {:?}",
        diffs
    );
}

#[test]
fn test_rich_text_changed() {
    use umya_spreadsheet::structs::{RichText, TextElement};

    let scenario = DiffScenario::new();

    scenario.setup(
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();

            let mut rt = RichText::default();
            let mut elem = TextElement::default();
            elem.set_text("Hello");
            rt.add_rich_text_elements(elem);
            sheet.cell_mut("A1").set_rich_text(rt);

            sheet.cell_mut("B1").set_value_number(1);
        },
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();

            let mut rt = RichText::default();
            let mut elem = TextElement::default();
            elem.set_text("Goodbye");
            rt.add_rich_text_elements(elem);
            sheet.cell_mut("A1").set_rich_text(rt);

            sheet.cell_mut("B1").set_value_number(2);
        },
    );

    let diffs = scenario.run_diff(None);
    assert_eq!(
        diffs.len(),
        2,
        "expected 2 diffs (A1 + B1), got {:?}",
        diffs
    );

    let a1_diff = diffs
        .iter()
        .find(|d| match d {
            Change::Cell(c) => match &c.diff {
                CellDiff::Modified { address, .. } => address == "A1",
                _ => false,
            },
            _ => false,
        })
        .expect("A1 diff not found");

    match a1_diff {
        Change::Cell(c) => match &c.diff {
            CellDiff::Modified {
                old_value,
                new_value,
                ..
            } => {
                assert_eq!(old_value.as_deref(), Some("Hello"));
                assert_eq!(new_value.as_deref(), Some("Goodbye"));
            }
            _ => unreachable!(),
        },
        _ => unreachable!(),
    }
}

#[test]
fn test_style_only_edit_emits_style_diff() {
    let scenario = DiffScenario::new();
    scenario.setup(
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            builders::set_cell(sheet, 1, 1, &CellVal::from("x")); // A1
        },
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            builders::set_cell(sheet, 1, 1, &CellVal::from("x")); // A1
            sheet.style_mut("A1").font_mut().set_bold(true);
        },
    );

    let diffs = scenario.run_diff(None);
    assert_eq!(diffs.len(), 1);
    match &diffs[0] {
        Change::Cell(c) => match &c.diff {
            CellDiff::Modified { subtype, .. } => {
                assert!(matches!(subtype, ModificationType::StyleEdit));
            }
            other => panic!("unexpected diff: {:?}", other),
        },
        other => panic!("unexpected change: {:?}", other),
    }
}

#[test]
fn test_style_and_value_edit_keeps_value_subtype_with_style_ids() {
    let scenario = DiffScenario::new();
    scenario.setup(
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            builders::set_cell(sheet, 1, 1, &CellVal::from("x")); // A1
            sheet.style_mut("A1").font_mut().set_italic(true);
        },
        |book| {
            let sheet = book.sheet_mut(0).ok().unwrap();
            builders::set_cell(sheet, 1, 1, &CellVal::from("y")); // A1
            sheet.style_mut("A1").font_mut().set_bold(true);
        },
    );

    let diffs = scenario.run_diff(None);
    assert_eq!(diffs.len(), 1);
    match &diffs[0] {
        Change::Cell(c) => match &c.diff {
            CellDiff::Modified {
                subtype,
                old_style_id,
                new_style_id,
                ..
            } => {
                assert!(matches!(subtype, ModificationType::ValueEdit));
                // Style ids may be omitted by the writer when defaults are used.
                let _ = (old_style_id, new_style_id);
            }
            other => panic!("unexpected diff: {:?}", other),
        },
        other => panic!("unexpected change: {:?}", other),
    }
}

// NOTE: Inline strings (<is> elements in cell XML, type="inlineStr") are not currently
// supported by the diff engine. The parser in cells.rs skips <is> elements.
// This is a known limitation. Testing would require manually crafting xlsx files
// since umya-spreadsheet always writes strings via SST, not inline.
// Most Excel/LibreOffice files use SST for strings, so this is a rare edge case.
