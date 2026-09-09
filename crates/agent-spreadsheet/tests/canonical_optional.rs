mod support;

use agent_spreadsheet::canonical_optional::InspectVbaData;
#[cfg(feature = "recalc-formualizer")]
use agent_spreadsheet::canonical_optional::SheetportManifestData;
use agent_spreadsheet::model::WorkbookId;
use agent_spreadsheet::operations::{
    CanonicalErrorCode, ResourceId, RuntimeCapabilities, execute_operation_json,
    operations_discovery,
};
#[cfg(feature = "recalc-formualizer")]
use agent_spreadsheet::operations::{
    canonical_error_schema, decode_operation, operation_descriptor,
};
use agent_spreadsheet::tools::{self, ListWorkbooksParams};
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;

fn operation_names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("discovery array")
        .iter()
        .filter_map(|entry| entry["name"].as_str().map(str::to_string))
        .collect()
}

fn optional_execution_golden(case: &str) -> Value {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/canonical/optional_execution_data.json");
    let goldens: Value = serde_json::from_str(&std::fs::read_to_string(fixture).unwrap()).unwrap();
    goldens[case].clone()
}

#[test]
fn optional_capabilities_are_not_advertised_when_unbacked() {
    let workspace = support::TestWorkspace::new();
    let actual = RuntimeCapabilities::from_state(&workspace.app_state());
    // The native raster backend needs no host backing, so `render` alone backs
    // the capability. Without it, the LibreOffice probe still governs.
    assert_eq!(actual.screenshot_rendering, cfg!(feature = "render"));
    assert!(!actual.workbook_write);

    let none = RuntimeCapabilities::default();
    let names = operation_names(&operations_discovery(&none));
    for optional in [
        "screenshot_sheet",
        "sheetport_manifest",
        "execute_sheetport",
        "inspect_vba",
    ] {
        assert!(!names.iter().any(|name| name == optional));
    }

    let enabled = RuntimeCapabilities {
        workbook_read: true,
        screenshot_rendering: true,
        sheetport: true,
        vba: true,
        ..RuntimeCapabilities::default()
    };
    let names = operation_names(&operations_discovery(&enabled));
    assert!(names.iter().any(|name| name == "inspect_vba"));
    assert_eq!(
        names.iter().any(|name| name == "sheetport_manifest"),
        cfg!(feature = "recalc-formualizer")
    );
    assert_eq!(
        names.iter().any(|name| name == "execute_sheetport"),
        cfg!(feature = "recalc-formualizer")
    );
    assert_eq!(
        names.iter().any(|name| name == "screenshot_sheet"),
        cfg!(feature = "recalc")
    );
}

