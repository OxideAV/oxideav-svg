//! Round 228 — SVG 2 §13.10.3 `text-rendering` integration tests.
//!
//! `text-rendering: auto | optimizeSpeed | optimizeLegibility |
//! geometricPrecision` is a *hint* to the user agent about quality vs
//! speed vs legibility tradeoffs when rasterising text glyphs — it
//! never changes the glyph outlines themselves. Per §13.10.3:
//!
//! * Initial value `auto` — UA balances speed / legibility / precision
//!   with a legibility bias.
//! * Applies to `<text>` (and through the inheritance cascade to
//!   descendant `<tspan>` / `<textPath>` runs).
//! * Inherited — a `<g text-rendering=…>` propagates the property down
//!   the normal cascade.
//!
//! Round 228 ships parse + inherited cascade + round-trip preservation.
//! The actual rendering-hint consumption (anti-alias toggle, hint
//! suspension) lives in `oxideav-raster` / `oxideav-scribe`; this crate
//! exposes the resolved value on `PaintState::text_rendering` and
//! round-trips the source attribute via `PreservedExtras::text_renderings`.

use oxideav_svg::element::{PaintState, TextRendering};
use oxideav_svg::parser::{parse_xml, Element, Node as XmlNode};
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Default `text-rendering` is `auto` (per §13.10.3 Initial table).
#[test]
fn default_text_rendering_is_auto() {
    let s = PaintState::default();
    assert_eq!(s.text_rendering, TextRendering::Auto);
}

/// Round 228: a document without `text-rendering=` parses cleanly and
/// the decoder does NOT pollute the side-channel.
#[test]
fn baseline_no_text_rendering_attr_no_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.text_renderings.is_empty(),
        "round 228: a document without text-rendering= must not record a binding"
    );
}

/// Round 228: `text-rendering="optimizeLegibility"` on a `<text>`
/// records a binding with the canonicalised camelCase keyword. The
/// `<text>` element needs a registered font resolver to emit glyph
/// children — we use a `<g>` wrapper for tests that exercise the
/// shape-side cascade without dragging in `oxideav-scribe`.
#[test]
fn optimize_legibility_on_g_records_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g text-rendering="optimizeLegibility">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.text_renderings.len(),
        1,
        "round 228: text-rendering= must record exactly one binding"
    );
    assert_eq!(
        extras.text_renderings[0].text_rendering,
        "optimizeLegibility"
    );
}

/// Round 228: each of the four §13.10.3 keywords parses + records.
#[test]
fn each_keyword_records_canonical_form() {
    for (input, expected) in [
        ("auto", "auto"),
        ("optimizeSpeed", "optimizeSpeed"),
        ("optimizeLegibility", "optimizeLegibility"),
        ("geometricPrecision", "geometricPrecision"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <g text-rendering="{}">
                    <rect x="10" y="10" width="50" height="50" fill="red"/>
                </g>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.text_renderings.len(), 1, "input={}", input);
        assert_eq!(
            extras.text_renderings[0].text_rendering, expected,
            "input={}",
            input
        );
    }
}

/// Round 228: source keywords are matched case-insensitively per CSS
/// rules. The binding canonicalises to the spec's camelCase spelling.
#[test]
fn keyword_matching_is_case_insensitive() {
    for (input, expected) in [
        ("AUTO", "auto"),
        ("OPTIMIZESPEED", "optimizeSpeed"),
        ("OptimizeLegibility", "optimizeLegibility"),
        ("GEOMETRICPRECISION", "geometricPrecision"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <g text-rendering="{}">
                    <rect x="10" y="10" width="50" height="50" fill="red"/>
                </g>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.text_renderings.len(), 1, "input={}", input);
        assert_eq!(
            extras.text_renderings[0].text_rendering, expected,
            "input={}",
            input
        );
    }
}

/// Round 228: explicit author `text-rendering="auto"` IS recorded.
/// Mirrors the round-221 `shape-rendering` policy — `auto` carries
/// author intent (e.g. an inheritance override on a descendant of a
/// `<g text-rendering="optimizeLegibility">`) so the round-trip
/// preserves it.
#[test]
fn explicit_auto_is_recorded() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g text-rendering="auto">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.text_renderings.len(),
        1,
        "round 228: explicit `auto` carries author intent and IS recorded"
    );
    assert_eq!(extras.text_renderings[0].text_rendering, "auto");
}

