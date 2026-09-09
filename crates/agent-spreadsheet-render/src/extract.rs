//! `umya_spreadsheet::Worksheet` -> [`Scene`], for a bounded range.
//!
//! Determinism rules this module obeys, all of them pinned by the 5003
//! bake-off:
//!
//! * Nothing aggregates over `Worksheet::get_cell_collection()`. Its iteration
//!   order is not stable across runs of the same binary, and the spike's
//!   "dominant font" heuristic over it produced a different column unit and a
//!   different image width between runs of the same fixture. The Normal font
//!   comes from `xl/styles.xml` instead (see [`crate::styles`]).
//! * Every traversal is an explicit `for row in .. { for col in .. }` over the
//!   requested bounds, so command order is a function of the range alone.
//! * Nothing recalculates. A formula cell without a cached value renders empty
//!   and raises `formulas_unevaluated` once per sheet.

use umya_spreadsheet::{Workbook as Spreadsheet, Worksheet, structs::drawing::Theme};

use crate::color;
use crate::metrics::{
    self, EXCEL_DEFAULT_ROW_HEIGHT_PT, PAD_BOTTOM_PT, PAD_LEFT_PT, PAD_RIGHT_PT, PAD_TOP_PT,
    PX_PER_PT,
};
use crate::numfmt;
use crate::raster::LINE_HEIGHT_FACTOR;
use crate::scene::{Cmd, Dash, HAlign, Rect, Rgba, Scene, VAlign, Warning};
use crate::styles::{self, NormalFont};
use crate::text::Fonts;
use crate::{RangeBounds, RenderError, RenderOptions};

/// Row heading gutter width and column heading strip height, in px at scale 1.
pub const HEADING_ROW_WIDTH_PX: f32 = 44.0;
pub const HEADING_COL_HEIGHT_PX: f32 = 20.0;
const HEADING_FONT_PX: f32 = 11.0;

/// Row auto-fit: Carlito's `usWinAscent + usWinDescent` over its em, which is
/// the box Excel sizes an auto row to, plus two device pixels of leading.
/// Calibri 11 -> 20 px -> 15.00 pt, Excel's default row height; Calibri 18 ->
/// 31 px -> 23.25 pt, which is exactly what Excel auto-fits an 18 pt title to.
/// The spike had no auto-fit at all and clipped 18 pt titles into 15 pt rows.
const AUTOFIT_EM_FACTOR: f32 = 1.2;
const AUTOFIT_PAD_PX: f32 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Merge {
    first_col: u32,
    first_row: u32,
    last_col: u32,
    last_row: u32,
}

impl Merge {
    fn contains(&self, col: u32, row: u32) -> bool {
        col >= self.first_col
            && col <= self.last_col
            && row >= self.first_row
            && row <= self.last_row
    }
}

#[derive(Debug, Clone, Copy)]
struct Track {
    index: u32,
    offset: f32,
    size: f32,
    hidden: bool,
}

#[derive(Debug, Clone, Copy)]
struct Edge {
    width_px: f32,
    dash: Dash,
    color: Rgba,
}

#[derive(Debug, Clone)]
struct CellPlan {
    col: u32,
    row: u32,
    merge: Option<Merge>,
    fill: Option<Rgba>,
    top: Option<Edge>,
    bottom: Option<Edge>,
    left: Option<Edge>,
    right: Option<Edge>,
    text: String,
    size_px: f32,
    color: Rgba,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    halign: HAlign,
    valign: VAlign,
    wrap: bool,
}

