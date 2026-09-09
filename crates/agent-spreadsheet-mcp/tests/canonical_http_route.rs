//! Integration coverage for the canonical HTTP route (`/v1`) served beside `/mcp`.

use agent_spreadsheet_mcp::{ServerConfig, http_app, state::AppState};
use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::net::TcpListener;

mod support;

struct RunningServer {
    base: String,
    client: reqwest::Client,
    handle: tokio::task::JoinHandle<()>,
}

impl RunningServer {
    async fn start(config: ServerConfig) -> Result<Self> {
        let state = Arc::new(AppState::new(Arc::new(config)));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let app = http_app(state);
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            base: format!("http://{addr}"),
            client: reqwest::Client::new(),
            handle,
        })
    }

    async fn get(&self, path: &str) -> Result<(reqwest::StatusCode, reqwest::Response)> {
        let response = self
            .client
            .get(format!("{}{path}", self.base))
            .send()
            .await?;
        Ok((response.status(), response))
    }

    async fn get_json(&self, path: &str) -> Result<(reqwest::StatusCode, Value)> {
        let (status, response) = self.get(path).await?;
        Ok((status, response.json().await?))
    }

    async fn op(&self, operation: &str, body: Value) -> Result<(reqwest::StatusCode, Value)> {
        let response = self
            .client
            .post(format!("{}/v1/op/{operation}", self.base))
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        Ok((status, response.json().await?))
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn fixture(workspace: &support::TestWorkspace) {
    workspace.create_workbook("route.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.cell_mut((1, 1)).set_value("Name".to_string());
        sheet.cell_mut((2, 1)).set_value("Amount".to_string());
        sheet.cell_mut((1, 2)).set_value("Alpha".to_string());
        sheet.cell_mut((2, 2)).set_value_number(42_f64);
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_request_identity_rejects_even_stateless_calls() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let server = RunningServer::start(workspace.config_with(|_| {})).await?;
    for identity in [String::new(), "x".repeat(257)] {
        let response = server.client.post(format!("{}/v1/op/list_workbooks", server.base))
            .header("x-agent-spreadsheet-request-id", identity)
            .json(&json!({})).send().await?;
        assert_eq!(response.status(), 400);
        let body: Value = response.json().await?;
        assert_eq!(body["error"]["code"], "INVALID_REQUEST");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn canonical_route_serves_discovery_reads_and_error_statuses() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    fixture(&workspace);
    let server = RunningServer::start(workspace.config_with(|config| {
        config.recalc_enabled = true;
    }))
    .await?;

    // Discovery.
    let (status, operations) = server.get_json("/v1/operations").await?;
    assert_eq!(status, 200);
    let names = operations
        .as_array()
        .expect("discovery is an array")
        .iter()
        .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert!(
        names.contains(&"describe_workbook".to_string()),
        "{names:?}"
    );
    assert!(names.contains(&"list_workbooks".to_string()), "{names:?}");

    let (status, registry) = server.get_json("/v1/registry").await?;
    assert_eq!(status, 200);
    assert_eq!(registry["schema_version"], "1");
    assert!(registry["operations"].as_array().unwrap().len() >= names.len());
    assert!(registry["error_schema"].is_object());

    // describe_workbook round trip.
    let (status, listed) = server.op("list_workbooks", json!({})).await?;
    assert_eq!(status, 200, "{listed}");
    let resource_id = listed["data"]["workbooks"][0]["resource_id"]
        .as_str()
        .expect("discovered resource id")
        .to_string();

    let (status, described) = server
        .op("describe_workbook", json!({ "resource_id": resource_id }))
        .await?;
    assert_eq!(status, 200, "{described}");
    assert_eq!(described["schema_version"], "1");
    assert_eq!(described["operation"], "describe_workbook");
    assert_eq!(described["resource_id"], resource_id);
    let revision = described["revision_id"]
        .as_str()
        .expect("reads report a revision")
        .to_string();

    // Explicit invalid identities reject before bootstrap; missing identities
    // work through the actual default-client process runner.
    let response = server.client.post(format!("{}/v1/op/create_fork",server.base))
        .header("x-agent-spreadsheet-request-id", "")
        .json(&json!({"resource_id":resource_id,"expected_revision":revision})).send().await?;
    assert_eq!(response.status(), 400);
    let rejected: Value = response.json().await?;
    assert_eq!(rejected["error"]["code"], "INVALID_REQUEST");
    assert!(rejected["error"]["message"].as_str().unwrap().contains("identity"));

    // Unknown operation maps to 404.
    let (status, unknown) = server.op("not_a_real_operation", json!({})).await?;
    assert_eq!(status, 404, "{unknown}");
    assert_eq!(unknown["error"]["code"], "UNKNOWN_OPERATION");

    // Malformed input maps to 400.
    let (status, invalid) = server
        .op("describe_workbook", json!({ "resource_id": 17 }))
        .await?;
    assert_eq!(status, 400, "{invalid}");
    assert_eq!(invalid["error"]["code"], "INVALID_REQUEST");

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn disabled_operations_map_to_capability_unavailable() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    fixture(&workspace);
    let server = RunningServer::start(workspace.config_with(|config| {
        config.enabled_tools = Some(["list_workbooks".to_string()].into_iter().collect());
    }))
    .await?;

    let (status, allowed) = server.op("list_workbooks", json!({})).await?;
    assert_eq!(status, 200, "{allowed}");

    let (status, blocked) = server
        .op(
            "describe_workbook",
            json!({ "resource_id": "workbook:anything" }),
        )
        .await?;
    assert_eq!(status, 501, "{blocked}");
    assert_eq!(blocked["error"]["code"], "CAPABILITY_UNAVAILABLE");
    assert_eq!(blocked["error"]["operation"], "describe_workbook");

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unavailable_capabilities_map_to_capability_unavailable() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    fixture(&workspace);
    // recalc disabled => no fork registry => canonical writes are unavailable.
    let server = RunningServer::start(workspace.config()).await?;

    let (status, blocked) = server
        .op(
            "write",
            json!({
                "resource_id": "fork:missing",
                "expected_revision": "r",
                "mode": "preview",
                "ops": [{
                    "kind": "set_cells",
                    "sheet_name": "Sheet1",
                    "cells": {"A1": {"kind": "value", "value": 1}}
                }]
            }),
        )
        .await?;
    assert_eq!(status, 501, "{blocked}");
    assert_eq!(blocked["error"]["code"], "CAPABILITY_UNAVAILABLE");

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_route_serves_verified_bytes_only() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    fixture(&workspace);
    let artifacts = workspace.root().join("artifacts");
    std::fs::create_dir_all(&artifacts)?;

    let png = b"\x89PNG\r\n\x1a\nstub-artifact-bytes";
    let hex = format!("{:x}", Sha256::digest(png));
    std::fs::write(artifacts.join(format!("{hex}.png")), png)?;

    let tampered_hex = format!("{:x}", Sha256::digest(b"declared-content"));
    std::fs::write(
        artifacts.join(format!("{tampered_hex}.png")),
        b"other-content",
    )?;

    let server = RunningServer::start(workspace.config()).await?;

    // Valid handle serves image/png bytes.
    let (status, response) = server
        .get(&format!("/v1/artifacts/artifact:sha256:{hex}"))
        .await?;
    assert_eq!(status, 200);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .unwrap(),
        "image/png"
    );
    assert_eq!(response.bytes().await?.as_ref(), png);

    // Malformed handle: 400.
    let (status, body) = server.get_json("/v1/artifacts/not-a-handle").await?;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "INVALID_REQUEST");

    let (status, body) = server
        .get_json("/v1/artifacts/artifact:sha256:0123abc")
        .await?;
    assert_eq!(status, 400, "{body}");

    // Unknown handle: 404.
    let (status, body) = server
        .get_json(&format!("/v1/artifacts/artifact:sha256:{}", "b".repeat(64)))
        .await?;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["error"]["code"], "RESOURCE_NOT_FOUND");

    // Content that does not hash to its name is never served.
    let (status, body) = server
        .get_json(&format!("/v1/artifacts/artifact:sha256:{tampered_hex}"))
        .await?;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["error"]["code"], "RESOURCE_NOT_FOUND");

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn artifacts_from_another_workspace_are_not_served() -> Result<()> {
    let producer = support::TestWorkspace::new();
    let png = b"\x89PNG\r\n\x1a\nother-workspace";
    let hex = format!("{:x}", Sha256::digest(png));
    let artifacts = producer.root().join("artifacts");
    std::fs::create_dir_all(&artifacts)?;
    std::fs::write(artifacts.join(format!("{hex}.png")), png)?;

    let workspace = support::TestWorkspace::new();
    fixture(&workspace);
    std::fs::create_dir_all(workspace.root().join("artifacts"))?;
    let server = RunningServer::start(workspace.config()).await?;

    let (status, body) = server
        .get_json(&format!("/v1/artifacts/artifact:sha256:{hex}"))
        .await?;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["error"]["code"], "RESOURCE_NOT_FOUND");

    Ok(())
}
