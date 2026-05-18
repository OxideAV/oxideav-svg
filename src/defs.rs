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

use oxideav_core::{Group, Path, Transform2D, ViewBox};

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

    #[test]
    fn parse_url_ref_extracts_id() {
        assert_eq!(parse_url_ref("url(#foo)"), Some("foo"));
        assert_eq!(parse_url_ref("  url(#bar)  "), Some("bar"));
        assert_eq!(parse_url_ref("url( #spc )"), Some("spc"));
        assert_eq!(parse_url_ref("none"), None);
        assert_eq!(parse_url_ref("#foo"), None);
        assert_eq!(parse_url_ref("url(foo)"), None);
    }
}
