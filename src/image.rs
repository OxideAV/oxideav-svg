//! Round 15 — `<image>` element capture.
//!
//! Per SVG 2 §6, the `<image>` element references an external raster
//! (PNG / JPEG / WebP / …) painted into vector space at
//! `(x, y, width, height)`. The `href` attribute (or legacy
//! `xlink:href`) carries either an external URL or an inline
//! `data:image/<mime>;base64,...` URI per RFC 2397.
//!
//! `oxideav_core::Node` has an `Image(ImageRef)` variant, but it
//! requires a fully-decoded `VideoFrame` — and decoding the raster
//! payload here would pull every image-format crate (oxideav-png,
//! oxideav-jpeg, oxideav-webp, …) into oxideav-svg's tree. That's
//! the wrong direction.
//!
//! Round 15 instead captures the `<image>` element verbatim into
//! [`crate::preserved::PreservedExtras::images`] alongside a typed
//! [`SvgImage`] view that:
//!
//! - Decodes inline base64 payloads to raw bytes + records the MIME
//!   type so a downstream renderer can pick the right decoder.
//! - Records external URLs verbatim for caller-side fetching.
//! - Tracks `(x, y, width, height)` and an optional `transform=` so
//!   the renderer knows where to paint.
//! - Survives a `parse → write_svg_with_extras` round-trip: the
//!   encoder re-emits each captured image as a `<image>` element with
//!   its data URI / external URL intact.

use base64::engine::general_purpose::STANDARD as B64_STANDARD;
use base64::Engine;

use crate::parser::{attr, escape_attr, Element};
use crate::transform::parse_transform;

/// One captured `<image>` element from the source SVG.
///
/// `parent_id` records the nearest ancestor's id (mirrors the
/// `AnimationFragment` shape) — surfaced for downstream tooling that
/// wants to associate the image with the surrounding group.
#[derive(Clone, Debug)]
pub struct SvgImage {
    /// Where the raster bytes live — inline data URI (decoded) or
    /// external URL (caller fetches).
    pub href: ImageHref,
    /// `x="..."` attribute value in user units (defaults to 0 per §6).
    /// This is the best-effort numeric projection; see [`Self::x_raw`]
    /// for the source string that round-trips units / percentages.
    pub x: f32,
    /// `y="..."` attribute value in user units (defaults to 0 per §6).
    pub y: f32,
    /// `width="..."` numeric value, when explicitly set.
    pub width: Option<f32>,
    /// `height="..."` numeric value, when explicitly set.
    pub height: Option<f32>,
    /// Round 382 — the *verbatim* geometry-attribute strings, when the
    /// source set them. `<image>`'s `x` / `y` / `width` / `height` are
    /// `<length>` values (SVG 2 §6, Geometry properties) that may carry
    /// a unit (`px`, `em`, …) or be a percentage of the viewport. The
    /// numeric fields above drop that unit; these slots preserve the
    /// exact source token so a `width="50%"` round-trips as `50%`, not a
    /// semantics-changing `50`. `None` means the source omitted the
    /// attribute (so the round-trip omits it too). The encoder prefers
    /// the raw string when present and falls back to the numeric field.
    pub x_raw: Option<String>,
    pub y_raw: Option<String>,
    pub width_raw: Option<String>,
    pub height_raw: Option<String>,
    /// Optional `transform=` attribute (parsed via the same
    /// `parse_transform` the rest of the SVG decoder uses).
    pub transform: Option<oxideav_core::Transform2D>,
    /// `id="..."` attribute when present — surfaced so the encoder
    /// can re-emit it on round-trip.
    pub id: Option<String>,
    /// Nearest ancestor's id, mirrors [`crate::preserved::AnimationFragment::parent_id`].
    /// Currently unused by the encoder (images emit at the document's
    /// trailing edge with the other extras), but recorded for future
    /// inline re-attachment.
    pub parent_id: Option<String>,
    /// Original `preserveAspectRatio` attribute value, captured
    /// verbatim. SVG 2 §6 lets `<image>` carry its own — independent
    /// of the root `<svg>`.
    pub preserve_aspect_ratio: Option<String>,
    /// Round 235 — SVG 2 §13.10.4 `image-rendering` keyword captured
    /// off the source `<image>` element when present and recognised
    /// (`auto` / `optimizeQuality` / `optimizeSpeed`, canonicalised
    /// to the spec's camelCase). `None` for an absent attribute, an
    /// `inherit` keyword, or an unrecognised token — the §13.10.4
    /// property is inherited so the cascade keeps the inherited value
    /// in those cases. Round-trip is byte-faithful: source
    /// `OPTIMIZEQUALITY` round-trips as `optimizeQuality`.
    pub image_rendering: Option<String>,
    /// Round 382 — SVG 2 §6 (embedded content) `crossorigin` presentation
    /// attribute captured off the source `<image>` element when present
    /// and recognised (`anonymous` / the bare `crossorigin` form / the
    /// `use-credentials` keyword). `None` for an absent attribute or an
    /// unrecognised token. The bare `crossorigin` / empty-value form
    /// canonicalises to `anonymous` on re-emit, matching the HTML
    /// CORS-settings-attribute state machine.
    pub crossorigin: Option<crate::filter::CrossOrigin>,
    /// Round 382 — every source attribute the typed fields above do not
    /// model, captured verbatim in document order so the round-trip
    /// preserves them. This covers the SVG 2 §6 `<image>` core / styling
    /// / conditional-processing attributes the crate does not otherwise
    /// interpret — `class`, `style`, `opacity`, `clip-path`, `mask`,
    /// `filter`, `visibility`, `requiredExtensions`, `systemLanguage`,
    /// `xlink:title`, `data-*`, and so on. The `href` / `xlink:href`
    /// pair is *not* stored here (it is re-derived from [`Self::href`]);
    /// neither are the fields with dedicated slots.
    pub extra_attrs: Vec<(String, String)>,
}

