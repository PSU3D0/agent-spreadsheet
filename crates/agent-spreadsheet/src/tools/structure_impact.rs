//! Pure planning functions for structure-batch impact reports and formula delta previews.
//!
//! These functions analyse a workbook + structure ops **without** mutating any file.

use crate::tools::fork::StructureOp;
use anyhow::Result;
use formualizer_parse::tokenizer::Tokenizer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

// ──────────────────────────────────────────────────────────────────
//  Public types (additive – never break existing callers)
// ──────────────────────────────────────────────────────────────────

/// Machine-readable impact report for a set of structure operations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StructureImpactReport {
    /// Per-operation shifted row/col spans.
    pub shifted_spans: Vec<ShiftedSpan>,
    /// Absolute-reference-risk warnings for `$`-anchored refs crossing insertion/deletion zones.
    pub absolute_ref_warnings: Vec<AbsoluteRefWarning>,
    /// Number of formula tokens affected (would be rewritten).
    pub tokens_affected: u64,
    /// Number of formula tokens unaffected (outside the zone).
    pub tokens_unaffected: u64,
    /// Informational notes (e.g. single-cell range no-expansion).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Describes a row/column span that would be shifted by a structure op.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ShiftedSpan {
    pub op_index: usize,
    pub sheet_name: String,
    pub axis: String, // "row" | "col"
    /// Human-readable description, e.g. "rows 5..∞ shift +3".
    pub description: String,
    pub at: u32,
    pub count: u32,
    pub direction: String, // "insert" | "delete"
}

/// A warning about an absolute reference (`$A$5`) that crosses a structural edit boundary.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AbsoluteRefWarning {
    /// Warning code: `ABSOLUTE_REF_CROSS_INSERT` or `ABSOLUTE_REF_CROSS_DELETE`.
    pub warning_code: String,
    /// Sheet + cell owning the formula, e.g. `"Calc!B2"`.
    pub cell: String,
    /// The original formula text.
    pub formula: String,
    /// The specific token that is at risk.
    pub token: String,
    /// Human-readable explanation.
    pub message: String,
}

/// A before/after formula delta item for preview.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FormulaDeltaItem {
    /// Sheet + cell, e.g. `"Sheet1!C4"`.
    pub cell: String,
    /// Formula text before the structure edit.
    pub before: String,
    /// Predicted formula text after the structure edit.
    pub after: String,
    /// Classification: `"shifted"`, `"deleted_ref"`, or `"unchanged"`.
    pub classification: String,
    /// Optional warning code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning_code: Option<String>,
}

// ──────────────────────────────────────────────────────────────────
//  Public entry point
// ──────────────────────────────────────────────────────────────────

