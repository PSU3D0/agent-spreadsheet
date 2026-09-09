//! `agent-spreadsheet-render` — a bounded native raster renderer for
//! spreadsheet ranges.
//!
//! Two entry points, and nothing else in the flow:
//!
//! 1. [`extract_scene`] turns a `umya_spreadsheet::Worksheet` plus a bounded
//!    range into a [`Scene`] — a flat, ordered command list in absolute device
//!    pixels — collecting a [`Warning`] for everything it cannot reproduce.
//! 2. [`rasterize`] turns that scene into PNG bytes plus a [`RenderReport`].
//!
//! The crate has no filesystem, process, tokio, MCP, `image` or Typst
//! dependency, and compiles for `wasm32-unknown-unknown`. It never
//! recalculates: a formula cell with no cached value renders empty and raises
//! `formulas_unevaluated`.
//!
//! ```no_run
//! # fn main() -> Result<(), agent_spreadsheet_render::RenderError> {
//! use agent_spreadsheet_render::{RangeBounds, RasterOptions, RenderOptions};
//!
//! let book = umya_spreadsheet::new_file();
//! let sheet = book.get_sheet(&0).unwrap();
//! let bounds = RangeBounds::new(1, 13, 1, 40);
//! let scene = agent_spreadsheet_render::extract_scene(
//!     sheet,
//!     &book,
//!     &bounds,
//!     &RenderOptions::default(),
//! )?;
//! let output = agent_spreadsheet_render::rasterize(&scene, &RasterOptions::default())?;
//! assert_eq!(output.report.renderer, "native-raster/1");
//! # Ok(())
//! # }
//! ```

pub mod color;
pub mod extract;
pub mod metrics;
pub mod numfmt;
pub mod raster;
pub mod scene;
pub mod styles;
pub mod text;

pub use raster::PngLevel;
pub use scene::{Cmd, Dash, HAlign, Rect, Rgba, Scene, VAlign, Warning};
pub use styles::NormalFont;
pub use text::Fonts;

use serde::{Deserialize, Serialize};

/// The renderer identity carried in every report. Bump the suffix when the
/// pixel output changes in a way that invalidates goldens.
pub const RENDERER_ID: &str = "native-raster/1";

/// A bounded, 1-based, inclusive cell range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeBounds {
    pub first_col: u32,
    pub last_col: u32,
    pub first_row: u32,
    pub last_row: u32,
}

impl RangeBounds {
    pub const fn new(first_col: u32, last_col: u32, first_row: u32, last_row: u32) -> Self {
        Self {
            first_col,
            last_col,
            first_row,
            last_row,
        }
    }

    pub const fn columns(&self) -> u32 {
        self.last_col - self.first_col + 1
    }

    pub const fn rows(&self) -> u32 {
        self.last_row - self.first_row + 1
    }
}

impl Default for RangeBounds {
    /// `A1:M40`, the canonical default screenshot window.
    fn default() -> Self {
        Self::new(1, 13, 1, 40)
    }
}

/// Extraction options.
#[derive(Debug, Clone, Copy)]
pub struct RenderOptions<'a> {
    /// The workbook's `xl/styles.xml` part, decompressed.
    ///
    /// The Normal font is the first `<font>` of that part, and Excel derives
    /// every column print metric from it. umya does not expose the stylesheet
    /// and this crate reads no archives, so the caller supplies the bytes.
    /// Passing `None` is supported and documented: the renderer then assumes
    /// Excel's own default Normal font, Calibri 11, which yields a 6 pt column
    /// unit. A workbook whose Normal font is not Calibri 11 will render at
    /// slightly different column widths in that case.
    pub styles_xml: Option<&'a [u8]>,
    /// Draw the column-letter and row-number gutters.
    pub headings: bool,
    /// `None` follows the sheet's own `showGridLines`.
    pub gridlines: Option<bool>,
    /// Device pixels per CSS pixel. 1.0 renders at 96 dpi.
    pub scale: f32,
}

