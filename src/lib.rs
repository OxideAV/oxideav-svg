//! Pure-Rust SVG (read + write) for the oxideav framework.
//!
//! Implements a focused subset of SVG 1.1 / 2.0 — enough to load
//! ~90% of real-world icons, logos, and editor exports — without any
//! external XML / SVG library. The decoder produces an
//! [`oxideav_core::VectorFrame`]; the encoder serialises one back.
//!
//! # Element subset (round 1)
//!
//! * Containers: `<svg>` (with `viewBox` / `width` / `height`), `<g>`,
//!   `<defs>`.
//! * Shapes: `<rect>` (incl. `rx`/`ry` rounded corners), `<circle>`,
//!   `<ellipse>`, `<line>`, `<polyline>`, `<polygon>`, `<path>` (full
//!   `d`-attribute mini-language: M/m, L/l, H/h, V/v, C/c, S/s, Q/q,
//!   T/t, A/a, Z/z; smooth-curve reflection per §8.3.6 / §8.3.7).
//! * Paint servers: `<linearGradient>`, `<radialGradient>` with
//!   `<stop>` children, resolved via `fill="url(#id)"`.
//! * Presentation attributes: `fill`, `stroke`, `stroke-width`,
//!   `stroke-linecap`, `stroke-linejoin`, `stroke-miterlimit`,
//!   `stroke-dasharray`, `stroke-dashoffset`, `opacity`,
//!   `fill-opacity`, `stroke-opacity`, `fill-rule`,
//!   `transform` (`matrix` / `translate` / `rotate` / `scale` /
//!   `skewX` / `skewY`).
//! * Colour values: CSS Color L3 named colours, `#RGB`, `#RGBA`,
//!   `#RRGGBB`, `#RRGGBBAA`, `rgb()` / `rgba()`, `none`,
//!   `url(#id)`, `currentColor` (resolved to opaque black in round 1).
//!
//! # Round 2 additions
//!
//! * `<text>` / `<tspan>` — vector-first via
//!   [`oxideav-scribe`](https://crates.io/crates/oxideav-scribe)'s
//!   `Shaper::shape_to_paths`. Caller installs a font resolver
//!   ([`text::set_font_resolver`]) once at startup; SVG-side parsing
//!   extracts `font-family` / `font-size` / `x` / `y` / inline content
//!   (and nested `<tspan dx dy>`) and emits positioned glyph PathNodes
//!   wrapped in a Group at the text origin. Gated behind the
//!   on-by-default `text` cargo feature.
//! * `<filter>` graceful pass-through. `<filter id="...">` definitions
//!   are captured into a side table; `filter="url(#id)"` on elements
//!   is recognised and wraps content in an extra `Group` so the
//!   structural intent ("these children are filtered") survives a
//!   parse → encode round-trip. The actual filter graph (Gaussian
//!   blur, color matrix, …) is rendered by `oxideav-raster` in a
//!   later round.
//! * `<mask>` and `<clipPath>` — multi-element masks map to
//!   [`oxideav_core::Node::SoftMask`] (luminance or alpha per
//!   `mask-type`); multi-shape `<clipPath>` collapses children (with
//!   their per-element `transform=`) into a single concatenated clip
//!   [`oxideav_core::Path`] applied to the wrapping group's `clip`
//!   field. The encoder rewrites both back into `<defs>` blocks with
//!   auto-generated ids on round-trip.
//! * Graceful skip for `<foreignObject>` (empty `Group`),
//!   `<animate>` / `<animateTransform>` / `<set>` (silently dropped),
//!   `<symbol>` (captured for round-3 `<use>` resolver but not yet
//!   rendered).
//!
//! # Deferred to round 3+
//!
//! * `<use>` cross-references.
//! * `<script>`.
//! * `.svgz` (gzip-compressed SVG) — registered as an extension but
//!   demuxing rejects it.

pub mod color;
pub mod container;
pub mod decoder;
pub mod defs;
pub mod element;
pub mod encoder;
pub mod parser;
pub mod path_data;
#[cfg(feature = "text")]
pub mod text;
pub mod transform;

pub use decoder::{make_decoder, parse_svg, CODEC_ID_STR};
pub use encoder::{make_encoder, write_svg};

use oxideav_core::{CodecCapabilities, CodecId, CodecInfo, CodecRegistry, ContainerRegistry};

/// Register the SVG codec (decoder + encoder) on `reg`.
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("svg_sw")
        .with_intra_only(true)
        .with_lossless(true)
        // SVG is resolution-independent — pick a generous cap that
        // mirrors the rest of the image-format crates so the registry
        // doesn't apply implementation-side limits.
        .with_max_size(65535, 65535);
    reg.register(
        CodecInfo::new(CodecId::new(CODEC_ID_STR))
            .capabilities(caps)
            .decoder(make_decoder)
            .encoder(make_encoder),
    );
}

/// Register the SVG container (demuxer + muxer + extensions + probe).
pub fn register_containers(reg: &mut ContainerRegistry) {
    container::register(reg);
}

/// Combined registration: codecs + containers. Same shape as every
/// other single-image sibling crate (`oxideav-bmp` / `oxideav-png` /
/// `oxideav-webp` / …).
pub fn register(codecs: &mut CodecRegistry, containers: &mut ContainerRegistry) {
    register_codecs(codecs);
    register_containers(containers);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_does_not_panic() {
        let mut codecs = CodecRegistry::new();
        let mut containers = ContainerRegistry::new();
        register(&mut codecs, &mut containers);
    }

    #[test]
    fn round_trip_single_rect() {
        let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10" viewBox="0 0 20 10">
  <rect x="0" y="0" width="20" height="10" fill="#0000ff"/>
</svg>"##;
        let frame = parse_svg(src).unwrap();
        let bytes = write_svg(&frame);
        let frame2 = parse_svg(&bytes).unwrap();
        assert_eq!(frame.width, frame2.width);
        assert_eq!(frame.height, frame2.height);
        assert_eq!(frame.root.children.len(), frame2.root.children.len());
    }
}