/// Extract a scene for `bounds`. See [`crate::extract_scene`].
pub fn extract(
    sheet: &Worksheet,
    workbook: &Spreadsheet,
    bounds: &RangeBounds,
    options: &RenderOptions<'_>,
    fonts: &Fonts,
) -> Result<Scene, RenderError> {
    if bounds.first_col == 0 || bounds.first_row == 0 {
        return Err(RenderError::InvalidRange(
            "range bounds are 1-based".to_string(),
        ));
    }
    if bounds.last_col < bounds.first_col || bounds.last_row < bounds.first_row {
        return Err(RenderError::InvalidRange(
            "range bounds are inverted".to_string(),
        ));
    }
    let scale = options.scale;
    if !(scale.is_finite() && scale > 0.0) {
        return Err(RenderError::InvalidRange(
            "scale must be positive".to_string(),
        ));
    }

    let mut scene = Scene::default();
    let theme = Some(workbook.theme());
    let normal = options
        .styles_xml
        .and_then(styles::extract_normal_font)
        .unwrap_or_default();
    let unit_pt = metrics::column_unit_pt(&normal.family, normal.size_pt);

    let columns = build_column_tracks(sheet, bounds, unit_pt, scale);
    let plans = plan_cells(
        sheet, bounds, &columns, &normal, theme, fonts, scale, &mut scene,
    );
    let rows = build_row_tracks(sheet, bounds, &columns, &plans, fonts, scale);

    let origin_x = if options.headings {
        HEADING_ROW_WIDTH_PX * scale
    } else {
        0.0
    };
    let origin_y = if options.headings {
        HEADING_COL_HEIGHT_PX * scale
    } else {
        0.0
    };
    let grid_width = columns.last().map_or(0.0, |t| t.offset + t.size);
    let grid_height = rows.last().map_or(0.0, |t| t.offset + t.size);
    scene.width = origin_x + grid_width;
    scene.height = origin_y + grid_height;

    // 1. background
    scene.cmds.push(Cmd::FillRect {
        rect: Rect::new(0.0, 0.0, scene.width, scene.height),
        color: Rgba::WHITE,
    });

    let cell_rect = |plan: &CellPlan| -> Option<Rect> {
        match plan.merge {
            Some(merge) => {
                let first_col = columns.iter().find(|t| t.index == merge.first_col)?;
                let first_row = rows.iter().find(|t| t.index == merge.first_row)?;
                let last_col = columns
                    .iter()
                    .find(|t| t.index == merge.last_col.min(bounds.last_col))?;
                let last_row = rows
                    .iter()
                    .find(|t| t.index == merge.last_row.min(bounds.last_row))?;
                Some(Rect::new(
                    origin_x + first_col.offset,
                    origin_y + first_row.offset,
                    (last_col.offset + last_col.size) - first_col.offset,
                    (last_row.offset + last_row.size) - first_row.offset,
                ))
            }
            None => {
                let col = columns.iter().find(|t| t.index == plan.col)?;
                let row = rows.iter().find(|t| t.index == plan.row)?;
                Some(Rect::new(
                    origin_x + col.offset,
                    origin_y + row.offset,
                    col.size,
                    row.size,
                ))
            }
        }
    };

    // 2. fills
    for plan in &plans {
        let (Some(fill), Some(rect)) = (plan.fill, cell_rect(plan)) else {
            continue;
        };
        scene.cmds.push(Cmd::FillRect { rect, color: fill });
    }

    // 3. gridlines
    let show_gridlines = options.gridlines.unwrap_or_else(|| {
        sheet
            .sheets_views()
            .sheet_view_list()
            .first()
            .map(|view| view.show_grid_lines())
            .unwrap_or(true)
    });
    if show_gridlines {
        for track in columns.iter().filter(|t| !t.hidden) {
            let x = origin_x + track.offset;
            scene
                .cmds
                .push(vertical_gridline(x, origin_y, scene.height));
        }
        scene.cmds.push(vertical_gridline(
            origin_x + grid_width,
            origin_y,
            scene.height,
        ));
        for track in rows.iter().filter(|t| !t.hidden) {
            let y = origin_y + track.offset;
            scene
                .cmds
                .push(horizontal_gridline(y, origin_x, scene.width));
        }
        scene.cmds.push(horizontal_gridline(
            origin_y + grid_height,
            origin_x,
            scene.width,
        ));
    }

    // 4. borders. office2pdf's model: a border is a filled band anchored to
    // the grid boundary, not a stroke centred on it, so each edge is offset
    // inward by half its band width before it becomes a `Line`.
    for plan in &plans {
        let Some(rect) = cell_rect(plan) else {
            continue;
        };
        if let Some(edge) = plan.top {
            scene.cmds.push(band(
                rect.x,
                rect.y + edge.width_px / 2.0,
                rect.x + rect.w,
                rect.y + edge.width_px / 2.0,
                edge,
            ));
        }
        if let Some(edge) = plan.bottom {
            let y = rect.y + rect.h - edge.width_px / 2.0;
            scene.cmds.push(band(rect.x, y, rect.x + rect.w, y, edge));
        }
        if let Some(edge) = plan.left {
            let x = rect.x + edge.width_px / 2.0;
            scene.cmds.push(band(x, rect.y, x, rect.y + rect.h, edge));
        }
        if let Some(edge) = plan.right {
            let x = rect.x + rect.w - edge.width_px / 2.0;
            scene.cmds.push(band(x, rect.y, x, rect.y + rect.h, edge));
        }
    }

    // 5. text
    let pad_left = (PAD_LEFT_PT as f32) * PX_PER_PT * scale;
    let pad_right = (PAD_RIGHT_PT as f32) * PX_PER_PT * scale;
    let pad_top = (PAD_TOP_PT as f32) * PX_PER_PT * scale;
    let pad_bottom = (PAD_BOTTOM_PT as f32) * PX_PER_PT * scale;
    for plan in &plans {
        if plan.text.is_empty() {
            continue;
        }
        let Some(rect) = cell_rect(plan) else {
            continue;
        };
        let inner = Rect::new(
            rect.x + pad_left,
            rect.y + pad_top,
            (rect.w - pad_left - pad_right).max(0.0),
            (rect.h - pad_top - pad_bottom).max(0.0),
        );
        let clip = spill_clip(plan, rect, &columns, sheet, fonts, inner.w);
        scene.cmds.push(Cmd::Text {
            rect: inner,
            text: plan.text.clone(),
            size_px: plan.size_px,
            color: plan.color,
            bold: plan.bold,
            italic: plan.italic,
            underline: plan.underline,
            strike: plan.strike,
            halign: plan.halign,
            valign: plan.valign,
            wrap: plan.wrap,
            clip,
        });
    }

    // 6. headings last, so they stay legible over anything that spilled.
    if options.headings {
        push_headings(&mut scene, &columns, &rows, origin_x, origin_y, scale);
    }

    if !sheet.conditional_formatting_collection().is_empty() {
        scene.warn(Warning::ConditionalFormatOmitted);
    }
    if !sheet.image_collection().is_empty() {
        scene.warn(Warning::ImageOmitted);
    }
    if !sheet.chart_collection().is_empty() {
        scene.warn(Warning::ChartOmitted);
    }
    Ok(scene)
}

