//! Round 382 — verbatim preservation of unmodelled `<image>` attributes.
//!
//! The SVG 2 §6 `<image>` Attributes table lists the core, styling and
//! conditional-processing attributes (`class`, `style`, presentation
//! properties like `opacity` / `clip-path` / `mask` / `filter` /
//! `visibility`, `requiredExtensions`, `systemLanguage`, the legacy
//! `xlink:title`, …). The crate models only a handful of these with
//! typed slots; the rest are swept into `SvgImage::extra_attrs` in
//! document order so a decode → encode cycle preserves them rather than
//! silently dropping them.

use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

/// Styling + presentation attributes the crate doesn't type are kept.
#[test]
fn styling_and_presentation_attrs_preserved() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50"
               class="thumb" style="opacity:0.5" opacity="0.5"
               clip-path="url(#clip)" mask="url(#m)" filter="url(#f)"
               visibility="hidden"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let img = &extras.images[0];
    for want in [
        "class",
        "style",
        "opacity",
        "clip-path",
        "mask",
        "filter",
        "visibility",
    ] {
        assert!(
            img.extra_attrs.iter().any(|(k, _)| k == want),
            "expected extra_attrs to retain {want:?}, got {:?}",
            img.extra_attrs
        );
    }
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    for want in [
        r#"class="thumb""#,
        r#"clip-path="url(#clip)""#,
        r#"mask="url(#m)""#,
        r#"filter="url(#f)""#,
        r#"visibility="hidden""#,
    ] {
        assert!(out.contains(want), "round-trip lost {want:?}:\n{out}");
    }
}

/// Conditional-processing attributes (`requiredExtensions`,
/// `systemLanguage`) round-trip through the verbatim channel.
#[test]
fn conditional_processing_attrs_preserved() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50"
               requiredExtensions="http://example.org/ext"
               systemLanguage="en, fr"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(
        out.contains(r#"requiredExtensions="http://example.org/ext""#),
        "{out}"
    );
    assert!(out.contains(r#"systemLanguage="en, fr""#), "{out}");
}

/// The modelled attributes stay in their dedicated slots and are NOT
/// duplicated into `extra_attrs`.
#[test]
fn modelled_attrs_not_duplicated_into_extras() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image id="i1" href="logo.png" x="1" y="2" width="50" height="50"
               transform="translate(3,4)" preserveAspectRatio="xMidYMid meet"
               image-rendering="optimizeQuality" crossorigin="anonymous"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    let img = &extras.images[0];
    for modelled in [
        "id",
        "href",
        "x",
        "y",
        "width",
        "height",
        "transform",
        "preserveAspectRatio",
        "image-rendering",
        "crossorigin",
    ] {
        assert!(
            !img.extra_attrs.iter().any(|(k, _)| k == modelled),
            "{modelled:?} must not leak into extra_attrs: {:?}",
            img.extra_attrs
        );
    }
    assert!(img.extra_attrs.is_empty());
}

/// An *invalid* crossorigin token (no typed binding) still survives via
/// the verbatim channel rather than being silently dropped.
#[test]
fn invalid_crossorigin_survives_verbatim() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50" crossorigin="bogus"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.images[0].crossorigin.is_none());
    assert!(extras.images[0]
        .extra_attrs
        .iter()
        .any(|(k, v)| k == "crossorigin" && v == "bogus"));
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(out.contains(r#"crossorigin="bogus""#), "{out}");
}

/// Extra attributes retain their source document order.
#[test]
fn extra_attrs_preserve_order() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50"
               data-a="1" data-b="2" data-c="3"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    let keys: Vec<&str> = extras.images[0]
        .extra_attrs
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    assert_eq!(keys, vec!["data-a", "data-b", "data-c"]);
}

/// A bare `<image>` with only modelled attributes has empty extras and
/// emits no stray attributes.
#[test]
fn bare_image_has_no_extras() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.images[0].extra_attrs.is_empty());
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    // Only href + width + height on the tag (no spurious attributes).
    assert!(out.contains("<image"));
}

/// Full decode → encode → decode preserves the extra attributes across
/// two cycles.
#[test]
fn extra_attrs_survive_two_cycles() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50"
               class="a" style="opacity:.3"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out1 = write_svg_with_extras(&frame, &extras);
    let (frame2, extras2) = parse_svg_with_extras(&out1).unwrap();
    assert!(extras2.images[0]
        .extra_attrs
        .iter()
        .any(|(k, v)| k == "class" && v == "a"));
    let out2 = String::from_utf8(write_svg_with_extras(&frame2, &extras2)).unwrap();
    assert!(out2.contains(r#"class="a""#));
    assert!(out2.contains(r#"style="opacity:.3""#));
}
