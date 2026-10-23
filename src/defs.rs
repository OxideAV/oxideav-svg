//! `<defs>`-resolved tables for round-2 cross-references.
//!
//! Round 1 only tracked gradients; round 2 adds three new tables —
//! filter, mask, clipPath, symbol — that are populated by a tree walk
//! before the main parse pass so forward references work.
//!
//! - **Filters** (`<filter>` / `filter="url(#id)"`) are stored as
//!   captured XML element subtrees. Round 2 doesn't render filters
//!   (deferred to oxideav-raster round 3 / #368), but the table lets
//!   the encoder re-emit the original definition for lossless
//!   round-trip and for downstream consumers that *can* render them.
//!
//! - **Masks** (`<mask>` / `mask="url(#id)"`) and **clipPaths**
//!   (`<clipPath>` / `clip-path="url(#id)"`) parse their child shapes
//!   into a [`crate::model::Group`] (mask) or a [`crate::model::Path`]
//!   (clipPath — multiple shapes are concatenated into one path so the
//!   even-odd / non-zero fill rule of the union approximates the SVG
//!   clip).
//!
//! - **Symbols** (`<symbol>` / future `<use>`) are captured as
//!   deferred-render groups. Round 2 doesn't yet implement `<use>` so
//!   the symbols table is wired into the parser but never consumed —
//!   this just preserves the captured definition for round 3.

use std::collections::HashMap;

use crate::bbox::BBox;
use crate::length::{Length, LengthAxis, LengthUnit, ResolveContext};
use crate::model::{
    FillRule, GradientStop, Group, LinearGradient, Paint, Path, Point, RadialGradient,
    SpreadMethod, Transform2D, ViewBox,
};

use crate::filter::{FilterGraph, PreserveAspectRatio};
use crate::parser::Element;

/// Round 20 — `patternUnits` / `patternContentUnits` value per SVG 2
/// §14.3.1. Defaults: `patternUnits` → `ObjectBoundingBox`,
/// `patternContentUnits` → `UserSpaceOnUse`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternUnits {
    UserSpaceOnUse,
    ObjectBoundingBox,
}

/// `gradientUnits` keyword per SVG 2 §14.2.2.1 / §14.2.3.1. Mirrors
/// [`PatternUnits`] but stays a distinct enum so a downstream consumer
/// can tell at a glance which paint server it's looking at. Default
/// per spec is `ObjectBoundingBox`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GradientUnits {
    /// `x1` … `fr` are user units of the referencing element (a `%`
    /// is a fraction of the current SVG viewport, §14.2.2.1).
    UserSpaceOnUse,
    /// SVG 2 §14.2.2.1 / §14.2.3.1 "initial value: objectBoundingBox":
    /// `x1` … `fr` are fractions of the referencing element's object
    /// bounding box (§8.11; a `%` is the same fraction ÷ 100).
    #[default]
    ObjectBoundingBox,
}

/// Geometry-bearing fields of a gradient definition, as authored.
///
/// Each attribute is kept as a [`Length`] (value + unit) because the
/// same literal means different things under the two `gradientUnits`
/// modes (SVG 2 §8.11): under `objectBoundingBox` a bare number is a
/// fraction of the referencing element's bounding box and `50%` is
/// `0.5`; under `userSpaceOnUse` a bare number is a user unit and `50%`
/// is half the viewport axis. The unit therefore has to survive until
/// the template chain has settled which mode applies.
///
/// Stored as `Option<_>` so the SVG 2 §14.1.1 template-inheritance
/// walker can distinguish "attribute not specified, inherit from the
/// template" from "attribute specified with an explicit value". After
/// resolution the `None`s collapse to the spec's per-attribute initial
/// value (§14.2.2.1 / §14.2.3.1: `x1=0%, y1=0%, x2=100%, y2=0%,
/// cx=50%, cy=50%, r=50%, fx=cx, fy=cy, fr=0%`).
#[derive(Clone, Debug, PartialEq)]
pub enum GradientKind {
    Linear {
        x1: Option<Length>,
        y1: Option<Length>,
        x2: Option<Length>,
        y2: Option<Length>,
    },
    Radial {
        cx: Option<Length>,
        cy: Option<Length>,
        r: Option<Length>,
        fx: Option<Length>,
        fy: Option<Length>,
        /// SVG 2-only `fr` attribute (radius of the focal circle).
        /// Defaults to 0 when unspecified per §14.2.3.1.
        fr: Option<Length>,
    },
}

impl GradientKind {
    /// A `<linearGradient>` with every geometry attribute unspecified.
    pub fn linear() -> Self {
        GradientKind::Linear {
            x1: None,
            y1: None,
            x2: None,
            y2: None,
        }
    }

    /// A `<radialGradient>` with every geometry attribute unspecified.
    pub fn radial() -> Self {
        GradientKind::Radial {
            cx: None,
            cy: None,
            r: None,
            fx: None,
            fy: None,
            fr: None,
        }
    }
}

/// `<linearGradient>` / `<radialGradient>` typed view.
///
/// SVG 2 §14.1.1 "Using paint servers as templates" lets a gradient
/// inherit any unspecified attribute (plus child stops) from a chain
/// of templates referenced via `href` / `xlink:href`. The typed
/// `GradientDef` records the *un-resolved* per-element state — each
/// attribute is `Option<_>` so the chain walker can tell `None`
/// (inherit) from `Some(..)` (explicitly specified). After resolution
/// via [`resolve_gradient_chain`] the `None`s collapse to the
/// spec-default initial values.
///
/// `stops` empty means "inherit from template," matching the §14.1.1
/// rule "if the current element does not have any child content other
/// than descriptive elements, the child content of the template
/// element is cloned to replace it."
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct GradientDef {
    pub kind: GradientKind,
    pub units: Option<GradientUnits>,
    pub transform: Option<Transform2D>,
    pub spread: Option<SpreadMethod>,
    pub stops: Vec<GradientStop>,
    /// Template reference per §14.2.2.1 / §14.2.3.1 — `href` (preferred)
    /// or legacy `xlink:href`. Stored as the bare id (`#`-stripped);
    /// empty `String` when absent. Self-references and cycles are
    /// guarded by [`resolve_gradient_chain`].
    pub href: String,
}

impl GradientDef {
    /// A gradient of the given kind with nothing else specified (every
    /// attribute inherits from the template chain or falls back to the
    /// spec initial value).
    pub fn new(kind: GradientKind) -> Self {
        Self {
            kind,
            units: None,
            transform: None,
            spread: None,
            stops: Vec::new(),
            href: String::new(),
        }
    }

