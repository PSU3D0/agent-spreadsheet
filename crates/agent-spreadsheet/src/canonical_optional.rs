use crate::model::{ManifestSheetStub, WorkbookId};
use crate::operations::ResourceId;
use crate::state::AppState;
use crate::tools;
use anyhow::{Result, anyhow, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
const MAX_MANIFEST_BYTES: usize = 1_048_576;
const MAX_SHEETPORT_INPUTS: usize = 256;
const MAX_SHEETPORT_TEXT_BYTES: usize = 65_536;
const MAX_SHEETPORT_ROWS: usize = 10_000;
const MAX_SHEETPORT_CELLS: usize = 100_000;
const MAX_SHEETPORT_FIELDS: usize = 1_000;
const MAX_SHEET_FILTER_BYTES: usize = 128;
const MAX_VBA_MODULES_PER_PAGE: u32 = 100;
const MAX_VBA_LINES_PER_PAGE: u32 = 1_000;
const MAX_VBA_SOURCE_PAGE_BYTES: usize = 256 * 1024;
const MAX_VBA_CURSOR_BYTES: usize = 2_048;
#[cfg(feature = "recalc")]
const MAX_SCREENSHOT_BYTES: usize = 16 * 1024 * 1024;
#[cfg(feature = "recalc")]
const MAX_SCREENSHOT_SHEET_NAME_BYTES: usize = 31;
#[cfg(feature = "recalc")]
const MAX_SCREENSHOT_RANGE_BYTES: usize = 32;
#[cfg(feature = "recalc")]
const MAX_SCREENSHOT_ROWS: u32 = 100;
#[cfg(feature = "recalc")]
const MAX_SCREENSHOT_COLS: u32 = 30;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SheetportManifestRequest {
    Candidates {
        resource_id: ResourceId,
        #[serde(default)]
        #[schemars(length(max = 128))]
        sheet_filter: Option<String>,
    },
    Schema {},
    Validate {
        #[schemars(length(max = 1_048_576))]
        manifest_yaml: String,
    },
    Normalize {
        #[schemars(length(max = 1_048_576))]
        manifest_yaml: String,
    },
    BindCheck {
        resource_id: ResourceId,
        #[schemars(length(max = 1_048_576))]
        manifest_yaml: String,
    },
}

impl SheetportManifestRequest {
    pub fn resource_id(&self) -> Option<&ResourceId> {
        match self {
            Self::Candidates { resource_id, .. } | Self::BindCheck { resource_id, .. } => {
                Some(resource_id)
            }
            Self::Schema {} | Self::Validate { .. } | Self::Normalize { .. } => None,
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32"), feature = "recalc"))]
mod tests {
    use super::*;

    #[test]
    fn png_artifacts_are_content_addressed_bounded_and_path_free() {
        let workspace = tempfile::tempdir().unwrap();
        let bytes = b"bounded-png-fixture";
        let artifact = persist_png_artifact(workspace.path(), bytes).unwrap();
        let expected = format!("{:x}", Sha256::digest(bytes));

        assert_eq!(artifact.hash, format!("sha256:{expected}"));
        assert_eq!(artifact.handle, format!("artifact:sha256:{expected}"));
        assert_eq!(artifact.bytes, bytes.len() as u64);
        assert_eq!(artifact.media_type, "image/png");
        assert_eq!(
            std::fs::read(
                workspace
                    .path()
                    .join("artifacts")
                    .join(format!("{expected}.png"))
            )
            .unwrap(),
            bytes
        );
        let serialized = serde_json::to_value(artifact).unwrap();
        assert!(serialized.get("path").is_none());
        assert!(serialized.get("output_path").is_none());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SheetportIssue {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SheetportManifestData {
    Candidates {
        slug: String,
        manifest_yaml: String,
        sheets: Vec<ManifestSheetStub>,
    },
    Schema {
        schema: Value,
    },
    Validate {
        valid: bool,
        issues: Vec<SheetportIssue>,
    },
    Normalize {
        manifest_yaml: String,
        valid: bool,
        issues: Vec<SheetportIssue>,
    },
    BindCheck {
        ok: bool,
        stage: SheetportBindStage,
        binding_count: u32,
        issues: Vec<SheetportIssue>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SheetportBindStage {
    Complete,
    Parse,
    Validate,
    Bind,
}

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
fn ensure_manifest_bound(manifest_yaml: &str) -> Result<()> {
    if manifest_yaml.len() > MAX_MANIFEST_BYTES {
        bail!("invalid request: manifest_yaml exceeds {MAX_MANIFEST_BYTES} bytes");
    }
    Ok(())
}

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
fn issues_from_value(value: Value) -> Vec<SheetportIssue> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|issue| SheetportIssue {
            path: issue
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("<document>")
                .to_string(),
            message: issue
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| issue.as_str().unwrap_or("invalid manifest"))
                .to_string(),
        })
        .collect()
}

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
pub fn sheetport_schema() -> Result<Value> {
    serde_json::from_str(formualizer::sheetport_spec::schema_json())
        .map_err(|error| anyhow!("failed to parse bundled SheetPort schema: {error}"))
}

#[cfg(not(all(feature = "recalc-formualizer", feature = "sheetport")))]
pub fn sheetport_schema() -> Result<Value> {
    bail!("SheetPort capability unavailable")
}

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
pub fn validate_manifest_content(manifest_yaml: &str) -> Result<Vec<SheetportIssue>> {
    ensure_manifest_bound(manifest_yaml)?;
    let manifest = match formualizer::sheetport_spec::Manifest::from_yaml_str(manifest_yaml) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Ok(vec![SheetportIssue {
                path: "<document>".to_string(),
                message: error.to_string(),
            }]);
        }
    };
    Ok(match manifest.validate() {
        Ok(()) => Vec::new(),
        Err(error) => issues_from_value(
            serde_json::to_value(error.issues()).unwrap_or_else(|_| Value::Array(Vec::new())),
        ),
    })
}

#[cfg(not(all(feature = "recalc-formualizer", feature = "sheetport")))]
pub fn validate_manifest_content(_manifest_yaml: &str) -> Result<Vec<SheetportIssue>> {
    bail!("SheetPort capability unavailable")
}

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
pub fn normalize_manifest_content(manifest_yaml: &str) -> Result<(String, Vec<SheetportIssue>)> {
    ensure_manifest_bound(manifest_yaml)?;
    let mut manifest = formualizer::sheetport_spec::Manifest::from_yaml_str(manifest_yaml)
        .map_err(|error| anyhow!("invalid request: failed to parse manifest YAML: {error}"))?;
    manifest.normalize();
    let normalized = manifest
        .to_yaml()
        .map_err(|error| anyhow!("failed to serialize normalized manifest: {error}"))?;
    let issues = validate_manifest_content(&normalized)?;
    Ok((normalized, issues))
}

#[cfg(not(all(feature = "recalc-formualizer", feature = "sheetport")))]
pub fn normalize_manifest_content(_manifest_yaml: &str) -> Result<(String, Vec<SheetportIssue>)> {
    bail!("SheetPort capability unavailable")
}

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
pub async fn bind_check_manifest_content(
    state: Arc<AppState>,
    workbook_id: WorkbookId,
    manifest_yaml: &str,
) -> Result<SheetportManifestData> {
    use formualizer::workbook::SpreadsheetReader;

    ensure_manifest_bound(manifest_yaml)?;
    let manifest = match formualizer::sheetport_spec::Manifest::from_yaml_str(manifest_yaml) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Ok(SheetportManifestData::BindCheck {
                ok: false,
                stage: SheetportBindStage::Parse,
                binding_count: 0,
                issues: vec![SheetportIssue {
                    path: "<document>".to_string(),
                    message: error.to_string(),
                }],
            });
        }
    };
    if let Err(error) = manifest.validate() {
        return Ok(SheetportManifestData::BindCheck {
            ok: false,
            stage: SheetportBindStage::Validate,
            binding_count: 0,
            issues: issues_from_value(
                serde_json::to_value(error.issues()).unwrap_or_else(|_| Value::Array(Vec::new())),
            ),
        });
    }

    let workbook = state.open_workbook(&workbook_id).await?;
    let bytes = std::fs::read(&workbook.path)?;
    let adapter = formualizer::workbook::UmyaAdapter::open_bytes(bytes)
        .or_else(|_| formualizer::workbook::UmyaAdapter::open_path(&workbook.path))
        .map_err(|_| anyhow!("failed to open workbook adapter"))?;
    let workbook = formualizer::workbook::Workbook::from_reader(
        adapter,
        formualizer::workbook::LoadStrategy::EagerAll,
        formualizer::workbook::WorkbookConfig::ephemeral(),
    )
    .map_err(|_| anyhow!("failed to load workbook for SheetPort binding"))?;

    Ok(
        match formualizer::sheetport::SheetPortSession::new(workbook, manifest) {
            Ok(session) => SheetportManifestData::BindCheck {
                ok: true,
                stage: SheetportBindStage::Complete,
                binding_count: session.bindings().len() as u32,
                issues: Vec::new(),
            },
            Err(error) => SheetportManifestData::BindCheck {
                ok: false,
                stage: SheetportBindStage::Bind,
                binding_count: 0,
                issues: vec![SheetportIssue {
                    path: "$.ports".to_string(),
                    message: error.to_string(),
                }],
            },
        },
    )
}

#[cfg(not(all(feature = "recalc-formualizer", feature = "sheetport")))]
pub async fn bind_check_manifest_content(
    _state: Arc<AppState>,
    _workbook_id: WorkbookId,
    _manifest_yaml: &str,
) -> Result<SheetportManifestData> {
    bail!("SheetPort capability unavailable")
}

pub async fn execute_sheetport_manifest_action(
    state: Arc<AppState>,
    request: SheetportManifestRequest,
) -> Result<SheetportManifestData> {
    match request {
        SheetportManifestRequest::Candidates {
            resource_id,
            sheet_filter,
        } => {
            if sheet_filter
                .as_deref()
                .is_some_and(|value| value.len() > MAX_SHEET_FILTER_BYTES)
            {
                bail!("invalid request: sheet_filter exceeds {MAX_SHEET_FILTER_BYTES} bytes");
            }
            let response = tools::get_manifest_stub(
                state,
                tools::ManifestStubParams {
                    workbook_or_fork_id: resource_id.to_workbook_id(),
                    sheet_filter,
                },
            )
            .await?;
            Ok(SheetportManifestData::Candidates {
                slug: response.slug,
                manifest_yaml: response.manifest_yaml,
                sheets: response.sheets,
            })
        }
        SheetportManifestRequest::Schema {} => Ok(SheetportManifestData::Schema {
            schema: sheetport_schema()?,
        }),
        SheetportManifestRequest::Validate { manifest_yaml } => {
            let issues = validate_manifest_content(&manifest_yaml)?;
            Ok(SheetportManifestData::Validate {
                valid: issues.is_empty(),
                issues,
            })
        }
        SheetportManifestRequest::Normalize { manifest_yaml } => {
            let (manifest_yaml, issues) = normalize_manifest_content(&manifest_yaml)?;
            Ok(SheetportManifestData::Normalize {
                valid: issues.is_empty(),
                manifest_yaml,
                issues,
            })
        }
        SheetportManifestRequest::BindCheck {
            resource_id,
            manifest_yaml,
        } => bind_check_manifest_content(state, resource_id.to_workbook_id(), &manifest_yaml).await,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SheetportValue {
    Empty {},
    Boolean {
        value: bool,
    },
    Number {
        value: f64,
    },
    Integer {
        value: i64,
    },
    Text {
        #[schemars(length(max = 65_536))]
        value: String,
    },
    Range {
        #[schemars(length(max = 10_000))]
        rows: Vec<Vec<SheetportScalar>>,
    },
    Table {
        #[schemars(length(max = 10_000))]
        rows: Vec<BTreeMap<String, SheetportScalar>>,
    },
    Record {
        #[schemars(length(max = 1_000))]
        fields: BTreeMap<String, SheetportScalar>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SheetportScalar {
    Empty {},
    Boolean {
        value: bool,
    },
    Number {
        value: f64,
    },
    Integer {
        value: i64,
    },
    Text {
        #[schemars(length(max = 65_536))]
        value: String,
    },
}

impl SheetportScalar {
    fn to_json(&self) -> Value {
        match self {
            Self::Empty {} => Value::Null,
            Self::Boolean { value } => Value::Bool(*value),
            Self::Number { value } => serde_json::json!(value),
            Self::Integer { value } => serde_json::json!(value),
            Self::Text { value } => Value::String(value.clone()),
        }
    }

    fn from_json(value: &Value) -> Self {
        match value {
            Value::Null => Self::Empty {},
            Value::Bool(value) => Self::Boolean { value: *value },
            Value::Number(value) if value.is_i64() => Self::Integer {
                value: value.as_i64().unwrap_or_default(),
            },
            Value::Number(value) => Self::Number {
                value: value.as_f64().unwrap_or_default(),
            },
            Value::String(value) => Self::Text {
                value: value.clone(),
            },
            other => Self::Text {
                value: other.to_string(),
            },
        }
    }
}

impl SheetportValue {
    fn to_json(&self) -> Value {
        match self {
            Self::Empty {} => Value::Null,
            Self::Boolean { value } => Value::Bool(*value),
            Self::Number { value } => serde_json::json!(value),
            Self::Integer { value } => serde_json::json!(value),
            Self::Text { value } => Value::String(value.clone()),
            Self::Range { rows } => Value::Array(
                rows.iter()
                    .map(|row| Value::Array(row.iter().map(SheetportScalar::to_json).collect()))
                    .collect(),
            ),
            Self::Table { rows } => Value::Array(
                rows.iter()
                    .map(|row| {
                        Value::Object(
                            row.iter()
                                .map(|(key, value)| (key.clone(), value.to_json()))
                                .collect(),
                        )
                    })
                    .collect(),
            ),
            Self::Record { fields } => Value::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_json()))
                    .collect(),
            ),
        }
    }

    fn from_json(value: &Value) -> Self {
        match value {
            Value::Null => Self::Empty {},
            Value::Bool(value) => Self::Boolean { value: *value },
            Value::Number(value) if value.is_i64() => Self::Integer {
                value: value.as_i64().unwrap_or_default(),
            },
            Value::Number(value) => Self::Number {
                value: value.as_f64().unwrap_or_default(),
            },
            Value::String(value) => Self::Text {
                value: value.clone(),
            },
            Value::Object(fields) => Self::Record {
                fields: fields
                    .iter()
                    .map(|(key, value)| (key.clone(), SheetportScalar::from_json(value)))
                    .collect(),
            },
            Value::Array(rows)
                if rows
                    .first()
                    .is_some_and(|row| matches!(row, Value::Object(_))) =>
            {
                Self::Table {
                    rows: rows
                        .iter()
                        .filter_map(Value::as_object)
                        .map(|row| {
                            row.iter()
                                .map(|(key, value)| {
                                    (key.clone(), SheetportScalar::from_json(value))
                                })
                                .collect()
                        })
                        .collect(),
                }
            }
            Value::Array(rows) => Self::Range {
                rows: rows
                    .iter()
                    .map(|row| {
                        row.as_array()
                            .map(|values| values.iter().map(SheetportScalar::from_json).collect())
                            .unwrap_or_else(|| vec![SheetportScalar::from_json(row)])
                    })
                    .collect(),
            },
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteSheetportRequest {
    pub resource_id: ResourceId,
    #[schemars(length(max = 1_048_576))]
    pub manifest_yaml: String,
    #[serde(default)]
    #[schemars(length(max = 256))]
    pub inputs: BTreeMap<String, SheetportValue>,
    #[serde(default)]
    pub rng_seed: Option<u64>,
    #[serde(default)]
    pub freeze_volatile: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SheetportCoverageState {
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SheetportExecutionStatus {
    Completed,
    Partial,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SheetportExecutionErrorCode {
    MissingRequiredInput,
    OutputNotReturned,
    PortConstraintViolation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SheetportConstraintKind {
    Required,
    ManifestConstraint,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SheetportPortConstraintError {
    pub kind: SheetportConstraintKind,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SheetportExecutionCoverage {
    pub state: SheetportCoverageState,
    pub declared_input_ports: u32,
    pub supplied_input_ports: u32,
    pub declared_output_ports: u32,
    pub returned_output_ports: u32,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SheetportExecutionError {
    pub code: SheetportExecutionErrorCode,
    pub message: String,
    pub port_id: Option<String>,
    pub constraint: Option<SheetportPortConstraintError>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteSheetportData {
    pub status: SheetportExecutionStatus,
    pub results: BTreeMap<String, SheetportValue>,
    pub coverage: SheetportExecutionCoverage,
    pub errors: Vec<SheetportExecutionError>,
}

struct ManifestPortSets {
    inputs: BTreeSet<String>,
    required_inputs: BTreeSet<String>,
    outputs: BTreeSet<String>,
}

#[cfg(all(feature = "recalc-formualizer", feature = "sheetport"))]
fn manifest_port_sets(manifest_yaml: &str) -> Result<ManifestPortSets> {
    ensure_manifest_bound(manifest_yaml)?;
    let manifest = formualizer::sheetport_spec::Manifest::from_yaml_str(manifest_yaml)
        .map_err(|error| anyhow!("invalid request: failed to parse manifest YAML: {error}"))?;
    let mut inputs = BTreeSet::new();
    let mut required_inputs = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    for port in manifest.ports {
        match port.dir {
            formualizer::sheetport_spec::Direction::In => {
                if port.required && port.default.is_none() {
                    required_inputs.insert(port.id.clone());
                }
                inputs.insert(port.id);
            }
            formualizer::sheetport_spec::Direction::Out => {
                outputs.insert(port.id);
            }
        }
    }
    Ok(ManifestPortSets {
        inputs,
        required_inputs,
        outputs,
    })
}

#[cfg(not(all(feature = "recalc-formualizer", feature = "sheetport")))]
fn manifest_port_sets(_manifest_yaml: &str) -> Result<ManifestPortSets> {
    bail!("SheetPort capability unavailable")
}

fn validate_sheetport_value(value: &SheetportValue, cells: &mut usize) -> Result<()> {
    let validate_scalar = |scalar: &SheetportScalar| -> Result<()> {
        if let SheetportScalar::Text { value } = scalar
            && value.len() > MAX_SHEETPORT_TEXT_BYTES
        {
            bail!("invalid request: SheetPort text exceeds {MAX_SHEETPORT_TEXT_BYTES} bytes");
        }
        Ok(())
    };
    match value {
        SheetportValue::Text { value } if value.len() > MAX_SHEETPORT_TEXT_BYTES => {
            bail!("invalid request: SheetPort text exceeds {MAX_SHEETPORT_TEXT_BYTES} bytes")
        }
        SheetportValue::Range { rows } => {
            if rows.len() > MAX_SHEETPORT_ROWS {
                bail!("invalid request: SheetPort range exceeds {MAX_SHEETPORT_ROWS} rows");
            }
            for row in rows {
                *cells = cells.saturating_add(row.len());
                for scalar in row {
                    validate_scalar(scalar)?;
                }
            }
        }
        SheetportValue::Table { rows } => {
            if rows.len() > MAX_SHEETPORT_ROWS {
                bail!("invalid request: SheetPort table exceeds {MAX_SHEETPORT_ROWS} rows");
            }
            for row in rows {
                if row.len() > MAX_SHEETPORT_FIELDS {
                    bail!("invalid request: SheetPort row exceeds {MAX_SHEETPORT_FIELDS} cells");
                }
                *cells = cells.saturating_add(row.len());
                for scalar in row.values() {
                    validate_scalar(scalar)?;
                }
            }
        }
        SheetportValue::Record { fields } => {
            if fields.len() > MAX_SHEETPORT_FIELDS {
                bail!("invalid request: SheetPort record exceeds {MAX_SHEETPORT_FIELDS} fields");
            }
            *cells = cells.saturating_add(fields.len());
            for scalar in fields.values() {
                validate_scalar(scalar)?;
            }
        }
        _ => *cells = cells.saturating_add(1),
    }
    if *cells > MAX_SHEETPORT_CELLS {
        bail!("invalid request: SheetPort inputs exceed {MAX_SHEETPORT_CELLS} cells");
    }
    Ok(())
}

pub async fn execute_sheetport(
    state: Arc<AppState>,
    request: ExecuteSheetportRequest,
) -> Result<ExecuteSheetportData> {
    if request.inputs.len() > MAX_SHEETPORT_INPUTS {
        bail!("invalid request: inputs exceeds {MAX_SHEETPORT_INPUTS} ports");
    }
    let issues = validate_manifest_content(&request.manifest_yaml)?;
    if !issues.is_empty() {
        bail!("invalid request: SheetPort manifest failed validation");
    }
    let ports = manifest_port_sets(&request.manifest_yaml)?;
    if let Some(unknown) = request
        .inputs
        .keys()
        .find(|port_id| !ports.inputs.contains(*port_id))
    {
        bail!("invalid request: input port '{unknown}' is not declared by the manifest");
    }
    let mut input_cells = 0;
    for value in request.inputs.values() {
        validate_sheetport_value(value, &mut input_cells)?;
    }
    let supplied_input_ports = request.inputs.len() as u32;
    let supplied = request.inputs.keys().cloned().collect::<BTreeSet<_>>();
    let missing_required = ports
        .required_inputs
        .difference(&supplied)
        .cloned()
        .collect::<Vec<_>>();
    if !missing_required.is_empty() {
        return Ok(ExecuteSheetportData {
            status: SheetportExecutionStatus::Failed,
            results: BTreeMap::new(),
            coverage: SheetportExecutionCoverage {
                state: SheetportCoverageState::Partial,
                declared_input_ports: ports.inputs.len() as u32,
                supplied_input_ports,
                declared_output_ports: ports.outputs.len() as u32,
                returned_output_ports: 0,
            },
            errors: missing_required
                .into_iter()
                .map(|port_id| SheetportExecutionError {
                    code: SheetportExecutionErrorCode::MissingRequiredInput,
                    message: "required input port was not supplied".to_string(),
                    port_id: Some(port_id),
                    constraint: Some(SheetportPortConstraintError {
                        kind: SheetportConstraintKind::Required,
                        expected: "supplied input value".to_string(),
                        actual: "missing".to_string(),
                    }),
                })
                .collect(),
        });
    }
    let response = match tools::execute_manifest(
        state,
        tools::ExecuteManifestParams {
            workbook_or_fork_id: request.resource_id.to_workbook_id(),
            manifest_yaml: request.manifest_yaml,
            inputs: request
                .inputs
                .into_iter()
                .map(|(key, value)| (key, value.to_json()))
                .collect(),
            rng_seed: request.rng_seed,
            freeze_volatile: request.freeze_volatile,
        },
    )
    .await
    {
        Ok(response) => response,
        Err(error)
            if error
                .to_string()
                .to_ascii_lowercase()
                .contains("constraint") =>
        {
            return Ok(ExecuteSheetportData {
                status: SheetportExecutionStatus::Failed,
                results: BTreeMap::new(),
                coverage: SheetportExecutionCoverage {
                    state: SheetportCoverageState::Partial,
                    declared_input_ports: ports.inputs.len() as u32,
                    supplied_input_ports,
                    declared_output_ports: ports.outputs.len() as u32,
                    returned_output_ports: 0,
                },
                errors: vec![SheetportExecutionError {
                    code: SheetportExecutionErrorCode::PortConstraintViolation,
                    message: "a supplied port value violated its manifest constraint".to_string(),
                    port_id: None,
                    constraint: Some(SheetportPortConstraintError {
                        kind: SheetportConstraintKind::ManifestConstraint,
                        expected: "value satisfying the declared manifest constraint".to_string(),
                        actual: "constraint violation".to_string(),
                    }),
                }],
            });
        }
        Err(error) => return Err(error),
    };
    let results = response
        .outputs
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), SheetportValue::from_json(value)))
        .collect::<BTreeMap<_, _>>();
    if results.len() > MAX_SHEETPORT_INPUTS {
        bail!("SheetPort results exceed {MAX_SHEETPORT_INPUTS} ports");
    }
    let mut result_cells = 0;
    for value in results.values() {
        validate_sheetport_value(value, &mut result_cells)
            .map_err(|_| anyhow!("SheetPort results exceed runtime bounds"))?;
    }
    let missing = ports
        .outputs
        .iter()
        .filter(|port_id| !results.contains_key(*port_id))
        .cloned()
        .collect::<Vec<_>>();
    let errors = missing
        .iter()
        .map(|port_id| SheetportExecutionError {
            code: SheetportExecutionErrorCode::OutputNotReturned,
            message: "declared output port was not returned".to_string(),
            port_id: Some(port_id.clone()),
            constraint: None,
        })
        .collect::<Vec<_>>();
    Ok(ExecuteSheetportData {
        status: if errors.is_empty() {
            SheetportExecutionStatus::Completed
        } else {
            SheetportExecutionStatus::Partial
        },
        coverage: SheetportExecutionCoverage {
            state: if errors.is_empty() {
                SheetportCoverageState::Complete
            } else {
                SheetportCoverageState::Partial
            },
            declared_input_ports: ports.inputs.len() as u32,
            supplied_input_ports,
            declared_output_ports: ports.outputs.len() as u32,
            returned_output_ports: results.len() as u32,
        },
        results,
        errors,
    })
}

/// Which renderer to use. `native` is the in-process raster renderer; it needs
/// no external process and is the default wherever it is compiled in.
/// `libreoffice` is the legacy macro-to-PDF-to-PNG path and stays opt-in.
#[cfg(feature = "recalc")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotBackend {
    Native,
    Libreoffice,
}

#[cfg(feature = "recalc")]
impl ScreenshotBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            ScreenshotBackend::Native => "native",
            ScreenshotBackend::Libreoffice => "libreoffice",
        }
    }
}

/// PNG encoder effort. Encoding dominates render time, so this is the one
/// renderer knob worth exposing: `fast` trades bytes for latency, `best`
/// trades latency for bytes. Geometry never depends on it.
#[cfg(feature = "recalc")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotPngLevel {
    Fast,
    Balanced,
    Best,
}

#[cfg(feature = "recalc")]
impl ScreenshotPngLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            ScreenshotPngLevel::Fast => "fast",
            ScreenshotPngLevel::Balanced => "balanced",
            ScreenshotPngLevel::Best => "best",
        }
    }
}

#[cfg(feature = "recalc")]
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotSheetRequest {
    pub resource_id: ResourceId,
    #[schemars(length(min = 1, max = 31))]
    pub sheet_name: String,
    #[serde(default)]
    #[schemars(
        pattern(r"^[A-Za-z]{1,3}[1-9][0-9]{0,6}(:[A-Za-z]{1,3}[1-9][0-9]{0,6})?$"),
        length(max = 32)
    )]
    pub range: Option<String>,
    /// Defaults to `native` when the `render` feature is compiled in, and to
    /// `libreoffice` otherwise.
    #[serde(default)]
    pub backend: Option<ScreenshotBackend>,
    /// PNG encoder effort for the native backend. Defaults to `balanced`.
    /// Rejected by the LibreOffice backend, which owns its own encoder.
    #[serde(default)]
    pub png_level: Option<ScreenshotPngLevel>,
}

#[cfg(feature = "recalc")]
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactHandle {
    pub handle: String,
    pub hash: String,
    pub bytes: u64,
    pub media_type: String,
}

/// How faithful the render is. Mirrors `agent_spreadsheet_render::Fidelity`,
/// and is `full` for the LibreOffice backend, which reports no warnings.
#[cfg(feature = "recalc")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotFidelity {
    Full,
    Partial,
}

