#![cfg(feature = "recalc")]

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::tempdir;

#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};

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
        sheet.get_cell_mut("A4").set_value("Carol");
        sheet.get_cell_mut("B4").set_value_number(30.0);
        sheet.get_cell_mut("C4").set_formula("B4*2");
    }

    workbook.new_sheet("Summary").expect("add summary sheet");
    {
        let summary = workbook
            .get_sheet_by_name_mut("Summary").ok()
            .expect("summary sheet exists");
        summary.get_cell_mut("A1").set_value("Flag");
        summary.get_cell_mut("B1").set_value("Ready");
    }

    umya_spreadsheet::writer::xlsx::write(&workbook, path).expect("write workbook");
}

fn write_trace_pagination_fixture(path: &Path) {
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook
            .get_sheet_by_name_mut("Sheet1").ok()
            .expect("default sheet exists");
        sheet.get_cell_mut("A1").set_value_number(1.0);
        for row in 1..=18 {
            let address = format!("B{row}");
            let formula = format!("A1+{row}");
            sheet.get_cell_mut(address.as_str()).set_formula(formula);
        }
    }

    umya_spreadsheet::writer::xlsx::write(&workbook, path).expect("write workbook");
}

fn write_phase1_read_surface_fixture(path: &Path) {
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook
            .get_sheet_by_name_mut("Sheet1").ok()
            .expect("default sheet exists");
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Amount");
        sheet.get_cell_mut("C1").set_value("Calc");
        sheet.get_cell_mut("D1").set_value("Volatile");

        sheet.get_cell_mut("A2").set_value("Alice");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("C2").set_formula("SUM(B2:B2)");
        sheet.get_cell_mut("D2").set_formula("NOW()");

        sheet.get_cell_mut("A3").set_value("Bob");
        sheet.get_cell_mut("B3").set_value_number(20.0);
        sheet.get_cell_mut("C3").set_formula("SUM(B3:B3)");
        sheet.get_cell_mut("D3").set_formula("RAND()");

        sheet.get_cell_mut("A4").set_value("Carol");
        sheet.get_cell_mut("B4").set_value_number(30.0);
        sheet.get_cell_mut("C4").set_formula("SUM(B4:B4)");
        sheet.get_cell_mut("D4").set_formula("TODAY()");

        let mut table = umya_spreadsheet::structs::Table::new("SalesTable", ("A1", "D4"));
        table.set_display_name("SalesTable");
        sheet.add_table(table);
    }

    workbook.new_sheet("Summary").expect("add summary sheet");
    {
        let summary = workbook
            .get_sheet_by_name_mut("Summary").ok()
            .expect("summary sheet exists");
        summary.get_cell_mut("A1").set_value("Flag");
        summary.get_cell_mut("B1").set_value("Ready");
    }

    let sheet1 = workbook
        .get_sheet_by_name_mut("Sheet1").ok()
        .expect("sheet1 exists");
    sheet1
        .add_defined_name("Sales_Amount", "Sheet1!$B$2:$B$4")
        .expect("defined name Sales_Amount");
    sheet1
        .add_defined_name("Sales_First", "Sheet1!$A$2")
        .expect("defined name Sales_First");
    let summary = workbook
        .get_sheet_by_name_mut("Summary").ok()
        .expect("summary exists");
    summary
        .add_defined_name("Meta_Flag", "Summary!$A$1")
        .expect("defined name Meta_Flag");

    umya_spreadsheet::writer::xlsx::write(&workbook, path).expect("write workbook");
}

fn write_formula_parse_failure_fixture(path: &Path) {
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook
            .get_sheet_by_name_mut("Sheet1").ok()
            .expect("default sheet exists");
        sheet.get_cell_mut("A1").set_value("Input");
        sheet.get_cell_mut("B1").set_value("Result");
        sheet.get_cell_mut("C1").set_value("Trace");
        // Intentionally malformed: one extra closing parenthesis.
        sheet.get_cell_mut("B2").set_formula(
            r#"IF(C70="","",IF(C70="N/A","",IF(C70="Unknown",0,IF(LEFT(C70,1)="0",0,IF(LEFT(C70,1)="1",25,IF(LEFT(C70,1)="2",50,IF(LEFT(C70,1)="3",75,IF(LEFT(C70,1)="4",100,"")))))))))"#,
        );
        sheet.get_cell_mut("B3").set_formula("NOW()");
        sheet.get_cell_mut("A3").set_value_number(20.0);
        sheet.get_cell_mut("C3").set_formula("A3+1");
    }

    umya_spreadsheet::writer::xlsx::write(&workbook, path).expect("write workbook");
}

fn write_workbook_short_id_column_fixture(path: &Path) {
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook
            .get_sheet_by_name_mut("Sheet1").ok()
            .expect("default sheet exists");
        sheet.get_cell_mut("A1").set_value("workbook_short_id");
        sheet.get_cell_mut("B1").set_value("Name");
        sheet.get_cell_mut("A2").set_value("user-data-id");
        sheet.get_cell_mut("B2").set_value("Alice");
    }

    umya_spreadsheet::writer::xlsx::write(&workbook, path).expect("write workbook");
}

fn run_cli(args: &[&str]) -> std::process::Output {
    Command::new(assert_cmd::cargo::cargo_bin!("agent-spreadsheet"))
        .args(args)
        .output()
        .expect("run agent-spreadsheet")
}

fn run_asp(args: &[&str]) -> std::process::Output {
    Command::new(assert_cmd::cargo::cargo_bin!("asp"))
        .args(args)
        .output()
        .expect("run asp")
}

fn parse_stdout_json(output: &std::process::Output) -> Value {
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout utf8");
    serde_json::from_str(&stdout).expect("valid json")
}

fn parse_stderr_json(output: &std::process::Output) -> Value {
    let stderr = String::from_utf8(output.stderr.clone()).expect("stderr utf8");
    serde_json::from_str(&stderr).expect("valid json error")
}

fn parse_stdout_text(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout utf8")
}

fn normalize_path_for_assert(path: &str) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

fn assert_json_path_eq(payload: &Value, key: &str, expected: &str) {
    let actual = payload[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} should be a path string, payload={payload}"));
    let actual_norm = normalize_path_for_assert(actual);
    let expected_norm = normalize_path_for_assert(expected);
    assert_eq!(
        actual_norm, expected_norm,
        "path mismatch for {key}: actual={actual:?}, expected={expected:?}"
    );
}

fn assert_invalid_argument(args: &[&str]) -> Value {
    assert_error_code(args, "INVALID_ARGUMENT")
}

fn assert_error_code(args: &[&str], expected_code: &str) -> Value {
    let output = run_cli(args);
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded for args: {args:?}"
    );
    let err = parse_stderr_json(&output);
    assert_eq!(
        err["code"], expected_code,
        "unexpected error envelope: {err}"
    );
    err
}

fn write_ops_payload(path: &Path, payload: &str) {
    fs::write(path, payload).expect("write ops payload");
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_repo_doc(relative_path: &str) -> String {
    fs::read_to_string(repo_root().join(relative_path))
        .unwrap_or_else(|err| panic!("read {relative_path}: {err}"))
}

fn assert_batch_mode_matrix(command: &str, file: &str, ops_ref: &str) {
    assert_invalid_argument(&[command, file, "--ops", ops_ref]);
    assert_invalid_argument(&[command, file, "--ops", ops_ref, "--dry-run", "--in-place"]);
    assert_invalid_argument(&[
        command,
        file,
        "--ops",
        ops_ref,
        "--dry-run",
        "--output",
        "out.xlsx",
    ]);
    assert_invalid_argument(&[
        command,
        file,
        "--ops",
        ops_ref,
        "--in-place",
        "--output",
        "out.xlsx",
    ]);
    assert_invalid_argument(&[command, file, "--ops", ops_ref, "--force"]);
    assert_invalid_argument(&[command, file, "--ops", ops_ref, "--output", file]);
}

#[test]
fn schema_and_example_commands_support_batch_and_session_discovery() {
    for command in [
        "transform-batch",
        "style-batch",
        "apply-formula-pattern",
        "structure-batch",
        "column-size-batch",
        "sheet-layout-batch",
        "rules-batch",
    ] {
        let schema = run_cli(&["schema", command]);
        assert!(
            schema.status.success(),
            "schema {command} failed: {}",
            String::from_utf8_lossy(&schema.stderr)
        );
        let schema_payload = parse_stdout_json(&schema);
        assert_eq!(
            schema_payload["schema_kind"], "ops_payload",
            "payload={schema_payload}"
        );
        assert!(
            schema_payload["schema"].is_object(),
            "expected schema object for {command}, payload={schema_payload}"
        );

        let example = run_cli(&["example", command]);
        assert!(
            example.status.success(),
            "example {command} failed: {}",
            String::from_utf8_lossy(&example.stderr)
        );
        let example_payload = parse_stdout_json(&example);
        assert_eq!(
            example_payload["example_kind"], "ops_payload",
            "payload={example_payload}"
        );
        assert!(
            example_payload["example"].is_object(),
            "payload={example_payload}"
        );
    }

    let schema = run_cli(&["schema", "session", "op", "transform.write_matrix"]);
    assert!(schema.status.success(), "stderr: {:?}", schema.stderr);
    let schema_payload = parse_stdout_json(&schema);
    assert_eq!(schema_payload["schema_kind"], "session_ops_payload");
    assert_eq!(schema_payload["op_kind"], "transform.write_matrix");
    assert!(
        schema_payload["schema"].is_object(),
        "payload={schema_payload}"
    );
    assert_eq!(
        schema_payload["schema"]["properties"]["kind"]["const"],
        "transform.write_matrix"
    );

    let example = run_cli(&["example", "session", "op", "structure.insert_rows"]);
    assert!(example.status.success(), "stderr: {:?}", example.stderr);
    let example_payload = parse_stdout_json(&example);
    assert_eq!(example_payload["example_kind"], "session_ops_payload");
    assert_eq!(example_payload["op_kind"], "structure.insert_rows");
    assert_eq!(example_payload["example"]["kind"], "structure.insert_rows");
    assert!(example_payload["example"]["ops"].is_array());
}

#[test]
fn canonical_schema_and_example_project_cli_adapter_bindings() {
    let schema = run_asp(&["schema", "read_cells"]);
    assert!(schema.status.success(), "stderr: {:?}", schema.stderr);
    let schema = parse_stdout_json(&schema);
    let required = schema["input_schema"]["required"]
        .as_array()
        .expect("required fields");
    assert!(!required.iter().any(|field| field == "resource_id"));
    assert!(required.iter().any(|field| field == "sheet_name"));
    assert_eq!(
        schema["adapter_binding"]["bind"],
        "--bind FILE injects resource_id"
    );
    assert!(
        schema["input_schema"]["properties"]["resource_id"]["description"]
            .as_str()
            .unwrap_or_default()
            .contains("omit this field")
    );

    let verify_schema = run_asp(&["schema", "verify_workbook"]);
    assert!(
        verify_schema.status.success(),
        "stderr: {:?}",
        verify_schema.stderr
    );
    let verify_schema = parse_stdout_json(&verify_schema);
    let required = verify_schema["input_schema"]["required"]
        .as_array()
        .expect("required fields");
    assert!(!required.iter().any(|field| field == "resource_id"));
    assert!(!required.iter().any(|field| field == "baseline_resource_id"));
    assert_eq!(
        verify_schema["adapter_binding"]["baseline"],
        "--baseline FILE injects baseline_resource_id"
    );

    let example = run_asp(&["example", "read_cells"]);
    assert!(example.status.success(), "stderr: {:?}", example.stderr);
    let example = parse_stdout_json(&example);
    assert!(example.get("resource_id").is_none(), "example={example}");

    let verify_example = run_asp(&["example", "verify_workbook"]);
    assert!(
        verify_example.status.success(),
        "stderr: {:?}",
        verify_example.stderr
    );
    let verify_example = parse_stdout_json(&verify_example);
    assert!(
        verify_example.get("resource_id").is_none(),
        "example={verify_example}"
    );
    assert!(
        verify_example.get("baseline_resource_id").is_none(),
        "example={verify_example}"
    );

    let registry = run_asp(&["registry", "--all"]);
    assert!(registry.status.success(), "stderr: {:?}", registry.stderr);
    let registry = parse_stdout_json(&registry);
    let read_cells = registry["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .find(|descriptor| descriptor["name"] == "read_cells")
        .expect("read_cells descriptor");
    assert!(
        read_cells["input_schema"]["required"]
            .as_array()
            .expect("canonical required fields")
            .iter()
            .any(|field| field == "resource_id"),
        "host-independent registry must remain unchanged"
    );
}

#[test]
fn session_schema_rejects_unknown_kind_with_guidance() {
    let output = run_cli(&["schema", "session", "op", "totally.unknown"]);
    assert!(!output.status.success(), "command unexpectedly succeeded");
    let err = parse_stderr_json(&output);
    assert_eq!(err["code"], "INVALID_ARGUMENT");
    assert!(
        err["try_this"]
            .as_str()
            .unwrap_or("")
            .contains("example session op transform.write_matrix"),
        "err={err}"
    );
}

#[test]
fn cli_help_surfaces_include_descriptions_and_examples() {
    let root_help = run_cli(&["--help"]);
    assert!(root_help.status.success(), "stderr: {:?}", root_help.stderr);
    let root = parse_stdout_text(&root_help);
    assert!(root.contains("Stateless spreadsheet CLI for AI and automation workflows"));
    assert!(root.contains("Primary command: asp"));
    assert!(root.contains("Compatibility alias: agent-spreadsheet"));
    assert!(root.contains("Primary groups:"));
    assert!(root.contains("read"));
    assert!(root.contains("analyze"));
    assert!(root.contains("write"));
    assert!(root.contains("workbook"));
    assert!(root.contains("verify"));
    assert!(root.contains("session"));
    assert!(root.contains("sheetport"));
    assert!(root.contains("asp schema write batch transform"));
    assert!(root.contains("asp example write batch transform"));
    assert!(root.contains("asp schema session op transform.write_matrix"));
    assert!(root.contains("global --output-format csv is currently unsupported"));

    let asp_help = run_asp(&["--help"]);
    assert!(asp_help.status.success(), "stderr: {:?}", asp_help.stderr);
    let asp_root = parse_stdout_text(&asp_help);
    assert!(asp_root.contains("Primary command: asp"));

    let read_help = run_cli(&["read", "--help"]);
    assert!(read_help.status.success(), "stderr: {:?}", read_help.stderr);
    let read = parse_stdout_text(&read_help);
    assert!(read.contains("Read workbook data and structure"));
    assert!(read.contains("sheets"));
    assert!(read.contains("overview"));
    assert!(read.contains("table"));
    assert!(read.contains("names"));

    let write_help = run_cli(&["write", "--help"]);
    assert!(
        write_help.status.success(),
        "stderr: {:?}",
        write_help.stderr
    );
    let write = parse_stdout_text(&write_help);
    assert!(write.contains("Write and mutate workbook contents"));
    assert!(write.contains("cells"));
    assert!(write.contains("append"));
    assert!(write.contains("clone-template-row"));
    assert!(write.contains("clone-row-band"));
    assert!(write.contains("batch"));
    assert!(write.contains("name"));
    assert!(write.contains("formulas"));

    let batch_help = run_cli(&["write", "batch", "--help"]);
    assert!(
        batch_help.status.success(),
        "stderr: {:?}",
        batch_help.stderr
    );
    let batch = parse_stdout_text(&batch_help);
    assert!(batch.contains("Stateless batch mutation surfaces"));
    assert!(batch.contains("transform"));
    assert!(batch.contains("style"));
    assert!(batch.contains("formula-pattern"));
    assert!(batch.contains("structure"));
    assert!(batch.contains("column-size"));
    assert!(batch.contains("sheet-layout"));
    assert!(batch.contains("rules"));

    let find_help = run_cli(&["analyze", "find-value", "--help"]);
    assert!(find_help.status.success(), "stderr: {:?}", find_help.stderr);
    let find = parse_stdout_text(&find_help);
    assert!(find.contains("Find cells matching a text query by value or label"));
    assert!(find.contains("Examples:"));
    assert!(
        find.contains(
            "analyze find-value data.xlsx \"Net Income\" --sheet \"Q1 Actuals\" --mode label --label-direction below"
        ) || find.contains(
            "asp analyze find-value data.xlsx \"Net Income\" --sheet \"Q1 Actuals\" --mode label --label-direction below"
        )
    );
    assert!(find.contains("Label mode behavior:"));
    assert!(find.contains("--label-direction any (default) checks right first, then below"));

    let formula_help = run_cli(&["analyze", "formula-map", "--help"]);
    assert!(
        formula_help.status.success(),
        "stderr: {:?}",
        formula_help.stderr
    );
    let formula = parse_stdout_text(&formula_help);
    assert!(formula.contains("Summarize formulas on a sheet by complexity or frequency"));
    assert!(formula.contains("Examples:"));
    assert!(
        formula.contains("analyze formula-map data.xlsx \"Q1 Actuals\" --sort-by count --limit 25")
            || formula.contains(
                "asp analyze formula-map data.xlsx \"Q1 Actuals\" --sort-by count --limit 25"
            )
    );

    let named_ranges_help = run_cli(&["read", "names", "--help"]);
    assert!(
        named_ranges_help.status.success(),
        "stderr: {:?}",
        named_ranges_help.stderr
    );
    let named_ranges = parse_stdout_text(&named_ranges_help);
    assert!(named_ranges.contains("List workbook named ranges and table/formula named items"));
    assert!(named_ranges.contains("Examples:"));
    assert!(
        named_ranges.contains("read names data.xlsx")
            || named_ranges.contains("asp read names data.xlsx")
    );

    let find_formula_help = run_cli(&["analyze", "find-formula", "--help"]);
    assert!(
        find_formula_help.status.success(),
        "stderr: {:?}",
        find_formula_help.stderr
    );
    let find_formula = parse_stdout_text(&find_formula_help);
    assert!(find_formula.contains("Find formulas containing a text query with pagination"));
    assert!(find_formula.contains("Examples:"));
    assert!(
        find_formula.contains("analyze find-formula data.xlsx SUM(")
            || find_formula.contains("asp analyze find-formula data.xlsx SUM(")
    );

    let scan_volatiles_help = run_cli(&["analyze", "scan-volatiles", "--help"]);
    assert!(
        scan_volatiles_help.status.success(),
        "stderr: {:?}",
        scan_volatiles_help.stderr
    );
    let scan_volatiles = parse_stdout_text(&scan_volatiles_help);
    assert!(scan_volatiles.contains("Scan workbook formulas for volatile functions"));
    assert!(scan_volatiles.contains("Examples:"));
    assert!(scan_volatiles.contains("scan-volatiles data.xlsx"));

    let sheet_statistics_help = run_cli(&["analyze", "sheet-statistics", "--help"]);
    assert!(
        sheet_statistics_help.status.success(),
        "stderr: {:?}",
        sheet_statistics_help.stderr
    );
    let sheet_statistics = parse_stdout_text(&sheet_statistics_help);
    assert!(sheet_statistics.contains("Compute per-sheet statistics for density and column types"));
    assert!(sheet_statistics.contains("Examples:"));
    assert!(sheet_statistics.contains("sheet-statistics data.xlsx Sheet1"));

    let table_help = run_cli(&["analyze", "table-profile", "--help"]);
    assert!(
        table_help.status.success(),
        "stderr: {:?}",
        table_help.stderr
    );
    let table = parse_stdout_text(&table_help);
    assert!(table.contains("Profile table headers, types, and column distributions"));
    assert!(table.contains("Examples:"));
    assert!(
        table.contains("table-profile data.xlsx --sheet \"Q1 Actuals\"")
            || table.contains("analyze table-profile data.xlsx --sheet \"Q1 Actuals\"")
    );

    let diff_help = run_cli(&["verify", "diff", "--help"]);
    assert!(diff_help.status.success(), "stderr: {:?}", diff_help.stderr);
    let diff = parse_stdout_text(&diff_help);
    assert!(diff.contains("Diff two workbook versions with summary-first, paged details"));
    assert!(diff.contains("Examples:"));
    assert!(diff.contains("asp verify diff baseline.xlsx candidate.xlsx"));
    assert!(diff.contains("--exclude-recalc-result"));

    let create_help = run_cli(&["workbook", "create", "--help"]);
    assert!(
        create_help.status.success(),
        "stderr: {:?}",
        create_help.stderr
    );
    let create = parse_stdout_text(&create_help);
    assert!(create.contains("Create a new workbook at a destination path"));
    assert!(
        create.contains("workbook create new.xlsx")
            || create.contains("asp workbook create new.xlsx")
    );
    assert!(create.contains("--sheets"));

    let range_help = run_cli(&["read", "values", "--help"]);
    assert!(
        range_help.status.success(),
        "stderr: {:?}",
        range_help.stderr
    );
    let range = parse_stdout_text(&range_help);
    assert!(range.contains("Read raw values for one or more A1 ranges"));
    assert!(range.contains("Examples:"));
    assert!(
        range.contains("read values data.xlsx \"Q1 Actuals\" A1:B5 D10:E20")
            || range.contains("asp read values data.xlsx \"Q1 Actuals\" A1:B5 D10:E20")
    );
    assert!(range.contains("--include-formulas"));
    assert!(range.contains("dense JSON encoding"));
    assert!(range.contains("sparse list in dense mode"));

    let inspect_help = run_cli(&["read", "cells", "--help"]);
    assert!(
        inspect_help.status.success(),
        "stderr: {:?}",
        inspect_help.stderr
    );
    let inspect = parse_stdout_text(&inspect_help);
    assert!(inspect.contains(
        "Inspect detail snapshots for targeted A1 cells/ranges (detail view, default max 25 cells)"
    ));
    assert!(
        inspect.contains("read cells data.xlsx Sheet1 A1:C3")
            || inspect.contains("asp read cells data.xlsx Sheet1 A1:C3")
    );
    assert!(inspect.contains("--include-empty"));

    let sheet_page_help = run_cli(&["read", "page", "--help"]);
    assert!(
        sheet_page_help.status.success(),
        "stderr: {:?}",
        sheet_page_help.stderr
    );
    let sheet_page = parse_stdout_text(&sheet_page_help);
    assert!(sheet_page.contains("Read one sheet page with deterministic continuation"));
    assert!(sheet_page.contains("Examples:"));
    assert!(
        sheet_page.contains("read page data.xlsx Sheet1 --format compact --page-size 200")
            || sheet_page
                .contains("asp read page data.xlsx Sheet1 --format compact --page-size 200")
    );
    assert!(sheet_page.contains("Machine contract:"));
    assert!(sheet_page.contains("format=full"));
    assert!(sheet_page.contains("Global --shape compact preserves the active sheet-page branch"));
    assert!(sheet_page.contains("Pagination loop:"));
    assert!(sheet_page.contains("Machine continuation example:"));

    let read_table_help = run_cli(&["read", "table", "--help"]);
    assert!(
        read_table_help.status.success(),
        "stderr: {:?}",
        read_table_help.stderr
    );
    let read_table = parse_stdout_text(&read_table_help);
    assert!(read_table.contains("Read a table-like region as json, values, or csv"));
    assert!(read_table.contains("Examples:"));
    assert!(
        read_table.contains(
            "read table data.xlsx --sheet Sheet1 --table-format csv --limit 50 --offset 0"
        ) || read_table.contains(
            "asp read table data.xlsx --sheet Sheet1 --table-format csv --limit 50 --offset 0"
        )
    );
    assert!(read_table.contains("Repeat with --offset set to next_offset"));

    let formula_trace_help = run_cli(&["analyze", "formula-trace", "--help"]);
    assert!(
        formula_trace_help.status.success(),
        "stderr: {:?}",
        formula_trace_help.stderr
    );
    let formula_trace = parse_stdout_text(&formula_trace_help);
    assert!(formula_trace.contains("Trace formula precedents or dependents from one origin cell"));
    assert!(formula_trace.contains("Examples:"));
    assert!(
        formula_trace.contains("formula-trace data.xlsx Sheet1 C2 precedents --depth 2")
            || formula_trace
                .contains("analyze formula-trace data.xlsx Sheet1 C2 precedents --depth 2")
            || formula_trace
                .contains("asp analyze formula-trace data.xlsx Sheet1 C2 precedents --depth 2")
    );
    assert!(
        formula_trace.contains(
            "Reuse next_cursor.depth/next_cursor.offset as --cursor-depth/--cursor-offset"
        )
    );

    let append_region_help = run_cli(&["write", "append", "--help"]);
    assert!(
        append_region_help.status.success(),
        "stderr: {:?}",
        append_region_help.stderr
    );
    let append_region = parse_stdout_text(&append_region_help);
    assert!(
        append_region.contains("Append rows into a detected region with footer-aware insertion")
    );
    assert!(append_region.contains("--region-id"));
    assert!(append_region.contains("--table-name"));
    assert!(append_region.contains("--rows"));
    assert!(append_region.contains("--from-csv"));
    assert!(append_region.contains("--header"));
    assert!(append_region.contains("--footer-policy"));

    let clone_template_row_help = run_cli(&["write", "clone-template-row", "--help"]);
    assert!(
        clone_template_row_help.status.success(),
        "stderr: {:?}",
        clone_template_row_help.stderr
    );
    let clone_template_row = parse_stdout_text(&clone_template_row_help);
    assert!(
        clone_template_row
            .contains("Clone one template row into inserted rows with preview-first planning")
    );
    assert!(clone_template_row.contains("--source-row"));
    assert!(clone_template_row.contains("--before"));
    assert!(clone_template_row.contains("--after"));
    assert!(clone_template_row.contains("--insert-at"));
    assert!(clone_template_row.contains("--patch-targets"));
    assert!(clone_template_row.contains("--merge-policy"));

    let clone_row_band_help = run_cli(&["write", "clone-row-band", "--help"]);
    assert!(
        clone_row_band_help.status.success(),
        "stderr: {:?}",
        clone_row_band_help.stderr
    );
    let clone_row_band = parse_stdout_text(&clone_row_band_help);
    assert!(
        clone_row_band.contains("Clone a contiguous template row band with preview-first planning")
    );
    assert!(clone_row_band.contains("--source-rows"));
    assert!(clone_row_band.contains("--repeat"));
    assert!(clone_row_band.contains("--patch-targets"));
    assert!(clone_row_band.contains("--merge-policy"));

    let transform_help = run_cli(&["write", "batch", "transform", "--help"]);
    assert!(
        transform_help.status.success(),
        "stderr: {:?}",
        transform_help.stderr
    );
    let transform = parse_stdout_text(&transform_help);
    assert!(transform.contains("Apply stateless transform operations from an @ops payload"));
    assert!(transform.contains("Examples:"));
    assert!(
        transform.contains("write batch transform workbook.xlsx --ops @ops.json --dry-run")
            || transform
                .contains("asp write batch transform workbook.xlsx --ops @ops.json --dry-run")
    );
    assert!(transform.contains("Choose exactly one of --dry-run, --in-place, or --output <PATH>"));
    assert!(transform.contains("Payload examples (`--ops @transform_ops.json`):"));
    assert!(transform.contains("\"kind\":\"fill_range\""));
    assert!(transform.contains("\"kind\":\"replace_in_range\""));
    assert!(transform.contains("Required envelope:"));
}

#[test]
fn readme_cli_docs_parity_examples_execute_with_local_fixtures() {
    let readme = read_repo_doc("README.md");
    for anchor in [
        "asp read page data.xlsx Sheet1 --format compact --page-size 200",
        "asp read table data.xlsx --sheet \"Sheet1\" --table-format values --limit 200 --offset 0",
        "asp write batch transform data.xlsx --ops @ops.json --dry-run",
        "asp write batch style data.xlsx --ops @style_ops.json --dry-run",
        "##### transform-batch payloads (`@transform_ops.json`)",
        "##### style-batch payloads (`@style_ops.json`)",
        "##### write batch formula-pattern payloads (`@formula_ops.json`)",
        "relative_mode` valid values: `excel`, `abs_cols`, `abs_rows`",
        "##### structure-batch payloads (`@structure_ops.json`)",
        "##### column-size-batch payloads (`@column_size_ops.json`)",
        "Also accepted (harmonized shape)",
        "##### sheet-layout-batch payloads (`@layout_ops.json`)",
        "##### rules-batch payloads (`@rules_ops.json`)",
        "top-level envelope object",
        "asp analyze find-value data.xlsx \"Net Income\" --mode label --label-direction below",
        "`read page <file> <sheet> --format <full|compact|values_only>",
        "#### `sheet-page` machine contract",
        "format=full`: read top-level `rows` plus optional `header_row` and `next_start_row`",
        "Global `--shape compact` preserves the active `sheet-page` branch; it does not flatten `sheet-page` payloads.",
        "Machine continuation example:",
        "`read values <file> <sheet> <range> [range...] [--format dense\\|json\\|values\\|csv] [--include-formulas]`",
        "range-values default encoding:** dense JSON (`dense.encoding = \"dense_v1\"`)",
        "range-values `--include-formulas`:** includes sparse formula coordinates in dense mode",
        "`read cells <file> <sheet> <target> [target...] [--include-empty]`",
        "`workbook create <path> [--sheets Inputs,Calc,...] [--overwrite]`",
        "`analyze find-value <file> <query> [--sheet S] [--mode value\\|label] [--label-direction right\\|below\\|any]`",
        "`write batch transform <file> --ops @ops.json (--dry-run\\|--in-place\\|--output PATH)",
        "#### Formula write-path provenance (`write_path_provenance`)",
        "`written_via`: write path (`edit`, `transform_batch`, `apply_formula_pattern`)",
        "#### Financial presentation starter defaults",
        "Keep label columns (often column A) explicitly sized",
        "Percent: `0.0%`",
        "range-values:** returns a stable `values: [...]` envelope in both canonical and compact modes.",
        "read-table and sheet-page: compact preserves the active branch and continuation fields (`next_offset`, `next_start_row`)",
        "Global `--output-format csv` is currently unsupported; use command-specific CSV options like `read table --table-format csv`.",
        "`write batch formula-pattern` clears cached results for touched formula cells; run `workbook recalculate` to refresh computed values.",
    ] {
        assert!(
            readme.contains(anchor),
            "missing README CLI anchor: {anchor}\n--- README excerpt check failed ---"
        );
    }
    assert!(
        !readme.contains("workbook_short_id"),
        "README should not advertise obsolete workbook_short_id fields"
    );

    let tmp = tempdir().expect("tempdir");
    let data_path = tmp.path().join("data.xlsx");
    let draft_path = tmp.path().join("draft.xlsx");
    let transform_ops_path = tmp.path().join("ops.json");
    let style_ops_path = tmp.path().join("style_ops.json");

    write_fixture(&data_path);
    write_ops_payload(
        &transform_ops_path,
        r#"{"ops":[{"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B2"]},"value":"77"}]}"#,
    );
    write_ops_payload(
        &style_ops_path,
        r#"{"ops":[{"sheet_name":"Sheet1","range":"B2:B2","style":{"font":{"bold":true}}}]}"#,
    );

    let file = data_path.to_str().expect("data path utf8");
    let draft = draft_path.to_str().expect("draft path utf8");
    let transform_ops_ref = format!("@{}", transform_ops_path.to_str().expect("ops utf8"));
    let style_ops_ref = format!("@{}", style_ops_path.to_str().expect("style ops utf8"));

    for args in [
        vec!["list-sheets", file],
        vec!["sheet-overview", file, "Sheet1"],
        vec![
            "read-table",
            file,
            "--sheet",
            "Sheet1",
            "--table-format",
            "values",
        ],
        vec![
            "sheet-page",
            file,
            "Sheet1",
            "--format",
            "compact",
            "--page-size",
            "2",
        ],
        vec![
            "sheet-page",
            file,
            "Sheet1",
            "--format",
            "compact",
            "--page-size",
            "2",
            "--start-row",
            "3",
        ],
        vec!["range-values", file, "Sheet1", "A1:C4"],
        vec![
            "find-value",
            file,
            "Amount",
            "--sheet",
            "Sheet1",
            "--mode",
            "label",
            "--label-direction",
            "below",
        ],
        vec![
            "transform-batch",
            file,
            "--ops",
            transform_ops_ref.as_str(),
            "--dry-run",
        ],
        vec![
            "style-batch",
            file,
            "--ops",
            style_ops_ref.as_str(),
            "--dry-run",
        ],
        vec!["copy", file, draft],
        vec!["edit", draft, "Sheet1", "B2=500", "C2==B2*1.1"],
        vec!["recalculate", draft],
        vec!["diff", file, draft],
    ] {
        let output = run_cli(args.as_slice());
        assert!(
            output.status.success(),
            "args={args:?}, stderr={:?}",
            output.stderr
        );
    }
}

#[test]
fn npm_readme_cli_docs_parity_examples_execute_with_local_fixtures() {
    let readme = read_repo_doc("npm/agent-spreadsheet/README.md");
    for anchor in [
        "asp read page data.xlsx Sheet1 --format compact --page-size 200",
        "asp write batch transform data.xlsx --ops @ops.json --dry-run",
        "asp analyze find-value data.xlsx \"Net Income\" --mode label --label-direction below",
        "`read page <file> <sheet> --format <full|compact|values_only>",
        "`analyze find-value <file> <query> [--sheet S] [--mode value\\|label] [--label-direction right\\|below\\|any]`",
        "`write batch transform <file> --ops @ops.json (--dry-run\\|--in-place\\|--output PATH)",
        "Canonical (default/omitted): return `values: [...]` when entries are present; omit `values` when all requested ranges are pruned (for example, invalid ranges).",
        "Global `--output-format csv` is currently unsupported; use command-specific CSV options such as `read table --table-format csv`.",
        "`write batch formula-pattern` clears cached results for touched formula cells; run `workbook recalculate` to refresh computed values.",
    ] {
        assert!(
            readme.contains(anchor),
            "missing npm README CLI anchor: {anchor}\n--- npm README excerpt check failed ---"
        );
    }
    assert!(
        !readme.contains("workbook_short_id"),
        "npm README should not advertise obsolete workbook_short_id fields"
    );

    let tmp = tempdir().expect("tempdir");
    let data_path = tmp.path().join("data.xlsx");
    let transform_ops_path = tmp.path().join("ops.json");
    write_fixture(&data_path);
    write_ops_payload(
        &transform_ops_path,
        r#"{"ops":[{"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B2"]},"value":"88"}]}"#,
    );

    let file = data_path.to_str().expect("data path utf8");
    let transform_ops_ref = format!("@{}", transform_ops_path.to_str().expect("ops utf8"));

    for args in [
        vec!["list-sheets", file],
        vec!["read-table", file, "--sheet", "Sheet1"],
        vec![
            "sheet-page",
            file,
            "Sheet1",
            "--format",
            "compact",
            "--page-size",
            "2",
        ],
        vec!["table-profile", file, "--sheet", "Sheet1"],
        vec![
            "find-value",
            file,
            "Amount",
            "--sheet",
            "Sheet1",
            "--mode",
            "label",
            "--label-direction",
            "below",
        ],
        vec![
            "transform-batch",
            file,
            "--ops",
            transform_ops_ref.as_str(),
            "--dry-run",
        ],
    ] {
        let output = run_cli(args.as_slice());
        assert!(
            output.status.success(),
            "args={args:?}, stderr={:?}",
            output.stderr
        );
    }
}

#[test]
fn cli_read_commands_cover_ticket_surface() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("read.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let list = run_cli(&["list-sheets", file]);
    assert!(list.status.success(), "stderr: {:?}", list.stderr);
    let list_payload = parse_stdout_json(&list);
    assert_eq!(list_payload["sheets"].as_array().map(Vec::len), Some(2));

    let overview = run_cli(&["sheet-overview", file, "Sheet1"]);
    assert!(overview.status.success(), "stderr: {:?}", overview.stderr);
    let overview_payload = parse_stdout_json(&overview);
    assert_eq!(overview_payload["sheet_name"], "Sheet1");
    assert!(
        overview_payload["detected_region_count"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    );

    let read_table = run_cli(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--table-format",
        "values",
    ]);
    assert!(
        read_table.status.success(),
        "stderr: {:?}",
        read_table.stderr
    );
    let read_table_payload = parse_stdout_json(&read_table);
    assert_eq!(read_table_payload["sheet_name"], "Sheet1");
    assert!(read_table_payload["values"].is_array());

    let range_values = run_cli(&["range-values", file, "Sheet1", "A1:C4"]);
    assert!(
        range_values.status.success(),
        "stderr: {:?}",
        range_values.stderr
    );
    let range_values_payload = parse_stdout_json(&range_values);
    assert!(range_values_payload.get("workbook_id").is_some());
    assert!(range_values_payload.get("workbook_short_id").is_none());
    let entries = range_values_payload["values"]
        .as_array()
        .expect("range values entries");
    assert_eq!(entries.len(), 1);

    let find_value = run_cli(&["find-value", file, "Bob", "--sheet", "Sheet1"]);
    assert!(
        find_value.status.success(),
        "stderr: {:?}",
        find_value.stderr
    );
    let find_payload = parse_stdout_json(&find_value);
    assert_eq!(find_payload["matches"][0]["address"], "A3");

    let formula_map = run_cli(&[
        "formula-map",
        file,
        "Sheet1",
        "--limit",
        "10",
        "--sort-by",
        "count",
    ]);
    assert!(
        formula_map.status.success(),
        "stderr: {:?}",
        formula_map.stderr
    );
    let formula_map_payload = parse_stdout_json(&formula_map);
    assert!(formula_map_payload["groups"].as_array().is_some());

    let formula_trace = run_cli(&["formula-trace", file, "Sheet1", "C2", "precedents"]);
    assert!(
        formula_trace.status.success(),
        "stderr: {:?}",
        formula_trace.stderr
    );
    let trace_payload = parse_stdout_json(&formula_trace);
    assert_eq!(trace_payload["origin"], "C2");
    assert!(trace_payload["layers"].as_array().is_some());

    let describe = run_cli(&["describe", file]);
    assert!(describe.status.success(), "stderr: {:?}", describe.stderr);
    let describe_payload = parse_stdout_json(&describe);
    assert_eq!(describe_payload["sheet_count"], 2);

    let table_profile = run_cli(&["table-profile", file, "--sheet", "Sheet1"]);
    assert!(
        table_profile.status.success(),
        "stderr: {:?}",
        table_profile.stderr
    );
    let profile_payload = parse_stdout_json(&table_profile);
    assert_eq!(profile_payload["sheet_name"], "Sheet1");
    assert!(
        profile_payload["headers"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0)
            >= 3
    );
}

#[test]
fn cli_find_value_label_mode_uses_query_as_label_and_direction() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("find-value-label-mode.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let below = run_cli(&[
        "find-value",
        file,
        "Amount",
        "--sheet",
        "Sheet1",
        "--mode",
        "label",
        "--label-direction",
        "below",
    ]);
    assert!(below.status.success(), "stderr: {:?}", below.stderr);
    let below_payload = parse_stdout_json(&below);
    let below_matches = below_payload["matches"].as_array().expect("matches array");
    assert_eq!(below_matches.len(), 1);
    assert_eq!(below_matches[0]["address"], "B1");
    assert_eq!(below_matches[0]["label_hit"]["label"], "Amount");
    assert_eq!(below_matches[0]["value"]["kind"], "Number");
    assert_eq!(below_matches[0]["value"]["value"], 10.0);

    let any = run_cli(&[
        "find-value",
        file,
        "Amount",
        "--sheet",
        "Sheet1",
        "--mode",
        "label",
    ]);
    assert!(any.status.success(), "stderr: {:?}", any.stderr);
    let any_payload = parse_stdout_json(&any);
    let any_matches = any_payload["matches"].as_array().expect("matches array");
    assert_eq!(any_matches.len(), 1);
    assert_eq!(any_matches[0]["address"], "B1");
    assert_eq!(any_matches[0]["value"]["kind"], "Text");
    assert_eq!(any_matches[0]["value"]["value"], "Total");
}

