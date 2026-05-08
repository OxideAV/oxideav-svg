//! CSS cascade — round 4 (CSS 2.1 baseline) extended in round 5 with a
//! CSS 3 Selectors Level 3 subset (W3C REC-css3-selectors), and in round
//! 11 with pseudo-element parsing (`::before` / `::after` /
//! `::first-letter` / `::first-line`), `@import` URL capture (CSS 2.1
//! §6.3), and stateful pseudo-classes (`:hover` / `:focus` / `:checked`
//! / `:active` / `:visited` / `:link` / `:disabled` / `:enabled`) that
//! parse + survive the round-trip but never match in a static document.
//!
//! Parses two surfaces that real SVG editors emit:
//!
//! 1. `<style>` blocks (typically inside `<defs>`). Bodies hold rules of
//!    the form `selector { decl-list; }` where `decl-list` is a `;`-
//!    separated list of `prop: value` pairs. Comments (`/* ... */`),
//!    `@`-rules, and unknown pseudo-classes are stripped/ignored.
//!
//! 2. `style="..."` attributes. The value is a `;`-separated list of
//!    `prop: value` pairs that take precedence over both the parent's
//!    inherited state and the element's own presentation attributes
//!    (per SVG 1.1 §6.1: an attribute defined as a CSS property MAY be
//!    overridden by a `style=` declaration).
//!
//! # Round 5 selector subset (CSS3 Selectors Level 3)
//!
//! - Type / tag (`rect`), class (`.foo`), id (`#bar`), universal (`*`).
//! - Attribute predicates: `[attr]`, `[attr=val]`, `[attr~=val]`,
//!   `[attr|=val]`, `[attr^=val]`, `[attr$=val]`, `[attr*=val]`. Quoted
//!   values (`"..."` / `'...'`) are unwrapped. Namespace-prefixed
//!   attribute names (`xlink:href`) are honoured verbatim.
//! - Combinators: descendant (` `), child (`>`), adjacent sibling
//!   (`+`), general sibling (`~`).
//! - Structural pseudo-classes: `:first-child`, `:last-child`,
//!   `:only-child`, `:nth-child(n)` and variants (`odd` / `even` /
//!   `An+B` form), `:first-of-type`, `:last-of-type`, `:only-of-type`,
//!   `:nth-of-type(n)`, `:nth-last-child(n)`, `:nth-last-of-type(n)`,
//!   `:not(simple)` (parameter is a single simple selector). Round 6
//!   adds `:lang(L)` (BCP 47 dash-match against the element's nearest
//!   `xml:lang` / `lang` attribute, walked up the ancestor chain).
//! - **Round 11** — interactive pseudo-classes (`:hover`, `:focus`,
//!   `:active`, `:checked`, `:visited`, `:link`, `:disabled`,
//!   `:enabled`) parse to [`Pseudo::Stateful`] and survive the
//!   round-trip but never match in a static rendering. Previously these
//!   were silently dropped, which over-matched their carrier rules
//!   (`.x:hover` collapsed to `.x`).
//! - Comma-separated selector lists, with each component being a
//!   compound selector chain.
//!
//! # Specificity (CSS3 §9 / equivalent to CSS2.1 §6.4.3 with attribute
//! and pseudo-class additions)
//!
//! `(id_count, class_count + attr_count + pseudo_count, tag_count)` —
//! larger triples win; ties broken by source order at the call site.
//! `:not(X)` contributes the specificity of `X` per spec §6.6.7. The
//! universal (`*`) selector contributes nothing.
//!
//! # Round 11 additions — pseudo-elements + `@import`
//!
//! - **Pseudo-elements** (`::before`, `::after`, `::first-letter`,
//!   `::first-line`) parse to [`PseudoElement`] and are recorded on the
//!   carrier selector's [`SimpleSelector::pseudo_element`] field. A
//!   selector with a pseudo-element never matches a real DOM element
//!   directly (per CSS, the pseudo-element is a synthesised box) — but
//!   the rule is preserved in the [`Stylesheet`] so a future renderer
//!   can synthesise the box.  Specificity per CSS3 §9: each pseudo-
//!   element contributes one tag-level point.
//! - **`@import url(…) [media-query-list];`** (CSS 2.1 §6.3) — the URL
//!   string is appended to [`Stylesheet::imports`]. Loading the
//!   imported sheet is up to the caller (we deliberately don't fetch
//!   external resources from the parser).

use crate::parser::{attr, tag_local, Element, Node as XmlNode};

/// One parsed CSS rule.
#[derive(Clone, Debug, Default)]
pub struct Rule {
    /// Comma-separated selector list, parsed into compound selectors.
    pub selectors: Vec<CompoundSelector>,
    /// `(prop, value)` declarations in source order.
    pub declarations: Vec<(String, String)>,
}

/// Combinator joining two simple-selector pieces in a compound chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combinator {
    /// `A B` — `B` is a descendant of `A`.
    Descendant,
    /// `A > B` — `B` is a direct child of `A`.
    Child,
    /// `A + B` — `B` immediately follows `A` (same parent, element
    /// siblings).
    AdjacentSibling,
    /// `A ~ B` — `B` is some later element sibling of `A`.
    GeneralSibling,
}

/// One attribute predicate inside a simple selector (`[attr=value]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttrPredicate {
    /// Attribute name. Compared case-insensitively per HTML; SVG attrs
    /// are typically already lowercase.
    pub name: String,
    pub op: AttrOp,
    /// Expected value (empty for `[attr]` existence-only).
    pub value: String,
}

/// Attribute-selector operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttrOp {
    /// `[attr]` — attribute exists.
    Exists,
    /// `[attr=val]` — exact match.
    Equals,
    /// `[attr~=val]` — whitespace-separated word match.
    Includes,
    /// `[attr|=val]` — exact or `val-` prefix (language tags).
    DashMatch,
    /// `[attr^=val]` — prefix match.
    StartsWith,
    /// `[attr$=val]` — suffix match.
    EndsWith,
    /// `[attr*=val]` — substring match.
    Contains,
}

/// CSS3 structural pseudo-classes that we model. Interactive
/// pseudo-classes (`:hover`, …) are silently dropped at parse time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pseudo {
    FirstChild,
    LastChild,
    OnlyChild,
    FirstOfType,
    LastOfType,
    OnlyOfType,
    /// `:nth-child(An+B)` — `(a, b)` where the index `i` (1-indexed)
    /// matches when there exists a non-negative integer `n` with
    /// `i = a*n + b`.
    NthChild(i32, i32),
    NthOfType(i32, i32),
    /// `:nth-last-child(An+B)` — same as [`Pseudo::NthChild`] but the
    /// index is counted from the *end* of the parent's element-children
    /// list (the last child has index 1). Round 6.
    NthLastChild(i32, i32),
    /// `:nth-last-of-type(An+B)` — same as [`Pseudo::NthOfType`] but the
    /// of-type index is counted from the end. Round 6.
    NthLastOfType(i32, i32),
    /// `:lang(prefix)` — matches when the element's effective BCP 47
    /// language tag (the nearest `xml:lang` / `lang` attribute on the
    /// element or any ancestor) equals `prefix` exactly, or starts with
    /// `prefix-` (Selectors L3 §6.6.2 + CSS dash-match rule). Round 6.
    Lang(String),
    /// `:not(simple)` — negation of a single simple selector.
    Not(Box<SimpleSelector>),
    /// **Round 11** — stateful / interactive pseudo-classes (`:hover`,
    /// `:focus`, `:active`, `:checked`, `:visited`, `:link`,
    /// `:disabled`, `:enabled`). These parse but never match in a
    /// static document — recorded so the rule survives round-trip and
    /// a future interactive consumer can re-evaluate.
    Stateful(StatefulPseudo),
}

