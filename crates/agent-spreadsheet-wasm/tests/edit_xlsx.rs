#![cfg(not(target_arch = "wasm32"))]
#[path = "../../agent-spreadsheet/tests/support/edit_xlsx.rs"]
mod fixture;
use agent_spreadsheet_wasm::SessionApi;
use fixture::*;
use serde_json::json;

fn ops(mode: &str) -> String {
    json!({"recalculate":mode,"ops":[{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":30}}}]}).to_string()
}
#[test]
fn direct_edit_returns_separate_typed_bytes_and_compact_receipt() {
    let api = SessionApi::new();
    let original = input();
    for mode in ["never", "auto", "always"] {
        let output = api.edit_xlsx(&original, &ops(mode), None).unwrap();
        let metadata = serde_json::to_value(&output.metadata).unwrap();
        assert!(metadata.get("bytes").is_none());
        assert!(metadata["edit"].get("details").is_none());
        assert!(metadata["edit"]["change_count"].is_null());
        assert_eq!(metadata["edit"]["diff_exact"], false);
        assert_eq!(
            metadata["edit"]["calculation"]["performed"],
            mode != "never"
        );
        assert!(!output.bytes.is_empty());
        if mode != "never" {
            assert_eq!(metadata["recalculation"]["summary"]["evaluated"], 1);
            let book = umya_spreadsheet::reader::xlsx::read_reader(
                std::io::Cursor::new(&output.bytes),
                true,
            )
            .unwrap();
            assert_eq!(
                book.sheet_by_name("Sheet1")
                    .unwrap()
                    .cell("B1")
                    .unwrap()
                    .value(),
                "60"
            );
        }
    }
    assert_eq!(original, input());
}
#[test]
fn strict_options_and_unknown_edit_fields_are_not_ignored() {
    let api = SessionApi::new();
    for options in [r#"{"limits":{"maxFormulaCells":0}}"#, r#"{"unknown":1}"#] {
        let error = api
            .edit_xlsx(&input(), &ops("always"), Some(options))
            .unwrap_err();
        assert!(!error.code().is_empty());
        assert!(!error.to_string().is_empty());
    }
    assert!(
        api.edit_xlsx(&input(), r#"{"ops":[],"mode":"preview"}"#, None)
            .is_err()
    );
    assert!(
        api.edit_xlsx(&input(), &ops("never"), Some(r#"{"cancel":true}"#))
            .is_err()
    );
}
