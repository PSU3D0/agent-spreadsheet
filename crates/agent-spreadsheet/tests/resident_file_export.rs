#![cfg(all(feature = "recalc-formualizer", feature = "native-fs"))]
use agent_spreadsheet::{
    canonical_lifecycle::ArtifactMetadata,
    canonical_write::{
        ResidentWriteSession, execute_durable_write_on_resident, recover_durable_resident_session,
    },
    core::resident_storage::{
        PortableHistoryState, ResidentCommitStorage, native::NativeResidentJournal,
    },
    resident_export::*,
};
use anyhow::Result;
use serde_json::json;
use std::{cell::Cell, path::PathBuf};

struct RealIoFaultSink {
    root: PathBuf,
    fail: Cell<bool>,
    captures: Cell<usize>,
}
#[async_trait::async_trait(?Send)]
impl FileExportSink for RealIoFaultSink {
    async fn authorize(&self, intent: &FileExportIntent) -> Result<(String, String, String)> {
        Ok((
            intent.destination.clone().unwrap(),
            "0".repeat(64),
            "missing".into(),
        ))
    }
    async fn retain(&self, bytes: &[u8]) -> Result<ArtifactMetadata> {
        self.captures.set(self.captures.get() + 1);
        let hash = agent_spreadsheet::utils::hash_bytes_sha256_hex(bytes);
        std::fs::write(self.root.join(&hash), bytes)?;
        std::fs::File::open(self.root.join(&hash))?.sync_all()?;
        Ok(ArtifactMetadata {
            artifact_id: format!("artifact-{hash}"),
            media_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
            bytes: bytes.len() as u64,
            sha256: hash,
        })
    }
    async fn publish(&self, plan: &FileExportPlan) -> Result<()> {
        let bytes = std::fs::read(self.root.join(&plan.artifact.sha256))?;
        assert_eq!(bytes.len() as u64, plan.artifact.bytes);
        std::fs::write(&plan.destination, bytes)?;
        std::fs::File::open(&plan.destination)?.sync_all()?;
        if self.fail.replace(false) {
            anyhow::bail!("injected error after real filesystem write+sync");
        }
        Ok(())
    }
}
#[tokio::test]
async fn reverse_public_phase_collisions_reject_before_capture_without_poison() {
    for pending_public in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let journal_directory = tempfile::tempdir().unwrap();
        let journal = NativeResidentJournal::open(journal_directory.path()).unwrap();
        let mut base = Vec::new();
        umya_spreadsheet::writer::xlsx::write_writer(&umya_spreadsheet::new_file(), &mut base)
            .unwrap();
        let mut owner = ResidentWriteSession::from_bytes("session:test", &base).unwrap();
        let sink = RealIoFaultSink {
            root: directory.path().into(),
            fail: Cell::new(true),
            captures: Cell::new(0),
        };
        let intent = FileExportIntent {
            resource_id: "session:test".into(),
            request_id: "candidate".into(),
            expected_revision: None,
            destination: Some(
                directory
                    .path()
                    .join("output.xlsx")
                    .to_str()
                    .unwrap()
                    .into(),
            ),
            force: true,
        };
        if pending_public {
            let mut old = intent.clone();
            old.request_id = plan_id("candidate");
            assert!(export_file(&mut owner, &journal, old, &sink).await.is_err());
        } else {
            let write = serde_json::from_value(json!({"resource_id":"session:test","expected_revision":owner.revision(),"mode":"apply","ops":[{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":99}}}]})).unwrap();
            execute_durable_write_on_resident(&mut owner, &journal, &plan_id("candidate"), write)
                .await
                .unwrap();
        }
        for _ in 0..2 {
            let count = sink.captures.get();
            let records = journal.load("test").await.unwrap().len();
            let error = export_file(&mut owner, &journal, intent.clone(), &sink)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("not submitted"), "{error:#}");
            assert!(!error.to_string().contains("unknown"));
            assert_eq!(sink.captures.get(), count);
            assert_eq!(journal.load("test").await.unwrap().len(), records);
            assert!(owner.read_view().is_ok());
            owner = recover_durable_resident_session("session:test", &base, &journal)
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn real_io_then_error_retains_named_capture_across_edit_and_restart() {
    let directory = tempfile::tempdir().unwrap();
    let journal_directory = tempfile::tempdir().unwrap();
    let journal = NativeResidentJournal::open(journal_directory.path()).unwrap();
    let mut book = umya_spreadsheet::new_file();
    book.get_sheet_mut(&0)
        .unwrap()
        .get_cell_mut("A1")
        .set_value_number(7);
    let mut base = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&book, &mut base).unwrap();
    let mut owner = ResidentWriteSession::from_bytes("session:test", &base).unwrap();
    let revision = owner.revision();
    let intent = FileExportIntent {
        resource_id: "session:test".into(),
        request_id: "export-once".into(),
        expected_revision: Some(revision.clone()),
        destination: Some(
            directory
                .path()
                .join("output.xlsx")
                .to_str()
                .unwrap()
                .into(),
        ),
        force: false,
    };
    let sink = RealIoFaultSink {
        root: directory.path().into(),
        fail: Cell::new(true),
        captures: Cell::new(0),
    };
    let error = export_file(&mut owner, &journal, intent.clone(), &sink)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("outcome unknown"));
    let first_bytes = std::fs::read(intent.destination.as_ref().unwrap()).unwrap();
    let records = journal.load("test").await.unwrap();
    let history = PortableHistoryState::replay(&records).unwrap();
    assert_eq!(history.file_export_plans.len(), 1);
    assert!(history.file_export_results.is_empty());
    for identity in ["export-once".to_string(), plan_id("export-once")] {
        let records = journal.load("test").await.unwrap();
        let write = serde_json::from_value(json!({"resource_id":"session:test","expected_revision":owner.revision(),"mode":"apply","ops":[{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":99}}}]})).unwrap();
        assert!(
            execute_durable_write_on_resident(&mut owner, &journal, &identity, write)
                .await
                .is_err()
        );
        assert_eq!(journal.load("test").await.unwrap().len(), records.len());
        owner = recover_durable_resident_session("session:test", &base, &journal)
            .await
            .unwrap();
    }
    let write = serde_json::from_value(json!({"resource_id":"session:test","expected_revision":owner.revision(),"mode":"apply","ops":[{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":99}}}]})).unwrap();
    execute_durable_write_on_resident(&mut owner, &journal, "later-edit", write)
        .await
        .unwrap();
    let mut owner = recover_durable_resident_session("session:test", &base, &journal)
        .await
        .unwrap();
    let result = export_file(&mut owner, &journal, intent.clone(), &sink)
        .await
        .unwrap();
    assert_eq!(result.revision_id, revision);
    assert_eq!(
        std::fs::read(intent.destination.as_ref().unwrap()).unwrap(),
        first_bytes
    );
    assert_eq!(sink.captures.get(), 1);
    let records = journal.load("test").await.unwrap();
    let mut conflict = intent.clone();
    conflict.force = true;
    assert!(
        export_file(&mut owner, &journal, conflict, &sink)
            .await
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(
            export_file(&mut owner, &journal, intent, &sink)
                .await
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(result).unwrap()
    );
    assert_eq!(journal.load("test").await.unwrap().len(), records.len());
    let mut forged = records;
    let record = forged.last_mut().unwrap();
    record.effects[0]["file_export_result"]["artifact"]["bytes"] = json!(0);
    assert!(PortableHistoryState::replay(&forged).is_err());
}
