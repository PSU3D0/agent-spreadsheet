use crate::fork::ChangeSummary;
#[cfg(not(target_arch = "wasm32"))]
use crate::fork::{StagedChange, StagedOp};
#[cfg(not(target_arch = "wasm32"))]
use crate::model::WorkbookId;
use crate::model::{
    CellValuePrimitive, FormulaParsePolicy, GridPayload, NamedRangeScope, StylePatch,
};
use crate::operations::{OperationRisk, ResourceId};
#[cfg(not(target_arch = "wasm32"))]
use crate::state::AppState;
use crate::styles::StylePatchMode;
use crate::tools::fork::{
    ApplyFormulaPatternOpInput, ColumnSizeOp, ColumnSizeSpec, ColumnTarget, MatrixCell,
    ReplaceInFormulasOp, StructureOp, StyleOp, StyleTarget, TransformOp, TransformTarget,
    apply_column_size_ops_to_workbook, apply_formula_pattern_ops_to_workbook,
    apply_replace_in_formulas_to_workbook, apply_structure_ops_to_workbook,
    apply_style_ops_to_workbook, apply_transform_ops_to_workbook,
};
use crate::tools::param_enums::{FillDirection, FormulaRelativeMode};
use crate::tools::rules_batch::{ConditionalFormatRuleSpec, RulesOp, apply_rules_ops_to_workbook};
use crate::tools::sheet_layout::{SheetLayoutOp, apply_sheet_layout_ops_to_workbook};
use crate::utils::{hash_file_sha256_hex, make_short_random_id};
use anyhow::{Result, anyhow, bail};
#[cfg(not(target_arch = "wasm32"))]
use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

const MAX_WRITE_OPS: usize = 128;
const MAX_WRITE_CELLS: usize = 100_000;
const MAX_WRITE_PAYLOAD_BYTES: usize = 1_048_576;