/// Structured account of what the renderer did not reproduce. A closed set:
/// nothing unsupported disappears silently.
#[cfg(feature = "recalc")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotWarning {
    ConditionalFormatOmitted,
    ChartOmitted,
    ImageOmitted,
    FontSubstituted,
    RichTextFlattened,
    NumberFormatApproximated,
    FormulasUnevaluated,
    TextRotationOmitted,
    PatternFillApproximated,
}

#[cfg(feature = "recalc")]
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotSheetData {
    pub sheet_name: String,
    pub range: String,
    pub artifact: ArtifactHandle,
    pub duration_ms: u64,
    /// Renderer identity, e.g. `native-raster/1` or `libreoffice`. Additive:
    /// older payloads without it deserialize to the LibreOffice default.
    #[serde(default = "default_renderer")]
    pub renderer: String,
    #[serde(default = "default_fidelity")]
    pub fidelity: ScreenshotFidelity,
    #[serde(default)]
    pub warnings: Vec<ScreenshotWarning>,
    /// Rendered image geometry in device pixels. Reported by renderers that
    /// know it before encoding; `null` for the LibreOffice path, which does
    /// not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// The PNG encoder effort the render actually used, `null` when the
    /// backend does not expose one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub png_level: Option<ScreenshotPngLevel>,
    /// Calculation state at the rendered revision. Rendering never
    /// recalculates, so this is how a caller learns whether what it is looking
    /// at is current.
    pub calculation: crate::model::CalculationMetadata,
}

