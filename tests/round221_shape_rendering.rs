//! Round 221 — SVG 2 §13.10.2 `shape-rendering` integration tests.
//!
//! `shape-rendering: auto | optimizeSpeed | crispEdges |
//! geometricPrecision` is a *hint* to the user agent about quality vs
//! speed vs pixel-snap tradeoffs when rendering vector shapes — it
//! never changes the geometry itself. Per §13.10.2:
//!
//! * Initial value `auto` — UA balances speed / crisp edges / geometric
//!   precision with a precision bias.
//! * Applies to `<shape>` elements (path / rect / circle / ellipse /
//!   line / polyline / polygon).
//! * Inherited — a `<g shape-rendering=…>` propagates the property to
//!   child shapes via the normal cascade.
//!
//! Round 221 ships parse + inherited cascade + round-trip preservation.
//! The actual rendering-hint consumption (anti-alias toggle, edge snap)
//! lives in `oxideav-raster`; this crate exposes the resolved value on
//! `PaintState::shape_rendering` and round-trips the source attribute
//! via `PreservedExtras::shape_renderings`.

use oxideav_svg::element::{PaintState, ShapeRendering};
use oxideav_svg::parser::{parse_xml, Element, Node as XmlNode};
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Default `shape-rendering` is `auto` (per §13.10.2 Initial table).
#[test]
fn default_shape_rendering_is_auto() {
    let s = PaintState::default();
    assert_eq!(s.shape_rendering, ShapeRendering::Auto);
}

/// Round 221: a shape without `shape-rendering=` parses cleanly and
/// the decoder does NOT pollute the side-channel.
#[test]
fn baseline_no_shape_rendering_attr_no_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.shape_renderings.is_empty(),
        "round 221: a shape without shape-rendering= must not record a binding"
    );
}

/// Round 221: `shape-rendering="crispEdges"` on a shape parses cleanly
/// and records a binding with the canonicalised camelCase keyword.
#[test]
fn crisp_edges_on_rect_records_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"
              shape-rendering="crispEdges"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.shape_renderings.len(),
        1,
        "round 221: shape-rendering= must record exactly one binding"
    );
    assert_eq!(extras.shape_renderings[0].shape_rendering, "crispEdges");
}

/// Round 221: each of the four §13.10.2 keywords parses + records.
#[test]
fn each_keyword_records_canonical_form() {
    for (input, expected) in [
        ("auto", "auto"),
        ("optimizeSpeed", "optimizeSpeed"),
        ("crispEdges", "crispEdges"),
        ("geometricPrecision", "geometricPrecision"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <rect x="10" y="10" width="50" height="50" fill="red"
                      shape-rendering="{}"/>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.shape_renderings.len(), 1, "input={}", input);
        assert_eq!(
            extras.shape_renderings[0].shape_rendering, expected,
            "input={}",
            input
        );
    }
}

/// Round 221: source keywords are matched case-insensitively per CSS
/// rules. The binding canonicalises to the spec's camelCase spelling.
#[test]
fn keyword_matching_is_case_insensitive() {
    for (input, expected) in [
        ("AUTO", "auto"),
        ("OPTIMIZESPEED", "optimizeSpeed"),
        ("CrispEdges", "crispEdges"),
        ("GEOMETRICPRECISION", "geometricPrecision"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <rect x="10" y="10" width="50" height="50" fill="red"
                      shape-rendering="{}"/>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.shape_renderings.len(), 1, "input={}", input);
        assert_eq!(
            extras.shape_renderings[0].shape_rendering, expected,
            "input={}",
            input
        );
    }
}

/// Round 221: explicit author `shape-rendering="auto"` IS recorded.
/// Unlike paint-order / vector-effect (which skip the initial value to
/// avoid no-op binding bloat), `auto` carries author intent — e.g. an
/// inheritance override on a descendant of a `<g shape-rendering=
/// "optimizeSpeed">` — so the round-trip preserves it.
#[test]
fn explicit_auto_is_recorded() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"
              shape-rendering="auto"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.shape_renderings.len(),
        1,
        "round 221: explicit `auto` carries author intent and IS recorded"
    );
    assert_eq!(extras.shape_renderings[0].shape_rendering, "auto");
}

