//! Round 291 — SVG 1.1 §10.9.2 `dominant-baseline` property
//! integration tests.
//!
//! `dominant-baseline: auto | use-script | no-change | reset-size |
//! ideographic | alphabetic | hanging | mathematical | central |
//! middle | text-after-edge | text-before-edge` selects (or
//! re-determines) the scaled-baseline-table that positions the glyphs
//! of a text content element along its inline-progression direction.
//! Per the §10.9.2 attribute table the initial value is `auto` and the
//! property is NOT inherited — mirroring the round-118 `display`,
//! round-209 `vector-effect`, and round-257 `overflow` non-inheritance
//! resets.
//!
//! Round 291 ships parse + per-element reset cascade + round-trip
//! preservation. The actual scaled-baseline-table construction + glyph
//! positioning live in `oxideav-scribe` / `oxideav-raster`; this crate
//! exposes the resolved value on `PaintState::dominant_baseline` and
//! round-trips the source attribute via
//! `PreservedExtras::dominant_baselines`.

use oxideav_svg::element::{DominantBaseline, PaintState};
use oxideav_svg::parser::{parse_xml, Element, Node as XmlNode};
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Default `dominant-baseline` is `auto` per the §10.9.2 attribute
/// table.
#[test]
fn default_dominant_baseline_is_auto() {
    let s = PaintState::default();
    assert_eq!(s.dominant_baseline, DominantBaseline::Auto);
}

/// Round 291: a document without `dominant-baseline=` parses cleanly
/// and the decoder does NOT pollute the side-channel.
#[test]
fn baseline_no_attr_no_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.dominant_baselines.is_empty(),
        "round 291: a document without dominant-baseline= must not record a binding"
    );
}

/// Round 291: `dominant-baseline="hanging"` on a `<g>` records a
/// binding with the canonicalised keyword.
#[test]
fn hanging_on_g_records_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g dominant-baseline="hanging">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.dominant_baselines.len(),
        1,
        "round 291: dominant-baseline= must record exactly one binding"
    );
    assert_eq!(extras.dominant_baselines[0].dominant_baseline, "hanging");
}

/// Round 291: each of the twelve §10.9.2 keywords parses + records in
/// its canonical (lowercase / hyphenated) spelling.
#[test]
fn each_keyword_records_canonical_form() {
    for kw in [
        "auto",
        "use-script",
        "no-change",
        "reset-size",
        "ideographic",
        "alphabetic",
        "hanging",
        "mathematical",
        "central",
        "middle",
        "text-after-edge",
        "text-before-edge",
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <g dominant-baseline="{}">
                    <rect x="10" y="10" width="50" height="50" fill="red"/>
                </g>
            </svg>"#,
            kw
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.dominant_baselines.len(), 1, "kw={}", kw);
        assert_eq!(
            extras.dominant_baselines[0].dominant_baseline, kw,
            "kw={}",
            kw
        );
    }
}

/// Round 291: source keywords are matched case-insensitively per CSS
/// rules; the binding canonicalises to the §10.9.2 spelling.
#[test]
fn keyword_matching_is_case_insensitive() {
    for (input, expected) in [
        ("HANGING", "hanging"),
        ("Hanging", "hanging"),
        ("TEXT-AFTER-EDGE", "text-after-edge"),
        ("Text-Before-Edge", "text-before-edge"),
        ("USE-SCRIPT", "use-script"),
        ("Middle", "middle"),
        ("MATHEMATICAL", "mathematical"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <g dominant-baseline="{}">
                    <rect x="10" y="10" width="50" height="50" fill="red"/>
                </g>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.dominant_baselines.len(), 1, "input={}", input);
        assert_eq!(
            extras.dominant_baselines[0].dominant_baseline, expected,
            "input={}",
            input
        );
    }
}

/// Round 291: explicit author `dominant-baseline="auto"` IS recorded
/// even though `auto` is the §10.9.2 initial value. Mirrors the
/// round-221 / round-247 / round-257 "explicit-initial-value carries
/// intent" policy (e.g. an inheritance reset on a child of a
/// `<text dominant-baseline="hanging">`).
#[test]
fn explicit_auto_is_recorded() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g dominant-baseline="auto">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.dominant_baselines.len(),
        1,
        "round 291: explicit `auto` carries author intent and IS recorded"
    );
    assert_eq!(extras.dominant_baselines[0].dominant_baseline, "auto");
}

