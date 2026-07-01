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
    /// Round 205 — SVG 2 §13.8 `paint-order` source-text bindings,
    /// recorded per emitted shape so the encoder can re-emit
    /// `paint-order="..."` on the matching `<path>` / `<rect>` /
    /// `<circle>` / `<ellipse>` / `<line>` / `<polyline>` /
    /// `<polygon>` element on round-trip.
    ///
    /// The decoder has already applied the §13.8 paint-operation
    /// order to the scene graph (splitting fill+stroke into two
    /// PathNodes when the stroke must paint first); this side-channel
    /// preserves the **author's original keyword string** so a
    /// `parse_svg_with_extras → write_svg_with_extras` cycle emits
    /// the source-equivalent attribute. Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub paint_orders: Vec<PaintOrderBinding>,
    /// Round 209 — SVG 2 §8.13 `vector-effect` source-text bindings,
    /// recorded per emitted shape (or `<use>` group) so the encoder can
    /// re-emit `vector-effect="..."` on the matching graphics element
    /// on round-trip. The property is NOT inherited per §8.13, so a
    /// binding records exactly where the author wrote the attribute —
    /// the decoder never propagates the value to descendants. Populated
    /// only by [`crate::decoder::parse_svg_with_extras`].
    pub vector_effects: Vec<VectorEffectBinding>,
    /// Round 215 — SVG 1.1 §14.3.5 `clip-rule` bindings, recorded per
    /// captured `<clipPath id="...">` so the encoder can re-emit
    /// `clip-rule=` on the inner `<path>` element on round-trip. The
    /// property only applies to graphics elements *inside* a
    /// `<clipPath>` (§14.3.5: "The 'clip-rule' property only applies to
    /// graphics elements that are contained within a 'clipPath'
    /// element"), so the binding keys on the source `<clipPath>` id
    /// instead of a scene-graph tree-path. Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub clip_rules: Vec<ClipRuleBinding>,
    /// Round 221 — SVG 2 §13.10.2 `shape-rendering` source-text
    /// bindings, recorded per emitted shape so the encoder can re-emit
    /// `shape-rendering="..."` on the matching `<path>` / `<rect>` /
    /// `<circle>` / `<ellipse>` / `<line>` / `<polyline>` /
    /// `<polygon>` / `<g>` on round-trip. The property is inherited
    /// per §13.10.2, but the binding is purely lexical — it records
    /// exactly where the author wrote the attribute (the topmost emit
    /// site for the shape, mirroring the round-205 `paint-order` and
    /// round-209 `vector-effect` carriers). Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub shape_renderings: Vec<ShapeRenderingBinding>,
    /// Round 228 — SVG 2 §13.10.3 `text-rendering` source-text
    /// bindings, recorded per emitted `<text>` so the encoder can
    /// re-emit `text-rendering="..."` on the matching element (or its
    /// `<g>` ancestor) on round-trip. Mirrors the round-221
    /// `shape-rendering` carrier — the property is inherited per
    /// §13.10.3, but the binding is purely lexical so the round-trip
    /// preserves exactly where the author wrote the attribute (the
    /// topmost emit site for the run). Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub text_renderings: Vec<TextRenderingBinding>,
    /// Round 247 — SVG 2 §13.10.1 `color-rendering` source-text
    /// bindings, recorded per emitted shape / `<g>` so the encoder can
    /// re-emit `color-rendering="..."` on the matching element on
    /// round-trip. Mirrors the round-221 `shape-rendering` / round-228
    /// `text-rendering` carriers — the property is inherited per
    /// §13.10.1, but the binding is purely lexical so the round-trip
    /// preserves exactly where the author wrote the attribute (the
    /// topmost emit site for the element). Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub color_renderings: Vec<ColorRenderingBinding>,
    /// Round 252 — SVG 2 §13.9 `color-interpolation` source-text
    /// bindings, recorded per emitted shape / `<g>` so the encoder can
    /// re-emit `color-interpolation="..."` on the matching element on
    /// round-trip. Mirrors the round-247 `color-rendering` carrier —
    /// the property is inherited per §13.9, but the binding is purely
    /// lexical so the round-trip preserves exactly where the author
    /// wrote the attribute (the topmost emit site for the element).
    /// Populated only by [`crate::decoder::parse_svg_with_extras`].
    ///
    /// §13.9 selects the working colour space for gradient stop
    /// interpolation, SMIL colour animation, and graphics-element
    /// compositing / blending; it is *distinct* from the §13.10.1
    /// `color-rendering` quality hint that rides on
    /// [`Self::color_renderings`].
    pub color_interpolations: Vec<ColorInterpolationBinding>,
    /// Round 257 — SVG 2 §3.11 `overflow` source-text bindings,
    /// recorded per emitted shape / `<g>` so the encoder can re-emit
    /// `overflow="..."` on the matching element on round-trip.
    /// Mirrors the round-252 `color-interpolation` carrier — the
    /// binding is purely lexical, so even though `overflow` is NOT
    /// inherited per CSS 2.1 the round-trip still captures exactly
    /// where the author wrote the attribute (the topmost emit site
    /// for the element). Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    ///
    /// §3.11 selects whether a UA establishes a clipping rectangle
    /// for an element's content; the actual clipping behaviour +
    /// UA-stylesheet override of the initial value to `hidden` for
    /// non-root `<svg>` / `<symbol>` / `<marker>` / `<pattern>` /
    /// `<image>` live in `oxideav-raster`.
    pub overflows: Vec<OverflowBinding>,
    /// Round 260 — SVG 2 §15.6 `pointer-events` source-text bindings,
    /// recorded per emitted shape / `<g>` so the encoder can re-emit
    /// `pointer-events="..."` on the matching element on round-trip.
    /// Mirrors the round-257 `overflow` carrier — the property IS
    /// inherited per §15.6, but the binding is purely lexical so the
    /// round-trip preserves exactly where the author wrote the
    /// attribute (the topmost emit site for the element). Populated
    /// only by [`crate::decoder::parse_svg_with_extras`].
    ///
    /// §15.6 selects the circumstances under which an element can be
    /// the target of a pointer event (mouse click, hover, hyperlink);
    /// the actual hit-test gating (the visibility / paint / bounding-
    /// box checks per §15.6) lives in the interactive layer (e.g.
    /// `oxideav-pipeline` event routing or `oxideav-raster`
    /// hit-queries).
    pub pointer_eventss: Vec<PointerEventsBinding>,
    /// Round 261 — SVG 1.1 §16.8.2 `cursor` source-text bindings,
    /// recorded per emitted shape / `<g>` so the encoder can re-emit
    /// `cursor="..."` on the matching element on round-trip. Mirrors
    /// the round-260 `pointer-events` carrier — the property IS
    /// inherited per the §16.8.2 attribute table, but the binding is
    /// purely lexical so the round-trip preserves exactly where the
    /// author wrote the attribute (the topmost emit site for the
    /// element). Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    ///
    /// §16.8.2 selects the cursor displayed while the pointing device
    /// hovers the element — zero or more `<funciri>` custom-cursor
    /// references followed by a mandatory generic keyword fallback;
    /// the actual cursor display is interactive-UA work (a windowing
    /// host embedding `oxideav-pipeline`).
    pub cursors: Vec<CursorBinding>,
    /// Round 291 — SVG 1.1 §10.9.2 `dominant-baseline` source-text
    /// bindings, recorded per emitted shape / `<g>` so the encoder can
    /// re-emit `dominant-baseline="..."` on the matching element on
    /// round-trip. Mirrors the round-257 `overflow` carrier — the
    /// property is NOT inherited per §10.9.2, but the binding is purely
    /// lexical so the round-trip preserves exactly where the author
    /// wrote the attribute (the topmost emit site for the element).
    /// Populated only by [`crate::decoder::parse_svg_with_extras`].
    ///
    /// §10.9.2 selects the scaled-baseline-table that positions the
    /// glyphs of a text content element; the actual baseline-table
    /// construction + glyph positioning live in `oxideav-scribe` /
    /// `oxideav-raster`.
    pub dominant_baselines: Vec<DominantBaselineBinding>,
    /// Round 372 — SVG 2 §5.6 `<use>` reference bindings, recorded per
    /// emitted [`oxideav_core::Node::Group`] (the decoder instantiates
    /// each `<use>` as a `Group` wrapping the referenced element's
    /// flattened geometry). `oxideav_core::Group` has no "this group is
    /// a `<use>` instance of #id" field, so the structural reference
    /// identity (`href` + `x`/`y`/`width`/`height` + the `<use>`'s own
    /// `transform`) is stowed here keyed by the group's scene-graph
    /// tree-path (same layout as [`id_paths`](Self::id_paths)). The
    /// encoder, when it reaches a group whose path matches a binding,
    /// emits `<use href="#id" …/>` and **skips re-walking the
    /// instantiated children** — collapsing the flattened geometry back
    /// to the source `<use>` so a `parse_svg_with_extras →
    /// write_svg_with_extras` cycle preserves the reference structure
    /// (and the document stays small instead of inlining the target N
    /// times). Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub uses: Vec<UseBinding>,
    /// Round 372 — verbatim `<defs>`-housed reference targets that
    /// produce no scene-graph node of their own (SVG 1.1 §5.5). A
    /// `<defs>` block holds never-rendered definitions; the typed
    /// side-channels already carry gradients / filters / patterns /
    /// markers / `<style>` verbatim, but a plain id-bearing shape /
    /// `<g>` / `<symbol>` inside `<defs>` (the canonical `<use>` target)
    /// has no other round-trip carrier — the decoder registers it in
    /// `ctx.defs.elements` for `<use>` resolution and emits nothing.
    ///
    /// Each captured element is re-emitted inside the output `<defs>`
    /// block so a `<use href="#id">` the encoder produces (see
    /// [`Self::uses`]) still resolves after round-trip. Captured
    /// verbatim (attribute ordering + descendant structure preserved).
    /// Populated only by [`crate::decoder::parse_svg_with_extras`].
    pub defs_targets: Vec<Element>,
    /// Round 372 — SVG 2 §5.7 `<switch>` verbatim bindings, recorded per
    /// emitted [`oxideav_core::Node::Group`] (the decoder renders the
    /// first child whose conditional-processing attributes test true and
    /// wraps it in a `Group`, discarding the unselected alternatives and
    /// the `<switch>` element identity). `oxideav_core::Group` cannot
    /// represent "first-match container", so the whole `<switch>`
    /// subtree — every alternative + their `requiredExtensions` /
    /// `requiredFeatures` / `systemLanguage` conditional attributes — is
    /// stowed here verbatim keyed by the group's scene-graph tree-path
    /// (same layout as [`id_paths`](Self::id_paths)). On write the
    /// encoder replaces the matching `Group` with the verbatim
    /// `<switch>` and skips re-walking the selected child, so a
    /// `parse_svg_with_extras → write_svg_with_extras` cycle preserves
    /// the conditional structure (and re-parsing under a *different*
    /// `systemLanguage` re-selects correctly rather than being frozen on
    /// the first decode's choice). Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub switches: Vec<SwitchBinding>,
    /// Round 372 — SVG 1.1 §15 `filter="url(#id)"` reference bindings,
    /// recorded per emitted node so the encoder can re-attach the
    /// `filter=` attribute on round-trip. The decoder wraps a filtered
    /// element in a pass-through `Group` (the actual rasterisation is
    /// `oxideav-raster` work) and the `<filter>` def itself rides
    /// [`Self::filters`] verbatim — but the *reference* from the
    /// graphics element to the filter was dropped on write, orphaning
    /// the def. This binding records the original `url(#id)` text keyed
    /// by the wrapper group's scene-graph tree-path (same layout as
    /// [`id_paths`](Self::id_paths)) so a `parse_svg_with_extras →
    /// write_svg_with_extras` cycle re-emits `filter="url(#id)"` on the
    /// matching `<g>`, reconnecting the graphics element to its
    /// preserved filter. Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub filter_refs: Vec<FilterRefBinding>,
    /// Round 372 — verbatim `<clipPath>` def elements (SVG 1.1 §14.3).
    /// The decoder collapses each referenced clip path into a single
    /// merged `oxideav_core::Path` on `Group.clip` (baking per-shape
    /// transforms in, dropping `clipPathUnits`, the original `id`, and
    /// the multi-shape structure). The encoder normally re-synthesises a
    /// `<clipPath id="clip{N}">` from that merged path; when a clip
    /// reference is bound (see [`Self::clip_refs`]) it instead re-emits
    /// the verbatim def captured here so the original id / units /
    /// shapes survive. Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub clip_paths_raw: Vec<Element>,
    /// Round 372 — verbatim `<mask>` def elements (SVG 1.1 §14.4). Same
    /// role as [`Self::clip_paths_raw`] for the soft-mask path: the
    /// decoder turns a `mask="url(#id)"` into a `Node::SoftMask` whose
    /// mask subtree is the flattened mask content (losing the original
    /// `id` / `maskUnits` / `maskContentUnits` / `x`/`y`/`width`/`height`
    /// region). When the mask reference is bound (see [`Self::mask_refs`])
    /// the encoder re-emits this verbatim def instead of the synthesised
    /// `<mask id="mask{N}">`. Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub masks_raw: Vec<Element>,
    /// Round 372 — `clip-path="url(#id)"` reference bindings (SVG 1.1
    /// §14.3.1), keyed by the **merged-clip-path fingerprint** the
    /// encoder's `ClipPathCollector` uses for its own dedup. When the
    /// encoder is about to synthesise a `<clipPath id="clip{N}">` for a
    /// merged path whose fingerprint matches a binding here, it instead
    /// re-emits the verbatim [`Self::clip_paths_raw`] def with the
    /// original `id` and references it as `url(#id)` — so the original
    /// id / `clipPathUnits` / multi-shape structure survive. Routing by
    /// fingerprint (not tree-path) naturally handles the
    /// `filter(mask(clip(node)))` wrapper stack where the clip group's
    /// scene-graph path is aliased. Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub clip_refs: Vec<RefBinding>,
    /// Round 372 — `mask="url(#id)"` reference bindings (SVG 1.1 §14.4),
    /// the soft-mask analogue of [`Self::clip_refs`]. Keyed by the
    /// **mask-subtree fingerprint** the encoder's `MaskCollector` uses
    /// for its dedup (`"{MaskKind:?}:{node_fingerprint}"`). Populated
    /// only by [`crate::decoder::parse_svg_with_extras`].
    pub mask_refs: Vec<RefBinding>,
    /// Round 372 — SVG 2 §13.7.4 `marker-start` / `marker-mid` /
    /// `marker-end` (and the `marker` shorthand) reference bindings,
    /// recorded per emitted shape so the encoder can re-attach the
    /// vertex-marker references on round-trip. `oxideav_core::Node` has
    /// no marker construct (vertex placement is deferred to a core
    /// `Marker` node), so the shape's marker references were dropped on
    /// write even though the `<marker>` def itself rides
    /// [`Self::markers`] verbatim — orphaning the def. This binding
    /// records the verbatim `marker-*` attribute text keyed by the
    /// shape's scene-graph tree-path (same layout as
    /// [`id_paths`](Self::id_paths)) so a `parse_svg_with_extras →
    /// write_svg_with_extras` cycle re-emits `marker-start="url(#id)"`
    /// etc. on the matching `<path>` / `<g>`. Populated only by
    /// [`crate::decoder::parse_svg_with_extras`].
    pub marker_refs: Vec<MarkerRefBinding>,
}

