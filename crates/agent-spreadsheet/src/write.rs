use crate::types::{CellEdit, CoreWarning};
use anyhow::{Context, Result, anyhow, bail};
use std::path::Path;

/// Excel worksheet bounds: max column XFD (16384), max row 1048576.
pub const MAX_COLUMN: u32 = 16_384;
pub const MAX_ROW: u32 = 1_048_576;

/// Parse and validate an A1-style cell reference within Excel bounds.
///
/// Returns a structured error (never panics) for malformed or out-of-bounds
/// addresses, so agents get a recoverable message instead of a library panic.
pub fn validate_cell_address(address: &str) -> Result<(u32, u32)> {
    let (col, row, _, _) = umya_spreadsheet::helper::coordinate::index_from_coordinate(address);
    match (col, row) {
        (Some(c), Some(r)) if (1..=MAX_COLUMN).contains(&c) && (1..=MAX_ROW).contains(&r) => {
            Ok((c, r))
        }
        _ => Err(anyhow!(
            "INVALID_CELL_REFERENCE: '{}' is not a valid A1-style cell reference within Excel bounds (columns A-XFD, rows 1-1048576)",
            address
        )),
    }
}

pub fn normalize_shorthand_edit(entry: &str) -> Result<(CellEdit, Vec<CoreWarning>)> {
    let Some((address_raw, rhs_raw)) = entry.split_once('=') else {
        bail!(
            "invalid shorthand edit: '{entry}' (expected 'A1=100' for literal values or 'B2==SUM(A1:A2)' for formulas)"
        );
    };

    let address = address_raw.trim();
    if address.is_empty() {
        bail!("invalid shorthand edit: '{entry}' (missing cell address before '=')");
    }

    // Validate bounds up front so agents get a structured error instead of a
    // downstream library panic on out-of-range addresses (e.g. ZZZZ999999).
    validate_cell_address(address)?;

    let warnings = Vec::new();

    let rhs_trimmed = rhs_raw.trim_start();
    if let Some(stripped) = rhs_trimmed.strip_prefix('=') {
        Ok((
            CellEdit {
                address: address.to_string(),
                value: stripped.to_string(),
                is_formula: true,
            },
            warnings,
        ))
    } else {
        Ok((
            CellEdit {
                address: address.to_string(),
                value: rhs_raw.to_string(),
                is_formula: false,
            },
            warnings,
        ))
    }
}

pub fn normalize_object_edit(
    address: &str,
    value: Option<String>,
    formula: Option<String>,
    is_formula: Option<bool>,
) -> Result<(CellEdit, Vec<CoreWarning>)> {
    let address = address.trim();
    if address.is_empty() {
        bail!("edit address is required");
    }
    validate_cell_address(address)?;

    let warnings = Vec::new();
    let (value, is_formula) = if let Some(formula) = formula {
        // A leading '=' on a formula field is documented, expected input —
        // stripping it silently is correct and must not warn.
        match formula.strip_prefix('=') {
            Some(stripped) => (stripped.to_string(), true),
            None => (formula, true),
        }
    } else if let Some(value) = value {
        // A leading '=' signals formula intent regardless of the flag; strip
        // it silently (documented behavior must not warn).
        match value.strip_prefix('=') {
            Some(stripped) => (stripped.to_string(), true),
            None => (value, is_formula.unwrap_or(false)),
        }
    } else {
        return Err(anyhow!("edit value or formula is required for {address}"));
    };

    Ok((
        CellEdit {
            address: address.to_string(),
            value,
            is_formula,
        },
        warnings,
    ))
}

pub fn apply_edits_to_file(path: &Path, sheet_name: &str, edits: &[CellEdit]) -> Result<()> {
    let mut book = crate::xlsx_import::read(path)
        .with_context(|| format!("failed to open workbook '{}'", path.display()))?;

    let sheet = book
        .sheet_by_name_mut(sheet_name).ok()
        .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;

    for edit in edits {
        let cell = sheet.cell_mut(edit.address.as_str());
        if edit.is_formula {
            cell.set_formula(edit.value.clone());
            cell.cell_value_mut()
                .set_formula_result_default(String::new());
        } else {
            cell.set_value(edit.value.clone());
        }
    }

    crate::xlsx_export::write(&book, path)
        .with_context(|| format!("failed to save workbook '{}'", path.display()))?;
    Ok(())
}
