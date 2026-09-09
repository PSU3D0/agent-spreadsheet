use agent_spreadsheet::model::WorkbookId;
use agent_spreadsheet::operations::{
    AdapterPersistence, CanonicalErrorCode, CanonicalErrorEnvelope, CanonicalResponse,
    OperationAdapter, OperationRisk, ResourceId, RuntimeCapabilities, canonical_error_schema,
    decode_operation, execute_operation_json, operation_registry,
};
use agent_spreadsheet::runtime::stateless::StatelessRuntime;
use agent_spreadsheet::tools::{SheetOverviewParams, sheet_overview};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/f1/baseline.xlsx")
}

async fn bound_state() -> (
    std::sync::Arc<agent_spreadsheet::state::AppState>,
    WorkbookId,
    ResourceId,
) {
    let (state, workbook_id) = StatelessRuntime
        .open_state_for_file(&fixture())
        .await
        .expect("bind fixture");
    let resource_id = ResourceId::bind_workbook(&workbook_id).expect("canonical binding");
    (state, workbook_id, resource_id)
}

#[tokio::test]
async fn borrowed_resident_read_cells_uses_the_file_canonical_projection() {
    use agent_spreadsheet::canonical_reads::{
        ReadCellsRequest, execute_read_cells, read_cells_from_view,
    };
    use agent_spreadsheet::canonical_write::ResidentWriteSession;
    let (state, workbook_id, resource_id) = bound_state().await;
    let workbook = state.open_workbook(&workbook_id).await.unwrap();
    let resident =
        ResidentWriteSession::from_bytes("session:borrowed", &std::fs::read(fixture()).unwrap())
            .unwrap();
    let view = resident.read_view().unwrap();
    assert!(view.describe().bytes.is_none());
    assert!(workbook.describe().bytes.is_some());
    for selection in [
        json!({"kind":"range","ranges":["A1:D10"]}),
        json!({"kind":"rows","start_row":1,"row_count":10}),
    ] {
        let request: ReadCellsRequest = serde_json::from_value(json!({"resource_id":resource_id,"sheet_name":workbook.sheet_names()[0],"selection":selection,"page_size":3})).unwrap();
        let expected = execute_read_cells(state.clone(), &request, &workbook.revision_id)
            .await
            .unwrap();
        let actual = read_cells_from_view(
            &view,
            &request,
            &workbook.revision_id,
            workbook.calculation_metadata(),
            state.config().max_cells(),
            state.config().max_payload_bytes(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
    assert_eq!(resident.diagnostic_workbook().serialization_count(), 0);
    assert_eq!(
        resident.diagnostic_workbook().evaluator_counters().ingests,
        1
    );
}

fn with_resource(resource_id: &ResourceId, mut payload: Value) -> Value {
    payload
        .as_object_mut()
        .expect("object payload")
        .insert("resource_id".to_string(), json!(resource_id.as_str()));
    payload
}

fn assert_object_schemas_closed(schema: &Value) {
    match schema {
        Value::Object(object) => {
            if object.contains_key("properties") {
                assert!(
                    object.get("additionalProperties") == Some(&Value::Bool(false))
                        || object.get("unevaluatedProperties") == Some(&Value::Bool(false)),
                    "open structured object schema: {object:?}"
                );
            }
            for child in object.values() {
                assert_object_schemas_closed(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                assert_object_schemas_closed(child);
            }
        }
        _ => {}
    }
}

fn validate(schema: &Value, instance: &Value) {
    let validator = jsonschema::validator_for(schema).expect("valid generated JSON Schema");
    if let Err(error) = validator.validate(instance) {
        panic!("schema validation failed: {error}\ninstance: {instance}\nschema: {schema}");
    }
}

fn golden(name: &str, response: &Value) -> Value {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/canonical")
            .join(format!("{name}.json")),
    )
    .expect("read golden");
    let mut text = text;
    let resource_id = response
        .get("resource_id")
        .and_then(Value::as_str)
        .or_else(|| {
            response
                .pointer("/data/workbooks/0/resource_id")
                .and_then(Value::as_str)
        });
    if let Some(resource_id) = resource_id {
        text = text.replace("{{RESOURCE_ID}}", resource_id);
        text = text.replace(
            "{{WORKBOOK_ID}}",
            resource_id.split_once(':').expect("typed resource").1,
        );
    }
    let revision_id = response
        .get("revision_id")
        .and_then(Value::as_str)
        .or_else(|| {
            response
                .pointer("/data/workbooks/0/metadata/revision_id")
                .and_then(Value::as_str)
        });
    if let Some(revision_id) = revision_id {
        text = text.replace("{{REVISION_ID}}", revision_id);
    }
    let short_id = response
        .pointer("/data/metadata/short_id")
        .or_else(|| response.pointer("/data/workbooks/0/metadata/short_id"))
        .and_then(Value::as_str);
    if let Some(short_id) = short_id {
        text = text.replace("{{SHORT_ID}}", short_id);
    }
    let last_modified = response
        .pointer("/data/metadata/last_modified")
        .or_else(|| response.pointer("/data/workbooks/0/metadata/last_modified"))
        .and_then(Value::as_str);
    if let Some(last_modified) = last_modified {
        text = text.replace("{{LAST_MODIFIED}}", last_modified);
    }
    text = text.replace(
        "{{FIXTURE_PATH}}",
        fixture().to_str().expect("UTF-8 fixture"),
    );
    serde_json::from_str(&text).expect("valid golden")
}

fn asp_op(operation: &str, payload: Value) -> Result<Value, Value> {
    asp_op_bound(operation, payload, &fixture())
}

fn asp_op_bound(operation: &str, payload: Value, path: &Path) -> Result<Value, Value> {
    let output = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args([
            "op",
            operation,
            "--bind",
            path.to_str().expect("utf8 fixture"),
            "--json",
            &payload.to_string(),
        ])
        .output()
        .expect("run asp op");
    if output.status.success() {
        Ok(serde_json::from_slice(&output.stdout).expect("canonical stdout JSON"))
    } else {
        Err(serde_json::from_slice(&output.stderr).expect("canonical stderr JSON"))
    }
}

#[test]
fn registry_schemas_are_closed_typed_and_discriminated() {
    let registry = operation_registry();
    let names = registry
        .iter()
        .map(|descriptor| descriptor.name)
        .collect::<HashSet<_>>();
    #[allow(unused_mut)]
    let mut expected = HashSet::from([
        "list_workbooks",
        "describe_workbook",
        "list_sheets",
        "sheet_overview",
        "read_cells",
        "inspect_cells",
        "read_table",
        "read_layout",
        "export_grid",
        "named_ranges",
        "analyze_styles",
        "search_values",
        "search_formulas",
        "formula_trace",
        "formula_map",
        "profile_table",
        "sheet_statistics",
        "inspect_vba",
    ]);
    #[cfg(feature = "recalc-formualizer")]
    expected.extend(["sheetport_manifest", "execute_sheetport"]);
    #[cfg(feature = "recalc")]
    expected.extend([
        "screenshot_sheet",
        "write",
        "create_fork",
        "list_forks",
        "recalculate",
        "verify_workbook",
        "export_fork",
        "discard_fork",
        "get_changes",
        "checkpoint",
        "staged_change",
        "session_history",
    ]);
    assert_eq!(names, expected);

    let capabilities = RuntimeCapabilities::native();
    for descriptor in registry {
        assert_eq!(descriptor.schema_version, "1");
        if descriptor.name == "screenshot_sheet" {
            assert_eq!(
                descriptor.is_available(&capabilities),
                capabilities.screenshot_rendering
            );
        } else if descriptor.name == "session_history" {
            assert!(!descriptor.is_available(&capabilities));
        } else {
            assert!(descriptor.is_available(&capabilities));
        }
        assert_eq!(
            descriptor.risk_ceiling,
            match descriptor.name {
                "write" | "discard_fork" | "checkpoint" | "staged_change" | "session_history" => {
                    OperationRisk::Destructive
                }
                "recalculate" | "export_fork" => OperationRisk::High,
                "create_fork" => OperationRisk::Moderate,
                _ => OperationRisk::Low,
            }
        );
        let input = (descriptor.input_schema)();
        let output = (descriptor.output_schema)();
        jsonschema::validator_for(&input).expect("valid generated input JSON Schema");
        jsonschema::validator_for(&output).expect("valid generated output JSON Schema");
        assert_object_schemas_closed(&input);
        assert_object_schemas_closed(&output);
        if matches!(descriptor.name, "list_workbooks" | "list_forks") {
            assert!(input["$defs"].get("ResourceId").is_none());
            assert!(output["properties"].get("resource_id").is_none());
            if descriptor.name == "list_workbooks" {
                assert_eq!(descriptor.capability.name, "workbook_discovery");
            }
        } else {
            assert_eq!(
                input["$defs"]["ResourceId"]["pattern"],
                "^(wb|fork|session):[A-Za-z0-9][A-Za-z0-9_-]{0,243}$"
            );
            assert_eq!(input["$defs"]["ResourceId"]["minLength"], 4);
            assert_eq!(input["$defs"]["ResourceId"]["maxLength"], 256);
        }
        assert_eq!(output["properties"]["schema_version"]["const"], "1");
        assert_eq!(output["properties"]["operation"]["const"], descriptor.name);
    }
    let error = canonical_error_schema();
    jsonschema::validator_for(&error).expect("valid generated error JSON Schema");
    assert_object_schemas_closed(&error);
    assert_eq!(error["properties"]["schema_version"]["const"], "1");
}

#[test]
fn resource_ids_reject_untyped_path_dot_drive_and_file_forms() {
    for invalid in [
        "book.xlsx",
        "..",
        ".",
        "C:book.xlsx",
        "file:book.xlsx",
        "wb-good",
        "shortid",
        "wb:/tmp/book.xlsx",
        r"wb:C:\\book.xlsx",
        "wb:book.xlsx",
        "wb:..",
        "other:abc",
        "wb:",
    ] {
        let error =
            decode_operation("list_sheets", json!({"resource_id":invalid})).expect_err(invalid);
        assert_eq!(error.error.code, CanonicalErrorCode::InvalidRequest);
    }
    for valid in ["wb:abc", "fork:fork-abc123", "session:session-123"] {
        decode_operation("list_sheets", json!({"resource_id":valid})).expect(valid);
    }

    assert_eq!(
        ResourceId::bind_workbook(&WorkbookId("abc123".to_string()))
            .unwrap()
            .as_str(),
        "wb:abc123"
    );
    assert_eq!(
        ResourceId::bind_workbook(&WorkbookId("fork-abc123".to_string()))
            .unwrap()
            .as_str(),
        "fork:fork-abc123"
    );
    assert!(ResourceId::bind_workbook(&WorkbookId("book.xlsx".to_string())).is_err());
}

#[test]
fn ergonomic_read_aliases_project_canonical_dispatcher_data() {
    let file = fixture();
    let file = file.to_str().unwrap();
    let cases: &[(&[&str], &str, Value)] = &[
        (&["read", "sheets", file], "list_sheets", json!({})),
        (
            &["read", "overview", file, "Sheet1"],
            "sheet_overview",
            json!({"sheet_name":"Sheet1"}),
        ),
        (
            &[
                "read", "table", file, "--sheet", "Sheet1", "--range", "A1:C1",
            ],
            "read_table",
            json!({"sheet_name":"Sheet1","range":"A1:C1"}),
        ),
        (&["read", "names", file], "named_ranges", json!({})),
        (
            &["analyze", "sheet-statistics", file, "Sheet1"],
            "sheet_statistics",
            json!({"sheet_name":"Sheet1"}),
        ),
    ];

    for (alias, operation, request) in cases {
        let ergonomic = assert_cmd::cargo::cargo_bin_cmd!("asp")
            .args(*alias)
            .output()
            .unwrap();
        assert!(
            ergonomic.status.success(),
            "{} failed: {}",
            alias.join(" "),
            String::from_utf8_lossy(&ergonomic.stderr)
        );
        let ergonomic: Value = serde_json::from_slice(&ergonomic.stdout).unwrap();
        let canonical = asp_op(operation, request.clone()).unwrap();
        let mut canonical_data = canonical["data"].clone();
        agent_spreadsheet::response_prune::prune_non_structural_empties(&mut canonical_data);
        assert_eq!(
            ergonomic,
            canonical_data,
            "ergonomic alias {} diverged from {operation}",
            alias.join(" ")
        );
    }
}

#[tokio::test]
async fn full_success_envelopes_match_dispatcher_cli_golden_and_schema() {
    let cases = [
        ("list_sheets", json!({"include_bounds":true})),
        ("sheet_overview", json!({"sheet_name":"Sheet1"})),
        (
            "read_table",
            json!({"sheet_name":"Sheet1","range":"A1:C1","format":"values"}),
        ),
    ];

    for (operation, payload) in cases {
        let (state, _, resource_id) = bound_state().await;
        let dispatcher = execute_operation_json(
            state,
            operation,
            with_resource(&resource_id, payload.clone()),
        )
        .await
        .expect("dispatcher success");
        let dispatcher = serde_json::to_value(dispatcher).unwrap();
        let cli = asp_op(operation, payload).expect("CLI success");
        assert_eq!(cli, dispatcher);
        assert_eq!(
            dispatcher,
            golden(&format!("{operation}.success"), &dispatcher)
        );
        let descriptor = operation_registry()
            .iter()
            .find(|descriptor| descriptor.name == operation)
            .unwrap();
        validate(&(descriptor.output_schema)(), &dispatcher);
    }
}

#[tokio::test]
async fn canonical_read_branches_match_dispatcher_cli_goldens_and_schemas() {
    let cases = [
        ("describe_workbook", "describe_workbook.default", json!({})),
        (
            "describe_workbook",
            "describe_workbook.summary",
            json!({"include":["summary"]}),
        ),
        (
            "read_cells",
            "read_cells.range",
            json!({"sheet_name":"Sheet1","selection":{"kind":"range","ranges":["A1:C1"]},"format":"dense"}),
        ),
        (
            "read_cells",
            "read_cells.rows",
            json!({"sheet_name":"Sheet1","selection":{"kind":"rows","start_row":1,"row_count":1},"format":"full"}),
        ),
        (
            "inspect_cells",
            "inspect_cells.success",
            json!({"sheet_name":"Sheet1","targets":["A1","C1"]}),
        ),
        (
            "read_layout",
            "read_layout.success",
            json!({"sheet_name":"Sheet1","range":"A1:C1"}),
        ),
        (
            "export_grid",
            "export_grid.success",
            json!({"sheet_name":"Sheet1","range":"A1:C1"}),
        ),
        ("named_ranges", "named_ranges.success", json!({})),
        (
            "analyze_styles",
            "analyze_styles.sheet",
            json!({"scope":{"kind":"sheet","sheet_name":"Sheet1","selection":{"kind":"all"}},"include":["descriptors","ranges","example_cells"]}),
        ),
        (
            "analyze_styles",
            "analyze_styles.workbook",
            json!({"scope":{"kind":"workbook"},"include":["descriptors","example_cells","theme","conditional_formats"]}),
        ),
        (
            "search_values",
            "search_values.success",
            json!({"query":"1","match_mode":"exact"}),
        ),
        (
            "search_formulas",
            "search_formulas.cells",
            json!({"query":{"text":"SUM","match_mode":"contains"},"result_mode":"cells"}),
        ),
        (
            "search_formulas",
            "search_formulas.groups",
            json!({"filter":{"volatile":true},"result_mode":"groups","group_by":"function"}),
        ),
        (
            "formula_trace",
            "formula_trace.success",
            json!({"sheet_name":"Sheet1","cell_address":"A1","direction":"precedents"}),
        ),
        (
            "formula_map",
            "formula_map.success",
            json!({"sheet_name":"Sheet1"}),
        ),
        (
            "profile_table",
            "profile_table.success",
            json!({"sheet_name":"Sheet1"}),
        ),
        (
            "sheet_statistics",
            "sheet_statistics.success",
            json!({"sheet_name":"Sheet1"}),
        ),
    ];

    for (operation, golden_name, payload) in cases {
        let (state, _, resource_id) = bound_state().await;
        let dispatcher = execute_operation_json(
            state,
            operation,
            with_resource(&resource_id, payload.clone()),
        )
        .await
        .expect("dispatcher success");
        let dispatcher = serde_json::to_value(dispatcher).unwrap();
        let cli = asp_op(operation, payload).expect("CLI success");
        assert_eq!(cli, dispatcher, "cross-surface mismatch for {golden_name}");
        assert_eq!(
            dispatcher,
            golden(golden_name, &dispatcher),
            "golden mismatch for {golden_name}"
        );
        let descriptor = operation_registry()
            .iter()
            .find(|descriptor| descriptor.name == operation)
            .unwrap();
        validate(&(descriptor.output_schema)(), &dispatcher);
    }
}

#[tokio::test]
async fn list_workbooks_has_no_request_or_response_resource_binding() {
    let (state, _, _) = bound_state().await;
    let dispatcher = execute_operation_json(state, "list_workbooks", json!({}))
        .await
        .unwrap();
    let dispatcher = serde_json::to_value(dispatcher).unwrap();
    let cli = asp_op("list_workbooks", json!({})).unwrap();
    assert_eq!(dispatcher, cli);
    assert!(dispatcher.get("resource_id").is_none());
    assert!(dispatcher.get("revision_id").is_none());
    assert_eq!(dispatcher, golden("list_workbooks.success", &dispatcher));
    let descriptor = operation_registry()
        .iter()
        .find(|descriptor| descriptor.name == "list_workbooks")
        .unwrap();
    validate(&(descriptor.output_schema)(), &dispatcher);
}

#[tokio::test]
async fn full_error_envelopes_match_dispatcher_cli_golden_and_schema() {
    let cases = [
        ("list_sheets", json!({"unexpected":true})),
        ("sheet_overview", json!({"sheet_name":"Missing"})),
        (
            "read_table",
            json!({"sheet_name":"Missing","range":"A1:C1","format":"values"}),
        ),
    ];
    let schema = canonical_error_schema();

    for (operation, payload) in cases {
        let (state, _, resource_id) = bound_state().await;
        let dispatcher = execute_operation_json(
            state,
            operation,
            with_resource(&resource_id, payload.clone()),
        )
        .await
        .expect_err("dispatcher error");
        let dispatcher = serde_json::to_value(dispatcher).unwrap();
        let cli = asp_op(operation, payload).expect_err("CLI error");
        assert_eq!(cli, dispatcher);
        assert_eq!(
            dispatcher,
            golden(&format!("{operation}.error"), &dispatcher)
        );
        validate(&schema, &dispatcher);
    }
}

#[tokio::test]
async fn read_cells_cursor_is_opaque_correlated_and_request_bound() {
    let (state, _, resource_id) = bound_state().await;
    let request = json!({
        "resource_id": resource_id.as_str(),
        "sheet_name": "Sheet1",
        "selection": {"kind":"range","ranges":["A1:A1","C1:C1"]},
        "format": "values",
        "page_size": 1
    });
    let first = execute_operation_json(state.clone(), "read_cells", request.clone())
        .await
        .unwrap();
    assert_eq!(first.data["blocks"][0]["selection_index"], 0);
    assert_eq!(first.data["page"]["complete"], false);
    let cursor = first.data["page"]["next_cursor"]
        .as_str()
        .expect("opaque cursor");
    assert!(cursor.starts_with("rc1_"));

    let mut continuation = request.clone();
    continuation
        .as_object_mut()
        .unwrap()
        .insert("cursor".to_string(), json!(cursor));
    let second = execute_operation_json(state.clone(), "read_cells", continuation)
        .await
        .unwrap();
    assert_eq!(second.data["blocks"][0]["selection_index"], 1);
    assert_eq!(second.data["page"]["complete"], true);
    assert!(second.data["page"]["next_cursor"].is_null());

    let mismatch = execute_operation_json(
        state,
        "read_cells",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "selection":{"kind":"range","ranges":["A1:B1"]},
            "format":"values", "page_size":1, "cursor":cursor
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(mismatch.error.code, CanonicalErrorCode::CursorMismatch);
}

#[tokio::test]
async fn canonical_semantic_errors_are_conservative_and_legacy_strings_are_unchanged() {
    let (state, workbook_id, resource_id) = bound_state().await;
    let canonical = execute_operation_json(
        state.clone(),
        "sheet_overview",
        with_resource(&resource_id, json!({"sheet_name":"Missing"})),
    )
    .await
    .unwrap_err();
    assert_eq!(canonical.error.code, CanonicalErrorCode::OperationFailed);
    assert_eq!(canonical.error.message, "sheet Missing not found");

    let legacy = sheet_overview(
        state,
        SheetOverviewParams {
            workbook_or_fork_id: workbook_id,
            sheet_name: "Missing".to_string(),
            max_regions: None,
            max_headers: None,
            include_headers: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(legacy.to_string(), "sheet Missing not found");
    assert!(serde_json::from_str::<CanonicalErrorEnvelope>(&legacy.to_string()).is_err());
}

#[test]
fn unknown_operation_precedes_json_and_binding_failures() {
    for payload in ["{", "{}"] {
        let output = assert_cmd::cargo::cargo_bin_cmd!("asp")
            .args([
                "op",
                "not_an_operation",
                "--bind",
                "/definitely/missing.xlsx",
                "--json",
                payload,
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error: CanonicalErrorEnvelope = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error.error.code, CanonicalErrorCode::UnknownOperation);
    }
}

#[test]
fn malformed_unknown_and_missing_resource_errors_are_canonical() {
    let malformed = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args([
            "op",
            "list_sheets",
            "--bind",
            fixture().to_str().unwrap(),
            "--json",
            "{",
        ])
        .output()
        .unwrap();
    let malformed: CanonicalErrorEnvelope = serde_json::from_slice(&malformed.stderr).unwrap();
    assert_eq!(malformed.error.code, CanonicalErrorCode::InvalidRequest);

    let missing = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args([
            "op",
            "list_sheets",
            "--bind",
            "/definitely/missing.xlsx",
            "--json",
            "{}",
        ])
        .output()
        .unwrap();
    let missing: CanonicalErrorEnvelope = serde_json::from_slice(&missing.stderr).unwrap();
    assert_eq!(missing.error.code, CanonicalErrorCode::ResourceNotFound);
}

#[test]
fn complete_registry_and_cli_discovery_are_distinct_projections() {
    let registry = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args(["registry", "--all"])
        .output()
        .unwrap();
    assert!(registry.status.success());
    let registry: Value = serde_json::from_slice(&registry.stdout).unwrap();
    let descriptors = registry["operations"].as_array().unwrap();
    assert_eq!(descriptors.len(), 32);
    assert!(descriptors.iter().all(|descriptor| {
        descriptor.get("input_schema").is_some()
            && descriptor.get("output_schema").is_some()
            && descriptor.get("available").is_none()
            && descriptor.pointer("/adapters/cli/binding_kind").is_some()
            && descriptor.pointer("/adapters/mcp/support_status").is_some()
            && descriptor.pointer("/adapters/wasm/persistence").is_some()
            && descriptor
                .pointer("/adapters/just_bash/binding_kind")
                .is_some()
    }));

    let operations = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .arg("operations")
        .output()
        .unwrap();
    assert!(operations.status.success());
    let operations: Value = serde_json::from_slice(&operations.stdout).unwrap();
    let names = operations
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|descriptor| descriptor["name"].as_str())
        .collect::<HashSet<_>>();
    assert!(names.contains("write"));
    assert!(names.contains("recalculate"));
    assert!(names.contains("verify_workbook"));
    for durable in [
        "create_fork",
        "list_forks",
        "export_fork",
        "discard_fork",
        "get_changes",
        "checkpoint",
        "staged_change",
        "session_history",
    ] {
        assert!(!names.contains(durable), "CLI advertised {durable}");
    }
}

#[test]
fn just_bash_registry_plan_is_the_portable_wasm_subset() {
    for descriptor in operation_registry() {
        let wasm = descriptor.adapter_metadata(OperationAdapter::Wasm);
        let just_bash = descriptor.adapter_metadata(OperationAdapter::JustBash);
        assert_eq!(
            just_bash.support_status, wasm.support_status,
            "{}",
            descriptor.name
        );
        assert_eq!(
            just_bash.binding_kind, wasm.binding_kind,
            "{}",
            descriptor.name
        );
        let expected = if matches!(descriptor.name, "write" | "recalculate") {
            AdapterPersistence::ExportRequired
        } else if matches!(descriptor.name, "checkpoint" | "staged_change" | "session_history") {
            AdapterPersistence::ResidentRequired
        } else if just_bash.is_supported() {
            AdapterPersistence::None
        } else {
            wasm.persistence
        };
        assert_eq!(just_bash.persistence, expected, "{}", descriptor.name);
    }
}

#[test]
fn machine_mode_accepts_stdin_json_and_discovery_commands_work() {
    assert_cmd::cargo::cargo_bin_cmd!("asp")
        .arg("operations")
        .assert()
        .success();
    assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args(["schema", "read_table"])
        .assert()
        .success();
    assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args(["op", "list_sheets", "--bind", fixture().to_str().unwrap()])
        .write_stdin("{}")
        .assert()
        .success();
}

#[test]
fn canonical_schema_and_examples_are_registry_generated_and_collision_free() {
    for descriptor in operation_registry() {
        let schema_output = assert_cmd::cargo::cargo_bin_cmd!("asp")
            .args(["schema", descriptor.name])
            .output()
            .expect("schema command");
        assert!(schema_output.status.success(), "schema {}", descriptor.name);
        let schema: Value = serde_json::from_slice(&schema_output.stdout).unwrap();
        assert_eq!(schema["operation"], descriptor.name);
        let mut expected_input = (descriptor.input_schema)();
        let mut injected = vec!["resource_id"];
        if descriptor.name == "verify_workbook" {
            injected.push("baseline_resource_id");
        }
        if let Some(required) = expected_input
            .get_mut("required")
            .and_then(Value::as_array_mut)
        {
            required.retain(|field| !injected.iter().any(|name| field == *name));
        }
        if let Some(properties) = expected_input
            .get_mut("properties")
            .and_then(Value::as_object_mut)
        {
            for field in injected {
                if let Some(property) = properties.get_mut(field).and_then(Value::as_object_mut) {
                    let flag = if field == "baseline_resource_id" {
                        "--baseline"
                    } else {
                        "--bind"
                    };
                    property.insert(
                        "description".to_string(),
                        Value::String(format!(
                            "Injected by the CLI adapter from {flag}; omit this field from operation JSON."
                        )),
                    );
                }
            }
        }
        assert_eq!(schema["input_schema"], expected_input);

        let example_output = assert_cmd::cargo::cargo_bin_cmd!("asp")
            .args(["example", descriptor.name])
            .output()
            .expect("example command");
        if descriptor.is_available_for(
            agent_spreadsheet::operations::OperationAdapter::Cli,
            &RuntimeCapabilities::native(),
        ) {
            assert!(
                example_output.status.success(),
                "example {} failed: {}",
                descriptor.name,
                String::from_utf8_lossy(&example_output.stderr)
            );
            let example: Value = serde_json::from_slice(&example_output.stdout).unwrap();
            jsonschema::validator_for(&expected_input)
                .unwrap()
                .validate(&example)
                .unwrap_or_else(|error| {
                    panic!(
                        "example for {} does not match its registry schema: {error}; {example}",
                        descriptor.name
                    )
                });
        } else {
            assert!(!example_output.status.success());
            let error: CanonicalErrorEnvelope =
                serde_json::from_slice(&example_output.stderr).unwrap();
            assert_eq!(error.error.code, CanonicalErrorCode::CapabilityUnavailable);
        }
    }
}

#[test]
fn cli_mutable_export_and_two_resource_binding_are_explicit() {
    let revision = agent_spreadsheet::utils::hash_file_sha256_hex(&fixture()).unwrap();
    let recalculate = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args([
            "op",
            "recalculate",
            "--bind",
            fixture().to_str().unwrap(),
            "--json",
            &json!({"expected_revision":revision}).to_string(),
        ])
        .output()
        .unwrap();
    assert!(!recalculate.status.success());
    let error: CanonicalErrorEnvelope = serde_json::from_slice(&recalculate.stderr).unwrap();
    assert_eq!(error.error.path.as_deref(), Some("adapter_flags"));

    let verify = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args([
            "op",
            "verify_workbook",
            "--bind",
            fixture().to_str().unwrap(),
            "--baseline",
            fixture().to_str().unwrap(),
            "--json",
            "{}",
        ])
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let response: Value = serde_json::from_slice(&verify.stdout).unwrap();
    assert_eq!(response["data"]["proof_status"], "proved");
    assert!(
        response["data"]["baseline_resource_id"]
            .as_str()
            .unwrap()
            .starts_with("wb:")
    );
}

#[test]
fn descriptor_schemas_drive_machine_binding_rules() {
    let portable = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args([
            "op",
            "sheetport_manifest",
            "--bind",
            fixture().to_str().unwrap(),
            "--json",
            r#"{"action":"schema"}"#,
        ])
        .output()
        .unwrap();
    assert!(!portable.status.success());
    let portable_error: CanonicalErrorEnvelope = serde_json::from_slice(&portable.stderr).unwrap();
    assert_eq!(
        portable_error.error.code,
        CanonicalErrorCode::InvalidRequest
    );
    assert_eq!(portable_error.error.path.as_deref(), Some("--bind"));

    let bound = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args([
            "op",
            "sheetport_manifest",
            "--json",
            r#"{"action":"candidates"}"#,
        ])
        .output()
        .unwrap();
    assert!(!bound.status.success());
    let bound_error: CanonicalErrorEnvelope = serde_json::from_slice(&bound.stderr).unwrap();
    assert_eq!(bound_error.error.code, CanonicalErrorCode::InvalidRequest);
    assert_eq!(bound_error.error.path.as_deref(), Some("--bind"));
}

#[allow(dead_code)]
fn _response_types_are_public(_: CanonicalResponse) {}

#[tokio::test]
async fn row_header_projection_and_volatile_groups_preserve_canonical_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("canonical-rich.xlsx");
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.get_cell_mut("A2").set_value("Name");
    sheet.get_cell_mut("B2").set_value("Amount");
    sheet.get_cell_mut("A3").set_value("Alpha");
    sheet.get_cell_mut("B3").set_value_number(42_f64);
    sheet.get_cell_mut("C3").set_formula("OFFSET(A3,0,0)");
    sheet.get_cell_mut("D3").set_formula("NOW()");
    umya_spreadsheet::writer::xlsx::write(&book, &path).unwrap();

    let (state, workbook_id) = StatelessRuntime
        .open_state_for_file(&path)
        .await
        .expect("bind rich fixture");
    let resource_id = ResourceId::bind_workbook(&workbook_id).unwrap();
    let rows = execute_operation_json(
        state.clone(),
        "read_cells",
        json!({
            "resource_id":resource_id.as_str(),
            "sheet_name":"Sheet1",
            "selection":{
                "kind":"rows", "start_row":3, "row_count":1,
                "columns":{"kind":"headers","values":["Amount"],"header_row":2},
                "include_header":true
            },
            "format":"full"
        }),
    )
    .await
    .unwrap();
    assert_eq!(rows.data["header"]["row_index"], 2);
    assert_eq!(
        rows.data["blocks"][0]["payload"]["snapshots"][0]["cells"][0]["address"],
        "B3"
    );

    let compact_details = execute_operation_json(
        state.clone(),
        "read_cells",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "selection":{"kind":"rows","start_row":3,"row_count":1,"columns":{"kind":"letters","values":["C"]}},
            "format":"compact", "include_formulas":true, "include_styles":true
        }),
    )
    .await
    .unwrap();
    let payload = &compact_details.data["blocks"][0]["payload"];
    assert_eq!(payload["projected"][0][0]["formula"], "OFFSET(A3,0,0)");
    assert_eq!(payload["projected"][0][0]["address"], "C3");
    for field in [
        "value",
        "formula",
        "cached_value",
        "stored_kind",
        "number_format",
        "style_tags",
    ] {
        assert!(
            payload["projected"][0][0].get(field).is_some(),
            "missing default projected field {field}"
        );
    }
    assert!(payload.get("snapshots").is_none());
    assert!(payload.get("compact").is_none());

    let compact_plain = execute_operation_json(
        state.clone(),
        "read_cells",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "selection":{"kind":"rows","start_row":3,"row_count":1,"columns":{"kind":"letters","values":["A"]}},
            "format":"compact"
        }),
    )
    .await
    .unwrap();
    let plain_payload = &compact_plain.data["blocks"][0]["payload"];
    assert!(plain_payload["compact"].is_object());
    assert!(plain_payload.get("projected").is_none());
    assert!(plain_payload.get("snapshots").is_none());

    let compact_explicit = execute_operation_json(
        state.clone(),
        "read_cells",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "selection":{"kind":"rows","start_row":3,"row_count":1,"columns":{"kind":"letters","values":["A"]}},
            "format":"compact", "fields":["value"]
        }),
    )
    .await
    .unwrap();
    let explicit_payload = &compact_explicit.data["blocks"][0]["payload"];
    assert!(explicit_payload["projected"].is_array());
    assert!(explicit_payload.get("compact").is_none());
    assert!(explicit_payload.get("snapshots").is_none());

    let grid = execute_operation_json(
        state.clone(),
        "export_grid",
        json!({
            "resource_id":resource_id.as_str(),
            "sheet_name":"Sheet1",
            "range":"A1:B2"
        }),
    )
    .await
    .unwrap();
    assert_eq!(grid.data["returned_range"], "A1:B2");
    assert_eq!(grid.data["grid"]["rows"].as_array().unwrap().len(), 2);
    assert!(
        grid.data["grid"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["cells"].as_array().unwrap().len() == 2)
    );

    let formulas = execute_operation_json(
        state,
        "search_formulas",
        json!({
            "resource_id":resource_id.as_str(),
            "filter":{"volatile":true},
            "result_mode":"groups",
            "group_by":"function"
        }),
    )
    .await
    .unwrap();
    let keys = formulas.data["groups"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|group| group["group_key"].as_str())
        .collect::<HashSet<_>>();
    assert_eq!(keys, HashSet::from(["NOW", "OFFSET"]));
    assert!(!keys.contains("volatile"));
}