#[cfg(feature = "recalc-formualizer")]
#[test]
fn optional_schemas_mirror_runtime_bounds() {
    let sheetport = (operation_descriptor("execute_sheetport")
        .unwrap()
        .input_schema)();
    assert_eq!(
        sheetport["properties"]["manifest_yaml"]["maxLength"],
        1_048_576
    );
    assert_eq!(sheetport["properties"]["inputs"]["maxProperties"], 256);
    let sheetport_schema = sheetport.to_string();
    assert!(sheetport_schema.contains("\"maxLength\":65536"));
    assert!(sheetport_schema.contains("\"maxItems\":10000"));
    assert!(sheetport_schema.contains("\"maxItems\":100000"));
    assert!(sheetport_schema.contains("\"maxProperties\":1000"));
    let sheetport_bounds = json!({
        "max_ports": 256,
        "max_total_cells": 100_000,
        "max_rows_per_value": 10_000,
        "max_fields_per_row_or_record": 1_000,
        "max_text_bytes": 65_536
    });
    assert_eq!(
        sheetport["properties"]["inputs"]["x-runtime-bounds"],
        sheetport_bounds
    );
    let sheetport_output = (operation_descriptor("execute_sheetport")
        .unwrap()
        .output_schema)();
    assert_eq!(
        sheetport_output["$defs"]["ExecuteSheetportData"]["properties"]["results"]["x-runtime-bounds"],
        sheetport_bounds
    );

    let screenshot = (operation_descriptor("screenshot_sheet")
        .unwrap()
        .input_schema)();
    assert_eq!(screenshot["properties"]["sheet_name"]["minLength"], 1);
    assert_eq!(screenshot["properties"]["sheet_name"]["maxLength"], 31);
    assert_eq!(screenshot["properties"]["range"]["maxLength"], 32);
    assert!(screenshot["properties"]["range"]["pattern"].is_string());
    assert_eq!(
        screenshot["properties"]["range"]["x-runtime-bounds"],
        json!({"max_rows": 100, "max_columns": 30})
    );
    let oversized_geometry = json!({
        "resource_id": "wb:abc",
        "sheet_name": "Sheet1",
        "range": "A1:AE101"
    });
    jsonschema::validator_for(&screenshot)
        .unwrap()
        .validate(&oversized_geometry)
        .expect("standard JSON Schema cannot express range geometry");
    let error = decode_operation("screenshot_sheet", oversized_geometry)
        .expect_err("runtime geometry validator enforces the vendor bound");
    assert_eq!(error.error.code, CanonicalErrorCode::InvalidRequest);

    let vba = (operation_descriptor("inspect_vba").unwrap().input_schema)();
    assert_eq!(vba["oneOf"][0]["properties"]["limit_modules"]["minimum"], 1);
    assert_eq!(
        vba["oneOf"][0]["properties"]["limit_modules"]["maximum"],
        100
    );
    assert_eq!(vba["oneOf"][1]["properties"]["limit_lines"]["minimum"], 1);
    assert_eq!(
        vba["oneOf"][1]["properties"]["limit_lines"]["maximum"],
        1_000
    );
    let vba_output = (operation_descriptor("inspect_vba").unwrap().output_schema)();
    let output_variants = &vba_output["$defs"]["InspectVbaData"]["oneOf"];
    assert_eq!(output_variants[0]["properties"]["modules"]["maxItems"], 100);
    assert_eq!(
        output_variants[1]["properties"]["source"]["maxLength"],
        262_144
    );
}

#[cfg(feature = "recalc-formualizer")]
#[test]
fn optional_response_goldens_are_full_and_schema_valid() {
    let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/canonical");
    let successes: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_root.join("optional_success_responses.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(successes.as_array().unwrap().len(), 9);
    for case in successes.as_array().unwrap() {
        let response = &case["response"];
        let operation = response["operation"].as_str().unwrap();
        let schema = (operation_descriptor(operation).unwrap().output_schema)();
        jsonschema::validator_for(&schema)
            .unwrap()
            .validate(response)
            .unwrap_or_else(|error| panic!("{}: {error}", case["case"]));
    }

    let errors: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_root.join("optional_error_responses.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(errors.as_array().unwrap().len(), 9);
    let schema = canonical_error_schema();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for case in errors.as_array().unwrap() {
        validator
            .validate(&case["response"])
            .unwrap_or_else(|error| panic!("{}: {error}", case["case"]));
    }
}

#[cfg(feature = "recalc-formualizer")]
#[test]
fn optional_action_discriminants_are_closed_and_schemas_compile() {
    let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/canonical");
    let actions: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_root.join("optional_actions.json")).unwrap(),
    )
    .unwrap();
    for action in actions.as_array().unwrap() {
        decode_operation(
            action["operation"].as_str().unwrap(),
            action["payload"].clone(),
        )
        .expect("golden optional action");
    }
    let errors: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_root.join("optional_errors.json")).unwrap(),
    )
    .unwrap();
    for error_case in errors.as_array().unwrap() {
        let error = decode_operation(
            error_case["operation"].as_str().unwrap(),
            error_case["payload"].clone(),
        )
        .expect_err("golden optional decode error");
        assert_eq!(
            serde_json::to_value(error.error.code).unwrap(),
            error_case["code"]
        );
    }

    for operation in [
        "screenshot_sheet",
        "sheetport_manifest",
        "execute_sheetport",
        "inspect_vba",
    ] {
        let descriptor = operation_descriptor(operation).expect("optional descriptor");
        jsonschema::validator_for(&(descriptor.input_schema)()).expect("valid input schema");
        jsonschema::validator_for(&(descriptor.output_schema)()).expect("valid output schema");
    }

    for action in [
        "candidates",
        "schema",
        "validate",
        "normalize",
        "bind_check",
    ] {
        let payload = match action {
            "candidates" => json!({"action":action,"resource_id":"wb:abc"}),
            "bind_check" => json!({
                "action":action,
                "resource_id":"wb:abc",
                "manifest_yaml":"spec: fio"
            }),
            "validate" | "normalize" => {
                json!({"action":action,"manifest_yaml":"spec: fio"})
            }
            _ => json!({"action":action}),
        };
        decode_operation("sheetport_manifest", payload).expect(action);
    }
    let error = decode_operation("sheetport_manifest", json!({"action":"load_path"}))
        .expect_err("unknown closed action");
    assert_eq!(error.error.code, CanonicalErrorCode::InvalidRequest);
    let error = decode_operation("inspect_vba", json!({"view":"file","resource_id":"wb:abc"}))
        .expect_err("unknown closed view");
    assert_eq!(error.error.code, CanonicalErrorCode::InvalidRequest);
}

