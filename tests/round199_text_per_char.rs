//! Round 199 — SVG 2 §11.2 / §11.2.2 list-of-values on `x`, `y`, `dx`,
//! `dy`, and `rotate` for `<text>` and `<tspan>`.
//!
//! Earlier rounds parsed only the first scalar of each attribute. The
//! round-199 change accepts the full list and applies the n-th value
//! to the n-th character per the §11.2.2 "n-th character" rule:
//!
//! - An absolute `x` / `y` slot seats the current text position for
//!   that character (so character 2's glyph sits at the supplied
//!   coordinates regardless of where character 1 left the pen).
//! - A relative `dx` / `dy` slot nudges the current text position
//!   before placing that character's glyph.
//! - A `rotate` slot rotates the character's glyph about its origin;
//!   if the list is shorter than the character count, the final
//!   supplied value sticks to every trailing character.
//!
//! `<tspan>` may carry its own list — the values overlay onto the
//! document-wide vectors starting at the current character ordinal so
//! a `<tspan x="100 200">` mid-`<text>` seats the first two characters
//! it contains.
//!
//! Font: the bundled DejaVuSansMono. Every Latin glyph has the same
//! advance, so a 16-px run of "ABCDE" without overrides lays down
//! glyphs at `(0, advance, 2*advance, …)` and the unadjusted advance
//! is recoverable as `xs[1] - xs[0]`.

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

/// Recursively collect placement-Group transforms into `(a, b, e, f)`
/// tuples. `a/b` carry rotation contributions; `e/f` are translations.
fn collect_xforms(node: &Node, out: &mut Vec<(f32, f32, f32, f32)>) {
    if let Node::Group(g) = node {
        let tx = g.transform;
        let is_identity = (tx.a - 1.0).abs() < 1e-6
            && tx.b.abs() < 1e-6
            && tx.c.abs() < 1e-6
            && (tx.d - 1.0).abs() < 1e-6
            && tx.e.abs() < 1e-6
            && tx.f.abs() < 1e-6;
        if !is_identity {
            out.push((tx.a, tx.b, tx.e, tx.f));
        }
        for c in &g.children {
            collect_xforms(c, out);
        }
    }
}

fn text_group(frame: &oxideav_core::VectorFrame) -> &oxideav_core::Group {
    for c in &frame.root.children {
        if let Node::Group(g) = c {
            return g;
        }
    }
    panic!("no text group");
}

fn placements(g: &oxideav_core::Group) -> Vec<(f32, f32, f32, f32)> {
    let mut out = Vec::new();
    for c in &g.children {
        collect_xforms(c, &mut out);
    }
    out
}

/// Sanity baseline: with no per-character lists, "ABCDE" lays out at
/// `(x, x+adv, x+2*adv, …)`. We use this to recover `advance` for the
/// other assertions.
fn natural_advance() -> f32 {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="100">
  <text x="0" y="50" font-size="16" font-family="mono">ABCDE</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let xs: Vec<f32> = placements(text_group(&frame))
        .into_iter()
        .map(|(_, _, e, _)| e)
        .collect();
    assert!(
        xs.len() >= 2,
        "expected ≥ 2 glyph placements; got {}",
        xs.len()
    );
    xs[1] - xs[0]
}

/// SVG 2 §11.2 `x` as a list — each value seats the corresponding
/// character at the supplied absolute x. With `x="10 50 100"` the first
/// three characters of "ABCDE" land at exactly 10, 50, 100; the
/// remainder advance from the third character's natural pen position.
#[test]
fn list_x_seats_individual_characters() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="100">
  <text x="10 50 100" y="50" font-size="16" font-family="mono">ABCDE</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let xs: Vec<f32> = placements(text_group(&frame))
        .into_iter()
        .map(|(_, _, e, _)| e)
        .collect();
    assert_eq!(xs.len(), 5, "expected 5 glyph placements; got {}", xs.len());
    assert!(
        (xs[0] - 10.0).abs() < 1e-3,
        "char 0 should be at 10; got {}",
        xs[0]
    );
    assert!(
        (xs[1] - 50.0).abs() < 1e-3,
        "char 1 should be at 50; got {}",
        xs[1]
    );
    assert!(
        (xs[2] - 100.0).abs() < 1e-3,
        "char 2 should be at 100; got {}",
        xs[2]
    );
    let adv = natural_advance();
    // Characters 3 and 4 have no x-override: they advance from char 2's
    // (post-glyph) pen position.
    assert!(
        (xs[3] - (100.0 + adv)).abs() < 1e-3,
        "char 3 should advance from char 2 origin + advance ({}); got {}",
        100.0 + adv,
        xs[3]
    );
    assert!(
        (xs[4] - (100.0 + 2.0 * adv)).abs() < 1e-3,
        "char 4 should advance from char 2 + 2 advances; got {}",
        xs[4]
    );
}

