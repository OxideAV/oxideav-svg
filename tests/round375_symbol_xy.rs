//! Round 375 — SVG 2 §5.5 `<symbol>` `x` / `y` geometry properties.
//!
//! "The x, y, width, and height geometry properties have the same effect
//! as on an `svg` element, when the `symbol` is instantiated by a `use`
//! element." (New in SVG 2.) The symbol's `x` / `y` position its
//! viewport inside the `<use>`'s coordinate system; the use's own
//! `x` / `y` translate is layered on top. Before this round the symbol's
//! `x` / `y` were ignored.

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
fn symbol_xy_offsets_the_instantiated_viewport() {
    // viewBox 0 0 100 100, use at 50×50 → scale 0.5. symbol x=10 y=20.
    // The viewBox origin (0,0) maps (scale 0.5) to (0,0) then the
    // symbol's x/y translate to (10,20); the use is at the default
    // (0,0). So a content point at viewBox (0,0) lands at (10,20).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <symbol id="box" viewBox="0 0 100 100" x="10" y="20">
      <rect x="0" y="0" width="4" height="4" fill="red"/>
    </symbol>
  </defs>
  <use href="#box" width="50" height="50"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let t = transform_to_first_path(&frame.root, Transform2D::identity())
        .expect("a group carrying the rect");
    let origin = t.apply(Point { x: 0.0, y: 0.0 });
    assert!(
        (origin.x - 10.0).abs() < 1e-3 && (origin.y - 20.0).abs() < 1e-3,
        "symbol x/y offsets viewBox origin to (10,20), got ({},{})",
        origin.x,
        origin.y
    );
}

#[test]
fn symbol_xy_composes_with_use_xy() {
    // symbol x=10 y=20, use x=5 y=7 → total offset of the viewBox origin
    // is (15, 27).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <symbol id="box" viewBox="0 0 100 100" x="10" y="20">
      <rect x="0" y="0" width="4" height="4" fill="red"/>
    </symbol>
  </defs>
  <use href="#box" x="5" y="7" width="50" height="50"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let t = transform_to_first_path(&frame.root, Transform2D::identity())
        .expect("a group carrying the rect");
    let origin = t.apply(Point { x: 0.0, y: 0.0 });
    assert!(
        (origin.x - 15.0).abs() < 1e-3 && (origin.y - 27.0).abs() < 1e-3,
        "symbol x/y + use x/y → (15,27), got ({},{})",
        origin.x,
        origin.y
    );
}

#[test]
fn symbol_without_xy_unchanged() {
    // No symbol x/y → the viewBox origin maps to the use origin (0,0).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <symbol id="box" viewBox="0 0 100 100">
      <rect x="0" y="0" width="4" height="4" fill="red"/>
    </symbol>
  </defs>
  <use href="#box" width="50" height="50"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let t = transform_to_first_path(&frame.root, Transform2D::identity())
        .expect("a group carrying the rect");
    let origin = t.apply(Point { x: 0.0, y: 0.0 });
    assert!(
        origin.x.abs() < 1e-3 && origin.y.abs() < 1e-3,
        "no symbol x/y: viewBox origin → (0,0), got ({},{})",
        origin.x,
        origin.y
    );
}
