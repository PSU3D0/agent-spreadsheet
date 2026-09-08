pub mod filters;
#[cfg(feature = "recalc")]
pub mod fork;
#[cfg(feature = "recalc")]
pub mod mutate_batch;
pub mod param_enums;
#[cfg(feature = "recalc")]
pub mod rules_batch;
#[cfg(feature = "recalc")]
pub mod sheet_layout;
#[cfg(feature = "recalc")]
pub mod structure_impact;
pub mod vba;
#[cfg(feature = "recalc")]
pub mod write_normalize;

use crate::analysis::{formula::FormulaGraph, stats};
use crate::config::OutputProfile;
use crate::model::*;
use crate::state::AppState;
use crate::utils::column_number_to_name;
use crate::verification::{
    VerifyOptions, VerifyResponse, compare_workbooks_with_coverage,
    evaluate_workbook_pair_for_verification, failed_verification_response,
};
use crate::workbook::{WorkbookContext, cell_to_value};
#[cfg(feature = "recalc")]
use anyhow::Context;
use anyhow::{Result, anyhow, bail};
use regex::Regex;
use schemars::JsonSchema;
use serde::Deserialize;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "recalc")]
fn fork_recalc_needed(state: &impl crate::read_context::ReadContext, workbook_or_fork_id: &WorkbookId) -> bool {
    state.recalc_needed(workbook_or_fork_id)
}

#[cfg(feature = "recalc")]
fn sheet_has_formula_in_bounds(sheet: &umya_spreadsheet::Worksheet, bounds: &[CellBounds]) -> bool {
    if bounds.is_empty() {
        return false;
    }
    for cell in sheet.get_cell_collection() {
        if !cell.is_formula() {
            continue;
        }
        let address = cell.get_coordinate().get_coordinate().to_string();
        let Some((col, row)) = parse_address(&address) else {
            continue;
        };
        if bounds
            .iter()
            .any(|b| col >= b.0.0 && col <= b.1.0 && row >= b.0.1 && row <= b.1.1)
        {
            return true;
        }
    }
    false
}

const DEFAULT_TRACE_PAGE_SIZE: usize = 20;
const TRACE_PAGE_MIN: usize = 5;
const TRACE_PAGE_MAX: usize = 200;
const TRACE_RANGE_THRESHOLD: usize = 4;
const TRACE_RANGE_HIGHLIGHT_LIMIT: usize = 3;
const TRACE_GROUP_HIGHLIGHT_LIMIT: usize = 3;
const TRACE_CELL_HIGHLIGHT_LIMIT: usize = 5;
const TRACE_RANGE_VALUE_SAMPLES: usize = 3;
const TRACE_RANGE_FORMULA_SAMPLES: usize = 2;
const TRACE_GROUP_SAMPLE_LIMIT: usize = 5;
const TRACE_DEPENDENTS_PER_CELL_LIMIT: usize = 500;

const DEFAULT_OVERVIEW_MAX_REGIONS: u32 = 25;
const DEFAULT_OVERVIEW_MAX_HEADERS: u32 = 50;
const DEFAULT_OVERVIEW_INCLUDE_HEADERS: bool = true;

const ENTRY_POINT_MAX_ROWS: u32 = 10_000;
const ENTRY_POINT_MAX_COLS: u32 = 200;

#[cfg(feature = "recalc")]
type CellBounds = ((u32, u32), (u32, u32));

pub async fn list_workbooks(
    state: impl crate::read_context::ReadContext,
    params: ListWorkbooksParams,
) -> Result<WorkbookListResponse> {
    let config = state.config();
    let output_profile = config.output_profile();
    let include_paths = params
        .include_paths
        .unwrap_or(!matches!(output_profile, OutputProfile::TokenDense));

    let offset = params.offset.unwrap_or(0) as usize;
    let limit = params.limit.unwrap_or(100) as usize;
    let filter = params.into_filter()?;
    let mut response = state.list_workbooks(filter)?;
    let total_count = response.workbooks.len();

    if offset < total_count {
        let end = (offset + limit).min(total_count);
        response.workbooks = response
            .workbooks
            .into_iter()
            .skip(offset)
            .take(end - offset)
            .collect();
    } else {
        response.workbooks.clear();
    }

    // Apply include_paths toggle
    if !include_paths {
        for wb in &mut response.workbooks {
            wb.path = None;
            wb.caps = None;
            wb.client_path = None;
        }
    } else if !config.path_mappings.is_empty() {
        for wb in &mut response.workbooks {
            if let Some(p) = wb.path.as_ref() {
                let abs = config.resolve_path(PathBuf::from(p));
                wb.client_path = config
                    .map_path_for_client(&abs)
                    .map(|mapped| mapped.display().to_string());
            }
        }
    }

    // Set next_offset if more data exists
    response.next_offset = if offset + response.workbooks.len() < total_count {
        Some((offset + response.workbooks.len()) as u32)
    } else {
        None
    };

    Ok(response)
}

pub async fn describe_workbook(
    state: impl crate::read_context::ReadContext,
    params: DescribeWorkbookParams,
) -> Result<WorkbookDescription> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let mut desc = workbook.describe();
    let config = state.config();
    if !config.path_mappings.is_empty() {
        let internal = PathBuf::from(&desc.path);
        desc.client_path = config
            .map_path_for_client(&internal)
            .map(|mapped| mapped.display().to_string());
    }
    Ok(desc)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListWorkbooksParams {
    /// Filter by workbook slug prefix
    pub slug_prefix: Option<String>,
    /// Filter by folder path
    pub folder: Option<String>,
    /// Filter by glob pattern (e.g., "**/*.xlsx")
    pub path_glob: Option<String>,
    /// Maximum number of workbooks to return (default: 100)
    #[serde(default)]
    pub limit: Option<u32>,
    /// Offset for pagination; use next_offset from previous response
    #[serde(default)]
    pub offset: Option<u32>,
    /// Include file paths and capabilities (default: false in token_dense profile)
    #[serde(default)]
    pub include_paths: Option<bool>,
}

impl ListWorkbooksParams {
    fn into_filter(self) -> Result<filters::WorkbookFilter> {
        filters::WorkbookFilter::new(self.slug_prefix, self.folder, self.path_glob)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DescribeWorkbookParams {
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListSheetsParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Maximum number of sheets to return (default: 100)
    #[serde(default)]
    pub limit: Option<u32>,
    /// Offset for pagination; use next_offset from previous response
    #[serde(default)]
    pub offset: Option<u32>,
    /// Include row/column counts and metrics (default: false in token_dense profile)
    #[serde(default)]
    pub include_bounds: Option<bool>,
}

pub async fn list_sheets(
    state: Arc<AppState>,
    params: ListSheetsParams,
) -> Result<SheetListResponse> {
    let request = crate::operations::ListSheetsRequest::try_from(params)
        .map_err(|error| anyhow::anyhow!(error))?;
    crate::operations::project_legacy(
        state,
        crate::operations::SpreadsheetOperation::ListSheets(request),
    )
    .await
}

pub(crate) async fn list_sheets_semantic(
    state: impl crate::read_context::ReadContext,
    params: ListSheetsParams,
) -> Result<SheetListResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let config = state.config();
    let output_profile = config.output_profile();
    let include_bounds = params
        .include_bounds
        .unwrap_or(!matches!(output_profile, OutputProfile::TokenDense));
    let mut summaries = workbook.list_summaries(include_bounds)?;

    let total_count = summaries.len();
    let offset = params.offset.unwrap_or(0) as usize;
    let limit = params.limit.unwrap_or(100) as usize;

    if offset < total_count {
        summaries = summaries.into_iter().skip(offset).take(limit).collect();
    } else {
        summaries.clear();
    }

    let next_offset = if offset + summaries.len() < total_count {
        Some((offset + summaries.len()) as u32)
    } else {
        None
    };

    let response = SheetListResponse {
        workbook_id: workbook.id.clone(),
        sheets: summaries,
        next_offset,
    };
    Ok(response)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SheetOverviewParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Sheet name
    pub sheet_name: String,
    /// Maximum detected regions to return (default: 25)
    #[serde(default)]
    pub max_regions: Option<u32>,
    /// Maximum headers per region (default: 50)
    #[serde(default)]
    pub max_headers: Option<u32>,
    /// Include headers in region info (default: true)
    #[serde(default)]
    pub include_headers: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkbookSummaryParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Return minimal summary without entry points or named ranges (default: true in token_dense profile)
    #[serde(default)]
    pub summary_only: Option<bool>,
    /// Include suggested entry points for exploration (default: !summary_only)
    #[serde(default)]
    pub include_entry_points: Option<bool>,
    /// Include key named ranges and tables (default: !summary_only)
    #[serde(default)]
    pub include_named_ranges: Option<bool>,
}

pub async fn workbook_summary(
    state: impl crate::read_context::ReadContext,
    params: WorkbookSummaryParams,
) -> Result<WorkbookSummaryResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let config = state.config();
    let output_profile = config.output_profile();
    let summary_only = params
        .summary_only
        .unwrap_or(matches!(output_profile, OutputProfile::TokenDense));
    let include_entry_points = params.include_entry_points.unwrap_or(!summary_only);
    let include_named_ranges = params.include_named_ranges.unwrap_or(!summary_only);

    state.with_workbook(workbook, move |workbook| {
        build_workbook_summary(workbook, include_entry_points, include_named_ranges)
    })
    .await
}

fn build_workbook_summary(
    workbook: Arc<WorkbookContext<impl crate::workbook::WorkbookReadSource>>,
    include_entry_points: bool,
    include_named_ranges: bool,
) -> Result<WorkbookSummaryResponse> {
    let sheet_names = workbook.sheet_names();

    let mut total_cells: u64 = 0;
    let mut total_formulas: u64 = 0;
    let mut breakdown = WorkbookBreakdown::default();
    let mut region_counts = RegionCountSummary::default();
    let mut entry_points: Vec<EntryPoint> = Vec::new();
    let mut key_named_ranges: Vec<NamedRangeDescriptor> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    for sheet_name in &sheet_names {
        let entry = workbook.get_sheet_metrics_fast(sheet_name)?;
        total_cells += (entry.metrics.row_count as u64) * (entry.metrics.column_count as u64);
        total_formulas += entry.metrics.formula_cells as u64;

        match entry.metrics.classification {
            SheetClassification::Calculator => breakdown.calculator_sheets += 1,
            SheetClassification::Metadata => breakdown.metadata_sheets += 1,
            SheetClassification::Empty => {}
            _ => breakdown.data_sheets += 1,
        }

        if entry.metrics.non_empty_cells == 0 {
            continue;
        }

        match entry.metrics.classification {
            SheetClassification::Calculator => region_counts.calculator += 1,
            SheetClassification::Metadata => region_counts.metadata += 1,
            SheetClassification::Empty => {}
            _ => region_counts.data += 1,
        }

        if include_entry_points {
            let priority = entry_point_priority(&entry.metrics.classification);
            entry_points.push(EntryPoint {
                sheet_name: sheet_name.clone(),
                region_id: None,
                bounds: entry_point_bounds(&entry.metrics),
                rationale: format!(
                    "Fast summary p{}: {:?} sheet",
                    priority, entry.metrics.classification
                ),
            });
        }
    }

    if include_entry_points {
        entry_points.sort_by(|a, b| {
            let pa = priority_from_rationale(&a.rationale);
            let pb = priority_from_rationale(&b.rationale);
            pa.cmp(&pb)
                .then_with(|| {
                    a.bounds
                        .as_ref()
                        .map(|_| 1)
                        .cmp(&b.bounds.as_ref().map(|_| 1))
                })
                .then_with(|| a.sheet_name.cmp(&b.sheet_name))
        });
        entry_points.truncate(5);
    }

    if include_named_ranges {
        let mut seen_ranges = std::collections::HashSet::new();
        for item in workbook.named_items()? {
            if item.kind != NamedItemKind::NamedRange && item.kind != NamedItemKind::Table {
                continue;
            }
            if !seen_ranges.insert(item.refers_to.clone()) {
                continue;
            }
            key_named_ranges.push(item);
            if key_named_ranges.len() >= 10 {
                break;
            }
        }
    }

    notes.push("Region counts and entry points are inferred from sheet metrics; use sheet_overview for full region detection.".to_string());

    Ok(WorkbookSummaryResponse {
        workbook_id: workbook.id.clone(),
        slug: workbook.slug.clone(),
        sheet_count: sheet_names.len(),
        total_cells,
        total_formulas,
        breakdown,
        region_counts,
        key_named_ranges,
        suggested_entry_points: entry_points,
        notes,
    })
}

fn entry_point_priority(classification: &SheetClassification) -> u32 {
    match classification {
        SheetClassification::Data => 1,
        SheetClassification::Mixed => 2,
        SheetClassification::Calculator => 3,
        SheetClassification::Metadata => 4,
        SheetClassification::Empty => 5,
    }
}

fn entry_point_bounds(metrics: &crate::workbook::SheetMetrics) -> Option<String> {
    if metrics.row_count == 0 || metrics.column_count == 0 {
        return None;
    }
    if metrics.row_count > ENTRY_POINT_MAX_ROWS || metrics.column_count > ENTRY_POINT_MAX_COLS {
        return None;
    }
    let end_col = column_number_to_name(metrics.column_count.max(1));
    let end_cell = format!("{}{}", end_col, metrics.row_count.max(1));
    Some(format!("A1:{}", end_cell))
}

fn priority_from_rationale(rationale: &str) -> u32 {
    if rationale.contains("p0") {
        0
    } else if rationale.contains("p1") {
        1
    } else if rationale.contains("p2") {
        2
    } else if rationale.contains("p3") {
        3
    } else if rationale.contains("p4") {
        4
    } else {
        5
    }
}

pub async fn sheet_overview(
    state: Arc<AppState>,
    params: SheetOverviewParams,
) -> Result<SheetOverviewResponse> {
    let request = crate::operations::SheetOverviewRequest::try_from(params)
        .map_err(|error| anyhow::anyhow!(error))?;
    crate::operations::project_legacy(
        state,
        crate::operations::SpreadsheetOperation::SheetOverview(request),
    )
    .await
}

pub(crate) async fn sheet_overview_semantic(
    state: impl crate::read_context::ReadContext,
    params: SheetOverviewParams,
) -> Result<SheetOverviewResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let sheet_name = params.sheet_name.clone();
    let mut overview =
        state.with_workbook(workbook, move |workbook| workbook.sheet_overview(&sheet_name)).await?;

    let max_regions = params
        .max_regions
        .unwrap_or(DEFAULT_OVERVIEW_MAX_REGIONS)
        .max(1);
    let max_headers = params
        .max_headers
        .unwrap_or(DEFAULT_OVERVIEW_MAX_HEADERS)
        .max(1);
    let include_headers = params
        .include_headers
        .unwrap_or(DEFAULT_OVERVIEW_INCLUDE_HEADERS);

    let region_limit = if params.max_regions == Some(0) {
        usize::MAX
    } else {
        max_regions as usize
    };
    let header_limit = if params.max_headers == Some(0) {
        usize::MAX
    } else {
        max_headers as usize
    };

    let total_regions = overview.detected_regions.len() as u32;
    let mut headers_truncated = false;

    for region in &mut overview.detected_regions {
        let header_count = region.header_count.max(region.headers.len() as u32);
        region.header_count = header_count;
        if !include_headers {
            region.headers.clear();
        } else if region.headers.len() > header_limit {
            region.headers.truncate(header_limit);
        }
        region.headers_truncated = region.headers.len() as u32 != header_count;
        headers_truncated |= region.headers_truncated;
    }

    let regions_truncated = if overview.detected_regions.len() > region_limit {
        overview.detected_regions.truncate(region_limit);
        true
    } else {
        false
    };

    overview.detected_region_count = total_regions;
    overview.detected_regions_truncated = regions_truncated;

    if regions_truncated {
        overview.notes.push(format!(
            "Detected regions truncated to {} ({} total).",
            region_limit, total_regions
        ));
    }
    if headers_truncated {
        overview.notes.push(format!(
            "Region headers truncated to {} columns.",
            header_limit
        ));
    }

    Ok(overview)
}

fn default_start_row() -> u32 {
    1
}

fn default_page_size() -> u32 {
    50
}

fn default_include_formulas() -> bool {
    true
}

fn default_include_header() -> bool {
    true
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetPageParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Sheet name
    pub sheet_name: String,
    /// 1-based starting row (default: 1)
    #[serde(default = "default_start_row")]
    pub start_row: u32,
    /// Number of rows per page (default: 50, max: 500)
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    /// Limit to specific columns by letter (e.g., ["A", "C", "D"])
    #[serde(default)]
    pub columns: Option<Vec<String>>,
    /// Limit to columns by header text (matched case-insensitively)
    #[serde(default)]
    pub columns_by_header: Option<Vec<String>>,
    /// Include formulas (default: false in token_dense profile)
    #[serde(default = "default_include_formulas")]
    pub include_formulas: bool,
    /// Include style information (default: false)
    #[serde(default)]
    pub include_styles: bool,
    /// Include header row in response (default: true)
    #[serde(default = "default_include_header")]
    pub include_header: bool,
    /// Output format: "compact" (default in token_dense) or "full" (per-cell objects)
    #[serde(default)]
    pub format: Option<SheetPageFormat>,
}

impl Default for SheetPageParams {
    fn default() -> Self {
        SheetPageParams {
            workbook_or_fork_id: WorkbookId(String::new()),
            sheet_name: String::new(),
            start_row: default_start_row(),
            page_size: default_page_size(),
            columns: None,
            columns_by_header: None,
            include_formulas: default_include_formulas(),
            include_styles: false,
            include_header: default_include_header(),
            format: None,
        }
    }
}

fn default_find_limit() -> u32 {
    50
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindValueParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Value or pattern to search for
    pub query: String,
    /// For label mode: find cells near this label text
    #[serde(default)]
    pub label: Option<String>,
    /// Search mode: "value" (default) or "label" for key-value lookups
    #[serde(default)]
    pub mode: Option<FindMode>,
    /// Match mode for text comparison
    #[serde(default)]
    pub match_mode: Option<MatchMode>,
    /// Case-sensitive matching (default: false)
    #[serde(default)]
    pub case_sensitive: bool,
    /// Limit search to specific sheet
    #[serde(default)]
    pub sheet_name: Option<String>,
    /// Limit search to specific detected region
    #[serde(default)]
    pub region_id: Option<u32>,
    /// Limit search to specific named table
    #[serde(default)]
    pub table_name: Option<String>,
    /// Filter by value types
    #[serde(default)]
    pub value_types: Option<Vec<ValueTypeFilter>>,
    /// Only search in header rows (default: false)
    #[serde(default)]
    pub search_headers_only: bool,
    /// For label mode: direction to look for value
    #[serde(default)]
    pub direction: Option<LabelDirection>,
    /// Maximum matches to return (default: 50)
    #[serde(default = "default_find_limit")]
    pub limit: u32,
    /// Offset for pagination
    #[serde(default)]
    pub offset: Option<u32>,
    /// Context to include with matches
    #[serde(default)]
    pub context: Option<FindContext>,
    /// Number of cells in each direction for context (default: 3)
    #[serde(default)]
    pub context_width: Option<u32>,
}

impl Default for FindValueParams {
    fn default() -> Self {
        Self {
            workbook_or_fork_id: WorkbookId(String::new()),
            query: String::new(),
            label: None,
            mode: None,
            match_mode: None,
            case_sensitive: false,
            sheet_name: None,
            region_id: None,
            table_name: None,
            value_types: None,
            search_headers_only: false,
            direction: None,
            limit: default_find_limit(),
            offset: None,
            context: None,
            context_width: None,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct ReadTableParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Sheet name (uses first sheet if omitted)
    #[serde(default)]
    pub sheet_name: Option<String>,
    /// Read from a named Excel table
    #[serde(default)]
    pub table_name: Option<String>,
    /// Read from a detected region by ID (from sheet_overview)
    #[serde(default)]
    pub region_id: Option<u32>,
    /// A1-style range (e.g., "A1:D100")
    #[serde(default)]
    pub range: Option<String>,
    /// 1-based row number for headers (auto-detected if omitted)
    #[serde(default)]
    pub header_row: Option<u32>,
    /// Number of header rows for multi-row headers (default: 1)
    #[serde(default)]
    pub header_rows: Option<u32>,
    /// Limit to specific columns by letter (e.g., ["A", "C", "D"])
    #[serde(default)]
    pub columns: Option<Vec<String>>,
    /// Row filters to apply
    #[serde(default)]
    pub filters: Option<Vec<TableFilter>>,
    /// Sampling mode for selecting rows
    #[serde(default)]
    pub sample_mode: Option<SampleMode>,
    /// Maximum rows to return
    #[serde(default)]
    pub limit: Option<u32>,
    /// Offset for pagination; use next_offset from previous response
    #[serde(default)]
    pub offset: Option<u32>,
    /// Output format: "csv" (default), "values" (arrays), or "json" (typed CellValue)
    #[serde(default)]
    pub format: Option<TableOutputFormat>,
    /// Include header row in output (default: true for csv)
    #[serde(default)]
    pub include_headers: Option<bool>,
    /// Include column type information (default: false)
    #[serde(default)]
    pub include_types: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema, Clone)]
pub struct TableFilter {
    /// Column letter or header name
    pub column: String,
    /// Comparison operator
    pub op: FilterOp,
    /// Value to compare against
    pub value: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct TableProfileParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Sheet name (uses first sheet if omitted)
    #[serde(default)]
    pub sheet_name: Option<String>,
    /// Profile a detected region by ID
    #[serde(default)]
    pub region_id: Option<u32>,
    /// Profile a named Excel table
    #[serde(default)]
    pub table_name: Option<String>,
    /// Profile an explicit A1 range.
    #[serde(default)]
    pub range: Option<String>,
    /// Sampling mode for selecting sample rows
    #[serde(default)]
    pub sample_mode: Option<SampleMode>,
    /// Number of sample rows to include (default: 5)
    #[serde(default)]
    pub sample_size: Option<u32>,
    /// Return only column types without samples (default: true in token_dense profile)
    #[serde(default)]
    pub summary_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RangeValuesParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Sheet name
    pub sheet_name: String,
    /// A1-style ranges to read (e.g., ["A1:C10", "E1:E10"])
    pub ranges: Vec<String>,
    /// Include detected header row (default: true)
    #[serde(default)]
    pub include_headers: Option<bool>,
    /// Include formula text payload (matrix for json, sparse list for dense) (default: false)
    #[serde(default)]
    pub include_formulas: Option<bool>,
    /// Output format: "dense" (default), "values", "csv", or "json"
    #[serde(default)]
    pub format: Option<TableOutputFormat>,
    /// Maximum rows per range before pagination
    #[serde(default)]
    pub page_size: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InspectCellsParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Sheet name
    pub sheet_name: String,
    /// One or more A1 targets (cells or ranges) to inspect
    pub targets: Vec<String>,
    /// Include empty cells in the response (default: false)
    #[serde(default)]
    pub include_empty: Option<bool>,
    /// Override the per-request cell budget (default 25, max 200).
    /// Values outside 1..=200 are rejected.
    #[serde(default)]
    pub budget: Option<u32>,
}

pub async fn sheet_page(
    state: impl crate::read_context::ReadContext,
    params: SheetPageParams,
) -> Result<SheetPageResponse> {
    sheet_page_with_header_row(state, params, 1).await
}

pub(crate) async fn sheet_page_with_header_row(
    state: impl crate::read_context::ReadContext,
    params: SheetPageParams,
    header_row: u32,
) -> Result<SheetPageResponse> {
    if params.page_size == 0 {
        return Err(anyhow!("page_size must be greater than zero"));
    }

    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let metrics = workbook.get_sheet_metrics_fast(&params.sheet_name)?;
    let config = state.config();
    let output_profile = config.output_profile();
    let format = params.format.unwrap_or(match output_profile {
        OutputProfile::TokenDense => SheetPageFormat::Compact,
        OutputProfile::Verbose => SheetPageFormat::Full,
    });

    let start_row = params.start_row.max(1);
    let page_size = params.page_size.min(500);
    let include_formulas =
        if params.format.is_none() && matches!(output_profile, OutputProfile::TokenDense) {
            false
        } else {
            params.include_formulas
        };
    let include_styles =
        if params.format.is_none() && matches!(output_profile, OutputProfile::TokenDense) {
            false
        } else {
            params.include_styles
        };
    let columns = params.columns.clone();
    let columns_by_header = params.columns_by_header.clone();
    let include_header = params.include_header;

    let mut page = workbook.with_sheet(&params.sheet_name, |sheet| {
        build_page(
            sheet,
            start_row,
            page_size,
            columns.clone(),
            columns_by_header.clone(),
            include_formulas,
            include_styles,
            include_header,
            header_row.max(1),
        )
    })?;

    let max_cells = config.max_cells();
    let max_payload_bytes = config.max_payload_bytes();
    let cells_per_row = page.rows.first().map(|row| row.cells.len()).unwrap_or(0);
    let original_row_count = page.rows.len();
    let mut row_limit = cap_rows_by_cells(page.rows.len(), cells_per_row, max_cells);

    if row_limit > 0 {
        row_limit = cap_rows_by_payload_bytes(row_limit, max_payload_bytes, |count| {
            let response = build_sheet_page_response(
                &workbook,
                &params.sheet_name,
                format,
                include_header,
                &page.header,
                &page.rows[..count],
                None,
            );
            serde_json::to_vec(&response)
                .map(|payload| payload.len())
                .unwrap_or(usize::MAX)
        });
    }

    let truncated = row_limit < original_row_count;
    if truncated {
        page.rows.truncate(row_limit);
    }

    let last_row_index = page
        .rows
        .last()
        .map(|row| row.row_index)
        .unwrap_or(start_row.saturating_sub(1));
    let next_start_row = if last_row_index < metrics.metrics.row_count {
        Some(last_row_index + 1)
    } else {
        None
    };

    let rows_returned = page.rows.len();
    let cells_returned = rows_returned * cells_per_row;
    let total_rows_available = metrics.metrics.row_count;

    // Build budget metadata when truncation occurred or limits are configured.
    let budget = if truncated || max_cells.is_some() || max_payload_bytes.is_some() {
        let continuation = next_start_row.map(|nsr| {
            format!(
                "use start_row={} to fetch the next page ({} rows remaining)",
                nsr,
                total_rows_available.saturating_sub(last_row_index)
            )
        });
        Some(ReadBudget {
            max_cells,
            max_payload_bytes,
            rows_returned,
            cells_returned,
            total_rows_available: Some(total_rows_available),
            continuation,
        })
    } else {
        None
    };

    let mut response = build_sheet_page_response(
        &workbook,
        &params.sheet_name,
        format,
        include_header,
        &page.header,
        &page.rows,
        next_start_row,
    );
    response.truncated = truncated;
    response.budget = budget;
    Ok(response)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn range_rows_from_view<S: crate::workbook::WorkbookReadSource>(
    workbook: &crate::workbook::WorkbookContext<S>,
    sheet_name: &str,
    start_col: u32,
    start_row: u32,
    end_col: u32,
    end_row: u32,
    include_formulas: bool,
    include_styles: bool,
) -> Result<Vec<RowSnapshot>> {
    let columns = (start_col..=end_col).collect::<Vec<_>>();
    workbook.with_sheet(sheet_name, |sheet| {
        (start_row..=end_row)
            .map(|row| build_row_snapshot(sheet, row, &columns, include_formulas, include_styles))
            .collect()
    })
}

pub(crate) fn sheet_rows_from_view<S: crate::workbook::WorkbookReadSource>(
    workbook: &crate::workbook::WorkbookContext<S>,
    params: SheetPageParams,
    header_row: u32,
) -> Result<(Option<RowSnapshot>, Vec<RowSnapshot>)> {
    if params.page_size == 0 {
        return Err(anyhow!("page_size must be greater than zero"));
    }
    let start_row = params.start_row.max(1);
    let page_size = params.page_size.min(500);
    let page = workbook.with_sheet(&params.sheet_name, |sheet| {
        build_page(
            sheet,
            start_row,
            page_size,
            params.columns,
            params.columns_by_header,
            params.include_formulas,
            params.include_styles,
            params.include_header,
            header_row.max(1),
        )
    })?;

    Ok((page.header, page.rows))
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetFormulaMapParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Sheet name
    pub sheet_name: String,
    /// Limit to A1-style range (e.g., "D2:D100")
    pub range: Option<String>,
    /// Expand range references in formulas (default: false)
    #[serde(default)]
    pub expand: bool,
    /// Maximum formula groups to return
    #[serde(default)]
    pub limit: Option<u32>,
    /// Zero-based formula group continuation offset.
    #[serde(default)]
    pub offset: Option<u32>,
    /// Sort by: "address" (default), "complexity" (longest formulas first), "count" (most repeated first)
    #[serde(default)]
    pub sort_by: Option<FormulaSortBy>,
    /// Return only formula text and count without addresses (default: true in token_dense profile)
    #[serde(default)]
    pub summary_only: Option<bool>,
    /// Include cell addresses for each formula group (default: !summary_only)
    #[serde(default)]
    pub include_addresses: Option<bool>,
    /// Maximum addresses to include per formula group (default: 15)
    #[serde(default)]
    pub addresses_limit: Option<u32>,
    /// Formula parse policy: fail, warn (default), or off
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FormulaSortBy {
    #[default]
    Address,
    Complexity,
    Count,
}

/// Match mode for text searches
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// Substring match (default)
    #[default]
    Contains,
    /// Exact match
    Exact,
    /// Prefix match
    Prefix,
    /// Regular expression match
    Regex,
}

/// Context to include with find_value matches
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FindContext {
    /// No context (default)
    #[default]
    None,
    /// Include neighboring cells
    Neighbors,
    /// Include full row context
    Row,
    /// Include both neighbors and row context
    Both,
}

/// Sampling mode for table reads
#[derive(Debug, Clone, Copy, Default, serde::Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SampleMode {
    /// First N rows (default)
    #[default]
    First,
    /// Last N rows
    Last,
    /// Evenly distributed sample
    Distributed,
}

/// Granularity for style analysis
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StyleGranularity {
    /// Group contiguous cells with same style (default)
    #[default]
    Runs,
    /// Report each cell individually
    Cells,
}

/// Filter operators for table queries
#[derive(Debug, Clone, Copy, serde::Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    /// Equal
    Eq,
    /// Not equal
    #[serde(alias = "ne")]
    Neq,
    /// Greater than
    Gt,
    /// Less than
    Lt,
    /// Greater than or equal
    Gte,
    /// Less than or equal
    Lte,
    /// Contains substring (text only)
    Contains,
    /// Starts with prefix (text only)
    StartsWith,
    /// Ends with suffix (text only)
    EndsWith,
    /// Value is in list
    In,
}

