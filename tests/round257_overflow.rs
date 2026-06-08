//! Round 257 — SVG 2 §3.11 `overflow` property integration tests.
//!
//! `overflow: visible | hidden | scroll | auto` selects whether the
//! UA establishes a clipping rectangle for an element's content. Per
//! §3.11 the property reuses the CSS 2.1 keyword set unchanged
//! (`visible` is the initial value per the §3.11 summary table, the
//! UA stylesheet additionally overrides the initial value to
//! `hidden` for non-root `<svg>` / `<symbol>` / `<marker>` /
//! `<pattern>` / `<image>`). The property is NOT inherited per
//! CSS 2.1 §11.1.1, mirroring the round-118 `display` and round-209
//! `vector-effect` non-inheritance resets.
//!
//! Round 257 ships parse + per-element reset cascade +
//! round-trip preservation. The actual clipping-rectangle
//! establishment + UA-stylesheet initial-value override live in
//! `oxideav-raster`; this crate exposes the resolved value on
//! `PaintState::overflow` and round-trips the source attribute via
//! `PreservedExtras::overflows`.

use oxideav_svg::element::{Overflow, PaintState};
use oxideav_svg::parser::{parse_xml, Element, Node as XmlNode};
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Default `overflow` is `visible` per the §3.11 summary table.
#[test]
fn default_overflow_is_visible() {
    let s = PaintState::default();
    assert_eq!(s.overflow, Overflow::Visible);
}

/// Round 257: a document without `overflow=` parses cleanly and the
/// decoder does NOT pollute the side-channel.
#[test]
fn baseline_no_overflow_attr_no_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.overflows.is_empty(),
        "round 257: a document without overflow= must not record a binding"
    );
}

/// Round 257: `overflow="hidden"` on a `<g>` records a binding with
/// the canonicalised lowercase keyword.
#[test]
fn hidden_on_g_records_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g overflow="hidden">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.overflows.len(),
        1,
        "round 257: overflow= must record exactly one binding"
    );
    assert_eq!(extras.overflows[0].overflow, "hidden");
}

/// Round 257: each of the four §3.11 keywords parses + records.
#[test]
fn each_keyword_records_canonical_form() {
    for (input, expected) in [
        ("visible", "visible"),
        ("hidden", "hidden"),
        ("scroll", "scroll"),
        ("auto", "auto"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <g overflow="{}">
                    <rect x="10" y="10" width="50" height="50" fill="red"/>
                </g>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.overflows.len(), 1, "input={}", input);
        assert_eq!(extras.overflows[0].overflow, expected, "input={}", input);
    }
}

/// Round 257: source keywords are matched case-insensitively per CSS
/// rules. The binding canonicalises to the §3.11 lowercase spelling.
#[test]
fn keyword_matching_is_case_insensitive() {
    for (input, expected) in [
        ("VISIBLE", "visible"),
        ("Visible", "visible"),
        ("HIDDEN", "hidden"),
        ("Hidden", "hidden"),
        ("SCROLL", "scroll"),
        ("Scroll", "scroll"),
        ("AUTO", "auto"),
        ("Auto", "auto"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <g overflow="{}">
                    <rect x="10" y="10" width="50" height="50" fill="red"/>
                </g>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.overflows.len(), 1, "input={}", input);
        assert_eq!(extras.overflows[0].overflow, expected, "input={}", input);
    }
}

/// Round 257: explicit author `overflow="visible"` IS recorded even
/// though `visible` is the §3.11 initial value. Mirrors the round-221
/// / round-247 / round-252 "explicit-initial-value carries intent"
/// policy (e.g. an override of the UA-stylesheet `overflow: hidden`
/// default that fires for non-root `<svg>` / `<symbol>` / `<marker>`
/// / `<pattern>` / `<image>`).
#[test]
fn explicit_visible_is_recorded() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <symbol id="s" overflow="visible">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </symbol>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    // <symbol> doesn't currently emit a scene node by default but
    // the cascade flow runs through every element via the <g> path
    // for the side-channel — the binding for an emit-less element
    // may or may not be recorded depending on the scene-graph path.
    // We instead use a <g> wrapper to exercise the explicit-initial-
    // value carrier deterministically.
    let _ = extras;
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g overflow="visible">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.overflows.len(),
        1,
        "round 257: explicit `visible` carries author intent and IS recorded"
    );
    assert_eq!(extras.overflows[0].overflow, "visible");
}

