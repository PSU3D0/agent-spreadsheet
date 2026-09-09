//! Docker E2E tests for structure_batch (Phase 4).

use anyhow::Result;
use serde_json::json;

#[cfg(feature = "recalc")]
use anyhow::{anyhow, bail};
#[cfg(feature = "recalc")]
use quick_xml::{Reader, events::Event};
#[cfg(feature = "recalc")]
use regex::Regex;
#[cfg(feature = "recalc")]
use std::fs::{self, File};
#[cfg(feature = "recalc")]
use std::io::{Cursor, Read, Write};
#[cfg(feature = "recalc")]
use std::path::Path;
#[cfg(feature = "recalc")]
use zip::{ZipArchive, ZipWriter, write::FileOptions};

use crate::support::mcp::{
    McpTestClient, call_tool, cell_is_error, cell_value, cell_value_f64, extract_json,
};

#[cfg(feature = "recalc")]
fn xml_escape_attr(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(feature = "recalc")]
fn xml_escape_text(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(feature = "recalc")]
fn inject_defined_names(path: &Path, entries: &[(&str, &str)]) -> Result<()> {
    let src_bytes = fs::read(path)?;
    let mut archive = ZipArchive::new(Cursor::new(src_bytes))?;

    let tmp_path = path.with_extension("xlsx.tmp");
    let tmp_file = File::create(&tmp_path)?;
    let mut writer = ZipWriter::new(tmp_file);

    let options = FileOptions::default();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name().to_string();
        if name.ends_with('/') {
            writer.add_directory(name, options)?;
            continue;
        }

        let mut data = Vec::new();
        file.read_to_end(&mut data)?;

        writer.start_file(name.clone(), options)?;
        if name == "xl/workbook.xml" {
            let mut xml =
                String::from_utf8(data).map_err(|e| anyhow!("workbook.xml not utf8: {}", e))?;

            let mut insert = String::new();
            for (name, refers_to) in entries {
                insert.push_str(&format!(
                    "<definedName name=\"{}\">{}</definedName>",
                    xml_escape_attr(name),
                    xml_escape_text(refers_to)
                ));
            }

            if let Some(pos) = xml.rfind("</definedNames>") {
                xml.insert_str(pos, &insert);
            } else if let Some(pos) = xml.rfind("</workbook>") {
                xml.insert_str(pos, &format!("<definedNames>{}</definedNames>", insert));
            } else {
                bail!("workbook.xml missing </workbook>");
            }

            writer.write_all(xml.as_bytes())?;
        } else {
            writer.write_all(&data)?;
        }
    }

    writer.finish()?;
    fs::rename(tmp_path, path)?;
    Ok(())
}

#[cfg(feature = "recalc")]
fn normalize_simple_quoted_sheet_refs(input: &str) -> String {
    let simple_quoted_sheet = Regex::new(r"'([A-Za-z_][A-Za-z0-9_.]*)'!").unwrap();
    simple_quoted_sheet.replace_all(input, "$1!").into_owned()
}

