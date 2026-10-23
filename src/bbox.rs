//! Object bounding boxes — SVG 2 §8.10 / §8.11.
//!
//! The *object bounding box* of a shape is "the tightest fitting
//! rectangle aligned with the axes of that element's user coordinate
//! system that entirely encloses" its geometry (§8.10). Fill, stroke
//! and markers do not contribute, and "for curved shapes, the bounding
//! box must enclose all portions of the shape along the edge, not just
//! end points" — control points that do not lie on the curve are
//! excluded, so Bézier and arc segments contribute their true extrema.
//!
//! §8.11 then uses that box to map `objectBoundingBox` paint-server /
//! clip / mask / filter geometry onto the shape: `(0,0)` → the box's
//! `(minx, miny)` corner, `(1,1)` → `(maxx, maxy)`, i.e. the matrix
//! `[(maxx-minx) 0 0 (maxy-miny) minx miny]` ([`BBox::unit_transform`]).

use crate::model::{Path, PathCommand, Point, Transform2D};

/// Axis-aligned bounding box in the owning element's user space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BBox {
    /// Left edge (`minx`).
    pub x: f32,
    /// Top edge (`miny`).
    pub y: f32,
    /// `maxx - minx`; never negative.
    pub width: f32,
    /// `maxy - miny`; never negative.
    pub height: f32,
}

impl BBox {
    /// Build a box from its origin and size.
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Build a box from two opposite corners (any order).
    pub fn from_corners(ax: f32, ay: f32, bx: f32, by: f32) -> Self {
        let x = ax.min(bx);
        let y = ay.min(by);
        Self {
            x,
            y,
            width: ax.max(bx) - x,
            height: ay.max(by) - y,
        }
    }

    /// Right edge (`maxx`).
    pub fn max_x(&self) -> f32 {
        self.x + self.width
    }

    /// Bottom edge (`maxy`).
    pub fn max_y(&self) -> f32 {
        self.y + self.height
    }

    /// `true` when the box has no width *or* no height — SVG 2 §8.11:
    /// "when the geometry of the applicable element has no width or
    /// height and `objectBoundingBox` is specified, then the given
    /// effect (e.g. a gradient or a filter) will be ignored".
    pub fn is_degenerate(&self) -> bool {
        self.width.is_nan() || self.height.is_nan() || self.width <= 0.0 || self.height <= 0.0
    }

    /// Smallest box enclosing both.
    pub fn union(&self, other: &BBox) -> BBox {
        BBox::from_corners(
            self.x.min(other.x),
            self.y.min(other.y),
            self.max_x().max(other.max_x()),
            self.max_y().max(other.max_y()),
        )
    }

    /// Grow the box by `margin` on every side (negative margins shrink
    /// but never below zero extent).
    pub fn expand(&self, margin: f32) -> BBox {
        let w = (self.width + 2.0 * margin).max(0.0);
        let h = (self.height + 2.0 * margin).max(0.0);
        BBox::new(self.x - margin, self.y - margin, w, h)
    }

    /// SVG 2 §8.11 bounding-box-units matrix: maps the abstract
    /// `(0,0)..(1,1)` box onto this box —
    /// `[(maxx-minx) 0 0 (maxy-miny) minx miny]`.
    pub fn unit_transform(&self) -> Transform2D {
        Transform2D {
            a: self.width,
            b: 0.0,
            c: 0.0,
            d: self.height,
            e: self.x,
            f: self.y,
        }
    }

    /// Bounding box of the four corners after `t` — the tightest
    /// axis-aligned box in the target space that contains the image of
    /// this box.
    pub fn transformed(&self, t: &Transform2D) -> BBox {
        let corners = [
            t.apply(Point::new(self.x, self.y)),
            t.apply(Point::new(self.max_x(), self.y)),
            t.apply(Point::new(self.x, self.max_y())),
            t.apply(Point::new(self.max_x(), self.max_y())),
        ];
        let mut acc = Accumulator::default();
        for c in corners {
            acc.add(c);
        }
        acc.finish().unwrap_or(*self)
    }

