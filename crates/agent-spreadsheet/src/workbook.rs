use crate::analysis::{
    classification,
    formula::{FormulaAtlas, FormulaGraph},
    style,
};
use crate::caps::BackendCaps;
use crate::config::ServerConfig;
#[cfg(feature = "native-fs")]
use crate::model::WorkbookListResponse;
use crate::model::{
    FormulaParseDiagnostics, FormulaParseDiagnosticsBuilder, FormulaParsePolicy, NamedItemKind,
    NamedRangeDescriptor, NamedRangeScope, SheetClassification, SheetOverviewResponse,
    SheetSummary, WorkbookDescription, WorkbookId,
};
#[cfg(feature = "native-fs")]
use crate::tools::filters::WorkbookFilter;
use crate::utils::{
    hash_bytes_sha256_hex, hash_file_sha256_hex, hash_path_identity, make_short_workbook_id,
    path_to_forward_slashes, system_time_to_rfc3339,
};
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use regex::Regex;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, OnceLock};
use crate::xlsx_import as xlsx;
use umya_spreadsheet::{DefinedName, Workbook as Spreadsheet, Worksheet};
use web_time::Instant;

const KV_MAX_WIDTH_FOR_DENSITY_CHECK: u32 = 6;
const KV_SAMPLE_ROWS: u32 = 20;
const KV_DENSITY_THRESHOLD: f32 = 0.4;
const KV_CHECK_ROWS: u32 = 15;
const KV_MAX_LABEL_LEN: usize = 25;
const KV_MIN_TEXT_VALUE_LEN: usize = 2;
const KV_MIN_PAIRS: u32 = 3;
const KV_MIN_PAIR_RATIO: f32 = 0.3;

const HEADER_MAX_SCAN_ROWS: u32 = 2;
const HEADER_LONG_STRING_PENALTY_THRESHOLD: usize = 40;
const HEADER_LONG_STRING_PENALTY: f32 = 1.5;
const HEADER_PROPER_NOUN_MIN_LEN: usize = 5;
const HEADER_PROPER_NOUN_PENALTY: f32 = 1.0;
const HEADER_DIGIT_STRING_MIN_LEN: usize = 3;
const HEADER_DIGIT_STRING_PENALTY: f32 = 0.5;
const HEADER_DATE_PENALTY: f32 = 1.0;
const HEADER_YEAR_LIKE_BONUS: f32 = 0.5;
const HEADER_YEAR_MIN: f64 = 1900.0;
const HEADER_YEAR_MAX: f64 = 2100.0;
const HEADER_UNIQUE_BONUS: f32 = 0.2;
const HEADER_NUMBER_PENALTY: f32 = 0.3;
const HEADER_SINGLE_COL_MIN_SCORE: f32 = 1.5;
const HEADER_SCORE_TIE_THRESHOLD: f32 = 0.3;
const HEADER_SECOND_ROW_MIN_SCORE_RATIO: f32 = 0.6;
const HEADER_MAX_COLUMNS: u32 = 200;

const DETECT_MAX_ROWS: u32 = 10_000;
const DETECT_MAX_COLS: u32 = 500;
const DETECT_MAX_AREA: u64 = 5_000_000;
const DETECT_MAX_CELLS: usize = 200_000;
const DETECT_MAX_LEAVES: usize = 200;
const DETECT_MAX_DEPTH: u32 = 12;
const DETECT_MAX_MS: u64 = 200;
const DETECT_OUTLIER_FRACTION: f32 = 0.01;
const DETECT_OUTLIER_MIN_CELLS: usize = 50;

