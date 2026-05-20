//! Round 4 — encoder-side preservation of source XML elements that
//! `oxideav_core::Node` doesn't represent natively.
//!
//! The decoder produces a [`VectorFrame`] whose scene graph holds only
//! shapes / groups / soft-masks / images. Round 1-3 already throws
//! away the original `<style>`, `<filter>`, `<animate>` definitions —
//! they're either consumed (snapshot at t=0) or held only as parser
//! side tables. After `parse → write_svg`, a round-trip therefore loses
//! the dynamic / filter / CSS pieces.
//!
//! Round 4 introduces an out-of-band [`PreservedExtras`] container.
//! Callers who care about lossless round-tripping use the paired API:
//!
//! ```ignore
//! let (frame, extras) = oxideav_svg::parse_svg_with_extras(bytes)?;
//! // ...mutate frame...
//! let bytes = oxideav_svg::write_svg_with_extras(&frame, &extras);
//! ```
//!
//! `extras` carries each `<style>`, `<filter>`, `<animate>` (and
//! related) element in serialised XML form. The encoder splices them
//! back into the output document so the rasterised representation +
//! the dynamic definitions both survive.

use crate::image::SvgImage;
use crate::parser::Element;

/// Side-channel buffer of source-XML fragments the encoder needs to
/// re-emit alongside the [`VectorFrame`] scene graph.
///
/// Populated by [`crate::decoder::parse_svg_with_extras`] during the
/// document pre-walk; consumed by
/// [`crate::encoder::write_svg_with_extras`].
#[derive(Clone, Debug, Default)]
pub struct PreservedExtras {
    /// `<style>` element bodies (CSS source). Kept verbatim so an
    /// inline `<style>` block survives round-trip even though the
    /// decoder also fold the rules into [`crate::css::Stylesheet`].
    pub styles: Vec<String>,
    /// `<filter>` element trees, captured verbatim. Filter primitives
    /// (`feGaussianBlur`, `feColorMatrix`, …) aren't yet rasterised
    /// here, but preserving them means downstream consumers still see
    /// the definition.
    pub filters: Vec<Element>,
    /// `<animate>` / `<set>` / `<animateTransform>` elements that were
    /// children of an id-bearing parent in the source document. Stored
    /// alongside the parent's id so the encoder can re-attach them to
    /// the correct emitted node.
    ///
    /// `parent_path` is a list of (tag_local, optional id) pairs from
    /// the document root down to the immediate parent of the animation
    /// — used by the encoder to route animation re-emission. Round 4
    /// only attaches animations to root-level path / group emit sites
    /// keyed by id; deeper or unkeyed parents are dropped (a
    /// followup).
    pub animations: Vec<AnimationFragment>,
    /// `<foreignObject>` element trees, captured verbatim. Round 4
    /// renders them as an empty group on the rasterised side; this
    /// preserves the source so XHTML embeddings survive.
    pub foreign_objects: Vec<Element>,
    /// `<script>` element bodies (JavaScript / ECMAScript source),
    /// captured verbatim. Round 12 — the parser treats `<script>` as a
    /// raw-text element (HTML5 "script data state") so unescaped `<`
    /// inside the body doesn't poison the rest of the document; the
    /// body is then stowed here for round-trip emission. The decoder
    /// does NOT execute scripts (oxideav has no JS engine and SVG
    /// produced via this crate is intended to be statically rendered).
    pub scripts: Vec<Element>,
    /// Round 12 — verbatim text of the root `<svg>` element's
    /// `preserveAspectRatio` attribute (e.g. `"xMinYMid slice"`). The
    /// decoder bakes the spec-mandated mapping into
    /// [`oxideav_core::VectorFrame::root.transform`] so rasterisers
    /// without aspect-ratio knowledge produce the correct visual
    /// result; this side-channel preserves the original keyword pair
    /// so the encoder can re-emit it verbatim.
    pub root_preserve_aspect_ratio: Option<String>,
    /// Round 13 — scene-graph tree-paths of source-id-bearing nodes.
    /// Each entry maps a source SVG `id="..."` to the
    /// [`Vec<usize>`] tree-path of the corresponding scene-graph node
    /// (where each `usize` is a child index from the root group, top to
    /// bottom). The encoder uses this to:
    ///
    /// 1. Re-emit the original `id="..."` attribute on each emitted
    ///    `<g>` / `<path>` so downstream tooling can still address the
    ///    element by its source name.
    /// 2. Re-attach captured `<animate>` / `<set>` /
    ///    `<animateTransform>` fragments as children of their declared
    ///    parent element instead of dumping them at the trailing edge of
    ///    the document with a parent-id comment hint (the round-12
    ///    fallback).
    ///
    /// Built only by [`crate::decoder::parse_svg_with_extras`]. Empty
    /// for documents that have no id-bearing elements.
    pub id_paths: Vec<IdScenePath>,
    /// Round 20 — `<pattern>` paint-server definitions captured
    /// verbatim from the source SVG. The decoder also stores a typed
    /// view on [`crate::defs::DefsTables::patterns`] for downstream
    /// consumption, but the verbatim element here is the source of
    /// truth for round-trip emission (preserves attribute ordering,
    /// nested defs, and any attributes the typed view doesn't yet
    /// model). The encoder re-emits each in a `<defs>` block so a
    /// `parse → write` preserves the paint server.
    pub patterns: Vec<Element>,
    /// Round 81 — `<linearGradient>` / `<radialGradient>` paint-server
    /// definitions captured verbatim from the source SVG. The typed
    /// view on [`crate::defs::DefsTables::gradients`] holds the
    /// resolved geometry / template chain, but the verbatim element
    /// here is the round-trip source of truth: it preserves
    /// `gradientUnits` / `gradientTransform` / `href` /
    /// `xlink:href` exactly as authored, plus any author-specified
    /// attribute ordering. The encoder re-emits each in a `<defs>`
    /// block so a `parse → write_svg_with_extras` round-trip preserves
    /// the paint server alongside the flattened scene graph.
    pub gradients: Vec<Element>,
    /// Round 15 — `<image>` elements captured from the source SVG.
    ///
    /// Each entry holds the parsed [`SvgImage`] view (decoded inline
    /// data URIs, external URLs verbatim, x/y/width/height,
    /// transform). The encoder re-emits each image as an `<image>`
    /// element at the trailing edge of the document, preserving the
    /// data URI / external URL and dimensions for round-trip.
    ///
    /// `oxideav_core::Node::Image` requires a fully-decoded
    /// `VideoFrame`; round 15 deliberately avoids pulling
    /// oxideav-png / oxideav-jpeg / oxideav-webp into the SVG crate's
    /// dep tree by carrying the raster payload as opaque bytes here
    /// for downstream renderer-side decoding.
    pub images: Vec<SvgImage>,
}

