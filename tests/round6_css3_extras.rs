//! Round 6 — CSS 3 Selectors L3 leftovers + SVG 2 `d` as a presentation
//! property.
//!
//! End-to-end coverage for:
//!
//! - `:nth-last-child(An+B)` and `:nth-last-of-type(An+B)` —
//!   structural pseudo-classes counted from the *end* of the parent's
//!   element-children list.
//! - `:lang(L)` — BCP 47 dash-match against the element's nearest
//!   `xml:lang` / `lang` attribute (walked up the ancestor chain).
//! - SVG 2 §9.3.2 — the `d` CSS property overrides the element's `d`
//!   attribute via the normal cascade.
//!
//! Verified by parsing a doc with a `<style>` block, then walking the
//! resulting `VectorFrame` to assert each path's resolved fill colour
//! (or path geometry, for the `d` property tests) matches what the
//! cascade dictates.

use oxideav_core::{Group, Node, Paint, PathCommand, Rgba, VectorFrame};
use oxideav_svg::parse_svg;

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

/// Walk every `Node::Path` in DFS order, returning each path's command
/// list. Used by the `d`-property tests to verify the path geometry
/// came from CSS rather than the `d` attribute.
fn path_commands_in_order(frame: &VectorFrame) -> Vec<Vec<PathCommand>> {
    fn rec(g: &Group, out: &mut Vec<Vec<PathCommand>>) {
        for c in &g.children {
            match c {
                Node::Path(p) => out.push(p.path.commands.clone()),
                Node::Group(sg) => rec(sg, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    rec(&frame.root, &mut out);
    out
}

// ---------- :nth-last-child / :nth-last-of-type ----------

#[test]
fn nth_last_child_matches_last_element() {
    // Five rects; `:nth-last-child(1)` matches only the last.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="10">
  <style>:nth-last-child(1) { fill: #ff0000 }</style>
  <rect x="0"  y="0" width="10" height="10"/>
  <rect x="10" y="0" width="10" height="10"/>
  <rect x="20" y="0" width="10" height="10"/>
  <rect x="30" y="0" width="10" height="10"/>
  <rect x="40" y="0" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills.len(), 5);
    for f in &fills[..4] {
        assert_ne!(*f, Some(Rgba::opaque(255, 0, 0)));
    }
    assert_eq!(fills[4], Some(Rgba::opaque(255, 0, 0)));
}

#[test]
fn nth_last_child_keyword_odd_matches_from_end() {
    // For 4 element-children, `:nth-last-child(odd)` matches indices
    // counted from the end where the last-from-end is 1, then 3, 5, …
    // — so children at positions 1 and 3 (1-indexed from start), i.e.
    // 1st and 3rd from the start since 4-1+1=4, 4-3+1=2 → wait, let me
    // recompute: child_index 0 → last_idx=4 (even); child_index 1 →
    // last_idx=3 (odd) MATCH; child_index 2 → last_idx=2 (even);
    // child_index 3 → last_idx=1 (odd) MATCH.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="10">
  <style>:nth-last-child(odd) { fill: #00ff00 }</style>
  <rect x="0"  y="0" width="10" height="10"/>
  <rect x="10" y="0" width="10" height="10"/>
  <rect x="20" y="0" width="10" height="10"/>
  <rect x="30" y="0" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    let target = Some(Rgba::opaque(0, 255, 0));
    assert_ne!(fills[0], target); // last_idx=4
    assert_eq!(fills[1], target); // last_idx=3
    assert_ne!(fills[2], target); // last_idx=2
    assert_eq!(fills[3], target); // last_idx=1
}

#[test]
fn nth_last_of_type_independent_of_other_tags() {
    // 3 <rect> + 2 <circle>, mixed. `:nth-last-of-type(1)` for rect
    // matches the last rect (index 2 from start), not the global last
    // child.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="20">
  <style>rect:nth-last-of-type(1) { fill: #0000ff }</style>
  <rect x="0"  y="0" width="10" height="10"/>
  <circle cx="15" cy="5" r="5"/>
  <rect x="20" y="0" width="10" height="10"/>
  <rect x="30" y="0" width="10" height="10"/>
  <circle cx="45" cy="5" r="5"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    let target = Some(Rgba::opaque(0, 0, 255));
    // DFS order: rect, circle, rect, rect, circle.
    assert_ne!(fills[0], target); // 1st rect (of-type idx 0)
    assert_ne!(fills[2], target); // 2nd rect (of-type idx 1)
    assert_eq!(fills[3], target); // 3rd rect (of-type idx 2 → last)
}

// ---------- :lang ----------

#[test]
fn lang_pseudo_matches_exact_tag() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
  <style>:lang(en) { fill: #aabbcc }</style>
  <rect lang="en" x="0"  y="0" width="10" height="10"/>
  <rect lang="fr" x="10" y="0" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(0xaa, 0xbb, 0xcc)));
    assert_ne!(fills[1], Some(Rgba::opaque(0xaa, 0xbb, 0xcc)));
}

#[test]
fn lang_pseudo_dash_matches_subtag() {
    // `:lang(en)` should match `lang="en-US"` (BCP 47 dash-match).
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="10">
  <style>:lang(en) { fill: #112233 }</style>
  <rect lang="en"     x="0"  y="0" width="10" height="10"/>
  <rect lang="en-US"  x="10" y="0" width="10" height="10"/>
  <rect lang="english" x="20" y="0" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    let target = Some(Rgba::opaque(0x11, 0x22, 0x33));
    assert_eq!(fills[0], target);
    assert_eq!(fills[1], target);
    // "english" is NOT a dash-match for "en" — the boundary char must
    // be `-` exactly.
    assert_ne!(fills[2], target);
}

#[test]
fn lang_pseudo_inherits_from_ancestor() {
    // The `<g>` carries `xml:lang`; the inner rect inherits it for the
    // purposes of `:lang(...)` per Selectors L3 §6.6.2.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <style>:lang(ja) { fill: #ffaa00 }</style>
  <g xml:lang="ja-JP">
    <rect x="0" y="0" width="10" height="10"/>
  </g>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_eq!(fills[0], Some(Rgba::opaque(0xff, 0xaa, 0x00)));
}

#[test]
fn lang_pseudo_no_attribute_does_not_match() {
    // No `lang` / `xml:lang` anywhere → `:lang(en)` cannot match.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <style>:lang(en) { fill: #ff0000 }</style>
  <rect x="0" y="0" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fills = fills_in_order(&frame);
    assert_ne!(fills[0], Some(Rgba::opaque(255, 0, 0)));
}

// ---------- SVG 2 `d` as a presentation property ----------

#[test]
fn css_d_property_overrides_attribute() {
    // The `d` attribute draws a line to (5, 5); the CSS rule overrides
    // it to draw to (50, 50). The resolved geometry should be the
    // CSS one.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <style>path { d: path("M 0 0 L 50 50") }</style>
  <path d="M 0 0 L 5 5"/>
</svg>"##;
    // Some CSS dialects use `path("...")`; our parser accepts a bare
    // quoted string (`d: "M 0 0 L 50 50"`). Use that simpler form.
    let _ = src; // silence the explanatory comment block.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <style>path { d: "M 0 0 L 50 50" }</style>
  <path d="M 0 0 L 5 5"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let cmds = path_commands_in_order(&frame);
    assert_eq!(cmds.len(), 1);
    // 2 commands: MoveTo + LineTo to (50, 50).
    assert_eq!(cmds[0].len(), 2);
    match cmds[0][1] {
        PathCommand::LineTo(p) => {
            assert!((p.x - 50.0).abs() < 1e-4, "got x={}", p.x);
            assert!((p.y - 50.0).abs() < 1e-4, "got y={}", p.y);
        }
        ref other => panic!("expected LineTo, got {:?}", other),
    }
}

#[test]
fn css_d_property_inline_style_wins() {
    // Inline `style="..."` is the highest-specificity surface; it
    // should override both the attribute and the matched rule.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <style>path { d: "M 0 0 L 30 30" }</style>
  <path d="M 0 0 L 5 5" style='d: "M 0 0 L 99 99"'/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let cmds = path_commands_in_order(&frame);
    assert_eq!(cmds.len(), 1);
    match cmds[0][1] {
        PathCommand::LineTo(p) => {
            assert!((p.x - 99.0).abs() < 1e-4);
            assert!((p.y - 99.0).abs() < 1e-4);
        }
        ref other => panic!("expected LineTo, got {:?}", other),
    }
}

#[test]
fn css_d_none_drops_path() {
    // SVG 2: `d: none` means "no rendering". The path produces no
    // visible scene-graph node, so the document has zero paths.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <style>path { d: none }</style>
  <path d="M 0 0 L 5 5"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let cmds = path_commands_in_order(&frame);
    assert_eq!(cmds.len(), 0);
}

#[test]
fn css_d_property_does_not_affect_non_path_elements() {
    // The `d` property only applies to `<path>` per the SVG 2 propdef;
    // any `d` declaration on a `<rect>` is harmlessly ignored — the
    // rect's `width`/`height` still produce a rectangle.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
  <style>rect { d: "M 0 0 L 5 5" }</style>
  <rect x="0" y="0" width="20" height="20"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let cmds = path_commands_in_order(&frame);
    // The rect produces a normal rectangular path (5 commands:
    // M, L, L, L, Z).
    assert_eq!(cmds.len(), 1);
    assert!(
        cmds[0].len() >= 4,
        "rect path should remain rectangle-shaped, got {} cmds",
        cmds[0].len()
    );
}
