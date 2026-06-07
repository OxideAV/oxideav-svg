//! Round 252 — SVG 2 §13.9 `color-interpolation` integration tests.
//!
//! `color-interpolation: auto | sRGB | linearRGB` selects the working
//! colour space used for the three operations that mix colours
//! componentwise: gradient stop interpolation, SMIL colour animation,
//! and graphics-element compositing / blending. Per §13.9:
//!
//! * Initial value `sRGB` — gradients / animation lerps default to the
//!   sRGB working space (1.1 backwards-compatibility default). This is
//!   *distinct* from the §13.10.x rendering hints whose initial value
//!   is `auto`.
//! * Applies to container / graphics / gradient elements, `<use>` and
//!   `<animate>` per the §13.9 attribute table.
//! * Inherited — a `<g color-interpolation=…>` propagates the property
//!   down the normal cascade.
//! * Per the §13.9 informative note, the filter-effects sibling
//!   property `color-interpolation-filters` governs the filter primitive
//!   graph instead; that interaction lives in `oxideav-filter` and is
//!   not exercised here.
//!
//! Round 252 ships parse + inherited cascade + round-trip preservation.
//! The actual working-space selection (for gradient lerps, colour
//! animation, and compositing) lives in `oxideav-raster`; this crate
//! exposes the resolved value on `PaintState::color_interpolation` and
//! round-trips the source attribute via
//! `PreservedExtras::color_interpolations`.

use oxideav_svg::element::{ColorInterpolation, PaintState};
use oxideav_svg::parser::{parse_xml, Element, Node as XmlNode};
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Default `color-interpolation` is `sRGB` (per §13.9 Initial table —
/// NOT `auto`, unlike the §13.10.x rendering hints).
#[test]
fn default_color_interpolation_is_srgb() {
    let s = PaintState::default();
    assert_eq!(s.color_interpolation, ColorInterpolation::Srgb);
}

/// Round 252: a document without `color-interpolation=` parses cleanly
/// and the decoder does NOT pollute the side-channel.
#[test]
fn baseline_no_color_interpolation_attr_no_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.color_interpolations.is_empty(),
        "round 252: a document without color-interpolation= must not record a binding"
    );
}

/// Round 252: `color-interpolation="linearRGB"` on a `<g>` records a
/// binding with the canonicalised mixed-case keyword.
#[test]
fn linearrgb_on_g_records_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-interpolation="linearRGB">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.color_interpolations.len(),
        1,
        "round 252: color-interpolation= must record exactly one binding"
    );
    assert_eq!(
        extras.color_interpolations[0].color_interpolation,
        "linearRGB"
    );
}

/// Round 252: each of the three §13.9 keywords parses + records.
#[test]
fn each_keyword_records_canonical_form() {
    for (input, expected) in [
        ("auto", "auto"),
        ("sRGB", "sRGB"),
        ("linearRGB", "linearRGB"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <g color-interpolation="{}">
                    <rect x="10" y="10" width="50" height="50" fill="red"/>
                </g>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.color_interpolations.len(), 1, "input={}", input);
        assert_eq!(
            extras.color_interpolations[0].color_interpolation, expected,
            "input={}",
            input
        );
    }
}

/// Round 252: source keywords are matched case-insensitively per CSS
/// rules. The binding canonicalises to the §13.9 mixed-case spelling.
#[test]
fn keyword_matching_is_case_insensitive() {
    for (input, expected) in [
        ("AUTO", "auto"),
        ("SRGB", "sRGB"),
        ("srgb", "sRGB"),
        ("LinearRGB", "linearRGB"),
        ("LINEARRGB", "linearRGB"),
        ("linearrgb", "linearRGB"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <g color-interpolation="{}">
                    <rect x="10" y="10" width="50" height="50" fill="red"/>
                </g>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.color_interpolations.len(), 1, "input={}", input);
        assert_eq!(
            extras.color_interpolations[0].color_interpolation, expected,
            "input={}",
            input
        );
    }
}

/// Round 252: explicit author `color-interpolation="sRGB"` IS recorded.
/// Mirrors the round-247 `color-rendering` policy — even though `sRGB`
/// is the §13.9 initial value, an explicit author write carries intent
/// (e.g. an inheritance reset on a descendant of a
/// `<g color-interpolation="linearRGB">`) so the round-trip preserves
/// it.
#[test]
fn explicit_initial_is_recorded() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-interpolation="sRGB">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.color_interpolations.len(),
        1,
        "round 252: explicit `sRGB` carries author intent and IS recorded"
    );
    assert_eq!(extras.color_interpolations[0].color_interpolation, "sRGB");
}

