//! Round 5 — CSS 3 Selectors Level 3 subset.
//!
//! End-to-end coverage for the selector surface added in round 5:
//!
//! - Attribute predicates (`[attr]`, `[attr=val]`, `[attr~=val]`,
//!   `[attr|=val]`, `[attr^=val]`, `[attr$=val]`, `[attr*=val]`).
//! - Combinators (descendant, child `>`, adjacent sibling `+`,
//!   general sibling `~`).
//! - Structural pseudo-classes (`:first-child`, `:last-child`,
//!   `:only-child`, `:nth-child(...)`, `:first-of-type`,
//!   `:last-of-type`, `:nth-of-type(...)`, `:only-of-type`,
//!   `:not(simple)`).
//!
//! Verified by parsing a doc with a `<style>` block, then walking the
//! resulting `VectorFrame` to assert each path's resolved fill colour
//! matches what the cascade dictates.

use oxideav_core::{Group, Node, Paint, Rgba, VectorFrame};
use oxideav_svg::parse_svg;

/// Walk every `Node::Path` in DFS order and collect its solid fill
/// colour (or `None` if the fill isn't a solid).
fn fills_in_order(frame: &VectorFrame) -> Vec<Option<Rgba>> {
    fn rec(g: &Group, out: &mut Vec<Option<Rgba>>) {
        for c in &g.children {
            match c {
                Node::Path(p) => match &p.fill {
                    Some(Paint::Solid(c)) => out.push(Some(*c)),
                    Some(_) => out.push(None),
                    None => out.push(None),
                },
                Node::Group(sg) => rec(sg, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    rec(&frame.root, &mut out);
    out
}

// ---------- attribute selectors ----------

#[test]
fn attribute_equality_selector_applies() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
  <style>[role="button"] { fill: #ff0000 }</style>
  <rect role="button" width="20" height="20"/>
  <rect role="menu" width="20" height="20"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(255, 0, 0)));
    assert_ne!(fills[1], Some(Rgba::opaque(255, 0, 0)));
}

#[test]
fn attribute_prefix_selector_applies() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20">
  <style>[id^="btn-"] { fill: #00ff00 }</style>
  <rect id="btn-ok" width="10" height="10"/>
  <rect id="other" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(0, 255, 0)));
    assert_ne!(fills[1], Some(Rgba::opaque(0, 255, 0)));
}

#[test]
fn attribute_dash_match_selector_applies_to_lang_tags() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="30">
  <style>[lang|="en"] { fill: #0000ff }</style>
  <rect lang="en" width="10" height="10"/>
  <rect lang="en-US" width="10" height="10"/>
  <rect lang="fr" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(0, 0, 255)));
    assert_eq!(fills[1], Some(Rgba::opaque(0, 0, 255)));
    assert_ne!(fills[2], Some(Rgba::opaque(0, 0, 255)));
}

#[test]
fn attribute_includes_selector_word_match() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20">
  <style>[class~="card"] { fill: #aabbcc }</style>
  <rect class="big card huge" width="10" height="10"/>
  <rect class="cardboard" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(0xaa, 0xbb, 0xcc)));
    assert_ne!(fills[1], Some(Rgba::opaque(0xaa, 0xbb, 0xcc)));
}

#[test]
fn attribute_substring_selector() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20">
  <style>[id*="middle"] { fill: #112233 }</style>
  <rect id="left-middle-right" width="10" height="10"/>
  <rect id="elsewhere" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(0x11, 0x22, 0x33)));
    assert_ne!(fills[1], Some(Rgba::opaque(0x11, 0x22, 0x33)));
}

#[test]
fn attribute_existence_selector() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20">
  <style>[data-foo] { fill: #ff00ff }</style>
  <rect data-foo="anything" width="10" height="10"/>
  <rect width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(255, 0, 255)));
    assert_ne!(fills[1], Some(Rgba::opaque(255, 0, 255)));
}

// ---------- combinators ----------