/// One captured animation child of a known-id parent element.
#[derive(Clone, Debug)]
pub struct AnimationFragment {
    /// `id` of the parent SVG element, e.g. the `id="rect1"` of the
    /// `<rect>` whose `<animate>` we captured. `None` if the parent
    /// had no id (in which case round 4 drops the fragment).
    pub parent_id: Option<String>,
    /// The animation element itself (one of `<animate>`, `<set>`,
    /// `<animateTransform>`, `<animateMotion>`).
    pub element: Element,
}

/// Round 13 — one (scene-graph tree-path, source-id) pair. Used by the
/// encoder to re-emit `id="..."` attributes on the right scene-graph
/// nodes and to re-attach SMIL animation fragments inside their
/// declared parent.
#[derive(Clone, Debug)]
pub struct IdScenePath {
    /// Source SVG `id="..."` value.
    pub id: String,
    /// Tree-path through the scene graph: each `usize` is the child
    /// index in the parent's `children` vector, from the root group
    /// down to the target node. Empty path means the root.
    pub path: Vec<usize>,
}

impl PreservedExtras {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` when the buffer holds nothing the encoder would emit.
    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
            && self.filters.is_empty()
            && self.animations.is_empty()
            && self.foreign_objects.is_empty()
            && self.scripts.is_empty()
            && self.root_preserve_aspect_ratio.is_none()
            && self.id_paths.is_empty()
            && self.images.is_empty()
            && self.patterns.is_empty()
            && self.gradients.is_empty()
    }
}
