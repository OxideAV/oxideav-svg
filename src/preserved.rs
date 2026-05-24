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

use std::collections::HashMap;

use crate::defs::ViewDef;
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
    /// Round 104 — `<marker>` definitions captured verbatim from the
    /// source SVG (SVG 2 §13.7.1). The decoder also stores a typed view
    /// on [`crate::defs::DefsTables::markers`] for downstream
    /// consumption, but the verbatim element here is the round-trip
    /// source of truth (preserves attribute ordering, descriptive
    /// children, content shapes, and any attributes the typed view
    /// doesn't model). The encoder re-emits each in a `<defs>` block so
    /// a `parse → write` round-trip preserves the marker definition.
    /// `oxideav_core::Node` has no `Marker` construct, so the marker is
    /// never drawn into the rasterised scene graph — only preserved.
    pub markers: Vec<Element>,
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
    /// Round 21 — SVG 2 §9.6.1 `pathLength` attribute, recorded per
    /// emitted shape so the encoder can re-emit
    /// `pathLength="..."` on the matching `<path>` / `<rect>` /
    /// `<circle>` / `<ellipse>` / `<line>` / `<polyline>` /
    /// `<polygon>` element on round-trip.
    ///
    /// The author-supplied path-length is stored in user units; the
    /// decoder has already scaled the corresponding stroke's
    /// `stroke-dasharray` / `stroke-dashoffset` by the
    /// `geometric_length / pathLength` ratio (per §9.6.1), so the
    /// emitted document with this attribute carries the same visual
    /// dash pattern as the source. Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub path_lengths: Vec<PathLengthBinding>,
    /// Round 95 — `<view>` element trees captured verbatim from the
    /// source SVG. The verbatim element is the round-trip source of
    /// truth so attribute ordering, descriptive children (`<title>` /
    /// `<desc>` / `<metadata>`), and any attributes the typed view
    /// doesn't yet model survive a `parse_svg_with_extras →
    /// write_svg_with_extras` cycle. The typed view (keyed by `id`)
    /// rides on [`typed_views`](Self::typed_views) for fragment-
    /// identifier resolution via [`crate::resolve_fragment`].
    pub views: Vec<Element>,
    /// Round 95 — typed [`ViewDef`]s for every captured `<view>` keyed
    /// by `id`. Consumed by [`crate::resolve_fragment`] (SVG 2 §16.3.2
    /// bare-name routing). Empty when the source SVG had no
    /// id-bearing `<view>` elements.
    pub typed_views: HashMap<String, ViewDef>,
    /// Round 115 — SVG 2 §16.5 `<a>` hyperlink bindings, recorded per
    /// emitted [`oxideav_core::Node::Group`] so the encoder can wrap the
    /// `<g>` back in its `<a href="...">…</a>` element on round-trip.
    ///
    /// `<a>` is a *container + renderable* element: it renders its
    /// children exactly like `<g>` (transform / opacity / paint
    /// cascade), so the decoder produces a `Node::Group` for it. But
    /// `oxideav_core::Group` has no hyperlink field, so the link target
    /// and its companion HTML attributes (`target` / `download` /
    /// `ping` / `rel` / `hreflang` / `type` / `referrerpolicy`) are
    /// stowed here keyed by the group's scene-graph tree-path (same
    /// layout as [`id_paths`](Self::id_paths)). Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub links: Vec<LinkBinding>,
    /// Round 122 — SVG 2 §5.8 `<title>` descriptive-element bindings,
    /// keyed by the scene-graph tree-path of the *parent* container
    /// (root `<svg>` → empty path; nested `<g>` / `<a>` / `<switch>` /
    /// `<symbol>` / `<defs>` → its own scene-graph slot). Each binding
    /// carries every sibling `<title>` (in document order, with the
    /// optional `lang` / `xml:lang` attribute captured) so the
    /// multilingual selection algorithm in §5.8 can pick the best match
    /// at render / serialise time. `<title>` is never-rendered (the UA
    /// stylesheet forces `display:none`), so it produces no scene-graph
    /// node — the binding is the round-trip source of truth.
    /// Populated only by [`crate::decoder::parse_svg_with_extras`].
    pub titles: Vec<DescriptiveBinding>,
    /// Round 122 — SVG 2 §5.8 `<desc>` descriptive-element bindings.
    /// Same layout as [`titles`](Self::titles) — keyed by the parent
    /// container's scene-graph tree-path, carries every sibling `<desc>`
    /// in document order with its optional `lang` / `xml:lang`. `<desc>`
    /// is never-rendered (same UA `display:none` rule) so the binding
    /// is the only round-trip carrier. Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub descs: Vec<DescriptiveBinding>,
    /// Round 122 — SVG 2 §5.9 `<metadata>` element trees captured
    /// verbatim from the source SVG. The `<metadata>` content model is
    /// "any elements or character data" (typically RDF / Dublin Core /
    /// arbitrary XML from other namespaces), so a structured parse is
    /// out of scope — we round-trip the whole element. Per §5.9 the
    /// UA stylesheet forces `display:none`, so the element never enters
    /// the rendering tree. The encoder re-emits each at the trailing
    /// edge of the document so a `parse → write` round-trip preserves
    /// embedded metadata blocks (Dublin Core, RDF, Inkscape /
    /// Sodipodi extensions in foreign namespaces, etc.).
    pub metadata: Vec<Element>,
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

