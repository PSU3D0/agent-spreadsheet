use crate::model::{CellValue, ColumnSummary};
use crate::utils::column_number_to_name;
use crate::workbook::cell_to_value;
use std::collections::{BTreeMap, HashSet};
use umya_spreadsheet::Worksheet;

pub struct SheetStats {
    pub numeric_columns: Vec<ColumnSummary>,
    pub text_columns: Vec<ColumnSummary>,
    pub null_counts: BTreeMap<String, u32>,
    pub duplicate_warnings: Vec<String>,
    pub density: f32,
}

pub fn compute_sheet_statistics(sheet: &Worksheet, _sample_rows: usize) -> SheetStats {
    let (max_col, max_row) = sheet.highest_column_and_row();
    if max_col == 0 || max_row == 0 {
        return SheetStats {
            numeric_columns: Vec::new(),
            text_columns: Vec::new(),
            null_counts: BTreeMap::new(),
            duplicate_warnings: Vec::new(),
            density: 0.0,
        };
    }

    let mut numeric = Vec::new();
    let mut text = Vec::new();
    let mut null_counts = BTreeMap::new();
    let mut duplicate_warnings = Vec::new();
    let mut filled_cells = 0u32;

    for col in 1..=max_col {
        let column_name = column_number_to_name(col);
        let header = sheet.cell((1u32, col)).and_then(cell_to_value);
        let mut numeric_values = Vec::new();
        let mut text_values = Vec::new();
        let mut samples = Vec::new();
        let mut unique_values: HashSet<String> = HashSet::new();
        let mut duplicate_flag = false;

        for row in 1..=max_row {
            if let Some(cell) = sheet.cell((row, col))
                && let Some(value) = cell_to_value(cell)
            {
                filled_cells += 1;
                match value.clone() {
                    CellValue::Number(n) => numeric_values.push(n),
                    CellValue::Bool(_)
                    | CellValue::Text(_)
                    | CellValue::Date(_)
                    | CellValue::Error(_) => {
                        if let CellValue::Text(ref s) = value
                            && !unique_values.insert(s.clone())
                        {
                            duplicate_flag = true;
                        }
                        text_values.push(value.clone());
                    }
                }
                if samples.len() < 5 {
                    samples.push(value);
                }
            }
        }

        let nulls = max_row - (numeric_values.len() as u32 + text_values.len() as u32);
        if nulls > 0 {
            null_counts.insert(column_name.clone(), nulls);
        }

        if duplicate_flag {
            duplicate_warnings.push(format!("Column {column_name} contains duplicate values"));
        }

        let summary = ColumnSummary {
            header: header.map(cell_value_to_string),
            column: column_name.clone(),
            samples,
            min: if numeric_values.is_empty() {
                None
            } else {
                numeric_values.iter().cloned().reduce(f64::min)
            },
            max: if numeric_values.is_empty() {
                None
            } else {
                numeric_values.iter().cloned().reduce(f64::max)
            },
            mean: if numeric_values.is_empty() {
                None
            } else {
                Some(numeric_values.iter().sum::<f64>() / numeric_values.len() as f64)
            },
        };

        if numeric_values.len() >= text_values.len() {
            numeric.push(summary);
        } else if !text_values.is_empty() {
            text.push(summary);
        }
    }

    let total_cells = (max_col * max_row) as f32;
    let density = if total_cells == 0.0 {
        0.0
    } else {
        filled_cells as f32 / total_cells
    };

    SheetStats {
        numeric_columns: numeric,
        text_columns: text,
        null_counts,
        duplicate_warnings,
        density,
    }
}
fn cell_value_to_string(value: CellValue) -> String {
    match value {
        CellValue::Text(s) => s,
        CellValue::Number(n) => format!("{n}"),
        CellValue::Bool(b) => b.to_string(),
        CellValue::Date(d) => d,
        CellValue::Error(e) => e,
    }
}
