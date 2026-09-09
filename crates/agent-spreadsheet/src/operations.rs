use crate::read_context::ReadContext;
#[cfg(feature = "recalc")]
use crate::canonical_lifecycle::*;
#[cfg(feature = "recalc")]
use crate::session_history::{SessionHistoryRequest, SessionHistoryData};
#[cfg(feature = "recalc-formualizer")]
use crate::canonical_optional::{
    ExecuteSheetportData, ExecuteSheetportRequest, SheetportManifestData, SheetportManifestRequest,
};
use crate::canonical_optional::{InspectVbaData, InspectVbaRequest};
#[cfg(feature = "recalc")]
use crate::canonical_optional::{ScreenshotSheetData, ScreenshotSheetRequest};
pub use crate::canonical_reads::*;
#[cfg(feature = "recalc")]
use crate::canonical_write::{WriteRequest, WriteResponseData};
use crate::model::{
    FindValueResponse, InspectCellsResponse, NamedRangesResponse, ReadTableResponse,
    SheetListResponse, SheetOverviewResponse, SheetStatisticsResponse, WorkbookId,
};
use crate::state::AppState;
use crate::tools;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

pub const CANONICAL_SCHEMA_VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct ResourceId(
    #[schemars(
        pattern(r"^(wb|fork|session):[A-Za-z0-9][A-Za-z0-9_-]{0,243}$"),
        length(min = 4, max = 256)
    )]
    String,
);

impl ResourceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn bind_workbook(id: &WorkbookId) -> Result<Self, String> {
        let prefix = if id.0.starts_with("fork-") {
            "fork"
        } else if id.0.starts_with("session-") {
            "session"
        } else {
            "wb"
        };
        Self::validate(format!("{prefix}:{}", id.0))
    }

    fn validate(value: String) -> Result<Self, String> {
        if !(4..=256).contains(&value.len()) {
            return Err("resource_id must contain 4 to 256 bytes".to_string());
        }
        let (prefix, opaque) = value.split_once(':').ok_or_else(|| {
            "resource_id must use a typed wb:, fork:, or session: prefix".to_string()
        })?;
        if !matches!(prefix, "wb" | "fork" | "session") {
            return Err("resource_id must use a typed wb:, fork:, or session: prefix".to_string());
        }
        let valid_opaque = !opaque.is_empty()
            && opaque.len() <= 244
            && opaque.bytes().enumerate().all(|(index, byte)| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => true,
                b'_' | b'-' => index > 0,
                _ => false,
            });
        if !valid_opaque {
            return Err("resource_id must be an opaque typed identifier, not a path, drive, dot, or file form".to_string());
        }
        Ok(Self(value))
    }

    pub fn to_workbook_id(&self) -> WorkbookId {
        WorkbookId(
            self.0
                .split_once(':')
                .expect("validated typed resource")
                .1
                .to_string(),
        )
    }
}