/// Round 21 — one (scene-graph tree-path, author `pathLength`) pair.
/// Same shape as [`IdScenePath`] but typed on the f32 path length so
/// the encoder doesn't need a string parse to re-emit the attribute.
#[derive(Clone, Debug)]
pub struct PathLengthBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`].
    pub path: Vec<usize>,
    /// Author-supplied `pathLength` in user units (per SVG 2 §9.6.1).
    pub path_length: f32,
}

/// Round 115 — one captured `<a>` hyperlink, keyed by the scene-graph
/// tree-path of the [`oxideav_core::Node::Group`] the decoder produced
/// for it. SVG 2 §16.5 defines the `<a>` element + the HTML-aligned
/// link attributes; only the `href` is structurally required, the rest
/// are optional descriptors the encoder re-emits verbatim on the
/// re-wrapping `<a>` element.
#[derive(Clone, Debug, Default)]
pub struct LinkBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`]. Identifies the `<g>` the encoder wraps in
    /// `<a>…</a>`.
    pub path: Vec<usize>,
    /// `href` (SVG 2 `href`, falling back to the deprecated SVG 1.1
    /// `xlink:href`). `None` when the source `<a>` carried neither — a
    /// bare `<a>` with no target still groups its children but the
    /// encoder re-emits `<a>` without an `href` attribute.
    pub href: Option<String>,
    /// `target` browsing-context name (`_self` / `_blank` / …; SVG 2
    /// §16.5). `None` = attribute absent (initial value `_self`).
    pub target: Option<String>,
    /// `download` — suggested file name (HTML link attribute).
    pub download: Option<String>,
    /// `ping` — space-separated URL tokens (HTML link attribute).
    pub ping: Option<String>,
    /// `rel` — space-separated relationship keywords (HTML link
    /// attribute).
    pub rel: Option<String>,
    /// `hreflang` — BCP 47 language tag of the target (HTML link
    /// attribute).
    pub hreflang: Option<String>,
    /// `type` — MIME type of the target (HTML link attribute).
    pub type_: Option<String>,
    /// `referrerpolicy` — referrer-policy string (HTML link attribute).
    pub referrerpolicy: Option<String>,
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
            && self.markers.is_empty()
            && self.gradients.is_empty()
            && self.path_lengths.is_empty()
            && self.views.is_empty()
            && self.typed_views.is_empty()
            && self.links.is_empty()
            && self.titles.is_empty()
            && self.descs.is_empty()
            && self.metadata.is_empty()
    }
}

/// Round 122 — one captured `<title>` or `<desc>` element body. Per SVG
/// 2 §5.8 the descriptive element's content model is "any elements or
/// character data", but the spec only mandates the *plain text* content
/// be exposed to assistive technologies; we capture the concatenated
/// text-runs of the immediate children for the structural round-trip.
/// Foreign-namespace markup inside descriptive elements is dropped to
/// the side-channel `metadata` queue if it is `<metadata>`; otherwise
/// it is reduced to plain text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DescriptiveText {
    /// Concatenated text content of the descriptive element. Per §5.8
    /// authoring guidance "SVG generators must not include empty title
    /// or desc elements with no text content"; we still record an empty
    /// string so a hand-authored empty element round-trips unchanged.
    pub text: String,
    /// `lang` (HTML-style) or `xml:lang` (legacy SVG / XML 1.0) tag
    /// captured verbatim. Per §5.8 multilingual selection: an empty
    /// language tag is "a lowest-priority match for any user, ranked
    /// below all user-specified language preferences"; an absent
    /// language tag inherits from the nearest ancestor (we record
    /// `None` here and leave inheritance to the renderer / language-
    /// match consumer).
    pub lang: Option<String>,
}

/// Round 122 — one (parent scene-graph tree-path, ordered list of
/// `<title>` or `<desc>` elements) pair. Per SVG 2 §5.8 a container
/// element may carry zero or more title / desc children; the multiple
/// case is for language-tagged alternatives. The encoder re-emits the
/// list in source order as the first children of the matching `<g>`
/// (or, for an empty path, before all top-level scene children of the
/// root `<svg>`).
#[derive(Clone, Debug)]
pub struct DescriptiveBinding {
    /// Tree-path through the scene graph of the *parent* container of
    /// the descriptive elements. Same layout as [`IdScenePath::path`].
    /// Empty path = root `<svg>` (direct child of the document root).
    pub parent_path: Vec<usize>,
    /// Descriptive elements in document order. Per §5.8 the UA selects
    /// the best language match; we preserve every entry so the consumer
    /// can run the §5.8 selection algorithm itself.
    pub items: Vec<DescriptiveText>,
}
