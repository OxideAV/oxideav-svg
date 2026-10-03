//! The image-crate contract's root functions, specialised to a vector
//! format.
//!
//! * [`probe`] / [`info`] work standalone and allocation-free /
//!   header-only respectively.
//! * [`parse`] (re-exported from [`crate::decoder`]) is the real
//!   standalone decode: it yields an [`SvgDocument`], the vector scene
//!   graph. [`write`] is the inverse.
//! * The raster verbs — [`decode`], [`decode_with`], [`decode_rgb8`],
//!   [`decode_rgba8`], [`decode_from`], [`encode`], [`encode_rgb8`],
//!   [`encode_rgba8`], [`encode_to`] — exist so the vocabulary is
//!   complete and callers can be generic over image crates, but they
//!   answer [`SvgError::Unsupported`]: rasterising SVG is
//!   `oxideav-raster`'s job through the framework (the framework
//!   `Decoder` emits a vector frame, which `oxideav-raster` paints), and
//!   an SVG is not a raster target. `decode*` still parse the document
//!   first, so syntax errors and [`DecodeOptions`] limits surface with
//!   their own error variants before the `Unsupported` answer.

use std::io::{Read, Write};

use crate::decoder::parse_document_xml;
use crate::error::{Result, SvgError};
use crate::length::{parse_length, Length, LengthUnit};
use crate::model::{SvgDocument, ViewBox};
use crate::options::{DecodeOptions, EncodeOptions};
use crate::parser::{attr, is_gzip, parse_root_start_tag, tag_local, Node as XmlNode};
use crate::picture::{ImageInfo, RgbImage, RgbaImage, SvgImage};

use crate::decoder::parse;

/// How many leading bytes [`probe`] inspects for the `<svg` start tag.
pub const PROBE_WINDOW: usize = 4096;

