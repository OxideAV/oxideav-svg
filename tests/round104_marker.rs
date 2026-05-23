//! Round 104 — `<marker>` definition capture (SVG 2 §13.7.1).
//!
//! `oxideav_core::Node` has no `Marker` construct, so — mirroring the
//! round-20 `<pattern>` capture — round 104 records a typed
//! [`oxideav_svg::defs::MarkerDef`] (consumable by a downstream
//! rasterizer) plus the verbatim source XML on
//! [`oxideav_svg::preserved::PreservedExtras::markers`] for a lossless
//! round-trip. Painting the marker at shape vertices (`orient` rotation,
//! `markerUnits` scaling per §13.7.4) is deferred until a `Marker` node
//! lands in oxideav-core.
//!
//! Verifies:
//! 1. `<marker>` is captured into [`PreservedExtras::markers`] and
//!    re-emitted in a `<defs>` block on round-trip.
//! 2. The typed [`MarkerDef`] reflects the spec defaults
//!    (`markerWidth=markerHeight=3`, `markerUnits=strokeWidth`,
//!    `orient=0`, `refX=refY=0`) per §13.7.1.
//! 3. Every explicit attribute (`refX` / `refY` / `markerWidth` /
//!    `markerHeight` / `markerUnits` / `orient` / `viewBox` /
//!    `preserveAspectRatio`) survives the typed parse.
//! 4. `orient` parses the two keywords plus `<angle>` (with unit
//!    suffixes) and a bare `<number>`.
//! 5. `refX` / `refY` geometric keywords resolve against the `viewBox`.
//! 6. A `<marker>` is a never-rendered element — it contributes no
//!    scene-graph node and does not break a document that references it
//!    via `marker-end`.

use oxideav_svg::defs::{MarkerOrient, MarkerUnits};
use oxideav_svg::element::{parse_marker_def, ParseContext};
use oxideav_svg::parser::{parse_xml, tag_local, Node as XmlNode};
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Pull the first `<marker>` child of the document `<defs>` (or the
/// document root) so a test can feed it to [`parse_marker_def`].
fn first_marker_element(src: &[u8]) -> oxideav_svg::parser::Element {
    let nodes = parse_xml(std::str::from_utf8(src).unwrap()).unwrap();
    let svg = match &nodes[0] {
        XmlNode::Element(e) => e,
        _ => unreachable!("expected <svg> root"),
    };
    // Search root and any <defs> for the first <marker>.
    fn find(el: &oxideav_svg::parser::Element) -> Option<oxideav_svg::parser::Element> {
        for c in &el.children {
            if let XmlNode::Element(e) = c {
                if tag_local(&e.name) == "marker" {
                    return Some(e.clone());
                }
                if let Some(found) = find(e) {
                    return Some(found);
                }
            }
        }
        None
    }
    find(svg).expect("no <marker> element in source")
}

#[test]
fn typed_marker_def_records_spec_defaults() {
    // Per §13.7.1: markerWidth=markerHeight=3, markerUnits=strokeWidth,
    // orient=0, refX=refY=0.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg">
  <defs>
    <marker id="m1">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="black"/>
    </marker>
  </defs>
</svg>"##;
    let el = first_marker_element(src);
    let mut ctx = ParseContext::new();
    let (id, def) = parse_marker_def(&el, &mut ctx).unwrap().unwrap();
    assert_eq!(id, "m1");
    assert_eq!(def.ref_x, 0.0);
    assert_eq!(def.ref_y, 0.0);
    assert_eq!(def.marker_width, 3.0);
    assert_eq!(def.marker_height, 3.0);
    assert_eq!(def.marker_units, MarkerUnits::StrokeWidth);
    assert_eq!(def.orient, MarkerOrient::Angle(0.0));
    assert!(def.view_box.is_none());
    assert_eq!(def.content.children.len(), 1);
}

#[test]
fn typed_marker_def_records_explicit_attributes() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg">
  <defs>
    <marker id="arrow"
            viewBox="0 0 10 10" refX="1" refY="5"
            markerUnits="userSpaceOnUse"
            markerWidth="4" markerHeight="3"
            orient="auto"
            preserveAspectRatio="xMidYMid slice">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="context-stroke"/>
    </marker>
  </defs>
</svg>"##;
    let el = first_marker_element(src);
    let mut ctx = ParseContext::new();
    let (id, def) = parse_marker_def(&el, &mut ctx).unwrap().unwrap();
    assert_eq!(id, "arrow");
    assert_eq!(def.ref_x, 1.0);
    assert_eq!(def.ref_y, 5.0);
    assert_eq!(def.marker_width, 4.0);
    assert_eq!(def.marker_height, 3.0);
    assert_eq!(def.marker_units, MarkerUnits::UserSpaceOnUse);
    assert_eq!(def.orient, MarkerOrient::Auto);
    let vb = def.view_box.unwrap();
    assert_eq!(
        (vb.min_x, vb.min_y, vb.width, vb.height),
        (0.0, 0.0, 10.0, 10.0)
    );
}

