#![cfg(feature = "recalc-formualizer")]

use agent_spreadsheet::core::session::SessionMatrixCell;
use agent_spreadsheet::recalc::{CalculationStamp, ResidentWorkbook};
use serde_json::json;

fn fixture() -> Vec<u8> {
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.cell_mut("A1").set_value_number(1.0);
    sheet.cell_mut("B1").set_formula("A1*2");
    sheet.cell_mut("C1").set_formula("B1+1");
    sheet.cell_mut("D1").set_value("fidelity");
    sheet.style_mut("D1").font_mut().set_bold(true);
    sheet.column_dimension_mut("D").set_width(24.0);
    sheet.add_merge_cells("D1:E1");
    sheet.add_defined_name("InputCell", "Sheet1!$A$1").unwrap();
    let mut bytes = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&book, &mut bytes).unwrap();
    bytes
}

#[test]
fn warm_reads_disclose_dirty_state_and_do_not_implicitly_calculate() {
    let mut resident = ResidentWorkbook::from_bytes(fixture()).unwrap();
    resident.recalculate(None).unwrap();
    resident
        .apply_write_matrix(
            "Sheet1",
            "A1",
            vec![vec![Some(SessionMatrixCell::Value(json!(5)))]],
            true,
        )
        .unwrap();
    assert!(matches!(
        resident.calculation_stamp(),
        CalculationStamp::Dirty { .. }
    ));
    let stale = resident.range_values("Sheet1", "B1").unwrap();
    assert!(matches!(
        stale[0].rows.as_ref().unwrap()[0][0],
        Some(agent_spreadsheet::model::CellValue::Number(value)) if value == 2.0
    ));
    assert_eq!(resident.evaluator_counters().evaluations, 1);
    resident.recalculate(None).unwrap();
    let current = resident.range_values("Sheet1", "B1").unwrap();
    assert!(matches!(
        current[0].rows.as_ref().unwrap()[0][0],
        Some(agent_spreadsheet::model::CellValue::Number(value)) if value == 10.0
    ));
}

#[test]
fn retained_engine_recalculates_dependencies_without_reingest() {
    let mut resident = ResidentWorkbook::from_bytes(fixture()).unwrap();
    let first = resident.recalculate(None).unwrap();
    assert_eq!(first.formula_cells, 2);
    let before = resident.evaluator_counters();
    assert_eq!(
        (before.constructions, before.ingests, before.evaluations),
        (1, 1, 1)
    );

    resident
        .apply_write_matrix(
            "Sheet1",
            "A1",
            vec![vec![Some(SessionMatrixCell::Value(json!(5)))]],
            false,
        )
        .unwrap();
    assert!(matches!(
        resident.calculation_stamp(),
        CalculationStamp::Dirty { .. }
    ));
    let second = resident.recalculate(None).unwrap();
    assert_eq!(second.evaluated_formula_cells, 2);
    let after = resident.evaluator_counters();
    assert_eq!(
        (
            after.constructions,
            after.ingests,
            after.evaluations,
            after.rebuilds
        ),
        (1, 1, 2, 0)
    );

    let exported = resident.export_bytes().unwrap();
    let book =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(exported), true).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    assert_eq!(sheet.cell("B1").unwrap().value(), "10");
    assert_eq!(sheet.cell("C1").unwrap().value(), "11");
}

#[test]
fn formula_update_stays_retained_and_export_uses_umya_document() {
    let mut resident = ResidentWorkbook::from_bytes(fixture()).unwrap();
    resident.recalculate(None).unwrap();
    resident
        .apply_write_matrix(
            "Sheet1",
            "B1",
            vec![vec![Some(SessionMatrixCell::Formula("=A1*3".into()))]],
            true,
        )
        .unwrap();
    resident.recalculate(None).unwrap();
    assert_eq!(resident.evaluator_counters().ingests, 1);
    let state_before_export = resident.revisions().state;
    let exported = resident.export_bytes().unwrap();
    assert_eq!(resident.revisions().state, state_before_export);

    let book =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(exported), true).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    assert_eq!(sheet.cell("B1").unwrap().value(), "3");
    assert!(sheet.style("D1").font().unwrap().bold());
    assert_eq!(sheet.column_dimension("D").unwrap().width(), 24.0);
    assert!(
        sheet
            .merge_cells()
            .iter()
            .any(|merge| merge.range() == "D1:E1")
    );
    assert!(
        sheet
            .defined_names()
            .iter()
            .any(|name| name.name() == "InputCell")
    );
}

#[test]
fn unsupported_change_rebuilds_before_publishing_coverage() {
    let mut resident = ResidentWorkbook::from_bytes(fixture()).unwrap();
    resident.recalculate(None).unwrap();
    resident.invalidate_after_unsupported_change();
    assert!(matches!(
        resident.calculation_stamp(),
        CalculationStamp::Dirty { .. }
    ));
    resident.recalculate(None).unwrap();
    assert_eq!(resident.evaluator_counters().rebuilds, 1);
    assert_eq!(
        resident.evaluator_counters(),
        agent_spreadsheet::recalc::EvaluatorCounters {
            constructions: 2,
            ingests: 2,
            evaluations: 2,
            rebuilds: 1,
        }
    );
    assert!(matches!(
        resident.calculation_stamp(),
        CalculationStamp::Current { .. }
    ));
}