/// Signature sniff: `true` for a gzip stream (an `.svgz`; the gzip
/// magic `1f 8b` cannot be looked through without inflating, so any
/// gzip member probes positive) or for text whose first
/// [`PROBE_WINDOW`] bytes — after an optional UTF-8 BOM, whitespace,
/// XML declaration, comments and `<!DOCTYPE>` — open an `<svg` element
/// (case-insensitive local name, with or without a namespace prefix).
/// Allocation-free; `false` on short or unrelated input.
pub fn probe(bytes: &[u8]) -> bool {
    if is_gzip(bytes) {
        return true;
    }
    let head = &bytes[..bytes.len().min(PROBE_WINDOW)];
    let head = head.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(head);
    let mut i = 0usize;
    while i < head.len() {
        // Skip whitespace.
        while i < head.len() && matches!(head[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        if i >= head.len() || head[i] != b'<' {
            return false;
        }
        let rest = &head[i..];
        if rest.starts_with(b"<?") {
            match find(rest, b"?>") {
                Some(e) => i += e + 2,
                None => return false,
            }
        } else if rest.starts_with(b"<!--") {
            match find(rest, b"-->") {
                Some(e) => i += e + 3,
                None => return false,
            }
        } else if rest.starts_with(b"<!") {
            // <!DOCTYPE …> possibly with an internal subset `[ … ]`.
            let mut j = 2;
            let mut bracket = 0usize;
            loop {
                if j >= rest.len() {
                    return false;
                }
                match rest[j] {
                    b'[' => bracket += 1,
                    b']' => bracket = bracket.saturating_sub(1),
                    b'>' if bracket == 0 => break,
                    _ => {}
                }
                j += 1;
            }
            i += j + 1;
        } else {
            // A start tag: `<svg`, `<svg:svg`, `<SVG …`.
            let mut j = 1;
            let name_start = j;
            while j < rest.len() && !matches!(rest[j], b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/') {
                j += 1;
            }
            let name = &rest[name_start..j];
            let local = match name.iter().rposition(|&b| b == b':') {
                Some(p) => &name[p + 1..],
                None => name,
            };
            return local.eq_ignore_ascii_case(b"svg");
        }
    }
    false
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Header-only read: the canvas a rasteriser would produce for the
/// document, plus `view_box`, the user-unit size, animation and
/// compression flags. Reads the root `<svg>` start tag only (an
/// `.svgz` is inflated first, under the default [`DecodeOptions`]
/// `max_bytes`); the document body is never built. See [`ImageInfo`]
/// for the CSS sizing rules.
pub fn info(bytes: &[u8]) -> Result<ImageInfo> {
    info_with(bytes, &DecodeOptions::default())
}

/// [`info`] with explicit limits (`max_bytes` bounds the input and the
/// inflated `.svgz` size; the other limits do not apply to a
/// header-only read).
pub fn info_with(bytes: &[u8], opts: &DecodeOptions) -> Result<ImageInfo> {
    if let Some(max) = opts.max_bytes {
        if bytes.len() as u64 > max {
            return Err(SvgError::limit(format!(
                "SVG: input of {} bytes exceeds max_bytes {max}",
                bytes.len()
            )));
        }
    }
    let compressed = is_gzip(bytes);
    let inflated;
    let raw: &[u8] = if compressed {
        inflated = crate::parser::inflate_gzip_capped(bytes, opts.inflate_cap())?;
        &inflated
    } else {
        bytes
    };
    let text = crate::parser::decode_utf8_lossy_stripping_bom(raw);
    let root = parse_root_start_tag(&text)?
        .ok_or_else(|| SvgError::invalid("SVG: missing <svg> root element"))?;

    let view_box = match attr(&root, "viewBox") {
        Some(v) => Some(parse_view_box_attr(v)?),
        None => None,
    };
    let w_attr = attr(&root, "width")
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let h_attr = attr(&root, "height")
        .map(str::trim)
        .filter(|s| !s.is_empty());

    // The user-unit size exactly as `parse` stores it (numeric prefix,
    // percentages → view box).
    let user_width = user_unit_or_default(w_attr, view_box.map(|vb| vb.width).unwrap_or(0.0))?;
    let user_height = user_unit_or_default(h_attr, view_box.map(|vb| vb.height).unwrap_or(0.0))?;

    // CSS px at 96 dpi (SVG 2 §8.6 / CSS Values L4 §6.1): absolute
    // units convert; relative ones fall back to the view box; a missing
    // axis follows the view-box aspect ratio.
    let w_px = w_attr.and_then(absolute_px);
    let h_px = h_attr.and_then(absolute_px);
    let (cw, ch) = match (w_px, h_px, view_box) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some(vb)) if vb.width > 0.0 => (w, w * vb.height / vb.width),
        (None, Some(h), Some(vb)) if vb.height > 0.0 => (h * vb.width / vb.height, h),
        (Some(w), None, _) => (w, 0.0),
        (None, Some(h), _) => (0.0, h),
        (None, None, Some(vb)) => (vb.width, vb.height),
        (None, None, None) => (0.0, 0.0),
    };
    let width = px_to_u32(cw);
    let height = px_to_u32(ch);

    let mut out = ImageInfo::new(width, height);
    out.user_width = user_width;
    out.user_height = user_height;
    out.view_box = view_box;
    out.compressed = compressed;
    out.has_xmp = has_element(&text, "xmpmeta") || has_element(&text, "RDF");
    out.animated = has_element(&text, "animate")
        || has_element(&text, "set")
        || has_element(&text, "animateTransform")
        || has_element(&text, "animateMotion")
        || text.contains("@keyframes");
    Ok(out)
}

/// `true` when `text` opens an element whose local name is `local`
/// (any namespace prefix, exact case) — a byte scan over start tags,
/// not a parse.
fn has_element(text: &str, local: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(off) = find(&bytes[i..], b"<") {
        let start = i + off + 1;
        let mut j = start;
        while j < bytes.len() && !matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/') {
            j += 1;
        }
        let name = &bytes[start..j];
        let name = match name.iter().rposition(|&b| b == b':') {
            Some(p) => &name[p + 1..],
            None => name,
        };
        if name == local.as_bytes() {
            return true;
        }
        i = start;
    }
    false
}

fn parse_view_box_attr(s: &str) -> Result<ViewBox> {
    let nums: Result<Vec<f32>> = s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(|n| {
            n.parse::<f32>()
                .map_err(|_| SvgError::invalid("SVG: malformed viewBox number"))
        })
        .collect();
    let nums = nums?;
    if nums.len() != 4 {
        return Err(SvgError::invalid("SVG: viewBox must be 4 numbers"));
    }
    Ok(ViewBox::new(nums[0], nums[1], nums[2], nums[3]))
}

fn user_unit_or_default(v: Option<&str>, default: f32) -> Result<f32> {
    match v {
        None => Ok(default),
        Some(s) if s.ends_with('%') => Ok(default),
        Some(s) => crate::element::parse_number(Some(s), default),
    }
}

