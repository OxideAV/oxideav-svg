//! Round 3 — `<use href="#id">` cross-references.
//!
//! Resolves a referenced element from the documentwide id table built
//! during the pre-walk. `<symbol>` references inline the symbol's
//! children; cycles are detected and dropped.

use oxideav_core::Node;
use oxideav_svg::parse_svg;

fn count_paths(g: &oxideav_core::Group) -> usize {
    let mut n = 0;
    for c in &g.children {
        match c {
            Node::Path(_) => n += 1,
            Node::Group(sg) => n += count_paths(sg),
            Node::SoftMask { content, .. } => {
                if let Node::Group(sg) = content.as_ref() {
                    n += count_paths(sg);
                }
            }
            _ => {}
        }
    }
    n
}

#[test]
fn use_of_rect_emits_a_path_under_a_group() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <rect id="r1" x="0" y="0" width="20" height="20" fill="red"/>
  </defs>
  <use href="#r1" x="10" y="10"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(
        frame.root.children.len(),
        1,
        "<use> should produce exactly one root child"
    );
    // The first (and only) child must be a Group wrapping the
    // re-instantiated path.
    match &frame.root.children[0] {
        Node::Group(g) => {
            // x=10 y=10 → translate(10, 10) on the group.
            assert!((g.transform.e - 10.0).abs() < f32::EPSILON);
            assert!((g.transform.f - 10.0).abs() < f32::EPSILON);
            assert!(
                count_paths(g) >= 1,
                "instantiated rect should produce at least one path"
            );
        }
        other => panic!("expected Group, got {:?}", other),
    }
}

#[test]
fn use_of_symbol_inlines_symbol_children() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <symbol id="sym1">
      <rect x="0" y="0" width="10" height="10" fill="green"/>
      <circle cx="20" cy="20" r="5" fill="blue"/>
    </symbol>
  </defs>
  <use href="#sym1"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let g = match &frame.root.children[0] {
        Node::Group(g) => g,
        other => panic!("expected Group, got {:?}", other),
    };
    assert_eq!(
        count_paths(g),
        2,
        "symbol's two children should both instantiate"
    );
}

#[test]
fn use_with_xlink_href_legacy_attribute() {
    // SVG 1.1 used `xlink:href`; SVG 2 added bare `href`. We accept both.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="50" height="50">
  <defs>
    <circle id="c" cx="0" cy="0" r="10" fill="red"/>
  </defs>
  <use xlink:href="#c" x="25" y="25"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(frame.root.children.len(), 1);
    let g = match &frame.root.children[0] {
        Node::Group(g) => g,
        other => panic!("expected Group, got {:?}", other),
    };
    assert!((g.transform.e - 25.0).abs() < f32::EPSILON);
    assert!((g.transform.f - 25.0).abs() < f32::EPSILON);
    assert_eq!(count_paths(g), 1);
}

#[test]
fn use_unknown_id_is_silently_dropped() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <use href="#missing"/>
  <rect x="0" y="0" width="10" height="10" fill="black"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    // The <use> drops; the <rect> remains.
    assert_eq!(frame.root.children.len(), 1);
    assert!(matches!(&frame.root.children[0], Node::Path(_)));
}

#[test]
fn use_without_href_is_silently_dropped() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <use/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert!(frame.root.children.is_empty());
}

#[test]
fn use_external_reference_is_dropped() {
    // External (`other.svg#id`) references aren't supported.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <use href="other.svg#x"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert!(frame.root.children.is_empty());
}

#[test]
fn use_cycle_does_not_recurse_infinitely() {
    // `<symbol id="s">` contains `<use href="#s"/>` — a cycle. The
    // parser must terminate (cycle detection drops the inner use).
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <defs>
    <symbol id="s">
      <rect x="0" y="0" width="5" height="5" fill="red"/>
      <use href="#s"/>
    </symbol>
  </defs>
  <use href="#s"/>
</svg>"##;
    let frame = parse_svg(src).expect("cycle parses without hanging");
    // The outer <use> instantiates the symbol once; the inner cyclic
    // <use> is dropped. So we expect exactly one path (the rect).
    let g = match &frame.root.children[0] {
        Node::Group(g) => g,
        other => panic!("expected Group, got {:?}", other),
    };
    assert_eq!(count_paths(g), 1);
}

#[test]
fn use_of_group_instantiates_all_children() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <g id="duo">
      <rect x="0" y="0" width="10" height="10" fill="red"/>
      <rect x="20" y="0" width="10" height="10" fill="blue"/>
    </g>
  </defs>
  <use href="#duo" x="5" y="5"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let outer = match &frame.root.children[0] {
        Node::Group(g) => g,
        other => panic!("expected Group, got {:?}", other),
    };
    // outer: translate(5, 5) wrapping a re-parsed `<g>` node.
    assert!((outer.transform.e - 5.0).abs() < f32::EPSILON);
    assert_eq!(count_paths(outer), 2);
}

#[test]
fn use_with_transform_attribute_is_honored() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <defs>
    <rect id="r" width="10" height="10" fill="red"/>
  </defs>
  <use href="#r" transform="translate(20, 30)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let g = match &frame.root.children[0] {
        Node::Group(g) => g,
        other => panic!("expected Group, got {:?}", other),
    };
    assert!((g.transform.e - 20.0).abs() < f32::EPSILON);
    assert!((g.transform.f - 30.0).abs() < f32::EPSILON);
}
