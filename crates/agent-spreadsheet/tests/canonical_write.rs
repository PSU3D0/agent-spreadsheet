#![cfg(feature = "recalc")]

use agent_spreadsheet::canonical_write::{WriteOpStatus, WriteRequest};
use agent_spreadsheet::operations::{CanonicalErrorCode, ResourceId, execute_operation_json};
use agent_spreadsheet::runtime::stateless::StatelessRuntime;
use agent_spreadsheet::utils::hash_file_sha256_hex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use umya_spreadsheet::EnumTrait;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/f1/baseline.xlsx")
}

async fn forked() -> (
    tempfile::TempDir,
    Arc<agent_spreadsheet::state::AppState>,
    ResourceId,
    PathBuf,
    String,
) {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.xlsx");
    std::fs::copy(fixture(), &input).unwrap();
    let (state, fork_id, fork_path) = StatelessRuntime
        .open_fork_state_for_file(&input)
        .await
        .unwrap();
    let resource = ResourceId::bind_workbook(&fork_id).unwrap();
    let revision = hash_file_sha256_hex(&fork_path).unwrap();
    (temp, state, resource, fork_path, revision)
}

fn request(
    resource: &ResourceId,
    revision: &str,
    mode: &str,
    atomic: bool,
    ops: Vec<Value>,
) -> Value {
    json!({
        "resource_id": resource.as_str(),
        "expected_revision": revision,
        "mode": mode,
        "atomic": atomic,
        "ops": ops,
    })
}

fn set_cell(value: Value) -> Value {
    json!({
        "kind":"set_cells",
        "sheet_name":"Sheet1",
        "cells":{"A1":{"kind":"value","value":value}}
    })
}

fn run_asp_write(bind: &Path, payload: &Value, output: Option<&Path>) -> std::process::Output {
    let mut command = assert_cmd::cargo::cargo_bin_cmd!("asp");
    command
        .args(["op", "write", "--bind"])
        .arg(bind)
        .args(["--json", &payload.to_string()]);
    if let Some(output) = output {
        command.arg("--output").arg(output);
    }
    command.output().expect("run asp op write")
}

fn fill_ops() -> Vec<Value> {
    vec![
        json!({
            "kind":"style",
            "sheet_name":"Sheet1",
            "target":{"kind":"range","range":"A10"},
            "patch":{"fill":{
                "kind":"pattern",
                "pattern_type":"solid",
                "foreground_color":"FFFF0000"
            }}
        }),
        json!({
            "kind":"style",
            "sheet_name":"Sheet1",
            "target":{"kind":"range","range":"B10"},
            "patch":{"fill":{
                "kind":"gradient",
                "degree":45.0,
                "stops":[
                    {"position":0.0,"color":"FFFF0000"},
                    {"position":1.0,"color":"FF00FF00"}
                ]
            }}
        }),
    ]
}

#[test]
fn write_risk_is_request_aware_with_a_destructive_ceiling() {
    let descriptor = agent_spreadsheet::operations::operation_descriptor("write").unwrap();
    assert_eq!(
        descriptor.risk_ceiling,
        agent_spreadsheet::operations::OperationRisk::Destructive
    );
    let moderate = agent_spreadsheet::operations::decode_operation("write", json!({
        "resource_id":"fork:fork-test","expected_revision":"r","mode":"preview","ops":[set_cell(json!(1))]
    })).unwrap();
    assert_eq!(
        (descriptor.risk_for)(&moderate),
        agent_spreadsheet::operations::OperationRisk::Moderate
    );
    let destructive = agent_spreadsheet::operations::decode_operation(
        "write",
        json!({
            "resource_id":"fork:fork-test","expected_revision":"r","mode":"preview","ops":[
                {"kind":"delete_rows","sheet_name":"Sheet1","start_row":1,"count":1}
            ]
        }),
    )
    .unwrap();
    assert_eq!(
        (descriptor.risk_for)(&destructive),
        agent_spreadsheet::operations::OperationRisk::Destructive
    );
}