#[cfg(feature = "recalc-formualizer")]
#[test]
fn cli_binding_rules_are_action_specific_and_deterministic() {
    let portable = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args([
            "op",
            "sheetport_manifest",
            "--json",
            r#"{"action":"schema"}"#,
        ])
        .output()
        .unwrap();
    assert!(
        portable.status.success(),
        "{}",
        String::from_utf8_lossy(&portable.stderr)
    );
    let response: Value = serde_json::from_slice(&portable.stdout).unwrap();
    assert!(response.get("resource_id").is_none());

    for (operation, payload) in [
        ("list_sheets", r#"{}"#),
        ("sheetport_manifest", r#"{"action":"candidates"}"#),
        (
            "sheetport_manifest",
            r#"{"action":"bind_check","manifest_yaml":"spec: fio"}"#,
        ),
    ] {
        let output = assert_cmd::cargo::cargo_bin_cmd!("asp")
            .args(["op", operation, "--json", payload])
            .output()
            .unwrap();
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["code"], "INVALID_REQUEST");
        assert_eq!(error["error"]["path"], "--bind");
    }

    let output = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args([
            "op",
            "sheetport_manifest",
            "--bind",
            "/definitely/missing.xlsx",
            "--json",
            r#"{"action":"validate","manifest_yaml":"spec: fio"}"#,
        ])
        .output()
        .unwrap();
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    assert_eq!(error["error"]["path"], "--bind");
}

#[cfg(feature = "recalc-formualizer")]
#[tokio::test]
async fn manifest_schema_validate_and_normalize_are_portable_content_actions() {
    let workspace = support::TestWorkspace::new();
    let state = workspace.app_state();

    let schema = execute_operation_json(
        state.clone(),
        "sheetport_manifest",
        json!({"action":"schema"}),
    )
    .await
    .expect("schema action");
    assert!(schema.resource_id.is_none());
    let schema_envelope = serde_json::to_value(&schema).unwrap();
    let output_schema = (operation_descriptor("sheetport_manifest")
        .unwrap()
        .output_schema)();
    jsonschema::validator_for(&output_schema)
        .unwrap()
        .validate(&schema_envelope)
        .expect("schema action output matches registry schema");
    assert!(matches!(
        serde_json::from_value::<SheetportManifestData>(schema.data).unwrap(),
        SheetportManifestData::Schema { .. }
    ));

    let invalid = execute_operation_json(
        state.clone(),
        "sheetport_manifest",
        json!({"action":"validate","manifest_yaml":"spec: fio"}),
    )
    .await
    .expect("validation result");
    match serde_json::from_value::<Value>(invalid.data).unwrap() {
        Value::Object(value) => {
            assert_eq!(value["action"], "validate");
            assert_eq!(value["valid"], false);
            assert!(!value["issues"].as_array().unwrap().is_empty());
        }
        _ => panic!("validation object"),
    }

    let oversized = "x".repeat(1_048_577);
    let error = execute_operation_json(
        state,
        "sheetport_manifest",
        json!({"action":"normalize","manifest_yaml":oversized}),
    )
    .await
    .expect_err("bounded manifest");
    assert_eq!(error.error.code, CanonicalErrorCode::InvalidRequest);
    assert!(!error.error.message.contains('/'));
}

#[cfg(feature = "recalc-formualizer")]
#[tokio::test]
async fn execute_sheetport_missing_required_inputs_is_structured_and_incomplete() {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("required.xlsx", |_| {});
    let state = workspace.app_state();
    let list = tools::list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: Some(false),
        },
    )
    .await
    .unwrap();
    let resource_id = ResourceId::bind_workbook(&list.workbooks[0].workbook_id).unwrap();
    let manifest = r#"spec: fio