/// Round 372 — one (scene-graph tree-path, `marker-*` references) entry
/// (SVG 2 §13.7.4). Each position-specific reference is recorded
/// verbatim (`url(#id)` / `none`); a `marker` shorthand is expanded by
/// the recorder into the three position-specific slots so the
/// round-trip is independent of the shorthand-vs-longhand spelling.
#[derive(Clone, Debug, Default)]
pub struct MarkerRefBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`]. Identifies the shape `<path>` / wrapper
    /// `<g>` the encoder re-tags with the marker references.
    pub path: Vec<usize>,
    /// `marker-start` verbatim value (e.g. `"url(#arrow)"`). `None` =
    /// attribute absent (initial value `none`).
    pub marker_start: Option<String>,
    /// `marker-mid` verbatim value. `None` = absent.
    pub marker_mid: Option<String>,
    /// `marker-end` verbatim value. `None` = absent.
    pub marker_end: Option<String>,
}

/// Round 372 — one (encoder dedup fingerprint, `url(#id)` reference)
/// pair for the `clip-path` / `mask` carriers. The encoder matches
/// `fingerprint` against the synthesised def it was about to emit and,
/// on a hit, re-emits the verbatim source def with `ref_id` instead.
#[derive(Clone, Debug)]
pub struct RefBinding {
    /// The encoder dedup fingerprint of the merged clip path
    /// (`ClipPathCollector`) or mask subtree (`MaskCollector`) this
    /// reference resolves to. Used to route the verbatim-def
    /// substitution.
    pub fingerprint: String,
    /// The referenced def id (no `#`, no `url(...)` wrapper), e.g.
    /// `"myclip"`. The encoder re-emits `clip-path="url(#myclip)"` /
    /// `mask="url(#myclip)"` from this and looks up the verbatim def by
    /// it.
    pub ref_id: String,
}

/// Round 372 — one (scene-graph tree-path, `filter` url-ref) pair (SVG
/// 1.1 §15). Same shape as [`PaintOrderBinding`]; the binding only
/// round-trips the source `filter=` attribute so the preserved
/// `<filter>` def (carried on [`PreservedExtras::filters`]) stays
/// referenced after a parse → write cycle.
#[derive(Clone, Debug)]
pub struct FilterRefBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`]. Identifies the filter wrapper `<g>` the
    /// encoder re-tags with `filter="url(#id)"`.
    pub path: Vec<usize>,
    /// The verbatim `filter` attribute source text, e.g.
    /// `"url(#f)"`. Preserved as-authored so a `filter="url(#a) url(#b)"`
    /// filter-list (SVG 2 chained references) round-trips unchanged even
    /// though the decoder only wraps a single pass-through group.
    pub filter: String,
}

/// Round 372 — one captured `<switch>` element, keyed by the
/// scene-graph tree-path of the [`oxideav_core::Node::Group`] the
/// decoder produced for the selected branch (SVG 2 §5.7). The element
/// is captured whole (every conditional alternative + the switch's own
/// `transform` / conditional attributes) so the round-trip preserves
/// the first-match container rather than freezing the decode-time
/// selection.
#[derive(Clone, Debug)]
pub struct SwitchBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`]. Identifies the `<g>` the encoder replaces
    /// with the verbatim `<switch>…</switch>`.
    pub path: Vec<usize>,
    /// The verbatim `<switch>` element (all children + attributes).
    pub element: Element,
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
            && self.paint_orders.is_empty()
            && self.vector_effects.is_empty()
            && self.clip_rules.is_empty()
            && self.shape_renderings.is_empty()
            && self.text_renderings.is_empty()
            && self.color_renderings.is_empty()
            && self.color_interpolations.is_empty()
            && self.overflows.is_empty()
            && self.pointer_eventss.is_empty()
            && self.cursors.is_empty()
            && self.dominant_baselines.is_empty()
            && self.uses.is_empty()
            && self.defs_targets.is_empty()
            && self.switches.is_empty()
            && self.filter_refs.is_empty()
            && self.clip_paths_raw.is_empty()
            && self.masks_raw.is_empty()
            && self.clip_refs.is_empty()
            && self.mask_refs.is_empty()
            && self.marker_refs.is_empty()
    }
}

