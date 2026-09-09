#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::sync::Arc;

use agent_spreadsheet::config::{OutputProfile, RecalcBackendKind, ServerConfig, TransportKind};
use agent_spreadsheet::operations::{CanonicalErrorCode, ResourceId, execute_operation_json};
use agent_spreadsheet::repository::{VirtualWorkbookInput, VirtualWorkspaceRepository};
use agent_spreadsheet::state::AppState;
use agent_spreadsheet_wasm::{MAX_PARAMS_JSON_BYTES, SessionApi};
use serde_json::{Value, json};

fn workbook_bytes() -> Vec<u8> {
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().expect("sheet");
    sheet.get_cell_mut("A1").set_value("Name");
    sheet.get_cell_mut("A2").set_value("Ada");
    let mut bytes = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&book, &mut bytes).expect("write workbook");
    bytes
}

fn config() -> Arc<ServerConfig> {
    Arc::new(ServerConfig {
        workspace_root: PathBuf::new(),
        screenshot_dir: PathBuf::new(),
        path_mappings: Vec::new(),
        cache_capacity: 1,
        supported_extensions: vec!["xlsx".to_string()],
        single_workbook: None,
        enabled_tools: None,
        transport: TransportKind::Stdio,
        http_bind_address: "127.0.0.1:0".parse().expect("address"),
        recalc_enabled: false,
        recalc_backend: RecalcBackendKind::Formualizer,
        vba_enabled: false,
        max_concurrent_recalcs: 1,
        tool_timeout_ms: None,
        max_response_bytes: Some(1_000_000),
        output_profile: OutputProfile::TokenDense,
        max_payload_bytes: Some(65_536),
        max_cells: Some(10_000),
        max_items: Some(500),
        allow_overwrite: false,
        slim_surface: true,
    })
}

fn normalize(mut response: Value) -> Value {
    response["resource_id"] = json!("<resource>");
    response["revision_id"] = json!("<revision>");
    response["data"]["workbook_id"] = json!("<workbook>");
    response
}

#[tokio::test(flavor = "current_thread")]
async fn supported_read_matches_native_canonical_dispatcher() {
    let bytes = workbook_bytes();
    let api = SessionApi::new();
    let session_id = api.create_session(&bytes).expect("session");
    assert!(session_id.starts_with("session:"));

    let adapter: Value = serde_json::from_str(
        &api.execute_operation(&session_id, "list_sheets", "{}")
            .await
            .expect("adapter response"),
    )
    .expect("adapter JSON");

    let config = config();
    let repository = Arc::new(VirtualWorkspaceRepository::new(config.clone()));
    let workbook_id = repository.register(VirtualWorkbookInput {
        key: "native-golden".to_string(),
        slug: Some("golden".to_string()),
        bytes,
    });
    let state = Arc::new(AppState::new_with_repository(config, repository));
    let resource_id = ResourceId::bind_workbook(&workbook_id).expect("resource");
    let native = execute_operation_json(state, "list_sheets", json!({"resource_id": resource_id}))
        .await
        .expect("native response");
    let native = serde_json::to_value(native).expect("native JSON");

    assert_eq!(normalize(adapter), normalize(native));
}

#[tokio::test(flavor = "current_thread")]
async fn canonical_errors_and_capabilities_are_conservative() {
    let api = SessionApi::new();
    let session_id = api.create_session(&workbook_bytes()).expect("session");

    for operation in ["list_workbooks", "inspect_vba", "sheetport_manifest"] {
        let error = api
            .execute_operation(&session_id, operation, "{}")
            .await
            .expect_err("operation must be hidden");
        assert_eq!(error.error.code, CanonicalErrorCode::CapabilityUnavailable);
        assert_eq!(error.error.operation.as_deref(), Some(operation));
    }

    let discovery: Value =
        serde_json::from_str(&api.operations_json().expect("operations")).expect("operations JSON");
    let names: Vec<&str> = discovery
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(names.contains(&"list_sheets"));
    assert!(names.contains(&"write"));
    assert!(names.contains(&"recalculate"));
    assert!(names.contains(&"verify_workbook"));
    assert!(!names.contains(&"list_workbooks"));
    assert!(!names.contains(&"inspect_vba"));
    assert!(!names.iter().any(|name| name.contains("fork")));

    let oversized = " ".repeat(MAX_PARAMS_JSON_BYTES + 1);
    let error = api
        .execute_operation(&session_id, "list_sheets", &oversized)
        .await
        .expect_err("oversized params");
    assert_eq!(error.error.code, CanonicalErrorCode::InvalidRequest);

    let error = api
        .execute_operation(&session_id, "not_an_operation", "{}")
        .await
        .expect_err("unknown operation");
    assert_eq!(error.error.code, CanonicalErrorCode::UnknownOperation);
}

