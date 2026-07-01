//! Round 382 — verbatim round-trip of unmodelled `<use>` attributes.
//!
//! The collapse-to-`<use>` encoder path (round 372) re-emitted only the
//! typed `href` / `x` / `y` / `width` / `height` / `transform` / `id`
//! attributes and dropped everything else — `class`, `style`,
//! presentation properties (`opacity`, `clip-path`, `mask`, `filter`,
//! `visibility`, `fill`, `stroke`), and conditional-processing
//! attributes. This round sweeps the remaining source attributes into
//! `UseBinding::extra_attrs` (document order) and re-emits them so the
//! `<use>` round-trip is lossless.

use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

fn roundtrip(src: &[u8]) -> String {
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    String::from_utf8(write_svg_with_extras(&frame, &extras)).expect("utf8")
}

/// Styling + presentation attributes on `<use>` survive the round-trip.
#[test]
fn styling_and_presentation_attrs_preserved() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <defs><rect id="r" width="10" height="10"/></defs>
        <use href="#r" x="5" y="5" class="dup" style="opacity:.5"
             fill="red" clip-path="url(#c)" mask="url(#m)" filter="url(#f)"
             visibility="hidden"/>
    </svg>"##;
    let out = roundtrip(src);
    assert!(out.contains("<use"), "{out}");
    for want in [
        r#"class="dup""#,
        r#"fill="red""#,
        r#"clip-path="url(#c)""#,
        r#"mask="url(#m)""#,
        r#"filter="url(#f)""#,
        r#"visibility="hidden""#,
    ] {
        assert!(out.contains(want), "round-trip lost {want:?}:\n{out}");
    }
    // The modelled attributes still emit via their dedicated slots.
    assert!(out.contains(r##"href="#r""##), "{out}");
    assert!(out.contains(r#"x="5""#), "{out}");
}

/// Conditional-processing attributes on `<use>` round-trip.
#[test]
fn conditional_processing_attrs_preserved() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <defs><rect id="r" width="10" height="10"/></defs>
        <use href="#r" requiredExtensions="http://example.org/e"
             systemLanguage="en, fr"/>
    </svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains(r#"requiredExtensions="http://example.org/e""#),
        "{out}"
    );
    assert!(out.contains(r#"systemLanguage="en, fr""#), "{out}");
}

/// The modelled attributes are NOT duplicated into extra_attrs.
#[test]
fn modelled_attrs_not_duplicated() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <defs><rect id="r" width="10" height="10"/></defs>
        <use id="u" href="#r" x="1" y="2" width="3" height="4"
             transform="translate(5,6)"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    let u = &extras.uses[0];
    for modelled in ["href", "x", "y", "width", "height", "transform", "id"] {
        assert!(
            !u.extra_attrs.iter().any(|(k, _)| k == modelled),
            "{modelled:?} must not leak into extra_attrs: {:?}",
            u.extra_attrs
        );
    }
    assert!(u.extra_attrs.is_empty());
}

/// A `<use>` with no unmodelled attributes records an empty extras vec
/// and emits no stray attributes.
#[test]
fn plain_use_has_no_extras() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <defs><rect id="r" width="10" height="10"/></defs>
        <use href="#r"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.uses[0].extra_attrs.is_empty());
}

/// Extra attributes retain their source document order across a cycle.
#[test]
fn extra_attrs_preserve_order() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <defs><rect id="r" width="10" height="10"/></defs>
        <use href="#r" data-a="1" data-b="2" data-c="3"/>
    </svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    let keys: Vec<&str> = extras.uses[0]
        .extra_attrs
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    assert_eq!(keys, vec!["data-a", "data-b", "data-c"]);
    let out = roundtrip(src);
    let (_f2, extras2) = parse_svg_with_extras(out.as_bytes()).unwrap();
    let keys2: Vec<&str> = extras2.uses[0]
        .extra_attrs
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    assert_eq!(keys2, vec!["data-a", "data-b", "data-c"]);
}