/// Round 372 — one captured `<use>` element, keyed by the scene-graph
/// tree-path of the [`oxideav_core::Node::Group`] the decoder produced
/// for it. SVG 2 §5.6 defines `<use>` as a reference that instantiates
/// the target element (`href` / deprecated `xlink:href`) at an
/// additive `x`/`y` translate, with an optional `transform` and (for
/// `<symbol>` / `<svg>` targets) `width`/`height` overrides. The
/// decoder flattens the instance into the scene graph, so this binding
/// is the only round-trip carrier of the source reference identity.
#[derive(Clone, Debug, Default)]
pub struct UseBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`]. Identifies the `<g>` the encoder replaces
    /// with `<use>…/>` (skipping its instantiated children).
    pub path: Vec<usize>,
    /// `href` target with its leading `#` preserved (SVG 2 `href`,
    /// falling back to the deprecated SVG 1.1 `xlink:href`). Always a
    /// local fragment reference — the decoder only instantiates
    /// in-document targets, so an external `other.svg#id` `<use>` is
    /// never recorded here (it parses to nothing and round-trips via no
    /// binding).
    pub href: String,
    /// Verbatim `x` attribute source text (e.g. `"10"`, `"2em"`). `None`
    /// when the source `<use>` omitted it (initial value `0`).
    pub x: Option<String>,
    /// Verbatim `y` attribute source text. `None` = attribute absent.
    pub y: Option<String>,
    /// Verbatim `width` attribute source text (SVG 2 §5.6 viewport
    /// override for `<symbol>` / `<svg>` targets). `None` = absent.
    pub width: Option<String>,
    /// Verbatim `height` attribute source text. `None` = absent.
    pub height: Option<String>,
    /// Verbatim `transform` attribute source text (e.g.
    /// `"rotate(45)"`). `None` = absent. Preserved verbatim rather than
    /// re-serialised from the composed matrix so author-authored
    /// `rotate` / `scale` / `translate` lists round-trip in their
    /// original spelling.
    pub transform: Option<String>,
    /// Verbatim `id` attribute source text on the `<use>` element
    /// itself (distinct from the referenced target id in
    /// [`Self::href`]). `None` = the `<use>` had no `id`.
    pub id: Option<String>,
    /// Round 382 — every `<use>` source attribute the typed fields above
    /// don't model, captured verbatim in document order so a round-trip
    /// preserves them. Covers the SVG 2 §5.6 `<use>` core / styling /
    /// conditional-processing attributes the collapse-to-`<use>` encoder
    /// path doesn't otherwise re-emit — `class`, `style`, presentation
    /// properties (`opacity`, `clip-path`, `mask`, `filter`,
    /// `visibility`, `fill`, `stroke`, …), `requiredExtensions`,
    /// `systemLanguage`, `data-*`, and so on. The `href` / `xlink:href`
    /// pair and the modelled `x` / `y` / `width` / `height` / `transform`
    /// / `id` attributes are *not* stored here.
    pub extra_attrs: Vec<(String, String)>,
}

