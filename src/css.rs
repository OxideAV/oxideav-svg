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
    /// **Round 14** — every `@font-face { ... }` block per CSS Fonts
    /// L3 §4. Captured in source order so a downstream font-resolver
    /// can iterate the family / src / weight / style descriptors and
    /// register the user-supplied fonts before the cascade is
    /// resolved against any `font-family: ...` declaration.
    /// Loading the actual font bytes is left to the caller; the
    /// parser only collects the descriptors.
    pub font_faces: Vec<FontFace>,
    /// **Round 15** — every `@keyframes <name> { ... }` block per CSS
    /// Animations L1 §3. Captured in source order so a downstream
    /// animation engine (or the rasteriser's own SMIL-via-`@keyframes`
    /// bridge) can iterate the rules without re-parsing the source.
    /// Loading these into a real animation timeline is left to the
    /// caller; the parser only collects the structure.
    pub keyframes: Vec<KeyframesRule>,
    /// **Round 16** — every `@media (cond) { ... rules ... }` block per
    /// CSS Media Queries L4. Captured in source order; the inner
    /// `rules` are NOT folded into [`Self::rules`] (they're conditional
    /// on the runtime viewport, which the parser doesn't know). Use
    /// [`Self::resolve_for_media_context`] to evaluate the condition
    /// against a concrete viewport and pull the matching rule list.
    pub media_rules: Vec<MediaRule>,
    /// **Round 17** — every `@supports (cond) { ... rules ... }` block
    /// per CSS Conditional Rules L3. Captured in source order; the
    /// inner `rules` are NOT folded into [`Self::rules`] (they're
    /// conditional on the runtime feature-detection result, which the
    /// parser doesn't know). Use [`Self::resolve_for_supports_context`]
    /// to evaluate the condition against a concrete supported-property
    /// set and pull the matching rule list.
    pub supports_rules: Vec<SupportsRule>,
}

/// One captured `@media (condition) { ... rules ... }` block per CSS
/// Media Queries L4.
///
/// Each rule has a [`MediaCondition`] (the `(feature: value)` clauses
/// joined by an operator) and a list of inner [`Rule`]s that apply
/// when the condition matches the current viewport. Rules inside a
/// non-matching `@media` block do NOT participate in the cascade — the
/// parser surfaces both halves so the consumer can decide.
#[derive(Clone, Debug, Default)]
pub struct MediaRule {
    /// Parsed media-condition prelude.
    pub condition: MediaCondition,
    /// Style rules nested inside the `@media` block, in source order.
    pub rules: Vec<Rule>,
}

/// One media-query condition — a list of `(feature: value)` clauses
/// joined by [`MediaOperator`].
///
/// Round 16 supports `width`, `height`, and `orientation` features
/// (with optional `min-` / `max-` prefixes per CSS Media Queries L4
/// §4); `color-gamut` / `prefers-*` / `hover` etc. are deferred.
/// Unrecognised features are kept verbatim in the [`MediaFeature`]
/// list but never match (so the rule body is dormant).
#[derive(Clone, Debug, Default)]
pub struct MediaCondition {
    /// Comma-separated `media_query_list` per CSS Media Queries L4 §3:
    /// each entry is one media query that ORs into the overall match.
    /// Empty list (no condition at all) → always matches (the spec's
    /// implicit `all` media type).
    pub queries: Vec<MediaQuery>,
}

/// One leaf media query — an optional `not | only` modifier plus a
/// list of feature clauses joined by `and`.
#[derive(Clone, Debug, Default)]
pub struct MediaQuery {
    /// Optional leading `not` / `only` modifier. `Only` is a
    /// browser-compat hint per CSS Media Queries L4 §3.1 — it parses
    /// like the absence of any modifier (we honour it as a passthrough).
    pub modifier: Option<MediaOperator>,
    /// Optional media type (`screen` / `print` / `all`). `None` is
    /// equivalent to `all` per §3.
    pub media_type: Option<String>,
    /// Feature clauses joined by `and`.
    pub features: Vec<MediaFeature>,
}

/// Boolean operator joining clauses inside a [`MediaCondition`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaOperator {
    /// `not (feature)` — overall match negates the inner expression.
    Not,
    /// `only` — browser-compat hint (CSS Media Queries L4 §3.1); we
    /// treat it as a no-op for matching purposes.
    Only,
}

/// One `(name: value)` clause inside a media query.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaFeature {
    /// Lowercased feature name (e.g. `"width"`, `"min-width"`,
    /// `"orientation"`).
    pub name: String,
    /// Comparison the value uses against the runtime viewport — `Eq`
    /// for plain `(width: 800px)`, `MinEq` / `MaxEq` for the
    /// `min-` / `max-` shorthand prefixes per §4.
    pub op: ComparisonOp,
    /// Expected value (parsed; for unrecognised features
    /// [`MediaValue::Raw`] preserves the source text).
    pub value: MediaValue,
}

/// Comparison kind for a [`MediaFeature`] against the runtime
/// viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComparisonOp {
    /// Exact match — used by plain `(name: value)` form.
    Eq,
    /// `>=` — used by `(min-name: value)` shorthand per §4.
    MinEq,
    /// `<=` — used by `(max-name: value)` shorthand per §4.
    MaxEq,
}

/// Parsed value of a [`MediaFeature`].
///
/// Round 16 typed variants cover the three features used in practice;
/// everything else falls through to [`MediaValue::Raw`] for round-trip
/// fidelity.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaValue {
    /// Numeric length (px / pt / em / etc.). Round 16 treats every
    /// unit as user units (matches `parse_number`'s SVG behaviour);
    /// `1em != 16px` is a future refinement.
    Length(f32),
    /// `(orientation: portrait | landscape)` per §4.
    Orientation(Orientation),
    /// Raw value text for unrecognised features (so the rule round-
    /// trips even if it never matches).
    Raw(String),
}

/// Orientation value for `@media (orientation: ...)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
}

