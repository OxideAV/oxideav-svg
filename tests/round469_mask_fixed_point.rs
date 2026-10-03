//! Round 469 — `<mask>` definitions reach the writer's fixed point.
//!
//! The parser wraps a `<mask>`'s children in one plain `Group`; the
//! writer used to emit that wrapper as a `<g>` inside `<mask>`, so a
//! re-parse wrapped it again and every `parse → write` cycle nested one
//! more `<g>` (found by the `write_roundtrip` fuzz target). The plain
//! wrapper now merges into the `<mask>` element exactly as the
//! `<g mask="url(#…)">` content wrapper already did (round 449).

use oxideav_svg::{parse, write};

fn fixed_point(src: &[u8]) {
    let once = write(&parse(src).expect("parse"));
    let twice = write(&parse(&once).expect("re-parse"));
    assert_eq!(
        String::from_utf8_lossy(&once),
        String::from_utf8_lossy(&twice),
        "write(parse(·)) is not a fixed point"
    );
    let thrice = write(&parse(&twice).expect("re-parse"));
    assert_eq!(twice, thrice);
}

#[test]
fn mask_with_shape_content_is_stable() {
    fixed_point(br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><mask id="m"><rect width="10" height="10" fill="white"/></mask></defs><rect width="5" height="5" fill="red" mask="url(#m)"/></svg>"##);
}

#[test]
fn empty_mask_is_stable() {
    fixed_point(br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><mask id="m"></mask></defs><rect width="5" height="5" fill="red" mask="url(#m)"/></svg>"##);
}

#[test]
fn mask_with_nested_group_is_stable() {
    fixed_point(br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><mask id="m"><g opacity="0.5"><circle cx="5" cy="5" r="4" fill="white"/></g></mask></defs><rect width="5" height="5" fill="red" mask="url(#m)"/></svg>"##);
}

#[test]
fn mask_content_emits_without_a_synthetic_wrapper() {
    let out = write(
        &parse(br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><mask id="m"><rect width="10" height="10" fill="white"/></mask></defs><rect width="5" height="5" fill="red" mask="url(#m)"/></svg>"##)
            .unwrap(),
    );
    let text = String::from_utf8(out).unwrap();
    let mask_start = text.find("<mask").unwrap();
    let mask_end = text.find("</mask>").unwrap();
    let body = &text[mask_start..mask_end];
    assert!(
        !body.contains("<g>"),
        "mask body carries a synthetic <g>: {body}"
    );
    assert!(body.contains("<path"), "mask body lost its shape: {body}");
}

/// The fuzz-found counterexample (minimised): mangled markup the lenient
/// parser still accepts must also settle.
#[test]
fn fuzz_counterexample_settles() {
    fixed_point(
        br##"<?xml version="1.0"?>
<svg eight="120">
  <defs>
    <lip" clipPathUnits="userSpaceOnUse" clip-rule="evenodd">
      <re"40"/>
      <circle cx="40" cy="40" r="20"/>
    </clipPath>
    <mask id="msk" mask      <rect x="0"url(#msk)"/>
</svg>
eight="120" fi-pip)"/>
  <circle cx="80" cy="80" r" r="30" fill="#602020" mask="url(#msk)"/>
</svg>
"##,
    );
}

/// A transform that is identity only up to the writer's six-decimal
/// precision (the `sin(2π)` residue of an evaluated `rotate(360)`)
/// prints like the identity its re-parse produces, so the wrapper group
/// does not flip between `<g transform="matrix(1 0 -0 1 0 0)">` and
/// `<g>` across cycles (second fuzz-found counterexample, minimised).
#[test]
fn near_identity_transform_settles() {
    fixed_point(br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="120">
  <rect width="20" height="20"><animateTransform attributeName="transform" type="rotate" from="0" to="360" dur="1s"/></rect>
</svg>"##);
    fixed_point(
        br##"<?xml version="1.0"?>
<svg xmlns="ht3.org/2000/svg" width="120" height="120">
  <rect x="" width="20" height="20" fiol="#204060"><animate <animate"20" height=  ml="#404080"/><animateTransform attributeName="transfom" type="rotate" form="0" to="360" ect>
</svg>"##,
    );
}
