//! Round 16 — `@keyframes` evaluation at `t_seconds` via `parse_svg_at`.
//!
//! Round 15 captured `@keyframes` blocks but never applied them to the
//! rendered scene. Round 16 closes the gap: an element whose CSS
//! cascade resolves to `animation-name: <kf>` + `animation-duration:
//! <s>` evaluates the bracketing keyframe pair at the runtime time and
//! folds the lerped property values into the element's effective
//! property map.

use oxideav_svg::parse_svg_at;

/// Helper — pluck the inner-most group's transform.a (cos rotation).
fn inner_transform(frame: &oxideav_core::VectorFrame) -> oxideav_core::Transform2D {
    use oxideav_core::Node;
    fn walk(n: &Node, depth: usize, depth_max: usize) -> Option<oxideav_core::Transform2D> {
        match n {
            Node::Group(g) => {
                if depth == depth_max {
                    return Some(g.transform);
                }
                for c in &g.children {
                    if let Some(t) = walk(c, depth + 1, depth_max) {
                        return Some(t);
                    }
                }
                None
            }
            _ => None,
        }
    }
    // Try a few depths until we find a non-identity transform.
    for d in 0..6 {
        if let Some(t) = walk(&oxideav_core::Node::Group(frame.root.clone()), 0, d) {
            if !t.is_identity() {
                return t;
            }
        }
    }
    frame.root.transform
}

#[test]
fn rotate_keyframe_at_half_duration_yields_180_degrees() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <style>
    @keyframes spin {
      from { transform: rotate(0deg) }
      to { transform: rotate(360deg) }
    }
    .a { animation-name: spin; animation-duration: 1s; }
  </style>
  <g class="a">
    <rect x="10" y="10" width="80" height="80" fill="red"/>
  </g>
</svg>"##;
    // At t=0.5s of a 1s spin, expected rotate(180deg).
    let frame = parse_svg_at(src, 0.5).unwrap();
    let t = inner_transform(&frame);
    // rotate(180deg) → cos = -1, sin = 0.
    assert!(
        (t.a + 1.0).abs() < 1e-3,
        "expected cos≈-1 (rotate 180), got a={} (full transform: {:?})",
        t.a,
        t
    );
    assert!(
        t.b.abs() < 1e-3,
        "expected sin≈0 (rotate 180), got b={}",
        t.b
    );
}

#[test]
fn rotate_at_t0_keeps_initial_orientation() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <style>
    @keyframes spin {
      from { transform: rotate(0deg) }
      to { transform: rotate(360deg) }
    }
    .a { animation-name: spin; animation-duration: 1s; }
  </style>
  <g class="a">
    <rect x="10" y="10" width="80" height="80" fill="red"/>
  </g>
</svg>"##;
    let frame = parse_svg_at(src, 0.0).unwrap();
    let t = inner_transform(&frame);
    // At t=0 the snapshot should be identity (no rotation).
    assert!(
        (t.a - 1.0).abs() < 1e-3,
        "at t=0 cos≈1 (no rotation), got a={}",
        t.a
    );
}

#[test]
fn rotate_at_t1_freezes_at_360_equivalent() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <style>
    @keyframes spin {
      from { transform: rotate(0deg) }
      to { transform: rotate(360deg) }
    }
    .a { animation-name: spin; animation-duration: 1s; }
  </style>
  <g class="a">
    <rect x="10" y="10" width="80" height="80" fill="red"/>
  </g>
</svg>"##;
    // t=1s is exactly the boundary → freezes on `to` (rotate 360 ≡
    // rotate 0 in matrix space, modulo numerical precision).
    let frame = parse_svg_at(src, 1.0).unwrap();
    let t = inner_transform(&frame);
    assert!(
        (t.a - 1.0).abs() < 1e-3,
        "at t=1 (rotate 360) cos≈1, got a={}",
        t.a
    );
}

#[test]
fn opacity_keyframe_lerps_at_runtime_t() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <style>
    @keyframes fade {
      from { opacity: 0 }
      to { opacity: 1 }
    }
    .a { animation-name: fade; animation-duration: 4s; }
  </style>
  <g class="a">
    <rect x="10" y="10" width="80" height="80" fill="blue"/>
  </g>
</svg>"##;
    // t=1 of 4s → opacity 0.25.
    let frame = parse_svg_at(src, 1.0).unwrap();
    // The wrapping group carries the `opacity` from the resolved cascade.
    let opacity = frame.root.children.iter().find_map(|n| match n {
        oxideav_core::Node::Group(g) => Some(g.opacity),
        _ => None,
    });
    assert!(opacity.is_some(), "expected a wrapping group");
    let o = opacity.unwrap();
    assert!(
        (o - 0.25).abs() < 1e-2,
        "expected opacity ≈ 0.25, got {}",
        o
    );
}

#[test]
fn t_zero_is_identical_to_parse_svg() {
    // Sanity — at t=0 the keyframed result equals the static parse
    // (the `from` keyframe is the initial state).
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
        <style>
          @keyframes fade { from { opacity: 1 } to { opacity: 0 } }
        </style>
        <rect x="0" y="0" width="50" height="50" fill="green"/>
    </svg>"##;
    let frame_t0 = parse_svg_at(src, 0.0).unwrap();
    let frame_default = oxideav_svg::parse_svg(src).unwrap();
    assert_eq!(frame_t0.width, frame_default.width);
    assert_eq!(
        frame_t0.root.children.len(),
        frame_default.root.children.len()
    );
}