    /// Set `gradientUnits`.
    pub fn with_units(mut self, units: GradientUnits) -> Self {
        self.units = Some(units);
        self
    }

    /// Set `gradientTransform`.
    pub fn with_transform(mut self, transform: Transform2D) -> Self {
        self.transform = Some(transform);
        self
    }

    /// Set `spreadMethod`.
    pub fn with_spread(mut self, spread: SpreadMethod) -> Self {
        self.spread = Some(spread);
        self
    }

    /// Replace the `<stop>` list.
    pub fn with_stops(mut self, stops: Vec<GradientStop>) -> Self {
        self.stops = stops;
        self
    }

    /// Set the template reference (bare id, no `#`).
    pub fn with_href(mut self, href: impl Into<String>) -> Self {
        self.href = href.into();
        self
    }
}

/// Final resolved gradient: every attribute pinned to a concrete
/// value, stops populated. Produced by [`resolve_gradient_chain`] /
/// [`resolve_gradient_chain_with`] from a chain of [`GradientDef`]s.
///
/// The geometry numbers are in the coordinate system `units` names:
/// bounding-box fractions under `ObjectBoundingBox` (percentages
/// already divided by 100), user units under `UserSpaceOnUse`
/// (percentages already resolved against the viewport). Bind the
/// gradient to a referencing shape with [`bind_gradient`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ResolvedGradient {
    pub kind: ResolvedGradientKind,
    pub units: GradientUnits,
    pub transform: Transform2D,
    pub spread: SpreadMethod,
    pub stops: Vec<GradientStop>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResolvedGradientKind {
    Linear {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
    Radial {
        cx: f32,
        cy: f32,
        r: f32,
        fx: f32,
        fy: f32,
        fr: f32,
    },
}

/// SVG 2 §14.1.1 template-inheritance walker with a default
/// [`ResolveContext`] (no viewport: a `%` under `userSpaceOnUse`
/// resolves to 0). Prefer [`resolve_gradient_chain_with`] when the
/// document's viewport is known.
pub fn resolve_gradient_chain(start: &GradientDef, defs: &DefsTables) -> ResolvedGradient {
    resolve_gradient_chain_with(start, defs, &ResolveContext::default())
}

/// SVG 2 §14.1.1 template-inheritance walker. Resolves `start.href`
/// chain through `defs.gradients` filling in any `None` attribute /
/// missing stops from the nearest template that *does* specify them.
/// Returns the final resolved gradient (with spec defaults substituted
/// for attributes that survive the whole chain unspecified).
///
/// `ctx` supplies the viewport a `%` resolves against under
/// `userSpaceOnUse` (§14.2.2.1: "percentages represent values relative
/// to the current SVG viewport") — `x`-ish attributes against the
/// width, `y`-ish against the height, radii against the normalised
/// diagonal (§7.10).
///
/// Cycle / depth limit: the walk caps at [`GRADIENT_HREF_DEPTH_CAP`]
/// hops (matches the eight-hop cap [`crate::css::Stylesheet::IMPORT_DEPTH_CAP`]
/// uses for CSS `@import` chains). A self-reference or a longer chain
/// terminates at the cap with whatever resolution was reached.
pub fn resolve_gradient_chain_with(
    start: &GradientDef,
    defs: &DefsTables,
    ctx: &ResolveContext,
) -> ResolvedGradient {
    use std::collections::HashSet;
    let mut chain: Vec<&GradientDef> = vec![start];
    let mut visited: HashSet<String> = HashSet::new();
    let mut href = start.href.clone();
    let mut hops = 0usize;
    while !href.is_empty() && hops < GRADIENT_HREF_DEPTH_CAP {
        if !visited.insert(href.clone()) {
            // Cycle.
            break;
        }
        match defs.gradients.get(&href) {
            Some(parent) => {
                chain.push(parent);
                href = parent.href.clone();
                hops += 1;
            }
            None => break,
        }
    }

    // Walk the chain to fill in each Option<...>, preferring the
    // child's value when present (per §14.1.1 "the value of the
    // attribute *for the template element* is used as the value for
    // the current element [only] if [the attribute is] not defined on
    // the current element").
    let mut units: Option<GradientUnits> = None;
    let mut transform: Option<Transform2D> = None;
    let mut spread: Option<SpreadMethod> = None;
    let mut stops: Vec<GradientStop> = Vec::new();

    // Geometry — kind has to come from the root (the child's element
    // type wins; a child <radialGradient href="#linear"> would
    // mis-inherit otherwise per §14.2.2.1).
    let kind_root = &start.kind;
    let (mut lin, mut rad) = match kind_root {
        GradientKind::Linear { x1, y1, x2, y2 } => (Some([*x1, *y1, *x2, *y2]), None),
        GradientKind::Radial {
            cx,
            cy,
            r,
            fx,
            fy,
            fr,
        } => (None, Some([*cx, *cy, *r, *fx, *fy, *fr])),
    };

    for def in &chain {
        // Walk in source order (child first). For each Option<_> on
        // `def`, only overwrite the running value if it's still None.
        if units.is_none() {
            units = def.units;
        }
        if transform.is_none() {
            transform = def.transform;
        }
        if spread.is_none() {
            spread = def.spread;
        }
        if stops.is_empty() && !def.stops.is_empty() {
            stops = def.stops.clone();
        }

        // Geometry fill-in — same shape as units/etc but per-component.
        // Only same-kind templates contribute geometry (a radial
        // gradient inheriting from a linear template just keeps the
        // child's defaults per §14.2.3.1).
        match (&def.kind, &mut lin, &mut rad) {
            (GradientKind::Linear { x1, y1, x2, y2 }, Some(l), _) => {
                for (slot, v) in l.iter_mut().zip([x1, y1, x2, y2]) {
                    if slot.is_none() {
                        *slot = *v;
                    }
                }
            }
            (
                GradientKind::Radial {
                    cx,
                    cy,
                    r,
                    fx,
                    fy,
                    fr,
                },
                _,
                Some(rd),
            ) => {
                for (slot, v) in rd.iter_mut().zip([cx, cy, r, fx, fy, fr]) {
                    if slot.is_none() {
                        *slot = *v;
                    }
                }
            }
            _ => {}
        }
    }

    let units = units.unwrap_or_default();
    // §8.11: under objectBoundingBox "a percentage represents the same
    // value as the corresponding decimal value (e.g., 50% means the
    // same as 0.5)"; any other unit suffix is meaningless in box space
    // and the bare number is used. Under userSpaceOnUse the length
    // resolves like any other coordinate, with `%` against the
    // viewport axis.
    let resolve = |len: Option<Length>, initial: Length, axis: LengthAxis| -> f32 {
        let len = len.unwrap_or(initial);
        match units {
            GradientUnits::ObjectBoundingBox => match len.unit {
                LengthUnit::Percent => len.value / 100.0,
                _ => len.value,
            },
            GradientUnits::UserSpaceOnUse => {
                len.resolve(ctx.with_percentage_basis(ctx.percentage_basis_for(axis)))
            }
        }
    };
    let pct = |v: f32| Length::new(v, LengthUnit::Percent);

    let kind = if let Some([x1, y1, x2, y2]) = lin {
        ResolvedGradientKind::Linear {
            // §14.2.2.1 initial values: 0%, 0%, 100%, 0%.
            x1: resolve(x1, pct(0.0), LengthAxis::X),
            y1: resolve(y1, pct(0.0), LengthAxis::Y),
            x2: resolve(x2, pct(100.0), LengthAxis::X),
            y2: resolve(y2, pct(0.0), LengthAxis::Y),
        }
    } else if let Some([cx, cy, r, fx, fy, fr]) = rad {
        // §14.2.3.1 initial values: cx=cy=r=50%, fx=cx, fy=cy, fr=0%.
        // "If attribute fx is not specified, fx will coincide with the
        // presentational value of cx for the element whether the value
        // for cx was inherited or not."
        let cx_v = resolve(cx, pct(50.0), LengthAxis::X);
        let cy_v = resolve(cy, pct(50.0), LengthAxis::Y);
        ResolvedGradientKind::Radial {
            cx: cx_v,
            cy: cy_v,
            r: resolve(r, pct(50.0), LengthAxis::Diagonal),
            fx: fx.map_or(cx_v, |l| resolve(Some(l), pct(0.0), LengthAxis::X)),
            fy: fy.map_or(cy_v, |l| resolve(Some(l), pct(0.0), LengthAxis::Y)),
            fr: resolve(fr, pct(0.0), LengthAxis::Diagonal),
        }
    } else {
        unreachable!("kind_root must be Linear or Radial");
    };

    ResolvedGradient {
        kind,
        units,
        transform: transform.unwrap_or_else(Transform2D::identity),
        spread: spread.unwrap_or(SpreadMethod::Pad),
        stops,
    }
}

/// What a resolved gradient becomes once bound to one referencing
/// shape (SVG 2 §8.11 / §14.2).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum GradientPaint {
    /// Nothing is painted: no `<stop>` (§14.2.4.3 "as if `none` were
    /// specified"), an `objectBoundingBox` gradient on a shape whose box
    /// has no width or height (§8.11 "the given effect … will be
    /// ignored"), a radial gradient whose end circle has no radius
    /// (§14.2.3.2 start and end circles fully overlap — "the area
    /// stays untouched"), or a singular `gradientTransform`.
    None,
    /// A paint whose geometry is expressed directly in the referencing
    /// element's user space — the gradient line / circle was mapped
    /// through the bounding-box matrix and `gradientTransform`.
    Paint(Paint),
    /// A paint that only exists in the gradient's own coordinate
    /// system: `transform` maps that system onto the referencing
    /// element's user space and `paint` is expressed in it. Produced
    /// for a radial gradient under a non-conformal mapping (a
    /// non-square bounding box or a skewing / non-uniform
    /// `gradientTransform`): §14.2.3.1 "the rings … will render as
    /// elliptical", and the scene-graph [`RadialGradient`] only carries
    /// a circle. A consumer paints `paint` inside a group carrying
    /// `transform`; the shape builder does exactly that through a
    /// soft-masked, transformed fill (see `crate::element`).
    Transformed {
        /// Gradient space → user space.
        transform: Transform2D,
        /// The gradient in its own space (circular rings).
        paint: Paint,
    },
}

