#![cfg(feature = "recalc-formualizer")]

use agent_spreadsheet::canonical_write::{
    ResidentWriteSession, WriteRequest, apply_staged_durable, checkpoint_durable,
    create_branch_durable, execute_durable_write_on_resident, execute_write_on_bytes,
    execute_write_on_resident, import_legacy_session, recalculate_durable_on_resident,
    recover_durable_resident_session, redo_durable, switch_branch_durable, undo_durable,
};
use agent_spreadsheet::core::resident_storage::native::NativeResidentJournal;
use agent_spreadsheet::core::resident_storage::{ResidentCommitStorage, ResidentTransition};
use serde_json::{Value, json};

fn fixture() -> Vec<u8> {
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.cell_mut("A1").set_value_number(1.0);
    sheet.cell_mut("B1").set_formula("A1*2");
    let mut bytes = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&book, &mut bytes).unwrap();
    bytes
}

fn request(session: &ResidentWriteSession, mode: &str, atomic: bool, ops: Value) -> WriteRequest {
    serde_json::from_value(json!({
        "resource_id": "session:test",
        "expected_revision": session.revision(),
        "mode": mode,
        "atomic": atomic,
        "ops": ops,
    }))
    .unwrap()
}

fn exported_cell(session: &mut ResidentWriteSession, address: &str) -> String {
    let bytes = session.export_bytes().unwrap();
    let book =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(bytes), true).unwrap();
    book.sheet_by_name("Sheet1").ok()
        .unwrap()
        .cell(address)
        .map(|cell| cell.value().to_string())
        .unwrap_or_default()
}

fn normalize_response(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        if object.contains_key("revision_before") {
            object.insert("revision_before".into(), json!("REV_BEFORE"));
        }
        if object.contains_key("revision_after") {
            object.insert("revision_after".into(), json!("REV_AFTER"));
        }
    }
    value
}

fn workbook_cells(bytes: &[u8]) -> Vec<(String, String, String)> {
    let book =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(bytes), true).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    ["A1", "B1", "C1", "D1", "A2", "B2", "C2", "D2"]
        .into_iter()
        .map(|address| {
            let cell = sheet.cell(address);
            (
                address.to_string(),
                cell.map(|cell| cell.value().to_string())
                    .unwrap_or_default(),
                cell.map(|cell| cell.formula().to_string())
                    .unwrap_or_default(),
            )
        })
        .collect()
}

fn assert_common_matches_byte(mode: &str, atomic: bool, ops: Value) {
    let base = fixture();
    let mut resident = ResidentWriteSession::from_bytes("session:test", &base).unwrap();
    let write = request(&resident, mode, atomic, ops);
    let (byte_response, candidate) =
        execute_write_on_bytes(&base, &resident.revision(), write.clone()).unwrap();
    let resident_response = execute_write_on_resident(&mut resident, write).unwrap();
    assert_eq!(
        normalize_response(serde_json::to_value(resident_response).unwrap()),
        normalize_response(serde_json::to_value(byte_response).unwrap())
    );
    if mode == "apply" {
        let expected = candidate.as_deref().unwrap_or(&base);
        let resident_bytes = resident.export_bytes().unwrap();
        assert_eq!(workbook_cells(&resident_bytes), workbook_cells(expected));
    }
}

#[test]
fn common_multidimensional_diff_matches_canonical_row_major_order() {
    let ops = json!([{
        "kind": "write_matrix", "sheet_name": "Sheet1", "anchor": "A1",
        "overwrite_formulas": true,
        "rows": [[{"v":4},{"v":5}],[{"v":6},{"v":7}]]
    }]);
    assert_common_matches_byte("apply", true, ops.clone());
    assert_common_matches_byte("preview", true, ops.clone());
    assert_common_matches_byte("apply", false, ops);
}

#[test]
fn imports_actual_legacy_cli_histories_without_append_order_corruption() {
    fn copy_tree(source: &std::path::Path, target: &std::path::Path) {
        std::fs::create_dir_all(target).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let destination = target.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &destination);
            } else {
                std::fs::copy(entry.path(), destination).unwrap();
            }
        }
    }
    let fixture_root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy-resident");
    assert!(
        fixture_root.is_dir(),
        "checked-in legacy text fixtures are required"
    );
    let cases = [
        (
            "linear-undone",
            "sess_1a079eebed3_e8d46e",
            "stg_001a079eebefd_dd1d2dd9",
            "5",
            "",
        ),
        (
            "divergent",
            "sess_1a079eebf76_481bb6",
            "stg_001a079eebfb5_38131839",
            "5",
            "11",
        ),
    ];
    for (name, session_id, rejected_stage, expected_a1, expected_c1) in cases {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join(".asp/sessions").join(session_id);
        copy_tree(&fixture_root.join(name), &directory);
        std::fs::write(directory.join("base.xlsx"), fixture()).unwrap();
        std::fs::create_dir_all(directory.join("staged")).unwrap();
        std::fs::write(
            directory
                .join("staged")
                .join(format!("{rejected_stage}.json")),
            "{}",
        )
        .unwrap();
        let base_hash = agent_spreadsheet::utils::hash_bytes_sha256_hex(
            &std::fs::read(directory.join("base.xlsx")).unwrap(),
        );
        std::fs::write(
            directory.join("resident-import-frozen.json"),
            serde_json::to_vec(&json!({"frozen":true,"base_sha256":base_hash})).unwrap(),
        )
        .unwrap();
        let imported = import_legacy_session(temp.path(), session_id).unwrap();
        assert_eq!(imported.rejected_staged_ids, vec![rejected_stage]);
        let mut resident = ResidentWriteSession::recover_from_records(
            format!("session:{session_id}"),
            &imported.base_bytes,
            &imported.records,
        )
        .unwrap();
        assert_eq!(exported_cell(&mut resident, "A1"), expected_a1);
        assert_eq!(exported_cell(&mut resident, "C1"), expected_c1);
        let branch_path = directory.join("branches.json");
        let mut branches: Value =
            serde_json::from_slice(&std::fs::read(&branch_path).unwrap()).unwrap();
        let main = branches["branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == "main")
            .unwrap()
            .clone();
        let mut alias = main.clone();
        alias["name"] = json!("alias");
        branches["branches"].as_array_mut().unwrap().push(alias);
        let mut base_branch = main.clone();
        base_branch["name"] = json!("base");
        base_branch["tip_op_id"] = Value::Null;
        base_branch["fork_point"] = Value::Null;
        branches["branches"]
            .as_array_mut()
            .unwrap()
            .push(base_branch.clone());
        std::fs::write(&branch_path, serde_json::to_vec(&branches).unwrap()).unwrap();
        std::fs::write(directory.join("CURRENT_BRANCH"), "alias").unwrap();
        std::fs::write(directory.join("HEAD"), main["tip_op_id"].as_str().unwrap()).unwrap();
        let alias_import = import_legacy_session(temp.path(), session_id).unwrap();
        let history = agent_spreadsheet::core::resident_storage::PortableHistoryState::replay(
            &alias_import.records,
        )
        .unwrap();
        assert_eq!(history.current_branch, "alias");
        assert_eq!(history.branches["alias"], history.branches["main"]);
        assert_eq!(history.branches["base"], None);
        std::fs::write(directory.join("CURRENT_BRANCH"), "base").unwrap();
        std::fs::write(directory.join("HEAD"), "").unwrap();
        let at_base = import_legacy_session(temp.path(), session_id).unwrap();
        assert_eq!(
            agent_spreadsheet::core::resident_storage::PortableHistoryState::replay(
                &at_base.records
            )
            .unwrap()
            .head,
            None
        );
        // Extend only the disposable copy with a deterministic nested branch.
        let events_path = directory.join("events.jsonl");
        let raw = std::fs::read_to_string(&events_path).unwrap();
        let mut nested: agent_spreadsheet::core::events::OpEvent =
            serde_json::from_str(raw.lines().last().unwrap()).unwrap();
        nested.prev_event_hash = nested.event_hash.take();
        nested.op_id = "op_nested_fixture".into();
        nested.parent_id = main["tip_op_id"].as_str().map(str::to_string);
        nested.payload = json!({"anchor":"D1","kind":"transform.write_matrix","rows":[[13]],"sheet_name":"Sheet1"});
        nested.canonical_payload_hash =
            agent_spreadsheet::core::events::canonical_payload_hash(&nested.payload);
        nested.seal();
        std::fs::write(
            &events_path,
            format!("{}{}\n", raw, serde_json::to_string(&nested).unwrap()),
        )
        .unwrap();
        let mut nested_branch = main.clone();
        nested_branch["name"] = json!("nested");
        nested_branch["fork_point"] = main["tip_op_id"].clone();
        nested_branch["tip_op_id"] = json!(nested.op_id);
        branches["branches"]
            .as_array_mut()
            .unwrap()
            .push(nested_branch);
        std::fs::write(&branch_path, serde_json::to_vec(&branches).unwrap()).unwrap();
        std::fs::write(directory.join("CURRENT_BRANCH"), "nested").unwrap();
        std::fs::write(directory.join("HEAD"), &nested.op_id).unwrap();
        let nested_import = import_legacy_session(temp.path(), session_id).unwrap();
        let mut nested_session = ResidentWriteSession::recover_from_records(
            format!("session:{session_id}"),
            &nested_import.base_bytes,
            &nested_import.records,
        )
        .unwrap();
        assert_eq!(exported_cell(&mut nested_session, "D1"), "13");
        // Ordinary zero-event legacy lifecycle, including old empty HEAD spelling.
        base_branch["name"] = json!("main");
        branches["branches"] = json!([base_branch]);
        std::fs::write(&branch_path, serde_json::to_vec(&branches).unwrap()).unwrap();
        std::fs::write(directory.join("events.jsonl"), "").unwrap();
        std::fs::write(directory.join("HEAD"), "").unwrap();
        std::fs::write(directory.join("CURRENT_BRANCH"), "main").unwrap();
        let empty = import_legacy_session(temp.path(), session_id).unwrap();
        assert!(empty.records.is_empty());
        let mut empty_session = ResidentWriteSession::recover_from_records(
            format!("session:{session_id}"),
            &empty.base_bytes,
            &empty.records,
        )
        .unwrap();
        assert_eq!(exported_cell(&mut empty_session, "A1"), "1");
    }
}

