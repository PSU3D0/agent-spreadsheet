use crate::runtime::stateless::StatelessRuntime;
use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::Builder;

#[derive(Debug, Serialize)]
struct RecalculateResponse {
    file: String,
    backend: String,
    duration_ms: u64,
    state: crate::model::EvaluationState,
    evaluation_coverage: crate::model::EvaluationCoverage,
    cells_evaluated: Option<u64>,
    eval_errors: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed_cells_summary: Option<ChangedCellsSummary>,
}

#[derive(Debug, Serialize)]
struct ChangedCellsSummary {
    total_changed: u64,
    by_sheet: BTreeMap<String, u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ignored_sheets: Option<Vec<String>>,
    /// Sample of changed cells (max 50)
    samples: Vec<ChangedCellSample>,
}

#[derive(Debug, Serialize)]
struct ChangedCellSample {
    sheet: String,
    address: String,
    before: String,
    after: String,
}

/// Snapshot cell values from a workbook file, skipping sheets in `ignore`.
fn snapshot_cell_values(
    path: &Path,
    ignore: &[String],
) -> Result<BTreeMap<(String, String), String>> {
    let book = crate::xlsx_import::read(path).map_err(|e| {
        anyhow!(
            "failed to read workbook '{}' for snapshot: {}",
            path.display(),
            e
        )
    })?;
    let mut cells = BTreeMap::new();

    for sheet in book.sheet_collection() {
        let sheet_name = sheet.name().to_string();
        if ignore.iter().any(|s| s == &sheet_name) {
            continue;
        }
        for cell in sheet.cells() {
            let address = cell.coordinate().get_coordinate().to_string();
            let value = cell.value().to_string();
            cells.insert((sheet_name.clone(), address), value);
        }
    }

    Ok(cells)
}

/// Compare before/after snapshots and produce a `ChangedCellsSummary`.
fn build_changed_cells_summary(
    before: &BTreeMap<(String, String), String>,
    after: &BTreeMap<(String, String), String>,
    ignored_sheets: Option<Vec<String>>,
) -> ChangedCellsSummary {
    let mut by_sheet: BTreeMap<String, u64> = BTreeMap::new();
    let mut samples: Vec<ChangedCellSample> = Vec::new();
    let mut total_changed: u64 = 0;

    // Collect all keys from both maps.
    let mut all_keys: Vec<&(String, String)> = before.keys().chain(after.keys()).collect();
    all_keys.sort();
    all_keys.dedup();

    for key in all_keys {
        let before_val = before.get(key).map(|s| s.as_str()).unwrap_or("");
        let after_val = after.get(key).map(|s| s.as_str()).unwrap_or("");

        if before_val != after_val {
            total_changed += 1;
            *by_sheet.entry(key.0.clone()).or_insert(0) += 1;

            if samples.len() < 50 {
                samples.push(ChangedCellSample {
                    sheet: key.0.clone(),
                    address: key.1.clone(),
                    before: before_val.to_string(),
                    after: after_val.to_string(),
                });
            }
        }
    }

    ChangedCellsSummary {
        total_changed,
        by_sheet,
        ignored_sheets,
        samples,
    }
}

