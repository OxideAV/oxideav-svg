//! Round 382 — verbatim round-trip of `<image>` child content.
//!
//! Per the SVG 2 §6 `<image>` content model the element may carry
//! descriptive elements (`<title>` / `<desc>` / `<metadata>`) and
//! animation elements (`<animate>` / `<set>` / `<animateMotion>` /
//! `<animateTransform>` / `<discard>`). The encoder previously emitted
//! `<image .../>` self-closing and dropped these children; round 382
//! captures them onto `SvgImage::children` and re-emits them, opening
//! the tag when element children are present and keeping the
//! self-closing form otherwise.

use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

/// A `<title>` child round-trips inside the `<image>`.
#[test]
fn title_child_round_trips() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50"><title>Company logo</title></image>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(!extras.images[0].children.is_empty());
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(out.contains("<title>"), "{out}");
    assert!(out.contains("Company logo"), "{out}");
    assert!(out.contains("</title>"), "{out}");
    assert!(out.contains("</image>"), "{out}");
    // Not emitted self-closing when it has element children.
    assert!(!out.contains(r#"height="50"/>"#), "{out}");
    // Re-parse: the title survives a full cycle.
    let (_f2, extras2) = parse_svg_with_extras(out.as_bytes()).unwrap();
    assert!(!extras2.images[0].children.is_empty());
}

/// A `<desc>` child round-trips.
#[test]
fn desc_child_round_trips() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50"><desc>A raster logo</desc></image>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(out.contains("<desc>"), "{out}");
    assert!(out.contains("A raster logo"), "{out}");
}

/// An animation child round-trips.
#[test]
fn animation_child_round_trips() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50"><animate attributeName="x" from="0" to="10" dur="1s"/></image>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(out.contains("<animate"), "{out}");
    assert!(out.contains(r#"attributeName="x""#), "{out}");
    assert!(out.contains("</image>"), "{out}");
}

/// A childless `<image>` keeps the self-closing form (no round-trip
/// bloat and no spurious `</image>`).
#[test]
fn childless_image_stays_self_closing() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.images[0].children.is_empty());
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(!out.contains("</image>"), "{out}");
    assert!(out.contains("/>"), "{out}");
}

/// An `<image>` whose only content is whitespace text keeps the
/// self-closing form.
#[test]
fn whitespace_only_child_stays_self_closing() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50">   </image>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(!out.contains("</image>"), "{out}");
}

/// Multiple children round-trip in order and survive two full cycles.
#[test]
fn multiple_children_survive_two_cycles() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50"><title>T</title><desc>D</desc></image>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out1 = write_svg_with_extras(&frame, &extras);
    let (frame2, extras2) = parse_svg_with_extras(&out1).unwrap();
    // Count element children only (pretty-printing inserts whitespace
    // text nodes between them on the intermediate serialisation).
    let el_children = extras2.images[0]
        .children
        .iter()
        .filter(|n| matches!(n, oxideav_svg::parser::Node::Element(_)))
        .count();
    assert_eq!(el_children, 2);
    let out2 = String::from_utf8(write_svg_with_extras(&frame2, &extras2)).unwrap();
    let t = out2.find("<title>");
    let d = out2.find("<desc>");
    assert!(t.is_some() && d.is_some(), "{out2}");
    assert!(t < d, "title must precede desc:\n{out2}");
}
