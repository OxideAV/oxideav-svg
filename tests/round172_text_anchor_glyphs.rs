//! Round 172 — SVG 2 §11.10.1.1 `text-anchor` end-to-end glyph
//! placement test. Runs in its own integration-test binary because the
//! global font-resolver hook is one-shot (`OnceLock`).
//!
//! Each test parses three sibling `<text>` elements that share the
//! same x / y origin and content but differ only in `text-anchor`. The
//! same DejaVuSansMono font is shaped for all three, so the run width
//! `W` is identical, and the §11.10.1.1 specification predicts:
//!
//! - `start`: first glyph origin sits at `x`.
//! - `middle`: first glyph origin sits at `x − W / 2`.
//! - `end`: first glyph origin sits at `x − W`.

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

/// Recursively collect every leaf placement group's translate `(e, f)`
/// so we can sort by x and identify the leftmost glyph origin.
fn collect_translates(node: &Node, out: &mut Vec<(f32, f32)>) {
    if let Node::Group(g) = node {
        // A non-identity translate without children that are pure-text
        // glyphs counts as a placement; we still recurse so nested
        // wrappers contribute their own translates.
        let tx = g.transform;
        let is_identity = (tx.a - 1.0).abs() < 1e-6
            && tx.b.abs() < 1e-6
            && tx.c.abs() < 1e-6
            && (tx.d - 1.0).abs() < 1e-6
            && tx.e.abs() < 1e-6
            && tx.f.abs() < 1e-6;
        if !is_identity {
            out.push((tx.e, tx.f));
        }
        for c in &g.children {
            collect_translates(c, out);
        }
    }
}

/// Walk the scene graph to find every immediate `<text>` Group. The
/// decoder wraps each `<text>` in a Group at the root of the frame —
/// one per source `<text>` element. We return them in source order.
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

/// Return the minimum `e` (x-translate) across every non-identity
/// placement inside `g`. Approximates "leftmost glyph origin" for the
/// horizontal LTR fixtures used here.
fn leftmost_glyph_x(g: &oxideav_core::Group) -> f32 {
    let mut tr = Vec::new();
    for c in &g.children {
        collect_translates(c, &mut tr);
    }
    tr.iter().map(|(e, _)| *e).fold(f32::INFINITY, f32::min)
}

/// Three text elements (start / middle / end), same x origin, same
/// content, same font. After the §11.10.1.1 shift, the leftmost glyph
/// origins must satisfy `x_start - x_middle ≈ W / 2` and `x_start -
/// x_end ≈ W` (both positive — middle / end move the run leftwards).
#[test]
fn three_anchors_shift_leftmost_glyph_predictably() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="400" height="200">
  <text x="200" y="50" font-size="16">ABCDE</text>
  <text x="200" y="100" font-size="16" text-anchor="middle">ABCDE</text>
  <text x="200" y="150" font-size="16" text-anchor="end">ABCDE</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let groups = text_groups(&frame);
    assert_eq!(groups.len(), 3, "three <text> elements expected");

    let x_start = leftmost_glyph_x(groups[0]);
    let x_middle = leftmost_glyph_x(groups[1]);
    let x_end = leftmost_glyph_x(groups[2]);

    assert!(x_start.is_finite(), "no glyphs emitted for start");
    assert!(x_middle.is_finite(), "no glyphs emitted for middle");
    assert!(x_end.is_finite(), "no glyphs emitted for end");

    // Start anchors at x = 200, so the leftmost glyph origin is right
    // around 200 (the first glyph's pen origin sits at `x` for LTR
    // start-anchored text).
    assert!(
        (x_start - 200.0).abs() < 1.0,
        "start anchor first glyph x should be ≈200; got {x_start}"
    );

    // Run width `W = x_start_leftmost - x_end_leftmost` (end shifts
    // the run by -W). Use this to derive the expected middle shift.
    let w = x_start - x_end;
    assert!(
        w > 0.0,
        "expected positive run width; start={x_start} end={x_end}"
    );

    // `middle` shifts by `-W/2`; tolerate ±0.5 px for the per-glyph
    // sub-pixel translation noise.
    let expected_middle = x_start - w * 0.5;
    assert!(
        (x_middle - expected_middle).abs() < 0.5,
        "middle anchor should sit halfway: expected {expected_middle}, got {x_middle}"
    );
}

/// `text-anchor="start"` is the default — an absent attribute and an
/// explicit `start` must produce identical leftmost-glyph x values.
#[test]
fn default_matches_explicit_start() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="400" height="100">
  <text x="100" y="50" font-size="16">XYZ</text>
  <text x="100" y="80" font-size="16" text-anchor="start">XYZ</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let groups = text_groups(&frame);
    assert_eq!(groups.len(), 2);
    let x_default = leftmost_glyph_x(groups[0]);
    let x_explicit = leftmost_glyph_x(groups[1]);
    assert!(
        (x_default - x_explicit).abs() < 1e-3,
        "default and explicit start should match: {x_default} vs {x_explicit}"
    );
}

/// `text-anchor="end"` shifts the run leftwards by the full pre-anchor
/// width, so the leftmost glyph origin lands at a smaller x than the
/// origin attribute.
#[test]
fn end_anchor_moves_run_leftwards() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="400" height="100">
  <text x="300" y="50" font-size="16" text-anchor="end">HELLO</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let groups = text_groups(&frame);
    assert_eq!(groups.len(), 1);
    let x = leftmost_glyph_x(groups[0]);
    assert!(
        x.is_finite(),
        "expected at least one glyph for end-anchored run"
    );
    assert!(
        x < 300.0,
        "end anchor should move leftmost glyph left of x=300; got {x}"
    );
}

/// `text-anchor` inherits through `<g>`. A `<g text-anchor="middle">`
/// wrapping a `<text>` with no own anchor must produce the same
/// leftmost-glyph x as an inline `<text text-anchor="middle">`.
#[test]
fn inheritance_through_group_matches_inline_attribute() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="400" height="200">
  <g text-anchor="middle">
    <text x="200" y="50" font-size="16">ABCDE</text>
  </g>
  <text x="200" y="100" font-size="16" text-anchor="middle">ABCDE</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    // First child is the outer <g> (a Group containing the inner text
    // Group), second is the inline <text>.
    let groups = text_groups(&frame);
    assert_eq!(
        groups.len(),
        2,
        "expected outer-g group + inline text group"
    );

    // Drill into the outer <g> to find its inner <text> Group.
    let inner_text = groups[0]
        .children
        .iter()
        .find_map(|c| match c {
            Node::Group(g) => Some(g),
            _ => None,
        })
        .expect("inner text group");

    let x_inherited = leftmost_glyph_x(inner_text);
    let x_inline = leftmost_glyph_x(groups[1]);
    assert!(x_inherited.is_finite() && x_inline.is_finite());
    assert!(
        (x_inherited - x_inline).abs() < 1e-3,
        "inherited middle should match inline middle: {x_inherited} vs {x_inline}"
    );
}

/// Empty `<text>` (no character content) — no glyphs emitted, no
/// shift, no crash. The wrapping Group is empty for every anchor.
#[test]
fn empty_text_runs_emit_nothing_for_every_anchor() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <text x="50" y="50" font-size="16"></text>
  <text x="50" y="60" font-size="16" text-anchor="middle"></text>
  <text x="50" y="70" font-size="16" text-anchor="end"></text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let groups = text_groups(&frame);
    assert_eq!(groups.len(), 3);
    for g in groups {
        assert!(
            g.children.is_empty(),
            "empty text should yield zero children"
        );
    }
}