/// Build an impact report (and optional formula delta preview) by analysing the workbook
/// on disk without mutating it.
///
/// * `impact_report` is always computed when called.
/// * `formula_delta_preview` is only computed when `include_formula_delta` is true.
pub fn compute_structure_impact(
    path: &Path,
    ops: &[StructureOp],
    include_formula_delta: bool,
) -> Result<(StructureImpactReport, Option<Vec<FormulaDeltaItem>>)> {
    let book = crate::xlsx_import::read(path)?;

    // 1. Build shifted spans from ops.
    let shifted_spans = build_shifted_spans(ops)?;

    // 2. Collect all formula cells across sheets.
    let mut tokens_affected: u64 = 0;
    let mut tokens_unaffected: u64 = 0;
    let mut absolute_ref_warnings: Vec<AbsoluteRefWarning> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut formula_deltas: Vec<FormulaDeltaItem> = Vec::new();

    for sheet in book.sheet_collection() {
        let sheet_name = sheet.name().to_string();
        for cell in sheet.cells() {
            if !cell.is_formula() {
                continue;
            }
            let formula_text = cell.formula();
            if formula_text.is_empty() {
                continue;
            }
            let cell_address = cell.coordinate().get_coordinate().to_string();
            let full_cell = format!("{}!{}", sheet_name, cell_address);

            let formula_with_equals = if formula_text.starts_with('=') {
                formula_text.to_string()
            } else {
                format!("={}", formula_text)
            };

            let tokens = match Tokenizer::new(&formula_with_equals) {
                Ok(tok) => tok.items,
                Err(_) => continue,
            };

            let mut cell_affected = false;

            for token in &tokens {
                if token.subtype != formualizer_parse::TokenSubType::Range {
                    continue;
                }
                let value = &token.value;

                // Determine which sheet this reference targets.
                let (ref_sheet, coord_part) = if let Some((sp, cp)) = value.split_once('!') {
                    (extract_sheet_name(sp), cp.to_string())
                } else {
                    // Unqualified ref → belongs to the same sheet as the cell.
                    (sheet_name.clone(), value.clone())
                };

                let relevant_spans: Vec<&ShiftedSpan> = shifted_spans
                    .iter()
                    .filter(|span| span.sheet_name == ref_sheet)
                    .collect();
                if relevant_spans.is_empty() {
                    continue;
                }

                // Count each token at most once as affected/unaffected, even when multiple
                // structure spans target the same sheet in a single batch.
                let mut token_affected = false;
                for span in &relevant_spans {
                    if ref_touches_zone(
                        &coord_part,
                        &span.axis,
                        span.at,
                        span.count,
                        &span.direction,
                    ) {
                        token_affected = true;
                        cell_affected = true;

                        // Check for absolute refs at risk.
                        if has_absolute_component(&coord_part, &span.axis) {
                            let warning_code = if span.direction == "insert" {
                                "ABSOLUTE_REF_CROSS_INSERT"
                            } else {
                                "ABSOLUTE_REF_CROSS_DELETE"
                            };
                            absolute_ref_warnings.push(AbsoluteRefWarning {
                                warning_code: warning_code.to_string(),
                                cell: full_cell.clone(),
                                formula: formula_text.to_string(),
                                token: value.clone(),
                                message: format!(
                                    "Absolute reference '{}' in {} crosses {} zone at {}={}",
                                    value, full_cell, span.direction, span.axis, span.at
                                ),
                            });
                        }
                    }
                }

                if token_affected {
                    tokens_affected += 1;
                    // Check single-cell range non-expansion.
                    if is_single_cell_range(&coord_part) {
                        notes.push(format!(
                            "Single-cell range '{}' in {} will not expand on insert (like SUM(K54:K54))",
                            value, full_cell
                        ));
                    }
                } else {
                    tokens_unaffected += 1;
                }
            }

            // Formula delta preview.
            if include_formula_delta && cell_affected {
                let predicted =
                    simulate_formula_after(&formula_with_equals, &sheet_name, &shifted_spans);
                let before_clean = formula_text.to_string();
                let after_clean = predicted
                    .strip_prefix('=')
                    .unwrap_or(&predicted)
                    .to_string();

                let classification = if after_clean.contains("#REF!") {
                    "deleted_ref"
                } else if before_clean == after_clean {
                    "unchanged"
                } else {
                    "shifted"
                };

                let warning_code = if classification == "deleted_ref" {
                    Some("DELETED_REF".to_string())
                } else {
                    None
                };

                formula_deltas.push(FormulaDeltaItem {
                    cell: full_cell,
                    before: before_clean,
                    after: after_clean,
                    classification: classification.to_string(),
                    warning_code,
                });
            }
        }
    }

    // Deduplicate notes.
    let notes: Vec<String> = {
        let mut seen = BTreeSet::new();
        notes
            .into_iter()
            .filter(|n| seen.insert(n.clone()))
            .collect()
    };

    let report = StructureImpactReport {
        shifted_spans,
        absolute_ref_warnings,
        tokens_affected,
        tokens_unaffected,
        notes,
    };

    // Cap formula delta preview to a reasonable sample (50 items).
    let delta = if include_formula_delta {
        let mut deltas = formula_deltas;
        deltas.truncate(50);
        Some(deltas)
    } else {
        None
    };

    Ok((report, delta))
}

