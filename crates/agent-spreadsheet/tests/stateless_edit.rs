#![cfg(feature = "recalc-formualizer")]
#[path = "support/edit_xlsx.rs"]
mod fixture;
use agent_spreadsheet::stateless_edit::{EditedArtifact, StatelessEditRequest, edit_xlsx_bytes};
use fixture::*;
use serde_json::{Value, json};
#[path = "support/source_xlsx.rs"]
mod source_fixture;

fn set(value: i64) -> Value {
    json!({"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":value}}})
}
fn request(ops: Value, mode: &str) -> StatelessEditRequest {
    serde_json::from_value(json!({"ops":ops,"recalculate":mode})).unwrap()
}
fn cell(bytes: &[u8], address: &str) -> String {
    let book =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(bytes), true).unwrap();
    book.sheet_by_name("Sheet1")
        .unwrap()
        .cell(address)
        .unwrap()
        .value()
        .to_string()
}
#[test]
fn never_edits_without_validating_cache_auto_and_always_repair_to_sixty() {
    let original = input();
    for mode in ["never", "auto", "always"] {
        let source = original.clone();
        let output =
            edit_xlsx_bytes(&source, request(json!([set(30)]), mode), Default::default()).unwrap();
        assert_eq!(source, original);
        assert_eq!(cell(output.bytes(), "A1"), "30");
        assert_eq!(output.receipt.ops_applied, 1);
        assert!(output.receipt.details.is_none());
        assert_eq!(output.receipt.change_count, None);
        assert!(!output.receipt.diff_exact);
        assert_eq!(output.receipt.calculation.performed, mode != "never");
        if mode == "never" {
            assert_eq!(output.receipt.calculation.cache_state, "unknown");
        } else {
            assert_eq!(cell(output.bytes(), "B1"), "60");
            let EditedArtifact::Recalculated(result) = &output.artifact else {
                panic!("calculation missing")
            };
            assert_eq!(result.summary.evaluated, 1);
        }
    }
}
#[test]
fn formula_edits_and_independent_reopen_need_no_session_handles() {
    let first = edit_xlsx_bytes(
        &input(),
        request(json!([set(30)]), "auto"),
        Default::default(),
    )
    .unwrap();
    let formula = json!({"kind":"set_cells","sheet_name":"Sheet1","overwrite_formulas":true,"cells":{"B1":{"kind":"formula","formula":"A1*3"}}});
    let second = edit_xlsx_bytes(
        first.bytes(),
        request(json!([formula]), "auto"),
        Default::default(),
    )
    .unwrap();
    assert_eq!(cell(second.bytes(), "B1"), "90");
    assert_eq!(second.receipt.calculation.reason, "auto_invalidating_batch");
}
#[test]
fn style_layout_auto_skips_but_always_is_explicit() {
    let ops = json!([
        {"kind":"style","sheet_name":"Sheet1","target":{"kind":"range","range":"A1:A1"},"patch":{"font":{"bold":true}}},
        {"kind":"column_size","sheet_name":"Sheet1","target":{"kind":"columns","range":"A:A"},"size":{"kind":"width","width_chars":18.0}}
    ]);
    for mode in ["auto", "always"] {
        let output =
            edit_xlsx_bytes(&input(), request(ops.clone(), mode), Default::default()).unwrap();
        assert_eq!(output.receipt.calculation.performed, mode == "always");
        if mode == "auto" {
            assert_eq!(output.receipt.calculation.cache_state, "unknown");
            assert_eq!(output.receipt.calculation.recommendation, "unknown");
        }
    }
}
#[test]
fn batch_failure_or_selected_strict_refusal_returns_no_artifact() {
    let source = input();
    let original = source.clone();
    let bad = json!({"kind":"set_cells","sheet_name":"Missing","cells":{"A1":{"kind":"value","value":1}}});
    assert!(
        edit_xlsx_bytes(
            &source,
            request(json!([set(30), bad]), "never"),
            Default::default()
        )
        .is_err()
    );
    let mut options = agent_spreadsheet::recalc::source_xlsx::XlsxRecalculateOptions::default();
    options.limits.max_formula_cells = 0;
    assert!(edit_xlsx_bytes(&source, request(json!([set(30)]), "always"), options).is_err());
    assert_eq!(source, original);
}
#[test]
fn invalid_request_fields_modes_and_ops_are_rejected() {
    for value in [
        json!({"ops":[set(30)],"recalculate":"sometimes"}),
        json!({"ops":[set(30)],"resource_id":"session:caller"}),
        json!({"ops":[set(30)],"expected_revision":"x"}),
        json!({"ops":[{"kind":"not_an_operation"}]}),
    ] {
        assert!(serde_json::from_value::<StatelessEditRequest>(value).is_err());
    }
    assert!(edit_xlsx_bytes(&input(), request(json!([]), "auto"), Default::default()).is_err());
}

#[test]
fn explicit_details_match_existing_canonical_diff_and_operation_facts() {
    use agent_spreadsheet::canonical_write::{
        WriteRequest, WriteResponseData, execute_write_on_bytes,
    };
    let original = input();
    let mut detailed = request(json!([set(30)]), "never");
    detailed.details = true;
    let output = edit_xlsx_bytes(&original, detailed, Default::default()).unwrap();
    let canonical: WriteRequest = serde_json::from_value(json!({
        "resource_id":"session:test", "expected_revision":"state:test",
        "mode":"apply", "atomic":true, "ops":[set(30)]
    }))
    .unwrap();
    let (response, _) = execute_write_on_bytes(&original, "state:test", canonical).unwrap();
    let WriteResponseData::Applied { diff, results, .. } = response else {
        panic!("not applied")
    };
    assert!(diff.change_count > 0);
    assert_eq!(output.receipt.change_count, Some(diff.change_count));
    assert_eq!(output.receipt.diff_exact, diff.exact);
    assert_eq!(
        serde_json::to_value(output.receipt.details.unwrap()).unwrap(),
        serde_json::to_value(results).unwrap()
    );
}

#[test]
fn known_style_free_rich_writer_refusal_is_actionable_and_keeps_input() {
    let original = source_fixture::pack(&source_fixture::stale_parts());
    let source = original.clone();
    let error = edit_xlsx_bytes(
        &source,
        request(json!([set(30)]), "never"),
        Default::default(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("invalid emitted style reference"),
        "{error}"
    );
    assert_eq!(source, original);
}

#[test]
fn explicit_details_keep_existing_facts_larger_than_sixty_four_kib() {
    let cells: serde_json::Map<String, Value> = (2..=6001)
        .map(|row| (format!("A{row}"), json!({"kind":"value","value":row})))
        .collect();
    let mut detailed = request(
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":cells}]),
        "never",
    );
    detailed.details = true;
    let output = edit_xlsx_bytes(&input(), detailed, Default::default()).unwrap();
    let details = output.receipt.details.unwrap();
    assert_eq!(details.len(), 1);
    assert!(serde_json::to_vec(&details).unwrap().len() > 64 * 1024);
    assert_eq!(
        details[0].detail.as_ref().unwrap()["affected_bounds"]
            .as_array()
            .unwrap()
            .len(),
        6000
    );
    assert!(output.receipt.change_count.unwrap() >= 6000);
}