#[test]
fn common_preparation_matches_byte_executor_for_multiop_overlap_skip_preview_and_failure() {
    let multi = json!([
        {"kind":"write_matrix","sheet_name":"Sheet1","anchor":"A1","rows":[[{"v":1}]]},
        {"kind":"write_matrix","sheet_name":"Sheet1","anchor":"B1","rows":[[{"v":2},{"v":3}]]}
    ]);
    assert_common_matches_byte("apply", true, multi.clone());
    assert_common_matches_byte("preview", true, multi);
    assert_common_matches_byte(
        "apply",
        true,
        json!([
            {"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":7}}},
            {"kind":"write_matrix","sheet_name":"Sheet1","anchor":"A1","rows":[[{"v":9}]]}
        ]),
    );
    assert_common_matches_byte(
        "apply",
        true,
        json!([
            {"kind":"write_matrix","sheet_name":"Sheet1","anchor":"B1","overwrite_formulas":false,"rows":[[{"v":99}]]},
            {"kind":"write_matrix","sheet_name":"Sheet1","anchor":"C1","rows":[[{"f":"=A1+3"}]]}
        ]),
    );
    assert_common_matches_byte(
        "apply",
        true,
        json!([
            {"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":7}}},
            {"kind":"set_cells","sheet_name":"Missing","cells":{"A1":{"kind":"value","value":8}}}
        ]),
    );
    assert_common_matches_byte(
        "apply",
        false,
        json!([
            {"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":7}}},
            {"kind":"set_cells","sheet_name":"Missing","cells":{"A1":{"kind":"value","value":8}}}
        ]),
    );
}

#[tokio::test]
async fn durable_snapshot_recovery_restores_calculated_copy_value_and_literal_type() {
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.cell_mut("A1").set_formula("2+3");
    sheet.cell_mut("A2").set_formula("\"00123\"");
    let mut base = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&book, &mut base).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(dir.path()).unwrap();
    let mut session = ResidentWriteSession::from_bytes("session:test", &base).unwrap();
    session.recalculate(None).unwrap();
    let write = request(
        &session,
        "apply",
        true,
        json!([
            {"kind":"copy_range","sheet_name":"Sheet1","src_range":"A1","dest_anchor":"B1","include_formulas":false,"include_styles":false},
            {"kind":"copy_range","sheet_name":"Sheet1","src_range":"A2","dest_anchor":"B2","include_formulas":false,"include_styles":false}
        ]),
    );
    execute_durable_write_on_resident(&mut session, &journal, "copy", write)
        .await
        .unwrap();
    let records = journal.load("test").await.unwrap();
    let mut recovered =
        ResidentWriteSession::recover_from_records("session:test", &base, &records).unwrap();
    let live = session.export_bytes().unwrap();
    let replayed = recovered.export_bytes().unwrap();
    let inspect = |bytes: &[u8], address: &str| {
        let book =
            umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(bytes), true).unwrap();
        let cell = book
            .sheet_by_name("Sheet1").ok()
            .unwrap()
            .cell(address)
            .unwrap();
        (
            cell.value().to_string(),
            cell.cell_value().data_type().to_string(),
        )
    };
    assert_eq!(inspect(&live, "B1"), inspect(&replayed, "B1"));
    assert_eq!(inspect(&live, "B1").0, "5");
    assert_eq!(inspect(&live, "B2"), inspect(&replayed, "B2"));
    assert_eq!(inspect(&live, "B2").0, "00123");

    let mut wrong =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(&base), true).unwrap();
    wrong
        .sheet_by_name_mut("Sheet1").ok()
        .unwrap()
        .cell_mut("D1")
        .set_value("wrong base");
    let mut wrong_bytes = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&wrong, &mut wrong_bytes).unwrap();
    assert!(
        ResidentWriteSession::recover_from_records("session:test", &wrong_bytes, &records).is_err()
    );
}

