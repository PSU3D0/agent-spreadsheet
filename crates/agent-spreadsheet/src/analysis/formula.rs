use crate::model::{
    FORMULA_PARSE_FAILED_PREFIX, FormulaGroup, FormulaParseDiagnosticsBuilder, FormulaParsePolicy,
    format_formula_parse_failure,
};
use crate::utils::column_number_to_name;
use anyhow::{Context, Result, anyhow};
use formualizer_parse::{
    ASTNode,
    parser::{BatchParser, CollectPolicy, ReferenceType},
    pretty::canonical_formula,
};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use umya_spreadsheet::{CellFormulaValues, Worksheet};

const RANGE_EXPANSION_LIMIT: usize = 500;

#[derive(Clone)]
pub struct FormulaAtlas {
    parser: Arc<Mutex<BatchParser>>,
    cache: Arc<RwLock<HashMap<String, Arc<ParsedFormula>>>>,
    _volatility: Arc<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct ParsedFormula {
    pub fingerprint: String,
    pub canonical: String,
    pub is_volatile: bool,
    pub dependencies: Vec<String>,
}

impl FormulaAtlas {
    pub fn new(volatility_functions: Vec<String>) -> Self {
        let normalized: Vec<String> = volatility_functions
            .into_iter()
            .map(|s| s.to_ascii_uppercase())
            .collect();
        let lookup = Arc::new(normalized);
        let closure_lookup = lookup.clone();
        let parser = BatchParser::builder()
            .with_volatility_classifier(move |name| {
                let upper = name.to_ascii_uppercase();
                closure_lookup.iter().any(|entry| entry == &upper)
            })
            .build();
        Self {
            parser: Arc::new(Mutex::new(parser)),
            cache: Arc::new(RwLock::new(HashMap::new())),
            _volatility: lookup,
        }
    }

