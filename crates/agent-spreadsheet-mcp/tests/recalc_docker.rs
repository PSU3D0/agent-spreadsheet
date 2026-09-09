//! Integration tests for recalc functionality via MCP protocol over Docker stdio.
//!
//! These tests spawn the full Docker container and communicate via the MCP protocol,
//! exercising the same pathway that real AI clients use.
#![cfg(feature = "docker-tests")]

mod support;

use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;
use support::mcp::{
    McpTestClient, call_tool, cell_error_type, cell_is_error, cell_value, cell_value_f64,
    extract_json,
};

// ============================================================================
// Basic Connectivity Test
// ============================================================================

#[tokio::test]
async fn test_mcp_stdio_basic_connectivity() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("test.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.get_cell_mut("A1").set_value("Hello");
        sheet.get_cell_mut("B1").set_value_number(42);
    });

    let client = test.connect().await?;

    let tools = client.list_all_tools().await?;
    assert!(!tools.is_empty(), "should have tools");
    assert!(tools.iter().any(|t| t.name == "list_workbooks"));
    assert!(tools.iter().any(|t| t.name == "create_fork"));
    assert!(tools.iter().any(|t| t.name == "recalculate"));

    let result = client
        .call_tool(call_tool("list_workbooks", json!({})))
        .await?;
    assert!(result.is_error != Some(true));

    client.cancel().await?;
    Ok(())
}

// ============================================================================
// Recalc Tests - Basic Formula Evaluation
// ============================================================================

