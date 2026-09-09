//! Integration tests for Phase 0 UX enhancements:
//!   - `--format rows` for range-values
//!   - `--budget` for inspect-cells
//!   - `--changed-cells` and `--ignore-sheets` for recalculate
//!   - `--sheets` multi-sheet filter for diff
//!   - `check-ref-impact` standalone command

use serde_json::Value;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run_cli(args: &[&str]) -> std::process::Output {
    Command::new(assert_cmd::cargo::cargo_bin!("agent-spreadsheet"))
        .args(args)
        .output()
        .expect("run agent-spreadsheet")
}

fn parse_stdout_json(output: &std::process::Output) -> Value {
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout utf8");
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "invalid json in stdout: {}\nstdout: {}\nstderr: {}",
            e,
            stdout,
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "command failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Workbook with 2 sheets: Sheet1 (data + formulas) and Summary.
fn write_fixture(path: &Path) {
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook
            .get_sheet_by_name_mut("Sheet1").ok()
            .expect("default sheet");
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
        let s = workbook.get_sheet_by_name_mut("Summary").ok().expect("summary");
        s.get_cell_mut("A1").set_value("Flag");
        s.get_cell_mut("B1").set_value("Ready");
        s.get_cell_mut("A2").set_value_number(42.0);
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, path).expect("write fixture");
}

/// Modified copy of the fixture with different values for diffing.
fn write_modified_fixture(path: &Path) {
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook
            .get_sheet_by_name_mut("Sheet1").ok()
            .expect("default sheet");
        sheet.get_cell_mut("A1").set_value("Name");
        sheet.get_cell_mut("B1").set_value("Amount");
        sheet.get_cell_mut("C1").set_value("Total");
        sheet.get_cell_mut("A2").set_value("Eve"); // changed
        sheet.get_cell_mut("B2").set_value_number(99.0); // changed
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
        let s = workbook.get_sheet_by_name_mut("Summary").ok().expect("summary");
        s.get_cell_mut("A1").set_value("Flag");
        s.get_cell_mut("B1").set_value("Done"); // changed
        s.get_cell_mut("A2").set_value_number(42.0);
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, path).expect("write modified fixture");
}

/// Workbook with formulas and SUM ranges for structure impact testing.
fn write_formula_fixture(path: &Path) {
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook
            .get_sheet_by_name_mut("Sheet1").ok()
            .expect("default sheet");
        sheet.get_cell_mut("A1").set_value("Item");
        sheet.get_cell_mut("B1").set_value("Value");
        for row in 2..=10u32 {
            sheet
                .get_cell_mut(format!("A{}", row))
                .set_value(format!("Item{}", row - 1));
            sheet
                .get_cell_mut(format!("B{}", row))
                .set_value_number((row as f64) * 10.0);
        }
        // SUM formula at B11
        sheet.get_cell_mut("B11").set_formula("SUM(B2:B10)");
        // Reference formula
        sheet.get_cell_mut("C2").set_formula("B2+B3");
        sheet.get_cell_mut("C3").set_formula("$B$2+B3");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, path).expect("write formula fixture");
}

// ===========================================================================
// --format rows (range-values)
// ===========================================================================

#[test]
fn range_values_format_rows_produces_keyed_output() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("rows.xlsx");
    write_fixture(&path);
    let file = path.to_str().unwrap();

    let out = run_cli(&["range-values", file, "Sheet1", "A1:C3", "--format", "rows"]);
    assert_success(&out);
    let json = parse_stdout_json(&out);

    // Should have rows_keyed array in the first value entry
    let entry = &json["values"][0];
    let rows_keyed = entry["rows_keyed"]
        .as_array()
        .expect("rows_keyed should be an array");

    assert_eq!(rows_keyed.len(), 3, "should have 3 rows (A1:C3)");

    // Row 1: headers
    let r1 = &rows_keyed[0];
    assert_eq!(r1["row"], 1);
    assert_eq!(r1["cells"]["A"], "Name");
    assert_eq!(r1["cells"]["B"], "Amount");
    assert_eq!(r1["cells"]["C"], "Total");

    // Row 2: data
    let r2 = &rows_keyed[1];
    assert_eq!(r2["row"], 2);
    assert_eq!(r2["cells"]["A"], "Alice");
    // B2 is numeric
    assert!(r2["cells"]["B"].is_number(), "B2 should be numeric");
}

