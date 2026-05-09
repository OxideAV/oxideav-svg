# oxideav-svg

Pure-Rust SVG (read + write) for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework. No
`xml-rs` / `quick-xml` / `usvg` / `lyon` / `resvg` source — the SVG-
relevant XML subset is small enough for a hand-rolled SAX parser.

## Element subset (round 1)

- `<svg>` — `viewBox`, `width`, `height`, `xmlns`
- `<rect>` — `x`, `y`, `width`, `height`, `rx`, `ry`
- `<circle>` — `cx`, `cy`, `r`
- `<ellipse>` — `cx`, `cy`, `rx`, `ry`
- `<line>` — `x1`, `y1`, `x2`, `y2`
- `<polyline>` / `<polygon>` — `points`
- `<path>` — full `d` mini-language: M/m, L/l, H/h, V/v, C/c, S/s, Q/q,
  T/t, A/a, Z/z
- `<g>` — group with `transform`
- `<linearGradient>` / `<radialGradient>` — `id`, stops, `spreadMethod`

## Attribute subset (round 1)

`fill` / `stroke` (named CSS colors / `#hex` (3/4/6/8) / `rgb()` /
`rgba()` / `none` / `url(#id)` for gradients), `stroke-width`,
`stroke-linecap`, `stroke-linejoin`, `stroke-miterlimit`,
`stroke-dasharray`, `stroke-dashoffset`, `opacity`, `fill-opacity`,
`stroke-opacity`, `fill-rule`, `transform` (`matrix` / `translate` /
`rotate` / `scale` / `skewX` / `skewY`).

## Round 2 additions

- `<text>` / `<tspan>` — vector-first via [`oxideav-scribe`]
  `Shaper::shape_to_paths`. Caller installs a font resolver
  (`oxideav_svg::text::set_font_resolver`) once at startup; each
  `<text>` element looks up a `FaceChain` by `(font-family, font-size)`
  and emits positioned glyph PathNodes wrapped in a Group at the
  text's `(x, y)` origin. Nested `<tspan dx dy x y font-size
  font-family>` updates the running pen + inheritance. Gated behind
  the on-by-default `text` cargo feature; without a registered
  resolver, every `<text>` parses to an empty `Group` so the rest of
  the document still loads.
- `<filter>` graceful pass-through. `<filter id="...">` definitions
  are captured into a side table; `filter="url(#id)"` on elements
  wraps content in an extra `Group` so the structural intent survives
  a parse → encode round-trip. The actual filter graph (Gaussian
  blur, color matrix, …) is rendered by `oxideav-raster` in a later
  round.
- `<mask>` and `<clipPath>` — multi-element masks map to
  `oxideav_core::Node::SoftMask` honouring `mask-type="luminance|
  alpha"`; multi-shape `<clipPath>` collapses children (with their
  per-element `transform=`) into a single concatenated clip
  `oxideav_core::Path` applied to the wrapping group's `clip` field.
  The encoder rewrites both back into `<defs>` blocks with
  auto-generated ids on round-trip.
- Graceful skip for `<foreignObject>` (parsed as empty `Group`),
  `<animate>` / `<animateTransform>` / `<set>` (silently dropped),
  `<symbol>` (captured for the round-3 `<use>` resolver but not yet
  rendered).

## Round 3 additions

- `<use href="#id">` cross-references. The pre-walk captures every
  `id`-bearing element into a documentwide table; `<use>` instantiates
  the referenced shape / group / `<symbol>` as a child node, applying
  the `<use>`'s `x` / `y` (additive translate) and `transform`. Both
  SVG-2 `href` and SVG-1.1 `xlink:href` are accepted. Cycles
  (`use → symbol → use of same id`) are detected and dropped instead
  of recursing infinitely.
- `.svgz` (gzip-compressed SVG, RFC 1952). `parse_svg` and the `svg`
  demuxer transparently sniff the gzip magic (`1f 8b`); the symmetric
  `write_svgz()` helper and a sister `svgz` muxer produce gzipped
  output. Pure-Rust `flate2` backend (`rust_backend`), no C deps.
- `<animate>` / `<set>` / `<animateTransform>` snapshot at `t=0`. The
  animation's `from` value (or first `values` entry, or `to` when
  neither is given) is folded into the parent element's attribute set
  before parsing — produces the same first-paint static rendering
  most browsers show, instead of silently dropping animated content.

## Round 4 additions