#[tokio::test]
async fn test_recalc_sum_formula() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("sum_test.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value_number(100);
        sheet.get_cell_mut("A2").set_value_number(20);
        let sum_cell = sheet.get_cell_mut("A3");
        sum_cell.set_formula("SUM(A1:A2)");
        sum_cell.set_formula_result_default("0");
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    let recalc = client
        .call_tool(call_tool(
            "recalculate",
            json!({
                "fork_id": fork_id
            }),
        ))
        .await?;
    assert!(recalc.is_error != Some(true), "recalc should succeed");

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    assert_eq!(
        cell_value_f64(&page, 2, 0),
        Some(120.0),
        "A3 should be 120 (100 + 20)"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_recalc_cross_sheet_reference() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("cross_sheet.xlsx", |book| {
            let sheet1 = book.get_sheet_mut(&0).ok().unwrap();
            sheet1.set_name("Input");
            sheet1.get_cell_mut("A1").set_value_number(50);

            let sheet2 = book.new_sheet("Output").unwrap();
            let ref_cell = sheet2.get_cell_mut("A1");
            ref_cell.set_formula("Input!A1*2");
            ref_cell.set_formula_result_default("0");
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Output",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    assert_eq!(
        cell_value_f64(&page, 0, 0),
        Some(100.0),
        "Output!A1 should be 100 (50 * 2)"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_recalc_complex_formulas() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("complex.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Sheet1");

        sheet.get_cell_mut("A1").set_value_number(10);
        sheet.get_cell_mut("A2").set_value_number(20);
        sheet.get_cell_mut("A3").set_value_number(30);

        let avg = sheet.get_cell_mut("B1");
        avg.set_formula("AVERAGE(A1:A3)");
        avg.set_formula_result_default("0");

        let max = sheet.get_cell_mut("B2");
        max.set_formula("MAX(A1:A3)");
        max.set_formula_result_default("0");

        let min = sheet.get_cell_mut("B3");
        min.set_formula("MIN(A1:A3)");
        min.set_formula_result_default("0");

        let nested = sheet.get_cell_mut("B4");
        nested.set_formula("IF(B1>15,\"High\",\"Low\")");
        nested.set_formula_result_default("");
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Sheet1",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    assert_eq!(
        cell_value_f64(&page, 0, 1),
        Some(20.0),
        "B1 AVERAGE should be 20"
    );
    assert_eq!(
        cell_value_f64(&page, 1, 1),
        Some(30.0),
        "B2 MAX should be 30"
    );
    assert_eq!(
        cell_value_f64(&page, 2, 1),
        Some(10.0),
        "B3 MIN should be 10"
    );
    assert_eq!(
        cell_value(&page, 3, 1),
        Some("High".to_string()),
        "B4 IF should be High"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_recalc_chain_dependencies() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("chain.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Chain");

        sheet.get_cell_mut("A1").set_value_number(5);

        let b1 = sheet.get_cell_mut("B1");
        b1.set_formula("A1*2");
        b1.set_formula_result_default("0");

        let c1 = sheet.get_cell_mut("C1");
        c1.set_formula("B1*2");
        c1.set_formula_result_default("0");

        let d1 = sheet.get_cell_mut("D1");
        d1.set_formula("C1*2");
        d1.set_formula_result_default("0");
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Chain",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    assert_eq!(
        cell_value_f64(&page, 0, 1),
        Some(10.0),
        "B1 should be 10 (5*2)"
    );
    assert_eq!(
        cell_value_f64(&page, 0, 2),
        Some(20.0),
        "C1 should be 20 (10*2)"
    );
    assert_eq!(
        cell_value_f64(&page, 0, 3),
        Some(40.0),
        "D1 should be 40 (20*2)"
    );

    client.cancel().await?;
    Ok(())
}

// ============================================================================
// Edit + Recalc Workflow Tests
// ============================================================================

#[tokio::test]
async fn test_edit_and_recalc_workflow() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("edit_test.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value_number(100);
        sheet.get_cell_mut("A2").set_value_number(20);
        let sum_cell = sheet.get_cell_mut("A3");
        sum_cell.set_formula("SUM(A1:A2)");
        sum_cell.set_formula_result_default("0");
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork_id,
                "sheet_name": "Data",
                "edits": [
                    { "address": "A1", "value": "200", "is_formula": false }
                ]
            }),
        ))
        .await?;

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let changeset = extract_json(
        &client
            .call_tool(call_tool(
                "get_changeset",
                json!({
                    "fork_id": fork_id,
                    "sheet_name": "Data"
                }),
            ))
            .await?,
    )?;

    let changes = changeset["changes"].as_array().unwrap();
    assert!(!changes.is_empty(), "should have changes");

    let a1_change = changes.iter().find(|c| c["address"] == "A1");
    assert!(a1_change.is_some(), "should have A1 change");

    let a3_change = changes.iter().find(|c| c["address"] == "A3");
    assert!(a3_change.is_some(), "should have A3 change (recalc result)");

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    assert_eq!(cell_value_f64(&page, 0, 0), Some(200.0), "A1 should be 200");
    assert_eq!(
        cell_value_f64(&page, 2, 0),
        Some(220.0),
        "A3 should be 220 (200+20)"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_edit_formula_and_recalc() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("formula_edit.xlsx", |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("Data");
            sheet.get_cell_mut("A1").set_value_number(10);
            sheet.get_cell_mut("A2").set_value_number(20);
            let sum_cell = sheet.get_cell_mut("A3");
            sum_cell.set_formula("SUM(A1:A2)");
            sum_cell.set_formula_result_default("0");
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork_id,
                "sheet_name": "Data",
                "edits": [
                    { "address": "A3", "value": "A1*A2", "is_formula": true }
                ]
            }),
        ))
        .await?;

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    assert_eq!(
        cell_value_f64(&page, 2, 0),
        Some(200.0),
        "A3 should be 200 (10*20)"
    );

    client.cancel().await?;
    Ok(())
}

// ============================================================================
// Fork Management Tests
// ============================================================================

