#[cfg(not(feature = "recalc"))]
use crate::core::types::{BasicDiffChange, BasicDiffResponse};
#[cfg(not(feature = "recalc"))]
use anyhow::Context;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

#[cfg(feature = "recalc")]
pub fn calculate_changeset(
    base_path: &Path,
    fork_path: &Path,
    sheet_filter: Option<&str>,
) -> Result<Vec<crate::diff::Change>> {
    crate::diff::calculate_changeset(base_path, fork_path, sheet_filter)
}

pub fn diff_workbooks_json(original: &Path, modified: &Path) -> Result<Value> {
    #[cfg(feature = "recalc")]
    {
        let changes = calculate_changeset(original, modified, None)?;
        Ok(serde_json::json!({
            "original": original.display().to_string(),
            "modified": modified.display().to_string(),
            "change_count": changes.len(),
            "changes": changes,
        }))
    }

    #[cfg(not(feature = "recalc"))]
    {
        let response = basic_diff_workbooks(original, modified)?;
        Ok(serde_json::to_value(response)?)
    }
}

#[cfg(not(feature = "recalc"))]
#[derive(Debug, Clone)]
struct CellSnapshot {
    value: String,
    formula: Option<String>,
}

#[cfg(not(feature = "recalc"))]
fn basic_diff_workbooks(original: &Path, modified: &Path) -> Result<BasicDiffResponse> {
    use std::collections::BTreeSet;

    let original_cells = collect_cells(original)?;
    let modified_cells = collect_cells(modified)?;

    let mut keys = BTreeSet::new();
    keys.extend(original_cells.keys().cloned());
    keys.extend(modified_cells.keys().cloned());

    let mut changes = Vec::new();
    for (sheet, address) in keys {
        let original_cell = original_cells.get(&(sheet.clone(), address.clone()));
        let modified_cell = modified_cells.get(&(sheet.clone(), address.clone()));

        if cells_equal(original_cell, modified_cell) {
            continue;
        }

        let change_type = match (original_cell, modified_cell) {
            (None, Some(_)) => "added",
            (Some(_), None) => "removed",
            (Some(orig), Some(next))
                if orig.formula != next.formula && orig.value != next.value =>
            {
                "formula_and_value_changed"
            }
            (Some(orig), Some(next)) if orig.formula != next.formula => "formula_changed",
            _ => "value_changed",
        }
        .to_string();

        changes.push(BasicDiffChange {
            sheet,
            address,
            change_type,
            original_value: original_cell.map(|cell| cell.value.clone()),
            original_formula: original_cell.and_then(|cell| cell.formula.clone()),
            modified_value: modified_cell.map(|cell| cell.value.clone()),
            modified_formula: modified_cell.and_then(|cell| cell.formula.clone()),
        });
    }

    Ok(BasicDiffResponse {
        original: original.display().to_string(),
        modified: modified.display().to_string(),
        change_count: changes.len(),
        changes,
    })
}

#[cfg(not(feature = "recalc"))]
fn collect_cells(
    path: &Path,
) -> Result<std::collections::BTreeMap<(String, String), CellSnapshot>> {
    let book = crate::xlsx_import::read(path)
        .with_context(|| format!("failed to read workbook '{}'", path.display()))?;
    let mut cells = std::collections::BTreeMap::new();

    for sheet in book.get_sheet_collection() {
        let sheet_name = sheet.get_name().to_string();
        for cell in sheet.cells() {
            let address = cell.get_coordinate().get_coordinate().to_string();
            let value = cell.get_value().to_string();
            let formula = if cell.is_formula() {
                Some(cell.get_formula().to_string())
            } else {
                None
            };

            cells.insert(
                (sheet_name.clone(), address),
                CellSnapshot { value, formula },
            );
        }
    }

    Ok(cells)
}

#[cfg(not(feature = "recalc"))]
fn cells_equal(left: Option<&CellSnapshot>, right: Option<&CellSnapshot>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(a), Some(b)) => a.value == b.value && a.formula == b.formula,
        _ => false,
    }
}