#[cfg(feature = "recalc")]
fn read_defined_name_refers_to(path: &Path, target_name: &str) -> Result<Option<String>> {
    let src_bytes = fs::read(path)?;
    let mut archive = ZipArchive::new(Cursor::new(src_bytes))?;
    let mut file = archive.by_name("xl/workbook.xml")?;

    let mut xml = String::new();
    file.read_to_string(&mut xml)?;

    let mut reader = Reader::from_str(&xml);
    reader.trim_text(true);

    let mut buf: Vec<u8> = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name().as_ref() == b"definedName" => {
                let mut name: Option<String> = None;
                for attr in e.attributes().with_checks(false).flatten() {
                    if attr.key.as_ref() == b"name" {
                        name = Some(attr.unescape_value()?.to_string());
                        break;
                    }
                }

                if name.as_deref() == Some(target_name) {
                    let mut text = String::new();
                    loop {
                        buf.clear();
                        match reader.read_event_into(&mut buf) {
                            Ok(Event::Text(t)) => {
                                text.push_str(t.unescape()?.as_ref());
                            }
                            Ok(Event::End(end)) if end.name().as_ref() == b"definedName" => {
                                return Ok(Some(normalize_simple_quoted_sheet_refs(&text)));
                            }
                            Ok(Event::Eof) => {
                                return Ok(Some(normalize_simple_quoted_sheet_refs(&text)));
                            }
                            Ok(_) => {}
                            Err(e) => return Err(anyhow!("failed parsing workbook.xml: {}", e)),
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(anyhow!("failed parsing workbook.xml: {}", e)),
        }
        buf.clear();
    }

    Ok(None)
}

#[tokio::test]
async fn test_structure_batch_insert_rows_updates_cross_sheet_formulas_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("structure_rows.xlsx", |book| {
            let inputs = book.sheet_mut(0).ok().unwrap();
            inputs.set_name("Inputs");
            inputs.cell_mut("A1").set_value_number(1);
            inputs.cell_mut("A2").set_value_number(2);

            book.new_sheet("Calc").unwrap();
            let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
            calc.cell_mut("A1")
                .set_formula("SUM(Inputs!A1:A2)".to_string());
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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "insert_rows",
                "sheet_name": "Inputs",
                "at_row": 2,
                "count": 1
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert!(!cell_is_error(&calc_page, 0, 0));
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(3.0));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_structure_batch_rename_sheet_preserves_formulas_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("structure_rename.xlsx", |book| {
            let inputs = book.sheet_mut(0).ok().unwrap();
            inputs.set_name("Inputs");
            inputs.cell_mut("A1").set_value_number(3);

            book.new_sheet("Calc").unwrap();
            let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
            calc.cell_mut("A1").set_formula("Inputs!A1".to_string());
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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "rename_sheet",
                "old_name": "Inputs",
                "new_name": "Data"
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert!(!cell_is_error(&calc_page, 0, 0));
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(3.0));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_structure_batch_insert_cols_updates_cross_sheet_formulas_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("structure_cols.xlsx", |book| {
            let inputs = book.sheet_mut(0).ok().unwrap();
            inputs.set_name("Inputs");
            inputs.cell_mut("A1").set_value_number(1);
            inputs.cell_mut("B1").set_value_number(2);

            book.new_sheet("Calc").unwrap();
            let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
            calc.cell_mut("A1")
                .set_formula("SUM(Inputs!A1:B1)".to_string());
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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "insert_cols",
                "sheet_name": "Inputs",
                "at_col": "A",
                "count": 1
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert!(!cell_is_error(&calc_page, 0, 0));
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(3.0));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_structure_batch_delete_rows_preserves_formula_result_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("structure_delete_rows.xlsx", |book| {
            let inputs = book.sheet_mut(0).ok().unwrap();
            inputs.set_name("Inputs");
            inputs.cell_mut("A1").set_value_number(1);
            inputs.cell_mut("A2").set_value_number(2);
            inputs.cell_mut("A3").set_value_number(3);

            book.new_sheet("Calc").unwrap();
            let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
            calc.cell_mut("A1")
                .set_formula("SUM(Inputs!A1:A3)".to_string());
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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "delete_rows",
                "sheet_name": "Inputs",
                "start_row": 2,
                "count": 1
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert!(!cell_is_error(&calc_page, 0, 0));
    // Should remain SUM of remaining values 1 and 3.
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(4.0));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_structure_batch_delete_cols_preserves_formula_result_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("structure_delete_cols.xlsx", |book| {
            let inputs = book.sheet_mut(0).ok().unwrap();
            inputs.set_name("Inputs");
            inputs.cell_mut("A1").set_value_number(1);
            inputs.cell_mut("B1").set_value_number(2);
            inputs.cell_mut("C1").set_value_number(3);

            book.new_sheet("Calc").unwrap();
            let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
            calc.cell_mut("A1")
                .set_formula("SUM(Inputs!A1:C1)".to_string());
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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "delete_cols",
                "sheet_name": "Inputs",
                "start_col": "B",
                "count": 1
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert!(!cell_is_error(&calc_page, 0, 0));
    // Should remain SUM of remaining values 1 and 3.
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(4.0));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_structure_batch_rename_quoted_sheet_preserves_formulas_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("structure_rename_quoted.xlsx", |book| {
            let inputs = book.sheet_mut(0).ok().unwrap();
            inputs.set_name("My Sheet");
            inputs.cell_mut("A1").set_value_number(3);

            book.new_sheet("Calc").unwrap();
            let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
            calc.cell_mut("A1")
                .set_formula("'My Sheet'!A1".to_string());
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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "rename_sheet",
                "old_name": "My Sheet",
                "new_name": "Data"
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool("recalculate", json!({ "fork_id": fork_id })))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert!(!cell_is_error(&calc_page, 0, 0));
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(3.0));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_structure_batch_copy_range_across_sheets_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("structure_copy_range_cross_sheet.xlsx", |book| {
            let sheet1 = book.sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet1.cell_mut("A1").set_value_number(1);
            sheet1.cell_mut("B1").set_value_number(10);
            sheet1.cell_mut("C1").set_formula("A1+B1".to_string());

            let sheet2 = book.new_sheet("Sheet2").unwrap();
            sheet2.cell_mut("B1").set_value_number(100);
            sheet2.cell_mut("C1").set_value_number(200);
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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "copy_range",
                "sheet_name": "Sheet1",
                "dest_sheet_name": "Sheet2",
                "src_range": "C1:C1",
                "dest_anchor": "D1",
                "include_styles": false,
                "include_formulas": true
              }]
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
                    "sheet_name": "Sheet2",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["D"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;

    assert!(!cell_is_error(&page, 0, 0));
    assert_eq!(cell_value_f64(&page, 0, 0), Some(300.0));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_structure_batch_copy_range_shifts_formulas_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("structure_copy_range.xlsx", |book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.cell_mut("A1").set_value_number(1);
            sheet.cell_mut("B1").set_value_number(10);
            sheet.cell_mut("A2").set_value_number(2);
            sheet.cell_mut("B2").set_value_number(20);
            sheet.cell_mut("C1").set_formula("A1+B1".to_string());
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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "copy_range",
                "sheet_name": "Sheet1",
                "src_range": "C1:C1",
                "dest_anchor": "D1",
                "include_styles": false,
                "include_formulas": true
              }]
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
                    "page_size": 1,
                    "columns": ["C", "D"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;

    assert!(!cell_is_error(&page, 0, 0));
    assert!(!cell_is_error(&page, 0, 1));
    assert_eq!(cell_value_f64(&page, 0, 0), Some(11.0));
    assert_eq!(cell_value_f64(&page, 0, 1), Some(21.0));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn test_structure_batch_move_range_moves_and_clears_source_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    test.workspace()
        .create_workbook("structure_move_range.xlsx", |book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.cell_mut("A1").set_value("x");
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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "move_range",
                "sheet_name": "Sheet1",
                "src_range": "A1:A1",
                "dest_anchor": "C3",
                "include_styles": false,
                "include_formulas": false
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
                    "page_size": 3,
                    "columns": ["A", "C"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;

    assert_eq!(cell_value(&page, 0, 0), None);
    assert_eq!(cell_value(&page, 2, 1).as_deref(), Some("x"));

    client.cancel().await?;
    Ok(())
}

#[cfg(feature = "recalc")]
#[tokio::test]
async fn test_structure_batch_insert_rows_preserves_named_range_outputs_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    let workbook_path =
        test.workspace()
            .create_workbook("structure_rows_named_ranges.xlsx", |book| {
                let inputs = book.sheet_mut(0).ok().unwrap();
                inputs.set_name("Inputs");
                inputs.cell_mut("A1").set_value_number(1);
                inputs.cell_mut("A2").set_value_number(2);

                book.new_sheet("Calc").unwrap();
                let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
                calc.cell_mut("A1")
                    .set_formula("SUM(InputVals) + SUM(Inputs!A1:A2)".to_string());
            });
    inject_defined_names(&workbook_path, &[("InputVals", "Inputs!$A$1:$A$2")])?;

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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "insert_rows",
                "sheet_name": "Inputs",
                "at_row": 2,
                "count": 1
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool(
            "recalculate",
            json!({ "fork_id": fork_id, "backend": "libreoffice" }),
        ))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(6.0));

    let saved_name = "structure_rows_named_ranges_out.xlsx";
    let _ = client
        .call_tool(call_tool(
            "save_fork",
            json!({
                "fork_id": fork_id,
                "target_path": format!("/data/{saved_name}"),
                "drop_fork": true
            }),
        ))
        .await?;

    let saved_path = test.workspace().path(saved_name);
    let refers_to =
        read_defined_name_refers_to(&saved_path, "InputVals")?.expect("InputVals definedName");
    assert_eq!(refers_to, "Inputs!$A$1:$A$3");

    client.cancel().await?;
    Ok(())
}

#[cfg(feature = "recalc")]
#[tokio::test]
async fn test_structure_batch_insert_rows_above_named_range_shifts_ref_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    let workbook_path =
        test.workspace()
            .create_workbook("structure_rows_named_ranges_above.xlsx", |book| {
                let inputs = book.sheet_mut(0).ok().unwrap();
                inputs.set_name("Inputs");
                inputs.cell_mut("A1").set_value_number(1);
                inputs.cell_mut("A2").set_value_number(2);

                book.new_sheet("Calc").unwrap();
                let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
                calc.cell_mut("A1")
                    .set_formula("SUM(InputVals) + SUM(Inputs!A1:A2)".to_string());
            });
    inject_defined_names(&workbook_path, &[("InputVals", "Inputs!$A$1:$A$2")])?;

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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "insert_rows",
                "sheet_name": "Inputs",
                "at_row": 1,
                "count": 1
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool(
            "recalculate",
            json!({ "fork_id": fork_id, "backend": "libreoffice" }),
        ))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(6.0));

    let saved_name = "structure_rows_named_ranges_above_out.xlsx";
    let _ = client
        .call_tool(call_tool(
            "save_fork",
            json!({
                "fork_id": fork_id,
                "target_path": format!("/data/{saved_name}"),
                "drop_fork": true
            }),
        ))
        .await?;

    let saved_path = test.workspace().path(saved_name);
    let refers_to =
        read_defined_name_refers_to(&saved_path, "InputVals")?.expect("InputVals definedName");
    assert_eq!(refers_to, "Inputs!$A$2:$A$3");

    client.cancel().await?;
    Ok(())
}

#[cfg(feature = "recalc")]
#[tokio::test]
async fn test_structure_batch_insert_rows_multirow_expands_named_range_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    let workbook_path =
        test.workspace()
            .create_workbook("structure_rows_named_ranges_multirow.xlsx", |book| {
                let inputs = book.sheet_mut(0).ok().unwrap();
                inputs.set_name("Inputs");
                inputs.cell_mut("A1").set_value_number(1);
                inputs.cell_mut("A2").set_value_number(2);

                book.new_sheet("Calc").unwrap();
                let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
                calc.cell_mut("A1")
                    .set_formula("SUM(InputVals) + SUM(Inputs!A1:A2)".to_string());
            });
    inject_defined_names(&workbook_path, &[("InputVals", "Inputs!$A$1:$A$2")])?;

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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "insert_rows",
                "sheet_name": "Inputs",
                "at_row": 2,
                "count": 2
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool(
            "recalculate",
            json!({ "fork_id": fork_id, "backend": "libreoffice" }),
        ))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(6.0));

    let saved_name = "structure_rows_named_ranges_multirow_out.xlsx";
    let _ = client
        .call_tool(call_tool(
            "save_fork",
            json!({
                "fork_id": fork_id,
                "target_path": format!("/data/{saved_name}"),
                "drop_fork": true
            }),
        ))
        .await?;

    let saved_path = test.workspace().path(saved_name);
    let refers_to =
        read_defined_name_refers_to(&saved_path, "InputVals")?.expect("InputVals definedName");
    assert_eq!(refers_to, "Inputs!$A$1:$A$4");

    client.cancel().await?;
    Ok(())
}