fn default_true() -> bool {
    true
}
fn default_clone_count() -> u32 {
    1
}
fn default_repeat() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    Preview,
    Apply,
    Stage,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CellContent {
    Value { value: CellValuePrimitive },
    Formula { formula: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetCellsOp {
    pub kind: SetCellsKind,
    pub sheet_name: String,
    pub cells: BTreeMap<String, CellContent>,
    #[serde(default)]
    pub overwrite_formulas: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
pub enum SetCellsKind {
    #[serde(rename = "set_cells")]
    SetCells,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CanonicalStructureOp {
    MergeCells {
        sheet_name: String,
        target_range: String,
    },
    UnmergeCells {
        sheet_name: String,
        target_range: String,
    },
    InsertRows {
        sheet_name: String,
        at_row: u32,
        count: u32,
        #[serde(default)]
        expand_adjacent_sums: bool,
    },
    DeleteRows {
        sheet_name: String,
        start_row: u32,
        count: u32,
    },
    InsertCols {
        sheet_name: String,
        at_col: String,
        count: u32,
    },
    DeleteCols {
        sheet_name: String,
        start_col: String,
        count: u32,
    },
    RenameSheet {
        old_name: String,
        new_name: String,
    },
    CreateSheet {
        name: String,
        #[serde(default)]
        position: Option<u32>,
    },
    DeleteSheet {
        name: String,
    },
    CopyRange {
        sheet_name: String,
        #[serde(default)]
        dest_sheet_name: Option<String>,
        src_range: String,
        dest_anchor: String,
        #[serde(default = "default_true")]
        include_styles: bool,
        #[serde(default = "default_true")]
        include_formulas: bool,
    },
    MoveRange {
        sheet_name: String,
        #[serde(default)]
        dest_sheet_name: Option<String>,
        src_range: String,
        dest_anchor: String,
        #[serde(default = "default_true")]
        include_styles: bool,
        #[serde(default = "default_true")]
        include_formulas: bool,
    },
}

impl From<&CanonicalStructureOp> for StructureOp {
    fn from(value: &CanonicalStructureOp) -> Self {
        match value {
            CanonicalStructureOp::MergeCells {
                sheet_name,
                target_range,
            } => Self::MergeCells {
                sheet_name: sheet_name.clone(),
                target_range: target_range.clone(),
            },
            CanonicalStructureOp::UnmergeCells {
                sheet_name,
                target_range,
            } => Self::UnmergeCells {
                sheet_name: sheet_name.clone(),
                target_range: target_range.clone(),
            },
            CanonicalStructureOp::InsertRows {
                sheet_name,
                at_row,
                count,
                expand_adjacent_sums,
            } => Self::InsertRows {
                sheet_name: sheet_name.clone(),
                at_row: *at_row,
                count: *count,
                expand_adjacent_sums: *expand_adjacent_sums,
            },
            CanonicalStructureOp::DeleteRows {
                sheet_name,
                start_row,
                count,
            } => Self::DeleteRows {
                sheet_name: sheet_name.clone(),
                start_row: *start_row,
                count: *count,
            },
            CanonicalStructureOp::InsertCols {
                sheet_name,
                at_col,
                count,
            } => Self::InsertCols {
                sheet_name: sheet_name.clone(),
                at_col: at_col.clone(),
                count: *count,
            },
            CanonicalStructureOp::DeleteCols {
                sheet_name,
                start_col,
                count,
            } => Self::DeleteCols {
                sheet_name: sheet_name.clone(),
                start_col: start_col.clone(),
                count: *count,
            },
            CanonicalStructureOp::RenameSheet { old_name, new_name } => Self::RenameSheet {
                old_name: old_name.clone(),
                new_name: new_name.clone(),
            },
            CanonicalStructureOp::CreateSheet { name, position } => Self::CreateSheet {
                name: name.clone(),
                position: *position,
            },
            CanonicalStructureOp::DeleteSheet { name } => Self::DeleteSheet { name: name.clone() },
            CanonicalStructureOp::CopyRange {
                sheet_name,
                dest_sheet_name,
                src_range,
                dest_anchor,
                include_styles,
                include_formulas,
            } => Self::CopyRange {
                sheet_name: sheet_name.clone(),
                dest_sheet_name: dest_sheet_name.clone(),
                src_range: src_range.clone(),
                dest_anchor: dest_anchor.clone(),
                include_styles: *include_styles,
                include_formulas: *include_formulas,
            },
            CanonicalStructureOp::MoveRange {
                sheet_name,
                dest_sheet_name,
                src_range,
                dest_anchor,
                include_styles,
                include_formulas,
            } => Self::MoveRange {
                sheet_name: sheet_name.clone(),
                dest_sheet_name: dest_sheet_name.clone(),
                src_range: src_range.clone(),
                dest_anchor: dest_anchor.clone(),
                include_styles: *include_styles,
                include_formulas: *include_formulas,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StyleWriteOp {
    Style {
        sheet_name: String,
        target: StyleTarget,
        patch: StylePatch,
        #[serde(default)]
        op_mode: Option<StylePatchMode>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ColumnWriteOp {
    ColumnSize {
        sheet_name: String,
        target: ColumnTarget,
        size: ColumnSizeSpec,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FormulaWriteOp {
    FormulaPattern {
        sheet_name: String,
        target_range: String,
        anchor_cell: String,
        base_formula: String,
        #[serde(default)]
        fill_direction: Option<FillDirection>,
        #[serde(default)]
        relative_mode: Option<FormulaRelativeMode>,
    },
    ReplaceInFormulas {
        sheet_name: String,
        find: String,
        replace: String,
        #[serde(default)]
        range: Option<String>,
        #[serde(default)]
        regex: bool,
        #[serde(default = "default_true")]
        case_sensitive: bool,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NameScope {
    Workbook,
    Sheet,
}
impl From<NameScope> for NamedRangeScope {
    fn from(value: NameScope) -> Self {
        match value {
            NameScope::Workbook => Self::Workbook,
            NameScope::Sheet => Self::Sheet,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NameWriteOp {
    DefineName {
        name: String,
        refers_to: String,
        scope: NameScope,
        #[serde(default)]
        scope_sheet_name: Option<String>,
    },
    UpdateName {
        name: String,
        #[serde(default)]
        refers_to: Option<String>,
        #[serde(default)]
        scope: Option<NameScope>,
        #[serde(default)]
        scope_sheet_name: Option<String>,
    },
    DeleteName {
        name: String,
        #[serde(default)]
        scope: Option<NameScope>,
        #[serde(default)]
        scope_sheet_name: Option<String>,
    },
}

pub use crate::core::write_planner::{AppendFooterPolicy, CloneMergePolicy, ClonePatchTargets};
fn default_patch_targets() -> ClonePatchTargets {
    ClonePatchTargets::LikelyInputs
}
fn default_merge_policy() -> CloneMergePolicy {
    CloneMergePolicy::Safe
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImportAndHelperOp {
    ImportGrid {
        sheet_name: String,
        anchor: String,
        grid: GridPayload,
        #[serde(default)]
        clear_target: bool,
    },
    ImportCsv {
        sheet_name: String,
        anchor: String,
        csv: String,
        #[serde(default)]
        header: bool,
        #[serde(default)]
        clear_target: bool,
    },
    AppendRows {
        sheet_name: String,
        #[serde(default)]
        region_id: Option<u32>,
        #[serde(default)]
        table_name: Option<String>,
        rows: Vec<Vec<Option<MatrixCell>>>,
        #[serde(default = "default_footer_policy")]
        footer_policy: AppendFooterPolicy,
    },
    CloneRow {
        sheet_name: String,
        source_row: u32,
        #[serde(default)]
        before: Option<u32>,
        #[serde(default)]
        after: Option<u32>,
        #[serde(default)]
        insert_at: Option<u32>,
        #[serde(default = "default_clone_count")]
        count: u32,
        #[serde(default)]
        expand_adjacent_sums: bool,
        #[serde(default = "default_patch_targets")]
        patch_targets: ClonePatchTargets,
        #[serde(default = "default_merge_policy")]
        merge_policy: CloneMergePolicy,
    },
    CloneRowBand {
        sheet_name: String,
        source_rows: String,
        #[serde(default)]
        before: Option<u32>,
        #[serde(default)]
        after: Option<u32>,
        #[serde(default)]
        insert_at: Option<u32>,
        #[serde(default = "default_repeat")]
        repeat: u32,
        #[serde(default)]
        expand_adjacent_sums: bool,
        #[serde(default = "default_patch_targets")]
        patch_targets: ClonePatchTargets,
        #[serde(default = "default_merge_policy")]
        merge_policy: CloneMergePolicy,
    },
}
fn default_footer_policy() -> AppendFooterPolicy {
    AppendFooterPolicy::Auto
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)] // Closed public schema union; boxing changes generated schema shape.
pub enum WriteOp {
    SetCells(SetCellsOp),
    Structure(CanonicalStructureOp),
    Style(StyleWriteOp),
    Column(ColumnWriteOp),
    Formula(FormulaWriteOp),
    Name(NameWriteOp),
    ImportAndHelper(ImportAndHelperOp),
    Transform(TransformOp),
    Layout(SheetLayoutOp),
    Rules(RulesOp),
}

impl WriteOp {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SetCells(_) => "set_cells",
            Self::Structure(op) => match op {
                CanonicalStructureOp::MergeCells { .. } => "merge_cells",
                CanonicalStructureOp::UnmergeCells { .. } => "unmerge_cells",
                CanonicalStructureOp::InsertRows { .. } => "insert_rows",
                CanonicalStructureOp::DeleteRows { .. } => "delete_rows",
                CanonicalStructureOp::InsertCols { .. } => "insert_cols",
                CanonicalStructureOp::DeleteCols { .. } => "delete_cols",
                CanonicalStructureOp::RenameSheet { .. } => "rename_sheet",
                CanonicalStructureOp::CreateSheet { .. } => "create_sheet",
                CanonicalStructureOp::DeleteSheet { .. } => "delete_sheet",
                CanonicalStructureOp::CopyRange { .. } => "copy_range",
                CanonicalStructureOp::MoveRange { .. } => "move_range",
            },
            Self::Style(_) => "style",
            Self::Column(_) => "column_size",
            Self::Formula(op) => match op {
                FormulaWriteOp::FormulaPattern { .. } => "formula_pattern",
                FormulaWriteOp::ReplaceInFormulas { .. } => "replace_in_formulas",
            },
            Self::Name(op) => match op {
                NameWriteOp::DefineName { .. } => "define_name",
                NameWriteOp::UpdateName { .. } => "update_name",
                NameWriteOp::DeleteName { .. } => "delete_name",
            },
            Self::ImportAndHelper(op) => match op {
                ImportAndHelperOp::ImportGrid { .. } => "import_grid",
                ImportAndHelperOp::ImportCsv { .. } => "import_csv",
                ImportAndHelperOp::AppendRows { .. } => "append_rows",
                ImportAndHelperOp::CloneRow { .. } => "clone_row",
                ImportAndHelperOp::CloneRowBand { .. } => "clone_row_band",
            },
            Self::Transform(op) => match op {
                TransformOp::ClearRange { .. } => "clear_range",
                TransformOp::FillRange { .. } => "fill_range",
                TransformOp::ReplaceInRange { .. } => "replace_in_range",
                TransformOp::WriteMatrix { .. } => "write_matrix",
            },
            Self::Layout(op) => match op {
                SheetLayoutOp::FreezePanes { .. } => "freeze_panes",
                SheetLayoutOp::SetZoom { .. } => "set_zoom",
                SheetLayoutOp::SetGridlines { .. } => "set_gridlines",
                SheetLayoutOp::SetPageMargins { .. } => "set_page_margins",
                SheetLayoutOp::SetPageSetup { .. } => "set_page_setup",
                SheetLayoutOp::SetPrintArea { .. } => "set_print_area",
                SheetLayoutOp::SetPageBreaks { .. } => "set_page_breaks",
            },
            Self::Rules(op) => match op {
                RulesOp::SetDataValidation { .. } => "set_data_validation",
                RulesOp::AddConditionalFormat { .. } => "add_conditional_format",
                RulesOp::SetConditionalFormat { .. } => "set_conditional_format",
                RulesOp::ClearConditionalFormats { .. } => "clear_conditional_formats",
            },
        }
    }

    pub fn risk(&self) -> OperationRisk {
        match self {
            Self::Structure(
                CanonicalStructureOp::DeleteRows { .. }
                | CanonicalStructureOp::DeleteCols { .. }
                | CanonicalStructureOp::DeleteSheet { .. }
                | CanonicalStructureOp::MoveRange { .. },
            )
            | Self::Name(NameWriteOp::DeleteName { .. })
            | Self::Formula(FormulaWriteOp::ReplaceInFormulas { .. }) => OperationRisk::Destructive,
            Self::Transform(TransformOp::ClearRange {
                clear_formulas: true,
                ..
            })
            | Self::ImportAndHelper(
                ImportAndHelperOp::ImportGrid {
                    clear_target: true, ..
                }
                | ImportAndHelperOp::ImportCsv {
                    clear_target: true, ..
                },
            ) => OperationRisk::Destructive,
            Self::Structure(_) | Self::ImportAndHelper(_) => OperationRisk::High,
            _ => OperationRisk::Moderate,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteRequest {
    pub resource_id: ResourceId,
    #[schemars(length(min = 1))]
    pub expected_revision: String,
    pub mode: WriteMode,
    #[serde(default = "default_true")]
    pub atomic: bool,
    #[schemars(length(min = 1, max = 128))]
    pub ops: Vec<WriteOp>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub formula_parse_policy: Option<FormulaParsePolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WriteOpStatus {
    Previewed,
    Staged,
    Applied,
    Failed,
    Skipped,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteOpError {
    pub code: String,
    pub message: String,
    pub path: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteOpResult {
    pub index: usize,
    pub kind: String,
    pub status: WriteOpStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<WriteOpError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteDiff {
    pub change_count: usize,
    pub exact: bool,
    pub precision: String,
    pub changes: Vec<Value>,
    pub effects: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteImpact {
    pub op_kinds: Vec<String>,
    pub risk: OperationRisk,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum WriteResponseData {
    Previewed {
        mode: WriteMode,
        atomic: bool,
        revision_before: String,
        revision_after: String,
        ops_previewed: usize,
        diff: WriteDiff,
        impact: WriteImpact,
        results: Vec<WriteOpResult>,
    },
    Staged {
        mode: WriteMode,
        atomic: bool,
        revision_before: String,
        revision_after: String,
        ops_staged: usize,
        change_id: String,
        diff: WriteDiff,
        impact: WriteImpact,
        results: Vec<WriteOpResult>,
    },
    Applied {
        mode: WriteMode,
        atomic: bool,
        revision_before: String,
        revision_after: String,
        ops_applied: usize,
        diff: WriteDiff,
        impact: WriteImpact,
        results: Vec<WriteOpResult>,
    },
    Partial {
        mode: WriteMode,
        atomic: bool,
        revision_before: String,
        revision_after: String,
        ops_applied: usize,
        diff: WriteDiff,
        impact: WriteImpact,
        results: Vec<WriteOpResult>,
    },
    Failed {
        mode: WriteMode,
        atomic: bool,
        revision_before: String,
        revision_after: String,
        ops_applied: usize,
        diff: WriteDiff,
        impact: WriteImpact,
        results: Vec<WriteOpResult>,
    },
    RolledBack {
        mode: WriteMode,
        atomic: bool,
        revision_before: String,
        revision_after: String,
        ops_applied: usize,
        rolled_back: bool,
        diff: WriteDiff,
        impact: WriteImpact,
        results: Vec<WriteOpResult>,
    },
}

impl WriteResponseData {
    pub fn revision_before(&self) -> &str {
        match self {
            Self::Previewed {
                revision_before, ..
            }
            | Self::Staged {
                revision_before, ..
            }
            | Self::Applied {
                revision_before, ..
            }
            | Self::Partial {
                revision_before, ..
            }
            | Self::Failed {
                revision_before, ..
            }
            | Self::RolledBack {
                revision_before, ..
            } => revision_before,
        }
    }

    pub fn revision_after(&self) -> &str {
        match self {
            Self::Previewed { revision_after, .. }
            | Self::Staged { revision_after, .. }
            | Self::Applied { revision_after, .. }
            | Self::Partial { revision_after, .. }
            | Self::Failed { revision_after, .. }
            | Self::RolledBack { revision_after, .. } => revision_after,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CanonicalStagedBundle {
    #[serde(default)]
    pub label: Option<String>,
    pub base_revision: String,
    pub max_risk: OperationRisk,
    pub atomic: bool,
    pub ops: Vec<WriteOp>,
    pub formula_parse_policy: Option<FormulaParsePolicy>,
}

fn invalid_request(message: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("invalid request: {message}")
}

fn validate_sheet_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.chars().count() > 31
        || value
            .chars()
            .any(|ch| matches!(ch, '[' | ']' | ':' | '*' | '?' | '/' | '\\'))
        || value.starts_with('\'')
        || value.ends_with('\'')
    {
        bail!("invalid sheet name '{value}'");
    }
    Ok(())
}

fn validate_cell(value: &str) -> Result<()> {
    let (column, row) = parse_cell_ref(value)?;
    if column > 16_384 || row > 1_048_576 {
        bail!("cell reference '{value}' exceeds XLSX grid bounds");
    }
    Ok(())
}

fn validate_range(value: &str) -> Result<()> {
    let bare = value.rsplit_once('!').map_or(value, |(_, range)| range);
    if bare.split(':').all(|part| {
        part.trim_matches('$')
            .chars()
            .all(|ch| ch.is_ascii_alphabetic())
    }) {
        return validate_column_range(bare);
    }
    let mut parts = bare.split(':');
    let start = parts.next().unwrap_or_default();
    let end = parts.next().unwrap_or(start);
    if parts.next().is_some() {
        bail!("invalid A1 range '{value}'");
    }
    validate_cell(start)?;
    validate_cell(end)?;
    let (start_col, start_row) = parse_cell_ref(start)?;
    let (end_col, end_row) = parse_cell_ref(end)?;
    if start_col > end_col || start_row > end_row {
        bail!("range '{value}' must be ascending");
    }
    Ok(())
}

fn validate_column(value: &str) -> Result<()> {
    let value = value.trim().trim_matches('$');
    if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_alphabetic()) {
        bail!("invalid column '{value}'");
    }
    let index = umya_spreadsheet::helper::coordinate::column_index_from_string(value);
    if index == 0 || index > 16_384 {
        bail!("column '{value}' exceeds XLSX grid bounds");
    }
    Ok(())
}

fn validate_column_range(value: &str) -> Result<()> {
    let (start, end) = value.split_once(':').unwrap_or((value, value));
    validate_column(start)?;
    validate_column(end)?;
    let start_index =
        umya_spreadsheet::helper::coordinate::column_index_from_string(start.trim_matches('$'));
    let end_index =
        umya_spreadsheet::helper::coordinate::column_index_from_string(end.trim_matches('$'));
    if start_index > end_index {
        bail!("column range '{value}' must be ascending");
    }
    Ok(())
}

fn validate_defined_name(value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || value.len() > 255 {
        bail!("defined name must contain 1..=255 bytes");
    }
    let mut chars = value.chars();
    let first = chars.next().expect("non-empty name");
    if !(first.is_ascii_alphabetic() || matches!(first, '_' | '\\'))
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '\\'))
        || parse_cell_ref(value).is_ok()
    {
        bail!("invalid defined name '{value}'");
    }
    Ok(())
}

fn validate_row_range(value: &str) -> Result<()> {
    let (start, end) = value
        .split_once(':')
        .ok_or_else(|| anyhow!("row range must use START:END notation"))?;
    let start = start.parse::<u32>()?;
    let end = end.parse::<u32>()?;
    if start == 0 || end < start || end > 1_048_576 {
        bail!("invalid row range '{value}'");
    }
    Ok(())
}

fn validate_static_fields(value: &Value) -> Result<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                validate_static_fields(value)?;
            }
        }
        Value::Object(object) => {
            for (key, value) in object {
                match (key.as_str(), value) {
                    (
                        "sheet" | "sheet_name" | "dest_sheet_name" | "scope_sheet_name"
                        | "old_name" | "new_name",
                        Value::String(value),
                    ) => validate_sheet_name(value)?,
                    (
                        "anchor" | "dest_anchor" | "anchor_cell" | "top_left_cell",
                        Value::String(value),
                    ) => validate_cell(value)?,
                    ("range" | "target_range" | "src_range", Value::String(value)) => {
                        validate_range(value)?
                    }
                    ("at_col" | "start_col", Value::String(value)) => validate_column(value)?,
                    ("source_rows", Value::String(value)) => validate_row_range(value)?,
                    ("cells", Value::Object(cells)) => {
                        for address in cells.keys() {
                            validate_cell(address)?;
                        }
                    }
                    ("cells", Value::Array(cells)) => {
                        for address in cells.iter().filter_map(Value::as_str) {
                            validate_cell(address)?;
                        }
                    }
                    ("columns", Value::String(value)) => validate_column_range(value)?,
                    _ => {}
                }
                validate_static_fields(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_request(request: &WriteRequest) -> Result<()> {
    let payload_bytes = serde_json::to_vec(request)?.len();
    if request.ops.is_empty() || request.ops.len() > MAX_WRITE_OPS {
        return Err(invalid_request(format!(
            "ops must contain 1..={MAX_WRITE_OPS} operations"
        )));
    }
    if request.expected_revision.trim().is_empty() {
        return Err(invalid_request("expected_revision must not be empty"));
    }
    if payload_bytes > MAX_WRITE_PAYLOAD_BYTES {
        return Err(invalid_request(format!(
            "request exceeds {MAX_WRITE_PAYLOAD_BYTES} bytes"
        )));
    }
    if request.mode == WriteMode::Stage && !request.atomic {
        return Err(invalid_request(
            "stage requires atomic:true; non-atomic staged replay is unsupported",
        ));
    }
    let mut cells = 0usize;
    for (index, op) in request.ops.iter().enumerate() {
        let invalid =
            |message: &str| invalid_request(format!("ops[{index}] ({}): {message}", op.kind()));
        validate_static_fields(&serde_json::to_value(op)?)
            .map_err(|error| invalid(&error.to_string()))?;
        match op {
            WriteOp::SetCells(value) => {
                if value.cells.is_empty() {
                    return Err(invalid("cells must not be empty"));
                }
                cells = cells.saturating_add(value.cells.len());
                for content in value.cells.values() {
                    if let CellContent::Formula { formula } = content {
                        crate::model::validate_formula(formula)
                            .map_err(|error| invalid(&format!("invalid formula: {error}")))?;
                    }
                }
            }
            WriteOp::Structure(CanonicalStructureOp::InsertRows { at_row, count, .. })
                if *at_row == 0 || *count == 0 || at_row.saturating_add(*count) > 1_048_577 =>
            {
                return Err(invalid("row and count values exceed XLSX bounds"));
            }
            WriteOp::Structure(CanonicalStructureOp::DeleteRows {
                start_row, count, ..
            }) if *start_row == 0
                || *count == 0
                || start_row.saturating_add(*count) > 1_048_577 =>
            {
                return Err(invalid("row and count values exceed XLSX bounds"));
            }
            WriteOp::Structure(
                CanonicalStructureOp::InsertCols { count, .. }
                | CanonicalStructureOp::DeleteCols { count, .. },
            ) if *count == 0 || *count > 16_384 => {
                return Err(invalid("column count exceeds XLSX bounds"));
            }
            WriteOp::Structure(CanonicalStructureOp::RenameSheet { old_name, new_name }) => {
                validate_sheet_name(old_name).map_err(|error| invalid(&error.to_string()))?;
                validate_sheet_name(new_name).map_err(|error| invalid(&error.to_string()))?;
                if old_name == new_name {
                    return Err(invalid("old_name and new_name must differ"));
                }
            }
            WriteOp::Structure(CanonicalStructureOp::CreateSheet { name, position }) => {
                validate_sheet_name(name).map_err(|error| invalid(&error.to_string()))?;
                if position.is_some_and(|position| position > 1_000) {
                    return Err(invalid("sheet position exceeds planner bound"));
                }
            }
            WriteOp::Structure(CanonicalStructureOp::DeleteSheet { name }) => {
                validate_sheet_name(name).map_err(|error| invalid(&error.to_string()))?
            }
            WriteOp::ImportAndHelper(ImportAndHelperOp::AppendRows {
                region_id,
                table_name,
                rows,
                ..
            }) => {
                if region_id.is_some() == table_name.is_some()
                    || rows.is_empty()
                    || rows.iter().any(Vec::is_empty)
                {
                    return Err(invalid(
                        "append_rows requires non-empty rows and exactly one of region_id or table_name",
                    ));
                }
                if table_name
                    .as_ref()
                    .is_some_and(|name| name.trim().is_empty())
                {
                    return Err(invalid("table_name must not be empty"));
                }
                for formula in rows.iter().flatten().filter_map(|cell| match cell {
                    Some(MatrixCell::Formula(formula)) => Some(formula),
                    _ => None,
                }) {
                    crate::model::validate_formula(formula)
                        .map_err(|error| invalid(&format!("invalid formula: {error}")))?;
                }
                cells = cells.saturating_add(rows.iter().map(Vec::len).sum::<usize>());
            }
            WriteOp::ImportAndHelper(ImportAndHelperOp::CloneRow {
                source_row,
                before,
                after,
                insert_at,
                count,
                ..
            }) => {
                let anchors = [*before, *after, *insert_at];
                let selected = anchors.into_iter().flatten().collect::<Vec<_>>();
                if *source_row == 0
                    || *count == 0
                    || selected.len() != 1
                    || selected[0] == 0
                    || selected[0].saturating_add(*count) > 1_048_577
                {
                    return Err(invalid(
                        "clone row fields exceed XLSX bounds or do not select exactly one anchor",
                    ));
                }
            }
            WriteOp::ImportAndHelper(ImportAndHelperOp::CloneRowBand {
                before,
                after,
                insert_at,
                repeat,
                source_rows,
                ..
            }) => {
                let anchors = [*before, *after, *insert_at];
                let selected = anchors.into_iter().flatten().collect::<Vec<_>>();
                if selected.len() != 1 || selected[0] == 0 || *repeat == 0 {
                    return Err(invalid(
                        "exactly one positive clone anchor and repeat are required",
                    ));
                }
                validate_row_range(source_rows).map_err(|error| invalid(&error.to_string()))?;
                let (start, end) = source_rows.split_once(':').expect("validated row range");
                let band = end.parse::<u32>().expect("validated end")
                    - start.parse::<u32>().expect("validated start")
                    + 1;
                if selected[0]
                    .checked_add(band.saturating_mul(*repeat))
                    .is_none_or(|end| end > 1_048_577)
                {
                    return Err(invalid("cloned row band exceeds XLSX bounds"));
                }
            }
            WriteOp::Formula(FormulaWriteOp::ReplaceInFormulas { find, regex, .. }) => {
                if find.is_empty() {
                    return Err(invalid("find must not be empty"));
                }
                if *regex {
                    regex::Regex::new(find)
                        .map_err(|error| invalid(&format!("invalid regex: {error}")))?;
                }
            }
            WriteOp::ImportAndHelper(ImportAndHelperOp::ImportCsv {
                anchor,
                csv,
                header,
                ..
            }) => {
                let records = csv_records(csv).map_err(|error| invalid(&error.to_string()))?;
                let (anchor_col, anchor_row) =
                    parse_cell_ref(anchor).map_err(|error| invalid(&error.to_string()))?;
                let data_rows = records.len().saturating_sub(usize::from(*header));
                let width = records.iter().map(Vec::len).max().unwrap_or(0);
                if anchor_row.saturating_add(data_rows as u32) > 1_048_577
                    || anchor_col.saturating_add(width as u32) > 16_385
                {
                    return Err(invalid("CSV exceeds XLSX grid bounds"));
                }
                cells = cells.saturating_add(records.iter().map(Vec::len).sum::<usize>());
            }
            WriteOp::ImportAndHelper(ImportAndHelperOp::ImportGrid { anchor, grid, .. }) => {
                validate_cell(&grid.anchor).map_err(|error| invalid(&error.to_string()))?;
                for merge in &grid.merges {
                    validate_range(merge).map_err(|error| invalid(&error.to_string()))?;
                }
                let (target_col, target_row) =
                    parse_cell_ref(anchor).map_err(|error| invalid(&error.to_string()))?;
                if grid.rows.iter().flat_map(|row| &row.cells).any(|cell| {
                    target_row
                        .checked_add(cell.offset[0])
                        .is_none_or(|row| row > 1_048_576)
                        || target_col
                            .checked_add(cell.offset[1])
                            .is_none_or(|col| col > 16_384)
                }) {
                    return Err(invalid("grid cell offset exceeds XLSX bounds"));
                }
                if grid.columns.iter().any(|column| {
                    target_col
                        .checked_add(column.offset)
                        .is_none_or(|col| col > 16_384)
                        || !column.width_chars.is_finite()
                        || column.width_chars < 0.0
                        || column.width_chars > 255.0
                }) {
                    return Err(invalid("grid column hint is invalid"));
                }
                cells = cells
                    .saturating_add(grid.rows.iter().map(|row| row.cells.len()).sum::<usize>());
            }
            WriteOp::Name(NameWriteOp::DefineName {
                name,
                refers_to,
                scope,
                scope_sheet_name,
            }) => {
                validate_defined_name(name).map_err(|error| invalid(&error.to_string()))?;
                if refers_to.trim().is_empty() {
                    return Err(invalid("refers_to must not be empty"));
                }
                if matches!(scope, NameScope::Sheet) && scope_sheet_name.is_none() {
                    return Err(invalid("sheet-scoped names require scope_sheet_name"));
                }
            }
            WriteOp::Name(NameWriteOp::UpdateName {
                name,
                refers_to,
                scope,
                scope_sheet_name,
            }) => {
                validate_defined_name(name).map_err(|error| invalid(&error.to_string()))?;
                if matches!(scope, Some(NameScope::Sheet)) && scope_sheet_name.is_none() {
                    return Err(invalid("sheet-scoped names require scope_sheet_name"));
                }
                if refers_to
                    .as_ref()
                    .is_some_and(|value| value.trim().is_empty())
                {
                    return Err(invalid("refers_to must not be empty"));
                }
            }
            WriteOp::Name(NameWriteOp::DeleteName {
                name,
                scope,
                scope_sheet_name,
            }) => {
                validate_defined_name(name).map_err(|error| invalid(&error.to_string()))?;
                if matches!(scope, Some(NameScope::Sheet)) && scope_sheet_name.is_none() {
                    return Err(invalid("sheet-scoped names require scope_sheet_name"));
                }
            }
            WriteOp::Style(StyleWriteOp::Style {
                target: StyleTarget::Cells { cells: addresses },
                ..
            }) if addresses.is_empty() => return Err(invalid("style cells must not be empty")),
            WriteOp::Transform(
                TransformOp::ClearRange {
                    target: TransformTarget::Cells { cells: addresses },
                    ..
                }
                | TransformOp::FillRange {
                    target: TransformTarget::Cells { cells: addresses },
                    ..
                }
                | TransformOp::ReplaceInRange {
                    target: TransformTarget::Cells { cells: addresses },
                    ..
                },
            ) if addresses.is_empty() => return Err(invalid("target cells must not be empty")),
            WriteOp::Transform(TransformOp::FillRange {
                value,
                is_formula: true,
                ..
            }) => crate::model::validate_formula(value)
                .map_err(|error| invalid(&format!("invalid formula: {error}")))?,
            WriteOp::Transform(TransformOp::ReplaceInRange { find, .. }) if find.is_empty() => {
                return Err(invalid("find must not be empty"));
            }
            WriteOp::Transform(TransformOp::WriteMatrix { anchor, rows, .. }) => {
                if rows.is_empty() || rows.iter().any(Vec::is_empty) {
                    return Err(invalid("matrix rows must not be empty"));
                }
                let (anchor_col, anchor_row) =
                    parse_cell_ref(anchor).map_err(|error| invalid(&error.to_string()))?;
                let width = rows.iter().map(Vec::len).max().unwrap_or(0) as u32;
                if anchor_row.saturating_add(rows.len() as u32) > 1_048_577
                    || anchor_col.saturating_add(width) > 16_385
                {
                    return Err(invalid("matrix exceeds XLSX grid bounds"));
                }
                cells = cells.saturating_add(rows.iter().map(Vec::len).sum::<usize>());
                for formula in rows.iter().flatten().filter_map(|cell| match cell {
                    Some(MatrixCell::Formula(formula)) => Some(formula),
                    _ => None,
                }) {
                    crate::model::validate_formula(formula)
                        .map_err(|error| invalid(&format!("invalid formula: {error}")))?;
                }
            }
            WriteOp::Column(ColumnWriteOp::ColumnSize { size, .. }) => match size {
                ColumnSizeSpec::Width { width_chars }
                    if !width_chars.is_finite() || *width_chars < 0.0 || *width_chars > 255.0 =>
                {
                    return Err(invalid("width_chars must be finite and between 0 and 255"));
                }
                ColumnSizeSpec::Auto {
                    min_width_chars,
                    max_width_chars,
                } => {
                    if min_width_chars
                        .is_some_and(|value| !value.is_finite() || !(0.0..=255.0).contains(&value))
                        || max_width_chars.is_some_and(|value| {
                            !value.is_finite() || !(0.0..=255.0).contains(&value)
                        })
                        || matches!((min_width_chars, max_width_chars), (Some(min), Some(max)) if min > max)
                    {
                        return Err(invalid(
                            "auto width bounds must be finite, ordered, and between 0 and 255",
                        ));
                    }
                }
                _ => {}
            },
            WriteOp::Layout(SheetLayoutOp::FreezePanes {
                freeze_rows,
                freeze_cols,
                ..
            }) if *freeze_rows > 1_048_576 || *freeze_cols > 16_384 => {
                return Err(invalid("freeze panes exceed XLSX grid bounds"));
            }
            WriteOp::Layout(SheetLayoutOp::SetZoom { zoom_percent, .. })
                if !(10..=400).contains(zoom_percent) =>
            {
                return Err(invalid("zoom_percent must be between 10 and 400"));
            }
            WriteOp::Layout(SheetLayoutOp::SetPageMargins {
                left,
                right,
                top,
                bottom,
                header,
                footer,
                ..
            }) => {
                if [
                    Some(*left),
                    Some(*right),
                    Some(*top),
                    Some(*bottom),
                    *header,
                    *footer,
                ]
                .into_iter()
                .flatten()
                .any(|value| !value.is_finite() || value < 0.0)
                {
                    return Err(invalid("page margins must be finite and non-negative"));
                }
            }
            WriteOp::Layout(SheetLayoutOp::SetPageSetup {
                fit_to_width,
                fit_to_height,
                scale_percent,
                ..
            }) => {
                if fit_to_width.is_some_and(|value| value == 0)
                    || fit_to_height.is_some_and(|value| value == 0)
                    || scale_percent.is_some_and(|value| !(10..=400).contains(&value))
                {
                    return Err(invalid("invalid page setup fit or scale value"));
                }
            }
            WriteOp::Layout(SheetLayoutOp::SetPageBreaks {
                row_breaks,
                col_breaks,
                ..
            }) => {
                if row_breaks
                    .iter()
                    .any(|value| *value == 0 || *value > 1_048_576)
                    || col_breaks
                        .iter()
                        .any(|value| *value == 0 || *value > 16_384)
                {
                    return Err(invalid("page break exceeds XLSX grid bounds"));
                }
            }
            WriteOp::Rules(
                RulesOp::AddConditionalFormat { rule, .. }
                | RulesOp::SetConditionalFormat { rule, .. },
            ) => {
                let formula = match rule {
                    ConditionalFormatRuleSpec::CellIs { formula, .. }
                    | ConditionalFormatRuleSpec::Expression { formula } => formula,
                };
                if formula.trim().is_empty() {
                    return Err(invalid("conditional format formula must not be empty"));
                }
            }
            WriteOp::Formula(FormulaWriteOp::FormulaPattern { base_formula, .. }) => {
                crate::model::validate_formula(base_formula)
                    .map_err(|error| invalid(&format!("invalid formula: {error}")))?
            }
            _ => {}
        }
        if cells > MAX_WRITE_CELLS {
            return Err(invalid(&format!("request exceeds {MAX_WRITE_CELLS} cells")));
        }
    }
    Ok(())
}

fn worst_risk(ops: &[WriteOp]) -> OperationRisk {
    ops.iter()
        .map(WriteOp::risk)
        .max_by_key(|risk| match risk {
            OperationRisk::Low => 0,
            OperationRisk::Moderate => 1,
            OperationRisk::High => 2,
            OperationRisk::Destructive => 3,
        })
        .unwrap_or(OperationRisk::Low)
}

fn temp_copy(path: &Path) -> Result<crate::hostfs::NamedTempFile> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file = crate::hostfs::Builder::new()
        .prefix(".canonical-write-")
        .suffix(".xlsx")
        .tempfile_in(parent)?;
    fs::copy(path, file.path())?;
    Ok(file)
}

fn swap_temp(temp: crate::hostfs::NamedTempFile, target: &Path) -> Result<()> {
    let (_file, path) = temp.keep()?;
    if let Err(error) = fs::rename(&path, target) {
        let _ = fs::remove_file(&path);
        return Err(error.into());
    }
    Ok(())
}

fn diff_bytes(before: &[u8], after: &[u8]) -> Result<WriteDiff> {
    let changes = crate::diff::calculate_changeset_bytes(before, after, None)?;
    changes_to_write_diff(changes)
}

fn changes_to_write_diff(changes: Vec<crate::diff::Change>) -> Result<WriteDiff> {
    let values = changes
        .into_iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(WriteDiff {
        change_count: values.len(),
        exact: true,
        precision: "exact_baseline_to_result".to_string(),
        changes: values,
        effects: Vec::new(),
    })
}

fn add_effect_manifest(diff: &mut WriteDiff, results: &[WriteOpResult]) {
    for result in results {
        if result.kind == "set_cells" {
            continue;
        }
        if let Some(effect) = &result.detail {
            diff.effects.push(json!({
                "kind": "operation_effect",
                "op_index": result.index,
                "op_kind": result.kind,
                "effect": effect,
            }));
        }
    }
    if !diff.effects.is_empty() {
        diff.exact = false;
        diff.precision = "exact_cells_names_tables_plus_declared_effects".to_string();
    }
}

pub(crate) fn staged_summary(op_kinds: Vec<String>, ops: usize, changes: usize) -> ChangeSummary {
    let mut summary = ChangeSummary { op_kinds, ..ChangeSummary::default() };
    summary.counts.insert("ops_staged".to_string(), ops as u64);
    summary.counts.insert("preview_change_items".to_string(), changes as u64);
    summary
}

fn summary_detail(summary: &ChangeSummary) -> Result<Value> {
    Ok(serde_json::to_value(summary)?)
}

fn set_cells_to_ops(op: &SetCellsOp) -> Vec<TransformOp> {
    op.cells
        .iter()
        .map(|(address, content)| {
            let cell = match content {
                CellContent::Value { value } => MatrixCell::Value(
                    serde_json::to_value(value).expect("cell primitive serializes"),
                ),
                CellContent::Formula { formula } => MatrixCell::Formula(formula.clone()),
            };
            TransformOp::WriteMatrix {
                sheet_name: op.sheet_name.clone(),
                anchor: address.clone(),
                rows: vec![vec![Some(cell)]],
                overwrite_formulas: op.overwrite_formulas,
            }
        })
        .collect()
}

fn csv_records(raw: &str) -> Result<Vec<Vec<String>>> {
    let mut records = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = raw.chars().peekable();
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(ch);
            }
        } else {
            match ch {
                '"' if field.is_empty() => quoted = true,
                ',' => {
                    row.push(std::mem::take(&mut field));
                }
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut row));
                }
                '\r' if chars.peek() == Some(&'\n') => {}
                '\r' => {
                    row.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut row));
                }
                _ => field.push(ch),
            }
        }
    }
    if quoted {
        bail!("unterminated quoted CSV field");
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        records.push(row);
    }
    Ok(records)
}
fn csv_value(value: String) -> Value {
    if value.is_empty() {
        Value::Null
    } else if value.eq_ignore_ascii_case("true") {
        Value::Bool(true)
    } else if value.eq_ignore_ascii_case("false") {
        Value::Bool(false)
    } else if let Ok(number) = value.parse::<i64>() {
        number.into()
    } else if let Ok(number) = value.parse::<f64>() {
        json!(number)
    } else {
        Value::String(value)
    }
}

fn parse_cell_ref(value: &str) -> Result<(u32, u32)> {
    let value = value.trim().replace('$', "");
    let split = value
        .find(|character: char| character.is_ascii_digit())
        .ok_or_else(|| anyhow!("invalid cell reference '{value}'"))?;
    let (column, row) = value.split_at(split);
    if column.is_empty() || row.is_empty() || !column.chars().all(|ch| ch.is_ascii_alphabetic()) {
        bail!("invalid cell reference '{value}'");
    }
    let column = umya_spreadsheet::helper::coordinate::column_index_from_string(column);
    let row = row
        .parse::<u32>()
        .map_err(|_| anyhow!("invalid cell reference '{value}'"))?;
    if column == 0 || row == 0 {
        bail!("invalid cell reference '{value}'");
    }
    Ok((column, row))
}

fn apply_grid_to_workbook(
    book: &mut umya_spreadsheet::Spreadsheet,
    sheet_name: &str,
    anchor: &str,
    grid: &GridPayload,
    clear_target: bool,
) -> Result<Value> {
    let (anchor_col, anchor_row) = parse_cell_ref(anchor)?;
    let mut max_col = anchor_col;
    let mut max_row = anchor_row;
    let mut rows: Vec<Vec<Option<MatrixCell>>> = Vec::new();
    let mut styles = Vec::new();
    for grid_row in &grid.rows {
        for cell in &grid_row.cells {
            let row = anchor_row + cell.offset[0];
            let col = anchor_col + cell.offset[1];
            max_row = max_row.max(row);
            max_col = max_col.max(col);
            let r = cell.offset[0] as usize;
            let c = cell.offset[1] as usize;
            while rows.len() <= r {
                rows.push(Vec::new());
            }
            while rows[r].len() <= c {
                rows[r].push(None);
            }
            rows[r][c] = cell
                .f
                .as_ref()
                .map(|formula| MatrixCell::Formula(formula.clone()))
                .or_else(|| {
                    cell.v
                        .as_ref()
                        .map(|value| MatrixCell::Value(value.clone()))
                });
            let mut patch = cell.style.clone().unwrap_or_default();
            if let Some(format) = &cell.fmt {
                patch.number_format = Some(Some(format.clone()));
            }
            if cell.style.is_some() || cell.fmt.is_some() {
                styles.push(StyleOp {
                    sheet_name: sheet_name.to_string(),
                    target: StyleTarget::Cells {
                        cells: vec![crate::utils::cell_address(col, row)],
                    },
                    patch,
                    op_mode: None,
                });
            }
        }
    }
    let footprint = format!(
        "{}:{}",
        crate::utils::cell_address(anchor_col, anchor_row),
        crate::utils::cell_address(max_col, max_row)
    );
    if clear_target {
        apply_structure_ops_to_workbook(
            book,
            &[StructureOp::UnmergeCells {
                sheet_name: sheet_name.to_string(),
                target_range: footprint.clone(),
            }],
            FormulaParsePolicy::Off,
        )?;
        apply_transform_ops_to_workbook(
            book,
            &[TransformOp::ClearRange {
                sheet_name: sheet_name.to_string(),
                target: TransformTarget::Range {
                    range: footprint.clone(),
                },
                clear_values: true,
                clear_formulas: true,
            }],
        )?;
        apply_style_ops_to_workbook(
            book,
            &[StyleOp {
                sheet_name: sheet_name.to_string(),
                target: StyleTarget::Range {
                    range: footprint.clone(),
                },
                patch: StylePatch {
                    font: Some(None),
                    fill: Some(None),
                    borders: Some(None),
                    alignment: Some(None),
                    number_format: Some(None),
                },
                op_mode: None,
            }],
        )?;
    }
    if !grid.merges.is_empty() {
        let (source_col, source_row) = parse_cell_ref(&grid.anchor)?;
        let ops = grid
            .merges
            .iter()
            .map(|range| {
                let (start, end) = range.split_once(':').unwrap_or((range, range));
                let (start_col, start_row) = parse_cell_ref(start)?;
                let (end_col, end_row) = parse_cell_ref(end)?;
                if start_col < source_col || start_row < source_row {
                    bail!(
                        "grid merge '{range}' begins before source anchor '{}'; cannot translate",
                        grid.anchor
                    );
                }
                let translated = format!(
                    "{}:{}",
                    crate::utils::cell_address(
                        anchor_col + start_col - source_col,
                        anchor_row + start_row - source_row
                    ),
                    crate::utils::cell_address(
                        anchor_col + end_col - source_col,
                        anchor_row + end_row - source_row
                    ),
                );
                Ok(StructureOp::MergeCells {
                    sheet_name: sheet_name.to_string(),
                    target_range: translated,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        apply_structure_ops_to_workbook(book, &ops, FormulaParsePolicy::Off)?;
    }
    for column in &grid.columns {
        let name = crate::utils::column_number_to_name(anchor_col + column.offset);
        apply_column_size_ops_to_workbook(
            book,
            sheet_name,
            &[ColumnSizeOp {
                target: ColumnTarget::Columns {
                    range: format!("{name}:{name}"),
                },
                size: ColumnSizeSpec::Width {
                    width_chars: column.width_chars,
                },
            }],
        )?;
    }
    apply_transform_ops_to_workbook(
        book,
        &[TransformOp::WriteMatrix {
            sheet_name: sheet_name.to_string(),
            anchor: anchor.to_string(),
            rows,
            overwrite_formulas: true,
        }],
    )?;
    if !styles.is_empty() {
        apply_style_ops_to_workbook(book, &styles)?;
    }
    Ok(
        json!({"sheet_name":sheet_name,"footprint":footprint,"cells":grid.rows.iter().map(|row| row.cells.len()).sum::<usize>()}),
    )
}

pub(crate) fn apply_write_op_to_workbook(
    book: &mut umya_spreadsheet::Spreadsheet,
    op: &WriteOp,
    policy: FormulaParsePolicy,
) -> Result<Value> {
    match op {
        WriteOp::SetCells(op) => {
            let result = apply_transform_ops_to_workbook(book, &set_cells_to_ops(op))?;
            summary_detail(&result.summary)
        }
        WriteOp::Transform(op) => {
            let result = apply_transform_ops_to_workbook(book, std::slice::from_ref(op))?;
            summary_detail(&result.summary)
        }
        WriteOp::Structure(op) => {
            let result = apply_structure_ops_to_workbook(book, &[StructureOp::from(op)], policy)?;
            summary_detail(&result.summary)
        }
        WriteOp::Style(StyleWriteOp::Style {
            sheet_name,
            target,
            patch,
            op_mode,
        }) => {
            let result = apply_style_ops_to_workbook(
                book,
                &[StyleOp {
                    sheet_name: sheet_name.clone(),
                    target: target.clone(),
                    patch: patch.clone(),
                    op_mode: *op_mode,
                }],
            )?;
            summary_detail(&result.summary)
        }
        WriteOp::Column(ColumnWriteOp::ColumnSize {
            sheet_name,
            target,
            size,
        }) => {
            let result = apply_column_size_ops_to_workbook(
                book,
                sheet_name,
                &[ColumnSizeOp {
                    target: target.clone(),
                    size: size.clone(),
                }],
            )?;
            summary_detail(&result.summary)
        }
        WriteOp::Layout(op) => {
            let result = apply_sheet_layout_ops_to_workbook(book, std::slice::from_ref(op))?;
            summary_detail(&result.summary)
        }
        WriteOp::Rules(op) => {
            let result = apply_rules_ops_to_workbook(book, std::slice::from_ref(op), policy)?;
            summary_detail(&result.summary)
        }
        WriteOp::Formula(FormulaWriteOp::FormulaPattern {
            sheet_name,
            target_range,
            anchor_cell,
            base_formula,
            fill_direction,
            relative_mode,
        }) => {
            let result = apply_formula_pattern_ops_to_workbook(
                book,
                &[ApplyFormulaPatternOpInput {
                    sheet_name: sheet_name.clone(),
                    target_range: target_range.clone(),
                    anchor_cell: anchor_cell.clone(),
                    base_formula: base_formula.clone(),
                    fill_direction: *fill_direction,
                    relative_mode: *relative_mode,
                }],
            )?;
            summary_detail(&result.summary)
        }
        WriteOp::Formula(FormulaWriteOp::ReplaceInFormulas {
            sheet_name,
            find,
            replace,
            range,
            regex,
            case_sensitive,
        }) => {
            let result = apply_replace_in_formulas_to_workbook(
                book,
                &ReplaceInFormulasOp {
                    sheet_name: sheet_name.clone(),
                    find: find.clone(),
                    replace: replace.clone(),
                    range: range.clone(),
                    regex: *regex,
                    case_sensitive: *case_sensitive,
                },
                policy,
            )?;
            Ok(
                json!({"formulas_checked":result.formulas_checked,"formulas_changed":result.formulas_changed,"samples":result.samples,"warnings":result.warnings}),
            )
        }
        WriteOp::Name(NameWriteOp::DefineName {
            name,
            refers_to,
            scope,
            scope_sheet_name,
        }) => {
            let mut session = crate::core::session::WorkbookSession::from_spreadsheet(
                std::mem::replace(book, umya_spreadsheet::new_file()),
            );
            let result = session.define_name(
                name,
                refers_to,
                Some(match scope {
                    NameScope::Workbook => "workbook",
                    NameScope::Sheet => "sheet",
                }),
                scope_sheet_name.as_deref(),
            );
            *book = session.into_spreadsheet();
            let result = result?;
            Ok(json!({"name":name,"defined":true,"name_result":result}))
        }
        WriteOp::Name(NameWriteOp::UpdateName {
            name,
            refers_to,
            scope,
            scope_sheet_name,
        }) => {
            let mut session = crate::core::session::WorkbookSession::from_spreadsheet(
                std::mem::replace(book, umya_spreadsheet::new_file()),
            );
            let result = session.update_name(
                name,
                refers_to.as_deref(),
                scope.map(|value| match value {
                    NameScope::Workbook => "workbook",
                    NameScope::Sheet => "sheet",
                }),
                scope_sheet_name.as_deref(),
            );
            *book = session.into_spreadsheet();
            let result = result?;
            Ok(json!({
                "name": name,
                "previous_refers_to": result.previous_refers_to,
                "scope": result.scope_kind,
                "scope_sheet_name": result.scope_sheet_name,
                "name_result": result,
            }))
        }
        WriteOp::Name(NameWriteOp::DeleteName {
            name,
            scope,
            scope_sheet_name,
        }) => {
            let mut session = crate::core::session::WorkbookSession::from_spreadsheet(
                std::mem::replace(book, umya_spreadsheet::new_file()),
            );
            let result = session.delete_name(
                name,
                scope.map(|value| match value {
                    NameScope::Workbook => "workbook",
                    NameScope::Sheet => "sheet",
                }),
                scope_sheet_name.as_deref(),
            );
            *book = session.into_spreadsheet();
            let result = result?;
            Ok(json!({"name":name,"deleted":true,"name_result":result}))
        }
        WriteOp::ImportAndHelper(ImportAndHelperOp::ImportGrid {
            sheet_name,
            anchor,
            grid,
            clear_target,
        }) => apply_grid_to_workbook(book, sheet_name, anchor, grid, *clear_target),
        WriteOp::ImportAndHelper(ImportAndHelperOp::ImportCsv {
            sheet_name,
            anchor,
            csv,
            header,
            clear_target,
        }) => {
            let mut records = csv_records(csv)?;
            if *header && !records.is_empty() {
                records.remove(0);
            }
            let row_count = records.len() as u32;
            let column_count = records.iter().map(Vec::len).max().unwrap_or(0) as u32;
            if *clear_target && row_count > 0 && column_count > 0 {
                let (column, row) = parse_cell_ref(anchor)?;
                let range = format!(
                    "{}:{}",
                    crate::utils::cell_address(column, row),
                    crate::utils::cell_address(column + column_count - 1, row + row_count - 1),
                );
                apply_transform_ops_to_workbook(
                    book,
                    &[TransformOp::ClearRange {
                        sheet_name: sheet_name.clone(),
                        target: TransformTarget::Range { range },
                        clear_values: true,
                        clear_formulas: true,
                    }],
                )?;
            }
            let rows = records
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|cell| {
                            let value = csv_value(cell);
                            (!value.is_null()).then_some(MatrixCell::Value(value))
                        })
                        .collect()
                })
                .collect();
            let result = apply_transform_ops_to_workbook(
                book,
                &[TransformOp::WriteMatrix {
                    sheet_name: sheet_name.clone(),
                    anchor: anchor.clone(),
                    rows,
                    overwrite_formulas: true,
                }],
            )?;
            summary_detail(&result.summary)
        }
        WriteOp::ImportAndHelper(ImportAndHelperOp::AppendRows {
            sheet_name,
            region_id,
            table_name,
            rows,
            footer_policy,
        }) => crate::core::write_planner::apply_append_rows_to_workbook(
            book,
            sheet_name,
            *region_id,
            table_name.as_deref(),
            *footer_policy,
            rows.clone(),
        ),
        WriteOp::ImportAndHelper(ImportAndHelperOp::CloneRow {
            sheet_name,
            source_row,
            before,
            after,
            insert_at,
            count,
            expand_adjacent_sums,
            patch_targets,
            merge_policy,
        }) => crate::core::write_planner::apply_clone_row_to_workbook(
            book,
            sheet_name,
            *source_row,
            *before,
            *after,
            *insert_at,
            *count,
            *expand_adjacent_sums,
            *patch_targets,
            *merge_policy,
        ),
        WriteOp::ImportAndHelper(ImportAndHelperOp::CloneRowBand {
            sheet_name,
            source_rows,
            before,
            after,
            insert_at,
            repeat,
            expand_adjacent_sums,
            patch_targets,
            merge_policy,
        }) => crate::core::write_planner::apply_clone_row_band_to_workbook(
            book,
            sheet_name,
            source_rows,
            *before,
            *after,
            *insert_at,
            *repeat,
            *expand_adjacent_sums,
            *patch_targets,
            *merge_policy,
        ),
    }
}

pub(crate) fn apply_bundle_atomically_to_path(
    path: &Path,
    bundle: &CanonicalStagedBundle,
) -> Result<usize> {
    if !bundle.atomic {
        return Err(invalid_request(
            "canonical staged bundles must preserve atomic semantics",
        ));
    }
    let current = hash_file_sha256_hex(path)?;
    if current != bundle.base_revision {
        bail!(
            "revision conflict: staged write expected {}, current {}",
            bundle.base_revision,
            current
        );
    }
    let original_bytes = fs::read(path)?;
    let original =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(&original_bytes), true)?;
    let policy = bundle
        .formula_parse_policy
        .unwrap_or(FormulaParsePolicy::Warn);
    let (candidate, results, failure) = apply_atomic_candidate(&original, &bundle.ops, policy);
    if let Some(index) = failure {
        bail!(
            "staged write op {index} failed: {}",
            results[index]
                .error
                .as_ref()
                .map(|error| error.message.as_str())
                .unwrap_or("unknown failure")
        );
    }
    let bytes = workbook_bytes(&candidate)?;
    let temp = temp_copy(path)?;
    fs::write(temp.path(), bytes)?;
    swap_temp(temp, path)?;
    Ok(bundle.ops.len())
}

/// Canonical transaction/catalog owner for one portable resident workbook.
/// Host persistence is attached at the commit boundary in M2; adapters remain M3.
pub struct ResidentWriteSession {
    resource_id: String,
    workbook: crate::recalc::ResidentWorkbook,
    staged: BTreeMap<String, CanonicalStagedBundle>,
    catalog_generation: u64,
    poisoned: Option<String>,
    base_sha256: String,
    base_bytes: std::sync::Arc<[u8]>,
}

impl ResidentWriteSession {
    pub fn from_bytes(resource_id: impl Into<String>, bytes: &[u8]) -> Result<Self> {
        let resource_id = resource_id.into();
        let parsed: ResourceId = serde_json::from_value(json!(resource_id.clone()))?;
        if !parsed.as_str().starts_with("session:") {
            return Err(invalid_request(
                "resident resources require a session: identifier",
            ));
        }
        Ok(Self {
            resource_id,
            workbook: crate::recalc::ResidentWorkbook::from_bytes(bytes)?,
            staged: BTreeMap::new(),
            catalog_generation: 0,
            poisoned: None,
            base_sha256: crate::utils::hash_bytes_sha256_hex(bytes),
            base_bytes: bytes.into(),
        })
    }

    pub(crate) fn immutable_base_sha256(&self) -> &str { &self.base_sha256 }
    pub(crate) fn immutable_base(&self) -> std::sync::Arc<[u8]> { self.base_bytes.clone() }

    fn validate_base(&self, bytes: &[u8]) -> Result<()> {
        self.ensure_usable()?;
        if bytes != self.base_bytes.as_ref() {
            bail!("resident history base identity mismatch");
        }
        Ok(())
    }

    pub fn revision(&self) -> String {
        self.workbook.state_revision_id()
    }
    pub(crate) fn poison_after_committed_outcome(&mut self, error: &str) {
        self.poisoned = Some(format!("committed canonical outcome requires recovery: {error}"));
    }

    pub fn poison_reason(&self) -> Option<&str> {
        self.poisoned.as_deref()
    }
    pub(crate) fn staged_bundles(&self) -> Result<&BTreeMap<String, CanonicalStagedBundle>> {
        self.ensure_usable()?;
        Ok(&self.staged)
    }

    pub fn catalog_generation(&self) -> u64 {
        self.catalog_generation
    }
    fn ensure_usable(&self) -> Result<()> {
        if let Some(reason) = &self.poisoned {
            bail!("resident session requires recovery before use: {reason}");
        }
        Ok(())
    }

    /// Borrowed shared projections over the authoritative document, with proof
    /// bound to the owner's revision. No XLSX export or reader reconstruction.
    pub fn read_view(
        &self,
    ) -> Result<crate::workbook::WorkbookContext<&umya_spreadsheet::Spreadsheet>> {
        self.ensure_usable()?;
        let view = crate::workbook::WorkbookContext::borrowed(
            self.workbook.spreadsheet(),
            crate::model::WorkbookId(
                self.resource_id
                    .split_once(':')
                    .expect("validated resource identity")
                    .1
                    .to_string(),
            ),
            self.revision(),
        );
        if let crate::recalc::CalculationStamp::Current { coverage, .. } =
            self.workbook.calculation_stamp()
        {
            Ok(view.with_evaluation_coverage(coverage.clone()))
        } else {
            Ok(view)
        }
    }

    /// Explicit diagnostics-only access. This never grants mutation authority.
    pub fn diagnostic_workbook(&self) -> &crate::recalc::ResidentWorkbook {
        &self.workbook
    }

    pub fn export_bytes(&mut self) -> Result<Vec<u8>> {
        self.ensure_usable()?;
        self.workbook.export_bytes()
    }

    pub fn recalculate(
        &mut self,
        timeout_ms: Option<u64>,
    ) -> Result<crate::model::EvaluationCoverage> {
        self.ensure_usable()?;
        self.workbook.recalculate(timeout_ms)
    }

    pub fn range_values(
        &self,
        sheet_name: &str,
        ranges: impl Into<crate::core::session::SessionRangeSelection>,
    ) -> Result<Vec<crate::model::RangeValuesEntry>> {
        self.ensure_usable()?;
        self.workbook.range_values(sheet_name, ranges)
    }

    pub fn sheet_page(
        &self,
        params: crate::core::session::SessionSheetPageParams,
    ) -> Result<crate::model::SheetPageResponse> {
        self.ensure_usable()?;
        self.workbook.sheet_page(params)
    }

    pub fn apply_staged(
        &mut self,
        change_id: &str,
        expected_revision: &str,
    ) -> Result<WriteResponseData> {
        self.ensure_usable()?;
        let bundle = self
            .staged
            .get(change_id)
            .ok_or_else(|| invalid_request(format!("staged change not found: {change_id}")))?
            .clone();
        if bundle.base_revision != self.revision() || expected_revision != self.revision() {
            bail!("revision conflict: staged write approval is stale");
        }
        let resource_id: ResourceId = serde_json::from_value(json!(self.resource_id.clone()))?;
        let response = execute_write_on_resident(
            self,
            WriteRequest {
                resource_id,
                expected_revision: expected_revision.to_string(),
                mode: WriteMode::Apply,
                atomic: bundle.atomic,
                ops: bundle.ops,
                label: None,
                formula_parse_policy: bundle.formula_parse_policy,
            },
        )?;
        self.staged.remove(change_id);
        self.catalog_generation += 1;
        Ok(response)
    }

    pub fn discard_staged(&mut self, change_id: &str) -> Result<bool> {
        self.ensure_usable()?;
        let removed = self.staged.remove(change_id).is_some();
        if removed {
            self.catalog_generation += 1;
        }
        Ok(removed)
    }

    pub fn recover_from_records(
        resource_id: impl Into<String>,
        base_bytes: &[u8],
        records: &[crate::core::resident_storage::PreparedResidentCommit],
    ) -> Result<Self> {
        let resource_id = resource_id.into();
        let parsed: ResourceId = serde_json::from_value(json!(resource_id.clone()))?;
        let session_id = parsed.to_workbook_id().0;
        let base_sha256 = crate::utils::hash_bytes_sha256_hex(base_bytes);
        let history = portable_history_state(records, &session_id, &base_sha256)?;
        let mut session = Self::from_bytes(resource_id, base_bytes)?;
        let first_prepared = records
            .iter()
            .find_map(prepared_transaction_from_record)
            .transpose()?;
        if let Some(first_prepared) = &first_prepared {
            if first_prepared.base_sha256 != session.base_sha256 {
                bail!("resident recovery base workbook identity mismatch");
            }
            let parts = response_revision_before(&first_prepared.response)
                .split(':')
                .collect::<Vec<_>>();
            if parts.len() != 4 || parts[0] != "resident" {
                bail!("resident recovery has invalid initial revision identity");
            }
            session.workbook.restore_revision_identity(
                parts[1].to_string(),
                parts[2].parse()?,
                parts[3].parse()?,
            );
        }
        let active = history
            .active_ancestry()?
            .into_iter()
            .collect::<BTreeSet<_>>();
        for record in records {
            if active.contains(&record.commit_id) {
                let prepared = prepared_transaction_from_record(record)
                    .ok_or_else(|| anyhow!("active mutation lacks prepared transaction"))??;
                recover_prepared_document(&mut session, &prepared)?;
            }
        }
        // Recovery is deliberately cold. Validate inactive document ancestry too:
        // a later checkout must not expose a semantically unchecked mutation.
        for record in records {
            if !active.contains(&record.commit_id)
                && matches!(
                    record.transition,
                    crate::core::resident_storage::ResidentTransition::Mutation
                        | crate::core::resident_storage::ResidentTransition::StageApply
                )
            {
                let prepared = prepared_transaction_from_record(record)
                    .transpose()?
                    .ok_or_else(|| anyhow!("mutation lacks prepared transaction"))?;
                if !matches!(prepared.publication, PreparedResidentPublication::None) {
                    materialize_portable_head(
                        &session.resource_id,
                        base_bytes,
                        records,
                        Some(&record.commit_id),
                    )?;
                }
            }
        }
        // Catalog authority follows the linear stream, independently of the active
        // document ancestry.
        for record in records {
            match record.transition {
                crate::core::resident_storage::ResidentTransition::CatalogStage => {
                    let prepared = prepared_transaction_from_record(record)
                        .ok_or_else(|| anyhow!("catalog-stage lacks prepared transaction"))??;
                    let (change_id, bundle) = prepared
                        .staged
                        .ok_or_else(|| anyhow!("catalog-stage lacks staged bundle"))?;
                    if session.staged.insert(change_id, bundle).is_some() {
                        bail!("duplicate staged change identity during recovery");
                    }
                    session.catalog_generation += 1;
                }
                crate::core::resident_storage::ResidentTransition::CatalogDiscard => {
                    let change_id = record
                        .effects
                        .iter()
                        .find_map(|effect| effect.get("discard_staged_change_id"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow!("catalog-discard lacks staged identity"))?;
                    if session.staged.remove(change_id).is_none() {
                        bail!("catalog-discard references missing staged change");
                    }
                    session.catalog_generation += 1;
                }
                crate::core::resident_storage::ResidentTransition::StageApply => {
                    let prepared = prepared_transaction_from_record(record)
                        .ok_or_else(|| anyhow!("stage-apply lacks prepared transaction"))??;
                    let change_id = prepared
                        .consume_staged
                        .ok_or_else(|| anyhow!("stage-apply lacks consumed staged identity"))?;
                    if session.staged.remove(&change_id).is_none() {
                        bail!("stage-apply references missing staged change");
                    }
                    session.catalog_generation += 1;
                }
                crate::core::resident_storage::ResidentTransition::Checkpoint
                | crate::core::resident_storage::ResidentTransition::CheckpointDelete => {
                    session.catalog_generation += 1;
                }
                _ => {}
            }
        }
        if session.catalog_generation != history.catalog_generation {
            bail!("resident recovered catalog generation mismatch");
        }
        if let Some(revision) = history.state_revision.as_deref() {
            let parts = revision.split(':').collect::<Vec<_>>();
            if parts.len() != 4 || parts[0] != "resident" {
                bail!("resident history has invalid final revision identity");
            }
            session.workbook.restore_revision_identity(
                parts[1].to_string(),
                parts[2].parse()?,
                parts[3].parse()?,
            );
            session.workbook.mark_restart_boundary();
        }
        Ok(session)
    }
}

#[derive(Debug)]
pub(crate) struct WriteSnapshot {
    bytes: Vec<u8>,
    revision: String,
    content_revision: String,
}

pub(crate) trait WriteTransactionBackend {
    fn validate_resource(&self, resource_id: &ResourceId) -> Result<()>;
    fn snapshot(&mut self) -> Result<WriteSnapshot>;
    fn supports_stage(&self) -> bool {
        false
    }
    fn commit(
        &mut self,
        expected_revision: &str,
        bytes: Vec<u8>,
        op_kinds: &[String],
    ) -> Result<String>;
    fn stage(
        &mut self,
        _expected_revision: &str,
        _bundle: CanonicalStagedBundle,
        _label: Option<&str>,
        _impact: &WriteImpact,
        _diff: &WriteDiff,
    ) -> Result<(String, String)> {
        Err(invalid_request(
            "mode 'stage' is unavailable for this storage backend; use preview or apply",
        ))
    }
}

struct ByteSessionBackend<'a> {
    bytes: &'a [u8],
    revision: &'a str,
    committed: Option<Vec<u8>>,
}

impl WriteTransactionBackend for ByteSessionBackend<'_> {
    fn validate_resource(&self, resource_id: &ResourceId) -> Result<()> {
        if resource_id.as_str().starts_with("session:") {
            Ok(())
        } else {
            Err(invalid_request(
                "in-memory write requires a session: mutable resource_id",
            ))
        }
    }

    fn snapshot(&mut self) -> Result<WriteSnapshot> {
        Ok(WriteSnapshot {
            bytes: self.bytes.to_vec(),
            revision: self.revision.to_string(),
            content_revision: crate::utils::hash_bytes_sha256_hex(self.bytes),
        })
    }

    fn commit(
        &mut self,
        expected_revision: &str,
        bytes: Vec<u8>,
        _op_kinds: &[String],
    ) -> Result<String> {
        if expected_revision != self.revision {
            bail!(
                "revision conflict: expected {}, current {}",
                expected_revision,
                self.revision
            );
        }
        let revision = format!("state:{}", make_short_random_id("rev", 20));
        self.committed = Some(bytes);
        Ok(revision)
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct ForkFileBackend<'a> {
    fork: &'a mut crate::fork::ForkContext,
}

#[cfg(not(target_arch = "wasm32"))]
impl WriteTransactionBackend for ForkFileBackend<'_> {
    fn validate_resource(&self, resource_id: &ResourceId) -> Result<()> {
        if resource_id.as_str().starts_with("fork:")
            && resource_id.to_workbook_id().as_str() == self.fork.fork_id
        {
            Ok(())
        } else {
            Err(invalid_request(
                "native canonical write requires the bound fork: mutable resource_id",
            ))
        }
    }

    fn snapshot(&mut self) -> Result<WriteSnapshot> {
        let revision = self.fork.sync_revisions()?;
        Ok(WriteSnapshot {
            bytes: fs::read(&self.fork.work_path)?,
            revision,
            content_revision: self.fork.content_revision.clone(),
        })
    }

    fn supports_stage(&self) -> bool {
        true
    }

    fn commit(
        &mut self,
        expected_revision: &str,
        bytes: Vec<u8>,
        op_kinds: &[String],
    ) -> Result<String> {
        let current = self.fork.sync_revisions()?;
        if current != expected_revision {
            bail!(
                "revision conflict: expected {}, current {}",
                expected_revision,
                current
            );
        }
        let temp = temp_copy(&self.fork.work_path)?;
        fs::write(temp.path(), bytes)?;
        swap_temp(temp, &self.fork.work_path)?;
        self.fork.recalc_needed = true;
        self.fork.content_revision = hash_file_sha256_hex(&self.fork.work_path)?;
        let revision_after = self.fork.advance_state_revision();
        self.fork.push_canonical_operation(
            "write",
            op_kinds.to_vec(),
            expected_revision.to_string(),
            revision_after.clone(),
        );
        Ok(revision_after)
    }

    fn stage(
        &mut self,
        expected_revision: &str,
        bundle: CanonicalStagedBundle,
        label: Option<&str>,
        impact: &WriteImpact,
        diff: &WriteDiff,
    ) -> Result<(String, String)> {
        let current = self.fork.sync_revisions()?;
        if current != expected_revision {
            bail!(
                "revision conflict: expected {}, current {}",
                expected_revision,
                current
            );
        }
        let change_id = make_short_random_id("chg", 12);
        let summary = staged_summary(impact.op_kinds.clone(), bundle.ops.len(), diff.change_count);
        self.fork.push_staged_change(StagedChange {
            change_id: change_id.clone(),
            created_at: Utc::now(),
            label: label.map(str::to_string),
            ops: vec![StagedOp {
                kind: "canonical_write_bundle".to_string(),
                payload: serde_json::to_value(bundle)?,
            }],
            summary,
            fork_path_snapshot: None,
        });
        let revision_after = self.fork.advance_state_revision();
        Ok((change_id, revision_after))
    }
}

fn write_error(index: usize, op: &WriteOp, error: anyhow::Error) -> WriteOpResult {
    WriteOpResult {
        index,
        kind: op.kind().to_string(),
        status: WriteOpStatus::Failed,
        detail: None,
        error: Some(WriteOpError {
            code: "OPERATION_FAILED".to_string(),
            message: error.to_string(),
            path: format!("$.ops[{index}]"),
            retryable: false,
        }),
    }
}

fn skipped_result(index: usize, op: &WriteOp) -> WriteOpResult {
    WriteOpResult {
        index,
        kind: op.kind().to_string(),
        status: WriteOpStatus::Skipped,
        detail: None,
        error: None,
    }
}

fn applied_result(index: usize, op: &WriteOp, detail: Value) -> WriteOpResult {
    WriteOpResult {
        index,
        kind: op.kind().to_string(),
        status: WriteOpStatus::Applied,
        detail: Some(detail),
        error: None,
    }
}

fn apply_atomic_candidate(
    original: &umya_spreadsheet::Spreadsheet,
    ops: &[WriteOp],
    policy: FormulaParsePolicy,
) -> (
    umya_spreadsheet::Spreadsheet,
    Vec<WriteOpResult>,
    Option<usize>,
) {
    let mut candidate = original.clone();
    let mut results = Vec::with_capacity(ops.len());
    let mut failure = None;
    for (index, op) in ops.iter().enumerate() {
        if failure.is_some() {
            results.push(skipped_result(index, op));
            continue;
        }
        match apply_write_op_to_workbook(&mut candidate, op, policy) {
            Ok(detail) => results.push(applied_result(index, op, detail)),
            Err(error) => {
                failure = Some(index);
                results.push(write_error(index, op, error));
            }
        }
    }
    (candidate, results, failure)
}

fn canonical_response_from_prepared(
    request: &WriteRequest,
    revision_before: String,
    revision_after: String,
    mut results: Vec<WriteOpResult>,
    mut diff: WriteDiff,
    impact: WriteImpact,
    failure: Option<usize>,
    applied: usize,
    stage_change_id: Option<String>,
) -> WriteResponseData {
    if let Some(index) = failure {
        if request.mode == WriteMode::Apply && request.atomic {
            for result in &mut results[..index] {
                result.status = WriteOpStatus::RolledBack;
            }
            return WriteResponseData::RolledBack {
                mode: WriteMode::Apply,
                atomic: true,
                revision_before: revision_before.clone(),
                revision_after: revision_before,
                ops_applied: 0,
                rolled_back: true,
                diff: WriteDiff {
                    change_count: 0,
                    exact: true,
                    precision: "exact_baseline_to_result".into(),
                    changes: Vec::new(),
                    effects: Vec::new(),
                },
                impact,
                results,
            };
        }
        if request.mode != WriteMode::Apply {
            for result in &mut results[..index] {
                result.status = WriteOpStatus::RolledBack;
            }
            add_effect_manifest(&mut diff, &results);
            return WriteResponseData::Failed {
                mode: request.mode,
                atomic: request.atomic,
                revision_before: revision_before.clone(),
                revision_after: revision_before,
                ops_applied: 0,
                diff,
                impact,
                results,
            };
        }
        add_effect_manifest(&mut diff, &results);
        return WriteResponseData::Partial {
            mode: WriteMode::Apply,
            atomic: false,
            revision_before,
            revision_after,
            ops_applied: applied,
            diff,
            impact,
            results,
        };
    }
    add_effect_manifest(&mut diff, &results);
    match request.mode {
        WriteMode::Preview => {
            for result in &mut results {
                result.status = WriteOpStatus::Previewed;
            }
            WriteResponseData::Previewed {
                mode: WriteMode::Preview,
                atomic: request.atomic,
                revision_before: revision_before.clone(),
                revision_after: revision_before,
                ops_previewed: request.ops.len(),
                diff,
                impact,
                results,
            }
        }
        WriteMode::Stage => {
            for result in &mut results {
                result.status = WriteOpStatus::Staged;
            }
            WriteResponseData::Staged {
                mode: WriteMode::Stage,
                atomic: true,
                revision_before,
                revision_after,
                ops_staged: request.ops.len(),
                change_id: stage_change_id.expect("successful stage has prepared identity"),
                diff,
                impact,
                results,
            }
        }
        WriteMode::Apply => WriteResponseData::Applied {
            mode: WriteMode::Apply,
            atomic: request.atomic,
            revision_before,
            revision_after,
            ops_applied: applied,
            diff,
            impact,
            results,
        },
    }
}

fn logical_workbook_sha256(bytes: &[u8]) -> Result<String> {
    let mut book = umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(bytes), true)?;
    for sheet in book.get_sheet_collection_mut() {
        for cell in sheet.get_cell_collection_mut() {
            if cell.is_formula() {
                cell.set_formula_result_blank();
            }
        }
    }
    Ok(crate::utils::hash_bytes_sha256_hex(&workbook_bytes(&book)?))
}

fn workbook_bytes(book: &umya_spreadsheet::Spreadsheet) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(book, &mut bytes)?;
    Ok(bytes)
}

fn execute_write_transaction<B: WriteTransactionBackend>(
    backend: &mut B,
    request: WriteRequest,
) -> Result<WriteResponseData> {
    validate_request(&request)?;
    backend.validate_resource(&request.resource_id)?;
    if request.mode == WriteMode::Stage && !backend.supports_stage() {
        return Err(invalid_request(
            "mode 'stage' is unavailable for in-memory sessions; use preview or apply",
        ));
    }

    let snapshot = backend.snapshot()?;
    if request.expected_revision != snapshot.revision {
        bail!(
            "revision conflict: expected {}, current {}",
            request.expected_revision,
            snapshot.revision
        );
    }
    let policy = request
        .formula_parse_policy
        .unwrap_or(FormulaParsePolicy::Warn);
    let impact = WriteImpact {
        op_kinds: request.ops.iter().map(|op| op.kind().to_string()).collect(),
        risk: worst_risk(&request.ops),
    };
    let original =
        umya_spreadsheet::reader::xlsx::read_reader(std::io::Cursor::new(&snapshot.bytes), true)?;

    if matches!(request.mode, WriteMode::Preview | WriteMode::Stage) || request.atomic {
        let (candidate, results, failure) = apply_atomic_candidate(&original, &request.ops, policy);
        if failure.is_some() {
            let diff = if request.mode == WriteMode::Apply {
                WriteDiff {
                    change_count: 0,
                    exact: true,
                    precision: "exact_baseline_to_result".to_string(),
                    changes: Vec::new(),
                    effects: Vec::new(),
                }
            } else {
                let candidate_bytes = workbook_bytes(&candidate)?;
                diff_bytes(&snapshot.bytes, &candidate_bytes)?
            };
            return Ok(canonical_response_from_prepared(
                &request,
                snapshot.revision.clone(),
                snapshot.revision,
                results,
                diff,
                impact,
                failure,
                0,
                None,
            ));
        }

        let candidate_bytes = workbook_bytes(&candidate)?;
        let diff = diff_bytes(&snapshot.bytes, &candidate_bytes)?;
        let (revision_after, change_id) = match request.mode {
            WriteMode::Preview => (snapshot.revision.clone(), None),
            WriteMode::Stage => {
                let bundle = CanonicalStagedBundle {
                    base_revision: snapshot.content_revision,
                    max_risk: impact.risk,
                    atomic: true,
                    ops: request.ops.clone(),
                    label: request.label.clone(),
                    formula_parse_policy: request.formula_parse_policy,
                };
                let (change_id, revision_after) = backend.stage(
                    &snapshot.revision,
                    bundle,
                    request.label.as_deref(),
                    &impact,
                    &diff,
                )?;
                (revision_after, Some(change_id))
            }
            WriteMode::Apply => (
                backend.commit(&snapshot.revision, candidate_bytes, &impact.op_kinds)?,
                None,
            ),
        };
        return Ok(canonical_response_from_prepared(
            &request,
            snapshot.revision,
            revision_after,
            results,
            diff,
            impact,
            None,
            request.ops.len(),
            change_id,
        ));
    }

    let mut current = original;
    let mut results = Vec::with_capacity(request.ops.len());
    let mut applied = 0usize;
    let mut failed = false;
    for (index, op) in request.ops.iter().enumerate() {
        if failed {
            results.push(skipped_result(index, op));
            continue;
        }
        let mut candidate = current.clone();
        match apply_write_op_to_workbook(&mut candidate, op, policy) {
            Ok(detail) => {
                current = candidate;
                applied += 1;
                results.push(applied_result(index, op, detail));
            }
            Err(error) => {
                failed = true;
                results.push(write_error(index, op, error));
            }
        }
    }
    let current_bytes = workbook_bytes(&current)?;
    let diff = diff_bytes(&snapshot.bytes, &current_bytes)?;
    let revision_after = if applied > 0 {
        backend.commit(
            &snapshot.revision,
            current_bytes,
            &impact.op_kinds[..applied],
        )?
    } else {
        snapshot.revision.clone()
    };
    Ok(canonical_response_from_prepared(
        &request,
        snapshot.revision,
        revision_after,
        results,
        diff,
        impact,
        failed.then_some(applied),
        applied,
        None,
    ))
}

const MAX_PREPARED_COMMON_CELLS: usize = 100_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PreparedResidentPublication {
    None,
    Cells {
        effects: Vec<crate::recalc::ResidentPreparedCellEffect>,
    },
    Snapshot {
        expected_before_sha256: String,
        bytes: Vec<u8>,
        sha256: String,
        calculation_effect: crate::recalc::ResidentCalculationEffect,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PreparedResidentTransaction {
    base_sha256: String,
    response: WriteResponseData,
    publication: PreparedResidentPublication,
    staged: Option<(String, CanonicalStagedBundle)>,
    consume_staged: Option<String>,
}

pub(crate) fn validate_portable_prepared_record(
    record: &crate::core::resident_storage::PreparedResidentCommit,
    previous_revision: Option<&str>,
) -> Result<()> {
    use crate::core::resident_storage::ResidentTransition as T;
    let Some(prepared) = prepared_transaction_from_record(record).transpose()? else {
        if matches!(
            record.transition,
            T::Mutation | T::StageApply | T::CatalogStage
        ) {
            bail!("transition requires prepared transaction");
        }
        return Ok(());
    };
    if prepared.base_sha256 != record.base_sha256
        || prepared.response.revision_after() != record.state_revision
    {
        bail!("prepared transaction identity/result mismatch");
    }
    if previous_revision
        .is_some_and(|revision| revision != response_revision_before(&prepared.response))
    {
        bail!("prepared transaction before revision mismatch");
    }
    match &prepared.publication {
        PreparedResidentPublication::Snapshot { bytes, sha256, .. } => {
            if crate::utils::hash_bytes_sha256_hex(bytes) != *sha256 {
                bail!("prepared snapshot content identity mismatch");
            }
        }
        PreparedResidentPublication::Cells { effects } => {
            let mut seen = BTreeSet::new();
            if effects.is_empty() {
                bail!("document publication has no cell effects");
            }
            for effect in effects {
                if effect.sheet_name.is_empty()
                    || effect.row == 0
                    || effect.column == 0
                    || effect.row > 1_048_576
                    || effect.column > 16_384
                    || !seen.insert((&effect.sheet_name, effect.row, effect.column))
                {
                    bail!("prepared cell target is invalid or duplicated");
                }
            }
        }
        PreparedResidentPublication::None => {}
    }
    let publishes_document = !matches!(prepared.publication, PreparedResidentPublication::None);
    if (publishes_document && !matches!(record.transition, T::Mutation | T::StageApply))
        || (!publishes_document && record.transition == T::Mutation)
        || prepared.staged.is_some() != (record.transition == T::CatalogStage)
        || prepared.consume_staged.is_some() != (record.transition == T::StageApply)
    {
        bail!("prepared publication/control relationship mismatch");
    }
    if !matches!(
        record.transition,
        T::Mutation | T::StageApply | T::CatalogStage | T::Receipt
    ) {
        bail!("control cannot carry prepared document transaction");
    }
    Ok(())
}

fn response_revision_before(response: &WriteResponseData) -> &str {
    match response {
        WriteResponseData::Previewed {
            revision_before, ..
        }
        | WriteResponseData::Staged {
            revision_before, ..
        }
        | WriteResponseData::Applied {
            revision_before, ..
        }
        | WriteResponseData::Partial {
            revision_before, ..
        }
        | WriteResponseData::Failed {
            revision_before, ..
        }
        | WriteResponseData::RolledBack {
            revision_before, ..
        } => revision_before,
    }
}

fn portable_history_state(
    records: &[crate::core::resident_storage::PreparedResidentCommit],
    session_id: &str,
    base_sha256: &str,
) -> Result<crate::core::resident_storage::PortableHistoryState> {
    crate::core::resident_storage::PortableHistoryState::replay_bound(
        records,
        session_id,
        base_sha256,
    )
}

fn materialize_portable_head(
    resource_id: &str,
    base_bytes: &[u8],
    records: &[crate::core::resident_storage::PreparedResidentCommit],
    target: Option<&str>,
) -> Result<Vec<u8>> {
    let mut session = ResidentWriteSession::from_bytes(resource_id.to_string(), base_bytes)?;
    if let Some(first) = records
        .iter()
        .find_map(prepared_transaction_from_record)
        .transpose()?
    {
        let parts = response_revision_before(&first.response)
            .split(':')
            .collect::<Vec<_>>();
        if parts.len() != 4 || parts[0] != "resident" {
            bail!("resident history has invalid epoch identity");
        }
        session.workbook.restore_revision_identity(
            parts[1].to_string(),
            parts[2].parse()?,
            parts[3].parse()?,
        );
    }
    let by_id = records
        .iter()
        .map(|record| (record.commit_id.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let mut ancestry = Vec::new();
    let mut cursor = target.map(str::to_string);
    let mut seen = BTreeSet::new();
    while let Some(id) = cursor {
        if !seen.insert(id.clone()) {
            bail!("resident history ancestry cycle");
        }
        let record = by_id
            .get(id.as_str())
            .ok_or_else(|| anyhow!("resident history target references unknown commit"))?;
        if !matches!(
            record.transition,
            crate::core::resident_storage::ResidentTransition::Mutation
                | crate::core::resident_storage::ResidentTransition::StageApply
        ) {
            bail!("resident history ancestry references non-mutation commit");
        }
        ancestry.push(*record);
        cursor = record.history_parent_commit_id.clone();
    }
    ancestry.reverse();
    for record in ancestry {
        let prepared = prepared_transaction_from_record(record)
            .ok_or_else(|| anyhow!("resident mutation lacks prepared transaction"))??;
        recover_prepared_document(&mut session, &prepared)?;
    }
    session.export_bytes()
}

fn prepared_transaction_from_record(
    record: &crate::core::resident_storage::PreparedResidentCommit,
) -> Option<Result<PreparedResidentTransaction>> {
    record
        .effects
        .iter()
        .find_map(|effect| effect.get("prepared_transaction"))
        .map(|value| serde_json::from_value(value.clone()).map_err(Into::into))
}

fn common_touched_cells(op: &WriteOp) -> Option<Result<Vec<(String, u32, u32)>>> {
    let mut touched = Vec::new();
    match op {
        WriteOp::SetCells(op) => {
            for address in op.cells.keys() {
                let (column, row) = match parse_cell_ref(address) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                touched.push((op.sheet_name.clone(), column, row));
            }
        }
        WriteOp::Transform(TransformOp::WriteMatrix {
            sheet_name,
            anchor,
            rows,
            ..
        }) => {
            let (anchor_column, anchor_row) = match parse_cell_ref(anchor) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            for (row_offset, values) in rows.iter().enumerate() {
                for (column_offset, value) in values.iter().enumerate() {
                    if value.is_none() {
                        continue;
                    }
                    let Some(row) = anchor_row.checked_add(row_offset as u32) else {
                        return Some(Err(anyhow!("matrix row range overflows")));
                    };
                    let Some(column) = anchor_column.checked_add(column_offset as u32) else {
                        return Some(Err(anyhow!("matrix column range overflows")));
                    };
                    if row > 1_048_576 || column > 16_384 {
                        return Some(Err(anyhow!("matrix exceeds XLSX worksheet bounds")));
                    }
                    touched.push((sheet_name.clone(), column, row));
                }
            }
        }
        _ => return None,
    }
    Some(Ok(touched))
}

fn materialized_value_string(value: &crate::recalc::ResidentMaterializedValue) -> Option<String> {
    use crate::recalc::ResidentMaterializedValue as Value;
    match value {
        Value::Empty => None,
        Value::String(value)
        | Value::RichText(value)
        | Value::Lazy(value)
        | Value::Error(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(if *value { "1" } else { "0" }.to_string()),
    }
}

fn prepared_cell_change(
    sheet: &str,
    address: String,
    before: Option<&crate::recalc::ResidentMaterializedCell>,
    after: Option<&crate::recalc::ResidentMaterializedCell>,
) -> Result<Option<Value>> {
    use crate::diff::merge::{CellDiff, ModificationType};
    let before_value = before.and_then(|cell| materialized_value_string(&cell.value));
    let after_value = after.and_then(|cell| materialized_value_string(&cell.value));
    let before_formula = before.and_then(|cell| cell.formula.clone());
    let after_formula = after.and_then(|cell| cell.formula.clone());
    let diff = match (before, after) {
        (None, None) => return Ok(None),
        (None, Some(_)) if after_value.is_none() && after_formula.is_none() => return Ok(None),
        (None, Some(_)) => CellDiff::Added {
            address,
            value: after_value,
            formula: after_formula,
        },
        (Some(_), None) => CellDiff::Deleted {
            address,
            old_value: before_value,
        },
        (Some(_), Some(_)) => {
            let formula_changed = before_formula != after_formula;
            let value_changed = match (&before_value, &after_value) {
                (Some(before), Some(after)) => {
                    match (before.parse::<f64>(), after.parse::<f64>()) {
                        (Ok(before), Ok(after)) => (before - after).abs() >= 1e-9,
                        _ => before != after,
                    }
                }
                _ => before_value != after_value,
            };
            if !formula_changed && !value_changed {
                return Ok(None);
            }
            let subtype = if formula_changed {
                ModificationType::FormulaEdit
            } else if after_formula.is_some() {
                ModificationType::RecalcResult
            } else {
                ModificationType::ValueEdit
            };
            CellDiff::Modified {
                address,
                subtype,
                old_value: before_value,
                new_value: after_value,
                old_formula: before_formula,
                new_formula: after_formula,
                old_style_id: None,
                new_style_id: None,
            }
        }
    };
    Ok(Some(serde_json::to_value(crate::diff::Change::Cell(
        crate::diff::CellChange {
            sheet: sheet.to_string(),
            diff,
        },
    ))?))
}

fn predicted_resident_revision(session: &ResidentWriteSession, mutated: bool) -> String {
    if !mutated {
        session.revision()
    } else {
        format!(
            "resident:{}:{}:{}",
            session.workbook.revisions().epoch,
            session.workbook.revisions().document + 1,
            session.workbook.revisions().state + 1
        )
    }
}

fn prepare_common_transaction(
    session: &ResidentWriteSession,
    request: &WriteRequest,
    stage_change_id: Option<String>,
) -> Result<PreparedResidentTransaction> {
    use crate::recalc::{
        ResidentMaterializedCell, ResidentPreparedCellEffect, materialize_umya_cell,
    };
    let source = session.workbook.spreadsheet();
    let mut shadow = umya_spreadsheet::new_file_empty_worksheet();
    for sheet in source.get_sheet_collection() {
        shadow
            .new_sheet(sheet.get_name())
            .map_err(|error| anyhow!(error))?;
    }
    let policy = request
        .formula_parse_policy
        .unwrap_or(FormulaParsePolicy::Warn);
    let impact = WriteImpact {
        op_kinds: request.ops.iter().map(|op| op.kind().to_string()).collect(),
        risk: worst_risk(&request.ops),
    };
    let mut before = BTreeMap::<(String, u32, u32), Option<ResidentMaterializedCell>>::new();
    let mut source_op_indices = BTreeMap::<(String, u32, u32), Vec<usize>>::new();
    let mut results = Vec::with_capacity(request.ops.len());
    let mut failure = None;
    let atomic_candidate = request.atomic || request.mode != WriteMode::Apply;
    for (index, op) in request.ops.iter().enumerate() {
        if failure.is_some() {
            results.push(skipped_result(index, op));
            continue;
        }
        let touched = match common_touched_cells(op).expect("common operation") {
            Ok(touched) => touched,
            Err(error) => {
                failure = Some(index);
                results.push(write_error(index, op, error));
                continue;
            }
        };
        let unique_after = before.len()
            + touched
                .iter()
                .filter(|cell| !before.contains_key(*cell))
                .collect::<BTreeSet<_>>()
                .len();
        if unique_after > MAX_PREPARED_COMMON_CELLS {
            failure = Some(index);
            results.push(write_error(
                index,
                op,
                anyhow!("prepared common-cell limit exceeds {MAX_PREPARED_COMMON_CELLS}"),
            ));
            continue;
        }
        for (sheet_name, column, row) in &touched {
            let key = (sheet_name.clone(), *column, *row);
            if before.contains_key(&key) {
                continue;
            }
            let source_sheet = match source.get_sheet_by_name(&sheet_name) {
                Some(sheet) => sheet,
                None => continue,
            };
            let original = source_sheet.get_cell((column, row)).cloned();
            before.insert(key, original.as_ref().map(materialize_umya_cell));
            if let Some(cell) = original {
                shadow
                    .get_sheet_by_name_mut(&sheet_name)
                    .expect("source sheet copied to shadow")
                    .set_cell(cell);
            }
        }
        match apply_write_op_to_workbook(&mut shadow, op, policy) {
            Ok(detail) => {
                for (sheet_name, column, row) in &touched {
                    source_op_indices
                        .entry((sheet_name.clone(), *column, *row))
                        .or_default()
                        .push(index);
                }
                results.push(applied_result(index, op, detail));
            }
            Err(error) => {
                failure = Some(index);
                results.push(write_error(index, op, error));
            }
        }
        if failure.is_some() && !atomic_candidate {
            continue;
        }
    }

    let mut effects = Vec::new();
    let mut changes = Vec::new();
    // Match the canonical worksheet diff's row-major order without scanning
    // or cloning any cells outside the bounded touched set.
    let mut ordered_before: Vec<_> = before.iter().collect();
    ordered_before.sort_by(|((ls, lc, lr), _), ((rs, rc, rr), _)| (ls, lr, lc).cmp(&(rs, rr, rc)));
    for ((sheet_name, column, row), expected_before) in ordered_before {
        let after = shadow
            .get_sheet_by_name(sheet_name)
            .and_then(|sheet| sheet.get_cell((*column, *row)))
            .map(materialize_umya_cell);
        if let Some(change) = prepared_cell_change(
            sheet_name,
            crate::utils::cell_address(*column, *row),
            expected_before.as_ref(),
            after.as_ref(),
        )? {
            changes.push(change);
        }
        if expected_before != &after {
            if let Some(after) = after {
                effects.push(ResidentPreparedCellEffect {
                    sheet_name: sheet_name.clone(),
                    column: *column,
                    row: *row,
                    source_op_indices: source_op_indices
                        .get(&(sheet_name.clone(), *column, *row))
                        .cloned()
                        .unwrap_or_default(),
                    expected_before: expected_before.clone(),
                    after,
                });
            }
        }
    }
    let candidate_diff = WriteDiff {
        change_count: changes.len(),
        exact: true,
        precision: "exact_baseline_to_result".into(),
        changes,
        effects: Vec::new(),
    };
    let revision_before = session.revision();
    let applied = results
        .iter()
        .filter(|result| matches!(result.status, WriteOpStatus::Applied))
        .count();
    let publish_cells = request.mode == WriteMode::Apply
        && !(request.atomic && failure.is_some())
        && !effects.is_empty();
    let revision_after = predicted_resident_revision(session, publish_cells);
    let staged = if request.mode == WriteMode::Stage && failure.is_none() {
        let change_id = stage_change_id.clone().expect("stage identity prepared");
        Some((
            change_id,
            CanonicalStagedBundle {
                base_revision: revision_before.clone(),
                max_risk: impact.risk,
                atomic: true,
                ops: request.ops.clone(),
                    label: request.label.clone(),
                formula_parse_policy: request.formula_parse_policy,
            },
        ))
    } else {
        None
    };
    let response = canonical_response_from_prepared(
        request,
        revision_before,
        revision_after,
        results,
        candidate_diff,
        impact,
        failure,
        applied,
        stage_change_id,
    );
    let publication = if publish_cells {
        PreparedResidentPublication::Cells { effects }
    } else {
        PreparedResidentPublication::None
    };
    Ok(PreparedResidentTransaction {
        base_sha256: session.base_sha256.clone(),
        response,
        publication,
        staged,
        consume_staged: None,
    })
}

fn calculation_effect_for_ops<'a>(
    mut ops: impl Iterator<Item = &'a WriteOp>,
) -> crate::recalc::ResidentCalculationEffect {
    if ops.all(|op| {
        matches!(
            op.kind(),
            "style"
                | "column_size"
                | "freeze_panes"
                | "set_zoom"
                | "set_gridlines"
                | "set_page_margins"
                | "set_page_setup"
                | "set_print_area"
                | "set_page_breaks"
                | "set_data_validation"
                | "add_conditional_format"
                | "set_conditional_format"
                | "clear_conditional_formats"
        )
    }) {
        crate::recalc::ResidentCalculationEffect::Preserve
    } else {
        crate::recalc::ResidentCalculationEffect::Invalidate
    }
}

fn rewrite_response_revisions(response: &mut WriteResponseData, before: &str, after: &str) {
    match response {
        WriteResponseData::Previewed {
            revision_before,
            revision_after,
            ..
        }
        | WriteResponseData::Staged {
            revision_before,
            revision_after,
            ..
        }
        | WriteResponseData::Applied {
            revision_before,
            revision_after,
            ..
        }
        | WriteResponseData::Partial {
            revision_before,
            revision_after,
            ..
        }
        | WriteResponseData::Failed {
            revision_before,
            revision_after,
            ..
        }
        | WriteResponseData::RolledBack {
            revision_before,
            revision_after,
            ..
        } => {
            *revision_before = before.to_string();
            *revision_after = after.to_string();
        }
    }
}

fn prepare_snapshot_transaction(
    session: &ResidentWriteSession,
    request: &WriteRequest,
    stage_change_id: Option<String>,
) -> Result<PreparedResidentTransaction> {
    let bytes = session.workbook.snapshot_bytes()?;
    let current_sha256 = logical_workbook_sha256(&bytes)?;
    let revision_before = session.revision();
    let base_sha256 = session.base_sha256.clone();
    if request.mode == WriteMode::Stage {
        let mut preview_request = request.clone();
        preview_request.mode = WriteMode::Preview;
        let (preview, _) = execute_write_on_bytes(&bytes, &revision_before, preview_request)?;
        let response = match preview {
            WriteResponseData::Previewed {
                ops_previewed,
                diff,
                impact,
                mut results,
                ..
            } => {
                for result in &mut results {
                    result.status = WriteOpStatus::Staged;
                }
                let change_id = stage_change_id.expect("stage identity prepared");
                WriteResponseData::Staged {
                    mode: WriteMode::Stage,
                    atomic: true,
                    revision_before: revision_before.clone(),
                    revision_after: revision_before.clone(),
                    ops_staged: ops_previewed,
                    change_id: change_id.clone(),
                    diff,
                    impact,
                    results,
                }
            }
            WriteResponseData::Failed {
                atomic,
                diff,
                impact,
                results,
                ..
            } => WriteResponseData::Failed {
                mode: WriteMode::Stage,
                atomic,
                revision_before: revision_before.clone(),
                revision_after: revision_before.clone(),
                ops_applied: 0,
                diff,
                impact,
                results,
            },
            _ => unreachable!("preview preparation returns previewed or failed"),
        };
        let staged = if let WriteResponseData::Staged {
            change_id, impact, ..
        } = &response
        {
            Some((
                change_id.clone(),
                CanonicalStagedBundle {
                    base_revision: revision_before,
                    max_risk: impact.risk,
                    atomic: true,
                    ops: request.ops.clone(),
                    label: request.label.clone(),
                    formula_parse_policy: request.formula_parse_policy,
                },
            ))
        } else {
            None
        };
        return Ok(PreparedResidentTransaction {
            base_sha256,
            response,
            publication: PreparedResidentPublication::None,
            staged,
            consume_staged: None,
        });
    }
    let (mut response, candidate) =
        execute_write_on_bytes(&bytes, &revision_before, request.clone())?;
    let mutated = candidate.is_some();
    let revision_after = predicted_resident_revision(session, mutated);
    rewrite_response_revisions(&mut response, &revision_before, &revision_after);
    let applied_results = match &response {
        WriteResponseData::Applied { results, .. } | WriteResponseData::Partial { results, .. } => {
            results.as_slice()
        }
        _ => &[],
    };
    let publication = match candidate {
        Some(bytes) => PreparedResidentPublication::Snapshot {
            expected_before_sha256: current_sha256,
            sha256: crate::utils::hash_bytes_sha256_hex(&bytes),
            bytes,
            calculation_effect: calculation_effect_for_ops(
                applied_results
                    .iter()
                    .filter(|result| matches!(result.status, WriteOpStatus::Applied))
                    .map(|result| &request.ops[result.index]),
            ),
        },
        None => PreparedResidentPublication::None,
    };
    Ok(PreparedResidentTransaction {
        base_sha256,
        response,
        publication,
        staged: None,
        consume_staged: None,
    })
}

fn prepare_resident_transaction(
    session: &ResidentWriteSession,
    request: &WriteRequest,
    stage_change_id: Option<String>,
) -> Result<PreparedResidentTransaction> {
    validate_request(request)?;
    if request.resource_id.as_str() != session.resource_id {
        return Err(invalid_request(
            "resident write resource does not match its owning session",
        ));
    }
    if request.expected_revision != session.revision() {
        bail!(
            "revision conflict: expected {}, current {}",
            request.expected_revision,
            session.revision()
        );
    }
    if request
        .ops
        .iter()
        .all(|op| common_touched_cells(op).is_some())
    {
        prepare_common_transaction(session, request, stage_change_id)
    } else {
        prepare_snapshot_transaction(session, request, stage_change_id)
    }
}

fn publish_resident_transaction(
    session: &mut ResidentWriteSession,
    prepared: &PreparedResidentTransaction,
) -> Result<()> {
    publish_resident_transaction_inner(session, prepared, false)
}

fn recover_prepared_document(
    session: &mut ResidentWriteSession,
    prepared: &PreparedResidentTransaction,
) -> Result<()> {
    if prepared.base_sha256 != session.base_sha256 {
        bail!("prepared transaction base identity does not match resident owner");
    }
    match &prepared.publication {
        PreparedResidentPublication::None => Ok(()),
        PreparedResidentPublication::Cells { effects } => {
            session.workbook.recover_prepared_cells(effects)?;
            Ok(())
        }
        PreparedResidentPublication::Snapshot {
            expected_before_sha256,
            bytes,
            sha256,
            calculation_effect,
        } => {
            if logical_workbook_sha256(&session.workbook.snapshot_bytes()?)?
                != *expected_before_sha256
            {
                bail!("prepared snapshot logical predecessor mismatch during recovery");
            }
            if crate::utils::hash_bytes_sha256_hex(bytes) != *sha256 {
                bail!("prepared snapshot content hash mismatch");
            }
            session
                .workbook
                .replace_from_bytes(bytes, *calculation_effect)?;
            Ok(())
        }
    }
}

fn publish_resident_transaction_inner(
    session: &mut ResidentWriteSession,
    prepared: &PreparedResidentTransaction,
    recovery: bool,
) -> Result<()> {
    if prepared.base_sha256 != session.base_sha256 {
        bail!("prepared transaction base identity does not match resident owner");
    }
    match &prepared.publication {
        PreparedResidentPublication::None => {}
        PreparedResidentPublication::Cells { effects } => {
            if recovery {
                session.workbook.recover_prepared_cells(effects)?;
            } else {
                session.workbook.publish_prepared_cells(effects)?;
            }
        }
        PreparedResidentPublication::Snapshot {
            expected_before_sha256,
            bytes,
            sha256,
            calculation_effect,
        } => {
            if logical_workbook_sha256(&session.workbook.snapshot_bytes()?)?
                != *expected_before_sha256
            {
                bail!("prepared snapshot logical before-state hash mismatch");
            }
            if crate::utils::hash_bytes_sha256_hex(bytes) != *sha256 {
                bail!("prepared snapshot content hash mismatch");
            }
            session
                .workbook
                .replace_from_bytes(bytes, *calculation_effect)?;
        }
    }
    if let Some((change_id, bundle)) = &prepared.staged {
        if session.staged.contains_key(change_id) {
            bail!("prepared stage identity already exists in catalog");
        }
        session.staged.insert(change_id.clone(), bundle.clone());
        session.catalog_generation += 1;
    }
    if let Some(change_id) = &prepared.consume_staged {
        if session.staged.remove(change_id).is_none() {
            bail!("prepared staged apply references missing catalog entry");
        }
        session.catalog_generation += 1;
    }
    if session.revision() != prepared.response.revision_after() {
        bail!("prepared publication revision mismatch");
    }
    Ok(())
}

/// Prepare once and publish exactly the canonical prepared effects.
pub fn execute_write_on_resident(
    session: &mut ResidentWriteSession,
    request: WriteRequest,
) -> Result<WriteResponseData> {
    session.ensure_usable()?;
    let stage_change_id =
        (request.mode == WriteMode::Stage).then(|| make_short_random_id("chg", 12));
    let prepared = prepare_resident_transaction(session, &request, stage_change_id)?;
    if request.mode != WriteMode::Preview {
        if let Err(error) = publish_resident_transaction(session, &prepared) {
            session.poisoned = Some(format!("prepared publication failed: {error}"));
            return Err(error);
        }
    }
    Ok(prepared.response)
}

async fn reconciled_record_by_request_id<
    S: crate::core::resident_storage::ResidentCommitStorage,
>(
    storage: &S,
    session: &ResidentWriteSession,
    request_id: &str,
) -> Result<Option<crate::core::resident_storage::PreparedResidentCommit>> {
    session.ensure_usable()?;
    let session_id = session.resource_id.split_once(':').unwrap().1;
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    if history
        .state_revision
        .as_deref()
        .is_some_and(|revision| revision != session.revision())
        || history.catalog_generation != session.catalog_generation
    {
        bail!(
            "resident session requires recovery before outcome reconciliation: state/catalog mismatch"
        );
    }
    let Some(record) = records
        .iter()
        .find(|record| record.request_id == request_id)
    else {
        return Ok(None);
    };
    match storage
        .reconcile(session_id, request_id, &record.request_fingerprint)
        .await?
    {
        crate::core::resident_storage::ReconcileOutcome::Committed(_) => {
            let mut outcome = record.clone();
            if outcome.transition == crate::core::resident_storage::ResidentTransition::Receipt {
                if let Some(operation) = outcome
                    .effects
                    .iter()
                    .find_map(|e| e.get("receipt_operation"))
                {
                    // Reconciliation returns the original operation outcome;
                    // the stored record remains an effect-free Receipt.
                    outcome.transition = serde_json::from_value(operation.clone())?;
                }
            }
            Ok(Some(outcome))
        }
        crate::core::resident_storage::ReconcileOutcome::NotFound => {
            bail!("request disappeared during durable reconciliation")
        }
    }
}

pub async fn discard_staged_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    change_id: &str,
) -> Result<bool> {
    session.ensure_usable()?;
    if request_id.is_empty() {
        return Err(invalid_request(
            "request_id is required for durable catalog mutations",
        ));
    }
    let session_id = session.resource_id.split_once(':').unwrap().1.to_string();
    if let Some(record) = reconciled_record_by_request_id(storage, session, request_id).await? {
        let matches_input = record.transition
            == crate::core::resident_storage::ResidentTransition::CatalogDiscard
            && record.effects.iter().any(|effect| {
                effect
                    .get("discard_staged_change_id")
                    .and_then(Value::as_str)
                    == Some(change_id)
            });
        if !matches_input {
            bail!("request identity reuse with different catalog input");
        }
        return Ok(!record
            .effects
            .iter()
            .any(|e| e.get("receipt_operation").is_some()));
    }
    if !session.staged.contains_key(change_id) {
        commit_control_receipt(
            session,
            storage,
            request_id,
            crate::core::resident_storage::ResidentTransition::CatalogDiscard,
            vec![json!({"discard_staged_change_id":change_id})],
        )
        .await?;
        return Ok(false);
    }
    let records = storage.load(&session_id).await?;
    let history = portable_history_state(&records, &session_id, &session.base_sha256)?;
    if records.last().is_some_and(|record| {
        record.state_revision != session.revision()
            || record.catalog_generation != session.catalog_generation
    }) {
        session.poisoned = Some("durable journal state does not match resident state".into());
        bail!("resident session requires recovery before catalog mutation");
    }
    let fingerprint = crate::utils::hash_bytes_sha256_hex(
        format!("discard:{change_id}:{}", session.revision()).as_bytes(),
    );
    let prepared = crate::core::resident_storage::PreparedResidentCommit {
        schema_version: crate::core::resident_storage::RESIDENT_COMMIT_SCHEMA.into(),
        session_id: session_id.clone(),
        commit_id: format!("commit_{}", uuid::Uuid::new_v4().simple()),
        parent_commit_id: history.stream_tip.clone(),
        history_parent_commit_id: None,
        base_sha256: session.base_sha256.clone(),
        request_id: request_id.to_string(),
        request_fingerprint: fingerprint.clone(),
        transition: crate::core::resident_storage::ResidentTransition::CatalogDiscard,
        effects: vec![json!({"discard_staged_change_id":change_id})],
        resulting_head: history.head.clone(),
        resulting_branch: history.current_branch.clone(),
        resulting_branches: history.branches.clone(),
        catalog_generation: session.catalog_generation + 1,
        state_revision: session.revision(),
    };
    if let Err(error) = storage.commit(&prepared).await {
        match storage
            .reconcile(&session_id, request_id, &fingerprint)
            .await
        {
            Ok(crate::core::resident_storage::ReconcileOutcome::Committed(_)) => {}
            Ok(crate::core::resident_storage::ReconcileOutcome::NotFound) => return Err(error),
            Err(reconcile) => {
                session.poisoned = Some(format!(
                    "catalog commit outcome unknown: {error}; {reconcile}"
                ));
                bail!("catalog commit outcome unknown; recovery required");
            }
        }
    }
    if !session.discard_staged(change_id)? {
        session.poisoned = Some("committed staged discard could not publish".into());
        bail!("committed catalog transition failed to publish; recovery required");
    }
    Ok(true)
}

pub async fn apply_staged_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    change_id: &str,
    expected_revision: &str,
) -> Result<WriteResponseData> {
    session.ensure_usable()?;
    if request_id.is_empty() {
        return Err(invalid_request(
            "request_id is required for durable staged apply",
        ));
    }
    if let Some(record) = reconciled_record_by_request_id(storage, session, request_id).await? {
        let persisted = prepared_transaction_from_record(&record)
            .ok_or_else(|| anyhow!("stage-apply lacks prepared transaction"))??;
        if record.transition != crate::core::resident_storage::ResidentTransition::StageApply
            || persisted.consume_staged.as_deref() != Some(change_id)
            || persisted.response.revision_before() != expected_revision
        {
            bail!("request identity reuse with different staged-apply input");
        }
        return Ok(persisted.response);
    }
    let bundle = session
        .staged
        .get(change_id)
        .ok_or_else(|| invalid_request(format!("staged change not found: {change_id}")))?
        .clone();
    if bundle.base_revision != session.revision() || expected_revision != session.revision() {
        bail!("revision conflict: staged write approval is stale");
    }
    let resource_id: ResourceId = serde_json::from_value(json!(session.resource_id.clone()))?;
    execute_durable_prepared_on_resident(
        session,
        storage,
        request_id,
        WriteRequest {
            resource_id,
            expected_revision: expected_revision.to_string(),
            mode: WriteMode::Apply,
            atomic: bundle.atomic,
            ops: bundle.ops,
            label: None,
            formula_parse_policy: bundle.formula_parse_policy,
        },
        Some(change_id),
    )
    .await
}

async fn commit_portable_control_transition<
    S: crate::core::resident_storage::ResidentCommitStorage,
>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    transition: crate::core::resident_storage::ResidentTransition,
    effects: Vec<Value>,
    resulting_head: Option<String>,
    resulting_branch: String,
    resulting_branches: BTreeMap<String, Option<String>>,
    target_bytes: Option<Vec<u8>>,
) -> Result<String> {
    session.ensure_usable()?;
    if request_id.is_empty() {
        return Err(invalid_request("request_id is required"));
    }
    let session_id = session.resource_id.split_once(':').unwrap().1.to_string();
    let records = storage.load(&session_id).await?;
    let history = portable_history_state(&records, &session_id, &session.base_sha256)?;
    if history
        .state_revision
        .as_deref()
        .is_some_and(|revision| revision != session.revision())
        || history.catalog_generation != session.catalog_generation
    {
        session.poisoned = Some("portable history state differs from resident owner".into());
        bail!("resident session requires recovery before history transition");
    }
    let catalog_only = matches!(
        transition,
        crate::core::resident_storage::ResidentTransition::Checkpoint
            | crate::core::resident_storage::ResidentTransition::CheckpointDelete
    );
    let receipt = transition == crate::core::resident_storage::ResidentTransition::Receipt;
    let predicted_revision = if catalog_only || receipt {
        session.revision()
    } else if target_bytes.is_some() {
        predicted_resident_revision(session, true)
    } else {
        format!(
            "resident:{}:{}:{}",
            session.workbook.revisions().epoch,
            session.workbook.revisions().document,
            session.workbook.revisions().state + 1,
        )
    };
    let fingerprint = crate::utils::hash_bytes_sha256_hex(&serde_json::to_vec(&(
        session.base_sha256.as_str(),
        &transition,
        &effects,
        &resulting_head,
        &resulting_branch,
        &resulting_branches,
    ))?);
    let proposed = crate::core::resident_storage::PreparedResidentCommit {
        schema_version: crate::core::resident_storage::RESIDENT_COMMIT_SCHEMA.into(),
        session_id: session_id.clone(),
        commit_id: format!("commit_{}", uuid::Uuid::new_v4().simple()),
        parent_commit_id: history.stream_tip.clone(),
        history_parent_commit_id: None,
        base_sha256: session.base_sha256.clone(),
        request_id: request_id.to_string(),
        request_fingerprint: fingerprint.clone(),
        transition,
        effects,
        resulting_head,
        resulting_branch,
        resulting_branches,
        catalog_generation: session.catalog_generation + u64::from(catalog_only),
        state_revision: predicted_revision.clone(),
    };
    let committed_id = match storage.commit(&proposed).await {
        Ok(outcome) => outcome.commit_id,
        Err(error) => match storage
            .reconcile(&session_id, request_id, &fingerprint)
            .await
        {
            Ok(crate::core::resident_storage::ReconcileOutcome::Committed(outcome)) => {
                outcome.commit_id
            }
            Ok(crate::core::resident_storage::ReconcileOutcome::NotFound) => return Err(error),
            Err(reconcile) => {
                session.poisoned = Some(format!(
                    "history commit outcome unknown: {error}; {reconcile}"
                ));
                bail!("history commit outcome unknown; recovery required");
            }
        },
    };
    session.poisoned =
        Some("committed history publication pending; recovery required on failure".into());
    if committed_id != proposed.commit_id {
        let existing = storage
            .load(&session_id)
            .await?
            .into_iter()
            .find(|record| record.commit_id == committed_id)
            .ok_or_else(|| anyhow!("reconciled history commit is not readable"))?;
        if session.revision() == existing.state_revision
            && session.catalog_generation == existing.catalog_generation
        {
            session.poisoned = None;
            return Ok(existing.state_revision);
        }
        bail!("reconciled history transition requires recovery");
    }
    if catalog_only {
        session.catalog_generation += 1;
    } else if receipt {
        // Persisted accepted outcome, no document or catalog publication.
    } else if let Some(bytes) = target_bytes {
        if let Err(error) = session
            .workbook
            .replace_from_bytes(&bytes, crate::recalc::ResidentCalculationEffect::Invalidate)
        {
            session.poisoned = Some(format!("committed history publication failed: {error}"));
            return Err(error);
        }
    } else if proposed.transition
        == crate::core::resident_storage::ResidentTransition::CalculationInvalidate
    {
        session.workbook.revoke_calculation_proof();
    } else {
        session.workbook.advance_metadata_state();
    }
    if session.revision() != predicted_revision {
        session.poisoned = Some("history publication revision mismatch".into());
        bail!("committed history transition failed to publish; recovery required");
    }
    session.poisoned = None;
    Ok(predicted_revision)
}

/// Initial resource creation is an effect-free, same-record canonical receipt.
/// Native publication retains this owner so its acknowledged CAS is immediately usable.
pub(crate) async fn commit_creation_receipt<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    request: &crate::canonical_lifecycle::CreateForkRequest,
) -> Result<String> {
    let session_id = session.resource_id.split_once(':').unwrap().1;
    let records = storage.load(session_id).await?;
    anyhow::ensure!(records.is_empty(), "creation requires an unpublished empty journal");
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    commit_portable_control_transition(session, storage, request_id,
        crate::core::resident_storage::ResidentTransition::Receipt,
        vec![json!({"resource_creation":request})], history.head,
        history.current_branch, history.branches, None).await
}

/// A terminal tombstone is a receipt, not a workbook edit or another outcome log.
pub(crate) async fn commit_discard_receipt<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession, storage: &S, request_id: &str,
    request: &crate::canonical_lifecycle::DiscardForkRequest,
) -> Result<String> {
    session.ensure_usable()?;
    let session_id = session.resource_id.split_once(':').unwrap().1;
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    anyhow::ensure!(!history.discarded, "resource has been discarded");
    commit_portable_control_transition(session, storage, request_id,
        crate::core::resident_storage::ResidentTransition::Receipt,
        vec![json!({"resource_discard":request})],
        history.head, history.current_branch, history.branches, None).await
}

pub(crate) async fn commit_export_receipt<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession, storage: &S, request_id: &str,
    request: &crate::canonical_lifecycle::ExportForkRequest,
    artifact: &crate::canonical_lifecycle::ArtifactMetadata,
) -> Result<String> {
    let session_id = session.resource_id.split_once(':').unwrap().1;
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    commit_portable_control_transition(session, storage, request_id,
        crate::core::resident_storage::ResidentTransition::Receipt,
        vec![json!({"resource_export":{"request":request,"artifact":artifact}})],
        history.head, history.current_branch, history.branches, None).await
}

pub(crate) async fn commit_file_export_receipt<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession, storage: &S, request_id: &str, effect: Value,
) -> Result<String> {
    let session_id = session.resource_id.split_once(':').unwrap().1;
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    commit_portable_control_transition(session, storage, request_id,
        crate::core::resident_storage::ResidentTransition::Receipt, vec![effect],
        history.head, history.current_branch, history.branches, None).await
}

async fn commit_control_receipt<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    operation: crate::core::resident_storage::ResidentTransition,
    mut effects: Vec<Value>,
) -> Result<String> {
    session.ensure_usable()?;
    let session_id = session.resource_id.split_once(':').unwrap().1;
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    effects.push(json!({"receipt_operation":operation}));
    commit_portable_control_transition(
        session,
        storage,
        request_id,
        crate::core::resident_storage::ResidentTransition::Receipt,
        effects,
        history.head,
        history.current_branch,
        history.branches,
        None,
    )
    .await
}

pub async fn revoke_calculation_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
) -> Result<String> {
    session.ensure_usable()?;
    let session_id = session.resource_id.split_once(':').unwrap().1;
    if let Some(record) = reconciled_record_by_request_id(storage, session, request_id).await? {
        if record.transition
            != crate::core::resident_storage::ResidentTransition::CalculationInvalidate
            || !record.effects.iter().any(|effect| {
                effect.get("reason").and_then(Value::as_str) == Some("explicit_proof_revocation")
            })
        {
            bail!("request identity reuse with different calculation input");
        }
        return Ok(record.state_revision);
    }
    if matches!(
        session.diagnostic_workbook().calculation_stamp(),
        crate::recalc::CalculationStamp::Dirty { .. }
    ) {
        return commit_control_receipt(
            session,
            storage,
            request_id,
            crate::core::resident_storage::ResidentTransition::CalculationInvalidate,
            vec![json!({"reason":"explicit_proof_revocation"})],
        )
        .await;
    }
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    commit_portable_control_transition(
        session,
        storage,
        request_id,
        crate::core::resident_storage::ResidentTransition::CalculationInvalidate,
        vec![json!({"reason":"explicit_proof_revocation"})],
        history.head.clone(),
        history.current_branch,
        history.branches,
        None,
    )
    .await
}

pub async fn recalculate_durable_on_resident<
    S: crate::core::resident_storage::ResidentCommitStorage,
>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    timeout_ms: Option<u64>,
) -> Result<crate::model::EvaluationCoverage> {
    recalculate_durable_with_backend(session, storage, request_id, timeout_ms, None).await
}

pub async fn recalculate_durable_with_backend<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    timeout_ms: Option<u64>,
    backend: Option<std::sync::Arc<dyn crate::recalc::RecalcBackend>>,
) -> Result<crate::model::EvaluationCoverage> {
    let backend_name = backend.as_ref().map(|backend| backend.name()).unwrap_or("formualizer");
    session.ensure_usable()?;
    if request_id.is_empty() {
        return Err(invalid_request("request_id is required"));
    }
    let session_id = session.resource_id.split_once(':').unwrap().1.to_string();
    if let Some(record) = reconciled_record_by_request_id(storage, session, request_id).await? {
        if record.transition
            != crate::core::resident_storage::ResidentTransition::CalculationPublish
            || record
                .effects
                .iter()
                .find_map(|effect| effect.get("timeout_ms"))
                != Some(&serde_json::to_value(timeout_ms)?)
        {
            bail!("request identity reuse with different calculation input");
        }
        let snapshot = record
            .effects
            .iter()
            .find_map(|effect| effect.get("calculation_proof"))
            .ok_or_else(|| anyhow!("calculation publication lacks snapshot"))?;
        if snapshot.get("backend").and_then(Value::as_str).unwrap_or("formualizer") != backend_name {
            bail!("request identity reuse with different calculation backend");
        }
        return Ok(serde_json::from_value(snapshot["coverage"].clone())?);
    }
    let prepared = if let Some(backend) = backend {
        #[cfg(feature = "native-fs")]
        { session.workbook.prepare_external_calculation(backend, timeout_ms).await
            .map(|(evaluation, date_system, outcome)| (evaluation, date_system, outcome.duration_ms, Some(outcome))) }
        #[cfg(not(feature = "native-fs"))]
        { let _ = backend; bail!("external calculation requires a native filesystem host"); }
    } else {
        session.workbook.prepare_calculation(timeout_ms)
            .map(|(evaluation, date_system, duration)| (evaluation, date_system, duration, None::<crate::core::types::RecalculateOutcome>))
    };
    let (evaluation, date_system, evaluation_duration_ms, external) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            session.poisoned = Some(format!(
                "calculation failed before proof publication: {error}; recovery required"
            ));
            return Err(error);
        }
    };
    let mut coverage = crate::model::EvaluationCoverage {
        formula_cells: evaluation.formula_cells,
        evaluated_formula_cells: evaluation.formula_cells,
        unsupported_formula_cells: 0,
        error_formula_cells: evaluation.error_formula_cells,
        source: crate::model::EvaluationSource::Formualizer,
        freshness: crate::model::EvaluationFreshness::CurrentRevision,
        revision_id: String::new(),
    };
    if let Some(outcome) = &external {
        coverage = outcome.evaluation_coverage.clone();
    }
    let predicted_revision = format!(
        "resident:{}:{}:{}",
        session.workbook.revisions().epoch,
        session.workbook.revisions().document,
        session.workbook.revisions().state + 1,
    );
    coverage.revision_id = predicted_revision.clone();
    let records = storage.load(&session_id).await?;
    let history = portable_history_state(&records, &session_id, &session.base_sha256)?;
    if history
        .state_revision
        .as_deref()
        .is_some_and(|revision| revision != session.revision())
        || history.catalog_generation != session.catalog_generation
    {
        session.poisoned =
            Some("portable history state differs before calculation publication".into());
        bail!("resident session requires recovery before calculation");
    }
    let mut effects = vec![
        json!({"timeout_ms":timeout_ms}),
        json!({"calculation_proof":{
            "evaluation_duration_ms":evaluation_duration_ms,
            "eval_errors": evaluation.eval_errors,
            "cells_evaluated": evaluation.cells_evaluated,
            "coverage":coverage,
        }}),
    ];
    if let Some(outcome) = external {
        effects[1]["calculation_proof"]["backend"] = json!(backend_name);
        effects[1]["calculation_proof"]["external_result"] = serde_json::to_value(
            crate::canonical_lifecycle::RecalculateData {
                revision_before: session.revision(), revision_after: predicted_revision.clone(),
                backend: outcome.backend, duration_ms: outcome.duration_ms,
                state: coverage.state(), status: if coverage.state() == crate::model::EvaluationState::Clean {
                    "completed".into()
                } else { "completed_with_errors".into() },
                error_count: outcome.eval_errors.as_ref().map(Vec::len),
                cells_evaluated: outcome.cells_evaluated, eval_errors: outcome.eval_errors,
                evaluation_coverage: coverage.clone(), warnings: vec![],
            }
        )?;
    }
    let mut fingerprint_input = serde_json::to_value((
        session.base_sha256.as_str(), "calculate", session.revision(), timeout_ms,
    ))?;
    if backend_name != "formualizer" {
        fingerprint_input.as_array_mut().unwrap().push(json!(backend_name));
    }
    let fingerprint = crate::utils::hash_bytes_sha256_hex(&serde_json::to_vec(&fingerprint_input)?);
    let proposed = crate::core::resident_storage::PreparedResidentCommit {
        schema_version: crate::core::resident_storage::RESIDENT_COMMIT_SCHEMA.into(),
        session_id: session_id.clone(),
        commit_id: format!("commit_{}", uuid::Uuid::new_v4().simple()),
        parent_commit_id: history.stream_tip.clone(),
        history_parent_commit_id: None,
        base_sha256: session.base_sha256.clone(),
        request_id: request_id.to_string(),
        request_fingerprint: fingerprint.clone(),
        transition: crate::core::resident_storage::ResidentTransition::CalculationPublish,
        effects: effects.clone(),
        resulting_head: history.head.clone(),
        resulting_branch: history.current_branch.clone(),
        resulting_branches: history.branches.clone(),
        catalog_generation: history.catalog_generation,
        state_revision: predicted_revision.clone(),
    };
    let committed = match storage.commit(&proposed).await {
        Ok(outcome) => outcome,
        Err(error) => match storage
            .reconcile(&session_id, request_id, &fingerprint)
            .await
        {
            Ok(crate::core::resident_storage::ReconcileOutcome::Committed(outcome)) => outcome,
            Ok(crate::core::resident_storage::ReconcileOutcome::NotFound) => return Err(error),
            Err(reconcile) => {
                session.poisoned = Some(format!(
                    "calculation commit outcome unknown: {error}; {reconcile}"
                ));
                bail!("calculation commit outcome unknown; recovery required");
            }
        },
    };
    session.poisoned =
        Some("committed calculation publication pending; recovery required on failure".into());
    if committed.commit_id != proposed.commit_id {
        let record = storage
            .load(&session_id)
            .await?
            .into_iter()
            .find(|record| record.commit_id == committed.commit_id)
            .ok_or_else(|| anyhow!("reconciled calculation record missing"))?;
        if session.revision() == record.state_revision
            && session.catalog_generation == record.catalog_generation
        {
            let coverage = record
                .effects
                .iter()
                .find_map(|effect| effect.get("calculation_proof"))
                .ok_or_else(|| anyhow!("calculation publication lacks snapshot"))?["coverage"]
                .clone();
            let coverage = serde_json::from_value(coverage)?;
            session.poisoned = None;
            return Ok(coverage);
        }
        bail!("reconciled calculation publication requires recovery");
    }
    if let Err(error) =
        session
            .workbook
            .publish_prepared_calculation(evaluation, date_system, coverage.clone())
    {
        session.poisoned = Some(format!("committed calculation publication failed: {error}"));
        return Err(error);
    }
    session.poisoned = None;
    Ok(coverage)
}

pub async fn checkpoint_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    label: Option<&str>,
) -> Result<String> {
    session.ensure_usable()?;
    let session_id = session.resource_id.split_once(':').unwrap().1;
    let effects = vec![json!({"label":label})];
    if let Some(record) = reconciled_record_by_request_id(storage, session, request_id).await? {
        if record.transition != crate::core::resident_storage::ResidentTransition::Checkpoint
            || !record.effects.iter().filter(|effect| effect.get("canonical_outcome").is_none()).eq(effects.iter())
        {
            bail!("request identity reuse with different checkpoint input");
        }
        return Ok(record.state_revision);
    }
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    commit_portable_control_transition(
        session,
        storage,
        request_id,
        crate::core::resident_storage::ResidentTransition::Checkpoint,
        effects,
        history.head.clone(),
        history.current_branch,
        history.branches,
        None,
    )
    .await
}

pub async fn list_checkpoints_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &ResidentWriteSession,
    storage: &S,
) -> Result<Vec<crate::core::resident_storage::ResidentCheckpoint>> {
    session.ensure_usable()?;
    let session_id = session.resource_id.split_once(':').unwrap().1;
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    Ok(history.checkpoints.into_values().collect())
}

pub async fn restore_checkpoint_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    checkpoint_id: &str,
) -> Result<String> {
    session.ensure_usable()?;
    if let Some(record) = reconciled_record_by_request_id(storage, session, request_id).await? {
        if record.transition != crate::core::resident_storage::ResidentTransition::Checkout
            || !record
                .effects
                .iter()
                .any(|e| e.get("checkpoint_id").and_then(Value::as_str) == Some(checkpoint_id))
        {
            bail!("request identity reuse with different checkpoint restore input");
        }
        return Ok(record.state_revision);
    }
    let session_id = session.resource_id.split_once(':').unwrap().1;
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    let checkpoint = history
        .checkpoints
        .get(checkpoint_id)
        .ok_or_else(|| anyhow!("unknown checkpoint"))?;
    let target = checkpoint.head.clone();
    let bytes = materialize_portable_head(
        &session.resource_id,
        &session.base_bytes,
        &records,
        target.as_deref(),
    )?;
    commit_portable_control_transition(
        session,
        storage,
        request_id,
        crate::core::resident_storage::ResidentTransition::Checkout,
        vec![json!({"checkpoint_id":checkpoint_id,"target_head":target})],
        target,
        history.current_branch,
        history.branches,
        Some(bytes),
    )
    .await
}