impl Default for RenderOptions<'_> {
    fn default() -> Self {
        Self {
            styles_xml: None,
            headings: true,
            gridlines: None,
            scale: 1.0,
        }
    }
}

/// Rasterization options.
#[derive(Debug, Clone, Copy, Default)]
pub struct RasterOptions {
    /// PNG zlib level. Encoding dominates render time; the default is
    /// [`PngLevel::Balanced`].
    pub png_level: PngLevel,
}

/// How faithful the render is to the workbook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fidelity {
    /// Nothing was omitted or approximated.
    Full,
    /// At least one [`Warning`] was raised.
    Partial,
}

/// The structured account of what this render did and did not reproduce.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderReport {
    pub renderer: String,
    pub fidelity: Fidelity,
    /// Sorted and deduplicated.
    pub warnings: Vec<Warning>,
}

impl RenderReport {
    pub fn from_warnings(warnings: Vec<Warning>) -> Self {
        Self {
            renderer: RENDERER_ID.to_string(),
            fidelity: if warnings.is_empty() {
                Fidelity::Full
            } else {
                Fidelity::Partial
            },
            warnings,
        }
    }
}

/// PNG bytes plus the report.
#[derive(Debug, Clone)]
pub struct RenderOutput {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub report: RenderReport,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("invalid range: {0}")]
    InvalidRange(String),
    #[error("rendered image is too large: {width}x{height}")]
    TooLarge { width: u32, height: u32 },
    #[error("png encoding failed: {0}")]
    Encode(String),
}

/// Extract a [`Scene`] for `range` from `sheet`.
///
/// `workbook` supplies the theme colour scheme; nothing else is read from it.
pub fn extract_scene(
    sheet: &umya_spreadsheet::Worksheet,
    workbook: &umya_spreadsheet::Workbook,
    range: &RangeBounds,
    options: &RenderOptions<'_>,
) -> Result<Scene, RenderError> {
    let fonts = Fonts::new();
    extract::extract(sheet, workbook, range, options, &fonts)
}

/// Rasterize a scene to PNG bytes plus a report.
pub fn rasterize(scene: &Scene, options: &RasterOptions) -> Result<RenderOutput, RenderError> {
    let fonts = Fonts::new();
    let pixmap = raster::render(scene, &fonts)?;
    let width = pixmap.width();
    let height = pixmap.height();
    let png = raster::encode_png(&pixmap, options.png_level)?;
    Ok(RenderOutput {
        png,
        width,
        height,
        report: RenderReport::from_warnings(scene.warnings.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fonts_parse_and_stay_small() {
        let fonts = Fonts::new();
        assert!(fonts.has_glyph('A'));
        assert!(fonts.has_glyph('\u{20ac}')); // euro, in the subset
        assert!(!fonts.has_glyph('\u{03b1}')); // greek alpha, outside it
        // Both faces together, per the bake-off's subset measurement.
        assert_eq!(text::embedded_font_bytes(), 149_492);
    }

    #[test]
    fn report_fidelity_follows_warnings() {
        assert_eq!(RenderReport::from_warnings(vec![]).fidelity, Fidelity::Full);
        assert_eq!(
            RenderReport::from_warnings(vec![Warning::ChartOmitted]).fidelity,
            Fidelity::Partial
        );
    }

    #[test]
    fn warning_names_are_snake_case_and_closed() {
        let all = [
            Warning::ConditionalFormatOmitted,
            Warning::ChartOmitted,
            Warning::ImageOmitted,
            Warning::FontSubstituted,
            Warning::RichTextFlattened,
            Warning::NumberFormatApproximated,
            Warning::FormulasUnevaluated,
            Warning::TextRotationOmitted,
            Warning::PatternFillApproximated,
        ];
        for warning in all {
            let json = serde_json::to_string(&warning).unwrap();
            assert_eq!(json, format!("\"{}\"", warning.as_str()));
        }
    }
}
