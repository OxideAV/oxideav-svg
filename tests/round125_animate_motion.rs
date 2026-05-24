//! Round 125 — SVG 1.1 §19.2.14 `<animateMotion>` evaluator.
//!
//! Coverage:
//! * `path=` attribute (straight line, cubic curve).
//! * `<mpath xlink:href="#id">` resolved against a sibling `<path>`.
//! * `from`/`to`, `values=`, `by`-only fallback grammar.
//! * `rotate="auto"` / `"auto-reverse"` / numeric / default-0.
//! * `repeatCount`, `begin`, `dur`, `fill=freeze` end-of-anim behaviour.
//! * `keyPoints` + `keyTimes` override the natural arc-length mapping.
//! * Round-trip preservation: the `<animateMotion>` element survives a
//!   `parse_svg_with_extras → write_svg_with_extras` cycle verbatim.

use oxideav_core::{Node, Path, PathCommand};
use oxideav_svg::{parse_svg_at, parse_svg_with_extras, write_svg_with_extras};

/// Extract the transform Matrix3x2 from the first child of the scene
/// root. A shape with a `transform=` (e.g. one we injected via
/// animateMotion) is wrapped in a single-child Group whose
/// `transform` field carries the supplemental matrix.
fn first_child_transform(svg: &[u8], t: f32) -> oxideav_core::Transform2D {
    let f = parse_svg_at(svg, t).expect("parse");
    let child = f.root.children.first().expect("at least one child");
    match child {
        Node::Group(g) => g.transform,
        _ => oxideav_core::Transform2D::identity(),
    }
}

/// Parse and assert the first scene-graph child of `svg` is at position
/// `(x, y)` at time `t` with an optional rotation tolerance.
fn assert_position(svg: &[u8], t: f32, want_x: f32, want_y: f32, tol: f32) {
    let m = first_child_transform(svg, t);
    let dx = (m.e - want_x).abs();
    let dy = (m.f - want_y).abs();
    assert!(
        dx <= tol && dy <= tol,
        "at t={t}s expected translate({want_x},{want_y}) got translate({},{}); m={:?}",
        m.e,
        m.f,
        m
    );
}

#[test]
fn straight_line_path_midpoint_at_half_dur() {
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100" width="200" height="100">
  <rect width="10" height="10">
    <animateMotion dur="2s" path="M 0 0 L 100 0"/>
  </rect>
</svg>"#;
    assert_position(svg, 0.0, 0.0, 0.0, 0.5);
    assert_position(svg, 1.0, 50.0, 0.0, 1.0);
    assert_position(svg, 2.0, 100.0, 0.0, 1.0);
}

#[test]
fn from_to_translates_endpoints() {
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200" width="200" height="200">
  <rect width="10" height="10">
    <animateMotion dur="1s" from="10,20" to="30,40"/>
  </rect>
</svg>"#;
    assert_position(svg, 0.0, 10.0, 20.0, 0.5);
    assert_position(svg, 0.5, 20.0, 30.0, 0.5);
    assert_position(svg, 1.0, 30.0, 40.0, 0.5);
}

#[test]
fn values_polyline_traverses_segments() {
    // Three points define two equal-length legs (10 each). At t=0.5s
    // (half the total 2s) the pen should be at (10,0) — the corner —
    // when calcMode defaults to paced.
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
  <circle r="2">
    <animateMotion dur="2s" values="0,0; 10,0; 10,10"/>
  </circle>
</svg>"#;
    assert_position(svg, 0.0, 0.0, 0.0, 0.5);
    assert_position(svg, 1.0, 10.0, 0.0, 1.0);
    assert_position(svg, 2.0, 10.0, 10.0, 1.0);
}

#[test]
fn rotate_auto_aligns_with_tangent_for_horizontal_line() {
    // For a horizontal-right path, the tangent angle is 0deg, so the
    // transform should be a plain translate (no rotate term).
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
  <rect width="10" height="10">
    <animateMotion dur="1s" path="M 0 0 L 50 0" rotate="auto"/>
  </rect>
</svg>"#;
    let m = first_child_transform(svg, 0.5);
    // Tangent is 0 → rotation 0 → matrix [a=1, b=0, c=0, d=1, tx≈25, ty≈0].
    assert!(
        (m.a - 1.0).abs() < 1e-4,
        "a should be 1 (no rotate) — got {}",
        m.a
    );
    assert!(m.b.abs() < 1e-4, "b should be 0 — got {}", m.b);
    assert!((m.e - 25.0).abs() < 0.5, "tx ≈ 25 — got {}", m.e);
}