/// An `<image>` element's `href` resolved into one of the two
/// encodable shapes: an inline base64-decoded blob or an external URL.
#[derive(Clone, Debug)]
pub enum ImageHref {
    /// Inline `data:image/<mime>;base64,<payload>` URI per RFC 2397.
    /// `mime` is the full MIME type string (e.g. `"image/png"`); the
    /// trailing `;base64` flag and any other attributes are stripped.
    DataUri {
        /// Full MIME type — `"image/png"`, `"image/jpeg"`, … Empty
        /// when the source URI omits it (RFC 2397 says the default is
        /// `"text/plain;charset=US-ASCII"`, but for the `<image>` use
        /// case we still record the empty string and let the
        /// downstream decoder sniff the bytes).
        mime: String,
        /// Decoded raster payload. Round-trips back to the same
        /// base64 string when re-encoded by [`SvgImage::to_href_attr`].
        bytes: Vec<u8>,
    },
    /// External URL — captured verbatim for caller-side fetching. The
    /// SVG decoder deliberately does NOT fetch network resources.
    External(String),
}

impl SvgImage {
    /// Try to parse an `<image>` [`Element`] into a typed [`SvgImage`].
    /// Returns `None` when the element has no `href` / `xlink:href`
    /// (no resolvable payload) or when the data-URI base64 is
    /// malformed (we keep the structural extras tolerant — a broken
    /// inline image shouldn't kill the whole document).
    pub fn from_element(el: &Element, parent_id: Option<&str>) -> Option<Self> {
        let raw = attr(el, "href").or_else(|| attr(el, "xlink:href"))?.trim();
        let href = parse_image_href(raw)?;
        let x = parse_optional_number(attr(el, "x")).unwrap_or(0.0);
        let y = parse_optional_number(attr(el, "y")).unwrap_or(0.0);
        let width = parse_optional_number(attr(el, "width"));
        let height = parse_optional_number(attr(el, "height"));
        // Round 382 — capture the verbatim geometry strings so a
        // unit-bearing / percentage `<length>` survives the round-trip.
        // Only record a raw slot when the source token differs from the
        // canonical numeric re-emit, so an initial-value document doesn't
        // bloat and a plain `x="10"` still emits via the numeric path.
        let raw_geom = |name: &str, num: f32| -> Option<String> {
            let src = attr(el, name)?.trim();
            if src.is_empty() {
                return None;
            }
            if src == trim_float(num) {
                None
            } else {
                Some(src.to_string())
            }
        };
        let x_raw = raw_geom("x", x);
        let y_raw = raw_geom("y", y);
        let width_raw = width.and_then(|w| raw_geom("width", w));
        let height_raw = height.and_then(|h| raw_geom("height", h));
        let transform = match attr(el, "transform") {
            Some(s) => parse_transform(s).ok(),
            None => None,
        };
        let id = attr(el, "id").map(str::to_string);
        let preserve_aspect_ratio = attr(el, "preserveAspectRatio").map(str::to_string);
        // Round 235 — capture an `image-rendering=` attribute when the
        // value resolves to a §13.10.4 keyword. Absent / `inherit` /
        // unknown payloads leave the slot empty so the cascade keeps
        // the inherited value and the round-trip doesn't bloat with a
        // redundant `image-rendering="auto"` on an initial-value
        // document.
        let image_rendering = attr(el, "image-rendering").and_then(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("inherit") {
                return None;
            }
            let ir = crate::element::ImageRendering::parse_keyword(trimmed)?;
            Some(ir.as_canonical_str().to_string())
        });
        // Round 382 — capture the `crossorigin` CORS-settings attribute
        // (SVG 2 §6). The bare `crossorigin` form parses as the empty
        // string, which the HTML rules map to `anonymous`.
        let crossorigin = attr(el, "crossorigin").and_then(crate::filter::CrossOrigin::parse_attr);
        // Round 382 — sweep up every attribute the typed slots above
        // don't model, preserving document order. `href` / `xlink:href`
        // are re-derived from `self.href`; the modelled attributes have
        // dedicated slots. `crossorigin` is only skipped when it parsed
        // into a typed keyword — an *invalid* `crossorigin` token (which
        // records no binding) still round-trips through `extra_attrs`
        // rather than being silently dropped.
        let crossorigin_modelled = crossorigin.is_some();
        let extra_attrs: Vec<(String, String)> = el
            .attrs
            .iter()
            .filter(|(k, _)| {
                let has_dedicated_slot = matches!(
                    k.as_str(),
                    "href"
                        | "xlink:href"
                        | "x"
                        | "y"
                        | "width"
                        | "height"
                        | "transform"
                        | "id"
                        | "preserveAspectRatio"
                        | "image-rendering"
                ) || (k == "crossorigin" && crossorigin_modelled);
                !has_dedicated_slot
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Some(Self {
            href,
            x,
            y,
            width,
            height,
            x_raw,
            y_raw,
            width_raw,
            height_raw,
            transform,
            id,
            parent_id: parent_id.map(str::to_string),
            preserve_aspect_ratio,
            image_rendering,
            crossorigin,
            extra_attrs,
        })
    }

    /// Re-encode the image's `href` back into the textual form the
    /// SVG `<image>` attribute expects:
    /// `data:<mime>;base64,<payload>` for inline, or the captured URL
    /// verbatim for external.
    pub fn to_href_attr(&self) -> String {
        match &self.href {
            ImageHref::DataUri { mime, bytes } => {
                let encoded = B64_STANDARD.encode(bytes);
                if mime.is_empty() {
                    // RFC 2397 default — but for <image> this is
                    // unusual; emit it with the bare `data:base64,`
                    // form (still valid per the RFC).
                    format!("data:;base64,{}", encoded)
                } else {
                    format!("data:{};base64,{}", mime, encoded)
                }
            }
            ImageHref::External(url) => url.clone(),
        }
    }

    /// Serialise the image as a single `<image .../>` element. Indent
    /// is the leading whitespace; the trailing newline is included.
    pub fn write_to(&self, out: &mut String, indent: &str) {
        out.push_str(indent);
        out.push_str("<image");
        if let Some(id) = &self.id {
            out.push_str(&format!(" id=\"{}\"", escape_attr(id)));
        }
        // Round 382 — prefer the verbatim source token (unit /
        // percentage preserving) when captured, else fall back to the
        // canonical numeric re-emit. `x` / `y` still suppress an
        // initial-value `0` when no raw token was recorded.
        if let Some(raw) = &self.x_raw {
            out.push_str(&format!(" x=\"{}\"", escape_attr(raw)));
        } else if self.x != 0.0 {
            out.push_str(&format!(" x=\"{}\"", trim_float(self.x)));
        }
        if let Some(raw) = &self.y_raw {
            out.push_str(&format!(" y=\"{}\"", escape_attr(raw)));
        } else if self.y != 0.0 {
            out.push_str(&format!(" y=\"{}\"", trim_float(self.y)));
        }
        if let Some(raw) = &self.width_raw {
            out.push_str(&format!(" width=\"{}\"", escape_attr(raw)));
        } else if let Some(w) = self.width {
            out.push_str(&format!(" width=\"{}\"", trim_float(w)));
        }
        if let Some(raw) = &self.height_raw {
            out.push_str(&format!(" height=\"{}\"", escape_attr(raw)));
        } else if let Some(h) = self.height {
            out.push_str(&format!(" height=\"{}\"", trim_float(h)));
        }
        if let Some(t) = &self.transform {
            if !t.is_identity() {
                out.push_str(&format!(" transform=\"{}\"", format_transform(t)));
            }
        }
        if let Some(par) = &self.preserve_aspect_ratio {
            out.push_str(&format!(" preserveAspectRatio=\"{}\"", escape_attr(par)));
        }
        // Round 235 — SVG 2 §13.10.4 `image-rendering`. Emit only when
        // a canonical keyword was captured (absent / `inherit` / unknown
        // tokens leave the slot empty so the round-trip doesn't bloat
        // an initial-value document).
        if let Some(ir) = &self.image_rendering {
            out.push_str(&format!(" image-rendering=\"{}\"", escape_attr(ir)));
        }
        // Round 382 — SVG 2 §6 `crossorigin`. Emit the canonical keyword
        // (the bare / empty-value form round-trips as `anonymous`).
        if let Some(co) = self.crossorigin {
            out.push_str(&format!(" crossorigin=\"{}\"", co.as_canonical_str()));
        }
        // Round 382 — re-emit the verbatim-captured attributes the typed
        // slots don't model (`class`, `style`, `opacity`, `clip-path`,
        // `mask`, `filter`, `visibility`, `requiredExtensions`,
        // `systemLanguage`, `xlink:title`, …) in their original document
        // order. Values are attribute-escaped on the way out.
        for (k, v) in &self.extra_attrs {
            out.push_str(&format!(" {}=\"{}\"", k, escape_attr(v)));
        }
        let href_value = self.to_href_attr();
        out.push_str(&format!(" href=\"{}\"", escape_attr(&href_value)));
        out.push_str("/>\n");
    }
}

/// Parse the contents of an `href` / `xlink:href` attribute on
/// `<image>`. Returns `None` when the value is empty or the data URI
/// is malformed (no `,` separator, bad base64 payload).
pub fn parse_image_href(raw: &str) -> Option<ImageHref> {
    let r = raw.trim();
    if r.is_empty() {
        return None;
    }
    if let Some(rest) = r.strip_prefix("data:").or_else(|| r.strip_prefix("DATA:")) {
        let comma = rest.find(',')?;
        let header = &rest[..comma];
        let payload = &rest[comma + 1..];
        // RFC 2397 header form: `<mime>[;<param>]*[;base64]`. We
        // only support the base64 case for `<image>` — text-payload
        // images are vanishingly rare and would need URL decoding.
        let lower_header = header.to_ascii_lowercase();
        let is_base64 = lower_header
            .split(';')
            .any(|p| p.trim().eq_ignore_ascii_case("base64"));
        if !is_base64 {
            // Non-base64 data URIs are out of scope — capture as
            // External so the value at least round-trips verbatim.
            return Some(ImageHref::External(r.to_string()));
        }
        // The MIME is the first segment up to the first `;`. Strip
        // any whitespace + the `;base64` flag.
        let mime = header.split(';').next().unwrap_or("").trim().to_string();
        // RFC 4648: tolerate stray whitespace inside the payload (line
        // breaks are common in editor exports).
        let cleaned: String = payload
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect();
        let bytes = B64_STANDARD.decode(cleaned.as_bytes()).ok()?;
        return Some(ImageHref::DataUri { mime, bytes });
    }
    Some(ImageHref::External(r.to_string()))
}

fn parse_optional_number(v: Option<&str>) -> Option<f32> {
    let s = v?.trim();
    if s.is_empty() {
        return None;
    }
    // Reuse `parse_number`'s lenient-suffix handling — `<image
    // width="100px">` is common.
    crate::element::parse_number(Some(s), 0.0).ok()
}

fn trim_float(v: f32) -> String {
    // Keep three decimals max, drop trailing zeros + dot.
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s.is_empty() {
        "0".to_string()
    } else {
        s
    }
}

fn format_transform(t: &oxideav_core::Transform2D) -> String {
    format!(
        "matrix({} {} {} {} {} {})",
        trim_float(t.a),
        trim_float(t.b),
        trim_float(t.c),
        trim_float(t.d),
        trim_float(t.e),
        trim_float(t.f),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Element;

    fn elem(name: &str, attrs: &[(&str, &str)]) -> Element {
        Element {
            name: name.to_string(),
            attrs: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            children: Vec::new(),
        }
    }

    #[test]
    fn parses_external_href() {
        let r = parse_image_href("logo.png").unwrap();
        match r {
            ImageHref::External(s) => assert_eq!(s, "logo.png"),
            _ => panic!("expected external href"),
        }
    }

    #[test]
    fn parses_data_uri_png_one_pixel() {
        // 1x1 transparent PNG, base64 of the canonical 67-byte PNG.
        let src = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkAAIAAAoAAv/lxKUAAAAASUVORK5CYII=";
        let r = parse_image_href(src).unwrap();
        match r {
            ImageHref::DataUri { mime, bytes } => {
                assert_eq!(mime, "image/png");
                // PNG signature.
                assert!(bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47]));
            }
            _ => panic!("expected data URI"),
        }
    }

