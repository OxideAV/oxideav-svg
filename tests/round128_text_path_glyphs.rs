//! Round 128 — `<textPath>` end-to-end glyph emission with a font
//! resolver installed.
//!
//! This file runs in its own integration-test binary so the global
//! one-shot font resolver doesn't conflict with the parser-only tests
//! in `round128_text_path.rs`.

#![cfg(feature = "text")]

use oxideav_core::Node;
use oxideav_scribe::{Face, FaceChain};
use oxideav_svg::{parse_svg, text::set_font_resolver};

const FONT: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");

/// Install the font resolver once for this binary; the OnceLock fails
/// the second call (subsequent tests use the same registered chain).
fn install_resolver() {
    let _ = set_font_resolver(move |_family, size_px| {
        let _ = size_px; // chain is size-agnostic; size is bound at shape time.
        Face::from_ttf_bytes(FONT.to_vec()).ok().map(FaceChain::new)
    });
}

/// Recursively count `Path` leaves in a scene-graph subtree. Each
/// `<textPath>` glyph emits one `Path` inside a placement Group inside
/// a glyph-cache Group, so the count matches the number of glyphs that
/// produced rendering output (whitespace + non-renderables are
/// skipped, matching `shape_to_paths`).
fn count_paths(node: &Node) -> usize {
    match node {
        Node::Path(_) => 1,
        Node::Group(g) => g.children.iter().map(count_paths).sum(),
        _ => 0,
    }
}

/// Recursively collect every `Group` whose `transform` is non-identity
/// (i.e. carries an explicit placement) along with the group itself.
fn collect_placed_groups<'a>(node: &'a Node, out: &mut Vec<&'a oxideav_core::Group>) {
    if let Node::Group(g) = node {
        let tx = g.transform;
        let is_identity = (tx.a - 1.0).abs() < 1e-6
            && tx.b.abs() < 1e-6
            && tx.c.abs() < 1e-6
            && (tx.d - 1.0).abs() < 1e-6
            && tx.e.abs() < 1e-6
            && tx.f.abs() < 1e-6;
        if !is_identity {
            out.push(g);
        }
        for c in &g.children {
            collect_placed_groups(c, out);
        }
    }
}

/// With a font resolver registered, a `<textPath>` along a horizontal
/// path emits at least one `Path` leaf per glyph and all glyph
/// placements have zero rotation (horizontal tangent).
#[test]
fn text_path_horizontal_emits_glyphs_with_zero_rotation() {
    install_resolver();

    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="400" height="100">
  <defs>
    <path id="line" d="M 10 50 L 390 50"/>
  </defs>
  <text font-size="16">
    <textPath href="#line">AB</textPath>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");

    // The outer text Group wraps a sequence of placement Groups, one
    // per emitted glyph.
    let root_child = &frame.root.children[0];
    let leaves = count_paths(root_child);
    assert!(
        leaves >= 2,
        "expected >= 2 glyph paths for \"AB\", got {leaves}"
    );

    // Every placement matrix has a near-zero rotation component (a≈1,
    // b≈0, c≈0, d≈1 — pure translate / scale).
    let mut placed = Vec::new();
    collect_placed_groups(root_child, &mut placed);
    let mut found_along_path = 0;
    for g in &placed {
        let t = g.transform;
        // Only inspect groups whose translate sits on the horizontal
        // line y≈50; that filters out the inner glyph-cache wrappers.
        if (t.f - 50.0).abs() < 1.0 {
            assert!(t.b.abs() < 1e-3, "horizontal tangent: b={}", t.b);
            assert!(t.c.abs() < 1e-3, "horizontal tangent: c={}", t.c);
            found_along_path += 1;
        }
    }
    assert!(
        found_along_path >= 2,
        "expected glyph placements along y=50; got {found_along_path}"
    );
}

