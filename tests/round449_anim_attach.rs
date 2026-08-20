//! Round 449 — SMIL animation parent re-attachment by scene-graph path
//! (SMIL Animation §3.1: an animation element with no explicit target
//! attribute targets its direct XML parent).
//!
//! The round-13 routing keyed captured animations by the *nearest
//! id-bearing ancestor*, which (a) orphaned every animation whose
//! parent chain is id-less — dumped detached at the document's
//! trailing edge, where the animation's implicit parent target becomes
//! the root `<svg>` and the animation is semantically lost — and
//! (b) mis-parented an animation whose id-less parent sits inside an
//! id-bearing container. `PreservedExtras::anim_targets` now records
//! the true parent slot during the scene-graph build, the encoder
//! re-emits each animation as a child of the node at that path, and a
//! suppression multiset cancels the double-capture against the
//! XML-walk channel (and against animations riding verbatim
//! side-channel trees), so each source animation is emitted exactly
//! once, in the right place.

use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

fn roundtrip(src: &[u8]) -> String {
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    let out = write_svg_with_extras(&frame, &extras);
    String::from_utf8(out).expect("utf8")
}

/// Asserts `needle` occurs exactly once in `hay` and returns its
/// position.
fn find_once(hay: &str, needle: &str) -> usize {
    let n = hay.matches(needle).count();
    assert_eq!(n, 1, "expected exactly one {needle:?}:\n{hay}");
    hay.find(needle).unwrap()
}

/// An `<animate>` child of an id-less shape re-attaches inside the
/// emitted element instead of being dumped detached at the trailing
/// edge.
#[test]
fn idless_shape_animation_reattaches() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <rect x="1" y="2" width="3" height="4"><animate attributeName="x" from="1" to="9" dur="2s"/></rect>
</svg>"##;
    let out = roundtrip(src);
    let p_anim = find_once(&out, "<animate");
    let p_open = out.find("<path").expect("shape present");
    let p_close = out.find("</path>").expect("shape closes to hold the child");
    assert!(
        p_open < p_anim && p_anim < p_close,
        "the animation sits inside its parent shape:\n{out}"
    );
}

/// A `<set>` child of an id-less `<circle>` re-attaches the same way
/// (regression: it used to land detached after the scene content).
#[test]
fn idless_set_reattaches() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <circle cx="5" cy="5" r="2"><set attributeName="r" to="4" begin="1s"/></circle>
</svg>"##;
    let out = roundtrip(src);
    let p_anim = find_once(&out, "<set");
    let p_close = out.find("</path>").expect("shape closes to hold the child");
    assert!(
        p_anim < p_close,
        "the set rides inside its parent shape:\n{out}"
    );
}

/// Deeply nested id-less parents re-attach at the right depth.
#[test]
fn deep_idless_animation_reattaches() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <g><g><rect x="1" y="2" width="3" height="4"><animate attributeName="y" to="9" dur="1s"/></rect></g></g>
</svg>"##;
    let out = roundtrip(src);
    let p_anim = find_once(&out, "<animate");
    let p_open = out.find("<path").expect("shape present");
    let p_close = out.find("</path>").expect("shape closes");
    assert!(
        p_open < p_anim && p_anim < p_close,
        "the animation sits inside the nested shape:\n{out}"
    );
}

/// An animation on an id-less shape inside an id-bearing group no
/// longer mis-parents onto the group: it belongs to the shape (SMIL
/// §3.1 — the implicit target is the direct parent).
#[test]
fn animation_targets_direct_parent_not_ancestor() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <g id="wrap"><rect x="1" y="2" width="3" height="4"><animate attributeName="width" to="8" dur="1s"/></rect></g>
</svg>"##;
    let out = roundtrip(src);
    let p_anim = find_once(&out, "<animate");
    let p_shape_open = out.find("<path").expect("shape present");
    let p_shape_close = out.find("</path>").expect("shape closes");
    assert!(
        p_shape_open < p_anim && p_anim < p_shape_close,
        "the animation is a child of the shape, not the id-bearing group:\n{out}"
    );
}

/// An id-bearing parent still re-attaches its animation exactly once.
#[test]
fn id_bearing_parent_still_inlines_once() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <rect id="r1" x="1" y="2" width="3" height="4"><animate attributeName="x" to="9" dur="1s"/></rect>
</svg>"##;
    let out = roundtrip(src);
    let p_anim = find_once(&out, "<animate");
    let p_close = out.find("</path>").expect("shape closes");
    assert!(p_anim < p_close, "inlined inside the shape:\n{out}");
    assert!(out.contains("id=\"r1\""), "the id survives:\n{out}");
}