/// Round 228: `text-rendering="inherit"` keeps the cascade's inherited
/// value and skips recording (no canonical token to emit).
#[test]
fn inherit_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g text-rendering="inherit">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.text_renderings.is_empty(),
        "round 228: `inherit` keeps the inherited value and skips recording"
    );
}

/// Round 228: unrecognised tokens fall back to the cascade's inherited
/// value and skip recording (matches the tolerant policy of round-118
/// visibility / round-172 text-anchor / round-205 paint-order / round-221
/// shape-rendering branches).
#[test]
fn unknown_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g text-rendering="someFutureKeyword">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.text_renderings.is_empty(),
        "round 228: unrecognised keyword keeps inherited value and skips recording"
    );
    // The document still loads (no parse failure).
    let _ = parse_svg(src).unwrap();
}

/// Round 228: empty `text-rendering=""` skips recording (no keyword to
/// canonicalise).
#[test]
fn empty_value_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g text-rendering="">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.text_renderings.is_empty());
}

/// Helper: extract a PaintState from a parsed XML fragment by finding
/// the named child of the SVG root.
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
        .expect("child element");
    let parent = PaintState::default();
    let sheet = oxideav_svg::css::Stylesheet::new();
    parent
        .merged_with_css(shape_el, &sheet)
        .expect("merged paint state")
}

/// Round 228: a presentation attribute resolves into PaintState
/// (presentation-attribute lane of the cascade).
#[test]
fn presentation_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <text text-rendering="geometricPrecision">hello</text>
            </svg>"#,
        "text",
    );
    assert_eq!(s.text_rendering, TextRendering::GeometricPrecision);
}

/// Round 228: an inline `style="…"` declaration resolves into
/// PaintState (style-attribute lane wins over presentation attribute
/// per the round-4 cascade order).
#[test]
fn style_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <text style="text-rendering: optimizeSpeed">hello</text>
            </svg>"#,
        "text",
    );
    assert_eq!(s.text_rendering, TextRendering::OptimizeSpeed);
}

/// Round 228: the property IS inherited per §13.10.3 — a child of a
/// `<g text-rendering=…>` picks up the resolved value via the cascade.
/// Verify via PaintState merge.
#[test]
fn property_is_inherited_through_cascade() {
    // Build a parent state with optimizeLegibility.
    let parent = PaintState {
        text_rendering: TextRendering::OptimizeLegibility,
        ..PaintState::default()
    };
    // Child element without its own text-rendering should inherit.
    let nodes = parse_xml(r#"<svg xmlns="http://www.w3.org/2000/svg"><text>hello</text></svg>"#)
        .expect("xml parse");
    let svg_el: &Element = nodes
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name.ends_with("svg") => Some(e),
            _ => None,
        })
        .expect("svg root");
    let text_el: &Element = svg_el
        .children
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name == "text" => Some(e),
            _ => None,
        })
        .expect("text");
    let sheet = oxideav_svg::css::Stylesheet::new();
    let child = parent.merged_with_css(text_el, &sheet).unwrap();
    assert_eq!(
        child.text_rendering,
        TextRendering::OptimizeLegibility,
        "round 228: text-rendering IS inherited per §13.10.3"
    );
}

/// Round 228: a child can override an inherited value back to a
/// different keyword via its own `text-rendering=`.
#[test]
fn child_can_override_inherited_value() {
    let parent = PaintState {
        text_rendering: TextRendering::OptimizeLegibility,
        ..PaintState::default()
    };
    let nodes = parse_xml(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <text text-rendering="optimizeSpeed">hi</text>
            </svg>"#,
    )
    .expect("xml parse");
    let svg_el: &Element = nodes
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name.ends_with("svg") => Some(e),
            _ => None,
        })
        .expect("svg root");
    let text_el: &Element = svg_el
        .children
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name == "text" => Some(e),
            _ => None,
        })
        .expect("text");
    let sheet = oxideav_svg::css::Stylesheet::new();
    let child = parent.merged_with_css(text_el, &sheet).unwrap();
    assert_eq!(child.text_rendering, TextRendering::OptimizeSpeed);
}

