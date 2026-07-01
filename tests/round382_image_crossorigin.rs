//! Round 382 — SVG 2 §6 (embedded content) `crossorigin` on `<image>`.
//!
//! `<image>` lists `crossorigin` among its presentation attributes
//! (SVG 2 §6, Attributes table). It is a CORS-settings attribute whose
//! keyword grammar comes from the HTML CORS rules:
//!
//! * `anonymous` (or the bare `crossorigin` / empty-value form) — the
//!   fetch omits credentials for cross-origin requests.
//! * `use-credentials` — the fetch includes credentials.
//! * An invalid token maps to the anonymous state in HTML; for a
//!   round-trip capture we drop it (no binding) so the raw attribute
//!   isn't silently rewritten to a keyword the author never typed.
//!
//! The fetch itself is caller / rasteriser work — this crate captures
//! the keyword onto `SvgImage::crossorigin` and round-trips it.

use oxideav_svg::filter::CrossOrigin;
use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

/// An `<image>` without `crossorigin=` records no binding.
#[test]
fn baseline_no_crossorigin_no_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="0" y="0" width="50" height="50"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.images.len(), 1);
    assert!(
        extras.images[0].crossorigin.is_none(),
        "an <image> without crossorigin= must not record a binding"
    );
}

/// `crossorigin="anonymous"` records the anonymous variant.
#[test]
fn anonymous_records_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50" crossorigin="anonymous"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.images[0].crossorigin, Some(CrossOrigin::Anonymous));
}

/// The bare / empty-value `crossorigin=""` form maps to anonymous per
/// the HTML CORS-settings state machine.
#[test]
fn empty_value_maps_to_anonymous() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50" crossorigin=""/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.images[0].crossorigin, Some(CrossOrigin::Anonymous));
}

/// `crossorigin="use-credentials"` records the credentialed variant.
#[test]
fn use_credentials_records_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50"
               crossorigin="use-credentials"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.images[0].crossorigin,
        Some(CrossOrigin::UseCredentials)
    );
}

/// An unrecognised token records no binding (round-trip parser drops
/// the invalid value rather than rewriting it to a keyword).
#[test]
fn unknown_token_no_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50" crossorigin="bogus"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.images[0].crossorigin.is_none());
}

/// Round-trip: both keywords re-emit as their canonical string form,
/// and the empty-value form canonicalises to `anonymous`.
#[test]
fn crossorigin_round_trips() {
    for (input, expected) in [
        ("anonymous", "crossorigin=\"anonymous\""),
        ("", "crossorigin=\"anonymous\""),
        ("use-credentials", "crossorigin=\"use-credentials\""),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                    viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50" crossorigin="{input}"/>
    </svg>"#
        );
        let (frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        let out = write_svg_with_extras(&frame, &extras);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains(expected),
            "crossorigin={input:?} should re-emit {expected:?}; got:\n{text}"
        );

        // Re-parse the emitted output — the binding must survive a full
        // decode/encode/decode cycle.
        let (_f2, extras2) = parse_svg_with_extras(text.as_bytes()).unwrap();
        assert!(extras2.images[0].crossorigin.is_some());
    }
}

/// An `<image>` with no `crossorigin=` re-emits with no such attribute
/// (no initial-value bloat).
#[test]
fn absent_crossorigin_not_emitted() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50" height="50"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let text = String::from_utf8(out).unwrap();
    assert!(!text.contains("crossorigin"));
}
