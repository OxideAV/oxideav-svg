//! Round 11 — long-tail filter primitives, part 4: `<feImage>` and
//! `<feTile>` close the W3C Filter Effects §11 short-name set. The
//! typed-graph allowlist now covers every short-name primitive (17
//! total).
//!
//! Mirrors the round-8 / 9 / 10 layout — typed-graph parsing assertions
//! + a verbatim-XML round-trip test (the rasterizer is still where the
//!   pixels happen, but the round-trip path has carried these elements
//!   verbatim since round 2).

use oxideav_svg::filter::{
    CrossOrigin, FilterInput, FilterPrimitive, MeetOrSlice, PreserveAspectRatio,
    PreserveAspectRatioAlign,
};
use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

fn graph_for_filter(src: &[u8], filter_id: &str) -> oxideav_svg::filter::FilterGraph {
    let (_frame, extras) = parse_svg_with_extras(src).expect("parse_svg_with_extras");
    for el in &extras.filters {
        if el
            .attrs
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("id") && v == filter_id)
        {
            return oxideav_svg::filter::parse_filter_graph(el);
        }
    }
    panic!("no <filter id=\"{filter_id}\"> in extras");
}

// ---- feImage ----

#[test]
fn fe_image_records_href_and_aspect_ratio() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feImage href="texture.png" preserveAspectRatio="xMaxYMax slice" crossorigin="anonymous"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 1);
    let FilterPrimitive::Image {
        href,
        preserve_aspect_ratio,
        crossorigin,
    } = &g.primitives[0].primitive
    else {
        panic!("not Image");
    };
    assert_eq!(href, "texture.png");
    assert_eq!(
        *preserve_aspect_ratio,
        PreserveAspectRatio {
            align: PreserveAspectRatioAlign::XMaxYMax,
            meet_or_slice: MeetOrSlice::Slice,
        }
    );
    assert_eq!(*crossorigin, Some(CrossOrigin::Anonymous));
}

#[test]
fn fe_image_default_preserve_aspect_ratio_xmidymid_meet() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feImage href="x.png"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::Image {
        preserve_aspect_ratio,
        crossorigin,
        ..
    } = &g.primitives[0].primitive
    else {
        panic!("not Image");
    };
    assert_eq!(*preserve_aspect_ratio, PreserveAspectRatio::default());
    assert_eq!(*crossorigin, None);
}

#[test]
fn fe_image_falls_back_to_xlink_href() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10">
        <defs><filter id="f">
          <feImage xlink:href="legacy.png"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::Image { href, .. } = &g.primitives[0].primitive else {
        panic!("not Image");
    };
    assert_eq!(href, "legacy.png");
}

#[test]
fn fe_image_preserves_data_uri_intact() {
    // Verbatim-XML round-trip should not corrupt long base64 bodies.
    let data = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABAQMAAAAl21bKAAAAA1BMVEUAAACnej3aAAAAAXRSTlMAQObYZgAAAApJREFUCNdjAAEAAAQAAQXt5fcAAAAASUVORK5CYII=";
    let src = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <defs><filter id="f"><feImage href="{data}"/></filter></defs>
            <rect width="10" height="10" filter="url(#f)"/>
          </svg>"##
    );
    let g = graph_for_filter(src.as_bytes(), "f");
    let FilterPrimitive::Image { href, .. } = &g.primitives[0].primitive else {
        panic!("not Image");
    };
    assert_eq!(href, data);
}

// ---- feTile ----

#[test]
fn fe_tile_with_explicit_input() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feFlood flood-color="#ff0000" result="rd"/>
          <feTile in="rd"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 2);
    let FilterPrimitive::Tile { input } = &g.primitives[1].primitive else {
        panic!("not Tile");
    };
    assert_eq!(*input, FilterInput::Reference("rd".into()));
}

#[test]
fn fe_tile_implicit_input_is_source_graphic_when_first() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feTile/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::Tile { input } = &g.primitives[0].primitive else {
        panic!("not Tile");
    };
    assert_eq!(*input, FilterInput::SourceGraphic);
}

#[test]
fn fe_tile_implicit_input_threads_previous_result() {
    // Per Filter Effects §6.2 — `in` defaults to the previous
    // primitive's `result` when the chain is mid-stream.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feGaussianBlur stdDeviation="2" result="b"/>
          <feTile/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::Tile { input } = &g.primitives[1].primitive else {
        panic!("not Tile");
    };
    assert_eq!(*input, FilterInput::Reference("b".into()));
}

// ---- mixed pipelines + round-trip ----

#[test]
fn mixed_pipeline_with_image_and_tile() {
    // Realistic recipe: external texture as background, tiled to fill,
    // then composited under the source.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
        <defs><filter id="f">
          <feImage href="brick.png" result="bg"/>
          <feTile in="bg" result="bgt"/>
          <feComposite in="SourceGraphic" in2="bgt" operator="over"/>
        </filter></defs>
        <rect width="50" height="50" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 3);
    assert!(matches!(
        g.primitives[0].primitive,
        FilterPrimitive::Image { .. }
    ));
    assert!(matches!(
        g.primitives[1].primitive,
        FilterPrimitive::Tile { .. }
    ));
    assert!(matches!(
        g.primitives[2].primitive,
        FilterPrimitive::Composite { .. }
    ));
}

#[test]
fn round_trip_preserves_fe_image_and_fe_tile_verbatim() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feImage href="t.png" preserveAspectRatio="xMidYMid slice"/>
          <feTile in="SourceGraphic"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("feImage"), "feImage missing in {s}");
    assert!(s.contains("feTile"), "feTile missing in {s}");
    // The href must survive the round-trip intact.
    assert!(s.contains("t.png"));
}
