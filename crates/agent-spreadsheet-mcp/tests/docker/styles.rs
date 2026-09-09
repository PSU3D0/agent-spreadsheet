//! Docker E2E test for style read parity (Phase 1).

use anyhow::Result;
use serde_json::json;

use crate::support::mcp::{McpTestClient, call_tool, extract_json};

#[tokio::test]
async fn test_sheet_styles_reports_descriptors_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("styles.xlsx", |book| {
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("Header");
        let style_a1 = sheet.get_style_mut("A1");
        style_a1.get_font_mut().set_bold(true);
        style_a1.get_number_format_mut().set_format_code("0.00");
    });

    let client = test.connect().await?;
    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let styles = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_styles",
                json!({
                    "workbook_or_fork_id": workbook_id,
                    "sheet_name": "Sheet1",
                    "include_descriptor": true
                }),
            ))
            .await?,
    )?;

    let items = styles["styles"].as_array().unwrap();
    assert!(!items.is_empty());
    let has_bold = items.iter().any(|style| {
        style["descriptor"]["font"]["bold"]
            .as_bool()
            .unwrap_or(false)
    });
    assert!(has_bold, "expected a bold/header style");

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_sheet_styles_truncates_large_style_counts_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("many_styles.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            for i in 0..205u32 {
                let row = i + 1;
                let addr = format!("A{row}");
                sheet.get_cell_mut(addr.as_str()).set_value_number(i as i32);
                let color = format!("FF{:02X}0000", (i % 256) as u8);
                sheet
                    .get_style_mut(addr.as_str())
                    .get_font_mut()
                    .get_color_mut()
                    .set_argb(color);
            }
        });

    let client = test.connect().await?;
    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let styles = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_styles",
                json!({ "workbook_or_fork_id": workbook_id, "sheet_name": "Sheet1" }),
            ))
            .await?,
    )?;

    assert!(styles["styles_truncated"].as_bool().unwrap_or(false));
    assert_eq!(styles["styles"].as_array().unwrap().len(), 200);
    assert!(styles["total_styles"].as_u64().unwrap_or(0) >= 205);

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_sheet_styles_reports_runs_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("styles_overview.xlsx", |book| {
            let sheet = book.get_sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.get_cell_mut("A1").set_value("a");
            sheet.get_cell_mut("B1").set_value("b");
            sheet.get_cell_mut("C1").set_value("c");
            sheet.get_style_mut("A1").get_font_mut().set_bold(true);
            sheet.get_style_mut("B1").get_font_mut().set_bold(true);
        });

    let client = test.connect().await?;
    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let styles = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_styles",
                json!({
                    "workbook_or_fork_id": workbook_id,
                    "sheet_name": "Sheet1",
                    "scope": { "kind": "range", "range": "A1:C1" },
                    "granularity": "runs",
                    "max_items": 100,
                    "summary_only": false,
                    "include_descriptor": true,
                    "include_ranges": true
                }),
            ))
            .await?,
    )?;

    let items = styles["styles"].as_array().unwrap();
    assert!(!items.is_empty());

    let has_bold = items.iter().any(|style| {
        style["descriptor"]["font"]["bold"]
            .as_bool()
            .unwrap_or(false)
            && style["cell_ranges"]
                .as_array()
                .is_some_and(|rs| rs.iter().any(|r| r.as_str() == Some("A1:B1")))
    });
    assert!(has_bold, "expected a bold run");

    client.cancel().await?;
    Ok(())
}
