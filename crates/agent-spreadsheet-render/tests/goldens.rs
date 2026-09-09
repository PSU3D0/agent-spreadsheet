//! Scene JSON goldens and PNG sha256 goldens, one of each per fixture.
//!
//! Regenerate with:
//!
//! ```sh
//! UPDATE_GOLDENS=1 cargo test -p agent-spreadsheet-render --test goldens \
//!     --config 'build.rustc-wrapper=""'
//! ```

mod common;

use std::collections::BTreeMap;

use common::{FIXTURES, GOLDEN_RANGE, golden_dir, pixel_signature, render_fixture, updating};

#[test]
fn scene_goldens_match() {
    let dir = golden_dir().join("scenes");
    if updating() {
        std::fs::create_dir_all(&dir).unwrap();
    }
    let mut mismatched = Vec::new();
    for (name, _) in FIXTURES {
        let (scene, _) = render_fixture(name);
        let json = serde_json::to_string(&scene).unwrap();
        let path = dir.join(format!("{name}.json"));
        if updating() {
            std::fs::write(&path, format!("{json}\n")).unwrap();
            continue;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("missing scene golden {}: {error}", path.display()));
        if expected.trim_end() != json {
            mismatched.push(name.to_string());
        }
        // The golden must round-trip through the public scene model, so the
        // serde representation stays a supported surface and not a debug dump.
        let parsed: agent_spreadsheet_render::Scene =
            serde_json::from_str(expected.trim_end()).unwrap();
        assert_eq!(parsed.cmds.len(), scene.cmds.len(), "{name} command count");
    }
    assert!(
        mismatched.is_empty(),
        "scene goldens differ for: {mismatched:?} (rerun with UPDATE_GOLDENS=1 after reviewing)"
    );
}

/// Pixel goldens: sha256 of the decoded RGBA buffer, plus the image size.
///
/// Deliberately not a hash of the PNG bytes. The compressed stream depends on
/// which deflate backend `flate2` resolves to, and Cargo decides that by
/// unifying features across the entire build — the same fixture encodes to
/// different PNG bytes under `cargo test -p agent-spreadsheet-render` and
/// under `cargo test --workspace`, with pixel-identical output both times.
/// What this crate guarantees, and what these goldens pin, is the pixels.
/// `determinism.rs` covers PNG-byte stability within one build.
#[test]
fn pixel_goldens_match() {
    let path = golden_dir().join("pixels-sha256.txt");
    let mut actual: BTreeMap<String, String> = BTreeMap::new();
    for (name, _) in FIXTURES {
        let (_, output) = render_fixture(name);
        actual.insert(name.to_string(), pixel_signature(&output));
    }
    let rendered: String = actual
        .iter()
        .map(|(name, line)| format!("{name} {line}\n"))
        .collect();
    if updating() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let header = format!(
            "# sha256 of the decoded RGBA buffer, and the pixel size, for each fixture\n\
             # rendered over {}..{} x {}..{}. No system fonts are used, so these are\n\
             # reproducible across machines and across native and wasm32.\n\
             # NOT a hash of the PNG bytes: the deflate stream depends on which backend\n\
             # Cargo's feature unification picks for flate2. See tests/goldens.rs.\n",
            GOLDEN_RANGE.first_col,
            GOLDEN_RANGE.last_col,
            GOLDEN_RANGE.first_row,
            GOLDEN_RANGE.last_row
        );
        std::fs::write(&path, format!("{header}{rendered}")).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("missing {}: {error}", path.display()));
    let expected: String = expected
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| format!("{line}\n"))
        .collect();
    assert_eq!(
        expected, rendered,
        "pixel goldens differ (rerun with UPDATE_GOLDENS=1 after reviewing)"
    );
}

#[test]
fn every_fixture_renders_a_nonempty_png() {
    // `DUMP_PNG_DIR=/some/dir` writes every fixture's PNG there, for eyeballing
    // a change before its golden is accepted.
    let dump = std::env::var("DUMP_PNG_DIR").ok();
    if let Some(dir) = &dump {
        std::fs::create_dir_all(dir).unwrap();
    }
    for (name, purpose) in FIXTURES {
        let (scene, output) = render_fixture(name);
        if let Some(dir) = &dump {
            std::fs::write(format!("{dir}/{name}.png"), &output.png).unwrap();
            std::fs::write(format!("{dir}/{name}.json"), serde_json::to_vec(&scene).unwrap()).unwrap();
        }
        assert!(output.width > 0 && output.height > 0, "{name} ({purpose})");
        assert!(
            output.png.starts_with(&[0x89, b'P', b'N', b'G']),
            "{name} is not a PNG"
        );
        assert!(!scene.cmds.is_empty(), "{name} produced an empty scene");
        assert_eq!(output.report.renderer, "native-raster/1");
    }
}