    pub fn parse(&self, formula: &str) -> Result<Arc<ParsedFormula>> {
        if let Some(existing) = self.cache.read().get(formula) {
            return Ok(existing.clone());
        }

        let ast = {
            let mut parser = self.parser.lock();
            parser
                .parse(formula)
                .with_context(|| format!("failed to parse formula: {formula}"))?
        };
        let parsed = Arc::new(parsed_from_ast(&ast));

        self.cache
            .write()
            .insert(formula.to_string(), parsed.clone());
        Ok(parsed)
    }
}

impl Default for FormulaAtlas {
    fn default() -> Self {
        Self::new(default_volatility_functions())
    }
}

fn unescape_formula_string(s: &str) -> String {
    s.replace("\"\"", "\"")
}

fn parsed_from_ast(ast: &ASTNode) -> ParsedFormula {
    let fingerprint = format!("{:016x}", ast.fingerprint());
    let canonical = unescape_formula_string(&canonical_formula(ast));
    let dependencies = ast
        .get_dependencies()
        .iter()
        .map(|reference| reference_to_string(reference))
        .collect();
    ParsedFormula {
        fingerprint,
        canonical,
        is_volatile: ast.contains_volatile(),
        dependencies,
    }
}

pub struct FormulaGraph {
    precedents: HashMap<String, Vec<String>>,
    dependents: HashMap<String, Vec<String>>,
    groups: HashMap<String, FormulaGroupAccumulator>,
    range_dependents: Vec<RangeDependentEntry>,
    sheet_name: String,
}

#[derive(Debug, Clone)]
struct RangeDependentEntry {
    #[allow(dead_code)]
    range_key: String,
    reference: ReferenceType,
    dependents: Vec<String>,
}

impl FormulaGraph {
    pub fn build(
        sheet: &Worksheet,
        atlas: &FormulaAtlas,
        policy: FormulaParsePolicy,
        mut diagnostics: Option<&mut FormulaParseDiagnosticsBuilder>,
    ) -> Result<Self> {
        let sheet_name = sheet.name().to_string();
        let mut precedents_build: HashMap<String, HashSet<String>> = HashMap::new();
        let mut dependents_build: HashMap<String, HashSet<String>> = HashMap::new();
        let mut groups: HashMap<String, FormulaGroupAccumulator> = HashMap::new();
        let mut range_dependents_build: HashMap<String, (ReferenceType, HashSet<String>)> =
            HashMap::new();

        let collect_policy = CollectPolicy {
            expand_small_ranges: true,
            range_expansion_limit: RANGE_EXPANSION_LIMIT,
            include_names: true,
        };

        for cell in sheet.cells() {
            if !cell.is_formula() {
                continue;
            }
            let coordinate = cell.coordinate();
            let address = coordinate.get_coordinate();
            let formula_text = cell.formula();
            if formula_text.is_empty() {
                continue;
            }
            let formula_with_prefix = if formula_text.starts_with('=') {
                formula_text.to_string()
            } else {
                format!("={}", formula_text)
            };

            let ast = {
                let mut parser = atlas.parser.lock();
                match parser.parse(&formula_with_prefix) {
                    Ok(ast) => Some(ast),
                    Err(batch_err) => {
                        let fallback = formualizer_parse::parse(&formula_with_prefix);
                        match fallback {
                            Ok(ast) => Some(ast),
                            Err(fallback_err) => {
                                let parse_error = format_formula_parse_failure(
                                    &formula_with_prefix,
                                    &fallback_err,
                                );

                                tracing::warn!(
                                    sheet = %sheet_name,
                                    address = %address,
                                    formula = %formula_with_prefix,
                                    batch_error = %batch_err,
                                    fallback_error = %parse_error,
                                    "skipping unparsable formula cell"
                                );

                                match policy {
                                    FormulaParsePolicy::Off => None,
                                    FormulaParsePolicy::Warn => {
                                        if let Some(diag) = diagnostics.as_deref_mut() {
                                            diag.record_error(
                                                &sheet_name,
                                                &address,
                                                &formula_with_prefix,
                                                &parse_error,
                                            );
                                        }
                                        None
                                    }
                                    FormulaParsePolicy::Fail => {
                                        if let Some(diag) = diagnostics.as_deref_mut() {
                                            diag.record_error(
                                                &sheet_name,
                                                &address,
                                                &formula_with_prefix,
                                                &parse_error,
                                            );
                                        }
                                        return Err(anyhow!(
                                            "{}sheet '{}' cell {}: {}",
                                            FORMULA_PARSE_FAILED_PREFIX,
                                            sheet_name,
                                            address,
                                            parse_error
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            };

            let Some(ast) = ast else {
                continue;
            };

            let fingerprint = format!("{:016x}", ast.fingerprint());
            let canonical = unescape_formula_string(&canonical_formula(&ast));
            let is_volatile = ast.contains_volatile();

            let (is_array, is_shared_type) = cell
                .formula_obj()
                .map(|obj| match obj.formula_type() {
                    CellFormulaValues::Array => (true, false),
                    CellFormulaValues::Shared => (false, true),
                    _ => (false, false),
                })
                .unwrap_or((false, false));

            let group =
                groups
                    .entry(fingerprint.clone())
                    .or_insert_with(|| FormulaGroupAccumulator {
                        canonical: canonical.clone(),
                        addresses: Vec::new(),
                        is_volatile,
                        is_array,
                        is_shared: is_shared_type,
                    });
            if cell.formula_shared_index().is_some() {
                group.is_shared = true;
            }
            group.addresses.push(address.clone());
            group.is_volatile |= is_volatile;

            let refs = ast.collect_references(&collect_policy);
            for reference in refs {
                match &reference {
                    ReferenceType::Cell {
                        sheet, row, col, ..
                    } => {
                        let dep_addr = format_cell_address(sheet.as_deref(), *row, *col);
                        precedents_build
                            .entry(address.clone())
                            .or_default()
                            .insert(dep_addr.clone());
                        dependents_build
                            .entry(dep_addr)
                            .or_default()
                            .insert(address.clone());
                    }
                    ReferenceType::Range {
                        start_row,
                        start_col,
                        end_row,
                        end_col,
                        ..
                    } => {
                        let prec_str = reference.to_string();
                        precedents_build
                            .entry(address.clone())
                            .or_default()
                            .insert(prec_str.clone());

                        if is_large_or_infinite_range(*start_row, *start_col, *end_row, *end_col) {
                            range_dependents_build
                                .entry(prec_str)
                                .or_insert_with(|| (reference.clone(), HashSet::new()))
                                .1
                                .insert(address.clone());
                        }
                    }
                    ReferenceType::NamedRange(name) => {
                        precedents_build
                            .entry(address.clone())
                            .or_default()
                            .insert(name.clone());
                    }
                    ReferenceType::Table(_) | ReferenceType::External(_) => {
                        let table_str = reference.to_string();
                        precedents_build
                            .entry(address.clone())
                            .or_default()
                            .insert(table_str);
                    }
                    // 3D references span multiple sheets; track them as opaque
                    // precedent strings (same treatment as tables/external refs).
                    ReferenceType::Cell3D { .. } | ReferenceType::Range3D { .. } => {
                        let ref_str = reference.to_string();
                        precedents_build
                            .entry(address.clone())
                            .or_default()
                            .insert(ref_str);
                    }
                }
            }
        }

        let precedents = precedents_build
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().collect()))
            .collect();
        let dependents = dependents_build
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().collect()))
            .collect();
        let range_dependents = range_dependents_build
            .into_iter()
            .map(|(key, (ref_type, addrs))| RangeDependentEntry {
                range_key: key,
                reference: ref_type,
                dependents: addrs.into_iter().collect(),
            })
            .collect();

        Ok(Self {
            precedents,
            dependents,
            groups,
            range_dependents,
            sheet_name,
        })
    }

    pub fn groups(&self) -> Vec<FormulaGroup> {
        self.groups
            .iter()
            .map(|(fingerprint, group)| FormulaGroup {
                fingerprint: fingerprint.clone(),
                addresses: group.addresses.clone(),
                count: Some(group.addresses.len() as u32),
                formula: group.canonical.clone(),
                is_array: group.is_array,
                is_shared: group.is_shared,
                is_volatile: group.is_volatile,
            })
            .collect()
    }

    pub fn precedents(&self, address: &str) -> Vec<String> {
        self.precedents.get(address).cloned().unwrap_or_default()
    }

    pub fn dependents(&self, address: &str) -> Vec<String> {
        self.dependents_limited(address, None).0
    }

    /// Returns cells that depend on the given address, with optional limit.
    ///
    /// Returns (dependents, was_truncated). If limit is Some and exceeded,
    /// was_truncated is true and only limit dependents are returned.
    ///
    /// Performance: O(n) where n = number of large range references in the sheet.
    /// Early exits when limit reached to keep response times bounded.
    pub fn dependents_limited(&self, address: &str, limit: Option<usize>) -> (Vec<String>, bool) {
        let mut result = self.dependents.get(address).cloned().unwrap_or_default();
        let limit = limit.unwrap_or(usize::MAX);

        if result.len() >= limit {
            result.truncate(limit);
            return (result, true);
        }

        if let Some((row, col)) = parse_cell_address(address) {
            let (query_sheet, _) = split_sheet_prefix(address);
            'outer: for entry in &self.range_dependents {
                if range_contains_cell(&entry.reference, query_sheet, &self.sheet_name, row, col) {
                    for addr in &entry.dependents {
                        if !result.contains(addr) {
                            result.push(addr.clone());
                            if result.len() >= limit {
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }

        let truncated = result.len() >= limit;
        (result, truncated)
    }
}

fn format_cell_address(sheet: Option<&str>, row: u32, col: u32) -> String {
    let col_str = column_number_to_name(col);
    match sheet {
        Some(s) => format!("{}!{}{}", s, col_str, row),
        None => format!("{}{}", col_str, row),
    }
}

fn is_large_or_infinite_range(
    start_row: Option<u32>,
    start_col: Option<u32>,
    end_row: Option<u32>,
    end_col: Option<u32>,
) -> bool {
    match (start_row, start_col, end_row, end_col) {
        (Some(sr), Some(sc), Some(er), Some(ec)) => {
            let rows = er.saturating_sub(sr) + 1;
            let cols = ec.saturating_sub(sc) + 1;
            (rows as usize) * (cols as usize) > RANGE_EXPANSION_LIMIT
        }
        _ => true,
    }
}

fn range_contains_cell(
    range: &ReferenceType,
    query_sheet: Option<&str>,
    current_sheet: &str,
    row: u32,
    col: u32,
) -> bool {
    match range {
        ReferenceType::Range {
            sheet: range_sheet,
            start_row,
            start_col,
            end_row,
            end_col,
            ..
        } => {
            let range_sheet_name = range_sheet.as_deref().unwrap_or(current_sheet);
            let query_sheet_name = query_sheet.unwrap_or(current_sheet);
            if !range_sheet_name.eq_ignore_ascii_case(query_sheet_name) {
                return false;
            }
            let row_ok = match (start_row, end_row) {
                (Some(sr), Some(er)) => row >= *sr && row <= *er,
                (Some(sr), None) => row >= *sr,
                (None, Some(er)) => row <= *er,
                (None, None) => true,
            };
            let col_ok = match (start_col, end_col) {
                (Some(sc), Some(ec)) => col >= *sc && col <= *ec,
                (Some(sc), None) => col >= *sc,
                (None, Some(ec)) => col <= *ec,
                (None, None) => true,
            };
            row_ok && col_ok
        }
        _ => false,
    }
}

fn parse_cell_address(address: &str) -> Option<(u32, u32)> {
    let (_, cell_part) = split_sheet_prefix(address);
    let cell_part = cell_part.trim_start_matches('$');

    let mut col_str = String::new();
    let mut row_str = String::new();

    for ch in cell_part.chars() {
        if ch == '$' {
            continue;
        }
        if ch.is_ascii_alphabetic() && row_str.is_empty() {
            col_str.push(ch.to_ascii_uppercase());
        } else if ch.is_ascii_digit() {
            row_str.push(ch);
        }
    }

    if col_str.is_empty() || row_str.is_empty() {
        return None;
    }

    let col = column_name_to_number(&col_str)?;
    let row: u32 = row_str.parse().ok()?;
    Some((row, col))
}

fn column_name_to_number(name: &str) -> Option<u32> {
    let mut result: u32 = 0;
    for ch in name.chars() {
        if !ch.is_ascii_alphabetic() {
            return None;
        }
        result = result * 26 + (ch.to_ascii_uppercase() as u32 - 'A' as u32 + 1);
    }
    Some(result)
}

fn split_sheet_prefix(address: &str) -> (Option<&str>, &str) {
    if let Some(idx) = address.find('!') {
        let sheet = &address[..idx];
        let sheet = sheet.trim_start_matches('\'').trim_end_matches('\'');
        let cell = &address[idx + 1..];
        (Some(sheet), cell)
    } else {
        (None, address)
    }
}

struct FormulaGroupAccumulator {
    canonical: String,
    addresses: Vec<String>,
    is_volatile: bool,
    is_array: bool,
    is_shared: bool,
}

fn reference_to_string(reference: &ReferenceType) -> String {
    reference.to_string()
}

pub fn normalize_cell_reference(sheet_name: &str, row: u32, col: u32) -> String {
    format!("{}!{}{}", sheet_name, column_number_to_name(col), row)
}

fn default_volatility_functions() -> Vec<String> {
    vec![
        "NOW",
        "TODAY",
        "RAND",
        "RANDBETWEEN",
        "OFFSET",
        "INDIRECT",
        "INFO",
        "CELL",
        "AREAS",
        "INDEX",
        "MOD",
        "ROW",
        "COLUMN",
        "ROWS",
        "COLUMNS",
        "HYPERLINK",
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect()
}