/// Stateful (interactive) pseudo-classes recognised in round 11. None
/// match in a static document — a future interactive renderer would
/// flip the relevant flag at runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatefulPseudo {
    /// `:hover` — pointing device is over the element.
    Hover,
    /// `:focus` — element has keyboard / programmatic focus.
    Focus,
    /// `:active` — element is being activated (mouse-down).
    Active,
    /// `:checked` — checkbox / radio / option in a checked state.
    Checked,
    /// `:visited` — visited hyperlink.
    Visited,
    /// `:link` — unvisited hyperlink (`<a href>` / `<area href>`).
    Link,
    /// `:disabled` — form control in a disabled state.
    Disabled,
    /// `:enabled` — form control in an enabled state (default).
    Enabled,
}

/// **Round 11** — a CSS pseudo-element (`::before`, `::after`, …).
/// Distinct from a pseudo-class in that the colon-pair syntax targets a
/// synthesised renderer-only box rather than an existing element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PseudoElement {
    /// `::before` — content box generated before the element's first
    /// child.
    Before,
    /// `::after` — content box generated after the element's last
    /// child.
    After,
    /// `::first-letter` — the first typographic letter of a block.
    FirstLetter,
    /// `::first-line` — the first formatted line of a block.
    FirstLine,
}

impl PseudoElement {
    fn from_str(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "before" => Some(Self::Before),
            "after" => Some(Self::After),
            "first-letter" => Some(Self::FirstLetter),
            "first-line" => Some(Self::FirstLine),
            _ => None,
        }
    }
}

/// One simple selector: optional tag + class list + id + attribute and
/// pseudo-class predicates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SimpleSelector {
    /// `Some("rect")` for tag-name match, `None` for `*` or
    /// class/id/attr-only selectors.
    pub tag: Option<String>,
    pub classes: Vec<String>,
    pub id: Option<String>,
    pub attrs: Vec<AttrPredicate>,
    pub pseudos: Vec<Pseudo>,
    /// **Round 11** — `::before` / `::after` / `::first-letter` /
    /// `::first-line` attached to this simple. CSS allows at most one
    /// pseudo-element per compound selector and it must come last; a
    /// rule with a pseudo-element never matches a live element
    /// directly (the renderer synthesises a box per CSS 3 §3.2).
    pub pseudo_element: Option<PseudoElement>,
}

/// A compound selector — one head simple-selector plus zero or more
/// `(combinator, simple)` pairs. Read left-to-right (`A > B + C`):
/// `head = A`, segments = `[(>, B), (+, C)]`. Matched right-to-left
/// against the candidate element + its ancestor chain.
#[derive(Clone, Debug, Default)]
pub struct CompoundSelector {
    pub head: SimpleSelector,
    pub segments: Vec<(Combinator, SimpleSelector)>,
}

/// Round-4 alias kept for source-compat (tests still spell `Selector`).
/// External callers should migrate to [`CompoundSelector`].
pub type Selector = CompoundSelector;

/// Per-element match context — required for combinators and structural
/// pseudo-classes. Built by the decoder during the tree walk.
///
/// `parent` chains via lifetime-tied references rather than allocating
/// a `Vec`, so a deep tree only costs a single pointer + index per
/// stack frame.
#[derive(Clone, Copy, Debug)]
pub struct MatchContext<'a> {
    /// The element being matched.
    pub el: &'a Element,
    /// Position among the parent's element-only children (0-indexed).
    pub child_index: usize,
    /// Position among element-children of the same tag in the parent
    /// (0-indexed).
    pub of_type_index: usize,
    /// Total element-only children in the parent.
    pub sibling_count: usize,
    /// Total element-only children of the same tag in the parent.
    pub of_type_count: usize,
    /// Parent context, or `None` at the document root.
    pub parent: Option<&'a MatchContext<'a>>,
}

impl<'a> MatchContext<'a> {
    /// Construct a root-level (no parent) context. Used by the decoder
    /// for the document `<svg>` element.
    pub fn root(el: &'a Element) -> Self {
        Self {
            el,
            child_index: 0,
            of_type_index: 0,
            sibling_count: 1,
            of_type_count: 1,
            parent: None,
        }
    }
}

impl SimpleSelector {
    /// Round-4 `(id, class, tag)` specificity, extended in round 5 with
    /// attribute and pseudo-class predicates per CSS3 §9. `:not(X)`
    /// folds in `X`'s specificity but `:not` itself doesn't add to the
    /// counts.
    pub fn specificity(&self) -> (u32, u32, u32) {
        let mut id = self.id.as_ref().map_or(0, |_| 1);
        let mut cls = self.classes.len() as u32 + self.attrs.len() as u32;
        let mut tag = self.tag.as_ref().map_or(0, |_| 1);
        for p in &self.pseudos {
            match p {
                Pseudo::Not(inner) => {
                    let s = inner.specificity();
                    id += s.0;
                    cls += s.1;
                    tag += s.2;
                }
                // Structural / stateful pseudo-classes count as a class
                // per CSS3 §9.
                _ => cls += 1,
            }
        }
        // CSS3 §9: each pseudo-element contributes one tag-level point.
        if self.pseudo_element.is_some() {
            tag += 1;
        }
        (id, cls, tag)
    }

    /// `true` when this simple selector matches `mctx.el`.
    pub fn matches(&self, mctx: &MatchContext<'_>) -> bool {
        // Round 11: a selector that targets a pseudo-element
        // (`::before`, `::after`, …) never matches a live DOM element
        // — the pseudo-element is a synthesised box. We still parse +
        // store the rule so a future renderer can apply it; the static
        // cascade simply skips it here.
        if self.pseudo_element.is_some() {
            return false;
        }
        let el = mctx.el;
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
        for ap in &self.attrs {
            if !ap.matches(el) {
                return false;
            }
        }
        for p in &self.pseudos {
            if !p.matches(mctx) {
                return false;
            }
        }
        true
    }
}

impl AttrPredicate {
    fn matches(&self, el: &Element) -> bool {
        let val = match attr(el, &self.name) {
            Some(v) => v,
            None => return false,
        };
        match self.op {
            AttrOp::Exists => true,
            AttrOp::Equals => val == self.value,
            AttrOp::Includes => {
                if self.value.is_empty() || self.value.contains(char::is_whitespace) {
                    return false;
                }
                val.split_whitespace().any(|w| w == self.value)
            }
            AttrOp::DashMatch => {
                val == self.value
                    || val
                        .strip_prefix(&self.value)
                        .is_some_and(|rest| rest.starts_with('-'))
            }
            AttrOp::StartsWith => !self.value.is_empty() && val.starts_with(&self.value),
            AttrOp::EndsWith => !self.value.is_empty() && val.ends_with(&self.value),
            AttrOp::Contains => !self.value.is_empty() && val.contains(&self.value),
        }
    }
}

