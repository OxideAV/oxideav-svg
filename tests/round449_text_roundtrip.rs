//! Round 449 — `<text>` verbatim round-trip fidelity (SVG 2 §11.2).
//!
//! The decoder flattens `<text>` into resolver-shaped glyph outline
//! paths (or an empty group when no font resolver is installed), which
//! loses the source character data, the font selection properties, the
//! `<tspan>` per-character positioning arrays (§11.2.2 `x` / `y` /
//! `dx` / `dy` / `rotate`), and any `<textPath>` layout (§11.8) —
//! none of which `oxideav_core::Node` can model. Before this round a
//! `parse → write` cycle dropped the text entirely.
//!
//! This round captures the whole `<text>` verbatim in
//! `PreservedExtras::texts` (keyed by scene-graph tree-path, like the
//! round-372 `<switch>` carrier) and replaces the flattened node with
//! the source markup on write. Character data is serialised through a
//! mixed-content-preserving inline writer so inter-span whitespace
//! survives byte-exactly (§11.1 content model; `xml:space="default"`
//! collapsing makes synthetic indentation around spans lossy, so none
//! is inserted).

use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

fn roundtrip(src: &[u8]) -> String {
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    let out = write_svg_with_extras(&frame, &extras);
    String::from_utf8(out).expect("utf8")
}

/// A plain `<text>` element survives a parse → write cycle with its
/// attributes and character data intact.
#[test]
fn simple_text_round_trips() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <text x="10" y="50" font-family="serif" font-size="20" fill="blue">Hello</text>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains(
            r#"<text x="10" y="50" font-family="serif" font-size="20" fill="blue">Hello</text>"#
        ),
        "the <text> element survives verbatim:\n{out}"
    );
}

/// §11.2.2 `<tspan>` per-character positioning arrays and the
/// inter-span character data (including significant spaces) survive
/// byte-exactly — no synthetic indentation, no trimming.
#[test]
fn tspan_positioning_arrays_and_spacing_round_trip() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <text x="10" y="50">Hello <tspan dx="2 4 6" dy="-3" rotate="10 20">World</tspan>!</text>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains(r#">Hello <tspan dx="2 4 6" dy="-3" rotate="10 20">World</tspan>!</text>"#),
        "tspan arrays + exact inter-span spacing survive:\n{out}"
    );
}

/// §11.8 `<textPath>` layout survives, and the `<defs>`-housed target
/// path is re-emitted so the reference still resolves after re-parse.
#[test]
fn text_path_round_trips_with_target() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <defs><path id="curve" d="M0 80 C 40 10, 65 10, 95 80"/></defs>
  <text x="5" y="80"><textPath href="#curve" startOffset="25%">bend</textPath></text>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains(r##"<textPath href="#curve" startOffset="25%">bend</textPath>"##),
        "the textPath survives verbatim:\n{out}"
    );
    assert!(
        out.contains(r##"id="curve""##),
        "the referenced path def survives so the href resolves:\n{out}"
    );
}

/// A `<text>` carrying styling / conditional attributes keeps them on
/// the verbatim element (the round-291 / round-228 side-channel
/// carriers are superseded by the verbatim emission at this slot).
#[test]
fn text_styling_attributes_round_trip() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <text x="1" y="2" dominant-baseline="hanging" text-rendering="optimizeLegibility" opacity="0.5">hi</text>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains(r#"dominant-baseline="hanging""#),
        "dominant-baseline survives:\n{out}"
    );
    assert!(
        out.contains(r#"text-rendering="optimizeLegibility""#),
        "text-rendering survives:\n{out}"
    );
    assert!(out.contains(r#"opacity="0.5""#), "opacity survives:\n{out}");
}

/// An id-bearing `<text>` with an `<animate>` child re-emits the
/// animation exactly once — inside the verbatim element, not
/// duplicated at the trailing edge.
#[test]
fn text_with_animation_child_emits_once() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <text id="t1" x="10" y="50">tick<animate attributeName="x" from="10" to="90" dur="2s"/></text>
</svg>"##;
    let out = roundtrip(src);
    let n = out.matches("<animate").count();
    assert_eq!(n, 1, "the <animate> child appears exactly once:\n{out}");
    assert!(
        out.contains(r#"tick<animate attributeName="x" from="10" to="90" dur="2s"/></text>"#),
        "the animation rides inside the verbatim <text>:\n{out}"
    );
}

/// The output re-parses, and a second write is byte-identical (the
/// write side reaches a fixed point after one cycle).
#[test]
fn text_round_trip_is_idempotent() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <rect x="0" y="0" width="10" height="10" fill="red"/>
  <text x="10" y="50" font-size="14">a <tspan dx="1">b</tspan> c</text>
  <circle cx="50" cy="50" r="5" fill="green"/>
</svg>"##;
    let w1 = roundtrip(src);
    let w2 = roundtrip(w1.as_bytes());
    assert_eq!(w1, w2, "write(parse(write(x))) == write(x)");
    assert!(w2.contains("a <tspan dx=\"1\">b</tspan> c"));
}

/// Multiple `<text>` elements each round-trip at their own document
/// position, interleaved with sibling shapes.
#[test]
fn multiple_texts_keep_document_order() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <text x="1" y="10">first</text>
  <rect x="0" y="0" width="10" height="10"/>
  <text x="1" y="30">second</text>
</svg>"##;
    let out = roundtrip(src);
    let p_first = out.find(">first</text>").expect("first text present");
    let p_rect = out.find("<path").expect("rect (as path) present");
    let p_second = out.find(">second</text>").expect("second text present");
    assert!(
        p_first < p_rect && p_rect < p_second,
        "document order first < rect < second:\n{out}"
    );
}

/// A `<text>` nested inside groups round-trips at its nested position
/// (the tree-path keying is depth-aware).
#[test]
fn nested_text_round_trips_in_place() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <g transform="translate(5 5)"><g><text x="1" y="10">deep</text></g></g>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains(">deep</text>"),
        "the nested text survives:\n{out}"
    );
    // It must sit inside the nested groups, before their close tags.
    let p_text = out.find(">deep</text>").unwrap();
    let p_close = out.find("</g>").expect("inner group close");
    assert!(
        p_text < p_close,
        "the text is emitted inside the group nest:\n{out}"
    );
}

/// Character data containing XML-special characters re-escapes
/// correctly on write (and survives a second parse).
#[test]
fn text_special_characters_reescape() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <text x="1" y="10">a &lt; b &amp; c</text>
</svg>"##;
    let w1 = roundtrip(src);
    assert!(
        w1.contains("a &lt; b &amp; c</text>"),
        "special characters re-escape:\n{w1}"
    );
    let w2 = roundtrip(w1.as_bytes());
    assert_eq!(w1, w2, "escaping is stable across cycles");
}

/// No-`<text>` documents record no bindings — the channel is inert for
/// text-free content.
#[test]
fn no_text_no_binding_guard() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <rect x="0" y="0" width="10" height="10"/>
</svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).expect("parse");
    assert!(extras.texts.is_empty(), "no <text> → no binding");
}
