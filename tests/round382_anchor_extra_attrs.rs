//! Round 382 — verbatim round-trip of unmodelled `<a>` attributes.
//!
//! The round-115 `<a>`-wrapper encoder path re-emitted only the typed
//! HTML-link attributes (`href` / `target` / `download` / `ping` /
//! `rel` / `hreflang` / `type` / `referrerpolicy`) and dropped the SVG
//! 2 §16.5 core / styling / conditional-processing attributes. This
//! round sweeps the remaining source attributes into
//! `LinkBinding::extra_attrs` (document order) and re-emits them so the
//! `<a>` round-trip is lossless.

use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

fn roundtrip(src: &[u8]) -> String {
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    String::from_utf8(write_svg_with_extras(&frame, &extras)).expect("utf8")
}

/// Core + styling attributes on `<a>` survive the round-trip.
#[test]
fn core_and_styling_attrs_preserved() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <a href="https://example.org" id="link1" class="nav"
           style="cursor:pointer" transform="translate(3,4)">
            <rect width="10" height="10"/>
        </a>
    </svg>"##;
    let out = roundtrip(src);
    assert!(out.contains("<a "), "{out}");
    for want in [
        r#"id="link1""#,
        r#"class="nav""#,
        r#"style="cursor:pointer""#,
        r#"transform="translate(3,4)""#,
    ] {
        assert!(out.contains(want), "round-trip lost {want:?}:\n{out}");
    }
    // The modelled href still emits via its dedicated slot.
    assert!(out.contains(r#"href="https://example.org""#), "{out}");
}

/// The modelled HTML-link attributes are NOT duplicated into extra_attrs.
#[test]
fn modelled_link_attrs_not_duplicated() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <a href="https://example.org" target="_blank" rel="noopener"
           download="f" ping="p" hreflang="en" type="text/html"
           referrerpolicy="no-referrer">
            <rect width="10" height="10"/>
        </a>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    let link = &extras.links[0];
    for modelled in [
        "href",
        "target",
        "download",
        "ping",
        "rel",
        "hreflang",
        "type",
        "referrerpolicy",
    ] {
        assert!(
            !link.extra_attrs.iter().any(|(k, _)| k == modelled),
            "{modelled:?} must not leak into extra_attrs: {:?}",
            link.extra_attrs
        );
    }
    assert!(link.extra_attrs.is_empty());
}

/// Conditional-processing attributes on `<a>` round-trip.
#[test]
fn conditional_processing_attrs_preserved() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <a href="https://example.org" systemLanguage="en, fr"
           requiredExtensions="http://example.org/e">
            <rect width="10" height="10"/>
        </a>
    </svg>"##;
    let out = roundtrip(src);
    assert!(out.contains(r#"systemLanguage="en, fr""#), "{out}");
    assert!(
        out.contains(r#"requiredExtensions="http://example.org/e""#),
        "{out}"
    );
}

/// A bare `<a href>` with no unmodelled attributes records empty extras.
#[test]
fn plain_anchor_has_no_extras() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <a href="https://example.org"><rect width="10" height="10"/></a>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.links[0].extra_attrs.is_empty());
}

/// Extra attributes preserve document order across a full cycle.
#[test]
fn extra_attrs_preserve_order() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <a href="https://example.org" data-a="1" data-b="2" data-c="3">
            <rect width="10" height="10"/>
        </a>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    let keys: Vec<&str> = extras.links[0]
        .extra_attrs
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    assert_eq!(keys, vec!["data-a", "data-b", "data-c"]);
    let out = roundtrip(src);
    let (_f2, extras2) = parse_svg_with_extras(out.as_bytes()).unwrap();
    let keys2: Vec<&str> = extras2.links[0]
        .extra_attrs
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    assert_eq!(keys2, vec!["data-a", "data-b", "data-c"]);
}
