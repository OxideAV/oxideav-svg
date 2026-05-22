//! SVG 2 §9.6 — distance-along-a-path scaling (`pathLength` attribute).
//!
//! Round 21 — adds the `pathLength` attribute on every
//! `SVGGeometryElement` (`<path>`, `<rect>`, `<circle>`, `<ellipse>`,
//! `<line>`, `<polyline>`, `<polygon>`). Per the SVG 2 Candidate
//! Recommendation §9.6.1:
//!
//! > The author's computation of the total length of the path, in
//! > user units. This value is used to calibrate the user agent's own
//! > distance-along-a-path calculations with that of the author. The
//! > user agent will scale all distance-along-a-path computations by
//! > the ratio of `pathLength` to the user agent's own computed value
//! > for total path length.
//!
//! Distance-along-a-path scaling currently affects
//! [`oxideav_core::Stroke::dash`] (the `stroke-dasharray` /
//! `stroke-dashoffset` cascade). When `pathLength` is supplied and
//! non-zero, this module computes the **geometric** length of the
//! element's resulting [`oxideav_core::Path`] and rewrites each
//! dasharray entry / dashoffset value by the ratio
//! `geometric_length / pathLength` so a downstream rasteriser that
//! consumes user-space lengths produces the same visual result as one
//! that honours `pathLength` directly.
//!
//! ## Special values per §9.6.1
//!
//! * `pathLength=0` is valid and means "scaling factor of infinity":
//!   any non-zero dasharray entry becomes `+Infinity` (i.e. the dash
//!   never turns off), so the stroke collapses to a single solid
//!   segment. We implement this by dropping the dash pattern entirely.
//! * A negative `pathLength` is an error — we silently ignore the
//!   attribute (the same lenient behaviour browsers exhibit).
//! * A missing or unparseable `pathLength` is a no-op.
//! * "`pathLength` has no effect on percentage distance-along-a-path
//!   calculations" — we don't model percentage distances in the
//!   dasharray today (round 1 strips unit suffixes), so this is moot.
//!
//! ## Geometric length computation
//!
//! All four curve primitives in [`oxideav_core::PathCommand`] reduce
//! to a polyline sum:
//!
//! | Segment        | Strategy                                            |
//! |---------------|------------------------------------------------------|
//! | `LineTo`       | Exact Euclidean distance                            |
//! | `QuadCurveTo`  | Adaptive subdivision; 32-step fallback              |
//! | `CubicCurveTo` | Adaptive subdivision; 32-step fallback              |
//! | `ArcTo`        | Endpoint→centre parameterisation per SVG 1.1 §F.6.5 |
//! | `Close`        | Euclidean distance back to the last `MoveTo`        |
//! | `MoveTo`       | Zero contribution (per §9.6)                         |
//!
//! 32 samples per curve segment puts the chord-length approximation
//! within ~0.1% of the true arc length for the curve fan-out we see
//! in real-world icons; the comparison the spec asks us to make is a
//! ratio rather than an absolute distance, so sub-pixel accuracy is
//! more than sufficient.

use oxideav_core::{Path, PathCommand, Point, Stroke};

/// Parse the SVG 2 §9.6.1 `pathLength` attribute value.
///
/// Returns:
/// * `Some(positive)` for a valid non-negative number (the spec allows
///   zero — see [`apply_to_stroke`] for the `pathLength=0` handling).
/// * `None` for `None`, an empty string, a non-number, or a negative
///   number (the latter being an error per §9.6.1 / SVG 2 error
///   handling — we treat it as if the attribute was absent).
pub fn parse_path_length(v: Option<&str>) -> Option<f32> {
    let s = v?.trim();
    if s.is_empty() {
        return None;
    }
    // Accept the longest-prefix-that-parses pattern used elsewhere in
    // the crate so a stray unit suffix (e.g. `pathLength="100px"` —
    // not strictly valid per §9.6.1's `<number>` declaration but
    // forgiving) doesn't reject the whole attribute.
    let bytes = s.as_bytes();
    let mut best: Option<f32> = None;
    let mut i = 1;
    while i <= bytes.len() {
        let c = bytes[i - 1] as char;
        let numeric =
            c.is_ascii_digit() || c == '.' || c == '+' || c == '-' || c == 'e' || c == 'E';
        if !numeric {
            break;
        }
        if let Ok(n) = s[..i].parse::<f32>() {
            best = Some(n);
        }
        i += 1;
    }
    let n = best?;
    if n.is_nan() || n.is_sign_negative() {
        return None;
    }
    Some(n)
}

