//! Round 205 — SVG 2 §13.8 `paint-order` integration tests.
//!
//! `paint-order: normal | [ fill || stroke || markers ]` controls the
//! order the three paint operations are applied to a shape.
//!
//! - `normal` (initial) → fill, then stroke, then markers.
//! - Any keyword list → operations in source order; omitted keywords
//!   are appended in the `normal` order.
//!
//! Round-205 ships:
//!   * The §13.8 cascade (presentation attribute + inline `style="…"`
//!     + `<style>`-block rule + inheritance from a `<g>` ancestor).
//!   * Scene-graph split: when the stroke must paint BEFORE the fill,
//!     the round-1 single-PathNode-per-shape shape becomes a two-
//!     PathNode group (stroke-only PathNode first, fill-only PathNode
//!     second) so the composited result honours the requested order.
//!   * `PreservedExtras::paint_orders` side-channel — preserves the
//!     author's keyword string for a byte-faithful round-trip.

use oxideav_core::{Node, VectorFrame};
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Count the number of `Node::Path` leaves under the first scene-graph
/// child. The round-205 `paint-order: stroke fill` split produces two
/// PathNodes where the round-1 default produces one.
fn count_paths(frame: &VectorFrame) -> usize {
    fn walk(node: &Node) -> usize {
        match node {
            Node::Path(_) => 1,
            Node::Group(g) => g.children.iter().map(walk).sum(),
            Node::SoftMask { content, .. } => walk(content),
            _ => 0,
        }
    }
    frame.root.children.iter().map(walk).sum()
}

