//! Colour resolution.
//!
//! Deliberately NOT office2pdf's `resolve_style_color`, which is a one-line
//! delegation to umya's `get_argb_with_theme`. That method short-circuits on
//! indexed colours before it ever looks at `tint`, so an indexed colour with a
//! tint silently loses the tint. The 5003 bake-off said 5004 should fix that
//! case rather than inherit it, so this module resolves the three colour
//! sources itself and applies `tint` uniformly.
//!
//! umya's `Color` exposes `indexed`, `theme_index` and `argb` through getters
//! that cannot distinguish "absent" from "zero" — `get_theme_index()` returns 0
//! for a colour that names no theme slot at all, and slot 0 is `lt1`, i.e.
//! white. Indexing the colour map off that getter paints every unstyled font
//! white on white, which is what an openpyxl `<font><b val="1"/></font>` (no
//! `<color>` at all) produced. Presence therefore has to be probed through
//! public source-preserving setters and equality: reselecting the current
//! source leaves the value unchanged only when that source was present.
//! This also distinguishes explicitly transparent RGB from an absent colour.
//!
//! What this module keeps from doing its own resolution is the fix the bake-off
//! asked for: an *indexed* colour carrying a `tint` gets the tint applied,
//! where umya short-circuits and drops it.

use umya_spreadsheet::structs::drawing::Theme;

use crate::scene::Rgba;

const HLS_MAX: f64 = 255.0;

/// Resolve a style colour to RGBA, or `None` when the colour is unset.
pub fn resolve(color: &umya_spreadsheet::Color, theme: Option<&Theme>) -> Option<Rgba> {
    // Umya 3 returns transparent black for an absent RGB value. Preserve the
    // distinction from explicitly transparent RGB without inspecting private
    // fields: selecting the same source leaves the public value unchanged.
    let direct = color.argb();
    let mut rgb_probe = color.clone();
    rgb_probe.set_argb(direct);
    if direct != Default::default() || rgb_probe == *color {
        // Spreadsheet style colours are opaque, matching from_argb_hex and
        // the pre-3.x renderer; OOXML producers commonly write an unused 00 alpha.
        let base = Rgba(direct.r, direct.g, direct.b, 255);
        let tint = color.tint();
        // umya returns here without ever looking at `tint`. We do not.
        return Some(if tint == 0.0 {
            base
        } else {
            apply_tint(base, tint)
        });
    }
    // No indexed and no literal rgb: either a theme slot, or nothing at all.
    // `get_argb_with_theme` distinguishes the two and applies the tint itself
    // on the theme path, so it is not reapplied here.
    let mut theme_probe = color.clone();
    theme_probe.set_theme_index(color.theme_index());
    if theme_probe != *color {
        return None;
    }
    let theme = theme?;
    theme.theme_elements().color_scheme().color_map().get(color.theme_index() as usize)?;
    let resolved = color.argb_with_theme(theme);
    Rgba::from_argb_hex(&resolved)
}

/// ECMA-376 `tint`, applied to the HLS luminance the way Excel does.
///
/// A negative tint darkens (`lum * (1 + tint)`), a positive one lightens
/// toward the maximum (`lum * (1 - tint) + HLSMAX * tint`). The round trip is
/// through Excel's 0..255 integer HLS space, not through floating-point HSL,
/// so the result matches what Excel and LibreOffice paint.
pub fn apply_tint(color: Rgba, tint: f64) -> Rgba {
    let (hue, lum, sat) = rgb_to_ms_hls(color);
    let tinted = if tint < 0.0 {
        lum * (1.0 + tint)
    } else {
        lum * (1.0 - tint) + (HLS_MAX - HLS_MAX * (1.0 - tint))
    };
    ms_hls_to_rgb(hue, tinted.round().clamp(0.0, HLS_MAX), sat, color.3)
}

fn rgb_to_ms_hls(color: Rgba) -> (f64, f64, f64) {
    let r = f64::from(color.0) / 255.0;
    let g = f64::from(color.1) / 255.0;
    let b = f64::from(color.2) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lum = (max + min) / 2.0;
    if (max - min).abs() < f64::EPSILON {
        return (0.0, lum * HLS_MAX, 0.0);
    }
    let delta = max - min;
    let sat = if lum <= 0.5 {
        delta / (max + min)
    } else {
        delta / (2.0 - max - min)
    };
    let hue = if (max - r).abs() < f64::EPSILON {
        (g - b) / delta
    } else if (max - g).abs() < f64::EPSILON {
        2.0 + (b - r) / delta
    } else {
        4.0 + (r - g) / delta
    };
    let hue = (hue / 6.0).rem_euclid(1.0);
    (hue * HLS_MAX, lum * HLS_MAX, sat * HLS_MAX)
}

fn ms_hls_to_rgb(hue: f64, lum: f64, sat: f64, alpha: u8) -> Rgba {
    let h = hue / HLS_MAX;
    let l = lum / HLS_MAX;
    let s = sat / HLS_MAX;
    if s <= 0.0 {
        let v = (l * 255.0).round().clamp(0.0, 255.0) as u8;
        return Rgba(v, v, v, alpha);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let channel = |mut t: f64| -> u8 {
        t = t.rem_euclid(1.0);
        let value = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (value * 255.0).round().clamp(0.0, 255.0) as u8
    };
    Rgba(
        channel(h + 1.0 / 3.0),
        channel(h),
        channel(h - 1.0 / 3.0),
        alpha,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_tint_is_identity() {
        assert_eq!(apply_tint(Rgba(0x44, 0x72, 0xc4, 255), 0.0).0, 0x44);
    }

    #[test]
    fn positive_tint_lightens_and_negative_darkens() {
        let base = Rgba(0x44, 0x72, 0xc4, 255);
        let lighter = apply_tint(base, 0.4);
        let darker = apply_tint(base, -0.5);
        let luminance = |c: Rgba| u32::from(c.0) + u32::from(c.1) + u32::from(c.2);
        assert!(luminance(lighter) > luminance(base));
        assert!(luminance(darker) < luminance(base));
    }

    #[test]
    fn grey_stays_grey_under_tint() {
        // Excel's 15% lighter black, the most common themed shade.
        let tinted = apply_tint(Rgba::BLACK, 0.15);
        assert_eq!(tinted.0, tinted.1);
        assert_eq!(tinted.1, tinted.2);
        assert_eq!(tinted.0, 38);
    }

    #[test]
    fn full_positive_tint_reaches_white() {
        assert_eq!(apply_tint(Rgba::BLACK, 1.0), Rgba(255, 255, 255, 255));
    }
}