async fn bound_path(
    path: &Path,
) -> (
    std::sync::Arc<agent_spreadsheet::state::AppState>,
    ResourceId,
) {
    let (state, workbook_id) = StatelessRuntime
        .open_state_for_file(path)
        .await
        .expect("bind temporary fixture");
    let resource_id = ResourceId::bind_workbook(&workbook_id).unwrap();
    (state, resource_id)
}

#[tokio::test]
async fn canonical_reads_reject_nonprogress_and_project_every_declared_cell_field() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("read-fields.xlsx");
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.get_cell_mut("A1").set_value_number(42_f64);
    sheet.get_cell_mut("B1").set_formula("SUM(A1,1)");
    sheet.get_cell_mut("C1").set_value("x".repeat(90_000));
    umya_spreadsheet::writer::xlsx::write(&book, &path).unwrap();
    let (state, resource_id) = bound_path(&path).await;

    let projected = execute_operation_json(
        state.clone(),
        "read_cells",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "selection":{"kind":"range","ranges":["A1:B1"]}, "format":"values",
            "fields":["value","formula","cached_value","stored_kind","number_format","style_tags"]
        }),
    )
    .await
    .unwrap();
    let cells = projected.data["blocks"][0]["payload"]["projected"][0]
        .as_array()
        .unwrap();
    for cell in cells {
        for field in [
            "value",
            "formula",
            "cached_value",
            "stored_kind",
            "number_format",
            "style_tags",
        ] {
            assert!(cell.get(field).is_some(), "missing {field}: {cell}");
        }
    }

    let error = execute_operation_json(
        state.clone(),
        "read_cells",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "selection":{"kind":"range","ranges":["C1:C1"]}, "format":"values"
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.error.code, CanonicalErrorCode::RowExceedsBudget);
    assert!(error.error.message.contains("inspect_cells"));
    assert!(error.error.message.contains("narrow the requested range"));

    let inspect_error = execute_operation_json(
        state.clone(),
        "inspect_cells",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1", "targets":["C1"]
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        inspect_error.error.code,
        CanonicalErrorCode::RowExceedsBudget
    );
    assert!(inspect_error.error.message.contains("inspect_cells"));
    assert!(
        inspect_error
            .error
            .message
            .contains("narrow the requested ranges")
    );
}

