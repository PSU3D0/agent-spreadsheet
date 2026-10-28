#![cfg(not(target_arch = "wasm32"))]
#[path = "../../agent-spreadsheet/tests/support/source_xlsx.rs"]
mod fixture;
use agent_spreadsheet_wasm::SessionApi;
#[test]
fn direct_read_metadata_preserves_cache_and_engine_distinction_without_sessions() {
    let api = SessionApi::new();
    let bytes = fixture::pack(&fixture::stale_parts());
    let original = bytes.clone();
    let inspected = api
        .inspect_xlsx(
            &bytes,
            r#"{"selection":{"kind":"cells","addresses":["Sheet1!B1"]}}"#,
        )
        .unwrap();
    assert_eq!(
        inspected.items[0]["stored_formula_cache"]["freshness"],
        "unverified"
    );
    let trace = api
        .trace_xlsx(&bytes, r#"{"roots":["Sheet1!B1"]}"#)
        .unwrap();
    assert_eq!(trace.items[0]["cell"]["staleness"], "NeverEvaluated");
    assert!(trace.items[0]["cell"]["value"].is_null());
    assert_eq!(bytes, original);
}
#[test]
fn unknown_options_and_stale_pages_are_plain_typed_errors() {
    let api = SessionApi::new();
    let bytes = fixture::pack(&fixture::stale_parts());
    let error = api
        .trace_xlsx(&bytes, r#"{"roots":["Sheet1!B1"],"calculate":true}"#)
        .unwrap_err();
    assert_eq!(error.code(), "INVALID_ARGUMENT");
    let error = api
        .inspect_xlsx(
            &bytes,
            r#"{"selection":{"kind":"range","address":"Sheet1!A1:B1"},"page":{"offset":1}}"#,
        )
        .unwrap_err();
    assert_eq!(error.code(), "STALE_CURSOR");
}

#[test]
fn typed_signed_zero_markers_survive_adapter_json_for_source_cache_and_engine() {
    let api = SessionApi::new();
    let input = fixture::pack(&fixture::parts(
        r#"<row r="1"><c r="A1"><v>-0.0</v></c><c r="B1"><f>A1</f><v>-0.0</v></c><c r="C1"><v>0.0</v></c><c r="D1"><f>C1</f><v>0.0</v></c></row>"#,
    ));
    let inspected = api
        .inspect_xlsx(
            &input,
            r#"{"selection":{"kind":"range","address":"Sheet1!A1:D1"}}"#,
        )
        .unwrap();
    let wire: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&inspected).unwrap()).unwrap();
    assert_eq!(wire["items"][0]["source_value"]["negative_zero"], true);
    assert_eq!(
        wire["items"][1]["stored_formula_cache"]["value"]["negative_zero"],
        true
    );
    assert!(
        wire["items"][2]["source_value"]
            .get("negative_zero")
            .is_none()
    );
    assert!(
        wire["items"][3]["stored_formula_cache"]["value"]
            .get("negative_zero")
            .is_none()
    );
    let trace = api
        .trace_xlsx(&input, r#"{"roots":["Sheet1!A1","Sheet1!C1"]}"#)
        .unwrap();
    let wire: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&trace).unwrap()).unwrap();
    assert_eq!(wire["items"][0]["cell"]["value"]["negative_zero"], true);
    assert!(
        wire["items"][1]["cell"]["value"]
            .get("negative_zero")
            .is_none()
    );
}
