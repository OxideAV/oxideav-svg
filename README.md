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

## Round 291 additions

- **SVG 1.1 §10.9.2 `dominant-baseline` property**
  (`auto | use-script | no-change | reset-size | ideographic |
  alphabetic | hanging | mathematical | central | middle |
  text-after-edge | text-before-edge`) on text content elements
  (`<text>` / `<tspan>` / `<tref>` / `<altGlyph>` / `<textPath>`).
  - New [`crate::element::DominantBaseline`] enum carried on
    [`crate::element::PaintState`]. Initial value
    [`DominantBaseline::Auto`]; the property is **NOT inherited** per
    the §10.9.2 attribute table, so
    [`crate::element::PaintState::merged_with_mctx`] resets it to the
    initial value before applying the element's own attribute
    (matching the round-118 `display` / round-209 `vector-effect` /
    round-257 `overflow` non-inheritance resets — distinct from the
    inherited §13.x rendering hints). The §10.9.2 prose that a child
    run's `auto` "remains the same as the parent text content element"
    is a *baseline-table* layout computation, not a property-value
    inheritance — so the property value itself is non-inherited.
    Resolves through presentation attributes, inline `style="..."`,
    and `<style>`-block rules via the existing round-4 cascade.
    Case-insensitive keyword matching; `inherit` / unknown tokens keep
    the post-reset value.
  - **Round-trip preservation.** New
    [`crate::preserved::DominantBaselineBinding`] +
    [`crate::preserved::PreservedExtras::dominant_baselines`]
    side-channel captures the canonicalised keyword at the topmost
    emit slot for each shape / `<g>` carrying a recognised
    `dominant-baseline=` attribute. The carrier is purely lexical (the
    cascade does not inherit the value), so a hand-authored
    `<text dominant-baseline="hanging">` survives a
    `parse_svg_with_extras → write_svg_with_extras` cycle on the same
    element. The encoder re-emits `dominant-baseline=` on the matching
    shape / `<g>` on round-trip.
  - **Explicit initial value `auto` is preserved.** Mirrors the
    round-221 / round-247 / round-257 explicit-initial-value policy —
    an explicit author `dominant-baseline="auto"` carries intent (e.g.
    an inheritance reset on a child of a
    `<text dominant-baseline="hanging">`); the absent-attribute case is
    still skipped so an initial-value document doesn't bloat with
    redundant `dominant-baseline="auto"` on every element.
  - **Canonical lowercase / hyphenated emission.** §10.9.2 spells
    every keyword all-lowercase (hyphenated for the multi-word
    `use-script` / `no-change` / `reset-size` / `text-after-edge` /
    `text-before-edge`); source `HANGING` / `TEXT-AFTER-EDGE`
    round-trip as `hanging` / `text-after-edge`.
  - The actual scaled-baseline-table construction + glyph positioning
    (§10.9.2 baseline-identifier / baseline-table / baseline-table
    font-size resolution) happen in `oxideav-scribe` / `oxideav-raster`;
    this round delivers parse + non-inherited cascade + round-trip
    preservation. A downstream layout engine reads the resolved value
    off the carried `PaintState` or off the per-element
    `DominantBaselineBinding`.
  - 22 integration tests in `tests/round291_dominant_baseline.rs` cover
    the default value, the no-attribute baseline (no binding), each of
    the twelve §10.9.2 keywords recorded with canonical spelling,
    case-insensitive matching, explicit-`auto` recording, `inherit`
    skipping, unknown-token tolerance, empty-value skipping,
    presentation-attribute / `style="…"` / `<style>`-block cascade
    resolution, non-inheritance through a parent `PaintState`, child
    attribute wins after the per-element reset, round-trip emission on
    `<g>` and on a bare `<rect>`, double round-trip convergence,
    source-case canonicalisation through round-trip, `parse_svg` (no
    extras) still loading, group-records-once-not-per-child,
    per-child-override records-separately, and coexistence with the
    §3.11 `overflow` / §13.10.3 `text-rendering` hints on the same
    group element.

## Round 295 additions

- **Pixel-level `<feComposite>` evaluation** — the second filter
  primitive given an in-crate evaluator, in `crate::filter_eval`
  (Filter Effects Module Level 1 §16 / SVG 1.1 §15.12). Covers the two
  operators the staged specs define **inline**:
  - **`over`** — Porter-Duff `over`, which SVG 1.1 §15.10 gives
    directly via the `normal` blend-mode equivalence: premultiplied
    `cr = (1 − qa)·cb + ca` per colour channel, `qr = 1 − (1 − qa)·
    (1 − qb)`, with `in` as image A (source) and `in2` as image B
    (destination);
  - **`arithmetic`** — `result = k1·i1·i2 + k2·i1 + k3·i2 + k4`,
    clamped to `[0, 1]`, per channel (alpha included), `k1..k4`
    default `0`;
  - `composite(i1, i2, op, k)` operates on two premultiplied
    `FilterImage` operands; `evaluate_composite_node` decodes two
    8-bit sRGB buffers into the node's resolved
    `color-interpolation-filters` space, evaluates, re-encodes. Both
    **decline** `in`/`out`/`atop`/`xor` (formula bodies in the
    un-staged `[PORTERDUFF]` reference) so the rasteriser owns them.
  - 10 unit tests in `filter_eval` pin opaque/transparent/partial-alpha
    `over`, the arithmetic add/product/clamp terms, and the
    operator-decline paths.

## Round 283 additions

- **Pixel-level `<feDropShadow>` evaluation** — the crate's first
  filter-primitive *evaluator* (the typed graph was parse-only; the
  general pixel pipeline remains `oxideav-raster` work). New
  `crate::filter_eval` module implements the W3C Filter Effects
  Module Level 1 §9.12 normative equivalent composite over
  premultiplied-RGBA `FilterImage` buffers:
  - the five §9.12 steps — input alpha → §9.14 Gaussian blur → §9.18
    offset → §9.13 flood composited with the §9.8 Porter-Duff `in`
    operator → §9.16 merge (`over`, input on top) — with steps 3–5
    fused into one pass (§9.12 permits not materialising the tree);
  - `gaussian_blur` is the exact §9.14 three-box-blur approximation
    (`d = floor(s·3·sqrt(2π)/4 + 0.5)`; odd `d` → three centred
    boxes; even `d` → left-boundary + right-boundary boxes of size
    `d` + centred box of size `d+1`), `edgeMode` `none` zero
    extension, per-axis zero blurring the other axis only, negative
    `stdDeviation` disabling the primitive;
  - `offset` resolves fractional `dx`/`dy` with bilinear
    interpolation per the §9.18 recommendation;
  - the working colour space follows the node's resolved
    `color-interpolation-filters` (§10: initial `linearRGB`, `auto`
    resolves to it) using the SVG 2 §13.9 sRGB ↔ linear transfer
    (`srgb_to_linear` / `linear_to_srgb`);
  - `evaluate_drop_shadow_node` runs a parsed `FilterPrimitiveNode`
    end-to-end over an 8-bit RGBA buffer; `DropShadowParams::default`
    carries the §9.12 initial values (`dx=dy=2`, `stdDeviation=2`,
    opaque-black flood, opacity 1).
  - 21 tests pin rendered output bytes hand-derived from the spec
    maths (even-`d` impulse kernel `[1/12, 1/4, 1/3, 1/4, 1/12]` at
    `s=0.8`, default `s=2` centre weight `0.175²`, kernel
    mass/symmetry, flood colour×opacity, merge-over in both working
    spaces, fractional offsets) in
    `tests/round283_drop_shadow_eval.rs`.

## Round 279 additions