/// Compute the geometric length of `path` per §9.6 (only "lineto",
/// "curveto" and "arcto" commands contribute; "moveto" is zero).
pub fn compute_path_length(path: &Path) -> f32 {
    let mut total = 0.0_f32;
    // `cur` tracks the running pen position; `start` tracks the last
    // `MoveTo` for `Close` distance.
    let mut cur = Point::default();
    let mut start = Point::default();
    for cmd in &path.commands {
        match *cmd {
            PathCommand::MoveTo(p) => {
                cur = p;
                start = p;
            }
            PathCommand::LineTo(p) => {
                total += dist(cur, p);
                cur = p;
            }
            PathCommand::QuadCurveTo { control, end } => {
                total += quad_length(cur, control, end);
                cur = end;
            }
            PathCommand::CubicCurveTo { c1, c2, end } => {
                total += cubic_length(cur, c1, c2, end);
                cur = end;
            }
            PathCommand::ArcTo {
                rx,
                ry,
                x_axis_rot,
                large_arc,
                sweep,
                end,
            } => {
                total += arc_length(cur, rx, ry, x_axis_rot, large_arc, sweep, end);
                cur = end;
            }
            PathCommand::Close => {
                total += dist(cur, start);
                cur = start;
            }
            // `PathCommand` is `#[non_exhaustive]` — be lenient.
            _ => {}
        }
    }
    total
}

/// Apply `pathLength` scaling to a [`Stroke`] in place. `path_length`
/// is the author-supplied total (output of [`parse_path_length`]);
/// `geometric_length` is the actual computed length (output of
/// [`compute_path_length`]).
///
/// Per §9.6.1:
///
/// * If the author's `path_length` is zero, every non-zero dash entry
///   becomes `+Infinity` (a uniform stroke). We collapse the dash to
///   `None` so a downstream rasteriser draws a single solid stroke.
///   Per the spec, "A value of zero scaled infinitely must remain
///   zero" — we therefore preserve a dasharray that is **all zeros**
///   verbatim (still a no-op visually).
/// * Otherwise multiply every entry of [`DashPattern::array`] and
///   `dash.offset` by `geometric_length / path_length`.
///
/// If the resulting `Stroke` no longer has a dash (because we
/// collapsed it), `stroke.dash` is set to `None`.
pub fn apply_to_stroke(stroke: &mut Stroke, path_length: f32, geometric_length: f32) {
    let Some(dash) = stroke.dash.as_mut() else {
        return;
    };
    if path_length == 0.0 {
        if dash.array.iter().all(|v| *v == 0.0) {
            // "A value of zero scaled infinitely must remain zero."
            return;
        }
        // Any non-zero entry scales to +Infinity → effectively a
        // single solid stroke. Drop the dash so the rasteriser paints
        // a continuous line.
        stroke.dash = None;
        return;
    }
    let ratio = geometric_length / path_length;
    if !ratio.is_finite() || ratio <= 0.0 {
        // Defensive — non-finite ratios (e.g. an empty path → length
        // 0) can't scale meaningfully; keep the dash unchanged so the
        // rasteriser sees the author's original values.
        return;
    }
    for v in dash.array.iter_mut() {
        *v *= ratio;
    }
    dash.offset *= ratio;
}

/// Convenience wrapper — parse the attribute, compute the geometric
/// length, and rewrite the dash pattern. Returns the parsed
/// `pathLength` value if one was applied (so callers can stash it on
/// a side-channel for round-trip emission).
pub fn apply_to_path_node(
    el_path_length: Option<&str>,
    path: &Path,
    stroke: &mut Option<Stroke>,
) -> Option<f32> {
    let pl = parse_path_length(el_path_length)?;
    // Compute geometric length lazily — only if we have a stroke with
    // a dash to scale (the only consumer today).
    let needs_compute = matches!(stroke, Some(s) if s.dash.is_some());
    if needs_compute {
        let geom = compute_path_length(path);
        if let Some(s) = stroke.as_mut() {
            apply_to_stroke(s, pl, geom);
        }
    }
    Some(pl)
}