#[tokio::test]
async fn read_cells_budget_uses_one_row_prefix_for_rows_and_exact_ranges() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("read-budget-prefix.xlsx");
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    let expected = (1..=200)
        .map(|row| format!("row{row:03}-{}", "x".repeat(213)))
        .collect::<Vec<_>>();
    for (row, value) in expected.iter().enumerate() {
        sheet.get_cell_mut(format!("A{}", row + 1)).set_value(value);
    }
    umya_spreadsheet::writer::xlsx::write(&book, &path).unwrap();
    let (state, resource_id) = bound_path(&path).await;
    let fields = json!([
        "value",
        "formula",
        "cached_value",
        "stored_kind",
        "number_format",
        "style_tags"
    ]);

    let mut row_cursor: Option<String> = None;
    let mut row_values = Vec::new();
    loop {
        let mut request = json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "selection":{"kind":"rows","start_row":1,"row_count":200,"include_header":false},
            "format":"values", "page_size":200, "fields":fields
        });
        if let Some(cursor) = row_cursor.take() {
            request["cursor"] = json!(cursor);
        }
        let response = execute_operation_json(state.clone(), "read_cells", request)
            .await
            .unwrap();
        let block = &response.data["blocks"][0];
        let count = block["row_count"].as_u64().unwrap() as usize;
        assert!(count > 0);
        assert_eq!(block["row_indices"].as_array().unwrap().len(), count);
        assert_eq!(
            block["payload"]["values_only"]["rows"]
                .as_array()
                .unwrap()
                .len(),
            count
        );
        assert_eq!(
            block["payload"]["projected"].as_array().unwrap().len(),
            count
        );
        assert_eq!(
            block["payload"]["snapshots"].as_array().unwrap().len(),
            count
        );
        for (value_row, projected_row) in block["payload"]["values_only"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .zip(block["payload"]["projected"].as_array().unwrap())
        {
            let value = value_row[0]["value"].as_str().unwrap();
            assert_eq!(projected_row[0]["value"]["value"], value);
            row_values.push(value.to_string());
        }
        let complete = response.data["page"]["complete"].as_bool().unwrap();
        row_cursor = response.data["page"]["next_cursor"]
            .as_str()
            .map(str::to_string);
        assert_eq!(complete, row_cursor.is_none());
        if complete {
            break;
        }
    }
    assert_eq!(row_values, expected);

    let mut range_cursor: Option<String> = None;
    let mut range_values = Vec::new();
    loop {
        let mut request = json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "selection":{"kind":"range","ranges":["A1:A200"]},
            "format":"values", "page_size":200, "fields":fields
        });
        if let Some(cursor) = range_cursor.take() {
            request["cursor"] = json!(cursor);
        }
        let response = execute_operation_json(state.clone(), "read_cells", request)
            .await
            .unwrap();
        let block = &response.data["blocks"][0];
        let count = block["row_count"].as_u64().unwrap() as usize;
        assert!(count > 0);
        assert_eq!(block["payload"]["values"].as_array().unwrap().len(), count);
        assert_eq!(
            block["payload"]["projected"].as_array().unwrap().len(),
            count
        );
        for (value_row, projected_row) in block["payload"]["values"]
            .as_array()
            .unwrap()
            .iter()
            .zip(block["payload"]["projected"].as_array().unwrap())
        {
            let value = value_row[0].as_str().unwrap();
            assert_eq!(projected_row[0]["value"]["value"], value);
            range_values.push(value.to_string());
        }
        let complete = response.data["page"]["complete"].as_bool().unwrap();
        range_cursor = response.data["page"]["next_cursor"]
            .as_str()
            .map(str::to_string);
        assert_eq!(complete, range_cursor.is_none());
        if complete {
            break;
        }
    }
    assert_eq!(range_values, expected);
}