pub async fn delete_checkpoint_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    checkpoint_id: &str,
) -> Result<String> {
    session.ensure_usable()?;
    let session_id = session.resource_id.split_once(':').unwrap().1;
    let effects = vec![json!({"checkpoint_id":checkpoint_id})];
    if let Some(record) = reconciled_record_by_request_id(storage, session, request_id).await? {
        if record.transition != crate::core::resident_storage::ResidentTransition::CheckpointDelete
            || !record.effects.iter().filter(|effect| effect.get("canonical_outcome").is_none()).eq(effects.iter())
        {
            bail!("request identity reuse with different checkpoint deletion input");
        }
        return Ok(record.state_revision);
    }
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    if !history.checkpoints.contains_key(checkpoint_id) {
        bail!("unknown checkpoint");
    }
    commit_portable_control_transition(
        session,
        storage,
        request_id,
        crate::core::resident_storage::ResidentTransition::CheckpointDelete,
        effects,
        history.head,
        history.current_branch,
        history.branches,
        None,
    )
    .await
}

pub async fn create_branch_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    branch_name: &str,
) -> Result<String> {
    create_branch_at_durable(session, storage, request_id, branch_name, None, None).await
}

/// Atomically create a branch at a validated mutation or immutable base. This
/// never moves the selected head and does not materialize another evaluator.
pub async fn create_branch_at_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    branch_name: &str,
    target_commit_id: Option<&str>,
    label: Option<&str>,
) -> Result<String> {
    session.ensure_usable()?;
    if label.is_some_and(|label| label.len() > 1024)
        || target_commit_id.is_some_and(|target| target.is_empty() || target.len() > 256)
    {
        return Err(invalid_request("invalid branch target or label length"));
    }
    if branch_name.is_empty()
        || !branch_name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalid_request("invalid branch name"));
    }
    let session_id = session.resource_id.split_once(':').unwrap().1;
    if let Some(record) = reconciled_record_by_request_id(storage, session, request_id).await? {
        if record.transition != crate::core::resident_storage::ResidentTransition::BranchCreate
            || !record.effects.iter().any(|effect| {
                effect.get("branch_name").and_then(Value::as_str) == Some(branch_name)
                    && effect.get("requested_target").and_then(Value::as_str) == target_commit_id
                    && effect.get("branch_label").and_then(Value::as_str) == label
            })
        {
            bail!("request identity reuse with different branch-create input");
        }
        return Ok(record.state_revision);
    }
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    if history.branches.contains_key(branch_name) {
        bail!("branch already exists");
    }
    let target = match target_commit_id {
        None => history.head.clone(),
        Some("base") => None,
        Some(target) if records.iter().any(|record| {
            record.commit_id == target
                && matches!(record.transition, crate::core::resident_storage::ResidentTransition::Mutation | crate::core::resident_storage::ResidentTransition::StageApply)
        }) => Some(target.to_owned()),
        Some(_) => return Err(invalid_request("unknown branch mutation target")),
    };
    let mut branches = history.branches.clone();
    branches.insert(branch_name.to_string(), target.clone());
    let effects = if target_commit_id.is_none() && label.is_none() {
        vec![json!({"branch_name":branch_name})]
    } else {
        vec![json!({"branch_name":branch_name,"branch_target":target,
            "requested_target":target_commit_id,"branch_label":label})]
    };
    commit_portable_control_transition(
        session,
        storage,
        request_id,
        crate::core::resident_storage::ResidentTransition::BranchCreate,
        effects,
        history.head,
        history.current_branch,
        branches,
        None,
    )
    .await
}