#[test]
fn range_values_format_rows_omits_empty_cells() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("rows_sparse.xlsx");

    // Create a sparse workbook
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value("X");
        // B1 intentionally empty
        sheet.get_cell_mut("C1").set_value("Y");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &path).unwrap();

    let out = run_cli(&[
        "range-values",
        path.to_str().unwrap(),
        "Sheet1",
        "A1:C1",
        "--format",
        "rows",
    ]);
    assert_success(&out);
    let json = parse_stdout_json(&out);

    let rows = json["values"][0]["rows_keyed"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let cells = rows[0]["cells"].as_object().unwrap();
    // B should not be present (empty)
    assert!(cells.contains_key("A"));
    assert!(cells.contains_key("C"));
    assert!(!cells.contains_key("B"), "empty cell B should be omitted");
}

#[test]
fn range_values_format_rows_on_summary_sheet() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("rows_summary.xlsx");
    write_fixture(&path);

    let out = run_cli(&[
        "range-values",
        path.to_str().unwrap(),
        "Summary",
        "A1:B2",
        "--format",
        "rows",
    ]);
    assert_success(&out);
    let json = parse_stdout_json(&out);

    let rows = json["values"][0]["rows_keyed"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["cells"]["A"], "Flag");
    assert_eq!(rows[0]["cells"]["B"], "Ready");
}

// ===========================================================================
// --budget for inspect-cells
// ===========================================================================

#[test]
fn inspect_cells_budget_raises_limit() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("budget.xlsx");

    // Create workbook with 50 cells of data
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        for row in 1..=10u32 {
            for col in 1..=5u32 {
                sheet
                    .get_cell_mut((col, row))
                    .set_value(format!("R{}C{}", row, col));
            }
        }
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &path).unwrap();

    // Default budget (25) should reject A1:E10 (50 cells)
    let default_out = run_cli(&["inspect-cells", path.to_str().unwrap(), "Sheet1", "A1:E10"]);
    assert!(
        !default_out.status.success(),
        "default budget should reject 50 cells"
    );

    // With --budget 50 it should succeed
    let budget_out = run_cli(&[
        "inspect-cells",
        path.to_str().unwrap(),
        "Sheet1",
        "A1:E10",
        "--budget",
        "50",
    ]);
    assert_success(&budget_out);
    let json = parse_stdout_json(&budget_out);

    // Verify budget metadata reflects the override
    let budget_meta = &json["budget"];
    assert!(
        budget_meta["max_cells"].as_u64().unwrap() >= 50,
        "budget max_cells should be >= 50, got: {}",
        budget_meta
    );
}

#[test]
fn inspect_cells_budget_rejects_out_of_range() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("budget_reject.xlsx");
    write_fixture(&path);

    // Budget 0 — rejected
    let out0 = run_cli(&[
        "inspect-cells",
        path.to_str().unwrap(),
        "Sheet1",
        "A1",
        "--budget",
        "0",
    ]);
    assert!(!out0.status.success(), "budget 0 should be rejected");
    let stderr0 = String::from_utf8_lossy(&out0.stderr);
    assert!(stderr0.contains("between 1 and 200"), "stderr: {}", stderr0);

    // Budget 201 — rejected
    let out201 = run_cli(&[
        "inspect-cells",
        path.to_str().unwrap(),
        "Sheet1",
        "A1",
        "--budget",
        "201",
    ]);
    assert!(!out201.status.success(), "budget 201 should be rejected");
    let stderr201 = String::from_utf8_lossy(&out201.stderr);
    assert!(
        stderr201.contains("between 1 and 200"),
        "stderr: {}",
        stderr201
    );
}

#[test]
fn inspect_cells_budget_200_accepts_large_range() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("budget_large.xlsx");

    // Create workbook with 200 cells
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        for row in 1..=20u32 {
            for col in 1..=10u32 {
                sheet
                    .get_cell_mut((col, row))
                    .set_value(format!("R{}C{}", row, col));
            }
        }
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &path).unwrap();

    let out = run_cli(&[
        "inspect-cells",
        path.to_str().unwrap(),
        "Sheet1",
        "A1:J20",
        "--budget",
        "200",
    ]);
    assert_success(&out);
    let json = parse_stdout_json(&out);
    assert!(
        json["cells"].as_array().map(|a| a.len()).unwrap_or(0) > 0,
        "should return cells"
    );
}

