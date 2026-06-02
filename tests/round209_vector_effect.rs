//! Round 209 — SVG 2 §8.13 `vector-effect` integration tests.
//!
//! `vector-effect: none | [ non-scaling-stroke | non-scaling-size |
//! non-rotation | fixed-position ]+ [ viewport | screen ]?` selects one
//! or more constrained-transform effects on graphics elements and
//! `<use>`. Per §8.13:
//!
//! * Initial value `none` (the renderer applies the normal CTM).
//! * Applies to graphics elements and `<use>` only.
//! * NOT inherited — a `<g vector-effect=…>` does NOT propagate the
//!   property to child shapes (the cascade resets to `none` at every
//!   element).
//! * Optional `viewport` / `screen` host suffix names the host
//!   coordinate space (initial / absent → `viewport`).
//!
//! Round 209 ships parse + non-inherited cascade + round-trip
//! preservation. The actual coordinate-transform suppression lives in
//! `oxideav-raster`; this crate exposes the resolved value on
//! `PaintState::vector_effect` and round-trips the source attribute via
//! `PreservedExtras::vector_effects`.

use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Round 209: a shape without `vector-effect=` parses cleanly and the
/// decoder does NOT pollute the side-channel.
#[test]
fn baseline_no_vector_effect_attr_no_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.vector_effects.is_empty(),
        "round 209: a shape without vector-effect= must not record a binding"
    );
}

/// Round 209: `vector-effect="non-scaling-stroke"` on a shape parses
/// cleanly and records a binding.
#[test]
fn non_scaling_stroke_on_rect_records_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red" stroke="black"
              stroke-width="3" vector-effect="non-scaling-stroke"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.vector_effects.len(),
        1,
        "round 209: vector-effect= must record exactly one binding"
    );
    assert_eq!(extras.vector_effects[0].vector_effect, "non-scaling-stroke");
}

/// Round 209: `vector-effect="none"` is the initial value and skips
/// recording.
#[test]
fn explicit_none_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"
              vector-effect="none"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.vector_effects.is_empty(),
        "round 209: explicit `none` is the initial value and skips recording"
    );
}

/// Round 209: §8.13 grammar accepts the multi-keyword form
/// `[ … ]+` — multiple effect keywords in source order.
#[test]
fn multi_keyword_set_is_preserved_in_source_order() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <path d="M 0 0 L 100 100" stroke="black" stroke-width="2"
              vector-effect="non-scaling-size non-rotation"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.vector_effects.len(), 1);
    assert_eq!(
        extras.vector_effects[0].vector_effect,
        "non-scaling-size non-rotation"
    );
}

/// Round 209: duplicate keywords drop per the `[ … ]+` CSS combinator
/// rule; first occurrence wins, subsequent occurrences are silently
/// removed.
#[test]
fn duplicate_keywords_are_dropped() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="0" y="0" width="50" height="50"
              vector-effect="non-scaling-stroke non-scaling-stroke"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.vector_effects.len(), 1);
    assert_eq!(extras.vector_effects[0].vector_effect, "non-scaling-stroke");
}

/// Round 209: explicit `screen` host suffix is preserved; absent host
/// suffix does NOT add a redundant `viewport` token (the initial value
/// is implied).
#[test]
fn host_suffix_screen_is_preserved_but_viewport_default_is_implicit() {
    let src_screen = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="0" y="0" width="50" height="50"
              vector-effect="non-scaling-stroke screen"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src_screen).unwrap();
    assert_eq!(extras.vector_effects.len(), 1);
    assert_eq!(
        extras.vector_effects[0].vector_effect,
        "non-scaling-stroke screen"
    );

    let src_default = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="0" y="0" width="50" height="50"
              vector-effect="non-scaling-stroke"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src_default).unwrap();
    assert_eq!(extras.vector_effects.len(), 1);
    assert_eq!(
        extras.vector_effects[0].vector_effect, "non-scaling-stroke",
        "round 209: absent host suffix must NOT add a redundant `viewport` token"
    );
}

/// Round 209: §8.13 attribute table says "Inherited: no". A
/// `<g vector-effect="non-scaling-stroke">` does NOT push the property
/// onto a child `<rect>` that has no `vector-effect=` of its own.
/// Round-trip preservation still captures the attribute on the `<g>`
/// emit site so the source survives.
#[test]
fn property_is_not_inherited_from_group() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g vector-effect="non-scaling-stroke">
            <rect x="0" y="0" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    // Exactly one binding — the `<g>`'s own — because the property is
    // non-inherited per §8.13 and the child `<rect>` carries no
    // attribute of its own.
    assert_eq!(
        extras.vector_effects.len(),
        1,
        "round 209: non-inherited property must not duplicate onto every descendant"
    );
}

/// Round 209: case-insensitive keyword matching (CSS values are
/// case-insensitive). The canonical form lowercases everything.
#[test]
fn keyword_matching_is_case_insensitive() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="0" y="0" width="50" height="50"
              vector-effect="Non-Scaling-Stroke"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.vector_effects.len(), 1);
    assert_eq!(
        extras.vector_effects[0].vector_effect, "non-scaling-stroke",
        "canonical form must be lowercase"
    );
}