#[tokio::test]
async fn detailed_compact_rows_match_the_original_trace02_request_without_duplication() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("trace02-compact-rows.xlsx");
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.set_name("Data");
    for (column, header) in [
        ("A", "ID"),
        ("B", "Region"),
        ("C", "Product"),
        ("D", "Units"),
        ("E", "UnitPrice"),
        ("F", "Revenue"),
        ("G", "Status"),
        ("H", "Notes"),
        ("I", "Flag"),
        ("J", "Oversize"),
    ] {
        sheet.get_cell_mut(format!("{column}1")).set_value(header);
    }
    for row in 2..=1201 {
        let item = row - 1;
        let units = f64::from((item * 7) % 97 + 1);
        let price = f64::from((item * 137) % 10_000 + 925) / 100.0;
        let revenue = (units * price * 100.0).round() / 100.0;
        sheet
            .get_cell_mut(format!("A{row}"))
            .set_value(format!("R{item:04}"));
        sheet
            .get_cell_mut(format!("D{row}"))
            .set_value_number(units);
        sheet
            .get_cell_mut(format!("E{row}"))
            .set_value_number(price);
        sheet
            .get_cell_mut(format!("F{row}"))
            .set_formula(format!("D{row}*E{row}"))
            .set_formula_result_default(revenue.to_string());
        sheet.get_cell_mut(format!("H{row}")).set_value(format!(
            "audit-row-{item:04}-{}-end-{item:04}",
            "x".repeat(150)
        ));
        for column in ["E", "F"] {
            sheet
                .get_style_mut(format!("{column}{row}"))
                .get_number_format_mut()
                .set_format_code("$#,##0.00");
        }
    }
    umya_spreadsheet::writer::xlsx::write(&book, &path).unwrap();
    let (state, resource_id) = bound_path(&path).await;

    let mut cursor = None;
    let mut turns = 0_usize;
    let mut response_bytes = 0_usize;
    let mut row_indices = Vec::new();
    let mut formula_count = 0_usize;
    let mut cached_count = 0_usize;
    let mut currency_count = 0_usize;
    loop {
        let mut request = json!({
            "resource_id":resource_id.as_str(),
            "sheet_name":"Data",
            "selection":{
                "kind":"rows", "start_row":2, "row_count":1200,
                "columns":{"kind":"headers","values":["ID","UnitPrice","Revenue","Notes"],"header_row":1},
                "include_header":true
            },
            "format":"compact", "include_formulas":true, "include_styles":true,
            "page_size":500
        });
        if let Some(value) = cursor.take() {
            request["cursor"] = Value::String(value);
        }
        let response = execute_operation_json(state.clone(), "read_cells", request)
            .await
            .unwrap();
        turns += 1;
        response_bytes += serde_json::to_vec(&response).unwrap().len();
        let block = &response.data["blocks"][0];
        let payload = block["payload"].as_object().unwrap();
        assert_eq!(
            payload.keys().map(String::as_str).collect::<HashSet<_>>(),
            HashSet::from(["encoding", "projected"])
        );
        let indices = block["row_indices"].as_array().unwrap();
        let projected = payload["projected"].as_array().unwrap();
        assert_eq!(indices.len(), projected.len());
        for (index, cells) in indices.iter().zip(projected) {
            let row = index.as_u64().unwrap() as u32;
            row_indices.push(row);
            let cells = cells.as_array().unwrap();
            assert_eq!(
                cells
                    .iter()
                    .map(|cell| cell["address"].as_str().unwrap().to_string())
                    .collect::<Vec<_>>(),
                [
                    format!("A{row}"),
                    format!("E{row}"),
                    format!("F{row}"),
                    format!("H{row}")
                ]
            );
            assert!(cells.iter().all(|cell| {
                [
                    "address",
                    "value",
                    "formula",
                    "cached_value",
                    "stored_kind",
                    "number_format",
                    "style_tags",
                ]
                .iter()
                .all(|field| cell.get(field).is_some())
            }));
            assert_eq!(cells[2]["formula"], format!("D{row}*E{row}"));
            formula_count += 1;
            cached_count += usize::from(!cells[2]["cached_value"].is_null());
            currency_count += cells
                .iter()
                .filter(|cell| {
                    cell["style_tags"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|tag| tag == "currency")
                })
                .count();
        }
        cursor = response.data["page"]["next_cursor"]
            .as_str()
            .map(str::to_string);
        if response.data["page"]["complete"].as_bool().unwrap() {
            assert!(cursor.is_none());
            break;
        }
    }

    assert_eq!(row_indices, (2..=1201).collect::<Vec<_>>());
    assert_eq!(formula_count, 1200);
    assert_eq!(cached_count, 1200);
    assert_eq!(currency_count, 2400);
    assert!(turns <= 16, "detailed compact read took {turns} turns");
    assert!(
        (900_000..=1_100_000).contains(&response_bytes),
        "unexpected detailed compact response cost: {response_bytes} bytes"
    );
}