#[cfg(feature = "recalc")]
fn default_renderer() -> String {
    "libreoffice".to_string()
}

#[cfg(feature = "recalc")]
fn default_fidelity() -> ScreenshotFidelity {
    ScreenshotFidelity::Full
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-fs", feature = "recalc"))]
fn safe_artifact_root(workspace_root: &std::path::Path) -> Result<std::path::PathBuf> {
    use std::fs;
    let workspace = workspace_root.canonicalize()?;
    let root = workspace.join("artifacts");
    match fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            bail!("invalid request: artifact root must be a real directory")
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)] {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&root)?;
        },
        Err(error) => return Err(error.into()),
    }
    let canonical = root.canonicalize()?;
    if !canonical.starts_with(workspace) {
        bail!("invalid request: artifact root escapes workspace");
    }
    Ok(canonical)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-fs", feature = "recalc"))]
fn validate_screenshot_directory(state: &AppState) -> Result<std::path::PathBuf> {
    use std::fs;
    let config = state.config();
    let workspace = config
        .workspace_root
        .canonicalize()
        .map_err(|_| anyhow!("invalid request: screenshot workspace is unavailable"))?;
    let configured = &config.screenshot_dir;
    match fs::symlink_metadata(configured) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            bail!("invalid request: screenshot directory must be a real directory")
        }
        Ok(_) => {
            let canonical = configured
                .canonicalize()
                .map_err(|_| anyhow!("invalid request: screenshot directory is unavailable"))?;
            if !canonical.starts_with(&workspace) {
                bail!("invalid request: screenshot directory escapes workspace");
            }
            Ok(canonical)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = configured
                .parent()
                .ok_or_else(|| anyhow!("invalid request: screenshot directory has no parent"))?
                .canonicalize()
                .map_err(|_| {
                    anyhow!("invalid request: screenshot directory parent is unavailable")
                })?;
            if !parent.starts_with(&workspace) {
                bail!("invalid request: screenshot directory escapes workspace");
            }
            Ok(configured.clone())
        }
        Err(_) => bail!("invalid request: screenshot directory is unavailable"),
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-fs", feature = "recalc"))]
pub(crate) fn persist_png_artifact(workspace_root: &std::path::Path, bytes: &[u8]) -> Result<ArtifactHandle> {
    use std::fs;
    use std::io::Write;

    if bytes.len() > MAX_SCREENSHOT_BYTES {
        bail!("screenshot artifact exceeds {MAX_SCREENSHOT_BYTES} bytes");
    }
    let hash = format!("{:x}", Sha256::digest(bytes));
    let artifact_root = safe_artifact_root(workspace_root)?;
    let target = artifact_root.join(format!("{hash}.png"));
    match fs::symlink_metadata(&target) {
        Ok(existing) if existing.file_type().is_symlink() || !existing.is_file() => {
            bail!("artifact hash collision with non-regular object")
        }
        Ok(_) => {
            if fs::read(&target)? != bytes {
                bail!("artifact hash collision")
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut temporary = tempfile::NamedTempFile::new_in(&artifact_root)?;
            temporary.write_all(bytes)?;
            temporary.as_file().sync_all()?;
            if let Err(error) = temporary.persist_noclobber(&target) {
                if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                    return persist_png_artifact(workspace_root, bytes);
                }
                return Err(error.error.into());
            }
        }
        Err(error) => return Err(error.into()),
    }
    let qualified_hash = format!("sha256:{hash}");
    Ok(ArtifactHandle {
        handle: format!("artifact:{qualified_hash}"),
        hash: qualified_hash,
        bytes: bytes.len() as u64,
        media_type: "image/png".to_string(),
    })
}

