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

## Deferred to round 3+

- `<text>` — needs `oxideav-scribe` font handling.
- `<use>` cross-references beyond a captured `<symbol>` table.
- `<script>`.
- `.svgz` (gzip-compressed SVG) — registered as an extension but
  demuxing rejects it.

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
let mut codecs = oxideav_core::CodecRegistry::new();
let mut containers = oxideav_core::ContainerRegistry::new();
oxideav_svg::register(&mut codecs, &mut containers);
```