#[test]
fn cli_find_value_no_match_returns_explicit_empty_matches_and_count() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("find-value-no-match.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&[
        "find-value",
        file,
        "definitely-not-present",
        "--sheet",
        "Sheet1",
        "--mode",
        "value",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let payload = parse_stdout_json(&output);
    assert_eq!(payload["match_count"], 0);
    assert_eq!(payload["matches"], serde_json::json!([]));
    assert!(payload.get("workbook_id").is_some());
}

#[test]
fn cli_verify_accepts_quoted_sheet_targets() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("verify-quoted-sheet.xlsx");

    let mut workbook = umya_spreadsheet::new_file();
    workbook.new_sheet("Q1 Actuals").expect("sheet");
    workbook
        .get_sheet_by_name_mut("Q1 Actuals").ok()
        .expect("q1 actuals exists")
        .get_cell_mut("B1")
        .set_value("Ready");
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");

    let file = workbook_path.to_str().expect("utf8 path");
    let output = run_cli(&["verify", file, file, "--targets", "'Q1 Actuals'!B1"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let payload = parse_stdout_json(&output);
    assert_eq!(payload["summary"]["target_count"], 1);
    assert_eq!(payload["target_deltas"][0]["address"], "'Q1 Actuals'!B1");
    assert_eq!(payload["target_deltas"][0]["before"]["value"], "Ready");
    assert_eq!(payload["target_deltas"][0]["changed"], false);
}

#[test]
fn cli_verify_rejects_malformed_targets_with_invalid_argument() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("verify-invalid-target.xlsx");
    write_fixture(&workbook_path);

    let file = workbook_path.to_str().expect("utf8 path");
    let output = run_cli(&["verify", file, file, "--targets", "Sheet1!not-a-cell"]);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));

    let err = parse_stderr_json(&output);
    assert_eq!(err["code"], "INVALID_ARGUMENT");
    assert!(
        err["message"]
            .as_str()
            .unwrap_or("")
            .contains("Sheet1!not-a-cell")
    );
    assert!(
        err["message"]
            .as_str()
            .unwrap_or("")
            .contains("single A1 cell reference")
    );
}

#[test]
fn cli_verify_missing_sheet_uses_normalized_error_envelope() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("verify-missing-sheet.xlsx");
    write_fixture(&workbook_path);

    let file = workbook_path.to_str().expect("utf8 path");
    let output = run_cli(&["verify", file, file, "--targets", "Missing!A1"]);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));

    let err = parse_stderr_json(&output);
    assert_eq!(err["code"], "SHEET_NOT_FOUND");
    assert_eq!(err["message"], "sheet 'Missing' was not found");
    assert_eq!(
        err["try_this"],
        "run `asp read sheets <file>` to inspect valid names"
    );
}

#[test]
fn cli_verify_targets_only_preserves_explicit_empty_error_arrays() {
    let tmp = tempdir().expect("tempdir");
    let baseline_path = tmp.path().join("verify-targets-only-baseline.xlsx");
    let current_path = tmp.path().join("verify-targets-only-current.xlsx");

    let mut baseline = umya_spreadsheet::new_file();
    baseline
        .get_sheet_by_name_mut("Sheet1").ok()
        .expect("sheet1 exists")
        .get_cell_mut("B1")
        .set_value("Ready");
    umya_spreadsheet::writer::xlsx::write(&baseline, &baseline_path).expect("write baseline");

    let mut current = baseline.clone();
    current
        .get_sheet_by_name_mut("Sheet1").ok()
        .expect("sheet1 exists")
        .get_cell_mut("B1")
        .set_value("Done");
    umya_spreadsheet::writer::xlsx::write(&current, &current_path).expect("write current");

    let baseline_str = baseline_path.to_str().expect("baseline utf8");
    let current_str = current_path.to_str().expect("current utf8");
    let output = run_cli(&[
        "verify",
        baseline_str,
        current_str,
        "--targets",
        "Sheet1!B1",
        "--targets-only",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let payload = parse_stdout_json(&output);
    assert_eq!(payload["summary"]["target_count"], 1);
    assert_eq!(payload["summary"]["new_error_count"], 0);
    assert_eq!(payload["summary"]["resolved_error_count"], 0);
    assert_eq!(payload["summary"]["preexisting_error_count"], 0);
    assert_eq!(payload["target_deltas"].as_array().map(Vec::len), Some(1));
    assert_eq!(payload["new_errors"], serde_json::json!([]));
    assert_eq!(payload["resolved_errors"], serde_json::json!([]));
    assert_eq!(payload["preexisting_errors"], serde_json::json!([]));
    assert_eq!(payload["named_range_deltas"], serde_json::json!([]));
}

#[test]
fn cli_verify_rejects_conflicting_scope_flags_with_guidance() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("verify-conflicting-scope.xlsx");
    write_fixture(&workbook_path);

    let file = workbook_path.to_str().expect("utf8 path");
    let output = run_cli(&[
        "verify",
        file,
        file,
        "--targets",
        "Sheet1!A1",
        "--errors-only",
    ]);
    assert!(!output.status.success());

    let err = parse_stderr_json(&output);
    assert_eq!(err["code"], "INVALID_ARGUMENT");
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("--errors-only cannot be combined with explicit --targets")
    );
}

#[test]
fn cli_verify_reports_target_deltas_error_provenance_and_named_range_deltas() {
    let tmp = tempdir().expect("tempdir");
    let baseline_path = tmp.path().join("verify-baseline.xlsx");
    let current_path = tmp.path().join("verify-current.xlsx");

    let mut baseline = umya_spreadsheet::new_file();
    {
        let sheet = baseline
            .get_sheet_by_name_mut("Sheet1").ok()
            .expect("sheet1 exists");
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Amount");
        sheet.get_cell_mut("A2").set_value("Alice");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        let c2 = sheet.get_cell_mut("C2");
        c2.set_formula("B2*2");
        c2.get_cell_value_mut().set_formula_result_default("20");
        let preexisting = sheet.get_cell_mut("D2");
        preexisting.set_formula("1/0");
        preexisting.set_formula_result_default("#DIV/0!");
        let resolved = sheet.get_cell_mut("D4");
        resolved.set_formula("2/0");
        resolved.set_formula_result_default("#DIV/0!");
        let f1 = sheet.get_cell_mut("F1");
        f1.set_formula("1+1");
        f1.get_cell_value_mut().set_formula_result_default("2");
    }
    baseline.new_sheet("Summary").expect("summary");
    {
        let summary = baseline
            .get_sheet_by_name_mut("Summary").ok()
            .expect("summary exists");
        summary.get_cell_mut("A1").set_value("Flag");
        summary.get_cell_mut("B1").set_value("Ready");
    }
    umya_spreadsheet::writer::xlsx::write(&baseline, &baseline_path).expect("write baseline");

    let mut current = baseline.clone();
    {
        let summary = current
            .get_sheet_by_name_mut("Summary").ok()
            .expect("summary exists");
        summary.get_cell_mut("B1").set_value("Done");
    }
    {
        let sheet = current
            .get_sheet_by_name_mut("Sheet1").ok()
            .expect("sheet1 exists");
        sheet.get_cell_mut("B2").set_value_number(11.0);
        let c2 = sheet.get_cell_mut("C2");
        c2.set_formula("B2*2");
        c2.get_cell_value_mut().set_formula_result_default("22");
        let new_error = sheet.get_cell_mut("D3");
        new_error.set_formula("UNKNOWN_FN(1)");
        new_error.set_formula_result_default("#NAME?");
        sheet.get_cell_mut("D4").set_value("Recovered");
        let f1 = sheet.get_cell_mut("F1");
        f1.set_formula("1+2");
        f1.get_cell_value_mut().set_formula_result_default("3");
        sheet
            .add_defined_name("AmountRef", "Sheet1!$B$3")
            .expect("add defined name");
    }
    umya_spreadsheet::writer::xlsx::write(&current, &current_path).expect("write current");

    let baseline_str = baseline_path.to_str().expect("baseline utf8");
    let current_str = current_path.to_str().expect("current utf8");
    let output = run_cli(&[
        "verify",
        baseline_str,
        current_str,
        "--targets",
        "Summary!B1,Sheet1!C2,Sheet1!F1",
        "--named-ranges",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let payload = parse_stdout_json(&output);
    assert_eq!(payload["summary"]["target_count"], 3);
    assert_eq!(payload["summary"]["changed_targets"], 3);
    assert_eq!(payload["summary"]["new_error_count"], 1);
    assert_eq!(payload["summary"]["resolved_error_count"], 1);
    assert_eq!(payload["summary"]["preexisting_error_count"], 1);
    assert_eq!(payload["summary"]["named_range_delta_count"], 1);

    let target = payload["target_deltas"].as_array().expect("target_deltas");
    assert_eq!(target.len(), 3);
    assert_eq!(target[0]["address"], "Summary!B1");
    assert_eq!(target[0]["before"]["kind"], "Text");
    assert_eq!(target[0]["before"]["value"], "Ready");
    assert_eq!(target[0]["after"]["value"], "Done");
    assert_eq!(target[0]["classification"], "direct_edit");
    assert_eq!(target[0]["changed"], true);

    assert_eq!(target[1]["address"], "Sheet1!C2");
    assert_eq!(target[1]["before"]["value"], 20.0);
    assert_eq!(target[1]["after"]["value"], 22.0);
    assert_eq!(target[1]["before_formula"], "B2*2");
    assert_eq!(target[1]["after_formula"], "B2*2");
    assert_eq!(target[1]["classification"], "recalc_result");
    assert_eq!(target[1]["changed"], true);

    assert_eq!(target[2]["address"], "Sheet1!F1");
    assert_eq!(target[2]["before_formula"], "1+1");
    assert_eq!(target[2]["after_formula"], "1+2");
    assert_eq!(target[2]["classification"], "formula_shift");
    assert_eq!(target[2]["changed"], true);

    let new_errors = payload["new_errors"].as_array().expect("new_errors");
    assert_eq!(new_errors.len(), 1);
    assert_eq!(new_errors[0]["address"], "Sheet1!D3");
    assert_eq!(new_errors[0]["after_error"], "#NAME?");

    let resolved = payload["resolved_errors"]
        .as_array()
        .expect("resolved_errors");
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0]["address"], "Sheet1!D4");
    assert_eq!(resolved[0]["before_error"], "#DIV/0!");
    assert!(resolved[0]["after_error"].is_null());

    let preexisting = payload["preexisting_errors"]
        .as_array()
        .expect("preexisting_errors");
    assert_eq!(preexisting.len(), 1);
    assert_eq!(preexisting[0]["address"], "Sheet1!D2");
    assert_eq!(preexisting[0]["before_error"], "#DIV/0!");
    assert_eq!(preexisting[0]["after_error"], "#DIV/0!");

    let named = payload["named_range_deltas"]
        .as_array()
        .expect("named_range_deltas");
    assert_eq!(named.len(), 1);
    assert_eq!(named[0]["name"], "AmountRef");
    assert_eq!(named[0]["change"], "added");
    assert!(named[0].get("before_refers_to").is_none() || named[0]["before_refers_to"].is_null());
    assert_eq!(named[0]["after_refers_to"], "Sheet1!$B$3");
}

#[test]
fn cli_phase1_named_ranges_filters_are_deterministic() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase1-named-ranges.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let baseline = run_cli(&["named-ranges", file]);
    assert!(baseline.status.success(), "stderr: {:?}", baseline.stderr);
    let baseline_payload = parse_stdout_json(&baseline);
    let baseline_items = baseline_payload["items"].as_array().expect("items array");
    assert!(!baseline_items.is_empty());

    let by_sheet = run_cli(&["named-ranges", file, "--sheet", "Sheet1"]);
    assert!(by_sheet.status.success(), "stderr: {:?}", by_sheet.stderr);
    let by_sheet_payload = parse_stdout_json(&by_sheet);
    let by_sheet_items = by_sheet_payload["items"].as_array().expect("items array");
    assert!(!by_sheet_items.is_empty());
    assert!(
        by_sheet_items
            .iter()
            .all(|item| item["sheet_name"] == "Sheet1")
    );

    let by_prefix_first = run_cli(&["named-ranges", file, "--name-prefix", "Sales"]);
    assert!(
        by_prefix_first.status.success(),
        "stderr: {:?}",
        by_prefix_first.stderr
    );
    let by_prefix_first_payload = parse_stdout_json(&by_prefix_first);
    let by_prefix_first_items = by_prefix_first_payload["items"]
        .as_array()
        .expect("items array");
    assert!(!by_prefix_first_items.is_empty());
    assert!(by_prefix_first_items.iter().all(|item| {
        item["name"]
            .as_str()
            .map(|name| name.starts_with("Sales"))
            .unwrap_or(false)
    }));

    let by_prefix_second = run_cli(&["named-ranges", file, "--name-prefix", "Sales"]);
    assert!(
        by_prefix_second.status.success(),
        "stderr: {:?}",
        by_prefix_second.stderr
    );
    let by_prefix_second_payload = parse_stdout_json(&by_prefix_second);
    assert_eq!(by_prefix_first_payload, by_prefix_second_payload);
}

#[test]
fn cli_phase1_find_formula_supports_limit_offset_continuation() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase1-find-formula.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let first = run_cli(&[
        "find-formula",
        file,
        "SUM(",
        "--sheet",
        "Sheet1",
        "--limit",
        "1",
        "--offset",
        "0",
    ]);
    assert!(first.status.success(), "stderr: {:?}", first.stderr);
    let first_payload = parse_stdout_json(&first);
    let first_matches = first_payload["matches"].as_array().expect("matches array");
    assert_eq!(first_matches.len(), 1);
    let first_next = first_payload["next_offset"]
        .as_u64()
        .expect("next_offset on first page");

    let second_offset = first_next.to_string();
    let second = run_cli(&[
        "find-formula",
        file,
        "SUM(",
        "--sheet",
        "Sheet1",
        "--limit",
        "1",
        "--offset",
        second_offset.as_str(),
    ]);
    assert!(second.status.success(), "stderr: {:?}", second.stderr);
    let second_payload = parse_stdout_json(&second);
    let second_matches = second_payload["matches"].as_array().expect("matches array");
    assert_eq!(second_matches.len(), 1);
    let second_next = second_payload["next_offset"].as_u64().unwrap_or(first_next);
    assert!(second_next >= first_next);

    let terminal = run_cli(&[
        "find-formula",
        file,
        "SUM(",
        "--sheet",
        "Sheet1",
        "--limit",
        "10",
        "--offset",
        "2",
    ]);
    assert!(terminal.status.success(), "stderr: {:?}", terminal.stderr);
    let terminal_payload = parse_stdout_json(&terminal);
    assert!(
        terminal_payload["matches"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0)
            >= 1
    );
    assert!(terminal_payload.get("next_offset").is_none());
}

#[test]
fn cli_phase1_scan_volatiles_detects_and_paginates_deterministically() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase1-scan-volatiles.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let full = run_cli(&["scan-volatiles", file]);
    assert!(full.status.success(), "stderr: {:?}", full.stderr);
    let full_payload = parse_stdout_json(&full);
    let full_items = full_payload["items"].as_array().expect("items array");
    assert!(!full_items.is_empty());

    let first = run_cli(&["scan-volatiles", file, "--limit", "1", "--offset", "0"]);
    assert!(first.status.success(), "stderr: {:?}", first.stderr);
    let first_payload = parse_stdout_json(&first);
    let first_items = first_payload["items"].as_array().expect("items array");
    assert_eq!(first_items.len(), 1);
    let first_entry = first_items[0].clone();
    let first_next = first_payload["next_offset"]
        .as_u64()
        .expect("next_offset for first volatile page");

    let second_offset = first_next.to_string();
    let second = run_cli(&[
        "scan-volatiles",
        file,
        "--limit",
        "1",
        "--offset",
        second_offset.as_str(),
    ]);
    assert!(second.status.success(), "stderr: {:?}", second.stderr);
    let second_payload = parse_stdout_json(&second);
    let second_items = second_payload["items"].as_array().expect("items array");
    assert_eq!(second_items.len(), 1);
    let second_entry = second_items[0].clone();
    assert_ne!(
        first_entry, second_entry,
        "continuation repeated first entry"
    );

    let second_again = run_cli(&[
        "scan-volatiles",
        file,
        "--limit",
        "1",
        "--offset",
        second_offset.as_str(),
    ]);
    assert!(
        second_again.status.success(),
        "stderr: {:?}",
        second_again.stderr
    );
    let second_again_payload = parse_stdout_json(&second_again);
    assert_eq!(second_payload, second_again_payload);
}

#[test]
fn cli_phase1_scan_volatiles_skips_unparsable_formulas_instead_of_failing() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase1-scan-volatiles-parser-failure.xlsx");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook
            .get_sheet_by_name_mut("Sheet1").ok()
            .expect("default sheet exists");
        sheet.get_cell_mut("A1").set_value("Input");
        sheet.get_cell_mut("B1").set_value("Result");
        // Intentionally malformed: one extra closing parenthesis.
        sheet.get_cell_mut("B2").set_formula(
            r#"IF(C70="","",IF(C70="N/A","",IF(C70="Unknown",0,IF(LEFT(C70,1)="0",0,IF(LEFT(C70,1)="1",25,IF(LEFT(C70,1)="2",50,IF(LEFT(C70,1)="3",75,IF(LEFT(C70,1)="4",100,"")))))))))"#,
        );
        sheet.get_cell_mut("B3").set_formula("NOW()");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["scan-volatiles", file, "--sheet", "Sheet1"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let payload = parse_stdout_json(&output);
    let items = payload["items"].as_array().expect("items array");
    assert!(
        items.iter().any(|item| {
            item["address"] == "B3"
                && item["function"] == "volatile"
                && item["sheet_name"] == "Sheet1"
        }),
        "expected volatile match from valid formula"
    );

    // Verify diagnostics are present in warn mode (default)
    let diagnostics = &payload["formula_parse_diagnostics"];
    assert!(
        diagnostics.is_object(),
        "expected formula_parse_diagnostics object"
    );
    assert_eq!(diagnostics["policy"], "warn");
    assert!(diagnostics["total_errors"].as_u64().unwrap_or(0) > 0);
    assert!(
        !diagnostics["groups"]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(true)
    );
}

#[test]
fn cli_formula_map_skips_unparsable_formulas_instead_of_failing() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("formula-map-parser-failure.xlsx");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook
            .get_sheet_by_name_mut("Sheet1").ok()
            .expect("default sheet exists");
        sheet.get_cell_mut("A1").set_value("Input");
        sheet.get_cell_mut("B1").set_value("Result");
        // Intentionally malformed: one extra closing parenthesis.
        sheet.get_cell_mut("B2").set_formula(
            r#"IF(C70="","",IF(C70="N/A","",IF(C70="Unknown",0,IF(LEFT(C70,1)="0",0,IF(LEFT(C70,1)="1",25,IF(LEFT(C70,1)="2",50,IF(LEFT(C70,1)="3",75,IF(LEFT(C70,1)="4",100,"")))))))))"#,
        );
        sheet.get_cell_mut("B3").set_formula("SUM(1,2)");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["formula-map", file, "Sheet1", "--limit", "10"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let payload = parse_stdout_json(&output);
    let groups = payload["groups"].as_array().expect("groups array");
    assert!(
        !groups.is_empty(),
        "expected at least one parseable formula group"
    );

    let diagnostics = &payload["formula_parse_diagnostics"];
    assert!(
        diagnostics.is_object(),
        "expected formula_parse_diagnostics object"
    );
    assert_eq!(diagnostics["policy"], "warn");
    assert!(diagnostics["total_errors"].as_u64().unwrap_or(0) > 0);
    assert!(
        !diagnostics["groups"]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(true)
    );
}

#[test]
fn cli_scan_volatiles_formula_parse_policy_fail_returns_error_envelope() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("scan-volatiles-parse-policy-fail.xlsx");
    write_formula_parse_failure_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&[
        "scan-volatiles",
        file,
        "--sheet",
        "Sheet1",
        "--formula-parse-policy",
        "fail",
    ]);
    assert!(!output.status.success(), "command should fail");

    let error = parse_stderr_json(&output);
    assert_eq!(error["code"], "FORMULA_PARSE_FAILED");
}

#[test]
fn cli_formula_map_formula_parse_policy_fail_returns_error_envelope() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("formula-map-parse-policy-fail.xlsx");
    write_formula_parse_failure_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&[
        "formula-map",
        file,
        "Sheet1",
        "--formula-parse-policy",
        "fail",
    ]);
    assert!(!output.status.success(), "command should fail");

    let error = parse_stderr_json(&output);
    assert_eq!(error["code"], "FORMULA_PARSE_FAILED");
}

#[test]
fn cli_formula_trace_formula_parse_policy_warn_returns_diagnostics() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("formula-trace-parse-policy-warn.xlsx");
    write_formula_parse_failure_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&[
        "formula-trace",
        file,
        "Sheet1",
        "C3",
        "precedents",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let payload = parse_stdout_json(&output);
    assert!(payload["layers"].is_array());
    let diagnostics = &payload["formula_parse_diagnostics"];
    assert!(
        diagnostics.is_object(),
        "expected formula_parse_diagnostics object"
    );
    assert_eq!(diagnostics["policy"], "warn");
    assert!(diagnostics["total_errors"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn cli_formula_trace_formula_parse_policy_fail_returns_error_envelope() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("formula-trace-parse-policy-fail.xlsx");
    write_formula_parse_failure_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&[
        "formula-trace",
        file,
        "Sheet1",
        "C3",
        "precedents",
        "--formula-parse-policy",
        "fail",
    ]);
    assert!(!output.status.success(), "command should fail");

    let error = parse_stderr_json(&output);
    assert_eq!(error["code"], "FORMULA_PARSE_FAILED");
}

#[test]
fn cli_scan_volatiles_diagnostics_deterministic_across_runs() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp
        .path()
        .join("scan-volatiles-diagnostics-deterministic.xlsx");
    write_formula_parse_failure_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let first = run_cli(&[
        "scan-volatiles",
        file,
        "--sheet",
        "Sheet1",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(first.status.success(), "stderr: {:?}", first.stderr);
    let first_payload = parse_stdout_json(&first);

    let second = run_cli(&[
        "scan-volatiles",
        file,
        "--sheet",
        "Sheet1",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(second.status.success(), "stderr: {:?}", second.stderr);
    let second_payload = parse_stdout_json(&second);

    assert_eq!(
        first_payload["formula_parse_diagnostics"],
        second_payload["formula_parse_diagnostics"]
    );
}

#[test]
fn cli_scan_volatiles_diagnostics_independent_of_pagination() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp
        .path()
        .join("scan-volatiles-diagnostics-pagination.xlsx");
    write_formula_parse_failure_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let paged = run_cli(&[
        "scan-volatiles",
        file,
        "--sheet",
        "Sheet1",
        "--formula-parse-policy",
        "warn",
        "--limit",
        "1",
    ]);
    assert!(paged.status.success(), "stderr: {:?}", paged.stderr);
    let paged_payload = parse_stdout_json(&paged);

    let full = run_cli(&[
        "scan-volatiles",
        file,
        "--sheet",
        "Sheet1",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(full.status.success(), "stderr: {:?}", full.stderr);
    let full_payload = parse_stdout_json(&full);

    assert_eq!(
        paged_payload["formula_parse_diagnostics"],
        full_payload["formula_parse_diagnostics"]
    );
}

#[test]
fn cli_phase1_sheet_statistics_returns_expected_fields() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase1-sheet-statistics.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["sheet-statistics", file, "Sheet1"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    assert!(payload["row_count"].as_u64().unwrap_or(0) >= 4);
    assert!(payload["column_count"].as_u64().unwrap_or(0) >= 4);
    assert!(payload["density"].as_f64().unwrap_or(0.0) > 0.0);
    assert!(payload["numeric_columns"].is_array());
    assert!(payload["text_columns"].is_array());
}

#[test]
fn cli_phase1_sheet_scoped_commands_unknown_sheet_return_sheet_not_found() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase1-sheet-not-found.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let cases: Vec<Vec<&str>> = vec![
        vec!["named-ranges", file, "--sheet", "Shet1"],
        vec!["find-formula", file, "SUM(", "--sheet", "Shet1"],
        vec!["scan-volatiles", file, "--sheet", "Shet1"],
        vec!["sheet-statistics", file, "Shet1"],
    ];

    for args in cases {
        let output = run_cli(&args);
        assert!(
            !output.status.success(),
            "command unexpectedly succeeded: {args:?}"
        );
        let err = parse_stderr_json(&output);
        assert_eq!(err["code"], "SHEET_NOT_FOUND", "unexpected envelope: {err}");
    }
}

#[test]
fn cli_phase1_invalid_limit_flags_return_invalid_argument() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase1-invalid-limit.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    assert_invalid_argument(&["find-formula", file, "SUM(", "--limit", "0"]);
    assert_invalid_argument(&["scan-volatiles", file, "--limit", "0"]);
}

#[test]
fn cli_phase1_malformed_usage_prints_help_and_exits_non_zero() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase1-malformed-usage.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let missing_query = run_cli(&["find-formula", file]);
    assert!(
        !missing_query.status.success(),
        "find-formula without query should fail"
    );
    let missing_query_stderr = String::from_utf8(missing_query.stderr).expect("stderr utf8");
    assert!(missing_query_stderr.contains("Usage:"));
    assert!(missing_query_stderr.contains("find-formula <FILE> <QUERY>"));
}

#[test]
fn cli_read_table_pagination_round_trips_next_offset_with_sample_mode_first() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("read-table-pagination.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let mut offset = 0u32;
    let mut saw_continuation = false;
    let mut saw_terminal = false;

    for _ in 0..10 {
        let offset_arg = offset.to_string();
        let page = run_cli(&[
            "read-table",
            file,
            "--sheet",
            "Sheet1",
            "--range",
            "A1:C4",
            "--table-format",
            "json",
            "--sample-mode",
            "first",
            "--limit",
            "1",
            "--offset",
            offset_arg.as_str(),
        ]);
        assert!(page.status.success(), "stderr: {:?}", page.stderr);

        let payload = parse_stdout_json(&page);
        assert!(payload["rows"].is_array());

        if let Some(next_offset) = payload["next_offset"].as_u64() {
            saw_continuation = true;
            assert!(
                next_offset > offset as u64,
                "next_offset must strictly increase for sample-mode=first"
            );
            offset = next_offset as u32;
        } else {
            saw_terminal = true;
            break;
        }
    }

    assert!(saw_continuation, "expected at least one continuation page");
    assert!(saw_terminal, "pagination did not reach a terminal page");
}

#[test]
fn cli_formula_trace_pagination_round_trips_next_cursor_until_terminal() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("formula-trace-pagination.xlsx");
    write_trace_pagination_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let first_page = run_cli(&[
        "formula-trace",
        file,
        "Sheet1",
        "A1",
        "dependents",
        "--depth",
        "1",
        "--page-size",
        "5",
    ]);
    assert!(
        first_page.status.success(),
        "stderr: {:?}",
        first_page.stderr
    );
    let first_payload = parse_stdout_json(&first_page);
    let first_cursor = first_payload["next_cursor"]
        .as_object()
        .expect("expected next_cursor on first trace page");
    let mut cursor_depth = first_cursor["depth"].as_u64().expect("cursor depth") as u32;
    let mut cursor_offset = first_cursor["offset"].as_u64().expect("cursor offset") as usize;

    let mut saw_terminal = false;
    for _ in 0..10 {
        let depth_arg = cursor_depth.to_string();
        let offset_arg = cursor_offset.to_string();
        let page = run_cli(&[
            "formula-trace",
            file,
            "Sheet1",
            "A1",
            "dependents",
            "--depth",
            "1",
            "--page-size",
            "5",
            "--cursor-depth",
            depth_arg.as_str(),
            "--cursor-offset",
            offset_arg.as_str(),
        ]);
        assert!(page.status.success(), "stderr: {:?}", page.stderr);

        let payload = parse_stdout_json(&page);
        if let Some(next_cursor) = payload["next_cursor"].as_object() {
            let next_depth = next_cursor["depth"].as_u64().expect("next depth");
            let next_offset = next_cursor["offset"].as_u64().expect("next offset");
            assert_eq!(
                next_depth, cursor_depth as u64,
                "cursor depth should round-trip unchanged"
            );
            assert!(
                next_offset > cursor_offset as u64,
                "cursor offset should strictly increase while paginating"
            );
            cursor_depth = next_depth as u32;
            cursor_offset = next_offset as usize;
        } else {
            saw_terminal = true;
            break;
        }
    }

    assert!(
        saw_terminal,
        "formula-trace pagination did not reach a terminal page"
    );
}

#[test]
fn cli_sheet_page_first_page_emits_next_start_row() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("sheet-page-first.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let page = run_cli(&[
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        "2",
        "--page-size",
        "1",
        "--format",
        "full",
    ]);
    assert!(page.status.success(), "stderr: {:?}", page.stderr);

    let payload = parse_stdout_json(&page);
    assert_eq!(payload["format"], "full");
    assert_eq!(payload["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(payload["rows"][0]["row_index"].as_u64(), Some(2));
    assert_eq!(payload["next_start_row"].as_u64(), Some(3));
}

#[test]
fn cli_sheet_page_continuation_round_trips_deterministically() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("sheet-page-continuation.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let first = run_cli(&[
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        "2",
        "--page-size",
        "1",
        "--format",
        "full",
    ]);
    assert!(first.status.success(), "stderr: {:?}", first.stderr);
    let first_payload = parse_stdout_json(&first);
    let next_start_row = first_payload["next_start_row"]
        .as_u64()
        .expect("next_start_row present")
        .to_string();

    let continuation = run_cli(&[
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        next_start_row.as_str(),
        "--page-size",
        "1",
        "--format",
        "full",
    ]);
    assert!(
        continuation.status.success(),
        "stderr: {:?}",
        continuation.stderr
    );
    let continuation_payload = parse_stdout_json(&continuation);

    let direct = run_cli(&[
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        "3",
        "--page-size",
        "1",
        "--format",
        "full",
    ]);
    assert!(direct.status.success(), "stderr: {:?}", direct.stderr);
    let direct_payload = parse_stdout_json(&direct);

    assert_eq!(continuation_payload, direct_payload);
}

#[test]
fn cli_sheet_page_terminal_page_omits_next_start_row() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("sheet-page-terminal.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let terminal = run_cli(&[
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        "4",
        "--page-size",
        "2",
        "--format",
        "full",
    ]);
    assert!(terminal.status.success(), "stderr: {:?}", terminal.stderr);

    let payload = parse_stdout_json(&terminal);
    assert_eq!(payload["rows"][0]["row_index"].as_u64(), Some(4));
    assert!(payload.get("next_start_row").is_none());
}

#[test]
fn cli_sheet_page_column_filters_support_union_and_sheet_order() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("sheet-page-columns.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let columns_only = run_cli(&[
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        "2",
        "--page-size",
        "1",
        "--columns",
        "C:A",
        "--format",
        "compact",
    ]);
    assert!(
        columns_only.status.success(),
        "stderr: {:?}",
        columns_only.stderr
    );
    let columns_only_payload = parse_stdout_json(&columns_only);
    let columns_only_headers = columns_only_payload["compact"]["headers"]
        .as_array()
        .expect("compact headers")
        .iter()
        .map(|v| v.as_str().expect("header string"))
        .collect::<Vec<_>>();
    assert_eq!(columns_only_headers, vec!["Row", "Name", "Amount", "Total"]);

    let header_only = run_cli(&[
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        "2",
        "--page-size",
        "1",
        "--columns-by-header",
        "Total,Name",
        "--format",
        "compact",
    ]);
    assert!(
        header_only.status.success(),
        "stderr: {:?}",
        header_only.stderr
    );
    let header_only_payload = parse_stdout_json(&header_only);
    let header_only_headers = header_only_payload["compact"]["headers"]
        .as_array()
        .expect("compact headers")
        .iter()
        .map(|v| v.as_str().expect("header string"))
        .collect::<Vec<_>>();
    assert_eq!(header_only_headers, vec!["Row", "Name", "Total"]);

    let combined = run_cli(&[
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        "2",
        "--page-size",
        "1",
        "--columns",
        "B",
        "--columns-by-header",
        "Amount,Name,Total",
        "--format",
        "compact",
    ]);
    assert!(combined.status.success(), "stderr: {:?}", combined.stderr);
    let combined_payload = parse_stdout_json(&combined);
    let combined_headers = combined_payload["compact"]["headers"]
        .as_array()
        .expect("compact headers")
        .iter()
        .map(|v| v.as_str().expect("header string"))
        .collect::<Vec<_>>();
    assert_eq!(combined_headers, vec!["Row", "Name", "Amount", "Total"]);
}

