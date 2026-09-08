// GENERATED FILE - DO NOT EDIT.
// Source: src/generated/canonical-registry.json
// Regenerate: npm run generate:types (operation types)

/* eslint-disable @typescript-eslint/no-namespace */
export namespace InListWorkbooks {
  export interface Input {
  folder?: (string | null)
  include_paths?: (boolean | null)
  limit?: (number | null)
  offset?: (number | null)
  path_glob?: (string | null)
  slug_prefix?: (string | null)
  }
}

export namespace OutListWorkbooks {
  export type BackendKind = ("xlsx_umya" | "ods_future")
  export type ResourceId = string

  export interface Output {
  data: ListWorkbooksData
  operation: "list_workbooks"
  schema_version: "1"
  }
  export interface ListWorkbooksData {
  next_offset?: (number | null)
  workbooks: CanonicalWorkbookDescriptor[]
  }
  export interface CanonicalWorkbookDescriptor {
  backend_capabilities?: (BackendCaps | null)
  metadata: WorkbookDiscoveryMetadata
  paths?: (WorkbookPaths | null)
  resource_id: ResourceId
  }
  export interface BackendCaps {
  backend: BackendKind
  supports_comments: boolean
  supports_conditional_formatting: boolean
  supports_defined_names: boolean
  supports_formula_graph: boolean
  supports_styles: boolean
  supports_tables: boolean
  }
  export interface WorkbookDiscoveryMetadata {
  bytes: number
  last_modified?: (string | null)
  revision_id?: (string | null)
  short_id: string
  slug: string
  }
  export interface WorkbookPaths {
  client?: (string | null)
  internal?: (string | null)
  }
}

export namespace InDescribeWorkbook {
  export type DescribeInclude = "summary"
  export type ResourceId = string

  export interface Input {
  include?: DescribeInclude[]
  include_paths?: (boolean | null)
  resource_id: ResourceId
  summary?: (DescribeSummaryOptions | null)
  }
  export interface DescribeSummaryOptions {
  include_entry_points?: (boolean | null)
  include_named_ranges?: (boolean | null)
  }
}

export namespace OutDescribeWorkbook {
  export type BackendKind = ("xlsx_umya" | "ods_future")
  export type NamedItemKind = ("named_range" | "table" | "formula" | "unknown")
  export type NamedRangeScope = ("workbook" | "sheet")
  export type CoverageStatus = ("complete" | "bounded")
  export type ResourceId = string

  export interface Output {
  data: DescribeWorkbookData
  operation: "describe_workbook"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface DescribeWorkbookData {
  capabilities: WorkbookCapabilities
  metadata: WorkbookExactMetadata
  paths?: (WorkbookPaths | null)
  summary?: (WorkbookDerivedSummary | null)
  warnings: Warning[]
  }
  export interface WorkbookCapabilities {
  backend: BackendCaps
  }
  export interface BackendCaps {
  backend: BackendKind
  supports_comments: boolean
  supports_conditional_formatting: boolean
  supports_defined_names: boolean
  supports_formula_graph: boolean
  supports_styles: boolean
  supports_tables: boolean
  }
  export interface WorkbookExactMetadata {
  bytes?: (number | null)
  defined_name_count: number
  last_modified?: (string | null)
  macros_present: boolean
  sheet_count: number
  short_id: string
  slug: string
  table_count: number
  }
  export interface WorkbookPaths {
  client?: (string | null)
  internal?: (string | null)
  }
  export interface WorkbookDerivedSummary {
  breakdown: WorkbookBreakdown
  coverage: WorkbookSummaryCoverage
  key_named_ranges: NamedRangeDescriptor[]
  notes: string[]
  region_counts: RegionCountSummary
  status: CoverageStatus
  suggested_entry_points: EntryPoint[]
  total_cells: number
  total_formulas: number
  }
  export interface WorkbookBreakdown {
  calculator_sheets: number
  data_sheets: number
  metadata_sheets: number
  parameter_sheets: number
  }
  export interface WorkbookSummaryCoverage {
  bounded: boolean
  sheets_scanned: number
  sheets_total: number
  }
  export interface NamedRangeDescriptor {
  comment?: (string | null)
  kind: NamedItemKind
  name: string
  refers_to: string
  scope?: (string | null)
  /**
   * Explicit scope kind: "workbook" or "sheet".
   */
  scope_kind?: (NamedRangeScope | null)
  /**
   * Sheet name when scope_kind is "sheet".
   */
  scope_sheet_name?: (string | null)
  sheet_name?: (string | null)
  }
  export interface RegionCountSummary {
  calculator: number
  data: number
  metadata: number
  other: number
  outputs: number
  parameters: number
  }
  export interface EntryPoint {
  bounds?: (string | null)
  rationale: string
  region_id?: (number | null)
  sheet_name: string
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InListSheets {
  export type ResourceId = string

  export interface Input {
  include_bounds?: (boolean | null)
  limit?: (number | null)
  offset?: (number | null)
  resource_id: ResourceId
  }
}

export namespace OutListSheets {
  export type SheetClassification = ("data" | "calculator" | "mixed" | "metadata" | "empty")
  export type ResourceId = string

  export interface Output {
  data: SheetListResponse
  operation: "list_sheets"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface SheetListResponse {
  next_offset?: (number | null)
  sheets: SheetSummary[]
  workbook_id: string
  }
  export interface SheetSummary {
  cached_values?: (number | null)
  classification: SheetClassification
  column_count?: (number | null)
  formula_cells?: (number | null)
  name: string
  non_empty_cells?: (number | null)
  row_count?: (number | null)
  style_tags?: string[]
  visible: boolean
  }
}

export namespace InSheetOverview {
  export type ResourceId = string

  export interface Input {
  include_headers?: (boolean | null)
  max_headers?: (number | null)
  max_regions?: (number | null)
  resource_id: ResourceId
  sheet_name: string
  }
}

export namespace OutSheetOverview {
  export type RegionKind = ("likely_table" | "likely_data" | "likely_parameters" | "likely_outputs" | "likely_calculator" | "likely_metadata" | "likely_styles" | "likely_comments" | "unknown")
  export type ResourceId = string

  export interface Output {
  data: SheetOverviewResponse
  operation: "sheet_overview"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface SheetOverviewResponse {
  detected_region_count: number
  detected_regions: DetectedRegion[]
  detected_regions_truncated: boolean
  formula_ratio: number
  key_ranges: string[]
  narrative: string
  notable_features: string[]
  notes: string[]
  regions: SheetRegion[]
  sheet_name: string
  workbook_id: string
  }
  export interface DetectedRegion {
  bounds: string
  classification: RegionKind
  confidence: number
  header_count: number
  header_row?: (number | null)
  headers: string[]
  headers_truncated: boolean
  id: number
  region_kind?: (RegionKind | null)
  row_count: number
  }
  export interface SheetRegion {
  address: string
  description: string
  kind: RegionKind
  }
}

export namespace InReadCells {
  export type ReadCellsFormat = ("dense" | "values" | "csv" | "json" | "rows" | "full" | "compact" | "values_only")
  export type ReadCellField = ("value" | "formula" | "cached_value" | "stored_kind" | "number_format" | "style_tags")
  export type ResourceId = string
  export type ReadCellsSelection = ({
  include_headers?: (boolean | null)
  kind: "range"
  ranges: string[]
  } | {
  columns?: (RowColumnSelection | null)
  include_header?: (boolean | null)
  kind: "rows"
  row_count: number
  start_row?: number
  })
  export type RowColumnSelection = ({
  kind: "all"
  } | {
  kind: "letters"
  values: string[]
  } | {
  header_row?: (number | null)
  kind: "headers"
  values: string[]
  })

  export interface Input {
  cursor?: (string | null)
  encoding?: (ReadCellsFormat | null)
  fields?: ReadCellField[]
  format?: (ReadCellsFormat | null)
  include_formulas?: (boolean | null)
  include_styles?: (boolean | null)
  page?: (CanonicalReadPage | null)
  page_size?: (number | null)
  resource_id: ResourceId
  selection: ReadCellsSelection
  sheet_name: string
  }
  export interface CanonicalReadPage {
  cursor?: (string | null)
  limit_rows?: (number | null)
  }
}

export namespace OutReadCells {
  export type CellValue = ({
  kind: "Text"
  value: string
  } | {
  kind: "Number"
  value: number
  } | {
  kind: "Bool"
  value: boolean
  } | {
  kind: "Error"
  value: string
  } | {
  kind: "Date"
  value: string
  })
  export type CellValuePrimitive = (string | number | boolean)
  export type ReadCellsFormat = ("dense" | "values" | "csv" | "json" | "rows" | "full" | "compact" | "values_only")
  export type CanonicalStoredKind = ("formula" | "text" | "number" | "bool" | "error" | "date" | "blank")
  export type EvaluationState = ("clean" | "errors_found" | "partial" | "not_evaluated")
  export type ReadCellsSelectionKind = ("range" | "rows")
  export type ResourceId = string