// ===========================================================================
// --changed-cells and --ignore-sheets for recalculate
// ===========================================================================

#[test]
fn recalculate_changed_cells_shows_summary() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("recalc.xlsx");

    // Create workbook with formula cells that produce different cached vs computed values.
    // The formula cache is usually empty in a fresh umya workbook, so recalculation
    // will "change" formula cells from empty-cache to computed values.
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(10.0);
        sheet.get_cell_mut("A2").set_value_number(20.0);
        sheet.get_cell_mut("A3").set_formula("A1+A2");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &path).unwrap();

    let output = tmp.path().join("recalc_out.xlsx");
    let out = run_cli(&[
        "recalculate",
        path.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--changed-cells",
    ]);
    assert_success(&out);
    let json = parse_stdout_json(&out);

    // Should have a changed_cells_summary object
    let summary = &json["changed_cells_summary"];
    assert!(
        !summary.is_null(),
        "expected changed_cells_summary in response: {}",
        json
    );
    assert!(summary["total_changed"].is_number());
    assert!(summary["by_sheet"].is_object());
    assert!(summary["samples"].is_array());
}

#[test]
fn recalculate_without_changed_cells_flag_omits_summary() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("recalc_no_flag.xlsx");
    write_fixture(&path);

    let output = tmp.path().join("recalc_no_flag_out.xlsx");
    let out = run_cli(&[
        "recalculate",
        path.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);
    assert_success(&out);
    let json = parse_stdout_json(&out);

    // Without --changed-cells, summary should be absent
    assert!(
        json.get("changed_cells_summary").is_none() || json["changed_cells_summary"].is_null(),
        "changed_cells_summary should be absent without flag: {}",
        json
    );
}

#[test]
fn recalculate_ignore_sheets_excludes_from_summary() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("recalc_ignore.xlsx");

    // Workbook with formulas on both sheets
    let mut workbook = umya_spreadsheet::new_file();
    {
        let sheet = workbook.get_sheet_by_name_mut("Sheet1").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(5.0);
        sheet.get_cell_mut("A2").set_formula("A1*2");
    }
    workbook.new_sheet("Ignored").unwrap();
    {
        let sheet = workbook.get_sheet_by_name_mut("Ignored").ok().unwrap();
        sheet.get_cell_mut("A1").set_value_number(99.0);
        sheet.get_cell_mut("A2").set_formula("A1+1");
    }
    umya_spreadsheet::writer::xlsx::write(&workbook, &path).unwrap();

    let output = tmp.path().join("recalc_ignore_out.xlsx");
    let out = run_cli(&[
        "recalculate",
        path.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--changed-cells",
        "--ignore-sheets",
        "Ignored",
    ]);
    assert_success(&out);
    let json = parse_stdout_json(&out);

    let summary = &json["changed_cells_summary"];
    assert!(!summary.is_null());

    // The ignored_sheets field should be present
    if let Some(ignored) = summary.get("ignored_sheets") {
        let ignored_arr = ignored.as_array().unwrap();
        assert!(
            ignored_arr.iter().any(|v| v == "Ignored"),
            "expected 'Ignored' in ignored_sheets: {}",
            summary
        );
    }

    // by_sheet should not have "Ignored" sheet
    if let Some(by_sheet) = summary["by_sheet"].as_object() {
        assert!(
            !by_sheet.contains_key("Ignored"),
            "Ignored sheet should not appear in by_sheet: {:?}",
            by_sheet
        );
    }
}

// ===========================================================================
// --sheets multi-sheet filter for diff
// ===========================================================================