/// On a vertical path the placement transforms should carry a ~90°
/// rotation (`a ≈ 0, b ≈ 1, c ≈ -1, d ≈ 0`).
#[test]
fn text_path_vertical_emits_glyphs_with_90deg_rotation() {
    install_resolver();

    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="400">
  <defs>
    <path id="line" d="M 50 10 L 50 390"/>
  </defs>
  <text font-size="16">
    <textPath href="#line">XY</textPath>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let root_child = &frame.root.children[0];
    assert!(count_paths(root_child) >= 2, "expected >= 2 glyph paths");

    let mut placed = Vec::new();
    collect_placed_groups(root_child, &mut placed);
    // Find the outer placement (transform.e ≈ 50, the x of the
    // vertical line) and assert its rotation component.
    let mut found = false;
    for g in &placed {
        let t = g.transform;
        if (t.e - 50.0).abs() < 1.0 && t.f > 10.0 && t.f < 390.0 {
            // 90° rotation: a≈0, b≈1, c≈-1, d≈0.
            assert!(t.a.abs() < 1e-2, "a should be 0 at 90°; got {}", t.a);
            assert!(
                (t.b - 1.0).abs() < 1e-2,
                "b should be 1 at 90°; got {}",
                t.b
            );
            assert!(
                (t.c + 1.0).abs() < 1e-2,
                "c should be -1 at 90°; got {}",
                t.c
            );
            assert!(t.d.abs() < 1e-2, "d should be 0 at 90°; got {}", t.d);
            found = true;
            break;
        }
    }
    assert!(found, "no placement found along the vertical line");
}

/// A `<textPath>` referencing a path that's too short for the text run
/// drops the off-path glyphs and renders nothing for a sufficiently
/// short path. With a 5px-long path and 16px-baseline text, the first
/// character's midpoint already lies past the path so no glyph paths
/// are emitted (only the empty wrapping Group).
#[test]
fn text_path_off_path_glyphs_dropped() {
    install_resolver();

    // Use a path so tiny (1px) that even the first glyph's midpoint at
    // ~half its advance lies past the end of the path. With font-size
    // 64 and a monospace face the first glyph midpoint sits at ~19px,
    // well beyond a 1px path. The SVG keeps text + textPath all on one
    // line so there's no inter-element whitespace producing space
    // glyphs that would interfere with the off-path count.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><defs><path id="tiny" d="M 0 50 L 1 50"/></defs><text font-size="64"><textPath href="#tiny">overflows</textPath></text></svg>"##;
    let frame = parse_svg(src).expect("parse");
    let leaves = count_paths(&frame.root.children[0]);
    // The first glyph's midpoint sits past the 1px path, so all are
    // dropped.
    assert_eq!(leaves, 0, "no glyphs should fit; got {leaves}");
}

/// `<textPath startOffset="…">` shifts every glyph's placement by the
/// offset along the path. With a horizontal path and the start-offset
/// expressed in user units, the first glyph's placement-x should be at
/// least `startOffset` units from the path's start point.
#[test]
fn text_path_start_offset_shifts_first_glyph() {
    install_resolver();

    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="400" height="100">
  <defs>
    <path id="line" d="M 0 50 L 400 50"/>
  </defs>
  <text font-size="16">
    <textPath href="#line" startOffset="100">A</textPath>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let root_child = &frame.root.children[0];

    let mut placed = Vec::new();
    collect_placed_groups(root_child, &mut placed);
    // The horizontal-line placement group should have e >= 100 (the
    // start offset). We look for a group whose y is near 50 (on the
    // path) — its x must be at least the start offset.
    let mut found = false;
    for g in &placed {
        let t = g.transform;
        if (t.f - 50.0).abs() < 1.0 {
            assert!(
                t.e >= 100.0 - 1.0,
                "first glyph x = {} should be >= startOffset=100",
                t.e
            );
            found = true;
            break;
        }
    }
    assert!(found, "no on-path placement found");
}
