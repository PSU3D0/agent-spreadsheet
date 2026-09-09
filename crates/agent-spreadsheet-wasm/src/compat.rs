use agent_spreadsheet::core::session::{
    SessionFindValueParams, SessionRangeSelection, SessionReadTableParams,
    SessionSheetOverviewParams, SessionSheetPageParams,
};
use agent_spreadsheet::model::{RangeValuesEntry, SheetPageFormat, TableOutputFormat};
use serde::{Deserialize, Serialize};

pub const MAX_WORKBOOK_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PARAMS_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_SESSIONS: usize = 16;
/// Rendered artifacts a single session may hold at once. Slots are the only
/// place image bytes live in this adapter, so they are capped twice: by count
/// per session and by aggregate bytes across every session.
pub const MAX_ARTIFACTS_PER_SESSION: usize = 8;
pub const MAX_TOTAL_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SessionApiError {
    #[error("session '{session_id}' not found")]
    SessionNotFound { session_id: String },
    #[error("invalid argument: {message}")]
    InvalidArgument { message: String },
    #[error("unsupported in wasm mvp: {message}")]
    Unsupported { message: String },
    #[error("internal error: {message}")]
    Internal { message: String },
}

impl SessionApiError {
    pub fn code(&self) -> &'static str {
        match self {
            SessionApiError::SessionNotFound { .. } => "SESSION_NOT_FOUND",
            SessionApiError::InvalidArgument { .. } => "INVALID_ARGUMENT",
            SessionApiError::Unsupported { .. } => "UNSUPPORTED",
            SessionApiError::Internal { .. } => "INTERNAL",
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }
}

pub type SessionResult<T> = Result<T, SessionApiError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionApiErrorPayload {
    pub code: String,
    pub message: String,
}

impl From<SessionApiError> for SessionApiErrorPayload {
    fn from(value: SessionApiError) -> Self {
        Self {
            code: value.code().to_string(),
            message: value.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RangeSelectionInput {
    Single(String),
    Multi(Vec<String>),
}

impl From<RangeSelectionInput> for SessionRangeSelection {
    fn from(value: RangeSelectionInput) -> Self {
        match value {
            RangeSelectionInput::Single(range) => SessionRangeSelection::Single(range),
            RangeSelectionInput::Multi(ranges) => SessionRangeSelection::Multi(ranges),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RangeValuesParams {
    #[serde(alias = "sheet_name")]
    pub sheet_name: String,
    pub ranges: RangeSelectionInput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RangeValuesResult {
    pub sheet_name: String,
    pub values: Vec<RangeValuesEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridExportParams {
    #[serde(alias = "sheet_name")]
    pub sheet_name: String,
    pub range: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TransformBatchOptions {
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetOverviewParams {
    #[serde(alias = "sheet_name")]
    pub sheet_name: String,
    #[serde(default)]
    pub max_regions: Option<u32>,
    #[serde(default)]
    pub max_headers: Option<u32>,
    #[serde(default)]
    pub include_headers: Option<bool>,
}

impl From<SheetOverviewParams> for SessionSheetOverviewParams {
    fn from(value: SheetOverviewParams) -> Self {
        SessionSheetOverviewParams {
            sheet_name: value.sheet_name,
            max_regions: value.max_regions,
            max_headers: value.max_headers,
            include_headers: value.include_headers,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindValueParams {
    pub query: String,
    #[serde(default, alias = "sheet_name")]
    pub sheet_name: Option<String>,
    #[serde(default)]
    pub case_sensitive: Option<bool>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
}

impl From<FindValueParams> for SessionFindValueParams {
    fn from(value: FindValueParams) -> Self {
        SessionFindValueParams {
            query: value.query,
            sheet_name: value.sheet_name,
            case_sensitive: value.case_sensitive.unwrap_or(false),
            limit: value.limit.unwrap_or(50),
            offset: value.offset,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadTableParams {
    #[serde(default, alias = "sheet_name")]
    pub sheet_name: Option<String>,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub columns: Option<Vec<String>>,
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

impl From<ReadTableParams> for SessionReadTableParams {
    fn from(value: ReadTableParams) -> Self {
        SessionReadTableParams {
            sheet_name: value.sheet_name,
            range: value.range,
            columns: value.columns,
            limit: value.limit.unwrap_or(100),
            offset: value.offset,
            format: value.format.unwrap_or(TableOutputFormat::Csv),
            include_headers: value.include_headers.unwrap_or(true),
            include_types: value.include_types.unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetPageParams {
    #[serde(alias = "sheet_name")]
    pub sheet_name: String,
    #[serde(default)]
    pub start_row: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub columns: Option<Vec<String>>,
    #[serde(default)]
    pub columns_by_header: Option<Vec<String>>,
    #[serde(default)]
    pub include_formulas: Option<bool>,
    #[serde(default)]
    pub include_styles: Option<bool>,
    #[serde(default)]
    pub include_header: Option<bool>,
    #[serde(default)]
    pub format: Option<SheetPageFormat>,
}

impl From<SheetPageParams> for SessionSheetPageParams {
    fn from(value: SheetPageParams) -> Self {
        SessionSheetPageParams {
            sheet_name: value.sheet_name,
            start_row: value.start_row.unwrap_or(1),
            page_size: value.page_size.unwrap_or(50),
            columns: value.columns,
            columns_by_header: value.columns_by_header,
            include_formulas: value.include_formulas.unwrap_or(true),
            include_styles: value.include_styles.unwrap_or(false),
            include_header: value.include_header.unwrap_or(true),
            format: value.format.unwrap_or_default(),
        }
    }
}