- **SMIL animation snapshot at arbitrary `t`** —
  `parse_svg_at(bytes, t_seconds)` evaluates every `<animate>` /
  `<set>` / `<animateTransform>` using the full SMIL timing model:
  `begin`, `dur` (with `s` / `ms` / `min` / `h` / `H:M:S` clock-value
  units), `repeatCount` (numeric or `indefinite`), `keyTimes` +
  `values` segmented interpolation, `from` / `to` / `by` shorthand,
  `calcMode="discrete|linear"`. Colours interpolate componentwise;
  numbers lerp; everything else is discrete. `<animateTransform>`
  serialises to a `transform="..."` attribute for
  `type="translate|rotate|scale"`. The legacy `parse_svg(bytes)`
  retains the round-3 `t=0` first-paint behaviour.
- **Minimal CSS cascade** — `<style>` blocks (with `/* */`
  comments, `@`-rule skipping, CDATA bodies) plus inline
  `style="..."` attributes resolve via tag / class / id selectors with
  CSS2.1 specificity ordering. Cascade applies to `fill`, `stroke`,
  `stroke-width`, `opacity`, `fill-rule`, etc.; unknown properties
  (e.g. `font-family`) are silently ignored rather than failing the
  document. Lives in the new `oxideav_svg::css` module.
- **Encoder preservation** of `<style>` / `<filter>` / `<animate>` /
  `<foreignObject>` via a `PreservedExtras` side-channel. New
  `parse_svg_with_extras(bytes)` returns `(VectorFrame,
  PreservedExtras)`; the symmetric `write_svg_with_extras(frame,
  extras)` re-emits the captured fragments alongside the rasterised
  scene so a `parse → write` round-trip preserves the dynamic /
  filter / CSS definitions. Bare `parse_svg` / `write_svg` retain
  round-3 behaviour.

## Round 5 additions

- **CSS 3 Selectors Level 3 subset** — extends the round-4 cascade
  with attribute predicates (`[attr]`, `[attr=val]`, `[attr~=val]`,
  `[attr|=val]`, `[attr^=val]`, `[attr$=val]`, `[attr*=val]`),
  combinators (descendant ` `, child `>`, adjacent sibling `+`,
  general sibling `~`), and structural pseudo-classes
  (`:first-child`, `:last-child`, `:only-child`, `:nth-child(An+B)`,
  `:first-of-type`, `:last-of-type`, `:only-of-type`,
  `:nth-of-type(An+B)`, `:not(simple)`). Combinator matching is
  right-to-left through a lifetime-tied `MatchContext` ancestor chain
  so deep trees don't allocate per-element scratch Vecs. Unsupported
  pseudo-classes (`:hover`, `:focus`, …) are silently dropped at
  parse time so the rest of the rule still applies.

## Round 6 additions

- **CSS 3 Selectors L3 leftovers** — `:nth-last-child(An+B)`,
  `:nth-last-of-type(An+B)` (1-indexed from the end of the parent's
  element-children list); `:lang(L)` (BCP 47 dash-match against the
  nearest `xml:lang` / `lang` attribute, walked up the ancestor
  chain via the existing `MatchContext` parent pointers).
- **SVG 2 §9.3.2 — `d` as a presentation property**. A `<style>`
  rule (`path { d: "M 0 0 L 10 10" }`) or inline
  `style='d: "..."'` overrides the `d` attribute via the normal
  cascade; the value is `none | <string>`. `d: none` reduces the path
  to a no-render. Wired through a new `parse_path_with_css(el, mctx,
  sheet)` next to the legacy `parse_path(el)`.

## Round 7 additions

- **Typed `<filter>` primitive graph parsing** — `<feGaussianBlur>`,
  `<feOffset>`, `<feFlood>`, `<feComposite>`, `<feBlend>`,
  `<feMorphology>` are walked into a typed `crate::filter::FilterGraph`
  (primitives + per-primitive region + `result` / `in` chaining per
  W3C Filter Effects §6.2). The graph hangs off `defs::FilterDef`
  alongside the verbatim XML, so a downstream rasterizer can consume
  the pipeline without re-parsing. Unknown primitives (e.g.
  `<feColorMatrix>`) survive the verbatim XML round-trip via
  `PreservedExtras`.
- **SMIL `calcMode="paced"`** — redistributes `keyTimes` so each
  segment is traversed at constant attribute-space speed (numeric:
  `|b - a|`; colour: Euclidean RGBA distance; otherwise: uniform
  fallback).
- **SMIL `calcMode="spline"`** — eases each segment through a cubic
  Bézier from `keySplines`; resolved with Newton-Raphson on the x
  curve. Missing / malformed `keySplines` falls back to linear.

## Round 13 additions