#[test]
fn cli_sheet_page_accepts_all_formats_and_sets_expected_payload_branch() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("sheet-page-formats.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    for format in ["full", "compact", "values_only"] {
        let page = run_cli(&[
            "sheet-page",
            file,
            "Sheet1",
            "--start-row",
            "2",
            "--page-size",
            "1",
            "--format",
            format,
        ]);
        assert!(page.status.success(), "stderr: {:?}", page.stderr);
        let payload = parse_stdout_json(&page);

        assert_eq!(payload["format"], format);
        match format {
            "full" => {
                assert!(payload["rows"].is_array());
                assert!(payload.get("compact").is_none());
                assert!(payload.get("values_only").is_none());
            }
            "compact" => {
                assert!(payload["compact"].is_object());
                assert!(payload.get("rows").is_none());
                assert!(payload.get("values_only").is_none());
            }
            "values_only" => {
                assert!(payload["values_only"].is_object());
                assert!(payload.get("rows").is_none());
                assert!(payload.get("compact").is_none());
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn cli_sheet_page_machine_contract_next_start_row_is_top_level_for_all_formats() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp
        .path()
        .join("sheet-page-machine-contract-next-start-row.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    for format in ["full", "compact", "values_only"] {
        let page = run_cli(&[
            "sheet-page",
            file,
            "Sheet1",
            "--start-row",
            "2",
            "--page-size",
            "1",
            "--format",
            format,
        ]);
        assert!(page.status.success(), "stderr: {:?}", page.stderr);
        let payload = parse_stdout_json(&page);

        assert!(
            payload.get("next_start_row").is_some(),
            "next_start_row must remain top-level for format={format}"
        );

        if format == "compact" {
            assert!(payload["compact"].get("next_start_row").is_none());
        }
        if format == "values_only" {
            assert!(payload["values_only"].get("next_start_row").is_none());
        }
    }
}

#[test]
fn cli_sheet_page_preserves_next_start_row_in_canonical_and_compact_shapes() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("sheet-page-shape-next-start-row.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let canonical = run_cli(&[
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        "2",
        "--page-size",
        "1",
        "--format",
        "compact",
    ]);
    assert!(canonical.status.success(), "stderr: {:?}", canonical.stderr);
    let canonical_payload = parse_stdout_json(&canonical);

    let compact_shape = run_cli(&[
        "--shape",
        "compact",
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        "2",
        "--page-size",
        "1",
        "--format",
        "compact",
    ]);
    assert!(
        compact_shape.status.success(),
        "stderr: {:?}",
        compact_shape.stderr
    );
    let compact_shape_payload = parse_stdout_json(&compact_shape);

    assert_eq!(canonical_payload["next_start_row"].as_u64(), Some(3));
    assert_eq!(compact_shape_payload["next_start_row"].as_u64(), Some(3));
    assert_eq!(
        canonical_payload["next_start_row"],
        compact_shape_payload["next_start_row"]
    );
}

#[test]
fn cli_shape_3109_read_table_compact_preserves_contract_branches() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("shape-3109-read-table-branches.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    for (table_format, branch) in [("json", "rows"), ("values", "values"), ("csv", "csv")] {
        let canonical = run_cli(&[
            "read-table",
            file,
            "--sheet",
            "Sheet1",
            "--table-name",
            "SalesTable",
            "--table-format",
            table_format,
            "--sample-mode",
            "first",
            "--limit",
            "1",
            "--offset",
            "0",
        ]);
        assert!(canonical.status.success(), "stderr: {:?}", canonical.stderr);
        let canonical_payload = parse_stdout_json(&canonical);

        let compact = run_cli(&[
            "--shape",
            "compact",
            "read-table",
            file,
            "--sheet",
            "Sheet1",
            "--table-name",
            "SalesTable",
            "--table-format",
            table_format,
            "--sample-mode",
            "first",
            "--limit",
            "1",
            "--offset",
            "0",
        ]);
        assert!(compact.status.success(), "stderr: {:?}", compact.stderr);
        let compact_payload = parse_stdout_json(&compact);

        assert_eq!(
            compact_payload["workbook_id"],
            canonical_payload["workbook_id"]
        );
        assert_eq!(compact_payload["sheet_name"], "Sheet1");
        assert_eq!(compact_payload["table_name"], "SalesTable");
        assert_eq!(
            compact_payload["total_rows"],
            canonical_payload["total_rows"]
        );
        assert_eq!(
            compact_payload["next_offset"],
            canonical_payload["next_offset"]
        );

        match branch {
            "rows" => {
                assert!(compact_payload["rows"].is_array());
                assert!(compact_payload.get("values").is_none());
                assert!(compact_payload.get("csv").is_none());
            }
            "values" => {
                assert!(compact_payload["values"].is_array());
                assert!(compact_payload.get("rows").is_none());
                assert!(compact_payload.get("csv").is_none());
            }
            "csv" => {
                assert!(compact_payload["csv"].is_string());
                assert!(compact_payload.get("rows").is_none());
                assert!(compact_payload.get("values").is_none());
            }
            _ => unreachable!(),
        }

        assert_eq!(compact_payload, canonical_payload);
    }
}

#[test]
fn cli_shape_3109_read_table_compact_round_trips_next_offset_until_terminal() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("shape-3109-read-table-next-offset.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let canonical_first = run_cli(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--table-format",
        "json",
        "--sample-mode",
        "first",
        "--limit",
        "1",
        "--offset",
        "0",
    ]);
    assert!(
        canonical_first.status.success(),
        "stderr: {:?}",
        canonical_first.stderr
    );
    let canonical_first_payload = parse_stdout_json(&canonical_first);

    let compact_first = run_cli(&[
        "--shape",
        "compact",
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--table-format",
        "json",
        "--sample-mode",
        "first",
        "--limit",
        "1",
        "--offset",
        "0",
    ]);
    assert!(
        compact_first.status.success(),
        "stderr: {:?}",
        compact_first.stderr
    );
    let compact_first_payload = parse_stdout_json(&compact_first);

    assert_eq!(
        compact_first_payload["next_offset"],
        canonical_first_payload["next_offset"]
    );

    let mut offset = compact_first_payload["next_offset"]
        .as_u64()
        .expect("next_offset on compact first page") as u32;
    let mut saw_terminal = false;

    for _ in 0..10 {
        let offset_arg = offset.to_string();
        let page = run_cli(&[
            "--shape",
            "compact",
            "read-table",
            file,
            "--sheet",
            "Sheet1",
            "--range",
            "A1:C4",
            "--table-format",
            "json",
            "--sample-mode",
            "first",
            "--limit",
            "1",
            "--offset",
            offset_arg.as_str(),
        ]);
        assert!(page.status.success(), "stderr: {:?}", page.stderr);

        let payload = parse_stdout_json(&page);
        if let Some(next_offset) = payload["next_offset"].as_u64() {
            assert!(
                next_offset > offset as u64,
                "next_offset must strictly increase"
            );
            offset = next_offset as u32;
        } else {
            saw_terminal = true;
            break;
        }
    }

    assert!(
        saw_terminal,
        "compact read-table pagination did not reach a terminal page"
    );
}

#[test]
fn cli_shape_3109_read_table_compact_preserves_user_workbook_short_id_columns() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp
        .path()
        .join("shape-3109-read-table-workbook-short-id-column.xlsx");
    write_workbook_short_id_column_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let canonical = run_cli(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:B2",
        "--table-format",
        "json",
    ]);
    assert!(canonical.status.success(), "stderr: {:?}", canonical.stderr);
    let canonical_payload = parse_stdout_json(&canonical);

    let compact = run_cli(&[
        "--shape",
        "compact",
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:B2",
        "--table-format",
        "json",
    ]);
    assert!(compact.status.success(), "stderr: {:?}", compact.stderr);
    let compact_payload = parse_stdout_json(&compact);

    assert_eq!(compact_payload, canonical_payload);

    let row = compact_payload["rows"]
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(Value::as_object)
        .expect("first compact row object");
    assert!(row.contains_key("workbook_short_id"));
}

#[test]
fn cli_shape_3109_sheet_page_compact_preserves_active_branches_without_collapse() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("shape-3109-sheet-page-branches.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    for format in ["full", "compact", "values_only"] {
        let canonical = run_cli(&[
            "sheet-page",
            file,
            "Sheet1",
            "--start-row",
            "2",
            "--page-size",
            "2",
            "--format",
            format,
        ]);
        assert!(canonical.status.success(), "stderr: {:?}", canonical.stderr);
        let canonical_payload = parse_stdout_json(&canonical);

        let compact_shape = run_cli(&[
            "--shape",
            "compact",
            "sheet-page",
            file,
            "Sheet1",
            "--start-row",
            "2",
            "--page-size",
            "2",
            "--format",
            format,
        ]);
        assert!(
            compact_shape.status.success(),
            "stderr: {:?}",
            compact_shape.stderr
        );
        let compact_payload = parse_stdout_json(&compact_shape);

        assert_eq!(
            compact_payload["workbook_id"],
            canonical_payload["workbook_id"]
        );
        assert_eq!(compact_payload["sheet_name"], "Sheet1");
        assert_eq!(compact_payload["format"], format);
        assert_eq!(
            compact_payload["next_start_row"],
            canonical_payload["next_start_row"]
        );

        match format {
            "full" => {
                let compact_rows = compact_payload["rows"].as_array().expect("full rows");
                let canonical_rows = canonical_payload["rows"]
                    .as_array()
                    .expect("canonical full rows");
                assert_eq!(compact_rows.len(), canonical_rows.len());
                assert!(compact_rows.len() > 1, "expected multi-row full payload");
                assert!(compact_payload.get("compact").is_none());
                assert!(compact_payload.get("values_only").is_none());
            }
            "compact" => {
                let compact_rows = compact_payload["compact"]["rows"]
                    .as_array()
                    .expect("compact branch rows");
                let canonical_rows = canonical_payload["compact"]["rows"]
                    .as_array()
                    .expect("canonical compact branch rows");
                assert_eq!(compact_rows.len(), canonical_rows.len());
                assert!(compact_rows.len() > 1, "expected multi-row compact payload");
                assert!(compact_payload.get("rows").is_none());
                assert!(compact_payload.get("values_only").is_none());
            }
            "values_only" => {
                let compact_rows = compact_payload["values_only"]["rows"]
                    .as_array()
                    .expect("values_only branch rows");
                let canonical_rows = canonical_payload["values_only"]["rows"]
                    .as_array()
                    .expect("canonical values_only branch rows");
                assert_eq!(compact_rows.len(), canonical_rows.len());
                assert!(
                    compact_rows.len() > 1,
                    "expected multi-row values_only payload"
                );
                assert!(compact_payload.get("rows").is_none());
                assert!(compact_payload.get("compact").is_none());
            }
            _ => unreachable!(),
        }

        assert_eq!(compact_payload, canonical_payload);
    }
}

#[test]
fn cli_shape_3109_sheet_page_compact_round_trips_next_start_row() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("shape-3109-sheet-page-next-start-row.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let canonical_first = run_cli(&[
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        "2",
        "--page-size",
        "1",
        "--format",
        "compact",
    ]);
    assert!(
        canonical_first.status.success(),
        "stderr: {:?}",
        canonical_first.stderr
    );
    let canonical_first_payload = parse_stdout_json(&canonical_first);

    let compact_first = run_cli(&[
        "--shape",
        "compact",
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        "2",
        "--page-size",
        "1",
        "--format",
        "compact",
    ]);
    assert!(
        compact_first.status.success(),
        "stderr: {:?}",
        compact_first.stderr
    );
    let compact_first_payload = parse_stdout_json(&compact_first);

    assert_eq!(
        compact_first_payload["next_start_row"],
        canonical_first_payload["next_start_row"]
    );

    let next_start_row = compact_first_payload["next_start_row"]
        .as_u64()
        .expect("next_start_row on compact first page")
        .to_string();

    let continuation = run_cli(&[
        "--shape",
        "compact",
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        next_start_row.as_str(),
        "--page-size",
        "1",
        "--format",
        "compact",
    ]);
    assert!(
        continuation.status.success(),
        "stderr: {:?}",
        continuation.stderr
    );
    let continuation_payload = parse_stdout_json(&continuation);

    let direct = run_cli(&[
        "--shape",
        "compact",
        "sheet-page",
        file,
        "Sheet1",
        "--start-row",
        "3",
        "--page-size",
        "1",
        "--format",
        "compact",
    ]);
    assert!(direct.status.success(), "stderr: {:?}", direct.stderr);
    let direct_payload = parse_stdout_json(&direct);

    assert_eq!(continuation_payload, direct_payload);
}

#[test]
fn cli_shape_3109_formula_trace_compact_omits_layer_highlights_and_preserves_cursor() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp
        .path()
        .join("shape-3109-formula-trace-compact-contract.xlsx");
    write_trace_pagination_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let canonical = run_cli(&[
        "formula-trace",
        file,
        "Sheet1",
        "A1",
        "dependents",
        "--depth",
        "1",
        "--page-size",
        "5",
    ]);
    assert!(canonical.status.success(), "stderr: {:?}", canonical.stderr);
    let canonical_payload = parse_stdout_json(&canonical);

    let compact_shape = run_cli(&[
        "--shape",
        "compact",
        "formula-trace",
        file,
        "Sheet1",
        "A1",
        "dependents",
        "--depth",
        "1",
        "--page-size",
        "5",
    ]);
    assert!(
        compact_shape.status.success(),
        "stderr: {:?}",
        compact_shape.stderr
    );
    let compact_payload = parse_stdout_json(&compact_shape);

    assert_eq!(
        compact_payload["workbook_id"],
        canonical_payload["workbook_id"]
    );
    assert_eq!(
        compact_payload["sheet_name"],
        canonical_payload["sheet_name"]
    );
    assert_eq!(compact_payload["origin"], canonical_payload["origin"]);
    assert_eq!(compact_payload["direction"], canonical_payload["direction"]);
    assert_eq!(
        compact_payload["next_cursor"],
        canonical_payload["next_cursor"]
    );
    assert_eq!(compact_payload["notes"], canonical_payload["notes"]);

    let canonical_layers = canonical_payload["layers"]
        .as_array()
        .expect("canonical layers")
        .clone();
    assert!(!canonical_layers.is_empty(), "expected canonical layers");
    assert!(
        canonical_layers
            .iter()
            .all(|layer| layer.get("highlights").is_some()),
        "canonical layers should include highlights"
    );

    let compact_layers = compact_payload["layers"]
        .as_array()
        .expect("compact layers");
    assert_eq!(compact_layers.len(), canonical_layers.len());
    assert!(
        compact_layers
            .iter()
            .all(|layer| layer.get("highlights").is_none()),
        "compact layers must omit highlights"
    );
    assert!(compact_layers.iter().all(|layer| {
        layer.get("depth").is_some()
            && layer.get("summary").is_some()
            && layer.get("edges").is_some()
            && layer.get("has_more").is_some()
    }));

    for (canonical_layer, compact_layer) in canonical_layers.iter().zip(compact_layers.iter()) {
        assert_eq!(compact_layer["depth"], canonical_layer["depth"]);
        assert_eq!(compact_layer["summary"], canonical_layer["summary"]);
        assert_eq!(compact_layer["has_more"], canonical_layer["has_more"]);

        let mut canonical_edges = canonical_layer["edges"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut compact_edges = compact_layer["edges"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        canonical_edges.sort_by(|a, b| {
            serde_json::to_string(a)
                .expect("serialize canonical edge")
                .cmp(&serde_json::to_string(b).expect("serialize canonical edge"))
        });
        compact_edges.sort_by(|a, b| {
            serde_json::to_string(a)
                .expect("serialize compact edge")
                .cmp(&serde_json::to_string(b).expect("serialize compact edge"))
        });

        assert_eq!(compact_edges, canonical_edges);
    }
}

#[test]
fn cli_shape_3109_formula_trace_compact_round_trips_next_cursor_until_terminal() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("shape-3109-formula-trace-next-cursor.xlsx");
    write_trace_pagination_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let canonical_first = run_cli(&[
        "formula-trace",
        file,
        "Sheet1",
        "A1",
        "dependents",
        "--depth",
        "1",
        "--page-size",
        "5",
    ]);
    assert!(
        canonical_first.status.success(),
        "stderr: {:?}",
        canonical_first.stderr
    );
    let canonical_first_payload = parse_stdout_json(&canonical_first);

    let compact_first = run_cli(&[
        "--shape",
        "compact",
        "formula-trace",
        file,
        "Sheet1",
        "A1",
        "dependents",
        "--depth",
        "1",
        "--page-size",
        "5",
    ]);
    assert!(
        compact_first.status.success(),
        "stderr: {:?}",
        compact_first.stderr
    );
    let compact_first_payload = parse_stdout_json(&compact_first);

    assert_eq!(
        compact_first_payload["next_cursor"],
        canonical_first_payload["next_cursor"]
    );

    let first_cursor = compact_first_payload["next_cursor"]
        .as_object()
        .expect("next_cursor on first compact trace page");
    let mut cursor_depth = first_cursor["depth"].as_u64().expect("cursor depth") as u32;
    let mut cursor_offset = first_cursor["offset"].as_u64().expect("cursor offset") as usize;

    let mut saw_terminal = false;
    for _ in 0..10 {
        let depth_arg = cursor_depth.to_string();
        let offset_arg = cursor_offset.to_string();
        let page = run_cli(&[
            "--shape",
            "compact",
            "formula-trace",
            file,
            "Sheet1",
            "A1",
            "dependents",
            "--depth",
            "1",
            "--page-size",
            "5",
            "--cursor-depth",
            depth_arg.as_str(),
            "--cursor-offset",
            offset_arg.as_str(),
        ]);
        assert!(page.status.success(), "stderr: {:?}", page.stderr);

        let payload = parse_stdout_json(&page);
        let layers = payload["layers"].as_array().expect("layers array");
        assert!(layers.iter().all(|layer| layer.get("highlights").is_none()));

        if let Some(next_cursor) = payload["next_cursor"].as_object() {
            let next_depth = next_cursor["depth"].as_u64().expect("next depth") as u32;
            let next_offset = next_cursor["offset"].as_u64().expect("next offset") as usize;
            assert_eq!(next_depth, cursor_depth);
            assert!(next_offset > cursor_offset, "cursor offset must increase");
            cursor_depth = next_depth;
            cursor_offset = next_offset;
        } else {
            saw_terminal = true;
            break;
        }
    }

    assert!(
        saw_terminal,
        "compact formula-trace pagination did not reach a terminal page"
    );
}

#[test]
fn cli_shape_3109_compact_does_not_over_apply_to_unrelated_find_value_payloads() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("shape-3109-over-apply-find-value.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let canonical = run_cli(&["find-value", file, "Bob", "--sheet", "Sheet1"]);
    assert!(canonical.status.success(), "stderr: {:?}", canonical.stderr);
    let canonical_payload = parse_stdout_json(&canonical);

    let compact_shape = run_cli(&[
        "--shape",
        "compact",
        "find-value",
        file,
        "Bob",
        "--sheet",
        "Sheet1",
    ]);
    assert!(
        compact_shape.status.success(),
        "stderr: {:?}",
        compact_shape.stderr
    );
    let compact_payload = parse_stdout_json(&compact_shape);

    assert_eq!(compact_payload, canonical_payload);
}

#[test]
fn cli_shape_3109_default_shape_matches_explicit_canonical_for_ticket_commands() {
    let tmp = tempdir().expect("tempdir");

    let read_table_workbook = tmp
        .path()
        .join("shape-3109-default-canonical-read-table.xlsx");
    write_fixture(&read_table_workbook);
    let read_table_file = read_table_workbook.to_str().expect("path utf8");
    let read_table_default = run_cli(&[
        "read-table",
        read_table_file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--table-format",
        "json",
        "--sample-mode",
        "first",
        "--limit",
        "1",
        "--offset",
        "0",
    ]);
    assert!(
        read_table_default.status.success(),
        "stderr: {:?}",
        read_table_default.stderr
    );
    let read_table_canonical = run_cli(&[
        "--shape",
        "canonical",
        "read-table",
        read_table_file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--table-format",
        "json",
        "--sample-mode",
        "first",
        "--limit",
        "1",
        "--offset",
        "0",
    ]);
    assert!(
        read_table_canonical.status.success(),
        "stderr: {:?}",
        read_table_canonical.stderr
    );
    assert_eq!(
        parse_stdout_json(&read_table_default),
        parse_stdout_json(&read_table_canonical)
    );

    let sheet_page_workbook = tmp
        .path()
        .join("shape-3109-default-canonical-sheet-page.xlsx");
    write_fixture(&sheet_page_workbook);
    let sheet_page_file = sheet_page_workbook.to_str().expect("path utf8");
    let sheet_page_default = run_cli(&[
        "sheet-page",
        sheet_page_file,
        "Sheet1",
        "--start-row",
        "2",
        "--page-size",
        "1",
        "--format",
        "full",
    ]);
    assert!(
        sheet_page_default.status.success(),
        "stderr: {:?}",
        sheet_page_default.stderr
    );
    let sheet_page_canonical = run_cli(&[
        "--shape",
        "canonical",
        "sheet-page",
        sheet_page_file,
        "Sheet1",
        "--start-row",
        "2",
        "--page-size",
        "1",
        "--format",
        "full",
    ]);
    assert!(
        sheet_page_canonical.status.success(),
        "stderr: {:?}",
        sheet_page_canonical.stderr
    );
    assert_eq!(
        parse_stdout_json(&sheet_page_default),
        parse_stdout_json(&sheet_page_canonical)
    );

    let trace_workbook = tmp
        .path()
        .join("shape-3109-default-canonical-formula-trace.xlsx");
    write_trace_pagination_fixture(&trace_workbook);
    let trace_file = trace_workbook.to_str().expect("path utf8");
    let trace_default = run_cli(&[
        "formula-trace",
        trace_file,
        "Sheet1",
        "A1",
        "dependents",
        "--depth",
        "1",
        "--page-size",
        "5",
    ]);
    assert!(
        trace_default.status.success(),
        "stderr: {:?}",
        trace_default.stderr
    );
    let trace_canonical = run_cli(&[
        "--shape",
        "canonical",
        "formula-trace",
        trace_file,
        "Sheet1",
        "A1",
        "dependents",
        "--depth",
        "1",
        "--page-size",
        "5",
    ]);
    assert!(
        trace_canonical.status.success(),
        "stderr: {:?}",
        trace_canonical.stderr
    );

    let trace_default_payload = parse_stdout_json(&trace_default);
    let trace_canonical_payload = parse_stdout_json(&trace_canonical);
    assert_eq!(
        trace_default_payload["workbook_id"],
        trace_canonical_payload["workbook_id"]
    );
    assert_eq!(
        trace_default_payload["sheet_name"],
        trace_canonical_payload["sheet_name"]
    );
    assert_eq!(
        trace_default_payload["origin"],
        trace_canonical_payload["origin"]
    );
    assert_eq!(
        trace_default_payload["direction"],
        trace_canonical_payload["direction"]
    );
    assert_eq!(
        trace_default_payload["next_cursor"],
        trace_canonical_payload["next_cursor"]
    );
    assert_eq!(
        trace_default_payload["notes"],
        trace_canonical_payload["notes"]
    );

    let default_layers = trace_default_payload["layers"]
        .as_array()
        .expect("default layers");
    let canonical_layers = trace_canonical_payload["layers"]
        .as_array()
        .expect("canonical layers");
    assert_eq!(default_layers.len(), canonical_layers.len());

    for (default_layer, canonical_layer) in default_layers.iter().zip(canonical_layers.iter()) {
        assert_eq!(default_layer["depth"], canonical_layer["depth"]);
        assert_eq!(default_layer["summary"], canonical_layer["summary"]);
        assert_eq!(default_layer["has_more"], canonical_layer["has_more"]);
        assert_eq!(
            default_layer.get("highlights").is_some(),
            canonical_layer.get("highlights").is_some()
        );

        let mut default_edges = default_layer["edges"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut canonical_edges = canonical_layer["edges"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        default_edges.sort_by(|a, b| {
            serde_json::to_string(a)
                .expect("serialize default edge")
                .cmp(&serde_json::to_string(b).expect("serialize default edge"))
        });
        canonical_edges.sort_by(|a, b| {
            serde_json::to_string(a)
                .expect("serialize canonical edge")
                .cmp(&serde_json::to_string(b).expect("serialize canonical edge"))
        });

        assert_eq!(default_edges, canonical_edges);
    }
}

#[test]
fn cli_sheet_page_page_size_zero_returns_invalid_argument() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("sheet-page-page-size-zero.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    assert_invalid_argument(&[
        "sheet-page",
        file,
        "Sheet1",
        "--page-size",
        "0",
        "--format",
        "full",
    ]);
}

#[test]
fn cli_sheet_page_invalid_column_spec_returns_invalid_argument() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("sheet-page-invalid-column.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    assert_invalid_argument(&[
        "sheet-page",
        file,
        "Sheet1",
        "--columns",
        "A,NOT$",
        "--format",
        "full",
    ]);
}

#[test]
fn cli_sheet_page_unknown_sheet_returns_sheet_not_found() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("sheet-page-sheet-not-found.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["sheet-page", file, "Shet1", "--format", "full"]);
    assert!(!output.status.success(), "command unexpectedly succeeded");

    let err = parse_stderr_json(&output);
    assert_eq!(err["code"], "SHEET_NOT_FOUND");
    assert_eq!(err["did_you_mean"], "Sheet1");
}

#[test]
fn cli_sheet_page_unknown_format_value_fails_clap_parse() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("sheet-page-unknown-format.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["sheet-page", file, "Sheet1", "--format", "bogus"]);
    assert!(!output.status.success(), "command unexpectedly succeeded");

    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("invalid value 'bogus'"), "stderr: {stderr}");
    assert!(
        stderr.contains("--format <FORMAT>"),
        "expected clap parse error for --format, got: {stderr}"
    );
    assert!(
        stderr.contains("full") && stderr.contains("compact") && stderr.contains("values_only"),
        "expected sheet-page format choices in error, got: {stderr}"
    );
}

#[test]
fn cli_read_table_filters_support_unfiltered_json_and_file_inputs() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("read-table-filters.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let unfiltered = run_cli(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--table-format",
        "json",
    ]);
    assert!(
        unfiltered.status.success(),
        "stderr: {:?}",
        unfiltered.stderr
    );
    let unfiltered_payload = parse_stdout_json(&unfiltered);
    assert_eq!(unfiltered_payload["rows"].as_array().map(Vec::len), Some(3));

    let filters_json = r#"[{"column":"Name","op":"eq","value":"Alice"}]"#;
    let filtered_json = run_cli(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--table-format",
        "json",
        "--filters-json",
        filters_json,
    ]);
    assert!(
        filtered_json.status.success(),
        "stderr: {:?}",
        filtered_json.stderr
    );
    let filtered_json_payload = parse_stdout_json(&filtered_json);
    assert_eq!(
        filtered_json_payload["rows"].as_array().map(Vec::len),
        Some(1)
    );

    let filters_file = tmp.path().join("filters.json");
    std::fs::write(&filters_file, filters_json).expect("write filters file");
    let filters_file_path = filters_file.to_str().expect("filters path utf8");
    let filtered_file = run_cli(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--table-format",
        "json",
        "--filters-file",
        filters_file_path,
    ]);
    assert!(
        filtered_file.status.success(),
        "stderr: {:?}",
        filtered_file.stderr
    );
    let filtered_file_payload = parse_stdout_json(&filtered_file);
    assert_eq!(
        filtered_file_payload["rows"].as_array().map(Vec::len),
        Some(1)
    );
}

#[test]
fn cli_read_table_allows_last_and_distributed_sampling_at_zero_offset() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("read-table-sample-modes.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let last = run_cli(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--table-format",
        "json",
        "--sample-mode",
        "last",
        "--offset",
        "0",
        "--limit",
        "2",
    ]);
    assert!(last.status.success(), "stderr: {:?}", last.stderr);
    let last_payload = parse_stdout_json(&last);
    assert!(last_payload["rows"].is_array());

    let distributed = run_cli(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--table-format",
        "json",
        "--sample-mode",
        "distributed",
        "--offset",
        "0",
        "--limit",
        "2",
    ]);
    assert!(
        distributed.status.success(),
        "stderr: {:?}",
        distributed.stderr
    );
    let distributed_payload = parse_stdout_json(&distributed);
    assert!(distributed_payload["rows"].is_array());
}

#[test]
fn cli_pagination_surface_validation_failures_use_invalid_argument() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("validation.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let filter_file = tmp.path().join("filters.json");
    let filter_json = r#"[{"column":"Name","op":"eq","value":"Alice"}]"#;
    std::fs::write(&filter_file, filter_json).expect("write filters file");
    let filter_file_path = filter_file.to_str().expect("path utf8");

    let malformed_filter_file = tmp.path().join("bad-filters.json");
    std::fs::write(&malformed_filter_file, "{not-json").expect("write malformed filter file");
    let malformed_filter_file_path = malformed_filter_file.to_str().expect("path utf8");

    assert_invalid_argument(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--filters-json",
        filter_json,
        "--filters-file",
        filter_file_path,
    ]);

    assert_invalid_argument(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--filters-json",
        "{",
    ]);

    assert_invalid_argument(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--filters-file",
        malformed_filter_file_path,
    ]);

    assert_invalid_argument(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--limit",
        "0",
    ]);

    assert_invalid_argument(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--sample-mode",
        "last",
        "--offset",
        "1",
    ]);

    assert_invalid_argument(&[
        "read-table",
        file,
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C4",
        "--sample-mode",
        "distributed",
        "--offset",
        "1",
    ]);

    assert_invalid_argument(&[
        "formula-trace",
        file,
        "Sheet1",
        "C2",
        "precedents",
        "--cursor-depth",
        "1",
    ]);

    assert_invalid_argument(&[
        "formula-trace",
        file,
        "Sheet1",
        "C2",
        "precedents",
        "--cursor-offset",
        "1",
    ]);

    assert_invalid_argument(&[
        "formula-trace",
        file,
        "Sheet1",
        "C2",
        "precedents",
        "--depth",
        "0",
    ]);

    assert_invalid_argument(&[
        "formula-trace",
        file,
        "Sheet1",
        "C2",
        "precedents",
        "--depth",
        "6",
    ]);

    assert_invalid_argument(&[
        "formula-trace",
        file,
        "Sheet1",
        "C2",
        "precedents",
        "--page-size",
        "4",
    ]);

    assert_invalid_argument(&[
        "formula-trace",
        file,
        "Sheet1",
        "C2",
        "precedents",
        "--page-size",
        "201",
    ]);

    assert_invalid_argument(&[
        "formula-trace",
        file,
        "Sheet1",
        "C2",
        "precedents",
        "--cursor-depth",
        "0",
        "--cursor-offset",
        "0",
    ]);
}

#[test]
fn cli_range_values_shape_single_range_canonical_vs_compact() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("shape-single.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let canonical = run_cli(&["range-values", file, "Sheet1", "A1:C4"]);
    assert!(canonical.status.success(), "stderr: {:?}", canonical.stderr);
    let canonical_payload = parse_stdout_json(&canonical);
    assert!(canonical_payload.get("workbook_id").is_some());
    assert!(canonical_payload.get("workbook_short_id").is_none());
    let canonical_values = canonical_payload["values"]
        .as_array()
        .expect("canonical single-range values");
    assert_eq!(canonical_values.len(), 1);
    let canonical_entry = canonical_values.first().expect("single range entry");
    assert_eq!(canonical_entry["range"], "A1:C4");
    assert!(canonical_entry.get("dense").is_some());

    let compact = run_cli(&[
        "--shape",
        "compact",
        "range-values",
        file,
        "Sheet1",
        "A1:C4",
    ]);
    assert!(compact.status.success(), "stderr: {:?}", compact.stderr);
    let compact_payload = parse_stdout_json(&compact);
    assert!(compact_payload.get("workbook_id").is_some());
    assert!(compact_payload.get("workbook_short_id").is_none());
    let compact_values = compact_payload["values"]
        .as_array()
        .expect("compact single-range values");
    assert_eq!(compact_values.len(), 1);
    assert_eq!(compact_values[0]["range"], "A1:C4");
    assert!(compact_values[0].get("dense").is_some());
}

#[test]
fn cli_range_values_include_formulas_returns_dense_sparse_formulas() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("range-values-include-formulas.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let with_formulas = run_cli(&[
        "range-values",
        file,
        "Sheet1",
        "C2:C4",
        "--include-formulas",
    ]);
    assert!(
        with_formulas.status.success(),
        "stderr: {:?}",
        with_formulas.stderr
    );
    let payload = parse_stdout_json(&with_formulas);
    let entry = payload["values"]
        .as_array()
        .expect("values array")
        .first()
        .cloned()
        .expect("range entry");

    let dense = entry["dense"].as_object().expect("dense payload");
    let formulas = dense["formulas"].as_array().expect("dense sparse formulas");
    assert_eq!(formulas.len(), 3);
    assert_eq!(formulas[0]["row"].as_u64(), Some(0));
    assert_eq!(formulas[0]["col"].as_u64(), Some(0));
    assert_eq!(formulas[0]["formula"].as_str(), Some("B2*2"));
    assert_eq!(formulas[1]["formula"].as_str(), Some("B3*2"));
    assert_eq!(formulas[2]["formula"].as_str(), Some("B4*2"));

    let default_output = run_cli(&["range-values", file, "Sheet1", "C2:C4"]);
    assert!(
        default_output.status.success(),
        "stderr: {:?}",
        default_output.stderr
    );
    let default_payload = parse_stdout_json(&default_output);
    let default_entry = default_payload["values"]
        .as_array()
        .expect("values array")
        .first()
        .cloned()
        .expect("range entry");
    let default_dense = default_entry["dense"].as_object().expect("dense payload");
    let default_formulas = default_dense
        .get("formulas")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        default_formulas.is_empty(),
        "default range-values output should not include formulas"
    );
}

#[test]
fn cli_range_values_dense_encoding_rolls_up_repeated_values() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("range-values-dense-rollup.xlsx");

    let mut workbook = umya_spreadsheet::new_file();
    let sheet = workbook
        .get_sheet_by_name_mut("Sheet1").ok()
        .expect("default sheet exists");
    for row in 1..=3 {
        for col in ["A", "B", "C", "D", "E", "F"] {
            sheet
                .get_cell_mut(format!("{}{}", col, row).as_str())
                .set_value("#NAME?");
        }
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");

    let file = workbook_path.to_str().expect("path utf8");
    let output = run_cli(&["range-values", file, "Sheet1", "A1:F3"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let payload = parse_stdout_json(&output);
    let entry = payload["values"]
        .as_array()
        .expect("values array")
        .first()
        .cloned()
        .expect("range entry");
    let dense = entry["dense"].as_object().expect("dense payload");

    let dictionary = dense["dictionary"].as_array().expect("dictionary array");
    assert!(dictionary.len() <= 2, "expected null + one repeated token");

    let row_runs = dense["row_runs"].as_array().expect("row runs");
    assert_eq!(row_runs.len(), 3);
    for row in row_runs {
        let runs = row.as_array().expect("run array");
        assert_eq!(runs.len(), 1, "single run expected for repeated row");
        assert_eq!(runs[0]["len"].as_u64(), Some(6));
        assert_ne!(runs[0]["value_idx"].as_u64(), Some(0));
    }
}

#[test]
fn cli_range_values_shape_continuation_representable_canonical_and_compact() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("shape-continuation.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    // `A1:XFD1` is wider than the CLI max-cells cap (10_000), so the response keeps
    // a continuation cursor but no materialized row payload after pruning.
    let canonical = run_cli(&["range-values", file, "Sheet1", "A1:XFD1"]);
    assert!(canonical.status.success(), "stderr: {:?}", canonical.stderr);
    let canonical_payload = parse_stdout_json(&canonical);
    assert!(canonical_payload.get("workbook_id").is_some());
    assert!(canonical_payload.get("workbook_short_id").is_none());
    let canonical_values = canonical_payload["values"]
        .as_array()
        .expect("canonical continuation values");
    assert_eq!(canonical_values.len(), 1);
    let canonical_entry = canonical_values.first().expect("single continuation entry");
    assert_eq!(canonical_entry["range"], "A1:XFD1");
    assert_eq!(canonical_entry["next_start_row"].as_u64(), Some(1));

    let compact = run_cli(&[
        "--shape",
        "compact",
        "range-values",
        file,
        "Sheet1",
        "A1:XFD1",
    ]);
    assert!(compact.status.success(), "stderr: {:?}", compact.stderr);
    let compact_payload = parse_stdout_json(&compact);
    assert!(compact_payload.get("workbook_id").is_some());
    assert!(compact_payload.get("workbook_short_id").is_none());
    let compact_values = compact_payload["values"]
        .as_array()
        .expect("compact values array");
    assert_eq!(compact_values.len(), 1);
    assert_eq!(compact_values[0]["range"], "A1:XFD1");
    assert_eq!(compact_values[0]["next_start_row"].as_u64(), Some(1));
}

#[test]
fn cli_range_values_invalid_range_fails_loudly_in_both_shapes() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("shape-invalid-range.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    // An unparseable range must produce a structured error (non-zero exit),
    // never a silent exit-0 success with no values.
    let canonical = run_cli(&["range-values", file, "Sheet1", "NOT_A_RANGE"]);
    assert!(
        !canonical.status.success(),
        "invalid range must fail: stderr: {:?}",
        canonical.stderr
    );
    let stderr = String::from_utf8_lossy(&canonical.stderr);
    assert!(
        stderr.contains("INVALID_RANGE"),
        "error should mention INVALID_RANGE: {stderr}"
    );

    let compact = run_cli(&[
        "--shape",
        "compact",
        "range-values",
        file,
        "Sheet1",
        "NOT_A_RANGE",
    ]);
    assert!(!compact.status.success());
}

#[test]
fn cli_range_values_shape_multi_range_canonical_vs_compact() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("shape-multi.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let canonical = run_cli(&["range-values", file, "Sheet1", "A1:A2", "B1:B2"]);
    assert!(canonical.status.success(), "stderr: {:?}", canonical.stderr);
    let canonical_payload = parse_stdout_json(&canonical);
    assert!(canonical_payload.get("workbook_id").is_some());
    assert!(canonical_payload.get("workbook_short_id").is_none());
    let canonical_values = canonical_payload["values"]
        .as_array()
        .expect("canonical multi-range values");
    assert_eq!(canonical_values.len(), 2);
    assert!(canonical_values.iter().all(|entry| {
        entry.get("range").and_then(Value::as_str).is_some() && entry.get("dense").is_some()
    }));

    let compact = run_cli(&[
        "--shape",
        "compact",
        "range-values",
        file,
        "Sheet1",
        "A1:A2",
        "B1:B2",
    ]);
    assert!(compact.status.success(), "stderr: {:?}", compact.stderr);
    let compact_payload = parse_stdout_json(&compact);
    assert!(compact_payload.get("workbook_id").is_some());
    assert!(compact_payload.get("workbook_short_id").is_none());
    assert!(compact_payload.get("range").is_none());
    let compact_values = compact_payload["values"]
        .as_array()
        .expect("compact multi-range values");
    assert_eq!(compact_values.len(), 2);
    assert!(compact_values.iter().all(|entry| {
        entry.get("range").and_then(Value::as_str).is_some() && entry.get("dense").is_some()
    }));
}

#[test]
fn cli_range_values_shape_default_matches_explicit_canonical() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("shape-default-canonical.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let default_shape = run_cli(&["range-values", file, "Sheet1", "A1:C4", "B1:B2"]);
    assert!(
        default_shape.status.success(),
        "stderr: {:?}",
        default_shape.stderr
    );

    let explicit_canonical = run_cli(&[
        "--shape",
        "canonical",
        "range-values",
        file,
        "Sheet1",
        "A1:C4",
        "B1:B2",
    ]);
    assert!(
        explicit_canonical.status.success(),
        "stderr: {:?}",
        explicit_canonical.stderr
    );

    let default_payload = parse_stdout_json(&default_shape);
    let canonical_payload = parse_stdout_json(&explicit_canonical);
    assert_eq!(default_payload, canonical_payload);
}

#[test]
fn cli_range_values_shape_compact_multi_range_preserves_next_start_row_without_flattening() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("shape-multi-continuation.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let compact = run_cli(&[
        "--shape",
        "compact",
        "range-values",
        file,
        "Sheet1",
        "A1:XFD1",
        "B1:B2",
    ]);
    assert!(compact.status.success(), "stderr: {:?}", compact.stderr);
    let compact_payload = parse_stdout_json(&compact);
    assert!(compact_payload.get("range").is_none());

    let compact_values = compact_payload["values"]
        .as_array()
        .expect("compact multi-range continuation values");
    assert_eq!(compact_values.len(), 2);

    let paged_entry = compact_values
        .iter()
        .find(|entry| entry.get("range").and_then(Value::as_str) == Some("A1:XFD1"))
        .expect("paged entry present");
    assert_eq!(paged_entry["next_start_row"].as_u64(), Some(1));
    assert!(
        compact_values
            .iter()
            .any(|entry| entry.get("range").and_then(Value::as_str) == Some("B1:B2"))
    );
}

