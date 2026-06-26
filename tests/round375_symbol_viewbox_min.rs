//! Round 375 — regression for the `<symbol>` / `<use>` §8.2 viewport
//! transform with a *non-zero* viewBox `min-x` / `min-y`.
//!
//! The §8.2 equivalent transform is
//!   `translate(align) ∘ scale(sx, sy) ∘ translate(-min_x, -min_y)`
//! so the viewBox-min corner maps to the viewport origin (plus any
//! meet/slice alignment offset). The transform helper previously seeded
//! its alignment translate with an extra `-min·scale` term, double-
//! counting the min translation; a viewBox with `min=0` (the common
//! case, and every prior test) was unaffected, hiding the error. These
//! tests pin a non-zero-min symbol so the corner lands where §8.2 says.

use oxideav_core::{Node, Point, Transform2D};
use oxideav_svg::parse_svg;

/// Accumulate the transform from the root down to the first group that
/// directly holds a `Node::Path`.
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
fn use_of_symbol_with_nonzero_viewbox_min_maps_corner_to_origin() {
    // viewBox "10 20 100 100" instantiated at 50×50 → uniform scale 0.5
    // (aspect ratios match, default xMidYMid meet → no letterbox). The
    // viewBox-min corner (10, 20) must map to the viewport origin (0,0);
    // the use places the viewport at the default (0,0).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <symbol id="logo" viewBox="10 20 100 100">
      <rect x="10" y="20" width="20" height="20" fill="red"/>
    </symbol>
  </defs>
  <use href="#logo" width="50" height="50"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let t = transform_to_first_path(&frame.root, Transform2D::identity())
        .expect("a group carrying the rect");
    let corner = t.apply(Point { x: 10.0, y: 20.0 });
    assert!(
        corner.x.abs() < 1e-3 && corner.y.abs() < 1e-3,
        "viewBox-min (10,20) → viewport origin (0,0), got ({},{})",
        corner.x,
        corner.y
    );
    // A point at viewBox (10+100, 20+100) = the far corner maps to the
    // viewport far corner (50, 50) under the 0.5 scale.
    let far = t.apply(Point { x: 110.0, y: 120.0 });
    assert!(
        (far.x - 50.0).abs() < 1e-3 && (far.y - 50.0).abs() < 1e-3,
        "viewBox far corner → viewport (50,50), got ({},{})",
        far.x,
        far.y
    );
}

#[test]
fn use_of_symbol_with_nonzero_min_and_use_xy_placement() {
    // Same symbol, but the use also places the viewport at (30, 40).
    // The viewBox-min corner now maps to (30, 40).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <symbol id="logo" viewBox="10 20 100 100">
      <rect x="10" y="20" width="20" height="20" fill="red"/>
    </symbol>
  </defs>
  <use href="#logo" x="30" y="40" width="50" height="50"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let t = transform_to_first_path(&frame.root, Transform2D::identity())
        .expect("a group carrying the rect");
    let corner = t.apply(Point { x: 10.0, y: 20.0 });
    assert!(
        (corner.x - 30.0).abs() < 1e-3 && (corner.y - 40.0).abs() < 1e-3,
        "viewBox-min → use placement (30,40), got ({},{})",
        corner.x,
        corner.y
    );
}