#[tokio::test]
async fn test_list_and_discard_forks() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("fork_test.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(1);
    });

    let client = test.connect().await?;

    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let fork1 = extract_json(
        &client
            .call_tool(call_tool(
                "create_fork",
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork1_id = fork1["fork_id"].as_str().unwrap();

    let fork2 = extract_json(
        &client
            .call_tool(call_tool(
                "create_fork",
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork2_id = fork2["fork_id"].as_str().unwrap();

    let forks = extract_json(&client.call_tool(call_tool("list_forks", json!({}))).await?)?;
    let fork_list = forks["forks"].as_array().unwrap();
    assert!(fork_list.len() >= 2, "should have at least 2 forks");

    client
        .call_tool(call_tool("discard_fork", json!({ "fork_id": fork1_id })))
        .await?;

    let forks_after = extract_json(&client.call_tool(call_tool("list_forks", json!({}))).await?)?;
    let fork_list_after = forks_after["forks"].as_array().unwrap();
    let has_fork1 = fork_list_after.iter().any(|f| f["fork_id"] == fork1_id);
    let has_fork2 = fork_list_after.iter().any(|f| f["fork_id"] == fork2_id);

    assert!(!has_fork1, "fork1 should be discarded");
    assert!(has_fork2, "fork2 should still exist");

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_get_edits_returns_applied_changes() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("edits_test.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value_number(1);
        sheet.get_cell_mut("A2").set_value_number(2);
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork_id,
                "sheet_name": "Data",
                "edits": [
                    { "address": "A1", "value": "100", "is_formula": false },
                    { "address": "A2", "value": "A1*2", "is_formula": true }
                ]
            }),
        ))
        .await?;

    let edits = extract_json(
        &client
            .call_tool(call_tool(
                "get_edits",
                json!({
                    "fork_id": fork_id
                }),
            ))
            .await?,
    )?;

    let edit_list = edits["edits"].as_array().unwrap();
    assert_eq!(edit_list.len(), 2, "should have 2 edits");

    client.cancel().await?;
    Ok(())
}

// ============================================================================
// Real Workbook Test (VLOOKUP)
// ============================================================================

#[tokio::test]
async fn test_vlookup_recalc_with_real_workbook() -> Result<()> {
    let source_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("stest/EHA DL 5.2.25 (2).xlsx");
    if !source_path.exists() {
        eprintln!(
            "Skipping test: source workbook not found at {:?}",
            source_path
        );
        return Ok(());
    }

    let test = McpTestClient::new();
    test.workspace()
        .copy_workbook(&source_path, "eha_test.xlsx");

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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork_id,
                "sheet_name": "Calculations",
                "edits": [
                    { "address": "C2", "value": "1234567890", "is_formula": false }
                ]
            }),
        ))
        .await?;

    let recalc_result = client
        .call_tool(call_tool(
            "recalculate",
            json!({
                "fork_id": fork_id,
                "timeout_ms": 60000
            }),
        ))
        .await?;
    assert!(
        recalc_result.is_error != Some(true),
        "recalc should succeed"
    );

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calculations",
                    "start_row": 1,
                    "page_size": 15,
                    "format": "full"
                }),
            ))
            .await?,
    )?;

    let c2 = cell_value(&page, 1, 2);
    let c3 = cell_value(&page, 2, 2);
    let c4 = cell_value(&page, 3, 2);
    let c11 = cell_value(&page, 10, 2);

    eprintln!(
        "After: C2={:?}, C3={:?}, C4={:?}, C11={:?}",
        c2, c3, c4, c11
    );

    assert_eq!(
        c2,
        Some("1234567890".to_string()),
        "C2 should be provider's ID"
    );
    assert_eq!(
        c3,
        Some("Provider1".to_string()),
        "C3 should be Provider1 (VLOOKUP result)"
    );
    assert_eq!(
        c4,
        Some("MD".to_string()),
        "C4 should be MD (provider's title)"
    );
    assert_eq!(
        c11,
        Some("Draw".to_string()),
        "C11 should be Draw (provider's model type)"
    );

    client.cancel().await?;
    Ok(())
}

// ============================================================================
// Edge Case Tests - Error Handling
// ============================================================================