- **SMIL animation re-attachment.** Round 4–12 captured every
  `<animate>` / `<set>` / `<animateTransform>` into
  `PreservedExtras::animations` and re-emitted them at the trailing
  edge of the SVG with a `<!-- animation parent: #id -->` comment
  hint. Round 13 inlines each animation as a child of its declared
  parent element when the parent has an `id`, and re-emits the
  original `id="..."` attribute on the matching `<g>` / `<path>` so
  downstream tooling can still address the element by source name.
  A new `PreservedExtras::id_paths` side-channel maps each source
  `id="..."` to the `Vec<usize>` scene-graph tree-path of the
  matching emit site; populated only by `parse_svg_with_extras`.
  Animations whose parent didn't carry an `id` fall back to the
  round-12 trailing-edge emission so no captured fragment is lost.
- **`Stylesheet::resolve_imports(fetcher)`.** Round 11 captured
  `@import url(…)` URLs into `Stylesheet::imports`. Round 13 adds
  a recursive resolver: the caller supplies a
  `Fn(&str) -> Option<Vec<u8>>` (lets the consumer pick HTTP / FS /
  cache); fetched bodies are parsed as CSS and their rules merged
  into `self.rules` so the cascade applies as if the rules were
  inline. Cycle detection (visited-URL set) and an 8-hop depth cap
  (`Stylesheet::IMPORT_DEPTH_CAP`) prevent runaway chains. Failure
  modes — fetcher returns `None`, body isn't UTF-8, parse produces
  no rules — log at `debug` and skip silently.

## Round 12 additions

- **`<script>` graceful capture** — HTML5-style "script data state":
  the parser treats `<script>` content as raw text, so unescaped `<`
  inside the body (e.g. `if (a < b)`) no longer poisons the rest of
  the document. Each `<script>` is captured verbatim into
  `PreservedExtras::scripts` and re-emitted with a `<![CDATA[…]]>`
  wrapping on the encoder side so a subsequent strict-XML round-trip
  still parses. **Scripts are NEVER executed** — oxideav has no JS
  engine.
- **`viewBox` + `preserveAspectRatio` on the root `<svg>`** — SVG 2
  §8.2 specifies how the canvas-vs-viewBox aspect-ratio mismatch maps
  via `preserveAspectRatio`. The decoder applies the spec's
  algorithm (steps 5–14: scale, then `xMin/Mid/MaxYMin/Mid/Max` ×
  `meet`/`slice`) and bakes the resulting translate+scale into
  `frame.root.transform`. A downstream rasteriser that knows nothing
  about `preserveAspectRatio` (one that simply stretches viewBox →
  canvas) therefore still produces the spec-correct visual result.
  `none` (and the aspect-match degenerate case) skip the correction.
  The original keyword pair survives the round-trip via
  `PreservedExtras::root_preserve_aspect_ratio`.

## Round 11 additions

- **`<feImage>` + `<feTile>` close the §11 short-name set.** The
  typed-graph allowlist now covers every short-name primitive (17 of
  17):
  - **`<feImage>`** — `href` (or legacy `xlink:href`),
    `preserveAspectRatio` (full SVG-2 §8.10 keyword set —
    `xMin/Mid/MaxYMin/Mid/Max`, plus `none`, plus optional
    `meet`/`slice` modifier; default `xMidYMid meet`),
    `crossorigin="anonymous|use-credentials"` (HTML CORS attribute).
    Empty `href` is recorded as the empty string per W3C Filter
    Effects §21 (the rasterizer treats it as a transparent-black
    no-op).
  - **`<feTile>`** — only `in=`; the primitive's region (already on
    `FilterPrimitiveNode`) drives the tiled-fill area per §20.
- **CSS pseudo-elements** — `::before`, `::after`, `::first-letter`,
  `::first-line` parse to a typed `PseudoElement` on the carrier
  selector. Per CSS 3 §3.2 a pseudo-element targets a synthesised box;
  a rule with one never matches a live element but survives the
  round-trip for a future renderer. CSS 2.1 §5.12.1 single-colon
  legacy syntax (`:before`, …) is honoured.
- **`@import url(…) [media-query-list];`** per CSS 2.1 §6.3 — the URL
  is appended to `Stylesheet::imports`. Both `url("foo.css")` and
  bare-string (`@import "foo.css";`) forms accepted; loading externals
  is the caller's job (the parser does not fetch network resources).
- **Stateful pseudo-classes** — `:hover`, `:focus`, `:active`,
  `:checked`, `:visited`, `:link`, `:disabled`, `:enabled` parse to a
  typed `Stateful` variant and never match in a static document. This
  fixes a round-5 over-match bug where `.x:hover` collapsed to `.x`
  because `:hover` was silently dropped.

## Round 10 additions