#[cfg(feature = "recalc")]
pub fn validate_screenshot_request(request: &ScreenshotSheetRequest) -> Result<()> {
    let sheet_name = request.sheet_name.as_str();
    if sheet_name.is_empty()
        || sheet_name.len() > MAX_SCREENSHOT_SHEET_NAME_BYTES
        || sheet_name.chars().any(|character| {
            character.is_control() || matches!(character, '/' | '\\' | '[' | ']' | ':' | '*' | '?')
        })
    {
        bail!("invalid request: sheet_name is not a valid worksheet name");
    }
    let Some(range) = request.range.as_deref() else {
        return Ok(());
    };
    if range.len() > MAX_SCREENSHOT_RANGE_BYTES {
        bail!("invalid request: screenshot range exceeds {MAX_SCREENSHOT_RANGE_BYTES} bytes");
    }
    let mut parts = range.split(':');
    let start = parts.next().unwrap_or_default();
    let end = parts.next().unwrap_or(start);
    if parts.next().is_some() {
        bail!("invalid request: screenshot range must be A1 or A1:B2");
    }
    let (start_col, start_row) = crate::write::validate_cell_address(start)
        .map_err(|_| anyhow!("invalid request: screenshot range must be A1 or A1:B2"))?;
    let (end_col, end_row) = crate::write::validate_cell_address(end)
        .map_err(|_| anyhow!("invalid request: screenshot range must be A1 or A1:B2"))?;
    let rows = start_row.abs_diff(end_row).saturating_add(1);
    let columns = start_col.abs_diff(end_col).saturating_add(1);
    if rows > MAX_SCREENSHOT_ROWS || columns > MAX_SCREENSHOT_COLS {
        bail!(
            "invalid request: screenshot range exceeds {MAX_SCREENSHOT_ROWS} rows by {MAX_SCREENSHOT_COLS} columns"
        );
    }
    Ok(())
}

