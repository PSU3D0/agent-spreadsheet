use crate::model::*;
use crate::operations::{CanonicalErrorCode, CanonicalErrorEnvelope, ResourceId};
use crate::tools::{self, FilterOp, FormulaSortBy, MatchMode, SampleMode, StyleGranularity};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListWorkbooksRequest {
    #[serde(default)]
    pub slug_prefix: Option<String>,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub path_glob: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub include_paths: Option<bool>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanonicalWorkbookDescriptor {
    pub resource_id: ResourceId,
    pub metadata: WorkbookDiscoveryMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<WorkbookPaths>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_capabilities: Option<crate::caps::BackendCaps>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkbookDiscoveryMetadata {
    pub short_id: String,
    pub slug: String,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkbookPaths {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListWorkbooksData {
    pub workbooks: Vec<CanonicalWorkbookDescriptor>,
    pub next_offset: Option<u32>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DescribeInclude {
    Summary,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeWorkbookRequest {
    pub resource_id: ResourceId,
    #[serde(default)]
    pub include_paths: Option<bool>,
    #[serde(default)]
    pub include: Vec<DescribeInclude>,
    #[serde(default)]
    pub summary: Option<DescribeSummaryOptions>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeSummaryOptions {
    #[serde(default)]
    pub include_entry_points: Option<bool>,
    #[serde(default)]
    pub include_named_ranges: Option<bool>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeWorkbookData {
    pub metadata: WorkbookExactMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<WorkbookPaths>,
    pub capabilities: WorkbookCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<WorkbookDerivedSummary>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkbookExactMetadata {
    pub short_id: String,
    pub slug: String,
    pub bytes: Option<u64>,
    pub last_modified: Option<String>,
    pub sheet_count: usize,
    pub defined_name_count: usize,
    pub table_count: usize,
    pub macros_present: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkbookCapabilities {
    pub backend: crate::caps::BackendCaps,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkbookDerivedSummary {
    pub status: CoverageStatus,
    pub total_cells: u64,
    pub total_formulas: u64,
    pub breakdown: WorkbookBreakdown,
    pub region_counts: RegionCountSummary,
    pub key_named_ranges: Vec<NamedRangeDescriptor>,
    pub suggested_entry_points: Vec<EntryPoint>,
    pub notes: Vec<String>,
    pub coverage: WorkbookSummaryCoverage,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    Complete,
    Bounded,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkbookSummaryCoverage {
    pub sheets_scanned: usize,
    pub sheets_total: usize,
    pub bounded: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadCellsFormat {
    Dense,
    Values,
    Csv,
    Json,
    Rows,
    Full,
    Compact,
    ValuesOnly,
}

impl ReadCellsFormat {
    fn range_format(self) -> TableOutputFormat {
        match self {
            Self::Dense => TableOutputFormat::Dense,
            Self::Values | Self::Compact | Self::ValuesOnly => TableOutputFormat::Values,
            Self::Csv => TableOutputFormat::Csv,
            Self::Json | Self::Full => TableOutputFormat::Json,
            Self::Rows => TableOutputFormat::Rows,
        }
    }

    fn row_format(self) -> SheetPageFormat {
        match self {
            Self::Full | Self::Json => SheetPageFormat::Full,
            Self::Compact | Self::Dense | Self::Csv | Self::Rows => SheetPageFormat::Compact,
            Self::Values | Self::ValuesOnly => SheetPageFormat::ValuesOnly,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadCellField {
    Value,
    Formula,
    CachedValue,
    StoredKind,
    NumberFormat,
    StyleTags,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadCellsSelection {
    Range {
        ranges: Vec<String>,
        #[serde(default)]
        include_headers: Option<bool>,
    },
    Rows {
        #[serde(default = "one")]
        start_row: u32,
        row_count: u32,
        #[serde(default)]
        columns: Option<RowColumnSelection>,
        #[serde(default)]
        include_header: Option<bool>,
    },
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RowColumnSelection {
    All,
    Letters {
        values: Vec<String>,
    },
    Headers {
        values: Vec<String>,
        #[serde(default)]
        header_row: Option<u32>,
    },
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanonicalReadPage {
    #[serde(default)]
    pub limit_rows: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadCellsRequest {
    pub resource_id: ResourceId,
    pub sheet_name: String,
    pub selection: ReadCellsSelection,
    #[serde(default)]
    pub fields: Vec<ReadCellField>,
    #[serde(default)]
    pub include_formulas: Option<bool>,
    #[serde(default)]
    pub include_styles: Option<bool>,
    #[serde(default)]
    pub format: Option<ReadCellsFormat>,
    #[serde(default)]
    pub encoding: Option<ReadCellsFormat>,
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub page: Option<CanonicalReadPage>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadCellsData {
    pub sheet_name: String,
    pub selection_kind: ReadCellsSelectionKind,
    pub encoding: ReadCellsFormat,
    pub blocks: Vec<ReadCellsBlock>,
    pub header: Option<RowSnapshot>,
    pub calculation: CalculationMetadata,
    pub page: ReadCellsPage,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReadCellsSelectionKind {
    Range,
    Rows,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadCellsBlock {
    pub selection_index: usize,
    pub requested_range: String,
    pub returned_range: String,
    pub row_count: usize,
    pub column_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_indices: Option<Vec<u32>>,
    pub payload: ReadCellsPayload,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadCellsPayload {
    pub encoding: ReadCellsFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<Vec<Option<CellValue>>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formulas: Option<Vec<Vec<Option<String>>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<Vec<Option<CellValuePrimitive>>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dense: Option<RangeValuesDensePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_keyed: Option<Vec<RangeValuesRowEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshots: Option<Vec<RowSnapshot>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compact: Option<SheetPageCompact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values_only: Option<SheetPageValues>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected: Option<Vec<Vec<CanonicalCellProjection>>>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalStoredKind {
    Formula,
    Text,
    Number,
    Bool,
    Error,
    Date,
    Blank,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanonicalCellProjection {
    pub address: String,
    pub value: Option<CellValue>,
    pub formula: Option<String>,
    pub cached_value: Option<CellValue>,
    pub stored_kind: CanonicalStoredKind,
    pub number_format: Option<String>,
    pub style_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadCellsPage {
    pub rows_returned: usize,
    pub cells_returned: usize,
    pub complete: bool,
    pub next_cursor: Option<String>,
    pub limits: ReadCellsLimits,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadCellsLimits {
    pub requested_rows: u32,
    pub max_cells: Option<usize>,
    pub max_payload_bytes: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadCellsCursor {
    revision_id: String,
    fingerprint: String,
    selection_index: usize,
    next_row: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectCellsRequest {
    pub resource_id: ResourceId,
    pub sheet_name: String,
    pub targets: Vec<String>,
    #[serde(default)]
    pub include_empty: Option<bool>,
    #[serde(default)]
    pub budget: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamedRangesRequest {
    pub resource_id: ResourceId,
    #[serde(default)]
    pub sheet_name: Option<String>,
    #[serde(default)]
    pub name_prefix: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchValuesRequest {
    pub resource_id: ResourceId,
    pub query: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub mode: Option<FindMode>,
    #[serde(default)]
    pub match_mode: Option<MatchMode>,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub sheet_name: Option<String>,
    #[serde(default)]
    pub region_id: Option<u32>,
    #[serde(default)]
    pub table_name: Option<String>,
    #[serde(default)]
    pub value_types: Option<Vec<tools::ValueTypeFilter>>,
    #[serde(default)]
    pub search_headers_only: bool,
    #[serde(default)]
    pub direction: Option<LabelDirection>,
    #[serde(default = "fifty")]
    pub limit: u32,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub context: Option<tools::FindContext>,
    #[serde(default)]
    pub context_width: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormulaQuery {
    pub text: String,
    #[serde(default)]
    pub match_mode: Option<MatchMode>,
    #[serde(default)]
    pub case_sensitive: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct FormulaFilter {
    #[serde(default)]
    pub volatile: Option<bool>,
    #[serde(default)]
    pub function_names: Vec<String>,
    #[serde(default)]
    pub has_external_references: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FormulaSearchScope {
    Workbook,
    Sheet {
        sheet_name: String,
        #[serde(default)]
        range: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FormulaResultMode {
    Cells,
    Groups,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FormulaGroupBy {
    Function,
    NormalizedFormula,
    Fingerprint,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormulaContextRequest {
    pub rows: u32,
    pub columns: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchFormulasRequest {
    pub resource_id: ResourceId,
    #[serde(default)]
    pub scope: Option<FormulaSearchScope>,
    #[serde(default)]
    pub query: Option<FormulaQuery>,
    #[serde(default)]
    pub filter: Option<FormulaFilter>,
    pub result_mode: FormulaResultMode,
    #[serde(default)]
    pub group_by: Option<FormulaGroupBy>,
    #[serde(default)]
    pub include_addresses: Option<bool>,
    #[serde(default)]
    pub addresses_per_group: Option<u32>,
    #[serde(default)]
    pub include_context: Option<FormulaContextRequest>,
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchFormulasData {
    pub result_mode: FormulaResultMode,
    pub matches: Vec<FormulaCellMatch>,
    pub groups: Vec<FormulaSearchGroup>,
    pub summary: FormulaSearchSummary,
    pub formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
    pub next_offset: Option<u32>,
    pub next_cursor: Option<String>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormulaCellMatch {
    pub sheet_name: String,
    pub address: String,
    pub formula: String,
    pub cached_value: Option<CellValue>,
    pub classifications: FormulaClassifications,
    pub context: Vec<RowSnapshot>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormulaClassifications {
    pub volatile: bool,
    pub functions: Vec<String>,
    pub external_references: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormulaSearchGroup {
    pub group_key: String,
    pub formula: Option<String>,
    pub fingerprint: Option<String>,
    pub volatile: bool,
    pub functions: Vec<String>,
    pub cell_count: usize,
    pub addresses: Vec<String>,
    pub addresses_complete: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormulaSearchSummary {
    pub formula_cells_scanned: usize,
    pub matched_cells: usize,
    pub matched_groups: usize,
    pub scan_complete: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormulaTraceRequest {
    pub resource_id: ResourceId,
    pub sheet_name: String,
    pub cell_address: String,
    pub direction: TraceDirection,
    #[serde(default)]
    pub depth: Option<u32>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub page_size: Option<usize>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormulaTraceData {
    pub workbook_id: WorkbookId,
    pub sheet_name: String,
    pub origin: String,
    pub direction: TraceDirection,
    pub layers: Vec<TraceLayer>,
    pub next_cursor: Option<String>,
    pub formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
    pub notes: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormulaMapRequest {
    pub resource_id: ResourceId,
    pub sheet_name: String,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub expand: bool,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub sort_by: Option<FormulaSortBy>,
    #[serde(default)]
    pub summary_only: Option<bool>,
    #[serde(default)]
    pub include_addresses: Option<bool>,
    #[serde(default)]
    pub addresses_limit: Option<u32>,
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormulaMapData {
    pub workbook_id: WorkbookId,
    pub sheet_name: String,
    pub groups: Vec<FormulaGroup>,
    pub formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
    pub next_offset: Option<u32>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileTableRequest {
    pub resource_id: ResourceId,
    #[serde(default)]
    pub sheet_name: Option<String>,
    #[serde(default)]
    pub region_id: Option<u32>,
    #[serde(default)]
    pub table_name: Option<String>,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub sample_mode: Option<SampleMode>,
    #[serde(default)]
    pub sample_size: Option<u32>,
    #[serde(default)]
    pub summary_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileTableData {
    pub workbook_id: WorkbookId,
    pub sheet_name: String,
    pub table_name: Option<String>,
    pub headers: Vec<String>,
    pub column_types: Vec<ColumnTypeSummary>,
    pub row_count: u32,
    pub samples: Vec<TableRow>,
    pub notes: Vec<String>,
    pub source: ProfileSource,
    pub coverage: ProfileCoverage,
    pub confidence: ProfileConfidence,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileSource {
    pub sheet_name: String,
    pub selector_kind: String,
    pub selector_value: Option<String>,
    pub bounds: String,
    pub header_row: u32,
    pub header_provenance: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileCoverage {
    pub rows_in_scope: u32,
    pub rows_scanned: u32,
    pub sample_mode: SampleMode,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfidence {
    pub status: String,
    pub heuristic: bool,
    pub reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SheetStatisticsRequest {
    pub resource_id: ResourceId,
    pub sheet_name: String,
    #[serde(default)]
    pub sample_rows: Option<usize>,
    #[serde(default)]
    pub summary_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadLayoutRequest {
    pub resource_id: ResourceId,
    pub sheet_name: String,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub mode: Option<LayoutMode>,
    #[serde(default)]
    pub max_col_width: Option<u32>,
    #[serde(default)]
    pub fit_columns: Option<bool>,
    #[serde(default)]
    pub trim_empty_columns: Option<bool>,
    #[serde(default)]
    pub render: Option<LayoutRender>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadLayoutData {
    pub lossiness: LayoutLossiness,
    pub layout: LayoutPageResponse,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LayoutLossiness {
    Lossy,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportGridRequest {
    pub resource_id: ResourceId,
    pub sheet_name: String,
    pub range: String,
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportGridData {
    pub fidelity: GridFidelity,
    pub requested_range: String,
    pub returned_range: String,
    pub grid: GridPayload,
    pub complete: bool,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GridFidelity {
    CellContentAndExplicitFormatting,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnalyzeStylesScope {
    Workbook,
    Sheet {
        sheet_name: String,
        #[serde(default)]
        selection: Option<StyleSelection>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StyleSelection {
    All,
    Range { range: String },
    Region { region_id: u32 },
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StyleInclude {
    Descriptors,
    Ranges,
    ExampleCells,
    Theme,
    ConditionalFormats,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnalyzeStylesLimits {
    #[serde(default)]
    pub cells_scanned: Option<u32>,
    #[serde(default)]
    pub examples_per_style: Option<u32>,
    #[serde(default)]
    pub ranges_per_style: Option<u32>,
    #[serde(default)]
    pub styles: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnalyzeStylesRequest {
    pub resource_id: ResourceId,
    pub scope: AnalyzeStylesScope,
    #[serde(default)]
    pub group_by: Option<StyleGranularity>,
    #[serde(default)]
    pub include: Vec<StyleInclude>,
    #[serde(default)]
    pub limits: Option<AnalyzeStylesLimits>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnalyzeStylesData {
    pub scope: AnalyzeStylesScope,
    pub styles: Vec<CanonicalStyleUsage>,
    pub theme: Option<ThemeSummary>,
    pub conditional_formats: Vec<ConditionalFormatSummary>,
    pub conditional_formats_complete: bool,
    pub coverage: StyleCoverage,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnalyzeStylesScopeKind {
    Workbook,
    Sheet,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanonicalStyleUsage {
    pub style_id: String,
    pub occurrences: u32,
    pub tags: Vec<String>,
    pub descriptor: Option<StyleDescriptor>,
    pub ranges: Vec<String>,
    pub ranges_complete: bool,
    pub example_cells: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StyleCoverage {
    pub status: CoverageStatus,
    pub cells_scanned: u64,
    pub cells_in_scope: Option<u64>,
    pub counts_exact: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanonicalTableFilter {
    pub column: String,
    pub op: FilterOp,
    pub value: Value,
}

impl From<CanonicalTableFilter> for tools::TableFilter {
    fn from(value: CanonicalTableFilter) -> Self {
        Self {
            column: value.column,
            op: value.op,
            value: value.value,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadTableRequest {
    pub resource_id: ResourceId,
    #[serde(default)]
    pub sheet_name: Option<String>,
    #[serde(default)]
    pub table_name: Option<String>,
    #[serde(default)]
    pub region_id: Option<u32>,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub header_row: Option<u32>,
    #[serde(default)]
    pub header_rows: Option<u32>,
    #[serde(default)]
    pub columns: Option<Vec<String>>,
    #[serde(default)]
    pub filters: Option<Vec<CanonicalTableFilter>>,
    #[serde(default)]
    pub sample_mode: Option<SampleMode>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub format: Option<TableOutputFormat>,
    #[serde(default)]
    pub include_headers: Option<bool>,
    #[serde(default)]
    pub include_types: Option<bool>,
}

fn one() -> u32 {
    1
}
fn fifty() -> u32 {
    50
}

pub fn fingerprint_read_cells(request: &ReadCellsRequest) -> String {
    let invariant = format!(
        "{}|{:?}|{:?}|{:?}|{:?}|{:?}",
        request.sheet_name,
        request.selection,
        request.fields,
        request.include_formulas,
        request.include_styles,
        request.format.or(request.encoding)
    );
    format!("{:x}", Sha256::digest(invariant.as_bytes()))
}

fn encode_cursor(cursor: &ReadCellsCursor) -> Result<String, serde_json::Error> {
    serde_json::to_vec(cursor).map(|bytes| {
        let mut encoded = String::from("rc1_");
        for byte in bytes {
            encoded.push_str(&format!("{byte:02x}"));
        }
        encoded
    })
}

fn decode_cursor(value: &str) -> Result<ReadCellsCursor, String> {
    let hex = value
        .strip_prefix("rc1_")
        .ok_or_else(|| "cursor is not a read_cells v1 cursor".to_string())?;
    if hex.len() % 2 != 0 {
        return Err("cursor has invalid length".to_string());
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&hex[index..index + 2], 16)
                .map_err(|_| "cursor is not valid hexadecimal".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::from_slice(&bytes).map_err(|_| "cursor payload is invalid".to_string())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalOpaqueCursor {
    revision_id: String,
    fingerprint: String,
    position: usize,
    subposition: usize,
}

fn encode_opaque_cursor(
    prefix: &str,
    cursor: &CanonicalOpaqueCursor,
) -> Result<String, serde_json::Error> {
    serde_json::to_vec(cursor).map(|bytes| {
        let mut encoded = format!("{prefix}_");
        for byte in bytes {
            encoded.push_str(&format!("{byte:02x}"));
        }
        encoded
    })
}

fn decode_opaque_cursor(value: &str, prefix: &str) -> Result<CanonicalOpaqueCursor, String> {
    let marker = format!("{prefix}_");
    let hex = value
        .strip_prefix(&marker)
        .ok_or_else(|| format!("cursor is not a {prefix} cursor"))?;
    if hex.len() % 2 != 0 {
        return Err("cursor has invalid length".to_string());
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&hex[index..index + 2], 16)
                .map_err(|_| "cursor is not valid hexadecimal".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::from_slice(&bytes).map_err(|_| "cursor payload is invalid".to_string())
}

fn validate_opaque_cursor(
    value: &str,
    prefix: &str,
    revision_id: &str,
    fingerprint: &str,
    operation: &str,
) -> Result<CanonicalOpaqueCursor, CanonicalErrorEnvelope> {
    let cursor = decode_opaque_cursor(value, prefix).map_err(|message| {
        canonical_error(
            CanonicalErrorCode::InvalidRequest,
            operation,
            message,
            Some("$.cursor"),
        )
    })?;
    if cursor.revision_id != revision_id {
        return Err(canonical_error(
            CanonicalErrorCode::StaleCursor,
            operation,
            "cursor is bound to a different resource revision",
            Some("$.cursor"),
        ));
    }
    if cursor.fingerprint != fingerprint {
        return Err(canonical_error(
            CanonicalErrorCode::CursorMismatch,
            operation,
            "cursor does not match the invariant request",
            Some("$.cursor"),
        ));
    }
    Ok(cursor)
}

fn parse_range(range: &str) -> Result<(u32, u32, u32, u32), String> {
    let bare = range.rsplit_once('!').map_or(range, |(_, value)| value);
    let mut parts = bare.split(':');
    let first = parts.next().unwrap_or_default();
    let second = parts.next().unwrap_or(first);
    if parts.next().is_some() {
        return Err(format!("invalid range: {range}"));
    }
    let parse = |cell: &str| -> Result<(u32, u32), String> {
        let cell = cell.trim();
        let bytes = cell.as_bytes();
        let mut index = usize::from(bytes.first() == Some(&b'$'));
        let letters_start = index;
        let mut col = 0_u32;
        while let Some(byte) = bytes.get(index).filter(|byte| byte.is_ascii_alphabetic()) {
            col = col
                .checked_mul(26)
                .and_then(|value| {
                    value.checked_add(u32::from(byte.to_ascii_uppercase() - b'A' + 1))
                })
                .ok_or_else(|| format!("invalid range: {range}"))?;
            index += 1;
        }
        if index == letters_start {
            return Err(format!("invalid range: {range}"));
        }
        if bytes.get(index) == Some(&b'$') {
            index += 1;
        }
        let digits_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == digits_start || index != bytes.len() {
            return Err(format!("invalid range: {range}"));
        }
        let row = cell[digits_start..]
            .parse::<u32>()
            .map_err(|_| format!("invalid range: {range}"))?;
        if row == 0 {
            return Err(format!("invalid range: {range}"));
        }
        Ok((col, row))
    };
    let (c1, r1) = parse(first)?;
    let (c2, r2) = parse(second)?;
    if c1 > c2 || r1 > r2 {
        return Err(format!("invalid range: {range}"));
    }
    Ok((c1, r1, c2, r2))
}

fn col_name(mut col: u32) -> String {
    let mut result = Vec::new();
    while col > 0 {
        col -= 1;
        result.push((b'A' + (col % 26) as u8) as char);
        col /= 26;
    }
    result.iter().rev().collect()
}

fn range_string(c1: u32, r1: u32, c2: u32, r2: u32) -> String {
    format!("{}{r1}:{}{r2}", col_name(c1), col_name(c2))
}

fn requested_cursor(request: &ReadCellsRequest) -> Option<&str> {
    request.cursor.as_deref().or_else(|| {
        request
            .page
            .as_ref()
            .and_then(|page| page.cursor.as_deref())
    })
}

fn requested_page_size(request: &ReadCellsRequest) -> u32 {
    request
        .page_size
        .or_else(|| request.page.as_ref().and_then(|page| page.limit_rows))
        .unwrap_or(50)
        .clamp(1, 500)
}

fn requested_encoding(request: &ReadCellsRequest) -> ReadCellsFormat {
    request
        .format
        .or(request.encoding)
        .unwrap_or(ReadCellsFormat::Dense)
}

fn canonical_error(
    code: CanonicalErrorCode,
    operation: &str,
    message: impl Into<String>,
    path: Option<&str>,
) -> CanonicalErrorEnvelope {
    CanonicalErrorEnvelope::new(code, message, Some(operation), path.map(str::to_string))
}

fn project_cell_snapshots(rows: &[RowSnapshot]) -> Vec<Vec<CanonicalCellProjection>> {
    rows.iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| CanonicalCellProjection {
                    address: cell.address.clone(),
                    value: cell.value.clone(),
                    formula: cell.formula.clone(),
                    cached_value: cell.cached_value.clone(),
                    stored_kind: if cell.formula.is_some() {
                        CanonicalStoredKind::Formula
                    } else {
                        match cell.value.as_ref().or(cell.cached_value.as_ref()) {
                            Some(CellValue::Text(_)) => CanonicalStoredKind::Text,
                            Some(CellValue::Number(_)) => CanonicalStoredKind::Number,
                            Some(CellValue::Bool(_)) => CanonicalStoredKind::Bool,
                            Some(CellValue::Error(_)) => CanonicalStoredKind::Error,
                            Some(CellValue::Date(_)) => CanonicalStoredKind::Date,
                            None => CanonicalStoredKind::Blank,
                        }
                    },
                    number_format: cell.number_format.clone(),
                    style_tags: cell.style_tags.clone(),
                })
                .collect()
        })
        .collect()
}

fn range_payload_from_snapshots(
    encoding: ReadCellsFormat,
    returned_range: &str,
    rows: &[RowSnapshot],
    include_formulas: bool,
    include_projection: bool,
) -> ReadCellsPayload {
    let values = rows
        .iter()
        .map(|row| row.cells.iter().map(|cell| cell.value.clone()).collect())
        .collect::<Vec<Vec<Option<CellValue>>>>();
    let formulas = include_formulas.then(|| {
        rows.iter()
            .map(|row| row.cells.iter().map(|cell| cell.formula.clone()).collect())
            .collect::<Vec<Vec<Option<String>>>>()
    });
    let entry = tools::build_range_values_entry(
        encoding.range_format(),
        returned_range,
        &values,
        formulas.as_deref(),
        None,
    );
    ReadCellsPayload {
        encoding,
        rows: entry.rows,
        formulas: entry.formulas,
        values: entry.values,
        dense: entry.dense,
        csv: entry.csv,
        rows_keyed: entry.rows_keyed,
        snapshots: None,
        compact: None,
        values_only: None,
        projected: include_projection.then(|| project_cell_snapshots(rows)),
    }
}

fn rows_payload_from_snapshots(
    encoding: ReadCellsFormat,
    header: &Option<RowSnapshot>,
    rows: &[RowSnapshot],
    include_header: bool,
    include_snapshots: bool,
    include_projection: bool,
) -> ReadCellsPayload {
    let format = encoding.row_format();
    let detailed_compact = matches!(format, SheetPageFormat::Compact) && include_projection;
    ReadCellsPayload {
        encoding,
        rows: None,
        formulas: None,
        values: None,
        dense: None,
        csv: None,
        rows_keyed: None,
        snapshots: (!detailed_compact
            && (matches!(format, SheetPageFormat::Full) || include_snapshots))
            .then(|| rows.to_vec()),
        compact: (matches!(format, SheetPageFormat::Compact) && !detailed_compact)
            .then(|| tools::build_compact_payload(header, rows, include_header)),
        values_only: matches!(format, SheetPageFormat::ValuesOnly)
            .then(|| tools::build_values_only_payload(header, rows, include_header)),
        projected: include_projection.then(|| project_cell_snapshots(rows)),
    }
}

struct CanonicalBudgetDecision {
    rows: usize,
    first_row_fits: bool,
}

fn canonical_budget_decision<F>(
    available_rows: usize,
    cells_per_row: usize,
    cells_already_emitted: usize,
    max_cells: Option<usize>,
    max_payload_bytes: Option<usize>,
    existing_blocks: &[ReadCellsBlock],
    mut build_block: F,
) -> CanonicalBudgetDecision
where
    F: FnMut(usize) -> ReadCellsBlock,
{
    let cell_limited_rows = max_cells.map_or(available_rows, |limit| {
        if cells_per_row == 0 {
            available_rows
        } else {
            limit
                .saturating_sub(cells_already_emitted)
                .checked_div(cells_per_row)
                .unwrap_or(0)
                .min(available_rows)
        }
    });
    let fits_payload = |count: usize, block: ReadCellsBlock, existing: &[ReadCellsBlock]| {
        max_payload_bytes.is_none_or(|limit| {
            let mut blocks = existing.to_vec();
            if count > 0 {
                blocks.push(block);
            }
            serde_json::to_vec(&blocks).is_ok_and(|payload| payload.len() <= limit)
        })
    };
    let mut low = 0_usize;
    let mut high = cell_limited_rows;
    while low < high {
        let mid = (low + high).div_ceil(2);
        if fits_payload(mid, build_block(mid), existing_blocks) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let first_row_fits = available_rows == 0
        || (max_cells.is_none_or(|limit| cells_per_row <= limit)
            && fits_payload(1, build_block(1), &[]));
    CanonicalBudgetDecision {
        rows: low,
        first_row_fits,
    }
}

pub async fn execute_read_cells(
    state: impl crate::read_context::ReadContext,
    request: &ReadCellsRequest,
    revision_id: &str,
) -> Result<ReadCellsData, CanonicalErrorEnvelope> {
    let workbook_id = request.resource_id.to_workbook_id();
    let workbook = state.open_workbook(&workbook_id).await.map_err(|error| {
        canonical_error(
            CanonicalErrorCode::ResourceNotFound,
            "read_cells",
            error.to_string(),
            Some("$.resource_id"),
        )
    })?;
    let calculation =
        state.calculation_metadata(&workbook_id, revision_id, workbook.calculation_metadata());
    read_cells_from_view(
        &workbook,
        request,
        revision_id,
        calculation,
        state.config().max_cells(),
        state.config().max_payload_bytes(),
    )
}

pub fn read_cells_from_view<S: crate::workbook::WorkbookReadSource>(
    workbook: &crate::workbook::WorkbookContext<S>,
    request: &ReadCellsRequest,
    revision_id: &str,
    calculation: crate::model::CalculationMetadata,
    max_cells: Option<usize>,
    max_payload_bytes: Option<usize>,
) -> Result<ReadCellsData, CanonicalErrorEnvelope> {
    let operation = "read_cells";
    let fingerprint = fingerprint_read_cells(request);
    let cursor = if let Some(value) = requested_cursor(request) {
        let cursor = decode_cursor(value).map_err(|message| {
            canonical_error(
                CanonicalErrorCode::InvalidRequest,
                operation,
                message,
                Some("$.cursor"),
            )
        })?;
        if cursor.revision_id != revision_id {
            return Err(canonical_error(
                CanonicalErrorCode::StaleCursor,
                operation,
                "cursor is bound to a different resource revision",
                Some("$.cursor"),
            ));
        }
        if cursor.fingerprint != fingerprint {
            return Err(canonical_error(
                CanonicalErrorCode::CursorMismatch,
                operation,
                "cursor does not match the invariant read_cells request",
                Some("$.cursor"),
            ));
        }
        Some(cursor)
    } else {
        None
    };

    let workbook_id = request.resource_id.to_workbook_id();
    let encoding = requested_encoding(request);
    let requested_rows = requested_page_size(request);
    let include_formulas = request
        .include_formulas
        .unwrap_or_else(|| request.fields.contains(&ReadCellField::Formula));
    let include_styles = request.include_styles.unwrap_or_else(|| {
        request.fields.iter().any(|field| {
            matches!(
                field,
                ReadCellField::NumberFormat | ReadCellField::StyleTags
            )
        })
    });
    let mut blocks = Vec::new();
    let mut rows_returned = 0_usize;
    let mut cells_returned = 0_usize;
    let mut next_cursor = None;
    let mut header = None;

    match &request.selection {
        ReadCellsSelection::Range {
            ranges,
            include_headers: _,
        } => {
            if ranges.is_empty() {
                return Err(canonical_error(
                    CanonicalErrorCode::InvalidRequest,
                    operation,
                    "selection.ranges must not be empty",
                    Some("$.selection.ranges"),
                ));
            }
            let start_index = cursor.as_ref().map_or(0, |value| value.selection_index);
            for (index, requested_range) in ranges.iter().enumerate().skip(start_index) {
                if rows_returned >= requested_rows as usize {
                    break;
                }
                let (c1, original_r1, c2, r2) =
                    parse_range(requested_range).map_err(|message| {
                        canonical_error(
                            CanonicalErrorCode::InvalidRequest,
                            operation,
                            message,
                            Some("$.selection.ranges"),
                        )
                    })?;
                let r1 = if index == start_index {
                    cursor.as_ref().map_or(original_r1, |value| value.next_row)
                } else {
                    original_r1
                };
                if r1 > r2 {
                    continue;
                }
                let remaining = requested_rows as usize - rows_returned;
                let candidate_end = r2.min(r1.saturating_add(remaining as u32).saturating_sub(1));
                let snapshots = tools::range_rows_from_view(
                    workbook,
                    &request.sheet_name,
                    c1,
                    r1,
                    c2,
                    candidate_end,
                    include_formulas || !request.fields.is_empty(),
                    include_styles || !request.fields.is_empty(),
                )
                .map_err(|error| {
                    canonical_error(
                        CanonicalErrorCode::OperationFailed,
                        operation,
                        error.to_string(),
                        None,
                    )
                })?;
                let column_count = (c2 - c1 + 1) as usize;
                let build_block = |count: usize| {
                    let returned_end = r1 + count as u32 - 1;
                    let returned_range = range_string(c1, r1, c2, returned_end);
                    ReadCellsBlock {
                        selection_index: index,
                        requested_range: requested_range.clone(),
                        returned_range: returned_range.clone(),
                        row_count: count,
                        column_count,
                        row_indices: None,
                        payload: range_payload_from_snapshots(
                            encoding,
                            &returned_range,
                            &snapshots[..count],
                            include_formulas,
                            !request.fields.is_empty(),
                        ),
                    }
                };
                let decision = canonical_budget_decision(
                    snapshots.len(),
                    column_count,
                    cells_returned,
                    max_cells,
                    max_payload_bytes,
                    &blocks,
                    build_block,
                );
                if decision.rows == 0 {
                    if !decision.first_row_fits {
                        return Err(canonical_error(
                            CanonicalErrorCode::RowExceedsBudget,
                            operation,
                            format!(
                                "row {r1} cannot fit within the configured read payload budget; narrow the requested range or use inspect_cells for targeted cells"
                            ),
                            Some("$.selection.ranges"),
                        ));
                    }
                    next_cursor = Some(
                        encode_cursor(&ReadCellsCursor {
                            revision_id: revision_id.to_string(),
                            fingerprint: fingerprint.clone(),
                            selection_index: index,
                            next_row: r1,
                        })
                        .map_err(|error| {
                            canonical_error(
                                CanonicalErrorCode::OperationFailed,
                                operation,
                                error.to_string(),
                                None,
                            )
                        })?,
                    );
                    break;
                }
                let returned_end = r1 + decision.rows as u32 - 1;
                blocks.push(build_block(decision.rows));
                rows_returned += decision.rows;
                cells_returned += decision.rows * column_count;
                if returned_end < r2 {
                    next_cursor = Some(
                        encode_cursor(&ReadCellsCursor {
                            revision_id: revision_id.to_string(),
                            fingerprint: fingerprint.clone(),
                            selection_index: index,
                            next_row: returned_end + 1,
                        })
                        .map_err(|error| {
                            canonical_error(
                                CanonicalErrorCode::OperationFailed,
                                operation,
                                error.to_string(),
                                None,
                            )
                        })?,
                    );
                    break;
                }
                if index + 1 < ranges.len() && rows_returned >= requested_rows as usize {
                    let (_, next_row, _, _) =
                        parse_range(&ranges[index + 1]).map_err(|message| {
                            canonical_error(
                                CanonicalErrorCode::InvalidRequest,
                                operation,
                                message,
                                Some("$.selection.ranges"),
                            )
                        })?;
                    next_cursor = Some(
                        encode_cursor(&ReadCellsCursor {
                            revision_id: revision_id.to_string(),
                            fingerprint: fingerprint.clone(),
                            selection_index: index + 1,
                            next_row,
                        })
                        .map_err(|error| {
                            canonical_error(
                                CanonicalErrorCode::OperationFailed,
                                operation,
                                error.to_string(),
                                None,
                            )
                        })?,
                    );
                    break;
                }
            }
        }
        ReadCellsSelection::Rows {
            start_row,
            row_count,
            columns,
            include_header,
        } => {
            if *row_count == 0 {
                return Err(canonical_error(
                    CanonicalErrorCode::InvalidRequest,
                    operation,
                    "selection.row_count must be greater than zero",
                    Some("$.selection.row_count"),
                ));
            }
            let current_row = cursor
                .as_ref()
                .map_or((*start_row).max(1), |value| value.next_row);
            let final_row = start_row
                .max(&1)
                .saturating_add(*row_count)
                .saturating_sub(1);
            let remaining = final_row
                .saturating_sub(current_row)
                .saturating_add(1)
                .min(requested_rows);
            let (letters, headers, header_row) = match columns {
                Some(RowColumnSelection::Letters { values }) => (Some(values.clone()), None, 1),
                Some(RowColumnSelection::Headers { values, header_row }) => {
                    (None, Some(values.clone()), header_row.unwrap_or(1).max(1))
                }
                _ => (None, None, 1),
            };
            let include_header = include_header.unwrap_or(true);
            let (canonical_header, snapshots) = tools::sheet_rows_from_view(
                workbook,
                tools::SheetPageParams {
                    workbook_or_fork_id: workbook_id,
                    sheet_name: request.sheet_name.clone(),
                    start_row: current_row,
                    page_size: remaining,
                    columns: letters,
                    columns_by_header: headers,
                    include_formulas: include_formulas || !request.fields.is_empty(),
                    include_styles: include_styles || !request.fields.is_empty(),
                    include_header,
                    format: Some(SheetPageFormat::Full),
                },
                header_row,
            )
            .map_err(|error| {
                canonical_error(
                    CanonicalErrorCode::OperationFailed,
                    operation,
                    error.to_string(),
                    None,
                )
            })?;
            let column_count = snapshots.first().map_or(0, |row| row.cells.len());
            let requested_range = format!("rows:{start_row}+{row_count}");
            let include_snapshots =
                include_formulas || include_styles || !request.fields.is_empty();
            let include_projection = !request.fields.is_empty()
                || (matches!(encoding.row_format(), SheetPageFormat::Compact)
                    && (include_formulas || include_styles));
            let build_block = |count: usize| {
                let emitted = &snapshots[..count];
                let returned_end = emitted.last().map_or(current_row, |row| row.row_index);
                ReadCellsBlock {
                    selection_index: 0,
                    requested_range: requested_range.clone(),
                    returned_range: format!("rows:{current_row}-{returned_end}"),
                    row_count: count,
                    column_count,
                    row_indices: Some(emitted.iter().map(|row| row.row_index).collect()),
                    payload: rows_payload_from_snapshots(
                        encoding,
                        &canonical_header,
                        emitted,
                        include_header,
                        include_snapshots,
                        include_projection,
                    ),
                }
            };
            let decision = canonical_budget_decision(
                snapshots.len(),
                column_count,
                0,
                max_cells,
                max_payload_bytes,
                &[],
                build_block,
            );
            if decision.rows == 0 && remaining > 0 {
                return Err(canonical_error(
                    CanonicalErrorCode::RowExceedsBudget,
                    operation,
                    format!(
                        "row {current_row} cannot fit within the configured read payload budget; narrow the requested range or use inspect_cells for targeted cells"
                    ),
                    Some("$.selection"),
                ));
            }
            let block = build_block(decision.rows);
            let returned_end = block
                .row_indices
                .as_ref()
                .and_then(|rows| rows.last())
                .copied()
                .unwrap_or(current_row);
            header = canonical_header;
            blocks.push(block);
            rows_returned = decision.rows;
            cells_returned = decision.rows * column_count;
            if returned_end < final_row {
                next_cursor = Some(
                    encode_cursor(&ReadCellsCursor {
                        revision_id: revision_id.to_string(),
                        fingerprint,
                        selection_index: 0,
                        next_row: returned_end + 1,
                    })
                    .map_err(|error| {
                        canonical_error(
                            CanonicalErrorCode::OperationFailed,
                            operation,
                            error.to_string(),
                            None,
                        )
                    })?,
                );
            }
        }
    }
    Ok(ReadCellsData {
        sheet_name: request.sheet_name.clone(),
        selection_kind: match request.selection {
            ReadCellsSelection::Range { .. } => ReadCellsSelectionKind::Range,
            ReadCellsSelection::Rows { .. } => ReadCellsSelectionKind::Rows,
        },
        encoding,
        blocks,
        header,
        calculation,
        page: ReadCellsPage {
            rows_returned,
            cells_returned,
            complete: next_cursor.is_none(),
            next_cursor,
            limits: ReadCellsLimits {
                requested_rows,
                max_cells,
                max_payload_bytes,
            },
        },
        warnings: Vec::new(),
    })
}

pub async fn execute_inspect_cells(
    state: impl crate::read_context::ReadContext,
    request: InspectCellsRequest,
    revision_id: &str,
) -> Result<InspectCellsResponse, CanonicalErrorEnvelope> {
    let operation = "inspect_cells";
    let workbook_id = request.resource_id.to_workbook_id();
    let mut response = tools::inspect_cells_semantic(
        state.clone(),
        tools::InspectCellsParams {
            workbook_or_fork_id: request.resource_id.to_workbook_id(),
            sheet_name: request.sheet_name,
            targets: request.targets,
            include_empty: request.include_empty,
            budget: request.budget,
        },
    )
    .await
    .map_err(|error| {
        canonical_error(
            CanonicalErrorCode::OperationFailed,
            operation,
            error.to_string(),
            None,
        )
    })?;
    response.calculation =
        state.calculation_metadata(&workbook_id, revision_id, response.calculation);
    if response.truncated {
        return Err(canonical_error(
            CanonicalErrorCode::RowExceedsBudget,
            operation,
            "requested inspect_cells details exceed the configured payload budget; narrow the requested ranges or targets; no partial response was returned",
            Some("$.targets"),
        ));
    }
    Ok(response)
}

pub async fn execute_list_workbooks(
    state: impl crate::read_context::ReadContext,
    request: ListWorkbooksRequest,
) -> anyhow::Result<ListWorkbooksData> {
    let response = tools::list_workbooks(
        state,
        tools::ListWorkbooksParams {
            slug_prefix: request.slug_prefix,
            folder: request.folder,
            path_glob: request.path_glob,
            limit: request.limit,
            offset: request.offset,
            include_paths: request.include_paths,
        },
    )
    .await?;
    let workbooks = response
        .workbooks
        .into_iter()
        .map(|workbook| {
            Ok(CanonicalWorkbookDescriptor {
                resource_id: ResourceId::bind_workbook(&workbook.workbook_id)
                    .map_err(anyhow::Error::msg)?,
                metadata: WorkbookDiscoveryMetadata {
                    short_id: workbook.short_id,
                    slug: workbook.slug,
                    bytes: workbook.bytes,
                    last_modified: workbook.last_modified,
                    revision_id: workbook.revision_id,
                },
                paths: (workbook.path.is_some() || workbook.client_path.is_some()).then_some(
                    WorkbookPaths {
                        internal: workbook.path,
                        client: workbook.client_path,
                    },
                ),
                backend_capabilities: workbook.caps,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(ListWorkbooksData {
        workbooks,
        next_offset: response.next_offset,
    })
}

pub async fn execute_describe(
    state: impl crate::read_context::ReadContext,
    request: DescribeWorkbookRequest,
) -> anyhow::Result<DescribeWorkbookData> {
    let workbook_id = request.resource_id.to_workbook_id();
    let description = tools::describe_workbook(
        state.clone(),
        tools::DescribeWorkbookParams {
            workbook_or_fork_id: workbook_id.clone(),
        },
    )
    .await?;
    let summary = if request.include.contains(&DescribeInclude::Summary) {
        let options = request.summary.unwrap_or(DescribeSummaryOptions {
            include_entry_points: None,
            include_named_ranges: None,
        });
        let value = tools::workbook_summary(
            state,
            tools::WorkbookSummaryParams {
                workbook_or_fork_id: workbook_id,
                summary_only: Some(false),
                include_entry_points: options.include_entry_points,
                include_named_ranges: options.include_named_ranges,
            },
        )
        .await?;
        Some(WorkbookDerivedSummary {
            status: CoverageStatus::Complete,
            total_cells: value.total_cells,
            total_formulas: value.total_formulas,
            breakdown: value.breakdown,
            region_counts: value.region_counts,
            key_named_ranges: value.key_named_ranges,
            suggested_entry_points: value.suggested_entry_points,
            notes: value.notes,
            coverage: WorkbookSummaryCoverage {
                sheets_scanned: description.sheet_count,
                sheets_total: description.sheet_count,
                bounded: false,
            },
        })
    } else {
        None
    };
    Ok(DescribeWorkbookData {
        metadata: WorkbookExactMetadata {
            short_id: description.short_id,
            slug: description.slug,
            bytes: description.bytes,
            last_modified: description.last_modified,
            sheet_count: description.sheet_count,
            defined_name_count: description.defined_names,
            table_count: description.tables,
            macros_present: description.macros_present,
        },
        paths: request
            .include_paths
            .unwrap_or(false)
            .then_some(WorkbookPaths {
                internal: description.bytes.is_some().then_some(description.path),
                client: if description.bytes.is_some() { description.client_path } else { None },
            }),
        capabilities: WorkbookCapabilities {
            backend: description.caps,
        },
        summary,
        warnings: Vec::new(),
    })
}

pub async fn execute_search_formulas(
    state: impl crate::read_context::ReadContext,
    request: SearchFormulasRequest,
    revision_id: &str,
) -> Result<SearchFormulasData, CanonicalErrorEnvelope> {
    let operation = "search_formulas";
    if request.query.is_none()
        && request.filter.as_ref().is_none_or(|filter| {
            filter.volatile.is_none()
                && filter.function_names.is_empty()
                && filter.has_external_references.is_none()
        })
    {
        return Err(canonical_error(
            CanonicalErrorCode::InvalidRequest,
            operation,
            "at least one of query or a non-empty filter is required",
            None,
        ));
    }
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
                request.scope,
                request.query,
                request.filter,
                request.result_mode,
                request.group_by,
                request.include_addresses,
                request.addresses_per_group,
                request.include_context,
                request.formula_parse_policy
            )
            .as_bytes()
        )
    );
    let offset = if let Some(value) = request.cursor.as_deref() {
        validate_opaque_cursor(value, "sf1", revision_id, &fingerprint, operation)?.position
    } else {
        request.offset.unwrap_or(0) as usize
    };
    let workbook_id = request.resource_id.to_workbook_id();
    let sheet_name = match request.scope.as_ref() {
        Some(FormulaSearchScope::Sheet { sheet_name, .. }) => Some(sheet_name.clone()),
        _ => None,
    };
    let scoped_range = match request.scope.as_ref() {
        Some(FormulaSearchScope::Sheet {
            range: Some(range), ..
        }) => Some(parse_range(range).map_err(|message| {
            canonical_error(
                CanonicalErrorCode::InvalidRequest,
                operation,
                message,
                Some("$.scope.range"),
            )
        })?),
        _ => None,
    };
    let policy = request
        .formula_parse_policy
        .unwrap_or(FormulaParsePolicy::Warn);
    let mut diagnostics = FormulaParseDiagnosticsBuilder::new(policy);
    let mut raw_matches = Vec::new();
    let mut scan_offset = 0_u32;
    loop {
        let find = tools::find_formula(
            state.clone(),
            tools::FindFormulaParams {
                workbook_or_fork_id: workbook_id.clone(),
                query: String::new(),
                sheet_name: sheet_name.clone(),
                case_sensitive: true,
                include_context: request.include_context.is_some(),
                limit: 500,
                offset: scan_offset,
                context_rows: request.include_context.as_ref().map(|value| value.rows),
                context_cols: request.include_context.as_ref().map(|value| value.columns),
            },
        )
        .await
        .map_err(|error| {
            canonical_error(
                CanonicalErrorCode::OperationFailed,
                operation,
                error.to_string(),
                None,
            )
        })?;
        raw_matches.extend(find.matches);
        if let Some(next) = find.next_offset {
            if next <= scan_offset {
                return Err(canonical_error(
                    CanonicalErrorCode::OperationFailed,
                    operation,
                    "formula scan returned a non-progressing continuation offset",
                    None,
                ));
            }
            scan_offset = next;
        } else {
            break;
        }
    }
    let scanned = raw_matches.len();
    let mut cells = Vec::new();
    for matched in raw_matches {
        if let Some((c1, r1, c2, r2)) = scoped_range {
            let Some((col, row)) = parse_range(&matched.address)
                .ok()
                .map(|(col, row, _, _)| (col, row))
            else {
                continue;
            };
            if col < c1 || col > c2 || row < r1 || row > r2 {
                continue;
            }
        }
        if policy != FormulaParsePolicy::Off
            && let Err(message) = validate_formula(&matched.formula)
        {
            if policy == FormulaParsePolicy::Fail {
                return Err(canonical_error(
                    CanonicalErrorCode::OperationFailed,
                    operation,
                    format!(
                        "formula parse failed at {}!{}: {message}",
                        matched.sheet_name, matched.address
                    ),
                    None,
                ));
            }
            diagnostics.record_error(
                &matched.sheet_name,
                &matched.address,
                &matched.formula,
                &message,
            );
        }
        let functions = formula_functions(&matched.formula);
        let volatile = functions.iter().any(|name| is_volatile_function(name));
        let external = matched.formula.contains('[') && matched.formula.contains(']');
        let query_matches = request
            .query
            .as_ref()
            .is_none_or(|query| formula_query_matches(&matched.formula, query));
        let filter_matches = request.filter.as_ref().is_none_or(|filter| {
            filter.volatile.is_none_or(|wanted| wanted == volatile)
                && (filter.function_names.is_empty()
                    || filter.function_names.iter().all(|wanted| {
                        functions
                            .iter()
                            .any(|actual| actual.eq_ignore_ascii_case(wanted))
                    }))
                && filter
                    .has_external_references
                    .is_none_or(|wanted| wanted == external)
        });
        if query_matches && filter_matches {
            cells.push(FormulaCellMatch {
                sheet_name: matched.sheet_name,
                address: matched.address,
                formula: matched.formula,
                cached_value: matched.cached_value,
                classifications: FormulaClassifications {
                    volatile,
                    functions,
                    external_references: external,
                },
                context: matched.context,
            });
        }
    }
    let total_matches = cells.len();
    let limit = request.limit.unwrap_or(50).clamp(1, 500) as usize;
    let mut groups = Vec::new();
    let mut total_groups = 0_usize;
    if request.result_mode == FormulaResultMode::Groups {
        let mut grouped: BTreeMap<String, Vec<&FormulaCellMatch>> = BTreeMap::new();
        for cell in &cells {
            let keys = match request.group_by.unwrap_or(FormulaGroupBy::Function) {
                FormulaGroupBy::Function if !cell.classifications.functions.is_empty() => {
                    let volatile_only =
                        request.filter.as_ref().and_then(|filter| filter.volatile) == Some(true);
                    let keys = cell
                        .classifications
                        .functions
                        .iter()
                        .filter(|name| !volatile_only || is_volatile_function(name))
                        .cloned()
                        .collect::<Vec<_>>();
                    if keys.is_empty() {
                        vec!["OTHER".to_string()]
                    } else {
                        keys
                    }
                }
                FormulaGroupBy::NormalizedFormula => vec![cell.formula.to_ascii_uppercase()],
                FormulaGroupBy::Fingerprint => {
                    vec![format!("{:x}", Sha256::digest(cell.formula.as_bytes()))]
                }
                FormulaGroupBy::Function => vec!["OTHER".to_string()],
            };
            for key in keys {
                grouped.entry(key).or_default().push(cell);
            }
        }
        let address_limit = request.addresses_per_group.unwrap_or(15) as usize;
        groups = grouped
            .into_iter()
            .map(|(key, values)| {
                let addresses = if request.include_addresses.unwrap_or(true) {
                    values
                        .iter()
                        .take(address_limit)
                        .map(|cell| format!("{}!{}", cell.sheet_name, cell.address))
                        .collect()
                } else {
                    Vec::new()
                };
                FormulaSearchGroup {
                    group_key: key.clone(),
                    formula: matches!(request.group_by, Some(FormulaGroupBy::NormalizedFormula))
                        .then(|| values[0].formula.clone()),
                    fingerprint: matches!(request.group_by, Some(FormulaGroupBy::Fingerprint))
                        .then_some(key),
                    volatile: values.iter().any(|cell| cell.classifications.volatile),
                    functions: values
                        .iter()
                        .flat_map(|cell| cell.classifications.functions.clone())
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                    cell_count: values.len(),
                    addresses_complete: addresses.len() == values.len(),
                    addresses,
                }
            })
            .collect();
        total_groups = groups.len();
        groups = groups.into_iter().skip(offset).take(limit).collect();
        cells.clear();
    } else {
        cells = cells.into_iter().skip(offset).take(limit).collect();
    }
    let returned = if request.result_mode == FormulaResultMode::Groups {
        groups.len()
    } else {
        cells.len()
    };
    let total = if request.result_mode == FormulaResultMode::Groups {
        total_groups
    } else {
        total_matches
    };
    let next_position = (offset + returned < total).then_some(offset + returned);
    let next_cursor = next_position
        .map(|position| {
            encode_opaque_cursor(
                "sf1",
                &CanonicalOpaqueCursor {
                    revision_id: revision_id.to_string(),
                    fingerprint: fingerprint.clone(),
                    position,
                    subposition: 0,
                },
            )
        })
        .transpose()
        .map_err(|error| {
            canonical_error(
                CanonicalErrorCode::OperationFailed,
                operation,
                error.to_string(),
                None,
            )
        })?;
    let formula_parse_diagnostics = if policy == FormulaParsePolicy::Off || diagnostics.is_empty() {
        None
    } else {
        Some(diagnostics.build())
    };
    Ok(SearchFormulasData {
        result_mode: request.result_mode,
        matches: cells,
        groups,
        summary: FormulaSearchSummary {
            formula_cells_scanned: scanned,
            matched_cells: total_matches,
            matched_groups: total_groups,
            scan_complete: true,
        },
        formula_parse_diagnostics,
        next_offset: next_position.map(|value| value as u32),
        next_cursor,
        warnings: Vec::new(),
    })
}

fn formula_query_matches(formula: &str, query: &FormulaQuery) -> bool {
    let (formula, needle) = if query.case_sensitive {
        (formula.to_string(), query.text.clone())
    } else {
        (
            formula.to_ascii_lowercase(),
            query.text.to_ascii_lowercase(),
        )
    };
    match query.match_mode.unwrap_or(MatchMode::Contains) {
        MatchMode::Contains => formula.contains(&needle),
        MatchMode::Exact => formula == needle,
        MatchMode::Prefix => formula.starts_with(&needle),
        MatchMode::Regex => {
            regex::Regex::new(&query.text).is_ok_and(|regex| regex.is_match(formula.as_str()))
        }
    }
}

fn formula_functions(formula: &str) -> Vec<String> {
    let regex =
        regex::Regex::new(r"(?i)([A-Z][A-Z0-9_.]*)\s*\(").expect("static formula function regex");
    regex
        .captures_iter(formula)
        .filter_map(|capture| capture.get(1))
        .map(|name| name.as_str().to_ascii_uppercase())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn is_volatile_function(name: &str) -> bool {
    matches!(
        name,
        "NOW" | "TODAY" | "RAND" | "RANDBETWEEN" | "OFFSET" | "INDIRECT" | "CELL" | "INFO"
    )
}

pub async fn execute_formula_trace(
    state: impl crate::read_context::ReadContext,
    request: FormulaTraceRequest,
    revision_id: &str,
) -> Result<FormulaTraceData, CanonicalErrorEnvelope> {
    let operation = "formula_trace";
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{}|{}|{:?}|{:?}|{:?}|{:?}",
                request.sheet_name,
                request.cell_address,
                request.direction,
                request.depth,
                request
                    .page_size
                    .or(request.limit.map(|value| value as usize)),
                request.formula_parse_policy
            )
            .as_bytes()
        )
    );
    let cursor = request
        .cursor
        .as_deref()
        .map(|value| validate_opaque_cursor(value, "ft1", revision_id, &fingerprint, operation))
        .transpose()?;
    let response = tools::formula_trace_semantic(
        state,
        tools::FormulaTraceParams {
            workbook_or_fork_id: request.resource_id.to_workbook_id(),
            sheet_name: request.sheet_name,
            cell_address: request.cell_address,
            direction: request.direction,
            depth: request.depth,
            limit: request.limit,
            page_size: request.page_size,
            cursor: cursor.map(|value| TraceCursor {
                depth: value.position as u32,
                offset: value.subposition,
            }),
            formula_parse_policy: request.formula_parse_policy,
        },
    )
    .await
    .map_err(|error| {
        canonical_error(
            CanonicalErrorCode::OperationFailed,
            operation,
            error.to_string(),
            None,
        )
    })?;
    let next_cursor = response
        .next_cursor
        .map(|value| {
            encode_opaque_cursor(
                "ft1",
                &CanonicalOpaqueCursor {
                    revision_id: revision_id.to_string(),
                    fingerprint,
                    position: value.depth as usize,
                    subposition: value.offset,
                },
            )
        })
        .transpose()
        .map_err(|error| {
            canonical_error(
                CanonicalErrorCode::OperationFailed,
                operation,
                error.to_string(),
                None,
            )
        })?;
    Ok(FormulaTraceData {
        workbook_id: response.workbook_id,
        sheet_name: response.sheet_name,
        origin: response.origin,
        direction: response.direction,
        layers: response.layers,
        next_cursor,
        formula_parse_diagnostics: response.formula_parse_diagnostics,
        notes: response.notes,
    })
}

pub async fn execute_formula_map(
    state: impl crate::read_context::ReadContext,
    request: FormulaMapRequest,
    revision_id: &str,
) -> Result<FormulaMapData, CanonicalErrorEnvelope> {
    let operation = "formula_map";
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{}|{:?}|{}|{:?}|{:?}|{:?}|{:?}|{:?}",
                request.sheet_name,
                request.range,
                request.expand,
                request.sort_by,
                request.summary_only,
                request.include_addresses,
                request.addresses_limit,
                request.formula_parse_policy
            )
            .as_bytes()
        )
    );
    let offset = request
        .cursor
        .as_deref()
        .map(|value| validate_opaque_cursor(value, "fm1", revision_id, &fingerprint, operation))
        .transpose()?
        .map_or(0, |value| value.position);
    let page_size = request.limit.unwrap_or(50).clamp(1, 500) as usize;
    let response = tools::sheet_formula_map_semantic(
        state,
        tools::SheetFormulaMapParams {
            workbook_or_fork_id: request.resource_id.to_workbook_id(),
            sheet_name: request.sheet_name,
            range: request.range,
            expand: request.expand,
            limit: Some(page_size as u32),
            offset: Some(offset as u32),
            sort_by: request.sort_by,
            summary_only: request.summary_only,
            include_addresses: request.include_addresses,
            addresses_limit: request.addresses_limit,
            formula_parse_policy: request.formula_parse_policy,
        },
    )
    .await
    .map_err(|error| {
        canonical_error(
            CanonicalErrorCode::OperationFailed,
            operation,
            error.to_string(),
            None,
        )
    })?;
    let next_position = response.next_offset.map(|value| value as usize);
    let groups = response.groups;
    let next_cursor = next_position
        .map(|position| {
            encode_opaque_cursor(
                "fm1",
                &CanonicalOpaqueCursor {
                    revision_id: revision_id.to_string(),
                    fingerprint,
                    position,
                    subposition: 0,
                },
            )
        })
        .transpose()
        .map_err(|error| {
            canonical_error(
                CanonicalErrorCode::OperationFailed,
                operation,
                error.to_string(),
                None,
            )
        })?;
    Ok(FormulaMapData {
        workbook_id: response.workbook_id,
        sheet_name: response.sheet_name,
        groups,
        formula_parse_diagnostics: response.formula_parse_diagnostics,
        next_offset: next_position.map(|value| value as u32),
        next_cursor,
    })
}

fn apply_style_limits(
    mut styles: Vec<CanonicalStyleUsage>,
    limits: Option<&AnalyzeStylesLimits>,
) -> (Vec<CanonicalStyleUsage>, u64, u64, bool) {
    let cells_in_scope = styles
        .iter()
        .map(|style| u64::from(style.occurrences))
        .sum::<u64>();
    let scan_limit = limits
        .and_then(|value| value.cells_scanned)
        .map(u64::from)
        .unwrap_or(u64::MAX);
    let mut remaining = scan_limit;
    let mut scan_bounded = false;
    for style in &mut styles {
        let retained = u64::from(style.occurrences).min(remaining) as u32;
        if retained < style.occurrences {
            style.occurrences = retained;
            style.ranges_complete = false;
            scan_bounded = true;
        }
        remaining = remaining.saturating_sub(u64::from(retained));
    }
    styles.retain(|style| style.occurrences > 0);
    let examples_limit = limits
        .and_then(|value| value.examples_per_style)
        .unwrap_or(u32::MAX) as usize;
    let ranges_limit = limits
        .and_then(|value| value.ranges_per_style)
        .unwrap_or(u32::MAX) as usize;
    for style in &mut styles {
        style.example_cells.truncate(examples_limit);
        if style.ranges.len() > ranges_limit {
            style.ranges.truncate(ranges_limit);
            style.ranges_complete = false;
        }
    }
    let styles_limit = limits.and_then(|value| value.styles).unwrap_or(u32::MAX) as usize;
    if styles.len() > styles_limit {
        styles.truncate(styles_limit);
        scan_bounded = true;
    }
    let cells_scanned = cells_in_scope.min(scan_limit);
    (styles, cells_scanned, cells_in_scope, scan_bounded)
}

pub async fn execute_analyze_styles(
    state: impl crate::read_context::ReadContext,
    request: AnalyzeStylesRequest,
) -> anyhow::Result<AnalyzeStylesData> {
    let workbook_id = request.resource_id.to_workbook_id();
    let include_descriptor = request.include.contains(&StyleInclude::Descriptors);
    let include_examples = request.include.contains(&StyleInclude::ExampleCells);
    let include_ranges = request.include.contains(&StyleInclude::Ranges);
    match request.scope {
        AnalyzeStylesScope::Workbook => {
            if request.include.contains(&StyleInclude::Ranges) {
                anyhow::bail!("ranges are only available for sheet scope");
            }
            let max_scan = request
                .limits
                .as_ref()
                .and_then(|limits| limits.cells_scanned);
            let response = tools::workbook_style_summary(
                state,
                tools::WorkbookStyleSummaryParams {
                    workbook_or_fork_id: workbook_id,
                    max_styles: None,
                    max_conditional_formats: None,
                    max_cells_scan: max_scan,
                    summary_only: Some(false),
                    include_descriptor: Some(include_descriptor),
                    include_example_cells: Some(include_examples),
                    include_theme: Some(request.include.contains(&StyleInclude::Theme)),
                    include_conditional_formats: Some(
                        request.include.contains(&StyleInclude::ConditionalFormats),
                    ),
                },
            )
            .await?;
            let scan_truncated = response.scan_truncated;
            let conditional_formats_complete = !response.conditional_formats_truncated;
            let styles = response
                .styles
                .into_iter()
                .map(|style| {
                    let mut examples = style.example_cells;
                    examples.sort();
                    CanonicalStyleUsage {
                        style_id: style.style_id,
                        occurrences: style.occurrences,
                        tags: style.tags,
                        descriptor: style.descriptor,
                        ranges: Vec::new(),
                        ranges_complete: true,
                        example_cells: examples,
                    }
                })
                .collect();
            let (styles, cells_scanned, cells_in_scope, locally_bounded) =
                apply_style_limits(styles, request.limits.as_ref());
            let bounded = scan_truncated || locally_bounded;
            Ok(AnalyzeStylesData {
                scope: AnalyzeStylesScope::Workbook,
                styles,
                theme: response.theme,
                conditional_formats: response.conditional_formats,
                conditional_formats_complete,
                coverage: StyleCoverage {
                    status: if bounded {
                        CoverageStatus::Bounded
                    } else {
                        CoverageStatus::Complete
                    },
                    cells_scanned,
                    cells_in_scope: (!scan_truncated).then_some(cells_in_scope),
                    counts_exact: !bounded,
                },
                warnings: response
                    .notes
                    .into_iter()
                    .map(|message| Warning {
                        code: "STYLE_ANALYSIS_NOTE".to_string(),
                        message,
                    })
                    .collect(),
            })
        }
        AnalyzeStylesScope::Sheet {
            sheet_name,
            selection,
        } => {
            if request.include.contains(&StyleInclude::Theme) {
                anyhow::bail!("theme is only available for workbook scope");
            }
            let response_scope = AnalyzeStylesScope::Sheet {
                sheet_name: sheet_name.clone(),
                selection: selection.clone(),
            };
            let scope = match selection {
                Some(StyleSelection::Range { range }) => {
                    Some(tools::SheetStylesScope::Range { range })
                }
                Some(StyleSelection::Region { region_id }) => {
                    Some(tools::SheetStylesScope::Region { region_id })
                }
                _ => None,
            };
            let bounded_response = tools::sheet_styles_bounded(
                state.clone(),
                tools::SheetStylesParams {
                    workbook_or_fork_id: workbook_id.clone(),
                    sheet_name: sheet_name.clone(),
                    scope,
                    granularity: request.group_by,
                    max_items: None,
                    summary_only: Some(false),
                    include_descriptor: Some(include_descriptor),
                    include_ranges: Some(include_ranges),
                    include_example_cells: Some(include_examples),
                },
                request
                    .limits
                    .as_ref()
                    .and_then(|limits| limits.cells_scanned),
            )
            .await?;
            let (conditional_formats, conditional_formats_complete) =
                if request.include.contains(&StyleInclude::ConditionalFormats) {
                    let summary = tools::workbook_style_summary(
                        state,
                        tools::WorkbookStyleSummaryParams {
                            workbook_or_fork_id: workbook_id,
                            max_styles: Some(0),
                            max_conditional_formats: None,
                            max_cells_scan: Some(0),
                            summary_only: Some(false),
                            include_descriptor: Some(false),
                            include_example_cells: Some(false),
                            include_theme: Some(false),
                            include_conditional_formats: Some(true),
                        },
                    )
                    .await?;
                    let complete = !summary.conditional_formats_truncated;
                    let formats = summary
                        .conditional_formats
                        .into_iter()
                        .filter(|format| format.sheet_name == sheet_name)
                        .collect();
                    (formats, complete)
                } else {
                    (Vec::new(), true)
                };
            let semantic_bounded =
                bounded_response.response.styles_truncated || bounded_response.scan_truncated;
            let cells_scanned = bounded_response.cells_scanned;
            let cells_in_scope = bounded_response.cells_in_scope;
            let styles = bounded_response
                .response
                .styles
                .into_iter()
                .map(|style| {
                    let mut examples = style.example_cells;
                    examples.sort();
                    let mut ranges = style.cell_ranges;
                    ranges.sort();
                    CanonicalStyleUsage {
                        style_id: style.style_id,
                        occurrences: style.occurrences,
                        tags: style.tags,
                        descriptor: style.descriptor,
                        ranges,
                        ranges_complete: !style.ranges_truncated,
                        example_cells: examples,
                    }
                })
                .collect();
            let (styles, _, _, locally_bounded) =
                apply_style_limits(styles, request.limits.as_ref());
            let bounded = semantic_bounded || locally_bounded;
            Ok(AnalyzeStylesData {
                scope: response_scope,
                styles,
                theme: None,
                conditional_formats,
                conditional_formats_complete,
                coverage: StyleCoverage {
                    status: if bounded {
                        CoverageStatus::Bounded
                    } else {
                        CoverageStatus::Complete
                    },
                    cells_scanned,
                    cells_in_scope: Some(cells_in_scope),
                    counts_exact: !bounded,
                },
                warnings: Vec::new(),
            })
        }
    }
}

pub async fn execute_profile_table(
    state: impl crate::read_context::ReadContext,
    mut request: ProfileTableRequest,
) -> Result<ProfileTableData, CanonicalErrorEnvelope> {
    let operation = "profile_table";
    if let Some(range) = request.range.as_deref() {
        let (c1, r1, c2, r2) = parse_range(range).map_err(|message| {
            canonical_error(
                CanonicalErrorCode::InvalidRequest,
                operation,
                message,
                Some("$.range"),
            )
        })?;
        request.range = Some(range_string(c1, r1, c2, r2));
    }
    let sample_mode = request.sample_mode.unwrap_or(SampleMode::Distributed);
    let selector_kind = if request.table_name.is_some() {
        "table"
    } else if request.region_id.is_some() {
        "region"
    } else if request.range.is_some() {
        "range"
    } else {
        "sheet_inferred"
    }
    .to_string();
    let selector_value = request
        .table_name
        .clone()
        .or_else(|| request.region_id.map(|value| value.to_string()))
        .or_else(|| request.range.clone());
    let table_selector = request.table_name.is_some();
    let region_selector = request.region_id.is_some();
    let range_selector = request.range.is_some();
    let resolved = tools::table_profile_semantic(
        state,
        tools::TableProfileParams {
            workbook_or_fork_id: request.resource_id.to_workbook_id(),
            sheet_name: request.sheet_name,
            region_id: request.region_id,
            table_name: request.table_name,
            range: request.range,
            sample_mode: request.sample_mode,
            sample_size: request.sample_size,
            summary_only: request.summary_only,
        },
    )
    .await
    .map_err(|error| {
        canonical_error(
            CanonicalErrorCode::OperationFailed,
            operation,
            error.to_string(),
            None,
        )
    })?;
    let header_provenance = if table_selector && resolved.header_from_selector {
        "table_definition"
    } else if region_selector && resolved.header_from_selector {
        "detected_region"
    } else if region_selector {
        "inferred_region_first_row"
    } else if range_selector {
        "range_first_row"
    } else {
        "inferred_first_row"
    }
    .to_string();
    let bounds = resolved.bounds;
    let header_row = resolved.header_row;
    let rows_scanned = resolved.rows_scanned;
    let response = resolved.response;
    let complete = rows_scanned >= response.row_count;
    Ok(ProfileTableData {
        workbook_id: response.workbook_id,
        sheet_name: response.sheet_name.clone(),
        table_name: response.table_name,
        headers: response.headers,
        column_types: response.column_types,
        row_count: response.row_count,
        samples: response.samples,
        notes: response.notes,
        source: ProfileSource {
            sheet_name: response.sheet_name,
            selector_kind,
            selector_value,
            bounds,
            header_row,
            header_provenance,
        },
        coverage: ProfileCoverage {
            rows_in_scope: response.row_count,
            rows_scanned,
            sample_mode,
            complete,
        },
        confidence: ProfileConfidence {
            status: if complete {
                "observed_complete"
            } else {
                "sampled_heuristic"
            }
            .to_string(),
            heuristic: !complete,
            reason: if complete {
                "all rows in scope were scanned"
            } else {
                "column statistics are inferred from a bounded sample"
            }
            .to_string(),
        },
    })
}

pub async fn execute_export_grid(
    state: impl crate::read_context::ReadContext,
    request: ExportGridRequest,
    revision_id: &str,
) -> Result<ExportGridData, CanonicalErrorEnvelope> {
    let operation = "export_grid";
    let (c1, r1, c2, r2) = parse_range(&request.range).map_err(|message| {
        canonical_error(
            CanonicalErrorCode::InvalidRequest,
            operation,
            message,
            Some("$.range"),
        )
    })?;
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(format!("{}|{}", request.sheet_name, request.range).as_bytes())
    );
    let start_row = if let Some(value) = &request.cursor {
        let cursor = decode_cursor(value).map_err(|message| {
            canonical_error(
                CanonicalErrorCode::InvalidRequest,
                operation,
                message,
                Some("$.cursor"),
            )
        })?;
        if cursor.revision_id != revision_id {
            return Err(canonical_error(
                CanonicalErrorCode::StaleCursor,
                operation,
                "cursor is bound to a different resource revision",
                Some("$.cursor"),
            ));
        }
        if cursor.fingerprint != fingerprint {
            return Err(canonical_error(
                CanonicalErrorCode::CursorMismatch,
                operation,
                "cursor does not match the export_grid request",
                Some("$.cursor"),
            ));
        }
        cursor.next_row
    } else {
        r1
    };
    let end_row = r2.min(
        start_row
            .saturating_add(request.page_size.unwrap_or(500).clamp(1, 5000))
            .saturating_sub(1),
    );
    let returned_range = range_string(c1, start_row, c2, end_row);
    let mut grid = tools::grid_export(
        state,
        tools::GridExportParams {
            workbook_or_fork_id: request.resource_id.to_workbook_id(),
            sheet_name: request.sheet_name,
            range: returned_range.clone(),
        },
    )
    .await
    .map_err(|error| {
        canonical_error(
            CanonicalErrorCode::OperationFailed,
            operation,
            error.to_string(),
            None,
        )
    })?;
    let mut populated = BTreeMap::new();
    for row in std::mem::take(&mut grid.rows) {
        for cell in row.cells {
            populated.insert(cell.offset, cell);
        }
    }
    grid.rows = (0..=end_row - start_row)
        .map(|row_offset| GridRow {
            cells: (0..=c2 - c1)
                .map(|column_offset| {
                    populated
                        .remove(&[row_offset, column_offset])
                        .unwrap_or(GridCell {
                            offset: [row_offset, column_offset],
                            v: None,
                            f: None,
                            fmt: None,
                            style: None,
                        })
                })
                .collect(),
        })
        .collect();
    let next_cursor = if end_row < r2 {
        Some(
            encode_cursor(&ReadCellsCursor {
                revision_id: revision_id.to_string(),
                fingerprint,
                selection_index: 0,
                next_row: end_row + 1,
            })
            .map_err(|error| {
                canonical_error(
                    CanonicalErrorCode::OperationFailed,
                    operation,
                    error.to_string(),
                    None,
                )
            })?,
        )
    } else {
        None
    };
    Ok(ExportGridData {
        fidelity: GridFidelity::CellContentAndExplicitFormatting,
        requested_range: request.range,
        returned_range,
        grid,
        complete: next_cursor.is_none(),
        next_cursor,
    })
}