/// Round 205 — one (scene-graph tree-path, author `paint-order`
/// keyword string) pair. Same shape as [`PathLengthBinding`] but
/// typed on the source-text payload (the scene graph has already
/// applied the §13.8 paint-operation order during the decode, so
/// the binding only needs to round-trip the source attribute).
#[derive(Clone, Debug)]
pub struct PaintOrderBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`].
    pub path: Vec<usize>,
    /// Author-supplied keyword string (e.g. `"stroke fill markers"`),
    /// canonicalised to lowercase. Trimmed and whitespace-collapsed.
    pub paint_order: String,
}

/// Round 209 — one (scene-graph tree-path, author `vector-effect`
/// keyword string) pair. Same shape as [`PaintOrderBinding`] but
/// scoped to the SVG 2 §8.13 grammar. The scene graph does not yet
/// expose a typed coordinate-suppression hook
/// (`oxideav_core::PathNode` carries no vector-effect field today —
/// rasterisation lives in `oxideav-raster`), so the binding's job is
/// purely to round-trip the source attribute so a `parse → write`
/// cycle preserves the author's request.
/// Round 221 — one (scene-graph tree-path, author `shape-rendering`
/// keyword) pair. Same shape as [`PaintOrderBinding`] /
/// [`VectorEffectBinding`] — the scene graph does not yet expose a
/// typed rendering-hint hook (`oxideav_core::PathNode` carries no
/// rendering-quality field; the hint consumption lives in
/// `oxideav-raster`), so the binding's job is purely to round-trip
/// the source attribute so a `parse → write` cycle preserves the
/// author's hint verbatim.
#[derive(Clone, Debug)]
pub struct ShapeRenderingBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`].
    pub path: Vec<usize>,
    /// Author-supplied keyword string canonicalised to the spec's
    /// camelCase spelling (`auto` / `optimizeSpeed` / `crispEdges` /
    /// `geometricPrecision`). The capture helper canonicalises
    /// case-insensitively, so source `OPTIMIZESPEED` round-trips as
    /// `optimizeSpeed`. The binding skips recording when the resolved
    /// keyword is the initial value (`auto`) AND the source author
    /// didn't write an explicit `shape-rendering=`; the
    /// initial-value-with-explicit-keyword case records so a
    /// hand-authored `shape-rendering="auto"` survives the round-trip.
    pub shape_rendering: String,
}

