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
