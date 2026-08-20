//! Round 21 — SVG 2 §9.6.1 `pathLength` attribute integration tests.
//!
//! The attribute calibrates user-agent distance-along-a-path
//! calculations with the author's. Round 21 wires it into the
//! `stroke-dasharray` / `stroke-dashoffset` rescaling and the
//! [`oxideav_svg::preserved::PreservedExtras`] round-trip channel.

use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Helper: pull the first `Node::Path`'s stroke from the scene graph.
fn first_path_stroke(frame: &oxideav_core::VectorFrame) -> Option<oxideav_core::Stroke> {
    fn walk(node: &oxideav_core::Node) -> Option<oxideav_core::Stroke> {
        match node {
            oxideav_core::Node::Path(p) => p.stroke.clone(),
            oxideav_core::Node::Group(g) => g.children.iter().find_map(walk),
            oxideav_core::Node::SoftMask { content, .. } => walk(content),
            _ => None,
        }
    }
    frame.root.children.iter().find_map(walk)
}

#[test]
fn dasharray_scales_by_geometric_over_pathlength_ratio() {
    // A 100-user-unit horizontal line. Author claims `pathLength=200`,
    // so the §9.6.1 ratio is 100 / 200 = 0.5 — every dash entry and
    // the dash offset halve.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="20" viewBox="0 0 200 20">
  <path d="M 0 10 L 100 10"
        stroke="red"
        stroke-width="2"
        stroke-dasharray="20 10"
        stroke-dashoffset="4"
        pathLength="200"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let stroke = first_path_stroke(&frame).expect("must have a stroke");
    let dash = stroke.dash.expect("dash must survive scaling");
    assert!(
        (dash.array[0] - 10.0).abs() < 1e-3,
        "dash[0] = {}",
        dash.array[0]
    );
    assert!(
        (dash.array[1] - 5.0).abs() < 1e-3,
        "dash[1] = {}",
        dash.array[1]
    );
    assert!((dash.offset - 2.0).abs() < 1e-3, "offset = {}", dash.offset);
}

#[test]
fn pathlength_zero_collapses_dash_to_solid() {
    // §9.6.1: "A value of zero is valid and must be treated as a
    // scaling factor of infinity. […] any non-percentage value
    // greater than zero must become +Infinity." → no dash boundaries
    // ever turn off → solid stroke.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20" viewBox="0 0 100 20">
  <path d="M 0 10 L 100 10"
        stroke="red"
        stroke-width="2"
        stroke-dasharray="20 10"
        pathLength="0"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let stroke = first_path_stroke(&frame).expect("must have a stroke");
    assert!(
        stroke.dash.is_none(),
        "pathLength=0 with non-zero dashes ⇒ no dash pattern"
    );
}

#[test]
fn negative_pathlength_is_ignored() {
    // §9.6.1: "A negative value is an error." We treat the attribute
    // as if absent — the dasharray stays at its user-unit values.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20" viewBox="0 0 100 20">
  <path d="M 0 10 L 100 10"
        stroke="red"
        stroke-width="2"
        stroke-dasharray="20 10"
        pathLength="-50"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let stroke = first_path_stroke(&frame).expect("must have a stroke");
    let dash = stroke.dash.expect("dash should be unchanged");
    assert!((dash.array[0] - 20.0).abs() < 1e-3);
    assert!((dash.array[1] - 10.0).abs() < 1e-3);
}

#[test]
fn pathlength_on_rect_with_perimeter() {
    // A 10×5 rectangle has perimeter 30. With pathLength=60 the ratio
    // is 30 / 60 = 0.5 — every dash entry halves.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10" viewBox="0 0 20 10">
  <rect x="0" y="0" width="10" height="5"
        fill="none" stroke="black"
        stroke-dasharray="6 3"
        pathLength="60"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let stroke = first_path_stroke(&frame).expect("must have a stroke");
    let dash = stroke.dash.expect("dash must survive scaling");
    assert!(
        (dash.array[0] - 3.0).abs() < 1e-3,
        "expected halved 3.0, got {}",
        dash.array[0]
    );
    assert!((dash.array[1] - 1.5).abs() < 1e-3);
}

#[test]
fn pathlength_on_circle_with_circumference() {
    // Unit circle has circumference 2π. With pathLength=2π the ratio
    // is 1.0 — dasharray is preserved verbatim. (Author has aligned
    // their length unit with the geometric one.)
    let r = 10.0;
    let circum = 2.0 * std::f32::consts::PI * r;
    let src = format!(
        r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" viewBox="0 0 40 40">
  <circle cx="20" cy="20" r="{r}"
          fill="none" stroke="black"
          stroke-dasharray="5 5"
          pathLength="{circum}"/>
</svg>"##
    );
    let frame = parse_svg(src.as_bytes()).unwrap();
    let stroke = first_path_stroke(&frame).expect("must have a stroke");
    let dash = stroke.dash.expect("dash must survive scaling");
    // Allow 1% tolerance for the chord-sum approximation of the circle.
    assert!(
        (dash.array[0] - 5.0).abs() < 0.1,
        "dash[0] = {}",
        dash.array[0]
    );
    assert!(
        (dash.array[1] - 5.0).abs() < 0.1,
        "dash[1] = {}",
        dash.array[1]
    );
}