- **Lighting filter primitives** — typed-graph allowlist extended
  from 13 to 15 primitives:
  - **`<feDiffuseLighting>`** — Lambertian-diffuse lighting model
    (W3C Filter Effects §18). Captures `surfaceScale` (default 1),
    `diffuseConstant` (default 1), `kernelUnitLength` (1 or 2
    numbers, mirrored if one), `lighting-color` (CSS colour, default
    opaque white per §21).
  - **`<feSpecularLighting>`** — Phong-specular lighting model
    (§19). Same shared attributes plus `specularConstant` and
    `specularExponent` (both default 1).
  - **`LightSource` enum** — shared by both primitives.
    `<feDistantLight azimuth elevation>` (§18.5),
    `<fePointLight x y z>` (§18.6) and the eight-attribute
    `<feSpotLight x y z pointsAtX pointsAtY pointsAtZ
    specularExponent limitingConeAngle>` (§18.7); `limitingConeAngle`
    is `Option<f32>` so an absent attribute records as "no cone
    clipping". A missing light-source child collapses to a default
    distant light at azimuth=0 / elevation=0.

## Round 9 additions

- **More long-tail filter primitives** — typed-graph allowlist
  extended from 10 to 13 primitives:
  - **`<feConvolveMatrix>`** — `order` (1 or 2 numbers), row-major
    `kernelMatrix` (`order_x * order_y` floats), `divisor` (defaults
    to kernel sum, or 1 if the sum is 0 per W3C Filter Effects §15.2),
    `bias`, `targetX` / `targetY` (default `floor(order/2)`),
    `edgeMode="duplicate|wrap|none"` (`ConvolveEdgeMode` enum;
    default `Duplicate`), `preserveAlpha="true|false"`.
  - **`<feTurbulence>`** — Perlin-noise primitive per Filter Effects
    §16. `baseFrequency` (1 or 2 numbers), `numOctaves` (default 1),
    `seed` (default 0), `stitchTiles="stitch"` flag (default off),
    `type="turbulence|fractalNoise"` (`TurbulenceKind` enum;
    default `Turbulence`).
  - **`<feDisplacementMap>`** — `scale`, `xChannelSelector` /
    `yChannelSelector` (`ChannelSelector` enum, R / G / B / A;
    default `A` per spec §17). `in2` defaults to `SourceGraphic`.

## Round 8 additions

- **Long-tail filter primitives** — typed-graph allowlist extended
  from 6 to 10 primitives:
  - **`<feColorMatrix>`** — `type="matrix"` (4×5 row-major) plus
    `"saturate"`, `"hueRotate"`, `"luminanceToAlpha"`. All variants
    reduce at parse time to a flat 4×5 RGBA-bias matrix using the
    coefficients given in W3C Filter Effects §13.2.4 / §13.2.5 /
    §13.2.6 (luminance 0.213 / 0.715 / 0.072).
  - **`<feMerge>`** — captures the source-order list of
    `<feMergeNode in="..."/>` children. Missing `in=` falls back to
    the previous primitive's `result` per §6.2 / §19.
  - **`<feComponentTransfer>`** — per-channel
    `crate::filter::TransferFunction` resolved from
    `<feFuncR/G/B/A>` children with `type="identity|table|discrete|linear|gamma"`;
    channels lacking a matching child default to `Identity`.
  - **`<feDropShadow>`** — single composite primitive (the syntactic
    sugar for `Gaussian blur + Offset + Flood + Composite` per §22),
    so the rasterizer can implement it directly. Defaults `dx=dy=2`,
    `stdDeviation=2 2`, `flood-color` opaque black, `flood-opacity=1`.

## Deferred to round 14+

- Actual filter-primitive rasterisation (the typed graph is
  pre-rasteriser plumbing; pixel evaluation is `oxideav-raster` work).
- `<marker>` defs + `marker-start` / `marker-mid` / `marker-end`
  (needs a `Marker` construct in `oxideav-core`).
- Live evaluation of pseudo-elements and stateful pseudo-classes (the
  selectors parse + survive the round-trip but a synthesised-box
  renderer is a separate oxideav-raster work-stream).

## Usage

```rust
use oxideav_svg::{parse_svg, write_svg};

let bytes = std::fs::read("icon.svg")?;
let frame = parse_svg(&bytes)?;
let out = write_svg(&frame);
std::fs::write("icon.out.svg", out)?;
```

## Registration

```rust
let mut ctx = oxideav_core::RuntimeContext::new();
oxideav_svg::register(&mut ctx);
```

## Optional text rendering

Round 2 emits glyph PathNodes for `<text>` / `<tspan>` only when a
font resolver is installed. The SVG crate intentionally does not own
a font registry — supply one at startup:

```rust
use oxideav_scribe::{Face, FaceChain};

let dejavu = std::fs::read("DejaVuSans.ttf")?;
oxideav_svg::text::set_font_resolver(move |_family, _size_px| {
    Face::from_ttf_bytes(dejavu.clone()).ok().map(FaceChain::new)
}).ok();
```