/// Bind a resolved gradient to a referencing shape.
///
/// `bbox` is the shape's object bounding box in its own user space
/// (`None` for a shape with no geometry). It is only consulted under
/// `gradientUnits="objectBoundingBox"`.
///
/// The gradient geometry goes through `M = B ∘ gradientTransform`,
/// where `B` is the §8.11 bounding-box matrix (identity under
/// `userSpaceOnUse`) — "gradientTransform … is post-multiplied to
/// (i.e., inserted to the right of) any previously defined
/// transformations, including the implicit transformation necessary to
/// convert from object bounding box units to user coordinate system".
///
/// * Linear (§14.2.2): any affine image of a linear gradient is again
///   a linear gradient — its iso-lines stay parallel — so the result is
///   always exact. When the mapping keeps the gradient normal
///   perpendicular (axis-parallel vector under an axis-aligned `M`, or
///   a conformal `M`) the endpoints are simply mapped; otherwise the
///   equivalent perpendicular-normal line is derived from the mapped
///   iso-line field (the spec's "may render non-perpendicular relative
///   to the gradient vector in user space" case).
/// * Radial (§14.2.3): exact under a conformal `M` (uniform scale,
///   rotation, reflection — circles stay circles); otherwise
///   [`GradientPaint::Transformed`].
/// * Stops are normalised per §14.2.4.3 (offsets clamped to `0..=1`
///   and made monotonic); one stop paints its solid colour; a
///   zero-length gradient vector paints the last stop (§14.2.2.2).
pub fn bind_gradient(r: &ResolvedGradient, bbox: Option<BBox>) -> GradientPaint {
    let stops = normalised_stops(&r.stops);
    if stops.is_empty() {
        return GradientPaint::None;
    }
    if stops.len() == 1 {
        return GradientPaint::Paint(Paint::Solid(stops[0].color));
    }
    let last = stops[stops.len() - 1].color;

    let m = match r.units {
        GradientUnits::ObjectBoundingBox => match bbox {
            Some(b) if !b.is_degenerate() => b.unit_transform().compose(&r.transform),
            _ => return GradientPaint::None,
        },
        GradientUnits::UserSpaceOnUse => r.transform,
    };
    let det = m.determinant();
    if det == 0.0 || !det.is_finite() {
        return GradientPaint::None;
    }

    match r.kind {
        ResolvedGradientKind::Linear { x1, y1, x2, y2 } => {
            let p1 = Point::new(x1, y1);
            let p2 = Point::new(x2, y2);
            if p1 == p2 {
                // §14.2.2.2 "If x1 = x2 and y1 = y2, then the area to be
                // painted will be painted as a single color using the
                // color and opacity of the last gradient stop."
                return GradientPaint::Paint(Paint::Solid(last));
            }
            let axis_parallel = (m.b == 0.0 && m.c == 0.0) && (x1 == x2 || y1 == y2);
            let (start, end) = if axis_parallel || is_conformal(&m) {
                (m.apply(p1), m.apply(p2))
            } else {
                // General affine: the gradient parameter is the linear
                // field t(p) = ((M⁻¹p − p1)·v) / |v|² with v = p2 − p1.
                // Writing t(p) = g·p + c (g in user space), the
                // perpendicular-normal line with the same field runs
                // from S = −c·g/|g|² to E = S + g/|g|².
                let inv = match m.invert() {
                    Some(i) => i,
                    None => return GradientPaint::None,
                };
                let vx = x2 - x1;
                let vy = y2 - y1;
                let v2 = vx * vx + vy * vy;
                // g = A⁻ᵀ v / |v|²  (A⁻¹ = [inv.a inv.c; inv.b inv.d]).
                let gx = (inv.a * vx + inv.b * vy) / v2;
                let gy = (inv.c * vx + inv.d * vy) / v2;
                // c = ((inv.e, inv.f) − p1)·v / |v|²
                let c = ((inv.e - x1) * vx + (inv.f - y1) * vy) / v2;
                let g2 = gx * gx + gy * gy;
                if !(g2 > 0.0 && g2.is_finite()) {
                    return GradientPaint::Paint(Paint::Solid(last));
                }
                let sx = -c * gx / g2;
                let sy = -c * gy / g2;
                (Point::new(sx, sy), Point::new(sx + gx / g2, sy + gy / g2))
            };
            GradientPaint::Paint(Paint::LinearGradient(LinearGradient {
                start,
                end,
                stops,
                spread: r.spread,
            }))
        }
        ResolvedGradientKind::Radial {
            cx,
            cy,
            r: radius,
            fx,
            fy,
            ..
        } => {
            if radius.is_nan() || radius <= 0.0 {
                // §14.2.3.1: a negative `r` is an error; §14.2.3.2: a
                // start circle that fully overlaps the end circle paints
                // nothing. (`fr` has no scene-graph carrier yet and is
                // not applied.)
                return GradientPaint::None;
            }
            let centre = Point::new(cx, cy);
            let focal = Point::new(fx, fy);
            let focal_opt = |f: Point, c: Point| {
                if (f.x - c.x).abs() < 1e-6 && (f.y - c.y).abs() < 1e-6 {
                    None
                } else {
                    Some(f)
                }
            };
            if is_conformal(&m) {
                let scale = (m.a * m.a + m.b * m.b).sqrt();
                let c_u = m.apply(centre);
                let f_u = m.apply(focal);
                GradientPaint::Paint(Paint::RadialGradient(RadialGradient {
                    center: c_u,
                    radius: radius * scale,
                    focal: focal_opt(f_u, c_u),
                    stops,
                    spread: r.spread,
                }))
            } else {
                GradientPaint::Transformed {
                    transform: m,
                    paint: Paint::RadialGradient(RadialGradient {
                        center: centre,
                        radius,
                        focal: focal_opt(focal, centre),
                        stops,
                        spread: r.spread,
                    }),
                }
            }
        }
    }
}

