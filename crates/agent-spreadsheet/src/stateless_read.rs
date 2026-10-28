//! Read-only file inspection and engine-authoritative navigation, without evaluation.
use formualizer::{
    CellAddress, CellSnapshot, NameResolution, OmittedCount, SemanticReference, TraceDirection,
    TraceOptions, TruncationReport,
    common::{LiteralValue, parse_a1_1based},
    workbook::{LoadStrategy, NativeXlsxAdapter, SpreadsheetReader, Workbook, WorkbookConfig},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("invalid read request: {0}")]
    Invalid(String),
    #[error("stale or mismatched request fingerprint; restart from offset zero")]
    Stale,
    #[error("native inspection failed: {0}")]
    Native(String),
}
impl ReadError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "INVALID_ARGUMENT",
            Self::Stale => "STALE_CURSOR",
            Self::Native(_) => "INSPECTION_FAILED",
        }
    }
}
fn invalid(e: impl ToString) -> ReadError {
    ReadError::Invalid(e.to_string())
}
fn native(e: impl ToString) -> ReadError {
    ReadError::Native(e.to_string())
}
fn twenty() -> usize {
    20
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    #[serde(default)]
    pub offset: u64,
    #[serde(default = "twenty")]
    pub limit: usize,
    #[serde(default)]
    pub expected_fingerprint: Option<String>,
}
impl Default for Page {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 20,
            expected_fingerprint: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    Cells { addresses: Vec<String> },
    Range { address: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
    pub selection: Selection,
    #[serde(default)]
    pub page: Page,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceLimits {
    #[serde(default)]
    pub max_depth: u32,
    #[serde(default = "nodes")]
    pub max_nodes: u32,
    #[serde(default = "links")]
    pub max_links: u32,
    #[serde(default = "work")]
    pub max_work: u64,
    #[serde(default)]
    pub range_member_budget: u32,
}
fn nodes() -> u32 {
    64
}
fn links() -> u32 {
    128
}
fn work() -> u64 {
    10_000
}
impl Default for TraceLimits {
    fn default() -> Self {
        Self {
            max_depth: 0,
            max_nodes: nodes(),
            max_links: links(),
            max_work: work(),
            range_member_budget: 0,
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Precedents,
    Dependents,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceRequest {
    pub roots: Vec<String>,
    #[serde(default)]
    pub details: bool,
    #[serde(default)]
    pub direction: Direction,
    #[serde(default)]
    pub limits: TraceLimits,
    #[serde(default)]
    pub page: Page,
}
#[derive(Debug, Serialize)]
pub struct SourceCell {
    pub address: String,
    pub formula: Option<String>,
    pub source_value: Option<Value>,
    pub stored_formula_cache: Option<StoredCache>,
}
#[derive(Debug, Serialize)]
pub struct StoredCache {
    pub value: Option<Value>,
    pub freshness: &'static str,
}
#[derive(Debug, Serialize)]
pub struct ReadReport {
    pub input_sha256: String,
    pub fingerprint: String,
    pub mode: &'static str,
    pub evaluated: bool,
    pub total_page_items: u64,
    pub remaining_page_items: u64,
    pub items: Vec<Value>,
    pub coverage: Value,
    pub hints: Vec<Value>,
    pub next_request: Option<Value>,
}
fn fingerprint(
    input: &[u8],
    scope: &impl Serialize,
    page: &Page,
) -> Result<(String, String), ReadError> {
    if page.limit == 0 {
        return Err(invalid("page.limit must be positive"));
    }
    let sha = crate::utils::hash_bytes_sha256_hex(input);
    let fp = crate::utils::hash_bytes_sha256_hex(
        format!(
            "read-v1:{sha}:{}",
            serde_json::to_string(scope).map_err(invalid)?
        )
        .as_bytes(),
    );
    if page
        .expected_fingerprint
        .as_ref()
        .is_some_and(|expected| expected != &fp)
        || (page.offset > 0 && page.expected_fingerprint.is_none())
    {
        return Err(ReadError::Stale);
    }
    Ok((sha, fp))
}
fn split_address(text: &str, sheets: &[String]) -> Result<(String, String), ReadError> {
    let (sheet, address) = text
        .rsplit_once('!')
        .ok_or_else(|| invalid("use a sheet-qualified address, e.g. 'My Sheet'!A1"))?;
    let sheet = if sheet.starts_with('\'') && sheet.ends_with('\'') && sheet.len() >= 2 {
        sheet[1..sheet.len() - 1].replace("''", "'")
    } else {
        sheet.to_string()
    };
    let canonical = sheets
        .iter()
        .find(|s| s.eq_ignore_ascii_case(&sheet))
        .ok_or_else(|| invalid(format!("unknown sheet {sheet:?}")))?;
    Ok((canonical.clone(), address.to_string()))
}
fn cell(text: &str, sheets: &[String]) -> Result<CellAddress, ReadError> {
    let (sheet, a1) = split_address(text, sheets)?;
    let (row, col, _, _) = parse_a1_1based(&a1).map_err(invalid)?;
    CellAddress::new(sheet, row, col).map_err(invalid)
}
fn source(reader: &mut NativeXlsxAdapter, address: &CellAddress) -> Result<SourceCell, ReadError> {
    let data = reader
        .read_cell(&address.sheet, address.row, address.column)
        .map_err(native)?;
    let formula = data.as_ref().and_then(|c| c.formula.clone());
    let value = data.and_then(|c| c.value).as_ref().map(literal);
    let is_formula = formula.is_some();
    Ok(SourceCell {
        address: address.to_string(),
        formula,
        source_value: if is_formula { None } else { value.clone() },
        stored_formula_cache: is_formula.then_some(StoredCache {
            value,
            freshness: "unverified",
        }),
    })
}
pub fn inspect_xlsx_bytes(input: &[u8], request: InspectRequest) -> Result<ReadReport, ReadError> {
    let mut scope = request.clone();
    scope.page.offset = 0;
    scope.page.expected_fingerprint = None;
    let (sha, fp) = fingerprint(input, &scope, &request.page)?;
    let mut reader = NativeXlsxAdapter::open_bytes(input.to_vec()).map_err(native)?;
    let sheets = reader.sheet_names().map_err(native)?;
    let addresses;
    let (total, range) = match &request.selection {
        Selection::Cells { addresses: raw } => {
            if raw.is_empty() {
                return Err(invalid("addresses must not be empty"));
            }
            let mut sorted = std::collections::BTreeMap::new();
            for raw in raw {
                let c = cell(raw, &sheets)?;
                sorted.insert((c.sheet.clone(), c.row, c.column), c);
            }
            addresses = sorted.into_values().collect::<Vec<_>>();
            (addresses.len() as u64, None)
        }
        Selection::Range { address } => {
            let (sheet, area) = split_address(address, &sheets)?;
            let (start, end) = area.split_once(':').unwrap_or((&area, &area));
            let (r1, c1, _, _) = parse_a1_1based(start).map_err(invalid)?;
            let (r2, c2, _, _) = parse_a1_1based(end).map_err(invalid)?;
            if r1 > r2 || c1 > c2 {
                return Err(invalid("inverted finite range"));
            }
            addresses = Vec::new();
            (
                (r2 - r1 + 1) as u64 * (c2 - c1 + 1) as u64,
                Some((sheet, r1, c1, c2 - c1 + 1)),
            )
        }
    };
    let end = request
        .page
        .offset
        .saturating_add(request.page.limit as u64)
        .min(total);
    let mut items = Vec::new();
    for i in request.page.offset..end {
        let address = if let Some((sheet, r, c, width)) = &range {
            CellAddress::new(
                sheet.clone(),
                r + (i / (*width as u64)) as u32,
                c + (i % (*width as u64)) as u32,
            )
            .map_err(invalid)?
        } else {
            addresses[i as usize].clone()
        };
        items.push(serde_json::to_value(source(&mut reader, &address)?).map_err(native)?);
    }
    let mut next = request.clone();
    next.page.offset = end;
    next.page.expected_fingerprint = Some(fp.clone());
    Ok(ReadReport {
        input_sha256: sha,
        fingerprint: fp,
        mode: "inspect",
        evaluated: false,
        total_page_items: total,
        remaining_page_items: total.saturating_sub(end),
        items,
        coverage: json!({"source":"native_reader","formula_caches":"unverified","engine_loaded":false,"range_total_is_coordinates_not_nonempty_cells":true}),
        hints: vec![],
        next_request: if end < total {
            Some(serde_json::to_value(next).map_err(native)?)
        } else {
            None
        },
    })
}
fn omitted(count: Option<OmittedCount>) -> Value {
    match count {
        Some(OmittedCount::Exact(n)) => json!({"kind":"exact","count":n}),
        Some(OmittedCount::AtLeast(n)) => json!({"kind":"at_least","count":n}),
        None => Value::Null,
        Some(other) => json!({"kind":"unknown","evidence":format!("{other:?}")}),
    }
}
fn truncation(t: &TruncationReport) -> Value {
    json!({"incomplete":t.incomplete,"omitted":if t.incomplete && t.omitted.is_none() {json!({"kind":"unknown"})} else {omitted(t.omitted)}})
}
fn snapshot(c: &CellSnapshot, details: bool) -> Value {
    json!({"address":c.address.to_string(),"formula":c.formula,"value":c.value.as_ref().map(literal),"value_included":c.value_included,
        "staleness":format!("{:?}",c.staleness),"details":if details {Some(json!({"volatile":c.volatile,"spill":c.spill.as_ref().map(|s|match s {formualizer::SpillRole::Anchor {extent}=>json!({"kind":"anchor","extent":range(extent)}),formualizer::SpillRole::Member {anchor}=>json!({"kind":"member","anchor":anchor.to_string()}),other=>json!({"kind":"unsupported_projection","evidence":format!("{other:?}")})})}))}else{None}})
}
fn reference(r: &SemanticReference) -> Value {
    match r {
        SemanticReference::Cell(c) => json!({"kind":"cell","address":c.to_string()}),
        SemanticReference::Range {
            declared,
            resolved,
            cell_count,
        } => {
            json!({"kind":"range","declared":area(declared),"resolved":resolved.as_ref().map(range),"cell_count":cell_count})
        }
        SemanticReference::Name { name, resolution } => {
            let resolution = match resolution {
                NameResolution::Cell(c) => json!({"kind":"cell","address":c.to_string()}),
                NameResolution::Range { declared, resolved } => {
                    json!({"kind":"range","declared":area(declared),"resolved":resolved.as_ref().map(range)})
                }
                NameResolution::Literal(v) => json!({"kind":"literal","value":literal(v)}),
                NameResolution::Formula { formula, value } => {
                    json!({"kind":"formula","formula":formula,"value":value.as_ref().map(literal)})
                }
                NameResolution::Unresolved => json!({"kind":"unresolved"}),
                other => json!({"kind":"unsupported_projection","evidence":format!("{other:?}")}),
            };
            json!({"kind":"name","name":name,"resolution":resolution})
        }
        SemanticReference::Table {
            name,
            specifier,
            resolved,
        } => json!({"kind":"table","name":name,"specifier":specifier,"resolved":range(resolved)}),
        SemanticReference::External { raw } => json!({"kind":"external","raw":raw}),
        SemanticReference::Unsupported { text, reason } => {
            json!({"kind":"unsupported","text":text,"reason":reason})
        }
        other => json!({"kind":"unsupported_projection","evidence":format!("{other:?}")}),
    }
}
pub fn trace_xlsx_bytes(input: &[u8], request: TraceRequest) -> Result<ReadReport, ReadError> {
    let mut scope = request.clone();
    scope.page.offset = 0;
    scope.page.expected_fingerprint = None;
    let (sha, fp) = fingerprint(input, &scope, &request.page)?;
    let reader = NativeXlsxAdapter::open_bytes(input.to_vec()).map_err(native)?;
    let sheets = reader.sheet_names().map_err(native)?;
    let roots = request
        .roots
        .iter()
        .map(|s| cell(s, &sheets))
        .collect::<Result<Vec<_>, _>>()?;
    let wb = Workbook::from_reader(reader, LoadStrategy::EagerAll, WorkbookConfig::ephemeral())
        .map_err(native)?;
    let options = TraceOptions {
        direction: match request.direction {
            Direction::Precedents => TraceDirection::Precedents,
            Direction::Dependents => TraceDirection::Dependents,
        },
        max_depth: request.limits.max_depth,
        max_nodes: request.limits.max_nodes,
        max_links: request.limits.max_links,
        max_work: request.limits.max_work,
        range_member_budget: request.limits.range_member_budget,
        include_values: true,
    };
    let graph = wb.engine().trace(&roots, &options).map_err(native)?;
    let total = graph.nodes.len() as u64;
    let start = request.page.offset.min(total) as usize;
    let end = start.saturating_add(request.page.limit).min(total as usize);
    let mut hints = Vec::new();
    let items=graph.nodes[start..end].iter().map(|node|{
        let mut next=request.clone();next.roots=vec![node.cell.address.to_string()];next.page=Page::default();
        let links=node.links.iter().map(|link|{
            let ref_fact=reference(&link.reference);
            if let SemanticReference::Range{resolved:Some(r),..}= &link.reference {
                hints.push(json!({"kind":"inspect_range","evidence":ref_fact,"request":InspectRequest {selection:Selection::Range {address:range_text(r)},page:Page::default()}}));
            }
            json!({"reference":ref_fact,"kind":match link.kind {formualizer::TraceLinkKind::Formula {provenance}=>json!({"kind":"formula","provenance":format!("{provenance:?}")}),formualizer::TraceLinkKind::SpillAnchor=>json!({"kind":"spill_anchor"}),formualizer::TraceLinkKind::SpillReader=>json!({"kind":"spill_reader"}),other=>json!({"kind":"unsupported_projection","evidence":format!("{other:?}")})},"omitted":omitted(link.omitted),
                "targets":link.targets.iter().map(|target|json!({"address":graph.nodes[target.node.0 as usize].cell.address.to_string(),"disposition":format!("{:?}",target.disposition)})).collect::<Vec<_>>()})
        }).collect::<Vec<_>>();
        json!({"cell":snapshot(&node.cell,request.details),"links":links,"next_trace_request":next,
            "next_inspect_request":InspectRequest{selection:Selection::Cells{addresses:vec![node.cell.address.to_string()]},page:Page::default()}})
    }).collect();
    let diagnostics=wb.name_import_diagnostics().iter().map(|d|json!({"name":d.name,"definition":d.definition,"scope_sheet":d.scope_sheet,"message":d.message})).collect::<Vec<_>>();
    let mut next = request.clone();
    next.page.offset = end as u64;
    next.page.expected_fingerprint = Some(fp.clone());
    Ok(ReadReport {
        input_sha256: sha,
        fingerprint: fp,
        mode: "trace",
        evaluated: false,
        total_page_items: total,
        remaining_page_items: total - end as u64,
        items,
        hints,
        next_request: if end < total as usize {
            Some(serde_json::to_value(next).map_err(native)?)
        } else {
            None
        },
        coverage: json!({"roots":roots.iter().map(ToString::to_string).collect::<Vec<_>>(),"engine_stamp":{"mutation_revision":graph.stamp.mutation_revision,"recalc_epoch":graph.stamp.recalc_epoch},"engine_limits":request.limits,"engine_truncation":truncation(&graph.truncation),
            "source_cache_overlay":"not_read; use next_inspect_request","transitive_graph_complete":if graph.truncation.incomplete {Some(false)}else{None},
            "name_import_diagnostics":diagnostics,"sheet_import_diagnostics":wb.sheet_import_diagnostics().iter().map(|d|json!({"name":d.name,"kind":d.kind})).collect::<Vec<_>>(),
            "loaded_calc_settings":wb.loaded_calc_settings().map(|c|json!({"iterate":c.iterate,"iterate_count":c.iterate_count,"iterate_delta":c.iterate_delta,"calc_mode":c.calc_mode,"full_calc_on_load":c.full_calc_on_load})),
            "cached_external_link_indices":wb.cached_external_link_indices(),
            "source_engine_reconciliation":"not_performed; native importer may normalize or refuse source expressions",
            "external_refusal_and_table_import_diagnostics":"not exposed by upstream Workbook; no completeness certification",
            "stop_reason":"not_reported_by_engine","page_scope":"materialized nodes only; next page does not resume engine exploration"}),
    })
}

fn literal(value: &LiteralValue) -> Value {
    match value {
        LiteralValue::Int(v) => json!({"kind":"integer","value":v}),
        LiteralValue::Number(v) => {
            let mut number = json!({"kind":"number","value":if v.is_finite(){json!(v)}else{json!(v.to_string())}});
            if *v == 0.0 && v.is_sign_negative() {
                number["negative_zero"] = json!(true);
            }
            number
        }
        LiteralValue::Text(v) => json!({"kind":"text","value":v}),
        LiteralValue::Boolean(v) => json!({"kind":"boolean","value":v}),
        LiteralValue::Array(v) => {
            json!({"kind":"array","value":v.iter().map(|r|r.iter().map(literal).collect::<Vec<_>>()).collect::<Vec<_>>()})
        }
        LiteralValue::Empty => json!({"kind":"empty"}),
        LiteralValue::Pending => json!({"kind":"pending"}),
        LiteralValue::Error(v) => {
            json!({"kind":"error","value":v.to_string(),"evidence":format!("{v:?}")})
        }
        LiteralValue::Date(v) => json!({"kind":"date","value":v.to_string()}),
        LiteralValue::DateTime(v) => json!({"kind":"datetime","value":v.to_string()}),
        LiteralValue::Time(v) => json!({"kind":"time","value":v.to_string()}),
        LiteralValue::Duration(v) => json!({"kind":"duration","value":v.to_string()}),
    }
}
fn area(r: &formualizer::common::RangeArea) -> Value {
    json!({"sheet":r.sheet,"start_row":r.start_row,"start_column":r.start_column,"end_row":r.end_row,"end_column":r.end_column})
}
fn range(r: &formualizer::common::RangeAddress) -> Value {
    json!({"sheet":r.sheet,"start_row":r.start_row,"start_column":r.start_col,"end_row":r.end_row,"end_column":r.end_col})
}
fn range_text(r: &formualizer::common::RangeAddress) -> String {
    let start = CellAddress::new(r.sheet.clone(), r.start_row, r.start_col)
        .expect("validated engine range");
    let end = CellAddress::new(r.sheet.clone(), r.end_row, r.end_col)
        .expect("validated engine range")
        .to_string();
    format!("{start}:{}", end.rsplit_once('!').unwrap().1)
}