#[tokio::test]
async fn durable_common_write_commits_before_publication_and_recovers_record() {
    let dir = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(dir.path()).unwrap();
    let mut session = ResidentWriteSession::from_bytes("session:test", &fixture()).unwrap();
    let write = request(
        &session,
        "apply",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":5}}}]),
    );
    execute_durable_write_on_resident(&mut session, &journal, "request_one", write)
        .await
        .unwrap();
    assert_eq!(exported_cell(&mut session, "A1"), "5");
    assert_eq!(
        session.diagnostic_workbook().evaluator_counters().ingests,
        1
    );
    let records = journal.load("test").await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].transition, ResidentTransition::Mutation);
    assert_eq!(records[0].state_revision, session.revision());
    assert_eq!(
        records[0].effects[0]["prepared_transaction"]["response"]["results"][0]["index"],
        0
    );

    let stage = request(
        &session,
        "stage",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":8}}}]),
    );
    execute_durable_write_on_resident(&mut session, &journal, "request_stage", stage)
        .await
        .unwrap();
    let records = journal.load("test").await.unwrap();
    let staged_id = records[1].effects[0]["prepared_transaction"]["response"]["change_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut recovered = recover_durable_resident_session("session:test", &fixture(), &journal)
        .await
        .unwrap();
    assert_eq!(exported_cell(&mut recovered, "A1"), "5");
    assert_eq!(recovered.catalog_generation(), 1);
    let revision = recovered.revision();
    assert!(recovered.apply_staged(&staged_id, &revision).is_err());
    assert_eq!(exported_cell(&mut recovered, "A1"), "5");
    let post_restart = request(
        &recovered,
        "apply",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":6}}}]),
    );
    execute_durable_write_on_resident(&mut recovered, &journal, "after_restart", post_restart)
        .await
        .unwrap();
    assert_eq!(exported_cell(&mut recovered, "A1"), "6");
}

#[tokio::test]
async fn duplicate_durable_stage_request_reuses_committed_catalog_result() {
    let dir = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(dir.path()).unwrap();
    let mut session = ResidentWriteSession::from_bytes("session:test", &fixture()).unwrap();
    let stage = request(
        &session,
        "stage",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":4}}}]),
    );
    let first = execute_durable_write_on_resident(&mut session, &journal, "same", stage.clone())
        .await
        .unwrap();
    let second = execute_durable_write_on_resident(&mut session, &journal, "same", stage)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(first).unwrap()["change_id"],
        serde_json::to_value(second).unwrap()["change_id"]
    );
    assert_eq!(session.catalog_generation(), 1);
    assert_eq!(journal.load("test").await.unwrap().len(), 1);
}

#[tokio::test]
async fn durable_request_outcomes_survive_intervening_noop_transitions() {
    let base = fixture();
    let dir = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(dir.path()).unwrap();
    let mut session = ResidentWriteSession::from_bytes("session:test", &base).unwrap();
    let staged_request = request(
        &session,
        "stage",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"D1":{"kind":"value","value":9}}}]),
    );
    let first = execute_durable_write_on_resident(
        &mut session,
        &journal,
        "stable_stage",
        staged_request.clone(),
    )
    .await
    .unwrap();
    checkpoint_durable(&mut session, &journal, "checkpoint", Some("noop"))
        .await
        .unwrap();
    let retried =
        execute_durable_write_on_resident(&mut session, &journal, "stable_stage", staged_request)
            .await
            .unwrap();
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(retried).unwrap()
    );
    assert_eq!(journal.load("test").await.unwrap().len(), 2);
    assert_eq!(
        checkpoint_durable(&mut session, &journal, "checkpoint", Some("noop"))
            .await
            .unwrap(),
        session.revision()
    );
    assert_eq!(journal.load("test").await.unwrap().len(), 2);
}

#[tokio::test]
async fn durable_staged_apply_records_mutation_and_catalog_consumption() {
    let dir = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(dir.path()).unwrap();
    let mut session = ResidentWriteSession::from_bytes("session:test", &fixture()).unwrap();
    let stage = request(
        &session,
        "stage",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":4}}}]),
    );
    let response = execute_durable_write_on_resident(&mut session, &journal, "stage", stage)
        .await
        .unwrap();
    let change_id = serde_json::to_value(response).unwrap()["change_id"]
        .as_str()
        .unwrap()
        .to_string();
    let revision = session.revision();
    apply_staged_durable(&mut session, &journal, "apply_stage", &change_id, &revision)
        .await
        .unwrap();
    assert_eq!(exported_cell(&mut session, "A1"), "4");
    assert_eq!(session.catalog_generation(), 2);
    checkpoint_durable(&mut session, &journal, "after_apply", None)
        .await
        .unwrap();
    let retried =
        apply_staged_durable(&mut session, &journal, "apply_stage", &change_id, &revision)
            .await
            .unwrap();
    assert_eq!(retried.revision_before(), revision);
    let records = journal.load("test").await.unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[1].transition, ResidentTransition::StageApply);
    let mut recovered =
        ResidentWriteSession::recover_from_records("session:test", &fixture(), &records).unwrap();
    assert_eq!(exported_cell(&mut recovered, "A1"), "4");
    assert_eq!(recovered.catalog_generation(), 3);
    assert!(
        recovered
            .apply_staged(&change_id, &recovered.revision())
            .is_err()
    );
}

#[tokio::test]
async fn true_noop_undo_receipt_cannot_mutate_after_intervening_write() {
    let base = fixture();
    let dir = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(dir.path()).unwrap();
    let mut session = ResidentWriteSession::from_bytes("session:test", &base).unwrap();
    let before = session.revision();
    undo_durable(&mut session, &journal, "noop", &base)
        .await
        .unwrap();
    assert_eq!(session.revision(), before);
    let write = request(
        &session,
        "apply",
        true,
        json!([
            {"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":7}}}
        ]),
    );
    execute_durable_write_on_resident(&mut session, &journal, "edit", write)
        .await
        .unwrap();
    let after = session.revision();
    assert_eq!(
        undo_durable(&mut session, &journal, "noop", &base)
            .await
            .unwrap(),
        before
    );
    assert_eq!(session.revision(), after);
    assert_eq!(exported_cell(&mut session, "A1"), "7");
    let mut wrong_book = umya_spreadsheet::new_file();
    wrong_book
        .sheet_by_name_mut("Sheet1").ok()
        .unwrap()
        .cell_mut("A1")
        .set_value_number(999.0);
    let mut wrong_base = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&wrong_book, &mut wrong_base).unwrap();
    assert!(
        undo_durable(&mut session, &journal, "wrong-undo-to-base", &wrong_base)
            .await
            .is_err()
    );
    assert!(
        switch_branch_durable(&mut session, &journal, "wrong-switch", "main", &wrong_base)
            .await
            .is_err()
    );
    assert_eq!(session.revision(), after);
    undo_durable(&mut session, &journal, "undo-to-base", &base)
        .await
        .unwrap();
    assert_eq!(exported_cell(&mut session, "A1"), "1");
    let mut recovered = recover_durable_resident_session("session:test", &base, &journal)
        .await
        .unwrap();
    assert_eq!(exported_cell(&mut recovered, "A1"), "1");
}

