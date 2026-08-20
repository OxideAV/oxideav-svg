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

/// Round 118 — SVG 1.1 §11.5 `visibility` property
/// (`visible | hidden | collapse | inherit`). Unlike `display`,
/// `visibility` IS an inherited property, so it lives on the cascaded
/// [`PaintState`]; a descendant may flip a `hidden` ancestor back to
/// `visible`. `hidden` and `collapse` are visually identical for SVG
/// (the spec note: "The current graphics element is invisible") — the
/// distinction only matters for CSS table layout, which SVG doesn't
/// model — so we collapse them to a single `Hidden` variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Visible,
    Hidden,
}

/// Round 172 — SVG 2 §11.10.1.1 `text-anchor` property (`start |
/// middle | end`). Inherited, initial value `start`. Shifts a text
/// chunk so the start / geometric middle / end of the rendered run
/// aligns to the chunk's initial current text position. For an
/// `<textPath>`, the same property biases the start-point-on-the-path
/// per §11.8.3 (subtract half the total advance for `middle`, the full
/// total for `end`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextAnchor {
    #[default]
    Start,
    Middle,
    End,
}

/// Round 205 — SVG 2 §13.8 `paint-order` property
/// (`normal | [ fill || stroke || markers ]`). Controls the order the
/// three paint operations are applied to a shape or text-content
/// element. Per §13.8:
///
/// * `normal` paints fill, then stroke, then markers.
/// * Any combination of the three keywords paints them in the order
///   given, left to right. Omitted keywords are appended in the
///   `normal` order, so `paint-order: stroke` is equivalent to
///   `paint-order: stroke fill markers`.
///
/// The property is **inherited** with initial value `normal`, and
/// applies to shapes and text-content elements per the §13.8 attribute
/// table.
///
/// `oxideav_core::Node` has no `Marker` variant — round 104 captured
/// `<marker>` definitions for round-trip but the vertex-binding /
/// rendering is deferred until core grows a `Marker` construct — so
/// the markers slot of `paint-order` parses and round-trips but
/// contributes no scene-graph node today.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PaintOp {
    #[default]
    Fill,
    Stroke,
    Markers,
}

/// Round 205 — SVG 2 §13.8 `paint-order` resolved value. `Normal`
/// reproduces the round-1 fill-then-stroke behaviour; `Custom` carries
/// the resolved three-deep operation list (which the §13.8 algorithm
/// always fills with three entries by appending omitted keywords in
/// `normal` order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PaintOrder {
    /// Initial value — fill, then stroke, then markers.
    #[default]
    Normal,
    /// Resolved 3-tuple of paint operations in source order.
    Custom(PaintOp, PaintOp, PaintOp),
}

impl PaintOrder {
    /// Resolve the spec's "the three operations are painted in source
    /// order; omitted keywords are appended in the normal order" rule
    /// to a 3-tuple. Returns `None` if no recognised keywords are
    /// present (caller treats as keep-inherited).
    pub(crate) fn parse_custom(value: &str) -> Option<Self> {
        // §13.8 grammar: `[ fill || stroke || markers ]`. Tokens are
        // whitespace-separated; at most one of each keyword; unknown
        // tokens are tolerated (they fall through silently the same
        // way unknown `text-anchor` keywords do — keeps the document
        // loading even on a typo).
        let mut seen = [false; 3]; // index 0=Fill, 1=Stroke, 2=Markers
        let mut order: Vec<PaintOp> = Vec::with_capacity(3);
        for tok in value.split_ascii_whitespace() {
            let op = if tok.eq_ignore_ascii_case("fill") {
                PaintOp::Fill
            } else if tok.eq_ignore_ascii_case("stroke") {
                PaintOp::Stroke
            } else if tok.eq_ignore_ascii_case("markers") {
                PaintOp::Markers
            } else {
                // Unknown keyword — skip per the same tolerant policy
                // the §11.10.1.1 text-anchor branch uses.
                continue;
            };
            let i = match op {
                PaintOp::Fill => 0,
                PaintOp::Stroke => 1,
                PaintOp::Markers => 2,
            };
            // Per §13.8 each keyword appears at most once; a duplicate
            // is a syntax error per the CSS `||` combinator — drop
            // subsequent occurrences silently.
            if seen[i] {
                continue;
            }
            seen[i] = true;
            order.push(op);
        }
        if order.is_empty() {
            return None;
        }
        // §13.8 — "If any of the three keywords are omitted, they are
        // painted last, in the order they would be painted with
        // paint-order: normal" — append fill, stroke, markers in that
        // order for any not yet seen.
        for (i, op) in [PaintOp::Fill, PaintOp::Stroke, PaintOp::Markers]
            .iter()
            .enumerate()
        {
            if !seen[i] {
                order.push(*op);
            }
        }
        Some(Self::Custom(order[0], order[1], order[2]))
    }

    /// Returns `true` when the resolved order would render the stroke
    /// **before** the fill (the `paint-order: stroke …` case from the
    /// §13.8 example). The non-trivial case for the scene graph today —
    /// `Normal` and fill-first orders all reduce to the round-1
    /// single-PathNode emission.
    pub(crate) fn stroke_before_fill(&self) -> bool {
        match self {
            Self::Normal => false,
            Self::Custom(a, b, c) => {
                // Find positions of Fill and Stroke.
                let pos =
                    |op: PaintOp| -> Option<usize> { [a, b, c].iter().position(|x| **x == op) };
                match (pos(PaintOp::Stroke), pos(PaintOp::Fill)) {
                    (Some(s), Some(f)) => s < f,
                    _ => false,
                }
            }
        }
    }
}

/// Round 209 — SVG 2 §8.13 `vector-effect` keyword. Each keyword in a
/// non-`none` `vector-effect` value selects one constrained-transform
/// effect; the spec's `[ non-scaling-stroke | non-scaling-size |
/// non-rotation | fixed-position ]+` grammar lets the author combine
/// several (e.g. `non-scaling-size non-rotation`). The actual transform
/// suppression happens in the renderer (`oxideav-raster`); this crate
/// parses the keyword set, round-trips it, and exposes it on the
/// resolved [`PaintState`] for downstream consumption.
///
/// Per SVG 2 §8.13 the spec WG flagged values other than
/// `non-scaling-stroke` and `none` as at risk of being dropped from
/// SVG 2 due to a lack of implementations (the issue 31 note in §8.13
/// preamble). We model all four so the parse + round-trip is faithful
/// to the spec grammar even if a future revision narrows the value
/// set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorEffectKeyword {
    NonScalingStroke,
    NonScalingSize,
    NonRotation,
    FixedPosition,
}

/// Round 209 — SVG 2 §8.13 `vector-effect` host-coordinate-space
/// suffix. The optional `[ viewport | screen ]?` half of the §8.13
/// grammar names the host coordinate space the constrained
/// transformations are evaluated against. Initial / absent → `Viewport`
/// per the spec ("An initial value in case it is not specified is
/// `viewport`").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VectorEffectHost {
    /// Initial value per §8.13 — host coordinate space is the immediate
    /// viewport coordinate system.
    #[default]
    Viewport,
    /// Host coordinate space is the rootmost content's coordinate
    /// system (the SVG-T 1.2 "screen coordinate space").
    Screen,
}

/// Round 209 — SVG 2 §8.13 `vector-effect` resolved value.
///
/// * [`Self::None`] — initial value; the renderer applies the normal
///   `CTM * (x, y)` coordinate transformation (SVG 1.1 behaviour).
/// * [`Self::Custom`] — one or more effect keywords plus a host
///   coordinate-space suffix. The keyword list is order-preserving and
///   de-duplicated at parse time (each `[ … ]+` keyword may appear at
///   most once per the CSS `|` combinator).
///
/// The property is **not inherited** (§8.13 attribute table:
/// "Inherited: no") — descendants do not pick the value up from an
/// ancestor `<g vector-effect=…>`. Applies to graphics elements and
/// `<use>` per the §8.13 attribute table.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum VectorEffect {
    /// Initial value per §8.13.
    #[default]
    None,
    /// Resolved keyword list with the host coordinate-space suffix.
    Custom {
        keywords: Vec<VectorEffectKeyword>,
        host: VectorEffectHost,
    },
}

impl VectorEffect {
    /// Resolve a non-`none` `vector-effect` payload per the §8.13
    /// grammar: `[ non-scaling-stroke | non-scaling-size | non-rotation
    /// | fixed-position ]+ [ viewport | screen ]?`. Returns `None`
    /// when no recognised keyword is present (caller treats as
    /// initial-value fallback).
    pub(crate) fn parse_custom(value: &str) -> Option<Self> {
        let mut keywords: Vec<VectorEffectKeyword> = Vec::with_capacity(4);
        let mut seen = [false; 4]; // 0=NSStroke, 1=NSSize, 2=NRot, 3=FixedPos
        let mut host: VectorEffectHost = VectorEffectHost::Viewport;
        let mut host_seen = false;
        for tok in value.split_ascii_whitespace() {
            let (kw, idx) = if tok.eq_ignore_ascii_case("non-scaling-stroke") {
                (VectorEffectKeyword::NonScalingStroke, 0)
            } else if tok.eq_ignore_ascii_case("non-scaling-size") {
                (VectorEffectKeyword::NonScalingSize, 1)
            } else if tok.eq_ignore_ascii_case("non-rotation") {
                (VectorEffectKeyword::NonRotation, 2)
            } else if tok.eq_ignore_ascii_case("fixed-position") {
                (VectorEffectKeyword::FixedPosition, 3)
            } else if tok.eq_ignore_ascii_case("viewport") {
                host = VectorEffectHost::Viewport;
                host_seen = true;
                continue;
            } else if tok.eq_ignore_ascii_case("screen") {
                host = VectorEffectHost::Screen;
                host_seen = true;
                continue;
            } else {
                // Unknown token — silently skip, matching the
                // tolerant policy of `paint-order` / `text-anchor` /
                // `visibility` so a future SVG keyword extension
                // doesn't reject the whole document.
                continue;
            };
            if !seen[idx] {
                seen[idx] = true;
                keywords.push(kw);
            }
        }
        if keywords.is_empty() {
            // No effect keyword — the §8.13 grammar requires at least
            // one. A payload of just `viewport` / `screen` / unknown
            // tokens is not a valid `vector-effect` value; caller
            // treats as initial-value fallback (`None`).
            let _ = host_seen;
            return None;
        }
        Some(Self::Custom { keywords, host })
    }

    /// Returns `true` when the effect set contains `non-scaling-stroke`.
    /// Convenience for downstream consumers that only care about the
    /// most common (and only SVG-2-stable) variant.
    pub fn has_non_scaling_stroke(&self) -> bool {
        match self {
            Self::None => false,
            Self::Custom { keywords, .. } => {
                keywords.contains(&VectorEffectKeyword::NonScalingStroke)
            }
        }
    }
}

/// Round 221 — SVG 2 §13.10.2 `shape-rendering` resolved value
/// (`auto | optimizeSpeed | crispEdges | geometricPrecision`).
///
/// Per §13.10.2 the property is a *hint* to the user agent about
/// quality / speed / pixel-snap tradeoffs when rendering vector
/// shapes — it never changes the geometry itself. Values:
///
/// * `Auto` (initial): balance speed / crisp edges / geometric
///   precision, with geometric precision given more importance than
///   the other two.
/// * `OptimizeSpeed`: rendering speed over geometric precision and
///   crisp edges — the UA may turn off anti-aliasing.
/// * `CrispEdges`: emphasise edge contrast over speed and precision —
///   the UA may snap line positions / widths to device pixels.
/// * `GeometricPrecision`: emphasise geometric precision over speed
///   and crisp edges.
///
/// **Inherited:** yes (§13.10.2 attribute table). **Applies to:** the
/// SVG-2 `<shape>` element set per the §13.10.2 attribute table —
/// shapes (`<path>` / `<rect>` / `<circle>` / `<ellipse>` / `<line>` /
/// `<polyline>` / `<polygon>`). Round 221 ships parse + cascade +
/// round-trip preservation; the actual rasteriser hint consumption
/// happens in `oxideav-raster` (which can read the resolved value off
/// the carried [`PaintState`] or off the per-shape
/// [`crate::preserved::ShapeRenderingBinding`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShapeRendering {
    /// Initial value — UA's own balance, with a precision bias.
    #[default]
    Auto,
    /// Prioritise rendering speed over edges and precision; the UA
    /// may disable anti-aliasing.
    OptimizeSpeed,
    /// Prioritise crisp pixel-aligned edges over speed and precision;
    /// the UA may snap edges to device pixels.
    CrispEdges,
    /// Prioritise geometric precision over edge crispness and speed.
    GeometricPrecision,
}

impl ShapeRendering {
    /// Parse a `shape-rendering` keyword (case-insensitive per CSS).
    /// `inherit` returns `None` so the caller can keep the inherited
    /// value already on its `PaintState`. Unknown / malformed tokens
    /// also return `None`, matching the tolerant policy used by
    /// `text-anchor` / `paint-order` / `visibility`.
    pub(crate) fn parse_keyword(value: &str) -> Option<Self> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("auto") {
            Some(Self::Auto)
        } else if v.eq_ignore_ascii_case("optimizespeed") {
            Some(Self::OptimizeSpeed)
        } else if v.eq_ignore_ascii_case("crispedges") {
            Some(Self::CrispEdges)
        } else if v.eq_ignore_ascii_case("geometricprecision") {
            Some(Self::GeometricPrecision)
        } else {
            // `inherit` and unknown tokens fall through; the cascade
            // keeps whatever value was inherited.
            None
        }
    }

    /// Canonicalised lower-camelCase keyword for round-trip emission.
    /// Matches the spec's source-text spelling (camelCase for the
    /// three non-`auto` keywords) so the round-trip is byte-faithful.
    pub(crate) fn as_canonical_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::OptimizeSpeed => "optimizeSpeed",
            Self::CrispEdges => "crispEdges",
            Self::GeometricPrecision => "geometricPrecision",
        }
    }
}

/// Round 228 — SVG 2 §13.10.3 `text-rendering` resolved value
/// (`auto | optimizeSpeed | optimizeLegibility | geometricPrecision`).
///
/// Per §13.10.3 the property is a *hint* to the user agent about the
/// speed / legibility / geometric-precision tradeoff used when
/// rasterising text glyphs — it never alters glyph geometry itself.
/// Values:
///
/// * `Auto` (initial): UA balances speed / legibility / precision with
///   legibility given more importance than the other two.
/// * `OptimizeSpeed`: rendering speed over legibility and precision —
///   the UA may turn off text anti-aliasing.
/// * `OptimizeLegibility`: legibility over speed and precision — the
///   UA may apply anti-aliasing techniques and built-in font hinting.
/// * `GeometricPrecision`: emphasise geometric precision over the
///   other two — the UA usually suspends hinting so glyph outlines
///   are drawn with the same precision as path data.
///
/// **Inherited:** yes (§13.10.3 attribute table). **Applies to:**
/// `<text>` per the §13.10.3 attribute table (with the property
/// cascading down to descendant `<tspan>` / `<textPath>` runs through
/// the normal CSS inheritance). Round 228 ships parse + cascade +
/// round-trip preservation; the actual rendering-hint consumption
/// (anti-alias toggle, hint suspension) happens in `oxideav-raster` /
/// `oxideav-scribe` (which can read the resolved value off the
/// carried [`PaintState`] or off the per-element
/// [`crate::preserved::TextRenderingBinding`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextRendering {
    /// Initial value — UA's own balance, with a legibility bias.
    #[default]
    Auto,
    /// Prioritise rendering speed over legibility and precision; the
    /// UA may disable text anti-aliasing.
    OptimizeSpeed,
    /// Prioritise legibility over speed and precision; the UA may
    /// apply anti-aliasing techniques and built-in font hinting.
    OptimizeLegibility,
    /// Prioritise geometric precision over legibility and speed; the
    /// UA usually suspends hinting so glyph outlines match path data
    /// precision.
    GeometricPrecision,
}

impl TextRendering {
    /// Parse a `text-rendering` keyword (case-insensitive per CSS).
    /// `inherit` returns `None` so the caller can keep the inherited
    /// value already on its `PaintState`. Unknown / malformed tokens
    /// also return `None`, matching the tolerant policy used by
    /// `text-anchor` / `paint-order` / `visibility` / `shape-rendering`.
    pub(crate) fn parse_keyword(value: &str) -> Option<Self> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("auto") {
            Some(Self::Auto)
        } else if v.eq_ignore_ascii_case("optimizespeed") {
            Some(Self::OptimizeSpeed)
        } else if v.eq_ignore_ascii_case("optimizelegibility") {
            Some(Self::OptimizeLegibility)
        } else if v.eq_ignore_ascii_case("geometricprecision") {
            Some(Self::GeometricPrecision)
        } else {
            // `inherit` and unknown tokens fall through; the cascade
            // keeps whatever value was inherited.
            None
        }
    }

    /// Canonicalised lower-camelCase keyword for round-trip emission.
    /// Matches the spec's source-text spelling (camelCase for the
    /// three non-`auto` keywords) so the round-trip is byte-faithful.
    pub(crate) fn as_canonical_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::OptimizeSpeed => "optimizeSpeed",
            Self::OptimizeLegibility => "optimizeLegibility",
            Self::GeometricPrecision => "geometricPrecision",
        }
    }
}

/// Round 247 — SVG 2 §13.10.1 `color-rendering` resolved value
/// (`auto | optimizeSpeed | optimizeQuality`).
///
/// Per §13.10.1 the property is a *hint* to the user agent about the
/// speed-versus-quality tradeoff when performing colour interpolation and
/// compositing operations — it never alters the numeric source colours
/// themselves. The §13.10.1 normative note also makes `color-rendering`
/// take precedence over the (filter-effects) `color-interpolation-filters`
/// property; that interaction is documented but not enforced here because
/// `color-interpolation-filters` lives in the filter primitive graph
/// (round 10 `oxideav-filter` work). Values:
///
/// * `Auto` (initial): UA's own balance, with a quality bias.
/// * `OptimizeSpeed`: rendering speed over quality. The §13.10.1
///   informative note observes that, on an RGB display device, the UA
///   may then perform colour interpolation and compositing in device
///   RGB rather than a linear / wide-gamut working space.
/// * `OptimizeQuality`: emphasise colour-operation quality over speed.
///
/// **Inherited:** yes (§13.10.1 attribute table). **Applies to:** the
/// §13.10.1 applies-to list — container elements, graphics elements,
/// gradient elements, `<use>` and `<animate>`. Round 247 ships parse +
/// inherited cascade + round-trip preservation; the actual
/// colour-interpolation-space selection happens in `oxideav-raster`,
/// which can read the resolved value off the carried [`PaintState`] or
/// off the per-element [`crate::preserved::ColorRenderingBinding`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorRendering {
    /// Initial value — UA's own balance, with a quality bias.
    #[default]
    Auto,
    /// Prioritise speed over quality; the UA may interpolate and
    /// composite in the device colour space.
    OptimizeSpeed,
    /// Prioritise colour-operation quality over speed.
    OptimizeQuality,
}

impl ColorRendering {
    /// Parse a `color-rendering` keyword (case-insensitive per CSS).
    /// `inherit` returns `None` so the caller can keep the inherited
    /// value already on its `PaintState`. Unknown / malformed tokens
    /// also return `None`, matching the tolerant policy used by
    /// `image-rendering` / `text-rendering` / `shape-rendering` /
    /// `text-anchor` / `paint-order` / `visibility`.
    pub(crate) fn parse_keyword(value: &str) -> Option<Self> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("auto") {
            Some(Self::Auto)
        } else if v.eq_ignore_ascii_case("optimizespeed") {
            Some(Self::OptimizeSpeed)
        } else if v.eq_ignore_ascii_case("optimizequality") {
            Some(Self::OptimizeQuality)
        } else {
            // `inherit` and unknown tokens fall through; the cascade
            // keeps whatever value was inherited.
            None
        }
    }

    /// Canonicalised lower-camelCase keyword for round-trip emission.
    /// Matches the spec's source-text spelling (camelCase for the
    /// two non-`auto` keywords) so the round-trip is byte-faithful.
    pub(crate) fn as_canonical_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::OptimizeSpeed => "optimizeSpeed",
            Self::OptimizeQuality => "optimizeQuality",
        }
    }
}

/// Round 252 — SVG 2 §13.9 `color-interpolation` resolved value
/// (`auto | sRGB | linearRGB`).
///
/// Per §13.9 the property selects the working colour space used for the
/// three SVG operations that mix colours componentwise: gradient stop
/// interpolation, SMIL colour animation, and compositing / blending of
/// graphics elements into the current background. Values:
///
/// * `Auto`: UA may pick `sRGB` or `linearRGB`. The author is signalling
///   that any componentwise lerp is acceptable.
/// * `Srgb` (initial): interpolate in the sRGB colour space — the
///   §13.9 initial value. The 1.1 spec calls this out as the default
///   for backwards compatibility.
/// * `LinearRgb`: interpolate in a linearised RGB space (the sRGB
///   electro-optical transfer function inverted per the §13.9 worked
///   example). Produces light-energy-linear blends instead of
///   gamma-encoded ones.
///
/// **Inherited:** yes (§13.9 attribute table). **Applies to:** the §13.9
/// applies-to list — container elements, graphics elements, gradient
/// elements, `<use>` and `<animate>`. Round 252 ships parse + inherited
/// cascade + round-trip preservation; the actual working-space selection
/// for gradient lerps and compositing happens in `oxideav-raster`,
/// which can read the resolved value off the carried [`PaintState`] or
/// off the per-element [`crate::preserved::ColorInterpolationBinding`].
///
/// The §13.9 informative note that the filter-effects sibling property
/// `color-interpolation-filters` governs the filter primitive graph
/// instead is documented but not enforced here — that interaction
/// belongs to the round-10 / round-7 filter graph work in
/// `oxideav-filter`. Note that §13.10.1 `color-rendering` (round 247)
/// is a separate quality hint, not the same property as §13.9
/// `color-interpolation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorInterpolation {
    /// UA-defined choice between `sRGB` and `linearRGB`.
    Auto,
    /// Interpolate in the sRGB colour space — §13.9 initial value.
    Srgb,
    /// Interpolate in a linearised RGB colour space (sRGB EOTF
    /// inverted per the §13.9 worked example).
    LinearRgb,
}

impl Default for ColorInterpolation {
    /// §13.9 attribute table — Initial value: `sRGB`. Distinct from the
    /// other inherited §13.10.x rendering hints whose initial value is
    /// `auto`.
    fn default() -> Self {
        Self::Srgb
    }
}