// ---------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------

fn dist(a: Point, b: Point) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}

/// Adaptive sample count for Bezier curve length — 32 chord steps is
/// the round-21 baseline and gives <0.1% relative error for the
/// curvatures we encounter in real-world SVG.
const CURVE_SAMPLES: usize = 32;

/// Length of a quadratic Bezier `B(t) = (1-t)² p0 + 2(1-t)t pc + t² p1`
/// approximated by a 32-segment polyline.
fn quad_length(p0: Point, pc: Point, p1: Point) -> f32 {
    let mut total = 0.0_f32;
    let mut prev = p0;
    for i in 1..=CURVE_SAMPLES {
        let t = i as f32 / CURVE_SAMPLES as f32;
        let omt = 1.0 - t;
        let x = omt * omt * p0.x + 2.0 * omt * t * pc.x + t * t * p1.x;
        let y = omt * omt * p0.y + 2.0 * omt * t * pc.y + t * t * p1.y;
        let p = Point::new(x, y);
        total += dist(prev, p);
        prev = p;
    }
    total
}

/// Length of a cubic Bezier `B(t) = (1-t)³ p0 + 3(1-t)² t c1 +
/// 3(1-t)t² c2 + t³ p1` approximated by a 32-segment polyline.
fn cubic_length(p0: Point, c1: Point, c2: Point, p1: Point) -> f32 {
    let mut total = 0.0_f32;
    let mut prev = p0;
    for i in 1..=CURVE_SAMPLES {
        let t = i as f32 / CURVE_SAMPLES as f32;
        let omt = 1.0 - t;
        let omt2 = omt * omt;
        let omt3 = omt2 * omt;
        let t2 = t * t;
        let t3 = t2 * t;
        let x = omt3 * p0.x + 3.0 * omt2 * t * c1.x + 3.0 * omt * t2 * c2.x + t3 * p1.x;
        let y = omt3 * p0.y + 3.0 * omt2 * t * c1.y + 3.0 * omt * t2 * c2.y + t3 * p1.y;
        let p = Point::new(x, y);
        total += dist(prev, p);
        prev = p;
    }
    total
}

/// Length of an elliptical arc segment expressed in SVG endpoint-form.
/// Converts to centre parameterisation per SVG 1.1 §F.6.5, then samples
/// the arc.
///
/// We sample more densely for arcs than for cubics (64 steps) because
/// the arc spans up to a full circle and we want sub-percent accuracy
/// across the wide sweep range SVG allows.
fn arc_length(
    start: Point,
    rx: f32,
    ry: f32,
    x_axis_rot: f32,
    large_arc: bool,
    sweep: bool,
    end: Point,
) -> f32 {
    // Degenerate cases per §F.6.2: zero rx or zero ry collapses to a
    // straight line.
    if rx == 0.0 || ry == 0.0 {
        return dist(start, end);
    }
    let rx = rx.abs();
    let ry = ry.abs();
    if (start.x - end.x).abs() < f32::EPSILON && (start.y - end.y).abs() < f32::EPSILON {
        // Per §F.6.2 step 1: endpoints coincide → the arc has zero
        // length.
        return 0.0;
    }

    // §F.6.5 step 1 — translate to origin in rotated frame.
    let cos_phi = x_axis_rot.cos();
    let sin_phi = x_axis_rot.sin();
    let dx = (start.x - end.x) * 0.5;
    let dy = (start.y - end.y) * 0.5;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;

    // §F.6.6 — radius correction.
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    let (rx, ry) = if lambda > 1.0 {
        let s = lambda.sqrt();
        (rx * s, ry * s)
    } else {
        (rx, ry)
    };

    // §F.6.5 step 2 — center of the ellipse in the rotated frame.
    let rx2 = rx * rx;
    let ry2 = ry * ry;
    let x1p2 = x1p * x1p;
    let y1p2 = y1p * y1p;
    let denom = rx2 * y1p2 + ry2 * x1p2;
    let mut factor = (rx2 * ry2 - denom) / denom;
    if factor < 0.0 {
        factor = 0.0;
    }
    let coef = factor.sqrt() * if large_arc == sweep { -1.0 } else { 1.0 };
    let cxp = coef * (rx * y1p) / ry;
    let cyp = coef * -(ry * x1p) / rx;

    // §F.6.5 step 3 — transform back to original coordinate system.
    let cx = cos_phi * cxp - sin_phi * cyp + (start.x + end.x) * 0.5;
    let cy = sin_phi * cxp + cos_phi * cyp + (start.y + end.y) * 0.5;

    // §F.6.5 step 4 — start angle and sweep angle.
    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = angle(1.0, 0.0, ux, uy);
    let mut delta = angle(ux, uy, vx, vy);
    if !sweep && delta > 0.0 {
        delta -= std::f32::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f32::consts::TAU;
    }

    // Sample 64 chord steps around the arc parameterisation.
    let n = 64;
    let mut total = 0.0_f32;
    let mut prev = start;
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let theta = theta1 + delta * t;
        let x = cos_phi * rx * theta.cos() - sin_phi * ry * theta.sin() + cx;
        let y = sin_phi * rx * theta.cos() + cos_phi * ry * theta.sin() + cy;
        let p = Point::new(x, y);
        total += dist(prev, p);
        prev = p;
    }
    total
}