#[tokio::test]
async fn analyze_styles_scan_limit_bounds_all_returned_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("style-scan-bound.xlsx");
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    for row in 1..=3 {
        sheet
            .get_cell_mut(format!("A{row}"))
            .set_value(format!("value-{row}"));
    }
    umya_spreadsheet::writer::xlsx::write(&book, &path).unwrap();
    let (state, resource_id) = bound_path(&path).await;

    let styles = execute_operation_json(
        state,
        "analyze_styles",
        json!({
            "resource_id":resource_id.as_str(),
            "scope":{"kind":"sheet","sheet_name":"Sheet1","selection":{"kind":"all"}},
            "include":["ranges","example_cells"],
            "limits":{"cells_scanned":1,"examples_per_style":10,"ranges_per_style":10}
        }),
    )
    .await
    .unwrap();
    assert_eq!(styles.data["coverage"]["cells_scanned"], 1);
    assert_eq!(styles.data["coverage"]["cells_in_scope"], 3);
    assert_eq!(styles.data["coverage"]["counts_exact"], false);
    let usages = styles.data["styles"].as_array().unwrap();
    assert_eq!(
        usages
            .iter()
            .map(|style| style["occurrences"].as_u64().unwrap())
            .sum::<u64>(),
        1
    );
    let examples = usages
        .iter()
        .flat_map(|style| style["example_cells"].as_array().unwrap())
        .collect::<Vec<_>>();
    let ranges = usages
        .iter()
        .flat_map(|style| style["ranges"].as_array().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(examples.len(), 1);
    assert_eq!(ranges, examples);
}