/// Round 257: `overflow="inherit"` keeps the cascade's value
/// (which, for a non-inherited property, is the initial-value reset)
/// and skips recording (no canonical token to emit).
#[test]
fn inherit_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g overflow="inherit">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.overflows.is_empty(),
        "round 257: `inherit` keeps the resolved value and skips recording"
    );
}

/// Round 257: unrecognised tokens fall back to the cascade's
/// inherited (here, initial-reset) value and skip recording (matches
/// the tolerant policy of the §13.x rendering-hint branches). Per
/// the §3.11 normative note: "as overflow="invalid" will result in a
/// rule setting overflow to visible".
#[test]
fn unknown_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g overflow="clip">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.overflows.is_empty(),
        "round 257: unrecognised keyword keeps the resolved value and skips recording"
    );
    // The document still loads (no parse failure).
    let _ = parse_svg(src).unwrap();
}

/// Round 257: empty `overflow=""` skips recording (no keyword to
/// canonicalise).
#[test]
fn empty_value_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g overflow="">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.overflows.is_empty());
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

/// Round 257: a presentation attribute resolves into PaintState
/// (presentation-attribute lane of the cascade).
#[test]
fn presentation_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect overflow="hidden" x="0" y="0" width="10" height="10" fill="red"/>
            </svg>"#,
        "rect",
    );
    assert_eq!(s.overflow, Overflow::Hidden);
}

/// Round 257: an inline `style="…"` declaration resolves into
/// PaintState (style-attribute lane wins over presentation attribute
/// per the round-4 cascade order).
#[test]
fn style_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect style="overflow: scroll" x="0" y="0" width="10" height="10" fill="red"/>
            </svg>"#,
        "rect",
    );
    assert_eq!(s.overflow, Overflow::Scroll);
}

/// Round 257: the property is NOT inherited per CSS 2.1 §11.1.1.
/// A child of a `<g overflow="hidden">` does NOT pick up `hidden`
/// from the parent — `merged_with_css` resets to the initial value
/// before applying the element's own attribute (matching the
/// round-118 `display` / round-209 `vector-effect` reset policy).
#[test]
fn property_is_not_inherited_through_cascade() {
    // Build a parent state with hidden.
    let parent = PaintState {
        overflow: Overflow::Hidden,
        ..PaintState::default()
    };
    // Child element without its own overflow attribute should reset
    // to the initial value, NOT pick up the parent's hidden.
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
        child.overflow,
        Overflow::Visible,
        "round 257: overflow is NOT inherited per CSS 2.1 §11.1.1 — the per-element reset must \
         restore the initial value"
    );
}

/// Round 257: a child with its own `overflow=` still resolves
/// correctly after the parent's value is reset.
#[test]
fn child_attribute_wins_after_reset() {
    let parent = PaintState {
        overflow: Overflow::Hidden,
        ..PaintState::default()
    };
    let nodes = parse_xml(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect overflow="scroll" x="0" y="0" width="10" height="10"/>
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
    assert_eq!(child.overflow, Overflow::Scroll);
}

/// Round 257: round-trip preserves `overflow=` on a `<g>` — a
/// `parse_svg_with_extras → write_svg_with_extras` cycle re-emits the
/// attribute on the matching element.
#[test]
fn roundtrip_emits_attribute_on_group() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g overflow="hidden">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("overflow=\"hidden\""),
        "round-trip output should re-emit overflow: {}",
        out_s
    );
}

/// Round 257: round-trip preserves `overflow=` on a shape — a shape
/// carrying the attribute directly (not via a group) also round-trips
/// on the matching `<rect>` / `<path>` emit slot.
#[test]
fn roundtrip_emits_attribute_on_shape() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red" overflow="scroll"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.overflows.len(), 1);
    assert_eq!(extras.overflows[0].overflow, "scroll");
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("overflow=\"scroll\""),
        "round-trip should re-emit overflow on the shape: {}",
        out_s
    );
}