/// SVG 2 §11.2 `dx` as a list — each value adds to the current pen
/// before placing the character. Three `dx` slots on a five-character
/// run shift only the first three characters; characters 3 and 4 then
/// continue at the natural cadence.
#[test]
fn list_dx_nudges_individual_characters() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="100">
  <text x="0" y="50" dx="0 5 10" font-size="16" font-family="mono">ABCDE</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let xs: Vec<f32> = placements(text_group(&frame))
        .into_iter()
        .map(|(_, _, e, _)| e)
        .collect();
    assert_eq!(xs.len(), 5);
    let adv = natural_advance();
    // char 0: x=0, dx=0 → 0
    assert!(
        (xs[0] - 0.0).abs() < 1e-3,
        "char 0 should be at 0; got {}",
        xs[0]
    );
    // char 1: x=0+adv (post-char-0 advance), dx=5 → adv + 5
    assert!(
        (xs[1] - (adv + 5.0)).abs() < 1e-3,
        "char 1 should be at adv+5 ({}); got {}",
        adv + 5.0,
        xs[1]
    );
    // char 2: x=adv+5+adv (post-char-1 advance), dx=10 → 2*adv + 15
    assert!(
        (xs[2] - (2.0 * adv + 15.0)).abs() < 1e-3,
        "char 2 should be at 2*adv+15 ({}); got {}",
        2.0 * adv + 15.0,
        xs[2]
    );
    // char 3: no dx override; just advance from char 2's post-glyph pen
    assert!(
        (xs[3] - (3.0 * adv + 15.0)).abs() < 1e-3,
        "char 3 should be at 3*adv+15; got {}",
        xs[3]
    );
    // char 4: same — natural advance
    assert!(
        (xs[4] - (4.0 * adv + 15.0)).abs() < 1e-3,
        "char 4 should be at 4*adv+15; got {}",
        xs[4]
    );
}

/// SVG 2 §11.2 `y` as a list — each value seats the corresponding
/// character's baseline at the supplied absolute y. With
/// `y="10 20 30 40 50"` each of the five characters of "ABCDE" sits on
/// its own baseline.
#[test]
fn list_y_seats_individual_baselines() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="100">
  <text x="0" y="10 20 30 40 50" font-size="16" font-family="mono">ABCDE</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let xforms = placements(text_group(&frame));
    assert_eq!(xforms.len(), 5);
    let ys: Vec<f32> = xforms.iter().map(|(_, _, _, f)| *f).collect();
    for (i, expected) in [10.0, 20.0, 30.0, 40.0, 50.0].iter().enumerate() {
        assert!(
            (ys[i] - *expected).abs() < 1e-3,
            "char {} y should be {}; got {}",
            i,
            expected,
            ys[i]
        );
    }
}

/// SVG 2 §11.2 `rotate` as a list — each value rotates the
/// corresponding character about its origin. With `rotate="0 90 180"`
/// on "ABCDE", char 1's `a` ≈ 0, `b` ≈ 1 (a 90° rotation), char 2's
/// `a` ≈ -1 (a 180° rotation), and the §11.2.2 sticky-final rule has
/// chars 3 + 4 also at 180°.
#[test]
fn list_rotate_per_character_with_sticky_final() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="100">
  <text x="100" y="50" rotate="0 90 180" font-size="16" font-family="mono">ABCDE</text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let xforms = placements(text_group(&frame));
    assert_eq!(
        xforms.len(),
        5,
        "expected 5 glyph placements; got {}",
        xforms.len()
    );
    // Char 0: rotate=0 → identity rotation (a=1, b=0).
    assert!(
        (xforms[0].0 - 1.0).abs() < 1e-3 && xforms[0].1.abs() < 1e-3,
        "char 0 rotation should be identity; got a={} b={}",
        xforms[0].0,
        xforms[0].1
    );
    // Char 1: rotate=90 → a=cos(90°)=0, b=sin(90°)=1.
    assert!(
        xforms[1].0.abs() < 1e-3 && (xforms[1].1 - 1.0).abs() < 1e-3,
        "char 1 rotation should be 90°; got a={} b={}",
        xforms[1].0,
        xforms[1].1
    );
    // Char 2 + sticky chars 3,4: rotate=180 → a=-1, b=0.
    for (i, xf) in xforms.iter().enumerate().take(5).skip(2) {
        assert!(
            (xf.0 - (-1.0)).abs() < 1e-3 && xf.1.abs() < 1e-3,
            "char {} should inherit 180° per §11.2.2 sticky-final; got a={} b={}",
            i,
            xf.0,
            xf.1
        );
    }
}

