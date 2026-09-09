//! Unit and integration tests for the replace-in-formulas CLI command.

#![cfg(feature = "recalc")]

use anyhow::Result;
use serde_json::Value;
use agent_spreadsheet::model::FormulaParsePolicy;
use std::path::PathBuf;
use tempfile::tempdir;

mod support;

fn create_formula_workbook(workspace: &support::TestWorkspace, name: &str) -> PathBuf {
    workspace.create_workbook(name, |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        // Row 1: headers
        sheet.cell_mut("A1").set_value("Label");
        sheet.cell_mut("B1").set_value("Value");
        // Row 2: formula cells
        sheet.cell_mut("A2").set_value("Sum");
        sheet
            .cell_mut("B2")
            .set_formula("SUM(C2:C10)".to_string());
        // Row 3: another formula cell
        sheet.cell_mut("A3").set_value("Avg");
        sheet
            .cell_mut("B3")
            .set_formula("AVERAGE(C2:C10)".to_string());
        // Row 4: formula referencing Sheet1
        sheet.cell_mut("A4").set_value("Ref");
        sheet
            .cell_mut("B4")
            .set_formula("Sheet1!D5+Sheet1!D6".to_string());
        // Row 5: literal value (should NOT be touched)
        sheet.cell_mut("A5").set_value("Literal");
        sheet.cell_mut("B5").set_value("SUM(C2:C10)");
    })
}

// ── Core unit tests ──────────────────────────────────────────────────────────

#[test]
fn replace_plain_text_in_formula_body() {
    use agent_spreadsheet::tools::fork::{ReplaceInFormulasOp, apply_replace_in_formulas_to_file};

    let workspace = support::TestWorkspace::new();
    let path = create_formula_workbook(&workspace, "plain.xlsx");

    // Copy to temp file for mutation
    let tmp = tempdir().unwrap();
    let work = tmp.path().join("plain.xlsx");
    std::fs::copy(&path, &work).unwrap();

    let op = ReplaceInFormulasOp {
        sheet_name: "Sheet1".to_string(),
        find: "C2:C10".to_string(),
        replace: "D2:D20".to_string(),
        range: None,
        regex: false,
        case_sensitive: true,
    };

    let result = apply_replace_in_formulas_to_file(&work, &op, FormulaParsePolicy::Off).unwrap();

    assert_eq!(
        result.formulas_changed, 2,
        "SUM and AVERAGE both reference C2:C10"
    );
    assert!(result.formulas_checked >= 3, "at least 3 formula cells");
    assert!(!result.samples.is_empty());

    // Verify the formulas were updated
    let book = umya_spreadsheet::reader::xlsx::read(&work).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();

    let b2 = sheet.cell("B2").unwrap();
    assert_eq!(b2.formula(), "SUM(D2:D20)");

    let b3 = sheet.cell("B3").unwrap();
    assert_eq!(b3.formula(), "AVERAGE(D2:D20)");

    // B5 is a literal value, should NOT be changed
    let b5 = sheet.cell("B5").unwrap();
    assert_eq!(b5.value(), "SUM(C2:C10)");
}

#[test]
fn replace_regex_mode() {
    use agent_spreadsheet::tools::fork::{ReplaceInFormulasOp, apply_replace_in_formulas_to_file};

    let workspace = support::TestWorkspace::new();
    let path = create_formula_workbook(&workspace, "regex.xlsx");

    let tmp = tempdir().unwrap();
    let work = tmp.path().join("regex.xlsx");
    std::fs::copy(&path, &work).unwrap();

    let op = ReplaceInFormulasOp {
        sheet_name: "Sheet1".to_string(),
        find: r"Sheet1!D(\d+)".to_string(),
        replace: "Sheet2!E$1".to_string(),
        range: None,
        regex: true,
        case_sensitive: true,
    };

    let result = apply_replace_in_formulas_to_file(&work, &op, FormulaParsePolicy::Off).unwrap();

    assert_eq!(result.formulas_changed, 1, "only B4 references Sheet1!D");

    let book = umya_spreadsheet::reader::xlsx::read(&work).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    let b4 = sheet.cell("B4").unwrap();
    assert_eq!(b4.formula(), "Sheet2!E5+Sheet2!E6");
}

