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
    let sheet = book.get_sheet_by_name_mut("Sheet1").unwrap();
    sheet.get_cell_mut("A1").set_value_number(1.0);
    sheet.get_cell_mut("B1").set_formula("A1*2");
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
    book.get_sheet_by_name("Sheet1")
        .unwrap()
        .get_cell(address)
        .map(|cell| cell.get_value().to_string())
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
    let sheet = book.get_sheet_by_name("Sheet1").unwrap();
    ["A1", "B1", "C1", "D1", "A2", "B2", "C2", "D2"]
        .into_iter()
        .map(|address| {
            let cell = sheet.get_cell(address);
            (
                address.to_string(),
                cell.map(|cell| cell.get_value().to_string())
                    .unwrap_or_default(),
                cell.map(|cell| cell.get_formula().to_string())
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
    let sheet = book.get_sheet_by_name_mut("Sheet1").unwrap();
    sheet.get_cell_mut("A1").set_formula("2+3");
    sheet.get_cell_mut("A2").set_formula("\"00123\"");
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
            .get_sheet_by_name("Sheet1")
            .unwrap()
            .get_cell(address)
            .unwrap();
        (
            cell.get_value().to_string(),
            cell.get_cell_value().get_data_type().to_string(),
        )
    };
    assert_eq!(inspect(&live, "B1"), inspect(&replayed, "B1"));
    assert_eq!(inspect(&live, "B1").0, "5");
    assert_eq!(inspect(&live, "B2"), inspect(&replayed, "B2"));
    assert_eq!(inspect(&live, "B2").0, "00123");

    let mut wrong =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(&base), true).unwrap();
    wrong
        .get_sheet_by_name_mut("Sheet1")
        .unwrap()
        .get_cell_mut("D1")
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
        .get_sheet_by_name_mut("Sheet1")
        .unwrap()
        .get_cell_mut("A1")
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
        *book
            .get_sheet_by_name("Sheet1")
            .unwrap()
            .get_style("A1")
            .get_font()
            .unwrap()
            .get_bold()
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