pub async fn switch_branch_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    branch_name: &str,
    base_bytes: &[u8],
) -> Result<String> {
    session.validate_base(base_bytes)?;
    let session_id = session.resource_id.split_once(':').unwrap().1;
    if let Some(record) = reconciled_record_by_request_id(storage, session, request_id).await? {
        if record.transition != crate::core::resident_storage::ResidentTransition::BranchSwitch
            || !record.effects.iter().any(|effect| {
                effect.get("branch_name").and_then(Value::as_str) == Some(branch_name)
            })
        {
            bail!("request identity reuse with different branch-switch input");
        }
        return Ok(record.state_revision);
    }
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    let target = history
        .branches
        .get(branch_name)
        .cloned()
        .ok_or_else(|| invalid_request("unknown branch"))?;
    let target_bytes = (target != history.head)
        .then(|| {
            materialize_portable_head(
                &session.resource_id,
                base_bytes,
                &records,
                target.as_deref(),
            )
        })
        .transpose()?;
    commit_portable_control_transition(
        session,
        storage,
        request_id,
        crate::core::resident_storage::ResidentTransition::BranchSwitch,
        vec![json!({"branch_name":branch_name})],
        target,
        branch_name.to_string(),
        history.branches,
        target_bytes,
    )
    .await
}

