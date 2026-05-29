//! Round 187 — SVG 2 §11.2.1 `textLength` + `lengthAdjust` on
//! `<text>` and `<tspan>` elements.
//!
//! The §11.2.1 attribute pair declares the author's intended sum of
//! glyph advance values for a chunk and selects between two adjustment
//! modes:
//!
//! * `lengthAdjust="spacing"` (initial) — rescale only inter-glyph
//!   advances; glyph outlines are not stretched.
//! * `lengthAdjust="spacingAndGlyphs"` — rescale advances and also
//!   stretch / compress glyph outlines along the inline-base
//!   direction.
//!
//! These integration tests verify the per-chunk extent matches the
//! requested target and the §11.10.1.1 `text-anchor` shift composes
//! correctly with the rescale. Runs in its own integration-test binary
//! because the global font-resolver hook is one-shot.
//!
//! Font: bundled DejaVuSansMono (monospaced), so the unadjusted run
//! width `W` for `"ABCDE"` at `font-size="16"` is `5 × glyph_advance`,
//! independent of letter identity.

#![cfg(feature = "text")]

use oxideav_core::Node;
use oxideav_scribe::{Face, FaceChain};
use oxideav_svg::{parse_svg, text::set_font_resolver};

const FONT: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");

fn install_resolver() {
    let _ = set_font_resolver(move |_family, _size_px| {
        Face::from_ttf_bytes(FONT.to_vec()).ok().map(FaceChain::new)
    });
}

/// Recursively collect every non-identity-translate placement group's
/// transform components `(a, e, f)` — `a` for the post-compose scale
/// (so `spacingAndGlyphs` is visible), `e/f` for the translate.
fn collect_placements(node: &Node, out: &mut Vec<(f32, f32, f32)>) {
    if let Node::Group(g) = node {
        let tx = g.transform;
        let is_identity = (tx.a - 1.0).abs() < 1e-6
            && tx.b.abs() < 1e-6
            && tx.c.abs() < 1e-6
            && (tx.d - 1.0).abs() < 1e-6
            && tx.e.abs() < 1e-6
            && tx.f.abs() < 1e-6;
        if !is_identity {
            out.push((tx.a, tx.e, tx.f));
        }
        for c in &g.children {
            collect_placements(c, out);
        }
    }
}

fn text_groups(frame: &oxideav_core::VectorFrame) -> Vec<&oxideav_core::Group> {
    frame
        .root
        .children
        .iter()
        .filter_map(|c| match c {
            Node::Group(g) => Some(g),
            _ => None,
        })
        .collect()
}

fn placements_of(g: &oxideav_core::Group) -> Vec<(f32, f32, f32)> {
    let mut out = Vec::new();
    for c in &g.children {
        collect_placements(c, &mut out);
    }
    out
}

fn xs_of(g: &oxideav_core::Group) -> Vec<f32> {
    placements_of(g).into_iter().map(|(_, x, _)| x).collect()
}

fn leftmost(xs: &[f32]) -> f32 {
    xs.iter().copied().fold(f32::INFINITY, f32::min)
}

fn rightmost(xs: &[f32]) -> f32 {
    xs.iter().copied().fold(f32::NEG_INFINITY, f32::max)
}

/// Baseline `<text>` width measurement: the natural advance of
/// `"ABCDE"` at `font-size="16"` in DejaVuSansMono. Returned as
/// `(leftmost, rightmost)` x positions so a follow-up test can derive
/// the run width `W = rightmost − leftmost` without re-shaping.
fn baseline_extent() -> (f32, f32) {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="800" height="100">
  <text x="100" y="50" font-size="16">ABCDE</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let groups = text_groups(&frame);
    let xs = xs_of(groups[0]);
    (leftmost(&xs), rightmost(&xs))
}

/// `<text textLength="300">` (no `lengthAdjust`) — the chunk extent
/// (rightmost glyph origin minus leftmost) must match the requested
/// 300 user units regardless of the natural glyph advance. The
/// leftmost glyph stays at the `<text>` x origin (text-anchor defaults
/// to `start`).
///
/// Note: `emit_run` advances the pen to the LAST glyph's origin (not
/// past the last glyph's own advance) per the round-2 implementation
/// note in `text.rs`, so the chunk's natural extent is exactly
/// `rightmost_glyph_x − leftmost_glyph_x` from the baseline run.
#[test]
fn text_length_rescales_chunk_width_spacing_default() {
    install_resolver();

    let (b_left, b_right) = baseline_extent();
    let baseline_w = b_right - b_left;
    assert!(baseline_w > 0.0, "baseline run must be non-empty");

    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="800" height="100">
  <text x="100" y="50" font-size="16" textLength="300">ABCDE</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let groups = text_groups(&frame);
    assert_eq!(groups.len(), 1);
    let xs = xs_of(groups[0]);
    assert!(
        xs.len() >= 2,
        "expected at least two glyph placements; got {}",
        xs.len()
    );

    let left = leftmost(&xs);
    let right = rightmost(&xs);
    let adjusted_w = right - left;
    assert!(
        (left - 100.0).abs() < 1e-3,
        "leftmost glyph should sit at x=100 for start anchor; got {left}"
    );
    assert!(
        (adjusted_w - 300.0).abs() < 1.0,
        "expected adjusted run width ≈ 300; got {adjusted_w} (baseline_w={baseline_w})"
    );

    // `lengthAdjust` defaults to `spacing`, so each placement must
    // carry an identity x-scale (the `a` component is 1).
    for (a, _, _) in placements_of(groups[0]) {
        assert!(
            (a - 1.0).abs() < 1e-6,
            "spacing mode should not scale glyphs (a={a})"
        );
    }
}

