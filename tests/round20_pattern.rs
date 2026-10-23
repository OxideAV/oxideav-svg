//! Round 20 — `<pattern>` paint server capture + SVG 2 §13.2 paint-list
//! fallback resolution. Round 480 — the pattern is modelled and tiled
//! (see `round480_pattern.rs` for the §14.3 geometry); this file keeps
//! the capture / typed-view / fallback contract.
//!
//! Verifies:
//! 1. `<pattern>` definitions are captured into
//!    [`PreservedExtras::patterns`] and re-emitted on round-trip.
//! 2. The typed [`PatternDef`] keeps "specified vs not" per attribute;
//!    the resolved view applies the spec defaults
//!    `patternUnits=objectBoundingBox` / `patternContentUnits=
//!    userSpaceOnUse` per SVG 2 §14.3.1.
//! 3. `<pattern>`'s `x` / `y` / `width` / `height` /
//!    `patternTransform` / `viewBox` / `preserveAspectRatio` / `href`
//!    survive the typed parse.
//! 4. A resolvable `<pattern>` paints — the shape becomes a soft-masked
//!    tile expansion — whether or not a paint-list fallback is given
//!    (SVG 2 §13.2: the fallback applies only when the reference cannot
//!    be resolved).
//! 5. `url(#unknown) blue` falls back to the colour when the id doesn't
//!    resolve; `url(#unknown) none` / a bare `url(#unknown)` paint
//!    nothing.

use oxideav_svg::length::Length;
use oxideav_svg::{Node, Paint};
use oxideav_svg::{
    defs::{DefsTables, PatternUnits, resolve_pattern_chain},
    parse, parse_with_extras, write_with_extras,
};

fn first_path(frame: &oxideav_svg::SvgDocument) -> &oxideav_svg::PathNode {
    fn find(n: &oxideav_svg::Node) -> Option<&oxideav_svg::PathNode> {
        match n {
            oxideav_svg::Node::Path(p) => Some(p),
            oxideav_svg::Node::Group(g) => g.children.iter().find_map(find),
            oxideav_svg::Node::SoftMask { content, .. } => find(content),
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

/// The shape's first child is the pattern layer group holding a
/// `SoftMask` whose content is the tile group.
fn pattern_tiles(frame: &oxideav_svg::SvgDocument) -> usize {
    match &frame.root.children[0] {
        Node::Group(layers) => match &layers.children[0] {
            Node::SoftMask { content, .. } => match &**content {
                Node::Group(tiles) => tiles.children.len(),
                other => panic!("expected tile group, got {other:?}"),
            },
            other => panic!("expected SoftMask layer, got {other:?}"),
        },
        other => panic!("expected layer group, got {other:?}"),
    }
}

#[test]
fn pattern_paint_with_colour_fallback_still_paints_the_pattern() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <defs>
    <pattern id="p1" patternUnits="userSpaceOnUse" x="0" y="0" width="10" height="10">
      <rect x="0" y="0" width="5" height="5" fill="black"/>
    </pattern>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="url(#p1) red"/>
</svg>"##;
    let frame = parse(src).unwrap();
    // 10 × 10 tiles cover the 100 × 100 rect.
    assert_eq!(pattern_tiles(&frame), 100);
}

#[test]
fn pattern_paint_without_fallback_paints_the_pattern() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <defs>
    <pattern id="p1" patternUnits="userSpaceOnUse" x="0" y="0" width="10" height="10">
      <rect x="0" y="0" width="5" height="5" fill="black"/>
    </pattern>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="url(#p1)"/>
</svg>"##;
    let frame = parse(src).unwrap();
    assert_eq!(pattern_tiles(&frame), 100);
}

#[test]
fn unknown_url_with_fallback_resolves_to_fallback_colour() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <rect x="0" y="0" width="10" height="10" fill="url(#nope) #00ff00"/>
</svg>"##;
    let frame = parse(src).unwrap();
    let path = first_path(&frame);
    match &path.fill {
        Some(Paint::Solid(c)) => assert_eq!((c.r, c.g, c.b, c.a), (0, 255, 0, 255)),
        other => panic!("expected fallback green, got {other:?}"),
    }
}

#[test]
fn unknown_url_with_explicit_none_fallback_yields_no_paint() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <rect x="0" y="0" width="10" height="10" fill="url(#nope) none"/>
</svg>"##;
    let frame = parse(src).unwrap();
    let path = first_path(&frame);
    assert!(path.fill.is_none());
}

