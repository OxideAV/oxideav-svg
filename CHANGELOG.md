# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