#[tokio::test]
async fn test_recalc_division_by_zero_error() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("div_zero.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value_number(100);
        sheet.get_cell_mut("A2").set_value_number(0);
        let div = sheet.get_cell_mut("A3");
        div.set_formula("A1/A2");
        div.set_formula_result_default("0");
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    assert!(
        cell_is_error(&page, 2, 0),
        "A3 should be an error (division by zero)"
    );
    let err = cell_error_type(&page, 2, 0);
    eprintln!("Division by zero error type: {:?}", err);
    assert!(
        err.as_ref().map(|e| e.contains("DIV")).unwrap_or(false),
        "Should be #DIV/0! error"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_recalc_error_propagation() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("error_prop.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value_number(10);
        sheet.get_cell_mut("A2").set_value_number(0);

        let div = sheet.get_cell_mut("B1");
        div.set_formula("A1/A2");
        div.set_formula_result_default("0");

        let dep = sheet.get_cell_mut("C1");
        dep.set_formula("B1+100");
        dep.set_formula_result_default("0");

        let dep2 = sheet.get_cell_mut("D1");
        dep2.set_formula("C1*2");
        dep2.set_formula_result_default("0");
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    eprintln!("B1 error: {:?}", cell_error_type(&page, 0, 1));
    eprintln!("C1 error: {:?}", cell_error_type(&page, 0, 2));
    eprintln!("D1 error: {:?}", cell_error_type(&page, 0, 3));

    assert!(cell_is_error(&page, 0, 1), "B1 should be error (div/0)");
    assert!(cell_is_error(&page, 0, 2), "C1 should propagate error");
    assert!(cell_is_error(&page, 0, 3), "D1 should propagate error");

    client.cancel().await?;
    Ok(())
}

// ============================================================================
// Edge Case Tests - Date Arithmetic
// ============================================================================

#[tokio::test]
async fn test_recalc_date_arithmetic() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("dates.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");

        let date_cell = sheet.get_cell_mut("A1");
        date_cell.set_formula("DATE(2025,1,15)");
        date_cell.set_formula_result_default("0");

        let add_days = sheet.get_cell_mut("A2");
        add_days.set_formula("A1+30");
        add_days.set_formula_result_default("0");

        let diff = sheet.get_cell_mut("A3");
        diff.set_formula("A2-A1");
        diff.set_formula_result_default("0");

        let year = sheet.get_cell_mut("B1");
        year.set_formula("YEAR(A1)");
        year.set_formula_result_default("0");

        let month = sheet.get_cell_mut("B2");
        month.set_formula("MONTH(A2)");
        month.set_formula_result_default("0");
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    assert_eq!(
        cell_value_f64(&page, 2, 0),
        Some(30.0),
        "A3 date diff should be 30 days"
    );
    assert_eq!(
        cell_value_f64(&page, 0, 1),
        Some(2025.0),
        "B1 YEAR should be 2025"
    );
    assert_eq!(
        cell_value_f64(&page, 1, 1),
        Some(2.0),
        "B2 MONTH should be 2 (Feb)"
    );

    client.cancel().await?;
    Ok(())
}

// ============================================================================
// Edge Case Tests - Large Dataset
// ============================================================================

#[tokio::test]
async fn test_recalc_large_dataset_sumif() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("large.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");

        for i in 1..=500 {
            let category = if i % 3 == 0 {
                "A"
            } else if i % 3 == 1 {
                "B"
            } else {
                "C"
            };
            sheet.get_cell_mut(format!("A{}", i)).set_value(category);
            sheet
                .get_cell_mut(format!("B{}", i))
                .set_value_number(i as f64);
        }

        let sumif_a = sheet.get_cell_mut("D1");
        sumif_a.set_formula("SUMIF(A1:A500,\"A\",B1:B500)");
        sumif_a.set_formula_result_default("0");

        let sumif_b = sheet.get_cell_mut("D2");
        sumif_b.set_formula("SUMIF(A1:A500,\"B\",B1:B500)");
        sumif_b.set_formula_result_default("0");

        let countif_a = sheet.get_cell_mut("E1");
        countif_a.set_formula("COUNTIF(A1:A500,\"A\")");
        countif_a.set_formula_result_default("0");

        let avg = sheet.get_cell_mut("F1");
        avg.set_formula("AVERAGE(B1:B500)");
        avg.set_formula_result_default("0");
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    let recalc_result = extract_json(
        &client
            .call_tool(call_tool(
                "recalculate",
                json!({
                    "fork_id": fork_id,
                    "timeout_ms": 60000
                }),
            ))
            .await?,
    )?;

    let duration = recalc_result["duration_ms"].as_u64().unwrap_or(0);
    eprintln!("Large dataset recalc took {}ms", duration);

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    let countif_result = cell_value_f64(&page, 0, 4);
    eprintln!("COUNTIF(A) = {:?}", countif_result);
    assert!(countif_result.is_some(), "COUNTIF should return a number");
    assert!(
        countif_result.unwrap() > 100.0,
        "Should count many A values"
    );

    let avg_result = cell_value_f64(&page, 0, 5);
    eprintln!("AVERAGE = {:?}", avg_result);
    assert_eq!(avg_result, Some(250.5), "AVERAGE of 1..500 should be 250.5");

    client.cancel().await?;
    Ok(())
}

// ============================================================================
// Edge Case Tests - Multiple Batch Edits
// ============================================================================