/// Round 252: explicit author `color-interpolation="auto"` IS recorded.
/// Same rationale as the `sRGB` case — an inheritance reset on a
/// descendant of a `<g color-interpolation="linearRGB">` is a
/// legitimate authored value.
#[test]
fn explicit_auto_is_recorded() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-interpolation="auto">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.color_interpolations.len(), 1);
    assert_eq!(extras.color_interpolations[0].color_interpolation, "auto");
}

/// Round 252: `color-interpolation="inherit"` keeps the cascade's
/// inherited value and skips recording (no canonical token to emit).
#[test]
fn inherit_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-interpolation="inherit">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.color_interpolations.is_empty(),
        "round 252: `inherit` keeps the inherited value and skips recording"
    );
}

/// Round 252: unrecognised tokens fall back to the cascade's inherited
/// value and skip recording (matches the tolerant policy of the round
/// 247 / 235 / 228 / 221 rendering-hint branches).
#[test]
fn unknown_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-interpolation="bt2020">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.color_interpolations.is_empty(),
        "round 252: unrecognised keyword keeps inherited value and skips recording"
    );
    // The document still loads (no parse failure).
    let _ = parse_svg(src).unwrap();
}

/// Round 252: empty `color-interpolation=""` skips recording (no
/// keyword to canonicalise).
#[test]
fn empty_value_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-interpolation="">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.color_interpolations.is_empty());
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

/// Round 252: a presentation attribute resolves into PaintState
/// (presentation-attribute lane of the cascade).
#[test]
fn presentation_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect color-interpolation="linearRGB" x="0" y="0" width="10" height="10" fill="red"/>
            </svg>"#,
        "rect",
    );
    assert_eq!(s.color_interpolation, ColorInterpolation::LinearRgb);
}

/// Round 252: an inline `style="…"` declaration resolves into
/// PaintState (style-attribute lane wins over presentation attribute
/// per the round-4 cascade order).
#[test]
fn style_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect style="color-interpolation: linearRGB" x="0" y="0" width="10" height="10" fill="red"/>
            </svg>"#,
        "rect",
    );
    assert_eq!(s.color_interpolation, ColorInterpolation::LinearRgb);
}

/// Round 252: the property IS inherited per §13.9 — a child of a
/// `<g color-interpolation=…>` picks up the resolved value via the
/// cascade. Verify via PaintState merge.
#[test]
fn property_is_inherited_through_cascade() {
    // Build a parent state with linearRGB.
    let parent = PaintState {
        color_interpolation: ColorInterpolation::LinearRgb,
        ..PaintState::default()
    };
    // Child element without its own color-interpolation should inherit.
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
        child.color_interpolation,
        ColorInterpolation::LinearRgb,
        "round 252: color-interpolation IS inherited per §13.9"
    );
}

/// Round 252: a child can override an inherited value back to a
/// different keyword via its own `color-interpolation=`.
#[test]
fn child_can_override_inherited_value() {
    let parent = PaintState {
        color_interpolation: ColorInterpolation::LinearRgb,
        ..PaintState::default()
    };
    let nodes = parse_xml(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect color-interpolation="sRGB" x="0" y="0" width="10" height="10"/>
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
    assert_eq!(child.color_interpolation, ColorInterpolation::Srgb);
}

/// Round 252: round-trip preserves `color-interpolation=` on a `<g>`
/// — a `parse_svg_with_extras → write_svg_with_extras` cycle re-emits
/// the attribute on the matching element.
#[test]
fn roundtrip_emits_attribute_on_group() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-interpolation="linearRGB">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("color-interpolation=\"linearRGB\""),
        "round-trip output should re-emit color-interpolation: {}",
        out_s
    );
}