#[tokio::test]
async fn profile_table_reports_resolved_region_bounds_and_header() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile-region-source.xlsx");
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.get_cell_mut("A5").set_value("Name");
    sheet.get_cell_mut("B5").set_value("Amount");
    for row in 6..=15 {
        sheet
            .get_cell_mut(format!("A{row}"))
            .set_value(format!("item-{row}"));
        sheet
            .get_cell_mut(format!("B{row}"))
            .set_value_number(row as f64);
    }
    let mut table = umya_spreadsheet::structs::Table::new("SourceTable", ("A5", "B15"));
    table.set_display_name("SourceTable");
    sheet.add_table(table);
    sheet.get_cell_mut("F5").set_value("Total");
    sheet.get_cell_mut("F6").set_value("needle-outside-table");
    umya_spreadsheet::writer::xlsx::write(&book, &path).unwrap();
    let (state, resource_id) = bound_path(&path).await;

    let overview = execute_operation_json(
        state.clone(),
        "sheet_overview",
        json!({"resource_id":resource_id.as_str(),"sheet_name":"Sheet1"}),
    )
    .await
    .unwrap();
    let region = overview.data["detected_regions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|region| region["bounds"] == "A5:B15")
        .expect("A5:B15 region");
    assert_eq!(region["header_row"], 5);

    let profile = execute_operation_json(
        state.clone(),
        "profile_table",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "region_id":region["id"], "sample_size":2
        }),
    )
    .await
    .unwrap();
    assert_eq!(profile.data["source"]["bounds"], "A5:B15");
    assert_eq!(profile.data["source"]["header_row"], 5);
    assert_eq!(
        profile.data["source"]["header_provenance"],
        "detected_region"
    );

    let range_profile = execute_operation_json(
        state.clone(),
        "profile_table",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "range":"A5:B15", "sample_size":2
        }),
    )
    .await
    .unwrap();
    assert_eq!(range_profile.data["source"]["selector_kind"], "range");
    assert_eq!(range_profile.data["source"]["selector_value"], "A5:B15");
    assert_eq!(range_profile.data["source"]["bounds"], "A5:B15");
    assert_eq!(range_profile.data["source"]["header_row"], 5);
    assert_eq!(
        range_profile.data["source"]["header_provenance"],
        "range_first_row"
    );

    let table_profile = execute_operation_json(
        state.clone(),
        "profile_table",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "table_name":"SourceTable", "sample_size":2
        }),
    )
    .await
    .unwrap();
    assert_eq!(table_profile.data["source"]["selector_kind"], "table");
    assert_eq!(
        table_profile.data["source"]["selector_value"],
        "SourceTable"
    );
    assert_eq!(table_profile.data["source"]["bounds"], "A5:B15");
    assert_eq!(table_profile.data["source"]["header_row"], 5);
    assert_eq!(
        table_profile.data["source"]["header_provenance"],
        "table_definition"
    );
    assert_eq!(table_profile.data["coverage"]["rows_scanned"], 2);
    assert_eq!(table_profile.data["coverage"]["rows_in_scope"], 10);
    assert_eq!(table_profile.data["coverage"]["complete"], false);
    let amount = table_profile.data["column_types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|column| column["name"] == "Amount")
        .unwrap();
    assert_eq!(amount["distinct"], 2);

    let scoped = execute_operation_json(
        state.clone(),
        "search_values",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "table_name":"SourceTable", "query":"needle-outside-table", "match_mode":"exact"
        }),
    )
    .await
    .unwrap();
    assert_eq!(scoped.data["match_count"], 0);

    let header = execute_operation_json(
        state,
        "search_values",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "table_name":"SourceTable", "query":"Amount", "match_mode":"exact",
            "search_headers_only":true
        }),
    )
    .await
    .unwrap();
    assert_eq!(header.data["match_count"], 1);
    assert_eq!(header.data["matches"][0]["address"], "B5");
}

