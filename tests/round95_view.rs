//! Round 95 — SVG 2 §16.3 `<view>` element + fragment-identifier
//! routing integration tests.

use oxideav_svg::{
    parse_svg, parse_svg_with_extras, resolve_fragment, write_svg, write_svg_with_extras,
};

#[test]
fn view_element_does_not_render_as_a_node() {
    // `<view>` is a pure metadata element — it must not push a
    // scene-graph node. The root group should be empty.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200" viewBox="0 0 200 200">
  <view id="zoomIn" viewBox="0 0 50 50"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert!(
        frame.root.children.is_empty(),
        "<view> must not contribute a renderable node; got {} children",
        frame.root.children.len()
    );
}

#[test]
fn view_lookup_returns_view_box_override() {
    // The `<view>` overrides the root's viewBox. resolve_fragment with
    // the bare-name fragment must surface the view's value, not the
    // root's.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <view id="topLeftQuarter" viewBox="0 0 50 50"/>
  <view id="botRightQuarter" viewBox="50 50 50 50"/>
  <rect x="0" y="0" width="100" height="100" fill="blue"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let r = resolve_fragment(&frame, &extras, "topLeftQuarter");
    let vb = r.view_box.unwrap();
    assert_eq!(vb.min_x, 0.0);
    assert_eq!(vb.width, 50.0);

    let r2 = resolve_fragment(&frame, &extras, "botRightQuarter");
    let vb2 = r2.view_box.unwrap();
    assert_eq!(vb2.min_x, 50.0);
    assert_eq!(vb2.min_y, 50.0);
    assert_eq!(vb2.width, 50.0);
}

#[test]
fn view_lookup_captures_preserve_aspect_ratio_and_zoom_and_pan() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <view id="v" viewBox="10 10 80 80" preserveAspectRatio="xMaxYMin slice" zoomAndPan="disable"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let r = resolve_fragment(&frame, &extras, "v");
    let vb = r.view_box.unwrap();
    assert_eq!(vb.min_x, 10.0);
    assert!(matches!(
        r.preserve_aspect_ratio.align,
        oxideav_svg::filter::PreserveAspectRatioAlign::XMaxYMin
    ));
    assert!(matches!(
        r.preserve_aspect_ratio.meet_or_slice,
        oxideav_svg::filter::MeetOrSlice::Slice
    ));
    assert_eq!(r.zoom_and_pan, oxideav_svg::defs::ZoomAndPan::Disable);
}

#[test]
fn svg_view_inline_spec_overrides_root_view_box() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"></svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let r = resolve_fragment(&frame, &extras, "svgView(viewBox(0,200,1000,1000))");
    let vb = r.view_box.unwrap();
    assert_eq!(vb.min_x, 0.0);
    assert_eq!(vb.min_y, 200.0);
    assert_eq!(vb.width, 1000.0);
    assert_eq!(vb.height, 1000.0);
}

#[test]
fn view_round_trips_through_write_svg_with_extras() {
    // A `parse → write_svg_with_extras → parse` cycle must preserve
    // every captured `<view>` definition so a caller can still
    // resolve `#viewId` after a round-trip.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200" viewBox="0 0 200 200">
  <view id="halfView" viewBox="0 0 100 100" preserveAspectRatio="xMidYMid slice"/>
  <rect x="0" y="0" width="200" height="200" fill="red"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let (frame2, extras2) = parse_svg_with_extras(&out).unwrap();
    assert!(
        extras2.typed_views.contains_key("halfView"),
        "round-trip lost the <view id='halfView'> definition"
    );
    let r = resolve_fragment(&frame2, &extras2, "halfView");
    let vb = r.view_box.unwrap();
    assert_eq!(vb.width, 100.0);
    assert!(matches!(
        r.preserve_aspect_ratio.meet_or_slice,
        oxideav_svg::filter::MeetOrSlice::Slice
    ));
}

#[test]
fn view_attribute_unspecified_inherits_from_root() {
    // The `<view>` sets only the viewBox; preserveAspectRatio must
    // inherit the root `<svg>` keyword pair per §16.3.2.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200"
                       preserveAspectRatio="xMinYMid slice">
  <view id="z" viewBox="20 20 50 50"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let r = resolve_fragment(&frame, &extras, "z");
    // viewBox overridden by the <view>.
    assert_eq!(r.view_box.unwrap().min_x, 20.0);
    // PAR inherits from the root.
    assert!(matches!(
        r.preserve_aspect_ratio.align,
        oxideav_svg::filter::PreserveAspectRatioAlign::XMinYMid
    ));
    assert!(matches!(
        r.preserve_aspect_ratio.meet_or_slice,
        oxideav_svg::filter::MeetOrSlice::Slice
    ));
}

#[test]
fn bare_write_svg_drops_views_without_extras() {
    // The legacy `write_svg(&frame)` path (with no extras) must still
    // produce a parseable document — it just won't carry the
    // metadata-only `<view>` elements since they have no scene-graph
    // representation.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <view id="v" viewBox="0 0 50 50"/>
  <rect width="100" height="100" fill="green"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let bytes = write_svg(&frame);
    let _ = parse_svg(&bytes).expect("re-parse must succeed even without view round-trip");
}

#[test]
fn empty_fragment_returns_root_view_box() {
    // SVG 2 §16.3.2 — "If no SVG fragment identifier is provided ...
    // the initial view ... is established using the view specification
    // attributes ... on the outermost svg element."
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"
                       preserveAspectRatio="xMinYMin meet">
  <view id="v" viewBox="0 0 32 32"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let r = resolve_fragment(&frame, &extras, "");
    let vb = r.view_box.unwrap();
    assert_eq!(vb.width, 64.0);
    assert!(matches!(
        r.preserve_aspect_ratio.align,
        oxideav_svg::filter::PreserveAspectRatioAlign::XMinYMin
    ));
}

#[test]
fn view_with_no_id_is_silently_dropped() {
    // An `<view>` without an `id` can't be addressed by any fragment,
    // so the parser drops it from the typed table. The verbatim
    // capture still keeps it for round-trip.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <view viewBox="0 0 50 50"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.typed_views.is_empty(),
        "id-less <view> must not populate the typed_views map"
    );
    // The verbatim element should still be on the side-channel so a
    // round-trip preserves it byte-faithfully.
    assert_eq!(extras.views.len(), 1);
}

#[test]
fn nested_view_under_a_group_is_still_discovered() {
    // `<view>` is allowed as a child of any container per the SVG 2
    // content model. The pre-walk must descend into `<g>` to find it.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200">
  <g>
    <view id="nested" viewBox="0 0 25 25"/>
  </g>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let r = resolve_fragment(&frame, &extras, "nested");
    let vb = r.view_box.unwrap();
    assert_eq!(vb.width, 25.0);
}
