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
//!   into a [`oxideav_core::Group`] (mask) or a [`oxideav_core::Path`]
//!   (clipPath — multiple shapes are concatenated into one path so the
//!   even-odd / non-zero fill rule of the union approximates the SVG
//!   clip).
//!
//! - **Symbols** (`<symbol>` / future `<use>`) are captured as
//!   deferred-render groups. Round 2 doesn't yet implement `<use>` so
//!   the symbols table is wired into the parser but never consumed —
//!   this just preserves the captured definition for round 3.

use std::collections::HashMap;

use oxideav_core::{FillRule, GradientStop, Group, Path, SpreadMethod, Transform2D, ViewBox};

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

/// Round 81 — `gradientUnits` keyword per SVG 2 §14.2.2.1 /
/// §14.2.3.1. Mirrors [`PatternUnits`] but stays a distinct enum so a
/// downstream consumer can tell at a glance which paint server it's
/// looking at. Default per spec is `ObjectBoundingBox`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GradientUnits {
    UserSpaceOnUse,
    /// SVG 2 §14.2.2.1 / §14.2.3.1 "initial value: objectBoundingBox".
    #[default]
    ObjectBoundingBox,
}

/// Round 81 — geometry-bearing fields of a gradient definition. Stored
/// as `Option<f32>` so the SVG 2 §14.1.1 template-inheritance walker
/// can distinguish "attribute not specified, inherit from template" from
/// "attribute specified with an explicit value." After resolution the
/// `None`s collapse to the spec's per-attribute initial value (per
/// §14.2.2.1 / §14.2.3.1: `x1=0, y1=0, x2=1, y2=0, cx=0.5, cy=0.5, r=0.5,
/// fx=cx, fy=cy, fr=0`).
#[derive(Clone, Debug)]
pub enum GradientKind {
    Linear {
        x1: Option<f32>,
        y1: Option<f32>,
        x2: Option<f32>,
        y2: Option<f32>,
    },
    Radial {
        cx: Option<f32>,
        cy: Option<f32>,
        r: Option<f32>,
        fx: Option<f32>,
        fy: Option<f32>,
        /// SVG 2-only `fr` attribute (radius of the focal point's
        /// circle). Defaults to 0 when unspecified per §14.2.3.1.
        fr: Option<f32>,
    },
}

/// Round 81 — `<linearGradient>` / `<radialGradient>` typed view.
///
/// SVG 2 §14.1.1 "Using paint servers as templates" lets a gradient
/// inherit any unspecified attribute (plus child stops) from a chain
/// of templates referenced via `href` / `xlink:href`. The typed
/// `GradientDef` records the *un-resolved* per-element state — each
/// numeric attribute is `Option<f32>` so the chain walker can tell
/// `None` (inherit) from `Some(0.0)` (explicitly zero). After
/// resolution via [`crate::defs::resolve_gradient_chain`] the
/// `None`s collapse to the spec-default initial values.
///
/// `units` / `transform` / `spread` are `Option<_>` for the same
/// reason. `stops` empty means "inherit from template," matching the
/// §14.1.1 rule "if the current element does not have any child
/// content other than descriptive elements, the child content of the
/// template element is cloned to replace it."
#[derive(Clone, Debug)]
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

/// Round 81 — final resolved gradient: every attribute pinned to a
/// concrete value, stops populated. Produced by
/// [`resolve_gradient_chain`] from a chain of [`GradientDef`]s.
#[derive(Clone, Debug)]
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

