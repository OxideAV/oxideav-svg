# oxideav-svg

[![CI](https://github.com/OxideAV/oxideav-svg/actions/workflows/ci.yml/badge.svg)](https://github.com/OxideAV/oxideav-svg/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/oxideav-svg.svg)](https://crates.io/crates/oxideav-svg) [![docs.rs](https://docs.rs/oxideav-svg/badge.svg)](https://docs.rs/oxideav-svg) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Pure-Rust SVG read + write, following the OxideAV image-crate contract
(`IMAGE_CRATE_API`), usable with or without the
[`oxideav`](https://github.com/OxideAV/oxideav) framework. Implements a
focused subset of SVG 1.1 / 2.0 — enough to load the great majority of
real-world icons, logos, and editor exports — with a hand-rolled SAX
parser and no external XML / SVG dependency. The parser produces an
[`SvgDocument`] (the crate's own vector scene graph); the writer
serialises one back byte-stably, and a `parse → write` round-trip
preserves dynamic / filter / CSS definitions the scene graph cannot
model via a `PreservedExtras` side-channel.

## SVG is vector: what the contract means here

Every other image crate decodes to pixels. SVG has none: a document is
paths, paints, strokes, transforms and groups, and producing pixels
from it is rasterisation — `oxideav-raster`'s job, through the
framework. So the contract vocabulary is present in full, with these
meanings:

| Verb | Standalone (`default-features = false`) | With `registry` |
|---|---|---|
| `probe` | allocation-free `<svg` / gzip sniff | same |
| `info` → `ImageInfo` | header-only: the canvas a rasteriser would produce (CSS px at 96 dpi), `Rgba`, one frame, sRGB, plus `view_box` / `user_width` / `user_height` / `animated` / `compressed` | same |
| **`parse`** → `SvgDocument` | **the real decode**: the vector document | same; `VectorFrame::from(doc)` |
| **`write`** → `Vec<u8>` | **the real encode**: SVG text (or `.svgz`) | same; `SvgDocument::from(&frame)` |
| `decode` / `decode_with` / `decode_rgb8` / `decode_rgba8` / `decode_from` | parse + limits, then `Error::Unsupported` ("rasterising SVG needs oxideav-raster through the framework") | same — the framework `Decoder` emits `Frame::Vector`, which `oxideav-raster` paints |
| `encode` / `encode_rgb8` / `encode_rgba8` / `encode_to` | validate, then `Error::Unsupported` (an SVG is not a raster target) | same |

`SvgImage` / `RgbImage` / `RgbaImage` / `PixelFormat` (`Rgba`, `Rgb24`)
exist so callers can be generic over image crates (and so a framework
`VideoFrame` can be held in the contract shape); this crate never
produces one. `info`'s canvas and `DecodeOptions`' geometry limits
describe what a rasteriser *would* allocate.

## Standalone use

```toml
[dependencies]
oxideav-svg = { version = "0.1", default-features = false }
```

```rust,no_run
let bytes = std::fs::read("icon.svg").unwrap();
if oxideav_svg::probe(&bytes) {
    let info = oxideav_svg::info(&bytes).unwrap();      // header only
    println!("{}×{} px canvas, view box {:?}", info.width, info.height, info.view_box);

    let doc = oxideav_svg::parse(&bytes).unwrap();      // SvgDocument: the scene graph
    for node in &doc.root.children { let _ = node; }     // Node::Path / Group / SoftMask

    let out = oxideav_svg::write(&doc);                 // byte-stable SVG text
    std::fs::write("icon.out.svg", out).unwrap();

    let opts = oxideav_svg::DecodeOptions::default().with_max_elements(10_000u64);
    let (doc, extras) = oxideav_svg::parse_with_extras_opts(&bytes, &opts).unwrap();
    let faithful = oxideav_svg::write_with_extras(&doc, &extras); // CSS / filters / SMIL kept
    let svgz = oxideav_svg::write_with(&doc, &oxideav_svg::EncodeOptions::new().with_compress(true)).unwrap();
    let _ = (faithful, svgz);
}
```

Depth entry points: `parse_at(bytes, t_seconds)` samples the SMIL
timeline, `parse_at_with_languages` drives `<switch systemLanguage>`,
`parse_with_extras` / `write_with_extras` carry the `PreservedExtras`
side-channel, `write_svgz` gzips, `parse_from` / `write_to` stream,
`resolve_fragment` routes SVG 2 §16.3 fragment identifiers. The
`text` feature (default-on, implies `registry`) shapes `<text>` into
glyph outlines through `oxideav-scribe`; without it a `<text>` element
keeps its slot as an empty group and still round-trips verbatim.

## Framework use

```rust,no_run
let mut ctx = oxideav_core::RuntimeContext::new();
oxideav_svg::register(&mut ctx);                 // codec "svg" + containers svg / svgz
// or: register_codecs(&mut ctx.codecs); register_containers(&mut ctx.containers);
let params = oxideav_core::CodecParameters::video(oxideav_core::CodecId::new(oxideav_svg::CODEC_ID_STR));
let _dec = oxideav_svg::make_decoder(&params).unwrap();   // Packet → Frame::Vector
let _enc = oxideav_svg::make_encoder(&params).unwrap();   // Frame::Vector → Packet

let doc = oxideav_svg::parse(b"<svg width='1' height='1'/>").unwrap();
let frame: oxideav_core::VectorFrame = doc.into();        // and back: SvgDocument::from(&frame)
```

The framework `Decoder` / `Encoder` are thin adapters over `parse` /
`write` (one implementation); `SvgDocument` ↔ `VectorFrame` converts
field for field (a frame's raster `Node::Image` becomes an empty group:
the SVG writer never serialised it). `SvgImage` bridges to `VideoFrame`
(`From`, `from_video_frame`, `TryFrom<(&VideoFrame, &CodecParameters)>`)
with the sRGB colour signal stamped. `SvgError` maps onto
`oxideav_core::Error` (`LimitExceeded` → `ResourceExhausted`).
The deprecated `parse_svg*` / `write_svg*` wrappers keep their
`VectorFrame` signatures for one release.

`<text>` emits glyphs only when a font hook is installed (the crate does
not own a font registry). The provider is asked once per distinct
`(family, weight, style)` of a document and its answer is shared by
every size and run, so hand out the same `Arc<FaceChain>`:

```rust,no_run
use std::sync::Arc;
use oxideav_scribe::{Face, FaceChain};

let regular = Arc::new(FaceChain::new(
    Face::from_ttf_bytes(std::fs::read("DejaVuSans.ttf").unwrap()).unwrap(),
));
oxideav_svg::text::set_font_provider(move |query| {
    // query.family() / .families() / .weight() / .italic() / .size_px()
    let _ = query;
    Some(regular.clone())
}).ok();
```

The provider first sees the raw `font-family` list; if it declines it is
asked for each family in turn and finally for `sans-serif`, so a missing
family falls back instead of dropping the text. An upright face answers
an italic query with sheared outlines, a regular one a bold query with
a thin outline in the fill colour, and a face without the `smcp` feature
gets small capitals from reduced capitals. The older
`text::set_font_resolver(|family, size_px| -> Option<FaceChain>)` keeps
working (same fallback order; called once per family *and size*).

## Supported layouts

| Layout | `info` | `decode` | `encode` |
|---|---|---|---|
| `Rgba` | reported (the canvas a rasteriser produces) | `Unsupported` — vector, see above | `Unsupported` — not a raster target |
| `Rgb24` | — | — | accepted by `SvgImage::from_rgb8`, then `Unsupported` |

The vector document (`parse` / `write`) is the real decode / encode
pair; its element coverage is listed under **Elements** below.

## Options

`DecodeOptions` (`#[non_exhaustive]`, `Default`, `with_*`):

| Field | Default | Applies to |
|---|---|---|
| `max_bytes` | 128 MiB | input length and the inflated size of an `.svgz` (`parse*`, `info`, `decode*`) |
| `max_elements` | 1 048 576 | XML elements per document (`parse*`, `decode*`) |
| `max_depth` | 128 | XML nesting depth (`parse*`, `decode*`) |
| `max_width` / `max_height` / `max_pixels` | `None` / `None` / 256 Mpx | the `info` canvas, checked by `decode*` only — parsing allocates no pixels |
| `strict` | `false` | reject mismatched / unterminated tags, a document element other than `<svg>`, more than one top-level element |

`EncodeOptions` has one field, `compress` (gzip `.svgz` body), because
SVG offers exactly one encoding choice; `write_with` /
`write_with_extras_opts` / `write_to` honour it.

## Metadata and colour

SVG colours are sRGB by definition (SVG 1.1 §4.2 / CSS Color), so
`ImageInfo.color` and `SvgImage::color` default to
`ColorInfo::srgb()` — full range, primaries `1`, transfer `13`, matrix
`0` — and the raster bridge stamps that signal on the frame (the
format defines it). SVG embeds no ICC profile or Exif payload
(`has_icc` / `has_exif` are `false`); `has_xmp` is `true` when a
`<metadata>` element carries an `<x:xmpmeta>` / `<rdf:RDF>` packet.
`Metadata.gamma` is always `None`.

`info`'s canvas: the root `width` / `height` with absolute units
converted at 96 dpi (`in`, `cm`, `mm`, `Q`, `pt`, `pc`, `px`; a bare
number is one user unit = 1 px), percentages and font-relative lengths
falling back to the `viewBox`, a missing axis derived from the other
via the `viewBox` aspect ratio, rounded up to whole pixels; `0 × 0`
when the document has no intrinsic size. `user_width` / `user_height`
carry the same canvas unrounded — the values `parse` stores in
`SvgDocument::{width, height}`, so a rasteriser sizes `12cm` as
453.54 px rather than 12.
`frames` is always `1`: SMIL animation is a continuous timeline
(`parse_at`), flagged by `animated`.

## Limits

Beyond `DecodeOptions`: the model builder caps render depth at
`element::MAX_RENDER_DEPTH` (128) and `<use>` instantiation at
`element::MAX_USE_EXPANSIONS`; gradient / pattern / filter reference
chains stop after eight hops; `probe` inspects the first `PROBE_WINDOW`
(4 KiB) bytes. `info` may report a canvas larger than any rasteriser
could allocate — it is header-only — and `decode_with` turns that into
`LimitExceeded` against `max_width` / `max_height` / `max_pixels`.

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
  `parse_with_extras → write_with_extras` round-trip keeps each
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
  `<stop>` children, `spreadMethod`, `gradientTransform` and
  `gradientUnits` (SVG 2 §8.11: the default `objectBoundingBox` binds
  the geometry — bare numbers and percentages — to each referencing
  shape's object bounding box; `userSpaceOnUse` percentages resolve
  against the viewport), resolved per shape via `fill="url(#id)"` /
  `stroke="url(#id)"` into the shape's own user space. A radial
  gradient whose rings turn elliptical (non-square box, non-uniform
  `gradientTransform`) paints through a soft mask of the shape with the
  circular gradient under the box matrix (`defs::GradientPaint`).
  `<pattern>` (SVG 2 §14.3) — `patternUnits` / `patternContentUnits` /
  `viewBox` + `preserveAspectRatio` / `patternTransform` / `href`
  template chains — is tiled over each referencing shape through a
  soft mask (`element::expand_pattern`, bounded tile count), so a
  path-only rasteriser paints it; the writer collapses the expansion
  back to the `fill="url(#id)"` reference on an extras round-trip.
  A `parse_with_extras → write_with_extras` round-trip
  preserves the **reference identity**: the author's verbatim gradient
  def (original id, `gradientUnits`, `gradientTransform`, `href`
  template chain) is re-emitted and the `fill=` / `stroke=` reference
  re-points at the source id instead of a flattened synthesised
  `grad{N}` twin.
* **Text** — `<text>` / `<tspan>` / `<textPath>` / `<a>` laid out by
  the SVG 2 §11.5 algorithm over
  [`oxideav-scribe`](https://github.com/OxideAV/oxideav-scribe) shaping;
  gated behind the default-on `text` feature. See **Text** below for the
  property coverage. A `parse_with_extras → write_with_extras`
  round-trip re-emits the **verbatim `<text>` element** (SVG 2 §11.2) in
  place of the flattened glyph outlines — string content, font
  selection properties, the §11.2.2 `<tspan>` per-character positioning
  arrays (`x`/`y`/`dx`/`dy`/`rotate`), `<textPath>` layout, and
  animation children all survive byte-exactly through a
  mixed-content-preserving serialiser (no synthetic indentation around
  spans, so `xml:space="default"` collapsing cannot corrupt inter-span
  whitespace).
* **References** — `<use href="#id">` (SVG 2 `href` + SVG 1.1
  `xlink:href`), with cycle detection. The decoder flattens each `<use>`
  into the instantiated geometry for rendering, but a
  `parse_with_extras → write_with_extras` round-trip *collapses*
  the instance back to a single `<use href="#id" …/>` (preserving the
  reference identity + `x`/`y`/`width`/`height`/`transform`/own-`id`)
  instead of inlining the target N times, and re-emits the
  `<defs>`-housed target (plain shape / `<g>` / `<symbol>`) so the
  reference still resolves.
* **Masking / clipping** — `<mask>` → `Node::SoftMask`
  honouring `mask-type`; `<clipPath>` collapsed into the group's `clip`.
  A `parse_with_extras → write_with_extras` round-trip preserves
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
  `parse_with_extras → write_with_extras` round-trip re-attaches
  the `filter="url(#id)"` reference on the wrapper group so the preserved
  `<filter>` def stays connected to its graphics element (a chained
  `url(#a) url(#b)` list round-trips verbatim).
* **Markers** — `<marker>` definitions parse into a typed `MarkerDef`
  and round-trip; a `parse_with_extras → write_with_extras`
  round-trip also re-attaches the shape's `marker-start` / `marker-mid`
  / `marker-end` references (the `marker` shorthand expands into the
  three longhands) so the preserved def stays referenced. Vertex
  placement / `orient` rendering is deferred to a core `Marker` node.
* **Animation** — `<animate>` / `<set>` / `<animateTransform>`
  snapshotting via `parse_at(bytes, t)` with the SMIL timing model
  (`begin` / `dur` / `repeatCount` / `keyTimes` / `values` /
  `from`/`to`/`by`, `calcMode` `discrete`/`linear`/`paced`/`spline`).
  `parse` snapshots first-paint at `t = 0`. On round-trip every
  animation element is re-emitted **as a child of its direct XML
  parent** (SMIL Animation §3.1 implicit targeting), keyed by
  scene-graph path — id-less parents included — and a structural
  suppression multiset guarantees each source animation appears exactly
  once even when it also rides a verbatim side-channel tree (pattern /
  defs target / `<text>` / `<switch>` / captured `<image>`).
* **Conditional processing** — `<switch>` evaluates
  `requiredExtensions` / `systemLanguage` and renders the first passing
  child; a `parse_with_extras → write_with_extras` round-trip
  re-emits the whole `<switch>` verbatim (every alternative + the
  conditional attributes) rather than freezing the decode-time
  selection, so a re-parse under a different `systemLanguage` re-selects
  correctly. `<view>` definitions + `resolve_fragment` honour both
  bare-name and `svgView(...)` fragment identifiers.
* **`<image>` capture** — the SVG 2 §6 `<image>` element is captured
  into `PreservedExtras::images` as a typed `SvgImage`: inline
  `data:` payloads are base64-decoded (MIME recorded), external URLs
  preserved verbatim (the decoder never fetches). A
  `parse_with_extras → write_with_extras` round-trip is
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
* **Hidden content** — a renderable element carrying an inline
  `display:none` (attribute or its own `style="…"` declaration; CSS 2.1
  §9.2.4) builds no scene node but survives the
  `parse_with_extras → write_with_extras` round-trip verbatim,
  re-emitted at the tail of its parent container. Stylesheet-driven
  `display:none` subtrees still drop (documented limitation).
* **Graceful handling** — `<foreignObject>` contributes no scene node
  (its verbatim capture rides `PreservedExtras`); unknown content
  survives the verbatim round-trip via `PreservedExtras`.

## Text

| Feature | Coverage |
|---|---|
| Paint | `fill` and `stroke` of `<text>`, `<tspan>`, `<textPath>`: colours, `currentColor`, linear / radial gradients and patterns in both `objectBoundingBox` and `userSpaceOnUse` units, the paint-list fallback, `fill-opacity` / `stroke-opacity`, stroke width / caps / joins / dashes, `paint-order`. Bounding-box units bind to the box of the **whole `<text>` element** (union of the glyph cells — advance × em box, §8.10), whichever span carries the paint (§11.1). |
| White space | `xml:space` `default` / `preserve` and `white-space` `normal` / `nowrap` / `pre` / `pre-wrap` / `pre-line` / `break-spaces` (§11.10.3); collapsing works across element boundaries; `pre` / `pre-line` line feeds are forced line breaks stacked by `line-height`; tabs advance to 8-space stops; soft hyphens and bidi controls are not addressable. |
| Positioning | `x` / `y` / `dx` / `dy` / `rotate` lists with units, indexed by addressable character, descendants overriding ancestors, the last `rotate` repeating; `dx` / `dy` accumulate and add to an absolute position on the same character. |
| `textLength` | on `<text>`, `<tspan>` and `<textPath>`, with units; `lengthAdjust="spacing"` distributes the difference between typographic characters (a nested element with its own `textLength` moves as one unit), `spacingAndGlyphs` also stretches the outlines; following text keeps its distance. |
| Anchoring | `text-anchor` per anchored chunk (each absolute `x` / `y`, each line, each `<textPath>`), in the element's `direction`; collapsible space before a repositioned chunk is trimmed as line-end space. |
| Spacing | `letter-spacing` (optional ligatures are not formed across it) and `word-spacing`, lengths in any unit. |
| Baselines | `dominant-baseline` (inherited through the element), `alignment-baseline`, `baseline-shift` (`sub` / `super` / length / percentage, accumulating): alphabetic, ideographic, hanging, mathematical, central, middle, text-top / text-before-edge, text-bottom / text-after-edge. |
| Writing modes | `writing-mode` `horizontal-tb`, `vertical-rl`, `vertical-lr`, `sideways-rl`, `sideways-lr` and the SVG 1.1 `lr` / `rl` / `tb` forms; `text-orientation` `mixed` (vertical scripts upright, others sideways) / `upright` / `sideways`, `glyph-orientation-vertical` `0` / `90`; lines stack right-to-left or left-to-right. |
| Bidirectional text | Unicode bidirectional algorithm per line with `direction` as the base and `unicode-bidi` `embed` / `bidi-override` / `isolate` / `isolate-override` / `plaintext`; right-to-left runs are shaped in logical order, mirrored and placed in visual order. |
| Fonts | `font-family` lists with fallback, `font-size` (lengths, percentages, `em`, the size keywords), `font-weight` (keywords, numbers, `bolder` / `lighter`), `font-style`, `font-variant: small-caps`, the `font` shorthand, `line-height`; all inherited from any ancestor including the root `<svg>` and settable from CSS. |
| Decoration | `text-decoration` / `-line` / `-style` / `-color` and the SVG `text-decoration-fill` / `-stroke`: underline, overline, line-through in `solid` / `double` / `dotted` / `dashed` / `wavy`, painted with the paint (gradients included) of the element that declared them, under the glyphs except line-through; bars follow rotated and on-path glyphs. |
| `<textPath>` | `href` to a `<path>` or a basic shape (its equivalent path) or the `path` attribute, with the target's own `transform` and `pathLength`; `startOffset` (length or percentage, negative and >100 % allowed); `side="right"`; `method="align"` and `method="stretch"` (outlines bent along the path); the single-closed-subpath wrap-around rule; `x` / `dx` along the path and `dy` along its normal; text after the path continues from its end point. |

`inline-size` (§11.4.1) wraps the text: lines break after white space
(which hangs) and between characters of the scripts written without
spaces, stack by `line-height`, and are anchored one by one; only the
first `x` / `y` of the `<text>` positions them. Under `white-space:
pre | nowrap` the text does not wrap and `text-overflow` (§11.10.2.10)
applies to what leaves the area: `clip` clips it, `ellipsis` replaces
the characters that do not fit (horizontal left-to-right text; other
layouts clip).

Not covered: text in a shape (`shape-inside` / `shape-subtract`),
`textPath` `spacing="auto"`, `font-stretch` / `font-size-adjust`,
`text-decoration-thickness` / `text-underline-position`, and colour
bitmap glyphs (the scene graph has no image node; layered colour glyphs
keep their own colours under a plain fill).

## Styling

* **Presentation attributes** — `fill` / `stroke` (named CSS colours,
  `#hex` 3/4/6/8, `rgb()`/`rgba()`, `none`, `url(#id)`, `currentColor`),
  `stroke-width`, line cap / join / miterlimit, dash array / offset,
  opacity family, `fill-rule`, `transform`. `currentColor` is the
  inherited `color` property; `opacity` is per element (not inherited).
  The outermost `<svg>` takes part in the cascade like any element, so
  the icon idiom `<svg fill="none" stroke="currentColor" …>` works.
  `stroke-width` and the dash values take any CSS unit and percentages
  (of the viewport diagonal). A nested `<svg>` and an instantiated
  `<symbol>` clip to their viewport unless `overflow` says otherwise;
  a `<use>` passes its own CSS-matched style down to the instance.
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

## Performance

`benches/pipeline.rs` (dependency-free, `cargo bench --bench pipeline`)
times the stages over four generated document sets and the in-crate
filter evaluators. Before → after the performance pass that followed the
text-layout work, on
one Apple-silicon machine shared with other builds — each figure is the
median of three interleaved rounds of nine runs, built standalone
against the published dependencies:

| Set | Stage | Before | After | |
|---|---|---:|---:|---:|
| icons — 200 stroke icons, 132 KB | `parse` | 3.76 ms | 1.37 ms | 2.7× |
| map — one 8.7 MB document, 18 000 styled paths + 480 `<use>` | XML tree | 10.2 ms | 5.5 ms | 1.8× |
| | `parse` (XML + cascade + geometry + scene) | 411 ms | 50.9 ms | 8.1× |
| | hand-off (`VectorFrame::from`) | 2.3 ms | 2.3 ms | — |
| text — 100 anchored lines, 1 200 words, 300 `<tspan>` | `parse` (incl. shaping + layout) | 81.1 ms | 6.4 ms | 12.7× |
| filters — 400 filtered groups, 12 six-primitive filters | `parse` | 3.49 ms | 0.83 ms | 4.2× |
| filter graph, 8 primitives, 256 × 256 (1×) | evaluate | 5.84 ms | 2.90 ms | 2.0× |
| filter graph, 8 primitives, 1024 × 1024 (4×) | evaluate | 308 ms | 136 ms | 2.3× |
| | — with a 4-thread budget | — | 49.7 ms | 6.2× |

What changed (output unchanged throughout — the filter routines and the
number parser are tested bit for bit against the forms they replace):

* **One cascade pass per element.** An element's matched declarations
  and cascaded state were recomputed for the `display` test, the
  element's branch, its font size, its `d` property and its keyframes;
  they are computed once and shared. The cascade no longer clones the
  whole state per attribute.
* **No allocation to compare names.** Attribute lookup and tag tests
  compare in place; selectors are rejected on their subject's tag / id /
  class before being walked.
* **XML without re-validation.** Names, values and text are sliced from
  the already-valid input instead of re-checking UTF-8 per slice;
  entity decoding copies straight through when there is no `&`.
* **Short decimals parsed exactly.** A path number of at most seven
  significant digits is `mantissa / 10^k` — two exact `f32`s, one
  correctly rounded division.
* **Fonts and glyphs resolved once per document**, not once per text
  run (the 12.7× above is mostly a font no longer being parsed 400
  times).
* **Filters.** The box-blur window sum keeps its tap order but is built
  a row at a time into two swapped buffers; `feMorphology` runs as two
  1-D passes; 8-bit decode is tabulated; graph inputs are borrowed.

Everything is serial by default. The one place a thread budget pays is
the blur: `filter_eval::evaluate_filter_graph_threads(.., threads)` /
`gaussian_blur_edge_threads` split the rows across that many scoped
threads when the caller grants them, with identical output at every
budget. Not taken: a sliding-window blur sum would make the blur
independent of the radius but changes the last bits of the float sums,
so the tap-ordered sum stays.

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
read (`probe` / `info` / `parse` / `decode*`); `write_svgz`,
`write_with(…, &EncodeOptions::new().with_compress(true))` and the
`svgz` muxer produce gzipped output through compcol. Pure Rust, no C
dependencies.

## Robustness

The parse/model surface is hardened against adversarial input — every
public parser returns a typed [`SvgError`] (or a value) and never
panics or aborts, and every unbounded-resource path carries an
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
* **`.svgz` inflation** — gzip input is inflated through compcol's
  capped decoder, bounded by `DecodeOptions::max_bytes` (default
  `parser::MAX_SVGZ_INFLATED`, 128 MiB), so a decompression bomb is
  refused with `LimitExceeded` before its payload is ever materialised.
* **Element count** — `DecodeOptions::max_elements` (default 1 M)
  bounds the XML tree a hostile `<g/><g/>…` stream can allocate.
* **Invalid values** — SVG 2 §4.2: a presentation attribute whose value
  fails to parse takes the property's initial value and a CSS
  declaration with an invalid value is ignored (CSS 2.1 §4.1.8), so a
  stray `fill=""`, `transform="0"` or a number that overflows `f32`
  degrades one property instead of failing the document; `strict`
  mode still rejects malformed XML structure.
* **Reference chains** — gradient/pattern template inheritance and
  `<filter>` `href` inheritance each combine a visited-set cycle guard
  with an eight-hop depth cap.

A curated adversarial corpus plus a seeded byte-mutation fuzzer over the
path/transform/length/paint/XML/document parsers (see
`tests/round403_parser_robustness.rs`) enforce the no-panic invariant,
and three `cargo fuzz` targets under `fuzz/` run it continuously:
`decode` (probe / info / decode* / parse* over raw bytes, default and
tight limits), `write_roundtrip` (the plain writer fixed point,
`.svgz` round-trip and re-parseability of the extras writer's output)
and `text` (a font is installed, so `<text>` / `<textPath>` shape real
glyphs under position lists, `textLength`, vertical and bidirectional
layout, paint servers, decorations and wrapping).

## Not yet supported

* Supplying the standard-input slots `BackgroundImage` / `FillPaint` /
  `StrokePaint` (the graph evaluator defaults them to transparent black
  until the rasteriser supplies them); the eleven extra `feBlend`
  `<blend-mode>` values + the `feComposite` `in` / `out` / `atop` / `xor`
  operators (whose mixing formulae live in the un-staged `[COMPOSITING-1]`
  / `[PORTERDUFF]` companion specs) and `feImage` external-reference
  resolution also remain rasteriser work.
* Marker rendering (`<marker>` definitions and the `marker-*`
  references round-trip; nothing is drawn at the vertices),
  *painting* the captured `<image>` raster into vector space (the
  element itself is parsed + losslessly round-tripped — see **`<image>`
  capture** above), and live pseudo-element / stateful pseudo-class
  evaluation (selectors parse + round-trip; the synthesised-box renderer
  is `oxideav-raster` work).
* Applying a `filter=` to its element: the scene graph has no filter
  node, so a filtered element is handed over unfiltered (the graph is
  parsed and the evaluators above can run it on pixels a rasteriser
  supplies).
* `vector-effect: non-scaling-stroke` (parsed and round-tripped; the
  stroke still scales), text in a shape (`shape-inside`), and the
  `:root` pseudo-class.

Every example of the staged SVG 2 and Filter Effects specifications (99
documents) parses and renders; the ones that differ from the
specification's figures are the filter examples, the marker examples
and the non-scaling-stroke example, for the reasons above, plus two
whose markup is not a document (`<parent>`-rooted XML, an unclosed
`<circle>`).

## License

MIT — see [LICENSE](LICENSE).