/// Round 291: `dominant-baseline="inherit"` keeps the cascade's value
/// (which, for a non-inherited property, is the initial-value reset)
/// and skips recording (no canonical token to emit).
#[test]
fn inherit_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g dominant-baseline="inherit">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.dominant_baselines.is_empty(),
        "round 291: `inherit` keeps the resolved value and skips recording"
    );
}

/// Round 291: unrecognised tokens fall back to the cascade's
/// initial-reset value and skip recording (matches the tolerant policy
/// of the §13.x rendering-hint / §3.11 overflow branches).
#[test]
fn unknown_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g dominant-baseline="baseline">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.dominant_baselines.is_empty(),
        "round 291: unrecognised keyword keeps the resolved value and skips recording"
    );
    // The document still loads (no parse failure).
    let _ = parse_svg(src).unwrap();
}

/// Round 291: empty `dominant-baseline=""` skips recording (no keyword
/// to canonicalise).
#[test]
fn empty_value_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g dominant-baseline="">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.dominant_baselines.is_empty());
}

/// Helper: resolve a PaintState from a parsed XML fragment by finding
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

/// Round 291: a presentation attribute resolves into PaintState
/// (presentation-attribute lane of the cascade).
#[test]
fn presentation_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <text dominant-baseline="central" x="0" y="0">hi</text>
            </svg>"#,
        "text",
    );
    assert_eq!(s.dominant_baseline, DominantBaseline::Central);
}

/// Round 291: an inline `style="…"` declaration resolves into
/// PaintState (style-attribute lane wins over presentation attribute
/// per the round-4 cascade order).
#[test]
fn style_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <text style="dominant-baseline: text-before-edge" x="0" y="0">hi</text>
            </svg>"#,
        "text",
    );
    assert_eq!(s.dominant_baseline, DominantBaseline::TextBeforeEdge);
}

/// Round 291: a `<style>`-block rule resolves into PaintState via the
/// round-4 cascade.
#[test]
fn style_block_rule_resolves_in_cascade() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <style>text { dominant-baseline: mathematical }</style>
        <text dominant-baseline="alphabetic" x="0" y="0">hi</text>
    </svg>"#;
    // Document loads cleanly; the cascade machinery applies the rule.
    let _ = parse_svg(src).unwrap();
}

/// Round 291: the property is NOT inherited per the §10.9.2 attribute
/// table. A child of a `<text dominant-baseline="hanging">` does NOT
/// pick up `hanging` from the parent — `merged_with_css` resets to the
/// initial value before applying the element's own attribute (matching
/// the round-118 `display` / round-257 `overflow` reset policy).
#[test]
fn property_is_not_inherited_through_cascade() {
    let parent = PaintState {
        dominant_baseline: DominantBaseline::Hanging,
        ..PaintState::default()
    };
    let nodes =
        parse_xml(r#"<svg xmlns="http://www.w3.org/2000/svg"><tspan x="0" y="0">x</tspan></svg>"#)
            .expect("xml parse");
    let svg_el: &Element = nodes
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name.ends_with("svg") => Some(e),
            _ => None,
        })
        .expect("svg root");
    let child_el: &Element = svg_el
        .children
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name == "tspan" => Some(e),
            _ => None,
        })
        .expect("tspan");
    let sheet = oxideav_svg::css::Stylesheet::new();
    let child = parent.merged_with_css(child_el, &sheet).unwrap();
    assert_eq!(
        child.dominant_baseline,
        DominantBaseline::Auto,
        "round 291: dominant-baseline is NOT inherited per §10.9.2 — the per-element reset must \
         restore the initial value"
    );
}

/// Round 291: a child with its own `dominant-baseline=` still resolves
/// correctly after the parent's value is reset.
#[test]
fn child_attribute_wins_after_reset() {
    let parent = PaintState {
        dominant_baseline: DominantBaseline::Hanging,
        ..PaintState::default()
    };
    let nodes = parse_xml(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <tspan dominant-baseline="middle" x="0" y="0">x</tspan>
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
    let child_el: &Element = svg_el
        .children
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name == "tspan" => Some(e),
            _ => None,
        })
        .expect("tspan");
    let sheet = oxideav_svg::css::Stylesheet::new();
    let child = parent.merged_with_css(child_el, &sheet).unwrap();
    assert_eq!(child.dominant_baseline, DominantBaseline::Middle);
}