#[tokio::test]
async fn profile_table_normalizes_explicit_a1_and_rejects_malformed_across_surfaces() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile-explicit-range.xlsx");
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.get_cell_mut("A5").set_value("Name");
    sheet.get_cell_mut("B5").set_value("Amount");
    sheet.get_cell_mut("A15").set_value("last");
    sheet.get_cell_mut("B15").set_value_number(15_f64);
    umya_spreadsheet::writer::xlsx::write(&book, &path).unwrap();
    let (state, resource_id) = bound_path(&path).await;

    for requested_range in ["a5:b15", "$A$5:$B$15"] {
        let payload = json!({"sheet_name":"Sheet1","range":requested_range});
        let dispatcher = execute_operation_json(
            state.clone(),
            "profile_table",
            with_resource(&resource_id, payload.clone()),
        )
        .await
        .expect("normalized dispatcher range");
        let dispatcher = serde_json::to_value(dispatcher).unwrap();
        let cli = asp_op_bound("profile_table", payload, &path).expect("normalized asp op range");
        assert_eq!(cli, dispatcher);
        assert_eq!(dispatcher["data"]["source"]["selector_kind"], "range");
        assert_eq!(dispatcher["data"]["source"]["selector_value"], "A5:B15");
        assert_eq!(dispatcher["data"]["source"]["bounds"], "A5:B15");
        assert_eq!(dispatcher["data"]["source"]["header_row"], 5);
        assert_eq!(
            dispatcher["data"]["source"]["header_provenance"],
            "range_first_row"
        );
    }

    let payload = json!({"sheet_name":"Sheet1","range":"not-a-range"});
    let dispatcher = execute_operation_json(
        state,
        "profile_table",
        with_resource(&resource_id, payload.clone()),
    )
    .await
    .expect_err("malformed dispatcher range");
    let dispatcher = serde_json::to_value(dispatcher).unwrap();
    let cli = asp_op_bound("profile_table", payload, &path).expect_err("malformed asp op range");
    assert_eq!(cli, dispatcher);
    assert_eq!(dispatcher["error"]["code"], "INVALID_REQUEST");
    assert_eq!(dispatcher["error"]["operation"], "profile_table");
    assert_eq!(dispatcher["error"]["path"], "$.range");
    assert_eq!(dispatcher["error"]["message"], "invalid range: not-a-range");
}