/// Round 221: `shape-rendering="inherit"` keeps the cascade's
/// inherited value and skips recording (no canonical token to emit).
#[test]
fn inherit_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"
              shape-rendering="inherit"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.shape_renderings.is_empty(),
        "round 221: `inherit` keeps the inherited value and skips recording"
    );
}

/// Round 221: unrecognised tokens fall back to the cascade's
/// inherited value and skip recording (matches the tolerant policy of
/// the round-118 visibility / round-172 text-anchor / round-205
/// paint-order branches).
#[test]
fn unknown_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"
              shape-rendering="someFutureKeyword"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.shape_renderings.is_empty(),
        "round 221: unrecognised keyword keeps inherited value and skips recording"
    );
    // The document still loads (no parse failure).
    let _ = parse_svg(src).unwrap();
}

/// Round 221: empty `shape-rendering=""` skips recording (no keyword
/// to canonicalise).
#[test]
fn empty_value_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"
              shape-rendering=""/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.shape_renderings.is_empty());
}

/// Round 221: the cascade resolves `shape-rendering` on a shape via a
/// presentation attribute. Build a minimal Element through the public
/// XML parser and exercise [`PaintState::merged_with_mctx`] directly.
fn shape_state(svg: &str, tag: &str) -> PaintState {
    let nodes = parse_xml(svg).expect("xml parse");
    let svg_el: &Element = nodes
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name.ends_with("svg") => Some(e),
            _ => None,
        })
        .expect("svg root");
    let shape_el: &Element = svg_el
        .children
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name == tag => Some(e),
            _ => None,
        })
        .expect("shape child");
    let parent = PaintState::default();
    let sheet = oxideav_svg::css::Stylesheet::new();
    parent
        .merged_with_css(shape_el, &sheet)
        .expect("merged paint state")
}

/// Round 221: a presentation attribute resolves into PaintState
/// (presentation-attribute lane of the cascade).
#[test]
fn presentation_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect shape-rendering="geometricPrecision"/>
            </svg>"#,
        "rect",
    );
    assert_eq!(s.shape_rendering, ShapeRendering::GeometricPrecision);
}

/// Round 221: an inline `style="…"` declaration resolves into
/// PaintState (style-attribute lane wins over presentation attribute
/// per the round-4 cascade order).
#[test]
fn style_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect style="shape-rendering: optimizeSpeed"/>
            </svg>"#,
        "rect",
    );
    assert_eq!(s.shape_rendering, ShapeRendering::OptimizeSpeed);
}

/// Round 221: the property IS inherited per §13.10.2 — a child shape
/// of a `<g shape-rendering=…>` picks up the resolved value via the
/// cascade. Verify via PaintState merge.
#[test]
fn property_is_inherited_through_cascade() {
    // Build a parent state with crispEdges.
    let parent = PaintState {
        shape_rendering: ShapeRendering::CrispEdges,
        ..PaintState::default()
    };
    // Child element without its own shape-rendering should inherit.
    let nodes =
        parse_xml(r#"<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>"#).expect("xml parse");
    let svg_el: &Element = nodes
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name.ends_with("svg") => Some(e),
            _ => None,
        })
        .expect("svg root");
    let rect_el: &Element = svg_el
        .children
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name == "rect" => Some(e),
            _ => None,
        })
        .expect("rect");
    let sheet = oxideav_svg::css::Stylesheet::new();
    let child = parent.merged_with_css(rect_el, &sheet).unwrap();
    assert_eq!(
        child.shape_rendering,
        ShapeRendering::CrispEdges,
        "round 221: shape-rendering IS inherited per §13.10.2"
    );
}

