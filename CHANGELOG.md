# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