#[tokio::test]
async fn noop_value_redo_and_discard_receipts_keep_original_outcomes() {
    use agent_spreadsheet::canonical_write::discard_staged_durable;
    let base = fixture();
    let dir = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(dir.path()).unwrap();
    let mut session = ResidentWriteSession::from_bytes("session:test", &base).unwrap();
    let noop = request(
        &session,
        "apply",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":1}}}]),
    );
    let before = session.revision();
    let original =
        execute_durable_write_on_resident(&mut session, &journal, "value-noop", noop.clone())
            .await
            .unwrap();
    assert_eq!(session.revision(), before);
    assert!(
        !discard_staged_durable(&mut session, &journal, "discard-noop", "missing")
            .await
            .unwrap()
    );
    redo_durable(&mut session, &journal, "redo-noop", &base)
        .await
        .unwrap();
    let edit = request(
        &session,
        "apply",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":8}}}]),
    );
    execute_durable_write_on_resident(&mut session, &journal, "edit", edit)
        .await
        .unwrap();
    let after = session.revision();
    let duplicate =
        execute_durable_write_on_resident(&mut session, &journal, "value-noop", noop.clone())
            .await
            .unwrap();
    assert_eq!(
        serde_json::to_value(original).unwrap(),
        serde_json::to_value(duplicate).unwrap()
    );
    assert_eq!(session.revision(), after);
    assert!(
        execute_durable_write_on_resident(&mut session, &journal, "new-stale", noop)
            .await
            .is_err()
    );
    assert!(
        discard_staged_durable(&mut session, &journal, "discard-noop", "different")
            .await
            .is_err()
    );
    assert!(
        !discard_staged_durable(&mut session, &journal, "discard-noop", "missing")
            .await
            .unwrap()
    );
    undo_durable(&mut session, &journal, "undo", &base)
        .await
        .unwrap();
    let undone = session.revision();
    assert_eq!(
        redo_durable(&mut session, &journal, "redo-noop", &base)
            .await
            .unwrap(),
        before
    );
    assert_eq!(session.revision(), undone);
    let recovered = recover_durable_resident_session("session:test", &base, &journal)
        .await
        .unwrap();
    assert!(recovered.poison_reason().is_none());
}

#[tokio::test]
async fn checkpoint_catalog_preserves_staged_approval_and_survives_restart() {
    use agent_spreadsheet::canonical_write::{delete_checkpoint_durable, list_checkpoints_durable};
    let base = fixture();
    let dir = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(dir.path()).unwrap();
    let mut session = ResidentWriteSession::from_bytes("session:test", &base).unwrap();
    let stage = request(
        &session,
        "stage",
        true,
        json!([
            {"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":7}}}
        ]),
    );
    let staged = execute_durable_write_on_resident(&mut session, &journal, "stage", stage)
        .await
        .unwrap();
    let id = serde_json::to_value(staged).unwrap()["change_id"]
        .as_str()
        .unwrap()
        .to_string();
    let revision = session.revision();
    checkpoint_durable(&mut session, &journal, "cp", Some("base"))
        .await
        .unwrap();
    assert_eq!(session.revision(), revision);
    assert_eq!(session.catalog_generation(), 2);
    apply_staged_durable(&mut session, &journal, "apply", &id, &revision)
        .await
        .unwrap();
    let mut recovered = recover_durable_resident_session("session:test", &base, &journal)
        .await
        .unwrap();
    let checkpoints = list_checkpoints_durable(&recovered, &journal)
        .await
        .unwrap();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].id, "cp");
    assert_eq!(checkpoints[0].state_revision, revision);
    agent_spreadsheet::canonical_write::restore_checkpoint_durable(
        &mut recovered,
        &journal,
        "restore",
        "cp",
    )
    .await
    .unwrap();
    assert_eq!(exported_cell(&mut recovered, "A1"), "1");
    let revision = recovered.revision();
    delete_checkpoint_durable(&mut recovered, &journal, "delete", "cp")
        .await
        .unwrap();
    assert_eq!(recovered.revision(), revision);
    assert!(
        list_checkpoints_durable(&recovered, &journal)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn journaled_dirty_calculation_retains_evaluator_without_serializing() {
    let base = fixture();
    let dir = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(dir.path()).unwrap();
    let mut session = ResidentWriteSession::from_bytes("session:test", &base).unwrap();
    for value in 2..6 {
        let write = request(
            &session,
            "apply",
            true,
            json!([
                {"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":value}}}
            ]),
        );
        execute_durable_write_on_resident(&mut session, &journal, &format!("write-{value}"), write)
            .await
            .unwrap();
        recalculate_durable_on_resident(&mut session, &journal, &format!("calc-{value}"), None)
            .await
            .unwrap();
        let counters = session.diagnostic_workbook().evaluator_counters();
        assert_eq!(counters.constructions, 1);
        assert_eq!(counters.ingests, 1);
        assert_eq!(counters.rebuilds, 0);
        assert_eq!(session.diagnostic_workbook().serialization_count(), 0);
        let current = session.range_values("Sheet1", "B1").unwrap();
        assert!(
            matches!(current[0].rows.as_ref().unwrap()[0][0], Some(agent_spreadsheet::model::CellValue::Number(number)) if number == (value * 2) as f64)
        );
    }
    let records = journal.load("test").await.unwrap();
    for record in records
        .iter()
        .filter(|r| r.transition == ResidentTransition::CalculationPublish)
    {
        let proof = record
            .effects
            .iter()
            .find_map(|effect| effect.get("calculation_proof"))
            .unwrap();
        assert!(proof.get("bytes").is_none());
    }
    assert_eq!(exported_cell(&mut session, "B1"), "10");
    assert_eq!(session.diagnostic_workbook().serialization_count(), 1);
    assert!(ResidentWriteSession::recover_from_records("session:other", &base, &records).is_err());
    let mut wrong = fixture();
    wrong.push(0);
    assert!(
        undo_durable(&mut session, &journal, "wrong-base", &wrong)
            .await
            .is_err()
    );
    let structural = request(
        &session,
        "apply",
        true,
        json!([{"kind":"create_sheet","name":"Added"}]),
    );
    execute_durable_write_on_resident(&mut session, &journal, "structural", structural)
        .await
        .unwrap();
    recalculate_durable_on_resident(&mut session, &journal, "cold", None)
        .await
        .unwrap();
    assert_eq!(
        session.diagnostic_workbook().evaluator_counters().rebuilds,
        1
    );
    assert!(session.diagnostic_workbook().serialization_count() > 1);
}