#[cfg(feature = "recalc")]
#[tokio::test]
async fn test_structure_batch_insert_rows_adjusts_union_named_ranges_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    let workbook_path =
        test.workspace()
            .create_workbook("structure_rows_named_union.xlsx", |book| {
                let inputs = book.sheet_mut(0).ok().unwrap();
                inputs.set_name("Inputs");
                inputs.cell_mut("A1").set_value_number(1);
                inputs.cell_mut("A3").set_value_number(3);

                book.new_sheet("Calc").unwrap();
                let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
                calc.cell_mut("A1")
                    .set_formula("SUM(UnionVals)".to_string());
            });
    inject_defined_names(&workbook_path, &[("UnionVals", "Inputs!$A$1,Inputs!$A$3")])?;

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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "insert_rows",
                "sheet_name": "Inputs",
                "at_row": 2,
                "count": 1
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool(
            "recalculate",
            json!({ "fork_id": fork_id, "backend": "libreoffice" }),
        ))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(4.0));

    let saved_name = "structure_rows_named_union_out.xlsx";
    let _ = client
        .call_tool(call_tool(
            "save_fork",
            json!({
                "fork_id": fork_id,
                "target_path": format!("/data/{saved_name}"),
                "drop_fork": true
            }),
        ))
        .await?;

    let saved_path = test.workspace().path(saved_name);
    let refers_to =
        read_defined_name_refers_to(&saved_path, "UnionVals")?.expect("UnionVals definedName");
    assert_eq!(refers_to.replace(' ', ""), "Inputs!$A$1,Inputs!$A$4");

    client.cancel().await?;
    Ok(())
}

