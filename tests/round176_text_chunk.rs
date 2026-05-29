//! Round 176 — SVG 2 §11.5 text-chunk boundaries on `<tspan x=…>`.
//!
//! Round 172 applied the §11.10.1.1 `text-anchor` shift once across an
//! entire `<text>` element (one chunk per `<text>`). Round 176 splits
//! the run at every absolute positioning adjustment on a `<tspan>` and
//! shifts each chunk independently using its own pre-anchor extent.
//!
//! The fixtures here use the same DejaVuSansMono font shared across the
//! round-172 / round-128 glyph tests so the per-glyph advance is
//! predictable.

#![cfg(feature = "text")]

use oxideav_core::Node;
use oxideav_scribe::{Face, FaceChain};
use oxideav_svg::{parse_svg, text::set_font_resolver};

const FONT: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");

fn install_resolver() {
    let _ = set_font_resolver(move |_family, _size_px| {
        Face::from_ttf_bytes(FONT.to_vec()).ok().map(FaceChain::new)
    });
}

/// Recursively collect every leaf placement group's translate `(e, f)`.
fn collect_translates(node: &Node, out: &mut Vec<(f32, f32)>) {
    if let Node::Group(g) = node {
        let tx = g.transform;
        let is_identity = (tx.a - 1.0).abs() < 1e-6
            && tx.b.abs() < 1e-6
            && tx.c.abs() < 1e-6
            && (tx.d - 1.0).abs() < 1e-6
            && tx.e.abs() < 1e-6
            && tx.f.abs() < 1e-6;
        if !is_identity {
            out.push((tx.e, tx.f));
        }
        for c in &g.children {
            collect_translates(c, out);
        }
    }
}

fn text_groups(frame: &oxideav_core::VectorFrame) -> Vec<&oxideav_core::Group> {
    frame
        .root
        .children
        .iter()
        .filter_map(|c| match c {
            Node::Group(g) => Some(g),
            _ => None,
        })
        .collect()
}

/// Split a text Group's child placements into two buckets by y. The
/// chunk fixtures here put the second `<tspan>` on a different baseline
/// (`y="…"`) so glyph y-translates cleanly partition the two chunks.
fn split_by_y(g: &oxideav_core::Group, threshold: f32) -> (Vec<f32>, Vec<f32>) {
    let mut t = Vec::new();
    for c in &g.children {
        collect_translates(c, &mut t);
    }
    let mut below = Vec::new();
    let mut above = Vec::new();
    for (x, y) in t {
        if y < threshold {
            below.push(x);
        } else {
            above.push(x);
        }
    }
    (below, above)
}

/// Minimum x across a placement slice; `+∞` when empty.
fn leftmost(xs: &[f32]) -> f32 {
    xs.iter().copied().fold(f32::INFINITY, f32::min)
}

/// Two `<tspan>`s carrying explicit `x=` attributes form two separate
/// anchored chunks. With `<text text-anchor="end">`, each chunk's
/// leftmost glyph sits ≤ its own `x=` origin (the entire chunk shifts
/// left by its own extent). Critically, both chunks must shift
/// **independently** — round 172's single-chunk model would have shifted
/// the second chunk by the *combined* extent.
#[test]
fn two_tspans_with_x_form_two_chunks_end_anchored() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="200">
  <text font-size="16" text-anchor="end">
    <tspan x="100" y="50">AAA</tspan>
    <tspan x="400" y="100">BBB</tspan>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let groups = text_groups(&frame);
    assert_eq!(groups.len(), 1);
    let (chunk_a, chunk_b) = split_by_y(groups[0], 75.0);
    assert!(
        !chunk_a.is_empty() && !chunk_b.is_empty(),
        "both chunks must emit glyphs (a={} b={})",
        chunk_a.len(),
        chunk_b.len()
    );
    let leftmost_a = leftmost(&chunk_a);
    let leftmost_b = leftmost(&chunk_b);
    // `end` anchor: the chunk's right edge lines up with its origin, so
    // the leftmost glyph sits at `origin - extent`. With AAA / BBB and
    // a monospace font, both extents are equal, so both leftmost-glyph
    // x values should sit at `origin - W` — chunk B near 400-W, chunk A
    // near 100-W.
    assert!(
        leftmost_a < 100.0,
        "end-anchored chunk A leftmost should be < 100; got {leftmost_a}"
    );
    assert!(
        leftmost_b < 400.0,
        "end-anchored chunk B leftmost should be < 400; got {leftmost_b}"
    );
    // The two chunks must be ~300 px apart (the difference of the two
    // origins) — proving the second chunk did NOT inherit the first
    // chunk's accumulated extent.
    let gap = leftmost_b - leftmost_a;
    assert!(
        (gap - 300.0).abs() < 1.0,
        "chunk B should sit 300 px to the right of chunk A; got gap={gap}"
    );
}