    /// SVG 2 §8.10 object bounding box of a path: the tight box of
    /// every segment's on-curve extent (line endpoints, Bézier extrema,
    /// elliptical-arc extrema). `None` when the path has no drawing
    /// command at all; a lone `M` or zero-extent subpath still yields a
    /// (degenerate) box — "an element which has zero width, zero
    /// height, or both … still has a bounding box".
    pub fn of_path(path: &Path) -> Option<BBox> {
        let mut acc = Accumulator::default();
        let mut current = Point::new(0.0, 0.0);
        let mut subpath_start = Point::new(0.0, 0.0);
        for cmd in &path.commands {
            match *cmd {
                PathCommand::MoveTo(p) => {
                    acc.add(p);
                    current = p;
                    subpath_start = p;
                }
                PathCommand::LineTo(p) => {
                    acc.add(p);
                    current = p;
                }
                PathCommand::QuadCurveTo { control, end } => {
                    acc.add(end);
                    quad_extrema(current, control, end, &mut acc);
                    current = end;
                }
                PathCommand::CubicCurveTo { c1, c2, end } => {
                    acc.add(end);
                    cubic_extrema(current, c1, c2, end, &mut acc);
                    current = end;
                }
                PathCommand::ArcTo {
                    rx,
                    ry,
                    x_axis_rot,
                    large_arc,
                    sweep,
                    end,
                } => {
                    acc.add(end);
                    arc_extrema(current, rx, ry, x_axis_rot, large_arc, sweep, end, &mut acc);
                    current = end;
                }
                PathCommand::Close => {
                    current = subpath_start;
                }
            }
        }
        acc.finish()
    }
}

/// Running min/max over points.
#[derive(Default)]
struct Accumulator {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    any: bool,
}

impl Accumulator {
    fn add(&mut self, p: Point) {
        if !(p.x.is_finite() && p.y.is_finite()) {
            return;
        }
        if !self.any {
            self.min_x = p.x;
            self.max_x = p.x;
            self.min_y = p.y;
            self.max_y = p.y;
            self.any = true;
        } else {
            self.min_x = self.min_x.min(p.x);
            self.max_x = self.max_x.max(p.x);
            self.min_y = self.min_y.min(p.y);
            self.max_y = self.max_y.max(p.y);
        }
    }

    fn finish(&self) -> Option<BBox> {
        self.any
            .then(|| BBox::from_corners(self.min_x, self.min_y, self.max_x, self.max_y))
    }
}

/// Quadratic Bézier: `B(t) = (1-t)²P0 + 2(1-t)t·C + t²P1`; the single
/// interior extremum per axis sits at `t = (P0-C) / (P0 - 2C + P1)`.
fn quad_extrema(p0: Point, c: Point, p1: Point, acc: &mut Accumulator) {
    let eval = |t: f32| -> Point {
        let mt = 1.0 - t;
        Point::new(
            mt * mt * p0.x + 2.0 * mt * t * c.x + t * t * p1.x,
            mt * mt * p0.y + 2.0 * mt * t * c.y + t * t * p1.y,
        )
    };
    for (a, b, d) in [(p0.x, c.x, p1.x), (p0.y, c.y, p1.y)] {
        let denom = a - 2.0 * b + d;
        if denom.abs() > f32::EPSILON {
            let t = (a - b) / denom;
            if t > 0.0 && t < 1.0 {
                acc.add(eval(t));
            }
        }
    }
}

/// Cubic Bézier: the derivative per axis is the quadratic
/// `3[(-P0 + 3C1 - 3C2 + P1)t² + 2(P0 - 2C1 + C2)t + (C1 - P0)]`;
/// its real roots in `(0,1)` are the interior extrema.
fn cubic_extrema(p0: Point, c1: Point, c2: Point, p1: Point, acc: &mut Accumulator) {
    let eval = |t: f32| -> Point {
        let mt = 1.0 - t;
        let w0 = mt * mt * mt;
        let w1 = 3.0 * mt * mt * t;
        let w2 = 3.0 * mt * t * t;
        let w3 = t * t * t;
        Point::new(
            w0 * p0.x + w1 * c1.x + w2 * c2.x + w3 * p1.x,
            w0 * p0.y + w1 * c1.y + w2 * c2.y + w3 * p1.y,
        )
    };
    for (a, b, c, d) in [(p0.x, c1.x, c2.x, p1.x), (p0.y, c1.y, c2.y, p1.y)] {
        // Derivative coefficients (the common factor 3 drops out).
        let qa = -a + 3.0 * b - 3.0 * c + d;
        let qb = 2.0 * (a - 2.0 * b + c);
        let qc = b - a;
        let mut roots = [0.0_f32; 2];
        let n = if qa.abs() <= f32::EPSILON {
            if qb.abs() <= f32::EPSILON {
                0
            } else {
                roots[0] = -qc / qb;
                1
            }
        } else {
            let disc = qb * qb - 4.0 * qa * qc;
            if disc < 0.0 {
                0
            } else {
                let s = disc.sqrt();
                roots[0] = (-qb + s) / (2.0 * qa);
                roots[1] = (-qb - s) / (2.0 * qa);
                2
            }
        };
        for &t in &roots[..n] {
            if t > 0.0 && t < 1.0 {
                acc.add(eval(t));
            }
        }
    }
}

