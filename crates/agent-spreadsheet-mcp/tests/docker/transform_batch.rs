//! Docker E2E tests for transform_batch (Phase 5).

use anyhow::Result;
use serde_json::json;

use crate::support::mcp::{McpTestClient, call_tool, cell_value, extract_json};

#[tokio::test]
async fn test_transform_batch_preview_and_apply_keeps_formulas() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("transform_batch.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.get_cell_mut("A1").set_value("x");
            sheet.get_cell_mut("B1").set_formula("A1".to_string());
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

    let preview = extract_json(
        &client
            .call_tool(call_tool(
                "transform_batch",
                json!({
                  "fork_id": fork_id,
                  "mode": "preview",
                  "ops": [{
                    "kind": "clear_range",
                    "sheet_name": "Sheet1",
                    "target": { "kind": "range", "range": "A1:B1" },
                    "clear_values": true,
                    "clear_formulas": false
                  }]
                }),
            ))
            .await?,
    )?;
    let change_id = preview["change_id"].as_str().unwrap();

    let page_before = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                  "workbook_or_fork_id": fork_id,
                  "sheet_name": "Sheet1",
                  "format": "full",
                  "start_row": 1,
                  "page_size": 1,
                  "columns": ["A", "B"],
                  "include_formulas": true,
                  "include_header": false
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value(&page_before, 0, 0).as_deref(), Some("x"));

    let _ = client
        .call_tool(call_tool(
            "apply_staged_change",
            json!({ "fork_id": fork_id, "change_id": change_id }),
        ))
        .await?;

    let page_after = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                  "workbook_or_fork_id": fork_id,
                  "sheet_name": "Sheet1",
                  "format": "full",
                  "start_row": 1,
                  "page_size": 1,
                  "columns": ["A", "B"],
                  "include_formulas": true,
                  "include_header": false
                }),
            ))
            .await?,
    )?;

    assert_eq!(cell_value(&page_after, 0, 0), None);
    assert_eq!(
        page_after["rows"][0]["cells"][1]["formula"].as_str(),
        Some("A1")
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_transform_batch_apply_emits_value_and_formula_diffs_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("transform_batch_diffs.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.get_cell_mut("A1").set_value("x");
            sheet.get_cell_mut("B1").set_formula("A1".to_string());
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
            "transform_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [
                {
                  "kind": "clear_range",
                  "sheet_name": "Sheet1",
                  "target": { "kind": "range", "range": "A1:A1" },
                  "clear_values": true,
                  "clear_formulas": false
                },
                {
                  "kind": "clear_range",
                  "sheet_name": "Sheet1",
                  "target": { "kind": "range", "range": "B1:B1" },
                  "clear_values": false,
                  "clear_formulas": true
                }
              ]
            }),
        ))
        .await?;

    let changeset = extract_json(
        &client
            .call_tool(call_tool("get_changeset", json!({ "fork_id": fork_id })))
            .await?,
    )?;

    let changes = changeset["changes"].as_array().unwrap();
    let mut saw_value = false;
    let mut saw_formula = false;
    for c in changes {
        let subtype = c
            .get("subtype")
            .or_else(|| c.get("diff").and_then(|d| d.get("subtype")))
            .and_then(|s| s.as_str());
        let change_type = c.get("type").and_then(|t| t.as_str());
        match (subtype, change_type) {
            (Some("value_edit"), _) => saw_value = true,
            (_, Some("deleted")) => saw_value = true,
            (Some("formula_edit"), _) => saw_formula = true,
            _ => {}
        }
    }

    assert!(
        saw_value,
        "expected value clear diff (value_edit or deleted), got {changeset:?}"
    );
    assert!(saw_formula, "expected formula_edit diff, got {changeset:?}");

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_transform_batch_region_target_clears_region_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("transform_batch_region.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.get_cell_mut("A1").set_value("x");
            sheet.get_cell_mut("B1").set_value("y");
            sheet.get_cell_mut("A2").set_value("z");
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

    let overview = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_overview",
                json!({ "workbook_or_fork_id": fork_id, "sheet_name": "Sheet1" }),
            ))
            .await?,
    )?;
    let regions = overview["detected_regions"].as_array().unwrap();
    let region_id = regions[0]["id"].as_u64().unwrap();

    let _ = client
        .call_tool(call_tool(
            "transform_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "clear_range",
                "sheet_name": "Sheet1",
                "target": { "kind": "region", "region_id": region_id },
                "clear_values": true,
                "clear_formulas": false
              }]
            }),
        ))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                  "workbook_or_fork_id": fork_id,
                  "sheet_name": "Sheet1",
                  "start_row": 1,
                  "page_size": 2,
                  "columns": ["A", "B"],
                  "include_formulas": false,
                  "include_header": false
                }),
            ))
            .await?,
    )?;

    assert_eq!(cell_value(&page, 0, 0), None);
    assert_eq!(cell_value(&page, 0, 1), None);
    assert_eq!(cell_value(&page, 1, 0), None);

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_transform_batch_fill_range_sets_values_and_preserves_formulas_in_docker() -> Result<()>
{
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("transform_batch_fill.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.get_cell_mut("A1").set_formula("1+1".to_string());
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
            "transform_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "fill_range",
                "sheet_name": "Sheet1",
                "target": { "kind": "range", "range": "A1:B1" },
                "value": "x",
                "is_formula": false,
                "overwrite_formulas": false
              }]
            }),
        ))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                  "workbook_or_fork_id": fork_id,
                  "sheet_name": "Sheet1",
                  "format": "full",
                  "start_row": 1,
                  "page_size": 1,
                  "columns": ["A", "B"],
                  "include_formulas": true,
                  "include_header": false
                }),
            ))
            .await?,
    )?;

    assert_eq!(page["rows"][0]["cells"][0]["formula"].as_str(), Some("1+1"));
    assert_eq!(cell_value(&page, 0, 1).as_deref(), Some("x"));

    let changeset = extract_json(
        &client
            .call_tool(call_tool("get_changeset", json!({ "fork_id": fork_id })))
            .await?,
    )?;
    let changes = changeset["changes"].as_array().unwrap();
    assert!(
        changes.iter().any(|c| {
            c.get("address").and_then(|a| a.as_str()) == Some("B1")
                && (c.get("subtype").and_then(|s| s.as_str()) == Some("value_edit")
                    || c.get("type").and_then(|t| t.as_str()) == Some("added"))
        }),
        "expected value diff on B1, got {changeset:?}"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_transform_batch_replace_in_range_emits_value_and_formula_edits_in_docker()
-> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("transform_batch_replace.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.get_cell_mut("A1").set_value("Foo");
            sheet.get_cell_mut("B1").set_formula("A1".to_string());
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
            "transform_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [
                {
                  "kind": "replace_in_range",
                  "sheet_name": "Sheet1",
                  "target": { "kind": "range", "range": "A1:A1" },
                  "find": "Foo",
                  "replace": "Bar",
                  "match_mode": "exact",
                  "case_sensitive": true,
                  "include_formulas": false
                },
                {
                  "kind": "replace_in_range",
                  "sheet_name": "Sheet1",
                  "target": { "kind": "range", "range": "B1:B1" },
                  "find": "A1",
                  "replace": "A2",
                  "match_mode": "exact",
                  "case_sensitive": true,
                  "include_formulas": true
                }
              ]
            }),
        ))
        .await?;

    let changeset = extract_json(
        &client
            .call_tool(call_tool("get_changeset", json!({ "fork_id": fork_id })))
            .await?,
    )?;

    let changes = changeset["changes"].as_array().unwrap();
    let mut saw_value = false;
    let mut saw_formula = false;
    for c in changes {
        let subtype = c
            .get("subtype")
            .or_else(|| c.get("diff").and_then(|d| d.get("subtype")))
            .and_then(|s| s.as_str());
        match subtype {
            Some("value_edit") => saw_value = true,
            Some("formula_edit") => saw_formula = true,
            _ => {}
        }
    }

    assert!(saw_value, "expected value_edit diff, got {changeset:?}");
    assert!(saw_formula, "expected formula_edit diff, got {changeset:?}");

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_transform_batch_fill_range_preview_stages_and_apply_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("transform_batch_fill_preview.xlsx", |book| {
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

    let preview = extract_json(
        &client
            .call_tool(call_tool(
                "transform_batch",
                json!({
                  "fork_id": fork_id,
                  "mode": "preview",
                  "ops": [{
                    "kind": "fill_range",
                    "sheet_name": "Sheet1",
                    "target": { "kind": "range", "range": "A1:B1" },
                    "value": "y",
                    "is_formula": false,
                    "overwrite_formulas": false
                  }]
                }),
            ))
            .await?,
    )?;
    let change_id = preview["change_id"].as_str().unwrap();

    let page_before = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                  "workbook_or_fork_id": fork_id,
                  "sheet_name": "Sheet1",
                  "start_row": 1,
                  "page_size": 1,
                  "columns": ["A", "B"],
                  "include_formulas": false,
                  "include_header": false
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value(&page_before, 0, 0).as_deref(), Some("x"));
    assert_eq!(cell_value(&page_before, 0, 1), None);

    let _ = client
        .call_tool(call_tool(
            "apply_staged_change",
            json!({ "fork_id": fork_id, "change_id": change_id }),
        ))
        .await?;

    let page_after = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                  "workbook_or_fork_id": fork_id,
                  "sheet_name": "Sheet1",
                  "start_row": 1,
                  "page_size": 1,
                  "columns": ["A", "B"],
                  "include_formulas": false,
                  "include_header": false
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value(&page_after, 0, 0).as_deref(), Some("y"));
    assert_eq!(cell_value(&page_after, 0, 1).as_deref(), Some("y"));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_transform_batch_replace_in_range_preview_stages_and_apply_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("transform_batch_replace_preview.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.get_cell_mut("A1").set_value("Foo");
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

    let preview = extract_json(
        &client
            .call_tool(call_tool(
                "transform_batch",
                json!({
                  "fork_id": fork_id,
                  "mode": "preview",
                  "ops": [{
                    "kind": "replace_in_range",
                    "sheet_name": "Sheet1",
                    "target": { "kind": "cells", "cells": ["A1"] },
                    "find": "Foo",
                    "replace": "Bar",
                    "match_mode": "exact",
                    "case_sensitive": true,
                    "include_formulas": false
                  }]
                }),
            ))
            .await?,
    )?;
    let change_id = preview["change_id"].as_str().unwrap();

    let page_before = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                  "workbook_or_fork_id": fork_id,
                  "sheet_name": "Sheet1",
                  "start_row": 1,
                  "page_size": 1,
                  "columns": ["A"],
                  "include_formulas": false,
                  "include_header": false
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value(&page_before, 0, 0).as_deref(), Some("Foo"));

    let _ = client
        .call_tool(call_tool(
            "apply_staged_change",
            json!({ "fork_id": fork_id, "change_id": change_id }),
        ))
        .await?;

    let page_after = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                  "workbook_or_fork_id": fork_id,
                  "sheet_name": "Sheet1",
                  "start_row": 1,
                  "page_size": 1,
                  "columns": ["A"],
                  "include_formulas": false,
                  "include_header": false
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value(&page_after, 0, 0).as_deref(), Some("Bar"));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_transform_batch_fill_range_region_target_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("transform_batch_fill_region.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.get_cell_mut("A1").set_value("x");
            sheet.get_cell_mut("B1").set_value("y");
            sheet.get_cell_mut("A2").set_value("z");
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

    let overview = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_overview",
                json!({ "workbook_or_fork_id": fork_id, "sheet_name": "Sheet1" }),
            ))
            .await?,
    )?;
    let regions = overview["detected_regions"].as_array().unwrap();
    let region_id = regions[0]["id"].as_u64().unwrap();

    let _ = client
        .call_tool(call_tool(
            "transform_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "fill_range",
                "sheet_name": "Sheet1",
                "target": { "kind": "region", "region_id": region_id },
                "value": "n",
                "is_formula": false,
                "overwrite_formulas": false
              }]
            }),
        ))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                  "workbook_or_fork_id": fork_id,
                  "sheet_name": "Sheet1",
                  "start_row": 1,
                  "page_size": 2,
                  "columns": ["A", "B"],
                  "include_formulas": false,
                  "include_header": false
                }),
            ))
            .await?,
    )?;

    assert_eq!(cell_value(&page, 0, 0).as_deref(), Some("n"));
    assert_eq!(cell_value(&page, 0, 1).as_deref(), Some("n"));
    assert_eq!(cell_value(&page, 1, 0).as_deref(), Some("n"));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_transform_batch_fill_range_overwrite_formulas_removes_formula_in_docker() -> Result<()>
{
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("transform_batch_fill_overwrite.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.get_cell_mut("A1").set_formula("1+1".to_string());
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
            "transform_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "fill_range",
                "sheet_name": "Sheet1",
                "target": { "kind": "cells", "cells": ["A1"] },
                "value": "x",
                "is_formula": false,
                "overwrite_formulas": true
              }]
            }),
        ))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                  "workbook_or_fork_id": fork_id,
                  "sheet_name": "Sheet1",
                  "format": "full",
                  "start_row": 1,
                  "page_size": 1,
                  "columns": ["A"],
                  "include_formulas": true,
                  "include_header": false
                }),
            ))
            .await?,
    )?;

    assert_eq!(cell_value(&page, 0, 0).as_deref(), Some("x"));
    assert!(
        page["rows"][0]["cells"][0]["formula"].is_null(),
        "expected formula to be cleared, got {page:?}"
    );

    let changeset = extract_json(
        &client
            .call_tool(call_tool("get_changeset", json!({ "fork_id": fork_id })))
            .await?,
    )?;
    let changes = changeset["changes"].as_array().unwrap();
    assert!(
        changes.iter().any(|c| {
            c.get("address").and_then(|a| a.as_str()) == Some("A1")
                && c.get("subtype").and_then(|s| s.as_str()) == Some("formula_edit")
        }),
        "expected formula_edit on A1, got {changeset:?}"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_get_changeset_paging_and_summary_only_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("changeset_paging.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.set_name("Sheet1");
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
            "transform_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "fill_range",
                "sheet_name": "Sheet1",
                "target": { "kind": "range", "range": "A1:J30" },
                "value": "x",
                "is_formula": false,
                "overwrite_formulas": false
              }]
            }),
        ))
        .await?;

    let page1 = extract_json(
        &client
            .call_tool(call_tool(
                "get_changeset",
                json!({
                  "fork_id": fork_id,
                  "limit": 5,
                  "offset": 0
                }),
            ))
            .await?,
    )?;

    assert_eq!(page1["changes"].as_array().unwrap().len(), 5);
    assert!(page1["summary"]["total_changes"].as_u64().unwrap() > 5);
    assert_eq!(page1["summary"]["returned_changes"].as_u64(), Some(5));
    assert_eq!(page1["summary"]["truncated"].as_bool(), Some(true));
    assert_eq!(page1["summary"]["next_offset"].as_u64(), Some(5));

    let summary_only = extract_json(
        &client
            .call_tool(call_tool(
                "get_changeset",
                json!({
                  "fork_id": fork_id,
                  "summary_only": true
                }),
            ))
            .await?,
    )?;

    assert_eq!(summary_only["changes"].as_array().unwrap().len(), 0);
    assert_eq!(
        summary_only["summary"]["returned_changes"].as_u64(),
        Some(0)
    );
    assert_eq!(
        summary_only["summary"]["total_changes"].as_u64(),
        page1["summary"]["total_changes"].as_u64()
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_get_changeset_exclude_recalc_result_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("changeset_filter.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.set_name("Sheet1");
            sheet.get_cell_mut("A1").set_value_number(1);
            sheet
                .get_cell_mut("B1")
                .set_formula("A1*2".to_string())
                .set_formula_result_default("2");
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
            "edit_batch",
            json!({
              "fork_id": fork_id,
              "sheet_name": "Sheet1",
              "edits": [
                {"address": "A1", "value": "2", "is_formula": false}
              ]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let filtered = extract_json(
        &client
            .call_tool(call_tool(
                "get_changeset",
                json!({
                  "fork_id": fork_id,
                  "exclude_subtypes": ["recalc_result"]
                }),
            ))
            .await?,
    )?;

    let changes = filtered["changes"].as_array().unwrap();
    assert!(changes.iter().all(|c| {
        c.get("subtype")
            .or_else(|| c.get("diff").and_then(|d| d.get("subtype")))
            .and_then(|s| s.as_str())
            != Some("recalc_result")
    }));
    assert!(
        changes.iter().any(|c| {
            c.get("address").and_then(|a| a.as_str()) == Some("A1")
                && (c
                    .get("subtype")
                    .or_else(|| c.get("diff").and_then(|d| d.get("subtype")))
                    .and_then(|s| s.as_str())
                    == Some("value_edit"))
        }),
        "expected A1 value_edit, got {filtered:?}"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_find_formula_defaults_and_paging_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("find_formula.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.set_name("Sheet1");
            for row in 1..=6 {
                sheet
                    .get_cell_mut((2, row))
                    .set_formula(format!("SUM(A{row}:A{row})"));
            }
        });

    let client = test.connect().await?;
    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let first_page = extract_json(
        &client
            .call_tool(call_tool(
                "find_formula",
                json!({
                  "workbook_or_fork_id": workbook_id,
                  "sheet_name": "Sheet1",
                  "query": "SUM(",
                  "limit": 2
                }),
            ))
            .await?,
    )?;

    assert_eq!(first_page["matches"].as_array().unwrap().len(), 2);
    assert_eq!(first_page["next_offset"].as_u64(), Some(2));
    assert_eq!(
        first_page["matches"][0]["context"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    let second_page = extract_json(
        &client
            .call_tool(call_tool(
                "find_formula",
                json!({
                  "workbook_or_fork_id": workbook_id,
                  "sheet_name": "Sheet1",
                  "query": "SUM(",
                  "limit": 2,
                  "offset": first_page["next_offset"].as_u64().unwrap()
                }),
            ))
            .await?,
    )?;

    assert!(!second_page["matches"].as_array().unwrap().is_empty());
    assert_ne!(
        first_page["matches"][0]["address"].as_str(),
        second_page["matches"][0]["address"].as_str()
    );

    client.cancel().await?;
    Ok(())
}
