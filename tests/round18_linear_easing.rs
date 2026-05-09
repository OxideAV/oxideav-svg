//! Round 18 — CSS Easing Functions L2 `linear()` function.
//!
//! Round 17 added the L1 `<easing-function>` surface (`linear` /
//! `ease*` / `cubic-bezier(...)` / `steps(...)`). Round 18 extends
//! the [`oxideav_svg::keyframe::TimingFunction`] enum with the L2
//! `linear(<stop>#)` function — a piecewise-linear curve through
//! explicit `(input, output)` control points.
//!
//! The end-to-end test wires `linear(0, 0.5 25%, 1)` into an SVG
//! `<style>` block's `animation-timing-function` and confirms that
//! `parse_svg_at(t_seconds)` produces the spec-correct interpolated
//! property value.

use oxideav_svg::keyframe::{LinearStop, TimingFunction};

#[test]
fn linear_function_two_outputs_only() {
    // `linear(0, 1)` is the L2 spelling of the identity function.
    let tf = TimingFunction::parse("linear(0, 1)").unwrap();
    match &tf {
        TimingFunction::LinearStops { stops } => {
            assert_eq!(stops.len(), 2);
            assert!((stops[0].input - 0.0).abs() < 1e-6);
            assert!((stops[0].output - 0.0).abs() < 1e-6);
            assert!((stops[1].input - 1.0).abs() < 1e-6);
            assert!((stops[1].output - 1.0).abs() < 1e-6);
        }
        other => panic!("expected LinearStops, got {other:?}"),
    }
    for &t in &[0.0, 0.25, 0.5, 0.75, 1.0] {
        assert!((tf.compute_progress(t) - t).abs() < 1e-6);
    }
}

#[test]
fn linear_function_explicit_midpoint_input() {
    // `linear(0, 0.5 25%, 1)` per the round-18 dispatch spec.
    let tf = TimingFunction::parse("linear(0, 0.5 25%, 1)").unwrap();
    // Endpoint pinning.
    assert!(tf.compute_progress(0.0).abs() < 1e-6);
    assert!((tf.compute_progress(1.0) - 1.0).abs() < 1e-6);
    // Stop boundary (t=0.25) → 0.5.
    assert!((tf.compute_progress(0.25) - 0.5).abs() < 1e-6);
    // In the second segment (t ∈ [0.25, 1.0]):
    // output = 0.5 + (1.0 - 0.5) * (t - 0.25) / (1.0 - 0.25)
    let expected_at = |t: f32| 0.5 + 0.5 * (t - 0.25) / 0.75;
    for &t in &[0.3, 0.5, 0.7, 0.9] {
        assert!(
            (tf.compute_progress(t) - expected_at(t)).abs() < 1e-5,
            "t={t} got {} expected {}",
            tf.compute_progress(t),
            expected_at(t)
        );
    }
    // In the first segment (t ∈ [0.0, 0.25]):
    // output = 0.0 + (0.5 - 0.0) * (t - 0.0) / (0.25 - 0.0) = 2t.
    for &t in &[0.05, 0.1, 0.2] {
        assert!((tf.compute_progress(t) - 2.0 * t).abs() < 1e-5);
    }
}

#[test]
fn linear_function_drives_opacity_keyframes_at_t() {
    // End-to-end — feed `linear(0, 0.5 25%, 1)` through an SVG
    // animation and verify `parse_svg_at` resolves the eased opacity
    // at runtime.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <style>
    @keyframes fade { from { opacity: 0 } to { opacity: 1 } }
    .a {
      animation-name: fade;
      animation-duration: 1s;
      animation-timing-function: linear(0, 0.5 25%, 1);
      animation-fill-mode: forwards;
    }
  </style>
  <g class="a">
    <rect x="10" y="10" width="80" height="80" fill="red"/>
  </g>
</svg>"##;
    // At t=0.25s of a 1s animation the eased progress is 0.5 →
    // opacity 0.5 (verified independently by the unit-level
    // compute_progress test above).
    let frame = oxideav_svg::parse_svg_at(src, 0.25).unwrap();
    let g = match &frame.root.children[0] {
        oxideav_core::Node::Group(g) => g,
        other => panic!("expected group, got {other:?}"),
    };
    // The group's effective opacity should be 0.5 (modulo the lerp
    // formatter's f32 precision).
    let op = g.opacity;
    assert!(
        (op - 0.5).abs() < 5e-3,
        "linear() at t=0.25 should set opacity ≈ 0.5, got {op}"
    );
}

#[test]
fn linear_stop_struct_is_constructable() {
    // Public API smoke — callers can build their own LinearStops if
    // they're feeding the easing curve from a non-CSS source.
    let stops = vec![
        LinearStop {
            input: 0.0,
            output: 0.0,
        },
        LinearStop {
            input: 0.5,
            output: 1.0,
        },
        LinearStop {
            input: 1.0,
            output: 0.0,
        },
    ];
    let tf = TimingFunction::LinearStops { stops };
    // Triangular peak at 0.5 → 1.0; symmetric.
    assert!((tf.compute_progress(0.5) - 1.0).abs() < 1e-6);
    assert!((tf.compute_progress(0.0)).abs() < 1e-6);
    assert!((tf.compute_progress(1.0)).abs() < 1e-6);
    // Quarter point of either ramp → 0.5.
    assert!((tf.compute_progress(0.25) - 0.5).abs() < 1e-6);
    assert!((tf.compute_progress(0.75) - 0.5).abs() < 1e-6);
}
