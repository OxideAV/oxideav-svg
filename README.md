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

## Deferred to round 7+

- `<script>`.
- Filter primitive rasterisation (feGaussianBlur, feColorMatrix,
  …). The `<filter>` definition survives round-trip via
  `PreservedExtras`; actual rasterisation lives in `oxideav-raster`.
- Animation `calcMode="paced|spline"` (currently degrade to
  `linear`).
- `<marker>` defs + `marker-start` / `marker-mid` / `marker-end`
  (needs a `Marker` construct in `oxideav-core`).
- Pseudo-elements (`::before`, `::after`); stateful pseudo-classes
  (`:hover`, `:focus`, `:checked`); `@import` of external stylesheets.
- Animation re-attachment to specific scene-graph emit sites in the
  encoder (currently appended at the trailing edge of the document
  with a parent-id comment).

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