/// `true` when the linear part of `t` is a similarity (uniform scale ×
/// rotation, optionally reflected): columns of equal length and
/// orthogonal to each other, so circles map to circles.
fn is_conformal(t: &Transform2D) -> bool {
    let c1 = t.a * t.a + t.b * t.b;
    let c2 = t.c * t.c + t.d * t.d;
    let dot = t.a * t.c + t.b * t.d;
    let scale = c1.max(c2);
    if scale.is_nan() || scale <= 0.0 || !scale.is_finite() {
        return false;
    }
    let eps = scale * 1e-5;
    (c1 - c2).abs() <= eps && dot.abs() <= eps
}

/// SVG 2 §14.2.4.3 stop normalisation: "Gradient offset values less
/// than 0 … are rounded up to 0%. Gradient offset values greater than
/// 1 … are rounded down to 100%. … If a given gradient stop's offset
/// value is not equal to or greater than all previous offset values,
/// then the offset value is adjusted to be equal to the largest of all
/// previous offset values."
pub fn normalised_stops(stops: &[GradientStop]) -> Vec<GradientStop> {
    let mut out = Vec::with_capacity(stops.len());
    let mut running = 0.0_f32;
    for s in stops {
        let mut off = if s.offset.is_finite() {
            s.offset.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if off < running {
            off = running;
        }
        running = off;
        out.push(GradientStop::new(off, s.color));
    }
    out
}

/// Per §14.1.1 the template chain "can be indirect to an arbitrary
/// level"; in practice browsers cap recursion to avoid pathological
/// DoS shapes. Mirror the round-13 CSS `@import` depth cap.
pub const GRADIENT_HREF_DEPTH_CAP: usize = 8;

/// Captured `<filter id="...">` element. Round 2 stored the original
/// element verbatim so the encoder could re-emit it; round 7 also
/// parses the primitive graph into a typed [`FilterGraph`] so a
/// downstream rasterizer (oxideav-raster) can consume the pipeline
/// without re-parsing XML.
///
/// Both fields stay populated — `element` is the source of truth for
/// round-trip emission (preserves attribute ordering, comments,
/// unknown primitives), while `graph` is the typed view consumers
/// should prefer for actual rendering.
#[derive(Clone, Debug)]
pub struct FilterDef {
    pub element: Element,
    pub graph: FilterGraph,
}

/// Captured `<mask id="...">`. The mask subtree is pre-parsed into a
/// `Group` so the resolver can wrap content in
/// [`crate::model::Node::SoftMask`] without re-walking the XML.
#[derive(Clone, Debug)]
pub struct MaskDef {
    /// `mask-type="luminance"` (default) or `"alpha"`.
    pub mask_kind: crate::model::MaskKind,
    pub content: Group,
}

/// Captured `<clipPath id="...">`. Round 2 collapses every child shape
/// into a single concatenated [`Path`] — the fill-rule union of the
/// shapes approximates the SVG semantics (which is "the union of every
/// child's filled interior").
///
/// Round 215 (SVG 1.1 §14.3.5) adds `clip_rule`: the resolved fill rule
/// (`nonzero` / `evenodd`) that determines clip-path interior membership.
/// The property cascades from the `<clipPath>` element to its child
/// shapes; a per-shape `clip-rule=` attribute overrides. Multiple child
/// shapes are merged into one [`Path`], so the def records the rule of
/// the **first** child shape (the others are assumed to share it — when
/// they don't, only the merged-path rule is preserved at the clip
/// evaluation site; the author's original per-child attribute survives
/// the round-trip via [`crate::preserved::PreservedExtras::clip_rules`]).
/// Initial value `nonzero` per the §14.3.5 attribute table.
#[derive(Clone, Debug)]
pub struct ClipPathDef {
    pub path: Path,
    /// Round 215 — SVG 1.1 §14.3.5 `clip-rule` resolved for the merged
    /// path. Initial value [`FillRule::NonZero`].
    pub clip_rule: FillRule,
}

/// Captured `<symbol id="...">`. Like `<defs>`, symbols are deferred
/// definitions — they only render when referenced via `<use>`. Stored
/// as a Group so the resolver can clone the subtree at the use site.
///
/// Round 14 extends the captured definition with the symbol's own
/// `viewBox`, intrinsic `width` / `height`, and `preserveAspectRatio`
/// so the [`crate::element::parse_use_element`] resolver can compute
/// the SVG 2 §5.5 / §8.2 viewport transform when a `<use>`
/// instantiates the symbol with its own `width` / `height`.
///
/// `view_box` is `None` when the source `<symbol>` had no `viewBox=`
/// attribute (in which case there's no viewport mapping to apply —
/// the use's `width`/`height` are ignored per spec).
/// `intrinsic_*` are the symbol's own `width=` / `height=` attributes
/// when present; the use site falls back to these if it doesn't carry
/// its own.
#[derive(Clone, Debug)]
pub struct SymbolDef {
    pub content: Group,
    pub view_box: Option<ViewBox>,
    pub preserve_aspect_ratio: PreserveAspectRatio,
    pub intrinsic_width: Option<f32>,
    pub intrinsic_height: Option<f32>,
    /// SVG 2 §5.5 `x` / `y` geometry properties — "have the same effect
    /// as on an `svg` element, when the `symbol` is instantiated by a
    /// `use` element". They position the symbol's viewport inside the
    /// `<use>`'s coordinate system (the use's own `x` / `y` translate is
    /// applied on top). `None` (treated as 0) when the attribute was
    /// absent. New in SVG 2.
    pub intrinsic_x: Option<f32>,
    pub intrinsic_y: Option<f32>,
    /// SVG 2 §5.5 `refX` / `refY` — the symbol's reference point, given
    /// in the symbol's own (post-`viewBox`) coordinate system. When a
    /// `<use>` instantiates the symbol, this point is aligned with the
    /// use's `x` / `y` position ("Similar to the matching attributes on
    /// `marker`"). `None` when the attribute was absent. The geometric
    /// keywords (`left` / `center` / `right` for `refX`; `top` /
    /// `center` / `bottom` for `refY`) resolve against the `viewBox`
    /// extent at parse time, mirroring the `<marker>` handling.
    pub ref_x: Option<f32>,
    pub ref_y: Option<f32>,
}

/// Round 20 — captured `<pattern id="...">` paint server. SVG 2 §14.3.
///
/// `crate::model::Paint` lacks a `Pattern` variant — the IR was last
/// frozen with only `Solid` / `LinearGradient` / `RadialGradient`. Round
/// 20 captures every `<pattern>` we see (typed view + verbatim XML on
/// the side-channel) so the encoder round-trips the definition AND a
/// downstream rasterizer that picks up the typed view can tile the
/// content correctly without re-parsing.
///
/// The fill / stroke resolver falls back to the SVG 2 §13.2 paint-list
/// fallback colour (`fill="url(#pat) red"`) for a non-pattern-aware
/// scene-graph renderer; once `Paint::Pattern` lands in oxideav-core
/// this typed view becomes the input to the [`Paint`] constructor and
/// the fallback path becomes a real edge case (invalid id, recursive
/// reference, etc.) per the spec.
#[derive(Clone, Debug)]
pub struct PatternDef {
    /// Tile reference rectangle. Stored in the units indicated by
    /// `pattern_units` (default `ObjectBoundingBox` per §14.3.1, in
    /// which case `0..=1` spans the referencing element's bounding
    /// box).
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Coordinate system for `x` / `y` / `width` / `height`. Default
    /// `ObjectBoundingBox`.
    pub pattern_units: PatternUnits,
    /// Coordinate system for the pattern's tile content. Default
    /// `UserSpaceOnUse`. Per §14.3.1, ignored when `view_box` is set.
    pub pattern_content_units: PatternUnits,
    /// Per-tile transform from the `patternTransform` attribute.
    pub pattern_transform: Transform2D,
    /// `viewBox` mapping inside the tile (§14.3.2 supplemental
    /// transform).
    pub view_box: Option<ViewBox>,
    /// `preserveAspectRatio` inside the tile (default `xMidYMid meet`).
    pub preserve_aspect_ratio: PreserveAspectRatio,
    /// Template reference — `href` (or legacy `xlink:href`) per
    /// §14.3.1. Stored as the bare id (`#`-stripped); empty `String`
    /// when absent. Pattern chaining (template inheritance of `x` /
    /// `y` / `width` / `height` / `viewBox` / `preserveAspectRatio` /
    /// `patternTransform` / `patternUnits` / `patternContentUnits`
    /// from the referenced template) is the rasterizer's job — round
    /// 20 captures the reference faithfully.
    pub href: String,
    /// Parsed tile content as a `Group`. Empty group when the source
    /// `<pattern>` had no renderable children.
    pub content: Group,
}

/// Round 104 — `markerUnits` keyword per SVG 2 §13.7.1. Defines the
/// coordinate system for `markerWidth` / `markerHeight` and the marker
/// contents. Default per spec is `strokeWidth`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarkerUnits {
    /// `markerUnits="strokeWidth"` — marker geometry is scaled by the
    /// painted stroke width of the referencing element (§13.7.1).
    #[default]
    StrokeWidth,
    /// `markerUnits="userSpaceOnUse"` — marker geometry is in the
    /// current user coordinate system of the referencing element.
    UserSpaceOnUse,
}

