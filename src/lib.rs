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
//! # Round 3 additions
//!
//! * `<use href="#id">` cross-references — resolves the referenced
//!   element (any shape, group, or `<symbol>`) from a documentwide
//!   id table built during the pre-walk; `x` / `y` / `transform` /
//!   `width` / `height` on the `<use>` are honoured. Cycles
//!   (`use → symbol → use of same id`) are detected and the offending
//!   instantiation is dropped instead of recursing infinitely.
//! * `.svgz` (gzip-compressed SVG, RFC 1952) — both [`parse_svg`] and
//!   the demuxer transparently sniff the gzip magic (`1f 8b`) and
//!   inflate; symmetric [`write_svgz`] + a `.svgz` muxer handle the
//!   output side.
//! * `<animate>` / `<set>` snapshot at `t=0` — the animation's `from`
//!   value (or first `values` entry, or `to` when neither is given)
//!   is applied as the static value of the targeted attribute on the
//!   parent element. Matches what most browsers paint on first frame.
//!
//! # Round 5 additions
//!
//! * **CSS 3 Selectors Level 3 subset** in the
//!   [`crate::css`] module. Extends the round-4 cascade with
//!   attribute predicates (`[attr=val]`, `[attr~=val]`, `[attr|=val]`,
//!   `[attr^=val]`, `[attr$=val]`, `[attr*=val]`, bare `[attr]`),
//!   combinators (descendant, `>`, `+`, `~`), and structural
//!   pseudo-classes (`:first-child`, `:last-child`, `:only-child`,
//!   `:nth-child(An+B)`, `:first-of-type`, `:last-of-type`,
//!   `:only-of-type`, `:nth-of-type(An+B)`, `:not(simple)`).
//!   Combinator matching is right-to-left through a lifetime-tied
//!   `MatchContext` ancestor chain; no per-element Vec allocations.
//!   Unsupported pseudo-classes (`:hover`, `:focus`, …) are silently
//!   dropped at parse time so the rest of the rule still applies.
//!
//! # Round 6 additions
//!
//! * **CSS 3 Selectors L3 leftovers** — `:nth-last-child(An+B)` and
//!   `:nth-last-of-type(An+B)` (1-indexed from the end of the parent's
//!   element-children list); `:lang(L)` (BCP 47 dash-match against the
//!   nearest `xml:lang` / `lang` attribute, walked up the ancestor
//!   chain via the existing [`crate::css::MatchContext`] parent
//!   pointers — no extra storage).
//! * **SVG 2 §9.3.2 — `d` as a presentation property.** A CSS
//!   declaration on a `<path>` element (from a `<style>` rule or an
//!   inline `style="..."`) overrides the `d` attribute via the normal
//!   cascade; the value is `none | <string>`, where the string is the
//!   same SVG 1.1 path-data mini-language. `d: none` drops the path
//!   entirely (no scene-graph node).
//!
//! # Round 7 additions
//!
//! * **Typed filter-primitive graph parsing.** Round 2-4 captured
//!   `<filter>` element trees verbatim and round-tripped them, but
//!   never inspected the primitives inside. Round 7 walks each
//!   `<feGaussianBlur>`, `<feOffset>`, `<feFlood>`, `<feComposite>`,
//!   `<feBlend>`, `<feMorphology>` child and folds it into a typed
//!   [`crate::filter::FilterGraph`] stored on the
//!   [`crate::defs::FilterDef`]. Implicit input chaining
//!   (`in` defaults to the previous primitive's `result`, or
//!   `SourceGraphic` for the first) follows W3C Filter Effects §6.2.
//!   Unknown primitives are skipped; the verbatim XML element is still
//!   kept on the def for lossless round-trip emission. A downstream
//!   rasterizer (oxideav-raster) is the consumer of the typed graph.
//!
//! # Round 8 additions
//!
//! * **Long-tail filter primitives.** Round 7 covered the six
//!   most-common primitives; round 8 extends typed parsing to the
//!   long tail flagged in W3C Filter Effects §11–§22:
//!   - **`<feColorMatrix>`** — all four `type=` variants (`matrix` /
//!     `saturate` / `hueRotate` / `luminanceToAlpha`) reduce at parse
//!     time to a flat 4×5 RGBA-bias matrix using the spec's
//!     coefficients (luminance 0.213 / 0.715 / 0.072 for `saturate`
//!     and `hueRotate`, 0.2125 / 0.7154 / 0.0721 for
//!     `luminanceToAlpha`).
//!   - **`<feMerge>`** — captures each `<feMergeNode in="..."/>`
//!     child's input in source order. Missing `in=` falls back to the
//!     previous primitive's `result` (per §6.2).
//!   - **`<feComponentTransfer>`** — per-channel
//!     [`crate::filter::TransferFunction`] resolved from
//!     `<feFuncR/G/B/A>` children with `type="identity|table|discrete|linear|gamma"`.
//!     Channels with no matching child default to `Identity`.
//!   - **`<feDropShadow>`** — stored as a single
//!     [`crate::filter::FilterPrimitive::DropShadow`] variant (the
//!     spec §22 sugar for blur+offset+flood+composite) so the
//!     rasterizer can implement it directly. Defaults `dx=dy=2`,
//!     `stdDeviation=2 2`, `flood-color` opaque black, `flood-opacity` 1.
//!
//!   The verbatim-XML round-trip path continues to preserve every
//!   primitive (including primitives still outside the typed
//!   allowlist) via `PreservedExtras`.
//!
//! # Round 9 additions
//!
//! * **More long-tail filter primitives.** Three more primitives join
//!   the typed-graph allowlist (now 13 of the W3C Filter Effects §11
//!   set):
//!   - **`<feConvolveMatrix>`** — `kernelMatrix` (row-major, length
//!     `order_x * order_y`) plus `divisor` / `bias` / `targetX/Y`,
//!     `edgeMode` (new [`crate::filter::ConvolveEdgeMode`]),
//!     `preserveAlpha`. Per W3C Filter Effects §15.
//!   - **`<feTurbulence>`** — `baseFrequency` (1 or 2 numbers),
//!     `numOctaves`, `seed`, `stitchTiles`, and `type` mapped to
//!     [`crate::filter::TurbulenceKind`]. Per §16.
//!   - **`<feDisplacementMap>`** — `scale` and X / Y channel selectors
//!     mapped to [`crate::filter::ChannelSelector`]. Per §17.
//!
//! # Round 10 additions
//!
//! * **Lighting filter primitives.** The typed-graph allowlist grows
//!   from 13 to 15: `<feDiffuseLighting>` (per W3C Filter Effects §18)
//!   and `<feSpecularLighting>` (§19). Both share a new
//!   [`crate::filter::LightSource`] enum capturing the three SVG
//!   light-source children — `<feDistantLight azimuth elevation>`,
//!   `<fePointLight x y z>` and the eight-attribute form
//!   `<feSpotLight x y z pointsAtX pointsAtY pointsAtZ
//!   specularExponent limitingConeAngle>` — per §18.5 / §18.6 / §18.7.
//!   `surfaceScale` / `diffuseConstant` / `specularConstant` /
//!   `specularExponent` / `kernelUnitLength` / `lighting-color` are all
//!   captured. Missing light-source child collapses to a default
//!   distant light at azimuth=0 / elevation=0; `lighting-color`
//!   defaults to opaque white per §21. The verbatim-XML round-trip
//!   path continues to preserve the elements regardless.
//!
//! # Round 11 additions
//!
//! * **`<feImage>` + `<feTile>`** close the W3C Filter Effects §11
//!   short-name set — the typed-graph allowlist now covers every
//!   short-name primitive (17 total). `<feImage>` records `href` (or
//!   legacy `xlink:href`), `preserveAspectRatio` (full SVG-2 §8.10
//!   keyword set with optional `meet`/`slice` modifier, default
//!   `xMidYMid meet`), and `crossorigin="anonymous|use-credentials"`.
//!   `<feTile>` carries only its `in=` (the primitive's region drives
//!   the tiled-fill area).
//! * **CSS pseudo-elements** (`::before`, `::after`, `::first-letter`,
//!   `::first-line`) parse to a typed [`crate::css::PseudoElement`] on
//!   the carrier selector. Per CSS 3 §3.2 a pseudo-element targets a
//!   synthesised box, so a rule with one never matches a live element
//!   — but it survives the round-trip for a future renderer. CSS 2.1
//!   §5.12.1 single-colon legacy syntax (`:before`, `:after`,
//!   `:first-letter`, `:first-line`) is honoured.
//! * **`@import url(…) [media];`** per CSS 2.1 §6.3 — the URL string
//!   is appended to [`crate::css::Stylesheet::imports`]. Both
//!   `url("foo.css")` and bare-string (`@import "foo.css";`) forms are
//!   accepted; loading external sheets is left to the caller (the
//!   parser deliberately doesn't fetch network resources).
//! * **Stateful / interactive pseudo-classes** (`:hover`, `:focus`,
//!   `:active`, `:checked`, `:visited`, `:link`, `:disabled`,
//!   `:enabled`) parse to [`crate::css::Pseudo::Stateful`] and never
//!   match in a static document. Fixes the round-5 over-match bug
//!   where `.x:hover` collapsed to `.x` because the `:hover` was
//!   silently dropped.
//!
//! # Round 12 additions
//!
//! * **`<script>` graceful capture.** Per HTML5 / SVG 2 §16.2.1,
//!   `<script>` content is raw text — `<` characters in the body
//!   must NOT be parsed as markup. Real-world SVGs frequently embed
//!   unescaped JS like `if (a < b)` without CDATA wrapping; the
//!   round-11 strict-XML parser would either error out or eat the
//!   trailing siblings. Round 12 adds an HTML5-style "script data
//!   state": when the parser opens a `<script>` element it reads raw
//!   bytes until the matching `</script>` close tag, captures the
//!   body verbatim, and stows it on
//!   [`crate::preserved::PreservedExtras::scripts`]. The encoder
//!   re-emits each captured `<script>` with a `<![CDATA[…]]>`
//!   wrapping so a subsequent strict-XML round-trip succeeds without
//!   raw-text mode being needed. **The decoder NEVER executes
//!   scripts** — oxideav has no JS engine.
//! * **`viewBox` + non-uniform `preserveAspectRatio` on the root
//!   `<svg>`.** SVG 2 §8.2 specifies how the canvas-vs-viewBox
//!   aspect-ratio mismatch maps via the `preserveAspectRatio` align
//!   keyword (`xMin/Mid/MaxYMin/Mid/Max` × `meet`/`slice`). The
//!   decoder applies the spec's algorithm (steps 5–14 of §8.2),
//!   computes the equivalent translate+scale, and pre-multiplies it
//!   into [`oxideav_core::VectorFrame::root.transform`] — so a
//!   downstream rasteriser that knows nothing about
//!   `preserveAspectRatio` (one that simply stretches viewBox →
//!   canvas) still produces the spec-correct visual result. The
//!   original keyword pair is preserved verbatim in
//!   [`crate::preserved::PreservedExtras::root_preserve_aspect_ratio`]
//!   so the encoder re-emits it on round-trip. `none` (and the
//!   aspect-match degenerate case) skip the correction — the
//!   renderer's stretch IS the spec's behaviour for those.
//!
//! # Round 13 additions
//!
//! * **SMIL animation re-attachment to the source emit site.** Round
//!   4–12 captured every `<animate>` / `<set>` / `<animateTransform>`
//!   into [`crate::preserved::PreservedExtras::animations`] keyed by
//!   the parent's `id` and re-emitted them at the trailing edge of the
//!   SVG with a `<!-- animation parent: #id -->` comment hint. Round
//!   13 inlines each animation as a child of its declared parent
//!   element when the parent's id was tracked. The decoder records
//!   each id-bearing element's scene-graph tree-path into the new
//!   [`crate::preserved::PreservedExtras::id_paths`] side-channel so
//!   the encoder can find the matching emit site without re-parsing
//!   the source. The original `id="..."` is also surfaced on the
//!   matching `<g>` / `<path>` so downstream tooling can address
//!   the element by source name. Animations whose parent didn't
//!   carry an id fall back to the round-12 trailing-edge emission so
//!   no captured fragment is ever lost.
//! * **`Stylesheet::resolve_imports(fetcher)`.** Round 11 captured
//!   `@import url(…)` URLs into [`crate::css::Stylesheet::imports`]
//!   but never fetched them. Round 13 adds a recursive resolver: the
//!   caller supplies a `Fn(&str) -> Option<Vec<u8>>` (lets the
//!   consumer pick HTTP / FS / cache); fetched bodies are parsed as
//!   CSS and their rules merged so the cascade applies as if the
//!   rules were inline. Cycle detection (visited-URL set) + an
//!   8-hop depth cap (`IMPORT_DEPTH_CAP`) prevent runaway chains.
//!
//! # Round 14 additions
//!
//! * **`<symbol>` + `<use>` viewport mapping.** Round 3 instantiated
//!   `<use href="#sym">` references but skipped the symbol's `viewBox`,
//!   the use's `width` / `height`, and the symbol's
//!   `preserveAspectRatio`. Round 14 wraps the symbol's children in
//!   an inner `Group` carrying the SVG 2 §8.2 viewport transform
//!   between the use's `transform=` / `x` / `y` / `opacity` and the
//!   instantiated content. The use's `width` / `height` fall through
//!   to the symbol's intrinsic `width` / `height` when omitted (per
//!   §5.6). Symbols with no `viewBox` skip the wrap — the use's
//!   `width` / `height` are ignored per spec, matching browser
//!   behaviour. [`crate::defs::SymbolDef`] gains four new fields
//!   (`view_box`, `preserve_aspect_ratio`, `intrinsic_width`,
//!   `intrinsic_height`) populated by [`crate::element::parse_symbol_def`].
//! * **`@font-face` block capture.** Round 11 + 13 routed `@import`
//!   to [`crate::css::Stylesheet::imports`] but tagged every other
//!   `@-rule` (including `@font-face`) for tolerant skip in
//!   `parse_block`. Round 14 routes `@font-face { ... }` to a
//!   dedicated parser that surfaces the descriptor list on the new
//!   [`crate::css::Stylesheet::font_faces`]. Each
//!   [`crate::css::FontFace`] carries a typed `family: String` +
//!   `src: Vec<FontSource>` view plus a `descriptors: HashMap` for
//!   the long tail (`font-weight`, `font-style`, `font-stretch`,
//!   `unicode-range`, `font-display`, …). [`crate::css::FontSource`]
//!   covers both the `url(...) [format(...)]` and `local(...)`
//!   shapes per CSS Fonts L3 §4.3 — a downstream font-resolver can
//!   iterate the list and register the user-supplied fonts before
//!   the cascade matches a `font-family: ...` declaration.
//!
//! # Round 15 additions
//!
//! * **`<image>` element capture (SVG 2 §6).** Inline
//!   `data:image/<mime>;base64,…` URIs are base64-decoded into
//!   [`crate::image::ImageHref::DataUri`] (raw bytes + MIME); external
//!   URLs (`href="logo.png"`) are captured verbatim into
//!   [`crate::image::ImageHref::External`] for caller-side fetching.
//!   `x` / `y` / `width` / `height` / `transform` /
//!   `preserveAspectRatio` are recorded on the typed
//!   [`crate::image::SvgImage`]. Each captured image lives on
//!   [`crate::preserved::PreservedExtras::images`]; the encoder
//!   re-emits them at the trailing edge with a faithful round-trip
//!   (data URIs re-encode from the decoded bytes; external URLs are
//!   preserved as-is). `oxideav_core::Node::Image` requires a
//!   fully-decoded `VideoFrame`, so round 15 deliberately keeps the
//!   raster bytes opaque on the SVG side — the renderer (or a caller
//!   that owns a PNG / JPEG decoder) decodes them lazily.
//! * **CSS `@keyframes` capture (CSS Animations L1).** The
//!   round-11/14 at-rule branch silently dropped `@keyframes`; round
//!   15 routes them to a dedicated parser. Each rule lands on
//!   [`crate::css::Stylesheet::keyframes`] as a typed
//!   [`crate::css::KeyframesRule`] carrying the animation name + a
//!   list of [`crate::css::KeyframeSelector`]s (each one has an
//!   `offset` — `from` / `to` / `<percent>%` per §3 — plus the
//!   declarations to apply at that timeline point). A downstream
//!   animation engine (or the rasteriser's own SMIL-via-`@keyframes`
//!   bridge) can iterate the list without re-parsing the source.
//!
//! # Deferred to round 16+
//!
//! * Actual filter-primitive rasterisation (the typed graph is
//!   pre-rasteriser plumbing; pixel evaluation is oxideav-raster work).
//! * `<marker>` defs + `marker-start` / `marker-mid` / `marker-end`
//!   (needs a `Marker` construct in `oxideav-core`).
//! * `<text>` `textPath` (SVG 2 §11.3) — text-on-path layout via the
//!   existing `oxideav-scribe` shaping path; touches scribe so
//!   deferred from round 14 to keep the round in-crate.
//! * Live evaluation of pseudo-elements (`::before` / `::after`) into
//!   synthesised boxes — also a renderer-side concern (oxideav-raster).
//! * CSS `@media` block honoured (currently the entire block is
//!   skipped; round 16 candidate alongside `@supports`).