spec_version: "0.3.0"
manifest:
  id: required-input
  name: Required Input
  workbook:
    uri: file://required.xlsx
ports:
  - id: rate
    dir: in
    shape: scalar
    location: { a1: Sheet1!A1 }
    schema: { type: number }
"#;
    let execution = execute_operation_json(
        state,
        "execute_sheetport",
        json!({
            "resource_id": resource_id.as_str(),
            "manifest_yaml": manifest,
            "inputs": {}
        }),
    )
    .await
    .expect("structured missing input result");
    assert_eq!(
        execution.data,
        optional_execution_golden("execute_sheetport.missing_required")
    );
    assert_eq!(execution.data["status"], "failed");
    assert_eq!(execution.data["coverage"]["state"], "partial");
    assert_eq!(execution.data["coverage"]["declared_input_ports"], 1);
    assert_eq!(execution.data["coverage"]["supplied_input_ports"], 0);
    assert_eq!(
        execution.data["errors"][0]["code"],
        "MISSING_REQUIRED_INPUT"
    );
    assert_eq!(execution.data["errors"][0]["port_id"], "rate");
    assert_eq!(
        execution.data["errors"][0]["constraint"]["kind"],
        "required"
    );
}

#[cfg(feature = "recalc-formualizer")]
#[tokio::test]
async fn execute_sheetport_constraint_failures_are_structured() {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("constraint.xlsx", |_| {});
    let state = workspace.app_state();
    let list = tools::list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: Some(false),
        },
    )
    .await
    .unwrap();
    let resource_id = ResourceId::bind_workbook(&list.workbooks[0].workbook_id).unwrap();
    let manifest = r#"spec: fio
spec_version: "0.3.0"
manifest:
  id: constrained-input
  name: Constrained Input
  workbook:
    uri: file://constraint.xlsx
ports:
  - id: rate
    dir: in
    shape: scalar
    location: { a1: Sheet1!A1 }
    schema: { type: number }
    constraints: { min: 0 }
"#;
    let execution = execute_operation_json(
        state,
        "execute_sheetport",
        json!({
            "resource_id": resource_id.as_str(),
            "manifest_yaml": manifest,
            "inputs": {"rate": {"kind": "number", "value": -1.0}}
        }),
    )
    .await
    .expect("structured constraint result");
    assert_eq!(
        execution.data,
        optional_execution_golden("execute_sheetport.constraint_failure")
    );
    assert_eq!(execution.data["status"], "failed");
    assert_eq!(execution.data["coverage"]["state"], "partial");
    assert_eq!(
        execution.data["errors"][0]["code"],
        "PORT_CONSTRAINT_VIOLATION"
    );
    assert_eq!(
        execution.data["errors"][0]["constraint"]["kind"],
        "manifest_constraint"
    );
}

#[cfg(feature = "recalc-formualizer")]
#[tokio::test]
async fn execute_sheetport_returns_typed_results_coverage_and_errors() {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("empty.xlsx", |_| {});
    let state = workspace.app_state();
    let list = tools::list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: Some(false),
        },
    )
    .await
    .unwrap();
    let resource_id = ResourceId::bind_workbook(&list.workbooks[0].workbook_id).unwrap();
    let candidates = execute_operation_json(
        state.clone(),
        "sheetport_manifest",
        json!({"action":"candidates","resource_id":resource_id.as_str()}),
    )
    .await
    .expect("candidate manifest");
    let manifest_yaml = candidates.data["manifest_yaml"].as_str().unwrap();
    let execution = execute_operation_json(
        state,
        "execute_sheetport",
        json!({
            "resource_id":resource_id.as_str(),
            "manifest_yaml":manifest_yaml,
            "inputs":{}
        }),
    )
    .await
    .expect("empty SheetPort execution");
    assert_eq!(
        execution.data,
        optional_execution_golden("execute_sheetport.empty")
    );
    assert_eq!(execution.data["status"], "completed");
    assert_eq!(execution.data["coverage"]["state"], "complete");
    assert_eq!(execution.data["coverage"]["declared_input_ports"], 0);
    assert_eq!(execution.data["coverage"]["declared_output_ports"], 0);
    assert_eq!(execution.data["results"], json!({}));
    assert_eq!(execution.data["errors"], json!([]));
}