impl MarkerUnits {
    /// Parse the `markerUnits` keyword. Unknown / malformed values fall
    /// back to the spec default `strokeWidth` per the §13.7.1 tolerance
    /// rule.
    pub fn parse(v: Option<&str>) -> Self {
        match v.map(str::trim) {
            Some("userSpaceOnUse") => MarkerUnits::UserSpaceOnUse,
            // `strokeWidth` and anything unrecognised collapse to the
            // default.
            _ => MarkerUnits::StrokeWidth,
        }
    }

    /// Emit the canonical keyword for round-trip serialisation.
    pub fn as_str(self) -> &'static str {
        match self {
            MarkerUnits::StrokeWidth => "strokeWidth",
            MarkerUnits::UserSpaceOnUse => "userSpaceOnUse",
        }
    }
}

/// Round 104 — `orient` attribute value per SVG 2 §13.7.1. Indicates
/// how the marker is rotated when placed at its position on the shape.
/// `orient="auto | auto-start-reverse | <angle> | <number>"`; default
/// `0` (a fixed zero-degree orientation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MarkerOrient {
    /// `orient="auto"` — the marker's positive x-axis points in the
    /// path direction at the vertex (§13.7.4 rendering rules).
    Auto,
    /// `orient="auto-start-reverse"` — identical to `auto` except a
    /// marker placed by `marker-start` is rotated 180°, so a single
    /// arrowhead can serve both ends of a path (§13.7.1).
    AutoStartReverse,
    /// A fixed orientation in degrees (`orient="45"` or
    /// `orient="45deg"`). A bare `<number>` is degrees per the spec.
    Angle(f32),
}