/// Cell value types for filtering
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValueTypeFilter {
    Text,
    Number,
    Bool,
    Date,
    Null,
}

pub async fn sheet_formula_map(
    state: Arc<AppState>,
    params: SheetFormulaMapParams,
) -> Result<SheetFormulaMapResponse> {
    let request = crate::operations::FormulaMapRequest {
        resource_id: crate::operations::ResourceId::bind_workbook(&params.workbook_or_fork_id)
            .map_err(anyhow::Error::msg)?,
        sheet_name: params.sheet_name,
        range: params.range,
        expand: params.expand,
        limit: params.limit,
        sort_by: params.sort_by,
        summary_only: params.summary_only,
        include_addresses: params.include_addresses,
        addresses_limit: params.addresses_limit,
        formula_parse_policy: params.formula_parse_policy,
        cursor: None,
    };
    crate::operations::project_legacy(
        state,
        crate::operations::SpreadsheetOperation::FormulaMap(request),
    )
    .await
}

pub(crate) async fn sheet_formula_map_semantic(
    state: impl crate::read_context::ReadContext,
    params: SheetFormulaMapParams,
) -> Result<SheetFormulaMapResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let config = state.config();
    let output_profile = config.output_profile();
    let summary_only = params
        .summary_only
        .unwrap_or(matches!(output_profile, OutputProfile::TokenDense));
    let include_addresses = params.include_addresses.unwrap_or(!summary_only);
    let addresses_limit = params.addresses_limit.unwrap_or(15);
    let max_items = config.max_items();
    let max_payload_bytes = config.max_payload_bytes();

    let policy = params
        .formula_parse_policy
        .unwrap_or(FormulaParsePolicy::Warn);
    let (graph, diagnostics) =
        workbook.formula_graph_with_diagnostics(&params.sheet_name, policy)?;
    let formula_parse_diagnostics = if diagnostics.total_errors > 0 {
        Some(diagnostics)
    } else {
        None
    };
    let all_groups = graph.groups();
    let mut groups = Vec::new();

    for mut group in all_groups {
        if let Some(range) = &params.range {
            group.addresses.retain(|addr| address_in_range(addr, range));
            if group.addresses.is_empty() {
                continue;
            }
        }
        group.addresses.sort_by(|a, b| compare_addresses(a, b));

        groups.push(group);
    }

    let total_groups = groups.len();

    let sort_by = params.sort_by.unwrap_or_default();
    match sort_by {
        FormulaSortBy::Address => {
            groups.sort_by(|a, b| {
                let left = a.addresses.iter().min_by(|x, y| compare_addresses(x, y));
                let right = b.addresses.iter().min_by(|x, y| compare_addresses(x, y));
                match (left, right) {
                    (Some(left), Some(right)) => compare_addresses(left, right)
                        .then_with(|| a.fingerprint.cmp(&b.fingerprint)),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => a.fingerprint.cmp(&b.fingerprint),
                }
            });
        }
        FormulaSortBy::Complexity => {
            groups.sort_by(|a, b| b.formula.len().cmp(&a.formula.len()));
        }
        FormulaSortBy::Count => {
            groups.sort_by(|a, b| {
                let count_a = a.count.unwrap_or(a.addresses.len() as u32);
                let count_b = b.count.unwrap_or(b.addresses.len() as u32);
                count_b.cmp(&count_a)
            });
        }
    }

    let offset = params.offset.unwrap_or(0) as usize;
    if offset > 0 {
        groups = groups.into_iter().skip(offset).collect();
    }

    if let Some(limit) = params.limit
        && groups.len() > limit as usize
    {
        groups.truncate(limit as usize);
    }

    if let Some(max_items) = max_items
        && groups.len() > max_items
    {
        groups.truncate(max_items);
    }

    for group in &mut groups {
        if summary_only || !include_addresses {
            group.addresses.clear();
        } else if !params.expand && group.addresses.len() > addresses_limit as usize {
            group.addresses.truncate(addresses_limit as usize);
        }
    }

    if let Some(max_bytes) = max_payload_bytes {
        let group_limit = cap_rows_by_payload_bytes(groups.len(), Some(max_bytes), |count| {
            let response = SheetFormulaMapResponse {
                workbook_id: workbook.id.clone(),
                sheet_name: params.sheet_name.clone(),
                groups: groups[..count].to_vec(),
                formula_parse_diagnostics: formula_parse_diagnostics.clone(),
                next_offset: None,
            };
            serde_json::to_vec(&response)
                .map(|payload| payload.len())
                .unwrap_or(usize::MAX)
        });

        if group_limit < groups.len() {
            groups.truncate(group_limit);
        }
    }

    let next_offset = if offset + groups.len() < total_groups {
        Some((offset + groups.len()) as u32)
    } else {
        None
    };

    let response = SheetFormulaMapResponse {
        workbook_id: workbook.id.clone(),
        sheet_name: params.sheet_name.clone(),
        groups,
        formula_parse_diagnostics,
        next_offset,
    };
    Ok(response)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FormulaTraceParams {
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    pub sheet_name: String,
    pub cell_address: String,
    pub direction: TraceDirection,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
    #[serde(default)]
    pub page_size: Option<usize>,
    #[serde(default)]
    pub cursor: Option<TraceCursor>,
    /// Formula parse policy: fail, warn (default), or off
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
}

pub async fn formula_trace(
    state: impl crate::read_context::ReadContext,
    params: FormulaTraceParams,
) -> Result<FormulaTraceResponse> {
    formula_trace_semantic(state, params).await
}

pub(crate) async fn formula_trace_semantic(
    state: impl crate::read_context::ReadContext,
    params: FormulaTraceParams,
) -> Result<FormulaTraceResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let policy = params
        .formula_parse_policy
        .unwrap_or(FormulaParsePolicy::Warn);
    let (graph, diagnostics) =
        workbook.formula_graph_with_diagnostics(&params.sheet_name, policy)?;
    let formula_parse_diagnostics = if diagnostics.total_errors > 0 {
        Some(diagnostics)
    } else {
        None
    };
    let formula_lookup = build_formula_lookup(&graph);
    let depth = params.depth.unwrap_or(3).clamp(1, 5);
    let page_size = params
        .page_size
        .or_else(|| params.limit.map(|v| v as usize))
        .unwrap_or(DEFAULT_TRACE_PAGE_SIZE)
        .clamp(TRACE_PAGE_MIN, TRACE_PAGE_MAX);

    let origin = params.cell_address.to_uppercase();
    let config = TraceConfig {
        direction: &params.direction,
        origin: &origin,
        sheet_name: &params.sheet_name,
        depth_limit: depth,
        page_size,
    };
    let (layers, next_cursor, notes) = build_trace_layers(
        &workbook,
        &graph,
        &formula_lookup,
        &config,
        params.cursor.clone(),
    )?;

    let response = FormulaTraceResponse {
        workbook_id: workbook.id.clone(),
        sheet_name: params.sheet_name.clone(),
        origin,
        direction: params.direction.clone(),
        layers,
        next_cursor,
        formula_parse_diagnostics,
        notes,
    };
    Ok(response)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NamedRangesParams {
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    pub sheet_name: Option<String>,
    pub name_prefix: Option<String>,
}

pub async fn named_ranges(
    state: Arc<AppState>,
    params: NamedRangesParams,
) -> Result<NamedRangesResponse> {
    let request = crate::operations::NamedRangesRequest {
        resource_id: crate::operations::ResourceId::bind_workbook(&params.workbook_or_fork_id)
            .map_err(anyhow::Error::msg)?,
        sheet_name: params.sheet_name,
        name_prefix: params.name_prefix,
    };
    crate::operations::project_legacy(
        state,
        crate::operations::SpreadsheetOperation::NamedRanges(request),
    )
    .await
}

pub(crate) async fn named_ranges_semantic(
    state: impl crate::read_context::ReadContext,
    params: NamedRangesParams,
) -> Result<NamedRangesResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let mut items = workbook.named_items()?;

    if let Some(sheet_filter) = &params.sheet_name {
        items.retain(|item| {
            item.sheet_name
                .as_ref()
                .map(|name| name.eq_ignore_ascii_case(sheet_filter))
                .unwrap_or(false)
        });
    }
    if let Some(prefix) = &params.name_prefix {
        let prefix_lower = prefix.to_ascii_lowercase();
        items.retain(|item| item.name.to_ascii_lowercase().starts_with(&prefix_lower));
    }

    let response = NamedRangesResponse {
        workbook_id: workbook.id.clone(),
        items,
    };
    Ok(response)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VerifyWorkbookParams {
    #[serde(alias = "baseline_id")]
    pub baseline_workbook_or_fork_id: WorkbookId,
    #[serde(alias = "current_id")]
    pub current_workbook_or_fork_id: WorkbookId,
    #[serde(default)]
    pub targets: Vec<String>,
    pub sheet_name: Option<String>,
    #[serde(default)]
    pub include_named_range_deltas: bool,
    #[serde(default)]
    pub errors_only: bool,
    #[serde(default)]
    pub targets_only: bool,
}

pub async fn verify_workbook(
    state: Arc<AppState>,
    params: VerifyWorkbookParams,
) -> Result<VerifyResponse> {
    let options = VerifyOptions {
        // Auto-qualify bare A1 targets with `sheet_name` when provided: the
        // schema exposes both params, and every read tool teaches the
        // sheet_name+bare-address convention — qualify instead of rejecting.
        targets: params
            .targets
            .iter()
            .map(|target| match (params.sheet_name.as_deref(), target) {
                (Some(sheet), t) if !t.contains('!') => format!("{sheet}!{t}"),
                _ => target.clone(),
            })
            .collect(),
        sheet_filter: params.sheet_name.clone(),
        include_named_range_deltas: params.include_named_range_deltas,
        errors_only: params.errors_only,
        targets_only: params.targets_only,
    };
    options.validate()?;

    let baseline_workbook = state
        .open_workbook(&params.baseline_workbook_or_fork_id)
        .await?;
    let current_workbook = state
        .open_workbook(&params.current_workbook_or_fork_id)
        .await?;

    let baseline_named = if params.include_named_range_deltas {
        Some(
            named_ranges(
                state.clone(),
                NamedRangesParams {
                    workbook_or_fork_id: params.baseline_workbook_or_fork_id.clone(),
                    sheet_name: params.sheet_name.clone(),
                    name_prefix: None,
                },
            )
            .await?,
        )
    } else {
        None
    };
    let current_named = if params.include_named_range_deltas {
        Some(
            named_ranges(
                state.clone(),
                NamedRangesParams {
                    workbook_or_fork_id: params.current_workbook_or_fork_id.clone(),
                    sheet_name: params.sheet_name.clone(),
                    name_prefix: None,
                },
            )
            .await?,
        )
    } else {
        None
    };

    let config = state.config();
    let evaluated = match evaluate_workbook_pair_for_verification(
        &config,
        &baseline_workbook,
        &config,
        &current_workbook,
    )
    .await
    {
        Ok(evaluated) => evaluated,
        Err(error) => {
            return Ok(failed_verification_response(
                params.baseline_workbook_or_fork_id.as_str().to_string(),
                params.current_workbook_or_fork_id.as_str().to_string(),
                &baseline_workbook,
                &current_workbook,
                format!("verification evaluation failed: {error}"),
            ));
        }
    };

    compare_workbooks_with_coverage(
        params.baseline_workbook_or_fork_id.as_str().to_string(),
        params.current_workbook_or_fork_id.as_str().to_string(),
        &evaluated.baseline().workbook,
        &evaluated.current().workbook,
        &options,
        baseline_named.as_ref().map(|r| r.items.as_slice()),
        current_named.as_ref().map(|r| r.items.as_slice()),
        evaluated.baseline().coverage.clone(),
        evaluated.current().coverage.clone(),
    )
}

// ── Named Range CRUD ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DefineNameParams {
    #[serde(alias = "workbook_id")]
    pub fork_id: WorkbookId,
    /// Name to define (e.g. "SalesTotal").
    pub name: String,
    /// Formula or range the name refers to (e.g. "Sheet1!$A$1:$B$10").
    pub refers_to: String,
    /// Scope: "workbook" (default) or "sheet".
    #[serde(default)]
    pub scope: Option<String>,
    /// Required when scope is "sheet". The sheet to scope the name to.
    pub scope_sheet_name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateNameParams {
    #[serde(alias = "workbook_id")]
    pub fork_id: WorkbookId,
    /// Existing name to update.
    pub name: String,
    /// New refers_to value. If omitted, keeps existing.
    pub refers_to: Option<String>,
    /// Scope filter to disambiguate: "workbook" or "sheet".
    pub scope: Option<String>,
    /// Sheet name to disambiguate when scope is "sheet".
    pub scope_sheet_name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteNameParams {
    #[serde(alias = "workbook_id")]
    pub fork_id: WorkbookId,
    /// Name to delete.
    pub name: String,
    /// Scope filter: "workbook" or "sheet".
    pub scope: Option<String>,
    /// Sheet name to disambiguate when scope is "sheet".
    pub scope_sheet_name: Option<String>,
}

pub fn parse_scope_kind(scope: Option<&str>) -> Result<NamedRangeScope> {
    match scope {
        Some("sheet") => Ok(NamedRangeScope::Sheet),
        Some("workbook") | None => Ok(NamedRangeScope::Workbook),
        Some(other) => Err(anyhow!(
            "invalid scope '{}': expected 'workbook' or 'sheet'",
            other
        )),
    }
}

pub fn parse_scope_kind_optional(scope: Option<&str>) -> Result<Option<NamedRangeScope>> {
    match scope {
        Some("sheet") => Ok(Some(NamedRangeScope::Sheet)),
        Some("workbook") => Ok(Some(NamedRangeScope::Workbook)),
        None => Ok(None),
        Some(other) => Err(anyhow!(
            "invalid scope '{}': expected 'workbook' or 'sheet'",
            other
        )),
    }
}

#[cfg(feature = "recalc")]
fn resolve_sheet_index_on_book(
    book: &umya_spreadsheet::Spreadsheet,
    sheet_name: &str,
) -> Result<u32> {
    for (idx, sheet) in book.get_sheet_collection().iter().enumerate() {
        if sheet.get_name() == sheet_name {
            return Ok(idx as u32);
        }
    }
    Err(anyhow!("sheet '{}' not found", sheet_name))
}

/// Apply define_name to an on-disk workbook file.
#[cfg(feature = "recalc")]
pub(crate) fn define_name_in_file(
    path: &std::path::Path,
    name: &str,
    refers_to: &str,
    scope_kind: NamedRangeScope,
    scope_sheet_name: Option<&str>,
) -> Result<()> {
    let mut book = umya_spreadsheet::reader::xlsx::read(path)
        .with_context(|| format!("failed to read workbook '{}'", path.display()))?;

    match scope_kind {
        NamedRangeScope::Sheet => {
            let sn = scope_sheet_name
                .ok_or_else(|| anyhow!("scope_sheet_name required for sheet scope"))?;
            let sheet_index = resolve_sheet_index_on_book(&book, sn)?;
            let sheet = book
                .get_sheet_by_name_mut(sn)
                .ok_or_else(|| anyhow!("sheet '{}' not found", sn))?;
            sheet
                .add_defined_name(name.to_string(), refers_to.to_string())
                .map_err(|e| anyhow!("failed to add defined name: {e}"))?;
            // Set local_sheet_id on the just-added entry.
            let sheet = book
                .get_sheet_by_name_mut(sn)
                .ok_or_else(|| anyhow!("sheet '{}' disappeared", sn))?;
            if let Some(last) = sheet.get_defined_names_mut().last_mut()
                && last.get_name() == name
            {
                last.set_local_sheet_id(sheet_index);
            }
        }
        NamedRangeScope::Workbook => {
            // set_name is pub(crate) in umya, so we create through a sheet then move
            // to workbook level.
            let first_sheet: String = book
                .get_sheet_collection()
                .first()
                .map(|s| s.get_name().to_string())
                .ok_or_else(|| anyhow!("workbook has no sheets"))?;
            let sheet = book
                .get_sheet_by_name_mut(&first_sheet)
                .ok_or_else(|| anyhow!("sheet '{}' not found", first_sheet))?;
            sheet
                .add_defined_name(name.to_string(), refers_to.to_string())
                .map_err(|e| anyhow!("failed to add defined name: {e}"))?;
            // Move the just-added entry from sheet-level to workbook-level.
            let sheet = book
                .get_sheet_by_name_mut(&first_sheet)
                .ok_or_else(|| anyhow!("sheet disappeared"))?;
            let entry = sheet.get_defined_names_mut().pop();
            if let Some(entry) = entry {
                book.add_defined_names(entry);
            }
        }
    }

    umya_spreadsheet::writer::xlsx::write(&book, path)?;
    Ok(())
}

/// Apply update_name to an on-disk workbook file.
#[cfg(feature = "recalc")]
pub(crate) fn update_name_in_file(
    path: &std::path::Path,
    name: &str,
    new_refers_to: Option<&str>,
    scope_kind: Option<NamedRangeScope>,
    scope_sheet_name: Option<&str>,
) -> Result<(String, NamedRangeScope, Option<String>)> {
    let mut book = umya_spreadsheet::reader::xlsx::read(path)
        .with_context(|| format!("failed to read workbook '{}'", path.display()))?;

    let mut found = false;
    let mut previous_refers_to = String::new();
    let mut effective_scope = NamedRangeScope::Workbook;
    let mut effective_sheet: Option<String> = None;

    // Try workbook-level defined names.
    if scope_kind.is_none() || scope_kind == Some(NamedRangeScope::Workbook) {
        for defined in book.get_defined_names_mut().iter_mut() {
            if defined.get_name() == name
                && (scope_kind == Some(NamedRangeScope::Workbook) || !defined.has_local_sheet_id())
            {
                previous_refers_to = defined.get_address();
                if let Some(new_addr) = new_refers_to {
                    defined.set_address(new_addr.to_string());
                }
                effective_scope = NamedRangeScope::Workbook;
                found = true;
                break;
            }
        }
    }

    // Try sheet-level.
    if !found && (scope_kind.is_none() || scope_kind == Some(NamedRangeScope::Sheet)) {
        let sheet_names: Vec<String> = book
            .get_sheet_collection()
            .iter()
            .map(|s: &umya_spreadsheet::Worksheet| s.get_name().to_string())
            .collect();
        for sn in &sheet_names {
            if let Some(filter_sheet) = scope_sheet_name
                && !sn.eq_ignore_ascii_case(filter_sheet)
            {
                continue;
            }
            if let Some(sheet) = book.get_sheet_by_name_mut(sn) {
                for defined in sheet.get_defined_names_mut().iter_mut() {
                    if defined.get_name() == name {
                        previous_refers_to = defined.get_address();
                        if let Some(new_addr) = new_refers_to {
                            defined.set_address(new_addr.to_string());
                        }
                        effective_scope = NamedRangeScope::Sheet;
                        effective_sheet = Some(sn.clone());
                        found = true;
                        break;
                    }
                }
            }
            if found {
                break;
            }
        }
    }

    if !found {
        return Err(anyhow!("named range '{}' not found", name));
    }

    // umya's DefinedName::set_address appends address objects. Recreate the
    // matching definition so update is replacement, never a union.
    if let Some(new_addr) = new_refers_to {
        match effective_scope {
            NamedRangeScope::Workbook => {
                book.get_defined_names_mut()
                    .retain(|defined| defined.get_name() != name);
                let first_sheet = book
                    .get_sheet_collection()
                    .first()
                    .map(|sheet| sheet.get_name().to_string())
                    .ok_or_else(|| anyhow!("workbook has no sheets"))?;
                let sheet = book
                    .get_sheet_by_name_mut(&first_sheet)
                    .ok_or_else(|| anyhow!("sheet disappeared"))?;
                sheet
                    .add_defined_name(name.to_string(), new_addr.to_string())
                    .map_err(|error| anyhow!("failed to replace defined name: {error}"))?;
                let replacement = sheet.get_defined_names_mut().pop();
                if let Some(replacement) = replacement {
                    book.add_defined_names(replacement);
                }
            }
            NamedRangeScope::Sheet => {
                let sheet_name = effective_sheet
                    .as_deref()
                    .ok_or_else(|| anyhow!("sheet-scoped name has no sheet"))?;
                let sheet_index = resolve_sheet_index_on_book(&book, sheet_name)?;
                let sheet = book
                    .get_sheet_by_name_mut(sheet_name)
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                sheet
                    .get_defined_names_mut()
                    .retain(|defined| defined.get_name() != name);
                sheet
                    .add_defined_name(name.to_string(), new_addr.to_string())
                    .map_err(|error| anyhow!("failed to replace defined name: {error}"))?;
                if let Some(replacement) = sheet.get_defined_names_mut().last_mut() {
                    replacement.set_local_sheet_id(sheet_index);
                }
            }
        }
    }

    umya_spreadsheet::writer::xlsx::write(&book, path)?;
    Ok((previous_refers_to, effective_scope, effective_sheet))
}

/// Apply delete_name to an on-disk workbook file.
#[cfg(feature = "recalc")]
pub(crate) fn delete_name_in_file(
    path: &std::path::Path,
    name: &str,
    scope_kind: Option<NamedRangeScope>,
    scope_sheet_name: Option<&str>,
) -> Result<bool> {
    let mut book = umya_spreadsheet::reader::xlsx::read(path)
        .with_context(|| format!("failed to read workbook '{}'", path.display()))?;

    let mut deleted = false;

    // Try workbook-level.
    if scope_kind.is_none() || scope_kind == Some(NamedRangeScope::Workbook) {
        let names = book.get_defined_names_mut();
        let before_len = names.len();
        names.retain(|d: &umya_spreadsheet::DefinedName| d.get_name() != name);
        if names.len() < before_len {
            deleted = true;
        }
    }

    // Try sheet-level.
    if !deleted && (scope_kind.is_none() || scope_kind == Some(NamedRangeScope::Sheet)) {
        let sheet_names: Vec<String> = book
            .get_sheet_collection()
            .iter()
            .map(|s: &umya_spreadsheet::Worksheet| s.get_name().to_string())
            .collect();
        for sn in &sheet_names {
            if let Some(filter_sheet) = scope_sheet_name
                && !sn.eq_ignore_ascii_case(filter_sheet)
            {
                continue;
            }
            if let Some(sheet) = book.get_sheet_by_name_mut(sn) {
                let names = sheet.get_defined_names_mut();
                let before_len = names.len();
                names.retain(|d: &umya_spreadsheet::DefinedName| d.get_name() != name);
                if names.len() < before_len {
                    deleted = true;
                    break;
                }
            }
        }
    }

    if !deleted {
        return Err(anyhow!("named range '{}' not found", name));
    }

    umya_spreadsheet::writer::xlsx::write(&book, path)?;
    Ok(true)
}

#[cfg(feature = "recalc")]
pub async fn define_name(
    state: Arc<AppState>,
    params: DefineNameParams,
) -> Result<DefineNameResponse> {
    let scope_kind = parse_scope_kind(params.scope.as_deref())?;
    if scope_kind == NamedRangeScope::Sheet && params.scope_sheet_name.is_none() {
        return Err(anyhow!(
            "scope_sheet_name is required when scope is 'sheet'"
        ));
    }
    if params.name.trim().is_empty() {
        return Err(anyhow!("name must not be empty"));
    }
    if params.refers_to.trim().is_empty() {
        return Err(anyhow!("refers_to must not be empty"));
    }

    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available (recalc feature required)"))?;
    let fork_ctx = registry.get_fork(params.fork_id.as_str())?;
    let work_path = fork_ctx.work_path.clone();

    let name = params.name.clone();
    let refers_to = params.refers_to.clone();
    let scope_sheet = params.scope_sheet_name.clone();

    crate::runtime::maybe_blocking(move || {
        define_name_in_file(
            &work_path,
            &name,
            &refers_to,
            scope_kind,
            scope_sheet.as_deref(),
        )
    })
    .await??;

    // Mark fork as needing recalc and invalidate cache.
    registry.with_fork_mut(params.fork_id.as_str(), |ctx| {
        ctx.recalc_needed = true;
        Ok(())
    })?;
    let fork_workbook_id = WorkbookId(params.fork_id.as_str().to_string());
    let _ = state.close_workbook(&fork_workbook_id);

    Ok(DefineNameResponse {
        workbook_id: params.fork_id,
        name: params.name,
        refers_to: params.refers_to,
        scope_kind,
        scope_sheet_name: params.scope_sheet_name,
    })
}

#[cfg(feature = "recalc")]
pub async fn update_name(
    state: Arc<AppState>,
    params: UpdateNameParams,
) -> Result<UpdateNameResponse> {
    let scope_kind = parse_scope_kind_optional(params.scope.as_deref())?;
    if params.name.trim().is_empty() {
        return Err(anyhow!("name must not be empty"));
    }

    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available (recalc feature required)"))?;
    let fork_ctx = registry.get_fork(params.fork_id.as_str())?;
    let work_path = fork_ctx.work_path.clone();

    let name = params.name.clone();
    let new_refers_to = params.refers_to.clone();
    let scope_sheet = params.scope_sheet_name.clone();

    let (previous_refers_to, effective_scope, effective_sheet) =
        crate::runtime::maybe_blocking(move || {
            update_name_in_file(
                &work_path,
                &name,
                new_refers_to.as_deref(),
                scope_kind,
                scope_sheet.as_deref(),
            )
        })
        .await??;

    registry.with_fork_mut(params.fork_id.as_str(), |ctx| {
        ctx.recalc_needed = true;
        Ok(())
    })?;
    let fork_workbook_id = WorkbookId(params.fork_id.as_str().to_string());
    let _ = state.close_workbook(&fork_workbook_id);

    let final_refers_to = params
        .refers_to
        .unwrap_or_else(|| previous_refers_to.clone());

    Ok(UpdateNameResponse {
        workbook_id: params.fork_id,
        name: params.name,
        refers_to: final_refers_to,
        scope_kind: effective_scope,
        scope_sheet_name: effective_sheet.or(params.scope_sheet_name),
        previous_refers_to: Some(previous_refers_to),
    })
}

#[cfg(feature = "recalc")]
pub async fn delete_name(
    state: Arc<AppState>,
    params: DeleteNameParams,
) -> Result<DeleteNameResponse> {
    let scope_kind = parse_scope_kind_optional(params.scope.as_deref())?;
    if params.name.trim().is_empty() {
        return Err(anyhow!("name must not be empty"));
    }

    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available (recalc feature required)"))?;
    let fork_ctx = registry.get_fork(params.fork_id.as_str())?;
    let work_path = fork_ctx.work_path.clone();

    let name = params.name.clone();
    let scope_sheet = params.scope_sheet_name.clone();

    crate::runtime::maybe_blocking(move || {
        delete_name_in_file(&work_path, &name, scope_kind, scope_sheet.as_deref())
    })
    .await??;

    registry.with_fork_mut(params.fork_id.as_str(), |ctx| {
        ctx.recalc_needed = true;
        Ok(())
    })?;
    let fork_workbook_id = WorkbookId(params.fork_id.as_str().to_string());
    let _ = state.close_workbook(&fork_workbook_id);

    Ok(DeleteNameResponse {
        workbook_id: params.fork_id,
        name: params.name,
        deleted: true,
    })
}

struct PageBuildResult {
    rows: Vec<RowSnapshot>,
    header: Option<RowSnapshot>,
}

#[allow(clippy::too_many_arguments)]
fn build_page(
    sheet: &umya_spreadsheet::Worksheet,
    start_row: u32,
    page_size: u32,
    columns: Option<Vec<String>>,
    columns_by_header: Option<Vec<String>>,
    include_formulas: bool,
    include_styles: bool,
    include_header: bool,
    header_row: u32,
) -> PageBuildResult {
    let max_col = sheet.get_highest_column();
    let end_row = (start_row + page_size - 1).min(sheet.get_highest_row().max(start_row));
    let column_indices = resolve_columns_with_headers(
        sheet,
        columns.as_ref(),
        columns_by_header.as_ref(),
        max_col,
        header_row,
    );

    let header = if include_header {
        Some(build_row_snapshot(
            sheet,
            header_row,
            &column_indices,
            include_formulas,
            include_styles,
        ))
    } else {
        None
    };

    let mut rows = Vec::new();
    for row_idx in start_row..=end_row {
        rows.push(build_row_snapshot(
            sheet,
            row_idx,
            &column_indices,
            include_formulas,
            include_styles,
        ));
    }

    PageBuildResult { rows, header }
}

fn build_row_snapshot(
    sheet: &umya_spreadsheet::Worksheet,
    row_index: u32,
    columns: &[u32],
    include_formulas: bool,
    include_styles: bool,
) -> RowSnapshot {
    let mut cells = Vec::new();
    for &col in columns {
        if let Some(cell) = sheet.get_cell((col, row_index)) {
            cells.push(build_cell_snapshot(cell, include_formulas, include_styles));
        } else {
            let address = crate::utils::cell_address(col, row_index);
            cells.push(CellSnapshot {
                address,
                value: None,
                formula: None,
                cached_value: None,
                number_format: None,
                style_tags: Vec::new(),
                notes: Vec::new(),
            });
        }
    }

    RowSnapshot { row_index, cells }
}

fn build_cell_snapshot(
    cell: &umya_spreadsheet::Cell,
    include_formulas: bool,
    include_styles: bool,
) -> CellSnapshot {
    let address = cell.get_coordinate().get_coordinate();
    let value = crate::workbook::cell_to_value(cell);
    let formula = if include_formulas && cell.is_formula() {
        Some(cell.get_formula().to_string())
    } else {
        None
    };
    let cached_value = if cell.is_formula() {
        value.clone()
    } else {
        None
    };
    let number_format = if include_styles {
        cell.get_style()
            .get_number_format()
            .map(|fmt| fmt.get_format_code().to_string())
    } else {
        None
    };
    let style_tags = if include_styles {
        crate::analysis::style::tag_cell(cell)
            .map(|(_, tagging)| tagging.tags)
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    CellSnapshot {
        address,
        value,
        formula,
        cached_value,
        number_format,
        style_tags,
        notes: Vec::new(),
    }
}

fn resolve_columns(columns: Option<&Vec<String>>, max_column: u32) -> Vec<u32> {
    use std::collections::BTreeSet;
    use umya_spreadsheet::helper::coordinate::column_index_from_string;

    let mut indices = BTreeSet::new();
    if let Some(specs) = columns {
        for spec in specs {
            if let Some((start, end)) = spec.split_once(':') {
                let start_idx = column_index_from_string(start);
                let end_idx = column_index_from_string(end);
                let (min_idx, max_idx) = if start_idx <= end_idx {
                    (start_idx, end_idx)
                } else {
                    (end_idx, start_idx)
                };
                for idx in min_idx..=max_idx {
                    indices.insert(idx);
                }
            } else {
                indices.insert(column_index_from_string(spec));
            }
        }
    } else {
        for idx in 1..=max_column.max(1) {
            indices.insert(idx);
        }
    }

    indices.into_iter().collect()
}

fn resolve_columns_with_headers(
    sheet: &umya_spreadsheet::Worksheet,
    columns: Option<&Vec<String>>,
    columns_by_header: Option<&Vec<String>>,
    max_column: u32,
    header_row: u32,
) -> Vec<u32> {
    use std::collections::BTreeSet;

    if columns_by_header.is_none() {
        return resolve_columns(columns, max_column);
    }

    let mut selected: BTreeSet<u32> = if columns.is_some() {
        resolve_columns(columns, max_column).into_iter().collect()
    } else {
        BTreeSet::new()
    };
    let mut matched_header = false;
    let header_targets: Vec<String> = columns_by_header
        .unwrap()
        .iter()
        .map(|h| h.trim().to_ascii_lowercase())
        .collect();

    for col_idx in 1..=max_column.max(1) {
        let header_cell = sheet.get_cell((col_idx, header_row));
        let header_value = header_cell
            .and_then(cell_to_value)
            .map(cell_value_to_string_lower);
        if let Some(hval) = header_value
            && header_targets.iter().any(|target| target == &hval)
        {
            selected.insert(col_idx);
            matched_header = true;
        }
    }

    if !matched_header && columns.is_none() {
        resolve_columns(None, max_column)
    } else {
        selected.into_iter().collect()
    }
}

fn cell_value_to_string_lower(value: CellValue) -> String {
    match value {
        CellValue::Text(s) => s.to_ascii_lowercase(),
        CellValue::Number(n) => n.to_string().to_ascii_lowercase(),
        CellValue::Bool(b) => b.to_string(),
        CellValue::Error(e) => e.to_ascii_lowercase(),
        CellValue::Date(d) => d.to_ascii_lowercase(),
    }
}

fn cell_value_to_plain_string(value: &CellValue) -> String {
    match value {
        CellValue::Text(s) => s.clone(),
        CellValue::Number(n) => n.to_string(),
        CellValue::Bool(b) => b.to_string(),
        CellValue::Error(e) => e.clone(),
        CellValue::Date(d) => d.clone(),
    }
}

fn cell_value_to_kind(value: &CellValue) -> CellValueKind {
    match value {
        CellValue::Text(_) => CellValueKind::Text,
        CellValue::Number(_) => CellValueKind::Number,
        CellValue::Bool(_) => CellValueKind::Bool,
        CellValue::Error(_) => CellValueKind::Error,
        CellValue::Date(_) => CellValueKind::Date,
    }
}

fn cell_value_to_primitive(value: &CellValue) -> CellValuePrimitive {
    match value {
        CellValue::Text(s) => CellValuePrimitive::Text(s.clone()),
        CellValue::Number(n) => CellValuePrimitive::Number(*n),
        CellValue::Bool(b) => CellValuePrimitive::Bool(*b),
        CellValue::Error(e) => CellValuePrimitive::Text(e.clone()),
        CellValue::Date(d) => CellValuePrimitive::Text(d.clone()),
    }
}

fn csv_escape_field(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        let mut escaped = String::with_capacity(field.len() + 2);
        escaped.push('"');
        for ch in field.chars() {
            if ch == '"' {
                escaped.push('"');
            }
            escaped.push(ch);
        }
        escaped.push('"');
        escaped
    } else {
        field.to_string()
    }
}

fn push_csv_row<I>(buffer: &mut String, fields: I)
where
    I: IntoIterator<Item = String>,
{
    let mut first = true;
    for field in fields {
        if !first {
            buffer.push(',');
        }
        first = false;
        let escaped = csv_escape_field(&field);
        buffer.push_str(&escaped);
    }
    buffer.push('\n');
}

fn table_rows_to_values(
    headers: &[String],
    rows: &[TableRow],
) -> Vec<Vec<Option<CellValuePrimitive>>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut vals = Vec::with_capacity(headers.len());
        for header in headers {
            let value = row
                .get(header)
                .and_then(|cell| cell.as_ref())
                .map(cell_value_to_primitive);
            vals.push(value);
        }
        out.push(vals);
    }
    out
}

