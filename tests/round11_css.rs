//! Round 11 — CSS extensions deferred from rounds 8-10:
//!
//! - **Pseudo-elements** (`::before`, `::after`, `::first-letter`,
//!   `::first-line`) per CSS 3 §3.7. Selectors with a pseudo-element
//!   parse to a typed `PseudoElement` field on the carrier
//!   `SimpleSelector` and never match a real element (the
//!   pseudo-element is a synthesised box; live matching is up to a
//!   future renderer).
//! - **`@import`** of external stylesheets per CSS 2.1 §6.3. URLs are
//!   captured in `Stylesheet::imports`; loading the imported sheet is
//!   left to the caller.
//! - **Stateful pseudo-classes** (`:hover`, `:focus`, `:active`,
//!   `:checked`, `:visited`, `:link`, `:disabled`, `:enabled`) per
//!   Selectors L3 §6.6. They parse to a typed `Stateful` variant and
//!   never match in a static document — fixing the round-5 over-match
//!   bug where `.x:hover` collapsed to `.x` because the `:hover` was
//!   silently dropped.

use oxideav_svg::css::{Pseudo, PseudoElement, StatefulPseudo, Stylesheet};
use oxideav_svg::parser::Element;

fn elem(name: &str, attrs: &[(&str, &str)]) -> Element {
    Element {
        name: name.into(),
        attrs: attrs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        children: Vec::new(),
    }
}

// ---- pseudo-elements ----

#[test]
fn pseudo_element_before_recorded_on_selector() {
    let mut s = Stylesheet::new();
    s.parse_block("p::before { content: '* ' }");
    assert_eq!(s.rules.len(), 1);
    let head = &s.rules[0].selectors[0].head;
    assert_eq!(head.tag, Some("p".into()));
    assert_eq!(head.pseudo_element, Some(PseudoElement::Before));
}

#[test]
fn pseudo_element_after_recorded() {
    let mut s = Stylesheet::new();
    s.parse_block(".item::after { content: ',' }");
    assert_eq!(
        s.rules[0].selectors[0].head.pseudo_element,
        Some(PseudoElement::After)
    );
}

#[test]
fn pseudo_element_first_letter_first_line() {
    let mut s = Stylesheet::new();
    s.parse_block("p::first-letter { font-size: 200% }");
    assert_eq!(
        s.rules[0].selectors[0].head.pseudo_element,
        Some(PseudoElement::FirstLetter)
    );
    let mut s = Stylesheet::new();
    s.parse_block("p::first-line { color: red }");
    assert_eq!(
        s.rules[0].selectors[0].head.pseudo_element,
        Some(PseudoElement::FirstLine)
    );
}

#[test]
fn css21_legacy_single_colon_pseudo_element() {
    // `:before`, `:after`, `:first-letter`, `:first-line` — single
    // colon variant per CSS 2.1 §5.12.1. Must still resolve as a
    // pseudo-element so the rule never matches a real element.
    let mut s = Stylesheet::new();
    s.parse_block("h1:before { content: '★ ' } h1:after { content: ' ★' }");
    assert_eq!(s.rules.len(), 2);
    assert_eq!(
        s.rules[0].selectors[0].head.pseudo_element,
        Some(PseudoElement::Before)
    );
    assert_eq!(
        s.rules[1].selectors[0].head.pseudo_element,
        Some(PseudoElement::After)
    );
}

#[test]
fn pseudo_element_rule_does_not_apply_to_real_element() {
    let mut s = Stylesheet::new();
    s.parse_block("rect::before { fill: red }");
    let r = elem("rect", &[]);
    let mctx = oxideav_svg::css::MatchContext::root(&r);
    let decls = s.matched_declarations(&mctx);
    assert!(
        decls.is_empty(),
        "pseudo-element rule must not match a live element, got {decls:?}"
    );
}

#[test]
fn pseudo_element_carries_one_tag_specificity_point() {
    let mut s = Stylesheet::new();
    s.parse_block("p::before { fill: red }");
    let head = &s.rules[0].selectors[0].head;
    let (i, c, t) = head.specificity();
    // tag p (1) + ::before (1) = 2 in the tag bucket.
    assert_eq!((i, c, t), (0, 0, 2));
}

#[test]
fn unknown_pseudo_element_dropped_silently() {
    // `::placeholder`, `::selection`, etc. — the keyword is dropped
    // but the rest of the rule survives.
    let mut s = Stylesheet::new();
    s.parse_block("input::placeholder { color: gray }");
    assert_eq!(s.rules.len(), 1);
    assert_eq!(s.rules[0].selectors[0].head.pseudo_element, None);
    assert_eq!(s.rules[0].selectors[0].head.tag, Some("input".into()));
}

