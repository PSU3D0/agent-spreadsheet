#![cfg(feature = "recalc-formualizer")]

use agent_spreadsheet::cli::RangeValuesFormatArg;
use agent_spreadsheet::cli::commands::{read, recalc, verify};
use agent_spreadsheet::config::ServerConfig;
use agent_spreadsheet::model::{
    CellValue, EvaluationCoverage, EvaluationFreshness, EvaluationSource, EvaluationState,
};
use agent_spreadsheet::verification::{
    evaluate_workbook_for_verification, evaluate_workbook_pair_for_verification,
};
use agent_spreadsheet::workbook::{WorkbookContext, cell_to_value};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod support;

fn test_config() -> Arc<ServerConfig> {
    Arc::new(support::TestWorkspace::new().config())
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/f1")
        .join(name)
}

fn load(name: &str) -> WorkbookContext {
    WorkbookContext::load(&test_config(), &fixture(name)).unwrap()
}

#[test]
fn all_eight_adversarial_fixtures_are_vendored() {
    for name in [
        "baseline.xlsx",
        "unevaluated_broken.xlsx",
        "partial.xlsx",
        "stale_cache.xlsx",
        "real_errors.xlsx",
        "ratio_600_percent.xlsx",
        "all_formulas_uncached.xlsx",
        "evaluated_empty_string.xlsx",
        "error_cell_types.xlsx",
    ] {
        assert!(fixture(name).is_file(), "missing fixture {name}");
    }
}

#[test]
fn imported_caches_never_claim_clean() {
    let stale = load("stale_cache.xlsx").imported_evaluation_coverage();
    assert_eq!(stale.formula_cells, 1);
    assert_eq!(stale.evaluated_formula_cells, 1);
    assert_eq!(stale.freshness, EvaluationFreshness::Unknown);
    assert_eq!(stale.state(), EvaluationState::NotEvaluated);

    let partial = load("partial.xlsx").imported_evaluation_coverage();
    assert_eq!(partial.formula_cells, 3);
    assert_eq!(partial.evaluated_formula_cells, 2);
    assert_eq!(partial.state(), EvaluationState::Partial);

    let uncached = load("unevaluated_broken.xlsx").imported_evaluation_coverage();
    assert_eq!(uncached.evaluated_formula_cells, 0);
    assert_eq!(uncached.state(), EvaluationState::NotEvaluated);
}

#[test]
fn imported_coverage_is_scanned_once_per_workbook_revision() {
    let workbook = load("partial.xlsx");
    assert_eq!(workbook.imported_evaluation_scan_count(), 0);

    let first = workbook.calculation_metadata();
    let second = workbook.calculation_metadata();

    assert_eq!(first.revision_id, second.revision_id);
    assert_eq!(workbook.imported_evaluation_scan_count(), 1);
}

#[test]
fn proof_coverage_requires_valid_counts_and_a_trusted_source() {
    let complete = EvaluationCoverage {
        formula_cells: 2,
        evaluated_formula_cells: 2,
        unsupported_formula_cells: 0,
        error_formula_cells: 0,
        source: EvaluationSource::Formualizer,
        freshness: EvaluationFreshness::CurrentRevision,
        revision_id: "revision".to_string(),
    };
    assert!(complete.is_complete_and_fresh());

    let mut untrusted = complete.clone();
    untrusted.source = EvaluationSource::None;
    assert!(!untrusted.is_complete_and_fresh());

    let mut over_evaluated = complete.clone();
    over_evaluated.evaluated_formula_cells = 3;
    assert!(!over_evaluated.is_complete_and_fresh());

    let mut impossible_errors = complete;
    impossible_errors.error_formula_cells = 3;
    assert!(!impossible_errors.is_complete_and_fresh());
}

