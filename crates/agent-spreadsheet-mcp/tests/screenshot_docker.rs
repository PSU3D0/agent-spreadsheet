//! Integration tests for screenshot functionality via MCP protocol over Docker stdio.
#![cfg(feature = "docker-tests")]

mod support;

use anyhow::Result;
use serde_json::{Value, json};
use support::mcp::{McpTestClient, call_tool, cell_value_f64, extract_json};
use umya_spreadsheet::{Color, Style};

#[cfg(feature = "recalc")]
use base64::Engine;
#[cfg(feature = "recalc")]
use image::ImageFormat;

fn workbook_id_by_name(workbooks: &Value, name: &str) -> String {
    // Use slug instead of path since token-dense defaults hide paths
    let name_without_ext = name.strip_suffix(".xlsx").unwrap_or(name);
    workbooks["workbooks"]
        .as_array()
        .and_then(|arr| {
            arr.iter().find(|w| {
                w["slug"]
                    .as_str()
                    .map(|s| s == name_without_ext)
                    .unwrap_or(false)
            })
        })
        .and_then(|w| w["workbook_id"].as_str())
        .expect("workbook id by name")
        .to_string()
}

async fn screenshot_ok(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    workbook_id: &str,
    sheet_name: &str,
    range: &str,
) -> Result<Value> {
    let result = client
        .call_tool(call_tool(
            "screenshot_sheet",
            json!({
                "workbook_or_fork_id": workbook_id,
                "sheet_name": sheet_name,
                "range": range
            }),
        ))
        .await?;
    assert!(result.is_error != Some(true), "screenshot should succeed");
    let response = extract_json(&result)?;
    assert!(
        response["output_path"].as_str().unwrap().ends_with(".png"),
        "output should be PNG"
    );
    Ok(response)
}

async fn screenshot_ok_inline(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    workbook_id: &str,
    sheet_name: &str,
    range: &str,
) -> Result<(Value, Vec<u8>)> {
    let result = client
        .call_tool(call_tool(
            "screenshot_sheet",
            json!({
                "workbook_or_fork_id": workbook_id,
                "sheet_name": sheet_name,
                "range": range            }),
        ))
        .await?;

    assert!(result.is_error != Some(true), "screenshot should succeed");

    let image_data = result
        .content
        .iter()
        .find_map(|c| c.as_image())
        .filter(|img| img.mime_type == "image/png" && !img.data.is_empty())
        .expect("inline image content");

    #[cfg(feature = "recalc")]
    let png_bytes = base64::engine::general_purpose::STANDARD
        .decode(&image_data.data)
        .expect("decode base64 png");

    #[cfg(not(feature = "recalc"))]
    let png_bytes: Vec<u8> = Vec::new();

    let response = extract_json(&result)?;
    assert!(
        response["output_path"].as_str().unwrap().ends_with(".png"),
        "output should be PNG"
    );

    Ok((response, png_bytes))
}

// ============================================================================
// Screenshot Tests
// ============================================================================

