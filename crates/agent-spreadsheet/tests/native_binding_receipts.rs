#![cfg(all(unix, feature = "native-fs", feature = "recalc-formualizer"))]
use agent_spreadsheet::{
    core::resident_storage::{ResidentCommitStorage, native::NativeResidentJournal},
    native_host::provision_root,
    native_resident::NativeBindings,
};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn child_creation_retry_checks_base_and_reserves_pending_namespace() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = provision_root(directory.path(), "host").unwrap();
    let source = directory.path().join("source.xlsx");
    umya_spreadsheet::writer::xlsx::write(&umya_spreadsheet::new_file(), &source).unwrap();
    let (state, _) = agent_spreadsheet::runtime::stateless::StatelessRuntime
        .open_state_for_file(&source)
        .await
        .unwrap();
    let hash = agent_spreadsheet::utils::hash_file_sha256_hex(&source).unwrap();
    let store = NativeBindings::open(root.join("resources")).unwrap();
    assert!(
        store
            .create(
                "fork:pending_visible",
                &source,
                &hash,
                (*state.config()).clone()
            )
            .is_err()
    );
    assert!(store.catalog_resources().unwrap().is_empty());
    let parent = store
        .create("fork:parent", &source, &hash, (*state.config()).clone())
        .unwrap();
    let request = || {
        serde_json::from_value(
            json!({"resource_id":"fork:parent","expected_revision":"opaque-parent-revision"}),
        )
        .unwrap()
    };
    let (lane, response) = store
        .create_child(
            "child-create".into(),
            request(),
            parent,
            std::fs::read(&source).unwrap(),
        )
        .await
        .unwrap();
    lane.detach().await.unwrap();
    assert_eq!(
        serde_json::to_value(
            store
                .creation_outcome("child-create", &request())
                .await
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(Some(&response)).unwrap()
    );
    let resource = response.resource_id.unwrap();
    let key = resource.as_str().split_once(':').unwrap().1;
    let path = root.join("resources").join(key).join("journal");
    let journal = NativeResidentJournal::open(&path).unwrap();
    let mut records = journal.load(key).await.unwrap();
    for record in &mut records {
        record.base_sha256 = "a".repeat(64);
    }
    drop(journal);
    std::fs::remove_dir_all(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let journal = NativeResidentJournal::open(&path).unwrap();
    for record in records {
        journal.commit(&record).await.unwrap();
    }
    assert!(
        store
            .creation_outcome("child-create", &request())
            .await
            .is_err()
    );
    assert!(store.attach(resource.as_str()).await.is_err());
}