/// The CSS-px value of an absolute `<length>` (or a bare number, which
/// is a user unit = 1 px); `None` for percentages, font-relative and
/// viewport-relative units, or unparsable text.
fn absolute_px(s: &str) -> Option<f32> {
    let Length { value, unit } = parse_length(s).ok()?;
    let px = match unit {
        LengthUnit::UserUnit | LengthUnit::Px => value,
        LengthUnit::Pt => value * 96.0 / 72.0,
        LengthUnit::Pc => value * 16.0,
        LengthUnit::Cm => value * 96.0 / 2.54,
        LengthUnit::Mm => value * 96.0 / 25.4,
        LengthUnit::Q => value * 96.0 / 101.6,
        LengthUnit::In => value * 96.0,
        _ => return None,
    };
    if px.is_finite() && px >= 0.0 {
        Some(px)
    } else {
        None
    }
}

/// Round a CSS-px canvas extent up to whole pixels, saturating at
/// `u32::MAX`.
fn px_to_u32(px: f32) -> u32 {
    if !px.is_finite() || px <= 0.0 {
        0
    } else if px >= u32::MAX as f32 {
        u32::MAX
    } else {
        px.ceil() as u32
    }
}

/// The contract's raster decode. Parses the document under
/// [`DecodeOptions::default()`] (so malformed input is
/// [`SvgError::InvalidData`] and over-limit input
/// [`SvgError::LimitExceeded`]) and then answers
/// [`SvgError::Unsupported`]: rasterising SVG needs `oxideav-raster`
/// through the framework. Use [`parse`] for the vector document.
pub fn decode(bytes: &[u8]) -> Result<SvgImage> {
    decode_with(bytes, &DecodeOptions::default())
}

/// [`decode`] with explicit options. The canvas-geometry limits
/// (`max_width` / `max_height` / `max_pixels`) are checked against the
/// [`info`] canvas before the `Unsupported` answer, so a caller
/// enforcing a pixel budget sees `LimitExceeded` exactly where a
/// rasterising crate would report it.
pub fn decode_with(bytes: &[u8], opts: &DecodeOptions) -> Result<SvgImage> {
    let nodes = parse_document_xml(bytes, opts)?;
    let has_root = nodes.iter().any(|n| match n {
        XmlNode::Element(e) => tag_local(&e.name) == "svg",
        _ => false,
    });
    if !has_root {
        return Err(SvgError::invalid("SVG: missing <svg> root element"));
    }
    let info = info_with(bytes, opts)?;
    opts.check_canvas(info.width, info.height)?;
    Err(unsupported_raster())
}

/// [`decode`] to tightly packed RGB8 — always [`SvgError::Unsupported`]
/// after the same validation as [`decode`].
pub fn decode_rgb8(bytes: &[u8]) -> Result<RgbImage> {
    decode(bytes).map(|img| RgbImage::new(img.width, img.height, img.to_rgb8()))
}

/// [`decode`] to tightly packed RGBA8 — always [`SvgError::Unsupported`]
/// after the same validation as [`decode`].
pub fn decode_rgba8(bytes: &[u8]) -> Result<RgbaImage> {
    decode(bytes).map(|img| RgbaImage::new(img.width, img.height, img.to_rgba8()))
}

/// Read `r` to end and [`decode`] it (I/O failures are
/// [`SvgError::Io`]).
pub fn decode_from<R: Read>(mut r: R) -> Result<SvgImage> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)?;
    decode(&buf)
}

/// Read `r` to end and [`parse`] it (I/O failures are [`SvgError::Io`]).
/// The vector counterpart of [`decode_from`].
pub fn parse_from<R: Read>(mut r: R) -> Result<SvgDocument> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)?;
    parse(&buf)
}

/// The contract's raster encode: always [`SvgError::Unsupported`]. An
/// SVG document is vector content; it is not a target a raster can be
/// written into (embedding pixels as a base64 `<image>` would need a
/// raster codec this crate does not have). Use [`write`] /
/// [`write_with`](crate::write_with) for an [`SvgDocument`].
pub fn encode(image: &SvgImage, _opts: &EncodeOptions) -> Result<Vec<u8>> {
    image.validate()?;
    Err(SvgError::unsupported(
        "SVG is a vector format: a raster cannot be encoded as SVG (write an SvgDocument with oxideav_svg::write)",
    ))
}