#[tokio::test]
async fn test_screenshot_sheet_basic() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("screenshot_test.xlsx", |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("Data");
            sheet.get_cell_mut("A1").set_value("Header1");
            sheet.get_cell_mut("B1").set_value("Header2");
            sheet.get_cell_mut("C1").set_value("Header3");
            sheet.get_cell_mut("A2").set_value_number(100);
            sheet.get_cell_mut("B2").set_value_number(200);
            sheet.get_cell_mut("C2").set_value_number(300);
            sheet.get_cell_mut("A3").set_value("Text Value");
            sheet.get_cell_mut("B3").set_value_number(42.5);
            sheet.get_cell_mut("C3").set_value("More Text");

            let mut header_style = Style::default();
            header_style.set_background_color(Color::COLOR_DARKBLUE);
            let white_font = Color::default().set_argb(Color::COLOR_WHITE).to_owned();
            {
                let font = header_style.get_font_mut();
                font.set_bold(true);
                font.set_color(white_font);
            }
            sheet.set_style_by_range("A1:C1", header_style);
        });

    let client = test.connect().await?;

    let tools = client.list_all_tools().await?;
    assert!(
        tools.iter().any(|t| t.name == "screenshot_sheet"),
        "screenshot_sheet tool should be registered"
    );

    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let result = client
        .call_tool(call_tool(
            "screenshot_sheet",
            json!({
                "workbook_or_fork_id": workbook_id,
                "sheet_name": "Data",
                "range": "A1:C5"
            }),
        ))
        .await?;

    assert!(result.is_error != Some(true), "screenshot should succeed");

    let response = extract_json(&result)?;
    eprintln!("Screenshot response: {:?}", response);

    assert_eq!(response["sheet_name"], "Data");
    assert_eq!(response["range"], "A1:C5");
    assert!(
        response["output_path"].as_str().unwrap().ends_with(".png"),
        "output should be PNG"
    );
    assert!(
        response["size_bytes"].as_u64().unwrap() > 0,
        "file should have content"
    );
    assert!(
        response["duration_ms"].as_u64().is_some(),
        "duration should be reported"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_screenshot_sheet_concurrent_requests_are_safe() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("screenshot_concurrent.xlsx", |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("Data");
            sheet.get_cell_mut("A1").set_value("X");
            sheet.get_cell_mut("D1").set_value("Y");
            let mut fill = Style::default();
            fill.set_background_color("FFEEEEEE");
            sheet.set_style_by_range("A1:F10", fill);
        });

    let client = test.connect().await?;

    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbook_id_by_name(&workbooks, "screenshot_concurrent.xlsx");

    let req1 = client.call_tool(call_tool(
        "screenshot_sheet",
        json!({
            "workbook_or_fork_id": workbook_id,
            "sheet_name": "Data",
            "range": "A1:C5"
        }),
    ));
    let req2 = client.call_tool(call_tool(
        "screenshot_sheet",
        json!({
            "workbook_or_fork_id": workbook_id,
            "sheet_name": "Data",
            "range": "D1:F5"
        }),
    ));

    let (r1, r2) = tokio::join!(req1, req2);
    let r1 = r1?;
    let r2 = r2?;

    assert!(r1.is_error != Some(true), "first screenshot should succeed");
    assert!(
        r2.is_error != Some(true),
        "second screenshot should succeed"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_screenshot_sheet_inline_image_content() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("screenshot_inline_test.xlsx", |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("Data");
            sheet.get_cell_mut("A1").set_value("Header1");
            sheet.get_cell_mut("A2").set_value_number(1);
            sheet.get_cell_mut("B2").set_value_number(2);
        });

    let client = test.connect().await?;

    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbook_id_by_name(&workbooks, "screenshot_inline_test.xlsx");

    let _ = screenshot_ok_inline(&client, &workbook_id, "Data", "A1:C5").await?;

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
#[cfg(feature = "recalc")]
async fn test_screenshot_sheet_targets_requested_sheet_not_first() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("screenshot_multi_sheet.xlsx", |book| {
            let sheet1 = book.get_sheet_mut(&0).ok().unwrap();
            sheet1.set_name("First");
            sheet1.get_cell_mut("A1").set_value("FIRST");
            let mut red = Style::default();
            red.set_background_color("FFFF0000");
            sheet1.set_style_by_range("A1:M40", red);

            let sheet2 = book.new_sheet("Calculations").unwrap();
            sheet2.get_cell_mut("A1").set_value("CALC");
            let mut green = Style::default();
            green.set_background_color("FF00FF00");
            sheet2.set_style_by_range("A1:M40", green);
        });

    let client = test.connect().await?;

    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbook_id_by_name(&workbooks, "screenshot_multi_sheet.xlsx");

    let (_response, png_bytes) =
        screenshot_ok_inline(&client, &workbook_id, "Calculations", "A1:M40").await?;

    let img = image::load_from_memory_with_format(&png_bytes, ImageFormat::Png)?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();

    // Sample a grid; background dominates if we got the right sheet.
    let mut greenish = 0u32;
    let mut redish = 0u32;
    for yi in 1..=5 {
        for xi in 1..=5 {
            let x = (w * xi) / 6;
            let y = (h * yi) / 6;
            let p = rgba.get_pixel(x, y).0;
            let (r, g, b) = (p[0], p[1], p[2]);

            // Ignore dark pixels (gridlines/text).
            if r < 40 && g < 40 && b < 40 {
                continue;
            }
            if g > 150 && r < 120 {
                greenish += 1;
            }
            if r > 150 && g < 120 {
                redish += 1;
            }
        }
    }

    assert!(
        greenish > redish,
        "expected green sheet to dominate (greenish={greenish}, redish={redish})"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_screenshot_visual_scenarios_original_and_forked() -> Result<()> {
    let test = McpTestClient::new();

    // Sparse, tiny content.
    test.workspace().create_workbook("sparse.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value("X");
        sheet.get_cell_mut("A2").set_value_number(1);
        sheet.get_cell_mut("B3").set_value_number(123);
    });

    // Offset content inside a larger requested range.
    test.workspace().create_workbook("offset.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("E10").set_value("Start");
        sheet.get_cell_mut("F12").set_value_number(42);
        sheet.get_cell_mut("H20").set_value_number(999);
    });

    // Very wide table.
    test.workspace().create_workbook("wide.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Wide");
        for (i, col) in ('A'..='Z').enumerate() {
            let header_addr = format!("{}1", col);
            let value_addr = format!("{}2", col);
            sheet
                .get_cell_mut(header_addr.as_str())
                .set_value(format!("H{}", i + 1));
            sheet
                .get_cell_mut(value_addr.as_str())
                .set_value_number((i + 1) as f64);
        }
    });

    // Very tall table near row limit.
    test.workspace().create_workbook("tall.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Tall");
        sheet.get_cell_mut("A1").set_value("Idx");
        sheet.get_cell_mut("B1").set_value("Val1");
        sheet.get_cell_mut("C1").set_value("Val2");
        sheet.get_cell_mut("D1").set_value("Val3");
        for row in 2..=100 {
            sheet
                .get_cell_mut(format!("A{}", row).as_str())
                .set_value_number((row - 1) as f64);
            sheet
                .get_cell_mut(format!("B{}", row).as_str())
                .set_value_number((row * 2) as f64);
            sheet
                .get_cell_mut(format!("C{}", row).as_str())
                .set_value_number((row * 3) as f64);
            sheet
                .get_cell_mut(format!("D{}", row).as_str())
                .set_value_number((row * 4) as f64);
        }
    });

    // Filled background all the way to selection edges.
    test.workspace().create_workbook("filled_bg.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Filled");
        sheet.get_cell_mut("A1").set_value("Filled Region");
        sheet.get_cell_mut("B3").set_value_number(10);
        sheet.get_cell_mut("C4").set_value_number(20);
        let mut fill_style = Style::default();
        fill_style.set_background_color("FFEEEEEE");
        sheet.set_style_by_range("A1:F15", fill_style);
    });

    // Low-contrast text on white.
    test.workspace()
        .create_workbook("low_contrast.xlsx", |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("LowContrast");
            sheet.get_cell_mut("A1").set_value("Faint Header");
            sheet.get_cell_mut("A2").set_value_number(1);
            sheet.get_cell_mut("B2").set_value_number(2);
            let mut faint_style = Style::default();
            let faint = Color::default().set_argb_str("FFDDDDDD").to_owned();
            faint_style.get_font_mut().set_color(faint);
            sheet.set_style_by_range("A1:D5", faint_style);
        });

    // Merged header with long text.
    test.workspace().create_workbook("merged.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Merged");
        sheet.add_merge_cells("A1:D2");
        sheet
            .get_cell_mut("A1")
            .set_value("This is a merged header with a long description");
        sheet.get_cell_mut("A3").set_value("Row");
        sheet.get_cell_mut("B3").set_value("Value");
        for row in 4..=12 {
            sheet
                .get_cell_mut(format!("A{}", row).as_str())
                .set_value_number((row - 3) as f64);
            sheet
                .get_cell_mut(format!("B{}", row).as_str())
                .set_value_number((row * 5) as f64);
        }
    });

    // Border-only-ish region: strong filled border around sparse content.
    test.workspace()
        .create_workbook("border_only.xlsx", |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("BorderOnly");
            sheet.get_cell_mut("B3").set_value("Inside");
            let mut edge_style = Style::default();
            edge_style.set_background_color("FF666666");
            sheet.set_style_by_range("A1:D1", edge_style);
            let mut edge_style = Style::default();
            edge_style.set_background_color("FF666666");
            sheet.set_style_by_range("A1:A10", edge_style);
            let mut edge_style = Style::default();
            edge_style.set_background_color("FF666666");
            sheet.set_style_by_range("D1:D10", edge_style);
            let mut edge_style = Style::default();
            edge_style.set_background_color("FF666666");
            sheet.set_style_by_range("A10:D10", edge_style);
        });

    // Non-Latin / RTL text.
    test.workspace().create_workbook("non_latin.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Intl");
        sheet.get_cell_mut("A1").set_value("标题");
        sheet.get_cell_mut("B1").set_value("القيمة");
        sheet.get_cell_mut("C1").set_value("ヘッダー");
        sheet.get_cell_mut("A2").set_value("数据");
        sheet.get_cell_mut("B2").set_value_number(42);
        sheet.get_cell_mut("C2").set_value("テキスト");
    });

    // Pseudo-chart / colored blocks (stands in for charts/images).
    test.workspace()
        .create_workbook("colored_blocks.xlsx", |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("Blocks");
            sheet.get_cell_mut("A1").set_value("Metric");
            sheet.get_cell_mut("B1").set_value("Value");
            for row in 2..=8 {
                sheet
                    .get_cell_mut(format!("A{}", row).as_str())
                    .set_value(format!("M{}", row - 1));
                sheet
                    .get_cell_mut(format!("B{}", row).as_str())
                    .set_value_number((row * 7) as f64);
            }
            let mut block_style = Style::default();
            block_style.set_background_color("FFB3D9FF");
            sheet.set_style_by_range("C2:C8", block_style);
        });

    // Forked workbook: edits should be screenshot-able too.
    test.workspace().create_workbook("forked.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("Data");
        sheet.get_cell_mut("A1").set_value("Original");
        sheet.get_cell_mut("A2").set_value_number(10);
        sheet.get_cell_mut("B2").set_formula("A2*2");
        sheet.get_cell_mut("B2").set_formula_result_default("0");
    });

    let client = test.connect().await?;

    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;

    screenshot_ok(
        &client,
        &workbook_id_by_name(&workbooks, "sparse.xlsx"),
        "Data",
        "A1:B3",
    )
    .await?;
    screenshot_ok(
        &client,
        &workbook_id_by_name(&workbooks, "offset.xlsx"),
        "Data",
        "A1:H25",
    )
    .await?;
    screenshot_ok(
        &client,
        &workbook_id_by_name(&workbooks, "wide.xlsx"),
        "Wide",
        "A1:Z10",
    )
    .await?;
    screenshot_ok(
        &client,
        &workbook_id_by_name(&workbooks, "tall.xlsx"),
        "Tall",
        "A1:D100",
    )
    .await?;
    screenshot_ok(
        &client,
        &workbook_id_by_name(&workbooks, "filled_bg.xlsx"),
        "Filled",
        "A1:F15",
    )
    .await?;
    screenshot_ok(
        &client,
        &workbook_id_by_name(&workbooks, "low_contrast.xlsx"),
        "LowContrast",
        "A1:D5",
    )
    .await?;
    screenshot_ok(
        &client,
        &workbook_id_by_name(&workbooks, "merged.xlsx"),
        "Merged",
        "A1:D12",
    )
    .await?;
    screenshot_ok(
        &client,
        &workbook_id_by_name(&workbooks, "border_only.xlsx"),
        "BorderOnly",
        "A1:D10",
    )
    .await?;
    screenshot_ok(
        &client,
        &workbook_id_by_name(&workbooks, "non_latin.xlsx"),
        "Intl",
        "A1:C5",
    )
    .await?;
    screenshot_ok(
        &client,
        &workbook_id_by_name(&workbooks, "colored_blocks.xlsx"),
        "Blocks",
        "A1:D10",
    )
    .await?;

    let forked_workbook_id = workbook_id_by_name(&workbooks, "forked.xlsx");
    let fork = extract_json(
        &client
            .call_tool(call_tool(
                "create_fork",
                json!({ "workbook_or_fork_id": forked_workbook_id }),
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
                "edits": [{ "address": "A2", "value": "99", "is_formula": false }]
            }),
        ))
        .await?;
    client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let original_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": forked_workbook_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 3, "format": "full"
                }),
            ))
            .await?,
    )?;
    let fork_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Data",
                    "start_row": 1,
                    "page_size": 3, "format": "full"
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value_f64(&original_page, 1, 0), Some(10.0));
    assert_eq!(cell_value_f64(&fork_page, 1, 0), Some(99.0));

    screenshot_ok(&client, &forked_workbook_id, "Data", "A1:B3").await?;
    screenshot_ok(&client, fork_id, "Data", "A1:B3").await?;

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_screenshot_range_too_large() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("large_range.xlsx", |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("Data");
            sheet.get_cell_mut("A1").set_value("Test");
        });

    let client = test.connect().await?;

    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let result = client
        .call_tool(call_tool(
            "screenshot_sheet",
            json!({
                "workbook_or_fork_id": workbook_id,
                "sheet_name": "Data",
                "range": "A1:AF150"
            }),
        ))
        .await;

    assert!(result.is_err(), "oversized range should fail");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("too large for a single screenshot"),
        "error should explain tiling: {}",
        err_msg
    );
    assert!(
        err_msg.contains("A1:AD100"),
        "should suggest first tile: {err_msg}"
    );
    assert!(
        err_msg.contains("AE1:AF100"),
        "should suggest second tile: {err_msg}"
    );
    assert!(
        err_msg.contains("A101:AD150"),
        "should suggest third tile: {err_msg}"
    );
    assert!(
        err_msg.contains("AE101:AF150"),
        "should suggest fourth tile: {err_msg}"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_screenshot_pixel_guard_suggests_split() -> Result<()> {
    let test = McpTestClient::new()
        .with_env_override("SPREADSHEET_MCP_MAX_PNG_DIM_PX", "200")
        .with_env_override("SPREADSHEET_MCP_MAX_PNG_AREA_PX", "20000");
    test.workspace()
        .create_workbook("pixel_guard.xlsx", |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("Data");
            sheet.get_cell_mut("A1").set_value("Header1");
            sheet.get_cell_mut("B1").set_value("Header2");
            sheet.get_cell_mut("C1").set_value("Header3");
            sheet.get_cell_mut("A2").set_value_number(100);
            sheet.get_cell_mut("B2").set_value_number(200);
            sheet.get_cell_mut("C2").set_value_number(300);
        });

    let client = test.connect().await?;
    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let result = client
        .call_tool(call_tool(
            "screenshot_sheet",
            json!({
                "workbook_or_fork_id": workbook_id,
                "sheet_name": "Data",
                "range": "A1:C5"
            }),
        ))
        .await;

    assert!(result.is_err(), "pixel guard should reject oversized PNG");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("exceeding limits") || err_msg.contains("Rendered PNG"),
        "error should mention pixel limits: {err_msg}"
    );
    assert!(
        err_msg.contains("A1:C2"),
        "should suggest split tile 1: {err_msg}"
    );
    assert!(
        err_msg.contains("A3:C5"),
        "should suggest split tile 2: {err_msg}"
    );

    client.cancel().await?;

    Ok(())
}

