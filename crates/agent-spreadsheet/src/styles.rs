use crate::model::{
    AlignmentDescriptor, AlignmentPatch, BorderSideDescriptor, BorderSidePatch, BordersDescriptor,
    BordersPatch, FillDescriptor, FillPatch, FontDescriptor, FontPatch, GradientFillDescriptor,
    GradientFillPatch, GradientStopDescriptor, PatternFillDescriptor, PatternFillPatch,
    StyleDescriptor, StylePatch,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::str::FromStr;
use umya_spreadsheet::structs::{EnumTrait, HorizontalAlignmentValues, VerticalAlignmentValues};
use umya_spreadsheet::{Border, Fill, Font, PatternValues, Style};

pub fn normalize_color_hex(input: &str) -> Option<(String, bool)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if hex.is_empty() {
        return None;
    }
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    match hex.len() {
        3 => {
            let mut expanded = String::with_capacity(6);
            for ch in hex.chars() {
                expanded.push(ch);
                expanded.push(ch);
            }
            Some((format!("FF{}", expanded.to_ascii_uppercase()), true))
        }
        6 => Some((format!("FF{}", hex.to_ascii_uppercase()), true)),
        8 => Some((hex.to_ascii_uppercase(), false)),
        _ => None,
    }
}

/// Set a validated RGB/ARGB colour without using Umya's panicking string setter.
/// Operation ingress validates colours; this low-level projection also tolerates
/// malformed caller-supplied descriptors without panicking.
pub(crate) fn set_color_hex(color: &mut umya_spreadsheet::Color, input: &str) -> bool {
    let Some(value) = normalize_color_hex(input)
        .and_then(|(hex, _)| umya_spreadsheet::Color::hex_to_argb8(&hex))
    else {
        return false;
    };
    color.set_argb(value);
    true
}

fn direct_argb(color: &umya_spreadsheet::Color) -> Option<String> {
    let value = color.argb();
    let mut probe = color.clone();
    probe.set_argb(value);
    (value != Default::default() || probe == *color)
        .then(|| umya_spreadsheet::Color::argb8_to_hex(value))
}

pub fn descriptor_from_style(style: &Style) -> StyleDescriptor {
    let font = style.font().and_then(descriptor_from_font);
    let fill = style.fill().and_then(descriptor_from_fill);
    let borders = style.borders().and_then(|borders| {
        let left = descriptor_from_border_side(borders.left_border());
        let right = descriptor_from_border_side(borders.right_border());
        let top = descriptor_from_border_side(borders.top_border());
        let bottom = descriptor_from_border_side(borders.bottom_border());
        let diagonal = descriptor_from_border_side(borders.diagonal_border());
        let vertical = descriptor_from_border_side(borders.vertical_border());
        let horizontal = descriptor_from_border_side(borders.horizontal_border());

        let diagonal_up = if borders.diagonal_up() {
            Some(true)
        } else {
            None
        };
        let diagonal_down = if borders.diagonal_down() {
            Some(true)
        } else {
            None
        };

        let descriptor = BordersDescriptor {
            left,
            right,
            top,
            bottom,
            diagonal,
            vertical,
            horizontal,
            diagonal_up,
            diagonal_down,
        };

        if descriptor.is_empty() {
            None
        } else {
            Some(descriptor)
        }
    });
    let alignment = style.alignment().and_then(descriptor_from_alignment);
    let number_format = style.number_format().and_then(|fmt| {
        let code = fmt.format_code();
        if code.eq_ignore_ascii_case("general") {
            None
        } else {
            Some(code.to_string())
        }
    });

    StyleDescriptor {
        font,
        fill,
        borders,
        alignment,
        number_format,
    }
}

pub fn stable_style_id(descriptor: &StyleDescriptor) -> String {
    let bytes = serde_json::to_vec(descriptor).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    hex.chars().take(12).collect()
}

