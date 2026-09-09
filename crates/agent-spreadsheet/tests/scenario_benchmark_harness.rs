use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;
use tempfile::tempdir;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScenarioBudgetFile {
    scenario_id: String,
    budgets: ScenarioBudgets,
    correctness: ScenarioCorrectness,
    #[serde(default)]
    observed_example: Option<ScenarioObserved>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScenarioBudgets {
    max_tool_calls: u32,
    max_manual_verification_loops: u32,
    max_wall_time_ms: u64,
    max_total_output_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScenarioCorrectness {
    expected_changed_targets: u32,
    expected_new_error_count: u32,
    expected_resolved_error_count: u32,
    expected_preexisting_error_count: u32,
    expected_targets: Vec<ScenarioTargetExpectation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScenarioTargetExpectation {
    address: String,
    classification: String,
    after_number: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScenarioObserved {
    tool_calls: u32,
    manual_verification_loops: u32,
    wall_time_ms: u64,
    total_output_bytes: u64,
    steps: Vec<StepObservation>,
    verify_summary: VerifySummarySnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StepObservation {
    name: String,
    wall_time_ms: u64,
    output_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VerifySummarySnapshot {
    target_count: u32,
    changed_targets: u32,
    new_error_count: u32,
    resolved_error_count: u32,
    preexisting_error_count: u32,
    named_range_delta_count: u32,
    target_classification_counts: BTreeMap<String, u32>,
}

fn run_asp(args: &[&str]) -> Output {
    Command::new(assert_cmd::cargo::cargo_bin!("asp"))
        .args(args)
        .output()
        .expect("run asp")
}

fn parse_stdout_json(output: &Output) -> Value {
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

fn assert_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn budget_file_path() -> PathBuf {
    repo_root()
        .join("benchmarks")
        .join("scenario-01-roll-forward")
        .join("budget.json")
}

fn write_roll_forward_fixture(path: &Path) {
    let mut workbook = umya_spreadsheet::new_file();
    {
        let inputs = workbook
            .sheet_by_name_mut("Sheet1").ok()
            .expect("default sheet exists");
        inputs.set_name("Inputs");
        inputs.cell_mut("A1").set_value("Period");
        inputs.cell_mut("B1").set_value("2024-Q2");
        inputs.cell_mut("A2").set_value("Monthly Revenue");
        inputs.cell_mut("B2").set_value_number(100.0);
    }

    workbook.new_sheet("Summary").expect("summary sheet");
    {
        let summary = workbook
            .sheet_by_name_mut("Summary").ok()
            .expect("summary exists");
        summary.cell_mut("A1").set_value("Annualized Revenue");
        let annualized = summary.cell_mut("B1");
        annualized.set_formula("Inputs!B2*4");
        annualized
            .cell_value_mut()
            .set_formula_result_default("400");

        summary.cell_mut("A2").set_value("Bonus Pool");
        let bonus = summary.cell_mut("B2");
        bonus.set_formula("Inputs!B2*4*0.1");
        bonus.cell_value_mut().set_formula_result_default("40");
    }

    umya_spreadsheet::writer::xlsx::write(&workbook, path).expect("write fixture workbook");
}

fn run_step(name: &str, args: &[&str]) -> (Output, StepObservation) {
    let start = Instant::now();
    let output = run_asp(args);
    let wall_time_ms = start.elapsed().as_millis() as u64;
    let output_bytes = (output.stdout.len() + output.stderr.len()) as u64;
    (
        output,
        StepObservation {
            name: name.to_string(),
            wall_time_ms,
            output_bytes,
        },
    )
}

fn extract_verify_summary(payload: &Value) -> VerifySummarySnapshot {
    let counts = &payload["summary"]["target_classification_counts"];
    let mut target_classification_counts = BTreeMap::new();
    for key in [
        "unchanged",
        "direct_edit",
        "recalc_result",
        "formula_shift",
        "new_error",
    ] {
        target_classification_counts
            .insert(key.to_string(), counts[key].as_u64().unwrap_or(0) as u32);
    }

    VerifySummarySnapshot {
        target_count: payload["summary"]["target_count"].as_u64().unwrap_or(0) as u32,
        changed_targets: payload["summary"]["changed_targets"].as_u64().unwrap_or(0) as u32,
        new_error_count: payload["summary"]["new_error_count"].as_u64().unwrap_or(0) as u32,
        resolved_error_count: payload["summary"]["resolved_error_count"]
            .as_u64()
            .unwrap_or(0) as u32,
        preexisting_error_count: payload["summary"]["preexisting_error_count"]
            .as_u64()
            .unwrap_or(0) as u32,
        named_range_delta_count: payload["summary"]["named_range_delta_count"]
            .as_u64()
            .unwrap_or(0) as u32,
        target_classification_counts,
    }
}

fn measure_scenario_01_roll_forward() -> (ScenarioObserved, Value) {
    let tmp = tempdir().expect("tempdir");
    let baseline_path = tmp.path().join("scenario-01-base.xlsx");
    let draft_path = tmp.path().join("scenario-01-draft.xlsx");
    let result_path = tmp.path().join("scenario-01-result.xlsx");
    write_roll_forward_fixture(&baseline_path);

    let baseline = baseline_path.to_str().expect("baseline utf8");
    let draft = draft_path.to_str().expect("draft utf8");
    let result = result_path.to_str().expect("result utf8");

    let started = Instant::now();
    let mut steps = Vec::new();

    let (edit_out, edit_step) = run_step(
        "edit",
        &[
            "edit",
            baseline,
            "Inputs",
            "--output",
            draft,
            "B1=2024-Q3",
            "B2=120",
        ],
    );
    assert_success(&edit_out, "edit");
    let edit_json = parse_stdout_json(&edit_out);
    assert_eq!(edit_json["edits_applied"].as_u64(), Some(2));
    steps.push(edit_step);

    let (recalc_out, recalc_step) =
        run_step("recalculate", &["recalculate", draft, "--output", result]);
    assert_success(&recalc_out, "recalculate");
    let recalc_json = parse_stdout_json(&recalc_out);
    assert_eq!(recalc_json["target_path"].as_str(), Some(result));
    steps.push(recalc_step);

    let (verify_out, verify_step) = run_step(
        "verify",
        &[
            "verify",
            baseline,
            result,
            "--targets",
            "Summary!B1,Summary!B2",
            "--sheet",
            "Summary",
        ],
    );
    assert_success(&verify_out, "verify");
    let verify_json = parse_stdout_json(&verify_out);
    steps.push(verify_step);

    let observed = ScenarioObserved {
        tool_calls: steps.len() as u32,
        manual_verification_loops: 1,
        wall_time_ms: started.elapsed().as_millis() as u64,
        total_output_bytes: steps.iter().map(|step| step.output_bytes).sum(),
        verify_summary: extract_verify_summary(&verify_json),
        steps,
    };

    (observed, verify_json)
}

fn assert_correctness(payload: &Value, expected: &ScenarioCorrectness) {
    assert_eq!(
        payload["summary"]["changed_targets"].as_u64().unwrap_or(0) as u32,
        expected.expected_changed_targets
    );
    assert_eq!(
        payload["summary"]["new_error_count"].as_u64().unwrap_or(0) as u32,
        expected.expected_new_error_count
    );
    assert_eq!(
        payload["summary"]["resolved_error_count"]
            .as_u64()
            .unwrap_or(0) as u32,
        expected.expected_resolved_error_count
    );
    assert_eq!(
        payload["summary"]["preexisting_error_count"]
            .as_u64()
            .unwrap_or(0) as u32,
        expected.expected_preexisting_error_count
    );

    let targets = payload["target_deltas"]
        .as_array()
        .expect("target_deltas array");
    assert_eq!(targets.len(), expected.expected_targets.len());
    for (actual, expectation) in targets.iter().zip(expected.expected_targets.iter()) {
        assert_eq!(
            actual["address"].as_str(),
            Some(expectation.address.as_str())
        );
        assert_eq!(
            actual["classification"].as_str(),
            Some(expectation.classification.as_str())
        );
        let after = actual["after"]["value"]
            .as_f64()
            .expect("after numeric value");
        assert!(
            (after - expectation.after_number).abs() < f64::EPSILON,
            "target {} after value mismatch: actual={after}, expected={}",
            expectation.address,
            expectation.after_number
        );
    }

    assert_eq!(payload["new_errors"], serde_json::json!([]));
    assert_eq!(payload["resolved_errors"], serde_json::json!([]));
    assert_eq!(payload["preexisting_errors"], serde_json::json!([]));
}

fn derived_budget_file(observed: &ScenarioObserved) -> ScenarioBudgetFile {
    ScenarioBudgetFile {
        scenario_id: "scenario-01-roll-forward".to_string(),
        budgets: ScenarioBudgets {
            max_tool_calls: observed.tool_calls,
            max_manual_verification_loops: observed.manual_verification_loops,
            max_wall_time_ms: (observed.wall_time_ms.saturating_mul(5)).max(1_500),
            max_total_output_bytes: (observed.total_output_bytes.saturating_mul(2)).max(4_096),
        },
        correctness: ScenarioCorrectness {
            expected_changed_targets: 2,
            expected_new_error_count: 0,
            expected_resolved_error_count: 0,
            expected_preexisting_error_count: 0,
            expected_targets: vec![
                ScenarioTargetExpectation {
                    address: "Summary!B1".to_string(),
                    classification: "recalc_result".to_string(),
                    after_number: 480.0,
                },
                ScenarioTargetExpectation {
                    address: "Summary!B2".to_string(),
                    classification: "recalc_result".to_string(),
                    after_number: 48.0,
                },
            ],
        },
        observed_example: Some(observed.clone()),
    }
}

fn write_budget_file(path: &Path, budget: &ScenarioBudgetFile) {
    fs::write(
        path,
        serde_json::to_string_pretty(budget).expect("serialize budget"),
    )
    .expect("write budget file");
}

#[test]
fn scenario_01_roll_forward_budget_regression_gate() {
    let (observed, verify_payload) = measure_scenario_01_roll_forward();

    let budget_path = budget_file_path();
    if std::env::var("UPDATE_SCENARIO_BUDGETS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        let refreshed = derived_budget_file(&observed);
        write_budget_file(&budget_path, &refreshed);
        eprintln!(
            "updated scenario budget baseline at {}\n{}",
            budget_path.display(),
            serde_json::to_string_pretty(&refreshed).expect("serialize refreshed budget")
        );
        return;
    }

    let baseline: ScenarioBudgetFile = serde_json::from_str(
        &fs::read_to_string(&budget_path)
            .unwrap_or_else(|e| panic!("read budget file '{}': {}", budget_path.display(), e)),
    )
    .expect("parse scenario budget file");

    assert_correctness(&verify_payload, &baseline.correctness);

    assert!(
        observed.tool_calls <= baseline.budgets.max_tool_calls,
        "tool call regression: observed={}, budget={}\nobserved={}",
        observed.tool_calls,
        baseline.budgets.max_tool_calls,
        serde_json::to_string_pretty(&observed).expect("serialize observed")
    );
    assert!(
        observed.manual_verification_loops <= baseline.budgets.max_manual_verification_loops,
        "manual verification loop regression: observed={}, budget={}\nobserved={}",
        observed.manual_verification_loops,
        baseline.budgets.max_manual_verification_loops,
        serde_json::to_string_pretty(&observed).expect("serialize observed")
    );
    assert!(
        observed.wall_time_ms <= baseline.budgets.max_wall_time_ms,
        "wall time regression: observed={}ms, budget={}ms\nobserved={}",
        observed.wall_time_ms,
        baseline.budgets.max_wall_time_ms,
        serde_json::to_string_pretty(&observed).expect("serialize observed")
    );
    assert!(
        observed.total_output_bytes <= baseline.budgets.max_total_output_bytes,
        "output-size regression: observed={}B, budget={}B\nobserved={}",
        observed.total_output_bytes,
        baseline.budgets.max_total_output_bytes,
        serde_json::to_string_pretty(&observed).expect("serialize observed")
    );
}