  export interface Output {
  data: ReadCellsData
  operation: "read_cells"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface ReadCellsData {
  blocks: ReadCellsBlock[]
  calculation: CalculationMetadata
  encoding: ReadCellsFormat
  header?: (RowSnapshot | null)
  page: ReadCellsPage
  selection_kind: ReadCellsSelectionKind
  sheet_name: string
  warnings: Warning[]
  }
  export interface ReadCellsBlock {
  column_count: number
  payload: ReadCellsPayload
  requested_range: string
  returned_range: string
  row_count: number
  row_indices?: (number[] | null)
  selection_index: number
  }
  export interface ReadCellsPayload {
  compact?: (SheetPageCompact | null)
  csv?: (string | null)
  dense?: (RangeValuesDensePayload | null)
  encoding: ReadCellsFormat
  formulas?: ((string | null)[][] | null)
  projected?: (CanonicalCellProjection[][] | null)
  rows?: ((CellValue | null)[][] | null)
  rows_keyed?: (RangeValuesRowEntry[] | null)
  snapshots?: (RowSnapshot[] | null)
  values?: ((CellValuePrimitive | null)[][] | null)
  values_only?: (SheetPageValues | null)
  }
  export interface SheetPageCompact {
  header_row: (CellValue | null)[]
  headers: string[]
  rows: (CellValue | null)[][]
  }
  export interface RangeValuesDensePayload {
  /**
   * Number of columns represented in each dense row.
   */
  col_count: number
  /**
   * Value dictionary. Index 0 is always null.
   */
  dictionary: (CellValuePrimitive | null)[]
  /**
   * Encoding contract version.
   */
  encoding: string
  /**
   * Sparse formulas by row/column, included only when requested.
   */
  formulas?: RangeValuesDenseFormula[]
  /**
   * Run-length encoded rows using dictionary indexes.
   */
  row_runs: RangeValuesDenseRun[][]
  }
  export interface RangeValuesDenseFormula {
  /**
   * Zero-based column index within returned rows.
   */
  col: number
  formula: string
  /**
   * Zero-based row index within returned rows.
   */
  row: number
  }
  export interface RangeValuesDenseRun {
  len: number
  value_idx: number
  }
  export interface CanonicalCellProjection {
  address: string
  cached_value?: (CellValue | null)
  formula?: (string | null)
  number_format?: (string | null)
  stored_kind: CanonicalStoredKind
  style_tags: string[]
  value?: (CellValue | null)
  }
  /**
   * A single row in the `rows` output format for `range-values`.
   * 
   * Maps column letters to cell values, giving agents a direct row-by-row
   * mapping without needing to decode dense encoding.
   */
  export interface RangeValuesRowEntry {
  /**
   * Column-letter-keyed cell values (only non-empty cells included).
   */
  cells: {
  [k: string]: CellValuePrimitive
  }
  /**
   * 1-based row number in the sheet.
   */
  row: number
  }
  export interface RowSnapshot {
  cells: CellSnapshot[]
  row_index: number
  }
  export interface CellSnapshot {
  address: string
  cached_value?: (CellValue | null)
  formula?: (string | null)
  notes: string[]
  number_format?: (string | null)
  style_tags: string[]
  value?: (CellValue | null)
  }
  export interface SheetPageValues {
  rows: (CellValue | null)[][]
  }
  export interface CalculationMetadata {
  revision_id: string
  state: EvaluationState
  }
  export interface ReadCellsPage {
  cells_returned: number
  complete: boolean
  limits: ReadCellsLimits
  next_cursor?: (string | null)
  rows_returned: number
  }
  export interface ReadCellsLimits {
  max_cells?: (number | null)
  max_payload_bytes?: (number | null)
  requested_rows: number
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InInspectCells {
  export type ResourceId = string

  export interface Input {
  budget?: (number | null)
  include_empty?: (boolean | null)
  resource_id: ResourceId
  sheet_name: string
  targets: string[]
  }
}

export namespace OutInspectCells {
  export type EvaluationState = ("clean" | "errors_found" | "partial" | "not_evaluated")
  export type CellValue = ({
  kind: "Text"
  value: string
  } | {
  kind: "Number"
  value: number
  } | {
  kind: "Bool"
  value: boolean
  } | {
  kind: "Error"
  value: string
  } | {
  kind: "Date"
  value: string
  })
  export type ResourceId = string

  export interface Output {
  data: InspectCellsResponse
  operation: "inspect_cells"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface InspectCellsResponse {
  /**
   * Machine-consumable budget/continuation metadata.
   */
  budget?: (ReadBudget | null)
  calculation: CalculationMetadata
  cells: CellSnapshot[]
  /**
   * Legacy single-range echo. For multi-target requests this is a comma-joined list.
   */
  range: string
  sheet_name: string
  /**
   * Requested A1 targets when more than one was supplied.
   */
  targets?: string[]
  truncated: boolean
  workbook_id: string
  }
  /**
   * Machine-consumable output-budget metadata attached to read-surface responses.
   * 
   * Allows agents to detect truncation deterministically and build continuation
   * requests without guessing.
   */
  export interface ReadBudget {
  /**
   * Number of cells actually returned.
   */
  cells_returned: number
  /**
   * Human/agent-readable continuation hint (e.g. "use start_row=51 to continue").
   */
  continuation?: (string | null)
  /**
   * Maximum cells allowed in a single response.
   */
  max_cells?: (number | null)
  /**
   * Maximum payload bytes allowed in a single response.
   */
  max_payload_bytes?: (number | null)
  /**
   * Number of rows actually returned.
   */
  rows_returned: number
  /**
   * Total rows available in the queried range (if known).
   */
  total_rows_available?: (number | null)
  }
  export interface CalculationMetadata {
  revision_id: string
  state: EvaluationState
  }
  export interface CellSnapshot {
  address: string
  cached_value?: (CellValue | null)
  formula?: (string | null)
  notes: string[]
  number_format?: (string | null)
  style_tags: string[]
  value?: (CellValue | null)
  }
}

export namespace InReadTable {
  /**
   * Filter operators for table queries
   */
  export type FilterOp = ("eq" | "neq" | "gt" | "lt" | "gte" | "lte" | "contains" | "starts_with" | "ends_with" | "in")
  export type TableOutputFormat = ("json" | "values" | "csv" | "dense" | "rows")
  export type ResourceId = string
  /**
   * Sampling mode for table reads
   */
  export type SampleMode = ("first" | "last" | "distributed")

  export interface Input {
  columns?: (string[] | null)
  filters?: (CanonicalTableFilter[] | null)
  format?: (TableOutputFormat | null)
  header_row?: (number | null)
  header_rows?: (number | null)
  include_headers?: (boolean | null)
  include_types?: (boolean | null)
  limit?: (number | null)
  offset?: (number | null)
  range?: (string | null)
  region_id?: (number | null)
  resource_id: ResourceId
  sample_mode?: (SampleMode | null)
  sheet_name?: (string | null)
  table_name?: (string | null)
  }
  export interface CanonicalTableFilter {
  column: string
  op: FilterOp
  value: unknown
  }
}

export namespace OutReadTable {
  export type EvaluationState = ("clean" | "errors_found" | "partial" | "not_evaluated")
  export type CellValue = ({
  kind: "Text"
  value: string
  } | {
  kind: "Number"
  value: number
  } | {
  kind: "Bool"
  value: boolean
  } | {
  kind: "Error"
  value: string
  } | {
  kind: "Date"
  value: string
  })
  export type CellValueKind = ("text" | "number" | "bool" | "error" | "date")
  export type CellValuePrimitive = (string | number | boolean)
  export type ResourceId = string

  export interface Output {
  data: ReadTableResponse
  operation: "read_table"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface ReadTableResponse {
  calculation: CalculationMetadata
  csv?: (string | null)
  headers?: string[]
  next_offset?: (number | null)
  rows?: {
  [k: string]: (CellValue | null)
  }[]
  sheet_name: string
  table_name?: (string | null)
  total_rows: number
  types?: ((CellValueKind | null)[][] | null)
  values?: ((CellValuePrimitive | null)[][] | null)
  warnings?: Warning[]
  workbook_id: string
  }
  export interface CalculationMetadata {
  revision_id: string
  state: EvaluationState
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InReadLayout {
  export type LayoutMode = ("values" | "formulas")
  export type LayoutRender = ("json" | "ascii" | "both")
  export type ResourceId = string

  export interface Input {
  fit_columns?: (boolean | null)
  max_col_width?: (number | null)
  mode?: (LayoutMode | null)
  range?: (string | null)
  render?: (LayoutRender | null)
  resource_id: ResourceId
  sheet_name: string
  trim_empty_columns?: (boolean | null)
  }
}

export namespace OutReadLayout {
  export type LayoutLossiness = "lossy"
  export type ResourceId = string

  export interface Output {
  data: ReadLayoutData
  operation: "read_layout"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface ReadLayoutData {
  layout: LayoutPageResponse
  lossiness: LayoutLossiness
  }
  export interface LayoutPageResponse {
  /**
   * ASCII art render (present when render=ascii or render=both)
   */
  ascii_render?: (string | null)
  columns: LayoutPageColumnInfo[]
  /**
   * Merged cell ranges that overlap the rendered region (e.g., ["B1:C1"])
   */
  merged_cells?: string[]
  notes?: string[]
  /**
   * The effective range that was rendered
   */
  range: string
  rows: LayoutRowInfo[]
  sheet_name: string
  /**
   * True when the requested range was capped to the row/column limits
   */
  truncated?: boolean
  workbook_id: string
  }
  export interface LayoutPageColumnInfo {
  /**
   * Column letter (e.g., "A")
   */
  col: string
  /**
   * 1-based column index
   */
  index: number
  /**
   * True when no explicit width was set (using the Excel default of 8.43)
   */
  is_default_width?: boolean
  /**
   * Column width in Excel character units (capped at max_col_width)
   */
  width_chars: number
  }
  export interface LayoutRowInfo {
  cells: LayoutCellInfo[]
  row: number
  }
  export interface LayoutCellInfo {
  address: string
  /**
   * Explicit horizontal alignment: "left", "center", "right"
   */
  align_h?: (string | null)
  bold?: (boolean | null)
  borders?: (LayoutCellBorders | null)
  italic?: (boolean | null)
  /**
   * True when this cell is the top-left of a merged range
   */
  merge_start?: (boolean | null)
  value?: (string | null)
  }
  export interface LayoutCellBorders {
  bottom?: (string | null)
  left?: (string | null)
  right?: (string | null)
  top?: (string | null)
  }
}

export namespace InExportGrid {
  export type ResourceId = string

  export interface Input {
  cursor?: (string | null)
  page_size?: (number | null)
  range: string
  resource_id: ResourceId
  sheet_name: string
  }
}

export namespace OutExportGrid {
  export type GridFidelity = "cell_content_and_explicit_formatting"
  export type FillPatch = ({
  background_color?: (string | null)
  foreground_color?: (string | null)
  kind: "pattern"
  pattern_type?: (string | null)
  } | {
  degree?: (number | null)
  kind: "gradient"
  stops?: (GradientStopPatch[] | null)
  })
  export type ResourceId = string

