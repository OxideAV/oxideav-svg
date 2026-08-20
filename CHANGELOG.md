# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- round 449 — native shape identity round-trip (SVG 2 §9.2–§9.7). The
  decoder flattens every basic shape into path commands, so the encoder
  emitted `<path d="…">` for a source `<rect>` / `<circle>` /
  `<ellipse>` / `<line>` / `<polyline>` / `<polygon>` — geometrically
  identical but losing the element identity, which broke every consumer
  addressing the shape *as* a shape (an inlined
  `<animate attributeName="x">` re-attached to a `<path>` targets an
  attribute the element doesn't have; `rect { … }` type selectors stop
  matching after a round-trip). New
  `PreservedExtras::shapes: Vec<ShapeBinding>` records the source tag +
  verbatim geometry attributes keyed by the inner geometry node's
  scene-graph tree-path (the §9.6.1 `pathLength` slot); the encoder's
  geometry arm emits the native tag with those attributes instead of
  the flattened `d`, with every existing carrier (`pathLength`,
  `marker-*`, §13.x hints, inline animations) riding the same element.
  Percentage / unit-bearing geometry survives verbatim and re-resolves
  identically because the viewport attributes and any `<style>` sheet
  round-trip alongside. The carrier declines ambiguous emit sites: the
  §13.8 stroke-first `paint-order` split (two single-purpose paths for
  one source shape) keeps the flattened form, and `<use>`-instantiated
  geometry is skipped (the instance collapses back to `<use>`). A
  masked shape keeps its identity inside the `<g mask="url(#…)">`
  wrapper (the strict sub-path resolver descends `SoftMask` content,
  which shares the wrapper's tree-path). The extras-free `write_svg`
  path is unchanged. New `tests/round449_shape_identity.rs` (10 tests);
  the round-13 / round-122 locator assertions that pinned the old
  `<path>` spelling are updated to the native tags.

- round 449 — SMIL animation parent re-attachment by scene-graph path
  (SMIL Animation §3.1: an animation element with no explicit target
  attribute targets its direct XML parent). The round-13 routing keyed
  captured animations by the *nearest id-bearing ancestor*, which
  (a) orphaned every animation whose parent chain is id-less — dumped
  detached at the document's trailing edge, where the implicit parent
  target becomes the root `<svg>` and the animation is semantically
  lost — and (b) mis-parented an animation whose id-less parent sits
  inside an id-bearing container. New
  `PreservedExtras::anim_targets: Vec<AnimTargetBinding>` records each
  element's direct animation children (verbatim, source order) keyed by
  the element's scene-graph tree-path during the scene build; the
  encoder re-emits them as children of the matching `<g>` / `<path>` /
  `<use>`. A suppression multiset (structural `PartialEq` on the XML
  `Element` tree, newly derived) cancels the XML-walk double-capture:
  fragments equal to a path-routed animation OR to an animation riding
  a verbatim side-channel tree (`<text>` / `<switch>` / `<defs>`
  target / `<filter>` / `<pattern>` / `<marker>` / gradient / `<view>`
  / `<foreignObject>` / `<metadata>` / captured `<image>` children) are
  dropped from the id / orphan channels, fixing the latent
  double-emission for animations housed in verbatim-preserved subtrees.
  Recording is skipped inside `<use>` target instantiation (the
  collapsed instance never emits and its boundary aliases the `<use>`'s
  own slot). The `Node::SoftMask` arm no longer emits id-keyed inline
  animations — the wrapped content shares the wrapper's tree-path and
  its own arm performs the emission, so a masked id-bearing shape with
  a captured animation emitted it twice. New
  `tests/round449_anim_attach.rs` (12 tests): id-less shape / `<set>` /
  deep nesting / group / `<use>` re-attachment, direct-parent (not
  ancestor) targeting, id-bearing single emission, pattern and
  defs-target single emission, `<animateMotion>`+`<mpath>` survival,
  orphan fallback for uncarried parents, and write fixed-point
  idempotence.

- round 449 — `<text>` verbatim round-trip fidelity (SVG 2 §11.2). The
  decoder flattens `<text>` into resolver-shaped glyph outline paths
  (or an empty group when no font resolver is installed), so a
  `parse → write` cycle dropped the source character data, the font
  selection properties, the §11.2.2 `<tspan>` per-character positioning
  arrays (`x` / `y` / `dx` / `dy` / `rotate`), and any §11.8
  `<textPath>` layout. New `PreservedExtras::texts: Vec<TextBinding>`
  captures each `<text>` element verbatim keyed by the scene-graph
  tree-path of the node the decoder produced (same carrier design as
  the round-372 `<switch>` binding); on write the encoder replaces the
  flattened node with the source markup and skips the shaped glyph
  children. Character data is serialised through a new
  mixed-content-preserving inline writer (`write_mixed_content_element`)
  — the pretty-printing verbatim serialiser trims text runs and inserts
  indentation, which corrupts the §11.1 mixed content model under
  `xml:space="default"` collapsing (lost inter-span spaces, synthetic
  spaces between adjacent spans) — so the text subtree round-trips
  byte-exactly with entities re-escaped. An `<animate>` child of an
  id-bearing `<text>` rides the verbatim element exactly once. New
  `tests/round449_text_roundtrip.rs` (10 tests): plain text, tspan
  arrays + exact spacing, textPath + target def resolution, styling
  attributes, animation single-emission, write fixed-point idempotence,
  document order, nested placement, special-character re-escape, and a
  no-text no-op guard.

- round 449 — encoder emit-lookup refactor: the twenty per-path
  side-channel lookup tables threaded through `write_group_children` /
  `write_node` as individual parameters now ride a single `EmitIndex`
  struct (pure refactor, no behavioural change), keeping the recursion
  signature stable as write-side round-trip channels accrue.

- round 375 — SVG 2 §5.5 `<symbol>` `x` / `y` geometry properties.
  "The x, y, width, and height geometry properties have the same effect
  as on an `svg` element, when the `symbol` is instantiated by a `use`
  element" (new in SVG 2). The symbol's `x` / `y` now position its
  viewport inside the `<use>`'s coordinate system (the use's own
  `x` / `y` translate is layered on top); they were previously ignored.
  New `SymbolDef::{intrinsic_x, intrinsic_y}` fields; new
  `tests/round375_symbol_xy.rs` (3 tests).

