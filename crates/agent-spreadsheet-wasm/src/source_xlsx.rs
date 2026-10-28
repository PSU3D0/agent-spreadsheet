//! Direct source-byte recalc, independent of session storage and materialization.
use crate::{MAX_PARAMS_JSON_BYTES, MAX_WORKBOOK_BYTES, SessionApi};
use agent_spreadsheet::recalc::source_xlsx::{
    ExternalLinkPolicy, IoError, XlsxRecalculateLimits, XlsxRecalculateOptions,
    XlsxRecalculateResult, recalculate_source_xlsx_bytes,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// JS-facing options intentionally omit EvalConfig and cancellation. Omitted
/// fields use upstream defaults. Unknown fields (including unsupported runtime
/// options) are errors, never silently ignored.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SourceXlsxOptions {
    pub limits: SourceXlsxLimits,
    pub error_location_limit: Option<usize>,
    pub external_links: Option<SourceExternalLinks>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceExternalLinks {
    Cached,
    Refuse,
}

/// Every upstream admission/output bound can be overridden. The adapter also
/// enforces its documented 64 MiB host input/output bound independently.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SourceXlsxLimits {
    pub max_input_bytes: Option<usize>,
    pub max_entries: Option<usize>,
    pub max_expanded_bytes: Option<usize>,
    pub max_worksheet_bytes: Option<usize>,
    pub max_formula_cells: Option<usize>,
    pub max_output_bytes: Option<usize>,
    pub max_xml_depth: Option<usize>,
    pub max_cells: Option<usize>,
    pub max_columns: Option<u32>,
}