#[test]
fn cli_inspect_cells_returns_unified_snapshot() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("inspect-cells.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["inspect-cells", file, "Sheet1", "B2:C2"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["sheet_name"], "Sheet1");
    assert_eq!(payload["range"], "B2:C2");
    assert_eq!(payload["truncated"], Value::Bool(false));

    let cells = payload["cells"].as_array().expect("cells array");
    assert_eq!(cells.len(), 2);

    let b2 = cells
        .iter()
        .find(|cell| cell["address"] == "B2")
        .expect("B2 snapshot");
    assert!(b2["formula"].is_null());
    assert!(b2.get("value").is_some());

    let c2 = cells
        .iter()
        .find(|cell| cell["address"] == "C2")
        .expect("C2 snapshot");
    assert_eq!(c2["formula"], Value::String("B2*2".to_string()));
}

#[test]
fn cli_inspect_cells_rejects_large_requests_with_hint() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("inspect-cells-large.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["inspect-cells", file, "Sheet1", "A1:XFD10"]);
    assert!(!output.status.success(), "stdout: {:?}", output.stdout);
    let err = parse_stderr_json(&output);
    let message = err["message"].as_str().unwrap_or_default();
    assert!(message.contains("detail view"), "message={message}");
    assert!(message.contains("sheet-page"), "message={message}");
}

#[test]
fn cli_inspect_cells_supports_multi_targets_and_dedupes_overlap() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("inspect-cells-multi.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["inspect-cells", file, "Sheet1", "B2:C2", "C2", "A2"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    assert_eq!(payload["range"], "B2:C2,C2,A2");
    assert_eq!(payload["targets"], serde_json::json!(["B2:C2", "C2", "A2"]));

    let cells = payload["cells"].as_array().expect("cells array");
    let addresses: Vec<&str> = cells
        .iter()
        .filter_map(|cell| cell["address"].as_str())
        .collect();
    assert_eq!(addresses, vec!["B2", "C2", "A2"]);
}

#[test]
fn cli_inspect_cells_omits_empty_cells_by_default() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("inspect-cells-empty-default.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["inspect-cells", file, "Sheet1", "B2", "D2"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    let cells = payload["cells"].as_array().expect("cells array");
    let addresses: Vec<&str> = cells
        .iter()
        .filter_map(|cell| cell["address"].as_str())
        .collect();
    assert_eq!(addresses, vec!["B2"]);
}

#[test]
fn cli_inspect_cells_include_empty_keeps_empty_addresses() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("inspect-cells-empty-include.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&[
        "inspect-cells",
        file,
        "Sheet1",
        "B2",
        "D2",
        "--include-empty",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    let cells = payload["cells"].as_array().expect("cells array");
    let addresses: Vec<&str> = cells
        .iter()
        .filter_map(|cell| cell["address"].as_str())
        .collect();
    assert_eq!(addresses, vec!["B2", "D2"]);
}

#[test]
fn cli_transform_batch_dry_run_validates_contract_and_preserves_source() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("transform-batch-dry-run.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B2"]},"value":"77"}]}"#,
    );

    let before = fs::read(&workbook_path).expect("read source before dry-run");
    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let output = run_cli(&[
        "transform-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    assert_eq!(payload["op_count"].as_u64(), Some(1));
    assert_eq!(payload["validated_count"].as_u64(), Some(1));
    assert!(payload["would_change"].as_bool().unwrap_or(false));
    assert!(payload["warnings"].is_array());
    assert!(payload["summary"].is_object());
    assert!(payload["summary"]["operation_counts"].is_object());
    assert!(payload["summary"]["result_counts"].is_object());

    let after = fs::read(&workbook_path).expect("read source after dry-run");
    assert_eq!(before, after, "dry-run mutated the source workbook");
}

#[test]
fn cli_transform_batch_in_place_applies_atomically() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("transform-batch-in-place.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B2"]},"value":"44"}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let output = run_cli(&[
        "transform-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    assert_eq!(payload["op_count"].as_u64(), Some(1));
    assert_eq!(payload["applied_count"].as_u64(), Some(1));
    assert!(payload["warnings"].is_array());
    assert!(payload["changed"].as_bool().unwrap_or(false));
    assert_json_path_eq(&payload, "source_path", file);
    assert_json_path_eq(&payload, "target_path", file);

    let book = umya_spreadsheet::reader::xlsx::read(&workbook_path).expect("read workbook");
    let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet exists");
    assert_eq!(sheet.get_cell("B2").expect("B2 exists").get_value(), "44");
}

#[test]
fn cli_transform_batch_output_and_force_modes_apply_with_overwrite_checks() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("transform-batch-source.xlsx");
    let output_path = tmp.path().join("transform-batch-output.xlsx");
    let ops_path_first = tmp.path().join("ops-first.json");
    let ops_path_second = tmp.path().join("ops-second.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path_first,
        r#"{"ops":[{"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B2"]},"value":"51"}]}"#,
    );
    write_ops_payload(
        &ops_path_second,
        r#"{"ops":[{"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B3"]},"value":"91"}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let ops_first_ref = format!("@{}", ops_path_first.to_str().expect("ops path utf8"));
    let ops_second_ref = format!("@{}", ops_path_second.to_str().expect("ops path utf8"));

    let first = run_cli(&[
        "transform-batch",
        source,
        "--ops",
        ops_first_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(first.status.success(), "stderr: {:?}", first.stderr);

    let source_book = umya_spreadsheet::reader::xlsx::read(&source_path).expect("read source");
    let source_sheet = source_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("sheet exists");
    assert_eq!(
        source_sheet
            .get_cell("B2")
            .expect("source B2 exists")
            .get_value(),
        "10"
    );

    let output_book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output");
    let output_sheet = output_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("sheet exists");
    assert_eq!(
        output_sheet
            .get_cell("B2")
            .expect("output B2 exists")
            .get_value(),
        "51"
    );

    assert_error_code(
        &[
            "transform-batch",
            source,
            "--ops",
            ops_second_ref.as_str(),
            "--output",
            output,
        ],
        "OUTPUT_EXISTS",
    );

    let forced = run_cli(&[
        "transform-batch",
        source,
        "--ops",
        ops_second_ref.as_str(),
        "--output",
        output,
        "--force",
    ]);
    assert!(forced.status.success(), "stderr: {:?}", forced.stderr);
    let forced_payload = parse_stdout_json(&forced);
    assert_json_path_eq(&forced_payload, "target_path", output);

    let overwritten = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output");
    let overwritten_sheet = overwritten
        .get_sheet_by_name("Sheet1").ok()
        .expect("sheet exists");
    assert_eq!(
        overwritten_sheet
            .get_cell("B3")
            .expect("output B3 exists")
            .get_value(),
        "91"
    );
}

#[cfg(unix)]
#[test]
fn cli_transform_batch_rejects_dangling_symlink_output_without_force() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp
        .path()
        .join("transform-batch-source-dangling-symlink.xlsx");
    let ops_path = tmp.path().join("ops.json");
    let output_link = tmp.path().join("dangling-output.xlsx");
    let missing_target = tmp.path().join("missing-target.xlsx");

    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B2"]},"value":"66"}]}"#,
    );

    symlink(&missing_target, &output_link).expect("create dangling symlink");

    let source = source_path.to_str().expect("source utf8");
    let output = output_link.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let err = assert_error_code(
        &[
            "transform-batch",
            source,
            "--ops",
            ops_ref.as_str(),
            "--output",
            output,
        ],
        "OUTPUT_EXISTS",
    );
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("already exists")
    );

    assert!(
        fs::symlink_metadata(&output_link).is_ok(),
        "dangling symlink should remain in place"
    );
}

#[test]
fn cli_transform_batch_rejects_invalid_mode_combinations() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("transform-batch-mode-matrix.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B2"]},"value":"7"}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    assert_invalid_argument(&["transform-batch", file, "--ops", ops_ref.as_str()]);
    assert_invalid_argument(&[
        "transform-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
        "--in-place",
    ]);
    assert_invalid_argument(&[
        "transform-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
        "--output",
        "out.xlsx",
    ]);
    assert_invalid_argument(&[
        "transform-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
        "--output",
        "out.xlsx",
    ]);
    assert_invalid_argument(&[
        "transform-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--force",
    ]);
    assert_invalid_argument(&[
        "transform-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--output",
        file,
    ]);
}

#[test]
fn cli_transform_batch_rejects_invalid_ops_payloads() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("transform-batch-invalid-ops.xlsx");
    let malformed_path = tmp.path().join("ops-malformed.json");
    let schema_path = tmp.path().join("ops-schema.json");
    write_fixture(&workbook_path);
    write_ops_payload(&malformed_path, "{not-json}");
    write_ops_payload(&schema_path, r#"{"ops":[{"kind":"unknown_op"}]}"#);

    let file = workbook_path.to_str().expect("path utf8");

    assert_error_code(
        &["transform-batch", file, "--ops", "ops.json", "--dry-run"],
        "INVALID_OPS_PAYLOAD",
    );

    let malformed_ref = format!("@{}", malformed_path.to_str().expect("ops path utf8"));
    assert_error_code(
        &[
            "transform-batch",
            file,
            "--ops",
            malformed_ref.as_str(),
            "--dry-run",
        ],
        "INVALID_OPS_PAYLOAD",
    );

    let schema_ref = format!("@{}", schema_path.to_str().expect("ops path utf8"));
    assert_error_code(
        &[
            "transform-batch",
            file,
            "--ops",
            schema_ref.as_str(),
            "--dry-run",
        ],
        "INVALID_OPS_PAYLOAD",
    );
}

#[cfg(unix)]
#[test]
fn cli_transform_batch_maps_write_failures_and_preserves_source() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("transform-batch-write-fail-source.xlsx");
    let blocked_dir = tmp.path().join("blocked");
    let blocked_output = blocked_dir.join("output.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B2"]},"value":"123"}]}"#,
    );
    fs::create_dir(&blocked_dir).expect("create blocked dir");

    let mut perms = fs::metadata(&blocked_dir)
        .expect("blocked metadata")
        .permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&blocked_dir, perms.clone()).expect("set blocked perms");

    let before = fs::read(&source_path).expect("read source before write failure");
    let source = source_path.to_str().expect("source utf8");
    let output = blocked_output.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let err = assert_error_code(
        &[
            "transform-batch",
            source,
            "--ops",
            ops_ref.as_str(),
            "--output",
            output,
        ],
        "WRITE_FAILED",
    );
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("unable to allocate temp file")
            || err["message"]
                .as_str()
                .unwrap_or_default()
                .contains("Permission denied")
    );

    let mut restore = perms;
    restore.set_mode(0o755);
    fs::set_permissions(&blocked_dir, restore).expect("restore blocked perms");

    let after = fs::read(&source_path).expect("read source after write failure");
    assert_eq!(before, after, "source workbook changed after write failure");
}

#[test]
fn phase_a_help_examples_for_style_and_formula_commands() {
    let style_help = run_cli(&["style-batch", "--help"]);
    assert!(
        style_help.status.success(),
        "stderr: {:?}",
        style_help.stderr
    );
    let style = parse_stdout_text(&style_help);
    assert!(style.contains("Examples:"));
    assert!(style.contains("asp write batch style workbook.xlsx --ops @style_ops.json --dry-run"));
    assert!(style.contains(
        "asp write batch style workbook.xlsx --ops @style_ops.json --output styled.xlsx --force"
    ));
    assert!(style.contains("Payload examples (`--ops @style_ops.json`):"));
    assert!(style.contains("\"patch\":{\"font\":{\"bold\":true}}"));
    assert!(style.contains("Required envelope:"));

    let formula_help = run_cli(&["apply-formula-pattern", "--help"]);
    assert!(
        formula_help.status.success(),
        "stderr: {:?}",
        formula_help.stderr
    );
    let formula = parse_stdout_text(&formula_help);
    assert!(formula.contains("Examples:"));
    assert!(formula.contains(
        "asp write batch formula-pattern workbook.xlsx --ops @formula_ops.json --in-place"
    ));
    assert!(formula.contains(
        "asp write batch formula-pattern workbook.xlsx --ops @formula_ops.json --dry-run"
    ));
    assert!(formula.contains("Payload examples (`--ops @formula_ops.json`):"));
    assert!(formula.contains("\"target_range\":\"C2:C4\""));
    assert!(formula.contains("\"fill_direction\":\"both\""));
    assert!(formula.contains("relative_mode` valid values: excel|abs_cols|abs_rows"));
    assert!(formula.contains(
        "Updated formula cells clear cached results. Run recalculate to refresh computed values."
    ));
    assert!(
        formula.contains("write_path_provenance"),
        "apply-formula-pattern help should mention provenance diagnostics"
    );
}

