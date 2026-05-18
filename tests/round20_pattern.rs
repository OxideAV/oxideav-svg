//! Round 20 — `<pattern>` paint server capture + SVG 2 §13.2 paint-list
//! fallback resolution.
//!
//! Verifies:
//! 1. `<pattern>` definitions are captured into
//!    [`PreservedExtras::patterns`] and re-emitted on round-trip.
//! 2. The typed [`PatternDef`] reflects the spec's default
//!    `patternUnits=objectBoundingBox` / `patternContentUnits=
//!    userSpaceOnUse` per SVG 2 §14.3.1.
//! 3. `<pattern>`'s `x` / `y` / `width` / `height` /
//!    `patternTransform` / `viewBox` / `preserveAspectRatio` / `href`
//!    survive the typed parse.
//! 4. SVG 2 §13.2 paint-list (`fill="url(#pat) red"`) renders as the
//!    fallback colour today (`oxideav_core::Paint` has no `Pattern`
//!    variant yet) — and a bare `url(#pat)` with no fallback yields no
//!    paint, matching the pre-round-20 behaviour for unknown ids.
//! 5. `url(#unknown) blue` falls back to the colour even when the id
//!    doesn't resolve.

use oxideav_core::Paint;
use oxideav_svg::{defs::PatternUnits, parse_svg, parse_svg_with_extras, write_svg_with_extras};

fn first_path(frame: &oxideav_core::VectorFrame) -> &oxideav_core::PathNode {
    fn find(n: &oxideav_core::Node) -> Option<&oxideav_core::PathNode> {
        match n {
            oxideav_core::Node::Path(p) => Some(p),
            oxideav_core::Node::Group(g) => g.children.iter().find_map(find),
            oxideav_core::Node::SoftMask { content, .. } => find(content),
            _ => None,
        }
    }
    frame
        .root
        .children
        .iter()
        .find_map(find)
        .expect("expected at least one PathNode in the scene graph")
}

#[test]
fn pattern_paint_with_colour_fallback_renders_as_fallback() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <defs>
    <pattern id="p1" patternUnits="userSpaceOnUse" x="0" y="0" width="10" height="10">
      <rect x="0" y="0" width="5" height="5" fill="black"/>
    </pattern>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="url(#p1) red"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame);
    match &path.fill {
        Some(Paint::Solid(c)) => {
            assert_eq!((c.r, c.g, c.b, c.a), (255, 0, 0, 255));
        }
        other => panic!(
            "expected Paint::Solid (fallback) for un-rendered <pattern>, got {:?}",
            other
        ),
    }
}

#[test]
fn pattern_paint_without_fallback_yields_no_paint() {
    // Round 20 — bare `url(#p1)` with no fallback to a known pattern
    // renders nothing today (no Paint::Pattern variant in core yet).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <defs>
    <pattern id="p1" patternUnits="userSpaceOnUse" x="0" y="0" width="10" height="10">
      <rect x="0" y="0" width="5" height="5" fill="black"/>
    </pattern>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="url(#p1)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame);
    assert!(
        path.fill.is_none(),
        "expected no fill for un-rendered pattern with no fallback, got {:?}",
        path.fill
    );
}

#[test]
fn unknown_url_with_fallback_resolves_to_fallback_colour() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <rect x="0" y="0" width="10" height="10" fill="url(#nope) #00ff00"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame);
    match &path.fill {
        Some(Paint::Solid(c)) => assert_eq!((c.r, c.g, c.b, c.a), (0, 255, 0, 255)),
        other => panic!("expected fallback green, got {:?}", other),
    }
}

#[test]
fn unknown_url_with_explicit_none_fallback_yields_no_paint() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <rect x="0" y="0" width="10" height="10" fill="url(#nope) none"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame);
    assert!(path.fill.is_none());
}

#[test]
fn typed_pattern_def_records_default_units() {
    // Default `patternUnits` is `objectBoundingBox` and default
    // `patternContentUnits` is `userSpaceOnUse` per SVG 2 §14.3.1.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg">
  <defs>
    <pattern id="p1" width="0.2" height="0.2">
      <rect width="0.1" height="0.1" fill="red"/>
    </pattern>
  </defs>
</svg>"##;
    let (_frame, _extras) = parse_svg_with_extras(src).unwrap();
    // Drop down to the typed view via a re-parse using the lower-level
    // ParseContext. Easier path: re-build defs via the public parser
    // entry — but parse_svg_with_extras already populated the defs
    // inside its internal ctx and dropped it. Test the typed view by
    // parsing through the element module directly.
    use oxideav_svg::element::{parse_pattern_def, ParseContext};
    use oxideav_svg::parser::parse_xml;
    let xml = std::str::from_utf8(src).unwrap();
    let nodes = parse_xml(xml).unwrap();
    let svg = match &nodes[0] {
        oxideav_svg::parser::Node::Element(e) => e,
        _ => unreachable!(),
    };
    let defs_el = svg
        .children
        .iter()
        .find_map(|c| match c {
            oxideav_svg::parser::Node::Element(e)
                if oxideav_svg::parser::tag_local(&e.name) == "defs" =>
            {
                Some(e)
            }
            _ => None,
        })
        .unwrap();
    let pattern_el = defs_el
        .children
        .iter()
        .find_map(|c| match c {
            oxideav_svg::parser::Node::Element(e)
                if oxideav_svg::parser::tag_local(&e.name) == "pattern" =>
            {
                Some(e)
            }
            _ => None,
        })
        .unwrap();
    let mut ctx = ParseContext::new();
    let (id, def) = parse_pattern_def(pattern_el, &mut ctx).unwrap().unwrap();
    assert_eq!(id, "p1");
    assert_eq!(def.pattern_units, PatternUnits::ObjectBoundingBox);
    assert_eq!(def.pattern_content_units, PatternUnits::UserSpaceOnUse);
    assert!((def.width - 0.2).abs() < 1e-6);
    assert!((def.height - 0.2).abs() < 1e-6);
    assert!(def.pattern_transform.is_identity());
    assert!(def.href.is_empty());
    assert_eq!(def.content.children.len(), 1);
}