// ──────────────────────────────────────────────────────────────────
//  Internal helpers
// ──────────────────────────────────────────────────────────────────

fn build_shifted_spans(ops: &[StructureOp]) -> Result<Vec<ShiftedSpan>> {
    let mut spans = Vec::new();
    for (idx, op) in ops.iter().enumerate() {
        match op {
            StructureOp::InsertRows {
                sheet_name,
                at_row,
                count,
                ..
            } => {
                spans.push(ShiftedSpan {
                    op_index: idx,
                    sheet_name: sheet_name.clone(),
                    axis: "row".to_string(),
                    description: format!("rows {}..∞ shift +{}", at_row, count),
                    at: *at_row,
                    count: *count,
                    direction: "insert".to_string(),
                });
            }
            StructureOp::DeleteRows {
                sheet_name,
                start_row,
                count,
            } => {
                spans.push(ShiftedSpan {
                    op_index: idx,
                    sheet_name: sheet_name.clone(),
                    axis: "row".to_string(),
                    description: format!(
                        "rows {}..{} deleted, rows {}..∞ shift -{}",
                        start_row,
                        start_row + count - 1,
                        start_row + count,
                        count
                    ),
                    at: *start_row,
                    count: *count,
                    direction: "delete".to_string(),
                });
            }
            StructureOp::InsertCols {
                sheet_name,
                at_col,
                count,
            } => {
                let col_letters = at_col.trim().to_uppercase();
                let col_index =
                    umya_spreadsheet::helper::coordinate::column_index_from_string(&col_letters);
                spans.push(ShiftedSpan {
                    op_index: idx,
                    sheet_name: sheet_name.clone(),
                    axis: "col".to_string(),
                    description: format!("cols {}..∞ shift +{}", col_letters, count),
                    at: col_index,
                    count: *count,
                    direction: "insert".to_string(),
                });
            }
            StructureOp::DeleteCols {
                sheet_name,
                start_col,
                count,
            } => {
                let col_letters = start_col.trim().to_uppercase();
                let col_index =
                    umya_spreadsheet::helper::coordinate::column_index_from_string(&col_letters);
                let end_col_index = col_index + count - 1;
                let end_col_letters =
                    umya_spreadsheet::helper::coordinate::string_from_column_index(end_col_index);
                let next_col_index = col_index + count;
                let next_col_letters =
                    umya_spreadsheet::helper::coordinate::string_from_column_index(next_col_index);
                spans.push(ShiftedSpan {
                    op_index: idx,
                    sheet_name: sheet_name.clone(),
                    axis: "col".to_string(),
                    description: format!(
                        "cols {}..{} deleted, cols {}..∞ shift -{}",
                        col_letters, end_col_letters, next_col_letters, count
                    ),
                    at: col_index,
                    count: *count,
                    direction: "delete".to_string(),
                });
            }
            // Non-row/col ops don't produce shifted spans (rename, create, delete sheet, etc.)
            _ => {}
        }
    }
    Ok(spans)
}