pub fn compress_positions_to_ranges(positions: &[(u32, u32)], limit: usize) -> (Vec<String>, bool) {
    if positions.is_empty() {
        return (Vec::new(), false);
    }

    let mut rows: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &(row, col) in positions {
        rows.entry(row).or_default().push(col);
    }
    for cols in rows.values_mut() {
        cols.sort_unstable();
        cols.dedup();
    }

    let mut spans_by_cols: BTreeMap<(u32, u32), Vec<u32>> = BTreeMap::new();
    for (row, cols) in rows {
        if cols.is_empty() {
            continue;
        }
        let mut start = cols[0];
        let mut prev = cols[0];
        for col in cols.into_iter().skip(1) {
            if col == prev + 1 {
                prev = col;
            } else {
                spans_by_cols.entry((start, prev)).or_default().push(row);
                start = col;
                prev = col;
            }
        }
        spans_by_cols.entry((start, prev)).or_default().push(row);
    }

    let mut ranges = Vec::new();
    let mut truncated = false;

    'outer: for ((start_col, end_col), mut span_rows) in spans_by_cols {
        span_rows.sort_unstable();
        span_rows.dedup();
        if span_rows.is_empty() {
            continue;
        }
        let mut run_start = span_rows[0];
        let mut prev_row = span_rows[0];
        for row in span_rows.into_iter().skip(1) {
            if row == prev_row + 1 {
                prev_row = row;
                continue;
            }
            ranges.push(format_range(start_col, end_col, run_start, prev_row));
            if ranges.len() >= limit {
                truncated = true;
                break 'outer;
            }
            run_start = row;
            prev_row = row;
        }
        ranges.push(format_range(start_col, end_col, run_start, prev_row));
        if ranges.len() >= limit {
            truncated = true;
            break;
        }
    }

    if truncated {
        ranges.truncate(limit);
    }
    (ranges, truncated)
}

fn format_range(start_col: u32, end_col: u32, start_row: u32, end_row: u32) -> String {
    let start_addr = crate::utils::cell_address(start_col, start_row);
    let end_addr = crate::utils::cell_address(end_col, end_row);
    if start_addr == end_addr {
        start_addr
    } else {
        format!("{start_addr}:{end_addr}")
    }
}

fn descriptor_from_font(font: &Font) -> Option<FontDescriptor> {
    let bold = font.bold();
    let italic = font.italic();
    let underline = font.underline();
    let strikethrough = font.strikethrough();
    let color = direct_argb(font.color());

    let descriptor = FontDescriptor {
        name: Some(font.name().to_string()).filter(|s| !s.is_empty()),
        size: Some(font.size()).filter(|s| *s > 0.0),
        bold: if bold { Some(true) } else { None },
        italic: if italic { Some(true) } else { None },
        underline: if underline.eq_ignore_ascii_case("none") {
            None
        } else {
            Some(underline.to_string())
        },
        strikethrough: if strikethrough { Some(true) } else { None },
        color,
    };

    if descriptor.is_empty() {
        None
    } else {
        Some(descriptor)
    }
}

fn descriptor_from_fill(fill: &Fill) -> Option<FillDescriptor> {
    if let Some(pattern) = fill.pattern_fill() {
        let pattern_type = pattern.pattern_type();
        let kind = pattern_type.value_string();
        let fg = pattern
            .foreground_color()
            .and_then(direct_argb)
            .filter(|s| !s.is_empty());
        let bg = pattern
            .background_color()
            .and_then(direct_argb)
            .filter(|s| !s.is_empty());

        if kind.eq_ignore_ascii_case("none") && fg.is_none() && bg.is_none() {
            return None;
        }

        return Some(FillDescriptor::Pattern(PatternFillDescriptor {
            pattern_type: if kind.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(kind.to_string())
            },
            foreground_color: fg,
            background_color: bg,
        }));
    }

    if let Some(gradient) = fill.gradient_fill() {
        let stops: Vec<GradientStopDescriptor> = gradient
            .gradient_stop()
            .iter()
            .map(|stop| GradientStopDescriptor {
                position: stop.position(),
                color: direct_argb(stop.color()).unwrap_or_default(),
            })
            .collect();

        let degree = gradient.degree();
        if stops.is_empty() && degree == 0.0 {
            return None;
        }

        return Some(FillDescriptor::Gradient(GradientFillDescriptor {
            degree: Some(degree).filter(|d| *d != 0.0),
            stops,
        }));
    }

    None
}

