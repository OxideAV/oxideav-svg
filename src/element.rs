//! Element-specific parsers — turn an [`Element`] into an
//! [`oxideav_core::Node`].
//!
//! Each shape parser builds a `PathNode` (rectangles / circles /
//! ellipses / lines / polylines / polygons all reduce to a `Path`).
//! `<g>` is mapped to a [`Group`] node that recurses into its children.
//! `<linearGradient>` / `<radialGradient>` are collected into a
//! `gradient table` keyed by `id`, then resolved when a paint
//! attribute references one via `url(#id)`.

use std::collections::{HashMap, HashSet};

use oxideav_core::{
    DashPattern, Error, FillRule, GradientStop, Group, LineCap, LineJoin, LinearGradient, MaskKind,
    Node, Paint, Path, PathCommand, PathNode, Point, RadialGradient, Result, Rgba, SpreadMethod,
    Stroke, Transform2D,
};

use crate::color::{parse_opacity, parse_paint, PaintValue};
use crate::css::{declarations_for, MatchContext, Stylesheet};
use crate::defs::{
    parse_url_ref, resolve_gradient_chain, ClipPathDef, DefsTables, FilterDef, GradientDef,
    GradientKind, GradientUnits, MaskDef, ResolvedGradient, ResolvedGradientKind, SymbolDef,
};
use crate::length::{parse_length, LengthAxis, ResolveContext};
use crate::parser::{attr, tag_local, Element, Node as XmlNode};
use crate::path_data::parse_path_data;
use crate::preserved::IdScenePath;
use crate::transform::parse_transform;

/// Inherited paint / stroke / opacity / fill-rule state. Round 1
/// keeps this minimal — full CSS inheritance lives in round 2 (text /
/// `<style>`).
#[derive(Clone, Debug)]
pub struct PaintState {
    pub fill: PaintValue,
    pub fill_opacity: f32,
    pub stroke: PaintValue,
    pub stroke_opacity: f32,
    pub stroke_width: f32,
    pub stroke_linecap: LineCap,
    pub stroke_linejoin: LineJoin,
    pub stroke_miterlimit: f32,
    pub stroke_dasharray: Option<Vec<f32>>,
    pub stroke_dashoffset: f32,
    pub opacity: f32,
    pub fill_rule: FillRule,
}

impl Default for PaintState {
    fn default() -> Self {
        Self {
            // Per SVG 1.1 §11.3 — default fill is opaque black, no
            // stroke.
            fill: PaintValue::Color(Rgba::opaque(0, 0, 0)),
            fill_opacity: 1.0,
            stroke: PaintValue::None,
            stroke_opacity: 1.0,
            stroke_width: 1.0,
            stroke_linecap: LineCap::Butt,
            stroke_linejoin: LineJoin::Miter,
            stroke_miterlimit: 4.0,
            stroke_dasharray: None,
            stroke_dashoffset: 0.0,
            opacity: 1.0,
            fill_rule: FillRule::NonZero,
        }
    }
}

impl PaintState {
    /// Override fields from `el`'s presentation attributes. Anything
    /// the element doesn't set inherits unchanged from `self`.
    pub fn merged_with(&self, el: &Element) -> Result<Self> {
        // No CSS context — use plain attrs only.
        self.merged_with_css(el, &Stylesheet::new())
    }

    /// Round 4 — variant that also consults a CSS [`Stylesheet`] for
    /// matched-by-specificity declarations + the inline `style="..."`
    /// attribute. Cascade order (last wins): inherited → presentation
    /// attrs → matched CSS rules → `style="..."`.
    ///
    /// This wrapper builds an isolated [`MatchContext`] (no parent /
    /// sibling info) for `el` — which matches the round-4 selector
    /// surface. Round-5 callers that need combinator / structural
    /// pseudo-class support should call [`Self::merged_with_mctx`]
    /// directly with a chained context built during the tree walk.
    pub fn merged_with_css(&self, el: &Element, sheet: &Stylesheet) -> Result<Self> {
        let mctx = MatchContext::root(el);
        self.merged_with_mctx(&mctx, sheet)
    }

    /// Round 5 — same as [`Self::merged_with_css`] but takes a
    /// fully-chained [`MatchContext`] so combinator selectors
    /// (`a > b`, `a + b`, `a ~ b`, descendant) and structural pseudo-
    /// classes (`:nth-child`, `:first-of-type`, …) can match.
    pub fn merged_with_mctx(&self, mctx: &MatchContext<'_>, sheet: &Stylesheet) -> Result<Self> {
        let mut s = self.clone();
        let el = mctx.el;
        // 1) presentation attributes from `el`.
        for (name, _) in &el.attrs {
            self.apply_one(&mut s, name, attr(el, name).unwrap_or(""))?;
        }
        // 2) matched CSS rules + inline style — last write wins.
        for (name, value) in declarations_for(mctx, sheet) {
            self.apply_one(&mut s, &name, &value)?;
        }
        Ok(s)
    }

    /// Apply one (name, value) presentation property to `s`. Unknown
    /// names are ignored (so `style=""` content like `font-family` doesn't
    /// blow up — it's just silently filed under "not modelled yet").
    fn apply_one(&self, s: &mut PaintState, name: &str, value: &str) -> Result<()> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "fill" => s.fill = parse_paint(value)?,
            "fill-opacity" => s.fill_opacity = parse_opacity(value)?,
            "stroke" => s.stroke = parse_paint(value)?,
            "stroke-opacity" => s.stroke_opacity = parse_opacity(value)?,
            "stroke-width" => {
                s.stroke_width = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| Error::invalid("SVG: malformed stroke-width"))?;
            }
            "stroke-linecap" => {
                s.stroke_linecap = match value.trim() {
                    "butt" => LineCap::Butt,
                    "round" => LineCap::Round,
                    "square" => LineCap::Square,
                    _ => return Err(Error::invalid("SVG: bad stroke-linecap")),
                };
            }
            "stroke-linejoin" => {
                s.stroke_linejoin = match value.trim() {
                    "miter" => LineJoin::Miter,
                    "round" => LineJoin::Round,
                    "bevel" => LineJoin::Bevel,
                    _ => return Err(Error::invalid("SVG: bad stroke-linejoin")),
                };
            }
            "stroke-miterlimit" => {
                s.stroke_miterlimit = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| Error::invalid("SVG: malformed stroke-miterlimit"))?;
            }
            "stroke-dasharray" => {
                let trimmed = value.trim();
                if trimmed.eq_ignore_ascii_case("none") {
                    s.stroke_dasharray = None;
                } else {
                    let arr: Result<Vec<f32>> = trimmed
                        .split(|c: char| c == ',' || c.is_whitespace())
                        .filter(|p| !p.is_empty())
                        .map(|n| {
                            n.parse::<f32>()
                                .map_err(|_| Error::invalid("SVG: malformed stroke-dasharray"))
                        })
                        .collect();
                    s.stroke_dasharray = Some(arr?);
                }
            }
            "stroke-dashoffset" => {
                s.stroke_dashoffset = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| Error::invalid("SVG: malformed stroke-dashoffset"))?;
            }
            "opacity" => s.opacity = parse_opacity(value)?,
            "fill-rule" => {
                s.fill_rule = match value.trim() {
                    "nonzero" => FillRule::NonZero,
                    "evenodd" => FillRule::EvenOdd,
                    _ => return Err(Error::invalid("SVG: bad fill-rule")),
                };
            }
            // Round-4 CSS may carry properties we don't yet model
            // (font-family, transform, …). Ignore them rather than
            // failing the document.
            _ => {}
        }
        Ok(())
    }

    fn solid_fill(&self, gradients: &GradientTable, defs: &DefsTables) -> Option<Paint> {
        resolve_paint(&self.fill, self.fill_opacity, gradients, defs)
    }

    fn solid_stroke(&self, gradients: &GradientTable, defs: &DefsTables) -> Option<Stroke> {
        let paint = resolve_paint(&self.stroke, self.stroke_opacity, gradients, defs)?;
        Some(Stroke {
            width: self.stroke_width,
            paint,
            cap: self.stroke_linecap,
            join: self.stroke_linejoin,
            miter_limit: self.stroke_miterlimit,
            dash: self.stroke_dasharray.as_ref().map(|arr| DashPattern {
                array: arr.clone(),
                offset: self.stroke_dashoffset,
            }),
        })
    }
}

fn apply_alpha(color: Rgba, alpha: f32) -> Rgba {
    let a = ((color.a as f32 / 255.0) * alpha.clamp(0.0, 1.0)) * 255.0;
    Rgba::new(color.r, color.g, color.b, a.round() as u8)
}

/// Resolve a parsed [`PaintValue`] against the gradient + pattern
/// tables. Returns `None` for `none` (no paint), an explicit `none`
/// fallback (round 20), or an unresolvable reference with no fallback.
///
/// Round 20 — the SVG 2 §13.2 paint-list (`url(#id) [none | <color>]`)
/// resolves through the following precedence:
///   1. Gradient table (round 1+ — typed [`Paint`] cloned into place).
///   2. Pattern table (round 20 — typed [`crate::defs::PatternDef`]).
///      The renderer doesn't yet have a `Paint::Pattern` constructor
///      so we treat a successful pattern lookup as "no fill" UNTIL
///      the fallback colour applies. In other words, a pattern with
///      a fallback colour renders as the fallback today; without a
///      fallback, no paint is applied. Once `oxideav_core::Paint`
///      gains a `Pattern` variant, the pattern branch will return the
///      tiled paint directly and the fallback path becomes a true
///      error case again.
///   3. Fallback (when present in the source paint-list).
fn resolve_paint(
    value: &PaintValue,
    opacity: f32,
    gradients: &GradientTable,
    defs: &DefsTables,
) -> Option<Paint> {
    match value {
        PaintValue::None => None,
        PaintValue::Color(c) => Some(Paint::Solid(apply_alpha(*c, opacity))),
        PaintValue::Reference { id, fallback } => {
            if let Some(p) = gradients.get(id) {
                return Some(p.clone());
            }
            if defs.patterns.contains_key(id) {
                // Pattern paint server known but `oxideav_core::Paint`
                // has no `Pattern` variant — fall through to the
                // fallback so the visual isn't silently dropped. Per
                // SVG 2 §13.2: the fallback applies "if the paint
                // server reference cannot be resolved." Strict
                // interpretation treats a successful resolution as
                // "render the pattern," but for a scene graph that
                // can't carry one, the fallback is the spec-friendly
                // proxy.
                return resolve_paint_fallback(fallback, opacity);
            }
            // Unknown id — apply fallback if any, otherwise no paint
            // (matches the pre-round-20 behaviour for a bare
            // `url(#missing)`).
            resolve_paint_fallback(fallback, opacity)
        }
    }
}