impl Pseudo {
    fn matches(&self, mctx: &MatchContext<'_>) -> bool {
        match self {
            Pseudo::FirstChild => mctx.child_index == 0,
            Pseudo::LastChild => mctx.child_index + 1 == mctx.sibling_count,
            Pseudo::OnlyChild => mctx.sibling_count == 1,
            Pseudo::FirstOfType => mctx.of_type_index == 0,
            Pseudo::LastOfType => mctx.of_type_index + 1 == mctx.of_type_count,
            Pseudo::OnlyOfType => mctx.of_type_count == 1,
            Pseudo::NthChild(a, b) => nth_match(mctx.child_index as i32 + 1, *a, *b),
            Pseudo::NthOfType(a, b) => nth_match(mctx.of_type_index as i32 + 1, *a, *b),
            // 1-indexed from the end: last child is 1.
            Pseudo::NthLastChild(a, b) => {
                let last_idx = mctx.sibling_count.saturating_sub(mctx.child_index) as i32;
                nth_match(last_idx, *a, *b)
            }
            Pseudo::NthLastOfType(a, b) => {
                let last_idx = mctx.of_type_count.saturating_sub(mctx.of_type_index) as i32;
                nth_match(last_idx, *a, *b)
            }
            // Walk up the parent chain to find the nearest element with
            // an `xml:lang` / `lang` attribute, then dash-match it
            // against the prefix per Selectors L3 §6.6.2.
            Pseudo::Lang(prefix) => {
                let mut cur: Option<&MatchContext<'_>> = Some(mctx);
                while let Some(c) = cur {
                    if let Some(tag) = lang_attr(c.el) {
                        return lang_dash_match(tag, prefix);
                    }
                    cur = c.parent;
                }
                false
            }
            Pseudo::Not(inner) => !inner.matches(mctx),
            // Round 11: stateful pseudo-classes never match in a static
            // document. A `:hover` or `:checked` rule is preserved (so
            // the round-trip + interactive consumers see it) but the
            // static cascade skips it.
            Pseudo::Stateful(_) => false,
        }
    }
}

/// Look up the BCP 47 language tag on `el`. SVG inherits the
/// XML-namespace `xml:lang` attribute and (in HTML / SVG 2) the bare
/// `lang` attribute; either is honoured, with `xml:lang` taking
/// precedence when both are present.
fn lang_attr(el: &Element) -> Option<&str> {
    if let Some(v) = attr(el, "xml:lang") {
        return Some(v);
    }
    attr(el, "lang")
}

/// Selectors L3 §6.6.2: `:lang(C)` matches when the element's language
/// tag is `C` (case-insensitive) or starts with `C-` (case-insensitive).
fn lang_dash_match(have: &str, want: &str) -> bool {
    if have.eq_ignore_ascii_case(want) {
        return true;
    }
    let n = want.len();
    have.len() > n && have.as_bytes()[n] == b'-' && have[..n].eq_ignore_ascii_case(want)
}

/// `:nth-child(An+B)` — CSS3 §6.6.5: matches when there exists a
/// non-negative integer `n` with `index = a*n + b`. `index` is
/// 1-based. Negative `a` produces a finite set whose largest element
/// is `b`.
fn nth_match(index: i32, a: i32, b: i32) -> bool {
    if a == 0 {
        return index == b;
    }
    let diff = index - b;
    // n = diff / a; require n >= 0 and a*n == diff.
    if diff == 0 {
        return true;
    }
    if a > 0 {
        diff > 0 && diff % a == 0
    } else {
        diff < 0 && diff % a == 0
    }
}

impl CompoundSelector {
    pub fn specificity(&self) -> (u32, u32, u32) {
        let mut s = self.head.specificity();
        for (_, simple) in &self.segments {
            let t = simple.specificity();
            s.0 += t.0;
            s.1 += t.1;
            s.2 += t.2;
        }
        s
    }

    /// Right-to-left match: the *rightmost* simple in the chain is the
    /// candidate (`mctx.el`); for each `(combinator, prev)` pair walking
    /// leftwards the matcher finds an ancestor / sibling that satisfies
    /// `prev`.
    pub fn matches(&self, mctx: &MatchContext<'_>) -> bool {
        // Build the chain in right-to-left order: the rightmost is the
        // candidate (`head` + appended segments); read backwards from
        // the segments list — the *last* segment is the rightmost.
        // Layout: head, seg[0]=(c0, s0), seg[1]=(c1, s1), …, seg[n]=(cn, sn).
        // Chain reads: head c0 s0 c1 s1 … cn sn.
        // The rightmost simple is `sn` (or `head` if no segments).
        // Match `sn` against mctx.el; then for each i in (n..0) match
        // s[i-1] against an ancestor / sibling reached via combinator
        // c[i].
        let (rightmost, lefts) = match self.segments.last() {
            None => (&self.head, Vec::<(&Combinator, &SimpleSelector)>::new()),
            Some(_) => {
                let mut v: Vec<(&Combinator, &SimpleSelector)> = Vec::new();
                let last_idx = self.segments.len() - 1;
                let rightmost = &self.segments[last_idx].1;
                // For i from last_idx down to 1: combinator at
                // segments[i].0 connects segments[i-1].1 (left) to
                // segments[i].1 (right).
                for i in (1..=last_idx).rev() {
                    v.push((&self.segments[i].0, &self.segments[i - 1].1));
                }
                // The first segment's combinator connects head (left)
                // to segments[0].1 (right).
                v.push((&self.segments[0].0, &self.head));
                (rightmost, v)
            }
        };
        if !rightmost.matches(mctx) {
            return false;
        }
        // Walk left-side selectors in order, threading `cursor`.
        let mut cursor: MatchContext<'_> = *mctx;
        for (combo, left) in lefts {
            if let Some(next) = find_match(&cursor, combo, left) {
                cursor = next;
            } else {
                return false;
            }
        }
        true
    }
}

/// Given a cursor and a left-side simple, find a `MatchContext` that
/// satisfies the combinator → left transition. For descendant we walk
/// up parents; for child we step once; for sibling combinators we walk
/// previous element-siblings via the parent's element-children list.
///
/// The returned context's `parent` pointer is copied from the cursor's
/// own parent chain, so the lifetimes line up with the original
/// MatchContext stack rather than with `cursor`.
fn find_match<'a>(
    cursor: &MatchContext<'a>,
    combo: &Combinator,
    left: &SimpleSelector,
) -> Option<MatchContext<'a>> {
    match combo {
        Combinator::Child => {
            let p = cursor.parent.copied()?;
            if left.matches(&p) {
                Some(p)
            } else {
                None
            }
        }
        Combinator::Descendant => {
            let mut p = cursor.parent;
            while let Some(ctx) = p {
                if left.matches(ctx) {
                    return Some(*ctx);
                }
                p = ctx.parent;
            }
            None
        }
        Combinator::AdjacentSibling | Combinator::GeneralSibling => {
            // Find an *element*-sibling earlier than `cursor` in the
            // parent's children. Build a fresh MatchContext for that
            // sibling so further combinator hops have correct indices.
            let parent_ctx = cursor.parent.copied()?;
            let parent_el = parent_ctx.el;
            let target_idx = cursor.child_index;
            // Collect element children + their indexing info.
            let mut prior: Vec<(&'a Element, usize)> = Vec::new();
            let mut elem_idx = 0usize;
            for child in &parent_el.children {
                if let XmlNode::Element(e) = child {
                    if elem_idx >= target_idx {
                        break;
                    }
                    prior.push((e, elem_idx));
                    elem_idx += 1;
                }
            }
            // For adjacent: only the immediately-preceding sibling.
            // For general: any previous sibling, walked nearest-first.
            let want_adjacent = matches!(combo, Combinator::AdjacentSibling);
            for (sib, sib_idx) in prior.iter().rev() {
                let (of_idx, _) = sibling_info(parent_el, sib, *sib_idx);
                let ctx = MatchContext {
                    el: sib,
                    child_index: *sib_idx,
                    of_type_index: of_idx,
                    sibling_count: parent_ctx.sibling_count,
                    of_type_count: of_type_count(parent_el, sib),
                    parent: parent_ctx.parent,
                };
                if left.matches(&ctx) {
                    return Some(ctx);
                }
                if want_adjacent {
                    break;
                }
            }
            None
        }
    }
}