  export interface Output {
  data: ExportGridData
  operation: "export_grid"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface ExportGridData {
  complete: boolean
  fidelity: GridFidelity
  grid: GridPayload
  next_cursor?: (string | null)
  requested_range: string
  returned_range: string
  }
  export interface GridPayload {
  anchor: string
  columns?: GridColumnHint[]
  merges?: string[]
  rows: GridRow[]
  sheet: string
  }
  export interface GridColumnHint {
  offset: number
  width_chars: number
  }
  export interface GridRow {
  cells: GridCell[]
  }
  export interface GridCell {
  f?: (string | null)
  fmt?: (string | null)
  /**
   * @minItems 2
   * @maxItems 2
   */
  offset: [number, number]
  style?: (StylePatch | null)
  v?: unknown
  }
  export interface StylePatch {
  alignment?: (AlignmentPatch | null)
  borders?: (BordersPatch | null)
  fill?: (FillPatch | null)
  font?: (FontPatch | null)
  number_format?: (string | null)
  }
  export interface AlignmentPatch {
  horizontal?: (string | null)
  text_rotation?: (number | null)
  vertical?: (string | null)
  wrap_text?: (boolean | null)
  }
  export interface BordersPatch {
  bottom?: (BorderSidePatch | null)
  diagonal?: (BorderSidePatch | null)
  diagonal_down?: (boolean | null)
  diagonal_up?: (boolean | null)
  horizontal?: (BorderSidePatch | null)
  left?: (BorderSidePatch | null)
  right?: (BorderSidePatch | null)
  top?: (BorderSidePatch | null)
  vertical?: (BorderSidePatch | null)
  }
  export interface BorderSidePatch {
  color?: (string | null)
  style?: (string | null)
  }
  export interface GradientStopPatch {
  color: string
  position: number
  }
  export interface FontPatch {
  bold?: (boolean | null)
  color?: (string | null)
  italic?: (boolean | null)
  name?: (string | null)
  size?: (number | null)
  strikethrough?: (boolean | null)
  underline?: (string | null)
  }
}

export namespace InNamedRanges {
  export type ResourceId = string

  export interface Input {
  name_prefix?: (string | null)
  resource_id: ResourceId
  sheet_name?: (string | null)
  }
}

export namespace OutNamedRanges {
  export type NamedItemKind = ("named_range" | "table" | "formula" | "unknown")
  export type NamedRangeScope = ("workbook" | "sheet")
  export type ResourceId = string

  export interface Output {
  data: NamedRangesResponse
  operation: "named_ranges"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface NamedRangesResponse {
  items: NamedRangeDescriptor[]
  workbook_id: string
  }
  export interface NamedRangeDescriptor {
  comment?: (string | null)
  kind: NamedItemKind
  name: string
  refers_to: string
  scope?: (string | null)
  /**
   * Explicit scope kind: "workbook" or "sheet".
   */
  scope_kind?: (NamedRangeScope | null)
  /**
   * Sheet name when scope_kind is "sheet".
   */
  scope_sheet_name?: (string | null)
  sheet_name?: (string | null)
  }
}

export namespace InAnalyzeStyles {
  /**
   * Granularity for style analysis
   */
  export type StyleGranularity = ("runs" | "cells")
  export type StyleInclude = ("descriptors" | "ranges" | "example_cells" | "theme" | "conditional_formats")
  export type ResourceId = string
  export type AnalyzeStylesScope = ({
  kind: "workbook"
  } | {
  kind: "sheet"
  selection?: (StyleSelection | null)
  sheet_name: string
  })
  export type StyleSelection = ({
  kind: "all"
  } | {
  kind: "range"
  range: string
  } | {
  kind: "region"
  region_id: number
  })

  export interface Input {
  group_by?: (StyleGranularity | null)
  include?: StyleInclude[]
  limits?: (AnalyzeStylesLimits | null)
  resource_id: ResourceId
  scope: AnalyzeStylesScope
  }
  export interface AnalyzeStylesLimits {
  cells_scanned?: (number | null)
  examples_per_style?: (number | null)
  ranges_per_style?: (number | null)
  styles?: (number | null)
  }
}

export namespace OutAnalyzeStyles {
  export type CoverageStatus = ("complete" | "bounded")
  export type AnalyzeStylesScope = ({
  kind: "workbook"
  } | {
  kind: "sheet"
  selection?: (StyleSelection | null)
  sheet_name: string
  })
  export type StyleSelection = ({
  kind: "all"
  } | {
  kind: "range"
  range: string
  } | {
  kind: "region"
  region_id: number
  })
  export type FillDescriptor = ({
  background_color?: (string | null)
  foreground_color?: (string | null)
  kind: "pattern"
  pattern_type?: (string | null)
  } | {
  degree?: (number | null)
  kind: "gradient"
  stops: GradientStopDescriptor[]
  })
  export type ResourceId = string

  export interface Output {
  data: AnalyzeStylesData
  operation: "analyze_styles"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface AnalyzeStylesData {
  conditional_formats: ConditionalFormatSummary[]
  conditional_formats_complete: boolean
  coverage: StyleCoverage
  scope: AnalyzeStylesScope
  styles: CanonicalStyleUsage[]
  theme?: (ThemeSummary | null)
  warnings: Warning[]
  }
  export interface ConditionalFormatSummary {
  range: string
  rule_count: number
  rule_types: string[]
  sheet_name: string
  }
  export interface StyleCoverage {
  cells_in_scope?: (number | null)
  cells_scanned: number
  counts_exact: boolean
  status: CoverageStatus
  }
  export interface CanonicalStyleUsage {
  descriptor?: (StyleDescriptor | null)
  example_cells: string[]
  occurrences: number
  ranges: string[]
  ranges_complete: boolean
  style_id: string
  tags: string[]
  }
  export interface StyleDescriptor {
  alignment?: (AlignmentDescriptor | null)
  borders?: (BordersDescriptor | null)
  fill?: (FillDescriptor | null)
  font?: (FontDescriptor | null)
  number_format?: (string | null)
  }
  export interface AlignmentDescriptor {
  horizontal?: (string | null)
  text_rotation?: (number | null)
  vertical?: (string | null)
  wrap_text?: (boolean | null)
  }
  export interface BordersDescriptor {
  bottom?: (BorderSideDescriptor | null)
  diagonal?: (BorderSideDescriptor | null)
  diagonal_down?: (boolean | null)
  diagonal_up?: (boolean | null)
  horizontal?: (BorderSideDescriptor | null)
  left?: (BorderSideDescriptor | null)
  right?: (BorderSideDescriptor | null)
  top?: (BorderSideDescriptor | null)
  vertical?: (BorderSideDescriptor | null)
  }
  export interface BorderSideDescriptor {
  color?: (string | null)
  style?: (string | null)
  }
  export interface GradientStopDescriptor {
  color: string
  position: number
  }
  export interface FontDescriptor {
  bold?: (boolean | null)
  color?: (string | null)
  italic?: (boolean | null)
  name?: (string | null)
  size?: (number | null)
  strikethrough?: (boolean | null)
  underline?: (string | null)
  }
  export interface ThemeSummary {
  colors: {
  [k: string]: string
  }
  font_scheme: ThemeFontSchemeSummary
  name?: (string | null)
  }
  export interface ThemeFontSchemeSummary {
  major_complex_script?: (string | null)
  major_east_asian?: (string | null)
  major_latin?: (string | null)
  minor_complex_script?: (string | null)
  minor_east_asian?: (string | null)
  minor_latin?: (string | null)
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InSearchValues {
  /**
   * Context to include with find_value matches
   */
  export type FindContext = ("none" | "neighbors" | "row" | "both")
  export type LabelDirection = ("right" | "below" | "any")
  /**
   * Match mode for text searches
   */
  export type MatchMode = ("contains" | "exact" | "prefix" | "regex")
  export type FindMode = ("value" | "label")
  export type ResourceId = string
  /**
   * Cell value types for filtering
   */
  export type ValueTypeFilter = ("text" | "number" | "bool" | "date" | "null")

  export interface Input {
  case_sensitive?: boolean
  context?: (FindContext | null)
  context_width?: (number | null)
  direction?: (LabelDirection | null)
  label?: (string | null)
  limit?: number
  match_mode?: (MatchMode | null)
  mode?: (FindMode | null)
  offset?: (number | null)
  query: string
  region_id?: (number | null)
  resource_id: ResourceId
  search_headers_only?: boolean
  sheet_name?: (string | null)
  table_name?: (string | null)
  value_types?: (ValueTypeFilter[] | null)
  }
}

export namespace OutSearchValues {
  export type CellValue = ({
  kind: "Text"
  value: string
  } | {
  kind: "Number"
  value: number
  } | {
  kind: "Bool"
  value: boolean
  } | {
  kind: "Error"
  value: string
  } | {
  kind: "Date"
  value: string
  })
  export type ResourceId = string

  export interface Output {
  data: FindValueResponse
  operation: "search_values"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface FindValueResponse {
  match_count: number
  matches: FindValueMatch[]
  next_offset?: (number | null)
  workbook_id: string
  }
  export interface FindValueMatch {
  address: string
  label_hit?: (LabelHit | null)
  neighbors?: (NeighborValues | null)
  row_context?: (RowContext | null)
  sheet_name: string
  value?: (CellValue | null)
  }
  export interface LabelHit {
  label: string
  label_address: string
  }
  export interface NeighborValues {
  down?: (CellValue | null)
  left?: (CellValue | null)
  right?: (CellValue | null)
  up?: (CellValue | null)
  }
  export interface RowContext {
  headers: string[]
  values: (CellValue | null)[]
  }
}

export namespace InSearchFormulas {
  export type FormulaParsePolicy = ("fail" | "warn" | "off")
  export type FormulaGroupBy = ("function" | "normalized_formula" | "fingerprint")
  /**
   * Match mode for text searches
   */
  export type MatchMode = ("contains" | "exact" | "prefix" | "regex")
  export type ResourceId = string
  export type FormulaResultMode = ("cells" | "groups")
  export type FormulaSearchScope = ({
  kind: "workbook"
  } | {
  kind: "sheet"
  range?: (string | null)
  sheet_name: string
  })

  export interface Input {
  addresses_per_group?: (number | null)
  cursor?: (string | null)
  filter?: (FormulaFilter | null)
  formula_parse_policy?: (FormulaParsePolicy | null)
  group_by?: (FormulaGroupBy | null)
  include_addresses?: (boolean | null)
  include_context?: (FormulaContextRequest | null)
  limit?: (number | null)
  offset?: (number | null)
  query?: (FormulaQuery | null)
  resource_id: ResourceId
  result_mode: FormulaResultMode
  scope?: (FormulaSearchScope | null)
  }
  export interface FormulaFilter {
  function_names?: string[]
  has_external_references?: (boolean | null)
  volatile?: (boolean | null)
  }
  export interface FormulaContextRequest {
  columns: number
  rows: number
  }
  export interface FormulaQuery {
  case_sensitive?: boolean
  match_mode?: (MatchMode | null)
  text: string
  }
}

export namespace OutSearchFormulas {
  export type FormulaParsePolicy = ("fail" | "warn" | "off")
  export type CellValue = ({
  kind: "Text"
  value: string
  } | {
  kind: "Number"
  value: number
  } | {
  kind: "Bool"
  value: boolean
  } | {
  kind: "Error"
  value: string
  } | {
  kind: "Date"
  value: string
  })
  export type FormulaResultMode = ("cells" | "groups")
  export type ResourceId = string