#[cfg(feature = "recalc")]
#[tokio::test]
async fn test_structure_batch_insert_rows_rewrites_formula_defined_names_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    let workbook_path =
        test.workspace()
            .create_workbook("structure_rows_named_formula.xlsx", |book| {
                let inputs = book.sheet_mut(0).ok().unwrap();
                inputs.set_name("Inputs");
                inputs.cell_mut("A1").set_value_number(1);
                inputs.cell_mut("A2").set_value_number(2);

                book.new_sheet("Calc").unwrap();
                let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
                calc.cell_mut("A1").set_formula("CalcTotal".to_string());
            });
    inject_defined_names(&workbook_path, &[("CalcTotal", "=SUM(Inputs!A1:A2)")])?;

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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "insert_rows",
                "sheet_name": "Inputs",
                "at_row": 2,
                "count": 1
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool(
            "recalculate",
            json!({ "fork_id": fork_id, "backend": "libreoffice" }),
        ))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(3.0));

    let saved_name = "structure_rows_named_formula_out.xlsx";
    let _ = client
        .call_tool(call_tool(
            "save_fork",
            json!({
                "fork_id": fork_id,
                "target_path": format!("/data/{saved_name}"),
                "drop_fork": true
            }),
        ))
        .await?;

    let saved_path = test.workspace().path(saved_name);
    let refers_to =
        read_defined_name_refers_to(&saved_path, "CalcTotal")?.expect("CalcTotal definedName");
    let normalized = refers_to.replace(' ', "");
    let normalized = normalized.strip_prefix('=').unwrap_or(normalized.as_str());
    assert_eq!(normalized, "SUM(Inputs!A1:A3)");

    client.cancel().await?;
    Ok(())
}