/// Compute `(of_type_index, _)` for the given element child of `parent`.
fn sibling_info(parent: &Element, target: &Element, target_idx: usize) -> (usize, usize) {
    let target_tag = tag_local(&target.name).to_ascii_lowercase();
    let mut of_type = 0usize;
    let mut elem_idx = 0usize;
    for child in &parent.children {
        if let XmlNode::Element(e) = child {
            if elem_idx == target_idx {
                return (of_type, 0);
            }
            if tag_local(&e.name).eq_ignore_ascii_case(&target_tag) {
                of_type += 1;
            }
            elem_idx += 1;
        }
    }
    (of_type, 0)
}

/// Total element-children of `parent` with the same tag as `target`.
fn of_type_count(parent: &Element, target: &Element) -> usize {
    let target_tag = tag_local(&target.name);
    parent
        .children
        .iter()
        .filter(|c| match c {
            XmlNode::Element(e) => tag_local(&e.name).eq_ignore_ascii_case(&target_tag),
            _ => false,
        })
        .count()
}

/// Parsed CSS rules collected from every `<style>` block in the
/// document. Built by the decoder during the pre-walk and consumed by
/// `merged_with` when resolving each element's presentation attrs.
#[derive(Clone, Debug, Default)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    /// **Round 11** — `@import` URLs in source order (CSS 2.1 §6.3).
    /// Loading the imported sheet is left to the caller; we don't
    /// fetch external resources from the parser. Quotes / `url(...)`
    /// wrapping are stripped.
    pub imports: Vec<String>,
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
            // @-rules — capture `@import url(...) [media];` per CSS
            // 2.1 §6.3 into `imports`; skip every other @-rule
            // (`@media`, `@font-face`, `@keyframes`, …).
            if bytes[i] == b'@' {
                let at_start = i;
                let mut depth = 0u32;
                let mut had_block = false;
                let mut end = i;
                while i < bytes.len() {
                    match bytes[i] {
                        b'{' => {
                            had_block = true;
                            depth += 1;
                        }
                        b'}' => {
                            if depth == 0 {
                                end = i + 1;
                                i += 1;
                                break;
                            }
                            depth -= 1;
                            if depth == 0 {
                                end = i + 1;
                                i += 1;
                                break;
                            }
                        }
                        b';' if depth == 0 => {
                            end = i;
                            i += 1;
                            break;
                        }
                        _ => {}
                    }
                    i += 1;
                }
                if !had_block {
                    let raw = &stripped[at_start..end];
                    if let Some(url) = parse_at_import(raw) {
                        self.imports.push(url);
                    }
                }
                continue;
            }
            // Find the `{` — but skip past `{` characters that appear
            // inside `[...]` attribute selectors. (Unlikely in practice
            // but keeps the parser honest.)
            let sel_start = i;
            let mut bracket_depth = 0u32;
            while i < bytes.len() {
                match bytes[i] {
                    b'[' => bracket_depth += 1,
                    b']' if bracket_depth > 0 => bracket_depth -= 1,
                    b'{' if bracket_depth == 0 => break,
                    _ => {}
                }
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

    /// Return every declaration matching `mctx.el`, sorted ascending by
    /// specificity (caller layers them in order — last wins, ties go to
    /// source order). The pairs are owned because the caller often
    /// stores them past the borrow of `el`.
    pub fn matched_declarations(&self, mctx: &MatchContext<'_>) -> Vec<(String, String)> {
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
                if sel.matches(mctx) {
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

/// Parse a comma-separated selector list (`a, .b, rect > circle`) into
/// one [`CompoundSelector`] per comma-separated piece.
fn parse_selector_list(s: &str) -> Vec<CompoundSelector> {
    let mut out: Vec<CompoundSelector> = Vec::new();
    for piece in s.split(',') {
        if let Some(c) = parse_compound(piece) {
            out.push(c);
        }
    }
    out
}

/// Parse one comma-piece — possibly with combinators + multiple simple
/// segments. Returns `None` if the input is empty / malformed.
fn parse_compound(s: &str) -> Option<CompoundSelector> {
    // Tokenize into alternating (simple, combinator) runs. A
    // combinator is either explicit (`>`, `+`, `~`, with surrounding
    // whitespace) or implicit (just whitespace = descendant).
    let bytes = s.as_bytes();
    let mut i = 0usize;
    let len = bytes.len();
    // Skip leading whitespace.
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= len {
        return None;
    }

    // Read the first simple selector.
    let head_end = scan_simple_end(bytes, i);
    let head = parse_simple(&s[i..head_end])?;
    i = head_end;

    let mut segments: Vec<(Combinator, SimpleSelector)> = Vec::new();
    loop {
        // Skip whitespace, capturing whether we crossed any.
        let ws_start = i;
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let saw_ws = i > ws_start;
        if i >= len {
            break;
        }
        let combo = match bytes[i] {
            b'>' => {
                i += 1;
                Combinator::Child
            }
            b'+' => {
                i += 1;
                Combinator::AdjacentSibling
            }
            b'~' => {
                i += 1;
                Combinator::GeneralSibling
            }
            _ => {
                if !saw_ws {
                    // No whitespace and no combinator char — malformed.
                    return None;
                }
                Combinator::Descendant
            }
        };
        // Skip whitespace after combinator.
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= len {
            // Trailing combinator with no rhs — skip.
            break;
        }
        let end = scan_simple_end(bytes, i);
        let next = parse_simple(&s[i..end])?;
        i = end;
        segments.push((combo, next));
    }

    Some(CompoundSelector { head, segments })
}

/// Find the end of one simple-selector token starting at `start`. A
/// simple ends at whitespace or at one of the combinator chars `>+~`
/// (when not inside `[...]` or `(...)`).
fn scan_simple_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    let mut bracket = 0u32;
    let mut paren = 0u32;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'[' => bracket += 1,
            b']' if bracket > 0 => bracket -= 1,
            b'(' => paren += 1,
            b')' if paren > 0 => paren -= 1,
            b' ' | b'\t' | b'\n' | b'\r' if bracket == 0 && paren == 0 => break,
            b'>' | b'+' | b'~' if bracket == 0 && paren == 0 => break,
            _ => {}
        }
        i += 1;
    }
    i
}