/// The backend a request resolves to. Native wins by default wherever it is
/// compiled in; an explicit `backend` always wins over the default.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-fs", feature = "recalc"))]
fn resolve_backend(requested: Option<ScreenshotBackend>) -> ScreenshotBackend {
    match requested {
        Some(backend) => backend,
        None if cfg!(feature = "render") => ScreenshotBackend::Native,
        None => ScreenshotBackend::Libreoffice,
    }
}

// Only the native path applies this default; the LibreOffice path gets its own
// from `tools::fork`.
#[cfg(all(feature = "recalc", feature = "render"))]
pub const DEFAULT_SCREENSHOT_RANGE: &str = "A1:M40";

#[cfg(all(not(target_arch = "wasm32"), feature = "native-fs", feature = "recalc"))]
pub async fn screenshot_sheet(
    state: Arc<AppState>,
    request: ScreenshotSheetRequest,
) -> Result<ScreenshotSheetData> {
    validate_screenshot_request(&request)?;
    match resolve_backend(request.backend) {
        ScreenshotBackend::Native => screenshot_sheet_native(state, request).await,
        ScreenshotBackend::Libreoffice => {
            if request.png_level.is_some() {
                bail!("invalid request: png_level applies to the native backend only");
            }
            screenshot_sheet_libreoffice(state, request).await
        }
    }
}