#[test]
fn marker_units_parser() {
    assert_eq!(
        MarkerUnits::parse(Some("userSpaceOnUse")),
        MarkerUnits::UserSpaceOnUse
    );
    assert_eq!(
        MarkerUnits::parse(Some("strokeWidth")),
        MarkerUnits::StrokeWidth
    );
    // Unknown / absent fall back to the spec default `strokeWidth`.
    assert_eq!(MarkerUnits::parse(Some("bogus")), MarkerUnits::StrokeWidth);
    assert_eq!(MarkerUnits::parse(None), MarkerUnits::StrokeWidth);
    // Round-trip keyword.
    assert_eq!(MarkerUnits::UserSpaceOnUse.as_str(), "userSpaceOnUse");
    assert_eq!(MarkerUnits::StrokeWidth.as_str(), "strokeWidth");
}

#[test]
fn orient_parser_keywords_and_angles() {
    assert_eq!(MarkerOrient::parse(Some("auto")), MarkerOrient::Auto);
    assert_eq!(
        MarkerOrient::parse(Some("auto-start-reverse")),
        MarkerOrient::AutoStartReverse
    );
    // Bare number is degrees per §13.7.1.
    assert_eq!(MarkerOrient::parse(Some("45")), MarkerOrient::Angle(45.0));
    assert_eq!(
        MarkerOrient::parse(Some("90deg")),
        MarkerOrient::Angle(90.0)
    );
    // 0.5turn = 180deg.
    match MarkerOrient::parse(Some("0.5turn")) {
        MarkerOrient::Angle(d) => assert!((d - 180.0).abs() < 1e-3),
        other => panic!("expected Angle(180), got {other:?}"),
    }
    // PI rad = 180deg.
    match MarkerOrient::parse(Some("3.14159265rad")) {
        MarkerOrient::Angle(d) => assert!((d - 180.0).abs() < 1e-2),
        other => panic!("expected Angle(~180), got {other:?}"),
    }
    // 400grad = 360deg.
    match MarkerOrient::parse(Some("400grad")) {
        MarkerOrient::Angle(d) => assert!((d - 360.0).abs() < 1e-3),
        other => panic!("expected Angle(360), got {other:?}"),
    }
    // Absent / malformed → spec default 0.
    assert_eq!(MarkerOrient::parse(None), MarkerOrient::Angle(0.0));
    assert_eq!(
        MarkerOrient::parse(Some("nonsense")),
        MarkerOrient::Angle(0.0)
    );
}

#[test]
fn orient_round_trip_attr() {
    assert_eq!(MarkerOrient::Auto.to_attr(), "auto");
    assert_eq!(
        MarkerOrient::AutoStartReverse.to_attr(),
        "auto-start-reverse"
    );
    assert_eq!(MarkerOrient::Angle(45.0).to_attr(), "45");
    assert_eq!(MarkerOrient::Angle(0.0).to_attr(), "0");
}