impl SourceXlsxOptions {
    pub(crate) fn into_upstream(self) -> XlsxRecalculateOptions {
        let mut limits = XlsxRecalculateLimits::default();
        macro_rules! apply {
            ($($field:ident),* $(,)?) => {
                $(if let Some(value) = self.limits.$field { limits.$field = value; })*
            };
        }
        apply!(
            max_input_bytes,
            max_entries,
            max_expanded_bytes,
            max_worksheet_bytes,
            max_formula_cells,
            max_output_bytes,
            max_xml_depth,
            max_cells,
            max_columns
        );
        // Preserve the adapter host ceiling before upstream constructs output.
        // A raised source limit cannot bypass this existing adapter guard.
        limits.max_output_bytes = limits.max_output_bytes.min(MAX_WORKBOOK_BYTES);
        let mut options = XlsxRecalculateOptions {
            limits,
            ..Default::default()
        };
        if let Some(limit) = self.error_location_limit {
            options.error_location_limit = limit;
        }
        if let Some(policy) = self.external_links {
            options.external_links = match policy {
                SourceExternalLinks::Cached => ExternalLinkPolicy::Cached,
                SourceExternalLinks::Refuse => ExternalLinkPolicy::Refuse,
            };
        }
        options
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SourceXlsxError {
    #[error("invalid source XLSX options: {0}")]
    InvalidOptions(String),
    #[error("source XLSX {direction} exceeds the {MAX_WORKBOOK_BYTES}-byte adapter host limit")]
    HostLimit { direction: &'static str },
    #[error(transparent)]
    Recalculate(#[from] IoError),
}

impl SourceXlsxError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidOptions(_) => "INVALID_ARGUMENT",
            Self::HostLimit { .. } => "HOST_LIMIT_EXCEEDED",
            Self::Recalculate(IoError::Unsupported { .. }) => "UNSUPPORTED",
            Self::Recalculate(IoError::FormulaParser { .. }) => "FORMULA_PARSE_ERROR",
            Self::Recalculate(IoError::LoadBudgetExceeded { .. }) => "LIMIT_EXCEEDED",
            Self::Recalculate(IoError::Engine(_)) => "ENGINE_ERROR",
            Self::Recalculate(_) => "RECALCULATION_FAILED",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceXlsxSheetSummary {
    pub evaluated: usize,
    pub errors: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceXlsxErrorSummary {
    pub count: usize,
    pub locations: Vec<String>,
    pub messages: Vec<Option<String>>,
    pub locations_truncated: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceXlsxSummary {
    pub status: String,
    pub evaluated: usize,
    pub errors: usize,
    pub sheets: BTreeMap<String, SourceXlsxSheetSummary>,
    pub error_summary: BTreeMap<String, SourceXlsxErrorSummary>,
    pub unknown_functions: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceXlsxMetadata {
    pub summary: SourceXlsxSummary,
    pub formula_cells: usize,
    /// Physical cache patches, not engine dirty deltas.
    pub cache_cells_changed: usize,
    pub worksheet_parts_changed: usize,
    /// RFC3339 UTC timestamp from upstream; null if upstream supplies none.
    pub clock_now_utc: Option<String>,
    pub external_links_used: usize,
}

/// Deliberately not Serialize: workbook bytes travel separately from metadata.
#[derive(Debug)]
pub struct SourceXlsxOutput {
    pub bytes: Vec<u8>,
    pub metadata: SourceXlsxMetadata,
}

impl From<XlsxRecalculateResult> for SourceXlsxOutput {
    fn from(result: XlsxRecalculateResult) -> Self {
        let summary = result.summary;
        Self {
            bytes: result.bytes,
            metadata: SourceXlsxMetadata {
                summary: SourceXlsxSummary {
                    status: summary.status.as_str().to_owned(),
                    evaluated: summary.evaluated,
                    errors: summary.errors,
                    sheets: summary
                        .sheets
                        .into_iter()
                        .map(|(name, stats)| {
                            (
                                name,
                                SourceXlsxSheetSummary {
                                    evaluated: stats.evaluated,
                                    errors: stats.errors,
                                },
                            )
                        })
                        .collect(),
                    error_summary: summary
                        .error_summary
                        .into_iter()
                        .map(|(kind, stats)| {
                            (
                                kind,
                                SourceXlsxErrorSummary {
                                    count: stats.count,
                                    locations: stats.locations,
                                    messages: stats.messages,
                                    locations_truncated: stats.locations_truncated,
                                },
                            )
                        })
                        .collect(),
                    unknown_functions: summary.unknown_functions,
                },
                formula_cells: result.formula_cells,
                cache_cells_changed: result.cache_cells_changed,
                worksheet_parts_changed: result.worksheet_parts_changed,
                clock_now_utc: result.clock_now_utc.map(|clock| clock.to_rfc3339()),
                external_links_used: result.external_links_used,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_output_ceiling_is_applied_before_upstream_construction() {
        let ceiling = MAX_WORKBOOK_BYTES;
        for requested in [0, 1, ceiling - 1, ceiling, ceiling + 1, usize::MAX] {
            let options = SourceXlsxOptions {
                limits: SourceXlsxLimits {
                    max_output_bytes: Some(requested),
                    ..Default::default()
                },
                ..Default::default()
            }
            .into_upstream();
            assert_eq!(options.limits.max_output_bytes, requested.min(ceiling));
        }
        assert!(
            SourceXlsxOptions::default()
                .into_upstream()
                .limits
                .max_output_bytes
                <= ceiling
        );
    }
}

impl SessionApi {
    /// Repair original source bytes directly without creating or modifying a
    /// session. This operation is synchronous and never publishes on failure.
    pub fn recalculate_source_xlsx(
        &self,
        source: &[u8],
        options_json: Option<&str>,
    ) -> Result<SourceXlsxOutput, SourceXlsxError> {
        if source.len() > MAX_WORKBOOK_BYTES {
            return Err(SourceXlsxError::HostLimit { direction: "input" });
        }
        let options = match options_json {
            Some(json) => {
                if json.len() > MAX_PARAMS_JSON_BYTES {
                    return Err(SourceXlsxError::InvalidOptions(format!(
                        "options exceed the {MAX_PARAMS_JSON_BYTES}-byte adapter JSON limit"
                    )));
                }
                serde_json::from_str::<SourceXlsxOptions>(json)
                    .map_err(|error| SourceXlsxError::InvalidOptions(error.to_string()))?
            }
            None => SourceXlsxOptions::default(),
        };
        let result = recalculate_source_xlsx_bytes(source, options.into_upstream())?;
        if result.bytes.len() > MAX_WORKBOOK_BYTES {
            return Err(SourceXlsxError::HostLimit {
                direction: "output",
            });
        }
        Ok(result.into())
    }
}
