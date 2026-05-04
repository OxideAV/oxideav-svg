//! Round 2 — `<mask>` and `<clipPath>` mapping. `<mask>` should map
//! to `Node::SoftMask`; `<clipPath>` should populate the wrapping
//! group's `clip` field.

use oxideav_core::{MaskKind, Node};
use oxideav_svg::{parse_svg, write_svg};

#[test]
fn mask_maps_to_soft_mask_with_luminance_default() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <mask id="m1">
      <rect x="0" y="0" width="100" height="100" fill="white"/>
      <circle cx="50" cy="50" r="30" fill="black"/>
    </mask>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="red" mask="url(#m1)"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let n = &frame.root.children[0];
    match n {
        Node::SoftMask {
            mask_kind, mask, ..
        } => {
            assert_eq!(*mask_kind, MaskKind::Luminance);
            // Mask subtree should contain the rect + circle (collapsed
            // into a Group).
            match mask.as_ref() {
                Node::Group(g) => {
                    assert!(!g.children.is_empty(), "mask subtree should not be empty")
                }
                other => panic!("expected mask Group, got {:?}", other),
            }
        }
        other => panic!("expected SoftMask, got {:?}", other),
    }
}

#[test]
fn mask_type_alpha_is_honored() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <defs>
    <mask id="m1" mask-type="alpha">
      <rect width="50" height="50" fill="white"/>
    </mask>
  </defs>
  <rect width="50" height="50" fill="blue" mask="url(#m1)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    match &frame.root.children[0] {
        Node::SoftMask { mask_kind, .. } => assert_eq!(*mask_kind, MaskKind::Alpha),
        other => panic!("expected SoftMask, got {:?}", other),
    }
}

#[test]
fn clip_path_populates_group_clip_field() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <clipPath id="c1">
      <circle cx="50" cy="50" r="40"/>
    </clipPath>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="green" clip-path="url(#c1)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let n = &frame.root.children[0];
    match n {
        Node::Group(g) => {
            assert!(g.clip.is_some(), "clip-path should populate Group::clip");
            let clip = g.clip.as_ref().unwrap();
            assert!(
                !clip.commands.is_empty(),
                "clip path must contain at least one command"
            );
        }
        other => panic!("expected Group, got {:?}", other),
    }
}

#[test]
fn multi_shape_clip_path_concatenates_into_single_path() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <clipPath id="c1">
      <rect x="0" y="0" width="50" height="50"/>
      <rect x="50" y="50" width="50" height="50"/>
    </clipPath>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="black" clip-path="url(#c1)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    match &frame.root.children[0] {
        Node::Group(g) => {
            let clip = g.clip.as_ref().expect("clip present");
            // Two rects → 2× (M + 3L + Z) = 10 commands.
            assert_eq!(clip.commands.len(), 10);
        }
        other => panic!("expected Group, got {:?}", other),
    }
}

#[test]
fn mask_round_trips_through_encoder() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <mask id="m1">
      <rect x="0" y="0" width="100" height="100" fill="white"/>
    </mask>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="red" mask="url(#m1)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let bytes = write_svg(&frame);
    let s = std::str::from_utf8(&bytes).unwrap();
    assert!(s.contains("<mask"), "encoder should emit a <mask> def: {s}");
    assert!(
        s.contains("mask=\"url(#"),
        "encoder should reference the mask: {s}"
    );
    let frame2 = parse_svg(&bytes).expect("re-parse");
    assert!(matches!(&frame2.root.children[0], Node::SoftMask { .. }));
}

#[test]
fn clip_path_round_trips_through_encoder() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <clipPath id="c1">
      <circle cx="50" cy="50" r="40"/>
    </clipPath>
  </defs>
  <rect width="100" height="100" fill="purple" clip-path="url(#c1)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let bytes = write_svg(&frame);
    let s = std::str::from_utf8(&bytes).unwrap();
    assert!(s.contains("<clipPath"), "encoder must emit clipPath: {s}");
    assert!(
        s.contains("clip-path=\"url(#"),
        "encoder must reference clipPath: {s}"
    );
}