#[test]
fn write_schema_composes_tagged_fill_refs_without_reopening_them() {
    let schema = (agent_spreadsheet::operations::operation_descriptor("write")
        .unwrap()
        .input_schema)();
    let validator = jsonschema::validator_for(&schema).expect("valid write schema");
    let valid = json!({
        "resource_id":"fork:fork-test",
        "expected_revision":"revision",
        "mode":"preview",
        "ops":fill_ops(),
    });
    validator.validate(&valid).unwrap();

    let fill_variants = schema["$defs"]["FillPatch"]["oneOf"]
        .as_array()
        .expect("fill variants");
    assert!(fill_variants.iter().all(|variant| {
        variant["additionalProperties"] == Value::Bool(false)
            && variant.get("$ref").is_none()
            && variant["properties"].as_object().unwrap().len() > 1
    }));

    let mut unknown = valid;
    unknown["ops"][0]["patch"]["fill"]["unexpected"] = Value::Bool(true);
    assert!(validator.validate(&unknown).is_err());
}

#[test]
fn asp_op_write_previews_and_applies_non_empty_pattern_and_gradient_fills() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.xlsx");
    let applied_path = temp.path().join("applied.xlsx");
    std::fs::copy(fixture(), &input).unwrap();
    let revision = hash_file_sha256_hex(&input).unwrap();

    let preview_payload = json!({
        "expected_revision":revision,
        "mode":"preview",
        "ops":fill_ops(),
    });
    let preview = run_asp_write(&input, &preview_payload, None);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let preview_json: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(preview_json["data"]["status"], "previewed");
    assert_eq!(preview_json["data"]["ops_previewed"], 2);
    assert_eq!(hash_file_sha256_hex(&input).unwrap(), revision);

    let apply_payload = json!({
        "expected_revision":revision,
        "mode":"apply",
        "ops":fill_ops(),
    });
    let apply = run_asp_write(&input, &apply_payload, Some(&applied_path));
    assert!(
        apply.status.success(),
        "{}",
        String::from_utf8_lossy(&apply.stderr)
    );
    let apply_json: Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(apply_json["data"]["status"], "applied");
    assert_eq!(apply_json["data"]["ops_applied"], 2);

    let book = umya_spreadsheet::reader::xlsx::read(&applied_path).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    let pattern = sheet
        .style("A10")
        .fill()
        .expect("fill")
        .pattern_fill()
        .expect("pattern fill");
    assert_eq!(pattern.pattern_type().value_string(), "solid");
    assert_eq!(
        pattern.foreground_color().unwrap().argb_str(),
        "FFFF0000"
    );
    let gradient = sheet
        .style("B10")
        .fill()
        .expect("fill")
        .gradient_fill()
        .expect("gradient fill");
    assert_eq!(gradient.degree(), 45.0);
    assert_eq!(gradient.gradient_stop().len(), 2);
}

