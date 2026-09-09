use umya_spreadsheet::{NumberingFormat, Worksheet};

#[derive(Clone, Debug)]
pub enum CellVal {
    Text(String),
    Num(f64),
    Date(f64),
    Formula(String),
    Empty,
}

impl From<&str> for CellVal {
    fn from(s: &str) -> Self {
        CellVal::Text(s.to_string())
    }
}

impl From<f64> for CellVal {
    fn from(n: f64) -> Self {
        CellVal::Num(n)
    }
}

impl From<i32> for CellVal {
    fn from(n: i32) -> Self {
        CellVal::Num(n as f64)
    }
}

fn col_index(col_letter: &str) -> u32 {
    let mut result = 0u32;
    for c in col_letter.chars() {
        result = result * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1);
    }
    result
}

fn parse_cell_ref(cell_ref: &str) -> (u32, u32) {
    let mut col_part = String::new();
    let mut row_part = String::new();
    for c in cell_ref.chars() {
        if c.is_ascii_alphabetic() {
            col_part.push(c);
        } else {
            row_part.push(c);
        }
    }
    let col = col_index(&col_part);
    let row: u32 = row_part.parse().unwrap_or(1);
    (col, row)
}

pub fn set_cell(sheet: &mut Worksheet, col: u32, row: u32, val: &CellVal) {
    match val {
        CellVal::Text(s) => {
            sheet.cell_mut((col, row)).set_value(s.clone());
        }
        CellVal::Num(n) => {
            sheet.cell_mut((col, row)).set_value_number(*n);
        }
        CellVal::Date(serial) => {
            sheet.cell_mut((col, row)).set_value_number(*serial);
            sheet
                .style_mut((col, row))
                .number_format_mut()
                .set_format_code(NumberingFormat::FORMAT_DATE_YYYYMMDD2);
        }
        CellVal::Formula(f) => {
            sheet.cell_mut((col, row)).set_formula(f.clone());
        }
        CellVal::Empty => {}
    }
}

pub fn fill_table<H, R, V>(sheet: &mut Worksheet, start: &str, headers: &[H], rows: &[R])
where
    H: AsRef<str>,
    R: AsRef<[V]>,
    V: Into<CellVal> + Clone,
{
    let (start_col, start_row) = parse_cell_ref(start);

    for (i, header) in headers.iter().enumerate() {
        let col = start_col + i as u32;
        sheet
            .cell_mut((col, start_row))
            .set_value(header.as_ref().to_string());
        let style = sheet.style_mut((col, start_row));
        style.font_mut().set_bold(true);
    }

    for (row_idx, row_data) in rows.iter().enumerate() {
        let row = start_row + 1 + row_idx as u32;
        for (col_idx, val) in row_data.as_ref().iter().enumerate() {
            let col = start_col + col_idx as u32;
            let cell_val: CellVal = val.clone().into();
            set_cell(sheet, col, row, &cell_val);
        }
    }
}

pub fn fill_key_value<K, V>(sheet: &mut Worksheet, start: &str, pairs: &[(K, V)])
where
    K: AsRef<str>,
    V: Into<CellVal> + Clone,
{
    let (start_col, start_row) = parse_cell_ref(start);

    for (i, (key, val)) in pairs.iter().enumerate() {
        let row = start_row + i as u32;
        sheet
            .cell_mut((start_col, row))
            .set_value(key.as_ref().to_string());
        let cell_val: CellVal = val.clone().into();
        set_cell(sheet, start_col + 1, row, &cell_val);
    }
}

pub fn fill_horizontal_kv<K, V>(sheet: &mut Worksheet, start: &str, pairs: &[(K, V)])
where
    K: AsRef<str>,
    V: Into<CellVal> + Clone,
{
    let (start_col, start_row) = parse_cell_ref(start);

    for (i, (key, val)) in pairs.iter().enumerate() {
        let col = start_col + i as u32;
        sheet
            .cell_mut((col, start_row))
            .set_value(key.as_ref().to_string());
        let cell_val: CellVal = val.clone().into();
        set_cell(sheet, col, start_row + 1, &cell_val);
    }
}

pub fn fill_sparse(sheet: &mut Worksheet, cells: &[(&str, CellVal)]) {
    for (cell_ref, val) in cells {
        let (col, row) = parse_cell_ref(cell_ref);
        set_cell(sheet, col, row, val);
    }
}

pub fn fill_formula_grid(
    sheet: &mut Worksheet,
    start: &str,
    rows: u32,
    cols: u32,
    formula_fn: impl Fn(u32, u32) -> String,
) {
    let (start_col, start_row) = parse_cell_ref(start);

    for r in 0..rows {
        for c in 0..cols {
            let row = start_row + r;
            let col = start_col + c;
            let formula = formula_fn(row, col);
            sheet.cell_mut((col, row)).set_formula(formula);
        }
    }
}

pub fn set_header_style(sheet: &mut Worksheet, range: &str) {
    let parts: Vec<&str> = range.split(':').collect();
    let (start_col, start_row) = parse_cell_ref(parts[0]);
    let (end_col, end_row) = if parts.len() > 1 {
        parse_cell_ref(parts[1])
    } else {
        (start_col, start_row)
    };

    for row in start_row..=end_row {
        for col in start_col..=end_col {
            let style = sheet.style_mut((col, row));
            style.font_mut().set_bold(true);
        }
    }
}

pub fn apply_date_format(sheet: &mut Worksheet, range: &str) {
    let parts: Vec<&str> = range.split(':').collect();
    let (start_col, start_row) = parse_cell_ref(parts[0]);
    let (end_col, end_row) = if parts.len() > 1 {
        parse_cell_ref(parts[1])
    } else {
        (start_col, start_row)
    };

    for row in start_row..=end_row {
        for col in start_col..=end_col {
            sheet
                .style_mut((col, row))
                .number_format_mut()
                .set_format_code(NumberingFormat::FORMAT_DATE_YYYYMMDD2);
        }
    }
}