/// Round 172's parse_text_element shifted by total `pen.x - x` for the
/// whole element. Round 176's per-chunk shift means the leftmost glyph
/// of the second chunk is the *origin minus its own width*, not the
/// origin minus the combined width of both chunks.
///
/// This test triangulates the regression-free behaviour by comparing
/// the round-176 two-chunk layout against the equivalent two `<text>`
/// elements (which always laid out independently even in round 172).
#[test]
fn multi_chunk_matches_two_text_elements() {
    install_resolver();
    let src_multi = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="200">
  <text font-size="16" text-anchor="middle">
    <tspan x="100" y="50">AAA</tspan>
    <tspan x="400" y="100">BBB</tspan>
  </text>
</svg>"##;
    let src_split = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="200">
  <text x="100" y="50" font-size="16" text-anchor="middle">AAA</text>
  <text x="400" y="100" font-size="16" text-anchor="middle">BBB</text>
</svg>"##;
    let frame_multi = parse_svg(src_multi).expect("parse multi");
    let frame_split = parse_svg(src_split).expect("parse split");

    let multi = text_groups(&frame_multi);
    let split = text_groups(&frame_split);
    assert_eq!(multi.len(), 1);
    assert_eq!(split.len(), 2);

    let (multi_a, multi_b) = split_by_y(multi[0], 75.0);
    let split_a: Vec<f32> = {
        let mut t = Vec::new();
        for c in &split[0].children {
            collect_translates(c, &mut t);
        }
        t.into_iter().map(|(x, _)| x).collect()
    };
    let split_b: Vec<f32> = {
        let mut t = Vec::new();
        for c in &split[1].children {
            collect_translates(c, &mut t);
        }
        t.into_iter().map(|(x, _)| x).collect()
    };

    let lm_a = leftmost(&multi_a);
    let lm_b = leftmost(&multi_b);
    let ls_a = leftmost(&split_a);
    let ls_b = leftmost(&split_b);

    assert!(
        (lm_a - ls_a).abs() < 0.5,
        "chunk A leftmost should match the split-text equivalent: \
         multi={lm_a} split={ls_a}"
    );
    assert!(
        (lm_b - ls_b).abs() < 0.5,
        "chunk B leftmost should match the split-text equivalent: \
         multi={lm_b} split={ls_b}"
    );
}

