# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
