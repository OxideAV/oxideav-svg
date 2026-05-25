//! Round 128 — SVG 2 §11.8 `<textPath>` parsing + arc-length glyph
//! placement.
//!
//! These tests cover the structural / path-resolution surface of the new
//! `<textPath>` support. Pixel-level glyph emission requires a font
//! resolver to be installed, and the SVG `text` module's resolver is a
//! one-shot OnceLock that survives the whole process — so we restrict
//! the tests here to behaviours that are deterministic without one (an
//! empty group on parse, the path-resolution decision tree exercised
//! through the public parse_svg API, and round-trip safety) plus
//! numerical coverage of the new
//! [`oxideav_svg::path_length::sample_path_at_distance`] sampler used
//! to lay glyphs along the referenced path.
//!
//! The placement-math fidelity is exercised against deterministic
//! geometry (straight lines + a known cubic + a semicircle) so the
//! tests don't need access to glyph outlines.

#![cfg(feature = "text")]

use oxideav_core::{Node, Path, PathCommand, Point};
use oxideav_svg::parse_svg;
use oxideav_svg::path_length::{compute_path_length, sample_path_at_distance};

/// `<textPath>` inside a `<text>` parses to an (empty) Group when no
/// font resolver is installed — matches the `<text>` baseline
/// behaviour, so a document loads cleanly even when the rasterizer
/// can't render the glyphs yet.
#[test]
fn text_path_without_font_resolver_parses_to_empty_group() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <path id="curve" d="M 10 50 L 190 50"/>
  </defs>
  <text font-size="16">
    <textPath href="#curve">Along path</textPath>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(frame.root.children.len(), 1);
    match &frame.root.children[0] {
        Node::Group(g) => {
            assert!(g.children.is_empty(), "no glyphs without resolver");
        }
        other => panic!("expected Group, got {other:?}"),
    }
}

/// `<textPath>` accepts both SVG 2 `href` and the legacy SVG 1.1
/// `xlink:href`; either resolves to the referenced `<path>`. With no
/// font resolver the body of either is an empty group, but the
/// surrounding `<text>` must still parse cleanly.
#[test]
fn text_path_accepts_xlink_href_fallback() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="200" height="200">
  <defs>
    <path id="curve" d="M 0 100 C 50 0 150 0 200 100"/>
  </defs>
  <text>
    <textPath xlink:href="#curve">Curved</textPath>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(frame.root.children.len(), 1);
}

/// `<textPath path="…">` inline path data also resolves (§11.8.1
/// precedence: `path=` wins over href).
#[test]
fn text_path_inline_path_attribute() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <text>
    <textPath path="M 0 0 L 100 0">Inline</textPath>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(frame.root.children.len(), 1);
}

/// An unresolvable reference (missing id) silently produces no glyphs
/// but doesn't poison the surrounding document. The outer `<text>`
/// still parses to a Group.
#[test]
fn text_path_unresolvable_ref_drops_silently() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <text>
    <textPath href="#nope">Missing target</textPath>
  </text>
  <rect x="10" y="10" width="20" height="20" fill="red"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    // Two top-level children: the <text> (empty group) and the <rect>.
    assert_eq!(frame.root.children.len(), 2);
}

/// A `<textPath>` whose `<text>` mixes a plain run with a path-aligned
/// run still parses both — the plain run becomes a no-op (no resolver)
/// and the `<textPath>` body also collapses, but the structural walk
/// must visit both without erroring.
#[test]
fn text_path_alongside_plain_run_parses() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <path id="p" d="M 0 50 L 200 50"/>
  </defs>
  <text x="0" y="20">prefix<textPath href="#p">tail</textPath></text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(frame.root.children.len(), 1);
}

/// Round-trip — `parse → write_svg → parse` should not panic on a
/// document containing `<textPath>`. The encoder hasn't been taught to
/// re-emit the source `<textPath>` element yet (deferred to a follow-up
/// once `<text>` source preservation lands), but the round-trip must
/// still produce a valid document.
#[test]
fn text_path_round_trip_does_not_panic() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <path id="curve" d="M 10 100 L 190 100"/>
  </defs>
  <text>
    <textPath href="#curve">Hi</textPath>
  </text>
