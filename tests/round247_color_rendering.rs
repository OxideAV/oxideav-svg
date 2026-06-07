//! Round 247 — SVG 2 §13.10.1 `color-rendering` integration tests.
//!
//! `color-rendering: auto | optimizeSpeed | optimizeQuality` is a *hint*
//! to the user agent about quality vs speed tradeoffs when performing
//! colour interpolation and compositing operations — it never alters
//! the numeric source colours themselves. Per §13.10.1:
//!
//! * Initial value `auto` — UA's own balance, with a quality bias.
//! * Applies to container / graphics / gradient elements, `<use>` and
//!   `<animate>` per the §13.10.1 attribute table.
//! * Inherited — a `<g color-rendering=…>` propagates the property down
//!   the normal cascade.
//! * Per the §13.10.1 informative note, `color-rendering` takes
//!   precedence over the filter-effects `color-interpolation-filters`
//!   property; that interaction lives in `oxideav-filter` / the filter
//!   primitive graph and is documented but not exercised here.
//!
//! Round 247 ships parse + inherited cascade + round-trip preservation.
//! The actual rendering-hint consumption (working colour-space
//! selection for interpolation and compositing) lives in
//! `oxideav-raster`; this crate exposes the resolved value on
//! `PaintState::color_rendering` and round-trips the source attribute
//! via `PreservedExtras::color_renderings`.

use oxideav_svg::element::{ColorRendering, PaintState};
use oxideav_svg::parser::{parse_xml, Element, Node as XmlNode};
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Default `color-rendering` is `auto` (per §13.10.1 Initial table).
#[test]
fn default_color_rendering_is_auto() {
    let s = PaintState::default();
    assert_eq!(s.color_rendering, ColorRendering::Auto);
}

/// Round 247: a document without `color-rendering=` parses cleanly and
/// the decoder does NOT pollute the side-channel.
#[test]
fn baseline_no_color_rendering_attr_no_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.color_renderings.is_empty(),
        "round 247: a document without color-rendering= must not record a binding"
    );
}

/// Round 247: `color-rendering="optimizeQuality"` on a `<g>` records a
/// binding with the canonicalised camelCase keyword.
#[test]
fn optimize_quality_on_g_records_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-rendering="optimizeQuality">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.color_renderings.len(),
        1,
        "round 247: color-rendering= must record exactly one binding"
    );
    assert_eq!(
        extras.color_renderings[0].color_rendering,
        "optimizeQuality"
    );
}

/// Round 247: each of the three §13.10.1 keywords parses + records.
#[test]
fn each_keyword_records_canonical_form() {
    for (input, expected) in [
        ("auto", "auto"),
        ("optimizeSpeed", "optimizeSpeed"),
        ("optimizeQuality", "optimizeQuality"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <g color-rendering="{}">
                    <rect x="10" y="10" width="50" height="50" fill="red"/>
                </g>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.color_renderings.len(), 1, "input={}", input);
        assert_eq!(
            extras.color_renderings[0].color_rendering, expected,
            "input={}",
            input
        );
    }
}

/// Round 247: source keywords are matched case-insensitively per CSS
/// rules. The binding canonicalises to the spec's camelCase spelling.
#[test]
fn keyword_matching_is_case_insensitive() {
    for (input, expected) in [
        ("AUTO", "auto"),
        ("OPTIMIZESPEED", "optimizeSpeed"),
        ("OptimizeQuality", "optimizeQuality"),
        ("optimizequality", "optimizeQuality"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <g color-rendering="{}">
                    <rect x="10" y="10" width="50" height="50" fill="red"/>
                </g>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.color_renderings.len(), 1, "input={}", input);
        assert_eq!(
            extras.color_renderings[0].color_rendering, expected,
            "input={}",
            input
        );
    }
}

