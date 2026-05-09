//! Round 18 — CSS Values L4 length-unit aware coordinate parsing.
//!
//! Validates that the new `oxideav_svg::length::Length` type correctly
//! parses every CSS Values L4 unit suffix and resolves to the
//! spec-mandated CSS px value at multiple viewport sizes / font-size
//! contexts. Existing user-unit (bare-number) coordinates must
//! resolve bit-for-bit identically to a raw `f32::from_str` to keep
//! the round-trip honest for fixtures that use only numeric
//! attributes.

use oxideav_svg::length::{parse_length, Length, LengthUnit, ParseError, ResolveContext};

#[test]
fn user_unit_resolution_matches_legacy_f32_parse() {
    // Bit-for-bit guarantee: bare numeric attributes have to round-
    // trip through Length::resolve as the same f32 the legacy
    // parser would have produced.
    for src in ["0", "1", "-7.5", "100", "0.0625", "1234.5"] {
        let l = parse_length(src).unwrap();
        assert_eq!(l.unit, LengthUnit::UserUnit);
        let legacy: f32 = src.parse().unwrap();
        let resolved = l.resolve(ResolveContext::default());
        assert!(
            (resolved - legacy).abs() < 1e-6,
            "user-unit drift: {src} → resolved {resolved} vs legacy {legacy}"
        );
    }
}

#[test]
fn em_resolves_against_supplied_font_size() {
    // <rect x="1em"> at font-size 16 → 16 px; at 24 → 24 px.
    let l = parse_length("1em").unwrap();
    assert_eq!(l.unit, LengthUnit::Em);
    let r16 = l.resolve(ResolveContext::default().with_font_size(16.0));
    assert!((r16 - 16.0).abs() < 1e-6);
    let r24 = l.resolve(ResolveContext::default().with_font_size(24.0));
    assert!((r24 - 24.0).abs() < 1e-6);
}

#[test]
fn percentage_resolves_against_basis() {
    // <line x1="50%"> against a 200px basis → 100 px;
    // against a 1000px basis → 500 px.
    let l = parse_length("50%").unwrap();
    assert_eq!(l.unit, LengthUnit::Percent);
    let r200 = l.resolve(ResolveContext::default().with_percentage_basis(200.0));
    assert!((r200 - 100.0).abs() < 1e-6);
    let r1000 = l.resolve(ResolveContext::default().with_percentage_basis(1000.0));
    assert!((r1000 - 500.0).abs() < 1e-6);
}

#[test]
fn vw_vh_track_distinct_axes() {
    // 2vw against 800x600 → 16 px; 2vh against 800x600 → 12 px.
    let lvw = parse_length("2vw").unwrap();
    let lvh = parse_length("2vh").unwrap();
    let ctx = ResolveContext::default().with_viewport(800.0, 600.0);
    assert!((lvw.resolve(ctx) - 16.0).abs() < 1e-6);
    assert!((lvh.resolve(ctx) - 12.0).abs() < 1e-6);
    // Re-resolve at a different viewport to confirm the unit + value
    // are stored separately from the resolution context.
    let ctx2 = ResolveContext::default().with_viewport(2000.0, 1000.0);
    assert!((lvw.resolve(ctx2) - 40.0).abs() < 1e-6);
    assert!((lvh.resolve(ctx2) - 20.0).abs() < 1e-6);
}

#[test]
fn css_values_l4_absolute_unit_factors() {
    // Spot-check every absolute unit per CSS Values L4 §6.1.1.
    let ctx = ResolveContext::default();
    let cases: &[(&str, f32)] = &[
        ("1px", 1.0),
        ("1in", 96.0),
        ("1pt", 4.0 / 3.0),
        ("1pc", 16.0),
        ("1cm", 96.0 / 2.54),
        ("1mm", 96.0 / 25.4),
        ("1q", (96.0 / 25.4) * 0.25),
    ];
    for (src, expected) in cases {
        let l = parse_length(src).unwrap();
        let r = l.resolve(ctx);
        assert!(
            (r - expected).abs() < 1e-3,
            "{src} resolved to {r}, expected {expected}"
        );
    }
}

#[test]
fn rem_pinned_to_root_font_size_not_element_font_size() {
    // 2rem against root 16px / element 99px → 32 px (rem ignores
    // the per-element font-size — that's the whole point).
    let l = parse_length("2rem").unwrap();
    assert_eq!(l.unit, LengthUnit::Rem);
    let ctx = ResolveContext::default()
        .with_font_size(99.0)
        .with_root_font_size(16.0);
    assert!((l.resolve(ctx) - 32.0).abs() < 1e-6);
}

#[test]
fn round_trip_for_unit_typed_constructor() {
    // Direct constructor preserves both fields.
    let l = Length::new(3.5, LengthUnit::Em);
    assert_eq!(l.value, 3.5);
    assert_eq!(l.unit, LengthUnit::Em);
}

#[test]
fn parse_errors_on_garbage_inputs() {
    assert_eq!(parse_length(""), Err(ParseError::Empty));
    assert_eq!(parse_length("abc"), Err(ParseError::BadNumber));
    // Unknown unit suffix.
    assert_eq!(parse_length("12foo"), Err(ParseError::UnknownUnit));
    // 'em' must beat the f32 'e' exponent — `3.5em` parses as Em(3.5),
    // not BadNumber.
    let l = parse_length("3.5em").unwrap();
    assert_eq!(l.unit, LengthUnit::Em);
}

#[test]
fn synthetic_svg_attribute_set_at_two_viewports() {
    // The dispatch report's acceptance test: synthetic SVG with `1em`
    // / `50%` / `2vw` coordinates resolved at multiple viewport sizes.
    // Source is a notional set of attribute values — the typed
    // length API is what an SVG renderer would call once it has its
    // per-element resolution context.
    let attrs: &[(&str, LengthUnit)] = &[
        ("1em", LengthUnit::Em),
        ("50%", LengthUnit::Percent),
        ("2vw", LengthUnit::Vw),
    ];
    // Build typed lengths up-front and resolve at each viewport.
    let parsed: Vec<Length> = attrs
        .iter()
        .map(|(s, _)| parse_length(s).unwrap())
        .collect();
    for (l, (_, expected_unit)) in parsed.iter().zip(attrs.iter()) {
        assert_eq!(l.unit, *expected_unit);
    }
    // Viewport 100x200, font-size 10, % basis 100.
    let ctx_a = ResolveContext::default()
        .with_viewport(100.0, 200.0)
        .with_font_size(10.0)
        .with_percentage_basis(100.0);
    assert!((parsed[0].resolve(ctx_a) - 10.0).abs() < 1e-6); // 1em → 10
    assert!((parsed[1].resolve(ctx_a) - 50.0).abs() < 1e-6); // 50% of 100 → 50
    assert!((parsed[2].resolve(ctx_a) - 2.0).abs() < 1e-6); //  2vw of 100 → 2
                                                            // Viewport 2000x500, font-size 20, % basis 800.
    let ctx_b = ResolveContext::default()
        .with_viewport(2000.0, 500.0)
        .with_font_size(20.0)
        .with_percentage_basis(800.0);
    assert!((parsed[0].resolve(ctx_b) - 20.0).abs() < 1e-6); // 1em → 20
    assert!((parsed[1].resolve(ctx_b) - 400.0).abs() < 1e-6); // 50% of 800 → 400
    assert!((parsed[2].resolve(ctx_b) - 40.0).abs() < 1e-6); // 2vw of 2000 → 40
}
