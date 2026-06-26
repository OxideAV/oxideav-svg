//! Round 375 — SVG 2 §5.5 `<symbol>` `refX` / `refY` reference point.
//!
//! SVG 2 added `refX` / `refY` to `<symbol>` ("Added to make it easier
//! to align symbols to a particular point, as is often done in maps.
//! Similar to the matching attributes on `marker`"). The reference
//! point (in the symbol's own coordinate system) is aligned with the
//! instantiating `<use>`'s `x` / `y`. Before this round the attributes
//! were ignored, so a `<symbol refX refY>` was positioned by its
//! top-left corner instead of its reference point.

use oxideav_core::{Node, Point, Transform2D};
use oxideav_svg::parse_svg;

fn transform_to_first_path(g: &oxideav_core::Group, acc: Transform2D) -> Option<Transform2D> {
    let here = acc.compose(&g.transform);
    if g.children.iter().any(|c| matches!(c, Node::Path(_))) {
        return Some(here);
    }
    for c in &g.children {
        if let Node::Group(sg) = c {
            if let Some(t) = transform_to_first_path(sg, here) {
                return Some(t);
            }
        }
    }
    None
}

#[test]
fn symbol_refx_refy_aligns_reference_point_with_use_origin() {
    // viewBox 0 0 100 100, instantiated at 50×50 → scale 0.5. refX/refY
    // = (50,50) is the symbol centre. The use is at the default (0,0),
    // so the symbol-centre point must land at (0,0). A content point at
    // viewBox (50,50) therefore maps to the use origin (0,0); without
    // refX/refY it would map to (25,25).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <symbol id="dot" viewBox="0 0 100 100" refX="50" refY="50">
      <rect x="50" y="50" width="4" height="4" fill="red"/>
    </symbol>
  </defs>
  <use href="#dot" width="50" height="50"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let t = transform_to_first_path(&frame.root, Transform2D::identity())
        .expect("a group carrying the rect");
    let centre = t.apply(Point { x: 50.0, y: 50.0 });
    assert!(
        centre.x.abs() < 1e-3 && centre.y.abs() < 1e-3,
        "refX/refY centre → use origin (0,0), got ({},{})",
        centre.x,
        centre.y
    );
}

#[test]
fn symbol_refx_refy_with_use_placement() {
    // Same symbol, use placed at (80, 90). The reference point lands at
    // (80, 90).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <symbol id="dot" viewBox="0 0 100 100" refX="50" refY="50">
      <rect x="50" y="50" width="4" height="4" fill="red"/>
    </symbol>
  </defs>
  <use href="#dot" x="80" y="90" width="50" height="50"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let t = transform_to_first_path(&frame.root, Transform2D::identity())
        .expect("a group carrying the rect");
    let centre = t.apply(Point { x: 50.0, y: 50.0 });
    assert!(
        (centre.x - 80.0).abs() < 1e-3 && (centre.y - 90.0).abs() < 1e-3,
        "refX/refY centre → use placement (80,90), got ({},{})",
        centre.x,
        centre.y
    );
}

#[test]
fn symbol_ref_geometric_keywords_resolve_against_viewbox() {
    // refX="center" refY="center" resolves to the viewBox centre
    // (50,50) for a 0 0 100 100 viewBox — same result as the numeric
    // case above.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <symbol id="dot" viewBox="0 0 100 100" refX="center" refY="center">
      <rect x="50" y="50" width="4" height="4" fill="red"/>
    </symbol>
  </defs>
  <use href="#dot" width="50" height="50"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let t = transform_to_first_path(&frame.root, Transform2D::identity())
        .expect("a group carrying the rect");
    let centre = t.apply(Point { x: 50.0, y: 50.0 });
    assert!(
        centre.x.abs() < 1e-3 && centre.y.abs() < 1e-3,
        "refX/refY=center → viewBox centre → use origin, got ({},{})",
        centre.x,
        centre.y
    );
}

#[test]
fn symbol_without_ref_keeps_corner_placement() {
    // No refX/refY → the symbol's top-left (viewBox origin) maps to the
    // use origin, so the viewBox-centre point (50,50) maps to (25,25)
    // under the 0.5 scale (the pre-round-375 behaviour, unchanged).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <symbol id="dot" viewBox="0 0 100 100">
      <rect x="50" y="50" width="4" height="4" fill="red"/>
    </symbol>
  </defs>
  <use href="#dot" width="50" height="50"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let t = transform_to_first_path(&frame.root, Transform2D::identity())
        .expect("a group carrying the rect");
    let centre = t.apply(Point { x: 50.0, y: 50.0 });
    assert!(
        (centre.x - 25.0).abs() < 1e-3 && (centre.y - 25.0).abs() < 1e-3,
        "no refX/refY: viewBox centre → (25,25), got ({},{})",
        centre.x,
        centre.y
    );
}