/// Collect `(has_fill, has_stroke)` for every `Node::Path` leaf in the
/// scene graph, in scene-graph order. Used to verify the round-205
/// stroke-first split lays down the stroke leaf before the fill leaf.
fn fill_stroke_signatures(frame: &VectorFrame) -> Vec<(bool, bool)> {
    fn walk(node: &Node, out: &mut Vec<(bool, bool)>) {
        match node {
            Node::Path(p) => out.push((p.fill.is_some(), p.stroke.is_some())),
            Node::Group(g) => {
                for c in &g.children {
                    walk(c, out);
                }
            }
            Node::SoftMask { content, .. } => walk(content, out),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &frame.root.children {
        walk(c, &mut out);
    }
    out
}

// --- 1. Baseline (no paint-order) — single PathNode, fill+stroke -----

#[test]
fn normal_default_emits_single_path_node_with_both_fills() {
    // Round-1 baseline: a stroked + filled rect emits ONE PathNode
    // carrying both. The round-205 paint-order branch is a no-op
    // when the property defaults to `normal`.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="100" height="100" fill="red" stroke="blue" stroke-width="4"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(count_paths(&frame), 1, "default normal: one PathNode");
    let sigs = fill_stroke_signatures(&frame);
    assert_eq!(sigs, vec![(true, true)], "fill + stroke on same leaf");
}

#[test]
fn explicit_normal_keyword_matches_default() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="100" height="100"
        fill="red" stroke="blue" stroke-width="4" paint-order="normal"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(count_paths(&frame), 1);
    assert_eq!(fill_stroke_signatures(&frame), vec![(true, true)]);
}

// --- 2. paint-order: stroke — stroke MUST render BEFORE fill --------

#[test]
fn paint_order_stroke_splits_into_two_path_nodes_stroke_first() {
    // The §13.8 example: `paint-order: stroke` means stroke is painted
    // first (under the fill). Equivalent to `stroke fill markers`.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="100" height="100"
        fill="red" stroke="blue" stroke-width="12" paint-order="stroke"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(count_paths(&frame), 2, "split into stroke + fill nodes");
    let sigs = fill_stroke_signatures(&frame);
    // First leaf: stroke only. Second leaf: fill only.
    assert_eq!(
        sigs,
        vec![(false, true), (true, false)],
        "stroke-only leaf precedes fill-only leaf"
    );
}

#[test]
fn paint_order_stroke_fill_explicit_pair_same_behaviour() {
    // `stroke fill` is the explicit form of `stroke` — same effect.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="100" height="100"
        fill="red" stroke="blue" stroke-width="12" paint-order="stroke fill"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let sigs = fill_stroke_signatures(&frame);
    assert_eq!(sigs, vec![(false, true), (true, false)]);
}

#[test]
fn paint_order_stroke_fill_markers_full_three_keyword_form() {
    // `stroke fill markers` — explicit form with markers slot at the
    // end. `oxideav_core::Node` has no Marker variant yet, so markers
    // doesn't emit a node but the stroke-first split is honoured.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="100" height="100"
        fill="red" stroke="blue" stroke-width="12"
        paint-order="stroke fill markers"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(count_paths(&frame), 2);
}

// --- 3. fill-first orders stay on a single PathNode ----------------

#[test]
fn paint_order_fill_stroke_explicit_normal_no_split() {
    // `fill stroke` — explicit normal order; no split, single PathNode.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="100" height="100"
        fill="red" stroke="blue" stroke-width="4" paint-order="fill stroke"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(count_paths(&frame), 1);
    assert_eq!(fill_stroke_signatures(&frame), vec![(true, true)]);
}

#[test]
fn paint_order_markers_only_means_normal_for_node_emission() {
    // `markers` resolves to `markers fill stroke` per §13.8's
    // "omitted keywords are appended in normal order" — so fill comes
    // before stroke and we keep the single PathNode (markers itself
    // doesn't yet emit a Marker node).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="100" height="100"
        fill="red" stroke="blue" stroke-width="4" paint-order="markers"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(count_paths(&frame), 1);
}

// --- 4. Edge cases: missing paint, hidden, invalid keyword ---------

#[test]
fn paint_order_stroke_without_a_stroke_emits_one_fill_only_path() {
    // If the shape has no stroke, there is nothing to put under the
    // fill — the split collapses back to a single PathNode (fill-only).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="100" height="100" fill="red" paint-order="stroke"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(count_paths(&frame), 1);
    assert_eq!(fill_stroke_signatures(&frame), vec![(true, false)]);
}

#[test]
fn paint_order_unknown_keyword_falls_back_to_normal() {
    // An unrecognised keyword in the list is tolerated; if no
    // recognised keywords survive, the attribute resolves to `normal`.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="100" height="100"
        fill="red" stroke="blue" stroke-width="4" paint-order="bogus"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(count_paths(&frame), 1);
    assert_eq!(fill_stroke_signatures(&frame), vec![(true, true)]);
}

// --- 5. Cascade resolution -----------------------------------------

#[test]
fn paint_order_inherited_from_g_ancestor() {
    // `paint-order` is inherited (§13.8 attribute table). A `<g
    // paint-order="stroke">` cascades onto its `<rect>` child so the
    // child splits even without its own attribute.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <g paint-order="stroke">
    <rect x="0" y="0" width="100" height="100"
          fill="red" stroke="blue" stroke-width="12"/>
  </g>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    // The split lives somewhere under the outer `<g>` group — the
    // total leaf count is 2 (stroke + fill).
    assert_eq!(count_paths(&frame), 2);
    let sigs = fill_stroke_signatures(&frame);
    assert_eq!(sigs, vec![(false, true), (true, false)]);
}

#[test]
fn paint_order_via_style_attribute_resolves_through_cascade() {
    // Inline `style="paint-order: stroke;"` flows through the round-4
    // cascade just like any other presentation property.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="100" height="100"
        fill="red" stroke="blue" stroke-width="12"
        style="paint-order: stroke;"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(count_paths(&frame), 2);
}

#[test]
fn paint_order_via_style_block_rule_resolves_through_cascade() {
    // A `<style>` block rule resolves the same way — the round-4 CSS
    // cascade routes the declaration into the shape's PaintState
    // independent of source order.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <style>rect { paint-order: stroke; }</style>
  <rect x="0" y="0" width="100" height="100"
        fill="red" stroke="blue" stroke-width="12"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(count_paths(&frame), 2);
}

// --- 6. Round-trip preservation via PreservedExtras ----------------

#[test]
fn paint_order_attribute_round_trips_via_extras() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="100" height="100"
        fill="red" stroke="blue" stroke-width="12" paint-order="stroke"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.paint_orders.len(),
        1,
        "one paint-order binding captured"
    );
    assert_eq!(extras.paint_orders[0].paint_order, "stroke");
    let out = write_svg_with_extras(&frame, &extras);
    let out_str = std::str::from_utf8(&out).unwrap();
    assert!(
        out_str.contains("paint-order=\"stroke\""),
        "expected paint-order attribute in output, got:\n{out_str}"
    );
    // Sanity round-trip: re-parsing the output preserves the split.
    let (frame2, extras2) = parse_svg_with_extras(&out).unwrap();
    assert_eq!(count_paths(&frame2), 2);
    assert_eq!(extras2.paint_orders.len(), 1);
    assert_eq!(extras2.paint_orders[0].paint_order, "stroke");
}

#[test]
fn paint_order_three_keyword_form_round_trips_canonical() {
    // The canonical form lowercases keywords + collapses inner
    // whitespace; duplicates are dropped (first occurrence wins).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="100" height="100"
        fill="red" stroke="blue" stroke-width="12"
        paint-order="Stroke  Fill  Markers"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.paint_orders[0].paint_order, "stroke fill markers");
    let out = write_svg_with_extras(&frame, &extras);
    let out_str = std::str::from_utf8(&out).unwrap();
    assert!(out_str.contains("paint-order=\"stroke fill markers\""));
}

#[test]
fn paint_order_normal_or_absent_does_not_record_a_binding() {
    // `paint-order="normal"` (or `inherit`, or missing) is the spec
    // default — no side-channel binding, no extra emission.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="100" height="100"
        fill="red" stroke="blue" paint-order="normal"/>
  <rect x="0" y="0" width="100" height="100" fill="red" stroke="blue"/>
</svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.paint_orders.is_empty(),
        "no bindings expected for normal / absent paint-order"
    );
}