async fn move_head_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    transition: crate::core::resident_storage::ResidentTransition,
    target: Option<String>,
    base_bytes: &[u8],
) -> Result<String> {
    session.validate_base(base_bytes)?;
    let session_id = session.resource_id.split_once(':').unwrap().1;
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    let target_bytes = materialize_portable_head(
        &session.resource_id,
        base_bytes,
        &records,
        target.as_deref(),
    )?;
    commit_portable_control_transition(
        session,
        storage,
        request_id,
        transition,
        vec![json!({"target_head":target})],
        target,
        history.current_branch,
        history.branches,
        Some(target_bytes),
    )
    .await
}

pub async fn undo_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    base_bytes: &[u8],
) -> Result<String> {
    session.validate_base(base_bytes)?;
    let session_id = session.resource_id.split_once(':').unwrap().1;
    if let Some(record) = reconciled_record_by_request_id(storage, session, request_id).await? {
        if record.transition != crate::core::resident_storage::ResidentTransition::Undo {
            bail!("request identity reuse with different history input");
        }
        return Ok(record.state_revision);
    }
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    let Some(head) = history.head.as_deref() else {
        return commit_control_receipt(
            session,
            storage,
            request_id,
            crate::core::resident_storage::ResidentTransition::Undo,
            vec![],
        )
        .await;
    };
    let target = history.history_parent(head)?;
    move_head_durable(
        session,
        storage,
        request_id,
        crate::core::resident_storage::ResidentTransition::Undo,
        target,
        base_bytes,
    )
    .await
}