/// SVG 2 §14.1.1 template-inheritance walker. Resolves `start.href`
/// chain through `defs.gradients` filling in any `None` attribute /
/// missing stops from the nearest template that *does* specify them.
/// Returns the final resolved gradient (with spec defaults substituted
/// for attributes that survive the whole chain unspecified).
///
/// Cycle / depth limit: the walk caps at [`GRADIENT_HREF_DEPTH_CAP`]
/// hops (matches the eight-hop cap [`crate::css::Stylesheet::IMPORT_DEPTH_CAP`]
/// uses for CSS `@import` chains). A self-reference or a longer chain
/// terminates at the cap with whatever resolution was reached.
pub fn resolve_gradient_chain(start: &GradientDef, defs: &DefsTables) -> ResolvedGradient {
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
        GradientKind::Linear { x1, y1, x2, y2 } => (Some((*x1, *y1, *x2, *y2)), None),
        GradientKind::Radial {
            cx,
            cy,
            r,
            fx,
            fy,
            fr,
        } => (None, Some((*cx, *cy, *r, *fx, *fy, *fr))),
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
                if l.0.is_none() {
                    l.0 = *x1;
                }
                if l.1.is_none() {
                    l.1 = *y1;
                }
                if l.2.is_none() {
                    l.2 = *x2;
                }
                if l.3.is_none() {
                    l.3 = *y2;
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
                if rd.0.is_none() {
                    rd.0 = *cx;
                }
                if rd.1.is_none() {
                    rd.1 = *cy;
                }
                if rd.2.is_none() {
                    rd.2 = *r;
                }
                if rd.3.is_none() {
                    rd.3 = *fx;
                }
                if rd.4.is_none() {
                    rd.4 = *fy;
                }
                if rd.5.is_none() {
                    rd.5 = *fr;
                }
            }
            _ => {}
        }
    }

    let kind = if let Some((x1, y1, x2, y2)) = lin {
        ResolvedGradientKind::Linear {
            // §14.2.2.1 initial values.
            x1: x1.unwrap_or(0.0),
            y1: y1.unwrap_or(0.0),
            x2: x2.unwrap_or(1.0),
            y2: y2.unwrap_or(0.0),
        }
    } else if let Some((cx, cy, r, fx, fy, fr)) = rad {
        let cx_v = cx.unwrap_or(0.5);
        let cy_v = cy.unwrap_or(0.5);
        ResolvedGradientKind::Radial {
            // §14.2.3.1 initial values: cx=cy=0.5, r=0.5, fx=cx, fy=cy,
            // fr=0.
            cx: cx_v,
            cy: cy_v,
            r: r.unwrap_or(0.5),
            fx: fx.unwrap_or(cx_v),
            fy: fy.unwrap_or(cy_v),
            fr: fr.unwrap_or(0.0),
        }
    } else {
        unreachable!("kind_root must be Linear or Radial");
    };

    ResolvedGradient {
        kind,
        units: units.unwrap_or_default(),
        transform: transform.unwrap_or_else(Transform2D::identity),
        spread: spread.unwrap_or(SpreadMethod::Pad),
        stops,
    }
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
/// [`oxideav_core::Node::SoftMask`] without re-walking the XML.
#[derive(Clone, Debug)]
pub struct MaskDef {
    /// `mask-type="luminance"` (default) or `"alpha"`.
    pub mask_kind: oxideav_core::MaskKind,
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
/// `oxideav_core::Paint` lacks a `Pattern` variant — the IR was last
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
/// `marker-mid` / `marker-end` properties. `oxideav_core::Node` has no
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
    /// order. `oxideav_core::Node` has no `Marker` variant yet, so the
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
    use oxideav_core::{GradientStop, Rgba};

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
        GradientDef {
            kind: GradientKind::Linear {
                x1: None,
                y1: None,
                x2,
                y2: None,
            },
            units: None,
            transform: None,
            spread: None,
            stops: Vec::new(),
            href: href.into(),
        }
    }

    fn radial(href: &str) -> GradientDef {
        GradientDef {
            kind: GradientKind::Radial {
                cx: None,
                cy: None,
                r: None,
                fx: None,
                fy: None,
                fr: None,
            },
            units: None,
            transform: None,
            spread: None,
            stops: Vec::new(),
            href: href.into(),
        }
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
            cx: Some(2.0),
            cy: Some(3.0),
            r: Some(5.0),
            fx: None,
            fy: None,
            fr: Some(0.25),
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