impl ColorInterpolation {
    /// Parse a `color-interpolation` keyword (case-insensitive per CSS).
    /// `inherit` returns `None` so the caller can keep the inherited
    /// value already on its `PaintState`. Unknown / malformed tokens
    /// also return `None`, matching the tolerant policy used by the
    /// other §13.10.x rendering hints and `text-anchor` / `paint-order`
    /// / `visibility`.
    pub(crate) fn parse_keyword(value: &str) -> Option<Self> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("auto") {
            Some(Self::Auto)
        } else if v.eq_ignore_ascii_case("srgb") {
            Some(Self::Srgb)
        } else if v.eq_ignore_ascii_case("linearrgb") {
            Some(Self::LinearRgb)
        } else {
            // `inherit` and unknown tokens fall through; the cascade
            // keeps whatever value was inherited.
            None
        }
    }

    /// Canonical keyword for round-trip emission. The §13.9 attribute
    /// table spells the non-`auto` keywords with mixed case
    /// (`sRGB`, `linearRGB`); source-text `SRGB` / `srgb` / `LINEARRGB`
    /// all round-trip as the canonical mixed-case spelling so the output
    /// matches §13.9 verbatim.
    pub(crate) fn as_canonical_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Srgb => "sRGB",
            Self::LinearRgb => "linearRGB",
        }
    }
}

/// Round 257 — SVG 2 §3.11 `overflow` property
/// (`visible | hidden | scroll | auto`).
///
/// Per §3.11 the property selects whether a UA establishes a clipping
/// rectangle for an element's content when that content would render
/// outside the element's bounds. The §3.11 normative summary table
/// gives `visible` as the initial value for every SVG element that
/// `overflow` applies to (document-root `<svg>`, other `<svg>`,
/// `<text>`, `<pattern>`, `<marker>`, `<symbol>`, `<image>`,
/// `<iframe>`, `<foreignObject>`); the UA stylesheet additionally
/// overrides the initial value to `hidden` for non-root `<svg>`,
/// `<pattern>`, `<marker>`, `<symbol>`, and `<image>` per the same
/// §3.11 table. Both behaviours fall outside this round — the cascade
/// resolution + side-channel round-trip live here, the UA-stylesheet
/// default + actual clipping-rectangle creation belong to
/// `oxideav-raster` (and the §3.11 informative note that
/// `scroll` may degrade to `hidden` when no scrolling mechanism is
/// available is also a renderer concern, not a parser one).
///
/// **Inherited:** **no** — CSS 2.1 §11.1.1 lists `overflow` as not
/// inherited (mirrors SVG 2 §3.11's "same parameter values and …
/// same meaning as defined in CSS 2.1" cross-reference). So a
/// `<g overflow="hidden">` does NOT push `hidden` down to descendant
/// shapes via the cascade; [`PaintState::merged_with_mctx`] resets
/// `overflow` to the initial value before applying the element's
/// own attribute (matching the `display` / `vector_effect` resets).
/// The §3.11 round-trip side-channel is purely lexical — it captures
/// the source attribute on its own emit slot regardless of
/// cascade — so a hand-authored `<g overflow="hidden">` survives a
/// `parse → write` cycle on the same group element even though the
/// resolved per-shape values would not have picked up `hidden`.
///
/// **Applies to:** the §3.11 summary table's element list (`svg` /
/// `symbol` / `marker` / `pattern` / `image` / `text` / `iframe` /
/// `foreignObject`). We don't enforce the apply-to gate at parse
/// time (consistent with the round-235 `image-rendering` /
/// round-247 `color-rendering` policy of accepting the property on
/// any element so a hand-authored attribute round-trips through any
/// emit slot); the §3.11 normative semantics still only fire when
/// the renderer pulls the resolved value off an applicable element.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Overflow {
    /// `visible` — §3.11 initial value: no clipping rectangle is
    /// established. Per §3.11: "If the overflow property has a value
    /// of 'visible', the property has no effect (i.e., a clipping
    /// rectangle is not created)."
    #[default]
    Visible,
    /// `hidden` — clip to the SVG viewport rectangle for applicable
    /// elements (`<svg>`, `<symbol>`, `<marker>`, `<pattern>`,
    /// `<image>`, `<text>`, `<iframe>`, `<foreignObject>`). Per
    /// §3.11: "If the overflow property has the value 'hidden' or
    /// 'scroll', a clip, the exact size of the SVG viewport is
    /// applied."
    Hidden,
    /// `scroll` — same clip as `hidden`, with a UA scrolling
    /// mechanism (per §3.11 bullet 3, "if the user agent uses a
    /// scrolling mechanism that is visible on the screen … that
    /// mechanism should be displayed for the SVG viewport whether or
    /// not any of its content is clipped"). For the §3.11 summary
    /// table's `<text>`/`<pattern>`/`<marker>`/`<symbol>`/`<image>`
    /// rows, `scroll` resolves to `hidden` (the cells in the table
    /// agree — `scroll` is listed as `hidden`); we still parse and
    /// round-trip the source keyword, the resolution lives in
    /// `oxideav-raster`.
    Scroll,
    /// `auto` — UA's choice between `visible` and `scroll` (per
    /// §3.11 bullet 4: "the value 'auto' implies that all rendered
    /// content for child elements must be visible, either through a
    /// scrolling mechanism, or by rendering with no clip … If the
    /// user agent has no scrolling mechanism, the content would not
    /// be clipped … then the value 'auto' must be treated as
    /// 'visible'"). Parsed and round-tripped here; the UA-side
    /// resolution lives in `oxideav-raster`.
    Auto,
}

impl Overflow {
    /// Parse an `overflow` keyword (case-insensitive per CSS).
    /// `inherit` returns `None` so the caller can keep the inherited
    /// (or, after the §3.11 non-inheritance reset, the initial)
    /// value. Unknown / malformed tokens also return `None`,
    /// matching the tolerant policy used by the §13.x rendering
    /// hints and `text-anchor` / `paint-order` / `visibility`.
    pub(crate) fn parse_keyword(value: &str) -> Option<Self> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("visible") {
            Some(Self::Visible)
        } else if v.eq_ignore_ascii_case("hidden") {
            Some(Self::Hidden)
        } else if v.eq_ignore_ascii_case("scroll") {
            Some(Self::Scroll)
        } else if v.eq_ignore_ascii_case("auto") {
            Some(Self::Auto)
        } else {
            // `inherit` and unknown tokens fall through; the cascade
            // keeps whatever value was inherited (or, since
            // `overflow` is not inherited, the initial `visible` set
            // by the per-element reset in `merged_with_mctx`).
            None
        }
    }

    /// Canonical keyword for round-trip emission. §3.11 reuses the
    /// CSS 2.1 keywords verbatim, all lowercase, so source `HIDDEN`
    /// / `Scroll` round-trip as `hidden` / `scroll`.
    pub(crate) fn as_canonical_str(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::Hidden => "hidden",
            Self::Scroll => "scroll",
            Self::Auto => "auto",
        }
    }
}

/// Round 260 — SVG 2 §15.6 `pointer-events` resolved value
/// (`bounding-box | visiblePainted | visibleFill | visibleStroke |
/// visible | painted | fill | stroke | all | none`).
///
/// Per §15.6 the property selects the circumstances under which an
/// element can be the **target** of a pointer event (mouse click,
/// hover, focus, hyperlink). The keyword set crosses two orthogonal
/// gates:
///
/// 1. **Visibility gate** (`visible*` prefix): only the keywords whose
///    name starts with `visible` (`visiblePainted`, `visibleFill`,
///    `visibleStroke`, `visible`) require the `visibility` property to
///    resolve to `visible`; the bare `painted` / `fill` / `stroke` /
///    `all` keywords ignore visibility, and `none` disables hit testing
///    entirely.
/// 2. **Paint gate** (`*Painted` suffix or bare `painted` / `fill` /
///    `stroke`): some keywords additionally require the corresponding
///    paint to resolve to a value other than `none` — `visiblePainted`
///    and `painted` need either fill or stroke to be painted;
///    `visibleFill` / `fill` only test the interior; `visibleStroke` /
///    `stroke` only test the perimeter; `visible` / `all` ignore the
///    paint properties entirely. The `bounding-box` keyword uses the
///    element's bounding box rather than the geometry, and `none`
///    short-circuits all of the above.
///
/// **Inherited:** yes (§15.6 attribute table). **Applies to:** container
/// elements, graphics elements, `<use>` (per the §15.6 attribute
/// table). The actual hit-testing happens in the interactive layer
/// (e.g. `oxideav-pipeline` event-routing or `oxideav-raster`
/// hit-test queries against the rendered scene); this crate parses,
/// cascades, and round-trips the keyword so the resolved value
/// reaches whichever layer cares.
///
/// Note: §15.6 spells the four `visible*` keywords with lower-camelCase
/// (`visiblePainted`, `visibleFill`, `visibleStroke`) and the
/// `bounding-box` keyword with a hyphen — the canonicalisation map
/// preserves that spelling on round-trip even when the source uses
/// `VISIBLEPAINTED` / `BOUNDING-BOX`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PointerEvents {
    /// `bounding-box` — hit-test the axis-aligned bounding box.
    BoundingBox,
    /// `visiblePainted` (initial) — only painted areas, only when
    /// `visibility: visible`. The §15.6 initial value.
    #[default]
    VisiblePainted,
    /// `visibleFill` — interior only, gated on `visibility: visible`;
    /// `fill: none` does NOT disable hit testing.
    VisibleFill,
    /// `visibleStroke` — perimeter only, gated on `visibility: visible`;
    /// `stroke: none` does NOT disable hit testing.
    VisibleStroke,
    /// `visible` — interior or perimeter, gated on
    /// `visibility: visible`; fill / stroke values are ignored.
    Visible,
    /// `painted` — only painted areas, ignoring `visibility`.
    Painted,
    /// `fill` — interior only, ignoring `visibility` and `fill` value.
    Fill,
    /// `stroke` — perimeter only, ignoring `visibility` and `stroke`
    /// value.
    Stroke,
    /// `all` — interior or perimeter, ignoring `visibility` and fill /
    /// stroke values.
    All,
    /// `none` — element is never a pointer-event target.
    None,
}

impl PointerEvents {
    /// Parse a `pointer-events` keyword (case-insensitive per CSS).
    /// `inherit` returns `None` so the caller can keep the inherited
    /// value already on its `PaintState`. Unknown / malformed tokens
    /// also return `None`, matching the tolerant policy used by the
    /// §13.x rendering hints and `text-anchor` / `paint-order` /
    /// `visibility`.
    pub(crate) fn parse_keyword(value: &str) -> Option<Self> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("bounding-box") {
            Some(Self::BoundingBox)
        } else if v.eq_ignore_ascii_case("visiblepainted") {
            Some(Self::VisiblePainted)
        } else if v.eq_ignore_ascii_case("visiblefill") {
            Some(Self::VisibleFill)
        } else if v.eq_ignore_ascii_case("visiblestroke") {
            Some(Self::VisibleStroke)
        } else if v.eq_ignore_ascii_case("visible") {
            Some(Self::Visible)
        } else if v.eq_ignore_ascii_case("painted") {
            Some(Self::Painted)
        } else if v.eq_ignore_ascii_case("fill") {
            Some(Self::Fill)
        } else if v.eq_ignore_ascii_case("stroke") {
            Some(Self::Stroke)
        } else if v.eq_ignore_ascii_case("all") {
            Some(Self::All)
        } else if v.eq_ignore_ascii_case("none") {
            Some(Self::None)
        } else {
            // `inherit` and unknown tokens fall through; the cascade
            // keeps whatever value was inherited.
            None
        }
    }

    /// Canonical keyword for round-trip emission. §15.6 uses
    /// lower-camelCase for the four `visible*` keywords
    /// (`visiblePainted`, `visibleFill`, `visibleStroke`) and a hyphen
    /// for `bounding-box`; the remaining keywords are all-lowercase.
    /// Source `VISIBLEPAINTED` / `BOUNDING-BOX` round-trip as the
    /// §15.6 canonical spelling.
    pub(crate) fn as_canonical_str(self) -> &'static str {
        match self {
            Self::BoundingBox => "bounding-box",
            Self::VisiblePainted => "visiblePainted",
            Self::VisibleFill => "visibleFill",
            Self::VisibleStroke => "visibleStroke",
            Self::Visible => "visible",
            Self::Painted => "painted",
            Self::Fill => "fill",
            Self::Stroke => "stroke",
            Self::All => "all",
            Self::None => "none",
        }
    }
}

/// Round 261 — SVG 1.1 §16.8.2 `cursor` generic keyword
/// (`auto | crosshair | default | pointer | move | e-resize |
/// ne-resize | nw-resize | n-resize | se-resize | sw-resize |
/// s-resize | w-resize | text | wait | help`).
///
/// §16.8.2 specifies the type of cursor displayed for the pointing
/// device while it hovers the element. The keyword is the *generic*
/// (built-in) cursor that terminates the property's value list; any
/// preceding `<funciri>` items reference custom cursors and live on
/// [`CursorValue::funciris`]. Keyword meanings per §16.8.2: `auto`
/// (initial — the UA picks based on context), `crosshair`, `default`
/// (platform default, often an arrow), `pointer` (indicates a link),
/// `move`, the eight `*-resize` edge keywords (named for the box
/// corner / edge the movement starts from, e.g. `se-resize` for the
/// south-east corner), `text` (often an I-bar), `wait` (watch /
/// hourglass), and `help` (question mark / balloon).
///
/// SVG 2 retains `cursor` as a presentation attribute (Appendix L
/// attribute table) and defers the property definition to CSS; the
/// SVG 1.1 §16.8.2 definition carries the keyword set and cascade
/// rules implemented here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CursorKeyword {
    /// `auto` (initial) — the UA determines the cursor from context.
    #[default]
    Auto,
    /// `crosshair` — short line segments resembling a "+" sign.
    Crosshair,
    /// `default` — the platform-dependent default cursor.
    Default,
    /// `pointer` — indicates a link.
    Pointer,
    /// `move` — indicates something is to be moved.
    Move,
    /// `e-resize` — movement starts from the east edge.
    EResize,
    /// `ne-resize` — movement starts from the north-east corner.
    NeResize,
    /// `nw-resize` — movement starts from the north-west corner.
    NwResize,
    /// `n-resize` — movement starts from the north edge.
    NResize,
    /// `se-resize` — movement starts from the south-east corner.
    SeResize,
    /// `sw-resize` — movement starts from the south-west corner.
    SwResize,
    /// `s-resize` — movement starts from the south edge.
    SResize,
    /// `w-resize` — movement starts from the west edge.
    WResize,
    /// `text` — selectable text; often an I-bar.
    Text,
    /// `wait` — program busy; watch or hourglass.
    Wait,
    /// `help` — help available; question mark or balloon.
    Help,
}

impl CursorKeyword {
    /// Parse a §16.8.2 generic cursor keyword (case-insensitive per
    /// CSS). Returns `None` for `inherit` and unknown / malformed
    /// tokens so the caller can keep the inherited value, matching
    /// the tolerant policy used by the §13.x rendering hints and
    /// `pointer-events`.
    pub(crate) fn parse_keyword(value: &str) -> Option<Self> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("auto") {
            Some(Self::Auto)
        } else if v.eq_ignore_ascii_case("crosshair") {
            Some(Self::Crosshair)
        } else if v.eq_ignore_ascii_case("default") {
            Some(Self::Default)
        } else if v.eq_ignore_ascii_case("pointer") {
            Some(Self::Pointer)
        } else if v.eq_ignore_ascii_case("move") {
            Some(Self::Move)
        } else if v.eq_ignore_ascii_case("e-resize") {
            Some(Self::EResize)
        } else if v.eq_ignore_ascii_case("ne-resize") {
            Some(Self::NeResize)
        } else if v.eq_ignore_ascii_case("nw-resize") {
            Some(Self::NwResize)
        } else if v.eq_ignore_ascii_case("n-resize") {
            Some(Self::NResize)
        } else if v.eq_ignore_ascii_case("se-resize") {
            Some(Self::SeResize)
        } else if v.eq_ignore_ascii_case("sw-resize") {
            Some(Self::SwResize)
        } else if v.eq_ignore_ascii_case("s-resize") {
            Some(Self::SResize)
        } else if v.eq_ignore_ascii_case("w-resize") {
            Some(Self::WResize)
        } else if v.eq_ignore_ascii_case("text") {
            Some(Self::Text)
        } else if v.eq_ignore_ascii_case("wait") {
            Some(Self::Wait)
        } else if v.eq_ignore_ascii_case("help") {
            Some(Self::Help)
        } else {
            // `inherit` and unknown tokens fall through; the cascade
            // keeps whatever value was inherited.
            None
        }
    }

    /// Canonical keyword for round-trip emission. §16.8.2 spells the
    /// whole keyword set in lowercase with hyphens for the eight
    /// `*-resize` keywords; source `POINTER` / `E-RESIZE` round-trip
    /// as `pointer` / `e-resize`.
    pub(crate) fn as_canonical_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Crosshair => "crosshair",
            Self::Default => "default",
            Self::Pointer => "pointer",
            Self::Move => "move",
            Self::EResize => "e-resize",
            Self::NeResize => "ne-resize",
            Self::NwResize => "nw-resize",
            Self::NResize => "n-resize",
            Self::SeResize => "se-resize",
            Self::SwResize => "sw-resize",
            Self::SResize => "s-resize",
            Self::WResize => "w-resize",
            Self::Text => "text",
            Self::Wait => "wait",
            Self::Help => "help",
        }
    }
}

/// Round 261 — SVG 1.1 §16.8.2 `cursor` resolved value:
/// `[ [<funciri> ,]* [ <generic keyword> ] ] | inherit`.
///
/// Per the §16.8.2 value grammar zero or more comma-separated
/// `<funciri>` custom-cursor references precede a single required
/// generic keyword. §16.8.2: "The user agent retrieves the cursor
/// from the resource designated by the URI. If the user agent cannot
/// handle the first cursor of a list of cursors, it shall attempt to
/// handle the second, etc. If the user agent cannot handle any
/// user-defined cursor, it must use the generic cursor at the end of
/// the list" — so the trailing keyword is the mandatory fallback and
/// a funciri list *without* a generic keyword is invalid (the cascade
/// keeps the inherited value).
///
/// **Inherited:** yes (§16.8.2 attribute table). **Applies to:**
/// container elements and graphics elements. The actual cursor
/// display is interactive-UA work (e.g. a windowing host embedding
/// `oxideav-pipeline`); this crate parses, cascades, and round-trips
/// the value so the resolved cursor request reaches whichever layer
/// cares. A `<funciri>` may point at an SVG 1.1 §16.8.3 `<cursor>`
/// element; the reference is carried verbatim (the `<cursor>`
/// element itself is a separate follow-up).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CursorValue {
    /// `<funciri>` custom-cursor references in source order, each
    /// canonicalised to `url(<iri>)` (lowercase `url` token, IRI
    /// preserved verbatim — IRIs are case-significant).
    pub funciris: Vec<String>,
    /// The mandatory trailing generic keyword (§16.8.2 fallback).
    pub keyword: CursorKeyword,
}

impl CursorValue {
    /// Parse a §16.8.2 `cursor` property value. Returns `None` for an
    /// empty payload, the `inherit` keyword, or any grammar violation
    /// (unknown trailing keyword, a non-`<funciri>` item before the
    /// keyword, a funciri list without the mandatory trailing generic
    /// keyword) — the cascade keeps the inherited value in those
    /// cases, matching the tolerant policy of the §13.x rendering
    /// hints and `pointer-events`.
    pub(crate) fn parse_custom(value: &str) -> Option<Self> {
        let v = value.trim();
        if v.is_empty() || v.eq_ignore_ascii_case("inherit") {
            return None;
        }
        // Split on top-level commas only: an IRI inside `url(...)` may
        // itself contain commas (e.g. a data: IRI), which must not
        // split the list.
        let items = split_top_level_commas(v);
        let (last, rest) = items.split_last()?;
        let keyword = CursorKeyword::parse_keyword(last)?;
        let mut funciris = Vec::with_capacity(rest.len());
        for item in rest {
            funciris.push(canonical_funciri(item)?);
        }
        Some(Self { funciris, keyword })
    }

    /// Canonical value string for round-trip emission: funciris in
    /// source order, comma-and-space separated, followed by the
    /// lowercase generic keyword — matching the §16.8.2 example
    /// `cursor : url("mything.cur"), url("second.svg#curs"), text`
    /// list shape (whitespace canonicalised to a single space after
    /// each comma).
    pub(crate) fn as_canonical_string(&self) -> String {
        let mut out = String::new();
        for f in &self.funciris {
            out.push_str(f);
            out.push_str(", ");
        }
        out.push_str(self.keyword.as_canonical_str());
        out
    }
}