fn type_fixture() -> Vec<u8> {
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.cell_mut("A1").set_value_number(1.0);
    for (cell, formula) in [
        ("B1", "ISNUMBER(A1)"),
        ("C1", "ISLOGICAL(A1)"),
        ("D1", "ISERROR(A1)"),
        ("E1", "ISBLANK(A1)"),
    ] {
        sheet.cell_mut(cell).set_formula(formula);
    }
    let mut bytes = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&book, &mut bytes).unwrap();
    bytes
}

fn typed_snapshot(resident: &mut ResidentWorkbook, cells: &[&str]) -> Vec<(String, String)> {
    let bytes = resident.export_bytes().unwrap();
    let book =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(bytes), true).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    cells
        .iter()
        .map(|address| {
            sheet.cell(*address).map_or_else(
                || (String::new(), String::new()),
                |cell| {
                    (
                        cell.value().to_string(),
                        cell.data_type().to_string(),
                    )
                },
            )
        })
        .collect()
}

#[test]
fn authoritative_umya_normalization_matches_warm_and_rebuilt_evaluation() {
    let cases = [
        (json!("123"), ("123", "n")),
        (json!("1e3"), ("1000", "n")),
        (json!("00123"), ("123", "n")),
        (json!("TrUe"), ("TRUE", "b")),
        (json!("fAlSe"), ("FALSE", "b")),
        (json!("#DIV/0!"), ("#DIV/0!", "e")),
        (json!(""), ("", "")),
        (serde_json::Value::Null, ("", "")),
        (json!(9_007_199_254_740_993_i64), ("9007199254740992", "n")),
        (
            json!({"kind": "text", "n": 1}),
            (r#"{"kind":"text","n":1}"#, "s"),
        ),
        (json!(["text", 1, true]), (r#"["text",1,true]"#, "s")),
    ];

    for (value, expected_a1) in cases {
        let mut resident = ResidentWorkbook::from_bytes(type_fixture()).unwrap();
        resident.recalculate(None).unwrap();
        resident
            .apply_write_matrix(
                "Sheet1",
                "A1",
                vec![vec![Some(SessionMatrixCell::Value(value.clone()))]],
                true,
            )
            .unwrap();
        resident.recalculate(None).unwrap();
        let warm = typed_snapshot(&mut resident, &["A1", "B1", "C1", "D1", "E1"]);
        assert_eq!(
            warm[0],
            (expected_a1.0.to_string(), expected_a1.1.to_string()),
            "authoritative normalization for {value}"
        );
        assert_eq!(resident.evaluator_counters().ingests, 1, "case {value}");

        resident.invalidate_after_unsupported_change();
        resident.recalculate(None).unwrap();
        let rebuilt = typed_snapshot(&mut resident, &["A1", "B1", "C1", "D1", "E1"]);
        assert_eq!(warm, rebuilt, "warm/rebuild mismatch for {value}");
    }
}

#[test]
fn formula_preservation_and_formula_to_value_overwrite_match_rebuild() {
    let mut preserved = ResidentWorkbook::from_bytes(fixture()).unwrap();
    preserved.recalculate(None).unwrap();
    let revision = preserved.revisions().clone();
    preserved
        .apply_write_matrix(
            "Sheet1",
            "B1",
            vec![vec![Some(SessionMatrixCell::Value(json!("00123")))]],
            false,
        )
        .unwrap();
    assert_eq!(preserved.revisions(), &revision);
    assert!(matches!(
        preserved.calculation_stamp(),
        CalculationStamp::Current { .. }
    ));
    assert_eq!(typed_snapshot(&mut preserved, &["B1"])[0].0, "2");

    let mut overwritten = ResidentWorkbook::from_bytes(fixture()).unwrap();
    overwritten.recalculate(None).unwrap();
    overwritten
        .apply_write_matrix(
            "Sheet1",
            "B1",
            vec![vec![Some(SessionMatrixCell::Value(json!("00123")))]],
            true,
        )
        .unwrap();
    overwritten.recalculate(None).unwrap();
    let warm = typed_snapshot(&mut overwritten, &["B1", "C1"]);
    assert_eq!(overwritten.evaluator_counters().ingests, 1);
    overwritten.invalidate_after_unsupported_change();
    overwritten.recalculate(None).unwrap();
    assert_eq!(warm, typed_snapshot(&mut overwritten, &["B1", "C1"]));
    assert_eq!(warm[0], ("123".into(), "n".into()));
    assert_eq!(warm[1].0, "124");
}

#[test]
fn proven_zero_effect_writes_preserve_revision_and_current_calculation() {
    let mut resident = ResidentWorkbook::from_bytes(fixture()).unwrap();
    resident.recalculate(None).unwrap();
    let revision = resident.revisions().clone();
    let calculation_revision = match resident.calculation_stamp() {
        CalculationStamp::Current { coverage, .. } => coverage.revision_id.clone(),
        other => panic!("expected current calculation, got {other:?}"),
    };

    resident
        .apply_write_matrix("Sheet1", "A1", Vec::new(), false)
        .unwrap();
    resident
        .apply_write_matrix("Sheet1", "A1", vec![vec![None]], false)
        .unwrap();
    resident
        .apply_write_matrix(
            "Sheet1",
            "B1",
            vec![vec![Some(SessionMatrixCell::Value(json!(99)))]],
            false,
        )
        .unwrap();

    assert_eq!(resident.revisions(), &revision);
    match resident.calculation_stamp() {
        CalculationStamp::Current { coverage, .. } => {
            assert_eq!(coverage.revision_id, calculation_revision)
        }
        other => panic!("zero-effect write dirtied calculation: {other:?}"),
    }
    assert_eq!(resident.evaluator_counters().evaluations, 1);
}