/// Round 228: round-trip preserves `text-rendering=` on a `<g>` — a
/// `parse_svg_with_extras → write_svg_with_extras` cycle re-emits the
/// attribute on the matching element.
#[test]
fn roundtrip_emits_attribute_on_group() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g text-rendering="optimizeSpeed">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("text-rendering=\"optimizeSpeed\""),
        "round-trip output should re-emit text-rendering: {}",
        out_s
    );
}

/// Round 228: double round-trip converges — a second parse-then-write
/// of the output produces identical `text-rendering=` content.
#[test]
fn roundtrip_is_idempotent() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g text-rendering="geometricPrecision">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame1, extras1) = parse_svg_with_extras(src).unwrap();
    let out1 = write_svg_with_extras(&frame1, &extras1);
    let (frame2, extras2) = parse_svg_with_extras(&out1).unwrap();
    let out2 = write_svg_with_extras(&frame2, &extras2);
    assert_eq!(
        out1, out2,
        "round 228: parse → write → parse → write must converge"
    );
    let s2 = String::from_utf8(out2).unwrap();
    assert!(s2.contains("text-rendering=\"geometricPrecision\""));
}

/// Round 228: `parse_svg` (no extras) still loads the document cleanly
/// — the property cascade machinery doesn't require the side-channel.
#[test]
fn parse_svg_without_extras_still_loads() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g text-rendering="optimizeLegibility">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let _ = parse_svg(src).unwrap();
}

/// Round 228: a `<g text-rendering=…>` ancestor records the attribute
/// on the group's own emit site (the cascade still pushes the resolved
/// value down via inheritance, but the side-channel keeps a single
/// lexical record on the source-faithful slot). This avoids the
/// redundant per-child binding that would otherwise bloat a large
/// group's round-trip.
#[test]
fn group_attribute_records_once_not_per_child() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g text-rendering="optimizeLegibility">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
            <rect x="30" y="30" width="50" height="50" fill="blue"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.text_renderings.len(),
        1,
        "round 228: a `<g>`-level attribute records once, not per child"
    );
    assert_eq!(
        extras.text_renderings[0].text_rendering,
        "optimizeLegibility"
    );
}

/// Round 228: round-trips on the matching `<g>` emit site (not on each
/// child shape).
#[test]
fn group_attribute_roundtrips_on_group_element() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g text-rendering="optimizeSpeed">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(out).unwrap();
    assert!(
        s.contains("<g ") && s.contains("text-rendering=\"optimizeSpeed\""),
        "round-trip should emit the attribute on `<g>`: {}",
        s
    );
}

/// Round 228: `text-rendering` and `shape-rendering` coexist as
/// independent inherited properties on the same `<g>`; both round-trip
/// faithfully via their own side-channels (round-221 + round-228).
#[test]
fn coexists_with_shape_rendering_on_same_group() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g text-rendering="optimizeLegibility" shape-rendering="geometricPrecision">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.text_renderings.len(), 1);
    assert_eq!(extras.shape_renderings.len(), 1);
    let out = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("text-rendering=\"optimizeLegibility\""));
    assert!(s.contains("shape-rendering=\"geometricPrecision\""));
}

/// Round 228: per-child override of an inherited group value is
/// captured on the child's own emit slot in addition to the group's.
/// A `<g text-rendering="optimizeSpeed">` whose `<g>` child overrides
/// to `geometricPrecision` produces two distinct bindings on the
/// side-channel, preserving both source attributes for a faithful
/// round-trip.
#[test]
fn per_child_override_records_separately() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g text-rendering="optimizeSpeed">
            <g text-rendering="geometricPrecision">
                <rect x="10" y="10" width="50" height="50" fill="red"/>
            </g>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.text_renderings.len(), 2);
    let kinds: Vec<&str> = extras
        .text_renderings
        .iter()
        .map(|b| b.text_rendering.as_str())
        .collect();
    assert!(kinds.contains(&"optimizeSpeed"));
    assert!(kinds.contains(&"geometricPrecision"));
}
