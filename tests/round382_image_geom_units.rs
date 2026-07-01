//! Round 382 — `<image>` geometry `<length>` unit / percentage fidelity.
//!
//! `<image>`'s `x` / `y` / `width` / `height` are `<length>` values
//! (SVG 2 §6, Geometry properties) that may carry a CSS unit (`px`,
//! `em`, …) or be a percentage of the current viewport. The numeric
//! projection on `SvgImage::{x,y,width,height}` drops that unit; the
//! raw slots (`*_raw`) preserve the exact source token so a
//! `width="50%"` round-trips as `50%` rather than a semantics-changing
//! bare `50`.

use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

/// A percentage width/height round-trips with its `%` intact.
#[test]
fn percentage_geometry_round_trips() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="50%" height="25%"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let img = &extras.images[0];
    assert_eq!(img.width_raw.as_deref(), Some("50%"));
    assert_eq!(img.height_raw.as_deref(), Some("25%"));
    // Numeric projection still exposes the magnitude for renderers.
    assert_eq!(img.width, Some(50.0));
    assert_eq!(img.height, Some(25.0));
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(out.contains(r#"width="50%""#), "{out}");
    assert!(out.contains(r#"height="25%""#), "{out}");
}

/// A unit-bearing x/y round-trips with the unit preserved.
#[test]
fn unit_bearing_position_round_trips() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="1em" y="2px" width="10" height="10"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let img = &extras.images[0];
    assert_eq!(img.x_raw.as_deref(), Some("1em"));
    assert_eq!(img.y_raw.as_deref(), Some("2px"));
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(out.contains(r#"x="1em""#), "{out}");
    assert!(out.contains(r#"y="2px""#), "{out}");
}

/// A plain numeric geometry records NO raw slot (no round-trip bloat)
/// and re-emits via the canonical numeric path.
#[test]
fn plain_numeric_geometry_no_raw_slot() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="1" y="2" width="50" height="50"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let img = &extras.images[0];
    assert!(img.x_raw.is_none());
    assert!(img.y_raw.is_none());
    assert!(img.width_raw.is_none());
    assert!(img.height_raw.is_none());
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(out.contains(r#"x="1""#));
    assert!(out.contains(r#"width="50""#));
}

/// A `0%`-valued position (numeric 0 but a non-default source token) is
/// still emitted rather than being suppressed by the `x != 0` guard.
#[test]
fn zero_percent_position_not_suppressed() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="0%" width="10" height="10"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.images[0].x_raw.as_deref(), Some("0%"));
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(out.contains(r#"x="0%""#), "{out}");
}

/// Full two-cycle round-trip keeps the percentage token stable.
#[test]
fn percentage_survives_two_cycles() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" width="33.5%" height="10%"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out1 = write_svg_with_extras(&frame, &extras);
    let (frame2, extras2) = parse_svg_with_extras(&out1).unwrap();
    assert_eq!(extras2.images[0].width_raw.as_deref(), Some("33.5%"));
    let out2 = String::from_utf8(write_svg_with_extras(&frame2, &extras2)).unwrap();
    assert!(out2.contains(r#"width="33.5%""#), "{out2}");
}