/// Apply the optional `[none | <color>]` fallback half of an SVG 2
/// paint-list. Returns `None` for explicit `none`, the alpha-scaled
/// colour for `<color>`, and `None` for no fallback at all.
fn resolve_paint_fallback(fallback: &Option<Option<Rgba>>, opacity: f32) -> Option<Paint> {
    match fallback {
        Some(Some(c)) => Some(Paint::Solid(apply_alpha(*c, opacity))),
        Some(None) => None,
        None => None,
    }
}

/// Look-up table of `id` → resolved [`Paint`] (gradient). Built up by
/// the decoder while it walks `<defs>` / inline gradient elements.
pub type GradientTable = HashMap<String, Paint>;

/// Mutable parse-time context: gradient table (resolved on the fly)
/// and the pre-walked round-2 defs tables (filter / mask / clipPath /
/// symbol). Threaded through every `parse_element_to_node` call so
/// nested elements can resolve `url(#id)` references in any of them.
///
/// `use_stack` (round 3) holds the chain of currently-instantiating
/// `<use>` ids. A `<use href="#x">` whose id is already on the stack
/// would create an infinite loop (`use → symbol → use of same id`)
/// and is dropped instead.
#[derive(Debug)]
pub struct ParseContext {
    pub gradients: GradientTable,
    pub defs: DefsTables,
    pub use_stack: HashSet<String>,
    /// Round 4: CSS rules collected from every `<style>` block in the
    /// document. Threaded through `merged_with_css` so each element
    /// pulls matched declarations during property resolution.
    pub stylesheet: Stylesheet,
    /// Round 4: time (in seconds) at which to evaluate `<animate>` /
    /// `<set>` / `<animateTransform>`. `0.0` reproduces the round-3
    /// first-paint snapshot. Set via [`ParseContext::with_time`].
    pub animation_t: f32,
    /// Round 13 — current scene-graph child-index path during the
    /// build, used by [`ParseContext::record_id_path`] to map source
    /// `id="..."` attributes to scene-graph emit sites. Pushed before
    /// each child build, popped after. Empty by default and only
    /// populated by [`crate::decoder::parse_svg_with_extras`].
    pub current_path: Vec<usize>,
    /// Round 13 — collected `(scene_path, source_id)` mappings.
    /// Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`]; the `parse_svg` /
    /// `parse_svg_at` paths leave the recorder untouched so the
    /// hot-path doesn't pay the (tiny) bookkeeping cost.
    pub id_paths: Vec<IdScenePath>,
    /// Round 13 — gate; when `false`, [`ParseContext::record_id_path`]
    /// is a no-op. Avoids spending allocations + clones on the
    /// non-extras parse paths that don't need the mapping.
    pub track_id_paths: bool,
    /// Round 19 — current CSS Values L4 / SVG 2 §10 length-resolution
    /// context. Carries the bracketing viewport width/height (for
    /// `vw` / `vh` / `vmin` / `vmax`) and the per-element font-size
    /// cascade (for `em` / `rem`). Updated as the tree walk descends:
    /// any element that sets `font-size` (via attr or CSS) pushes a
    /// new context for its descendants. Bare-numeric coordinate
    /// values (`<rect x="100">`) round-trip bit-for-bit identical to
    /// the legacy [`parse_number`] path because [`crate::length::Length::resolve`]
    /// is the identity for [`crate::length::LengthUnit::UserUnit`].
    pub resolve_ctx: ResolveContext,
}

impl Default for ParseContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ParseContext {
    pub fn new() -> Self {
        Self {
            gradients: GradientTable::new(),
            defs: DefsTables::new(),
            use_stack: HashSet::new(),
            stylesheet: Stylesheet::new(),
            animation_t: 0.0,
            current_path: Vec::new(),
            id_paths: Vec::new(),
            track_id_paths: false,
            resolve_ctx: ResolveContext::default(),
        }
    }

    /// Round 19 — bind the root [`ResolveContext`]. Used by
    /// [`crate::decoder::parse_svg_at`] to seed the viewport
    /// dimensions / root font-size before the tree walk starts.
    pub fn with_resolve_ctx(mut self, ctx: ResolveContext) -> Self {
        self.resolve_ctx = ctx;
        self
    }

    /// Set the animation evaluation time (in seconds). Builder-style.
    pub fn with_time(mut self, t_seconds: f32) -> Self {
        self.animation_t = t_seconds;
        self
    }

    /// Round 13 — opt in to scene-graph id-path tracking. Must be set
    /// before the build walk begins; otherwise [`Self::id_paths`] is
    /// left empty.
    pub fn enable_id_path_tracking(&mut self) {
        self.track_id_paths = true;
    }

    /// Round 13 — record the current scene-graph path against `id`.
    /// No-op when [`Self::track_id_paths`] is `false`.
    pub fn record_id_path(&mut self, id: &str) {
        if !self.track_id_paths || id.is_empty() {
            return;
        }
        self.id_paths.push(IdScenePath {
            id: id.to_string(),
            path: self.current_path.clone(),
        });
    }
}

/// Parse `<linearGradient id="...">` into a [`Paint`] entry. Returns
/// `Some((id, paint))` on success, `None` if the element lacks an `id`
/// (in which case it can't be referenced).
///
/// **Round 81** — this is the *legacy* path that ignores `href`
/// template inheritance and `gradientUnits` / `gradientTransform`.
/// The decoder pre-walk now builds typed [`GradientDef`]s via
/// [`parse_linear_gradient_def`] and flattens them through
/// [`crate::defs::resolve_gradient_chain`]; this entry point survives
/// for the round-1 unit-tests and for downstream callers that need a
/// direct `Element → Paint` conversion without going through the def
/// table.
pub fn parse_linear_gradient(el: &Element) -> Result<Option<(String, Paint)>> {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    let x1 = parse_coord(attr(el, "x1"), 0.0)?;
    let y1 = parse_coord(attr(el, "y1"), 0.0)?;
    let x2 = parse_coord(attr(el, "x2"), 1.0)?;
    let y2 = parse_coord(attr(el, "y2"), 0.0)?;
    let spread = parse_spread_method(attr(el, "spreadMethod"))?;
    let stops = collect_stops(el)?;
    Ok(Some((
        id,
        Paint::LinearGradient(LinearGradient {
            start: Point::new(x1, y1),
            end: Point::new(x2, y2),
            stops,
            spread,
        }),
    )))
}

/// Parse `<radialGradient id="...">` into a [`Paint`] entry.
///
/// See the [`parse_linear_gradient`] doc-comment regarding the
/// round-81 split with [`parse_radial_gradient_def`].
pub fn parse_radial_gradient(el: &Element) -> Result<Option<(String, Paint)>> {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    let cx = parse_coord(attr(el, "cx"), 0.5)?;
    let cy = parse_coord(attr(el, "cy"), 0.5)?;
    let r = parse_coord(attr(el, "r"), 0.5)?;
    let fx = attr(el, "fx")
        .map(|v| parse_coord(Some(v), cx))
        .transpose()?;
    let fy = attr(el, "fy")
        .map(|v| parse_coord(Some(v), cy))
        .transpose()?;
    let focal = match (fx, fy) {
        (Some(x), Some(y)) => Some(Point::new(x, y)),
        (Some(x), None) => Some(Point::new(x, cy)),
        (None, Some(y)) => Some(Point::new(cx, y)),
        (None, None) => None,
    };
    let spread = parse_spread_method(attr(el, "spreadMethod"))?;
    let stops = collect_stops(el)?;
    Ok(Some((
        id,
        Paint::RadialGradient(RadialGradient {
            center: Point::new(cx, cy),
            radius: r,
            focal,
            stops,
            spread,
        }),
    )))
}

/// Round 81 — parse `<linearGradient>` into a typed [`GradientDef`]
/// preserving per-attribute "specified vs absent" so
/// [`crate::defs::resolve_gradient_chain`] can apply SVG 2 §14.1.1
/// template inheritance.
///
/// Returns `None` when the element lacks an `id` (no `url(#id)`
/// reference can target it).
pub fn parse_linear_gradient_def(el: &Element) -> Result<Option<(String, GradientDef)>> {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    let kind = GradientKind::Linear {
        x1: parse_opt_coord(attr(el, "x1"))?,
        y1: parse_opt_coord(attr(el, "y1"))?,
        x2: parse_opt_coord(attr(el, "x2"))?,
        y2: parse_opt_coord(attr(el, "y2"))?,
    };
    Ok(Some((id, gradient_def_common(el, kind)?)))
}

/// Round 81 — parse `<radialGradient>` into a typed [`GradientDef`]
/// preserving per-attribute "specified vs absent." See
/// [`parse_linear_gradient_def`].
pub fn parse_radial_gradient_def(el: &Element) -> Result<Option<(String, GradientDef)>> {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    let kind = GradientKind::Radial {
        cx: parse_opt_coord(attr(el, "cx"))?,
        cy: parse_opt_coord(attr(el, "cy"))?,
        r: parse_opt_coord(attr(el, "r"))?,
        fx: parse_opt_coord(attr(el, "fx"))?,
        fy: parse_opt_coord(attr(el, "fy"))?,
        fr: parse_opt_coord(attr(el, "fr"))?,
    };
    Ok(Some((id, gradient_def_common(el, kind)?)))
}

/// Shared (units / transform / spread / stops / href) tail parser used
/// by both [`parse_linear_gradient_def`] and
/// [`parse_radial_gradient_def`]. Per SVG 2 §14.2.2.1 / §14.2.3.1 +
/// §14.1.1 every field stays `Option<_>` so the template walker can
/// distinguish "not specified" (inherit from template) from
/// "specified-with-default" (the child's explicit choice wins).
fn gradient_def_common(el: &Element, kind: GradientKind) -> Result<GradientDef> {
    let units = match attr(el, "gradientUnits") {
        Some(s) => parse_gradient_units_opt(s),
        None => None,
    };
    let transform = match attr(el, "gradientTransform") {
        Some(s) => Some(parse_transform(s)?),
        None => None,
    };
    let spread = match attr(el, "spreadMethod") {
        Some(s) => Some(parse_spread_method_value(s)?),
        None => None,
    };
    let stops = collect_stops(el)?;
    let href = attr(el, "href")
        .or_else(|| attr(el, "xlink:href"))
        .map(|s| s.trim().trim_start_matches('#').to_string())
        .unwrap_or_default();
    Ok(GradientDef {
        kind,
        units,
        transform,
        spread,
        stops,
        href,
    })
}

/// Round 81 — flatten a typed [`GradientDef`] (via the template chain)
/// into a legacy [`Paint`] so [`resolve_paint`] keeps working without a
/// renderer-side rewrite. `gradient_transform` is folded into the
/// gradient's geometry (start / end points or center / focal) by
/// applying the transform to the original coords. For
/// `gradientUnits="objectBoundingBox"` (the default) the bare 0..1
/// coords are kept literal — the existing round-1 paint resolver
/// already documented that gradient coordinates are unitless box
/// fractions — but the units field stays preserved in
/// [`crate::defs::DefsTables`] for a renderer that needs it.
pub fn flatten_gradient_to_paint(def: &GradientDef, defs: &DefsTables) -> Paint {
    let resolved = resolve_gradient_chain(def, defs);
    resolved_to_paint(&resolved)
}