async fn vba_state() -> (
    support::TestWorkspace,
    std::sync::Arc<agent_spreadsheet::state::AppState>,
    WorkbookId,
    ResourceId,
    PathBuf,
) {
    let workspace = support::TestWorkspace::new();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../agent-spreadsheet-mcp/tests/test_files/vba_minimal.xlsm");
    let path = workspace.copy_workbook(&fixture, "macro.xlsm");
    let state = workspace.app_state();
    let list = tools::list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: Some(false),
        },
    )
    .await
    .expect("list macro workbook");
    let workbook_id = list.workbooks[0].workbook_id.clone();
    let resource_id = ResourceId::bind_workbook(&workbook_id).unwrap();
    (workspace, state, workbook_id, resource_id, path)
}

#[tokio::test]
async fn vba_module_source_cursor_is_bounded_fingerprinted_and_revision_bound() {
    let (workspace, state, workbook_id, resource_id, path) = vba_state().await;
    let summary = execute_operation_json(
        state.clone(),
        "inspect_vba",
        json!({
            "view":"project_summary",
            "resource_id":resource_id.as_str(),
            "limit_modules":1,
            "include_references":false
        }),
    )
    .await
    .expect("project summary");
    let mut summary_golden = summary.data.clone();
    summary_golden["next_cursor"] = json!("<cursor>");
    assert_eq!(
        summary_golden,
        optional_execution_golden("inspect_vba.project_summary_page")
    );
    let module_name = match serde_json::from_value::<InspectVbaData>(summary.data).unwrap() {
        InspectVbaData::ProjectSummary { modules, .. } => modules[0].name.clone(),
        _ => panic!("summary branch"),
    };

    let first = execute_operation_json(
        state.clone(),
        "inspect_vba",
        json!({
            "view":"module_source",
            "resource_id":resource_id.as_str(),
            "module_name":module_name,
            "limit_lines":1
        }),
    )
    .await
    .expect("first source page");
    let mut source_golden = first.data.clone();
    source_golden["next_cursor"] = json!("<cursor>");
    assert_eq!(
        source_golden,
        optional_execution_golden("inspect_vba.module_source_page")
    );
    let cursor = match serde_json::from_value::<InspectVbaData>(first.data).unwrap() {
        InspectVbaData::ModuleSource {
            returned_lines,
            next_cursor,
            source,
            ..
        } => {
            assert_eq!(returned_lines, 1);
            assert_eq!(source.lines().count(), 1);
            next_cursor.expect("more source")
        }
        _ => panic!("source branch"),
    };
    assert!(!cursor.contains(&module_name));

    let mismatch = execute_operation_json(
        state.clone(),
        "inspect_vba",
        json!({
            "view":"module_source",
            "resource_id":resource_id.as_str(),
            "module_name":"DifferentModule",
            "limit_lines":1,
            "cursor":cursor
        }),
    )
    .await
    .expect_err("request fingerprint mismatch");
    assert_eq!(mismatch.error.code, CanonicalErrorCode::CursorMismatch);

    state.close_workbook(&workbook_id).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"\0")
        .unwrap();
    let stale_state = workspace.app_state();
    let stale = execute_operation_json(
        stale_state,
        "inspect_vba",
        json!({
            "view":"module_source",
            "resource_id":resource_id.as_str(),
            "module_name":module_name,
            "limit_lines":1,
            "cursor":cursor
        }),
    )
    .await
    .expect_err("revision-bound cursor");
    assert_eq!(stale.error.code, CanonicalErrorCode::StaleCursor);
}

