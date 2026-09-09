use crate::model::{
    CellSnapshot, CellValue, CellValueKind, CellValuePrimitive, DefineNameResponse,
    DeleteNameResponse, FindValueMatch, FindValueResponse, GridCell, GridColumnHint, GridPayload,
    GridRow, NamedRangesResponse, RangeValuesEntry, ReadTableResponse, RowSnapshot,
    SheetOverviewResponse, SheetPageCompact, SheetPageFormat, SheetPageResponse, SheetPageValues,
    StylePatch, TableOutputFormat, TableRow, UpdateNameResponse, Warning, WorkbookDescription,
    WorkbookId,
};
use crate::styles::descriptor_from_style;
use crate::workbook::{WorkbookContext, cell_to_value};
use anyhow::{Context, Result, anyhow};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use umya_spreadsheet::{Workbook as Spreadsheet, Worksheet};

/// Surface-agnostic in-memory workbook session.
///
/// This API avoids workbook IDs, fork handles, and MCP-specific wiring so it can
/// be reused by CLI, SDK, or WASM bindings.
pub struct WorkbookSession {
    spreadsheet: Spreadsheet,
}

#[cfg(feature = "recalc-formualizer")]
pub(crate) enum SessionEvaluatorCell {
    Value(formualizer::common::LiteralValue),
    Formula(String),
}

impl WorkbookSession {
    pub(crate) fn from_spreadsheet(spreadsheet: Spreadsheet) -> Self {
        Self { spreadsheet }
    }

    pub(crate) fn into_spreadsheet(self) -> Spreadsheet {
        self.spreadsheet
    }

    pub(crate) fn spreadsheet(&self) -> &Spreadsheet {
        &self.spreadsheet
    }

    pub(crate) fn spreadsheet_mut(&mut self) -> &mut Spreadsheet {
        &mut self.spreadsheet
    }

    /// Open a workbook session from raw XLSX bytes.
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Result<Self> {
        let workbook_bytes = bytes.as_ref();
        let cursor = std::io::Cursor::new(workbook_bytes);
        let spreadsheet = crate::xlsx_import::read_reader(cursor, true)
            .context("failed to parse workbook bytes")?;
        Ok(Self { spreadsheet })
    }

