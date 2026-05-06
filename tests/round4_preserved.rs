//! Round 4 — encoder preservation of `<style>` / `<filter>` /
//! `<animate>` / `<foreignObject>` via the [`PreservedExtras`]
//! side-channel.

use oxideav_svg::{parse_svg_with_extras, write_svg, write_svg_with_extras};

#[test]
fn parse_with_extras_captures_style_block() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <defs><style>.a { fill: red }</style></defs>
  <rect class="a" width="10" height="10"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.styles.len(), 1);
    assert!(extras.styles[0].contains(".a"));
}

#[test]
fn parse_with_extras_captures_filter_def() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <defs>
    <filter id="b1"><feGaussianBlur stdDeviation="2"/></filter>
  </defs>
  <rect width="10" height="10" filter="url(#b1)"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.filters.len(), 1);
    assert_eq!(extras.filters[0].name, "filter");
}

#[test]
fn parse_with_extras_captures_animation_with_parent_id() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <rect id="r1" width="10" height="10" fill="red">
    <animate attributeName="fill" from="red" to="blue" dur="2s"/>
  </rect>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.animations.len(), 1);
    assert_eq!(extras.animations[0].parent_id.as_deref(), Some("r1"));
}

#[test]
fn parse_with_extras_captures_foreign_object() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
  <foreignObject width="100" height="50">
    <body xmlns="http://www.w3.org/1999/xhtml">hi</body>
  </foreignObject>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.foreign_objects.len(), 1);
}

#[test]
fn write_with_extras_round_trips_style() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <defs><style>.a { fill: red }</style></defs>
  <rect class="a" width="10" height="10"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("<style"), "output should contain <style>: {s}");
    assert!(s.contains(".a"), "output should contain .a: {s}");
}

#[test]
fn write_with_extras_round_trips_filter() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <defs><filter id="f1"><feGaussianBlur stdDeviation="2"/></filter></defs>
  <rect width="10" height="10"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("<filter"), "output should contain <filter>: {s}");
    assert!(
        s.contains("feGaussianBlur"),
        "primitives should survive: {s}"
    );
}

#[test]
fn write_with_extras_round_trips_animation() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <rect id="r1" width="10" height="10" fill="red">
    <animate attributeName="fill" from="red" to="blue" dur="2s"/>
  </rect>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let s = std::str::from_utf8(&out).unwrap();
    assert!(
        s.contains("<animate"),
        "output should contain <animate>: {s}"
    );
}

#[test]
fn write_without_extras_drops_definitions_as_before() {
    // Backwards-compat: bare write_svg() still loses extras.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <defs><style>.a { fill: red }</style></defs>
  <rect class="a" width="10" height="10"/>
</svg>"##;
    let (frame, _extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg(&frame);
    let s = std::str::from_utf8(&out).unwrap();
    // The CSS rule was applied to the rect's fill (so the path is
    // red), but the style block itself does NOT round-trip through
    // bare write_svg.
    assert!(
        !s.contains("<style"),
        "bare write_svg should not emit <style>: {s}"
    );
    // But the rect should still be red because the cascade resolved.
    assert!(
        s.contains("ff0000") || s.contains("#ff0000"),
        "expected red fill, got {s}"
    );
}

#[test]
fn round_trip_through_write_with_extras_then_reparse() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <defs>
    <style>.box { fill: #336699 }</style>
    <filter id="bb"><feGaussianBlur stdDeviation="3"/></filter>
  </defs>
  <rect class="box" id="r1" width="10" height="10" filter="url(#bb)">
    <animate attributeName="x" from="0" to="50" dur="1s"/>
  </rect>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let (frame2, extras2) = parse_svg_with_extras(&out).unwrap();
    // Frame width / height preserved.
    assert_eq!(frame.width, frame2.width);
    assert_eq!(frame.height, frame2.height);
    // Extras preserved.
    assert_eq!(extras.styles.len(), extras2.styles.len());
    assert_eq!(extras.filters.len(), extras2.filters.len());
    assert_eq!(extras.animations.len(), extras2.animations.len());
}

#[test]
fn empty_extras_emits_no_extra_blocks() {
    use oxideav_svg::PreservedExtras;
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <rect width="10" height="10" fill="red"/>
</svg>"##;
    let (frame, _) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &PreservedExtras::default());
    let s = std::str::from_utf8(&out).unwrap();
    assert!(!s.contains("<style"));
    assert!(!s.contains("<filter"));
    assert!(!s.contains("<animate"));
}