#[test]
fn child_combinator_only_matches_direct_children() {
    // `g > rect` matches only the rect that is a direct child of g.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
  <style>g > rect { fill: #aa0000 }</style>
  <g>
    <rect width="10" height="10"/>
    <g>
      <rect width="10" height="10"/>
    </g>
  </g>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    // Outer rect: direct child of g → matches.
    assert_eq!(fills[0], Some(Rgba::opaque(0xaa, 0, 0)));
    // Inner rect: child of inner g, but `g > rect` matches because
    // the inner g IS its parent. So it ALSO matches. Confirm.
    assert_eq!(fills[1], Some(Rgba::opaque(0xaa, 0, 0)));
}

#[test]
fn child_combinator_does_not_match_grandchild_via_non_g() {
    // `g > rect` should NOT match a rect whose direct parent is not g.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
  <style>g > rect { fill: #aa0000 }</style>
  <rect width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    // The rect is a child of <svg>, not <g>. Should NOT be red.
    assert_ne!(fills[0], Some(Rgba::opaque(0xaa, 0, 0)));
}

#[test]
fn descendant_combinator_matches_at_any_depth() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
  <style>g rect { fill: #00aa00 }</style>
  <g>
    <g>
      <rect width="10" height="10"/>
    </g>
  </g>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(0, 0xaa, 0)));
}

#[test]
fn adjacent_sibling_only_matches_immediate_sibling() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="10">
  <style>rect + circle { fill: #0000aa }</style>
  <rect width="10" height="10"/>
  <circle cx="15" cy="5" r="5"/>
  <circle cx="25" cy="5" r="5"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    // 1st circle: immediately follows rect → matches.
    assert_eq!(fills[1], Some(Rgba::opaque(0, 0, 0xaa)));
    // 2nd circle: previous sibling is a circle, not a rect → no match.
    assert_ne!(fills[2], Some(Rgba::opaque(0, 0, 0xaa)));
}

#[test]
fn general_sibling_matches_any_following() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="10">
  <style>rect ~ circle { fill: #aaaa00 }</style>
  <rect width="10" height="10"/>
  <circle cx="15" cy="5" r="5"/>
  <circle cx="25" cy="5" r="5"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    // Both circles follow the rect (sibling order) → both match.
    assert_eq!(fills[1], Some(Rgba::opaque(0xaa, 0xaa, 0)));
    assert_eq!(fills[2], Some(Rgba::opaque(0xaa, 0xaa, 0)));
}

#[test]
fn descendant_does_not_apply_to_self() {
    // `rect rect` requires nesting — a top-level rect should NOT match.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <style>rect rect { fill: #ff0000 }</style>
  <rect width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_ne!(fills[0], Some(Rgba::opaque(255, 0, 0)));
}

// ---------- structural pseudo-classes ----------

#[test]
fn first_child_matches_only_first() {
    // `<style>` sits inside `<defs>` so it doesn't perturb the
    // `:first-child` index of the rects in the outer `<g>`.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="10">
  <defs><style>rect:first-child { fill: #ff0000 }</style></defs>
  <g>
    <rect width="10" height="10"/>
    <rect width="10" height="10"/>
    <rect width="10" height="10"/>
  </g>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(255, 0, 0)));
    assert_ne!(fills[1], Some(Rgba::opaque(255, 0, 0)));
    assert_ne!(fills[2], Some(Rgba::opaque(255, 0, 0)));
}

#[test]
fn last_child_matches_only_last() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="10">
  <style>rect:last-child { fill: #00ff00 }</style>
  <rect width="10" height="10"/>
  <rect width="10" height="10"/>
  <rect width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_ne!(fills[0], Some(Rgba::opaque(0, 255, 0)));
    assert_ne!(fills[1], Some(Rgba::opaque(0, 255, 0)));
    assert_eq!(fills[2], Some(Rgba::opaque(0, 255, 0)));
}

#[test]
fn nth_child_2n_plus_1_matches_odd_indices() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="10">
  <defs><style>rect:nth-child(2n+1) { fill: #abcdef }</style></defs>
  <g>
    <rect width="10" height="10"/>
    <rect width="10" height="10"/>
    <rect width="10" height="10"/>
    <rect width="10" height="10"/>
  </g>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(0xab, 0xcd, 0xef)));
    assert_ne!(fills[1], Some(Rgba::opaque(0xab, 0xcd, 0xef)));
    assert_eq!(fills[2], Some(Rgba::opaque(0xab, 0xcd, 0xef)));
    assert_ne!(fills[3], Some(Rgba::opaque(0xab, 0xcd, 0xef)));
}