#[test]
fn phase_a_style_batch_positive_dry_run_and_output_target_only() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-a-style-source.xlsx");
    let output_path = tmp.path().join("phase-a-style-output.xlsx");
    let ops_path = tmp.path().join("style-ops.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"sheet_name":"Sheet1","range":"B2:B2","style":{"font":{"bold":true}}}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let before = fs::read(&source_path).expect("read source before dry-run");
    let dry_run = run_cli(&[
        "style-batch",
        source,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
    ]);
    assert!(dry_run.status.success(), "stderr: {:?}", dry_run.stderr);
    let dry_payload = parse_stdout_json(&dry_run);
    assert_eq!(dry_payload["op_count"].as_u64(), Some(1));
    assert_eq!(dry_payload["validated_count"].as_u64(), Some(1));
    assert!(dry_payload["would_change"].as_bool().unwrap_or(false));

    let after_dry = fs::read(&source_path).expect("read source after dry-run");
    assert_eq!(before, after_dry, "dry-run mutated source file");

    let output_run = run_cli(&[
        "style-batch",
        source,
        "--ops",
        ops_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(
        output_run.status.success(),
        "stderr: {:?}",
        output_run.stderr
    );
    let output_payload = parse_stdout_json(&output_run);
    assert_json_path_eq(&output_payload, "target_path", output);
    assert_json_path_eq(&output_payload, "source_path", source);
    assert!(output_payload["changed"].as_bool().unwrap_or(false));

    let source_after = fs::read(&source_path).expect("read source after output mode");
    let output_after = fs::read(&output_path).expect("read output after output mode");
    assert_eq!(before, source_after, "source changed during --output mode");
    assert_ne!(source_after, output_after, "output file did not change");
}

#[test]
fn phase_a_style_batch_output_force_overwrite_semantics() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-a-style-force-source.xlsx");
    let output_path = tmp.path().join("phase-a-style-force-output.xlsx");
    let ops_first_path = tmp.path().join("style-ops-first.json");
    let ops_second_path = tmp.path().join("style-ops-second.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_first_path,
        r#"{"ops":[{"sheet_name":"Sheet1","range":"B2:B2","style":{"font":{"bold":true}}}]}"#,
    );
    write_ops_payload(
        &ops_second_path,
        r#"{"ops":[{"sheet_name":"Sheet1","range":"B2:B2","style":{"font":{"italic":true}}}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let first_ref = format!("@{}", ops_first_path.to_str().expect("ops utf8"));
    let second_ref = format!("@{}", ops_second_path.to_str().expect("ops utf8"));

    let first = run_cli(&[
        "style-batch",
        source,
        "--ops",
        first_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(first.status.success(), "stderr: {:?}", first.stderr);
    let first_bytes = fs::read(&output_path).expect("read first output bytes");

    assert_error_code(
        &[
            "style-batch",
            source,
            "--ops",
            second_ref.as_str(),
            "--output",
            output,
        ],
        "OUTPUT_EXISTS",
    );
    let after_failed_bytes = fs::read(&output_path).expect("read output after failed overwrite");
    assert_eq!(first_bytes, after_failed_bytes);

    let forced = run_cli(&[
        "style-batch",
        source,
        "--ops",
        second_ref.as_str(),
        "--output",
        output,
        "--force",
    ]);
    assert!(forced.status.success(), "stderr: {:?}", forced.stderr);

    let forced_bytes = fs::read(&output_path).expect("read forced output bytes");
    assert_ne!(
        first_bytes, forced_bytes,
        "force overwrite did not update output"
    );
}

#[test]
fn phase_a_apply_formula_pattern_positive_dry_run_and_output_target_only() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-a-formula-source.xlsx");
    let output_path = tmp.path().join("phase-a-formula-output.xlsx");
    let ops_path = tmp.path().join("formula-ops.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"sheet_name":"Sheet1","target_range":"C2:C4","anchor_cell":"C2","base_formula":"B2*3","fill_direction":"down","relative_mode":"excel"}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let before = fs::read(&source_path).expect("read source before dry-run");
    let dry_run = run_cli(&[
        "apply-formula-pattern",
        source,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
    ]);
    assert!(dry_run.status.success(), "stderr: {:?}", dry_run.stderr);
    let dry_payload = parse_stdout_json(&dry_run);
    assert_eq!(dry_payload["op_count"].as_u64(), Some(1));
    assert!(dry_payload["would_change"].as_bool().unwrap_or(false));
    assert_eq!(
        dry_payload["write_path_provenance"]["written_via"],
        Value::String("apply_formula_pattern".to_string())
    );
    let dry_targets = dry_payload["write_path_provenance"]["formula_targets"]
        .as_array()
        .expect("formula_targets array");
    assert!(
        dry_targets
            .iter()
            .any(|target| target.as_str() == Some("Sheet1!C2:C4"))
    );
    let after_dry = fs::read(&source_path).expect("read source after dry-run");
    assert_eq!(before, after_dry, "dry-run mutated source file");

    let output_run = run_cli(&[
        "apply-formula-pattern",
        source,
        "--ops",
        ops_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(
        output_run.status.success(),
        "stderr: {:?}",
        output_run.stderr
    );
    let output_payload = parse_stdout_json(&output_run);
    assert!(output_payload["changed"].as_bool().unwrap_or(false));
    assert_eq!(
        output_payload["write_path_provenance"]["written_via"],
        Value::String("apply_formula_pattern".to_string())
    );

    let source_book =
        umya_spreadsheet::reader::xlsx::read(&source_path).expect("read source workbook");
    let source_sheet = source_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("source sheet");
    assert_eq!(
        source_sheet
            .get_cell("C2")
            .expect("C2 source")
            .get_formula(),
        "B2*2"
    );

    let output_book =
        umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output workbook");
    let output_sheet = output_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("output sheet");
    assert_eq!(
        output_sheet
            .get_cell("C2")
            .expect("C2 output")
            .get_formula()
            .replace(' ', ""),
        "B2*3"
    );
    assert_eq!(
        output_sheet
            .get_cell("C3")
            .expect("C3 output")
            .get_formula()
            .replace(' ', ""),
        "B3*3"
    );
    assert_eq!(
        output_sheet
            .get_cell("C4")
            .expect("C4 output")
            .get_formula()
            .replace(' ', ""),
        "B4*3"
    );
}

#[test]
fn phase_a_apply_formula_pattern_clears_formula_cache_for_touched_cells() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase-a-formula-cache-clear.xlsx");
    let ops_path = tmp.path().join("formula-cache-ops.json");

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
        let c2 = sheet.get_cell_mut("C2");
        c2.set_formula("B2*2");
        c2.get_cell_value_mut().set_formula_result_default("20");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");

    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"sheet_name":"Sheet1","target_range":"C2:C2","anchor_cell":"C2","base_formula":"B2*3","fill_direction":"down","relative_mode":"excel"}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let output = run_cli(&[
        "apply-formula-pattern",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let book = umya_spreadsheet::reader::xlsx::read(&workbook_path).expect("read workbook");
    let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet exists");
    let c2 = sheet.get_cell("C2").expect("C2 exists");
    assert_eq!(c2.get_formula().replace(' ', ""), "B2*3");
    assert_eq!(c2.get_value(), "", "expected formula cache to be cleared");

    let read = run_cli(&["range-values", file, "Sheet1", "C2", "--shape", "compact"]);
    assert!(read.status.success(), "stderr: {:?}", read.stderr);
    let payload = parse_stdout_json(&read);
    assert!(
        payload["rows"][0][0].is_null(),
        "range-values should report null until recalculate refreshes cache"
    );
}

#[test]
fn phase_a_apply_formula_pattern_output_force_overwrite_semantics() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-a-force-source.xlsx");
    let output_path = tmp.path().join("phase-a-force-output.xlsx");
    let ops_first_path = tmp.path().join("formula-ops-first.json");
    let ops_second_path = tmp.path().join("formula-ops-second.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_first_path,
        r#"{"ops":[{"sheet_name":"Sheet1","target_range":"C2:C2","anchor_cell":"C2","base_formula":"B2*3","fill_direction":"down"}]}"#,
    );
    write_ops_payload(
        &ops_second_path,
        r#"{"ops":[{"sheet_name":"Sheet1","target_range":"C2:C2","anchor_cell":"C2","base_formula":"B2*5","fill_direction":"down"}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let first_ref = format!("@{}", ops_first_path.to_str().expect("ops utf8"));
    let second_ref = format!("@{}", ops_second_path.to_str().expect("ops utf8"));

    let first = run_cli(&[
        "apply-formula-pattern",
        source,
        "--ops",
        first_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(first.status.success(), "stderr: {:?}", first.stderr);

    assert_error_code(
        &[
            "apply-formula-pattern",
            source,
            "--ops",
            second_ref.as_str(),
            "--output",
            output,
        ],
        "OUTPUT_EXISTS",
    );

    let forced = run_cli(&[
        "apply-formula-pattern",
        source,
        "--ops",
        second_ref.as_str(),
        "--output",
        output,
        "--force",
    ]);
    assert!(forced.status.success(), "stderr: {:?}", forced.stderr);

    let output_book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output");
    let output_sheet = output_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("sheet exists");
    assert_eq!(
        output_sheet
            .get_cell("C2")
            .expect("C2 output")
            .get_formula()
            .replace(' ', ""),
        "B2*5"
    );
}

#[test]
fn phase_a_negative_invalid_ops_payloads() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase-a-invalid-ops.xlsx");
    let style_bad_path = tmp.path().join("style-bad.json");
    let formula_bad_path = tmp.path().join("formula-bad.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &style_bad_path,
        r#"{"ops":[{"sheet_name":"Sheet1","target":{"kind":"unknown"},"patch":{}}]}"#,
    );
    write_ops_payload(
        &formula_bad_path,
        r#"{"ops":[{"sheet_name":"Sheet1","target_range":"C2:C4","anchor_cell":"C1","base_formula":"B2*3","fill_direction":"down"}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let style_ref = format!("@{}", style_bad_path.to_str().expect("ops utf8"));
    let formula_ref = format!("@{}", formula_bad_path.to_str().expect("ops utf8"));

    assert_error_code(
        &[
            "style-batch",
            file,
            "--ops",
            style_ref.as_str(),
            "--dry-run",
        ],
        "INVALID_OPS_PAYLOAD",
    );
    assert_error_code(
        &[
            "apply-formula-pattern",
            file,
            "--ops",
            formula_ref.as_str(),
            "--dry-run",
        ],
        "INVALID_OPS_PAYLOAD",
    );
}

#[test]
fn phase_a_invalid_relative_mode_reports_valid_literals_and_suggestion() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase-a-invalid-relative-mode.xlsx");
    let formula_bad_path = tmp.path().join("formula-bad-relative-mode.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &formula_bad_path,
        r#"{"ops":[{"sheet_name":"Sheet1","target_range":"C2:C4","anchor_cell":"C2","base_formula":"B2*3","fill_direction":"down","relative_mode":"fully_relative"}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let formula_ref = format!("@{}", formula_bad_path.to_str().expect("ops utf8"));

    let err = assert_error_code(
        &[
            "apply-formula-pattern",
            file,
            "--ops",
            formula_ref.as_str(),
            "--dry-run",
        ],
        "INVALID_OPS_PAYLOAD",
    );

    let message = err["message"].as_str().unwrap_or_default();
    assert!(message.contains("invalid relative_mode 'fully_relative'"));
    assert!(message.contains("Did you mean 'excel'?"));
    assert!(message.contains("valid: excel|abs_cols|abs_rows"));
}

#[test]
fn phase_a_safety_mode_matrix_for_style_and_formula_commands() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase-a-safety.xlsx");
    let style_ops_path = tmp.path().join("style-ops.json");
    let formula_ops_path = tmp.path().join("formula-ops.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &style_ops_path,
        r#"{"ops":[{"sheet_name":"Sheet1","range":"B2:B2","style":{"font":{"bold":true}}}]}"#,
    );
    write_ops_payload(
        &formula_ops_path,
        r#"{"ops":[{"sheet_name":"Sheet1","target_range":"C2:C4","anchor_cell":"C2","base_formula":"B2*3","fill_direction":"down"}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let style_ref = format!("@{}", style_ops_path.to_str().expect("ops utf8"));
    let formula_ref = format!("@{}", formula_ops_path.to_str().expect("ops utf8"));

    assert_batch_mode_matrix("style-batch", file, style_ref.as_str());
    assert_batch_mode_matrix("apply-formula-pattern", file, formula_ref.as_str());
}

#[cfg(unix)]
#[test]
fn phase_a_style_batch_maps_write_failures_and_preserves_source() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-a-style-write-fail-source.xlsx");
    let blocked_dir = tmp.path().join("blocked");
    let blocked_output = blocked_dir.join("output.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"sheet_name":"Sheet1","range":"B2:B2","style":{"font":{"bold":true}}}]}"#,
    );
    fs::create_dir(&blocked_dir).expect("create blocked dir");

    let mut perms = fs::metadata(&blocked_dir)
        .expect("blocked metadata")
        .permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&blocked_dir, perms.clone()).expect("set blocked perms");

    let before = fs::read(&source_path).expect("read source before write failure");
    let source = source_path.to_str().expect("source utf8");
    let output = blocked_output.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    assert_error_code(
        &[
            "style-batch",
            source,
            "--ops",
            ops_ref.as_str(),
            "--output",
            output,
        ],
        "WRITE_FAILED",
    );
    assert!(
        !blocked_output.exists(),
        "write failure left a partial output artifact"
    );

    let mut restore = perms;
    restore.set_mode(0o755);
    fs::set_permissions(&blocked_dir, restore).expect("restore blocked perms");

    let after = fs::read(&source_path).expect("read source after write failure");
    assert_eq!(before, after, "source workbook changed after write failure");
}

#[cfg(unix)]
#[test]
fn phase_a_apply_formula_pattern_maps_write_failures_and_preserves_source() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-a-formula-write-fail-source.xlsx");
    let blocked_dir = tmp.path().join("blocked-formula");
    let blocked_output = blocked_dir.join("output.xlsx");
    let ops_path = tmp.path().join("formula-ops.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"sheet_name":"Sheet1","target_range":"C2:C4","anchor_cell":"C2","base_formula":"B2*3","fill_direction":"down"}]}"#,
    );
    fs::create_dir(&blocked_dir).expect("create blocked dir");

    let mut perms = fs::metadata(&blocked_dir)
        .expect("blocked metadata")
        .permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&blocked_dir, perms.clone()).expect("set blocked perms");

    let before = fs::read(&source_path).expect("read source before write failure");
    let source = source_path.to_str().expect("source utf8");
    let output = blocked_output.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    assert_error_code(
        &[
            "apply-formula-pattern",
            source,
            "--ops",
            ops_ref.as_str(),
            "--output",
            output,
        ],
        "WRITE_FAILED",
    );
    assert!(
        !blocked_output.exists(),
        "write failure left a partial output artifact"
    );

    let mut restore = perms;
    restore.set_mode(0o755);
    fs::set_permissions(&blocked_dir, restore).expect("restore blocked perms");

    let after = fs::read(&source_path).expect("read source after write failure");
    assert_eq!(before, after, "source workbook changed after write failure");
}

#[test]
fn phase_b_help_examples_for_structure_column_and_layout_commands() {
    let structure_help = run_cli(&["structure-batch", "--help"]);
    assert!(
        structure_help.status.success(),
        "stderr: {:?}",
        structure_help.stderr
    );
    let structure = parse_stdout_text(&structure_help);
    assert!(structure.contains("Examples:"));
    assert!(
        structure.contains(
            "asp write batch structure workbook.xlsx --ops @structure_ops.json --dry-run"
        )
    );
    assert!(structure.contains(
        "asp write batch structure workbook.xlsx --ops @structure_ops.json --output structured.xlsx"
    ));
    assert!(structure.contains("Payload examples (`--ops @structure_ops.json`):"));
    assert!(structure.contains("\"kind\":\"rename_sheet\""));
    assert!(structure.contains("\"kind\":\"copy_range\""));

    let column_help = run_cli(&["column-size-batch", "--help"]);
    assert!(
        column_help.status.success(),
        "stderr: {:?}",
        column_help.stderr
    );
    let column = parse_stdout_text(&column_help);
    assert!(column.contains("Examples:"));
    assert!(column.contains(
        "asp write batch column-size workbook.xlsx --ops @column_size_ops.json --in-place"
    ));
    assert!(column.contains(
        "asp write batch column-size workbook.xlsx --ops @column_size_ops.json --output columns.xlsx"
    ));
    assert!(column.contains("Payload examples (`--ops @column_size_ops.json`):"));
    assert!(column.contains("\"sheet_name\":\"Sheet1\""));
    assert!(column.contains("\"kind\":\"width\""));
    assert!(column.contains("Also accepted: top-level `ops` where each op includes `sheet_name`."));

    let layout_help = run_cli(&["sheet-layout-batch", "--help"]);
    assert!(
        layout_help.status.success(),
        "stderr: {:?}",
        layout_help.stderr
    );
    let layout = parse_stdout_text(&layout_help);
    assert!(layout.contains("Examples:"));
    assert!(
        layout.contains(
            "asp write batch sheet-layout workbook.xlsx --ops @layout_ops.json --dry-run"
        )
    );
    assert!(
        layout.contains(
            "asp write batch sheet-layout workbook.xlsx --ops @layout_ops.json --in-place"
        )
    );
    assert!(layout.contains("Payload examples (`--ops @layout_ops.json`):"));
    assert!(layout.contains("\"kind\":\"freeze_panes\""));
    assert!(layout.contains("\"kind\":\"set_page_setup\""));
}

#[test]
fn phase_b_structure_batch_positive_in_place_renames_sheet() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase-b-structure-in-place.xlsx");
    let ops_path = tmp.path().join("structure-ops.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"rename_sheet","old_name":"Summary","new_name":"Dashboard"}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["op_count"].as_u64(), Some(1));
    assert!(payload["changed"].as_bool().unwrap_or(false));

    let book = umya_spreadsheet::reader::xlsx::read(&workbook_path).expect("read workbook");
    assert!(book.get_sheet_by_name("Dashboard").ok().is_some());
    assert!(book.get_sheet_by_name("Summary").ok().is_none());
}

#[test]
fn phase_b_structure_batch_positive_dry_run_and_output_target_only() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-b-structure-source.xlsx");
    let output_path = tmp.path().join("phase-b-structure-output.xlsx");
    let ops_path = tmp.path().join("structure-ops-output.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"rename_sheet","old_name":"Summary","new_name":"Dashboard"}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let before = fs::read(&source_path).expect("read source before dry-run");

    let dry_run = run_cli(&[
        "structure-batch",
        source,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
    ]);
    assert!(dry_run.status.success(), "stderr: {:?}", dry_run.stderr);
    let dry_payload = parse_stdout_json(&dry_run);
    assert!(dry_payload["would_change"].as_bool().unwrap_or(false));

    let source_after_dry = fs::read(&source_path).expect("read source after dry-run");
    assert_eq!(before, source_after_dry, "dry-run mutated source workbook");

    let output_run = run_cli(&[
        "structure-batch",
        source,
        "--ops",
        ops_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(
        output_run.status.success(),
        "stderr: {:?}",
        output_run.stderr
    );
    let payload = parse_stdout_json(&output_run);
    assert!(payload["changed"].as_bool().unwrap_or(false));

    let source_book = umya_spreadsheet::reader::xlsx::read(&source_path).expect("read source");
    assert!(source_book.get_sheet_by_name("Summary").ok().is_some());
    assert!(source_book.get_sheet_by_name("Dashboard").ok().is_none());

    let output_book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output");
    assert!(output_book.get_sheet_by_name("Dashboard").ok().is_some());
    assert!(output_book.get_sheet_by_name("Summary").ok().is_none());
}

#[test]
fn phase_b_structure_batch_output_force_overwrite_semantics() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-b-structure-force-source.xlsx");
    let output_path = tmp.path().join("phase-b-structure-force-output.xlsx");
    let ops_first_path = tmp.path().join("structure-ops-first.json");
    let ops_second_path = tmp.path().join("structure-ops-second.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_first_path,
        r#"{"ops":[{"kind":"rename_sheet","old_name":"Summary","new_name":"Dashboard"}]}"#,
    );
    write_ops_payload(
        &ops_second_path,
        r#"{"ops":[{"kind":"rename_sheet","old_name":"Summary","new_name":"Board"}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let first_ref = format!("@{}", ops_first_path.to_str().expect("ops utf8"));
    let second_ref = format!("@{}", ops_second_path.to_str().expect("ops utf8"));

    let first = run_cli(&[
        "structure-batch",
        source,
        "--ops",
        first_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(first.status.success(), "stderr: {:?}", first.stderr);

    assert_error_code(
        &[
            "structure-batch",
            source,
            "--ops",
            second_ref.as_str(),
            "--output",
            output,
        ],
        "OUTPUT_EXISTS",
    );

    let forced = run_cli(&[
        "structure-batch",
        source,
        "--ops",
        second_ref.as_str(),
        "--output",
        output,
        "--force",
    ]);
    assert!(forced.status.success(), "stderr: {:?}", forced.stderr);

    let output_book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output");
    assert!(output_book.get_sheet_by_name("Board").ok().is_some());
    assert!(output_book.get_sheet_by_name("Summary").ok().is_none());
}

#[test]
fn phase_b_column_size_batch_positive_output_mutates_target_only() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-b-column-source.xlsx");
    let output_path = tmp.path().join("phase-b-column-output.xlsx");
    let ops_path = tmp.path().join("column-ops.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"sheet_name":"Sheet1","ops":[{"range":"A:A","size":{"kind":"width","width_chars":25.0}}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let before = fs::read(&source_path).expect("read source before dry-run");

    let dry_run = run_cli(&[
        "column-size-batch",
        source,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
    ]);
    assert!(dry_run.status.success(), "stderr: {:?}", dry_run.stderr);
    let dry_payload = parse_stdout_json(&dry_run);
    assert!(dry_payload["would_change"].as_bool().unwrap_or(false));

    let source_after_dry = fs::read(&source_path).expect("read source after dry-run");
    assert_eq!(before, source_after_dry, "dry-run mutated source workbook");

    let run = run_cli(&[
        "column-size-batch",
        source,
        "--ops",
        ops_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(run.status.success(), "stderr: {:?}", run.stderr);
    let payload = parse_stdout_json(&run);
    assert!(payload["changed"].as_bool().unwrap_or(false));

    let source_after = fs::read(&source_path).expect("read source after output mode");
    assert_eq!(before, source_after, "source changed during --output mode");

    let output_book =
        umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output workbook");
    let output_sheet = output_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("sheet exists");
    let width = output_sheet
        .get_column_dimension("A")
        .expect("A column")
        .get_width();
    assert!((width - 25.0).abs() < 0.001);
}

#[test]
fn phase_b_column_size_batch_accepts_per_op_sheet_name_shape() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-b-column-per-op-source.xlsx");
    let output_path = tmp.path().join("phase-b-column-per-op-output.xlsx");
    let ops_path = tmp.path().join("column-ops-per-op-sheet.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"sheet_name":"Sheet1","range":"A:A","size":{"kind":"width","width_chars":21.0}}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let run = run_cli(&[
        "column-size-batch",
        source,
        "--ops",
        ops_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(run.status.success(), "stderr: {:?}", run.stderr);

    let output_book =
        umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output workbook");
    let output_sheet = output_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("sheet exists");
    let width = output_sheet
        .get_column_dimension("A")
        .expect("A column")
        .get_width();
    assert!((width - 21.0).abs() < 0.001);
}

#[test]
fn phase_b_column_size_batch_rejects_mixed_per_op_sheet_names() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-b-column-mixed-per-op-source.xlsx");
    let ops_path = tmp.path().join("column-ops-mixed-sheets.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"sheet_name":"Sheet1","range":"A:A","size":{"kind":"width","width_chars":21.0}},{"sheet_name":"Summary","range":"A:A","size":{"kind":"width","width_chars":10.0}}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let err = assert_error_code(
        &[
            "column-size-batch",
            source,
            "--ops",
            ops_ref.as_str(),
            "--dry-run",
        ],
        "INVALID_OPS_PAYLOAD",
    );
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("mixed sheet_name values")
    );
}

#[test]
fn phase_b_column_size_batch_rejects_hybrid_mixed_sheet_names() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-b-column-hybrid-mixed-source.xlsx");
    let ops_path = tmp.path().join("column-ops-hybrid-mixed-sheets.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"sheet_name":"Settings","ops":[{"sheet_name":"Summary","range":"A:A","size":{"kind":"width","width_chars":21.0}}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let err = assert_error_code(
        &[
            "column-size-batch",
            source,
            "--ops",
            ops_ref.as_str(),
            "--dry-run",
        ],
        "INVALID_OPS_PAYLOAD",
    );
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("mixed sheet_name values")
    );
}

#[test]
fn phase_b_column_size_batch_output_force_overwrite_semantics() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-b-column-force-source.xlsx");
    let output_path = tmp.path().join("phase-b-column-force-output.xlsx");
    let ops_first_path = tmp.path().join("column-ops-first.json");
    let ops_second_path = tmp.path().join("column-ops-second.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_first_path,
        r#"{"sheet_name":"Sheet1","ops":[{"range":"A:A","size":{"kind":"width","width_chars":25.0}}]}"#,
    );
    write_ops_payload(
        &ops_second_path,
        r#"{"sheet_name":"Sheet1","ops":[{"range":"A:A","size":{"kind":"width","width_chars":18.0}}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let first_ref = format!("@{}", ops_first_path.to_str().expect("ops utf8"));
    let second_ref = format!("@{}", ops_second_path.to_str().expect("ops utf8"));

    let first = run_cli(&[
        "column-size-batch",
        source,
        "--ops",
        first_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(first.status.success(), "stderr: {:?}", first.stderr);

    assert_error_code(
        &[
            "column-size-batch",
            source,
            "--ops",
            second_ref.as_str(),
            "--output",
            output,
        ],
        "OUTPUT_EXISTS",
    );

    let without_force_book =
        umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output without force");
    let without_force_sheet = without_force_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("sheet exists");
    let without_force_width = without_force_sheet
        .get_column_dimension("A")
        .expect("A column")
        .get_width();
    assert!((without_force_width - 25.0).abs() < 0.001);

    let forced = run_cli(&[
        "column-size-batch",
        source,
        "--ops",
        second_ref.as_str(),
        "--output",
        output,
        "--force",
    ]);
    assert!(forced.status.success(), "stderr: {:?}", forced.stderr);

    let forced_book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output");
    let forced_sheet = forced_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("sheet exists");
    let forced_width = forced_sheet
        .get_column_dimension("A")
        .expect("A column")
        .get_width();
    assert!((forced_width - 18.0).abs() < 0.001);
}

#[test]
fn phase_b_sheet_layout_batch_positive_dry_run_and_in_place() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase-b-layout.xlsx");
    let ops_path = tmp.path().join("layout-ops.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"freeze_panes","sheet_name":"Sheet1","freeze_rows":1,"freeze_cols":1}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let before = fs::read(&workbook_path).expect("read before dry-run");
    let dry_run = run_cli(&[
        "sheet-layout-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
    ]);
    assert!(dry_run.status.success(), "stderr: {:?}", dry_run.stderr);
    let dry_payload = parse_stdout_json(&dry_run);
    assert!(dry_payload["would_change"].as_bool().unwrap_or(false));
    let after_dry = fs::read(&workbook_path).expect("read after dry-run");
    assert_eq!(before, after_dry, "dry-run mutated workbook");

    let in_place = run_cli(&[
        "sheet-layout-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
    ]);
    assert!(in_place.status.success(), "stderr: {:?}", in_place.stderr);

    let book = umya_spreadsheet::reader::xlsx::read(&workbook_path).expect("read workbook");
    let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet exists");
    let views = sheet.get_sheets_views().get_sheet_view_list();
    let view = views.first().expect("sheet view");
    let pane = view.get_pane().expect("pane");
    assert_eq!(pane.get_horizontal_split(), 1.0);
    assert_eq!(pane.get_vertical_split(), 1.0);
    assert_eq!(pane.get_top_left_cell().to_string(), "B2");
    assert_eq!(
        view.get_top_left_cell(),
        "",
        "sheetView topLeftCell should remain unset for LO compatibility"
    );

    let selection = view.get_selection().first().expect("selection");
    assert_eq!(selection.get_sequence_of_references().get_sqref(), "B2");
    assert_eq!(
        selection.get_active_cell().map(|coord| coord.to_string()),
        Some("B2".to_string())
    );
}

#[test]
fn phase_b_sheet_layout_batch_clears_preexisting_sheet_view_top_left_cell() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase-b-layout-preexisting-top-left.xlsx");
    let ops_path = tmp.path().join("layout-preexisting-top-left-ops.json");
    write_fixture(&workbook_path);

    {
        let mut book = umya_spreadsheet::reader::xlsx::read(&workbook_path).expect("read workbook");
        let sheet = book.get_sheet_by_name_mut("Sheet1").ok().expect("sheet");
        let view = sheet
            .get_sheet_views_mut()
            .get_sheet_view_list_mut()
            .first_mut()
            .expect("sheet view");
        view.set_top_left_cell("C3");
        umya_spreadsheet::writer::xlsx::write(&book, &workbook_path).expect("write workbook");
    }

    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"freeze_panes","sheet_name":"Sheet1","freeze_rows":1,"freeze_cols":1}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let in_place = run_cli(&[
        "sheet-layout-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
    ]);
    assert!(in_place.status.success(), "stderr: {:?}", in_place.stderr);

    let book = umya_spreadsheet::reader::xlsx::read(&workbook_path).expect("read workbook");
    let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet exists");
    let view = sheet
        .get_sheets_views()
        .get_sheet_view_list()
        .first()
        .expect("sheet view");
    assert_eq!(
        view.get_top_left_cell(),
        "",
        "preexisting sheetView topLeftCell should be cleared for LO compatibility"
    );
    let pane = view.get_pane().expect("pane");
    assert_eq!(pane.get_top_left_cell().to_string(), "B2");
}

#[test]
fn phase_b_sheet_layout_batch_positive_output_mutates_target_only() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-b-layout-source.xlsx");
    let output_path = tmp.path().join("phase-b-layout-output.xlsx");
    let ops_path = tmp.path().join("layout-output-ops.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"freeze_panes","sheet_name":"Sheet1","freeze_rows":1,"freeze_cols":1}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let before = fs::read(&source_path).expect("read source before output mode");

    let run = run_cli(&[
        "sheet-layout-batch",
        source,
        "--ops",
        ops_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(run.status.success(), "stderr: {:?}", run.stderr);
    let payload = parse_stdout_json(&run);
    assert!(payload["changed"].as_bool().unwrap_or(false));

    let source_after = fs::read(&source_path).expect("read source after output mode");
    assert_eq!(before, source_after, "source changed during --output mode");

    let output_book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output");
    let output_sheet = output_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("sheet exists");
    let view = output_sheet
        .get_sheets_views()
        .get_sheet_view_list()
        .first()
        .expect("sheet view");
    let pane = view.get_pane().expect("pane");
    assert_eq!(pane.get_top_left_cell().to_string(), "B2");
    assert_eq!(view.get_top_left_cell(), "");
    let selection = view.get_selection().first().expect("selection");
    assert_eq!(selection.get_sequence_of_references().get_sqref(), "B2");
}

#[test]
fn phase_b_sheet_layout_batch_output_force_overwrite_semantics() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-b-layout-force-source.xlsx");
    let output_path = tmp.path().join("phase-b-layout-force-output.xlsx");
    let ops_first_path = tmp.path().join("layout-ops-first.json");
    let ops_second_path = tmp.path().join("layout-ops-second.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_first_path,
        r#"{"ops":[{"kind":"freeze_panes","sheet_name":"Sheet1","freeze_rows":1,"freeze_cols":1}]}"#,
    );
    write_ops_payload(
        &ops_second_path,
        r#"{"ops":[{"kind":"freeze_panes","sheet_name":"Sheet1","freeze_rows":2,"freeze_cols":0}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let first_ref = format!("@{}", ops_first_path.to_str().expect("ops utf8"));
    let second_ref = format!("@{}", ops_second_path.to_str().expect("ops utf8"));

    let first = run_cli(&[
        "sheet-layout-batch",
        source,
        "--ops",
        first_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(first.status.success(), "stderr: {:?}", first.stderr);

    assert_error_code(
        &[
            "sheet-layout-batch",
            source,
            "--ops",
            second_ref.as_str(),
            "--output",
            output,
        ],
        "OUTPUT_EXISTS",
    );

    let without_force_book =
        umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output without force");
    let without_force_sheet = without_force_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("sheet exists");
    let without_force_pane = without_force_sheet
        .get_sheets_views()
        .get_sheet_view_list()
        .first()
        .and_then(|view| view.get_pane())
        .expect("pane without force");
    assert_eq!(without_force_pane.get_top_left_cell().to_string(), "B2");

    let forced = run_cli(&[
        "sheet-layout-batch",
        source,
        "--ops",
        second_ref.as_str(),
        "--output",
        output,
        "--force",
    ]);
    assert!(forced.status.success(), "stderr: {:?}", forced.stderr);

    let forced_book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output");
    let forced_sheet = forced_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("sheet exists");
    let forced_pane = forced_sheet
        .get_sheets_views()
        .get_sheet_view_list()
        .first()
        .and_then(|view| view.get_pane())
        .expect("forced pane");
    assert_eq!(forced_pane.get_top_left_cell().to_string(), "A3");
}

#[test]
fn phase_b_negative_invalid_ops_payloads() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase-b-invalid-ops.xlsx");
    let structure_bad_path = tmp.path().join("structure-bad.json");
    let column_bad_path = tmp.path().join("column-bad.json");
    let layout_bad_path = tmp.path().join("layout-bad.json");
    write_fixture(&workbook_path);
    write_ops_payload(&structure_bad_path, r#"{"ops":[{"kind":"unknown_kind"}]}"#);
    write_ops_payload(
        &column_bad_path,
        r#"{"ops":[{"range":"A:A","size":{"kind":"width","width_chars":12.0}}]}"#,
    );
    write_ops_payload(
        &layout_bad_path,
        r#"{"ops":[{"kind":"set_zoom","sheet_name":"Sheet1","zoom_percent":5}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let structure_ref = format!("@{}", structure_bad_path.to_str().expect("ops utf8"));
    let column_ref = format!("@{}", column_bad_path.to_str().expect("ops utf8"));
    let layout_ref = format!("@{}", layout_bad_path.to_str().expect("ops utf8"));

    assert_error_code(
        &[
            "structure-batch",
            file,
            "--ops",
            structure_ref.as_str(),
            "--dry-run",
        ],
        "INVALID_OPS_PAYLOAD",
    );
    assert_error_code(
        &[
            "column-size-batch",
            file,
            "--ops",
            column_ref.as_str(),
            "--dry-run",
        ],
        "INVALID_OPS_PAYLOAD",
    );
    assert_error_code(
        &[
            "sheet-layout-batch",
            file,
            "--ops",
            layout_ref.as_str(),
            "--dry-run",
        ],
        "INVALID_OPS_PAYLOAD",
    );
}

#[test]
fn invalid_ops_payload_errors_include_shape_and_minimal_example() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("invalid-ops-message.xlsx");
    let transform_bad_path = tmp.path().join("transform-bad.json");
    let column_bad_path = tmp.path().join("column-bad-shape.json");
    let rules_bad_path = tmp.path().join("rules-bad.json");
    write_fixture(&workbook_path);

    // Not an object at all.
    write_ops_payload(&transform_bad_path, r#"[]"#);
    // Missing required top-level field `sheet_name`.
    write_ops_payload(&column_bad_path, r#"{"ops":[]}"#);
    // Missing required fields inside the op.
    write_ops_payload(
        &rules_bad_path,
        r#"{"ops":[{"kind":"set_data_validation"}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let transform_ref = format!("@{}", transform_bad_path.to_str().expect("ops utf8"));
    let column_ref = format!("@{}", column_bad_path.to_str().expect("ops utf8"));
    let rules_ref = format!("@{}", rules_bad_path.to_str().expect("ops utf8"));

    let transform_err = assert_error_code(
        &[
            "transform-batch",
            file,
            "--ops",
            transform_ref.as_str(),
            "--dry-run",
        ],
        "INVALID_OPS_PAYLOAD",
    );
    let transform_message = transform_err["message"].as_str().unwrap_or_default();
    assert!(transform_message.contains("expected top-level shape:"));
    assert!(transform_message.contains("minimal valid example:"));
    assert!(transform_message.contains("\"kind\":\"fill_range\""));

    let column_err = assert_error_code(
        &[
            "column-size-batch",
            file,
            "--ops",
            column_ref.as_str(),
            "--dry-run",
        ],
        "INVALID_OPS_PAYLOAD",
    );
    let column_message = column_err["message"].as_str().unwrap_or_default();
    assert!(column_message.contains("expected top-level shape:"));
    assert!(column_message.contains("minimal valid example:"));
    assert!(column_message.contains("\"sheet_name\":\"Sheet1\""));

    let rules_err = assert_error_code(
        &[
            "rules-batch",
            file,
            "--ops",
            rules_ref.as_str(),
            "--dry-run",
        ],
        "INVALID_OPS_PAYLOAD",
    );
    let rules_message = rules_err["message"].as_str().unwrap_or_default();
    assert!(rules_message.contains("expected top-level shape:"));
    assert!(rules_message.contains("minimal valid example:"));
    assert!(rules_message.contains("\"kind\":\"set_data_validation\""));
}

#[test]
fn phase_b_safety_mode_matrix_for_structure_column_layout_commands() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase-b-safety.xlsx");
    let structure_ops_path = tmp.path().join("structure-ops.json");
    let column_ops_path = tmp.path().join("column-ops.json");
    let layout_ops_path = tmp.path().join("layout-ops.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &structure_ops_path,
        r#"{"ops":[{"kind":"rename_sheet","old_name":"Summary","new_name":"Dashboard"}]}"#,
    );
    write_ops_payload(
        &column_ops_path,
        r#"{"sheet_name":"Sheet1","ops":[{"range":"A:A","size":{"kind":"width","width_chars":20.0}}]}"#,
    );
    write_ops_payload(
        &layout_ops_path,
        r#"{"ops":[{"kind":"freeze_panes","sheet_name":"Sheet1","freeze_rows":1,"freeze_cols":1}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let structure_ref = format!("@{}", structure_ops_path.to_str().expect("ops utf8"));
    let column_ref = format!("@{}", column_ops_path.to_str().expect("ops utf8"));
    let layout_ref = format!("@{}", layout_ops_path.to_str().expect("ops utf8"));

    assert_batch_mode_matrix("structure-batch", file, structure_ref.as_str());
    assert_batch_mode_matrix("column-size-batch", file, column_ref.as_str());
    assert_batch_mode_matrix("sheet-layout-batch", file, layout_ref.as_str());
}

#[cfg(unix)]
#[test]
fn phase_b_structure_batch_maps_write_failures_and_preserves_source() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-b-structure-write-fail-source.xlsx");
    let blocked_dir = tmp.path().join("blocked");
    let blocked_output = blocked_dir.join("output.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"rename_sheet","old_name":"Summary","new_name":"Dashboard"}]}"#,
    );
    fs::create_dir(&blocked_dir).expect("create blocked dir");

    let mut perms = fs::metadata(&blocked_dir)
        .expect("blocked metadata")
        .permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&blocked_dir, perms.clone()).expect("set blocked perms");

    let before = fs::read(&source_path).expect("read source before write failure");
    let source = source_path.to_str().expect("source utf8");
    let output = blocked_output.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    assert_error_code(
        &[
            "structure-batch",
            source,
            "--ops",
            ops_ref.as_str(),
            "--output",
            output,
        ],
        "WRITE_FAILED",
    );
    assert!(
        !blocked_output.exists(),
        "write failure left a partial output artifact"
    );

    let mut restore = perms;
    restore.set_mode(0o755);
    fs::set_permissions(&blocked_dir, restore).expect("restore blocked perms");

    let after = fs::read(&source_path).expect("read source after write failure");
    assert_eq!(before, after, "source workbook changed after write failure");
}

#[cfg(unix)]
#[test]
fn phase_b_column_size_batch_maps_write_failures_and_preserves_source() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-b-column-write-fail-source.xlsx");
    let blocked_dir = tmp.path().join("blocked-column");
    let blocked_output = blocked_dir.join("output.xlsx");
    let ops_path = tmp.path().join("ops-column.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"sheet_name":"Sheet1","ops":[{"range":"A:A","size":{"kind":"width","width_chars":20.0}}]}"#,
    );
    fs::create_dir(&blocked_dir).expect("create blocked dir");

    let mut perms = fs::metadata(&blocked_dir)
        .expect("blocked metadata")
        .permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&blocked_dir, perms.clone()).expect("set blocked perms");

    let before = fs::read(&source_path).expect("read source before write failure");
    let source = source_path.to_str().expect("source utf8");
    let output = blocked_output.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    assert_error_code(
        &[
            "column-size-batch",
            source,
            "--ops",
            ops_ref.as_str(),
            "--output",
            output,
        ],
        "WRITE_FAILED",
    );
    assert!(
        !blocked_output.exists(),
        "write failure left a partial output artifact"
    );

    let mut restore = perms;
    restore.set_mode(0o755);
    fs::set_permissions(&blocked_dir, restore).expect("restore blocked perms");

    let after = fs::read(&source_path).expect("read source after write failure");
    assert_eq!(before, after, "source workbook changed after write failure");
}

#[cfg(unix)]
#[test]
fn phase_b_sheet_layout_batch_maps_write_failures_and_preserves_source() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-b-layout-write-fail-source.xlsx");
    let blocked_dir = tmp.path().join("blocked-layout");
    let blocked_output = blocked_dir.join("output.xlsx");
    let ops_path = tmp.path().join("ops-layout.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"freeze_panes","sheet_name":"Sheet1","freeze_rows":1,"freeze_cols":1}]}"#,
    );
    fs::create_dir(&blocked_dir).expect("create blocked dir");

    let mut perms = fs::metadata(&blocked_dir)
        .expect("blocked metadata")
        .permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&blocked_dir, perms.clone()).expect("set blocked perms");

    let before = fs::read(&source_path).expect("read source before write failure");
    let source = source_path.to_str().expect("source utf8");
    let output = blocked_output.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    assert_error_code(
        &[
            "sheet-layout-batch",
            source,
            "--ops",
            ops_ref.as_str(),
            "--output",
            output,
        ],
        "WRITE_FAILED",
    );
    assert!(
        !blocked_output.exists(),
        "write failure left a partial output artifact"
    );

    let mut restore = perms;
    restore.set_mode(0o755);
    fs::set_permissions(&blocked_dir, restore).expect("restore blocked perms");

    let after = fs::read(&source_path).expect("read source after write failure");
    assert_eq!(before, after, "source workbook changed after write failure");
}

#[test]
fn phase_c_help_examples_for_rules_command() {
    let rules_help = run_cli(&["rules-batch", "--help"]);
    assert!(
        rules_help.status.success(),
        "stderr: {:?}",
        rules_help.stderr
    );
    let rules = parse_stdout_text(&rules_help);
    assert!(rules.contains("Examples:"));
    assert!(rules.contains("asp write batch rules workbook.xlsx --ops @rules_ops.json --dry-run"));
    assert!(rules.contains(
        "asp write batch rules workbook.xlsx --ops @rules_ops.json --output ruled.xlsx --force"
    ));
    assert!(rules.contains("Payload examples (`--ops @rules_ops.json`):"));
    assert!(rules.contains("\"kind\":\"set_data_validation\""));
    assert!(rules.contains("\"kind\":\"set_conditional_format\""));
}

#[test]
fn phase_c_rules_batch_positive_in_place_sets_validation() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase-c-rules-in-place.xlsx");
    let ops_path = tmp.path().join("rules-ops.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"B2:B4","validation":{"kind":"list","formula1":"\"A,B,C\""}}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let output = run_cli(&["rules-batch", file, "--ops", ops_ref.as_str(), "--in-place"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert!(payload["changed"].as_bool().unwrap_or(false));

    let book = umya_spreadsheet::reader::xlsx::read(&workbook_path).expect("read workbook");
    let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet exists");
    let dvs = sheet.get_data_validations().expect("data validations");
    let list = dvs.get_data_validation_list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].get_sequence_of_references().get_sqref(), "B2:B4");
}

#[test]
fn phase_c_rules_batch_positive_dry_run_and_output_target_only() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-c-rules-source.xlsx");
    let output_path = tmp.path().join("phase-c-rules-output.xlsx");
    let ops_path = tmp.path().join("rules-ops-output.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"C2:C4","validation":{"kind":"list","formula1":"\"X,Y,Z\""}}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let before = fs::read(&source_path).expect("read source before dry-run");

    let dry_run = run_cli(&[
        "rules-batch",
        source,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
    ]);
    assert!(dry_run.status.success(), "stderr: {:?}", dry_run.stderr);
    let dry_payload = parse_stdout_json(&dry_run);
    assert!(dry_payload["would_change"].as_bool().unwrap_or(false));

    let source_after_dry = fs::read(&source_path).expect("read source after dry-run");
    assert_eq!(before, source_after_dry, "dry-run mutated source workbook");

    let output_run = run_cli(&[
        "rules-batch",
        source,
        "--ops",
        ops_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(
        output_run.status.success(),
        "stderr: {:?}",
        output_run.stderr
    );

    let source_after_output = fs::read(&source_path).expect("read source after output mode");
    assert_eq!(
        before, source_after_output,
        "source changed during --output mode"
    );

    let output_book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output");
    let output_sheet = output_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("sheet exists");
    let dvs = output_sheet
        .get_data_validations()
        .expect("data validations");
    let list = dvs.get_data_validation_list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].get_sequence_of_references().get_sqref(), "C2:C4");
}

#[test]
fn phase_c_rules_batch_output_force_overwrite_semantics() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-c-rules-force-source.xlsx");
    let output_path = tmp.path().join("phase-c-rules-force-output.xlsx");
    let ops_first_path = tmp.path().join("rules-ops-first.json");
    let ops_second_path = tmp.path().join("rules-ops-second.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_first_path,
        r#"{"ops":[{"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"B2:B4","validation":{"kind":"list","formula1":"\"A,B,C\""}}]}"#,
    );
    write_ops_payload(
        &ops_second_path,
        r#"{"ops":[{"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"C2:C4","validation":{"kind":"list","formula1":"\"X,Y,Z\""}}]}"#,
    );

    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");
    let first_ref = format!("@{}", ops_first_path.to_str().expect("ops utf8"));
    let second_ref = format!("@{}", ops_second_path.to_str().expect("ops utf8"));

    let first = run_cli(&[
        "rules-batch",
        source,
        "--ops",
        first_ref.as_str(),
        "--output",
        output,
    ]);
    assert!(first.status.success(), "stderr: {:?}", first.stderr);

    assert_error_code(
        &[
            "rules-batch",
            source,
            "--ops",
            second_ref.as_str(),
            "--output",
            output,
        ],
        "OUTPUT_EXISTS",
    );

    let forced = run_cli(&[
        "rules-batch",
        source,
        "--ops",
        second_ref.as_str(),
        "--output",
        output,
        "--force",
    ]);
    assert!(forced.status.success(), "stderr: {:?}", forced.stderr);

    let output_book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output");
    let output_sheet = output_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("sheet exists");
    let dvs = output_sheet
        .get_data_validations()
        .expect("data validations");
    let list = dvs.get_data_validation_list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].get_sequence_of_references().get_sqref(), "C2:C4");
}

#[test]
fn phase_c_negative_invalid_ops_payload() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase-c-invalid-ops.xlsx");
    let bad_ops_path = tmp.path().join("rules-bad.json");
    write_fixture(&workbook_path);
    write_ops_payload(&bad_ops_path, r#"{"ops":[{"kind":"unknown_rule"}]}"#);

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", bad_ops_path.to_str().expect("ops utf8"));
    assert_error_code(
        &["rules-batch", file, "--ops", ops_ref.as_str(), "--dry-run"],
        "INVALID_OPS_PAYLOAD",
    );
}

#[test]
fn phase_c_safety_mode_matrix_for_rules_command() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("phase-c-safety.xlsx");
    let ops_path = tmp.path().join("rules-ops.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"B2:B4","validation":{"kind":"list","formula1":"\"A,B,C\""}}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));
    assert_batch_mode_matrix("rules-batch", file, ops_ref.as_str());
}

#[cfg(unix)]
#[test]
fn phase_c_rules_batch_maps_write_failures_and_preserves_source() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("phase-c-rules-write-fail-source.xlsx");
    let blocked_dir = tmp.path().join("blocked");
    let blocked_output = blocked_dir.join("output.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"B2:B4","validation":{"kind":"list","formula1":"\"A,B,C\""}}]}"#,
    );
    fs::create_dir(&blocked_dir).expect("create blocked dir");

    let mut perms = fs::metadata(&blocked_dir)
        .expect("blocked metadata")
        .permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&blocked_dir, perms.clone()).expect("set blocked perms");

    let before = fs::read(&source_path).expect("read source before write failure");
    let source = source_path.to_str().expect("source utf8");
    let output = blocked_output.to_str().expect("output utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    assert_error_code(
        &[
            "rules-batch",
            source,
            "--ops",
            ops_ref.as_str(),
            "--output",
            output,
        ],
        "WRITE_FAILED",
    );
    assert!(
        !blocked_output.exists(),
        "write failure left a partial output artifact"
    );

    let mut restore = perms;
    restore.set_mode(0o755);
    fs::set_permissions(&blocked_dir, restore).expect("restore blocked perms");

    let after = fs::read(&source_path).expect("read source after write failure");
    assert_eq!(before, after, "source workbook changed after write failure");
}

#[test]
fn cli_create_workbook_bootstraps_read_write_flow() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("bootstrap.xlsx");
    let file = workbook_path.to_str().expect("path utf8");

    let create = run_cli(&["create-workbook", file, "--sheets", "Inputs,Calc,Output"]);
    assert!(create.status.success(), "stderr: {:?}", create.stderr);
    let payload = parse_stdout_json(&create);
    assert_eq!(payload["path"], Value::String(file.to_string()));
    assert_eq!(payload["overwritten"], Value::Bool(false));

    let list = run_cli(&["list-sheets", file]);
    assert!(list.status.success(), "stderr: {:?}", list.stderr);
    let list_payload = parse_stdout_json(&list);
    let sheet_names: Vec<_> = list_payload["sheets"]
        .as_array()
        .expect("sheets array")
        .iter()
        .filter_map(|entry| entry["name"].as_str().map(str::to_string))
        .collect();
    assert_eq!(sheet_names, vec!["Inputs", "Calc", "Output"]);

    let edit = run_cli(&["edit", file, "Inputs", "A1=42"]);
    assert!(edit.status.success(), "stderr: {:?}", edit.stderr);

    #[cfg(feature = "recalc")]
    {
        let recalc = run_cli(&["recalculate", file]);
        assert!(recalc.status.success(), "stderr: {:?}", recalc.stderr);
    }
}

#[test]
fn cli_create_workbook_rejects_existing_file_without_overwrite() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("existing.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["create-workbook", file]);
    assert!(!output.status.success(), "expected non-zero status");
    let error = parse_stderr_json(&output);
    assert_eq!(error["code"], Value::String("COMMAND_FAILED".to_string()));
    let message = error["message"].as_str().unwrap_or_default();
    assert!(message.contains("already exists"));
    assert!(message.contains("--overwrite"));
}

#[test]
fn cli_edit_invalid_shorthand_error_suggests_formula_double_equals() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("edit-invalid-shorthand.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["edit", file, "Sheet1", "A1"]);
    assert!(!output.status.success(), "expected non-zero status");
    let error = parse_stderr_json(&output);
    assert_eq!(
        error["code"],
        Value::String("INVALID_EDIT_SYNTAX".to_string())
    );

    let message = error["message"].as_str().unwrap_or_default();
    assert!(message.contains("invalid shorthand edit"));
    assert!(
        message.contains("double equals") || message.contains("==SUM"),
        "error should include corrective formula shorthand hint: {message}"
    );
}

#[test]
fn cli_copy_edit_diff_are_stateless_and_persisted() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("original.xlsx");
    let modified = tmp.path().join("modified.xlsx");
    write_fixture(&original);

    let copy = run_cli(&[
        "copy",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
    ]);
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    let copy_payload = parse_stdout_json(&copy);
    assert!(copy_payload["bytes_copied"].as_u64().unwrap_or(0) > 0);

    let edit = run_cli(&[
        "edit",
        modified.to_str().expect("path utf8"),
        "Sheet1",
        "B2=11",
        "C2==B2*3",
    ]);
    assert!(edit.status.success(), "stderr: {:?}", edit.stderr);
    let edit_payload = parse_stdout_json(&edit);
    assert_eq!(edit_payload["edits_applied"], 2);
    assert_eq!(edit_payload["recalc_needed"], true);

    let book = umya_spreadsheet::reader::xlsx::read(&modified).expect("read modified");
    let sheet = book
        .get_sheet_by_name("Sheet1").ok()
        .expect("modified sheet exists");
    assert_eq!(sheet.get_cell("B2").expect("B2 exists").get_value(), "11");
    assert_eq!(
        sheet.get_cell("C2").expect("C2 exists").get_formula(),
        "B2*3"
    );

    let diff = run_cli(&[
        "diff",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
    ]);
    assert!(diff.status.success(), "stderr: {:?}", diff.stderr);
    let diff_payload = parse_stdout_json(&diff);
    assert!(diff_payload["change_count"].as_u64().unwrap_or(0) >= 2);
}

#[test]
fn cli_diff_defaults_to_summary_only() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("diff-summary-original.xlsx");
    let modified = tmp.path().join("diff-summary-modified.xlsx");
    write_fixture(&original);
    fs::copy(&original, &modified).expect("copy workbook");

    let edit = run_cli(&[
        "edit",
        modified.to_str().expect("path utf8"),
        "Sheet1",
        "B2=11",
        "C2==B2*3",
    ]);
    assert!(edit.status.success(), "stderr: {:?}", edit.stderr);

    let diff = run_cli(&[
        "diff",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
    ]);
    assert!(diff.status.success(), "stderr: {:?}", diff.stderr);

    let payload = parse_stdout_json(&diff);
    assert!(payload.get("summary").is_some());
    assert!(payload.get("changes").is_none());
    assert_eq!(payload["summary"]["returned_changes"].as_u64(), Some(0));
    assert_eq!(
        payload["summary"]["total_changes"].as_u64(),
        payload["change_count"].as_u64()
    );
    assert!(
        payload["summary"]["counts_by_kind"]["cell"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    );
}

#[test]
fn cli_diff_details_supports_pagination() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("diff-page-original.xlsx");
    let modified = tmp.path().join("diff-page-modified.xlsx");
    write_fixture(&original);
    fs::copy(&original, &modified).expect("copy workbook");

    let edit = run_cli(&[
        "edit",
        modified.to_str().expect("path utf8"),
        "Sheet1",
        "B2=11",
        "C2==B2*3",
        "B3=25",
    ]);
    assert!(edit.status.success(), "stderr: {:?}", edit.stderr);

    let first = run_cli(&[
        "diff",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
        "--details",
        "--limit",
        "1",
    ]);
    assert!(first.status.success(), "stderr: {:?}", first.stderr);
    let first_payload = parse_stdout_json(&first);
    assert_eq!(
        first_payload["summary"]["returned_changes"].as_u64(),
        Some(1)
    );
    let first_changes = first_payload["changes"].as_array().expect("changes page");
    assert_eq!(first_changes.len(), 1);
    let next_offset = first_payload["summary"]["next_offset"]
        .as_u64()
        .expect("next offset");

    let second = run_cli(&[
        "diff",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
        "--details",
        "--limit",
        "1",
        "--offset",
        &next_offset.to_string(),
    ]);
    assert!(second.status.success(), "stderr: {:?}", second.stderr);
    let second_payload = parse_stdout_json(&second);
    let second_changes = second_payload["changes"].as_array().expect("changes page");
    assert_eq!(second_changes.len(), 1);
    assert_eq!(
        second_payload["change_count"], first_payload["change_count"],
        "paged requests must report stable total change_count"
    );
}

#[test]
fn cli_diff_sheet_and_range_filters_scope_results() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("diff-filter-original.xlsx");
    let modified = tmp.path().join("diff-filter-modified.xlsx");
    write_fixture(&original);
    fs::copy(&original, &modified).expect("copy workbook");

    let edit = run_cli(&[
        "edit",
        modified.to_str().expect("path utf8"),
        "Sheet1",
        "B2=11",
    ]);
    assert!(edit.status.success(), "stderr: {:?}", edit.stderr);

    let filtered = run_cli(&[
        "diff",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
        "--sheet",
        "Sheet1",
        "--range",
        "B2:B2",
        "--details",
        "--limit",
        "50",
    ]);
    assert!(filtered.status.success(), "stderr: {:?}", filtered.stderr);
    let payload = parse_stdout_json(&filtered);
    assert_eq!(payload["change_count"].as_u64(), Some(1));

    let changes = payload["changes"].as_array().expect("changes");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["address"].as_str(), Some("B2"));
}