impl Default for MarkerOrient {
    /// Spec initial value is `0`, i.e. a fixed zero-degree angle.
    fn default() -> Self {
        MarkerOrient::Angle(0.0)
    }
}

impl MarkerOrient {
    /// Parse the `orient` value. Recognises the two keywords plus an
    /// `<angle>` (with the CSS angle units `deg` / `grad` / `rad` /
    /// `turn`) or a bare `<number>` (interpreted as degrees per
    /// §13.7.1). Unknown / malformed values fall back to the spec
    /// default `0` (`Angle(0.0)`).
    pub fn parse(v: Option<&str>) -> Self {
        let s = match v {
            None => return MarkerOrient::default(),
            Some(s) => s.trim(),
        };
        match s {
            "auto" => MarkerOrient::Auto,
            "auto-start-reverse" => MarkerOrient::AutoStartReverse,
            _ => match parse_angle_deg(s) {
                Some(deg) => MarkerOrient::Angle(deg),
                None => MarkerOrient::default(),
            },
        }
    }

    /// Emit the canonical attribute value for round-trip serialisation.
    /// Angles emit as a bare number (degrees), matching the spec's
    /// `<number>` shorthand.
    pub fn to_attr(self) -> String {
        match self {
            MarkerOrient::Auto => "auto".to_string(),
            MarkerOrient::AutoStartReverse => "auto-start-reverse".to_string(),
            MarkerOrient::Angle(deg) => trim_orient_float(deg),
        }
    }
}

/// Compact float formatter for the `orient` round-trip serialisation.
/// Up to 6 significant decimals, trailing zeros trimmed, `-0` → `0`.
fn trim_orient_float(v: f32) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    let s = format!("{v:.6}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".into()
    } else {
        trimmed.to_string()
    }
}

/// Parse an `<angle>` to canonical degrees. Accepts the CSS angle units
/// (`deg` / `grad` / `rad` / `turn`) and a bare number (degrees).
/// Returns `None` for anything that doesn't parse as a number.
fn parse_angle_deg(s: &str) -> Option<f32> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    let (num_part, factor) = if let Some(n) = lower.strip_suffix("deg") {
        (n, 1.0)
    } else if let Some(n) = lower.strip_suffix("grad") {
        (n, 0.9) // 400 grad = 360 deg
    } else if let Some(n) = lower.strip_suffix("rad") {
        (n, 180.0 / std::f32::consts::PI)
    } else if let Some(n) = lower.strip_suffix("turn") {
        (n, 360.0)
    } else {
        (lower.as_str(), 1.0)
    };
    num_part.trim().parse::<f32>().ok().map(|v| v * factor)
}

/// Round 104 — captured `<marker id="...">` definition. SVG 2 §13.7.1.
///
/// A `<marker>` is a never-rendered container whose graphics are painted
/// at vertices of a referencing shape via the `marker-start` /
/// `marker-mid` / `marker-end` properties. `crate::model::Node` has no
/// `Marker` construct (the IR was frozen without one), so — mirroring the
/// round-20 `<pattern>` capture — round 104 records a typed `MarkerDef`
/// (consumable by a downstream rasterizer) plus the verbatim source XML
/// on [`crate::preserved::PreservedExtras::markers`] (round-trip source
/// of truth). The vertex placement + `orient` rotation + `markerUnits`
/// scaling described in §13.7.4 are the rasterizer's job once a `Marker`
/// node lands in oxideav-core.
#[derive(Clone, Debug)]
pub struct MarkerDef {
    /// `refX` — x of the marker reference point, placed exactly at the
    /// vertex. In the marker's content coordinate system (after
    /// `viewBox` / `preserveAspectRatio`). Default 0 per §13.7.1. The
    /// geometric keywords (`left` / `center` / `right`) are pre-resolved
    /// to their percentage-of-viewBox equivalents by the parser.
    pub ref_x: f32,
    /// `refY` — y of the marker reference point. Default 0.
    pub ref_y: f32,
    /// `markerWidth` — width of the marker viewport. Default 3 per
    /// §13.7.1. Zero suppresses the marker; a negative value is an
    /// error (captured as-is; the rasterizer decides).
    pub marker_width: f32,
    /// `markerHeight` — height of the marker viewport. Default 3.
    pub marker_height: f32,
    /// `markerUnits` — coordinate system for `markerWidth` /
    /// `markerHeight` and the contents. Default `strokeWidth`.
    pub marker_units: MarkerUnits,
    /// `orient` — marker rotation at the vertex. Default `0`.
    pub orient: MarkerOrient,
    /// `viewBox` mapping inside the marker viewport (§13.7.4 supplemental
    /// transform). `None` when omitted.
    pub view_box: Option<ViewBox>,
    /// `preserveAspectRatio` inside the marker viewport (default
    /// `xMidYMid meet`).
    pub preserve_aspect_ratio: PreserveAspectRatio,
    /// Parsed marker content as a `Group`. Empty group when the source
    /// `<marker>` had no renderable children.
    pub content: Group,
}

/// Round 95 — `zoomAndPan` attribute keyword per SVG 2 §16.3.3 / §16.3.2.
///
/// SVG 2 carries the SVG 1.1 attribute forward as a no-op for `disable`
/// (host UA may not zoom or pan the document) and `magnify` (host UA may
/// zoom or pan). Default is `magnify`. The decoder captures the typed
/// value on every `<view>` so a caller routing a fragment-identifier
/// view can surface the intent even though we have no zoom-and-pan host
/// UA wired up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ZoomAndPan {
    /// `zoomAndPan="disable"` — UA must not honour user-driven zoom or
    /// pan gestures.
    Disable,
    /// `zoomAndPan="magnify"` — the default; UA may honour zoom + pan.
    #[default]
    Magnify,
}