/// One captured `@supports (condition) { ... rules ... }` block per CSS
/// Conditional Rules L3 §2.
///
/// Each rule has a [`SupportsCondition`] (a property/value declaration
/// or boolean combination thereof) and a list of inner [`Rule`]s that
/// apply when the condition matches the runtime supported-property
/// set. Rules inside a non-matching `@supports` block do NOT
/// participate in the cascade — the parser surfaces both halves so the
/// consumer can decide.
#[derive(Clone, Debug, Default)]
pub struct SupportsRule {
    /// Parsed support condition.
    pub condition: SupportsCondition,
    /// Style rules nested inside the `@supports` block, in source
    /// order.
    pub rules: Vec<Rule>,
}

/// One support condition — either a leaf `(prop: value)` declaration
/// test or a boolean combination of nested conditions per CSS
/// Conditional Rules L3 §3.1.
///
/// Round 17 supports the complete grammar surface: leaf
/// `(property: value)`, `not (...)`, `(...) and (...)`, and
/// `(...) or (...)`. Nested combinations are honoured via the boxed
/// recursion on [`SupportsCondition::Not`] and the `Vec` arms.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SupportsCondition {
    /// Leaf `(prop: value)` test. Matches when the runtime supplies
    /// the (lowercased property name, value) pair in
    /// [`Stylesheet::resolve_for_supports_context`].
    Property { name: String, value: String },
    /// `not (cond)` — overall match negates the inner expression.
    Not(Box<SupportsCondition>),
    /// `(a) and (b) and (c)` — every entry must match.
    And(Vec<SupportsCondition>),
    /// `(a) or (b) or (c)` — at least one entry must match.
    Or(Vec<SupportsCondition>),
    /// Empty / always-matches condition (returned for an empty
    /// `@supports` prelude — a defensive fallback; an `@supports`
    /// rule without a condition is malformed per L3 §2.4 but we keep
    /// the rule so it round-trips).
    #[default]
    Always,
}

/// One captured `@keyframes <name> { ... }` block per CSS Animations
/// L1 §3.
///
/// Each rule has a name (the animation identifier referenced by an
/// `animation-name:` declaration) and a list of selectors; each
/// selector pairs an offset on the animation timeline with the CSS
/// declarations to apply at that point.
#[derive(Clone, Debug, Default)]
pub struct KeyframesRule {
    /// Animation name — the identifier after `@keyframes`. Quotes (a
    /// CSS Animations L1 alternate syntax) are stripped.
    pub name: String,
    /// Per-offset declaration blocks, in source order.
    pub selectors: Vec<KeyframeSelector>,
}

/// One keyframe selector inside an `@keyframes` block.
///
/// Per CSS Animations L1 §3.1, a keyframe selector is one of `from`
/// (= `0%`), `to` (= `100%`), or a percentage. Multiple comma-
/// separated offsets are supported; we expand them into one selector
/// per offset so each entry has exactly one [`KeyframeOffset`].
#[derive(Clone, Debug)]
pub struct KeyframeSelector {
    /// Animation timeline position for this keyframe.
    pub offset: KeyframeOffset,
    /// CSS declarations to apply at this offset.
    pub declarations: Vec<(String, String)>,
}

/// Animation timeline position for a [`KeyframeSelector`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyframeOffset {
    /// `from` keyword (= `0%`).
    From,
    /// `to` keyword (= `100%`).
    To,
    /// Explicit percentage in the closed interval `[0, 100]` (per
    /// §3.1; out-of-range offsets are still kept verbatim — the
    /// downstream animator decides how to handle them).
    Percent(f32),
}

impl KeyframeOffset {
    /// Convert to a normalised `[0.0, 1.0]` value for sorting /
    /// timeline interpolation.
    pub fn as_normalised(&self) -> f32 {
        match self {
            KeyframeOffset::From => 0.0,
            KeyframeOffset::To => 1.0,
            KeyframeOffset::Percent(p) => p / 100.0,
        }
    }
}

/// One captured `@font-face { ... }` block per CSS Fonts L3 §4.
///
/// `family` is the unquoted value of the `font-family:` descriptor;
/// `src` is the parsed `src:` list (may be empty if the descriptor
/// was missing or malformed). `descriptors` holds **every** parsed
/// descriptor verbatim — `font-weight`, `font-style`, `font-stretch`,
/// `unicode-range`, `font-display`, plus any future descriptors —
/// indexed by lowercase name. The two-table split is for ergonomic
/// access (callers reach for `family` and `src` 99 % of the time)
/// while keeping the long-tail capability lossless.
#[derive(Clone, Debug, Default)]
pub struct FontFace {
    /// Value of the `font-family:` descriptor with surrounding quotes
    /// stripped. Empty if the descriptor was missing.
    pub family: String,
    /// Parsed `src:` list — multiple `url(...)` / `local(...)` values
    /// in fallback order per CSS Fonts L3 §4.3.
    pub src: Vec<FontSource>,
    /// Every descriptor (lowercase name → trimmed value). Includes
    /// `font-family` + `src` raw text alongside the typed views above
    /// for full round-trip fidelity.
    pub descriptors: std::collections::HashMap<String, String>,
}

