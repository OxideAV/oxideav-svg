# oxideav-svg

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
  `<g>` (with `transform`), `<defs>`, `<symbol>`.
* **Shapes** — `<rect>` (incl. `rx`/`ry`), `<circle>`, `<ellipse>`,
  `<line>`, `<polyline>`, `<polygon>`, `<path>` (full `d` mini-language:
  M/m L/l H/h V/v C/c S/s Q/q T/t A/a Z/z, smooth-curve reflection),
  plus the `pathLength` rescaling of dash patterns.
* **Paint servers** — `<linearGradient>` / `<radialGradient>` with
  `<stop>` children and `spreadMethod`, resolved via `fill="url(#id)"`.
* **Text** — `<text>` / `<tspan>` (and `textPath` align-mode layout)
  rasterised vector-first through
  [`oxideav-scribe`](https://github.com/OxideAV/oxideav-scribe); the
  caller installs a font resolver once at startup. Gated behind the
  default-on `text` feature.
* **References** — `<use href="#id">` (SVG 2 `href` + SVG 1.1
  `xlink:href`), with cycle detection.
* **Masking / clipping** — `<mask>` → `oxideav_core::Node::SoftMask`
  honouring `mask-type`; `<clipPath>` collapsed into the group's `clip`.
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
  formulae), and `feComponentTransfer` (the §9.7 per-channel
  `identity` / `table` / `discrete` / `linear` / `gamma` transfer
  functions); the general filter pixel pipeline is `oxideav-raster` work.
* **Markers** — `<marker>` definitions parse into a typed `MarkerDef`
  and round-trip; vertex placement / `orient` rendering is deferred to a
  core `Marker` node.
* **Animation** — `<animate>` / `<set>` / `<animateTransform>`
  snapshotting via `parse_svg_at(bytes, t)` with the SMIL timing model
  (`begin` / `dur` / `repeatCount` / `keyTimes` / `values` /
  `from`/`to`/`by`, `calcMode` `discrete`/`linear`/`paced`/`spline`).
  `parse_svg` snapshots first-paint at `t = 0`.
* **Conditional processing** — `<switch>` evaluates
  `requiredExtensions` / `systemLanguage` and renders the first passing
  child; `<view>` definitions + `resolve_fragment` honour both bare-name
  and `svgView(...)` fragment identifiers.
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

## Compression

`.svgz` (gzip-compressed SVG, RFC 1952) is sniffed transparently on
read; `write_svgz()` and the `svgz` muxer produce gzipped output. Pure
Rust, no C dependencies.

## Not yet supported

* Filter-primitive rasterisation beyond the in-crate evaluators above
  (the typed graph is pre-rasteriser plumbing).
* Marker rendering, `textPath method="stretch"` per-glyph warping, the
  `<image>` element, and live pseudo-element / stateful pseudo-class
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