fn pattern_element(src: &[u8]) -> oxideav_svg::parser::Element {
    use oxideav_svg::parser::{Node as XmlNode, parse_xml, tag_local};
    let nodes = parse_xml(std::str::from_utf8(src).unwrap()).unwrap();
    let svg = match &nodes[0] {
        XmlNode::Element(e) => e.clone(),
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
    defs_el
        .children
        .iter()
        .find_map(|c| match c {
            XmlNode::Element(e) if tag_local(&e.name) == "pattern" => Some(e.clone()),
            _ => None,
        })
        .unwrap()
}

#[test]
fn typed_pattern_def_records_default_units() {
    // Default `patternUnits` is `objectBoundingBox` and default
    // `patternContentUnits` is `userSpaceOnUse` per SVG 2 §14.3.1 —
    // unspecified on the def, applied by the chain resolver.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg">
  <defs>
    <pattern id="p1" width="0.2" height="0.2">
      <rect width="0.1" height="0.1" fill="red"/>
    </pattern>
  </defs>
</svg>"##;
    use oxideav_svg::element::{ParseContext, parse_pattern_def};
    let pattern_el = pattern_element(src);
    let mut ctx = ParseContext::new();
    let (id, def) = parse_pattern_def(&pattern_el, &mut ctx).unwrap().unwrap();
    assert_eq!(id, "p1");
    assert_eq!(def.pattern_units, None);
    assert_eq!(def.pattern_content_units, None);
    assert_eq!(def.width, Some(Length::user_units(0.2)));
    assert_eq!(def.height, Some(Length::user_units(0.2)));
    assert_eq!(def.x, None);
    assert!(def.pattern_transform.is_none());
    assert!(def.href.is_empty());
    assert_eq!(def.content.children.len(), 1);
    let resolved = resolve_pattern_chain(&def, &DefsTables::new());
    assert_eq!(resolved.pattern_units, PatternUnits::ObjectBoundingBox);
    assert_eq!(resolved.pattern_content_units, PatternUnits::UserSpaceOnUse);
    assert!((resolved.width - 0.2).abs() < 1e-6);
    assert_eq!(resolved.x, 0.0);
    assert!(resolved.pattern_transform.is_identity());
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
    use oxideav_svg::element::{ParseContext, parse_pattern_def};
    let pattern_el = pattern_element(src);
    let mut ctx = ParseContext::new();
    let (_id, def) = parse_pattern_def(&pattern_el, &mut ctx).unwrap().unwrap();
    assert_eq!(def.pattern_units, Some(PatternUnits::UserSpaceOnUse));
    assert_eq!(
        def.pattern_content_units,
        Some(PatternUnits::ObjectBoundingBox)
    );
    assert_eq!(def.x, Some(Length::user_units(5.0)));
    assert_eq!(def.y, Some(Length::user_units(10.0)));
    assert_eq!(def.width, Some(Length::user_units(20.0)));
    assert_eq!(def.height, Some(Length::user_units(30.0)));
    // patternTransform="translate(1, 2)" → e=1, f=2.
    let t = def.pattern_transform.unwrap();
    assert_eq!(t.e, 1.0);
    assert_eq!(t.f, 2.0);
    let vb = def.view_box.unwrap();
    assert_eq!(
        (vb.min_x, vb.min_y, vb.width, vb.height),
        (0.0, 0.0, 10.0, 10.0)
    );
    assert!(def.preserve_aspect_ratio.is_some());
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
    let (frame, extras) = parse_with_extras(src).unwrap();
    assert_eq!(
        extras.patterns.len(),
        1,
        "<pattern> not captured into PreservedExtras"
    );
    let out = write_with_extras(&frame, &extras);
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
    // The tiled expansion collapses back to the source reference
    // (fallback included) on the native shape — no `<mask>` / tile
    // groups in the output.
    assert!(
        out_str.contains(r##"<rect x="0" y="0" width="40" height="40" fill="url(#dots) #808080""##),
        "{out_str}"
    );
    assert!(!out_str.contains("<mask"), "{out_str}");
    assert_eq!(out_str.matches("<circle").count(), 1, "{out_str}");

    // Re-parse and confirm the typed view is still populated and the
    // rect still tiles the pattern (16 tiles on 40 × 40).
    let (frame2, extras2) = parse_with_extras(&out).unwrap();
    assert_eq!(extras2.patterns.len(), 1);
    assert_eq!(pattern_tiles(&frame2), 16);
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
    let frame = parse(src).unwrap();
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
    use oxideav_svg::element::{ParseContext, parse_pattern_def};
    let pattern_el = pattern_element(src);
    let mut ctx = ParseContext::new();
    let (_id, def) = parse_pattern_def(&pattern_el, &mut ctx).unwrap().unwrap();
    assert_eq!(def.href, "tpl");
}
