use serde_json::Value;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn write_fixture(path: &Path) {
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook
            .get_sheet_by_name_mut("Sheet1").ok()
            .expect("default sheet exists");
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Amount");
        sheet.get_cell_mut("C1").set_value("Total");
        sheet.get_cell_mut("A2").set_value("Alice");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("C2").set_formula("B2*2");
        sheet.get_cell_mut("A3").set_value("Bob");
        sheet.get_cell_mut("B3").set_value_number(20.0);
        sheet.get_cell_mut("C3").set_formula("B3*2");
    }
    workbook.new_sheet("Summary").expect("add summary sheet");
    umya_spreadsheet::writer::xlsx::write(&workbook, path).expect("write workbook");
}

fn run_cli(args: &[&str]) -> std::process::Output {
    Command::new(assert_cmd::cargo::cargo_bin!("agent-spreadsheet"))
        .args(args)
        .output()
        .expect("run agent-spreadsheet")
}

fn parse_stdout_json(output: &std::process::Output) -> Value {
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout utf8");
    serde_json::from_str(&stdout).expect("valid json")
}

#[test]
fn core_write_and_cli_write_produce_parity_diff_counts() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("original.xlsx");
    let cli_modified = tmp.path().join("cli.xlsx");
    let core_modified = tmp.path().join("core.xlsx");
    write_fixture(&original);
    std::fs::copy(&original, &cli_modified).expect("copy cli target");
    std::fs::copy(&original, &core_modified).expect("copy core target");

    let edit = run_cli(&[
        "edit",
        cli_modified.to_str().expect("path utf8"),
        "Sheet1",
        "A2=Eve",
        "C2==B2*3",
    ]);
    assert!(edit.status.success(), "stderr: {:?}", edit.stderr);

    let shorthand = vec!["A2=Eve", "C2==B2*3"];
    let mut core_edits = Vec::new();
    for entry in shorthand {
        let (edit, _warnings) = agent_spreadsheet::core::write::normalize_shorthand_edit(entry)
            .expect("normalize shorthand");
        core_edits.push(edit);
    }
    agent_spreadsheet::core::write::apply_edits_to_file(&core_modified, "Sheet1", &core_edits)
        .expect("apply core edits");

    let cli_diff = agent_spreadsheet::core::diff::diff_workbooks_json(&original, &cli_modified)
        .expect("cli diff");
    let core_diff = agent_spreadsheet::core::diff::diff_workbooks_json(&original, &core_modified)
        .expect("core diff");

    assert_eq!(cli_diff["change_count"], core_diff["change_count"]);
    assert!(cli_diff["change_count"].as_u64().unwrap_or(0) >= 1);
}

#[test]
fn cli_read_matches_mcp_tools_for_representative_commands() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("parity.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let cli_list = run_cli(&["list-sheets", file]);
    assert!(cli_list.status.success(), "stderr: {:?}", cli_list.stderr);
    let cli_list_payload = parse_stdout_json(&cli_list);

    let cli_ranges = run_cli(&["range-values", file, "Sheet1", "A1:B3"]);
    assert!(
        cli_ranges.status.success(),
        "stderr: {:?}",
        cli_ranges.stderr
    );
    let cli_ranges_payload = parse_stdout_json(&cli_ranges);

    let cli_describe = run_cli(&["describe", file]);
    assert!(
        cli_describe.status.success(),
        "stderr: {:?}",
        cli_describe.stderr
    );
    let cli_describe_payload = parse_stdout_json(&cli_describe);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (mcp_list_payload, mcp_ranges_payload, mcp_describe_payload) = runtime.block_on(async {
        let runtime = agent_spreadsheet::runtime::stateless::StatelessRuntime;
        let (state, workbook_id) = runtime
            .open_state_for_file(&workbook_path)
            .await
            .expect("open workbook in state");

        let list = agent_spreadsheet::tools::list_sheets(
            state.clone(),
            agent_spreadsheet::tools::ListSheetsParams {
                workbook_or_fork_id: workbook_id.clone(),
                limit: None,
                offset: None,
                include_bounds: None,
            },
        )
        .await
        .expect("mcp list");

        let ranges = agent_spreadsheet::tools::range_values(
            state.clone(),
            agent_spreadsheet::tools::RangeValuesParams {
                workbook_or_fork_id: workbook_id.clone(),
                sheet_name: "Sheet1".to_string(),
                ranges: vec!["A1:B3".to_string()],
                include_headers: None,
                include_formulas: None,
                format: Some(agent_spreadsheet::model::TableOutputFormat::Dense),
                page_size: None,
            },
        )
        .await
        .expect("mcp ranges");

        let describe = agent_spreadsheet::tools::describe_workbook(
            state,
            agent_spreadsheet::tools::DescribeWorkbookParams {
                workbook_or_fork_id: workbook_id,
            },
        )
        .await
        .expect("mcp describe");

        (
            serde_json::to_value(list).expect("list value"),
            serde_json::to_value(ranges).expect("ranges value"),
            serde_json::to_value(describe).expect("describe value"),
        )
    });

    assert_eq!(cli_list_payload["sheets"], mcp_list_payload["sheets"]);
    assert_eq!(
        cli_ranges_payload["values"][0]["dense"],
        mcp_ranges_payload["values"][0]["dense"]
    );
    assert_eq!(
        cli_describe_payload["sheet_count"],
        mcp_describe_payload["sheet_count"]
    );
    assert_eq!(
        cli_describe_payload["sheets"],
        mcp_describe_payload["sheets"]
    );
}

#[cfg(feature = "recalc")]
#[test]
fn mcp_normalize_wrapper_matches_core_write_normalization() {
    use agent_spreadsheet::tools::write_normalize::{EditBatchParamsInput, normalize_edit_batch};

    let input = serde_json::json!({
        "fork_id": "fork-1",
        "sheet_name": "Sheet1",
        "edits": [
            "A1=Hello",
            { "address": "B2", "formula": "=SUM(A1:A1)" }
        ]
    });
    let params: EditBatchParamsInput = serde_json::from_value(input).expect("valid params");
    let (wrapped, wrapped_warnings) = normalize_edit_batch(params).expect("normalize wrapper");

    let (s1, mut expected_warnings) =
        agent_spreadsheet::core::write::normalize_shorthand_edit("A1=Hello")
            .expect("shorthand normalize");
    let (s2, more_warnings) = agent_spreadsheet::core::write::normalize_object_edit(
        "B2",
        None,
        Some("=SUM(A1:A1)".to_string()),
        None,
    )
    .expect("object normalize");
    expected_warnings.extend(more_warnings);

    assert_eq!(wrapped.edits.len(), 2);
    assert_eq!(wrapped.edits[0].address, s1.address);
    assert_eq!(wrapped.edits[0].value, s1.value);
    assert_eq!(wrapped.edits[0].is_formula, s1.is_formula);
    assert_eq!(wrapped.edits[1].address, s2.address);
    assert_eq!(wrapped.edits[1].value, s2.value);
    assert_eq!(wrapped.edits[1].is_formula, s2.is_formula);

    let wrapped_codes: Vec<_> = wrapped_warnings.iter().map(|w| w.code.as_str()).collect();
    let expected_codes: Vec<_> = expected_warnings.iter().map(|w| w.code.as_str()).collect();
    assert_eq!(wrapped_codes, expected_codes);
}
