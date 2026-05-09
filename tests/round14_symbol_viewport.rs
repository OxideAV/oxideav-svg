//! Round 14 — `<symbol>` + `<use>` viewBox / width / height resolution
//! per SVG 2 §5.5 + §5.6 + §8.2.
//!
//! Today (round 13) `<symbol>` was preserved structurally but `<use
//! href="#sym">` instantiated the symbol's content WITHOUT applying:
//!
//! - The symbol's `viewBox` (defines a coordinate system the use's
//!   viewport maps onto).
//! - The use's `width` + `height` (defines the destination viewport).
//! - The symbol's `preserveAspectRatio` (controls scaling within the
//!   viewport).
//!
//! Round 14 closes that gap: the use-instantiation path now wraps the
//! symbol's children in an inner Group carrying the §8.2 viewport
//! transform before the outer Group applies the use's `transform=` /
//! `x` / `y` / `opacity`.

use oxideav_core::Node;
use oxideav_svg::parse_svg;

#[test]
fn use_of_symbol_with_viewbox_and_width_height_scales_by_half() {
    // Symbol is a 100×100 viewBox; use instantiates at 50×50 → uniform
    // scale 0.5 (default `preserveAspectRatio="xMidYMid meet"` —
    // aspect ratios match so no letterbox).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <symbol id="logo" viewBox="0 0 100 100">
      <rect x="0" y="0" width="100" height="100" fill="red"/>
    </symbol>
  </defs>
  <use href="#logo" width="50" height="50"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(frame.root.children.len(), 1);
    let outer = match &frame.root.children[0] {
        Node::Group(g) => g,
        other => panic!("expected outer Group from <use>, got {:?}", other),
    };
    // Outer group carries the use's translate (default 0,0) — should
    // be identity.
    assert!((outer.transform.a - 1.0).abs() < 1e-6);
    assert!((outer.transform.d - 1.0).abs() < 1e-6);
    assert!((outer.transform.e).abs() < 1e-6);
    assert!((outer.transform.f).abs() < 1e-6);
    assert_eq!(
        outer.children.len(),
        1,
        "viewport transform wrap should produce exactly one inner Group"
    );
    let inner = match &outer.children[0] {
        Node::Group(g) => g,
        other => panic!(
            "expected inner Group carrying viewport transform, got {:?}",
            other
        ),
    };
    // Inner group: scale(0.5, 0.5) (50/100 × 50/100, meet → uniform).
    assert!(
        (inner.transform.a - 0.5).abs() < 1e-6,
        "expected scale-x 0.5 got {}",
        inner.transform.a
    );
    assert!(
        (inner.transform.d - 0.5).abs() < 1e-6,
        "expected scale-y 0.5 got {}",
        inner.transform.d
    );
}

#[test]
fn use_of_symbol_falls_back_to_intrinsic_width_height() {
    // Symbol carries its own width="80" height="80" — use omits them,
    // so the viewport is 80×80, viewBox is 0..160 → scale 0.5.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <symbol id="s" viewBox="0 0 160 160" width="80" height="80">
    <rect x="0" y="0" width="160" height="160" fill="blue"/>
  </symbol>
  <use href="#s"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let outer = match &frame.root.children[0] {
        Node::Group(g) => g,
        _ => panic!("outer Group missing"),
    };
    assert_eq!(outer.children.len(), 1);
    let inner = match &outer.children[0] {
        Node::Group(g) => g,
        _ => panic!("inner viewport Group missing"),
    };
    assert!((inner.transform.a - 0.5).abs() < 1e-6);
    assert!((inner.transform.d - 0.5).abs() < 1e-6);
}

#[test]
fn use_translate_x_y_composes_with_viewport_transform() {
    // x=10, y=20 → outer translate(10, 20). Symbol viewBox 0..100 →
    // viewport 50×50 → inner scale(0.5).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <symbol id="s" viewBox="0 0 100 100">
    <rect x="0" y="0" width="100" height="100" fill="green"/>
  </symbol>
  <use href="#s" x="10" y="20" width="50" height="50"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let outer = match &frame.root.children[0] {
        Node::Group(g) => g,
        _ => panic!("outer Group missing"),
    };
    // Outer: translate(10, 20).
    assert!((outer.transform.e - 10.0).abs() < 1e-6);
    assert!((outer.transform.f - 20.0).abs() < 1e-6);
    let inner = match &outer.children[0] {
        Node::Group(g) => g,
        _ => panic!("inner viewport Group missing"),
    };
    // Inner: scale(0.5).
    assert!((inner.transform.a - 0.5).abs() < 1e-6);
    assert!((inner.transform.d - 0.5).abs() < 1e-6);
}

#[test]
fn use_of_symbol_without_viewbox_skips_viewport_transform() {
    // No viewBox on symbol → no §8.2 mapping, so the use's
    // width/height are ignored per spec. The decoder should NOT add
    // an inner viewport Group.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <symbol id="s">
    <rect x="0" y="0" width="20" height="20" fill="cyan"/>
  </symbol>
  <use href="#s" width="50" height="50"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let outer = match &frame.root.children[0] {
        Node::Group(g) => g,
        _ => panic!("outer Group missing"),
    };
    // Without a viewBox there should NOT be an inner viewport Group —
    // the symbol's children appear directly under the outer Group.
    assert!(
        !outer.children.is_empty(),
        "symbol's children should be inlined"
    );
    // First child should be the rect (a Path), not a Group from the
    // viewport wrap.
    assert!(
        matches!(outer.children[0], Node::Path(_)),
        "expected Path child (no viewport wrap), got {:?}",
        outer.children[0]
    );
}

#[test]
fn use_of_symbol_with_meet_letterbox_centers_content() {
    // Symbol viewBox 0..100×100, use viewport 200×100, default
    // `xMidYMid meet`. Spec scale = min(2, 1) = 1, then translate-x
    // = (200 - 100)/2 = 50.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="400" height="200">
  <symbol id="s" viewBox="0 0 100 100">
    <rect x="0" y="0" width="100" height="100" fill="magenta"/>
  </symbol>
  <use href="#s" width="200" height="100"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let outer = match &frame.root.children[0] {
        Node::Group(g) => g,
        _ => panic!("outer Group missing"),
    };
    let inner = match &outer.children[0] {
        Node::Group(g) => g,
        _ => panic!("inner viewport Group missing"),
    };
    // sx = sy = 1 (meet); tx = 50 (xMid); ty = 0 (yMid → 0 here).
    assert!((inner.transform.a - 1.0).abs() < 1e-6);
    assert!((inner.transform.d - 1.0).abs() < 1e-6);
    assert!(
        (inner.transform.e - 50.0).abs() < 1e-6,
        "expected tx=50 (xMid centring), got {}",
        inner.transform.e
    );
    assert!((inner.transform.f - 0.0).abs() < 1e-6);
}