pub async fn redo_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    base_bytes: &[u8],
) -> Result<String> {
    session.validate_base(base_bytes)?;
    let session_id = session.resource_id.split_once(':').unwrap().1;
    if let Some(record) = reconciled_record_by_request_id(storage, session, request_id).await? {
        if record.transition != crate::core::resident_storage::ResidentTransition::Redo {
            bail!("request identity reuse with different history input");
        }
        return Ok(record.state_revision);
    }
    let records = storage.load(session_id).await?;
    let history = portable_history_state(&records, session_id, &session.base_sha256)?;
    let Some(target) = history.redo_target()? else {
        return commit_control_receipt(
            session,
            storage,
            request_id,
            crate::core::resident_storage::ResidentTransition::Redo,
            vec![],
        )
        .await;
    };
    move_head_durable(
        session,
        storage,
        request_id,
        crate::core::resident_storage::ResidentTransition::Redo,
        Some(target),
        base_bytes,
    )
    .await
}

pub async fn checkout_durable<S: crate::core::resident_storage::ResidentCommitStorage>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    target: &str,
    base_bytes: &[u8],
) -> Result<String> {
    session.validate_base(base_bytes)?;
    if let Some(record) = reconciled_record_by_request_id(storage, session, request_id).await? {
        if record.transition != crate::core::resident_storage::ResidentTransition::Checkout
            || !record
                .effects
                .iter()
                .any(|effect| effect.get("target_head").and_then(Value::as_str) == Some(target))
        {
            bail!("request identity reuse with different checkout input");
        }
        return Ok(record.state_revision);
    }
    move_head_durable(
        session,
        storage,
        request_id,
        crate::core::resident_storage::ResidentTransition::Checkout,
        Some(target.to_string()),
        base_bytes,
    )
    .await
}