fn resolved_to_paint(r: &ResolvedGradient) -> Paint {
    let tx = &r.transform;
    match r.kind {
        ResolvedGradientKind::Linear { x1, y1, x2, y2 } => {
            let start = apply_xform(tx, x1, y1);
            let end = apply_xform(tx, x2, y2);
            Paint::LinearGradient(LinearGradient {
                start,
                end,
                stops: r.stops.clone(),
                spread: r.spread,
            })
        }
        ResolvedGradientKind::Radial {
            cx,
            cy,
            r: radius,
            fx,
            fy,
            ..
        } => {
            let center = apply_xform(tx, cx, cy);
            let focal_pt = apply_xform(tx, fx, fy);
            let focal_opt = if (fx - cx).abs() < 1e-6 && (fy - cy).abs() < 1e-6 {
                None
            } else {
                Some(focal_pt)
            };
            // Approximate transformed radius via the geometric mean of
            // the transform's per-axis scale (the spec semantics for a
            // non-uniformly-scaled radial gradient need a renderer
            // that tracks the full 2x2 matrix; this preserves area for
            // a uniform scale + falls back to a sensible default for
            // shear / non-uniform — the typed `ResolvedGradient` on
            // [`DefsTables::gradients`] still has the full geometry
            // for a renderer that wants exact behaviour).
            let sx = (tx.a * tx.a + tx.b * tx.b).sqrt();
            let sy = (tx.c * tx.c + tx.d * tx.d).sqrt();
            let scaled = radius * (sx * sy).sqrt();
            Paint::RadialGradient(RadialGradient {
                center,
                radius: scaled,
                focal: focal_opt,
                stops: r.stops.clone(),
                spread: r.spread,
            })
        }
    }
}

#[inline]
fn apply_xform(t: &Transform2D, x: f32, y: f32) -> Point {
    // (x', y') = (a*x + c*y + e, b*x + d*y + f) per Transform2D's
    // column-major SVG/PDF matrix layout.
    Point::new(t.a * x + t.c * y + t.e, t.b * x + t.d * y + t.f)
}

fn parse_opt_coord(v: Option<&str>) -> Result<Option<f32>> {
    match v {
        None => Ok(None),
        Some(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            // Strip `%` (matches the legacy `parse_coord` lenience).
            let bare = trimmed.trim_end_matches('%');
            bare.parse::<f32>()
                .map(Some)
                .map_err(|_| Error::invalid("SVG gradient: malformed coordinate"))
        }
    }
}

fn parse_gradient_units_opt(s: &str) -> Option<GradientUnits> {
    match s.trim() {
        "userSpaceOnUse" => Some(GradientUnits::UserSpaceOnUse),
        "objectBoundingBox" => Some(GradientUnits::ObjectBoundingBox),
        // Unknown → leave as None so the template-chain walker can fall
        // back to the spec default rather than baking the unknown
        // keyword in.
        _ => None,
    }
}

fn parse_spread_method_value(s: &str) -> Result<SpreadMethod> {
    Ok(match s.trim() {
        "pad" => SpreadMethod::Pad,
        "reflect" => SpreadMethod::Reflect,
        "repeat" => SpreadMethod::Repeat,
        _ => return Err(Error::invalid("SVG gradient: bad spreadMethod")),
    })
}

fn parse_coord(v: Option<&str>, default: f32) -> Result<f32> {
    match v {
        None => Ok(default),
        Some(s) => {
            // Strip optional `%` — round 1 ignores user-units and just
            // takes the numeric value (gradients in SVG default to
            // objectBoundingBox where 0..1 spans the bounding box).
            let trimmed = s.trim().trim_end_matches('%');
            trimmed
                .parse::<f32>()
                .map_err(|_| Error::invalid("SVG gradient: malformed coordinate"))
        }
    }
}

fn parse_spread_method(v: Option<&str>) -> Result<SpreadMethod> {
    Ok(match v.map(str::trim) {
        None | Some("pad") => SpreadMethod::Pad,
        Some("reflect") => SpreadMethod::Reflect,
        Some("repeat") => SpreadMethod::Repeat,
        Some(_) => return Err(Error::invalid("SVG gradient: bad spreadMethod")),
    })
}

fn collect_stops(parent: &Element) -> Result<Vec<GradientStop>> {
    let mut stops = Vec::new();
    for child in &parent.children {
        if let XmlNode::Element(el) = child {
            if tag_local(&el.name) == "stop" {
                let offset = attr(el, "offset")
                    .map(|v| {
                        let trimmed = v.trim().trim_end_matches('%');
                        let raw = trimmed
                            .parse::<f32>()
                            .map_err(|_| Error::invalid("SVG gradient stop: malformed offset"))?;
                        Ok::<f32, Error>(if v.contains('%') { raw / 100.0 } else { raw })
                    })
                    .transpose()?
                    .unwrap_or(0.0);
                let color = match attr(el, "stop-color") {
                    Some(v) => match parse_paint(v)? {
                        PaintValue::Color(c) => c,
                        _ => Rgba::opaque(0, 0, 0),
                    },
                    None => Rgba::opaque(0, 0, 0),
                };
                let opacity = attr(el, "stop-opacity")
                    .map(parse_opacity)
                    .transpose()?
                    .unwrap_or(1.0);
                stops.push(GradientStop::new(offset, apply_alpha(color, opacity)));
            }
        }
    }
    Ok(stops)
}

/// Parse a `<rect>`. Returns `None` for degenerate (zero-size) rects.
///
/// Round 19 — coordinate values resolve via the supplied
/// [`ResolveContext`]. Bare-numeric inputs round-trip bit-for-bit
/// identical to the round-1 [`parse_number`] path; unit-suffixed
/// inputs (`<rect x="1em" width="50%">`) resolve per CSS Values L4 §6
/// against the per-axis basis (X for `x` / `width` / `rx`, Y for `y`
/// / `height` / `ry`).
pub fn parse_rect(el: &Element, ctx: &ResolveContext) -> Result<Option<Path>> {
    let x = parse_length_attr(attr(el, "x"), 0.0, LengthAxis::X, ctx)?;
    let y = parse_length_attr(attr(el, "y"), 0.0, LengthAxis::Y, ctx)?;
    let w = parse_length_attr(attr(el, "width"), 0.0, LengthAxis::X, ctx)?;
    let h = parse_length_attr(attr(el, "height"), 0.0, LengthAxis::Y, ctx)?;
    if w <= 0.0 || h <= 0.0 {
        return Ok(None);
    }
    let rx_attr = attr(el, "rx")
        .map(|v| parse_length_attr(Some(v), 0.0, LengthAxis::X, ctx))
        .transpose()?;
    let ry_attr = attr(el, "ry")
        .map(|v| parse_length_attr(Some(v), 0.0, LengthAxis::Y, ctx))
        .transpose()?;
    // Per §9.2: if only one of rx/ry is given, the other defaults to it.
    let (rx, ry) = match (rx_attr, ry_attr) {
        (Some(rx), Some(ry)) => (rx, ry),
        (Some(rx), None) => (rx, rx),
        (None, Some(ry)) => (ry, ry),
        (None, None) => (0.0, 0.0),
    };
    let rx = rx.min(w * 0.5).max(0.0);
    let ry = ry.min(h * 0.5).max(0.0);

    let mut path = Path::new();
    if rx == 0.0 && ry == 0.0 {
        path.move_to(Point::new(x, y));
        path.line_to(Point::new(x + w, y));
        path.line_to(Point::new(x + w, y + h));
        path.line_to(Point::new(x, y + h));
        path.close();
    } else {
        // Rounded rect — emit the §9.2 path: 4 lines + 4 elliptic arcs.
        path.move_to(Point::new(x + rx, y));
        path.line_to(Point::new(x + w - rx, y));
        path.commands.push(PathCommand::ArcTo {
            rx,
            ry,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(x + w, y + ry),
        });
        path.line_to(Point::new(x + w, y + h - ry));
        path.commands.push(PathCommand::ArcTo {
            rx,
            ry,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(x + w - rx, y + h),
        });
        path.line_to(Point::new(x + rx, y + h));
        path.commands.push(PathCommand::ArcTo {
            rx,
            ry,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(x, y + h - ry),
        });
        path.line_to(Point::new(x, y + ry));
        path.commands.push(PathCommand::ArcTo {
            rx,
            ry,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(x + rx, y),
        });
        path.close();
    }
    Ok(Some(path))
}

/// Parse a `<circle>`. Returns `None` for r ≤ 0.
///
/// Round 19 — `r` is an SVG 2 §10 "diagonal" length-percentage (it
/// resolves against `sqrt(w² + h²) / sqrt(2)` per §7.10).
pub fn parse_circle(el: &Element, ctx: &ResolveContext) -> Result<Option<Path>> {
    let cx = parse_length_attr(attr(el, "cx"), 0.0, LengthAxis::X, ctx)?;
    let cy = parse_length_attr(attr(el, "cy"), 0.0, LengthAxis::Y, ctx)?;
    let r = parse_length_attr(attr(el, "r"), 0.0, LengthAxis::Diagonal, ctx)?;
    if r <= 0.0 {
        return Ok(None);
    }
    Ok(Some(ellipse_path(cx, cy, r, r)))
}

/// Parse an `<ellipse>`. Returns `None` for rx ≤ 0 or ry ≤ 0.
pub fn parse_ellipse(el: &Element, ctx: &ResolveContext) -> Result<Option<Path>> {
    let cx = parse_length_attr(attr(el, "cx"), 0.0, LengthAxis::X, ctx)?;
    let cy = parse_length_attr(attr(el, "cy"), 0.0, LengthAxis::Y, ctx)?;
    let rx = parse_length_attr(attr(el, "rx"), 0.0, LengthAxis::X, ctx)?;
    let ry = parse_length_attr(attr(el, "ry"), 0.0, LengthAxis::Y, ctx)?;
    if rx <= 0.0 || ry <= 0.0 {
        return Ok(None);
    }
    Ok(Some(ellipse_path(cx, cy, rx, ry)))
}

fn ellipse_path(cx: f32, cy: f32, rx: f32, ry: f32) -> Path {
    // Two half-arcs starting at the rightmost point; round-trips
    // cleanly through any rasterizer that flattens `ArcTo`.
    let mut path = Path::new();
    path.move_to(Point::new(cx + rx, cy));
    path.commands.push(PathCommand::ArcTo {
        rx,
        ry,
        x_axis_rot: 0.0,
        large_arc: false,
        sweep: true,
        end: Point::new(cx - rx, cy),
    });
    path.commands.push(PathCommand::ArcTo {
        rx,
        ry,
        x_axis_rot: 0.0,
        large_arc: false,
        sweep: true,
        end: Point::new(cx + rx, cy),
    });
    path.close();
    path
}