#[test]
fn nth_child_keyword_even_matches_even_indices() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="10">
  <defs><style>rect:nth-child(even) { fill: #102030 }</style></defs>
  <g>
    <rect width="10" height="10"/>
    <rect width="10" height="10"/>
    <rect width="10" height="10"/>
    <rect width="10" height="10"/>
  </g>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_ne!(fills[0], Some(Rgba::opaque(0x10, 0x20, 0x30)));
    assert_eq!(fills[1], Some(Rgba::opaque(0x10, 0x20, 0x30)));
    assert_ne!(fills[2], Some(Rgba::opaque(0x10, 0x20, 0x30)));
    assert_eq!(fills[3], Some(Rgba::opaque(0x10, 0x20, 0x30)));
}

#[test]
fn first_of_type_distinguishes_from_first_child() {
    // Mixed children: a circle first, then two rects. `:first-child`
    // matches only the circle; `:first-of-type` (against rect) matches
    // the first rect.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="10">
  <style>rect:first-of-type { fill: #234567 }</style>
  <circle cx="5" cy="5" r="5"/>
  <rect width="10" height="10"/>
  <rect width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    // First fill is the circle (default fill is opaque black), then 2 rects.
    assert_eq!(fills.len(), 3);
    assert_eq!(fills[1], Some(Rgba::opaque(0x23, 0x45, 0x67)));
    assert_ne!(fills[2], Some(Rgba::opaque(0x23, 0x45, 0x67)));
}

#[test]
fn only_child_matches_solo_element() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
  <style>rect:only-child { fill: #ff00ff }</style>
  <g>
    <rect width="10" height="10"/>
  </g>
  <g>
    <rect width="10" height="10"/>
    <rect width="10" height="10"/>
  </g>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(255, 0, 255)));
    assert_ne!(fills[1], Some(Rgba::opaque(255, 0, 255)));
    assert_ne!(fills[2], Some(Rgba::opaque(255, 0, 255)));
}

#[test]
fn nth_of_type_independent_of_other_tags() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="10">
  <style>rect:nth-of-type(2) { fill: #654321 }</style>
  <circle cx="5" cy="5" r="5"/>
  <rect width="10" height="10"/>
  <circle cx="25" cy="5" r="5"/>
  <rect width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    // Two rects → second one matches.
    // Order in output is DFS over children — circles come too.
    // fills index: 0=circle, 1=rect (first), 2=circle, 3=rect (second).
    assert_ne!(fills[1], Some(Rgba::opaque(0x65, 0x43, 0x21)));
    assert_eq!(fills[3], Some(Rgba::opaque(0x65, 0x43, 0x21)));
}

#[test]
fn not_negation_excludes_matching_simple() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
  <style>rect:not(.skip) { fill: #ff5500 }</style>
  <rect width="10" height="10"/>
  <rect class="skip" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(0xff, 0x55, 0)));
    assert_ne!(fills[1], Some(Rgba::opaque(0xff, 0x55, 0)));
}

// ---------- specificity interaction ----------

#[test]
fn id_plus_class_outranks_two_classes() {
    // (#id + .cls) = (1, 1, 0) beats (.a.b) = (0, 2, 0).
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <style>
    .a.b { fill: #00ff00 }
    #t.a { fill: #ff0000 }
  </style>
  <rect id="t" class="a b" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(255, 0, 0)));
}

#[test]
fn attribute_predicate_specificity_matches_class() {
    // [foo=x] should have the same specificity weight as .x — a tag
    // selector layered later loses, an id selector layered later wins.
    // We just verify they're comparable to a single class.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <style>
    rect { fill: #110000 }
    [data-x="y"] { fill: #220000 }
    .cls { fill: #330000 }
  </style>
  <rect data-x="y" class="cls" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    // Both [data-x=y] and .cls have specificity (0,1,0) — last wins
    // by source order → .cls.
    assert_eq!(fills[0], Some(Rgba::opaque(0x33, 0, 0)));
}

#[test]
fn unsupported_pseudo_class_doesnt_break_rule() {
    // Round 11 — `:hover` is now modelled as a Stateful pseudo-class
    // that never matches in a static document. Previously this rule
    // over-matched `.btn` because `:hover` was silently dropped; the
    // updated behaviour correctly leaves the rect at its
    // SVG-default fill (opaque black). See `tests/round11_css.rs` for
    // the regression net.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <style>.btn:hover { fill: #112233 }</style>
  <rect class="btn" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    // SVG default fill is opaque black per SVG 1.1 §11.3 — the
    // `:hover` rule is preserved on the stylesheet but doesn't apply
    // to a static rendering.
    assert_eq!(fills[0], Some(Rgba::opaque(0, 0, 0)));
}