#[cfg(feature = "recalc")]
#[tokio::test]
async fn test_structure_batch_rename_sheet_updates_named_ranges_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    let workbook_path =
        test.workspace()
            .create_workbook("structure_rename_named_ranges.xlsx", |book| {
                let inputs = book.sheet_mut(0).ok().unwrap();
                inputs.set_name("Inputs");
                inputs.cell_mut("A1").set_value_number(1);
                inputs.cell_mut("A2").set_value_number(2);

                book.new_sheet("Calc").unwrap();
                let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
                calc.cell_mut("A1")
                    .set_formula("SUM(InputVals)".to_string());
            });
    inject_defined_names(&workbook_path, &[("InputVals", "Inputs!$A$1:$A$2")])?;

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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "rename_sheet",
                "old_name": "Inputs",
                "new_name": "Data"
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool(
            "recalculate",
            json!({ "fork_id": fork_id, "backend": "libreoffice" }),
        ))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(3.0));

    let saved_name = "structure_rename_named_inputvals_out.xlsx";
    let _ = client
        .call_tool(call_tool(
            "save_fork",
            json!({
                "fork_id": fork_id,
                "target_path": format!("/data/{saved_name}"),
                "drop_fork": true
            }),
        ))
        .await?;

    let saved_path = test.workspace().path(saved_name);
    let refers_to =
        read_defined_name_refers_to(&saved_path, "InputVals")?.expect("InputVals definedName");
    assert_eq!(refers_to, "Data!$A$1:$A$2");

    client.cancel().await?;
    Ok(())
}

