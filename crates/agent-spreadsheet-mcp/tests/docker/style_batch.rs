//! Docker E2E test for style_batch write parity (Phase 2).

use anyhow::Result;
use serde_json::json;

use crate::support::mcp::{McpTestClient, call_tool, extract_json};

#[tokio::test]
async fn test_style_batch_apply_emits_style_diff_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("style_batch.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.get_cell_mut("A1").set_value("x");
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

    let _ = client
        .call_tool(call_tool(
            "style_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "sheet_name": "Sheet1",
                "target": { "kind": "cells", "cells": ["A1"] },
                "patch": { "font": { "bold": true } },
                "op_mode": "merge"
              }]
            }),
        ))
        .await?;

    let changeset = extract_json(
        &client
            .call_tool(call_tool("get_changeset", json!({ "fork_id": fork_id })))
            .await?,
    )?;

    let changes = changeset["changes"].as_array().unwrap();
    assert!(
        changes.iter().any(|c| {
            // get_changeset flattens CellDiff into the change object.
            c.get("subtype")
                .or_else(|| c.get("diff").and_then(|d| d.get("subtype")))
                .and_then(|s| s.as_str())
                == Some("style_edit")
        }),
        "expected style_edit diff, got {changeset:?}"
    );

    Ok(())
}

#[tokio::test]
async fn test_style_batch_large_range_counts_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("large.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("x");
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

    let resp = extract_json(
        &client
            .call_tool(call_tool(
                "style_batch",
                json!({
                  "fork_id": fork_id,
                  "mode": "apply",
                  "ops": [{
                    "sheet_name": "Sheet1",
                    "target": { "kind": "range", "range": "A1:AD100" },
                    "patch": { "fill": { "kind": "pattern", "pattern_type": "solid", "foreground_color": "FFEEEEEE" } },
                    "op_mode": "set"
                  }]
                }),
            ))
            .await?,
    )?;

    let touched = resp["summary"]["counts"]["cells_touched"].as_u64().unwrap();
    assert_eq!(touched, 3000);

    Ok(())
}
