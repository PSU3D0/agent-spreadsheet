//! Docker E2E tests for staging/checkpoint Phase 0.

use anyhow::Result;
use serde_json::json;

use crate::support::mcp::{McpTestClient, call_tool, extract_json};

#[tokio::test]
async fn test_checkpoint_restore_roundtrip() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("checkpoint.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value_number(1);
        let out = sheet.get_cell_mut("A2");
        out.set_formula("A1*10");
        out.set_formula_result_default("0");
    });

    let client = test.connect().await?;

    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let fork = extract_json(
        &client
            .call_tool(call_tool(
                "create_fork",
                json!({ "workbook_or_fork_id": workbook_id }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    // First edit before checkpoint.
    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork_id,
                "sheet_name": "Data",
                "edits": [{ "address": "A1", "value": "5", "is_formula": false }]
            }),
        ))
        .await?;

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let checkpoint = extract_json(
        &client
            .call_tool(call_tool(
                "checkpoint_fork",
                json!({ "fork_id": fork_id, "label": "after-first-edit" }),
            ))
            .await?,
    )?;
    let checkpoint_id = checkpoint["checkpoint"]["checkpoint_id"].as_str().unwrap();

    // Second edit after checkpoint.
    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork_id,
                "sheet_name": "Data",
                "edits": [{ "address": "A1", "value": "10", "is_formula": false }]
            }),
        ))
        .await?;

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    // Restore checkpoint; should revert to A1=5/A2=50.
    client
        .call_tool(call_tool(
            "restore_checkpoint",
            json!({ "fork_id": fork_id, "checkpoint_id": checkpoint_id }),
        ))
        .await?;

    let changeset = extract_json(
        &client
            .call_tool(call_tool(
                "get_changeset",
                json!({ "fork_id": fork_id, "sheet_name": "Data" }),
            ))
            .await?,
    )?;

    let changes = changeset["changes"].as_array().unwrap();
    let a1 = changes.iter().find(|c| c["address"] == "A1").unwrap();
    assert_eq!(a1["type"], "modified");
    assert_eq!(a1["subtype"], "value_edit");
    let a1_val = a1["new_value"].as_str().unwrap_or_default();
    assert!(a1_val == "5" || a1_val == "5.0");

    let a2 = changes.iter().find(|c| c["address"] == "A2").unwrap();
    assert_eq!(a2["type"], "modified");
    assert_eq!(a2["subtype"], "recalc_result");
    let a2_val = a2["new_value"].as_str().unwrap_or_default();
    assert!(a2_val == "50" || a2_val == "50.0");

    client.cancel().await?;
    Ok(())
}