#[test]
fn ref_keywords_resolve_against_viewbox() {
    // refX/refY geometric keywords: left/top → 0%, center → 50%,
    // right/bottom → 100% of viewBox width/height per §13.7.1.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg">
  <defs>
    <marker id="m" viewBox="0 0 20 40" refX="right" refY="center">
      <circle cx="10" cy="20" r="5" fill="green"/>
    </marker>
  </defs>
</svg>"##;
    let el = first_marker_element(src);
    let mut ctx = ParseContext::new();
    let (_id, def) = parse_marker_def(&el, &mut ctx).unwrap().unwrap();
    // right = 100% of width 20 = 20; center = 50% of height 40 = 20.
    assert_eq!(def.ref_x, 20.0);
    assert_eq!(def.ref_y, 20.0);
}

#[test]
fn ref_keyword_without_viewbox_falls_back_to_zero() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg">
  <defs>
    <marker id="m" refX="center" refY="bottom">
      <circle cx="1" cy="1" r="1"/>
    </marker>
  </defs>
</svg>"##;
    let el = first_marker_element(src);
    let mut ctx = ParseContext::new();
    let (_id, def) = parse_marker_def(&el, &mut ctx).unwrap().unwrap();
    assert_eq!(def.ref_x, 0.0);
    assert_eq!(def.ref_y, 0.0);
}

#[test]
fn marker_without_id_is_skipped() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg">
  <defs>
    <marker markerWidth="5" markerHeight="5">
      <circle cx="2" cy="2" r="2"/>
    </marker>
  </defs>
</svg>"##;
    let el = first_marker_element(src);
    let mut ctx = ParseContext::new();
    assert!(
        parse_marker_def(&el, &mut ctx).unwrap().is_none(),
        "a <marker> with no id can't be referenced; should yield None"
    );
}

#[test]
fn marker_is_never_rendered_and_does_not_break_document() {
    // The <marker> contributes no scene-graph node (never-rendered per
    // §13.7.1); the <path> that references it via marker-end must still
    // parse cleanly.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <defs>
    <marker id="arrow" viewBox="0 0 10 10" refX="5" refY="5"
            markerWidth="6" markerHeight="6" orient="auto">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="black"/>
    </marker>
  </defs>
  <path d="M 10,50 L 90,50" stroke="black" fill="none" marker-end="url(#arrow)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    // Exactly one scene-graph child: the <path>. The <defs> + <marker>
    // produce nothing.
    assert_eq!(
        frame.root.children.len(),
        1,
        "expected only the <path> in the scene graph (marker is never-rendered)"
    );
}

#[test]
fn marker_round_trips_through_preserved_extras() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 30">
  <defs>
    <marker id="dot" viewBox="0 0 10 10" refX="5" refY="5"
            markerWidth="8" markerHeight="8" markerUnits="userSpaceOnUse"
            orient="auto-start-reverse">
      <circle cx="5" cy="5" r="5" fill="green"/>
    </marker>
  </defs>
  <path d="M10,15 h80" fill="none" stroke="black" marker-start="url(#dot)"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.markers.len(),
        1,
        "<marker> not captured into PreservedExtras"
    );
    let out = write_svg_with_extras(&frame, &extras);
    let out_str = std::str::from_utf8(&out).unwrap();
    assert!(
        out_str.contains("<marker"),
        "marker definition missing from re-emitted SVG; got:\n{out_str}"
    );
    assert!(
        out_str.contains("id=\"dot\""),
        "marker id missing from re-emitted SVG"
    );
    assert!(
        out_str.contains("orient=\"auto-start-reverse\""),
        "orient attribute lost on round-trip"
    );

    // Re-parse the emitted document and confirm the marker survives a
    // second cycle and the typed parse still recovers the attributes.
    let (_frame2, extras2) = parse_svg_with_extras(&out).unwrap();
    assert_eq!(extras2.markers.len(), 1);
    let el = first_marker_element(&out);
    let mut ctx = ParseContext::new();
    let (id, def) = parse_marker_def(&el, &mut ctx).unwrap().unwrap();
    assert_eq!(id, "dot");
    assert_eq!(def.marker_units, MarkerUnits::UserSpaceOnUse);
    assert_eq!(def.orient, MarkerOrient::AutoStartReverse);
    assert_eq!(def.marker_width, 8.0);
    assert_eq!(def.ref_x, 5.0);
}