/// Reconstruct deterministic committed effects, invalidate volatile evaluation
/// context at the restart boundary, and durably record that state transition
/// before returning a writable session.
pub async fn recover_durable_resident_session<
    S: crate::core::resident_storage::ResidentCommitStorage,
>(
    resource_id: impl Into<String>,
    base_bytes: &[u8],
    storage: &S,
) -> Result<ResidentWriteSession> {
    let resource_id = resource_id.into();
    let parsed: ResourceId = serde_json::from_value(json!(resource_id.clone()))?;
    let session_id = parsed.to_workbook_id().0;
    let records = storage.load(&session_id).await?;
    let session = ResidentWriteSession::recover_from_records(resource_id, base_bytes, &records)?;
    if records.is_empty() {
        return Ok(session);
    }
    let history = portable_history_state(
        &records,
        &session_id,
        &crate::utils::hash_bytes_sha256_hex(base_bytes),
    )?;
    // Terminal resource receipts retain their original outcomes but cannot gain
    // a new restart/live-CAS transition. The runtime fences all workbook access.
    if history.discarded {
        return Ok(session);
    }
    let fingerprint = crate::utils::hash_bytes_sha256_hex(
        format!(
            "restart:{}:{}",
            records.last().unwrap().commit_id,
            session.revision()
        )
        .as_bytes(),
    );
    let restart_request_id = format!("restart_{}", uuid::Uuid::new_v4().simple());
    let prepared = crate::core::resident_storage::PreparedResidentCommit {
        schema_version: crate::core::resident_storage::RESIDENT_COMMIT_SCHEMA.into(),
        session_id,
        commit_id: format!("commit_{}", uuid::Uuid::new_v4().simple()),
        parent_commit_id: history.stream_tip.clone(),
        history_parent_commit_id: None,
        base_sha256: history.base_sha256.clone(),
        request_id: restart_request_id.clone(),
        request_fingerprint: fingerprint.clone(),
        transition: crate::core::resident_storage::ResidentTransition::Restart,
        effects: vec![json!({"restart_boundary":true})],
        resulting_head: history.head.clone(),
        resulting_branch: history.current_branch.clone(),
        resulting_branches: history.branches.clone(),
        catalog_generation: session.catalog_generation,
        state_revision: session.revision(),
    };
    if let Err(error) = storage.commit(&prepared).await {
        match storage
            .reconcile(&prepared.session_id, &restart_request_id, &fingerprint)
            .await
        {
            Ok(crate::core::resident_storage::ReconcileOutcome::Committed(_)) => {}
            Ok(crate::core::resident_storage::ReconcileOutcome::NotFound) => return Err(error),
            Err(reconcile) => {
                bail!("restart commit outcome unknown: {error}; reconciliation failed: {reconcile}")
            }
        }
    }
    Ok(session)
}

/// Journal a prepared canonical transition before publishing it. If the host
/// cannot establish commit outcome, the session is poisoned until recovery;
/// callers never receive a false effect-free failure.
async fn execute_durable_prepared_on_resident<
    S: crate::core::resident_storage::ResidentCommitStorage,