/// Round 247: explicit author `color-rendering="auto"` IS recorded.
/// Mirrors the round-221 `shape-rendering` / round-228 `text-rendering`
/// / round-235 `image-rendering` policy — `auto` carries author intent
/// (e.g. an inheritance override on a descendant of a
/// `<g color-rendering="optimizeQuality">`) so the round-trip preserves
/// it.
#[test]
fn explicit_auto_is_recorded() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-rendering="auto">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.color_renderings.len(),
        1,
        "round 247: explicit `auto` carries author intent and IS recorded"
    );
    assert_eq!(extras.color_renderings[0].color_rendering, "auto");
}

/// Round 247: `color-rendering="inherit"` keeps the cascade's inherited
/// value and skips recording (no canonical token to emit).
#[test]
fn inherit_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-rendering="inherit">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.color_renderings.is_empty(),
        "round 247: `inherit` keeps the inherited value and skips recording"
    );
}

/// Round 247: unrecognised tokens fall back to the cascade's inherited
/// value and skip recording (matches the tolerant policy of round-118
/// visibility / round-172 text-anchor / round-205 paint-order / round-221
/// shape-rendering / round-228 text-rendering / round-235 image-rendering
/// branches).
#[test]
fn unknown_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-rendering="someFutureKeyword">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.color_renderings.is_empty(),
        "round 247: unrecognised keyword keeps inherited value and skips recording"
    );
    // The document still loads (no parse failure).
    let _ = parse_svg(src).unwrap();
}

/// Round 247: empty `color-rendering=""` skips recording (no keyword to
/// canonicalise).
#[test]
fn empty_value_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-rendering="">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.color_renderings.is_empty());
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

/// Round 247: a presentation attribute resolves into PaintState
/// (presentation-attribute lane of the cascade).
#[test]
fn presentation_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect color-rendering="optimizeSpeed" x="0" y="0" width="10" height="10" fill="red"/>
            </svg>"#,
        "rect",
    );
    assert_eq!(s.color_rendering, ColorRendering::OptimizeSpeed);
}

/// Round 247: an inline `style="…"` declaration resolves into
/// PaintState (style-attribute lane wins over presentation attribute
/// per the round-4 cascade order).
#[test]
fn style_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect style="color-rendering: optimizeQuality" x="0" y="0" width="10" height="10" fill="red"/>
            </svg>"#,
        "rect",
    );
    assert_eq!(s.color_rendering, ColorRendering::OptimizeQuality);
}

/// Round 247: the property IS inherited per §13.10.1 — a child of a
/// `<g color-rendering=…>` picks up the resolved value via the cascade.
/// Verify via PaintState merge.
#[test]
fn property_is_inherited_through_cascade() {
    // Build a parent state with optimizeQuality.
    let parent = PaintState {
        color_rendering: ColorRendering::OptimizeQuality,
        ..PaintState::default()
    };
    // Child element without its own color-rendering should inherit.
    let nodes = parse_xml(
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="10" height="10"/></svg>"#,
    )
    .expect("xml parse");
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
        child.color_rendering,
        ColorRendering::OptimizeQuality,
        "round 247: color-rendering IS inherited per §13.10.1"
    );
}

/// Round 247: a child can override an inherited value back to a
/// different keyword via its own `color-rendering=`.
#[test]
fn child_can_override_inherited_value() {
    let parent = PaintState {
        color_rendering: ColorRendering::OptimizeQuality,
        ..PaintState::default()
    };
    let nodes = parse_xml(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect color-rendering="optimizeSpeed" x="0" y="0" width="10" height="10"/>
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
    assert_eq!(child.color_rendering, ColorRendering::OptimizeSpeed);
}

/// Round 247: round-trip preserves `color-rendering=` on a `<g>` — a
/// `parse_svg_with_extras → write_svg_with_extras` cycle re-emits the
/// attribute on the matching element.
#[test]
fn roundtrip_emits_attribute_on_group() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-rendering="optimizeSpeed">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("color-rendering=\"optimizeSpeed\""),
        "round-trip output should re-emit color-rendering: {}",
        out_s
    );
}

