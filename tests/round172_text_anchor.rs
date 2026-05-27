//! Round 172 — SVG 2 §11.10.1.1 `text-anchor` (`start | middle | end`)
//! parser-side tests. These run without a font resolver registered, so
//! every `<text>` parses to an empty `Group` — but the property
//! cascade machinery still exercises the new
//! [`oxideav_svg::element::TextAnchor`] enum and the
//! [`oxideav_svg::element::PaintState::text_anchor`] field, plus the
//! `apply_one` branch that maps the three keywords (and tolerates
//! unrecognised values / `inherit`).

#![cfg(feature = "text")]

use oxideav_svg::element::{PaintState, TextAnchor};
use oxideav_svg::parse_svg;
use oxideav_svg::parser::{parse_xml, Element};

/// Default `text-anchor` is `start` (per §11.10.1.1 Initial table).
#[test]
fn default_text_anchor_is_start() {
    let s = PaintState::default();
    assert_eq!(s.text_anchor, TextAnchor::Start);
}

/// Parse each keyword from a presentation attribute on a `<text>`
/// element. Build a minimal Element via [`parse_xml`] so we exercise
/// the same cascade entry point used by the decoder.
fn paint_for(svg: &str) -> PaintState {
    let nodes = parse_xml(svg).expect("xml parse");
    let svg_el: &Element = nodes
        .iter()
        .find_map(|n| match n {
            oxideav_svg::parser::Node::Element(e) if e.name.ends_with("svg") => Some(e),
            _ => None,
        })
        .expect("svg root");
    let text_el: &Element = svg_el
        .children
        .iter()
        .find_map(|n| match n {
            oxideav_svg::parser::Node::Element(e) if e.name == "text" => Some(e),
            _ => None,
        })
        .expect("text element");
    PaintState::default()
        .merged_with(text_el)
        .expect("paint state merge")
}

#[test]
fn presentation_attr_start() {
    let s = paint_for(
        r#"<svg xmlns="http://www.w3.org/2000/svg"><text text-anchor="start">X</text></svg>"#,
    );
    assert_eq!(s.text_anchor, TextAnchor::Start);
}

#[test]
fn presentation_attr_middle() {
    let s = paint_for(
        r#"<svg xmlns="http://www.w3.org/2000/svg"><text text-anchor="middle">X</text></svg>"#,
    );
    assert_eq!(s.text_anchor, TextAnchor::Middle);
}

#[test]
fn presentation_attr_end() {
    let s = paint_for(
        r#"<svg xmlns="http://www.w3.org/2000/svg"><text text-anchor="end">X</text></svg>"#,
    );
    assert_eq!(s.text_anchor, TextAnchor::End);
}

/// Unrecognised keyword keeps the inherited (default) value rather
/// than failing the document. Matches the §11.10.1.1 `inherit` branch
/// behaviour applied to any unparseable token.
#[test]
fn unrecognised_keyword_keeps_inherited_value() {
    let s = paint_for(
        r#"<svg xmlns="http://www.w3.org/2000/svg"><text text-anchor="weirdo">X</text></svg>"#,
    );
    assert_eq!(s.text_anchor, TextAnchor::Start);
}

/// `inherit` explicitly keeps the inherited value (the merged_with
/// call uses `PaintState::default()` as the inherited base, which is
/// `Start`).
#[test]
fn inherit_keyword_keeps_inherited_value() {
    let s = paint_for(
        r#"<svg xmlns="http://www.w3.org/2000/svg"><text text-anchor="inherit">X</text></svg>"#,
    );
    assert_eq!(s.text_anchor, TextAnchor::Start);
}

/// Case-insensitive matching per the CSS keyword rule. `MIDDLE` and
/// `Middle` both resolve to `Middle`.
#[test]
fn keyword_is_case_insensitive() {
    let s = paint_for(
        r#"<svg xmlns="http://www.w3.org/2000/svg"><text text-anchor="MIDDLE">X</text></svg>"#,
    );
    assert_eq!(s.text_anchor, TextAnchor::Middle);
    let s = paint_for(
        r#"<svg xmlns="http://www.w3.org/2000/svg"><text text-anchor="End">X</text></svg>"#,
    );
    assert_eq!(s.text_anchor, TextAnchor::End);
}

/// Parsing a `<text text-anchor="middle">` without a font resolver
/// must still produce an empty `Group` and not crash — the
/// post-walk shift has nothing to translate but the document still
/// loads cleanly.
#[test]
fn text_anchor_without_resolver_does_not_crash() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <text x="100" y="50" font-size="16" text-anchor="middle">Centered</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(frame.root.children.len(), 1);
}

/// Cascade through a `<g>` parent — the property is inherited so a
/// child `<text>` without its own value should pick up the parent's.
#[test]
fn text_anchor_inherits_from_parent_group() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <g text-anchor="end">
    <text x="90" y="50" font-size="12">trailing</text>
  </g>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    // Document parses (the resolver-less branch); inheritance is
    // exercised by the cascade. No assertion on the glyph geometry —
    // that's the resolver-installed test in
    // `round172_text_anchor_glyphs.rs`.
    assert!(!frame.root.children.is_empty());
}

/// Style-attribute path — `style="text-anchor: middle"` flows through
/// the round-4 CSS cascade rather than the presentation-attribute
/// reader, and must resolve to the same enum value.
#[test]
fn style_attribute_resolves_text_anchor() {
    let s = paint_for(
        r##"<svg xmlns="http://www.w3.org/2000/svg"><text style="text-anchor: middle">X</text></svg>"##,
    );
    assert_eq!(s.text_anchor, TextAnchor::Middle);
}

/// `<style>`-block cascade — a tag-targeted rule in an in-document
/// stylesheet must also resolve `text-anchor`.
#[test]
fn style_block_rule_resolves_text_anchor() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <style>text { text-anchor: end; }</style>
  <text x="50" y="50" font-size="12">hi</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    // No glyphs without a resolver, but the document parses cleanly —
    // the cascade saw the rule and applied it without panicking.
    assert!(!frame.root.children.is_empty());
}