#[tokio::test]
async fn portable_history_supports_calculate_write_undo_redo_and_divergent_branches() {
    let base = fixture();
    let dir = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(dir.path()).unwrap();
    let mut session = ResidentWriteSession::from_bytes("session:test", &base).unwrap();
    let first = request(
        &session,
        "apply",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":5}}}]),
    );
    execute_durable_write_on_resident(&mut session, &journal, "first", first)
        .await
        .unwrap();
    recalculate_durable_on_resident(&mut session, &journal, "calculate", None)
        .await
        .unwrap();
    let second = request(
        &session,
        "apply",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":6}}}]),
    );
    execute_durable_write_on_resident(&mut session, &journal, "second", second)
        .await
        .unwrap();
    undo_durable(&mut session, &journal, "undo", &base)
        .await
        .unwrap();
    assert_eq!(exported_cell(&mut session, "A1"), "5");
    redo_durable(&mut session, &journal, "redo", &base)
        .await
        .unwrap();
    assert_eq!(exported_cell(&mut session, "A1"), "6");
    undo_durable(&mut session, &journal, "undo_again", &base)
        .await
        .unwrap();
    create_branch_durable(&mut session, &journal, "branch", "feature")
        .await
        .unwrap();
    switch_branch_durable(&mut session, &journal, "switch_feature", "feature", &base)
        .await
        .unwrap();
    let feature = request(
        &session,
        "apply",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":7}}}]),
    );
    execute_durable_write_on_resident(&mut session, &journal, "feature_edit", feature)
        .await
        .unwrap();
    undo_durable(&mut session, &journal, "undo_feature", &base)
        .await
        .unwrap();
    redo_durable(&mut session, &journal, "redo_feature", &base)
        .await
        .unwrap();
    let mut forged = journal.load("test").await.unwrap();
    let main_child = forged.last().unwrap().resulting_branches["main"].clone();
    let last = forged.last_mut().unwrap();
    last.resulting_head = main_child.clone();
    last.effects = vec![json!({"target_head":main_child})];
    assert!(
        agent_spreadsheet::core::resident_storage::PortableHistoryState::replay(&forged).is_err()
    );
    switch_branch_durable(&mut session, &journal, "switch_main", "main", &base)
        .await
        .unwrap();
    assert_eq!(exported_cell(&mut session, "A1"), "6");
    let mut recovered = recover_durable_resident_session("session:test", &base, &journal)
        .await
        .unwrap();
    assert_eq!(exported_cell(&mut recovered, "A1"), "6");
    let after_restart = request(
        &recovered,
        "apply",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"D10":{"kind":"value","value":8}}}]),
    );
    execute_durable_write_on_resident(&mut recovered, &journal, "after_restart", after_restart)
        .await
        .unwrap();
    assert_eq!(exported_cell(&mut recovered, "D10"), "8");
}

#[tokio::test]
async fn branch_at_is_atomic_validated_labeled_and_recoverable() {
    use agent_spreadsheet::canonical_write::create_branch_at_durable;
    use agent_spreadsheet::core::resident_storage::PortableHistoryState;
    let base = fixture();
    let dir = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(dir.path()).unwrap();
    let mut session = ResidentWriteSession::from_bytes("session:test", &base).unwrap();
    for (id, value) in [("one", 5), ("two", 6)] {
        let write = request(&session, "apply", true, json!([
            {"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":value}}}
        ]));
        execute_durable_write_on_resident(&mut session, &journal, id, write).await.unwrap();
    }
    let before = journal.load("test").await.unwrap();
    let target = before[0].commit_id.clone();
    let revision = session.revision();
    let counters = session.diagnostic_workbook().serialization_count();
    assert!(create_branch_at_durable(&mut session, &journal, "bad", "bad", Some("missing"), None).await.is_err());
    assert_eq!(journal.load("test").await.unwrap().len(), before.len());
    assert_eq!(session.revision(), revision);
    create_branch_at_durable(&mut session, &journal, "branch_at", "past", Some(&target), Some("retained label")).await.unwrap();
    create_branch_at_durable(&mut session, &journal, "base_branch", "base_copy", Some("base"), None).await.unwrap();
    assert_eq!(session.diagnostic_workbook().serialization_count(), counters);
    let records = journal.load("test").await.unwrap();
    let history = PortableHistoryState::replay(&records).unwrap();
    assert_eq!(history.head, before.last().unwrap().resulting_head);
    assert_eq!(history.current_branch, "main");
    assert_eq!(history.branches["past"], Some(target.clone()));
    assert_eq!(history.branches["base_copy"], None);
    assert_eq!(history.branch_labels["past"], "retained label");
    assert!(create_branch_at_durable(&mut session, &journal, "branch_at", "past", Some(&target), Some("changed")).await.is_err());
    let count = records.len();
    create_branch_at_durable(&mut session, &journal, "branch_at", "past", Some(&target), Some("retained label")).await.unwrap();
    assert_eq!(journal.load("test").await.unwrap().len(), count);
    let mut recovered = recover_durable_resident_session("session:test", &base, &journal).await.unwrap();
    create_branch_at_durable(&mut recovered, &journal, "branch_at", "past", Some(&target), Some("retained label")).await.unwrap();
    switch_branch_durable(&mut recovered, &journal, "switch_past", "past", &base).await.unwrap();
    assert_eq!(exported_cell(&mut recovered, "A1"), "5");
    let mut forged = records;
    forged.last_mut().unwrap().effects[0]["branch_target"] = json!("missing");
    assert!(PortableHistoryState::replay(&forged).is_err());
}

#[tokio::test]
async fn durable_atomic_failure_preserves_apply_rollback_contract_and_writes_no_record() {
    let dir = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(dir.path()).unwrap();
    let mut session = ResidentWriteSession::from_bytes("session:test", &fixture()).unwrap();
    let write = request(
        &session,
        "apply",
        true,
        json!([{"kind":"style","sheet_name":"Missing","target":{"kind":"range","range":"A1"},"patch":{"font":{"bold":true}}}]),
    );
    let response = execute_durable_write_on_resident(&mut session, &journal, "failure", write)
        .await
        .unwrap();
    let response = serde_json::to_value(response).unwrap();
    assert_eq!(response["status"], "rolled_back");
    assert_eq!(response["mode"], "apply");
    assert_eq!(response["diff"]["change_count"], 0);
    assert_eq!(journal.load("test").await.unwrap().len(), 0);
}

#[tokio::test]
async fn durable_non_atomic_record_contains_exact_indexed_successes_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(dir.path()).unwrap();
    let mut session = ResidentWriteSession::from_bytes("session:test", &fixture()).unwrap();
    let write = request(
        &session,
        "apply",
        false,
        json!([
            {"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":7}}},
            {"kind":"set_cells","sheet_name":"Missing","cells":{"A1":{"kind":"value","value":8}}}
        ]),
    );
    let response = execute_durable_write_on_resident(&mut session, &journal, "partial", write)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(response).unwrap()["status"], "partial");
    let records = journal.load("test").await.unwrap();
    let persisted = &records[0].effects[0]["prepared_transaction"]["response"];
    assert_eq!(persisted["status"], "partial");
    assert_eq!(persisted["results"][0]["status"], "applied");
    assert_eq!(persisted["results"][1]["status"], "failed");
    let mut recovered =
        ResidentWriteSession::recover_from_records("session:test", &fixture(), &records).unwrap();
    assert_eq!(exported_cell(&mut recovered, "A1"), "7");
}

#[test]
fn preview_is_pure_and_common_apply_retains_engine() {
    let mut session = ResidentWriteSession::from_bytes("session:test", &fixture()).unwrap();
    session.recalculate(None).unwrap();
    let initial = session.revision();
    let preview = request(
        &session,
        "preview",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":5}}}]),
    );
    let response = execute_write_on_resident(&mut session, preview).unwrap();
    assert_eq!(
        serde_json::to_value(response).unwrap()["status"],
        "previewed"
    );
    assert_eq!(session.revision(), initial);
    assert_eq!(exported_cell(&mut session, "A1"), "1");

    let apply = request(
        &session,
        "apply",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":"00123"}}}]),
    );
    let stale_competitor = apply.clone();
    execute_write_on_resident(&mut session, apply).unwrap();
    assert!(execute_write_on_resident(&mut session, stale_competitor).is_err());
    assert_eq!(
        session.diagnostic_workbook().evaluator_counters().ingests,
        1
    );
    session.recalculate(None).unwrap();
    assert_eq!(
        session.diagnostic_workbook().evaluator_counters().ingests,
        1
    );
    assert_eq!(exported_cell(&mut session, "B1"), "246");
}

