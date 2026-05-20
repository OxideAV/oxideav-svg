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

use oxideav_core::{GradientStop, Group, Path, SpreadMethod, Transform2D, ViewBox};

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
#[derive(Clone, Debug)]
pub struct ClipPathDef {
    pub path: Path,
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