/// Round 221: round-trip preserves `shape-rendering=` on a `<rect>` —
/// `parse_svg_with_extras → write_svg_with_extras` re-emits the
/// attribute on the matching element.
#[test]
fn roundtrip_emits_attribute_on_rect() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"
              shape-rendering="optimizeSpeed"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("shape-rendering=\"optimizeSpeed\""),
        "round-trip output should re-emit shape-rendering: {}",
        out_s
    );
}

/// Round 221: double round-trip converges — a second
/// parse-then-write of the output produces identical
/// `shape-rendering=` attribute content.
#[test]
fn roundtrip_is_idempotent() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <path d="M 10 10 L 90 90" stroke="black" stroke-width="2"
              shape-rendering="geometricPrecision"/>
    </svg>"#;
    let (frame1, extras1) = parse_svg_with_extras(src).unwrap();
    let out1 = write_svg_with_extras(&frame1, &extras1);
    let (frame2, extras2) = parse_svg_with_extras(&out1).unwrap();
    let out2 = write_svg_with_extras(&frame2, &extras2);
    assert_eq!(
        out1, out2,
        "round 221: parse → write → parse → write must converge"
    );
    // And the canonical attribute is still there.
    let s2 = String::from_utf8(out2).unwrap();
    assert!(s2.contains("shape-rendering=\"geometricPrecision\""));
}

/// Round 221: `parse_svg` (no extras) still loads the document
/// cleanly — the property cascade machinery doesn't require the
/// side-channel.
#[test]
fn parse_svg_without_extras_still_loads() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"
              shape-rendering="crispEdges"/>
    </svg>"#;
    let _ = parse_svg(src).unwrap();
}

/// Round 221: a `<g shape-rendering=…>` ancestor records the
/// attribute on the group's emit site (the cascade still pushes the
/// resolved value down via inheritance, but the side-channel keeps a
/// single lexical record on the source-faithful slot). This avoids
/// the redundant per-child binding that would otherwise bloat a
/// large group's round-trip.
#[test]
fn group_attribute_records_on_group_slot() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g shape-rendering="crispEdges">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
            <rect x="30" y="30" width="50" height="50" fill="blue"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    // Exactly one binding — on the `<g>` itself (the children inherit
    // via cascade but don't write their own bindings).
    assert_eq!(
        extras.shape_renderings.len(),
        1,
        "round 221: a `<g>`-level attribute records once, not per child"
    );
    assert_eq!(extras.shape_renderings[0].shape_rendering, "crispEdges");
}

/// Round 221: `<g shape-rendering=…>` round-trips on the matching
/// `<g>` emit site (not on each child shape).
#[test]
fn group_attribute_roundtrips_on_group_element() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g shape-rendering="optimizeSpeed">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(out).unwrap();
    // The attribute lives on `<g`, not on the inner `<rect`.
    assert!(
        s.contains("<g shape-rendering=\"optimizeSpeed\"")
            || s.contains("<g ") && s.contains("shape-rendering=\"optimizeSpeed\""),
        "round-trip should emit the attribute on `<g>`: {}",
        s
    );
}

/// Round 221: per-child override of an inherited group value is
/// captured on the child's own emit slot in addition to the group's.
/// A `<g shape-rendering="optimizeSpeed">` whose `<rect>` overrides to
/// `crispEdges` produces two distinct bindings on the side-channel,
/// preserving both source attributes for a faithful round-trip.
#[test]
fn per_child_override_records_separately() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g shape-rendering="optimizeSpeed">
            <rect x="10" y="10" width="50" height="50" fill="red"
                  shape-rendering="crispEdges"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.shape_renderings.len(), 2);
    let kinds: Vec<&str> = extras
        .shape_renderings
        .iter()
        .map(|b| b.shape_rendering.as_str())
        .collect();
    assert!(kinds.contains(&"optimizeSpeed"));
    assert!(kinds.contains(&"crispEdges"));
}