#[test]
fn human_table_append_matches_canonical_write_bytes_and_rich_detail() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.xlsx");
    let human_output = temp.path().join("human.xlsx");
    let canonical_output = temp.path().join("canonical.xlsx");
    let rows_path = temp.path().join("rows.json");
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.cell_mut("C1").set_value("Name");
    sheet.cell_mut("D1").set_value("Amount");
    sheet.cell_mut("C2").set_value("Alice");
    sheet.cell_mut("D2").set_value_number(10.0);
    sheet.cell_mut("C3").set_value("Bob");
    sheet.cell_mut("D3").set_value_number(20.0);
    sheet.cell_mut("C4").set_value("Total");
    sheet.cell_mut("D4").set_formula("SUM(D2:D3)");
    let mut table = umya_spreadsheet::structs::Table::new("SalesTable", ("C1", "D4"));
    table.set_display_name("SalesTable");
    sheet.add_table(table);
    umya_spreadsheet::writer::xlsx::write(&book, &input).unwrap();
    std::fs::write(&rows_path, r#"{"rows":[["Cara",30]]}"#).unwrap();

    let human = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args([
            "append-region",
            input.to_str().unwrap(),
            "--sheet",
            "Sheet1",
            "--table-name",
            "SalesTable",
            "--rows",
            &format!("@{}", rows_path.display()),
            "--output",
            human_output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        human.status.success(),
        "{}",
        String::from_utf8_lossy(&human.stderr)
    );
    let human_json: Value = serde_json::from_slice(&human.stdout).unwrap();

    let payload = json!({
        "expected_revision":hash_file_sha256_hex(&input).unwrap(),
        "mode":"apply",
        "ops":[{
            "kind":"append_rows",
            "sheet_name":"Sheet1",
            "table_name":"SalesTable",
            "rows":[[{"v":"Cara"},{"v":30}]],
            "footer_policy":"auto"
        }]
    });
    let canonical = run_asp_write(&input, &payload, Some(&canonical_output));
    assert!(
        canonical.status.success(),
        "{}",
        String::from_utf8_lossy(&canonical.stderr)
    );
    let canonical_json: Value = serde_json::from_slice(&canonical.stdout).unwrap();
    let detail = &canonical_json["data"]["results"][0]["detail"];

    assert_eq!(
        std::fs::read(&human_output).unwrap(),
        std::fs::read(&canonical_output).unwrap()
    );
    for field in [
        "target_kind",
        "table_name",
        "region_bounds",
        "footer_row",
        "insert_at_row",
        "target_anchor",
        "target_range",
        "confidence",
        "warnings",
    ] {
        assert_eq!(
            human_json[field], detail[field],
            "rich detail field {field}"
        );
    }
    assert_eq!(detail["target_anchor"], "C4");
}

#[test]
fn asp_op_write_rejects_unknown_pattern_fill_fields() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.xlsx");
    std::fs::copy(fixture(), &input).unwrap();
    let payload = json!({
        "expected_revision":hash_file_sha256_hex(&input).unwrap(),
        "mode":"preview",
        "ops":[{
            "kind":"style",
            "sheet_name":"Sheet1",
            "target":{"kind":"range","range":"A10"},
            "patch":{"fill":{
                "kind":"pattern",
                "pattern_type":"solid",
                "foreground_color":"FFFF0000",
                "unexpected":true
            }}
        }],
    });
    let output = run_asp_write(&input, &payload, None);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unexpected")
    );
}