    #[test]
    fn data_uri_without_base64_flag_falls_through_to_external() {
        let r = parse_image_href("data:text/plain,hello").unwrap();
        match r {
            ImageHref::External(s) => assert!(s.starts_with("data:text/plain,")),
            _ => panic!("expected external (non-base64 data URI)"),
        }
    }

    #[test]
    fn round_trip_external_href() {
        let img = SvgImage::from_element(
            &elem(
                "image",
                &[
                    ("href", "logo.png"),
                    ("x", "10"),
                    ("y", "20"),
                    ("width", "100"),
                    ("height", "50"),
                ],
            ),
            None,
        )
        .unwrap();
        assert_eq!(img.x, 10.0);
        assert_eq!(img.y, 20.0);
        assert_eq!(img.width, Some(100.0));
        assert_eq!(img.height, Some(50.0));
        assert_eq!(img.to_href_attr(), "logo.png");
    }

    #[test]
    fn round_trip_data_uri_preserves_bytes() {
        let original = b"\x89PNG\r\n\x1a\nfake-png-bytes";
        let encoded = B64_STANDARD.encode(original);
        let src = format!("data:image/png;base64,{}", encoded);
        let img = SvgImage::from_element(
            &elem("image", &[("href", &src), ("width", "1"), ("height", "1")]),
            None,
        )
        .unwrap();
        match &img.href {
            ImageHref::DataUri { mime, bytes } => {
                assert_eq!(mime, "image/png");
                assert_eq!(bytes, original);
            }
            _ => panic!(),
        }
        let re = img.to_href_attr();
        assert_eq!(re, src);
    }

    #[test]
    fn xlink_href_is_honoured() {
        let img = SvgImage::from_element(&elem("image", &[("xlink:href", "x.png")]), None).unwrap();
        assert!(matches!(img.href, ImageHref::External(s) if s == "x.png"));
    }

    #[test]
    fn missing_href_returns_none() {
        let img = SvgImage::from_element(&elem("image", &[("width", "1")]), None);
        assert!(img.is_none());
    }
}