// ---- stateful pseudo-classes ----

#[test]
fn hover_pseudo_does_not_overmatch_in_static_doc() {
    // Round-5 bug: `:hover` was dropped, so `.x:hover` collapsed to
    // `.x`. Round 11 fixes this by recording `:hover` as a Stateful
    // pseudo-class that never matches statically.
    let mut s = Stylesheet::new();
    s.parse_block(".x:hover { fill: red } .x { stroke: blue }");
    let el = elem("rect", &[("class", "x")]);
    let mctx = oxideav_svg::css::MatchContext::root(&el);
    let decls = s.matched_declarations(&mctx);
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0], ("stroke".into(), "blue".into()));
}

#[test]
fn all_eight_stateful_pseudo_classes_parse() {
    let mut s = Stylesheet::new();
    s.parse_block(
        "a:hover { x: 1 }
         a:focus { x: 1 }
         a:active { x: 1 }
         a:visited { x: 1 }
         a:link { x: 1 }
         input:checked { x: 1 }
         input:disabled { x: 1 }
         input:enabled { x: 1 }",
    );
    assert_eq!(s.rules.len(), 8);
    let want = [
        StatefulPseudo::Hover,
        StatefulPseudo::Focus,
        StatefulPseudo::Active,
        StatefulPseudo::Visited,
        StatefulPseudo::Link,
        StatefulPseudo::Checked,
        StatefulPseudo::Disabled,
        StatefulPseudo::Enabled,
    ];
    for (i, w) in want.iter().enumerate() {
        match &s.rules[i].selectors[0].head.pseudos[0] {
            Pseudo::Stateful(got) => assert_eq!(got, w),
            other => panic!("rule {i}: expected Stateful({w:?}), got {other:?}"),
        }
    }
}

#[test]
fn stateful_pseudo_inside_not_is_rejected() {
    // `:not(:hover)` — round 5 banned nested `:not`, but `:not(simple)`
    // with a stateful pseudo inside is OK at parse time. The Stateful
    // variant is "never matches" so `:not(:hover)` matches everything.
    let mut s = Stylesheet::new();
    s.parse_block("a:not(:hover) { fill: red }");
    assert_eq!(s.rules.len(), 1);
    let el = elem("a", &[]);
    let mctx = oxideav_svg::css::MatchContext::root(&el);
    let decls = s.matched_declarations(&mctx);
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].1, "red");
}

// ---- @import ----

#[test]
fn at_import_url_form_records_the_url() {
    let mut s = Stylesheet::new();
    s.parse_block(r#"@import url("brand.css") screen;"#);
    assert_eq!(s.imports, vec!["brand.css".to_string()]);
}

#[test]
fn at_import_bare_string_form_records_the_url() {
    let mut s = Stylesheet::new();
    s.parse_block(r#"@import "fallback.css";"#);
    assert_eq!(s.imports, vec!["fallback.css".to_string()]);
}

#[test]
fn at_import_supports_url_without_quotes() {
    let mut s = Stylesheet::new();
    s.parse_block(r#"@import url(plain.css);"#);
    assert_eq!(s.imports, vec!["plain.css".to_string()]);
}

#[test]
fn at_import_records_multiple_in_source_order() {
    let mut s = Stylesheet::new();
    s.parse_block(
        r#"
          @import url("a.css");
          @import "b.css" print;
          .x { fill: red }
          @import url(c.css) screen and (min-width: 600px);
        "#,
    );
    assert_eq!(
        s.imports,
        vec![
            "a.css".to_string(),
            "b.css".to_string(),
            "c.css".to_string()
        ]
    );
    assert_eq!(s.rules.len(), 1);
}

#[test]
fn at_media_block_is_not_recorded_as_an_import() {
    // `@media` opens a `{ ... }` block — must skip, not record.
    let mut s = Stylesheet::new();
    s.parse_block("@media print { .x { fill: red } }");
    assert!(s.imports.is_empty());
    assert_eq!(s.rules.len(), 0); // @media's interior is skipped per round 5
}

#[test]
fn at_import_inside_style_element_is_collected() {
    use oxideav_svg::css::collect_stylesheet;
    use oxideav_svg::parser::Node;
    let style = Element {
        name: "style".into(),
        attrs: vec![],
        children: vec![Node::Text(
            r#"@import url("theme.css"); .x { fill: red }"#.into(),
        )],
    };
    let svg = Element {
        name: "svg".into(),
        attrs: vec![],
        children: vec![Node::Element(style)],
    };
    let mut sheet = Stylesheet::new();
    collect_stylesheet(&svg, &mut sheet);
    assert_eq!(sheet.imports, vec!["theme.css".to_string()]);
    assert_eq!(sheet.rules.len(), 1);
}