  export interface Output {
  data: SearchFormulasData
  operation: "search_formulas"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface SearchFormulasData {
  formula_parse_diagnostics?: (FormulaParseDiagnostics | null)
  groups: FormulaSearchGroup[]
  matches: FormulaCellMatch[]
  next_cursor?: (string | null)
  next_offset?: (number | null)
  result_mode: FormulaResultMode
  summary: FormulaSearchSummary
  warnings: Warning[]
  }
  export interface FormulaParseDiagnostics {
  groups: FormulaParseErrorGroup[]
  groups_truncated: boolean
  policy: FormulaParsePolicy
  total_errors: number
  }
  export interface FormulaParseErrorGroup {
  count: number
  error_code: string
  error_message: string
  formula_preview: string
  sample_addresses: string[]
  sheet_name: string
  }
  export interface FormulaSearchGroup {
  addresses: string[]
  addresses_complete: boolean
  cell_count: number
  fingerprint?: (string | null)
  formula?: (string | null)
  functions: string[]
  group_key: string
  volatile: boolean
  }
  export interface FormulaCellMatch {
  address: string
  cached_value?: (CellValue | null)
  classifications: FormulaClassifications
  context: RowSnapshot[]
  formula: string
  sheet_name: string
  }
  export interface FormulaClassifications {
  external_references: boolean
  functions: string[]
  volatile: boolean
  }
  export interface RowSnapshot {
  cells: CellSnapshot[]
  row_index: number
  }
  export interface CellSnapshot {
  address: string
  cached_value?: (CellValue | null)
  formula?: (string | null)
  notes: string[]
  number_format?: (string | null)
  style_tags: string[]
  value?: (CellValue | null)
  }
  export interface FormulaSearchSummary {
  formula_cells_scanned: number
  matched_cells: number
  matched_groups: number
  scan_complete: boolean
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InFormulaTrace {
  export type TraceDirection = ("precedents" | "dependents")
  export type FormulaParsePolicy = ("fail" | "warn" | "off")
  export type ResourceId = string

  export interface Input {
  cell_address: string
  cursor?: (string | null)
  depth?: (number | null)
  direction: TraceDirection
  formula_parse_policy?: (FormulaParsePolicy | null)
  limit?: (number | null)
  page_size?: (number | null)
  resource_id: ResourceId
  sheet_name: string
  }
}

export namespace OutFormulaTrace {
  export type TraceDirection = ("precedents" | "dependents")
  export type FormulaParsePolicy = ("fail" | "warn" | "off")
  export type TraceCellKind = ("formula" | "literal" | "blank" | "external")
  export type CellValue = ({
  kind: "Text"
  value: string
  } | {
  kind: "Number"
  value: number
  } | {
  kind: "Bool"
  value: boolean
  } | {
  kind: "Error"
  value: string
  } | {
  kind: "Date"
  value: string
  })
  export type ResourceId = string

  export interface Output {
  data: FormulaTraceData
  operation: "formula_trace"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface FormulaTraceData {
  direction: TraceDirection
  formula_parse_diagnostics?: (FormulaParseDiagnostics | null)
  layers: TraceLayer[]
  next_cursor?: (string | null)
  notes: string[]
  origin: string
  sheet_name: string
  workbook_id: string
  }
  export interface FormulaParseDiagnostics {
  groups: FormulaParseErrorGroup[]
  groups_truncated: boolean
  policy: FormulaParsePolicy
  total_errors: number
  }
  export interface FormulaParseErrorGroup {
  count: number
  error_code: string
  error_message: string
  formula_preview: string
  sample_addresses: string[]
  sheet_name: string
  }
  export interface TraceLayer {
  depth: number
  edges: FormulaTraceEdge[]
  has_more: boolean
  highlights: TraceLayerHighlights
  summary: TraceLayerSummary
  }
  export interface FormulaTraceEdge {
  formula?: (string | null)
  from: string
  note?: (string | null)
  to: string
  }
  export interface TraceLayerHighlights {
  notable_cells: TraceCellHighlight[]
  top_formula_groups: TraceFormulaGroupHighlight[]
  top_ranges: TraceRangeHighlight[]
  }
  export interface TraceCellHighlight {
  address: string
  formula?: (string | null)
  kind: TraceCellKind
  value?: (CellValue | null)
  }
  export interface TraceFormulaGroupHighlight {
  count: number
  fingerprint: string
  formula: string
  sample_addresses: string[]
  }
  export interface TraceRangeHighlight {
  blanks: number
  count: number
  end: string
  formulas: number
  literals: number
  sample_addresses: string[]
  sample_formulas: string[]
  sample_values: CellValue[]
  start: string
  }
  export interface TraceLayerSummary {
  blank_nodes: number
  external_nodes: number
  formula_nodes: number
  total_nodes: number
  unique_formula_groups: number
  value_nodes: number
  }
}

export namespace InFormulaMap {
  export type FormulaParsePolicy = ("fail" | "warn" | "off")
  export type ResourceId = string
  export type FormulaSortBy = ("address" | "complexity" | "count")

  export interface Input {
  addresses_limit?: (number | null)
  cursor?: (string | null)
  expand?: boolean
  formula_parse_policy?: (FormulaParsePolicy | null)
  include_addresses?: (boolean | null)
  limit?: (number | null)
  range?: (string | null)
  resource_id: ResourceId
  sheet_name: string
  sort_by?: (FormulaSortBy | null)
  summary_only?: (boolean | null)
  }
}

export namespace OutFormulaMap {
  export type FormulaParsePolicy = ("fail" | "warn" | "off")
  export type ResourceId = string

  export interface Output {
  data: FormulaMapData
  operation: "formula_map"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface FormulaMapData {
  formula_parse_diagnostics?: (FormulaParseDiagnostics | null)
  groups: FormulaGroup[]
  next_cursor?: (string | null)
  next_offset?: (number | null)
  sheet_name: string
  workbook_id: string
  }
  export interface FormulaParseDiagnostics {
  groups: FormulaParseErrorGroup[]
  groups_truncated: boolean
  policy: FormulaParsePolicy
  total_errors: number
  }
  export interface FormulaParseErrorGroup {
  count: number
  error_code: string
  error_message: string
  formula_preview: string
  sample_addresses: string[]
  sheet_name: string
  }
  export interface FormulaGroup {
  addresses?: string[]
  count?: (number | null)
  fingerprint: string
  formula: string
  is_array: boolean
  is_shared: boolean
  is_volatile: boolean
  }
}

export namespace InProfileTable {
  export type ResourceId = string
  /**
   * Sampling mode for table reads
   */
  export type SampleMode = ("first" | "last" | "distributed")

  export interface Input {
  range?: (string | null)
  region_id?: (number | null)
  resource_id: ResourceId
  sample_mode?: (SampleMode | null)
  sample_size?: (number | null)
  sheet_name?: (string | null)
  summary_only?: (boolean | null)
  table_name?: (string | null)
  }
}

export namespace OutProfileTable {
  /**
   * Sampling mode for table reads
   */
  export type SampleMode = ("first" | "last" | "distributed")
  export type CellValue = ({
  kind: "Text"
  value: string
  } | {
  kind: "Number"
  value: number
  } | {
  kind: "Bool"
  value: boolean
  } | {
  kind: "Error"
  value: string
  } | {
  kind: "Date"
  value: string
  })
  export type ResourceId = string

  export interface Output {
  data: ProfileTableData
  operation: "profile_table"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface ProfileTableData {
  column_types: ColumnTypeSummary[]
  confidence: ProfileConfidence
  coverage: ProfileCoverage
  headers: string[]
  notes: string[]
  row_count: number
  samples: {
  [k: string]: (CellValue | null)
  }[]
  sheet_name: string
  source: ProfileSource
  table_name?: (string | null)
  workbook_id: string
  }
  export interface ColumnTypeSummary {
  distinct: number
  inferred_type: string
  max?: (number | null)
  mean?: (number | null)
  min?: (number | null)
  name: string
  nulls: number
  top_values: string[]
  }
  export interface ProfileConfidence {
  heuristic: boolean
  reason: string
  status: string
  }
  export interface ProfileCoverage {
  complete: boolean
  rows_in_scope: number
  rows_scanned: number
  sample_mode: SampleMode
  }
  export interface ProfileSource {
  bounds: string
  header_provenance: string
  header_row: number
  selector_kind: string
  selector_value?: (string | null)
  sheet_name: string
  }
}

export namespace InSheetStatistics {
  export type ResourceId = string

  export interface Input {
  resource_id: ResourceId
  sample_rows?: (number | null)
  sheet_name: string
  summary_only?: (boolean | null)
  }
}

export namespace OutSheetStatistics {
  export type CellValue = ({
  kind: "Text"
  value: string
  } | {
  kind: "Number"
  value: number
  } | {
  kind: "Bool"
  value: boolean
  } | {
  kind: "Error"
  value: string
  } | {
  kind: "Date"
  value: string
  })
  export type ResourceId = string

  export interface Output {
  data: SheetStatisticsResponse
  operation: "sheet_statistics"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface SheetStatisticsResponse {
  column_count: number
  density: number
  duplicate_warnings: string[]
  null_counts: {
  [k: string]: number
  }
  numeric_columns?: ColumnSummary[]
  row_count: number
  sheet_name: string
  text_columns?: ColumnSummary[]
  workbook_id: string
  }
  export interface ColumnSummary {
  column: string
  header?: (string | null)
  max?: (number | null)
  mean?: (number | null)
  min?: (number | null)
  samples?: CellValue[]
  }
}

export namespace InScreenshotSheet {
  /**
   * Which renderer to use. `native` is the in-process raster renderer; it needs
   * no external process and is the default wherever it is compiled in.
   * `libreoffice` is the legacy macro-to-PDF-to-PNG path and stays opt-in.
   */
  export type ScreenshotBackend = ("native" | "libreoffice")
  /**
   * PNG encoder effort. Encoding dominates render time, so this is the one
   * renderer knob worth exposing: `fast` trades bytes for latency, `best`
   * trades latency for bytes. Geometry never depends on it.
   */
  export type ScreenshotPngLevel = ("fast" | "balanced" | "best")
  export type ResourceId = string