/// A `<tspan>` with only `dx=` (a relative pen nudge, no absolute `x`)
/// must NOT start a new chunk — both pieces stay in the same anchored
/// chunk and the §11.10.1.1 shift covers the whole run.
#[test]
fn tspan_with_dx_only_stays_in_same_chunk() {
    install_resolver();
    let src_dx = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="200">
  <text x="200" y="50" font-size="16" text-anchor="middle">A<tspan dx="20">B</tspan></text>
</svg>"##;
    // Reference: one chunk with the equivalent literal content. The two
    // forms should produce the *same* leftmost glyph (one chunk, one
    // shift).
    let src_one = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="200">
  <text x="200" y="50" font-size="16" text-anchor="middle">A<tspan dx="20">B</tspan></text>
</svg>"##;
    let f_dx = parse_svg(src_dx).expect("parse dx");
    let f_one = parse_svg(src_one).expect("parse one");
    let g_dx = &text_groups(&f_dx)[0];
    let g_one = &text_groups(&f_one)[0];
    let mut t_dx = Vec::new();
    for c in &g_dx.children {
        collect_translates(c, &mut t_dx);
    }
    let mut t_one = Vec::new();
    for c in &g_one.children {
        collect_translates(c, &mut t_one);
    }
    let lm_dx = leftmost(&t_dx.iter().map(|(x, _)| *x).collect::<Vec<_>>());
    let lm_one = leftmost(&t_one.iter().map(|(x, _)| *x).collect::<Vec<_>>());
    assert!(
        (lm_dx - lm_one).abs() < 1e-3,
        "dx-only tspan must not create a chunk boundary: {lm_dx} vs {lm_one}"
    );
    // And the single-chunk middle-anchor first glyph sits to the LEFT
    // of x=200 since `A<dx=20>B` has > 0 total extent.
    assert!(
        lm_dx < 200.0,
        "middle-anchored single chunk first glyph should sit left of x=200; got {lm_dx}"
    );
}

/// A `<tspan x=…>` may also carry its own `text-anchor=`; round 176
/// honours that override when opening the chunk so the new chunk's
/// shift uses the tspan's anchor rather than the inherited one.
#[test]
fn chunk_picks_up_tspan_text_anchor_override() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="200">
  <text font-size="16" text-anchor="start">
    <tspan x="100" y="50">AAA</tspan>
    <tspan x="400" y="100" text-anchor="end">BBB</tspan>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let groups = text_groups(&frame);
    assert_eq!(groups.len(), 1);
    let (chunk_a, chunk_b) = split_by_y(groups[0], 75.0);
    assert!(!chunk_a.is_empty() && !chunk_b.is_empty());

    let lm_a = leftmost(&chunk_a);
    let lm_b = leftmost(&chunk_b);

    // Chunk A is `start`-anchored at x=100 → leftmost ≈ 100.
    assert!(
        (lm_a - 100.0).abs() < 1.0,
        "start chunk leftmost should be ≈100; got {lm_a}"
    );
    // Chunk B is `end`-anchored at x=400 → leftmost < 400 (shifted left
    // by its own extent).
    assert!(
        lm_b < 400.0,
        "end-anchored override chunk leftmost should be < 400; got {lm_b}"
    );
}

/// Three chunks: anchor each one independently and confirm none of the
/// shifts cross-contaminates. Uses `<text text-anchor="end">` so every
/// chunk's leftmost glyph sits left of that chunk's `x=` origin.
#[test]
fn three_chunks_are_each_shifted_independently() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="900" height="200">
  <text font-size="16" text-anchor="end">
    <tspan x="100" y="50">AAA</tspan>
    <tspan x="400" y="100">BBB</tspan>
    <tspan x="700" y="150">CCC</tspan>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let groups = text_groups(&frame);
    assert_eq!(groups.len(), 1);

    // Partition by y: a < 75, 75 ≤ b < 125, c ≥ 125.
    let mut chunks: Vec<Vec<f32>> = vec![Vec::new(); 3];
    let mut all = Vec::new();
    for c in &groups[0].children {
        collect_translates(c, &mut all);
    }
    for (x, y) in all {
        let bucket = if y < 75.0 {
            0
        } else if y < 125.0 {
            1
        } else {
            2
        };
        chunks[bucket].push(x);
    }
    let a = leftmost(&chunks[0]);
    let b = leftmost(&chunks[1]);
    let c = leftmost(&chunks[2]);

    assert!(a.is_finite() && b.is_finite() && c.is_finite());

    // Each chunk's leftmost glyph sits left of its own origin.
    assert!(a < 100.0 && b < 400.0 && c < 700.0);
    // Adjacent chunk leftmost spacings should be ~300 (the difference
    // of their origins) — independent shifts, no accumulation.
    assert!(
        (b - a - 300.0).abs() < 1.0 && (c - b - 300.0).abs() < 1.0,
        "chunk leftmost gaps should each be ≈300: a={a} b={b} c={c}"
    );
}