/// Parse a `<line>`.
pub fn parse_line(el: &Element, ctx: &ResolveContext) -> Result<Path> {
    let x1 = parse_length_attr(attr(el, "x1"), 0.0, LengthAxis::X, ctx)?;
    let y1 = parse_length_attr(attr(el, "y1"), 0.0, LengthAxis::Y, ctx)?;
    let x2 = parse_length_attr(attr(el, "x2"), 0.0, LengthAxis::X, ctx)?;
    let y2 = parse_length_attr(attr(el, "y2"), 0.0, LengthAxis::Y, ctx)?;
    let mut path = Path::new();
    path.move_to(Point::new(x1, y1));
    path.line_to(Point::new(x2, y2));
    Ok(path)
}

/// Parse `<polyline>` / `<polygon>`. `closed=true` appends a `Close`.
pub fn parse_polyline(el: &Element, closed: bool) -> Result<Option<Path>> {
    let points = match attr(el, "points") {
        Some(v) => parse_points(v)?,
        None => return Ok(None),
    };
    if points.is_empty() {
        return Ok(None);
    }
    let mut path = Path::new();
    path.move_to(points[0]);
    for p in points.iter().skip(1) {
        path.line_to(*p);
    }
    if closed {
        path.close();
    }
    Ok(Some(path))
}

fn parse_points(s: &str) -> Result<Vec<Point>> {
    let nums: Result<Vec<f32>> = s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(|n| {
            n.parse::<f32>()
                .map_err(|_| Error::invalid("SVG: malformed number in points"))
        })
        .collect();
    let nums = nums?;
    if nums.len() % 2 != 0 {
        return Err(Error::invalid("SVG: odd number count in points"));
    }
    Ok(nums
        .chunks_exact(2)
        .map(|c| Point::new(c[0], c[1]))
        .collect())
}

/// Parse `<path d="...">` directly into [`Path`].
///
/// Reads the `d` attribute only — for the round-6 SVG 2 cascade where
/// CSS `d` overrides the attribute, callers thread through
/// [`parse_path_with_css`] which consults the [`Stylesheet`] first.
pub fn parse_path(el: &Element) -> Result<Option<Path>> {
    let d = match attr(el, "d") {
        Some(v) => v,
        None => return Ok(None),
    };
    let cmds = parse_path_data(d)?;
    if cmds.is_empty() {
        return Ok(None);
    }
    Ok(Some(Path { commands: cmds }))
}

/// Round 6 — SVG 2 §9.3.2: `d` is a presentation property, so a CSS
/// declaration on the element (from a `<style>` rule or an inline
/// `style="..."`) overrides the `d` attribute. The CSS value is
/// `none | <string>`; the string must be quoted with `'` or `"` and
/// holds the same path-data mini-language the attribute does.
///
/// Cascade: matched stylesheet rules (sorted by specificity) + inline
/// style come *after* the presentation attribute, so the last `d`
/// declaration wins. A `d: none` declaration (or empty string) reduces
/// the path to "no rendering" — we return `Ok(None)` so the caller
/// drops the node.
pub fn parse_path_with_css(
    el: &Element,
    mctx: &MatchContext<'_>,
    sheet: &Stylesheet,
) -> Result<Option<Path>> {
    // Walk the cascade in order — last `d` declaration wins.
    let mut effective: Option<String> = attr(el, "d").map(|s| s.to_string());
    for (name, value) in declarations_for(mctx, sheet) {
        if name == "d" {
            effective = Some(value);
        }
    }
    let raw = match effective {
        Some(v) => v,
        None => return Ok(None),
    };
    let stripped = unwrap_d_property(&raw);
    if stripped.eq_ignore_ascii_case("none") || stripped.is_empty() {
        return Ok(None);
    }
    let cmds = parse_path_data(stripped)?;
    if cmds.is_empty() {
        return Ok(None);
    }
    Ok(Some(Path { commands: cmds }))
}

/// Strip the surrounding quotes that the CSS `d` property requires
/// (`d: "M 0 0 L 10 10"`). Inputs that came from the raw `d` attribute
/// are passed through unchanged. Whitespace around the literal is
/// trimmed in either case.
fn unwrap_d_property(raw: &str) -> &str {
    let t = raw.trim();
    let bytes = t.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return t[1..t.len() - 1].trim();
        }
    }
    t
}

/// Parse a parsed `Element` into an `oxideav-core` `Node`. Returns
/// `Ok(None)` when the element produces no visible output (e.g. a rect
/// of width 0, an unknown element).
///
/// Builds an isolated [`MatchContext`] (no parent / sibling info)
/// around `el` and delegates to [`parse_element_to_node_ctx`]. Useful
/// for callers that don't carry tree position information (e.g. the
/// `<use>` resolver, where the source element lives in `<defs>` and
/// has no meaningful position).
pub fn parse_element_to_node(
    el: &Element,
    parent_state: &PaintState,
    ctx: &mut ParseContext,
) -> Result<Option<Node>> {
    let mctx = MatchContext::root(el);
    parse_element_to_node_ctx(el, parent_state, ctx, &mctx)
}

/// Per-element-child sibling info — count of element-only children +
/// per-tag totals — pre-computed once per parent so each child's
/// MatchContext has accurate `:nth-child` / `:nth-of-type` numbers.
fn child_sibling_totals(parent_el: &Element) -> (usize, HashMap<String, usize>) {
    let mut total = 0usize;
    let mut tag_totals: HashMap<String, usize> = HashMap::new();
    for c in &parent_el.children {
        if let XmlNode::Element(e) = c {
            total += 1;
            let lower = tag_local(&e.name).to_ascii_lowercase();
            *tag_totals.entry(lower).or_insert(0) += 1;
        }
    }
    (total, tag_totals)
}

/// Build the [`MatchContext`] for one element child.
fn child_match_context<'a>(
    parent_mctx: &'a MatchContext<'a>,
    child_el: &'a Element,
    child_index: usize,
    of_type_index: usize,
    sibling_count: usize,
    of_type_count: usize,
) -> MatchContext<'a> {
    MatchContext {
        el: child_el,
        child_index,
        of_type_index,
        sibling_count,
        of_type_count,
        parent: Some(parent_mctx),
    }
}