  export interface Input {
  /**
   * Defaults to `native` when the `render` feature is compiled in, and to
   * `libreoffice` otherwise.
   */
  backend?: (ScreenshotBackend | null)
  /**
   * PNG encoder effort for the native backend. Defaults to `balanced`.
   * Rejected by the LibreOffice backend, which owns its own encoder.
   */
  png_level?: (ScreenshotPngLevel | null)
  range?: (string | null)
  resource_id: ResourceId
  sheet_name: string
  }
}

export namespace OutScreenshotSheet {
  export type EvaluationState = ("clean" | "errors_found" | "partial" | "not_evaluated")
  /**
   * PNG encoder effort. Encoding dominates render time, so this is the one
   * renderer knob worth exposing: `fast` trades bytes for latency, `best`
   * trades latency for bytes. Geometry never depends on it.
   */
  export type ScreenshotPngLevel = ("fast" | "balanced" | "best")
  /**
   * Structured account of what the renderer did not reproduce. A closed set:
   * nothing unsupported disappears silently.
   */
  export type ScreenshotWarning = ("conditional_format_omitted" | "chart_omitted" | "image_omitted" | "font_substituted" | "rich_text_flattened" | "number_format_approximated" | "formulas_unevaluated" | "text_rotation_omitted" | "pattern_fill_approximated")
  export type ResourceId = string

  export interface Output {
  data: ScreenshotSheetData
  operation: "screenshot_sheet"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface ScreenshotSheetData {
  artifact: ArtifactHandle
  calculation: CalculationMetadata
  duration_ms: number
  /**
   * How faithful the render is. Mirrors `agent_spreadsheet_render::Fidelity`,
   * and is `full` for the LibreOffice backend, which reports no warnings.
   */
  fidelity?: ("full" | "partial")
  height?: (number | null)
  /**
   * The PNG encoder effort the render actually used, `null` when the
   * backend does not expose one.
   */
  png_level?: (ScreenshotPngLevel | null)
  range: string
  /**
   * Renderer identity, e.g. `native-raster/1` or `libreoffice`. Additive:
   * older payloads without it deserialize to the LibreOffice default.
   */
  renderer?: string
  sheet_name: string
  warnings?: ScreenshotWarning[]
  /**
   * Rendered image geometry in device pixels. Reported by renderers that
   * know it before encoding; `null` for the LibreOffice path, which does
   * not.
   */
  width?: (number | null)
  }
  export interface ArtifactHandle {
  bytes: number
  handle: string
  hash: string
  media_type: string
  }
  /**
   * Calculation state at the rendered revision. Rendering never
   * recalculates, so this is how a caller learns whether what it is looking
   * at is current.
   */
  export interface CalculationMetadata {
  revision_id: string
  state: EvaluationState
  }
}

export namespace InSheetportManifest {
  export type Input = ({
  action: "candidates"
  resource_id: ResourceId
  sheet_filter?: (string | null)
  } | {
  action: "schema"
  } | {
  action: "validate"
  manifest_yaml: string
  } | {
  action: "normalize"
  manifest_yaml: string
  } | {
  action: "bind_check"
  manifest_yaml: string
  resource_id: ResourceId
  })
  export type ResourceId = string
}

export namespace OutSheetportManifest {
  export type SheetportManifestData = ({
  action: "candidates"
  manifest_yaml: string
  sheets: ManifestSheetStub[]
  slug: string
  } | {
  action: "schema"
  schema: unknown
  } | {
  action: "validate"
  issues: SheetportIssue[]
  valid: boolean
  } | {
  action: "normalize"
  issues: SheetportIssue[]
  manifest_yaml: string
  valid: boolean
  } | {
  action: "bind_check"
  binding_count: number
  issues: SheetportIssue[]
  ok: boolean
  stage: SheetportBindStage
  })
  export type SheetClassification = ("data" | "calculator" | "mixed" | "metadata" | "empty")
  export type SheetportBindStage = ("complete" | "parse" | "validate" | "bind")
  export type ResourceId = string

  export interface Output {
  data: SheetportManifestData
  operation: "sheetport_manifest"
  resource_id?: (ResourceId | null)
  revision_id?: (string | null)
  schema_version: "1"
  }
  export interface ManifestSheetStub {
  candidate_expectations: string[]
  classification: SheetClassification
  notes: string[]
  sheet_name: string
  }
  export interface SheetportIssue {
  message: string
  path: string
  }
}

export namespace InExecuteSheetport {
  export type SheetportValue = ({
  kind: "empty"
  } | {
  kind: "boolean"
  value: boolean
  } | {
  kind: "number"
  value: number
  } | {
  kind: "integer"
  value: number
  } | {
  kind: "text"
  value: string
  } | {
  kind: "range"
  /**
   * @maxItems 10000
   */
  rows: SheetportScalar[][]
  } | {
  kind: "table"
  /**
   * @maxItems 10000
   */
  rows: {
  [k: string]: SheetportScalar
  }[]
  } | {
  fields: {
  [k: string]: SheetportScalar
  }
  kind: "record"
  })
  export type SheetportScalar = ({
  kind: "empty"
  } | {
  kind: "boolean"
  value: boolean
  } | {
  kind: "number"
  value: number
  } | {
  kind: "integer"
  value: number
  } | {
  kind: "text"
  value: string
  })
  export type ResourceId = string

  export interface Input {
  freeze_volatile?: boolean
  inputs?: {
  [k: string]: SheetportValue
  }
  manifest_yaml: string
  resource_id: ResourceId
  rng_seed?: (number | null)
  }
}

export namespace OutExecuteSheetport {
  export type SheetportCoverageState = ("complete" | "partial")
  export type SheetportExecutionErrorCode = ("MISSING_REQUIRED_INPUT" | "OUTPUT_NOT_RETURNED" | "PORT_CONSTRAINT_VIOLATION")
  export type SheetportConstraintKind = ("required" | "manifest_constraint")
  export type SheetportValue = ({
  kind: "empty"
  } | {
  kind: "boolean"
  value: boolean
  } | {
  kind: "number"
  value: number
  } | {
  kind: "integer"
  value: number
  } | {
  kind: "text"
  value: string
  } | {
  kind: "range"
  /**
   * @maxItems 10000
   */
  rows: SheetportScalar[][]
  } | {
  kind: "table"
  /**
   * @maxItems 10000
   */
  rows: {
  [k: string]: SheetportScalar
  }[]
  } | {
  fields: {
  [k: string]: SheetportScalar
  }
  kind: "record"
  })
  export type SheetportScalar = ({
  kind: "empty"
  } | {
  kind: "boolean"
  value: boolean
  } | {
  kind: "number"
  value: number
  } | {
  kind: "integer"
  value: number
  } | {
  kind: "text"
  value: string
  })
  export type SheetportExecutionStatus = ("completed" | "partial" | "failed")
  export type ResourceId = string

  export interface Output {
  data: ExecuteSheetportData
  operation: "execute_sheetport"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface ExecuteSheetportData {
  coverage: SheetportExecutionCoverage
  errors: SheetportExecutionError[]
  results: {
  [k: string]: SheetportValue
  }
  status: SheetportExecutionStatus
  }
  export interface SheetportExecutionCoverage {
  declared_input_ports: number
  declared_output_ports: number
  returned_output_ports: number
  state: SheetportCoverageState
  supplied_input_ports: number
  }
  export interface SheetportExecutionError {
  code: SheetportExecutionErrorCode
  constraint?: (SheetportPortConstraintError | null)
  message: string
  port_id?: (string | null)
  }
  export interface SheetportPortConstraintError {
  actual: string
  expected: string
  kind: SheetportConstraintKind
  }
}

export namespace InInspectVba {
  export type Input = ({
  cursor?: (string | null)
  include_references?: (boolean | null)
  limit_modules?: (number | null)
  resource_id: ResourceId
  view: "project_summary"
  } | {
  cursor?: (string | null)
  limit_lines?: (number | null)
  module_name: VbaModuleName
  resource_id: ResourceId
  view: "module_source"
  })
  export type ResourceId = string
  export type VbaModuleName = string
}

export namespace OutInspectVba {
  export type InspectVbaData = ({
  code_page?: (number | null)
  has_vba: boolean
  /**
   * @maxItems 100
   */
  modules: VbaModuleSummary[]
  next_cursor?: (string | null)
  references: VbaReferenceSummary[]
  sys_kind?: (string | null)
  view: "project_summary"
  } | {
  module_name: string
  next_cursor?: (string | null)
  returned_lines: number
  source: string
  start_line: number
  view: "module_source"
  })
  export type ResourceId = string

