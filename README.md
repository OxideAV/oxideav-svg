# oxideav-svg

[![CI](https://github.com/OxideAV/oxideav-svg/actions/workflows/ci.yml/badge.svg)](https://github.com/OxideAV/oxideav-svg/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/oxideav-svg.svg)](https://crates.io/crates/oxideav-svg) [![docs.rs](https://docs.rs/oxideav-svg/badge.svg)](https://docs.rs/oxideav-svg) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Pure-Rust SVG read + write for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework. Implements a
focused subset of SVG 1.1 / 2.0 — enough to load the great majority of
real-world icons, logos, and editor exports — with a hand-rolled SAX
parser and no external XML / SVG dependency. The decoder produces an
[`oxideav_core::VectorFrame`]; the encoder serialises one back, and a
`parse → write` round-trip preserves dynamic / filter / CSS definitions
the IR cannot model directly via a `PreservedExtras` side-channel.

## Elements

* **Containers** — `<svg>` (`viewBox` / `width` / `height` / `xmlns`),
  `<g>` (with `transform`), `<defs>`, `<symbol>`. A *nested* `<svg>`
  establishes a new viewport per §8.2 — its `x` / `y` place the
  viewport, `width` / `height` (default `100%`) size it, and an optional
  `viewBox` + `preserveAspectRatio` (incl. the optional `defer` prefix)
  re-map the inner coordinate system;
  descendant percentage lengths resolve against the nested viewport and
  a zero-size nested `<svg>` drops its subtree. A `<symbol>` instantiated
  via `<use>` honours its `viewBox` / `preserveAspectRatio` / intrinsic
  size, the SVG 2 §5.5 `x` / `y` geometry properties (positioning the
  instantiated viewport), and the §5.5 `refX` / `refY` reference point
  (`<length>` or the `left`/`center`/`right` · `top`/`center`/`bottom`
  keywords), aligning that point with the use's `x` / `y`.
* **Shapes** — `<rect>` (incl. `rx`/`ry`), `<circle>`, `<ellipse>`,
  `<line>`, `<polyline>`, `<polygon>`, `<path>` (full `d` mini-language:
  M/m L/l H/h V/v C/c S/s Q/q T/t A/a Z/z, smooth-curve reflection),
  plus the `pathLength` rescaling of dash patterns. A
  `parse_svg_with_extras → write_svg_with_extras` round-trip keeps each
  basic shape's **native identity** (SVG 2 §9.2–§9.7): the encoder
  re-emits `<rect x=… width=…>` / `<circle cx=…>` / … with the verbatim
  geometry attributes instead of the flattened `<path d>`, so
  attribute-targeting SMIL animations and `rect {}` type selectors
  still resolve after a round-trip (the §13.8 stroke-first
  `paint-order` split declines — its two single-purpose paths keep the
  flattened form). The emitted `stroke-dasharray` / `stroke-dashoffset`
  next to a re-emitted `pathLength=` are divided back to author units
  so the §9.6.1 rescale is applied exactly once per parse.
* **Paint servers** — `<linearGradient>` / `<radialGradient>` with
  `<stop>` children and `spreadMethod`, resolved via `fill="url(#id)"`.
  A `parse_svg_with_extras → write_svg_with_extras` round-trip
  preserves the **reference identity**: the author's verbatim gradient
  def (original id, `gradientUnits`, `gradientTransform`, `href`
  template chain) is re-emitted and the `fill=` / `stroke=` reference
  re-points at the source id instead of a flattened synthesised
  `grad{N}` twin.
* **Text** — `<text>` / `<tspan>` (and `textPath` align-mode layout)
  rasterised vector-first through
  [`oxideav-scribe`](https://github.com/OxideAV/oxideav-scribe); the
  caller installs a font resolver once at startup. Gated behind the
  default-on `text` feature. A
  `parse_svg_with_extras → write_svg_with_extras` round-trip re-emits
  the **verbatim `<text>` element** (SVG 2 §11.2) in place of the
  flattened glyph outlines — string content, font selection properties,
  the §11.2.2 `<tspan>` per-character positioning arrays
  (`x`/`y`/`dx`/`dy`/`rotate`), `<textPath>` layout, and animation
  children all survive byte-exactly through a
  mixed-content-preserving serialiser (no synthetic indentation around
  spans, so `xml:space="default"` collapsing cannot corrupt inter-span
  whitespace).
* **References** — `<use href="#id">` (SVG 2 `href` + SVG 1.1
  `xlink:href`), with cycle detection. The decoder flattens each `<use>`
  into the instantiated geometry for rendering, but a
  `parse_svg_with_extras → write_svg_with_extras` round-trip *collapses*
  the instance back to a single `<use href="#id" …/>` (preserving the
  reference identity + `x`/`y`/`width`/`height`/`transform`/own-`id`)
  instead of inlining the target N times, and re-emits the
  `<defs>`-housed target (plain shape / `<g>` / `<symbol>`) so the
  reference still resolves.
* **Masking / clipping** — `<mask>` → `oxideav_core::Node::SoftMask`
  honouring `mask-type`; `<clipPath>` collapsed into the group's `clip`.
  A `parse_svg_with_extras → write_svg_with_extras` round-trip preserves
  the *reference identity*: the verbatim `<clipPath>` / `<mask>` def
  (original id, `clipPathUnits` / `maskUnits`, and every clip shape) is
  re-emitted and the `clip-path=` / `mask=` reference re-points at the
  source id, instead of the flattened single-shape synthesis the render
  path uses.
* **Filters** — `<filter>` primitive graphs (`feGaussianBlur`,
  `feOffset`, `feFlood`, `feComposite`, `feBlend`, `feMorphology`,
  `feColorMatrix`, `feComponentTransfer`, `feDropShadow`, …) parsed into
  a typed `FilterGraph`, with the `<filter>` element's
  coordinate-system / colour-space / `filterRes` / href-inheritance
  attributes captured. In-crate pixel-level evaluators exist for
  `feDropShadow`, `feComposite`, `feColorMatrix`, `feFlood`,
  `feGaussianBlur` (all three `edgeMode`s — `none` / `duplicate` /
  `wrap`), `feOffset`, `feMerge` (the §9.16 bottom-to-top `over`
  stack), `feBlend` (the five SVG 1.1 §15.9 modes — `normal` /
  `multiply` / `screen` / `darken` / `lighten`; the other eleven
  `<blend-mode>` values defer to the un-staged `[COMPOSITING-1]`
  formulae), `feComponentTransfer` (the §9.7 per-channel
  `identity` / `table` / `discrete` / `linear` / `gamma` transfer
  functions), `feMorphology` (the §9.17 `erode` / `dilate`
  component-wise min/max over the `2·radius_x × 2·radius_y` kernel
  rectangle on premultiplied values, with independent x/y radii and the
  negative/zero-radius identity short-circuit), and `feConvolveMatrix`
  (the §9.9 2-D linear convolution with the 180°-rotated `kernelMatrix`,
  `divisor` / `bias` / `targetX` / `targetY` placement, all three
  `edgeMode`s, the `divisor="0"` → kernel-sum fallback, and both
  `preserveAlpha` modes — premultiplied all-channel convolution when
  `false`, un-premultiplied colour-only with the alpha passed through
  when `true`), `feDisplacementMap` (the §9.11 spatial displacement
  `P'(x,y) ← P(x + scale·(XC−½), y + scale·(YC−½))` with the
  dual-colour-space rule — `in2` channels read non-premultiplied in the
  working space, `in` passed through in its own space), `feTile` (the
  §9.20 reference-tile periodic replication), `feTurbulence` (a
  clean-room port of the §9.21 Perlin-noise / `fractalNoise` reference
  algorithm — Park–Miller LCG, unit-disc gradient lattice, `noise2`
  bilinear sampling, per-octave sum, and `stitchTiles`), and the two
  lighting primitives `feDiffuseLighting` / `feSpecularLighting` (the
  §18 alpha-as-height-map surface normal from all nine position-
  dependent Sobel kernels — four corners, four edges, the interior, each
  with its `FACTORx`/`FACTORy` scale and edge-clamped sampling — driven
  by the constant `<feDistantLight>` vector or the position-dependent
  `<fePointLight>` / `<feSpotLight>` vectors, the spot
  `pow(-L·S, exp)` cone fall-off with `limitingConeAngle` clipping, the
  §18 Lambertian `D = kd·(N·L)·Lcolor` opaque map, and the §19
  Blinn-Phong `S = ks·pow(N·H, exp)·Lcolor` highlight with the constant
  `E = (0,0,1)` eye vector and the `Sa = max(Sr,Sg,Sb)` non-opaque
  alpha). A top-level `evaluate_filter_graph` chains those per-primitive
  evaluators into a full DAG: it maintains the named-`result` map and the
  implicit "previous result" fallback the `in` attribute defines (first
  primitive → `SourceGraphic`, subsequent → prior result), derives
  `SourceAlpha` / `BackgroundAlpha` from their colour counterparts,
  resolves every `in` / `in2` (including unknown `result` references) to a
  pixel buffer, dispatches each node, and returns the final layer; the
  whole graph runs at full filter-region resolution.
  `evaluate_filter_graph_clipped` additionally applies each primitive's
  resolved subregion as the §9.4 hard clip on that primitive's result —
  pixels outside the supplied `PixelRect` become transparent black and a
  zero-extent subregion disables the primitive — applied *before* the
  result is stored / reused. `resolve_subregions` computes those
  `PixelRect`s end-to-end from a `FilterSubregionCtx` (the §8 filter
  region plus the user-space / object-bounding-box mappings): it honours
  the §7 `<length-percentage>` distinction (percentages resolve against
  the filter region, numbers consult `primitiveUnits`), the §9.4 default
  subregion (union of referenced nodes, whole filter region for
  standard-input references and `feImage` / `feTurbulence` / `feTile`),
  and the negative/zero-extent disable rule — so the SVG layer owns the
  full §9.4 resolve → clip pipeline. The turn-key
  `evaluate_filter_graph_resolved` composes the resolver with the clipped
  evaluator, deriving the working-raster size from the filter region. The
  general rasteriser surface remains `oxideav-raster` work. A
  `parse_svg_with_extras → write_svg_with_extras` round-trip re-attaches
  the `filter="url(#id)"` reference on the wrapper group so the preserved
  `<filter>` def stays connected to its graphics element (a chained
  `url(#a) url(#b)` list round-trips verbatim).
* **Markers** — `<marker>` definitions parse into a typed `MarkerDef`
  and round-trip; a `parse_svg_with_extras → write_svg_with_extras`
  round-trip also re-attaches the shape's `marker-start` / `marker-mid`
  / `marker-end` references (the `marker` shorthand expands into the
  three longhands) so the preserved def stays referenced. Vertex
  placement / `orient` rendering is deferred to a core `Marker` node.
* **Animation** — `<animate>` / `<set>` / `<animateTransform>`
  snapshotting via `parse_svg_at(bytes, t)` with the SMIL timing model
  (`begin` / `dur` / `repeatCount` / `keyTimes` / `values` /
  `from`/`to`/`by`, `calcMode` `discrete`/`linear`/`paced`/`spline`).
  `parse_svg` snapshots first-paint at `t = 0`. On round-trip every
  animation element is re-emitted **as a child of its direct XML
  parent** (SMIL Animation §3.1 implicit targeting), keyed by
  scene-graph path — id-less parents included — and a structural
  suppression multiset guarantees each source animation appears exactly
  once even when it also rides a verbatim side-channel tree (pattern /
  defs target / `<text>` / `<switch>` / captured `<image>`).
* **Conditional processing** — `<switch>` evaluates
  `requiredExtensions` / `systemLanguage` and renders the first passing
  child; a `parse_svg_with_extras → write_svg_with_extras` round-trip
  re-emits the whole `<switch>` verbatim (every alternative + the
  conditional attributes) rather than freezing the decode-time
  selection, so a re-parse under a different `systemLanguage` re-selects
  correctly. `<view>` definitions + `resolve_fragment` honour both
  bare-name and `svgView(...)` fragment identifiers.
* **`<image>` capture** — the SVG 2 §6 `<image>` element is captured
  into `PreservedExtras::images` as a typed `SvgImage`: inline
  `data:` payloads are base64-decoded (MIME recorded), external URLs
  preserved verbatim (the decoder never fetches). A
  `parse_svg_with_extras → write_svg_with_extras` round-trip is
  lossless — the geometry `x` / `y` / `width` / `height` keep their
  source `<length>` unit / percentage token (`width="50%"` survives as
  `50%`), `preserveAspectRatio` / `image-rendering` / `crossorigin` are
  preserved, every unmodelled core / styling / conditional-processing
  attribute (`class`, `style`, `opacity`, `clip-path`, `mask`, `filter`,
  `visibility`, `requiredExtensions`, `systemLanguage`, `xlink:title`,
  `data-*`) round-trips through `SvgImage::extra_attrs` in document
  order, and the §6 child content (`<title>` / `<desc>` / `<metadata>`
  + animation elements) is re-emitted verbatim. Painting the decoded
  raster into vector space remains `oxideav-raster` work.
* **Graceful handling** — `<foreignObject>` parses to an empty group;
  unknown content survives the verbatim round-trip via `PreservedExtras`.

## Styling

* **Presentation attributes** — `fill` / `stroke` (named CSS colours,
  `#hex` 3/4/6/8, `rgb()`/`rgba()`, `none`, `url(#id)`, `currentColor`),
  `stroke-width`, line cap / join / miterlimit, dash array / offset,
  opacity family, `fill-rule`, `transform`.
* **CSS cascade** — `<style>` blocks and inline `style="..."` resolve
  through a CSS 3 Selectors Level 3 subset (tag / class / id selectors,
  attribute predicates, combinators, structural pseudo-classes) with
  CSS 2.1 specificity ordering, including `@media` query gating.
* **Rendering / colour hints** — the inherited §13.x hints
  (`color-rendering`, `shape-rendering`, `text-rendering`,
  `image-rendering`, `color-interpolation`,
  `color-interpolation-filters`) plus the `overflow`, `pointer-events`,
  `cursor`, and `dominant-baseline` properties parse, cascade, and
  round-trip; their visual effect is consumed downstream by
  `oxideav-raster` / `oxideav-scribe`.

## Round-trip conformance

A write-side conformance gate
(`tests/round449_write_conformance.rs`) runs a per-feature corpus
(`tests/fixtures/corpus/*.svg` — shapes, gradients, pattern,
use/defs/symbol, switch, filter chains, clip/mask, markers, text, all
four SMIL animation elements, image, nested `<svg>`, CSS + `@media`,
presentation-hint carriers, hyperlinks + descriptive elements,
view/script/foreignObject, plus a real-world icon and a `.svgz` leg)
through four invariants:

1. parse and re-parse of the writer's output succeed;
2. the write reaches an **immediate byte fixed point**
   (`write(parse(write(x))) == write(x)`);
3. an element **census** — occurrence counts for 31 semantically
   countable tags survive exactly (nothing lost, nothing duplicated);
4. **scene equivalence** — source and round-tripped documents flatten
   to byte-identical extras-free scene serialisations.

## Compression

`.svgz` (gzip-compressed SVG, RFC 1952) is sniffed transparently on
read; `write_svgz()` and the `svgz` muxer produce gzipped output. Pure
Rust, no C dependencies.

## Robustness

The parse/model surface is hardened against adversarial input — every
public parser returns a typed `oxideav_core::Error` (or a value) and
never panics or aborts, and every unbounded-resource path carries an
explicit ceiling:

* **Nesting depth** — the SAX parser refuses to descend past
  `parser::MAX_XML_DEPTH` (128) and the model-builder past
  `element::MAX_RENDER_DEPTH` (128), so a document with tens of
  thousands of nested `<g>` elements is rejected with an error instead
  of overflowing the native stack.
* **`<use>` expansion** — beyond the existing cycle guard, a global
  instantiation budget (`element::MAX_USE_EXPANSIONS`) caps *diamond*
  blow-up (`#n0 → #n1 ×2 → …`, 2ⁿ nodes with no repeated id), and the
  render-depth guard caps a *linear* `<use>` chain (a decode recursion
  as deep as the chain even though the XML is flat).
* **`.svgz` inflation** — gzip input is inflated through a limited
  reader capped at `parser::MAX_SVGZ_INFLATED` (128 MiB), so a
  decompression bomb is refused before its payload is ever materialised.
* **Reference chains** — gradient/pattern template inheritance and
  `<filter>` `href` inheritance each combine a visited-set cycle guard
  with an eight-hop depth cap.

A curated adversarial corpus plus a seeded byte-mutation fuzzer over the
path/transform/length/paint/XML/document parsers (see
`tests/round403_parser_robustness.rs`) enforce the no-panic invariant.

## Not yet supported

* Supplying the standard-input slots `BackgroundImage` / `FillPaint` /
  `StrokePaint` (the graph evaluator defaults them to transparent black
  until the rasteriser supplies them); the eleven extra `feBlend`
  `<blend-mode>` values + the `feComposite` `in` / `out` / `atop` / `xor`
  operators (whose mixing formulae live in the un-staged `[COMPOSITING-1]`
  / `[PORTERDUFF]` companion specs) and `feImage` external-reference
  resolution also remain rasteriser work.
* Marker rendering, `textPath method="stretch"` per-glyph warping,
  *painting* the captured `<image>` raster into vector space (the
  element itself is parsed + losslessly round-tripped — see **`<image>`
  capture** above), and live pseudo-element / stateful pseudo-class
  evaluation (selectors parse + round-trip; the synthesised-box renderer
  is `oxideav-raster` work).

## Usage

```rust,no_run
use oxideav_svg::{parse_svg, write_svg};

let bytes = std::fs::read("icon.svg").unwrap();
let frame = parse_svg(&bytes).unwrap();
let out = write_svg(&frame);
std::fs::write("icon.out.svg", out).unwrap();
```

Register the codec into a runtime context:

```rust,no_run
let mut ctx = oxideav_core::RuntimeContext::new();
oxideav_svg::register(&mut ctx);
```

`<text>` / `<tspan>` emit glyph paths only when a font resolver is
installed (the crate does not own a font registry):

```rust,no_run
use oxideav_scribe::{Face, FaceChain};

let dejavu = std::fs::read("DejaVuSans.ttf").unwrap();
oxideav_svg::text::set_font_resolver(move |_family, _size_px| {
    Face::from_ttf_bytes(dejavu.clone()).ok().map(FaceChain::new)
}).ok();
```

```toml
[dependencies]
oxideav-svg = "0.1"
```

Disable default features (`default-features = false`) to drop the
`<text>` path and the scribe dependency tree.

## License

MIT — see [LICENSE](LICENSE).