#[tokio::test]
async fn preview_is_pure_and_creates_no_stage() {
    let (_temp, state, resource, fork_path, revision) = forked().await;
    let response = execute_operation_json(
        state.clone(),
        "write",
        request(
            &resource,
            &revision,
            "preview",
            true,
            vec![set_cell(json!(42))],
        ),
    )
    .await
    .unwrap();

    let output_schema = (agent_spreadsheet::operations::operation_descriptor("write")
        .unwrap()
        .output_schema)();
    jsonschema::validator_for(&output_schema)
        .unwrap()
        .validate(&serde_json::to_value(&response).unwrap())
        .unwrap();
    assert_eq!(response.data["ops_previewed"], 1);
    assert_eq!(
        response.data["revision_before"],
        response.data["revision_after"]
    );
    assert_eq!(response.data["diff"]["change_count"], 1);
    assert_eq!(hash_file_sha256_hex(&fork_path).unwrap(), revision);
    let fork_id = resource.as_str().strip_prefix("fork:").unwrap();
    assert!(
        state
            .fork_registry()
            .unwrap()
            .list_staged_changes(fork_id)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn atomic_failure_rolls_back_and_non_atomic_failure_is_structured() {
    let (_temp, state, resource, fork_path, revision) = forked().await;
    let ops = vec![
        set_cell(json!(10)),
        json!({"kind":"set_cells","sheet_name":"Missing","cells":{"A1":{"kind":"value","value":20}}}),
        set_cell(json!(30)),
    ];
    let atomic = execute_operation_json(
        state.clone(),
        "write",
        request(&resource, &revision, "apply", true, ops.clone()),
    )
    .await
    .unwrap();
    assert_eq!(atomic.data["rolled_back"], true);
    assert_eq!(atomic.data["ops_applied"], 0);
    assert_eq!(atomic.data["results"][0]["status"], "rolled_back");
    assert_eq!(atomic.data["results"][1]["status"], "failed");
    assert_eq!(atomic.data["results"][2]["status"], "skipped");
    assert_eq!(hash_file_sha256_hex(&fork_path).unwrap(), revision);

    let partial = execute_operation_json(
        state,
        "write",
        request(&resource, &revision, "apply", false, ops),
    )
    .await
    .unwrap();
    assert_eq!(partial.data["ops_applied"], 1);
    assert_eq!(partial.data["results"][0]["status"], "applied");
    assert_eq!(partial.data["results"][1]["status"], "failed");
    assert_eq!(partial.data["results"][2]["status"], "skipped");
    assert_ne!(hash_file_sha256_hex(&fork_path).unwrap(), revision);
}

#[tokio::test]
async fn cas_conflict_has_zero_effects() {
    let (_temp, state, resource, fork_path, revision) = forked().await;
    let error = execute_operation_json(
        state,
        "write",
        request(&resource, "stale", "apply", true, vec![set_cell(json!(2))]),
    )
    .await
    .unwrap_err();
    assert_eq!(error.error.code, CanonicalErrorCode::RevisionConflict);
    assert_eq!(hash_file_sha256_hex(&fork_path).unwrap(), revision);
}

#[tokio::test]
async fn stage_creates_one_ordered_bundle_with_base_revision() {
    let (_temp, state, resource, _fork_path, revision) = forked().await;
    let response = execute_operation_json(
        state.clone(),
        "write",
        request(
            &resource,
            &revision,
            "stage",
            true,
            vec![set_cell(json!(2)), set_cell(json!(3))],
        ),
    )
    .await
    .unwrap();
    assert_eq!(response.data["ops_staged"], 2);
    let fork_id = resource.as_str().strip_prefix("fork:").unwrap();
    let staged = state
        .fork_registry()
        .unwrap()
        .list_staged_changes(fork_id)
        .unwrap();
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].ops.len(), 1);
    assert_eq!(staged[0].ops[0].kind, "canonical_write_bundle");
    assert_eq!(staged[0].ops[0].payload["base_revision"], revision);
    assert_eq!(staged[0].ops[0].payload["ops"][0]["kind"], "set_cells");
    assert_eq!(staged[0].ops[0].payload["ops"][1]["kind"], "set_cells");
}

#[tokio::test]
async fn staged_bundle_applies_through_the_canonical_dispatcher_in_order() {
    let (_temp, state, resource, fork_path, revision) = forked().await;
    let staged = execute_operation_json(
        state.clone(),
        "write",
        request(
            &resource,
            &revision,
            "stage",
            true,
            vec![
                set_cell(json!(2)),
                json!({
                    "kind":"set_cells",
                    "sheet_name":"Sheet1",
                    "cells":{"B1":{"kind":"formula","formula":"=A1*3"}}
                }),
            ],
        ),
    )
    .await
    .unwrap();
    let change_id = staged.data["change_id"].as_str().unwrap().to_string();
    let fork_id = resource.as_str().strip_prefix("fork:").unwrap().to_string();
    let applied = agent_spreadsheet::tools::fork::apply_staged_change(
        state.clone(),
        agent_spreadsheet::tools::fork::ApplyStagedChangeParams {
            fork_id: fork_id.clone(),
            change_id,
        },
    )
    .await
    .unwrap();
    assert_eq!(applied.ops_applied, 2);
    assert!(
        state
            .fork_registry()
            .unwrap()
            .list_staged_changes(&fork_id)
            .unwrap()
            .is_empty()
    );
    assert_ne!(hash_file_sha256_hex(&fork_path).unwrap(), revision);
    let book = umya_spreadsheet::reader::xlsx::read(&fork_path).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    assert_eq!(sheet.value("A1"), "2");
    assert_eq!(sheet.cell("B1").unwrap().formula(), "A1*3");
}