impl ZoomAndPan {
    /// Parse the `zoomAndPan` keyword. Unknown values fall back to the
    /// spec default `magnify`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "disable" => ZoomAndPan::Disable,
            // `magnify` and anything unrecognised collapse to the
            // default per the SVG 2 §16.3.3 tolerance rule.
            _ => ZoomAndPan::Magnify,
        }
    }

    /// Emit the canonical keyword for round-trip serialisation.
    pub fn as_str(self) -> &'static str {
        match self {
            ZoomAndPan::Disable => "disable",
            ZoomAndPan::Magnify => "magnify",
        }
    }
}

/// Round 95 — captured `<view id="...">` definition (SVG 2 §16.3.3 /
/// §16.3.2). The `<view>` element doesn't itself render anything — its
/// purpose is to be addressed by an SVG fragment identifier of the form
/// `MyDrawing.svg#MyView`, in which case the renderer adopts the view's
/// `viewBox` / `preserveAspectRatio` / `zoomAndPan` (overriding the
/// corresponding attributes on the root `<svg>`) for the initial view
/// into the document.
///
/// Round 95 captures the three typed attributes plus the original
/// element id so [`resolve_fragment`](crate::resolve_fragment) can wire
/// a caller-supplied fragment string to the right `<view>`. The
/// scene-graph itself is unaffected — the `<view>` lives in the
/// def-table side of the document.
#[derive(Clone, Debug, Default)]
pub struct ViewDef {
    /// `viewBox=` on the `<view>`. `None` when omitted; per §16.3.2 the
    /// caller then falls back to the root `<svg>` viewBox.
    pub view_box: Option<ViewBox>,
    /// `preserveAspectRatio=` on the `<view>`. `None` when omitted; per
    /// §16.3.2 the caller falls back to the root `<svg>` value (and
    /// from there to the spec default `xMidYMid meet`).
    pub preserve_aspect_ratio: Option<PreserveAspectRatio>,
    /// `zoomAndPan=` on the `<view>`. `None` when omitted; the caller
    /// falls back to the root `<svg>` value (default `magnify`).
    pub zoom_and_pan: Option<ZoomAndPan>,
}

/// Aggregated tables built during the pre-walk, consumed by the main
/// element parser when it resolves `url(#id)` references.
///
/// `elements` is a round-3 addition keyed by `id="..."`: any element
/// in the document that carries an id is captured verbatim so
/// `<use href="#id">` can re-instantiate it (works with rect / circle
/// / path / g / symbol / …, not just the predefined def categories).
#[derive(Clone, Debug, Default)]
pub struct DefsTables {
    pub filters: HashMap<String, FilterDef>,
    pub masks: HashMap<String, MaskDef>,
    pub clip_paths: HashMap<String, ClipPathDef>,
    pub symbols: HashMap<String, SymbolDef>,
    /// Round 20 — typed `<pattern>` paint servers (SVG 2 §14.3) keyed
    /// by `id`. Populated during the pre-walk so a forward `fill=
    /// "url(#pat)"` reference resolves regardless of source order.
    pub patterns: HashMap<String, PatternDef>,
    /// Round 81 — typed `<linearGradient>` / `<radialGradient>` paint
    /// servers (SVG 2 §14.2) keyed by `id`. Stored as un-resolved
    /// [`GradientDef`]s so [`resolve_gradient_chain`] can walk the
    /// §14.1.1 `href` template inheritance chain. The legacy
    /// `ParseContext::gradients` table is populated *after* the
    /// pre-walk by flattening every entry here through
    /// `resolve_gradient_chain` so the existing
    /// [`crate::element::resolve_paint`] code path keeps working.
    pub gradients: HashMap<String, GradientDef>,
    /// Round 95 — typed `<view>` definitions (SVG 2 §16.3.3) keyed by
    /// the view's `id`. Consumed by [`crate::resolve_fragment`] to map
    /// a `MyDrawing.svg#MyView` fragment to the correct initial-view
    /// parameters per §16.3.2.
    pub views: HashMap<String, ViewDef>,
    /// Round 104 — typed `<marker>` definitions (SVG 2 §13.7.1) keyed by
    /// `id`. Populated during the pre-walk so a forward
    /// `marker-end="url(#arrow)"` reference resolves regardless of source
    /// order. `crate::model::Node` has no `Marker` variant yet, so the
    /// scene-walk skips `<marker>` (it's a never-rendered element per
    /// §13.7.1); this typed table + the verbatim XML side-channel are the
    /// inputs a downstream rasterizer consumes once a `Marker` construct
    /// lands in oxideav-core.
    pub markers: HashMap<String, MarkerDef>,
    /// Round 3: every `id`-bearing element in the source XML, captured
    /// for `<use href="#id">` resolution. Includes shapes, groups,
    /// symbols, defs children, etc. — anything addressable by id.
    pub elements: HashMap<String, Element>,
}

impl DefsTables {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Strip a leading `url(` and trailing `)` plus the `#`, returning the
/// referenced id. Returns `None` for any string that doesn't match
/// `url(#...)`.
pub fn parse_url_ref(s: &str) -> Option<&str> {
    let s = s.trim();
    let inner = s.strip_prefix("url(")?.strip_suffix(')')?.trim();
    inner.strip_prefix('#')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GradientStop, Rgba};

    #[test]
    fn parse_url_ref_extracts_id() {
        assert_eq!(parse_url_ref("url(#foo)"), Some("foo"));
        assert_eq!(parse_url_ref("  url(#bar)  "), Some("bar"));
        assert_eq!(parse_url_ref("url( #spc )"), Some("spc"));
        assert_eq!(parse_url_ref("none"), None);
        assert_eq!(parse_url_ref("#foo"), None);
        assert_eq!(parse_url_ref("url(foo)"), None);
    }

    fn linear(href: &str, x2: Option<f32>) -> GradientDef {
        GradientDef::new(GradientKind::Linear {
            x1: None,
            y1: None,
            x2: x2.map(Length::user_units),
            y2: None,
        })
        .with_href(href)
    }

    fn radial(href: &str) -> GradientDef {
        GradientDef::new(GradientKind::radial()).with_href(href)
    }