fn descriptor_from_border_side(border: &Border) -> Option<BorderSideDescriptor> {
    let style = border.border_style();
    let style = if style.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(style.to_string())
    };
    let color = border.color().as_ref().and_then(direct_argb);

    let descriptor = BorderSideDescriptor { style, color };
    if descriptor.is_empty() {
        None
    } else {
        Some(descriptor)
    }
}

fn descriptor_from_alignment(
    alignment: &umya_spreadsheet::Alignment,
) -> Option<AlignmentDescriptor> {
    let horizontal = if alignment.horizontal() != &HorizontalAlignmentValues::General {
        Some(alignment.horizontal().value_string().to_string())
    } else {
        None
    };
    let vertical = if alignment.vertical() != &VerticalAlignmentValues::Bottom {
        Some(alignment.vertical().value_string().to_string())
    } else {
        None
    };
    let wrap_text = if alignment.wrap_text() {
        Some(true)
    } else {
        None
    };
    let text_rotation = if alignment.text_rotation() != 0 {
        Some(alignment.text_rotation())
    } else {
        None
    };

    let descriptor = AlignmentDescriptor {
        horizontal,
        vertical,
        wrap_text,
        text_rotation,
    };
    if descriptor.is_empty() {
        None
    } else {
        Some(descriptor)
    }
}

trait IsEmpty {
    fn is_empty(&self) -> bool;
}

impl IsEmpty for FontDescriptor {
    fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.size.is_none()
            && self.bold.is_none()
            && self.italic.is_none()
            && self.underline.is_none()
            && self.strikethrough.is_none()
            && self.color.is_none()
    }
}

impl IsEmpty for BorderSideDescriptor {
    fn is_empty(&self) -> bool {
        self.style.is_none() && self.color.is_none()
    }
}

impl IsEmpty for BordersDescriptor {
    fn is_empty(&self) -> bool {
        self.left.is_none()
            && self.right.is_none()
            && self.top.is_none()
            && self.bottom.is_none()
            && self.diagonal.is_none()
            && self.vertical.is_none()
            && self.horizontal.is_none()
            && self.diagonal_up.is_none()
            && self.diagonal_down.is_none()
    }
}