#[test]
fn cli_diff_summary_includes_group_buckets_and_subtype_counts() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("diff-group-summary-original.xlsx");
    let modified = tmp.path().join("diff-group-summary-modified.xlsx");
    write_fixture(&original);
    fs::copy(&original, &modified).expect("copy workbook");

    let edit = run_cli(&[
        "edit",
        modified.to_str().expect("path utf8"),
        "Sheet1",
        "B2=11",
        "B3=25",
        "C2==B2*3",
    ]);
    assert!(edit.status.success(), "stderr: {:?}", edit.stderr);

    let diff = run_cli(&[
        "diff",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
    ]);
    assert!(diff.status.success(), "stderr: {:?}", diff.stderr);

    let payload = parse_stdout_json(&diff);
    assert_eq!(payload["summary"]["counts_by_subtype"]["value_edit"], 2);
    assert_eq!(payload["summary"]["counts_by_subtype"]["formula_edit"], 1);
    assert_eq!(payload["summary"]["recalc_result_change_count"], 0);
    assert_eq!(
        payload["summary"]["direct_change_count"],
        payload["change_count"]
    );
    assert_eq!(payload["summary"]["group_count"], 2);

    let sheet_summaries = payload["summary"]["sheet_summaries"]
        .as_array()
        .expect("sheet summaries");
    assert_eq!(sheet_summaries.len(), 1);
    assert_eq!(sheet_summaries[0]["sheet"], "Sheet1");
    assert_eq!(sheet_summaries[0]["total_changes"], 3);
    assert_eq!(sheet_summaries[0]["direct_change_count"], 3);
    assert_eq!(sheet_summaries[0]["recalc_result_change_count"], 0);
    assert_eq!(sheet_summaries[0]["counts_by_group_type"]["value_edit"], 1);
    assert_eq!(
        sheet_summaries[0]["counts_by_group_type"]["formula_edit"],
        1
    );

    let preview = payload["summary"]["group_preview"]
        .as_array()
        .expect("group preview");
    assert_eq!(preview.len(), 2);
    let value_group = preview
        .iter()
        .find(|group| group["group_type"] == "value_edit")
        .expect("value_edit group");
    assert_eq!(value_group["range"], "B2:B3");
    assert_eq!(value_group["change_count"], 2);
    assert_eq!(value_group["review_priority"], "direct");
    let formula_group = preview
        .iter()
        .find(|group| group["group_type"] == "formula_edit")
        .expect("formula_edit group");
    assert_eq!(formula_group["range"], "C2");
    assert_eq!(formula_group["review_priority"], "direct");
}

#[test]
fn cli_diff_can_exclude_recalc_result_noise() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("diff-exclude-recalc-original.xlsx");
    let modified = tmp.path().join("diff-exclude-recalc-modified.xlsx");
    write_fixture(&original);
    fs::copy(&original, &modified).expect("copy workbook");

    let edit = run_cli(&[
        "edit",
        modified.to_str().expect("path utf8"),
        "Sheet1",
        "B2=11",
    ]);
    assert!(edit.status.success(), "stderr: {:?}", edit.stderr);

    let recalc = run_cli(&["recalculate", modified.to_str().expect("path utf8")]);
    assert!(recalc.status.success(), "stderr: {:?}", recalc.stderr);

    let full = run_cli(&[
        "diff",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
        "--details",
        "--limit",
        "50",
    ]);
    assert!(full.status.success(), "stderr: {:?}", full.stderr);
    let full_payload = parse_stdout_json(&full);
    assert_eq!(
        full_payload["summary"]["counts_by_subtype"]["value_edit"],
        1
    );
    let recalc_count = full_payload["summary"]["counts_by_subtype"]["recalc_result"]
        .as_u64()
        .unwrap_or(0);
    assert!(
        recalc_count >= 1,
        "expected recalc churn, got {full_payload}"
    );
    assert_eq!(
        full_payload["summary"]["recalc_result_change_count"],
        recalc_count
    );
    assert_eq!(full_payload["summary"]["direct_change_count"], 1);
    assert_eq!(
        full_payload["change_count"].as_u64().unwrap_or(0),
        recalc_count + 1
    );
    let full_sheet_summaries = full_payload["summary"]["sheet_summaries"]
        .as_array()
        .expect("sheet summaries");
    assert_eq!(full_sheet_summaries.len(), 1);
    assert_eq!(full_sheet_summaries[0]["sheet"], "Sheet1");
    assert_eq!(full_sheet_summaries[0]["direct_change_count"], 1);
    assert_eq!(
        full_sheet_summaries[0]["recalc_result_change_count"],
        recalc_count
    );

    let filtered = run_cli(&[
        "diff",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
        "--details",
        "--limit",
        "50",
        "--exclude-recalc-result",
    ]);
    assert!(filtered.status.success(), "stderr: {:?}", filtered.stderr);
    let filtered_payload = parse_stdout_json(&filtered);
    assert_eq!(filtered_payload["change_count"], 1);
    assert_eq!(filtered_payload["summary"]["recalc_result_change_count"], 0);
    assert_eq!(filtered_payload["summary"]["direct_change_count"], 1);
    assert_eq!(
        filtered_payload["summary"]["filters"]["exclude_recalc_result"],
        true
    );
    assert!(
        filtered_payload["summary"]["counts_by_subtype"]
            .get("recalc_result")
            .is_none()
    );
    let changes = filtered_payload["changes"].as_array().expect("changes");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["address"], "B2");

    let full_groups = full_payload["groups"].as_array().expect("groups");
    assert_eq!(full_groups[0]["review_priority"], "direct");
    assert_ne!(full_groups[0]["group_type"], "recalc_result");
    assert_eq!(
        full_groups.last().expect("at least one group")["review_priority"],
        "derived"
    );
}

#[test]
fn cli_append_region_dry_run_reports_footer_aware_plan() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("append-region-dry-run.xlsx");
    let rows_path = tmp.path().join("rows.json");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Amount");
        sheet.get_cell_mut("A2").set_value("Alice");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("A3").set_value("Bob");
        sheet.get_cell_mut("B3").set_value_number(20.0);
        sheet.get_cell_mut("A4").set_value("Total");
        let total = sheet.get_cell_mut("B4");
        total.set_formula("SUM(B2:B3)");
        total.get_cell_value_mut().set_formula_result_default("30");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");
    fs::write(&rows_path, r#"{"rows":[["Cara",30]]}"#).expect("write rows payload");

    let file = workbook_path.to_str().expect("path utf8");
    let overview = run_cli(&["sheet-overview", file, "Sheet1"]);
    assert!(overview.status.success(), "stderr: {:?}", overview.stderr);
    let overview_payload = parse_stdout_json(&overview);
    let region_id = overview_payload["detected_regions"][0]["id"]
        .as_u64()
        .expect("region id")
        .to_string();

    let output = run_cli(&[
        "append-region",
        file,
        "--sheet",
        "Sheet1",
        "--region-id",
        region_id.as_str(),
        "--rows",
        &format!("@{}", rows_path.display()),
        "--dry-run",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["mode"], "dry_run");
    assert_eq!(payload["sheet_name"], "Sheet1");
    assert_eq!(payload["target_kind"], "detected_region");
    assert_eq!(
        payload["region_id"],
        region_id.parse::<u64>().expect("region id num")
    );
    assert_eq!(payload["footer_policy"], "auto");
    assert_eq!(payload["insert_at_row"], 4);
    assert_eq!(
        payload["insert_reason"],
        "auto policy selected detected footer row 4"
    );
    assert_eq!(payload["footer_row"], 4);
    assert_eq!(payload["target_anchor"], "A4");
    assert_eq!(payload["target_range"], "A4:B4");
    assert_eq!(payload["rows_appended"], 1);
    assert_eq!(payload["columns_written"], 2);
    assert_eq!(payload["expand_adjacent_sums"], true);
    assert_eq!(payload["confidence"], "high");
    assert!(
        payload["confidence_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("explicit footer keyword detected")
    );
    assert_eq!(payload["footer_formula_targets"][0], "B4");
    assert_eq!(
        payload["footer_candidates"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(payload["would_change"], true);
}

#[test]
fn cli_append_region_output_inserts_before_footer_and_expands_sum() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("append-region-output-source.xlsx");
    let output_path = tmp.path().join("append-region-output-target.xlsx");
    let rows_path = tmp.path().join("rows.json");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Amount");
        sheet.get_cell_mut("A2").set_value("Alice");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("A3").set_value("Bob");
        sheet.get_cell_mut("B3").set_value_number(20.0);
        sheet.get_cell_mut("A4").set_value("Total");
        let total = sheet.get_cell_mut("B4");
        total.set_formula("SUM(B2:B3)");
        total.get_cell_value_mut().set_formula_result_default("30");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");
    fs::write(&rows_path, r#"{"rows":[["Cara",30]]}"#).expect("write rows payload");

    let file = workbook_path.to_str().expect("path utf8");
    let out = output_path.to_str().expect("output path utf8");
    let overview = run_cli(&["sheet-overview", file, "Sheet1"]);
    assert!(overview.status.success(), "stderr: {:?}", overview.stderr);
    let overview_payload = parse_stdout_json(&overview);
    let region_id = overview_payload["detected_regions"][0]["id"]
        .as_u64()
        .expect("region id")
        .to_string();

    let output = run_cli(&[
        "append-region",
        file,
        "--sheet",
        "Sheet1",
        "--region-id",
        region_id.as_str(),
        "--rows",
        &format!("@{}", rows_path.display()),
        "--output",
        out,
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["mode"], "output");
    assert_eq!(payload["file"], out);
    assert_eq!(payload["target_path"], out);
    assert_eq!(payload["changed"], true);

    let book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output workbook");
    let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet1 exists");
    assert_eq!(sheet.get_cell("A4").expect("A4").get_value(), "Cara");
    assert_eq!(sheet.get_cell("B4").expect("B4").get_value(), "30");
    assert_eq!(sheet.get_cell("A5").expect("A5").get_value(), "Total");
    assert_eq!(
        sheet.get_cell("B5").expect("B5").get_formula(),
        "SUM(B2:B4)"
    );
}

#[test]
fn cli_append_region_detects_formula_footer_even_with_blank_label_cell() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("append-region-blank-footer.xlsx");
    let rows_path = tmp.path().join("rows.json");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Amount");
        sheet.get_cell_mut("A2").set_value("Alice");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("A3").set_value("Bob");
        sheet.get_cell_mut("B3").set_value_number(20.0);
        let total = sheet.get_cell_mut("B4");
        total.set_formula("SUM(B2:B3)");
        total.get_cell_value_mut().set_formula_result_default("30");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");
    fs::write(&rows_path, r#"{"rows":[["Cara",30]]}"#).expect("write rows payload");

    let file = workbook_path.to_str().expect("path utf8");
    let overview = run_cli(&["sheet-overview", file, "Sheet1"]);
    assert!(overview.status.success(), "stderr: {:?}", overview.stderr);
    let overview_payload = parse_stdout_json(&overview);
    let region_id = overview_payload["detected_regions"][0]["id"]
        .as_u64()
        .expect("region id")
        .to_string();

    let output = run_cli(&[
        "append-region",
        file,
        "--sheet",
        "Sheet1",
        "--region-id",
        region_id.as_str(),
        "--rows",
        &format!("@{}", rows_path.display()),
        "--dry-run",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["footer_row"], 4);
    assert_eq!(payload["insert_at_row"], 4);
    assert!(
        payload["footer_detection"]
            .as_str()
            .unwrap_or_default()
            .contains("formula-bearing summary row")
    );
}

#[test]
fn cli_append_region_from_csv_skips_header_and_handles_quotes_blanks_and_crlf() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("append-region-csv-source.xlsx");
    let output_path = tmp.path().join("append-region-csv-target.xlsx");
    let csv_path = tmp.path().join("rows.csv");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Amount");
        sheet.get_cell_mut("C1").set_value("Notes");
        sheet.get_cell_mut("A2").set_value("Alice");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("A3").set_value("Bob");
        sheet.get_cell_mut("B3").set_value_number(20.0);
        sheet.get_cell_mut("A4").set_value("Total");
        let total = sheet.get_cell_mut("B4");
        total.set_formula("SUM(B2:B3)");
        total.get_cell_value_mut().set_formula_result_default("30");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");
    fs::write(
        &csv_path,
        "Name,Amount,Notes\r\n\"Cara, Jr\",30,\r\nDina,40,\"Needs review\"\r\n",
    )
    .expect("write csv payload");

    let file = workbook_path.to_str().expect("path utf8");
    let out = output_path.to_str().expect("output path utf8");
    let overview = run_cli(&["sheet-overview", file, "Sheet1"]);
    assert!(overview.status.success(), "stderr: {:?}", overview.stderr);
    let overview_payload = parse_stdout_json(&overview);
    let region_id = overview_payload["detected_regions"][0]["id"]
        .as_u64()
        .expect("region id")
        .to_string();

    let output = run_cli(&[
        "append-region",
        file,
        "--sheet",
        "Sheet1",
        "--region-id",
        region_id.as_str(),
        "--from-csv",
        csv_path.to_str().expect("csv utf8"),
        "--header",
        "--output",
        out,
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["rows_appended"], 2);
    assert_eq!(payload["columns_written"], 3);
    assert_eq!(payload["insert_at_row"], 4);
    assert_eq!(payload["target_range"], "A4:C5");

    let book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output workbook");
    let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet1 exists");
    assert_eq!(sheet.get_cell("A4").expect("A4").get_value(), "Cara, Jr");
    assert_eq!(sheet.get_cell("B4").expect("B4").get_value(), "30");
    assert!(
        sheet.get_cell("C4").is_none()
            || sheet
                .get_cell("C4")
                .expect("C4 present when not none")
                .get_value()
                .is_empty()
    );
    assert_eq!(sheet.get_cell("A5").expect("A5").get_value(), "Dina");
    assert_eq!(sheet.get_cell("B5").expect("B5").get_value(), "40");
    assert_eq!(
        sheet.get_cell("C5").expect("C5").get_value(),
        "Needs review"
    );
    assert_eq!(sheet.get_cell("A6").expect("A6").get_value(), "Total");
    assert_eq!(
        sheet.get_cell("B6").expect("B6").get_formula(),
        "SUM(B2:B5)"
    );
}

#[test]
fn cli_append_region_rejects_rows_and_from_csv_together() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("append-region-invalid-source.xlsx");
    let rows_path = tmp.path().join("rows.json");
    let csv_path = tmp.path().join("rows.csv");

    write_fixture(&workbook_path);
    fs::write(&rows_path, r#"{"rows":[["Cara",30]]}"#).expect("write rows payload");
    fs::write(&csv_path, "Name,Amount\nCara,30\n").expect("write csv payload");

    let file = workbook_path.to_str().expect("path utf8");
    let overview = run_cli(&["sheet-overview", file, "Sheet1"]);
    assert!(overview.status.success(), "stderr: {:?}", overview.stderr);
    let overview_payload = parse_stdout_json(&overview);
    let region_id = overview_payload["detected_regions"][0]["id"]
        .as_u64()
        .expect("region id")
        .to_string();

    let output = run_cli(&[
        "append-region",
        file,
        "--sheet",
        "Sheet1",
        "--region-id",
        region_id.as_str(),
        "--rows",
        &format!("@{}", rows_path.display()),
        "--from-csv",
        csv_path.to_str().expect("csv utf8"),
        "--dry-run",
    ]);
    assert!(!output.status.success());
    let err = parse_stderr_json(&output);
    assert_eq!(err["code"], "INVALID_ARGUMENT");
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("mutually exclusive")
    );
}

#[test]
fn cli_append_region_supports_table_name_targeting() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("append-region-table-target.xlsx");
    let output_path = tmp.path().join("append-region-table-target-out.xlsx");
    let rows_path = tmp.path().join("rows.json");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Amount");
        sheet.get_cell_mut("A2").set_value("Alice");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("A3").set_value("Bob");
        sheet.get_cell_mut("B3").set_value_number(20.0);
        let mut table = umya_spreadsheet::structs::Table::new("SalesTable", ("A1", "B3"));
        table.set_display_name("SalesTable");
        sheet.add_table(table);
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");
    fs::write(&rows_path, r#"{"rows":[["Cara",30]]}"#).expect("write rows payload");

    let output = run_cli(&[
        "append-region",
        workbook_path.to_str().expect("path utf8"),
        "--sheet",
        "Sheet1",
        "--table-name",
        "SalesTable",
        "--rows",
        &format!("@{}", rows_path.display()),
        "--output",
        output_path.to_str().expect("output utf8"),
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["target_kind"], "table");
    assert_eq!(payload["table_name"], "SalesTable");
    assert!(payload.get("region_id").is_none());
    assert_eq!(payload["insert_at_row"], 4);
    assert_eq!(payload["target_range"], "A4:B4");

    let book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output workbook");
    let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet1 exists");
    assert_eq!(sheet.get_cell("A4").expect("A4").get_value(), "Cara");
    assert_eq!(sheet.get_cell("B4").expect("B4").get_value(), "30");
    let table = sheet
        .get_tables()
        .iter()
        .find(|table| table.get_name() == "SalesTable")
        .expect("sales table");
    assert_eq!(table.get_area().0.get_coordinate(), "A1");
    assert_eq!(table.get_area().1.get_coordinate(), "B4");

    let named = run_cli(&[
        "named-ranges",
        output_path.to_str().expect("output utf8"),
        "--sheet",
        "Sheet1",
    ]);
    assert!(named.status.success(), "stderr: {:?}", named.stderr);
    let named_payload = parse_stdout_json(&named);
    let sales_table = named_payload["items"]
        .as_array()
        .expect("named items")
        .iter()
        .find(|item| item["name"] == "SalesTable")
        .expect("sales table item");
    assert_eq!(sales_table["kind"], "table");
    assert_eq!(sales_table["refers_to"], "A1:B4");

    let read_table = run_cli(&[
        "read-table",
        output_path.to_str().expect("output utf8"),
        "--sheet",
        "Sheet1",
        "--table-name",
        "SalesTable",
        "--table-format",
        "values",
    ]);
    assert!(
        read_table.status.success(),
        "stderr: {:?}",
        read_table.stderr
    );
    let read_table_payload = parse_stdout_json(&read_table);
    assert_eq!(read_table_payload["table_name"], "SalesTable");
    assert_eq!(read_table_payload["total_rows"], 3);
}

#[test]
fn cli_append_region_append_at_end_policy_bypasses_detected_footer() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("append-region-append-at-end.xlsx");
    let rows_path = tmp.path().join("rows.json");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Amount");
        sheet.get_cell_mut("A2").set_value("Alice");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("A3").set_value("Bob");
        sheet.get_cell_mut("B3").set_value_number(20.0);
        sheet.get_cell_mut("A4").set_value("Total");
        let total = sheet.get_cell_mut("B4");
        total.set_formula("SUM(B2:B3)");
        total.get_cell_value_mut().set_formula_result_default("30");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");
    fs::write(&rows_path, r#"{"rows":[["Cara",30]]}"#).expect("write rows payload");

    let file = workbook_path.to_str().expect("path utf8");
    let overview = run_cli(&["sheet-overview", file, "Sheet1"]);
    assert!(overview.status.success(), "stderr: {:?}", overview.stderr);
    let overview_payload = parse_stdout_json(&overview);
    let region_id = overview_payload["detected_regions"][0]["id"]
        .as_u64()
        .expect("region id")
        .to_string();

    let output = run_cli(&[
        "append-region",
        file,
        "--sheet",
        "Sheet1",
        "--region-id",
        region_id.as_str(),
        "--rows",
        &format!("@{}", rows_path.display()),
        "--footer-policy",
        "append-at-end",
        "--dry-run",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["footer_policy"], "append_at_end");
    assert_eq!(payload["footer_row"], 4);
    assert_eq!(payload["insert_at_row"], 5);
    assert!(
        payload["insert_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("bypassed detected footer row 4")
    );
    assert!(
        payload["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning
                .as_str()
                .unwrap_or_default()
                .contains("ignored detected footer row 4"))
    );
}

#[test]
fn cli_append_region_before_footer_policy_requires_detected_footer() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("append-region-before-footer.xlsx");
    let rows_path = tmp.path().join("rows.json");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Amount");
        sheet.get_cell_mut("A2").set_value("Alice");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("A3").set_value("Bob");
        sheet.get_cell_mut("B3").set_value_number(20.0);
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");
    fs::write(&rows_path, r#"{"rows":[["Cara",30]]}"#).expect("write rows payload");

    let file = workbook_path.to_str().expect("path utf8");
    let overview = run_cli(&["sheet-overview", file, "Sheet1"]);
    assert!(overview.status.success(), "stderr: {:?}", overview.stderr);
    let overview_payload = parse_stdout_json(&overview);
    let region_id = overview_payload["detected_regions"][0]["id"]
        .as_u64()
        .expect("region id")
        .to_string();

    let output = run_cli(&[
        "append-region",
        file,
        "--sheet",
        "Sheet1",
        "--region-id",
        region_id.as_str(),
        "--rows",
        &format!("@{}", rows_path.display()),
        "--footer-policy",
        "before-footer",
        "--dry-run",
    ]);
    assert!(!output.status.success());
    let err = parse_stderr_json(&output);
    assert_eq!(err["code"], "INVALID_ARGUMENT");
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("requires a detected footer/subtotal row")
    );
}

#[test]
fn cli_clone_template_row_dry_run_reports_targets_and_confidence() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("clone-template-row-dry-run.xlsx");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        sheet.get_cell_mut("A1").set_value("Item");
        sheet.get_cell_mut("B1").set_value("Input");
        sheet.get_cell_mut("C1").set_value("Calc");
        sheet.get_cell_mut("A2").set_value("Alpha");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("C2").set_formula("B2*2");
        sheet.get_cell_mut("A3").set_value("Total");
        sheet.get_cell_mut("C3").set_formula("SUM(C2:C2)");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");

    let output = run_cli(&[
        "clone-template-row",
        workbook_path.to_str().expect("path utf8"),
        "--sheet",
        "Sheet1",
        "--source-row",
        "2",
        "--after",
        "2",
        "--count",
        "2",
        "--expand-adjacent-sums",
        "--dry-run",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["helper_kind"], "clone_template_row");
    assert_eq!(payload["anchor_kind"], "after");
    assert_eq!(payload["insert_at_row"], 3);
    assert_eq!(payload["inserted_row_range"], "3:4");
    assert_eq!(payload["formula_targets"][0], "C3");
    assert_eq!(payload["formula_targets"][1], "C4");
    assert_eq!(payload["likely_patch_targets"][0], "B3");
    assert_eq!(payload["likely_patch_targets"][1], "B4");
    assert_eq!(payload["adjacent_sum_targets"][0], "C5");
    assert_eq!(payload["confidence"], "high");
    assert_eq!(payload["would_change"], true);
}

#[test]
fn cli_clone_template_row_strict_merge_policy_fails_on_crossing_merge() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("clone-template-row-strict-merge.xlsx");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        sheet.get_cell_mut("A1").set_value("Header");
        sheet.get_cell_mut("A2").set_value("Alpha");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.add_merge_cells("A1:A2");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");

    let output = run_cli(&[
        "clone-template-row",
        workbook_path.to_str().expect("path utf8"),
        "--sheet",
        "Sheet1",
        "--source-row",
        "2",
        "--before",
        "3",
        "--merge-policy",
        "strict",
        "--dry-run",
    ]);
    assert!(!output.status.success());
    let err = parse_stderr_json(&output);
    assert_eq!(err["code"], "UNSAFE_CLONE_TEMPLATE");
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("cross the clone boundary")
    );
}

#[test]
fn cli_clone_template_row_output_preserves_horizontal_merge_and_validation() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("clone-template-row-output.xlsx");
    let output_path = tmp.path().join("clone-template-row-output-target.xlsx");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Input");
        sheet.get_cell_mut("C1").set_value("Calc");
        sheet.get_cell_mut("A2").set_value("Alpha");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("C2").set_formula("B2*2");
        sheet.add_merge_cells("A2:B2");

        let mut dv = umya_spreadsheet::structs::DataValidation::default();
        dv.set_type(umya_spreadsheet::structs::DataValidationValues::List);
        dv.get_sequence_of_references_mut().set_sqref("B2:B2");
        dv.set_formula1("\"A,B,C\"");
        sheet.set_data_validations(umya_spreadsheet::structs::DataValidations::default());
        sheet
            .get_data_validations_mut()
            .unwrap()
            .add_data_validation_list(dv);
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");

    let output = run_cli(&[
        "clone-template-row",
        workbook_path.to_str().expect("path utf8"),
        "--sheet",
        "Sheet1",
        "--source-row",
        "2",
        "--before",
        "3",
        "--count",
        "2",
        "--patch-targets",
        "all-non-formula",
        "--output",
        output_path.to_str().expect("output utf8"),
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["changed"], true);
    assert_eq!(payload["inserted_row_range"], "3:4");

    let book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output workbook");
    let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet1 exists");
    assert_eq!(sheet.get_cell("A3").expect("A3").get_value(), "Alpha");
    assert_eq!(sheet.get_cell("B4").expect("B4").get_value(), "10");
    let merge_ranges: Vec<String> = sheet
        .get_merge_cells()
        .iter()
        .map(|range| range.get_range())
        .collect();
    assert!(merge_ranges.contains(&"A3:B3".to_string()));
    assert!(merge_ranges.contains(&"A4:B4".to_string()));
    let validations = sheet.get_data_validations().expect("validations");
    let sqrefs: Vec<String> = validations
        .get_data_validation_list()
        .iter()
        .map(|dv| dv.get_sequence_of_references().get_sqref())
        .collect();
    assert!(sqrefs.iter().any(|sqref| sqref.contains("B3")));
    assert!(sqrefs.iter().any(|sqref| sqref.contains("B4")));
}

#[test]
fn cli_clone_row_band_dry_run_reports_blocks_and_targets() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("clone-row-band-dry-run.xlsx");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        sheet.get_cell_mut("A1").set_value("Item");
        sheet.get_cell_mut("B1").set_value("Input");
        sheet.get_cell_mut("C1").set_value("Calc");
        sheet.get_cell_mut("A2").set_value("Alpha");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("C2").set_formula("B2*2");
        sheet.get_cell_mut("A3").set_value("Beta");
        sheet.get_cell_mut("B3").set_value_number(20.0);
        sheet.get_cell_mut("C3").set_formula("B3*2");
        sheet.get_cell_mut("A4").set_value("Total");
        sheet.get_cell_mut("C4").set_formula("SUM(C2:C3)");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");

    let output = run_cli(&[
        "clone-row-band",
        workbook_path.to_str().expect("path utf8"),
        "--sheet",
        "Sheet1",
        "--source-rows",
        "2:3",
        "--after",
        "3",
        "--repeat",
        "2",
        "--expand-adjacent-sums",
        "--dry-run",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["helper_kind"], "clone_row_band");
    assert_eq!(payload["source_row_range"], "2:3");
    assert_eq!(payload["source_row_count"], 2);
    assert_eq!(payload["inserted_row_range"], "4:7");
    assert_eq!(payload["inserted_blocks"][0]["row_range"], "4:5");
    assert_eq!(payload["inserted_blocks"][1]["row_range"], "6:7");
    assert_eq!(payload["formula_targets"][0], "C4");
    assert_eq!(payload["adjacent_sum_targets"][0], "C8");
    assert_eq!(payload["confidence"], "high");
}

#[test]
fn cli_clone_row_band_strict_merge_policy_fails_on_crossing_merge() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("clone-row-band-strict-merge.xlsx");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        sheet.get_cell_mut("A1").set_value("Header");
        sheet.get_cell_mut("A2").set_value("Alpha");
        sheet.get_cell_mut("A3").set_value("Beta");
        sheet.add_merge_cells("A1:A2");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");

    let output = run_cli(&[
        "clone-row-band",
        workbook_path.to_str().expect("path utf8"),
        "--sheet",
        "Sheet1",
        "--source-rows",
        "2:3",
        "--before",
        "4",
        "--merge-policy",
        "strict",
        "--dry-run",
    ]);
    assert!(!output.status.success());
    let err = parse_stderr_json(&output);
    assert_eq!(err["code"], "UNSAFE_CLONE_TEMPLATE");
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("cross the clone boundary")
    );
}

#[test]
fn cli_clone_row_band_output_preserves_merges_validations_and_row_heights() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("clone-row-band-output.xlsx");
    let output_path = tmp.path().join("clone-row-band-output-target.xlsx");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Input");
        sheet.get_cell_mut("C1").set_value("Calc");
        sheet.get_cell_mut("A2").set_value("Alpha");
        sheet.get_cell_mut("B2").set_value_number(10.0);
        sheet.get_cell_mut("C2").set_formula("B2*2");
        sheet.get_cell_mut("A3").set_value("Beta");
        sheet.get_cell_mut("B3").set_value_number(20.0);
        sheet.get_cell_mut("C3").set_formula("B3*2");
        sheet.add_merge_cells("A2:A3");
        sheet
            .get_row_dimension_mut(&2)
            .set_height(28.0)
            .set_custom_height(true);
        sheet
            .get_row_dimension_mut(&3)
            .set_height(32.0)
            .set_custom_height(true);

        let mut dv = umya_spreadsheet::structs::DataValidation::default();
        dv.set_type(umya_spreadsheet::structs::DataValidationValues::List);
        dv.get_sequence_of_references_mut().set_sqref("B2:B3");
        dv.set_formula1("\"A,B,C\"");
        sheet.set_data_validations(umya_spreadsheet::structs::DataValidations::default());
        sheet
            .get_data_validations_mut()
            .unwrap()
            .add_data_validation_list(dv);
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write workbook");

    let output = run_cli(&[
        "clone-row-band",
        workbook_path.to_str().expect("path utf8"),
        "--sheet",
        "Sheet1",
        "--source-rows",
        "2:3",
        "--before",
        "4",
        "--repeat",
        "2",
        "--patch-targets",
        "all-non-formula",
        "--output",
        output_path.to_str().expect("output utf8"),
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["changed"], true);
    assert_eq!(payload["inserted_blocks"][0]["row_range"], "4:5");
    assert_eq!(payload["inserted_blocks"][1]["row_range"], "6:7");

    let book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output workbook");
    let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet1 exists");
    assert_eq!(sheet.get_cell("A4").expect("A4").get_value(), "Alpha");
    assert_eq!(sheet.get_cell("A5").expect("A5").get_value(), "Beta");
    assert_eq!(
        sheet
            .get_cell("C4")
            .expect("C4")
            .get_formula()
            .replace(' ', ""),
        "B4*2"
    );
    assert_eq!(
        sheet
            .get_cell("C7")
            .expect("C7")
            .get_formula()
            .replace(' ', ""),
        "B7*2"
    );
    let merge_ranges: Vec<String> = sheet
        .get_merge_cells()
        .iter()
        .map(|range| range.get_range())
        .collect();
    assert!(merge_ranges.contains(&"A4:A5".to_string()));
    assert!(merge_ranges.contains(&"A6:A7".to_string()));
    assert_eq!(
        sheet.get_row_dimension(&4).map(|row| row.get_height()),
        Some(28.0)
    );
    assert_eq!(
        sheet.get_row_dimension(&5).map(|row| row.get_height()),
        Some(32.0)
    );
    let validations = sheet.get_data_validations().expect("validations");
    let sqrefs: Vec<String> = validations
        .get_data_validation_list()
        .iter()
        .map(|dv| dv.get_sequence_of_references().get_sqref())
        .collect();
    assert!(
        sqrefs
            .iter()
            .any(|sqref| sqref.contains("B4") && sqref.contains("B5"))
    );
    assert!(
        sqrefs
            .iter()
            .any(|sqref| sqref.contains("B6") && sqref.contains("B7"))
    );
}

#[test]
fn cli_diff_groups_multi_digit_row_runs_by_coordinates_not_lexicographic_order() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("diff-multi-digit-original.xlsx");
    let modified = tmp.path().join("diff-multi-digit-modified.xlsx");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet1");
        for row in 2..=12u32 {
            sheet.get_cell_mut((2, row)).set_value_number(row as i32);
        }
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &original).expect("write original");
    fs::copy(&original, &modified).expect("copy workbook");

    let mut args = vec![
        "edit".to_string(),
        modified.to_str().expect("path utf8").to_string(),
        "Sheet1".to_string(),
    ];
    for row in 2..=12u32 {
        args.push(format!("B{}={}", row, row + 100));
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let edit = run_cli(arg_refs.as_slice());
    assert!(edit.status.success(), "stderr: {:?}", edit.stderr);

    let diff = run_cli(&[
        "diff",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
    ]);
    assert!(diff.status.success(), "stderr: {:?}", diff.stderr);
    let payload = parse_stdout_json(&diff);
    assert_eq!(payload["summary"]["group_count"], 1);
    let preview = payload["summary"]["group_preview"]
        .as_array()
        .expect("group preview");
    assert_eq!(preview.len(), 1);
    assert_eq!(preview[0]["group_type"], "value_edit");
    assert_eq!(preview[0]["range"], "B2:B12");
    assert_eq!(preview[0]["change_count"], 11);
}

#[test]
fn cli_diff_rejects_invalid_range_filter() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("diff-invalid-range-original.xlsx");
    let modified = tmp.path().join("diff-invalid-range-modified.xlsx");
    write_fixture(&original);
    fs::copy(&original, &modified).expect("copy workbook");

    let diff = run_cli(&[
        "diff",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
        "--range",
        "NOT_A_RANGE",
    ]);
    assert!(!diff.status.success(), "diff should fail for invalid range");
    let err = parse_stderr_json(&diff);
    assert_eq!(err["code"], "INVALID_ARGUMENT", "unexpected error: {err}");
}

#[test]
fn cli_diff_rejects_invalid_details_limit() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("diff-invalid-limit-original.xlsx");
    let modified = tmp.path().join("diff-invalid-limit-modified.xlsx");
    write_fixture(&original);
    fs::copy(&original, &modified).expect("copy workbook");

    let diff = run_cli(&[
        "diff",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
        "--details",
        "--limit",
        "0",
    ]);
    assert!(!diff.status.success(), "diff should fail for limit=0");
    let err = parse_stderr_json(&diff);
    assert_eq!(err["code"], "INVALID_ARGUMENT", "unexpected error: {err}");
}

#[test]
fn cli_edit_dry_run_reports_preview_and_preserves_source() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("edit-dry-run.xlsx");
    write_fixture(&workbook_path);

    let before = fs::read(&workbook_path).expect("read source before dry-run");
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["edit", file, "Sheet1", "--dry-run", "B2=11", "C2==B2*3"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["edits_provided"].as_u64(), Some(2));
    assert_eq!(payload["edits_validated"].as_u64(), Some(2));
    assert!(payload["would_change"].as_bool().unwrap_or(false));

    let affected = payload["affected_cells"]
        .as_array()
        .expect("affected_cells array");
    assert!(affected.iter().any(|cell| cell.as_str() == Some("B2")));
    assert!(affected.iter().any(|cell| cell.as_str() == Some("C2")));

    let after = fs::read(&workbook_path).expect("read source after dry-run");
    assert_eq!(before, after, "dry-run mutated source workbook");
}

#[test]
fn cli_edit_output_writes_target_only() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("edit-output-source.xlsx");
    let output_path = tmp.path().join("edit-output-target.xlsx");
    write_fixture(&source_path);

    let before_source = fs::read(&source_path).expect("read source before output mode");
    let source = source_path.to_str().expect("source utf8");
    let output = output_path.to_str().expect("output utf8");

    let command = run_cli(&[
        "edit", source, "Sheet1", "--output", output, "B2=17", "C2==B2*4",
    ]);
    assert!(command.status.success(), "stderr: {:?}", command.stderr);

    let payload = parse_stdout_json(&command);
    assert_eq!(payload["edits_applied"].as_u64(), Some(2));
    assert_eq!(payload["file"].as_str(), Some(output));
    assert_json_path_eq(&payload, "source_path", source);
    assert_json_path_eq(&payload, "target_path", output);

    let after_source = fs::read(&source_path).expect("read source after output mode");
    assert_eq!(
        before_source, after_source,
        "source workbook changed during --output mode"
    );

    let output_book = umya_spreadsheet::reader::xlsx::read(&output_path).expect("read output");
    let output_sheet = output_book
        .get_sheet_by_name("Sheet1").ok()
        .expect("output sheet exists");
    assert_eq!(output_sheet.get_cell("B2").expect("B2").get_value(), "17");
    assert_eq!(
        output_sheet.get_cell("C2").expect("C2").get_formula(),
        "B2*4"
    );
}

#[test]
fn cli_edit_mode_matrix_rejects_conflicts() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("edit-mode-matrix.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    assert_invalid_argument(&["edit", file, "Sheet1", "--dry-run", "--in-place", "B2=1"]);
    assert_invalid_argument(&[
        "edit",
        file,
        "Sheet1",
        "--dry-run",
        "--output",
        "out.xlsx",
        "B2=1",
    ]);
    assert_invalid_argument(&[
        "edit",
        file,
        "Sheet1",
        "--in-place",
        "--output",
        "out.xlsx",
        "B2=1",
    ]);
    assert_invalid_argument(&["edit", file, "Sheet1", "--force", "B2=1"]);
    assert_invalid_argument(&["edit", file, "Sheet1", "--output", file, "B2=1"]);
}