#[test]
fn pathlength_round_trip_via_extras() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="20" viewBox="0 0 200 20">
  <path d="M 0 10 L 100 10"
        stroke="red"
        stroke-width="2"
        stroke-dasharray="20 10"
        pathLength="200"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.path_lengths.len(),
        1,
        "one pathLength binding recorded"
    );
    assert!(
        (extras.path_lengths[0].path_length - 200.0).abs() < 1e-3,
        "binding value mismatch"
    );

    // Encoder must re-emit the attribute.
    let bytes = write_svg_with_extras(&frame, &extras);
    let out = std::str::from_utf8(&bytes).unwrap();
    assert!(
        out.contains("pathLength=\"200\""),
        "encoder must re-emit pathLength: {out}"
    );

    // Round 449 — re-parsing the encoder output must recover the SAME
    // scaled dashes as the first parse: the encoder divides the §9.6.1
    // decode-time rescale back out before writing (the emitted
    // `stroke-dasharray` is the author-unit value, so the re-parse
    // re-applies the identical `geometric / pathLength` ratio and
    // lands on the same rendered pattern). Before this round the
    // encoder emitted the *scaled* dash next to `pathLength=`, so
    // every parse → write cycle compounded the ratio (20 → 10 → 5 →
    // …) and the round-trip never reached a fixed point.
    assert!(
        out.contains("stroke-dasharray=\"20,10\""),
        "encoder emits the author-unit dash next to pathLength: {out}"
    );
    let frame2 = parse_svg(&bytes).unwrap();
    let stroke2 = first_path_stroke(&frame2).expect("must have a stroke");
    let dash2 = stroke2.dash.expect("dash survives");
    // First parse scaled (20,10) -> (10,5); the re-parse of the
    // emitted (20,10) + pathLength=200 lands on (10,5) again.
    assert!(
        (dash2.array[0] - 10.0).abs() < 0.1,
        "second pass dash[0] = {} (must match the first parse)",
        dash2.array[0]
    );
}

#[test]
fn pathlength_absent_is_noop() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20" viewBox="0 0 100 20">
  <path d="M 0 10 L 100 10"
        stroke="red"
        stroke-width="2"
        stroke-dasharray="20 10"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.path_lengths.is_empty());
    let frame = parse_svg(src).unwrap();
    let stroke = first_path_stroke(&frame).expect("must have a stroke");
    let dash = stroke.dash.expect("dash");
    assert!((dash.array[0] - 20.0).abs() < 1e-3);
    assert!((dash.array[1] - 10.0).abs() < 1e-3);
}

#[test]
fn pathlength_without_stroke_or_dash_is_noop() {
    // A pathLength on a fill-only shape doesn't scale anything but
    // shouldn't error. The side-channel still records the value for
    // round-trip emission.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20" viewBox="0 0 100 20">
  <path d="M 0 10 L 100 10" fill="blue" pathLength="42"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.path_lengths.len(), 1);
    assert!((extras.path_lengths[0].path_length - 42.0).abs() < 1e-3);
}

#[test]
fn pathlength_on_polyline_with_dasharray() {
    // A polyline `0,0 → 30,0 → 30,40` has length 30 + 40 = 70.
    // pathLength=140 ⇒ ratio 0.5 ⇒ dash halves.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="60" height="60" viewBox="0 0 60 60">
  <polyline points="0,0 30,0 30,40"
            fill="none" stroke="green"
            stroke-dasharray="10 5"
            pathLength="140"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let stroke = first_path_stroke(&frame).expect("must have a stroke");
    let dash = stroke.dash.expect("dash");
    assert!(
        (dash.array[0] - 5.0).abs() < 1e-3,
        "polyline dash[0] = {}",
        dash.array[0]
    );
    assert!((dash.array[1] - 2.5).abs() < 1e-3);
}

#[test]
fn pathlength_on_line_with_dashoffset() {
    // A line from (0,0)→(100,0) has length 100. pathLength=200 ⇒
    // offset halves.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="10" viewBox="0 0 100 10">
  <line x1="0" y1="5" x2="100" y2="5"
        stroke="black"
        stroke-dasharray="50 50"
        stroke-dashoffset="20"
        pathLength="200"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let stroke = first_path_stroke(&frame).expect("must have a stroke");
    let dash = stroke.dash.expect("dash");
    assert!(
        (dash.offset - 10.0).abs() < 1e-3,
        "halved offset = {}",
        dash.offset
    );
    assert!((dash.array[0] - 25.0).abs() < 1e-3);
}
