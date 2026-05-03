//! Round-trip a minimal SVG (rect + circle) and assert the parsed
//! [`VectorFrame`] preserves shape count and dimensions.

use oxideav_core::Node;
use oxideav_svg::parse_svg;

#[test]
fn parses_rect_and_circle_and_preserves_count() {
    let src = br##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="10" y="10" width="80" height="40" fill="red" stroke="black" stroke-width="2"/>
  <circle cx="50" cy="75" r="20" fill="#00ff00"/>
</svg>"##;

    let frame = parse_svg(src).expect("svg parses");

    assert_eq!(frame.width, 100.0);
    assert_eq!(frame.height, 100.0);
    let vb = frame.view_box.expect("viewBox preserved");
    assert_eq!(vb.width, 100.0);

    assert_eq!(frame.root.children.len(), 2);
    for child in &frame.root.children {
        match child {
            Node::Path(_) => {}
            other => panic!("expected Path, got {other:?}"),
        }
    }
}

#[test]
fn empty_svg_parses_to_empty_root() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#;
    let frame = parse_svg(src).unwrap();
    assert!(frame.root.children.is_empty());
}

#[test]
fn skips_comments_and_processing_instructions() {
    let src = b"<?xml version=\"1.0\"?>\n<!-- a comment -->\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"5\" height=\"5\"><!-- inside -->\n</svg>\n";
    let frame = parse_svg(src).unwrap();
    assert_eq!(frame.width, 5.0);
}