/// Round 228 — one (scene-graph tree-path, author `text-rendering`
/// keyword) pair. Mirrors [`ShapeRenderingBinding`] — the scene graph
/// does not yet expose a typed text-quality hook (`oxideav-scribe` /
/// `oxideav-raster` own the hint consumption), so the binding's job
/// is purely to round-trip the source attribute so a `parse → write`
/// cycle preserves the author's hint verbatim.
#[derive(Clone, Debug)]
pub struct TextRenderingBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`].
    pub path: Vec<usize>,
    /// Author-supplied keyword string canonicalised to the spec's
    /// camelCase spelling (`auto` / `optimizeSpeed` /
    /// `optimizeLegibility` / `geometricPrecision`). The capture
    /// helper canonicalises case-insensitively, so source
    /// `OPTIMIZELEGIBILITY` round-trips as `optimizeLegibility`. The
    /// binding records explicit `auto` (matching round-221
    /// `shape-rendering` policy) but skips the absent-attribute case
    /// so an initial-value document doesn't bloat with redundant
    /// `text-rendering="auto"` on every `<text>`.
    pub text_rendering: String,
}

/// Round 247 — one (scene-graph tree-path, author `color-rendering`
/// keyword) pair. Mirrors [`TextRenderingBinding`] / [`ShapeRenderingBinding`]
/// — the scene graph does not yet expose a typed
/// colour-interpolation-space hook (`oxideav-raster` owns the working
/// colour-space selection), so the binding's job is purely to
/// round-trip the source attribute so a `parse → write` cycle preserves
/// the author's hint verbatim.
#[derive(Clone, Debug)]
pub struct ColorRenderingBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`].
    pub path: Vec<usize>,
    /// Author-supplied keyword string canonicalised to the spec's
    /// camelCase spelling (`auto` / `optimizeSpeed` / `optimizeQuality`).
    /// The capture helper canonicalises case-insensitively, so source
    /// `OPTIMIZESPEED` round-trips as `optimizeSpeed`. The binding
    /// records explicit `auto` (matching round-221 `shape-rendering` /
    /// round-228 `text-rendering` / round-235 `image-rendering` policy)
    /// but skips the absent-attribute case so an initial-value document
    /// doesn't bloat with redundant `color-rendering="auto"` on every
    /// element.
    pub color_rendering: String,
}