fn vertical_gridline(x: f32, top: f32, bottom: f32) -> Cmd {
    Cmd::Line {
        x1: x,
        y1: top,
        x2: x,
        y2: bottom,
        width: 1.0,
        color: Rgba::GRIDLINE,
        dash: Dash::Solid,
    }
}

fn horizontal_gridline(y: f32, left: f32, right: f32) -> Cmd {
    Cmd::Line {
        x1: left,
        y1: y,
        x2: right,
        y2: y,
        width: 1.0,
        color: Rgba::GRIDLINE,
        dash: Dash::Solid,
    }
}

fn band(x1: f32, y1: f32, x2: f32, y2: f32, edge: Edge) -> Cmd {
    Cmd::Line {
        x1,
        y1,
        x2,
        y2,
        width: edge.width_px,
        color: edge.color,
        dash: edge.dash,
    }
}

fn build_column_tracks(
    sheet: &Worksheet,
    bounds: &RangeBounds,
    unit_pt: f64,
    scale: f32,
) -> Vec<Track> {
    let properties = sheet.sheet_format_properties();
    // office2pdf's `> 0` guards: umya reports 0 for an absent attribute, a
    // width Excel never writes.
    let declared_default = {
        let value = properties.default_column_width();
        (value > 0.0).then_some(value)
    };
    let base = {
        let value = properties.base_column_width();
        (value > 0).then_some(value)
    };
    let default_pt = metrics::default_column_width_pt(declared_default, base, unit_pt);

    let mut tracks = Vec::new();
    let mut x = 0.0f32;
    for index in bounds.first_col..=bounds.last_col {
        let dimension = sheet.column_dimension_by_number(index);
        let (width_pt, hidden) = match dimension {
            Some(dimension) => {
                let width = dimension.width();
                let width_pt = if width > 0.0 {
                    metrics::column_width_to_pt(width, unit_pt)
                } else {
                    default_pt
                };
                (width_pt, dimension.hidden())
            }
            None => (default_pt, false),
        };
        // Hidden columns are ours, not office2pdf's: it has no `hidden`
        // handling at all. A hidden track is a zero-width track.
        let size = if hidden {
            0.0
        } else {
            (width_pt as f32) * PX_PER_PT * scale
        };
        tracks.push(Track {
            index,
            offset: x,
            size,
            hidden,
        });
        x += size;
    }
    tracks
}