#[tokio::test]
async fn formualizer_coverage_counts_errors_and_empty_results_as_evaluated() -> Result<()> {
    let config = test_config();

    let stale_book = load("stale_cache.xlsx");
    let stale = evaluate_workbook_for_verification(&config, &stale_book).await?;
    assert_eq!(stale.coverage.state(), EvaluationState::Clean);
    assert_eq!(stale.coverage.formula_cells, 1);
    assert_eq!(stale.coverage.evaluated_formula_cells, 1);
    assert_eq!(
        stale.coverage.freshness,
        EvaluationFreshness::CurrentRevision
    );
    let value = stale.workbook.with_sheet("Sheet1", |sheet| {
        cell_to_value(sheet.get_cell("B1").unwrap())
    })?;
    assert!(matches!(value, Some(CellValue::Number(value)) if value == 20.0));

    let partial_book = load("partial.xlsx");
    let partial = evaluate_workbook_for_verification(&config, &partial_book).await?;
    assert_eq!(partial.coverage.state(), EvaluationState::ErrorsFound);
    assert_eq!(partial.coverage.formula_cells, 3);
    assert_eq!(partial.coverage.evaluated_formula_cells, 3);
    assert_eq!(partial.coverage.error_formula_cells, 1);

    let broken_book = load("unevaluated_broken.xlsx");
    let broken = evaluate_workbook_for_verification(&config, &broken_book).await?;
    assert_eq!(broken.coverage.state(), EvaluationState::ErrorsFound);
    assert_eq!(broken.coverage.error_formula_cells, 2);
    let broken_value = broken.workbook.with_sheet("Sheet1", |sheet| {
        cell_to_value(sheet.get_cell("B1").unwrap())
    })?;
    assert!(matches!(broken_value, Some(CellValue::Error(_))));

    let empty_book = load("evaluated_empty_string.xlsx");
    let empty = evaluate_workbook_for_verification(&config, &empty_book).await?;
    assert_eq!(empty.coverage.state(), EvaluationState::Clean);
    assert_eq!(empty.coverage.formula_cells, 1);
    assert_eq!(empty.coverage.evaluated_formula_cells, 1);
    Ok(())
}

#[tokio::test]
async fn recalculate_response_reports_canonical_state_and_coverage() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let output = temp.path().join("recalculated.xlsx");
    let response =
        recalc::recalculate(fixture("partial.xlsx"), Some(output), false, None, false).await?;
    assert_eq!(response["state"], "errors_found");
    assert_eq!(response["evaluation_coverage"]["formula_cells"], 3);
    assert_eq!(
        response["evaluation_coverage"]["evaluated_formula_cells"],
        3
    );
    assert_eq!(response["evaluation_coverage"]["error_formula_cells"], 1);
    assert_eq!(response["evaluation_coverage"]["source"], "formualizer");
    assert_eq!(
        response["evaluation_coverage"]["freshness"],
        "current_revision"
    );
    assert!(
        response["evaluation_coverage"]["revision_id"]
            .as_str()
            .is_some_and(|revision| !revision.is_empty())
    );
    Ok(())
}