#[tokio::test]
async fn test_recalc_multiple_batch_edits() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("multi_edit.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");

        for i in 1..=5 {
            sheet
                .get_cell_mut(format!("A{}", i))
                .set_value_number(i as f64);
        }

        let sum = sheet.get_cell_mut("B1");
        sum.set_formula("SUM(A1:A5)");
        sum.set_formula_result_default("0");

        let product = sheet.get_cell_mut("B2");
        product.set_formula("PRODUCT(A1:A5)");
        product.set_formula_result_default("0");

        let avg = sheet.get_cell_mut("B3");
        avg.set_formula("AVERAGE(A1:A5)");
        avg.set_formula_result_default("0");
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork_id,
                "sheet_name": "Data",
                "edits": [
                    { "address": "A1", "value": "10", "is_formula": false },
                    { "address": "A2", "value": "20", "is_formula": false },
                    { "address": "A3", "value": "30", "is_formula": false },
                    { "address": "A4", "value": "40", "is_formula": false },
                    { "address": "A5", "value": "50", "is_formula": false }
                ]
            }),
        ))
        .await?;

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    assert_eq!(
        cell_value_f64(&page, 0, 1),
        Some(150.0),
        "B1 SUM should be 150 (10+20+30+40+50)"
    );
    assert_eq!(
        cell_value_f64(&page, 1, 1),
        Some(12000000.0),
        "B2 PRODUCT should be 12000000"
    );
    assert_eq!(
        cell_value_f64(&page, 2, 1),
        Some(30.0),
        "B3 AVERAGE should be 30"
    );

    let changeset = extract_json(
        &client
            .call_tool(call_tool(
                "get_changeset",
                json!({
                    "fork_id": fork_id
                }),
            ))
            .await?,
    )?;

    let changes = changeset["changes"].as_array().unwrap();
    eprintln!("Total changes: {}", changes.len());
    assert!(
        changes.len() >= 8,
        "Should have changes for 5 inputs + 3 formulas"
    );

    client.cancel().await?;
    Ok(())
}

// ============================================================================
// Edge Case Tests - Concurrent Fork Isolation
// ============================================================================

#[tokio::test]
async fn test_concurrent_forks_isolation() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("isolation.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value_number(100);
        let formula = sheet.get_cell_mut("A2");
        formula.set_formula("A1*2");
        formula.set_formula_result_default("0");
    });

    let client = test.connect().await?;

    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let fork1 = extract_json(
        &client
            .call_tool(call_tool(
                "create_fork",
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork1_id = fork1["fork_id"].as_str().unwrap();

    let fork2 = extract_json(
        &client
            .call_tool(call_tool(
                "create_fork",
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork2_id = fork2["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork1_id,
                "sheet_name": "Data",
                "edits": [{ "address": "A1", "value": "500", "is_formula": false }]
            }),
        ))
        .await?;

    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork2_id,
                "sheet_name": "Data",
                "edits": [{ "address": "A1", "value": "1000", "is_formula": false }]
            }),
        ))
        .await?;

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork1_id })))
        .await?;
    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork2_id })))
        .await?;

    let page1 = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork1_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    let page2 = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork2_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    assert_eq!(
        cell_value_f64(&page1, 0, 0),
        Some(500.0),
        "Fork1 A1 should be 500"
    );
    assert_eq!(
        cell_value_f64(&page1, 1, 0),
        Some(1000.0),
        "Fork1 A2 should be 1000 (500*2)"
    );

    assert_eq!(
        cell_value_f64(&page2, 0, 0),
        Some(1000.0),
        "Fork2 A1 should be 1000"
    );
    assert_eq!(
        cell_value_f64(&page2, 1, 0),
        Some(2000.0),
        "Fork2 A2 should be 2000 (1000*2)"
    );

    let original_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": workbook_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    assert_eq!(
        cell_value_f64(&original_page, 0, 0),
        Some(100.0),
        "Original A1 should still be 100"
    );

    client.cancel().await?;
    Ok(())
}

// ============================================================================
// Edge Case Tests - Empty Cells and Blanks
// ============================================================================

