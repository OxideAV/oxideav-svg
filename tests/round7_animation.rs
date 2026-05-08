//! Round 7 — `calcMode="paced"` and `calcMode="spline"` for SMIL
//! animation. Round 4 collapsed both to `linear`; round 7 implements
//! them per SMIL3 §4.6.
//!
//! These tests evaluate the parsed SVG at intermediate timeline points
//! and verify that the static-snapshot value the renderer sees matches
//! the easing curve.

use oxideav_svg::parse_svg_at;

/// Find the first painted path-or-group node in the frame, preferring
/// the deepest leaf so any wrapping `<g>` produced by the encoder for a
/// per-element transform doesn't shadow the path we want to inspect.
fn find_first_path(frame: &oxideav_core::VectorFrame) -> &oxideav_core::Node {
    fn walk(g: &oxideav_core::Group) -> Option<&oxideav_core::Node> {
        for c in &g.children {
            if let oxideav_core::Node::Path(_) = c {
                return Some(c);
            }
            if let oxideav_core::Node::Group(sg) = c {
                if let Some(hit) = walk(sg) {
                    return Some(hit);
                }
            }
        }
        None
    }
    walk(&frame.root).expect("no path in frame")
}

fn rect_x(node: &oxideav_core::Node) -> f32 {
    let path_node = match node {
        oxideav_core::Node::Path(p) => p,
        _ => panic!("not a path"),
    };
    match path_node.path.commands.first() {
        Some(oxideav_core::PathCommand::MoveTo(p)) => p.x,
        _ => panic!("no MoveTo"),
    }
}

#[test]
fn paced_redistributes_to_constant_attribute_speed() {
    // values 0;10;100 with linear keyTimes (0,0.5,1) at t=0.5 →
    // mid-segment lerp → 10. paced should redistribute so the value
    // half-way through the timeline is in the *attribute-space* middle
    // (~50), not the time-space middle (10).
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="20">
  <rect width="10" height="10" fill="red">
    <animate attributeName="x" calcMode="paced" values="0;10;100" dur="2s"/>
  </rect>
</svg>"##;
    // Linear baseline: mid-segment at t=1.0s should give x=10.
    let frame_lin = parse_svg_at(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="20">
  <rect width="10" height="10" fill="red">
    <animate attributeName="x" calcMode="linear" values="0;10;100" dur="2s"/>
  </rect>
</svg>"##,
        1.0,
    )
    .unwrap();
    let lin_x = rect_x(find_first_path(&frame_lin));
    assert!(
        (lin_x - 10.0).abs() < 1e-3,
        "linear at midpoint should be 10, got {lin_x}"
    );

    let frame_paced = parse_svg_at(src, 1.0).unwrap();
    let paced_x = rect_x(find_first_path(&frame_paced));
    // With paced, the segment 0→10 (length 10) takes ~10/110 of total
    // time, and 10→100 (length 90) takes ~90/110. So at time-fraction
    // 0.5 we're well inside the second segment, which means a value
    // around 50.
    assert!(
        paced_x > 30.0 && paced_x < 70.0,
        "paced midpoint should land near attribute-mid (~50), got {paced_x}"
    );
}

#[test]
fn spline_ease_in_curve_lands_below_linear_at_midpoint() {
    // keySplines = "0.42 0 1 1" is a strong ease-in; at t=0.5 the
    // remapped value should be well below the linear baseline.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20">
  <rect width="10" height="10" fill="red">
    <animate attributeName="x" calcMode="spline"
             values="0;100" keyTimes="0;1"
             keySplines="0.42 0 1 1" dur="2s"/>
  </rect>
</svg>"##;
    let frame = parse_svg_at(src, 1.0).unwrap();
    let x = rect_x(find_first_path(&frame));
    // Linear would give 50; ease-in should be noticeably less.
    assert!(x < 40.0, "ease-in spline at t=0.5 should be < 40, got {x}");
    assert!(
        x > 5.0,
        "ease-in spline at t=0.5 should still have moved off 0, got {x}"
    );
}

#[test]
fn spline_linear_curve_matches_calcmode_linear() {
    // keySplines="0 0 1 1" (the identity cubic) should reproduce
    // calcMode="linear".
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20">
  <rect width="10" height="10" fill="red">
    <animate attributeName="x" calcMode="spline"
             values="0;100" keyTimes="0;1"
             keySplines="0 0 1 1" dur="2s"/>
  </rect>
</svg>"##;
    let frame = parse_svg_at(src, 1.0).unwrap();
    let x = rect_x(find_first_path(&frame));
    assert!(
        (x - 50.0).abs() < 1.0,
        "identity-spline midpoint should be ~50, got {x}"
    );
}

#[test]
fn spline_missing_keysplines_falls_back_to_linear() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20">
  <rect width="10" height="10" fill="red">
    <animate attributeName="x" calcMode="spline"
             values="0;100" keyTimes="0;1" dur="2s"/>
  </rect>
</svg>"##;
    let frame = parse_svg_at(src, 1.0).unwrap();
    let x = rect_x(find_first_path(&frame));
    assert!(
        (x - 50.0).abs() < 1.0,
        "spline without keySplines should behave linearly, got {x}"
    );
}