fn build_row_tracks(
    sheet: &Worksheet,
    bounds: &RangeBounds,
    columns: &[Track],
    plans: &[CellPlan],
    fonts: &Fonts,
    scale: f32,
) -> Vec<Track> {
    let properties = sheet.sheet_format_properties();
    let default_pt = {
        let value = properties.default_row_height();
        if value > 0.0 {
            value
        } else {
            EXCEL_DEFAULT_ROW_HEIGHT_PT
        }
    };
    let mut tracks = Vec::new();
    let mut y = 0.0f32;
    for index in bounds.first_row..=bounds.last_row {
        let dimension = sheet.row_dimension(index);
        let hidden = dimension.map(|d| d.hidden()).unwrap_or(false);
        let recorded = dimension.map(|d| d.height()).filter(|h| *h > 0.0);
        let custom = dimension.map(|d| d.custom_height()).unwrap_or(false);
        let base_px = (recorded.unwrap_or(default_pt) as f32) * PX_PER_PT * scale;
        // A recorded height with `customHeight` is the user's; anything else
        // is Excel auto-fitting the row to its tallest content, which is what
        // the bake-off found the spike missing.
        let height_px = if custom && recorded.is_some() {
            base_px
        } else {
            let mut needed = base_px;
            for plan in plans.iter().filter(|p| p.row == index) {
                needed = needed.max(autofit_px(plan, columns, fonts));
            }
            needed
        };
        let size = if hidden { 0.0 } else { height_px };
        tracks.push(Track {
            index,
            offset: y,
            size,
            hidden,
        });
        y += size;
    }
    tracks
}

/// The height one cell's content needs, in device px.
fn autofit_px(plan: &CellPlan, columns: &[Track], fonts: &Fonts) -> f32 {
    if plan.text.is_empty() {
        return 0.0;
    }
    let mut lines = 1usize;
    // office2pdf `cell_wraps_past_one_line`, with the crude
    // `estimate_text_width_pt` replaced by real ab_glyph measurement.
    if plan.wrap {
        let available =
            merged_width(plan, columns) - ((PAD_LEFT_PT + PAD_RIGHT_PT) as f32) * PX_PER_PT;
        if available > 0.0 {
            lines =
                crate::raster::wrap_lines(&plan.text, available, plan.size_px, plan.bold, fonts)
                    .len();
        }
    } else if plan.text.contains('\n') {
        lines = 1;
    }
    // A merged cell carries its own height across its member rows; do not
    // grow a single row to fit it.
    if plan.merge.is_some_and(|m| m.last_row != m.first_row) {
        return 0.0;
    }
    (plan.size_px * AUTOFIT_EM_FACTOR * lines as f32).ceil() + AUTOFIT_PAD_PX
}