#[tokio::test]
async fn test_recalc_empty_cells_in_range() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("blanks.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");

        sheet.get_cell_mut("A1").set_value_number(10);
        sheet.get_cell_mut("A3").set_value_number(30);
        sheet.get_cell_mut("A5").set_value_number(50);

        let sum = sheet.get_cell_mut("B1");
        sum.set_formula("SUM(A1:A5)");
        sum.set_formula_result_default("0");

        let count = sheet.get_cell_mut("B2");
        count.set_formula("COUNT(A1:A5)");
        count.set_formula_result_default("0");

        let counta = sheet.get_cell_mut("B3");
        counta.set_formula("COUNTA(A1:A5)");
        counta.set_formula_result_default("0");

        let countblank = sheet.get_cell_mut("B4");
        countblank.set_formula("COUNTBLANK(A1:A5)");
        countblank.set_formula_result_default("0");
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    assert_eq!(
        cell_value_f64(&page, 0, 1),
        Some(90.0),
        "SUM should be 90 (10+30+50)"
    );
    assert_eq!(
        cell_value_f64(&page, 1, 1),
        Some(3.0),
        "COUNT should be 3 (only numbers)"
    );
    assert_eq!(
        cell_value_f64(&page, 2, 1),
        Some(3.0),
        "COUNTA should be 3 (non-empty)"
    );
    assert_eq!(
        cell_value_f64(&page, 3, 1),
        Some(2.0),
        "COUNTBLANK should be 2 (A2, A4)"
    );

    client.cancel().await?;
    Ok(())
}

// ============================================================================
// Edge Case Tests - Text and Boolean in Formulas
// ============================================================================

#[tokio::test]
async fn test_recalc_text_and_boolean_handling() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("types.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");

        sheet.get_cell_mut("A1").set_value("Hello");
        sheet.get_cell_mut("A2").set_value("World");
        sheet.get_cell_mut("A3").set_value_bool(true);
        sheet.get_cell_mut("A4").set_value_bool(false);
        sheet.get_cell_mut("A5").set_value_number(42);

        let concat = sheet.get_cell_mut("B1");
        concat.set_formula("CONCATENATE(A1,\" \",A2)");
        concat.set_formula_result_default("");

        let len = sheet.get_cell_mut("B2");
        len.set_formula("LEN(A1)");
        len.set_formula_result_default("0");

        let and_result = sheet.get_cell_mut("B3");
        and_result.set_formula("AND(A3,A4)");
        and_result.set_formula_result_default("");

        let or_result = sheet.get_cell_mut("B4");
        or_result.set_formula("OR(A3,A4)");
        or_result.set_formula_result_default("");

        let istext = sheet.get_cell_mut("B5");
        istext.set_formula("ISTEXT(A1)");
        istext.set_formula_result_default("");
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
                json!({
                    "workbook_or_fork_id": workbook_id
                }),
            ))
            .await?,
    )?;
    let fork_id = fork["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    assert_eq!(
        cell_value(&page, 0, 1),
        Some("Hello World".to_string()),
        "CONCATENATE should work"
    );
    assert_eq!(
        cell_value_f64(&page, 1, 1),
        Some(5.0),
        "LEN(Hello) should be 5"
    );

    let and_val = cell_value(&page, 2, 1);
    eprintln!("AND result: {:?}", and_val);
    assert!(
        and_val == Some("false".to_string()) || and_val == Some("FALSE".to_string()),
        "AND(true,false) should be false"
    );

    let or_val = cell_value(&page, 3, 1);
    eprintln!("OR result: {:?}", or_val);
    assert!(
        or_val == Some("true".to_string()) || or_val == Some("TRUE".to_string()),
        "OR(true,false) should be true"
    );

    client.cancel().await?;
    Ok(())
}

// ============================================================================
// Save Fork Tests
// ============================================================================

