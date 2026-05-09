//! Round 12 — `viewBox` + non-uniform `preserveAspectRatio` on the
//! root `<svg>` element. The 10 align variants × meet/slice combos
//! affect viewport mapping per SVG 2 §8.2.
//!
//! The decoder bakes the spec-mandated translate+scale into
//! `frame.root.transform` so a downstream rasteriser that knows
//! nothing about `preserveAspectRatio` (just stretches viewBox →
//! canvas) still produces the correct visual result. The original
//! attribute string is preserved verbatim in
//! [`PreservedExtras::root_preserve_aspect_ratio`] for round-trip.
//!
//! ## Worked example (matches the spec verbatim)
//!
//! Source: `<svg width="200" height="100" viewBox="0 0 100 100">` with
//! the default `preserveAspectRatio="xMidYMid meet"`.
//!
//! Spec algorithm 8.2:
//!   * scale-x = 200/100 = 2; scale-y = 100/100 = 1
//!   * meet → set larger to smaller → scale-x = scale-y = 1
//!   * translate-x = 0 - (0 * 1) = 0; translate-y = 0
//!   * align contains "xMid" → translate-x += (200 - 100*1)/2 = 50
//!
//! Result: spec_correct = translate(50, 0) * scale(1, 1).
//!
//! A point (50, 50) in the viewBox → (100, 50) on the canvas. A point
//! (0, 0) → (50, 0). The "meet" letterbox shows up as 50px of empty
//! canvas at left and right.

use oxideav_core::{Point, Transform2D};
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Apply a [`Transform2D`] to a `(x, y)` point. Mirrors the
/// renderer's left-multiply convention (`a*x + c*y + e`).
fn apply(t: &Transform2D, p: Point) -> Point {
    Point::new(t.a * p.x + t.c * p.y + t.e, t.b * p.x + t.d * p.y + t.f)
}

/// Compose the renderer's natural mapping (which the raster crate
/// applies on top of `root.transform`) with the root transform — i.e.
/// the effective transform every painted vertex sees.
fn effective(
    width: f32,
    height: f32,
    vb_min_x: f32,
    vb_min_y: f32,
    vb_w: f32,
    vb_h: f32,
    root: &Transform2D,
) -> Transform2D {
    let natural = Transform2D::scale(width / vb_w, height / vb_h)
        .compose(&Transform2D::translate(-vb_min_x, -vb_min_y));
    natural.compose(root)
}

#[test]
fn matching_aspect_ratio_yields_identity_root_transform() {
    // viewBox aspect 2:1 matches the canvas 2:1 — meet/slice are
    // moot, no correction needed.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 100 50" preserveAspectRatio="xMidYMid meet"/>"#;
    let frame = parse_svg(src).expect("parse");
    assert!(
        frame.root.transform.is_identity(),
        "no correction expected, got {:?}",
        frame.root.transform
    );
}

#[test]
fn preserve_aspect_ratio_none_yields_identity_root_transform() {
    // `none` matches the renderer's default (stretch).
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 100 100" preserveAspectRatio="none"/>"#;
    let frame = parse_svg(src).expect("parse");
    assert!(
        frame.root.transform.is_identity(),
        "preserveAspectRatio='none' should leave root identity"
    );
}

#[test]
fn xmidymid_meet_letterboxes_horizontally() {
    // Source: 200x100 canvas, 100x100 viewBox, default xMidYMid meet.
    // Per spec: sx=sy=1, tx=50, ty=0. (0,0) → (50,0); (100,100) →
    // (150,100). Effective transform must map the same way.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 100 100"/>"#;
    let frame = parse_svg(src).expect("parse");
    let eff = effective(200.0, 100.0, 0.0, 0.0, 100.0, 100.0, &frame.root.transform);
    let p00 = apply(&eff, Point::new(0.0, 0.0));
    let p100 = apply(&eff, Point::new(100.0, 100.0));
    let p50 = apply(&eff, Point::new(50.0, 50.0));
    assert!((p00.x - 50.0).abs() < 1e-3, "p00.x = {}", p00.x);
    assert!(p00.y.abs() < 1e-3, "p00.y = {}", p00.y);
    assert!((p100.x - 150.0).abs() < 1e-3, "p100.x = {}", p100.x);
    assert!((p100.y - 100.0).abs() < 1e-3, "p100.y = {}", p100.y);
    assert!((p50.x - 100.0).abs() < 1e-3);
    assert!((p50.y - 50.0).abs() < 1e-3);
}

#[test]
fn xmidymid_meet_letterboxes_vertically() {
    // 100x200 canvas, 100x100 viewBox, default xMidYMid meet:
    // sx=sy=1, tx=0, ty=50. Vertical letterbox.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="200" viewBox="0 0 100 100"/>"#;
    let frame = parse_svg(src).expect("parse");
    let eff = effective(100.0, 200.0, 0.0, 0.0, 100.0, 100.0, &frame.root.transform);
    let p00 = apply(&eff, Point::new(0.0, 0.0));
    let p100 = apply(&eff, Point::new(100.0, 100.0));
    assert!(p00.x.abs() < 1e-3);
    assert!((p00.y - 50.0).abs() < 1e-3);
    assert!((p100.x - 100.0).abs() < 1e-3);
    assert!((p100.y - 150.0).abs() < 1e-3);
}

