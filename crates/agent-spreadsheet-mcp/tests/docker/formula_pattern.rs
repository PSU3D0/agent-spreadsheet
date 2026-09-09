//! Docker E2E tests for apply_formula_pattern (Phase 3).

use anyhow::Result;
use serde_json::json;

use crate::support::mcp::{McpTestClient, call_tool, cell_value_f64, extract_json};

#[tokio::test]
async fn test_apply_formula_pattern_recalc_fidelity_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("formula_pattern.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.get_cell_mut("A1").set_value_number(1);
            sheet.get_cell_mut("B1").set_value_number(2);
            sheet.get_cell_mut("A2").set_value_number(10);
            sheet.get_cell_mut("B2").set_value_number(20);
            sheet.get_cell_mut("A3").set_value_number(100);
            sheet.get_cell_mut("B3").set_value_number(200);
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
            "apply_formula_pattern",
            json!({
              "fork_id": fork_id,
              "sheet_name": "Sheet1",
              "target_range": "C1:C3",
              "anchor_cell": "C1",
              "base_formula": "A1+B1",
              "fill_direction": "down",
              "mode": "apply"
            }),
        ))
        .await?;

    let _ = client
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
                    "page_size": 3,
                    "columns": ["A", "B", "C"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;

    assert_eq!(cell_value_f64(&page, 0, 2), Some(3.0));
    assert_eq!(cell_value_f64(&page, 1, 2), Some(30.0));
    assert_eq!(cell_value_f64(&page, 2, 2), Some(300.0));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_apply_formula_pattern_2d_fill_resolves_dependencies_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("formula_pattern_2d.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.get_cell_mut("A1").set_value_number(1);
            sheet.get_cell_mut("B1").set_value_number(2);
            sheet.get_cell_mut("A2").set_value_number(10);
            sheet.get_cell_mut("B2").set_value_number(20);
            sheet.get_cell_mut("A3").set_value_number(100);
            sheet.get_cell_mut("B3").set_value_number(200);
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
            "apply_formula_pattern",
            json!({
              "fork_id": fork_id,
              "sheet_name": "Sheet1",
              "target_range": "C1:D3",
              "anchor_cell": "C1",
              "base_formula": "A1+B1",
              "fill_direction": "both",
              "mode": "apply"
            }),
        ))
        .await?;

    let _ = client
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
                    "page_size": 3,
                    "columns": ["C", "D"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;

    assert_eq!(cell_value_f64(&page, 0, 0), Some(3.0)); // C1 = A1+B1
    assert_eq!(cell_value_f64(&page, 0, 1), Some(5.0)); // D1 = B1+C1
    assert_eq!(cell_value_f64(&page, 1, 0), Some(30.0)); // C2 = A2+B2
    assert_eq!(cell_value_f64(&page, 1, 1), Some(50.0)); // D2 = B2+C2
    assert_eq!(cell_value_f64(&page, 2, 0), Some(300.0)); // C3 = A3+B3
    assert_eq!(cell_value_f64(&page, 2, 1), Some(500.0)); // D3 = B3+C3

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_apply_formula_pattern_abs_rows_freezes_row_offsets_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("formula_pattern_abs_rows.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.get_cell_mut("A1").set_value_number(1);
            sheet.get_cell_mut("B1").set_value_number(2);
            sheet.get_cell_mut("A2").set_value_number(10);
            sheet.get_cell_mut("B2").set_value_number(20);
            sheet.get_cell_mut("A3").set_value_number(100);
            sheet.get_cell_mut("B3").set_value_number(200);
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
            "apply_formula_pattern",
            json!({
              "fork_id": fork_id,
              "sheet_name": "Sheet1",
              "target_range": "C1:C3",
              "anchor_cell": "C1",
              "base_formula": "A1+B1",
              "fill_direction": "down",
              "relative_mode": "abs_rows",
              "mode": "apply"
            }),
        ))
        .await?;

    let _ = client
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
                    "page_size": 3,
                    "columns": ["C"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;

    // Rows are frozen, so every row references A1+B1.
    assert_eq!(cell_value_f64(&page, 0, 0), Some(3.0));
    assert_eq!(cell_value_f64(&page, 1, 0), Some(3.0));
    assert_eq!(cell_value_f64(&page, 2, 0), Some(3.0));

    client.cancel().await?;
    Ok(())
}
