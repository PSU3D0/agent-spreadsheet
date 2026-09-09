//! Docker E2E tests for workbook_style_summary (Phase 1).

use anyhow::Result;
use serde_json::json;
use std::collections::HashSet;
use umya_spreadsheet::{
    ConditionalFormatValues, ConditionalFormatting, ConditionalFormattingRule, Formula,
};

use crate::support::mcp::{McpTestClient, call_tool, extract_json};

#[tokio::test]
async fn test_workbook_style_summary_reports_theme_and_conditional_formats_in_docker() -> Result<()>
{
    let test = McpTestClient::new();
    test.workspace().create_workbook("wb_styles.xlsx", |book| {
        let sheet1 = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        for col in ['A', 'B', 'C'] {
            let addr = format!("{col}1");
            sheet1.cell_mut(addr.as_str()).set_value("x");
        }
        sheet1.cell_mut("D1").set_value("Header");
        sheet1.style_mut("D1").font_mut().set_bold(true);

        book.new_sheet("Sheet2").unwrap();
        let sheet2 = book.sheet_by_name_mut("Sheet2").ok().unwrap();
        sheet2.cell_mut("A1").set_value_number(1);

        let mut cf = ConditionalFormatting::default();
        cf.sequence_of_references_mut().set_sqref("A1:A3");
        let mut rule = ConditionalFormattingRule::default();
        rule.set_type(ConditionalFormatValues::Expression);
        rule.set_priority(1);
        let mut formula = Formula::default();
        formula.set_string_value("A1>0");
        rule.set_formula(formula);
        cf.add_conditional_collection(rule);
        sheet2.add_conditional_formatting_collection(cf);
    });

    let client = test.connect().await?;
    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let summary = extract_json(
        &client
            .call_tool(call_tool(
                "workbook_style_summary",
                json!({ "workbook_or_fork_id": workbook_id, "summary_only": false }),
            ))
            .await?,
    )?;

    assert!(
        summary["theme"]["colors"]
            .as_object()
            .map(|m| !m.is_empty())
            .unwrap_or(false),
        "expected theme colors"
    );
    assert!(
        summary["styles"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "expected styles"
    );

    let cf = summary["conditional_formats"].as_array().unwrap();
    assert_eq!(cf.len(), 1);
    assert_eq!(cf[0]["sheet_name"].as_str().unwrap(), "Sheet2");

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_workbook_style_summary_works_on_forks_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("fork_styles.xlsx", |book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.cell_mut("A1").set_value("x");
            sheet.cell_mut("A2").set_value("y");
        });

    let client = test.connect().await?;
    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let base_summary = extract_json(
        &client
            .call_tool(call_tool(
                "workbook_style_summary",
                json!({ "workbook_or_fork_id": workbook_id }),
            ))
            .await?,
    )?;

    let base_ids: HashSet<String> = base_summary["styles"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|s| s["style_id"].as_str().map(|v| v.to_string()))
        .collect();

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
                "target": { "kind": "cells", "cells": ["A2"] },
                "patch": { "font": { "italic": true } },
                "op_mode": "merge"
              }]
            }),
        ))
        .await?;

    let fork_summary = extract_json(
        &client
            .call_tool(call_tool(
                "workbook_style_summary",
                json!({ "workbook_or_fork_id": fork_id }),
            ))
            .await?,
    )?;

    let fork_ids: HashSet<String> = fork_summary["styles"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|s| s["style_id"].as_str().map(|v| v.to_string()))
        .collect();

    assert!(
        fork_ids.difference(&base_ids).next().is_some(),
        "expected fork to introduce a new style"
    );

    client.cancel().await?;
    Ok(())
}