/// Round-5 entry point. Same as [`parse_element_to_node`] but takes a
/// fully-chained [`MatchContext`] so the CSS cascade can resolve
/// combinator selectors and structural pseudo-classes.
pub fn parse_element_to_node_ctx(
    el: &Element,
    parent_state: &PaintState,
    ctx: &mut ParseContext,
    mctx: &MatchContext<'_>,
) -> Result<Option<Node>> {
    // Round 4: snapshot any `<animate>` / `<set>` /
    // `<animateTransform>` children at `ctx.animation_t` and fold them
    // into the parent element's attribute set before we look at the
    // attrs. Round 3 hard-coded `t=0`; round 4 picks the time from the
    // ParseContext so `parse_svg_at(bytes, t)` produces a stable
    // snapshot at any point on the timeline.
    let with_anim = apply_animation_overrides(el, ctx.animation_t);
    // Round 16: also fold any CSS `@keyframes`-driven animation that
    // targets this element. The SMIL pass above handles `<animate>` /
    // `<set>`; the keyframe pass below handles `animation-name: <kf>;
    // animation-duration: <s>` declarations resolved from inline
    // `style=` or matched CSS rules.
    let after_smil = with_anim.as_ref().unwrap_or(el);
    let with_kf = apply_keyframe_overrides(after_smil, mctx, &ctx.stylesheet, ctx.animation_t);
    let el = with_kf.as_ref().unwrap_or(after_smil);
    // If we cloned the element to fold animation values in, the
    // MatchContext still references the *original* element (its
    // tag/class/id/attrs are byte-identical post-clone, plus the
    // sibling counts are computed from the original parent). Rebuild
    // the context to point at the cloned `el` so attribute predicates
    // see the snapshotted values.
    let mctx_local = MatchContext { el, ..*mctx };
    let mctx = &mctx_local;
    let local = tag_local(&el.name);
    let node_opt = match local.as_str() {
        "g" => {
            let state = parent_state.merged_with_mctx(mctx, &ctx.stylesheet)?;
            // Round 19 — push the `<g>`'s `font-size` cascade into a
            // child resolve context so descendants resolve `em` /
            // `rem` against the group's font-size, not the outer
            // viewport's. Restored after this branch returns.
            let saved_ctx = ctx.resolve_ctx;
            ctx.resolve_ctx = derive_child_ctx(el, mctx, &ctx.stylesheet, &saved_ctx);
            let transform = match attr(el, "transform") {
                Some(v) => parse_transform(v)?,
                None => Transform2D::identity(),
            };
            let mut group = Group {
                transform,
                opacity: state.opacity,
                clip: None,
                children: Vec::new(),
                cache_key: None,
            };
            let (total, tag_totals) = child_sibling_totals(el);
            let mut child_idx = 0usize;
            let mut tag_seen: HashMap<String, usize> = HashMap::new();
            for child in &el.children {
                if let XmlNode::Element(c) = child {
                    let lower = tag_local(&c.name).to_ascii_lowercase();
                    let of_idx = *tag_seen.entry(lower.clone()).or_insert(0);
                    *tag_seen.get_mut(&lower).unwrap() += 1;
                    let of_count = *tag_totals.get(&lower).unwrap_or(&0);
                    let cmctx = child_match_context(mctx, c, child_idx, of_idx, total, of_count);
                    // Round 13: scene-graph index = number of children
                    // already pushed onto this group (so a `<defs>` /
                    // `<style>` in the source that produces no scene
                    // node doesn't shift the index).
                    let scene_idx = group.children.len();
                    ctx.current_path.push(scene_idx);
                    let result = parse_element_to_node_ctx(c, &state, ctx, &cmctx);
                    ctx.current_path.pop();
                    if let Some(node) = result? {
                        group.children.push(node);
                    }
                    child_idx += 1;
                }
            }
            // Restore the parent's resolve context — em-cascade is
            // strictly per-subtree.
            ctx.resolve_ctx = saved_ctx;
            Some(Node::Group(group))
        }
        "defs" => {
            // Walk children — gradient defs (round 1) plus filter /
            // mask / clipPath / symbol defs (round 2). Pre-walk in
            // `decoder::register_all_defs` already populated the
            // round-2 tables; this pass picks up gradients (which
            // depend on inheritance and are cheap to re-resolve).
            for child in &el.children {
                if let XmlNode::Element(c) = child {
                    register_def(c, ctx)?;
                }
            }
            None
        }
        "lineargradient" | "radialgradient" => {
            register_def(el, ctx)?;
            None
        }
        // Round-2: filter / mask / clipPath / symbol definitions don't
        // produce visible output by themselves — they're consumed via
        // url(#id) references on other elements. The pre-walk in
        // `decoder::register_all_defs` already captured them, so just
        // return None here.
        "filter" | "mask" | "clippath" | "symbol" => None,
        // Round-4: <style> is consumed by `css::collect_stylesheet`
        // during the pre-walk; it produces no scene-graph output.
        "style" => None,
        // Round-2: <foreignObject> remains a graceful skip — the
        // contents are typically HTML / xhtml which is out of scope.
        "foreignobject" => Some(Node::Group(Group::default())),
        // Round-3: animation tags don't render directly; their `t=0`
        // value has already been folded into the parent element's
        // attrs by `apply_animations_to_parent_attrs` (see
        // `parse_g_children` / `parse_shape_children`). Drop them
        // here so they don't appear in the scene graph.
        "animate" | "animatetransform" | "animatemotion" | "set" => None,
        // Round-3: `<use href="#id">` resolves the referenced element
        // and instantiates it as a child node, applying the use's
        // x / y / transform / width / height. See `parse_use_element`.
        "use" => parse_use_element(el, parent_state, ctx, mctx)?,
        "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "path" => {
            let state = parent_state.merged_with_mctx(mctx, &ctx.stylesheet)?;
            // Round 19 — derive the per-element resolve context: any
            // `font-size: …` cascade on this shape applies to its own
            // `em`/`rem` resolution. The viewport / root font-size /
            // viewport axes inherit from the parent unchanged.
            let elem_ctx = derive_child_ctx(el, mctx, &ctx.stylesheet, &ctx.resolve_ctx);
            let path_opt = match local.as_str() {
                "rect" => parse_rect(el, &elem_ctx)?,
                "circle" => parse_circle(el, &elem_ctx)?,
                "ellipse" => parse_ellipse(el, &elem_ctx)?,
                "line" => Some(parse_line(el, &elem_ctx)?),
                "polyline" => parse_polyline(el, false)?,
                "polygon" => parse_polyline(el, true)?,
                // Round 6: the SVG 2 `d` property allows CSS to set the
                // path data, which overrides the `d` attribute via the
                // normal cascade. `parse_path_with_css` honours that.
                "path" => parse_path_with_css(el, mctx, &ctx.stylesheet)?,
                _ => unreachable!(),
            };
            let path = match path_opt {
                Some(p) => p,
                None => return Ok(None),
            };
            // For an open `<line>` / `<polyline>`, default fill of
            // black would paint a triangle — that's surprising and
            // wrong. Per SVG 1.1 §11.4 fill *does* apply, but the
            // common-sense default is "no fill on lines unless asked".
            // We follow the spec literally; users who don't want a
            // fill set fill="none".
            let transform = attr(el, "transform").map(parse_transform).transpose()?;
            let fill = state.solid_fill(&ctx.gradients, &ctx.defs);
            let stroke = state.solid_stroke(&ctx.gradients, &ctx.defs);
            let path_node = PathNode {
                path,
                fill,
                stroke,
                fill_rule: state.fill_rule,
            };
            // If element-level transform or opacity differs from
            // parent's, wrap in a tiny one-child group so the
            // round-trip preserves them.
            let needs_wrap =
                transform.is_some() || (state.opacity - parent_state.opacity).abs() > f32::EPSILON;
            let inner = if needs_wrap {
                Node::Group(Group {
                    transform: transform.unwrap_or_else(Transform2D::identity),
                    opacity: state.opacity,
                    clip: None,
                    children: vec![Node::Path(path_node)],
                    cache_key: None,
                })
            } else {
                Node::Path(path_node)
            };
            Some(inner)
        }
        #[cfg(feature = "text")]
        "text" => {
            let state = parent_state.merged_with_mctx(mctx, &ctx.stylesheet)?;
            crate::text::parse_text_element(el, &state, ctx)?
        }
        // <text> when text feature is disabled — silently skip.
        #[cfg(not(feature = "text"))]
        "text" => None,
        // Round-2 deferral list — silently skip <use>, <script>, etc.
        // so the rest of the document still loads.
        _ => None,
    };
    // Round-2: apply mask / clip-path / filter wrappers from this
    // element's attributes to whatever node we just produced. The
    // wrapping order is filter(mask(clip(node))) — outer-most last.
    let node = match node_opt {
        Some(n) => n,
        None => return Ok(None),
    };
    let wrapped = apply_referenced_defs(el, node, &ctx.defs);
    // Round 13: record the source `id="..."` against the current
    // scene-graph path (which the caller pushed before invoking us).
    // We record against the *outer-most* wrapping so the encoder
    // emits `id=` on the topmost emitted element — matching the
    // SVG-source layout where `<rect id=... clip-path=...>` keeps the
    // id on the rect itself even though our scene-graph wraps it in
    // a clip group.
    if let Some(id) = attr(el, "id") {
        ctx.record_id_path(id);
    }
    Ok(Some(wrapped))
}

/// Apply `clip-path="url(#id)"` / `mask="url(#id)"` / `filter="url(#id)"`
/// from `el` to `node`, looking the ids up in `defs`. Missing refs are
/// silently dropped (the rest of the document still renders).
fn apply_referenced_defs(el: &Element, mut node: Node, defs: &DefsTables) -> Node {
    if let Some(id) = attr(el, "clip-path").and_then(parse_url_ref) {
        if let Some(cp) = defs.clip_paths.get(id) {
            node = wrap_with_clip(node, cp.path.clone());
        }
    }
    if let Some(id) = attr(el, "mask").and_then(parse_url_ref) {
        if let Some(m) = defs.masks.get(id) {
            node = Node::SoftMask {
                mask: Box::new(Node::Group(m.content.clone())),
                mask_kind: m.mask_kind,
                content: Box::new(node),
            };
        }
    }
    // Filter is graceful pass-through in round 2 — wrap content in a
    // single-child Group so the structural intent ("these children are
    // filtered") survives the round-trip even though the actual
    // rasterisation is deferred to oxideav-raster.
    if let Some(id) = attr(el, "filter").and_then(parse_url_ref) {
        if defs.filters.contains_key(id) {
            // TODO(round-3 / oxideav-raster #368): rasterise the
            // referenced filter graph (Gaussian blur, color matrix,
            // …). Round 2 just preserves the children so a
            // parse → encode round-trip succeeds.
            node = Node::Group(Group {
                children: vec![node],
                ..Group::default()
            });
        }
    }
    node
}

/// Wrap `node` so the rasterizer applies `clip` to its children.
/// Existing groups gain the clip directly (avoiding an extra layer);
/// other node kinds get a fresh single-child group.
fn wrap_with_clip(node: Node, clip: Path) -> Node {
    match node {
        Node::Group(mut g) if g.clip.is_none() => {
            g.clip = Some(clip);
            Node::Group(g)
        }
        other => Node::Group(Group {
            clip: Some(clip),
            children: vec![other],
            ..Group::default()
        }),
    }
}

// ---------------------------------------------------------------------------
// Round-2 def parsers — invoked by the decoder's pre-walk so forward
// references resolve regardless of source order.
// ---------------------------------------------------------------------------

/// Parse `<filter id="...">` into a [`FilterDef`]. Returns `None` if
/// the element has no `id` (then it can't be referenced).
///
/// Round 7 also walks each primitive child and parses it into the
/// typed [`crate::filter::FilterGraph`] stored on the def, so consumers
/// can access the pipeline without re-parsing the XML.
pub fn parse_filter_def(el: &Element) -> Option<(String, FilterDef)> {
    let id = attr(el, "id")?.to_string();
    let graph = crate::filter::parse_filter_graph(el);
    Some((
        id,
        FilterDef {
            element: el.clone(),
            graph,
        },
    ))
}

/// Parse `<mask id="..." mask-type="luminance|alpha">` into a
/// [`MaskDef`]. The mask subtree is parsed into a [`Group`] using the
/// provided gradient context (so gradient-filled masks work).
pub fn parse_mask_def(el: &Element, ctx: &mut ParseContext) -> Result<Option<(String, MaskDef)>> {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    let mask_kind = match attr(el, "mask-type").map(str::trim) {
        Some("alpha") => MaskKind::Alpha,
        _ => MaskKind::Luminance,
    };
    let parent_state = PaintState::default();
    let mut group = Group::default();
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            if let Some(node) = parse_element_to_node(c, &parent_state, ctx)? {
                group.children.push(node);
            }
        }
    }
    Ok(Some((
        id,
        MaskDef {
            mask_kind,
            content: group,
        },
    )))
}

/// Parse `<clipPath id="...">` into a [`ClipPathDef`]. Multiple child
/// shapes are concatenated into one path — successive shapes start
/// with a fresh `MoveTo` so the union under the non-zero / even-odd
/// fill rule reproduces the SVG semantic of "the union of every
/// child's filled interior". `<use>` references inside `<clipPath>`
/// are deferred (round 3).
pub fn parse_clip_path_def(
    el: &Element,
    ctx: &mut ParseContext,
) -> Result<Option<(String, ClipPathDef)>> {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    let mut path = Path::new();
    let elem_ctx = ctx.resolve_ctx;
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            // Re-use the shape parsers — they already handle rect /
            // circle / ellipse / polyline / polygon / line / path.
            let local = tag_local(&c.name);
            let sub = match local.as_str() {
                "rect" => parse_rect(c, &elem_ctx)?,
                "circle" => parse_circle(c, &elem_ctx)?,
                "ellipse" => parse_ellipse(c, &elem_ctx)?,
                "line" => Some(parse_line(c, &elem_ctx)?),
                "polyline" => parse_polyline(c, false)?,
                "polygon" => parse_polyline(c, true)?,
                "path" => parse_path(c)?,
                _ => None,
            };
            if let Some(p) = sub {
                // Apply per-element transform if present.
                let transformed = match attr(c, "transform") {
                    Some(v) => transform_path(p, &parse_transform(v)?),
                    None => p,
                };
                path.commands.extend(transformed.commands);
            }
        }
    }
    if path.commands.is_empty() {
        return Ok(None);
    }
    let _ = ctx;
    Ok(Some((id, ClipPathDef { path })))
}

/// Parse `<symbol id="...">` into a [`SymbolDef`]. Captured here for
/// the `<use>` resolver. Round 14 also captures the symbol's own
/// `viewBox` / `width` / `height` / `preserveAspectRatio` so the
/// resolver can apply the SVG 2 §5.5 / §8.2 viewport transform when a
/// `<use>` instantiates the symbol with its own `width` / `height`.
pub fn parse_symbol_def(
    el: &Element,
    ctx: &mut ParseContext,
) -> Result<Option<(String, SymbolDef)>> {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    let parent_state = PaintState::default();
    let mut group = Group::default();
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            if let Some(node) = parse_element_to_node(c, &parent_state, ctx)? {
                group.children.push(node);
            }
        }
    }
    let view_box = match attr(el, "viewBox") {
        Some(s) => parse_symbol_view_box(s),
        None => None,
    };
    let preserve_aspect_ratio = match attr(el, "preserveAspectRatio") {
        Some(s) => crate::filter::PreserveAspectRatio::from_str(s),
        None => crate::filter::PreserveAspectRatio::default(),
    };
    let intrinsic_width = parse_optional_length(attr(el, "width"))?;
    let intrinsic_height = parse_optional_length(attr(el, "height"))?;
    Ok(Some((
        id,
        SymbolDef {
            content: group,
            view_box,
            preserve_aspect_ratio,
            intrinsic_width,
            intrinsic_height,
        },
    )))
}