/// `lengthAdjust="spacingAndGlyphs"` — same chunk width as the spacing
/// case, but every glyph placement also carries an `a` (x-scale)
/// component equal to the rescale factor `s = target / baseline_W`.
#[test]
fn length_adjust_spacing_and_glyphs_stretches_outlines() {
    install_resolver();

    let (b_left, b_right) = baseline_extent();
    let baseline_w = b_right - b_left;

    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="800" height="100">
  <text x="100" y="50" font-size="16" textLength="300" lengthAdjust="spacingAndGlyphs">ABCDE</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let groups = text_groups(&frame);
    let placements = placements_of(groups[0]);
    assert!(
        placements.len() >= 2,
        "expected glyph placements for spacingAndGlyphs run"
    );

    // The scale factor `s` is the ratio of the target width (after
    // accounting for the per-glyph advance subtraction is moot here
    // because the scale is computed on the chunk extent only). For
    // monospaced "ABCDE" at the same font-size, the chunk extent is
    // `4 * per_glyph_advance = baseline_w` so `s = 300 / baseline_w`.
    let expected_s = 300.0 / baseline_w;
    for (a, _, _) in &placements {
        assert!(
            (a - expected_s).abs() < 1e-3,
            "spacingAndGlyphs should set a≈{expected_s} on every placement; got {a}"
        );
    }
}

/// `text-anchor="middle"` + `textLength="300"` — the rescaling pass
/// runs first, so the §11.10.1.1 shift sees the adjusted extent. The
/// leftmost glyph must end up at `x − 300/2 = x − 150`.
#[test]
fn text_length_composes_with_middle_anchor() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="800" height="100">
  <text x="400" y="50" font-size="16" textLength="300" text-anchor="middle">ABCDE</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let groups = text_groups(&frame);
    let xs = xs_of(groups[0]);
    let left = leftmost(&xs);
    // The §11.10.1.1 shift moves the chunk's leftmost glyph by
    // `−adjusted_W / 2`. The adjusted chunk extent is the spec's
    // target (the actual right-minus-left we computed above), so the
    // leftmost x = origin − target/2 = 400 − 150 = 250 (within the
    // sub-pixel tolerance the existing round-172 tests use).
    let expected_left = 400.0 - 300.0 / 2.0;
    assert!(
        (left - expected_left).abs() < 1.0,
        "middle-anchor + textLength=300 should place leftmost at {expected_left}; got {left}"
    );
}

/// A `<tspan textLength=…>` on an absolute-positioning chunk-opener
/// (one with `x=`) only rescales its own chunk; the sibling chunk
/// before it stays at the natural width.
#[test]
fn per_tspan_text_length_isolates_to_its_chunk() {
    install_resolver();
    let (b_left, b_right) = baseline_extent();
    let baseline_w = b_right - b_left;

    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="800" height="100">
  <text x="50" y="50" font-size="16">ABCDE<tspan x="500" textLength="200">FGHIJ</tspan></text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let groups = text_groups(&frame);
    let xs = xs_of(groups[0]);
    assert!(
        xs.len() >= 10,
        "expected two five-glyph chunks; got {} placements",
        xs.len()
    );

    // Split by chunk: the first 5 placements sit ≈ at [50, 50+W],
    // the next 5 at [500, 500 + adjusted-extent]. We sort and split.
    let mut sorted = xs.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let first = &sorted[..5];
    let second = &sorted[5..];

    let first_w = rightmost(first) - leftmost(first);
    let second_w = rightmost(second) - leftmost(second);

    // First chunk: natural width, no rescaling.
    assert!(
        (first_w - baseline_w).abs() < 1.0,
        "first chunk should keep natural width {baseline_w}; got {first_w}"
    );
    // Second chunk: rescaled to ≈ 200 (the chunk extent is
    // `rightmost − leftmost` glyph-origin, matching how the rescale
    // pass treats `chunk.x_end − chunk.x_origin`).
    assert!(
        (second_w - 200.0).abs() < 1.0,
        "second chunk should rescale to ≈ 200; got {second_w}"
    );
    // The second chunk's leftmost glyph sits at the tspan's x=500.
    assert!(
        (leftmost(second) - 500.0).abs() < 1.0,
        "second chunk start-anchored at x=500; got {}",
        leftmost(second)
    );
}

/// A negative `textLength` is an error per §11.2.1; we drop the
/// attribute so the run shapes at its natural width.
#[test]
fn negative_text_length_is_ignored() {
    install_resolver();
    let (b_left, b_right) = baseline_extent();
    let baseline_w = b_right - b_left;

    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="800" height="100">
  <text x="100" y="50" font-size="16" textLength="-50">ABCDE</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let groups = text_groups(&frame);
    let xs = xs_of(groups[0]);
    let w = rightmost(&xs) - leftmost(&xs);
    assert!(
        (w - baseline_w).abs() < 1e-3,
        "negative textLength must be ignored: expected natural width {baseline_w}; got {w}"
    );
}