- round 375 — SVG 2 §5.5 `<symbol>` `refX` / `refY` reference point.
  SVG 2 added these attributes to `<symbol>` ("Added to make it easier
  to align symbols to a particular point, as is often done in maps;
  Similar to the matching attributes on `marker`"). The reference point
  — given in the symbol's own coordinate system, accepting a `<length>`
  or the geometric keywords (`left` / `center` / `right` for `refX`;
  `top` / `center` / `bottom` for `refY`, resolved against the `viewBox`
  extent via the existing `<marker>` helper) — is now aligned with the
  instantiating `<use>`'s `x` / `y`: the point is mapped through the
  §8.2 viewport transform and the result subtracted so it lands at the
  viewport origin (which the use placement then positions). Before this
  round the attributes were ignored and a `<symbol refX refY>` was
  positioned by its top-left corner. New `SymbolDef::{ref_x, ref_y}`
  fields; new `tests/round375_symbol_ref.rs` (4 tests).

### Fixed

- round 375 — `preserveAspectRatio` now consumes the optional leading
  `defer` keyword (SVG 2 §8.7: `[defer] <align> [<meetOrSlice>]`).
  Previously `defer` was parsed *as* the `<align>` token (falling back
  to the `xMidYMid` default) and shifted the real align into the
  `<meetOrSlice>` slot — so e.g. `"defer xMinYMin slice"` lost both the
  `xMinYMin` alignment and the `slice` mode. `defer` is only meaningful
  on `<image>` (ignored elsewhere) but must be consumed everywhere so
  the remaining tokens read from their correct positions. New unit test
  + a nested-`<svg>` integration assertion.
- round 375 — the `<symbol>` / `<use>` §8.2 viewport transform
  (`symbol_viewport_transform`) double-counted the viewBox min
  translation: its alignment translate seeded `tx = -min_x · scale`
  *and* the chain kept a trailing `translate(-min_x, -min_y)`, so a
  `<symbol viewBox="min-x min-y …">` with a non-zero `min-x` / `min-y`
  shifted the instantiated content by an extra `min · scale`. Documents
  with the usual `min=0` viewBox were unaffected (which is why every
  prior test passed). The alignment translate now seeds at zero, leaving
  the trailing `translate(-min)` as the sole min-mapping term, so the
  viewBox-min corner maps exactly to the viewport origin per §8.6. New
  `tests/round375_symbol_viewbox_min.rs` (2 tests).

### Added

- round 375 — *nested* `<svg>` viewport establishment (SVG 1.1 §7.10 /
  SVG 2 §8.2). An inner `<svg>` previously fell through the
  element-dispatch deferral (`_ => None`) and was dropped together with
  its entire subtree. The new `"svg"` arm models it as a `Node::Group`
  whose transform is `translate(x, y) ∘ viewport_transform(viewBox →
  width × height)`: `x` / `y` (default 0) place the new viewport in the
  current user space, `width` / `height` (default `100%` of the parent
  viewport, resolved through the existing length machinery) size it, and
  an optional `viewBox` + `preserveAspectRatio` re-map the inner
  coordinate system via a new `nested_svg_viewport_transform` (the
  canonical §8.2 single-chain form `translate(align) ∘ scale ∘
  translate(-min)`, so the viewBox-min corner maps exactly to the
  viewport origin). Descendant percentage lengths now resolve against
  the nested viewport (the resolve context's `viewport_w` /
  `viewport_h` are swapped to the inner viewBox extent for the child
  walk and restored after). A zero or negative `width` / `height`
  disables rendering of the element and its children per §8.2 step 1.
  New `tests/round375_nested_svg.rs` (6 tests).
- round 372 — `marker-start` / `marker-mid` / `marker-end` (and the
  `marker` shorthand) reference round-trip fidelity (SVG 2 §13.7.4).
  `oxideav_core::Node` has no marker construct (vertex placement is
  deferred to a core `Marker` node), so a shape's marker references were
  dropped on write even though the `<marker>` def itself rides
  `PreservedExtras::markers` verbatim — orphaning the def. New
  `PreservedExtras::marker_refs: Vec<MarkerRefBinding>` records the
  verbatim `marker-*` attribute text keyed by the shape's scene-graph
  tree-path (a `marker` shorthand is expanded into the three
  position-specific slots so the longhand round-trips regardless of
  source spelling; `none` / absent records nothing). On write the
  encoder re-emits `marker-start=` / `marker-mid=` / `marker-end=` on the
  matching `<path>` / `<g>`, reconnecting the shape to its preserved
  marker def. New `tests/round372_marker_ref_roundtrip.rs` (5 tests):
  single position, all-three-positions, shorthand expansion,
  reconnect-after-reparse, and a no-marker no-op guard.
- round 372 — `<clipPath>` / `<mask>` reference-identity round-trip
  fidelity (SVG 1.1 §14.3 / §14.4). The decoder collapses a
  `clip-path="url(#id)"` into a single merged `Path` on `Group.clip`
  (baking per-shape transforms in, dropping `clipPathUnits`, the
  original id, and the multi-shape structure) and a `mask="url(#id)"`
  into a `Node::SoftMask` with flattened content (dropping the original
  id / `maskUnits` / region). The encoder used to re-synthesise a
  single-shape `<clipPath id="clip1">` / `<mask id="mask1">` and
  reference the synthesised id. New side-channels:
  `clip_paths_raw` / `masks_raw` (verbatim `<clipPath>` / `<mask>` defs)
  plus `clip_refs` / `mask_refs: Vec<RefBinding>` keyed by the encoder's
  own dedup fingerprint (`path_fingerprint` for clips,
  `mask_fingerprint` for masks — the latter newly exposed). On write,
  when a synthesised clip / mask's fingerprint is bound *and* the
  verbatim source def was captured, the encoder re-emits the source def
  (original id + units + every shape) via a per-collector `id_override`,
  and `ClipPathCollector`/`MaskCollector::lookup` substitutes the
  original id into the `clip-path=` / `mask=` reference — falling back to
  the old synthesis when no verbatim def is available. New
  `tests/round372_clip_mask_roundtrip.rs` (6 tests): id/units survival,
  multi-shape survival, mask id/units, clip+mask stacking, idempotent
  re-parse, and a no-op guard.
- round 372 — `filter="url(#id)"` reference round-trip fidelity (SVG 1.1
  §15). The decoder wraps a filtered element in a pass-through `Group`
  and preserves the `<filter>` def verbatim in `PreservedExtras::filters`,
  but the *reference* from the graphics element to the filter was dropped
  on write, orphaning the def. New
  `PreservedExtras::filter_refs: Vec<FilterRefBinding>` records the source
  `filter=` attribute text keyed by the wrapper group's scene-graph
  tree-path (only when the referenced `<filter>` def actually exists);
  on write the encoder re-emits `filter="url(#id)"` on the matching `<g>`,
  reconnecting the graphics element to its preserved filter (a chained
  `filter="url(#a) url(#b)"` list round-trips verbatim). New
  `tests/round372_filter_ref_roundtrip.rs` (4 tests): re-attachment,
  reconnect-after-reparse, unresolved-ref records nothing, and a
  no-filter no-op guard.
- round 372 — `<switch>` verbatim round-trip fidelity (SVG 2 §5.7). The
  decoder renders the first child whose conditional-processing
  attributes test true and wraps it in a `Group`, discarding the
  unselected alternatives + the `<switch>` identity; before this round
  `parse → write` re-emitted a plain `<g>` with only the selected child,
  freezing the decode-time selection and losing the alternatives. New
  `PreservedExtras::switches: Vec<SwitchBinding>` captures the whole
  `<switch>` verbatim keyed by the selected branch's scene-graph
  tree-path; on write the encoder replaces the matching `Group` with the
  verbatim `<switch>` and skips re-walking the selected child, so a
  re-parse under a *different* `systemLanguage` re-selects correctly.
  New `tests/round372_switch_roundtrip.rs` (4 tests): all-alternatives
  preservation, faithful re-selection on re-parse, switch-transform, and
  a no-`<switch>` no-op guard.
- round 372 — `<use>` reference + `<defs>` target round-trip fidelity
  (SVG 2 §5.6 / SVG 1.1 §5.5). The decoder flattens every `<use>` into
  the instantiated geometry, so before this round a `parse → write`
  re-emitted the inlined shapes and lost the reference identity entirely
  (inlining the target N times for an N-instance document), and the
  `<defs>`-housed target shape was dropped on write. Two new
  `PreservedExtras` side-channels close the gap:
  * `uses: Vec<UseBinding>` — recorded per instantiated instance group
    keyed by scene-graph tree-path, carrying the `href` (with `#`),
    `x`/`y`/`width`/`height`, the `<use>`'s own `transform`, and its own
    `id` (verbatim source text). On write the encoder collapses the
    matching `Node::Group` back to a single `<use href="#id" …/>` and
    skips re-walking the flattened children. The use's own `id` is
    emitted — never the round-13 `id_paths` value (which carries the
    *target's* id and would self-reference).
  * `defs_targets: Vec<Element>` — verbatim id-bearing reference targets
    housed in `<defs>` (plain shapes / `<g>`) plus any id-bearing
    `<symbol>`, which produce no scene-graph node and had no other
    round-trip carrier. Re-emitted at the head of the output `<defs>`
    block so a round-tripped `<use href="#id">` still resolves. Gradients
    / filters / patterns / markers / `<style>` are skipped (they ride
    their own side-channels). New `tests/round372_use_roundtrip.rs`
    (6 tests) covers defs-rect, symbol, multi-instance, transform, and
    own-id cases, plus a `parse → write → re-parse` instance-count
    invariant and a no-`<use>` no-op guard.
- round 367 — §9.4 filter-primitive subregion *resolution*: the missing
  half of round 361's clip arithmetic. `resolve_subregions(graph, ctx)`
  turns each primitive's `x` / `y` / `width` / `height` into the
  `Vec<Option<PixelRect>>` that `evaluate_filter_graph_clipped` already
  consumes, so the SVG layer now owns the full §9.4 subregion pipeline
  (resolution → clip) rather than leaving resolution to the rasteriser.
  The turn-key `evaluate_filter_graph_resolved(graph, sources, ctx)`
  composes resolution + clipped evaluation, deriving the working-raster
  size from the filter region on `ctx` so the caller supplies the filter
  geometry once.
  New `FilterSubregionCtx` carries the §8 filter region (as a pixel
  rectangle) plus the user-space and object-bounding-box mappings; the
  resolver honours: the §7 `<length-percentage>` distinction (percentages
  always resolve against the filter region, numbers consult
  `primitiveUnits` — user-space length vs object-bounding-box fraction),
  the §9.4 default subregion (union/tightest-fitting bounding box of the
  referenced nodes' resolved subregions; whole filter region when a
  primitive references any standard input or is `feFlood` /
  `feTurbulence`), the §9.4 `feImage` / `feTurbulence` / `feTile`
  whole-region forcing, and the §9.4 negative/zero-extent disable rule.
  Float subregions snap to the enclosing integer `PixelRect` (left/top
  floor, right/bottom ceil) so partly-covered pixels survive, per the
  §9.4 "even partly intersect" wording. To carry the percentage flag a
  new `FilterCoord` (`Number` / `Percentage`) + `RegionCoords` are parsed
  alongside the existing lossy numeric `PrimitiveRegion` on both
  `FilterGraph` and `FilterPrimitiveNode` (the numeric view is unchanged
  for round-trip / back-compat). 10 new unit tests (explicit percentage,
  user-space number, object-bounding-box fraction, flood/standard-input/
  feTile full-region defaults, referenced-node union default, zero-extent
  disable, integer snapping) plus 8 end-to-end integration tests in
  `tests/round367_subregion_resolve.rs` driving the resolver from parsed
  `<filter>` documents — including the §9.4 worked example
  (`<feFlood x="25%" y="25%" width="50%" height="50%">` → a
  half-the-region-centred rectangle) — exercising the `RegionCoords`
  percentage capture through the real parser.
- round 361 — filter-primitive subregion clipping per Filter Effects §9.4.
  `clip_to_subregion` applies a `PixelRect` as a hard clip on a primitive's
  result (pixels outside become transparent black; a zero-extent rectangle
  disables the primitive), and `evaluate_filter_graph_clipped` threads a
  per-primitive `&[Option<PixelRect>]` through the DAG evaluator, clipping
  each node's output *before* it is stored as a named `result` or reused as
  the next primitive's default input. `evaluate_filter_graph` is now a
  no-clip wrapper. Resolving a subregion's user / `objectBoundingBox`
  attributes to pixels stays a rasteriser concern; the clip arithmetic is
  in-crate. 5 new tests (keep-inside/zero-outside, zero-extent disables,
  partial raster overlap, clipped flood, clip-propagates-to-downstream).
- round 361 — top-level filter-graph DAG evaluator `evaluate_filter_graph`
  in [`crate::filter_eval`], chaining the per-primitive `evaluate_*_node`
  functions into a complete pipeline. Maintains the named-`result` map and
  the implicit "previous result" fallback the Filter Effects `in` attribute
  defines (first primitive → `SourceGraphic`, subsequent → prior result),
  resolves every `in` / `in2` — including unknown `result` references
  (falling back to the previous result) and the `SourceAlpha` /
  `BackgroundAlpha` alpha-only derivations — to an 8-bit RGBA buffer, and
  dispatches each node in source order, returning the final layer. Adds a
  `FilterSources` operand set carrying the `SourceGraphic` plus the optional
  `BackgroundImage` / `FillPaint` / `StrokePaint` standard inputs (each
  defaulting to transparent black per §15.7.2 when unsupplied). The whole
  graph runs at full filter-region resolution; per-primitive sub-region
  clipping and `feImage` external-reference resolution remain rasteriser
  work. 9 new tests (empty graph, identity offset, previous-result default
  chaining, named-result reference, unknown-reference fallback, SourceAlpha
  derivation, two-layer feMerge, unresolved feImage → None, missing standard
  input → transparent).
- round 354 — pixel-level `<feSpecularLighting>` node evaluator in
  [`crate::filter_eval`] (`specular_lighting` +
  `evaluate_specular_lighting_node`), reusing the §18 `surface_normal` /
  `light_geometry` kernel. Implements the Filter Effects Module Level 1 §19
  Blinn-Phong specular model: the viewer is at infinity along `+Z` so the eye
  vector is the constant `E = (0, 0, 1)` and the half-vector is
  `H = (L + E) / Norm(L + E)`; per pixel `S = ks · pow(max(N·H, 0),
  specularExponent) · Lcolor` with the §19 non-opaque alpha
  `Sa = max(Sr, Sg, Sb)` (so the highlight can be *added* to a texture — a zero
  highlight adds no coverage, a white highlight adds full opacity). The
  spot-light cone fall-off folds into the per-pixel colour scale, and
  `lighting-color` decodes into the working space (§10). 6 new tests
  (flat-overhead-white, alpha-is-max-of-colour, zero-highlight-transparent,
  exponent-sharpens, node round-trip, declines-other-primitive).

- round 354 — pixel-level `<feDiffuseLighting>` node evaluator in
  [`crate::filter_eval`] (`diffuse_lighting` + `evaluate_diffuse_lighting_node`,
  with the shared `surface_normal` / `light_geometry` kernel). Implements the
  Filter Effects Module Level 1 §18 Lambertian-diffuse model: the alpha channel
  is treated as a height map `Z = surfaceScale · I`, the surface normal `N`
  comes from the position-dependent §18 Sobel gradient (all nine kernels — four
  corners, four edges, the interior — each with its listed `FACTORx`/`FACTORy`
  scale and edge-clamped sampling), and per pixel `D = kd · max(N·L, 0) ·
  Lcolor` with `Da = 1.0` (opaque). The light unit-vector `L` is constant for a
  `<feDistantLight>` (`cos·cos`, `sin·cos`, `sin` of azimuth/elevation) and
  position-dependent for `<fePointLight>` / `<feSpotLight>`; the spot-light
  `pow(-L·S, specularExponent)` cone fall-off (with `limitingConeAngle`
  clipping) folds into a per-pixel colour scale. `lighting-color` is decoded
  into the node's `color-interpolation-filters` working space (§10) exactly as
  `<feFlood>` decodes `flood-color`. 8 new tests (flat-overhead-is-white,
  always-opaque, kd scaling, colour tint, slope reduction, spot centre, node
  round-trip, declines-other-primitive).

- round 343 — pixel-level `<feTurbulence>` Perlin-noise / fractal-sum
  generator in [`crate::filter_eval`] (`turbulence_image` +
  `evaluate_turbulence_node`, with the private `Turbulence` lattice
  engine). A clean-room Rust port of the Filter Effects Module Level 1
  §9.21 normative reference algorithm: the Park–Miller minimal-standard
  LCG (`a = 16807`, `m = 2³¹ − 1`, Schrage's method), the rejection-
  sampled unit-disc gradient lattice with the Fisher–Yates permutation
  shuffle and high-half duplication, the `s_curve` / `lerp` bilinear
  `noise2` sample, and the per-octave `turbulence` sum with optional
  `stitchTiles` frequency-snap + lattice wrapping. Per §9.21 the four
  channels R/G/B/A are generated in that fixed order (deterministic
  per-channel random streams), mapped to colour values by
  `(result + 1)/2` for `fractalNoise` or `result` for `turbulence`
  (clamped to `[0, 1]`), then premultiplied into the
  [`crate::filter_eval::FilterImage`] working space. 8 new tests,
  including the §9.21 normative anchor that the LCG's 10,000th number
  from seed 1 is `1043618065`.

- round 343 — pixel-level `<feTile>` node evaluator in
  [`crate::filter_eval`] (`tile` + `evaluate_tile_node`). Implements the
  Filter Effects Module Level 1 §9.20 reference-tile replication: the
  destination pixel `(dx, dy)` samples the input's reference tile
  `(tile_x, tile_y, tile_w, tile_h)` at the periodic coordinate
  `tile_x + ((dx − tile_x) mod tile_w)`,
  `tile_y + ((dy − tile_y) mod tile_h)` (Euclidean modulo, so the
  §9.20 `(x + i·width, y + j·height)` tiling wraps for destinations
  left of / above the tile origin). The caller (graph-level rasteriser)
  supplies the input's filter-primitive subregion as the tile rectangle;
  the evaluator does no subregion plumbing. A zero-area tile yields
  transparent black. Tiling copies premultiplied storage verbatim, so it
  is colour-space-neutral. 6 new tests.

- round 343 — pixel-level `<feDisplacementMap>` node evaluator in
  [`crate::filter_eval`] (`displacement_map` +
  `evaluate_displacement_map_node`). Implements the Filter Effects
  Module Level 1 §9.11 spatial displacement
  `P'(x, y) ← P(x + scale·(XC − ½), y + scale·(YC − ½))` over the
  [`crate::filter_eval::FilterImage`] storage, where `XC` / `YC` are the
  `xChannelSelector` / `yChannelSelector` components (`R`/`G`/`B`/`A`,
  initial `A`) of the **non-premultiplied** `in2` displacement-map
  pixel, and `P` is the `in` source. Honours the §9.11 dual-colour-space
  rule: `color-interpolation-filters` applies only to `in2` (the map),
  while `in` "must remain in its current colour space" — the source is
  passed through with the sRGB-identity transfer (no §13.9
  linearisation) and stays premultiplied. `scale == 0` is the §9.11
  identity; out-of-image displaced source coordinates read transparent
  black (the §9.11 note leaves sub-pixel interpolation unspecified, so
  the evaluator takes the nearest source texel). 6 new tests.

- round 336 — pixel-level `<feConvolveMatrix>` node evaluator in
  [`crate::filter_eval`] (`convolve_matrix` +
  `evaluate_convolve_matrix_node`). Implements the Filter Effects
  Module Level 1 §9.9 2-D linear convolution over the
  [`crate::filter_eval::FilterImage`] storage: the kernel is applied
  180°-rotated relative to the image (`kernelMatrix[orderX-J-1,
  orderY-I-1]`) per the spec's convolution-theory convention, with
  `divisor` / `bias` / `targetX` / `targetY` placement and all three
  `edgeMode`s (`duplicate` — the §9.9 initial — / `wrap` / `none`). An
  explicit invalid `divisor="0"` falls back to the §9.9.4 kernel-sum
  default (or `1` when that sum is zero). Both `preserveAlpha` modes are
  honoured: `false` (the initial) convolves all four channels on
  premultiplied data with `bias·ALPHA` using the convolved alpha;
  `true` temporarily un-premultiplies the colour channels, convolves
  them only (with `ALPHA = SOURCE`), and passes the source alpha through
  unchanged. A kernel whose length disagrees with `orderX·orderY` is a
  no-op. 9 new tests (7 core + 2 node wrappers).

- round 330 — pixel-level `<feMorphology>` node evaluator in
  [`crate::filter_eval`] (`morphology` + `evaluate_morphology_node`).
  Implements the Filter Effects Module Level 1 §9.17 morphological
  operator over the premultiplied
  [`crate::filter_eval::FilterImage`] storage: `dilate` takes the
  component-wise maximum and `erode` the component-wise minimum of the
  R,G,B,A values in the kernel rectangle (§9.17 width `2·radius_x`,
  height `2·radius_y`), realised as the symmetric inclusive integer
  window `[x − rx, x + rx] × [y − ry, y + ry]` clamped to the image
  (§9.17 attaches no `edgeMode`). Independent x/y radii are honoured, a
  negative or zero radius short-circuits to the identity (result =
  input), and operating on premultiplied values preserves the
  `Rᵖ, Gᵖ, Bᵖ ≤ Aᵖ` invariant the spec guarantees.

- round 327 — pixel-level `<feComponentTransfer>` node evaluator in
  [`crate::filter_eval`] (`component_transfer` +
  `evaluate_component_transfer_node`). Implements the Filter Effects
  Module Level 1 §9.7 per-channel remap `R' = feFuncR(R)`,
  `G' = feFuncG(G)`, `B' = feFuncB(B)`, `A' = feFuncA(A)` over the
  premultiplied [`crate::filter_eval::FilterImage`] storage. Each
  `<feFunc*>` transfer function is evaluated on non-premultiplied colour
  per the spec and re-premultiplied for storage; all five inline-defined
  types are honoured — `identity` (`C' = C`), `table` (piecewise-linear
  interpolation between the `n + 1` `tableValues`, `C = 1 → vn`),
  `discrete` (`n`-step `C' = v_floor(C·n)`, `C = 1 → v(n−1)`), `linear`
  (`C' = slope·C + intercept`), and `gamma`
  (`C' = amplitude·pow(C, exponent) + offset`) — with results clamped to
  `[0, 1]` in the resolved `color-interpolation-filters` working space.
  11 new unit tests cover each transfer-function formula, the table /
  discrete degenerate (empty / single-value) cases, the
  non-premultiplied round-trip, the transparent-pixel path, the node
  entry point (identity + discrete threshold), and its decline path.
- round 323 — pixel-level `<feBlend>` node evaluator in
  [`crate::filter_eval`] (`blend` + `evaluate_blend_node`). Implements
  the five blend modes SVG 1.1 §15.9 / Filter Effects §9.5 define inline
  — `normal`, `multiply`, `screen`, `darken`, `lighten` — over the
  premultiplied [`crate::filter_eval::FilterImage`] storage, with `in`
  the source image A (`Cs`), `in2` the backdrop image B (`Cb`), shared
  result opacity `qr = 1 − (1 − qa)·(1 − qb)`, and the resolved
  `color-interpolation-filters` working space. The remaining eleven
  `<blend-mode>` values (`overlay`, `color-dodge`, `color-burn`,
  `hard-light`, `soft-light`, `difference`, `exclusion`, `hue`,
  `saturation`, `color`, `luminosity`) defer to the un-staged
  `[COMPOSITING-1]` mixing formulae and are declined for the
  graph-level rasteriser. 9 new unit tests cover each mode's algebra,
  the shared opacity rule, `in2` zero-extension, the node entry point,
  and its decline paths.
- round 318 — pixel-level `<feMerge>`, `<feOffset>`, and
  `<feGaussianBlur>` node evaluators in [`crate::filter_eval`] (Filter
  Effects Module Level 1 §9.16 / §9.18 / §9.14). `feMerge` composites its
  `<feMergeNode>` layers bottom-to-top with the §9.16 `over` operator
  (`merge` + `evaluate_merge_node`). `feGaussianBlur` now honours all
  three §9.14 `edgeMode` values — `none` (the spec initial value, zero
  extension), `duplicate` (clamp to the edge pixel) and `wrap` (toroidal
  sampling) — via the new edge-aware `gaussian_blur_edge` /
  `evaluate_gaussian_blur_node`; the existing `none`-only `gaussian_blur`
  is retained for the `<feDropShadow>` §9.12 equivalent composite.
  `feOffset` gains a node entry point (`evaluate_offset_node`) over the
  already-implemented §9.18 bilinear shift. 20 new unit tests cover the
  three edge modes, the merge z-order/blend semantics, and each node
  evaluator's decline path.

## [0.1.7](https://github.com/OxideAV/oxideav-svg/compare/v0.1.6...v0.1.7) - 2026-06-15

### Added

- round 260 — SVG 2 §15.6 pointer-events property + inherited cascade + round-trip
- round 257 — SVG 2 §3.11 overflow property + non-inherited cascade + round-trip
- round 252 — SVG 2 §13.9 color-interpolation property + inherited cascade + round-trip
- round 247 — SVG 2 §13.10.1 color-rendering property + inherited cascade + round-trip
- round 235 — SVG 2 §13.10.4 image-rendering property + inherited cascade + round-trip
- round 228 — SVG 2 §13.10.3 text-rendering property + inherited cascade + round-trip
- round 221 — SVG 2 §13.10.2 shape-rendering property + inherited cascade + round-trip
- round 215 — SVG 1.1 §14.3.5 clip-rule property + scope-restricted cascade

### Other

- pixel-level <feFlood> evaluation (Filter Effects §9.13)
- pixel-level <feColorMatrix> evaluation (Filter Effects L1 §9.6)
- round 295 — correct feBlend citation §15.10 → §15.9
- round 295 — pixel-level feComposite (over + arithmetic) in filter_eval
- round 291 — SVG 1.1 §10.9.2 dominant-baseline property (parse + non-inherited cascade + round-trip)
- round 283: pixel-level feDropShadow evaluation per Filter Effects §9.12 equivalent composite
- complete SVG 1.1 §15.3 <filter> attribute set — filterRes + xlink:href chain inheritance
- capture <filter> filterUnits/primitiveUnits/color-interpolation-filters
- Round 261: SVG 1.1 §16.8.2 cursor property — parse + inherited cascade + round-trip
- neutralise pre-existing authoring-tool attribution in <metadata> comment
- drop release-plz.toml — use release-plz defaults across the workspace
- *(svg)* reorder CHANGELOG — round 215 entry above 0.1.6 section

### Added

- **Round 308** — pixel-level evaluation of `<feFlood>` in
  [`crate::filter_eval`] (Filter Effects Module Level 1 §9.13): "creates
  a rectangle filled with the color and opacity values from properties
  `flood-color` and `flood-opacity`. The rectangle is as large as the
  filter primitive subregion." New [`crate::filter_eval::flood`] fills
  the whole `width × height` subregion with one uniform pixel — the
  §9.13.1 `flood-color` decoded into the node's resolved
  `color-interpolation-filters` working space (§10), at the alpha
  `flood-color.a × flood-opacity` (the colour's own alpha channel
  multiplied with the §9.13.2 `flood-opacity`), stored premultiplied per
  the [`crate::filter_eval::FilterImage`] convention. New
  [`crate::filter_eval::evaluate_flood_node`] drives a parsed `Flood`
  node end-to-end over a subregion size (the primitive has no pixel
  input) and re-encodes to an 8-bit non-premultiplied sRGB RGBA buffer;
  non-`Flood` nodes are declined. 8 unit tests pin the opaque-white
  fill, alpha-only opacity scaling, the sRGB colour round-trip, the
  linearRGB endpoint invariance, transparent-colour / zero-opacity
  transparent-black, the default opaque-black node, and the decline
  path. The `<feFlood>` primitive was already parsed (round 7); this
  round gives it a node-level evaluator entry point, joining
  `feColorMatrix` / `feComposite` / `feDropShadow`.
- **Round 302** — pixel-level evaluation of `<feColorMatrix>` in
  [`crate::filter_eval`] (Filter Effects Module Level 1 §9.6). The
  parser already reduces every type variant to a flat row-major 4×5
  RGBA-bias matrix; the evaluator applies
  `[R' G' B' A' 1]ᵀ = M · [R G B A 1]ᵀ` per output channel. Per §9.6
  the calculation runs on **non-premultiplied** colour, so
  [`crate::filter_eval::color_matrix`] un-premultiplies each
  [`crate::filter_eval::FilterImage`] pixel, applies the matrix,
  clamps each result to `[0, 1]`, and re-premultiplies for storage.
  New [`crate::filter_eval::evaluate_color_matrix_node`] decodes an
  8-bit sRGB buffer into the node's resolved
  `color-interpolation-filters` working space (§10), evaluates, and
  re-encodes; non-`ColorMatrix` nodes are declined. 6 unit tests pin
  the identity no-op, cross-channel swap + bias, per-channel clamp,
  the luminanceToAlpha template, and the node decode/decline paths.
- **Round 295** — pixel-level evaluation of `<feComposite>` in
  [`crate::filter_eval`] (Filter Effects Module Level 1 §16 / SVG 1.1
  §15.12), for the two operators the staged specifications define
  **inline**:
  - **`over`** — the Porter-Duff `over`, given directly by SVG 1.1
    §15.9 ("'normal' blend mode is equivalent to `operator="over"`"):
    premultiplied `cr = (1 − qa)·cb + ca` per colour channel and
    `qr = 1 − (1 − qa)·(1 − qb)` for the result opacity, with image A
    the source (`in`) and image B the destination (`in2`).
  - **`arithmetic`** — the component-wise
    `result = k1·i1·i2 + k2·i1 + k3·i2 + k4`, clamped to `[0, 1]`,
    applied to every channel (alpha included) on the premultiplied
    operands; `k1..k4` default to `0`.
  - New [`crate::filter_eval::composite`] (two [`FilterImage`]
    operands → one) and the node entry point
    [`crate::filter_eval::evaluate_composite_node`], which decodes two
    8-bit sRGB RGBA buffers into the node's resolved
    `color-interpolation-filters` working space, evaluates, and
    re-encodes. The node entry point **declines** (`None`) the
    `in`/`out`/`atop`/`xor` operators whose formula bodies live in the
    un-staged `[PORTERDUFF]` / `[COMPOSITING-1]` companion spec, so the
    graph-level rasteriser can own those.
  - 10 new unit tests covering opaque/transparent/partial-alpha `over`,
    the arithmetic add / product / clamp terms, the un-staged-operator
    pass-through, and both node-entry paths.

- **Round 291** — SVG 1.1 §10.9.2 `dominant-baseline` property
  (`auto | use-script | no-change | reset-size | ideographic |
  alphabetic | hanging | mathematical | central | middle |
  text-after-edge | text-before-edge`) on text content elements.
  - New [`crate::element::DominantBaseline`] enum carried on
    [`crate::element::PaintState`]. Initial value
    [`DominantBaseline::Auto`]; the property is **NOT inherited** per
    the §10.9.2 attribute table, so
    [`crate::element::PaintState::merged_with_mctx`] resets it to the
    initial value before applying the element's own attribute
    (matching the `display` / `vector-effect` / `overflow`
    non-inheritance resets). Resolves through presentation attributes,
    inline `style="..."`, and `<style>`-block rules via the round-4
    cascade. Case-insensitive keyword matching; `inherit` / unknown
    tokens keep the post-reset value.
  - **Round-trip preservation** via a new
    [`crate::preserved::DominantBaselineBinding`] +
    [`crate::preserved::PreservedExtras::dominant_baselines`]
    side-channel — purely lexical, recorded at the topmost emit slot
    for each shape / `<g>` carrying a recognised `dominant-baseline=`
    attribute. The encoder re-emits the canonicalised (lowercase /
    hyphenated per §10.9.2) keyword on round-trip; explicit `auto`
    (the initial value) is preserved, the absent-attribute case is
    skipped.
  - The actual scaled-baseline-table construction + glyph positioning
    live in `oxideav-scribe` / `oxideav-raster`; this round delivers
    parse + non-inherited cascade + round-trip preservation. 22
    integration tests in `tests/round291_dominant_baseline.rs`.
- **Round 283** — pixel-level `<feDropShadow>` evaluation per the W3C
  Filter Effects Module Level 1 §9.12 normative equivalent composite,
  in the new [`crate::filter_eval`] module (the crate's first
  filter-primitive *evaluator*; the typed graph was parse-only).
  - [`crate::filter_eval::drop_shadow`] runs the five §9.12 steps over
    a premultiplied-RGBA [`crate::filter_eval::FilterImage`]: input
    alpha → §9.14 Gaussian blur → §9.18 offset → §9.13 flood
    composited with the §9.8 Porter-Duff `in` operator → §9.16 merge
    (`over`, input on top). Steps 3–5 are fused into one pass (§9.12
    explicitly permits not materialising the equivalent tree).
  - [`crate::filter_eval::gaussian_blur`] implements the §9.14
    three-box-blur approximation exactly (`d = floor(s·3·sqrt(2π)/4 +
    0.5)`; odd `d` → three centred boxes of size `d`; even `d` →
    left-boundary box + right-boundary box of size `d` + centred box
    of size `d+1`), with the §9.14 initial `edgeMode` `none` (zero
    extension), per-axis zero disabling that axis only and a negative
    `stdDeviation` disabling the primitive.
  - [`crate::filter_eval::offset`] implements §9.18 with bilinear
    interpolation for fractional offsets (the §9.18 recommendation).
  - Pixel maths honours the node's resolved
    `color-interpolation-filters` (§10): working space `linearRGB`
    (initial; `auto` resolves to it) or `sRGB`, with the SVG 2 §13.9
    transfer formula exposed as
    [`crate::filter_eval::srgb_to_linear`] /
    [`crate::filter_eval::linear_to_srgb`].
  - [`crate::filter_eval::evaluate_drop_shadow_node`] evaluates a
    parsed [`crate::filter::FilterPrimitiveNode`] end-to-end over an
    8-bit RGBA buffer; [`crate::filter_eval::DropShadowParams`]
    defaults carry the §9.12 initial values (`dx=dy=2`,
    `stdDeviation=2`, opaque-black flood, opacity 1).
  - 21 new tests (11 unit + 10 integration in
    `tests/round283_drop_shadow_eval.rs`) pin rendered output bytes
    hand-derived from the spec maths: the even-`d` impulse kernel
    `[1/12, 1/4, 1/3, 1/4, 1/12]` for `s=0.8`, the default `s=2`
    centre weight `0.175²`, kernel mass/symmetry, flood
    colour×opacity, merge-over in both working spaces, and fractional
    offsets.
- **Round 279** — `<filter>` element attribute set completed on the
  typed graph: SVG 1.1 §15.3 `filterRes` + `xlink:href`, the two
  attributes still missing after round 272.
  - New [`crate::filter::FilterRes`] (`x_pixels` / `y_pixels`) on
    `FilterGraph::filter_res`. Per §15.5 non-integer values truncate
    toward zero at parse time; a single `<number-optional-number>`
    expands to both axes; negative (error) / zero (disables rendering
    of the referencing element) values are captured as-is for the
    rasteriser to enforce; absent / non-numeric → `None`.
  - `FilterGraph::href` captures `xlink:href` / SVG 2 `href` as the
    bare target id. New
    [`crate::filter::resolve_filter_element_chain`] implements the
    §15.3 inheritance: attributes defined on the referenced `<filter>`
    but absent on this one merge in (nearest chain definition wins;
    `id` and the reference attribute never inherit); an element with
    no filter nodes inherits the filter nodes of the nearest chain
    member that has any (unknown `fe*` children count as filter
    nodes). Chains resolve indirectly up to
    [`crate::filter::FILTER_HREF_DEPTH_CAP`] (8) hops; cycles,
    self-references and dangling ids terminate gracefully.
  - The decoder resolves every captured `FilterDef::graph` in a
    post-pass after the defs pre-walk, so forward references work;
    `FilterDef::element` stays the verbatim source and round-trip
    emission is unchanged.
  - 23 integration tests in `tests/round279_filter_href.rs`.
- **Round 272** — `<filter>` coordinate-system + colour-space
  attributes captured in the typed [`crate::filter::FilterGraph`].
  The primitive set was already complete; this fills the gap where the
  filter element's own `filterUnits` / `primitiveUnits` /
  `color-interpolation-filters` were dropped at parse time.
  - New [`crate::filter::FilterUnits`] enum (`UserSpaceOnUse` /
    `ObjectBoundingBox`) carried on `FilterGraph::filter_units` and
    `FilterGraph::primitive_units`. Per SVG 1.1 §15.7.2 the two
    attributes have *different* defaults: `filterUnits` →
    `objectBoundingBox`, `primitiveUnits` → `userSpaceOnUse`. Unknown
    values fall back to those defaults.
  - New [`crate::filter::ColorInterpolationFilters`] enum (`Auto` /
    `Srgb` / `LinearRgb`) with `Default` = `LinearRgb`. Per SVG 1.1
    §11.7.1 the property is inherited, applies to filter primitives,
    and has initial value `linearRGB` (distinct from
    `color-interpolation`'s `sRGB`). The `<filter>` element's value is
    stored on `FilterGraph::color_interpolation_filters`
    (`Option`, `None` = absent), and each primitive's resolved value
    lands on `FilterPrimitiveNode::color_interpolation_filters`:
    primitive's own attribute wins, else the `<filter>`-inherited
    value, else the initial `linearRGB`. `inherit` with no cascade
    context collapses to the initial value.
  - `filterUnits` / `primitiveUnits` / `color-interpolation-filters`
    continue to survive the verbatim XML round-trip; the typed graph
    is a parallel pre-rasteriser representation.
  - 9 new tests in `tests/round12_filter_units.rs`.
- **Round 261** — SVG 1.1 §16.8.2 `cursor` property
  (`[ [<funciri> ,]* [ auto | crosshair | default | pointer | move |
  e-resize | ne-resize | nw-resize | n-resize | se-resize |
  sw-resize | s-resize | w-resize | text | wait | help ] ] |
  inherit`) on the §16.8.2 applies-to set (container + graphics
  elements). SVG 2 retains `cursor` as a presentation attribute and
  defers the property definition to CSS; the SVG 1.1 §16.8.2
  definition carries the keyword set + grammar implemented here.
  - New typed [`crate::element::CursorKeyword`] (sixteen generic
    keywords) + [`crate::element::CursorValue`] (funciri list +
    mandatory trailing generic keyword) carried on
    [`crate::element::PaintState`]. Initial value `auto`; the
    property IS inherited per the §16.8.2 attribute table (no
    per-element reset). Resolves through presentation attributes,
    inline `style="…"`, and `<style>`-block rules via the round-4
    cascade; case-insensitive matching; `inherit` / invalid payloads
    keep the inherited value.
  - **`<funciri>` list grammar** — zero or more comma-separated
    `url(...)` custom-cursor references precede the generic keyword.
    Per §16.8.2 the generic keyword is the mandatory fallback ("it
    must use the generic cursor at the end of the list"), so a
    funciri list without one is invalid. Top-level comma splitting
    keeps a comma-bearing IRI (e.g. `data:`) as one item per the
    `<funciri>` production.
  - **Round-trip preservation.** New
    [`crate::preserved::CursorBinding`] +
    [`crate::preserved::PreservedExtras::cursors`] side-channel;
    canonical emission lowercases the `url` token + generic keyword,
    preserves each IRI verbatim, and joins items comma-and-space
    (`URL( #c ) , POINTER` → `url(#c), pointer`). Explicit
    `cursor="auto"` is preserved per the explicit-initial-value
    policy. Coexists with the §15.6 `pointer-events` + §3.11
    `overflow` carriers on the same `<g>`.
  - The actual cursor display (funciri resolution + fallback walk)
    is interactive-UA work; this round delivers parse + inherited
    cascade + round-trip preservation.
  - 27 integration tests in `tests/round261_cursor.rs`.

- **Round 260** — SVG 2 §15.6 `pointer-events` property
  (`bounding-box | visiblePainted | visibleFill | visibleStroke |
  visible | painted | fill | stroke | all | none`) on the §15.6
  applies-to set (container elements, graphics elements, `<use>`).
  - New typed [`crate::element::PointerEvents`] enum carried on
    [`crate::element::PaintState`]. Per the §15.6 attribute table the
    initial value is `VisiblePainted`; the property IS inherited per
    the same table, so [`PaintState::merged_with_mctx`] does NOT reset
    `pointer_events` before applying the element's own attribute
    (matching the round-118 `visibility` / round-172 `text-anchor` /
    round-205 `paint-order` / round-247 `color-rendering` / round-252
    `color-interpolation` inheritance flow; distinct from the
    round-118 `display` / round-209 `vector-effect` / round-257
    `overflow` non-inherited resets). Resolves through presentation
    attributes, inline `style="…"`, and `<style>`-block rules via the
    existing round-4 cascade. Case-insensitive keyword matching;
    `inherit` / unknown tokens keep the inherited value.
  - **Round-trip preservation.** New
    [`crate::preserved::PointerEventsBinding`] +
    [`crate::preserved::PreservedExtras::pointer_eventss`] side-channel
    captures the canonicalised keyword at the topmost emit slot for
    each shape / `<g>` carrying a recognised `pointer-events=`
    attribute. Mirrors the round-247 / round-252 / round-257 lexical
    carriers — the property cascades through `PaintState`, but the
    binding only records the source emit slot so a hand-authored
    `<g pointer-events="none">` survives a `parse_svg_with_extras →
    write_svg_with_extras` cycle on the same group element. The
    encoder re-emits `pointer-events=` on the matching shape / `<g>`
    on round-trip.
  - **Explicit initial value `visiblePainted` is preserved.** Mirrors
    the round-221 `shape-rendering` / round-247 `color-rendering` /
    round-252 `color-interpolation` / round-257 `overflow`
    explicit-initial-value policy — even though `visiblePainted` is
    the §15.6 initial value, an explicit author write carries intent
    (e.g. an inheritance reset on a descendant of a
    `<g pointer-events="none">`). The absent-attribute case is still
    skipped so an initial-value document doesn't bloat with redundant
    `pointer-events="visiblePainted"` on every element.
  - **Canonical mixed-spelling emission.** §15.6 spells the keyword
    set with three conventions: lower-camelCase for the four
    `visible*` keywords (`visiblePainted` / `visibleFill` /
    `visibleStroke`), a hyphen for `bounding-box`, and all-lowercase
    for `visible` / `painted` / `fill` / `stroke` / `all` / `none`.
    Source `VISIBLEPAINTED` / `BOUNDING-BOX` / `Painted` round-trip
    as the canonical §15.6 spelling.
  - **Coexists with the §3.11 `overflow` + §13.9 / §13.10.x
    rendering / colour hints.** §15.6 (hit-test gate) is orthogonal
    to §3.11 (clipping rectangle), §13.9 (working colour space) and
    §13.10.x (rendering-quality hints) — they can all ride on the
    same `<g>` without interfering. Each side-channel records
    independently and the encoder emits every recognised attribute
    on round-trip.
  - The actual hit-test gating (the §15.6 visibility + paint suffix
    resolution that decides whether a pointer event over the element
    counts as a hit) happens in the interactive layer (e.g.
    `oxideav-pipeline` event routing or `oxideav-raster` hit-test
    queries against the rendered scene); this round delivers parse +
    inherited cascade + round-trip preservation. A downstream
    consumer reads the resolved value off the carried `PaintState`
    or off the per-element `PointerEventsBinding`.
  - 23 integration tests in `tests/round260_pointer_events.rs`.

- **Round 257** — SVG 2 §3.11 `overflow` property
  (`visible | hidden | scroll | auto`) on the §3.11 summary-table
  element list (`<svg>` / `<symbol>` / `<marker>` / `<pattern>` /
  `<image>` / `<text>` / `<iframe>` / `<foreignObject>`).
  - New typed [`crate::element::Overflow`] enum carried on
    [`crate::element::PaintState`]. Per the §3.11 summary table the
    initial value is `Visible`; the property is NOT inherited per
    CSS 2.1 §11.1.1 (matching the round-118 `display` and round-209
    `vector-effect` non-inheritance reset policy), so
    [`PaintState::merged_with_mctx`] resets `overflow` to the
    initial value before applying the element's own attribute.
    Resolves through presentation attributes, inline `style="…"`,
    and `<style>`-block rules via the existing round-4 cascade.
    Case-insensitive keyword matching; `inherit` / unknown tokens
    keep the post-reset value (per the §3.11 normative tolerance
    note: "as `overflow="invalid"` will result in a rule setting
    overflow to visible").
  - **Round-trip preservation.** New
    [`crate::preserved::OverflowBinding`] +
    [`crate::preserved::PreservedExtras::overflows`] side-channel
    captures the canonicalised lowercase keyword at the topmost
    emit slot for each shape / `<g>` carrying a recognised
    `overflow=` attribute. The carrier is purely lexical (the
    cascade itself does not inherit `overflow`), so a hand-authored
    `<g overflow="hidden">` survives a
    `parse_svg_with_extras → write_svg_with_extras` cycle on the
    same group element. The encoder re-emits `overflow=` on the
    matching shape / `<g>` on round-trip.
  - **Explicit initial value `visible` is preserved.** Mirrors the
    round-221 `shape-rendering` / round-247 `color-rendering` /
    round-252 `color-interpolation` explicit-initial-value policy
    — even though `visible` is the §3.11 initial value, an
    explicit author write carries intent (e.g. an override of the
    UA-stylesheet `overflow: hidden` default that fires for
    non-root `<svg>` / `<symbol>` / `<marker>` / `<pattern>` /
    `<image>` per §3.11). The absent-attribute case is still
    skipped so an initial-value document doesn't bloat with
    redundant `overflow="visible"` on every element.
  - **Canonical lowercase emission.** Source `HIDDEN` / `Hidden` /
    `SCROLL` round-trip as `hidden` / `scroll` — §3.11 reuses the
    CSS 2.1 keyword set verbatim, all lowercase (distinct from the
    §13.9 mixed-case spellings `sRGB` / `linearRGB` and the
    §13.10.x lower-camelCase spellings).
  - **Coexists with the §13.x rendering / colour hints.** §3.11
    (clipping rectangle) and §13.10.1 `color-rendering` (quality
    hint) / §13.9 `color-interpolation` (working colour space) are
    orthogonal properties — they can all ride on the same `<g>`
    without interfering. Each side-channel records independently
    and the encoder emits every recognised attribute on round-trip.
  - The actual clipping-rectangle establishment (per §3.11
    `hidden` / `scroll` → clip-to-viewport behaviour) + the
    UA-stylesheet override of the initial value to `hidden` for
    non-root `<svg>` / `<symbol>` / `<marker>` / `<pattern>` /
    `<image>` + the renderer-side resolution of `scroll` / `auto`
    against UA scrolling-mechanism availability all happen in
    `oxideav-raster`; this round delivers parse + non-inherited
    cascade + round-trip preservation. A downstream rasteriser
    reads the resolved value off the carried `PaintState` or off
    the per-element `OverflowBinding`.
  - 21 integration tests in `tests/round257_overflow.rs`.

- **Round 252** — SVG 2 §13.9 `color-interpolation` property
  (`auto | sRGB | linearRGB`) on container, graphics, and gradient
  elements (plus `<use>` and `<animate>` per the §13.9 applies-to list).
  - New typed [`crate::element::ColorInterpolation`] enum carried on
    [`crate::element::PaintState`]. Inherited per the §13.9 attribute
    table; initial value `Srgb` (distinct from the §13.10.x rendering
    hints whose initial value is `Auto`). Resolves through presentation
    attributes, inline `style="…"`, and `<style>`-block rules via the
    existing round-4 cascade. Case-insensitive keyword matching;
    `inherit` / unknown tokens keep the inherited value.
  - **Round-trip preservation.** New
    [`crate::preserved::ColorInterpolationBinding`] +
    [`crate::preserved::PreservedExtras::color_interpolations`]
    side-channel captures the canonicalised §13.9 mixed-case keyword
    at the topmost emit slot for each shape / `<g>` carrying a
    recognised `color-interpolation=` attribute. A
    `<g color-interpolation=…>` ancestor records on the group's own
    slot (one binding per source attribute slot — not per cascaded
    descendant). The encoder re-emits `color-interpolation=` on the
    matching element on round-trip.
  - **Canonical mixed-case emission.** Source `SRGB` / `srgb` /
    `LINEARRGB` / `linearrgb` all round-trip as the §13.9 attribute
    table's spelling (`sRGB` / `linearRGB`). Distinct from the
    lower-camelCase canonicalisation used for the §13.10.x hints.
  - **Explicit `sRGB` (initial value) is preserved.** Mirrors the
    round-247 `color-rendering` "explicit `auto` is recorded" policy
    — even though `sRGB` is the §13.9 initial value, an explicit
    author write carries intent (e.g. an inheritance reset on a
    descendant of a `<g color-interpolation="linearRGB">`). The
    absent-attribute case is still skipped so an initial-value
    document doesn't bloat with redundant
    `color-interpolation="sRGB"` on every element.
  - **Coexists with the §13.10.x rendering hints.** §13.9 (working
    colour space selector) and §13.10.1 `color-rendering` (quality
    hint) are orthogonal properties — both can ride on the same
    `<g>` without interfering. The encoder emits every recognised
    attribute on the matching element on round-trip.
  - The actual working-colour-space selection (sRGB vs linearised RGB
    for gradient stop interpolation, SMIL colour animation, and
    graphics-element compositing) happens in `oxideav-raster`; this
    round delivers parse + inherited cascade + round-trip
    preservation. A downstream rasteriser reads the resolved value
    off the carried `PaintState` or off the per-element
    `ColorInterpolationBinding`. The §13.9 informative note that the
    filter-effects sibling property `color-interpolation-filters`
    governs the filter primitive graph instead is documented but not
    enforced here — that interaction belongs to the round-7 / round-10
    filter graph work in `oxideav-filter`.
  - 22 integration tests in `tests/round252_color_interpolation.rs`
    cover the no-attribute baseline (no binding), each of the three
    §13.9 keywords recorded with canonical case, case-insensitive
    matching, explicit-`sRGB` recording (initial value preserved),
    explicit-`auto` recording, `inherit` skipping, unknown-token
    tolerance, empty-value skipping, presentation-attribute /
    `style="…"` cascade resolution, inheritance through a parent
    `PaintState`, child override of the inherited value, round-trip
    emission on `<g>` and on a bare `<rect>`, double round-trip
    convergence, source-case canonicalisation through round-trip,
    `parse_svg` (no extras) still loading the document, the
    per-child-override-records-separately pattern, and coexistence
    with the round-221 / round-228 / round-247 hints on the same
    group element.

- **Round 247** — SVG 2 §13.10.1 `color-rendering` property
  (`auto | optimizeSpeed | optimizeQuality`) on container, graphics,
  and gradient elements (plus `<use>` and `<animate>` per the
  §13.10.1 applies-to list).
  - New typed [`crate::element::ColorRendering`] enum carried on
    [`crate::element::PaintState`]. Inherited per the §13.10.1
    attribute table; initial value `Auto`. Resolves through
    presentation attributes, inline `style="…"`, and `<style>`-block
    rules via the existing round-4 cascade. Case-insensitive keyword
    matching; `inherit` / unknown tokens keep the inherited value.
  - **Round-trip preservation.** New
    [`crate::preserved::ColorRenderingBinding`] +
    [`crate::preserved::PreservedExtras::color_renderings`]
    side-channel captures the canonicalised camelCase keyword at the
    topmost emit slot for each shape / `<g>` carrying a recognised
    `color-rendering=` attribute. A `<g color-rendering=…>` ancestor
    records on the group's own slot (one binding per source attribute
    slot — not per cascaded descendant). The encoder re-emits
    `color-rendering=` on the matching element on round-trip.
  - **Explicit `auto` is preserved** — mirrors the round-221
    `shape-rendering` / round-228 `text-rendering` / round-235
    `image-rendering` policy. The absent-attribute case is still
    skipped so an initial-value document doesn't bloat the output
    with redundant `color-rendering="auto"` on every element.
  - **Canonical camelCase emission.** Source `OPTIMIZEQUALITY` /
    `optimizequality` / `OptimizeQuality` all round-trip as
    `optimizeQuality`, matching the §13.10.1 attribute table's
    spelling.
  - **Coexists with the other rendering hints.** All four §13.10.x
    inherited hints (`color-rendering` / `shape-rendering` /
    `text-rendering` / `image-rendering`) can ride on the same `<g>`
    without interfering; each side-channel records independently and
    the encoder emits every recognised attribute.
  - The actual working colour-space selection for interpolation and
    compositing happens in `oxideav-raster`; this round delivers
    parse + inherited cascade + round-trip preservation. A
    downstream rasteriser reads the resolved value off the carried
    `PaintState` or off the per-element `ColorRenderingBinding`. The
    §13.10.1 informative note that `color-rendering` takes precedence
    over the filter-effects `color-interpolation-filters` property is
    documented but not enforced here — that interaction lives in
    `oxideav-filter` / the filter primitive graph (round-10 work).
  - 21 integration tests in `tests/round247_color_rendering.rs`
    cover the no-attribute baseline, each of the three spec
    keywords with canonical camelCase, case-insensitive matching,
    explicit-`auto` recording, `inherit` skipping, unknown-token
    tolerance, empty-value skipping, presentation-attribute /
    `style="…"` cascade resolution, inheritance through a parent
    `PaintState`, child override of the inherited value, round-trip
    emission on `<g>` and on a bare `<rect>`, double round-trip
    convergence, source-case canonicalisation through round-trip,
    `parse_svg` (no extras) still loading the document, the
    per-child-override-records-separately pattern, and coexistence
    with the round-221 / round-228 / round-235 hints on the same
    group element.

- **Round 235** — SVG 2 §13.10.4 `image-rendering` property
  (`auto | optimizeQuality | optimizeSpeed`) on `<image>` and (via
  the cascade) any descendant element that paints raster content.
  - New typed [`crate::element::ImageRendering`] enum carried on
    [`crate::element::PaintState`]. Inherited per the §13.10.4
    attribute table; initial value `Auto`. Resolves through
    presentation attributes, inline `style="…"`, and `<style>`-block
    rules via the existing round-4 cascade. Case-insensitive keyword
    matching; `inherit` / unknown tokens keep the inherited value.
  - **Round-trip preservation.** New
    [`crate::image::SvgImage::image_rendering`] field captures the
    canonicalised camelCase keyword off each source `<image>`
    element. The encoder re-emits `image-rendering=` on the
    matching `<image>` on round-trip; the §13.10.4 property applies
    to images, so the natural emit site is the image itself rather
    than a separate side-channel table.
  - **Explicit `auto` is preserved** — mirrors the round-221
    `shape-rendering` / round-228 `text-rendering` policy. The
    absent-attribute case is still skipped so an initial-value
    document doesn't bloat the output.
  - **Canonical camelCase emission.** Source `OPTIMIZEQUALITY` /
    `optimizequality` / `OptimizeQuality` all round-trip as
    `optimizeQuality`, matching the §13.10.4 attribute table's
    spelling.
  - **Coexists with `shape-rendering`** — the two inherited hints
    record independently (the `<g shape-rendering=…>` carrier lives
    on the round-221 side-channel, the `<image image-rendering=…>`
    carrier lives on the per-image `SvgImage::image_rendering`
    slot) and the encoder emits both attributes faithfully.
  - 18 integration tests in `tests/round235_image_rendering.rs`
    cover the no-attribute baseline, each of the three spec
    keywords, case-insensitive matching, explicit `auto` recording,
    `inherit` skipping, unknown-token tolerance, empty-value
    skipping, presentation-attribute / `style="…"` cascade
    resolution, inheritance through a parent `PaintState`, child
    override, round-trip emission on `<image>`, double round-trip
    convergence, source-case canonicalisation, `parse_svg` (no
    extras) loading the document, and coexistence with the
    round-221 `shape-rendering` attribute on a sibling subtree.

- **Round 228** — SVG 2 §13.10.3 `text-rendering` property
  (`auto | optimizeSpeed | optimizeLegibility | geometricPrecision`)
  on `<text>` and (via the cascade) descendant `<tspan>` /
  `<textPath>` runs.
  - New typed [`crate::element::TextRendering`] enum carried on
    [`crate::element::PaintState`]. Inherited per the §13.10.3
    attribute table; initial value `Auto`. Resolves through
    presentation attributes, inline `style="…"`, and `<style>`-block
    rules via the existing round-4 cascade. Case-insensitive
    keyword matching; `inherit` / unknown tokens keep the inherited
    value.
  - **Round-trip preservation.** New
    [`crate::preserved::TextRenderingBinding`] +
    [`crate::preserved::PreservedExtras::text_renderings`]
    side-channel captures the canonicalised camelCase keyword
    string at the topmost emit slot for each `<text>` / `<g>`
    carrying a recognised `text-rendering=` attribute. A `<g
    text-rendering=…>` ancestor records on the group's own slot
    (one binding per source-attribute slot — not per cascaded
    descendant). The encoder re-emits `text-rendering=` on the
    matching element on round-trip.
  - **Explicit `auto` is preserved** — mirrors the round-221
    `shape-rendering` policy. The absent-attribute case is still
    skipped so an initial-value document doesn't bloat the output.
  - **Canonical camelCase emission.** Source `OPTIMIZELEGIBILITY` /
    `optimizelegibility` / `OptimizeLegibility` all round-trip as
    `optimizeLegibility`, matching the §13.10.3 attribute table's
    spelling.
  - **Coexists with `shape-rendering`** on the same `<g>` — the
    two side-channels (round-221 and round-228) record
    independently and the encoder emits both attributes on the
    same element.
  - The actual rendering-hint consumption (anti-alias toggle,
    hint suspension) happens in `oxideav-raster` /
    `oxideav-scribe`; this round delivers parse + inherited
    cascade + round-trip preservation.
  - 20 integration tests in `tests/round228_text_rendering.rs`
    cover the no-attribute baseline, each of the four spec
    keywords with canonical camelCase, case-insensitive matching,
    explicit-`auto` recording, `inherit` skipping, unknown-token
    tolerance, empty-value skipping, presentation-attribute /
    `style="…"` cascade resolution, inheritance through a `<g>`
    ancestor, child override of the inherited value, round-trip
    emission on `<g>`, double round-trip convergence,
    `parse_svg` (no extras) still loading the document,
    per-child-override-records-separately, and coexistence with
    the round-221 `shape-rendering` attribute on the same group.

- **Round 221** — SVG 2 §13.10.2 `shape-rendering` property
  (`auto | optimizeSpeed | crispEdges | geometricPrecision`) on shapes.
  - New typed [`crate::element::ShapeRendering`] enum carried on
    [`crate::element::PaintState`]. Inherited per the §13.10.2
    attribute table; initial value `Auto`. Resolves through
    presentation attributes, inline `style="…"`, and `<style>`-block
    rules via the existing round-4 cascade. Case-insensitive keyword
    matching; `inherit` / unknown tokens keep the inherited value
    (matches the tolerant policy of `text-anchor` / `paint-order`).
  - **Round-trip preservation.** New
    [`crate::preserved::ShapeRenderingBinding`] +
    [`crate::preserved::PreservedExtras::shape_renderings`]
    side-channel captures the canonicalised camelCase keyword string
    at the topmost emit slot for each shape / `<g>` carrying a
    recognised `shape-rendering=` attribute. A `<g shape-rendering=…>`
    ancestor records on the group's own slot (one binding per
    source-attribute slot — not per cascaded descendant), so a
    hand-authored grouping attribute survives a
    `parse_svg_with_extras → write_svg_with_extras` cycle. The
    encoder re-emits `shape-rendering=` on the matching shape /
    `<g>` on round-trip.
  - **Explicit `auto` is preserved.** Unlike round-205 `paint-order`
    / round-209 `vector-effect` (which skip the initial value to
    avoid no-op binding bloat), an explicit author
    `shape-rendering="auto"` is recorded — it carries author intent
    (e.g. an inheritance reset on a descendant of a `<g
    shape-rendering="optimizeSpeed">`). The absent-attribute case
    is still skipped so an initial-value document doesn't bloat the
    output.
  - **Canonical camelCase emission.** Source
    `OPTIMIZESPEED` / `optimizespeed` / `OptimizeSpeed` all
    round-trip as `optimizeSpeed`, matching the §13.10.2 attribute
    table's spelling.
  - The actual rendering-hint consumption (anti-alias toggle, edge
    snap) happens in `oxideav-raster`; this round delivers parse +
    inherited cascade + round-trip preservation. A downstream
    rasteriser reads the resolved value off the carried `PaintState`
    or off the per-shape `ShapeRenderingBinding`.
  - 18 integration tests in `tests/round221_shape_rendering.rs`
    cover the no-attribute baseline (no binding), each of the four
    spec keywords recorded with canonical camelCase, case-insensitive
    matching, explicit-`auto` recording, `inherit` skipping,
    unknown-token tolerance, empty-value skipping, presentation-
    attribute / `style="…"` cascade resolution, the inheritance
    through a `<g>` ancestor, round-trip emission on `<rect>` /
    `<path>` / `<g>`, double round-trip convergence,
    `parse_svg` (no extras) still loading the document, and the
    per-child-override-records-separately pattern.

- **Round 215** — SVG 1.1 §14.3.5 `clip-rule` property
  (`nonzero | evenodd | inherit`) on graphics elements within a
  `<clipPath>` element.
  - New typed [`crate::defs::ClipPathDef::clip_rule`] exposes the
    resolved [`oxideav_core::FillRule`] for the merged-path
    representation. Initial value `nonzero` per the §14.3.5 attribute
    table.
  - **Inheritance + override.** `clip-rule` is an inherited
    presentation property per §14.3.5; the value on the `<clipPath>`
    element cascades to its shape children, and a per-shape
    `clip-rule=` overrides. Multiple shape children flatten into one
    merged path; the resolved rule is the first contributing shape's
    rule (subsequent children that disagree are tolerated but only
    one rule survives onto the merged path).
  - **Scope-restricted.** Per §14.3.5, the property "only applies to
    graphics elements that are contained within a 'clipPath'
    element". `clip-rule=` on the referencing element (the shape
    with `clip-path="url(#…)"`) is silently ignored — matching the
    spec's second worked example.
  - **Round-trip preservation.** New
    [`crate::preserved::ClipRuleBinding`] +
    [`crate::preserved::PreservedExtras::clip_rules`] side-channel
    records the canonical keyword (`nonzero` / `evenodd`) for each
    captured `<clipPath>` whose resolved rule deviates from the
    `nonzero` initial value OR whose subtree carries an explicit
    author `clip-rule=`. The binding keys on the path-bytes
    fingerprint the encoder uses for its own clipPath dedup, so the
    encoder routes the keyword to the right auto-generated def even
    when the source `<clipPath id="...">` id is rewritten on
    round-trip. The encoder re-emits `clip-rule="..."` on the inner
    `<path>` of the `<clipPath>` def (matching the §14.3.5 worked
    example structure). A pristine document with no explicit author
    keyword skips the binding entirely.
  - **Case-insensitive matching** of `nonzero` / `evenodd`; unknown /
    malformed tokens (including the spec's `inherit` keyword and any
    author typo) fall back to the initial value `nonzero` without
    recording a binding.
  - **Id-less `<clipPath>` skipped** — an id-less `<clipPath>` cannot
    be referenced and has no round-trip emit site, so its
    `clip-rule=` (if any) is silently dropped.
  - Actual clip-rule evaluation lives in `oxideav-raster`; this
    round delivers parse + scope-restricted cascade + round-trip
    preservation. `oxideav_core::Path` (used for `Group::clip`)
    has no `fill_rule` field today, so a rasterizer that wants the
    non-default rule reads it from
    [`crate::defs::ClipPathDef`] (or via the side-channel binding
    keyed by the same path fingerprint).
  - 18 integration tests in `tests/round215_clip_rule.rs`.

- **Round 209** — SVG 2 §8.13 `vector-effect` property
  (`none | [ non-scaling-stroke | non-scaling-size | non-rotation |
  fixed-position ]+ [ viewport | screen ]?`) on graphics elements and
  `<use>`.
  - New [`crate::element::VectorEffectKeyword`] +
    [`crate::element::VectorEffectHost`] +
    [`crate::element::VectorEffect`] types capture the §8.13 grammar.
    `VectorEffect::parse_custom` resolves the `[ … ]+` keyword list
    (each effect at most once, source order preserved) plus the
    optional host suffix.
  - `vector-effect` joins the resolved-property surface on
    [`crate::element::PaintState`] (initial [`VectorEffect::None`]).
    NOT inherited per the §8.13 attribute table — the
    [`PaintState::merged_with_mctx`] cascade resets the field to the
    initial value at every element before applying the element's own
    attribute, mirroring the round-118 `display` non-inheritance
    reset.
  - Resolves through presentation attributes, inline `style="..."`,
    and `<style>`-block rules via the existing round-4 cascade. Empty
    / `none` / `inherit` payloads fall back to the initial value;
    unknown keywords are silently dropped (matches the tolerant
    paint-order / text-anchor / visibility policy).
  - **Round-trip preservation.** New
    [`crate::preserved::VectorEffectBinding`] +
    [`crate::preserved::PreservedExtras::vector_effects`] side-channel
    captures the canonicalised keyword string (lowercased, whitespace
    collapsed to single spaces, duplicates dropped) at the emit slot
    for each graphics element / `<g>` carrying a recognised non-`none`
    `vector-effect=` attribute. The encoder re-emits the attribute on
    the matching shape / group on round-trip. The canonical form omits
    the implicit `viewport` host suffix; an explicit `viewport` /
    `screen` is preserved.
  - The actual transform suppression happens in `oxideav-raster`;
    this round only parses, exposes the resolved value on
    `PaintState`, and round-trips the source attribute.
  - 17 integration tests in `tests/round209_vector_effect.rs`.

- **Round 205** — SVG 2 §13.8 `paint-order` property
  (`normal | [ fill || stroke || markers ]`) on shapes (`<rect>` /
  `<circle>` / `<ellipse>` / `<line>` / `<polyline>` / `<polygon>` /
  `<path>`).
  - New [`crate::element::PaintOp`] + [`crate::element::PaintOrder`]
    types capturing the §13.8 grammar. `PaintOrder::parse_custom`
    resolves the `[ fill || stroke || markers ]` keyword list into a
    three-deep operation list with the §13.8 "omitted keywords are
    appended in normal order" rule (`paint-order: stroke` resolves to
    `stroke fill markers`). Unknown keywords are tolerated; an empty /
    all-unknown list falls back to `Normal`.
  - `paint-order` joins the inherited paint cascade on
    [`crate::element::PaintState`] (initial value `Normal` per the
    §13.8 attribute table). Resolves through presentation attributes,
    inline `style="..."`, and `<style>`-block rules via the existing
    round-4 cascade.
  - **Scene-graph paint-operation order.** `oxideav_core::PathNode`
    paints fill before stroke (the §13.8 `normal` order); when the
    resolved order would paint stroke BEFORE fill, the shape branch
    splits into TWO single-purpose PathNodes inside a wrapping
    `Group` — a stroke-only PathNode first (`fill: None`) then a
    fill-only PathNode second (`stroke: None`) — so the composited
    result honours the requested order under the round-1 scene-graph
    model. The `markers` slot parses and round-trips but emits no
    node today (`oxideav_core::Node` has no `Marker` variant — the
    round-104 `<marker>` capture still applies). Shapes with no
    stroke (or no fill) trivially collapse back to a single PathNode.
  - **Round-trip preservation.** New
    [`crate::preserved::PaintOrderBinding`] +
    [`crate::preserved::PreservedExtras::paint_orders`] side-channel
    captures the canonicalised keyword string (lowercased, whitespace
    collapsed, duplicates dropped) at the topmost emit slot for each
    shape carrying a non-`normal` `paint-order=` attribute. A
    `<g paint-order="…">` ancestor records on the group's own slot so
    the source representation survives a
    `parse_svg_with_extras → write_svg_with_extras` cycle. The
    encoder re-emits the attribute on the matching `<g>` / `<path>` /
    `<rect>` / `<circle>` / `<ellipse>` / `<line>` / `<polyline>` /
    `<polygon>`.
  - **§9.6.1 `pathLength` interplay.** When a shape carries both
    `paint-order` (stroke-first) and `pathLength`, the round-21
    pathLength binding now targets the stroke-bearing PathNode in the
    split so the dasharray rescaling attaches to the path that
    actually carries the stroke (`find_inner_path_subpath` picks the
    fill-none/stroke-some child when a two-child group is detected).
  - 15 integration tests in `tests/round205_paint_order.rs` cover
    the default (no split), explicit `normal`, the §13.8 example
    (`paint-order: stroke`), the explicit two-keyword form
    (`stroke fill`), the full three-keyword form
    (`stroke fill markers`), the fill-first / markers-only forms
    (no split), the stroke-without-stroke and unknown-keyword
    fallbacks, cascade resolution via a `<g>` ancestor / inline
    `style=` / a `<style>`-block rule, the
    `PreservedExtras::paint_orders` round-trip carrier, the
    keyword-canonicalisation policy, and the no-binding-when-normal
    case.

- **Round 199** — SVG 2 §11.2 / §11.2.2 list-of-values on `x`, `y`,
  `dx`, `dy` and `rotate` for `<text>` and `<tspan>` elements.
  - Earlier rounds parsed only the first scalar of each attribute
    (`x="10 50 100"` collapsed to `x=10`). Round 199 parses the full
    list and applies the n-th value to the n-th character per the
    §11.2.2 "n-th character" rule.
  - `<tspan>` lists overlay onto document-wide per-character vectors
    at the current character ordinal (a `<tspan x="100 200">` mid-
    `<text>` seats its first two characters at exactly 100 and 200).
  - `rotate` follows the §11.2.2 sticky-final rule: a list shorter
    than the run's character count has its final supplied value
    apply to every trailing character.
  - Composes cleanly with rounds 176 (§11.5 chunk boundaries), 187
    (§11.2.1 `textLength` rescale) and 172 (§11.10.1.1 `text-anchor`
    shift). A `<tspan x="100 200">` still opens a fresh chunk via
    the first list value, then places its remaining characters
    within that chunk.
  - Lenient list grammar (whitespace and / or single commas;
    over-supplied tokens silently dropped; empty `rotate=""` is a
    no-op).
  - Whitespace runs (leading / trailing / inter-tspan source-format
    whitespace) leave the pen unchanged, matching the round-2
    `max_advance == 0 ⇒ pen.x = origin_x` rule.
  - 9 integration tests in `tests/round199_text_per_char.rs`.

## [0.1.6](https://github.com/OxideAV/oxideav-svg/compare/v0.1.5...v0.1.6) - 2026-06-02

### Added

- round 209 — SVG 2 §8.13 vector-effect property + non-inherited cascade
- round 205 — SVG 2 §13.8 paint-order property + scene-graph operation-order split
- round 199 — SVG 2 §11.2 / §11.2.2 list-of-values on <text>/<tspan> x/y/dx/dy/rotate

## [0.1.5](https://github.com/OxideAV/oxideav-svg/compare/v0.1.4...v0.1.5) - 2026-05-29

### Added

- round 187 — SVG 2 §11.2.1 textLength + lengthAdjust on <text> / <tspan>
- round 176 — SVG 2 §11.5 anchored-chunk boundaries on <tspan x=…>
- round 172 — SVG 2 §11.10.1.1 text-anchor property
- round 128 — SVG 2 §11.8 <textPath> text-on-path layout
- round 125 — SVG 1.1 §19.2.14 <animateMotion> snapshot evaluator
- round 118 — SVG 1.1 §11.5 display + visibility properties

### Other

- round 122: SVG 2 §5.8 <title> / <desc> + §5.9 <metadata> capture

### Added

- **Round 187** — SVG 2 §11.2.1 `textLength` + `lengthAdjust` on
  `<text>` / `<tspan>` elements.
  - New [`crate::element::TextLengthAdjust`] enum (`Spacing` /
    `SpacingAndGlyphs`; initial `Spacing` per the spec's attribute
    table). The attribute is NOT inherited; it applies only to the
    chunk-opening element that carries it (root `<text>` or a
    `<tspan x|y …>` that opens its own §11.5 chunk). A per-`<tspan>`
    `textLength` on an element that does not open a chunk folds onto
    the currently-open chunk's binding so a descendant's target still
    drives the rescale per §11.2.1's ancestor/descendant rule.
  - New `apply_text_length_rescaling` pass in `crate::text` rewrites
    every placement Group's `transform.e` so the chunk extent
    (`rightmost − leftmost glyph origin`) matches the requested
    target in user units. The pass runs **before** the round-172
    §11.10.1.1 `text-anchor` shift so the anchor measures against
    the adjusted width — a `<text x="400" textLength="300"
    text-anchor="middle">` shifts by `−150` rather than by half of
    the natural-glyph extent.
  - `lengthAdjust="spacingAndGlyphs"` additionally post-composes
    `scale(s, 1)` onto each placement transform (where
    `s = target / natural_chunk_extent`) so the glyph outlines
    stretch / compress along the inline-base direction in addition
    to the inter-glyph rescaling.
  - `<textPath>` chunks are excluded from the rescaling pass — the
    existing `textpath_indices` set skips them, leaving the §11.8.3
    path-distance bias untouched. A new chunk opened immediately
    after a `<textPath>` carries no `textLength` binding (matching
    §11.2.1's "applies only when the wrapping area is not defined by
    shape-inside").
  - Spec-compliance edge cases: a non-finite or negative
    `textLength` value is rejected (`parse_text_length` returns
    `None`), and the run shapes at its natural width per §11.2.1's
    "A negative value is an error" sentence. Unknown
    `lengthAdjust` keywords fall back to `spacing`.
  - Five new tests in `tests/round187_text_length.rs` verify: the
    `spacing`-default chunk extent matches the requested target; the
    `spacingAndGlyphs` mode sets `a ≈ target / natural` on every
    placement; `text-anchor="middle"` + `textLength="300"` places
    the leftmost glyph at `origin − 150`; a per-`<tspan x= textLength>`
    rescales only its own chunk (the sibling chunk stays at the
    natural width); a negative `textLength` is silently dropped.

- **Round 176** — SVG 2 §11.5 anchored-chunk boundaries on
  `<tspan x=…>` / `<tspan y=…>`.
  - The text walker now splits a `<text>` element's run at every
    absolute positioning adjustment on a `<tspan>` (an explicit `x=`
    or `y=` attribute), per §11.5's "an absolute positioning
    adjustment opens a new anchored chunk". Each chunk records its
    own `[start_index, end_index)` over the parent Group's children,
    the pen-x at chunk-open, the pen-x at chunk-close, and the
    `text-anchor` inherited at the chunk-opening element.
  - The §11.10.1.1 shift (`0` / `-extent/2` / `-extent` for
    `start` / `middle` / `end`) is now applied **per chunk** rather
    than once across the whole element. Round 172's
    `apply_chunk_anchor_shifts` pass walks every recorded chunk and
    rewrites glyph placements within its index range, skipping the
    parallel `<textPath>` index set (whose §11.8.3 bias has already
    been applied inline by `emit_text_path`).
  - A `<tspan>` may carry its own `text-anchor=` keyword (case-
    insensitive, with `inherit` / unknowns keeping the parent's
    value); the new chunk it opens uses that override rather than
    inheriting the root `<text>`'s anchor.
  - Relative pen nudges (`dx=` / `dy=` on a `<tspan>`) explicitly
    do **not** open a chunk — both pieces stay in the same anchored
    chunk and a single §11.10.1.1 shift covers the whole run.
  - A `<textPath>` element closes the surrounding chunk before its
    first glyph and opens a fresh chunk for any sibling content that
    follows (§11.8 "an embedded textPath always creates an anchored-
    chunk boundary"). The textPath's own glyphs remain in the skip
    set so the outer per-chunk pass does not double-bias them.
  - Five new tests in `tests/round176_text_chunk.rs` verify: two
    `<tspan x=…>` form two independent end-anchored chunks ~300 px
    apart; the per-chunk layout matches the equivalent two-`<text>`
    decomposition; `dx`-only `<tspan>` stays in a single chunk;
    `<tspan>` `text-anchor=` overrides are honoured per chunk; and
    three chunks shift independently with no accumulation.

- **Round 172** — SVG 2 §11.10.1.1 `text-anchor` property
  (`start | middle | end`).
  - New [`crate::element::TextAnchor`] enum (initial `Start` per the
    spec's Initial table). The property is inherited via the round-118-
    style cascade onto [`crate::element::PaintState::text_anchor`];
    presentation attributes, inline `style=` declarations, and
    `<style>`-block tag / class / id rules all resolve through the
    same `apply_one` branch. Case-insensitive keyword matching;
    `inherit` and unrecognised tokens keep the inherited value rather
    than failing the document.
  - **`<text>` chunk shift** — after the text walker emits glyphs the
    chunk's pre-anchor x extent (`pen.x − x`) is multiplied by the
    `0` / `−0.5` / `−1` shift factor for `start` / `middle` / `end`
    and applied as a pure x-translate to every glyph's placement
    Group. Round 172 ships one chunk per `<text>` (multi-chunk
    splitting on author-supplied `<tspan x=…>` boundaries per §11.5
    is a later round's work).
  - **`<textPath>` start-point bias per §11.8.3** — the same
    `0` / `−W/2` / `−W` term (where `W` is the total of every shaped
    glyph's `x_advance`, whitespace included) folds directly into
    `startOffset` before glyphs are laid along the curve. The §11.8.3
    rule is "subtract half the total advance values for all of the
    glyphs … from the start of the path" for `middle` and the full
    total for `end`.
  - **`<textPath>` children inside a `<text>` opt out of the outer
    chunk shift** — the walker records each `<textPath>`'s emitted-
    glyph indices in a parallel skip-set so the outer post-walk shift
    leaves them alone; the `<textPath>`'s own §11.8.3 bias is applied
    inline by `emit_text_path`.
  - **Without a font resolver** the shift collapses to a no-op (zero
    glyphs in the chunk) and the document still loads, matching the
    round-2 baseline `<text>` behaviour.
  - 18 integration tests across `tests/round172_text_anchor.rs` (11
    parser-side: default value, three keyword variants via
    presentation attribute, `inherit` + unrecognised-keyword
    tolerance, case-insensitive matching, no-crash without resolver,
    `<g>`-cascade inheritance, `style=` resolution, `<style>`-block
    rule resolution), `tests/round172_text_anchor_glyphs.rs` (5 glyph-
    placement: leftmost-glyph x shifts by `0` / `−W/2` / `−W`,
    default-vs-explicit-start parity, `end` moves leftwards, `<g>`-
    inherited middle matches inline middle, empty runs emit nothing
    for every anchor), and `tests/round172_text_path_anchor.rs` (2
    `<textPath>` cases: §11.8.3 bias along a horizontal path,
    default-vs-explicit-start parity).

- **Round 128** — SVG 2 §11.8 `<textPath>` text-on-path layout.
  - `<text>` `<textPath>` children now lay their text run along a
    referenced path instead of the parent `<text>`'s baseline. The
    glyph midpoint of each typographic character is moved to the
    corresponding point on the path per §11.8 and rotated by the path
    tangent at that point.
  - **Path-resolution precedence** per §11.8.1: `path=` (inline
    `d`-mini-language data) overrides `href` (SVG-2 canonical)
    overrides `xlink:href` (deprecated SVG-1.1 fallback). The
    referenced `<path>` resolves through the pre-walked
    [`crate::defs::DefsTables::elements`] id table.
  - **`startOffset`** (§11.8.2): both `<number>` (user units along
    the path) and `<percentage>` (of total path length) shapes are
    accepted; negative values and offsets > 100% are honoured per
    spec (glyphs whose midpoint lands off the path are silently
    dropped by the placement rule).
  - **`side="right"`** (§11.8.2): flips the path-distance about the
    total length so the text runs along the opposite side (matches
    the spec's "right" side semantics for monotonic paths).
  - **Arc-length aware placement** — new public
    [`crate::path_length::sample_path_at_distance`] sampler walks
    line / quadratic-Bézier / cubic-Bézier / elliptic-arc / close
    segments, returning `(point, tangent_degrees)` for an absolute
    path-distance query. Sampling cadence matches
    [`compute_path_length`] (32 chord steps per Bézier, 64 per arc)
    so cumulative distance and the running advance agree.
  - **No font resolver → empty group** — keeps the round-2 baseline
    `<text>` behaviour: a `<textPath>` whose font-family cannot be
    resolved by the installed
    [`crate::text::set_font_resolver`] hook parses to an empty
    Group so the rest of the document still loads.
  - 21 integration tests in `tests/round128_text_path.rs` +
    `tests/round128_text_path_glyphs.rs` (path-resolution decision
    tree, off-path glyph drop, `startOffset` shift, horizontal /
    vertical / curved tangent rotation, sampler unit tests on
    straight / polyline / cubic / arc / multi-subpath geometry,
    round-trip safety).

- **Round 125** — SVG 1.1 §19.2.14 `<animateMotion>` snapshot evaluator
  with `<mpath>` resolution + `rotate="auto"` / `auto-reverse` /
  numeric + `keyPoints` / `keyTimes` remapping.
  - The animation now folds a supplemental `translate(x,y) rotate(angle)`
    matrix into the parent element's `transform=` attribute set per
    §19.2.14, matching the spec's "supplemental transformation matrix
    onto the CTM" rule. Earlier rounds captured `<animateMotion>`
    verbatim for round-trip preservation but its scene-graph
    contribution was silently dropped at snapshot time.
  - **Motion-path resolution precedence** (§19.2.14): `<mpath>`
    overrides `path=` overrides `values=` overrides `from`/`by`/`to`.
    `<mpath xlink:href="#id">` and the SVG-2 bare-`href` form both
    resolve the referenced `<path>` via the pre-walked
    [`crate::defs::DefsTables::elements`] id table.
  - **Tangent-aware rotation**: `rotate="auto"` reads the path
    tangent at the sampled position; `auto-reverse` adds 180°;
    a numeric value holds constant; the default `0` emits no
    `rotate` term (keeping the common-case output as a plain
    `translate(...)`).
  - **`calcMode` defaults to `paced`** per the §19.2.14 difference
    from the rest of the SMIL animation family. Arc-length sampling
    uses 32-chord flattening for cubics / quadratics and 64-chord
    flattening for elliptic arcs (matching the
    [`crate::path_length`] density so the running accumulator and
    the total arc length agree).
  - **`keyPoints` + `keyTimes` override** the natural arc-length
    fraction mapping per §19.2.14: the (keyTimes, keyPoints) pair
    remaps document time to path-distance.
  - **Public API**: new
    [`crate::animation::evaluate_motion_at(el, t, id_lookup)`] +
    [`crate::animation::snapshot_children_with_resolver(parent, t, id_lookup)`].
    The legacy `snapshot_children(parent, t)` keeps working but
    resolves `<mpath>` references only when the caller threads an
    id-lookup closure through. The decoder routes through the
    resolver variant with `&ctx.defs.elements`.
  - **Round-trip preservation** continues via
    [`PreservedExtras::animations`] (the existing animation capture
    path already covered `<animateMotion>` verbatim — round 125 just
    starts honouring it in scene-graph evaluation, not only in the
    round-trip output).
  - 26 integration tests in `tests/round125_animate_motion.rs`
    (straight-line / cubic / arc paths, `<mpath>` via both
    `xlink:href` and SVG-2 `href`, all four `rotate` modes,
    `repeatCount="indefinite"`, `begin` delay, `fill="freeze"`
    end-of-anim hold, `keyPoints`/`keyTimes` remapping, malformed
    input recovery, override precedence, round-trip preservation).

- **Round 122** — SVG 2 §5.8 `<title>` / `<desc>` and §5.9 `<metadata>`
  descriptive-element capture + round-trip preservation.
  - **`<title>` / `<desc>`** are *never-rendered* elements per the §5.8
    dfn block (UA stylesheet forces `display:none` with importance over
    any other CSS rule); they MUST NOT contribute scene-graph nodes.
    Round 122 captures each occurrence into a new typed
    `crate::preserved::DescriptiveText { text, lang }`, keyed by the
    **parent** container's scene-graph tree-path on the new
    `PreservedExtras::titles` / `PreservedExtras::descs:
    Vec<DescriptiveBinding>` side-channels. Multiple sibling `<title>`s
    under the same parent (the §5.8 multilingual-alternative pattern)
    append to the same binding's `items` list in document order so the
    consumer can run the §5.8 best-language selection algorithm itself.
  - **`lang` / `xml:lang` capture** — SVG-2 `lang` is the canonical
    form; round 122 falls back to the deprecated `xml:lang` only when
    `lang` is absent, matching the round-trip-preserving convention
    used by other side-channels in this crate.
  - **Encoder** — `write_svg_with_extras` re-emits captured titles +
    descs as the **first children** of the matching `<g>` (or, for the
    root-`<svg>` empty path, at the top of the output) so an SVG 1.1
    reader that "may not recognize a title element that is not the
    first child of its parent" still picks them up. `<title>` precedes
    `<desc>` per the §5.8 example.
  - **`<metadata>`** (§5.9) — opaque foreign-namespace XML body
    (typically RDF / Dublin Core / Inkscape extensions); captured
    verbatim on `PreservedExtras::metadata: Vec<Element>` and re-emitted
    at the trailing edge of the document on round-trip. Like `<title>`
    / `<desc>` it carries the UA `display:none` rule, so it never
    enters the rendering tree.
  - 15 integration tests in `tests/round122_descriptive.rs`.

- **Round 118** — SVG 1.1 §11.5 `display` + `visibility` presentation
  properties.
  - `display: none` removes the element **and its children** from the
    rendering tree (no scene-graph node at all). `display` is NOT
    inherited (§11.5, Inherited: no), so the cascade resets it to the
    initial `inline` before applying each element's own value. Resolved
    via both presentation attributes and the CSS cascade. Applies to
    `<svg>`, `<g>`, `<switch>`, `<a>`, `<foreignObject>`, `<use>`, and
    the graphics elements (`<rect>`, `<circle>`, `<ellipse>`, `<line>`,
    `<polyline>`, `<polygon>`, `<path>`, `<text>`) — never-rendered
    elements (`<defs>`, gradients, `<marker>`, `<symbol>`, `<mask>`,
    `<clipPath>`, `<style>`, animation) are excluded per the spec.
  - `display: none` does **not** prevent referencing: a `<use>` of a
    `display:none` definition still renders the instance (§11.5: "the
    path element can still be referenced"). A new
    `ParseContext::use_instance_root_pending` flag exempts the
    instance *root* from the drop while a nested `display:none`
    *descendant* inside the instantiated subtree still drops.
  - `visibility: hidden | collapse` keeps the element in the rendering
    tree — its geometry still contributes to bounding-box / clipping
    calculations (§11.5) — but paints nothing (fill + stroke dropped).
    `visibility` IS inherited, so a `<g visibility="hidden">` makes its
    children invisible while a descendant may flip back to
    `visibility="visible"`. Text glyphs honour the same suppression via
    `PaintState::solid_fill_public`.
  - New `crate::element::Visibility` enum + `PaintState::display` /
    `PaintState::visibility` fields. 13 integration tests in
    `tests/round118_display_visibility.rs`.

## [0.1.4](https://github.com/OxideAV/oxideav-svg/compare/v0.1.3...v0.1.4) - 2026-05-24

### Added

- round 115 — SVG 2 §16.5 <a> hyperlink element
- round 104 — SVG 2 §13.7.1 <marker> definition capture
- round 98 — SVG 2 §5.7 <switch> conditional processing
- round 95 — SVG 2 §16.3 <view> element + fragment-identifier routing
- round 21 — SVG 2 §9.6.1 pathLength attribute on every SVGGeometryElement
- round 81 — SVG 2 §14.1.1 gradient href template inheritance + gradientUnits/gradientTransform capture

### Other

- Round 75: pattern fill + color/defs/element extensions

### Added

- **Round 115** — SVG 2 §16.5 `<a>` hyperlink element.
  - `<a>` is categorised as both a *container element* and a
    *renderable element*. The decoder now renders its children into an
    `oxideav_core::Node::Group` exactly like `<g>` (honouring
    `transform` per §8.5, `opacity`, the paint cascade, and the
    per-element `em` / `rem` resolution context) instead of dropping
    the whole subtree. A shape wrapped in `<a href="…">` is therefore
    painted rather than silently invisible.
  - New [`crate::preserved::LinkBinding`] carries the SVG 2 §16.5 link
    target + HTML companion attributes: `href` (SVG-2 `href` with
    SVG-1.1 `xlink:href` fallback; `href` wins when both present),
    `target`, `download`, `ping`, `rel`, `hreflang`, `type`,
    `referrerpolicy`. Keyed by the group's scene-graph tree-path,
    mirroring the round-13 `id_paths` / round-21 `path_lengths`
    side-channels. Exposed via the new
    [`PreservedExtras::links: Vec<LinkBinding>`] field +
    [`ParseContext::record_link`].
  - `parse_svg_with_extras` populates the table;
    `write_svg_with_extras` re-wraps the matching `<g>` in its
    `<a href="…">…</a>` element so a `parse_svg_with_extras →
    write_svg_with_extras` round-trip preserves the hyperlink + every
    captured attribute. A bare `<a>` (no `href`) still groups its
    children and round-trips as `<a>`. New `write_link_attrs` encoder
    helper.
  - 12 integration tests in `tests/round115_anchor.rs` (child renders,
    group-node shape, `transform` / `opacity` on the group, link-binding
    capture, `xlink:href` fallback + `href`-precedence, full attribute
    round-trip, nested `<a>` inside `<g>` tree-path targeting, bare-`<a>`
    grouping, multi-child grouping).

- **Round 104** — SVG 2 §13.7.1 `<marker>` definition capture.
  - New typed [`crate::defs::MarkerDef`] carrying every §13.7.1
    presentation attribute: `refX` / `refY` (with the SVG-2 geometric
    keywords `left` / `center` / `right` and `top` / `center` /
    `bottom` pre-resolved against the `viewBox` per the §13.7.1 mapping
    table), `markerWidth` / `markerHeight` (default 3), `markerUnits`
    (default `strokeWidth`), `orient` (default `0`), `viewBox`,
    `preserveAspectRatio`, plus the parsed tile content as a `Group`.
    Captured into `DefsTables::markers: HashMap<String, MarkerDef>`
    during the pre-walk so a forward `marker-end="url(#arrow)"`
    reference resolves regardless of source order.
  - New [`crate::defs::MarkerUnits`] enum (`StrokeWidth` /
    `UserSpaceOnUse`; default `StrokeWidth`) and
    [`crate::defs::MarkerOrient`] enum (`Auto` / `AutoStartReverse` /
    `Angle(f32)`; default `Angle(0.0)`). `MarkerOrient::parse` accepts
    the two keywords plus an `<angle>` (with the CSS `deg` / `grad` /
    `rad` / `turn` units) or a bare `<number>` interpreted as degrees.
  - `<marker>` is a never-rendered element per §13.7.1 — the scene-walk
    skips it (contributes no scene-graph node), exactly like
    `<filter>` / `<mask>` / `<clipPath>` / `<symbol>`.
  - [`PreservedExtras::markers: Vec<Element>`] — verbatim source XML of
    every `<marker>` element. The encoder re-emits each in the
    `<defs>` block (alongside `<pattern>` / `<filter>` extras) so a
    `parse_svg_with_extras → write_svg_with_extras` round-trip
    preserves the marker definition byte-faithfully.
  - SVG 2 §13.2 `context-fill` / `context-stroke` `<paint>` keywords
    (used by the spec's own `<marker>` examples to match marker colour
    to the referencing element's stroke) are now accepted by
    `parse_paint`. The static scene graph has no context element, so
    they map to no paint per the spec rule "If there is no context
    element and these keywords are used, then no paint is applied" —
    instead of failing the document.
  - `oxideav_core::Node` has no `Marker` construct, so vertex
    placement + `orient` rotation + `markerUnits` scaling (§13.7.4)
    and the per-shape `marker-start` / `marker-mid` / `marker-end` /
    `marker` property binding remain a followup for once a `Marker`
    node lands in core; round 104 delivers the typed definition + the
    lossless round-trip.
  - 10 integration tests in `tests/round104_marker.rs` (spec defaults,
    explicit attributes, `markerUnits` / `orient` parsing, `refX` /
    `refY` keyword resolution against the viewBox + fallback without a
    viewBox, marker-with-no-id skip, never-rendered invariant,
    verbatim round-trip through `PreservedExtras`) plus 4 unit tests in
    `crate::defs::tests` (enum defaults, keyword + angle-unit parsing,
    round-trip serialisation).

- **Round 95** — SVG 2 §16.3 `<view>` element + fragment-identifier
  routing.
  - New [`crate::defs::ViewDef`] capturing the three typed `<view>`
    attributes per §16.3.3 (`viewBox`, `preserveAspectRatio`,
    `zoomAndPan`). Stored on the new
    [`crate::defs::DefsTables::views: HashMap<String, ViewDef>`]
    table during the pre-walk so a forward / nested reference resolves
    regardless of source order.
  - New [`crate::defs::ZoomAndPan`] enum (`Disable` / `Magnify`;
    default `Magnify`) — SVG 2 §16.3.3 keyword.
  - New [`crate::resolve_fragment(&frame, &extras, fragment)`] top-level
    API + [`crate::ResolvedView`] typed return per §16.3.2. Honours
    both fragment shapes:
    - **Bare-name** (`MyDrawing.svg#MyView`) — addresses an
      id-bearing `<view>`; any attribute the view specified
      overrides the corresponding root `<svg>` attribute, anything
      the view left out inherits from the root.
    - **`svgView(...)` spec** (`MyDrawing.svg#svgView(viewBox(0,200,1000,1000);preserveAspectRatio(xMidYMid))`)
      — semicolon-separated `viewBox(...)` / `preserveAspectRatio(...)`
      / `transform(...)` / `zoomAndPan(...)` in any order, each at
      most once. `%3B` (percent-encoded semicolon, per CSSOM
      escaping) tolerated. Malformed payloads drop silently.
    - Empty fragment / spatial (`xywh=`) / temporal (`t=`) /
      track / id media-fragments degrade to the document root's
      baseline view per §16.3.2 ("if the SVG fragment identifier
      addresses a time segment ... as if no fragment identifier was
      provided").
  - New [`PreservedExtras::views: Vec<Element>`] (verbatim XML for
    round-trip) + [`PreservedExtras::typed_views: HashMap<String, ViewDef>`]
    (typed mirror for fragment resolution). Encoder re-emits each
    captured `<view>` at the trailing edge of the output so a
    `parse_svg_with_extras → write_svg_with_extras → parse_svg_with_extras`
    cycle preserves every view definition + lookup.
  - The `<view>` element itself contributes no scene-graph node
    (it's pure metadata per §16.3.3) — its only effect is making
    the typed mirror available to [`resolve_fragment`].
  - 12 module-level unit tests + 10 integration tests covering
    every §16.3.2 input shape (bare-name, full `svgView` spec,
    multi-attribute order independence, percent-encoded
    semicolons, malformed payloads, unknown attributes,
    inheritance from root, empty fragment, unknown bare name,
    nested `<view>` discovery, round-trip).

- **Round 21** — SVG 2 §9.6.1 `pathLength` attribute on every
  `SVGGeometryElement`.
  - New [`crate::path_length`] module: parser (rejects negative,
    accepts a longest-prefix-parses pattern for tolerant unit
    suffixes), [`compute_path_length`] (line / quadratic / cubic /
    elliptic-arc geometric length via chord sums + centre
    parameterisation per SVG 1.1 §F.6.5), and
    [`apply_to_stroke`] (rewrites `stroke-dasharray` /
    `stroke-dashoffset` by `geometric_length / pathLength` per §9.6.1).
  - Honours the §9.6.1 special cases: `pathLength=0` collapses a
    non-zero dasharray to a solid stroke (infinity scaling); an
    all-zero dasharray survives ("zero scaled infinitely must remain
    zero"); a negative or non-numeric value is silently ignored.
  - New [`PreservedExtras::path_lengths: Vec<PathLengthBinding>`]
    side-channel records the author's original value keyed by
    scene-graph tree-path; [`encoder::write_svg_with_extras`] re-emits
    `pathLength="..."` on the matching `<path>` / `<rect>` /
    `<circle>` / `<ellipse>` / `<line>` / `<polyline>` / `<polygon>`
    on round-trip.
  - 10 integration tests + 12 module-level unit tests covering all
    seven geometry elements and the §9.6.1 edge cases.

- **Round 81** — SVG 2 §14.1.1 gradient `href` template inheritance +
  §14.2.2.1 / §14.2.3.1 `gradientUnits` / `gradientTransform` /
  `spreadMethod` typed capture.
  - New typed [`crate::defs::GradientDef`] carrying every spec
    attribute on `<linearGradient>` / `<radialGradient>` as
    `Option<_>` so [`crate::defs::resolve_gradient_chain`] can tell
    "attribute not specified, inherit from template" from
    "specified-with-explicit-value." Geometry kind discriminator
    (`Linear` / `Radial`) — per-kind attributes are
    `x1`/`y1`/`x2`/`y2` for linear and `cx`/`cy`/`r`/`fx`/`fy`/`fr`
    for radial (including the SVG-2 `fr` focal-circle radius). Shared
    `units` / `transform` / `spread` / `stops` / `href` on the parent
    struct.
  - New [`crate::defs::GradientUnits`] enum (`UserSpaceOnUse` /
    `ObjectBoundingBox`; default `ObjectBoundingBox` per §14.2.2.1
    / §14.2.3.1).
  - New [`crate::defs::ResolvedGradient`] / [`ResolvedGradientKind`]
    — the output of `resolve_gradient_chain`: every attribute pinned
    to a concrete value, stops populated. Spec defaults populated
    when the whole chain leaves an attribute unspecified (linear:
    `x1=0`, `y1=0`, `x2=1`, `y2=0`; radial: `cx=cy=0.5`, `r=0.5`,
    `fx=cx`, `fy=cy`, `fr=0`).
  - `<linearGradient>` / `<radialGradient>` honour both SVG-2 `href`
    and SVG-1.1 `xlink:href`; child-specified attributes win over the
    template per §14.1.1.
  - Cycle / depth-cap guard: chain walker terminates at
    `GRADIENT_HREF_DEPTH_CAP = 8` hops or on a self-reference,
    matching the round-13 CSS `@import` cap.
  - [`crate::element::flatten_gradient_to_paint`] folds the resolved
    chain into a legacy [`oxideav_core::Paint::LinearGradient`] /
    `RadialGradient`, with `gradientTransform` applied to the start /
    end / centre / focal points; the radius is scaled by the
    geometric mean of the matrix's per-axis scale (a uniform-scale
    `gradientTransform` is bit-exact; non-uniform scale / shear keeps
    full fidelity in the typed `ResolvedGradient` on
    `DefsTables::gradients` for a renderer that wants it).
  - [`PreservedExtras::gradients: Vec<Element>`] — verbatim source
    XML of every `<linearGradient>` / `<radialGradient>` element. The
    encoder re-emits each verbatim in the `<defs>` block and skips
    the scene-walk's flattened emission for any id the side-channel
    already carried, so `parse_svg_with_extras → write_svg_with_extras`
    preserves `gradientUnits` / `gradientTransform` / `href` /
    `xlink:href` byte-faithfully without duplicating definitions.
  - 9 new integration tests in `tests/round81_gradient_template.rs`
    (linear template chain copies coords + stops, `xlink:href`
    deprecated form resolves the same way, radial template chain
    copies `cx`/`cy`/`r`/`fx`/`fy`, child-specified attribute
    overrides template, self-reference is broken with spec defaults,
    `gradientTransform` is folded into the flattened paint, typed
    def records units / transform / href + spread, round-trip
    preserves the template chain verbatim, explicit
    `gradientUnits="userSpaceOnUse"` passes through the resolver
    intact). Plus 6 unit tests in `crate::defs::tests` covering the
    chain walker (no chain → spec defaults, single-hop inheritance,
    child-wins precedence, cycle termination, radial defaults, radial
    chain inheritance with kind preservation).

- **Round 20** — `<pattern>` paint-server capture (SVG 2 §14.3) + SVG 2
  §13.2 paint-list fallback grammar
  (`<paint> = url(#id) [none | <color>]?`).
  - New typed [`crate::defs::PatternDef`] carrying every spec attribute
    on `<pattern>`: `x` / `y` / `width` / `height` (parsed as numbers
    in the units indicated by `patternUnits`), `patternUnits` /
    `patternContentUnits` (`UserSpaceOnUse` / `ObjectBoundingBox`,
    defaults per §14.3.1), `patternTransform` (`Transform2D`),
    `viewBox`, `preserveAspectRatio`, `href` (template reference;
    SVG-2 `href` and SVG-1.1 `xlink:href` both honoured), and the
    parsed tile content as a `Group`. Captured into
    `DefsTables::patterns: HashMap<String, PatternDef>` during the
    pre-walk so forward references resolve.
  - New [`crate::defs::PatternUnits`] enum (`UserSpaceOnUse` /
    `ObjectBoundingBox`).
  - [`PreservedExtras::patterns: Vec<Element>`] — verbatim source XML
    of every `<pattern>` element. The encoder re-emits each in the
    `<defs>` block (alongside `<filter>` extras) so a `parse → write`
    round-trip preserves the paint-server definition byte-faithfully.
  - [`PaintValue::Reference`] widened to a struct variant carrying an
    optional `fallback: Option<Option<Rgba>>` per SVG 2 §13.2 —
    `None` = legacy bare `url(...)` (no fallback token),
    `Some(None)` = explicit `none` (suppress paint on resolution
    failure), `Some(Some(rgba))` = explicit `<color>` fallback.
    `PaintValue::reference(id)` constructor preserved as a
    backwards-compat shorthand.
  - [`crate::element::resolve_paint`] now consults both the gradient
    table and the pattern table; a known pattern id resolves to the
    fallback colour today (since `oxideav_core::Paint` has no
    `Pattern` variant yet — once it lands, the pattern branch will
    return the tiled paint directly and the fallback path will become
    a true error case again per the spec).
  - 9 new integration tests in `tests/round20_pattern.rs` (pattern
    with fallback colour renders as the colour, pattern without
    fallback yields no paint, unknown id with fallback resolves to
    the colour, explicit `none` fallback suppresses paint, typed
    `PatternDef` records spec defaults, every attribute survives the
    typed parse, `<pattern>` round-trips through `PreservedExtras`,
    legacy `xlink:href` template reference, missing pattern with no
    fallback doesn't poison the document). Plus 3 new unit tests in
    `crate::color::tests` covering the paint-list grammar (colour
    fallback, `none` fallback, rejection of chained paint servers).


## [0.1.3](https://github.com/OxideAV/oxideav-svg/compare/v0.1.2...v0.1.3) - 2026-05-09

### Added

- round 19 — thread ResolveContext through element.rs / decoder.rs
- round 18 — CSS Values L4 length units + CSS Easing L2 linear()
- round 17 — CSS @supports + animation long-tail (timing/direction/fill-mode/multi-name)
- round 16 — CSS @media + @keyframes-at-t evaluation
- round 15 — <image> capture + CSS @keyframes capture
- round 14 — <symbol> + <use> viewport mapping + CSS @font-face capture
- round 13 — animation re-attachment to source emit site + Stylesheet::resolve_imports
- round 12 — <script> graceful capture + viewBox/preserveAspectRatio mapping
- round 11 — feImage / feTile + ::before/::after + @import + stateful pseudos
- round 10 — feDiffuseLighting / feSpecularLighting + LightSource
- round 9 — feConvolveMatrix / feTurbulence / feDisplacementMap
- round 8 — long-tail filter primitives (feColorMatrix / feMerge / feComponentTransfer / feDropShadow)
- round 7 — typed <filter> primitive graph + calcMode paced/spline
- round 6 — Selectors L3 leftovers (:nth-last-*, :lang) + SVG 2 d as a CSS property
- round 5 — CSS 3 Selectors Level 3 subset (attrs + combinators + structural pseudos)
- round 4 — SMIL @ arbitrary t + CSS cascade + encoder preservation

### Other

- round 15 — <image> capture + @keyframes capture
- document round 5 (CSS3 Selectors L3) + round 6 sections
- drop stale REGISTRARS / with_all_features intra-doc links
- drop dead `linkme` dep
- drop committed Cargo.lock + relax oxideav-core to "0.1"
- auto-register via oxideav_core::register! macro (linkme distributed slice)
- unify entry point on register(&mut RuntimeContext) ([#502](https://github.com/OxideAV/oxideav-svg/pull/502))

### Added

- **Round 19** — SVG 2 §10 length-resolution wiring through
  `element.rs` / `decoder.rs`. The round-18 typed
  `crate::length::Length` surface now feeds the per-element coordinate
  parsers via the new `crate::length::LengthAxis` enum +
  `ResolveContext::percentage_basis_for(axis)` helper +
  `crate::element::parse_length_attr(v, default, axis, ctx)`. Each
  shape parser (`parse_rect`, `parse_circle`, `parse_ellipse`,
  `parse_line`) now takes `&ResolveContext`; the `<g>` branch in
  `parse_element_to_node_ctx` saves the parent context, derives a
  child context (via the new `crate::element::derive_child_ctx`) that
  picks up the element's `font-size` cascade, recurses, then restores
  the parent context. The decoder seeds the root context from the
  `<svg>` width / height (for `vw`/`vh`/`vmin`/`vmax`) plus the
  spec-default 16 px font-size, then folds any
  `<svg font-size="...">` cascade in (also pinning the root
  font-size as the `rem` basis for every descendant). Bare-numeric
  coordinate values (`<rect x="100">`) round-trip bit-for-bit
  identical to the round-1 path because `Length::resolve` is the
  identity for `LengthUnit::UserUnit`. Per-axis percentage basis per
  SVG 2 §7.10 — `width="50%"` against viewport width, `height="50%"`
  against viewport height, `r="50%"` against the spec "diagonal"
  (`sqrt(w² + h²) / sqrt(2)`).
  - New context field: `ParseContext::resolve_ctx: ResolveContext` +
    builder `ParseContext::with_resolve_ctx(ctx)`.
  - New helpers: `crate::length::LengthAxis::{X, Y, Diagonal}`,
    `ResolveContext::percentage_basis_for(axis)`,
    `crate::element::parse_length_attr`,
    `crate::element::derive_child_ctx`.
  - 8 new integration tests in `tests/round19_length_threading.rs`
    cover root-default em (16 px), `<g>` em-cascade override, sibling
    isolation across the cascade boundary, axis-specific `%` basis
    (X / Y), `vw` / `vh` against the root viewport, root `font-size`
    seeding `rem` independent of nested `<g font-size="…">`,
    bare-numeric round-trip, and inherit-through-intermediate-`<g>`
    em propagation. Three new shape-parser unit tests in `element.rs`
    cover the same surface at the `parse_rect` / `parse_circle`
    level with explicit `ResolveContext` inputs.

- **Round 18** — CSS Values L4 length-unit aware coordinate parsing
  + CSS Easing Functions L2 `linear()` function.
  - **`crate::length` module** (new) — typed `Length { value, unit }`
    + `LengthUnit` enum covering every CSS Values L4 §6 unit
    (`UserUnit`, `Px`, `Em`, `Rem`, `Percent`, `Vw`, `Vh`, `Vmin`,
    `Vmax`, `Pt`, `Cm`, `Mm`, `In`, `Pc`, `Q`). `parse_length(s)`
    recognises every suffix (case-insensitive); `Length::resolve(ctx)`
    returns the px value given a `ResolveContext` (current font-size,
    root font-size, viewport dimensions, percentage basis). Existing
    bare-number coordinates (`<rect x="100">`) parse to
    `LengthUnit::UserUnit` and resolve bit-for-bit identically to the
    legacy `parse_number` path — no fixture round-trip drift.
  - **CSS Easing L2 `linear()`** — `crate::keyframe::TimingFunction`
    gains a `LinearStops { stops: Vec<LinearStop> }` variant. Parses
    `linear(<stop>#)` per L2 §3.1 — each stop is `<number>
    [<percentage>]?{0,2}` with the missing-input fill-in algorithm
    (first stop → 0%, last → max(prev, 100%), middle → linear ramp,
    monotonic-clamp on regressions). `compute_progress(t)` walks the
    sorted stops and lerps the bracketing pair. `animation-timing-
    function` parsing now uses paren-aware comma-splitting so
    `linear(0, 0.5 25%, 1)` survives the CSS cascade unscathed; the
    bare `linear` keyword still maps to the L1 unit-variant identity.

- **Round 17** — CSS `@supports` block parse + evaluation per CSS
  Conditional Rules L3, plus CSS Animations L1 long tail
  (`animation-timing-function`, multi-name `animation-name`,
  `animation-direction`, `animation-fill-mode`).
  - **`@supports (cond) { ... }` blocks** are routed to the new
    `crate::css::SupportsRule { condition, rules }`. The prelude
    parses into `SupportsCondition::{Property, Not, And, Or, Always}`
    — a leaf `(prop: value)` test or a boolean combination thereof.
    New `Stylesheet::resolve_for_supports_context(supported)` walks
    the captured rules against a runtime-supplied
    `HashSet<(String, String)>` of supported (property, value) pairs
    and returns the merged cascade — symmetric to round 16's
    `@media` evaluation.
  - **`animation-timing-function`** — `linear` / `ease` / `ease-in`
    / `ease-out` / `ease-in-out` / `cubic-bezier(x1,y1,x2,y2)` /
    `steps(N, start|end)` per CSS Easing Functions L1 §3 / §4. The
    new `crate::keyframe::TimingFunction` enum carries a
    `compute_progress(t) -> f32` solver: cubic-bezier solves the
    parametric curve via bisection (sub-1e-5 absolute error in <16
    iterations); steps buckets per L1 §4. Default is `ease` per
    L1 §3.4 (round 16 was effectively `linear`).
  - **multi-name `animation-name`** — `animation-name: a, b, c`
    evaluates each animation independently per L1 §6 with mod-indexed
    pairing on every other longhand list. Later animations override
    earlier ones on shared properties (the L1 §6 cascade).
  - **`animation-direction`** (`normal` / `reverse` / `alternate` /
    `alternate-reverse` per §4.4) flips the per-iteration direction
    on the keyframe timeline.
  - **`animation-fill-mode`** (`none` / `forwards` / `backwards` /
    `both` per §4.7) pins the start or end keyframe outside the
    active interval.

- **Round 16** — CSS `@media` block parse + evaluation per CSS Media
  Queries L4, plus CSS `@keyframes` evaluation at runtime
  `t_seconds` per CSS Animations L1 §3.
  - **`@media (cond) { ... }` blocks** are routed to the new
    `crate::css::MediaRule { condition, rules }`. The prelude parses
    into `MediaCondition` (a list of `MediaQuery`s ORed via
    comma-separated lists; each query carries an optional `not` /
    `only` modifier, an optional media type, and a list of
    `MediaFeature`s ANDed together). Width / height (with `min-` /
    `max-` prefixes per §4) and `orientation: portrait | landscape`
    are honoured; unrecognised features (`prefers-color-scheme`,
    `color-gamut`, etc.) round-trip as `MediaValue::Raw` but never
    match (the rule is dormant). New
    `Stylesheet::resolve_for_media_context(viewport_w, viewport_h,
    orientation)` evaluates each captured query and returns the
    merged cascade in source order so `matched_declarations` still
    resolves specificity / source-order ties correctly.
  - **`@keyframes` evaluation at runtime `t_seconds`** via the new
    `crate::keyframe` module. An element whose CSS cascade resolves
    to `animation-name: <kf>` + `animation-duration: <s>` has the
    bracketing keyframe pair lerped at `t_seconds`, and the
    resulting property values folded into the element's effective
    property map (transform values land in the `transform=`
    attribute slot; everything else lands in `style=`). Honoured
    longhands: `animation-name`, `animation-duration` (`s` / `ms`),
    `animation-iteration-count` (numeric or `infinite`),
    `animation-delay`. Lerp coverage: `transform: rotate | translate
    | scale(...)`, `opacity` / `fill-opacity` / `stroke-opacity` /
    `stroke-width`, colour properties via the shared SMIL
    `lerp_string` path. Wired into `parse_svg_at(t_seconds)` so a
    single `transform: rotate(180deg)` renders correctly at
    `t = 0.5s` of a 1-second `from rotate(0deg) → to
    rotate(360deg)` animation.
  - **Transform parser accepts CSS unit suffixes** per SVG 2 / CSS
    Transforms L1 — `rotate(180deg)` / `rotate(0.5turn)` /
    `translate(10px, 20px)`. Angle units (`deg` / `rad` / `grad` /
    `turn`) convert to canonical degrees; length units (`px` / `em`
    / `%`) parse and are dropped (round 16 still treats every
    length as user units).

- **Round 15** — `<image>` element capture (SVG 2 §6) and CSS
  `@keyframes` rule capture (CSS Animations L1 §3).
  - **`<image>` element capture.** Inline
    `data:image/<mime>;base64,…` URIs are base64-decoded into
    `crate::image::ImageHref::DataUri { mime, bytes }`; external
    `href="logo.png"` (and legacy `xlink:href`) are captured verbatim
    into `crate::image::ImageHref::External(String)` for caller-side
    fetching. The new typed `crate::image::SvgImage` carries
    `(x, y, width, height, transform, id, parent_id,
    preserve_aspect_ratio)`. Each captured image lives on the new
    `PreservedExtras::images: Vec<SvgImage>`; the encoder re-emits
    them at the trailing edge of the document with a faithful
    round-trip (data URIs re-encode from the decoded bytes; external
    URLs are preserved as-is). `oxideav_core::Node::Image` requires a
    fully-decoded `VideoFrame`, so round 15 deliberately keeps the
    raster bytes opaque on the SVG side — the renderer (or a caller
    that owns a PNG / JPEG decoder) decodes them lazily, avoiding a
    fan-out of image-format crate dependencies into oxideav-svg.
  - **CSS `@keyframes` capture.** Round 11 + 14 routed `@import` and
    `@font-face` to dedicated parsers but silently dropped
    `@keyframes`. Round 15 routes
    `@keyframes <name> { sel { ... } sel { ... } }` (and the
    `-webkit-` prefix variant) to a dedicated parser that surfaces
    each rule on the new `Stylesheet::keyframes: Vec<KeyframesRule>`.
    `KeyframesRule` carries the animation name + a list of
    `KeyframeSelector`s (each with an `offset: KeyframeOffset` —
    `From` / `To` / `Percent(f32)` — and the declarations to apply at
    that timeline point). Comma-separated selector lists
    (`0%, 100% { ... }`) expand to one `KeyframeSelector` entry per
    offset so a downstream animation engine can iterate without
    re-parsing.

- **Round 14** — `<symbol>` + `<use>` `viewBox` / `width` / `height`
  resolution and CSS `@font-face` block capture.
  - **Symbol viewport mapping.** `<use href="#sym">` instantiations
    now apply the symbol's `viewBox`, the use's `width` / `height`
    (falling back to the symbol's intrinsic `width` / `height` when
    omitted), and the symbol's `preserveAspectRatio` per SVG 2 §5.5
    + §5.6 + §8.2. The viewport transform is wrapped in an inner
    `Group` between the use's `transform=` / translate / opacity
    and the symbol's children, so all three semantics compose
    cleanly. `SymbolDef` (in `crate::defs`) gains
    `view_box: Option<ViewBox>`,
    `preserve_aspect_ratio: PreserveAspectRatio`,
    `intrinsic_width: Option<f32>`, and `intrinsic_height: Option<f32>`
    fields populated by `parse_symbol_def`. Symbols without a
    `viewBox` skip the viewport wrap (the use's `width` / `height`
    are ignored per spec).
  - **`@font-face` capture.** Round 11 + 13 routed `@import` to
    `Stylesheet::imports` but tagged every other `@-rule` (including
    `@font-face`) for tolerant skip in `parse_block`. Round 14 adds
    a dedicated parser that surfaces the descriptor list on the new
    `Stylesheet::font_faces: Vec<FontFace>`. `FontFace` carries the
    typed `family: String` + `src: Vec<FontSource>` views plus a
    `descriptors: HashMap<String, String>` map for the long tail
    (`font-weight`, `font-style`, `font-stretch`, `unicode-range`,
    `font-display`, …). `FontSource` covers both the `url(...)
    [format(...)]` and `local(...)` shapes per CSS Fonts L3 §4.3.
    A downstream font-resolver consumer can iterate the list and
    register the user-supplied fonts before the cascade matches a
    `font-family: ...` declaration.

- **Round 13** — animation re-attachment to the source emit site +
  `Stylesheet::resolve_imports` caller-fetcher hook for `@import`.
  - **Animation re-attachment.** Round 4–12 captured every
    `<animate>` / `<set>` / `<animateTransform>` into
    `PreservedExtras::animations` keyed by the parent's `id`, then
    re-emitted them at the trailing edge of the SVG with a
    `<!-- animation parent: #id -->` comment hint. Round 13 inlines
    each animation as a child of its declared parent when the
    parent's `id` was tracked into the new
    `PreservedExtras::id_paths` side-channel by
    `parse_svg_with_extras`. Re-emission also surfaces the
    original `id="..."` on the matching `<g>` / `<path>` so
    downstream tooling can still address the element by source
    name. Animations whose parent didn't carry an `id` (or whose
    parent didn't survive the scene-graph build) fall back to the
    round-12 trailing-edge emission with the comment hint — so no
    captured fragment is ever lost.
  - **`PreservedExtras::id_paths` + `IdScenePath`.** New
    `Vec<IdScenePath>` field on the side-channel; each entry maps
    a source `id="..."` to the `Vec<usize>` scene-graph tree-path
    of the corresponding emit site. Populated only by
    `parse_svg_with_extras`; left empty for the back-compat
    `parse_svg` / `parse_svg_at` paths so they don't pay the
    bookkeeping cost.
  - **`Stylesheet::resolve_imports(fetcher)`.** Round 11 captured
    `@import url(…)` URLs into `Stylesheet::imports` but never
    fetched / parsed them. Round 13 adds a recursive resolver:
    the caller supplies a `Fn(&str) -> Option<Vec<u8>>` (lets the
    consumer choose HTTP / FS / cache); each fetched body is
    parsed as CSS and its rules are appended to `self.rules` so
    the cascade applies as if the rules were inline. Cycle
    detection (visited-URL `HashSet`) and a depth cap of 8 hops
    (`Stylesheet::IMPORT_DEPTH_CAP`) keep runaway chains in check.
    Failure modes — fetcher returns `None`, body isn't UTF-8,
    parse produces no rules — log at `debug` and skip silently
    rather than fail the document.
  - **New `log = "0.4"` dependency.** Used only by
    `resolve_imports` to surface skipped imports under the `debug`
    level; the rest of the crate stays silent.

- **Round 12** — `<script>` graceful capture (HTML5-style raw-text
  body) + `viewBox` / `preserveAspectRatio` viewport mapping baked
  into `root.transform`.
  - **`<script>` raw-text parsing.** When the parser opens a
    `<script>` element it reads bytes verbatim until the matching
    `</script>` close tag, ignoring any `<` characters inside the
    body. Real-world SVGs frequently embed unescaped JS like
    `if (a < b)` without CDATA wrapping; round 11 either errored
    out or silently ate the trailing siblings. Round 12 captures
    such bodies cleanly. The decoder NEVER executes scripts.
  - **`PreservedExtras::scripts`** — new `Vec<Element>` field
    capturing each `<script>` verbatim. The encoder re-emits each
    captured `<script>` with a `<![CDATA[…]]>` wrapping so a
    subsequent strict-XML round-trip succeeds without raw-text
    mode being needed. A stray `]]>` in the body is split across
    two CDATA sections defensively.
  - **`viewBox` + `preserveAspectRatio` on the root `<svg>`.** SVG
    2 §8.2 specifies how the canvas-vs-viewBox aspect-ratio
    mismatch maps via the `preserveAspectRatio` align keyword
    (`xMin/Mid/MaxYMin/Mid/Max` × `meet`/`slice`). The decoder
    applies the spec algorithm (steps 5–14 of §8.2), computes the
    equivalent translate+scale, and pre-multiplies it into
    `frame.root.transform` — so a downstream rasteriser that
    knows nothing about `preserveAspectRatio` (one that simply
    stretches viewBox → canvas) still produces the spec-correct
    visual result. `none` (and the aspect-match degenerate case)
    skip the correction — the renderer's stretch IS the spec's
    behaviour for those.
  - **`PreservedExtras::root_preserve_aspect_ratio`** — new
    `Option<String>` holding the original keyword pair verbatim
    (e.g. `"xMinYMid slice"`) so the encoder re-emits the
    attribute on round-trip.
  - **`crate::filter::PreserveAspectRatio::from_str`** /
    `PreserveAspectRatioAlign::from_str` /
    `MeetOrSlice::from_str` — promoted to `pub` so the
    root-viewport mapper in `crate::decoder` can reuse the same
    parser used by `<feImage>`.

- **Round 11** — `<feImage>` + `<feTile>` close the W3C Filter Effects
  §11 short-name set; CSS pseudo-elements parse to typed
  `PseudoElement`; `@import` URL capture per CSS 2.1 §6.3; stateful
  pseudo-classes parse to typed `Stateful` variant.
  - **`<feImage>`** — `crate::filter::FilterPrimitive::Image { href,
    preserve_aspect_ratio, crossorigin }`. Per W3C Filter Effects §21.
    `href` falls back to `xlink:href` for legacy SVG-1.1 documents.
    `preserveAspectRatio` parses the full SVG-2 §8.10 keyword set
    (`xMin/Mid/MaxYMin/Mid/Max` + `none`, with optional `meet`/`slice`
    modifier; default `xMidYMid meet`). `crossorigin` is
    `Option<CrossOrigin>` with the HTML CORS values
    (`anonymous`/`use-credentials`; empty value maps to `anonymous`
    per HTML §2.7). Absent `href` records as `""` (rasterizer treats
    as transparent-black no-op).
  - **`<feTile>`** — `crate::filter::FilterPrimitive::Tile { input }`.
    Per W3C Filter Effects §20. The only attribute is `in`; the
    primitive's region (already on `FilterPrimitiveNode`) drives the
    tiled-fill area.
  - **`crate::filter::PreserveAspectRatio`** + helper enums
    `PreserveAspectRatioAlign` (10 keyword variants + `None`) and
    `MeetOrSlice`.
  - **`crate::filter::CrossOrigin`** — `Anonymous` /
    `UseCredentials`.
  - **`crate::css::PseudoElement`** — `Before` / `After` /
    `FirstLetter` / `FirstLine` (CSS 3 §3.7). Recorded on
    `SimpleSelector::pseudo_element`. CSS 2.1 §5.12.1 single-colon
    legacy syntax (`:before`, `:after`, …) honoured.
    `pseudo_element` adds one tag-level point to specificity per
    CSS3 §9. A rule with a pseudo-element never matches a live
    element (the pseudo-element is a synthesised box; live matching
    is up to a future renderer).
  - **`crate::css::Pseudo::Stateful(StatefulPseudo)`** — wraps the
    eight interactive pseudo-classes recognised by Selectors L3
    §6.6: `Hover` / `Focus` / `Active` / `Checked` / `Visited` /
    `Link` / `Disabled` / `Enabled`. None match in a static document
    — they're preserved on the cascade so a future interactive
    consumer can re-evaluate. Fixes a round-5 over-match bug where
    `.x:hover` collapsed to `.x` because `:hover` was silently
    dropped.
  - **`crate::css::Stylesheet::imports: Vec<String>`** — populated
    from every `@import url(…) [media-query-list];` (CSS 2.1 §6.3).
    Both `@import url("foo.css")` and bare-string
    (`@import "foo.css";`) forms accepted; loading external
    stylesheets is left to the caller (the parser deliberately does
    not fetch network resources). `@media`, `@font-face`,
    `@keyframes` and other block-form @-rules continue to be
    skipped.
  - 9 new integration tests in `tests/round11_filter.rs`, 17 new
    integration tests in `tests/round11_css.rs`, plus 25 new unit
    tests across `crate::filter::tests` and `crate::css::tests`
    (per-primitive defaults, explicit attrs, legacy `xlink:href`,
    `data:` URI preservation, `crossorigin` empty-string mapping,
    pseudo-element specificity, single-colon legacy parsing,
    @import URL forms with quotes / parentheses / media-queries,
    stateful-pseudo never-match, `:not(:hover)` matching all real
    `<a>` because the inner `:hover` rejects).
  - The verbatim-XML round-trip path continues to preserve every
    primitive (including any future tail elements like
    `<feFunctionalNotation>` should they appear) via
    `PreservedExtras`.
  - Round-7 "unknown primitive" tests retargeted from `<feImage>` to
    a deliberately-fake `<feBogusPrimitive>` so the skip-then-
    preserve invariant keeps a stable witness target.
  - Round-5 `unsupported_pseudo_class_doesnt_break_rule` test
    updated to assert the new (correct) static behaviour: `:hover`
    is recorded but never matches, so the rule does not paint.

- **Round 10** — lighting filter primitives. Two more primitives join
  the typed-graph allowlist (now 15 of the W3C Filter Effects §11
  set):
  - **`<feDiffuseLighting>`** — `crate::filter::FilterPrimitive::DiffuseLighting
    { input, surface_scale, diffuse_constant, kernel_unit_length,
    lighting_color, light_source }`. Per W3C Filter Effects §18.
    `surfaceScale` and `diffuseConstant` default to 1; `kernelUnitLength`
    is `Option<(f32, f32)>` (absent → `None`, single number mirrors);
    `lighting-color` defaults to opaque white per §21.
  - **`<feSpecularLighting>`** — same shared shape plus
    `specular_constant` and `specular_exponent` (both default 1) per
    §19.
  - **`crate::filter::LightSource` enum** — shared by both lighting
    primitives. `Distant { azimuth, elevation }` for
    `<feDistantLight>` (§18.5), `Point { x, y, z }` for
    `<fePointLight>` (§18.6) and the eight-attribute
    `Spot { x, y, z, points_at_x, points_at_y, points_at_z,
    specular_exponent, limiting_cone_angle }` for `<feSpotLight>`
    (§18.7). `limiting_cone_angle: Option<f32>` so an absent
    attribute records as "no cone clipping". A missing light-source
    child collapses to a default distant light at azimuth=0,
    elevation=0.
  - 11 new integration tests in `tests/round10_filter.rs` plus 11 new
    unit tests in `crate::filter::tests` (per-primitive defaults,
    explicit attrs, spot-light eight-attribute form, kernel-unit-
    length single-number mirroring, currentColor → opaque-black
    fallback, child precedence when multiple light-source elements
    appear, mixed pipelines round-tripped through
    `parse_svg_with_extras` / `write_svg_with_extras`).
  - The verbatim-XML round-trip path continues to preserve
    `<feImage>` and `<feTile>` (the still-untyped tail) via
    `PreservedExtras`.
  - Round-7 / round-9 "unknown primitive" tests retargeted to
    `<feImage>` (the only short-name primitive still outside the
    allowlist) so they keep witnessing the skip-then-preserve
    invariant.

- **Round 9** — three more long-tail filter primitives. Round 8
  covered four (`feColorMatrix` / `feMerge` / `feComponentTransfer` /
  `feDropShadow`); round 9 extends typed parsing to:
  - **`<feConvolveMatrix>`** — `crate::filter::FilterPrimitive::ConvolveMatrix
    { input, order_x, order_y, kernel_matrix, divisor, bias,
    target_x, target_y, edge_mode, preserve_alpha }`. Per W3C Filter
    Effects §15: `divisor` defaults to the sum of `kernelMatrix` (or
    1 if that sum is zero); `targetX` / `targetY` default to
    `floor(order/2)`; `edgeMode` defaults to `duplicate` via the new
    `ConvolveEdgeMode` enum (`Duplicate` / `Wrap` / `None`);
    `preserveAlpha` defaults to `false`.
  - **`<feTurbulence>`** — Perlin-noise primitive. New
    `crate::filter::TurbulenceKind` enum (`Turbulence` /
    `FractalNoise`, default `Turbulence`). `base_frequency_x` and
    `base_frequency_y` mirror when only one number is supplied (per
    §16.3); `num_octaves` defaults to 1; `seed` defaults to 0;
    `stitch_tiles` flips on `stitchTiles="stitch"` (default off, per
    §16).
  - **`<feDisplacementMap>`** — new `crate::filter::ChannelSelector`
    enum (`R` / `G` / `B` / `A`, default `A` per Filter Effects §17).
    `in2` defaults to `SourceGraphic`; `scale` defaults to 0.
  - The typed-graph allowlist is now thirteen primitives;
    `<feDiffuseLighting>` / `<feSpecularLighting>` / `<feImage>` /
    `<feTile>` still flow through the verbatim-XML round-trip path.
  - 13 new integration tests in `tests/round9_filter.rs` plus 12 new
    unit tests in `crate::filter::tests` (default-divisor = kernel
    sum, zero-sum kernel falls back to 1, non-square `order="5 3"`,
    fractal noise + stitch, channel-selector defaults, …).

- **Round 8** — long-tail filter primitives. Round 7 covered six
  primitives (`feGaussianBlur` / `feOffset` / `feFlood` /
  `feComposite` / `feBlend` / `feMorphology`); round 8 extends typed
  parsing to:
  - **`<feColorMatrix>`** — `type="matrix"` (4×5 row-major) plus
    `"saturate"`, `"hueRotate"`, `"luminanceToAlpha"`. Each non-matrix
    variant reduces at parse time to its 4×5 equivalent using the
    coefficients given in W3C Filter Effects §13.2.4 / §13.2.5 /
    §13.2.6. Malformed / wrong-length `values=` falls back to the
    identity matrix.
  - **`<feMerge>`** — `Merge { inputs: Vec<FilterInput> }`,
    populated from the source-order list of `<feMergeNode in="..."/>`
    children. Missing `in=` falls back to the previous primitive's
    `result` per §6.2 / §19.
  - **`<feComponentTransfer>`** — captures the four
    `<feFuncR/G/B/A>` children into a new
    `filter::TransferFunction` enum with five variants
    (`Identity` / `Table { values }` / `Discrete { values }` /
    `Linear { slope, intercept }` / `Gamma { amplitude, exponent, offset }`).
    Channels lacking a matching `<feFunc*>` child default to
    `Identity` per §12.
  - **`<feDropShadow>`** — single composite primitive (the syntactic
    sugar for `Gaussian blur + Offset + Flood + Composite` per §22).
    Defaults `dx=dy=2`, `stdDeviation=2 2`, `flood-color` opaque
    black, `flood-opacity=1`.
  - The typed-graph allowlist is now ten primitives; remaining
    `<feConvolveMatrix>`, `<feTurbulence>`, lighting, displacement,
    `<feImage>`, `<feTile>` still flow through the verbatim-XML
    round-trip path.
  - 14 new integration tests in `tests/round8_filter.rs` plus 14 new
    unit tests in `crate::filter::tests` (color-matrix saturate-zero
    grayscale, hue-rotate identity at 0°, drop-shadow defaults,
    component-transfer routing, merge ordering, …).

- **Round 7** — typed `<filter>` primitive graph parsing + SMIL
  animation `calcMode="paced"` and `calcMode="spline"`.
  - **`crate::filter` module** — walks each `<filter>` element and
    parses its primitive children (`<feGaussianBlur>`, `<feOffset>`,
    `<feFlood>`, `<feComposite>`, `<feBlend>`, `<feMorphology>`) into
    a typed `FilterGraph`. Each `FilterPrimitiveNode` carries the
    primitive's region (`x` / `y` / `width` / `height`), optional
    `result="..."` label, and the typed `FilterPrimitive` enum value.
    Implicit input chaining: `in=` defaults to the previous
    primitive's `result`, or `SourceGraphic` for the first primitive
    (per W3C Filter Effects §6.2). Unknown primitives (e.g.
    `<feColorMatrix>`) are skipped from the typed graph but still
    survive the verbatim XML round-trip via `PreservedExtras`.
  - `defs::FilterDef` now carries a `graph: FilterGraph` field
    alongside the existing `element: Element`. The verbatim XML
    remains the source of truth for round-trip emission; the typed
    graph is the parallel view a downstream rasterizer should
    consume.
  - **`calcMode="paced"`** — redistributes `keyTimes` so each segment
    is traversed at constant attribute-space speed. Numeric values
    use `|b - a|`; colour values use Euclidean distance in 4-component
    RGBA. Non-numeric / non-colour values fall back to uniform
    spacing (the round-4 default).
  - **`calcMode="spline"`** — eases each segment through a cubic
    Bézier from `keySplines="x1 y1 x2 y2 ; ..."` (one quadruple per
    segment).  Resolved with 6 Newton-Raphson iterations on the x
    curve to invert `x(s)→s`, then `y(s)` gives the eased fraction.
    Missing or malformed `keySplines` falls back to linear within
    the segment.

- **Round 6** — CSS 3 Selectors L3 leftovers + SVG 2 `d` as a
  presentation property.
  - **`:nth-last-child(An+B)`** and **`:nth-last-of-type(An+B)`** —
    1-indexed structural pseudo-classes counted from the *end* of the
    parent's element-children list. Uses the existing
    `MatchContext.{sibling,of_type}_count` totals — no extra storage.
  - **`:lang(L)`** — BCP 47 dash-match against the element's nearest
    `xml:lang` / `lang` attribute. Walks the existing `MatchContext`
    parent chain so an `xml:lang` on a `<g>` or root `<svg>` flows
    through to descendants per Selectors L3 §6.6.2.
  - **`d` as a CSS property** (SVG 2 §9.3.2) — a `<path>` element's
    geometry can now be set via a CSS rule (`path { d: "M 0 0 L 10 10" }`)
    or inline `style="..."`. The cascade is the same as for `fill` /
    `stroke`: the last `d` declaration wins; presentation-attr is the
    floor; `d: none` reduces the path to a no-render. New
    `parse_path_with_css(el, mctx, sheet)` helper sits next to the
    legacy `parse_path(el)`; the path branch of `parse_element_to_node_ctx`
    routes through the CSS-aware version.

  *Alternatives considered* (Round 6 candidate list, picked option
  3 + option 1's surviving piece): bearing commands `B/b` (#1) — were
  dropped from SVG 2 CR, so out of scope; marker rendering (#2) —
  needs a `Marker` construct in `oxideav-core` (deferred); filter
  primitive rasterisation (#4) — `oxideav-raster` work; text
  rendering (#5) — already wired through scribe in round 2.
  CSS3 leftovers + SVG 2 `d` are the highest-leverage unblock for
  modern editor exports (Figma + Illustrator emit both).

- **Round 5** — CSS 3 Selectors Level 3 subset (W3C
  REC-css3-selectors). Extends the round-4 cascade with:
  - **Attribute predicates**: `[attr]`, `[attr=val]`, `[attr~=val]`,
    `[attr|=val]`, `[attr^=val]`, `[attr$=val]`, `[attr*=val]`. Quoted
    values are unwrapped; namespace-prefixed names (`xlink:href`) are
    honoured verbatim.
  - **Combinators**: descendant (` `), child (`>`), adjacent sibling
    (`+`), general sibling (`~`). Matched right-to-left through a
    lifetime-tied `MatchContext` ancestor chain — no Vec allocations
    per element.
  - **Structural pseudo-classes**: `:first-child`, `:last-child`,
    `:only-child`, `:nth-child(N)` (numeric, `odd`, `even`, `An+B`),
    `:first-of-type`, `:last-of-type`, `:only-of-type`,
    `:nth-of-type(N)`, `:not(simple)` (negation of one simple
    selector per Selectors L3).
  - Specificity extended per CSS3 §9: attribute and pseudo-class
    predicates count as a class; `:not(X)` folds in `X`'s
    specificity.
  - Unsupported pseudo-classes (`:hover`, `:focus`, `:checked`, …)
    are silently dropped at parse time so the rest of the rule still
    applies — over-match is the friendlier failure mode for static
    document scrapes.

  *Alternatives considered* (Round 5 candidate list, picked option
  2): SVG-2 path syntax extensions (#1) — narrow surface, low usage
  in real exports; filter primitive rasterisation (#3) — needs
  `oxideav-raster` filter graph (deferred); marker rendering (#4) —
  modest scope but lower download-share than CSS3 selectors; text
  rendering (#5) — already wired through scribe in round 2 for the
  vector path. CSS3 selector subset is the highest-leverage unblock
  for editor-emitted SVG (Inkscape/Illustrator/Figma frequently emit
  `:nth-child` and attribute selectors in their `<style>` blocks).

  Implemented in `oxideav_svg::css` (rewrite). New public types:
  `MatchContext`, `SimpleSelector`, `CompoundSelector`, `Combinator`,
  `AttrPredicate`, `AttrOp`, `Pseudo`. Existing `Selector` is now an
  alias for `CompoundSelector`. New `PaintState::merged_with_mctx`
  takes a chained context; `merged_with_css` keeps the round-4
  signature by building an isolated context internally. New
  `parse_element_to_node_ctx` is the round-5 entry point used by the
  decoder; `parse_element_to_node` is the round-4 wrapper.

- **Round 4** — SMIL animation snapshot at arbitrary `t`. New
  `parse_svg_at(bytes, t_seconds)` evaluates every `<animate>` /
  `<set>` / `<animateTransform>` using the full SMIL timing model:
  `begin`, `dur` (with `s` / `ms` / `min` / `h` / `H:M:S` clock-value
  units), `repeatCount` (numeric or `indefinite`), `keyTimes` /
  `values` segmented interpolation, `from` / `to` / `by` shorthand,
  `calcMode="discrete|linear"`. Colours interpolate componentwise;
  numbers lerp; everything else is discrete. `<animateTransform>`
  works for `type="translate|rotate|scale"`. `parse_svg(bytes)`
  retains the round-3 t=0 behaviour.
- **Round 4** — minimal CSS cascade. `<style>` blocks (with comments,
  `@`-rule skipping, and CDATA bodies) plus inline `style="..."`
  attributes resolve via tag / class / id selectors with CSS2.1
  specificity ordering. Cascade applies to `fill`, `stroke`,
  `stroke-width`, `opacity`, `fill-rule`, etc.; properties not
  modelled by the paint state are silently ignored. Implemented in
  the new `oxideav_svg::css` module.
- **Round 4** — encoder preservation of `<style>` / `<filter>` /
  `<animate>` / `<foreignObject>` via the `PreservedExtras`
  side-channel. New `parse_svg_with_extras(bytes)` returns the scene
  graph plus a captured-XML buffer; the symmetric
  `write_svg_with_extras(frame, extras)` re-emits those fragments so
  a parse → write round-trip no longer drops dynamic / filter / CSS
  definitions. Bare `parse_svg` / `write_svg` keep the round-3
  behaviour.

## [0.1.2](https://github.com/OxideAV/oxideav-svg/compare/v0.1.1...v0.1.2) - 2026-05-04

### Added

- round 3 — <use>, .svgz inflate, <animate>/<set> snapshot at t=0
- round 2 — <text>/<tspan> via scribe vector-first API
- round 2 — <mask>/<clipPath> + multi-shape clip + SoftMask compositing
- round 2 — <filter> graceful pass-through via DefsTables

### Added

- **Round 3** — `<use href="#id">` cross-references. Resolves the
  referenced element from a documentwide id table built during the
  pre-walk; honours `x` / `y` / `transform` on the `<use>` and
  recognises both SVG-2 `href` and SVG-1.1 `xlink:href`. `<symbol>`
  references inline the symbol's children. Cycles
  (`use → symbol → use of same id`) are detected and dropped.
- **Round 3** — `.svgz` (gzip-compressed SVG, RFC 1952) inflate +
  deflate. `parse_svg` and the `svg` demuxer transparently sniff the
  gzip magic (`1f 8b`); `write_svgz()` and a sister `svgz` muxer
  produce gzipped output. Pure-Rust backend (`flate2 rust_backend`),
  no C deps.
- **Round 3** — `<animate>` / `<set>` / `<animateTransform>` snapshot
  at `t=0`. The animation's `from` (or first `values` entry, or `to`)
  is folded into the parent element's attribute set, matching what
  most browsers paint on first frame instead of silently dropping
  animated content.

## [0.1.1](https://github.com/OxideAV/oxideav-svg/compare/v0.1.0...v0.1.1) - 2026-05-04

### Fixed

- parse_number accepts unit suffixes; trim_float normalises -0
- *(docs)* clippy doc_lazy_continuation in parser.rs (Rust 1.95)

### Other

- snake_case fn name + non-exhaustive Node arm

## [0.1.0] - 2026-05-04

### Added

- Initial release: pure-Rust SVG (read + write) for the oxideav framework.
- Hand-rolled SAX-style XML parser (no external XML crate).
- `d` attribute mini-language parser: M/m, L/l, H/h, V/v, C/c, S/s, Q/q,
  T/t, A/a, Z/z (absolute and relative; smooth-curve reflection of the
  previous control point).
- Element parsers: `<svg>`, `<rect>`, `<circle>`, `<ellipse>`, `<line>`,
  `<polyline>`, `<polygon>`, `<path>`, `<g>`, `<linearGradient>`,
  `<radialGradient>`.
- Attribute parsers: `fill` / `stroke` (named CSS colors + `#hex` 3/4/6/8
  + `rgb()` + `rgba()` + `none`), `stroke-width`, `stroke-linecap`,
  `stroke-linejoin`, `stroke-miterlimit`, `stroke-dasharray`,
  `stroke-dashoffset`, `opacity`, `fill-opacity`, `stroke-opacity`,
  `fill-rule`, `transform` (matrix / translate / rotate / scale / skewX /
  skewY).
- Encoder: emits well-formed SVG covering the round-1 element subset.
- Codec + container registration (the SVG file *is* its own container —
  same pattern as `oxideav-png` for a static PNG).

### Deferred (round 2+)

- `<text>` — needs font handling and tight `oxideav-scribe` coupling.
  Tracked on #352 (scribe vector-first work, blocked on round 5 scribe).
- `<filter>` / `<feGaussianBlur>` etc.
- `<mask>` / `<clipPath>` beyond simple shape clip.
- `<use>` / `<symbol>` / `<defs>` cross-references.
- `<foreignObject>`.
- `<animate>` / `<animateTransform>` / `<set>`.
- `<script>`.