#[cfg(feature = "recalc")]
#[tokio::test]
async fn test_structure_batch_rename_sheet_updates_quoted_named_ranges_in_docker() -> Result<()> {
    let test = McpTestClient::new();
    let workbook_path =
        test.workspace()
            .create_workbook("structure_rename_named_quoted.xlsx", |book| {
                let inputs = book.sheet_mut(0).ok().unwrap();
                inputs.set_name("My Sheet");
                inputs.cell_mut("A1").set_value_number(1);
                inputs.cell_mut("A2").set_value_number(2);

                book.new_sheet("Calc").unwrap();
                let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
                calc.cell_mut("A1")
                    .set_formula("SUM(InputVals)".to_string());
            });
    inject_defined_names(&workbook_path, &[("InputVals", "'My Sheet'!$A$1:$A$2")])?;

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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "rename_sheet",
                "old_name": "My Sheet",
                "new_name": "Data"
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool(
            "recalculate",
            json!({ "fork_id": fork_id, "backend": "libreoffice" }),
        ))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(3.0));

    let saved_name = "structure_rename_named_inputvals_out.xlsx";
    let _ = client
        .call_tool(call_tool(
            "save_fork",
            json!({
                "fork_id": fork_id,
                "target_path": format!("/data/{saved_name}"),
                "drop_fork": true
            }),
        ))
        .await?;

    let saved_path = test.workspace().path(saved_name);
    let refers_to =
        read_defined_name_refers_to(&saved_path, "InputVals")?.expect("InputVals definedName");
    assert_eq!(refers_to, "Data!$A$1:$A$2");

    client.cancel().await?;
    Ok(())
}

#[cfg(feature = "recalc")]
#[tokio::test]
async fn test_structure_batch_rename_sheet_rewrites_formula_defined_names_in_docker() -> Result<()>
{
    let test = McpTestClient::new();
    let workbook_path =
        test.workspace()
            .create_workbook("structure_rename_named_formula.xlsx", |book| {
                let inputs = book.sheet_mut(0).ok().unwrap();
                inputs.set_name("Inputs");
                inputs.cell_mut("A1").set_value_number(1);
                inputs.cell_mut("A2").set_value_number(2);

                book.new_sheet("Calc").unwrap();
                let calc = book.sheet_by_name_mut("Calc").ok().unwrap();
                calc.cell_mut("A1").set_formula("CalcTotal".to_string());
            });
    inject_defined_names(&workbook_path, &[("CalcTotal", "=SUM(Inputs!A1:A2)")])?;

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
            "structure_batch",
            json!({
              "fork_id": fork_id,
              "mode": "apply",
              "ops": [{
                "kind": "rename_sheet",
                "old_name": "Inputs",
                "new_name": "Data"
              }]
            }),
        ))
        .await?;

    let _ = client
        .call_tool(call_tool(
            "recalculate",
            json!({ "fork_id": fork_id, "backend": "libreoffice" }),
        ))
        .await?;

    let calc_page = extract_json(
        &client
            .call_tool(call_tool(
                "sheet_page",
                json!({
                    "workbook_or_fork_id": fork_id,
                    "sheet_name": "Calc",
                    "start_row": 1,
                    "page_size": 1,
                    "columns": ["A"],
                    "include_formulas": false
                }),
            ))
            .await?,
    )?;
    assert_eq!(cell_value_f64(&calc_page, 0, 0), Some(3.0));

    let saved_name = "structure_rename_named_formula_out.xlsx";
    let _ = client
        .call_tool(call_tool(
            "save_fork",
            json!({
                "fork_id": fork_id,
                "target_path": format!("/data/{saved_name}"),
                "drop_fork": true
            }),
        ))
        .await?;

    let saved_path = test.workspace().path(saved_name);
    let refers_to =
        read_defined_name_refers_to(&saved_path, "CalcTotal")?.expect("CalcTotal definedName");
    let normalized = refers_to.replace(' ', "");
    let normalized = normalized.strip_prefix('=').unwrap_or(normalized.as_str());
    assert_eq!(normalized, "SUM(Data!A1:A2)");

    client.cancel().await?;
    Ok(())
}