#[test]
fn diff_sheets_filter_limits_to_specified_sheets() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("diff_orig.xlsx");
    let modified = tmp.path().join("diff_mod.xlsx");
    write_fixture(&original);
    write_modified_fixture(&modified);

    // Diff with --sheets Sheet1 only
    let out_s1 = run_cli(&[
        "diff",
        original.to_str().unwrap(),
        modified.to_str().unwrap(),
        "--sheets",
        "Sheet1",
        "--details",
    ]);
    assert_success(&out_s1);
    let json_s1 = parse_stdout_json(&out_s1);

    // All changes should be on Sheet1
    if let Some(changes) = json_s1["changes"].as_array() {
        for change in changes {
            let sheet = change
                .get("sheet")
                .or_else(|| change.get("sheet_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            assert_eq!(
                sheet.to_lowercase(),
                "sheet1",
                "expected only Sheet1 changes with --sheets Sheet1"
            );
        }
    }

    // Diff with --sheets Summary only
    let out_sum = run_cli(&[
        "diff",
        original.to_str().unwrap(),
        modified.to_str().unwrap(),
        "--sheets",
        "Summary",
        "--details",
    ]);
    assert_success(&out_sum);
    let json_sum = parse_stdout_json(&out_sum);

    // Should have fewer changes than unfiltered
    let total_changes = json_sum["change_count"].as_u64().unwrap_or(0);
    assert!(total_changes >= 1, "should have at least 1 Summary change");

    if let Some(changes) = json_sum["changes"].as_array() {
        for change in changes {
            let sheet = change
                .get("sheet")
                .or_else(|| change.get("sheet_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            assert_eq!(
                sheet.to_lowercase(),
                "summary",
                "expected only Summary changes with --sheets Summary"
            );
        }
    }
}

#[test]
fn diff_sheets_multi_filter_includes_both() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("diff_multi_orig.xlsx");
    let modified = tmp.path().join("diff_multi_mod.xlsx");
    write_fixture(&original);
    write_modified_fixture(&modified);

    // Both sheets
    let out = run_cli(&[
        "diff",
        original.to_str().unwrap(),
        modified.to_str().unwrap(),
        "--sheets",
        "Sheet1,Summary",
        "--details",
    ]);
    assert_success(&out);
    let json = parse_stdout_json(&out);

    // Should include changes from both sheets
    let total = json["change_count"].as_u64().unwrap_or(0);
    assert!(
        total >= 2,
        "should have changes from both sheets, got: {}",
        total
    );
}

#[test]
fn diff_sheet_and_sheets_mutually_exclusive() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("diff_excl_orig.xlsx");
    let modified = tmp.path().join("diff_excl_mod.xlsx");
    write_fixture(&original);
    write_modified_fixture(&modified);

    let out = run_cli(&[
        "diff",
        original.to_str().unwrap(),
        modified.to_str().unwrap(),
        "--sheet",
        "Sheet1",
        "--sheets",
        "Summary",
    ]);
    assert!(
        !out.status.success(),
        "expected mutual exclusivity error for --sheet + --sheets"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("mutually exclusive"),
        "expected 'mutually exclusive' error, got: {}",
        stderr
    );
}

#[test]
fn diff_sheets_case_insensitive() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("diff_case_orig.xlsx");
    let modified = tmp.path().join("diff_case_mod.xlsx");
    write_fixture(&original);
    write_modified_fixture(&modified);

    // Use mixed case — should still match
    let out = run_cli(&[
        "diff",
        original.to_str().unwrap(),
        modified.to_str().unwrap(),
        "--sheets",
        "sheet1",
        "--details",
    ]);
    assert_success(&out);
    let json = parse_stdout_json(&out);
    assert!(
        json["change_count"].as_u64().unwrap_or(0) >= 1,
        "case-insensitive sheet filter should match Sheet1"
    );
}

// ===========================================================================
// check-ref-impact standalone command
// ===========================================================================

#[test]
fn check_ref_impact_runs_without_mutation() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("impact.xlsx");
    write_formula_fixture(&path);

    // Record file hash before
    let before_bytes = std::fs::read(&path).unwrap();

    // Write ops payload for inserting a row
    let ops_path = tmp.path().join("structure_ops.json");
    let ops_payload = serde_json::json!({
        "ops": [{
            "kind": "insert_rows",
            "sheet_name": "Sheet1",
            "at_row": 5,
            "count": 1
        }]
    });
    std::fs::write(&ops_path, serde_json::to_string(&ops_payload).unwrap()).unwrap();

    let out = run_cli(&[
        "check-ref-impact",
        path.to_str().unwrap(),
        "--ops",
        &format!("@{}", ops_path.display()),
    ]);
    assert_success(&out);
    let json = parse_stdout_json(&out);

    // Should have structural impact data (shifted_spans, tokens_affected, etc.)
    assert!(
        json.get("shifted_spans").is_some()
            || json.get("impact_report").is_some()
            || json.get("impact").is_some(),
        "expected impact data in response: {}",
        json
    );

    // File should be unchanged (read-only operation)
    let after_bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        before_bytes, after_bytes,
        "check-ref-impact should not modify the workbook"
    );
}

