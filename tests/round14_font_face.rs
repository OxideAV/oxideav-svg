//! Round 14 — CSS `@font-face` block capture per CSS Fonts L3 §4.
//!
//! Round 11 + 13 routed `@import` to `Stylesheet::imports` but tagged
//! every other `@-rule` (including `@font-face`) for tolerant skip in
//! `parse_block`. Round 14 routes `@font-face { ... }` to a dedicated
//! parser that surfaces the descriptor list on
//! `Stylesheet::font_faces` so a downstream font-resolver can register
//! the user-supplied fonts before the cascade matches a
//! `font-family: ...` declaration.

use oxideav_svg::css::{FontSource, Stylesheet};

#[test]
fn parses_two_font_face_blocks_in_one_stylesheet() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @font-face {
            font-family: "Acme Sans";
            src: url("acme-sans.woff2") format("woff2");
            font-weight: 400;
            font-style: normal;
        }
        @font-face {
            font-family: "Acme Serif";
            src: url("acme-serif.ttf") format("truetype");
            font-weight: 700;
        }
        rect { fill: red }
        "#,
    );
    assert_eq!(s.font_faces.len(), 2, "expected two @font-face blocks");
    assert_eq!(s.font_faces[0].family, "Acme Sans");
    assert_eq!(s.font_faces[1].family, "Acme Serif");
    // Cascade rule survived alongside the @font-face blocks.
    assert_eq!(s.rules.len(), 1, "non-@font-face rule still parsed");
}

#[test]
fn captures_url_src_with_format_hint() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @font-face {
            font-family: "MyFont";
            src: url("foo.woff2") format("woff2");
        }
        "#,
    );
    assert_eq!(s.font_faces.len(), 1);
    assert_eq!(s.font_faces[0].src.len(), 1);
    let entry = &s.font_faces[0].src[0];
    assert_eq!(entry.url.as_deref(), Some("foo.woff2"));
    assert_eq!(entry.format_hint.as_deref(), Some("woff2"));
    assert_eq!(entry.local_name, None);
}

#[test]
fn captures_local_src() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @font-face {
            font-family: "System UI";
            src: local("SF Pro Text");
        }
        "#,
    );
    let face = &s.font_faces[0];
    assert_eq!(face.src.len(), 1);
    assert_eq!(face.src[0].local_name.as_deref(), Some("SF Pro Text"));
    assert_eq!(face.src[0].url, None);
    assert_eq!(face.src[0].format_hint, None);
}

#[test]
fn captures_fallback_src_list_with_local_then_url() {
    // Per CSS Fonts L3 §4.3 — comma-separated fallback list.
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @font-face {
            font-family: "Mixed";
            src: local("Helvetica Neue"), url("helvetica.woff2") format("woff2"), local("Arial");
        }
        "#,
    );
    let entries = &s.font_faces[0].src;
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].local_name.as_deref(), Some("Helvetica Neue"));
    assert_eq!(entries[1].url.as_deref(), Some("helvetica.woff2"));
    assert_eq!(entries[1].format_hint.as_deref(), Some("woff2"));
    assert_eq!(entries[2].local_name.as_deref(), Some("Arial"));
}

#[test]
fn captures_long_tail_descriptors_in_descriptors_map() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @font-face {
            font-family: Test;
            src: url(x.woff2);
            font-weight: 100 900;
            font-style: italic;
            font-stretch: condensed;
            unicode-range: U+0000-00FF, U+0131;
            font-display: swap;
        }
        "#,
    );
    let face = &s.font_faces[0];
    assert_eq!(
        face.descriptors.get("font-weight").map(|s| s.as_str()),
        Some("100 900")
    );
    assert_eq!(
        face.descriptors.get("font-style").map(|s| s.as_str()),
        Some("italic")
    );
    assert_eq!(
        face.descriptors.get("font-stretch").map(|s| s.as_str()),
        Some("condensed")
    );
    assert_eq!(
        face.descriptors.get("unicode-range").map(|s| s.as_str()),
        Some("U+0000-00FF, U+0131")
    );
    assert_eq!(
        face.descriptors.get("font-display").map(|s| s.as_str()),
        Some("swap")
    );
    // font-family + src are also retained verbatim for round-trip.
    assert_eq!(
        face.descriptors.get("font-family").map(|s| s.as_str()),
        Some("Test")
    );
    assert!(face.descriptors.contains_key("src"));
}

#[test]
fn at_font_face_does_not_emit_a_cascade_rule() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @font-face { font-family: "X"; src: url("x.woff2") }
        "#,
    );
    assert_eq!(s.rules.len(), 0, "@font-face must not appear as a Rule");
    assert_eq!(s.font_faces.len(), 1);
}

#[test]
fn font_source_default_is_all_none() {
    let f = FontSource::default();
    assert_eq!(f.url, None);
    assert_eq!(f.format_hint, None);
    assert_eq!(f.local_name, None);
}

#[test]
fn malformed_at_font_face_with_no_descriptors_is_dropped() {
    let mut s = Stylesheet::new();
    s.parse_block("@font-face {}");
    assert_eq!(s.font_faces.len(), 0);
}

#[test]
fn at_import_still_works_alongside_at_font_face() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
        @import url("base.css");
        @font-face { font-family: "X"; src: url("x.woff2") }
        @import "extra.css";
        "#,
    );
    assert_eq!(s.imports.len(), 2);
    assert_eq!(s.imports[0], "base.css");
    assert_eq!(s.imports[1], "extra.css");
    assert_eq!(s.font_faces.len(), 1);
}
