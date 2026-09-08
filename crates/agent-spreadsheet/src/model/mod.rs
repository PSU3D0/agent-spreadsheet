use crate::caps::BackendCaps;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod diagnostics;
pub use diagnostics::*;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, Default)]
#[serde(transparent)]
pub struct WorkbookId(pub String);

impl WorkbookId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationState {
    Clean,
    ErrorsFound,
    Partial,
    NotEvaluated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationSource {
    Formualizer,
    TrustedCache,
    None,
}

impl EvaluationSource {
    pub fn is_trusted(self) -> bool {
        matches!(self, Self::Formualizer | Self::TrustedCache)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationFreshness {
    CurrentRevision,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EvaluationCoverage {
    pub formula_cells: u64,
    pub evaluated_formula_cells: u64,
    pub unsupported_formula_cells: u64,
    pub error_formula_cells: u64,
    pub source: EvaluationSource,
    pub freshness: EvaluationFreshness,
    pub revision_id: String,
}

impl EvaluationCoverage {
    pub fn has_valid_counts(&self) -> bool {
        self.evaluated_formula_cells <= self.formula_cells
            && self.unsupported_formula_cells <= self.formula_cells
            && self.error_formula_cells <= self.evaluated_formula_cells
    }

    pub fn state(&self) -> EvaluationState {
        if self.error_formula_cells > 0 {
            EvaluationState::ErrorsFound
        } else if self.formula_cells == 0 {
            EvaluationState::Clean
        } else if self.freshness != EvaluationFreshness::CurrentRevision {
            if self.evaluated_formula_cells > 0 && self.evaluated_formula_cells < self.formula_cells
            {
                EvaluationState::Partial
            } else {
                EvaluationState::NotEvaluated
            }
        } else if self.evaluated_formula_cells == 0 {
            EvaluationState::NotEvaluated
        } else if self.evaluated_formula_cells < self.formula_cells
            || self.unsupported_formula_cells > 0
        {
            EvaluationState::Partial
        } else {
            EvaluationState::Clean
        }
    }

    pub fn is_complete_and_fresh(&self) -> bool {
        self.has_valid_counts()
            && self.freshness == EvaluationFreshness::CurrentRevision
            && self.evaluated_formula_cells == self.formula_cells
            && self.unsupported_formula_cells == 0
            && (self.formula_cells == 0 || self.source.is_trusted())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalculationMetadata {
    pub state: EvaluationState,
    pub revision_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkbookDescriptor {
    pub workbook_id: WorkbookId,
    pub short_id: String,
    pub slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_path: Option<String>,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caps: Option<BackendCaps>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkbookListResponse {
    pub workbooks: Vec<WorkbookDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkbookDescription {
    pub workbook_id: WorkbookId,
    pub short_id: String,
    pub slug: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_path: Option<String>,
    /// Exact matching artifact size, or unknown for an unexported resident revision.
    pub bytes: Option<u64>,
    pub sheet_count: usize,
    pub defined_names: usize,
    pub tables: usize,
    pub macros_present: bool,
    pub last_modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<String>,
    pub caps: BackendCaps,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkbookSummaryResponse {
    pub workbook_id: WorkbookId,
    pub slug: String,
    pub sheet_count: usize,
    pub total_cells: u64,
    pub total_formulas: u64,
    pub breakdown: WorkbookBreakdown,
    pub region_counts: RegionCountSummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_named_ranges: Vec<NamedRangeDescriptor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_entry_points: Vec<EntryPoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct WorkbookBreakdown {
    pub data_sheets: u32,
    pub calculator_sheets: u32,
    pub parameter_sheets: u32,
    pub metadata_sheets: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RegionCountSummary {
    pub data: u32,
    pub parameters: u32,
    pub outputs: u32,
    pub calculator: u32,
    pub metadata: u32,
    pub other: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EntryPoint {
    pub sheet_name: String,
    pub region_id: Option<u32>,
    pub bounds: Option<String>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SheetSummary {
    pub name: String,
    pub visible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub non_empty_cells: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_cells: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_values: Option<u32>,
    pub classification: SheetClassification,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub style_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SheetClassification {
    Data,
    Calculator,
    Mixed,
    Metadata,
    Empty,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SheetListResponse {
    pub workbook_id: WorkbookId,
    pub sheets: Vec<SheetSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SheetOverviewResponse {
    pub workbook_id: WorkbookId,
    pub sheet_name: String,
    pub narrative: String,
    pub regions: Vec<SheetRegion>,
    pub detected_regions: Vec<DetectedRegion>,
    pub detected_region_count: u32,
    pub detected_regions_truncated: bool,
    pub key_ranges: Vec<String>,
    pub formula_ratio: f32,
    pub notable_features: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SheetRegion {
    pub kind: RegionKind,
    pub address: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum RegionKind {
    #[serde(rename = "likely_table")]
    Table,
    #[serde(rename = "likely_data")]
    Data,
    #[serde(rename = "likely_parameters")]
    Parameters,
    #[serde(rename = "likely_outputs")]
    Outputs,
    #[serde(rename = "likely_calculator")]
    Calculator,
    #[serde(rename = "likely_metadata")]
    Metadata,
    #[serde(rename = "likely_styles")]
    Styles,
    #[serde(rename = "likely_comments")]
    Comments,
    #[serde(rename = "unknown")]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DetectedRegion {
    pub id: u32,
    pub bounds: String,
    pub header_row: Option<u32>,
    pub headers: Vec<String>,
    pub header_count: u32,
    pub headers_truncated: bool,
    pub row_count: u32,
    pub classification: RegionKind,
    pub region_kind: Option<RegionKind>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SheetPageResponse {
    pub workbook_id: WorkbookId,
    pub sheet_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<RowSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_start_row: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_row: Option<RowSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compact: Option<SheetPageCompact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values_only: Option<SheetPageValues>,
    pub format: SheetPageFormat,
    /// True when the response was truncated by cell/payload budget limits.
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
    /// Machine-consumable budget/continuation metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<ReadBudget>,
}

/// Machine-consumable output-budget metadata attached to read-surface responses.
///
/// Allows agents to detect truncation deterministically and build continuation
/// requests without guessing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadBudget {
    /// Maximum cells allowed in a single response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_cells: Option<usize>,
    /// Maximum payload bytes allowed in a single response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_payload_bytes: Option<usize>,
    /// Number of rows actually returned.
    pub rows_returned: usize,
    /// Number of cells actually returned.
    pub cells_returned: usize,
    /// Total rows available in the queried range (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_rows_available: Option<u32>,
    /// Human/agent-readable continuation hint (e.g. "use start_row=51 to continue").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation: Option<String>,
}

fn is_false(v: &bool) -> bool {
    !v
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RowSnapshot {
    pub row_index: u32,
    pub cells: Vec<CellSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CellSnapshot {
    pub address: String,
    pub value: Option<CellValue>,
    pub formula: Option<String>,
    pub cached_value: Option<CellValue>,
    pub number_format: Option<String>,
    pub style_tags: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "value")]
pub enum CellValue {
    Text(String),
    Number(f64),
    Bool(bool),
    Error(String),
    Date(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CellValueKind {
    Text,
    Number,
    Bool,
    Error,
    Date,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum CellValuePrimitive {
    Text(String),
    Number(f64),
    Bool(bool),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TableOutputFormat {
    Json,
    Values,
    Csv,
    Dense,
    Rows,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SheetPageFormat {
    #[default]
    Full,
    Compact,
    ValuesOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SheetPageCompact {
    pub headers: Vec<String>,
    pub header_row: Vec<Option<CellValue>>,
    pub rows: Vec<Vec<Option<CellValue>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SheetPageValues {
    pub rows: Vec<Vec<Option<CellValue>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SheetStatisticsResponse {
    pub workbook_id: WorkbookId,
    pub sheet_name: String,
    pub row_count: u32,
    pub column_count: u32,
    pub density: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub numeric_columns: Vec<ColumnSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_columns: Vec<ColumnSummary>,
    pub null_counts: BTreeMap<String, u32>,
    pub duplicate_warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ColumnSummary {
    pub header: Option<String>,
    pub column: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<CellValue>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub mean: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SheetFormulaMapResponse {
    pub workbook_id: WorkbookId,
    pub sheet_name: String,
    pub groups: Vec<FormulaGroup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FormulaGroup {
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    pub formula: String,
    pub is_array: bool,
    pub is_shared: bool,
    pub is_volatile: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FormulaTraceResponse {
    pub workbook_id: WorkbookId,
    pub sheet_name: String,
    pub origin: String,
    pub direction: TraceDirection,
    pub layers: Vec<TraceLayer>,
    pub next_cursor: Option<TraceCursor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FormulaTraceEdge {
    pub from: String,
    pub to: String,
    pub formula: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraceLayer {
    pub depth: u32,
    pub summary: TraceLayerSummary,
    pub highlights: TraceLayerHighlights,
    pub edges: Vec<FormulaTraceEdge>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraceLayerSummary {
    pub total_nodes: usize,
    pub formula_nodes: usize,
    pub value_nodes: usize,
    pub blank_nodes: usize,
    pub external_nodes: usize,
    pub unique_formula_groups: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraceLayerHighlights {
    pub top_ranges: Vec<TraceRangeHighlight>,
    pub top_formula_groups: Vec<TraceFormulaGroupHighlight>,
    pub notable_cells: Vec<TraceCellHighlight>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraceRangeHighlight {
    pub start: String,
    pub end: String,
    pub count: usize,
    pub literals: usize,
    pub formulas: usize,
    pub blanks: usize,
    pub sample_values: Vec<CellValue>,
    pub sample_formulas: Vec<String>,
    pub sample_addresses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraceFormulaGroupHighlight {
    pub fingerprint: String,
    pub formula: String,
    pub count: usize,
    pub sample_addresses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraceCellHighlight {
    pub address: String,
    pub kind: TraceCellKind,
    pub value: Option<CellValue>,
    pub formula: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TraceCellKind {
    Formula,
    Literal,
    Blank,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TraceCursor {
    pub depth: u32,
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceDirection {
    Precedents,
    Dependents,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NamedRangeScope {
    Workbook,
    Sheet,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct NamedRangeDescriptor {
    pub name: String,
    pub scope: Option<String>,
    /// Explicit scope kind: "workbook" or "sheet".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_kind: Option<NamedRangeScope>,
    /// Sheet name when scope_kind is "sheet".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_sheet_name: Option<String>,
    pub refers_to: String,
    pub kind: NamedItemKind,
    pub sheet_name: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NamedItemKind {
    NamedRange,
    Table,
    Formula,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NamedRangesResponse {
    pub workbook_id: WorkbookId,
    pub items: Vec<NamedRangeDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DefineNameResponse {
    pub workbook_id: WorkbookId,
    pub name: String,
    pub refers_to: String,
    pub scope_kind: NamedRangeScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_sheet_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdateNameResponse {
    pub workbook_id: WorkbookId,
    pub name: String,
    pub refers_to: String,
    pub scope_kind: NamedRangeScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_sheet_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_refers_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeleteNameResponse {
    pub workbook_id: WorkbookId,
    pub name: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FindFormulaMatch {
    pub address: String,
    pub sheet_name: String,
    pub formula: String,
    pub cached_value: Option<CellValue>,
    pub context: Vec<RowSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FindFormulaResponse {
    pub workbook_id: WorkbookId,
    pub matches: Vec<FindFormulaMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VolatileScanEntry {
    pub address: String,
    pub sheet_name: String,
    pub function: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VolatileScanResponse {
    pub workbook_id: WorkbookId,
    pub items: Vec<VolatileScanEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula_parse_diagnostics: Option<FormulaParseDiagnostics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct StyleDescriptor {
    pub font: Option<FontDescriptor>,
    pub fill: Option<FillDescriptor>,
    pub borders: Option<BordersDescriptor>,
    pub alignment: Option<AlignmentDescriptor>,
    pub number_format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct FontDescriptor {
    pub name: Option<String>,
    pub size: Option<f64>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<String>,
    pub strikethrough: Option<bool>,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FillDescriptor {
    Pattern(PatternFillDescriptor),
    Gradient(GradientFillDescriptor),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PatternFillDescriptor {
    pub pattern_type: Option<String>,
    pub foreground_color: Option<String>,
    pub background_color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct GradientFillDescriptor {
    pub degree: Option<f64>,
    pub stops: Vec<GradientStopDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GradientStopDescriptor {
    pub position: f64,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct BordersDescriptor {
    pub left: Option<BorderSideDescriptor>,
    pub right: Option<BorderSideDescriptor>,
    pub top: Option<BorderSideDescriptor>,
    pub bottom: Option<BorderSideDescriptor>,
    pub diagonal: Option<BorderSideDescriptor>,
    pub vertical: Option<BorderSideDescriptor>,
    pub horizontal: Option<BorderSideDescriptor>,
    pub diagonal_up: Option<bool>,
    pub diagonal_down: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct BorderSideDescriptor {
    pub style: Option<String>,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AlignmentDescriptor {
    pub horizontal: Option<String>,
    pub vertical: Option<String>,
    pub wrap_text: Option<bool>,
    pub text_rotation: Option<u32>,
}

// Patch variants for write tools (Phase 2+). Double-option fields distinguish:
// - missing field => no change (merge mode)
// - null => clear to default
// - value => set/merge that value
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct StylePatch {
    #[serde(default)]
    pub font: Option<Option<FontPatch>>,
    #[serde(default)]
    pub fill: Option<Option<FillPatch>>,
    #[serde(default)]
    pub borders: Option<Option<BordersPatch>>,
    #[serde(default)]
    pub alignment: Option<Option<AlignmentPatch>>,
    #[serde(default)]
    pub number_format: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct FontPatch {
    #[serde(default)]
    pub name: Option<Option<String>>,
    #[serde(default)]
    pub size: Option<Option<f64>>,
    #[serde(default)]
    pub bold: Option<Option<bool>>,
    #[serde(default)]
    pub italic: Option<Option<bool>>,
    #[serde(default)]
    pub underline: Option<Option<String>>,
    #[serde(default)]
    pub strikethrough: Option<Option<bool>>,
    #[serde(default)]
    pub color: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FillPatch {
    Pattern(PatternFillPatch),
    Gradient(GradientFillPatch),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PatternFillPatch {
    #[serde(default)]
    pub pattern_type: Option<Option<String>>,
    #[serde(default)]
    pub foreground_color: Option<Option<String>>,
    #[serde(default)]
    pub background_color: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct GradientFillPatch {
    #[serde(default)]
    pub degree: Option<Option<f64>>,
    #[serde(default)]
    pub stops: Option<Vec<GradientStopPatch>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GradientStopPatch {
    pub position: f64,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct BordersPatch {
    #[serde(default)]
    pub left: Option<Option<BorderSidePatch>>,
    #[serde(default)]
    pub right: Option<Option<BorderSidePatch>>,
    #[serde(default)]
    pub top: Option<Option<BorderSidePatch>>,
    #[serde(default)]
    pub bottom: Option<Option<BorderSidePatch>>,
    #[serde(default)]
    pub diagonal: Option<Option<BorderSidePatch>>,
    #[serde(default)]
    pub vertical: Option<Option<BorderSidePatch>>,
    #[serde(default)]
    pub horizontal: Option<Option<BorderSidePatch>>,
    #[serde(default)]
    pub diagonal_up: Option<Option<bool>>,
    #[serde(default)]
    pub diagonal_down: Option<Option<bool>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct BorderSidePatch {
    #[serde(default)]
    pub style: Option<Option<String>>,
    #[serde(default)]
    pub color: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AlignmentPatch {
    #[serde(default)]
    pub horizontal: Option<Option<String>>,
    #[serde(default)]
    pub vertical: Option<Option<String>>,
    #[serde(default)]
    pub wrap_text: Option<Option<bool>>,
    #[serde(default)]
    pub text_rotation: Option<Option<u32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SheetStylesResponse {
    pub workbook_id: WorkbookId,
    pub sheet_name: String,
    pub styles: Vec<StyleSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditional_rules: Vec<String>,
    pub total_styles: u32,
    pub styles_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StyleSummary {
    pub style_id: String,
    pub occurrences: u32,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub example_cells: Vec<String>,
    pub descriptor: Option<StyleDescriptor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cell_ranges: Vec<String>,
    pub ranges_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkbookStyleSummaryResponse {
    pub workbook_id: WorkbookId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inferred_default_style_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inferred_default_font: Option<FontDescriptor>,
    pub styles: Vec<WorkbookStyleUsage>,
    pub total_styles: u32,
    pub styles_truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditional_formats: Vec<ConditionalFormatSummary>,
    pub conditional_formats_truncated: bool,
    pub scan_truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkbookStyleUsage {
    pub style_id: String,
    pub occurrences: u32,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub example_cells: Vec<String>,
    pub descriptor: Option<StyleDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ThemeSummary {
    pub name: Option<String>,
    pub colors: BTreeMap<String, String>,
    pub font_scheme: ThemeFontSchemeSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ThemeFontSchemeSummary {
    pub major_latin: Option<String>,
    pub major_east_asian: Option<String>,
    pub major_complex_script: Option<String>,
    pub minor_latin: Option<String>,
    pub minor_east_asian: Option<String>,
    pub minor_complex_script: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConditionalFormatSummary {
    pub sheet_name: String,
    pub range: String,
    pub rule_types: Vec<String>,
    pub rule_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ManifestStubResponse {
    pub workbook_id: WorkbookId,
    pub slug: String,
    pub manifest_yaml: String,
    pub sheets: Vec<ManifestSheetStub>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ManifestSheetStub {
    pub sheet_name: String,
    pub classification: SheetClassification,
    pub candidate_expectations: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum FindMode {
    #[default]
    Value,
    Label,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LabelDirection {
    Right,
    Below,
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FindValueMatch {
    pub address: String,
    pub sheet_name: String,
    pub value: Option<CellValue>,
    pub row_context: Option<RowContext>,
    pub neighbors: Option<NeighborValues>,
    pub label_hit: Option<LabelHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RowContext {
    pub headers: Vec<String>,
    pub values: Vec<Option<CellValue>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NeighborValues {
    pub left: Option<CellValue>,
    pub right: Option<CellValue>,
    pub up: Option<CellValue>,
    pub down: Option<CellValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LabelHit {
    pub label_address: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FindValueResponse {
    pub workbook_id: WorkbookId,
    pub matches: Vec<FindValueMatch>,
    pub match_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u32>,
}

pub type TableRow = BTreeMap<String, Option<CellValue>>;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadTableResponse {
    pub workbook_id: WorkbookId,
    pub calculation: CalculationMetadata,
    pub sheet_name: String,
    pub table_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<TableRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<Vec<Option<CellValuePrimitive>>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<Vec<Option<CellValueKind>>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv: Option<String>,
    pub total_rows: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ColumnTypeSummary {
    pub name: String,
    pub inferred_type: String,
    pub nulls: u32,
    pub distinct: u32,
    pub top_values: Vec<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub mean: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TableProfileResponse {
    pub workbook_id: WorkbookId,
    pub sheet_name: String,
    pub table_name: Option<String>,
    pub headers: Vec<String>,
    pub column_types: Vec<ColumnTypeSummary>,
    pub row_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<TableRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Canonical `range-values` response contract.
///
/// CLI `--shape canonical` uses a `values: Vec<RangeValuesEntry>` envelope whenever
/// at least one range entry is emitted. Because CLI output pruning removes empty arrays,
/// `values` may be omitted when no valid entries remain (for example, fully invalid
/// or unparseable range inputs).
///
/// CLI output keeps this stable top-level shape in both canonical and compact modes.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RangeValuesResponse {
    pub workbook_id: WorkbookId,
    pub calculation: CalculationMetadata,
    pub sheet_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    pub values: Vec<RangeValuesEntry>,
}

/// Per-range payload for `range-values`.
///
/// `range` is the mandatory correlation key in canonical and compact output.
/// `next_start_row` is an optional continuation cursor when output is truncated.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RangeValuesEntry {
    pub range: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<Vec<Option<CellValue>>>>,
    /// Formula text matrix aligned to `rows` when `include_formulas=true`.
    ///
    /// Each entry is `Some(formula_text)` for formula-driven cells and `None`
    /// for literal/non-formula cells.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formulas: Option<Vec<Vec<Option<String>>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<Vec<Option<CellValuePrimitive>>>>,
    /// Dense JSON encoding optimized for agent consumption.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dense: Option<RangeValuesDensePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv: Option<String>,
    /// Row-keyed JSON array: each element maps column letters to values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_keyed: Option<Vec<RangeValuesRowEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_start_row: Option<u32>,
}

/// A single row in the `rows` output format for `range-values`.
///
/// Maps column letters to cell values, giving agents a direct row-by-row
/// mapping without needing to decode dense encoding.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RangeValuesRowEntry {
    /// 1-based row number in the sheet.
    pub row: u32,
    /// Column-letter-keyed cell values (only non-empty cells included).
    pub cells: BTreeMap<String, CellValuePrimitive>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RangeValuesDensePayload {
    /// Encoding contract version.
    pub encoding: String,
    /// Number of columns represented in each dense row.
    pub col_count: u32,
    /// Value dictionary. Index 0 is always null.
    pub dictionary: Vec<Option<CellValuePrimitive>>,
    /// Run-length encoded rows using dictionary indexes.
    pub row_runs: Vec<Vec<RangeValuesDenseRun>>,
    /// Sparse formulas by row/column, included only when requested.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formulas: Vec<RangeValuesDenseFormula>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RangeValuesDenseRun {
    pub value_idx: u32,
    pub len: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RangeValuesDenseFormula {
    /// Zero-based row index within returned rows.
    pub row: u32,
    /// Zero-based column index within returned rows.
    pub col: u32,
    pub formula: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InspectCellsResponse {
    pub workbook_id: WorkbookId,
    pub calculation: CalculationMetadata,
    pub sheet_name: String,
    /// Legacy single-range echo. For multi-target requests this is a comma-joined list.
    pub range: String,
    /// Requested A1 targets when more than one was supplied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
    pub cells: Vec<CellSnapshot>,
    pub truncated: bool,
    /// Machine-consumable budget/continuation metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<ReadBudget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CloseWorkbookResponse {
    pub workbook_id: WorkbookId,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VbaProjectSummaryResponse {
    pub workbook_id: WorkbookId,
    pub has_vba: bool,
    pub code_page: Option<u16>,
    pub sys_kind: Option<String>,
    pub modules: Vec<VbaModuleDescriptor>,
    pub modules_truncated: bool,
    pub references: Vec<VbaReferenceDescriptor>,
    pub references_truncated: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VbaModuleDescriptor {
    pub name: String,
    pub stream_name: String,
    pub doc_string: String,
    pub text_offset: u64,
    pub help_context: u32,
    pub module_type: String,
    pub read_only: bool,
    pub private: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VbaReferenceDescriptor {
    pub kind: String,
    pub debug: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VbaModuleSourceResponse {
    pub workbook_id: WorkbookId,
    pub module_name: String,
    pub offset_lines: u32,
    pub limit_lines: u32,
    pub total_lines: u32,
    pub truncated: bool,
    pub source: String,
}

// ── layout-page ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    #[default]
    Values,
    Formulas,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LayoutRender {
    #[default]
    Json,
    Ascii,
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LayoutPageColumnInfo {
    /// Column letter (e.g., "A")
    pub col: String,
    /// 1-based column index
    pub index: u32,
    /// Column width in Excel character units (capped at max_col_width)
    pub width_chars: f64,
    /// True when no explicit width was set (using the Excel default of 8.43)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_default_width: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LayoutCellBorders {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bottom: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<String>,
}

impl LayoutCellBorders {
    pub fn is_empty(&self) -> bool {
        self.top.is_none() && self.bottom.is_none() && self.left.is_none() && self.right.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LayoutCellInfo {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    /// Explicit horizontal alignment: "left", "center", "right"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align_h: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub borders: Option<LayoutCellBorders>,
    /// True when this cell is the top-left of a merged range
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_start: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LayoutRowInfo {
    pub row: u32,
    pub cells: Vec<LayoutCellInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LayoutPageResponse {
    pub workbook_id: WorkbookId,
    pub sheet_name: String,
    /// The effective range that was rendered
    pub range: String,
    pub columns: Vec<LayoutPageColumnInfo>,
    /// Merged cell ranges that overlap the rendered region (e.g., ["B1:C1"])
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merged_cells: Vec<String>,
    pub rows: Vec<LayoutRowInfo>,
    /// ASCII art render (present when render=ascii or render=both)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ascii_render: Option<String>,
    /// True when the requested range was capped to the row/column limits
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GridPayload {
    pub sheet: String,
    pub anchor: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<GridColumnHint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merges: Vec<String>,
    pub rows: Vec<GridRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GridColumnHint {
    pub offset: u32,
    pub width_chars: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GridRow {
    pub cells: Vec<GridCell>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GridCell {
    pub offset: [u32; 2], // [row_offset, col_offset]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub v: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub f: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fmt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<crate::model::StylePatch>,
}