/// Check whether a cell reference (the coordinate part, e.g. `$A$5` or `B2:C10`)
/// intersects the affected zone.
fn ref_touches_zone(coord_part: &str, axis: &str, at: u32, count: u32, direction: &str) -> bool {
    // Parse potential range A1:B2 or single cell.
    let parts: Vec<&str> = coord_part.split(':').collect();
    for part in &parts {
        let (col_idx, row_idx, _, _) =
            umya_spreadsheet::helper::coordinate::index_from_coordinate(part);
        match axis {
            "row" => {
                if let Some(r) = row_idx {
                    if direction == "insert" && r >= at {
                        return true;
                    }
                    if direction == "delete" {
                        let end = at + count - 1;
                        if r >= at && r <= end {
                            return true; // in deleted zone
                        }
                        if r > end {
                            return true; // will be shifted
                        }
                    }
                }
            }
            "col" => {
                if let Some(c) = col_idx {
                    if direction == "insert" && c >= at {
                        return true;
                    }
                    if direction == "delete" {
                        let end = at + count - 1;
                        if c >= at && c <= end {
                            return true;
                        }
                        if c > end {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// Check whether a coordinate part has an absolute `$` component on the relevant axis.
fn has_absolute_component(coord_part: &str, axis: &str) -> bool {
    let parts: Vec<&str> = coord_part.split(':').collect();
    for part in &parts {
        let (_, _, col_lock, row_lock) =
            umya_spreadsheet::helper::coordinate::index_from_coordinate(part);
        match axis {
            "row" => {
                if row_lock == Some(true) {
                    return true;
                }
            }
            "col" => {
                if col_lock == Some(true) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Check if coord_part is a single-cell range like K54:K54.
fn is_single_cell_range(coord_part: &str) -> bool {
    if let Some((start, end)) = coord_part.split_once(':') {
        // Normalize by removing $ for comparison.
        let clean_start = start.replace('$', "").to_uppercase();
        let clean_end = end.replace('$', "").to_uppercase();
        clean_start == clean_end
    } else {
        false
    }
}

/// Extract sheet name from a potentially quoted sheet prefix like `'My Sheet'` or `Sheet1`.
fn extract_sheet_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(stripped) = trimmed.strip_prefix('\'')
        && let Some(inner) = stripped.strip_suffix('\'')
    {
        return inner.replace("''", "'");
    }
    trimmed.to_string()
}

/// Simulate what a formula would look like after applying the given shifted spans.
/// This is a best-effort prediction using the same token-level approach as the real rewriter.
fn simulate_formula_after(
    formula_with_equals: &str,
    cell_sheet: &str,
    spans: &[ShiftedSpan],
) -> String {
    let tokens = match Tokenizer::new(formula_with_equals) {
        Ok(tok) => tok.items,
        Err(_) => return formula_with_equals.to_string(),
    };

    let mut out = String::with_capacity(formula_with_equals.len());
    let mut cursor = 0usize;

    for token in &tokens {
        if token.start > cursor {
            out.push_str(&formula_with_equals[cursor..token.start]);
        }

        let mut value = token.value.clone();
        if token.subtype == formualizer_parse::TokenSubType::Range {
            let (ref_sheet, mut coord_part, prefix) = if let Some((sp, cp)) = value.split_once('!')
            {
                (extract_sheet_name(sp), cp.to_string(), format!("{}!", sp))
            } else {
                (cell_sheet.to_string(), value.clone(), String::new())
            };

            for span in spans {
                if span.sheet_name != ref_sheet {
                    continue;
                }
                coord_part = simulate_adjust_coord(&coord_part, span);
            }
            value = format!("{}{}", prefix, coord_part);
        }

        out.push_str(&value);
        cursor = token.end;
    }

    if cursor < formula_with_equals.len() {
        out.push_str(&formula_with_equals[cursor..]);
    }

    out
}

fn simulate_adjust_coord(coord_part: &str, span: &ShiftedSpan) -> String {
    if coord_part == "#REF!" {
        return coord_part.to_string();
    }
    if let Some((start, end)) = coord_part.split_once(':') {
        let start_adj = simulate_adjust_segment(start, span);
        let end_adj = simulate_adjust_segment(end, span);
        if start_adj == "#REF!" || end_adj == "#REF!" {
            return "#REF!".to_string();
        }
        format!("{}:{}", start_adj, end_adj)
    } else {
        simulate_adjust_segment(coord_part, span)
    }
}

fn simulate_adjust_segment(segment: &str, span: &ShiftedSpan) -> String {
    use umya_spreadsheet::helper::coordinate::{
        coordinate_from_index_with_lock, index_from_coordinate, string_from_column_index,
    };

    let (col, row, col_lock, row_lock) = index_from_coordinate(segment);
    let mut col = col;
    let mut row = row;

    match span.axis.as_str() {
        "col" => {
            if let Some(c) = col {
                if span.direction == "insert" {
                    col = Some(if c >= span.at { c + span.count } else { c });
                } else {
                    // delete
                    let end = span.at + span.count - 1;
                    if c >= span.at && c <= end {
                        col = None; // deleted
                    } else if c > end {
                        col = Some(c - span.count);
                    }
                }
            }
        }
        "row" => {
            if let Some(r) = row {
                if span.direction == "insert" {
                    row = Some(if r >= span.at { r + span.count } else { r });
                } else {
                    let end = span.at + span.count - 1;
                    if r >= span.at && r <= end {
                        row = None;
                    } else if r > end {
                        row = Some(r - span.count);
                    }
                }
            }
        }
        _ => {}
    }

    if col.is_none() && row.is_none() {
        return "#REF!".to_string();
    }

    match (col, row) {
        (Some(c), Some(r)) => coordinate_from_index_with_lock(
            c,
            r,
            col_lock.unwrap_or(false),
            row_lock.unwrap_or(false),
        ),
        (Some(c), None) => {
            let col_str = string_from_column_index(c);
            format!(
                "{}{}",
                if col_lock.unwrap_or(false) { "$" } else { "" },
                col_str
            )
        }
        (None, Some(r)) => {
            format!("{}{}", if row_lock.unwrap_or(false) { "$" } else { "" }, r)
        }
        (None, None) => "#REF!".to_string(),
    }
}

// ──────────────────────────────────────────────────────────────────
//  Tests
// ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_workbook(
        setup: impl FnOnce(&mut umya_spreadsheet::Workbook),
    ) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.xlsx");
        let mut book = umya_spreadsheet::new_file();
        setup(&mut book);
        crate::xlsx_export::write(&book, &path).unwrap();
        dir
    }

    fn wb_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join("test.xlsx")
    }

    #[test]
    fn shifted_spans_for_insert_rows() {
        let ops = vec![StructureOp::InsertRows {
            sheet_name: "Sheet1".to_string(),
            at_row: 5,
            count: 3,
            expand_adjacent_sums: false,
        }];
        let spans = build_shifted_spans(&ops).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].axis, "row");
        assert_eq!(spans[0].at, 5);
        assert_eq!(spans[0].count, 3);
        assert_eq!(spans[0].direction, "insert");
        assert!(spans[0].description.contains("shift +3"));
    }

    #[test]
    fn shifted_spans_for_delete_cols() {
        let ops = vec![StructureOp::DeleteCols {
            sheet_name: "Data".to_string(),
            start_col: "C".to_string(),
            count: 2,
        }];
        let spans = build_shifted_spans(&ops).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].axis, "col");
        assert_eq!(spans[0].direction, "delete");
        assert!(spans[0].description.contains("deleted"));
    }

    #[test]
    fn impact_report_detects_affected_formulas() {
        let tmp = create_test_workbook(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.cell_mut("A1").set_value_number(10);
            sheet.cell_mut("A2").set_value_number(20);
            sheet.cell_mut("B1").set_formula("A1+A2".to_string());
            sheet.cell_mut("C1").set_formula("$A$5".to_string());
        });

        let ops = vec![StructureOp::InsertRows {
            sheet_name: "Sheet1".to_string(),
            at_row: 2,
            count: 1,
            expand_adjacent_sums: false,
        }];

        let (report, _) = compute_structure_impact(&wb_path(&tmp), &ops, false).unwrap();
        assert_eq!(report.shifted_spans.len(), 1);
        // A2 reference in B1 formula should be affected (row 2 >= at_row 2).
        assert!(report.tokens_affected > 0);
    }

    #[test]
    fn impact_report_flags_absolute_ref_crossing_insert() {
        let tmp = create_test_workbook(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet
                .cell_mut("A1")
                .set_formula("$A$5+$A$10".to_string());
        });

        let ops = vec![StructureOp::InsertRows {
            sheet_name: "Sheet1".to_string(),
            at_row: 3,
            count: 2,
            expand_adjacent_sums: false,
        }];

        let (report, _) = compute_structure_impact(&wb_path(&tmp), &ops, false).unwrap();
        assert!(
            !report.absolute_ref_warnings.is_empty(),
            "should flag absolute refs crossing insert zone"
        );
        assert!(
            report
                .absolute_ref_warnings
                .iter()
                .any(|w| w.warning_code == "ABSOLUTE_REF_CROSS_INSERT")
        );
    }

    #[test]
    fn formula_delta_preview_shows_before_after() {
        let tmp = create_test_workbook(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.cell_mut("A1").set_value_number(10);
            sheet.cell_mut("A5").set_value_number(50);
            sheet.cell_mut("B1").set_formula("A5*2".to_string());
        });

        let ops = vec![StructureOp::InsertRows {
            sheet_name: "Sheet1".to_string(),
            at_row: 3,
            count: 2,
            expand_adjacent_sums: false,
        }];

        let (_report, delta) = compute_structure_impact(&wb_path(&tmp), &ops, true).unwrap();
        let delta = delta.expect("delta should be present");
        assert!(!delta.is_empty(), "should have at least one delta");

        let b1_delta = delta.iter().find(|d| d.cell == "Sheet1!B1");
        assert!(b1_delta.is_some(), "B1 should have a delta");
        let item = b1_delta.unwrap();
        assert_eq!(item.before, "A5*2");
        assert_eq!(item.after, "A7*2"); // row 5 shifted by +2
        assert_eq!(item.classification, "shifted");
    }

    #[test]
    fn token_counts_do_not_double_count_across_multiple_spans() {
        let tmp = create_test_workbook(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.cell_mut("B1").set_formula("A5*2".to_string());
        });

        let ops = vec![
            StructureOp::InsertRows {
                sheet_name: "Sheet1".to_string(),
                at_row: 2,
                count: 1,
                expand_adjacent_sums: false,
            },
            StructureOp::InsertRows {
                sheet_name: "Sheet1".to_string(),
                at_row: 4,
                count: 1,
                expand_adjacent_sums: false,
            },
        ];

        let (report, delta) = compute_structure_impact(&wb_path(&tmp), &ops, true).unwrap();
        assert_eq!(
            report.tokens_affected, 1,
            "single range token should count once"
        );
        assert_eq!(report.tokens_unaffected, 0);

        let delta = delta.expect("delta should be present");
        let item = delta
            .iter()
            .find(|d| d.cell == "Sheet1!B1")
            .expect("B1 delta");
        assert_eq!(item.before, "A5*2");
        assert_eq!(item.after, "A7*2"); // sequential +1 at row>=2 then +1 at row>=4
    }

    #[test]
    fn no_mutation_occurs_during_preview() {
        let tmp = create_test_workbook(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet.cell_mut("A1").set_value_number(42);
            sheet.cell_mut("B1").set_formula("A1*2".to_string());
        });

        let before_bytes = std::fs::read(wb_path(&tmp)).unwrap();

        let ops = vec![StructureOp::InsertRows {
            sheet_name: "Sheet1".to_string(),
            at_row: 1,
            count: 5,
            expand_adjacent_sums: false,
        }];

        let _ = compute_structure_impact(&wb_path(&tmp), &ops, true).unwrap();

        let after_bytes = std::fs::read(wb_path(&tmp)).unwrap();
        assert_eq!(
            before_bytes, after_bytes,
            "preview must not mutate the file"
        );
    }

    #[test]
    fn single_cell_range_noted() {
        let tmp = create_test_workbook(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().unwrap();
            sheet
                .cell_mut("B1")
                .set_formula("SUM(K54:K54)".to_string());
        });

        let ops = vec![StructureOp::InsertRows {
            sheet_name: "Sheet1".to_string(),
            at_row: 50,
            count: 1,
            expand_adjacent_sums: false,
        }];

        let (report, _) = compute_structure_impact(&wb_path(&tmp), &ops, false).unwrap();
        assert!(
            report.notes.iter().any(|n| n.contains("Single-cell range")),
            "should note single-cell range non-expansion"
        );
    }
}