#[test]
fn rotate_auto_picks_up_90_deg_for_downward_path() {
    // M 0 0 L 0 100 — straight down → tangent is +90deg (atan2(100,0)).
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
  <rect width="10" height="10">
    <animateMotion dur="1s" path="M 0 0 L 0 100" rotate="auto"/>
  </rect>
</svg>"#;
    let m = first_child_transform(svg, 0.5);
    // rotate(90deg) → matrix a=0,b=1,c=-1,d=0 with translate(0,50).
    assert!(m.a.abs() < 1e-3, "a≈0 — got {}", m.a);
    assert!((m.b - 1.0).abs() < 1e-3, "b≈1 — got {}", m.b);
    assert!((m.c + 1.0).abs() < 1e-3, "c≈-1 — got {}", m.c);
    assert!(m.d.abs() < 1e-3, "d≈0 — got {}", m.d);
    assert!((m.f - 50.0).abs() < 1.0, "ty≈50 — got {}", m.f);
}

#[test]
fn rotate_auto_reverse_adds_180() {
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
  <rect width="10" height="10">
    <animateMotion dur="1s" path="M 0 0 L 50 0" rotate="auto-reverse"/>
  </rect>
</svg>"#;
    let m = first_child_transform(svg, 0.5);
    // Tangent 0 + 180 = 180 → rotate(180) → a=-1, b=0, c=0, d=-1.
    assert!((m.a + 1.0).abs() < 1e-3, "a≈-1 — got {}", m.a);
    assert!(m.b.abs() < 1e-3, "b≈0 — got {}", m.b);
    assert!((m.d + 1.0).abs() < 1e-3, "d≈-1 — got {}", m.d);
}

#[test]
fn rotate_numeric_holds_constant_angle() {
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
  <rect width="10" height="10">
    <animateMotion dur="1s" path="M 0 0 L 100 0" rotate="45"/>
  </rect>
</svg>"#;
    let m = first_child_transform(svg, 0.5);
    // rotate(45) → a=cos(45), b=sin(45)
    let s = std::f32::consts::FRAC_1_SQRT_2;
    assert!((m.a - s).abs() < 1e-3, "a≈cos45 — got {}", m.a);
    assert!((m.b - s).abs() < 1e-3, "b≈sin45 — got {}", m.b);
}

#[test]
fn mpath_resolves_referenced_path_element() {
    // The path `M 0 0 L 200 0` is defined elsewhere with id=mp; mpath
    // references it. Same expected positions as the path= form.
    let svg = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 300 100" width="300" height="100">
  <defs>
    <path id="mp" d="M 0 0 L 200 0"/>
  </defs>
  <rect width="10" height="10">
    <animateMotion dur="2s">
      <mpath xlink:href="#mp"/>
    </animateMotion>
  </rect>
</svg>"##;
    assert_position(svg, 0.0, 0.0, 0.0, 0.5);
    assert_position(svg, 1.0, 100.0, 0.0, 1.0);
    assert_position(svg, 2.0, 200.0, 0.0, 1.0);
}

#[test]
fn mpath_resolves_svg2_href_form() {
    // SVG 2 dropped the `xlink:` namespace — bare `href` should also work.
    let svg = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 300 100" width="300" height="100">
  <defs>
    <path id="mp" d="M 0 0 L 200 0"/>
  </defs>
  <rect width="10" height="10">
    <animateMotion dur="2s">
      <mpath href="#mp"/>
    </animateMotion>
  </rect>
</svg>"##;
    assert_position(svg, 1.0, 100.0, 0.0, 1.0);
}