/// Parse one simple-selector piece (no combinators). Returns `None` for
/// empty input.
fn parse_simple(s: &str) -> Option<SimpleSelector> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let bytes = s.as_bytes();
    let mut sel = SimpleSelector::default();
    let mut i = 0usize;
    let len = bytes.len();

    // Optional tag at the start: `*` or ASCII alpha.
    if bytes[0] == b'*' {
        i = 1;
    } else if bytes[0].is_ascii_alphabetic() {
        let start = i;
        while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_')
        {
            i += 1;
        }
        sel.tag = Some(s[start..i].to_ascii_lowercase());
    }

    while i < len {
        let kind = bytes[i];
        match kind {
            b'.' => {
                i += 1;
                let start = i;
                while i < len
                    && bytes[i] != b'.'
                    && bytes[i] != b'#'
                    && bytes[i] != b'['
                    && bytes[i] != b':'
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_')
                {
                    i += 1;
                }
                let token = &s[start..i];
                if !token.is_empty() {
                    sel.classes.push(token.to_string());
                }
            }
            b'#' => {
                i += 1;
                let start = i;
                while i < len
                    && bytes[i] != b'.'
                    && bytes[i] != b'#'
                    && bytes[i] != b'['
                    && bytes[i] != b':'
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_')
                {
                    i += 1;
                }
                let token = &s[start..i];
                if !token.is_empty() && sel.id.is_none() {
                    sel.id = Some(token.to_string());
                }
            }
            b'[' => {
                // `[name op "value"]` or `[name]`.
                let close = memchr_close_bracket(bytes, i + 1)?;
                let inner = &s[i + 1..close];
                if let Some(p) = parse_attr_predicate(inner) {
                    sel.attrs.push(p);
                }
                i = close + 1;
            }
            b':' => {
                i += 1;
                if i < len && bytes[i] == b':' {
                    // `::pseudo-element` — round 11 typed parsing.
                    i += 1;
                    let name_start = i;
                    while i < len
                        && (bytes[i].is_ascii_alphanumeric()
                            || bytes[i] == b'-'
                            || bytes[i] == b'_')
                    {
                        i += 1;
                    }
                    let name = &s[name_start..i];
                    if let Some(pe) = PseudoElement::from_str(name) {
                        // CSS rules: at most one pseudo-element per
                        // compound. The last one wins (matches browser
                        // tolerance — extras are accepted gracefully).
                        sel.pseudo_element = Some(pe);
                    }
                    // Unknown `::pseudo-element` (e.g. `::placeholder`,
                    // `::selection`) — silently drop the keyword. The
                    // rule's other components survive.
                    continue;
                }
                let name_start = i;
                while i < len
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_')
                {
                    i += 1;
                }
                let name = &s[name_start..i];
                let mut arg: Option<&str> = None;
                if i < len && bytes[i] == b'(' {
                    let close = memchr_close_paren(bytes, i + 1)?;
                    arg = Some(&s[i + 1..close]);
                    i = close + 1;
                }
                // CSS 2.1 §5.12.1 — `:before`, `:after`, `:first-letter`,
                // `:first-line` may appear with a single colon. Treat
                // them as pseudo-elements (round 11) so the rule never
                // matches a real element.
                if arg.is_none() {
                    if let Some(pe) = PseudoElement::from_str(name) {
                        sel.pseudo_element = Some(pe);
                        continue;
                    }
                }
                if let Some(p) = parse_pseudo(name, arg) {
                    sel.pseudos.push(p);
                }
                // Unknown pseudo-classes are silently dropped (treated
                // as "no extra constraint" — they'll over-match. That's
                // bad, but leaving them as "always-false" would drop
                // entire rules. We pick over-match as the friendlier
                // failure mode for unsupported `:hover` etc).
            }
            c if c.is_ascii_whitespace() => break,
            _ => return None,
        }
    }

    Some(sel)
}