#[tokio::test(flavor = "current_thread")]
async fn canonical_write_persists_export_and_enforces_cas_and_stage_policy() {
    let api = SessionApi::new();
    let session_id = api.create_session(&workbook_bytes()).expect("session");
    let read: Value = serde_json::from_str(
        &api.execute_operation(&session_id, "list_sheets", "{}")
            .await
            .expect("read"),
    )
    .expect("read JSON");
    let revision = read["revision_id"].as_str().expect("revision");
    let before_preview = api
        .export_workbook(&session_id)
        .expect("pre-preview export");

    let preview: Value = serde_json::from_str(
        &api.execute_operation(
            &session_id,
            "write",
            &json!({
                "expected_revision": revision,
                "mode": "preview",
                "ops": [{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A2":{"kind":"value","value":"Grace"}}}]
            }).to_string(),
        )
        .await
        .expect("preview"),
    )
    .expect("preview JSON");
    assert_eq!(preview["data"]["status"], "previewed");
    assert_eq!(preview["revision_id"], revision);
    assert_eq!(
        api.export_workbook(&session_id)
            .expect("post-preview export"),
        before_preview
    );

    let applied: Value = serde_json::from_str(
        &api.execute_operation(
            &session_id,
            "write",
            &json!({
                "expected_revision": revision,
                "mode": "apply",
                "ops": [{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A2":{"kind":"value","value":"Grace"}}}]
            }).to_string(),
        )
        .await
        .expect("apply"),
    )
    .expect("apply JSON");
    assert_eq!(applied["data"]["status"], "applied");
    let next_revision = applied["revision_id"].as_str().expect("next revision");
    assert_ne!(next_revision, revision);
    let exported = api.export_workbook(&session_id).expect("export");
    let reopened = agent_spreadsheet::core::session::WorkbookSession::from_bytes(exported)
        .expect("reopen export");
    let cells = reopened
        .range_values("Sheet1", "A2")
        .expect("read exported");
    assert!(matches!(
        cells[0].rows.as_ref().unwrap()[0][0],
        Some(agent_spreadsheet::model::CellValue::Text(ref value)) if value == "Grace"
    ));

    let stale = api
        .execute_operation(
            &session_id,
            "write",
            &json!({
                "expected_revision": revision,
                "mode": "apply",
                "ops": [{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A2":{"kind":"value","value":"stale"}}}]
            }).to_string(),
        )
        .await
        .expect_err("stale CAS");
    assert_eq!(stale.error.code, CanonicalErrorCode::RevisionConflict);

    let stage = api
        .execute_operation(
            &session_id,
            "write",
            &json!({
                "expected_revision": next_revision,
                "mode": "stage",
                "ops": [{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A2":{"kind":"value","value":"staged"}}}]
            }).to_string(),
        )
        .await
        .expect("stage retained in the volatile resident journal");
    let stage: Value = serde_json::from_str(&stage).unwrap();
    assert_eq!(stage["data"]["status"], "staged");
    assert_eq!(stage["revision_id"], next_revision);
    assert!(!stage["data"]["change_id"].as_str().unwrap().is_empty());
    assert_eq!(api.session_metadata(&session_id).unwrap()["durability"], "memory");

    let partial: Value = serde_json::from_str(
        &api.execute_operation(
            &session_id,
            "write",
            &json!({
                "expected_revision": next_revision,
                "mode": "apply",
                "atomic": false,
                "ops": [
                    {"kind":"set_cells","sheet_name":"Sheet1","cells":{"A2":{"kind":"value","value":"partial"}}},
                    {"kind":"set_cells","sheet_name":"Missing","cells":{"A1":{"kind":"value","value":"fail"}}}
                ]
            }).to_string(),
        )
        .await
        .expect("structured partial"),
    )
    .expect("partial JSON");
    assert_eq!(partial["data"]["status"], "partial");
    assert_eq!(partial["data"]["ops_applied"], 1);
    assert_eq!(partial["data"]["results"][1]["status"], "failed");
}

#[tokio::test(flavor = "current_thread")]
async fn recalculate_persists_f1_coverage_and_verify_binds_two_sessions() {
    let api = SessionApi::new();
    let partial = include_bytes!("../../agent-spreadsheet/tests/fixtures/f1/partial.xlsx");
    let partial_id = api.create_session(partial).expect("partial session");
    let read: Value = serde_json::from_str(
        &api.execute_operation(&partial_id, "list_sheets", "{}")
            .await
            .expect("read"),
    )
    .expect("read JSON");
    let revision = read["revision_id"].as_str().expect("revision");
    let recalculated: Value = serde_json::from_str(
        &api.execute_operation(
            &partial_id,
            "recalculate",
            &json!({"expected_revision": revision, "backend":"formualizer"}).to_string(),
        )
        .await
        .expect("recalculate"),
    )
    .expect("recalculate JSON");
    assert_eq!(recalculated["data"]["state"], "errors_found");
    assert_eq!(
        recalculated["data"]["evaluation_coverage"]["formula_cells"],
        3
    );
    assert_eq!(
        recalculated["data"]["evaluation_coverage"]["evaluated_formula_cells"],
        3
    );
    assert_eq!(
        recalculated["data"]["evaluation_coverage"]["error_formula_cells"],
        1
    );
    assert_eq!(
        recalculated["data"]["evaluation_coverage"]["source"],
        "formualizer"
    );
    assert_ne!(recalculated["revision_id"], revision);
    let recalculated_revision = recalculated["revision_id"].as_str().unwrap();
    for (operation, params) in [
        (
            "read_cells",
            json!({"sheet_name":"Sheet1","selection":{"kind":"range","ranges":["A1:C1"]}}),
        ),
        (
            "inspect_cells",
            json!({"sheet_name":"Sheet1","targets":["A1"]}),
        ),
        (
            "read_table",
            json!({"sheet_name":"Sheet1","range":"A1:C3","limit":2}),
        ),
    ] {
        let read: Value = serde_json::from_str(
            &api.execute_operation(&partial_id, operation, &params.to_string())
                .await
                .expect("post-recalc read"),
        )
        .unwrap();
        assert_eq!(
            read["data"]["calculation"]["revision_id"],
            recalculated_revision
        );
        assert_eq!(read["data"]["calculation"]["state"], "errors_found");
    }
    api.execute_operation(
        &partial_id,
        "write",
        &json!({
            "expected_revision": recalculated_revision,
            "mode":"apply",
            "ops":[{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":99}}}]
        }).to_string(),
    )
    .await
    .expect("write invalidates coverage");
    let after_write: Value = serde_json::from_str(
        &api.execute_operation(
            &partial_id,
            "read_cells",
            &json!({"sheet_name":"Sheet1","selection":{"kind":"range","ranges":["A1"]}})
                .to_string(),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_ne!(
        after_write["data"]["calculation"]["revision_id"],
        recalculated_revision
    );

    let base_bytes = include_bytes!("../../agent-spreadsheet/tests/fixtures/f1/baseline.xlsx");
    let baseline_id = api.create_session(base_bytes).expect("baseline session");
    let current_id = api.create_session(base_bytes).expect("current session");
    let current_read: Value = serde_json::from_str(
        &api.execute_operation(&current_id, "list_sheets", "{}")
            .await
            .expect("read current"),
    )
    .expect("current JSON");
    let current_revision = current_read["revision_id"]
        .as_str()
        .expect("current revision");
    api.execute_operation(
        &current_id,
        "write",
        &json!({
            "expected_revision": current_revision,
            "mode": "apply",
            "ops": [{"kind":"set_cells","sheet_name":"Sheet1","cells":{"B2":{"kind":"formula","formula":"=1/0"}}}]
        }).to_string(),
    )
    .await
    .expect("write error formula");

    let verified: Value = serde_json::from_str(
        &api.execute_operation(
            &current_id,
            "verify_workbook",
            &json!({
                "baseline_resource_id": baseline_id,
                "errors_only": true
            })
            .to_string(),
        )
        .await
        .expect("verify"),
    )
    .expect("verify JSON");
    assert_eq!(verified["data"]["baseline_resource_id"], baseline_id);
    assert_eq!(verified["data"]["current_resource_id"], current_id);
    assert_eq!(verified["data"]["proof_status"], "differences_found");
    assert!(
        !verified["data"]["new_errors"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        verified["data"]["current_evaluation_coverage"]["source"],
        "formualizer"
    );
}