fn table_rows_to_types(headers: &[String], rows: &[TableRow]) -> Vec<Vec<Option<CellValueKind>>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut kinds = Vec::with_capacity(headers.len());
        for header in headers {
            let kind = row
                .get(header)
                .and_then(|cell| cell.as_ref())
                .map(cell_value_to_kind);
            kinds.push(kind);
        }
        out.push(kinds);
    }
    out
}

fn table_rows_to_csv(headers: &[String], rows: &[TableRow], include_headers: bool) -> String {
    let mut csv = String::new();
    if include_headers {
        push_csv_row(&mut csv, headers.iter().cloned());
    }
    for row in rows {
        let values = headers.iter().map(|header| {
            row.get(header)
                .and_then(|cell| cell.as_ref())
                .map(cell_value_to_plain_string)
                .unwrap_or_default()
        });
        push_csv_row(&mut csv, values);
    }
    csv
}

fn filter_table_row(row: &TableRow, headers: &[String]) -> TableRow {
    let mut filtered = TableRow::new();
    for header in headers {
        if let Some(value) = row.get(header) {
            filtered.insert(header.clone(), value.clone());
        }
    }
    filtered
}

type ReadTablePayload = (
    Vec<String>,
    Vec<TableRow>,
    Option<Vec<Vec<Option<CellValuePrimitive>>>>,
    Option<Vec<Vec<Option<CellValueKind>>>>,
    Option<String>,
);

fn build_read_table_payload(
    format: TableOutputFormat,
    headers: &[String],
    rows: &[TableRow],
    include_headers: bool,
    include_types: bool,
) -> ReadTablePayload {
    let headers_out = if include_headers {
        headers.to_vec()
    } else {
        Vec::new()
    };

    let types_out = if include_types {
        Some(table_rows_to_types(headers, rows))
    } else {
        None
    };

    match format {
        TableOutputFormat::Json | TableOutputFormat::Rows => {
            (headers_out, rows.to_vec(), None, types_out, None)
        }
        TableOutputFormat::Values | TableOutputFormat::Dense => (
            headers_out,
            Vec::new(),
            Some(table_rows_to_values(headers, rows)),
            types_out,
            None,
        ),
        TableOutputFormat::Csv => (
            Vec::new(),
            Vec::new(),
            None,
            types_out,
            Some(table_rows_to_csv(headers, rows, include_headers)),
        ),
    }
}

fn cell_matrix_to_values(rows: &[Vec<Option<CellValue>>]) -> Vec<Vec<Option<CellValuePrimitive>>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut vals = Vec::with_capacity(row.len());
        for cell in row {
            vals.push(cell.as_ref().map(cell_value_to_primitive));
        }
        out.push(vals);
    }
    out
}