/// Round 20 — parse `<pattern id="...">` into a typed
/// [`crate::defs::PatternDef`]. SVG 2 §14.3.
///
/// Returns `None` if the element lacks an `id` (then it can't be
/// referenced via `url(#id)`). The tile content (shapes / groups /
/// nested gradients) is parsed using the existing element pipeline so a
/// `<pattern>` containing a `<rect>` round-trips faithfully.
pub fn parse_pattern_def(
    el: &Element,
    ctx: &mut ParseContext,
) -> Result<Option<(String, crate::defs::PatternDef)>> {
    use crate::defs::{PatternDef, PatternUnits};
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    // x / y default to 0; width / height default to 0. Per §14.3.1
    // negative widths/heights are errors and zero suppresses paint —
    // mirror that by capturing the parsed values as-is and letting a
    // downstream rasterizer decide.
    let x = parse_number(attr(el, "x"), 0.0)?;
    let y = parse_number(attr(el, "y"), 0.0)?;
    let width = parse_number(attr(el, "width"), 0.0)?;
    let height = parse_number(attr(el, "height"), 0.0)?;
    let pattern_units =
        parse_pattern_units(attr(el, "patternUnits"), PatternUnits::ObjectBoundingBox);
    let pattern_content_units = parse_pattern_units(
        attr(el, "patternContentUnits"),
        PatternUnits::UserSpaceOnUse,
    );
    let pattern_transform = match attr(el, "patternTransform") {
        Some(s) => parse_transform(s)?,
        None => Transform2D::identity(),
    };
    let view_box = match attr(el, "viewBox") {
        Some(s) => parse_symbol_view_box(s),
        None => None,
    };
    let preserve_aspect_ratio = match attr(el, "preserveAspectRatio") {
        Some(s) => crate::filter::PreserveAspectRatio::from_str(s),
        None => crate::filter::PreserveAspectRatio::default(),
    };
    let href = attr(el, "href")
        .or_else(|| attr(el, "xlink:href"))
        .map(|s| s.trim().trim_start_matches('#').to_string())
        .unwrap_or_default();

    // Parse the tile content using the standard pipeline. Pattern
    // children may include shapes / groups / use / nested gradients;
    // round 20 reuses the existing element parser so we get all of
    // them for free (the round-2 def-walker already registered any
    // nested defs into ctx during the pre-walk).
    let parent_state = PaintState::default();
    let mut content = Group::default();
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            if let Some(node) = parse_element_to_node(c, &parent_state, ctx)? {
                content.children.push(node);
            }
        }
    }
    Ok(Some((
        id,
        PatternDef {
            x,
            y,
            width,
            height,
            pattern_units,
            pattern_content_units,
            pattern_transform,
            view_box,
            preserve_aspect_ratio,
            href,
            content,
        },
    )))
}

/// Round 20 — `patternUnits` / `patternContentUnits` keyword parser
/// per SVG 2 §14.3.1. Unknown / malformed values fall back to the
/// caller-supplied default to mirror the spec's "ignore unknown" lenient
/// processing rule.
fn parse_pattern_units(
    v: Option<&str>,
    default: crate::defs::PatternUnits,
) -> crate::defs::PatternUnits {
    use crate::defs::PatternUnits;
    match v.map(str::trim) {
        Some("userSpaceOnUse") => PatternUnits::UserSpaceOnUse,
        Some("objectBoundingBox") => PatternUnits::ObjectBoundingBox,
        _ => default,
    }
}

/// Parse the four-number `viewBox=` attribute payload. Returns `None`
/// for any malformed input (matches the decoder-side tolerance — a bad
/// `viewBox` shouldn't kill the whole document).
fn parse_symbol_view_box(s: &str) -> Option<oxideav_core::ViewBox> {
    let nums: Vec<f32> = s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .filter_map(|n| n.parse::<f32>().ok())
        .collect();
    if nums.len() != 4 {
        return None;
    }
    Some(oxideav_core::ViewBox {
        min_x: nums[0],
        min_y: nums[1],
        width: nums[2],
        height: nums[3],
    })
}

/// Parse an optional length attribute. Returns `Ok(None)` when the
/// attribute is absent, empty, or `%`-suffixed (we don't model
/// percentage lengths on `<symbol>` / `<use>` viewports). Otherwise
/// returns the parsed number.
fn parse_optional_length(v: Option<&str>) -> Result<Option<f32>> {
    let s = match v {
        None => return Ok(None),
        Some(s) => s.trim(),
    };
    if s.is_empty() || s.ends_with('%') {
        return Ok(None);
    }
    Ok(Some(parse_number(Some(s), 0.0)?))
}

/// Apply a 2D affine to every coordinate in `path`. Used by
/// `<clipPath>` resolution where each child shape may carry its own
/// `transform=` attribute.
fn transform_path(path: Path, t: &Transform2D) -> Path {
    let map = |p: Point| t.apply(p);
    let cmds = path
        .commands
        .into_iter()
        .map(|c| match c {
            PathCommand::MoveTo(p) => PathCommand::MoveTo(map(p)),
            PathCommand::LineTo(p) => PathCommand::LineTo(map(p)),
            PathCommand::QuadCurveTo { control, end } => PathCommand::QuadCurveTo {
                control: map(control),
                end: map(end),
            },
            PathCommand::CubicCurveTo { c1, c2, end } => PathCommand::CubicCurveTo {
                c1: map(c1),
                c2: map(c2),
                end: map(end),
            },
            // ArcTo end point is a coordinate; the radii / rotation
            // would also need adjustment under a non-uniform scale —
            // that's out of scope for round 2's clipPath transform.
            PathCommand::ArcTo {
                rx,
                ry,
                x_axis_rot,
                large_arc,
                sweep,
                end,
            } => PathCommand::ArcTo {
                rx,
                ry,
                x_axis_rot,
                large_arc,
                sweep,
                end: map(end),
            },
            PathCommand::Close => PathCommand::Close,
            other => other,
        })
        .collect();
    Path { commands: cmds }
}