/// Raster encode from packed RGB8 — always [`SvgError::Unsupported`]
/// (geometry mismatches are [`SvgError::InvalidData`] first).
pub fn encode_rgb8(width: u32, height: u32, rgb: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    let img = SvgImage::from_rgb8(width, height, rgb.to_vec())?;
    encode(&img, opts)
}

/// Raster encode from packed RGBA8 — always [`SvgError::Unsupported`]
/// (geometry mismatches are [`SvgError::InvalidData`] first).
pub fn encode_rgba8(width: u32, height: u32, rgba: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    let img = SvgImage::from_rgba8(width, height, rgba.to_vec())?;
    encode(&img, opts)
}

/// Streaming [`encode`] — always [`SvgError::Unsupported`].
pub fn encode_to<W: Write>(image: &SvgImage, opts: &EncodeOptions, mut w: W) -> Result<()> {
    let bytes = encode(image, opts)?;
    w.write_all(&bytes)?;
    Ok(())
}

/// Streaming [`write`](crate::write): serialise `doc` with `opts` into
/// `w`. The vector counterpart of [`encode_to`].
pub fn write_to<W: Write>(doc: &SvgDocument, opts: &EncodeOptions, mut w: W) -> Result<()> {
    let bytes = crate::encoder::write_with(doc, opts)?;
    w.write_all(&bytes)?;
    Ok(())
}