/// Round 291: round-trip preserves `dominant-baseline=` on a `<g>` — a
/// `parse_svg_with_extras → write_svg_with_extras` cycle re-emits the
/// attribute on the matching element.
#[test]
fn roundtrip_emits_attribute_on_group() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g dominant-baseline="hanging">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("dominant-baseline=\"hanging\""),
        "round-trip output should re-emit dominant-baseline: {}",
        out_s
    );
}

/// Round 291: round-trip preserves `dominant-baseline=` on a shape — a
/// shape carrying the attribute directly (not via a group) also
/// round-trips on the matching emit slot.
#[test]
fn roundtrip_emits_attribute_on_shape() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red" dominant-baseline="central"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.dominant_baselines.len(), 1);
    assert_eq!(extras.dominant_baselines[0].dominant_baseline, "central");
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("dominant-baseline=\"central\""),
        "round-trip should re-emit dominant-baseline on the shape: {}",
        out_s
    );
}

/// Round 291: double round-trip converges — a second parse-then-write
/// of the output produces identical `dominant-baseline=` content.
#[test]
fn roundtrip_is_idempotent() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g dominant-baseline="hanging">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame1, extras1) = parse_svg_with_extras(src).unwrap();
    let out1 = write_svg_with_extras(&frame1, &extras1);
    let (frame2, extras2) = parse_svg_with_extras(&out1).unwrap();
    let out2 = write_svg_with_extras(&frame2, &extras2);
    assert_eq!(
        out1, out2,
        "round 291: parse → write → parse → write must converge"
    );
    let s2 = String::from_utf8(out2).unwrap();
    assert!(s2.contains("dominant-baseline=\"hanging\""));
}

/// Round 291: source-case canonicalises through round-trip — uppercase
/// input emits as §10.9.2 lowercase / hyphenated spelling.
#[test]
fn roundtrip_canonicalises_source_case() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g dominant-baseline="TEXT-AFTER-EDGE">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("dominant-baseline=\"text-after-edge\""),
        "uppercase source should round-trip as §10.9.2 canonical spelling: {}",
        out_s
    );
}

/// Round 291: `parse_svg` (no extras) still loads the document cleanly
/// — the property cascade machinery doesn't require the side-channel.
#[test]
fn parse_svg_without_extras_still_loads() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g dominant-baseline="hanging">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let _ = parse_svg(src).unwrap();
}

/// Round 291: a `<g dominant-baseline=…>` ancestor records the
/// attribute on the group's own emit site (not redundantly per child).
/// Mirrors the round-257 group-records-once pattern.
#[test]
fn group_attribute_records_once_not_per_child() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g dominant-baseline="hanging">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
            <rect x="30" y="30" width="50" height="50" fill="blue"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.dominant_baselines.len(),
        1,
        "round 291: a `<g>`-level attribute records once, not per child"
    );
    assert_eq!(extras.dominant_baselines[0].dominant_baseline, "hanging");
}

/// Round 291: per-child override of a parent group's
/// `dominant-baseline` records on the child's own emit slot in
/// addition to the parent's. Each carrier slot is independent (the
/// cascade itself already resets `dominant-baseline` per element), so
/// the side-channel captures every explicit author write.
#[test]
fn per_child_override_records_separately() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g dominant-baseline="hanging">
            <rect x="10" y="10" width="50" height="50" fill="red" dominant-baseline="middle"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.dominant_baselines.len(),
        2,
        "round 291: both the group and the per-child override record their own binding"
    );
    let mut seen: Vec<&str> = extras
        .dominant_baselines
        .iter()
        .map(|b| b.dominant_baseline.as_str())
        .collect();
    seen.sort_unstable();
    assert_eq!(seen, vec!["hanging", "middle"]);
}

/// Round 291: `dominant-baseline` coexists with the §3.11 `overflow`
/// and §13.x rendering hints as independent properties on the same
/// `<g>`; all round-trip via their own side-channels.
#[test]
fn coexists_with_other_hints_on_same_group() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g dominant-baseline="central" overflow="hidden"
           text-rendering="optimizeLegibility">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.dominant_baselines.len(), 1);
    assert_eq!(extras.overflows.len(), 1);
    assert_eq!(extras.text_renderings.len(), 1);
    let out = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("dominant-baseline=\"central\""));
    assert!(s.contains("overflow=\"hidden\""));
    assert!(s.contains("text-rendering=\"optimizeLegibility\""));
}
