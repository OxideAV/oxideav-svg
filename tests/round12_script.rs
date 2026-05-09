//! Round 12 — `<script>` graceful capture.
//!
//! SVG / HTML5 specify that `<script>` element content is raw text
//! ("script data state") — `<` characters in the body must NOT be
//! parsed as markup. Real-world SVGs frequently embed unescaped JS
//! like `if (a < b)` without CDATA wrapping; the round-11 parser
//! choked on these. Round 12 treats the body as raw text, captures
//! the `<script>` element verbatim into [`PreservedExtras`], and
//! re-emits it CDATA-wrapped on round-trip.
//!
//! The decoder NEVER executes script content (oxideav has no JS
//! engine and SVGs round-tripped through this crate are intended for
//! static rendering).

use oxideav_svg::parser::{parse_xml, Node as XmlNode};
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

#[test]
fn parser_treats_script_body_as_raw_text() {
    // `if (a < b)` would normally tank a strict-XML parser by
    // looking like a tag open. Round-12 raw-text mode lets the rest
    // of the document parse cleanly.
    let src = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <script type="text/ecmascript">
            if (a < b && c > 0) { x = "y"; }
        </script>
        <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
    </svg>"##;
    let nodes = parse_xml(src).expect("parse_xml");
    let svg = match &nodes[0] {
        XmlNode::Element(e) => e,
        _ => panic!("expected svg element"),
    };
    // <script> + <rect> survive as siblings of the root.
    let elements: Vec<_> = svg
        .children
        .iter()
        .filter_map(|n| match n {
            XmlNode::Element(e) => Some(e),
            _ => None,
        })
        .collect();
    assert_eq!(
        elements.len(),
        2,
        "<script> body shouldn't have eaten the trailing <rect>"
    );
    assert_eq!(oxideav_svg::parser::tag_local(&elements[0].name), "script");
    assert_eq!(oxideav_svg::parser::tag_local(&elements[1].name), "rect");
    // Body is captured as one Text child.
    let body = match &elements[0].children[0] {
        XmlNode::Text(t) => t.clone(),
        _ => panic!("expected text body"),
    };
    assert!(
        body.contains("if (a < b && c > 0)"),
        "raw script body must keep its `<` and `>`: got {body:?}"
    );
}

#[test]
fn parse_svg_with_unescaped_script_does_not_fail() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <script>function f(){ return 1<2; }</script>
        <rect x="0" y="0" width="10" height="10" fill="#0000ff"/>
    </svg>"##;
    let frame = parse_svg(src).expect("parse_svg with unescaped script");
    // Scripts are dropped from the scene graph (they don't paint), so
    // only the <rect> shows up.
    assert_eq!(frame.root.children.len(), 1);
}

#[test]
fn extras_capture_script_verbatim() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <script type="text/ecmascript"><![CDATA[ var x = 1; ]]></script>
        <rect x="0" y="0" width="10" height="10" fill="#00ff00"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).expect("parse_svg_with_extras");
    assert_eq!(extras.scripts.len(), 1);
    let body = extras.scripts[0]
        .children
        .iter()
        .find_map(|c| match c {
            XmlNode::Text(t) => Some(t.clone()),
            _ => None,
        })
        .expect("script body");
    assert!(body.contains("var x = 1"), "body was {body:?}");
}

#[test]
fn round_trip_preserves_script_with_cdata_wrapping() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <script>if (1<2) { foo(); }</script>
        <rect x="0" y="0" width="10" height="10" fill="#0000ff"/>
    </svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).expect("parse_svg_with_extras");
    let bytes = write_svg_with_extras(&frame, &extras);
    let out = String::from_utf8(bytes).expect("utf-8");
    // Body is CDATA-wrapped on output so the unescaped `<` survives
    // a second parse without raw-text mode being needed.
    assert!(
        out.contains("<script") && out.contains("<![CDATA[") && out.contains("if (1<2)"),
        "expected CDATA-wrapped script in output:\n{out}"
    );
    // Re-parse the output and verify the script is still there.
    let (_frame2, extras2) = parse_svg_with_extras(out.as_bytes()).expect("re-parse");
    assert_eq!(extras2.scripts.len(), 1);
}

#[test]
fn empty_script_emits_self_closing() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <script></script>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(
        out.contains("<script/>") || out.contains("<script />"),
        "empty script should be self-closing, got:\n{out}"
    );
}

#[test]
fn extras_is_empty_when_no_script_present() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <rect x="0" y="0" width="10" height="10" fill="#000"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).expect("parse");
    assert!(extras.scripts.is_empty());
    assert!(extras.is_empty());
}