/// Round 252: round-trip preserves `color-interpolation=` on a shape
/// — a shape carrying the attribute directly (not via a group) also
/// round-trips on the matching `<rect>` / `<path>` emit slot.
#[test]
fn roundtrip_emits_attribute_on_shape() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red" color-interpolation="linearRGB"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.color_interpolations.len(), 1);
    assert_eq!(
        extras.color_interpolations[0].color_interpolation,
        "linearRGB"
    );
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("color-interpolation=\"linearRGB\""),
        "round-trip should re-emit color-interpolation on the shape: {}",
        out_s
    );
}

/// Round 252: double round-trip converges — a second parse-then-write
/// of the output produces identical `color-interpolation=` content.
#[test]
fn roundtrip_is_idempotent() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-interpolation="linearRGB">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame1, extras1) = parse_svg_with_extras(src).unwrap();
    let out1 = write_svg_with_extras(&frame1, &extras1);
    let (frame2, extras2) = parse_svg_with_extras(&out1).unwrap();
    let out2 = write_svg_with_extras(&frame2, &extras2);
    assert_eq!(
        out1, out2,
        "round 252: parse → write → parse → write must converge"
    );
    let s2 = String::from_utf8(out2).unwrap();
    assert!(s2.contains("color-interpolation=\"linearRGB\""));
}

/// Round 252: source-case canonicalises through round-trip — uppercase
/// input emits as §13.9 mixed-case per the attribute table.
#[test]
fn roundtrip_canonicalises_source_case() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-interpolation="LINEARRGB">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("color-interpolation=\"linearRGB\""),
        "uppercase source should round-trip as §13.9 canonical mixed case: {}",
        out_s
    );
}

/// Round 252: `parse_svg` (no extras) still loads the document cleanly
/// — the property cascade machinery doesn't require the side-channel.
#[test]
fn parse_svg_without_extras_still_loads() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-interpolation="linearRGB">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let _ = parse_svg(src).unwrap();
}

/// Round 252: a `<g color-interpolation=…>` ancestor records the
/// attribute on the group's own emit site (the cascade still pushes
/// the resolved value down via inheritance, but the side-channel keeps
/// a single lexical record on the source-faithful slot). This avoids
/// the redundant per-child binding that would otherwise bloat a large
/// group's round-trip.
#[test]
fn group_attribute_records_once_not_per_child() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-interpolation="linearRGB">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
            <rect x="30" y="30" width="50" height="50" fill="blue"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.color_interpolations.len(),
        1,
        "round 252: a `<g>`-level attribute records once, not per child"
    );
    assert_eq!(
        extras.color_interpolations[0].color_interpolation,
        "linearRGB"
    );
}

/// Round 252: `color-interpolation` coexists with `color-rendering` /
/// `shape-rendering` / `text-rendering` / `image-rendering` as
/// independent inherited properties on the same `<g>`; all round-trip
/// via their own side-channels. §13.9 (working colour space) and
/// §13.10.1 (quality hint) are orthogonal so the author can write
/// both — verify nothing collides.
#[test]
fn coexists_with_other_painting_hints_on_same_group() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-interpolation="linearRGB" color-rendering="optimizeQuality"
           shape-rendering="geometricPrecision" text-rendering="optimizeLegibility">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.color_interpolations.len(), 1);
    assert_eq!(extras.color_renderings.len(), 1);
    assert_eq!(extras.shape_renderings.len(), 1);
    assert_eq!(extras.text_renderings.len(), 1);
    let out = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("color-interpolation=\"linearRGB\""));
    assert!(s.contains("color-rendering=\"optimizeQuality\""));
    assert!(s.contains("shape-rendering=\"geometricPrecision\""));
    assert!(s.contains("text-rendering=\"optimizeLegibility\""));
}

/// Round 252: per-child override of an inherited group value is
/// captured on the child's own emit slot in addition to the group's.
/// A `<g color-interpolation="linearRGB">` whose `<g>` child overrides
/// to `sRGB` produces two distinct bindings on the side-channel,
/// preserving both source attributes for a faithful round-trip.
#[test]
fn per_child_override_records_separately() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g color-interpolation="linearRGB">
            <g color-interpolation="sRGB">
                <rect x="10" y="10" width="50" height="50" fill="red"/>
            </g>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.color_interpolations.len(), 2);
    let kinds: Vec<&str> = extras
        .color_interpolations
        .iter()
        .map(|b| b.color_interpolation.as_str())
        .collect();
    assert!(kinds.contains(&"linearRGB"));
    assert!(kinds.contains(&"sRGB"));
}
