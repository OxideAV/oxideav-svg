//! Round 16 — CSS `@media (cond) { ... }` block parse + evaluation per
//! CSS Media Queries L4.
//!
//! Round 11 + 14 + 15 routed `@import`, `@font-face` and `@keyframes`
//! to dedicated parsers but still silently dropped `@media`. Round 16
//! routes `@media` to a typed [`MediaRule`] and adds
//! [`Stylesheet::resolve_for_media_context`] which evaluates each
//! captured query against a runtime viewport and returns the merged
//! cascade in source order.

use oxideav_svg::css::{
    ComparisonOp, MediaFeature, MediaOperator, MediaValue, Orientation, Stylesheet,
};

#[test]
fn parses_at_media_block_into_media_rules() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        rect { fill: red }
        @media (max-width: 800px) {
            rect { fill: blue }
        }
        "#,
    );
    // Unconditional rule survived alongside the @media block.
    assert_eq!(s.rules.len(), 1, "only the unconditional rule");
    assert_eq!(s.media_rules.len(), 1, "one @media block captured");
    let mr = &s.media_rules[0];
    assert_eq!(mr.rules.len(), 1, "one inner rule under (max-width: 800px)");
    // The query has one feature: `max-width`.
    assert_eq!(mr.condition.queries.len(), 1);
    let q = &mr.condition.queries[0];
    assert_eq!(q.features.len(), 1);
    assert_eq!(q.features[0].name, "max-width");
    assert_eq!(q.features[0].op, ComparisonOp::MaxEq);
    matches!(q.features[0].value, MediaValue::Length(800.0));
}

#[test]
fn parses_min_width_block() {
    let mut s = Stylesheet::new();
    s.parse_block(r#"@media (min-width: 600px) { .x { fill: green } }"#);
    let mr = &s.media_rules[0];
    let q = &mr.condition.queries[0];
    assert_eq!(q.features[0].name, "min-width");
    assert_eq!(q.features[0].op, ComparisonOp::MinEq);
}

#[test]
fn resolves_correct_rule_set_at_each_viewport() {
    // Two @media blocks (different breakpoints) + one unconditional
    // rule. The resolve method should pick the right inner rules for
    // each viewport.
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        rect { fill: black }
        @media (max-width: 800px) {
            rect { fill: blue }
        }
        @media (min-width: 1000px) {
            rect { fill: red }
        }
        "#,
    );
    // viewport 600 → only the (max-width: 800px) block matches.
    let cascade_600 = s.resolve_for_media_context(600.0, 400.0, Orientation::Landscape);
    assert_eq!(
        cascade_600.len(),
        2,
        "unconditional + (max-width: 800px) match"
    );
    assert_eq!(cascade_600[0].declarations[0].1, "black");
    assert_eq!(cascade_600[1].declarations[0].1, "blue");

    // viewport 1200 → only the (min-width: 1000px) block matches.
    let cascade_1200 = s.resolve_for_media_context(1200.0, 800.0, Orientation::Landscape);
    assert_eq!(
        cascade_1200.len(),
        2,
        "unconditional + (min-width: 1000px) match"
    );
    assert_eq!(cascade_1200[0].declarations[0].1, "black");
    assert_eq!(cascade_1200[1].declarations[0].1, "red");

    // viewport 900 → neither @media matches.
    let cascade_900 = s.resolve_for_media_context(900.0, 400.0, Orientation::Landscape);
    assert_eq!(cascade_900.len(), 1);
    assert_eq!(cascade_900[0].declarations[0].1, "black");
}

#[test]
fn orientation_query_matches_landscape_only() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @media (orientation: landscape) {
            rect { fill: cyan }
        }
        "#,
    );
    let landscape = s.resolve_for_media_context(800.0, 400.0, Orientation::Landscape);
    assert_eq!(landscape.len(), 1);
    let portrait = s.resolve_for_media_context(400.0, 800.0, Orientation::Portrait);
    assert_eq!(portrait.len(), 0);
}