/// Round-1 helper that captured `<linearGradient>` / `<radialGradient>`
/// during the second tree-walk pass and inserted the legacy
/// [`Paint`] into the gradient table.
///
/// **Round 81** — superseded by [`crate::defs::resolve_gradient_chain`]
/// flattening on the typed [`crate::defs::DefsTables::gradients`] table
/// (populated during the pre-walk in
/// [`crate::decoder::register_all_defs`] then flattened en-masse
/// once after the pre-walk). The second-pass entry-point now goes
/// through `register_def_typed` which (a) registers the typed def if
/// it's not already on the table and (b) keeps the
/// `ctx.gradients` legacy `Paint` cache in sync via the chain
/// resolver.
fn register_def(el: &Element, ctx: &mut ParseContext) -> Result<()> {
    match tag_local(&el.name).as_str() {
        "lineargradient" => {
            if let Some((id, def)) = parse_linear_gradient_def(el)? {
                // Pre-walk normally got here first; only insert if the
                // tree-walk reaches an element the pre-walk missed.
                ctx.defs.gradients.entry(id.clone()).or_insert(def);
                if let Some(def) = ctx.defs.gradients.get(&id).cloned() {
                    let p = flatten_gradient_to_paint(&def, &ctx.defs);
                    ctx.gradients.insert(id, p);
                }
            }
        }
        "radialgradient" => {
            if let Some((id, def)) = parse_radial_gradient_def(el)? {
                ctx.defs.gradients.entry(id.clone()).or_insert(def);
                if let Some(def) = ctx.defs.gradients.get(&id).cloned() {
                    let p = flatten_gradient_to_paint(&def, &ctx.defs);
                    ctx.gradients.insert(id, p);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Round 3: <use href="#id"> resolver
// ---------------------------------------------------------------------------

/// Parse a `<use href="#id">` (or legacy `xlink:href`). The referenced
/// element is looked up in the documentwide id table, parsed
/// recursively, and wrapped in a `Group` carrying the use's
/// `transform` / `x` / `y` translation. `<symbol>` references skip the
/// `<symbol>` element itself and inline its children (per SVG 1.1
/// §5.5: `<use>` of a symbol does *not* re-instantiate the symbol's
/// own attrs, only its content). Cycles are detected via
/// `ctx.use_stack` and silently dropped.
pub fn parse_use_element(
    el: &Element,
    parent_state: &PaintState,
    ctx: &mut ParseContext,
    _mctx: &MatchContext<'_>,
) -> Result<Option<Node>> {
    let href = match attr(el, "href").or_else(|| attr(el, "xlink:href")) {
        Some(v) => v.trim(),
        None => return Ok(None),
    };
    let id = match href.strip_prefix('#') {
        Some(s) => s,
        // External references (`<use href="other.svg#id">`) are not
        // supported — silently drop.
        None => return Ok(None),
    };
    if ctx.use_stack.contains(id) {
        // Cycle — `use → … → use of same id`. Drop instead of
        // recursing infinitely.
        return Ok(None);
    }
    let source = match ctx.defs.elements.get(id) {
        Some(e) => e.clone(),
        None => return Ok(None),
    };

    // x / y on `<use>` are an additive translate per §5.6.
    // Round 19 — resolve unit-suffixed values via the current context.
    let x = parse_length_attr(attr(el, "x"), 0.0, LengthAxis::X, &ctx.resolve_ctx)?;
    let y = parse_length_attr(attr(el, "y"), 0.0, LengthAxis::Y, &ctx.resolve_ctx)?;
    let use_transform = match attr(el, "transform") {
        Some(v) => parse_transform(v)?,
        None => Transform2D::identity(),
    };
    // Compose: (use transform) ∘ translate(x, y) — so the translate
    // is applied first to the source, then the explicit transform.
    let translate = Transform2D::translate(x, y);
    let total = use_transform.compose(&translate);

    let state = parent_state.merged_with(el)?;

    // Round 14: <use> may carry its own width / height that override
    // the symbol's intrinsic size. Captured here so the symbol branch
    // below can compute the §8.2 viewport transform.
    let use_width = parse_optional_length(attr(el, "width"))?;
    let use_height = parse_optional_length(attr(el, "height"))?;

    ctx.use_stack.insert(id.to_string());
    // For `<symbol>` references, instantiate the symbol's children
    // directly (skip the `<symbol>` wrapper — symbols are by-spec
    // invisible until referenced via <use>). For any other element,
    // re-parse the source itself.
    let source_local = tag_local(&source.name);
    let mut children: Vec<Node> = Vec::new();
    let mut viewport_transform: Option<Transform2D> = None;
    if source_local == "symbol" {
        // Round 14: prefer the pre-parsed SymbolDef (it carries the
        // symbol's viewBox + preserveAspectRatio + intrinsic size).
        // The defs.elements clone is the verbatim XML; the SymbolDef
        // is the structured form built during register_all_defs.
        let sym_meta = ctx.defs.symbols.get(id).cloned();
        let (sym_view_box, sym_par, sym_w, sym_h) = match &sym_meta {
            Some(s) => (
                s.view_box,
                s.preserve_aspect_ratio,
                s.intrinsic_width,
                s.intrinsic_height,
            ),
            None => (
                None,
                crate::filter::PreserveAspectRatio::default(),
                None,
                None,
            ),
        };
        // SVG 2 §5.6 — the use's width / height fall through to the
        // symbol's intrinsic width / height when the use omits them.
        let dst_w = use_width.or(sym_w);
        let dst_h = use_height.or(sym_h);
        if let (Some(vb), Some(w), Some(h)) = (sym_view_box, dst_w, dst_h) {
            if vb.width > 0.0 && vb.height > 0.0 && w > 0.0 && h > 0.0 {
                // Spec algorithm 8.2 — viewport rect (0..w × 0..h)
                // mapping to viewBox rect.
                viewport_transform = Some(symbol_viewport_transform(w, h, vb, sym_par));
            }
        }
        for child in &source.children {
            if let XmlNode::Element(c) = child {
                if let Some(node) = parse_element_to_node(c, &state, ctx)? {
                    children.push(node);
                }
            }
        }
    } else if let Some(node) = parse_element_to_node(&source, &state, ctx)? {
        children.push(node);
    }
    ctx.use_stack.remove(id);

    if children.is_empty() {
        return Ok(None);
    }

    // Round 14: when the symbol contributes a viewport transform, wrap
    // the symbol's children in an inner Group carrying that transform
    // BEFORE the outer Group applies the use's translate / transform /
    // opacity. The two-level wrap keeps the use's `transform=` /
    // `opacity` semantics independent of the viewport mapping.
    let final_children = if let Some(vp) = viewport_transform {
        vec![Node::Group(Group {
            transform: vp,
            opacity: 1.0,
            clip: None,
            children,
            cache_key: None,
        })]
    } else {
        children
    };

    // Always wrap in a Group so the use's transform / opacity apply
    // to every instantiated child, even when there's just one.
    Ok(Some(Node::Group(Group {
        transform: total,
        opacity: state.opacity,
        clip: None,
        children: final_children,
        cache_key: None,
    })))
}

/// Round 14 — given a `<use>` viewport (`width` × `height`), the
/// referenced symbol's `viewBox`, and its `preserveAspectRatio`,
/// return the transform that maps points in the symbol's user
/// coordinate system into the use's viewport. Mirrors the root-`<svg>`
/// `viewport_correction_transform` in [`crate::decoder`] but emits the
/// full spec transform directly (no "natural mapping" subtraction)
/// because the symbol's children have no implicit canvas-vs-viewBox
/// stretch baked in by an outer renderer.
fn symbol_viewport_transform(
    width: f32,
    height: f32,
    vb: oxideav_core::ViewBox,
    par: crate::filter::PreserveAspectRatio,
) -> Transform2D {
    use crate::filter::{MeetOrSlice, PreserveAspectRatioAlign};
    // Spec algorithm 8.2 step 2 — initial scale.
    let nat_sx = width / vb.width;
    let nat_sy = height / vb.height;
    let (sx, sy) = if matches!(par.align, PreserveAspectRatioAlign::None) {
        (nat_sx, nat_sy)
    } else {
        match par.meet_or_slice {
            MeetOrSlice::Meet => {
                let s = nat_sx.min(nat_sy);
                (s, s)
            }
            MeetOrSlice::Slice => {
                let s = nat_sx.max(nat_sy);
                (s, s)
            }
        }
    };
    // Spec steps 9–14 — translate.
    let mut tx = -vb.min_x * sx;
    let mut ty = -vb.min_y * sy;
    if !matches!(par.align, PreserveAspectRatioAlign::None) {
        let dx = width - vb.width * sx;
        let dy = height - vb.height * sy;
        let x_mid = matches!(
            par.align,
            PreserveAspectRatioAlign::XMidYMin
                | PreserveAspectRatioAlign::XMidYMid
                | PreserveAspectRatioAlign::XMidYMax
        );
        let x_max = matches!(
            par.align,
            PreserveAspectRatioAlign::XMaxYMin
                | PreserveAspectRatioAlign::XMaxYMid
                | PreserveAspectRatioAlign::XMaxYMax
        );
        let y_mid = matches!(
            par.align,
            PreserveAspectRatioAlign::XMinYMid
                | PreserveAspectRatioAlign::XMidYMid
                | PreserveAspectRatioAlign::XMaxYMid
        );
        let y_max = matches!(
            par.align,
            PreserveAspectRatioAlign::XMinYMax
                | PreserveAspectRatioAlign::XMidYMax
                | PreserveAspectRatioAlign::XMaxYMax
        );
        if x_mid {
            tx += dx / 2.0;
        } else if x_max {
            tx += dx;
        }
        if y_mid {
            ty += dy / 2.0;
        } else if y_max {
            ty += dy;
        }
    }
    // viewport_transform = translate(tx, ty) * scale(sx, sy) *
    //                      translate(-vb.min_x, -vb.min_y)
    Transform2D::translate(tx, ty)
        .compose(&Transform2D::scale(sx, sy))
        .compose(&Transform2D::translate(-vb.min_x, -vb.min_y))
}

// ---------------------------------------------------------------------------
// Round 4: animation snapshot at arbitrary `t` (replaces the round-3
// hard-coded `t=0` shortcut). Delegates to `crate::animation` for the
// SMIL timing model.
// ---------------------------------------------------------------------------

/// **Round 16** — evaluate every CSS `@keyframes`-driven animation
/// targeting `el` at `t_seconds` and fold the lerped property values
/// into a clone of `el`'s `style=` attribute so the cascade picks them
/// up via the existing [`PaintState::merged_with_mctx`] code path.
///
/// Returns `None` when no keyframe overrides apply (so the caller can
/// keep the original `el` by reference — the common case).
fn apply_keyframe_overrides(
    el: &Element,
    mctx: &MatchContext<'_>,
    sheet: &Stylesheet,
    t_seconds: f32,
) -> Option<Element> {
    // Build a temp MatchContext that points at `el` (it might be the
    // SMIL-folded clone, in which case the original `mctx.el` would
    // miss any animation-* attrs spliced in by the SMIL pass).
    let mctx_local = MatchContext { el, ..*mctx };
    let overrides = crate::keyframe::evaluate_at(&mctx_local, sheet, t_seconds);
    if overrides.is_empty() {
        return None;
    }
    // Splice the resolved declarations into `style=` so the existing
    // declarations_for() pipeline picks them up at the highest
    // precedence (inline-style wins over matched-CSS rules per the
    // round-4 cascade). The `transform` property additionally lands in
    // the `transform=` attribute because the `<g>` / shape parsers
    // read `parse_transform(attr(el, "transform"))` directly — going
    // through the CSS cascade isn't enough.
    let mut clone = el.clone();
    let mut style_overrides: Vec<&(String, String)> = Vec::new();
    for entry in &overrides {
        let lower = entry.0.to_ascii_lowercase();
        if lower == "transform" {
            // Write straight to the `transform=` attribute slot.
            let mut replaced = false;
            for (k, v) in clone.attrs.iter_mut() {
                if k.eq_ignore_ascii_case("transform") {
                    *v = entry.1.clone();
                    replaced = true;
                    break;
                }
            }
            if !replaced {
                clone.attrs.push(("transform".into(), entry.1.clone()));
            }
        } else {
            style_overrides.push(entry);
        }
    }
    if !style_overrides.is_empty() {
        let mut existing = String::new();
        let mut style_idx: Option<usize> = None;
        for (i, (k, v)) in clone.attrs.iter().enumerate() {
            if k.eq_ignore_ascii_case("style") {
                existing = v.clone();
                style_idx = Some(i);
                break;
            }
        }
        let mut composed = existing.trim().trim_end_matches(';').to_string();
        if !composed.is_empty() {
            composed.push(';');
        }
        for (i, entry) in style_overrides.iter().enumerate() {
            if i > 0 {
                composed.push(';');
            }
            composed.push_str(&entry.0);
            composed.push(':');
            composed.push_str(&entry.1);
        }
        match style_idx {
            Some(i) => clone.attrs[i].1 = composed,
            None => clone.attrs.push(("style".into(), composed)),
        }
    }
    Some(clone)
}

/// Walk `el`'s children for `<animate>` / `<set>` / `<animateTransform>`
/// tags, evaluate each at `t_seconds`, and return a clone of `el` with
/// the snapshot values spliced into its attrs (taking precedence over
/// any existing attribute of the same name). Returns `None` when there
/// are no animation children, so the caller can keep using the
/// original element by reference for the common case.
fn apply_animation_overrides(el: &Element, t_seconds: f32) -> Option<Element> {
    let overrides = crate::animation::snapshot_children(el, t_seconds);
    if overrides.is_empty() {
        return None;
    }
    let mut clone = el.clone();
    for (name, value) in overrides {
        let mut replaced = false;
        for (k, v) in clone.attrs.iter_mut() {
            if tag_local(k).eq_ignore_ascii_case(&name) {
                *v = value.clone();
                replaced = true;
                break;
            }
        }
        if !replaced {
            clone.attrs.push((name, value));
        }
    }
    Some(clone)
}

/// Round 19 — parse a length-bearing attribute and resolve it to a
/// CSS px (≡ user unit) value via the supplied [`ResolveContext`].
///
/// Bare-numeric inputs (`<rect x="100">`) round-trip bit-for-bit
/// identical to [`parse_number`] because [`crate::length::Length::resolve`]
/// is the identity for [`crate::length::LengthUnit::UserUnit`]. Inputs
/// carrying a CSS Values L4 unit suffix (`em` / `rem` / `%` / `vw` /
/// `vh` / `vmin` / `vmax` / `pt` / `pc` / `cm` / `mm` / `in` / `q` /
/// `px`) are resolved via the typed [`crate::length::Length::resolve`]
/// path with `axis` selecting the percentage basis (per SVG 2 §7.10:
/// width attrs against viewport width, height against viewport height,
/// `r` / radii against the viewport diagonal).
///
/// Empty / missing values fall through to `default`. Malformed values
/// fall back to [`parse_number`]'s lenient prefix-strip — preserving
/// the round-1..18 behaviour where a malformed coordinate didn't
/// crash the document.
pub fn parse_length_attr(
    v: Option<&str>,
    default: f32,
    axis: LengthAxis,
    ctx: &ResolveContext,
) -> Result<f32> {
    let s = match v {
        None => return Ok(default),
        Some(s) => s.trim(),
    };
    if s.is_empty() {
        return Ok(default);
    }
    match parse_length(s) {
        Ok(l) => {
            // Inject the axis-specific percentage basis so `%` inputs
            // resolve against the right viewport axis.
            let basis = ctx.percentage_basis_for(axis);
            let local = ResolveContext {
                percentage_basis_px: basis,
                ..*ctx
            };
            Ok(l.resolve(local))
        }
        // Fall back to the legacy lenient parser — keeps round-1..18
        // behaviour for the rare malformed-but-numeric-prefix inputs
        // the typed parser rejects (`12foo`).
        Err(_) => parse_number(Some(s), default),
    }
}

/// Round 19 — derive a child [`ResolveContext`] that carries any
/// `font-size: <length>` cascade `el` (or its matched CSS rules)
/// declares. The em / rem / vw / vh / viewport state inherits from
/// `parent` unchanged when no `font-size` resolves.
///
/// Per CSS Values L4 §6.1.2 + CSS Cascade L4: a `font-size: 1em`
/// resolves against the *parent* element's font-size — so the new
/// `font_size_px` is computed using `parent.font_size_px` as the em
/// basis, *then* installed on the returned context for the element's
/// descendants to use.
pub fn derive_child_ctx(
    el: &Element,
    mctx: &MatchContext<'_>,
    sheet: &Stylesheet,
    parent: &ResolveContext,
) -> ResolveContext {
    // Walk the cascade — last `font-size` declaration wins. Inline
    // `font-size="…"` presentation attrs lose to CSS rules + inline
    // style (per the round-4 cascade), so check attrs first then CSS.
    let mut effective: Option<String> = None;
    for (name, _) in &el.attrs {
        if name.eq_ignore_ascii_case("font-size") {
            effective = attr(el, name).map(|s| s.to_string());
        }
    }
    for (name, value) in declarations_for(mctx, sheet) {
        if name.eq_ignore_ascii_case("font-size") {
            effective = Some(value);
        }
    }
    let raw = match effective {
        Some(s) => s,
        None => return *parent,
    };
    // Resolve the font-size value against the *parent* font-size /
    // viewport (em-on-font-size cascades, not on the new font-size).
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return *parent;
    }
    let parsed = match parse_length(trimmed) {
        Ok(l) => l,
        Err(_) => return *parent,
    };
    // Use a Y-axis basis for percentage font-sizes (CSS Fonts L3 §3.5
    // — `font-size: 50%` resolves against the parent's font-size, but
    // since we don't have a separate parent-font-size axis we feed the
    // parent font-size in via `percentage_basis_px`).
    let basis_ctx = ResolveContext {
        percentage_basis_px: parent.font_size_px,
        ..*parent
    };
    let new_font_size = parsed.resolve(basis_ctx);
    if !new_font_size.is_finite() || new_font_size <= 0.0 {
        return *parent;
    }
    ResolveContext {
        font_size_px: new_font_size,
        ..*parent
    }
}

/// Parse a number literal — strips optional unit suffix (`px`, `pt`,
/// `em`, `%`, etc.). Round 1 treats every unit as user units, so the
/// numeric value is preserved as f32 with no scaling.
pub fn parse_number(v: Option<&str>, default: f32) -> Result<f32> {
    let s = match v {
        None => return Ok(default),
        Some(s) => s.trim(),
    };
    if s.is_empty() {
        return Ok(default);
    }
    // Find the longest prefix that parses as f32. This handles unit
    // suffixes like "12px", "3.5em", "50%" — and avoids treating the
    // 'e' in "3.5em" as the start of an exponent (which would consume
    // "3.5e" and then fail to parse).
    let bytes = s.as_bytes();
    let mut best: Option<f32> = None;
    let mut i = 1;
    while i <= bytes.len() {
        // Only consider extending while the prefix could still be a
        // valid number character; once we hit a unit char we stop.
        let c = bytes[i - 1] as char;
        let numeric =
            c.is_ascii_digit() || c == '.' || c == '+' || c == '-' || c == 'e' || c == 'E';
        if !numeric {
            break;
        }
        if let Ok(v) = s[..i].parse::<f32>() {
            best = Some(v);
        }
        i += 1;
    }
    best.ok_or_else(|| Error::invalid("SVG: malformed number"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elem(name: &str, attrs: &[(&str, &str)]) -> Element {
        Element {
            name: name.to_string(),
            attrs: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            children: Vec::new(),
        }
    }

    #[test]
    fn rect_with_no_radius_is_5_segment_path() {
        let ctx = ResolveContext::default();
        let r = parse_rect(
            &elem(
                "rect",
                &[("x", "1"), ("y", "2"), ("width", "10"), ("height", "5")],
            ),
            &ctx,
        )
        .unwrap()
        .unwrap();
        // M, L, L, L, Z = 5 commands.
        assert_eq!(r.commands.len(), 5);
        assert_eq!(r.commands[0], PathCommand::MoveTo(Point::new(1.0, 2.0)));
        assert_eq!(*r.commands.last().unwrap(), PathCommand::Close);
    }

    #[test]
    fn rect_with_radius_uses_arcs() {
        let ctx = ResolveContext::default();
        let r = parse_rect(
            &elem(
                "rect",
                &[
                    ("x", "0"),
                    ("y", "0"),
                    ("width", "10"),
                    ("height", "10"),
                    ("rx", "2"),
                ],
            ),
            &ctx,
        )
        .unwrap()
        .unwrap();
        assert!(r
            .commands
            .iter()
            .any(|c| matches!(c, PathCommand::ArcTo { .. })));
    }

    #[test]
    fn circle_zero_radius_returns_none() {
        let ctx = ResolveContext::default();
        let r = parse_circle(&elem("circle", &[("r", "0")]), &ctx).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn rect_em_resolves_against_default_font_size() {
        // <rect x="1em" width="2em"> at default font-size 16 → x=16, w=32.
        let ctx = ResolveContext::default();
        let r = parse_rect(
            &elem(
                "rect",
                &[("x", "1em"), ("y", "0"), ("width", "2em"), ("height", "1")],
            ),
            &ctx,
        )
        .unwrap()
        .unwrap();
        // First command is MoveTo(x, y).
        match r.commands[0] {
            PathCommand::MoveTo(p) => {
                assert!((p.x - 16.0).abs() < 1e-4, "x: got {}", p.x);
                assert!(p.y.abs() < 1e-4, "y: got {}", p.y);
            }
            _ => panic!("expected MoveTo"),
        }
        // Width is reflected in the LineTo of the second command —
        // x + w = 16 + 32 = 48.
        match r.commands[1] {
            PathCommand::LineTo(p) => {
                assert!((p.x - 48.0).abs() < 1e-4, "x+w: got {}", p.x);
            }
            _ => panic!("expected LineTo"),
        }
    }

    #[test]
    fn rect_em_under_explicit_font_size_cascade() {
        // <rect width="2em"> resolves to 64 with font-size 32.
        let ctx = ResolveContext::default().with_font_size(32.0);
        let r = parse_rect(
            &elem(
                "rect",
                &[("x", "0"), ("y", "0"), ("width", "2em"), ("height", "1")],
            ),
            &ctx,
        )
        .unwrap()
        .unwrap();
        match r.commands[1] {
            PathCommand::LineTo(p) => {
                assert!((p.x - 64.0).abs() < 1e-4, "x+w: got {}", p.x);
            }
            _ => panic!("expected LineTo"),
        }
    }

    #[test]
    fn circle_percent_resolves_against_viewport_diagonal() {
        // <circle cx="50%" cy="50%" r="10%"> at viewport 200x200.
        // Diagonal basis = sqrt(2*200²) / sqrt(2) = 200; so r=20.
        // cx/cy use the X/Y axes → 100 each.
        let ctx = ResolveContext::default().with_viewport(200.0, 200.0);
        let c = parse_circle(
            &elem("circle", &[("cx", "50%"), ("cy", "50%"), ("r", "10%")]),
            &ctx,
        )
        .unwrap()
        .unwrap();
        // First MoveTo is at (cx + r, cy) = (120, 100).
        match c.commands[0] {
            PathCommand::MoveTo(p) => {
                assert!((p.x - 120.0).abs() < 1e-3, "x: got {}", p.x);
                assert!((p.y - 100.0).abs() < 1e-3, "y: got {}", p.y);
            }
            _ => panic!("expected MoveTo"),
        }
    }

    #[test]
    fn polyline_parses_points() {
        let p = parse_polyline(&elem("polyline", &[("points", "0,0 10,10 20,0")]), false)
            .unwrap()
            .unwrap();
        assert_eq!(p.commands.len(), 3); // M + 2L
    }

    #[test]
    fn polygon_parses_points_and_closes() {
        let p = parse_polyline(&elem("polygon", &[("points", "0,0 10,10 20,0")]), true)
            .unwrap()
            .unwrap();
        assert_eq!(p.commands.len(), 4); // M + 2L + Z
    }

    #[test]
    fn parse_number_strips_unit_suffix() {
        assert_eq!(parse_number(Some("12px"), 0.0).unwrap(), 12.0);
        assert_eq!(parse_number(Some("3.5em"), 0.0).unwrap(), 3.5);
    }

    #[test]
    fn linear_gradient_with_two_stops() {
        let stop_a = Element {
            name: "stop".into(),
            attrs: vec![
                ("offset".into(), "0".into()),
                ("stop-color".into(), "red".into()),
            ],
            children: Vec::new(),
        };
        let stop_b = Element {
            name: "stop".into(),
            attrs: vec![
                ("offset".into(), "1".into()),
                ("stop-color".into(), "blue".into()),
            ],
            children: Vec::new(),
        };
        let lg = Element {
            name: "linearGradient".into(),
            attrs: vec![("id".into(), "g1".into())],
            children: vec![XmlNode::Element(stop_a), XmlNode::Element(stop_b)],
        };
        let (id, paint) = parse_linear_gradient(&lg).unwrap().unwrap();
        assert_eq!(id, "g1");
        match paint {
            Paint::LinearGradient(g) => {
                assert_eq!(g.stops.len(), 2);
                assert_eq!(g.stops[0].color, Rgba::opaque(255, 0, 0));
            }
            _ => panic!("expected linear gradient"),
        }
    }

    #[test]
    fn paint_state_inheritance_overrides_specified_only() {
        let parent = PaintState::default();
        let el = elem("rect", &[("fill", "red")]);
        let child = parent.merged_with(&el).unwrap();
        match child.fill {
            PaintValue::Color(c) => assert_eq!(c, Rgba::opaque(255, 0, 0)),
            _ => panic!(),
        }
        // stroke unchanged.
        assert_eq!(child.stroke, parent.stroke);
    }
}