fn merged_width(plan: &CellPlan, columns: &[Track]) -> f32 {
    match plan.merge {
        Some(merge) => columns
            .iter()
            .filter(|t| t.index >= merge.first_col && t.index <= merge.last_col)
            .map(|t| t.size)
            .sum(),
        None => columns
            .iter()
            .find(|t| t.index == plan.col)
            .map_or(0.0, |t| t.size),
    }
}

fn merges(sheet: &Worksheet) -> Vec<Merge> {
    sheet
        .merge_cells()
        .iter()
        .map(|range| {
            let first_col = range
                .coordinate_start_col()
                .map(|c| c.num())
                .unwrap_or(1);
            let first_row = range
                .coordinate_start_row()
                .map(|c| c.num())
                .unwrap_or(1);
            Merge {
                first_col,
                first_row,
                last_col: range
                    .coordinate_end_col()
                    .map(|c| c.num())
                    .unwrap_or(first_col),
                last_row: range
                    .coordinate_end_row()
                    .map(|c| c.num())
                    .unwrap_or(first_row),
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn plan_cells(
    sheet: &Worksheet,
    bounds: &RangeBounds,
    columns: &[Track],
    normal: &NormalFont,
    theme: Option<&Theme>,
    fonts: &Fonts,
    scale: f32,
    scene: &mut Scene,
) -> Vec<CellPlan> {
    let merge_list = merges(sheet);
    let mut plans = Vec::new();
    for row in bounds.first_row..=bounds.last_row {
        for col in bounds.first_col..=bounds.last_col {
            if columns
                .iter()
                .find(|t| t.index == col)
                .is_some_and(|t| t.hidden)
            {
                continue;
            }
            let merge = merge_list.iter().find(|m| m.contains(col, row)).copied();
            if let Some(merge) = merge
                && (merge.first_col != col || merge.first_row != row)
            {
                continue; // interior of a merge; the top-left member owns it
            }
            let Some(cell) = sheet.cell((col, row)) else {
                continue;
            };
            let style = cell.style();

            // Fill. office2pdf reads the pattern fill directly rather than
            // `Style::get_background_color()`, which hands back `fgColor`
            // whatever the pattern type is, collapsing every hatch onto a
            // solid foreground.
            let mut fill = None;
            if let Some(pattern) = style.fill().and_then(|f| f.pattern_fill()) {
                let pattern_type = pattern.pattern_type();
                let coverage = metrics::pattern_ink_coverage(pattern_type);
                if coverage > 0.0 {
                    let foreground = pattern
                        .foreground_color()
                        .and_then(|c| color::resolve(c, theme))
                        .unwrap_or(Rgba::BLACK);
                    fill = Some(if coverage >= 1.0 {
                        foreground
                    } else {
                        scene.warn(Warning::PatternFillApproximated);
                        // An omitted `bgColor` is white.
                        let background = pattern
                            .background_color()
                            .and_then(|c| color::resolve(c, theme))
                            .unwrap_or(Rgba::WHITE);
                        metrics::blend_color(background, foreground, coverage)
                    });
                }
            }

            let (top, bottom, left, right) = match merge {
                Some(merge) => merged_range_border(sheet, &merge, theme, scale),
                None => cell_borders(cell, theme, scale),
            };

            // Text. Never recalculated: a formula cell with no cached value
            // renders empty and declares it.
            let raw = cell.cell_value();
            let has_formula = !cell.formula().is_empty();
            let empty_value = matches!(raw.raw_value(), umya_spreadsheet::CellRawValue::Empty);
            if has_formula && empty_value {
                scene.warn(Warning::FormulasUnevaluated);
            }
            if matches!(
                raw.raw_value(),
                umya_spreadsheet::CellRawValue::RichText(_)
            ) {
                scene.warn(Warning::RichTextFlattened);
            }
            let format_code = style
                .number_format()
                .map(|f| f.format_code().to_string())
                .unwrap_or_default();
            let (text, format_color) = render_value(cell, &format_code, scene);

            let font = style.font();
            let (bold, italic, underline, strike, size_pt, font_color, family) = match font {
                Some(font) => (
                    font.bold(),
                    font.italic(),
                    !matches!(
                        font.font_underline().val(),
                        umya_spreadsheet::UnderlineValues::None
                    ),
                    font.strikethrough(),
                    {
                        let size = font.size();
                        if size > 0.0 { size } else { normal.size_pt }
                    },
                    color::resolve(font.color(), theme).unwrap_or(Rgba::BLACK),
                    {
                        let name = font.name();
                        if name.is_empty() {
                            normal.family.clone()
                        } else {
                            name.to_string()
                        }
                    },
                ),
                None => (
                    false,
                    false,
                    false,
                    false,
                    normal.size_pt,
                    Rgba::BLACK,
                    normal.family.clone(),
                ),
            };
            // Only Carlito is embedded, so any other family is a substitution,
            // and so is any codepoint outside the subset.
            if !text.is_empty() && (!is_carlito_metric(&family) || fonts.has_missing_glyphs(&text))
            {
                scene.warn(Warning::FontSubstituted);
            }

            let (explicit_halign, valign, wrap) = match style.alignment() {
                Some(alignment) => {
                    use umya_spreadsheet::{
                        HorizontalAlignmentValues as H, VerticalAlignmentValues as V,
                    };
                    let horizontal = match alignment.horizontal() {
                        H::Center | H::CenterContinuous => Some(HAlign::Center),
                        H::Right => Some(HAlign::Right),
                        H::Left | H::Justify => Some(HAlign::Left),
                        _ => None,
                    };
                    let vertical = match alignment.vertical() {
                        V::Center => VAlign::Center,
                        V::Top => VAlign::Top,
                        _ => VAlign::Bottom,
                    };
                    if alignment.text_rotation() != 0 {
                        scene.warn(Warning::TextRotationOmitted);
                    }
                    (horizontal, vertical, alignment.wrap_text())
                }
                None => (None, VAlign::Bottom, false),
            };
            // Excel's "general" horizontal default: numbers right, text left.
            let numeric = cell.value_number().is_some();
            let halign =
                explicit_halign.unwrap_or(if numeric { HAlign::Right } else { HAlign::Left });

            if fill.is_none()
                && top.is_none()
                && bottom.is_none()
                && left.is_none()
                && right.is_none()
                && text.is_empty()
            {
                continue;
            }

            plans.push(CellPlan {
                col,
                row,
                merge,
                fill,
                top,
                bottom,
                left,
                right,
                text,
                size_px: (size_pt as f32) * PX_PER_PT * scale,
                color: format_color.unwrap_or(font_color),
                bold,
                italic,
                underline,
                strike,
                halign,
                valign,
                wrap,
            });
        }
    }
    plans
}

/// Carlito is metrically compatible with Calibri; every other family renders
/// in Carlito and declares `font_substituted`.
fn is_carlito_metric(family: &str) -> bool {
    family.is_empty()
        || family.eq_ignore_ascii_case("Calibri")
        || family.eq_ignore_ascii_case("Carlito")
}

/// Format the cached value. Falls back to umya only for grammar the bounded
/// formatter declines, and declares the fallback when it does.
fn render_value(
    cell: &umya_spreadsheet::Cell,
    format_code: &str,
    scene: &mut Scene,
) -> (String, Option<Rgba>) {
    let raw = cell.cell_value();
    match raw.raw_value() {
        umya_spreadsheet::CellRawValue::Empty => (String::new(), None),
        umya_spreadsheet::CellRawValue::Numeric(value) => {
            if format_code.is_empty() || format_code.eq_ignore_ascii_case("general") {
                return (numfmt::general(*value), None);
            }
            match numfmt::format_number(*value, format_code) {
                Some(formatted) => (formatted.text, formatted.color),
                None => {
                    scene.warn(Warning::NumberFormatApproximated);
                    (cell.formatted_value(), None)
                }
            }
        }
        _ => {
            let text = raw.value().to_string();
            if format_code.is_empty() || format_code.eq_ignore_ascii_case("general") {
                return (text, None);
            }
            match numfmt::format_text(&text, format_code) {
                Some(formatted) => (formatted.text, formatted.color),
                None => {
                    scene.warn(Warning::NumberFormatApproximated);
                    (cell.formatted_value(), None)
                }
            }
        }
    }
}

fn edge_of(border: &umya_spreadsheet::Border, theme: Option<&Theme>, scale: f32) -> Option<Edge> {
    let style = border.border_style();
    let width_pt = metrics::border_style_to_width(style)?;
    Some(Edge {
        width_px: width_pt * PX_PER_PT * scale,
        dash: metrics::border_style_to_line_style(style),
        color: border.color().as_ref().and_then(|value| color::resolve(value, theme)).unwrap_or(Rgba::BLACK),
    })
}

type Edges = (Option<Edge>, Option<Edge>, Option<Edge>, Option<Edge>);

fn cell_borders(cell: &umya_spreadsheet::Cell, theme: Option<&Theme>, scale: f32) -> Edges {
    let Some(borders) = cell.style().borders() else {
        return (None, None, None, None);
    };
    (
        edge_of(borders.top(), theme, scale),
        edge_of(borders.bottom(), theme, scale),
        edge_of(borders.left(), theme, scale),
        edge_of(borders.right(), theme, scale),
    )
}

/// office2pdf `merged_range_border`.
///
/// Excel writes a merged range's border format onto its constituent cells, so
/// a rule under a two-row header lands on the *bottom* row's cells and a rule
/// down the right-hand side lands on the right column's — neither of which the
/// top-left member records. Each edge is composed from the first member along
/// that edge which declares it.
fn merged_range_border(
    sheet: &Worksheet,
    merge: &Merge,
    theme: Option<&Theme>,
    scale: f32,
) -> Edges {
    // umya does not export the `Borders` type, so the side is selected by
    // index into the tuple `cell_borders` already returns.
    let side = |cells: &mut dyn Iterator<Item = (u32, u32)>, index: usize| -> Option<Edge> {
        cells
            .filter_map(|(col, row)| {
                let edges = cell_borders(sheet.cell((col, row))?, theme, scale);
                match index {
                    0 => edges.0,
                    1 => edges.1,
                    2 => edges.2,
                    _ => edges.3,
                }
            })
            .next()
    };
    (
        side(
            &mut (merge.first_col..=merge.last_col).map(|c| (c, merge.first_row)),
            0,
        ),
        side(
            &mut (merge.first_col..=merge.last_col).map(|c| (c, merge.last_row)),
            1,
        ),
        side(
            &mut (merge.first_row..=merge.last_row).map(|r| (merge.first_col, r)),
            2,
        ),
        side(
            &mut (merge.first_row..=merge.last_row).map(|r| (merge.last_col, r)),
            3,
        ),
    )
}

/// office2pdf `compute_spill_width`, with `estimate_text_width_pt` replaced by
/// real ab_glyph measurement.
///
/// `wrapText="false"` means exactly that: the text never moves to a second
/// line. What varies is only how far it may paint before being clipped. A
/// general/left cell paints on across consecutive empty neighbours to its
/// right; a centred or right-aligned cell, or one with nowhere to go, is
/// clipped at its own edge. A merged cell never paints past the merge edge.
fn spill_clip(
    plan: &CellPlan,
    rect: Rect,
    columns: &[Track],
    sheet: &Worksheet,
    fonts: &Fonts,
    available: f32,
) -> Rect {
    let own = Rect::new(rect.x, rect.y, rect.w, rect.h);
    if plan.wrap || plan.merge.is_some() || plan.text.contains('\n') {
        return own;
    }
    if fonts.measure(&plan.text, plan.size_px, plan.bold) <= available {
        return own;
    }
    if plan.halign != HAlign::Left {
        return own;
    }
    let mut extra = 0.0f32;
    for track in columns.iter().filter(|t| t.index > plan.col && !t.hidden) {
        let occupied = sheet
            .cell((track.index, plan.row))
            .map(|cell| {
                !matches!(
                    cell.cell_value().raw_value(),
                    umya_spreadsheet::CellRawValue::Empty
                )
            })
            .unwrap_or(false);
        if occupied {
            break;
        }
        extra += track.size;
    }
    Rect::new(own.x, own.y, own.w + extra, own.h)
}

fn push_headings(
    scene: &mut Scene,
    columns: &[Track],
    rows: &[Track],
    origin_x: f32,
    origin_y: f32,
    scale: f32,
) {
    scene.cmds.push(Cmd::FillRect {
        rect: Rect::new(0.0, 0.0, scene.width, origin_y),
        color: Rgba::HEADING_BG,
    });
    scene.cmds.push(Cmd::FillRect {
        rect: Rect::new(0.0, 0.0, origin_x, scene.height),
        color: Rgba::HEADING_BG,
    });
    for track in columns.iter().filter(|t| !t.hidden && t.size > 6.0) {
        let x = origin_x + track.offset;
        scene.cmds.push(Cmd::Text {
            rect: Rect::new(x, 2.0, track.size, origin_y - 4.0),
            text: column_name(track.index),
            size_px: HEADING_FONT_PX * scale,
            color: Rgba::HEADING_FG,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            halign: HAlign::Center,
            valign: VAlign::Center,
            wrap: false,
            clip: Rect::new(x, 0.0, track.size, origin_y),
        });
    }
    for track in rows.iter().filter(|t| !t.hidden && t.size > 6.0) {
        let y = origin_y + track.offset;
        scene.cmds.push(Cmd::Text {
            rect: Rect::new(2.0, y, origin_x - 6.0, track.size),
            text: track.index.to_string(),
            size_px: HEADING_FONT_PX * scale,
            color: Rgba::HEADING_FG,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            halign: HAlign::Right,
            valign: VAlign::Center,
            wrap: false,
            clip: Rect::new(0.0, y, origin_x, track.size),
        });
    }
    scene.cmds.push(Cmd::Line {
        x1: 0.0,
        y1: origin_y,
        x2: scene.width,
        y2: origin_y,
        width: 1.0,
        color: Rgba::HEADING_RULE,
        dash: Dash::Solid,
    });
    scene.cmds.push(Cmd::Line {
        x1: origin_x,
        y1: 0.0,
        x2: origin_x,
        y2: scene.height,
        width: 1.0,
        color: Rgba::HEADING_RULE,
        dash: Dash::Solid,
    });
}

/// 1-based column index to its Excel letters.
pub fn column_name(mut index: u32) -> String {
    let mut out = String::new();
    while index > 0 {
        let remainder = ((index - 1) % 26) as u8;
        out.insert(0, (b'A' + remainder) as char);
        index = (index - 1) / 26;
    }
    out
}

/// Line box height used by auto-fit, exposed for tests.
pub fn line_height_px(size_px: f32) -> f32 {
    size_px * LINE_HEIGHT_FACTOR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_names_roll_over() {
        assert_eq!(column_name(1), "A");
        assert_eq!(column_name(26), "Z");
        assert_eq!(column_name(27), "AA");
        assert_eq!(column_name(702), "ZZ");
    }

    #[test]
    fn autofit_matches_excel_for_the_two_pinned_sizes() {
        // Calibri 11 -> 15.00 pt (Excel's default row height).
        let eleven = (11.0 * PX_PER_PT * AUTOFIT_EM_FACTOR).ceil() + AUTOFIT_PAD_PX;
        assert_eq!(eleven, 20.0);
        assert_eq!(eleven / PX_PER_PT, 15.0);
        // Calibri 18 -> 23.25 pt, which is what Excel auto-fits an 18 pt
        // title to. The spike clipped it into a 15 pt row.
        let eighteen = (18.0 * PX_PER_PT * AUTOFIT_EM_FACTOR).ceil() + AUTOFIT_PAD_PX;
        assert_eq!(eighteen, 31.0);
        assert_eq!(eighteen / PX_PER_PT, 23.25);
    }
}
