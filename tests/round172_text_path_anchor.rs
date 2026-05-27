//! Round 172 — SVG 2 §11.8.3 `<textPath>` start-point-on-the-path bias
//! by the `text-anchor` property. Three sibling `<textPath>` elements
//! along the same straight horizontal path differ only in their
//! inherited `text-anchor`; the §11.8.3 algorithm predicts:
//!
//! - `start`: first glyph midpoint sits at `startOffset` along the path.
//! - `middle`: first glyph midpoint at `startOffset − W / 2`.
//! - `end`: first glyph midpoint at `startOffset − W`.
//!
//! For a horizontal path the path-distance maps linearly to x, so the
//! same leftmost-glyph-x predictions used for the plain `<text>` test
//! apply here.

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

fn collect_translates(node: &Node, out: &mut Vec<(f32, f32)>) {
    if let Node::Group(g) = node {
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

/// Walk the scene graph and return the minimum-x translate of every
/// non-identity placement that sits on the horizontal line y ≈
/// `y_target`. For a horizontal path the textPath glyphs land at
/// `(start_offset + midpoint_x, y_target)` — the leftmost one is the
/// first emitted glyph.
fn leftmost_on_line(frame: &oxideav_core::VectorFrame, y_target: f32) -> f32 {
    let mut tr = Vec::new();
    for c in &frame.root.children {
        collect_translates(c, &mut tr);
    }
    tr.iter()
        .filter(|(_, f)| (f - y_target).abs() < 1.0)
        .map(|(e, _)| *e)
        .fold(f32::INFINITY, f32::min)
}

#[test]
fn text_path_anchor_biases_start_point_per_spec() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="300">
  <defs>
    <path id="line_a" d="M 0 50 L 600 50"/>
    <path id="line_b" d="M 0 150 L 600 150"/>
    <path id="line_c" d="M 0 250 L 600 250"/>
  </defs>
  <text font-size="16">
    <textPath href="#line_a" startOffset="300">ABCDE</textPath>
  </text>
  <text font-size="16" text-anchor="middle">
    <textPath href="#line_b" startOffset="300">ABCDE</textPath>
  </text>
  <text font-size="16" text-anchor="end">
    <textPath href="#line_c" startOffset="300">ABCDE</textPath>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");

    // Each <textPath> lives on its own horizontal line; leftmost
    // glyph translation along that line is the first-glyph midpoint
    // (or close enough — the per-glyph centring `-x_advance/2`
    // translate composes into the outer placement, but our
    // collect_translates walks ALL non-identity transforms, so we
    // pick up the outermost placement closest to the path baseline).
    let left_start = leftmost_on_line(&frame, 50.0);
    let left_middle = leftmost_on_line(&frame, 150.0);
    let left_end = leftmost_on_line(&frame, 250.0);

    assert!(left_start.is_finite(), "no glyphs along start path");
    assert!(left_middle.is_finite(), "no glyphs along middle path");
    assert!(left_end.is_finite(), "no glyphs along end path");

    // Predictions per §11.8.3:
    //   middle = start - W/2
    //   end    = start - W
    // We derive W from start - end and verify middle sits at half.
    let w = left_start - left_end;
    assert!(
        w > 0.0,
        "expected positive width; start={left_start}, end={left_end}"
    );
    let expected_middle = left_start - w * 0.5;
    assert!(
        (left_middle - expected_middle).abs() < 0.5,
        "middle anchor leftmost should be ≈{expected_middle}; got {left_middle}"
    );
}

#[test]
fn text_path_default_anchor_is_start() {
    install_resolver();
    // Two paths, identical except one omits text-anchor (default
    // start) and one is explicit start. Leftmost glyph x must match.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="400" height="200">
  <defs>
    <path id="p1" d="M 0 60 L 400 60"/>
    <path id="p2" d="M 0 140 L 400 140"/>
  </defs>
  <text font-size="16">
    <textPath href="#p1" startOffset="50">ABCDE</textPath>
  </text>
  <text font-size="16" text-anchor="start">
    <textPath href="#p2" startOffset="50">ABCDE</textPath>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let left_default = leftmost_on_line(&frame, 60.0);
    let left_explicit = leftmost_on_line(&frame, 140.0);
    assert!(left_default.is_finite() && left_explicit.is_finite());
    assert!(
        (left_default - left_explicit).abs() < 1e-3,
        "default and explicit start should match: {left_default} vs {left_explicit}"
    );
}
