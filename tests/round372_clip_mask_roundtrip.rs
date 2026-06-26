//! Round 372 — `<clipPath>` / `<mask>` reference identity round-trip
//! fidelity (SVG 1.1 §14.3 / §14.4).
//!
//! The decoder collapses a `clip-path="url(#id)"` reference into a
//! single merged `Path` on `Group.clip` (baking per-shape transforms
//! in, dropping `clipPathUnits`, the original id, and the multi-shape
//! structure), and a `mask="url(#id)"` into a `Node::SoftMask` with the
//! flattened mask content (dropping the original id / `maskUnits` /
//! region). Before this round the encoder re-synthesised a
//! `<clipPath id="clip1">` / `<mask id="mask1">` with a single merged
//! shape and referenced the synthesised id — losing the source identity.
//!
//! This round captures the verbatim `<clipPath>` / `<mask>` defs and a
//! fingerprint-keyed reference binding so the encoder re-emits the
//! source def (original id + units + every shape) and references it by
//! its original id.

use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

fn roundtrip(src: &[u8]) -> String {
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    let out = write_svg_with_extras(&frame, &extras);
    String::from_utf8(out).expect("utf8")
}

#[test]
fn clip_path_original_id_and_units_survive() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <defs>
    <clipPath id="cp" clipPathUnits="userSpaceOnUse">
      <rect x="0" y="0" width="20" height="20"/>
    </clipPath>
  </defs>
  <rect width="50" height="50" clip-path="url(#cp)" fill="red"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains("id=\"cp\""),
        "original clipPath id survives:\n{out}"
    );
    assert!(
        !out.contains("id=\"clip1\""),
        "the synthesised clip1 id is suppressed:\n{out}"
    );
    assert!(
        out.contains("clipPathUnits=\"userSpaceOnUse\""),
        "clipPathUnits survives:\n{out}"
    );
    assert!(
        out.contains("clip-path=\"url(#cp)\""),
        "the reference re-points at the original id:\n{out}"
    );
}

#[test]
fn clip_path_multiple_shapes_survive() {
    // The merged-path decode loses the multi-shape structure; the
    // verbatim def round-trip keeps both shapes.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <defs>
    <clipPath id="cp">
      <rect x="0" y="0" width="20" height="20"/>
      <circle cx="30" cy="30" r="10"/>
    </clipPath>
  </defs>
  <rect width="50" height="50" clip-path="url(#cp)" fill="red"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(out.contains("<rect"), "clip rect survives:\n{out}");
    assert!(
        out.contains("<circle"),
        "the second clip shape (circle) survives the round-trip:\n{out}"
    );
}

#[test]
fn mask_original_id_and_units_survive() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <defs>
    <mask id="mk" maskUnits="userSpaceOnUse">
      <rect x="0" y="0" width="30" height="30" fill="white"/>
    </mask>
  </defs>
  <rect width="50" height="50" mask="url(#mk)" fill="red"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains("id=\"mk\""),
        "original mask id survives:\n{out}"
    );
    assert!(
        !out.contains("id=\"mask1\""),
        "the synthesised mask1 id is suppressed:\n{out}"
    );
    assert!(
        out.contains("maskUnits=\"userSpaceOnUse\""),
        "maskUnits survives:\n{out}"
    );
    assert!(
        out.contains("mask=\"url(#mk)\""),
        "the reference re-points at the original id:\n{out}"
    );
}

#[test]
fn clip_and_mask_stack_both_preserved() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <defs>
    <clipPath id="cp"><rect x="0" y="0" width="20" height="20"/></clipPath>
    <mask id="mk"><rect x="0" y="0" width="30" height="30" fill="white"/></mask>
  </defs>
  <rect width="50" height="50" clip-path="url(#cp)" mask="url(#mk)" fill="red"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(out.contains("clip-path=\"url(#cp)\""), "clip ref:\n{out}");
    assert!(out.contains("mask=\"url(#mk)\""), "mask ref:\n{out}");
    assert!(
        out.contains("id=\"cp\"") && out.contains("id=\"mk\""),
        "both defs:\n{out}"
    );
}

#[test]
fn clip_mask_reconnect_after_reparse() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <defs>
    <clipPath id="cp"><rect x="0" y="0" width="20" height="20"/></clipPath>
  </defs>
  <rect width="50" height="50" clip-path="url(#cp)" fill="red"/>
</svg>"##;
    let out = roundtrip(src);
    // Re-parse: the binding must reappear so the reference is a stable
    // round-trip carrier.
    let (_, extras) = parse_svg_with_extras(out.as_bytes()).expect("reparse");
    assert_eq!(extras.clip_refs.len(), 1, "clip ref on re-parse");
    assert_eq!(extras.clip_refs[0].ref_id, "cp");
    // And a second write still emits the original id (idempotent).
    let out2 = roundtrip(out.as_bytes());
    assert!(
        out2.contains("id=\"cp\""),
        "id stable across two cycles:\n{out2}"
    );
    assert!(
        out2.contains("clip-path=\"url(#cp)\""),
        "ref stable:\n{out2}"
    );
}

#[test]
fn document_without_clip_mask_is_unaffected() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <rect width="10" height="10" fill="red"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).expect("parse");
    assert!(extras.clip_refs.is_empty(), "no clip refs");
    assert!(extras.mask_refs.is_empty(), "no mask refs");
    assert!(extras.clip_paths_raw.is_empty(), "no raw clipPaths");
    assert!(extras.masks_raw.is_empty(), "no raw masks");
}