#[test]
fn repeat_count_indefinite_keeps_repeating() {
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
  <rect width="10" height="10">
    <animateMotion dur="1s" repeatCount="indefinite" path="M 0 0 L 50 0"/>
  </rect>
</svg>"#;
    // t=0.5 within first cycle → 25; t=1.5 within second cycle → 25.
    assert_position(svg, 0.5, 25.0, 0.0, 0.5);
    assert_position(svg, 1.5, 25.0, 0.0, 0.5);
}

#[test]
fn begin_delays_animation_start() {
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
  <rect width="10" height="10">
    <animateMotion begin="2s" dur="1s" path="M 0 0 L 50 0"/>
  </rect>
</svg>"#;
    // Before begin → animation not active → no transform override.
    // The rect's identity transform should still be unchanged.
    let m = first_child_transform(svg, 1.0);
    assert!(
        (m.e).abs() < 1e-4 && (m.f).abs() < 1e-4,
        "before begin: should still be identity translate — got tx={} ty={}",
        m.e,
        m.f
    );
    // At t=2.5 we're 0.5s into the active duration → position 25.
    assert_position(svg, 2.5, 25.0, 0.0, 0.5);
}

#[test]
fn fill_freeze_default_holds_last_frame_past_end() {
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
  <rect width="10" height="10">
    <animateMotion dur="1s" path="M 0 0 L 100 0"/>
  </rect>
</svg>"#;
    // Past the end → SMIL `fill=freeze` default holds the last frame.
    assert_position(svg, 10.0, 100.0, 0.0, 1.0);
}

#[test]
fn key_points_remaps_time_to_path_distance() {
    // keyTimes/keyPoints redistribute the time→position mapping.
    // At t=0.5 (half the duration), keyPoints(0; 0.2; 1) with
    // keyTimes(0; 0.5; 1) puts us at position 0.2 along a 100-unit
    // line → x=20 (not 50).
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
  <rect width="10" height="10">
    <animateMotion dur="1s" path="M 0 0 L 100 0" keyTimes="0;0.5;1" keyPoints="0;0.2;1" calcMode="linear"/>
  </rect>
</svg>"#;
    assert_position(svg, 0.5, 20.0, 0.0, 1.0);
}

#[test]
fn cubic_path_midpoint_is_geometrically_centred() {
    // M 0 0 C 50 0 50 100 100 100 — symmetric S-curve. At paced t=0.5
    // we should be roughly at the geometric midpoint by arc length.
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200" width="200" height="200">
  <circle r="2">
    <animateMotion dur="1s" path="M 0 0 C 50 0 50 100 100 100"/>
  </circle>
</svg>"#;
    let m = first_child_transform(svg, 0.5);
    // The arc-length midpoint of this symmetric curve sits near
    // (50, 50). Generous tolerance because 32-sample chord
    // approximation drifts a few percent.
    assert!(
        (m.e - 50.0).abs() < 5.0 && (m.f - 50.0).abs() < 5.0,
        "expected (~50, ~50) — got ({}, {})",
        m.e,
        m.f
    );
}

#[test]
fn round_trip_preserves_animate_motion_verbatim() {
    let svg = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 200 200" width="200" height="200">
  <defs>
    <path id="mp" d="M 10 10 L 90 90"/>
  </defs>
  <rect id="r" width="10" height="10">
    <animateMotion dur="2s" rotate="auto" repeatCount="indefinite">
      <mpath xlink:href="#mp"/>
    </animateMotion>
  </rect>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(svg).expect("parse with extras");
    // Animation captured.
    assert!(
        !extras.animations.is_empty(),
        "<animateMotion> should ride on PreservedExtras::animations"
    );
    let out_bytes = write_svg_with_extras(&frame, &extras);
    let out = String::from_utf8(out_bytes).expect("utf-8 output");
    assert!(
        out.contains("animateMotion"),
        "round-trip should re-emit <animateMotion>: {}",
        out
    );
    assert!(
        out.contains("mpath"),
        "round-trip should re-emit the <mpath> child: {}",
        out
    );
    assert!(
        out.contains("#mp") || out.contains("\"mp\""),
        "round-trip should preserve the mpath href target: {}",
        out
    );
}

