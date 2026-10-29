//! Stateless cache repair of an original XLSX package.
//!
//! This path never imports a rich document or creates a resident evaluator. It
//! keeps upstream admission/refusal semantics, separately from warm editing.
pub use formualizer::workbook::{
    CalculationDeferredReason, ExternalLinkPolicy, IoError, ResolvedSourceCalculation,
    SourceCalculationCoverage, SourceCalculationPolicy, SourceCoverageScope,
    SourceOperationDiagnostics, SourceRecalculateLimits, XlsxRecalculateLimits,
    XlsxRecalculateOptions, XlsxRecalculateProfile, XlsxRecalculateResult,
};

/// Recalculate original source bytes, preserving untouched package members.
///
/// A successful Excel error is represented in the exact upstream summary.
/// Admission, parse, unsupported-input and cancellation failures return `Err`
/// without result bytes. The caller owns publication: this function performs
/// no filesystem writes or revision/CAS checks.
///
/// All upstream limits, sampling, external-link, evaluation configuration and
/// cancellation options pass through unchanged. Current packages are returned
/// byte-for-byte when no cache changes are necessary.
pub fn recalculate_source_xlsx_bytes(
    source: &[u8],
    options: XlsxRecalculateOptions,
) -> Result<XlsxRecalculateResult, IoError> {
    formualizer::recalculate_xlsx_bytes(source, options)
}