/// Round 261 — split `v` on commas that sit outside any parentheses,
/// trimming each piece. `url(data:...,...)` therefore stays one item
/// per the §16.8.2 `<funciri>` production (`url(` wsp* IRI wsp* `)`).
fn split_top_level_commas(v: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in v.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                items.push(v[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    items.push(v[start..].trim());
    items
}

/// Round 261 — validate + canonicalise one `<funciri>` item from a
/// §16.8.2 `cursor` list. The production is `url(` wsp* IRI wsp* `)`
/// with a case-insensitive `url` functional token (per CSS core
/// tokenisation); the canonical form lowercases the token and trims
/// the wsp padding while preserving the IRI verbatim (IRIs are
/// case-significant). Returns `None` for anything that isn't a
/// well-formed funciri — the whole `cursor` value is then rejected
/// and the cascade keeps the inherited value.
fn canonical_funciri(item: &str) -> Option<String> {
    let t = item.trim();
    if t.len() < 5 || !t[..4].eq_ignore_ascii_case("url(") || !t.ends_with(')') {
        return None;
    }
    let inner = t[4..t.len() - 1].trim();
    if inner.is_empty() {
        return None;
    }
    Some(format!("url({})", inner))
}

/// Round 291 — SVG 1.1 §10.9.2 `dominant-baseline` property
/// (`auto | use-script | no-change | reset-size | ideographic |
/// alphabetic | hanging | mathematical | central | middle |
/// text-after-edge | text-before-edge | inherit`).
///
/// Per §10.9.2 the property determines (or re-determines) the
/// *scaled-baseline-table* — a compound value of (baseline-identifier,
/// baseline-table, baseline-table font-size) — that positions the
/// glyphs of a text content element along its inline-progression
/// direction. Some keywords re-determine all three components, others
/// only re-scale the baseline-table font-size:
///
/// * `auto` (initial): on a `<text>` element the dominant baseline is
///   `alphabetic` for a horizontal `writing-mode` and `central` for a
///   vertical one; on a `<tspan>` / `<tref>` / `<altGlyph>` /
///   `<textPath>` the dominant baseline and baseline-table remain those
///   of the parent text content element.
/// * `use-script`: pick the baseline-table from the predominant script
///   of the character data, re-scaled to this element's `font-size`.
/// * `no-change`: keep the parent's dominant baseline, baseline-table,
///   *and* baseline-table font-size unchanged.
/// * `reset-size`: keep the parent's dominant baseline + baseline-table
///   but re-scale the baseline-table font-size to this element's
///   `font-size`.
/// * `ideographic` / `alphabetic` / `hanging` / `mathematical`: set the
///   dominant baseline to the named baseline, derive the baseline-table
///   from that named table in the nominal font, re-scaled to this
///   element's `font-size`.
/// * `central` / `middle` / `text-after-edge` / `text-before-edge`: set
///   the dominant baseline to the named position; the derived
///   baseline-table is constructed from the nominal font's baselines
///   (`central` / `middle` use a fixed priority order over the
///   `ideographic` / `alphabetic` / `hanging` / `mathematical` tables;
///   the two `*-edge` keywords leave the choice implementation-defined),
///   re-scaled to this element's `font-size`.
///
/// **Inherited:** **no** (§10.9.2 attribute table). So a
/// `<text dominant-baseline="hanging">` does NOT push `hanging` down to
/// a nested `<tspan>` through the cascade — [`PaintState::merged_with_mctx`]
/// resets `dominant_baseline` to the initial value before applying the
/// element's own attribute (matching the `display` / `vector_effect` /
/// `overflow` non-inheritance resets). The §10.9.2 prose that a
/// `<tspan>`'s `auto` "remains the same as the parent text content
/// element" is a *baseline-table* computation that lives in the text
/// layout engine (`oxideav-scribe` / `oxideav-raster`), not in the
/// property cascade — the property value itself is non-inherited.
///
/// **Applies to:** text content elements (`<text>`, `<tspan>`,
/// `<tref>`, `<altGlyph>`, `<textPath>`). We don't enforce the
/// applies-to gate at parse time (consistent with the round-235 /
/// round-247 / round-257 policy of accepting the property on any
/// element so a hand-authored attribute round-trips through any emit
/// slot); the §10.9.2 semantics only fire when the text layout engine
/// pulls the resolved value off an applicable element.
///
/// Round 291 ships parse + non-inherited cascade + round-trip
/// preservation; the actual scaled-baseline-table construction +
/// glyph positioning live in `oxideav-scribe` / `oxideav-raster`,
/// which read the resolved value off the carried [`PaintState`] or off
/// the per-element [`crate::preserved::DominantBaselineBinding`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DominantBaseline {
    /// `auto` (initial) — derive from `writing-mode` on `<text>` or
    /// inherit the parent text element's baseline on a child run.
    #[default]
    Auto,
    /// `use-script` — derive from the predominant script of the
    /// character data.
    UseScript,
    /// `no-change` — keep the parent's dominant baseline, table, and
    /// baseline-table font-size.
    NoChange,
    /// `reset-size` — keep the parent's dominant baseline + table but
    /// re-scale the baseline-table font-size to this element.
    ResetSize,
    /// `ideographic` — ideographic baseline-table.
    Ideographic,
    /// `alphabetic` — alphabetic baseline-table.
    Alphabetic,
    /// `hanging` — hanging baseline-table.
    Hanging,
    /// `mathematical` — mathematical baseline-table.
    Mathematical,
    /// `central` — central baseline; derived-table priority order
    /// `ideographic`, `alphabetic`, `hanging`, `mathematical`.
    Central,
    /// `middle` — middle baseline; derived-table priority order
    /// `alphabetic`, `ideographic`, `hanging`, `mathematical`.
    Middle,
    /// `text-after-edge` — text-after-edge baseline (derived-table
    /// choice implementation-defined per §10.9.2).
    TextAfterEdge,
    /// `text-before-edge` — text-before-edge baseline (derived-table
    /// choice implementation-defined per §10.9.2).
    TextBeforeEdge,
}

impl DominantBaseline {
    /// Parse a `dominant-baseline` keyword (case-insensitive per CSS).
    /// `inherit` returns `None` so the caller can keep the inherited
    /// (or, after the §10.9.2 non-inheritance reset, the initial)
    /// value. Unknown / malformed tokens also return `None`, matching
    /// the tolerant policy used by the §13.x rendering hints and
    /// `text-anchor` / `paint-order` / `visibility` / `overflow`.
    pub(crate) fn parse_keyword(value: &str) -> Option<Self> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("auto") {
            Some(Self::Auto)
        } else if v.eq_ignore_ascii_case("use-script") {
            Some(Self::UseScript)
        } else if v.eq_ignore_ascii_case("no-change") {
            Some(Self::NoChange)
        } else if v.eq_ignore_ascii_case("reset-size") {
            Some(Self::ResetSize)
        } else if v.eq_ignore_ascii_case("ideographic") {
            Some(Self::Ideographic)
        } else if v.eq_ignore_ascii_case("alphabetic") {
            Some(Self::Alphabetic)
        } else if v.eq_ignore_ascii_case("hanging") {
            Some(Self::Hanging)
        } else if v.eq_ignore_ascii_case("mathematical") {
            Some(Self::Mathematical)
        } else if v.eq_ignore_ascii_case("central") {
            Some(Self::Central)
        } else if v.eq_ignore_ascii_case("middle") {
            Some(Self::Middle)
        } else if v.eq_ignore_ascii_case("text-after-edge") {
            Some(Self::TextAfterEdge)
        } else if v.eq_ignore_ascii_case("text-before-edge") {
            Some(Self::TextBeforeEdge)
        } else {
            // `inherit` and unknown tokens fall through; the cascade
            // keeps whatever value was inherited (or, since
            // `dominant-baseline` is not inherited, the initial `auto`
            // set by the per-element reset in `merged_with_mctx`).
            None
        }
    }

    /// Canonical keyword for round-trip emission. §10.9.2 spells every
    /// keyword all-lowercase (hyphenated for the multi-word ones), so
    /// source `HANGING` / `TEXT-AFTER-EDGE` round-trip as `hanging` /
    /// `text-after-edge`.
    pub(crate) fn as_canonical_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::UseScript => "use-script",
            Self::NoChange => "no-change",
            Self::ResetSize => "reset-size",
            Self::Ideographic => "ideographic",
            Self::Alphabetic => "alphabetic",
            Self::Hanging => "hanging",
            Self::Mathematical => "mathematical",
            Self::Central => "central",
            Self::Middle => "middle",
            Self::TextAfterEdge => "text-after-edge",
            Self::TextBeforeEdge => "text-before-edge",
        }
    }
}

/// Round 235 — SVG 2 §13.10.4 `image-rendering` resolved value
/// (`auto | optimizeQuality | optimizeSpeed`).
///
/// Per §13.10.4 the property is a *hint* to the user agent about the
/// speed-versus-quality tradeoff when sampling a raster `<image>` into
/// vector space — it never alters the source bytes themselves. Values:
///
/// * `Auto` (initial): UA's own balance, with quality given more
///   importance than speed. The §13.10.4 normative text requires the
///   resampling algorithm to be at least as good as nearest-neighbour,
///   with bilinear strongly preferred.
/// * `OptimizeQuality`: emphasise quality over speed; resampling at
///   least as good as bilinear.
/// * `OptimizeSpeed`: emphasise speed over quality; resampling at
///   least as good as nearest-neighbour, with a UA free to upgrade
///   if a higher-quality algorithm meets the performance goal.
///
/// **Inherited:** yes (§13.10.4 attribute table). **Applies to:** the
/// `images` category (`<image>`, plus any element that paints raster
/// content through a `<filter>` `<feImage>` or a `<pattern>` carrying
/// raster children — for round 235 only `<image>` is captured).
/// Round 235 ships parse + cascade + round-trip preservation; the
/// actual resampling-algorithm selection lives in `oxideav-raster`,
/// which can read the resolved value off the carried [`PaintState`]
/// or off the per-image [`crate::image::SvgImage::image_rendering`]
/// field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ImageRendering {
    /// Initial value — UA's own balance, with a quality bias.
    #[default]
    Auto,
    /// Prioritise quality over speed; resampling at least as good as
    /// bilinear.
    OptimizeQuality,
    /// Prioritise speed over quality; resampling at least as good as
    /// nearest-neighbour, with a UA free to upgrade.
    OptimizeSpeed,
}

impl ImageRendering {
    /// Parse an `image-rendering` keyword (case-insensitive per CSS).
    /// `inherit` returns `None` so the caller can keep the inherited
    /// value already on its `PaintState`. Unknown / malformed tokens
    /// also return `None`, matching the tolerant policy used by
    /// `text-rendering` / `shape-rendering` / `text-anchor` /
    /// `paint-order` / `visibility`.
    pub(crate) fn parse_keyword(value: &str) -> Option<Self> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("auto") {
            Some(Self::Auto)
        } else if v.eq_ignore_ascii_case("optimizequality") {
            Some(Self::OptimizeQuality)
        } else if v.eq_ignore_ascii_case("optimizespeed") {
            Some(Self::OptimizeSpeed)
        } else {
            // `inherit` and unknown tokens fall through; the cascade
            // keeps whatever value was inherited.
            None
        }
    }

    /// Canonicalised lower-camelCase keyword for round-trip emission.
    /// Matches the spec's source-text spelling (camelCase for the
    /// two non-`auto` keywords) so the round-trip is byte-faithful.
    pub(crate) fn as_canonical_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::OptimizeQuality => "optimizeQuality",
            Self::OptimizeSpeed => "optimizeSpeed",
        }
    }
}