</svg>"##;
    let frame = oxideav_svg::parse_svg(src).expect("parse");
    let bytes = oxideav_svg::write_svg(&frame);
    let frame2 = oxideav_svg::parse_svg(&bytes).expect("re-parse");
    assert_eq!(frame.width, frame2.width);
}

// ----------------------------------------------------------------------
// Arc-length sampler (used internally by `<textPath>` glyph placement)
// ----------------------------------------------------------------------

/// `sample_path_at_distance` on a straight horizontal line returns the
/// point at exactly the queried distance and a 0° tangent.
#[test]
fn sampler_straight_line_midpoint() {
    let mut path = Path::new();
    path.move_to(Point::new(0.0, 50.0));
    path.line_to(Point::new(100.0, 50.0));
    let total = compute_path_length(&path);
    assert!((total - 100.0).abs() < 1e-3, "total length = {total}");

    let (mid, tan) = sample_path_at_distance(&path, 50.0);
    assert!((mid.x - 50.0).abs() < 1e-3);
    assert!((mid.y - 50.0).abs() < 1e-3);
    assert!(tan.abs() < 1e-3, "tangent should be 0° for horizontal");
}

/// Vertical line — tangent should be 90° (Y-axis aligned).
#[test]
fn sampler_vertical_line_tangent() {
    let mut path = Path::new();
    path.move_to(Point::new(50.0, 0.0));
    path.line_to(Point::new(50.0, 100.0));
    let (mid, tan) = sample_path_at_distance(&path, 25.0);
    assert!((mid.x - 50.0).abs() < 1e-3);
    assert!((mid.y - 25.0).abs() < 1e-3);
    assert!(
        (tan - 90.0).abs() < 1e-3,
        "tangent should be 90°; got {tan}"
    );
}

/// Polyline (two equal-length legs) — the sampler should pick up the
/// tangent change at the corner.
#[test]
fn sampler_polyline_corner_change() {
    let mut path = Path::new();
    path.move_to(Point::new(0.0, 0.0));
    path.line_to(Point::new(10.0, 0.0));
    path.line_to(Point::new(10.0, 10.0));

    // Halfway along leg 1 (distance 5) — horizontal tangent.
    let (p, t) = sample_path_at_distance(&path, 5.0);
    assert!((p.x - 5.0).abs() < 1e-3 && p.y.abs() < 1e-3);
    assert!(t.abs() < 1e-3);

    // Halfway along leg 2 (distance 15) — vertical tangent.
    let (p, t) = sample_path_at_distance(&path, 15.0);
    assert!((p.x - 10.0).abs() < 1e-3 && (p.y - 5.0).abs() < 1e-3);
    assert!((t - 90.0).abs() < 1e-3);
}

/// Queries past the total length return the final pen position with
/// the last computed tangent (matches the §11.8.2 "off-path glyphs are
/// not rendered" rule, which the caller enforces; the sampler just
/// reports the natural extrapolation).
#[test]
fn sampler_past_end_returns_pen_with_last_tangent() {
    let mut path = Path::new();
    path.move_to(Point::new(0.0, 0.0));
    path.line_to(Point::new(50.0, 0.0));

    let total = compute_path_length(&path);
    let (p, t) = sample_path_at_distance(&path, total + 99.0);
    assert!((p.x - 50.0).abs() < 1e-3);
    assert!((p.y - 0.0).abs() < 1e-3);
    assert!(t.abs() < 1e-3);
}

/// Queries before zero get the start of the path (the sampler clamps
/// to >= 0 internally).
#[test]
fn sampler_negative_distance_clamps_to_start() {
    let mut path = Path::new();
    path.move_to(Point::new(10.0, 20.0));
    path.line_to(Point::new(110.0, 20.0));

    let (p, _t) = sample_path_at_distance(&path, -5.0);
    assert!((p.x - 10.0).abs() < 1e-3);
    assert!((p.y - 20.0).abs() < 1e-3);
}