#[test]
fn comma_separated_query_list_ors() {
    // `@media (max-width: 600px), (orientation: landscape)` → matches
    // when EITHER query passes.
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @media (max-width: 600px), (orientation: landscape) {
            rect { fill: yellow }
        }
        "#,
    );
    // 1200px + landscape → matches via the orientation half.
    let big_landscape = s.resolve_for_media_context(1200.0, 400.0, Orientation::Landscape);
    assert_eq!(big_landscape.len(), 1);
    // 1200px + portrait → neither half matches.
    let big_portrait = s.resolve_for_media_context(1200.0, 1600.0, Orientation::Portrait);
    assert_eq!(big_portrait.len(), 0);
    // 400px + portrait → matches via the max-width half.
    let small_portrait = s.resolve_for_media_context(400.0, 800.0, Orientation::Portrait);
    assert_eq!(small_portrait.len(), 1);
}

#[test]
fn and_joined_clauses_require_both() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @media (min-width: 600px) and (max-width: 1000px) {
            rect { fill: magenta }
        }
        "#,
    );
    // Inside the band → matches.
    assert_eq!(
        s.resolve_for_media_context(800.0, 400.0, Orientation::Landscape)
            .len(),
        1
    );
    // Below the band → no match.
    assert_eq!(
        s.resolve_for_media_context(500.0, 400.0, Orientation::Landscape)
            .len(),
        0
    );
    // Above the band → no match.
    assert_eq!(
        s.resolve_for_media_context(1200.0, 400.0, Orientation::Landscape)
            .len(),
        0
    );
}

#[test]
fn not_modifier_inverts_match() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @media not (max-width: 800px) {
            rect { fill: violet }
        }
        "#,
    );
    // 600 ≤ 800 → inner condition true → `not` makes it false.
    assert_eq!(
        s.resolve_for_media_context(600.0, 400.0, Orientation::Landscape)
            .len(),
        0
    );
    // 1200 > 800 → inner condition false → `not` makes it true.
    assert_eq!(
        s.resolve_for_media_context(1200.0, 400.0, Orientation::Landscape)
            .len(),
        1
    );
}

#[test]
fn media_type_screen_matches_default_runtime() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @media screen and (min-width: 400px) {
            rect { fill: lime }
        }
        "#,
    );
    let cascade = s.resolve_for_media_context(800.0, 400.0, Orientation::Landscape);
    assert_eq!(cascade.len(), 1);
}

#[test]
fn print_media_type_does_not_match_screen_runtime() {
    let mut s = Stylesheet::new();
    s.parse_block(r#"@media print { rect { fill: pink } }"#);
    let cascade = s.resolve_for_media_context(800.0, 400.0, Orientation::Landscape);
    assert_eq!(cascade.len(), 0);
}

#[test]
fn empty_at_media_body_drops_block() {
    let mut s = Stylesheet::new();
    s.parse_block(r#"@media (max-width: 800px) {}"#);
    assert_eq!(s.media_rules.len(), 0, "empty @media body produces no rule");
}

#[test]
fn raw_unrecognised_feature_is_dormant() {
    // `(prefers-color-scheme: dark)` isn't modelled in round 16; the
    // `@media` block is captured but never matches.
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @media (prefers-color-scheme: dark) {
            rect { fill: black }
        }
        "#,
    );
    assert_eq!(s.media_rules.len(), 1);
    let cascade = s.resolve_for_media_context(800.0, 400.0, Orientation::Landscape);
    assert_eq!(cascade.len(), 0);
}

#[test]
fn programmatic_media_feature_constructible() {
    // Smoke-test the public types compile + round-trip.
    let f = MediaFeature {
        name: "min-width".into(),
        op: ComparisonOp::MinEq,
        value: MediaValue::Length(600.0),
    };
    assert_eq!(f.name, "min-width");
    let _op = MediaOperator::Not;
}

#[test]
fn unconditional_rules_remain_unaffected_by_media_blocks() {
    // Existing test case `at_rule_skipped` (in css.rs) was the round-15
    // contract: an @media block did NOT inject rules into Stylesheet::rules.
    // Round 16 keeps that contract — only the trailing `.y` rule lives in
    // `rules`; the @media inner rule lives in `media_rules`.
    let mut s = Stylesheet::new();
    s.parse_block("@media print { .x { fill: red } } .y { fill: blue }");
    assert_eq!(s.rules.len(), 1);
    assert_eq!(s.rules[0].selectors[0].head.classes, vec!["y".to_string()]);
    assert_eq!(s.media_rules.len(), 1);
}
