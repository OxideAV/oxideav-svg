# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