#[test]
fn no_op_when_pattern_absent() {
    use agent_spreadsheet::tools::fork::{ReplaceInFormulasOp, apply_replace_in_formulas_to_file};

    let workspace = support::TestWorkspace::new();
    let path = create_formula_workbook(&workspace, "noop.xlsx");

    let tmp = tempdir().unwrap();
    let work = tmp.path().join("noop.xlsx");
    std::fs::copy(&path, &work).unwrap();

    let op = ReplaceInFormulasOp {
        sheet_name: "Sheet1".to_string(),
        find: "NONEXISTENT_FUNCTION".to_string(),
        replace: "REPLACEMENT".to_string(),
        range: None,
        regex: false,
        case_sensitive: true,
    };

    let result = apply_replace_in_formulas_to_file(&work, &op, FormulaParsePolicy::Off).unwrap();

    assert_eq!(result.formulas_changed, 0);
    assert!(
        result
            .warnings
            .iter()
            .any(|w: &String| w.contains("WARN_NO_MATCH"))
    );
}

#[test]
fn range_scoped_replace_touches_only_target_area() {
    use agent_spreadsheet::tools::fork::{ReplaceInFormulasOp, apply_replace_in_formulas_to_file};

    let workspace = support::TestWorkspace::new();
    let path = create_formula_workbook(&workspace, "scoped.xlsx");

    let tmp = tempdir().unwrap();
    let work = tmp.path().join("scoped.xlsx");
    std::fs::copy(&path, &work).unwrap();

    // Only target B2 (not B3 or B4)
    let op = ReplaceInFormulasOp {
        sheet_name: "Sheet1".to_string(),
        find: "C2:C10".to_string(),
        replace: "X1:X5".to_string(),
        range: Some("B2:B2".to_string()),
        regex: false,
        case_sensitive: true,
    };

    let result = apply_replace_in_formulas_to_file(&work, &op, FormulaParsePolicy::Off).unwrap();

    assert_eq!(result.formulas_changed, 1, "only B2 is in the range");

    let book = umya_spreadsheet::reader::xlsx::read(&work).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();

    // B2 changed
    assert_eq!(sheet.cell("B2").unwrap().formula(), "SUM(X1:X5)");
    // B3 unchanged (outside range)
    assert_eq!(
        sheet.cell("B3").unwrap().formula(),
        "AVERAGE(C2:C10)"
    );
}

#[test]
fn case_insensitive_plain_text_replace() {
    use agent_spreadsheet::tools::fork::{ReplaceInFormulasOp, apply_replace_in_formulas_to_file};

    let workspace = support::TestWorkspace::new();
    let path = create_formula_workbook(&workspace, "case.xlsx");

    let tmp = tempdir().unwrap();
    let work = tmp.path().join("case.xlsx");
    std::fs::copy(&path, &work).unwrap();

    let op = ReplaceInFormulasOp {
        sheet_name: "Sheet1".to_string(),
        find: "sum".to_string(),
        replace: "SUMPRODUCT".to_string(),
        range: None,
        regex: false,
        case_sensitive: false,
    };

    let result = apply_replace_in_formulas_to_file(&work, &op, FormulaParsePolicy::Off).unwrap();

    assert_eq!(
        result.formulas_changed, 1,
        "SUM matches 'sum' case-insensitively"
    );

    let book = umya_spreadsheet::reader::xlsx::read(&work).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    assert_eq!(
        sheet.cell("B2").unwrap().formula(),
        "SUMPRODUCT(C2:C10)"
    );
}

#[test]
fn parse_policy_fail_validates_all_replacements_not_just_samples() {
    use agent_spreadsheet::tools::fork::{ReplaceInFormulasOp, apply_replace_in_formulas_to_file};

    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("fail-all.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        for row in 1..=30 {
            sheet
                .cell_mut((2, row))
                .set_formula("SUM(C2:C10)".to_string());
        }
    });

    let tmp = tempdir().unwrap();
    let work = tmp.path().join("fail-all.xlsx");
    std::fs::copy(&path, &work).unwrap();

    let op = ReplaceInFormulasOp {
        sheet_name: "Sheet1".to_string(),
        find: "SUM(".to_string(),
        replace: "SUM((".to_string(),
        range: None,
        regex: false,
        case_sensitive: true,
    };

    let err = apply_replace_in_formulas_to_file(&work, &op, FormulaParsePolicy::Fail)
        .expect_err("invalid replacement formulas should fail under fail policy");
    assert!(
        err.to_string().contains("failed parse"),
        "unexpected error: {err}"
    );
}