/// Elliptical arc extrema. The endpoint parameterisation is converted
/// to centre form (SVG 2 §B.2.4 / §B.2.5 — the same conversion the
/// path-length sampler uses); the ellipse's axis-aligned extrema are at
/// the parametric angles where `dx/dθ = 0` / `dy/dθ = 0`, and each
/// candidate that lies within the swept range is added.
#[allow(clippy::too_many_arguments)]
fn arc_extrema(
    start: Point,
    rx: f32,
    ry: f32,
    x_axis_rot: f32,
    large_arc: bool,
    sweep: bool,
    end: Point,
    acc: &mut Accumulator,
) {
    if rx == 0.0 || ry == 0.0 {
        // §B.2.5 step 1: a zero radius degrades to a straight line —
        // endpoints only.
        return;
    }
    if (start.x - end.x).abs() < f32::EPSILON && (start.y - end.y).abs() < f32::EPSILON {
        // §B.2.5 step 1: identical endpoints omit the arc entirely.
        return;
    }
    let rx = rx.abs();
    let ry = ry.abs();
    let (cos_phi, sin_phi) = (x_axis_rot.cos(), x_axis_rot.sin());
    let dx = (start.x - end.x) * 0.5;
    let dy = (start.y - end.y) * 0.5;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;
    // §B.2.5 step 3: scale radii up when the endpoints are too far apart.
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    let (rx, ry) = if lambda > 1.0 {
        let s = lambda.sqrt();
        (rx * s, ry * s)
    } else {
        (rx, ry)
    };
    let rx2 = rx * rx;
    let ry2 = ry * ry;
    let denom = rx2 * y1p * y1p + ry2 * x1p * x1p;
    if denom <= 0.0 {
        return;
    }
    let factor = ((rx2 * ry2 - denom) / denom).max(0.0);
    let coef = factor.sqrt() * if large_arc == sweep { -1.0 } else { 1.0 };
    let cxp = coef * (rx * y1p) / ry;
    let cyp = coef * -(ry * x1p) / rx;
    let cx = cos_phi * cxp - sin_phi * cyp + (start.x + end.x) * 0.5;
    let cy = sin_phi * cxp + cos_phi * cyp + (start.y + end.y) * 0.5;
    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = uy.atan2(ux);
    let mut delta = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    if !sweep && delta > 0.0 {
        delta -= std::f32::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f32::consts::TAU;
    }

    // Point on the ellipse at parametric angle θ.
    let at = |theta: f32| -> Point {
        let (s, c) = theta.sin_cos();
        Point::new(
            cx + rx * c * cos_phi - ry * s * sin_phi,
            cy + rx * c * sin_phi + ry * s * cos_phi,
        )
    };
    // dx/dθ = -rx sinθ cosφ - ry cosθ sinφ = 0  →  tanθ = -(ry sinφ)/(rx cosφ)
    // dy/dθ = -rx sinθ sinφ + ry cosθ cosφ = 0  →  tanθ =  (ry cosφ)/(rx sinφ)
    let theta_x = (-ry * sin_phi).atan2(rx * cos_phi);
    let theta_y = (ry * cos_phi).atan2(rx * sin_phi);
    for base in [theta_x, theta_y] {
        for k in 0..2 {
            let cand = base + k as f32 * std::f32::consts::PI;
            if angle_in_sweep(cand, theta1, delta) {
                acc.add(at(cand));
            }
        }
    }
}

