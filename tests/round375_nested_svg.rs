//! Round 375 — nested `<svg>` viewport establishment (SVG 1.1 §7.10 /
//! SVG 2 §8.2).
//!
//! An inner `<svg>` establishes a new viewport: its `x` / `y` place the
//! viewport in the current user space, `width` / `height` size it, and a
//! `viewBox` + `preserveAspectRatio` re-map the inner coordinate system.
//! Before this round an inner `<svg>` fell through the element-dispatch
//! deferral and was dropped along with its whole subtree.
//!
//! These tests assert (a) the inner subtree now reaches the scene graph
//! and (b) the resulting group's transform maps inner-viewport
//! coordinates the way §8.2 prescribes — verified by applying the
//! transform to a control point rather than by inspecting matrix cells.

use oxideav_core::{Node, Point, Transform2D};
use oxideav_svg::parse_svg;

/// Recursively find the first `Node::Group` whose `children` contains a
/// `Node::Path`, returning the accumulated transform from the root down
/// to (and including) that group.
fn first_group_with_path(g: &oxideav_core::Group, acc: Transform2D) -> Option<Transform2D> {
    let here = acc.compose(&g.transform);
    if g.children.iter().any(|c| matches!(c, Node::Path(_))) {
        return Some(here);
    }
    for c in &g.children {
        if let Node::Group(sg) = c {
            if let Some(t) = first_group_with_path(sg, here) {
                return Some(t);
            }
        }
    }
    None
}

fn count_paths(g: &oxideav_core::Group) -> usize {
    let mut n = 0;
    for c in &g.children {
        match c {
            Node::Path(_) => n += 1,
            Node::Group(sg) => n += count_paths(sg),
            _ => {}
        }
    }
    n
}

#[test]
fn nested_svg_subtree_is_no_longer_dropped() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <svg x="10" y="20" width="40" height="40">
    <rect x="0" y="0" width="10" height="10" fill="red"/>
  </svg>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(
        count_paths(&frame.root),
        1,
        "the nested <svg>'s child <rect> must reach the scene graph"
    );
}

#[test]
fn nested_svg_x_y_translate_the_viewport() {
    // No viewBox → only the x/y placement applies (the inner viewport's
    // user space coincides 1:1 with the new viewport). A point at the
    // inner origin (0,0) must land at (x, y) = (10, 20).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <svg x="10" y="20" width="40" height="40">
    <rect x="0" y="0" width="10" height="10" fill="red"/>
  </svg>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let t = first_group_with_path(&frame.root, Transform2D::identity())
        .expect("a group carrying the rect");
    let mapped = t.apply(Point { x: 0.0, y: 0.0 });
    assert!(
        (mapped.x - 10.0).abs() < 1e-4 && (mapped.y - 20.0).abs() < 1e-4,
        "inner origin maps to (x,y)=(10,20), got ({},{})",
        mapped.x,
        mapped.y
    );
}

#[test]
fn nested_svg_viewbox_scales_the_inner_coordinates() {
    // A 0 0 10 10 viewBox mapped onto a 40×40 viewport scales by 4 and
    // (with no x/y) keeps the origin at (0,0). A point at inner (10,10)
    // maps to (40,40); at inner (5,5) → (20,20).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <svg x="0" y="0" width="40" height="40" viewBox="0 0 10 10">
    <rect x="0" y="0" width="10" height="10" fill="red"/>
  </svg>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let t = first_group_with_path(&frame.root, Transform2D::identity())
        .expect("a group carrying the rect");
    let mid = t.apply(Point { x: 5.0, y: 5.0 });
    assert!(
        (mid.x - 20.0).abs() < 1e-4 && (mid.y - 20.0).abs() < 1e-4,
        "viewBox scale ×4: inner (5,5) → (20,20), got ({},{})",
        mid.x,
        mid.y
    );
}

#[test]
fn nested_svg_viewbox_with_offset_and_placement() {
    // viewBox min (5,5) shifts the inner origin; x/y place the viewport.
    // scale = 40/10 = 4. A point at inner (5,5) (= viewBox min) maps to
    // the viewport origin, then the x/y translate puts it at (30,30).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <svg x="30" y="30" width="40" height="40" viewBox="5 5 10 10">
    <rect x="5" y="5" width="2" height="2" fill="red"/>
  </svg>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let t = first_group_with_path(&frame.root, Transform2D::identity())
        .expect("a group carrying the rect");
    let corner = t.apply(Point { x: 5.0, y: 5.0 });
    assert!(
        (corner.x - 30.0).abs() < 1e-4 && (corner.y - 30.0).abs() < 1e-4,
        "viewBox-min corner → viewport origin + (30,30), got ({},{})",
        corner.x,
        corner.y
    );
}

#[test]
fn nested_svg_zero_size_disables_rendering() {
    // §8.2 step 1: a zero (or negative) width/height suppresses the
    // element and its children.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <svg x="0" y="0" width="0" height="40">
    <rect x="0" y="0" width="10" height="10" fill="red"/>
  </svg>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(
        count_paths(&frame.root),
        0,
        "a zero-width nested <svg> drops its subtree"
    );
}

#[test]
fn nested_svg_default_dimensions_fill_parent() {
    // Absent width/height default to 100% of the parent viewport; absent
    // x/y default to 0. The subtree renders unshifted.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <svg>
    <rect x="0" y="0" width="10" height="10" fill="red"/>
  </svg>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let t = first_group_with_path(&frame.root, Transform2D::identity())
        .expect("a group carrying the rect");
    let origin = t.apply(Point { x: 0.0, y: 0.0 });
    assert!(
        origin.x.abs() < 1e-4 && origin.y.abs() < 1e-4,
        "default x/y=0 keeps the inner origin at (0,0), got ({},{})",
        origin.x,
        origin.y
    );
    assert_eq!(count_paths(&frame.root), 1, "subtree renders");
}