  export interface Output {
  data: InspectVbaData
  operation: "inspect_vba"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface VbaModuleSummary {
  module_type: string
  name: string
  private: boolean
  read_only: boolean
  }
  export interface VbaReferenceSummary {
  kind: string
  }
}

export namespace InWrite {
  export type FormulaParsePolicy = ("fail" | "warn" | "off")
  export type WriteMode = ("preview" | "apply" | "stage")
  export type WriteOp = (SetCellsOp | {
  kind: "merge_cells"
  sheet_name: string
  target_range: string
  } | {
  kind: "unmerge_cells"
  sheet_name: string
  target_range: string
  } | {
  at_row: number
  count: number
  expand_adjacent_sums?: boolean
  kind: "insert_rows"
  sheet_name: string
  } | {
  count: number
  kind: "delete_rows"
  sheet_name: string
  start_row: number
  } | {
  at_col: string
  count: number
  kind: "insert_cols"
  sheet_name: string
  } | {
  count: number
  kind: "delete_cols"
  sheet_name: string
  start_col: string
  } | {
  kind: "rename_sheet"
  new_name: string
  old_name: string
  } | {
  kind: "create_sheet"
  name: string
  position?: (number | null)
  } | {
  kind: "delete_sheet"
  name: string
  } | {
  dest_anchor: string
  dest_sheet_name?: (string | null)
  include_formulas?: boolean
  include_styles?: boolean
  kind: "copy_range"
  sheet_name: string
  src_range: string
  } | {
  dest_anchor: string
  dest_sheet_name?: (string | null)
  include_formulas?: boolean
  include_styles?: boolean
  kind: "move_range"
  sheet_name: string
  src_range: string
  } | {
  kind: "style"
  op_mode?: (StylePatchMode | null)
  patch: StylePatch
  sheet_name: string
  target: StyleTarget
  } | {
  kind: "column_size"
  sheet_name: string
  size: ColumnSizeSpec
  target: ColumnTarget
  } | {
  anchor_cell: string
  base_formula: string
  fill_direction?: (FillDirection | null)
  kind: "formula_pattern"
  relative_mode?: (FormulaRelativeMode | null)
  sheet_name: string
  target_range: string
  } | {
  case_sensitive?: boolean
  find: string
  kind: "replace_in_formulas"
  range?: (string | null)
  regex?: boolean
  replace: string
  sheet_name: string
  } | {
  kind: "define_name"
  name: string
  refers_to: string
  scope: NameScope
  scope_sheet_name?: (string | null)
  } | {
  kind: "update_name"
  name: string
  refers_to?: (string | null)
  scope?: (NameScope | null)
  scope_sheet_name?: (string | null)
  } | {
  kind: "delete_name"
  name: string
  scope?: (NameScope | null)
  scope_sheet_name?: (string | null)
  } | {
  anchor: string
  clear_target?: boolean
  grid: GridPayload
  kind: "import_grid"
  sheet_name: string
  } | {
  anchor: string
  clear_target?: boolean
  csv: string
  header?: boolean
  kind: "import_csv"
  sheet_name: string
  } | {
  footer_policy?: ("auto" | "before_footer" | "append_at_end")
  kind: "append_rows"
  region_id?: (number | null)
  /**
   * @maxItems 100000
   */
  rows: (MatrixCell | null)[][]
  sheet_name: string
  table_name?: (string | null)
  } | {
  after?: (number | null)
  before?: (number | null)
  count?: number
  expand_adjacent_sums?: boolean
  insert_at?: (number | null)
  kind: "clone_row"
  merge_policy?: ("safe" | "strict")
  patch_targets?: ("likely_inputs" | "all_non_formula" | "none")
  sheet_name: string
  source_row: number
  } | {
  after?: (number | null)
  before?: (number | null)
  expand_adjacent_sums?: boolean
  insert_at?: (number | null)
  kind: "clone_row_band"
  merge_policy?: ("safe" | "strict")
  patch_targets?: ("likely_inputs" | "all_non_formula" | "none")
  repeat?: number
  sheet_name: string
  source_rows: string
  } | {
  clear_formulas?: boolean
  clear_values?: boolean
  kind: "clear_range"
  sheet_name: string
  target: TransformTarget
  } | {
  is_formula?: boolean
  kind: "fill_range"
  overwrite_formulas?: boolean
  sheet_name: string
  target: TransformTarget
  value: string
  } | {
  case_sensitive?: boolean
  find: string
  include_formulas?: boolean
  kind: "replace_in_range"
  match_mode?: ("exact" | "contains")
  replace: string
  sheet_name: string
  target: TransformTarget
  } | {
  anchor: string
  kind: "write_matrix"
  overwrite_formulas?: boolean
  /**
   * @maxItems 100000
   */
  rows: (MatrixCell | null)[][]
  sheet_name: string
  } | {
  freeze_cols?: number
  freeze_rows?: number
  kind: "freeze_panes"
  sheet_name: string
  top_left_cell?: (string | null)
  } | {
  kind: "set_zoom"
  sheet_name: string
  zoom_percent: number
  } | {
  kind: "set_gridlines"
  sheet_name: string
  show: boolean
  } | {
  bottom: number
  footer?: (number | null)
  header?: (number | null)
  kind: "set_page_margins"
  left: number
  right: number
  sheet_name: string
  top: number
  } | {
  fit_to_height?: (number | null)
  fit_to_width?: (number | null)
  kind: "set_page_setup"
  orientation: PageOrientation
  scale_percent?: (number | null)
  sheet_name: string
  } | {
  kind: "set_print_area"
  range: string
  sheet_name: string
  } | {
  /**
   * @maxItems 100000
   */
  col_breaks?: number[]
  kind: "set_page_breaks"
  /**
   * @maxItems 100000
   */
  row_breaks?: number[]
  sheet_name: string
  } | {
  kind: "set_data_validation"
  sheet_name: string
  target_range: string
  validation: DataValidationSpec
  } | {
  kind: "add_conditional_format"
  rule: ConditionalFormatRuleSpec
  sheet_name: string
  style?: ConditionalFormatStyleSpec
  target_range: string
  } | {
  kind: "set_conditional_format"
  rule: ConditionalFormatRuleSpec
  sheet_name: string
  style?: ConditionalFormatStyleSpec1
  target_range: string
  } | {
  kind: "clear_conditional_formats"
  sheet_name: string
  target_range: string
  })
  export type CellContent = ({
  kind: "value"
  value: CellValuePrimitive
  } | {
  formula: string
  kind: "formula"
  })
  export type CellValuePrimitive = (string | number | boolean)
  export type SetCellsKind = "set_cells"
  export type StylePatchMode = ("merge" | "set" | "clear")
  export type FillPatch = ({
  background_color?: (string | null)
  foreground_color?: (string | null)
  kind: "pattern"
  pattern_type?: (string | null)
  } | {
  degree?: (number | null)
  kind: "gradient"
  stops?: (GradientStopPatch[] | null)
  })
  export type StyleTarget = ({
  kind: "range"
  range: string
  } | {
  kind: "region"
  region_id: number
  } | {
  /**
   * @maxItems 100000
   */
  cells: string[]
  kind: "cells"
  })
  export type ColumnSizeSpec = ({
  kind: "auto"
  max_width_chars?: (number | null)
  min_width_chars?: (number | null)
  } | {
  kind: "width"
  width_chars: number
  })
  export type ColumnTarget = {
  kind: "columns"
  range: string
  }
  export type FillDirection = ("down" | "right" | "both")
  export type FormulaRelativeMode = ("excel" | "abs_cols" | "abs_rows")
  export type NameScope = ("workbook" | "sheet")
  export type MatrixCell = ({
  v: unknown
  } | {
  f: string
  })
  export type TransformTarget = ({
  kind: "range"
  range: string
  } | {
  kind: "region"
  region_id: number
  } | {
  /**
   * @maxItems 100000
   */
  cells: string[]
  kind: "cells"
  })
  export type PageOrientation = ("portrait" | "landscape")
  export type DataValidationKind = ("list" | "whole" | "decimal" | "date" | "custom")
  export type ConditionalFormatRuleSpec = ({
  formula: string
  kind: "cell_is"
  operator: ConditionalFormatOperator
  } | {
  formula: string
  kind: "expression"
  })
  export type ConditionalFormatOperator = ("less_than" | "less_than_or_equal" | "greater_than" | "greater_than_or_equal" | "equal" | "not_equal" | "between" | "not_between")
  export type ResourceId = string

  export interface Input {
  atomic?: boolean
  expected_revision: string
  formula_parse_policy?: (FormulaParsePolicy | null)
  label?: (string | null)
  mode: WriteMode
  /**
   * @minItems 1
   * @maxItems 128
   */
  ops: [WriteOp, ...(WriteOp)[]]
  resource_id: ResourceId
  }
  export interface SetCellsOp {
  cells: {
  [k: string]: CellContent
  }
  kind: SetCellsKind
  overwrite_formulas?: boolean
  sheet_name: string
  }
  export interface StylePatch {
  alignment?: (AlignmentPatch | null)
  borders?: (BordersPatch | null)
  fill?: (FillPatch | null)
  font?: (FontPatch | null)
  number_format?: (string | null)
  }
  export interface AlignmentPatch {
  horizontal?: (string | null)
  text_rotation?: (number | null)
  vertical?: (string | null)
  wrap_text?: (boolean | null)
  }
  export interface BordersPatch {
  bottom?: (BorderSidePatch | null)
  diagonal?: (BorderSidePatch | null)
  diagonal_down?: (boolean | null)
  diagonal_up?: (boolean | null)
  horizontal?: (BorderSidePatch | null)
  left?: (BorderSidePatch | null)
  right?: (BorderSidePatch | null)
  top?: (BorderSidePatch | null)
  vertical?: (BorderSidePatch | null)
  }
  export interface BorderSidePatch {
  color?: (string | null)
  style?: (string | null)
  }
  export interface GradientStopPatch {
  color: string
  position: number
  }
  export interface FontPatch {
  bold?: (boolean | null)
  color?: (string | null)
  italic?: (boolean | null)
  name?: (string | null)
  size?: (number | null)
  strikethrough?: (boolean | null)
  underline?: (string | null)
  }
  export interface GridPayload {
  anchor: string
  /**
   * @maxItems 100000
   */
  columns?: GridColumnHint[]
  /**
   * @maxItems 100000
   */
  merges?: string[]
  /**
   * @maxItems 100000
   */
  rows: GridRow[]
  sheet: string
  }
  export interface GridColumnHint {
  offset: number
  width_chars: number
  }
  export interface GridRow {
  /**
   * @maxItems 100000
   */
  cells: GridCell[]
  }
  export interface GridCell {
  f?: (string | null)
  fmt?: (string | null)
  /**
   * @minItems 2
   * @maxItems 2
   */
  offset: [number, number]
  style?: (StylePatch | null)
  v?: unknown
  }
  export interface DataValidationSpec {
  allow_blank?: (boolean | null)
  error?: (ValidationMessage | null)
  formula1: string
  formula2?: (string | null)
  kind: DataValidationKind
  prompt?: (ValidationMessage | null)
  }
  export interface ValidationMessage {
  message: string
  title: string
  }
  export interface ConditionalFormatStyleSpec {
  bold?: (boolean | null)
  fill_color?: (string | null)
  font_color?: (string | null)
  }
  export interface ConditionalFormatStyleSpec1 {
  bold?: (boolean | null)
  fill_color?: (string | null)
  font_color?: (string | null)
  }
}

export namespace OutWrite {
  export type WriteResponseData = ({
  atomic: boolean
  diff: WriteDiff
  impact: WriteImpact
  mode: "preview"
  ops_previewed: number
  results: WriteOpResult[]
  revision_after: string
  revision_before: string
  status: "previewed"
  } | {
  atomic: true
  change_id: string
  diff: WriteDiff
  impact: WriteImpact
  mode: "stage"
  ops_staged: number
  results: WriteOpResult[]
  revision_after: string
  revision_before: string
  status: "staged"
  } | {
  atomic: boolean
  diff: WriteDiff
  impact: WriteImpact
  mode: "apply"
  ops_applied: number
  results: WriteOpResult[]
  revision_after: string
  revision_before: string
  status: "applied"
  } | {
  atomic: false
  diff: WriteDiff
  impact: WriteImpact
  mode: "apply"
  ops_applied: number
  results: WriteOpResult[]
  revision_after: string
  revision_before: string
  status: "partial"
  } | {
  atomic: boolean
  diff: WriteDiff
  impact: WriteImpact
  mode: WriteMode
  ops_applied: number
  results: WriteOpResult[]
  revision_after: string
  revision_before: string
  status: "failed"
  } | {
  atomic: true
  diff: WriteDiff
  impact: WriteImpact
  mode: "apply"
  ops_applied: number
  results: WriteOpResult[]
  revision_after: string
  revision_before: string
  rolled_back: boolean
  status: "rolled_back"
  })
  export type OperationRisk = ("low" | "moderate" | "high" | "destructive")
  export type WriteOpStatus = ("previewed" | "staged" | "applied" | "failed" | "skipped" | "rolled_back")
  export type WriteMode = ("preview" | "apply" | "stage")
  export type ResourceId = string