    /// Open a workbook session from a filesystem path.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path)
            .with_context(|| format!("failed to read workbook '{}'", path.display()))?;
        Self::from_bytes(bytes)
    }

    /// Return sheet names in workbook order.
    pub fn list_sheets(&self) -> Vec<String> {
        self.spreadsheet
            .sheet_collection()
            .iter()
            .map(|sheet| sheet.name().to_string())
            .collect()
    }

    /// Return workbook-level descriptor for this in-memory session.
    pub fn describe_workbook(&self) -> Result<WorkbookDescription> {
        let workbook = self.as_workbook_context()?;
        Ok(workbook.describe())
    }

    /// Return workbook defined names and table descriptors.
    pub fn named_ranges(&self) -> Result<NamedRangesResponse> {
        let workbook = self.as_workbook_context()?;
        let items = workbook.named_items()?;
        Ok(NamedRangesResponse {
            workbook_id: workbook.id.clone(),
            items,
        })
    }

    /// Define a new named range.
    pub fn define_name(
        &mut self,
        name: &str,
        refers_to: &str,
        scope: Option<&str>,
        scope_sheet_name: Option<&str>,
    ) -> Result<DefineNameResponse> {
        use crate::model::{DefineNameResponse, NamedRangeScope};

        let scope_kind = match scope {
            Some("sheet") => NamedRangeScope::Sheet,
            Some("workbook") | None => NamedRangeScope::Workbook,
            Some(other) => {
                return Err(anyhow!(
                    "invalid scope '{}': expected 'workbook' or 'sheet'",
                    other
                ));
            }
        };
        if scope_kind == NamedRangeScope::Sheet && scope_sheet_name.is_none() {
            return Err(anyhow!(
                "scope_sheet_name is required when scope is 'sheet'"
            ));
        }
        if name.trim().is_empty() {
            return Err(anyhow!("name must not be empty"));
        }
        if refers_to.trim().is_empty() {
            return Err(anyhow!("refers_to must not be empty"));
        }

        let book = &mut self.spreadsheet;

        match scope_kind {
            NamedRangeScope::Sheet => {
                let sn = scope_sheet_name.unwrap();
                let sheet_index = resolve_sheet_index_on_spreadsheet(book, sn)?;
                let sheet = book
                    .sheet_by_name_mut(sn).ok()
                    .ok_or_else(|| anyhow!("sheet '{}' not found", sn))?;
                sheet
                    .add_defined_name(name.to_string(), refers_to.to_string())
                    .map_err(|e| anyhow!("failed to add defined name: {e}"))?;
                let sheet = book
                    .sheet_by_name_mut(sn).ok()
                    .ok_or_else(|| anyhow!("sheet disappeared"))?;
                if let Some(last) = sheet.defined_names_mut().last_mut()
                    && last.name() == name
                {
                    last.set_local_sheet_id(sheet_index);
                }
            }
            NamedRangeScope::Workbook => {
                let first_sheet: String = book
                    .sheet_collection()
                    .first()
                    .map(|s| s.name().to_string())
                    .ok_or_else(|| anyhow!("workbook has no sheets"))?;
                let sheet = book
                    .sheet_by_name_mut(&first_sheet).ok()
                    .ok_or_else(|| anyhow!("sheet not found"))?;
                sheet
                    .add_defined_name(name.to_string(), refers_to.to_string())
                    .map_err(|e| anyhow!("failed to add defined name: {e}"))?;
                let sheet = book
                    .sheet_by_name_mut(&first_sheet).ok()
                    .ok_or_else(|| anyhow!("sheet disappeared"))?;
                let entry = sheet.defined_names_mut().pop();
                if let Some(entry) = entry {
                    book.add_defined_names(entry);
                }
            }
        }

        Ok(DefineNameResponse {
            workbook_id: WorkbookId("session".to_string()),
            name: name.to_string(),
            refers_to: refers_to.to_string(),
            scope_kind,
            scope_sheet_name: scope_sheet_name.map(|s| s.to_string()),
        })
    }

    /// Update an existing named range.
    pub fn update_name(
        &mut self,
        name: &str,
        refers_to: Option<&str>,
        scope: Option<&str>,
        scope_sheet_name: Option<&str>,
    ) -> Result<UpdateNameResponse> {
        use crate::model::{NamedRangeScope, UpdateNameResponse};

        let scope_kind = match scope {
            Some("sheet") => Some(NamedRangeScope::Sheet),
            Some("workbook") => Some(NamedRangeScope::Workbook),
            None => None,
            Some(other) => {
                return Err(anyhow!(
                    "invalid scope '{}': expected 'workbook' or 'sheet'",
                    other
                ));
            }
        };
        if name.trim().is_empty() {
            return Err(anyhow!("name must not be empty"));
        }

        let book = &mut self.spreadsheet;
        let mut found = false;
        let mut previous_refers_to = String::new();
        let mut effective_scope = NamedRangeScope::Workbook;
        let mut effective_sheet: Option<String> = None;

        // Workbook-level.
        if scope_kind.is_none() || scope_kind == Some(NamedRangeScope::Workbook) {
            for defined in book.defined_names_mut().iter_mut() {
                if defined.name() == name
                    && (scope_kind == Some(NamedRangeScope::Workbook)
                        || !defined.has_local_sheet_id())
                {
                    previous_refers_to = defined.address();
                    if let Some(new_addr) = refers_to {
                        defined.set_address(new_addr.to_string());
                    }
                    effective_scope = NamedRangeScope::Workbook;
                    found = true;
                    break;
                }
            }
        }

        // Sheet-level.
        if !found && (scope_kind.is_none() || scope_kind == Some(NamedRangeScope::Sheet)) {
            let sheet_names: Vec<String> = book
                .sheet_collection()
                .iter()
                .map(|s| s.name().to_string())
                .collect();
            for sn in &sheet_names {
                if let Some(filter) = scope_sheet_name
                    && !sn.eq_ignore_ascii_case(filter)
                {
                    continue;
                }
                if let Some(sheet) = book.sheet_by_name_mut(sn).ok() {
                    for defined in sheet.defined_names_mut().iter_mut() {
                        if defined.name() == name {
                            previous_refers_to = defined.address();
                            if let Some(new_addr) = refers_to {
                                defined.set_address(new_addr.to_string());
                            }
                            effective_scope = NamedRangeScope::Sheet;
                            effective_sheet = Some(sn.clone());
                            found = true;
                            break;
                        }
                    }
                }
                if found {
                    break;
                }
            }
        }

        if !found {
            return Err(anyhow!("named range '{}' not found", name));
        }

        // Umya appends address objects in set_address; rebuild the matching name so an update
        // replaces the definition instead of creating a union.
        if let Some(new_addr) = refers_to {
            match effective_scope {
                NamedRangeScope::Workbook => {
                    book.defined_names_mut()
                        .retain(|defined| defined.name() != name);
                    let first_sheet = book
                        .sheet_collection()
                        .first()
                        .map(|sheet| sheet.name().to_string())
                        .ok_or_else(|| anyhow!("workbook has no sheets"))?;
                    let sheet = book
                        .sheet_by_name_mut(&first_sheet).ok()
                        .ok_or_else(|| anyhow!("sheet disappeared"))?;
                    sheet
                        .add_defined_name(name.to_string(), new_addr.to_string())
                        .map_err(|error| anyhow!("failed to replace defined name: {error}"))?;
                    if let Some(replacement) = sheet.defined_names_mut().pop() {
                        book.add_defined_names(replacement);
                    }
                }
                NamedRangeScope::Sheet => {
                    let sheet_name = effective_sheet
                        .as_deref()
                        .ok_or_else(|| anyhow!("sheet-scoped name has no sheet"))?;
                    let sheet_index = resolve_sheet_index_on_spreadsheet(book, sheet_name)?;
                    let sheet = book
                        .sheet_by_name_mut(sheet_name).ok()
                        .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))?;
                    sheet
                        .defined_names_mut()
                        .retain(|defined| defined.name() != name);
                    sheet
                        .add_defined_name(name.to_string(), new_addr.to_string())
                        .map_err(|error| anyhow!("failed to replace defined name: {error}"))?;
                    if let Some(replacement) = sheet.defined_names_mut().last_mut() {
                        replacement.set_local_sheet_id(sheet_index);
                    }
                }
            }
        }

        let final_refers_to = refers_to
            .map(|s| s.to_string())
            .unwrap_or_else(|| previous_refers_to.clone());

        Ok(UpdateNameResponse {
            workbook_id: WorkbookId("session".to_string()),
            name: name.to_string(),
            refers_to: final_refers_to,
            scope_kind: effective_scope,
            scope_sheet_name: effective_sheet.or_else(|| scope_sheet_name.map(|s| s.to_string())),
            previous_refers_to: Some(previous_refers_to),
        })
    }

    /// Delete a named range.
    pub fn delete_name(
        &mut self,
        name: &str,
        scope: Option<&str>,
        scope_sheet_name: Option<&str>,
    ) -> Result<DeleteNameResponse> {
        use crate::model::{DeleteNameResponse, NamedRangeScope};

        let scope_kind = match scope {
            Some("sheet") => Some(NamedRangeScope::Sheet),
            Some("workbook") => Some(NamedRangeScope::Workbook),
            None => None,
            Some(other) => {
                return Err(anyhow!(
                    "invalid scope '{}': expected 'workbook' or 'sheet'",
                    other
                ));
            }
        };
        if name.trim().is_empty() {
            return Err(anyhow!("name must not be empty"));
        }

        let book = &mut self.spreadsheet;
        let mut deleted = false;

        // Workbook-level.
        if scope_kind.is_none() || scope_kind == Some(NamedRangeScope::Workbook) {
            let names = book.defined_names_mut();
            let before_len = names.len();
            names.retain(|d| d.name() != name);
            if names.len() < before_len {
                deleted = true;
            }
        }

        // Sheet-level.
        if !deleted && (scope_kind.is_none() || scope_kind == Some(NamedRangeScope::Sheet)) {
            let sheet_names: Vec<String> = book
                .sheet_collection()
                .iter()
                .map(|s| s.name().to_string())
                .collect();
            for sn in &sheet_names {
                if let Some(filter) = scope_sheet_name
                    && !sn.eq_ignore_ascii_case(filter)
                {
                    continue;
                }
                if let Some(sheet) = book.sheet_by_name_mut(sn).ok() {
                    let names = sheet.defined_names_mut();
                    let before_len = names.len();
                    names.retain(|d| d.name() != name);
                    if names.len() < before_len {
                        deleted = true;
                        break;
                    }
                }
            }
        }

        if !deleted {
            return Err(anyhow!("named range '{}' not found", name));
        }

        Ok(DeleteNameResponse {
            workbook_id: WorkbookId("session".to_string()),
            name: name.to_string(),
            deleted: true,
        })
    }

    /// Return overview/classification information for a sheet.
    pub fn sheet_overview(
        &self,
        params: SessionSheetOverviewParams,
    ) -> Result<SheetOverviewResponse> {
        let workbook = self.as_workbook_context()?;
        let mut overview = workbook.sheet_overview(&params.sheet_name)?;

        let max_regions = params.max_regions.unwrap_or(25).max(1);
        let max_headers = params.max_headers.unwrap_or(50).max(1);
        let include_headers = params.include_headers.unwrap_or(true);

        let region_limit = if params.max_regions == Some(0) {
            usize::MAX
        } else {
            max_regions as usize
        };
        let header_limit = if params.max_headers == Some(0) {
            usize::MAX
        } else {
            max_headers as usize
        };

        let total_regions = overview.detected_regions.len() as u32;
        let mut headers_truncated = false;

        for region in &mut overview.detected_regions {
            let header_count = region.header_count.max(region.headers.len() as u32);
            region.header_count = header_count;
            if !include_headers {
                region.headers.clear();
            } else if region.headers.len() > header_limit {
                region.headers.truncate(header_limit);
            }
            region.headers_truncated = region.headers.len() as u32 != header_count;
            headers_truncated |= region.headers_truncated;
        }

        let regions_truncated = if overview.detected_regions.len() > region_limit {
            overview.detected_regions.truncate(region_limit);
            true
        } else {
            false
        };

        overview.detected_region_count = total_regions;
        overview.detected_regions_truncated = regions_truncated;

        if regions_truncated {
            overview.notes.push(format!(
                "Detected regions truncated to {} ({} total).",
                region_limit, total_regions
            ));
        }
        if headers_truncated {
            overview.notes.push(format!(
                "Region headers truncated to {} columns.",
                header_limit
            ));
        }

        Ok(overview)
    }

    /// Search for values in one sheet or across all sheets.
    pub fn find_value(&self, params: SessionFindValueParams) -> Result<FindValueResponse> {
        if params.query.trim().is_empty() {
            return Err(anyhow!("query is required"));
        }

        let query = if params.case_sensitive {
            params.query.clone()
        } else {
            params.query.to_ascii_lowercase()
        };
        let offset = params.offset.unwrap_or(0);
        let limit = params.limit.max(1);

        let sheet_names: Vec<String> = if let Some(sheet_name) = params.sheet_name.as_ref() {
            vec![sheet_name.clone()]
        } else {
            self.list_sheets()
        };

        let mut seen = 0u32;
        let mut matches = Vec::new();
        let mut truncated = false;

        'outer: for sheet_name in sheet_names {
            let sheet = self.sheet_by_name_required(&sheet_name)?;
            let max_row = sheet.highest_row().max(1);
            let max_col = sheet.highest_column().max(1);

            for row in 1..=max_row {
                for col in 1..=max_col {
                    let Some(cell) = sheet.cell((col, row)) else {
                        continue;
                    };
                    let Some(value) = cell_to_value(cell) else {
                        continue;
                    };

                    let haystack = if params.case_sensitive {
                        cell_value_to_string(value.clone())
                    } else {
                        cell_value_to_string_lower(value.clone())
                    };
                    if !haystack.contains(&query) {
                        continue;
                    }

                    if seen < offset {
                        seen += 1;
                        continue;
                    }

                    if matches.len() >= limit as usize {
                        truncated = true;
                        break 'outer;
                    }

                    matches.push(FindValueMatch {
                        address: crate::utils::cell_address(col, row),
                        sheet_name: sheet_name.clone(),
                        value: Some(value),
                        row_context: None,
                        neighbors: None,
                        label_hit: None,
                    });
                    seen += 1;
                }
            }
        }

        Ok(FindValueResponse {
            workbook_id: WorkbookId("session".to_string()),
            match_count: matches.len() as u32,
            matches,
            next_offset: truncated.then_some(offset + limit),
        })
    }

    /// Read a rectangular table snapshot from a sheet.
    pub fn read_table(&self, params: SessionReadTableParams) -> Result<ReadTableResponse> {
        let calculation = self.as_workbook_context()?.calculation_metadata();
        let sheet_name = if let Some(name) = params.sheet_name.clone() {
            name
        } else {
            self.list_sheets()
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("workbook has no sheets"))?
        };
        let sheet = self.sheet_by_name_required(&sheet_name)?;

        let bounds = if let Some(range) = params.range.as_ref() {
            parse_range_bounds(range)?
        } else {
            RangeBounds {
                min_col: 1,
                min_row: 1,
                max_col: sheet.highest_column().max(1),
                max_row: sheet.highest_row().max(1),
            }
        };

        let include_headers = params.include_headers;
        let include_types = params.include_types;
        let format = params.format;
        let offset = params.offset.unwrap_or(0) as usize;
        let limit = params.limit.max(1) as usize;

        let column_indices = if let Some(columns) = params.columns.as_ref() {
            resolve_columns(Some(columns), bounds.max_col)?
                .into_iter()
                .filter(|col| *col >= bounds.min_col && *col <= bounds.max_col)
                .collect::<Vec<_>>()
        } else {
            (bounds.min_col..=bounds.max_col).collect::<Vec<_>>()
        };

        if column_indices.is_empty() {
            return Err(anyhow!("no columns selected for read_table"));
        }

        let header_row_idx = bounds.min_row;
        let mut headers: Vec<String> = column_indices
            .iter()
            .map(|col| {
                if include_headers {
                    sheet
                        .cell((*col, header_row_idx))
                        .and_then(cell_to_value)
                        .map(cell_value_to_string)
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| crate::utils::column_number_to_name(*col))
                } else {
                    crate::utils::column_number_to_name(*col)
                }
            })
            .collect();
        dedupe_headers_in_place(&mut headers);

        let data_start_row = if include_headers {
            header_row_idx.saturating_add(1)
        } else {
            bounds.min_row
        };
        let data_rows_count = if data_start_row > bounds.max_row {
            0usize
        } else {
            (bounds.max_row - data_start_row + 1) as usize
        };

        let row_start = data_start_row.saturating_add(offset as u32);
        let row_end_exclusive = row_start.saturating_add(limit as u32);

        let mut json_rows: Vec<TableRow> = Vec::new();
        let mut raw_rows: Vec<Vec<Option<CellValue>>> = Vec::new();
        let mut values_rows: Vec<Vec<Option<CellValuePrimitive>>> = Vec::new();
        let mut types_rows: Vec<Vec<Option<CellValueKind>>> = Vec::new();

        let mut row_idx = row_start;
        while row_idx <= bounds.max_row && row_idx < row_end_exclusive {
            let mut json_row = BTreeMap::new();
            let mut raw_row = Vec::new();
            let mut values_row = Vec::new();
            let mut types_row = Vec::new();

            for (idx, col) in column_indices.iter().enumerate() {
                let value = sheet.cell((*col, row_idx)).and_then(cell_to_value);
                json_row.insert(headers[idx].clone(), value.clone());
                raw_row.push(value.clone());
                values_row.push(value.as_ref().and_then(cell_value_to_primitive));
                types_row.push(value.as_ref().map(cell_value_kind));
            }

            json_rows.push(json_row);
            raw_rows.push(raw_row);
            values_rows.push(values_row);
            types_rows.push(types_row);
            row_idx = row_idx.saturating_add(1);
        }

        let next_offset = if offset + json_rows.len() < data_rows_count {
            Some((offset + json_rows.len()) as u32)
        } else {
            None
        };

        let csv = if matches!(format, TableOutputFormat::Csv) {
            Some(build_csv_payload(&headers, &raw_rows, include_headers))
        } else {
            None
        };

        Ok(ReadTableResponse {
            workbook_id: WorkbookId("session".to_string()),
            calculation,
            sheet_name,
            table_name: None,
            warnings: Vec::<Warning>::new(),
            headers: if matches!(format, TableOutputFormat::Csv) {
                Vec::new()
            } else {
                headers
            },
            rows: if matches!(format, TableOutputFormat::Json) {
                json_rows
            } else {
                Vec::new()
            },
            values: if matches!(format, TableOutputFormat::Values) {
                Some(values_rows)
            } else {
                None
            },
            types: if include_types {
                Some(types_rows)
            } else {
                None
            },
            csv,
            total_rows: data_rows_count as u32,
            next_offset,
        })
    }

    /// Read one or more A1 ranges from a sheet.
    pub fn range_values(
        &self,
        sheet_name: &str,
        ranges: impl Into<SessionRangeSelection>,
    ) -> Result<Vec<RangeValuesEntry>> {
        let sheet = self.sheet_by_name_required(sheet_name)?;
        let ranges = ranges.into().into_vec();
        if ranges.is_empty() {
            return Err(anyhow!("at least one range is required"));
        }

        let mut out = Vec::with_capacity(ranges.len());
        for range in ranges {
            let bounds = parse_range_bounds(&range)?;
            let mut rows = Vec::new();

            for row in bounds.min_row..=bounds.max_row {
                let mut row_values = Vec::new();
                for col in bounds.min_col..=bounds.max_col {
                    let value = sheet.cell((col, row)).and_then(cell_to_value);
                    row_values.push(value);
                }
                rows.push(row_values);
            }

            out.push(RangeValuesEntry {
                range,
                rows: Some(rows),
                formulas: None,
                values: None,
                dense: None,
                csv: None,
                rows_keyed: None,
                next_start_row: None,
            });
        }

        Ok(out)
    }

    /// Read a page-oriented snapshot from a sheet.
    pub fn sheet_page(&self, params: SessionSheetPageParams) -> Result<SheetPageResponse> {
        if params.page_size == 0 {
            return Err(anyhow!("page_size must be greater than zero"));
        }

        let sheet = self.sheet_by_name_required(&params.sheet_name)?;
        let start_row = params.start_row.max(1);
        let page_size = params.page_size.min(500);
        let max_row = sheet.highest_row();

        let page = build_sheet_page(
            sheet,
            start_row,
            page_size,
            params.columns.as_ref(),
            params.columns_by_header.as_ref(),
            params.include_formulas,
            params.include_styles,
            params.include_header,
        )?;

        let last_row_index = page
            .rows
            .last()
            .map(|row| row.row_index)
            .unwrap_or(start_row.saturating_sub(1));
        let next_start_row = if last_row_index < max_row {
            Some(last_row_index + 1)
        } else {
            None
        };

        Ok(build_sheet_page_response(
            WorkbookId("session".to_string()),
            params.sheet_name,
            params.format,
            params.include_header,
            page.header,
            page.rows,
            next_start_row,
        ))
    }

    /// Export a range as grid payload (value/formula/style patch surface).
    pub fn grid_export(&self, sheet_name: &str, range: &str) -> Result<GridPayload> {
        let sheet = self.sheet_by_name_required(sheet_name)?;
        let bounds = parse_range_bounds(range)?;

        let mut columns = Vec::new();
        for col_idx in bounds.min_col..=bounds.max_col {
            if let Some(dim) = sheet.column_dimension_by_number(col_idx) {
                let width = dim.width();
                if width > 0.0 {
                    columns.push(GridColumnHint {
                        offset: col_idx - bounds.min_col,
                        width_chars: width,
                    });
                }
            }
        }

        let mut merges = Vec::new();
        for merge_cell in sheet.merge_cells() {
            let merge_range = merge_cell.range();
            if let Ok(merge_bounds) = parse_range_bounds(&merge_range)
                && merge_bounds.min_col <= bounds.max_col
                && merge_bounds.max_col >= bounds.min_col
                && merge_bounds.min_row <= bounds.max_row
                && merge_bounds.max_row >= bounds.min_row
            {
                merges.push(merge_range.to_string());
            }
        }

        let mut rows = Vec::new();
        for row in bounds.min_row..=bounds.max_row {
            let mut cells = Vec::new();
            for col in bounds.min_col..=bounds.max_col {
                let Some(cell) = sheet.cell((col, row)) else {
                    continue;
                };

                let (value, formula) = if cell.is_formula() {
                    (None, Some(format!("={}", cell.formula())))
                } else {
                    (cell_to_json_value(cell_to_value(cell)), None)
                };

                let descriptor = descriptor_from_style(cell.style());
                let number_format = descriptor.number_format.clone();
                let style_patch = style_descriptor_to_patch(descriptor);

                if value.is_some()
                    || formula.is_some()
                    || number_format.is_some()
                    || style_patch.is_some()
                {
                    cells.push(GridCell {
                        offset: [row - bounds.min_row, col - bounds.min_col],
                        v: value,
                        f: formula,
                        fmt: number_format,
                        style: style_patch,
                    });
                }
            }
            if !cells.is_empty() {
                rows.push(GridRow { cells });
            }
        }

        Ok(GridPayload {
            sheet: sheet_name.to_string(),
            anchor: crate::utils::cell_address(bounds.min_col, bounds.min_row),
            columns,
            merges,
            rows,
        })
    }

    /// Apply a transform batch in-session.
    ///
    /// For this extraction pass we support `write_matrix` operations.
    pub fn apply_ops(&mut self, ops: &[SessionTransformOp]) -> Result<SessionApplySummary> {
        // Validate all operations first so failures are atomic for this batch.
        self.validate_ops(ops)?;

        let mut summary = SessionApplySummary {
            ops_applied: ops.len(),
            ..Default::default()
        };

        for op in ops {
            match op {
                SessionTransformOp::WriteMatrix {
                    sheet_name,
                    anchor,
                    rows,
                    overwrite_formulas,
                } => {
                    let sheet = self.sheet_by_name_mut(sheet_name)?;
                    let (anchor_col, anchor_row) = parse_cell_ref(anchor)?;

                    for (row_offset, row_values) in rows.iter().enumerate() {
                        let row_idx = anchor_row + row_offset as u32;
                        for (col_offset, cell_value) in row_values.iter().enumerate() {
                            let Some(cell_value) = cell_value else {
                                continue;
                            };
                            let col_idx = anchor_col + col_offset as u32;
                            let cell = sheet.cell_mut((col_idx, row_idx));
                            summary.cells_touched += 1;

                            if cell.is_formula() {
                                if !*overwrite_formulas {
                                    summary.cells_skipped_keep_formulas += 1;
                                    continue;
                                }
                                cell.set_formula(String::new());
                                summary.cells_formula_cleared += 1;
                            }

                            match cell_value {
                                SessionMatrixCell::Value(raw) => {
                                    cell.set_value(json_value_to_cell_string(raw));
                                    summary.cells_value_set += 1;
                                }
                                SessionMatrixCell::Formula(formula) => {
                                    let formula = formula.strip_prefix('=').unwrap_or(formula);
                                    cell.set_formula(formula.to_string());
                                    cell.set_formula_result_default("");
                                    summary.cells_formula_set += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(summary)
    }

    /// Convenience wrapper for a single `write_matrix` operation.
    pub fn apply_write_matrix(
        &mut self,
        sheet_name: impl Into<String>,
        anchor: impl Into<String>,
        rows: Vec<Vec<Option<SessionMatrixCell>>>,
        overwrite_formulas: bool,
    ) -> Result<SessionApplySummary> {
        let op = SessionTransformOp::WriteMatrix {
            sheet_name: sheet_name.into(),
            anchor: anchor.into(),
            rows,
            overwrite_formulas,
        };
        self.apply_ops(&[op])
    }

    /// Publish complete evaluator results into formula caches without replacing the
    /// authoritative Umya document or round-tripping through XLSX.
    #[cfg(feature = "recalc-formualizer")]
    pub(crate) fn apply_formula_cache_updates(
        &mut self,
        updates: &[formualizer::workbook::FormulaCacheUpdate],
        date_system: formualizer::eval::engine::DateSystem,
    ) -> Result<()> {
        use formualizer::common::LiteralValue;

        // Validate the complete publication set before changing any cache. With
        // `&mut self` held, these invariants cannot change between the passes.
        for update in updates {
            let sheet = self.sheet_by_name_required(&update.sheet)?;
            let cell = sheet.cell((update.col, update.row)).ok_or_else(|| {
                anyhow!(
                    "formula cache target {}!{}{} does not exist",
                    update.sheet,
                    crate::utils::column_number_to_name(update.col),
                    update.row
                )
            })?;
            if !cell.is_formula() {
                return Err(anyhow!(
                    "formula cache target {}!{}{} is not a formula",
                    update.sheet,
                    crate::utils::column_number_to_name(update.col),
                    update.row
                ));
            }
        }

        for update in updates {
            let sheet = self
                .spreadsheet
                .sheet_by_name_mut(&update.sheet).ok()
                .expect("cache target sheet was preflighted");
            let cell = sheet.cell_mut((update.col, update.row));
            match &update.value {
                LiteralValue::Empty => {
                    cell.set_formula_result_default("");
                }
                LiteralValue::Int(value) => {
                    cell.set_formula_result_number(*value as f64);
                }
                LiteralValue::Number(value) => {
                    cell.set_formula_result_number(*value);
                }
                LiteralValue::Boolean(value) => {
                    cell.set_formula_result_bool(*value);
                }
                LiteralValue::Text(value) => {
                    cell.set_formula_result_string(value.clone());
                }
                LiteralValue::Error(error) => {
                    cell.set_formula_result_default(error.kind.to_string());
                }
                LiteralValue::Date(value) => {
                    cell.set_formula_result_number(formualizer::common::datetime_to_serial_for(
                        date_system,
                        &value.and_hms_opt(0, 0, 0).expect("midnight is valid"),
                    ));
                }
                LiteralValue::DateTime(value) => {
                    cell.set_formula_result_number(formualizer::common::datetime_to_serial_for(
                        date_system,
                        value,
                    ));
                }
                LiteralValue::Time(value) => {
                    cell.set_formula_result_number(formualizer::common::time_to_fraction(value));
                }
                LiteralValue::Duration(value) => {
                    cell.set_formula_result_number(value.num_seconds() as f64 / 86_400.0);
                }
                LiteralValue::Pending | LiteralValue::Array(_) => {
                    cell.set_formula_result_default("#VALUE!");
                }
            }
        }
        Ok(())
    }

    /// Serialize the current in-memory workbook state back to XLSX bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        crate::xlsx_export::write_writer(&self.spreadsheet, &mut bytes)
            .context("failed to serialize workbook to bytes")?;
        Ok(bytes)
    }

    /// Write current workbook state to a temporary XLSX file.
    ///
    /// The caller owns the `NamedTempFile` and must keep it alive for as long as
    /// the path is needed (e.g. during apply-to-file round-trips).
    pub fn to_temp_file(&self) -> Result<crate::hostfs::NamedTempFile> {
        let bytes = self.to_bytes()?;
        let mut tmp = crate::hostfs::Builder::new()
            .suffix(".xlsx")
            .tempfile()
            .context("failed to create session temp file")?;
        std::io::Write::write_all(&mut tmp, &bytes)
            .context("failed to write workbook to temp file")?;
        Ok(tmp)
    }

    /// Reload in-memory workbook state from an on-disk XLSX file.
    ///
    /// Used after an external `apply_*_to_file()` function has mutated the file
    /// to bring the session back in sync.
    pub fn reload_from_path(&mut self, path: &Path) -> Result<()> {
        let bytes = fs::read(path)
            .with_context(|| format!("failed to read workbook from '{}'", path.display()))?;
        let cursor = std::io::Cursor::new(&bytes);
        let spreadsheet = crate::xlsx_import::read_reader(cursor, true)
            .context("failed to parse workbook after reload")?;
        self.spreadsheet = spreadsheet;
        Ok(())
    }

    /// Serialize and consume the current in-memory workbook state.
    pub fn into_bytes(self) -> Result<Vec<u8>> {
        self.to_bytes()
    }

    fn as_workbook_context(&self) -> Result<WorkbookContext<&Spreadsheet>> {
        Ok(WorkbookContext::borrowed(
            &self.spreadsheet,
            WorkbookId("session".into()),
            "session".into(),
        ))
    }

    /// Return whether a cell currently contains a formula.
    pub(crate) fn cell_is_formula(&self, sheet_name: &str, col: u32, row: u32) -> Result<bool> {
        Ok(self
            .sheet_by_name_required(sheet_name)?
            .cell((col, row))
            .is_some_and(|cell| cell.is_formula()))
    }

    /// Read the authoritative, Umya-normalized cell representation used to
    /// synchronize a retained evaluator after a committed document write.
    #[cfg(feature = "recalc-formualizer")]
    pub(crate) fn evaluator_cell(
        &self,
        sheet_name: &str,
        col: u32,
        row: u32,
    ) -> Result<SessionEvaluatorCell> {
        use formualizer::common::{ExcelError, ExcelErrorKind, LiteralValue};
        use umya_spreadsheet::CellRawValue;

        let Some(cell) = self
            .sheet_by_name_required(sheet_name)?
            .cell((col, row))
        else {
            return Ok(SessionEvaluatorCell::Value(LiteralValue::Empty));
        };
        if cell.is_formula() {
            return Ok(SessionEvaluatorCell::Formula(format!(
                "={}",
                cell.formula()
            )));
        }
        let raw = cell.raw_value();
        let value = if raw.is_error() {
            let kind = match cell.value().as_ref() {
                "#DIV/0!" => ExcelErrorKind::Div,
                "#N/A" => ExcelErrorKind::Na,
                "#NAME?" => ExcelErrorKind::Name,
                "#NULL!" => ExcelErrorKind::Null,
                "#NUM!" => ExcelErrorKind::Num,
                "#REF!" => ExcelErrorKind::Ref,
                "#VALUE!" => ExcelErrorKind::Value,
                _ => ExcelErrorKind::Value,
            };
            LiteralValue::Error(ExcelError::new(kind))
        } else {
            match raw {
                CellRawValue::Numeric(value) => LiteralValue::Number(*value),
                CellRawValue::Bool(value) => LiteralValue::Boolean(*value),
                CellRawValue::String(value) => LiteralValue::Text(value.to_string()),
                CellRawValue::RichText(value) => LiteralValue::Text(value.text().to_string()),
                CellRawValue::Lazy(value) => {
                    let value = value.as_ref();
                    if let Ok(number) = value.parse::<f64>() {
                        LiteralValue::Number(number)
                    } else if value.eq_ignore_ascii_case("TRUE") {
                        LiteralValue::Boolean(true)
                    } else if value.eq_ignore_ascii_case("FALSE") {
                        LiteralValue::Boolean(false)
                    } else {
                        LiteralValue::Text(value.to_string())
                    }
                }
                CellRawValue::Error(_) => unreachable!("handled by is_error"),
                CellRawValue::Empty => LiteralValue::Empty,
            }
        };
        Ok(SessionEvaluatorCell::Value(value))
    }

    /// Look up a sheet by name, returning `Some` if found.
    pub fn sheet_by_name(&self, sheet_name: &str) -> Option<&Worksheet> {
        self.spreadsheet.sheet_by_name(sheet_name).ok()
    }

    fn sheet_by_name_required(&self, sheet_name: &str) -> Result<&Worksheet> {
        self.spreadsheet
            .sheet_by_name(sheet_name).ok()
            .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))
    }

    fn sheet_by_name_mut(&mut self, sheet_name: &str) -> Result<&mut Worksheet> {
        self.spreadsheet
            .sheet_by_name_mut(sheet_name).ok()
            .ok_or_else(|| anyhow!("sheet '{}' not found", sheet_name))
    }

    fn validate_ops(&self, ops: &[SessionTransformOp]) -> Result<()> {
        for op in ops {
            match op {
                SessionTransformOp::WriteMatrix {
                    sheet_name, anchor, ..
                } => {
                    self.sheet_by_name_required(sheet_name)?;
                    let _ = parse_cell_ref(anchor)?;
                }
            }
        }
        Ok(())
    }
}

fn default_start_row() -> u32 {
    1
}

fn default_page_size() -> u32 {
    50
}

fn default_include_formulas() -> bool {
    true
}

fn default_include_styles() -> bool {
    false
}

fn default_include_header() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct SessionSheetOverviewParams {
    pub sheet_name: String,
    #[serde(default)]
    pub max_regions: Option<u32>,
    #[serde(default)]
    pub max_headers: Option<u32>,
    #[serde(default)]
    pub include_headers: Option<bool>,
}

fn default_find_limit() -> u32 {
    50
}

fn default_read_table_limit() -> u32 {
    100
}

fn default_read_table_include_headers() -> bool {
    true
}

fn default_read_table_include_types() -> bool {
    false
}

fn default_read_table_format() -> TableOutputFormat {
    TableOutputFormat::Csv
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionFindValueParams {
    pub query: String,
    #[serde(default)]
    pub sheet_name: Option<String>,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default = "default_find_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: Option<u32>,
}

impl Default for SessionFindValueParams {
    fn default() -> Self {
        Self {
            query: String::new(),
            sheet_name: None,
            case_sensitive: false,
            limit: default_find_limit(),
            offset: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionReadTableParams {
    #[serde(default)]
    pub sheet_name: Option<String>,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub columns: Option<Vec<String>>,
    #[serde(default = "default_read_table_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default = "default_read_table_format")]
    pub format: TableOutputFormat,
    #[serde(default = "default_read_table_include_headers")]
    pub include_headers: bool,
    #[serde(default = "default_read_table_include_types")]
    pub include_types: bool,
}

impl Default for SessionReadTableParams {
    fn default() -> Self {
        Self {
            sheet_name: None,
            range: None,
            columns: None,
            limit: default_read_table_limit(),
            offset: None,
            format: default_read_table_format(),
            include_headers: default_read_table_include_headers(),
            include_types: default_read_table_include_types(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionSheetPageParams {
    pub sheet_name: String,
    #[serde(default = "default_start_row")]
    pub start_row: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    #[serde(default)]
    pub columns: Option<Vec<String>>,
    #[serde(default)]
    pub columns_by_header: Option<Vec<String>>,
    #[serde(default = "default_include_formulas")]
    pub include_formulas: bool,
    #[serde(default = "default_include_styles")]
    pub include_styles: bool,
    #[serde(default = "default_include_header")]
    pub include_header: bool,
    #[serde(default)]
    pub format: SheetPageFormat,
}

impl SessionSheetPageParams {
    pub fn with_sheet_name(sheet_name: impl Into<String>) -> Self {
        Self {
            sheet_name: sheet_name.into(),
            ..Self::default()
        }
    }
}

impl Default for SessionSheetPageParams {
    fn default() -> Self {
        Self {
            sheet_name: String::new(),
            start_row: default_start_row(),
            page_size: default_page_size(),
            columns: None,
            columns_by_header: None,
            include_formulas: default_include_formulas(),
            include_styles: default_include_styles(),
            include_header: default_include_header(),
            format: SheetPageFormat::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum SessionRangeSelection {
    Single(String),
    Multi(Vec<String>),
}

impl SessionRangeSelection {
    fn into_vec(self) -> Vec<String> {
        match self {
            SessionRangeSelection::Single(range) => vec![range],
            SessionRangeSelection::Multi(ranges) => ranges,
        }
    }
}

impl From<String> for SessionRangeSelection {
    fn from(value: String) -> Self {
        SessionRangeSelection::Single(value)
    }
}

impl From<&str> for SessionRangeSelection {
    fn from(value: &str) -> Self {
        SessionRangeSelection::Single(value.to_string())
    }
}

impl From<Vec<String>> for SessionRangeSelection {
    fn from(value: Vec<String>) -> Self {
        SessionRangeSelection::Multi(value)
    }
}

impl From<Vec<&str>> for SessionRangeSelection {
    fn from(value: Vec<&str>) -> Self {
        SessionRangeSelection::Multi(value.into_iter().map(str::to_string).collect())
    }
}

impl From<&[String]> for SessionRangeSelection {
    fn from(value: &[String]) -> Self {
        SessionRangeSelection::Multi(value.to_vec())
    }
}

impl From<&[&str]> for SessionRangeSelection {
    fn from(value: &[&str]) -> Self {
        SessionRangeSelection::Multi(value.iter().map(|entry| entry.to_string()).collect())
    }
}

impl<const N: usize> From<[&str; N]> for SessionRangeSelection {
    fn from(value: [&str; N]) -> Self {
        SessionRangeSelection::Multi(value.into_iter().map(str::to_string).collect())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub enum SessionMatrixCell {
    #[serde(rename = "v")]
    Value(serde_json::Value),
    #[serde(rename = "f")]
    Formula(String),
}

fn default_overwrite_formulas() -> bool {
    false
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionTransformOp {
    WriteMatrix {
        sheet_name: String,
        anchor: String,
        rows: Vec<Vec<Option<SessionMatrixCell>>>,
        #[serde(default = "default_overwrite_formulas")]
        overwrite_formulas: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SessionApplySummary {
    pub ops_applied: usize,
    pub cells_touched: u64,
    pub cells_value_set: u64,
    pub cells_formula_set: u64,
    pub cells_formula_cleared: u64,
    pub cells_skipped_keep_formulas: u64,
}

#[derive(Debug, Clone, Copy)]
struct RangeBounds {
    min_col: u32,
    min_row: u32,
    max_col: u32,
    max_row: u32,
}

fn resolve_sheet_index_on_spreadsheet(
    book: &umya_spreadsheet::Workbook,
    sheet_name: &str,
) -> Result<u32> {
    for (idx, sheet) in book.sheet_collection().iter().enumerate() {
        if sheet.name() == sheet_name {
            return Ok(idx as u32);
        }
    }
    Err(anyhow!("sheet '{}' not found", sheet_name))
}

fn parse_cell_ref(cell: &str) -> Result<(u32, u32)> {
    use umya_spreadsheet::helper::coordinate::index_from_coordinate;

    let (col, row, _, _) = index_from_coordinate(cell);
    match (col, row) {
        (Some(c), Some(r)) => Ok((c, r)),
        _ => Err(anyhow!("invalid cell reference: {}", cell)),
    }
}

fn parse_range_bounds(range: &str) -> Result<RangeBounds> {
    let parts: Vec<&str> = range.split(':').collect();
    if parts.is_empty() || parts.len() > 2 {
        return Err(anyhow!(
            "invalid range '{}'; expected 'A1' or 'A1:Z99'",
            range
        ));
    }

    let start = parse_cell_ref(parts[0])?;
    let end = if parts.len() == 2 {
        parse_cell_ref(parts[1])?
    } else {
        start
    };

    Ok(RangeBounds {
        min_col: start.0.min(end.0),
        min_row: start.1.min(end.1),
        max_col: start.0.max(end.0),
        max_row: start.1.max(end.1),
    })
}

fn json_value_to_cell_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(raw) => raw.to_string(),
        serde_json::Value::Number(raw) => raw.to_string(),
        serde_json::Value::String(raw) => raw.clone(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => value.to_string(),
    }
}

fn cell_to_json_value(value: Option<crate::model::CellValue>) -> Option<serde_json::Value> {
    match value {
        Some(crate::model::CellValue::Text(text)) => Some(serde_json::Value::String(text)),
        Some(crate::model::CellValue::Number(number)) => Some(serde_json::json!(number)),
        Some(crate::model::CellValue::Bool(value)) => Some(serde_json::Value::Bool(value)),
        Some(crate::model::CellValue::Error(text)) => Some(serde_json::Value::String(text)),
        Some(crate::model::CellValue::Date(text)) => Some(serde_json::Value::String(text)),
        None => None,
    }
}

fn style_descriptor_to_patch(desc: crate::model::StyleDescriptor) -> Option<StylePatch> {
    if desc.font.is_none()
        && desc.fill.is_none()
        && desc.borders.is_none()
        && desc.alignment.is_none()
    {
        return None;
    }

    Some(StylePatch {
        font: desc.font.map(|font| {
            Some(crate::model::FontPatch {
                name: font.name.map(Some),
                size: font.size.map(Some),
                bold: font.bold.map(Some),
                italic: font.italic.map(Some),
                underline: font.underline.map(Some),
                strikethrough: font.strikethrough.map(Some),
                color: font.color.map(Some),
            })
        }),
        fill: desc.fill.map(|fill| {
            Some(match fill {
                crate::model::FillDescriptor::Pattern(pattern) => {
                    crate::model::FillPatch::Pattern(crate::model::PatternFillPatch {
                        pattern_type: pattern.pattern_type.map(Some),
                        foreground_color: pattern.foreground_color.map(Some),
                        background_color: pattern.background_color.map(Some),
                    })
                }
                crate::model::FillDescriptor::Gradient(gradient) => {
                    crate::model::FillPatch::Gradient(crate::model::GradientFillPatch {
                        degree: gradient.degree.map(Some),
                        stops: Some(
                            gradient
                                .stops
                                .into_iter()
                                .map(|stop| crate::model::GradientStopPatch {
                                    position: stop.position,
                                    color: stop.color,
                                })
                                .collect(),
                        ),
                    })
                }
            })
        }),
        borders: desc.borders.map(|borders| {
            Some(crate::model::BordersPatch {
                left: borders.left.map(|side| {
                    Some(crate::model::BorderSidePatch {
                        style: side.style.map(Some),
                        color: side.color.map(Some),
                    })
                }),
                right: borders.right.map(|side| {
                    Some(crate::model::BorderSidePatch {
                        style: side.style.map(Some),
                        color: side.color.map(Some),
                    })
                }),
                top: borders.top.map(|side| {
                    Some(crate::model::BorderSidePatch {
                        style: side.style.map(Some),
                        color: side.color.map(Some),
                    })
                }),
                bottom: borders.bottom.map(|side| {
                    Some(crate::model::BorderSidePatch {
                        style: side.style.map(Some),
                        color: side.color.map(Some),
                    })
                }),
                diagonal: borders.diagonal.map(|side| {
                    Some(crate::model::BorderSidePatch {
                        style: side.style.map(Some),
                        color: side.color.map(Some),
                    })
                }),
                vertical: borders.vertical.map(|side| {
                    Some(crate::model::BorderSidePatch {
                        style: side.style.map(Some),
                        color: side.color.map(Some),
                    })
                }),
                horizontal: borders.horizontal.map(|side| {
                    Some(crate::model::BorderSidePatch {
                        style: side.style.map(Some),
                        color: side.color.map(Some),
                    })
                }),
                diagonal_up: borders.diagonal_up.map(Some),
                diagonal_down: borders.diagonal_down.map(Some),
            })
        }),
        alignment: desc.alignment.map(|alignment| {
            Some(crate::model::AlignmentPatch {
                horizontal: alignment.horizontal.map(Some),
                vertical: alignment.vertical.map(Some),
                wrap_text: alignment.wrap_text.map(Some),
                text_rotation: alignment.text_rotation.map(Some),
            })
        }),
        number_format: None,
    })
}

struct PageBuildResult {
    rows: Vec<RowSnapshot>,
    header: Option<RowSnapshot>,
}

#[allow(clippy::too_many_arguments)]
fn build_sheet_page(
    sheet: &umya_spreadsheet::Worksheet,
    start_row: u32,
    page_size: u32,
    columns: Option<&Vec<String>>,
    columns_by_header: Option<&Vec<String>>,
    include_formulas: bool,
    include_styles: bool,
    include_header: bool,
) -> Result<PageBuildResult> {
    let max_col = sheet.highest_column();
    let end_row = start_row
        .saturating_add(page_size.saturating_sub(1))
        .min(sheet.highest_row());
    let column_indices = resolve_columns_with_headers(sheet, columns, columns_by_header, max_col)?;

    let header = if include_header {
        Some(build_row_snapshot(
            sheet,
            1,
            &column_indices,
            include_formulas,
            include_styles,
        ))
    } else {
        None
    };

    let mut rows = Vec::new();
    for row_idx in start_row..=end_row {
        rows.push(build_row_snapshot(
            sheet,
            row_idx,
            &column_indices,
            include_formulas,
            include_styles,
        ));
    }

    Ok(PageBuildResult { rows, header })
}

fn build_row_snapshot(
    sheet: &umya_spreadsheet::Worksheet,
    row_index: u32,
    columns: &[u32],
    include_formulas: bool,
    include_styles: bool,
) -> RowSnapshot {
    let mut cells = Vec::new();
    for &col in columns {
        if let Some(cell) = sheet.cell((col, row_index)) {
            cells.push(build_cell_snapshot(cell, include_formulas, include_styles));
        } else {
            let address = crate::utils::cell_address(col, row_index);
            cells.push(CellSnapshot {
                address,
                value: None,
                formula: None,
                cached_value: None,
                number_format: None,
                style_tags: Vec::new(),
                notes: Vec::new(),
            });
        }
    }

    RowSnapshot { row_index, cells }
}

fn build_cell_snapshot(
    cell: &umya_spreadsheet::Cell,
    include_formulas: bool,
    include_styles: bool,
) -> CellSnapshot {
    let address = cell.coordinate().get_coordinate();
    let value = crate::workbook::cell_to_value(cell);
    let formula = if include_formulas && cell.is_formula() {
        Some(cell.formula().to_string())
    } else {
        None
    };
    let cached_value = if cell.is_formula() {
        value.clone()
    } else {
        None
    };
    let number_format = if include_styles {
        cell.style()
            .number_format()
            .map(|fmt| fmt.format_code().to_string())
    } else {
        None
    };
    let style_tags = if include_styles {
        crate::analysis::style::tag_cell(cell)
            .map(|(_, tagging)| tagging.tags)
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    CellSnapshot {
        address,
        value,
        formula,
        cached_value,
        number_format,
        style_tags,
        notes: Vec::new(),
    }
}

fn parse_column_index(spec: &str) -> Result<u32> {
    use umya_spreadsheet::helper::coordinate::column_index_from_string;

    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("invalid column spec: empty"));
    }
    if !trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(anyhow!(
            "invalid column spec '{}'; expected letters like 'A' or 'A:C'",
            spec
        ));
    }
    if trimmed.len() > 3 {
        return Err(anyhow!(
            "invalid column spec '{}'; expected at most 3 column letters",
            spec
        ));
    }

    Ok(column_index_from_string(trimmed.to_ascii_uppercase()))
}

fn resolve_columns(columns: Option<&Vec<String>>, max_column: u32) -> Result<Vec<u32>> {
    use std::collections::BTreeSet;

    let mut indices = BTreeSet::new();
    if let Some(specs) = columns {
        for spec in specs {
            if let Some((start, end)) = spec.split_once(':') {
                let start_idx = parse_column_index(start)?;
                let end_idx = parse_column_index(end)?;
                let (min_idx, max_idx) = if start_idx <= end_idx {
                    (start_idx, end_idx)
                } else {
                    (end_idx, start_idx)
                };
                for idx in min_idx..=max_idx {
                    indices.insert(idx);
                }
            } else {
                indices.insert(parse_column_index(spec)?);
            }
        }
    } else {
        for idx in 1..=max_column.max(1) {
            indices.insert(idx);
        }
    }

    Ok(indices.into_iter().collect())
}

fn resolve_columns_with_headers(
    sheet: &umya_spreadsheet::Worksheet,
    columns: Option<&Vec<String>>,
    columns_by_header: Option<&Vec<String>>,
    max_column: u32,
) -> Result<Vec<u32>> {
    use std::collections::BTreeSet;

    if columns_by_header.is_none() {
        return resolve_columns(columns, max_column);
    }

    let mut selected: BTreeSet<u32> = if columns.is_some() {
        resolve_columns(columns, max_column)?.into_iter().collect()
    } else {
        BTreeSet::new()
    };
    let mut matched_header = false;
    let header_targets: Vec<String> = columns_by_header
        .expect("checked")
        .iter()
        .map(|h| h.trim().to_ascii_lowercase())
        .collect();

    for col_idx in 1..=max_column.max(1) {
        let header_cell = sheet.cell((col_idx, 1u32));
        let header_value = header_cell
            .and_then(cell_to_value)
            .map(cell_value_to_string_lower);
        if let Some(hval) = header_value
            && header_targets.iter().any(|target| target == &hval)
        {
            selected.insert(col_idx);
            matched_header = true;
        }
    }

    if !matched_header && columns.is_none() {
        resolve_columns(None, max_column)
    } else {
        Ok(selected.into_iter().collect())
    }
}

fn cell_value_to_string(value: CellValue) -> String {
    match value {
        CellValue::Text(s) => s,
        CellValue::Number(n) => n.to_string(),
        CellValue::Bool(b) => b.to_string(),
        CellValue::Error(e) => e,
        CellValue::Date(d) => d,
    }
}

fn cell_value_to_string_lower(value: CellValue) -> String {
    cell_value_to_string(value).to_ascii_lowercase()
}

fn cell_value_to_primitive(value: &CellValue) -> Option<CellValuePrimitive> {
    match value {
        CellValue::Text(s) => Some(CellValuePrimitive::Text(s.clone())),
        CellValue::Number(n) => Some(CellValuePrimitive::Number(*n)),
        CellValue::Bool(b) => Some(CellValuePrimitive::Bool(*b)),
        CellValue::Error(e) => Some(CellValuePrimitive::Text(e.clone())),
        CellValue::Date(d) => Some(CellValuePrimitive::Text(d.clone())),
    }
}

fn cell_value_kind(value: &CellValue) -> CellValueKind {
    match value {
        CellValue::Text(_) => CellValueKind::Text,
        CellValue::Number(_) => CellValueKind::Number,
        CellValue::Bool(_) => CellValueKind::Bool,
        CellValue::Error(_) => CellValueKind::Error,
        CellValue::Date(_) => CellValueKind::Date,
    }
}

fn build_csv_payload(
    headers: &[String],
    rows: &[Vec<Option<CellValue>>],
    include_headers: bool,
) -> String {
    fn escape_csv(value: &str) -> String {
        if value.contains(',')
            || value.contains('"')
            || value.contains('\n')
            || value.contains('\r')
        {
            format!("\"{}\"", value.replace('"', "\"\""))
        } else {
            value.to_string()
        }
    }

    let mut out = String::new();
    if include_headers {
        out.push_str(
            &headers
                .iter()
                .map(|h| escape_csv(h))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }

    for row in rows {
        let line = row
            .iter()
            .map(|cell| match cell {
                Some(CellValue::Text(s)) => escape_csv(s),
                Some(CellValue::Number(n)) => n.to_string(),
                Some(CellValue::Bool(b)) => b.to_string(),
                Some(CellValue::Error(e)) => escape_csv(e),
                Some(CellValue::Date(d)) => escape_csv(d),
                None => String::new(),
            })
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&line);
        out.push('\n');
    }

    out
}

fn dedupe_headers_in_place(headers: &mut [String]) {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for header in headers.iter_mut() {
        let base = if header.trim().is_empty() {
            "column".to_string()
        } else {
            header.clone()
        };
        let counter = counts.entry(base.clone()).or_insert(0);
        if *counter == 0 {
            *header = base;
        } else {
            *header = format!("{}_{}", base, *counter + 1);
        }
        *counter += 1;
    }
}

fn build_compact_payload(
    header: &Option<RowSnapshot>,
    rows: &[RowSnapshot],
    include_header: bool,
) -> SheetPageCompact {
    let headers = derive_headers(header, rows);
    let header_row = if include_header {
        header
            .as_ref()
            .map(|h| h.cells.iter().map(|c| c.value.clone()).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let data_rows = rows
        .iter()
        .map(|row| {
            let mut vals: Vec<Option<CellValue>> = Vec::new();
            vals.push(Some(CellValue::Number(row.row_index as f64)));
            vals.extend(row.cells.iter().map(|c| c.value.clone()));
            vals
        })
        .collect();

    SheetPageCompact {
        headers,
        header_row,
        rows: data_rows,
    }
}

fn build_values_only_payload(
    header: &Option<RowSnapshot>,
    rows: &[RowSnapshot],
    include_header: bool,
) -> SheetPageValues {
    let mut data = Vec::new();
    if include_header && let Some(h) = header {
        data.push(h.cells.iter().map(|c| c.value.clone()).collect());
    }
    for row in rows {
        data.push(row.cells.iter().map(|c| c.value.clone()).collect());
    }

    SheetPageValues { rows: data }
}

fn build_sheet_page_response(
    workbook_id: WorkbookId,
    sheet_name: String,
    format: SheetPageFormat,
    include_header: bool,
    header: Option<RowSnapshot>,
    rows: Vec<RowSnapshot>,
    next_start_row: Option<u32>,
) -> SheetPageResponse {
    let compact_payload = if matches!(format, SheetPageFormat::Compact) {
        Some(build_compact_payload(&header, &rows, include_header))
    } else {
        None
    };

    let values_only_payload = if matches!(format, SheetPageFormat::ValuesOnly) {
        Some(build_values_only_payload(&header, &rows, include_header))
    } else {
        None
    };

    let rows_payload = if matches!(format, SheetPageFormat::Full) {
        rows
    } else {
        Vec::new()
    };

    let header_row = if include_header && matches!(format, SheetPageFormat::Full) {
        header
    } else {
        None
    };

    SheetPageResponse {
        workbook_id,
        sheet_name,
        rows: rows_payload,
        next_start_row,
        header_row,
        compact: compact_payload,
        values_only: values_only_payload,
        format,
        truncated: false,
        budget: None,
    }
}

fn derive_headers(header: &Option<RowSnapshot>, rows: &[RowSnapshot]) -> Vec<String> {
    if let Some(h) = header {
        let mut headers: Vec<String> = h
            .cells
            .iter()
            .map(|c| match &c.value {
                Some(CellValue::Text(t)) => t.clone(),
                Some(CellValue::Number(n)) => n.to_string(),
                Some(CellValue::Bool(b)) => b.to_string(),
                Some(CellValue::Date(d)) => d.clone(),
                Some(CellValue::Error(e)) => e.clone(),
                None => c.address.clone(),
            })
            .collect();
        headers.insert(0, "Row".to_string());
        headers
    } else if let Some(first) = rows.first() {
        let mut headers = Vec::new();
        headers.push("Row".to_string());
        for cell in &first.cells {
            headers.push(cell.address.clone());
        }
        headers
    } else {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CellValue;
    use anyhow::Result;
    use tempfile::tempdir;

    fn workbook_bytes(setup: impl FnOnce(&mut Spreadsheet)) -> Vec<u8> {
        let mut book = umya_spreadsheet::new_file();
        setup(&mut book);

        let mut bytes = Vec::new();
        crate::xlsx_export::write_writer(&book, &mut bytes).expect("write workbook");
        bytes
    }

    fn grid_cell(payload: &GridPayload, row_offset: u32, col_offset: u32) -> Option<&GridCell> {
        payload
            .rows
            .iter()
            .flat_map(|row| row.cells.iter())
            .find(|cell| cell.offset == [row_offset, col_offset])
    }

    #[test]
    fn bytes_roundtrip_and_multi_range_reads() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            book.sheet_by_name_mut("Sheet1").ok()
                .expect("sheet")
                .cell_mut("A1")
                .set_value("hello");
            let _ = book.new_sheet("Data");
            book.sheet_by_name_mut("Data").ok()
                .expect("data sheet")
                .cell_mut("B2")
                .set_value_number(42.0);
        });

        let session = WorkbookSession::from_bytes(bytes)?;
        assert_eq!(session.list_sheets(), vec!["Sheet1", "Data"]);

        let entries = session.range_values("Data", vec!["A1:B2", "B2:B2"])?;
        assert_eq!(entries.len(), 2);

        let rows = entries[0].rows.as_ref().expect("rows");
        assert!(
            matches!(rows[1][1], Some(CellValue::Number(v)) if (v - 42.0).abs() < f64::EPSILON)
        );

        let out_bytes = session.into_bytes()?;
        let reopened = WorkbookSession::from_bytes(out_bytes)?;
        let reopened_entries = reopened.range_values("Sheet1", "A1")?;
        let reopened_rows = reopened_entries[0].rows.as_ref().expect("rows");
        assert!(matches!(
            reopened_rows[0][0],
            Some(CellValue::Text(ref value)) if value == "hello"
        ));

        Ok(())
    }

    #[test]
    fn apply_write_matrix_updates_in_session_and_roundtrips() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("before");
            sheet.cell_mut("B1").set_formula("1+1");
        });

        let mut session = WorkbookSession::from_bytes(bytes)?;

        let summary = session.apply_write_matrix(
            "Sheet1",
            "A1",
            vec![vec![
                Some(SessionMatrixCell::Value(serde_json::json!("after"))),
                Some(SessionMatrixCell::Value(serde_json::json!(99))),
            ]],
            false,
        )?;

        assert_eq!(summary.ops_applied, 1);
        assert_eq!(summary.cells_skipped_keep_formulas, 1);

        let before_overwrite = session.grid_export("Sheet1", "A1:B1")?;
        let b1_before = grid_cell(&before_overwrite, 0, 1).expect("B1 cell");
        assert_eq!(b1_before.f.as_deref(), Some("=1+1"));

        session.apply_write_matrix(
            "Sheet1",
            "B1",
            vec![vec![Some(SessionMatrixCell::Formula(
                "=SUM(1,2)".to_string(),
            ))]],
            true,
        )?;

        let after = session.grid_export("Sheet1", "A1:B1")?;
        let a1_after = grid_cell(&after, 0, 0).expect("A1 cell");
        let b1_after = grid_cell(&after, 0, 1).expect("B1 cell");
        assert_eq!(a1_after.v, Some(serde_json::json!("after")));
        assert_eq!(b1_after.f.as_deref(), Some("=SUM(1,2)"));

        let roundtrip = WorkbookSession::from_bytes(session.into_bytes()?)?;
        let persisted = roundtrip.grid_export("Sheet1", "B1")?;
        let persisted_b1 = grid_cell(&persisted, 0, 0).expect("persisted B1");
        assert_eq!(persisted_b1.f.as_deref(), Some("=SUM(1,2)"));

        Ok(())
    }

    #[test]
    fn write_matrix_default_overwrite_formulas_is_false() -> Result<()> {
        let raw = serde_json::json!({
            "kind": "write_matrix",
            "sheet_name": "Sheet1",
            "anchor": "A1",
            "rows": [[{"v": "x"}]]
        });

        let op: SessionTransformOp = serde_json::from_value(raw)?;
        match op {
            SessionTransformOp::WriteMatrix {
                overwrite_formulas, ..
            } => {
                assert!(!overwrite_formulas);
            }
        }

        Ok(())
    }

    #[test]
    fn apply_ops_is_atomic_on_validation_failure() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("before");
        });

        let mut session = WorkbookSession::from_bytes(bytes)?;

        let ops = vec![
            SessionTransformOp::WriteMatrix {
                sheet_name: "Sheet1".to_string(),
                anchor: "A1".to_string(),
                rows: vec![vec![Some(SessionMatrixCell::Value(serde_json::json!(
                    "after"
                )))]],
                overwrite_formulas: false,
            },
            SessionTransformOp::WriteMatrix {
                sheet_name: "MissingSheet".to_string(),
                anchor: "A1".to_string(),
                rows: vec![vec![Some(SessionMatrixCell::Value(serde_json::json!(
                    "bad"
                )))]],
                overwrite_formulas: false,
            },
        ];

        let err = session.apply_ops(&ops).unwrap_err();
        assert!(err.to_string().contains("MissingSheet"));

        let after = session.range_values("Sheet1", "A1")?;
        let rows = after[0].rows.as_ref().expect("rows");
        assert!(matches!(
            rows[0][0],
            Some(CellValue::Text(ref value)) if value == "before"
        ));

        Ok(())
    }

    #[test]
    fn sheet_page_full_supports_paging_and_formulas() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("Name");
            sheet.cell_mut("B1").set_value("Calc");
            sheet.cell_mut("A2").set_value("alpha");
            sheet.cell_mut("B2").set_formula("1+1");
            sheet.cell_mut("A3").set_value("beta");
            sheet.cell_mut("B3").set_value_number(7.0);
        });

        let session = WorkbookSession::from_bytes(bytes)?;
        let page = session.sheet_page(SessionSheetPageParams {
            sheet_name: "Sheet1".to_string(),
            start_row: 2,
            page_size: 1,
            include_formulas: true,
            format: SheetPageFormat::Full,
            ..SessionSheetPageParams::default()
        })?;

        assert_eq!(page.sheet_name, "Sheet1");
        assert!(matches!(page.workbook_id, WorkbookId(ref id) if id == "session"));
        assert_eq!(page.next_start_row, Some(3));
        let header = page.header_row.as_ref().expect("header row");
        assert_eq!(header.row_index, 1);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].row_index, 2);
        assert_eq!(page.rows[0].cells[1].formula.as_deref(), Some("1+1"));

        Ok(())
    }

    #[test]
    fn sheet_page_compact_respects_columns_by_header() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("Name");
            sheet.cell_mut("B1").set_value("Score");
            sheet.cell_mut("C1").set_value("Ignore");
            sheet.cell_mut("A2").set_value("alpha");
            sheet.cell_mut("B2").set_value_number(99.0);
            sheet.cell_mut("C2").set_value("x");
        });

        let session = WorkbookSession::from_bytes(bytes)?;
        let page = session.sheet_page(SessionSheetPageParams {
            sheet_name: "Sheet1".to_string(),
            start_row: 2,
            page_size: 1,
            columns_by_header: Some(vec!["score".to_string()]),
            format: SheetPageFormat::Compact,
            ..SessionSheetPageParams::default()
        })?;

        let compact = page.compact.as_ref().expect("compact payload");
        assert_eq!(compact.headers, vec!["Row", "Score"]);
        assert_eq!(compact.rows.len(), 1);
        assert!(
            matches!(compact.rows[0][0], Some(CellValue::Number(n)) if (n - 2.0).abs() < f64::EPSILON)
        );
        assert!(
            matches!(compact.rows[0][1], Some(CellValue::Number(n)) if (n - 99.0).abs() < f64::EPSILON)
        );

        Ok(())
    }

    #[test]
    fn sheet_page_rejects_invalid_column_specs() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            book.sheet_by_name_mut("Sheet1").ok()
                .expect("sheet")
                .cell_mut("A1")
                .set_value("x");
        });

        let session = WorkbookSession::from_bytes(bytes)?;
        let err = session
            .sheet_page(SessionSheetPageParams {
                sheet_name: "Sheet1".to_string(),
                columns: Some(vec!["1".to_string()]),
                ..SessionSheetPageParams::default()
            })
            .expect_err("invalid columns should error");
        assert!(err.to_string().contains("invalid column spec"));

        Ok(())
    }

    #[test]
    fn sheet_page_handles_large_start_rows_without_overflow() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            book.sheet_by_name_mut("Sheet1").ok()
                .expect("sheet")
                .cell_mut("A1")
                .set_value("x");
        });

        let session = WorkbookSession::from_bytes(bytes)?;
        let page = session.sheet_page(SessionSheetPageParams {
            sheet_name: "Sheet1".to_string(),
            start_row: u32::MAX,
            page_size: 500,
            format: SheetPageFormat::ValuesOnly,
            ..SessionSheetPageParams::default()
        })?;

        assert_eq!(page.next_start_row, None);

        Ok(())
    }

    #[test]
    fn describe_and_named_ranges_return_session_metadata() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("Revenue");
            sheet.cell_mut("A2").set_value_number(100.0);
            sheet
                .add_defined_name("TotalRevenue", "Sheet1!$A$2")
                .expect("defined name");
        });

        let session = WorkbookSession::from_bytes(bytes)?;
        let desc = session.describe_workbook()?;
        assert!(matches!(desc.workbook_id, WorkbookId(ref id) if id == "session"));
        assert!(desc.sheet_count >= 1);

        let named = session.named_ranges()?;
        assert!(matches!(named.workbook_id, WorkbookId(ref id) if id == "session"));
        assert!(named.items.iter().all(|item| !item.name.trim().is_empty()));

        Ok(())
    }

    #[test]
    fn sheet_overview_applies_region_and_header_limits() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("Name");
            sheet.cell_mut("B1").set_value("Score");
            sheet.cell_mut("A2").set_value("alpha");
            sheet.cell_mut("B2").set_value_number(10.0);
            sheet.cell_mut("A10").set_value("Name");
            sheet.cell_mut("B10").set_value("Score");
            sheet.cell_mut("A11").set_value("beta");
            sheet.cell_mut("B11").set_value_number(20.0);
        });

        let session = WorkbookSession::from_bytes(bytes)?;
        let overview = session.sheet_overview(SessionSheetOverviewParams {
            sheet_name: "Sheet1".to_string(),
            max_regions: Some(1),
            max_headers: Some(1),
            include_headers: Some(true),
        })?;

        assert_eq!(overview.sheet_name, "Sheet1");
        assert!(overview.detected_region_count >= overview.detected_regions.len() as u32);
        assert!(overview.detected_regions.len() <= 1);

        Ok(())
    }

    #[test]
    fn find_value_returns_matches_with_pagination() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("alpha");
            sheet.cell_mut("A2").set_value("alpha");
            sheet.cell_mut("A3").set_value("beta");
        });

        let session = WorkbookSession::from_bytes(bytes)?;
        let result = session.find_value(SessionFindValueParams {
            query: "alpha".to_string(),
            limit: 1,
            offset: Some(0),
            ..SessionFindValueParams::default()
        })?;

        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.next_offset, Some(1));

        Ok(())
    }

    #[test]
    fn read_table_values_mode_returns_values_and_types() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("Name");
            sheet.cell_mut("B1").set_value("Score");
            sheet.cell_mut("A2").set_value("alpha");
            sheet.cell_mut("B2").set_value_number(42.0);
        });

        let session = WorkbookSession::from_bytes(bytes)?;
        let table = session.read_table(SessionReadTableParams {
            sheet_name: Some("Sheet1".to_string()),
            range: Some("A1:B2".to_string()),
            format: TableOutputFormat::Values,
            include_headers: true,
            include_types: true,
            ..SessionReadTableParams::default()
        })?;

        assert_eq!(table.sheet_name, "Sheet1");
        assert_eq!(table.headers, vec!["Name", "Score"]);
        assert!(table.rows.is_empty());
        assert_eq!(table.values.as_ref().map(Vec::len), Some(1));
        assert_eq!(table.types.as_ref().map(Vec::len), Some(1));

        Ok(())
    }

    #[test]
    fn read_table_csv_preserves_date_and_text_values() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("Date");
            sheet.cell_mut("B1").set_value("Note");
            sheet.cell_mut("A2").set_value_number(45292.0);
            sheet
                .style_mut("A2")
                .number_format_mut()
                .set_format_code("yyyy-mm-dd");
            sheet.cell_mut("B2").set_value("ok");
        });

        let session = WorkbookSession::from_bytes(bytes)?;
        let table = session.read_table(SessionReadTableParams {
            sheet_name: Some("Sheet1".to_string()),
            range: Some("A1:B2".to_string()),
            format: TableOutputFormat::Csv,
            ..SessionReadTableParams::default()
        })?;

        assert!(table.headers.is_empty());
        let csv = table.csv.expect("csv");
        assert!(csv.contains("-"));
        assert!(csv.contains("ok"));

        Ok(())
    }

    #[test]
    fn read_table_dedupes_duplicate_headers() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("Dup");
            sheet.cell_mut("B1").set_value("Dup");
            sheet.cell_mut("A2").set_value("v1");
            sheet.cell_mut("B2").set_value("v2");
        });

        let session = WorkbookSession::from_bytes(bytes)?;
        let table = session.read_table(SessionReadTableParams {
            sheet_name: Some("Sheet1".to_string()),
            range: Some("A1:B2".to_string()),
            format: TableOutputFormat::Json,
            ..SessionReadTableParams::default()
        })?;

        assert_eq!(table.headers, vec!["Dup", "Dup_2"]);
        let row = table.rows.first().expect("row");
        assert!(row.contains_key("Dup"));
        assert!(row.contains_key("Dup_2"));

        Ok(())
    }

    #[test]
    fn from_path_loads_workbook() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("session-path.xlsx");

        let bytes = workbook_bytes(|book| {
            book.sheet_by_name_mut("Sheet1").ok()
                .expect("sheet")
                .cell_mut("C3")
                .set_value("path-load");
        });
        fs::write(&path, bytes)?;

        let session = WorkbookSession::from_path(&path)?;
        let entries = session.range_values("Sheet1", "C3")?;
        let rows = entries[0].rows.as_ref().expect("rows");
        assert!(matches!(
            rows[0][0],
            Some(CellValue::Text(ref value)) if value == "path-load"
        ));

        Ok(())
    }

    #[test]
    fn define_name_workbook_scope_roundtrips() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("Revenue");
            sheet.cell_mut("A2").set_value_number(100.0);
        });
        let mut session = WorkbookSession::from_bytes(bytes)?;

        let resp = session.define_name("TotalRev", "Sheet1!$A$2", None, None)?;
        assert_eq!(resp.name, "TotalRev");
        assert_eq!(resp.scope_kind, crate::model::NamedRangeScope::Workbook);
        assert!(resp.scope_sheet_name.is_none());

        // Roundtrip: verify the name is visible after re-read.
        let export = session.to_bytes()?;
        let session2 = WorkbookSession::from_bytes(export)?;
        let named = session2.named_ranges()?;
        assert!(
            named.items.iter().any(|item| item.name == "TotalRev"),
            "TotalRev should be visible after roundtrip"
        );

        Ok(())
    }

    #[test]
    fn define_name_sheet_scope_roundtrips() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("Revenue");
        });
        let mut session = WorkbookSession::from_bytes(bytes)?;

        let resp =
            session.define_name("LocalName", "Sheet1!$A$1", Some("sheet"), Some("Sheet1"))?;
        assert_eq!(resp.name, "LocalName");
        assert_eq!(resp.scope_kind, crate::model::NamedRangeScope::Sheet);
        assert_eq!(resp.scope_sheet_name.as_deref(), Some("Sheet1"));

        Ok(())
    }

    #[test]
    fn update_name_changes_refers_to() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("Revenue");
            sheet
                .add_defined_name("MyName", "Sheet1!$A$1")
                .expect("defined name");
        });
        let mut session = WorkbookSession::from_bytes(bytes)?;

        let resp = session.update_name("MyName", Some("Sheet1!$A$1:$B$5"), None, None)?;
        assert_eq!(resp.name, "MyName");
        assert!(resp.previous_refers_to.is_some());

        Ok(())
    }

    #[test]
    fn delete_name_removes_defined_name() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("Revenue");
            sheet
                .add_defined_name("ToDelete", "Sheet1!$A$1")
                .expect("defined name");
        });
        let mut session = WorkbookSession::from_bytes(bytes)?;

        let named_before = session.named_ranges()?;
        assert!(named_before.items.iter().any(|i| i.name == "ToDelete"));

        let resp = session.delete_name("ToDelete", None, None)?;
        assert!(resp.deleted);

        let named_after = session.named_ranges()?;
        assert!(!named_after.items.iter().any(|i| i.name == "ToDelete"));

        Ok(())
    }

    #[test]
    fn delete_name_not_found_returns_error() -> Result<()> {
        let bytes = workbook_bytes(|_| {});
        let mut session = WorkbookSession::from_bytes(bytes)?;

        let result = session.delete_name("NonExistent", None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));

        Ok(())
    }

    #[test]
    fn named_ranges_scope_metadata_populated() -> Result<()> {
        let bytes = workbook_bytes(|book| {
            let sheet = book.sheet_by_name_mut("Sheet1").ok().expect("sheet");
            sheet.cell_mut("A1").set_value("Revenue");
            sheet
                .add_defined_name("SheetLocal", "Sheet1!$A$1")
                .expect("defined name");
        });
        let session = WorkbookSession::from_bytes(bytes)?;
        let named = session.named_ranges()?;

        for item in &named.items {
            assert!(
                item.scope_kind.is_some(),
                "scope_kind should be populated for item '{}'",
                item.name
            );
        }

        Ok(())
    }
}
