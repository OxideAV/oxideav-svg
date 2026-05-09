# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