/// Adapters with no host filesystem never reach this entry point: they
/// intercept `screenshot_sheet` and hold the artifact bytes themselves (the
/// WASM byte/session adapter keeps them in a bounded per-session slot). Keeping
/// the symbol makes the canonical dispatcher build everywhere.
#[cfg(all(
    feature = "recalc",
    not(all(not(target_arch = "wasm32"), feature = "native-fs"))
))]
pub async fn screenshot_sheet(
    _state: Arc<AppState>,
    _request: ScreenshotSheetRequest,
) -> Result<ScreenshotSheetData> {
    bail!(
        "invalid request: screenshot_sheet requires an adapter that can hold artifact bytes"
    )
}

/// Project one native render onto the canonical screenshot envelope.
///
/// Every adapter that runs the in-process renderer shares this, so `renderer`,
/// `fidelity`, `warnings` and geometry cannot drift between the file-backed
/// native path and the WASM session-slot path. Only the artifact handle differs.
#[cfg(all(feature = "recalc", feature = "render"))]
#[allow(clippy::too_many_arguments)]
pub fn screenshot_data_from_render(
    sheet_name: String,
    range: String,
    artifact: ArtifactHandle,
    duration_ms: u64,
    png_level: ScreenshotPngLevel,
    rendered: &crate::render::NativeScreenshot,
    calculation: crate::model::CalculationMetadata,
) -> ScreenshotSheetData {
    ScreenshotSheetData {
        sheet_name,
        range,
        artifact,
        duration_ms,
        renderer: rendered.report.renderer.clone(),
        fidelity: match rendered.report.fidelity {
            crate::render::Fidelity::Full => ScreenshotFidelity::Full,
            crate::render::Fidelity::Partial => ScreenshotFidelity::Partial,
        },
        warnings: rendered
            .report
            .warnings
            .iter()
            .map(|warning| map_render_warning(*warning))
            .collect(),
        width: Some(rendered.width),
        height: Some(rendered.height),
        png_level: Some(png_level),
        calculation,
    }
}

/// The largest PNG any adapter may hand back inside one artifact.
#[cfg(feature = "recalc")]
pub const fn max_screenshot_bytes() -> usize {
    MAX_SCREENSHOT_BYTES
}

/// Calculation state for the rendered revision, sourced exactly the way
/// `read_cells` sources its own calculation block.
#[cfg(feature = "recalc")]
pub fn calculation_for(
    state: &AppState,
    workbook: &crate::workbook::WorkbookContext,
) -> crate::model::CalculationMetadata {
    state.calculation_metadata(
        &workbook.id,
        &workbook.revision_id,
        workbook.calculation_metadata(),
    )
}

/// In-process raster render. No subprocess, no temporary file: the bytes go
/// straight into the same content-addressed artifact store the LibreOffice
/// path writes to, so MCP artifact handling is unchanged.
#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "native-fs",
    feature = "recalc",
    feature = "render"
))]
async fn screenshot_sheet_native(
    state: Arc<AppState>,
    request: ScreenshotSheetRequest,
) -> Result<ScreenshotSheetData> {
    let started = std::time::Instant::now();
    let workbook_id = request.resource_id.to_workbook_id();
    let workbook = state
        .open_workbook(&workbook_id)
        .await
        .map_err(|_| anyhow!("screenshot rendering failed"))?;
    let range = request
        .range
        .as_deref()
        .unwrap_or(DEFAULT_SCREENSHOT_RANGE)
        .to_string();
    let png_level = request.png_level.unwrap_or(ScreenshotPngLevel::Balanced);
    let rendered = crate::render::render_sheet(
        &workbook,
        &request.sheet_name,
        &range,
        map_png_level(png_level),
    )
    .map_err(|_| anyhow!("screenshot rendering failed"))?;
    let artifact = persist_png_artifact(&state.config().workspace_root, &rendered.png)
        .map_err(|_| anyhow!("screenshot artifact persistence failed"))?;
    Ok(screenshot_data_from_render(
        request.sheet_name,
        range,
        artifact,
        started.elapsed().as_millis() as u64,
        png_level,
        &rendered,
        calculation_for(&state, &workbook),
    ))
}

/// Canonical PNG level to the renderer's own enum.
#[cfg(all(feature = "recalc", feature = "render"))]
pub fn map_png_level(level: ScreenshotPngLevel) -> crate::render::PngLevel {
    match level {
        ScreenshotPngLevel::Fast => crate::render::PngLevel::Fast,
        ScreenshotPngLevel::Balanced => crate::render::PngLevel::Balanced,
        ScreenshotPngLevel::Best => crate::render::PngLevel::Best,
    }
}