#[test]
fn empty_path_or_zero_length_yields_no_transform() {
    // M-only motion path — zero length, no segments. The animation
    // contributes nothing.
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
  <rect width="10" height="10">
    <animateMotion dur="1s" path="M 50 50"/>
  </rect>
</svg>"#;
    // Single MoveTo still pins the transform to that point.
    let m = first_child_transform(svg, 0.5);
    assert!(
        (m.e - 50.0).abs() < 0.5 && (m.f - 50.0).abs() < 0.5,
        "MoveTo-only path should pin translate at the move point — got ({},{})",
        m.e,
        m.f
    );
}

#[test]
fn malformed_path_attribute_is_ignored_gracefully() {
    // Unparseable `path=` should not poison the document.
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
  <rect width="10" height="10">
    <animateMotion dur="1s" path="not a valid path"/>
  </rect>
</svg>"#;
    let f = parse_svg_at(svg, 0.5).expect("parse must still succeed");
    // The rect child is still there (no transform override applied).
    assert!(!f.root.children.is_empty(), "scene-graph child survives");
}

#[test]
fn missing_mpath_target_falls_through_gracefully() {
    // mpath points at a non-existent id — should not poison parsing.
    let svg = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 100 100" width="100" height="100">
  <rect width="10" height="10">
    <animateMotion dur="1s">
      <mpath xlink:href="#does-not-exist"/>
    </animateMotion>
  </rect>
</svg>"##;
    let f = parse_svg_at(svg, 0.5).expect("parse must still succeed");
    assert!(!f.root.children.is_empty(), "scene-graph child survives");
}

#[test]
fn by_attribute_translates_from_origin() {
    // by-only motion: from defaults to underlying value (0,0), so
    // we walk from (0,0) to (0+by_x, 0+by_y).
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
  <rect width="10" height="10">
    <animateMotion dur="1s" from="0,0" by="40,30"/>
  </rect>
</svg>"#;
    assert_position(svg, 1.0, 40.0, 30.0, 0.5);
}

#[test]
fn precedence_path_attr_beats_values() {
    // §19.2.14: path overrides values overrides from/by/to.
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200" width="200" height="200">
  <rect width="10" height="10">
    <animateMotion dur="1s" path="M 0 0 L 50 0" values="0,0; 10,10; 200,200"/>
  </rect>
</svg>"#;
    // Path wins → final pen at (50, 0), not (200, 200).
    assert_position(svg, 1.0, 50.0, 0.0, 0.5);
}

#[test]
fn unit_test_evaluate_motion_at_directly() {
    // Verify the public API surface at its narrowest contract: pass
    // an isolated <animateMotion> element with no extras and request
    // a midpoint sample.
    use oxideav_svg::animation::evaluate_motion_at;
    use oxideav_svg::parser::Element;

    let am = Element {
        name: "animateMotion".into(),
        attrs: vec![
            ("dur".into(), "1s".into()),
            ("path".into(), "M 0 0 L 100 0".into()),
        ],
        children: vec![],
    };
    let lookup = |_: &str| None;
    let v = evaluate_motion_at(&am, 0.5, &lookup).expect("active animation");
    assert!(
        v.starts_with("translate("),
        "want translate prefix, got {v}"
    );
    // Drop the "translate(" prefix + ")" suffix and parse the two
    // numbers; expect ~50,0.
    let inner = v.trim_start_matches("translate(").trim_end_matches(')');
    let mut parts = inner.split(',');
    let x: f32 = parts.next().unwrap().parse().unwrap();
    let y: f32 = parts.next().unwrap().parse().unwrap();
    assert!((x - 50.0).abs() < 1e-3, "x ≈ 50 — got {x}");
    assert!(y.abs() < 1e-3, "y ≈ 0 — got {y}");
}