#[tokio::test]
async fn test_screenshot_default_range() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("default_range.xlsx", |book| {
            let sheet = book.get_sheet_mut(&0).ok().unwrap();
            sheet.set_name("Sheet1");
            sheet.get_cell_mut("A1").set_value("Default Range Test");
        });

    let client = test.connect().await?;

    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let result = client
        .call_tool(call_tool(
            "screenshot_sheet",
            json!({
                "workbook_or_fork_id": workbook_id,
                "sheet_name": "Sheet1"
            }),
        ))
        .await?;

    assert!(result.is_error != Some(true), "default range should work");

    let response = extract_json(&result)?;
    assert_eq!(
        response["range"], "A1:M40",
        "default range should be A1:M40"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_screenshot_invalid_sheet() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace().create_workbook("valid.xlsx", |book| {
        let sheet = book.get_sheet_mut(&0).ok().unwrap();
        sheet.set_name("RealSheet");
    });

    let client = test.connect().await?;

    let workbooks = extract_json(
        &client
            .call_tool(call_tool("list_workbooks", json!({})))
            .await?,
    )?;
    let workbook_id = workbooks["workbooks"][0]["workbook_id"].as_str().unwrap();

    let result = client
        .call_tool(call_tool(
            "screenshot_sheet",
            json!({
                "workbook_or_fork_id": workbook_id,
                "sheet_name": "NonExistentSheet",
                "range": "A1:B5"
            }),
        ))
        .await;

    assert!(result.is_err(), "invalid sheet should fail");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not found") || err_msg.contains("NonExistentSheet"),
        "error should mention sheet not found: {}",
        err_msg
    );

    client.cancel().await?;
    Ok(())
}