  export interface Output {
  data: WriteResponseData
  operation: "write"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface WriteDiff {
  change_count: number
  changes: unknown[]
  effects: unknown[]
  exact: boolean
  precision: string
  }
  export interface WriteImpact {
  op_kinds: string[]
  risk: OperationRisk
  }
  export interface WriteOpResult {
  detail?: unknown
  error?: (WriteOpError | null)
  index: number
  kind: string
  status: WriteOpStatus
  }
  export interface WriteOpError {
  code: string
  message: string
  path: string
  retryable: boolean
  }
}

export namespace InCreateFork {
  export type ResourceId = string

  export interface Input {
  expected_revision: string
  label?: (string | null)
  resource_id: ResourceId
  }
}

export namespace OutCreateFork {
  export type ResourceId = string

  export interface Output {
  data: CreateForkData
  operation: "create_fork"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface CreateForkData {
  base_resource_id: ResourceId
  base_revision_id: string
  fork_resource_id: ResourceId
  label?: (string | null)
  revision_id: string
  /**
   * Null means no automatic resource expiry (idle detach is separate).
   */
  ttl_seconds?: (number | null)
  warnings: Warning[]
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InListForks {
  export interface Input {

  }
}

export namespace OutListForks {
  export type ResourceId = string

  export interface Output {
  data: ListForksData
  operation: "list_forks"
  schema_version: "1"
  }
  export interface ListForksData {
  forks: CanonicalForkDescriptor[]
  warnings: Warning[]
  }
  export interface CanonicalForkDescriptor {
  /**
   * Unknown when the durable catalog has no authoritative creation clock.
   */
  age_seconds?: (number | null)
  checkpoint_count: number
  operation_count: number
  recalc_needed: boolean
  resource_id: ResourceId
  /**
   * No live CAS exists for an inactive durable owner.
   */
  revision_id?: (string | null)
  staged_change_count: number
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InRecalculate {
  export type RecalcBackendKind = ("formualizer" | "libreoffice" | "auto")
  export type ResourceId = string

  export interface Input {
  backend?: (RecalcBackendKind | null)
  expected_revision: string
  resource_id: ResourceId
  timeout_ms?: number
  }
}

export namespace OutRecalculate {
  export type EvaluationFreshness = ("current_revision" | "unknown")
  export type EvaluationSource = ("formualizer" | "trusted_cache" | "none")
  export type EvaluationState = ("clean" | "errors_found" | "partial" | "not_evaluated")
  export type ResourceId = string

  export interface Output {
  data: RecalculateData
  operation: "recalculate"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface RecalculateData {
  backend: string
  cells_evaluated?: (number | null)
  duration_ms: number
  error_count?: (number | null)
  eval_errors?: (string[] | null)
  evaluation_coverage: EvaluationCoverage
  revision_after: string
  revision_before: string
  state: EvaluationState
  status: string
  warnings: Warning[]
  }
  export interface EvaluationCoverage {
  error_formula_cells: number
  evaluated_formula_cells: number
  formula_cells: number
  freshness: EvaluationFreshness
  revision_id: string
  source: EvaluationSource
  unsupported_formula_cells: number
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InVerifyWorkbook {
  export type ResourceId = string

  export interface Input {
  baseline_resource_id: ResourceId
  errors_only?: boolean
  include_named_range_deltas?: boolean
  resource_id: ResourceId
  sheet_name?: (string | null)
  targets?: string[]
  targets_only?: boolean
  }
}

export namespace OutVerifyWorkbook {
  export type EvaluationFreshness = ("current_revision" | "unknown")
  export type EvaluationSource = ("formualizer" | "trusted_cache" | "none")
  export type ResourceId = string
  export type EvaluationState = ("clean" | "errors_found" | "partial" | "not_evaluated")
  export type NamedItemKind = ("named_range" | "table" | "formula" | "unknown")
  export type NamedRangeScope = ("workbook" | "sheet")
  export type ProofStatus = ("proved" | "differences_found" | "inconclusive_unevaluated" | "failed")
  export type CellValue = ({
  kind: "Text"
  value: string
  } | {
  kind: "Number"
  value: number
  } | {
  kind: "Bool"
  value: boolean
  } | {
  kind: "Error"
  value: string
  } | {
  kind: "Date"
  value: string
  })

  export interface Output {
  data: VerifyWorkbookData
  operation: "verify_workbook"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface VerifyWorkbookData {
  baseline: string
  baseline_evaluation_coverage: EvaluationCoverage
  baseline_resource_id: ResourceId
  baseline_revision_id: string
  baseline_state: EvaluationState
  current: string
  current_evaluation_coverage: EvaluationCoverage
  current_resource_id: ResourceId
  current_revision_id: string
  current_state: EvaluationState
  failure?: (string | null)
  named_range_deltas: NamedRangeDelta[]
  new_errors: ErrorDelta[]
  preexisting_errors: ErrorDelta[]
  proof_status: ProofStatus
  resolved_errors: ErrorDelta[]
  summary: VerifySummary
  target_deltas: TargetDelta[]
  warnings: Warning[]
  }
  export interface EvaluationCoverage {
  error_formula_cells: number
  evaluated_formula_cells: number
  formula_cells: number
  freshness: EvaluationFreshness
  revision_id: string
  source: EvaluationSource
  unsupported_formula_cells: number
  }
  export interface NamedRangeDelta {
  after_kind?: (NamedItemKind | null)
  after_refers_to?: (string | null)
  before_kind?: (NamedItemKind | null)
  before_refers_to?: (string | null)
  change: string
  name: string
  scope_kind?: (NamedRangeScope | null)
  scope_sheet_name?: (string | null)
  }
  export interface ErrorDelta {
  address: string
  after_error?: (string | null)
  after_formula?: (string | null)
  before_error?: (string | null)
  before_formula?: (string | null)
  }
  export interface VerifySummary {
  changed_targets: number
  named_range_delta_count: number
  new_error_count: number
  preexisting_error_count: number
  resolved_error_count: number
  target_classification_counts: TargetClassificationCounts
  target_count: number
  }
  export interface TargetClassificationCounts {
  direct_edit: number
  formula_shift: number
  new_error: number
  recalc_result: number
  unchanged: number
  }
  export interface TargetDelta {
  address: string
  after?: (CellValue | null)
  after_formula?: (string | null)
  before?: (CellValue | null)
  before_formula?: (string | null)
  changed: boolean
  classification: string
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InExportFork {
  export type ExportDestination = {
  kind: "workspace"
  name: string
  }
  export type ResourceId = string

  export interface Input {
  destination: ExportDestination
  expected_revision: string
  resource_id: ResourceId
  }
}

export namespace OutExportFork {
  export type ExportedDestination = {
  kind: "workspace"
  name: string
  }
  export type ResourceId = string

  export interface Output {
  data: ExportForkData
  operation: "export_fork"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface ExportForkData {
  artifact: ArtifactMetadata
  destination: ExportedDestination
  revision_after: string
  revision_before: string
  warnings: Warning[]
  }
  export interface ArtifactMetadata {
  artifact_id: string
  bytes: number
  media_type: string
  sha256: string
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InDiscardFork {
  export type ResourceId = string

  export interface Input {
  expected_revision: string
  resource_id: ResourceId
  }
}

export namespace OutDiscardFork {
  export type ResourceId = string

  export interface Output {
  data: DiscardForkData
  operation: "discard_fork"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface DiscardForkData {
  discarded: boolean
  revision_after: string
  revision_before: string
  warnings: Warning[]
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InGetChanges {
  export type ResourceId = string
  export type ChangesView = ({
  kind: "operations"
  limit?: number
  offset?: number
  } | {
  kind: "net_diff"
  limit?: number
  offset?: number
  sheet_name?: (string | null)
  })

  export interface Input {
  resource_id: ResourceId
  view: ChangesView
  }
}

export namespace OutGetChanges {
  export type GetChangesData = ({
  kind: "operations"
  next_offset?: (number | null)
  operations: CanonicalOperationRecord[]
  revision_id: string
  total: number
  warnings: Warning[]
  } | {
  baseline: string
  baseline_revision_id: string
  changes: Change[]
  kind: "net_diff"
  next_offset?: (number | null)
  revision_id: string
  total: number
  warnings: Warning[]
  })
  export type Change = (CellChange | TableDiff | NameDiff)
  export type CellChange = ({
  sheet: string
  } & CellChange1)
  export type CellChange1 = ({
  address: string
  formula?: (string | null)
  type: "added"
  value?: (string | null)
  } | {
  address: string
  old_value?: (string | null)
  type: "deleted"
  } | {
  address: string
  new_formula?: (string | null)
  new_style_id?: (number | null)
  new_value?: (string | null)
  old_formula?: (string | null)
  old_style_id?: (number | null)
  old_value?: (string | null)
  subtype: ModificationType
  type: "modified"
  })
  export type ModificationType = ("formula_edit" | "recalc_result" | "value_edit" | "style_edit")
  export type TableDiff = ({
  display_name: string
  range: string
  sheet: string
  type: "table_added"
  } | {
  display_name: string
  sheet: string
  type: "table_deleted"
  } | {
  display_name: string
  new_range: string
  old_range: string
  sheet: string
  type: "table_modified"
  })
  export type NameDiff = ({
  formula: string
  name: string
  scope_sheet?: (string | null)
  type: "name_added"
  } | {
  name: string
  scope_sheet?: (string | null)
  type: "name_deleted"
  } | {
  name: string
  new_formula: string
  old_formula: string
  scope_sheet?: (string | null)
  type: "name_modified"
  })
  export type ResourceId = string

  export interface Output {
  data: GetChangesData
  operation: "get_changes"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface CanonicalOperationRecord {
  kind: string
  op_kinds: string[]
  revision_after: string
  revision_before: string
  sequence: number
  timestamp?: (string | null)
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InCheckpoint {
  export type Input = ({
  action: "create"
  expected_revision: string
  label?: (string | null)
  resource_id: ResourceId
  } | {
  action: "list"
  resource_id: ResourceId
  } | {
  action: "restore"
  checkpoint_id: string
  expected_revision: string
  resource_id: ResourceId
  } | {
  action: "delete"
  checkpoint_id: string
  expected_revision: string
  resource_id: ResourceId
  })
  export type ResourceId = string
}

export namespace OutCheckpoint {
  export type CheckpointData = ({
  action: "create"
  checkpoint: CheckpointDescriptor
  revision_after: string
  revision_before: string
  total_checkpoints: number
  warnings: Warning[]
  } | {
  action: "list"
  checkpoints: CheckpointDescriptor[]
  revision_id: string
  warnings: Warning[]
  } | {
  action: "restore"
  invalidated_checkpoint_ids: string[]
  operations_removed: number
  recalc_needed: boolean
  restored_checkpoint: CheckpointDescriptor
  retained_checkpoint_ids: string[]
  revision_after: string
  revision_before: string
  staged_changes_discarded: number
  warnings: Warning[]
  } | {
  action: "delete"
  checkpoint_id: string
  deleted: boolean
  revision_after: string
  revision_before: string
  warnings: Warning[]
  })
  export type ResourceId = string

  export interface Output {
  data: CheckpointData
  operation: "checkpoint"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface CheckpointDescriptor {
  checkpoint_id: string
  created_at?: (string | null)
  label?: (string | null)
  recalc_needed: boolean
  snapshot_revision: string
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InStagedChange {
  export type Input = ({
  action: "list"
  resource_id: ResourceId
  } | {
  action: "apply"
  change_id: string
  expected_revision: string
  resource_id: ResourceId
  } | {
  action: "discard"
  change_id: string
  expected_revision: string
  resource_id: ResourceId
  })
  export type ResourceId = string
}

export namespace OutStagedChange {
  export type StagedChangeData = ({
  action: "list"
  revision_id: string
  staged_changes: StagedChangeDescriptor[]
  warnings: Warning[]
  } | {
  action: "apply"
  change_id: string
  head?: (string | null)
  op_kinds: string[]
  ops_applied: number
  recalc_needed: boolean
  revision_after: string
  revision_before: string
  warnings: Warning[]
  } | {
  action: "discard"
  change_id: string
  discarded: boolean
  revision_after: string
  revision_before: string
  warnings: Warning[]
  })
  export type ResourceId = string

  export interface Output {
  data: StagedChangeData
  operation: "staged_change"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface StagedChangeDescriptor {
  base_revision: string
  change_id: string
  created_at?: (string | null)
  label?: (string | null)
  summary: ChangeSummary
  }
  export interface ChangeSummary {
  affected_bounds: string[]
  affected_sheets: string[]
  counts: {
  [k: string]: number
  }
  flags?: {
  [k: string]: boolean
  }
  op_kinds: string[]
  warnings: string[]
  }
  export interface Warning {
  code: string
  message: string
  }
}

export namespace InSessionHistory {
  export type Input = ({
  action: "status"
  resource_id: ResourceId
  } | {
  action: "outcome"
  request_id: string
  resource_id: ResourceId
  } | {
  action: "list"
  limit?: number
  offset?: number
  resource_id: ResourceId
  } | {
  action: "undo"
  expected_revision: string
  resource_id: ResourceId
  } | {
  action: "redo"
  expected_revision: string
  resource_id: ResourceId
  } | {
  action: "checkout"
  expected_revision: string
  resource_id: ResourceId
  target_commit_id: string
  } | {
  action: "create_branch"
  expected_revision: string
  label?: (string | null)
  name: string
  resource_id: ResourceId
  target_commit_id?: (string | null)
  } | {
  action: "switch_branch"
  expected_revision: string
  name: string
  resource_id: ResourceId
  })
  export type ResourceId = string
}

export namespace OutSessionHistory {
  export type SessionHistoryData = ({
  health: SessionHealth
  kind: "status"
  poisoned_request_id?: (string | null)
  reason?: (string | null)
  revision_id?: (string | null)
  } | {
  kind: "outcome"
  request_id: string
  response?: (CanonicalResponse | null)
  state: RequestOutcomeState
  } | {
  branch: string
  branch_labels: {
  [k: string]: string
  }
  branches: {
  [k: string]: (string | null)
  }
  head?: (string | null)
  kind: "list"
  next_offset?: (number | null)
  records: SessionHistoryEntry[]
  revision_id: string
  total: number
  } | {
  branch: string
  head?: (string | null)
  kind: "mutation"
  revision_after: string
  revision_before: string
  transition: ResidentTransition
  })
  export type SessionHealth = ("usable" | "poisoned")
  export type ResourceId = string
  export type RequestOutcomeState = ("committed" | "not_found" | "unknown")
  export type ResidentTransition = ("mutation" | "stage_apply" | "catalog_stage" | "catalog_discard" | "calculation_publish" | "calculation_invalidate" | "undo" | "redo" | "checkout" | "branch_create" | "branch_switch" | "checkpoint" | "checkpoint_delete" | "receipt" | "restart")

  export interface Output {
  data: SessionHistoryData
  operation: "session_history"
  resource_id: ResourceId
  revision_id: string
  schema_version: "1"
  }
  export interface CanonicalResponse {
  data: unknown
  operation: string
  resource_id?: (ResourceId | null)
  revision_id?: (string | null)
  schema_version: string
  }
  export interface SessionHistoryEntry {
  branch: string
  commit_id: string
  history_parent_commit_id?: (string | null)
  op_kinds: string[]
  request_id: string
  resulting_head?: (string | null)
  revision_id: string
  sequence: number
  transition: ResidentTransition
  }
}

export namespace CanonicalErrors {
  export type CanonicalErrorCode = ("UNKNOWN_OPERATION" | "INVALID_REQUEST" | "CAPABILITY_UNAVAILABLE" | "RESOURCE_NOT_FOUND" | "OPERATION_FAILED" | "STALE_CURSOR" | "CURSOR_MISMATCH" | "ROW_EXCEEDS_BUDGET" | "REVISION_CONFLICT" | "OUTCOME_UNKNOWN" | "RECOVERY_REQUIRED")

  export interface Envelope {
  error: CanonicalError
  schema_version: "1"
  }
  export interface CanonicalError {
  code: CanonicalErrorCode
  message: string
  operation?: (string | null)
  path?: (string | null)
  }
}

/** The canonical error envelope returned by every adapter. */
export type CanonicalErrorEnvelope = CanonicalErrors.Envelope

/** The canonical error code set. */
export type CanonicalErrorCode = CanonicalErrors.CanonicalErrorCode

/** Canonical input object keyed by operation name. */
export interface OperationInputs {
  list_workbooks: InListWorkbooks.Input
  describe_workbook: InDescribeWorkbook.Input
  list_sheets: InListSheets.Input
  sheet_overview: InSheetOverview.Input
  read_cells: InReadCells.Input
  inspect_cells: InInspectCells.Input
  read_table: InReadTable.Input
  read_layout: InReadLayout.Input
  export_grid: InExportGrid.Input
  named_ranges: InNamedRanges.Input
  analyze_styles: InAnalyzeStyles.Input
  search_values: InSearchValues.Input
  search_formulas: InSearchFormulas.Input
  formula_trace: InFormulaTrace.Input
  formula_map: InFormulaMap.Input
  profile_table: InProfileTable.Input
  sheet_statistics: InSheetStatistics.Input
  screenshot_sheet: InScreenshotSheet.Input
  sheetport_manifest: InSheetportManifest.Input
  execute_sheetport: InExecuteSheetport.Input
  inspect_vba: InInspectVba.Input
  write: InWrite.Input
  create_fork: InCreateFork.Input
  list_forks: InListForks.Input
  recalculate: InRecalculate.Input
  verify_workbook: InVerifyWorkbook.Input
  export_fork: InExportFork.Input
  discard_fork: InDiscardFork.Input
  get_changes: InGetChanges.Input
  checkpoint: InCheckpoint.Input
  staged_change: InStagedChange.Input
  session_history: InSessionHistory.Input
}

/** Canonical response envelope keyed by operation name. */
export interface OperationOutputs {
  list_workbooks: OutListWorkbooks.Output
  describe_workbook: OutDescribeWorkbook.Output
  list_sheets: OutListSheets.Output
  sheet_overview: OutSheetOverview.Output
  read_cells: OutReadCells.Output
  inspect_cells: OutInspectCells.Output
  read_table: OutReadTable.Output
  read_layout: OutReadLayout.Output
  export_grid: OutExportGrid.Output
  named_ranges: OutNamedRanges.Output
  analyze_styles: OutAnalyzeStyles.Output
  search_values: OutSearchValues.Output
  search_formulas: OutSearchFormulas.Output
  formula_trace: OutFormulaTrace.Output
  formula_map: OutFormulaMap.Output
  profile_table: OutProfileTable.Output
  sheet_statistics: OutSheetStatistics.Output
  screenshot_sheet: OutScreenshotSheet.Output
  sheetport_manifest: OutSheetportManifest.Output
  execute_sheetport: OutExecuteSheetport.Output
  inspect_vba: OutInspectVba.Output
  write: OutWrite.Output
  create_fork: OutCreateFork.Output
  list_forks: OutListForks.Output
  recalculate: OutRecalculate.Output
  verify_workbook: OutVerifyWorkbook.Output
  export_fork: OutExportFork.Output
  discard_fork: OutDiscardFork.Output
  get_changes: OutGetChanges.Output
  checkpoint: OutCheckpoint.Output
  staged_change: OutStagedChange.Output
  session_history: OutSessionHistory.Output
}

/** Every operation registered by the canonical dispatcher. */
export type OperationName = keyof OperationInputs

/** The canonical input object for `K`. */
export type InputOf<K extends OperationName> = OperationInputs[K]

/** The canonical response envelope for `K`. */
export type OutputOf<K extends OperationName> = OperationOutputs[K]
