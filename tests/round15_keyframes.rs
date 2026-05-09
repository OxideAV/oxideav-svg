//! Round 15 — CSS `@keyframes` block capture per CSS Animations L1 §3.
//!
//! Round 11 + 14 routed `@import` and `@font-face` to dedicated parsers
//! but still silently dropped `@keyframes`. Round 15 routes
//! `@keyframes <name> { sel { ... } sel { ... } }` to a dedicated
//! parser that surfaces the rule on `Stylesheet::keyframes` for a
//! downstream animation engine to consume.

use oxideav_svg::css::{KeyframeOffset, Stylesheet};

#[test]
fn captures_two_keyframes_rules_in_one_stylesheet() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @keyframes spin {
            from { transform: rotate(0); }
            to { transform: rotate(360deg); }
        }
        @keyframes fade {
            0% { opacity: 0; }
            100% { opacity: 1; }
        }
        rect { fill: red }
        "#,
    );
    assert_eq!(s.keyframes.len(), 2, "expected two @keyframes rules");
    assert_eq!(s.keyframes[0].name, "spin");
    assert_eq!(s.keyframes[1].name, "fade");
    // Cascade rule survived alongside the @keyframes blocks.
    assert_eq!(s.rules.len(), 1);
}

#[test]
fn from_to_offsets_resolve_to_normalised_positions() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @keyframes spin {
            from { transform: rotate(0); }
            to { transform: rotate(360deg); }
        }
        "#,
    );
    let rule = &s.keyframes[0];
    assert_eq!(rule.selectors.len(), 2);
    assert_eq!(rule.selectors[0].offset, KeyframeOffset::From);
    assert_eq!(rule.selectors[1].offset, KeyframeOffset::To);
    assert!((rule.selectors[0].offset.as_normalised() - 0.0).abs() < 1e-6);
    assert!((rule.selectors[1].offset.as_normalised() - 1.0).abs() < 1e-6);
}

#[test]
fn percentage_offsets_parse() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @keyframes pulse {
            0% { opacity: 0; }
            50% { opacity: 1; }
            100% { opacity: 0; }
        }
        "#,
    );
    let rule = &s.keyframes[0];
    assert_eq!(rule.selectors.len(), 3);
    assert_eq!(rule.selectors[0].offset, KeyframeOffset::Percent(0.0));
    assert_eq!(rule.selectors[1].offset, KeyframeOffset::Percent(50.0));
    assert_eq!(rule.selectors[2].offset, KeyframeOffset::Percent(100.0));
    assert!((rule.selectors[1].offset.as_normalised() - 0.5).abs() < 1e-6);
}

#[test]
fn declarations_per_selector_are_captured() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @keyframes slide {
            from {
                transform: translateX(0);
                opacity: 0;
            }
            to {
                transform: translateX(100px);
                opacity: 1;
            }
        }
        "#,
    );
    let rule = &s.keyframes[0];
    assert_eq!(rule.name, "slide");
    let from_decls = &rule.selectors[0].declarations;
    assert_eq!(from_decls.len(), 2);
    // Names get lowercased by parse_declarations.
    assert_eq!(from_decls[0].0, "transform");
    assert_eq!(from_decls[0].1, "translateX(0)");
    assert_eq!(from_decls[1].0, "opacity");
    assert_eq!(from_decls[1].1, "0");
    let to_decls = &rule.selectors[1].declarations;
    assert_eq!(to_decls[0].1, "translateX(100px)");
}

#[test]
fn comma_separated_offsets_expand_to_multiple_selectors() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @keyframes glance {
            0%, 100% { opacity: 0.2; }
            50% { opacity: 1; }
        }
        "#,
    );
    let rule = &s.keyframes[0];
    // `0%, 100%` → two entries with the same declarations; plus `50%`.
    assert_eq!(rule.selectors.len(), 3);
    assert_eq!(rule.selectors[0].offset, KeyframeOffset::Percent(0.0));
    assert_eq!(rule.selectors[1].offset, KeyframeOffset::Percent(100.0));
    assert_eq!(rule.selectors[2].offset, KeyframeOffset::Percent(50.0));
    // First two share declarations.
    assert_eq!(
        rule.selectors[0].declarations,
        rule.selectors[1].declarations
    );
}

#[test]
fn webkit_prefix_keyframes_recognised() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @-webkit-keyframes spin {
            from { transform: rotate(0); }
            to { transform: rotate(360deg); }
        }
        "#,
    );
    assert_eq!(s.keyframes.len(), 1);
    assert_eq!(s.keyframes[0].name, "spin");
}

#[test]
fn empty_keyframes_block_is_dropped() {
    let mut s = Stylesheet::new();
    s.parse_block("@keyframes empty {}");
    assert_eq!(s.keyframes.len(), 0, "empty body produces no rule");
}

#[test]
fn unnamed_keyframes_block_is_dropped() {
    let mut s = Stylesheet::new();
    // `@keyframes` with no name → invalid per §3.
    s.parse_block("@keyframes { from { opacity: 0 } to { opacity: 1 } }");
    assert_eq!(s.keyframes.len(), 0);
}

#[test]
fn quoted_animation_name_is_unquoted() {
    let mut s = Stylesheet::new();
    s.parse_block(r#"@keyframes "my-anim" { from { opacity: 0 } to { opacity: 1 } }"#);
    assert_eq!(s.keyframes.len(), 1);
    assert_eq!(s.keyframes[0].name, "my-anim");
}

#[test]
fn keyframes_alongside_font_face_and_import() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @import url("theme.css");
        @font-face { font-family: "Acme"; src: url("acme.woff2"); }
        @keyframes spin { from { opacity: 0 } to { opacity: 1 } }
        rect { fill: red }
        "#,
    );
    assert_eq!(s.imports.len(), 1);
    assert_eq!(s.font_faces.len(), 1);
    assert_eq!(s.keyframes.len(), 1);
    assert_eq!(s.rules.len(), 1);
}
