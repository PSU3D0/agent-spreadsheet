#![cfg(feature = "recalc-formualizer")]
#[path = "support/source_xlsx.rs"]
mod fixture;
use agent_spreadsheet::recalc::source_xlsx::{
    ExternalLinkPolicy, IoError, XlsxRecalculateOptions, recalculate_source_xlsx_bytes,
};
use fixture::*;

#[test]
fn original_source_repairs_cache_preserves_members_and_becomes_exact_noop() {
    let mut parts = stale_parts();
    let sheet = parts.get_mut(SHEET).unwrap();
    *sheet = sheet.replace("</row>", "<c r=\"C1\"><v>-0</v></c></row>");
    let input = pack(&parts);
    let original = input.clone();
    let result = recalculate_source_xlsx_bytes(&input, Default::default()).unwrap();
    assert_eq!(input, original);
    assert!(member(&result.bytes, SHEET).contains("<f>A1*2</f><v>42</v>"));
    assert!(member(&result.bytes, SHEET).contains("<c r=\"C1\"><v>-0</v></c>"));
    assert_eq!(result.formula_cells, 1);
    assert_eq!(result.cache_cells_changed, 1);
    assert_eq!(result.worksheet_parts_changed, 1);
    assert_eq!(result.summary.evaluated, 1);
    assert_eq!(result.summary.errors, 0);
    for (name, body) in &parts {
        if name != SHEET {
            assert_eq!(member(&result.bytes, name), *body, "{name}");
        }
    }
    let noop = recalculate_source_xlsx_bytes(&result.bytes, Default::default()).unwrap();
    assert_eq!(noop.bytes, result.bytes);
    assert_eq!(noop.cache_cells_changed, 0);
    assert_eq!(noop.worksheet_parts_changed, 0);
}

#[test]
fn exact_error_totals_are_independent_of_location_sampling() {
    let options = XlsxRecalculateOptions {
        error_location_limit: 3,
        ..Default::default()
    };
    let result = recalculate_source_xlsx_bytes(&errors(201), options).unwrap();
    assert_eq!(result.summary.status.as_str(), "errors_found");
    assert_eq!(result.summary.evaluated, 201);
    assert_eq!(result.summary.errors, 201);
    assert_eq!(result.summary.sheets["Sheet1"].errors, 201);
    let error = &result.summary.error_summary["#DIV/0!"];
    assert_eq!(error.count, 201);
    assert_eq!(error.locations.len(), 3);
    assert_eq!(error.messages.len(), 3);
    assert_eq!(error.locations_truncated, 198);
}

#[test]
fn refusal_parse_failure_and_precancellation_return_no_result() {
    let mut signed = stale_parts();
    signed.insert("_xmlsignatures/sig1.xml".into(), "<Signature/>".into());
    assert!(matches!(
        recalculate_source_xlsx_bytes(&pack(&signed), Default::default()),
        Err(IoError::Unsupported { .. })
    ));
    let malformed = pack(&parts(
        "<row r=\"1\"><c r=\"A1\"><f>SUM(</f><v>99</v></c></row>",
    ));
    assert!(recalculate_source_xlsx_bytes(&malformed, Default::default()).is_err());
    let cancel = formualizer::eval::engine::CancelToken::new();
    cancel.cancel();
    let options = XlsxRecalculateOptions {
        cancel: Some(cancel),
        ..Default::default()
    };
    assert!(recalculate_source_xlsx_bytes(&pack(&stale_parts()), options).is_err());
}

#[test]
fn configured_limits_can_refuse_and_be_raised() {
    let input = pack(&stale_parts());
    let mut options = XlsxRecalculateOptions::default();
    options.limits.max_input_bytes = input.len() - 1;
    assert!(recalculate_source_xlsx_bytes(&input, options.clone()).is_err());
    options.limits.max_input_bytes = input.len();
    options.limits.max_formula_cells = 0;
    assert!(recalculate_source_xlsx_bytes(&input, options.clone()).is_err());
    options.limits.max_formula_cells = 1;
    let result = recalculate_source_xlsx_bytes(&input, options.clone()).unwrap();
    options.limits.max_output_bytes = 1;
    assert!(recalculate_source_xlsx_bytes(&input, options.clone()).is_err());
    options.limits.max_output_bytes = result.bytes.len();
    assert!(recalculate_source_xlsx_bytes(&input, options).is_ok());
}

#[test]
fn cached_external_values_are_reported_or_refused_without_refresh() {
    let input = external();
    let result = recalculate_source_xlsx_bytes(&input, Default::default()).unwrap();
    assert_eq!(result.external_links_used, 1);
    assert!(member(&result.bytes, SHEET).contains("<v>42</v>"));
    assert_eq!(
        member(&result.bytes, "xl/externalLinks/externalLink1.xml"),
        member(&input, "xl/externalLinks/externalLink1.xml")
    );
    let options = XlsxRecalculateOptions {
        external_links: ExternalLinkPolicy::Refuse,
        ..Default::default()
    };
    assert!(matches!(recalculate_source_xlsx_bytes(&input, options),
        Err(IoError::Unsupported { feature, .. }) if feature == "external link values"));
}

#[test]
fn native_eval_config_clock_is_passed_through() {
    use formualizer::eval::{engine::DeterministicMode, timezone::TimeZoneSpec};
    let instant = chrono::DateTime::parse_from_rfc3339("2025-01-02T03:04:05Z")
        .unwrap()
        .to_utc();
    let mut options = XlsxRecalculateOptions::default();
    options.eval_config.deterministic_mode = DeterministicMode::Enabled {
        timestamp_utc: instant,
        timezone: TimeZoneSpec::Utc,
    };
    let input = pack(&parts(
        "<row r=\"1\"><c r=\"A1\"><f>NOW()</f><v>0</v></c></row>",
    ));
    let result = recalculate_source_xlsx_bytes(&input, options.clone()).unwrap();
    assert_eq!(result.clock_now_utc, Some(instant));
    let again = recalculate_source_xlsx_bytes(&result.bytes, options).unwrap();
    assert_eq!(again.bytes, result.bytes);
}
