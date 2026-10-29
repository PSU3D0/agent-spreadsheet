#![cfg(not(target_arch = "wasm32"))]
#[path = "../../agent-spreadsheet/tests/support/source_xlsx.rs"]
mod fixture;
use agent_spreadsheet_wasm::{SessionApi, SourceXlsxError};
use fixture::*;
use serde_json::json;

fn formula(value: &str) -> Vec<u8> {
    pack(&parts(&format!(
        "<row r=\"1\"><c r=\"A1\"><f>{value}</f><v>99</v></c><c r=\"B1\"><v>7</v></c></row>"
    )))
}

#[test]
fn physical_telemetry_is_opt_in_and_preserves_exact_zero() {
    let api = SessionApi::new();
    let input = formula("1+1");
    let default = api.recalculate_source_xlsx(&input, None).unwrap();
    let meta = serde_json::to_value(default.metadata).unwrap();
    assert!(meta["operationDiagnostics"]["physicalFunctionDispatches"].is_null());
    let enabled = api
        .recalculate_source_xlsx(&input, Some(r#"{"physicalTelemetry":true}"#))
        .unwrap();
    let meta = serde_json::to_value(enabled.metadata).unwrap();
    assert_eq!(
        meta["operationDiagnostics"]["physicalFunctionDispatches"],
        0
    );
    assert_eq!(
        meta["operationDiagnostics"]["physicalUnavailableObservations"],
        0
    );
}

#[test]
fn canonical_diagnostic_serializer_preserves_large_exact_counters() {
    use agent_spreadsheet::recalc::source_xlsx::SourceOperationDiagnostics;
    let report = SourceOperationDiagnostics {
        physical_function_dispatches: Some(u64::MAX),
        physical_unavailable_observations: Some(0),
        ..Default::default()
    };
    let value = serde_json::to_value(report).unwrap();
    assert_eq!(value["physicalFunctionDispatches"], u64::MAX.to_string());
    assert_eq!(value["physicalUnavailableObservations"], 0);
}

#[test]
fn all_new_evidence_limits_are_typed_fatal_with_zero_samples_and_recover() {
    let api = SessionApi::new();
    let input = formula("MISSING_FN(1)");
    for field in [
        "maxOperationRoots",
        "maxOperationTextBytes",
        "maxAffectedWork",
        "maxAffectedRegions",
        "maxAffectedScratchBytes",
    ] {
        let options = json!({"errorLocationLimit":0,"limits":{field:0}}).to_string();
        assert!(
            matches!(
                api.recalculate_source_xlsx(&input, Some(&options)),
                Err(SourceXlsxError::Recalculate(
                    agent_spreadsheet::recalc::source_xlsx::IoError::Engine(_)
                ))
            ),
            "{field}"
        );
        assert!(api.recalculate_source_xlsx(&input, None).is_ok());
    }
    let strict = r#"{"profile":"strict","limits":{"maxAffectedWork":0}}"#;
    assert!(api.recalculate_source_xlsx(&input, Some(strict)).is_err());
}

#[test]
fn canonical_portable_limits_reject_nonfinite_or_noninteger_and_preserve_presets() {
    let api = SessionApi::new();
    let input = formula("1+1");
    for value in ["null", "true", "-1", "0.5", "4294967296"] {
        let options = format!(r#"{{"limits":{{"maxOperationRoots":{value}}}}}"#);
        assert!(
            api.recalculate_source_xlsx(&input, Some(&options)).is_err(),
            "{value}"
        );
    }
    for options in [
        r#"{"limits":[]}"#,
        r#"{"limits":{"maxAffectedWork":9007199254740992}}"#,
    ] {
        assert!(api.recalculate_source_xlsx(&input, Some(options)).is_err());
    }
    assert!(
        api.recalculate_source_xlsx(
            &input,
            Some(r#"{"limits":{"maxAffectedWork":9007199254740991}}"#)
        )
        .is_ok()
    );
}

#[test]
fn actual_modes_and_unverified_callables_are_independent_of_sample_display() {
    let api = SessionApi::new();
    for f in [
        r#"CELL("format",B1)"#,
        r#"IFERROR(CELL("format",B1),7)"#,
        "PIVOTBY(B1,B1,B1,SUM)",
        "IF(TRUE,SUM,AVERAGE)",
        "HSTACK(SUM,AVERAGE)",
    ] {
        let input = formula(f);
        let out = api
            .recalculate_source_xlsx(&input, Some(r#"{"errorLocationLimit":0}"#))
            .unwrap();
        let meta = serde_json::to_value(out.metadata).unwrap();
        assert_eq!(meta["coverage"], "partial", "{f}");
        assert!(
            !meta["operationDiagnostics"]["functionModes"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{f}"
        );
        assert_eq!(meta["operationDiagnostics"]["samples"], json!([]));
        assert!(
            api.recalculate_source_xlsx(&input, Some(r#"{"requireComplete":true}"#))
                .is_err()
        );
    }
    for f in [
        "1/0",
        "IFERROR(1/0,7)",
        r#"CELL("invalid",B1)"#,
        r#"IF(FALSE,CELL("format",B1),7)"#,
        "IF(FALSE,IF(TRUE,SUM,AVERAGE),7)",
        r#"CELL("row",B1)"#,
    ] {
        let out = api
            .recalculate_source_xlsx(&formula(f), Some(r#"{"requireComplete":true}"#))
            .unwrap();
        assert_eq!(
            serde_json::to_value(out.metadata).unwrap()["coverage"],
            "complete",
            "{f}"
        );
    }
}

#[test]
fn independent_dynamic_scalar_and_spill_caches_survive_actual_reopen() {
    let input = pack(&parts(
        r#"<row r="1"><c r="A1"><f>MISSING_FN(1)</f><v>99</v></c><c r="B1"><f>INDIRECT("A1")</f><v>88</v></c><c r="D1"><v>7</v></c><c r="E1"><f>INDIRECT("D1")</f><v>66</v></c><c r="F1"><f>E1+1</f><v>55</v></c></row><row r="3"><c r="D3"><v>7</v></c><c r="E3"><f>INDIRECT("D3:D4")</f><v>44</v></c></row><row r="4"><c r="D4"><v>8</v></c></row>"#,
    ));
    let out = SessionApi::new()
        .recalculate_source_xlsx(&input, None)
        .unwrap();
    let meta = serde_json::to_value(out.metadata).unwrap();
    assert_eq!(meta["coverage"], "partial");
    assert_eq!(
        meta["operationDiagnostics"]["dependencyEvidence"]["unknownFacts"],
        0
    );
    assert_eq!(
        meta["operationDiagnostics"]["dependencyEvidence"]["semanticReadsUnattested"],
        true
    );
    let xml = member(&out.bytes, SHEET);
    assert!(xml.contains(r#"<f>INDIRECT("D1")</f><v>7</v>"#), "{xml}");
    assert!(xml.contains("<f>E1+1</f><v>8</v>"), "{xml}");
    let book =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(out.bytes), true).unwrap();
    let sheet = book.sheet_by_name("Sheet1").unwrap();
    assert_eq!(sheet.cell("E1").unwrap().formula(), r#"INDIRECT("D1")"#);
    for (address, value) in [("E1", "7"), ("F1", "8"), ("E3", "7"), ("E4", "8")] {
        assert_eq!(sheet.cell(address).unwrap().value(), value, "{address}");
    }
}
