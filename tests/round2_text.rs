//! Round 2 — `<text>` / `<tspan>` parsing via scribe's vector-first
//! API. These tests exercise the structural parsing only (no font
//! resolver registered) so they don't need real font bytes — every
//! `<text>` is expected to parse to an empty `Group` when no resolver
//! is installed.

#![cfg(feature = "text")]

use oxideav_core::Node;
use oxideav_svg::parse_svg;

#[test]
fn text_without_font_resolver_parses_to_empty_group() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <text x="10" y="50" font-size="24" font-family="sans-serif">Hello</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(frame.root.children.len(), 1);
    match &frame.root.children[0] {
        Node::Group(g) => {
            // No font resolver means no glyphs were emitted — the
            // wrapper Group exists but has no children.
            assert!(g.children.is_empty(), "no glyphs expected without resolver");
        }
        other => panic!("expected Group, got {:?}", other),
    }
}

#[test]
fn text_with_nested_tspan_parses() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <text x="10" y="50">Hello <tspan dx="5" dy="-2" font-size="32">World</tspan></text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(frame.root.children.len(), 1);
    // Just verifying no panic / error when nested tspans are present.
    assert!(matches!(&frame.root.children[0], Node::Group(_)));
}

#[test]
fn text_round_trip_keeps_document_intact() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <rect width="100" height="100" fill="red"/>
  <text x="20" y="50" font-size="16">Hi</text>
</svg>"##;
    let frame = oxideav_svg::parse_svg(src).expect("parse");
    let bytes = oxideav_svg::write_svg(&frame);
    let frame2 = oxideav_svg::parse_svg(&bytes).expect("re-parse");
    assert_eq!(frame.width, frame2.width);
    // The rect should still be there after round-trip.
    let has_path = frame2.root.children.iter().any(|c| match c {
        Node::Path(_) => true,
        Node::Group(g) => g.children.iter().any(|cc| matches!(cc, Node::Path(_))),
        _ => false,
    });
    assert!(has_path, "rect path lost during text round-trip");
}