#[test]
fn pattern_units_user_space_on_use_recorded() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg">
  <defs>
    <pattern id="p1"
             patternUnits="userSpaceOnUse"
             patternContentUnits="objectBoundingBox"
             x="5" y="10" width="20" height="30"
             patternTransform="translate(1, 2)"
             viewBox="0 0 10 10"
             preserveAspectRatio="xMidYMid slice"
             href="#tpl">
      <rect width="10" height="10" fill="blue"/>
    </pattern>
  </defs>
</svg>"##;
    use oxideav_svg::element::{parse_pattern_def, ParseContext};
    use oxideav_svg::parser::{parse_xml, tag_local, Node as XmlNode};
    let nodes = parse_xml(std::str::from_utf8(src).unwrap()).unwrap();
    let svg = match &nodes[0] {
        XmlNode::Element(e) => e,
        _ => unreachable!(),
    };
    let defs_el = svg
        .children
        .iter()
        .find_map(|c| match c {
            XmlNode::Element(e) if tag_local(&e.name) == "defs" => Some(e),
            _ => None,
        })
        .unwrap();
    let pattern_el = defs_el
        .children
        .iter()
        .find_map(|c| match c {
            XmlNode::Element(e) if tag_local(&e.name) == "pattern" => Some(e),
            _ => None,
        })
        .unwrap();
    let mut ctx = ParseContext::new();
    let (_id, def) = parse_pattern_def(pattern_el, &mut ctx).unwrap().unwrap();
    assert_eq!(def.pattern_units, PatternUnits::UserSpaceOnUse);
    assert_eq!(def.pattern_content_units, PatternUnits::ObjectBoundingBox);
    assert_eq!(def.x, 5.0);
    assert_eq!(def.y, 10.0);
    assert_eq!(def.width, 20.0);
    assert_eq!(def.height, 30.0);
    // patternTransform="translate(1, 2)" → e=1, f=2.
    assert_eq!(def.pattern_transform.e, 1.0);
    assert_eq!(def.pattern_transform.f, 2.0);
    let vb = def.view_box.unwrap();
    assert_eq!(
        (vb.min_x, vb.min_y, vb.width, vb.height),
        (0.0, 0.0, 10.0, 10.0)
    );
    assert_eq!(def.href, "tpl");
}

#[test]
fn pattern_round_trips_through_preserved_extras() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" viewBox="0 0 40 40">
  <defs>
    <pattern id="dots" patternUnits="userSpaceOnUse" width="10" height="10">
      <circle cx="5" cy="5" r="2" fill="black"/>
    </pattern>
  </defs>
  <rect x="0" y="0" width="40" height="40" fill="url(#dots) gray"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.patterns.len(),
        1,
        "<pattern> not captured into PreservedExtras"
    );
    let out = write_svg_with_extras(&frame, &extras);
    let out_str = std::str::from_utf8(&out).unwrap();
    assert!(
        out_str.contains("<pattern"),
        "pattern definition missing from re-emitted SVG; got:\n{}",
        out_str
    );
    assert!(
        out_str.contains("id=\"dots\""),
        "pattern id missing from re-emitted SVG"
    );

    // Re-parse and confirm the typed view is still populated and the
    // rect still picks up its fallback colour.
    let (frame2, extras2) = parse_svg_with_extras(&out).unwrap();
    assert_eq!(extras2.patterns.len(), 1);
    let path = first_path(&frame2);
    match &path.fill {
        Some(Paint::Solid(c)) => assert_eq!((c.r, c.g, c.b, c.a), (128, 128, 128, 255)),
        other => panic!("expected gray fallback, got {:?}", other),
    }
}

#[test]
fn pattern_alone_with_no_fallback_does_not_break_document() {
    // No pattern definition AND no fallback colour — the rect should
    // still parse, just with no fill. The whole document must still
    // load.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <rect x="0" y="0" width="10" height="10" fill="url(#missing)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(frame.root.children.len(), 1);
    let path = first_path(&frame);
    assert!(path.fill.is_none());
}

#[test]
fn pattern_with_xlink_href_template_reference() {
    // Legacy SVG 1.1 `xlink:href` template reference per SVG 2 §14.3.1
    // (deprecated, still accepted).
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg"
                     xmlns:xlink="http://www.w3.org/1999/xlink">
  <defs>
    <pattern id="p1" xlink:href="#tpl"/>
  </defs>
</svg>"##;
    use oxideav_svg::element::{parse_pattern_def, ParseContext};
    use oxideav_svg::parser::{parse_xml, tag_local, Node as XmlNode};
    let nodes = parse_xml(std::str::from_utf8(src).unwrap()).unwrap();
    let svg = match &nodes[0] {
        XmlNode::Element(e) => e,
        _ => unreachable!(),
    };
    let defs_el = svg
        .children
        .iter()
        .find_map(|c| match c {
            XmlNode::Element(e) if tag_local(&e.name) == "defs" => Some(e),
            _ => None,
        })
        .unwrap();
    let pattern_el = defs_el
        .children
        .iter()
        .find_map(|c| match c {
            XmlNode::Element(e) if tag_local(&e.name) == "pattern" => Some(e),
            _ => None,
        })
        .unwrap();
    let mut ctx = ParseContext::new();
    let (_id, def) = parse_pattern_def(pattern_el, &mut ctx).unwrap().unwrap();
    assert_eq!(def.href, "tpl");
}