/// Round 252 — one (scene-graph tree-path, author `color-interpolation`
/// keyword) pair. Mirrors [`ColorRenderingBinding`] /
/// [`ShapeRenderingBinding`] — the scene graph does not yet expose a
/// typed colour-interpolation-space hook (`oxideav-raster` owns the
/// working colour-space selection for gradient lerps / colour animation
/// / compositing), so the binding's job is purely to round-trip the
/// source attribute so a `parse → write` cycle preserves the author's
/// hint verbatim.
///
/// SVG 2 §13.9 spells the non-`auto` keywords with mixed case
/// (`sRGB`, `linearRGB`) — distinct from the §13.10.x rendering hints
/// whose keywords are lower-camelCase. The capture helper canonicalises
/// to the §13.9 spelling, so source `SRGB` / `linearrgb` round-trip as
/// the spec spelling.
#[derive(Clone, Debug)]
pub struct ColorInterpolationBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`].
    pub path: Vec<usize>,
    /// Author-supplied keyword string canonicalised to the §13.9
    /// spelling (`auto` / `sRGB` / `linearRGB`). The capture helper
    /// canonicalises case-insensitively, so source `SRGB` round-trips
    /// as `sRGB`. The binding records explicit `sRGB` (the §13.9
    /// initial value) when an author writes it, matching the round-247
    /// `color-rendering` policy of preserving an explicit-initial-value
    /// override — but skips the absent-attribute case so an
    /// initial-value document doesn't bloat with redundant
    /// `color-interpolation="sRGB"` on every element.
    pub color_interpolation: String,
}

/// Round 257 — one (scene-graph tree-path, author `overflow`
/// keyword) pair. Mirrors [`ColorInterpolationBinding`] /
/// [`ShapeRenderingBinding`] — the scene graph does not yet expose
/// a typed clipping-rectangle hook (`oxideav-raster` owns the
/// §3.11 clip-vs-no-clip decision), so the binding's job is purely
/// to round-trip the source attribute so a `parse → write` cycle
/// preserves the author's request verbatim.
///
/// SVG 2 §3.11 reuses the CSS 2.1 keyword set unchanged (`visible`,
/// `hidden`, `scroll`, `auto`), all lowercase — distinct from the
/// §13.9 mixed-case spellings (`sRGB` / `linearRGB`). The capture
/// helper canonicalises case-insensitively, so source `HIDDEN`
/// round-trips as `hidden`.
#[derive(Clone, Debug)]
pub struct OverflowBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`].
    pub path: Vec<usize>,
    /// Author-supplied keyword string canonicalised to the §3.11
    /// spelling (`visible` / `hidden` / `scroll` / `auto`). The
    /// capture helper canonicalises case-insensitively, so source
    /// `HIDDEN` round-trips as `hidden`. The binding records
    /// explicit `visible` (the §3.11 initial value) when an author
    /// writes it — matching the round-221 / round-247 / round-252
    /// "explicit initial value carries intent" policy (e.g. an
    /// override of the UA-stylesheet `overflow: hidden` default that
    /// fires for non-root `<svg>` / `<symbol>` / `<marker>` /
    /// `<pattern>` / `<image>` per §3.11) — but skips the
    /// absent-attribute case so an initial-value document doesn't
    /// bloat with redundant `overflow="visible"` on every element.
    pub overflow: String,
}

