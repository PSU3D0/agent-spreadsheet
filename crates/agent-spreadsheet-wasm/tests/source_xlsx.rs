#![cfg(not(target_arch = "wasm32"))]
#[path = "../../agent-spreadsheet/tests/support/source_xlsx.rs"]
mod fixture;
use agent_spreadsheet_wasm::{MAX_SESSIONS, SessionApi, SourceXlsxError};
use fixture::*;

#[test]
fn direct_original_bytes_bypass_session_creation_and_keep_resident_editing_working() {
    let api = SessionApi::new();
    // A full resident registry cannot block this stateless byte operation.
    let mut book = umya_spreadsheet::new_file();
    book.sheet_by_name_mut("Sheet1")
        .unwrap()
        .cell_mut("A1")
        .set_value_number(21);
    book.sheet_by_name_mut("Sheet1")
        .unwrap()
        .cell_mut("B1")
        .set_formula("A1*2");
    let mut editable = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&book, &mut editable).unwrap();
    let sessions: Vec<_> = (0..MAX_SESSIONS)
        .map(|_| api.create_session(&editable).unwrap())
        .collect();
    let input = pack(&stale_parts());
    let original = input.clone();
    let output = api.recalculate_source_xlsx(&input, None).unwrap();
    assert_eq!(input, original);
    assert!(member(&output.bytes, SHEET).contains("<f>A1*2</f><v>42</v>"));
    assert_eq!(
        member(&output.bytes, "custom/opaque.bin"),
        member(&input, "custom/opaque.bin")
    );
    assert_eq!(output.metadata.summary.evaluated, 1);
    assert_eq!(output.metadata.summary.errors, 0);
    assert_eq!(output.metadata.cache_cells_changed, 1);
    assert_eq!(output.metadata.worksheet_parts_changed, 1);
    assert!(output.metadata.clock_now_utc.is_some());
    let json = serde_json::to_value(&output.metadata).unwrap();
    assert!(json.get("bytes").is_none());
    let noop = api
        .recalculate_source_xlsx(&output.bytes, Some("{}"))
        .unwrap();
    assert_eq!(noop.bytes, output.bytes);
    assert_eq!(noop.metadata.cache_cells_changed, 0);
    for session in &sessions {
        assert!(api.session_metadata(session).is_ok());
    }
    // No registry slot was allocated by source recalc.
    assert!(api.dispose_session(&sessions[0]).unwrap());
    let replacement = api.create_session(&editable).unwrap();
    assert!(api.export_workbook(&replacement).is_ok());
}

#[test]
fn exact_error_counts_and_samples_are_serialized_separately() {
    let output = SessionApi::new()
        .recalculate_source_xlsx(&errors(201), Some(r#"{"errorLocationLimit":2}"#))
        .unwrap();
    let metadata = serde_json::to_value(&output.metadata).unwrap();
    assert_eq!(metadata["summary"]["status"], "errors_found");
    assert_eq!(metadata["summary"]["evaluated"], 201);
    assert_eq!(metadata["summary"]["errors"], 201);
    let stats = &metadata["summary"]["errorSummary"]["#DIV/0!"];
    assert_eq!(stats["count"], 201);
    assert_eq!(stats["locations"].as_array().unwrap().len(), 2);
    assert_eq!(stats["messages"].as_array().unwrap().len(), 2);
    assert_eq!(stats["locationsTruncated"], 199);
    assert!(!output.bytes.is_empty()); // Excel errors are successful result data.
}

#[test]
fn every_limit_override_is_applied_and_can_be_raised() {
    let api = SessionApi::new();
    let input = pack(&stale_parts());
    // Input/output/entries/expansion/depth/cell/formula/column limits all
    // reach upstream; none is accepted and ignored by the JSON adapter.
    for field in [
        "maxInputBytes",
        "maxEntries",
        "maxExpandedBytes",
        "maxWorksheetBytes",
        "maxFormulaCells",
        "maxOutputBytes",
        "maxXmlDepth",
        "maxCells",
        "maxColumns",
    ] {
        let options = serde_json::json!({"limits": {field: 0}}).to_string();
        assert!(
            matches!(
                api.recalculate_source_xlsx(&input, Some(&options)),
                Err(SourceXlsxError::Recalculate(_))
            ),
            "{field}"
        );
    }
    let options = serde_json::json!({"limits": {
        "maxInputBytes": input.len(), "maxFormulaCells": 1,
        "maxColumns": 2, "maxOutputBytes": 65536
    }})
    .to_string();
    assert!(api.recalculate_source_xlsx(&input, Some(&options)).is_ok());
}

#[test]
fn refused_inputs_and_unknown_or_unavailable_options_have_no_success_bytes() {
    let api = SessionApi::new();
    let mut signed = stale_parts();
    signed.insert("_xmlsignatures/sig1.xml".into(), "<Signature/>".into());
    let error = api
        .recalculate_source_xlsx(&pack(&signed), None)
        .unwrap_err();
    assert_eq!(error.code(), "UNSUPPORTED");
    for options in [
        r#"{"limits":{"maxColums":2}}"#,
        r#"{"evalConfig":{}}"#,
        r#"{"cancel":true}"#,
        r#"{"externalLinks":"refresh"}"#,
        r#"{"errorLocationLimit":-1}"#,
        r#"{"limits":null}"#,
    ] {
        assert!(
            matches!(
                api.recalculate_source_xlsx(&pack(&stale_parts()), Some(options)),
                Err(SourceXlsxError::InvalidOptions(_))
            ),
            "{options}"
        );
    }
}

#[test]
fn external_cached_values_and_refuse_policy_are_not_ignored() {
    let api = SessionApi::new();
    let input = external();
    let output = api
        .recalculate_source_xlsx(&input, Some(r#"{"externalLinks":"cached"}"#))
        .unwrap();
    assert_eq!(output.metadata.external_links_used, 1);
    assert!(member(&output.bytes, SHEET).contains("<v>42</v>"));
    let error = api
        .recalculate_source_xlsx(&input, Some(r#"{"externalLinks":"refuse"}"#))
        .unwrap_err();
    assert_eq!(error.code(), "UNSUPPORTED");
    assert!(error.to_string().contains("external link values"));
}