#[tokio::test]
async fn formula_map_address_sort_is_natural_and_stable_across_cursor_pages() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("formula-map-address-order.xlsx");
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    for (address, formula) in [
        ("A10", "SUM(10,1)"),
        ("A2", "MAX(2,1)"),
        ("Z1", "MIN(1,0)"),
        ("B1", "ABS(-1)"),
    ] {
        sheet.get_cell_mut(address).set_formula(formula);
    }
    umya_spreadsheet::writer::xlsx::write(&book, &path).unwrap();
    let (state, resource_id) = bound_path(&path).await;
    let mut cursor = None;
    let mut addresses = Vec::new();
    loop {
        let mut request = json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1",
            "sort_by":"address", "summary_only":false, "include_addresses":true, "limit":2
        });
        if let Some(value) = cursor {
            request["cursor"] = Value::String(value);
        }
        let page = execute_operation_json(state.clone(), "formula_map", request)
            .await
            .unwrap();
        addresses.extend(
            page.data["groups"]
                .as_array()
                .unwrap()
                .iter()
                .map(|group| group["addresses"][0].as_str().unwrap().to_string()),
        );
        cursor = page.data["next_cursor"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(addresses, ["B1", "Z1", "A2", "A10"]);
}

#[tokio::test]
async fn formula_search_scans_all_pages_honors_range_and_uses_bound_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("formula-search.xlsx");
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    for row in 1..=520 {
        sheet
            .get_cell_mut(format!("A{row}"))
            .set_formula(format!("SUM({row},1)"));
    }
    sheet.get_cell_mut("C1").set_formula("SUM(OFFSET(A1,0,0))");
    umya_spreadsheet::writer::xlsx::write(&book, &path).unwrap();
    let (state, resource_id) = bound_path(&path).await;

    let last = execute_operation_json(
        state.clone(),
        "search_formulas",
        json!({
            "resource_id":resource_id.as_str(),
            "scope":{"kind":"sheet","sheet_name":"Sheet1","range":"A520:A520"},
            "query":{"text":"520","match_mode":"contains"}, "result_mode":"cells"
        }),
    )
    .await
    .unwrap();
    assert_eq!(last.data["matches"][0]["address"], "A520");
    assert_eq!(last.data["summary"]["formula_cells_scanned"], 521);
    assert_eq!(last.data["summary"]["scan_complete"], true);

    let grouped = execute_operation_json(
        state.clone(),
        "search_formulas",
        json!({
            "resource_id":resource_id.as_str(), "scope":{"kind":"sheet","sheet_name":"Sheet1","range":"C1:C1"},
            "filter":{"volatile":true}, "result_mode":"groups", "group_by":"function"
        }),
    )
    .await
    .unwrap();
    assert_eq!(grouped.data["groups"].as_array().unwrap().len(), 1);
    assert_eq!(grouped.data["groups"][0]["group_key"], "OFFSET");

    let first = execute_operation_json(
        state.clone(),
        "search_formulas",
        json!({
            "resource_id":resource_id.as_str(), "query":{"text":"SUM"},
            "result_mode":"cells", "limit":1
        }),
    )
    .await
    .unwrap();
    let cursor = first.data["next_cursor"].as_str().unwrap();
    assert!(cursor.starts_with("sf1_"));
    let second = execute_operation_json(
        state.clone(),
        "search_formulas",
        json!({
            "resource_id":resource_id.as_str(), "query":{"text":"SUM"},
            "result_mode":"cells", "limit":1, "cursor":cursor
        }),
    )
    .await
    .unwrap();
    assert_ne!(
        first.data["matches"][0]["address"],
        second.data["matches"][0]["address"]
    );
    let mismatch = execute_operation_json(
        state,
        "search_formulas",
        json!({
            "resource_id":resource_id.as_str(), "query":{"text":"OFFSET"},
            "result_mode":"cells", "limit":1, "cursor":cursor
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(mismatch.error.code, CanonicalErrorCode::CursorMismatch);
}

#[tokio::test]
async fn canonical_analysis_bounds_paths_paging_and_profile_provenance_are_explicit() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("analysis-contracts.xlsx");
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.get_cell_mut("A1").set_value("Header");
    for row in 2..=22 {
        sheet
            .get_cell_mut(format!("A{row}"))
            .set_value_number(row as f64);
    }
    sheet.get_cell_mut("B1").set_formula("SUM(A2:A22)");
    sheet.get_cell_mut("C1").set_formula("AVERAGE(A2:A22)");
    for row in 2..=22 {
        sheet
            .get_cell_mut(format!("B{row}"))
            .set_formula(format!("A2+{row}"));
    }
    umya_spreadsheet::writer::xlsx::write(&book, &path).unwrap();
    let (state, resource_id) = bound_path(&path).await;

    let hidden = execute_operation_json(
        state.clone(),
        "describe_workbook",
        json!({"resource_id":resource_id.as_str()}),
    )
    .await
    .unwrap();
    assert!(hidden.data.get("paths").is_none());
    let visible = execute_operation_json(
        state.clone(),
        "describe_workbook",
        json!({"resource_id":resource_id.as_str(),"include_paths":true}),
    )
    .await
    .unwrap();
    assert!(visible.data["paths"]["internal"].is_string());

    let styles = execute_operation_json(
        state.clone(),
        "analyze_styles",
        json!({
            "resource_id":resource_id.as_str(),
            "scope":{"kind":"sheet","sheet_name":"Sheet1","selection":{"kind":"all"}},
            "include":["ranges","example_cells"],
            "limits":{"cells_scanned":1,"examples_per_style":10,"ranges_per_style":10}
        }),
    )
    .await
    .unwrap();
    assert_eq!(styles.data["coverage"]["status"], "bounded");
    assert_eq!(styles.data["coverage"]["cells_scanned"], 1);
    assert_eq!(styles.data["coverage"]["counts_exact"], false);
    assert_eq!(
        styles.data["styles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|style| style["occurrences"].as_u64().unwrap())
            .sum::<u64>(),
        1
    );
    assert!(
        styles.data["styles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|style| {
                let ranges = style["ranges"].as_array().unwrap();
                let examples = style["example_cells"].as_array().unwrap();
                ranges.len() == 1 && examples.len() == 1 && ranges[0] == examples[0]
            })
    );
    assert!(styles.data["conditional_formats_complete"].is_boolean());

    let map = execute_operation_json(
        state.clone(),
        "formula_map",
        json!({"resource_id":resource_id.as_str(),"sheet_name":"Sheet1","limit":1}),
    )
    .await
    .unwrap();
    let map_cursor = map.data["next_cursor"].as_str().unwrap();
    assert!(map_cursor.starts_with("fm1_"));
    let resumed = execute_operation_json(
        state.clone(),
        "formula_map",
        json!({"resource_id":resource_id.as_str(),"sheet_name":"Sheet1","limit":1,"cursor":map_cursor}),
    )
    .await
    .unwrap();
    assert_ne!(
        map.data["groups"][0]["fingerprint"],
        resumed.data["groups"][0]["fingerprint"]
    );

    let trace = execute_operation_json(
        state.clone(),
        "formula_trace",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1", "cell_address":"A2",
            "direction":"dependents", "page_size":10
        }),
    )
    .await
    .unwrap();
    let trace_cursor = trace.data["next_cursor"].as_str().unwrap();
    assert!(trace_cursor.starts_with("ft1_"));
    let trace_resumed = execute_operation_json(
        state.clone(),
        "formula_trace",
        json!({
            "resource_id":resource_id.as_str(), "sheet_name":"Sheet1", "cell_address":"A2",
            "direction":"dependents", "page_size":10, "cursor":trace_cursor
        }),
    )
    .await
    .unwrap();
    assert_ne!(
        trace.data["layers"][0]["edges"],
        trace_resumed.data["layers"][0]["edges"]
    );

    let profile = execute_operation_json(
        state,
        "profile_table",
        json!({"resource_id":resource_id.as_str(),"sheet_name":"Sheet1","sample_size":2}),
    )
    .await
    .unwrap();
    assert_eq!(
        profile.data["source"]["header_provenance"],
        "inferred_first_row"
    );
    assert_eq!(profile.data["coverage"]["rows_scanned"], 2);
    assert_eq!(profile.data["confidence"]["heuristic"], true);
}