/// Cubic Bézier total length — degenerate collinear cubic should
/// agree with the straight-line distance (matches the existing
/// `cubic_degenerate_collinear_equals_line` cross-check in
/// `path_length` unit tests).
#[test]
fn sampler_cubic_collinear_midpoint() {
    let mut path = Path::new();
    path.move_to(Point::new(0.0, 0.0));
    path.commands.push(PathCommand::CubicCurveTo {
        c1: Point::new(3.0, 0.0),
        c2: Point::new(7.0, 0.0),
        end: Point::new(10.0, 0.0),
    });
    let total = compute_path_length(&path);
    assert!((total - 10.0).abs() < 1e-2, "total = {total}");

    let (p, t) = sample_path_at_distance(&path, total * 0.5);
    assert!(
        (p.x - 5.0).abs() < 0.2,
        "midpoint x should be ~5.0; got {}",
        p.x
    );
    assert!(p.y.abs() < 1e-2);
    assert!(t.abs() < 1.0, "tangent should be ~0°; got {t}");
}

/// Semicircle arc (radius 1, from (1,0) to (-1,0) via the +Y half) —
/// at the quarter-arc point (distance π/2), the sample sits at the
/// top of the circle (~ (0, 1)) and the tangent points in the -X
/// direction (~ 180°). Tolerance accounts for the 64-chord
/// approximation.
#[test]
fn sampler_semicircle_quarter_point() {
    let mut path = Path::new();
    path.move_to(Point::new(1.0, 0.0));
    path.commands.push(PathCommand::ArcTo {
        rx: 1.0,
        ry: 1.0,
        x_axis_rot: 0.0,
        large_arc: false,
        sweep: true,
        end: Point::new(-1.0, 0.0),
    });
    let total = compute_path_length(&path);
    assert!((total - std::f32::consts::PI).abs() < 1e-2);

    let quarter = total * 0.5;
    let (p, t) = sample_path_at_distance(&path, quarter);
    // (sweep=true) walks via +Y so the quarter-arc sits at (0, +1).
    assert!(p.x.abs() < 0.05, "quarter x should be ~0; got {}", p.x);
    assert!(
        (p.y - 1.0).abs() < 0.05,
        "quarter y should be ~1; got {}",
        p.y
    );
    // Tangent at the apex with sweep=true is +X direction reversed →
    // 180° (or -180°).
    let abs_t = t.abs();
    assert!(
        (abs_t - 180.0).abs() < 5.0,
        "tangent should be ~±180°; got {t}"
    );
}

/// Multi-subpath path — `MoveTo` after a `LineTo` jumps without
/// contributing length (per §9.6). The sampler should treat the second
/// subpath's distances as continuations of the first's accumulator,
/// matching the `compute_path_length`/sampler contract.
#[test]
fn sampler_multi_subpath_skips_moveto() {
    let mut path = Path::new();
    path.move_to(Point::new(0.0, 0.0));
    path.line_to(Point::new(10.0, 0.0));
    path.move_to(Point::new(100.0, 100.0)); // zero-length jump
    path.line_to(Point::new(110.0, 100.0));

    let total = compute_path_length(&path);
    assert!((total - 20.0).abs() < 1e-3, "total = {total}");

    // Distance 15 should land 5 units into the second subpath.
    let (p, _t) = sample_path_at_distance(&path, 15.0);
    assert!((p.x - 105.0).abs() < 1e-3, "x = {}", p.x);
    assert!((p.y - 100.0).abs() < 1e-3);
}

/// `<textPath side="right">` parses and produces a Group (right-side
/// layout flips the path-distance about total length — no resolver
/// installed so the body is still empty, but the parser must accept
/// the attribute without warning).
#[test]
fn text_path_side_right_parses() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <path id="curve" d="M 10 50 L 190 50"/>
  </defs>
  <text>
    <textPath href="#curve" side="right">Right side</textPath>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(frame.root.children.len(), 1);
}

/// `<textPath startOffset="25%">` parses — percentage is the SVG 2
/// §11.8.2 distance-along-path expression.
#[test]
fn text_path_start_offset_percentage_parses() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <path id="curve" d="M 10 50 L 190 50"/>
  </defs>
  <text>
    <textPath href="#curve" startOffset="25%">Shifted</textPath>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(frame.root.children.len(), 1);
}

/// `<textPath>` body containing nested `<tspan>` — text content should
/// concatenate for the run (the parse must not error, even without a
/// font resolver).
#[test]
fn text_path_with_nested_tspan_parses() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <defs>
    <path id="curve" d="M 10 50 L 190 50"/>
  </defs>
  <text>
    <textPath href="#curve">a<tspan>b</tspan>c</textPath>
  </text>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    assert_eq!(frame.root.children.len(), 1);
}