impl<'de> Deserialize<'de> for ResourceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::validate(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationRisk {
    Low,
    Moderate,
    High,
    Destructive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationCostClass {
    Cheap,
    Bounded,
    Expensive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct OperationCost {
    pub class: OperationCostClass,
    pub bounded_by: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CapabilityMetadata {
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdapterBindingKind {
    None,
    SingleRead,
    SingleMutable,
    TwoResource,
    DurableOrchestration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdapterPersistence {
    None,
    ExportRequired,
    DurableRequired,
    /// Requires a retained owner; durability is reported by that owner's journal.
    ResidentRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdapterSupportStatus {
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct AdapterOperationMetadata {
    pub binding_kind: AdapterBindingKind,
    pub persistence: AdapterPersistence,
    pub support_status: AdapterSupportStatus,
}

impl AdapterOperationMetadata {
    pub const fn supported(
        binding_kind: AdapterBindingKind,
        persistence: AdapterPersistence,
    ) -> Self {
        Self {
            binding_kind,
            persistence,
            support_status: AdapterSupportStatus::Supported,
        }
    }

    pub const fn unsupported(
        binding_kind: AdapterBindingKind,
        persistence: AdapterPersistence,
    ) -> Self {
        Self {
            binding_kind,
            persistence,
            support_status: AdapterSupportStatus::Unsupported,
        }
    }

    pub fn is_supported(self) -> bool {
        self.support_status == AdapterSupportStatus::Supported
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct DescriptorAdapterMetadata {
    pub cli: AdapterOperationMetadata,
    pub mcp: AdapterOperationMetadata,
    pub wasm: AdapterOperationMetadata,
    pub just_bash: AdapterOperationMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuntimeCapabilities {
    pub workbook_discovery: bool,
    pub workbook_read: bool,
    pub workbook_write: bool,
    pub resident_history: bool,
    pub screenshot_rendering: bool,
    pub sheetport: bool,
    pub vba: bool,
}

impl RuntimeCapabilities {
    pub fn native() -> Self {
        Self {
            resident_history: false,
            workbook_discovery: true,
            workbook_read: true,
            workbook_write: cfg!(all(
                not(target_arch = "wasm32"),
                feature = "native-fs",
                feature = "recalc"
            )),
            screenshot_rendering: native_screenshot_available(),
            sheetport: cfg!(all(feature = "recalc-formualizer", feature = "sheetport")),
            vba: true,
        }
    }

    pub fn from_state(state: &AppState) -> Self {
        #[cfg(not(feature = "recalc"))]
        let _ = state;
        Self {
            resident_history: false,
            workbook_discovery: true,
            workbook_read: true,
            #[cfg(feature = "recalc")]
            workbook_write: state.fork_registry().is_some(),
            #[cfg(not(feature = "recalc"))]
            workbook_write: false,
            // The native raster backend is always available when it is
            // compiled in; the LibreOffice fallback still depends on the
            // host state that serializes its macro export.
            #[cfg(feature = "recalc")]
            screenshot_rendering: cfg!(feature = "render")
                || state.screenshot_semaphore().is_some(),
            #[cfg(not(feature = "recalc"))]
            screenshot_rendering: false,
            sheetport: cfg!(all(feature = "recalc-formualizer", feature = "sheetport")),
            vba: true,
        }
    }
}

pub struct OperationDescriptor {
    pub name: &'static str,
    pub schema_version: &'static str,
    pub description: &'static str,
    pub capability: CapabilityMetadata,
    pub capability_predicate: fn(&RuntimeCapabilities) -> bool,
    pub adapters: DescriptorAdapterMetadata,
    pub cost: OperationCost,
    pub risk_ceiling: OperationRisk,
    pub risk_for: fn(&SpreadsheetOperation) -> OperationRisk,
    pub input_schema: fn() -> Value,
    pub output_schema: fn() -> Value,
}

impl OperationDescriptor {
    pub fn is_available(&self, capabilities: &RuntimeCapabilities) -> bool {
        (self.capability_predicate)(capabilities)
    }
    pub fn discovery_json(&self, capabilities: &RuntimeCapabilities) -> Value {
        json!({"name":self.name,"schema_version":self.schema_version,"description":self.description,"capability":self.capability,"available":self.is_available(capabilities),"adapters":self.adapters,"cost":self.cost,"risk_ceiling":self.risk_ceiling})
    }

    pub fn registry_json(&self) -> Value {
        json!({"name":self.name,"schema_version":self.schema_version,"description":self.description,"capability":self.capability,"adapters":self.adapters,"cost":self.cost,"risk_ceiling":self.risk_ceiling,"input_schema":(self.input_schema)(),"output_schema":(self.output_schema)()})
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListSheetsRequest {
    pub resource_id: ResourceId,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub include_bounds: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SheetOverviewRequest {
    pub resource_id: ResourceId,
    pub sheet_name: String,
    #[serde(default)]
    pub max_regions: Option<u32>,
    #[serde(default)]
    pub max_headers: Option<u32>,
    #[serde(default)]
    pub include_headers: Option<bool>,
}

#[derive(Debug)]
pub enum SpreadsheetOperation {
    ListWorkbooks(ListWorkbooksRequest),
    DescribeWorkbook(DescribeWorkbookRequest),
    ListSheets(ListSheetsRequest),
    SheetOverview(SheetOverviewRequest),
    ReadCells(ReadCellsRequest),
    InspectCells(InspectCellsRequest),
    ReadTable(ReadTableRequest),
    ReadLayout(ReadLayoutRequest),
    ExportGrid(ExportGridRequest),
    NamedRanges(NamedRangesRequest),
    AnalyzeStyles(AnalyzeStylesRequest),
    SearchValues(SearchValuesRequest),
    SearchFormulas(SearchFormulasRequest),
    FormulaTrace(FormulaTraceRequest),
    FormulaMap(FormulaMapRequest),
    ProfileTable(ProfileTableRequest),
    SheetStatistics(SheetStatisticsRequest),
    #[cfg(feature = "recalc")]
    ScreenshotSheet(ScreenshotSheetRequest),
    #[cfg(feature = "recalc-formualizer")]
    SheetportManifest(SheetportManifestRequest),
    #[cfg(feature = "recalc-formualizer")]
    ExecuteSheetport(ExecuteSheetportRequest),
    InspectVba(InspectVbaRequest),
    #[cfg(feature = "recalc")]
    Write(WriteRequest),
    #[cfg(feature = "recalc")]
    CreateFork(CreateForkRequest),
    #[cfg(feature = "recalc")]
    ListForks(ListForksRequest),
    #[cfg(feature = "recalc")]
    Recalculate(RecalculateRequest),
    #[cfg(feature = "recalc")]
    VerifyWorkbook(VerifyWorkbookRequest),
    #[cfg(feature = "recalc")]
    ExportFork(ExportForkRequest),
    #[cfg(feature = "recalc")]
    DiscardFork(DiscardForkRequest),
    #[cfg(feature = "recalc")]
    GetChanges(GetChangesRequest),
    #[cfg(feature = "recalc")]
    Checkpoint(CheckpointRequest),
    #[cfg(feature = "recalc")]
    StagedChange(StagedChangeRequest),
    #[cfg(feature = "recalc")]
    SessionHistory(SessionHistoryRequest),
}

impl SpreadsheetOperation {
    pub fn is_owner_diagnostic(&self) -> bool {
        #[cfg(feature = "recalc")]
        if let Self::SessionHistory(request) = self { return request.is_diagnostic(); }
        false
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::ListWorkbooks(_) => "list_workbooks",
            Self::DescribeWorkbook(_) => "describe_workbook",
            Self::ListSheets(_) => "list_sheets",
            Self::SheetOverview(_) => "sheet_overview",
            Self::ReadCells(_) => "read_cells",
            Self::InspectCells(_) => "inspect_cells",
            Self::ReadTable(_) => "read_table",
            Self::ReadLayout(_) => "read_layout",
            Self::ExportGrid(_) => "export_grid",
            Self::NamedRanges(_) => "named_ranges",
            Self::AnalyzeStyles(_) => "analyze_styles",
            Self::SearchValues(_) => "search_values",
            Self::SearchFormulas(_) => "search_formulas",
            Self::FormulaTrace(_) => "formula_trace",
            Self::FormulaMap(_) => "formula_map",
            Self::ProfileTable(_) => "profile_table",
            Self::SheetStatistics(_) => "sheet_statistics",
            #[cfg(feature = "recalc")]
            Self::ScreenshotSheet(_) => "screenshot_sheet",
            #[cfg(feature = "recalc-formualizer")]
            Self::SheetportManifest(_) => "sheetport_manifest",
            #[cfg(feature = "recalc-formualizer")]
            Self::ExecuteSheetport(_) => "execute_sheetport",
            Self::InspectVba(_) => "inspect_vba",
            #[cfg(feature = "recalc")]
            Self::Write(_) => "write",
            #[cfg(feature = "recalc")]
            Self::CreateFork(_) => "create_fork",
            #[cfg(feature = "recalc")]
            Self::ListForks(_) => "list_forks",
            #[cfg(feature = "recalc")]
            Self::Recalculate(_) => "recalculate",
            #[cfg(feature = "recalc")]
            Self::VerifyWorkbook(_) => "verify_workbook",
            #[cfg(feature = "recalc")]
            Self::ExportFork(_) => "export_fork",
            #[cfg(feature = "recalc")]
            Self::DiscardFork(_) => "discard_fork",
            #[cfg(feature = "recalc")]
            Self::GetChanges(_) => "get_changes",
            #[cfg(feature = "recalc")]
            Self::Checkpoint(_) => "checkpoint",
            #[cfg(feature = "recalc")]
            Self::StagedChange(_) => "staged_change",
            #[cfg(feature = "recalc")]
            Self::SessionHistory(_) => "session_history",
        }
    }

    pub fn resource_id(&self) -> Option<&ResourceId> {
        match self {
            Self::ListWorkbooks(_) => None,
            Self::DescribeWorkbook(value) => Some(&value.resource_id),
            Self::ListSheets(value) => Some(&value.resource_id),
            Self::SheetOverview(value) => Some(&value.resource_id),
            Self::ReadCells(value) => Some(&value.resource_id),
            Self::InspectCells(value) => Some(&value.resource_id),
            Self::ReadTable(value) => Some(&value.resource_id),
            Self::ReadLayout(value) => Some(&value.resource_id),
            Self::ExportGrid(value) => Some(&value.resource_id),
            Self::NamedRanges(value) => Some(&value.resource_id),
            Self::AnalyzeStyles(value) => Some(&value.resource_id),
            Self::SearchValues(value) => Some(&value.resource_id),
            Self::SearchFormulas(value) => Some(&value.resource_id),
            Self::FormulaTrace(value) => Some(&value.resource_id),
            Self::FormulaMap(value) => Some(&value.resource_id),
            Self::ProfileTable(value) => Some(&value.resource_id),
            Self::SheetStatistics(value) => Some(&value.resource_id),
            #[cfg(feature = "recalc")]
            Self::ScreenshotSheet(value) => Some(&value.resource_id),
            #[cfg(feature = "recalc-formualizer")]
            Self::SheetportManifest(value) => value.resource_id(),
            #[cfg(feature = "recalc-formualizer")]
            Self::ExecuteSheetport(value) => Some(&value.resource_id),
            Self::InspectVba(value) => Some(value.resource_id()),
            #[cfg(feature = "recalc")]
            Self::Write(value) => Some(&value.resource_id),
            #[cfg(feature = "recalc")]
            Self::CreateFork(value) => Some(&value.resource_id),
            #[cfg(feature = "recalc")]
            Self::ListForks(_) => None,
            #[cfg(feature = "recalc")]
            Self::Recalculate(value) => Some(&value.resource_id),
            #[cfg(feature = "recalc")]
            Self::VerifyWorkbook(value) => Some(&value.resource_id),
            #[cfg(feature = "recalc")]
            Self::ExportFork(value) => Some(&value.resource_id),
            #[cfg(feature = "recalc")]
            Self::DiscardFork(value) => Some(&value.resource_id),
            #[cfg(feature = "recalc")]
            Self::GetChanges(value) => Some(&value.resource_id),
            #[cfg(feature = "recalc")]
            Self::Checkpoint(value) => Some(value.resource_id()),
            #[cfg(feature = "recalc")]
            Self::StagedChange(value) => Some(value.resource_id()),
            #[cfg(feature = "recalc")]
            Self::SessionHistory(value) => Some(value.resource_id()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanonicalResponse {
    pub schema_version: String,
    pub operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<ResourceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<String>,
    pub data: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CanonicalErrorCode {
    UnknownOperation,
    InvalidRequest,
    CapabilityUnavailable,
    ResourceNotFound,
    OperationFailed,
    StaleCursor,
    CursorMismatch,
    RowExceedsBudget,
    RevisionConflict,
    OutcomeUnknown,
    RecoveryRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanonicalError {
    pub code: CanonicalErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanonicalErrorEnvelope {
    pub schema_version: String,
    pub error: CanonicalError,
}

impl CanonicalErrorEnvelope {
    pub fn new(
        code: CanonicalErrorCode,
        message: impl Into<String>,
        operation: Option<&str>,
        path: Option<String>,
    ) -> Self {
        Self {
            schema_version: CANONICAL_SCHEMA_VERSION.to_string(),
            error: CanonicalError {
                code,
                message: message.into(),
                operation: operation.map(str::to_string),
                path,
            },
        }
    }
    fn invalid_request(operation: &str, error: serde_json::Error) -> Self {
        Self::new(
            CanonicalErrorCode::InvalidRequest,
            error.to_string(),
            Some(operation),
            Some(format!(
                "$ (line {}, column {})",
                error.line(),
                error.column()
            )),
        )
    }
    pub(crate) fn operation_failed(operation: &str, message: String) -> Self {
        Self::new(
            resident_failure_code(&message).unwrap_or(CanonicalErrorCode::OperationFailed),
            message,
            Some(operation),
            None,
        )
    }
}

impl std::fmt::Display for CanonicalErrorEnvelope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_string(self) {
            Ok(json) => formatter.write_str(&json),
            Err(_) => formatter.write_str(&self.error.message),
        }
    }
}
impl std::error::Error for CanonicalErrorEnvelope {}

#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
struct ResourceResponseSchema<T: JsonSchema> {
    schema_version: String,
    operation: String,
    resource_id: ResourceId,
    revision_id: String,
    data: T,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
struct DiscoveryResponseSchema<T: JsonSchema> {
    schema_version: String,
    operation: String,
    data: T,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
struct OptionalResourceResponseSchema<T: JsonSchema> {
    schema_version: String,
    operation: String,
    resource_id: Option<ResourceId>,
    revision_id: Option<String>,
    data: T,
}

/// Whether this build can render a screenshot at all.
///
/// The native raster backend needs nothing but the `render` feature: no
/// process, no profile directory, no probe. LibreOffice is the fallback and
/// stays behind its own availability probe, so a build without `render` behaves
/// exactly as it did before.
#[cfg(feature = "render")]
fn native_screenshot_available() -> bool {
    true
}
#[cfg(all(
    not(feature = "render"),
    not(target_arch = "wasm32"),
    feature = "recalc-libreoffice"
))]
fn native_screenshot_available() -> bool {
    crate::recalc::ScreenshotExecutor::new(&crate::recalc::RecalcConfig::default()).is_available()
}
#[cfg(all(
    not(feature = "render"),
    not(all(not(target_arch = "wasm32"), feature = "recalc-libreoffice"))
))]
fn native_screenshot_available() -> bool {
    false
}

fn workbook_read(capabilities: &RuntimeCapabilities) -> bool {
    capabilities.workbook_read
}
fn workbook_discovery(capabilities: &RuntimeCapabilities) -> bool {
    capabilities.workbook_discovery
}
#[cfg(feature = "recalc")]
fn screenshot_rendering(capabilities: &RuntimeCapabilities) -> bool {
    capabilities.workbook_read && capabilities.screenshot_rendering
}
#[cfg(feature = "recalc-formualizer")]
fn sheetport(capabilities: &RuntimeCapabilities) -> bool {
    capabilities.sheetport
}
fn vba(capabilities: &RuntimeCapabilities) -> bool {
    capabilities.workbook_read && capabilities.vba
}
#[cfg(feature = "recalc")]
fn workbook_write(capabilities: &RuntimeCapabilities) -> bool {
    capabilities.workbook_write
}
fn read_risk(_: &SpreadsheetOperation) -> OperationRisk {
    OperationRisk::Low
}
#[cfg(feature = "recalc")]
fn write_risk(operation: &SpreadsheetOperation) -> OperationRisk {
    match operation {
        SpreadsheetOperation::Write(request) => request
            .ops
            .iter()
            .map(crate::canonical_write::WriteOp::risk)
            .max_by_key(|risk| match risk {
                OperationRisk::Low => 0,
                OperationRisk::Moderate => 1,
                OperationRisk::High => 2,
                OperationRisk::Destructive => 3,
            })
            .unwrap_or(OperationRisk::Moderate),
        SpreadsheetOperation::Checkpoint(request) => checkpoint_risk(request),
        SpreadsheetOperation::StagedChange(request) => staged_change_risk(request),
        SpreadsheetOperation::SessionHistory(request) => request.risk(),
        SpreadsheetOperation::CreateFork(_) => OperationRisk::Moderate,
        SpreadsheetOperation::Recalculate(_) => OperationRisk::High,
        SpreadsheetOperation::ExportFork(_) => OperationRisk::High,
        SpreadsheetOperation::DiscardFork(_) => OperationRisk::Destructive,
        _ => OperationRisk::Low,
    }
}

fn closed_schema<T: JsonSchema>() -> Value {
    let mut schema = serde_json::to_value(schema_for!(T)).expect("schema serializes");
    let definitions = schema
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    inline_composed_object_refs(&mut schema, &definitions);
    close_object_schemas(&mut schema);
    schema
}
// Inline compatible local object refs so closing the branch sees both the tagged
// sibling fields and the referenced fields as one evaluated property set.
fn inline_composed_object_refs(value: &mut Value, definitions: &serde_json::Map<String, Value>) {
    match value {
        Value::Object(object) => {
            let referenced = object
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| reference.strip_prefix("#/$defs/"))
                .and_then(|name| definitions.get(name))
                .and_then(Value::as_object)
                .filter(|_| object.contains_key("properties"))
                .cloned();
            if let Some(mut referenced) = referenced {
                let compatible_properties = referenced
                    .get("properties")
                    .and_then(Value::as_object)
                    .is_none_or(|referenced_properties| {
                        object
                            .get("properties")
                            .and_then(Value::as_object)
                            .is_none_or(|local_properties| {
                                referenced_properties.iter().all(|(name, schema)| {
                                    local_properties
                                        .get(name)
                                        .is_none_or(|local_schema| local_schema == schema)
                                })
                            })
                    });
                let compatible = compatible_properties
                    && referenced.iter().all(|(key, referenced_value)| {
                        matches!(key.as_str(), "properties" | "required")
                            || object
                                .get(key)
                                .is_none_or(|local_value| local_value == referenced_value)
                    });
                if compatible {
                    object.remove("$ref");
                    if let Some(referenced_properties) = referenced
                        .remove("properties")
                        .and_then(|properties| properties.as_object().cloned())
                    {
                        let local_properties = object
                            .entry("properties")
                            .or_insert_with(|| Value::Object(serde_json::Map::new()))
                            .as_object_mut()
                            .expect("composed object properties");
                        for (name, schema) in referenced_properties {
                            local_properties.entry(name).or_insert(schema);
                        }
                    }
                    if let Some(referenced_required) = referenced
                        .remove("required")
                        .and_then(|required| required.as_array().cloned())
                    {
                        let local_required = object
                            .entry("required")
                            .or_insert_with(|| Value::Array(Vec::new()))
                            .as_array_mut()
                            .expect("composed object required properties");
                        for required in referenced_required {
                            if !local_required.contains(&required) {
                                local_required.push(required);
                            }
                        }
                    }
                    for (key, referenced_value) in referenced {
                        object.entry(key).or_insert(referenced_value);
                    }
                }
            }
            for child in object.values_mut() {
                inline_composed_object_refs(child, definitions);
            }
        }
        Value::Array(values) => {
            for child in values {
                inline_composed_object_refs(child, definitions);
            }
        }
        _ => {}
    }
}
fn close_object_schemas(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("object")
                || object.contains_key("properties")
            {
                let closure_keyword = if object.contains_key("$ref") || object.contains_key("allOf")
                {
                    "unevaluatedProperties"
                } else {
                    "additionalProperties"
                };
                object.entry(closure_keyword).or_insert(Value::Bool(false));
            }
            for child in object.values_mut() {
                close_object_schemas(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                close_object_schemas(child);
            }
        }
        _ => {}
    }
}
fn set_property_const(schema: &mut Value, property: &str, value: &str) {
    let object = schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .and_then(|properties| properties.get_mut(property))
        .and_then(Value::as_object_mut)
        .expect("envelope property");
    object.clear();
    object.insert("type".to_string(), Value::String("string".to_string()));
    object.insert("const".to_string(), Value::String(value.to_string()));
}
fn resource_output_schema<T: JsonSchema>(operation: &str) -> Value {
    let mut schema = closed_schema::<ResourceResponseSchema<T>>();
    set_property_const(&mut schema, "schema_version", CANONICAL_SCHEMA_VERSION);
    set_property_const(&mut schema, "operation", operation);
    schema
}
fn discovery_output_schema<T: JsonSchema>(operation: &str) -> Value {
    let mut schema = closed_schema::<DiscoveryResponseSchema<T>>();
    set_property_const(&mut schema, "schema_version", CANONICAL_SCHEMA_VERSION);
    set_property_const(&mut schema, "operation", operation);
    schema
}
#[cfg(feature = "recalc-formualizer")]
fn optional_resource_output_schema<T: JsonSchema>(operation: &str) -> Value {
    let mut schema = closed_schema::<OptionalResourceResponseSchema<T>>();
    set_property_const(&mut schema, "schema_version", CANONICAL_SCHEMA_VERSION);
    set_property_const(&mut schema, "operation", operation);
    schema
}

macro_rules! schemas {
    ($input_fn:ident, $output_fn:ident, $request:ty, $response:ty, $name:literal) => {
        fn $input_fn() -> Value {
            closed_schema::<$request>()
        }
        fn $output_fn() -> Value {
            resource_output_schema::<$response>($name)
        }
    };
}
fn list_workbooks_input_schema() -> Value {
    closed_schema::<ListWorkbooksRequest>()
}
fn list_workbooks_output_schema() -> Value {
    discovery_output_schema::<ListWorkbooksData>("list_workbooks")
}
schemas!(
    describe_workbook_input_schema,
    describe_workbook_output_schema,
    DescribeWorkbookRequest,
    DescribeWorkbookData,
    "describe_workbook"
);
schemas!(
    list_sheets_input_schema,
    list_sheets_output_schema,
    ListSheetsRequest,
    SheetListResponse,
    "list_sheets"
);
schemas!(
    sheet_overview_input_schema,
    sheet_overview_output_schema,
    SheetOverviewRequest,
    SheetOverviewResponse,
    "sheet_overview"
);
schemas!(
    read_cells_input_schema,
    read_cells_output_schema,
    ReadCellsRequest,
    ReadCellsData,
    "read_cells"
);
schemas!(
    inspect_cells_input_schema,
    inspect_cells_output_schema,
    InspectCellsRequest,
    InspectCellsResponse,
    "inspect_cells"
);
schemas!(
    read_table_input_schema,
    read_table_output_schema,
    ReadTableRequest,
    ReadTableResponse,
    "read_table"
);
schemas!(
    read_layout_input_schema,
    read_layout_output_schema,
    ReadLayoutRequest,
    ReadLayoutData,
    "read_layout"
);
schemas!(
    export_grid_input_schema,
    export_grid_output_schema,
    ExportGridRequest,
    ExportGridData,
    "export_grid"
);
schemas!(
    named_ranges_input_schema,
    named_ranges_output_schema,
    NamedRangesRequest,
    NamedRangesResponse,
    "named_ranges"
);
schemas!(
    analyze_styles_input_schema,
    analyze_styles_output_schema,
    AnalyzeStylesRequest,
    AnalyzeStylesData,
    "analyze_styles"
);
schemas!(
    search_values_input_schema,
    search_values_output_schema,
    SearchValuesRequest,
    FindValueResponse,
    "search_values"
);
schemas!(
    search_formulas_input_schema,
    search_formulas_output_schema,
    SearchFormulasRequest,
    SearchFormulasData,
    "search_formulas"
);
schemas!(
    formula_trace_input_schema,
    formula_trace_output_schema,
    FormulaTraceRequest,
    FormulaTraceData,
    "formula_trace"
);
schemas!(
    formula_map_input_schema,
    formula_map_output_schema,
    FormulaMapRequest,
    FormulaMapData,
    "formula_map"
);
schemas!(
    profile_table_input_schema,
    profile_table_output_schema,
    ProfileTableRequest,
    ProfileTableData,
    "profile_table"
);
schemas!(
    sheet_statistics_input_schema,
    sheet_statistics_output_schema,
    SheetStatisticsRequest,
    SheetStatisticsResponse,
    "sheet_statistics"
);
#[cfg(feature = "recalc")]
fn screenshot_sheet_input_schema() -> Value {
    let mut schema = closed_schema::<ScreenshotSheetRequest>();
    schema["properties"]["range"]["x-runtime-bounds"] = json!({
        "max_rows": 100,
        "max_columns": 30
    });
    schema
}
#[cfg(feature = "recalc")]
fn screenshot_sheet_output_schema() -> Value {
    resource_output_schema::<ScreenshotSheetData>("screenshot_sheet")
}
#[cfg(feature = "recalc-formualizer")]
fn sheetport_manifest_input_schema() -> Value {
    closed_schema::<SheetportManifestRequest>()
}
#[cfg(feature = "recalc-formualizer")]
fn sheetport_manifest_output_schema() -> Value {
    optional_resource_output_schema::<SheetportManifestData>("sheetport_manifest")
}
#[cfg(feature = "recalc-formualizer")]
fn apply_sheetport_value_bounds(value: &mut Value, property: Option<&str>) {
    if let Some(object) = value.as_object_mut() {
        if matches!(property, Some("inputs" | "results")) {
            object.insert("maxProperties".to_string(), json!(256));
        }
        if object.get("type") == Some(&Value::String("object".to_string()))
            && object
                .get("additionalProperties")
                .and_then(Value::as_object)
                .and_then(|schema| schema.get("$ref"))
                == Some(&json!("#/$defs/SheetportScalar"))
        {
            object.insert("maxProperties".to_string(), json!(1_000));
        }
        if object.get("type") == Some(&Value::String("array".to_string()))
            && object
                .get("items")
                .and_then(Value::as_object)
                .and_then(|schema| schema.get("$ref"))
                == Some(&json!("#/$defs/SheetportScalar"))
        {
            object.insert("maxItems".to_string(), json!(100_000));
        }
        let keys = object.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            if let Some(child) = object.get_mut(&key) {
                apply_sheetport_value_bounds(child, Some(&key));
            }
        }
    } else if let Some(array) = value.as_array_mut() {
        for child in array {
            apply_sheetport_value_bounds(child, property);
        }
    }
}
#[cfg(feature = "recalc-formualizer")]
fn execute_sheetport_input_schema() -> Value {
    let mut schema = closed_schema::<ExecuteSheetportRequest>();
    apply_sheetport_value_bounds(&mut schema, None);
    schema["properties"]["inputs"]["x-runtime-bounds"] = json!({
        "max_ports": 256,
        "max_total_cells": 100_000,
        "max_rows_per_value": 10_000,
        "max_fields_per_row_or_record": 1_000,
        "max_text_bytes": 65_536
    });
    schema
}
#[cfg(feature = "recalc-formualizer")]
fn execute_sheetport_output_schema() -> Value {
    let mut schema = resource_output_schema::<ExecuteSheetportData>("execute_sheetport");
    apply_sheetport_value_bounds(&mut schema, None);
    schema["$defs"]["ExecuteSheetportData"]["properties"]["results"]["x-runtime-bounds"] = json!({
        "max_ports": 256,
        "max_total_cells": 100_000,
        "max_rows_per_value": 10_000,
        "max_fields_per_row_or_record": 1_000,
        "max_text_bytes": 65_536
    });
    schema
}
schemas!(
    inspect_vba_input_schema,
    inspect_vba_output_schema,
    InspectVbaRequest,
    InspectVbaData,
    "inspect_vba"
);
#[cfg(feature = "recalc")]
fn write_input_schema() -> Value {
    fn apply_write_bounds(value: &mut Value, property: Option<&str>) {
        if let Some(object) = value.as_object_mut() {
            if matches!(property, Some("ops")) {
                object.insert("maxItems".to_string(), json!(128));
            }
            if matches!(property, Some("cells")) {
                if object.get("type") == Some(&Value::String("object".to_string())) {
                    object.insert("maxProperties".to_string(), json!(100_000));
                } else {
                    object.insert("maxItems".to_string(), json!(100_000));
                }
            }
            if matches!(
                property,
                Some("rows" | "merges" | "columns" | "row_breaks" | "col_breaks")
            ) {
                object.insert("maxItems".to_string(), json!(100_000));
            }
            if matches!(property, Some("items"))
                && object.get("type") == Some(&Value::String("array".to_string()))
            {
                object.insert("maxItems".to_string(), json!(100_000));
            }
            if matches!(property, Some("csv")) {
                object.insert("maxLength".to_string(), json!(1_048_576));
            }
            let keys = object.keys().cloned().collect::<Vec<_>>();
            for key in keys {
                if let Some(child) = object.get_mut(&key) {
                    apply_write_bounds(child, Some(&key));
                }
            }
        } else if let Some(array) = value.as_array_mut() {
            for child in array {
                apply_write_bounds(child, property);
            }
        }
    }

    let mut schema = closed_schema::<WriteRequest>();
    let defs = schema
        .get_mut("$defs")
        .and_then(Value::as_object_mut)
        .expect("write schema definitions");
    let family_names = [
        "CanonicalStructureOp",
        "StyleWriteOp",
        "ColumnWriteOp",
        "FormulaWriteOp",
        "NameWriteOp",
        "ImportAndHelperOp",
        "TransformOp",
        "SheetLayoutOp",
        "RulesOp",
    ];
    let mut variants = vec![json!({"$ref":"#/$defs/SetCellsOp"})];
    for family in family_names {
        let family_variants = defs
            .get(family)
            .and_then(|value| value.get("oneOf"))
            .and_then(Value::as_array)
            .expect("tagged write family")
            .clone();
        variants.extend(family_variants);
    }
    defs.insert("WriteOp".to_string(), json!({"oneOf": variants}));
    for family in family_names {
        defs.remove(family);
    }
    apply_write_bounds(&mut schema, None);
    schema
}

#[cfg(feature = "recalc")]
schemas!(
    create_fork_input_schema,
    create_fork_output_schema,
    CreateForkRequest,
    CreateForkData,
    "create_fork"
);
#[cfg(feature = "recalc")]
fn list_forks_input_schema() -> Value {
    closed_schema::<ListForksRequest>()
}
#[cfg(feature = "recalc")]
fn list_forks_output_schema() -> Value {
    discovery_output_schema::<ListForksData>("list_forks")
}
#[cfg(feature = "recalc")]
schemas!(
    recalculate_input_schema,
    recalculate_output_schema,
    RecalculateRequest,
    RecalculateData,
    "recalculate"
);
#[cfg(feature = "recalc")]
schemas!(
    verify_workbook_input_schema,
    verify_workbook_output_schema,
    VerifyWorkbookRequest,
    VerifyWorkbookData,
    "verify_workbook"
);
#[cfg(feature = "recalc")]
schemas!(
    export_fork_input_schema,
    export_fork_output_schema,
    ExportForkRequest,
    ExportForkData,
    "export_fork"
);
#[cfg(feature = "recalc")]
schemas!(
    discard_fork_input_schema,
    discard_fork_output_schema,
    DiscardForkRequest,
    DiscardForkData,
    "discard_fork"
);
#[cfg(feature = "recalc")]
schemas!(
    get_changes_input_schema,
    get_changes_output_schema,
    GetChangesRequest,
    GetChangesData,
    "get_changes"
);
#[cfg(feature = "recalc")]
schemas!(
    checkpoint_input_schema,
    checkpoint_output_schema,
    CheckpointRequest,
    CheckpointData,
    "checkpoint"
);
#[cfg(feature = "recalc")]
schemas!(
    staged_change_input_schema,
    staged_change_output_schema,
    StagedChangeRequest,
    StagedChangeData,
    "staged_change"
);
#[cfg(feature = "recalc")]
schemas!(session_history_input_schema, session_history_output_schema, SessionHistoryRequest, SessionHistoryData, "session_history");

#[cfg(feature = "recalc")]
fn write_output_schema() -> Value {
    let mut schema = resource_output_schema::<WriteResponseData>("write");
    if let Some(variants) = schema
        .pointer_mut("/$defs/WriteResponseData/oneOf")
        .and_then(Value::as_array_mut)
    {
        for variant in variants {
            let Some(properties) = variant.get_mut("properties").and_then(Value::as_object_mut)
            else {
                continue;
            };
            let status = properties
                .get("status")
                .and_then(|value| value.get("const"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let mode = match status.as_deref() {
                Some("previewed") => Some("preview"),
                Some("staged") => Some("stage"),
                Some("applied" | "partial" | "rolled_back") => Some("apply"),
                _ => None,
            };
            if let Some(mode) = mode {
                properties.insert("mode".to_string(), json!({"type":"string","const":mode}));
            }
            if let Some(atomic) = match status.as_deref() {
                Some("staged" | "rolled_back") => Some(true),
                Some("partial") => Some(false),
                _ => None,
            } {
                properties.insert(
                    "atomic".to_string(),
                    json!({"type":"boolean","const":atomic}),
                );
            }
        }
    }
    schema
}

const WORKBOOK_DISCOVERY: CapabilityMetadata = CapabilityMetadata {
    name: "workbook_discovery",
    description: "Discover workbook resources available to this runtime",
};
const WORKBOOK_READ: CapabilityMetadata = CapabilityMetadata {
    name: "workbook_read",
    description: "Read an already-bound workbook resource",
};
#[cfg(feature = "recalc")]
const SCREENSHOT_RENDERING: CapabilityMetadata = CapabilityMetadata {
    name: "screenshot_rendering",
    description: "Render bounded workbook regions to content-addressed image artifacts",
};
#[cfg(feature = "recalc-formualizer")]
const SHEETPORT: CapabilityMetadata = CapabilityMetadata {
    name: "sheetport",
    description: "Validate, bind, and execute portable typed SheetPort manifests",
};
const VBA: CapabilityMetadata = CapabilityMetadata {
    name: "vba",
    description: "Inspect bounded VBA project metadata and module source",
};
#[cfg(feature = "recalc")]
const RESIDENT_HISTORY: CapabilityMetadata = CapabilityMetadata { name: "resident_history", description: "Inspect and move durable resident history with exact request outcomes" };
#[cfg(feature = "recalc")]
const WORKBOOK_WRITE: CapabilityMetadata = CapabilityMetadata {
    name: "workbook_write",
    description: "Mutate an isolated fork or session resource with revision CAS",
};
const CHEAP_READ: OperationCost = OperationCost {
    class: OperationCostClass::Cheap,
    bounded_by: &["items"],
};
const BOUNDED_READ: OperationCost = OperationCost {
    class: OperationCostClass::Bounded,
    bounded_by: &["items", "cells", "payload_bytes"],
};
const EXPENSIVE_READ: OperationCost = OperationCost {
    class: OperationCostClass::Expensive,
    bounded_by: &["sheets", "cells", "items", "payload_bytes"],
};

const NONE: AdapterPersistence = AdapterPersistence::None;
#[cfg(feature = "recalc")]
const EXPORT_REQUIRED: AdapterPersistence = AdapterPersistence::ExportRequired;
const DURABLE_REQUIRED: AdapterPersistence = AdapterPersistence::DurableRequired;
const CLI_NONE: AdapterOperationMetadata =
    AdapterOperationMetadata::supported(AdapterBindingKind::None, NONE);
const CLI_READ: AdapterOperationMetadata =
    AdapterOperationMetadata::supported(AdapterBindingKind::SingleRead, NONE);
#[cfg(feature = "recalc")]
const CLI_MUTABLE: AdapterOperationMetadata =
    AdapterOperationMetadata::supported(AdapterBindingKind::SingleMutable, EXPORT_REQUIRED);
#[cfg(feature = "recalc")]
const CLI_TWO_RESOURCE: AdapterOperationMetadata =
    AdapterOperationMetadata::supported(AdapterBindingKind::TwoResource, NONE);
#[cfg(feature = "recalc")]
const CLI_DURABLE_UNSUPPORTED: AdapterOperationMetadata = AdapterOperationMetadata::unsupported(
    AdapterBindingKind::DurableOrchestration,
    DURABLE_REQUIRED,
);
const MCP_NONE: AdapterOperationMetadata =
    AdapterOperationMetadata::supported(AdapterBindingKind::None, NONE);
const MCP_READ: AdapterOperationMetadata =
    AdapterOperationMetadata::supported(AdapterBindingKind::SingleRead, DURABLE_REQUIRED);
#[cfg(feature = "recalc")]
const MCP_MUTABLE: AdapterOperationMetadata =
    AdapterOperationMetadata::supported(AdapterBindingKind::SingleMutable, DURABLE_REQUIRED);
#[cfg(feature = "recalc")]
const MCP_TWO_RESOURCE: AdapterOperationMetadata =
    AdapterOperationMetadata::supported(AdapterBindingKind::TwoResource, DURABLE_REQUIRED);
#[cfg(feature = "recalc")]
const MCP_DURABLE: AdapterOperationMetadata =
    AdapterOperationMetadata::supported(AdapterBindingKind::DurableOrchestration, DURABLE_REQUIRED);
const WASM_NONE_UNSUPPORTED: AdapterOperationMetadata =
    AdapterOperationMetadata::unsupported(AdapterBindingKind::None, NONE);
const WASM_READ: AdapterOperationMetadata =
    AdapterOperationMetadata::supported(AdapterBindingKind::SingleRead, NONE);
const WASM_READ_UNSUPPORTED: AdapterOperationMetadata =
    AdapterOperationMetadata::unsupported(AdapterBindingKind::SingleRead, NONE);
#[cfg(feature = "recalc")]
const WASM_MUTABLE: AdapterOperationMetadata =
    AdapterOperationMetadata::supported(AdapterBindingKind::SingleMutable, NONE);
#[cfg(feature = "recalc")]
const WASM_TWO_RESOURCE: AdapterOperationMetadata =
    AdapterOperationMetadata::supported(AdapterBindingKind::TwoResource, NONE);
#[cfg(feature = "recalc")]
const WASM_DURABLE_UNSUPPORTED: AdapterOperationMetadata = AdapterOperationMetadata::unsupported(
    AdapterBindingKind::DurableOrchestration,
    DURABLE_REQUIRED,
);
const fn just_bash_adapter(
    wasm: AdapterOperationMetadata,
    persistence: AdapterPersistence,
) -> AdapterOperationMetadata {
    match wasm.support_status {
        AdapterSupportStatus::Supported => {
            AdapterOperationMetadata::supported(wasm.binding_kind, persistence)
        }
        AdapterSupportStatus::Unsupported => {
            AdapterOperationMetadata::unsupported(wasm.binding_kind, persistence)
        }
    }
}
const JUST_BASH_NONE_UNSUPPORTED: AdapterOperationMetadata =
    just_bash_adapter(WASM_NONE_UNSUPPORTED, NONE);
const JUST_BASH_READ: AdapterOperationMetadata = just_bash_adapter(WASM_READ, NONE);
const JUST_BASH_READ_UNSUPPORTED: AdapterOperationMetadata =
    just_bash_adapter(WASM_READ_UNSUPPORTED, NONE);
#[cfg(feature = "recalc")]
const JUST_BASH_MUTABLE: AdapterOperationMetadata =
    just_bash_adapter(WASM_MUTABLE, EXPORT_REQUIRED);
#[cfg(feature = "recalc")]
const JUST_BASH_TWO_RESOURCE: AdapterOperationMetadata = just_bash_adapter(WASM_TWO_RESOURCE, NONE);
#[cfg(feature = "recalc")]
const JUST_BASH_DURABLE_UNSUPPORTED: AdapterOperationMetadata =
    just_bash_adapter(WASM_DURABLE_UNSUPPORTED, DURABLE_REQUIRED);
const SHARED_READ_ADAPTERS: DescriptorAdapterMetadata = DescriptorAdapterMetadata {
    cli: CLI_READ,
    mcp: MCP_READ,
    wasm: WASM_READ,
    just_bash: JUST_BASH_READ,
};
const DISCOVERY_ADAPTERS: DescriptorAdapterMetadata = DescriptorAdapterMetadata {
    cli: CLI_NONE,
    mcp: MCP_NONE,
    wasm: WASM_NONE_UNSUPPORTED,
    just_bash: JUST_BASH_NONE_UNSUPPORTED,
};
const NATIVE_READ_ADAPTERS: DescriptorAdapterMetadata = DescriptorAdapterMetadata {
    cli: CLI_READ,
    mcp: MCP_READ,
    wasm: WASM_READ_UNSUPPORTED,
    just_bash: JUST_BASH_READ_UNSUPPORTED,
};
#[cfg(feature = "recalc")]
const MUTABLE_ADAPTERS: DescriptorAdapterMetadata = DescriptorAdapterMetadata {
    cli: CLI_MUTABLE,
    mcp: MCP_MUTABLE,
    wasm: WASM_MUTABLE,
    just_bash: JUST_BASH_MUTABLE,
};
#[cfg(feature = "recalc")]
const TWO_RESOURCE_ADAPTERS: DescriptorAdapterMetadata = DescriptorAdapterMetadata {
    cli: CLI_TWO_RESOURCE,
    mcp: MCP_TWO_RESOURCE,
    wasm: WASM_TWO_RESOURCE,
    just_bash: JUST_BASH_TWO_RESOURCE,
};
#[cfg(feature = "recalc")]
const DURABLE_ADAPTERS: DescriptorAdapterMetadata = DescriptorAdapterMetadata {
    cli: CLI_DURABLE_UNSUPPORTED,
    mcp: MCP_DURABLE,
    wasm: WASM_DURABLE_UNSUPPORTED,
    just_bash: JUST_BASH_DURABLE_UNSUPPORTED,
};

#[cfg(feature = "recalc")]
const RESIDENT_ADAPTERS: DescriptorAdapterMetadata = DescriptorAdapterMetadata {
    cli: CLI_DURABLE_UNSUPPORTED,
    mcp: MCP_DURABLE,
    wasm: AdapterOperationMetadata::supported(AdapterBindingKind::SingleMutable, AdapterPersistence::ResidentRequired),
    just_bash: AdapterOperationMetadata::supported(AdapterBindingKind::SingleMutable, AdapterPersistence::ResidentRequired),
};

// Generated projection only: native tests compare every entry with the Rust
// schema builders. WASM avoids linking runtime schema-generation machinery.
#[cfg(any(target_arch = "wasm32", test))]
fn frozen_schema(name: &str) -> Value {
    static SCHEMAS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    SCHEMAS.get_or_init(|| serde_json::from_str(include_str!("../schema/canonical-schemas.json"))
        .expect("generated canonical schemas must be valid JSON"))
        .get(name).expect("generated canonical schema is missing").clone()
}
macro_rules! schema_function {
    ($function:ident) => {{
        #[cfg(target_arch = "wasm32")]
        { || frozen_schema(stringify!($function)) }
        #[cfg(not(target_arch = "wasm32"))]
        { $function }
    }};
}

macro_rules! descriptor {
    ($name:literal, $description:literal, $capability:expr, $predicate:expr, $cost:expr, $input:ident, $output:ident) => {
        descriptor_adapters!(
            $name,
            $description,
            $capability,
            $predicate,
            SHARED_READ_ADAPTERS,
            $cost,
            $input,
            $output
        )
    };
}

macro_rules! descriptor_adapters {
    ($name:literal, $description:literal, $capability:expr, $predicate:expr, $adapters:expr, $cost:expr, $input:ident, $output:ident) => {
        OperationDescriptor {
            name: $name,
            schema_version: CANONICAL_SCHEMA_VERSION,
            description: $description,
            capability: $capability,
            capability_predicate: $predicate,
            adapters: $adapters,
            cost: $cost,
            risk_ceiling: OperationRisk::Low,
            risk_for: read_risk,
            input_schema: schema_function!($input),
            output_schema: schema_function!($output),
        }
    };
}

static REGISTRY: &[OperationDescriptor] = &[
    descriptor_adapters!(
        "list_workbooks",
        "Discover workbook resources available to this runtime.",
        WORKBOOK_DISCOVERY,
        workbook_discovery,
        DISCOVERY_ADAPTERS,
        CHEAP_READ,
        list_workbooks_input_schema,
        list_workbooks_output_schema
    ),
    descriptor!(
        "describe_workbook",
        "Return cheap exact workbook metadata, with an opt-in derived summary.",
        WORKBOOK_READ,
        workbook_read,
        BOUNDED_READ,
        describe_workbook_input_schema,
        describe_workbook_output_schema
    ),
    descriptor!(
        "list_sheets",
        "List sheets in a bound workbook resource with optional bounds.",
        WORKBOOK_READ,
        workbook_read,
        CHEAP_READ,
        list_sheets_input_schema,
        list_sheets_output_schema
    ),
    descriptor!(
        "sheet_overview",
        "Detect regions, headers, bounds, and notable structure for one sheet.",
        WORKBOOK_READ,
        workbook_read,
        BOUNDED_READ,
        sheet_overview_input_schema,
        sheet_overview_output_schema
    ),
    descriptor!(
        "read_cells",
        "Read correlated exact ranges or projected row windows with revision-bound continuation.",
        WORKBOOK_READ,
        workbook_read,
        BOUNDED_READ,
        read_cells_input_schema,
        read_cells_output_schema
    ),
    descriptor!(
        "inspect_cells",
        "Inspect bounded sparse cells with values, formulas, formats, styles, and calculation state.",
        WORKBOOK_READ,
        workbook_read,
        BOUNDED_READ,
        inspect_cells_input_schema,
        inspect_cells_output_schema
    ),
    descriptor!(
        "read_table",
        "Read a header-aware table or detected region with filtering and paging.",
        WORKBOOK_READ,
        workbook_read,
        BOUNDED_READ,
        read_table_input_schema,
        read_table_output_schema
    ),
    descriptor!(
        "read_layout",
        "Read a deliberately lossy bounded display/layout projection.",
        WORKBOOK_READ,
        workbook_read,
        BOUNDED_READ,
        read_layout_input_schema,
        read_layout_output_schema
    ),
    descriptor!(
        "export_grid",
        "Export cell content and explicit formatting with coordinates, merges, formats, and styles; implicit presentation defaults are excluded.",
        WORKBOOK_READ,
        workbook_read,
        BOUNDED_READ,
        export_grid_input_schema,
        export_grid_output_schema
    ),
    descriptor!(
        "named_ranges",
        "List workbook- and sheet-scoped named items.",
        WORKBOOK_READ,
        workbook_read,
        CHEAP_READ,
        named_ranges_input_schema,
        named_ranges_output_schema
    ),
    descriptor!(
        "analyze_styles",
        "Analyze style patterns at explicit workbook or sheet scope.",
        WORKBOOK_READ,
        workbook_read,
        EXPENSIVE_READ,
        analyze_styles_input_schema,
        analyze_styles_output_schema
    ),
    descriptor!(
        "search_values",
        "Search values while preserving label, direction, region, table, type, header, and context modes.",
        WORKBOOK_READ,
        workbook_read,
        EXPENSIVE_READ,
        search_values_input_schema,
        search_values_output_schema
    ),
    descriptor!(
        "search_formulas",
        "Search formula cells or grouped classifications, including actual volatile function names.",
        WORKBOOK_READ,
        workbook_read,
        EXPENSIVE_READ,
        search_formulas_input_schema,
        search_formulas_output_schema
    ),
    descriptor!(
        "formula_trace",
        "Trace bounded precedents or dependents from one formula target.",
        WORKBOOK_READ,
        workbook_read,
        EXPENSIVE_READ,
        formula_trace_input_schema,
        formula_trace_output_schema
    ),
    descriptor!(
        "formula_map",
        "Map sheet formula topology and repeated formula groups.",
        WORKBOOK_READ,
        workbook_read,
        EXPENSIVE_READ,
        formula_map_input_schema,
        formula_map_output_schema
    ),
    descriptor!(
        "profile_table",
        "Profile tabular columns, types, distributions, samples, and data quality.",
        WORKBOOK_READ,
        workbook_read,
        EXPENSIVE_READ,
        profile_table_input_schema,
        profile_table_output_schema
    ),
    descriptor!(
        "sheet_statistics",
        "Compute bounded sheet-level statistics.",
        WORKBOOK_READ,
        workbook_read,
        EXPENSIVE_READ,
        sheet_statistics_input_schema,
        sheet_statistics_output_schema
    ),
    #[cfg(feature = "recalc")]
    descriptor_adapters!(
        "screenshot_sheet",
        "Render a bounded sheet range to a content-addressed PNG artifact without exposing a server path.",
        SCREENSHOT_RENDERING,
        screenshot_rendering,
        // The raster renderer compiles to wasm32 and needs no host process, so
        // the byte/session adapters render too. The artifact bytes never enter
        // the envelope: each adapter hands them across its own boundary.
        SHARED_READ_ADAPTERS,
        EXPENSIVE_READ,
        screenshot_sheet_input_schema,
        screenshot_sheet_output_schema
    ),
    #[cfg(feature = "recalc-formualizer")]
    descriptor_adapters!(
        "sheetport_manifest",
        "Discover, inspect, validate, normalize, or bind-check portable SheetPort manifest content.",
        SHEETPORT,
        sheetport,
        NATIVE_READ_ADAPTERS,
        BOUNDED_READ,
        sheetport_manifest_input_schema,
        sheetport_manifest_output_schema
    ),
    #[cfg(feature = "recalc-formualizer")]
    descriptor_adapters!(
        "execute_sheetport",
        "Execute a portable SheetPort manifest with closed typed inputs, results, errors, and coverage.",
        SHEETPORT,
        sheetport,
        NATIVE_READ_ADAPTERS,
        EXPENSIVE_READ,
        execute_sheetport_input_schema,
        execute_sheetport_output_schema
    ),
    descriptor_adapters!(
        "inspect_vba",
        "Inspect a VBA project summary or bounded module source with revision-bound opaque paging.",
        VBA,
        vba,
        NATIVE_READ_ADAPTERS,
        BOUNDED_READ,
        inspect_vba_input_schema,
        inspect_vba_output_schema
    ),
    #[cfg(feature = "recalc")]
    OperationDescriptor {
        name: "write",
        schema_version: CANONICAL_SCHEMA_VERSION,
        description: "Preview, stage, or apply canonical mutations with revision CAS and atomic rollback; stage retention follows the host journal's lifetime.",
        capability: WORKBOOK_WRITE,
        capability_predicate: workbook_write,
        adapters: MUTABLE_ADAPTERS,
        cost: OperationCost {
            class: OperationCostClass::Expensive,
            bounded_by: &["ops", "cells", "payload_bytes"],
        },
        risk_ceiling: OperationRisk::Destructive,
        risk_for: write_risk,
        input_schema: schema_function!(write_input_schema),
        output_schema: schema_function!(write_output_schema),
    },
    #[cfg(feature = "recalc")]
    OperationDescriptor {
        name: "create_fork",
        schema_version: CANONICAL_SCHEMA_VERSION,
        description: "Create an isolated mutable fork from a revision-bound resource.",
        capability: WORKBOOK_WRITE,
        capability_predicate: workbook_write,
        adapters: DURABLE_ADAPTERS,
        cost: EXPENSIVE_READ,
        risk_ceiling: OperationRisk::Moderate,
        risk_for: write_risk,
        input_schema: schema_function!(create_fork_input_schema),
        output_schema: schema_function!(create_fork_output_schema),
    },
    #[cfg(feature = "recalc")]
    descriptor_adapters!(
        "list_forks",
        "Discover active forks without exposing server-local paths.",
        WORKBOOK_WRITE,
        workbook_write,
        DURABLE_ADAPTERS,
        CHEAP_READ,
        list_forks_input_schema,
        list_forks_output_schema
    ),
    #[cfg(feature = "recalc")]
    OperationDescriptor {
        name: "recalculate",
        schema_version: CANONICAL_SCHEMA_VERSION,
        description: "Evaluate a mutable fork or in-memory session with revision CAS and complete F1 coverage metadata.",
        capability: WORKBOOK_WRITE,
        capability_predicate: workbook_write,
        adapters: MUTABLE_ADAPTERS,
        cost: OperationCost {
            class: OperationCostClass::Expensive,
            bounded_by: &["formula_cells", "timeout_ms"],
        },
        risk_ceiling: OperationRisk::High,
        risk_for: write_risk,
        input_schema: schema_function!(recalculate_input_schema),
        output_schema: schema_function!(recalculate_output_schema),
    },
    #[cfg(feature = "recalc")]
    descriptor_adapters!(
        "verify_workbook",
        "Evaluate and compare baseline and current resources with sound proof states and coverage.",
        WORKBOOK_WRITE,
        workbook_write,
        TWO_RESOURCE_ADAPTERS,
        EXPENSIVE_READ,
        verify_workbook_input_schema,
        verify_workbook_output_schema
    ),
    #[cfg(feature = "recalc")]
    OperationDescriptor {
        name: "export_fork",
        schema_version: CANONICAL_SCHEMA_VERSION,
        description: "Export a revision-bound fork to a portable artifact destination.",
        capability: WORKBOOK_WRITE,
        capability_predicate: workbook_write,
        adapters: DURABLE_ADAPTERS,
        cost: OperationCost {
            class: OperationCostClass::Expensive,
            bounded_by: &["workbook_bytes"],
        },
        risk_ceiling: OperationRisk::High,
        risk_for: write_risk,
        input_schema: schema_function!(export_fork_input_schema),
        output_schema: schema_function!(export_fork_output_schema),
    },
    #[cfg(feature = "recalc")]
    OperationDescriptor {
        name: "discard_fork",
        schema_version: CANONICAL_SCHEMA_VERSION,
        description: "Destroy an isolated fork with revision CAS.",
        capability: WORKBOOK_WRITE,
        capability_predicate: workbook_write,
        adapters: DURABLE_ADAPTERS,
        cost: CHEAP_READ,
        risk_ceiling: OperationRisk::Destructive,
        risk_for: write_risk,
        input_schema: schema_function!(discard_fork_input_schema),
        output_schema: schema_function!(discard_fork_output_schema),
    },
    #[cfg(feature = "recalc")]
    descriptor_adapters!(
        "get_changes",
        "Read either the canonical operation audit or a direct base-to-current net diff.",
        WORKBOOK_WRITE,
        workbook_write,
        DURABLE_ADAPTERS,
        EXPENSIVE_READ,
        get_changes_input_schema,
        get_changes_output_schema
    ),
    #[cfg(feature = "recalc")]
    OperationDescriptor {
        name: "checkpoint",
        schema_version: CANONICAL_SCHEMA_VERSION,
        description: "Create, list, atomically restore, or delete resident checkpoints.",
        capability: WORKBOOK_WRITE,
        capability_predicate: workbook_write,
        adapters: RESIDENT_ADAPTERS,
        cost: OperationCost {
            class: OperationCostClass::Expensive,
            bounded_by: &["workbook_bytes", "checkpoints"],
        },
        risk_ceiling: OperationRisk::Destructive,
        risk_for: write_risk,
        input_schema: schema_function!(checkpoint_input_schema),
        output_schema: schema_function!(checkpoint_output_schema),
    },
    #[cfg(feature = "recalc")]
    OperationDescriptor {
        name: "staged_change",
        schema_version: CANONICAL_SCHEMA_VERSION,
        description: "List, atomically apply, or discard canonical staged write bundles.",
        capability: WORKBOOK_WRITE,
        capability_predicate: workbook_write,
        adapters: RESIDENT_ADAPTERS,
        cost: OperationCost {
            class: OperationCostClass::Expensive,
            bounded_by: &["ops", "cells", "payload_bytes"],
        },
        risk_ceiling: OperationRisk::Destructive,
        risk_for: write_risk,
        input_schema: schema_function!(staged_change_input_schema),
        output_schema: schema_function!(staged_change_output_schema),
    },
    #[cfg(feature = "recalc")]
    OperationDescriptor {
        name: "session_history", schema_version: CANONICAL_SCHEMA_VERSION,
        description: "Inspect owner health, reconcile request outcomes, or navigate resident history; durability follows the host journal.",
        capability: RESIDENT_HISTORY, capability_predicate: |capabilities| capabilities.resident_history,
        adapters: RESIDENT_ADAPTERS, cost: OperationCost { class: OperationCostClass::Expensive, bounded_by: &["records", "payload_bytes"] },
        risk_ceiling: OperationRisk::Destructive, risk_for: write_risk,
        input_schema: schema_function!(session_history_input_schema), output_schema: schema_function!(session_history_output_schema),
    },
];

#[cfg(all(test, not(target_arch = "wasm32")))]
mod frozen_schema_tests {
    use super::*;
    #[test]
    fn generated_wasm_schemas_match_native_builders() {
        for descriptor in REGISTRY {
            assert_eq!(frozen_schema(&format!("{}_input_schema", descriptor.name)),
                (descriptor.input_schema)(), "input schema drift: {}", descriptor.name);
            assert_eq!(frozen_schema(&format!("{}_output_schema", descriptor.name)),
                (descriptor.output_schema)(), "output schema drift: {}", descriptor.name);
        }
        let schemas: Value = serde_json::from_str(include_str!("../schema/canonical-schemas.json")).unwrap();
        assert_eq!(schemas.as_object().unwrap().len(), CANONICAL_OPERATION_NAMES.len() * 2);
    }
}

pub const CANONICAL_OPERATION_NAMES: &[&str] = &[
    "list_workbooks",
    "describe_workbook",
    "list_sheets",
    "sheet_overview",
    "read_cells",
    "inspect_cells",
    "read_table",
    "read_layout",
    "export_grid",
    "named_ranges",
    "analyze_styles",
    "search_values",
    "search_formulas",
    "formula_trace",
    "formula_map",
    "profile_table",
    "sheet_statistics",
    "screenshot_sheet",
    "sheetport_manifest",
    "execute_sheetport",
    "inspect_vba",
    "write",
    "create_fork",
    "list_forks",
    "recalculate",
    "verify_workbook",
    "export_fork",
    "discard_fork",
    "get_changes",
    "checkpoint",
    "staged_change",
    "session_history",
];

pub fn is_canonical_operation_name(name: &str) -> bool {
    CANONICAL_OPERATION_NAMES.contains(&name)
}

pub fn operation_registry() -> &'static [OperationDescriptor] {
    REGISTRY
}
pub fn operation_descriptor(name: &str) -> Option<&'static OperationDescriptor> {
    REGISTRY.iter().find(|descriptor| descriptor.name == name)
}
pub fn canonical_error_schema() -> Value {
    let mut schema = closed_schema::<CanonicalErrorEnvelope>();
    set_property_const(&mut schema, "schema_version", CANONICAL_SCHEMA_VERSION);
    schema
}
pub fn operation_schema(name: &str) -> Result<Value, CanonicalErrorEnvelope> {
    let descriptor = operation_descriptor(name).ok_or_else(|| {
        CanonicalErrorEnvelope::new(
            CanonicalErrorCode::UnknownOperation,
            format!("unknown operation '{name}'"),
            Some(name),
            Some("$.operation".to_string()),
        )
    })?;
    Ok(
        json!({"schema_version":descriptor.schema_version,"operation":descriptor.name,"input_schema":(descriptor.input_schema)(),"output_schema":(descriptor.output_schema)(),"error_schema":canonical_error_schema()}),
    )
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationAdapter {
    Cli,
    Mcp,
    Wasm,
    JustBash,
}

impl OperationDescriptor {
    pub fn adapter_metadata(&self, adapter: OperationAdapter) -> AdapterOperationMetadata {
        match adapter {
            OperationAdapter::Cli => self.adapters.cli,
            OperationAdapter::Mcp => self.adapters.mcp,
            OperationAdapter::Wasm => self.adapters.wasm,
            OperationAdapter::JustBash => self.adapters.just_bash,
        }
    }

    pub fn is_available_for(
        &self,
        adapter: OperationAdapter,
        capabilities: &RuntimeCapabilities,
    ) -> bool {
        self.adapter_metadata(adapter).is_supported() && self.is_available(capabilities)
    }
}

pub fn operations_discovery(capabilities: &RuntimeCapabilities) -> Value {
    operations_discovery_for(OperationAdapter::Mcp, capabilities)
}

pub fn operations_discovery_for(
    adapter: OperationAdapter,
    capabilities: &RuntimeCapabilities,
) -> Value {
    Value::Array(
        REGISTRY
            .iter()
            .filter(|descriptor| descriptor.is_available_for(adapter, capabilities))
            .map(|descriptor| descriptor.discovery_json(capabilities))
            .collect(),
    )
}

pub fn registry_projection() -> Value {
    json!({
        "schema_version": CANONICAL_SCHEMA_VERSION,
        "operations": REGISTRY.iter().map(OperationDescriptor::registry_json).collect::<Vec<_>>(),
        "error_schema": canonical_error_schema(),
    })
}

macro_rules! decode {
    ($payload:expr, $name:expr, $request:ty, $variant:path) => {
        serde_json::from_value::<$request>($payload)
            .map($variant)
            .map_err(|error| CanonicalErrorEnvelope::invalid_request($name, error))
    };
}
pub fn decode_operation(
    name: &str,
    payload: Value,
) -> Result<SpreadsheetOperation, CanonicalErrorEnvelope> {
    match name {
        "list_workbooks" => decode!(
            payload,
            name,
            ListWorkbooksRequest,
            SpreadsheetOperation::ListWorkbooks
        ),
        "describe_workbook" => decode!(
            payload,
            name,
            DescribeWorkbookRequest,
            SpreadsheetOperation::DescribeWorkbook
        ),
        "list_sheets" => decode!(
            payload,
            name,
            ListSheetsRequest,
            SpreadsheetOperation::ListSheets
        ),
        "sheet_overview" => decode!(
            payload,
            name,
            SheetOverviewRequest,
            SpreadsheetOperation::SheetOverview
        ),
        "read_cells" => decode!(
            payload,
            name,
            ReadCellsRequest,
            SpreadsheetOperation::ReadCells
        ),
        "inspect_cells" => decode!(
            payload,
            name,
            InspectCellsRequest,
            SpreadsheetOperation::InspectCells
        ),
        "read_table" => decode!(
            payload,
            name,
            ReadTableRequest,
            SpreadsheetOperation::ReadTable
        ),
        "read_layout" => decode!(
            payload,
            name,
            ReadLayoutRequest,
            SpreadsheetOperation::ReadLayout
        ),
        "export_grid" => decode!(
            payload,
            name,
            ExportGridRequest,
            SpreadsheetOperation::ExportGrid
        ),
        "named_ranges" => decode!(
            payload,
            name,
            NamedRangesRequest,
            SpreadsheetOperation::NamedRanges
        ),
        "analyze_styles" => decode!(
            payload,
            name,
            AnalyzeStylesRequest,
            SpreadsheetOperation::AnalyzeStyles
        ),
        "search_values" => decode!(
            payload,
            name,
            SearchValuesRequest,
            SpreadsheetOperation::SearchValues
        ),
        "search_formulas" => decode!(
            payload,
            name,
            SearchFormulasRequest,
            SpreadsheetOperation::SearchFormulas
        ),
        "formula_trace" => decode!(
            payload,
            name,
            FormulaTraceRequest,
            SpreadsheetOperation::FormulaTrace
        ),
        "formula_map" => decode!(
            payload,
            name,
            FormulaMapRequest,
            SpreadsheetOperation::FormulaMap
        ),
        "profile_table" => decode!(
            payload,
            name,
            ProfileTableRequest,
            SpreadsheetOperation::ProfileTable
        ),
        "sheet_statistics" => decode!(
            payload,
            name,
            SheetStatisticsRequest,
            SpreadsheetOperation::SheetStatistics
        ),
        #[cfg(feature = "recalc")]
        "screenshot_sheet" => {
            let request = serde_json::from_value::<ScreenshotSheetRequest>(payload)
                .map_err(|error| CanonicalErrorEnvelope::invalid_request(name, error))?;
            crate::canonical_optional::validate_screenshot_request(&request).map_err(|error| {
                CanonicalErrorEnvelope::new(
                    CanonicalErrorCode::InvalidRequest,
                    error.to_string(),
                    Some(name),
                    None,
                )
            })?;
            Ok(SpreadsheetOperation::ScreenshotSheet(request))
        }
        #[cfg(feature = "recalc-formualizer")]
        "sheetport_manifest" => decode!(
            payload,
            name,
            SheetportManifestRequest,
            SpreadsheetOperation::SheetportManifest
        ),
        #[cfg(feature = "recalc-formualizer")]
        "execute_sheetport" => decode!(
            payload,
            name,
            ExecuteSheetportRequest,
            SpreadsheetOperation::ExecuteSheetport
        ),
        "inspect_vba" => decode!(
            payload,
            name,
            InspectVbaRequest,
            SpreadsheetOperation::InspectVba
        ),
        #[cfg(feature = "recalc")]
        "create_fork" => decode!(
            payload,
            name,
            CreateForkRequest,
            SpreadsheetOperation::CreateFork
        ),
        #[cfg(feature = "recalc")]
        "list_forks" => decode!(
            payload,
            name,
            ListForksRequest,
            SpreadsheetOperation::ListForks
        ),
        #[cfg(feature = "recalc")]
        "recalculate" => decode!(
            payload,
            name,
            RecalculateRequest,
            SpreadsheetOperation::Recalculate
        ),
        #[cfg(feature = "recalc")]
        "verify_workbook" => decode!(
            payload,
            name,
            VerifyWorkbookRequest,
            SpreadsheetOperation::VerifyWorkbook
        ),
        #[cfg(feature = "recalc")]
        "export_fork" => decode!(
            payload,
            name,
            ExportForkRequest,
            SpreadsheetOperation::ExportFork
        ),
        #[cfg(feature = "recalc")]
        "discard_fork" => decode!(
            payload,
            name,
            DiscardForkRequest,
            SpreadsheetOperation::DiscardFork
        ),
        #[cfg(feature = "recalc")]
        "get_changes" => decode!(
            payload,
            name,
            GetChangesRequest,
            SpreadsheetOperation::GetChanges
        ),
        #[cfg(feature = "recalc")]
        "checkpoint" => decode!(
            payload,
            name,
            CheckpointRequest,
            SpreadsheetOperation::Checkpoint
        ),
        #[cfg(feature = "recalc")]
        "session_history" => decode!(payload, name, SessionHistoryRequest, SpreadsheetOperation::SessionHistory),
        #[cfg(feature = "recalc")]
        "staged_change" => decode!(
            payload,
            name,
            StagedChangeRequest,
            SpreadsheetOperation::StagedChange
        ),
        #[cfg(feature = "recalc")]
        "write" => {
            static VALIDATOR: std::sync::OnceLock<Result<jsonschema::Validator, String>> = std::sync::OnceLock::new();
            let validator = VALIDATOR.get_or_init(|| {
                let schema = (operation_descriptor("write").expect("write descriptor").input_schema)();
                jsonschema::validator_for(&schema).map_err(|error| error.to_string())
            }).as_ref().map_err(|error| {
                CanonicalErrorEnvelope::new(
                    CanonicalErrorCode::OperationFailed,
                    format!("invalid generated write schema: {error}"),
                    Some(name),
                    None,
                )
            })?;
            if let Err(error) = validator.validate(&payload) {
                return Err(CanonicalErrorEnvelope::new(
                    CanonicalErrorCode::InvalidRequest,
                    error.to_string(),
                    Some(name),
                    Some(error.instance_path.to_string()),
                ));
            }
            decode!(payload, name, WriteRequest, SpreadsheetOperation::Write)
        }
        _ => Err(CanonicalErrorEnvelope::new(
            CanonicalErrorCode::UnknownOperation,
            format!("unknown operation '{name}'"),
            Some(name),
            Some("$.operation".to_string()),
        )),
    }
}

fn optional_error(operation: &str, error: anyhow::Error) -> CanonicalErrorEnvelope {
    let message = error.to_string();
    let code = if message.starts_with("invalid request:") {
        CanonicalErrorCode::InvalidRequest
    } else if message.starts_with("stale cursor:") {
        CanonicalErrorCode::StaleCursor
    } else if message.starts_with("cursor mismatch:") {
        CanonicalErrorCode::CursorMismatch
    } else if message.contains("not found") {
        CanonicalErrorCode::ResourceNotFound
    } else {
        CanonicalErrorCode::OperationFailed
    };
    CanonicalErrorEnvelope::new(code, message, Some(operation), None)
}

fn resident_failure_code(message: &str) -> Option<CanonicalErrorCode> {
    if message.contains("outcome unknown") || message.contains("outcome requires operator reconciliation") {
        Some(CanonicalErrorCode::OutcomeUnknown)
    } else if message.contains("requires recovery") || message.contains("recovery required") {
        Some(CanonicalErrorCode::RecoveryRequired)
    } else { None }
}

#[cfg(feature = "recalc")]
pub(crate) fn lifecycle_error(operation: &str, error: anyhow::Error) -> CanonicalErrorEnvelope {
    let message = error.to_string();
    let code = if let Some(code) = resident_failure_code(&message) {
        code
    } else if message.starts_with("request identity reuse") {
        CanonicalErrorCode::InvalidRequest
    } else if message.starts_with("revision conflict:") {
        CanonicalErrorCode::RevisionConflict
    } else if message.starts_with("invalid request:") {
        CanonicalErrorCode::InvalidRequest
    } else if message.contains("not found") {
        CanonicalErrorCode::ResourceNotFound
    } else {
        CanonicalErrorCode::OperationFailed
    };
    CanonicalErrorEnvelope::new(code, message, Some(operation), None)
}

pub async fn execute_operation_json(
    state: impl crate::execution_context::ExecutionContext,
    name: &str,
    payload: Value,
) -> Result<CanonicalResponse, CanonicalErrorEnvelope> {
    execute_operation(state, decode_operation(name, payload)?).await
}

pub async fn execute_operation(
    mut state: impl crate::execution_context::ExecutionContext,
    operation: SpreadsheetOperation,
) -> Result<CanonicalResponse, CanonicalErrorEnvelope> {
    let name = operation.name();
    let descriptor = operation_descriptor(name).expect("decoded operations are registered");
    if !descriptor.is_available(&state.capabilities()) {
        return Err(CanonicalErrorEnvelope::new(
            CanonicalErrorCode::CapabilityUnavailable,
            format!("operation '{name}' is unavailable in this runtime"),
            Some(name),
            None,
        ));
    }

    #[allow(unused_mut)]
    let (mut resource_id, mut revision_id) = if let Some(requested) = operation.resource_id() {
        let identified = if operation.is_owner_diagnostic() { state.identify_diagnostic(requested).await } else { state.identify(requested).await.map(|(resource, revision)| (resource, Some(revision))) };
        let (resource, revision) = identified.map_err(|error| CanonicalErrorEnvelope::new(
            resident_failure_code(&error.to_string()).unwrap_or(CanonicalErrorCode::ResourceNotFound),
            if name == "inspect_vba" { "VBA resource could not be opened".into() } else if name == "screenshot_sheet" { "screenshot resource could not be opened".into() } else { error.to_string() },
            Some(name), Some("$.resource_id".into()),
        ))?;
        (Some(resource), revision)
    } else { (None, None) };

    let workbook_id = resource_id.as_ref().map(ResourceId::to_workbook_id);
    let data = match operation {
        SpreadsheetOperation::ListWorkbooks(request) => serde_json::to_value(
            execute_list_workbooks(state.reads()?, request)
                .await
                .map_err(|error| {
                    CanonicalErrorEnvelope::operation_failed(name, error.to_string())
                })?,
        ),
        SpreadsheetOperation::DescribeWorkbook(request) => {
            serde_json::to_value(execute_describe(state.reads()?, request).await.map_err(|error| {
                CanonicalErrorEnvelope::operation_failed(name, error.to_string())
            })?)
        }
        SpreadsheetOperation::ListSheets(request) => serde_json::to_value(
            tools::list_sheets_semantic(
                state.reads()?,
                tools::ListSheetsParams {
                    workbook_or_fork_id: request.resource_id.to_workbook_id(),
                    limit: request.limit,
                    offset: request.offset,
                    include_bounds: request.include_bounds,
                },
            )
            .await
            .map_err(|error| CanonicalErrorEnvelope::operation_failed(name, error.to_string()))?,
        ),
        SpreadsheetOperation::SheetOverview(request) => serde_json::to_value(
            tools::sheet_overview_semantic(
                state.reads()?,
                tools::SheetOverviewParams {
                    workbook_or_fork_id: request.resource_id.to_workbook_id(),
                    sheet_name: request.sheet_name,
                    max_regions: request.max_regions,
                    max_headers: request.max_headers,
                    include_headers: request.include_headers,
                },
            )
            .await
            .map_err(|error| CanonicalErrorEnvelope::operation_failed(name, error.to_string()))?,
        ),
        SpreadsheetOperation::ReadCells(request) => serde_json::to_value(
            execute_read_cells(
                state.reads()?,
                &request,
                revision_id.as_deref().expect("read resource revision"),
            )
            .await?,
        ),
        SpreadsheetOperation::InspectCells(request) => serde_json::to_value(
            execute_inspect_cells(
                state.reads()?,
                request,
                revision_id.as_deref().expect("read resource revision"),
            )
            .await?,
        ),
        SpreadsheetOperation::ReadTable(request) => {
            let workbook_id = request.resource_id.to_workbook_id();
            let mut response = tools::read_table_semantic(
                state.reads()?,
                tools::ReadTableParams {
                    workbook_or_fork_id: workbook_id.clone(),
                    sheet_name: request.sheet_name,
                    table_name: request.table_name,
                    region_id: request.region_id,
                    range: request.range,
                    header_row: request.header_row,
                    header_rows: request.header_rows,
                    columns: request.columns,
                    filters: request
                        .filters
                        .map(|values| values.into_iter().map(tools::TableFilter::from).collect()),
                    sample_mode: request.sample_mode,
                    limit: request.limit,
                    offset: request.offset,
                    format: request.format,
                    include_headers: request.include_headers,
                    include_types: request.include_types,
                },
            )
            .await
            .map_err(|error| CanonicalErrorEnvelope::operation_failed(name, error.to_string()))?;
            response.calculation = state.reads()?.calculation_metadata(
                &workbook_id,
                revision_id.as_deref().expect("read resource revision"),
                response.calculation,
            );
            serde_json::to_value(response)
        }
        SpreadsheetOperation::ReadLayout(request) => {
            let response = tools::layout_page(
                state.reads()?,
                tools::LayoutPageParams {
                    workbook_or_fork_id: request.resource_id.to_workbook_id(),
                    sheet_name: request.sheet_name,
                    range: request.range,
                    mode: request.mode,
                    max_col_width: request.max_col_width,
                    fit_columns: request.fit_columns,
                    trim_empty_columns: request.trim_empty_columns,
                    render: request.render,
                },
            )
            .await
            .map_err(|error| CanonicalErrorEnvelope::operation_failed(name, error.to_string()))?;
            serde_json::to_value(ReadLayoutData {
                lossiness: LayoutLossiness::Lossy,
                layout: response,
            })
        }
        SpreadsheetOperation::ExportGrid(request) => serde_json::to_value(
            execute_export_grid(
                state.reads()?,
                request,
                revision_id.as_deref().expect("read resource revision"),
            )
            .await?,
        ),
        SpreadsheetOperation::NamedRanges(request) => serde_json::to_value(
            tools::named_ranges_semantic(
                state.reads()?,
                tools::NamedRangesParams {
                    workbook_or_fork_id: request.resource_id.to_workbook_id(),
                    sheet_name: request.sheet_name,
                    name_prefix: request.name_prefix,
                },
            )
            .await
            .map_err(|error| CanonicalErrorEnvelope::operation_failed(name, error.to_string()))?,
        ),
        SpreadsheetOperation::AnalyzeStyles(request) => serde_json::to_value(
            execute_analyze_styles(state.reads()?, request)
                .await
                .map_err(|error| {
                    CanonicalErrorEnvelope::operation_failed(name, error.to_string())
                })?,
        ),
        SpreadsheetOperation::SearchValues(request) => serde_json::to_value(
            tools::find_value_semantic(
                state.reads()?,
                tools::FindValueParams {
                    workbook_or_fork_id: request.resource_id.to_workbook_id(),
                    query: request.query,
                    label: request.label,
                    mode: request.mode,
                    match_mode: request.match_mode,
                    case_sensitive: request.case_sensitive,
                    sheet_name: request.sheet_name,
                    region_id: request.region_id,
                    table_name: request.table_name,
                    value_types: request.value_types,
                    search_headers_only: request.search_headers_only,
                    direction: request.direction,
                    limit: request.limit,
                    offset: request.offset,
                    context: request.context,
                    context_width: request.context_width,
                },
            )
            .await
            .map_err(|error| CanonicalErrorEnvelope::operation_failed(name, error.to_string()))?,
        ),
        SpreadsheetOperation::SearchFormulas(request) => serde_json::to_value(
            execute_search_formulas(
                state.reads()?,
                request,
                revision_id.as_deref().expect("read resource revision"),
            )
            .await?,
        ),
        SpreadsheetOperation::FormulaTrace(request) => serde_json::to_value(
            execute_formula_trace(
                state.reads()?,
                request,
                revision_id.as_deref().expect("read resource revision"),
            )
            .await?,
        ),
        SpreadsheetOperation::FormulaMap(request) => serde_json::to_value(
            execute_formula_map(
                state.reads()?,
                request,
                revision_id.as_deref().expect("read resource revision"),
            )
            .await?,
        ),
        SpreadsheetOperation::ProfileTable(request) => {
            serde_json::to_value(execute_profile_table(state.reads()?, request).await?)
        }
        SpreadsheetOperation::SheetStatistics(request) => serde_json::to_value(
            tools::sheet_statistics_semantic(
                state.reads()?,
                tools::SheetStatisticsParams {
                    workbook_or_fork_id: request.resource_id.to_workbook_id(),
                    sheet_name: request.sheet_name,
                    sample_rows: request.sample_rows,
                    summary_only: request.summary_only,
                },
            )
            .await
            .map_err(|error| CanonicalErrorEnvelope::operation_failed(name, error.to_string()))?,
        ),
        #[cfg(feature = "recalc")]
        SpreadsheetOperation::ScreenshotSheet(request) => serde_json::to_value(
            state.screenshot_sheet(request)
                .await
                .map_err(|error| optional_error(name, error))?,
        ),
        #[cfg(feature = "recalc-formualizer")]
        SpreadsheetOperation::SheetportManifest(request) => serde_json::to_value(
            state.sheetport_manifest(request)
                .await
                .map_err(|error| optional_error(name, error))?,
        ),
        #[cfg(feature = "recalc-formualizer")]
        SpreadsheetOperation::ExecuteSheetport(request) => serde_json::to_value(
            state.execute_sheetport(request)
                .await
                .map_err(|error| optional_error(name, error))?,
        ),
        SpreadsheetOperation::InspectVba(request) => serde_json::to_value(
            state.inspect_vba(
                request,
                revision_id.as_deref().expect("VBA resource revision"),
            )
            .await
            .map_err(|error| optional_error(name, error))?,
        ),
        #[cfg(feature = "recalc")]
        SpreadsheetOperation::Write(request) => {
            let result = state.write(request)
                .await
                .map_err(|error| lifecycle_error(name, error))?;
            revision_id = Some(result.revision_after().to_string());
            serde_json::to_value(result)
        }
        #[cfg(feature = "recalc")]
        SpreadsheetOperation::CreateFork(request) => {
            let result = state.create_fork(request)
                .await
                .map_err(|error| lifecycle_error(name, error))?;
            resource_id = Some(result.fork_resource_id.clone());
            revision_id = Some(result.revision_id.clone());
            serde_json::to_value(result)
        }
        #[cfg(feature = "recalc")]
        SpreadsheetOperation::ListForks(request) => serde_json::to_value(
            state.list_forks(request).await
                .map_err(|error| lifecycle_error(name, error))?,
        ),
        #[cfg(feature = "recalc")]
        SpreadsheetOperation::Recalculate(request) => {
            let result = state.recalculate(request)
                .await
                .map_err(|error| lifecycle_error(name, error))?;
            revision_id = Some(result.revision_after.clone());
            serde_json::to_value(result)
        }
        // `canonical_lifecycle::verify_workbook` snapshots both resources into a
        // host temp directory and re-binds them through the path workspace.
        #[cfg(feature = "recalc")]
        SpreadsheetOperation::VerifyWorkbook(request) => {
            let result = state.verify_workbook(request)
                .await
                .map_err(|error| lifecycle_error(name, error))?;
            revision_id = Some(result.current_revision_id.clone());
            serde_json::to_value(result)
        }
        #[cfg(feature = "recalc")]
        SpreadsheetOperation::ExportFork(request) => {
            let result = state.export_fork(request).await
                .map_err(|error| lifecycle_error(name, error))?;
            revision_id = Some(result.revision_after.clone());
            serde_json::to_value(result)
        }
        #[cfg(feature = "recalc")]
        SpreadsheetOperation::DiscardFork(request) => {
            let result = state.discard_fork(request).await
                .map_err(|error| lifecycle_error(name, error))?;
            revision_id = Some(result.revision_after.clone());
            serde_json::to_value(result)
        }
        #[cfg(feature = "recalc")]
        SpreadsheetOperation::GetChanges(request) => {
            let result = state.get_changes(request)
                .await
                .map_err(|error| lifecycle_error(name, error))?;
            revision_id = Some(match &result {
                GetChangesData::Operations { revision_id, .. }
                | GetChangesData::NetDiff { revision_id, .. } => revision_id.clone(),
            });
            serde_json::to_value(result)
        }
        #[cfg(feature = "recalc")]
        SpreadsheetOperation::Checkpoint(request) => {
            let result = state.checkpoint(request).await
                .map_err(|error| lifecycle_error(name, error))?;
            revision_id = Some(match &result {
                CheckpointData::Create { revision_after, .. }
                | CheckpointData::Restore { revision_after, .. }
                | CheckpointData::Delete { revision_after, .. } => revision_after.clone(),
                CheckpointData::List { revision_id, .. } => revision_id.clone(),
            });
            serde_json::to_value(result)
        }
        #[cfg(feature = "recalc")]
        SpreadsheetOperation::SessionHistory(request) => {
            let result = state.session_history(request).await.map_err(|error| lifecycle_error(name, error))?;
            match &result {
                SessionHistoryData::Mutation { revision_after, .. } => revision_id = Some(revision_after.clone()),
                SessionHistoryData::Status { revision_id: current, .. } => revision_id = current.clone(),
                _ => {},
            }
            serde_json::to_value(result)
        }
        #[cfg(feature = "recalc")]
        SpreadsheetOperation::StagedChange(request) => {
            let result = state.staged_change(request).await
                .map_err(|error| lifecycle_error(name, error))?;
            revision_id = Some(match &result {
                StagedChangeData::Apply { revision_after, .. }
                | StagedChangeData::Discard { revision_after, .. } => revision_after.clone(),
                StagedChangeData::List { revision_id, .. } => revision_id.clone(),
            });
            serde_json::to_value(result)
        }
    }
    .map_err(|error| CanonicalErrorEnvelope::operation_failed(name, error.to_string()))?;
    let _ = workbook_id;
    Ok(CanonicalResponse {
        schema_version: CANONICAL_SCHEMA_VERSION.to_string(),
        operation: name.to_string(),
        resource_id,
        revision_id,
        data,
    })
}

pub(crate) async fn project_legacy<T: serde::de::DeserializeOwned>(
    state: Arc<AppState>,
    operation: SpreadsheetOperation,
) -> anyhow::Result<T> {
    let response = execute_operation(state, operation)
        .await
        .map_err(|error| anyhow::anyhow!(error.error.message))?;
    serde_json::from_value(response.data).map_err(Into::into)
}

impl TryFrom<tools::ListSheetsParams> for ListSheetsRequest {
    type Error = String;
    fn try_from(params: tools::ListSheetsParams) -> Result<Self, Self::Error> {
        Ok(Self {
            resource_id: ResourceId::bind_workbook(&params.workbook_or_fork_id)?,
            limit: params.limit,
            offset: params.offset,
            include_bounds: params.include_bounds,
        })
    }
}
impl TryFrom<tools::SheetOverviewParams> for SheetOverviewRequest {
    type Error = String;
    fn try_from(params: tools::SheetOverviewParams) -> Result<Self, Self::Error> {
        Ok(Self {
            resource_id: ResourceId::bind_workbook(&params.workbook_or_fork_id)?,
            sheet_name: params.sheet_name,
            max_regions: params.max_regions,
            max_headers: params.max_headers,
            include_headers: params.include_headers,
        })
    }
}
impl TryFrom<tools::ReadTableParams> for ReadTableRequest {
    type Error = String;
    fn try_from(params: tools::ReadTableParams) -> Result<Self, Self::Error> {
        Ok(Self {
            resource_id: ResourceId::bind_workbook(&params.workbook_or_fork_id)?,
            sheet_name: params.sheet_name,
            table_name: params.table_name,
            region_id: params.region_id,
            range: params.range,
            header_row: params.header_row,
            header_rows: params.header_rows,
            columns: params.columns,
            filters: params.filters.map(|values| {
                values
                    .into_iter()
                    .map(|value| CanonicalTableFilter {
                        column: value.column,
                        op: value.op,
                        value: value.value,
                    })
                    .collect()
            }),
            sample_mode: params.sample_mode,
            limit: params.limit,
            offset: params.offset,
            format: params.format,
            include_headers: params.include_headers,
            include_types: params.include_types,
        })
    }
}