/// Round 257: double round-trip converges — a second parse-then-write
/// of the output produces identical `overflow=` content.
#[test]
fn roundtrip_is_idempotent() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g overflow="hidden">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame1, extras1) = parse_svg_with_extras(src).unwrap();
    let out1 = write_svg_with_extras(&frame1, &extras1);
    let (frame2, extras2) = parse_svg_with_extras(&out1).unwrap();
    let out2 = write_svg_with_extras(&frame2, &extras2);
    assert_eq!(
        out1, out2,
        "round 257: parse → write → parse → write must converge"
    );
    let s2 = String::from_utf8(out2).unwrap();
    assert!(s2.contains("overflow=\"hidden\""));
}

/// Round 257: source-case canonicalises through round-trip —
/// uppercase input emits as §3.11 lowercase per the CSS 2.1 keyword
/// set.
#[test]
fn roundtrip_canonicalises_source_case() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g overflow="HIDDEN">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("overflow=\"hidden\""),
        "uppercase source should round-trip as §3.11 canonical lowercase: {}",
        out_s
    );
}

/// Round 257: `parse_svg` (no extras) still loads the document
/// cleanly — the property cascade machinery doesn't require the
/// side-channel.
#[test]
fn parse_svg_without_extras_still_loads() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g overflow="hidden">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let _ = parse_svg(src).unwrap();
}

/// Round 257: a `<g overflow=…>` ancestor records the attribute on
/// the group's own emit site (not redundantly per child). Mirrors
/// the round-252 group-records-once pattern.
#[test]
fn group_attribute_records_once_not_per_child() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g overflow="hidden">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
            <rect x="30" y="30" width="50" height="50" fill="blue"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.overflows.len(),
        1,
        "round 257: a `<g>`-level attribute records once, not per child"
    );
    assert_eq!(extras.overflows[0].overflow, "hidden");
}

/// Round 257: `overflow` coexists with the §13.9 / §13.10.x
/// rendering hints as independent inherited (or non-inherited)
/// properties on the same `<g>`; all round-trip via their own
/// side-channels. §3.11 (clipping rectangle) and §13.10.1 (quality
/// hint) are orthogonal so the author can write any combination —
/// verify nothing collides.
#[test]
fn coexists_with_other_painting_hints_on_same_group() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g overflow="hidden" color-interpolation="linearRGB" color-rendering="optimizeQuality"
           shape-rendering="geometricPrecision" text-rendering="optimizeLegibility">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.overflows.len(), 1);
    assert_eq!(extras.color_interpolations.len(), 1);
    assert_eq!(extras.color_renderings.len(), 1);
    assert_eq!(extras.shape_renderings.len(), 1);
    assert_eq!(extras.text_renderings.len(), 1);
    let out = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("overflow=\"hidden\""));
    assert!(s.contains("color-interpolation=\"linearRGB\""));
    assert!(s.contains("color-rendering=\"optimizeQuality\""));
    assert!(s.contains("shape-rendering=\"geometricPrecision\""));
    assert!(s.contains("text-rendering=\"optimizeLegibility\""));
}

/// Round 257: per-child override of a parent group's `overflow`
/// records on the child's own emit slot in addition to the parent's.
/// Unlike inherited properties where a child override is a cascade
/// reset, here each carrier slot is independent (the cascade itself
/// already resets `overflow` per element), so the side-channel
/// captures every explicit author write.
#[test]
fn per_child_override_records_separately() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g overflow="hidden">
            <g overflow="scroll">
                <rect x="10" y="10" width="50" height="50" fill="red"/>
            </g>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.overflows.len(), 2);
    let kinds: Vec<&str> = extras
        .overflows
        .iter()
        .map(|b| b.overflow.as_str())
        .collect();
    assert!(kinds.contains(&"hidden"));
    assert!(kinds.contains(&"scroll"));
}
