#![cfg(not(target_arch = "wasm32"))]
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
    assert_eq!(metadata["calculation"]["workbookSeed"], "18446744073709551615");
    assert_eq!(metadata["coverage"], "complete");
    assert_eq!(metadata["attempted"], true);
    assert_eq!(metadata["completed"], true);
    assert_eq!(metadata["artifactAvailable"], true);
    assert_eq!(metadata["bytesUnchanged"], false);
    assert!(metadata["operationDiagnostics"].is_object());
    assert_eq!(metadata["cachesInvalidated"], 0);
    assert!(api.recalculate_source_xlsx(&input, Some(r#"{"profile":"strict"}"#)).is_ok());
}

#[test]
fn skipped_edits_validate_shared_calculation_without_evaluation() {
    let api = SessionApi::new();
    for mode in ["never", "auto"] {
        let request = json!({"recalculate":mode,"ops":[]}).to_string();
        assert!(api.edit_xlsx(&pack(&stale_parts()), &request,
            Some(r#"{"calculation":{"workbookSeed":"18446744073709551615"}}"#)).is_ok());
        for invalid in [
            r#"{"calculation":{"maxElapsedMs":0}}"#,
            r#"{"calculation":{"iteration":{"mode":"iterate","maxIterations":0}}}"#,
            r#"{"calculation":{"workbookSeed":9007199254740992}}"#,
            r#"{"calculation":{"unknown":1}}"#,
        ] {
            let error = api.edit_xlsx(&pack(&stale_parts()), &request, Some(invalid)).unwrap_err();
            assert_eq!(error.code(), "INVALID_ARGUMENT", "{invalid}: {error}");
        }
    }
}

#[test]
fn output_resource_and_corrupt_errors_never_become_deferred_artifacts() {
    let api = SessionApi::new();
    for options in [r#"{"limits":{"maxOutputBytes":1}}"#, r#"{"limits":{"maxFormulaCells":0}}"#] {
        assert!(api.recalculate_source_xlsx(&pack(&stale_parts()), Some(options)).is_err());
        let request = json!({"recalculate":"always","ops":[]}).to_string();
        assert!(api.edit_xlsx(&pack(&stale_parts()), &request, Some(options)).is_err());
    }
    assert!(api.recalculate_source_xlsx(b"bad zip", None).is_err());
}