#[tokio::test]
async fn vba_cursor_is_bound_to_resource_even_when_workbook_bytes_match() {
    let workspace = support::TestWorkspace::new();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../agent-spreadsheet-mcp/tests/test_files/vba_minimal.xlsm");
    workspace.copy_workbook(&fixture, "macro-a.xlsm");
    workspace.copy_workbook(&fixture, "macro-b.xlsm");
    let state = workspace.app_state();
    let list = tools::list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: Some(false),
        },
    )
    .await
    .unwrap();
    let first_resource = ResourceId::bind_workbook(&list.workbooks[0].workbook_id).unwrap();
    let second_resource = ResourceId::bind_workbook(&list.workbooks[1].workbook_id).unwrap();
    let first = execute_operation_json(
        state.clone(),
        "inspect_vba",
        json!({
            "view": "project_summary",
            "resource_id": first_resource.as_str(),
            "limit_modules": 1
        }),
    )
    .await
    .unwrap();
    let cursor = first.data["next_cursor"]
        .as_str()
        .expect("second module page");
    let mismatch = execute_operation_json(
        state,
        "inspect_vba",
        json!({
            "view": "project_summary",
            "resource_id": second_resource.as_str(),
            "limit_modules": 1,
            "cursor": cursor
        }),
    )
    .await
    .expect_err("cross-resource cursor rejected");
    assert_eq!(mismatch.error.code, CanonicalErrorCode::CursorMismatch);
}

#[cfg(feature = "recalc")]
#[tokio::test]
async fn screenshot_validation_is_invalid_request_path_free_and_does_not_precreate() {
    use agent_spreadsheet::canonical_optional::{
        ScreenshotBackend, ScreenshotSheetRequest, screenshot_sheet,
    };

    let workspace = support::TestWorkspace::new();
    let state = workspace.app_state();
    let invalid = screenshot_sheet(
        state.clone(),
        ScreenshotSheetRequest {
            resource_id: serde_json::from_value(json!("wb:abc")).unwrap(),
            sheet_name: "Sheet1".to_string(),
            range: Some("../secret".to_string()),
            backend: None,
            png_level: None,
        },
    )
    .await
    .expect_err("invalid range");
    assert!(invalid.to_string().starts_with("invalid request:"));
    assert!(!invalid.to_string().contains("../secret"));
    assert!(!workspace.path("screenshots").exists());

    workspace.create_workbook("plain.xlsx", |_| {});
    let list = tools::list_workbooks(
        state.clone(),
        ListWorkbooksParams {
            slug_prefix: None,
            folder: None,
            path_glob: None,
            limit: None,
            offset: None,
            include_paths: Some(false),
        },
    )
    .await
    .unwrap();
    let resource_id = ResourceId::bind_workbook(&list.workbooks[0].workbook_id).unwrap();
    let invalid = execute_operation_json(
        state.clone(),
        "screenshot_sheet",
        json!({
            "resource_id": resource_id.as_str(),
            "sheet_name": "Sheet1",
            "range": "A0:XFE9999999"
        }),
    )
    .await
    .expect_err("canonical invalid range");
    assert_eq!(invalid.error.code, CanonicalErrorCode::InvalidRequest);
    assert!(!invalid.error.message.contains('/'));

    let dispatched = execute_operation_json(
        state.clone(),
        "screenshot_sheet",
        json!({
            "resource_id": resource_id.as_str(),
            "sheet_name": "Sheet1",
            "range": "A1:B2"
        }),
    )
    .await;
    if cfg!(feature = "render") {
        // The native backend needs no host backing, so dispatch renders.
        let response = dispatched.expect("native backend renders without host backing");
        assert_eq!(response.data["renderer"], "native-raster/1");
        assert_eq!(response.data["artifact"]["media_type"], "image/png");
        assert!(response.data["calculation"]["revision_id"].is_string());
    } else {
        let unavailable = dispatched.expect_err("dispatch uses the bound state's capabilities");
        assert_eq!(
            unavailable.error.code,
            CanonicalErrorCode::CapabilityUnavailable
        );
    }
    // Either way the LibreOffice output directory is never pre-created.
    assert!(!workspace.path("screenshots").exists());

    // Explicitly asking for LibreOffice on a runtime with no LibreOffice
    // backing still fails, and still says nothing about paths.
    let error = screenshot_sheet(
        state,
        ScreenshotSheetRequest {
            resource_id,
            sheet_name: "Sheet1".to_string(),
            range: None,
            backend: Some(ScreenshotBackend::Libreoffice),
            png_level: None,
        },
    )
    .await
    .expect_err("runtime has no LibreOffice screenshot backing");
    assert_eq!(error.to_string(), "screenshot rendering failed");
    assert!(!workspace.path("screenshots").exists());
}

