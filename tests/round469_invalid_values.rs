//! Round 469 — SVG 2 §4.2 error handling for property values.
//!
//! A presentation attribute whose value fails to parse "is assumed to
//! have been specified as the given initial value"; a CSS declaration
//! with an invalid value is ignored (CSS 2.1 §4.1.8). Neither is a
//! document error. Found by the `write_roundtrip` fuzz target: an
//! `<image fill>` captured verbatim from inside an unknown element was
//! re-emitted as `fill=""` at the top level, which the parser then
//! rejected with "SVG paint: empty".

use oxideav_svg::{parse, parse_with_extras, write_with_extras, Node, Paint, Rgba};

fn first_path(node: &Node) -> Option<&oxideav_svg::PathNode> {
    match node {
        Node::Path(p) => Some(p),
        Node::Group(g) => g.children.iter().find_map(first_path),
        _ => None,
    }
}

fn first_path_fill(src: &[u8]) -> Option<Paint> {
    let doc = parse(src).expect("parse");
    first_path(&Node::Group(doc.root.clone()))
        .expect("a path")
        .fill
        .clone()
}

#[test]
fn invalid_presentation_attribute_takes_the_initial_value() {
    // `fill` initial value is black.
    let f = first_path_fill(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><g fill="red"><rect width="5" height="5" fill="not-a-colour"/></g></svg>"##,
    );
    assert!(
        matches!(f, Some(Paint::Solid(c)) if c == Rgba::opaque(0, 0, 0)),
        "{f:?}"
    );
    // An empty value is invalid too.
    let f = first_path_fill(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><g fill="red"><rect width="5" height="5" fill=""/></g></svg>"##,
    );
    assert!(
        matches!(f, Some(Paint::Solid(c)) if c == Rgba::opaque(0, 0, 0)),
        "{f:?}"
    );
    // `stroke` initial value is none; a bad stroke must not kill the fill.
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="5" height="5" fill="blue" stroke="??" stroke-width="abc"/></svg>"##,
    )
    .unwrap();
    let root = Node::Group(doc.root.clone());
    let p = first_path(&root).expect("a path");
    assert!(p.stroke.is_none());
    assert!(matches!(p.fill, Some(Paint::Solid(c)) if c == Rgba::opaque(0, 0, 255)));
}

#[test]
fn invalid_css_declaration_is_ignored() {
    // The inherited red stands: the invalid declaration is dropped,
    // not replaced by the initial value.
    let f = first_path_fill(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><g fill="red"><rect width="5" height="5" style="fill: bogus"/></g></svg>"##,
    );
    assert!(
        matches!(f, Some(Paint::Solid(c)) if c == Rgba::opaque(255, 0, 0)),
        "{f:?}"
    );
}

#[test]
fn fuzz_counterexample_reparses() {
    let src = b"<svg><tktl><image href=\"a\n\" fill>";
    let (doc, extras) = parse_with_extras(src).expect("lenient parse");
    let out = write_with_extras(&doc, &extras);
    parse_with_extras(&out).expect("writer output must re-parse");
}

/// A numeric literal that overflows `f32`, or is not a number at all,
/// is an invalid attribute value: SVG 2 §4.2 substitutes the initial
/// value, and no infinity reaches the scene graph (the writer could not
/// print one back as a `<number>`; the third fuzz-found counterexample
/// wrote `height="inf"`).
#[test]
fn out_of_range_and_malformed_numbers_take_the_initial_value() {
    let huge = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="{}"/>"##,
        "9".repeat(40)
    );
    let doc = parse(huge.as_bytes()).expect("not a document error");
    assert_eq!(doc.height, 0.0);
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect x="abc" width="1e40" height="2"/></svg>"##,
    )
    .expect("not a document error");
    let out = oxideav_svg::write(&doc);
    let text = String::from_utf8(out.clone()).unwrap();
    assert!(!text.contains("inf"), "{text}");
    parse(&out).expect("writer output must re-parse");
}

/// `transform` is a presentation attribute (SVG 2): an unparsable value
/// is the initial value (identity), not a document error — including a
/// value the SMIL evaluator folded in (fourth fuzz-found counterexample).
#[test]
fn invalid_transform_is_identity() {
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><g transform="0"><rect width="5" height="5" transform="bogus(1)"/></g></svg>"##,
    )
    .expect("invalid transform is not an error");
    match &doc.root.children[0] {
        Node::Group(g) => assert!(g.transform.is_identity()),
        other => panic!("expected group, got {other:?}"),
    }
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><g><set attributeName="transform" to="0"/><rect width="5" height="5"/></g></svg>"##,
    )
    .expect("a folded invalid animation value is not an error");
    assert_eq!(doc.root.children.len(), 1);
}