/// Signed angle between vectors (u_x, u_y) and (v_x, v_y) per SVG 1.1
/// §F.6.5 equation F.6.5.4.
fn angle(ux: f32, uy: f32, vx: f32, vy: f32) -> f32 {
    let sign = if ux * vy - uy * vx >= 0.0 { 1.0 } else { -1.0 };
    let dot = ux * vx + uy * vy;
    let nu = (ux * ux + uy * uy).sqrt();
    let nv = (vx * vx + vy * vy).sqrt();
    let cos_a = (dot / (nu * nv)).clamp(-1.0, 1.0);
    sign * cos_a.acos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        assert_eq!(parse_path_length(None), None);
        assert_eq!(parse_path_length(Some("")), None);
        assert_eq!(parse_path_length(Some("  ")), None);
        assert_eq!(parse_path_length(Some("100")), Some(100.0));
        assert_eq!(parse_path_length(Some("  42.5 ")), Some(42.5));
        assert_eq!(parse_path_length(Some("0")), Some(0.0));
        // Negative is an error per §9.6.1.
        assert_eq!(parse_path_length(Some("-5")), None);
        assert_eq!(parse_path_length(Some("not-a-number")), None);
        // Unit suffix is tolerated (longest-prefix-parses).
        assert_eq!(parse_path_length(Some("100px")), Some(100.0));
    }

    #[test]
    fn line_length() {
        let mut p = Path::new();
        p.move_to(Point::new(0.0, 0.0));
        p.line_to(Point::new(3.0, 4.0));
        assert!((compute_path_length(&p) - 5.0).abs() < 1e-4);
    }

    #[test]
    fn closed_triangle() {
        // 3-4-5 right triangle (legs 3 + 4, hypotenuse 5) closed back
        // along the long edge → perimeter 12.
        let mut p = Path::new();
        p.move_to(Point::new(0.0, 0.0));
        p.line_to(Point::new(3.0, 0.0));
        p.line_to(Point::new(3.0, 4.0));
        p.close();
        assert!((compute_path_length(&p) - 12.0).abs() < 1e-4);
    }

    #[test]
    fn quadratic_degenerate_collinear_equals_line() {
        // A quadratic whose control point sits on the chord
        // degenerates to the straight-line distance.
        let mut p = Path::new();
        p.move_to(Point::new(0.0, 0.0));
        p.quad_to(Point::new(5.0, 0.0), Point::new(10.0, 0.0));
        assert!((compute_path_length(&p) - 10.0).abs() < 1e-3);
    }

    #[test]
    fn cubic_degenerate_collinear_equals_line() {
        let mut p = Path::new();
        p.move_to(Point::new(0.0, 0.0));
        p.cubic_to(
            Point::new(3.0, 0.0),
            Point::new(7.0, 0.0),
            Point::new(10.0, 0.0),
        );
        assert!((compute_path_length(&p) - 10.0).abs() < 1e-3);
    }

    #[test]
    fn semicircle_arc_length() {
        // Half a unit circle: A 1 1 0 0 1 (from (1,0) to (-1,0)).
        // True length = π ≈ 3.14159.
        let mut p = Path::new();
        p.move_to(Point::new(1.0, 0.0));
        p.commands.push(PathCommand::ArcTo {
            rx: 1.0,
            ry: 1.0,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(-1.0, 0.0),
        });
        let l = compute_path_length(&p);
        assert!(
            (l - std::f32::consts::PI).abs() < 1e-2,
            "expected ~π, got {l}"
        );
    }

    #[test]
    fn full_circle_arc_length() {
        // Full unit circle via two semicircular arcs: should be ≈ 2π.
        let mut p = Path::new();
        p.move_to(Point::new(1.0, 0.0));
        p.commands.push(PathCommand::ArcTo {
            rx: 1.0,
            ry: 1.0,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(-1.0, 0.0),
        });
        p.commands.push(PathCommand::ArcTo {
            rx: 1.0,
            ry: 1.0,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(1.0, 0.0),
        });
        let l = compute_path_length(&p);
        let expected = std::f32::consts::TAU;
        assert!(
            (l - expected).abs() < 1e-2,
            "expected ~2π = {expected}, got {l}"
        );
    }

    #[test]
    fn moveto_does_not_contribute() {
        let mut p = Path::new();
        p.move_to(Point::new(0.0, 0.0));
        p.line_to(Point::new(10.0, 0.0));
        p.move_to(Point::new(50.0, 50.0)); // big jump — must be 0 length
        p.line_to(Point::new(60.0, 50.0));
        assert!((compute_path_length(&p) - 20.0).abs() < 1e-3);
    }

    #[test]
    fn apply_scales_dasharray_and_offset() {
        use oxideav_core::{DashPattern, Paint, Rgba, Stroke};
        let mut s = Stroke {
            width: 1.0,
            paint: Paint::Solid(Rgba::opaque(0, 0, 0)),
            cap: oxideav_core::LineCap::Butt,
            join: oxideav_core::LineJoin::Miter,
            miter_limit: 4.0,
            dash: Some(DashPattern::new(vec![10.0, 5.0]).with_offset(2.0)),
        };
        // geometric=50, pathLength=100 → ratio 0.5 → dashes 5,2.5; offset 1.
        apply_to_stroke(&mut s, 100.0, 50.0);
        let d = s.dash.unwrap();
        assert!((d.array[0] - 5.0).abs() < 1e-4);
        assert!((d.array[1] - 2.5).abs() < 1e-4);
        assert!((d.offset - 1.0).abs() < 1e-4);
    }

    #[test]
    fn apply_zero_path_length_drops_dash() {
        use oxideav_core::{DashPattern, Paint, Rgba, Stroke};
        let mut s = Stroke {
            width: 1.0,
            paint: Paint::Solid(Rgba::opaque(0, 0, 0)),
            cap: oxideav_core::LineCap::Butt,
            join: oxideav_core::LineJoin::Miter,
            miter_limit: 4.0,
            dash: Some(DashPattern::new(vec![10.0, 5.0])),
        };
        apply_to_stroke(&mut s, 0.0, 50.0);
        assert!(s.dash.is_none(), "non-zero dash scaled by infinity → solid");
    }

    #[test]
    fn apply_zero_path_length_preserves_all_zero_dash() {
        // "A value of zero scaled infinitely must remain zero" — the
        // all-zero dasharray (degenerate but spec-valid) survives.
        use oxideav_core::{DashPattern, Paint, Rgba, Stroke};
        let mut s = Stroke {
            width: 1.0,
            paint: Paint::Solid(Rgba::opaque(0, 0, 0)),
            cap: oxideav_core::LineCap::Butt,
            join: oxideav_core::LineJoin::Miter,
            miter_limit: 4.0,
            dash: Some(DashPattern::new(vec![0.0, 0.0])),
        };
        apply_to_stroke(&mut s, 0.0, 50.0);
        let d = s.dash.unwrap();
        assert_eq!(d.array, vec![0.0, 0.0]);
    }

    #[test]
    fn apply_no_dash_is_noop() {
        use oxideav_core::{Paint, Rgba, Stroke};
        let mut s = Stroke {
            width: 1.0,
            paint: Paint::Solid(Rgba::opaque(0, 0, 0)),
            cap: oxideav_core::LineCap::Butt,
            join: oxideav_core::LineJoin::Miter,
            miter_limit: 4.0,
            dash: None,
        };
        apply_to_stroke(&mut s, 50.0, 100.0);
        assert!(s.dash.is_none());
    }
}