#[tokio::test]
async fn verify_evaluates_errors_and_detects_same_address_changes() -> Result<()> {
    let config = test_config();
    let identical_workbook = load("unevaluated_broken.xlsx");
    let evaluated = evaluate_workbook_pair_for_verification(
        &config,
        &identical_workbook,
        &config,
        &identical_workbook,
    )
    .await?;
    assert_eq!(evaluated.evaluations_performed(), 1);

    let response = verify::verify(
        fixture("unevaluated_broken.xlsx"),
        fixture("unevaluated_broken.xlsx"),
        None,
        None,
        false,
        true,
        false,
    )
    .await?;
    assert_eq!(response["proof_status"], "proved");
    assert_eq!(response["baseline_state"], "errors_found");
    assert_eq!(response["current_state"], "errors_found");
    assert_eq!(
        response["baseline_evaluation_coverage"]["evaluated_formula_cells"],
        2
    );
    assert_eq!(
        response["current_evaluation_coverage"]["evaluated_formula_cells"],
        2
    );
    assert_eq!(response["preexisting_errors"].as_array().unwrap().len(), 2);

    let temp = tempfile::tempdir()?;
    let changed_path = temp.path().join("changed-errors.xlsx");
    let mut changed = umya_spreadsheet::reader::xlsx::read(fixture("real_errors.xlsx"))?;
    changed
        .get_sheet_by_name_mut("Sheet1").ok()
        .unwrap()
        .get_cell_mut("A1")
        .set_formula("UNKNOWNFN(2)")
        .set_formula_result_default("#NAME?");
    umya_spreadsheet::writer::xlsx::write(&changed, &changed_path)?;

    let changed_errors = verify::verify(
        fixture("real_errors.xlsx"),
        changed_path,
        None,
        None,
        false,
        true,
        false,
    )
    .await?;
    assert_eq!(changed_errors["proof_status"], "differences_found");
    assert_eq!(changed_errors["new_errors"].as_array().unwrap().len(), 0);
    assert_eq!(
        changed_errors["preexisting_errors"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let changed_a1 = changed_errors["preexisting_errors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|delta| delta["address"] == "Sheet1!A1")
        .unwrap();
    assert_ne!(changed_a1["before_error"], changed_a1["after_error"]);
    assert_ne!(changed_a1["before_formula"], changed_a1["after_formula"]);

    let differences = verify::verify(
        fixture("baseline.xlsx"),
        fixture("unevaluated_broken.xlsx"),
        None,
        None,
        false,
        true,
        false,
    )
    .await?;
    assert_eq!(differences["proof_status"], "differences_found");
    assert_eq!(differences["new_errors"].as_array().unwrap().len(), 2);
    Ok(())
}

#[test]
fn formula_ratios_are_structural_and_bounded() {
    let ratio = load("ratio_600_percent.xlsx")
        .sheet_overview("Sheet1")
        .unwrap();
    assert!((ratio.formula_ratio - (6.0 / 7.0)).abs() < 0.000_001);
    assert!((0.0..=1.0).contains(&ratio.formula_ratio));

    let all_formulas = load("all_formulas_uncached.xlsx")
        .sheet_overview("Sheet1")
        .unwrap();
    assert_eq!(all_formulas.formula_ratio, 1.0);
    assert!(!all_formulas.narrative.starts_with("Empty sheet"));

    let partial = load("partial.xlsx").sheet_overview("Sheet1").unwrap();
    assert_eq!(partial.formula_ratio, 0.75);
}

#[test]
fn ooxml_errors_are_typed_but_error_like_strings_remain_text() {
    let cell_types = load("error_cell_types.xlsx");
    cell_types
        .with_sheet("Sheet1", |sheet| {
            assert!(matches!(
                cell_to_value(sheet.get_cell("A1").unwrap()),
                Some(CellValue::Error(value)) if value == "#N/A"
            ));
            assert!(matches!(
                cell_to_value(sheet.get_cell("B1").unwrap()),
                Some(CellValue::Text(value)) if value == "#N/A"
            ));
            assert!(matches!(
                cell_to_value(sheet.get_cell("C1").unwrap()),
                Some(CellValue::Text(value)) if value == "#REF!"
            ));
            assert!(matches!(
                cell_to_value(sheet.get_cell("D1").unwrap()),
                Some(CellValue::Error(value)) if value == "#DIV/0!"
            ));
        })
        .unwrap();
}

#[tokio::test]
async fn value_reads_disclose_calculation_state_and_revision() -> Result<()> {
    let response = read::range_values(
        fixture("stale_cache.xlsx"),
        "Sheet1".to_string(),
        vec!["A1:B1".to_string()],
        Some(RangeValuesFormatArg::Json),
        Some(true),
    )
    .await?;
    assert_eq!(response["calculation"]["state"], "not_evaluated");
    assert!(
        response["calculation"]["revision_id"]
            .as_str()
            .is_some_and(|revision| !revision.is_empty())
    );
    Ok(())
}
