//! Round 4 — minimal CSS cascade.
//!
//! Parses two surfaces that real SVG editors emit:
//!
//! 1. `<style>` blocks (typically inside `<defs>`). Bodies hold rules of
//!    the form `selector { decl-list; }` where `decl-list` is a `;`-
//!    separated list of `prop: value` pairs. Comments (`/* ... */`),
//!    `@`-rules, and pseudo-classes are stripped/ignored — round 4
//!    targets the basic set used by Inkscape / Illustrator / Figma SVG
//!    output.
//!
//! 2. `style="..."` attributes. The value is a `;`-separated list of
//!    `prop: value` pairs that take precedence over both the parent's
//!    inherited state and the element's own presentation attributes
//!    (per SVG 1.1 §6.1: an attribute defined as a CSS property MAY be
//!    overridden by a `style=` declaration).
//!
//! Selector subset:
//!
//! - Tag selectors: `rect`, `circle`, `g`, …
//! - Class selectors: `.foo`
//! - Id selectors: `#bar`
//! - Universal: `*`
//! - Comma lists: `a, b, .c`
//!
//! Combinators (descendant, child, sibling) and pseudo-classes are
//! deferred. Specificity follows CSS2.1 §6.4.3 (id > class > tag).
//!
//! Resolution model: at parse time the decoder walks `<style>` blocks
//! and stores rules in [`Stylesheet`]. When a presentation attribute
//! resolves on an element, the cascade pulls matching rules in
//! specificity order, then layers the inline `style="..."` on top.

use crate::parser::{attr, tag_local, Element, Node as XmlNode};

/// One parsed CSS rule.
#[derive(Clone, Debug, Default)]
pub struct Rule {
    /// Comma-separated selector list, parsed into compound selectors.
    pub selectors: Vec<Selector>,
    /// `(prop, value)` declarations in source order.
    pub declarations: Vec<(String, String)>,
}

/// One simple compound selector (no combinators).
#[derive(Clone, Debug, Default)]
pub struct Selector {
    /// `Some("rect")` for tag-name match, `None` for `*` or class/id-only
    /// selectors.
    pub tag: Option<String>,
    /// Required class names (all must match).
    pub classes: Vec<String>,
    /// Required id (at most one — multiple `#a#b` selectors don't match
    /// any element so we collapse them).
    pub id: Option<String>,
}

impl Selector {
    /// CSS2.1 §6.4.3 specificity tuple — `(id, class, tag)`. Larger
    /// triples win; ties broken by source order at the call site.
    pub fn specificity(&self) -> (u32, u32, u32) {
        (
            self.id.as_ref().map_or(0, |_| 1),
            self.classes.len() as u32,
            self.tag.as_ref().map_or(0, |_| 1),
        )
    }

    /// `true` when the selector matches `el` (tag, every class, and the
    /// id all align). Class lists from the element are split on
    /// whitespace per HTML/SVG convention.
    pub fn matches(&self, el: &Element) -> bool {
        if let Some(tag) = &self.tag {
            if !tag_local(&el.name).eq_ignore_ascii_case(tag) {
                return false;
            }
        }
        if let Some(want_id) = &self.id {
            match attr(el, "id") {
                Some(have) if have == want_id => {}
                _ => return false,
            }
        }
        if !self.classes.is_empty() {
            let class_attr = attr(el, "class").unwrap_or("");
            let have: Vec<&str> = class_attr.split_whitespace().collect();
            for c in &self.classes {
                if !have.iter().any(|h| *h == c) {
                    return false;
                }
            }
        }
        true
    }
}

/// Parsed CSS rules collected from every `<style>` block in the
/// document. Built by the decoder during the pre-walk and consumed by
/// `merged_with` when resolving each element's presentation attrs.
#[derive(Clone, Debug, Default)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
}