impl IsEmpty for AlignmentDescriptor {
    fn is_empty(&self) -> bool {
        self.horizontal.is_none()
            && self.vertical.is_none()
            && self.wrap_text.is_none()
            && self.text_rotation.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StylePatchMode {
    Merge,
    Set,
    Clear,
}

pub fn apply_style_patch(current: &Style, patch: &StylePatch, mode: StylePatchMode) -> Style {
    match mode {
        StylePatchMode::Clear => Style::default(),
        StylePatchMode::Set | StylePatchMode::Merge => {
            let mut desc = match mode {
                StylePatchMode::Merge => descriptor_from_style(current),
                StylePatchMode::Set => StyleDescriptor::default(),
                StylePatchMode::Clear => StyleDescriptor::default(),
            };
            merge_style_patch(&mut desc, patch);
            let mut style = Style::default();
            apply_descriptor_to_style(&mut style, &desc);
            style
        }
    }
}

fn merge_style_patch(desc: &mut StyleDescriptor, patch: &StylePatch) {
    if let Some(font_patch) = &patch.font {
        match font_patch {
            None => desc.font = None,
            Some(p) => {
                let mut font_desc = desc.font.take().unwrap_or_default();
                apply_font_patch(&mut font_desc, p);
                if font_desc.is_empty() {
                    desc.font = None;
                } else {
                    desc.font = Some(font_desc);
                }
            }
        }
    }

    if let Some(fill_patch) = &patch.fill {
        match fill_patch {
            None => desc.fill = None,
            Some(p) => {
                let fill_desc = apply_fill_patch(desc.fill.take(), p);
                desc.fill = fill_desc;
            }
        }
    }

    if let Some(borders_patch) = &patch.borders {
        match borders_patch {
            None => desc.borders = None,
            Some(p) => {
                let mut borders_desc = desc.borders.take().unwrap_or_default();
                apply_borders_patch(&mut borders_desc, p);
                if borders_desc.is_empty() {
                    desc.borders = None;
                } else {
                    desc.borders = Some(borders_desc);
                }
            }
        }
    }

    if let Some(alignment_patch) = &patch.alignment {
        match alignment_patch {
            None => desc.alignment = None,
            Some(p) => {
                let mut align_desc = desc.alignment.take().unwrap_or_default();
                apply_alignment_patch(&mut align_desc, p);
                if align_desc.is_empty() {
                    desc.alignment = None;
                } else {
                    desc.alignment = Some(align_desc);
                }
            }
        }
    }

    if let Some(nf_patch) = &patch.number_format {
        match nf_patch {
            None => desc.number_format = None,
            Some(fmt) => {
                let fmt = fmt.trim();
                if fmt.is_empty() || fmt.eq_ignore_ascii_case("general") {
                    desc.number_format = None;
                } else {
                    desc.number_format = Some(fmt.to_string());
                }
            }
        }
    }
}

fn apply_font_patch(desc: &mut FontDescriptor, patch: &FontPatch) {
    apply_double(&mut desc.name, &patch.name);
    if desc.name.as_deref().is_some_and(|s| s.trim().is_empty()) {
        desc.name = None;
    }

    apply_double(&mut desc.size, &patch.size);
    if desc.size.is_some_and(|s| s <= 0.0) {
        desc.size = None;
    }

    apply_double(&mut desc.bold, &patch.bold);
    if desc.bold == Some(false) {
        desc.bold = None;
    }

    apply_double(&mut desc.italic, &patch.italic);
    if desc.italic == Some(false) {
        desc.italic = None;
    }

    apply_double(&mut desc.underline, &patch.underline);
    if desc
        .underline
        .as_deref()
        .is_some_and(|u| u.trim().is_empty() || u.eq_ignore_ascii_case("none"))
    {
        desc.underline = None;
    }

    apply_double(&mut desc.strikethrough, &patch.strikethrough);
    if desc.strikethrough == Some(false) {
        desc.strikethrough = None;
    }

    apply_double(&mut desc.color, &patch.color);
    if desc.color.as_deref().is_some_and(|c| c.trim().is_empty()) {
        desc.color = None;
    }
}

fn apply_fill_patch(existing: Option<FillDescriptor>, patch: &FillPatch) -> Option<FillDescriptor> {
    match patch {
        FillPatch::Pattern(patch_pattern) => {
            let mut desc = match existing {
                Some(FillDescriptor::Pattern(p)) => p,
                _ => PatternFillDescriptor::default(),
            };
            apply_pattern_fill_patch(&mut desc, patch_pattern);
            if desc.pattern_type.is_none()
                && desc.foreground_color.is_none()
                && desc.background_color.is_none()
            {
                None
            } else {
                Some(FillDescriptor::Pattern(desc))
            }
        }
        FillPatch::Gradient(patch_gradient) => {
            let mut desc = match existing {
                Some(FillDescriptor::Gradient(g)) => g,
                _ => GradientFillDescriptor::default(),
            };
            apply_gradient_fill_patch(&mut desc, patch_gradient);
            if desc.degree.is_none() && desc.stops.is_empty() {
                None
            } else {
                Some(FillDescriptor::Gradient(desc))
            }
        }
    }
}

fn apply_pattern_fill_patch(desc: &mut PatternFillDescriptor, patch: &PatternFillPatch) {
    apply_double(&mut desc.pattern_type, &patch.pattern_type);
    if desc
        .pattern_type
        .as_deref()
        .is_some_and(|p| p.trim().is_empty() || p.eq_ignore_ascii_case("none"))
    {
        desc.pattern_type = None;
    }

    apply_double(&mut desc.foreground_color, &patch.foreground_color);
    if desc
        .foreground_color
        .as_deref()
        .is_some_and(|c| c.trim().is_empty())
    {
        desc.foreground_color = None;
    }

    apply_double(&mut desc.background_color, &patch.background_color);
    if desc
        .background_color
        .as_deref()
        .is_some_and(|c| c.trim().is_empty())
    {
        desc.background_color = None;
    }
}

fn apply_gradient_fill_patch(desc: &mut GradientFillDescriptor, patch: &GradientFillPatch) {
    apply_double(&mut desc.degree, &patch.degree);
    if desc.degree == Some(0.0) {
        desc.degree = None;
    }

    if let Some(stops) = &patch.stops {
        desc.stops = stops
            .iter()
            .map(|s| GradientStopDescriptor {
                position: s.position,
                color: s.color.clone(),
            })
            .collect();
    }
}

fn apply_borders_patch(desc: &mut BordersDescriptor, patch: &BordersPatch) {
    apply_border_side_patch(&mut desc.left, &patch.left);
    apply_border_side_patch(&mut desc.right, &patch.right);
    apply_border_side_patch(&mut desc.top, &patch.top);
    apply_border_side_patch(&mut desc.bottom, &patch.bottom);
    apply_border_side_patch(&mut desc.diagonal, &patch.diagonal);
    apply_border_side_patch(&mut desc.vertical, &patch.vertical);
    apply_border_side_patch(&mut desc.horizontal, &patch.horizontal);

    apply_double(&mut desc.diagonal_up, &patch.diagonal_up);
    if desc.diagonal_up == Some(false) {
        desc.diagonal_up = None;
    }
    apply_double(&mut desc.diagonal_down, &patch.diagonal_down);
    if desc.diagonal_down == Some(false) {
        desc.diagonal_down = None;
    }
}

fn apply_border_side_patch(
    target: &mut Option<BorderSideDescriptor>,
    patch: &Option<Option<BorderSidePatch>>,
) {
    match patch {
        None => {}
        Some(None) => *target = None,
        Some(Some(p)) => {
            let mut side = target.take().unwrap_or_default();
            apply_double(&mut side.style, &p.style);
            if side
                .style
                .as_deref()
                .is_some_and(|s| s.trim().is_empty() || s.eq_ignore_ascii_case("none"))
            {
                side.style = None;
            }
            apply_double(&mut side.color, &p.color);
            if side.color.as_deref().is_some_and(|c| c.trim().is_empty()) {
                side.color = None;
            }
            if side.is_empty() {
                *target = None;
            } else {
                *target = Some(side);
            }
        }
    }
}

fn apply_alignment_patch(desc: &mut AlignmentDescriptor, patch: &AlignmentPatch) {
    apply_double(&mut desc.horizontal, &patch.horizontal);
    if desc
        .horizontal
        .as_deref()
        .is_some_and(|h| h.trim().is_empty() || h.eq_ignore_ascii_case("general"))
    {
        desc.horizontal = None;
    }

    apply_double(&mut desc.vertical, &patch.vertical);
    if desc
        .vertical
        .as_deref()
        .is_some_and(|v| v.trim().is_empty() || v.eq_ignore_ascii_case("bottom"))
    {
        desc.vertical = None;
    }

    apply_double(&mut desc.wrap_text, &patch.wrap_text);
    if desc.wrap_text == Some(false) {
        desc.wrap_text = None;
    }

    apply_double(&mut desc.text_rotation, &patch.text_rotation);
    if desc.text_rotation == Some(0) {
        desc.text_rotation = None;
    }
}

fn apply_descriptor_to_style(style: &mut Style, desc: &StyleDescriptor) {
    if let Some(font_desc) = &desc.font {
        let font = style.font_mut();
        if let Some(name) = &font_desc.name {
            font.set_name(name.clone());
        }
        if let Some(size) = font_desc.size {
            font.set_size(size);
        }
        if let Some(bold) = font_desc.bold {
            font.set_bold(bold);
        }
        if let Some(italic) = font_desc.italic {
            font.set_italic(italic);
        }
        if let Some(underline) = &font_desc.underline {
            font.set_underline(underline.clone());
        }
        if let Some(strike) = font_desc.strikethrough {
            font.set_strikethrough(strike);
        }
        if let Some(color) = &font_desc.color {
            set_color_hex(font.color_mut(), color);
        }
    }

    if let Some(fill_desc) = &desc.fill {
        match fill_desc {
            FillDescriptor::Pattern(p) => {
                let pat = style.fill_mut().pattern_fill_mut();
                if let Some(kind) = &p.pattern_type
                    && let Ok(pv) = PatternValues::from_str(kind)
                {
                    pat.set_pattern_type(pv);
                }
                if let Some(fg) = &p.foreground_color {
                    set_color_hex(pat.foreground_color_mut(), fg);
                }
                if let Some(bg) = &p.background_color {
                    set_color_hex(pat.background_color_mut(), bg);
                }
            }
            FillDescriptor::Gradient(g) => {
                let grad = style.fill_mut().gradient_fill_mut();
                if let Some(deg) = g.degree {
                    grad.set_degree(deg);
                }
                grad.gradient_stop_mut().clear();
                for stop in &g.stops {
                    let mut st = umya_spreadsheet::GradientStop::default();
                    st.set_position(stop.position);
                    set_color_hex(st.color_mut(), &stop.color);
                    grad.set_gradient_stop(st);
                }
            }
        }
    }

    if let Some(border_desc) = &desc.borders {
        let borders = style.borders_mut();
        apply_border_side_descriptor(borders.left_border_mut(), &border_desc.left);
        apply_border_side_descriptor(borders.right_border_mut(), &border_desc.right);
        apply_border_side_descriptor(borders.top_border_mut(), &border_desc.top);
        apply_border_side_descriptor(borders.bottom_border_mut(), &border_desc.bottom);
        apply_border_side_descriptor(borders.diagonal_border_mut(), &border_desc.diagonal);
        apply_border_side_descriptor(borders.vertical_border_mut(), &border_desc.vertical);
        apply_border_side_descriptor(borders.horizontal_border_mut(), &border_desc.horizontal);
        if let Some(up) = border_desc.diagonal_up {
            borders.set_diagonal_up(up);
        }
        if let Some(down) = border_desc.diagonal_down {
            borders.set_diagonal_down(down);
        }
    }

    if let Some(align_desc) = &desc.alignment {
        let align = style.alignment_mut();
        if let Some(h) = &align_desc.horizontal
            && let Ok(val) = HorizontalAlignmentValues::from_str(h)
        {
            align.set_horizontal(val);
        }
        if let Some(v) = &align_desc.vertical
            && let Ok(val) = VerticalAlignmentValues::from_str(v)
        {
            align.set_vertical(val);
        }
        if let Some(wrap) = align_desc.wrap_text {
            align.set_wrap_text(wrap);
        }
        if let Some(rot) = align_desc.text_rotation {
            align.set_text_rotation(rot);
        }
    }

    if let Some(fmt) = &desc.number_format {
        style.number_format_mut().set_format_code(fmt.clone());
    }
}

fn apply_border_side_descriptor(border: &mut Border, desc: &Option<BorderSideDescriptor>) {
    if let Some(side) = desc {
        if let Some(style_name) = &side.style {
            border.set_border_style(style_name.clone());
        }
        if let Some(color) = &side.color {
            let mut value = border.color().unwrap_or_default();
            if set_color_hex(&mut value, color) {
                border.set_color(value);
            }
        }
    }
}

fn apply_double<T: Clone>(target: &mut Option<T>, patch: &Option<Option<T>>) {
    match patch {
        None => {}
        Some(None) => *target = None,
        Some(Some(v)) => *target = Some(v.clone()),
    }
}