pub async fn recalculate(
    file: PathBuf,
    output: Option<PathBuf>,
    force: bool,
    ignore_sheets: Option<Vec<String>>,
    changed_cells: bool,
) -> Result<Value> {
    if force && output.is_none() {
        bail!("invalid argument: --force requires --output <PATH>");
    }

    let runtime = StatelessRuntime;
    let source = runtime.normalize_existing_file(&file)?;

    let ignore_list = ignore_sheets.clone().unwrap_or_default();

    match output {
        None => {
            // In-place mode (existing behavior)
            let before_snapshot = if changed_cells {
                Some(snapshot_cell_values(&source, &ignore_list)?)
            } else {
                None
            };

            let outcome = runtime.recalculate_file(&source).await?;

            let summary = if changed_cells {
                let after_snapshot = snapshot_cell_values(&source, &ignore_list)?;
                Some(build_changed_cells_summary(
                    before_snapshot.as_ref().unwrap(),
                    &after_snapshot,
                    if ignore_list.is_empty() {
                        None
                    } else {
                        Some(ignore_list)
                    },
                ))
            } else {
                None
            };

            Ok(serde_json::to_value(RecalculateResponse {
                file: source.display().to_string(),
                backend: outcome.backend,
                duration_ms: outcome.duration_ms,
                state: outcome.state,
                evaluation_coverage: outcome.evaluation_coverage,
                cells_evaluated: outcome.cells_evaluated,
                eval_errors: outcome.eval_errors,
                source_path: None,
                target_path: None,
                changed: None,
                changed_cells_summary: summary,
            })?)
        }
        Some(output_path) => {
            // Output mode: copy source to a temp file in the target directory,
            // recalculate temp, then atomically move into place. This keeps an
            // existing target untouched if recalc fails.
            let target = runtime.normalize_destination_path(&output_path)?;
            ensure_output_path_is_distinct(&source, &target)?;

            let target_exists = target.exists();
            if target_exists && !force {
                bail!(
                    "output exists: output path '{}' already exists",
                    target.display()
                );
            }

            let target_parent = target.parent().unwrap_or_else(|| Path::new("."));
            let temp_file = Builder::new()
                .prefix(".recalculate-")
                .suffix(".xlsx")
                .tempfile_in(target_parent)
                .map_err(|error| {
                    anyhow!(
                        "write failed: unable to create temp output in '{}': {}",
                        target_parent.display(),
                        error
                    )
                })?;
            let temp_path = temp_file.path().to_path_buf();

            runtime.copy_file(&source, &temp_path).map_err(|error| {
                anyhow!(
                    "write failed: unable to copy workbook from '{}' to '{}': {}",
                    source.display(),
                    target.display(),
                    error
                )
            })?;

            // Snapshot before recalc (from the copy, which has the same values as source).
            let before_snapshot = if changed_cells {
                Some(snapshot_cell_values(&temp_path, &ignore_list)?)
            } else {
                None
            };

            let outcome = runtime.recalculate_file(&temp_path).await?;

            // Snapshot after recalc (from the recalculated temp file).
            let summary = if changed_cells {
                let after_snapshot = snapshot_cell_values(&temp_path, &ignore_list)?;
                Some(build_changed_cells_summary(
                    before_snapshot.as_ref().unwrap(),
                    &after_snapshot,
                    if ignore_list.is_empty() {
                        None
                    } else {
                        Some(ignore_list)
                    },
                ))
            } else {
                None
            };

            if target_exists {
                fs::remove_file(&target).map_err(|error| {
                    anyhow!(
                        "write failed: unable to remove existing output '{}': {}",
                        target.display(),
                        error
                    )
                })?;
            }

            temp_file.persist(&target).map_err(|error| {
                anyhow!(
                    "write failed: unable to persist recalculated output to '{}': {}",
                    target.display(),
                    error.error
                )
            })?;

            Ok(serde_json::to_value(RecalculateResponse {
                file: target.display().to_string(),
                backend: outcome.backend,
                duration_ms: outcome.duration_ms,
                state: outcome.state,
                evaluation_coverage: outcome.evaluation_coverage,
                cells_evaluated: outcome.cells_evaluated,
                eval_errors: outcome.eval_errors,
                source_path: Some(source.display().to_string()),
                target_path: Some(target.display().to_string()),
                changed: Some(true),
                changed_cells_summary: summary,
            })?)
        }
    }
}

fn ensure_output_path_is_distinct(source: &Path, output: &Path) -> Result<()> {
    let source_identity = canonical_identity_path(source)?;
    let output_identity = canonical_identity_path(output)?;
    if source_identity == output_identity {
        bail!("invalid argument: --output path resolves to the same file as input");
    }
    Ok(())
}

fn canonical_identity_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path).map_err(|e| {
            anyhow!(
                "failed to resolve canonical identity path for '{}': {}",
                path.display(),
                e
            )
        });
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("invalid argument: output path must include a file name"))?;

    let parent_canonical = fs::canonicalize(parent).map_err(|_| {
        anyhow!(
            "invalid argument: output parent directory '{}' does not exist or is inaccessible",
            parent.display()
        )
    })?;

    Ok(parent_canonical.join(name))
}