#[tokio::test]
async fn staged_replay_conflict_is_typed_and_retains_bundle() {
    let (_temp, state, resource, _fork_path, revision) = forked().await;
    let staged = execute_operation_json(
        state.clone(),
        "write",
        request(
            &resource,
            &revision,
            "stage",
            true,
            vec![set_cell(json!(2))],
        ),
    )
    .await
    .unwrap();
    let change_id = staged.data["change_id"].as_str().unwrap().to_string();
    let stage_revision = staged.data["revision_after"].as_str().unwrap();
    execute_operation_json(
        state.clone(),
        "write",
        request(
            &resource,
            stage_revision,
            "apply",
            true,
            vec![set_cell(json!(9))],
        ),
    )
    .await
    .unwrap();
    let fork_id = resource.as_str().strip_prefix("fork:").unwrap().to_string();
    let error = agent_spreadsheet::tools::fork::apply_staged_change(
        state.clone(),
        agent_spreadsheet::tools::fork::ApplyStagedChangeParams {
            fork_id: fork_id.clone(),
            change_id,
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().starts_with("revision conflict:"));
    assert_eq!(
        state
            .fork_registry()
            .unwrap()
            .list_staged_changes(&fork_id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn asp_op_write_requires_explicit_atomic_export_and_never_mutates_bind_silently() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.xlsx");
    let output = temp.path().join("output.xlsx");
    std::fs::copy(fixture(), &input).unwrap();
    let revision = hash_file_sha256_hex(&input).unwrap();
    let payload = json!({
        "expected_revision":revision,
        "mode":"apply",
        "ops":[set_cell(json!(77))]
    });
    let original = hash_file_sha256_hex(&input).unwrap();
    let missing_export = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args([
            "op",
            "write",
            "--bind",
            input.to_str().unwrap(),
            "--json",
            &payload.to_string(),
        ])
        .output()
        .unwrap();
    assert!(!missing_export.status.success());
    assert_eq!(hash_file_sha256_hex(&input).unwrap(), original);

    let applied = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args([
            "op",
            "write",
            "--bind",
            input.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--json",
            &payload.to_string(),
        ])
        .output()
        .unwrap();
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert_eq!(hash_file_sha256_hex(&input).unwrap(), original);
    assert_ne!(hash_file_sha256_hex(&output).unwrap(), original);
}

#[test]
fn every_canonical_write_kind_has_a_closed_union_fixture() {
    let ops = vec![
        set_cell(json!(1)),
        json!({"kind":"clear_range","sheet_name":"Sheet1","target":{"kind":"range","range":"A1:A1"}}),
        json!({"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"range","range":"A1:A1"},"value":"1"}),
        json!({"kind":"replace_in_range","sheet_name":"Sheet1","target":{"kind":"range","range":"A1:A1"},"find":"a","replace":"b"}),
        json!({"kind":"write_matrix","sheet_name":"Sheet1","anchor":"A1","rows":[[{"v":1}]]}),
        json!({"kind":"merge_cells","sheet_name":"Sheet1","target_range":"A1:B1"}),
        json!({"kind":"unmerge_cells","sheet_name":"Sheet1","target_range":"A1:B1"}),
        json!({"kind":"insert_rows","sheet_name":"Sheet1","at_row":2,"count":1}),
        json!({"kind":"delete_rows","sheet_name":"Sheet1","start_row":2,"count":1}),
        json!({"kind":"insert_cols","sheet_name":"Sheet1","at_col":"B","count":1}),
        json!({"kind":"delete_cols","sheet_name":"Sheet1","start_col":"B","count":1}),
        json!({"kind":"rename_sheet","old_name":"Sheet1","new_name":"Data"}),
        json!({"kind":"create_sheet","name":"Data"}),
        json!({"kind":"delete_sheet","name":"Data"}),
        json!({"kind":"copy_range","sheet_name":"Sheet1","src_range":"A1:A1","dest_anchor":"B1"}),
        json!({"kind":"move_range","sheet_name":"Sheet1","src_range":"A1:A1","dest_anchor":"B1"}),
        json!({"kind":"style","sheet_name":"Sheet1","target":{"kind":"range","range":"A1:A1"},"patch":{"font":{"bold":true}}}),
        json!({"kind":"column_size","sheet_name":"Sheet1","target":{"kind":"columns","range":"A:A"},"size":{"kind":"width","width_chars":12.0}}),
        json!({"kind":"freeze_panes","sheet_name":"Sheet1","freeze_rows":1}),
        json!({"kind":"set_zoom","sheet_name":"Sheet1","zoom_percent":100}),
        json!({"kind":"set_gridlines","sheet_name":"Sheet1","show":true}),
        json!({"kind":"set_page_margins","sheet_name":"Sheet1","left":1.0,"right":1.0,"top":1.0,"bottom":1.0}),
        json!({"kind":"set_page_setup","sheet_name":"Sheet1","orientation":"portrait"}),
        json!({"kind":"set_print_area","sheet_name":"Sheet1","range":"A1:B2"}),
        json!({"kind":"set_page_breaks","sheet_name":"Sheet1"}),
        json!({"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"A1:A1","validation":{"kind":"list","formula1":"\"a,b\""}}),
        json!({"kind":"add_conditional_format","sheet_name":"Sheet1","target_range":"A1:A1","rule":{"kind":"expression","formula":"A1>0"}}),
        json!({"kind":"set_conditional_format","sheet_name":"Sheet1","target_range":"A1:A1","rule":{"kind":"expression","formula":"A1>0"}}),
        json!({"kind":"clear_conditional_formats","sheet_name":"Sheet1","target_range":"A1:A1"}),
        json!({"kind":"formula_pattern","sheet_name":"Sheet1","target_range":"A1:A2","anchor_cell":"A1","base_formula":"=B1"}),
        json!({"kind":"replace_in_formulas","sheet_name":"Sheet1","find":"SUM","replace":"AVERAGE"}),
        json!({"kind":"define_name","name":"Rate","refers_to":"Sheet1!$A$1","scope":"workbook"}),
        json!({"kind":"update_name","name":"Rate","refers_to":"Sheet1!$A$2"}),
        json!({"kind":"delete_name","name":"Rate"}),
        json!({"kind":"import_grid","sheet_name":"Sheet1","anchor":"A1","grid":{"sheet":"Sheet1","anchor":"A1","rows":[]}}),
        json!({"kind":"import_csv","sheet_name":"Sheet1","anchor":"A1","csv":"a,b\n1,2\n"}),
        json!({"kind":"append_rows","sheet_name":"Sheet1","region_id":0,"rows":[[{"v":"x"}]]}),
        json!({"kind":"clone_row","sheet_name":"Sheet1","source_row":1,"insert_at":2}),
        json!({"kind":"clone_row_band","sheet_name":"Sheet1","source_rows":"1:2","insert_at":3}),
    ];
    let expected = ops
        .iter()
        .map(|op| op["kind"].as_str().unwrap())
        .collect::<Vec<_>>();
    let instance = json!({
        "resource_id":"fork:fork-test",
        "expected_revision":"revision",
        "mode":"preview",
        "ops":ops,
    });
    let parsed: WriteRequest = serde_json::from_value(instance.clone()).unwrap();
    let input_schema = (agent_spreadsheet::operations::operation_descriptor("write")
        .unwrap()
        .input_schema)();
    jsonschema::validator_for(&input_schema)
        .unwrap()
        .validate(&instance)
        .unwrap();
    assert_eq!(
        parsed.ops.iter().map(|op| op.kind()).collect::<Vec<_>>(),
        expected
    );
    assert!(
        serde_json::from_value::<WriteRequest>(json!({
            "resource_id":"fork:fork-test",
            "expected_revision":"revision",
            "mode":"preview",
            "ops":[{"kind":"not_a_write"}]
        }))
        .is_err()
    );
    let exact_error = agent_spreadsheet::operations::decode_operation("write", json!({
        "resource_id":"fork:fork-test",
        "expected_revision":"revision",
        "mode":"preview",
        "ops":[{"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"range","range":"A1"},"value":"1","unexpected":true}]
    }))
    .unwrap_err();
    assert_eq!(exact_error.error.code, CanonicalErrorCode::InvalidRequest);
    let write_op = &input_schema["$defs"]["WriteOp"];
    assert_eq!(write_op["oneOf"].as_array().unwrap().len(), 39);
    assert!(write_op.get("anyOf").is_none());
    assert_eq!(input_schema["properties"]["ops"]["maxItems"], 128);
    let _ = WriteOpStatus::Previewed;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_same_revision_has_exactly_one_success() {
    let (_temp, state, resource, _fork_path, revision) = forked().await;
    let barrier = Arc::new(tokio::sync::Barrier::new(8));
    let mut tasks = Vec::new();
    for index in 0..8 {
        let state = state.clone();
        let resource = resource.clone();
        let revision = revision.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            execute_operation_json(
                state,
                "write",
                request(
                    &resource,
                    &revision,
                    "apply",
                    true,
                    vec![set_cell(json!(index))],
                ),
            )
            .await
        }));
    }
    let mut successes = 0;
    let mut conflicts = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(response) => {
                assert_eq!(response.data["status"], "applied");
                successes += 1;
            }
            Err(error) => {
                assert_eq!(error.error.code, CanonicalErrorCode::RevisionConflict);
                conflicts += 1;
            }
        }
    }
    assert_eq!((successes, conflicts), (1, 7));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_stage_same_revision_has_exactly_one_success() {
    let (_temp, state, resource, _fork_path, revision) = forked().await;
    let barrier = Arc::new(tokio::sync::Barrier::new(8));
    let mut tasks = Vec::new();
    for index in 0..8 {
        let state = state.clone();
        let resource = resource.clone();
        let revision = revision.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            execute_operation_json(
                state,
                "write",
                request(
                    &resource,
                    &revision,
                    "stage",
                    true,
                    vec![set_cell(json!(index))],
                ),
            )
            .await
        }));
    }
    let mut successes = 0;
    let mut conflicts = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(response) => {
                assert_eq!(response.data["status"], "staged");
                successes += 1;
            }
            Err(error) => {
                assert_eq!(error.error.code, CanonicalErrorCode::RevisionConflict);
                conflicts += 1;
            }
        }
    }
    assert_eq!((successes, conflicts), (1, 7));
}