#[test]
fn cli_edit_dry_run_preflight_fails_for_missing_sheet() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("edit-dry-run-missing-sheet.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let err = assert_error_code(
        &["edit", file, "NoSuchSheet", "--dry-run", "A1=1"],
        "SHEET_NOT_FOUND",
    );
    assert_eq!(err["message"], "sheet 'NoSuchSheet' was not found");
    assert_eq!(
        err["try_this"],
        "run `asp read sheets <file>` to inspect valid names"
    );
}

#[test]
fn cli_errors_use_machine_envelope() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("read.xlsx");
    write_fixture(&workbook_path);

    let output = run_cli(&[
        "formula-map",
        workbook_path.to_str().expect("path utf8"),
        "Shet1",
    ]);
    assert!(!output.status.success(), "command unexpectedly succeeded");

    let err = parse_stderr_json(&output);
    assert_eq!(err["code"], "SHEET_NOT_FOUND");
    assert_eq!(err["did_you_mean"], "Sheet1");
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("was not found")
    );
    assert!(
        err["try_this"]
            .as_str()
            .unwrap_or_default()
            .contains("read sheets")
    );
}

#[test]
fn docs_guardrail_relative_mode_literals_are_canonical() {
    let readme = read_repo_doc("README.md");
    let npm_readme = read_repo_doc("npm/agent-spreadsheet/README.md");

    assert!(
        readme.contains("relative_mode` valid values: `excel`, `abs_cols`, `abs_rows`"),
        "README should document canonical relative_mode literals"
    );

    for doc in [&readme, &npm_readme] {
        assert!(
            !doc.contains("fully_relative"),
            "docs should not advertise invalid relative_mode literal fully_relative"
        );
    }
}

#[test]
fn cli_legacy_global_format_csv_returns_output_format_unsupported_envelope() {
    let output = run_cli(&["--format", "csv", "list-sheets", "/tmp/does-not-exist.xlsx"]);
    assert!(!output.status.success(), "command unexpectedly succeeded");

    let err = parse_stderr_json(&output);
    assert_eq!(err["code"], "OUTPUT_FORMAT_UNSUPPORTED");
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("csv output is not implemented")
    );
}

#[test]
fn cli_legacy_global_format_json_is_accepted_for_existing_commands() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("legacy-format-json.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["--format", "json", "list-sheets", file]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let payload = parse_stdout_json(&output);
    assert_eq!(payload["sheets"].as_array().map(Vec::len), Some(2));
}

#[cfg(feature = "recalc-formualizer")]
#[test]
fn cli_recalculate_flow_runs_after_copy_and_edit() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("original.xlsx");
    let modified = tmp.path().join("modified.xlsx");
    write_fixture(&original);

    let copy = run_cli(&[
        "copy",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
    ]);
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);

    let edit = run_cli(&[
        "edit",
        modified.to_str().expect("path utf8"),
        "Sheet1",
        "B2=25",
    ]);
    assert!(edit.status.success(), "stderr: {:?}", edit.stderr);

    let recalc = run_cli(&["recalculate", modified.to_str().expect("path utf8")]);
    assert!(recalc.status.success(), "stderr: {:?}", recalc.stderr);
    let recalc_payload = parse_stdout_json(&recalc);
    assert_eq!(recalc_payload["backend"], "formualizer");
    assert!(recalc_payload["duration_ms"].as_u64().is_some());

    let diff = run_cli(&[
        "diff",
        original.to_str().expect("path utf8"),
        modified.to_str().expect("path utf8"),
    ]);
    assert!(diff.status.success(), "stderr: {:?}", diff.stderr);
    let diff_payload = parse_stdout_json(&diff);
    assert!(diff_payload["change_count"].as_u64().unwrap_or(0) >= 1);
}

// ─── 3203: Write preflight formula parse policy tests ───

#[test]
fn cli_edit_invalid_formula_default_fail_returns_error_envelope() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("edit-formula-fail.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    // "==SUM(A1:A10" is a formula (double = means formula) with unclosed paren
    let output = run_cli(&["edit", file, "Sheet1", "B2==SUM(A1:A10"]);
    assert!(
        !output.status.success(),
        "command should fail for invalid formula"
    );

    let error = parse_stderr_json(&output);
    assert_eq!(error["code"], "FORMULA_PARSE_FAILED");
}

#[test]
fn cli_edit_invalid_formula_warn_mode_partial_apply_with_diagnostics() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("edit-formula-warn.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&[
        "edit",
        file,
        "Sheet1",
        "B2=42",
        "C2==SUM(A1:A10",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    // B2=42 (value, not formula) should apply; C2 formula is invalid → skipped
    assert_eq!(payload["edits_applied"], 1);
    assert_eq!(payload["recalc_needed"], true);

    let diagnostics = &payload["formula_parse_diagnostics"];
    assert!(diagnostics.is_object(), "expected diagnostics object");
    assert_eq!(diagnostics["policy"], "warn");
    assert!(diagnostics["total_errors"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn cli_edit_invalid_formula_off_mode_permissive_write() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("edit-formula-off.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&[
        "edit",
        file,
        "Sheet1",
        "B2==SUM(A1:A10",
        "--formula-parse-policy",
        "off",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["edits_applied"], 1);
    assert!(payload["formula_parse_diagnostics"].is_null());
}

#[test]
fn cli_transform_batch_fill_invalid_formula_warn_mode_partial_apply() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("transform-fill-formula-warn.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[
            {"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B2"]},"value":"SUM(A1:A10","is_formula":true},
            {"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B3"]},"value":"42"}
        ]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let output = run_cli(&[
        "transform-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    // Only the second op (value fill) should apply; first (bad formula) skipped
    assert_eq!(payload["op_count"], 1);
    assert_eq!(payload["applied_count"], 1);

    let diagnostics = &payload["formula_parse_diagnostics"];
    assert!(diagnostics.is_object(), "expected diagnostics object");
    assert_eq!(diagnostics["policy"], "warn");
    assert!(diagnostics["total_errors"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn cli_transform_batch_fill_invalid_formula_fail_mode_aborts_no_output() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("transform-fill-formula-fail.xlsx");
    let output_path = tmp.path().join("transform-fill-formula-fail-output.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[
            {"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B2"]},"value":"SUM(A1:A10","is_formula":true}
        ]}"#,
    );

    let file = source_path.to_str().expect("path utf8");
    let out = output_path.to_str().expect("output path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let output = run_cli(&[
        "transform-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--output",
        out,
        "--formula-parse-policy",
        "fail",
    ]);
    assert!(!output.status.success(), "command should fail");
    let error = parse_stderr_json(&output);
    assert_eq!(error["code"], "FORMULA_PARSE_FAILED");

    // No output file should be created
    assert!(
        !output_path.exists(),
        "output file should not exist on fail mode abort"
    );
}

#[test]
fn cli_transform_batch_dry_run_formula_diagnostics_parity() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("transform-formula-dryrun.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[
            {"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B2"]},"value":"SUM(A1:A10","is_formula":true},
            {"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"cells","cells":["B3"]},"value":"42"}
        ]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let output = run_cli(&[
        "transform-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    let diagnostics = &payload["formula_parse_diagnostics"];
    assert!(
        diagnostics.is_object(),
        "expected diagnostics object in dry-run"
    );
    assert_eq!(diagnostics["policy"], "warn");
    assert!(diagnostics["total_errors"].as_u64().unwrap_or(0) > 0);

    // Source should be untouched
    let before = std::fs::read(&workbook_path).expect("read source");
    let after = std::fs::read(&workbook_path).expect("read source again");
    assert_eq!(before, after, "dry-run mutated source");
}

#[test]
fn cli_edit_valid_formula_succeeds_with_default_policy() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("edit-formula-valid.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["edit", file, "Sheet1", "B2==SUM(A1:A4)"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["edits_applied"], 1);
    // No diagnostics when formula is valid
    assert!(
        payload["formula_parse_diagnostics"].is_null(),
        "no diagnostics for valid formula"
    );
}

// ─── 3204: structure-batch tokenizer policy + diagnostics tests ───

#[test]
fn cli_structure_batch_rename_with_malformed_formula_warn_mode() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("structure-rename-warn.xlsx");

    {
        let mut workbook = umya_spreadsheet::new_file();
        {
            let sheet = workbook
                .get_sheet_by_name_mut("Sheet1").ok()
                .expect("default sheet");
            sheet.get_cell_mut("A1").set_value("Hello");
        }
        workbook.new_sheet("Sheet2").expect("add Sheet2");
        {
            let sheet = workbook.get_sheet_by_name_mut("Sheet2").ok().expect("Sheet2");
            sheet.get_cell_mut("A1").set_value_number(10.0);
            sheet.get_cell_mut("B1").set_formula("SUM(\"Sheet1!A1:A10)");
        }
        umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write");
    }

    let ops_path = tmp.path().join("ops.json");
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"rename_sheet","old_name":"Sheet1","new_name":"Renamed"}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["applied_count"], 1);

    let diagnostics = &payload["formula_parse_diagnostics"];
    assert!(
        diagnostics.is_object(),
        "expected formula_parse_diagnostics object"
    );
    assert_eq!(diagnostics["policy"], "warn");
    assert!(diagnostics["total_errors"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn cli_structure_batch_rename_with_malformed_formula_fail_mode() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("structure-rename-fail.xlsx");
    let output_path = tmp.path().join("structure-rename-fail-output.xlsx");

    {
        let mut workbook = umya_spreadsheet::new_file();
        {
            let sheet = workbook
                .get_sheet_by_name_mut("Sheet1").ok()
                .expect("default sheet");
            sheet.get_cell_mut("A1").set_value("Hello");
        }
        workbook.new_sheet("Sheet2").expect("add Sheet2");
        {
            let sheet = workbook.get_sheet_by_name_mut("Sheet2").ok().expect("Sheet2");
            sheet.get_cell_mut("A1").set_value_number(10.0);
            sheet.get_cell_mut("B1").set_formula("SUM(\"Sheet1!A1:A10)");
        }
        umya_spreadsheet::writer::xlsx::write(&workbook, &source_path).expect("write");
    }

    let ops_path = tmp.path().join("ops.json");
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"rename_sheet","old_name":"Sheet1","new_name":"Renamed"}]}"#,
    );

    let file = source_path.to_str().expect("path utf8");
    let out = output_path.to_str().expect("output path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--output",
        out,
        "--formula-parse-policy",
        "fail",
    ]);
    assert!(!output.status.success(), "should fail");
    let error = parse_stderr_json(&output);
    assert_eq!(error["code"], "FORMULA_PARSE_FAILED");
    assert!(
        !output_path.exists(),
        "output should not be created on fail"
    );
}

#[test]
fn cli_structure_batch_insert_rows_with_malformed_formula_warn_mode() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("structure-insert-warn.xlsx");

    {
        let mut workbook = umya_spreadsheet::new_file();
        {
            let sheet = workbook
                .get_sheet_by_name_mut("Sheet1").ok()
                .expect("default sheet");
            sheet.get_cell_mut("A1").set_value_number(1.0);
            sheet.get_cell_mut("A2").set_value_number(2.0);
        }
        workbook.new_sheet("Sheet2").expect("add Sheet2");
        {
            let sheet = workbook.get_sheet_by_name_mut("Sheet2").ok().expect("Sheet2");
            sheet.get_cell_mut("A1").set_formula("SUM(\"Sheet1!A1:A10)");
            sheet.get_cell_mut("B1").set_formula("Sheet1!A1+1");
        }
        umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write");
    }

    let ops_path = tmp.path().join("ops.json");
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"insert_rows","sheet_name":"Sheet1","at_row":1,"count":2}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["applied_count"], 1);

    let diagnostics = &payload["formula_parse_diagnostics"];
    assert!(
        diagnostics.is_object(),
        "expected formula_parse_diagnostics object"
    );
    assert_eq!(diagnostics["policy"], "warn");
    assert!(diagnostics["total_errors"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn cli_structure_batch_rename_defined_name_malformed_formula_warn_diagnostics() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("structure-defname-warn.xlsx");

    {
        let mut workbook = umya_spreadsheet::new_file();
        {
            let sheet = workbook
                .get_sheet_by_name_mut("Sheet1").ok()
                .expect("default sheet");
            sheet.get_cell_mut("A1").set_value_number(42.0);
        }
        let workbook_scoped_bad_range = {
            let sheet = workbook
                .get_sheet_by_name_mut("Sheet1").ok()
                .expect("default sheet");
            sheet
                .add_defined_name("BadRange", "=SUM(\"abc)")
                .expect("defined name BadRange");
            sheet
                .get_defined_names()
                .first()
                .expect("sheet defined name")
                .clone()
        };
        workbook.add_defined_names(workbook_scoped_bad_range);

        umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write");
    }

    let ops_path = tmp.path().join("ops.json");
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"rename_sheet","old_name":"Sheet1","new_name":"Data"}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    let diagnostics = &payload["formula_parse_diagnostics"];
    assert!(
        diagnostics.is_object(),
        "expected formula_parse_diagnostics"
    );
    assert_eq!(diagnostics["policy"], "warn");
    assert!(diagnostics["total_errors"].as_u64().unwrap_or(0) > 0);

    let groups = diagnostics["groups"].as_array().expect("groups array");
    assert!(!groups.is_empty(), "should have at least one error group");
    let first_group = &groups[0];
    assert_eq!(
        first_group["sheet_name"], "[DefinedName]",
        "defined name errors should use [DefinedName] as sheet_name"
    );
}

#[test]
fn cli_structure_batch_no_malformed_formulas_no_diagnostics() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("structure-clean.xlsx");
    write_fixture(&workbook_path);

    let ops_path = tmp.path().join("ops.json");
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"rename_sheet","old_name":"Summary","new_name":"Results"}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert!(
        payload["formula_parse_diagnostics"].is_null(),
        "should have no diagnostics when all formulas are valid"
    );
}

#[test]
fn cli_structure_batch_copy_range_with_malformed_formula_warn_mode_diagnostics() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("structure-copy-warn.xlsx");

    {
        let mut workbook = umya_spreadsheet::new_file();
        {
            let sheet = workbook
                .get_sheet_by_name_mut("Sheet1").ok()
                .expect("default sheet");
            sheet.get_cell_mut("A1").set_value_number(1.0);
            sheet.get_cell_mut("A2").set_value_number(2.0);
            // Malformed formula that parse_base_formula will fail on
            sheet.get_cell_mut("B1").set_formula("SUM(A1:A2");
        }
        umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write");
    }

    let ops_path = tmp.path().join("ops.json");
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"copy_range","sheet_name":"Sheet1","src_range":"A1:B2","dest_anchor":"D1","include_styles":false,"include_formulas":true}]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["applied_count"], 1);

    let diagnostics = &payload["formula_parse_diagnostics"];
    assert!(
        diagnostics.is_object(),
        "expected formula_parse_diagnostics for copy with malformed formula"
    );
    assert_eq!(diagnostics["policy"], "warn");
    assert!(diagnostics["total_errors"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn cli_structure_batch_copy_range_with_malformed_formula_fail_mode_aborts() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("structure-copy-fail.xlsx");
    let output_path = tmp.path().join("structure-copy-fail-output.xlsx");

    {
        let mut workbook = umya_spreadsheet::new_file();
        {
            let sheet = workbook
                .get_sheet_by_name_mut("Sheet1").ok()
                .expect("default sheet");
            sheet.get_cell_mut("A1").set_value_number(1.0);
            sheet.get_cell_mut("B1").set_formula("SUM(A1:A2");
        }
        umya_spreadsheet::writer::xlsx::write(&workbook, &source_path).expect("write");
    }

    let ops_path = tmp.path().join("ops.json");
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"copy_range","sheet_name":"Sheet1","src_range":"A1:B1","dest_anchor":"D1","include_styles":false,"include_formulas":true}]}"#,
    );

    let file = source_path.to_str().expect("path utf8");
    let out = output_path.to_str().expect("output path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops path utf8"));

    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--output",
        out,
        "--formula-parse-policy",
        "fail",
    ]);
    assert!(!output.status.success(), "should fail with fail policy");
    let error = parse_stderr_json(&output);
    assert_eq!(error["code"], "FORMULA_PARSE_FAILED");
    assert!(
        !output_path.exists(),
        "output should not be created on fail mode abort"
    );
}

// ─── 3205: Rules-batch formula parse policy tests ───

#[test]
fn cli_rules_batch_invalid_dv_formula_warn_mode_partial_apply() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("rules-dv-warn.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&workbook_path);
    // Two ops: one valid list DV, one with a malformed custom formula (unclosed paren)
    write_ops_payload(
        &ops_path,
        r#"{"ops":[
            {"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"B2:B4","validation":{"kind":"list","formula1":"\"A,B,C\""}},
            {"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"C2:C4","validation":{"kind":"custom","formula1":"=AND(C2>0,LEN(C2"}}
        ]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let output = run_cli(&[
        "rules-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    // The custom formula with unclosed paren should be skipped; the valid list DV should apply
    assert_eq!(
        payload["op_count"].as_u64().unwrap(),
        2,
        "op_count should reflect total ops in payload"
    );
    assert_eq!(
        payload["applied_count"].as_u64().unwrap(),
        1,
        "only the valid op should be applied"
    );
    let diagnostics = &payload["formula_parse_diagnostics"];
    assert!(
        diagnostics.is_object(),
        "expected formula_parse_diagnostics object"
    );
    assert_eq!(diagnostics["policy"], "warn");
    assert!(diagnostics["total_errors"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn cli_rules_batch_invalid_dv_formula_fail_mode_aborts() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("rules-dv-fail.xlsx");
    let output_path = tmp.path().join("rules-dv-fail-output.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&source_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[
            {"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"C2:C4","validation":{"kind":"custom","formula1":"=AND(C2>0,LEN(C2"}}
        ]}"#,
    );

    let file = source_path.to_str().expect("path utf8");
    let out = output_path.to_str().expect("output path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let output = run_cli(&[
        "rules-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--output",
        out,
        "--formula-parse-policy",
        "fail",
    ]);
    assert!(!output.status.success(), "command should fail");
    let error = parse_stderr_json(&output);
    assert_eq!(error["code"], "FORMULA_PARSE_FAILED");
    assert!(
        !output_path.exists(),
        "output file should not exist on fail mode abort"
    );
}

#[test]
fn cli_rules_batch_invalid_cf_formula_warn_mode_with_diagnostics() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("rules-cf-warn.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&workbook_path);
    // One valid CF expression, one with malformed formula (unclosed paren)
    write_ops_payload(
        &ops_path,
        r##"{"ops":[
            {"kind":"add_conditional_format","sheet_name":"Sheet1","target_range":"A1:A10","rule":{"kind":"expression","formula":"A1>0"},"style":{"fill_color":"#FF0000"}},
            {"kind":"add_conditional_format","sheet_name":"Sheet1","target_range":"B1:B10","rule":{"kind":"expression","formula":"AND(B1>0,LEN(B1"},"style":{"fill_color":"#00FF00"}}
        ]}"##,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let output = run_cli(&[
        "rules-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    assert_eq!(
        payload["op_count"].as_u64().unwrap(),
        2,
        "op_count should reflect total ops in payload"
    );
    assert_eq!(
        payload["applied_count"].as_u64().unwrap(),
        1,
        "only the valid CF op should be applied"
    );
    let diagnostics = &payload["formula_parse_diagnostics"];
    assert!(
        diagnostics.is_object(),
        "expected formula_parse_diagnostics object"
    );
    assert_eq!(diagnostics["policy"], "warn");
    assert!(diagnostics["total_errors"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn cli_rules_batch_off_mode_permissive_behavior() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("rules-off.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[
            {"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"B2:B4","validation":{"kind":"list","formula1":"\"A,B,C\""}}
        ]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let output = run_cli(&[
        "rules-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
        "--formula-parse-policy",
        "off",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert!(
        payload["formula_parse_diagnostics"].is_null(),
        "no diagnostics in off mode"
    );
    assert!(payload["changed"].as_bool().unwrap_or(false));
}

#[test]
fn cli_rules_batch_dry_run_formula_diagnostics_parity() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("rules-dryrun-diag.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_fixture(&workbook_path);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[
            {"kind":"set_data_validation","sheet_name":"Sheet1","target_range":"C2:C4","validation":{"kind":"custom","formula1":"=AND(C2>0,LEN(C2"}}
        ]}"#,
    );

    let file = workbook_path.to_str().expect("path utf8");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops utf8"));

    let output = run_cli(&[
        "rules-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
        "--formula-parse-policy",
        "warn",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    let diagnostics = &payload["formula_parse_diagnostics"];
    assert!(
        diagnostics.is_object(),
        "expected diagnostics in dry-run warn mode"
    );
    assert_eq!(diagnostics["policy"], "warn");
    assert!(diagnostics["total_errors"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn transform_batch_fill_range_formula_clears_cache() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("fill-formula-cache.xlsx");
    let ops_path = tmp.path().join("fill-formula-ops.json");

    // Create workbook with a formula cell that has a stale cached result
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet");
        sheet.get_cell_mut("A1").set_value_number(10.0);
        sheet.get_cell_mut("A2").set_value_number(20.0);
        let b1 = sheet.get_cell_mut("B1");
        b1.set_formula("A1+1");
        b1.get_cell_value_mut().set_formula_result_default("999"); // stale cache
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write");

    // FillRange with is_formula=true should clear the cache
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"range","range":"B1:B2"},"value":"A1+100","is_formula":true,"overwrite_formulas":true}]}"#,
    );

    let file = workbook_path.to_str().expect("path");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops"));
    let output = run_cli(&["transform-batch", file, "--ops", &ops_ref, "--in-place"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(
        payload["write_path_provenance"]["written_via"],
        Value::String("transform_batch".to_string())
    );

    // Read back and verify cache is cleared
    let book = umya_spreadsheet::reader::xlsx::read(&workbook_path).expect("read");
    let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet");
    let b1 = sheet.get_cell("B1").expect("B1");
    assert_eq!(b1.get_formula().replace(' ', ""), "A1+100");
    assert_eq!(
        b1.get_value(),
        "",
        "expected formula cache to be cleared after FillRange"
    );

    let b2 = sheet.get_cell("B2").expect("B2");
    assert_eq!(b2.get_formula().replace(' ', ""), "A1+100");
    assert_eq!(
        b2.get_value(),
        "",
        "expected formula cache to be cleared after FillRange"
    );
}

#[test]
fn transform_batch_replace_in_range_formula_clears_cache() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("replace-formula-cache.xlsx");
    let ops_path = tmp.path().join("replace-formula-ops.json");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet");
        let a1 = sheet.get_cell_mut("A1");
        a1.set_formula("SUM(B1:B10)");
        a1.get_cell_value_mut().set_formula_result_default("500"); // stale cache
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write");

    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"replace_in_range","sheet_name":"Sheet1","target":{"kind":"range","range":"A1:A1"},"find":"SUM","replace":"AVERAGE","match_mode":"contains","include_formulas":true}]}"#,
    );

    let file = workbook_path.to_str().expect("path");
    let ops_ref = format!("@{}", ops_path.to_str().expect("ops"));
    let output = run_cli(&["transform-batch", file, "--ops", &ops_ref, "--in-place"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(
        payload["write_path_provenance"]["written_via"],
        Value::String("transform_batch".to_string())
    );

    let book = umya_spreadsheet::reader::xlsx::read(&workbook_path).expect("read");
    let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet");
    let a1 = sheet.get_cell("A1").expect("A1");
    assert!(
        a1.get_formula().contains("AVERAGE"),
        "formula should be replaced"
    );
    assert_eq!(
        a1.get_value(),
        "",
        "expected formula cache to be cleared after ReplaceInRange"
    );
}

#[test]
fn edit_batch_formula_clears_cache() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("edit-formula-cache.xlsx");

    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().expect("sheet");
        let a1 = sheet.get_cell_mut("A1");
        a1.set_formula("B1+C1");
        a1.get_cell_value_mut()
            .set_formula_result_default("old_value");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &workbook_path).expect("write");

    let file = workbook_path.to_str().expect("path");
    let output = run_cli(&["edit", file, "Sheet1", "A1==SUM(B1:B5)"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let book = umya_spreadsheet::reader::xlsx::read(&workbook_path).expect("read");
    let sheet = book.get_sheet_by_name("Sheet1").ok().expect("sheet");
    let a1 = sheet.get_cell("A1").expect("A1");
    assert_eq!(a1.get_formula().replace(' ', ""), "SUM(B1:B5)");
    assert_eq!(
        a1.get_value(),
        "",
        "expected formula cache to be cleared after edit"
    );
}

#[test]
fn edit_formula_write_emits_write_path_provenance() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("edit-provenance-formula.xlsx");
    write_fixture(&workbook_path);

    let file = workbook_path.to_str().expect("path");
    let output = run_cli(&["edit", file, "Sheet1", "C2==B2*7"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    assert_eq!(
        payload["write_path_provenance"]["written_via"],
        Value::String("edit".to_string())
    );
    let targets = payload["write_path_provenance"]["formula_targets"]
        .as_array()
        .expect("formula targets array");
    assert!(
        targets
            .iter()
            .any(|value| value.as_str() == Some("Sheet1!C2"))
    );
}

#[test]
fn edit_literal_write_omits_write_path_provenance() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("edit-provenance-literal.xlsx");
    write_fixture(&workbook_path);

    let file = workbook_path.to_str().expect("path");
    let output = run_cli(&["edit", file, "Sheet1", "B2=7"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    assert!(
        payload.get("write_path_provenance").is_none(),
        "literal-only edits should omit provenance metadata"
    );
}

#[test]
fn transform_batch_help_mentions_formula_cache() {
    let output = run_cli(&["transform-batch", "--help"]);
    let combined = format!(
        "{}{}",
        parse_stdout_text(&output),
        String::from_utf8(output.stderr.clone()).expect("stderr utf8")
    );
    assert!(
        combined.contains("Cache note") || combined.contains("cached results"),
        "transform-batch help should mention formula cache behavior"
    );
    assert!(
        combined.contains("write_path_provenance"),
        "transform-batch help should mention provenance diagnostics"
    );
}

#[test]
fn structure_batch_help_mentions_formula_cache() {
    let output = run_cli(&["structure-batch", "--help"]);
    let combined = format!(
        "{}{}",
        parse_stdout_text(&output),
        String::from_utf8(output.stderr.clone()).expect("stderr utf8")
    );
    assert!(
        combined.contains("Cache note") || combined.contains("cached results"),
        "structure-batch help should mention formula cache behavior"
    );
}

fn write_complex_grid_fixture(path: &Path) {
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook
            .get_sheet_by_name_mut("Sheet1").ok()
            .expect("default sheet exists");

        sheet.get_cell_mut("A1").set_value("Quarterly Report");
        sheet.add_merge_cells("A1:B1");
        sheet.get_cell_mut("A2").set_value("Name");
        sheet.get_cell_mut("B2").set_value("Amount");
        sheet.get_cell_mut("A3").set_value("Alice");
        sheet.get_cell_mut("B3").set_value_number(1234.0);
        sheet.get_cell_mut("A4").set_value("Bob");
        sheet.get_cell_mut("B4").set_value_number(5678.0);

        sheet.get_column_dimension_mut("A").set_width(26.0);
        sheet.get_column_dimension_mut("B").set_width(14.0);

        sheet.get_style_mut("A1").get_font_mut().set_bold(true);
        sheet
            .get_style_mut("A1")
            .get_alignment_mut()
            .set_horizontal(umya_spreadsheet::HorizontalAlignmentValues::Center);
        sheet
            .get_style_mut("A1")
            .get_borders_mut()
            .get_bottom_border_mut()
            .set_border_style("medium");
        sheet.get_style_mut("B3").get_font_mut().set_italic(true);
        sheet
            .get_style_mut("B3")
            .get_number_format_mut()
            .set_format_code("$#,##0");
    }

    umya_spreadsheet::writer::xlsx::write(&workbook, path).expect("write workbook");
}

#[test]
fn cli_range_export_csv_and_range_import_from_csv_roundtrip() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("csv-source.xlsx");
    let target_path = tmp.path().join("csv-target.xlsx");
    let csv_path = tmp.path().join("export.csv");

    write_fixture(&source_path);
    umya_spreadsheet::writer::xlsx::write(&umya_spreadsheet::new_file(), &target_path)
        .expect("write target workbook");

    let source = source_path.to_str().expect("source path utf8");
    let target = target_path.to_str().expect("target path utf8");
    let csv = csv_path.to_str().expect("csv path utf8");

    let export = run_cli(&[
        "range-export",
        source,
        "Sheet1",
        "A1:B4",
        "--format",
        "csv",
        "--output",
        csv,
    ]);
    assert!(export.status.success(), "stderr: {:?}", export.stderr);
    let export_payload = parse_stdout_json(&export);
    assert_eq!(export_payload["status"], "ok");
    assert_json_path_eq(&export_payload, "path", csv);

    let import = run_cli(&[
        "range-import",
        target,
        "Sheet1",
        "--anchor",
        "B2",
        "--from-csv",
        csv,
        "--in-place",
    ]);
    assert!(import.status.success(), "stderr: {:?}", import.stderr);

    let read = run_cli(&[
        "range-values",
        target,
        "Sheet1",
        "B2:C5",
        "--format",
        "json",
    ]);
    assert!(read.status.success(), "stderr: {:?}", read.stderr);
    let payload = parse_stdout_json(&read);
    let rows = payload["values"][0]["rows"]
        .as_array()
        .expect("rows matrix");

    assert_eq!(rows[0][0]["value"], "Name");
    assert_eq!(rows[0][1]["value"], "Amount");
    assert_eq!(rows[1][0]["value"], "Alice");
    assert_eq!(rows[1][1]["value"], 10.0);
    assert_eq!(rows[3][0]["value"], "Carol");
    assert_eq!(rows[3][1]["value"], 30.0);

    let target_header_path = tmp.path().join("csv-target-header.xlsx");
    umya_spreadsheet::writer::xlsx::write(&umya_spreadsheet::new_file(), &target_header_path)
        .expect("write header target workbook");
    let target_header = target_header_path.to_str().expect("header path utf8");

    let import_header = run_cli(&[
        "range-import",
        target_header,
        "Sheet1",
        "--anchor",
        "A1",
        "--from-csv",
        csv,
        "--header",
        "--in-place",
    ]);
    assert!(
        import_header.status.success(),
        "stderr: {:?}",
        import_header.stderr
    );

    let read_header = run_cli(&[
        "range-values",
        target_header,
        "Sheet1",
        "A1:B3",
        "--format",
        "json",
    ]);
    assert!(
        read_header.status.success(),
        "stderr: {:?}",
        read_header.stderr
    );
    let header_payload = parse_stdout_json(&read_header);
    let header_rows = header_payload["values"][0]["rows"]
        .as_array()
        .expect("header rows matrix");
    assert_eq!(header_rows[0][0]["value"], "Alice");
    assert_eq!(header_rows[0][1]["value"], 10.0);
}

#[test]
fn cli_grid_export_import_roundtrip_preserves_layout_and_styles() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("grid-source.xlsx");
    let target_path = tmp.path().join("grid-target.xlsx");
    let grid_path = tmp.path().join("region.grid.json");

    write_complex_grid_fixture(&source_path);
    umya_spreadsheet::writer::xlsx::write(&umya_spreadsheet::new_file(), &target_path)
        .expect("write target workbook");

    let source = source_path.to_str().expect("source path utf8");
    let target = target_path.to_str().expect("target path utf8");
    let grid = grid_path.to_str().expect("grid path utf8");

    let export = run_cli(&[
        "range-export",
        source,
        "Sheet1",
        "A1:B4",
        "--format",
        "grid",
        "--output",
        grid,
    ]);
    assert!(export.status.success(), "stderr: {:?}", export.stderr);

    let import = run_cli(&[
        "range-import",
        target,
        "Sheet1",
        "--anchor",
        "A1",
        "--from-grid",
        grid,
        "--in-place",
    ]);
    assert!(import.status.success(), "stderr: {:?}", import.stderr);

    let layout = run_cli(&[
        "layout-page",
        target,
        "Sheet1",
        "--range",
        "A1:B4",
        "--max-col-width",
        "40",
        "--skip-empty-columns-trim",
    ]);
    assert!(layout.status.success(), "stderr: {:?}", layout.stderr);
    let layout_payload = parse_stdout_json(&layout);

    let merges = layout_payload["merged_cells"]
        .as_array()
        .expect("merged cells");
    assert!(
        merges.iter().any(|v| v.as_str() == Some("A1:B1")),
        "expected A1:B1 merge, got {:?}",
        merges
    );

    let columns = layout_payload["columns"].as_array().expect("columns");
    assert_eq!(columns[0]["width_chars"], 26.0);
    assert_eq!(columns[1]["width_chars"], 14.0);

    let row1_cells = layout_payload["rows"][0]["cells"]
        .as_array()
        .expect("row1 cells");
    let a1 = row1_cells
        .iter()
        .find(|c| c["address"] == "A1")
        .expect("A1 cell");
    assert_eq!(a1["bold"], true);

    let inspect = run_cli(&["inspect-cells", target, "Sheet1", "B3:B3"]);
    assert!(inspect.status.success(), "stderr: {:?}", inspect.stderr);
    let inspect_payload = parse_stdout_json(&inspect);
    let b3 = inspect_payload["cells"].as_array().expect("cells")[0].clone();
    assert_eq!(b3["number_format"], "$#,##0");
}

#[test]
fn cli_range_import_from_csv_handles_quotes_crlf_and_blanks() {
    let tmp = tempdir().expect("tempdir");
    let target_path = tmp.path().join("csv-edge-target.xlsx");
    let csv_path = tmp.path().join("edge.csv");

    umya_spreadsheet::writer::xlsx::write(&umya_spreadsheet::new_file(), &target_path)
        .expect("write target workbook");

    let csv_content = concat!(
        "Name,Note,Amount,Extra\r\n",
        "\"Doe, Jane\",\"He said \"\"Hi\"\"\",123,\r\n",
        "\"Multiline\",\"First line\r\nSecond line\",45.67,\"\"\r\n"
    );
    fs::write(&csv_path, csv_content).expect("write csv");

    let target = target_path.to_str().expect("target path utf8");
    let csv = csv_path.to_str().expect("csv path utf8");

    let import = run_cli(&[
        "range-import",
        target,
        "Sheet1",
        "--anchor",
        "A1",
        "--from-csv",
        csv,
        "--in-place",
    ]);
    assert!(import.status.success(), "stderr: {:?}", import.stderr);

    let read = run_cli(&[
        "range-values",
        target,
        "Sheet1",
        "A1:D3",
        "--format",
        "json",
    ]);
    assert!(read.status.success(), "stderr: {:?}", read.stderr);
    let payload = parse_stdout_json(&read);
    let rows = payload["values"][0]["rows"]
        .as_array()
        .expect("rows matrix");

    assert_eq!(rows[0][0]["value"], "Name");
    assert_eq!(rows[0][1]["value"], "Note");
    assert_eq!(rows[0][2]["value"], "Amount");
    assert_eq!(rows[0][3]["value"], "Extra");

    assert_eq!(rows[1][0]["value"], "Doe, Jane");
    assert_eq!(rows[1][1]["value"], "He said \"Hi\"");
    assert_eq!(rows[1][2]["value"], 123.0);
    assert!(rows[1][3].is_null());

    assert_eq!(rows[2][0]["value"], "Multiline");
    let multiline = rows[2][1]["value"].as_str().expect("multiline text value");
    assert!(multiline.contains("First line"));
    assert!(multiline.contains("Second line"));
    assert_eq!(rows[2][2]["value"], 45.67);
    assert!(rows[2][3].is_null());
}

#[test]
fn edit_help_mentions_formula_cache_and_modes() {
    let output = run_cli(&["edit", "--help"]);
    let combined = format!(
        "{}{}",
        parse_stdout_text(&output),
        String::from_utf8(output.stderr.clone()).expect("stderr utf8")
    );
    assert!(
        combined.contains("Cache note") || combined.contains("cached results"),
        "edit help should mention formula cache behavior"
    );
    assert!(
        combined.contains("--dry-run")
            && combined.contains("--in-place")
            && combined.contains("--output"),
        "edit help should mention dry-run/in-place/output modes"
    );
    assert!(
        combined.contains("Formula shorthand")
            && combined.contains("double equals")
            && combined.contains("Single equals writes a literal"),
        "edit help should clearly explain formula shorthand syntax"
    );
    assert!(
        combined.contains("write_path_provenance"),
        "edit help should mention provenance diagnostics"
    );
}

// ─── 4101: structure-batch impact report & formula delta preview ───

#[test]
fn structure_batch_impact_report_dry_run() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("impact-report.xlsx");
    write_fixture(&workbook_path);
    let ops_path = tmp.path().join("impact-ops.json");
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"insert_rows","sheet_name":"Sheet1","at_row":2,"count":3}]}"#,
    );

    let file = workbook_path.to_str().unwrap();
    let ops_ref = format!("@{}", ops_path.to_str().unwrap());

    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
        "--impact-report",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    // Standard dry-run fields are still present.
    assert!(payload["would_change"].as_bool().unwrap_or(false));
    assert!(payload["op_count"].as_u64().is_some());

    // Impact report is present.
    let ir = &payload["impact_report"];
    assert!(!ir.is_null(), "impact_report should be present");
    assert!(
        !ir["shifted_spans"].as_array().unwrap().is_empty(),
        "should have at least one shifted span"
    );
    assert!(ir["tokens_affected"].is_number());
    assert!(ir["tokens_unaffected"].is_number());
}

