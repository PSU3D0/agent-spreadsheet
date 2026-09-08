#![cfg(all(
    feature = "native-fs",
    feature = "recalc-formualizer",
    feature = "cli",
    unix
))]
use agent_spreadsheet::{
    native_host::{HostRequest, NativeHostClient, provision_root},
    native_resident::NativeBindings,
};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

struct OwnedHost {
    child: Child,
}
impl Drop for OwnedHost {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
async fn spawn_host(binary: &str, root: &Path) -> (OwnedHost, NativeHostClient) {
    spawn_host_with_profile(binary, root, None).await
}
async fn spawn_host_with_profile(
    binary: &str,
    root: &Path,
    profile: Option<&Path>,
) -> (OwnedHost, NativeHostClient) {
    let mut command = Command::new(binary);
    if let Some(profile) = profile {
        command.env("SPREADSHEET_MCP_LIBREOFFICE_USER_INSTALLATION", profile);
    }
    let child = command
        .arg("--private-resident-host")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut owned = OwnedHost { child };
    for _ in 0..400 {
        assert!(
            owned.child.try_wait().unwrap().is_none(),
            "host exited during startup"
        );
        if let Ok(client) = NativeHostClient::discover(root) {
            if client.ping().await.is_ok() {
                return (owned, client);
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("host did not start");
}
fn cli(binary: &str, root: &Path, operation: &str, id: &str, payload: Value) -> Value {
    let output = Command::new(binary)
        .env("ASP_RESIDENT_ROOT", root)
        // Explicit per-call identity must win over a process-wide fallback.
        .env("ASP_REQUEST_ID", "ignored-fallback")
        .args([
            "op",
            operation,
            "--request-id",
            id,
            "--bind",
            "session:test",
            "--json",
            &payload.to_string(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_session_family_starts_stages_applies_reads_and_navigates() {
    let workspace = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(workspace.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let host_root = provision_root(&root.path().canonicalize().unwrap(), "host").unwrap();
    // Only the optional managed root is provisioned; no workbook binding or host.
    let root = host_root;
    let _cleanup = AutoHostCleanup(root.clone());
    let source = workspace.path().join("source.xlsx");
    let mut book = umya_spreadsheet::new_file();
    book.get_sheet_mut(&0)
        .unwrap()
        .get_cell_mut("A1")
        .set_value_number(1);
    book.get_sheet_mut(&0)
        .unwrap()
        .get_cell_mut("B1")
        .set_formula("A1*2");
    umya_spreadsheet::writer::xlsx::write(&book, &source).unwrap();
    let invoke = |arguments: &[&str]| -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_asp"))
            .current_dir(workspace.path())
            .env("ASP_RESIDENT_ROOT", &root)
            .env_remove("ASP_REQUEST_ID")
            .arg("session")
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "arguments={arguments:?} stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let start = invoke(&[
        "start",
        "--base",
        "source.xlsx",
        "--label",
        "retained source label",
        "--request-id",
        "family-start",
    ]);
    let session = start["session_id"].as_str().unwrap();
    assert!(session.starts_with("fork:"));
    assert!(!workspace.path().join(".asp/sessions").exists());
    std::fs::write(
        workspace.path().join("edit.json"),
        json!({"kind":"transform.write_matrix","sheet_name":"Sheet1","anchor":"A1","rows":[[7]]})
            .to_string(),
    )
    .unwrap();
    let staged = invoke(&[
        "op",
        "--session",
        session,
        "--ops",
        "@edit.json",
        "--request-id",
        "stage-first",
    ]);
    let stage_id = staged["staged_id"].as_str().unwrap();
    let applied = invoke(&[
        "apply",
        "--session",
        session,
        stage_id,
        "--request-id",
        "apply-first",
    ]);
    let calc = invoke(&["exec", "--session", session, "recalculate"]);
    assert!(uuid::Uuid::parse_str(calc["request_id"].as_str().unwrap()).is_ok());
    let read_payload = json!({"sheet_name":"Sheet1","selection":{"kind":"range","ranges":["A1:B1"]},"format":"values"}).to_string();
    let read = invoke(&[
        "exec",
        "--session",
        session,
        "read_cells",
        "--json",
        &read_payload,
    ]);
    assert_eq!(read["data"]["blocks"][0]["payload"]["values"][0][1], 14.0);
    assert_eq!(read["revision_id"], calc["revision_id"]);
    assert_eq!(
        invoke(&[
            "op",
            "--session",
            session,
            "--ops",
            "@edit.json",
            "--request-id",
            "stage-first"
        ]),
        staged
    );
    assert_eq!(
        invoke(&[
            "apply",
            "--session",
            session,
            stage_id,
            "--request-id",
            "apply-first"
        ]),
        applied
    );
    let read_cli = |arguments: &[&str]| -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_asp"))
            .current_dir(workspace.path())
            .env("ASP_RESIDENT_ROOT", &root)
            .env_remove("ASP_REQUEST_ID")
            .args(arguments)
            .args(["--session", session])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "arguments={arguments:?} stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let page = read_cli(&[
        "sheet-page",
        "missing.xlsx",
        "Sheet1",
        "--page-size",
        "1",
        "--format",
        "values_only",
    ]);
    assert_eq!(page["revision_id"], read["revision_id"]);
    assert!(page["data"].to_string().contains("14.0"), "{page}");
    let table = read_cli(&[
        "read-table",
        "missing.xlsx",
        "--sheet",
        "Sheet1",
        "--range",
        "A1:B1",
    ]);
    assert_eq!(table["revision_id"], read["revision_id"]);
    let found = read_cli(&["find-value", "missing.xlsx", "7", "--sheet", "Sheet1"]);
    assert!(found["data"].to_string().contains("A1"), "{found}");
    let layout = read_cli(&[
        "layout-page",
        "missing.xlsx",
        "Sheet1",
        "--range",
        "A1:B1",
        "--render",
        "both",
    ]);
    assert!(layout["data"].to_string().contains("14"), "{layout}");
    let exported_range = read_cli(&[
        "range-export",
        "missing.xlsx",
        "Sheet1",
        "A1:B1",
        "--format",
        "json",
    ]);
    assert!(
        exported_range.to_string().contains("14"),
        "{exported_range}"
    );
    let grid = read_cli(&[
        "range-export",
        "missing.xlsx",
        "Sheet1",
        "A1:B1",
        "--format",
        "grid",
    ]);
    assert!(grid.to_string().contains("A1*2"), "{grid}");
    let saved_csv = read_cli(&[
        "range-export",
        "missing.xlsx",
        "Sheet1",
        "A1:B1",
        "--format",
        "csv",
        "--output",
        "range.csv",
    ]);
    assert_eq!(saved_csv["status"], "ok");
    assert!(
        std::fs::read_to_string(workspace.path().join("range.csv"))
            .unwrap()
            .contains("14")
    );
    let trace = read_cli(&[
        "formula-trace",
        "missing.xlsx",
        "Sheet1",
        "B1",
        "precedents",
    ]);
    assert!(trace["data"].to_string().contains("A1"), "{trace}");
    let client = NativeHostClient::discover(&root).unwrap();
    assert_eq!(
        client
            .request(&HostRequest::Diagnostics {
                resource_id: session.into()
            })
            .await
            .unwrap(),
        json!({"ingests":1,"evaluations":1,"serializations":0})
    );
    let log = invoke(&["log", "--session", session]);
    let target = log["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["transition"] == "stage_apply")
        .unwrap()["commit_id"]
        .as_str()
        .unwrap();
    let branch = invoke(&[
        "fork",
        "--session",
        session,
        "past",
        "--from",
        target,
        "--label",
        "kept",
        "--request-id",
        "branch-first",
    ]);
    let branches = invoke(&["branches", "--session", session]);
    assert!(
        branches["branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["name"] == "past" && b["label"] == "kept")
    );
    invoke(&["undo", "--session", session]);
    invoke(&["redo", "--session", session]);
    invoke(&["switch", "--session", session, "--branch", "past"]);
    invoke(&["checkout", "--session", session, target]);
    invoke(&[
        "exec",
        "--session",
        session,
        "checkpoint",
        "--json",
        "{\"action\":\"create\",\"label\":\"checkpoint\"}",
    ]);
    let status = invoke(&[
        "exec",
        "--session",
        session,
        "session_history",
        "--json",
        "{\"action\":\"status\"}",
    ]);
    let named = status["data"]["revision_id"].as_str().unwrap();
    // Save-as is independent of original source generation.
    std::fs::write(&source, b"external replacement").unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_asp"))
        .current_dir(workspace.path())
        .env("ASP_RESIDENT_ROOT", &root)
        .args([
            "session",
            "materialize",
            "--session",
            session,
            "--source",
            "--request-id",
            "reject-source",
        ])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert_eq!(std::fs::read(&source).unwrap(), b"external replacement");
    let exported = invoke(&[
        "materialize",
        "--session",
        session,
        "--out",
        "saved.xlsx",
        "--revision",
        named,
        "--request-id",
        "file-export",
    ]);
    let bytes = std::fs::read(workspace.path().join("saved.xlsx")).unwrap();
    assert_eq!(exported["revision_id"], named);
    assert_eq!(exported["artifact"]["bytes"], bytes.len());
    assert_eq!(
        exported["artifact"]["sha256"],
        agent_spreadsheet::utils::hash_bytes_sha256_hex(&bytes)
    );
    let saved =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(&bytes), true).unwrap();
    assert_eq!(
        saved
            .get_sheet_by_name("Sheet1")
            .unwrap()
            .get_cell("A1")
            .unwrap()
            .get_value(),
        "7"
    );
    invoke(&[
        "materialize",
        "--session",
        session,
        "--source",
        "--force",
        "--request-id",
        "force-source",
    ]);
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    std::fs::remove_file(&source).unwrap();
    client.request(&HostRequest::Shutdown).await.unwrap();
    for _ in 0..400 {
        if !root.join("discovery.json").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        invoke(&[
            "start",
            "--base",
            "source.xlsx",
            "--label",
            "retained source label",
            "--request-id",
            "family-start"
        ]),
        start
    );
    assert_eq!(
        invoke(&[
            "apply",
            "--session",
            session,
            stage_id,
            "--request-id",
            "apply-first"
        ]),
        applied
    );
    assert_eq!(
        invoke(&[
            "fork",
            "--session",
            session,
            "past",
            "--from",
            target,
            "--label",
            "kept",
            "--request-id",
            "branch-first"
        ]),
        branch
    );
    assert_eq!(
        invoke(&[
            "materialize",
            "--session",
            session,
            "--out",
            "saved.xlsx",
            "--revision",
            named,
            "--request-id",
            "file-export"
        ]),
        exported
    );
    invoke(&[
        "materialize",
        "--session",
        session,
        "--out",
        "recovered.xlsx",
        "--request-id",
        "missing-source-save-as",
    ]);
    invoke(&["exec", "--session", session, "recalculate"]);
    let read = invoke(&[
        "exec",
        "--session",
        session,
        "read_cells",
        "--json",
        &read_payload,
    ]);
    assert_eq!(read["data"]["blocks"][0]["payload"]["values"][0][1], 14.0);
    invoke(&["exec", "--session", session, "discard_fork"]);
    assert_eq!(
        invoke(&[
            "materialize",
            "--session",
            session,
            "--out",
            "saved.xlsx",
            "--revision",
            named,
            "--request-id",
            "file-export"
        ]),
        exported
    );
    let final_client = NativeHostClient::discover(&root).unwrap();
    assert!(
        final_client
            .request(&HostRequest::Diagnostics {
                resource_id: session.into()
            })
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_forced_export_cannot_overwrite_newer_destination() {
    use std::os::unix::fs::PermissionsExt;
    let workspace = tempfile::tempdir().unwrap();
    std::fs::set_permissions(workspace.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = provision_root(&workspace.path().canonicalize().unwrap(), "host").unwrap();
    let _cleanup = AutoHostCleanup(root.clone());
    let out = workspace.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let source = out.join("source.xlsx");
    let mut book = umya_spreadsheet::new_file();
    book.get_sheet_mut(&0)
        .unwrap()
        .get_cell_mut("A1")
        .set_value_number(7);
    umya_spreadsheet::writer::xlsx::write(&book, &source).unwrap();
    let invoke = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_asp"))
            .current_dir(workspace.path())
            .env("ASP_RESIDENT_ROOT", &root)
            .env_remove("ASP_REQUEST_ID")
            .arg("session")
            .args(args)
            .output()
            .unwrap()
    };
    for source_mode in [false, true] {
        let start = invoke(&["start", "--base", "out/source.xlsx"]);
        assert!(
            start.status.success(),
            "{}",
            String::from_utf8_lossy(&start.stderr)
        );
        let start: Value = serde_json::from_slice(&start.stdout).unwrap();
        let session = start["session_id"].as_str().unwrap();
        let initial_edit = invoke(&[
            "exec",
            "--session",
            session,
            "write",
            "--json",
            "{\"mode\":\"apply\",\"ops\":[{\"kind\":\"set_cells\",\"sheet_name\":\"Sheet1\",\"cells\":{\"A1\":{\"kind\":\"value\",\"value\":8}}}]}",
        ]);
        assert!(
            initial_edit.status.success(),
            "{}",
            String::from_utf8_lossy(&initial_edit.stderr)
        );
        let destination = if source_mode {
            vec!["--source"]
        } else {
            vec!["--out", "out/saved.xlsx"]
        };
        let mut old = vec![
            "materialize",
            "--session",
            session,
            "--force",
            "--request-id",
            "pending-old",
        ];
        old.extend(destination.iter().copied());
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o555)).unwrap();
        let failed = invoke(&old);
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            !failed.status.success(),
            "must execute real post-plan permission failure, not skip"
        );
        assert!(
            String::from_utf8_lossy(&failed.stderr).contains("outcome unknown"),
            "{}",
            String::from_utf8_lossy(&failed.stderr)
        );
        let collision = invoke(&[
            "exec",
            "--session",
            session,
            "write",
            "--request-id",
            "pending-old",
            "--json",
            "{\"mode\":\"apply\",\"ops\":[{\"kind\":\"set_cells\",\"sheet_name\":\"Sheet1\",\"cells\":{\"A1\":{\"kind\":\"value\",\"value\":66}}}]}",
        ]);
        assert!(!collision.status.success());
        assert!(
            String::from_utf8_lossy(&collision.stderr).contains("reserved"),
            "{}",
            String::from_utf8_lossy(&collision.stderr)
        );
        assert!(!String::from_utf8_lossy(&collision.stderr).contains("OUTCOME_UNKNOWN"));
        let edit = invoke(&[
            "exec",
            "--session",
            session,
            "write",
            "--json",
            "{\"mode\":\"apply\",\"ops\":[{\"kind\":\"set_cells\",\"sheet_name\":\"Sheet1\",\"cells\":{\"A1\":{\"kind\":\"value\",\"value\":99}}}]}",
        ]);
        assert!(
            edit.status.success(),
            "{}",
            String::from_utf8_lossy(&edit.stderr)
        );
        let mut new = vec![
            "materialize",
            "--session",
            session,
            "--force",
            "--request-id",
            "newer-export",
        ];
        new.extend(destination.iter().copied());
        let newer = invoke(&new);
        assert!(
            newer.status.success(),
            "{}",
            String::from_utf8_lossy(&newer.stderr)
        );
        let target = if source_mode {
            source.clone()
        } else {
            out.join("saved.xlsx")
        };
        let bytes = std::fs::read(&target).unwrap();
        NativeHostClient::discover(&root)
            .unwrap()
            .request(&HostRequest::Shutdown)
            .await
            .unwrap();
        for _ in 0..400 {
            if !root.join("discovery.json").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let retry = invoke(&old);
        assert!(!retry.status.success());
        assert!(
            String::from_utf8_lossy(&retry.stderr).contains("destination changed"),
            "{}",
            String::from_utf8_lossy(&retry.stderr)
        );
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        let retry = invoke(&new);
        assert!(retry.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&retry.stdout).unwrap(),
            serde_json::from_slice::<Value>(&newer.stdout).unwrap()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_cli_branch_at_retains_labels_and_original_retry_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = provision_root(&directory.path().canonicalize().unwrap(), "host").unwrap();
    let source = directory.path().join("source.xlsx");
    umya_spreadsheet::writer::xlsx::write(&umya_spreadsheet::new_file(), &source).unwrap();
    let (state, _) = agent_spreadsheet::runtime::stateless::StatelessRuntime
        .open_state_for_file(&source)
        .await
        .unwrap();
    let hash = agent_spreadsheet::utils::hash_file_sha256_hex(&source).unwrap();
    NativeBindings::open(root.join("resources"))
        .unwrap()
        .create("session:test", &source, &hash, (*state.config()).clone())
        .unwrap();
    let (mut host, client) = spawn_host(env!("CARGO_BIN_EXE_asp"), &root).await;
    let status = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "session_history",
        "status",
        json!({"action":"status"}),
    );
    let revision = status["data"]["revision_id"].clone();
    let payload = json!({"action":"create_branch","expected_revision":revision,"name":"archived","target_commit_id":"base","label":"Base label"});
    let created = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "session_history",
        "branch",
        payload.clone(),
    );
    let listed = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "session_history",
        "list",
        json!({"action":"list"}),
    );
    assert_eq!(listed["data"]["branch_labels"]["archived"], "Base label");
    assert_eq!(listed["data"]["branches"]["archived"], Value::Null);
    assert_eq!(listed["data"]["branch"], "main");
    let diagnostics = client
        .request(&HostRequest::Diagnostics {
            resource_id: "session:test".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        diagnostics,
        json!({"ingests":1,"evaluations":0,"serializations":0})
    );
    host.child.kill().unwrap();
    host.child.wait().unwrap();
    let (_restarted, _) = spawn_host(env!("CARGO_BIN_EXE_asp"), &root).await;
    assert_eq!(
        cli(
            env!("CARGO_BIN_EXE_asp"),
            &root,
            "session_history",
            "branch",
            payload
        ),
        created
    );
    let listed = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "session_history",
        "list-again",
        json!({"action":"list"}),
    );
    assert_eq!(listed["data"]["branch_labels"]["archived"], "Base label");
}

#[cfg(feature = "recalc-libreoffice")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_native_configured_libreoffice_retains_document_and_retry() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = provision_root(&directory.path().canonicalize().unwrap(), "host").unwrap();
    let source = directory.path().join("source.xlsx");
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.get_sheet_mut(&0).unwrap();
    sheet.get_cell_mut("A1").set_value_number(1);
    sheet
        .get_cell_mut("A1")
        .get_style_mut()
        .get_font_mut()
        .set_bold(true);
    sheet.get_cell_mut("B1").set_formula("A1*3");
    sheet.get_cell_mut("B2").set_formula("\"00123\"");
    sheet.get_cell_mut("B3").set_formula("\"TRUE\"");
    sheet.get_cell_mut("B4").set_formula("\"\"");
    sheet.get_cell_mut("C1").set_formula("1/0");
    umya_spreadsheet::writer::xlsx::write(&book, &source).unwrap();
    let (files, _) = agent_spreadsheet::runtime::stateless::StatelessRuntime
        .open_state_for_file(&source)
        .await
        .unwrap();
    let mut config = (*files.config()).clone();
    config.recalc_backend = agent_spreadsheet::config::RecalcBackendKind::Libreoffice;
    config.recalc_enabled = true;
    let hash = agent_spreadsheet::utils::hash_file_sha256_hex(&source).unwrap();
    NativeBindings::open(root.join("resources"))
        .unwrap()
        .create("session:test", &source, &hash, config)
        .unwrap();
    let profile = directory.path().join("office-profile");
    let standard = profile.join("user/basic/Standard");
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
        profile.join("user/registrymodifications.xcu"),
        include_bytes!("../../../docker/libreoffice/registrymodifications.xcu"),
    )
    .unwrap();
    std::fs::write(profile.join("user/basic/script.xlc"), br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE library:libraries PUBLIC "-//OpenOffice.org//DTD OfficeDocument 1.0//EN" "libraries.dtd">
<library:libraries xmlns:library="http://openoffice.org/2000/library"><library:library library:name="Standard" library:link="false"/></library:libraries>"#).unwrap();
    let (mut host, _) =
        spawn_host_with_profile(env!("CARGO_BIN_EXE_asp"), &root, Some(&profile)).await;
    let status = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "session_history",
        "status",
        json!({"action":"status"}),
    );
    let written = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "write",
        "edit",
        json!({
            "expected_revision":status["data"]["revision_id"], "mode":"apply",
            "ops":[{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":4}}}]
        }),
    );
    let request = json!({"expected_revision":written["revision_id"]});
    let calculated = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "recalculate",
        "calculate",
        request.clone(),
    );
    assert_eq!(calculated["data"]["backend"], "libreoffice");
    let read = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "read_cells",
        "read",
        json!({
            "sheet_name":"Sheet1","selection":{"kind":"range","ranges":["B1"]},"format":"values"
        }),
    );
    assert_eq!(read["data"]["blocks"][0]["payload"]["values"][0][0], 12.0);
    let destination = directory.path().join("saved.xlsx");
    let saved = Command::new(env!("CARGO_BIN_EXE_asp"))
        .env("ASP_RESIDENT_ROOT", &root)
        .args(["session", "materialize", "--session", "test", "--out"])
        .arg(&destination)
        .output()
        .unwrap();
    assert!(
        saved.status.success(),
        "{}",
        String::from_utf8_lossy(&saved.stderr)
    );
    let saved = umya_spreadsheet::reader::xlsx::read(&destination).unwrap();
    let sheet = saved.get_sheet(&0).unwrap();
    assert_eq!(sheet.get_cell("B1").unwrap().get_formula(), "A1*3");
    assert_eq!(sheet.get_cell("B1").unwrap().get_value(), "12");
    assert_eq!(sheet.get_cell("B2").unwrap().get_value(), "00123");
    assert_eq!(sheet.get_cell("B3").unwrap().get_value(), "TRUE");
    assert_eq!(sheet.get_cell("B4").unwrap().get_value(), "");
    assert_eq!(sheet.get_cell("C1").unwrap().get_value(), "#DIV/0!");
    assert!(
        *sheet
            .get_cell("A1")
            .unwrap()
            .get_style()
            .get_font()
            .unwrap()
            .get_bold()
    );
    host.child.kill().unwrap();
    host.child.wait().unwrap();
    let (_restarted, _) =
        spawn_host_with_profile(env!("CARGO_BIN_EXE_asp"), &root, Some(&profile)).await;
    assert_eq!(
        cli(
            env!("CARGO_BIN_EXE_asp"),
            &root,
            "recalculate",
            "calculate",
            request
        ),
        calculated
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_cli_processes_share_owner_and_recover_original_outcomes() {
    let directory = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = provision_root(&directory.path().canonicalize().unwrap(), "host").unwrap();
    let source = directory.path().join("source.xlsx");
    let mut book = umya_spreadsheet::new_file();
    book.get_sheet_mut(&0)
        .unwrap()
        .get_cell_mut("A1")
        .set_value_number(1);
    book.get_sheet_mut(&0)
        .unwrap()
        .get_cell_mut("B1")
        .set_formula("A1*2");
    umya_spreadsheet::writer::xlsx::write(&book, &source).unwrap();
    let (state, _) = agent_spreadsheet::runtime::stateless::StatelessRuntime
        .open_state_for_file(&source)
        .await
        .unwrap();
    let hash = agent_spreadsheet::utils::hash_file_sha256_hex(&source).unwrap();
    let store = NativeBindings::open(root.join("resources")).unwrap();
    store
        .create("session:test", &source, &hash, (*state.config()).clone())
        .unwrap();
    let (mut host, client) = spawn_host(env!("CARGO_BIN_EXE_asp"), &root).await;
    let instance = client.ping().await.unwrap();
    let status = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "session_history",
        "status",
        json!({"action":"status"}),
    );
    let mut revision = status["data"]["revision_id"].as_str().unwrap().to_string();
    let mut outcomes = Vec::new();
    let mut calculation_outcomes = Vec::new();
    for n in 2..5 {
        let payload = json!({"expected_revision":revision,"mode":"apply","atomic":true,"ops":[{"kind":"write_matrix","sheet_name":"Sheet1","anchor":"A1","rows":[[{"v":n}]]}]});
        let id = format!("write-{n}");
        let response = cli(
            env!("CARGO_BIN_EXE_agent-spreadsheet"),
            &root,
            "write",
            &id,
            payload.clone(),
        );
        revision = response["revision_id"].as_str().unwrap().into();
        outcomes.push((id, payload, response));
        let calculation_payload = json!({"expected_revision":revision});
        let response = cli(
            env!("CARGO_BIN_EXE_asp"),
            &root,
            "recalculate",
            &format!("calc-{n}"),
            calculation_payload.clone(),
        );
        revision = response["revision_id"].as_str().unwrap().into();
        calculation_outcomes.push((format!("calc-{n}"), calculation_payload, response));
        let read = cli(
            env!("CARGO_BIN_EXE_asp"),
            &root,
            "read_cells",
            "read",
            json!({"sheet_name":"Sheet1","selection":{"kind":"range","ranges":["A1:B1"]},"format":"values"}),
        );
        assert_eq!(
            read["data"]["blocks"][0]["payload"]["values"][0][1],
            json!((n * 2) as f64)
        );
    }
    assert_eq!(client.ping().await.unwrap(), instance);
    let diagnostics = client
        .request(&HostRequest::Diagnostics {
            resource_id: "session:test".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        diagnostics,
        json!({"ingests":1,"evaluations":3,"serializations":0})
    );
    // The lifetime owner lock fences a second independent evaluator, not merely append.
    assert!(store.attach("session:test").await.is_err());
    for (id, payload, response) in &outcomes {
        assert_eq!(
            &cli(
                env!("CARGO_BIN_EXE_asp"),
                &root,
                "write",
                id,
                payload.clone()
            ),
            response
        );
    }
    let checkpoint_payload =
        json!({"action":"create","expected_revision":revision,"label":"original checkpoint"});
    let checkpoint = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "checkpoint",
        "checkpoint",
        checkpoint_payload.clone(),
    );
    assert_eq!(checkpoint["revision_id"], revision);
    let stage_payload = json!({"expected_revision":revision,"mode":"stage","atomic":true,"label":"approval","ops":[{"kind":"write_matrix","sheet_name":"Sheet1","anchor":"A1","rows":[[{"v":7}]]}]});
    let stage = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "write",
        "stage",
        stage_payload.clone(),
    );
    assert_eq!(stage["revision_id"], revision);
    let staged = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "staged_change",
        "staged-list",
        json!({"action":"list"}),
    );
    assert_eq!(staged["data"]["staged_changes"][0]["label"], "approval");
    let apply_payload = json!({"action":"apply","expected_revision":revision,"change_id":stage["data"]["change_id"]});
    let applied = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "staged_change",
        "apply-stage",
        apply_payload.clone(),
    );
    assert_ne!(applied["revision_id"], revision);
    let describe = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "describe_workbook",
        "describe",
        json!({}),
    );
    assert!(describe["data"]["metadata"]["bytes"].is_null());
    // Real process kill, not a storage double; no graceful flush is relied upon.
    host.child.kill().unwrap();
    host.child.wait().unwrap();
    let (mut recovered, client) = spawn_host(env!("CARGO_BIN_EXE_agent-spreadsheet"), &root).await;
    for (id, payload, response) in &calculation_outcomes {
        assert_eq!(
            &cli(
                env!("CARGO_BIN_EXE_asp"),
                &root,
                "recalculate",
                id,
                payload.clone()
            ),
            response
        );
    }
    for (id, payload, response) in &outcomes {
        assert_eq!(
            &cli(
                env!("CARGO_BIN_EXE_asp"),
                &root,
                "write",
                id,
                payload.clone()
            ),
            response
        );
    }
    assert_eq!(
        cli(
            env!("CARGO_BIN_EXE_asp"),
            &root,
            "checkpoint",
            "checkpoint",
            checkpoint_payload
        ),
        checkpoint
    );
    assert_eq!(
        cli(
            env!("CARGO_BIN_EXE_asp"),
            &root,
            "write",
            "stage",
            stage_payload
        ),
        stage
    );
    assert_eq!(
        cli(
            env!("CARGO_BIN_EXE_asp"),
            &root,
            "staged_change",
            "apply-stage",
            apply_payload
        ),
        applied
    );
    let status = cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "session_history",
        "status2",
        json!({"action":"status"}),
    );
    let revision = &status["data"]["revision_id"];
    cli(
        env!("CARGO_BIN_EXE_asp"),
        &root,
        "write",
        "continued",
        json!({"expected_revision":revision,"mode":"apply","atomic":true,"ops":[{"kind":"write_matrix","sheet_name":"Sheet1","anchor":"A1","rows":[[{"v":9}]]}]}),
    );
    // Unauthenticated loopback access and browser-origin requests are denied.
    let discovery: Value =
        serde_json::from_slice(&std::fs::read(root.join("discovery.json")).unwrap()).unwrap();
    let url = format!("http://127.0.0.1:{}/", discovery["port"]);
    let http = reqwest::Client::new();
    assert_eq!(
        http.post(&url)
            .json(&json!({"control":"ping"}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.post(&url)
            .bearer_auth(discovery["credential"].as_str().unwrap())
            .header("origin", "http://evil.invalid")
            .json(&json!({"control":"ping"}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    client.request(&HostRequest::Shutdown).await.unwrap();
    for _ in 0..200 {
        if recovered.child.try_wait().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        recovered.child.try_wait().unwrap().is_some(),
        "host did not shut down"
    );

    // Rebind the actual vacated port as an adversarial local listener. The
    // attacker knows expected health fields, but cannot authenticate a response.
    let listener = tokio::net::TcpListener::bind((
        std::net::Ipv4Addr::LOCALHOST,
        discovery["port"].as_u64().unwrap() as u16,
    ))
    .await
    .unwrap();
    let captured = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let capture = captured.clone();
    let forged = json!({"protocol":discovery["protocol"],"instance":discovery["instance"],"pid":instance["pid"]});
    let app = axum::Router::new().route(
        "/",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let capture = capture.clone();
                let forged = forged.clone();
                async move {
                    *capture.lock().await = Some((headers.clone(), body.to_vec()));
                    // Reflect the request MAC: domain separation must reject it.
                    (
                        [(
                            "x-asp-signature",
                            headers["x-asp-signature"].to_str().unwrap().to_owned(),
                        )],
                        axum::Json(forged),
                    )
                }
            },
        ),
    );
    let stop = std::sync::Arc::new(tokio::sync::Notify::new());
    let stopped = stop.clone();
    let adversary = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { stopped.notified().await })
            .await
            .unwrap();
    });
    assert!(
        client.ping().await.is_err(),
        "forged stale-port health must not authenticate"
    );
    let (headers, body) = captured.lock().await.take().unwrap();
    assert!(
        !format!("{headers:?}{}", String::from_utf8_lossy(&body))
            .contains(discovery["credential"].as_str().unwrap()),
        "client disclosed reusable key to stale listener"
    );
    assert!(!headers.contains_key("authorization"));
    stop.notify_one();
    adversary.await.unwrap();

    // The captured signed request must not authenticate to a fresh host either.
    let (_third, fresh) = spawn_host(env!("CARGO_BIN_EXE_asp"), &root).await;
    let fresh_discovery: Value =
        serde_json::from_slice(&std::fs::read(root.join("discovery.json")).unwrap()).unwrap();
    assert_ne!(fresh_discovery["credential"], discovery["credential"]);
    let replay_url = format!("http://127.0.0.1:{}/", fresh_discovery["port"]);
    assert_eq!(
        http.post(replay_url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    fresh.request(&HostRequest::Shutdown).await.unwrap();
}

// Auto-started daemons are not direct children of this runner. Cleanup uses the
// private authenticated endpoint, including during assertion unwinding.
struct AutoHostCleanup(std::path::PathBuf);
impl Drop for AutoHostCleanup {
    fn drop(&mut self) {
        let root = self.0.clone();
        let _ = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                if let Ok(client) = NativeHostClient::discover(&root) {
                    let _ = tokio::time::timeout(
                        Duration::from_secs(10),
                        client.request(&HostRequest::Shutdown),
                    )
                    .await;
                }
            });
            for _ in 0..400 {
                if !root.join("discovery.json").exists() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        })
        .join();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_cli_autostart_races_aliases_and_live_unreachable_owner() {
    use fs2::FileExt;
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = provision_root(&directory.path().canonicalize().unwrap(), "automatic").unwrap();
    let _cleanup = AutoHostCleanup(root.clone());
    let source = directory.path().join("source.xlsx");
    umya_spreadsheet::writer::xlsx::write(&umya_spreadsheet::new_file(), &source).unwrap();
    let (state, _) = agent_spreadsheet::runtime::stateless::StatelessRuntime
        .open_state_for_file(&source)
        .await
        .unwrap();
    NativeBindings::open(root.join("resources"))
        .unwrap()
        .create(
            "session:test",
            &source,
            &agent_spreadsheet::utils::hash_file_sha256_hex(&source).unwrap(),
            (*state.config()).clone(),
        )
        .unwrap();

    // Invalid/missing identities fail before bootstrapping, and file commands
    // cannot misleadingly promise the resident retry contract.
    for arguments in [
        vec![
            "op",
            "session_history",
            "--bind",
            "session:test",
            "--json",
            r#"{"action":"status"}"#,
        ],
        vec![
            "op",
            "session_history",
            "--bind",
            "session:test",
            "--request-id",
            "",
            "--json",
            r#"{"action":"status"}"#,
        ],
        vec![
            "op",
            "list_workbooks",
            "--request-id",
            "not-durable",
            "--json",
            "{}",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_asp"))
            .env("ASP_RESIDENT_ROOT", &root)
            .env_remove("ASP_REQUEST_ID")
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("--request-id"));
        assert!(!root.join("discovery.json").exists());
    }

    // An OS owner without reachable discovery is never stolen, even on the
    // first user invocation. This is not an age/marker simulation.
    let lock_path = root.join("host.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    lock.lock_exclusive().unwrap();
    let blocked = Command::new(env!("CARGO_BIN_EXE_asp"))
        .env("ASP_RESIDENT_ROOT", &root)
        .env_remove("ASP_REQUEST_ID")
        .args([
            "op",
            "session_history",
            "--bind",
            "session:test",
            "--request-id",
            "status",
            "--json",
            r#"{"action":"status"}"#,
        ])
        .output()
        .unwrap();
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("live native host is unreachable"));
    assert!(!root.join("discovery.json").exists());
    FileExt::unlock(&lock).unwrap();

    // No explicit host start: both real entrypoints race through the user path.
    let handles: Vec<_> = (0..6)
        .map(|n| {
            let root = root.clone();
            std::thread::spawn(move || {
                cli(
                    if n % 2 == 0 {
                        env!("CARGO_BIN_EXE_asp")
                    } else {
                        env!("CARGO_BIN_EXE_agent-spreadsheet")
                    },
                    &root,
                    "session_history",
                    &format!("status-{n}"),
                    json!({"action":"status"}),
                )
            })
        })
        .collect();
    let responses: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for response in &responses[1..] {
        assert_eq!(
            response["data"]["revision_id"],
            responses[0]["data"]["revision_id"]
        );
    }
    let client = NativeHostClient::discover(&root).unwrap();
    let first = client.ping().await.unwrap();
    let stale_discovery = std::fs::read(root.join("discovery.json")).unwrap();
    let counters = client
        .request(&HostRequest::Diagnostics {
            resource_id: "session:test".into(),
        })
        .await
        .unwrap();
    assert_eq!(counters["ingests"], 1, "{counters}");
    assert_eq!(counters["serializations"], 0, "{counters}");
    cli(
        env!("CARGO_BIN_EXE_agent-spreadsheet"),
        &root,
        "session_history",
        "last-status",
        json!({"action":"status"}),
    );
    assert_eq!(client.ping().await.unwrap(), first);
    client.request(&HostRequest::Shutdown).await.unwrap();
    for _ in 0..400 {
        if !root.join("discovery.json").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!root.join("discovery.json").exists());
    let mut released = false;
    for _ in 0..400 {
        if lock.try_lock_exclusive().is_ok() {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        released,
        "authenticated shutdown did not release the host lock"
    );
    FileExt::unlock(&lock).unwrap();

    // A stale discovery file with no owner is recoverable through the same
    // actual user path, not an explicit test-only start command.
    std::fs::write(root.join("discovery.json"), &stale_discovery).unwrap();
    std::fs::set_permissions(
        root.join("discovery.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let recovered = cli(
        env!("CARGO_BIN_EXE_agent-spreadsheet"),
        &root,
        "session_history",
        "after-stale",
        json!({"action":"status"}),
    );
    // Recovery creates a fresh public CAS epoch, even with an empty journal.
    assert_ne!(
        recovered["data"]["revision_id"],
        responses[0]["data"]["revision_id"]
    );
    let replacement = NativeHostClient::discover(&root).unwrap();
    assert_ne!(
        replacement.ping().await.unwrap()["instance"],
        first["instance"]
    );
    let old: Value = serde_json::from_slice(&stale_discovery).unwrap();
    let new: Value =
        serde_json::from_slice(&std::fs::read(root.join("discovery.json")).unwrap()).unwrap();
    assert_ne!(old["credential"], new["credential"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_cli_canonical_creation_keeps_owner_and_original_outcome() {
    use fs2::FileExt;
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = provision_root(&directory.path().canonicalize().unwrap(), "created-host").unwrap();
    let _cleanup = AutoHostCleanup(root.clone());
    let source = directory.path().join("source.xlsx");
    let mut book = umya_spreadsheet::new_file();
    book.get_sheet_mut(&0)
        .unwrap()
        .get_cell_mut("A1")
        .set_value_number(2);
    book.get_sheet_mut(&0)
        .unwrap()
        .get_cell_mut("B1")
        .set_formula("A1*3");
    let sheet = book.get_sheet_mut(&0).unwrap();
    sheet.get_style_mut("D1").get_font_mut().set_bold(true);
    sheet.get_column_dimension_mut("D").set_width(24.0);
    sheet.add_merge_cells("D1:E1");
    sheet.add_defined_name("InputCell", "Sheet1!$A$1").unwrap();
    umya_spreadsheet::writer::xlsx::write(&book, &source).unwrap();
    let invoke = |binary: &str,
                  operation: &str,
                  identity: Option<&str>,
                  bind: Option<&str>,
                  payload: Value| {
        let mut command = Command::new(binary);
        command
            .current_dir(directory.path())
            .env("ASP_RESIDENT_ROOT", &root)
            .env_remove("ASP_REQUEST_ID")
            .args(["op", operation, "--json", &payload.to_string()]);
        if let Some(identity) = identity {
            command.args(["--request-id", identity]);
        }
        if let Some(bind) = bind {
            command.args(["--bind", bind]);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    // Discover the canonical source through the actual stateless CLI. No
    // NativeBindings::create, direct host start, or test-only resource seed.
    let listing = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "list_workbooks",
        None,
        None,
        json!({}),
    );
    let entry = &listing["data"]["workbooks"][0];
    let source_id = entry["resource_id"].as_str().unwrap();
    let revision = entry["metadata"]["revision_id"].as_str().unwrap();
    let payload = json!({"resource_id":source_id,"expected_revision":revision});
    let created = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            invoke(
                env!("CARGO_BIN_EXE_asp"),
                "create_fork",
                Some("create-one"),
                None,
                payload.clone(),
            )
        });
        let second = scope.spawn(|| {
            invoke(
                env!("CARGO_BIN_EXE_agent-spreadsheet"),
                "create_fork",
                Some("create-one"),
                None,
                payload.clone(),
            )
        });
        let first = first.join().unwrap();
        assert_eq!(first, second.join().unwrap());
        first
    });
    assert!(created["data"]["ttl_seconds"].is_null());
    let resource = created["resource_id"].as_str().unwrap();
    let initial = created["revision_id"].as_str().unwrap();
    let client = NativeHostClient::discover(&root).unwrap();
    let instance = client.ping().await.unwrap();
    let written = invoke(
        env!("CARGO_BIN_EXE_agent-spreadsheet"),
        "write",
        Some("first-write"),
        Some(resource),
        json!({"expected_revision":initial,"mode":"apply","atomic":true,"ops":[{"kind":"write_matrix","sheet_name":"Sheet1","anchor":"A1","rows":[[{"v":7}]]}]}),
    );
    let calculated = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "recalculate",
        Some("first-calc"),
        Some(resource),
        json!({"expected_revision":written["revision_id"]}),
    );
    assert_ne!(calculated["revision_id"], written["revision_id"]);
    let stage_payload = json!({"expected_revision":calculated["revision_id"],"mode":"stage","atomic":true,"label":"created-owner-approval","ops":[{"kind":"write_matrix","sheet_name":"Sheet1","anchor":"F1","rows":[[{"v":42}]]}]});
    let staged = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "write",
        Some("created-stage"),
        Some(resource),
        stage_payload.clone(),
    );
    assert_eq!(staged["revision_id"], calculated["revision_id"]);
    let checkpoint_payload = json!({"action":"create","expected_revision":calculated["revision_id"],"label":"created-owner-checkpoint"});
    let checkpoint = invoke(
        env!("CARGO_BIN_EXE_agent-spreadsheet"),
        "checkpoint",
        Some("created-checkpoint"),
        Some(resource),
        checkpoint_payload.clone(),
    );
    assert_eq!(checkpoint["revision_id"], calculated["revision_id"]);
    let read = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "read_cells",
        Some("first-read"),
        Some(resource),
        json!({"sheet_name":"Sheet1","selection":{"kind":"range","ranges":["B1"]},"format":"values"}),
    );
    assert_eq!(
        read["data"]["blocks"][0]["payload"]["values"][0][0],
        json!(21.0),
        "{read}"
    );
    let counters = client
        .request(&HostRequest::Diagnostics {
            resource_id: resource.into(),
        })
        .await
        .unwrap();
    assert_eq!(
        counters,
        json!({"ingests":1,"evaluations":1,"serializations":0})
    );
    assert_eq!(client.ping().await.unwrap(), instance);
    let verified = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "verify_workbook",
        Some("cold-verify"),
        Some(resource),
        json!({"baseline_resource_id": source_id, "targets":["Sheet1!A1", "Sheet1!B1"]}),
    );
    assert_eq!(verified["revision_id"], calculated["revision_id"]);
    assert_eq!(verified["data"]["baseline_revision_id"], revision);
    assert_eq!(
        verified["data"]["current_revision_id"],
        calculated["revision_id"]
    );
    assert_eq!(
        verified["data"]["summary"]["changed_targets"], 2,
        "{verified}"
    );
    assert_eq!(verified["data"]["target_deltas"][1]["before"]["value"], 6.0);
    assert_eq!(verified["data"]["target_deltas"][1]["after"]["value"], 21.0);
    let counters_after_verify = client
        .request(&HostRequest::Diagnostics {
            resource_id: resource.into(),
        })
        .await
        .unwrap();
    assert_eq!(
        counters_after_verify,
        json!({"ingests":1,"evaluations":1,"serializations":1})
    );
    let reversed = client
        .request(&HostRequest::Execute {
            resource_id: source_id.into(),
            request_id: "file-current-native-baseline".into(),
            operation: "verify_workbook".into(),
            payload: json!({"resource_id": source_id, "baseline_resource_id": resource,
            "targets": ["Sheet1!A1", "Sheet1!B1"]}),
        })
        .await
        .unwrap();
    let reversed = &reversed["response"];
    assert_eq!(reversed["revision_id"], revision);
    assert_eq!(
        reversed["data"]["baseline_revision_id"],
        calculated["revision_id"]
    );
    assert_eq!(
        reversed["data"]["target_deltas"][1]["before"]["value"],
        21.0
    );
    assert_eq!(reversed["data"]["target_deltas"][1]["after"]["value"], 6.0);
    // Differential against the existing file-bound verification entrypoint,
    // not a separately implemented resident verification algorithm.
    let verification_files = tempfile::tempdir().unwrap();
    std::fs::copy(&source, verification_files.path().join("baseline.xlsx")).unwrap();
    let mut expected_book = book.clone();
    expected_book
        .get_sheet_mut(&0)
        .unwrap()
        .get_cell_mut("A1")
        .set_value_number(7);
    umya_spreadsheet::writer::xlsx::write(
        &expected_book,
        verification_files.path().join("current.xlsx"),
    )
    .unwrap();
    let mut verification_config: agent_spreadsheet::config::ServerConfig =
        serde_json::from_slice(&std::fs::read(root.join("config.json")).unwrap()).unwrap();
    verification_config.workspace_root = verification_files.path().to_owned();
    let verification_state = std::sync::Arc::new(agent_spreadsheet::state::AppState::new(
        std::sync::Arc::new(verification_config),
    ));
    let listing = verification_state
        .list_workbooks(Default::default())
        .unwrap();
    let file_id = |slug: &str| {
        agent_spreadsheet::operations::ResourceId::bind_workbook(
            &listing
                .workbooks
                .iter()
                .find(|item| item.slug == slug)
                .unwrap()
                .workbook_id,
        )
        .unwrap()
    };
    let differential = agent_spreadsheet::canonical_lifecycle::verify_workbook(verification_state.clone(), serde_json::from_value(json!({
        "resource_id":file_id("current"),"baseline_resource_id":file_id("baseline"),"targets":["Sheet1!A1","Sheet1!B1"]
    })).unwrap()).await.unwrap();
    let differential = serde_json::to_value(differential).unwrap();
    for field in [
        "target_deltas",
        "summary",
        "baseline_state",
        "current_state",
        "proof_status",
        "named_range_deltas",
        "new_errors",
        "resolved_errors",
        "preexisting_errors",
    ] {
        assert_eq!(
            verified["data"][field], differential[field],
            "verification differential: {field}"
        );
    }
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_agent-spreadsheet"),
            "create_fork",
            Some("create-one"),
            None,
            payload.clone()
        ),
        created
    );
    let wrong = Command::new(env!("CARGO_BIN_EXE_asp"))
        .current_dir(directory.path())
        .env("ASP_RESIDENT_ROOT", &root)
        .env_remove("ASP_REQUEST_ID")
        .args([
            "op",
            "create_fork",
            "--request-id",
            "create-one",
            "--json",
            &json!({"resource_id":source_id,"expected_revision":"different-input"}).to_string(),
        ])
        .output()
        .unwrap();
    assert!(!wrong.status.success());
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("request identity reuse"));
    let reconciled = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "session_history",
        Some("creation-outcome"),
        Some(resource),
        json!({"action":"outcome","request_id":"create-one"}),
    );
    assert_eq!(reconciled["data"]["state"], "committed");
    assert_eq!(reconciled["data"]["response"], created);
    let independent = invoke(
        env!("CARGO_BIN_EXE_agent-spreadsheet"),
        "create_fork",
        Some("create-two"),
        None,
        payload.clone(),
    );
    let independent_id = independent["resource_id"].as_str().unwrap();
    assert_ne!(independent_id, resource);
    invoke(
        env!("CARGO_BIN_EXE_asp"),
        "write",
        Some("independent-write"),
        Some(independent_id),
        json!({"expected_revision":independent["revision_id"],"mode":"apply","atomic":true,"ops":[{"kind":"write_matrix","sheet_name":"Sheet1","anchor":"A1","rows":[[{"v":99}]]}]}),
    );
    let independent_read = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "read_cells",
        Some("independent-read"),
        Some(independent_id),
        json!({"sheet_name":"Sheet1","selection":{"kind":"range","ranges":["A1"]},"format":"values"}),
    );
    assert_eq!(
        independent_read["data"]["blocks"][0]["payload"]["values"][0][0],
        json!(99.0)
    );
    let unchanged = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "read_cells",
        Some("isolated-read"),
        Some(resource),
        json!({"sheet_name":"Sheet1","selection":{"kind":"range","ranges":["A1:B1"]},"format":"values"}),
    );
    assert_eq!(
        unchanged["data"]["blocks"][0]["payload"]["values"][0],
        json!([7.0, 21.0])
    );
    let export_payload = json!({"expected_revision":calculated["revision_id"],"destination":{"kind":"workspace","name":"immutable.xlsx"}});
    let exported = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "export_fork",
        Some("export-one"),
        Some(resource),
        export_payload.clone(),
    );
    assert_eq!(exported["revision_id"], calculated["revision_id"]);
    assert_eq!(
        exported["data"]["revision_before"],
        exported["data"]["revision_after"]
    );
    let artifact = &exported["data"]["artifact"];
    let artifact_path = directory
        .path()
        .join("artifacts")
        .join(format!("{}.xlsx", artifact["sha256"].as_str().unwrap()));
    assert_eq!(
        artifact["bytes"].as_u64().unwrap(),
        std::fs::metadata(&artifact_path).unwrap().len()
    );
    assert_eq!(
        artifact["sha256"],
        agent_spreadsheet::utils::hash_file_sha256_hex(&artifact_path).unwrap()
    );
    let saved = umya_spreadsheet::reader::xlsx::read(&artifact_path).unwrap();
    let sheet = saved.get_sheet_by_name("Sheet1").unwrap();
    assert_eq!(sheet.get_cell("A1").unwrap().get_value(), "7");
    assert_eq!(sheet.get_cell("B1").unwrap().get_value(), "21");
    assert_eq!(sheet.get_cell("B1").unwrap().get_formula(), "A1*3");
    assert!(*sheet.get_style("D1").get_font().unwrap().get_bold());
    assert_eq!(*sheet.get_column_dimension("D").unwrap().get_width(), 24.0);
    assert!(
        sheet
            .get_merge_cells()
            .iter()
            .any(|merge| merge.get_range() == "D1:E1")
    );
    assert!(
        sheet
            .get_defined_names()
            .iter()
            .any(|name| name.get_name() == "InputCell")
    );
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_agent-spreadsheet"),
            "export_fork",
            Some("export-one"),
            Some(resource),
            export_payload.clone()
        ),
        exported
    );
    book.get_sheet_mut(&0)
        .unwrap()
        .get_cell_mut("A1")
        .set_value_number(3);
    umya_spreadsheet::writer::xlsx::write(&book, &source).unwrap();
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_asp"),
            "create_fork",
            Some("create-one"),
            None,
            payload.clone()
        ),
        created
    );
    let stale = Command::new(env!("CARGO_BIN_EXE_asp"))
        .current_dir(directory.path())
        .env("ASP_RESIDENT_ROOT", &root)
        .env_remove("ASP_REQUEST_ID")
        .args([
            "op",
            "create_fork",
            "--request-id",
            "create-stale",
            "--json",
            &payload.to_string(),
        ])
        .output()
        .unwrap();
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("REVISION_CONFLICT"));
    let source_conflict = Command::new(env!("CARGO_BIN_EXE_asp"))
        .current_dir(directory.path())
        .env("ASP_RESIDENT_ROOT", &root)
        .env_remove("ASP_REQUEST_ID")
        .args([
            "op",
            "export_fork",
            "--bind",
            resource,
            "--request-id",
            "export-source-conflict",
            "--json",
            &export_payload.to_string(),
        ])
        .output()
        .unwrap();
    assert!(!source_conflict.status.success());
    assert!(String::from_utf8_lossy(&source_conflict.stderr).contains("REVISION_CONFLICT"));
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_asp"),
            "export_fork",
            Some("export-one"),
            Some(resource),
            export_payload.clone()
        ),
        exported
    );
    // Original outcome does not depend on resolving/hashing the source again.
    std::fs::remove_file(&source).unwrap();
    let net = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "get_changes",
        Some("net-diff"),
        Some(resource),
        json!({"view":{"kind":"net_diff","sheet_name":"Sheet1"}}),
    );
    assert_eq!(net["data"]["baseline_revision_id"], json!(revision));
    let change = net["data"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["address"] == "A1")
        .unwrap();
    assert_eq!(change["old_value"], "2");
    assert_eq!(change["new_value"], "7");
    assert_eq!(net["revision_id"], calculated["revision_id"]);
    let cold = client
        .request(&HostRequest::Diagnostics {
            resource_id: resource.into(),
        })
        .await
        .unwrap();
    assert_eq!(
        cold,
        json!({"ingests":1,"evaluations":1,"serializations":1})
    );
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_asp"),
            "create_fork",
            Some("create-one"),
            None,
            payload.clone()
        ),
        created
    );
    let key = resource.strip_prefix("fork:").unwrap();
    let binding: Value = serde_json::from_slice(
        &std::fs::read(root.join("resources").join(key).join("binding.json")).unwrap(),
    )
    .unwrap();
    {
        use agent_spreadsheet::core::resident_storage::{
            PortableHistoryState, ResidentCommitStorage,
        };
        let journal =
            agent_spreadsheet::core::resident_storage::native::NativeResidentJournal::open(
                root.join("resources").join(key).join("journal"),
            )
            .unwrap();
        // Loaded bytes alone never authorize a reconciled creation success.
        struct RefuseBarrier<'a>(
            &'a agent_spreadsheet::core::resident_storage::native::NativeResidentJournal,
        );
        #[async_trait::async_trait(?Send)]
        impl ResidentCommitStorage for RefuseBarrier<'_> {
            async fn load(
                &self,
                id: &str,
            ) -> anyhow::Result<
                Vec<agent_spreadsheet::core::resident_storage::PreparedResidentCommit>,
            > {
                self.0.load(id).await
            }
            async fn commit(
                &self,
                _: &agent_spreadsheet::core::resident_storage::PreparedResidentCommit,
            ) -> anyhow::Result<agent_spreadsheet::core::resident_storage::DurableCommitOutcome>
            {
                anyhow::bail!("read-only")
            }
            async fn reconcile(
                &self,
                _: &str,
                _: &str,
                _: &str,
            ) -> anyhow::Result<agent_spreadsheet::core::resident_storage::ReconcileOutcome>
            {
                anyhow::bail!("required creation reconciliation barrier failed")
            }
            fn outcome_retention(
                &self,
            ) -> agent_spreadsheet::core::resident_storage::OutcomeRetention {
                self.0.outcome_retention()
            }
        }
        let request =
            serde_json::from_value(json!({"resource_id":source_id,"expected_revision":revision}))
                .unwrap();
        let error = agent_spreadsheet::session::reconcile_creation(
            &RefuseBarrier(&journal),
            &serde_json::from_value(json!(resource)).unwrap(),
            revision,
            "create-one",
            &request,
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("required creation reconciliation barrier failed")
        );
        let records = journal.load(key).await.unwrap();
        assert_eq!(records[0].state_revision, initial);
        assert_eq!(records[0].request_id, "create-one");
        assert_eq!(records[0].effects.len(), 2);
        let mut forged = records[..1].to_vec();
        let outcome = forged[0]
            .effects
            .iter_mut()
            .find_map(|effect| effect.get_mut("canonical_outcome"))
            .unwrap();
        outcome["response"]["data"]["ttl_seconds"] = json!(3600);
        assert!(PortableHistoryState::replay(&forged).is_err());
        let mut forged = records[..1].to_vec();
        let creation = forged[0]
            .effects
            .iter_mut()
            .find_map(|effect| effect.get_mut("resource_creation"))
            .unwrap();
        creation["expected_revision"] = json!("forged-base");
        assert!(PortableHistoryState::replay(&forged).is_err());
    }
    let mut config: agent_spreadsheet::config::ServerConfig =
        serde_json::from_value(binding["config"].clone()).unwrap();
    config.cache_capacity += 1;
    assert!(
        client
            .request(&HostRequest::Configure { config })
            .await
            .unwrap_err()
            .to_string()
            .contains("authority differs")
    );
    // Actual process crash, followed by automatic bootstrap from the other alias.
    let pid = instance["pid"].as_u64().unwrap().to_string();
    assert!(
        Command::new("kill")
            .args(["-KILL", &pid])
            .status()
            .unwrap()
            .success()
    );
    {
        use fs2::FileExt;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join("host.lock"))
            .unwrap();
        let mut exited = false;
        for _ in 0..400 {
            if lock.try_lock_exclusive().is_ok() {
                exited = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(exited, "killed host still owns its lifetime lock");
        FileExt::unlock(&lock).unwrap();
    }
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_agent-spreadsheet"),
            "create_fork",
            Some("create-one"),
            None,
            payload
        ),
        created
    );
    let recovered = NativeHostClient::discover(&root).unwrap();
    let inactive_catalog = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "list_forks",
        None,
        None,
        json!({}),
    );
    assert_eq!(
        inactive_catalog["data"]["forks"].as_array().unwrap().len(),
        2
    );
    for fork in inactive_catalog["data"]["forks"].as_array().unwrap() {
        assert!(
            fork["revision_id"].is_null(),
            "inactive owner must not manufacture live CAS"
        );
        assert!(fork["age_seconds"].is_null());
    }
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_asp"),
            "write",
            Some("created-stage"),
            Some(resource),
            stage_payload
        ),
        staged
    );
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_agent-spreadsheet"),
            "checkpoint",
            Some("created-checkpoint"),
            Some(resource),
            checkpoint_payload
        ),
        checkpoint
    );
    assert_ne!(
        recovered.ping().await.unwrap()["instance"],
        instance["instance"]
    );
    let status = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "session_history",
        Some("recovered-status"),
        Some(resource),
        json!({"action":"status"}),
    );
    let continued = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "write",
        Some("continued-write"),
        Some(resource),
        json!({"expected_revision":status["data"]["revision_id"],"mode":"apply","atomic":true,"ops":[{"kind":"write_matrix","sheet_name":"Sheet1","anchor":"A1","rows":[[{"v":8}]]}]}),
    );
    invoke(
        env!("CARGO_BIN_EXE_asp"),
        "recalculate",
        Some("continued-calc"),
        Some(resource),
        json!({"expected_revision":continued["revision_id"]}),
    );
    let read = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "read_cells",
        Some("continued-read"),
        Some(resource),
        json!({"sheet_name":"Sheet1","selection":{"kind":"range","ranges":["B1"]},"format":"values"}),
    );
    assert_eq!(
        read["data"]["blocks"][0]["payload"]["values"][0][0],
        json!(24.0)
    );
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_agent-spreadsheet"),
            "export_fork",
            Some("export-one"),
            Some(resource),
            export_payload
        ),
        exported
    );
    assert_eq!(
        agent_spreadsheet::utils::hash_file_sha256_hex(&artifact_path).unwrap(),
        artifact["sha256"]
    );

    // Fork-from-fork captures the retained parent CAS, even after its external
    // source was deleted, and owns independent immutable base/configuration.
    let child_request = json!({"resource_id":resource,"expected_revision":read["revision_id"]});
    let child = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "create_fork",
        Some("child-one"),
        None,
        child_request.clone(),
    );
    let child_id = child["resource_id"].as_str().unwrap();
    let child_write = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "write",
        Some("child-write"),
        Some(child_id),
        json!({"expected_revision":child["revision_id"],"mode":"apply","atomic":true,"ops":[{"kind":"write_matrix","sheet_name":"Sheet1","anchor":"A1","rows":[[{"v":11}]]}]}),
    );
    invoke(
        env!("CARGO_BIN_EXE_asp"),
        "recalculate",
        Some("child-calc"),
        Some(child_id),
        json!({"expected_revision":child_write["revision_id"]}),
    );
    let child_read_request = json!({"sheet_name":"Sheet1","selection":{"kind":"range","ranges":["B1"]},"format":"values"});
    let child_read = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "read_cells",
        Some("child-read"),
        Some(child_id),
        child_read_request.clone(),
    );
    assert_eq!(
        child_read["data"]["blocks"][0]["payload"]["values"][0][0],
        33.0
    );
    let parent_read = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "read_cells",
        Some("parent-isolation"),
        Some(resource),
        child_read_request.clone(),
    );
    assert_eq!(
        parent_read["data"]["blocks"][0]["payload"]["values"][0][0],
        24.0
    );
    assert_eq!(parent_read["revision_id"], read["revision_id"]);

    // Destructive lifecycle is a terminal same-stream receipt, not deletion of
    // the retry authority. No further workbook access survives this action.
    let catalog = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "list_forks",
        None,
        None,
        json!({}),
    );
    let live = catalog["data"]["forks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|fork| fork["resource_id"] == resource)
        .unwrap();
    assert_eq!(live["revision_id"], read["revision_id"]);
    assert!(live["age_seconds"].is_null());
    let discard_payload = json!({"expected_revision":read["revision_id"]});
    let discarded = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "discard_fork",
        Some("discard-one"),
        Some(resource),
        discard_payload.clone(),
    );
    assert_eq!(discarded["data"]["discarded"], true);
    assert_eq!(
        discarded["revision_id"],
        discarded["data"]["revision_after"]
    );
    assert!(
        discarded["revision_id"]
            .as_str()
            .unwrap()
            .starts_with("discarded:")
    );
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_asp"),
            "discard_fork",
            Some("discard-one"),
            Some(resource),
            discard_payload.clone()
        ),
        discarded
    );
    let rejected = Command::new(env!("CARGO_BIN_EXE_asp"))
        .current_dir(directory.path()).env("ASP_RESIDENT_ROOT", &root).env_remove("ASP_REQUEST_ID")
        .args(["op", "read_cells", "--request-id", "discarded-read", "--bind", resource, "--json",
            &json!({"sheet_name":"Sheet1","selection":{"kind":"range","ranges":["A1"]},"format":"values"}).to_string()])
        .output().unwrap();
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("RESOURCE_NOT_FOUND"),
        "{} {}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    let catalog = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "list_forks",
        None,
        None,
        json!({}),
    );
    assert_eq!(catalog["data"]["forks"].as_array().unwrap().len(), 2);
    assert!(
        catalog["data"]["forks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fork| fork["resource_id"] == independent_id)
    );
    assert!(
        catalog["data"]["forks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fork| fork["resource_id"] == child_id)
    );
    let original = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "session_history",
        Some("discard-outcome"),
        Some(resource),
        json!({"action":"outcome","request_id":"discard-one"}),
    );
    assert_eq!(original["data"]["response"], discarded);
    assert!(original.get("revision_id").is_none_or(Value::is_null));
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_asp"),
            "export_fork",
            Some("export-one"),
            Some(resource),
            json!({"expected_revision":calculated["revision_id"],"destination":{"kind":"workspace","name":"immutable.xlsx"}})
        ),
        exported
    );
    let creation_request = json!({"resource_id":source_id,"expected_revision":revision});
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_asp"),
            "create_fork",
            Some("create-one"),
            None,
            creation_request.clone()
        ),
        created
    );
    recovered.request(&HostRequest::Shutdown).await.unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("host.lock"))
        .unwrap();
    let mut released = false;
    for _ in 0..400 {
        if !root.join("discovery.json").exists() && lock.try_lock_exclusive().is_ok() {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(released, "discard restart did not release host ownership");
    FileExt::unlock(&lock).unwrap();
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_asp"),
            "discard_fork",
            Some("discard-one"),
            Some(resource),
            discard_payload
        ),
        discarded
    );
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_asp"),
            "create_fork",
            Some("create-one"),
            None,
            creation_request
        ),
        created
    );
    assert_eq!(
        invoke(
            env!("CARGO_BIN_EXE_asp"),
            "create_fork",
            Some("child-one"),
            None,
            child_request
        ),
        child
    );
    let child_status = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "session_history",
        Some("child-recovered-status"),
        Some(child_id),
        json!({"action":"status"}),
    );
    invoke(
        env!("CARGO_BIN_EXE_asp"),
        "recalculate",
        Some("child-recovered-calc"),
        Some(child_id),
        json!({"expected_revision":child_status["data"]["revision_id"]}),
    );
    let child_after = invoke(
        env!("CARGO_BIN_EXE_asp"),
        "read_cells",
        Some("child-after-parent-discard"),
        Some(child_id),
        child_read_request,
    );
    assert_eq!(
        child_after["data"]["blocks"][0]["payload"]["values"][0][0],
        33.0
    );
    // Old creation success must not reactivate the tombstoned workbook.
    for (operation, identity, request) in [
        (
            "session_history",
            "discarded-status",
            json!({"action":"status"}),
        ),
        (
            "write",
            "discarded-write",
            json!({"expected_revision":read["revision_id"],"mode":"apply","atomic":true,"ops":[]}),
        ),
        (
            "discard_fork",
            "discard-one",
            json!({"expected_revision":"different"}),
        ),
        (
            "discard_fork",
            "discard-two",
            json!({"expected_revision":read["revision_id"]}),
        ),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_asp"))
            .current_dir(directory.path())
            .env("ASP_RESIDENT_ROOT", &root)
            .env_remove("ASP_REQUEST_ID")
            .args([
                "op",
                operation,
                "--request-id",
                identity,
                "--bind",
                resource,
                "--json",
                &request.to_string(),
            ])
            .output()
            .unwrap();
        assert!(
            !result.status.success(),
            "discarded operation unexpectedly succeeded: {operation}"
        );
    }
    {
        use agent_spreadsheet::core::resident_storage::{
            PortableHistoryState, ResidentCommitStorage,
        };
        let journal =
            agent_spreadsheet::core::resident_storage::native::NativeResidentJournal::open(
                root.join("resources").join(key).join("journal"),
            )
            .unwrap();
        let records = journal.load(key).await.unwrap();
        assert!(PortableHistoryState::replay(&records).unwrap().discarded);
        assert_eq!(
            records.last().unwrap().request_id,
            "discard-one",
            "restart/retry must not append after a tombstone"
        );
        let mut forged = records.clone();
        let last = forged.last_mut().unwrap();
        last.effects
            .iter_mut()
            .find_map(|effect| effect.get_mut("canonical_outcome"))
            .unwrap()["response"]["data"]["discarded"] = json!(false);
        assert!(PortableHistoryState::replay(&forged).is_err());
        let mut forged = records.clone();
        forged.push(records.last().unwrap().clone());
        assert!(
            PortableHistoryState::replay(&forged)
                .unwrap_err()
                .to_string()
                .contains("after resource discard")
        );
    }
}
