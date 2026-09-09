#![allow(dead_code)]

//! Shared fixture plumbing for the golden and determinism suites.
//!
//! The library itself reads no files and opens no archives. Everything in this
//! module is test-only scaffolding for getting a fixture off disk and into the
//! two public entry points.

use std::io::Read;
use std::path::{Path, PathBuf};

use agent_spreadsheet_render::{
    RangeBounds, RasterOptions, RenderOptions, RenderOutput, Scene, extract_scene, rasterize,
};

/// The golden window. Every fixture's content fits inside it, and keeping it
/// tight keeps the checked-in scene JSON small.
pub const GOLDEN_RANGE: RangeBounds = RangeBounds::new(1, 6, 1, 12);

/// Every fixture, and what it is here to prove.
pub const FIXTURES: &[(&str, &str)] = &[
    (
        "gen_01_fonts",
        "seven families x regular/bold/italic/underline/strike; every non-Calibri family raises font_substituted",
    ),
    (
        "gen_02_fills",
        "solid fills and theme-coloured fills through the crate's own tint-aware resolver",
    ),
    (
        "gen_03_borders",
        "the ported border width and line-style tables, drawn as boundary-anchored bands",
    ),
    (
        "gen_04_merges",
        "merged ranges, and the ported merged_range_border edge composition",
    ),
    (
        "gen_05_numfmt",
        "the bounded number formatter, including the multi-section shapes umya mis-renders",
    ),
    (
        "gen_06_align_wrap",
        "horizontal/vertical alignment, wrapText, and the auto-fit growth a wrapped row needs",
    ),
    (
        "gen_07_colwidths",
        "the ported column metric model: declared widths quantized to integer points",
    ),
    (
        "gen_08_rowheights",
        "explicit customHeight rows are honoured verbatim, not auto-fitted",
    ),
    (
        "gen_09_hidden",
        "hidden rows and columns collapse to zero-width tracks (ours, not office2pdf's)",
    ),
    (
        "gen_10_gridlines",
        "sheetView showGridLines=false suppresses the grid but not the headings",
    ),
    (
        "gen_11_dashboard",
        "cached formula values after `asp workbook recalculate`, the 0.5727 -> 57.3% case, and the 18pt title that auto-fits to 23.25pt",
    ),
    (
        "gen_12_unicode",
        "the bake-off's unicode fixture: subset coverage plus the Greek row that used to vanish",
    ),
    ("cf_conditional_format", "conditional_format_omitted"),
    ("chart_bar", "chart_omitted"),
    (
        "unicode_wide",
        "font_substituted and the .notdef box across five scripts",
    ),
    (
        "formulas_uncached",
        "formulas_unevaluated: formulas with no cached value render empty",
    ),
    (
        "warnings_grabbag",
        "image_omitted, text_rotation_omitted, pattern_fill_approximated and number_format_approximated",
    ),
];

pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")
}

/// Read a fixture's `xl/styles.xml`. The crate takes these bytes rather than
/// opening the archive itself; this is the caller-side half of that contract.
pub fn styles_xml(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut part = archive.by_name("xl/styles.xml").ok()?;
    let mut bytes = Vec::new();
    part.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Extract and rasterize one fixture over [`GOLDEN_RANGE`].
pub fn render_fixture(name: &str) -> (Scene, RenderOutput) {
    let path = fixture_dir().join(format!("{name}.xlsx"));
    // Use the production cold import boundary; the renderer itself receives a document.
    let book = formualizer_workbook::backends::umya3::read_document_path(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    let styles = styles_xml(&path);
    let options = RenderOptions {
        styles_xml: styles.as_deref(),
        ..RenderOptions::default()
    };
    let sheet = book.get_sheet(&0).ok().expect("fixture has a first sheet");
    let scene = extract_scene(sheet, &book, &GOLDEN_RANGE, &options)
        .unwrap_or_else(|error| panic!("extracting {name}: {error}"));
    let output = rasterize(&scene, &RasterOptions::default())
        .unwrap_or_else(|error| panic!("rasterizing {name}: {error}"));
    (scene, output)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// Decode a PNG back to its raw RGBA buffer.
///
/// Goldens hash this, not the PNG container. The container's compressed
/// stream depends on which deflate backend `flate2` resolves to, and that is
/// decided by Cargo feature unification across the whole build: rendering the
/// same fixture under `cargo test -p agent-spreadsheet-render` and under
/// `cargo test --workspace` produced pixel-identical images with entirely
/// different PNG bytes, because the workspace build pulls in a crate that
/// turns on flate2's zlib backend. The renderer is deterministic; the zlib
/// stream is not something the renderer decides.
pub fn decode_pixels(png_bytes: &[u8]) -> Vec<u8> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut reader = decoder.read_info().expect("golden PNG decodes");
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buffer).expect("golden PNG frame");
    buffer.truncate(info.buffer_size());
    buffer
}

/// The stable identity of a render: the pixels and their dimensions.
pub fn pixel_signature(output: &RenderOutput) -> String {
    format!(
        "{} {}x{}",
        sha256_hex(&decode_pixels(&output.png)),
        output.width,
        output.height
    )
}

/// Set `UPDATE_GOLDENS=1` to rewrite the checked-in goldens.
pub fn updating() -> bool {
    std::env::var("UPDATE_GOLDENS").is_ok_and(|v| v == "1")
}