/// Round 187 — SVG 2 §11.2.1 `lengthAdjust` attribute on
/// `<text>` / `<tspan>` (`spacing | spacingAndGlyphs`). Selects how a
/// `textLength`-driven width adjustment is distributed across the run:
///
/// - [`Self::Spacing`] (initial): adjust only the **inter-glyph
///   advances**; the glyph outlines themselves are not stretched.
/// - [`Self::SpacingAndGlyphs`]: scale glyph advances **and** the
///   glyph outlines along the inline-base direction so the run
///   visibly stretches / compresses.
///
/// The attribute is NOT inherited (it applies only to the element
/// that carries it; see §11.2.1 attribute table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextLengthAdjust {
    /// Initial value per §11.2.1.
    #[default]
    Spacing,
    SpacingAndGlyphs,
}

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
    /// Round 118 — SVG 1.1 §11.5 `display`. `false` means
    /// `display: none` was resolved on *this* element. `display` is
    /// NOT inherited (Inherited: no), so [`PaintState::merged_with_mctx`]
    /// resets it to `true` before applying the element's own value —
    /// a child of a `display:none` ancestor would never be reached
    /// anyway because the ancestor is dropped from the rendering tree.
    pub display: bool,
    /// Round 118 — SVG 1.1 §11.5 `visibility` (inherited).
    pub visibility: Visibility,
    /// Round 172 — SVG 2 §11.10.1.1 `text-anchor` (inherited).
    /// Initial value `start`. Consumed by [`crate::text`] when laying
    /// glyphs for `<text>` / `<tspan>` / `<textPath>` runs.
    pub text_anchor: TextAnchor,
    /// Round 205 — SVG 2 §13.8 `paint-order` (inherited).
    /// Initial value `Normal` (fill, then stroke, then markers).
    /// Consumed by the shape branch in
    /// [`parse_element_to_node_ctx`] — when the resolved order would
    /// paint the stroke before the fill, the shape emits two
    /// single-purpose `PathNode`s in a wrapping `Group` so the scene
    /// graph composites correctly under the round-1
    /// `oxideav_core::Node::Path { fill, stroke }` model (which has
    /// no built-in operation-order field).
    pub paint_order: PaintOrder,
    /// Round 209 — SVG 2 §8.13 `vector-effect` (NOT inherited).
    /// Initial value [`VectorEffect::None`]. Applies to graphics
    /// elements and `<use>` per the §8.13 attribute table; a
    /// `<g vector-effect=…>` ancestor does NOT push the property to
    /// child shapes (cf. the `display` reset in
    /// [`Self::merged_with_mctx`]). The actual transform suppression
    /// happens in `oxideav-raster`; this crate parses + round-trips
    /// the property and exposes it on the resolved [`PaintState`].
    pub vector_effect: VectorEffect,
    /// Round 221 — SVG 2 §13.10.2 `shape-rendering` (inherited).
    /// Initial value [`ShapeRendering::Auto`]. The actual hint
    /// consumption (e.g. anti-aliasing toggle, edge snap) happens in
    /// `oxideav-raster`; this crate parses + cascades + round-trips
    /// the keyword.
    pub shape_rendering: ShapeRendering,
    /// Round 228 — SVG 2 §13.10.3 `text-rendering` (inherited).
    /// Initial value [`TextRendering::Auto`]. The actual hint
    /// consumption (anti-alias toggle, hint suspension) happens in
    /// `oxideav-raster` / `oxideav-scribe`; this crate parses,
    /// cascades, and round-trips the keyword.
    pub text_rendering: TextRendering,
    /// Round 235 — SVG 2 §13.10.4 `image-rendering` (inherited).
    /// Initial value [`ImageRendering::Auto`]. The actual hint
    /// consumption (resampling-algorithm selection) happens in
    /// `oxideav-raster`; this crate parses, cascades, and round-trips
    /// the keyword.
    pub image_rendering: ImageRendering,
    /// Round 247 — SVG 2 §13.10.1 `color-rendering` (inherited).
    /// Initial value [`ColorRendering::Auto`]. The actual hint
    /// consumption (working colour-space selection for interpolation
    /// and compositing) happens in `oxideav-raster`; this crate parses,
    /// cascades, and round-trips the keyword.
    pub color_rendering: ColorRendering,
    /// Round 252 — SVG 2 §13.9 `color-interpolation` (inherited).
    /// Initial value [`ColorInterpolation::Srgb`] (per the §13.9 attribute
    /// table — NOT `auto`, unlike the §13.10.x rendering hints). The
    /// actual working-space selection for gradient lerps / colour
    /// animation / compositing happens in `oxideav-raster`; this crate
    /// parses, cascades, and round-trips the keyword.
    pub color_interpolation: ColorInterpolation,
    /// Round 257 — SVG 2 §3.11 `overflow` (NOT inherited per CSS
    /// 2.1). Initial value [`Overflow::Visible`]. The actual
    /// clipping-rectangle establishment + UA-stylesheet override of
    /// the initial value to `hidden` for non-root `<svg>` /
    /// `<symbol>` / `<marker>` / `<pattern>` / `<image>` happen in
    /// `oxideav-raster`; this crate parses + resets per-element +
    /// round-trips the source attribute via the
    /// [`crate::preserved::OverflowBinding`] side-channel.
    pub overflow: Overflow,
    /// Round 260 — SVG 2 §15.6 `pointer-events` (inherited). Initial
    /// value [`PointerEvents::VisiblePainted`]. The actual hit-test
    /// gating happens in the interactive layer (e.g.
    /// `oxideav-pipeline` event routing or `oxideav-raster`
    /// hit-query); this crate parses, cascades, and round-trips the
    /// keyword via the [`crate::preserved::PointerEventsBinding`]
    /// side-channel.
    pub pointer_events: PointerEvents,
    /// Round 261 — SVG 1.1 §16.8.2 `cursor` (inherited). Initial
    /// value: empty funciri list + [`CursorKeyword::Auto`]. The
    /// actual cursor display is interactive-UA work (a windowing
    /// host embedding `oxideav-pipeline`); this crate parses,
    /// cascades, and round-trips the value via the
    /// [`crate::preserved::CursorBinding`] side-channel.
    pub cursor: CursorValue,
    /// Round 291 — SVG 1.1 §10.9.2 `dominant-baseline` (NOT inherited
    /// per the §10.9.2 attribute table). Initial value
    /// [`DominantBaseline::Auto`]. The actual scaled-baseline-table
    /// construction + glyph positioning happen in `oxideav-scribe` /
    /// `oxideav-raster`; this crate parses + resets per-element +
    /// round-trips the source attribute via the
    /// [`crate::preserved::DominantBaselineBinding`] side-channel.
    pub dominant_baseline: DominantBaseline,
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
            // §11.5 — `display` initial is `inline` (rendered);
            // `visibility` initial is `visible`.
            display: true,
            visibility: Visibility::Visible,
            // §11.10.1.1 — initial value `start`.
            text_anchor: TextAnchor::Start,
            // §13.8 — initial value `normal`.
            paint_order: PaintOrder::Normal,
            // §8.13 — initial value `none`.
            vector_effect: VectorEffect::None,
            // §13.10.2 — initial value `auto`.
            shape_rendering: ShapeRendering::Auto,
            // §13.10.3 — initial value `auto`.
            text_rendering: TextRendering::Auto,
            // §13.10.4 — initial value `auto`.
            image_rendering: ImageRendering::Auto,
            // §13.10.1 — initial value `auto`.
            color_rendering: ColorRendering::Auto,
            // §13.9 — initial value `sRGB`. NOT `auto`; the §13.9
            // attribute table specifies `sRGB` as the initial value so
            // a UA without an explicit `color-interpolation=` attribute
            // performs gradient / animation / compositing lerps in
            // sRGB by default.
            color_interpolation: ColorInterpolation::Srgb,
            // §3.11 — initial value `visible` per the summary table.
            // CSS 2.1 §11.1.1 lists `overflow` as not inherited, so
            // `merged_with_mctx` resets this to the initial value
            // before applying the element's own attribute.
            overflow: Overflow::Visible,
            // §15.6 — initial value `visiblePainted` per the attribute
            // table. The property IS inherited, so a child without its
            // own `pointer-events=` picks up the parent's resolved
            // value (no per-element reset like `display` /
            // `vector-effect` / `overflow`).
            pointer_events: PointerEvents::VisiblePainted,
            // §16.8.2 — initial value `auto` (no custom funciris).
            // The property IS inherited per the attribute table, so a
            // child without its own `cursor=` picks up the parent's
            // resolved value through the clone in `merged_with_mctx`.
            cursor: CursorValue::default(),
            // §10.9.2 — initial value `auto`. The property is NOT
            // inherited per the §10.9.2 attribute table, so
            // `merged_with_mctx` resets this to the initial value
            // before applying the element's own attribute (matching
            // the `display` / `vector-effect` / `overflow` resets).
            dominant_baseline: DominantBaseline::Auto,
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
        // Round 118 — `display` is NOT inherited (SVG 1.1 §11.5,
        // Inherited: no). Reset it to the initial `true` before the
        // element's own `display` (if any) is applied below. Without
        // the reset, a `<g display="none">` parent's state would
        // (incorrectly) carry `display:false` into a child we *do*
        // reach via a `<use>` of the inner element. `visibility` is
        // inherited, so it is left as cloned from `self`.
        s.display = true;
        // Round 209 — `vector-effect` is NOT inherited (SVG 2 §8.13,
        // Inherited: no). Reset to the initial value before the
        // element's own `vector-effect` (if any) is applied below, so
        // a child of a `<g vector-effect="non-scaling-stroke">` does
        // NOT silently pick the property up from the ancestor — only
        // an explicit `vector-effect=` on the child itself sets it.
        s.vector_effect = VectorEffect::None;
        // Round 257 — `overflow` is NOT inherited (SVG 2 §3.11
        // cross-references CSS 2.1 §11.1.1, which lists `overflow`
        // with Inherited: no). Reset to the initial value
        // [`Overflow::Visible`] before the element's own attribute
        // (if any) is applied below, matching the `display` /
        // `vector_effect` reset policy. A `<g overflow="hidden">`
        // ancestor therefore does NOT push `hidden` onto descendant
        // shapes via the cascade — the round-trip side-channel
        // captures the source attribute on its own emit slot instead
        // (so the author's intent still survives a `parse → write`).
        s.overflow = Overflow::Visible;
        // Round 291 — `dominant-baseline` is NOT inherited (SVG 1.1
        // §10.9.2, Inherited: no). Reset to the initial value
        // [`DominantBaseline::Auto`] before the element's own attribute
        // (if any) is applied below, matching the `display` /
        // `vector_effect` / `overflow` reset policy. A
        // `<text dominant-baseline="hanging">` ancestor therefore does
        // NOT push `hanging` onto a nested `<tspan>` via the property
        // cascade — the §10.9.2 baseline-table inheritance for a child
        // run's `auto` is a layout-engine computation, distinct from
        // the (non-inherited) property value. The round-trip
        // side-channel captures the source attribute on its own emit
        // slot instead.
        s.dominant_baseline = DominantBaseline::Auto;
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
            // Round 118 — SVG 1.1 §11.5 `display`. A value of `none`
            // removes the element + its children from the rendering
            // tree; `inherit` keeps the (already-reset) inherited
            // value; any other CSS2 display keyword (`inline`,
            // `block`, `list-item`, table-*, …) means "rendered".
            // Unknown / malformed values fall through to "rendered"
            // rather than failing the document.
            "display" => {
                let v = value.trim();
                if v.eq_ignore_ascii_case("none") {
                    s.display = false;
                } else if v.eq_ignore_ascii_case("inherit") {
                    // Leave `s.display` as-is. (`merged_with_mctx`
                    // reset it to the initial `true` before this loop;
                    // `inherit` therefore behaves like the default.)
                } else {
                    s.display = true;
                }
            }
            // Round 118 — SVG 1.1 §11.5 `visibility`
            // (`visible | hidden | collapse | inherit`). `hidden` and
            // `collapse` both make the graphics element invisible;
            // `inherit` keeps the inherited value already in `s`.
            "visibility" => {
                let v = value.trim();
                if v.eq_ignore_ascii_case("visible") {
                    s.visibility = Visibility::Visible;
                } else if v.eq_ignore_ascii_case("hidden") || v.eq_ignore_ascii_case("collapse") {
                    s.visibility = Visibility::Hidden;
                } else {
                    // `inherit` or unrecognised — keep inherited value.
                }
            }
            // Round 172 — SVG 2 §11.10.1.1 `text-anchor`
            // (`start | middle | end`). Inherited; `inherit` /
            // unrecognised values keep the inherited value already in
            // `s`. Unknown keywords are tolerated rather than failing
            // the document (matches the round-118 visibility branch).
            "text-anchor" => {
                let v = value.trim();
                if v.eq_ignore_ascii_case("start") {
                    s.text_anchor = TextAnchor::Start;
                } else if v.eq_ignore_ascii_case("middle") {
                    s.text_anchor = TextAnchor::Middle;
                } else if v.eq_ignore_ascii_case("end") {
                    s.text_anchor = TextAnchor::End;
                } else {
                    // `inherit` or unrecognised — keep inherited value.
                }
            }
            // Round 205 — SVG 2 §13.8 `paint-order`
            // (`normal | [ fill || stroke || markers ]`). Inherited.
            // `normal` (or `inherit`) keeps / resets to the initial.
            // Otherwise resolve the keyword list per the §13.8 grammar
            // — unknown / missing tokens are tolerated the same way
            // the visibility / text-anchor branches handle them.
            "paint-order" => {
                let v = value.trim();
                if v.is_empty() || v.eq_ignore_ascii_case("normal") {
                    s.paint_order = PaintOrder::Normal;
                } else if v.eq_ignore_ascii_case("inherit") {
                    // Keep inherited value already in `s`.
                } else if let Some(order) = PaintOrder::parse_custom(v) {
                    s.paint_order = order;
                } else {
                    // No recognised keyword at all — fall back to the
                    // initial value rather than failing the document.
                    s.paint_order = PaintOrder::Normal;
                }
            }
            // Round 209 — SVG 2 §8.13 `vector-effect`
            // (`none | [ non-scaling-stroke | non-scaling-size |
            // non-rotation | fixed-position ]+ [ viewport | screen ]?`).
            // NOT inherited (the reset above clears the field to the
            // initial value before this loop runs). `inherit` is
            // tolerated but, because the `merged_with_mctx` reset
            // already cleared the inherited value, `inherit` produces
            // the initial value (which is what a CSS-compliant UA
            // would also produce when the inherited value is its own
            // initial). Unknown / malformed payloads fall back to the
            // initial rather than failing the document.
            "vector-effect" => {
                let v = value.trim();
                if v.is_empty() || v.eq_ignore_ascii_case("none") {
                    s.vector_effect = VectorEffect::None;
                } else if v.eq_ignore_ascii_case("inherit") {
                    // Initial-value fallback per the note above.
                    s.vector_effect = VectorEffect::None;
                } else if let Some(ve) = VectorEffect::parse_custom(v) {
                    s.vector_effect = ve;
                } else {
                    s.vector_effect = VectorEffect::None;
                }
            }
            // Round 221 — SVG 2 §13.10.2 `shape-rendering` (inherited).
            // `auto | optimizeSpeed | crispEdges | geometricPrecision`.
            // `inherit` keeps the inherited value already in `s` (the
            // property IS inherited so the value flowed in from the
            // cloned parent state at the top of `merged_with_mctx`).
            // Unknown / malformed tokens also keep the inherited value,
            // matching the tolerant policy of `text-anchor` /
            // `visibility` / `paint-order` — a future spec addition
            // (e.g. a `crispEdges` synonym) won't break documents.
            "shape-rendering" => {
                if let Some(sr) = ShapeRendering::parse_keyword(value) {
                    s.shape_rendering = sr;
                }
            }
            // Round 228 — SVG 2 §13.10.3 `text-rendering` (inherited).
            // `auto | optimizeSpeed | optimizeLegibility |
            // geometricPrecision`. Same tolerant-keep-on-`inherit`-or-
            // unknown policy as `shape-rendering` above.
            "text-rendering" => {
                if let Some(tr) = TextRendering::parse_keyword(value) {
                    s.text_rendering = tr;
                }
            }
            // Round 235 — SVG 2 §13.10.4 `image-rendering` (inherited).
            // `auto | optimizeQuality | optimizeSpeed`. Same tolerant-
            // keep-on-`inherit`-or-unknown policy as the round-221 /
            // round-228 rendering hints above. Per §13.10.4 the
            // property applies to `images`; we accept it on any
            // element so a hand-authored `<g image-rendering=…>`
            // cascades down to descendant `<image>` runs through the
            // normal CSS inheritance, matching the spec note that the
            // property cascades like other inherited properties even
            // when an interior element doesn't itself paint a raster.
            "image-rendering" => {
                if let Some(ir) = ImageRendering::parse_keyword(value) {
                    s.image_rendering = ir;
                }
            }
            // Round 247 — SVG 2 §13.10.1 `color-rendering` (inherited).
            // `auto | optimizeSpeed | optimizeQuality`. Same tolerant-
            // keep-on-`inherit`-or-unknown policy as the round-221 /
            // round-228 / round-235 rendering hints above. §13.10.1
            // applies to container / graphics / gradient elements,
            // `<use>` and `<animate>`; the cascade itself is uniform
            // (the property simply propagates), so we accept it on any
            // element and let the inheritance flow do its work.
            "color-rendering" => {
                if let Some(cr) = ColorRendering::parse_keyword(value) {
                    s.color_rendering = cr;
                }
            }
            // Round 252 — SVG 2 §13.9 `color-interpolation` (inherited).
            // `auto | sRGB | linearRGB`. Same tolerant-keep-on-`inherit`-
            // or-unknown policy as the §13.10.x rendering hints. §13.9
            // applies to container / graphics / gradient elements,
            // `<use>` and `<animate>`; the cascade itself is uniform
            // (the property simply propagates), so we accept it on any
            // element and let inheritance flow. Note: §13.9 is the
            // working-colour-space selector for gradient / animation /
            // compositing lerps — distinct from the §13.10.1
            // `color-rendering` quality hint above.
            "color-interpolation" => {
                if let Some(ci) = ColorInterpolation::parse_keyword(value) {
                    s.color_interpolation = ci;
                }
            }
            // Round 257 — SVG 2 §3.11 `overflow` (NOT inherited per
            // CSS 2.1). `visible | hidden | scroll | auto`. Same
            // tolerant-keep-on-`inherit`-or-unknown policy as the
            // §13.x rendering hints above. §3.11 lists `<svg>` /
            // `<symbol>` / `<marker>` / `<pattern>` / `<image>` /
            // `<text>` / `<iframe>` / `<foreignObject>` as the
            // elements the property applies to; we accept the
            // keyword on any element so a hand-authored attribute
            // resolves into the carried PaintState even when the
            // renderer's applies-to gate would later ignore it.
            "overflow" => {
                if let Some(o) = Overflow::parse_keyword(value) {
                    s.overflow = o;
                }
            }
            // Round 260 — SVG 2 §15.6 `pointer-events` (inherited).
            // `bounding-box | visiblePainted | visibleFill |
            // visibleStroke | visible | painted | fill | stroke | all |
            // none`. Same tolerant-keep-on-`inherit`-or-unknown policy
            // as the §13.x rendering hints — a future spec addition or
            // an upstream typo won't break documents. §15.6 lists
            // container / graphics elements + `<use>` as the applies-to
            // set; we accept the keyword on any element so a
            // hand-authored attribute resolves into the carried
            // PaintState even when the renderer's applies-to gate would
            // later ignore it (matches the round-247 / round-252 /
            // round-257 policy).
            "pointer-events" => {
                if let Some(pe) = PointerEvents::parse_keyword(value) {
                    s.pointer_events = pe;
                }
            }
            // Round 261 — SVG 1.1 §16.8.2 `cursor` (inherited).
            // `[ [<funciri> ,]* [ auto | crosshair | default |
            // pointer | move | e-resize | ne-resize | nw-resize |
            // n-resize | se-resize | sw-resize | s-resize | w-resize |
            // text | wait | help ] ] | inherit`. Same tolerant
            // keep-on-`inherit`-or-invalid policy as the §13.x
            // rendering hints and `pointer-events` above. §16.8.2
            // lists container + graphics elements as the applies-to
            // set; we accept the value on any element so a
            // hand-authored attribute resolves into the carried
            // PaintState even when the renderer's applies-to gate
            // would later ignore it.
            "cursor" => {
                if let Some(c) = CursorValue::parse_custom(value) {
                    s.cursor = c;
                }
            }
            // Round 291 — SVG 1.1 §10.9.2 `dominant-baseline` (NOT
            // inherited). `auto | use-script | no-change | reset-size |
            // ideographic | alphabetic | hanging | mathematical |
            // central | middle | text-after-edge | text-before-edge`.
            // Same tolerant-keep-on-`inherit`-or-unknown policy as the
            // §13.x rendering hints / `overflow` above. §10.9.2 lists
            // text content elements (`<text>` / `<tspan>` / `<tref>` /
            // `<altGlyph>` / `<textPath>`) as the applies-to set; we
            // accept the keyword on any element so a hand-authored
            // attribute resolves into the carried PaintState even when
            // the layout engine's applies-to gate would later ignore it.
            "dominant-baseline" => {
                if let Some(db) = DominantBaseline::parse_keyword(value) {
                    s.dominant_baseline = db;
                }
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
    /// Round 403 — running total of `<use>` instantiations performed
    /// during this decode. The path-based [`Self::use_stack`] prevents a
    /// self-referential *cycle*, but not a *diamond*: a `<g id="a">` that
    /// references `<g id="b">` twice, where `b` references `c` twice, and
    /// so on, expands 2ⁿ nodes with no id ever appearing twice on the
    /// instantiation path. This global counter caps the total expansion
    /// at [`MAX_USE_EXPANSIONS`], turning that exponential blow-up into a
    /// bounded, quickly-terminating decode.
    pub use_expansions: usize,
    /// Round 403 — current model-builder recursion depth. Unlike the
    /// parse-time [`crate::parser::MAX_XML_DEPTH`] guard (which bounds
    /// the *lexical* nesting of the source tree), this bounds the
    /// *decode* recursion — which can run far deeper than the XML tree
    /// through `<use>` chains: a flat list of groups where `#n0` uses
    /// `#n1` uses `#n2` … instantiates a decode stack as deep as the
    /// chain even though the XML nests only two levels. Capped at
    /// [`MAX_RENDER_DEPTH`].
    pub render_depth: usize,
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
    /// Round 21 — collected `(scene_path, pathLength)` mappings.
    /// Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (the same gate that
    /// enables [`Self::track_id_paths`]). Each entry records the
    /// author-supplied `pathLength` for a shape so the encoder can
    /// re-emit the attribute on round-trip.
    pub path_lengths: Vec<crate::preserved::PathLengthBinding>,
    /// Round 21 — scratch slot used by the shape branch to hand the
    /// parsed `pathLength` over to the wrapper-aware recorder that
    /// runs after `apply_referenced_defs`. Always cleared at the end
    /// of each `parse_element_to_node_ctx` call.
    pub pending_path_length: Option<f32>,
    /// Round 98 — SVG 2 §5.7.5 "language tags indicated by user
    /// preferences", consulted by `<switch>` conditional processing
    /// (§5.7.3) when it evaluates a child's `systemLanguage` attribute.
    /// oxideav owns no user-agent locale registry, so the caller
    /// supplies the list (default empty — a present, non-empty
    /// `systemLanguage` then matches nothing, while an absent attribute
    /// still implicitly evaluates to true per §5.7.5).
    pub system_language: Vec<String>,
    /// Round 115 — collected `(scene_path, <a> hyperlink)` mappings
    /// (SVG 2 §16.5). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]); the encoder re-wraps each recorded
    /// `Node::Group` in its `<a href="…">…</a>` element on round-trip.
    pub links: Vec<crate::preserved::LinkBinding>,
    /// Round 122 — collected `(parent_scene_path, [<title>])` bindings
    /// (SVG 2 §5.8). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). One entry per container that carries
    /// at least one `<title>` child; the encoder re-emits the list as
    /// the first children of the matching `<g>` / root `<svg>` on
    /// round-trip.
    pub titles: Vec<crate::preserved::DescriptiveBinding>,
    /// Round 122 — collected `(parent_scene_path, [<desc>])` bindings
    /// (SVG 2 §5.8). Same gate + layout as [`Self::titles`].
    pub descs: Vec<crate::preserved::DescriptiveBinding>,
    /// Round 205 — SVG 2 §13.8 `paint-order` bindings collected during
    /// the build walk. Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder re-emits the source
    /// attribute on the matching `<path>` / `<rect>` / `<circle>` /
    /// `<ellipse>` / `<line>` / `<polyline>` / `<polygon>` on
    /// round-trip.
    pub paint_orders: Vec<crate::preserved::PaintOrderBinding>,
    /// Round 205 — scratch slot for the shape branch to hand a
    /// resolved `paint-order` keyword string over to the
    /// wrapper-aware recorder that runs after `apply_referenced_defs`.
    /// Always cleared at the end of each `parse_element_to_node_ctx`
    /// call.
    pub pending_paint_order: Option<String>,
    /// Round 209 — collected `(scene_path, vector-effect)` bindings
    /// (SVG 2 §8.13). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder re-emits the source
    /// attribute on the matching shape / `<use>` group on round-trip.
    pub vector_effects: Vec<crate::preserved::VectorEffectBinding>,
    /// Round 209 — scratch slot for the shape / `<use>` branch to hand
    /// a captured `vector-effect` keyword string to the wrapper-aware
    /// recorder that runs after `apply_referenced_defs`. Cleared at
    /// the end of each `parse_element_to_node_ctx` call (the drain
    /// matches the round-205 `pending_paint_order` flow).
    pub pending_vector_effect: Option<String>,
    /// Round 221 — collected `(scene_path, shape-rendering)` bindings
    /// (SVG 2 §13.10.2). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder re-emits the source
    /// attribute on the matching shape / `<g>` on round-trip.
    pub shape_renderings: Vec<crate::preserved::ShapeRenderingBinding>,
    /// Round 221 — scratch slot for the shape / `<g>` branch to hand a
    /// captured `shape-rendering` keyword string to the wrapper-aware
    /// recorder that runs after `apply_referenced_defs`. Cleared at
    /// the end of each `parse_element_to_node_ctx` call (the drain
    /// matches the round-205 `pending_paint_order` flow).
    pub pending_shape_rendering: Option<String>,
    /// Round 228 — collected `(scene_path, text-rendering)` bindings
    /// (SVG 2 §13.10.3). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder re-emits the source
    /// attribute on the matching `<text>` / `<g>` on round-trip.
    pub text_renderings: Vec<crate::preserved::TextRenderingBinding>,
    /// Round 228 — scratch slot for the `<text>` / `<g>` branch to
    /// hand a captured `text-rendering` keyword string to the
    /// wrapper-aware recorder that runs after `apply_referenced_defs`.
    /// Cleared at the end of each `parse_element_to_node_ctx` call
    /// (the drain matches the round-221 `pending_shape_rendering` flow).
    pub pending_text_rendering: Option<String>,
    /// Round 247 — collected `(scene_path, color-rendering)` bindings
    /// (SVG 2 §13.10.1). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder re-emits the source
    /// attribute on the matching shape / `<g>` on round-trip.
    pub color_renderings: Vec<crate::preserved::ColorRenderingBinding>,
    /// Round 247 — scratch slot for the shape / `<g>` branch to hand a
    /// captured `color-rendering` keyword string to the wrapper-aware
    /// recorder that runs after `apply_referenced_defs`. Cleared at the
    /// end of each `parse_element_to_node_ctx` call (the drain matches
    /// the round-221 / round-228 flows).
    pub pending_color_rendering: Option<String>,
    /// Round 252 — collected `(scene_path, color-interpolation)` bindings
    /// (SVG 2 §13.9). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder re-emits the source
    /// attribute on the matching shape / `<g>` on round-trip.
    pub color_interpolations: Vec<crate::preserved::ColorInterpolationBinding>,
    /// Round 252 — scratch slot for the shape / `<g>` branch to hand a
    /// captured `color-interpolation` keyword string to the wrapper-aware
    /// recorder that runs after `apply_referenced_defs`. Cleared at the
    /// end of each `parse_element_to_node_ctx` call (the drain matches
    /// the round-221 / round-228 / round-247 flows).
    pub pending_color_interpolation: Option<String>,
    /// Round 257 — collected `(scene_path, overflow)` bindings
    /// (SVG 2 §3.11). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder re-emits the source
    /// attribute on the matching shape / `<g>` on round-trip.
    pub overflows: Vec<crate::preserved::OverflowBinding>,
    /// Round 257 — scratch slot for the shape / `<g>` branch to hand a
    /// captured `overflow` keyword string to the wrapper-aware recorder
    /// that runs after `apply_referenced_defs`. Cleared at the end of
    /// each `parse_element_to_node_ctx` call (the drain matches the
    /// round-221 / round-228 / round-247 / round-252 flows).
    pub pending_overflow: Option<String>,
    /// Round 260 — collected `(scene_path, pointer-events)` bindings
    /// (SVG 2 §15.6). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder re-emits the source
    /// attribute on the matching shape / `<g>` on round-trip.
    pub pointer_eventss: Vec<crate::preserved::PointerEventsBinding>,
    /// Round 260 — scratch slot for the shape / `<g>` branch to hand a
    /// captured `pointer-events` keyword string to the wrapper-aware
    /// recorder that runs after `apply_referenced_defs`. Cleared at
    /// the end of each `parse_element_to_node_ctx` call (the drain
    /// matches the round-221 / round-228 / round-247 / round-252 /
    /// round-257 flows).
    pub pending_pointer_events: Option<String>,
    /// Round 261 — collected `(scene_path, cursor)` bindings
    /// (SVG 1.1 §16.8.2). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder re-emits the source
    /// attribute on the matching shape / `<g>` on round-trip.
    pub cursors: Vec<crate::preserved::CursorBinding>,
    /// Round 261 — scratch slot for the shape / `<g>` branch to hand
    /// a captured `cursor` value string to the wrapper-aware recorder
    /// that runs after `apply_referenced_defs`. Cleared at the end of
    /// each `parse_element_to_node_ctx` call (the drain matches the
    /// round-221 .. round-260 flows).
    pub pending_cursor: Option<String>,
    /// Round 291 — collected `(scene_path, dominant-baseline)` bindings
    /// (SVG 1.1 §10.9.2). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder re-emits the source
    /// attribute on the matching shape / `<g>` on round-trip.
    pub dominant_baselines: Vec<crate::preserved::DominantBaselineBinding>,
    /// Round 291 — scratch slot for the shape / `<g>` branch to hand a
    /// captured `dominant-baseline` keyword string to the wrapper-aware
    /// recorder that runs after `apply_referenced_defs`. Cleared at the
    /// end of each `parse_element_to_node_ctx` call (the drain matches
    /// the round-221 .. round-261 flows).
    pub pending_dominant_baseline: Option<String>,
    /// Round 118 — "next element is a `<use>` instance root" flag.
    /// SVG 1.1 §11.5: "setting display: none on a `<path>` element will
    /// prevent that element from getting rendered directly onto the
    /// canvas, but the `<path>` element can still be referenced". So
    /// when a `<use>` re-parses its referenced source, the source's own
    /// `display:none` must NOT drop the instance — the author asked for
    /// it to be drawn here. The `<use>` resolver sets this flag right
    /// before re-parsing the source; the top-level `display:none`
    /// short-circuit in [`parse_element_to_node_ctx`] consumes (clears)
    /// it and skips the drop for that single element only. Cleared
    /// immediately so a `display:none` *descendant* inside the
    /// instantiated subtree still drops, matching a direct render.
    pub use_instance_root_pending: bool,
    /// Round 372 — collected `(scene_path, <use> reference)` bindings
    /// (SVG 2 §5.6). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder replaces the matching
    /// `<g>` (the instantiated instance) with `<use href="#id" …/>` on
    /// round-trip, skipping the flattened children.
    pub uses: Vec<crate::preserved::UseBinding>,
    /// Round 372 — collected `(scene_path, verbatim <switch>)` bindings
    /// (SVG 2 §5.7). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder replaces the matching
    /// `<g>` (the selected branch) with the verbatim `<switch>` on
    /// round-trip, skipping the selected child.
    pub switches: Vec<crate::preserved::SwitchBinding>,
    /// Round 449 — collected `(scene_path, verbatim <text>)` bindings
    /// (SVG 2 §11.2). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder replaces the matching
    /// flattened-glyph node with the verbatim `<text>` on round-trip,
    /// skipping the shaped outline children.
    pub texts: Vec<crate::preserved::TextBinding>,
    /// Round 449 — collected `(scene_path, animation children)`
    /// bindings (SMIL Animation §3.1 parent targeting). Populated only
    /// when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder re-emits each animation
    /// element as a child of the node at the recorded path so an
    /// animation keeps its direct XML parent — id-bearing or not — on
    /// round-trip.
    pub anim_targets: Vec<crate::preserved::AnimTargetBinding>,
    /// Round 449 — collected `(scene_path, native shape identity)`
    /// bindings (SVG 2 §9.2–§9.7). Populated only when the caller
    /// opted in via [`crate::decoder::parse_svg_with_extras`] (same
    /// gate as [`Self::track_id_paths`]). The encoder emits the
    /// matching geometry node as the source `<rect>` / `<circle>` /
    /// … tag with the verbatim geometry attributes instead of the
    /// flattened `<path d="…">`.
    pub shapes: Vec<crate::preserved::ShapeBinding>,
    /// Round 372 — collected `(scene_path, filter url-ref)` bindings
    /// (SVG 1.1 §15). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder re-emits
    /// `filter="url(#id)"` on the matching filter-wrapper `<g>` on
    /// round-trip so the preserved `<filter>` def stays referenced.
    pub filter_refs: Vec<crate::preserved::FilterRefBinding>,
    /// Round 372 — collected `(scene_path, marker-* refs)` bindings (SVG
    /// 2 §13.7.4). Populated only when the caller opted in via
    /// [`crate::decoder::parse_svg_with_extras`] (same gate as
    /// [`Self::track_id_paths`]). The encoder re-emits `marker-start` /
    /// `marker-mid` / `marker-end` on the matching shape on round-trip
    /// so the preserved `<marker>` def stays referenced.
    pub marker_refs: Vec<crate::preserved::MarkerRefBinding>,
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
            use_expansions: 0,
            render_depth: 0,
            stylesheet: Stylesheet::new(),
            animation_t: 0.0,
            current_path: Vec::new(),
            id_paths: Vec::new(),
            track_id_paths: false,
            resolve_ctx: ResolveContext::default(),
            path_lengths: Vec::new(),
            pending_path_length: None,
            system_language: Vec::new(),
            links: Vec::new(),
            titles: Vec::new(),
            descs: Vec::new(),
            use_instance_root_pending: false,
            paint_orders: Vec::new(),
            pending_paint_order: None,
            vector_effects: Vec::new(),
            pending_vector_effect: None,
            shape_renderings: Vec::new(),
            pending_shape_rendering: None,
            text_renderings: Vec::new(),
            pending_text_rendering: None,
            color_renderings: Vec::new(),
            pending_color_rendering: None,
            color_interpolations: Vec::new(),
            pending_color_interpolation: None,
            overflows: Vec::new(),
            pending_overflow: None,
            pointer_eventss: Vec::new(),
            pending_pointer_events: None,
            cursors: Vec::new(),
            pending_cursor: None,
            dominant_baselines: Vec::new(),
            pending_dominant_baseline: None,
            uses: Vec::new(),
            switches: Vec::new(),
            texts: Vec::new(),
            anim_targets: Vec::new(),
            shapes: Vec::new(),
            filter_refs: Vec::new(),
            marker_refs: Vec::new(),
        }
    }

    /// Round 372 — record the SVG 2 §13.7.4 `marker-start` / `marker-mid`
    /// / `marker-end` (or expanded `marker` shorthand) references at the
    /// current scene-graph path. Same [`Self::track_id_paths`] gate as
    /// the other side-channel recorders. No-op when no marker reference
    /// is present so a plain shape records nothing. The encoder re-emits
    /// the matching shape with the marker references on round-trip.
    pub fn record_marker_refs(&mut self, el: &Element) {
        if !self.track_id_paths {
            return;
        }
        // The `marker` shorthand sets all three position-specific
        // properties (§13.7.4); a position-specific longhand overrides
        // the shorthand for its slot.
        let shorthand = attr(el, "marker");
        let start = attr(el, "marker-start").or(shorthand);
        let mid = attr(el, "marker-mid").or(shorthand);
        let end = attr(el, "marker-end").or(shorthand);
        // Record only references (`url(...)`) — a `none` / absent value
        // is the initial value and needs no carrier.
        let norm = |v: Option<&str>| -> Option<String> {
            let t = v?.trim();
            if t.is_empty() || t.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(t.to_string())
            }
        };
        let marker_start = norm(start);
        let marker_mid = norm(mid);
        let marker_end = norm(end);
        if marker_start.is_none() && marker_mid.is_none() && marker_end.is_none() {
            return;
        }
        self.marker_refs.push(crate::preserved::MarkerRefBinding {
            path: self.current_path.clone(),
            marker_start,
            marker_mid,
            marker_end,
        });
    }

    /// Round 372 — record a `filter="url(#id)"` reference at the current
    /// scene-graph path (SVG 1.1 §15). Same [`Self::track_id_paths`]
    /// gate as the other side-channel recorders. The encoder re-emits
    /// `filter="url(#id)"` on the matching filter-wrapper `<g>` so the
    /// preserved `<filter>` def stays referenced after round-trip.
    pub fn record_filter_ref(&mut self, filter: String) {
        if !self.track_id_paths {
            return;
        }
        self.filter_refs.push(crate::preserved::FilterRefBinding {
            path: self.current_path.clone(),
            filter,
        });
    }

    /// Round 372 — record a `<switch>` verbatim binding at the current
    /// scene-graph path (SVG 2 §5.7). Same [`Self::track_id_paths`] gate
    /// as the other side-channel recorders. The encoder replaces the
    /// matching selected-branch `Node::Group` with the verbatim
    /// `<switch>` on round-trip, skipping the selected child.
    pub fn record_switch(&mut self, element: Element) {
        if !self.track_id_paths {
            return;
        }
        self.switches.push(crate::preserved::SwitchBinding {
            path: self.current_path.clone(),
            element,
        });
    }

    /// Round 449 — record a `<text>` verbatim binding at the current
    /// scene-graph path (SVG 2 §11.2). Same [`Self::track_id_paths`]
    /// gate as the other side-channel recorders. The encoder replaces
    /// the matching flattened-glyph node with the verbatim
    /// `<text>…</text>` on round-trip, skipping the shaped outline
    /// children.
    pub fn record_text(&mut self, element: Element) {
        if !self.track_id_paths {
            return;
        }
        self.texts.push(crate::preserved::TextBinding {
            path: self.current_path.clone(),
            element,
        });
    }

    /// Round 449 — record the direct SMIL animation-element children of
    /// `el` at the current scene-graph path (SMIL Animation §3.1: an
    /// animation element with no explicit target attribute targets its
    /// XML parent). Same [`Self::track_id_paths`] gate as the other
    /// side-channel recorders. No-op when `el` has no animation
    /// children. `<text>` and `<switch>` are excluded by the caller —
    /// their verbatim carriers already re-emit the animation children
    /// in place.
    pub fn record_anim_targets(&mut self, el: &Element) {
        if !self.track_id_paths {
            return;
        }
        // Inside a `<use>` target instantiation the flattened instance
        // subtree is never emitted (the encoder collapses it back to a
        // single `<use href=…/>`), and the instance boundary shares the
        // `<use>`'s own scene-graph slot — recording here would alias
        // the reference target's animations onto the `<use>` element,
        // duplicating the verbatim `<defs>`-target emission. The
        // `<use>` element's own animation children are recorded after
        // the instantiation completes (the stack is empty again by
        // then).
        if !self.use_stack.is_empty() {
            return;
        }
        let anims: Vec<Element> = el
            .children
            .iter()
            .filter_map(|c| match c {
                crate::parser::Node::Element(e)
                    if matches!(
                        tag_local(&e.name).as_str(),
                        "animate" | "set" | "animatetransform" | "animatemotion"
                    ) =>
                {
                    Some(e.clone())
                }
                _ => None,
            })
            .collect();
        if anims.is_empty() {
            return;
        }
        self.anim_targets.push(crate::preserved::AnimTargetBinding {
            path: self.current_path.clone(),
            anims,
        });
    }

    /// Round 372 — record a `<use>` reference binding at the current
    /// scene-graph path (SVG 2 §5.6). Same [`Self::track_id_paths`] gate
    /// as the other side-channel recorders. The encoder replaces the
    /// matching instantiated `Node::Group` with `<use href="#id" …/>` on
    /// round-trip, skipping the flattened children.
    pub fn record_use(&mut self, mut binding: crate::preserved::UseBinding) {
        if !self.track_id_paths {
            return;
        }
        binding.path = self.current_path.clone();
        self.uses.push(binding);
    }

    /// Round 98 — bind the user-preferred language list consulted by
    /// `<switch>` (`systemLanguage`, SVG 2 §5.7.5). Builder-style.
    pub fn with_system_language(mut self, langs: Vec<String>) -> Self {
        self.system_language = langs;
        self
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

    /// Round 21 — record the current scene-graph path against an
    /// author-supplied `pathLength`. Gated behind the same
    /// [`Self::track_id_paths`] flag as [`Self::record_id_path`]
    /// because both binding tables are consumed by the
    /// `parse_svg_with_extras` round-trip path.
    pub fn record_path_length(&mut self, path_length: f32) {
        if !self.track_id_paths {
            return;
        }
        self.path_lengths.push(crate::preserved::PathLengthBinding {
            path: self.current_path.clone(),
            path_length,
        });
    }

    /// Round 205 — record an author `paint-order` attribute against
    /// the current scene-graph path (SVG 2 §13.8). Same `track_id_paths`
    /// gate as the other side-channel recorders. The encoder re-emits
    /// the matching shape with `paint-order="..."` on round-trip.
    pub fn record_paint_order(&mut self, paint_order: String) {
        if !self.track_id_paths {
            return;
        }
        self.paint_orders.push(crate::preserved::PaintOrderBinding {
            path: self.current_path.clone(),
            paint_order,
        });
    }

    /// Round 209 — record an author `vector-effect` attribute against
    /// the current scene-graph path (SVG 2 §8.13). Same `track_id_paths`
    /// gate as the other side-channel recorders. The encoder re-emits
    /// the matching shape / `<use>` group with `vector-effect="..."` on
    /// round-trip.
    pub fn record_vector_effect(&mut self, vector_effect: String) {
        if !self.track_id_paths {
            return;
        }
        self.vector_effects
            .push(crate::preserved::VectorEffectBinding {
                path: self.current_path.clone(),
                vector_effect,
            });
    }

    /// Round 221 — record an author `shape-rendering` attribute against
    /// the current scene-graph path (SVG 2 §13.10.2). Same
    /// `track_id_paths` gate as the other side-channel recorders. The
    /// encoder re-emits the matching shape / `<g>` with
    /// `shape-rendering="..."` on round-trip.
    pub fn record_shape_rendering(&mut self, shape_rendering: String) {
        if !self.track_id_paths {
            return;
        }
        self.shape_renderings
            .push(crate::preserved::ShapeRenderingBinding {
                path: self.current_path.clone(),
                shape_rendering,
            });
    }

    /// Round 228 — record an author `text-rendering` attribute against
    /// the current scene-graph path (SVG 2 §13.10.3). Same
    /// `track_id_paths` gate as the other side-channel recorders. The
    /// encoder re-emits the matching `<text>` / `<g>` with
    /// `text-rendering="..."` on round-trip.
    pub fn record_text_rendering(&mut self, text_rendering: String) {
        if !self.track_id_paths {
            return;
        }
        self.text_renderings
            .push(crate::preserved::TextRenderingBinding {
                path: self.current_path.clone(),
                text_rendering,
            });
    }

    /// Round 247 — record an author `color-rendering` attribute against
    /// the current scene-graph path (SVG 2 §13.10.1). Same
    /// `track_id_paths` gate as the other side-channel recorders. The
    /// encoder re-emits the matching shape / `<g>` with
    /// `color-rendering="..."` on round-trip.
    pub fn record_color_rendering(&mut self, color_rendering: String) {
        if !self.track_id_paths {
            return;
        }
        self.color_renderings
            .push(crate::preserved::ColorRenderingBinding {
                path: self.current_path.clone(),
                color_rendering,
            });
    }

    /// Round 252 — record an author `color-interpolation` attribute
    /// against the current scene-graph path (SVG 2 §13.9). Same
    /// `track_id_paths` gate as the other side-channel recorders. The
    /// encoder re-emits the matching shape / `<g>` with
    /// `color-interpolation="..."` on round-trip.
    pub fn record_color_interpolation(&mut self, color_interpolation: String) {
        if !self.track_id_paths {
            return;
        }
        self.color_interpolations
            .push(crate::preserved::ColorInterpolationBinding {
                path: self.current_path.clone(),
                color_interpolation,
            });
    }

    /// Round 257 — record an author `overflow` attribute against
    /// the current scene-graph path (SVG 2 §3.11). Same
    /// `track_id_paths` gate as the other side-channel recorders. The
    /// encoder re-emits the matching shape / `<g>` with
    /// `overflow="..."` on round-trip. Unlike the §13.9 / §13.10.x
    /// hints, `overflow` is NOT inherited, so the per-element reset
    /// in [`PaintState::merged_with_mctx`] ensures the resolved value
    /// stops at the carrier element; the lexical side-channel still
    /// preserves the source attribute on its own emit slot.
    pub fn record_overflow(&mut self, overflow: String) {
        if !self.track_id_paths {
            return;
        }
        self.overflows.push(crate::preserved::OverflowBinding {
            path: self.current_path.clone(),
            overflow,
        });
    }

    /// Round 260 — record an author `pointer-events` attribute against
    /// the current scene-graph path (SVG 2 §15.6). Same
    /// `track_id_paths` gate as the other side-channel recorders. The
    /// encoder re-emits the matching shape / `<g>` with
    /// `pointer-events="..."` on round-trip. §15.6 selects the
    /// circumstances under which an element can be the target of a
    /// pointer event; the property IS inherited, so the lexical
    /// side-channel captures the source attribute at the topmost
    /// emit site (the carrier element) — descendant emissions inherit
    /// the resolved value through `PaintState`.
    pub fn record_pointer_events(&mut self, pointer_events: String) {
        if !self.track_id_paths {
            return;
        }
        self.pointer_eventss
            .push(crate::preserved::PointerEventsBinding {
                path: self.current_path.clone(),
                pointer_events,
            });
    }

    /// Round 261 — record an author `cursor` attribute against the
    /// current scene-graph path (SVG 1.1 §16.8.2). Same
    /// `track_id_paths` gate as the other side-channel recorders. The
    /// encoder re-emits the matching shape / `<g>` with
    /// `cursor="..."` on round-trip. §16.8.2 specifies the cursor
    /// shown while the pointing device hovers the element; the
    /// property IS inherited, so the lexical side-channel captures
    /// the source attribute at the topmost emit site (the carrier
    /// element) — descendant emissions inherit the resolved value
    /// through `PaintState`.
    pub fn record_cursor(&mut self, cursor: String) {
        if !self.track_id_paths {
            return;
        }
        self.cursors.push(crate::preserved::CursorBinding {
            path: self.current_path.clone(),
            cursor,
        });
    }

    /// Round 291 — record an author `dominant-baseline` attribute
    /// against the current scene-graph path (SVG 1.1 §10.9.2). Same
    /// `track_id_paths` gate as the other side-channel recorders. The
    /// encoder re-emits the matching shape / `<g>` with
    /// `dominant-baseline="..."` on round-trip. Unlike the §13.9 /
    /// §13.10.x hints, `dominant-baseline` is NOT inherited, so the
    /// per-element reset in [`PaintState::merged_with_mctx`] ensures the
    /// resolved value stops at the carrier element; the lexical
    /// side-channel still preserves the source attribute on its own
    /// emit slot (mirrors the round-257 `overflow` recorder).
    pub fn record_dominant_baseline(&mut self, dominant_baseline: String) {
        if !self.track_id_paths {
            return;
        }
        self.dominant_baselines
            .push(crate::preserved::DominantBaselineBinding {
                path: self.current_path.clone(),
                dominant_baseline,
            });
    }

    /// Round 115 — record an `<a>` hyperlink binding at the current
    /// scene-graph path (SVG 2 §16.5). Gated behind the same
    /// [`Self::track_id_paths`] flag as [`Self::record_id_path`]; the
    /// encoder consumes the table on the `parse_svg_with_extras`
    /// round-trip path to re-wrap the matching `Node::Group` in `<a>`.
    pub fn record_link(&mut self, mut link: crate::preserved::LinkBinding) {
        if !self.track_id_paths {
            return;
        }
        link.path = self.current_path.clone();
        self.links.push(link);
    }

    /// Round 122 — record a `<title>` or `<desc>` against the *parent*
    /// container's scene-graph path (SVG 2 §5.8). `is_title=true` routes
    /// to `self.titles`; `false` routes to `self.descs`. The
    /// `parent_path` is the caller's current scene-graph path with the
    /// last index stripped — that index belongs to the descriptive
    /// element itself, but `<title>` / `<desc>` are never-rendered so
    /// they never become scene-graph nodes; we attach to the parent
    /// container instead. Successive siblings of the same kind under
    /// the same parent append to the existing binding's `items` list,
    /// preserving source order for §5.8 multilingual selection.
    /// Gated behind [`Self::track_id_paths`] like the other side-channel
    /// recorders.
    pub fn record_descriptive(&mut self, el: &Element, is_title: bool) {
        if !self.track_id_paths {
            return;
        }
        // The descriptive element's own slot was pushed by the caller;
        // strip it to get the parent container's scene path.
        let mut parent_path = self.current_path.clone();
        parent_path.pop();
        let item = parse_descriptive_text(el);
        let bucket = if is_title {
            &mut self.titles
        } else {
            &mut self.descs
        };
        // Append to an existing binding for the same parent if one
        // exists (multi-sibling §5.8 case); otherwise create a new one.
        if let Some(b) = bucket.iter_mut().find(|b| b.parent_path == parent_path) {
            b.items.push(item);
        } else {
            bucket.push(crate::preserved::DescriptiveBinding {
                parent_path,
                items: vec![item],
            });
        }
    }
}

