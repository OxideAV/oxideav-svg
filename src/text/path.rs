//! Arc-length sampling of a `<textPath>` path (SVG 2 §11.8.3).
//!
//! The path is flattened once into a polyline with cumulative lengths,
//! so placing `n` glyphs costs `n` binary searches instead of `n`
//! walks over the path data.

use crate::model::{Path, PathCommand, Point, Transform2D};

/// One straight piece of the flattened path.
#[derive(Clone, Copy, Debug)]
struct Seg {
    a: Point,
    b: Point,
    /// Distance along the path at `a`.
    start: f32,
    len: f32,
}

/// A path flattened for distance-along-path queries.
#[derive(Clone, Debug)]
pub(crate) struct PathSampler {
    segs: Vec<Seg>,
    total: f32,
    closed: bool,
}

impl PathSampler {
    /// Flatten `path`, mapping every coordinate through `transform`
    /// (the referenced element's own `transform`, §11.8.2). `None` when
    /// the path has no length.
    pub(crate) fn new(path: &Path, transform: &Transform2D) -> Option<Self> {
        let mut pts: Vec<(Point, bool)> = Vec::new(); // (point, starts a subpath)
        let mut cur = Point::default();
        let mut start = Point::default();
        let mut subpaths = 0usize;
        let mut last_was_close = false;
        let mut drawn_since_move = false;
        for cmd in &path.commands {
            last_was_close = false;
            match *cmd {
                PathCommand::MoveTo(p) => {
                    cur = p;
                    start = p;
                    subpaths += 1;
                    drawn_since_move = false;
                    pts.push((p, true));
                }
                PathCommand::LineTo(p) => {
                    pts.push((p, false));
                    cur = p;
                    drawn_since_move = true;
                }
                PathCommand::QuadCurveTo { control, end } => {
                    let n = curve_steps(dist(cur, control) + dist(control, end));
                    for i in 1..=n {
                        let t = i as f32 / n as f32;
                        let u = 1.0 - t;
                        pts.push((
                            Point::new(
                                u * u * cur.x + 2.0 * u * t * control.x + t * t * end.x,
                                u * u * cur.y + 2.0 * u * t * control.y + t * t * end.y,
                            ),
                            false,
                        ));
                    }
                    cur = end;
                    drawn_since_move = true;
                }
                PathCommand::CubicCurveTo { c1, c2, end } => {
                    let n = curve_steps(dist(cur, c1) + dist(c1, c2) + dist(c2, end));
                    for i in 1..=n {
                        let t = i as f32 / n as f32;
                        let u = 1.0 - t;
                        let (b0, b1, b2, b3) =
                            (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                        pts.push((
                            Point::new(
                                b0 * cur.x + b1 * c1.x + b2 * c2.x + b3 * end.x,
                                b0 * cur.y + b1 * c1.y + b2 * c2.y + b3 * end.y,
                            ),
                            false,
                        ));
                    }
                    cur = end;
                    drawn_since_move = true;
                }
                PathCommand::ArcTo {
                    rx,
                    ry,
                    x_axis_rot,
                    large_arc,
                    sweep,
                    end,
                } => {
                    flatten_arc(cur, rx, ry, x_axis_rot, large_arc, sweep, end, &mut pts);
                    cur = end;
                    drawn_since_move = true;
                }
                PathCommand::Close => {
                    pts.push((start, false));
                    cur = start;
                    last_was_close = drawn_since_move;
                }
            }
        }
        let mut segs = Vec::with_capacity(pts.len());
        let mut total = 0.0f32;
        let mut prev: Option<Point> = None;
        for (p, is_move) in pts {
            let p = transform.apply(p);
            if let Some(a) = prev
                && !is_move
            {
                let len = dist(a, p);
                if len > 0.0 && len.is_finite() {
                    segs.push(Seg {
                        a,
                        b: p,
                        start: total,
                        len,
                    });
                    total += len;
                }
            }
            prev = Some(p);
        }
        if segs.is_empty() || total <= 0.0 || !total.is_finite() {
            return None;
        }
        Some(Self {
            segs,
            total,
            // §11.8.2: the wrap-around rule applies to "paths consisting
            // of a single closed subpath".
            closed: subpaths == 1 && last_was_close,
        })
    }

    /// Total length of the flattened path.
    pub(crate) fn length(&self) -> f32 {
        self.total
    }

    /// The path is a single closed subpath (§11.8.2).
    pub(crate) fn is_closed(&self) -> bool {
        self.closed
    }

    /// The same path traversed end to start (`side="right"`).
    pub(crate) fn reversed(&self) -> Self {
        let mut segs = Vec::with_capacity(self.segs.len());
        let mut total = 0.0;
        for s in self.segs.iter().rev() {
            segs.push(Seg {
                a: s.b,
                b: s.a,
                start: total,
                len: s.len,
            });
            total += s.len;
        }
        Self {
            segs,
            total: self.total,
            closed: self.closed,
        }
    }

    /// Point and unit tangent at `distance` along the path (clamped to
    /// the path's extent).
    pub(crate) fn sample(&self, distance: f32) -> (Point, (f32, f32)) {
        let d = distance.clamp(0.0, self.total);
        let idx = match self
            .segs
            .binary_search_by(|s| s.start.partial_cmp(&d).unwrap_or(std::cmp::Ordering::Less))
        {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        };
        let s = &self.segs[idx];
        let t = ((d - s.start) / s.len).clamp(0.0, 1.0);
        let dir = ((s.b.x - s.a.x) / s.len, (s.b.y - s.a.y) / s.len);
        (
            Point::new(s.a.x + (s.b.x - s.a.x) * t, s.a.y + (s.b.y - s.a.y) * t),
            dir,
        )
    }

    /// The path's final point.
    pub(crate) fn end_point(&self) -> Point {
        self.segs.last().map(|s| s.b).unwrap_or_default()
    }
}

fn dist(a: Point, b: Point) -> f32 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

/// Number of chords for a curve whose control polygon is `poly_len`
/// long: about one per two user units, bounded.
fn curve_steps(poly_len: f32) -> usize {
    if !poly_len.is_finite() {
        return 8;
    }
    ((poly_len / 2.0).ceil() as usize).clamp(8, 512)
}

/// Flatten an SVG elliptical arc (endpoint parameterisation) by
/// converting to the centre parameterisation: with the chord midpoint
/// rotated into the ellipse frame as `(x1', y1')`, the centre is
/// `±sqrt((rx²ry² − rx²y1'² − ry²x1'²) / (rx²y1'² + ry²x1'²)) ·
/// (rx·y1'/ry, −ry·x1'/rx)` and the start / sweep angles follow from
/// the unit-circle images of the two endpoints.
#[allow(clippy::too_many_arguments)]
fn flatten_arc(
    from: Point,
    rx: f32,
    ry: f32,
    x_axis_rot_deg: f32,
    large_arc: bool,
    sweep: bool,
    to: Point,
    out: &mut Vec<(Point, bool)>,
) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 || (from.x == to.x && from.y == to.y) {
        out.push((to, false));
        return;
    }
    let phi = x_axis_rot_deg.to_radians();
    let (sin_p, cos_p) = phi.sin_cos();
    let hx = (from.x - to.x) / 2.0;
    let hy = (from.y - to.y) / 2.0;
    let x1 = cos_p * hx + sin_p * hy;
    let y1 = -sin_p * hx + cos_p * hy;
    // Scale radii up when the chord does not fit.
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = if den > 0.0 {
        (num.max(0.0) / den).sqrt()
    } else {
        0.0
    };
    if large_arc == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1 / ry;
    let cyp = -coef * ry * x1 / rx;
    let cx = cos_p * cxp - sin_p * cyp + (from.x + to.x) / 2.0;
    let cy = sin_p * cxp + cos_p * cyp + (from.y + to.y) / 2.0;
    let theta1 = ((y1 - cyp) / ry).atan2((x1 - cxp) / rx);
    let theta2 = ((-y1 - cyp) / ry).atan2((-x1 - cxp) / rx);
    let mut delta = theta2 - theta1;
    let tau = std::f32::consts::TAU;
    if sweep && delta < 0.0 {
        delta += tau;
    } else if !sweep && delta > 0.0 {
        delta -= tau;
    }
    let approx_len = delta.abs() * rx.max(ry);
    let n = curve_steps(approx_len);
    for i in 1..=n {
        let a = theta1 + delta * (i as f32 / n as f32);
        let (sa, ca) = a.sin_cos();
        let ex = rx * ca;
        let ey = ry * sa;
        out.push((
            Point::new(cos_p * ex - sin_p * ey + cx, sin_p * ex + cos_p * ey + cy),
            false,
        ));
    }
    // Land exactly on the endpoint.
    if let Some(last) = out.last_mut() {
        last.0 = to;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_line_samples_linearly() {
        let mut p = Path::new();
        p.move_to(Point::new(0.0, 0.0))
            .line_to(Point::new(100.0, 0.0));
        let s = PathSampler::new(&p, &Transform2D::identity()).unwrap();
        assert!((s.length() - 100.0).abs() < 1e-3);
        let (pt, t) = s.sample(25.0);
        assert!((pt.x - 25.0).abs() < 1e-3 && pt.y.abs() < 1e-3);
        assert!((t.0 - 1.0).abs() < 1e-5);
        assert!(!s.is_closed());
        let r = s.reversed();
        let (pt, t) = r.sample(25.0);
        assert!((pt.x - 75.0).abs() < 1e-3);
        assert!((t.0 + 1.0).abs() < 1e-5);
    }

    #[test]
    fn full_circle_from_two_arcs_has_circumference_length() {
        let mut p = Path::new();
        p.move_to(Point::new(100.0, 50.0));
        for end in [Point::new(0.0, 50.0), Point::new(100.0, 50.0)] {
            p.commands.push(PathCommand::ArcTo {
                rx: 50.0,
                ry: 50.0,
                x_axis_rot: 0.0,
                large_arc: false,
                sweep: true,
                end,
            });
        }
        p.close();
        let s = PathSampler::new(&p, &Transform2D::identity()).unwrap();
        let c = std::f32::consts::TAU * 50.0;
        assert!((s.length() - c).abs() < 0.5, "{} vs {c}", s.length());
        assert!(s.is_closed());
        // A quarter of the way round a clockwise (y-down) circle that
        // starts at the east point is the south point.
        let (pt, _) = s.sample(c / 4.0);
        assert!(
            (pt.x - 50.0).abs() < 0.5 && (pt.y - 100.0).abs() < 0.5,
            "{pt:?}"
        );
    }

    #[test]
    fn transform_scales_the_path_not_the_parameter() {
        let mut p = Path::new();
        p.move_to(Point::new(0.0, 10.0))
            .line_to(Point::new(40.0, 10.0));
        let s = PathSampler::new(&p, &Transform2D::scale(2.0, 2.0)).unwrap();
        assert!((s.length() - 80.0).abs() < 1e-3);
        assert!((s.end_point().x - 80.0).abs() < 1e-3);
    }
}
