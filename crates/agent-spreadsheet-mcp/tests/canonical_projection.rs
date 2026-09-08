use agent_spreadsheet::operations::{
    RuntimeCapabilities, operation_descriptor, operation_registry,
};
use anyhow::Result;
use rmcp::{
    ServiceExt,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde_json::json;
use std::collections::HashSet;
use std::process::Stdio;
use tokio::process::Command;

mod support;

use support::mcp::{call_tool, extract_json};

struct OwnedNativeHost(std::path::PathBuf);
impl Drop for OwnedNativeHost {
    fn drop(&mut self) {
        let root = self.0.clone();
        let _ = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(async {
                if let Ok(client) = agent_spreadsheet::native_host::NativeHostClient::discover(&root) {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), client.request(&agent_spreadsheet::native_host::HostRequest::Shutdown)).await;
                }
            });
            for _ in 0..400 {
                if !root.join("discovery.json").exists() { break; }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }).join();
    }
}

trait IdentifiedToolCall {
    async fn call_identified(&self, params: rmcp::model::CallToolRequestParam) -> Result<rmcp::model::CallToolResult>;
}
impl IdentifiedToolCall for rmcp::Peer<rmcp::RoleClient> {
    async fn call_identified(&self, params: rmcp::model::CallToolRequestParam) -> Result<rmcp::model::CallToolResult> {
        use rmcp::model::*;
        let meta = Meta(serde_json::Map::from_iter([("agent-spreadsheet/request-id".into(), json!(uuid::Uuid::new_v4().to_string()))]));
        let response = self.send_request_with_option(ClientRequest::CallToolRequest(CallToolRequest {
            method: Default::default(), params, extensions: Default::default(),
        }), rmcp::service::PeerRequestOptions { timeout: None, meta: Some(meta) }).await?.await_response().await?;
        match response { ServerResult::CallToolResult(value) => Ok(value), _ => anyhow::bail!("unexpected MCP response") }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn live_json_rpc_projects_every_available_canonical_descriptor() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("canonical.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").unwrap();
        sheet.get_cell_mut((1, 1)).set_value("Name".to_string());
        sheet.get_cell_mut((2, 1)).set_value("Amount".to_string());
        sheet.get_cell_mut((1, 2)).set_value("Alpha".to_string());
        sheet.get_cell_mut((2, 2)).set_value_number(42_f64);
        sheet.get_cell_mut((3, 2)).set_formula("B2*2");
    });

    let native_parent = tempfile::tempdir()?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(native_parent.path(), std::fs::Permissions::from_mode(0o700))?;
    }
    let native_root = agent_spreadsheet::native_host::provision_root(native_parent.path(), "host")?;
    let _host_cleanup = OwnedNativeHost(native_root.clone());
    let child_root = native_root.clone();
    let root = workspace.root().to_path_buf();
    let (transport, _stderr) = TokioChildProcess::builder(
        Command::new(env!("CARGO_BIN_EXE_agent-spreadsheet-mcp")).configure(move |command| {
            command.env("ASP_RESIDENT_ROOT", child_root);
            command.args([
                "--transport",
                "stdio",
                "--workspace-root",
                root.to_str().expect("UTF-8 workspace"),
                "--recalc-enabled",
                "--recalc-backend",
                "formualizer",
                "--vba-enabled",
            ]);
        }),
    )
    .stderr(Stdio::piped())
    .spawn()?;
    let client = ().serve(transport).await?;

    let tools = client.list_all_tools().await?;
    let actual = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<HashSet<_>>();
    let mut capabilities = RuntimeCapabilities::native();
    capabilities.screenshot_rendering = actual.contains("screenshot_sheet");
    capabilities.vba = true;
    capabilities.resident_history = cfg!(feature = "recalc-formualizer");
    let expected = operation_registry()
        .iter()
        .filter(|descriptor| descriptor.is_available(&capabilities))
        .map(|descriptor| descriptor.name)
        .collect::<HashSet<_>>();
    assert_eq!(actual, expected);
    assert_eq!(
        actual.len(),
        30 + usize::from(capabilities.screenshot_rendering) + usize::from(capabilities.resident_history)
    );
    assert!(!actual.contains("close_workbook"));

    for tool in &tools {
        let descriptor = operation_descriptor(&tool.name).expect("canonical descriptor");
        let marker = tool
            .meta
            .as_ref()
            .and_then(|meta| meta.0.get("agent-spreadsheet/canonical"))
            .expect("canonical marker must survive tools/list");
        assert_eq!(marker["schema_version"], "1");
        assert_eq!(marker["operation"], tool.name.as_ref());
        let mut canonical_schema = (descriptor.input_schema)();
        canonical_schema.as_object_mut().unwrap().remove("title");
        assert_eq!(tool.schema_as_json_value(), canonical_schema);
        assert!(
            tool.output_schema.is_none(),
            "output schema must stay stripped"
        );
        assert!(
            tool.description
                .as_deref()
                .unwrap_or_default()
                .contains("Risk"),
            "{} description must explain risk",
            tool.name
        );
        let annotations = tool.annotations.as_ref().expect("risk annotations");
        let expected_read_only =
            descriptor.risk_ceiling == agent_spreadsheet::operations::OperationRisk::Low;
        let expected_destructive = matches!(
            descriptor.risk_ceiling,
            agent_spreadsheet::operations::OperationRisk::High
                | agent_spreadsheet::operations::OperationRisk::Destructive
        );
        assert_eq!(annotations.read_only_hint, Some(expected_read_only));
        assert_eq!(annotations.destructive_hint, Some(expected_destructive));
        assert_eq!(annotations.open_world_hint, Some(false));

        let result = client.call_identified(call_tool(&tool.name, json!({}))).await?;
        let envelope = extract_json(&result)?;
        assert_eq!(envelope["schema_version"], "1", "{} envelope", tool.name);
        if result.is_error == Some(true) {
            assert_eq!(envelope["error"]["operation"], tool.name.as_ref());
        } else {
            assert_eq!(envelope["operation"], tool.name.as_ref());
        }
    }

    let list = client
        .call_identified(call_tool("list_workbooks", json!({})))
        .await?;
    assert_ne!(list.is_error, Some(true));
    let list = extract_json(&list)?;
    let resource_id = list["data"]["workbooks"][0]["resource_id"]
        .as_str()
        .expect("canonical resource id");
    assert!(resource_id.starts_with("wb:"));

    let sheets = client
        .call_identified(call_tool(
            "list_sheets",
            json!({"resource_id":resource_id,"include_bounds":true}),
        ))
        .await?;
    assert_ne!(sheets.is_error, Some(true));
    let sheets = extract_json(&sheets)?;
    assert_eq!(sheets["operation"], "list_sheets");
    assert_eq!(sheets["resource_id"], resource_id);
    assert!(sheets["revision_id"].is_string());
    assert_eq!(sheets["data"]["sheets"][0]["name"], "Sheet1");

    let source_revision = sheets["revision_id"].as_str().unwrap();
    let fork = client
        .call_identified(call_tool(
            "create_fork",
            json!({"resource_id":resource_id,"expected_revision":source_revision}),
        ))
        .await?;
    let fork = extract_json(&fork)?;
    let fork_id = fork["resource_id"].as_str().unwrap();
    let recalculated = client
        .call_identified(call_tool(
            "recalculate",
            json!({
                "resource_id":fork_id,
                "expected_revision":fork["revision_id"],
                "backend":"formualizer"
            }),
        ))
        .await?;
    let recalculated = extract_json(&recalculated)?;
    assert_eq!(recalculated["data"]["state"], "clean");
    let recalculated_revision = recalculated["revision_id"].as_str().unwrap();
    let read = client
        .call_identified(call_tool(
            "read_cells",
            json!({
                "resource_id":fork_id,
                "sheet_name":"Sheet1",
                "selection":{"kind":"range","ranges":["C2"]},
                "format":"values"
            }),
        ))
        .await?;
    let read = extract_json(&read)?;
    assert_eq!(read["revision_id"], recalculated_revision);
    assert_eq!(read["data"]["calculation"]["state"], "clean");
    assert_eq!(
        read["data"]["calculation"]["revision_id"],
        recalculated_revision
    );
    assert_eq!(
        read["data"]["blocks"][0]["payload"]["values"][0][0].as_f64(),
        Some(84.0)
    );

    let malformed = client
        .call_identified(call_tool("list_sheets", json!({"resource_id":42})))
        .await?;
    assert_eq!(malformed.is_error, Some(true));
    let malformed = extract_json(&malformed)?;
    assert_eq!(malformed["schema_version"], "1");
    assert_eq!(malformed["error"]["code"], "INVALID_REQUEST");
    assert_eq!(malformed["error"]["operation"], "list_sheets");

    let list_bytes = serde_json::to_vec(&tools)?.len();
    let baseline_tools = tools
        .iter()
        .filter(|tool| {
            !matches!(
                tool.name.as_ref(),
                "sheetport_manifest" | "execute_sheetport" | "inspect_vba" | "screenshot_sheet" | "session_history"
            )
        })
        .collect::<Vec<_>>();
    let baseline_bytes = serde_json::to_vec(&baseline_tools)?.len();
    assert_eq!(baseline_tools.len(), 27);
    eprintln!(
        "canonical baseline: 27 tools/{baseline_bytes} bytes; capability-backed: {}/{} bytes",
        tools.len(),
        list_bytes
    );
    // Budget raised from 71 KiB in 5004: `screenshot_sheet` gained an optional
    // `backend` input and an additive `renderer` / `fidelity` / `warnings` /
    // `calculation` output block, which costs about 2.5 KiB of schema across
    // the whole projection. Structured fidelity warnings are the point of the
    // ticket; the budget still holds the projection under 72 KiB.
    assert!(
        list_bytes < 72 * 1024,
        "canonical tools/list projection is {list_bytes} bytes"
    );

    agent_spreadsheet::native_host::NativeHostClient::discover(&native_root)?.request(&agent_spreadsheet::native_host::HostRequest::Shutdown).await?;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    client.cancel().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_compat_router_preserves_legacy_shared_routes() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    workspace.create_workbook("compat.xlsx", |_| {});
    let root = workspace.root().to_path_buf();
    let (transport, _stderr) = TokioChildProcess::builder(
        Command::new(env!("CARGO_BIN_EXE_agent-spreadsheet-mcp")).configure(move |command| {
            command.args([
                "--transport",
                "stdio",
                "--workspace-root",
                root.to_str().expect("UTF-8 workspace"),
                "--recalc-enabled",
                "--recalc-backend",
                "formualizer",
                "--slim-surface=false",
            ]);
        }),
    )
    .stderr(Stdio::piped())
    .spawn()?;
    let client = ().serve(transport).await?;

    let tools = client.list_all_tools().await?;
    let names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    assert!(names.contains(&"close_workbook"));
    assert!(names.contains(&"mutate_batch"));
    assert!(names.contains(&"read_cells"));
    assert_eq!(names.iter().collect::<HashSet<_>>().len(), names.len());

    let marked = tools
        .iter()
        .filter_map(|tool| {
            tool.meta
                .as_ref()
                .and_then(|meta| meta.0.get("agent-spreadsheet/canonical"))
                .map(|marker| (tool.name.as_ref(), marker))
        })
        .collect::<Vec<_>>();
    for (name, marker) in &marked {
        assert_eq!(marker["schema_version"], "1");
        assert_eq!(marker["operation"], *name);
    }
    for legacy_shared in [
        "list_workbooks",
        "describe_workbook",
        "list_sheets",
        "sheet_overview",
        "read_table",
        "named_ranges",
        "create_fork",
        "list_forks",
        "verify_workbook",
        "discard_fork",
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == legacy_shared)
            .expect("legacy shared route");
        assert!(tool.meta.is_none(), "{legacy_shared} must remain legacy");
    }
    if let Some(screenshot) = tools.iter().find(|tool| tool.name == "screenshot_sheet") {
        assert!(
            screenshot.meta.is_none(),
            "legacy screenshot must be unmarked"
        );
    }
    assert!(
        marked.iter().any(|(name, _)| *name == "read_cells"),
        "canonical-only tools remain discoverable in compatibility mode"
    );

    let list = client
        .call_identified(call_tool("list_workbooks", json!({"include_paths":false})))
        .await?;
    assert_ne!(list.is_error, Some(true));
    let list = extract_json(&list)?;
    assert!(list["workbooks"].is_array());
    assert!(list.get("schema_version").is_none());

    let canonical_only = client.call_identified(call_tool("read_cells", json!({}))).await?;
    assert_eq!(canonical_only.is_error, Some(true));
    assert_eq!(extract_json(&canonical_only)?["schema_version"], "1");

    client.cancel().await?;
    Ok(())
}