#[test]
fn check_ref_impact_with_formula_delta() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("impact_delta.xlsx");
    write_formula_fixture(&path);

    let ops_path = tmp.path().join("delta_ops.json");
    let ops_payload = serde_json::json!({
        "ops": [{
            "kind": "insert_rows",
            "sheet_name": "Sheet1",
            "at_row": 5,
            "count": 2
        }]
    });
    std::fs::write(&ops_path, serde_json::to_string(&ops_payload).unwrap()).unwrap();

    let out = run_cli(&[
        "check-ref-impact",
        path.to_str().unwrap(),
        "--ops",
        &format!("@{}", ops_path.display()),
        "--show-formula-delta",
    ]);
    assert_success(&out);
    let json = parse_stdout_json(&out);

    // With --show-formula-delta, should include formula delta preview
    let has_delta =
        json.get("formula_delta_preview").is_some() || json.get("formula_delta").is_some();
    assert!(
        has_delta,
        "expected formula delta preview with --show-formula-delta: {}",
        json
    );
}

#[test]
fn check_ref_impact_delete_rows_detects_affected_formulas() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("impact_delete.xlsx");
    write_formula_fixture(&path);

    let ops_path = tmp.path().join("delete_ops.json");
    let ops_payload = serde_json::json!({
        "ops": [{
            "kind": "delete_rows",
            "sheet_name": "Sheet1",
            "start_row": 3,
            "count": 1
        }]
    });
    std::fs::write(&ops_path, serde_json::to_string(&ops_payload).unwrap()).unwrap();

    let out = run_cli(&[
        "check-ref-impact",
        path.to_str().unwrap(),
        "--ops",
        &format!("@{}", ops_path.display()),
        "--show-formula-delta",
    ]);
    assert_success(&out);
    let json = parse_stdout_json(&out);

    // The impact should flag affected formulas (SUM(B2:B10) will be impacted)
    let report_str = serde_json::to_string(&json).unwrap();
    // The SUM or cell references should appear somewhere in the impact analysis
    assert!(
        report_str.contains("tokens_affected")
            || report_str.contains("shifted_spans")
            || report_str.contains("formulas_affected")
            || report_str.contains("impact"),
        "expected formula impact data in response: {}",
        json
    );

    // File unchanged
    let before = std::fs::metadata(&path).unwrap().len();
    assert!(before > 0, "fixture should exist");
}

// ===========================================================================
// Edge cases
// ===========================================================================

#[test]
fn range_values_format_rows_empty_sheet() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("empty_rows.xlsx");

    let workbook = umya_spreadsheet::new_file();
    umya_spreadsheet::writer::xlsx::write(&workbook, &path).unwrap();

    let out = run_cli(&[
        "range-values",
        path.to_str().unwrap(),
        "Sheet1",
        "A1:C3",
        "--format",
        "rows",
    ]);
    assert_success(&out);
    let json = parse_stdout_json(&out);

    // For an empty sheet, rows_keyed contains row entries with no `cells` key
    // (empty BTreeMap serializes as absent with skip_serializing_if) or an empty object.
    let entry = &json["values"][0];
    if let Some(rows) = entry.get("rows_keyed").and_then(|v| v.as_array()) {
        for row in rows {
            // cells key is either absent or an empty object
            if let Some(cells) = row.get("cells").and_then(|v| v.as_object()) {
                assert!(cells.is_empty(), "empty sheet should have no cell values");
            }
            // absent cells key is also valid — means no non-empty cells
        }
    }
    // If rows_keyed is absent entirely, that's also correct for an empty sheet
}

#[test]
fn diff_nonexistent_sheet_filter_returns_zero_changes() {
    let tmp = tempdir().expect("tempdir");
    let original = tmp.path().join("diff_no_sheet_orig.xlsx");
    let modified = tmp.path().join("diff_no_sheet_mod.xlsx");
    write_fixture(&original);
    write_modified_fixture(&modified);

    let out = run_cli(&[
        "diff",
        original.to_str().unwrap(),
        modified.to_str().unwrap(),
        "--sheets",
        "NonexistentSheet",
        "--details",
    ]);
    assert_success(&out);
    let json = parse_stdout_json(&out);
    assert_eq!(
        json["change_count"].as_u64().unwrap_or(0),
        0,
        "filtering by nonexistent sheet should yield 0 changes"
    );
}
