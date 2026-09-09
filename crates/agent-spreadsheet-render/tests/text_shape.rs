//! Text-stack goldens.
//!
//! Two things the 5003 bake-off pinned as day-one tests:
//!
//! * a multi-contour glyph, because ab_glyph returns a flat segment list and an
//!   unstitched path collapses the winding fill to hairline fragments — the
//!   defect that invalidated the bake-off's first ab_glyph measurement run;
//! * the `.notdef` box, because the subset drops out-of-coverage codepoints
//!   silently and a blank cell is exactly the failure that must not ship.

use ab_glyph::{Font, FontRef};
use agent_spreadsheet_render::text::{REGULAR, contour_count, stitch_contours};
use agent_spreadsheet_render::{
    Fonts, RangeBounds, RasterOptions, RenderOptions, Rgba, Warning, extract_scene, rasterize,
};
use tiny_skia::{FillRule, Paint, Pixmap, Transform};

fn face() -> FontRef<'static> {
    FontRef::try_from_slice(REGULAR).unwrap()
}

/// Fill one glyph at 64 px and count how many pixels it inks.
fn ink_pixels(character: char) -> u32 {
    let font = face();
    let glyph = font.glyph_id(character);
    let outline = font.outline(glyph).expect("glyph has an outline");
    let path = stitch_contours(&outline.curves).expect("stitched path");
    let mut pixmap = Pixmap::new(96, 96).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, 255);
    paint.anti_alias = true;
    let scale = 64.0 / font.units_per_em().unwrap();
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::from_row(scale, 0.0, 0.0, -scale, 16.0, 80.0),
        None,
    );
    pixmap
        .pixels()
        .iter()
        .filter(|pixel| pixel.red() < 128)
        .count() as u32
}

#[test]
fn two_contour_glyphs_have_two_contours() {
    let font = face();
    for (character, expected) in [('o', 2usize), ('8', 3), ('l', 1)] {
        let outline = font.outline(font.glyph_id(character)).unwrap();
        assert_eq!(
            contour_count(&outline.curves),
            expected,
            "contour count for {character:?}"
        );
    }
}

#[test]
fn a_two_contour_glyph_fills_as_a_ring_not_a_disc() {
    // 'o' is a filled outer contour with a counter-wound inner one. If the
    // contours are not stitched, the winding fill produces a sliver instead of
    // a ring; if they are stitched but the inner contour is lost, it produces
    // a solid disc. The ring's ink area sits strictly between the two.
    let ring = ink_pixels('o');
    let disc = ink_pixels('0'); // also a ring, but a wider one
    assert!(ring > 400, "'o' inked only {ring} px — contours collapsed");
    assert!(
        ring < 1800,
        "'o' inked {ring} px — the counter is filled in"
    );
    assert!(
        disc > ring,
        "'0' should ink more than 'o' ({disc} vs {ring})"
    );
}

#[test]
fn glyph_ink_is_stable_across_repeated_fills() {
    // Same path, same transform, same result. This is the per-glyph half of
    // the determinism guarantee the PNG goldens depend on.
    let first = ink_pixels('g');
    for _ in 0..4 {
        assert_eq!(ink_pixels('g'), first);
    }
}

#[test]
fn notdef_is_drawn_and_declared_for_out_of_subset_codepoints() {
    let fonts = Fonts::new();
    assert!(fonts.has_glyph('A'));
    assert!(fonts.has_glyph('\u{20ac}'), "euro is inside the subset");
    assert!(!fonts.has_glyph('\u{03b1}'), "greek alpha is outside it");
    assert!(fonts.has_missing_glyphs("alpha \u{03b1}"));
    assert!(!fonts.has_missing_glyphs("plain ascii"));

    // A missing codepoint still advances, so the row does not silently
    // collapse to nothing.
    let advance = fonts.measure("\u{03b1}", 20.0, false);
    assert!(advance > 5.0, "notdef advance was {advance}");

    // And it inks: draw one and confirm the pixmap is not blank.
    let mut pixmap = Pixmap::new(40, 40).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);
    fonts.draw(
        &mut pixmap,
        "\u{03b1}",
        4.0,
        30.0,
        24.0,
        false,
        false,
        Rgba::BLACK,
        None,
    );
    let inked = pixmap
        .pixels()
        .iter()
        .filter(|pixel| pixel.red() < 128)
        .count();
    assert!(inked > 20, "notdef box inked only {inked} px");
}

#[test]
fn a_sheet_of_out_of_subset_text_raises_font_substituted() {
    let mut book = umya_spreadsheet::new_file();
    book.sheet_mut(0).ok()
        .unwrap()
        .cell_mut("A1")
        .set_value("\u{03b1} \u{03b2} \u{03b3}");
    let sheet = book.sheet(0).ok().unwrap();
    let scene = extract_scene(
        sheet,
        &book,
        &RangeBounds::new(1, 2, 1, 2),
        &RenderOptions::default(),
    )
    .unwrap();
    assert!(scene.warnings.contains(&Warning::FontSubstituted));
    let output = rasterize(&scene, &RasterOptions::default()).unwrap();
    assert!(output.report.warnings.contains(&Warning::FontSubstituted));
}