- **`<filter>` element attribute set completed on the typed graph —
  SVG 1.1 §15.3 `filterRes` + `xlink:href`** (the two attributes of
  the §15.3 attribute list still missing after round 272's
  `filterUnits` / `primitiveUnits` / `color-interpolation-filters`):
  - **`filterRes`** (§15.5) — new `crate::filter::FilterRes`
    (`x_pixels` / `y_pixels`) on `FilterGraph::filter_res`. Per §15.5
    non-integer values are truncated toward zero at parse time and a
    single `<number-optional-number>` value expands to both axes;
    negative (error) and zero (disables rendering of the referencing
    element) values are captured as-is for the rasteriser to enforce.
    Absent / non-numeric → `None` ("the user agent will use
    reasonable values").
  - **`xlink:href` / SVG 2 `href`** — captured `#`-stripped on
    `FilterGraph::href`; new
    `crate::filter::resolve_filter_element_chain` implements the
    §15.3 inheritance: attributes defined on the referenced
    `<filter>` but not on this one are merged in (nearest chain
    definition wins; `id` / the reference itself never inherit), and
    an element with no filter nodes inherits the filter nodes of the
    nearest chain member that has any (unknown `fe*` children count
    as filter nodes per the §15.3 content model). Indirect chains
    resolve up to an eight-hop cap mirroring the §14.1.1 gradient
    template walker; cycles / self-references / dangling ids
    terminate gracefully. The decoder resolves every captured
    `FilterDef::graph` after the defs pre-walk (forward references
    work); `FilterDef::element` stays the verbatim source so
    round-trip emission is untouched.
  - 23 integration tests in `tests/round279_filter_href.rs`. The
    typed `<filter>` element surface now matches the §15.3 attribute
    list; the pixel pipeline remains `oxideav-raster` work.

## Round 272 additions

- **`<filter>` coordinate-system + colour-space attributes** —
  rounds 7–11 typed every filter primitive, but the `<filter>`
  element's own `filterUnits` / `primitiveUnits` /
  `color-interpolation-filters` were dropped at parse time. They are
  now captured on the typed `crate::filter::FilterGraph` /
  `FilterPrimitiveNode`:
  - **`filterUnits` / `primitiveUnits`** — new `FilterUnits` enum
    (`UserSpaceOnUse` / `ObjectBoundingBox`). Per SVG 1.1 §15.7.2 the
    two attributes have *different* defaults: `filterUnits` →
    `objectBoundingBox`, `primitiveUnits` → `userSpaceOnUse`; unknown
    values fall back to those.
  - **`color-interpolation-filters`** — new
    `ColorInterpolationFilters` enum (`Auto` / `Srgb` / `LinearRgb`),
    `Default` = `LinearRgb`. Per SVG 1.1 §11.7.1 the property is
    inherited, applies to filter primitives, and has initial value
    `linearRGB` (distinct from `color-interpolation`'s `sRGB`). The
    `<filter>`-level value is stored on the graph; each primitive's
    resolved value (own attribute → filter-inherited → initial
    `linearRGB`) lands on `FilterPrimitiveNode`. `inherit` with no
    cascade context collapses to the initial value. This closes the
    round-4/round-10 note that `color-interpolation-filters` was
    documented but not represented in the typed graph.
  - All three attributes still survive the verbatim XML round-trip.

## Round 261 additions

- **SVG 1.1 §16.8.2 `cursor` property**
  (`[ [<funciri> ,]* [ auto | crosshair | default | pointer | move |
  e-resize | ne-resize | nw-resize | n-resize | se-resize |
  sw-resize | s-resize | w-resize | text | wait | help ] ] |
  inherit`) on the §16.8.2 applies-to set (container elements,
  graphics elements). SVG 2 retains `cursor` as a presentation
  attribute and defers the property definition to CSS; the SVG 1.1
  §16.8.2 definition carries the keyword set + grammar implemented
  here.
  - New [`crate::element::CursorKeyword`] (sixteen generic keywords)
    + [`crate::element::CursorValue`] (funciri list + mandatory
    trailing generic keyword) carried on
    [`crate::element::PaintState`]. Per the §16.8.2 attribute table
    the initial value is `auto` and the property IS inherited
    (matching the round-260 `pointer-events` inherited-cascade flow;
    distinct from the round-118 `display` / round-209
    `vector-effect` / round-257 `overflow` non-inherited resets).
    Resolves through presentation attributes, inline `style="..."`,
    and `<style>`-block rules via the existing round-4 cascade.
    Case-insensitive keyword matching; `inherit` / invalid payloads
    keep the inherited value.
  - **`<funciri>` list grammar.** Zero or more comma-separated
    `url(...)` custom-cursor references precede the generic keyword;
    per §16.8.2 "if the user agent cannot handle any user-defined
    cursor, it must use the generic cursor at the end of the list",
    so a funciri list *without* a trailing generic keyword is
    invalid (inherited value kept). The list splits on top-level
    commas only — an IRI containing commas (e.g. a `data:` IRI)
    stays one item per the `<funciri>` production
    (`url(` wsp* IRI wsp* `)`). A `<funciri>` may target an SVG 1.1
    §16.8.3 `<cursor>` element; the reference round-trips verbatim
    (typed `<cursor>` element capture is a follow-up).
  - **Round-trip preservation.** New
    [`crate::preserved::CursorBinding`] +
    [`crate::preserved::PreservedExtras::cursors`] side-channel
    captures the canonicalised value at the topmost emit slot for
    each shape / `<g>` carrying a recognised `cursor=` attribute,
    mirroring the round-260 lexical carrier. Canonical form: `url`
    tokens lowercased with the IRI preserved verbatim (IRIs are
    case-significant), items joined comma-and-space, generic keyword
    lowercased — `URL( #c ) , POINTER` round-trips as
    `url(#c), pointer`. Explicit `cursor="auto"` is preserved per
    the round-221 .. round-260 explicit-initial-value policy.
  - **Coexists with §15.6 `pointer-events` + §3.11 `overflow`** on
    the same `<g>` — orthogonal interactivity / clipping properties,
    independent side-channels, every recognised attribute re-emits
    on round-trip.
  - The actual cursor display (funciri resolution + generic fallback
    walk per §16.8.2) is interactive-UA work (a windowing host
    embedding `oxideav-pipeline`); this round delivers parse +
    inherited cascade + round-trip preservation. A downstream
    consumer reads the resolved value off the carried `PaintState`
    or off the per-element `CursorBinding`.
  - 27 integration tests in `tests/round261_cursor.rs` cover the
    default value, the no-attribute baseline (no binding), all
    sixteen §16.8.2 keywords recorded with canonical spelling,
    case-insensitive matching, explicit-`auto` recording, `inherit`
    skipping, unknown-token tolerance, empty-value skipping, the
    funciri + keyword list form, multi-funciri whitespace
    canonicalisation, top-level comma splitting (`data:` IRI with
    internal commas), the funciri-without-keyword and
    non-funciri-item invalid forms, presentation-attribute /
    `style="…"` / `<style>`-block cascade resolution, inheritance
    through a parent `PaintState`, child attribute overrides,
    round-trip emission on `<g>` and on a bare `<rect>` (funciri
    list included), double round-trip convergence, source-case
    canonicalisation, `parse_svg` (no extras) still loading,
    group-records-once-not-per-child, per-child-override-records-
    separately, and coexistence with `pointer-events` / `overflow`
    on the same group element.

## Round 260 additions

- **SVG 2 §15.6 `pointer-events` property**
  (`bounding-box | visiblePainted | visibleFill | visibleStroke |
  visible | painted | fill | stroke | all | none`) on the §15.6
  applies-to set (container elements, graphics elements, `<use>`).
  - New [`crate::element::PointerEvents`] enum carried on
    [`crate::element::PaintState`]. Per the §15.6 attribute table
    the initial value is `VisiblePainted`; the property IS inherited
    per the same table, so
    [`crate::element::PaintState::merged_with_mctx`] does NOT reset
    `pointer_events` before applying the element's own attribute
    (matching the §13.x rendering-hint inherited-cascade flow;
    distinct from the round-118 `display` / round-209
    `vector-effect` / round-257 `overflow` non-inherited resets).
    Resolves through presentation attributes, inline `style="..."`,
    and `<style>`-block rules via the existing round-4 cascade.
    Case-insensitive keyword matching; `inherit` / unknown tokens
    keep the inherited value.
  - **Round-trip preservation.** New
    [`crate::preserved::PointerEventsBinding`] +
    [`crate::preserved::PreservedExtras::pointer_eventss`]
    side-channel captures the canonicalised keyword at the topmost
    emit slot for each shape / `<g>` carrying a recognised
    `pointer-events=` attribute. Mirrors the round-247 / round-252 /
    round-257 lexical carriers — the property cascades through
    `PaintState`, but the binding only records the source emit slot
    so a hand-authored `<g pointer-events="none">` survives a
    `parse_svg_with_extras → write_svg_with_extras` cycle on the
    same group element. The encoder re-emits `pointer-events=` on
    the matching shape / `<g>` on round-trip.
  - **Explicit initial value `visiblePainted` is preserved.**
    Mirrors the round-221 / round-247 / round-252 / round-257
    explicit-initial-value policy — even though `visiblePainted` is
    the §15.6 initial value, an explicit author write carries
    intent (e.g. an inheritance reset on a descendant of a
    `<g pointer-events="none">`). The absent-attribute case is
    still skipped so an initial-value document doesn't bloat with
    redundant `pointer-events="visiblePainted"` on every element.
  - **Canonical mixed-spelling emission.** §15.6 spells the keyword
    set with three conventions: lower-camelCase for the four
    `visible*` keywords (`visiblePainted` / `visibleFill` /
    `visibleStroke`), a hyphen for `bounding-box`, and all-lowercase
    for the remainder. Source `VISIBLEPAINTED` / `BOUNDING-BOX` /
    `Painted` round-trip as the canonical §15.6 spelling.
  - **Coexists with §3.11 `overflow` and the §13.x rendering /
    colour hints.** §15.6 (hit-test gate) is orthogonal to §3.11
    (clipping rectangle), §13.9 (working colour space) and §13.10.x
    (rendering-quality hints) — they can all ride on the same `<g>`
    without interfering. Each side-channel records independently
    and the encoder emits every recognised attribute on round-trip.
  - The actual hit-test gating (the §15.6 visibility + paint suffix
    resolution that decides whether a pointer over the element
    counts as a hit) happens in the interactive layer (e.g.
    `oxideav-pipeline` event routing or `oxideav-raster` hit-test
    queries against the rendered scene); this round delivers parse
    + inherited cascade + round-trip preservation. A downstream
    consumer reads the resolved value off the carried `PaintState`
    or off the per-element `PointerEventsBinding`.
  - 23 integration tests in `tests/round260_pointer_events.rs`
    cover the default value, the no-attribute baseline (no
    binding), each of the ten §15.6 keywords recorded with
    canonical spelling, case-insensitive matching across all three
    spelling conventions, explicit-`visiblePainted` recording
    (initial value preserved), `inherit` skipping, unknown-token
    tolerance, empty-value skipping, presentation-attribute /
    `style="…"` cascade resolution, inheritance through a parent
    `PaintState`, child attribute overrides inherited value,
    round-trip emission on `<g>` and on a bare `<rect>`, double
    round-trip convergence, source-case canonicalisation through
    round-trip for both lower-camelCase and hyphenated keywords,
    `parse_svg` (no extras) still loading the document, the
    group-records-once-not-per-child pattern, coexistence with the
    round-221 / round-228 / round-247 / round-252 / round-257
    hints on the same group element, the per-child override
    records-separately pattern, and a CSS-block-rule cascade
    smoke-test.

## Round 257 additions

- **SVG 2 §3.11 `overflow` property**
  (`visible | hidden | scroll | auto`) on the §3.11 summary-table
  element list (`<svg>` / `<symbol>` / `<marker>` / `<pattern>` /
  `<image>` / `<text>` / `<iframe>` / `<foreignObject>`).
  - New [`crate::element::Overflow`] enum carried on
    [`crate::element::PaintState`]. Per the §3.11 summary table
    the initial value is `Visible`; the property is NOT inherited
    per CSS 2.1 §11.1.1 (matching the round-118 `display` and
    round-209 `vector-effect` non-inheritance reset policy), so
    [`crate::element::PaintState::merged_with_mctx`] resets
    `overflow` to the initial value before applying the element's
    own attribute. Resolves through presentation attributes,
    inline `style="..."`, and `<style>`-block rules via the
    existing round-4 cascade. Case-insensitive keyword matching;
    `inherit` / unknown tokens keep the post-reset value (per the
    §3.11 normative tolerance note: "as `overflow="invalid"` will
    result in a rule setting overflow to visible").
  - **Round-trip preservation.** New
    [`crate::preserved::OverflowBinding`] +
    [`crate::preserved::PreservedExtras::overflows`] side-channel
    captures the canonicalised lowercase keyword at the topmost
    emit slot for each shape / `<g>` carrying a recognised
    `overflow=` attribute. The carrier is purely lexical (the
    cascade itself does not inherit `overflow`), so a
    hand-authored `<g overflow="hidden">` survives a
    `parse_svg_with_extras → write_svg_with_extras` cycle on the
    same group element even though the cascade would have already
    reset descendants to `visible`. The encoder re-emits
    `overflow=` on the matching shape / `<g>` on round-trip.
  - **Explicit initial value `visible` is preserved.** Mirrors the
    round-221 / round-247 / round-252 explicit-initial-value
    policy — even though `visible` is the §3.11 initial value, an
    explicit author write carries intent (e.g. an override of the
    UA-stylesheet `overflow: hidden` default that fires for
    non-root `<svg>` / `<symbol>` / `<marker>` / `<pattern>` /
    `<image>` per §3.11). The absent-attribute case is still
    skipped so an initial-value document doesn't bloat with
    redundant `overflow="visible"` on every element.
  - **Canonical lowercase emission.** Source `HIDDEN` / `Hidden`
    / `SCROLL` round-trip as `hidden` / `scroll` — §3.11 reuses
    the CSS 2.1 keyword set verbatim, all lowercase (distinct
    from the §13.9 mixed-case spellings `sRGB` / `linearRGB` and
    the §13.10.x lower-camelCase spellings).
  - **Coexists with the §13.x rendering / colour hints.** §3.11
    (clipping rectangle), §13.10.1 `color-rendering` (quality
    hint), and §13.9 `color-interpolation` (working colour space)
    are orthogonal properties — they can all ride on the same
    `<g>` without interfering. Each side-channel records
    independently and the encoder emits every recognised
    attribute on round-trip.
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
  - 21 integration tests in `tests/round257_overflow.rs` cover the
    default value, the no-attribute baseline (no binding), each
    of the four §3.11 keywords recorded with canonical case,
    case-insensitive matching, explicit-`visible` recording
    (initial value preserved), `inherit` skipping, unknown-token
    tolerance, empty-value skipping, presentation-attribute /
    `style="…"` cascade resolution, non-inheritance through a
    parent `PaintState`, child attribute wins after the
    per-element reset, round-trip emission on `<g>` and on a bare
    `<rect>`, double round-trip convergence, source-case
    canonicalisation through round-trip, `parse_svg` (no extras)
    still loading the document, the
    group-records-once-not-per-child pattern, coexistence with the
    round-221 / round-228 / round-247 / round-252 hints on the
    same group element, and the per-child override
    records-separately pattern.

## Round 252 additions

- **SVG 2 §13.9 `color-interpolation` property**
  (`auto | sRGB | linearRGB`) on container, graphics, and gradient
  elements (plus `<use>` and `<animate>` per the §13.9 applies-to list).
  - New [`crate::element::ColorInterpolation`] enum carried on
    [`crate::element::PaintState`]. Inherited per the §13.9 attribute
    table; initial value `Srgb` — distinct from the §13.10.x rendering
    hints whose initial value is `Auto`. Resolves through presentation
    attributes, inline `style="..."`, and `<style>`-block rules via the
    existing round-4 cascade. Case-insensitive keyword matching;
    `inherit` / unknown tokens keep the inherited value (matches the
    tolerant policy of `color-rendering` / `image-rendering` /
    `text-rendering` / `shape-rendering` / `text-anchor` /
    `paint-order` / `visibility`).
  - **Round-trip preservation.** New
    [`crate::preserved::ColorInterpolationBinding`] +
    [`crate::preserved::PreservedExtras::color_interpolations`]
    side-channel captures the canonicalised §13.9 mixed-case keyword
    at the topmost emit slot for each shape / `<g>` carrying a
    recognised `color-interpolation=` attribute. A
    `<g color-interpolation=…>` ancestor records on the group's own
    slot (one binding per source-attribute slot — not per cascaded
    descendant). The encoder re-emits `color-interpolation=` on the
    matching element on round-trip.
  - **Explicit initial value `sRGB` is preserved.** Mirrors the
    round-247 / round-235 / round-228 / round-221 explicit-initial-
    value policy — even though `sRGB` is the §13.9 initial value, an
    explicit author write carries intent (e.g. an inheritance reset
    on a descendant of a `<g color-interpolation="linearRGB">`). The
    absent-attribute case is still skipped so an initial-value
    document doesn't bloat with redundant
    `color-interpolation="sRGB"` on every element. Explicit `auto` is
    recorded by the same rationale.
  - **Canonical mixed-case emission.** Source `SRGB` / `srgb` /
    `LINEARRGB` / `linearrgb` all round-trip as the §13.9 spelling
    (`sRGB` / `linearRGB`). Distinct from the lower-camelCase
    canonicalisation used for the §13.10.x rendering hints — §13.9
    is the only §13.x property whose attribute-table spelling uses
    mixed case for non-`auto` keywords.
  - **Coexists with the §13.10.x rendering hints.** §13.9 (working
    colour space selector) and §13.10.1 `color-rendering` (quality
    hint) are orthogonal properties — both can ride on the same `<g>`
    without interfering. Each side-channel records independently and
    the encoder emits every recognised attribute on round-trip.
  - The actual working-colour-space selection (sRGB vs linearised
    RGB for gradient stop interpolation, SMIL colour animation, and
    graphics-element compositing) happens in `oxideav-raster`; this
    round delivers parse + inherited cascade + round-trip
    preservation. A downstream rasteriser reads the resolved value
    off the carried `PaintState` or off the per-element
    `ColorInterpolationBinding`. The §13.9 informative note that the
    filter-effects sibling property `color-interpolation-filters`
    governs the filter primitive graph instead is now captured in the
    typed graph as of round 272 (see `crate::filter`); pixel-space
    colour conversion remains `oxideav-raster` work.
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

## Round 247 additions

- **SVG 2 §13.10.1 `color-rendering` property**
  (`auto | optimizeSpeed | optimizeQuality`) on container, graphics,
  and gradient elements (plus `<use>` and `<animate>` per the §13.10.1
  applies-to list).
  - New [`crate::element::ColorRendering`] enum carried on
    [`crate::element::PaintState`]. Inherited per the §13.10.1
    attribute table; initial value `Auto`. Resolves through
    presentation attributes, inline `style="..."`, and
    `<style>`-block rules via the existing round-4 cascade.
    Case-insensitive keyword matching; `inherit` / unknown tokens
    keep the inherited value (matches the tolerant policy of
    `image-rendering` / `text-rendering` / `shape-rendering` /
    `text-anchor` / `paint-order` / `visibility`).
  - **Round-trip preservation.** New
    [`crate::preserved::ColorRenderingBinding`] +
    [`crate::preserved::PreservedExtras::color_renderings`]
    side-channel captures the canonicalised camelCase keyword at the
    topmost emit slot for each shape / `<g>` carrying a recognised
    `color-rendering=` attribute. A `<g color-rendering=…>` ancestor
    records on the group's own slot (one binding per source-attribute
    slot — not per cascaded descendant), so a hand-authored grouping
    attribute survives a `parse_svg_with_extras → write_svg_with_extras`
    cycle. The encoder re-emits `color-rendering=` on the matching
    element on round-trip.
  - **Explicit `auto` is preserved.** Mirrors the round-221
    `shape-rendering` / round-228 `text-rendering` / round-235
    `image-rendering` policy — an explicit author
    `color-rendering="auto"` is recorded because it carries author
    intent (e.g. an inheritance reset on a descendant of a
    `<g color-rendering="optimizeQuality">`). The absent-attribute
    case is still skipped so an initial-value document doesn't bloat
    the output with redundant `color-rendering="auto"` on every
    element.
  - **Canonical camelCase emission.** Source `OPTIMIZEQUALITY` /
    `optimizequality` / `OptimizeQuality` all round-trip as
    `optimizeQuality`, matching the §13.10.1 attribute table's
    spelling.
  - **Coexists with the other rendering hints.** All four §13.10.x
    inherited hints (`color-rendering` / `shape-rendering` /
    `text-rendering` / `image-rendering`) can ride on the same `<g>`
    without interfering; each side-channel records independently and
    the encoder emits every recognised attribute faithfully.
  - The actual working-colour-space selection (device RGB vs
    linear RGB vs a wider working space) for colour interpolation
    and compositing happens in `oxideav-raster`; this round delivers
    parse + inherited cascade + round-trip preservation. A
    downstream rasteriser reads the resolved value off the carried
    `PaintState` or off the per-element `ColorRenderingBinding`. The
    §13.10.1 informative note that `color-rendering` takes precedence
    over the filter-effects `color-interpolation-filters` property is
    not enforced here — `color-interpolation-filters` is captured in
    the typed filter primitive graph as of round 272 (see
    `crate::filter`), and the precedence is something the raster-side
    composer enforces.
  - 21 integration tests in `tests/round247_color_rendering.rs`
    cover the no-attribute baseline (no binding), each of the three
    spec keywords recorded with canonical camelCase,
    case-insensitive matching, explicit-`auto` recording, `inherit`
    skipping, unknown-token tolerance, empty-value skipping,
    presentation-attribute / `style="…"` cascade resolution,
    inheritance through a parent `PaintState`, child override of the
    inherited value, round-trip emission on `<g>` and on a bare
    `<rect>`, double round-trip convergence, source-case
    canonicalisation through round-trip, `parse_svg` (no extras)
    still loading the document, the per-child-override-records-
    separately pattern, and coexistence with the round-221 /
    round-228 / round-235 hints on the same group element.

## Round 235 additions

- **SVG 2 §13.10.4 `image-rendering` property**
  (`auto | optimizeQuality | optimizeSpeed`) on `<image>` and (via
  the cascade) any descendant element that paints raster content.
  - New [`crate::element::ImageRendering`] enum carried on
    [`crate::element::PaintState`]. Inherited per the §13.10.4
    attribute table; initial value `Auto`. Resolves through
    presentation attributes, inline `style="..."`, and
    `<style>`-block rules via the existing round-4 cascade.
    Case-insensitive keyword matching; `inherit` / unknown tokens
    keep the inherited value (matches the tolerant policy of
    `text-rendering` / `shape-rendering` / `text-anchor` /
    `paint-order` / `visibility`).
  - **Round-trip preservation.** New
    [`crate::image::SvgImage::image_rendering`] field captures the
    canonicalised camelCase keyword off each source `<image>`
    element carrying a recognised `image-rendering=` attribute. The
    `<image>` is captured into
    [`crate::preserved::PreservedExtras::images`] and the encoder
    re-emits `image-rendering=` on the matching `<image>` on
    round-trip — the §13.10.4 property applies to images, so the
    natural emit site is the image itself rather than a separate
    side-channel table.
  - **Explicit `auto` is preserved.** Mirrors the round-221
    `shape-rendering` / round-228 `text-rendering` policy — an
    explicit author `image-rendering="auto"` is recorded because it
    carries author intent (e.g. an inheritance reset on a
    descendant of a `<g image-rendering="optimizeSpeed">`). The
    absent-attribute case is still skipped so an initial-value
    document doesn't bloat the output with redundant
    `image-rendering="auto"` on every `<image>`.
  - **Canonical camelCase emission.** Source `OPTIMIZEQUALITY` /
    `optimizequality` / `OptimizeQuality` all round-trip as
    `optimizeQuality`, matching the §13.10.4 attribute table's
    spelling.
  - **Coexists with `shape-rendering`.** Both inherited hints can
    ride on the same subtree without interfering; the
    `shape-rendering` side-channel (round 221) and the per-image
    `image_rendering` slot record independently and the encoder
    emits both attributes faithfully.
  - The actual resampling-algorithm selection (nearest-neighbour,
    bilinear, …) happens in `oxideav-raster`; this round delivers
    parse + inherited cascade + round-trip preservation. A
    downstream rasteriser reads the resolved value off the carried
    `PaintState` or off the per-image `SvgImage::image_rendering`
    field.
  - 18 integration tests in `tests/round235_image_rendering.rs`
    cover the no-attribute baseline (no binding), each of the
    three spec keywords recorded with canonical camelCase,
    case-insensitive matching, explicit-`auto` recording, `inherit`
    skipping, unknown-token tolerance, empty-value skipping,
    presentation-attribute / `style="…"` cascade resolution,
    inheritance through a parent `PaintState`, child override of
    the inherited value, round-trip emission on `<image>`, double
    round-trip convergence, source-case canonicalisation through
    round-trip, `parse_svg` (no extras) still loading the document,
    and coexistence with the round-221 `shape-rendering` attribute
    on a sibling subtree.

## Round 228 additions

- **SVG 2 §13.10.3 `text-rendering` property**
  (`auto | optimizeSpeed | optimizeLegibility | geometricPrecision`)
  on `<text>` and (via the cascade) descendant `<tspan>` /
  `<textPath>` runs.
  - New [`crate::element::TextRendering`] enum carried on
    [`crate::element::PaintState`]. Inherited per the §13.10.3
    attribute table; initial value `Auto`. Resolves through
    presentation attributes, inline `style="..."`, and
    `<style>`-block rules via the existing round-4 cascade.
    Case-insensitive keyword matching; `inherit` / unknown tokens
    keep the inherited value (matches the tolerant policy of
    `text-anchor` / `paint-order` / `visibility` /
    `shape-rendering`).
  - **Round-trip preservation.** New
    [`crate::preserved::TextRenderingBinding`] +
    [`crate::preserved::PreservedExtras::text_renderings`]
    side-channel captures the canonicalised camelCase keyword
    string at the topmost emit slot for each `<text>` / `<g>`
    carrying a recognised `text-rendering=` attribute. A `<g
    text-rendering=…>` ancestor records on the group's own slot
    (one binding per source-attribute slot — not per cascaded
    descendant), so a hand-authored grouping attribute survives a
    `parse_svg_with_extras → write_svg_with_extras` cycle. The
    encoder re-emits `text-rendering=` on the matching element on
    round-trip.
  - **Explicit `auto` is preserved.** Mirrors the round-221
    `shape-rendering` policy — an explicit author
    `text-rendering="auto"` is recorded because it carries author
    intent (e.g. an inheritance reset on a descendant of a `<g
    text-rendering="optimizeLegibility">`). The absent-attribute
    case is still skipped so an initial-value document doesn't
    bloat the output with redundant `text-rendering="auto"` on
    every `<text>`.
  - **Canonical camelCase emission.** Source `OPTIMIZELEGIBILITY` /
    `optimizelegibility` / `OptimizeLegibility` all round-trip as
    `optimizeLegibility`, matching the §13.10.3 attribute table's
    spelling.
  - **Coexists with `shape-rendering`.** Both inherited hints can
    ride on the same `<g>` without interfering; the two
    side-channels (round-221 and round-228) record independently
    and the encoder emits both attributes on the same element.
  - The actual rendering-hint consumption (anti-alias toggle,
    hint suspension) happens in `oxideav-raster` / `oxideav-scribe`;
    this round delivers parse + inherited cascade + round-trip
    preservation. A downstream rasteriser reads the resolved value
    off the carried `PaintState` or off the per-element
    `TextRenderingBinding`.
  - 20 integration tests in `tests/round228_text_rendering.rs`
    cover the no-attribute baseline (no binding), each of the four
    spec keywords recorded with canonical camelCase,
    case-insensitive matching, explicit-`auto` recording,
    `inherit` skipping, unknown-token tolerance, empty-value
    skipping, presentation-attribute / `style="…"` cascade
    resolution, inheritance through a `<g>` ancestor, child
    override of the inherited value, round-trip emission on `<g>`,
    double round-trip convergence, `parse_svg` (no extras) still
    loading the document, the per-child-override-records-separately
    pattern, and coexistence with the round-221 `shape-rendering`
    attribute on the same group element.

## Round 221 additions

- **SVG 2 §13.10.2 `shape-rendering` property**
  (`auto | optimizeSpeed | crispEdges | geometricPrecision`) on
  shapes.
  - New [`crate::element::ShapeRendering`] enum carried on
    [`crate::element::PaintState`]. Inherited per the §13.10.2
    attribute table; initial value `Auto`. Resolves through
    presentation attributes, inline `style="..."`, and
    `<style>`-block rules via the existing round-4 cascade.
    Case-insensitive keyword matching; `inherit` / unknown tokens
    keep the inherited value (matches the tolerant policy of
    `text-anchor` / `paint-order` / `visibility`).
  - **Round-trip preservation.** New
    [`crate::preserved::ShapeRenderingBinding`] +
    [`crate::preserved::PreservedExtras::shape_renderings`]
    side-channel captures the canonicalised camelCase keyword string
    at the topmost emit slot for each shape / `<g>` carrying a
    recognised `shape-rendering=` attribute. A `<g shape-rendering=…>`
    ancestor records on the group's own slot (one binding per source
    attribute slot — not per cascaded descendant), so a hand-authored
    grouping attribute survives a
    `parse_svg_with_extras → write_svg_with_extras` cycle. The
    encoder re-emits `shape-rendering=` on the matching shape / `<g>`
    on round-trip.
  - **Explicit `auto` is preserved.** Unlike the round-205
    `paint-order` / round-209 `vector-effect` capturers (which skip
    the initial value to avoid no-op binding bloat), an explicit
    author `shape-rendering="auto"` is recorded — it carries author
    intent (e.g. an inheritance reset on a descendant of a `<g
    shape-rendering="optimizeSpeed">`). The absent-attribute case is
    still skipped so an initial-value document doesn't bloat the
    output with redundant `shape-rendering="auto"` on every shape.
  - **Canonical camelCase emission.** Source `OPTIMIZESPEED` /
    `optimizespeed` / `OptimizeSpeed` all round-trip as
    `optimizeSpeed`, matching the §13.10.2 attribute table's
    spelling.
  - The actual rendering-hint consumption (anti-alias toggle, edge
    snap) happens in `oxideav-raster`; this round delivers parse +
    inherited cascade + round-trip preservation. A downstream
    rasteriser reads the resolved value off the carried `PaintState`
    or off the per-shape `ShapeRenderingBinding`.
  - 18 integration tests in `tests/round221_shape_rendering.rs` cover
    the no-attribute baseline (no binding), each of the four spec
    keywords recorded with canonical camelCase, case-insensitive
    matching, explicit-`auto` recording, `inherit` skipping,
    unknown-token tolerance, empty-value skipping, the presentation-
    attribute and `style="..."` cascade lanes, the inheritance
    through a `<g>` ancestor, round-trip emission on `<rect>` /
    `<path>` / `<g>`, double round-trip convergence, `parse_svg` (no
    extras) still loading the document, and the per-child-override-
    records-separately pattern.

## Round 215 additions

- **SVG 1.1 §14.3.5 `clip-rule` property** (`nonzero | evenodd |
  inherit`) on graphics elements within a `<clipPath>` element.
  - New typed [`crate::defs::ClipPathDef::clip_rule`] exposes the
    resolved [`oxideav_core::FillRule`] for the merged-path
    representation. Initial value [`oxideav_core::FillRule::NonZero`]
    per the §14.3.5 attribute table.
  - **Inheritance + override** — `clip-rule` is an inherited
    presentation property per §14.3.5, so a value on the `<clipPath>`
    element itself cascades to its shape children. A per-shape
    `clip-rule=` overrides the inherited value (the spec's worked
    example: `<clipPath clip-rule="nonzero"><path clip-rule="evenodd"/></clipPath>`
    resolves the child's rule to `evenodd`). Multiple shape children
    merge into one [`oxideav_core::Path`]; the resolved rule is the
    **first contributing shape's** rule (subsequent children that
    disagree are tolerated but the merged path honours only one
    rule).
  - **Scope-restricted** — per §14.3.5, "the 'clip-rule' property only
    applies to graphics elements that are contained within a
    'clipPath' element". `clip-rule=` on the *referencing* element
    (the shape with `clip-path="url(#…)"`) is silently ignored —
    matching the spec's second worked example
    (`<rect clip-path="url(#MyClip)" clip-rule="evenodd"/>` does NOT
    flip the clip rule).
  - **Round-trip preservation.** New
    [`crate::preserved::ClipRuleBinding`] +
    [`crate::preserved::PreservedExtras::clip_rules`] side-channel
    records the canonical keyword (`nonzero` / `evenodd`) for each
    captured `<clipPath>` that either resolves to `evenodd` OR carries
    an explicit `clip-rule=` keyword in its subtree. The binding keys
    on the **source `<clipPath>` id** (for diagnostic visibility) plus
    the **path-bytes fingerprint** the encoder uses for its own
    clipPath dedup — the encoder generates fresh `clip1`/`clip2` ids
    per de-duplicated path, so routing the keyword by fingerprint
    lands it on the right def even though the source id is rewritten
    on round-trip. The encoder re-emits `clip-rule="..."` on the
    inner `<path>` of the matching `<clipPath>` def (matching the
    §14.3.5 worked example — rule on the clipping-shape, not on the
    `<clipPath>` element itself). An initial-value document with no
    explicit author keyword skips the binding entirely so a no-op
    case doesn't bloat the output.
  - **Case-insensitive matching** — `EVENODD` / `EvenOdd` /
    `evenodd` are all canonicalised to lowercase `evenodd` on the
    binding; unknown / malformed tokens (including the spec's
    `inherit` keyword and any author typo) fall back to the §14.3.5
    initial value `nonzero` without recording a binding.
  - **Id-less `<clipPath>` skipped** — a `<clipPath>` without an
    `id="..."` cannot be referenced by `clip-path="url(#…)"` and has
    no round-trip emit site, so the binding skips it even when
    `clip-rule=evenodd` is present on its children.
  - The actual clip-rule evaluation happens in `oxideav-raster`; this
    round delivers parse + scope-restricted cascade + round-trip
    preservation. `oxideav_core::Path` (used for `Group::clip`)
    carries no `fill_rule` field today, so a rasterizer that wants
    the non-default rule reads it from
    [`crate::defs::ClipPathDef`] (or via the side-channel binding
    keyed by the same path fingerprint).
  - 18 integration tests in `tests/round215_clip_rule.rs` cover the
    no-attribute baseline (no binding), `clip-rule=evenodd` on the
    child shape, `clip-rule=evenodd` on the `<clipPath>` element
    cascading to the child, per-child override of the inherited
    rule (§14.3.5 worked example), explicit `clip-rule=nonzero`
    recording for round-trip fidelity, case-insensitive keyword
    matching, unknown-token fallback, the `clip-rule` on the
    referencing element ignored per §14.3.5, id-less clipPath
    skipping, `parse_svg` (no extras) still loading the document,
    round-trip re-emitting `clip-rule="evenodd"` inside the
    `<clipPath>` def (not on the referencing rect), explicit-nonzero
    round-trip, no-attribute round-trip omitting the attribute,
    double-round-trip convergence, first-child-rule-wins for the
    merged path, typed-def exposing the resolved rule
    (`FillRule::EvenOdd` and `FillRule::NonZero`), and two distinct
    clipPaths each recording their own binding.

## Round 209 additions

- **SVG 2 §8.13 `vector-effect` property**
  (`none | [ non-scaling-stroke | non-scaling-size | non-rotation |
  fixed-position ]+ [ viewport | screen ]?`) on graphics elements and
  `<use>`.
  - New [`crate::element::VectorEffectKeyword`] +
    [`crate::element::VectorEffectHost`] +
    [`crate::element::VectorEffect`] types capture the §8.13 grammar.
    `VectorEffect::parse_custom` resolves the `[ … ]+` keyword list
    (each effect at most once, source order preserved) plus the
    optional `viewport` / `screen` host suffix; the initial host value
    is `viewport`.
  - `vector-effect` joins the resolved-property surface on
    [`crate::element::PaintState`] (initial [`VectorEffect::None`]).
    The property is **NOT inherited** per the §8.13 attribute table
    ("Inherited: no") — [`PaintState::merged_with_mctx`] resets the
    field to the initial value at every element before applying that
    element's own attribute, so a `<g vector-effect="non-scaling-
    stroke">` does NOT push the property onto child shapes.
  - Resolves through presentation attributes, inline `style="..."`,
    and `<style>`-block rules via the existing round-4 cascade. Empty
    / `none` / `inherit` payloads fall back to the initial value (the
    `inherit` keyword would inherit the parent's value, but with the
    non-inheritance reset that's also the initial value). Unknown
    keywords are silently dropped, matching the tolerant policy of
    `paint-order` / `text-anchor` / `visibility`.
  - **Round-trip preservation.** New
    [`crate::preserved::VectorEffectBinding`] +
    [`crate::preserved::PreservedExtras::vector_effects`] side-channel
    captures the canonicalised keyword string (lowercased, whitespace
    collapsed to single spaces, duplicates dropped) at the emit slot
    for each graphics element / `<use>` / `<g>` carrying a recognised
    non-`none` `vector-effect=` attribute. A `<g vector-effect=…>`
    ancestor's attribute round-trips on the group's emit site even
    though the cascade does not propagate it — the side-channel is
    purely lexical so a hand-authored grouping attribute survives a
    `parse_svg_with_extras → write_svg_with_extras` cycle. The
    encoder re-emits `vector-effect=` on the matching `<rect>` /
    `<circle>` / `<ellipse>` / `<line>` / `<polyline>` / `<polygon>` /
    `<path>` / `<g>` on round-trip.
  - **Canonical form omits the implicit host suffix.** A source
    `vector-effect="non-scaling-stroke"` round-trips as
    `vector-effect="non-scaling-stroke"` (NOT
    `... viewport`) — emitting the initial host value without source
    provenance would inflate every round-trip with a redundant token.
    A source `... screen` (or explicit `... viewport`) is preserved.
  - The actual transform suppression happens in `oxideav-raster`;
    this round only parses, exposes the resolved value on
    `PaintState`, and round-trips the source attribute. SVG 2 issue 31
    flagged values other than `non-scaling-stroke` and `none` as at
    risk of being dropped from SVG 2 due to a lack of implementations
    — we model all four so the parse + round-trip is faithful to the
    spec grammar even if a future revision narrows the value set.
  - 17 integration tests in `tests/round209_vector_effect.rs` cover
    the no-attribute baseline (no binding), single-keyword
    `non-scaling-stroke` recording, explicit `none` skip, the
    multi-keyword `[ … ]+` form, the case-insensitive matching, the
    duplicate-drop rule, the explicit `screen` host suffix preserved,
    the implicit `viewport` default omitted from the canonical form,
    the non-inheritance from a `<g>` ancestor, unknown-token tolerance,
    payload-without-effect-keyword skip, empty / `inherit` skip,
    `parse_svg`-without-extras still loads the document, round-trip
    re-emission on `<rect>` / `<path>` (multi-keyword + host) / `<g>`,
    convergence under a double round-trip pass, and whitespace
    canonicalisation.

## Round 205 additions

- **SVG 2 §13.8 `paint-order` property**
  (`normal | [ fill || stroke || markers ]`) on shapes.
  - The §13.8 cascade lands on a new
    [`crate::element::PaintOrder`] enum carried on
    [`crate::element::PaintState`] (inherited; initial `Normal`).
    Resolves through presentation attributes, inline `style="..."`,
    and `<style>`-block rules via the existing round-4 cascade. The
    spec rule "if any of the three keywords are omitted, they are
    painted last, in the order they would be painted with
    paint-order: normal" is honoured at parse time
    (`PaintOrder::parse_custom` resolves `paint-order: stroke` to
    `stroke fill markers`).
  - **Scene-graph paint-operation order.** The round-1
    `oxideav_core::PathNode` paints fill before stroke (the `normal`
    case); when the resolved order would paint stroke BEFORE fill
    (the canonical §13.8 example — stroked text where the stroke
    must appear UNDER the fill), the shape branch splits into TWO
    single-purpose `PathNode`s in a wrapping `Group` — a
    stroke-only node first (`fill: None`), then a fill-only node
    (`stroke: None`) — so the composited result honours the
    requested order under the round-1 scene-graph model. `markers`
    parses and round-trips but emits no node today
    (`oxideav_core::Node` has no `Marker` variant — round 104
    captures `<marker>` definitions; vertex-binding is a separate
    follow-up).
  - **Round-trip preservation.** New
    [`crate::preserved::PaintOrderBinding`] +
    [`crate::preserved::PreservedExtras::paint_orders`] side-channel
    captures the canonicalised keyword string (lowercased,
    whitespace collapsed, duplicates dropped) at the topmost emit
    slot for each shape. A `<g paint-order="…">` ancestor records
    on the group's own slot so the source representation survives a
    `parse_svg_with_extras → write_svg_with_extras` cycle. The
    encoder re-emits `paint-order=` on the matching shape on
    round-trip.
  - **§9.6.1 `pathLength` interplay.** When a shape carries both
    `paint-order` (stroke-first split) and `pathLength`, the
    round-21 pathLength binding targets the stroke-bearing
    `PathNode` so the dasharray rescaling attaches to the path that
    carries the stroke (`find_inner_path_subpath` picks the
    fill-none / stroke-some child when a two-child group is
    detected).
  - 15 integration tests in `tests/round205_paint_order.rs` cover
    the default-`normal` baseline, explicit `paint-order="normal"`,
    the §13.8 example (`paint-order: stroke` → two-node split with
    stroke-only first), explicit `stroke fill` and
    `stroke fill markers` forms, the fill-first / markers-only
    forms (no split), stroke-without-stroke and unknown-keyword
    fallback to single-node emission, cascade resolution via a
    `<g>` ancestor / inline `style=` / a `<style>`-block rule,
    the `PreservedExtras::paint_orders` round-trip carrier (single
    capture + re-emit + recapture), keyword-canonicalisation
    (lowercase + duplicate-drop), and the no-binding-when-normal
    policy.

## Round 199 additions

- **SVG 2 §11.2 / §11.2.2 list-of-values on `x`, `y`, `dx`, `dy` and
  `rotate`** for `<text>` and `<tspan>`. Earlier rounds parsed only
  the first scalar of each attribute (so `x="10 50 100"` collapsed to
  `x=10` and characters 2 and 3 advanced at the natural cadence);
  round 199 parses the full list and applies the n-th supplied value
  to the n-th character per the §11.2.2 "n-th character" rule.
  - An absolute `x` / `y` slot seats the current text position for
    that character (so the second character of `<text x="10 50">AB`
    lands at `x=50` regardless of where `A` left the pen).
  - A relative `dx` / `dy` slot is added to the current text position
    BEFORE the character's glyph is placed; subsequent characters
    advance from the nudged position.
  - A `rotate` slot rotates the character's glyph about its origin.
    Per §11.2.2 the final supplied `rotate` value "sticks" to every
    trailing character with no slot of its own (so
    `rotate="0 90 180"` on a 5-character run rotates char 2, 3 and 4
    all by 180°).
- **Document-wide character counter.** The five lists are layered into
  per-character vectors shared across the whole `<text>` element, so a
  `<tspan>`'s `x="100 200"` writes into slots `[char_offset,
  char_offset + 2)` where `char_offset` is the count of characters
  emitted so far in the enclosing `<text>`. A `<textPath>` body
  bumps the counter by its own character count so a sibling `<tspan>`
  after the textPath still lines up with its intended ordinal.
- **Composes with rounds 176 / 187 / 172.** An absolute `x` / `y`
  list on a `<tspan>` still opens a §11.5 anchored chunk (round 176)
  using the FIRST list value; subsequent per-character values place
  individual glyphs WITHIN the chunk without splitting it further.
  The §11.10.1.1 `text-anchor` shift (round 172) and the §11.2.1
  `textLength` rescaling (round 187) both fold over the
  per-character-placed run unchanged.
- **Lenient list grammar** — whitespace and / or single commas
  separate values per the SVG generic list-of-numbers production;
  empty tokens are skipped, unparseable prefixes are dropped, and a
  list longer than the run's character count silently drops the
  excess (no n-th character exists ⇒ no slot to apply).
- **Whitespace runs leave the pen unchanged** — leading / trailing /
  inter-tspan whitespace text in pretty-printed source no longer
  inflates a chunk's extent (a regression risk uncovered while
  adding per-character placement). Matches the round-2 `max_advance
  == 0 ⇒ pen.x = origin_x` behaviour exactly.
- 9 integration tests in `tests/round199_text_per_char.rs` cover each
  of the five list attributes, the §11.2.2 sticky-final `rotate`
  rule, the `<tspan>` overlay-at-current-ordinal layering, the
  lenient list grammar (whitespace / comma / mixed), the
  longer-than-run drop policy, the empty-`rotate=""` no-op, and the
  composition with §11.5 chunk boundaries on a multi-value `<tspan
  x>`.

## Round 187 additions

- **SVG 2 §11.2.1 `textLength` + `lengthAdjust` on `<text>` /
  `<tspan>`.** Author-supplied `textLength=…` is parsed on the root
  `<text>` and on chunk-opening `<tspan x|y …>` elements; an
  unaccompanied per-`<tspan textLength>` (no `x|y`) also folds onto
  the open anchored chunk per the §11.2.1 ancestor/descendant rule.
  The new `apply_text_length_rescaling` pass rewrites every glyph
  placement so the chunk's actual extent (`x_end − x_origin`) matches
  the requested target. `lengthAdjust="spacing"` (initial) adjusts
  only inter-glyph advances; `lengthAdjust="spacingAndGlyphs"`
  additionally post-composes `scale(s, 1)` onto each placement so the
  outlines stretch along the inline-base direction.
- **Ordering with §11.10.1.1 `text-anchor`** — the rescaling pass
  runs **before** the existing chunk-anchor shift, so the anchor
  measures against the adjusted width. A
  `<text x="400" textLength="300" text-anchor="middle">` therefore
  shifts by `−150` (not by half of the un-adjusted glyph extent),
  matching the spec's "the user agent expands/compresses the text
  string to fit within a length of textLength" wording.
- **§11.2.1 error policy** — a non-finite or negative `textLength`
  value is rejected at parse time (the binding is dropped, leaving
  the run at its natural width); unknown `lengthAdjust` keywords fall
  back to the `spacing` initial value. `<textPath>` chunks are
  excluded — they have their own §11.8.3 path-distance bias and the
  rescale pass skips them via the existing `textpath_indices` set.
- Five new tests in `tests/round187_text_length.rs` verify: baseline
  extent matches the requested target under `spacing`; every glyph
  carries the `s = target / natural` x-scale under `spacingAndGlyphs`;
  composition with `text-anchor="middle"` places the leftmost glyph
  at `x − target/2`; a per-`<tspan>` `textLength` rescales only its
  own chunk; a negative `textLength` is ignored and the run shapes
  at its natural width.

## Round 176 additions

- **SVG 2 §11.5 anchored-chunk boundaries on `<tspan x=…>` /
  `<tspan y=…>`.** Round 172 shipped a single anchored chunk per
  `<text>` element; round 176 splits the run at every absolute-
  positioning adjustment on a `<tspan>` and shifts each chunk
  independently.
  - The text walker maintains a `Vec<Chunk>` while descending into
    `<text>`. Each `Chunk` records `[start_index, end_index)` into
    the parent Group's children, the pen-x at chunk-open and
    chunk-close (for computing the chunk's extent), and the
    `text-anchor` inherited at the chunk-opening element. The root
    `<text>` opens the first chunk at its `(x, y)`; every subsequent
    `<tspan>` with an explicit `x=` or `y=` closes the open chunk
    and opens a fresh one.
  - **§11.10.1.1 shift is now per-chunk** — for each chunk,
    `extent = x_end − x_origin` and the glyph placements inside its
    index range receive an x-translate of `0` / `−extent/2` /
    `−extent` for `start` / `middle` / `end`. The previous one-shift-
    per-element behaviour from round 172 is now a special case (one
    chunk).
  - **`<tspan>` may carry its own `text-anchor=`** — case-insensitive,
    with `inherit` / unknown tokens keeping the inherited value; the
    chunk it opens uses that override rather than the root `<text>`'s
    anchor.
  - **Relative pen nudges (`dx=` / `dy=` only) do not open a chunk.**
    Both pieces stay in the same anchored chunk and a single shift
    covers the whole run.
  - **`<textPath>` closes the surrounding chunk and reopens a fresh
    one** (per §11.8 "an embedded textPath always creates an
    anchored-chunk boundary"). The textPath's own glyphs remain in
    the parallel skip-set so the outer per-chunk pass leaves them
    alone — their §11.8.3 bias has already been applied inline by
    `emit_text_path`.
  - 5 layout tests in `tests/round176_text_chunk.rs` cover: two
    `<tspan x=…>` form independent end-anchored chunks ~300 px
    apart; the per-chunk layout matches the equivalent two-`<text>`
    decomposition (proof that no extent accumulates across the
    boundary); a `dx`-only `<tspan>` stays in a single chunk; a
    `<tspan>`'s own `text-anchor=` override is honoured on its chunk;
    three chunks shift independently with the expected ~300 px gaps.

## Round 172 additions

- **SVG 2 §11.10.1.1 `text-anchor` property** (`start | middle | end`).
  Inherited via the round-118-style cascade: a new
  [`crate::element::TextAnchor`] enum lands on
  [`crate::element::PaintState::text_anchor`] (initial `Start` per the
  spec's Initial table), the `apply_one` branch case-insensitively maps
  the three keywords plus `inherit` (and tolerates unrecognised tokens
  the same way the §11.5 `visibility` branch does), and the cascade
  applies to presentation attributes, inline `style=` declarations, and
  `<style>`-block tag / class / id rules without any other plumbing.
  - **`<text>` chunk shift** — after walking the element's children the
    text module computes the chunk's pre-anchor x extent
    (`pen.x − x`) and shifts every emitted glyph's placement Group by
    `0` / `−W/2` / `−W` for `start` / `middle` / `end`. Round 172 has
    one chunk per `<text>` (the round-2 walker doesn't yet split on
    author-supplied `<tspan x=…>` boundaries — that's the §11.5 chunk-
    boundary work for a later round).
  - **`<textPath>` start-point bias per §11.8.3** — the same
    `0` / `−W/2` / `−W` term folds directly into `startOffset` before
    glyphs are laid along the curve, matching the spec's "subtract half
    of the total advance values for all of the glyphs … from the start
    of the path" rule. Total advance sums every shaped glyph's
    `x_advance` (whitespace included, since those glyphs still consume
    horizontal space).
  - **`<textPath>` children opt out of the outer `<text>` shift** —
    the walker records each textPath's emitted-glyph indices so the
    post-walk shift skips them; their §11.8.3 bias is applied inline.
  - **Without a font resolver** the post-walk shift is a no-op (zero
    glyphs in the chunk) and the document still loads cleanly, matching
    the round-2 baseline.
  - 11 parser-side tests in `tests/round172_text_anchor.rs` (default
    value, three keyword variants via presentation attribute, `inherit`
    + unrecognised-keyword tolerance, case-insensitive matching, parse-
    no-crash without a resolver, `<g>`-cascade inheritance,
    `style=`-attribute resolution, `<style>`-block rule resolution) +
    5 glyph-emission tests in `tests/round172_text_anchor_glyphs.rs`
    (three anchors shift the leftmost-glyph x by `0` / `−W/2` / `−W`,
    default matches explicit `start`, `end` moves leftwards, `<g>`-
    inherited middle matches inline middle, empty runs emit nothing
    for every anchor) + 2 `<textPath>` anchor tests in
    `tests/round172_text_path_anchor.rs` (§11.8.3 bias along a
    horizontal path, default-vs-explicit-start parity).

## Round 128 additions

- **SVG 2 §11.8 `<textPath>`** — text-on-path layout. `<textPath>`
  children of `<text>` lay their text run along a referenced path
  instead of the parent `<text>`'s baseline; each glyph's midpoint is
  moved to the corresponding point on the path and rotated by the
  path tangent at that position, matching the spec's "midpoint of
  each typographic character is moved to the corresponding point on
  the path" rule.
  - **Path-resolution precedence** per §11.8.1: `path=` (inline
    `d`-mini-language) > `href` (SVG-2 canonical) > `xlink:href`
    (deprecated SVG-1.1 fallback). The referenced `<path>` is looked
    up via the pre-walked `DefsTables::elements` id table (same
    mechanism `<animateMotion>`'s `<mpath>` resolver uses).
  - **`startOffset`** (§11.8.2): both `<number>` (user units) and
    `<percentage>` (of total path length) accepted. Negative values
    and offsets > 100% are honoured per the spec; glyphs whose
    midpoint lands off the path are silently dropped by the
    placement rule.
  - **`side="right"`** flips the path-distance about the total length
    so the text runs along the opposite side.
  - **Arc-length aware sampler** — new public
    `oxideav_svg::path_length::sample_path_at_distance(path,
    distance)` returns `(point, tangent_degrees)` for an absolute
    path-distance query. Walks line / quadratic / cubic / elliptic-arc
    / close segments at the same chord-sampling cadence as
    `compute_path_length` (32 steps per Bézier, 64 per arc) so
    cumulative distance and the running advance agree.
  - **No font resolver → empty group** — consistent with the round-2
    baseline `<text>` behaviour: a `<textPath>` whose font-family
    can't be resolved by the installed
    [`crate::text::set_font_resolver`] hook parses to an empty
    `Group`, so the surrounding document still loads.
  - 21 integration tests in `tests/round128_text_path.rs` +
    `tests/round128_text_path_glyphs.rs`.

## Round 125 additions

- **SVG 1.1 §19.2.14 `<animateMotion>` snapshot evaluator** —
  earlier rounds captured `<animateMotion>` verbatim for round-trip
  preservation but its contribution to the parent shape's transform
  was silently dropped at snapshot time. Round 125 evaluates the
  element at the caller's `t_seconds` and folds the supplemental
  `translate(x,y) rotate(angle)` matrix into the parent element's
  attribute set, matching the spec's "the effect of a motion path
  animation is to add a supplemental transformation matrix onto the
  CTM" rule. The §19.2.14 motion-path resolution precedence is
  honoured: `<mpath>` overrides `path=` overrides `values=` overrides
  `from`/`by`/`to`. Both SVG-2 `href` and SVG-1.1 `xlink:href` work
  on `<mpath>`; the referenced `<path>` is looked up via the
  pre-walked `DefsTables::elements` id table.
  - **`rotate="auto"` / `"auto-reverse"` / `<number>` / default `0`**
    — `auto` aligns the rotation with the path's tangent at the
    sampled position; `auto-reverse` adds 180°; a numeric value
    holds constant; the implicit default emits no `rotate` term so
    the output stays a plain `translate(...)` in the common case.
  - **`keyPoints` + `keyTimes` override** the natural arc-length
    fraction mapping per §19.2.14: the (keyTimes, keyPoints) pair
    remaps document time to path-distance.
  - **`calcMode` defaults to `paced`** per the §19.2.14 difference
    from the rest of the SMIL animation family — paced traverses
    the motion path at constant arc-length velocity.
  - **Arc-length aware sampling** — straight segments use exact
    geometry; cubic / quadratic Béziers use 32-chord flattening;
    elliptic arcs use 64-chord flattening (matching the
    `path_length` module's density, so the running accumulator and
    the total arc length agree).
  - **Public API**: new
    `oxideav_svg::animation::evaluate_motion_at(el, t, id_lookup)`
    + `snapshot_children_with_resolver(parent, t, id_lookup)`. The
    legacy `snapshot_children(parent, t)` keeps working but resolves
    `<mpath>` references only when the caller threads an id-lookup
    closure through.
  - 26 integration tests in `tests/round125_animate_motion.rs`
    (straight-line / cubic / arc paths, `<mpath>` resolution via
    both `xlink:href` and SVG-2 `href`, all four `rotate` modes,
    `repeatCount="indefinite"`, `begin` delay, `fill="freeze"`
    end-of-anim hold, `keyPoints`/`keyTimes` remapping, malformed
    input recovery, override precedence, round-trip preservation
    via `PreservedExtras`).

## Round 122 additions

- **SVG 2 §5.8 `<title>` / `<desc>` + §5.9 `<metadata>` descriptive
  elements.** All three are *never-rendered* per the §5.8 / §5.9 dfn
  blocks (the UA stylesheet forces `display:none` with importance over
  any other CSS rule), so they MUST NOT contribute scene-graph nodes.
  Round 122 captures them on side-channels for lossless round-trip.
  - **`<title>` / `<desc>`** capture into a typed
    `crate::preserved::DescriptiveText { text, lang }`, keyed by the
    **parent** container's scene-graph tree-path on the new
    `PreservedExtras::titles` / `PreservedExtras::descs:
    Vec<DescriptiveBinding>` side-channels (same layout as the round-13
    `id_paths` / round-115 `links`). Multiple sibling `<title>`s under
    the same parent (the §5.8 multilingual-alternative pattern,
    `<title lang="en">…</title> <title lang="nl">…</title>`) append to
    the same binding's `items` list in document order so a downstream
    consumer can run the §5.8 best-language selection algorithm.
    `lang` (SVG-2 canonical) is captured first; `xml:lang` (deprecated)
    is the fallback when `lang` is absent.
  - **`<metadata>`** captures verbatim on
    `PreservedExtras::metadata: Vec<Element>` — the content model is
    "any elements or character data" (typically RDF / Dublin Core /
    Inkscape extensions), so a structured parse is out of scope.
  - **Encoder** — `write_svg_with_extras` re-emits captured titles +
    descs as the **first children** of the matching `<g>` (or, for the
    root-`<svg>` empty path, at the top of the output document) so an
    SVG 1.1 reader that "may not recognize a title element that is not
    the first child of its parent" still picks them up. `<title>`
    precedes `<desc>` per the §5.8 example structure. `<metadata>`
    re-emits at the trailing edge of the output.
  - 15 integration tests in `tests/round122_descriptive.rs`.

## Round 118 additions

- **SVG 1.1 §11.5 `display` + `visibility` presentation properties.**
  - **`display: none`** removes the element *and its whole subtree* from
    the rendering tree — no scene-graph node is produced. Resolved
    through both presentation attributes and the CSS cascade. `display`
    is *not* inherited (§11.5, Inherited: no), so the cascade resets it
    to the initial `inline` before each element applies its own value.
    Applies to `<svg>` / `<g>` / `<switch>` / `<a>` / `<foreignObject>`
    / `<use>` / the graphics elements (`<rect>` / `<circle>` /
    `<ellipse>` / `<line>` / `<polyline>` / `<polygon>` / `<path>` /
    `<text>`); never-rendered elements (`<defs>`, gradients, `<marker>`,
    `<symbol>`, `<mask>`, `<clipPath>`, `<style>`, animation) are
    excluded per the spec.
  - **Referencing still works.** §11.5: a `display:none` definition
    "can still be referenced." A `<use href="#hidden">` of a
    `display:none` element renders its instance; only the *instance
    root* is exempt, so a `display:none` descendant inside the
    instantiated subtree still drops.
  - **`visibility: hidden | collapse`** keeps the node in the tree (its
    geometry still contributes to bounding-box / clipping calculations
    per §11.5) but paints nothing — fill + stroke are dropped.
    `visibility` *is* inherited, so a `<g visibility="hidden">` hides
    its children while a descendant may flip back to
    `visibility="visible"`. Text glyphs honour the same suppression.
  - 13 integration tests in `tests/round118_display_visibility.rs`.

## Round 115 additions

- **SVG 2 §16.5 `<a>` hyperlink element.** `<a>` is categorised as both
  a *container element* and a *renderable element*, so it now renders
  its children into an `oxideav_core::Node::Group` exactly like `<g>` —
  honouring `transform` (§8.5 presentation property), `opacity`, the
  paint cascade, and the per-element `em` / `rem` length-resolution
  context. Earlier rounds dropped the whole `<a>` subtree (it fell
  through to the no-op default arm), so a shape wrapped in an
  `<a href="…">` was silently invisible; round 115 paints it.
- **Hyperlink preservation via `PreservedExtras::links`.**
  `oxideav_core::Group` has no hyperlink field, so the link target and
  its HTML companion attributes (`href` — SVG-2 `href` with SVG-1.1
  `xlink:href` fallback — plus `target` / `download` / `ping` / `rel` /
  `hreflang` / `type` / `referrerpolicy`) are stowed on the new
  `crate::preserved::LinkBinding`, keyed by the group's scene-graph
  tree-path (same layout as the round-13 `id_paths` / round-21
  `path_lengths` side-channels). `parse_svg_with_extras` populates the
  table; `write_svg_with_extras` re-wraps the matching `<g>` in its
  `<a href="…">…</a>` element on round-trip. A bare `<a>` (no `href`)
  still groups its children and round-trips as `<a>`.
- 12 integration tests in `tests/round115_anchor.rs` (child renders,
  group node shape, `transform` / `opacity` on the group, link-binding
  capture, `xlink:href` fallback + `href`-wins precedence, full
  attribute round-trip, nested-`<a>`-inside-`<g>` tree-path targeting,
  bare-`<a>` grouping, multi-child grouping).

## Round 104 additions

- **SVG 2 §13.7.1 `<marker>` definition capture.** `<marker
  id="...">` definitions now parse into a typed
  `crate::defs::MarkerDef` carrying every spec presentation
  attribute: `refX` / `refY` (with the SVG-2 geometric keywords
  `left` / `center` / `right` and `top` / `center` / `bottom`
  pre-resolved against the `viewBox` per the §13.7.1 mapping table),
  `markerWidth` / `markerHeight` (default 3), `markerUnits`
  (`strokeWidth` / `userSpaceOnUse`; default `strokeWidth`), `orient`
  (`auto` / `auto-start-reverse` / `<angle>` / `<number>`; default
  `0`), `viewBox`, `preserveAspectRatio`, plus the parsed marker
  content as a `Group`. Captured into
  `DefsTables::markers: HashMap<String, MarkerDef>` during the
  pre-walk so forward references resolve. The verbatim XML also rides
  on `PreservedExtras::markers: Vec<Element>` so `parse → write_svg
  _with_extras` round-trips the definition byte-faithfully (encoder
  re-emits it inside the `<defs>` block alongside `<pattern>` /
  `<filter>` extras).
- **`<marker>` is never-rendered** per §13.7.1 — the scene-walk skips
  it (no scene-graph node), exactly like `<filter>` / `<mask>` /
  `<clipPath>` / `<symbol>`. A document that references a marker via
  `marker-end="url(#arrow)"` therefore loads cleanly even though the
  marker isn't yet painted into the scene.
- **SVG 2 §13.2 `context-fill` / `context-stroke` `<paint>` keywords**
  — used by the spec's own `<marker>` examples to match marker colour
  to the referencing element's stroke — now parse gracefully. The
  static scene graph has no context element, so they map to no paint
  per the spec rule "If there is no context element and these
  keywords are used, then no paint is applied" — instead of failing
  the whole document.
- `oxideav_core::Node` lacks a `Marker` variant, so the vertex
  placement + `orient` rotation + `markerUnits` scaling (§13.7.4)
  and the per-shape `marker-start` / `marker-mid` / `marker-end` /
  `marker` shorthand property binding remain a followup for once a
  `Marker` node lands in core. Round 104 delivers the typed
  definition + lossless round-trip, mirroring the round-20
  `<pattern>` capture pattern.

## Round 98 additions

- **SVG 2 §5.7 `<switch>` conditional processing**. `<switch>` now
  evaluates the `requiredExtensions` (§5.7.4) and `systemLanguage`
  (§5.7.5) test attributes on its direct children in document order and
  renders the **first** child for which all tests pass; the rest are
  bypassed (§5.7.3). A chosen container child (`<g>`, …) renders its
  whole subtree. Lives in the new `oxideav_svg::conditional` module.
  - **`requiredExtensions`** — absent → true; empty/whitespace-only
    string → false; any named extension → false (oxideav implements no
    language extensions, so "all of the given extensions are supported"
    can't hold).
  - **`systemLanguage`** — comma-separated BCP 47 tokens; absent → true;
    empty string → false; otherwise true iff a user-preferred tag
    case-insensitively equals, or is a `-`-boundary prefix of, one of
    the attribute's tags. The user-preferred list is caller-supplied via
    the new `parse_svg_at_with_languages(bytes, t, &["en", …])` entry
    point (oxideav owns no UA locale registry); `parse_svg` /
    `parse_svg_at` pass an empty list, so a present, non-empty
    `systemLanguage` matches nothing and the `<switch>` falls through to
    the spec-recommended un-tagged "catch-all" child.
  - **Never-rendered children** (`<style>`, `<script>`, `<desc>`,
    animation elements, defs) are skipped without consuming the
    "first-match" slot, per §5.7.1. The legacy SVG 1.1
    `requiredFeatures` attribute was removed in SVG 2 (§5.7.1) and is
    deliberately not evaluated.

## Round 95 additions

- **SVG 2 §16.3 `<view>` element + fragment-identifier routing**.
  `<view id="...">` per §16.3.3 captures into a new typed
  `crate::defs::ViewDef { view_box, preserve_aspect_ratio,
  zoom_and_pan }` populated on `DefsTables::views` during the
  pre-walk. The element itself doesn't push a scene-graph node — it
  carries the override parameters a fragment-identifier link should
  apply when a host loads `MyDrawing.svg#MyView`.
- **New `resolve_fragment(&frame, &extras, fragment) -> ResolvedView`
  top-level API** per §16.3.2. Honours both fragment shapes:
  - **Bare-name** (`#MyView`) — looks up the id on
    `extras.typed_views`; attributes the view specified override the
    root `<svg>`'s, anything the view left out inherits from the root.
  - **`svgView(...)` spec** —
    `#svgView(viewBox(0,0,200,200);preserveAspectRatio(xMidYMid);zoomAndPan(disable);transform(scale(5)))`,
    semicolon-separated, in any order, each attribute at most once.
    Percent-encoded semicolons (`%3B`) tolerated per CSSOM escaping.
    Unknown attributes drop silently; malformed payloads (e.g. a
    `viewBox(...)` with the wrong number of arguments) fall back to
    the baseline.
  - **Empty fragment** / spatial (`xywh=`) / temporal (`t=`) /
    track / id media-fragments degrade to the document root's
    baseline view, matching the spec's "as if no fragment
    identifier was provided" rule.
- **New `ZoomAndPan` enum** (`Disable` / `Magnify`; default
  `Magnify` per §16.3.3).
- **Round-trip preservation** — `PreservedExtras::views:
  Vec<Element>` (verbatim XML) plus
  `PreservedExtras::typed_views: HashMap<String, ViewDef>` (typed
  mirror for resolution). `write_svg_with_extras` re-emits each
  captured `<view>` at the trailing edge of the output so a
  `parse_svg_with_extras → write_svg_with_extras →
  parse_svg_with_extras` cycle preserves every view definition and
  bare-name lookup still resolves on the round-tripped document.

## Round 21 additions

- **SVG 2 §9.6.1 `pathLength` attribute** on every
  `SVGGeometryElement` (`<path>`, `<rect>`, `<circle>`, `<ellipse>`,
  `<line>`, `<polyline>`, `<polygon>`). The decoder parses the
  author's value, computes the **geometric** length of the resulting
  `oxideav_core::Path` via the new `oxideav_svg::path_length` module
  (chord-sum for line / quadratic / cubic; centre-parameterised
  sampling for elliptic arcs per SVG 1.1 §F.6.5), and rescales
  `stroke-dasharray` / `stroke-dashoffset` by
  `geometric_length / pathLength`. A downstream rasteriser that
  consumes user-space lengths therefore paints the spec-correct dash
  pattern even though it knows nothing about `pathLength`.
- **§9.6.1 edge cases** — `pathLength=0` collapses a non-zero
  dasharray to a solid stroke (the spec's "scaling factor of infinity"
  interpretation); an all-zero dasharray survives ("zero scaled
  infinitely must remain zero"); negative values are an error per
  §9.6.1 and silently ignored; missing / unparseable values are a
  no-op.
- **Round-trip preservation** — `PreservedExtras::path_lengths:
  Vec<PathLengthBinding>` records the author's original value keyed
  by scene-graph tree-path; `encoder::write_svg_with_extras` re-emits
  `pathLength="..."` on the matching shape on round-trip so a
  consumer that wants the calibration metadata sees it.

## Round 81 additions

- **SVG 2 §14.1.1 gradient `href` template inheritance** — `<linearGradient
  id="child" href="#tmpl"/>` (and the legacy `xlink:href` form) now
  inherits any *unspecified* attribute from the template chain (`x1` /
  `y1` / `x2` / `y2` / `cx` / `cy` / `r` / `fx` / `fy` / `fr` /
  `gradientUnits` / `gradientTransform` / `spreadMethod`) AND inherits
  the template's `<stop>` children when the child has none. A child's
  *specified* attribute always wins per §14.1.1. Self-references and
  longer cycles terminate at an 8-hop depth cap (matching the round-13
  CSS `@import` cap) and fall back to spec-default initial values
  rather than diverging.
- **Typed `<linearGradient>` / `<radialGradient>` view** — new
  [`crate::defs::GradientDef`] records the un-resolved per-element
  state (every numeric attribute is `Option<f32>` so the chain walker
  can tell "not specified" from "specified-with-explicit-value"); the
  pre-walk populates `DefsTables::gradients`, and the second-pass tree
  walk flattens each entry through
  [`crate::defs::resolve_gradient_chain`] into a legacy
  [`oxideav_core::Paint::LinearGradient`] / `RadialGradient` for the
  existing round-1 fill resolver. The typed view stays on
  `DefsTables::gradients` so a downstream rasteriser that wants the
  full SVG-2 surface (`gradientUnits` mapping into the referencing
  element's bounding box, full 2×2 `gradientTransform`, `fr` focal-
  circle radius) reads it directly without re-parsing XML.
- **`gradientTransform` is folded into the flattened paint** — the
  start / end / centre / focal points are transformed in place. The
  radius scales by the geometric mean of the matrix's per-axis scale
  (a uniform-scale `gradientTransform` is bit-exact; non-uniform scale
  / shear keeps full fidelity in the typed `ResolvedGradient`).
- **Verbatim round-trip** — `PreservedExtras::gradients: Vec<Element>`
  carries each source `<linearGradient>` / `<radialGradient>` for the
  encoder to re-emit byte-faithfully inside the `<defs>` block. A
  `parse_svg_with_extras → write_svg_with_extras` cycle preserves
  `gradientUnits` / `gradientTransform` / `href` / `xlink:href`
  exactly as authored, plus the original `<stop>` ordering. The
  encoder skips the scene-walk's flattened emission for any id the
  side-channel already carried so the output never duplicates a
  definition.

## Round 20 additions

- **`<pattern>` paint-server capture (SVG 2 §14.3)** —
  `<pattern id="...">` definitions now parse into a typed
  `crate::defs::PatternDef` carrying every spec attribute
  (`x` / `y` / `width` / `height` + `patternUnits` /
  `patternContentUnits` per §14.3.1 + `patternTransform` + `viewBox`
  + `preserveAspectRatio` + `href` / `xlink:href` template
  reference). The typed view hangs off
  `DefsTables::patterns: HashMap<String, PatternDef>` so a downstream
  rasterizer can consume it without re-parsing. The verbatim XML
  also rides on `PreservedExtras::patterns: Vec<Element>` so
  `parse → write_svg_with_extras` round-trips the definition
  byte-faithfully (encoder re-emits it inside the `<defs>` block).
- **SVG 2 §13.2 paint-list (`url(#id) [none | <color>]?`)** —
  `PaintValue::Reference` widened to a struct variant with an
  optional `fallback: Option<Option<Rgba>>` capturing the SVG 2
  three-way distinction (no fallback / explicit `none` / explicit
  colour). The fill / stroke resolver consults both the gradient
  table and the pattern table; a known pattern resolves to the
  fallback colour today because `oxideav_core::Paint` has no
  `Pattern` variant yet — once it lands, the pattern branch will
  return the tiled paint directly. Unknown ids fall back the same
  way, matching the spec's "if the paint server reference cannot be
  resolved" wording. Inkscape / Illustrator hatch-pattern exports
  therefore no longer render as silent-empty fills; they pick up
  the author-supplied fallback colour while still preserving the
  pattern definition for a later renderer.

## Round 18 additions

- **CSS Values L4 length-unit aware coordinate parsing** via the new
  `crate::length` module. `Length { value, unit }` carries every CSS
  Values L4 §6 unit (`Px`, `Em`, `Rem`, `Percent`, `Vw`, `Vh`, `Vmin`,
  `Vmax`, `Pt`, `Cm`, `Mm`, `In`, `Pc`, `Q`, plus `UserUnit` for
  bare-number SVG attributes). `parse_length(s)` recognises every
  suffix (case-insensitive); `Length::resolve(ctx)` returns the px
  value given a `ResolveContext` (current font-size, root font-size,
  viewport dimensions, percentage basis). The legacy `parse_number`
  path stays unchanged — bare numeric coordinates parse to
  `LengthUnit::UserUnit` and resolve bit-for-bit identically to a raw
  `f32::from_str`, so existing fixtures round-trip without drift.
- **CSS Easing Functions L2 `linear()` function** —
  `crate::keyframe::TimingFunction` gains a `LinearStops { stops }`
  variant with the L2 §3.1 missing-input fill-in algorithm (first
  stop → 0%, last → max(prev, 100%), monotonic-clamp on regressions,
  linear ramp through unspecified middle inputs). `compute_progress`
  walks the sorted stops and lerps the bracketing pair. The
  `animation-timing-function` cascade reader now uses paren-aware
  comma splitting so `linear(0, 0.5 25%, 1)` survives intact; the
  bare `linear` keyword still maps to the L1 unit-variant identity.

## Round 17 additions

- **CSS `@supports` block parse + evaluation** per CSS Conditional
  Rules L3 — `@supports (cond) { rules }` lands on
  `Stylesheet::supports_rules: Vec<SupportsRule>` with a parsed
  `SupportsCondition::{Property, Not, And, Or, Always}` enum.
  `Stylesheet::resolve_for_supports_context(supported)` walks the
  rules against a `HashSet<(String, String)>` of supported (property,
  value) pairs and returns the merged cascade (symmetric to round
  16's `@media` evaluation).
- **CSS Animations L1 long tail** —
  `animation-timing-function` (`linear` / `ease*` / `cubic-bezier(...)`
  / `steps(N, start|end)` per CSS Easing Functions L1 §3 / §4),
  multi-name `animation-name: a, b, c` evaluating each animation
  independently with mod-indexed pairing on every other longhand list
  (L1 §6), `animation-direction` (`normal` / `reverse` / `alternate` /
  `alternate-reverse` per §4.4), `animation-fill-mode` (`none` /
  `forwards` / `backwards` / `both` per §4.7).

## Round 16 additions

- **CSS `@media` block parse + evaluation** per CSS Media Queries L4.
  Round 11–15 silently dropped `@media` blocks; round 16 routes them
  to the new `Stylesheet::media_rules: Vec<MediaRule>`. Each
  `MediaRule` carries the parsed `MediaCondition` (a list of
  `MediaQuery`s ORed via comma-separated lists; each query carries an
  optional `not` / `only` modifier, an optional media type, and a
  list of `MediaFeature`s ANDed together) plus the inner rules. The
  new `Stylesheet::resolve_for_media_context(viewport_w, viewport_h,
  orientation)` evaluates each captured query and returns the merged
  cascade in source order so `matched_declarations` still resolves
  specificity / source-order ties correctly. Width / height (with
  `min-` / `max-` prefixes per §4) and `orientation: portrait |
  landscape` are honoured; unrecognised features
  (`prefers-color-scheme`, `color-gamut`, etc.) round-trip as
  `MediaValue::Raw` but never match (the rule is dormant).
- **CSS `@keyframes` evaluation at runtime `t_seconds`** per CSS
  Animations L1 §3 via the new `crate::keyframe` module. An element
  whose CSS cascade resolves to `animation-name: <kf>` +
  `animation-duration: <s>` has the bracketing keyframe pair lerped
  at `t_seconds`, and the resulting property values folded into the
  element's effective property map (transform values land in the
  `transform=` attribute slot; everything else lands in `style=`).
  Honoured longhands: `animation-name`, `animation-duration` (`s` /
  `ms`), `animation-iteration-count` (numeric or `infinite`),
  `animation-delay`. Lerp coverage: `transform: rotate | translate |
  scale(...)`, `opacity` / `fill-opacity` / `stroke-opacity` /
  `stroke-width`, colour properties via the shared SMIL
  `lerp_string` path. Wired into `parse_svg_at(t_seconds)` so a
  single `transform: rotate(180deg)` renders correctly at `t = 0.5s`
  of a 1-second `from rotate(0deg) → to rotate(360deg)` animation.
- **Transform parser accepts CSS unit suffixes** per SVG 2 / CSS
  Transforms L1 — `rotate(180deg)` / `rotate(0.5turn)` /
  `translate(10px, 20px)`. Angle units (`deg` / `rad` / `grad` /
  `turn`) convert to canonical degrees; length units (`px` / `em` /
  `%`) parse and are dropped (round 16 still treats every length as
  user units).

## Round 15 additions

- **`<image>` element capture (SVG 2 §6).** Inline
  `data:image/<mime>;base64,…` URIs are base64-decoded into
  `crate::image::ImageHref::DataUri { mime, bytes }`; external
  `href="logo.png"` (and legacy `xlink:href`) is captured verbatim
  into `crate::image::ImageHref::External(String)` for caller-side
  fetching. The new typed `crate::image::SvgImage` carries
  `(x, y, width, height, transform, id, parent_id,
  preserve_aspect_ratio)`. Each captured image lives on the new
  `PreservedExtras::images: Vec<SvgImage>`; the encoder re-emits
  them at the trailing edge with a faithful round-trip (data URIs
  re-encode from the decoded bytes; external URLs are preserved
  as-is). `oxideav_core::Node::Image` requires a fully-decoded
  `VideoFrame`, so round 15 deliberately keeps the raster bytes
  opaque on the SVG side — the renderer (or a caller that owns a
  PNG / JPEG decoder) decodes them lazily, avoiding a fan-out of
  image-format crate dependencies into oxideav-svg.
- **CSS `@keyframes` block capture (CSS Animations L1 §3).** Round
  11 + 14 routed `@import` and `@font-face` to dedicated parsers
  but silently dropped `@keyframes`. Round 15 routes
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

## Round 14 additions

- **`<symbol>` + `<use>` viewport mapping.** Round 3 instantiated
  `<use href="#sym">` references but skipped the symbol's `viewBox`,
  the use's `width` / `height`, and the symbol's
  `preserveAspectRatio`. Round 14 wraps the symbol's children in an
  inner `Group` carrying the SVG 2 §8.2 viewport transform between
  the use's `transform=` / `x` / `y` / `opacity` and the
  instantiated content. The use's `width` / `height` fall through to
  the symbol's intrinsic `width` / `height` when omitted (per §5.6);
  symbols with no `viewBox` skip the wrap (the use's `width` /
  `height` are ignored per spec). `SymbolDef` (in `crate::defs`)
  gains four new fields (`view_box`, `preserve_aspect_ratio`,
  `intrinsic_width`, `intrinsic_height`) populated by
  `parse_symbol_def`.
- **`@font-face` block capture.** Rounds 11 + 13 routed `@import` to
  `Stylesheet::imports` but tagged every other `@-rule` (including
  `@font-face`) for tolerant skip in `parse_block`. Round 14 routes
  `@font-face { ... }` to a dedicated parser that surfaces the
  descriptor list on the new `Stylesheet::font_faces`. Each
  `FontFace` carries a typed `family: String` + `src: Vec<FontSource>`
  view plus a `descriptors: HashMap` for the long tail (`font-weight`,
  `font-style`, `font-stretch`, `unicode-range`, `font-display`, …).
  `FontSource` covers both the `url(...) [format(...)]` and
  `local(...)` shapes per CSS Fonts L3 §4.3 — a downstream
  font-resolver can iterate the list and register the user-supplied
  fonts before the cascade matches a `font-family: ...` declaration.

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

## Deferred to round 15+

- Actual filter-primitive rasterisation (the typed graph is
  pre-rasteriser plumbing; pixel evaluation is `oxideav-raster` work).
- `marker-start` / `marker-mid` / `marker-end` *rendering* — round 104
  captures the `<marker>` definition (typed `MarkerDef` + verbatim
  round-trip via `PreservedExtras::markers`); painting the marker
  graphics at shape vertices with the §13.7.4 `orient` / `markerUnits`
  rules needs a `Marker` construct in `oxideav-core`.
- `<text>` `textPath` (SVG 2 §11.8) — text-on-path layout **landed in
  round 128**. Remaining deferral: `method="stretch"` per-glyph path
  warping (round 128 ships `method="align"` semantics — affine glyph
  placement without outline warping, which is the default and matches
  almost every real-world use case).
- `<image>` element with embedded data URIs / external href.
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
