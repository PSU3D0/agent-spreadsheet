#![cfg(not(target_arch = "wasm32"))]
#[path = "../../agent-spreadsheet/tests/support/edit_xlsx.rs"]
mod edit_fixture;
#[path = "../../agent-spreadsheet/tests/support/source_xlsx.rs"]
mod fixture;
use agent_spreadsheet_wasm::SessionApi;
use fixture::*;
use serde_json::json;

#[test]
fn workbook_default_serializes_shared_settings_and_outcome() {
    let api = SessionApi::new();
    let input = pack(&stale_parts());
    let output = api.recalculate_source_xlsx(&input, Some(r#"{"calculation":{"workbookSeed":"18446744073709551615","fixedNowUtc":"2025-01-02T03:04:05Z","timezone":"utc"}}"#)).unwrap();
    let metadata = serde_json::to_value(output.metadata).unwrap();
    assert_eq!(
        metadata["calculation"]["workbookSeed"],
        "18446744073709551615"
    );
    assert_eq!(metadata["coverage"], "complete");
    assert_eq!(metadata["attempted"], true);
    assert_eq!(metadata["completed"], true);
    assert_eq!(metadata["artifactAvailable"], true);
    assert_eq!(metadata["bytesUnchanged"], false);
    assert!(metadata["operationDiagnostics"].is_object());
    assert_eq!(metadata["cachesInvalidated"], 0);
    assert!(
        api.recalculate_source_xlsx(&input, Some(r#"{"profile":"strict"}"#))
            .is_ok()
    );
}

#[test]
fn skipped_edits_validate_shared_calculation_without_evaluation() {
    let api = SessionApi::new();
    for mode in ["never", "auto"] {
        let request = json!({"recalculate":mode,"ops":[{"kind":"style","sheet_name":"Sheet1","target":{"kind":"range","range":"A1"},"patch":{"font":{"bold":true}}}]}).to_string();
        assert!(
            api.edit_xlsx(
                &edit_fixture::input(),
                &request,
                Some(r#"{"calculation":{"workbookSeed":"18446744073709551615"}}"#)
            )
            .is_ok()
        );
        for invalid in [
            r#"{"calculation":{"maxElapsedMs":0}}"#,
            r#"{"calculation":{"iteration":{"mode":"iterate","maxIterations":0}}}"#,
            r#"{"calculation":{"workbookSeed":9007199254740992}}"#,
            r#"{"calculation":{"unknown":1}}"#,
        ] {
            let error = api
                .edit_xlsx(&edit_fixture::input(), &request, Some(invalid))
                .unwrap_err();
            assert_eq!(error.code(), "INVALID_ARGUMENT", "{invalid}: {error}");
        }
    }
}

#[test]
fn verified_iteration_defers_without_invented_counts_and_strict_never_downgrades() {
    let api = SessionApi::new();
    let mut p = parts("<row r=\"1\"><c r=\"A1\"><f>(A1+1)/2</f><v>0.9</v></c></row>");
    let workbook = p.get_mut("xl/workbook.xml").unwrap();
    *workbook = workbook.replace(
        "</workbook>",
        "<calcPr iterate=\"1\" iterateCount=\"10\"/></workbook>",
    );
    let input = pack(&p);
    let result = api.recalculate_source_xlsx(&input, None).unwrap();
    assert_eq!(result.bytes, input);
    let meta = serde_json::to_value(result.metadata).unwrap();
    assert_eq!(meta["coverage"], "deferred");
    assert_eq!(meta["attempted"], true);
    assert_eq!(meta["completed"], false);
    assert_eq!(meta["bytesUnchanged"], true);
    for field in [
        "summary",
        "formulaCells",
        "cachesInvalidated",
        "operationDiagnostics",
        "externalLinksUsed",
    ] {
        assert!(meta[field].is_null(), "{field}");
    }
    for options in [r#"{"requireComplete":true}"#, r#"{"profile":"strict"}"#] {
        assert!(api.recalculate_source_xlsx(&input, Some(options)).is_err());
    }
}

#[test]
fn edited_iterative_artifact_survives_only_typed_workbook_deferral() {
    let api = SessionApi::new();
    let request = json!({"recalculate":"always","ops":[{
        "kind":"set_cells","sheet_name":"Sheet1","overwrite_formulas":true,
        "cells":{"A1":{"kind":"value","value":30},"B1":{"kind":"formula","formula":"(B1+1)/2"}}
    }]})
    .to_string();
    let options = r#"{"calculation":{"iteration":{"mode":"iterate","maxIterations":10}}}"#;
    let output = api
        .edit_xlsx(&edit_fixture::input(), &request, Some(options))
        .unwrap();
    let metadata = serde_json::to_value(output.metadata).unwrap();
    assert_eq!(metadata["edit"]["calculation"]["cache_state"], "deferred");
    assert_eq!(metadata["edit"]["calculation"]["completed"], false);
    assert_eq!(metadata["recalculation"]["coverage"], "deferred");
    assert!(metadata["recalculation"]["formulaCells"].is_null());
    let book =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(output.bytes), true)
            .unwrap();
    let sheet = book.sheet_by_name("Sheet1").unwrap();
    assert_eq!(sheet.cell("A1").unwrap().value(), "30");
    assert_eq!(sheet.cell("B1").unwrap().formula(), "(B1+1)/2");
    for options in [
        r#"{"requireComplete":true,"calculation":{"iteration":{"mode":"iterate"}}}"#,
        r#"{"profile":"strict","calculation":{"iteration":{"mode":"iterate"}}}"#,
        r#"{"limits":{"maxOutputBytes":1},"calculation":{"iteration":{"mode":"iterate"}}}"#,
    ] {
        assert!(
            api.edit_xlsx(&edit_fixture::input(), &request, Some(options))
                .is_err(),
            "{options}"
        );
    }
}

#[test]
fn partial_publication_uses_shared_outcome_and_invalidates_dependent_caches() {
    let input = pack(&parts(
        "<row r=\"1\"><c r=\"A1\"><f>UNAVAILABLE_FN(1)</f><v>123</v></c><c r=\"B1\"><f>A1+1</f><v>124</v></c><c r=\"C1\"><f>2+2</f><v>0</v></c></row>",
    ));
    let api = SessionApi::new();
    let output = api.recalculate_source_xlsx(&input, None).unwrap();
    let metadata = serde_json::to_value(output.metadata).unwrap();
    assert_eq!(metadata["coverage"], "partial");
    assert_eq!(metadata["coverageScope"], "conservative");
    assert_eq!(metadata["cachesInvalidated"], 2);
    assert_eq!(metadata["operationDiagnostics"]["logicalRootCellCount"], 1);
    let sheet = member(&output.bytes, SHEET);
    assert!(!sheet.contains("<v>123</v>"));
    assert!(!sheet.contains("<v>124</v>"));
    assert!(sheet.contains("<v>4</v>"));
    assert!(
        api.recalculate_source_xlsx(&input, Some(r#"{"requireComplete":true}"#))
            .is_err()
    );
}

#[test]
fn output_resource_and_corrupt_errors_never_become_deferred_artifacts() {
    let api = SessionApi::new();
    for options in [
        r#"{"limits":{"maxOutputBytes":1}}"#,
        r#"{"limits":{"maxFormulaCells":0}}"#,
    ] {
        assert!(
            api.recalculate_source_xlsx(&pack(&stale_parts()), Some(options))
                .is_err()
        );
        let request = json!({"recalculate":"always","ops":[{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":30}}}]}).to_string();
        assert!(
            api.edit_xlsx(&edit_fixture::input(), &request, Some(options))
                .is_err()
        );
    }
    assert!(api.recalculate_source_xlsx(b"bad zip", None).is_err());
}