#[tokio::test]
async fn malformed_late_address_is_invalid_before_non_atomic_effects() {
    let (_temp, state, resource, fork_path, revision) = forked().await;
    let error = execute_operation_json(
        state,
        "write",
        request(
            &resource,
            &revision,
            "apply",
            false,
            vec![
                set_cell(json!(99)),
                json!({"kind":"set_cells","sheet_name":"Sheet1","cells":{"BAD":{"kind":"value","value":1}}}),
            ],
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(error.error.code, CanonicalErrorCode::InvalidRequest);
    assert_eq!(hash_file_sha256_hex(&fork_path).unwrap(), revision);
}

#[tokio::test]
async fn structural_preview_does_not_readd_unchanged_inline_text_cells() {
    let (_temp, state, resource, _fork_path, revision) = forked().await;
    let seeded = execute_operation_json(
        state.clone(),
        "write",
        request(
            &resource,
            &revision,
            "apply",
            true,
            vec![json!({"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":"Header"}}})],
        ),
    )
    .await
    .unwrap();
    let preview = execute_operation_json(
        state,
        "write",
        request(
            &resource,
            seeded.data["revision_after"].as_str().unwrap(),
            "preview",
            true,
            vec![json!({"kind":"insert_rows","sheet_name":"Sheet1","at_row":2,"count":1})],
        ),
    )
    .await
    .unwrap();
    let changes = preview.data["diff"]["changes"].as_array().unwrap();
    assert_eq!(
        preview.data["diff"]["change_count"].as_u64().unwrap(),
        changes.len() as u64
    );
    assert!(!changes.iter().any(|change| {
        change["address"] == "A1"
            && change["old_value"].is_null()
            && change["new_value"] == "Header"
    }));
}