static OOXML_SHEET_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?s)<sheet\b([^>]*)/?>"#).unwrap());
static OOXML_REL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?s)<Relationship\b([^>]*)/?>"#).unwrap());
static OOXML_CELL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?s)<c\b([^>]*)>(.*?)</c>"#).unwrap());
static OOXML_TEXT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?s)<t(?:\s[^>]*)?>(.*?)</t>"#).unwrap());
static OOXML_R_ID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"\br:id="([^"]+)""#).unwrap());
static OOXML_ID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"\bId="([^"]+)""#).unwrap());
static OOXML_TARGET_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"\bTarget="([^"]+)""#).unwrap());
static OOXML_COORD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"\br="([^"]+)""#).unwrap());
static OOXML_TYPE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"\bt="([^"]+)""#).unwrap());

/// Read backing for the shared workbook projections. Resident callers borrow
/// their authority; file callers retain the historical owned read cache.
pub trait WorkbookReadSource {
    type Guard<'a>: std::ops::Deref<Target = Spreadsheet>
    where
        Self: 'a;
    fn read(&self) -> Self::Guard<'_>;
    fn artifact_bytes(&self, bytes: u64) -> Option<u64> {
        Some(bytes)
    }
}

impl WorkbookReadSource for Arc<RwLock<Spreadsheet>> {
    type Guard<'a> = parking_lot::RwLockReadGuard<'a, Spreadsheet>;
    fn read(&self) -> Self::Guard<'_> {
        self.as_ref().read()
    }
}

impl WorkbookReadSource for &Spreadsheet {
    type Guard<'a>
        = &'a Spreadsheet
    where
        Self: 'a;
    fn read(&self) -> Self::Guard<'_> {
        self
    }
    fn artifact_bytes(&self, _bytes: u64) -> Option<u64> {
        None
    }
}

pub struct WorkbookContext<S = Arc<RwLock<Spreadsheet>>> {
    pub id: WorkbookId,
    pub short_id: String,
    pub revision_id: String,
    pub slug: String,
    pub path: PathBuf,
    pub caps: BackendCaps,
    pub bytes: u64,
    pub last_modified: Option<DateTime<Utc>>,
    spreadsheet: S,
    sheet_cache: RwLock<HashMap<String, Arc<SheetCacheEntry>>>,
    formula_atlas: Arc<FormulaAtlas>,
    imported_evaluation_coverage: OnceLock<crate::model::EvaluationCoverage>,
    imported_evaluation_scans: AtomicU64,
}

pub struct SheetCacheEntry {
    pub metrics: SheetMetrics,
    pub style_tags: Vec<String>,
    pub named_ranges: Vec<NamedRangeDescriptor>,
    detected_regions: RwLock<Option<Vec<crate::model::DetectedRegion>>>,
    region_notes: RwLock<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct SheetMetrics {
    pub row_count: u32,
    pub column_count: u32,
    pub non_empty_cells: u32,
    pub formula_cells: u32,
    pub cached_values: u32,
    pub comments: u32,
    pub style_map: HashMap<String, StyleUsage>,
    pub classification: SheetClassification,
}

#[derive(Debug, Clone)]
pub struct StyleUsage {
    pub occurrences: u32,
    pub tags: Vec<String>,
    pub example_cells: Vec<String>,
}

impl SheetCacheEntry {
    pub fn detected_regions(&self) -> Vec<crate::model::DetectedRegion> {
        self.detected_regions
            .read()
            .as_ref()
            .cloned()
            .unwrap_or_default()
    }

    pub fn region_notes(&self) -> Vec<String> {
        self.region_notes.read().clone()
    }

    pub fn has_detected_regions(&self) -> bool {
        self.detected_regions.read().is_some()
    }

    pub fn set_detected_regions(&self, regions: Vec<crate::model::DetectedRegion>) {
        let mut guard = self.detected_regions.write();
        if guard.is_none() {
            *guard = Some(regions);
        }
    }

    pub fn set_region_notes(&self, notes: Vec<String>) {
        if notes.is_empty() {
            return;
        }
        let mut guard = self.region_notes.write();
        if guard.is_empty() {
            *guard = notes;
        }
    }
}

fn zip_text<R: Read + Seek>(archive: &mut zip::ZipArchive<R>, name: &str) -> Option<String> {
    let mut entry = archive.by_name(name).ok()?;
    let mut text = String::new();
    entry.read_to_string(&mut text).ok()?;
    Some(text)
}

fn relationship_target(target: &str) -> String {
    let target = target.trim_start_matches('/');
    if target.starts_with("xl/") {
        target.to_string()
    } else {
        format!("xl/{target}")
    }
}

fn restore_inline_error_text_types<R: Read + Seek>(
    spreadsheet: &mut Spreadsheet,
    archive: &mut zip::ZipArchive<R>,
) {
    let Some(workbook_xml) = zip_text(archive, "xl/workbook.xml") else {
        return;
    };
    let Some(relationships_xml) = zip_text(archive, "xl/_rels/workbook.xml.rels") else {
        return;
    };

    let relationships: HashMap<String, String> = OOXML_REL_RE
        .captures_iter(&relationships_xml)
        .filter_map(|captures| {
            let attributes = captures.get(1)?.as_str();
            let id = OOXML_ID_RE.captures(attributes)?.get(1)?.as_str();
            let target = OOXML_TARGET_RE.captures(attributes)?.get(1)?.as_str();
            Some((id.to_string(), relationship_target(target)))
        })
        .collect();
    let worksheet_paths: Vec<String> = OOXML_SHEET_RE
        .captures_iter(&workbook_xml)
        .filter_map(|captures| {
            let attributes = captures.get(1)?.as_str();
            let id = OOXML_R_ID_RE.captures(attributes)?.get(1)?.as_str();
            relationships.get(id).cloned()
        })
        .collect();

    for (sheet_index, worksheet_path) in worksheet_paths.iter().enumerate() {
        let Some(worksheet_xml) = zip_text(archive, worksheet_path) else {
            continue;
        };
        let Some(sheet) = spreadsheet.sheet_collection_mut().get_mut(sheet_index) else {
            continue;
        };
        for captures in OOXML_CELL_RE.captures_iter(&worksheet_xml) {
            let Some(attributes) = captures.get(1).map(|value| value.as_str()) else {
                continue;
            };
            let Some(body) = captures.get(2).map(|value| value.as_str()) else {
                continue;
            };
            let is_inline_string = OOXML_TYPE_RE
                .captures(attributes)
                .and_then(|capture| capture.get(1))
                .is_some_and(|value| value.as_str() == "inlineStr");
            if !is_inline_string || body.contains("<f") {
                continue;
            }
            let text = OOXML_TEXT_RE
                .captures_iter(body)
                .filter_map(|capture| capture.get(1).map(|value| value.as_str()))
                .collect::<String>();
            if !is_spreadsheet_error(&text) {
                continue;
            }
            if let Some(coordinate) = OOXML_COORD_RE
                .captures(attributes)
                .and_then(|capture| capture.get(1))
                .map(|value| value.as_str())
            {
                sheet.cell_mut(coordinate).set_value_string(text);
            }
        }
    }
}

impl WorkbookContext {
    pub fn load(_config: &Arc<ServerConfig>, path: &Path) -> Result<Self> {
        fs::metadata(path).with_context(|| format!("unable to read metadata for {:?}", path))?;
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let slug = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "workbook".to_string());
        let id = WorkbookId(hash_path_identity(&canonical));
        let short_id = make_short_workbook_id(&slug, id.as_str());
        let revision_id = hash_file_sha256_hex(path)
            .with_context(|| format!("unable to hash workbook {:?}", path))?;

        Self::load_from_path(_config, path, id, short_id, Some(revision_id))
    }

    pub fn load_from_path(
        _config: &Arc<ServerConfig>,
        path: &Path,
        stable_id: WorkbookId,
        short_id: String,
        revision_id: Option<String>,
    ) -> Result<Self> {
        let metadata = fs::metadata(path)
            .with_context(|| format!("unable to read metadata for {:?}", path))?;
        let slug = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "workbook".to_string());
        let bytes = metadata.len();
        let last_modified = metadata.modified().ok().and_then(system_time_to_rfc3339);
        let revision_id = match revision_id {
            Some(id) => id,
            None => hash_file_sha256_hex(path)
                .with_context(|| format!("unable to hash workbook {:?}", path))?,
        };
        let mut spreadsheet =
            xlsx::read(path).with_context(|| format!("failed to parse workbook {:?}", path))?;
        if let Ok(file) = File::open(path)
            && let Ok(mut archive) = zip::ZipArchive::new(file)
        {
            restore_inline_error_text_types(&mut spreadsheet, &mut archive);
        }

        Ok(Self {
            id: stable_id,
            short_id,
            revision_id,
            slug,
            path: path.to_path_buf(),
            caps: BackendCaps::xlsx(),
            bytes,
            last_modified,
            spreadsheet: Arc::new(RwLock::new(spreadsheet)),
            sheet_cache: RwLock::new(HashMap::new()),
            formula_atlas: Arc::new(FormulaAtlas::default()),
            imported_evaluation_coverage: OnceLock::new(),
            imported_evaluation_scans: AtomicU64::new(0),
        })
    }

    pub fn load_from_bytes(
        _config: &Arc<ServerConfig>,
        display_name: &str,
        bytes: &[u8],
        stable_id: WorkbookId,
        short_id: String,
        revision_id: Option<String>,
    ) -> Result<Self> {
        let slug = Path::new(display_name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "workbook".to_string());
        let cursor = Cursor::new(bytes);
        let mut spreadsheet = xlsx::read_reader(cursor, true)
            .with_context(|| format!("failed to parse workbook bytes for {display_name}"))?;
        if let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(bytes)) {
            restore_inline_error_text_types(&mut spreadsheet, &mut archive);
        }
        let revision_id = revision_id.unwrap_or_else(|| hash_bytes_sha256_hex(bytes));

        Ok(Self {
            id: stable_id,
            short_id,
            revision_id,
            slug,
            path: PathBuf::from(format!("virtual/{display_name}")),
            caps: BackendCaps::xlsx(),
            bytes: bytes.len() as u64,
            last_modified: None,
            spreadsheet: Arc::new(RwLock::new(spreadsheet)),
            sheet_cache: RwLock::new(HashMap::new()),
            formula_atlas: Arc::new(FormulaAtlas::default()),
            imported_evaluation_coverage: OnceLock::new(),
            imported_evaluation_scans: AtomicU64::new(0),
        })
    }
}

#[cfg(test)]
mod borrowed_view_tests {
    use super::*;

    #[test]
    fn borrowed_context_projects_the_exact_authority_without_copying() {
        let mut document = umya_spreadsheet::new_file();
        document
            .sheet_by_name_mut("Sheet1").ok()
            .unwrap()
            .cell_mut("A1")
            .set_value_number(7.0);
        let view = WorkbookContext::borrowed(
            &document,
            WorkbookId("session_test".into()),
            "resident:test:0:0".into(),
        );
        assert!(
            view.with_spreadsheet(|book| std::ptr::eq(book, &document))
                .unwrap()
        );
        assert_eq!(view.sheet_names(), vec!["Sheet1"]);
        assert_eq!(
            view.describe().revision_id.as_deref(),
            Some("resident:test:0:0")
        );
        assert_eq!(
            view.list_summaries(true).unwrap()[0].non_empty_cells,
            Some(1)
        );
        assert_eq!(
            view.with_sheet("Sheet1", |sheet| sheet
                .cell("A1")
                .unwrap()
                .value()
                .to_string())
                .unwrap(),
            "7"
        );
        assert!(view.named_items().unwrap().is_empty());
    }
}

impl<'a> WorkbookContext<&'a Spreadsheet> {
    pub fn borrowed(spreadsheet: &'a Spreadsheet, id: WorkbookId, revision_id: String) -> Self {
        Self {
            short_id: make_short_workbook_id("session", id.as_str()),
            id,
            revision_id,
            slug: "session".into(),
            path: PathBuf::from("virtual/session.xlsx"),
            caps: BackendCaps::xlsx(),
            bytes: 0,
            last_modified: None,
            spreadsheet,
            sheet_cache: RwLock::new(HashMap::new()),
            formula_atlas: Arc::new(FormulaAtlas::default()),
            imported_evaluation_coverage: OnceLock::new(),
            imported_evaluation_scans: AtomicU64::new(0),
        }
    }
}

impl<S: WorkbookReadSource> WorkbookContext<S> {
    #[cfg(feature = "recalc")]
    pub(crate) fn with_evaluation_coverage(
        self,
        coverage: crate::model::EvaluationCoverage,
    ) -> Self {
        let _ = self.imported_evaluation_coverage.set(coverage);
        self
    }

    pub fn sheet_names(&self) -> Vec<String> {
        let book = self.spreadsheet.read();
        book.sheet_collection()
            .iter()
            .map(|sheet| sheet.name().to_string())
            .collect()
    }

    pub fn save_copy(&self, path: &Path) -> Result<()> {
        let book = self.spreadsheet.read();
        crate::xlsx_export::write(&book, path)
            .with_context(|| format!("failed to write workbook copy {:?}", path))
    }

    pub fn imported_evaluation_coverage(&self) -> crate::model::EvaluationCoverage {
        self.imported_evaluation_coverage
            .get_or_init(|| {
                self.imported_evaluation_scans
                    .fetch_add(1, AtomicOrdering::Relaxed);
                let book = self.spreadsheet.read();
                let mut formula_cells = 0u64;
                let mut cached_formula_cells = 0u64;
                let mut error_formula_cells = 0u64;

                for sheet in book.sheet_collection() {
                    for cell in sheet.cells() {
                        if !cell.is_formula() {
                            continue;
                        }
                        formula_cells += 1;
                        let value = cell.value();
                        if !value.is_empty() {
                            cached_formula_cells += 1;
                        }
                        if is_spreadsheet_error(&value) {
                            error_formula_cells += 1;
                        }
                    }
                }

                crate::model::EvaluationCoverage {
                    formula_cells,
                    evaluated_formula_cells: cached_formula_cells,
                    unsupported_formula_cells: 0,
                    error_formula_cells,
                    source: if cached_formula_cells > 0 {
                        crate::model::EvaluationSource::TrustedCache
                    } else {
                        crate::model::EvaluationSource::None
                    },
                    freshness: if formula_cells == 0 {
                        crate::model::EvaluationFreshness::CurrentRevision
                    } else {
                        crate::model::EvaluationFreshness::Unknown
                    },
                    revision_id: self.revision_id.clone(),
                }
            })
            .clone()
    }

    #[doc(hidden)]
    pub fn imported_evaluation_scan_count(&self) -> u64 {
        self.imported_evaluation_scans.load(AtomicOrdering::Relaxed)
    }

    pub fn calculation_metadata(&self) -> crate::model::CalculationMetadata {
        let coverage = self.imported_evaluation_coverage();
        crate::model::CalculationMetadata {
            state: coverage.state(),
            revision_id: self.revision_id.clone(),
        }
    }

    pub fn describe(&self) -> WorkbookDescription {
        let book = self.spreadsheet.read();
        let defined_names_count = book.defined_names().len();
        let table_count: usize = book
            .sheet_collection()
            .iter()
            .map(|sheet| sheet.tables().len())
            .sum();
        let macros_present = false;

        WorkbookDescription {
            workbook_id: self.id.clone(),
            short_id: self.short_id.clone(),
            slug: self.slug.clone(),
            path: path_to_forward_slashes(&self.path),
            client_path: None,
            bytes: self.spreadsheet.artifact_bytes(self.bytes),
            sheet_count: book.sheet_collection().len(),
            defined_names: defined_names_count,
            tables: table_count,
            macros_present,
            last_modified: self
                .last_modified
                .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            revision_id: Some(self.revision_id.clone()),
            caps: self.caps.clone(),
        }
    }

    pub fn get_sheet_metrics_fast(&self, sheet_name: &str) -> Result<Arc<SheetCacheEntry>> {
        if let Some(entry) = self.sheet_cache.read().get(sheet_name) {
            return Ok(entry.clone());
        }

        let mut writer = self.sheet_cache.write();
        if let Some(entry) = writer.get(sheet_name) {
            return Ok(entry.clone());
        }

        let book = self.spreadsheet.read();
        let sheet = book
            .sheet_by_name(sheet_name).ok()
            .ok_or_else(|| anyhow!("sheet {} not found", sheet_name))?;
        let (metrics, style_tags) = compute_sheet_metrics(sheet);
        let named_ranges = gather_named_ranges(sheet, book.defined_names());

        let entry = Arc::new(SheetCacheEntry {
            metrics,
            style_tags,
            named_ranges,
            detected_regions: RwLock::new(None),
            region_notes: RwLock::new(Vec::new()),
        });

        writer.insert(sheet_name.to_string(), entry.clone());
        Ok(entry)
    }

    pub fn get_sheet_metrics(&self, sheet_name: &str) -> Result<Arc<SheetCacheEntry>> {
        let entry = self.get_sheet_metrics_fast(sheet_name)?;
        if entry.has_detected_regions() {
            return Ok(entry);
        }

        let book = self.spreadsheet.read();
        let sheet = book
            .sheet_by_name(sheet_name).ok()
            .ok_or_else(|| anyhow!("sheet {} not found", sheet_name))?;
        let detected = detect_regions(sheet, &entry.metrics);
        entry.set_detected_regions(detected.regions);
        entry.set_region_notes(detected.notes);
        Ok(entry)
    }

    pub fn list_summaries(&self, include_bounds: bool) -> Result<Vec<SheetSummary>> {
        let book = self.spreadsheet.read();
        let mut summaries = Vec::new();
        for sheet in book.sheet_collection() {
            let name = sheet.name().to_string();
            let entry = self.get_sheet_metrics_fast(&name)?;
            summaries.push(SheetSummary {
                name: name.clone(),
                visible: sheet.sheet_state() != "hidden",
                row_count: include_bounds.then_some(entry.metrics.row_count),
                column_count: include_bounds.then_some(entry.metrics.column_count),
                non_empty_cells: include_bounds.then_some(entry.metrics.non_empty_cells),
                formula_cells: include_bounds.then_some(entry.metrics.formula_cells),
                cached_values: include_bounds.then_some(entry.metrics.cached_values),
                classification: entry.metrics.classification.clone(),
                style_tags: if include_bounds {
                    entry.style_tags.clone()
                } else {
                    Vec::new()
                },
            });
        }
        Ok(summaries)
    }

    pub fn with_sheet<T, F>(&self, sheet_name: &str, func: F) -> Result<T>
    where
        F: FnOnce(&Worksheet) -> T,
    {
        let book = self.spreadsheet.read();
        let sheet = book
            .sheet_by_name(sheet_name).ok()
            .ok_or_else(|| anyhow!("sheet {} not found", sheet_name))?;
        Ok(func(sheet))
    }

    pub fn with_spreadsheet<T, F>(&self, func: F) -> Result<T>
    where
        F: FnOnce(&Spreadsheet) -> T,
    {
        let book = self.spreadsheet.read();
        Ok(func(&book))
    }

    pub fn formula_graph(&self, sheet_name: &str) -> Result<FormulaGraph> {
        self.with_sheet(sheet_name, |sheet| {
            FormulaGraph::build(sheet, &self.formula_atlas, FormulaParsePolicy::Warn, None)
        })?
    }

    pub fn formula_graph_with_diagnostics(
        &self,
        sheet_name: &str,
        policy: FormulaParsePolicy,
    ) -> Result<(FormulaGraph, FormulaParseDiagnostics)> {
        let mut builder = FormulaParseDiagnosticsBuilder::new(policy);
        let graph = self.with_sheet(sheet_name, |sheet| {
            FormulaGraph::build(sheet, &self.formula_atlas, policy, Some(&mut builder))
        })??;
        Ok((graph, builder.build()))
    }

    pub(crate) fn formula_graph_with_diagnostics_builder(
        &self,
        sheet_name: &str,
        policy: FormulaParsePolicy,
        builder: &mut FormulaParseDiagnosticsBuilder,
    ) -> Result<FormulaGraph> {
        self.with_sheet(sheet_name, |sheet| {
            FormulaGraph::build(sheet, &self.formula_atlas, policy, Some(builder))
        })?
    }

    pub fn named_items(&self) -> Result<Vec<NamedRangeDescriptor>> {
        let book = self.spreadsheet.read();
        let sheet_names: Vec<String> = book
            .sheet_collection()
            .iter()
            .map(|sheet| sheet.name().to_string())
            .collect();
        let mut items = Vec::new();
        for defined in book.defined_names() {
            let refers_to = defined.address();
            let scope = if defined.has_local_sheet_id() {
                let idx = defined.local_sheet_id() as usize;
                sheet_names.get(idx).cloned()
            } else {
                None
            };
            let kind = if refers_to.starts_with('=') {
                NamedItemKind::Formula
            } else {
                NamedItemKind::NamedRange
            };

            let (scope_kind, scope_sheet_name) = if defined.has_local_sheet_id() {
                let idx = defined.local_sheet_id() as usize;
                (Some(NamedRangeScope::Sheet), sheet_names.get(idx).cloned())
            } else {
                (Some(NamedRangeScope::Workbook), None)
            };

            items.push(NamedRangeDescriptor {
                name: defined.name().to_string(),
                scope: scope.clone(),
                scope_kind,
                scope_sheet_name: scope_sheet_name.clone(),
                refers_to: refers_to.clone(),
                kind,
                sheet_name: scope,
                comment: None,
            });
        }

        // Also collect sheet-level defined names
        for sheet in book.sheet_collection() {
            for defined in sheet.defined_names() {
                let refers_to = defined.address();
                let kind = if refers_to.starts_with('=') {
                    NamedItemKind::Formula
                } else {
                    NamedItemKind::NamedRange
                };
                let sheet_name_str = sheet.name().to_string();
                // Avoid duplicates: skip if already present from workbook-level
                let already_present = items.iter().any(|item| {
                    item.name == defined.name()
                        && item.refers_to == refers_to
                        && (item.scope_kind == Some(NamedRangeScope::Workbook)
                            || item.scope_sheet_name.as_deref() == Some(sheet_name_str.as_str()))
                });
                if already_present {
                    continue;
                }
                let is_sheet_scoped = defined.has_local_sheet_id();
                items.push(NamedRangeDescriptor {
                    name: defined.name().to_string(),
                    scope: is_sheet_scoped.then(|| sheet_name_str.clone()),
                    scope_kind: Some(if is_sheet_scoped {
                        NamedRangeScope::Sheet
                    } else {
                        NamedRangeScope::Workbook
                    }),
                    scope_sheet_name: is_sheet_scoped.then(|| sheet_name_str.clone()),
                    refers_to,
                    kind,
                    sheet_name: is_sheet_scoped.then_some(sheet_name_str),
                    comment: None,
                });
            }

            for table in sheet.tables() {
                let start = table.area().0.get_coordinate();
                let end = table.area().1.get_coordinate();
                items.push(NamedRangeDescriptor {
                    name: table.name().to_string(),
                    scope: Some(sheet.name().to_string()),
                    scope_kind: Some(NamedRangeScope::Sheet),
                    scope_sheet_name: Some(sheet.name().to_string()),
                    refers_to: format!("{}:{}", start, end),
                    kind: NamedItemKind::Table,
                    sheet_name: Some(sheet.name().to_string()),
                    comment: None,
                });
            }
        }

        Ok(items)
    }

    pub fn sheet_overview(&self, sheet_name: &str) -> Result<SheetOverviewResponse> {
        let entry = self.get_sheet_metrics(sheet_name)?;
        let narrative = classification::narrative(&entry.metrics);
        let regions = classification::regions(&entry.metrics);
        let key_ranges = classification::key_ranges(&entry.metrics);
        let detected_regions = entry.detected_regions();

        Ok(SheetOverviewResponse {
            workbook_id: self.id.clone(),
            sheet_name: sheet_name.to_string(),
            narrative,
            regions,
            detected_regions: detected_regions.clone(),
            detected_region_count: detected_regions.len() as u32,
            detected_regions_truncated: false,
            key_ranges,
            formula_ratio: if entry.metrics.non_empty_cells == 0 {
                0.0
            } else {
                (entry.metrics.formula_cells as f32 / entry.metrics.non_empty_cells as f32)
                    .clamp(0.0, 1.0)
            },
            notable_features: entry.style_tags.clone(),
            notes: entry.region_notes(),
        })
    }

    pub fn detected_region(
        &self,
        sheet_name: &str,
        id: u32,
    ) -> Result<crate::model::DetectedRegion> {
        let entry = self.get_sheet_metrics(sheet_name)?;
        entry
            .detected_regions()
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("region {} not found on sheet {}", id, sheet_name))
    }
}

fn contains_date_time_token(format_code: &str) -> bool {
    let mut in_quote = false;
    let mut in_bracket = false;
    let chars: Vec<char> = format_code.chars().collect();

    for (i, &ch) in chars.iter().enumerate() {
        match ch {
            '"' => in_quote = !in_quote,
            '[' if !in_quote => in_bracket = true,
            ']' if !in_quote => in_bracket = false,
            'y' | 'd' | 'h' | 's' | 'm' if !in_quote && !in_bracket => {
                if ch == 'm' {
                    let prev = if i > 0 { chars.get(i - 1) } else { None };
                    let next = chars.get(i + 1);
                    let after_time_sep = prev == Some(&':') || prev == Some(&'h');
                    let before_time_sep = next == Some(&':') || next == Some(&'s');
                    if after_time_sep || before_time_sep {
                        return true;
                    }
                    if prev == Some(&'m') || next == Some(&'m') {
                        return true;
                    }
                    if matches!(prev, Some(&'/') | Some(&'-') | Some(&'.'))
                        || matches!(next, Some(&'/') | Some(&'-') | Some(&'.'))
                    {
                        return true;
                    }
                } else {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

const DATE_FORMAT_IDS: &[u32] = &[
    14, 15, 16, 17, 18, 19, 20, 21, 22, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 45, 46, 47, 50, 51,
    52, 53, 54, 55, 56, 57, 58,
];

const EXCEL_LEAP_YEAR_BUG_SERIAL: i64 = 60;

fn is_date_formatted(cell: &umya_spreadsheet::Cell) -> bool {
    let Some(nf) = cell.style().number_format() else {
        return false;
    };

    let format_id = nf.number_format_id();
    if DATE_FORMAT_IDS.contains(&format_id) {
        return true;
    }

    let code = nf.format_code();
    if code == "General" || code == "@" || code == "0" || code == "0.00" {
        return false;
    }

    contains_date_time_token(code)
}

pub fn excel_serial_to_iso(serial: f64, use_1904_system: bool) -> String {
    excel_serial_to_iso_with_leap_bug(serial, use_1904_system, true)
}

pub fn excel_serial_to_iso_with_leap_bug(
    serial: f64,
    use_1904_system: bool,
    compensate_leap_bug: bool,
) -> String {
    use chrono::NaiveDate;

    let days = serial.trunc() as i64;

    if use_1904_system {
        let epoch_1904 = NaiveDate::from_ymd_opt(1904, 1, 1).unwrap();
        return epoch_1904
            .checked_add_signed(chrono::Duration::days(days))
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| serial.to_string());
    }

    let epoch = if compensate_leap_bug && days >= EXCEL_LEAP_YEAR_BUG_SERIAL {
        NaiveDate::from_ymd_opt(1899, 12, 30).unwrap()
    } else {
        NaiveDate::from_ymd_opt(1899, 12, 31).unwrap()
    };

    epoch
        .checked_add_signed(chrono::Duration::days(days))
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| serial.to_string())
}

pub fn cell_to_value(cell: &umya_spreadsheet::Cell) -> Option<crate::model::CellValue> {
    cell_to_value_with_date_system(cell, false)
}

pub fn cell_to_value_with_date_system(
    cell: &umya_spreadsheet::Cell,
    use_1904_system: bool,
) -> Option<crate::model::CellValue> {
    let raw = cell.value();
    if raw.is_empty() {
        return None;
    }
    if cell.data_type() == "e" || (cell.is_formula() && is_spreadsheet_error(&raw)) {
        return Some(crate::model::CellValue::Error(raw.to_string()));
    }
    if let Ok(number) = raw.parse::<f64>() {
        if is_date_formatted(cell) {
            return Some(crate::model::CellValue::Date(excel_serial_to_iso(
                number,
                use_1904_system,
            )));
        }
        return Some(crate::model::CellValue::Number(number));
    }

    let lower = raw.to_ascii_lowercase();
    if lower == "true" {
        return Some(crate::model::CellValue::Bool(true));
    }
    if lower == "false" {
        return Some(crate::model::CellValue::Bool(false));
    }

    Some(crate::model::CellValue::Text(raw.to_string()))
}

pub fn is_spreadsheet_error(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_uppercase().as_str(),
        "#DIV/0!"
            | "#VALUE!"
            | "#NAME?"
            | "#REF!"
            | "#N/A"
            | "#NULL!"
            | "#NUM!"
            | "#SPILL!"
            | "#CALC!"
            | "#CIRC!"
            | "#BUSY!"
            | "#FIELD!"
            | "#UNKNOWN!"
            | "#CONNECT!"
            | "#GETTING_DATA"
            | "#DATA!"
    )
}

pub fn compute_sheet_metrics(sheet: &Worksheet) -> (SheetMetrics, Vec<String>) {
    use std::collections::HashMap as StdHashMap;
    let mut non_empty = 0u32;
    let mut formulas = 0u32;
    let mut cached = 0u32;
    let comments = sheet.comments().len() as u32;
    let mut style_usage: StdHashMap<String, StyleUsage> = StdHashMap::new();

    for cell in sheet.cells() {
        let value = cell.value();
        let is_formula = cell.is_formula();
        if is_formula || !value.is_empty() {
            non_empty += 1;
        }
        if is_formula {
            formulas += 1;
            if !value.is_empty() {
                cached += 1;
            }
        }

        if let Some((style_key, usage)) = style::tag_cell(cell) {
            let entry = style_usage.entry(style_key).or_insert_with(|| StyleUsage {
                occurrences: 0,
                tags: usage.tags.clone(),
                example_cells: Vec::new(),
            });
            entry.occurrences += 1;
            if entry.example_cells.len() < 5 {
                entry.example_cells.push(usage.example_cell.clone());
            }
        }
    }

    let (max_col, max_row) = sheet.highest_column_and_row();

    let classification = classification::classify(
        non_empty,
        formulas,
        max_row,
        max_col,
        comments,
        &style_usage,
    );

    let style_tags: Vec<String> = style_usage
        .values()
        .flat_map(|usage| usage.tags.clone())
        .collect();

    let metrics = SheetMetrics {
        row_count: max_row,
        column_count: max_col,
        non_empty_cells: non_empty,
        formula_cells: formulas,
        cached_values: cached,
        comments,
        style_map: style_usage,
        classification,
    };
    (metrics, style_tags)
}

#[derive(Debug, Clone, Copy)]
struct Rect {
    start_row: u32,
    end_row: u32,
    start_col: u32,
    end_col: u32,
}

#[derive(Debug, Clone)]
struct CellInfo {
    value: Option<crate::model::CellValue>,
    is_formula: bool,
}

#[derive(Debug)]
struct Occupancy {
    cells: HashMap<(u32, u32), CellInfo>,
    rows: HashMap<u32, Vec<u32>>,
    cols: HashMap<u32, Vec<u32>>,
    min_row: u32,
    max_row: u32,
    min_col: u32,
    max_col: u32,
}

impl Occupancy {
    fn bounds_rect(&self) -> Option<Rect> {
        if self.cells.is_empty() {
            None
        } else {
            Some(Rect {
                start_row: self.min_row,
                end_row: self.max_row,
                start_col: self.min_col,
                end_col: self.max_col,
            })
        }
    }

    fn dense_bounds(&self) -> Option<Rect> {
        let bounds = self.bounds_rect()?;
        let total_cells = self.cells.len();
        if total_cells < DETECT_OUTLIER_MIN_CELLS {
            return Some(bounds);
        }
        let trim_cells = ((total_cells as f32) * DETECT_OUTLIER_FRACTION).round() as usize;
        if trim_cells == 0 || trim_cells * 2 >= total_cells {
            return Some(bounds);
        }

        let mut row_counts: Vec<(u32, usize)> = self
            .rows
            .iter()
            .map(|(row, cols)| (*row, cols.len()))
            .collect();
        row_counts.sort_by_key(|(row, _)| *row);

        let mut col_counts: Vec<(u32, usize)> = self
            .cols
            .iter()
            .map(|(col, rows)| (*col, rows.len()))
            .collect();
        col_counts.sort_by_key(|(col, _)| *col);

        let (start_row, end_row) =
            trim_bounds_by_cells(&row_counts, trim_cells, bounds.start_row, bounds.end_row);
        let (start_col, end_col) =
            trim_bounds_by_cells(&col_counts, trim_cells, bounds.start_col, bounds.end_col);

        if start_row > end_row || start_col > end_col {
            return Some(bounds);
        }

        Some(Rect {
            start_row,
            end_row,
            start_col,
            end_col,
        })
    }

    fn row_col_counts(&self, rect: &Rect) -> (Vec<u32>, Vec<u32>) {
        let height = (rect.end_row - rect.start_row + 1) as usize;
        let width = (rect.end_col - rect.start_col + 1) as usize;
        let mut row_counts = vec![0u32; height];
        let mut col_counts = vec![0u32; width];

        for (row, cols) in &self.rows {
            if *row < rect.start_row || *row > rect.end_row {
                continue;
            }
            let count = count_in_sorted_range(cols, rect.start_col, rect.end_col);
            row_counts[(row - rect.start_row) as usize] = count;
        }
        for (col, rows) in &self.cols {
            if *col < rect.start_col || *col > rect.end_col {
                continue;
            }
            let count = count_in_sorted_range(rows, rect.start_row, rect.end_row);
            col_counts[(col - rect.start_col) as usize] = count;
        }
        (row_counts, col_counts)
    }

    fn stats_in_rect(&self, rect: &Rect) -> RegionStats {
        let mut stats = RegionStats::default();
        for (row, cols) in &self.rows {
            if *row < rect.start_row || *row > rect.end_row {
                continue;
            }
            let start_idx = lower_bound(cols, rect.start_col);
            let end_idx = upper_bound(cols, rect.end_col);
            for col in &cols[start_idx..end_idx] {
                if let Some(info) = self.cells.get(&(*row, *col)) {
                    stats.non_empty += 1;
                    if info.is_formula {
                        stats.formulas += 1;
                    }
                    if let Some(val) = &info.value {
                        match val {
                            crate::model::CellValue::Text(_) => stats.text += 1,
                            crate::model::CellValue::Number(_) => stats.numbers += 1,
                            crate::model::CellValue::Bool(_) => stats.bools += 1,
                            crate::model::CellValue::Date(_) => stats.dates += 1,
                            crate::model::CellValue::Error(_) => stats.errors += 1,
                        }
                    }
                }
            }
        }
        stats
    }

    fn value_at(&self, row: u32, col: u32) -> Option<&crate::model::CellValue> {
        self.cells.get(&(row, col)).and_then(|c| c.value.as_ref())
    }
}

fn lower_bound(values: &[u32], target: u32) -> usize {
    let mut left = 0;
    let mut right = values.len();
    while left < right {
        let mid = (left + right) / 2;
        if values[mid] < target {
            left = mid + 1;
        } else {
            right = mid;
        }
    }
    left
}

fn upper_bound(values: &[u32], target: u32) -> usize {
    let mut left = 0;
    let mut right = values.len();
    while left < right {
        let mid = (left + right) / 2;
        if values[mid] <= target {
            left = mid + 1;
        } else {
            right = mid;
        }
    }
    left
}

fn count_in_sorted_range(values: &[u32], start: u32, end: u32) -> u32 {
    if values.is_empty() {
        return 0;
    }
    let start_idx = lower_bound(values, start);
    let end_idx = upper_bound(values, end);
    end_idx.saturating_sub(start_idx) as u32
}

fn trim_bounds_by_cells(
    entries: &[(u32, usize)],
    trim_cells: usize,
    default_start: u32,
    default_end: u32,
) -> (u32, u32) {
    if entries.is_empty() {
        return (default_start, default_end);
    }

    let mut remaining = trim_cells;
    let mut start_idx = 0usize;
    while start_idx < entries.len() {
        let count = entries[start_idx].1;
        if remaining < count {
            break;
        }
        remaining -= count;
        start_idx += 1;
    }

    let mut remaining = trim_cells;
    let mut end_idx = entries.len();
    while end_idx > 0 {
        let count = entries[end_idx - 1].1;
        if remaining < count {
            break;
        }
        remaining -= count;
        end_idx -= 1;
    }

    let start = entries
        .get(start_idx)
        .map(|(idx, _)| *idx)
        .unwrap_or(default_start);
    let end = if end_idx == 0 {
        default_end
    } else {
        entries
            .get(end_idx - 1)
            .map(|(idx, _)| *idx)
            .unwrap_or(default_end)
    };
    (start, end)
}

#[derive(Debug, Default, Clone)]
struct RegionStats {
    non_empty: u32,
    formulas: u32,
    text: u32,
    numbers: u32,
    bools: u32,
    dates: u32,
    errors: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gutter {
    Row { start: u32, end: u32 },
    Col { start: u32, end: u32 },
}

#[derive(Debug, Default)]
struct DetectRegionsResult {
    regions: Vec<crate::model::DetectedRegion>,
    notes: Vec<String>,
}

#[derive(Debug)]
struct DetectLimits {
    start: Instant,
    max_ms: u64,
    max_leaves: usize,
    max_depth: u32,
    leaves: usize,
    exceeded_time: bool,
    exceeded_leaves: bool,
}

impl DetectLimits {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            max_ms: DETECT_MAX_MS,
            max_leaves: DETECT_MAX_LEAVES,
            max_depth: DETECT_MAX_DEPTH,
            leaves: 0,
            exceeded_time: false,
            exceeded_leaves: false,
        }
    }

    fn should_stop(&mut self) -> bool {
        if !self.exceeded_time && self.start.elapsed().as_millis() as u64 >= self.max_ms {
            self.exceeded_time = true;
        }
        self.exceeded_time || self.exceeded_leaves
    }

    fn note_leaf(&mut self) {
        self.leaves += 1;
        if self.leaves >= self.max_leaves {
            self.exceeded_leaves = true;
        }
    }
}

fn detect_regions(sheet: &Worksheet, metrics: &SheetMetrics) -> DetectRegionsResult {
    if metrics.row_count == 0 || metrics.column_count == 0 {
        return DetectRegionsResult::default();
    }

    let occupancy = build_occupancy(sheet);
    if occupancy.cells.is_empty() {
        return DetectRegionsResult::default();
    }

    let area = (metrics.row_count as u64) * (metrics.column_count as u64);
    let exceeds_caps = metrics.row_count > DETECT_MAX_ROWS
        || metrics.column_count > DETECT_MAX_COLS
        || area > DETECT_MAX_AREA
        || occupancy.cells.len() > DETECT_MAX_CELLS;

    if exceeds_caps {
        let mut result = DetectRegionsResult::default();
        if let Some(bounds) = occupancy.dense_bounds() {
            result.regions.push(build_fallback_region(&bounds, metrics));
        }
        result.notes.push(format!(
            "Region detection capped: rows {}, cols {}, occupied {}.",
            metrics.row_count,
            metrics.column_count,
            occupancy.cells.len()
        ));
        return result;
    }

    let root = occupancy.bounds_rect().unwrap_or(Rect {
        start_row: 1,
        end_row: metrics.row_count.max(1),
        start_col: 1,
        end_col: metrics.column_count.max(1),
    });

    let mut leaves = Vec::new();
    let mut limits = DetectLimits::new();
    split_rect(&occupancy, &root, 0, &mut limits, &mut leaves);

    let mut regions = Vec::new();
    for (idx, rect) in leaves.into_iter().enumerate() {
        if limits.should_stop() {
            break;
        }
        if let Some(trimmed) = trim_rect(&occupancy, rect, &mut limits) {
            let region = build_region(&occupancy, &trimmed, metrics, idx as u32);
            regions.push(region);
        }
    }

    let mut notes = Vec::new();
    if limits.exceeded_time || limits.exceeded_leaves {
        notes.push("Region detection truncated due to time/complexity caps.".to_string());
    }
    if regions.is_empty()
        && let Some(bounds) = occupancy.dense_bounds()
    {
        regions.push(build_fallback_region(&bounds, metrics));
        notes.push("Region detection returned no regions; fallback bounds used.".to_string());
    }

    DetectRegionsResult { regions, notes }
}

fn build_fallback_region(rect: &Rect, metrics: &SheetMetrics) -> crate::model::DetectedRegion {
    let kind = match metrics.classification {
        SheetClassification::Calculator => crate::model::RegionKind::Calculator,
        SheetClassification::Metadata => crate::model::RegionKind::Metadata,
        _ => crate::model::RegionKind::Data,
    };
    let end_col = crate::utils::column_number_to_name(rect.end_col.max(1));
    let end_cell = format!("{}{}", end_col, rect.end_row.max(1));
    let header_count = rect.end_col - rect.start_col + 1;
    crate::model::DetectedRegion {
        id: 0,
        bounds: format!(
            "{}{}:{}",
            crate::utils::column_number_to_name(rect.start_col),
            rect.start_row,
            end_cell
        ),
        header_row: None,
        headers: Vec::new(),
        header_count,
        headers_truncated: header_count > 0,
        row_count: rect.end_row - rect.start_row + 1,
        classification: kind.clone(),
        region_kind: Some(kind),
        confidence: 0.2,
    }
}

fn build_occupancy(sheet: &Worksheet) -> Occupancy {
    let mut cells = HashMap::new();
    let mut rows: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut cols: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut min_row = u32::MAX;
    let mut max_row = 0u32;
    let mut min_col = u32::MAX;
    let mut max_col = 0u32;

    for cell in sheet.cells() {
        let coord = cell.coordinate();
        let row = coord.row_num();
        let col = coord.col_num();
        let value = cell_to_value(cell);
        let is_formula = cell.is_formula();
        cells.insert((row, col), CellInfo { value, is_formula });
        rows.entry(row).or_default().push(col);
        cols.entry(col).or_default().push(row);
        min_row = min_row.min(row);
        max_row = max_row.max(row);
        min_col = min_col.min(col);
        max_col = max_col.max(col);
    }

    for cols in rows.values_mut() {
        cols.sort_unstable();
    }
    for rows in cols.values_mut() {
        rows.sort_unstable();
    }

    if cells.is_empty() {
        min_row = 0;
        min_col = 0;
    }

    Occupancy {
        cells,
        rows,
        cols,
        min_row,
        max_row,
        min_col,
        max_col,
    }
}

fn split_rect(
    occupancy: &Occupancy,
    rect: &Rect,
    depth: u32,
    limits: &mut DetectLimits,
    leaves: &mut Vec<Rect>,
) {
    if limits.should_stop() || depth >= limits.max_depth {
        limits.note_leaf();
        leaves.push(*rect);
        return;
    }
    if rect.start_row >= rect.end_row && rect.start_col >= rect.end_col {
        limits.note_leaf();
        leaves.push(*rect);
        return;
    }
    if let Some(gutter) = find_best_gutter(occupancy, rect, limits) {
        match gutter {
            Gutter::Row { start, end } => {
                if start > rect.start_row {
                    let upper = Rect {
                        start_row: rect.start_row,
                        end_row: start - 1,
                        start_col: rect.start_col,
                        end_col: rect.end_col,
                    };
                    split_rect(occupancy, &upper, depth + 1, limits, leaves);
                }
                if end < rect.end_row {
                    let lower = Rect {
                        start_row: end + 1,
                        end_row: rect.end_row,
                        start_col: rect.start_col,
                        end_col: rect.end_col,
                    };
                    split_rect(occupancy, &lower, depth + 1, limits, leaves);
                }
            }
            Gutter::Col { start, end } => {
                if start > rect.start_col {
                    let left = Rect {
                        start_row: rect.start_row,
                        end_row: rect.end_row,
                        start_col: rect.start_col,
                        end_col: start - 1,
                    };
                    split_rect(occupancy, &left, depth + 1, limits, leaves);
                }
                if end < rect.end_col {
                    let right = Rect {
                        start_row: rect.start_row,
                        end_row: rect.end_row,
                        start_col: end + 1,
                        end_col: rect.end_col,
                    };
                    split_rect(occupancy, &right, depth + 1, limits, leaves);
                }
            }
        }
        return;
    }
    limits.note_leaf();
    leaves.push(*rect);
}

fn find_best_gutter(
    occupancy: &Occupancy,
    rect: &Rect,
    limits: &mut DetectLimits,
) -> Option<Gutter> {
    if limits.should_stop() {
        return None;
    }
    let (row_counts, col_counts) = occupancy.row_col_counts(rect);
    let width = rect.end_col - rect.start_col + 1;
    let height = rect.end_row - rect.start_row + 1;

    let row_blank_runs = find_blank_runs(&row_counts, width);
    let col_blank_runs = find_blank_runs(&col_counts, height);

    let mut best: Option<(Gutter, u32)> = None;

    if let Some((start, end, len)) = row_blank_runs {
        let gutter = Gutter::Row {
            start: rect.start_row + start,
            end: rect.start_row + end,
        };
        best = Some((gutter, len));
    }
    if let Some((start, end, len)) = col_blank_runs {
        let gutter = Gutter::Col {
            start: rect.start_col + start,
            end: rect.start_col + end,
        };
        if best.map(|(_, l)| len > l).unwrap_or(true) {
            best = Some((gutter, len));
        }
    }

    best.map(|(g, _)| g)
}

fn find_blank_runs(counts: &[u32], span: u32) -> Option<(u32, u32, u32)> {
    if counts.is_empty() {
        return None;
    }
    let mut best_start = 0;
    let mut best_end = 0;
    let mut best_len = 0;
    let mut current_start = None;
    for (idx, count) in counts.iter().enumerate() {
        let is_blank = *count == 0 || (*count as f32 / span as f32) < 0.05;
        if is_blank {
            if current_start.is_none() {
                current_start = Some(idx as u32);
            }
        } else if let Some(start) = current_start.take() {
            let end = idx as u32 - 1;
            let len = end - start + 1;
            if len > best_len && start > 0 && end + 1 < counts.len() as u32 {
                best_len = len;
                best_start = start;
                best_end = end;
            }
        }
    }
    if let Some(start) = current_start {
        let end = counts.len() as u32 - 1;
        let len = end - start + 1;
        if len > best_len && start > 0 && end + 1 < counts.len() as u32 {
            best_len = len;
            best_start = start;
            best_end = end;
        }
    }
    if best_len >= 2 {
        Some((best_start, best_end, best_len))
    } else {
        None
    }
}

fn trim_rect(occupancy: &Occupancy, rect: Rect, limits: &mut DetectLimits) -> Option<Rect> {
    let mut r = rect;
    loop {
        if limits.should_stop() {
            return Some(r);
        }
        let (row_counts, col_counts) = occupancy.row_col_counts(&r);
        let width = r.end_col - r.start_col + 1;
        let height = r.end_row - r.start_row + 1;
        let top_blank = row_counts
            .first()
            .map(|c| *c == 0 || (*c as f32 / width as f32) < 0.1)
            .unwrap_or(false);
        let bottom_blank = row_counts
            .last()
            .map(|c| *c == 0 || (*c as f32 / width as f32) < 0.1)
            .unwrap_or(false);
        let left_blank = col_counts
            .first()
            .map(|c| *c == 0 || (*c as f32 / height as f32) < 0.1)
            .unwrap_or(false);
        let right_blank = col_counts
            .last()
            .map(|c| *c == 0 || (*c as f32 / height as f32) < 0.1)
            .unwrap_or(false);

        let mut changed = false;
        if top_blank && r.start_row < r.end_row {
            r.start_row += 1;
            changed = true;
        }
        if bottom_blank && r.end_row > r.start_row {
            r.end_row -= 1;
            changed = true;
        }
        if left_blank && r.start_col < r.end_col {
            r.start_col += 1;
            changed = true;
        }
        if right_blank && r.end_col > r.start_col {
            r.end_col -= 1;
            changed = true;
        }

        if !changed {
            break;
        }
        if r.start_row > r.end_row || r.start_col > r.end_col {
            return None;
        }
    }
    Some(r)
}

fn build_region(
    occupancy: &Occupancy,
    rect: &Rect,
    metrics: &SheetMetrics,
    id: u32,
) -> crate::model::DetectedRegion {
    let header_info = detect_headers(occupancy, rect);
    let stats = occupancy.stats_in_rect(rect);
    let (kind, confidence) = classify_region(rect, &stats, &header_info, metrics);
    let header_len = header_info.headers.len() as u32;
    let header_count = rect.end_col - rect.start_col + 1;
    let headers_truncated = header_len != header_count;
    crate::model::DetectedRegion {
        id,
        bounds: format!(
            "{}{}:{}{}",
            crate::utils::column_number_to_name(rect.start_col),
            rect.start_row,
            crate::utils::column_number_to_name(rect.end_col),
            rect.end_row
        ),
        header_row: header_info.header_row,
        headers: header_info.headers,
        header_count,
        headers_truncated,
        row_count: rect.end_row - rect.start_row + 1,
        classification: kind.clone(),
        region_kind: Some(kind),
        confidence,
    }
}

#[derive(Debug, Default)]
struct HeaderInfo {
    header_row: Option<u32>,
    headers: Vec<String>,
    is_key_value: bool,
}

fn is_key_value_layout(occupancy: &Occupancy, rect: &Rect) -> bool {
    let width = rect.end_col - rect.start_col + 1;

    if width == 2 {
        return check_key_value_columns(occupancy, rect, rect.start_col, rect.start_col + 1);
    }

    if width <= KV_MAX_WIDTH_FOR_DENSITY_CHECK {
        let rows_to_sample = (rect.end_row - rect.start_row + 1).min(KV_SAMPLE_ROWS);
        let density_threshold = (rows_to_sample as f32 * KV_DENSITY_THRESHOLD) as u32;

        let mut col_densities: Vec<(u32, u32)> = Vec::new();
        for col in rect.start_col..=rect.end_col {
            let count = (rect.start_row..rect.start_row + rows_to_sample)
                .filter(|&row| occupancy.value_at(row, col).is_some())
                .count() as u32;
            if count >= density_threshold {
                col_densities.push((col, count));
            }
        }

        if col_densities.len() == 2 {
            let label_col = col_densities[0].0;
            let value_col = col_densities[1].0;
            return check_key_value_columns(occupancy, rect, label_col, value_col);
        } else if col_densities.len() == 4 && width >= 4 {
            let pair1 =
                check_key_value_columns(occupancy, rect, col_densities[0].0, col_densities[1].0);
            let pair2 =
                check_key_value_columns(occupancy, rect, col_densities[2].0, col_densities[3].0);
            return pair1 && pair2;
        }
    }

    false
}

fn check_key_value_columns(
    occupancy: &Occupancy,
    rect: &Rect,
    label_col: u32,
    value_col: u32,
) -> bool {
    let mut label_value_pairs = 0u32;
    let rows_to_check = (rect.end_row - rect.start_row + 1).min(KV_CHECK_ROWS);

    for row in rect.start_row..rect.start_row + rows_to_check {
        let first_col = occupancy.value_at(row, label_col);
        let second_col = occupancy.value_at(row, value_col);

        if let (Some(crate::model::CellValue::Text(label)), Some(val)) = (first_col, second_col) {
            let label_looks_like_key = label.len() <= KV_MAX_LABEL_LEN
                && !label.chars().any(|c| c.is_ascii_digit())
                && label.contains(|c: char| c.is_alphabetic());

            let value_is_data = matches!(
                val,
                crate::model::CellValue::Number(_) | crate::model::CellValue::Date(_)
            ) || matches!(val, crate::model::CellValue::Text(s) if s.len() > KV_MIN_TEXT_VALUE_LEN);

            if label_looks_like_key && value_is_data {
                label_value_pairs += 1;
            }
        }
    }

    label_value_pairs >= KV_MIN_PAIRS
        && label_value_pairs as f32 / rows_to_check as f32 >= KV_MIN_PAIR_RATIO
}

fn header_data_penalty(s: &str) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    if s.len() > HEADER_LONG_STRING_PENALTY_THRESHOLD {
        return HEADER_LONG_STRING_PENALTY;
    }
    let first_char = s.chars().next().unwrap();
    let is_capitalized = first_char.is_uppercase();
    let has_lowercase = s.chars().skip(1).any(|c| c.is_lowercase());
    let is_all_caps = s.chars().all(|c| !c.is_alphabetic() || c.is_uppercase());
    let has_digits = s.chars().any(|c| c.is_ascii_digit());
    let is_proper_noun =
        is_capitalized && has_lowercase && !is_all_caps && s.len() > HEADER_PROPER_NOUN_MIN_LEN;

    let mut penalty = 0.0;
    if is_proper_noun {
        penalty += HEADER_PROPER_NOUN_PENALTY;
    }
    if has_digits && s.len() > HEADER_DIGIT_STRING_MIN_LEN {
        penalty += HEADER_DIGIT_STRING_PENALTY;
    }
    penalty
}

fn detect_headers(occupancy: &Occupancy, rect: &Rect) -> HeaderInfo {
    if is_key_value_layout(occupancy, rect) {
        let mut headers = Vec::new();
        for col in rect.start_col..=rect.end_col {
            headers.push(crate::utils::column_number_to_name(col));
        }
        return HeaderInfo {
            header_row: None,
            headers,
            is_key_value: true,
        };
    }

    let width = rect.end_col - rect.start_col + 1;
    if width > HEADER_MAX_COLUMNS {
        return HeaderInfo {
            header_row: None,
            headers: Vec::new(),
            is_key_value: false,
        };
    }

    let mut candidates = Vec::new();
    let max_row = rect
        .start_row
        .saturating_add(HEADER_MAX_SCAN_ROWS)
        .min(rect.end_row);
    for row in rect.start_row..=max_row {
        let mut text = 0;
        let mut numbers = 0;
        let mut non_empty = 0;
        let mut unique = HashSet::new();
        let mut data_like_penalty: f32 = 0.0;
        let mut year_like_bonus: f32 = 0.0;

        for col in rect.start_col..=rect.end_col {
            if let Some(val) = occupancy.value_at(row, col) {
                non_empty += 1;
                match val {
                    crate::model::CellValue::Text(s) => {
                        text += 1;
                        unique.insert(s.clone());
                        data_like_penalty += header_data_penalty(s);
                    }
                    crate::model::CellValue::Number(n) => {
                        if *n >= HEADER_YEAR_MIN && *n <= HEADER_YEAR_MAX && n.fract() == 0.0 {
                            year_like_bonus += HEADER_YEAR_LIKE_BONUS;
                            text += 1;
                        } else {
                            numbers += 1;
                        }
                    }
                    crate::model::CellValue::Bool(_) => text += 1,
                    crate::model::CellValue::Date(_) => {
                        data_like_penalty += HEADER_DATE_PENALTY;
                    }
                    crate::model::CellValue::Error(_) => {}
                }
            }
        }
        if non_empty == 0 {
            continue;
        }
        let score = text as f32 + unique.len() as f32 * HEADER_UNIQUE_BONUS
            - numbers as f32 * HEADER_NUMBER_PENALTY
            - data_like_penalty
            + year_like_bonus;
        candidates.push((row, score, text, non_empty));
    }

    let is_single_col = rect.start_col == rect.end_col;

    let header_candidates: Vec<&(u32, f32, u32, u32)> = candidates
        .iter()
        .filter(|(_, score, text, non_empty)| {
            *text >= 1
                && *text * 2 >= *non_empty
                && (!is_single_col || *score > HEADER_SINGLE_COL_MIN_SCORE)
        })
        .collect();

    let best = header_candidates.iter().copied().max_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.0.cmp(&a.0))
    });
    let earliest = header_candidates
        .iter()
        .copied()
        .min_by(|a, b| a.0.cmp(&b.0));

    let maybe_header = match (best, earliest) {
        (Some(best_row), Some(early_row)) => {
            if (best_row.1 - early_row.1).abs() <= HEADER_SCORE_TIE_THRESHOLD {
                Some(early_row.0)
            } else {
                Some(best_row.0)
            }
        }
        (Some(best_row), None) => Some(best_row.0),
        _ => None,
    };

    let mut header_rows = Vec::new();
    if let Some(hr) = maybe_header {
        header_rows.push(hr);
        if hr < rect.end_row
            && let Some((_, score_next, text_next, non_empty_next)) =
                candidates.iter().find(|(r, _, _, _)| *r == hr + 1)
            && *text_next >= 1
            && *text_next * 2 >= *non_empty_next
            && *score_next
                >= HEADER_SECOND_ROW_MIN_SCORE_RATIO
                    * candidates
                        .iter()
                        .find(|(r, _, _, _)| *r == hr)
                        .map(|c| c.1)
                        .unwrap_or(0.0)
        {
            header_rows.push(hr + 1);
        }
    }

    let mut headers = Vec::new();
    for col in rect.start_col..=rect.end_col {
        let mut parts = Vec::new();
        for hr in &header_rows {
            if let Some(val) = occupancy.value_at(*hr, col) {
                match val {
                    crate::model::CellValue::Text(s) if !s.trim().is_empty() => {
                        parts.push(s.trim().to_string())
                    }
                    crate::model::CellValue::Number(n) => parts.push(n.to_string()),
                    crate::model::CellValue::Bool(b) => parts.push(b.to_string()),
                    crate::model::CellValue::Date(d) => parts.push(d.clone()),
                    crate::model::CellValue::Error(e) => parts.push(e.clone()),
                    _ => {}
                }
            }
        }
        if parts.is_empty() {
            headers.push(crate::utils::column_number_to_name(col));
        } else {
            headers.push(parts.join(" / "));
        }
    }

    HeaderInfo {
        header_row: header_rows.first().copied(),
        headers,
        is_key_value: false,
    }
}

fn classify_region(
    rect: &Rect,
    stats: &RegionStats,
    header_info: &HeaderInfo,
    metrics: &SheetMetrics,
) -> (crate::model::RegionKind, f32) {
    let width = rect.end_col - rect.start_col + 1;
    let height = rect.end_row - rect.start_row + 1;
    let area = width.max(1) * height.max(1);
    let density = if area == 0 {
        0.0
    } else {
        stats.non_empty as f32 / area as f32
    };
    let formula_ratio = if stats.non_empty == 0 {
        0.0
    } else {
        stats.formulas as f32 / stats.non_empty as f32
    };
    let text_ratio = if stats.non_empty == 0 {
        0.0
    } else {
        stats.text as f32 / stats.non_empty as f32
    };

    let mut kind = crate::model::RegionKind::Data;
    if formula_ratio > 0.25 && is_outputs_band(rect, metrics, height, width) {
        kind = crate::model::RegionKind::Outputs;
    } else if formula_ratio > 0.55 {
        kind = crate::model::RegionKind::Calculator;
    } else if height <= 3
        && width <= 4
        && text_ratio > 0.5
        && rect.end_row >= metrics.row_count.saturating_sub(3)
    {
        kind = crate::model::RegionKind::Metadata;
    } else if header_info.is_key_value
        || (formula_ratio < 0.25
            && stats.numbers > 0
            && stats.text > 0
            && text_ratio >= 0.3
            && (width <= 2 || (width <= 3 && header_info.header_row.is_none())))
    {
        kind = crate::model::RegionKind::Parameters;
    } else if height <= 4 && width <= 6 && formula_ratio < 0.2 && text_ratio > 0.4 && density < 0.5
    {
        kind = crate::model::RegionKind::Metadata;
    }

    let mut confidence: f32 = 0.4;
    if header_info.header_row.is_some() {
        confidence += 0.2;
    }
    confidence += (density * 0.2).min(0.2);
    confidence += (formula_ratio * 0.2).min(0.2);
    if matches!(
        kind,
        crate::model::RegionKind::Parameters | crate::model::RegionKind::Metadata
    ) && width <= 4
    {
        confidence += 0.1;
    }
    if confidence > 1.0 {
        confidence = 1.0;
    }

    (kind, confidence)
}

fn is_outputs_band(rect: &Rect, metrics: &SheetMetrics, height: u32, width: u32) -> bool {
    let near_bottom = rect.end_row >= metrics.row_count.saturating_sub(6);
    let near_right = rect.end_col >= metrics.column_count.saturating_sub(3);
    let is_shallow = height <= 6;
    let is_narrow_at_edge = width <= 6 && near_right;
    let not_at_top_left = rect.start_row > 1 || rect.start_col > 1;
    let sheet_has_depth = metrics.row_count > 10 || metrics.column_count > 6;
    let is_band = (is_shallow && near_bottom) || is_narrow_at_edge;
    is_band && not_at_top_left && sheet_has_depth
}

fn gather_named_ranges(
    sheet: &Worksheet,
    defined_names: &[DefinedName],
) -> Vec<NamedRangeDescriptor> {
    let name_str = sheet.name();
    defined_names
        .iter()
        .filter(|name| name.address().contains(name_str))
        .map(|name| {
            let (scope_kind, scope_sheet_name) = if name.has_local_sheet_id() {
                (Some(NamedRangeScope::Sheet), Some(name_str.to_string()))
            } else {
                (Some(NamedRangeScope::Workbook), None)
            };
            NamedRangeDescriptor {
                name: name.name().to_string(),
                scope: if name.has_local_sheet_id() {
                    Some(name_str.to_string())
                } else {
                    None
                },
                scope_kind,
                scope_sheet_name,
                refers_to: name.address(),
                kind: NamedItemKind::NamedRange,
                sheet_name: Some(name_str.to_string()),
                comment: None,
            }
        })
        .collect()
}

/// Scan the configured workspace root for workbooks. Host-filesystem only:
/// the wasm32 byte/session adapter registers workbooks explicitly through
/// [`crate::repository::VirtualWorkspaceRepository`].
#[cfg(feature = "native-fs")]
pub fn build_workbook_list(
    config: &Arc<ServerConfig>,
    filter: &WorkbookFilter,
) -> Result<WorkbookListResponse> {
    let mut descriptors = Vec::new();

    if let Some(single) = config.single_workbook() {
        let metadata = fs::metadata(single)
            .with_context(|| format!("unable to read metadata for {:?}", single))?;
        let canonical = fs::canonicalize(single).unwrap_or_else(|_| single.to_path_buf());
        let id = WorkbookId(hash_path_identity(&canonical));
        let slug = single
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "workbook".to_string());
        let folder = derive_folder(config, single);
        let short_id = make_short_workbook_id(&slug, id.as_str());
        let caps = BackendCaps::xlsx();

        if filter.matches(&slug, folder.as_deref(), single) {
            let relative = single
                .strip_prefix(&config.workspace_root)
                .unwrap_or(single);
            let descriptor = crate::model::WorkbookDescriptor {
                workbook_id: id,
                short_id,
                slug,
                folder,
                path: Some(path_to_forward_slashes(relative)),
                client_path: None,
                bytes: metadata.len(),
                last_modified: metadata
                    .modified()
                    .ok()
                    .and_then(system_time_to_rfc3339)
                    .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                revision_id: Some(hash_file_sha256_hex(single)?),
                caps: Some(caps),
            };
            descriptors.push(descriptor);
        }

        return Ok(WorkbookListResponse {
            workbooks: descriptors,
            next_offset: None,
        });
    }

    use walkdir::WalkDir;

    for entry in WalkDir::new(&config.workspace_root) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !has_supported_extension(&config.supported_extensions, path) {
            continue;
        }
        let metadata = entry.metadata()?;
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let id = WorkbookId(hash_path_identity(&canonical));
        let slug = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "workbook".to_string());
        let folder = derive_folder(config, path);
        let short_id = make_short_workbook_id(&slug, id.as_str());
        let caps = BackendCaps::xlsx();

        if !filter.matches(&slug, folder.as_deref(), path) {
            continue;
        }

        let relative = path.strip_prefix(&config.workspace_root).unwrap_or(path);
        let descriptor = crate::model::WorkbookDescriptor {
            workbook_id: id,
            short_id,
            slug,
            folder,
            path: Some(path_to_forward_slashes(relative)),
            client_path: None,
            bytes: metadata.len(),
            last_modified: metadata
                .modified()
                .ok()
                .and_then(system_time_to_rfc3339)
                .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            revision_id: Some(hash_file_sha256_hex(path)?),
            caps: Some(caps),
        };
        descriptors.push(descriptor);
    }

    descriptors.sort_by(|a, b| a.slug.cmp(&b.slug));

    Ok(WorkbookListResponse {
        workbooks: descriptors,
        next_offset: None,
    })
}

#[cfg(feature = "native-fs")]
fn derive_folder(config: &Arc<ServerConfig>, path: &Path) -> Option<String> {
    path.strip_prefix(&config.workspace_root)
        .ok()
        .and_then(|relative| relative.parent())
        .and_then(|parent| parent.file_name())
        .map(|os| os.to_string_lossy().to_string())
}

#[cfg(feature = "native-fs")]
fn has_supported_extension(allowed: &[String], path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let lower = ext.to_ascii_lowercase();
            allowed.iter().any(|candidate| candidate == &lower)
        })
        .unwrap_or(false)
}