fn unsupported_raster() -> SvgError {
    SvgError::unsupported(
        "rasterising SVG needs oxideav-raster through the framework (parse the document with oxideav_svg::parse)",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picture::PixelFormat;

    const MIN: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"/>"#;

    #[test]
    fn probe_accepts_svg_shapes() {
        assert!(probe(MIN));
        assert!(probe(b"<?xml version=\"1.0\"?>\n<svg/>"));
        assert!(probe(
            b"\xEF\xBB\xBF  <!-- c --> <!DOCTYPE svg PUBLIC \"x\" \"y\"> <svg>"
        ));
        assert!(probe(
            b"<svg:svg xmlns:svg=\"http://www.w3.org/2000/svg\"/>"
        ));
        assert!(probe(b"<SVG>"));
        assert!(probe(&[0x1f, 0x8b, 0x08, 0, 0, 0, 0, 0, 0, 3]));
    }

    #[test]
    fn probe_rejects_other_input() {
        assert!(!probe(b""));
        assert!(!probe(b"<"));
        assert!(!probe(b"<sv"));
        assert!(!probe(b"<html><svg/></html>"));
        assert!(!probe(b"\x89PNG\r\n\x1a\n"));
        assert!(!probe(b"<?xml version=\"1.0\"?>"));
        assert!(!probe(b"   "));
        assert!(!probe(b"<svgx/>"));
    }

    #[test]
    fn info_resolves_css_units_at_96_dpi() {
        let i = info(MIN).unwrap();
        assert_eq!((i.width, i.height), (20, 10));
        assert_eq!(i.format, PixelFormat::Rgba);
        assert_eq!(i.frames, 1);
        assert!(i.has_alpha);
        assert!(!i.compressed);
        let i = info(br#"<svg width="1in" height="2.54cm"/>"#).unwrap();
        assert_eq!((i.width, i.height), (96, 96));
        let i = info(br#"<svg width="72pt" height="6pc"/>"#).unwrap();
        assert_eq!((i.width, i.height), (96, 96));
        let i = info(br#"<svg width="25.4mm" height="101.6Q"/>"#).unwrap();
        assert_eq!((i.width, i.height), (96, 96));
        let i = info(br#"<svg width="10.2px" height="3"/>"#).unwrap();
        assert_eq!((i.width, i.height), (11, 3));
    }

    #[test]
    fn info_falls_back_to_view_box() {
        let i = info(br#"<svg viewBox="0 0 64 32"/>"#).unwrap();
        assert_eq!((i.width, i.height), (64, 32));
        assert_eq!(i.view_box, Some(ViewBox::new(0.0, 0.0, 64.0, 32.0)));
        assert_eq!((i.user_width, i.user_height), (64.0, 32.0));
        // Percentages are not intrinsic: view box wins.
        let i = info(br#"<svg width="100%" height="50%" viewBox="0 0 64 32"/>"#).unwrap();
        assert_eq!((i.width, i.height), (64, 32));
        // One absolute axis: the other follows the view-box ratio.
        let i = info(br#"<svg width="128" viewBox="0 0 64 32"/>"#).unwrap();
        assert_eq!((i.width, i.height), (128, 64));
        let i = info(br#"<svg height="2em" viewBox="0 0 64 32"/>"#).unwrap();
        assert_eq!((i.width, i.height), (64, 32));
        // `parse` keeps the numeric prefix as user units.
        let i = info(br#"<svg width="10cm" height="5cm"/>"#).unwrap();
        assert_eq!((i.user_width, i.user_height), (10.0, 5.0));
        assert_eq!((i.width, i.height), (378, 189));
        // No intrinsic size at all.
        let i = info(br#"<svg/>"#).unwrap();
        assert_eq!((i.width, i.height), (0, 0));
    }

    #[test]
    fn info_reads_only_the_root_tag() {
        let doc = br#"<?xml version="1.0"?><!DOCTYPE svg><!-- hi --><svg width="3" height="4"><rect width="1" height="1"/><metadata><x:xmpmeta xmlns:x="adobe:ns:meta/"/></metadata><animate attributeName="x"/></svg>"#;
        let i = info(doc).unwrap();
        assert_eq!((i.width, i.height), (3, 4));
        assert!(i.has_xmp);
        assert!(i.animated);
        let i = info(MIN).unwrap();
        assert!(!i.has_xmp && !i.animated);
        // Nested root behind a wrapper element (lenient, like `parse`).
        let i = info(br#"<wrapper><svg width="5" height="6"/></wrapper>"#).unwrap();
        assert_eq!((i.width, i.height), (5, 6));
        assert!(matches!(
            info(b"<html></html>"),
            Err(SvgError::InvalidData(_))
        ));
        assert!(matches!(info(b""), Err(SvgError::InvalidData(_))));
        assert!(matches!(
            info(br#"<svg viewBox="a b"/>"#),
            Err(SvgError::InvalidData(_))
        ));
    }

    #[test]
    fn info_inflates_svgz() {
        let gz = crate::parser::deflate_gzip(MIN).unwrap();
        let i = info(&gz).unwrap();
        assert_eq!((i.width, i.height), (20, 10));
        assert!(i.compressed);
        let small = DecodeOptions::default().with_max_bytes(16u64);
        assert!(matches!(
            info_with(&gz, &small),
            Err(SvgError::LimitExceeded(_))
        ));
    }

    #[test]
    fn raster_verbs_are_unsupported_after_validation() {
        assert!(matches!(decode(MIN), Err(SvgError::Unsupported(_))));
        assert!(matches!(decode_rgb8(MIN), Err(SvgError::Unsupported(_))));
        assert!(matches!(decode_rgba8(MIN), Err(SvgError::Unsupported(_))));
        assert!(matches!(decode_from(MIN), Err(SvgError::Unsupported(_))));
        assert!(matches!(decode(b"<html/>"), Err(SvgError::InvalidData(_))));
        assert!(matches!(decode(b"<svg"), Err(SvgError::InvalidData(_))));
        let tiny = DecodeOptions::default().with_max_pixels(10u64);
        assert!(matches!(
            decode_with(MIN, &tiny),
            Err(SvgError::LimitExceeded(_))
        ));
        let img = SvgImage::from_rgba8(1, 1, vec![0; 4]).unwrap();
        let o = EncodeOptions::default();
        assert!(matches!(encode(&img, &o), Err(SvgError::Unsupported(_))));
        assert!(matches!(
            encode_rgb8(1, 1, &[0; 3], &o),
            Err(SvgError::Unsupported(_))
        ));
        assert!(matches!(
            encode_rgba8(1, 1, &[0; 4], &o),
            Err(SvgError::Unsupported(_))
        ));
        assert!(matches!(
            encode_rgba8(1, 1, &[0; 3], &o),
            Err(SvgError::InvalidData(_))
        ));
        let mut sink = Vec::new();
        assert!(matches!(
            encode_to(&img, &o, &mut sink),
            Err(SvgError::Unsupported(_))
        ));
        assert!(sink.is_empty());
    }

    #[test]
    fn vector_streaming_counterparts() {
        let doc = parse_from(MIN).unwrap();
        assert_eq!((doc.width, doc.height), (20.0, 10.0));
        let mut out = Vec::new();
        write_to(&doc, &EncodeOptions::default(), &mut out).unwrap();
        assert_eq!(out, crate::encoder::write(&doc));
        let mut gz = Vec::new();
        write_to(&doc, &EncodeOptions::default().with_compress(true), &mut gz).unwrap();
        assert!(is_gzip(&gz));
        assert_eq!(parse(&gz).unwrap().width, 20.0);
    }
}
