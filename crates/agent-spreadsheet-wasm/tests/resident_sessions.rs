#![cfg(not(target_arch = "wasm32"))]

//! Resident-session invariants.
//!
//! The adapter keeps the parsed workbook alive per session instead of rebuilding
//! it from `session_bytes` on every read. These tests lock the properties that
//! makes safe: reads after a mutation observe the new revision and the new
//! values, `exportWorkbook` still returns exactly the stored session bytes, and
//! disposal releases the resident state along with the byte accounting.

use agent_spreadsheet::core::session::{SessionMatrixCell, SessionTransformOp};
use agent_spreadsheet_wasm::{MAX_SESSIONS, SessionApi, TransformBatchOptions};
use serde_json::{Value, json};

const WORKBOOK: &[u8] = include_bytes!("../../agent-spreadsheet/tests/fixtures/f1/baseline.xlsx");

async fn execute(api: &SessionApi, session: &str, operation: &str, params: Value) -> Value {
    let raw = api
        .execute_operation(session, operation, &params.to_string())
        .await
        .unwrap_or_else(|error| panic!("{operation} failed: {}", error.error.message));
    serde_json::from_str(&raw).expect("canonical response is JSON")
}

async fn read_a1(api: &SessionApi, session: &str) -> Value {
    execute(
        api,
        session,
        "read_cells",
        json!({
            "sheet_name": "Sheet1",
            "selection": { "kind": "range", "ranges": ["A1"] },
            "format": "dense"
        }),
    )
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn reads_after_a_write_observe_the_new_revision_and_values() {
    let api = SessionApi::new();
    let session = api.create_session(WORKBOOK).expect("session");

    let before = read_a1(&api, &session).await;
    let revision_before = before["revision_id"]
        .as_str()
        .expect("revision")
        .to_string();
    assert!(
        !serde_json::to_string(&before["data"])
            .unwrap()
            .contains("resident-marker"),
        "fixture already contains the marker value"
    );

    let written = execute(
        &api,
        &session,
        "write",
        json!({
            "expected_revision": revision_before,
            "mode": "apply",
            "ops": [{
                "kind": "set_cells",
                "sheet_name": "Sheet1",
                "cells": { "A1": { "kind": "value", "value": "resident-marker" } }
            }]
        }),
    )
    .await;
    assert_eq!(written["data"]["status"], json!("applied"));
    let revision_after = written["revision_id"]
        .as_str()
        .expect("revision")
        .to_string();
    assert_ne!(revision_before, revision_after);

    // Three consecutive reads all have to see post-write state, not a stale
    // resident parse of the pre-write bytes.
    for attempt in 0..3 {
        let after = read_a1(&api, &session).await;
        assert_eq!(
            after["revision_id"],
            json!(revision_after),
            "read {attempt} reported a stale revision"
        );
        assert_eq!(after["resource_id"], json!(session));
        assert!(
            serde_json::to_string(&after["data"])
                .unwrap()
                .contains("resident-marker"),
            "read {attempt} did not observe the applied write"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn reads_after_a_compatibility_mutation_observe_the_new_values() {
    let api = SessionApi::new();
    let session = api.create_session(WORKBOOK).expect("session");

    let before = read_a1(&api, &session).await;
    let revision_before = before["revision_id"]
        .as_str()
        .expect("revision")
        .to_string();

    api.transform_batch(
        &session,
        vec![SessionTransformOp::WriteMatrix {
            sheet_name: "Sheet1".to_string(),
            anchor: "A1".to_string(),
            rows: vec![vec![Some(SessionMatrixCell::Value(json!("legacy-marker")))]],
            overwrite_formulas: true,
        }],
        TransformBatchOptions::default(),
    )
    .expect("transform batch");

    let after = read_a1(&api, &session).await;
    assert_ne!(after["revision_id"], json!(revision_before));
    assert!(
        serde_json::to_string(&after["data"])
            .unwrap()
            .contains("legacy-marker"),
        "read did not observe the legacy transform"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn export_workbook_returns_the_stored_session_bytes_verbatim() {
    let api = SessionApi::new();
    let session = api.create_session(WORKBOOK).expect("session");

    let exported_before_reads = api.export_workbook(&session).expect("export");

    // Reads must not perturb the exported bytes, regardless of how many of them
    // share one resident parse.
    for _ in 0..3 {
        let _ = read_a1(&api, &session).await;
    }
    let exported_after_reads = api.export_workbook(&session).expect("export");
    assert_eq!(
        exported_before_reads, exported_after_reads,
        "reads changed the exported workbook bytes"
    );

    // Re-binding the exported bytes must produce the same bytes again, which is
    // the property `session_bytes` being the single source of truth gives us.
    let rebound = api.create_session(&exported_after_reads).expect("rebind");
    let rebound_bytes = api.export_workbook(&rebound).expect("export");
    assert_eq!(
        rebound_bytes,
        exported_after_reads,
        "round-tripping the exported bytes was not byte-identical"
    );
    assert!(api.dispose_session(&rebound).expect("dispose"));
    assert!(api.dispose_session(&session).expect("dispose"));
}

#[tokio::test(flavor = "current_thread")]
async fn disposing_a_session_frees_the_resident_state_and_its_memory_ceiling() {
    let api = SessionApi::new();

    // Fill the session table, touching each session so every one of them holds a
    // resident parse, then dispose them all.
    let mut sessions = Vec::new();
    for _ in 0..MAX_SESSIONS {
        let session = api.create_session(WORKBOOK).expect("session");
        let _ = read_a1(&api, &session).await;
        sessions.push(session);
    }
    assert!(
        api.create_session(WORKBOOK).is_err(),
        "session ceiling was not enforced"
    );

    for session in &sessions {
        assert!(api.dispose_session(session).expect("dispose"));
    }
    for session in &sessions {
        let error = api
            .execute_operation(session, "list_sheets", "{}")
            .await
            .expect_err("disposed session must not resolve");
        assert!(
            error.error.message.contains("not found"),
            "unexpected error after dispose: {}",
            error.error.message
        );
    }

    // The ceiling is available again, which is only true if dispose released
    // both the byte accounting and the resident state keyed by the same id.
    let reused = api.create_session(WORKBOOK).expect("session after dispose");
    let _ = read_a1(&api, &reused).await;
    assert!(api.dispose_session(&reused).expect("dispose"));
}