/// Round 291 — one (scene-graph tree-path, author `dominant-baseline`
/// keyword) pair. Mirrors [`OverflowBinding`] — the scene graph does
/// not yet expose a typed baseline-table (`oxideav-scribe` /
/// `oxideav-raster` own the §10.9.2 scaled-baseline-table construction
/// and glyph positioning), so the binding's job is purely to
/// round-trip the source attribute so a `parse → write` cycle
/// preserves the author's request verbatim.
///
/// SVG 1.1 §10.9.2 spells every keyword all-lowercase (hyphenated for
/// the multi-word `use-script` / `no-change` / `reset-size` /
/// `text-after-edge` / `text-before-edge`). The capture helper
/// canonicalises case-insensitively, so source `HANGING` /
/// `TEXT-AFTER-EDGE` round-trip as `hanging` / `text-after-edge`.
#[derive(Clone, Debug)]
pub struct DominantBaselineBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`].
    pub path: Vec<usize>,
    /// Author-supplied keyword string canonicalised to the §10.9.2
    /// spelling. The binding records explicit `auto` (the §10.9.2
    /// initial value) when an author writes it — matching the
    /// round-221 / round-247 / round-257 "explicit initial value
    /// carries intent" policy (e.g. an inheritance reset on a child of
    /// a `<text dominant-baseline="hanging">`) — but skips the
    /// absent-attribute case so an initial-value document doesn't bloat
    /// with redundant `dominant-baseline="auto"` on every element.
    pub dominant_baseline: String,
}

/// Round 260 — one (scene-graph tree-path, author `pointer-events`
/// keyword) pair. Mirrors [`OverflowBinding`] / [`ColorInterpolationBinding`]
/// — the scene graph does not yet expose a typed hit-test gate
/// (`oxideav-pipeline` event routing / `oxideav-raster` hit-queries
/// own the §15.6 visibility + paint resolution), so the binding's job
/// is purely to round-trip the source attribute so a `parse → write`
/// cycle preserves the author's request verbatim.
///
/// SVG 2 §15.6 spells the keyword set with three different
/// conventions: lower-camelCase for the four `visible*` keywords
/// (`visiblePainted` / `visibleFill` / `visibleStroke`), hyphenated
/// for `bounding-box`, and all-lowercase for the rest (`visible` /
/// `painted` / `fill` / `stroke` / `all` / `none`). The capture
/// helper canonicalises case-insensitively, so source
/// `VISIBLEPAINTED` / `BOUNDING-BOX` round-trip as the §15.6
/// canonical spelling.
#[derive(Clone, Debug)]
pub struct PointerEventsBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`].
    pub path: Vec<usize>,
    /// Author-supplied keyword string canonicalised to the §15.6
    /// spelling. The capture helper canonicalises case-insensitively,
    /// so source `VISIBLEPAINTED` round-trips as `visiblePainted`.
    /// The binding records explicit `visiblePainted` (the §15.6
    /// initial value) when an author writes it — matching the
    /// round-221 / round-247 / round-252 / round-257 "explicit
    /// initial value carries intent" policy (e.g. an inheritance
    /// reset on a descendant of a `<g pointer-events="none">`) — but
    /// skips the absent-attribute case so an initial-value document
    /// doesn't bloat with redundant `pointer-events="visiblePainted"`
    /// on every element.
    pub pointer_events: String,
}

