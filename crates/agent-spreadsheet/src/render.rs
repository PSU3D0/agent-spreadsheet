//! The native raster screenshot backend.
//!
//! A thin adapter over `agent-spreadsheet-render`: it resolves the range, hands
//! the worksheet and the workbook's `xl/styles.xml` to the renderer, and hands
//! PNG bytes back. Everything downstream — artifact persistence, MCP image
//! content, the canonical envelope — is unchanged, because this produces the
//! same thing the LibreOffice path did: bytes.
//!
//! Rendering never recalculates. The caller reports calculation state
//! separately, sourced the same way `read_cells` sources it.

use anyhow::{Result, anyhow, bail};

use crate::workbook::WorkbookContext;

pub use agent_spreadsheet_render::{Fidelity, PngLevel, RenderReport, Warning};

/// The result of one native render.
pub struct NativeScreenshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub report: RenderReport,
}

/// Parse an `A1` or `A1:B2` range into 1-based inclusive bounds.
fn bounds(range: &str) -> Result<agent_spreadsheet_render::RangeBounds> {
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
    Ok(agent_spreadsheet_render::RangeBounds::new(
        start_col.min(end_col),
        start_col.max(end_col),
        start_row.min(end_row),
        start_row.max(end_row),
    ))
}

/// Read `xl/styles.xml` out of the workbook on disk.
///
/// The renderer takes the part bytes rather than opening archives itself. This
/// is best effort by design: when the part is unreadable the renderer falls
/// back to Excel's default Normal font, Calibri 11, which is documented on
/// `RenderOptions::styles_xml`. A fork's in-memory edits never change the
/// stylesheet's first font, so reading it from the backing file is correct even
/// for a fork.
fn styles_xml(path: &std::path::Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut part = archive.by_name("xl/styles.xml").ok()?;
    let mut bytes = Vec::new();
    part.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Read `xl/styles.xml` out of workbook bytes held in memory.
///
/// The WASM adapter has no file to open: the session owns the archive bytes.
/// Handing the same part to the renderer is what keeps WASM pixels identical to
/// native pixels instead of silently falling back to Calibri 11.
pub fn styles_xml_from_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut part = archive.by_name("xl/styles.xml").ok()?;
    let mut out = Vec::new();
    part.read_to_end(&mut out).ok()?;
    Some(out)
}

/// Render one bounded sheet range to PNG bytes, in memory.
pub fn render_sheet(
    workbook: &WorkbookContext,
    sheet_name: &str,
    range: &str,
    png_level: PngLevel,
) -> Result<NativeScreenshot> {
    let styles = styles_xml(&workbook.path);
    render_sheet_with_styles(workbook, sheet_name, range, png_level, styles.as_deref())
}

/// Render with the stylesheet part supplied by the caller.
pub fn render_sheet_with_styles(
    workbook: &WorkbookContext,
    sheet_name: &str,
    range: &str,
    png_level: PngLevel,
    styles: Option<&[u8]>,
) -> Result<NativeScreenshot> {
    let range = bounds(range)?;
    let options = agent_spreadsheet_render::RenderOptions {
        styles_xml: styles,
        ..Default::default()
    };
    let scene = workbook.with_spreadsheet(|book| {
        let sheet = book
            .sheet_by_name(sheet_name).ok()
            .ok_or_else(|| anyhow!("sheet {sheet_name} not found"))?;
        agent_spreadsheet_render::extract_scene(sheet, book, &range, &options)
            .map_err(|error| anyhow!("{error}"))
    })??;
    let output = agent_spreadsheet_render::rasterize(
        &scene,
        &agent_spreadsheet_render::RasterOptions { png_level },
    )
    .map_err(|error| anyhow!("{error}"))?;
    Ok(NativeScreenshot {
        png: output.png,
        width: output.width,
        height: output.height,
        report: output.report,
    })
}

/// Parse a `fast` / `balanced` / `best` level name.
pub fn png_level_from_str(value: &str) -> Option<PngLevel> {
    match value.to_ascii_lowercase().as_str() {
        "fast" => Some(PngLevel::Fast),
        "balanced" => Some(PngLevel::Balanced),
        "best" => Some(PngLevel::Best),
        _ => None,
    }
}

/// The report's `fidelity` as its canonical snake_case name.
pub fn fidelity_name(fidelity: Fidelity) -> &'static str {
    match fidelity {
        Fidelity::Full => "full",
        Fidelity::Partial => "partial",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_bounds_normalize_and_reject() {
        let parsed = bounds("A1:C10").unwrap();
        assert_eq!(parsed.first_col, 1);
        assert_eq!(parsed.last_col, 3);
        assert_eq!(parsed.first_row, 1);
        assert_eq!(parsed.last_row, 10);
        // A single cell is a 1x1 range.
        let single = bounds("B2").unwrap();
        assert_eq!(single.rows(), 1);
        assert_eq!(single.columns(), 1);
        // Inverted input normalizes rather than erroring.
        let inverted = bounds("C10:A1").unwrap();
        assert_eq!(inverted, parsed);
        assert!(bounds("A1:B2:C3").is_err());
        assert!(bounds("not-a-range").is_err());
    }

    #[test]
    fn png_level_names_round_trip() {
        for name in ["fast", "balanced", "best"] {
            assert_eq!(png_level_from_str(name).unwrap().as_str(), name);
        }
        assert!(png_level_from_str("maximum").is_none());
    }
}
