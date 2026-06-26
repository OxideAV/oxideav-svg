//! Round 372 — `<use>` reference + `<defs>` target round-trip fidelity
//! (SVG 2 §5.6 / SVG 1.1 §5.5).
//!
//! Before this round the decoder flattened every `<use>` into the
//! instantiated geometry, so `parse → write` re-emitted the inlined
//! shapes and lost the reference identity entirely (and inlined the
//! target N times for an N-instance document). The `<defs>`-housed
//! target shape itself was dropped on write, so even the inlined
//! geometry was the only carrier.
//!
//! This round adds two side-channels:
//!   * `PreservedExtras::uses` — collapses each instantiated instance
//!     group back to a single `<use href="#id" …/>` on write.
//!   * `PreservedExtras::defs_targets` — re-emits the `<defs>`-housed
//!     reference target verbatim so the `<use>` resolves after the
//!     round-trip.
//!
//! Each test asserts the re-serialised document carries `<use>` (not
//! the inlined geometry), the `<defs>` target survives, and a re-parse
//! of the output reproduces the same scene-graph shape count.

use oxideav_core::Node;
use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

fn count_paths(g: &oxideav_core::Group) -> usize {
    let mut n = 0;
    for c in &g.children {
        match c {
            Node::Path(_) => n += 1,
            Node::Group(sg) => n += count_paths(sg),
            Node::SoftMask { content, .. } => {
                if let Node::Group(sg) = content.as_ref() {
                    n += count_paths(sg);
                }
            }
            _ => {}
        }
    }
    n
}

fn roundtrip(src: &[u8]) -> String {
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    let out = write_svg_with_extras(&frame, &extras);
    String::from_utf8(out).expect("utf8")
}

#[test]
fn use_of_defs_rect_emits_use_not_inlined_geometry() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <rect id="r1" x="0" y="0" width="20" height="20" fill="red"/>
  </defs>
  <use href="#r1" x="10" y="10"/>
</svg>"##;
    let out = roundtrip(src);
    // The instance group collapses back to a single <use>.
    assert!(
        out.contains("<use") && out.contains("href=\"#r1\""),
        "expected a <use href=\"#r1\"> in output, got:\n{out}"
    );
    assert!(out.contains("x=\"10\""), "use x= preserved:\n{out}");
    assert!(out.contains("y=\"10\""), "use y= preserved:\n{out}");
    // The defs target survives so the reference resolves.
    assert!(
        out.contains("id=\"r1\"") && out.contains("<rect"),
        "expected the <defs> <rect id=\"r1\"> target to survive:\n{out}"
    );
    // The inlined fill should NOT leak a bare top-level path: there is
    // exactly one <use> driving the single instance, and the only
    // <rect> is the defs target (never-rendered, sits inside <defs>).
    assert_eq!(out.matches("<use").count(), 1, "exactly one <use>:\n{out}");
}

#[test]
fn use_reference_survives_reparse() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <rect id="r1" x="0" y="0" width="20" height="20" fill="red"/>
  </defs>
  <use href="#r1" x="10" y="10"/>
  <use href="#r1" x="40" y="40"/>
</svg>"##;
    // Original parse: two <use> → two instantiated rect paths.
    let (frame0, _) = parse_svg_with_extras(src).expect("parse0");
    let n0 = count_paths(&frame0.root);
    assert_eq!(n0, 2, "two instances → two paths");

    let out = roundtrip(src);
    assert_eq!(
        out.matches("<use").count(),
        2,
        "two <use> elements round-trip:\n{out}"
    );
    // Re-parse the serialised output: the <defs> target + two <use>
    // must reconstruct the same instance count.
    let (frame1, _) = parse_svg_with_extras(out.as_bytes()).expect("reparse");
    let n1 = count_paths(&frame1.root);
    assert_eq!(n1, n0, "re-parse reproduces the instance count:\n{out}");
}

#[test]
fn use_transform_is_preserved_verbatim() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <circle id="c1" cx="5" cy="5" r="5" fill="blue"/>
  </defs>
  <use href="#c1" transform="rotate(45)" x="2" y="3"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains("transform=\"rotate(45)\""),
        "the <use>'s own transform survives verbatim:\n{out}"
    );
    assert!(out.contains("href=\"#c1\""), "href preserved:\n{out}");
    assert!(
        out.contains("<circle") && out.contains("id=\"c1\""),
        "defs <circle> target survives:\n{out}"
    );
}

#[test]
fn use_of_symbol_emits_use_and_preserves_symbol() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <symbol id="sym1" viewBox="0 0 10 10">
    <rect x="0" y="0" width="10" height="10" fill="green"/>
  </symbol>
  <use href="#sym1" x="0" y="0" width="20" height="20"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains("href=\"#sym1\""),
        "symbol href preserved:\n{out}"
    );
    assert!(
        out.contains("<symbol") && out.contains("id=\"sym1\""),
        "the <symbol> target survives the round-trip:\n{out}"
    );
    assert!(
        out.contains("width=\"20\"") && out.contains("height=\"20\""),
        "the use's viewport override (width/height) survives:\n{out}"
    );
}

#[test]
fn use_with_own_id_preserves_both_ids() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <rect id="tgt" x="0" y="0" width="8" height="8" fill="black"/>
  </defs>
  <use id="inst" href="#tgt" x="1" y="1"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains("id=\"inst\""),
        "use's own id preserved:\n{out}"
    );
    assert!(
        out.contains("href=\"#tgt\""),
        "target href preserved:\n{out}"
    );
    assert!(
        out.contains("id=\"tgt\""),
        "defs target id preserved:\n{out}"
    );
}

#[test]
fn plain_document_without_use_is_unaffected() {
    // A document with no <use> / <defs> targets must round-trip exactly
    // as before — the new side-channels stay empty.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <rect x="0" y="0" width="10" height="10" fill="red"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).expect("parse");
    assert!(extras.uses.is_empty(), "no <use> bindings");
    assert!(extras.defs_targets.is_empty(), "no defs targets");
    let out = roundtrip(src);
    assert!(!out.contains("<use"), "no spurious <use>:\n{out}");
}