/// Round 261 — one (scene-graph tree-path, author `cursor` value) pair.
/// Mirrors [`PointerEventsBinding`] — the actual cursor display
/// (SVG 1.1 §16.8.2 funciri resolution + generic-keyword fallback
/// walk) is interactive-UA work, so the binding's job is purely to
/// round-trip the source attribute so a `parse → write` cycle
/// preserves the author's request.
///
/// §16.8.2's value grammar is
/// `[ [<funciri> ,]* [ <generic keyword> ] ] | inherit`: zero or more
/// comma-separated custom-cursor references followed by a mandatory
/// generic keyword fallback. The capture helper canonicalises the
/// value — `url` tokens lowercased with the IRI preserved verbatim,
/// items joined comma-and-space, generic keyword lowercased — so
/// source `URL( #c ) , POINTER` round-trips as `url(#c), pointer`.
#[derive(Clone, Debug)]
pub struct CursorBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`].
    pub path: Vec<usize>,
    /// Author-supplied value canonicalised per the §16.8.2 grammar
    /// (see type-level docs). The binding records explicit `auto`
    /// (the §16.8.2 initial value) when an author writes it —
    /// matching the round-221 .. round-260 "explicit initial value
    /// carries intent" policy (e.g. an inheritance reset on a
    /// descendant of a `<g cursor="wait">`) — but skips the
    /// absent-attribute case so an initial-value document doesn't
    /// bloat with redundant `cursor="auto"` on every element.
    pub cursor: String,
}

#[derive(Clone, Debug)]
pub struct VectorEffectBinding {
    /// Tree-path through the scene graph; matches the layout of
    /// [`IdScenePath::path`].
    pub path: Vec<usize>,
    /// Author-supplied keyword string, canonicalised: lowercased,
    /// trimmed, whitespace-collapsed to single spaces, with effect
    /// keywords in source order (`[ … ]+`-combinator, each at most
    /// once) followed by an optional host suffix (`viewport` /
    /// `screen`). Always carries at least one effect keyword (the
    /// `none` / empty / `inherit` cases skip recording entirely so
    /// the binding is never an initial-value no-op).
    pub vector_effect: String,
}

/// Round 215 — SVG 1.1 §14.3.5 `clip-rule` source-text binding,
/// recorded per captured `<clipPath>` so the encoder can re-emit
/// `clip-rule=` on the round-trip without inheriting the property
/// onto the wrong element. Unlike the round-205 / 209 bindings (which
/// key on a scene-graph tree-path), `clip-rule` only applies inside
/// `<clipPath>` — so the binding keys on the **source `<clipPath>`
/// id** for diagnostic visibility plus the **path-bytes fingerprint**
/// for encoder routing (the encoder generates fresh `clip1`/`clip2`
/// ids per de-duplicated path, so we can't round-trip via the source
/// id alone). The encoder emits the canonical keyword (`nonzero` /
/// `evenodd`) on the inner `<path>` element it generates for the
/// merged-path representation; the binding is omitted when the
/// resolved rule equals the §14.3.5 initial value (`nonzero`) AND the
/// author didn't explicitly write a `clip-rule=` keyword, so a no-op
/// binding doesn't bloat the output.
#[derive(Clone, Debug)]
pub struct ClipRuleBinding {
    /// Source `<clipPath id="...">` id this rule applies to. The
    /// binding never records when the source `<clipPath>` had no `id`,
    /// since an id-less clipPath cannot be referenced and therefore
    /// has no round-trip emit site.
    pub clip_path_id: String,
    /// Path-bytes fingerprint (matches the encoder's
    /// `ClipPathCollector` dedup key). The encoder uses this to route
    /// the rule to the right auto-generated `<clipPath>` def, even
    /// though the source id is rewritten on round-trip.
    pub path_fingerprint: String,
    /// Canonicalised keyword string: lowercased, trimmed. Either
    /// `"nonzero"` or `"evenodd"`. Only recorded when the resolved
    /// rule is non-default (`evenodd`) OR when the source explicitly
    /// declared `clip-rule="nonzero"` (so the round-trip preserves
    /// author intent even when the value matches the initial).
    pub clip_rule: String,
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