/// One entry in an `@font-face { src: ... }` list.
///
/// CSS Fonts L3 §4.3 allows `src:` to be a comma-separated fallback
/// list, where each entry is one of:
///
/// - `url(<url>) [format("<hint>")]` — external font file
/// - `local(<name>)` — installed system font by family / PostScript name
///
/// Exactly one of `url` / `local_name` is `Some` for a well-formed
/// entry; both being `None` means the parser kept the entry in the
/// list but couldn't extract a usable reference (the raw text still
/// survives in `FontFace::descriptors["src"]`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FontSource {
    /// External font URL. `None` for `local(...)` entries.
    pub url: Option<String>,
    /// Optional `format("woff2"|"truetype"|...)` hint per §4.3.
    pub format_hint: Option<String>,
    /// Installed font name for `local(...)` entries. `None` for
    /// `url(...)` entries.
    pub local_name: Option<String>,
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
            // 2.1 §6.3 into `imports`; capture `@font-face { ... }`
            // per CSS Fonts L3 §4 into `font_faces`; skip every other
            // @-rule (`@media`, `@keyframes`, `@page`, …).
            if bytes[i] == b'@' {
                let at_start = i;
                let mut depth = 0u32;
                let mut had_block = false;
                let mut end = i;
                let mut block_body_start = 0usize;
                let mut block_body_end = 0usize;
                while i < bytes.len() {
                    match bytes[i] {
                        b'{' => {
                            if depth == 0 {
                                block_body_start = i + 1;
                            }
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
                                block_body_end = i;
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
                // An unterminated block (`@media {…` with no closing `}`
                // before EOF) exits the scan with `block_body_end` still
                // at its `0` initialiser while `block_body_start` points
                // past the `{`. Treat the remainder of the input as the
                // block body so every `stripped[start..end]` slice below
                // stays ordered instead of panicking on `begin > end`.
                if had_block && block_body_end < block_body_start {
                    block_body_end = bytes.len();
                }
                if had_block {
                    // Block-style @-rule. Route `@font-face`,
                    // `@keyframes`, `@media`, and `@supports` to
                    // dedicated parsers; skip everything else
                    // (`@page`, vendor-specific rules, …).
                    let prelude_end = block_body_start.saturating_sub(1).min(stripped.len());
                    let prelude = stripped[at_start..prelude_end].trim();
                    let name = prelude
                        .strip_prefix('@')
                        .map(|r| r.split_whitespace().next().unwrap_or("").to_string())
                        .unwrap_or_default();
                    if name.eq_ignore_ascii_case("font-face") {
                        let body = &stripped[block_body_start..block_body_end];
                        if let Some(face) = parse_at_font_face(body) {
                            self.font_faces.push(face);
                        }
                    } else if name.eq_ignore_ascii_case("keyframes")
                        || name.eq_ignore_ascii_case("-webkit-keyframes")
                    {
                        let body = &stripped[block_body_start..block_body_end];
                        if let Some(rule) = parse_at_keyframes(prelude, body) {
                            self.keyframes.push(rule);
                        }
                    } else if name.eq_ignore_ascii_case("media") {
                        let body = &stripped[block_body_start..block_body_end];
                        if let Some(media_rule) = parse_at_media(prelude, body) {
                            self.media_rules.push(media_rule);
                        }
                    } else if name.eq_ignore_ascii_case("supports") {
                        let body = &stripped[block_body_start..block_body_end];
                        if let Some(sup_rule) = parse_at_supports(prelude, body) {
                            self.supports_rules.push(sup_rule);
                        }
                    }
                } else {
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

    /// Round 13 — resolve every `@import url(…)` URL recorded in
    /// [`Self::imports`] using a caller-supplied `fetcher`, parse the
    /// fetched body as CSS, and append the imported sheet's rules to
    /// `self.rules` so the cascade applies as if the rules were
    /// inline.
    ///
    /// Why caller-supplied: the SVG parser has no opinion on whether
    /// `@import url("foo.css")` should resolve via HTTP, the local
    /// filesystem, an in-memory cache, or a sandboxed bundler — the
    /// fetcher closure lets the consumer pick. `fetcher` returns
    /// `None` to signal "I can't / won't fetch this URL"; the import
    /// is then quietly skipped (matching browser tolerance — a
    /// missing imported sheet doesn't break the whole document).
    ///
    /// Recursion: the fetched sheet's own `@import`s are resolved
    /// transitively, up to a depth cap of 8 (matches what major
    /// browsers cap at — see CSS 2.1 §6.3 implementation note in the
    /// CSSOM specs). Cycles (a sheet eventually imports itself
    /// directly or transitively) are detected via a visited-URL set;
    /// the offending re-import is skipped and the rest of the sheet
    /// still applies.
    ///
    /// `imports` is left populated post-resolve so callers can
    /// re-introspect what was requested (e.g. for cache invalidation
    /// or a separate "show all sheets" UI).
    ///
    /// Failure modes (per the round-13 spec):
    ///
    /// - `fetcher` returns `None` → the import is silently dropped
    ///   (logged at `debug` for observability).
    /// - Fetched bytes aren't valid UTF-8 → silently dropped.
    /// - Parse produces no rules → silently dropped (matches
    ///   `parse_block`'s tolerant behaviour for malformed CSS).
    pub fn resolve_imports<F>(&mut self, fetcher: F)
    where
        F: Fn(&str) -> Option<Vec<u8>>,
    {
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let pending: Vec<String> = self.imports.clone();
        for url in pending {
            self.resolve_import_recursive(&url, &fetcher, &mut visited, 0);
        }
    }

    /// Internal recursive helper for `resolve_imports`. Tracks
    /// visited URLs (cycle detection) and recursion depth (runaway
    /// chain protection — capped at [`Self::IMPORT_DEPTH_CAP`]).
    fn resolve_import_recursive<F>(
        &mut self,
        url: &str,
        fetcher: &F,
        visited: &mut std::collections::HashSet<String>,
        depth: usize,
    ) where
        F: Fn(&str) -> Option<Vec<u8>>,
    {
        if depth >= Self::IMPORT_DEPTH_CAP {
            log::debug!(
                "@import depth cap ({}) reached; skipping `{}`",
                Self::IMPORT_DEPTH_CAP,
                url
            );
            return;
        }
        if !visited.insert(url.to_string()) {
            log::debug!("@import cycle detected on `{}`; skipping", url);
            return;
        }
        let bytes = match fetcher(url) {
            Some(b) => b,
            None => {
                log::debug!("@import fetcher returned None for `{}`; skipping", url);
                return;
            }
        };
        let css = match std::str::from_utf8(&bytes) {
            Ok(s) => s.to_string(),
            Err(e) => {
                log::debug!("@import body for `{}` is not UTF-8: {}", url, e);
                return;
            }
        };
        // Parse the fetched body into a fresh sheet so we can recurse
        // into its imports separately without polluting `self.imports`.
        let mut nested = Stylesheet::new();
        nested.parse_block(&css);
        // Per CSS 2.1 §6.3, the imported sheet's rules behave as if
        // they appeared at the @import statement's position. We
        // append to `self.rules` after the existing rules — the
        // parent's later rules still win on equal-specificity
        // ties (matched_declarations sorts stably by source
        // order). For full spec accuracy callers should call
        // `resolve_imports` before `parse_block`-ing additional
        // inline rules; documenting this is sufficient for round 13.
        self.rules.append(&mut nested.rules);
        // Recurse into the nested sheet's own imports.
        for nested_url in &nested.imports {
            self.resolve_import_recursive(nested_url, fetcher, visited, depth + 1);
        }
    }

    /// **Round 16** — return the cascade as if every matching `@media`
    /// block were inlined alongside the unconditional rules.
    ///
    /// Per CSS Media Queries L4 §5, an `@media` block's inner rules
    /// participate in the cascade only when the condition matches the
    /// runtime viewport. The parser captures both halves separately
    /// (unconditional rules in [`Self::rules`], conditional groups in
    /// [`Self::media_rules`]); this method walks both, evaluates each
    /// `@media` condition against `(viewport_w, viewport_h,
    /// orientation)`, and returns the merged rule list in source
    /// order — first the unconditional rules, then each matching
    /// `@media` block's inner rules in source order. Source order is
    /// preserved so the existing specificity / source-order tie-break
    /// in [`Self::matched_declarations`] still resolves correctly.
    ///
    /// Callers that want the matched declarations against a specific
    /// element + viewport should clone this sheet, append the matching
    /// media rules to `rules` (or build a synthetic sheet around the
    /// returned slice), and call [`Self::matched_declarations`]; the
    /// API surface deliberately stays decoupled from the cascade so
    /// tests can introspect the selection without needing an element.
    pub fn resolve_for_media_context(
        &self,
        viewport_w: f32,
        viewport_h: f32,
        orientation: Orientation,
    ) -> Vec<&Rule> {
        let mut out: Vec<&Rule> = self.rules.iter().collect();
        for mr in &self.media_rules {
            if mr.condition.matches(viewport_w, viewport_h, orientation) {
                out.extend(mr.rules.iter());
            }
        }
        out
    }

    /// **Round 17** — return the cascade as if every matching
    /// `@supports` block were inlined alongside the unconditional
    /// rules.
    ///
    /// Per CSS Conditional Rules L3 §2, an `@supports` block's inner
    /// rules participate in the cascade only when the runtime asserts
    /// support for every leaf `(prop: value)` declaration the
    /// condition requires. The parser has no opinion on what a given
    /// runtime supports; the caller passes a [`std::collections::HashSet`]
    /// of `(lowercase property name, value)` pairs that the runtime
    /// claims to handle, and this method walks each captured rule,
    /// evaluates its condition, and returns the merged rule list in
    /// source order — first the unconditional rules, then each
    /// matching `@supports` block's inner rules.
    ///
    /// Property name is compared case-insensitively (CSS property
    /// names are ASCII-case-insensitive per CSS Syntax L3 §4.2);
    /// value is compared verbatim after both sides are
    /// whitespace-trimmed by the caller (callers SHOULD pre-normalise
    /// to e.g. `" rotate(45deg) "` → `"rotate(45deg)"`). For
    /// "do you support this property at all?" tests, callers may pass
    /// the empty string as the value and a leaf
    /// [`SupportsCondition::Property`] with the same empty value will
    /// match — but condition leaves with the empty value are not
    /// emitted by the parser (CSS L3 §3 grammar requires both halves).
    pub fn resolve_for_supports_context(
        &self,
        supported: &std::collections::HashSet<(String, String)>,
    ) -> Vec<&Rule> {
        let mut out: Vec<&Rule> = self.rules.iter().collect();
        for sr in &self.supports_rules {
            if sr.condition.matches(supported) {
                out.extend(sr.rules.iter());
            }
        }
        out
    }

    /// Maximum `@import` recursion depth. Eight matches what major
    /// browsers cap at (Firefox + WebKit historically; see CSSOM
    /// implementation notes). Enough to load a typical theme tree
    /// without runaway expansion.
    pub const IMPORT_DEPTH_CAP: usize = 8;

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
    // `get(..6)` (rather than `r[..6]`) yields `None` when byte 6 falls
    // inside a multi-byte char — that input can't be the ASCII keyword
    // anyway, so it is correctly rejected instead of panicking.
    if !r
        .get(..6)
        .is_some_and(|kw| kw.eq_ignore_ascii_case("import"))
    {
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

/// Parse one `@font-face { ... }` block body (just the `prop: value;`
/// list inside the braces) per CSS Fonts L3 §4. Returns `None` only
/// when the body has no recognisable descriptors at all (the parser
/// is otherwise tolerant of malformed entries — matches the rest of
/// `parse_block` and lets a downstream font-resolver decide what to
/// do about partial info).
fn parse_at_font_face(body: &str) -> Option<FontFace> {
    let decls = parse_declarations(body);
    if decls.is_empty() {
        return None;
    }
    let mut face = FontFace::default();
    for (name, value) in decls {
        // `font-family` + `src` get the typed views; everything else
        // (font-weight / font-style / font-stretch / unicode-range /
        // font-display / …) is captured verbatim in `descriptors`.
        match name.as_str() {
            "font-family" => {
                face.family = unquote(value.trim()).trim().to_string();
            }
            "src" => {
                face.src = parse_font_src_list(&value);
            }
            _ => {}
        }
        face.descriptors.insert(name, value);
    }
    Some(face)
}

/// Parse the comma-separated value of `@font-face { src: ... }` per
/// CSS Fonts L3 §4.3. Each entry is one of:
///
/// - `url(<url>) [format("<hint>")]`
/// - `local(<name>)`
///
/// Quotes around URLs and names are stripped. Unrecognised entry
/// shapes are still appended (with all `Option` fields `None`) so
/// the typed list length matches the source list — callers that
/// need byte-identical fidelity reach for `descriptors["src"]`.
fn parse_font_src_list(s: &str) -> Vec<FontSource> {
    let mut out: Vec<FontSource> = Vec::new();
    // Comma-split must respect parens — `url(a,b)` shouldn't split.
    let mut start = 0usize;
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        match *b {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b',' if depth <= 0 => {
                let piece = s[start..i].trim();
                if !piece.is_empty() {
                    out.push(parse_font_src_entry(piece));
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = s[start..].trim();
    if !tail.is_empty() {
        out.push(parse_font_src_entry(tail));
    }
    out
}

fn parse_font_src_entry(s: &str) -> FontSource {
    let s = s.trim();
    // `local(...)` — name may or may not be quoted per §4.3.
    if let Some((inner, _tail)) = split_func_call(s, "local") {
        let name = unquote(inner.trim()).trim().to_string();
        return FontSource {
            url: None,
            format_hint: None,
            local_name: if name.is_empty() { None } else { Some(name) },
        };
    }
    // `url(...) [format(...)] [tech(...)]`. Only `url` and
    // `format` are captured; future descriptors fall through to the
    // raw `descriptors["src"]` blob.
    if let Some((inner, tail)) = split_func_call(s, "url") {
        let url = unquote(inner.trim()).trim().to_string();
        let tail = tail.trim();
        let format_hint = split_func_call(tail, "format")
            .map(|(arg, _)| unquote(arg.trim()).trim().to_string())
            .filter(|s| !s.is_empty());
        return FontSource {
            url: if url.is_empty() { None } else { Some(url) },
            format_hint,
            local_name: None,
        };
    }
    // Bare quoted string is treated as a URL (some legacy CSS).
    let bytes = s.as_bytes();
    if !bytes.is_empty() && (bytes[0] == b'"' || bytes[0] == b'\'') {
        let unq = unquote(s);
        if !unq.is_empty() {
            return FontSource {
                url: Some(unq),
                format_hint: None,
                local_name: None,
            };
        }
    }
    FontSource::default()
}

/// Round 15 — parse one `@keyframes <name> { sel { props } sel { props } }`
/// block. `prelude` is the text from the leading `@` up to (but not
/// including) the opening brace; `body` is the text between the
/// outer braces.
///
/// Per CSS Animations L1 §3:
///
/// - The animation name (after `@keyframes`) may be a quoted string
///   or an identifier; quotes are stripped.
/// - Each inner rule has a comma-separated selector list of
///   `from | to | <percent>%` offsets followed by a `{ ... }` block
///   of declarations.
/// - Multiple offsets in the same selector list expand to one
///   [`KeyframeSelector`] per offset (each carrying the same
///   declarations) so downstream code can iterate without re-parsing.
///
/// Returns `None` only when the rule has no recognisable name +
/// selectors (matches the rest of the parser's tolerance — a
/// malformed rule shouldn't kill the whole stylesheet).
fn parse_at_keyframes(prelude: &str, body: &str) -> Option<KeyframesRule> {
    // Prelude: `@keyframes <name>` (or `@-webkit-keyframes <name>`).
    // Strip the leading `@<keyword>` and then take whatever's left
    // as the animation name.
    let after_at = prelude.trim().strip_prefix('@')?;
    let mut iter = after_at.splitn(2, char::is_whitespace);
    let _keyword = iter.next()?;
    let name_raw = iter.next().unwrap_or("").trim();
    if name_raw.is_empty() {
        return None;
    }
    let name = unquote(name_raw).trim().to_string();
    if name.is_empty() {
        return None;
    }

    // Body: a sequence of `selector_list { declarations }` pairs.
    // No nested at-rules (CSS Animations L1 disallows them in
    // `@keyframes`), but `{` can still appear inside a `content:
    // "..."` declaration so we honour string boundaries.
    let mut selectors: Vec<KeyframeSelector> = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip leading whitespace.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // Read up to the next `{` (selector list).
        let sel_start = i;
        while i < bytes.len() && bytes[i] != b'{' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let sel_text = body[sel_start..i].trim();
        i += 1; // skip `{`
        let body_start = i;
        while i < bytes.len() && bytes[i] != b'}' {
            i += 1;
        }
        let body_end = i.min(bytes.len());
        let inner = &body[body_start..body_end];
        if i < bytes.len() {
            i += 1; // skip `}`
        }
        if sel_text.is_empty() {
            continue;
        }
        let decls = parse_declarations(inner);
        // Expand comma-separated offsets — one selector entry per
        // offset, each carrying a clone of the declarations.
        for piece in sel_text.split(',') {
            let p = piece.trim();
            if p.is_empty() {
                continue;
            }
            if let Some(offset) = parse_keyframe_offset(p) {
                selectors.push(KeyframeSelector {
                    offset,
                    declarations: decls.clone(),
                });
            }
        }
    }

    if selectors.is_empty() {
        // No usable keyframes — drop the rule rather than emitting an
        // empty entry.
        return None;
    }
    Some(KeyframesRule { name, selectors })
}

impl MediaCondition {
    /// Return `true` when at least one of the contained queries matches
    /// the supplied runtime context. An empty query list (no `@media`
    /// prelude) matches per CSS Media Queries L4 §3 — equivalent to
    /// the implicit `all` media type.
    pub fn matches(&self, viewport_w: f32, viewport_h: f32, orientation: Orientation) -> bool {
        if self.queries.is_empty() {
            return true;
        }
        self.queries
            .iter()
            .any(|q| q.matches(viewport_w, viewport_h, orientation))
    }
}

impl MediaQuery {
    /// Evaluate one media query against the runtime viewport. Per CSS
    /// Media Queries L4 §3, a query matches when:
    ///
    /// - the (optional) media type matches `screen` / `all`, AND
    /// - every feature clause matches, AND
    /// - the leading `not` modifier (if any) inverts the result.
    ///
    /// Unrecognised features ([`MediaValue::Raw`]) never match — the
    /// query containing them is dormant. `only` is honoured as a
    /// passthrough modifier per the spec's compat note.
    pub fn matches(&self, viewport_w: f32, viewport_h: f32, orientation: Orientation) -> bool {
        // Media type: only `screen`, `all` or omitted are accepted as
        // "this is a screen-style runtime". `print` never matches the
        // viewport-driven path.
        if let Some(t) = self.media_type.as_deref() {
            let lower = t.to_ascii_lowercase();
            if lower != "screen" && lower != "all" {
                return matches!(self.modifier, Some(MediaOperator::Not));
            }
        }
        let inner = self
            .features
            .iter()
            .all(|f| feature_matches(f, viewport_w, viewport_h, orientation));
        match self.modifier {
            Some(MediaOperator::Not) => !inner,
            _ => inner,
        }
    }
}

fn feature_matches(
    f: &MediaFeature,
    viewport_w: f32,
    viewport_h: f32,
    orientation: Orientation,
) -> bool {
    let name = f.name.as_str();
    let (base, range) = strip_min_max(name);
    let target = match base {
        "width" => viewport_w,
        "height" => viewport_h,
        "orientation" => {
            return match &f.value {
                MediaValue::Orientation(o) => *o == orientation,
                _ => false,
            };
        }
        _ => return false,
    };
    let v = match &f.value {
        MediaValue::Length(n) => *n,
        _ => return false,
    };
    // Honour the explicit `op` first (lets a hand-built `MediaFeature`
    // pick `MinEq` even with the bare name) and fall back to the
    // attribute-prefix-derived range when `op == Eq`.
    match (f.op, range) {
        (ComparisonOp::MinEq, _) | (ComparisonOp::Eq, ComparisonRange::Min) => target >= v,
        (ComparisonOp::MaxEq, _) | (ComparisonOp::Eq, ComparisonRange::Max) => target <= v,
        (ComparisonOp::Eq, ComparisonRange::Exact) => (target - v).abs() < 0.5,
    }
}

#[derive(Clone, Copy)]
enum ComparisonRange {
    Exact,
    Min,
    Max,
}

fn strip_min_max(name: &str) -> (&str, ComparisonRange) {
    if let Some(rest) = name.strip_prefix("min-") {
        return (rest, ComparisonRange::Min);
    }
    if let Some(rest) = name.strip_prefix("max-") {
        return (rest, ComparisonRange::Max);
    }
    (name, ComparisonRange::Exact)
}

impl SupportsCondition {
    /// Return `true` when this condition matches the supplied
    /// runtime supported-property set per CSS Conditional Rules L3
    /// §3. Property names are compared case-insensitively; values
    /// are compared verbatim (callers should pre-normalise
    /// whitespace).
    pub fn matches(&self, supported: &std::collections::HashSet<(String, String)>) -> bool {
        match self {
            SupportsCondition::Always => true,
            SupportsCondition::Property { name, value } => {
                let key = (name.to_ascii_lowercase(), value.trim().to_string());
                supported.contains(&key)
            }
            SupportsCondition::Not(inner) => !inner.matches(supported),
            SupportsCondition::And(items) => items.iter().all(|c| c.matches(supported)),
            SupportsCondition::Or(items) => items.iter().any(|c| c.matches(supported)),
        }
    }
}

/// Parse `@supports <prelude> { <inner-rules> }` per CSS Conditional
/// Rules L3 §2. Returns `None` when the body has no usable inner rules
/// (matches the rest of the parser's tolerance).
fn parse_at_supports(prelude: &str, body: &str) -> Option<SupportsRule> {
    let after_at = prelude.trim().strip_prefix('@')?;
    let mut iter = after_at.splitn(2, char::is_whitespace);
    let kw = iter.next()?;
    if !kw.eq_ignore_ascii_case("supports") {
        return None;
    }
    let condition_text = iter.next().unwrap_or("").trim();
    let condition = parse_supports_condition(condition_text).unwrap_or(SupportsCondition::Always);

    let mut nested = Stylesheet::new();
    nested.parse_block(body);
    if nested.rules.is_empty() {
        return None;
    }
    Some(SupportsRule {
        condition,
        rules: nested.rules,
    })
}

/// Parse a `<supports-condition>` per CSS Conditional Rules L3 §3.1.
///
/// Grammar (informal):
/// ```text
/// supports-condition  := not <supports-in-parens>
///                      | <supports-in-parens> [ ( and|or <supports-in-parens> )* ]
/// supports-in-parens  := ( <supports-condition> )
///                      | <supports-feature>
/// supports-feature    := <prop> : <value>
/// ```
/// Mixing `and` / `or` at the same level without explicit grouping is
/// forbidden by the spec; this parser accepts the leftmost operator
/// for the whole sequence and folds the rest into the same arm
/// (matches every browser's lenient behaviour).
fn parse_supports_condition(s: &str) -> Option<SupportsCondition> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // `not (...)` form — the outer keyword.
    if let Some(rest) = s.strip_prefix_ci("not") {
        let rest = rest.trim_start();
        if rest.starts_with('(') {
            let inner = parse_supports_in_parens(rest)?;
            return Some(SupportsCondition::Not(Box::new(inner.0)));
        }
    }
    // Otherwise: a `(...)` clause optionally followed by `and (...)`
    // or `or (...)` repetitions.
    let mut cur = s;
    let mut items: Vec<SupportsCondition> = Vec::new();
    let (first, mut tail) = parse_supports_in_parens(cur)?;
    items.push(first);
    cur = tail.trim_start();
    let mut combinator: Option<&str> = None;
    while !cur.is_empty() {
        // Expect `and` / `or`.
        let combo = if let Some(rest) = cur.strip_prefix_ci("and") {
            cur = rest.trim_start();
            "and"
        } else if let Some(rest) = cur.strip_prefix_ci("or") {
            cur = rest.trim_start();
            "or"
        } else {
            break;
        };
        if let Some(prev) = combinator {
            if prev != combo {
                // Mixing — bail and use what we have.
                break;
            }
        }
        combinator = Some(combo);
        let (next, t) = parse_supports_in_parens(cur)?;
        items.push(next);
        tail = t;
        cur = tail.trim_start();
    }
    if items.len() == 1 {
        return Some(items.into_iter().next().unwrap());
    }
    match combinator {
        Some("and") => Some(SupportsCondition::And(items)),
        Some("or") => Some(SupportsCondition::Or(items)),
        _ => Some(items.into_iter().next().unwrap()),
    }
}

/// Parse a `(supports-in-parens)` chunk and return the parsed
/// condition plus the remaining input slice past the closing paren.
fn parse_supports_in_parens(s: &str) -> Option<(SupportsCondition, &str)> {
    let s = s.trim_start();
    if !s.starts_with('(') {
        return None;
    }
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut close = None;
    for (i, b) in bytes.iter().enumerate() {
        match *b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let inner = &s[1..close];
    let tail = &s[close + 1..];
    // Inner is either a nested `<supports-condition>` (recognised by
    // `not` or another `(`) or a `<supports-feature>` leaf.
    let inner_trim = inner.trim();
    if inner_trim.starts_with('(')
        || inner_trim
            .strip_prefix_ci("not")
            .map(|r| r.trim_start().starts_with('('))
            .unwrap_or(false)
    {
        if let Some(c) = parse_supports_condition(inner_trim) {
            return Some((c, tail));
        }
    }
    if let Some(c) = parse_supports_feature(inner_trim) {
        return Some((c, tail));
    }
    None
}

/// Parse one `<supports-feature>` — `prop: value` per L3 §3.
fn parse_supports_feature(s: &str) -> Option<SupportsCondition> {
    let colon = s.find(':')?;
    let name = s[..colon].trim().to_ascii_lowercase();
    let value = s[colon + 1..].trim().to_string();
    if name.is_empty() || value.is_empty() {
        return None;
    }
    Some(SupportsCondition::Property { name, value })
}

trait StripPrefixCi {
    fn strip_prefix_ci<'a>(&'a self, prefix: &str) -> Option<&'a str>;
}

impl StripPrefixCi for str {
    fn strip_prefix_ci<'a>(&'a self, prefix: &str) -> Option<&'a str> {
        if self.len() < prefix.len() {
            return None;
        }
        let head = &self[..prefix.len()];
        if head.eq_ignore_ascii_case(prefix) {
            Some(&self[prefix.len()..])
        } else {
            None
        }
    }
}

/// Parse `@media <prelude> { <inner-rules> }` per CSS Media Queries L4
/// §2 / §3. Returns `None` when the body has no usable inner rules
/// (matches the rest of the parser's tolerance).
fn parse_at_media(prelude: &str, body: &str) -> Option<MediaRule> {
    let after_at = prelude.trim().strip_prefix('@')?;
    // Strip the leading `media` keyword (case-insensitive).
    let mut iter = after_at.splitn(2, char::is_whitespace);
    let kw = iter.next()?;
    if !kw.eq_ignore_ascii_case("media") {
        return None;
    }
    let condition_text = iter.next().unwrap_or("").trim();
    let condition = parse_media_condition(condition_text);

    // Parse the inner block as a fresh stylesheet, then take its
    // unconditional `rules`. Nested at-rules inside `@media` are
    // tolerated — Media Queries L4 §2 disallows `@media` nesting in
    // CSS 2.1 but L4 lifts that — we drop any nested at-rules to keep
    // the surface small.
    let mut nested = Stylesheet::new();
    nested.parse_block(body);
    if nested.rules.is_empty() {
        return None;
    }
    Some(MediaRule {
        condition,
        rules: nested.rules,
    })
}

/// Parse a media-condition prelude into typed [`MediaQuery`] entries.
/// An empty prelude returns an empty query list (which matches per the
/// `MediaCondition::matches` implicit-`all` rule).
fn parse_media_condition(s: &str) -> MediaCondition {
    let s = s.trim();
    if s.is_empty() {
        return MediaCondition::default();
    }
    let mut queries: Vec<MediaQuery> = Vec::new();
    for piece in split_media_queries(s) {
        if let Some(q) = parse_one_media_query(piece.trim()) {
            queries.push(q);
        }
    }
    MediaCondition { queries }
}

/// Split the media-query-list on top-level commas (commas inside
/// `(...)` parens are kept inside the clause they belong to).
fn split_media_queries(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0usize;
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        match *b {
            b'(' => depth += 1,
            b')' if depth > 0 => depth -= 1,
            b',' if depth == 0 => {
                out.push(s[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = &s[start..];
    if !tail.trim().is_empty() {
        out.push(tail.to_string());
    }
    out
}

fn parse_one_media_query(s: &str) -> Option<MediaQuery> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut q = MediaQuery::default();
    // Scan tokens left to right, picking up `not` / `only` / a media
    // type, then the `and (feature: value)` clauses.
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut saw_modifier_or_type = false;
    while i < bytes.len() {
        // Skip whitespace.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // Feature clause `(name : value)` or bare `(name)`.
        if bytes[i] == b'(' {
            let close = memchr_close_paren(bytes, i + 1)?;
            let inner = &s[i + 1..close];
            if let Some(feat) = parse_media_feature(inner) {
                q.features.push(feat);
            } else {
                // Unrecognised feature shape — keep the raw text so the
                // query is dormant (never matches) instead of dropping
                // the whole `@media` block.
                q.features.push(MediaFeature {
                    name: inner.trim().to_ascii_lowercase(),
                    op: ComparisonOp::Eq,
                    value: MediaValue::Raw(String::new()),
                });
            }
            i = close + 1;
            continue;
        }
        // Otherwise it's a keyword: `not` / `only` / a media type, or
        // the `and` glue between features.
        let word_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'('
            && bytes[i] != b','
        {
            i += 1;
        }
        let word = &s[word_start..i];
        let lower = word.to_ascii_lowercase();
        match lower.as_str() {
            "and" => continue,
            "not" if !saw_modifier_or_type => {
                q.modifier = Some(MediaOperator::Not);
                saw_modifier_or_type = true;
            }
            "only" if !saw_modifier_or_type => {
                q.modifier = Some(MediaOperator::Only);
                saw_modifier_or_type = true;
            }
            _ => {
                if !saw_modifier_or_type && q.media_type.is_none() {
                    q.media_type = Some(lower);
                    saw_modifier_or_type = true;
                }
                // Otherwise drop the unrecognised token — the parser is
                // tolerant of malformed input per the rest of `css.rs`.
            }
        }
    }
    Some(q)
}

fn parse_media_feature(s: &str) -> Option<MediaFeature> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Bare `(feature)` form — not used by width/height/orientation but
    // tolerated for forward compat.
    let (name_raw, value_raw) = match s.find(':') {
        Some(c) => (s[..c].trim(), s[c + 1..].trim()),
        None => (s, ""),
    };
    let name = name_raw.to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    let (base, range) = strip_min_max(&name);
    let op = match range {
        ComparisonRange::Exact => ComparisonOp::Eq,
        ComparisonRange::Min => ComparisonOp::MinEq,
        ComparisonRange::Max => ComparisonOp::MaxEq,
    };
    let value = match base {
        "orientation" => match value_raw.to_ascii_lowercase().as_str() {
            "portrait" => MediaValue::Orientation(Orientation::Portrait),
            "landscape" => MediaValue::Orientation(Orientation::Landscape),
            _ => MediaValue::Raw(value_raw.to_string()),
        },
        "width" | "height" => match parse_media_length(value_raw) {
            Some(n) => MediaValue::Length(n),
            None => MediaValue::Raw(value_raw.to_string()),
        },
        _ => MediaValue::Raw(value_raw.to_string()),
    };
    Some(MediaFeature { name, op, value })
}

/// Parse a `<length>` per CSS Values L4 — a number plus an optional
/// unit. Round 16 treats every unit as user units (matches the rest of
/// the SVG-side number parser); pixel-perfect unit conversion is
/// future work.
fn parse_media_length(s: &str) -> Option<f32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let mut saw_digit = false;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
        saw_digit = true;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
            saw_digit = true;
        }
    }
    if !saw_digit {
        return None;
    }
    s[..i].parse::<f32>().ok()
}

/// Parse one keyframe offset — `from`, `to`, or `<percent>%`.
fn parse_keyframe_offset(s: &str) -> Option<KeyframeOffset> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("from") {
        return Some(KeyframeOffset::From);
    }
    if t.eq_ignore_ascii_case("to") {
        return Some(KeyframeOffset::To);
    }
    let stripped = t.strip_suffix('%')?;
    let n: f32 = stripped.trim().parse().ok()?;
    Some(KeyframeOffset::Percent(n))
}

/// If `s` starts with `name(`, return `(arg, tail)` where `arg` is
/// the substring inside the matching parens and `tail` is everything
/// after the closing paren. Case-insensitive on `name`. Returns
/// `None` when `s` doesn't begin with `name(...)` or the parens are
/// unbalanced. Tracks paren depth so a nested `(` inside the
/// argument doesn't trip the matcher.
fn split_func_call<'a>(s: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    let s = s.trim_start();
    let head = s.get(..name.len())?;
    if !head.eq_ignore_ascii_case(name) {
        return None;
    }
    let after = s[name.len()..].strip_prefix('(')?;
    let bytes = after.as_bytes();
    let mut depth = 1i32;
    for (i, b) in bytes.iter().enumerate() {
        match *b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    let inner = &after[..i];
                    let tail = &after[i + 1..];
                    return Some((inner, tail));
                }
            }
            _ => {}
        }
    }
    None
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

    // ----- Round 17: @supports parsing + evaluation tests -------------

    fn supported(set: &[(&str, &str)]) -> std::collections::HashSet<(String, String)> {
        set.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn parse_at_supports_captures_block() {
        let mut s = Stylesheet::new();
        s.parse_block(
            "@supports (transform: rotate(45deg)) { rect { fill: red } } .x { stroke: blue }",
        );
        // Inner rule is conditional and stays out of the unconditional set.
        assert_eq!(s.rules.len(), 1, "only .x is unconditional");
        assert_eq!(s.supports_rules.len(), 1);
        let sr = &s.supports_rules[0];
        match &sr.condition {
            SupportsCondition::Property { name, value } => {
                assert_eq!(name, "transform");
                assert_eq!(value, "rotate(45deg)");
            }
            other => panic!("expected Property leaf, got {other:?}"),
        }
        assert_eq!(sr.rules.len(), 1);
    }

    #[test]
    fn supports_resolution_includes_only_matching_rules() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @supports (transform: rotate(45deg)) { rect { fill: red } }
            @supports (display: grid) { circle { fill: green } }
            .x { stroke: blue }
            "#,
        );
        let supported_set = supported(&[("transform", "rotate(45deg)")]);
        let merged = s.resolve_for_supports_context(&supported_set);
        // Unconditional `.x` plus the matching `rect` rule (but not `circle`).
        assert_eq!(merged.len(), 2);
        // The unconditional rules are first, then the matching @supports.
        assert!(merged[0].selectors[0].head.classes.contains(&"x".into()));
        assert_eq!(merged[1].selectors[0].head.tag.as_deref(), Some("rect"));
    }

    #[test]
    fn supports_not_negates_inner_condition() {
        let mut s = Stylesheet::new();
        s.parse_block("@supports not (display: grid) { .legacy { fill: red } }");
        assert_eq!(s.supports_rules.len(), 1);
        match &s.supports_rules[0].condition {
            SupportsCondition::Not(inner) => match &**inner {
                SupportsCondition::Property { name, value } => {
                    assert_eq!(name, "display");
                    assert_eq!(value, "grid");
                }
                other => panic!("expected Property inside Not, got {other:?}"),
            },
            other => panic!("expected Not, got {other:?}"),
        }
        // With grid supported → does NOT match.
        let supported_set = supported(&[("display", "grid")]);
        let merged = s.resolve_for_supports_context(&supported_set);
        assert_eq!(merged.len(), 0);
        // Without grid → matches.
        let supported_set = supported(&[]);
        let merged = s.resolve_for_supports_context(&supported_set);
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn supports_and_combinator_requires_all() {
        let mut s = Stylesheet::new();
        s.parse_block("@supports (display: grid) and (gap: 1px) { .modern { fill: red } }");
        assert_eq!(s.supports_rules.len(), 1);
        match &s.supports_rules[0].condition {
            SupportsCondition::And(items) => assert_eq!(items.len(), 2),
            other => panic!("expected And, got {other:?}"),
        }
        let both = supported(&[("display", "grid"), ("gap", "1px")]);
        assert_eq!(s.resolve_for_supports_context(&both).len(), 1);
        let one = supported(&[("display", "grid")]);
        assert_eq!(s.resolve_for_supports_context(&one).len(), 0);
    }

    #[test]
    fn supports_or_combinator_requires_any() {
        let mut s = Stylesheet::new();
        s.parse_block("@supports (display: grid) or (display: flex) { .modern { fill: red } }");
        match &s.supports_rules[0].condition {
            SupportsCondition::Or(items) => assert_eq!(items.len(), 2),
            other => panic!("expected Or, got {other:?}"),
        }
        let flex_only = supported(&[("display", "flex")]);
        assert_eq!(s.resolve_for_supports_context(&flex_only).len(), 1);
        let neither = supported(&[]);
        assert_eq!(s.resolve_for_supports_context(&neither).len(), 0);
    }

    #[test]
    fn supports_property_match_is_case_insensitive_on_name() {
        let mut s = Stylesheet::new();
        s.parse_block("@supports (Transform: rotate(45deg)) { .x { fill: red } }");
        let set = supported(&[("transform", "rotate(45deg)")]);
        assert_eq!(s.resolve_for_supports_context(&set).len(), 1);
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