/// Round 247: round-trip preserves `color-rendering=` on a shape — a
/// shape carrying the attribute directly (not via a group) also
/// round-trips on the matching `<rect>` / `<path>` emit slot.
#[test]
fn roundtrip_emits_attribute_on_shape() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red" color-rendering="optimizeQuality"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.color_renderings.len(), 1);
    assert_eq!(
        extras.color_renderings[0].color_rendering,
        "optimizeQuality"
    );
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("color-rendering=\"optimizeQuality\""),
        "round-trip should re-emit color-rendering on the shape: {}",
        out_s
    );
}

/// Round 247: double round-trip converges — a second parse-then-write
/// of the output produces identical `color-rendering=` content.
#[test]
fn roundtrip_is_idempotent() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-rendering="optimizeQuality">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame1, extras1) = parse_svg_with_extras(src).unwrap();
    let out1 = write_svg_with_extras(&frame1, &extras1);
    let (frame2, extras2) = parse_svg_with_extras(&out1).unwrap();
    let out2 = write_svg_with_extras(&frame2, &extras2);
    assert_eq!(
        out1, out2,
        "round 247: parse → write → parse → write must converge"
    );
    let s2 = String::from_utf8(out2).unwrap();
    assert!(s2.contains("color-rendering=\"optimizeQuality\""));
}

/// Round 247: source-case canonicalises through round-trip — uppercase
/// input emits as camelCase per the §13.10.1 attribute table.
#[test]
fn roundtrip_canonicalises_source_case() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-rendering="OPTIMIZESPEED">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("color-rendering=\"optimizeSpeed\""),
        "uppercase source should round-trip as canonical camelCase: {}",
        out_s
    );
}

/// Round 247: `parse_svg` (no extras) still loads the document cleanly
/// — the property cascade machinery doesn't require the side-channel.
#[test]
fn parse_svg_without_extras_still_loads() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-rendering="optimizeQuality">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let _ = parse_svg(src).unwrap();
}

/// Round 247: a `<g color-rendering=…>` ancestor records the attribute
/// on the group's own emit site (the cascade still pushes the resolved
/// value down via inheritance, but the side-channel keeps a single
/// lexical record on the source-faithful slot). This avoids the
/// redundant per-child binding that would otherwise bloat a large
/// group's round-trip.
#[test]
fn group_attribute_records_once_not_per_child() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-rendering="optimizeQuality">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
            <rect x="30" y="30" width="50" height="50" fill="blue"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.color_renderings.len(),
        1,
        "round 247: a `<g>`-level attribute records once, not per child"
    );
    assert_eq!(
        extras.color_renderings[0].color_rendering,
        "optimizeQuality"
    );
}

/// Round 247: `color-rendering` coexists with `shape-rendering` /
/// `text-rendering` / `image-rendering` as independent inherited
/// properties on the same `<g>`; all round-trip via their own
/// side-channels (round-221 / round-228 / round-235 / round-247).
#[test]
fn coexists_with_other_rendering_hints_on_same_group() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-rendering="optimizeQuality" shape-rendering="geometricPrecision"
           text-rendering="optimizeLegibility">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.color_renderings.len(), 1);
    assert_eq!(extras.shape_renderings.len(), 1);
    assert_eq!(extras.text_renderings.len(), 1);
    let out = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("color-rendering=\"optimizeQuality\""));
    assert!(s.contains("shape-rendering=\"geometricPrecision\""));
    assert!(s.contains("text-rendering=\"optimizeLegibility\""));
}

/// Round 247: per-child override of an inherited group value is
/// captured on the child's own emit slot in addition to the group's.
/// A `<g color-rendering="optimizeSpeed">` whose `<g>` child overrides
/// to `optimizeQuality` produces two distinct bindings on the
/// side-channel, preserving both source attributes for a faithful
/// round-trip.
#[test]
fn per_child_override_records_separately() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-rendering="optimizeSpeed">
            <g color-rendering="optimizeQuality">
                <rect x="10" y="10" width="50" height="50" fill="red"/>
            </g>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.color_renderings.len(), 2);
    let kinds: Vec<&str> = extras
        .color_renderings
        .iter()
        .map(|b| b.color_rendering.as_str())
        .collect();
    assert!(kinds.contains(&"optimizeSpeed"));
    assert!(kinds.contains(&"optimizeQuality"));
}