    #[test]
    fn resolve_no_chain_uses_spec_defaults() {
        let g = linear("", None);
        let defs = DefsTables::new();
        let r = resolve_gradient_chain(&g, &defs);
        match r.kind {
            ResolvedGradientKind::Linear { x1, y1, x2, y2 } => {
                // §14.2.2.1: x1=0, y1=0, x2=1, y2=0.
                assert_eq!(x1, 0.0);
                assert_eq!(y1, 0.0);
                assert_eq!(x2, 1.0);
                assert_eq!(y2, 0.0);
            }
            _ => panic!("expected linear"),
        }
        assert_eq!(r.units, GradientUnits::ObjectBoundingBox);
        assert_eq!(r.spread, SpreadMethod::Pad);
        assert!(r.stops.is_empty());
    }

    #[test]
    fn resolve_href_inherits_unspecified_attrs() {
        let mut defs = DefsTables::new();
        // Template carries explicit x2 = 7.0 + a stop.
        let mut tmpl = linear("", Some(7.0));
        tmpl.units = Some(GradientUnits::UserSpaceOnUse);
        tmpl.stops
            .push(GradientStop::new(0.0, Rgba::opaque(1, 2, 3)));
        defs.gradients.insert("tmpl".into(), tmpl);

        // Child has no x2, no units, no stops — should inherit all
        // three via the §14.1.1 template chain.
        let child = linear("tmpl", None);
        let r = resolve_gradient_chain(&child, &defs);
        match r.kind {
            ResolvedGradientKind::Linear { x2, .. } => assert_eq!(x2, 7.0),
            _ => panic!("expected linear"),
        }
        assert_eq!(r.units, GradientUnits::UserSpaceOnUse);
        assert_eq!(r.stops.len(), 1);
        assert_eq!(r.stops[0].color, Rgba::opaque(1, 2, 3));
    }

    #[test]
    fn resolve_child_specified_attr_wins_over_template() {
        let mut defs = DefsTables::new();
        let tmpl = linear("", Some(7.0));
        defs.gradients.insert("tmpl".into(), tmpl);

        // Child explicitly sets x2 = 99 — must NOT be overwritten by
        // the template's x2 = 7.
        let child = linear("tmpl", Some(99.0));
        let r = resolve_gradient_chain(&child, &defs);
        match r.kind {
            ResolvedGradientKind::Linear { x2, .. } => assert_eq!(x2, 99.0),
            _ => panic!("expected linear"),
        }
    }

    #[test]
    fn resolve_cycle_terminates() {
        // a → b → a is a cycle; the walker must stop without diverging.
        let mut defs = DefsTables::new();
        let a = linear("b", None);
        let b = linear("a", None);
        defs.gradients.insert("a".into(), a.clone());
        defs.gradients.insert("b".into(), b);
        let r = resolve_gradient_chain(&a, &defs);
        // Cycle terminated; spec defaults populated.
        match r.kind {
            ResolvedGradientKind::Linear { x2, .. } => assert_eq!(x2, 1.0),
            _ => panic!("expected linear"),
        }
    }

    #[test]
    fn resolve_radial_uses_radial_defaults() {
        let g = radial("");
        let defs = DefsTables::new();
        let r = resolve_gradient_chain(&g, &defs);
        match r.kind {
            ResolvedGradientKind::Radial {
                cx,
                cy,
                r,
                fx,
                fy,
                fr,
            } => {
                // §14.2.3.1: cx=cy=0.5, r=0.5, fx=cx, fy=cy, fr=0.
                assert_eq!(cx, 0.5);
                assert_eq!(cy, 0.5);
                assert_eq!(r, 0.5);
                assert_eq!(fx, 0.5);
                assert_eq!(fy, 0.5);
                assert_eq!(fr, 0.0);
            }
            _ => panic!("expected radial"),
        }
    }

    #[test]
    fn marker_units_default_and_parse() {
        assert_eq!(MarkerUnits::default(), MarkerUnits::StrokeWidth);
        assert_eq!(
            MarkerUnits::parse(Some("userSpaceOnUse")),
            MarkerUnits::UserSpaceOnUse
        );
        assert_eq!(MarkerUnits::parse(None), MarkerUnits::StrokeWidth);
        assert_eq!(MarkerUnits::parse(Some(" bad ")), MarkerUnits::StrokeWidth);
    }

    #[test]
    fn marker_orient_default_is_zero_angle() {
        assert_eq!(MarkerOrient::default(), MarkerOrient::Angle(0.0));
    }

    #[test]
    fn marker_orient_parses_keywords_and_angle_units() {
        assert_eq!(MarkerOrient::parse(Some("auto")), MarkerOrient::Auto);
        assert_eq!(
            MarkerOrient::parse(Some("auto-start-reverse")),
            MarkerOrient::AutoStartReverse
        );
        assert_eq!(MarkerOrient::parse(Some("30")), MarkerOrient::Angle(30.0));
        match MarkerOrient::parse(Some("1turn")) {
            MarkerOrient::Angle(d) => assert!((d - 360.0).abs() < 1e-3),
            other => panic!("expected Angle(360), got {other:?}"),
        }
        // Malformed → spec default 0.
        assert_eq!(MarkerOrient::parse(Some("xx")), MarkerOrient::Angle(0.0));
    }

    #[test]
    fn marker_orient_round_trip_attr() {
        assert_eq!(MarkerOrient::Auto.to_attr(), "auto");
        assert_eq!(MarkerOrient::Angle(0.0).to_attr(), "0");
        assert_eq!(MarkerOrient::Angle(-45.0).to_attr(), "-45");
    }

    #[test]
    fn resolve_radial_inherits_from_radial_template_but_keeps_kind() {
        let mut defs = DefsTables::new();
        let mut tmpl = radial("");
        tmpl.kind = GradientKind::Radial {
            cx: Some(Length::user_units(2.0)),
            cy: Some(Length::user_units(3.0)),
            r: Some(Length::user_units(5.0)),
            fx: None,
            fy: None,
            fr: Some(Length::user_units(0.25)),
        };
        defs.gradients.insert("tmpl".into(), tmpl);

        let child = radial("tmpl");
        let r = resolve_gradient_chain(&child, &defs);
        match r.kind {
            ResolvedGradientKind::Radial {
                cx, cy, r: rr, fr, ..
            } => {
                assert_eq!(cx, 2.0);
                assert_eq!(cy, 3.0);
                assert_eq!(rr, 5.0);
                assert_eq!(fr, 0.25);
            }
            _ => panic!("expected radial"),
        }
    }
}
