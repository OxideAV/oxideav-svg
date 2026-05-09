//! Round 13 — animation re-attachment to the original parent emit
//! site. Round 4–12 dumped captured `<animate>` / `<set>` /
//! `<animateTransform>` fragments at the trailing edge of the SVG with
//! a parent-id comment hint. Round 13 inlines them inside their
//! declared parent element, re-emits the original `id="..."` on the
//! parent, and preserves the parent-child relationship across a
//! parse → encode round-trip.

use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

#[test]
fn animate_inlines_inside_parent_rect_with_id() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect id="r1" x="10" y="10" width="80" height="80" fill="#ff0000">
    <animate attributeName="x" from="0" to="50" dur="2s"/>
  </rect>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let bytes = write_svg_with_extras(&frame, &extras);
    let s = std::str::from_utf8(&bytes).unwrap();
    // The <animate> must appear between the opening <path id="r1" ...>
    // and the closing </path> — i.e. inlined as a child, not at the
    // trailing edge of the document.
    let path_open = s.find("id=\"r1\"").expect("id=\"r1\" missing");
    let animate = s.find("<animate").expect("<animate missing");
    let path_close = s.find("</path>").expect("</path> missing");
    assert!(
        path_open < animate && animate < path_close,
        "expected `<animate>` inside the id=\"r1\" element. Got:\n{s}"
    );
    // No trailing-edge comment hint should appear when the animation
    // was successfully inlined.
    assert!(
        !s.contains("animation parent: #r1"),
        "trailing-edge comment hint should NOT appear for a re-attached animation. Got:\n{s}"
    );
}

#[test]
fn animate_inlines_inside_group_with_id() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <g id="grp1" opacity="0.5">
    <rect x="0" y="0" width="50" height="50" fill="#0000ff"/>
    <animateTransform attributeName="transform" type="rotate" from="0" to="360" dur="4s"/>
  </g>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let bytes = write_svg_with_extras(&frame, &extras);
    let s = std::str::from_utf8(&bytes).unwrap();
    // The animation should appear inside the <g id="grp1"> ... </g>
    // block.
    let group_open = s.find("id=\"grp1\"").expect("id=\"grp1\" missing");
    let animate = s
        .find("<animateTransform")
        .expect("<animateTransform missing");
    let group_close = s[group_open..].find("</g>").expect("</g> missing");
    assert!(animate > group_open);
    assert!(animate - group_open < group_close);
}

#[test]
fn untracked_animation_falls_back_to_trailing_edge() {
    // Animation parent has NO id. Round-13 inline path requires an
    // id, so this falls back to the round-12 trailing-edge emission.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <rect x="0" y="0" width="50" height="50" fill="#0000ff">
    <animate attributeName="x" from="0" to="50" dur="2s"/>
  </rect>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let bytes = write_svg_with_extras(&frame, &extras);
    let s = std::str::from_utf8(&bytes).unwrap();
    // No id on the parent → animation can't be inlined — but it must
    // not be lost. Confirm the <animate> element survives somewhere
    // in the output.
    assert!(s.contains("<animate"));
}

#[test]
fn round_trip_preserves_id_attribute_on_path() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <rect id="myRect" x="0" y="0" width="50" height="50" fill="#abcdef"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let bytes = write_svg_with_extras(&frame, &extras);
    let s = std::str::from_utf8(&bytes).unwrap();
    // Even without an animation, the source id should be re-emitted
    // by round 13.
    assert!(
        s.contains("id=\"myRect\""),
        "expected id attribute in output. Got:\n{s}"
    );
}

#[test]
fn id_paths_populated_in_extras() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <rect id="a" x="0" y="0" width="10" height="10" fill="#ff0000"/>
  <g id="b">
    <rect id="c" x="20" y="20" width="10" height="10" fill="#00ff00"/>
  </g>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).unwrap();
    // Three id-bearing elements → three id_paths entries.
    let ids: Vec<&str> = extras.id_paths.iter().map(|e| e.id.as_str()).collect();
    assert!(ids.contains(&"a"));
    assert!(ids.contains(&"b"));
    assert!(ids.contains(&"c"));
}

#[test]
fn animation_inlining_survives_a_second_round_trip() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <rect id="r2" x="10" y="10" width="80" height="80" fill="#00ff00">
    <set attributeName="fill" to="#ff0000"/>
  </rect>
</svg>"##;
    let (frame1, extras1) = parse_svg_with_extras(src).unwrap();
    let bytes1 = write_svg_with_extras(&frame1, &extras1);
    let (frame2, extras2) = parse_svg_with_extras(&bytes1).unwrap();
    let bytes2 = write_svg_with_extras(&frame2, &extras2);
    let s = std::str::from_utf8(&bytes2).unwrap();
    // After two round-trips, the id and the inlined <set> survive.
    assert!(s.contains("id=\"r2\""));
    assert!(s.contains("<set"));
}
