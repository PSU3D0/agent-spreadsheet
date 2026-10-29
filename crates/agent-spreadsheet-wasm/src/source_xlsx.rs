//! Direct source-byte recalc, independent of session storage and materialization.
use crate::{MAX_PARAMS_JSON_BYTES, MAX_WORKBOOK_BYTES, SessionApi};
use agent_spreadsheet::recalc::source_xlsx::{
    CalculationDeferredReason, ExternalLinkPolicy, IoError, ResolvedSourceCalculation,
    SourceCalculationCoverage, SourceCalculationPolicy, SourceCoverageScope,
    SourceOperationDiagnostics, XlsxRecalculateOptions, XlsxRecalculateProfile,
    XlsxRecalculateResult, recalculate_source_xlsx_bytes,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// JS-facing portable source policy; file-facing defaults select workbook.
/// Raw EvalConfig and cancellation are native-only. Unknown fields are errors.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SourceXlsxOptions {
    pub physical_telemetry: bool,
    pub profile: SourceProfile,
    pub calculation: Option<SourceCalculationPolicy>,
    pub require_complete: bool,
    pub limits: SourceXlsxLimits,
    pub error_location_limit: Option<usize>,
    pub external_links: Option<SourceExternalLinks>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProfile {
    Strict,
    #[default]
    Workbook,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceExternalLinks {
    Cached,
    Refuse,
}

/// Canonical partial limits: omissions preserve the selected profile's bounds.
pub use agent_spreadsheet::recalc::source_xlsx::SourceRecalculateLimits as SourceXlsxLimits;

impl SourceXlsxOptions {
    pub(crate) fn into_upstream(self) -> Result<XlsxRecalculateOptions, SourceXlsxError> {
        let mut options = match self.profile {
            SourceProfile::Strict => XlsxRecalculateOptions::default(),
            SourceProfile::Workbook => XlsxRecalculateOptions::workbook(),
        };
        options.require_complete = self.require_complete;
        if let Some(policy) = self.calculation {
            options.calculation = Some(policy);
        }
        if let Some(policy) = &options.calculation {
            policy
                .validate()
                .map_err(|error| SourceXlsxError::InvalidOptions(error.to_string()))?;
        }
        let mut limits = options.limits.clone();
        self.limits.apply(&mut limits)?;
        options.physical_telemetry = self.physical_telemetry;
        // Preserve the adapter host ceiling before upstream constructs output.
        // A raised source limit cannot bypass this existing adapter guard.
        limits.max_output_bytes = limits.max_output_bytes.min(MAX_WORKBOOK_BYTES);
        options.limits = limits;
        if let Some(limit) = self.error_location_limit {
            options.error_location_limit = limit;
        }
        if let Some(policy) = self.external_links {
            options.external_links = match policy {
                SourceExternalLinks::Cached => ExternalLinkPolicy::Cached,
                SourceExternalLinks::Refuse => ExternalLinkPolicy::Refuse,
            };
        }
        Ok(options)
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
            Self::Recalculate(IoError::InvalidOptions { .. }) => "INVALID_ARGUMENT",
            Self::Recalculate(IoError::CalculationDeferred { .. }) => "CALCULATION_DEFERRED",
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
    pub summary: Option<SourceXlsxSummary>,
    pub formula_cells: Option<usize>,
    /// Physical cache patches, not engine dirty deltas.
    pub cache_cells_changed: Option<usize>,
    pub worksheet_parts_changed: Option<usize>,
    /// RFC3339 UTC timestamp from upstream; null if upstream supplies none.
    pub clock_now_utc: Option<String>,
    pub external_links_used: Option<usize>,
    pub calculation: Option<ResolvedSourceCalculation>,
    pub coverage: SourceArtifactCoverage,
    pub coverage_scope: SourceCoverageScope,
    pub caches_invalidated: Option<usize>,
    /// Known only after source publication; skipped/deferred artifacts do not publish flag edits.
    pub calculation_flags_changed: Option<bool>,
    pub operation_diagnostics: Option<SourceOperationDiagnostics>,
    pub attempted: bool,
    pub completed: bool,
    pub artifact_available: bool,
    pub bytes_unchanged: bool,
    pub deferred_reason: Option<&'static str>,
    pub deferred_context: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum SourceArtifactCoverage {
    Calculated(SourceCalculationCoverage),
    Deferred(DeferredCoverage),
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeferredCoverage {
    Deferred,
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
                summary: Some(SourceXlsxSummary {
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
                }),
                formula_cells: Some(result.formula_cells),
                cache_cells_changed: Some(result.cache_cells_changed),
                worksheet_parts_changed: Some(result.worksheet_parts_changed),
                clock_now_utc: result.clock_now_utc.map(|clock| clock.to_rfc3339()),
                external_links_used: Some(result.external_links_used),
                calculation: result.calculation,
                coverage: SourceArtifactCoverage::Calculated(result.coverage),
                coverage_scope: result.coverage_scope,
                caches_invalidated: Some(result.caches_invalidated),
                calculation_flags_changed: Some(result.calculation_flags_changed),
                operation_diagnostics: Some(result.operation_diagnostics),
                attempted: true,
                completed: true,
                artifact_available: true,
                // Set by the entrypoint against its original input, never inferred from patch counts.
                bytes_unchanged: false,
                deferred_reason: None,
                deferred_context: None,
            },
        }
    }
}

impl SourceXlsxOutput {
    pub(crate) fn deferred(
        bytes: Vec<u8>,
        reason: CalculationDeferredReason,
        context: String,
    ) -> Self {
        Self {
            bytes,
            metadata: SourceXlsxMetadata {
                summary: None,
                formula_cells: None,
                cache_cells_changed: None,
                worksheet_parts_changed: None,
                clock_now_utc: None,
                external_links_used: None,
                calculation: None,
                coverage: SourceArtifactCoverage::Deferred(DeferredCoverage::Deferred),
                coverage_scope: SourceCoverageScope::None,
                caches_invalidated: None,
                calculation_flags_changed: None,
                operation_diagnostics: None,
                attempted: true,
                completed: false,
                artifact_available: true,
                bytes_unchanged: false,
                deferred_reason: Some(match reason {
                    CalculationDeferredReason::IterativeInitialStateUnverified => {
                        "iterative_initial_state_unverified"
                    }
                    CalculationDeferredReason::KnownCoverageIncomplete => {
                        "known_coverage_incomplete"
                    }
                    CalculationDeferredReason::WorkbookPrecisionUnverified => {
                        "workbook_precision_unverified"
                    }
                    CalculationDeferredReason::DependencyCoverageUnavailable => {
                        "dependency_coverage_unavailable"
                    }
                    CalculationDeferredReason::UnlocatedOperation => "unlocated_operation",
                    CalculationDeferredReason::AffectedArrayCoverageUnverified => {
                        "affected_array_coverage_unverified"
                    }
                }),
                deferred_context: Some(context),
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
        for requested in [0, 1, ceiling - 1, ceiling, ceiling + 1, u32::MAX as usize] {
            let options = SourceXlsxOptions {
                limits: SourceXlsxLimits {
                    max_output_bytes: Some(requested as u64),
                    ..Default::default()
                },
                ..Default::default()
            }
            .into_upstream()
            .unwrap();
            assert_eq!(options.limits.max_output_bytes, requested.min(ceiling));
        }
        assert!(
            SourceXlsxOptions::default()
                .into_upstream()
                .unwrap()
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
        let options = options.into_upstream()?;
        let allow_deferred =
            options.profile == XlsxRecalculateProfile::Workbook && !options.require_complete;
        let output_limit = options.limits.max_output_bytes;
        let mut result = match recalculate_source_xlsx_bytes(source, options) {
            Ok(result) => SourceXlsxOutput::from(result),
            Err(IoError::CalculationDeferred { reason, context }) if allow_deferred => {
                SourceXlsxOutput::deferred(source.to_vec(), reason, context)
            }
            Err(error) => return Err(error.into()),
        };
        if result.bytes.len() > output_limit {
            return Err(SourceXlsxError::Recalculate(IoError::LoadBudgetExceeded {
                backend: "xlsx".into(),
                sheet: String::new(),
                message: "artifact exceeds maxOutputBytes".into(),
            }));
        }
        result.metadata.bytes_unchanged = result.bytes == source;
        if result.bytes.len() > MAX_WORKBOOK_BYTES {
            return Err(SourceXlsxError::HostLimit {
                direction: "output",
            });
        }
        Ok(result)
    }
}
