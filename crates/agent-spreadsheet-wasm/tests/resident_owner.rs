use agent_spreadsheet_wasm::{RangeSelectionInput, RangeValuesParams, SessionApi};
use serde_json::{Value, json};

#[tokio::test(flavor = "current_thread")]
async fn actual_adapter_retains_one_owner_across_positive_edits_and_original_retries() {
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.cell_mut("A1").set_value_number(1);
    sheet.cell_mut("B1").set_formula("A1*3");
    let mut bytes = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&book, &mut bytes).unwrap();
    let api = SessionApi::new();
    let id = api.create_session(&bytes).unwrap();
    let foreign = SessionApi::new();
    assert!(foreign.session_metadata(&id).is_err());
    let mut original = None;
    for value in 2..8 {
        let revision = api.session_metadata(&id).unwrap()["revision_id"].clone();
        let input = json!({"expected_revision":revision,"mode":"apply","ops":[{
            "kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":value}}
        }]})
        .to_string();
        let request_id = format!("edit-{value}");
        let written = api
            .execute_operation_with_request_id(&id, "write", &input, &request_id)
            .await
            .unwrap();
        if original.is_none() {
            original = Some((input.clone(), request_id, written.clone()));
        }
        let written: Value = serde_json::from_str(&written).unwrap();
        api.execute_operation_with_request_id(
            &id,
            "recalculate",
            &json!({"expected_revision":written["revision_id"]}).to_string(),
            &format!("calc-{value}"),
        )
        .await
        .unwrap();
        let read = api
            .range_values(
                &id,
                RangeValuesParams {
                    sheet_name: "Sheet1".into(),
                    ranges: RangeSelectionInput::Single("B1".into()),
                },
            )
            .unwrap();
        assert_eq!(
            serde_json::to_value(&read.values[0].rows).unwrap()[0][0],
            json!({"kind":"Number","value":f64::from(value * 3)})
        );
    }
    let metadata = api.session_metadata(&id).unwrap();
    assert_eq!(metadata["durability"], "memory");
    assert_eq!(metadata["evaluator"]["ingests"], 1);
    assert_eq!(metadata["evaluator"]["evaluations"], 6);
    assert_eq!(metadata["serializations"], 0);
    let (input, request_id, response) = original.unwrap();
    assert_eq!(
        api.execute_operation_with_request_id(&id, "write", &input, &request_id)
            .await
            .unwrap(),
        response
    );
    let first = api.export_workbook(&id).unwrap();
    assert_eq!(api.export_workbook(&id).unwrap(), first);
    assert_eq!(api.session_metadata(&id).unwrap()["serializations"], 1);
}