#[test]
fn parse_policy_warn_skips_invalid_replacements_and_reports_diagnostics() {
    use agent_spreadsheet::tools::fork::{ReplaceInFormulasOp, apply_replace_in_formulas_to_file};

    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("warn-invalid.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        for row in 1..=30 {
            sheet
                .cell_mut((2, row))
                .set_formula("SUM(C2:C10)".to_string());
        }
    });

    let tmp = tempdir().unwrap();
    let work = tmp.path().join("warn-invalid.xlsx");
    std::fs::copy(&path, &work).unwrap();

    let op = ReplaceInFormulasOp {
        sheet_name: "Sheet1".to_string(),
        find: "SUM(".to_string(),
        replace: "SUM((".to_string(),
        range: None,
        regex: false,
        case_sensitive: true,
    };

    let result = apply_replace_in_formulas_to_file(&work, &op, FormulaParsePolicy::Warn)
        .expect("warn policy should not fail");

    assert_eq!(result.formulas_checked, 30);
    assert_eq!(
        result.formulas_changed, 0,
        "invalid replacements should be skipped"
    );
    let diagnostics = result
        .formula_parse_diagnostics
        .expect("warn policy should return diagnostics");
    assert!(diagnostics.total_errors >= 30);
}

// ── CLI integration tests ────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn cli_dry_run_preview_shows_expected_changes() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let path = create_formula_workbook(&workspace, "dry_run.xlsx");

    let result = agent_spreadsheet::cli::commands::write::replace_in_formulas(
        path.clone(),
        "Sheet1".to_string(),
        "C2:C10".to_string(),
        "D2:D20".to_string(),
        None,
        false,
        true,
        true,  // dry_run
        false, // in_place
        None,  // output
        false, // force
        None,  // formula_parse_policy
    )
    .await?;

    let obj = result.as_object().unwrap();
    assert_eq!(obj.get("would_change").and_then(Value::as_bool), Some(true));
    assert!(obj.get("formulas_changed").and_then(Value::as_u64).unwrap() >= 2);

    let samples = obj.get("samples").and_then(Value::as_array).unwrap();
    assert!(!samples.is_empty());

    // Verify original file is NOT modified (dry run)
    let book = umya_spreadsheet::reader::xlsx::read(&path).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    assert_eq!(sheet.cell("B2").unwrap().formula(), "SUM(C2:C10)");

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn cli_in_place_writes_expected_formulas() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let path = create_formula_workbook(&workspace, "inplace.xlsx");

    let result = agent_spreadsheet::cli::commands::write::replace_in_formulas(
        path.clone(),
        "Sheet1".to_string(),
        "C2:C10".to_string(),
        "D2:D20".to_string(),
        None,
        false,
        true,
        false, // dry_run
        true,  // in_place
        None,
        false,
        None,
    )
    .await?;

    let obj = result.as_object().unwrap();
    assert_eq!(obj.get("changed").and_then(Value::as_bool), Some(true));
    assert!(obj.get("formulas_changed").and_then(Value::as_u64).unwrap() >= 2);

    // Verify source file IS modified
    let book = umya_spreadsheet::reader::xlsx::read(&path).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    assert_eq!(sheet.cell("B2").unwrap().formula(), "SUM(D2:D20)");
    assert_eq!(
        sheet.cell("B3").unwrap().formula(),
        "AVERAGE(D2:D20)"
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn cli_in_place_fail_policy_rejects_without_mutating_source() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let path = workspace.create_workbook("inplace-fail-policy.xlsx", |book| {
        let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
        for row in 1..=30 {
            sheet
                .cell_mut((2, row))
                .set_formula("SUM(C2:C10)".to_string());
        }
    });

    let before = std::fs::read(&path)?;

    let err = agent_spreadsheet::cli::commands::write::replace_in_formulas(
        path.clone(),
        "Sheet1".to_string(),
        "SUM(".to_string(),
        "SUM((".to_string(),
        None,
        false,
        true,
        false,
        true,
        None,
        false,
        Some(FormulaParsePolicy::Fail),
    )
    .await
    .expect_err("fail policy should reject invalid replacement formulas");

    assert!(
        err.to_string().contains("failed parse"),
        "unexpected error: {err}"
    );

    let after = std::fs::read(&path)?;
    assert_eq!(
        before, after,
        "source workbook must remain unchanged on failure"
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn cli_output_mode_writes_to_target() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let path = create_formula_workbook(&workspace, "output.xlsx");
    let target = workspace.path("output_result.xlsx");

    let result = agent_spreadsheet::cli::commands::write::replace_in_formulas(
        path.clone(),
        "Sheet1".to_string(),
        "Sheet1!".to_string(),
        "Sheet2!".to_string(),
        None,
        false,
        true,
        false,
        false,
        Some(target.clone()),
        false,
        None,
    )
    .await?;

    let obj = result.as_object().unwrap();
    assert_eq!(obj.get("changed").and_then(Value::as_bool), Some(true));

    // Source unchanged
    let book = umya_spreadsheet::reader::xlsx::read(&path).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    assert_eq!(
        sheet.cell("B4").unwrap().formula(),
        "Sheet1!D5+Sheet1!D6"
    );

    // Target has the change
    let book = umya_spreadsheet::reader::xlsx::read(&target).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    assert_eq!(
        sheet.cell("B4").unwrap().formula(),
        "Sheet2!D5+Sheet2!D6"
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn cli_range_scoped_replace_only_modifies_target_area() -> Result<()> {
    let workspace = support::TestWorkspace::new();
    let path = create_formula_workbook(&workspace, "range_scope.xlsx");

    let result = agent_spreadsheet::cli::commands::write::replace_in_formulas(
        path.clone(),
        "Sheet1".to_string(),
        "C2:C10".to_string(),
        "X1:X5".to_string(),
        Some("B2:B2".to_string()),
        false,
        true,
        false,
        true,
        None,
        false,
        None,
    )
    .await?;

    let obj = result.as_object().unwrap();
    assert_eq!(obj.get("formulas_changed").and_then(Value::as_u64), Some(1));

    let book = umya_spreadsheet::reader::xlsx::read(&path).unwrap();
    let sheet = book.sheet_by_name("Sheet1").ok().unwrap();
    assert_eq!(sheet.cell("B2").unwrap().formula(), "SUM(X1:X5)");
    assert_eq!(
        sheet.cell("B3").unwrap().formula(),
        "AVERAGE(C2:C10)"
    );

    Ok(())
}

// ── CLI parse tests ──────────────────────────────────────────────────────────

#[test]
fn parses_replace_in_formulas_arguments() {
    use clap::Parser;
    use agent_spreadsheet::cli::Cli;

    let cli = Cli::try_parse_from([
        "agent-spreadsheet",
        "replace-in-formulas",
        "data.xlsx",
        "Sheet1",
        "--find",
        "$64",
        "--replace",
        "$65",
        "--range",
        "A1:Z100",
        "--regex",
        "--dry-run",
    ])
    .expect("parse replace-in-formulas");

    match cli.command {
        agent_spreadsheet::cli::Commands::ReplaceInFormulas {
            file,
            sheet,
            find,
            replace,
            range,
            regex,
            case_sensitive,
            dry_run,
            in_place,
            output,
            force,
            formula_parse_policy,
        } => {
            assert_eq!(file, std::path::PathBuf::from("data.xlsx"));
            assert_eq!(sheet, "Sheet1");
            assert_eq!(find, "$64");
            assert_eq!(replace, "$65");
            assert_eq!(range, Some("A1:Z100".to_string()));
            assert!(regex);
            assert!(case_sensitive.is_none());
            assert!(dry_run);
            assert!(!in_place);
            assert!(output.is_none());
            assert!(!force);
            assert!(formula_parse_policy.is_none());
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parses_replace_in_formulas_output_mode() {
    use clap::Parser;
    use agent_spreadsheet::cli::Cli;

    let cli = Cli::try_parse_from([
        "agent-spreadsheet",
        "replace-in-formulas",
        "data.xlsx",
        "Sheet1",
        "--find",
        "SUM",
        "--replace",
        "SUMIFS",
        "--output",
        "fixed.xlsx",
        "--force",
        "--formula-parse-policy",
        "warn",
    ])
    .expect("parse replace-in-formulas output mode");

    match cli.command {
        agent_spreadsheet::cli::Commands::ReplaceInFormulas {
            dry_run,
            in_place,
            output,
            force,
            formula_parse_policy,
            ..
        } => {
            assert!(!dry_run);
            assert!(!in_place);
            assert_eq!(output, Some(std::path::PathBuf::from("fixed.xlsx")));
            assert!(force);
            assert!(matches!(
                formula_parse_policy,
                Some(agent_spreadsheet::model::FormulaParsePolicy::Warn)
            ));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}