impl Stylesheet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse one `<style>` block body and append the rules to `self`.
    /// Malformed rules are silently dropped (matches browser tolerance —
    /// breaking the whole document on a typo would surprise users).
    pub fn parse_block(&mut self, css: &str) {
        let stripped = strip_comments(css);
        let mut i = 0;
        let bytes = stripped.as_bytes();
        while i < bytes.len() {
            // Skip leading whitespace.
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= bytes.len() {
                break;
            }
            // Skip @-rules — find the matching `;` or `}`.
            if bytes[i] == b'@' {
                let mut depth = 0u32;
                while i < bytes.len() {
                    match bytes[i] {
                        b'{' => depth += 1,
                        b'}' => {
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        b';' if depth == 0 => {
                            i += 1;
                            break;
                        }
                        _ => {}
                    }
                    i += 1;
                }
                continue;
            }
            // Find the `{`.
            let sel_start = i;
            while i < bytes.len() && bytes[i] != b'{' {
                i += 1;
            }
            if i >= bytes.len() {
                break;
            }
            let selector_text = &stripped[sel_start..i];
            i += 1; // skip `{`
            let body_start = i;
            // Find matching `}` (no nested braces in plain CSS rules).
            while i < bytes.len() && bytes[i] != b'}' {
                i += 1;
            }
            let body_end = i.min(bytes.len());
            let body = &stripped[body_start..body_end];
            if i < bytes.len() {
                i += 1; // skip `}`
            }
            let selectors = parse_selector_list(selector_text);
            let declarations = parse_declarations(body);
            if selectors.is_empty() || declarations.is_empty() {
                continue;
            }
            self.rules.push(Rule {
                selectors,
                declarations,
            });
        }
    }

    /// Return every declaration matching `el`, sorted ascending by
    /// specificity (caller layers them in order — last wins, ties go to
    /// source order). The pairs are owned because the caller often
    /// stores them past the borrow of `el`.
    pub fn matched_declarations(&self, el: &Element) -> Vec<(String, String)> {
        // Tuple `(specificity_id, specificity_class, specificity_tag,
        // rule_idx)` is sortable as-is — lower specificity comes
        // first, ties broken by source order.
        type SortKey = (u32, u32, u32, usize);
        let mut hits: Vec<(SortKey, &(String, String))> = Vec::new();
        for (rule_idx, rule) in self.rules.iter().enumerate() {
            // Find the *highest-specificity* selector in the rule that
            // matches — CSS treats each rule's selectors independently,
            // so a rule with `a, .b` matches with the higher of the
            // two specificities for any given element.
            let mut best: Option<(u32, u32, u32)> = None;
            for sel in &rule.selectors {
                if sel.matches(el) {
                    let s = sel.specificity();
                    let take = match best {
                        None => true,
                        Some(b) => s > b,
                    };
                    if take {
                        best = Some(s);
                    }
                }
            }
            if let Some(spec) = best {
                for decl in &rule.declarations {
                    hits.push(((spec.0, spec.1, spec.2, rule_idx), decl));
                }
            }
        }
        // Stable sort by (specificity, source order). Same key →
        // preserves the original order which is what we want for ties.
        hits.sort_by_key(|(k, _)| *k);
        hits.into_iter()
            .map(|(_, d)| (d.0.clone(), d.1.clone()))
            .collect()
    }
}

/// Parse the contents of a `style="..."` attribute into a list of
/// `(name, value)` pairs.
pub fn parse_inline_style(s: &str) -> Vec<(String, String)> {
    parse_declarations(s)
}

fn parse_declarations(body: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for piece in body.split(';') {
        let p = piece.trim();
        if p.is_empty() {
            continue;
        }
        let colon = match p.find(':') {
            Some(c) => c,
            None => continue,
        };
        let name = p[..colon].trim();
        let value = p[colon + 1..].trim();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        // Strip a trailing `!important` flag — round 4 doesn't model
        // !important specificity but we want to capture the value.
        let cleaned = value.trim_end_matches(|c: char| c.is_whitespace());
        let cleaned = if let Some(stripped) = cleaned.strip_suffix("!important") {
            stripped.trim()
        } else {
            cleaned
        };
        out.push((name.to_ascii_lowercase(), cleaned.to_string()));
    }
    out
}

fn parse_selector_list(s: &str) -> Vec<Selector> {
    let mut out: Vec<Selector> = Vec::new();
    for piece in s.split(',') {
        let p = piece.trim();
        if p.is_empty() {
            continue;
        }
        // Drop everything after the first whitespace (no descendant
        // combinator support in round 4). Drop everything after the
        // first `:` (no pseudo-class support).
        let head = p.split_whitespace().next().unwrap_or("");
        let head = head.split(':').next().unwrap_or("");
        if head.is_empty() {
            continue;
        }
        let mut sel = Selector::default();
        let mut i = 0;
        let bytes = head.as_bytes();
        // Optional tag at the start: must begin with `*` or an ASCII
        // alpha. Anything else (`#`, `.`) starts the class/id parse
        // immediately.
        if bytes[0] == b'*' {
            i = 1;
        } else if bytes[0].is_ascii_alphabetic() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
                i += 1;
            }
            sel.tag = Some(head[start..i].to_ascii_lowercase());
        }
        // Class / id segments. Multiple `.c` are accepted; multiple
        // `#a` collapse the selector to "no match" by the implementation
        // (we just keep the first id).
        while i < bytes.len() {
            let kind = bytes[i];
            i += 1;
            let start = i;
            while i < bytes.len()
                && bytes[i] != b'.'
                && bytes[i] != b'#'
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_')
            {
                i += 1;
            }
            let token = &head[start..i];
            if token.is_empty() {
                continue;
            }
            match kind {
                b'.' => sel.classes.push(token.to_string()),
                b'#' if sel.id.is_none() => sel.id = Some(token.to_string()),
                _ => {}
            }
        }
        out.push(sel);
    }
    out
}

fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Walk an XML tree and collect every `<style>` block's body into the
/// supplied [`Stylesheet`]. The body lives in the element's text
/// children (CDATA-wrapped or bare).
pub fn collect_stylesheet(root: &Element, sheet: &mut Stylesheet) {
    fn walk(el: &Element, sheet: &mut Stylesheet) {
        if tag_local(&el.name).eq_ignore_ascii_case("style") {
            let mut body = String::new();
            for child in &el.children {
                if let XmlNode::Text(t) = child {
                    body.push_str(t);
                }
            }
            sheet.parse_block(&body);
        }
        for child in &el.children {
            if let XmlNode::Element(c) = child {
                walk(c, sheet);
            }
        }
    }
    walk(root, sheet);
}

/// Return the effective declaration list for `el` — the sheet's
/// matched-by-specificity declarations followed by the element's inline
/// `style="..."`. Caller iterates the result in order, last write wins.
pub fn declarations_for(el: &Element, sheet: &Stylesheet) -> Vec<(String, String)> {
    let mut out = sheet.matched_declarations(el);
    if let Some(s) = attr(el, "style") {
        out.extend(parse_inline_style(s));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Element;

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

    #[test]
    fn parse_simple_class_rule() {
        let mut s = Stylesheet::new();
        s.parse_block(".big { fill: #ff0000; stroke-width: 4 }");
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].selectors[0].classes, vec!["big".to_string()]);
        assert_eq!(s.rules[0].declarations.len(), 2);
    }

    #[test]
    fn parse_id_and_tag_selectors() {
        let mut s = Stylesheet::new();
        s.parse_block("#main { fill: blue } rect { stroke: black }");
        assert_eq!(s.rules.len(), 2);
        let r = &s.rules[0];
        assert_eq!(r.selectors[0].id, Some("main".into()));
        let r = &s.rules[1];
        assert_eq!(r.selectors[0].tag, Some("rect".into()));
    }

    #[test]
    fn parse_comma_list() {
        let mut s = Stylesheet::new();
        s.parse_block("a, .b, #c { opacity: 0.5 }");
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].selectors.len(), 3);
    }

    #[test]
    fn comments_stripped() {
        let mut s = Stylesheet::new();
        s.parse_block("/* fake */ .x /* mid */ { /* inner */ fill: #abc /* trail */ }");
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].declarations[0].0, "fill");
    }

    #[test]
    fn at_rule_skipped() {
        let mut s = Stylesheet::new();
        s.parse_block("@media print { rect { fill: red } } .x { fill: blue }");
        assert_eq!(s.rules.len(), 1, "only the .x rule should make it");
        assert_eq!(s.rules[0].selectors[0].classes, vec!["x".to_string()]);
    }

    #[test]
    fn matches_class_and_id() {
        let mut sheet = Stylesheet::new();
        sheet.parse_block(".foo { fill: red } #bar { fill: blue }");
        let el = elem("rect", &[("class", "foo"), ("id", "bar")]);
        let decls = sheet.matched_declarations(&el);
        // Both rules match — id wins because of higher specificity, so
        // its declaration comes after .foo's in the output (last wins).
        assert_eq!(decls.len(), 2);
        let last = &decls[1];
        assert_eq!(last.1, "blue");
    }

    #[test]
    fn inline_style_overrides_sheet() {
        let mut sheet = Stylesheet::new();
        sheet.parse_block(".a { fill: red }");
        let el = elem("rect", &[("class", "a"), ("style", "fill: green")]);
        let decls = declarations_for(&el, &sheet);
        // `.a` then inline → last is inline.
        assert_eq!(decls.last().unwrap().1, "green");
    }

    #[test]
    fn specificity_order() {
        let id = Selector {
            id: Some("x".into()),
            ..Selector::default()
        };
        let cls = Selector {
            classes: vec!["x".into()],
            ..Selector::default()
        };
        let tag = Selector {
            tag: Some("x".into()),
            ..Selector::default()
        };
        assert!(id.specificity() > cls.specificity());
        assert!(cls.specificity() > tag.specificity());
    }

    #[test]
    fn important_flag_stripped() {
        let mut s = Stylesheet::new();
        s.parse_block(".x { fill: red !important }");
        assert_eq!(s.rules[0].declarations[0].1, "red");
    }

    #[test]
    fn collect_stylesheet_walks_nested() {
        let style = Element {
            name: "style".into(),
            attrs: vec![],
            children: vec![XmlNode::Text(".x { fill: red }".into())],
        };
        let defs = Element {
            name: "defs".into(),
            attrs: vec![],
            children: vec![XmlNode::Element(style)],
        };
        let svg = Element {
            name: "svg".into(),
            attrs: vec![],
            children: vec![XmlNode::Element(defs)],
        };
        let mut sheet = Stylesheet::new();
        collect_stylesheet(&svg, &mut sheet);
        assert_eq!(sheet.rules.len(), 1);
    }
}