>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    request: WriteRequest,
    consume_staged: Option<&str>,
) -> Result<WriteResponseData> {
    if request_id.is_empty() {
        return Err(invalid_request("request_id is required for durable writes"));
    }
    session.ensure_usable()?;
    let session_id = request.resource_id.to_workbook_id().0;
    let existing = storage.load(&session_id).await?;
    let history = portable_history_state(&existing, &session_id, &session.base_sha256)?;
    if let Some(last) = existing.last()
        && (last.state_revision != session.revision()
            || last.catalog_generation != session.catalog_generation)
    {
        session.poisoned =
            Some("durable journal state does not match resident state/catalog".into());
        bail!("resident session requires recovery before durable write");
    }
    let fingerprint = crate::utils::hash_bytes_sha256_hex(&serde_json::to_vec(&(
        session.base_sha256.as_str(),
        consume_staged,
        &request,
    ))?);
    if let crate::core::resident_storage::ReconcileOutcome::Committed(outcome) = storage
        .reconcile(&session_id, request_id, &fingerprint)
        .await?
    {
        let committed = existing
            .iter()
            .find(|record| record.commit_id == outcome.commit_id)
            .ok_or_else(|| anyhow!("reconciled resident commit is not readable"))?;
        let persisted: PreparedResidentTransaction = serde_json::from_value(
            committed
                .effects
                .iter()
                .find_map(|effect| effect.get("prepared_transaction"))
                .cloned()
                .ok_or_else(|| anyhow!("resident commit lacks prepared transaction"))?,
        )?;
        return Ok(persisted.response);
    }
    let stage_change_id =
        (request.mode == WriteMode::Stage).then(|| make_short_random_id("chg", 12));
    let mut prepared_transaction =
        prepare_resident_transaction(session, &request, stage_change_id)?;
    prepared_transaction.consume_staged = consume_staged.map(str::to_string);
    if request.mode == WriteMode::Preview
        || matches!(
            prepared_transaction.response,
            WriteResponseData::Failed { .. } | WriteResponseData::RolledBack { .. }
        )
    {
        return Ok(prepared_transaction.response);
    }
    let transition = if matches!(
        prepared_transaction.publication,
        PreparedResidentPublication::None
    ) && prepared_transaction.staged.is_none()
        && prepared_transaction.consume_staged.is_none()
    {
        crate::core::resident_storage::ResidentTransition::Receipt
    } else if prepared_transaction.consume_staged.is_some() {
        crate::core::resident_storage::ResidentTransition::StageApply
    } else if prepared_transaction.staged.is_some() {
        crate::core::resident_storage::ResidentTransition::CatalogStage
    } else {
        crate::core::resident_storage::ResidentTransition::Mutation
    };
    let is_mutation = !matches!(
        prepared_transaction.publication,
        PreparedResidentPublication::None
    );
    let commit_id = format!("commit_{}", uuid::Uuid::new_v4().simple());
    let resulting_head = if is_mutation {
        Some(commit_id.clone())
    } else {
        history.head.clone()
    };
    let mut resulting_branches = history.branches.clone();
    if is_mutation {
        resulting_branches.insert(history.current_branch.clone(), resulting_head.clone());
    }
    let proposed = crate::core::resident_storage::PreparedResidentCommit {
        schema_version: crate::core::resident_storage::RESIDENT_COMMIT_SCHEMA.into(),
        session_id: session_id.clone(),
        commit_id,
        parent_commit_id: history.stream_tip.clone(),
        history_parent_commit_id: is_mutation.then(|| history.head.clone()).flatten(),
        base_sha256: session.base_sha256.clone(),
        request_id: request_id.to_string(),
        request_fingerprint: fingerprint.clone(),
        transition,
        effects: vec![json!({"prepared_transaction": prepared_transaction.clone()})],
        resulting_head,
        resulting_branch: history.current_branch.clone(),
        resulting_branches,
        catalog_generation: session.catalog_generation
            + u64::from(
                prepared_transaction.staged.is_some()
                    || prepared_transaction.consume_staged.is_some(),
            ),
        state_revision: prepared_transaction.response.revision_after().to_string(),
    };
    let committed_id = match storage.commit(&proposed).await {
        Ok(outcome) => outcome.commit_id,
        Err(commit_error) => match storage
            .reconcile(&session_id, request_id, &fingerprint)
            .await
        {
            Ok(crate::core::resident_storage::ReconcileOutcome::Committed(outcome)) => {
                outcome.commit_id
            }
            Ok(crate::core::resident_storage::ReconcileOutcome::NotFound) => {
                return Err(commit_error);
            }
            Err(reconcile_error) => {
                session.poisoned = Some(format!(
                    "durable commit outcome unknown: {commit_error}; reconciliation failed: {reconcile_error}"
                ));
                bail!("durable commit outcome unknown; recovery/reconciliation required");
            }
        },
    };
    session.poisoned =
        Some("committed transaction publication pending; recovery required on failure".into());
    let is_new_commit = committed_id == proposed.commit_id;
    let committed = if is_new_commit {
        proposed
    } else {
        storage
            .load(&session_id)
            .await?
            .into_iter()
            .find(|record| record.commit_id == committed_id)
            .ok_or_else(|| anyhow!("reconciled resident commit is not readable"))?
    };
    let persisted: PreparedResidentTransaction = serde_json::from_value(
        committed
            .effects
            .iter()
            .find_map(|effect| effect.get("prepared_transaction"))
            .cloned()
            .ok_or_else(|| anyhow!("resident commit lacks prepared transaction"))?,
    )?;
    // A retained request identity denotes its original outcome, not a command
    // to replay old effects into a newer document/catalog state.
    if !is_new_commit {
        session.poisoned = None;
        return Ok(persisted.response);
    }
    if session.revision() == committed.state_revision
        && session.catalog_generation == committed.catalog_generation
    {
        session.poisoned = None;
        return Ok(persisted.response);
    }
    if let Err(error) = publish_resident_transaction(session, &persisted) {
        session.poisoned = Some(format!("post-commit prepared publication failed: {error}"));
        bail!("durable commit succeeded but publication failed; recovery required");
    }
    if session.revision() != committed.state_revision
        || session.catalog_generation != committed.catalog_generation
    {
        session.poisoned = Some("post-commit state/catalog differs from committed record".into());
        bail!("durable publication invariant failed; recovery required");
    }
    session.poisoned = None;
    Ok(persisted.response)
}

pub async fn execute_durable_write_on_resident<
    S: crate::core::resident_storage::ResidentCommitStorage,
>(
    session: &mut ResidentWriteSession,
    storage: &S,
    request_id: &str,
    request: WriteRequest,
) -> Result<WriteResponseData> {
    execute_durable_prepared_on_resident(session, storage, request_id, request, None).await
}

pub fn execute_write_on_bytes(
    bytes: &[u8],
    current_revision: &str,
    request: WriteRequest,
) -> Result<(WriteResponseData, Option<Vec<u8>>)> {
    let mut backend = ByteSessionBackend {
        bytes,
        revision: current_revision,
        committed: None,
    };
    let response = execute_write_transaction(&mut backend, request)?;
    Ok((response, backend.committed))
}

#[cfg(feature = "native-fs")]
pub struct LegacyResidentImport {
    pub base_bytes: Vec<u8>,
    pub records: Vec<crate::core::resident_storage::PreparedResidentCommit>,
    /// Legacy HEAD-only approvals cannot prove applicability and must be re-previewed.
    pub rejected_staged_ids: Vec<String>,
}

#[cfg(feature = "native-fs")]
pub fn import_legacy_session(
    workspace_root: &Path,
    session_id: &str,
) -> Result<LegacyResidentImport> {
    use crate::core::binlog::BranchesFile;
    use crate::core::session_store::SessionStore;
    use anyhow::Context;
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(invalid_request("invalid legacy session id"));
    }
    let handle = SessionStore::open(workspace_root)?.open_frozen_import(session_id)?;
    let base_bytes = fs::read(handle.base_path())?;
    let base_sha256 = crate::utils::hash_bytes_sha256_hex(&base_bytes);
    // Import only a private frozen copy made while legacy writers were excluded.
    // This marker is an explicit host ownership assertion, not a substitute for
    // fencing older binaries during cutover.
    let frozen: Value = serde_json::from_slice(
        &fs::read(handle.dir().join("resident-import-frozen.json")).context(
            "legacy import requires a frozen copy and resident-import-frozen.json binding",
        )?,
    )?;
    if frozen.get("frozen").and_then(Value::as_bool) != Some(true)
        || frozen.get("base_sha256").and_then(Value::as_str) != Some(base_sha256.as_str())
    {
        bail!("legacy frozen import base binding mismatch");
    }
    let events = handle.read_events()?;
    let mut by_id = BTreeMap::new();
    let mut previous_event_hash: Option<String> = None;
    for event in &events {
        if event.session_id != session_id || !event.verify_integrity() {
            bail!(
                "legacy event '{}' failed session/payload/hash validation",
                event.op_id
            );
        }
        if event.prev_event_hash != previous_event_hash {
            bail!(
                "legacy event append hash chain is corrupt at '{}'",
                event.op_id
            );
        }
        previous_event_hash = event.event_hash.clone();
        if by_id.insert(event.op_id.clone(), event).is_some() {
            bail!("duplicate legacy event id '{}'", event.op_id);
        }
    }
    for event in &events {
        if let Some(parent) = &event.parent_id
            && !by_id.contains_key(parent)
        {
            bail!(
                "legacy event '{}' references missing parent '{}'",
                event.op_id,
                parent
            );
        }
    }
    let branches = BranchesFile::load(&handle.branches_path())?;
    let current_branch = fs::read_to_string(handle.current_branch_path())?
        .trim()
        .to_string();
    let projected_head = if handle.head_path().exists() {
        let head = fs::read_to_string(handle.head_path())?.trim().to_string();
        (!head.is_empty()).then_some(head)
    } else {
        None
    };
    let mut branch_tips = BTreeMap::new();
    for branch in &branches.branches {
        if branch_tips
            .insert(branch.name.clone(), branch.tip_op_id.clone())
            .is_some()
        {
            bail!("duplicate legacy branch '{}'", branch.name);
        }
    }
    if !branch_tips.contains_key(&current_branch) {
        bail!("legacy CURRENT_BRANCH references an unknown branch");
    }
    let ancestry = |tip: Option<&str>| -> Result<Vec<String>> {
        let mut result = Vec::new();
        let mut cursor = tip.map(str::to_string);
        let mut seen = BTreeSet::new();
        while let Some(id) = cursor {
            if !seen.insert(id.clone()) {
                bail!("legacy event ancestry cycle");
            }
            let event = by_id
                .get(&id)
                .ok_or_else(|| anyhow!("legacy branch references unknown event '{id}'"))?;
            result.push(id);
            cursor = event.parent_id.clone();
        }
        result.reverse();
        Ok(result)
    };
    let mut ordered = Vec::new();
    let main_path = ancestry(branch_tips.get("main").and_then(|tip| tip.as_deref()))?;
    ordered.extend(main_path.clone());
    for (name, tip) in &branch_tips {
        if name == "main" {
            continue;
        }
        for id in ancestry(tip.as_deref())? {
            if !ordered.contains(&id) {
                ordered.push(id);
            }
        }
    }
    if ordered.len() != events.len() {
        bail!("legacy event log contains orphan events not committed by any branch");
    }

    let epoch = crate::utils::hash_bytes_sha256_hex(
        format!("legacy-import:{session_id}:{base_sha256}").as_bytes(),
    );
    let mut records = Vec::new();
    let mut document_revision = 0u64;
    let mut state_generation = 0u64;
    let append_control = |transition: crate::core::resident_storage::ResidentTransition,
                          effects: Vec<Value>,
                          head: Option<String>,
                          branch: String,
                          branches: BTreeMap<String, Option<String>>,
                          records: &mut Vec<
        crate::core::resident_storage::PreparedResidentCommit,
    >,
                          state_generation: &mut u64,
                          document_revision: &mut u64| {
        if matches!(
            transition,
            crate::core::resident_storage::ResidentTransition::Undo
                | crate::core::resident_storage::ResidentTransition::Checkout
        ) || (transition == crate::core::resident_storage::ResidentTransition::BranchSwitch
            && records.last().and_then(|r| r.resulting_head.clone()) != head)
        {
            *document_revision += 1;
        }
        *state_generation += 1;
        let commit_id = format!("import_control_{:016x}", *state_generation);
        records.push(crate::core::resident_storage::PreparedResidentCommit {
            schema_version: crate::core::resident_storage::RESIDENT_COMMIT_SCHEMA.into(),
            session_id: session_id.into(),
            commit_id: commit_id.clone(),
            parent_commit_id: records.last().map(|record| record.commit_id.clone()),
            history_parent_commit_id: None,
            base_sha256: base_sha256.clone(),
            request_id: format!("legacy_control_{commit_id}"),
            request_fingerprint: crate::utils::hash_bytes_sha256_hex(commit_id.as_bytes()),
            transition,
            effects,
            resulting_head: head,
            resulting_branch: branch,
            resulting_branches: branches,
            catalog_generation: 0,
            state_revision: format!("resident:{epoch}:{document_revision}:{}", *state_generation),
        });
    };

    // Import main first, then explicit branch divergences. Synthetic control
    // records preserve the distinction between stream order and ancestry.
    let mut active_branch = "main".to_string();
    let mut active_head: Option<String> = None;
    let mut imported_branches = BTreeMap::from([("main".to_string(), None)]);
    for legacy_id in ordered {
        let event = *by_id
            .get(&legacy_id)
            .ok_or_else(|| anyhow!("legacy event identity missing"))?;
        let desired_branch = branch_tips
            .iter()
            .find(|(name, tip)| {
                *name != "main"
                    && ancestry(tip.as_deref())
                        .map(|p| p.contains(&legacy_id))
                        .unwrap_or(false)
                    && !main_path.contains(&legacy_id)
            })
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "main".into());
        if desired_branch != active_branch {
            let fork = event.parent_id.clone();
            if active_head != fork {
                append_control(
                    crate::core::resident_storage::ResidentTransition::Checkout,
                    vec![json!({"target_head":fork})],
                    fork.clone(),
                    active_branch.clone(),
                    imported_branches.clone(),
                    &mut records,
                    &mut state_generation,
                    &mut document_revision,
                );
                active_head = fork.clone();
            }
            if !imported_branches.contains_key(&desired_branch) {
                imported_branches.insert(desired_branch.clone(), fork.clone());
                append_control(
                    crate::core::resident_storage::ResidentTransition::BranchCreate,
                    vec![json!({"branch_name":desired_branch})],
                    active_head.clone(),
                    active_branch.clone(),
                    imported_branches.clone(),
                    &mut records,
                    &mut state_generation,
                    &mut document_revision,
                );
            }
            active_branch = desired_branch.clone();
            active_head = imported_branches
                .get(&desired_branch)
                .cloned()
                .ok_or_else(|| anyhow!("legacy branch was not imported"))?;
            append_control(
                crate::core::resident_storage::ResidentTransition::BranchSwitch,
                vec![json!({"branch_name":desired_branch})],
                active_head.clone(),
                active_branch.clone(),
                imported_branches.clone(),
                &mut records,
                &mut state_generation,
                &mut document_revision,
            );
        }
        let predecessor = match event.parent_id.as_deref() {
            Some(parent) => handle.materialize_at(parent)?,
            None => base_bytes.clone(),
        };
        let after = handle.materialize_at(&legacy_id)?;
        document_revision += 1;
        state_generation += 1;
        let revision_before = format!(
            "resident:{epoch}:{}:{}",
            document_revision - 1,
            state_generation - 1
        );
        let revision_after = format!("resident:{epoch}:{document_revision}:{state_generation}");
        let prepared = PreparedResidentTransaction {
            base_sha256: base_sha256.clone(),
            response: WriteResponseData::Applied {
                mode: WriteMode::Apply,
                atomic: true,
                revision_before,
                revision_after: revision_after.clone(),
                ops_applied: 1,
                diff: WriteDiff {
                    change_count: 0,
                    exact: false,
                    precision: "legacy_materialized_snapshot".into(),
                    changes: vec![],
                    effects: vec![],
                },
                impact: WriteImpact {
                    op_kinds: vec![event.kind.to_string()],
                    risk: OperationRisk::Moderate,
                },
                results: vec![],
            },
            publication: PreparedResidentPublication::Snapshot {
                expected_before_sha256: logical_workbook_sha256(&predecessor)?,
                sha256: crate::utils::hash_bytes_sha256_hex(&after),
                bytes: after.clone(),
                calculation_effect: crate::recalc::ResidentCalculationEffect::Invalidate,
            },
            staged: None,
            consume_staged: None,
        };
        let commit_id = legacy_id.clone();
        active_head = Some(commit_id.clone());
        imported_branches.insert(active_branch.clone(), active_head.clone());
        records.push(crate::core::resident_storage::PreparedResidentCommit {
            schema_version: crate::core::resident_storage::RESIDENT_COMMIT_SCHEMA.into(),
            session_id: session_id.into(),
            commit_id: commit_id.clone(),
            parent_commit_id: records
                .iter()
                .rev()
                .nth(0)
                .map(|record| record.commit_id.clone())
                .filter(|id| id != &commit_id),
            history_parent_commit_id: event.parent_id.clone(),
            base_sha256: base_sha256.clone(),
            request_id: format!("legacy_event_{commit_id}"),
            request_fingerprint: event.event_hash.clone().unwrap(),
            transition: crate::core::resident_storage::ResidentTransition::Mutation,
            effects: vec![
                json!({"prepared_transaction":prepared,"legacy_event_hash":event.event_hash}),
            ],
            resulting_head: active_head.clone(),
            resulting_branch: active_branch.clone(),
            resulting_branches: imported_branches.clone(),
            catalog_generation: 0,
            state_revision: revision_after,
        });
    }
    // Branch identity is independent of event ownership: aliases and base
    // branches need catalog entries even when they own no unique mutation.
    for (name, tip) in &branch_tips {
        if imported_branches.contains_key(name) {
            continue;
        }
        if active_head != *tip {
            append_control(
                crate::core::resident_storage::ResidentTransition::Checkout,
                vec![json!({"target_head":tip})],
                tip.clone(),
                active_branch.clone(),
                imported_branches.clone(),
                &mut records,
                &mut state_generation,
                &mut document_revision,
            );
            active_head = tip.clone();
        }
        imported_branches.insert(name.clone(), tip.clone());
        append_control(
            crate::core::resident_storage::ResidentTransition::BranchCreate,
            vec![json!({"branch_name":name})],
            active_head.clone(),
            active_branch.clone(),
            imported_branches.clone(),
            &mut records,
            &mut state_generation,
            &mut document_revision,
        );
    }
    // Restore the actual selected branch and possibly-undone HEAD.
    if active_branch != current_branch {
        active_branch = current_branch.clone();
        active_head = imported_branches
            .get(&active_branch)
            .cloned()
            .ok_or_else(|| anyhow!("selected legacy branch was not imported"))?;
        append_control(
            crate::core::resident_storage::ResidentTransition::BranchSwitch,
            vec![json!({"branch_name":active_branch})],
            active_head.clone(),
            active_branch.clone(),
            imported_branches.clone(),
            &mut records,
            &mut state_generation,
            &mut document_revision,
        );
    }
    if active_head != projected_head {
        let transition = if active_head
            .as_ref()
            .and_then(|id| by_id.get(id))
            .and_then(|event| event.parent_id.clone())
            == projected_head
        {
            crate::core::resident_storage::ResidentTransition::Undo
        } else {
            crate::core::resident_storage::ResidentTransition::Checkout
        };
        active_head = projected_head.clone();
        append_control(
            transition,
            vec![json!({"target_head":projected_head})],
            active_head.clone(),
            active_branch,
            imported_branches.clone(),
            &mut records,
            &mut state_generation,
            &mut document_revision,
        );
    }
    // Validate the generated authority before returning it.
    portable_history_state(&records, session_id, &base_sha256)?;
    let mut rejected_staged_ids = fs::read_dir(handle.staged_dir())?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .path()
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    rejected_staged_ids.sort();
    Ok(LegacyResidentImport {
        base_bytes,
        records,
        rejected_staged_ids,
    })
}

#[cfg(all(test, feature = "native-fs"))]
mod durable_calculation_failure_tests {
    use super::*;
    use crate::core::resident_storage::{ResidentCommitStorage, native::NativeResidentJournal};

    #[tokio::test]
    async fn failed_retained_evaluation_poison_requires_durable_recovery() {
        let mut book = umya_spreadsheet::new_file();
        book.get_sheet_by_name_mut("Sheet1")
            .unwrap()
            .get_cell_mut("A1")
            .set_formula("1+1");
        let mut base = Vec::new();
        umya_spreadsheet::writer::xlsx::write_writer(&book, &mut base).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let journal = NativeResidentJournal::open(dir.path()).unwrap();
        let mut session = ResidentWriteSession::from_bytes("session:test", &base).unwrap();
        recalculate_durable_on_resident(&mut session, &journal, "first", None)
            .await
            .unwrap();
        session.workbook.fail_next_evaluation_for_test();
        assert!(
            recalculate_durable_on_resident(&mut session, &journal, "failure", None)
                .await
                .is_err()
        );
        assert!(session.poison_reason().is_some());
        assert!(session.export_bytes().is_err());
        assert!(
            checkpoint_durable(&mut session, &journal, "cp", None)
                .await
                .is_err()
        );
        assert!(
            recalculate_durable_on_resident(&mut session, &journal, "first", None)
                .await
                .is_err()
        );
        assert_eq!(journal.load("test").await.unwrap().len(), 1);
        let mut recovered = recover_durable_resident_session("session:test", &base, &journal)
            .await
            .unwrap();
        recalculate_durable_on_resident(&mut recovered, &journal, "recovered", None)
            .await
            .unwrap();
        assert!(recovered.poison_reason().is_none());
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn execute_write(
    state: Arc<AppState>,
    request: WriteRequest,
) -> Result<WriteResponseData> {
    let fork_id = request.resource_id.to_workbook_id().0;
    let registry = state
        .fork_registry()
        .ok_or_else(|| anyhow!("fork registry not available"))?;
    let response = registry.with_fork_mut(&fork_id, |fork| {
        execute_write_transaction(&mut ForkFileBackend { fork }, request)
    })?;
    if matches!(
        response,
        WriteResponseData::Applied { .. } | WriteResponseData::Partial { .. }
    ) {
        let workbook_id = WorkbookId(fork_id);
        state.invalidate_calculation(&workbook_id);
        let _ = state.close_workbook(&workbook_id);
    }
    Ok(response)
}