#[cfg(all(unix, feature = "recalc"))]
#[tokio::test]
async fn screenshot_rejects_symlinked_workspace_output_before_rendering() {
    use agent_spreadsheet::canonical_optional::{
        ScreenshotBackend, ScreenshotSheetRequest, screenshot_sheet,
    };
    use std::os::unix::fs::symlink;

    let workspace = support::TestWorkspace::new();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), workspace.path("screenshots")).unwrap();
    let state = workspace.app_state();
    // The output-directory guard belongs to the LibreOffice path; the native
    // renderer never writes there at all.
    let error = screenshot_sheet(
        state,
        ScreenshotSheetRequest {
            resource_id: serde_json::from_value(json!("wb:abc")).unwrap(),
            sheet_name: "Sheet1".to_string(),
            range: None,
            backend: Some(ScreenshotBackend::Libreoffice),
            png_level: None,
        },
    )
    .await
    .expect_err("symlink rejected");
    assert!(error.to_string().contains("real directory"));
}

#[cfg(feature = "recalc-libreoffice")]
#[test]
fn canonical_cli_bound_screenshot_succeeds_without_precreated_output_directory() {
    if !std::path::Path::new("/usr/bin/soffice").exists()
        && !std::path::Path::new("/bin/soffice").exists()
    {
        return;
    }
    let profile = tempfile::tempdir().unwrap();
    let standard = profile.path().join("user/basic/Standard");
    std::fs::create_dir_all(&standard).unwrap();
    std::fs::write(
        standard.join("Module1.xba"),
        include_bytes!("../../../docker/libreoffice/Module1.xba"),
    )
    .unwrap();
    std::fs::write(
        standard.join("script.xlb"),
        include_bytes!("../../../docker/libreoffice/script.xlb"),
    )
    .unwrap();
    std::fs::write(
        profile.path().join("user/registrymodifications.xcu"),
        include_bytes!("../../../docker/libreoffice/registrymodifications.xcu"),
    )
    .unwrap();
    std::fs::write(
        profile.path().join("user/basic/script.xlc"),
        br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE library:libraries PUBLIC "-//OpenOffice.org//DTD OfficeDocument 1.0//EN" "libraries.dtd">
<library:libraries xmlns:library="http://openoffice.org/2000/library">
 <library:library library:name="Standard" library:link="false"/>
</library:libraries>"#,
    )
    .unwrap();

    let discovery = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .env(
            "SPREADSHEET_MCP_LIBREOFFICE_USER_INSTALLATION",
            profile.path(),
        )
        .arg("operations")
        .output()
        .unwrap();
    assert!(discovery.status.success());
    let advertised: Value = serde_json::from_slice(&discovery.stdout).unwrap();
    assert!(advertised.as_array().unwrap().iter().any(|operation| {
        operation["name"] == "screenshot_sheet" && operation["available"] == true
    }));

    let workspace = support::TestWorkspace::new();
    let workbook = workspace.create_workbook("bound-screenshot.xlsx", |book| {
        book.sheet_by_name_mut("Sheet1").ok()
            .unwrap()
            .cell_mut("A1")
            .set_value("canonical screenshot");
    });
    assert!(!workspace.path("screenshots").exists());

    let output = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .env(
            "SPREADSHEET_MCP_LIBREOFFICE_USER_INSTALLATION",
            profile.path(),
        )
        .args([
            "op",
            "screenshot_sheet",
            "--bind",
            workbook.to_str().unwrap(),
            "--json",
            r#"{"sheet_name":"Sheet1","range":"A1:B2"}"#,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["operation"], "screenshot_sheet");
    assert_eq!(response["data"]["range"], "A1:B2");
    assert!(
        response["data"]["artifact"]["handle"]
            .as_str()
            .unwrap()
            .starts_with("artifact:sha256:")
    );
    assert!(workspace.path("screenshots").is_dir());
    assert!(workspace.path("artifacts").is_dir());
}
