//! Round 15 — `<image>` element capture.
//!
//! Per SVG 2 §6, the `<image>` element references a raster image
//! (PNG / JPEG / WebP / …) painted into vector space at
//! `(x, y, width, height)`. The `href` (or legacy `xlink:href`)
//! attribute carries either an external URL or an inline
//! `data:image/<mime>;base64,...` URI per RFC 2397.
//!
//! These tests cover both shapes and the round-trip through
//! `parse_svg_with_extras` + `write_svg_with_extras`.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

use oxideav_svg::image::{ImageHref, SvgImage};
use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

/// 67-byte canonical 1x1 transparent PNG payload.
const PNG_1X1_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkAAIAAAoAAv/lxKUAAAAASUVORK5CYII=";

#[test]
fn captures_inline_data_uri_image() {
    let src = format!(
        r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <image x="10" y="20" width="50" height="50"
         href="data:image/png;base64,{}"/>
</svg>"##,
        PNG_1X1_BASE64
    );
    let (_, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
    assert_eq!(extras.images.len(), 1, "expected one captured <image>");
    let img = &extras.images[0];
    assert_eq!(img.x, 10.0);
    assert_eq!(img.y, 20.0);
    assert_eq!(img.width, Some(50.0));
    assert_eq!(img.height, Some(50.0));
    match &img.href {
        ImageHref::DataUri { mime, bytes } => {
            assert_eq!(mime, "image/png");
            // PNG signature.
            assert!(bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47]));
        }
        _ => panic!("expected inline data URI"),
    }
}

#[test]
fn captures_external_href_image() {
    let src = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <image x="0" y="0" width="100" height="100" href="logo.png"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
    assert_eq!(extras.images.len(), 1);
    let img = &extras.images[0];
    assert!(matches!(&img.href, ImageHref::External(s) if s == "logo.png"));
    assert_eq!(img.width, Some(100.0));
}

#[test]
fn captures_xlink_href_legacy_form() {
    let src = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"
     width="64" height="64">
  <image xlink:href="bg.jpg" width="64" height="64"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
    assert_eq!(extras.images.len(), 1);
    assert!(matches!(&extras.images[0].href, ImageHref::External(s) if s == "bg.jpg"));
}

#[test]
fn round_trip_inline_data_uri() {
    let src = format!(
        r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <image x="0" y="0" width="50" height="50"
         href="data:image/png;base64,{}"/>
</svg>"##,
        PNG_1X1_BASE64
    );
    let (frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
    let bytes = write_svg_with_extras(&frame, &extras);
    let out = std::str::from_utf8(&bytes).unwrap();
    // The encoder re-emits the data URI with the same MIME + payload.
    assert!(
        out.contains("data:image/png;base64,"),
        "encoder should re-emit data URI: {}",
        out
    );
    // Round-trip preserves byte-identity for the decoded payload.
    let (_, extras2) = parse_svg_with_extras(out.as_bytes()).unwrap();
    assert_eq!(extras2.images.len(), 1);
    match (&extras.images[0].href, &extras2.images[0].href) {
        (
            ImageHref::DataUri {
                mime: m1,
                bytes: b1,
            },
            ImageHref::DataUri {
                mime: m2,
                bytes: b2,
            },
        ) => {
            assert_eq!(m1, m2);
            assert_eq!(b1, b2);
        }
        _ => panic!("expected DataUri on both sides"),
    }
}

#[test]
fn round_trip_external_href() {
    let src = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <image x="5" y="5" width="40" height="40" href="https://example.com/icon.svg"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
    let bytes = write_svg_with_extras(&frame, &extras);
    let out = std::str::from_utf8(&bytes).unwrap();
    assert!(
        out.contains("https://example.com/icon.svg"),
        "encoder should re-emit external URL: {}",
        out
    );
    let (_, extras2) = parse_svg_with_extras(out.as_bytes()).unwrap();
    assert_eq!(extras2.images.len(), 1);
    assert!(matches!(
        &extras2.images[0].href,
        ImageHref::External(s) if s == "https://example.com/icon.svg"
    ));
    assert_eq!(extras2.images[0].x, 5.0);
}

#[test]
fn data_uri_with_whitespace_in_payload_decodes() {
    // Editor exports often wrap base64 across multiple lines.
    let original = b"\x89PNG\r\n\x1a\nfaked-png-bytes-for-testing";
    let payload = B64.encode(original);
    // Insert a newline mid-payload — the parser must tolerate it
    // (RFC 4648 §3.3 allows whitespace).
    let mid = payload.len() / 2;
    let chunked = format!("{}\n  {}", &payload[..mid], &payload[mid..]);
    let src = format!(
        r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <image href="data:image/png;base64,{}"/>
</svg>"##,
        chunked
    );
    let (_, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
    assert_eq!(extras.images.len(), 1);
    match &extras.images[0].href {
        ImageHref::DataUri { mime, bytes } => {
            assert_eq!(mime, "image/png");
            assert_eq!(bytes, original);
        }
        _ => panic!("expected data URI"),
    }
}

#[test]
fn malformed_data_uri_drops_the_image() {
    // No comma → not a valid RFC 2397 payload.
    let src = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <image href="data:image/png;base64-no-comma"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
    // Drops the malformed entry; the rest of the document still parses.
    assert_eq!(extras.images.len(), 0);
}

#[test]
fn missing_href_is_dropped() {
    // No href at all — nothing to capture.
    let src = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <image x="0" y="0" width="10" height="10"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
    assert_eq!(extras.images.len(), 0);
}

#[test]
fn captures_id_attribute_for_round_trip() {
    let src = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <image id="hero" x="0" y="0" width="100" height="100" href="hero.png"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
    assert_eq!(extras.images.len(), 1);
    assert_eq!(extras.images[0].id.as_deref(), Some("hero"));
    let bytes = write_svg_with_extras(&frame, &extras);
    let out = std::str::from_utf8(&bytes).unwrap();
    assert!(
        out.contains(r#"id="hero""#),
        "expected id to round-trip: {}",
        out
    );
}

#[test]
fn from_element_directly_accepts_typed_view() {
    use oxideav_svg::parser::Element;
    let el = Element {
        name: "image".into(),
        attrs: vec![
            ("href".into(), "x.png".into()),
            ("x".into(), "10".into()),
            ("y".into(), "10".into()),
            ("width".into(), "32".into()),
            ("height".into(), "32".into()),
        ],
        children: Vec::new(),
    };
    let img = SvgImage::from_element(&el, None).unwrap();
    assert_eq!(img.x, 10.0);
    assert_eq!(img.width, Some(32.0));
    assert!(matches!(img.href, ImageHref::External(s) if s == "x.png"));
}