/// `true` when `theta` (mod 2π) lies on the arc from `theta1` sweeping
/// by `delta` (signed).
fn angle_in_sweep(theta: f32, theta1: f32, delta: f32) -> bool {
    let tau = std::f32::consts::TAU;
    let mut rel = (theta - theta1) % tau;
    if delta >= 0.0 {
        if rel < 0.0 {
            rel += tau;
        }
        rel <= delta
    } else {
        if rel > 0.0 {
            rel -= tau;
        }
        rel >= delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn empty_path_has_no_box() {
        assert!(BBox::of_path(&Path::new()).is_none());
    }

    #[test]
    fn lines_use_endpoints() {
        let mut p = Path::new();
        p.move_to(Point::new(10.0, 20.0));
        p.line_to(Point::new(50.0, 5.0));
        p.line_to(Point::new(30.0, 60.0));
        p.close();
        let b = BBox::of_path(&p).unwrap();
        assert_eq!(b, BBox::new(10.0, 5.0, 40.0, 55.0));
    }

    #[test]
    fn horizontal_line_is_degenerate_but_present() {
        let mut p = Path::new();
        p.move_to(Point::new(5.0, 10.0));
        p.line_to(Point::new(95.0, 10.0));
        let b = BBox::of_path(&p).unwrap();
        assert!(b.is_degenerate());
        assert_eq!(b.width, 90.0);
        assert_eq!(b.height, 0.0);
    }

    #[test]
    fn quad_control_point_excluded_but_curve_extent_included() {
        // SVG 2 §8.10 figure: 'M20,50 L35,100 H120 V50 Q70,10 20,50' —
        // the top control point (70,10) is outside the box; the curve
        // peaks at y = 30 (midpoint of the quadratic).
        let mut p = Path::new();
        p.move_to(Point::new(20.0, 50.0));
        p.line_to(Point::new(35.0, 100.0));
        p.line_to(Point::new(120.0, 100.0));
        p.line_to(Point::new(120.0, 50.0));
        p.quad_to(Point::new(70.0, 10.0), Point::new(20.0, 50.0));
        let b = BBox::of_path(&p).unwrap();
        assert!(close(b.x, 20.0) && close(b.max_x(), 120.0));
        assert!(close(b.y, 30.0), "top edge {}", b.y);
        assert!(close(b.max_y(), 100.0));
    }

    #[test]
    fn cubic_extrema_found() {
        // Symmetric bulge: y peaks at 0.75 * 40 = 30 above the baseline.
        let mut p = Path::new();
        p.move_to(Point::new(0.0, 0.0));
        p.cubic_to(
            Point::new(0.0, 40.0),
            Point::new(100.0, 40.0),
            Point::new(100.0, 0.0),
        );
        let b = BBox::of_path(&p).unwrap();
        assert!(close(b.y, 0.0));
        assert!(close(b.max_y(), 30.0), "max_y {}", b.max_y());
        assert!(close(b.x, 0.0) && close(b.max_x(), 100.0));
    }

    #[test]
    fn full_circle_from_two_arcs() {
        // The parser's <circle cx=50 cy=50 r=20> shape: two semicircular
        // arcs. Box must be the circle's, not the endpoints' chord.
        let mut p = Path::new();
        p.move_to(Point::new(70.0, 50.0));
        p.commands.push(PathCommand::ArcTo {
            rx: 20.0,
            ry: 20.0,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(30.0, 50.0),
        });
        p.commands.push(PathCommand::ArcTo {
            rx: 20.0,
            ry: 20.0,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(70.0, 50.0),
        });
        p.close();
        let b = BBox::of_path(&p).unwrap();
        assert!(close(b.x, 30.0) && close(b.y, 30.0), "{b:?}");
        assert!(close(b.width, 40.0) && close(b.height, 40.0), "{b:?}");
    }

    #[test]
    fn quarter_arc_only_includes_swept_extremum() {
        // Quarter circle from (10,0) to (0,10) about the origin (r=10),
        // sweeping through the +x/+y quadrant: box is 0..10 on both axes
        // and must not include (-10, 0) / (0, -10).
        let mut p = Path::new();
        p.move_to(Point::new(10.0, 0.0));
        p.commands.push(PathCommand::ArcTo {
            rx: 10.0,
            ry: 10.0,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(0.0, 10.0),
        });
        let b = BBox::of_path(&p).unwrap();
        assert!(close(b.x, 0.0) && close(b.y, 0.0), "{b:?}");
        assert!(close(b.max_x(), 10.0) && close(b.max_y(), 10.0), "{b:?}");
    }

    #[test]
    fn rotated_ellipse_arc_extrema() {
        // Half ellipse rx=20 ry=10 rotated 90°: its extent is ±10 in x
        // and ±20 in y about the centre.
        let mut p = Path::new();
        p.move_to(Point::new(0.0, 20.0));
        p.commands.push(PathCommand::ArcTo {
            rx: 20.0,
            ry: 10.0,
            x_axis_rot: std::f32::consts::FRAC_PI_2,
            large_arc: false,
            sweep: true,
            end: Point::new(0.0, -20.0),
        });
        let b = BBox::of_path(&p).unwrap();
        assert!(close(b.height, 40.0), "{b:?}");
        assert!(close(b.width, 10.0), "{b:?}");
    }

    #[test]
    fn unit_transform_maps_corners() {
        let b = BBox::new(10.0, 20.0, 40.0, 60.0);
        let t = b.unit_transform();
        assert_eq!(t.apply(Point::new(0.0, 0.0)), Point::new(10.0, 20.0));
        assert_eq!(t.apply(Point::new(1.0, 1.0)), Point::new(50.0, 80.0));
    }

    #[test]
    fn union_and_expand() {
        let a = BBox::new(0.0, 0.0, 10.0, 10.0);
        let b = BBox::new(5.0, -5.0, 10.0, 10.0);
        assert_eq!(a.union(&b), BBox::new(0.0, -5.0, 15.0, 15.0));
        assert_eq!(a.expand(2.0), BBox::new(-2.0, -2.0, 14.0, 14.0));
    }
}