#[tokio::test]
async fn merge_preview_has_structured_effect_and_grid_merges_translate() {
    let (_temp, state, resource, fork_path, revision) = forked().await;
    let preview = execute_operation_json(
        state.clone(),
        "write",
        request(
            &resource,
            &revision,
            "preview",
            true,
            vec![json!({"kind":"merge_cells","sheet_name":"Sheet1","target_range":"A10:B10"})],
        ),
    )
    .await
    .unwrap();
    assert_eq!(preview.data["status"], "previewed");
    assert_eq!(preview.data["diff"]["change_count"], 0);
    assert_eq!(preview.data["diff"]["exact"], false);
    assert_eq!(
        preview.data["diff"]["precision"],
        "exact_cells_names_tables_plus_declared_effects"
    );
    assert_eq!(
        preview.data["diff"]["change_count"].as_u64().unwrap(),
        preview.data["diff"]["changes"].as_array().unwrap().len() as u64
    );
    assert!(
        preview.data["diff"]["effects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["kind"] == "operation_effect")
    );

    let applied = execute_operation_json(
        state.clone(),
        "write",
        request(
            &resource,
            &revision,
            "apply",
            true,
            vec![json!({
                "kind":"import_grid","sheet_name":"Sheet1","anchor":"D5",
                "grid":{"sheet":"Sheet1","anchor":"A1","merges":["A1:B1"],"rows":[{"cells":[{"offset":[0,0],"v":"header"}]}]}
            })],
        ),
    )
    .await
    .unwrap();
    assert_eq!(applied.data["status"], "applied");
    let book = umya_spreadsheet::reader::xlsx::read(&fork_path).unwrap();
    let merges = book.sheet_by_name("Sheet1").ok().unwrap().merge_cells();
    assert!(merges.iter().any(|merge| merge.range() == "D5:E5"));
    assert!(!merges.iter().any(|merge| merge.range() == "A1:B1"));
}

#[tokio::test]
async fn update_name_replaces_definition_and_non_atomic_stage_is_rejected() {
    let (_temp, state, resource, fork_path, revision) = forked().await;
    let stage_error = execute_operation_json(
        state.clone(),
        "write",
        request(
            &resource,
            &revision,
            "stage",
            false,
            vec![set_cell(json!(1))],
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(stage_error.error.code, CanonicalErrorCode::InvalidRequest);

    let applied = execute_operation_json(
        state.clone(),
        "write",
        request(
            &resource,
            &revision,
            "apply",
            true,
            vec![
                json!({"kind":"define_name","name":"Rate","refers_to":"Sheet1!$A$1","scope":"workbook"}),
                json!({"kind":"update_name","name":"Rate","refers_to":"Sheet1!$B$1"}),
            ],
        ),
    )
    .await
    .unwrap();
    assert_eq!(applied.data["status"], "applied");
    let names = execute_operation_json(
        state,
        "named_ranges",
        json!({"resource_id":resource.as_str(),"name_prefix":"Rate"}),
    )
    .await
    .unwrap();
    assert_eq!(names.data["items"].as_array().unwrap().len(), 1);
    assert_eq!(names.data["items"][0]["scope_kind"], "workbook");
    assert_eq!(names.data["items"][0]["scope_sheet_name"], Value::Null);
    assert_eq!(names.data["items"][0]["sheet_name"], Value::Null);
    let book = umya_spreadsheet::reader::xlsx::read(&fork_path).unwrap();
    let address = book
        .defined_names()
        .iter()
        .chain(
            book.sheet_collection()
                .iter()
                .flat_map(|sheet| sheet.defined_names().iter()),
        )
        .find(|name| name.name() == "Rate")
        .map(|name| name.address())
        .expect("Rate definition");
    assert_eq!(address, "Sheet1!$B$1");
    assert!(!address.contains(','));
}

#[test]
fn checked_in_write_response_goldens_cover_closed_status_union() {
    let schema = (agent_spreadsheet::operations::operation_descriptor("write")
        .unwrap()
        .output_schema)();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for status in [
        "previewed",
        "staged",
        "applied",
        "partial",
        "failed",
        "rolled_back",
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/canonical")
            .join(format!("write.{status}.json"));
        let value: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        validator.validate(&value).unwrap();
        assert_eq!(value["data"]["status"], status);
    }
}

#[test]
fn discoverability_write_needs_no_separator() {
    let schema = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args(["schema", "write"])
        .output()
        .unwrap();
    assert!(
        schema.status.success(),
        "{}",
        String::from_utf8_lossy(&schema.stderr)
    );
    let example = assert_cmd::cargo::cargo_bin_cmd!("asp")
        .args(["example", "write"])
        .output()
        .unwrap();
    assert!(
        example.status.success(),
        "{}",
        String::from_utf8_lossy(&example.stderr)
    );
}