/// Round 122 — extract the plain-text content and the optional `lang` /
/// `xml:lang` of a `<title>` or `<desc>` element per SVG 2 §5.8. Per
/// the spec only the plain text is exposed to assistive technologies,
/// so we flatten the element's direct text-child runs (children that
/// are markup nodes contribute the empty string here — a structured
/// foreign-namespace capture is out of scope for this round and would
/// fall to a future revision of [`crate::preserved::DescriptiveText`]).
fn parse_descriptive_text(el: &Element) -> crate::preserved::DescriptiveText {
    let mut text = String::new();
    for child in &el.children {
        if let XmlNode::Text(t) = child {
            text.push_str(t);
        }
    }
    // Per SVG 2 §5.12.3 the SVG `lang` attribute mirrors HTML `lang`;
    // the deprecated `xml:lang` survives in legacy SVG 1.1 documents
    // and per §5.12.3 wins when both are present and disagree (we
    // surface the SVG-2 `lang` first and only fall back to `xml:lang`
    // when `lang` is absent, matching the round-trip-preserving
    // convention used by other side-channels in this crate).
    let lang = attr(el, "lang")
        .or_else(|| attr(el, "xml:lang"))
        .map(str::to_string);
    crate::preserved::DescriptiveText { text, lang }
}

/// Round 115 — extract the SVG 2 §16.5 `<a>` hyperlink attributes into a
/// [`crate::preserved::LinkBinding`] (the `path` is filled in by the
/// caller via [`ParseContext::record_link`]). `href` prefers the SVG-2
/// `href` and falls back to the deprecated SVG-1.1 `xlink:href`.
fn parse_link_binding(el: &Element) -> crate::preserved::LinkBinding {
    // Round 382 — sweep up every attribute the typed link fields below
    // don't model, preserving document order, so the `<a>`-wrapper round
    // trip keeps `id` / `class` / `style` / `transform` / presentation /
    // conditional-processing attributes.
    let extra_attrs: Vec<(String, String)> = el
        .attrs
        .iter()
        .filter(|(k, _)| {
            !matches!(
                k.as_str(),
                "href"
                    | "xlink:href"
                    | "target"
                    | "download"
                    | "ping"
                    | "rel"
                    | "hreflang"
                    | "type"
                    | "referrerpolicy"
            )
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    crate::preserved::LinkBinding {
        path: Vec::new(),
        href: attr(el, "href")
            .or_else(|| attr(el, "xlink:href"))
            .map(str::to_string),
        target: attr(el, "target").map(str::to_string),
        download: attr(el, "download").map(str::to_string),
        ping: attr(el, "ping").map(str::to_string),
        rel: attr(el, "rel").map(str::to_string),
        hreflang: attr(el, "hreflang").map(str::to_string),
        type_: attr(el, "type").map(str::to_string),
        referrerpolicy: attr(el, "referrerpolicy").map(str::to_string),
        extra_attrs,
    }
}

/// Round 372 — extract the SVG 2 §5.6 `<use>` reference identity into a
/// [`crate::preserved::UseBinding`] (the `path` is filled in by the
/// caller via [`ParseContext::record_use`]). `href` prefers the SVG-2
/// `href` and falls back to the deprecated SVG-1.1 `xlink:href`.
///
/// Returns `None` when the source carried no local fragment reference
/// (no `href`/`xlink:href`, or an external `other.svg#id` target the
/// decoder never instantiates) — those `<use>` elements produce no
/// scene-graph node, so there is nothing to collapse on round-trip.
fn parse_use_binding(el: &Element) -> Option<crate::preserved::UseBinding> {
    let href = attr(el, "href").or_else(|| attr(el, "xlink:href"))?;
    let href = href.trim();
    // Only local `#id` references instantiate; an external reference is
    // dropped by `parse_use_element`, so we don't record a binding for
    // it (it has no emit site to collapse).
    if !href.starts_with('#') {
        return None;
    }
    // Round 382 — sweep up every attribute the typed slots below don't
    // model, preserving document order, so a collapse-to-`<use>` round
    // trip doesn't drop `class` / `style` / presentation / conditional-
    // processing attributes.
    let extra_attrs: Vec<(String, String)> = el
        .attrs
        .iter()
        .filter(|(k, _)| {
            !matches!(
                k.as_str(),
                "href" | "xlink:href" | "x" | "y" | "width" | "height" | "transform" | "id"
            )
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Some(crate::preserved::UseBinding {
        path: Vec::new(),
        href: href.to_string(),
        x: attr(el, "x").map(str::to_string),
        y: attr(el, "y").map(str::to_string),
        width: attr(el, "width").map(str::to_string),
        height: attr(el, "height").map(str::to_string),
        transform: attr(el, "transform").map(str::to_string),
        id: attr(el, "id").map(str::to_string),
        extra_attrs,
    })
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

/// Round 118 — does SVG 1.1 §11.5 `display` apply to this element?
/// Per the property table, `display` applies to `svg`, `g`, `switch`,
/// `a`, `foreignObject`, graphics elements (incl. `text`), and the
/// text sub-elements (`tspan`, …). SVG 2 also lists `use`. We
/// enumerate the renderable / container tags this crate produces a
/// scene node for; never-rendered tags (`defs`, gradients, `marker`,
/// `symbol`, `mask`, `clipPath`, `style`, animation) are excluded
/// because §11.5 states `display` "does not apply" to them — and they
/// return `None` from their own match arms regardless.
fn display_applies(local_lower: &str) -> bool {
    matches!(
        local_lower,
        "g" | "a"
            | "switch"
            | "foreignobject"
            | "use"
            | "rect"
            | "circle"
            | "ellipse"
            | "line"
            | "polyline"
            | "polygon"
            | "path"
            | "text"
            | "image"
    )
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

/// Round 98 — is a `<switch>` child eligible to be the rendered choice?
///
/// SVG 2 §5.7.1: conditional processing "does not affect the processing
/// of a `style` or `script` element" and "will have no effect on
/// never-rendered elements", and it "prevents animation elements from
/// playing". So a `<switch>` only ever selects among its *renderable*
/// direct children; never-rendered children (`<defs>`, `<style>`,
/// `<script>`, gradients, filter/mask/clipPath/symbol/marker defs, and
/// the animation elements) are skipped without consuming the "first
/// match" slot. The eligible set is the renderable content listed in
/// the §5.7.3 `<switch>` content model.
fn is_switch_candidate(local: &str) -> bool {
    matches!(
        local,
        "circle"
            | "ellipse"
            | "line"
            | "path"
            | "polygon"
            | "polyline"
            | "rect"
            | "a"
            | "audio"
            | "canvas"
            | "foreignobject"
            | "g"
            | "iframe"
            | "image"
            | "svg"
            | "switch"
            | "text"
            | "use"
            | "video"
    )
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

/// Round 403 — the maximum model-builder recursion depth. The heavy
/// per-element decode frame limits how deep the native stack can go, and
/// `<use>` chains can drive that recursion arbitrarily deep independent
/// of the (parse-time-bounded) XML nesting, so this guard converts a
/// pathologically deep decode into a typed error rather than a stack
/// overflow / abort. Real SVG decode never nests anywhere near this far.
pub const MAX_RENDER_DEPTH: usize = 128;

/// Round-5 entry point. Same as [`parse_element_to_node`] but takes a
/// fully-chained [`MatchContext`] so the CSS cascade can resolve
/// combinator selectors and structural pseudo-classes.
///
/// Round 403 — thin depth-guarding wrapper around the recursive body
/// ([`parse_element_to_node_ctx_inner`]). Every recursive descent — a
/// nested `<g>`, an instantiated `<use>` target, a `<switch>` branch —
/// re-enters through here, so incrementing the shared counter on the way
/// in and decrementing on the way out bounds the total decode recursion
/// at [`MAX_RENDER_DEPTH`] regardless of which recursion source drove it.
pub fn parse_element_to_node_ctx(
    el: &Element,
    parent_state: &PaintState,
    ctx: &mut ParseContext,
    mctx: &MatchContext<'_>,
) -> Result<Option<Node>> {
    if ctx.render_depth >= MAX_RENDER_DEPTH {
        return Err(Error::invalid(
            "SVG: element tree too deeply nested to decode",
        ));
    }
    ctx.render_depth += 1;
    let result = parse_element_to_node_ctx_inner(el, parent_state, ctx, mctx);
    ctx.render_depth -= 1;
    result
}

fn parse_element_to_node_ctx_inner(
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
    // snapshot at any point on the timeline. Round 125 widened the
    // helper to also evaluate `<animateMotion>` — and that branch
    // needs the id table so an `<mpath xlink:href="#path1">` can
    // resolve to the referenced `<path>`. We pass `&ctx.defs.elements`
    // (already populated by the pre-walk) as the id-lookup.
    let with_anim = apply_animation_overrides(el, ctx.animation_t, &ctx.defs.elements);
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
    // Round 118 — SVG 1.1 §11.5 `display:none`. The property applies to
    // `svg`, `g`, `switch`, `a`, `foreignObject`, graphics elements
    // (incl. `text`) and `use`. When resolved to `none`, the element
    // and its children "do not become part of the rendering tree" —
    // drop the node here so neither it nor its subtree is walked. The
    // never-rendered elements (`defs`, gradients, `marker`, `symbol`,
    // `mask`, `clipPath`, `style`, animation) are excluded: §11.5 says
    // `display` "does not apply" to them, and they already return
    // `None` via their own match arms regardless.
    //
    // Suppressed for the *root* of a `<use>` instance (§11.5: a
    // `display:none` definition "can still be referenced"). The `<use>`
    // resolver sets `ctx.use_instance_root_pending` before re-parsing
    // the source; we consume it here so only the instance root is
    // exempted — a nested `display:none` *inside* the instantiated
    // subtree still drops, matching how a direct render would behave.
    if display_applies(&local) {
        let exempt = std::mem::take(&mut ctx.use_instance_root_pending);
        if !exempt
            && !parent_state
                .merged_with_mctx(mctx, &ctx.stylesheet)?
                .display
        {
            return Ok(None);
        }
    } else {
        // A never-rendered element should not consume the instance-root
        // exemption (a `<use href="#defs-child">` wouldn't reach one,
        // but keep the flag honest in case the source root is such an
        // element — it returns `None` anyway).
        ctx.use_instance_root_pending = false;
    }
    let node_opt = match local.as_str() {
        "g" => {
            let state = parent_state.merged_with_mctx(mctx, &ctx.stylesheet)?;
            // Round 19 — push the `<g>`'s `font-size` cascade into a
            // child resolve context so descendants resolve `em` /
            // `rem` against the group's font-size, not the outer
            // viewport's. Restored after this branch returns.
            let saved_ctx = ctx.resolve_ctx;
            ctx.resolve_ctx = derive_child_ctx(el, mctx, &ctx.stylesheet, &saved_ctx);
            // Round 205 — capture any `paint-order=` carried on the
            // `<g>` itself so the round-trip preserves the attribute
            // on the same emit site. A `<g paint-order="stroke">`
            // cascades the property to every child shape's PaintState
            // and the round-trip re-parse would otherwise capture it
            // on each child shape redundantly; emitting it on the
            // group matches the source-faithful round-trip per the
            // round-13 id-path policy (emit on the topmost slot the
            // source attribute lived on). The saved value is restored
            // *after* the child walk because each child also goes
            // through this drain slot.
            let saved_pending_paint_order = ctx.pending_paint_order.take();
            let group_paint_order = capture_paint_order_attr(el);
            // Round 209 — capture any `vector-effect=` carried on the
            // `<g>` so a hand-authored attribute survives round-trip.
            // The property does NOT cascade per §8.13 (Inherited: no),
            // but the round-trip carrier is purely lexical — it
            // preserves the author's literal source. The drain at the
            // bottom of this function attaches the captured string to
            // the group's emit site.
            let saved_pending_vector_effect = ctx.pending_vector_effect.take();
            let group_vector_effect = capture_vector_effect_attr(el);
            // Round 221 — capture any `shape-rendering=` carried on the
            // `<g>` so a hand-authored attribute survives round-trip.
            // The property IS inherited per §13.10.2, but the round-trip
            // carrier is purely lexical — emitting it on the topmost
            // emit site (the group) avoids redundantly recording on
            // every cascaded descendant.
            let saved_pending_shape_rendering = ctx.pending_shape_rendering.take();
            let group_shape_rendering = capture_shape_rendering_attr(el);
            // Round 228 — capture any `text-rendering=` carried on the
            // `<g>` so a hand-authored attribute survives round-trip.
            // The property IS inherited per §13.10.3; the carrier is
            // purely lexical, so emitting it on the topmost emit site
            // (the group) avoids redundantly recording on every
            // cascaded descendant `<text>`.
            let saved_pending_text_rendering = ctx.pending_text_rendering.take();
            let group_text_rendering = capture_text_rendering_attr(el);
            // Round 247 — capture any `color-rendering=` carried on the
            // `<g>` so a hand-authored attribute survives round-trip.
            // The property IS inherited per §13.10.1; the carrier is
            // purely lexical so emitting it on the topmost emit site
            // (the group) avoids redundantly recording on every
            // cascaded descendant.
            let saved_pending_color_rendering = ctx.pending_color_rendering.take();
            let group_color_rendering = capture_color_rendering_attr(el);
            // Round 252 — capture any `color-interpolation=` carried on
            // the `<g>` so a hand-authored attribute survives round-trip.
            // The property IS inherited per §13.9; the carrier is purely
            // lexical so emitting it on the topmost emit site (the
            // group) avoids redundantly recording on every cascaded
            // descendant.
            let saved_pending_color_interpolation = ctx.pending_color_interpolation.take();
            let group_color_interpolation = capture_color_interpolation_attr(el);
            // Round 257 — capture any `overflow=` carried on the
            // `<g>` so a hand-authored attribute survives round-trip.
            // The property is NOT inherited per CSS 2.1 §11.1.1, but
            // the round-trip carrier is purely lexical so a
            // `<g overflow="hidden">` round-trips on its own emit
            // slot regardless of whether the cascade would have
            // pushed the value to descendants (it doesn't).
            let saved_pending_overflow = ctx.pending_overflow.take();
            let group_overflow = capture_overflow_attr(el);
            // Round 260 — capture any `pointer-events=` carried on the
            // `<g>` so a hand-authored attribute survives round-trip.
            // The property IS inherited per §15.6; the carrier is
            // purely lexical so emitting it on the topmost emit site
            // (the group) avoids redundantly recording on every
            // cascaded descendant.
            let saved_pending_pointer_events = ctx.pending_pointer_events.take();
            let group_pointer_events = capture_pointer_events_attr(el);
            // Round 261 — capture any `cursor=` carried on the `<g>`
            // so a hand-authored attribute survives round-trip. The
            // property IS inherited per SVG 1.1 §16.8.2; the carrier
            // is purely lexical so emitting it on the topmost emit
            // site (the group) avoids redundantly recording on every
            // cascaded descendant.
            let saved_pending_cursor = ctx.pending_cursor.take();
            let group_cursor = capture_cursor_attr(el);
            // Round 291 — capture any `dominant-baseline=` carried on the
            // `<g>` so a hand-authored attribute survives round-trip.
            // The property is NOT inherited per §10.9.2, but the
            // round-trip carrier is purely lexical so a
            // `<g dominant-baseline="hanging">` round-trips on its own
            // emit slot regardless of whether the cascade would have
            // pushed the value to descendants (it doesn't). Mirrors the
            // round-257 `overflow` capture.
            let saved_pending_dominant_baseline = ctx.pending_dominant_baseline.take();
            let group_dominant_baseline = capture_dominant_baseline_attr(el);
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
            // Round 205 — restore the outer pending slot, then layer
            // the `<g>`'s own paint-order attribute (if any) so the
            // outer drain attaches it to the group's emit site.
            ctx.pending_paint_order = group_paint_order.or(saved_pending_paint_order);
            // Round 209 — same restore-then-layer flow for the
            // `vector-effect` carrier.
            ctx.pending_vector_effect = group_vector_effect.or(saved_pending_vector_effect);
            // Round 221 — same restore-then-layer flow for the
            // `shape-rendering` carrier.
            ctx.pending_shape_rendering = group_shape_rendering.or(saved_pending_shape_rendering);
            // Round 228 — same restore-then-layer flow for the
            // `text-rendering` carrier.
            ctx.pending_text_rendering = group_text_rendering.or(saved_pending_text_rendering);
            // Round 247 — same restore-then-layer flow for the
            // `color-rendering` carrier.
            ctx.pending_color_rendering = group_color_rendering.or(saved_pending_color_rendering);
            // Round 252 — same restore-then-layer flow for the
            // `color-interpolation` carrier.
            ctx.pending_color_interpolation =
                group_color_interpolation.or(saved_pending_color_interpolation);
            // Round 257 — same restore-then-layer flow for the
            // `overflow` carrier.
            ctx.pending_overflow = group_overflow.or(saved_pending_overflow);
            // Round 260 — same restore-then-layer flow for the
            // `pointer-events` carrier.
            ctx.pending_pointer_events = group_pointer_events.or(saved_pending_pointer_events);
            // Round 261 — same restore-then-layer flow for the
            // `cursor` carrier.
            ctx.pending_cursor = group_cursor.or(saved_pending_cursor);
            // Round 291 — same restore-then-layer flow for the
            // `dominant-baseline` carrier.
            ctx.pending_dominant_baseline =
                group_dominant_baseline.or(saved_pending_dominant_baseline);
            Some(Node::Group(group))
        }
        // Round 115 — SVG 2 §16.5 `<a>` hyperlink. The `<a>` element is
        // categorised as both a *container element* and a *renderable
        // element*: it groups + renders its children exactly like `<g>`
        // (transform / opacity / paint cascade / per-element `em`
        // cascade), and merely *also* establishes a hyperlink. So we
        // build a `Node::Group` identical to the `<g>` arm; the
        // hyperlink target + its HTML companion attributes (`href` /
        // `target` / `download` / `ping` / `rel` / `hreflang` / `type`
        // / `referrerpolicy`) are stowed on `ctx.links` for the encoder
        // to re-wrap in `<a>` on round-trip (`oxideav_core::Group` has
        // no hyperlink field). Per the §16.5 content model an `<a>` may
        // not contain another `<a>`, but a UA still renders nested
        // anchor content; we render it and simply record the inner
        // link too (the outer wins navigationally, but both groups
        // round-trip).
        "a" => {
            let state = parent_state.merged_with_mctx(mctx, &ctx.stylesheet)?;
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
            ctx.resolve_ctx = saved_ctx;
            // Record the hyperlink at this group's own scene-graph path
            // so the encoder re-wraps it in `<a>`. `current_path` here
            // points at the `<a>`'s own slot (the caller pushed the
            // scene index before invoking us).
            ctx.record_link(parse_link_binding(el));
            Some(Node::Group(group))
        }
        // Round 98 — SVG 2 §5.7.3 `<switch>` conditional processing.
        // "The `switch` element evaluates the `requiredExtensions` and
        // `systemLanguage` attributes on its direct child elements in
        // order, and then processes and renders the first child for
        // which these attributes evaluate to true. All others will be
        // bypassed and therefore not rendered."
        "switch" => {
            // The switch is itself a container element — wrap the chosen
            // child in a Group carrying the switch's own transform /
            // opacity so authored grouping survives the parse → encode
            // round-trip (mirrors the `<g>` arm).
            let state = parent_state.merged_with_mctx(mctx, &ctx.stylesheet)?;
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
                    child_idx += 1;
                    // §5.7.1: conditional processing "does not affect
                    // the processing of a `style` or `script` element"
                    // and has "no effect on never-rendered elements".
                    // Such children are not switch candidates — they are
                    // skipped without consuming the "first match" slot.
                    // (Animation elements never produce a scene node in
                    // our model, so excluding them here also matches
                    // "conditional processing prevents animation
                    // elements from playing".)
                    if !is_switch_candidate(&lower) {
                        continue;
                    }
                    // §5.7.3: render the first child whose conditional
                    // processing attributes all test true; bypass the
                    // rest.
                    if !crate::conditional::passes_conditional(c, &ctx.system_language) {
                        continue;
                    }
                    let cmctx =
                        child_match_context(mctx, c, child_idx - 1, of_idx, total, of_count);
                    let scene_idx = group.children.len();
                    ctx.current_path.push(scene_idx);
                    let result = parse_element_to_node_ctx(c, &state, ctx, &cmctx);
                    ctx.current_path.pop();
                    match result? {
                        Some(node) => {
                            group.children.push(node);
                            // First matching, renderable child wins —
                            // stop scanning.
                            break;
                        }
                        // A candidate that passed the tests but produced
                        // no scene node (e.g. an empty `<g>`): per §5.7.3
                        // it is still "the first child for which these
                        // attributes evaluate to true", so it is chosen
                        // and the remaining children are bypassed.
                        None => break,
                    }
                }
            }
            ctx.resolve_ctx = saved_ctx;
            Some(Node::Group(group))
        }
        // Round 375 — a *nested* `<svg>` element (SVG 1.1 §7.10 /
        // SVG 2 §8.2). Unlike the outermost `<svg>` (handled by the
        // decoder before the element walk begins), an inner `<svg>`
        // establishes a brand-new viewport: it carries its own
        // `x` / `y` / `width` / `height` (placing + sizing the new
        // viewport in the *current* user space) and, optionally, a
        // `viewBox` + `preserveAspectRatio` that re-maps the new
        // viewport's coordinate system. Before this round an inner
        // `<svg>` fell through to the `_ => None` deferral and was
        // silently dropped along with its entire subtree.
        //
        // We model it as a `Node::Group` whose transform is
        //   translate(x, y) ∘ viewport_transform(viewBox → w×h)
        // and whose children are walked with the inner viewport's
        // dimensions installed on the resolve context (so a child's
        // `width="50%"` resolves against the nested viewport, per
        // §7.10). When there is no `viewBox` the viewport transform is
        // the identity and only the `x`/`y` placement applies.
        "svg" => {
            let state = parent_state.merged_with_mctx(mctx, &ctx.stylesheet)?;
            // §8.2: `x` / `y` default to 0; `width` / `height` default
            // to `100%` (the full parent viewport). Resolve percentages
            // against the *parent* viewport axes that are live on the
            // current resolve context.
            let saved_ctx = ctx.resolve_ctx;
            let x = parse_length_attr(attr(el, "x"), 0.0, LengthAxis::X, &saved_ctx)?;
            let y = parse_length_attr(attr(el, "y"), 0.0, LengthAxis::Y, &saved_ctx)?;
            let width = parse_length_attr(
                attr(el, "width"),
                saved_ctx.viewport_w,
                LengthAxis::X,
                &saved_ctx,
            )?;
            let height = parse_length_attr(
                attr(el, "height"),
                saved_ctx.viewport_h,
                LengthAxis::Y,
                &saved_ctx,
            )?;
            // §8.2 step 1: a zero (or negative) width / height disables
            // rendering of the element and its children. Drop the whole
            // subtree in that case.
            if width <= 0.0 || height <= 0.0 {
                None
            } else {
                let view_box = attr(el, "viewBox").and_then(parse_symbol_view_box);
                let par = match attr(el, "preserveAspectRatio") {
                    Some(s) => crate::filter::PreserveAspectRatio::from_str(s),
                    None => crate::filter::PreserveAspectRatio::default(),
                };
                // The viewport transform maps the `viewBox` rectangle
                // onto the `width × height` viewport per §8.2; with no
                // `viewBox` (or a degenerate one) it is the identity.
                let viewport_t = match view_box {
                    Some(vb) if vb.width > 0.0 && vb.height > 0.0 => {
                        symbol_viewport_transform(width, height, vb, par)
                    }
                    _ => Transform2D::identity(),
                };
                let transform = Transform2D::translate(x, y).compose(&viewport_t);
                // Install the nested viewport's dimensions so descendant
                // percentage lengths resolve against it (§7.10). When a
                // `viewBox` is present the inner user-space extent is the
                // viewBox width / height; otherwise it is the viewport
                // `width` / `height`.
                let inner = match view_box {
                    Some(vb) if vb.width > 0.0 && vb.height > 0.0 => (vb.width, vb.height),
                    _ => (width, height),
                };
                ctx.resolve_ctx = ResolveContext {
                    viewport_w: inner.0,
                    viewport_h: inner.1,
                    ..saved_ctx
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
                        let cmctx =
                            child_match_context(mctx, c, child_idx, of_idx, total, of_count);
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
                ctx.resolve_ctx = saved_ctx;
                Some(Node::Group(group))
            }
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
        // return None here. Round 104 adds `<marker>` — a never-rendered
        // element per SVG 2 §13.7.1, consumed via the `marker-*`
        // properties on shapes (pre-walked into `ctx.defs.markers`).
        "filter" | "mask" | "clippath" | "symbol" | "marker" => None,
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
        // Round 122 — SVG 2 §5.8 descriptive elements. `<title>` and
        // `<desc>` are categorised as `never-rendered element`s per the
        // §5.8 dfn block; the UA stylesheet forces `display:none` "with
        // importance over any other CSS rule or presentation attribute"
        // so they MUST NOT contribute a scene-graph node. Capture the
        // text + optional `lang` against the *parent* container's
        // scene-graph path so the encoder can re-emit them as the first
        // children of the matching `<g>` on round-trip (the spec
        // multilingual selection algorithm runs at the consumer side
        // against the captured list).
        //
        // `<metadata>` (§5.9) is also a descriptive / never-rendered
        // element; it carries arbitrary foreign-namespace XML
        // (RDF / Dublin Core / Inkscape extensions). It rides on
        // `PreservedExtras::metadata` via the pre-walk in
        // `decoder::collect_extras` and re-emits verbatim at the
        // trailing edge of the document — no parse-side capture needed
        // here.
        "title" => {
            ctx.record_descriptive(el, true);
            None
        }
        "desc" => {
            ctx.record_descriptive(el, false);
            None
        }
        "metadata" => None,
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
            // Round 118 — SVG 1.1 §11.5 `visibility: hidden | collapse`.
            // The graphics element is "invisible (i.e., nothing is
            // painted on the canvas)" but, unlike `display:none`, "the
            // geometry of the graphics element still contributes to
            // bounding box and clipping path calculations". So we keep
            // the `Path` node (geometry intact) but drop its fill and
            // stroke paint. `visibility` is inherited, so a hidden `<g>`
            // ancestor reaches here via `state.visibility`; a descendant
            // that set `visibility="visible"` overrides it back on.
            let hidden = state.visibility == Visibility::Hidden;
            let fill = if hidden {
                None
            } else {
                state.solid_fill(&ctx.gradients, &ctx.defs)
            };
            let mut stroke = if hidden {
                None
            } else {
                state.solid_stroke(&ctx.gradients, &ctx.defs)
            };
            // Round 21 — SVG 2 §9.6.1: the `pathLength` attribute
            // re-scales `stroke-dasharray` / `stroke-dashoffset` so a
            // downstream rasteriser that only knows user units paints
            // the spec-correct dash pattern. The author-supplied total
            // is also captured on `ctx.pending_path_length`; the
            // outer-most wrapper logic below records it at the
            // **inner Path's** scene-graph slot so the encoder
            // re-emits `pathLength="..."` on the `<path>` element
            // itself (not on a wrapping `<g transform=...>`).
            ctx.pending_path_length =
                crate::path_length::apply_to_path_node(attr(el, "pathLength"), &path, &mut stroke);
            // Round 205 — SVG 2 §13.8 `paint-order` round-trip
            // capture. If the source `paint-order=` attribute was
            // present (and parsed to a recognised non-normal value),
            // stash the canonicalised keyword string for the outer
            // wrapper-aware recorder to attach to the inner Path
            // node's scene slot. The cascade may also resolve a
            // non-normal paint-order via CSS or a `<g paint-order=…>`
            // ancestor; we only round-trip the *attribute* literal so
            // the source representation survives, matching the
            // round-21 pathLength capture's "presentation attribute
            // only" policy. Empty / `normal` / `inherit` are dropped.
            ctx.pending_paint_order = capture_paint_order_attr(el);
            // Round 209 — SVG 2 §8.13 `vector-effect` round-trip
            // capture. Same flow as the round-205 `paint-order`
            // attribute: the cascade has already set the resolved
            // value on `state.vector_effect`, but the round-trip
            // carrier records the source-literal so a `parse → write`
            // cycle re-emits the author's keyword list verbatim
            // (canonicalised). Empty / `none` / `inherit` skip
            // recording so the binding is never a no-op.
            ctx.pending_vector_effect = capture_vector_effect_attr(el);
            // Round 221 — SVG 2 §13.10.2 `shape-rendering` round-trip
            // capture. Same flow as the round-205 / round-209 captures
            // above: the cascade resolves the property onto
            // `state.shape_rendering`, but the round-trip carrier
            // records the source-literal so a `parse → write` cycle
            // re-emits the author's keyword verbatim (canonicalised to
            // the spec's camelCase). Absent / `inherit` / unrecognised
            // tokens skip recording.
            ctx.pending_shape_rendering = capture_shape_rendering_attr(el);
            // Round 247 — SVG 2 §13.10.1 `color-rendering` round-trip
            // capture. Same flow as the round-221 `shape-rendering`
            // capture: the cascade has already resolved the property
            // onto `state.color_rendering`, but the round-trip carrier
            // records the source-literal so a `parse → write` cycle
            // re-emits the author's keyword verbatim (canonicalised to
            // the spec's camelCase). Absent / `inherit` / unrecognised
            // tokens skip recording.
            ctx.pending_color_rendering = capture_color_rendering_attr(el);
            // Round 252 — SVG 2 §13.9 `color-interpolation` round-trip
            // capture. Same flow as the round-247 `color-rendering`
            // capture above: the cascade has already resolved the
            // property onto `state.color_interpolation`, but the
            // round-trip carrier records the source-literal so a
            // `parse → write` cycle re-emits the author's keyword
            // verbatim (canonicalised to §13.9's mixed-case spelling
            // `auto` / `sRGB` / `linearRGB`). Absent / `inherit` /
            // unrecognised tokens skip recording.
            ctx.pending_color_interpolation = capture_color_interpolation_attr(el);
            // Round 257 — SVG 2 §3.11 `overflow` round-trip capture.
            // Same flow as the round-252 `color-interpolation` capture
            // above: the cascade has already resolved the property
            // onto `state.overflow` (after the §3.11 non-inheritance
            // reset), but the round-trip carrier records the
            // source-literal so a `parse → write` cycle re-emits the
            // author's keyword verbatim (canonicalised to lowercase
            // per the §3.11 / CSS 2.1 keyword set `visible` /
            // `hidden` / `scroll` / `auto`). Absent / `inherit` /
            // unrecognised tokens skip recording.
            ctx.pending_overflow = capture_overflow_attr(el);
            // Round 260 — SVG 2 §15.6 `pointer-events` round-trip
            // capture. Same flow as the round-257 `overflow` capture
            // above: the cascade has already resolved the property
            // onto `state.pointer_events`, but the round-trip carrier
            // records the source-literal so a `parse → write` cycle
            // re-emits the author's keyword verbatim (canonicalised
            // to the §15.6 spelling — lower-camelCase for
            // `visiblePainted` / `visibleFill` / `visibleStroke`,
            // hyphenated for `bounding-box`, lowercase for the rest).
            // Absent / `inherit` / unrecognised tokens skip recording.
            ctx.pending_pointer_events = capture_pointer_events_attr(el);
            // Round 261 — SVG 1.1 §16.8.2 `cursor` round-trip capture.
            // Same flow as the round-260 `pointer-events` capture
            // above: the cascade has already resolved the property
            // onto `state.cursor`, but the round-trip carrier records
            // the source-literal so a `parse → write` cycle re-emits
            // the author's value (canonicalised: lowercase keyword,
            // funciris in source order, comma-and-space separated per
            // the §16.8.2 list example). Absent / `inherit` / invalid
            // payloads skip recording.
            ctx.pending_cursor = capture_cursor_attr(el);
            // Round 291 — SVG 1.1 §10.9.2 `dominant-baseline` round-trip
            // capture. Same flow as the round-257 `overflow` capture
            // above: the cascade has already resolved the property onto
            // `state.dominant_baseline` (after the §10.9.2
            // non-inheritance reset), but the round-trip carrier records
            // the source-literal so a `parse → write` cycle re-emits the
            // author's keyword verbatim (canonicalised to the §10.9.2
            // all-lowercase / hyphenated spelling). Absent / `inherit` /
            // unrecognised tokens skip recording.
            ctx.pending_dominant_baseline = capture_dominant_baseline_attr(el);
            // Round 205 — SVG 2 §13.8 `paint-order`. The round-1
            // `PathNode { fill, stroke }` shape always paints fill
            // BEFORE stroke (the §13.8 `normal` order); when the
            // resolved property requests stroke BEFORE fill, split
            // the shape into two single-purpose PathNodes inside a
            // wrapping Group so the scene graph composites in the
            // requested order. `markers` parses and round-trips but
            // contributes no node here — `oxideav_core::Node` has no
            // `Marker` variant yet, so a `paint-order: markers stroke
            // fill` collapses to `paint-order: stroke fill` for the
            // purpose of node emission (the markers slot is otherwise
            // a no-op today, as documented at the
            // [`crate::defs::MarkerDef`] capture site).
            let inner_node =
                if state.paint_order.stroke_before_fill() && fill.is_some() && stroke.is_some() {
                    // Split: stroke-only PathNode painted first, fill-only
                    // PathNode painted second. Both reference the same
                    // geometric path; the stroke-only's `fill` is None so
                    // the rasteriser does not double-paint the interior,
                    // and the fill-only's `stroke` is None so the stroke
                    // does not also re-paint on top.
                    let stroke_node = PathNode {
                        path: path.clone(),
                        fill: None,
                        stroke: stroke.clone(),
                        fill_rule: state.fill_rule,
                    };
                    let fill_node = PathNode {
                        path,
                        fill,
                        stroke: None,
                        fill_rule: state.fill_rule,
                    };
                    Node::Group(Group {
                        transform: Transform2D::identity(),
                        opacity: 1.0,
                        clip: None,
                        children: vec![Node::Path(stroke_node), Node::Path(fill_node)],
                        cache_key: None,
                    })
                } else {
                    Node::Path(PathNode {
                        path,
                        fill,
                        stroke,
                        fill_rule: state.fill_rule,
                    })
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
                    children: vec![inner_node],
                    cache_key: None,
                })
            } else {
                inner_node
            };
            Some(inner)
        }
        #[cfg(feature = "text")]
        "text" => {
            let state = parent_state.merged_with_mctx(mctx, &ctx.stylesheet)?;
            // Round 228 — SVG 2 §13.10.3 `text-rendering` round-trip
            // capture. Mirrors the round-221 `shape-rendering` flow on
            // the shape branch: the cascade has already resolved the
            // property onto `state.text_rendering`, but the round-trip
            // carrier records the source-literal so a `parse → write`
            // cycle re-emits the author's keyword verbatim
            // (canonicalised to the spec's camelCase). Absent /
            // `inherit` / unrecognised tokens skip recording.
            ctx.pending_text_rendering = capture_text_rendering_attr(el);
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
    // Round 372 — SVG 2 §5.6: record the `<use>` reference identity at
    // the current scene-graph path so the encoder can collapse the
    // instantiated `Node::Group` back to `<use href="#id" …/>` on
    // round-trip. Only fires for a `<use>` that produced a node (an
    // unresolved / cyclic / external reference returned `None` above
    // and short-circuited before reaching here, so no binding is
    // recorded for those — they round-trip via the absence of a node).
    if tag_local(&el.name) == "use" {
        if let Some(b) = parse_use_binding(el) {
            ctx.record_use(b);
        }
    }
    // Round 372 — SVG 2 §5.7: record the verbatim `<switch>` at the
    // current scene-graph path so the encoder can collapse the selected
    // branch `Node::Group` back to the full `<switch>` (all
    // alternatives) on round-trip. Only fires for a `<switch>` that
    // produced a node (an all-fail switch returned `None` above and
    // short-circuited before reaching here, so no binding is recorded;
    // it round-trips via the absence of a node — matching the empty
    // render).
    if tag_local(&el.name) == "switch" {
        ctx.record_switch(el.clone());
    }
    // Round 449 — SVG 2 §11.2: record the verbatim `<text>` at the
    // current scene-graph path so the encoder can replace the flattened
    // glyph-outline node with the source text markup (string content,
    // font properties, `<tspan>` positioning arrays, `<textPath>`,
    // animation children) on round-trip. Only fires for a `<text>` that
    // produced a node — the text branch always yields a group (empty
    // when no font resolver is installed), so every parsed `<text>`
    // gains write-side fidelity.
    if tag_local(&el.name) == "text" {
        ctx.record_text(el.clone());
    }
    // Round 449 — SMIL Animation §3.1: record this element's direct
    // animation-element children at the current scene-graph path so the
    // encoder re-emits them as children of the matching node — keeping
    // the parent-target relationship for id-less parents the round-13
    // id-keyed routing orphaned (a detached `<animate>` at the trailing
    // edge has no target). `<text>` / `<switch>` are skipped: their
    // verbatim carriers already re-emit the animation children in
    // place, and recording here too would double-count the fragment
    // suppression the encoder performs.
    {
        let local = tag_local(&el.name);
        if local != "text" && local != "switch" {
            ctx.record_anim_targets(el);
        }
    }
    // Round 372 — SVG 1.1 §15: record the `filter="url(#id)"` reference
    // at the current scene-graph path (the topmost emit site, which is
    // the filter wrapper group when the ref resolved) so the encoder can
    // re-attach `filter=` on round-trip. Only record when the referenced
    // `<filter>` def actually exists — `apply_referenced_defs` wraps the
    // node in a pass-through group precisely then, so the binding has a
    // matching `<g>` emit site. An unresolved ref produced no wrapper,
    // so recording one would mis-tag an unrelated node.
    if let Some(filter_text) = attr(el, "filter") {
        if let Some(id) = parse_url_ref(filter_text) {
            if ctx.defs.filters.contains_key(id) {
                ctx.record_filter_ref(filter_text.to_string());
            }
        }
    }
    // Round 372 — SVG 2 §13.7.4: record any `marker-start` / `marker-mid`
    // / `marker-end` (or `marker` shorthand) references at the current
    // scene-graph path so the encoder re-attaches them on round-trip.
    // The recorder is a no-op when no marker reference is present, so a
    // plain shape records nothing. Recorded at the topmost emit site
    // (same slot as the round-13 `id_paths` / round-205+ hint carriers)
    // because the encoder writes presentation attributes on the topmost
    // node it produces for a shape.
    ctx.record_marker_refs(el);
    // Round 21 — drain the shape branch's pending `pathLength` (if
    // any) and record it at the **inner Path's** scene-graph slot.
    // The encoder emits `pathLength="..."` on the `<path>` element
    // itself (where SVG carries the attribute), not on a wrapping
    // `<g transform=...>` / `<g clip-path=...>` / `<g filter=...>` /
    // `Node::SoftMask`. `find_inner_path_subpath` walks the final
    // wrapped node and returns the child-index sub-path to the leaf.
    if let Some(pl) = ctx.pending_path_length.take() {
        if let Some(sub) = find_inner_path_subpath(&wrapped) {
            let save = ctx.current_path.len();
            for idx in &sub {
                ctx.current_path.push(*idx);
            }
            ctx.record_path_length(pl);
            ctx.current_path.truncate(save);
        }
    }
    // Round 449 — SVG 2 §9.2–§9.7 native shape identity. The shape
    // branch flattens every basic shape into path commands, so the
    // encoder used to emit `<path d="…">` — geometrically identical but
    // losing the element identity (an inlined
    // `<animate attributeName="x">` re-attached to a `<path>` targets
    // an attribute the element doesn't have; `rect { … }` type
    // selectors stop matching on re-parse). Record the source tag +
    // verbatim geometry attributes at the inner geometry node's slot
    // (same slot as the §9.6.1 `pathLength` carrier) so the encoder
    // emits the native tag instead. Only when the shape produced a
    // single unambiguous geometry node — the §13.8 stroke-first
    // `paint-order` split emits two single-purpose paths and keeps the
    // flattened form. Skipped inside `<use>` instantiation (the
    // collapsed instance never emits).
    if ctx.track_id_paths && ctx.use_stack.is_empty() {
        let local = tag_local(&el.name);
        let geom_names: Option<&[&str]> = match local.as_str() {
            "rect" => Some(&["x", "y", "width", "height", "rx", "ry"]),
            "circle" => Some(&["cx", "cy", "r"]),
            "ellipse" => Some(&["cx", "cy", "rx", "ry"]),
            "line" => Some(&["x1", "y1", "x2", "y2"]),
            "polyline" | "polygon" => Some(&["points"]),
            _ => None,
        };
        if let Some(names) = geom_names {
            if let Some(sub) = find_sole_path_subpath(&wrapped) {
                let attrs: Vec<(String, String)> = names
                    .iter()
                    .filter_map(|n| attr(el, n).map(|v| (n.to_string(), v.to_string())))
                    .collect();
                let mut path = ctx.current_path.clone();
                path.extend(sub);
                ctx.shapes.push(crate::preserved::ShapeBinding {
                    path,
                    tag: local.to_string(),
                    attrs,
                });
            }
        }
    }
    // Round 205 — drain the shape branch's pending `paint-order` (if
    // any) and record it. We target the **outer-most wrapping** the
    // shape produces — same emit site the round-13 `id_paths`
    // recorder uses — because the encoder's existing `path_to_id`
    // routing emits attributes on the topmost group / path it
    // produces for a given shape. A subsequent round can split the
    // paint-order out to the inner geometry path if that turns out
    // to be more faithful, but the current emitter writes
    // presentation attributes on the topmost node and the round-trip
    // matches that contract.
    if let Some(order) = ctx.pending_paint_order.take() {
        ctx.record_paint_order(order);
    }
    // Round 209 — drain the shape / `<use>` / `<g>` branch's pending
    // `vector-effect` (if any) and record it at the same outer-most
    // emit site the round-205 `paint-order` recorder uses. Per §8.13
    // the property applies to graphics elements and `<use>` but NOT
    // groups; we still capture a `<g vector-effect=…>` so a faithful
    // round-trip survives a hand-authored (off-spec) group attribute,
    // mirroring how `<g paint-order=…>` is captured for round-trip
    // even though paint-order is itself per-shape in the scene graph.
    if let Some(effect) = ctx.pending_vector_effect.take() {
        ctx.record_vector_effect(effect);
    }
    // Round 221 — drain the shape / `<g>` branch's pending
    // `shape-rendering` (if any) and record it at the same outer-most
    // emit site the round-205 / round-209 recorders use. The encoder
    // re-emits the source attribute on the matching shape / `<g>` on
    // round-trip; the actual rendering-hint consumption (anti-alias
    // toggle, edge snap) is `oxideav-raster`'s job.
    if let Some(sr) = ctx.pending_shape_rendering.take() {
        ctx.record_shape_rendering(sr);
    }
    // Round 228 — drain the `<text>` / `<g>` branch's pending
    // `text-rendering` (if any) and record it at the same outer-most
    // emit site the round-221 recorder uses. The encoder re-emits the
    // source attribute on the matching `<text>` / `<g>` on round-trip;
    // the actual hint consumption (anti-alias toggle, hint suspension)
    // is `oxideav-raster` / `oxideav-scribe` work.
    if let Some(tr) = ctx.pending_text_rendering.take() {
        ctx.record_text_rendering(tr);
    }
    // Round 247 — drain the shape / `<g>` branch's pending
    // `color-rendering` (if any) and record it at the same outer-most
    // emit site the round-221 / round-228 recorders use. The encoder
    // re-emits the source attribute on the matching shape / `<g>` on
    // round-trip; the actual hint consumption (working colour-space
    // selection for interpolation and compositing) lives in
    // `oxideav-raster`.
    if let Some(cr) = ctx.pending_color_rendering.take() {
        ctx.record_color_rendering(cr);
    }
    // Round 252 — drain the shape / `<g>` branch's pending
    // `color-interpolation` (if any) and record it at the same
    // outer-most emit site the round-221 / round-228 / round-247
    // recorders use. The encoder re-emits the source attribute on the
    // matching shape / `<g>` on round-trip; the actual working-colour-
    // space selection for gradient lerps / colour animation /
    // compositing happens in `oxideav-raster`.
    if let Some(ci) = ctx.pending_color_interpolation.take() {
        ctx.record_color_interpolation(ci);
    }
    // Round 257 — drain the shape / `<g>` branch's pending
    // `overflow` (if any) and record it at the same outer-most emit
    // site the §13.x / §3.11 lexical recorders use. The encoder
    // re-emits the source attribute on the matching shape / `<g>` on
    // round-trip; the actual clipping-rectangle establishment +
    // UA-stylesheet initial-value override happen in `oxideav-raster`.
    if let Some(o) = ctx.pending_overflow.take() {
        ctx.record_overflow(o);
    }
    // Round 260 — drain the shape / `<g>` branch's pending
    // `pointer-events` (if any) and record it at the same outer-most
    // emit site the §13.x / §3.11 lexical recorders use. The encoder
    // re-emits the source attribute on the matching shape / `<g>` on
    // round-trip; the actual hit-test gating (visibility + paint
    // suffix resolution per §15.6) happens in the interactive layer
    // (e.g. `oxideav-pipeline` event routing).
    if let Some(pe) = ctx.pending_pointer_events.take() {
        ctx.record_pointer_events(pe);
    }
    // Round 261 — drain the shape / `<g>` branch's pending `cursor`
    // (if any) and record it at the same outer-most emit site the
    // other lexical recorders use. The encoder re-emits the source
    // attribute on the matching shape / `<g>` on round-trip; the
    // actual cursor display (§16.8.2 funciri resolution + generic
    // fallback walk) is interactive-UA work.
    if let Some(c) = ctx.pending_cursor.take() {
        ctx.record_cursor(c);
    }
    // Round 291 — drain the shape / `<g>` branch's pending
    // `dominant-baseline` (if any) and record it at the same outer-most
    // emit site the other lexical recorders use. The encoder re-emits
    // the source attribute on the matching shape / `<g>` on round-trip;
    // the actual scaled-baseline-table construction + glyph positioning
    // (§10.9.2) live in `oxideav-scribe` / `oxideav-raster`.
    if let Some(db) = ctx.pending_dominant_baseline.take() {
        ctx.record_dominant_baseline(db);
    }
    Ok(Some(wrapped))
}

/// Round 205 — extract a canonicalised `paint-order` attribute from
/// `el` for round-trip preservation. Returns `Some(canonical)` when
/// the attribute carries a recognised non-`normal` keyword list;
/// returns `None` for an absent attribute, `normal`, `inherit`, or a
/// payload that didn't parse to any recognised keyword. Canonical
/// form lowercases the keywords, collapses whitespace to single
/// spaces, and drops duplicate keywords (preserving first
/// occurrence) per the §13.8 grammar.
fn capture_paint_order_attr(el: &Element) -> Option<String> {
    let raw = attr(el, "paint-order")?;
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("normal")
        || trimmed.eq_ignore_ascii_case("inherit")
    {
        return None;
    }
    let mut keywords: Vec<&'static str> = Vec::with_capacity(3);
    let mut seen = [false; 3];
    for tok in trimmed.split_ascii_whitespace() {
        let (k, i) = if tok.eq_ignore_ascii_case("fill") {
            ("fill", 0)
        } else if tok.eq_ignore_ascii_case("stroke") {
            ("stroke", 1)
        } else if tok.eq_ignore_ascii_case("markers") {
            ("markers", 2)
        } else {
            continue;
        };
        if !seen[i] {
            seen[i] = true;
            keywords.push(k);
        }
    }
    if keywords.is_empty() {
        return None;
    }
    Some(keywords.join(" "))
}

/// Round 209 — extract a canonicalised `vector-effect` attribute from
/// `el` for round-trip preservation. Returns `Some(canonical)` when
/// the attribute carries at least one recognised effect keyword;
/// returns `None` for an absent attribute, `none`, `inherit`, or a
/// payload that didn't parse to any recognised effect keyword.
///
/// Canonical form lowercases the keywords, collapses whitespace to
/// single spaces, drops duplicate keywords (preserving first
/// occurrence) per the `[ … ]+` CSS combinator rule, and appends a
/// host suffix (`viewport` / `screen`) only when the source explicitly
/// named one. The initial host value (`viewport`) is implied — emitting
/// it without source provenance would inflate every round-trip with a
/// redundant token.
fn capture_vector_effect_attr(el: &Element) -> Option<String> {
    let raw = attr(el, "vector-effect")?;
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("none")
        || trimmed.eq_ignore_ascii_case("inherit")
    {
        return None;
    }
    let mut keywords: Vec<&'static str> = Vec::with_capacity(4);
    let mut seen = [false; 4];
    let mut host: Option<&'static str> = None;
    for tok in trimmed.split_ascii_whitespace() {
        let (k, i) = if tok.eq_ignore_ascii_case("non-scaling-stroke") {
            ("non-scaling-stroke", 0)
        } else if tok.eq_ignore_ascii_case("non-scaling-size") {
            ("non-scaling-size", 1)
        } else if tok.eq_ignore_ascii_case("non-rotation") {
            ("non-rotation", 2)
        } else if tok.eq_ignore_ascii_case("fixed-position") {
            ("fixed-position", 3)
        } else if tok.eq_ignore_ascii_case("viewport") {
            host = Some("viewport");
            continue;
        } else if tok.eq_ignore_ascii_case("screen") {
            host = Some("screen");
            continue;
        } else {
            continue;
        };
        if !seen[i] {
            seen[i] = true;
            keywords.push(k);
        }
    }
    if keywords.is_empty() {
        // No effect keyword — §8.13 grammar requires at least one;
        // a payload of bare `viewport` / `screen` / unknown tokens
        // is not a valid `vector-effect` value, so we skip recording.
        return None;
    }
    let mut canon = keywords.join(" ");
    if let Some(h) = host {
        canon.push(' ');
        canon.push_str(h);
    }
    Some(canon)
}

/// Round 221 — extract a canonicalised `shape-rendering` attribute
/// from `el` for round-trip preservation. Returns `Some(canonical)`
/// when the attribute resolves to one of the four spec keywords;
/// returns `None` for an absent attribute, an `inherit` keyword, or
/// an unrecognised token (the cascade keeps the inherited value in
/// those cases, so the round-trip carrier matches the parse-time
/// fallback).
///
/// Canonical form is the spec's camelCase spelling (`auto` /
/// `optimizeSpeed` / `crispEdges` / `geometricPrecision`) — source
/// `OPTIMIZESPEED` round-trips as `optimizeSpeed`, matching the
/// §13.10.2 attribute table.
///
/// Unlike the round-205 `paint-order` / round-209 `vector-effect`
/// capture helpers (which skip the initial-value keyword to avoid
/// no-op binding bloat), the `auto` keyword IS recorded when the
/// author explicitly wrote it — a hand-authored
/// `shape-rendering="auto"` is a deliberate annotation (e.g. an
/// inheritance reset on a descendant of a `<g shape-rendering=
/// "optimizeSpeed">`) and round-tripping that intent is more
/// faithful than silently dropping it. The absent-attribute case is
/// still skipped (no binding) so an initial-value document doesn't
/// bloat the output with redundant `shape-rendering="auto"` on every
/// shape.
fn capture_shape_rendering_attr(el: &Element) -> Option<String> {
    let raw = attr(el, "shape-rendering")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("inherit") {
        return None;
    }
    let sr = ShapeRendering::parse_keyword(trimmed)?;
    Some(sr.as_canonical_str().to_string())
}

/// Round 228 — extract a canonicalised `text-rendering` attribute from
/// `el` for round-trip preservation. Returns `Some(canonical)` when the
/// attribute resolves to one of the four §13.10.3 keywords; returns
/// `None` for an absent attribute, an `inherit` keyword, or an
/// unrecognised token (the cascade keeps the inherited value in those
/// cases, so the round-trip carrier matches the parse-time fallback).
///
/// Canonical form is the spec's camelCase spelling (`auto` /
/// `optimizeSpeed` / `optimizeLegibility` / `geometricPrecision`) —
/// source `OPTIMIZESPEED` round-trips as `optimizeSpeed`, matching the
/// §13.10.3 attribute table.
///
/// Like the round-221 `shape-rendering` helper (and unlike the
/// round-205 / round-209 helpers which skip the initial-value keyword
/// to avoid no-op binding bloat), an explicit author
/// `text-rendering="auto"` IS recorded — it carries author intent
/// (e.g. an inheritance reset on a `<text>` descendant of a
/// `<g text-rendering="optimizeLegibility">`). The absent-attribute
/// case is still skipped so an initial-value document doesn't bloat
/// the output with redundant `text-rendering="auto"` on every `<text>`.
fn capture_text_rendering_attr(el: &Element) -> Option<String> {
    let raw = attr(el, "text-rendering")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("inherit") {
        return None;
    }
    let tr = TextRendering::parse_keyword(trimmed)?;
    Some(tr.as_canonical_str().to_string())
}

/// Round 247 — extract a canonicalised `color-rendering` attribute from
/// `el` for round-trip preservation. Returns `Some(canonical)` when the
/// attribute resolves to one of the three §13.10.1 keywords; returns
/// `None` for an absent attribute, an `inherit` keyword, or an
/// unrecognised token (the cascade keeps the inherited value in those
/// cases, so the round-trip carrier matches the parse-time fallback).
///
/// Canonical form is the spec's camelCase spelling (`auto` /
/// `optimizeSpeed` / `optimizeQuality`) — source `OPTIMIZESPEED`
/// round-trips as `optimizeSpeed`, matching the §13.10.1 attribute
/// table.
///
/// Like the round-221 `shape-rendering` / round-228 `text-rendering` /
/// round-235 `image-rendering` helpers, an explicit author
/// `color-rendering="auto"` IS recorded — it carries author intent
/// (e.g. an inheritance reset on a descendant of a
/// `<g color-rendering="optimizeQuality">`). The absent-attribute case
/// is still skipped so an initial-value document doesn't bloat the
/// output with redundant `color-rendering="auto"` on every element.
fn capture_color_rendering_attr(el: &Element) -> Option<String> {
    let raw = attr(el, "color-rendering")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("inherit") {
        return None;
    }
    let cr = ColorRendering::parse_keyword(trimmed)?;
    Some(cr.as_canonical_str().to_string())
}

/// Round 252 — extract a canonicalised `color-interpolation` attribute
/// from `el` for round-trip preservation. Returns `Some(canonical)` when
/// the attribute resolves to one of the three §13.9 keywords (`auto`,
/// `sRGB`, `linearRGB`); returns `None` for an absent attribute, an
/// `inherit` keyword, or an unrecognised token (the cascade keeps the
/// inherited value in those cases, so the round-trip carrier matches
/// the parse-time fallback).
///
/// Canonical form is the §13.9 spelling (`auto` / `sRGB` / `linearRGB`)
/// — source `SRGB` / `srgb` / `LINEARRGB` all round-trip as the
/// canonical mixed-case spelling so the output matches the §13.9
/// attribute table verbatim.
///
/// Like the round-247 `color-rendering` helper, an explicit author
/// `color-interpolation="sRGB"` IS recorded — even though `sRGB` is the
/// §13.9 initial value, an explicit author write carries intent (e.g.
/// an inheritance reset on a descendant of a
/// `<g color-interpolation="linearRGB">`). The absent-attribute case is
/// still skipped so an initial-value document doesn't bloat the output
/// with redundant `color-interpolation="sRGB"` on every element.
fn capture_color_interpolation_attr(el: &Element) -> Option<String> {
    let raw = attr(el, "color-interpolation")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("inherit") {
        return None;
    }
    let ci = ColorInterpolation::parse_keyword(trimmed)?;
    Some(ci.as_canonical_str().to_string())
}

/// Round 257 — extract a canonicalised `overflow` attribute from
/// `el` for round-trip preservation. Returns `Some(canonical)` when
/// the attribute resolves to one of the four §3.11 keywords
/// (`visible` / `hidden` / `scroll` / `auto`); returns `None` for an
/// absent attribute, an `inherit` keyword, or an unrecognised token
/// (the cascade keeps the inherited — or, since `overflow` is not
/// inherited, the per-element-reset initial — value in those cases,
/// so the round-trip carrier matches the parse-time fallback).
///
/// Canonical form is the §3.11 spelling (all lowercase, matching the
/// CSS 2.1 keyword set verbatim) — source `HIDDEN` / `Hidden`
/// round-trip as `hidden`.
///
/// Like the round-221 / round-228 / round-247 / round-252 helpers,
/// an explicit author `overflow="visible"` IS recorded — even though
/// `visible` is the §3.11 initial value, an explicit author write
/// carries intent (e.g. an override of the UA stylesheet's
/// `hidden` default that fires for non-root `<svg>` / `<symbol>` /
/// `<marker>` / `<pattern>` / `<image>`, per §3.11). The
/// absent-attribute case is still skipped so an initial-value
/// document doesn't bloat the output with redundant
/// `overflow="visible"` on every element.
fn capture_overflow_attr(el: &Element) -> Option<String> {
    let raw = attr(el, "overflow")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("inherit") {
        return None;
    }
    let o = Overflow::parse_keyword(trimmed)?;
    Some(o.as_canonical_str().to_string())
}

/// Round 260 — extract a canonicalised `pointer-events` attribute from
/// `el` for round-trip preservation. Returns `Some(canonical)` when
/// the attribute resolves to one of the ten §15.6 keywords
/// (`bounding-box | visiblePainted | visibleFill | visibleStroke |
/// visible | painted | fill | stroke | all | none`); returns `None`
/// for an absent attribute, an `inherit` keyword, or an unrecognised
/// token (the cascade keeps the inherited value in those cases, so
/// the round-trip carrier matches the parse-time fallback).
///
/// Canonical form is the §15.6 spelling — lower-camelCase for the four
/// `visible*` keywords, hyphenated for `bounding-box`, all-lowercase
/// for the rest. Source `VISIBLEPAINTED` / `BOUNDING-BOX` / `Painted`
/// round-trip as `visiblePainted` / `bounding-box` / `painted`.
///
/// Like the round-221 / round-228 / round-247 / round-252 / round-257
/// helpers, an explicit author `pointer-events="visiblePainted"` IS
/// recorded — even though `visiblePainted` is the §15.6 initial value,
/// an explicit author write carries intent (e.g. an inheritance reset
/// on a descendant of a `<g pointer-events="none">`). The
/// absent-attribute case is still skipped so an initial-value document
/// doesn't bloat the output with redundant
/// `pointer-events="visiblePainted"` on every element.
fn capture_pointer_events_attr(el: &Element) -> Option<String> {
    let raw = attr(el, "pointer-events")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("inherit") {
        return None;
    }
    let pe = PointerEvents::parse_keyword(trimmed)?;
    Some(pe.as_canonical_str().to_string())
}

/// Round 261 — extract a canonicalised `cursor` attribute from `el`
/// for round-trip preservation. Returns `Some(canonical)` when the
/// attribute parses per the SVG 1.1 §16.8.2 grammar
/// (`[ [<funciri> ,]* [ <generic keyword> ] ]`); returns `None` for an
/// absent attribute, an `inherit` keyword, or an invalid payload
/// (unknown trailing keyword, malformed funciri, funciri list without
/// the mandatory trailing generic keyword) — the cascade keeps the
/// inherited value in those cases, so the round-trip carrier matches
/// the parse-time fallback.
///
/// Canonical form: funciris in source order (`url` token lowercased,
/// IRI verbatim), comma-and-space separated, followed by the lowercase
/// generic keyword — matching the §16.8.2 example list shape.
///
/// Like the round-221 .. round-260 helpers, an explicit author
/// `cursor="auto"` IS recorded — even though `auto` is the §16.8.2
/// initial value, an explicit author write carries intent (e.g. an
/// inheritance reset on a descendant of a `<g cursor="wait">`). The
/// absent-attribute case is still skipped so an initial-value document
/// doesn't bloat the output with redundant `cursor="auto"` on every
/// element.
fn capture_cursor_attr(el: &Element) -> Option<String> {
    let raw = attr(el, "cursor")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("inherit") {
        return None;
    }
    let c = CursorValue::parse_custom(trimmed)?;
    Some(c.as_canonical_string())
}

/// Round 291 — extract a canonicalised `dominant-baseline` attribute
/// from `el` for round-trip preservation. Returns `Some(canonical)`
/// when the attribute resolves to one of the twelve §10.9.2 keywords
/// (`auto | use-script | no-change | reset-size | ideographic |
/// alphabetic | hanging | mathematical | central | middle |
/// text-after-edge | text-before-edge`); returns `None` for an absent
/// attribute, an `inherit` keyword, or an unrecognised token (the
/// cascade keeps the inherited — or, since `dominant-baseline` is not
/// inherited, the per-element-reset initial — value in those cases, so
/// the round-trip carrier matches the parse-time fallback).
///
/// Canonical form is the §10.9.2 spelling (all lowercase, hyphenated
/// for the multi-word keywords) — source `HANGING` / `TEXT-AFTER-EDGE`
/// round-trip as `hanging` / `text-after-edge`.
///
/// Like the round-221 / round-247 / round-257 helpers, an explicit
/// author `dominant-baseline="auto"` IS recorded — even though `auto`
/// is the §10.9.2 initial value, an explicit author write carries
/// intent (e.g. an inheritance reset on a child of a
/// `<text dominant-baseline="hanging">`). The absent-attribute case is
/// still skipped so an initial-value document doesn't bloat the output
/// with redundant `dominant-baseline="auto"` on every element.
fn capture_dominant_baseline_attr(el: &Element) -> Option<String> {
    let raw = attr(el, "dominant-baseline")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("inherit") {
        return None;
    }
    let db = DominantBaseline::parse_keyword(trimmed)?;
    Some(db.as_canonical_str().to_string())
}

/// Round 449 — like [`find_inner_path_subpath`] but strict: descends
/// only single-child wrappers and requires exactly one terminal
/// geometry `Path`. The §13.8 stroke-first `paint-order` split (two
/// single-purpose paths for one source shape) returns `None`, so the
/// native-shape-identity carrier never binds an ambiguous emit site. A
/// `SoftMask` wrapper shares its scene-graph slot with the masked
/// content (the encoder pushes no extra index for it), so it is
/// descended without extending the sub-path — the native shape then
/// emits inside the wrapper's `<g mask="url(#…)">`.
fn find_sole_path_subpath(node: &Node) -> Option<Vec<usize>> {
    match node {
        Node::Path(_) => Some(Vec::new()),
        Node::Group(g) if g.children.len() == 1 => {
            let inner = find_sole_path_subpath(&g.children[0])?;
            let mut out = Vec::with_capacity(inner.len() + 1);
            out.push(0);
            out.extend(inner);
            Some(out)
        }
        Node::SoftMask { content, .. } => find_sole_path_subpath(content),
        _ => None,
    }
}

/// Round 21 — return the child-index sub-path from `root` down to the
/// first [`Node::Path`] leaf, matching what the encoder pushes on its
/// `path_stack` while walking the same node tree. Used to attach an
/// author-supplied `pathLength` to the right emit site even when
/// `apply_referenced_defs` (clip / mask / filter) added wrappers
/// after the shape branch.
///
/// * `Node::Path` → returns the empty path (this node is the leaf).
/// * `Node::Group` → descends into `children[0]` with index 0
///   prepended. (Shape wrappers built by the shape branch are
///   single-child groups, so `children[0]` is always the inner.)
/// * `Node::SoftMask` → descends into `content` **without** adding an
///   index, matching the encoder's `write_node` arm for SoftMask
///   which doesn't push an extra index for the wrapped content.
/// * Anything else → returns `None`.
fn find_inner_path_subpath(node: &Node) -> Option<Vec<usize>> {
    match node {
        Node::Path(_) => Some(Vec::new()),
        Node::Group(g) => {
            // Single-child shape wrappers (the round-2/3 case) — descend.
            // Round 205 — when the §13.8 paint-order split produces a
            // two-child group (stroke-only then fill-only), target the
            // stroke-bearing child so the §9.6.1 `pathLength` dash
            // rescaling attaches to the path that carries the stroke.
            // Otherwise (>= 2 children with no recognised paint-order
            // shape) we can't disambiguate, so bail.
            if g.children.len() == 1 {
                let inner = find_inner_path_subpath(&g.children[0])?;
                let mut out = Vec::with_capacity(inner.len() + 1);
                out.push(0);
                out.extend(inner);
                Some(out)
            } else if g.children.len() == 2 {
                if let (Node::Path(a), Node::Path(b)) = (&g.children[0], &g.children[1]) {
                    let stroke_idx = if a.stroke.is_some() && a.fill.is_none() {
                        Some(0)
                    } else if b.stroke.is_some() && b.fill.is_none() {
                        Some(1)
                    } else {
                        None
                    };
                    if let Some(i) = stroke_idx {
                        return Some(vec![i]);
                    }
                }
                None
            } else {
                None
            }
        }
        Node::SoftMask { content, .. } => find_inner_path_subpath(content),
        _ => None,
    }
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
///
/// Round 215 — SVG 1.1 §14.3.5 `clip-rule`. The property cascades from
/// the `<clipPath>` element into its child shapes (it is an inherited
/// property per the §14.3.5 attribute table). Each child shape's own
/// `clip-rule=` overrides the inherited value. The resolved rule for
/// the merged path is the **first child shape's** rule (subsequent
/// children that disagree are tolerated but the merged path can carry
/// only one rule — the author's original per-child attribute is
/// preserved on the round-trip side-channel by
/// [`crate::decoder::parse_svg_with_extras`]). Initial value `nonzero`.
pub fn parse_clip_path_def(
    el: &Element,
    ctx: &mut ParseContext,
) -> Result<Option<(String, ClipPathDef)>> {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    // Round 215 — inherited rule from the `<clipPath>` element itself.
    // The `<clipPath>` is a never-rendered def, but `clip-rule` is an
    // inherited presentation property per SVG 1.1 §14.3.5, so a value
    // declared on the parent reaches the child shape unless that shape
    // overrides it. Unknown / malformed values fall back to the initial
    // `nonzero` rather than failing the document.
    let inherited_rule = parse_clip_rule_attr(attr(el, "clip-rule")).unwrap_or(FillRule::NonZero);
    let mut path = Path::new();
    let mut resolved_rule: Option<FillRule> = None;
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
                // Round 215 — record the rule contributed by the first
                // shape child that actually adds geometry. Subsequent
                // children that disagree are tolerated; the merged path
                // can only honour one rule at the rasterizer.
                if resolved_rule.is_none() {
                    resolved_rule =
                        Some(parse_clip_rule_attr(attr(c, "clip-rule")).unwrap_or(inherited_rule));
                }
            }
        }
    }
    if path.commands.is_empty() {
        return Ok(None);
    }
    let _ = ctx;
    Ok(Some((
        id,
        ClipPathDef {
            path,
            clip_rule: resolved_rule.unwrap_or(inherited_rule),
        },
    )))
}

/// Round 215 — parse a `clip-rule` attribute value into a typed
/// [`FillRule`] per SVG 1.1 §14.3.5. `nonzero` / `evenodd` only; any
/// other token (including `inherit`, the empty string, or unknown
/// payloads) returns `None` so the caller can fall back to the
/// inherited / initial value.
pub(crate) fn parse_clip_rule_attr(value: Option<&str>) -> Option<FillRule> {
    let v = value?.trim();
    if v.eq_ignore_ascii_case("nonzero") {
        Some(FillRule::NonZero)
    } else if v.eq_ignore_ascii_case("evenodd") {
        Some(FillRule::EvenOdd)
    } else {
        // `inherit` and unrecognised tokens fall through; callers
        // resolve those against the inherited / initial value.
        None
    }
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
    // SVG 2 §5.5 — `x` / `y` geometry properties position the symbol's
    // viewport. Absent → `None` (treated as 0 at instantiation).
    let intrinsic_x = parse_optional_length(attr(el, "x"))?;
    let intrinsic_y = parse_optional_length(attr(el, "y"))?;
    // SVG 2 §5.5 — `refX` / `refY` reference point. Absent → `None`
    // (no reference-point offset). The geometric keywords resolve
    // against the `viewBox` extent, reusing the `<marker>` helper; a
    // numeric value is a coordinate in the symbol's own coordinate
    // system.
    let ref_x =
        attr(el, "refX").map(|s| parse_marker_ref(Some(s), view_box.map(|v| (v.min_x, v.width))));
    let ref_y =
        attr(el, "refY").map(|s| parse_marker_ref(Some(s), view_box.map(|v| (v.min_y, v.height))));
    Ok(Some((
        id,
        SymbolDef {
            content: group,
            view_box,
            preserve_aspect_ratio,
            intrinsic_width,
            intrinsic_height,
            intrinsic_x,
            intrinsic_y,
            ref_x,
            ref_y,
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

/// Round 104 — parse `<marker id="...">` into a typed
/// [`crate::defs::MarkerDef`]. SVG 2 §13.7.1.
///
/// Returns `None` if the element lacks an `id` (then it can't be
/// referenced via `marker-*="url(#id)"`). The marker content (shapes /
/// groups / nested gradients / use) is parsed using the existing element
/// pipeline so a `<marker>` containing a `<path>` round-trips faithfully.
///
/// `refX` / `refY` accept the SVG-2 geometric keywords
/// (`left` / `center` / `right` for `refX`; `top` / `center` / `bottom`
/// for `refY`); these resolve to a percentage of the `viewBox`
/// width / height per the §13.7.1 mapping table (`left`/`top` 0%,
/// `center` 50%, `right`/`bottom` 100%). Without a `viewBox` the keyword
/// has no width/height to resolve against and falls back to 0.
pub fn parse_marker_def(
    el: &Element,
    ctx: &mut ParseContext,
) -> Result<Option<(String, crate::defs::MarkerDef)>> {
    use crate::defs::{MarkerDef, MarkerOrient, MarkerUnits};
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };

    let view_box = match attr(el, "viewBox") {
        Some(s) => parse_symbol_view_box(s),
        None => None,
    };
    // refX/refY default to 0 per §13.7.1; geometric keywords resolve
    // against the viewBox.
    let ref_x = parse_marker_ref(attr(el, "refX"), view_box.map(|v| (v.min_x, v.width)));
    let ref_y = parse_marker_ref(attr(el, "refY"), view_box.map(|v| (v.min_y, v.height)));
    // markerWidth / markerHeight default to 3 per §13.7.1.
    let marker_width = parse_number(attr(el, "markerWidth"), 3.0)?;
    let marker_height = parse_number(attr(el, "markerHeight"), 3.0)?;
    let marker_units = MarkerUnits::parse(attr(el, "markerUnits"));
    let orient = MarkerOrient::parse(attr(el, "orient"));
    let preserve_aspect_ratio = match attr(el, "preserveAspectRatio") {
        Some(s) => crate::filter::PreserveAspectRatio::from_str(s),
        None => crate::filter::PreserveAspectRatio::default(),
    };

    // Parse the marker content using the standard pipeline. Per §13.7.1
    // the content model allows shapes / groups / use / nested paint
    // servers; reuse the element parser so we get all of them (any
    // nested defs were already registered into `ctx` by the pre-walk).
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
        MarkerDef {
            ref_x,
            ref_y,
            marker_width,
            marker_height,
            marker_units,
            orient,
            view_box,
            preserve_aspect_ratio,
            content,
        },
    )))
}

/// Parse a `refX` / `refY` value per SVG 2 §13.7.1. Accepts a
/// `<number>` (user units) or one of the geometric keywords. Keywords
/// resolve to `min + percentage * extent` where `(min, extent)` is the
/// viewBox `(min_x, width)` for `refX` or `(min_y, height)` for `refY`;
/// `None` extent (no viewBox) collapses a keyword to 0. An absent /
/// malformed value defaults to 0.
fn parse_marker_ref(v: Option<&str>, extent: Option<(f32, f32)>) -> f32 {
    let s = match v {
        None => return 0.0,
        Some(s) => s.trim(),
    };
    // Geometric keywords (§13.7.1 mapping table). `left`/`top` → 0%,
    // `center` → 50%, `right`/`bottom` → 100% of the viewBox extent.
    let pct = match s {
        "left" | "top" => Some(0.0),
        "center" => Some(0.5),
        "right" | "bottom" => Some(1.0),
        _ => None,
    };
    if let Some(p) = pct {
        return match extent {
            Some((min, ext)) => min + p * ext,
            None => 0.0,
        };
    }
    // Otherwise a number (a trailing unit suffix is tolerated by
    // `parse_number` returning the default; markers are user-unit-only
    // in practice).
    parse_number(v, 0.0).unwrap_or(0.0)
}

/// Round 95 — parse `<view id="...">` into a typed
/// [`crate::defs::ViewDef`]. SVG 2 §16.3.3.
///
/// The element only contributes when it carries an `id` — without one
/// no fragment identifier can address it. Per §16.3.3 the three
/// view-relevant attributes are `viewBox`, `preserveAspectRatio`, and
/// `zoomAndPan`; descriptive children (`<title>` / `<desc>` /
/// `<metadata>`) are not consumed here (the verbatim XML round-trip
/// channel preserves them via [`crate::preserved::PreservedExtras::views`]).
///
/// Returns `None` when the element has no `id` (then it can't be
/// referenced via `MyDrawing.svg#view-id`).
pub fn parse_view_def(el: &Element) -> Option<(String, crate::defs::ViewDef)> {
    let id = attr(el, "id")?.to_string();
    let view_box = attr(el, "viewBox").and_then(parse_symbol_view_box);
    let preserve_aspect_ratio =
        attr(el, "preserveAspectRatio").map(crate::filter::PreserveAspectRatio::from_str);
    let zoom_and_pan = attr(el, "zoomAndPan").map(crate::defs::ZoomAndPan::from_str);
    Some((
        id,
        crate::defs::ViewDef {
            view_box,
            preserve_aspect_ratio,
            zoom_and_pan,
        },
    ))
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
///
/// Round 403 — the total number of `<use>` instantiations one decode
/// will perform. Guards against exponential diamond-expansion (see
/// [`ParseContext::use_expansions`]). Set generously — real documents
/// instantiate at most a few thousand instances — while still cutting a
/// 2ⁿ blow-up short after a bounded amount of work.
pub const MAX_USE_EXPANSIONS: usize = 100_000;

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

    // Round 403 — bound the total `<use>` expansion. A diamond of
    // mutually-referencing `<use>`s expands exponentially even though no
    // id repeats on the instantiation path (so `use_stack` never trips).
    // Once the running total reaches the budget, stop instantiating
    // further references: the partial tree is a bounded, safe result for
    // what is invariably an adversarial document (real SVG never nests
    // this many instances).
    if ctx.use_expansions >= MAX_USE_EXPANSIONS {
        return Ok(None);
    }
    ctx.use_expansions += 1;
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
        let (sym_view_box, sym_par, sym_w, sym_h, sym_x, sym_y, sym_ref_x, sym_ref_y) =
            match &sym_meta {
                Some(s) => (
                    s.view_box,
                    s.preserve_aspect_ratio,
                    s.intrinsic_width,
                    s.intrinsic_height,
                    s.intrinsic_x,
                    s.intrinsic_y,
                    s.ref_x,
                    s.ref_y,
                ),
                None => (
                    None,
                    crate::filter::PreserveAspectRatio::default(),
                    None,
                    None,
                    None,
                    None,
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
                let mut vp = symbol_viewport_transform(w, h, vb, sym_par);
                // SVG 2 §5.5 — `refX` / `refY` align the symbol's
                // reference point (given in the symbol's own coordinate
                // system) with the use's `x` / `y`. Mirroring the
                // `<marker>` rule, the reference point is mapped through
                // the viewport transform and the result is subtracted so
                // that point lands at the viewport origin (which the
                // outer group's `translate(x, y)` then positions).
                if sym_ref_x.is_some() || sym_ref_y.is_some() {
                    let rx = sym_ref_x.unwrap_or(0.0);
                    let ry = sym_ref_y.unwrap_or(0.0);
                    let mapped = apply_xform(&vp, rx, ry);
                    vp = Transform2D::translate(-mapped.x, -mapped.y).compose(&vp);
                }
                // SVG 2 §5.5 — the symbol's own `x` / `y` geometry
                // properties position its viewport inside the use's
                // coordinate system ("the same effect as on an `svg`
                // element"). Applied outside the viewport transform; the
                // use's own `x` / `y` translate is layered on top by the
                // outer group below.
                if sym_x.is_some() || sym_y.is_some() {
                    let sx = sym_x.unwrap_or(0.0);
                    let sy = sym_y.unwrap_or(0.0);
                    vp = Transform2D::translate(sx, sy).compose(&vp);
                }
                viewport_transform = Some(vp);
            }
        }
        for child in &source.children {
            if let XmlNode::Element(c) = child {
                if let Some(node) = parse_element_to_node(c, &state, ctx)? {
                    children.push(node);
                }
            }
        }
    } else {
        // Round 118 — `<use>` references its target *directly*: per
        // SVG 1.1 §11.5 a `display:none` reference target still renders
        // when instantiated. Mark the next element (the instance root)
        // exempt from the `display:none` drop. (Symbol children above
        // are NOT exempted — only the named reference target is.)
        ctx.use_instance_root_pending = true;
        let node = parse_element_to_node(&source, &state, ctx)?;
        // Defensive: clear in case the source root was a never-rendered
        // element that returned `None` without consuming the flag.
        ctx.use_instance_root_pending = false;
        if let Some(node) = node {
            children.push(node);
        }
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
    // §8.2 translate. The trailing `translate(-vb.min_x, -vb.min_y)`
    // already maps the viewBox origin to (0,0), so this term carries
    // *only* the meet/slice alignment offset (`dx/2`, `dx`, …) — never
    // the `-min·scale` term (round 375 fix: the prior `-vb.min_x * sx`
    // seed double-counted the min translation, so a `<symbol>` /
    // `<use>` viewBox with a non-zero `min-x` / `min-y` was shifted by
    // an extra `min·scale`; documents with the usual `min=0` viewBox
    // were unaffected, which is why it went unnoticed).
    let (mut tx, mut ty) = (0.0_f32, 0.0_f32);
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
/// / `<animateMotion>` tags, evaluate each at `t_seconds`, and return
/// a clone of `el` with the snapshot values spliced into its attrs
/// (taking precedence over any existing attribute of the same name).
/// Returns `None` when there are no animation children, so the caller
/// can keep using the original element by reference for the common
/// case.
///
/// Round 125: `id_lookup` resolves `<mpath xlink:href="#id">`
/// references inside an `<animateMotion>` child to the corresponding
/// source `<path>` element. The caller should pass the
/// `ParseContext::defs::elements` table that the pre-walk already
/// populated.
fn apply_animation_overrides(
    el: &Element,
    t_seconds: f32,
    id_lookup: &std::collections::HashMap<String, Element>,
) -> Option<Element> {
    let overrides =
        crate::animation::snapshot_children_with_resolver(el, t_seconds, &|id| id_lookup.get(id));
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