/// SVG 2 §11.2 — `<tspan>` carries its own list, which overlays onto
/// the document-wide vectors at the current character ordinal. A
/// root-`<text>` with no `x`-list followed by a `<tspan x="100 200">`
/// seats the tspan's two characters at exactly 100 and 200.
#[test]
fn tspan_list_overlays_at_current_ordinal() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="100"><text x="0" y="50" font-size="16" font-family="mono">A<tspan x="100 200">BC</tspan>D</text></svg>"##;
    let frame = parse_svg(src).expect("parse");
    let xs: Vec<f32> = placements(text_group(&frame))
        .into_iter()
        .map(|(_, _, e, _)| e)
        .collect();
    assert_eq!(xs.len(), 4, "expected 4 glyphs (A B C D); got {}", xs.len());
    let adv = natural_advance();
    // Char 0 ('A'): origin x=0.
    assert!(
        (xs[0] - 0.0).abs() < 1e-3,
        "char A should be at 0; got {}",
        xs[0]
    );
    // Char 1 ('B'): tspan x[0] → 100.
    assert!(
        (xs[1] - 100.0).abs() < 1e-3,
        "char B should be at 100; got {}",
        xs[1]
    );
    // Char 2 ('C'): tspan x[1] → 200.
    assert!(
        (xs[2] - 200.0).abs() < 1e-3,
        "char C should be at 200; got {}",
        xs[2]
    );
    // Char 3 ('D'): natural advance from C's post-glyph pen position.
    assert!(
        (xs[3] - (200.0 + adv)).abs() < 1e-3,
        "char D should follow at 200+adv ({}); got {}",
        200.0 + adv,
        xs[3]
    );
}

/// A `dx` list whose length exceeds the run's character count must NOT
/// crash and must NOT apply extra slots — the over-supplied values are
/// silently dropped per the §11.2.2 "n-th character" lookup (no n-th
/// character exists ⇒ no override emitted).
#[test]
fn list_longer_than_run_is_lenient() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="100"><text x="0" y="50" dx="10 20 30 40 50 60 70 80" font-size="16" font-family="mono">AB</text></svg>"##;
    let frame = parse_svg(src).expect("parse");
    let xs: Vec<f32> = placements(text_group(&frame))
        .into_iter()
        .map(|(_, _, e, _)| e)
        .collect();
    assert_eq!(xs.len(), 2);
    let adv = natural_advance();
    // char 0: x=0 + dx[0]=10 → 10
    assert!(
        (xs[0] - 10.0).abs() < 1e-3,
        "char 0 should be at 10; got {}",
        xs[0]
    );
    // char 1: pen=10+adv + dx[1]=20 → adv + 30
    assert!(
        (xs[1] - (adv + 30.0)).abs() < 1e-3,
        "char 1 should be at adv+30 ({}); got {}",
        adv + 30.0,
        xs[1]
    );
}

/// Comma-separated, multi-whitespace, and mixed-separator lists all
/// parse identically — the SVG list grammar accepts whitespace and / or
/// a single comma between values.
#[test]
fn list_separator_grammar_accepts_commas_and_whitespace() {
    install_resolver();
    let variants: [&[u8]; 3] = [
        br##"<svg xmlns="http://www.w3.org/2000/svg"><text x="10 20 30" y="50" font-size="16" font-family="mono">ABC</text></svg>"##,
        br##"<svg xmlns="http://www.w3.org/2000/svg"><text x="10,20,30" y="50" font-size="16" font-family="mono">ABC</text></svg>"##,
        br##"<svg xmlns="http://www.w3.org/2000/svg"><text x="10 , 20  ,  30" y="50" font-size="16" font-family="mono">ABC</text></svg>"##,
    ];
    let mut all = Vec::new();
    for v in &variants {
        let frame = parse_svg(v).expect("parse");
        let xs: Vec<f32> = placements(text_group(&frame))
            .into_iter()
            .map(|(_, _, e, _)| e)
            .collect();
        all.push(xs);
    }
    for xs in &all {
        assert_eq!(xs.len(), 3);
        assert!((xs[0] - 10.0).abs() < 1e-3);
        assert!((xs[1] - 20.0).abs() < 1e-3);
        assert!((xs[2] - 30.0).abs() < 1e-3);
    }
}