/// Round 209: unknown / unrecognised keywords are silently dropped
/// (matches the tolerant policy used by paint-order / text-anchor /
/// visibility). A payload with at least one recognised keyword still
/// records a binding.
#[test]
fn unknown_tokens_are_silently_dropped() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="0" y="0" width="50" height="50"
              vector-effect="non-scaling-stroke bogus-keyword"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.vector_effects.len(), 1);
    assert_eq!(extras.vector_effects[0].vector_effect, "non-scaling-stroke");
}

/// Round 209: a payload with NO recognised effect keyword (e.g. a bare
/// `viewport` / unknown token) does NOT record a binding — the §8.13
/// grammar requires at least one effect keyword. The decoder treats
/// such a payload as the initial value (`none`).
#[test]
fn payload_without_effect_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="0" y="0" width="50" height="50"
              vector-effect="bogus-only"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.vector_effects.is_empty(),
        "round 209: a payload with no recognised effect keyword is not a valid vector-effect"
    );
}

/// Round 209: empty `vector-effect=""` and `vector-effect="inherit"`
/// both fall back to the initial value and skip recording.
#[test]
fn empty_and_inherit_skip_recording() {
    for payload in [r#"vector-effect="""#, r#"vector-effect="inherit""#] {
        let xml = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <rect x="0" y="0" width="50" height="50" {}/>
            </svg>"#,
            payload
        );
        let (_frame, extras) = parse_svg_with_extras(xml.as_bytes()).unwrap();
        assert!(
            extras.vector_effects.is_empty(),
            "round 209: payload {:?} must not record a binding",
            payload
        );
    }
}

/// Round 209: legacy `parse_svg` (no extras) still parses a document
/// with `vector-effect=` cleanly — the property's source-faithful
/// round-trip is opt-in.
#[test]
fn parse_svg_without_extras_still_loads_document() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"
              vector-effect="non-scaling-stroke"/>
    </svg>"#;
    // `parse_svg` returns the frame only — no binding is captured here
    // but the call must succeed.
    let frame = parse_svg(src).unwrap();
    assert!(!frame.root.children.is_empty(), "rect must still emit");
}

/// Round 209: round-trip through `write_svg_with_extras` re-emits the
/// `vector-effect=` attribute on the matching `<rect>` element.
#[test]
fn round_trip_re_emits_attribute_on_rect() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red" stroke="black"
              stroke-width="3" vector-effect="non-scaling-stroke"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_str = String::from_utf8(out).unwrap();
    assert!(
        out_str.contains(r#"vector-effect="non-scaling-stroke""#),
        "round 209: round-trip must re-emit the canonicalised vector-effect attribute, got:\n{}",
        out_str
    );
}

/// Round 209: round-trip re-emits the multi-keyword form too, in the
/// canonical source order.
#[test]
fn round_trip_preserves_multi_keyword_order() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <path d="M 0 0 L 100 100" stroke="black" stroke-width="2"
              vector-effect="non-rotation non-scaling-size screen"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_str = String::from_utf8(out).unwrap();
    assert!(
        out_str.contains(r#"vector-effect="non-rotation non-scaling-size screen""#),
        "round 209: round-trip must preserve source order and host suffix, got:\n{}",
        out_str
    );
}

/// Round 209: a `<g vector-effect=…>` attribute round-trips on the `<g>`
/// emit site so a hand-authored grouping attribute survives a
/// `parse → write` cycle. The decoder doesn't propagate the property
/// (non-inherited per §8.13), but the round-trip carrier is purely
/// lexical.
#[test]
fn round_trip_preserves_group_attribute() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g vector-effect="non-scaling-stroke">
            <rect x="0" y="0" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_str = String::from_utf8(out).unwrap();
    assert!(
        out_str.contains(r#"vector-effect="non-scaling-stroke""#),
        "round 209: group's source attribute must round-trip on the <g>, got:\n{}",
        out_str
    );
}

/// Round 209: two consecutive round-trip cycles converge — the
/// canonical form is stable so `parse → write → parse → write` produces
/// the same bytes as `parse → write`.
#[test]
fn round_trip_is_stable_under_double_pass() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red" stroke="black"
              stroke-width="3" vector-effect="non-scaling-stroke"/>
    </svg>"#;
    let (frame1, extras1) = parse_svg_with_extras(src).unwrap();
    let out1 = write_svg_with_extras(&frame1, &extras1);
    let (frame2, extras2) = parse_svg_with_extras(&out1).unwrap();
    let out2 = write_svg_with_extras(&frame2, &extras2);
    assert_eq!(
        out1, out2,
        "round 209: a second round-trip must converge byte-for-byte"
    );
    assert_eq!(extras1.vector_effects.len(), 1);
    assert_eq!(extras2.vector_effects.len(), 1);
    assert_eq!(
        extras1.vector_effects[0].vector_effect, extras2.vector_effects[0].vector_effect,
        "round 209: canonical keyword string must be stable across cycles"
    );
}

/// Round 209: comma-or-whitespace separator tolerance — the §8.13
/// grammar is whitespace-separated, but a comma-separated payload is
/// commonly seen in the wild (especially from authoring tools that
/// generate CSS-style value lists). Whitespace-only keeps strict
/// behaviour; the canonical form always emits single-space separators.
#[test]
fn keywords_canonicalise_to_single_space_separators() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="0" y="0" width="50" height="50"
              vector-effect="non-scaling-stroke   non-rotation"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.vector_effects.len(), 1);
    assert_eq!(
        extras.vector_effects[0].vector_effect, "non-scaling-stroke non-rotation",
        "round 209: canonical form collapses repeated whitespace to single spaces"
    );
}