#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "native-fs",
    feature = "recalc",
    not(feature = "render")
))]
async fn screenshot_sheet_native(
    _state: Arc<AppState>,
    _request: ScreenshotSheetRequest,
) -> Result<ScreenshotSheetData> {
    bail!(
        "invalid request: the native screenshot backend is not compiled in (build with the `render` feature)"
    )
}

#[cfg(all(feature = "recalc", feature = "render"))]
pub fn map_render_warning(warning: crate::render::Warning) -> ScreenshotWarning {
    use crate::render::Warning as W;
    match warning {
        W::ConditionalFormatOmitted => ScreenshotWarning::ConditionalFormatOmitted,
        W::ChartOmitted => ScreenshotWarning::ChartOmitted,
        W::ImageOmitted => ScreenshotWarning::ImageOmitted,
        W::FontSubstituted => ScreenshotWarning::FontSubstituted,
        W::RichTextFlattened => ScreenshotWarning::RichTextFlattened,
        W::NumberFormatApproximated => ScreenshotWarning::NumberFormatApproximated,
        W::FormulasUnevaluated => ScreenshotWarning::FormulasUnevaluated,
        W::TextRotationOmitted => ScreenshotWarning::TextRotationOmitted,
        W::PatternFillApproximated => ScreenshotWarning::PatternFillApproximated,
    }
}

/// The legacy macro-to-PDF-to-PNG path, unchanged except that it now reports
/// its renderer identity and the calculation state alongside the artifact.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-fs", feature = "recalc"))]
async fn screenshot_sheet_libreoffice(
    state: Arc<AppState>,
    request: ScreenshotSheetRequest,
) -> Result<ScreenshotSheetData> {
    use std::fs;

    let screenshot_root = validate_screenshot_directory(&state)?;
    let workbook_id = request.resource_id.to_workbook_id();
    let response = tools::fork::screenshot_sheet(
        state.clone(),
        tools::fork::ScreenshotSheetParams {
            workbook_or_fork_id: workbook_id.clone(),
            sheet_name: request.sheet_name,
            range: request.range,
        },
    )
    .await
    .map_err(|_| anyhow!("screenshot rendering failed"))?;
    let path = response
        .output_path
        .strip_prefix("file://")
        .ok_or_else(|| anyhow!("screenshot rendering failed"))?;
    let path = std::path::PathBuf::from(path);
    let metadata =
        fs::symlink_metadata(&path).map_err(|_| anyhow!("screenshot rendering failed"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("screenshot rendering failed");
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| anyhow!("screenshot rendering failed"))?;
    let canonical_root = screenshot_root
        .canonicalize()
        .map_err(|_| anyhow!("screenshot rendering failed"))?;
    if !canonical.starts_with(&canonical_root) {
        bail!("screenshot rendering failed");
    }
    if metadata.len() > MAX_SCREENSHOT_BYTES as u64 {
        bail!("screenshot artifact exceeds {MAX_SCREENSHOT_BYTES} bytes");
    }
    let bytes = fs::read(&canonical).map_err(|_| anyhow!("screenshot rendering failed"))?;
    let artifact = persist_png_artifact(&state.config().workspace_root, &bytes)
        .map_err(|_| anyhow!("screenshot artifact persistence failed"))?;
    let workbook = state
        .open_workbook(&workbook_id)
        .await
        .map_err(|_| anyhow!("screenshot rendering failed"))?;
    Ok(ScreenshotSheetData {
        sheet_name: response.sheet_name,
        range: response.range,
        artifact,
        duration_ms: response.duration_ms,
        renderer: ScreenshotBackend::Libreoffice.as_str().to_string(),
        // LibreOffice renders the workbook itself, so it makes no claim about
        // what it dropped; it reports no warnings rather than guessing.
        fidelity: ScreenshotFidelity::Full,
        warnings: Vec::new(),
        width: None,
        height: None,
        png_level: None,
        calculation: calculation_for(&state, &workbook),
    })
}

#[derive(Debug, Clone, JsonSchema)]
pub struct VbaModuleName(
    #[schemars(pattern(r"^[A-Za-z_][A-Za-z0-9_]{0,127}$"), length(min = 1, max = 128))] String,
);

impl VbaModuleName {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for VbaModuleName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        validate_module_name(&value).map_err(serde::de::Error::custom)?;
        Ok(Self(value))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "view", rename_all = "snake_case", deny_unknown_fields)]
pub enum InspectVbaRequest {
    ProjectSummary {
        resource_id: ResourceId,
        #[serde(default)]
        #[schemars(length(max = 2_048))]
        cursor: Option<String>,
        #[serde(default)]
        #[schemars(range(min = 1, max = 100))]
        limit_modules: Option<u32>,
        #[serde(default)]
        include_references: Option<bool>,
    },
    ModuleSource {
        resource_id: ResourceId,
        module_name: VbaModuleName,
        #[serde(default)]
        #[schemars(length(max = 2_048))]
        cursor: Option<String>,
        #[serde(default)]
        #[schemars(range(min = 1, max = 1_000))]
        limit_lines: Option<u32>,
    },
}