pub mod animation;
pub mod color;
pub mod container;
pub mod css;
pub mod decoder;
pub mod defs;
pub mod element;
pub mod encoder;
pub mod filter;
pub mod image;
pub mod parser;
pub mod path_data;
pub mod preserved;
#[cfg(feature = "text")]
pub mod text;
pub mod transform;

pub use decoder::{make_decoder, parse_svg, parse_svg_at, parse_svg_with_extras, CODEC_ID_STR};
pub use encoder::{make_encoder, write_svg, write_svg_with_extras, write_svgz};
pub use preserved::PreservedExtras;

use oxideav_core::{
    CodecCapabilities, CodecId, CodecInfo, CodecRegistry, ContainerRegistry, RuntimeContext,
};

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

/// Unified registration entry point — installs the SVG codec into the
/// codec sub-registry and the SVG container into the container
/// sub-registry of the supplied [`RuntimeContext`].
///
/// Also wired into [`oxideav_meta::register_all`] via the
/// [`oxideav_core::register!`] macro below.
pub fn register(ctx: &mut RuntimeContext) {
    register_codecs(&mut ctx.codecs);
    register_containers(&mut ctx.containers);
}

oxideav_core::register!("svg", register);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_does_not_panic() {
        let mut ctx = RuntimeContext::new();
        register(&mut ctx);
    }

    #[test]
    fn register_via_runtime_context_installs_both_sides() {
        let mut ctx = RuntimeContext::new();
        register(&mut ctx);
        let id = CodecId::new(CODEC_ID_STR);
        assert!(
            ctx.codecs.has_decoder(&id),
            "SVG decoder factory not installed via RuntimeContext"
        );
        assert!(
            ctx.codecs.has_encoder(&id),
            "SVG encoder factory not installed via RuntimeContext"
        );
        assert_eq!(
            ctx.containers.container_for_extension("svg"),
            Some("svg"),
            "SVG container extension not installed via RuntimeContext"
        );
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