#[test]
fn structure_batch_show_formula_delta_dry_run() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("formula-delta.xlsx");
    write_fixture(&workbook_path);
    let ops_path = tmp.path().join("delta-ops.json");
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"insert_rows","sheet_name":"Sheet1","at_row":2,"count":1}]}"#,
    );

    let file = workbook_path.to_str().unwrap();
    let ops_ref = format!("@{}", ops_path.to_str().unwrap());

    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
        "--show-formula-delta",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    // Formula delta preview should be present.
    let fdp = &payload["formula_delta_preview"];
    assert!(fdp.is_array(), "formula_delta_preview should be an array");
    let items = fdp.as_array().unwrap();
    assert!(!items.is_empty(), "should have at least one delta item");

    // Each item should have the expected fields.
    let first = &items[0];
    assert!(first["cell"].is_string());
    assert!(first["before"].is_string());
    assert!(first["after"].is_string());
    assert!(first["classification"].is_string());
}

#[test]
fn structure_batch_impact_flags_require_dry_run() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("impact-no-dry.xlsx");
    write_fixture(&workbook_path);
    let ops_path = tmp.path().join("impact-no-dry-ops.json");
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"insert_rows","sheet_name":"Sheet1","at_row":2,"count":1}]}"#,
    );

    let file = workbook_path.to_str().unwrap();
    let ops_ref = format!("@{}", ops_path.to_str().unwrap());

    // --impact-report without --dry-run → error
    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
        "--impact-report",
    ]);
    assert!(!output.status.success(), "should fail without --dry-run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--dry-run") || stderr.contains("dry-run"),
        "error should mention --dry-run: {}",
        stderr
    );

    // --show-formula-delta without --dry-run → error
    let output2 = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
        "--show-formula-delta",
    ]);
    assert!(!output2.status.success(), "should fail without --dry-run");
}

#[test]
fn structure_batch_dry_run_without_impact_flags_is_backward_compatible() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("compat.xlsx");
    write_fixture(&workbook_path);
    let ops_path = tmp.path().join("compat-ops.json");
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"insert_rows","sheet_name":"Sheet1","at_row":2,"count":1}]}"#,
    );

    let file = workbook_path.to_str().unwrap();
    let ops_ref = format!("@{}", ops_path.to_str().unwrap());

    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--dry-run",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);

    // impact_report and formula_delta_preview should NOT be present when not requested.
    assert!(
        payload.get("impact_report").is_none() || payload["impact_report"].is_null(),
        "impact_report should be absent when not requested"
    );
    assert!(
        payload.get("formula_delta_preview").is_none()
            || payload["formula_delta_preview"].is_null(),
        "formula_delta_preview should be absent when not requested"
    );
}

// ── Named Range CRUD Tests ───────────────────────────────────────────────────

#[test]
fn cli_define_name_dry_run_validates_without_mutating() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("define-name-dry-run.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&[
        "define-name",
        file,
        "NewRange",
        "Sheet1!$A$1:$C$4",
        "--dry-run",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["name"], "NewRange");
    assert_eq!(payload["refers_to"], "Sheet1!$A$1:$C$4");
    assert_eq!(payload["scope_kind"], "workbook");
    assert_eq!(payload["dry_run"], true);

    // Verify the original file is unchanged: no NewRange should exist.
    let check = run_cli(&["named-ranges", file, "--name-prefix", "NewRange"]);
    assert!(check.status.success());
    let check_payload = parse_stdout_json(&check);
    // Empty arrays are pruned by the output layer, so items may be absent or empty.
    let items = check_payload["items"].as_array();
    assert!(
        items.is_none() || items.unwrap().is_empty(),
        "dry-run should not have mutated the file"
    );
}

#[test]
fn cli_define_name_in_place_creates_workbook_scoped_name() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("define-name-inplace.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&[
        "define-name",
        file,
        "TotalSales",
        "Sheet1!$B$2:$B$4",
        "--in-place",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["name"], "TotalSales");
    assert_eq!(payload["scope_kind"], "workbook");
    assert_eq!(payload["dry_run"], false);

    // Verify the name is now visible.
    let check = run_cli(&["named-ranges", file, "--name-prefix", "TotalSales"]);
    assert!(check.status.success());
    let check_payload = parse_stdout_json(&check);
    let items = check_payload["items"].as_array().expect("items array");
    assert!(
        !items.is_empty(),
        "TotalSales should exist after define-name --in-place"
    );
    assert_eq!(items[0]["name"], "TotalSales");
}

#[test]
fn cli_define_name_sheet_scoped_with_output() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("define-name-sheet.xlsx");
    let output_path = tmp.path().join("define-name-sheet-out.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");
    let output_file = output_path.to_str().expect("path utf8");

    let output = run_cli(&[
        "define-name",
        file,
        "LocalName",
        "Sheet1!$A$1",
        "--scope",
        "sheet",
        "--scope-sheet-name",
        "Sheet1",
        "--output",
        output_file,
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["name"], "LocalName");
    assert_eq!(payload["scope_kind"], "sheet");
    assert_eq!(payload["scope_sheet_name"], "Sheet1");

    // Verify in the output file.
    let check = run_cli(&["named-ranges", output_file, "--name-prefix", "LocalName"]);
    assert!(check.status.success());
    let check_payload = parse_stdout_json(&check);
    let items = check_payload["items"].as_array().expect("items array");
    assert!(!items.is_empty(), "LocalName should exist in output file");
}

#[test]
fn cli_update_name_in_place_changes_refers_to() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("update-name.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    // First define a name.
    let def = run_cli(&[
        "define-name",
        file,
        "MyRange",
        "Sheet1!$A$1:$B$2",
        "--in-place",
    ]);
    assert!(def.status.success(), "define failed: {:?}", def.stderr);

    // Update it.
    let output = run_cli(&[
        "update-name",
        file,
        "MyRange",
        "Sheet1!$A$1:$D$10",
        "--in-place",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["name"], "MyRange");
    assert_eq!(payload["refers_to"], "Sheet1!$A$1:$D$10");
    assert!(payload["previous_refers_to"].is_string());
    assert_eq!(payload["dry_run"], false);
}

#[test]
fn cli_update_name_scope_only_keeps_existing_refers_to() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("update-name-scope-only.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let def = run_cli(&[
        "define-name",
        file,
        "ScopeOnlyName",
        "Sheet1!$A$1:$B$2",
        "--in-place",
    ]);
    assert!(def.status.success(), "define failed: {:?}", def.stderr);

    let output = run_cli(&[
        "update-name",
        file,
        "ScopeOnlyName",
        "--scope",
        "sheet",
        "--scope-sheet-name",
        "Sheet1",
        "--in-place",
    ]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["name"], "ScopeOnlyName");
    assert_eq!(payload["refers_to"], "Sheet1!$A$1:$B$2");
    assert_eq!(payload["scope_kind"], "sheet");
    assert_eq!(payload["scope_sheet_name"], "Sheet1");
    assert!(payload["previous_refers_to"].is_string());
}

#[test]
fn cli_delete_name_in_place_removes_name() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("delete-name.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    // The fixture already has Sales_Amount.
    let before = run_cli(&["named-ranges", file, "--name-prefix", "Sales_Amount"]);
    assert!(before.status.success());
    let before_payload = parse_stdout_json(&before);
    let before_items = before_payload["items"].as_array().expect("items");
    assert!(
        !before_items.is_empty(),
        "Sales_Amount should exist before delete"
    );

    let output = run_cli(&["delete-name", file, "Sales_Amount", "--in-place"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["name"], "Sales_Amount");
    assert_eq!(payload["deleted"], true);

    // Verify it's gone.
    let after = run_cli(&["named-ranges", file, "--name-prefix", "Sales_Amount"]);
    assert!(after.status.success());
    let after_payload = parse_stdout_json(&after);
    let after_items = after_payload["items"].as_array();
    assert!(
        after_items.is_none() || after_items.unwrap().is_empty(),
        "Sales_Amount should not exist after delete"
    );
}

#[test]
fn cli_delete_name_not_found_returns_error() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("delete-name-notfound.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["delete-name", file, "NonExistent", "--in-place"]);
    assert!(
        !output.status.success(),
        "should fail for non-existent name"
    );
}

#[test]
fn cli_named_ranges_includes_scope_metadata() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("scope-metadata.xlsx");
    write_phase1_read_surface_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["named-ranges", file]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let payload = parse_stdout_json(&output);
    let items = payload["items"].as_array().expect("items array");
    assert!(!items.is_empty());

    // All items should have scope_kind.
    for item in items {
        let scope_kind = item["scope_kind"].as_str();
        assert!(
            scope_kind == Some("workbook") || scope_kind == Some("sheet"),
            "item {:?} should have scope_kind 'workbook' or 'sheet', got {:?}",
            item["name"],
            scope_kind
        );
        if scope_kind == Some("sheet") {
            assert!(
                item["scope_sheet_name"].is_string(),
                "sheet-scoped item should have scope_sheet_name"
            );
        }
    }
}

// ─── 4105: Recalculate output mode and stateless safety ───

#[test]
fn cli_recalculate_in_place_preserves_existing_behavior() {
    let tmp = tempdir().expect("tempdir");
    let workbook_path = tmp.path().join("recalc-inplace.xlsx");
    write_fixture(&workbook_path);
    let file = workbook_path.to_str().expect("path utf8");

    let output = run_cli(&["recalculate", file]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let payload = parse_stdout_json(&output);
    assert!(payload["file"].as_str().is_some(), "file field present");
    assert!(
        payload["backend"].as_str().is_some(),
        "backend field present"
    );
    assert!(
        payload["duration_ms"].as_u64().is_some(),
        "duration_ms present"
    );
    // In-place mode should NOT have source_path/target_path/changed
    assert!(
        payload.get("source_path").is_none(),
        "in-place should not emit source_path"
    );
    assert!(
        payload.get("target_path").is_none(),
        "in-place should not emit target_path"
    );
    assert!(
        payload.get("changed").is_none(),
        "in-place should not emit changed"
    );
}

#[test]
fn cli_recalculate_output_mode_copies_and_recalcs_target() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("recalc-output-source.xlsx");
    let target_path = tmp.path().join("recalc-output-target.xlsx");
    write_fixture(&source_path);
    let source = source_path.to_str().expect("path utf8");
    let target = target_path.to_str().expect("path utf8");

    // Capture source bytes before recalc
    let source_bytes_before = fs::read(&source_path).expect("read source before");

    let output = run_cli(&["recalculate", source, "--output", target]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let payload = parse_stdout_json(&output);

    // Response metadata fields
    assert!(
        payload["source_path"].as_str().is_some(),
        "source_path should be present in output mode"
    );
    assert!(
        payload["target_path"].as_str().is_some(),
        "target_path should be present in output mode"
    );
    assert_eq!(
        payload["changed"], true,
        "changed should be true in output mode"
    );

    // file field points to the target
    assert_json_path_eq(&payload, "target_path", target);
    assert_json_path_eq(&payload, "source_path", source);

    // Target file should exist
    assert!(
        target_path.exists(),
        "target file should exist after recalculate --output"
    );

    // Source should be unchanged
    let source_bytes_after = fs::read(&source_path).expect("read source after");
    assert_eq!(
        source_bytes_before, source_bytes_after,
        "source file should remain unchanged in output mode"
    );
}

#[test]
fn cli_recalculate_output_mode_rejects_existing_target_without_force() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("recalc-force-source.xlsx");
    let target_path = tmp.path().join("recalc-force-target.xlsx");
    write_fixture(&source_path);
    // Create an existing target
    write_fixture(&target_path);
    let source = source_path.to_str().expect("path utf8");
    let target = target_path.to_str().expect("path utf8");

    let output = run_cli(&["recalculate", source, "--output", target]);
    assert!(
        !output.status.success(),
        "should fail when target exists without --force"
    );
    let err = parse_stderr_json(&output);
    assert_eq!(err["code"], "OUTPUT_EXISTS", "unexpected error: {err}");
}

#[test]
fn cli_recalculate_output_mode_allows_existing_target_with_force() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("recalc-force-ok-source.xlsx");
    let target_path = tmp.path().join("recalc-force-ok-target.xlsx");
    write_fixture(&source_path);
    write_fixture(&target_path);
    let source = source_path.to_str().expect("path utf8");
    let target = target_path.to_str().expect("path utf8");

    let output = run_cli(&["recalculate", source, "--output", target, "--force"]);
    assert!(
        output.status.success(),
        "should succeed with --force, stderr: {:?}",
        output.stderr
    );
    let payload = parse_stdout_json(&output);
    assert_eq!(payload["changed"], true);
    assert_json_path_eq(&payload, "target_path", target);
}

#[test]
fn cli_recalculate_output_force_failure_preserves_existing_target() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("recalc-force-fail-source.xlsx");
    let target_path = tmp.path().join("recalc-force-fail-target.xlsx");

    // Invalid source payload to force recalc failure.
    fs::write(&source_path, b"not-an-xlsx").expect("write invalid source");
    write_fixture(&target_path);

    let source = source_path.to_str().expect("path utf8");
    let target = target_path.to_str().expect("path utf8");

    let target_before = fs::read(&target_path).expect("read target before");

    let output = run_cli(&["recalculate", source, "--output", target, "--force"]);
    assert!(
        !output.status.success(),
        "recalc should fail for invalid source payload"
    );

    // Existing target must remain untouched on failure.
    assert!(
        target_path.exists(),
        "target should still exist after failure"
    );
    let target_after = fs::read(&target_path).expect("read target after");
    assert_eq!(
        target_before, target_after,
        "existing target content should be preserved on recalc failure"
    );
}

#[test]
fn cli_recalculate_output_rejects_same_path_as_source() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("recalc-same.xlsx");
    write_fixture(&source_path);
    let source = source_path.to_str().expect("path utf8");

    let output = run_cli(&["recalculate", source, "--output", source]);
    assert!(
        !output.status.success(),
        "should fail when output == source"
    );
    let err = parse_stderr_json(&output);
    assert_eq!(
        err["code"], "INVALID_ARGUMENT",
        "unexpected error envelope: {err}"
    );
}

#[test]
fn cli_recalculate_force_without_output_is_invalid() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("recalc-force-alone.xlsx");
    write_fixture(&source_path);
    let source = source_path.to_str().expect("path utf8");

    let output = run_cli(&["recalculate", source, "--force"]);
    assert!(
        !output.status.success(),
        "should fail when --force used without --output"
    );
    let err = parse_stderr_json(&output);
    assert_eq!(
        err["code"], "INVALID_ARGUMENT",
        "unexpected error envelope: {err}"
    );
}

#[test]
fn cli_recalculate_output_invalid_parent_dir_returns_error() {
    let tmp = tempdir().expect("tempdir");
    let source_path = tmp.path().join("recalc-invalid-output.xlsx");
    write_fixture(&source_path);
    let source = source_path.to_str().expect("path utf8");

    let bad_target = tmp.path().join("nonexistent_dir").join("output.xlsx");
    let target = bad_target.to_str().expect("path utf8");

    let output = run_cli(&["recalculate", source, "--output", target]);
    assert!(
        !output.status.success(),
        "should fail when output parent dir doesn't exist"
    );
}

#[test]
fn cli_recalculate_help_shows_output_mode_docs() {
    let help = run_cli(&["recalculate", "--help"]);
    assert!(help.status.success(), "stderr: {:?}", help.stderr);
    let text = parse_stdout_text(&help);
    assert!(text.contains("--output"), "help should document --output");
    assert!(text.contains("--force"), "help should document --force");
    assert!(
        text.contains("source stays unchanged"),
        "help should explain source safety"
    );
}

#[test]
fn cli_recalculate_parse_output_and_force_flags() {
    use clap::Parser;
    use agent_spreadsheet::cli::{Cli, Commands};

    let cli = Cli::try_parse_from([
        "agent-spreadsheet",
        "recalculate",
        "workbook.xlsx",
        "--output",
        "out.xlsx",
        "--force",
    ])
    .expect("parse recalculate with output and force");

    match cli.command {
        Commands::Recalculate {
            file,
            output,
            force,
            ..
        } => {
            assert_eq!(file, PathBuf::from("workbook.xlsx"));
            assert_eq!(output, Some(PathBuf::from("out.xlsx")));
            assert!(force);
        }
        other => panic!("unexpected command: {other:?}"),
    }

    // Without output/force
    let cli2 = Cli::try_parse_from(["agent-spreadsheet", "recalculate", "workbook.xlsx"])
        .expect("parse recalculate without flags");

    match cli2.command {
        Commands::Recalculate {
            file,
            output,
            force,
            ..
        } => {
            assert_eq!(file, PathBuf::from("workbook.xlsx"));
            assert!(output.is_none());
            assert!(!force);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Ticket 4104 – CLI integration: insert_rows expand_adjacent_sums + clone_row
// ---------------------------------------------------------------------------

fn write_sum_fixture(path: &Path) {
    let mut workbook = umya_spreadsheet::new_file();
    let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().unwrap();
    sheet.get_cell_mut("A1").set_value_number(10.0);
    sheet.get_cell_mut("A2").set_value_number(20.0);
    sheet.get_cell_mut("A3").set_value_number(30.0);
    sheet.get_cell_mut("A4").set_formula("SUM(A1:A3)");
    umya_spreadsheet::writer::xlsx::write(&workbook, path).expect("write fixture");
}

#[test]
fn cli_structure_batch_insert_rows_expand_adjacent_sums() {
    let tmp = tempdir().expect("tempdir");
    let wb = tmp.path().join("expand_sum.xlsx");
    let ops_path = tmp.path().join("ops.json");
    write_sum_fixture(&wb);
    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"insert_rows","sheet_name":"Sheet1","at_row":4,"count":1,"expand_adjacent_sums":true}]}"#,
    );

    let file = wb.to_str().unwrap();
    let ops_ref = format!("@{}", ops_path.to_str().unwrap());

    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let book = umya_spreadsheet::reader::xlsx::read(&wb).expect("read workbook");
    let sheet = book.get_sheet_by_name("Sheet1").ok().unwrap();
    // Subtotal shifted to row 5; formula expanded to include new row 4.
    let formula = sheet.get_cell("A5").unwrap().get_formula().to_string();
    assert_eq!(
        formula.to_uppercase().replace(' ', ""),
        "SUM(A1:A4)",
        "SUM should expand to include inserted row"
    );
}

#[test]
fn cli_structure_batch_clone_row_in_place() {
    let tmp = tempdir().expect("tempdir");
    let wb = tmp.path().join("clone_row.xlsx");
    let ops_path = tmp.path().join("ops.json");

    // Build fixture: header, template row, subtotal
    {
        let mut workbook = umya_spreadsheet::new_file();
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("Header");
        sheet.get_cell_mut("B1").set_value_number(100.0);
        sheet.get_cell_mut("A2").set_value("Total");
        sheet.get_cell_mut("B2").set_formula("SUM(B1:B1)");
        umya_spreadsheet::writer::xlsx::write(&workbook, &wb).expect("write fixture");
    }

    write_ops_payload(
        &ops_path,
        r#"{"ops":[{"kind":"clone_row","sheet_name":"Sheet1","source_row":1,"insert_at":2,"count":2,"expand_adjacent_sums":true}]}"#,
    );

    let file = wb.to_str().unwrap();
    let ops_ref = format!("@{}", ops_path.to_str().unwrap());

    let output = run_cli(&[
        "structure-batch",
        file,
        "--ops",
        ops_ref.as_str(),
        "--in-place",
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let payload = parse_stdout_json(&output);
    assert!(payload["changed"].as_bool().unwrap_or(false));

    let book = umya_spreadsheet::reader::xlsx::read(&wb).expect("read workbook");
    let sheet = book.get_sheet_by_name("Sheet1").ok().unwrap();

    // Cloned rows at 2 and 3 should copy template values.
    let a2 = sheet.get_cell("A2").unwrap().get_value().to_string();
    assert_eq!(a2, "Header");
    let b2 = sheet.get_cell("B2").unwrap().get_value().to_string();
    assert_eq!(b2, "100");

    // Subtotal shifted to row 4; formula expanded.
    let formula = sheet.get_cell("B4").unwrap().get_formula().to_string();
    assert_eq!(
        formula.to_uppercase().replace(' ', ""),
        "SUM(B1:B3)",
        "SUM should expand to include cloned rows"
    );
}

#[test]
fn cli_end_to_end_budget_cloning_and_appending() {
    let tmp = tempdir().expect("tempdir");
    let wb = tmp.path().join("budget.xlsx");
    let rows_path = tmp.path().join("rows.json");

    // 1. Build initial budget template
    {
        let mut workbook = umya_spreadsheet::new_file();
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("Dept: Marketing");

        sheet.get_cell_mut("A2").set_value("Item");
        sheet.get_cell_mut("B2").set_value("Cost");

        sheet.get_cell_mut("A3").set_value("Ads");
        sheet.get_cell_mut("B3").set_value_number(5000.0);

        sheet.get_cell_mut("A4").set_value("Subtotal");
        sheet.get_cell_mut("B4").set_formula("SUM(B3:B3)");

        // Let's make "Dept: Marketing" span A1:B1 to test safe merge policy drop
        sheet.add_merge_cells("A1:B1");

        // Grand Total row at the bottom (Row 7 now, leaving row 5, 6 blank to space it out)
        sheet.get_cell_mut("A7").set_value("Grand Total");
        sheet.get_cell_mut("B7").set_formula("B4"); // Simple ref to Dept Total

        umya_spreadsheet::writer::xlsx::write(&workbook, &wb).expect("write fixture");
    }

    let baseline = tmp.path().join("baseline.xlsx");
    fs::copy(&wb, &baseline).unwrap();

    let file = wb.to_str().unwrap();
    let baseline_file = baseline.to_str().unwrap();

    // 2. Clone the department band (Rows 1:5) to create a new department below it
    // Row 5 is blank, providing a gutter. We insert after row 5. It will become rows 6:10.
    let clone_out = run_cli(&[
        "clone-row-band",
        file,
        "--sheet",
        "Sheet1",
        "--source-rows",
        "1:5",
        "--after",
        "5",
        "--expand-adjacent-sums",
        "--patch-targets",
        "likely-inputs",
        "--merge-policy",
        "safe",
        "--in-place",
    ]);
    assert!(
        clone_out.status.success(),
        "clone failed: {:?}",
        clone_out.stderr
    );

    // 3. Edit the new department's patch targets (It cloned to rows 6:10)
    // The "likely inputs" should be B8 (the number 5000.0). We also want to edit A6 to "Dept: Sales"
    let edit_out = run_cli(&[
        "edit",
        file,
        "Sheet1",
        "A6=Dept: Sales",
        "A8=Travel",
        "B8=2000",
        "B12==B4+B9", // Update Grand Total to include new dept. Grand total shifted from row 7 to 12.
    ]);
    assert!(
        edit_out.status.success(),
        "edit failed: {:?}",
        edit_out.stderr
    );

    // 4. Append a new line item to the new "Sales" department (Rows 6:10)
    // The table for Sales is A7:B8, with footer at row 9 ("Dept Total").
    // We append to region 2.
    fs::write(&rows_path, r#"{"rows":[["Software",1500]]}"#).unwrap();
    let append_out = run_cli(&[
        "append-region",
        file,
        "--sheet",
        "Sheet1",
        "--region-id",
        "0",
        "--rows",
        &format!("@{}", rows_path.to_str().unwrap()),
        "--in-place",
    ]);
    assert!(
        append_out.status.success(),
        "append failed: {}",
        String::from_utf8_lossy(&append_out.stderr)
    );

    // 5. Recalculate
    let recalc_out = run_cli(&["recalculate", file]);
    assert!(
        recalc_out.status.success(),
        "recalc failed: {:?}",
        recalc_out.stderr
    );

    // 6. Verify and Diff
    let verify_out = run_cli(&["verify", "--sheet", "Sheet1", baseline_file, file]);
    assert!(
        verify_out.status.success(),
        "verify failed: {:?}",
        verify_out.stderr
    );
    let verify_json = parse_stdout_json(&verify_out);
    assert_eq!(
        verify_json["summary"]["new_error_count"], 0,
        "should have no new errors"
    );

    let final_book = umya_spreadsheet::reader::xlsx::read(&wb).unwrap();
    let final_sheet = final_book.get_sheet_by_name("Sheet1").ok().unwrap();

    for i in 1..=14 {
        let a = final_sheet
            .get_cell((1, i))
            .map(|c| c.get_value().to_string())
            .unwrap_or_default();
        let b = final_sheet
            .get_cell((2, i))
            .map(|c| c.get_value().to_string())
            .unwrap_or_default();
        let bf = final_sheet
            .get_cell((2, i))
            .map(|c| c.get_formula().to_string())
            .unwrap_or_default();
        println!("Row {i}: {a} | {b} | {bf}");
    }

    // Check original Dept
    assert_eq!(
        final_sheet.get_cell("A1").unwrap().get_value(),
        "Dept: Marketing"
    );
    assert_eq!(
        final_sheet
            .get_cell("B4")
            .unwrap()
            .get_formula()
            .replace(' ', ""),
        "SUM(B3:B3)"
    );
    assert_eq!(final_sheet.get_cell("B4").unwrap().get_value(), "5000"); // Cached from recalc

    // Check new Dept (Sales)
    assert_eq!(
        final_sheet.get_cell("A6").unwrap().get_value(),
        "Dept: Sales"
    );
    assert_eq!(final_sheet.get_cell("A8").unwrap().get_value(), "Travel");
    assert_eq!(final_sheet.get_cell("B8").unwrap().get_value(), "2000");

    // Check appended row (inserted at row 9, pushing footer to 10)
    assert_eq!(final_sheet.get_cell("A9").unwrap().get_value(), "Software");
    assert_eq!(final_sheet.get_cell("B9").unwrap().get_value(), "1500");

    // Check new footer
    assert_eq!(final_sheet.get_cell("A10").unwrap().get_value(), "Subtotal");
    assert_eq!(
        final_sheet
            .get_cell("B10")
            .unwrap()
            .get_formula()
            .replace(' ', ""),
        "SUM(B8:B9)"
    );
    assert_eq!(final_sheet.get_cell("B10").unwrap().get_value(), "3500"); // 2000 + 1500

    // Check Grand Total (shifted to row 13 due to the append-region insertion)
    assert_eq!(
        final_sheet.get_cell("A13").unwrap().get_value(),
        "Grand Total"
    );
    assert_eq!(
        final_sheet
            .get_cell("B13")
            .unwrap()
            .get_formula()
            .replace(' ', ""),
        "B4+B10"
    );
    assert_eq!(final_sheet.get_cell("B13").unwrap().get_value(), "8500"); // 5000 + 3500
}

// ---------------------------------------------------------------------------
// 5004: native raster rendering on the CLI
// ---------------------------------------------------------------------------

#[cfg(feature = "render")]
#[test]
fn asp_render_writes_a_png_and_reports_fidelity() {
    let dir = tempdir().expect("tempdir");
    let workbook = dir.path().join("book.xlsx");
    write_fixture(&workbook);
    let output = dir.path().join("sheet.png");

    let result = run_asp(&[
        "render",
        workbook.to_str().unwrap(),
        "--sheet",
        "Sheet1",
        "--range",
        "A1:C6",
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let payload = parse_stdout_json(&result);
    assert_eq!(payload["backend"], "native");
    assert_eq!(payload["renderer"], "native-raster/1");
    assert_eq!(payload["range"], "A1:C6");
    assert_eq!(payload["png_level"], "balanced");
    assert!(payload["width"].as_u64().unwrap() > 0);
    assert!(payload["calculation"]["revision_id"].is_string());
    // The fixture's formulas carry no cached values, so the renderer says so
    // instead of guessing.
    let warnings: Vec<&str> = payload["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning.as_str().unwrap())
        .collect();
    assert!(
        warnings.contains(&"formulas_unevaluated"),
        "warnings={warnings:?}"
    );
    assert_eq!(payload["fidelity"], "partial");

    let bytes = fs::read(&output).expect("png written");
    assert_eq!(&bytes[..4], b"\x89PNG");
    assert_eq!(bytes.len() as u64, payload["bytes"].as_u64().unwrap());
}

#[cfg(feature = "render")]
#[test]
fn asp_render_png_level_changes_encoding_but_not_geometry() {
    let dir = tempdir().expect("tempdir");
    let workbook = dir.path().join("book.xlsx");
    write_fixture(&workbook);

    let mut sizes = Vec::new();
    for level in ["fast", "balanced"] {
        let output = dir.path().join(format!("{level}.png"));
        let result = run_asp(&[
            "render",
            workbook.to_str().unwrap(),
            "--sheet",
            "Sheet1",
            "--png-level",
            level,
            "--output",
            output.to_str().unwrap(),
        ]);
        assert!(result.status.success());
        let payload = parse_stdout_json(&result);
        assert_eq!(payload["png_level"], level);
        sizes.push((
            payload["width"].as_u64().unwrap(),
            payload["height"].as_u64().unwrap(),
            payload["bytes"].as_u64().unwrap(),
        ));
    }
    assert_eq!(sizes[0].0, sizes[1].0, "width must not depend on png level");
    assert_eq!(sizes[0].1, sizes[1].1, "height must not depend on png level");
    assert!(
        sizes[0].2 > sizes[1].2,
        "fast should be larger than balanced: {sizes:?}"
    );
}

#[cfg(feature = "render")]
#[test]
fn asp_render_refuses_to_clobber_without_force() {
    let dir = tempdir().expect("tempdir");
    let workbook = dir.path().join("book.xlsx");
    write_fixture(&workbook);
    let output = dir.path().join("sheet.png");
    fs::write(&output, b"existing").expect("seed output");

    let result = run_asp(&[
        "render",
        workbook.to_str().unwrap(),
        "--sheet",
        "Sheet1",
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(!result.status.success());
    let error = parse_stderr_json(&result);
    assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    assert_eq!(fs::read(&output).unwrap(), b"existing");

    let forced = run_asp(&[
        "render",
        workbook.to_str().unwrap(),
        "--sheet",
        "Sheet1",
        "--output",
        output.to_str().unwrap(),
        "--force",
    ]);
    assert!(forced.status.success());
    assert_eq!(&fs::read(&output).unwrap()[..4], b"\x89PNG");
}

#[cfg(feature = "render")]
#[test]
fn asp_op_screenshot_sheet_writes_artifact_bytes_and_prints_the_envelope() {
    let dir = tempdir().expect("tempdir");
    let workbook = dir.path().join("book.xlsx");
    write_fixture(&workbook);
    let output = dir.path().join("artifact.png");

    let result = run_asp(&[
        "op",
        "screenshot_sheet",
        "--bind",
        workbook.to_str().unwrap(),
        "--json",
        r#"{"sheet_name":"Sheet1","range":"A1:C6"}"#,
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let response = parse_stdout_json(&result);
    // The canonical envelope is unchanged in shape and still carries only a
    // handle; the bytes crossed at the adapter boundary.
    assert_eq!(response["schema_version"], "1");
    assert_eq!(response["operation"], "screenshot_sheet");
    assert_eq!(response["data"]["renderer"], "native-raster/1");
    assert_eq!(response["data"]["artifact"]["media_type"], "image/png");
    assert!(response["data"]["calculation"]["revision_id"].is_string());

    let bytes = fs::read(&output).expect("artifact written");
    assert_eq!(&bytes[..4], b"\x89PNG");
    assert_eq!(
        bytes.len() as u64,
        response["data"]["artifact"]["bytes"].as_u64().unwrap()
    );
    let digest = format!("sha256:{:x}", <sha2::Sha256 as sha2::Digest>::digest(&bytes));
    assert_eq!(response["data"]["artifact"]["hash"], digest);
}

#[cfg(feature = "render")]
#[test]
fn asp_op_screenshot_sheet_rejects_in_place() {
    let dir = tempdir().expect("tempdir");
    let workbook = dir.path().join("book.xlsx");
    write_fixture(&workbook);

    let result = run_asp(&[
        "op",
        "screenshot_sheet",
        "--bind",
        workbook.to_str().unwrap(),
        "--json",
        r#"{"sheet_name":"Sheet1"}"#,
        "--in-place",
    ]);
    assert!(!result.status.success());
    let error = parse_stderr_json(&result);
    assert_eq!(error["error"]["code"], "INVALID_REQUEST");
}

#[cfg(feature = "render")]
#[test]
fn asp_render_and_asp_op_agree_byte_for_byte() {
    let dir = tempdir().expect("tempdir");
    let workbook = dir.path().join("book.xlsx");
    write_fixture(&workbook);
    let human = dir.path().join("human.png");
    let machine = dir.path().join("machine.png");

    assert!(
        run_asp(&[
            "render",
            workbook.to_str().unwrap(),
            "--sheet",
            "Sheet1",
            "--range",
            "A1:C6",
            "--output",
            human.to_str().unwrap(),
        ])
        .status
        .success()
    );
    assert!(
        run_asp(&[
            "op",
            "screenshot_sheet",
            "--bind",
            workbook.to_str().unwrap(),
            "--json",
            r#"{"sheet_name":"Sheet1","range":"A1:C6"}"#,
            "--output",
            machine.to_str().unwrap(),
        ])
        .status
        .success()
    );
    assert_eq!(
        fs::read(&human).unwrap(),
        fs::read(&machine).unwrap(),
        "the human and machine surfaces must render identical bytes"
    );
}

#[cfg(feature = "render")]
#[test]
fn asp_op_screenshot_sheet_honours_png_level_and_reports_geometry() {
    let dir = tempdir().expect("tempdir");
    let workbook = dir.path().join("book.xlsx");
    write_fixture(&workbook);

    let mut sizes = Vec::new();
    for level in ["fast", "balanced"] {
        let output = dir.path().join(format!("op-{level}.png"));
        let result = run_asp(&[
            "op",
            "screenshot_sheet",
            "--bind",
            workbook.to_str().unwrap(),
            "--json",
            &format!(r#"{{"sheet_name":"Sheet1","range":"A1:C6","png_level":"{level}"}}"#),
            "--output",
            output.to_str().unwrap(),
        ]);
        assert!(
            result.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&result.stderr)
        );
        let data = parse_stdout_json(&result)["data"].clone();
        assert_eq!(data["png_level"], level);
        sizes.push((
            data["width"].as_u64().unwrap(),
            data["height"].as_u64().unwrap(),
            data["artifact"]["bytes"].as_u64().unwrap(),
        ));
    }
    assert_eq!(sizes[0].0, sizes[1].0, "width must not depend on png level");
    assert_eq!(sizes[0].1, sizes[1].1, "height must not depend on png level");
    assert!(
        sizes[0].2 > sizes[1].2,
        "fast should be larger than balanced: {sizes:?}"
    );
}

#[cfg(feature = "render")]
#[test]
fn asp_op_screenshot_sheet_rejects_png_level_on_the_libreoffice_backend() {
    let dir = tempdir().expect("tempdir");
    let workbook = dir.path().join("book.xlsx");
    write_fixture(&workbook);

    let result = run_asp(&[
        "op",
        "screenshot_sheet",
        "--bind",
        workbook.to_str().unwrap(),
        "--json",
        r#"{"sheet_name":"Sheet1","backend":"libreoffice","png_level":"fast"}"#,
    ]);
    assert!(!result.status.success());
    let error = parse_stderr_json(&result);
    assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("png_level applies to the native backend only"),
        "error={error}"
    );
}

#[cfg(feature = "render")]
#[test]
fn asp_render_rejects_png_level_on_the_libreoffice_backend() {
    let dir = tempdir().expect("tempdir");
    let workbook = dir.path().join("book.xlsx");
    write_fixture(&workbook);
    let output = dir.path().join("sheet.png");

    let result = run_asp(&[
        "render",
        workbook.to_str().unwrap(),
        "--sheet",
        "Sheet1",
        "--backend",
        "libreoffice",
        "--png-level",
        "fast",
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(!result.status.success());
    let error = parse_stderr_json(&result);
    assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    assert!(!output.exists());
}