impl InspectVbaRequest {
    pub fn resource_id(&self) -> &ResourceId {
        match self {
            Self::ProjectSummary { resource_id, .. } | Self::ModuleSource { resource_id, .. } => {
                resource_id
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VbaModuleSummary {
    pub name: String,
    pub module_type: String,
    pub read_only: bool,
    pub private: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VbaReferenceSummary {
    pub kind: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "view", rename_all = "snake_case", deny_unknown_fields)]
pub enum InspectVbaData {
    ProjectSummary {
        has_vba: bool,
        code_page: Option<u16>,
        sys_kind: Option<String>,
        #[schemars(length(max = 100))]
        modules: Vec<VbaModuleSummary>,
        references: Vec<VbaReferenceSummary>,
        next_cursor: Option<String>,
    },
    ModuleSource {
        module_name: String,
        start_line: u32,
        returned_lines: u32,
        #[schemars(length(max = 262_144))]
        source: String,
        next_cursor: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct VbaCursorPayload {
    revision: String,
    fingerprint: String,
    offset: u32,
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("cursor mismatch: malformed VBA cursor");
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| anyhow!("cursor mismatch: malformed VBA cursor"))
        })
        .collect()
}

fn vba_fingerprint(
    resource_id: &ResourceId,
    view: &str,
    module_name: Option<&str>,
    include_references: bool,
) -> String {
    let material = format!(
        "{}\0{view}\0{}\0{include_references}",
        resource_id.as_str(),
        module_name.unwrap_or("")
    );
    format!("{:x}", Sha256::digest(material.as_bytes()))
}

fn encode_vba_cursor(revision: &str, fingerprint: &str, offset: u32) -> Result<String> {
    let payload = serde_json::to_vec(&VbaCursorPayload {
        revision: revision.to_string(),
        fingerprint: fingerprint.to_string(),
        offset,
    })?;
    let body = encode_hex(&payload);
    let signature = format!("{:x}", Sha256::digest(format!("vba1:{body}").as_bytes()));
    Ok(format!("vba1.{body}.{signature}"))
}

fn decode_vba_cursor(cursor: Option<&str>, revision: &str, fingerprint: &str) -> Result<u32> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    if cursor.len() > MAX_VBA_CURSOR_BYTES {
        bail!("cursor mismatch: malformed VBA cursor");
    }
    let mut parts = cursor.split('.');
    let (Some("vba1"), Some(body), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        bail!("cursor mismatch: malformed VBA cursor");
    };
    let expected = format!("{:x}", Sha256::digest(format!("vba1:{body}").as_bytes()));
    if signature != expected {
        bail!("cursor mismatch: malformed VBA cursor");
    }
    let payload: VbaCursorPayload = serde_json::from_slice(&decode_hex(body)?)
        .map_err(|_| anyhow!("cursor mismatch: malformed VBA cursor"))?;
    if payload.revision != revision {
        bail!("stale cursor: VBA project revision changed");
    }
    if payload.fingerprint != fingerprint {
        bail!("cursor mismatch: VBA cursor belongs to another request");
    }
    Ok(payload.offset)
}

fn validate_module_name(module_name: &str) -> Result<()> {
    let mut bytes = module_name.bytes();
    let first = bytes.next();
    if module_name.len() > 128
        || !first.is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        bail!("invalid request: module_name must be a VBA identifier of at most 128 bytes");
    }
    Ok(())
}

pub async fn inspect_vba(
    state: Arc<AppState>,
    request: InspectVbaRequest,
    revision: &str,
) -> Result<InspectVbaData> {
    inspect_vba_bound(state, request, revision, None).await
}

/// Separate logical cursor identity from an explicitly cold snapshot lookup.
/// Rebinding must never make a valid next-page cursor depend on a temp path.
pub(crate) async fn inspect_vba_bound(
    state: Arc<AppState>,
    request: InspectVbaRequest,
    revision: &str,
    snapshot_id: Option<crate::model::WorkbookId>,
) -> Result<InspectVbaData> {
    match request {
        InspectVbaRequest::ProjectSummary {
            resource_id,
            cursor,
            limit_modules,
            include_references,
        } => {
            let include_references = include_references.unwrap_or(true);
            let limit = limit_modules.unwrap_or(50);
            if !(1..=MAX_VBA_MODULES_PER_PAGE).contains(&limit) {
                bail!(
                    "invalid request: limit_modules must be between 1 and {MAX_VBA_MODULES_PER_PAGE}"
                );
            }
            let fingerprint =
                vba_fingerprint(&resource_id, "project_summary", None, include_references);
            let offset = decode_vba_cursor(cursor.as_deref(), revision, &fingerprint)?;
            if offset > 10_000 {
                bail!("invalid request: VBA module cursor exceeds paging limit");
            }
            let response = tools::vba::vba_project_summary(
                state,
                tools::vba::VbaProjectSummaryParams {
                    workbook_or_fork_id: snapshot_id.clone().unwrap_or_else(|| resource_id.to_workbook_id()),
                    max_modules: Some(offset.saturating_add(limit).saturating_add(1)),
                    include_references: Some(include_references),
                },
            )
            .await
            .map_err(|_| anyhow!("VBA project summary failed"))?;
            let has_more = response.modules.len() > offset.saturating_add(limit) as usize
                || response.modules_truncated;
            let modules = response
                .modules
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .map(|module| VbaModuleSummary {
                    name: module.name,
                    module_type: module.module_type,
                    read_only: module.read_only,
                    private: module.private,
                })
                .collect::<Vec<_>>();
            let next_offset = offset.saturating_add(modules.len() as u32);
            Ok(InspectVbaData::ProjectSummary {
                has_vba: response.has_vba,
                code_page: response.code_page,
                sys_kind: response.sys_kind,
                modules,
                references: response
                    .references
                    .into_iter()
                    .map(|reference| VbaReferenceSummary {
                        kind: reference.kind,
                    })
                    .collect(),
                next_cursor: has_more
                    .then(|| encode_vba_cursor(revision, &fingerprint, next_offset))
                    .transpose()?,
            })
        }
        InspectVbaRequest::ModuleSource {
            resource_id,
            module_name,
            cursor,
            limit_lines,
        } => {
            let module_name = module_name.as_str().to_string();
            let limit = limit_lines.unwrap_or(200);
            if !(1..=MAX_VBA_LINES_PER_PAGE).contains(&limit) {
                bail!(
                    "invalid request: limit_lines must be between 1 and {MAX_VBA_LINES_PER_PAGE}"
                );
            }
            let fingerprint =
                vba_fingerprint(&resource_id, "module_source", Some(&module_name), false);
            let offset = decode_vba_cursor(cursor.as_deref(), revision, &fingerprint)?;
            if offset > 1_000_000 {
                bail!("invalid request: VBA source cursor exceeds paging limit");
            }
            let summary = tools::vba::vba_project_summary(
                state.clone(),
                tools::vba::VbaProjectSummaryParams {
                    workbook_or_fork_id: snapshot_id.clone().unwrap_or_else(|| resource_id.to_workbook_id()),
                    max_modules: Some(10_000),
                    include_references: Some(false),
                },
            )
            .await
            .map_err(|_| anyhow!("VBA project summary failed"))?;
            if !summary
                .modules
                .iter()
                .any(|module| module.name == module_name)
            {
                bail!("invalid request: module_name does not name a VBA module in this project");
            }
            let response = tools::vba::vba_module_source(
                state,
                tools::vba::VbaModuleSourceParams {
                    workbook_or_fork_id: snapshot_id.clone().unwrap_or_else(|| resource_id.to_workbook_id()),
                    module_name: module_name.clone(),
                    offset_lines: offset,
                    limit_lines: limit,
                },
            )
            .await
            .map_err(|_| anyhow!("VBA module source read failed"))?;
            if response.source.len() > MAX_VBA_SOURCE_PAGE_BYTES {
                bail!(
                    "VBA source page exceeds the {MAX_VBA_SOURCE_PAGE_BYTES}-byte response limit"
                );
            }
            let returned_lines = response.source.lines().count() as u32;
            let next_offset = offset.saturating_add(returned_lines);
            Ok(InspectVbaData::ModuleSource {
                module_name,
                start_line: offset.saturating_add(1),
                returned_lines,
                source: response.source,
                next_cursor: response
                    .truncated
                    .then(|| encode_vba_cursor(revision, &fingerprint, next_offset))
                    .transpose()?,
            })
        }
    }
}