fn memchr_close_bracket(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    let mut in_str: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_str {
            if b == q {
                in_str = None;
            }
        } else {
            match b {
                b'"' | b'\'' => in_str = Some(b),
                b']' => return Some(i),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn memchr_close_paren(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    let mut depth = 0u32;
    let mut in_str: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_str {
            if b == q {
                in_str = None;
            }
        } else {
            match b {
                b'"' | b'\'' => in_str = Some(b),
                b'(' => depth += 1,
                b')' => {
                    if depth == 0 {
                        return Some(i);
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn parse_attr_predicate(inner: &str) -> Option<AttrPredicate> {
    let inner = inner.trim();
    if inner.is_empty() {
        return None;
    }
    // Find the operator, in order of specificity (`~=`, `|=`, `^=`,
    // `$=`, `*=`, `=`, then bare `[name]`).
    for (sigil, op) in [
        ("~=", AttrOp::Includes),
        ("|=", AttrOp::DashMatch),
        ("^=", AttrOp::StartsWith),
        ("$=", AttrOp::EndsWith),
        ("*=", AttrOp::Contains),
    ] {
        if let Some(idx) = inner.find(sigil) {
            let name = inner[..idx].trim();
            let value = unquote(inner[idx + 2..].trim());
            if name.is_empty() {
                return None;
            }
            return Some(AttrPredicate {
                name: name.to_string(),
                op,
                value,
            });
        }
    }
    if let Some(idx) = inner.find('=') {
        let name = inner[..idx].trim();
        let value = unquote(inner[idx + 1..].trim());
        if name.is_empty() {
            return None;
        }
        return Some(AttrPredicate {
            name: name.to_string(),
            op: AttrOp::Equals,
            value,
        });
    }
    Some(AttrPredicate {
        name: inner.to_string(),
        op: AttrOp::Exists,
        value: String::new(),
    })
}

fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

fn parse_pseudo(name: &str, arg: Option<&str>) -> Option<Pseudo> {
    let lower = name.to_ascii_lowercase();
    match (lower.as_str(), arg) {
        ("first-child", None) => Some(Pseudo::FirstChild),
        ("last-child", None) => Some(Pseudo::LastChild),
        ("only-child", None) => Some(Pseudo::OnlyChild),
        ("first-of-type", None) => Some(Pseudo::FirstOfType),
        ("last-of-type", None) => Some(Pseudo::LastOfType),
        ("only-of-type", None) => Some(Pseudo::OnlyOfType),
        ("nth-child", Some(a)) => {
            let (an, b) = parse_nth(a)?;
            Some(Pseudo::NthChild(an, b))
        }
        ("nth-of-type", Some(a)) => {
            let (an, b) = parse_nth(a)?;
            Some(Pseudo::NthOfType(an, b))
        }
        ("nth-last-child", Some(a)) => {
            let (an, b) = parse_nth(a)?;
            Some(Pseudo::NthLastChild(an, b))
        }
        ("nth-last-of-type", Some(a)) => {
            let (an, b) = parse_nth(a)?;
            Some(Pseudo::NthLastOfType(an, b))
        }
        ("lang", Some(arg)) => {
            // CSS strips surrounding whitespace + optional quotes per
            // §3.3 ("strings"). Empty after strip → drop the rule.
            let trimmed = arg.trim();
            let unquoted = unquote(trimmed);
            if unquoted.is_empty() {
                return None;
            }
            Some(Pseudo::Lang(unquoted))
        }
        ("not", Some(arg)) => {
            // Recursive, but only one level deep — we only allow a
            // simple selector inside `:not(...)` per Selectors L3 spec.
            let inner = parse_simple(arg.trim())?;
            // Forbid nested `:not` per spec.
            if inner.pseudos.iter().any(|p| matches!(p, Pseudo::Not(_))) {
                return None;
            }
            Some(Pseudo::Not(Box::new(inner)))
        }
        // Round 11 — interactive / link pseudo-classes parse to
        // `Stateful` so the rule survives the round-trip but never
        // matches in a static document. Previously these were silently
        // dropped (over-matching their carrier rules).
        ("hover", None) => Some(Pseudo::Stateful(StatefulPseudo::Hover)),
        ("focus", None) => Some(Pseudo::Stateful(StatefulPseudo::Focus)),
        ("active", None) => Some(Pseudo::Stateful(StatefulPseudo::Active)),
        ("checked", None) => Some(Pseudo::Stateful(StatefulPseudo::Checked)),
        ("visited", None) => Some(Pseudo::Stateful(StatefulPseudo::Visited)),
        ("link", None) => Some(Pseudo::Stateful(StatefulPseudo::Link)),
        ("disabled", None) => Some(Pseudo::Stateful(StatefulPseudo::Disabled)),
        ("enabled", None) => Some(Pseudo::Stateful(StatefulPseudo::Enabled)),
        // Anything else — silently drop (over-matches the rule, which
        // is the friendlier failure mode for unknown pseudos).
        _ => None,
    }
}

/// Parse `@import url(…) [media-query-list];` per CSS 2.1 §6.3 and
/// return the URL (with `url(...)` wrapping + quotes stripped). Returns
/// `None` for any other @-rule or a malformed `@import`.
fn parse_at_import(raw: &str) -> Option<String> {
    let r = raw.trim();
    let r = r.strip_prefix('@')?;
    // Match the `import` keyword case-insensitively per CSS3 §3.1.
    if r.len() < 6 || !r[..6].eq_ignore_ascii_case("import") {
        return None;
    }
    let rest = r[6..].trim_start_matches(|c: char| c.is_whitespace() || c == ';');
    // Strip a trailing `;` if present.
    let rest = rest.trim().trim_end_matches(';').trim();
    if rest.is_empty() {
        return None;
    }
    // Two surface forms:
    //   @import url("foo.css") screen;
    //   @import "foo.css" screen;
    let (url_token, _media) = if let Some(after_url) = rest
        .strip_prefix("url(")
        .or_else(|| rest.strip_prefix("URL("))
        .or_else(|| rest.strip_prefix("Url("))
    {
        let close = after_url.find(')')?;
        let inside = &after_url[..close];
        let media = after_url[close + 1..].trim();
        (inside.trim(), media)
    } else {
        // Bare `"..."` / `'...'` form. Find the closing quote.
        let bytes = rest.as_bytes();
        let q = bytes[0];
        if q != b'"' && q != b'\'' {
            return None;
        }
        let close_rel = rest[1..].find(q as char)?;
        let close = close_rel + 1;
        let inside = &rest[1..close];
        let media = rest[close + 1..].trim();
        (inside, media)
    };
    // Strip surrounding quotes if any.
    let url = unquote(url_token.trim());
    if url.is_empty() {
        return None;
    }
    Some(url)
}

/// Parse the argument of `:nth-child` etc. — `odd`, `even`, `An+B`,
/// `An`, `B`, `n`, `-n`, etc.
fn parse_nth(arg: &str) -> Option<(i32, i32)> {
    let t = arg.trim().to_ascii_lowercase();
    match t.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    // Find an optional `n`.
    if let Some(n_pos) = t.find('n') {
        let a_str = t[..n_pos].trim();
        let a = match a_str {
            "" | "+" => 1,
            "-" => -1,
            other => other.parse::<i32>().ok()?,
        };
        let rest = t[n_pos + 1..].trim();
        let b = if rest.is_empty() {
            0
        } else {
            // Expect `+B` or `-B`.
            let mut chars = rest.chars();
            let sign = chars.next()?;
            let num: String = chars.collect();
            let n: i32 = num.trim().parse().ok()?;
            match sign {
                '+' => n,
                '-' => -n,
                _ => return None,
            }
        };
        Some((a, b))
    } else {
        // Plain number.
        let b: i32 = t.parse().ok()?;
        Some((0, b))
    }
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

/// Return the effective declaration list for `mctx.el` — the sheet's
/// matched-by-specificity declarations followed by the element's inline
/// `style="..."`. Caller iterates the result in order, last write wins.
pub fn declarations_for(mctx: &MatchContext<'_>, sheet: &Stylesheet) -> Vec<(String, String)> {
    let mut out = sheet.matched_declarations(mctx);
    if let Some(s) = attr(mctx.el, "style") {
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

    fn ctx<'a>(e: &'a Element) -> MatchContext<'a> {
        MatchContext::root(e)
    }

    #[test]
    fn parse_simple_class_rule() {
        let mut s = Stylesheet::new();
        s.parse_block(".big { fill: #ff0000; stroke-width: 4 }");
        assert_eq!(s.rules.len(), 1);
        assert_eq!(
            s.rules[0].selectors[0].head.classes,
            vec!["big".to_string()]
        );
        assert_eq!(s.rules[0].declarations.len(), 2);
    }

    #[test]
    fn parse_id_and_tag_selectors() {
        let mut s = Stylesheet::new();
        s.parse_block("#main { fill: blue } rect { stroke: black }");
        assert_eq!(s.rules.len(), 2);
        let r = &s.rules[0];
        assert_eq!(r.selectors[0].head.id, Some("main".into()));
        let r = &s.rules[1];
        assert_eq!(r.selectors[0].head.tag, Some("rect".into()));
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
        assert_eq!(s.rules[0].selectors[0].head.classes, vec!["x".to_string()]);
    }

    #[test]
    fn matches_class_and_id() {
        let mut sheet = Stylesheet::new();
        sheet.parse_block(".foo { fill: red } #bar { fill: blue }");
        let el = elem("rect", &[("class", "foo"), ("id", "bar")]);
        let decls = sheet.matched_declarations(&ctx(&el));
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
        let decls = declarations_for(&ctx(&el), &sheet);
        // `.a` then inline → last is inline.
        assert_eq!(decls.last().unwrap().1, "green");
    }

    #[test]
    fn specificity_order() {
        let id = SimpleSelector {
            id: Some("x".into()),
            ..SimpleSelector::default()
        };
        let cls = SimpleSelector {
            classes: vec!["x".into()],
            ..SimpleSelector::default()
        };
        let tag = SimpleSelector {
            tag: Some("x".into()),
            ..SimpleSelector::default()
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

    // ---- Round 5 attribute selector tests ----

    #[test]
    fn attr_exists_predicate() {
        let mut s = Stylesheet::new();
        s.parse_block("[lang] { fill: red }");
        let with = elem("rect", &[("lang", "en")]);
        let without = elem("rect", &[]);
        assert_eq!(s.matched_declarations(&ctx(&with)).len(), 1);
        assert_eq!(s.matched_declarations(&ctx(&without)).len(), 0);
    }

    #[test]
    fn attr_equals_predicate() {
        let mut s = Stylesheet::new();
        s.parse_block(r#"[role="button"] { fill: red }"#);
        let yes = elem("g", &[("role", "button")]);
        let no = elem("g", &[("role", "menu")]);
        assert_eq!(s.matched_declarations(&ctx(&yes)).len(), 1);
        assert_eq!(s.matched_declarations(&ctx(&no)).len(), 0);
    }

    #[test]
    fn attr_includes_predicate() {
        let mut s = Stylesheet::new();
        s.parse_block(r#"[class~="big"] { fill: red }"#);
        let yes = elem("rect", &[("class", "small big huge")]);
        let no = elem("rect", &[("class", "smallbig")]);
        assert_eq!(s.matched_declarations(&ctx(&yes)).len(), 1);
        assert_eq!(s.matched_declarations(&ctx(&no)).len(), 0);
    }

    #[test]
    fn attr_prefix_suffix_substring() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"[href^="https"] { fill: red }
               [href$=".svg"]  { fill: blue }
               [href*="example"] { fill: green }"#,
        );
        let el = elem("a", &[("href", "https://example.com/foo.svg")]);
        let decls = s.matched_declarations(&ctx(&el));
        assert_eq!(decls.len(), 3);
    }

    #[test]
    fn attr_dash_match_predicate() {
        let mut s = Stylesheet::new();
        s.parse_block(r#"[lang|="en"] { fill: red }"#);
        let plain = elem("text", &[("lang", "en")]);
        let region = elem("text", &[("lang", "en-US")]);
        let other = elem("text", &[("lang", "fr")]);
        let endash = elem("text", &[("lang", "english")]);
        assert_eq!(s.matched_declarations(&ctx(&plain)).len(), 1);
        assert_eq!(s.matched_declarations(&ctx(&region)).len(), 1);
        assert_eq!(s.matched_declarations(&ctx(&other)).len(), 0);
        assert_eq!(s.matched_declarations(&ctx(&endash)).len(), 0);
    }

    // ---- Round 5 pseudo-class tests ----

    #[test]
    fn nth_child_match() {
        // 1-indexed.
        assert!(nth_match(1, 0, 1));
        assert!(!nth_match(2, 0, 1));
        // 2n: even indices.
        assert!(nth_match(2, 2, 0));
        assert!(nth_match(4, 2, 0));
        assert!(!nth_match(3, 2, 0));
        // 2n+1: odd.
        assert!(nth_match(1, 2, 1));
        assert!(nth_match(3, 2, 1));
        assert!(!nth_match(2, 2, 1));
        // -n+3: indices 1, 2, 3.
        assert!(nth_match(1, -1, 3));
        assert!(nth_match(3, -1, 3));
        assert!(!nth_match(4, -1, 3));
    }

    #[test]
    fn parse_nth_argument() {
        assert_eq!(parse_nth("odd"), Some((2, 1)));
        assert_eq!(parse_nth("even"), Some((2, 0)));
        assert_eq!(parse_nth("3"), Some((0, 3)));
        assert_eq!(parse_nth("2n"), Some((2, 0)));
        assert_eq!(parse_nth("n"), Some((1, 0)));
        assert_eq!(parse_nth("2n+1"), Some((2, 1)));
        assert_eq!(parse_nth("-n+3"), Some((-1, 3)));
        assert_eq!(parse_nth("-2n-1"), Some((-2, -1)));
    }

    #[test]
    fn parse_pseudo_classes() {
        let mut s = Stylesheet::new();
        s.parse_block(
            ":first-child { fill: red } :nth-child(2n+1) { fill: blue } :not(.x) { stroke: green }",
        );
        assert_eq!(s.rules.len(), 3);
        assert_eq!(s.rules[0].selectors[0].head.pseudos.len(), 1);
        assert_eq!(s.rules[1].selectors[0].head.pseudos.len(), 1);
        assert_eq!(s.rules[2].selectors[0].head.pseudos.len(), 1);
    }

    #[test]
    fn parse_combinators() {
        let mut s = Stylesheet::new();
        s.parse_block(
            "a > b { fill: red } a + b { stroke: blue } a ~ b { opacity: 0.5 } a b { fill: green }",
        );
        assert_eq!(s.rules.len(), 4);
        assert_eq!(s.rules[0].selectors[0].segments[0].0, Combinator::Child);
        assert_eq!(
            s.rules[1].selectors[0].segments[0].0,
            Combinator::AdjacentSibling
        );
        assert_eq!(
            s.rules[2].selectors[0].segments[0].0,
            Combinator::GeneralSibling
        );
        assert_eq!(
            s.rules[3].selectors[0].segments[0].0,
            Combinator::Descendant
        );
    }

    #[test]
    fn double_colon_pseudo_element_recorded() {
        // **Round 11** — `::before` etc. now parse to a typed
        // `PseudoElement` on the carrier selector. The rule survives.
        let mut s = Stylesheet::new();
        s.parse_block("p::before { content: 'x' }");
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].selectors[0].head.tag, Some("p".into()));
        assert_eq!(
            s.rules[0].selectors[0].head.pseudo_element,
            Some(PseudoElement::Before)
        );
    }

    #[test]
    fn stateful_pseudo_class_recorded_round11() {
        // Round 11 — `:hover` parses to Pseudo::Stateful so the rule
        // doesn't over-match. Previously it was silently dropped which
        // collapsed `.x:hover { fill: red }` to `.x { fill: red }`.
        let mut s = Stylesheet::new();
        s.parse_block(".x:hover { fill: red }");
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].selectors[0].head.classes, vec!["x".to_string()]);
        assert_eq!(s.rules[0].selectors[0].head.pseudos.len(), 1);
        match &s.rules[0].selectors[0].head.pseudos[0] {
            Pseudo::Stateful(StatefulPseudo::Hover) => {}
            other => panic!("expected Stateful(Hover), got {:?}", other),
        }
    }

    // ---- Round 6 selector tests ----

    #[test]
    fn parse_nth_last_child_pseudo() {
        let mut s = Stylesheet::new();
        s.parse_block(":nth-last-child(2) { fill: red }");
        assert_eq!(s.rules.len(), 1);
        match &s.rules[0].selectors[0].head.pseudos[0] {
            Pseudo::NthLastChild(0, 2) => {}
            other => panic!("expected NthLastChild(0,2), got {:?}", other),
        }
    }

    #[test]
    fn parse_nth_last_of_type_pseudo() {
        let mut s = Stylesheet::new();
        s.parse_block(":nth-last-of-type(odd) { fill: red }");
        assert_eq!(s.rules.len(), 1);
        match &s.rules[0].selectors[0].head.pseudos[0] {
            Pseudo::NthLastOfType(2, 1) => {}
            other => panic!("expected NthLastOfType(2,1), got {:?}", other),
        }
    }

    #[test]
    fn parse_lang_pseudo_unquoted() {
        let mut s = Stylesheet::new();
        s.parse_block(":lang(en) { fill: red }");
        assert_eq!(s.rules.len(), 1);
        match &s.rules[0].selectors[0].head.pseudos[0] {
            Pseudo::Lang(p) => assert_eq!(p, "en"),
            other => panic!("expected Lang, got {:?}", other),
        }
    }

    #[test]
    fn parse_lang_pseudo_quoted() {
        let mut s = Stylesheet::new();
        s.parse_block(r#":lang("zh") { fill: red }"#);
        match &s.rules[0].selectors[0].head.pseudos[0] {
            Pseudo::Lang(p) => assert_eq!(p, "zh"),
            other => panic!("expected Lang, got {:?}", other),
        }
    }

    #[test]
    fn lang_dash_match_helper() {
        assert!(lang_dash_match("en", "en"));
        assert!(lang_dash_match("en-US", "en"));
        assert!(lang_dash_match("EN-us", "en")); // case-insensitive
        assert!(!lang_dash_match("english", "en")); // not a dash boundary
        assert!(!lang_dash_match("fr", "en"));
        assert!(!lang_dash_match("en", "fr"));
    }

    #[test]
    fn lang_attr_prefers_xml_lang_over_lang() {
        let only_lang = elem("text", &[("lang", "fr")]);
        assert_eq!(lang_attr(&only_lang), Some("fr"));
        let both = elem("text", &[("xml:lang", "ja"), ("lang", "en")]);
        // `xml:lang` checked first.
        assert_eq!(lang_attr(&both), Some("ja"));
    }

    // ---- Round 11: pseudo-elements + @import + stateful pseudo-classes ----

    #[test]
    fn pseudo_element_after_recorded() {
        let mut s = Stylesheet::new();
        s.parse_block("li::after { content: ',' }");
        assert_eq!(
            s.rules[0].selectors[0].head.pseudo_element,
            Some(PseudoElement::After)
        );
    }

    #[test]
    fn pseudo_element_first_letter_first_line() {
        let mut s = Stylesheet::new();
        s.parse_block("p::first-letter { font-size: 200% } p::first-line { color: red }");
        assert_eq!(s.rules.len(), 2);
        assert_eq!(
            s.rules[0].selectors[0].head.pseudo_element,
            Some(PseudoElement::FirstLetter)
        );
        assert_eq!(
            s.rules[1].selectors[0].head.pseudo_element,
            Some(PseudoElement::FirstLine)
        );
    }

    #[test]
    fn legacy_single_colon_before_after_treated_as_pseudo_element() {
        // CSS 2.1 §5.12.1 — `:before` (single colon) is the legacy form
        // of `::before`. Must still parse as a pseudo-element so the
        // rule does not match an actual `::before` pseudo-class lookup.
        let mut s = Stylesheet::new();
        s.parse_block("h1:before { content: '★ ' }");
        assert_eq!(
            s.rules[0].selectors[0].head.pseudo_element,
            Some(PseudoElement::Before)
        );
    }

    #[test]
    fn pseudo_element_selector_never_matches_real_element() {
        // A `p::before` rule must not apply to a real `<p>` — the
        // pseudo-element is a synthesised box.
        let mut s = Stylesheet::new();
        s.parse_block("p::before { fill: red }");
        let p = elem("p", &[]);
        assert_eq!(s.matched_declarations(&ctx(&p)).len(), 0);
    }

    #[test]
    fn unknown_pseudo_element_silently_dropped() {
        // `::placeholder` / `::selection` / etc. — not modelled but the
        // rule's other components survive.
        let mut s = Stylesheet::new();
        s.parse_block("input::placeholder { color: gray }");
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].selectors[0].head.tag, Some("input".into()));
        assert_eq!(s.rules[0].selectors[0].head.pseudo_element, None);
    }

    #[test]
    fn pseudo_element_specificity_counts_one_tag_point() {
        // `::before` — one tag-level point per CSS3 §9.
        let mut s = Stylesheet::new();
        s.parse_block("p::before { fill: red }");
        let spec = s.rules[0].selectors[0].head.specificity();
        assert_eq!(spec, (0, 0, 2)); // tag p + ::before
    }

    #[test]
    fn at_import_url_form_recorded() {
        let mut s = Stylesheet::new();
        s.parse_block(r#"@import url("foo.css") screen;  .x { fill: red }"#);
        assert_eq!(s.imports, vec!["foo.css".to_string()]);
        // The trailing rule still parses.
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].selectors[0].head.classes, vec!["x".to_string()]);
    }

    #[test]
    fn at_import_bare_string_form_recorded() {
        let mut s = Stylesheet::new();
        s.parse_block(r#"@import "theme.css"; .a { fill: blue }"#);
        assert_eq!(s.imports, vec!["theme.css".to_string()]);
        assert_eq!(s.rules.len(), 1);
    }

    #[test]
    fn at_import_with_single_quotes() {
        let mut s = Stylesheet::new();
        s.parse_block(r#"@import url('a.css');"#);
        assert_eq!(s.imports, vec!["a.css".to_string()]);
    }

    #[test]
    fn at_import_with_media_query_keeps_url() {
        let mut s = Stylesheet::new();
        s.parse_block(r#"@import url("print.css") print and (min-width: 600px);"#);
        assert_eq!(s.imports, vec!["print.css".to_string()]);
    }

    #[test]
    fn at_import_multiple_urls_in_source_order() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
              @import url("a.css");
              @import url("b.css");
              .x { fill: red }
              @import url("c.css");
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
    }

    #[test]
    fn at_media_block_still_skipped_not_an_import() {
        // `@media` opens a block — must not be recorded as an import.
        let mut s = Stylesheet::new();
        s.parse_block("@media print { .x { fill: red } } .y { fill: blue }");
        assert!(s.imports.is_empty());
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].selectors[0].head.classes, vec!["y".to_string()]);
    }

    #[test]
    fn stateful_pseudo_classes_all_recognised() {
        let mut s = Stylesheet::new();
        s.parse_block(
            "a:hover { fill: red }
             a:focus { fill: blue }
             a:active { fill: green }
             input:checked { fill: yellow }
             a:visited { fill: purple }
             a:link { fill: orange }
             input:disabled { fill: gray }
             input:enabled { fill: black }",
        );
        assert_eq!(s.rules.len(), 8);
        let want = [
            StatefulPseudo::Hover,
            StatefulPseudo::Focus,
            StatefulPseudo::Active,
            StatefulPseudo::Checked,
            StatefulPseudo::Visited,
            StatefulPseudo::Link,
            StatefulPseudo::Disabled,
            StatefulPseudo::Enabled,
        ];
        for (i, w) in want.iter().enumerate() {
            match &s.rules[i].selectors[0].head.pseudos[0] {
                Pseudo::Stateful(got) => assert_eq!(got, w, "rule {i}"),
                other => panic!("rule {i}: expected Stateful({w:?}), got {other:?}"),
            }
        }
    }

    #[test]
    fn stateful_pseudo_class_never_matches_in_static_doc() {
        // Static cascade — `:hover` never selects a real `<a>`.
        let mut s = Stylesheet::new();
        s.parse_block("a:hover { fill: red }");
        let a = elem("a", &[]);
        assert_eq!(s.matched_declarations(&ctx(&a)).len(), 0);
    }

    #[test]
    fn stateful_pseudo_class_does_not_overmatch_carrier_selector() {
        // **Bug fixed in round 11** — previously `.x:hover` was
        // silently truncated to `.x`, so `.x` matched and applied red.
        // Now the `:hover` Pseudo participates in matching and rejects.
        let mut s = Stylesheet::new();
        s.parse_block(".x:hover { fill: red } .x { stroke: blue }");
        let el = elem("rect", &[("class", "x")]);
        let decls = s.matched_declarations(&ctx(&el));
        // Only the second rule (no pseudo) should apply.
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].0, "stroke");
        assert_eq!(decls[0].1, "blue");
    }

    #[test]
    fn parse_at_import_helper_strips_url_quotes() {
        assert_eq!(
            parse_at_import(r#"@import url("foo.css")"#),
            Some("foo.css".into())
        );
        assert_eq!(
            parse_at_import(r#"@import "foo.css";"#),
            Some("foo.css".into())
        );
        assert_eq!(
            parse_at_import(r#"@import url(foo.css)"#),
            Some("foo.css".into())
        );
        assert_eq!(
            parse_at_import(r#"@import 'foo.css'"#),
            Some("foo.css".into())
        );
        // Not an @import.
        assert_eq!(parse_at_import("@media print"), None);
        // Empty url — drop.
        assert_eq!(parse_at_import(r#"@import ""#), None);
    }
}