#[test]
fn path_command_arc_is_flattened_to_polyline() {
    // ArcTo motion-path sampling flattens the elliptical arc into a
    // 64-segment polyline (matching `path_length` density), so the
    // sample position at t=0.5 traverses the *arc* rather than the
    // chord. For `M 0 0 A 50 50 0 0 1 100 100` (sweep=1, small-arc)
    // the inscribed quarter-circle has its center at (100,0), so
    // the arc traverses (0,0) → (100/√2, 100−100/√2) → (100,100) and
    // the arc-length midpoint sits around (100, 0) — the geometric
    // mid-point of the quarter circle. The chord midpoint would be
    // (50, 50); the fact that the sample is far from that confirms
    // we're walking the arc, not the chord.
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200" width="200" height="200">
  <rect width="10" height="10">
    <animateMotion dur="1s" path="M 0 0 A 50 50 0 0 1 100 100"/>
  </rect>
</svg>"#;
    let m = first_child_transform(svg, 0.5);
    // Inside the arc's bounding box.
    assert!(
        m.e >= -1.0 && m.e <= 101.0 && m.f >= -1.0 && m.f <= 101.0,
        "midpoint inside the arc bounding box — got ({},{})",
        m.e,
        m.f
    );
    // Distance from the geometric arc midpoint (100, 0) — should be
    // small (the actual point is (100, 0) for a perfect quarter
    // circle).
    let dx = m.e - 100.0;
    let dy = m.f;
    let r = (dx * dx + dy * dy).sqrt();
    assert!(
        r < 5.0,
        "arc midpoint approx (100, 0) — got ({},{}) (radius from (100,0) = {})",
        m.e,
        m.f,
        r
    );
}

#[test]
fn path_node_works_as_first_child() {
    // Sanity check: a <path> with an animateMotion child also gets
    // its transform updated (not just <rect>).
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200" width="200" height="200">
  <path d="M 0 0 L 5 0 L 5 5 L 0 5 Z" fill="red">
    <animateMotion dur="1s" path="M 0 0 L 100 0"/>
  </path>
</svg>"#;
    let m = first_child_transform(svg, 0.5);
    assert!(
        (m.e - 50.0).abs() < 1.0,
        "path got translate(~50,0) — got e={}",
        m.e
    );
    // The PathNode is wrapped in a Group carrying the supplemental
    // transform from the animateMotion override; the Path is inside.
    let f = parse_svg_at(svg, 0.5).expect("parse");
    let child = f.root.children.first().expect("child");
    if let Node::Group(g) = child {
        assert!(
            matches!(g.children.first(), Some(Node::Path(_))),
            "group should wrap a Path inside: {:?}",
            g.children
        );
    } else {
        panic!(
            "expected Group wrapper around the animated Path, got {:?}",
            child
        );
    }
}

#[test]
fn motion_path_with_close_command_wraps_around() {
    // A closed triangle path. The Close command contributes its
    // chord length back to the total arc length, so paced traversal
    // at t = total reaches the subpath start, not the third vertex.
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200" width="200" height="200">
  <rect width="5" height="5">
    <animateMotion dur="3s" path="M 0 0 L 30 0 L 30 40 Z"/>
  </rect>
</svg>"#;
    // The triangle has perimeter 3+4+5 → not right. Let me recompute:
    // legs 30 + 40 = 70, hypotenuse 50; perimeter 120. The Z chord is
    // 50 long. At t=3s (end), we wrap back to the origin (0,0).
    let f = parse_svg_at(svg, 3.0).expect("parse");
    let m = first_child_transform(svg, 3.0);
    // Past-end with fill=freeze: last position is the subpath start
    // (per Close behaviour). Allow some tolerance for chord stepping.
    assert!(
        m.e.abs() < 1.0 && m.f.abs() < 1.0,
        "expected close to wrap back to origin — got ({}, {})",
        m.e,
        m.f
    );
    let _ = f; // silence unused
}

#[test]
fn empty_path_data_yields_unmodified_element() {
    // Path data parses to no commands → motion is a no-op. The
    // parent element should still emit normally.
    let svg = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
  <rect width="10" height="10">
    <animateMotion dur="1s" path=""/>
  </rect>
</svg>"#;
    let f = parse_svg_at(svg, 0.5).expect("parse");
    assert!(!f.root.children.is_empty(), "child survives");
}

/// Suppress dead-code warning on Path/PathCommand imports — they're
/// useful for diagnostic context if a test ever needs to introspect
/// the underlying `oxideav_core::Path` shape.
#[test]
fn types_in_scope() {
    let _ = std::any::TypeId::of::<Path>();
    let _ = std::any::TypeId::of::<PathCommand>();
}