/// Empty `rotate=""` is treated as "no rotation" rather than crashing
/// or applying a sentinel; missing attributes ditto. Sanity check that
/// the lenient parser does not change baseline behaviour.
#[test]
fn empty_rotate_attribute_is_no_op() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="100"><text x="100" y="50" rotate="" font-size="16" font-family="mono">A</text></svg>"##;
    let frame = parse_svg(src).expect("parse");
    let xforms = placements(text_group(&frame));
    assert_eq!(xforms.len(), 1);
    // Identity rotation: a=1, b=0.
    assert!(
        (xforms[0].0 - 1.0).abs() < 1e-3 && xforms[0].1.abs() < 1e-3,
        "empty rotate= should yield identity; got a={} b={}",
        xforms[0].0,
        xforms[0].1
    );
}

/// Round-176 §11.5 anchored-chunk semantics still hold when a tspan
/// carries `x="…"` as a multi-value list — the FIRST value of the list
/// opens a new chunk (matching the §11.5 "absolute positioning
/// adjustment" rule) and the chunk-anchor shift composes correctly with
/// per-character `x` placements.
///
/// Under `text-anchor="end"`, the chunk's extent equals
/// `pen.x at chunk close − chunk origin`; with the C glyph's
/// post-advance pen position factored in (per the round-199
/// per-character convention), the shift evaluates to `-(200 − 100 +
/// adv) = -(100 + adv)`. The post-shift positions of B and C are
/// therefore `100 − (100 + adv) = -adv` and `200 − (100 + adv) =
/// 100 − adv` respectively — a 100-px gap between them that survives
/// the anchor shift unchanged (proof the chunk's per-character layout
/// is faithfully reflected through the anchor pass).
#[test]
fn tspan_list_x_first_value_opens_chunk() {
    install_resolver();
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="600" height="100"><text x="0" y="50" font-size="16" font-family="mono" text-anchor="end">A<tspan x="100 200">BC</tspan></text></svg>"##;
    let frame = parse_svg(src).expect("parse");
    let xs: Vec<f32> = placements(text_group(&frame))
        .into_iter()
        .map(|(_, _, e, _)| e)
        .collect();
    assert_eq!(xs.len(), 3, "expected 3 glyphs; got {}", xs.len());
    let adv = natural_advance();
    // Chunk 0 ('A'): a single glyph under end-anchor with the
    // round-2 legacy `pen sits at last visible glyph origin`
    // convention has extent 0, so the shift is 0 and A stays at its
    // origin x=0. (This matches the round-176 baseline for any
    // single-glyph chunk.)
    assert!(
        xs[0].abs() < 1e-3,
        "char A under end-anchor should sit at 0; got {}",
        xs[0]
    );
    // Chunk 1 ('B','C'): origin 100; post-walk x_end = pen.x = 200 +
    // adv (the C-glyph advance was retained because the run carried
    // a per-character override). Extent = 100 + adv; shift = -(100 +
    // adv). B and C therefore land at:
    //   B: 100 − (100 + adv) = −adv
    //   C: 200 − (100 + adv) = 100 − adv
    // The crucial invariant: the gap between B and C is exactly 100
    // px (the spread the author requested via `x="100 200"`),
    // independent of the anchor shift.
    let expected_b = -adv;
    let expected_c = 100.0 - adv;
    assert!(
        (xs[1] - expected_b).abs() < 1e-3,
        "char B should land at -adv ({}); got {}",
        expected_b,
        xs[1]
    );
    assert!(
        (xs[2] - expected_c).abs() < 1e-3,
        "char C should land at 100-adv ({}); got {}",
        expected_c,
        xs[2]
    );
    let gap = xs[2] - xs[1];
    assert!(
        (gap - 100.0).abs() < 1e-3,
        "BC gap should match the authored 100-px spread; got {}",
        gap
    );
}