#[test]
fn stage_catalog_is_separate_and_approvals_bind_workbook_state() {
    let mut session = ResidentWriteSession::from_bytes("session:test", &fixture()).unwrap();
    let revision = session.revision();
    let staged_one = request(
        &session,
        "stage",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":2}}}]),
    );
    let first = execute_write_on_resident(&mut session, staged_one).unwrap();
    let first_json = serde_json::to_value(&first).unwrap();
    let first_id = first_json["change_id"].as_str().unwrap().to_string();
    assert_eq!(session.revision(), revision);

    let staged_two = request(
        &session,
        "stage",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":3}}}]),
    );
    let second = execute_write_on_resident(&mut session, staged_two).unwrap();
    let second_id = serde_json::to_value(second).unwrap()["change_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(session.revision(), revision);
    assert_eq!(session.catalog_generation(), 2);

    session.apply_staged(&first_id, &revision).unwrap();
    assert_eq!(exported_cell(&mut session, "A1"), "2");
    assert!(
        session
            .apply_staged(&second_id, &session.revision())
            .is_err()
    );

    let stale = request(
        &session,
        "stage",
        true,
        json!([{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":4}}}]),
    );
    let stale_response = execute_write_on_resident(&mut session, stale).unwrap();
    let stale_id = serde_json::to_value(stale_response).unwrap()["change_id"]
        .as_str()
        .unwrap()
        .to_string();
    let approval_revision = session.revision();
    session.recalculate(None).unwrap();
    assert!(session.apply_staged(&stale_id, &approval_revision).is_err());
}

#[test]
fn atomic_failure_rolls_back_and_non_atomic_reports_exact_prefix() {
    let mut session = ResidentWriteSession::from_bytes("session:test", &fixture()).unwrap();
    let before = session.revision();
    let ops = json!([
        {"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":7}}},
        {"kind":"set_cells","sheet_name":"Missing","cells":{"A1":{"kind":"value","value":8}}}
    ]);
    let atomic = request(&session, "apply", true, ops.clone());
    let response = execute_write_on_resident(&mut session, atomic).unwrap();
    let value = serde_json::to_value(response).unwrap();
    assert_eq!(value["status"], "rolled_back");
    assert_eq!(session.revision(), before);
    assert_eq!(exported_cell(&mut session, "A1"), "1");

    let non_atomic = request(&session, "apply", false, ops);
    let response = execute_write_on_resident(&mut session, non_atomic).unwrap();
    let value = serde_json::to_value(response).unwrap();
    assert_eq!(value["status"], "partial");
    assert_eq!(value["ops_applied"], 1);
    assert_eq!(value["results"][0]["status"], "applied");
    assert_eq!(value["results"][1]["status"], "failed");
    assert_eq!(exported_cell(&mut session, "A1"), "7");
}

#[test]
fn non_atomic_failed_value_edit_does_not_invalidate_style_only_success() {
    let mut session = ResidentWriteSession::from_bytes("session:test", &fixture()).unwrap();
    session.recalculate(None).unwrap();
    let before = session.diagnostic_workbook().evaluator_counters();
    let write = request(
        &session,
        "apply",
        false,
        json!([
            {"kind":"style","sheet_name":"Sheet1","target":{"kind":"range","range":"A1"},"patch":{"font":{"bold":true}}},
            {"kind":"set_cells","sheet_name":"Missing","cells":{"A1":{"kind":"value","value":99}}}
        ]),
    );
    let response =
        serde_json::to_value(execute_write_on_resident(&mut session, write).unwrap()).unwrap();
    assert_eq!(response["status"], "partial");
    assert_eq!(response["results"][0]["status"], "applied");
    assert_eq!(response["results"][1]["status"], "failed");
    assert!(matches!(
        session.diagnostic_workbook().calculation_stamp(),
        agent_spreadsheet::recalc::CalculationStamp::Current { .. }
    ));
    let exported = session.export_bytes().unwrap();
    let book =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(exported), true).unwrap();
    assert!(
        book
            .sheet_by_name("Sheet1").ok()
            .unwrap()
            .style("A1")
            .font()
            .unwrap()
            .bold()
    );
    session.recalculate(None).unwrap();
    let after = session.diagnostic_workbook().evaluator_counters();
    assert_eq!(after.constructions, before.constructions);
    assert_eq!(after.ingests, before.ingests);
    assert_eq!(after.rebuilds, before.rebuilds);
}

#[test]
fn structural_fallback_rebuilds_but_style_fallback_preserves_evaluator() {
    let mut session = ResidentWriteSession::from_bytes("session:test", &fixture()).unwrap();
    session.recalculate(None).unwrap();
    let style = request(
        &session,
        "apply",
        true,
        json!([{"kind":"style","sheet_name":"Sheet1","target":{"kind":"range","range":"A1"},"patch":{"font":{"bold":true}}}]),
    );
    execute_write_on_resident(&mut session, style).unwrap();
    session.recalculate(None).unwrap();
    assert_eq!(
        session.diagnostic_workbook().evaluator_counters().ingests,
        1
    );

    let structural = request(
        &session,
        "apply",
        true,
        json!([{"kind":"create_sheet","name":"Added"}]),
    );
    execute_write_on_resident(&mut session, structural).unwrap();
    session.recalculate(None).unwrap();
    assert_eq!(
        session.diagnostic_workbook().evaluator_counters().rebuilds,
        1
    );
}

struct LostNativeAcknowledgement {
    inner: NativeResidentJournal,
    refuse_reconciliation: bool,
    committed: std::cell::Cell<bool>,
}
#[async_trait::async_trait(?Send)]
impl ResidentCommitStorage for LostNativeAcknowledgement {
    fn outcome_retention(&self) -> agent_spreadsheet::core::resident_storage::OutcomeRetention { self.inner.outcome_retention() }
    async fn load(&self, session_id: &str) -> anyhow::Result<Vec<agent_spreadsheet::core::resident_storage::PreparedResidentCommit>> { self.inner.load(session_id).await }
    async fn commit(&self, prepared: &agent_spreadsheet::core::resident_storage::PreparedResidentCommit) -> anyhow::Result<agent_spreadsheet::core::resident_storage::DurableCommitOutcome> {
        self.inner.commit(prepared).await?;
        self.committed.set(true);
        anyhow::bail!("injected acknowledgement loss after durable commit")
    }
    async fn reconcile(&self, session_id: &str, request_id: &str, fingerprint: &str) -> anyhow::Result<agent_spreadsheet::core::resident_storage::ReconcileOutcome> {
        if self.refuse_reconciliation && self.committed.get() { anyhow::bail!("injected reconciliation failure"); }
        self.inner.reconcile(session_id,request_id,fingerprint).await
    }
}

#[tokio::test]
async fn canonical_same_record_outcomes_reconcile_lost_acknowledgements_and_fence_unknowns() {
    use agent_spreadsheet::operations::{CanonicalErrorCode,decode_operation};
    for refuse_reconciliation in [false,true] {
        let (_directory,runtime,base)=canonical_runtime().await;
        let mut runtime=agent_spreadsheet::session::ResidentSessionRuntime::new(runtime.owner,LostNativeAcknowledgement {inner:runtime.storage,refuse_reconciliation,committed:std::cell::Cell::new(false)},runtime.config,serde_json::from_value(json!("session:test")).unwrap()).unwrap();
        let mut payload=canonical_write_payload(runtime.owner.revision(),9,"apply");
        payload["resource_id"]=json!("session:test");
        let result=runtime.execute("lost-ack",decode_operation("write",payload.clone()).unwrap()).await;
        let records=runtime.storage.inner.load("test").await.unwrap();
        assert_eq!(records.len(),1);
        let original=records[0].effects.iter().find_map(|e|e.pointer("/canonical_outcome/response")).cloned().unwrap();
        if refuse_reconciliation {
            assert_eq!(result.unwrap_err().error.code,CanonicalErrorCode::OutcomeUnknown);
            assert!(runtime.owner.poison_reason().is_some());
            let error=runtime.execute("read",decode_operation("describe_workbook",json!({"resource_id":"session:test"})).unwrap()).await.unwrap_err();
            assert_eq!(error.error.code,CanonicalErrorCode::RecoveryRequired);
            let status=runtime.execute("status",decode_operation("session_history",json!({"resource_id":"session:test","action":"status"})).unwrap()).await.unwrap();
            assert_eq!(status.data["health"],"poisoned");
            assert!(status.revision_id.is_none());
            assert!(status.data["revision_id"].is_null());
            assert_eq!(status.data["poisoned_request_id"],"lost-ack");
            let outcome=runtime.execute("query",decode_operation("session_history",json!({"resource_id":"session:test","action":"outcome","request_id":"lost-ack"})).unwrap()).await.unwrap();
            assert_eq!(outcome.data["state"],"unknown");
            assert!(outcome.data["response"].is_null());
            let owner=recover_durable_resident_session("session:test",&base,&runtime.storage.inner).await.unwrap();
            let mut recovered=agent_spreadsheet::session::ResidentSessionRuntime::new(owner,runtime.storage.inner,runtime.config,serde_json::from_value(json!("session:test")).unwrap()).unwrap();
            let retry=recovered.execute("lost-ack",decode_operation("write",payload).unwrap()).await.unwrap();
            assert_eq!(serde_json::to_value(retry).unwrap(),original);
        } else {
            assert_eq!(serde_json::to_value(result.unwrap()).unwrap(),original);
            assert!(!runtime.owner.poison_reason().is_some());
        }
    }
}

async fn canonical_runtime() -> (tempfile::TempDir, agent_spreadsheet::session::ResidentSessionRuntime<NativeResidentJournal>, Vec<u8>) {
    let directory = tempfile::tempdir().unwrap();
    let bytes = fixture();
    let source = directory.path().join("base.xlsx");
    std::fs::write(&source, &bytes).unwrap();
    let (state, _) = agent_spreadsheet::runtime::stateless::StatelessRuntime.open_state_for_file(&source).await.unwrap();
    let owner = ResidentWriteSession::from_bytes("session:test", &bytes).unwrap();
    let storage = NativeResidentJournal::open(&directory.path().join("journal")).unwrap();
    let runtime = agent_spreadsheet::session::ResidentSessionRuntime::new(owner, storage, state.config(), serde_json::from_value(json!("session:test")).unwrap()).unwrap();
    (directory, runtime, bytes)
}
async fn runtime_call(runtime: &mut agent_spreadsheet::session::ResidentSessionRuntime<NativeResidentJournal>, request_id: &str, name: &str, mut payload: Value) -> agent_spreadsheet::operations::CanonicalResponse {
    payload["resource_id"] = json!("session:test");
    let operation = agent_spreadsheet::operations::decode_operation(name, payload).unwrap();
    runtime.execute(request_id, operation).await.unwrap()
}
fn canonical_write_payload(revision: String, value: u64, mode: &str) -> Value {
    json!({"expected_revision":revision,"mode":mode,"atomic":true,"label":"retained label","ops":[{"kind":"write_matrix","sheet_name":"Sheet1","anchor":"A1","rows":[[{"v":value}]]}]})
}

#[tokio::test]
async fn canonical_dispatch_uses_one_retained_owner_and_original_same_record_outcomes() {
    let (_directory, mut runtime, base) = canonical_runtime().await;
    let mut saved = Vec::new();
    for n in 2..5 {
        let payload = canonical_write_payload(runtime.owner.revision(), n, "apply");
        let response = runtime_call(&mut runtime, &format!("write-{n}"), "write", payload.clone()).await;
        saved.push((format!("write-{n}"),"write",payload,response));
        let payload = json!({"expected_revision":runtime.owner.revision()});
        let response = runtime_call(&mut runtime, &format!("calc-{n}"), "recalculate", payload.clone()).await;
        assert_eq!(response.data["state"], "clean");
        saved.push((format!("calc-{n}"),"recalculate",payload,response));
        let read = runtime_call(&mut runtime,"read","read_cells",json!({"sheet_name":"Sheet1","selection":{"kind":"range","ranges":["A1:B1"]},"format":"values"})).await;
        assert_eq!(read.data["blocks"][0]["payload"]["values"][0][1], json!((n*2) as f64));
        let describe = runtime_call(&mut runtime,"describe","describe_workbook",json!({"include_paths":true})).await;
        assert!(describe.data["metadata"]["bytes"].is_null());
        assert!(describe.data["paths"]["internal"].is_null());
        assert!(describe.data["paths"]["client"].is_null());
        runtime_call(&mut runtime,"map","formula_map",json!({"sheet_name":"Sheet1"})).await;
    }
    let history = runtime_call(&mut runtime,"history","get_changes",json!({"view":{"kind":"operations","offset":0,"limit":2}})).await;
    assert_eq!(history.data["total"],6);
    assert_eq!(history.data["next_offset"],2);
    assert!(history.data["operations"][0]["timestamp"].is_null());
    let head = runtime.owner.revision();
    let count = runtime.storage.load("test").await.unwrap().len();
    for (id, name, payload, original) in &saved {
        let retry = runtime_call(&mut runtime,id,name,payload.clone()).await;
        assert_eq!(serde_json::to_value(retry).unwrap(),serde_json::to_value(original).unwrap());
        assert_eq!(runtime.owner.revision(),head);
    }
    assert_eq!(runtime.storage.load("test").await.unwrap().len(),count);
    assert_eq!(runtime.owner.diagnostic_workbook().serialization_count(),0);
    assert_eq!(runtime.owner.diagnostic_workbook().evaluator_counters().ingests,1);
    assert_eq!(runtime.owner.diagnostic_workbook().evaluator_counters().evaluations,3);
    let mut different=saved[0].2.clone();
    different["resource_id"]=json!("session:test");
    different["label"]=json!("different canonical input");
    assert!(runtime.execute(&saved[0].0,agent_spreadsheet::operations::decode_operation("write",different).unwrap()).await.is_err());
    let records=runtime.storage.load("test").await.unwrap();
    for record in &records { assert!(record.effects.iter().any(|e|e.get("canonical_outcome").is_some())); }
    let mut forged=records[0].clone();
    let outcome=forged.effects.iter_mut().find_map(|e|e.get_mut("canonical_outcome")).unwrap();
    outcome["response"]["data"]["ops_applied"]=json!(999);
    let other=tempfile::tempdir().unwrap();
    let journal=NativeResidentJournal::open(other.path()).unwrap();
    assert!(journal.commit(&forged).await.unwrap_err().to_string().contains("canonical write outcome"));
    assert!(journal.load("test").await.unwrap().is_empty());
    let owner=recover_durable_resident_session("session:test",&base,&runtime.storage).await.unwrap();
    let mut recovered=agent_spreadsheet::session::ResidentSessionRuntime::new(owner,runtime.storage,runtime.config,serde_json::from_value(json!("session:test")).unwrap()).unwrap();
    let (_,name,payload,original)=&saved[1];
    let retry=runtime_call(&mut recovered,&saved[1].0,name,payload.clone()).await;
    assert_eq!(serde_json::to_value(retry).unwrap(),serde_json::to_value(original).unwrap());
    assert_eq!(recovered.owner.diagnostic_workbook().evaluator_counters().evaluations,0);
}

#[tokio::test]
async fn canonical_history_actions_and_outcomes_share_the_registry() {
    let (_directory,mut runtime,_base)=canonical_runtime().await;
    let mut saved=Vec::new();
    let payload=json!({"action":"undo","expected_revision":runtime.owner.revision()});
    let response=runtime_call(&mut runtime,"root-undo","session_history",payload.clone()).await;
    saved.push(("root-undo",payload,response));
    let payload=canonical_write_payload(runtime.owner.revision(),3,"apply");
    runtime_call(&mut runtime,"first","write",payload).await;
    let first=runtime.storage.load("test").await.unwrap().last().unwrap().resulting_head.clone().unwrap();
    let payload=json!({"action":"create_branch","expected_revision":runtime.owner.revision(),"name":"feature"});
    let response=runtime_call(&mut runtime,"branch","session_history",payload.clone()).await;
    saved.push(("branch",payload,response));
    let payload=canonical_write_payload(runtime.owner.revision(),4,"apply");
    runtime_call(&mut runtime,"main-edit","write",payload).await;
    for action in ["undo","redo"] {
        let payload=json!({"action":action,"expected_revision":runtime.owner.revision()});
        let response=runtime_call(&mut runtime,action,"session_history",payload.clone()).await;
        saved.push((action,payload,response));
    }
    for (id,branch) in [("feature-switch","feature"),("main-switch","main")] {
        let payload=json!({"action":"switch_branch","expected_revision":runtime.owner.revision(),"name":branch});
        let response=runtime_call(&mut runtime,id,"session_history",payload.clone()).await;
        saved.push((id,payload,response));
        if branch=="feature" {let payload=canonical_write_payload(runtime.owner.revision(),9,"apply");runtime_call(&mut runtime,"feature-edit","write",payload).await;}
    }
    let payload=json!({"action":"checkout","expected_revision":runtime.owner.revision(),"target_commit_id":first});
    let response=runtime_call(&mut runtime,"checkout","session_history",payload.clone()).await;
    saved.push(("checkout",payload,response));
    let head=runtime.owner.revision();
    let count=runtime.storage.load("test").await.unwrap().len();
    for (id,payload,original) in saved {
        let retry=runtime_call(&mut runtime,id,"session_history",payload).await;
        assert_eq!(serde_json::to_value(retry).unwrap(),serde_json::to_value(&original).unwrap());
        assert_eq!(runtime.owner.revision(),head);
        let outcome=runtime_call(&mut runtime,"query","session_history",json!({"action":"outcome","request_id":id})).await;
        assert_eq!(outcome.data["state"],"committed");
        assert_eq!(outcome.data["response"],serde_json::to_value(original).unwrap());
    }
    let list=runtime_call(&mut runtime,"list-history","session_history",json!({"action":"list","limit":2})).await;
    assert_eq!(list.data["records"].as_array().unwrap().len(),2);
    assert_eq!(list.data["total"],count);
    assert!(list.data["records"][0].get("effects").is_none());
    let status=runtime_call(&mut runtime,"status","session_history",json!({"action":"status"})).await;
    assert_eq!(status.data["health"],"usable");
    assert_eq!(status.data["revision_id"],head);
    assert_eq!(runtime.storage.load("test").await.unwrap().len(),count);
}

#[tokio::test]
async fn canonical_catalog_outcomes_survive_later_catalog_and_document_changes() {
    let (_directory,mut runtime,_base)=canonical_runtime().await;
    let mut saved=Vec::new();
    let payload=json!({"action":"create","expected_revision":runtime.owner.revision(),"label":"original"});
    let response=runtime_call(&mut runtime,"cp","checkpoint",payload.clone()).await;
    saved.push(("cp","checkpoint",payload,response));
    let payload=canonical_write_payload(runtime.owner.revision(),7,"stage");
    let staged=runtime_call(&mut runtime,"stage","write",payload.clone()).await;
    let id=staged.data["change_id"].as_str().unwrap().to_owned();
    saved.push(("stage","write",payload,staged));
    let list=runtime_call(&mut runtime,"list","staged_change",json!({"action":"list"})).await;
    assert_eq!(list.data["staged_changes"][0]["label"],"retained label");
    let payload=json!({"action":"apply","expected_revision":runtime.owner.revision(),"change_id":id});
    let response=runtime_call(&mut runtime,"apply-stage","staged_change",payload.clone()).await;
    saved.push(("apply-stage","staged_change",payload,response));
    let payload=json!({"action":"restore","expected_revision":runtime.owner.revision(),"checkpoint_id":"cp"});
    let response=runtime_call(&mut runtime,"restore","checkpoint",payload.clone()).await;
    saved.push(("restore","checkpoint",payload,response));
    let payload=json!({"action":"delete","expected_revision":runtime.owner.revision(),"checkpoint_id":"cp"});
    let response=runtime_call(&mut runtime,"delete","checkpoint",payload.clone()).await;
    saved.push(("delete","checkpoint",payload,response));
    let payload=json!({"action":"discard","expected_revision":runtime.owner.revision(),"change_id":"missing"});
    let response=runtime_call(&mut runtime,"discard","staged_change",payload.clone()).await;
    assert_eq!(response.data["discarded"],false);
    saved.push(("discard","staged_change",payload,response));
    let payload=json!({"action":"create","expected_revision":runtime.owner.revision(),"label":"later"});
    runtime_call(&mut runtime,"cp-later","checkpoint",payload).await;
    let payload=json!({"expected_revision":runtime.owner.revision()});
    runtime_call(&mut runtime,"calc-later","recalculate",payload).await;
    let count=runtime.storage.load("test").await.unwrap().len();
    let head=runtime.owner.revision();
    for (id,name,payload,original) in saved {
        let response=runtime_call(&mut runtime,id,name,payload).await;
        assert_eq!(serde_json::to_value(response).unwrap(),serde_json::to_value(original).unwrap(),"{id}");
        assert_eq!(runtime.owner.revision(),head);
    }
    assert_eq!(runtime.storage.load("test").await.unwrap().len(),count);
}