#[tokio::test]
async fn test_save_fork_to_new_path() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("original.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value_number(100);
        let formula = sheet.get_cell_mut("A2");
        formula.set_formula("A1*2");
        formula.set_formula_result_default("0");
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

    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork_id,
                "sheet_name": "Data",
                "edits": [{ "address": "A1", "value": "500", "is_formula": false }]
            }),
        ))
        .await?;

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let save_result = extract_json(
        &client
            .call_tool(call_tool(
                "save_fork",
                json!({
                    "fork_id": fork_id,
                    "target_path": "saved_copy.xlsx"
                }),
            ))
            .await?,
    )?;

    assert_eq!(
        save_result["saved_to"].as_str().unwrap(),
        "/data/saved_copy.xlsx"
    );
    assert_eq!(save_result["fork_dropped"].as_bool(), Some(true));

    let saved_path = test.workspace().path("saved_copy.xlsx");
    assert!(saved_path.exists(), "saved file should exist");

    let book = umya_spreadsheet::reader::xlsx::read(&saved_path)?;
    let sheet = book.get_sheet_by_name("Data").ok().unwrap();
    assert_eq!(sheet.get_cell("A1").unwrap().get_value(), "500");
    assert_eq!(sheet.get_cell("A2").unwrap().get_value(), "1000");

    let original_path = test.workspace().path("original.xlsx");
    let original_book = umya_spreadsheet::reader::xlsx::read(&original_path)?;
    let original_sheet = original_book.get_sheet_by_name("Data").ok().unwrap();
    assert_eq!(
        original_sheet.get_cell("A1").unwrap().get_value(),
        "100",
        "original should be unchanged"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_save_fork_overwrite_blocked_by_default() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("no_overwrite.xlsx", |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.get_cell_mut("A1").set_value_number(1);
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

    let save_result = client
        .call_tool(call_tool(
            "save_fork",
            json!({
                "fork_id": fork_id
            }),
        ))
        .await;

    assert!(
        save_result.is_err(),
        "should fail without --allow-overwrite"
    );
    let err_msg = save_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("overwrite") || err_msg.contains("allow-overwrite"),
        "error should mention overwrite: {}",
        err_msg
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_save_fork_overwrite_with_flag() -> Result<()> {
    let test = McpTestClient::new().with_allow_overwrite();
    test.workspace()
        .create_workbook("overwritable.xlsx", |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("Data");
            sheet.get_cell_mut("A1").set_value_number(100);
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

    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork_id,
                "sheet_name": "Data",
                "edits": [{ "address": "A1", "value": "999", "is_formula": false }]
            }),
        ))
        .await?;

    let save_result = extract_json(
        &client
            .call_tool(call_tool("save_fork", json!({ "fork_id": fork_id })))
            .await?,
    )?;

    assert!(
        save_result["saved_to"]
            .as_str()
            .unwrap()
            .contains("overwritable.xlsx")
    );

    let path = test.workspace().path("overwritable.xlsx");
    let book = umya_spreadsheet::reader::xlsx::read(&path)?;
    let sheet = book.get_sheet_by_name("Data").ok().unwrap();
    assert_eq!(
        sheet.get_cell("A1").unwrap().get_value(),
        "999",
        "original should be overwritten"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_save_fork_drop_fork_false_keeps_fork() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("keep_fork.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value_number(1);
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

    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork_id,
                "sheet_name": "Data",
                "edits": [{ "address": "A1", "value": "100", "is_formula": false }]
            }),
        ))
        .await?;

    let save_result = extract_json(
        &client
            .call_tool(call_tool(
                "save_fork",
                json!({
                    "fork_id": fork_id,
                    "target_path": "v1.xlsx",
                    "drop_fork": false
                }),
            ))
            .await?,
    )?;

    assert_eq!(save_result["fork_dropped"].as_bool(), Some(false));

    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork_id,
                "sheet_name": "Data",
                "edits": [{ "address": "A1", "value": "200", "is_formula": false }]
            }),
        ))
        .await?;

    let save_result2 = extract_json(
        &client
            .call_tool(call_tool(
                "save_fork",
                json!({
                    "fork_id": fork_id,
                    "target_path": "v2.xlsx",
                    "drop_fork": true
                }),
            ))
            .await?,
    )?;

    assert_eq!(save_result2["fork_dropped"].as_bool(), Some(true));

    let v1 = umya_spreadsheet::reader::xlsx::read(test.workspace().path("v1.xlsx"))?;
    let v2 = umya_spreadsheet::reader::xlsx::read(test.workspace().path("v2.xlsx"))?;

    assert_eq!(
        v1.get_sheet_by_name("Data").ok()
            .unwrap()
            .get_cell("A1")
            .unwrap()
            .get_value(),
        "100"
    );
    assert_eq!(
        v2.get_sheet_by_name("Data").ok()
            .unwrap()
            .get_cell("A1")
            .unwrap()
            .get_value(),
        "200"
    );

    let list_forks_response = client.call_tool(call_tool("list_forks", json!({}))).await?;
    assert!(
        list_forks_response.is_error != Some(true),
        "list_forks returned an error: {list_forks_response:?}"
    );
    let forks = extract_json(&list_forks_response)?;
    let fork_list = if let Some(array) = forks.get("forks").and_then(Value::as_array) {
        array.to_owned()
    } else if let Some(array) = forks.as_array() {
        array.to_owned()
    } else if forks.as_object().is_some_and(|map| map.is_empty()) {
        Vec::new()
    } else {
        panic!("list_forks response has unsupported shape: {forks:?}");
    };
    let has_fork = fork_list.iter().any(|f| f["fork_id"] == fork_id);
    assert!(!has_fork, "fork should be dropped after second save");

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_save_fork_reject_outside_workspace() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("test.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(1);
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

    let save_result = client
        .call_tool(call_tool(
            "save_fork",
            json!({
                "fork_id": fork_id,
                "target_path": "/etc/passwd.xlsx"
            }),
        ))
        .await;

    assert!(save_result.is_err(), "should reject path outside workspace");
    let err_msg = save_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("workspace"),
        "error should mention workspace: {}",
        err_msg
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_save_fork_reject_non_xlsx() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("test.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(1);
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

    let save_result = client
        .call_tool(call_tool(
            "save_fork",
            json!({
                "fork_id": fork_id,
                "target_path": "output.csv"
            }),
        ))
        .await;

    assert!(save_result.is_err(), "should reject non-xlsx extension");
    let err_msg = save_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("xlsx"),
        "error should mention xlsx: {}",
        err_msg
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_save_then_load_as_new_original() -> Result<()> {
    let test = McpTestClient::new().with_allow_overwrite();
    test.workspace().create_workbook("evolving.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value_number(1);
        let sum = sheet.get_cell_mut("A2");
        sum.set_formula("A1*10");
        sum.set_formula_result_default("0");
    });

    let client = test.connect().await?;

    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();
    eprintln!("Initial workbook_id: {}", workbook_id);

    let fork1 = extract_json(
        &client
            .call_tool(call_tool(
                "create_fork",
                json!({ "workbook_or_fork_id": workbook_id }),
            ))
            .await?,
    )?;
    let fork1_id = fork1["fork_id"].as_str().unwrap();

    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork1_id,
                "sheet_name": "Data",
                "edits": [{ "address": "A1", "value": "5", "is_formula": false }]
            }),
        ))
        .await?;

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork1_id })))
        .await?;

    extract_json(
        &client
            .call_tool(call_tool("save_fork", json!({ "fork_id": fork1_id })))
            .await?,
    )?;
    eprintln!("Saved fork1 back to original (A1=5, A2=50)");

    let fork2 = extract_json(
        &client
            .call_tool(call_tool(
                "create_fork",
                json!({ "workbook_or_fork_id": workbook_id }),
            ))
            .await?,
    )?;
    let fork2_id = fork2["fork_id"].as_str().unwrap();
    eprintln!("Created fork2: {}", fork2_id);

    let page_before = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork2_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    let a1_before = cell_value_f64(&page_before, 0, 0);
    let a2_before = cell_value_f64(&page_before, 1, 0);
    eprintln!(
        "Fork2 initial state: A1={:?}, A2={:?}",
        a1_before, a2_before
    );

    assert_eq!(a1_before, Some(5.0), "Fork2 should see updated A1=5");
    assert_eq!(a2_before, Some(50.0), "Fork2 should see recalculated A2=50");

    client
        .call_tool(call_tool(
            "edit_batch",
            json!({
                "fork_id": fork2_id,
                "sheet_name": "Data",
                "edits": [{ "address": "A1", "value": "10", "is_formula": false }]
            }),
        ))
        .await?;

    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork2_id })))
        .await?;

    let page_after = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork2_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 10, "format": "full"
                }),
            ))
            .await?,
    )?;

    let a1_after = cell_value_f64(&page_after, 0, 0);
    let a2_after = cell_value_f64(&page_after, 1, 0);
    eprintln!("Fork2 after edit: A1={:?}, A2={:?}", a1_after, a2_after);

    assert_eq!(a1_after, Some(10.0), "Fork2 A1 should be 10");
    assert_eq!(a2_after, Some(100.0), "Fork2 A2 should be 100 (10*10)");

    client.cancel().await?;
    Ok(())
}

// Screenshot tests live in `tests/screenshot_docker.rs`.