fn cell_matrix_to_csv(rows: &[Vec<Option<CellValue>>]) -> String {
    let mut csv = String::new();
    for row in rows {
        let values = row.iter().map(|cell| {
            cell.as_ref()
                .map(cell_value_to_plain_string)
                .unwrap_or_default()
        });
        push_csv_row(&mut csv, values);
    }
    csv
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum DensePrimitiveKey {
    Text(String),
    NumberBits(u64),
    Bool(bool),
}

fn dense_key(value: &CellValuePrimitive) -> DensePrimitiveKey {
    match value {
        CellValuePrimitive::Text(text) => DensePrimitiveKey::Text(text.clone()),
        CellValuePrimitive::Number(number) => DensePrimitiveKey::NumberBits(number.to_bits()),
        CellValuePrimitive::Bool(flag) => DensePrimitiveKey::Bool(*flag),
    }
}

fn row_to_dense_runs(indexes: &[u32]) -> Vec<RangeValuesDenseRun> {
    if indexes.is_empty() {
        return Vec::new();
    }
    let mut runs = Vec::new();
    let mut current = indexes[0];
    let mut len: u32 = 1;

    for idx in indexes.iter().copied().skip(1) {
        if idx == current {
            len += 1;
        } else {
            runs.push(RangeValuesDenseRun {
                value_idx: current,
                len,
            });
            current = idx;
            len = 1;
        }
    }

    runs.push(RangeValuesDenseRun {
        value_idx: current,
        len,
    });
    runs
}

fn cell_matrix_to_dense(
    rows: &[Vec<Option<CellValue>>],
    formulas: Option<&[Vec<Option<String>>]>,
) -> RangeValuesDensePayload {
    let primitive_rows = cell_matrix_to_values(rows);
    let mut dictionary: Vec<Option<CellValuePrimitive>> = vec![None];
    let mut dict_index: HashMap<DensePrimitiveKey, u32> = HashMap::new();
    let mut row_runs: Vec<Vec<RangeValuesDenseRun>> = Vec::with_capacity(primitive_rows.len());

    for row in &primitive_rows {
        let mut indexes = Vec::with_capacity(row.len());
        for cell in row {
            let idx = match cell {
                None => 0,
                Some(value) => {
                    let key = dense_key(value);
                    if let Some(existing) = dict_index.get(&key).copied() {
                        existing
                    } else {
                        let next = dictionary.len() as u32;
                        dictionary.push(Some(value.clone()));
                        dict_index.insert(key, next);
                        next
                    }
                }
            };
            indexes.push(idx);
        }
        row_runs.push(row_to_dense_runs(&indexes));
    }

    let dense_formulas = formulas
        .map(|matrix| {
            let mut out = Vec::new();
            for (row_idx, row) in matrix.iter().enumerate() {
                for (col_idx, formula) in row.iter().enumerate() {
                    if let Some(formula) = formula {
                        out.push(RangeValuesDenseFormula {
                            row: row_idx as u32,
                            col: col_idx as u32,
                            formula: formula.clone(),
                        });
                    }
                }
            }
            out
        })
        .unwrap_or_default();

    let col_count = primitive_rows.first().map_or(0, |row| row.len() as u32);

    RangeValuesDensePayload {
        encoding: "dense_v1".to_string(),
        col_count,
        dictionary,
        row_runs,
        formulas: dense_formulas,
    }
}

fn cell_matrix_to_rows_keyed(
    range: &str,
    rows: &[Vec<Option<CellValue>>],
) -> Vec<RangeValuesRowEntry> {
    let ((start_col, start_row), _) = parse_range(range).unwrap_or(((1, 1), (1, 1)));
    let mut out = Vec::with_capacity(rows.len());
    for (row_offset, row) in rows.iter().enumerate() {
        let row_number = start_row + row_offset as u32;
        let mut cells = std::collections::BTreeMap::new();
        for (col_offset, cell) in row.iter().enumerate() {
            if let Some(value) = cell {
                let col_letter = column_number_to_name(start_col + col_offset as u32);
                cells.insert(col_letter, cell_value_to_primitive(value));
            }
        }
        out.push(RangeValuesRowEntry {
            row: row_number,
            cells,
        });
    }
    out
}

pub(crate) fn build_range_values_entry(
    format: TableOutputFormat,
    range: &str,
    rows: &[Vec<Option<CellValue>>],
    formulas: Option<&[Vec<Option<String>>]>,
    next_start_row: Option<u32>,
) -> RangeValuesEntry {
    match format {
        TableOutputFormat::Json => RangeValuesEntry {
            range: range.to_string(),
            rows: Some(rows.to_vec()),
            formulas: formulas.map(|matrix| matrix.to_vec()),
            values: None,
            dense: None,
            csv: None,
            rows_keyed: None,
            next_start_row,
        },
        TableOutputFormat::Values => RangeValuesEntry {
            range: range.to_string(),
            rows: None,
            formulas: None,
            values: Some(cell_matrix_to_values(rows)),
            dense: None,
            csv: None,
            rows_keyed: None,
            next_start_row,
        },
        TableOutputFormat::Csv => RangeValuesEntry {
            range: range.to_string(),
            rows: None,
            formulas: None,
            values: None,
            dense: None,
            csv: Some(cell_matrix_to_csv(rows)),
            rows_keyed: None,
            next_start_row,
        },
        TableOutputFormat::Dense => RangeValuesEntry {
            range: range.to_string(),
            rows: None,
            formulas: None,
            values: None,
            dense: Some(cell_matrix_to_dense(rows, formulas)),
            csv: None,
            rows_keyed: None,
            next_start_row,
        },
        TableOutputFormat::Rows => RangeValuesEntry {
            range: range.to_string(),
            rows: None,
            formulas: None,
            values: None,
            dense: None,
            csv: None,
            rows_keyed: Some(cell_matrix_to_rows_keyed(range, rows)),
            next_start_row,
        },
    }
}

fn cap_rows_by_cells(row_count: usize, cells_per_row: usize, max_cells: Option<usize>) -> usize {
    let Some(max_cells) = max_cells else {
        return row_count;
    };
    if cells_per_row == 0 {
        return row_count;
    }
    let allowed = max_cells / cells_per_row;
    row_count.min(allowed)
}

fn cap_rows_by_payload_bytes<F>(
    row_count: usize,
    max_bytes: Option<usize>,
    mut size_for_rows: F,
) -> usize
where
    F: FnMut(usize) -> usize,
{
    let Some(max_bytes) = max_bytes else {
        return row_count;
    };
    if row_count == 0 {
        return 0;
    }
    let mut low = 0usize;
    let mut high = row_count;
    while low < high {
        let mid = (low + high).div_ceil(2);
        if size_for_rows(mid) <= max_bytes {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

pub(crate) fn build_compact_payload(
    header: &Option<RowSnapshot>,
    rows: &[RowSnapshot],
    include_header: bool,
) -> SheetPageCompact {
    let headers = derive_headers(header, rows);
    let header_row = if include_header {
        header
            .as_ref()
            .map(|h| h.cells.iter().map(|c| c.value.clone()).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let data_rows = rows
        .iter()
        .map(|row| {
            let mut vals: Vec<Option<CellValue>> = Vec::new();
            vals.push(Some(CellValue::Number(row.row_index as f64)));
            vals.extend(row.cells.iter().map(|c| c.value.clone()));
            vals
        })
        .collect();

    SheetPageCompact {
        headers,
        header_row,
        rows: data_rows,
    }
}

pub(crate) fn build_values_only_payload(
    header: &Option<RowSnapshot>,
    rows: &[RowSnapshot],
    include_header: bool,
) -> SheetPageValues {
    let mut data = Vec::new();
    if include_header && let Some(h) = header {
        data.push(h.cells.iter().map(|c| c.value.clone()).collect());
    }
    for row in rows {
        data.push(row.cells.iter().map(|c| c.value.clone()).collect());
    }

    SheetPageValues { rows: data }
}

fn build_sheet_page_response(
    workbook: &WorkbookContext<impl crate::workbook::WorkbookReadSource>,
    sheet_name: &str,
    format: SheetPageFormat,
    include_header: bool,
    header: &Option<RowSnapshot>,
    rows: &[RowSnapshot],
    next_start_row: Option<u32>,
) -> SheetPageResponse {
    let compact_payload = if matches!(format, SheetPageFormat::Compact) {
        Some(build_compact_payload(header, rows, include_header))
    } else {
        None
    };

    let values_only_payload = if matches!(format, SheetPageFormat::ValuesOnly) {
        Some(build_values_only_payload(header, rows, include_header))
    } else {
        None
    };

    let rows_payload = if matches!(format, SheetPageFormat::Full) {
        rows.to_vec()
    } else {
        Vec::new()
    };

    let header_row = if include_header && matches!(format, SheetPageFormat::Full) {
        header.clone()
    } else {
        None
    };

    SheetPageResponse {
        workbook_id: workbook.id.clone(),
        sheet_name: sheet_name.to_string(),
        rows: rows_payload,
        next_start_row,
        header_row,
        compact: compact_payload,
        values_only: values_only_payload,
        format,
        truncated: false,
        budget: None,
    }
}

fn derive_headers(header: &Option<RowSnapshot>, rows: &[RowSnapshot]) -> Vec<String> {
    if let Some(h) = header {
        let mut headers: Vec<String> = h
            .cells
            .iter()
            .map(|c| match &c.value {
                Some(CellValue::Text(t)) => t.clone(),
                Some(CellValue::Number(n)) => n.to_string(),
                Some(CellValue::Bool(b)) => b.to_string(),
                Some(CellValue::Date(d)) => d.clone(),
                Some(CellValue::Error(e)) => e.clone(),
                None => c.address.clone(),
            })
            .collect();
        headers.insert(0, "Row".to_string());
        headers
    } else if let Some(first) = rows.first() {
        let mut headers = Vec::new();
        headers.push("Row".to_string());
        for cell in &first.cells {
            headers.push(cell.address.clone());
        }
        headers
    } else {
        vec![]
    }
}
fn default_stats_sample() -> usize {
    500
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetStatisticsParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Sheet name
    pub sheet_name: String,
    /// Number of rows to sample for statistics (default: 500)
    #[serde(default)]
    pub sample_rows: Option<usize>,
    /// Return stats without sample values (default: true in token_dense profile)
    #[serde(default)]
    pub summary_only: Option<bool>,
}

pub async fn sheet_statistics(
    state: Arc<AppState>,
    params: SheetStatisticsParams,
) -> Result<SheetStatisticsResponse> {
    let request = crate::operations::SheetStatisticsRequest {
        resource_id: crate::operations::ResourceId::bind_workbook(&params.workbook_or_fork_id)
            .map_err(anyhow::Error::msg)?,
        sheet_name: params.sheet_name,
        sample_rows: params.sample_rows,
        summary_only: params.summary_only,
    };
    crate::operations::project_legacy(
        state,
        crate::operations::SpreadsheetOperation::SheetStatistics(request),
    )
    .await
}

pub(crate) async fn sheet_statistics_semantic(
    state: impl crate::read_context::ReadContext,
    params: SheetStatisticsParams,
) -> Result<SheetStatisticsResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let config = state.config();
    let output_profile = config.output_profile();
    let summary_only = params
        .summary_only
        .unwrap_or(matches!(output_profile, OutputProfile::TokenDense));
    let sheet_metrics = workbook.get_sheet_metrics_fast(&params.sheet_name)?;
    let sample_rows = params.sample_rows.unwrap_or_else(default_stats_sample);
    let stats = workbook.with_sheet(&params.sheet_name, |sheet| {
        stats::compute_sheet_statistics(sheet, sample_rows)
    })?;
    let mut numeric_columns = stats.numeric_columns;
    let mut text_columns = stats.text_columns;

    if summary_only {
        for column in &mut numeric_columns {
            column.samples.clear();
        }
        for column in &mut text_columns {
            column.samples.clear();
        }
    }

    let max_items = config.max_items();
    let max_payload_bytes = config.max_payload_bytes();

    if let Some(max_items) = max_items {
        if numeric_columns.len() > max_items {
            numeric_columns.truncate(max_items);
        }
        if text_columns.len() > max_items {
            text_columns.truncate(max_items);
        }
    }

    if let Some(max_bytes) = max_payload_bytes {
        let response_size = |numeric: &Vec<ColumnSummary>, text: &Vec<ColumnSummary>| {
            let response = SheetStatisticsResponse {
                workbook_id: workbook.id.clone(),
                sheet_name: params.sheet_name.clone(),
                row_count: sheet_metrics.metrics.row_count,
                column_count: sheet_metrics.metrics.column_count,
                density: stats.density,
                numeric_columns: numeric.clone(),
                text_columns: text.clone(),
                null_counts: stats.null_counts.clone(),
                duplicate_warnings: stats.duplicate_warnings.clone(),
            };
            serde_json::to_vec(&response)
                .map(|payload| payload.len())
                .unwrap_or(usize::MAX)
        };

        let mut current_size = response_size(&numeric_columns, &text_columns);
        if current_size > max_bytes && !summary_only {
            for column in &mut numeric_columns {
                column.samples.clear();
            }
            for column in &mut text_columns {
                column.samples.clear();
            }
            current_size = response_size(&numeric_columns, &text_columns);
        }

        while current_size > max_bytes && (!text_columns.is_empty() || !numeric_columns.is_empty())
        {
            if !text_columns.is_empty() {
                text_columns.pop();
            } else if !numeric_columns.is_empty() {
                numeric_columns.pop();
            }
            current_size = response_size(&numeric_columns, &text_columns);
        }
    }

    Ok(SheetStatisticsResponse {
        workbook_id: workbook.id.clone(),
        sheet_name: params.sheet_name,
        row_count: sheet_metrics.metrics.row_count,
        column_count: sheet_metrics.metrics.column_count,
        density: stats.density,
        numeric_columns,
        text_columns,
        null_counts: stats.null_counts,
        duplicate_warnings: stats.duplicate_warnings,
    })
}

fn address_in_range(address: &str, range: &str) -> bool {
    parse_range(range).is_none_or(|((start_col, start_row), (end_col, end_row))| {
        if let Some((col, row)) = parse_address(address) {
            col >= start_col && col <= end_col && row >= start_row && row <= end_row
        } else {
            false
        }
    })
}

fn parse_range(range: &str) -> Option<((u32, u32), (u32, u32))> {
    let mut parts = range.split(':');
    let start = parts.next()?;
    let end = parts.next().unwrap_or(start);
    if parts.next().is_some() {
        return None;
    }
    let start_idx = parse_address(start)?;
    let end_idx = parse_address(end)?;
    Some((
        (start_idx.0.min(end_idx.0), start_idx.1.min(end_idx.1)),
        (start_idx.0.max(end_idx.0), start_idx.1.max(end_idx.1)),
    ))
}

fn parse_address(address: &str) -> Option<(u32, u32)> {
    let address = address.trim();
    let bytes = address.as_bytes();
    let mut index = usize::from(bytes.first() == Some(&b'$'));
    let letters_start = index;
    let mut col = 0_u32;
    while let Some(byte) = bytes.get(index).filter(|byte| byte.is_ascii_alphabetic()) {
        col = col
            .checked_mul(26)?
            .checked_add(u32::from(byte.to_ascii_uppercase() - b'A' + 1))?;
        index += 1;
    }
    if index == letters_start {
        return None;
    }
    if bytes.get(index) == Some(&b'$') {
        index += 1;
    }
    let digits_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    if index == digits_start || index != bytes.len() {
        return None;
    }
    let row = address[digits_start..].parse::<u32>().ok()?;
    (row > 0).then_some((col, row))
}

#[derive(Clone)]
struct TableTarget {
    sheet_name: String,
    table_name: Option<String>,
    range: ((u32, u32), (u32, u32)),
    header_hint: Option<u32>,
}

fn resolve_table_target(
    workbook: &WorkbookContext<impl crate::workbook::WorkbookReadSource>,
    params: &ReadTableParams,
) -> Result<TableTarget> {
    let explicit_range = params
        .range
        .as_deref()
        .map(|range| parse_range(range).ok_or_else(|| anyhow!("invalid range: {range}")))
        .transpose()?;

    if let Some(region_id) = params.region_id
        && let Some(sheet) = &params.sheet_name
        && let Ok(region) = workbook.detected_region(sheet, region_id)
    {
        return Ok(TableTarget {
            sheet_name: sheet.clone(),
            table_name: None,
            range: parse_range(&region.bounds).unwrap_or(((1, 1), (1, 1))),
            header_hint: region.header_row,
        });
    }

    if let Some(table_name) = &params.table_name {
        let items = workbook.named_items()?;
        for item in items {
            if item.name.eq_ignore_ascii_case(table_name)
                || item
                    .name
                    .to_ascii_lowercase()
                    .contains(&table_name.to_ascii_lowercase())
            {
                let mut sheet_name = item
                    .sheet_name
                    .clone()
                    .or_else(|| params.sheet_name.clone())
                    .unwrap_or_else(|| workbook.sheet_names().first().cloned().unwrap_or_default());
                let refers_to = item.refers_to.trim_start_matches('=');
                let mut range_part = refers_to;
                if let Some((sheet_part, rest)) = refers_to.split_once('!') {
                    sheet_name = sheet_part.trim_matches('\'').to_string();
                    range_part = rest;
                }
                if let Some(range) = parse_range(range_part) {
                    return Ok(TableTarget {
                        sheet_name,
                        table_name: Some(item.name.clone()),
                        range,
                        header_hint: if item.kind == NamedItemKind::Table {
                            Some(range.0.1)
                        } else {
                            None
                        },
                    });
                }
            }
        }
    }

    let sheet_name = params
        .sheet_name
        .clone()
        .unwrap_or_else(|| workbook.sheet_names().first().cloned().unwrap_or_default());

    if let Some(range) = explicit_range {
        return Ok(TableTarget {
            sheet_name,
            table_name: None,
            range,
            header_hint: None,
        });
    }

    let metrics = workbook.get_sheet_metrics_fast(&sheet_name)?;
    let end_col = metrics.metrics.column_count.max(1);
    let end_row = metrics.metrics.row_count.max(1);
    Ok(TableTarget {
        sheet_name,
        table_name: None,
        range: ((1, 1), (end_col, end_row)),
        header_hint: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn extract_table_rows(
    sheet: &umya_spreadsheet::Worksheet,
    target: &TableTarget,
    header_row: Option<u32>,
    header_rows: Option<u32>,
    columns: Option<Vec<String>>,
    filters: Option<Vec<TableFilter>>,
    limit: usize,
    offset: usize,
    sample_mode: SampleMode,
) -> Result<(Vec<String>, Vec<TableRow>, u32)> {
    let ((start_col, start_row), (end_col, end_row)) = target.range;
    let mut header_start = header_row.or(target.header_hint).unwrap_or(start_row);
    if header_start < start_row {
        header_start = start_row;
    }
    if header_start > end_row {
        header_start = start_row;
    }
    let header_rows_count = header_rows.unwrap_or(1).max(1);
    let data_start_row = (header_start + header_rows_count).max(start_row + header_rows_count);
    let column_indices: Vec<u32> = if let Some(cols) = columns.as_ref() {
        resolve_columns(Some(cols), end_col).into_iter().collect()
    } else {
        (start_col..=end_col).collect()
    };

    let headers = build_headers(sheet, &column_indices, header_start, header_rows_count);
    let mut all_rows: Vec<TableRow> = Vec::new();
    let mut total_rows: u32 = 0;

    for row_idx in data_start_row..=end_row {
        let mut row = BTreeMap::new();
        for (i, col_idx) in column_indices.iter().enumerate() {
            let header = headers
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("Col{col_idx}"));
            let value = sheet.get_cell((*col_idx, row_idx)).and_then(cell_to_value);
            row.insert(header, value);
        }
        if !row_passes_filters(&row, filters.as_ref()) {
            continue;
        }
        total_rows += 1;
        if matches!(sample_mode, SampleMode::First) && total_rows as usize > offset + limit {
            continue;
        }
        all_rows.push(row);
    }

    let rows = sample_rows(all_rows, limit, offset, sample_mode);

    Ok((headers, rows, total_rows))
}

fn build_headers(
    sheet: &umya_spreadsheet::Worksheet,
    columns: &[u32],
    header_start: u32,
    header_rows: u32,
) -> Vec<String> {
    let mut headers = Vec::new();
    for col_idx in columns {
        let mut parts = Vec::new();
        for h in header_start..(header_start + header_rows) {
            let (origin_col, origin_row) = sheet.map_merged_cell((*col_idx, h));
            if let Some(value) = sheet
                .get_cell((origin_col, origin_row))
                .and_then(cell_to_value)
            {
                match value {
                    CellValue::Text(ref s) if s.trim().is_empty() => {}
                    CellValue::Text(s) => parts.push(s),
                    CellValue::Number(n) => parts.push(n.to_string()),
                    CellValue::Bool(b) => parts.push(b.to_string()),
                    CellValue::Error(e) => parts.push(e),
                    CellValue::Date(d) => parts.push(d),
                }
            }
        }
        if parts.is_empty() {
            headers.push(crate::utils::column_number_to_name(*col_idx));
        } else {
            headers.push(parts.join(" / "));
        }
    }

    if headers.iter().all(|h| h.trim().is_empty()) {
        return columns
            .iter()
            .map(|c| crate::utils::column_number_to_name(*c))
            .collect();
    }

    dedupe_headers(headers)
}

fn dedupe_headers(mut headers: Vec<String>) -> Vec<String> {
    let mut seen: HashMap<String, u32> = HashMap::new();
    for h in headers.iter_mut() {
        let key = h.clone();
        if key.trim().is_empty() {
            continue;
        }
        let count = seen.entry(key.clone()).or_insert(0);
        if *count > 0 {
            h.push_str(&format!("_{}", *count + 1));
        }
        *count += 1;
    }
    headers
}

fn row_passes_filters(row: &TableRow, filters: Option<&Vec<TableFilter>>) -> bool {
    if let Some(filters) = filters {
        for filter in filters {
            if let Some(value) = row.get(&filter.column) {
                match filter.op {
                    FilterOp::Eq => {
                        if !value_eq(value, &filter.value) {
                            return false;
                        }
                    }
                    FilterOp::Neq => {
                        if value_eq(value, &filter.value) {
                            return false;
                        }
                    }
                    FilterOp::Contains => {
                        if !value_contains(value, &filter.value) {
                            return false;
                        }
                    }
                    FilterOp::Gt => {
                        if !value_gt(value, &filter.value) {
                            return false;
                        }
                    }
                    FilterOp::Lt => {
                        if !value_lt(value, &filter.value) {
                            return false;
                        }
                    }
                    FilterOp::Gte => {
                        if !value_gte(value, &filter.value) {
                            return false;
                        }
                    }
                    FilterOp::Lte => {
                        if !value_lte(value, &filter.value) {
                            return false;
                        }
                    }
                    FilterOp::StartsWith => {
                        if !value_starts_with(value, &filter.value) {
                            return false;
                        }
                    }
                    FilterOp::EndsWith => {
                        if !value_ends_with(value, &filter.value) {
                            return false;
                        }
                    }
                    FilterOp::In => {
                        let list = filter
                            .value
                            .as_array()
                            .cloned()
                            .unwrap_or_else(|| vec![filter.value.clone()]);
                        if !list.iter().any(|cmp| value_eq(value, cmp)) {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}

fn value_eq(cell: &Option<CellValue>, cmp: &serde_json::Value) -> bool {
    match (cell, cmp) {
        (Some(CellValue::Text(s)), serde_json::Value::String(t)) => s == t,
        (Some(CellValue::Number(n)), serde_json::Value::Number(v)) => {
            v.as_f64().is_some_and(|f| (*n - f).abs() < f64::EPSILON)
        }
        (Some(CellValue::Number(n)), serde_json::Value::String(t)) => t
            .parse::<f64>()
            .map(|f| (*n - f).abs() < f64::EPSILON)
            .unwrap_or(false),
        (Some(CellValue::Bool(b)), serde_json::Value::Bool(v)) => b == v,
        (Some(CellValue::Bool(b)), serde_json::Value::String(t)) => {
            t.eq_ignore_ascii_case("true") == *b
        }
        (Some(CellValue::Date(d)), serde_json::Value::String(t)) => d == t,
        (None, serde_json::Value::Null) => true,
        _ => false,
    }
}

fn value_contains(cell: &Option<CellValue>, cmp: &serde_json::Value) -> bool {
    if let (Some(CellValue::Text(s)), serde_json::Value::String(t)) = (cell, cmp) {
        return s.to_ascii_lowercase().contains(&t.to_ascii_lowercase());
    }
    false
}

fn value_gt(cell: &Option<CellValue>, cmp: &serde_json::Value) -> bool {
    match (cell, cmp) {
        (Some(CellValue::Number(n)), serde_json::Value::Number(v)) => {
            v.as_f64().is_some_and(|f| *n > f)
        }
        _ => false,
    }
}

fn value_lt(cell: &Option<CellValue>, cmp: &serde_json::Value) -> bool {
    match (cell, cmp) {
        (Some(CellValue::Number(n)), serde_json::Value::Number(v)) => {
            v.as_f64().is_some_and(|f| *n < f)
        }
        _ => false,
    }
}

fn value_gte(cell: &Option<CellValue>, cmp: &serde_json::Value) -> bool {
    match (cell, cmp) {
        (Some(CellValue::Number(n)), serde_json::Value::Number(v)) => {
            v.as_f64().is_some_and(|f| *n >= f)
        }
        _ => false,
    }
}

fn value_lte(cell: &Option<CellValue>, cmp: &serde_json::Value) -> bool {
    match (cell, cmp) {
        (Some(CellValue::Number(n)), serde_json::Value::Number(v)) => {
            v.as_f64().is_some_and(|f| *n <= f)
        }
        _ => false,
    }
}

fn value_starts_with(cell: &Option<CellValue>, cmp: &serde_json::Value) -> bool {
    if let (Some(CellValue::Text(s)), serde_json::Value::String(t)) = (cell, cmp) {
        return s.to_ascii_lowercase().starts_with(&t.to_ascii_lowercase());
    }
    false
}

fn value_ends_with(cell: &Option<CellValue>, cmp: &serde_json::Value) -> bool {
    if let (Some(CellValue::Text(s)), serde_json::Value::String(t)) = (cell, cmp) {
        return s.to_ascii_lowercase().ends_with(&t.to_ascii_lowercase());
    }
    false
}

fn sample_rows(
    rows: Vec<TableRow>,
    limit: usize,
    offset: usize,
    mode: SampleMode,
) -> Vec<TableRow> {
    if rows.is_empty() {
        return rows;
    }

    match mode {
        SampleMode::Distributed => {
            if limit == 0 {
                return Vec::new();
            }
            let mut indices = Vec::new();
            let span = rows.len().saturating_sub(1);
            let step = std::cmp::max(1, span / std::cmp::max(1, limit.saturating_sub(1)));
            let mut idx = offset;
            while idx < rows.len() && indices.len() < limit {
                indices.push(idx);
                idx = idx.saturating_add(step);
                if idx == indices.last().copied().unwrap_or(0) {
                    idx += 1;
                }
            }
            if indices.len() < limit {
                let last_idx = rows.len().saturating_sub(1);
                if !indices.contains(&last_idx) {
                    indices.push(last_idx);
                }
            }
            indices
                .into_iter()
                .filter_map(|i| rows.get(i).cloned())
                .collect()
        }
        SampleMode::Last => {
            let start = rows.len().saturating_sub(limit + offset);
            rows.into_iter().skip(start + offset).take(limit).collect()
        }
        SampleMode::First => rows.into_iter().skip(offset).take(limit).collect(),
    }
}

fn summarize_columns(headers: &[String], rows: &[TableRow]) -> Vec<ColumnTypeSummary> {
    let mut summaries = Vec::new();
    for header in headers {
        let mut nulls = 0u32;
        let mut distinct_set: HashSet<String> = HashSet::new();
        let mut values: Vec<f64> = Vec::new();
        let mut top_counts: HashMap<String, u32> = HashMap::new();

        for row in rows {
            match row.get(header) {
                Some(Some(CellValue::Number(n))) => {
                    values.push(*n);
                    let key = n.to_string();
                    distinct_set.insert(key.clone());
                    *top_counts.entry(key).or_default() += 1;
                }
                Some(Some(CellValue::Text(s))) => {
                    distinct_set.insert(s.clone());
                    *top_counts.entry(s.clone()).or_default() += 1;
                }
                Some(Some(CellValue::Bool(b))) => {
                    let key = b.to_string();
                    distinct_set.insert(key.clone());
                    *top_counts.entry(key).or_default() += 1;
                }
                Some(Some(CellValue::Date(d))) => {
                    distinct_set.insert(d.clone());
                    *top_counts.entry(d.clone()).or_default() += 1;
                }
                Some(Some(CellValue::Error(e))) => {
                    distinct_set.insert(e.clone());
                    *top_counts.entry(e.clone()).or_default() += 1;
                }
                _ => {
                    nulls += 1;
                }
            }
        }

        let inferred_type = if !values.is_empty() {
            "number"
        } else if !distinct_set.is_empty() {
            "text"
        } else {
            "unknown"
        }
        .to_string();

        let min = values.iter().cloned().reduce(f64::min);
        let max = values.iter().cloned().reduce(f64::max);
        let mean = if values.is_empty() {
            None
        } else {
            Some(values.iter().sum::<f64>() / values.len() as f64)
        };

        let mut top_values: Vec<(String, u32)> = top_counts.into_iter().collect();
        top_values.sort_by(|a, b| b.1.cmp(&a.1));
        let top_values = top_values.into_iter().take(3).map(|(v, _)| v).collect();

        summaries.push(ColumnTypeSummary {
            name: header.clone(),
            inferred_type,
            nulls,
            distinct: distinct_set.len() as u32,
            top_values,
            min,
            max,
            mean,
        });
    }
    summaries
}

#[allow(clippy::too_many_arguments)]
fn collect_value_matches(
    sheet: &umya_spreadsheet::Worksheet,
    sheet_name: &str,
    mode: &FindMode,
    match_mode: MatchMode,
    direction: &LabelDirection,
    params: &FindValueParams,
    region: Option<&DetectedRegion>,
    table_bounds: Option<((u32, u32), (u32, u32))>,
    default_bounds: ((u32, u32), (u32, u32)),
    offset: u32,
    limit: u32,
    seen_so_far: u32,
) -> Result<(Vec<FindValueMatch>, u32, bool)> {
    let mut results = Vec::new();
    let mut seen = seen_so_far;
    let regex = if match_mode == MatchMode::Regex {
        Regex::new(&params.query).ok()
    } else {
        None
    };
    let bounds = table_bounds
        .or_else(|| region.as_ref().and_then(|r| parse_range(&r.bounds)))
        .unwrap_or(default_bounds);

    let header_row = table_bounds
        .map(|bounds| bounds.0.1)
        .or_else(|| region.and_then(|r| r.header_row))
        .unwrap_or(1);

    let context_mode = params.context.unwrap_or_default();
    let include_neighbors = matches!(context_mode, FindContext::Neighbors | FindContext::Both);
    let include_row_context = matches!(context_mode, FindContext::Row | FindContext::Both);
    let context_width = params.context_width.unwrap_or(3).max(1);

    for cell in sheet.get_cell_collection() {
        let coord = cell.get_coordinate();
        let col = *coord.get_col_num();
        let row = *coord.get_row_num();
        if col < bounds.0.0 || col > bounds.1.0 || row < bounds.0.1 || row > bounds.1.1 {
            continue;
        }
        if params.search_headers_only && row != header_row {
            continue;
        }

        let value = cell_to_value(cell);
        if let Some(ref allowed) = params.value_types
            && !value_type_matches(&value, allowed)
        {
            continue;
        }
        if matches!(mode, FindMode::Value) {
            if !value_matches(
                &value,
                &params.query,
                match_mode,
                params.case_sensitive,
                &regex,
            ) {
                continue;
            }
        } else if let Some(label) = &params.label {
            if !label_matches(cell, label, match_mode, params.case_sensitive, &regex) {
                continue;
            }
        } else {
            continue;
        }

        if seen < offset {
            seen += 1;
            continue;
        }

        if results.len() as u32 >= limit {
            return Ok((results, seen, true));
        }

        let neighbors = if include_neighbors {
            collect_neighbors(sheet, row, col)
        } else {
            None
        };
        let (label_hit, match_value) = if matches!(mode, FindMode::Label) {
            let target_value = match direction {
                LabelDirection::Right => sheet.get_cell((col + 1, row)),
                LabelDirection::Below => sheet.get_cell((col, row + 1)),
                LabelDirection::Any => sheet
                    .get_cell((col + 1, row))
                    .or_else(|| sheet.get_cell((col, row + 1))),
            }
            .and_then(cell_to_value);
            if target_value.is_none() {
                continue;
            }
            (
                Some(LabelHit {
                    label_address: coord.get_coordinate(),
                    label: label_from_cell(cell),
                }),
                target_value,
            )
        } else {
            (None, value.clone())
        };

        let row_context = if include_row_context {
            build_row_context(sheet, row, col, context_width)
        } else {
            None
        };

        results.push(FindValueMatch {
            address: coord.get_coordinate(),
            sheet_name: sheet_name.to_string(),
            value: match_value,
            row_context,
            neighbors,
            label_hit,
        });
        seen += 1;
    }

    Ok((results, seen, false))
}

fn label_from_cell(cell: &umya_spreadsheet::Cell) -> String {
    cell_to_value(cell)
        .map(|v| match v {
            CellValue::Text(s) => s,
            CellValue::Number(n) => n.to_string(),
            CellValue::Bool(b) => b.to_string(),
            CellValue::Date(d) => d,
            CellValue::Error(e) => e,
        })
        .unwrap_or_else(|| cell.get_value().to_string())
}

fn value_matches(
    value: &Option<CellValue>,
    query: &str,
    mode: MatchMode,
    case_sensitive: bool,
    regex: &Option<Regex>,
) -> bool {
    if value.is_none() {
        return false;
    }
    let haystack = cell_value_to_string_lower(value.clone().unwrap());
    let needle = if case_sensitive {
        query.to_string()
    } else {
        query.to_ascii_lowercase()
    };

    match mode {
        MatchMode::Exact => haystack == needle,
        MatchMode::Prefix => haystack.starts_with(&needle),
        MatchMode::Regex => regex
            .as_ref()
            .map(|re| re.is_match(&haystack))
            .unwrap_or(false),
        MatchMode::Contains => haystack.contains(&needle),
    }
}

fn label_matches(
    cell: &umya_spreadsheet::Cell,
    label: &str,
    mode: MatchMode,
    case_sensitive: bool,
    regex: &Option<Regex>,
) -> bool {
    let value = cell_to_value(cell);
    if value.is_none() {
        return false;
    }
    let haystack = cell_value_to_string_lower(value.unwrap());
    let needle = if case_sensitive {
        label.to_string()
    } else {
        label.to_ascii_lowercase()
    };
    match mode {
        MatchMode::Exact => haystack == needle,
        MatchMode::Prefix => haystack.starts_with(&needle),
        MatchMode::Regex => regex
            .as_ref()
            .map(|re| re.is_match(&haystack))
            .unwrap_or(false),
        MatchMode::Contains => haystack.contains(&needle),
    }
}

fn value_type_matches(value: &Option<CellValue>, allowed: &[ValueTypeFilter]) -> bool {
    if value.is_none() {
        return allowed.contains(&ValueTypeFilter::Null);
    }
    match value.as_ref().unwrap() {
        CellValue::Text(_) => allowed.contains(&ValueTypeFilter::Text),
        CellValue::Number(_) => allowed.contains(&ValueTypeFilter::Number),
        CellValue::Bool(_) => allowed.contains(&ValueTypeFilter::Bool),
        CellValue::Date(_) => allowed.contains(&ValueTypeFilter::Date),
        CellValue::Error(_) => true,
    }
}

fn collect_neighbors(
    sheet: &umya_spreadsheet::Worksheet,
    row: u32,
    col: u32,
) -> Option<NeighborValues> {
    Some(NeighborValues {
        left: if col > 1 {
            sheet.get_cell((col - 1, row)).and_then(cell_to_value)
        } else {
            None
        },
        right: sheet.get_cell((col + 1, row)).and_then(cell_to_value),
        up: if row > 1 {
            sheet.get_cell((col, row - 1)).and_then(cell_to_value)
        } else {
            None
        },
        down: sheet.get_cell((col, row + 1)).and_then(cell_to_value),
    })
}

fn build_row_context(
    sheet: &umya_spreadsheet::Worksheet,
    row: u32,
    col: u32,
    width: u32,
) -> Option<RowContext> {
    let width = width.max(1);
    let half = width / 2;
    let max_col = sheet.get_highest_column().max(1);
    let start_col = col.saturating_sub(half).max(1);
    let end_col = (col + half).min(max_col);

    let mut headers = Vec::new();
    let mut values = Vec::new();

    for current_col in start_col..=end_col {
        let header_value = sheet
            .get_cell((current_col, 1u32))
            .and_then(cell_to_value)
            .map(|v| match v {
                CellValue::Text(s) => s,
                CellValue::Number(n) => n.to_string(),
                CellValue::Bool(b) => b.to_string(),
                CellValue::Date(d) => d,
                CellValue::Error(e) => e,
            })
            .unwrap_or_else(|| format!("Col{}", current_col));
        let value = sheet.get_cell((current_col, row)).and_then(cell_to_value);
        headers.push(header_value);
        values.push(value);
    }

    Some(RowContext { headers, values })
}

fn default_find_formula_limit() -> u32 {
    50
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct FindFormulaParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Text to search for in formulas (e.g., "SUM(", "VLOOKUP")
    pub query: String,
    /// Limit to specific sheet (searches all if omitted)
    pub sheet_name: Option<String>,
    /// Case-sensitive matching (default: false)
    #[serde(default)]
    pub case_sensitive: bool,
    /// Include header row and cell context (default: false)
    #[serde(default)]
    pub include_context: bool,
    /// Maximum matches to return (default: 50)
    #[serde(default = "default_find_formula_limit")]
    pub limit: u32,
    /// Offset for pagination; use next_offset from previous response
    #[serde(default)]
    pub offset: u32,
    /// Rows of context to include above/below (requires include_context=true)
    #[serde(default)]
    pub context_rows: Option<u32>,
    /// Columns of context to include left/right (requires include_context=true)
    #[serde(default)]
    pub context_cols: Option<u32>,
}

pub async fn find_formula(
    state: impl crate::read_context::ReadContext,
    params: FindFormulaParams,
) -> Result<FindFormulaResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let query = if params.case_sensitive {
        params.query.clone()
    } else {
        params.query.to_ascii_lowercase()
    };

    let sheet_names: Vec<String> = if let Some(sheet) = &params.sheet_name {
        vec![sheet.clone()]
    } else {
        workbook.sheet_names()
    };

    let limit = params.limit.clamp(1, 500);
    let offset = params.offset;
    let context_rows = params.context_rows.unwrap_or(1);
    let context_cols = params.context_cols.unwrap_or(1);

    let mut matches = Vec::new();
    let mut seen: u32 = 0;
    let mut truncated = false;

    for sheet_name in sheet_names {
        let (sheet_matches, sheet_seen, sheet_truncated) =
            workbook.with_sheet(&sheet_name, |sheet| {
                collect_formula_matches(
                    sheet,
                    &sheet_name,
                    &query,
                    params.case_sensitive,
                    params.include_context,
                    context_rows,
                    context_cols,
                    offset,
                    limit,
                    seen,
                )
            })?;

        seen = sheet_seen;
        truncated |= sheet_truncated;
        matches.extend(sheet_matches);

        if truncated {
            break;
        }
    }

    let next_offset = if truncated {
        Some(offset.saturating_add(matches.len() as u32))
    } else {
        None
    };

    let response = FindFormulaResponse {
        workbook_id: workbook.id.clone(),
        matches,
        next_offset,
    };
    Ok(response)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScanVolatilesParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Limit to specific sheet (scans all if omitted)
    pub sheet_name: Option<String>,
    /// Return counts only without addresses (default: true in token_dense profile)
    #[serde(default)]
    pub summary_only: Option<bool>,
    /// Include cell addresses for each volatile (default: !summary_only)
    #[serde(default)]
    pub include_addresses: Option<bool>,
    /// Maximum addresses to include per volatile function (default: 15)
    #[serde(default)]
    pub addresses_limit: Option<u32>,
    /// Maximum entries to return for this page
    #[serde(default)]
    pub limit: Option<u32>,
    /// Entry offset for pagination; use next_offset from previous response
    #[serde(default)]
    pub offset: Option<u32>,
    /// Formula parse policy: fail, warn (default), or off
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
}

pub async fn scan_volatiles(
    state: impl crate::read_context::ReadContext,
    params: ScanVolatilesParams,
) -> Result<VolatileScanResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let config = state.config();
    let output_profile = config.output_profile();
    let summary_only = params
        .summary_only
        .unwrap_or(matches!(output_profile, OutputProfile::TokenDense));
    let include_addresses = params.include_addresses.unwrap_or(!summary_only);
    let addresses_limit = params.addresses_limit.unwrap_or(15);
    let policy = params
        .formula_parse_policy
        .unwrap_or(FormulaParsePolicy::Warn);
    let mut combined_builder = FormulaParseDiagnosticsBuilder::new(policy);
    let max_items = config.max_items();
    let max_payload_bytes = config.max_payload_bytes();

    let mut target_sheets: Vec<String> = if let Some(sheet) = &params.sheet_name {
        vec![sheet.clone()]
    } else {
        workbook.sheet_names()
    };
    target_sheets.sort_by(|left, right| {
        left.to_ascii_lowercase()
            .cmp(&right.to_ascii_lowercase())
            .then_with(|| left.cmp(right))
    });

    let mut items = Vec::new();
    for sheet_name in target_sheets {
        let graph = workbook.formula_graph_with_diagnostics_builder(
            &sheet_name,
            policy,
            &mut combined_builder,
        )?;
        let mut groups = graph
            .groups()
            .into_iter()
            .filter(|group| group.is_volatile)
            .collect::<Vec<_>>();
        groups.sort_by(|left, right| {
            left.formula
                .cmp(&right.formula)
                .then_with(|| left.fingerprint.cmp(&right.fingerprint))
                .then_with(|| left.addresses.len().cmp(&right.addresses.len()))
        });

        for group in groups {
            if summary_only {
                items.push(VolatileScanEntry {
                    address: String::new(),
                    sheet_name: sheet_name.clone(),
                    function: "volatile".to_string(),
                    note: Some(format!("Count: {}", group.addresses.len())),
                });
                continue;
            }

            if include_addresses {
                let mut addresses = group.addresses;
                addresses.sort();
                for address in addresses.into_iter().take(addresses_limit as usize) {
                    items.push(VolatileScanEntry {
                        address,
                        sheet_name: sheet_name.clone(),
                        function: "volatile".to_string(),
                        note: Some(group.formula.clone()),
                    });
                }
            } else {
                items.push(VolatileScanEntry {
                    address: String::new(),
                    sheet_name: sheet_name.clone(),
                    function: "volatile".to_string(),
                    note: Some(group.formula.clone()),
                });
            }
        }
    }

    let diagnostics = combined_builder.build();
    let formula_parse_diagnostics = if diagnostics.total_errors > 0 {
        Some(diagnostics)
    } else {
        None
    };

    items.sort_by(|left, right| {
        left.sheet_name
            .cmp(&right.sheet_name)
            .then_with(|| left.function.cmp(&right.function))
            .then_with(|| left.note.cmp(&right.note))
            .then_with(|| left.address.cmp(&right.address))
    });

    let total_items = items.len();
    let offset = params.offset.unwrap_or(0) as usize;
    let page_limit = params
        .limit
        .map(|limit| limit.max(1) as usize)
        .unwrap_or(usize::MAX);
    let start = offset.min(total_items);
    let end = start.saturating_add(page_limit).min(total_items);
    let mut page_items = items[start..end].to_vec();

    if let Some(max_items) = max_items
        && page_items.len() > max_items
    {
        page_items.truncate(max_items);
    }

    if let Some(max_bytes) = max_payload_bytes {
        let item_limit = cap_rows_by_payload_bytes(page_items.len(), Some(max_bytes), |count| {
            let response = VolatileScanResponse {
                workbook_id: workbook.id.clone(),
                items: page_items[..count].to_vec(),
                formula_parse_diagnostics: formula_parse_diagnostics.clone(),
                next_offset: None,
            };
            serde_json::to_vec(&response)
                .map(|payload| payload.len())
                .unwrap_or(usize::MAX)
        });

        if item_limit < page_items.len() {
            page_items.truncate(item_limit);
        }
    }

    let emitted = page_items.len();
    let absolute_next = offset.saturating_add(emitted);
    let next_offset = if emitted > 0 && absolute_next < total_items {
        Some(absolute_next as u32)
    } else {
        None
    };

    let response = VolatileScanResponse {
        workbook_id: workbook.id.clone(),
        items: page_items,
        formula_parse_diagnostics,
        next_offset,
    };
    Ok(response)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkbookStyleSummaryParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Maximum distinct styles to return (default: 50)
    pub max_styles: Option<u32>,
    /// Maximum conditional format rules to return (default: 20)
    pub max_conditional_formats: Option<u32>,
    /// Maximum cells to scan per sheet (default: 10000)
    pub max_cells_scan: Option<u32>,
    /// Return counts and tags only, no descriptors (default: true in token_dense profile)
    #[serde(default)]
    pub summary_only: Option<bool>,
    /// Include full style descriptors (fonts, fills, borders)
    #[serde(default)]
    pub include_descriptor: Option<bool>,
    /// Include example cell addresses for each style
    #[serde(default)]
    pub include_example_cells: Option<bool>,
    /// Include workbook theme colors
    #[serde(default)]
    pub include_theme: Option<bool>,
    /// Include conditional formatting rules
    #[serde(default)]
    pub include_conditional_formats: Option<bool>,
}

#[derive(Debug)]
struct WorkbookStyleAccum {
    descriptor: StyleDescriptor,
    occurrences: u32,
    tags: HashSet<String>,
    example_cells: Vec<String>,
}

impl WorkbookStyleAccum {
    fn new(descriptor: StyleDescriptor) -> Self {
        Self {
            descriptor,
            occurrences: 0,
            tags: HashSet::new(),
            example_cells: Vec::new(),
        }
    }
}

pub async fn workbook_style_summary(
    state: impl crate::read_context::ReadContext,
    params: WorkbookStyleSummaryParams,
) -> Result<WorkbookStyleSummaryResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let config = state.config();
    let output_profile = config.output_profile();
    let summary_only = params
        .summary_only
        .unwrap_or(matches!(output_profile, OutputProfile::TokenDense));
    let include_descriptor = params.include_descriptor.unwrap_or(!summary_only);
    let include_example_cells = params.include_example_cells.unwrap_or(!summary_only);
    let include_theme = params.include_theme.unwrap_or(!summary_only);
    let include_conditional_formats = params.include_conditional_formats.unwrap_or(!summary_only);
    let max_payload_bytes = config.max_payload_bytes();
    let sheet_names = workbook.sheet_names();

    const STYLE_EXAMPLE_LIMIT: usize = 5;
    const STYLE_LIMIT_DEFAULT: usize = 200;
    const CF_LIMIT_DEFAULT: usize = 200;
    const CELL_SCAN_LIMIT_DEFAULT: usize = 500_000;

    let style_limit = params
        .max_styles
        .map(|v| v as usize)
        .unwrap_or(STYLE_LIMIT_DEFAULT);
    let style_limit = config
        .max_items()
        .map(|limit| style_limit.min(limit))
        .unwrap_or(style_limit);
    let cf_limit = params
        .max_conditional_formats
        .map(|v| v as usize)
        .unwrap_or(CF_LIMIT_DEFAULT);
    let cf_limit = config
        .max_items()
        .map(|limit| cf_limit.min(limit))
        .unwrap_or(cf_limit);
    let cell_scan_limit = params
        .max_cells_scan
        .map(|v| v as usize)
        .unwrap_or(CELL_SCAN_LIMIT_DEFAULT);

    let mut acc: HashMap<String, WorkbookStyleAccum> = HashMap::new();
    let mut scanned_cells: usize = 0;
    let mut scan_truncated = false;

    for sheet_name in &sheet_names {
        if scan_truncated {
            break;
        }
        workbook.with_sheet(sheet_name, |sheet| {
            for cell in sheet.get_cell_collection() {
                if scanned_cells >= cell_scan_limit {
                    scan_truncated = true;
                    break;
                }
                scanned_cells += 1;

                let address = cell.get_coordinate().get_coordinate().to_string();
                let descriptor = crate::styles::descriptor_from_style(cell.get_style());
                let style_id = crate::styles::stable_style_id(&descriptor);

                let entry = acc
                    .entry(style_id.clone())
                    .or_insert_with(|| WorkbookStyleAccum::new(descriptor.clone()));
                entry.occurrences += 1;
                if entry.example_cells.len() < STYLE_EXAMPLE_LIMIT {
                    entry.example_cells.push(format!("{sheet_name}!{address}"));
                }

                if let Some((_, tagging)) = crate::analysis::style::tag_cell(cell) {
                    for tag in tagging.tags {
                        entry.tags.insert(tag);
                    }
                }
            }
        })?;
    }

    let total_styles = acc.len() as u32;
    let mut styles: Vec<WorkbookStyleUsage> = acc
        .into_iter()
        .map(|(style_id, entry)| {
            let mut tags: Vec<String> = entry.tags.into_iter().collect();
            tags.sort();
            WorkbookStyleUsage {
                style_id,
                occurrences: entry.occurrences,
                tags,
                example_cells: if include_example_cells {
                    entry.example_cells
                } else {
                    Vec::new()
                },
                descriptor: if include_descriptor {
                    Some(entry.descriptor)
                } else {
                    None
                },
            }
        })
        .collect();

    styles.sort_by(|a, b| {
        b.occurrences
            .cmp(&a.occurrences)
            .then_with(|| a.style_id.cmp(&b.style_id))
    });

    let inferred_default_style_id = styles.first().map(|s| s.style_id.clone());
    let mut inferred_default_font = styles
        .first()
        .and_then(|s| s.descriptor.as_ref().and_then(|d| d.font.clone()));

    let mut styles_truncated = if styles.len() > style_limit {
        styles.truncate(style_limit);
        true
    } else {
        false
    };

    let theme = workbook.with_spreadsheet(|book| {
        let theme = book.get_theme();
        let elements = theme.get_theme_elements();
        let scheme = elements.get_color_scheme();
        let mut colors = BTreeMap::new();

        let mut insert_color = |name: &str, value: String| {
            if !value.trim().is_empty() {
                colors.insert(name.to_string(), value);
            }
        };

        insert_color("dk1", scheme.get_dk1().get_val());
        insert_color("lt1", scheme.get_lt1().get_val());
        insert_color("dk2", scheme.get_dk2().get_val());
        insert_color("lt2", scheme.get_lt2().get_val());
        insert_color("accent1", scheme.get_accent1().get_val());
        insert_color("accent2", scheme.get_accent2().get_val());
        insert_color("accent3", scheme.get_accent3().get_val());
        insert_color("accent4", scheme.get_accent4().get_val());
        insert_color("accent5", scheme.get_accent5().get_val());
        insert_color("accent6", scheme.get_accent6().get_val());
        insert_color("hlink", scheme.get_hlink().get_val());
        insert_color("fol_hlink", scheme.get_fol_hlink().get_val());

        let font_scheme = elements.get_font_scheme();
        let major = font_scheme.get_major_font();
        let minor = font_scheme.get_minor_font();
        let font_scheme_summary = ThemeFontSchemeSummary {
            major_latin: Some(major.get_latin_font().get_typeface().to_string())
                .filter(|s| !s.trim().is_empty()),
            major_east_asian: Some(major.get_east_asian_font().get_typeface().to_string())
                .filter(|s| !s.trim().is_empty()),
            major_complex_script: Some(major.get_complex_script_font().get_typeface().to_string())
                .filter(|s| !s.trim().is_empty()),
            minor_latin: Some(minor.get_latin_font().get_typeface().to_string())
                .filter(|s| !s.trim().is_empty()),
            minor_east_asian: Some(minor.get_east_asian_font().get_typeface().to_string())
                .filter(|s| !s.trim().is_empty()),
            minor_complex_script: Some(minor.get_complex_script_font().get_typeface().to_string())
                .filter(|s| !s.trim().is_empty()),
        };

        ThemeSummary {
            name: Some(theme.get_name().to_string()).filter(|s| !s.trim().is_empty()),
            colors,
            font_scheme: font_scheme_summary,
        }
    })?;

    if inferred_default_font.is_none()
        && let Some(name) = theme
            .font_scheme
            .minor_latin
            .clone()
            .or_else(|| theme.font_scheme.major_latin.clone())
    {
        inferred_default_font = Some(FontDescriptor {
            name: Some(name),
            size: None,
            bold: None,
            italic: None,
            underline: None,
            strikethrough: None,
            color: None,
        });
    }

    let theme = if include_theme { Some(theme) } else { None };

    let mut conditional_formats: Vec<ConditionalFormatSummary> = Vec::new();
    let mut conditional_formats_truncated = false;
    if include_conditional_formats {
        use umya_spreadsheet::structs::EnumTrait;
        for sheet_name in &sheet_names {
            if conditional_formats_truncated {
                break;
            }
            workbook.with_sheet(sheet_name, |sheet| {
                for cf in sheet.get_conditional_formatting_collection() {
                    if conditional_formats.len() >= cf_limit {
                        conditional_formats_truncated = true;
                        break;
                    }
                    let range = cf.get_sequence_of_references().get_sqref().to_string();
                    let mut types: HashSet<String> = HashSet::new();
                    for rule in cf.get_conditional_collection() {
                        types.insert(rule.get_type().get_value_string().to_string());
                    }
                    let mut rule_types: Vec<String> = types.into_iter().collect();
                    rule_types.sort();
                    conditional_formats.push(ConditionalFormatSummary {
                        sheet_name: sheet_name.clone(),
                        range,
                        rule_types,
                        rule_count: cf.get_conditional_collection().len() as u32,
                    });
                }
            })?;
        }
    }

    let mut notes: Vec<String> = Vec::new();
    if scan_truncated {
        notes.push(format!(
            "Stopped scanning after {cell_scan_limit} cells; style counts may be incomplete."
        ));
    }
    notes.push(
        "Named styles are not directly exposed by umya-spreadsheet; styles here are inferred from cell formatting."
            .to_string(),
    );

    if let Some(max_bytes) = max_payload_bytes {
        let style_limit = cap_rows_by_payload_bytes(styles.len(), Some(max_bytes), |count| {
            let response = WorkbookStyleSummaryResponse {
                workbook_id: workbook.id.clone(),
                theme: theme.clone(),
                inferred_default_style_id: inferred_default_style_id.clone(),
                inferred_default_font: inferred_default_font.clone(),
                styles: styles[..count].to_vec(),
                total_styles,
                styles_truncated: false,
                conditional_formats: conditional_formats.clone(),
                conditional_formats_truncated,
                scan_truncated,
                notes: notes.clone(),
            };
            serde_json::to_vec(&response)
                .map(|payload| payload.len())
                .unwrap_or(usize::MAX)
        });

        if style_limit < styles.len() {
            styles.truncate(style_limit);
            styles_truncated = true;
        }

        if !conditional_formats.is_empty() {
            let cf_limit =
                cap_rows_by_payload_bytes(conditional_formats.len(), Some(max_bytes), |count| {
                    let response = WorkbookStyleSummaryResponse {
                        workbook_id: workbook.id.clone(),
                        theme: theme.clone(),
                        inferred_default_style_id: inferred_default_style_id.clone(),
                        inferred_default_font: inferred_default_font.clone(),
                        styles: styles.clone(),
                        total_styles,
                        styles_truncated,
                        conditional_formats: conditional_formats[..count].to_vec(),
                        conditional_formats_truncated: false,
                        scan_truncated,
                        notes: notes.clone(),
                    };
                    serde_json::to_vec(&response)
                        .map(|payload| payload.len())
                        .unwrap_or(usize::MAX)
                });

            if cf_limit < conditional_formats.len() {
                conditional_formats.truncate(cf_limit);
                conditional_formats_truncated = true;
            }
        }
    }

    Ok(WorkbookStyleSummaryResponse {
        workbook_id: workbook.id.clone(),
        theme,
        inferred_default_style_id,
        inferred_default_font,
        styles,
        total_styles,
        styles_truncated,
        conditional_formats,
        conditional_formats_truncated,
        scan_truncated,
        notes,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetStylesParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Sheet name
    pub sheet_name: String,
    /// Limit scope: use range (e.g., "A1:D100") or region_id
    #[serde(default)]
    pub scope: Option<SheetStylesScope>,
    /// Granularity for style grouping
    #[serde(default)]
    pub granularity: Option<StyleGranularity>,
    /// Maximum style entries to return (default: 100)
    #[serde(default)]
    pub max_items: Option<usize>,
    /// Return counts and tags only (default: true in token_dense profile)
    #[serde(default)]
    pub summary_only: Option<bool>,
    /// Include full style descriptors (fonts, fills, borders)
    #[serde(default)]
    pub include_descriptor: Option<bool>,
    /// Include cell ranges for each style
    #[serde(default)]
    pub include_ranges: Option<bool>,
    /// Include example cell addresses
    #[serde(default)]
    pub include_example_cells: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SheetStylesScope {
    Range { range: String },
    Region { region_id: u32 },
}

#[derive(Debug)]
struct StyleAccum {
    descriptor: StyleDescriptor,
    occurrences: u32,
    tags: HashSet<String>,
    example_cells: Vec<String>,
    positions: Vec<(u32, u32)>,
}

impl StyleAccum {
    fn new(descriptor: StyleDescriptor) -> Self {
        Self {
            descriptor,
            occurrences: 0,
            tags: HashSet::new(),
            example_cells: Vec::new(),
            positions: Vec::new(),
        }
    }
}

pub(crate) struct BoundedSheetStyles {
    pub response: SheetStylesResponse,
    pub cells_scanned: u64,
    pub cells_in_scope: u64,
    pub scan_truncated: bool,
}

pub async fn sheet_styles(
    state: impl crate::read_context::ReadContext,
    params: SheetStylesParams,
) -> Result<SheetStylesResponse> {
    Ok(sheet_styles_bounded(state, params, None).await?.response)
}

pub(crate) async fn sheet_styles_bounded(
    state: impl crate::read_context::ReadContext,
    params: SheetStylesParams,
    max_cells_scan: Option<u32>,
) -> Result<BoundedSheetStyles> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let config = state.config();
    let output_profile = config.output_profile();
    let summary_only = params
        .summary_only
        .unwrap_or(matches!(output_profile, OutputProfile::TokenDense));
    let include_descriptor = params.include_descriptor.unwrap_or(!summary_only);
    let include_ranges = params.include_ranges.unwrap_or(!summary_only);
    let include_example_cells = params.include_example_cells.unwrap_or(!summary_only);
    const STYLE_EXAMPLE_LIMIT: usize = 5;
    const STYLE_RANGE_LIMIT: usize = 50;
    const STYLE_LIMIT: usize = 200;
    const MAX_MAX_ITEMS: usize = 5000;

    let max_payload_bytes = config.max_payload_bytes();
    let style_limit = config
        .max_items()
        .map(|limit| STYLE_LIMIT.min(limit))
        .unwrap_or(STYLE_LIMIT);

    let metrics = workbook.get_sheet_metrics_fast(&params.sheet_name)?;
    let full_bounds = (
        (1, 1),
        (
            metrics.metrics.column_count.max(1),
            metrics.metrics.row_count.max(1),
        ),
    );

    let bounds = match &params.scope {
        Some(SheetStylesScope::Range { range }) => {
            parse_range(range).ok_or_else(|| anyhow!("invalid range: {}", range))?
        }
        Some(SheetStylesScope::Region { region_id }) => {
            let region = workbook.detected_region(&params.sheet_name, *region_id)?;
            parse_range(&region.bounds)
                .ok_or_else(|| anyhow!("invalid region bounds: {}", region.bounds))?
        }
        None => full_bounds,
    };

    let granularity = params.granularity.unwrap_or_default();

    let max_items = params
        .max_items
        .unwrap_or(STYLE_RANGE_LIMIT)
        .clamp(1, MAX_MAX_ITEMS);

    let scan_limit = max_cells_scan.map(u64::from).unwrap_or(u64::MAX);
    let (
        mut styles,
        total_styles,
        mut styles_truncated,
        cells_scanned,
        cells_in_scope,
        scan_truncated,
    ) = workbook.with_sheet(&params.sheet_name, |sheet| {
        let mut acc: HashMap<String, StyleAccum> = HashMap::new();
        let mut cells_scanned = 0_u64;
        let mut cells_in_scope = 0_u64;

        for cell in sheet.get_cell_collection() {
            let address = cell.get_coordinate().get_coordinate().to_string();
            let Some((col, row)) = parse_address(&address) else {
                continue;
            };
            if col < bounds.0.0 || col > bounds.1.0 || row < bounds.0.1 || row > bounds.1.1 {
                continue;
            }
            cells_in_scope += 1;
            if cells_scanned >= scan_limit {
                continue;
            }
            cells_scanned += 1;

            let descriptor = crate::styles::descriptor_from_style(cell.get_style());
            let style_id = crate::styles::stable_style_id(&descriptor);

            let entry = acc
                .entry(style_id.clone())
                .or_insert_with(|| StyleAccum::new(descriptor.clone()));
            entry.occurrences += 1;
            if entry.example_cells.len() < STYLE_EXAMPLE_LIMIT {
                entry.example_cells.push(address.clone());
            }

            if let Some((_, tagging)) = crate::analysis::style::tag_cell(cell) {
                for tag in tagging.tags {
                    entry.tags.insert(tag);
                }
            }

            entry.positions.push((row, col));
        }

        let mut summaries: Vec<StyleSummary> = acc
            .into_iter()
            .map(|(style_id, mut entry)| {
                entry.positions.sort_unstable();
                entry.positions.dedup();

                let (cell_ranges, ranges_truncated) = if include_ranges {
                    if granularity == StyleGranularity::Cells {
                        let mut out = Vec::new();
                        for (row, col) in entry.positions.iter().take(max_items) {
                            out.push(crate::utils::cell_address(*col, *row));
                        }
                        (out, entry.positions.len() > max_items)
                    } else {
                        crate::styles::compress_positions_to_ranges(&entry.positions, max_items)
                    }
                } else {
                    (Vec::new(), false)
                };

                StyleSummary {
                    style_id,
                    occurrences: entry.occurrences,
                    tags: entry.tags.into_iter().collect(),
                    example_cells: if include_example_cells {
                        entry.example_cells
                    } else {
                        Vec::new()
                    },
                    descriptor: if include_descriptor {
                        Some(entry.descriptor)
                    } else {
                        None
                    },
                    cell_ranges,
                    ranges_truncated,
                }
            })
            .collect();

        summaries.sort_by(|a, b| {
            b.occurrences
                .cmp(&a.occurrences)
                .then_with(|| a.style_id.cmp(&b.style_id))
        });

        let total = summaries.len() as u32;
        let truncated = if summaries.len() > style_limit {
            summaries.truncate(style_limit);
            true
        } else {
            false
        };

        Ok::<_, anyhow::Error>((
            summaries,
            total,
            truncated,
            cells_scanned,
            cells_in_scope,
            cells_scanned < cells_in_scope,
        ))
    })??;

    if let Some(max_bytes) = max_payload_bytes {
        let row_limit = cap_rows_by_payload_bytes(styles.len(), Some(max_bytes), |count| {
            let response = SheetStylesResponse {
                workbook_id: workbook.id.clone(),
                sheet_name: params.sheet_name.clone(),
                styles: styles[..count].to_vec(),
                conditional_rules: Vec::new(),
                total_styles,
                styles_truncated: false,
            };
            serde_json::to_vec(&response)
                .map(|payload| payload.len())
                .unwrap_or(usize::MAX)
        });
        if row_limit < styles.len() {
            styles.truncate(row_limit);
            styles_truncated = true;
        }
    }

    Ok(BoundedSheetStyles {
        response: SheetStylesResponse {
            workbook_id: workbook.id.clone(),
            sheet_name: params.sheet_name.clone(),
            styles,
            conditional_rules: Vec::new(),
            total_styles,
            styles_truncated,
        },
        cells_scanned,
        cells_in_scope,
        scan_truncated,
    })
}

pub async fn range_values(
    state: impl crate::read_context::ReadContext,
    params: RangeValuesParams,
) -> Result<RangeValuesResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let calculation = workbook.calculation_metadata();
    let config = state.config();
    let output_profile = config.output_profile();
    let format = params.format.unwrap_or(match output_profile {
        OutputProfile::TokenDense => TableOutputFormat::Dense,
        OutputProfile::Verbose => TableOutputFormat::Json,
    });
    let include_headers = params.include_headers.unwrap_or(false);
    let include_formulas = params.include_formulas.unwrap_or(false);
    if let Some(page_size) = params.page_size
        && page_size == 0
    {
        return Err(anyhow!("page_size must be greater than zero"));
    }
    let max_cells = config.max_cells();
    let max_payload_bytes = config.max_payload_bytes();
    // Fail loudly on unparseable ranges: silently dropping them produced an
    // exit-0 response with no values (the worst failure shape for an agent).
    let mut parsed_ranges = Vec::with_capacity(params.ranges.len());
    for range in &params.ranges {
        match parse_range(range) {
            Some(bounds) => parsed_ranges.push((range.as_str(), bounds)),
            None => {
                return Err(anyhow!(
                    "INVALID_RANGE: '{}' is not a valid A1-style range (expected 'A1:C10' or a single cell like 'B2'). To read a named range, resolve it first with `asp read names` and use its refers_to address.",
                    range
                ));
            }
        }
    }
    #[cfg(feature = "recalc")]
    let requested_bounds: Vec<((u32, u32), (u32, u32))> =
        parsed_ranges.iter().map(|(_, b)| *b).collect();

    #[cfg(feature = "recalc")]
    let (values, has_formula_in_target) = workbook.with_sheet(&params.sheet_name, |sheet| {
        let has_formula_in_target = sheet_has_formula_in_bounds(sheet, &requested_bounds);
        let values = parsed_ranges
            .iter()
            .map(|(range, bounds)| {
                let ((start_col, start_row), (end_col, end_row)) = *bounds;
                let total_rows = (end_row - start_row + 1) as usize;
                let total_cols = (end_col - start_col + 1) as usize;
                let mut row_limit = total_rows;
                if let Some(page_size) = params.page_size {
                    row_limit = row_limit.min(page_size as usize);
                }

                let include_formula_matrix = include_formulas
                    && matches!(format, TableOutputFormat::Json | TableOutputFormat::Dense);
                let mut rows = Vec::new();
                let mut formula_rows = include_formula_matrix.then(Vec::new);
                for r in start_row..=end_row {
                    if rows.len() >= row_limit {
                        break;
                    }
                    let mut row_vals = Vec::new();
                    let mut row_formulas = include_formula_matrix.then(Vec::new);
                    for c in start_col..=end_col {
                        let row_index = if include_headers && r == start_row && start_row == 1 {
                            1u32
                        } else {
                            r
                        };
                        let cell = sheet.get_cell((c, row_index));
                        row_vals.push(cell.and_then(cell_to_value));
                        if let Some(formulas) = row_formulas.as_mut() {
                            formulas.push(cell.and_then(|entry| {
                                entry.is_formula().then(|| entry.get_formula().to_string())
                            }));
                        }
                    }
                    rows.push(row_vals);
                    if let Some(formulas) = formula_rows.as_mut()
                        && let Some(row) = row_formulas
                    {
                        formulas.push(row);
                    }
                }

                let mut row_limit = cap_rows_by_cells(rows.len(), total_cols, max_cells);
                if row_limit > 0 {
                    row_limit = cap_rows_by_payload_bytes(row_limit, max_payload_bytes, |count| {
                        let entry = build_range_values_entry(
                            format,
                            range,
                            &rows[..count],
                            formula_rows.as_ref().map(|matrix| &matrix[..count]),
                            None,
                        );
                        serde_json::to_vec(&entry)
                            .map(|payload| payload.len())
                            .unwrap_or(usize::MAX)
                    });
                }

                if row_limit < rows.len() {
                    rows.truncate(row_limit);
                    if let Some(formulas) = formula_rows.as_mut() {
                        formulas.truncate(row_limit);
                    }
                }

                let next_start_row = if rows.len() < total_rows {
                    Some(start_row + rows.len() as u32)
                } else {
                    None
                };

                build_range_values_entry(
                    format,
                    range,
                    &rows,
                    formula_rows.as_deref(),
                    next_start_row,
                )
            })
            .collect();

        Ok::<_, anyhow::Error>((values, has_formula_in_target))
    })??;

    #[cfg(not(feature = "recalc"))]
    let values = workbook.with_sheet(&params.sheet_name, |sheet| {
        let values = params
            .ranges
            .iter()
            .filter_map(|range| {
                parse_range(range).map(|((start_col, start_row), (end_col, end_row))| {
                    let total_rows = (end_row - start_row + 1) as usize;
                    let total_cols = (end_col - start_col + 1) as usize;
                    let mut row_limit = total_rows;
                    if let Some(page_size) = params.page_size {
                        row_limit = row_limit.min(page_size as usize);
                    }

                    let include_formula_matrix = include_formulas
                        && matches!(format, TableOutputFormat::Json | TableOutputFormat::Dense);
                    let mut rows = Vec::new();
                    let mut formula_rows = include_formula_matrix.then(Vec::new);
                    for r in start_row..=end_row {
                        if rows.len() >= row_limit {
                            break;
                        }
                        let mut row_vals = Vec::new();
                        let mut row_formulas = include_formula_matrix.then(Vec::new);
                        for c in start_col..=end_col {
                            let row_index = if include_headers && r == start_row && start_row == 1 {
                                1u32
                            } else {
                                r
                            };
                            let cell = sheet.get_cell((c, row_index));
                            row_vals.push(cell.and_then(cell_to_value));
                            if let Some(formulas) = row_formulas.as_mut() {
                                formulas.push(cell.and_then(|entry| {
                                    entry.is_formula().then(|| entry.get_formula().to_string())
                                }));
                            }
                        }
                        rows.push(row_vals);
                        if let Some(formulas) = formula_rows.as_mut()
                            && let Some(row) = row_formulas
                        {
                            formulas.push(row);
                        }
                    }

                    let mut row_limit = cap_rows_by_cells(rows.len(), total_cols, max_cells);
                    if row_limit > 0 {
                        row_limit =
                            cap_rows_by_payload_bytes(row_limit, max_payload_bytes, |count| {
                                let entry = build_range_values_entry(
                                    format,
                                    range,
                                    &rows[..count],
                                    formula_rows.as_ref().map(|matrix| &matrix[..count]),
                                    None,
                                );
                                serde_json::to_vec(&entry)
                                    .map(|payload| payload.len())
                                    .unwrap_or(usize::MAX)
                            });
                    }

                    if row_limit < rows.len() {
                        rows.truncate(row_limit);
                        if let Some(formulas) = formula_rows.as_mut() {
                            formulas.truncate(row_limit);
                        }
                    }

                    let next_start_row = if rows.len() < total_rows {
                        Some(start_row + rows.len() as u32)
                    } else {
                        None
                    };

                    build_range_values_entry(
                        format,
                        range,
                        &rows,
                        formula_rows.as_deref(),
                        next_start_row,
                    )
                })
            })
            .collect();

        Ok::<_, anyhow::Error>(values)
    })??;

    #[cfg(feature = "recalc")]
    let warnings: Vec<Warning> = {
        if fork_recalc_needed(&state, &params.workbook_or_fork_id) && has_formula_in_target {
            vec![Warning {
                code: "WARN_STALE_FORMULAS".to_string(),
                message: "Fork has pending edits and may contain stale formula results; call recalculate on the fork for fresh values.".to_string(),
            }]
        } else {
            Vec::new()
        }
    };

    #[cfg(not(feature = "recalc"))]
    let warnings: Vec<Warning> = Vec::new();

    Ok(RangeValuesResponse {
        workbook_id: workbook.id.clone(),
        calculation,
        sheet_name: params.sheet_name,
        warnings,
        values,
    })
}

pub async fn inspect_cells(
    state: impl crate::read_context::ReadContext,
    params: InspectCellsParams,
) -> Result<InspectCellsResponse> {
    inspect_cells_semantic(state, params).await
}

pub(crate) async fn inspect_cells_semantic(
    state: impl crate::read_context::ReadContext,
    params: InspectCellsParams,
) -> Result<InspectCellsResponse> {
    const DETAIL_LIMIT: usize = 25;
    const DETAIL_LIMIT_MAX: usize = 200;

    if params.targets.is_empty() {
        return Err(anyhow!(
            "inspect-cells requires at least one A1 target (cell or range)"
        ));
    }

    if let Some(b) = params.budget
        && (b < 1 || b as usize > DETAIL_LIMIT_MAX)
    {
        return Err(anyhow!(
            "budget must be between 1 and {} (got {})",
            DETAIL_LIMIT_MAX,
            b
        ));
    }

    let effective_cap = params
        .budget
        .map(|b| (b as usize).min(DETAIL_LIMIT_MAX))
        .unwrap_or(DETAIL_LIMIT);

    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let calculation = workbook.calculation_metadata();
    let config = state.config();
    let detail_limit = config
        .max_cells()
        .map(|limit| limit.min(effective_cap))
        .unwrap_or(effective_cap)
        .max(1);
    let max_payload_bytes = config.max_payload_bytes();
    let include_empty = params.include_empty.unwrap_or(false);

    let mut coords = Vec::new();
    let mut seen = HashSet::new();
    for target in &params.targets {
        let ((start_col, start_row), (end_col, end_row)) =
            parse_range(target).ok_or_else(|| {
                anyhow!(
                    "invalid target '{}'; expected A1 cell or range like A1:C10",
                    target
                )
            })?;

        for row in start_row..=end_row {
            for col in start_col..=end_col {
                if seen.insert((col, row)) {
                    coords.push((col, row));
                }
            }
        }
    }

    if coords.len() > detail_limit {
        return Err(anyhow!(
            "inspect-cells is a detail view and accepts up to {} cells per request (got {}). Narrow your selection or use sheet-page, range-values, or layout-page for broader discovery.",
            detail_limit,
            coords.len()
        ));
    }

    let mut cells = workbook.with_sheet(&params.sheet_name, |sheet| {
        let mut out = Vec::new();
        for (col, row) in &coords {
            if let Some(cell) = sheet.get_cell((*col, *row)) {
                out.push(build_cell_snapshot(cell, true, true));
            } else if include_empty {
                out.push(CellSnapshot {
                    address: format!("{}{}", column_number_to_name(*col), row),
                    value: None,
                    formula: None,
                    cached_value: None,
                    number_format: None,
                    style_tags: Vec::new(),
                    notes: Vec::new(),
                });
            }
        }
        Ok::<_, anyhow::Error>(out)
    })??;

    let total_requested_rows = coords
        .iter()
        .map(|(_, row)| *row)
        .collect::<HashSet<u32>>()
        .len();
    let mut cell_rows: Vec<u32> = cells
        .iter()
        .filter_map(|cell| parse_address(&cell.address).map(|(_, row)| row))
        .collect();
    let mut truncated = false;
    let cell_limit = cap_rows_by_payload_bytes(cells.len(), max_payload_bytes, |count| {
        let response = InspectCellsResponse {
            workbook_id: workbook.id.clone(),
            calculation: calculation.clone(),
            sheet_name: params.sheet_name.clone(),
            range: params.targets.join(","),
            targets: if params.targets.len() > 1 {
                params.targets.clone()
            } else {
                Vec::new()
            },
            cells: cells[..count].to_vec(),
            truncated: false,
            budget: None,
        };
        serde_json::to_vec(&response)
            .map(|payload| payload.len())
            .unwrap_or(usize::MAX)
    });
    if cell_limit < cells.len() {
        cells.truncate(cell_limit);
        cell_rows.truncate(cell_limit);
        truncated = true;
    }

    let cells_returned = cells.len();
    let rows_returned = cell_rows.into_iter().collect::<HashSet<u32>>().len();
    let budget = Some(ReadBudget {
        max_cells: Some(detail_limit),
        max_payload_bytes,
        rows_returned,
        cells_returned,
        total_rows_available: Some(total_requested_rows as u32),
        continuation: if truncated {
            Some(
                "inspect-cells is a strict detail-view tool; narrow your targets or use sheet-page / range-values for bulk reads"
                    .to_string(),
            )
        } else {
            None
        },
    });

    Ok(InspectCellsResponse {
        workbook_id: workbook.id.clone(),
        calculation,
        sheet_name: params.sheet_name,
        range: params.targets.join(","),
        targets: if params.targets.len() > 1 {
            params.targets
        } else {
            Vec::new()
        },
        cells,
        truncated,
        budget,
    })
}

pub async fn find_value(
    state: Arc<AppState>,
    params: FindValueParams,
) -> Result<FindValueResponse> {
    let request = crate::operations::SearchValuesRequest {
        resource_id: crate::operations::ResourceId::bind_workbook(&params.workbook_or_fork_id)
            .map_err(anyhow::Error::msg)?,
        query: params.query,
        label: params.label,
        mode: params.mode,
        match_mode: params.match_mode,
        case_sensitive: params.case_sensitive,
        sheet_name: params.sheet_name,
        region_id: params.region_id,
        table_name: params.table_name,
        value_types: params.value_types,
        search_headers_only: params.search_headers_only,
        direction: params.direction,
        limit: params.limit,
        offset: params.offset,
        context: params.context,
        context_width: params.context_width,
    };
    crate::operations::project_legacy(
        state,
        crate::operations::SpreadsheetOperation::SearchValues(request),
    )
    .await
}

pub(crate) async fn find_value_semantic(
    state: impl crate::read_context::ReadContext,
    params: FindValueParams,
) -> Result<FindValueResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let mut matches = Vec::new();
    let mut truncated = false;
    let mut seen: u32 = 0;
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit;
    let mode = params.mode.clone().unwrap_or_else(|| {
        if params.label.is_some() {
            FindMode::Label
        } else {
            FindMode::Value
        }
    });
    let match_mode = params.match_mode.unwrap_or_default();
    let direction = params.direction.clone().unwrap_or(LabelDirection::Any);

    let table_target = if let Some(table_name) = params.table_name.as_deref() {
        let item = workbook
            .named_items()?
            .into_iter()
            .find(|item| {
                item.kind == NamedItemKind::Table && item.name.eq_ignore_ascii_case(table_name)
            })
            .ok_or_else(|| anyhow!("table {table_name} not found"))?;
        let sheet_name = item
            .sheet_name
            .ok_or_else(|| anyhow!("table {table_name} has no owning sheet"))?;
        if let Some(requested_sheet) = params.sheet_name.as_deref()
            && requested_sheet != sheet_name
        {
            bail!("table {table_name} is on sheet {sheet_name}, not {requested_sheet}");
        }
        let range = parse_range(item.refers_to.trim_start_matches('='))
            .ok_or_else(|| anyhow!("table {table_name} has invalid bounds {}", item.refers_to))?;
        Some((sheet_name, range))
    } else {
        None
    };

    let target_sheets: Vec<String> = if let Some((sheet, _)) = &table_target {
        vec![sheet.clone()]
    } else if let Some(sheet) = &params.sheet_name {
        vec![sheet.clone()]
    } else {
        workbook.sheet_names()
    };

    for sheet_name in target_sheets {
        let metrics_entry = workbook.get_sheet_metrics_fast(&sheet_name)?;
        let default_bounds = (
            (1, 1),
            (
                metrics_entry.metrics.column_count.max(1),
                metrics_entry.metrics.row_count.max(1),
            ),
        );
        let region_bounds = params
            .region_id
            .and_then(|id| workbook.detected_region(&sheet_name, id).ok());
        let (sheet_matches, sheet_seen, sheet_truncated) =
            workbook.with_sheet(&sheet_name, |sheet| {
                collect_value_matches(
                    sheet,
                    &sheet_name,
                    &mode,
                    match_mode,
                    &direction,
                    &params,
                    region_bounds.as_ref(),
                    table_target.as_ref().map(|(_, bounds)| *bounds),
                    default_bounds,
                    offset,
                    limit,
                    seen,
                )
            })??;
        seen = sheet_seen;
        matches.extend(sheet_matches);
        if sheet_truncated {
            truncated = true;
            break;
        }
    }

    let next_offset = if truncated {
        Some(offset.saturating_add(matches.len() as u32))
    } else {
        None
    };

    Ok(FindValueResponse {
        workbook_id: workbook.id.clone(),
        match_count: matches.len() as u32,
        matches,
        next_offset,
    })
}

pub async fn read_table(
    state: Arc<AppState>,
    params: ReadTableParams,
) -> Result<ReadTableResponse> {
    let request = crate::operations::ReadTableRequest::try_from(params)
        .map_err(|error| anyhow::anyhow!(error))?;
    crate::operations::project_legacy(
        state,
        crate::operations::SpreadsheetOperation::ReadTable(request),
    )
    .await
}

pub(crate) async fn read_table_semantic(
    state: impl crate::read_context::ReadContext,
    params: ReadTableParams,
) -> Result<ReadTableResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let calculation = workbook.calculation_metadata();
    let config = state.config();
    let output_profile = config.output_profile();
    let format = params.format.unwrap_or(match output_profile {
        OutputProfile::TokenDense => TableOutputFormat::Csv,
        OutputProfile::Verbose => TableOutputFormat::Json,
    });
    let include_headers = params.include_headers.unwrap_or(true);
    let include_types = params.include_types.unwrap_or(false);
    let resolved = resolve_table_target(&workbook, &params)?;
    let limit = params.limit.unwrap_or(100) as usize;
    let offset = params.offset.unwrap_or(0) as usize;
    let sample_mode = params.sample_mode.unwrap_or_default();

    #[cfg(feature = "recalc")]
    let (headers, rows, total_rows, has_formula_in_target) =
        workbook.with_sheet(&resolved.sheet_name, |sheet| {
            let has_formula_in_target = sheet_has_formula_in_bounds(sheet, &[resolved.range]);
            let (headers, rows, total_rows) = extract_table_rows(
                sheet,
                &resolved,
                params.header_row,
                params.header_rows,
                params.columns.clone(),
                params.filters.clone(),
                limit,
                offset,
                sample_mode,
            )?;
            Ok::<_, anyhow::Error>((headers, rows, total_rows, has_formula_in_target))
        })??;

    #[cfg(not(feature = "recalc"))]
    let (headers, rows, total_rows) = workbook.with_sheet(&resolved.sheet_name, |sheet| {
        let (headers, rows, total_rows) = extract_table_rows(
            sheet,
            &resolved,
            params.header_row,
            params.header_rows,
            params.columns.clone(),
            params.filters.clone(),
            limit,
            offset,
            sample_mode,
        )?;
        Ok::<_, anyhow::Error>((headers, rows, total_rows))
    })??;

    #[cfg(feature = "recalc")]
    let warnings: Vec<Warning> = {
        if fork_recalc_needed(&state, &params.workbook_or_fork_id) && has_formula_in_target {
            vec![Warning {
                code: "WARN_STALE_FORMULAS".to_string(),
                message: "Fork has pending edits and may contain stale formula results; call recalculate on the fork for fresh values.".to_string(),
            }]
        } else {
            Vec::new()
        }
    };

    #[cfg(not(feature = "recalc"))]
    let warnings: Vec<Warning> = Vec::new();

    let max_cells = config.max_cells();
    let max_payload_bytes = config.max_payload_bytes();
    let mut row_limit = cap_rows_by_cells(rows.len(), headers.len().max(1), max_cells);
    if row_limit > 0 {
        row_limit = cap_rows_by_payload_bytes(row_limit, max_payload_bytes, |count| {
            let (headers_out, rows_out, values_out, types_out, csv_out) = build_read_table_payload(
                format,
                &headers,
                &rows[..count],
                include_headers,
                include_types,
            );
            let response = ReadTableResponse {
                workbook_id: workbook.id.clone(),
                calculation: calculation.clone(),
                sheet_name: resolved.sheet_name.clone(),
                table_name: resolved.table_name.clone(),
                warnings: warnings.clone(),
                headers: headers_out,
                rows: rows_out,
                values: values_out,
                types: types_out,
                csv: csv_out,
                total_rows,
                next_offset: None,
            };
            serde_json::to_vec(&response)
                .map(|payload| payload.len())
                .unwrap_or(usize::MAX)
        });
    }

    let rows = rows.into_iter().take(row_limit).collect::<Vec<_>>();
    let next_offset = if offset + rows.len() < total_rows as usize {
        Some((offset + rows.len()) as u32)
    } else {
        None
    };
    let (headers_out, rows_out, values_out, types_out, csv_out) =
        build_read_table_payload(format, &headers, &rows, include_headers, include_types);

    Ok(ReadTableResponse {
        workbook_id: workbook.id.clone(),
        calculation,
        sheet_name: resolved.sheet_name,
        table_name: resolved.table_name,
        warnings,
        headers: headers_out,
        rows: rows_out,
        values: values_out,
        types: types_out,
        csv: csv_out,
        total_rows,
        next_offset,
    })
}

pub async fn table_profile(
    state: Arc<AppState>,
    params: TableProfileParams,
) -> Result<TableProfileResponse> {
    let request = crate::operations::ProfileTableRequest {
        resource_id: crate::operations::ResourceId::bind_workbook(&params.workbook_or_fork_id)
            .map_err(anyhow::Error::msg)?,
        sheet_name: params.sheet_name,
        region_id: params.region_id,
        table_name: params.table_name,
        range: params.range,
        sample_mode: params.sample_mode,
        sample_size: params.sample_size,
        summary_only: params.summary_only,
    };
    crate::operations::project_legacy(
        state,
        crate::operations::SpreadsheetOperation::ProfileTable(request),
    )
    .await
}

pub(crate) struct ResolvedTableProfile {
    pub response: TableProfileResponse,
    pub bounds: String,
    pub header_row: u32,
    pub header_from_selector: bool,
    pub rows_scanned: u32,
}

pub(crate) async fn table_profile_semantic(
    state: impl crate::read_context::ReadContext,
    params: TableProfileParams,
) -> Result<ResolvedTableProfile> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let config = state.config();
    let output_profile = config.output_profile();
    let summary_only = params
        .summary_only
        .unwrap_or(matches!(output_profile, OutputProfile::TokenDense));
    let resolved = resolve_table_target(
        &workbook,
        &ReadTableParams {
            workbook_or_fork_id: params.workbook_or_fork_id.clone(),
            sheet_name: params.sheet_name.clone(),
            table_name: params.table_name.clone(),
            region_id: params.region_id,
            range: params.range.clone(),
            header_row: None,
            header_rows: None,
            columns: None,
            filters: None,
            sample_mode: params.sample_mode,
            limit: params.sample_size,
            offset: Some(0),
            format: Some(TableOutputFormat::Json),
            include_headers: None,
            include_types: None,
        },
    )?;

    let ((start_col, start_row), (end_col, end_row)) = resolved.range;
    let header_row = resolved
        .header_hint
        .unwrap_or(start_row)
        .clamp(start_row, end_row);
    let header_from_selector = resolved.header_hint.is_some();
    let bounds = format!(
        "{}{}:{}{}",
        crate::utils::column_number_to_name(start_col),
        start_row,
        crate::utils::column_number_to_name(end_col),
        end_row
    );
    let sample_size = params.sample_size.unwrap_or(10) as usize;
    let sample_mode = params.sample_mode.unwrap_or(SampleMode::Distributed);

    let (mut headers, rows, total_rows) =
        workbook.with_sheet(&resolved.sheet_name, |sheet| {
            extract_table_rows(
                sheet,
                &resolved,
                None,
                None,
                None,
                None,
                sample_size,
                0,
                sample_mode,
            )
        })??;

    let max_items = config.max_items();
    let max_payload_bytes = config.max_payload_bytes();

    if let Some(max_items) = max_items
        && headers.len() > max_items
    {
        headers.truncate(max_items);
    }

    let rows_scanned = rows.len() as u32;
    let mut column_types = summarize_columns(&headers, &rows);

    let mut samples: Vec<TableRow> = if summary_only {
        Vec::new()
    } else {
        rows.into_iter()
            .map(|row| filter_table_row(&row, &headers))
            .collect()
    };

    if !summary_only {
        if let Some(max_items) = max_items
            && samples.len() > max_items
        {
            samples.truncate(max_items);
        }

        if let Some(max_bytes) = max_payload_bytes {
            let sample_limit = cap_rows_by_payload_bytes(samples.len(), Some(max_bytes), |count| {
                let response = TableProfileResponse {
                    workbook_id: workbook.id.clone(),
                    sheet_name: resolved.sheet_name.clone(),
                    table_name: resolved.table_name.clone(),
                    headers: headers.clone(),
                    column_types: column_types.clone(),
                    row_count: total_rows,
                    samples: samples[..count].to_vec(),
                    notes: Vec::new(),
                };
                serde_json::to_vec(&response)
                    .map(|payload| payload.len())
                    .unwrap_or(usize::MAX)
            });
            if sample_limit < samples.len() {
                samples.truncate(sample_limit);
            }

            let response = TableProfileResponse {
                workbook_id: workbook.id.clone(),
                sheet_name: resolved.sheet_name.clone(),
                table_name: resolved.table_name.clone(),
                headers: headers.clone(),
                column_types: column_types.clone(),
                row_count: total_rows,
                samples: samples.clone(),
                notes: Vec::new(),
            };
            if serde_json::to_vec(&response)
                .map(|payload| payload.len() > max_bytes)
                .unwrap_or(false)
                && !headers.is_empty()
            {
                let header_limit =
                    cap_rows_by_payload_bytes(headers.len(), Some(max_bytes), |count| {
                        let headers_slice = headers[..count].to_vec();
                        let column_slice = column_types[..count.min(column_types.len())].to_vec();
                        let samples_slice = samples
                            .iter()
                            .map(|row| filter_table_row(row, &headers_slice))
                            .collect::<Vec<_>>();
                        let response = TableProfileResponse {
                            workbook_id: workbook.id.clone(),
                            sheet_name: resolved.sheet_name.clone(),
                            table_name: resolved.table_name.clone(),
                            headers: headers_slice,
                            column_types: column_slice,
                            row_count: total_rows,
                            samples: samples_slice,
                            notes: Vec::new(),
                        };
                        serde_json::to_vec(&response)
                            .map(|payload| payload.len())
                            .unwrap_or(usize::MAX)
                    });

                if header_limit < headers.len() {
                    headers.truncate(header_limit);
                    column_types.truncate(header_limit.min(column_types.len()));
                    samples = samples
                        .into_iter()
                        .map(|row| filter_table_row(&row, &headers))
                        .collect();
                }
            }
        }
    }

    Ok(ResolvedTableProfile {
        response: TableProfileResponse {
            workbook_id: workbook.id.clone(),
            sheet_name: resolved.sheet_name,
            table_name: resolved.table_name,
            headers,
            column_types,
            row_count: total_rows,
            samples,
            notes: Vec::new(),
        },
        bounds,
        header_row,
        header_from_selector,
        rows_scanned,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ManifestStubParams {
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    pub sheet_filter: Option<String>,
}

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
pub async fn get_manifest_stub(
    state: impl crate::read_context::ReadContext,
    params: ManifestStubParams,
) -> Result<ManifestStubResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let mut summaries = workbook.list_summaries(true)?;

    if let Some(filter) = &params.sheet_filter {
        summaries.retain(|summary| summary.name.eq_ignore_ascii_case(filter));
    }

    let mut ports = Vec::new();
    let sanitize_id_re = regex::Regex::new(r"[^a-z0-9_-]").expect("valid id regex");
    let sanitize_header_re = regex::Regex::new(r"[^a-zA-Z0-9_]").expect("valid header regex");

    for summary in &summaries {
        let sheet_name = &summary.name;
        if let Ok(overview) = workbook.sheet_overview(sheet_name) {
            for (idx, region) in overview.detected_regions.iter().enumerate() {
                let classification_str = format!("{:?}", region.classification).to_lowercase();
                let id = format!(
                    "{}_{}_{}",
                    sheet_name.replace(" ", "_"),
                    classification_str,
                    idx
                )
                .to_lowercase()
                .replace(char::is_whitespace, "_");
                let id = sanitize_id_re.replace_all(&id, "").to_string();

                let (dir, shape, schema, location) = match region.classification {
                    crate::model::RegionKind::Parameters => {
                        let shape = formualizer::sheetport_spec::Shape::Range;
                        let schema = formualizer::sheetport_spec::Schema::Range(
                            formualizer::sheetport_spec::RangeSchema {
                                kind: formualizer::sheetport_spec::RangeKind::Range,
                                cell_type: formualizer::sheetport_spec::ValueType::Number,
                                format: None,
                            },
                        );
                        let a1 = region.bounds.clone();
                        let location = formualizer::sheetport_spec::Selector::A1(
                            formualizer::sheetport_spec::SelectorA1 {
                                a1: format!("'{}'!{}", sheet_name, a1),
                            },
                        );
                        (
                            formualizer::sheetport_spec::Direction::In,
                            shape,
                            schema,
                            location,
                        )
                    }
                    crate::model::RegionKind::Data => {
                        let shape = formualizer::sheetport_spec::Shape::Table;
                        let anchor_col = region
                            .bounds
                            .chars()
                            .take_while(|c| c.is_alphabetic())
                            .collect::<String>();
                        let anchor_col = if anchor_col.is_empty() {
                            "A".to_string()
                        } else {
                            anchor_col
                        };
                        let layout = formualizer::sheetport_spec::LayoutDescriptor {
                            kind: formualizer::sheetport_spec::LayoutKind::HeaderContiguousV1,
                            sheet: sheet_name.clone(),
                            header_row: region.header_row.unwrap_or(1),
                            anchor_col,
                            terminate:
                                formualizer::sheetport_spec::LayoutTermination::FirstBlankRow,
                            marker_text: None,
                        };
                        let mut columns = Vec::new();
                        for (c_idx, header) in region.headers.iter().enumerate() {
                            let clean_name = sanitize_header_re
                                .replace_all(header, "_")
                                .to_string()
                                .to_lowercase();
                            let clean_name = if clean_name.is_empty() {
                                format!("col_{}", c_idx)
                            } else {
                                clean_name
                            };
                            columns.push(formualizer::sheetport_spec::TableColumn {
                                name: clean_name,
                                value_type: formualizer::sheetport_spec::ValueType::String,
                                col: None,
                                format: None,
                                units: None,
                            });
                        }
                        let schema = formualizer::sheetport_spec::Schema::Table(
                            formualizer::sheetport_spec::TableSchema {
                                kind: formualizer::sheetport_spec::TableKind::Table,
                                columns,
                                keys: None,
                            },
                        );
                        (
                            formualizer::sheetport_spec::Direction::In,
                            shape,
                            schema,
                            formualizer::sheetport_spec::Selector::Layout(
                                formualizer::sheetport_spec::SelectorLayout { layout },
                            ),
                        )
                    }
                    crate::model::RegionKind::Calculator | crate::model::RegionKind::Outputs => {
                        let shape = formualizer::sheetport_spec::Shape::Range;
                        let schema = formualizer::sheetport_spec::Schema::Range(
                            formualizer::sheetport_spec::RangeSchema {
                                kind: formualizer::sheetport_spec::RangeKind::Range,
                                cell_type: formualizer::sheetport_spec::ValueType::Number,
                                format: None,
                            },
                        );
                        let a1 = region.bounds.clone();
                        let location = formualizer::sheetport_spec::Selector::A1(
                            formualizer::sheetport_spec::SelectorA1 {
                                a1: format!("'{}'!{}", sheet_name, a1),
                            },
                        );
                        (
                            formualizer::sheetport_spec::Direction::Out,
                            shape,
                            schema,
                            location,
                        )
                    }
                    _ => continue,
                };

                ports.push(formualizer::sheetport_spec::Port {
                    id,
                    dir,
                    shape,
                    description: None,
                    required: true,
                    location,
                    schema,
                    constraints: None,
                    units: None,
                    default: None,
                    partition_key: None,
                });
            }
        }
    }

    let manifest_obj = formualizer::sheetport_spec::Manifest {
        spec: "fio".to_string(),
        spec_version: formualizer::sheetport_spec::SpecVersion("0.3.0".parse().unwrap()),
        capabilities: Some(formualizer::sheetport_spec::Capabilities {
            profile: formualizer::sheetport_spec::Profile::CoreV0,
            features: None,
        }),
        manifest: formualizer::sheetport_spec::ManifestMeta {
            id: workbook
                .slug
                .clone()
                .to_lowercase()
                .replace(char::is_whitespace, "-")
                .replace(|c: char| !c.is_alphanumeric() && c != '-', ""),
            name: workbook.slug.clone(),
            description: Some("Auto-generated manifest stub".to_string()),
            tags: None,
            workbook: Some(formualizer::sheetport_spec::WorkbookMeta {
                uri: Some(format!("file://{}", workbook.slug)),
                locale: None,
                date_system: None,
                timezone: None,
            }),
            metadata: None,
        },
        ports,
    };

    let manifest_yaml = manifest_obj.to_yaml().unwrap_or_else(|_| "".to_string());

    let sheets = summaries
        .into_iter()
        .map(|summary| ManifestSheetStub {
            sheet_name: summary.name.clone(),
            classification: summary.classification.clone(),
            candidate_expectations: vec![format!(
                "Review {} sheet for expectation candidates",
                format!("{:?}", summary.classification).to_ascii_lowercase()
            )],
            notes: summary.style_tags,
        })
        .collect();

    let response = ManifestStubResponse {
        workbook_id: workbook.id.clone(),
        slug: workbook.slug.clone(),
        manifest_yaml,
        sheets,
    };
    Ok(response)
}

#[cfg(not(all(feature = "recalc-formualizer", feature = "sheetport")))]
pub async fn get_manifest_stub(
    _state: impl crate::read_context::ReadContext,
    _params: ManifestStubParams,
) -> Result<ManifestStubResponse> {
    Err(anyhow!(
        "sheetport operations require the 'recalc-formualizer' feature"
    ))
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CloseWorkbookParams {
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
}

pub async fn close_workbook(
    state: impl crate::read_context::ReadContext,
    params: CloseWorkbookParams,
) -> Result<CloseWorkbookResponse> {
    state.close_workbook(&params.workbook_or_fork_id)?;
    Ok(CloseWorkbookResponse {
        workbook_id: params.workbook_or_fork_id.clone(),
        message: format!("workbook {} evicted", params.workbook_or_fork_id.as_str()),
    })
}
#[allow(clippy::too_many_arguments)]
fn collect_formula_matches(
    sheet: &umya_spreadsheet::Worksheet,
    sheet_name: &str,
    query: &str,
    case_sensitive: bool,
    include_context: bool,
    context_rows: u32,
    context_cols: u32,
    offset: u32,
    limit: u32,
    seen_so_far: u32,
) -> (Vec<FindFormulaMatch>, u32, bool) {
    use crate::workbook::cell_to_value;

    let mut results = Vec::new();
    let mut seen = seen_so_far;

    for cell in sheet.get_cell_collection() {
        if !cell.is_formula() {
            continue;
        }
        let formula = cell.get_formula();
        let haystack = if case_sensitive {
            formula.to_string()
        } else {
            formula.to_ascii_lowercase()
        };
        if !haystack.contains(query) {
            continue;
        }

        if seen < offset {
            seen += 1;
            continue;
        }

        if results.len() as u32 >= limit {
            return (results, seen, true);
        }

        let coord = cell.get_coordinate();
        let column = *coord.get_col_num();
        let row = *coord.get_row_num();

        let context = if include_context {
            let col_start = column.saturating_sub(context_cols / 2).max(1);
            let col_end = column + context_cols / 2;
            let columns: Vec<u32> = (col_start..=col_end).collect();

            let mut context_rows_vec = Vec::new();

            if context_rows > 0 {
                let header_row = build_row_snapshot(sheet, 1, &columns, false, false);
                context_rows_vec.push(header_row);
            }

            let row_start = row.saturating_sub(context_rows / 2).max(1);
            let row_end = (row + context_rows / 2).min(sheet.get_highest_row());

            for ctx_row in row_start..=row_end {
                let ctx_row_snapshot = build_row_snapshot(sheet, ctx_row, &columns, true, false);
                context_rows_vec.push(ctx_row_snapshot);
            }

            context_rows_vec
        } else {
            Vec::new()
        };

        results.push(FindFormulaMatch {
            address: coord.get_coordinate(),
            sheet_name: sheet_name.to_string(),
            formula: formula.to_string(),
            cached_value: cell_to_value(cell),
            context,
        });

        seen += 1;
    }

    (results, seen, false)
}

#[derive(Clone)]
struct TraceFormulaInfo {
    fingerprint: String,
    formula: String,
}

#[derive(Clone)]
struct TraceEdgeRaw {
    from: String,
    to: String,
    neighbor: String,
}

#[derive(Clone)]
struct LayerLinks {
    depth: u32,
    edges: Vec<TraceEdgeRaw>,
    truncated_cells: Vec<String>,
}

#[derive(Clone)]
struct NeighborDetail {
    address: String,
    column: Option<u32>,
    row: Option<u32>,
    kind: TraceCellKind,
    value: Option<CellValue>,
    formula: Option<String>,
    fingerprint: Option<String>,
    external: bool,
}

fn build_formula_lookup(graph: &FormulaGraph) -> HashMap<String, TraceFormulaInfo> {
    let mut map = HashMap::new();
    for group in graph.groups() {
        for address in group.addresses.clone() {
            map.insert(
                address.to_ascii_uppercase(),
                TraceFormulaInfo {
                    fingerprint: group.fingerprint.clone(),
                    formula: group.formula.clone(),
                },
            );
        }
    }
    map
}

struct TraceConfig<'a> {
    direction: &'a TraceDirection,
    origin: &'a str,
    sheet_name: &'a str,
    depth_limit: u32,
    page_size: usize,
}

fn build_trace_layers(
    workbook: &WorkbookContext<impl crate::workbook::WorkbookReadSource>,
    graph: &FormulaGraph,
    formula_lookup: &HashMap<String, TraceFormulaInfo>,
    config: &TraceConfig<'_>,
    cursor: Option<TraceCursor>,
) -> Result<(Vec<TraceLayer>, Option<TraceCursor>, Vec<String>)> {
    let layer_links =
        collect_layer_links(graph, config.direction, config.origin, config.depth_limit);
    let mut layers = Vec::new();
    let mut next_cursor = None;
    let mut notes = Vec::new();
    let focus_depth = cursor.as_ref().map(|c| c.depth);

    for layer in layer_links {
        let produce_edges = focus_depth.is_none_or(|depth| depth == layer.depth);
        let offset = cursor
            .as_ref()
            .filter(|c| c.depth == layer.depth)
            .map(|c| c.offset)
            .unwrap_or(0);

        let mut node_set: HashSet<String> = HashSet::new();
        for edge in &layer.edges {
            node_set.insert(edge.neighbor.clone());
        }
        let mut nodes: Vec<String> = node_set.into_iter().collect();
        nodes.sort_by(|a, b| compare_addresses(a, b));

        let details = workbook.with_sheet(config.sheet_name, |sheet| {
            collect_neighbor_details(sheet, config.sheet_name, &nodes, formula_lookup)
        })?;
        let total_nodes = details.len();
        let start = offset.min(total_nodes);
        let end = if produce_edges {
            (start + config.page_size).min(total_nodes)
        } else {
            start
        };
        let selected_slice = if produce_edges {
            &details[start..end]
        } else {
            &details[0..0]
        };
        let selected_addresses: HashSet<String> = selected_slice
            .iter()
            .map(|detail| detail.address.clone())
            .collect();

        let summary = build_layer_summary(&details);
        let range_highlights = build_range_highlights(&details);
        let group_highlights = build_formula_group_highlights(&details);
        let notable_cells = build_notable_cells(&details, &range_highlights, &group_highlights);

        let highlights = TraceLayerHighlights {
            top_ranges: range_highlights.clone(),
            top_formula_groups: group_highlights.clone(),
            notable_cells,
        };

        let edges = if produce_edges {
            build_edges_for_layer(&layer.edges, &selected_addresses, formula_lookup)
        } else {
            Vec::new()
        };

        let has_more = produce_edges && end < total_nodes;
        if has_more && next_cursor.is_none() {
            next_cursor = Some(TraceCursor {
                depth: layer.depth,
                offset: end,
            });
        }
        if has_more {
            notes.push(format!(
                "Layer {} truncated at {} of {} nodes; supply cursor.depth={} and cursor.offset={} to continue",
                layer.depth, end, total_nodes, layer.depth, end
            ));
        }

        if !layer.truncated_cells.is_empty() {
            let cell_list = if layer.truncated_cells.len() <= 3 {
                layer.truncated_cells.join(", ")
            } else {
                format!(
                    "{}, ... ({} more)",
                    layer.truncated_cells[..3].join(", "),
                    layer.truncated_cells.len() - 3
                )
            };
            notes.push(format!(
                "Layer {}: dependents truncated at {} per cell for: {}",
                layer.depth, TRACE_DEPENDENTS_PER_CELL_LIMIT, cell_list
            ));
        }

        layers.push(TraceLayer {
            depth: layer.depth,
            summary,
            highlights,
            edges,
            has_more,
        });
    }

    Ok((layers, next_cursor, notes))
}

fn collect_layer_links(
    graph: &FormulaGraph,
    direction: &TraceDirection,
    origin: &str,
    depth_limit: u32,
) -> Vec<LayerLinks> {
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(origin.to_string());
    let mut frontier = vec![origin.to_string()];
    let mut layers = Vec::new();

    for depth in 1..=depth_limit {
        let mut next_frontier_set: HashSet<String> = HashSet::new();
        let mut edges = Vec::new();
        let mut truncated_cells = Vec::new();

        for cell in &frontier {
            let (neighbors, was_truncated) = match direction {
                TraceDirection::Precedents => (graph.precedents(cell), false),
                TraceDirection::Dependents => {
                    graph.dependents_limited(cell, Some(TRACE_DEPENDENTS_PER_CELL_LIMIT))
                }
            };

            if was_truncated {
                truncated_cells.push(cell.clone());
            }

            for neighbor in neighbors {
                let neighbor_upper = neighbor.to_ascii_uppercase();
                let edge = match direction {
                    TraceDirection::Precedents => TraceEdgeRaw {
                        from: cell.clone(),
                        to: neighbor_upper.clone(),
                        neighbor: neighbor_upper.clone(),
                    },
                    TraceDirection::Dependents => TraceEdgeRaw {
                        from: neighbor_upper.clone(),
                        to: cell.clone(),
                        neighbor: neighbor_upper.clone(),
                    },
                };
                edges.push(edge);
                if visited.insert(neighbor_upper.clone()) {
                    next_frontier_set.insert(neighbor_upper);
                }
            }
        }

        if edges.is_empty() {
            break;
        }

        layers.push(LayerLinks {
            depth,
            edges,
            truncated_cells,
        });
        if next_frontier_set.is_empty() {
            break;
        }
        let mut next_frontier: Vec<String> = next_frontier_set.into_iter().collect();
        next_frontier.sort();
        frontier = next_frontier;
    }

    layers
}

fn collect_neighbor_details(
    sheet: &umya_spreadsheet::Worksheet,
    current_sheet: &str,
    addresses: &[String],
    formula_lookup: &HashMap<String, TraceFormulaInfo>,
) -> Vec<NeighborDetail> {
    let mut details = Vec::new();
    for address in addresses {
        let (sheet_part, cell_part) = split_sheet_and_cell(address);
        let normalized_sheet = sheet_part
            .as_ref()
            .map(|s| clean_sheet_name(s).to_ascii_lowercase());
        let is_external = normalized_sheet
            .as_ref()
            .map(|s| !s.eq_ignore_ascii_case(current_sheet))
            .unwrap_or(false);

        let Some(cell_ref) = cell_part else {
            details.push(NeighborDetail {
                address: address.clone(),
                column: None,
                row: None,
                kind: TraceCellKind::External,
                value: None,
                formula: None,
                fingerprint: None,
                external: true,
            });
            continue;
        };

        let cell_ref_upper = cell_ref.to_ascii_uppercase();

        if is_external {
            let formula_info = lookup_formula_info(formula_lookup, &cell_ref_upper, address);
            details.push(NeighborDetail {
                address: address.clone(),
                column: None,
                row: None,
                kind: TraceCellKind::External,
                value: None,
                formula: formula_info.map(|info| info.formula.clone()),
                fingerprint: formula_info.map(|info| info.fingerprint.clone()),
                external: true,
            });
            continue;
        }

        let Some((col, row)) = parse_address(&cell_ref_upper) else {
            details.push(NeighborDetail {
                address: address.clone(),
                column: None,
                row: None,
                kind: TraceCellKind::External,
                value: None,
                formula: None,
                fingerprint: None,
                external: true,
            });
            continue;
        };

        let cell_opt = sheet.get_cell((&col, &row));
        let formula_info = lookup_formula_info(formula_lookup, &cell_ref_upper, address);
        if let Some(cell) = cell_opt {
            let value = cell_to_value(cell);
            let kind = if cell.is_formula() {
                TraceCellKind::Formula
            } else if value.is_some() {
                TraceCellKind::Literal
            } else {
                TraceCellKind::Blank
            };
            details.push(NeighborDetail {
                address: address.clone(),
                column: Some(col),
                row: Some(row),
                kind,
                value,
                formula: formula_info.map(|info| info.formula.clone()),
                fingerprint: formula_info.map(|info| info.fingerprint.clone()),
                external: false,
            });
        } else {
            details.push(NeighborDetail {
                address: address.clone(),
                column: Some(col),
                row: Some(row),
                kind: TraceCellKind::Blank,
                value: None,
                formula: formula_info.map(|info| info.formula.clone()),
                fingerprint: formula_info.map(|info| info.fingerprint.clone()),
                external: false,
            });
        }
    }
    details
}

fn build_layer_summary(details: &[NeighborDetail]) -> TraceLayerSummary {
    let mut summary = TraceLayerSummary {
        total_nodes: details.len(),
        formula_nodes: 0,
        value_nodes: 0,
        blank_nodes: 0,
        external_nodes: 0,
        unique_formula_groups: 0,
    };

    let mut fingerprints: HashSet<String> = HashSet::new();

    for detail in details {
        match detail.kind {
            TraceCellKind::Formula => {
                summary.formula_nodes += 1;
                if let Some(fp) = &detail.fingerprint {
                    fingerprints.insert(fp.clone());
                }
            }
            TraceCellKind::Literal => summary.value_nodes += 1,
            TraceCellKind::Blank => summary.blank_nodes += 1,
            TraceCellKind::External => summary.external_nodes += 1,
        }
    }

    summary.unique_formula_groups = fingerprints.len();
    summary
}

fn build_formula_group_highlights(details: &[NeighborDetail]) -> Vec<TraceFormulaGroupHighlight> {
    let mut aggregates: HashMap<String, (String, usize, Vec<String>)> = HashMap::new();
    for detail in details {
        if let (Some(fingerprint), Some(formula)) = (&detail.fingerprint, &detail.formula) {
            let entry = aggregates
                .entry(fingerprint.clone())
                .or_insert_with(|| (formula.clone(), 0, Vec::new()));
            entry.1 += 1;
            if entry.2.len() < TRACE_GROUP_SAMPLE_LIMIT {
                entry.2.push(detail.address.clone());
            }
        }
    }

    let mut highlights: Vec<TraceFormulaGroupHighlight> = aggregates
        .into_iter()
        .map(
            |(fingerprint, (formula, count, sample_addresses))| TraceFormulaGroupHighlight {
                fingerprint,
                formula,
                count,
                sample_addresses,
            },
        )
        .collect();

    highlights.sort_by(|a, b| b.count.cmp(&a.count));
    highlights.truncate(TRACE_GROUP_HIGHLIGHT_LIMIT);
    highlights
}

fn build_range_highlights(details: &[NeighborDetail]) -> Vec<TraceRangeHighlight> {
    let mut by_column: HashMap<u32, Vec<&NeighborDetail>> = HashMap::new();
    for detail in details {
        if let (Some(col), Some(_row)) = (detail.column, detail.row)
            && !detail.external
        {
            by_column.entry(col).or_default().push(detail);
        }
    }

    for column_entries in by_column.values_mut() {
        column_entries.sort_by(|a, b| a.row.cmp(&b.row));
    }

    let mut ranges = Vec::new();
    for entries in by_column.values() {
        let mut current: Vec<&NeighborDetail> = Vec::new();
        for detail in entries {
            if current.is_empty() {
                current.push(detail);
                continue;
            }
            let prev_row = current.last().and_then(|d| d.row).unwrap_or(0);
            if detail.row.unwrap_or(0) == prev_row + 1 {
                current.push(detail);
            } else {
                if current.len() >= TRACE_RANGE_THRESHOLD {
                    ranges.push(make_range_highlight(&current));
                }
                current.clear();
                current.push(detail);
            }
        }
        if current.len() >= TRACE_RANGE_THRESHOLD {
            ranges.push(make_range_highlight(&current));
        }
    }

    ranges.sort_by(|a, b| b.count.cmp(&a.count));
    ranges.truncate(TRACE_RANGE_HIGHLIGHT_LIMIT);
    ranges
}

fn make_range_highlight(details: &[&NeighborDetail]) -> TraceRangeHighlight {
    let mut literals = 0usize;
    let mut formulas = 0usize;
    let mut blanks = 0usize;
    let mut sample_values = Vec::new();
    let mut sample_formulas = Vec::new();
    let mut sample_addresses = Vec::new();

    for detail in details {
        match detail.kind {
            TraceCellKind::Formula => {
                formulas += 1;
                if let Some(formula) = &detail.formula
                    && sample_formulas.len() < TRACE_RANGE_FORMULA_SAMPLES
                    && !sample_formulas.contains(formula)
                {
                    sample_formulas.push(formula.clone());
                }
            }
            TraceCellKind::Literal => {
                literals += 1;
                if let Some(value) = &detail.value
                    && sample_values.len() < TRACE_RANGE_VALUE_SAMPLES
                {
                    sample_values.push(value.clone());
                }
            }
            TraceCellKind::Blank => blanks += 1,
            TraceCellKind::External => {}
        }
        if sample_addresses.len() < TRACE_RANGE_VALUE_SAMPLES {
            sample_addresses.push(detail.address.clone());
        }
    }

    TraceRangeHighlight {
        start: details
            .first()
            .map(|d| d.address.clone())
            .unwrap_or_default(),
        end: details
            .last()
            .map(|d| d.address.clone())
            .unwrap_or_default(),
        count: details.len(),
        literals,
        formulas,
        blanks,
        sample_values,
        sample_formulas,
        sample_addresses,
    }
}

fn build_notable_cells(
    details: &[NeighborDetail],
    ranges: &[TraceRangeHighlight],
    groups: &[TraceFormulaGroupHighlight],
) -> Vec<TraceCellHighlight> {
    let mut exclude: HashSet<String> = HashSet::new();
    for range in ranges {
        exclude.insert(range.start.clone());
        exclude.insert(range.end.clone());
        for addr in &range.sample_addresses {
            exclude.insert(addr.clone());
        }
    }
    for group in groups {
        for addr in &group.sample_addresses {
            exclude.insert(addr.clone());
        }
    }

    let mut highlights = Vec::new();
    let mut kind_counts: HashMap<TraceCellKind, usize> = HashMap::new();

    for detail in details {
        if highlights.len() >= TRACE_CELL_HIGHLIGHT_LIMIT {
            break;
        }
        if exclude.contains(&detail.address) {
            continue;
        }
        let counter = kind_counts.entry(detail.kind.clone()).or_insert(0);
        if *counter >= 2 && detail.kind != TraceCellKind::External {
            continue;
        }
        highlights.push(TraceCellHighlight {
            address: detail.address.clone(),
            kind: detail.kind.clone(),
            value: detail.value.clone(),
            formula: detail.formula.clone(),
        });
        *counter += 1;
    }

    highlights
}

fn build_edges_for_layer(
    raw_edges: &[TraceEdgeRaw],
    selected: &HashSet<String>,
    formula_lookup: &HashMap<String, TraceFormulaInfo>,
) -> Vec<FormulaTraceEdge> {
    let mut edges = Vec::new();
    for edge in raw_edges {
        if selected.contains(&edge.neighbor) {
            let formula = lookup_formula_info(formula_lookup, &edge.neighbor, &edge.neighbor)
                .map(|info| info.formula.clone());
            edges.push(FormulaTraceEdge {
                from: edge.from.clone(),
                to: edge.to.clone(),
                formula,
                note: None,
            });
        }
    }
    edges.sort_by(|a, b| compare_addresses(&a.to, &b.to));
    edges
}

fn lookup_formula_info<'a>(
    lookup: &'a HashMap<String, TraceFormulaInfo>,
    cell_ref: &str,
    original: &str,
) -> Option<&'a TraceFormulaInfo> {
    if let Some(info) = lookup.get(cell_ref) {
        return Some(info);
    }
    if let (Some(_sheet), Some(cell)) = split_sheet_and_cell(original) {
        let upper = cell.to_ascii_uppercase();
        return lookup.get(&upper);
    }
    None
}

fn compare_addresses(left: &str, right: &str) -> Ordering {
    let (sheet_left, cell_left) = split_sheet_and_cell(left);
    let (sheet_right, cell_right) = split_sheet_and_cell(right);

    let sheet_left_key = sheet_left
        .as_ref()
        .map(|s| clean_sheet_name(s).to_ascii_uppercase())
        .unwrap_or_default();
    let sheet_right_key = sheet_right
        .as_ref()
        .map(|s| clean_sheet_name(s).to_ascii_uppercase())
        .unwrap_or_default();

    match sheet_left_key.cmp(&sheet_right_key) {
        Ordering::Equal => {
            let left_core = cell_left.unwrap_or_else(|| left.to_string());
            let right_core = cell_right.unwrap_or_else(|| right.to_string());
            let left_coords = parse_address(&left_core.to_ascii_uppercase());
            let right_coords = parse_address(&right_core.to_ascii_uppercase());
            match (left_coords, right_coords) {
                (Some((lc, lr)), Some((rc, rr))) => lr
                    .cmp(&rr)
                    .then_with(|| lc.cmp(&rc))
                    .then_with(|| left_core.cmp(&right_core)),
                _ => left_core.cmp(&right_core),
            }
        }
        other => other,
    }
}

fn split_sheet_and_cell(address: &str) -> (Option<String>, Option<String>) {
    if let Some(idx) = address.rfind('!') {
        let sheet = address[..idx].to_string();
        let cell = address[idx + 1..].to_string();
        (Some(sheet), Some(cell))
    } else {
        (None, Some(address.to_string()))
    }
}

fn clean_sheet_name(sheet: &str) -> String {
    let trimmed = sheet.trim_matches(|c| c == '\'' || c == '"');
    let after_bracket = trimmed.rsplit(']').next().unwrap_or(trimmed);
    after_bracket
        .trim_matches(|c| c == '\'' || c == '"')
        .to_string()
}

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
fn json_to_literal(value: &serde_json::Value) -> formualizer::workbook::LiteralValue {
    match value {
        serde_json::Value::Null => formualizer::workbook::LiteralValue::Empty,
        serde_json::Value::Bool(b) => formualizer::workbook::LiteralValue::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                formualizer::workbook::LiteralValue::Number(f)
            } else {
                formualizer::workbook::LiteralValue::Empty
            }
        }
        serde_json::Value::String(s) => formualizer::workbook::LiteralValue::Text(s.clone()),
        _ => formualizer::workbook::LiteralValue::Empty,
    }
}

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
fn json_to_port_value(value: &serde_json::Value) -> formualizer::sheetport::PortValue {
    match value {
        serde_json::Value::Object(map) => {
            let mut record = std::collections::BTreeMap::new();
            for (k, v) in map {
                record.insert(k.clone(), json_to_literal(v));
            }
            formualizer::sheetport::PortValue::Record(record)
        }
        serde_json::Value::Array(arr) => {
            // Check if array of objects (table) or array of arrays (range)
            if let Some(serde_json::Value::Object(_)) = arr.first() {
                let mut rows = Vec::new();
                for row_val in arr {
                    if let serde_json::Value::Object(map) = row_val {
                        let mut values = std::collections::BTreeMap::new();
                        for (k, v) in map {
                            values.insert(k.clone(), json_to_literal(v));
                        }
                        rows.push(formualizer::sheetport::TableRow::new(values));
                    }
                }
                formualizer::sheetport::PortValue::Table(formualizer::sheetport::TableValue::new(
                    rows,
                ))
            } else if let Some(serde_json::Value::Array(_)) = arr.first() {
                let mut rows = Vec::new();
                for row_val in arr {
                    if let serde_json::Value::Array(inner) = row_val {
                        rows.push(inner.iter().map(json_to_literal).collect());
                    }
                }
                formualizer::sheetport::PortValue::Range(rows)
            } else {
                formualizer::sheetport::PortValue::Scalar(
                    formualizer::workbook::LiteralValue::Empty,
                )
            }
        }
        _ => formualizer::sheetport::PortValue::Scalar(json_to_literal(value)),
    }
}

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
fn port_value_to_json(value: &formualizer::sheetport::PortValue) -> serde_json::Value {
    match value {
        formualizer::sheetport::PortValue::Scalar(lit) => literal_to_json(lit),
        formualizer::sheetport::PortValue::Record(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                obj.insert(k.clone(), literal_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
        formualizer::sheetport::PortValue::Range(rows) => {
            let arr: Vec<serde_json::Value> = rows
                .iter()
                .map(|row| serde_json::Value::Array(row.iter().map(literal_to_json).collect()))
                .collect();
            serde_json::Value::Array(arr)
        }
        formualizer::sheetport::PortValue::Table(table) => {
            let arr: Vec<serde_json::Value> = table
                .rows
                .iter()
                .map(|row| {
                    let mut obj = serde_json::Map::new();
                    for (k, v) in &row.values {
                        obj.insert(k.clone(), literal_to_json(v));
                    }
                    serde_json::Value::Object(obj)
                })
                .collect();
            serde_json::Value::Array(arr)
        }
    }
}

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
fn literal_to_json(lit: &formualizer::workbook::LiteralValue) -> serde_json::Value {
    match lit {
        formualizer::workbook::LiteralValue::Empty => serde_json::Value::Null,
        formualizer::workbook::LiteralValue::Boolean(b) => serde_json::Value::Bool(*b),
        formualizer::workbook::LiteralValue::Number(n) => serde_json::json!(n),
        formualizer::workbook::LiteralValue::Int(i) => serde_json::json!(i),
        formualizer::workbook::LiteralValue::Text(t) => serde_json::Value::String(t.clone()),
        formualizer::workbook::LiteralValue::Error(e) => {
            serde_json::Value::String(format!("#ERROR: {:?}", e))
        }
        formualizer::workbook::LiteralValue::Date(d) => serde_json::Value::String(d.to_string()),
        formualizer::workbook::LiteralValue::DateTime(dt) => {
            serde_json::Value::String(dt.to_string())
        }
        formualizer::workbook::LiteralValue::Time(t) => serde_json::Value::String(t.to_string()),
        formualizer::workbook::LiteralValue::Duration(d) => {
            serde_json::Value::String(d.to_string())
        }
        formualizer::workbook::LiteralValue::Array(arr) => {
            let json_rows: Vec<serde_json::Value> = arr
                .iter()
                .map(|row| serde_json::Value::Array(row.iter().map(literal_to_json).collect()))
                .collect();
            serde_json::Value::Array(json_rows)
        }
        formualizer::workbook::LiteralValue::Pending => serde_json::Value::Null,
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ExecuteManifestParams {
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    pub manifest_yaml: String,
    #[serde(default)]
    pub inputs: std::collections::BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub rng_seed: Option<u64>,
    #[serde(default)]
    pub freeze_volatile: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ExecuteManifestResponse {
    pub workbook_id: WorkbookId,
    pub outputs: serde_json::Value,
}

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
pub async fn execute_manifest(
    state: impl crate::read_context::ReadContext,
    params: ExecuteManifestParams,
) -> Result<ExecuteManifestResponse> {
    use formualizer::workbook::SpreadsheetReader;
    let workbook_ctx = state.open_workbook(&params.workbook_or_fork_id).await?;
    let path = &workbook_ctx.path;

    let workbook_bytes = std::fs::read(path)?;
    let adapter = formualizer::workbook::UmyaAdapter::open_bytes(workbook_bytes)
        .or_else(|_| formualizer::workbook::UmyaAdapter::open_path(path))
        .map_err(|e| anyhow!("Failed to open adapter: {}", e))?;

    let workbook = formualizer::workbook::Workbook::from_reader(
        adapter,
        formualizer::workbook::LoadStrategy::EagerAll,
        formualizer::workbook::WorkbookConfig::ephemeral(),
    )
    .map_err(|e| anyhow!("Failed to load workbook: {}", e))?;

    let manifest = formualizer::sheetport_spec::Manifest::from_yaml_str(&params.manifest_yaml)
        .map_err(|e| anyhow!("Failed to parse manifest YAML: {}", e))?;

    let mut session = formualizer::sheetport::SheetPortSession::new(workbook, manifest)
        .map_err(|e| anyhow!("Failed to create SheetPort session: {}", e))?;

    let mut input_update = formualizer::sheetport::InputUpdate::new();
    for (key, val) in params.inputs {
        input_update.insert(key, json_to_port_value(&val));
    }

    if !input_update.is_empty() {
        session
            .write_inputs(input_update)
            .map_err(|e| anyhow!("Failed to write inputs: {}", e))?;
    }

    let options = formualizer::sheetport::EvalOptions {
        rng_seed: params.rng_seed,
        freeze_volatile: params.freeze_volatile,
        ..Default::default()
    };

    let outputs = session
        .evaluate_once(options)
        .map_err(|e| anyhow!("Failed to evaluate: {}", e))?;

    let mut out_map = serde_json::Map::new();
    for (k, v) in outputs.into_inner() {
        out_map.insert(k, port_value_to_json(&v));
    }

    Ok(ExecuteManifestResponse {
        workbook_id: params.workbook_or_fork_id.clone(),
        outputs: serde_json::Value::Object(out_map),
    })
}

#[cfg(not(all(feature = "recalc-formualizer", feature = "sheetport")))]
pub async fn execute_manifest(
    _state: impl crate::read_context::ReadContext,
    _params: ExecuteManifestParams,
) -> Result<ExecuteManifestResponse> {
    Err(anyhow!(
        "sheetport operations require the 'recalc-formualizer' feature"
    ))
}

// ── layout_page ───────────────────────────────────────────────────────────────

// ── grid_export ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GridExportParams {
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    pub sheet_name: String,
    pub range: String,
}

pub async fn grid_export(
    state: impl crate::read_context::ReadContext,
    params: GridExportParams,
) -> Result<crate::model::GridPayload> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;
    let ((min_col, min_row), (max_col, max_row)) =
        parse_range(&params.range).ok_or_else(|| anyhow!("invalid range: {}", params.range))?;

    let payload = workbook.with_sheet(&params.sheet_name, |sheet| {
        let mut columns = Vec::new();
        for col_idx in min_col..=max_col {
            if let Some(dim) = sheet.get_column_dimension_by_number(&col_idx) {
                let w = *dim.get_width();
                if w > 0.0 {
                    columns.push(crate::model::GridColumnHint {
                        offset: col_idx - min_col,
                        width_chars: w,
                    });
                }
            }
        }

        let mut merges = Vec::new();
        for mc in sheet.get_merge_cells() {
            let m_range = mc.get_range();
            if let Some(((c1, r1), (c2, r2))) = parse_range(&m_range)
                && c1 <= max_col
                && c2 >= min_col
                && r1 <= max_row
                && r2 >= min_row
            {
                merges.push(m_range.to_string());
            }
        }

        let mut rows = Vec::new();
        for row in min_row..=max_row {
            let mut cells = Vec::new();
            for col in min_col..=max_col {
                if let Some(cell) = sheet.get_cell((&col, &row)) {
                    let mut v = None;
                    let mut f = None;

                    if cell.is_formula() {
                        f = Some(format!("={}", cell.get_formula()));
                    } else {
                        let value = crate::workbook::cell_to_value(cell);
                        if let Some(cv) = value {
                            match cv {
                                crate::model::CellValue::Text(s) => {
                                    v = Some(serde_json::Value::String(s))
                                }
                                crate::model::CellValue::Number(n) => {
                                    v = Some(serde_json::json!(n))
                                }
                                crate::model::CellValue::Bool(b) => {
                                    v = Some(serde_json::Value::Bool(b))
                                }
                                crate::model::CellValue::Error(e) => {
                                    v = Some(serde_json::Value::String(e))
                                }
                                crate::model::CellValue::Date(d) => {
                                    v = Some(serde_json::Value::String(d))
                                }
                            }
                        }
                    }

                    let style = cell.get_style();
                    let desc = crate::styles::descriptor_from_style(style);

                    let fmt = desc.number_format.clone();

                    let mut style_patch = None;
                    if desc.font.is_some()
                        || desc.fill.is_some()
                        || desc.borders.is_some()
                        || desc.alignment.is_some()
                    {
                        style_patch = Some(crate::model::StylePatch {
                            font: desc.font.map(|f| {
                                Some(crate::model::FontPatch {
                                    name: f.name.map(Some),
                                    size: f.size.map(Some),
                                    bold: f.bold.map(Some),
                                    italic: f.italic.map(Some),
                                    underline: f.underline.map(Some),
                                    strikethrough: f.strikethrough.map(Some),
                                    color: f.color.map(Some),
                                })
                            }),
                            fill: desc.fill.map(|f| {
                                Some(match f {
                                    crate::model::FillDescriptor::Pattern(p) => {
                                        crate::model::FillPatch::Pattern(
                                            crate::model::PatternFillPatch {
                                                pattern_type: p.pattern_type.map(Some),
                                                foreground_color: p.foreground_color.map(Some),
                                                background_color: p.background_color.map(Some),
                                            },
                                        )
                                    }
                                    crate::model::FillDescriptor::Gradient(g) => {
                                        crate::model::FillPatch::Gradient(
                                            crate::model::GradientFillPatch {
                                                degree: g.degree.map(Some),
                                                stops: Some(
                                                    g.stops
                                                        .into_iter()
                                                        .map(|s| crate::model::GradientStopPatch {
                                                            position: s.position,
                                                            color: s.color,
                                                        })
                                                        .collect(),
                                                ),
                                            },
                                        )
                                    }
                                })
                            }),
                            borders: desc.borders.map(|b| {
                                Some(crate::model::BordersPatch {
                                    left: b.left.map(|s| {
                                        Some(crate::model::BorderSidePatch {
                                            style: s.style.map(Some),
                                            color: s.color.map(Some),
                                        })
                                    }),
                                    right: b.right.map(|s| {
                                        Some(crate::model::BorderSidePatch {
                                            style: s.style.map(Some),
                                            color: s.color.map(Some),
                                        })
                                    }),
                                    top: b.top.map(|s| {
                                        Some(crate::model::BorderSidePatch {
                                            style: s.style.map(Some),
                                            color: s.color.map(Some),
                                        })
                                    }),
                                    bottom: b.bottom.map(|s| {
                                        Some(crate::model::BorderSidePatch {
                                            style: s.style.map(Some),
                                            color: s.color.map(Some),
                                        })
                                    }),
                                    diagonal: b.diagonal.map(|s| {
                                        Some(crate::model::BorderSidePatch {
                                            style: s.style.map(Some),
                                            color: s.color.map(Some),
                                        })
                                    }),
                                    vertical: b.vertical.map(|s| {
                                        Some(crate::model::BorderSidePatch {
                                            style: s.style.map(Some),
                                            color: s.color.map(Some),
                                        })
                                    }),
                                    horizontal: b.horizontal.map(|s| {
                                        Some(crate::model::BorderSidePatch {
                                            style: s.style.map(Some),
                                            color: s.color.map(Some),
                                        })
                                    }),
                                    diagonal_up: b.diagonal_up.map(Some),
                                    diagonal_down: b.diagonal_down.map(Some),
                                })
                            }),
                            alignment: desc.alignment.map(|a| {
                                Some(crate::model::AlignmentPatch {
                                    horizontal: a.horizontal.map(Some),
                                    vertical: a.vertical.map(Some),
                                    wrap_text: a.wrap_text.map(Some),
                                    text_rotation: a.text_rotation.map(Some),
                                })
                            }),
                            number_format: None,
                        });
                    }

                    if v.is_some() || f.is_some() || fmt.is_some() || style_patch.is_some() {
                        cells.push(crate::model::GridCell {
                            offset: [row - min_row, col - min_col],
                            v,
                            f,
                            fmt,
                            style: style_patch,
                        });
                    }
                }
            }
            if !cells.is_empty() {
                rows.push(crate::model::GridRow { cells });
            }
        }

        let anchor = format!(
            "{}{}",
            crate::utils::column_number_to_name(min_col),
            min_row
        );

        Ok::<_, anyhow::Error>(crate::model::GridPayload {
            sheet: params.sheet_name.clone(),
            anchor,
            columns,
            merges,
            rows,
        })
    })??;

    Ok(payload)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LayoutPageParams {
    /// Workbook ID or fork ID
    #[serde(alias = "workbook_id")]
    pub workbook_or_fork_id: WorkbookId,
    /// Sheet name
    pub sheet_name: String,
    /// A1 range to render (e.g., "A1:F40"). Defaults to "A1:T50". Capped at 80 rows × 25 cols.
    #[serde(default)]
    pub range: Option<String>,
    /// Cell content mode: "values" (default) or "formulas"
    #[serde(default)]
    pub mode: Option<LayoutMode>,
    /// Maximum column width in character units before truncating content (default: 20)
    #[serde(default)]
    pub max_col_width: Option<u32>,
    /// Set column widths to the longest rendered value in each column (default: false)
    #[serde(default)]
    pub fit_columns: Option<bool>,
    /// Trim empty edge columns from the rendered range (default: true)
    #[serde(default)]
    pub trim_empty_columns: Option<bool>,
    /// Output format: "json" (default), "ascii", or "both"
    #[serde(default)]
    pub render: Option<LayoutRender>,
}

const LAYOUT_MAX_ROWS: u32 = 80;
const LAYOUT_MAX_COLS: u32 = 25;
const LAYOUT_DEFAULT_COL_WIDTH: f64 = 8.43;
const LAYOUT_DEFAULT_MAX_COL_WIDTH: u32 = 20;

pub async fn layout_page(
    state: impl crate::read_context::ReadContext,
    params: LayoutPageParams,
) -> Result<LayoutPageResponse> {
    let workbook = state.open_workbook(&params.workbook_or_fork_id).await?;

    let range_str = params.range.as_deref().unwrap_or("A1:T50");
    let ((min_col, min_row), (raw_max_col, raw_max_row)) =
        parse_range(range_str).ok_or_else(|| anyhow!("invalid range: {}", range_str))?;

    let requested_max_col_width = params
        .max_col_width
        .unwrap_or(LAYOUT_DEFAULT_MAX_COL_WIDTH)
        .max(3) as f64;
    let fit_columns = params.fit_columns.unwrap_or(false);
    let trim_empty_columns = params.trim_empty_columns.unwrap_or(true);
    let mode = params.mode.unwrap_or_default();
    let render = params.render.unwrap_or_default();

    // Cap to hard limits
    let max_col = raw_max_col.min(min_col + LAYOUT_MAX_COLS - 1);
    let max_row = raw_max_row.min(min_row + LAYOUT_MAX_ROWS - 1);
    let truncated = max_col < raw_max_col || max_row < raw_max_row;

    let (mut columns, mut merged_cells, mut rows) =
        workbook.with_sheet(&params.sheet_name, |sheet| {
            // ── column widths ────────────────────────────────────────────────
            let columns: Vec<LayoutPageColumnInfo> = (min_col..=max_col)
                .map(|col_idx| {
                    let col_name = column_number_to_name(col_idx);
                    let (raw_width, is_default) =
                        match sheet.get_column_dimension_by_number(&col_idx) {
                            Some(dim) => {
                                let w = *dim.get_width();
                                if w > 0.0 {
                                    (w, false)
                                } else {
                                    (LAYOUT_DEFAULT_COL_WIDTH, true)
                                }
                            }
                            None => (LAYOUT_DEFAULT_COL_WIDTH, true),
                        };
                    LayoutPageColumnInfo {
                        col: col_name,
                        index: col_idx,
                        width_chars: raw_width,
                        is_default_width: is_default,
                    }
                })
                .collect();

            // ── merged cells ─────────────────────────────────────────────────
            let merged_strings: Vec<String> = sheet
                .get_merge_cells()
                .iter()
                .map(|m| m.get_range())
                .collect();

            // Build set of (col, row) that are top-left of a merge span
            let merge_starts: std::collections::HashSet<(u32, u32)> = merged_strings
                .iter()
                .filter_map(|r| parse_range(r).map(|((c, ro), _)| (c, ro)))
                .collect();

            // ── cells ────────────────────────────────────────────────────────
            let mut cell_map: HashMap<(u32, u32), LayoutCellInfo> = HashMap::new();

            for cell in sheet.get_cell_collection() {
                let address = cell.get_coordinate().get_coordinate().to_string();
                let Some((col, row)) = parse_address(&address) else {
                    continue;
                };
                if col < min_col || col > max_col || row < min_row || row > max_row {
                    continue;
                }

                let text: String = match mode {
                    LayoutMode::Formulas => {
                        let formula = cell.get_formula();
                        if !formula.is_empty() {
                            format!("={formula}")
                        } else {
                            cell_display_string(cell)
                        }
                    }
                    LayoutMode::Values => cell_display_string(cell),
                };

                let desc = crate::styles::descriptor_from_style(cell.get_style());
                let bold = desc.font.as_ref().and_then(|f| f.bold);
                let italic = desc.font.as_ref().and_then(|f| f.italic);
                let align_h = desc.alignment.as_ref().and_then(|a| a.horizontal.clone());
                let borders = desc.borders.as_ref().map(|b| LayoutCellBorders {
                    top: b.top.as_ref().and_then(|s| s.style.clone()),
                    bottom: b.bottom.as_ref().and_then(|s| s.style.clone()),
                    left: b.left.as_ref().and_then(|s| s.style.clone()),
                    right: b.right.as_ref().and_then(|s| s.style.clone()),
                });
                let borders = borders.and_then(|b| if b.is_empty() { None } else { Some(b) });

                cell_map.insert(
                    (col, row),
                    LayoutCellInfo {
                        address,
                        value: if text.is_empty() { None } else { Some(text) },
                        bold,
                        italic,
                        align_h,
                        borders,
                        merge_start: if merge_starts.contains(&(col, row)) {
                            Some(true)
                        } else {
                            None
                        },
                    },
                );
            }

            // ── build row structs ────────────────────────────────────────────
            let rows: Vec<LayoutRowInfo> = (min_row..=max_row)
                .map(|row| {
                    let cells = (min_col..=max_col)
                        .map(|col| {
                            cell_map
                                .remove(&(col, row))
                                .unwrap_or_else(|| LayoutCellInfo {
                                    address: format!("{}{}", column_number_to_name(col), row),
                                    value: None,
                                    bold: None,
                                    italic: None,
                                    align_h: None,
                                    borders: None,
                                    merge_start: None,
                                })
                        })
                        .collect();
                    LayoutRowInfo { row, cells }
                })
                .collect();

            Ok::<_, anyhow::Error>((columns, merged_strings, rows))
        })??;

    let mut notes = Vec::new();
    let mut render_min_col = min_col;
    let mut render_max_col = max_col;

    if trim_empty_columns && !columns.is_empty() {
        let has_visible_content = |col_idx: usize| -> bool {
            rows.iter().any(|row| {
                let cell = &row.cells[col_idx];
                cell.value
                    .as_deref()
                    .map(|v| !v.is_empty())
                    .unwrap_or(false)
                    || cell.bold.unwrap_or(false)
                    || cell.italic.unwrap_or(false)
                    || cell.merge_start.unwrap_or(false)
                    || cell.borders.is_some()
            })
        };

        let mut start = 0usize;
        while start < columns.len() && !has_visible_content(start) {
            start += 1;
        }

        let mut end = columns.len();
        while end > start && !has_visible_content(end - 1) {
            end -= 1;
        }

        if start > 0 || end < columns.len() {
            if start < end {
                let trimmed_left = start;
                let trimmed_right = columns.len() - end;
                render_min_col = columns[start].index;
                render_max_col = columns[end - 1].index;
                columns = columns[start..end].to_vec();
                for row in &mut rows {
                    row.cells = row.cells[start..end].to_vec();
                }
                notes.push(format!(
                    "Trimmed empty edge columns (left: {trimmed_left}, right: {trimmed_right})"
                ));
            } else {
                // All requested columns are empty; keep one column to avoid empty-grid output.
                render_min_col = columns[0].index;
                render_max_col = columns[0].index;
                columns = vec![columns[0].clone()];
                for row in &mut rows {
                    row.cells = vec![row.cells[0].clone()];
                }
                notes.push(
                    "All requested columns were empty; kept one placeholder column".to_string(),
                );
            }
        }
    }

    // Filter merged cells to only those overlapping the rendered range
    merged_cells.retain(|mc| {
        parse_range(mc).is_some_and(|((c1, r1), (c2, r2))| {
            c1 <= render_max_col && c2 >= render_min_col && r1 <= max_row && r2 >= min_row
        })
    });

    if fit_columns {
        for (ci, col) in columns.iter_mut().enumerate() {
            // Fit-to-content should be able to shrink overly wide workbook columns,
            // so start from a small floor instead of the sheet's stored width.
            let mut max_len = 3usize;
            for row in &rows {
                if let Some(cell) = row.cells.get(ci) {
                    let content_len = cell
                        .value
                        .as_deref()
                        .map(|s| s.chars().count())
                        .unwrap_or(0);
                    let marker_len = usize::from(cell.bold.unwrap_or(false)) * 2
                        + usize::from(cell.italic.unwrap_or(false)) * 2;
                    max_len = max_len.max(content_len + marker_len);
                }
            }
            col.width_chars = max_len as f64;
        }
    } else {
        for col in &mut columns {
            col.width_chars = col.width_chars.min(requested_max_col_width);
        }
    }

    let effective_range = format!(
        "{}{}:{}{}",
        column_number_to_name(render_min_col),
        min_row,
        column_number_to_name(render_max_col),
        max_row,
    );

    let ascii_render = match render {
        LayoutRender::Ascii | LayoutRender::Both => Some(render_layout_ascii(
            &columns,
            &rows,
            if fit_columns {
                None
            } else {
                Some(requested_max_col_width as usize)
            },
        )),
        LayoutRender::Json => None,
    };

    if truncated {
        notes.push(format!(
            "Range capped to {LAYOUT_MAX_ROWS} rows × {LAYOUT_MAX_COLS} columns"
        ));
    }

    Ok(LayoutPageResponse {
        workbook_id: workbook.id.clone(),
        sheet_name: params.sheet_name,
        range: effective_range,
        columns,
        merged_cells,
        rows,
        ascii_render,
        truncated,
        notes,
    })
}

/// Format a cell's value as a display string for the layout render.
fn cell_display_string(cell: &umya_spreadsheet::Cell) -> String {
    use crate::workbook::cell_to_value;
    match cell_to_value(cell) {
        Some(CellValue::Text(s)) => s,
        Some(CellValue::Number(n)) => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{}", n as i64)
            } else {
                format!("{n}")
            }
        }
        Some(CellValue::Bool(b)) => if b { "TRUE" } else { "FALSE" }.to_string(),
        Some(CellValue::Error(e)) => e,
        Some(CellValue::Date(d)) => d,
        None => String::new(),
    }
}

/// Map an Excel border style string to a render weight (0–3).
fn border_weight(style: Option<&str>) -> u8 {
    match style {
        None => 0,
        Some(s) => match s.to_ascii_lowercase().as_str() {
            "none" => 0,
            "hair" | "dotted" | "dashed" | "dashDot" | "dashDotDot" => 1,
            "thin" | "slantDashDot" | "mediumDashDot" | "mediumDashDotDot" => 1,
            "medium" | "mediumDashed" => 2,
            "thick" => 2,
            "double" => 3,
            _ => 1,
        },
    }
}

/// Render a compact ASCII grid from layout data.
#[allow(
    clippy::unnecessary_map_or,
    clippy::if_same_then_else,
    clippy::needless_range_loop
)]
fn render_layout_ascii(
    columns: &[LayoutPageColumnInfo],
    rows: &[LayoutRowInfo],
    max_col_width: Option<usize>,
) -> String {
    use std::fmt::Write;

    if rows.is_empty() || columns.is_empty() {
        return String::new();
    }

    // Column display widths (capped, minimum 3 for truncation marker)
    let col_widths: Vec<usize> = columns
        .iter()
        .map(|c| {
            let raw = c.width_chars.ceil() as usize;
            let capped = max_col_width.map_or(raw, |cap| raw.min(cap));
            capped.max(3)
        })
        .collect();

    let n_cols = columns.len();
    let n_rows = rows.len();

    // Per-cell border weights indexed by (row_idx, col_idx)
    // We also need the border of the top edge of row 0 and left edge of col 0.
    let cell_borders = |ri: usize, ci: usize| -> (u8, u8, u8, u8) {
        // (top, bottom, left, right)
        if ri >= n_rows || ci >= n_cols {
            return (0, 0, 0, 0);
        }
        let cell = &rows[ri].cells[ci];
        let b = cell.borders.as_ref();
        (
            border_weight(b.and_then(|b| b.top.as_deref())),
            border_weight(b.and_then(|b| b.bottom.as_deref())),
            border_weight(b.and_then(|b| b.left.as_deref())),
            border_weight(b.and_then(|b| b.right.as_deref())),
        )
    };

    // Horizontal separator weight between row `above` (row_idx) and the next row.
    // above = usize::MAX means the top frame edge (above row 0).
    // above = n_rows means the bottom frame edge.
    let h_sep_weight = |above: usize, ci: usize| -> u8 {
        let bottom_of_above = if above < n_rows {
            cell_borders(above, ci).1
        } else {
            0
        };
        let top_of_below = if above.checked_add(1).map_or(false, |r| r < n_rows) {
            cell_borders(above + 1, ci).0
        } else {
            0
        };
        bottom_of_above.max(top_of_below)
    };

    // Vertical separator weight between col `left` and the next col.
    let v_sep_weight = |ri: usize, left: usize| -> u8 {
        let right_of_left = if left < n_cols {
            cell_borders(ri, left).3
        } else {
            0
        };
        let left_of_right = if left + 1 < n_cols {
            cell_borders(ri, left + 1).2
        } else {
            0
        };
        right_of_left.max(left_of_right)
    };

    // Characters by weight
    let h_char = |w: u8| match w {
        0 => ' ',
        1 => '─',
        2 => '━',
        _ => '═',
    };
    let v_char = |w: u8| match w {
        0 => ' ',
        1 => '│',
        2 => '┃',
        _ => '║',
    };

    // Junction character at intersection of horizontal line (weight hw) and vertical line (weight vw)
    let junction = |hw: u8, vw: u8| -> char {
        match (hw, vw) {
            (0, 0) => ' ',
            (0, _) => v_char(vw),
            (_, 0) => h_char(hw),
            (1, 1) => '┼',
            (1, 2) | (1, 3) => '╂',
            (2, 1) | (3, 1) => '┿',
            _ => '╋',
        }
    };

    let mut out = String::new();

    // Legend
    let _ = writeln!(
        out,
        "[*=bold  /=italic  border weight: ─thin ━medium ═double]"
    );

    // Draw a horizontal separator line (top/bottom/between-row)
    // `above` is the row index above this separator (usize::MAX = before row 0, n_rows = after last row)
    let draw_h_line = |out: &mut String, above: usize, is_top: bool, is_bottom: bool| {
        // Left junction
        let left_v = if is_top || is_bottom { 0u8 } else { 0u8 }; // outer frame has no vertical sep
        let _ = write!(
            out,
            "{}",
            junction(
                h_sep_weight(above, 0).max(1)
                    * (is_top || is_bottom || h_sep_weight(above, 0) > 0) as u8,
                left_v
            )
        );
        for ci in 0..n_cols {
            let hw = h_sep_weight(above, ci).max(if is_top || is_bottom { 1 } else { 0 });
            for _ in 0..col_widths[ci] + 2 {
                let _ = write!(out, "{}", h_char(hw));
            }
            // Right junction or edge
            if ci + 1 < n_cols {
                let vw = v_sep_weight(if above < n_rows { above } else { n_rows - 1 }, ci);
                let _ = write!(out, "{}", junction(hw, vw));
            } else {
                let _ = write!(out, "{}", junction(hw, 0));
            }
        }
        let _ = writeln!(out);
    };

    // Top border line
    draw_h_line(&mut out, usize::MAX, true, false);

    for ri in 0..n_rows {
        // Content row
        // Left outer edge
        let _ = write!(out, "{}", v_char(1));
        for ci in 0..n_cols {
            let cell = &rows[ri].cells[ci];
            let w = col_widths[ci];

            let content = cell.value.as_deref().unwrap_or("");
            // Truncate to fit (leaving room for bold/italic markers)
            let bold = cell.bold.unwrap_or(false);
            let italic = cell.italic.unwrap_or(false);
            let marker_chars = if bold && italic {
                4
            } else if bold || italic {
                2
            } else {
                0
            };
            let content_width = w.saturating_sub(marker_chars);
            let truncated_content: String = if content.chars().count() > content_width {
                let mut s: String = content
                    .chars()
                    .take(content_width.saturating_sub(1))
                    .collect();
                s.push('…');
                s
            } else {
                content.to_string()
            };

            // Determine alignment: right-align if align_h is "right", or if it looks numeric and align_h is not explicitly left
            let is_numeric = content
                .trim_start_matches(['-', '$', '('])
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false);
            let right_align = cell.align_h.as_deref() == Some("right")
                || (is_numeric && cell.align_h.as_deref().map(|a| a != "left").unwrap_or(true));

            // Build decorated content
            let decorated = {
                let mut s = String::new();
                if bold {
                    s.push('*');
                }
                if italic {
                    s.push('/');
                }
                s.push_str(&truncated_content);
                if italic {
                    s.push('/');
                }
                if bold {
                    s.push('*');
                }
                s
            };

            // Pad to column width
            let decorated_len = decorated.chars().count();
            let padded = if right_align {
                format!(
                    " {:>width$} ",
                    decorated,
                    width = w.saturating_sub(decorated_len) + decorated_len
                )
            } else {
                format!(
                    " {:<width$} ",
                    decorated,
                    width = w.saturating_sub(decorated_len) + decorated_len
                )
            };
            let _ = write!(out, "{}", padded);

            // Right separator
            if ci + 1 < n_cols {
                let vw = v_sep_weight(ri, ci).max(1); // always at least thin inside the frame
                let _ = write!(out, "{}", v_char(vw));
            } else {
                let _ = write!(out, "{}", v_char(1));
            }
        }
        let _ = writeln!(out);

        // Separator after this row
        if ri + 1 < n_rows {
            // Only draw if any column has a border between these rows
            let max_w: u8 = (0..n_cols)
                .map(|ci| h_sep_weight(ri, ci))
                .max()
                .unwrap_or(0);
            if max_w > 0 {
                draw_h_line(&mut out, ri, false, false);
            }
        }
    }

    // Bottom border
    draw_h_line(&mut out, n_rows - 1, false, true);

    out
}