/// An animation inside a verbatim-preserved `<pattern>` is emitted
/// exactly once (inside the pattern), not duplicated at the trailing
/// edge by the XML-walk capture.
#[test]
fn pattern_animation_emits_once() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs><pattern id="p" width="10" height="10"><rect width="5" height="5"><animate attributeName="width" to="9" dur="1s"/></rect></pattern></defs>
  <rect x="0" y="0" width="50" height="50" fill="url(#p)"/>
</svg>"##;
    let out = roundtrip(src);
    let p_anim = find_once(&out, "<animate");
    let p_pat_open = out.find("<pattern").expect("pattern survives");
    let p_pat_close = out.find("</pattern>").expect("pattern closes");
    assert!(
        p_pat_open < p_anim && p_anim < p_pat_close,
        "the animation rides inside the verbatim pattern only:\n{out}"
    );
}

/// An animation inside an id-bearing `<defs>` target (a `<use>`
/// reference target preserved verbatim) is emitted exactly once.
#[test]
fn defs_target_animation_emits_once() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs><rect id="tpl" width="5" height="5"><animate attributeName="height" to="9" dur="1s"/></rect></defs>
  <use href="#tpl" x="10"/>
</svg>"##;
    let out = roundtrip(src);
    find_once(&out, "<animate");
}

/// An `<animate>` child of the `<use>` element itself re-attaches
/// inside the round-tripped `<use>…</use>`.
#[test]
fn use_animation_reattaches() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs><rect id="tpl" width="5" height="5"/></defs>
  <use href="#tpl"><animate attributeName="x" to="20" dur="1s"/></use>
</svg>"##;
    let out = roundtrip(src);
    let p_anim = find_once(&out, "<animate");
    let p_use_open = out.find("<use").expect("use survives");
    let p_use_close = out.find("</use>").expect("use closes to hold the child");
    assert!(
        p_use_open < p_anim && p_anim < p_use_close,
        "the animation rides inside the <use>:\n{out}"
    );
}

/// An animation on an id-less `<g>` re-attaches inside the emitted
/// `<g>` (group arm coverage).
#[test]
fn idless_group_animation_reattaches() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <g transform="translate(2 2)"><rect x="0" y="0" width="4" height="4"/><animateTransform attributeName="transform" type="rotate" to="90" dur="1s"/></g>
</svg>"##;
    let out = roundtrip(src);
    let p_anim = find_once(&out, "<animateTransform");
    let p_close = out.rfind("</g>").expect("group closes");
    let p_open = out.find("<g").expect("group opens");
    assert!(
        p_open < p_anim && p_anim < p_close,
        "the animateTransform rides inside the group:\n{out}"
    );
}

/// `<animateMotion>` with an `<mpath>` child re-attaches with the
/// child intact.
#[test]
fn animate_motion_mpath_survives() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs><path id="track" d="M0 0 L50 50"/></defs>
  <rect x="0" y="0" width="4" height="4"><animateMotion dur="2s"><mpath href="#track"/></animateMotion></rect>
</svg>"##;
    let out = roundtrip(src);
    find_once(&out, "<animateMotion");
    assert!(
        out.contains("<mpath href=\"#track\""),
        "the mpath child survives:\n{out}"
    );
}

/// An animation under an uncaptured parent (an id-less shape inside
/// `<defs>` — no scene node, no verbatim carrier) still falls back to
/// the trailing-edge orphan emission rather than being lost.
#[test]
fn uncarried_animation_still_orphan_emits() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs><rect width="5" height="5"><animate attributeName="width" to="9" dur="1s"/></rect></defs>
  <circle cx="5" cy="5" r="2"/>
</svg>"##;
    let out = roundtrip(src);
    find_once(&out, "<animate");
}

/// The animated write output reaches a byte fixed point after one
/// cycle.
#[test]
fn animated_roundtrip_is_idempotent() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <g id="wrap"><rect x="1" y="2" width="3" height="4"><animate attributeName="x" to="9" dur="1s"/></rect></g>
  <circle cx="5" cy="5" r="2"><set attributeName="r" to="4" begin="1s"/></circle>
</svg>"##;
    let w1 = roundtrip(src);
    let w2 = roundtrip(w1.as_bytes());
    assert_eq!(w1, w2, "write(parse(write(x))) == write(x)");
    assert_eq!(w2.matches("<animate").count(), 1);
    assert_eq!(w2.matches("<set").count(), 1);
}