#[test]
fn xmidymid_slice_overflows_horizontally() {
    // 200x100 canvas, 100x100 viewBox, xMidYMid slice:
    // sx=sy=2, tx=0, ty=-50. Vertical content overflows top+bottom.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 100 100" preserveAspectRatio="xMidYMid slice"/>"#;
    let frame = parse_svg(src).expect("parse");
    let eff = effective(200.0, 100.0, 0.0, 0.0, 100.0, 100.0, &frame.root.transform);
    let p_center = apply(&eff, Point::new(50.0, 50.0));
    // Center of viewBox lands at center of canvas.
    assert!(
        (p_center.x - 100.0).abs() < 1e-3,
        "p_center.x={}",
        p_center.x
    );
    assert!(
        (p_center.y - 50.0).abs() < 1e-3,
        "p_center.y={}",
        p_center.y
    );
    // Top-left of viewBox lands at (0, -50): off-canvas above.
    let p00 = apply(&eff, Point::new(0.0, 0.0));
    assert!(p00.x.abs() < 1e-3);
    assert!((p00.y + 50.0).abs() < 1e-3);
}

#[test]
fn xminymin_meet_anchors_top_left() {
    // 200x100 canvas, 100x100 viewBox, xMinYMin meet:
    // sx=sy=1, tx=0, ty=0. Letterbox is on right side.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 100 100" preserveAspectRatio="xMinYMin meet"/>"#;
    let frame = parse_svg(src).expect("parse");
    let eff = effective(200.0, 100.0, 0.0, 0.0, 100.0, 100.0, &frame.root.transform);
    let p00 = apply(&eff, Point::new(0.0, 0.0));
    let p100 = apply(&eff, Point::new(100.0, 100.0));
    assert!(p00.x.abs() < 1e-3);
    assert!(p00.y.abs() < 1e-3);
    assert!((p100.x - 100.0).abs() < 1e-3);
    assert!((p100.y - 100.0).abs() < 1e-3);
}

#[test]
fn xmaxymax_meet_anchors_bottom_right() {
    // 200x100 canvas, 100x100 viewBox, xMaxYMax meet:
    // sx=sy=1, tx=100, ty=0. Letterbox on left side.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 100 100" preserveAspectRatio="xMaxYMax meet"/>"#;
    let frame = parse_svg(src).expect("parse");
    let eff = effective(200.0, 100.0, 0.0, 0.0, 100.0, 100.0, &frame.root.transform);
    let p00 = apply(&eff, Point::new(0.0, 0.0));
    let p100 = apply(&eff, Point::new(100.0, 100.0));
    assert!((p00.x - 100.0).abs() < 1e-3, "p00.x={}", p00.x);
    assert!(p00.y.abs() < 1e-3);
    assert!((p100.x - 200.0).abs() < 1e-3);
    assert!((p100.y - 100.0).abs() < 1e-3);
}

#[test]
fn nonzero_viewbox_origin_maps_correctly_with_meet() {
    // viewBox starts at (10, 10), 200x100 canvas, viewBox 100x100,
    // default xMidYMid meet: sx=sy=1, tx = 0 - 10*1 + (200-100*1)/2
    // = -10 + 50 = 40. ty = 0 - 10*1 + 0 = -10.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="10 10 100 100"/>"#;
    let frame = parse_svg(src).expect("parse");
    let eff = effective(
        200.0,
        100.0,
        10.0,
        10.0,
        100.0,
        100.0,
        &frame.root.transform,
    );
    // viewBox origin (10, 10) lands at (40, -10) on canvas.
    let p_origin = apply(&eff, Point::new(10.0, 10.0));
    assert!(
        (p_origin.x - 40.0).abs() < 1e-3,
        "p_origin.x={}",
        p_origin.x
    );
    assert!(
        (p_origin.y + 10.0).abs() < 1e-3,
        "p_origin.y={}",
        p_origin.y
    );
}

#[test]
fn round_trip_preserves_root_preserve_aspect_ratio_attribute() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 100 100" preserveAspectRatio="xMinYMid slice">
        <rect x="0" y="0" width="100" height="100" fill="#f00"/>
    </svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    assert_eq!(
        extras.root_preserve_aspect_ratio.as_deref(),
        Some("xMinYMid slice")
    );
    let bytes = write_svg_with_extras(&frame, &extras);
    let out = String::from_utf8(bytes).expect("utf-8");
    assert!(
        out.contains("preserveAspectRatio=\"xMinYMid slice\""),
        "expected re-emitted attribute:\n{out}"
    );
}

#[test]
fn missing_preserve_aspect_ratio_extras_field_does_not_emit_attr() {
    // No PAR on input → extras field is None → no attribute on output.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100"/>"#;
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    assert!(extras.root_preserve_aspect_ratio.is_none());
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(
        !out.contains("preserveAspectRatio="),
        "no PAR was set, none should be emitted:\n{out}"
    );
}
