//! Round 7 — typed parsing of `<filter>` primitive graphs.
//!
//! Round 2-4 already round-tripped `<filter>` definitions verbatim.
//! Round 7 adds a parallel typed [`FilterGraph`] view on each
//! [`FilterDef`] so a downstream rasterizer can consume the pipeline
//! without re-parsing the XML. These tests verify that the typed graph
//! is populated correctly, that the round-trip path is unaffected, and
//! that consumers can reach the parsed primitives via the public API.

use oxideav_svg::filter::{
    BlendMode, CompositeOperator, FilterInput, FilterPrimitive, MorphologyOperator,
};
use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

/// Re-parse the source through the public API and dig out the typed
/// `<filter id="f">` graph from the live parse context. The typed graph
/// hangs off `FilterDef` in `crate::defs`; consumers normally drive it
/// from a render pass, but the tests need direct access — we round-trip
/// through `parse_filter_graph` on the captured XML element instead.
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

#[test]
fn gaussian_blur_one_value_propagates_to_both_axes() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f"><feGaussianBlur stdDeviation="3"/></filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 1);
    let FilterPrimitive::GaussianBlur {
        std_deviation_x,
        std_deviation_y,
        ..
    } = &g.primitives[0].primitive
    else {
        panic!("not a blur");
    };
    assert_eq!(*std_deviation_x, 3.0);
    assert_eq!(*std_deviation_y, 3.0);
}

#[test]
fn offset_dx_dy_are_captured() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f"><feOffset dx="4" dy="-2"/></filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::Offset { dx, dy, .. } = &g.primitives[0].primitive else {
        panic!("not an offset");
    };
    assert_eq!(*dx, 4.0);
    assert_eq!(*dy, -2.0);
}

#[test]
fn flood_color_named_html_parses() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f"><feFlood flood-color="red" flood-opacity="0.25"/></filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::Flood {
        flood_color,
        flood_opacity,
    } = &g.primitives[0].primitive
    else {
        panic!("not a flood");
    };
    assert_eq!(flood_color.r, 0xff);
    assert_eq!(flood_color.g, 0);
    assert_eq!(flood_color.b, 0);
    assert!((*flood_opacity - 0.25).abs() < 1e-6);
}

#[test]
fn composite_arithmetic_records_all_four_k_constants() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feFlood result="bg" flood-color="#000000"/>
          <feComposite in="SourceGraphic" in2="bg" operator="arithmetic"
                       k1="0.5" k2="0.25" k3="0.125" k4="0.0625"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 2);
    let FilterPrimitive::Composite {
        operator,
        k1,
        k2,
        k3,
        k4,
        input2,
        ..
    } = &g.primitives[1].primitive
    else {
        panic!("not a composite");
    };
    assert_eq!(*operator, CompositeOperator::Arithmetic);
    assert!((*k1 - 0.5).abs() < 1e-6);
    assert!((*k2 - 0.25).abs() < 1e-6);
    assert!((*k3 - 0.125).abs() < 1e-6);
    assert!((*k4 - 0.0625).abs() < 1e-6);
    assert_eq!(*input2, FilterInput::Reference("bg".into()));
}

#[test]
fn blend_mode_keywords_round_trip_to_enum() {
    for (kw, expected) in [
        ("normal", BlendMode::Normal),
        ("multiply", BlendMode::Multiply),
        ("screen", BlendMode::Screen),
        ("darken", BlendMode::Darken),
        ("lighten", BlendMode::Lighten),
        ("color-dodge", BlendMode::ColorDodge),
        ("hard-light", BlendMode::HardLight),
        ("difference", BlendMode::Difference),
        ("luminosity", BlendMode::Luminosity),
    ] {
        let src = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
              <defs><filter id="f">
                <feBlend in="SourceGraphic" in2="SourceAlpha" mode="{kw}"/>
              </filter></defs>
              <rect width="10" height="10" filter="url(#f)"/>
            </svg>"##
        );
        let g = graph_for_filter(src.as_bytes(), "f");
        let FilterPrimitive::Blend { mode, .. } = &g.primitives[0].primitive else {
            panic!("not a blend for {kw}");
        };
        assert_eq!(*mode, expected, "mode={kw}");
    }
}

#[test]
fn morphology_dilate_and_erode() {
    for (op, expected) in [
        ("dilate", MorphologyOperator::Dilate),
        ("erode", MorphologyOperator::Erode),
    ] {
        let src = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
              <defs><filter id="f">
                <feMorphology operator="{op}" radius="2 3"/>
              </filter></defs>
              <rect width="10" height="10" filter="url(#f)"/>
            </svg>"##
        );
        let g = graph_for_filter(src.as_bytes(), "f");
        let FilterPrimitive::Morphology {
            operator,
            radius_x,
            radius_y,
            ..
        } = &g.primitives[0].primitive
        else {
            panic!("not a morphology for {op}");
        };
        assert_eq!(*operator, expected);
        assert_eq!(*radius_x, 2.0);
        assert_eq!(*radius_y, 3.0);
    }
}

#[test]
fn first_primitive_input_defaults_to_source_graphic() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f"><feGaussianBlur stdDeviation="2"/></filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::GaussianBlur { input, .. } = &g.primitives[0].primitive else {
        panic!("not a blur");
    };
    assert_eq!(*input, FilterInput::SourceGraphic);
}

#[test]
fn implicit_chain_uses_previous_result_when_in_is_omitted() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feGaussianBlur stdDeviation="3" result="blur"/>
          <feOffset dx="5" dy="5"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 2);
    let FilterPrimitive::Offset { input, .. } = &g.primitives[1].primitive else {
        panic!("expected offset");
    };
    assert_eq!(*input, FilterInput::Reference("blur".into()));
}

#[test]
fn unknown_primitive_is_skipped_in_typed_graph() {
    // Round-8 added feColorMatrix and round-9 added feConvolveMatrix /
    // feTurbulence / feDisplacementMap to the typed-graph allowlist, so
    // we use a still-unknown primitive (feDiffuseLighting) here.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feGaussianBlur stdDeviation="1"/>
          <feDiffuseLighting surfaceScale="1" diffuseConstant="1"/>
          <feOffset dx="1" dy="1"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(
        g.primitives.len(),
        2,
        "feDiffuseLighting isn't yet typed; should be skipped"
    );
}

#[test]
fn round_trip_preserves_unknown_primitives_via_extras() {
    // The typed graph drops unknown primitives, but the verbatim XML
    // round-trip keeps them — this is the round-4 invariant we must
    // not regress.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feGaussianBlur stdDeviation="2"/>
          <feDiffuseLighting surfaceScale="1" diffuseConstant="1"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("feGaussianBlur"));
    assert!(
        s.contains("feDiffuseLighting"),
        "verbatim XML must keep unknown primitives: {s}"
    );
}

#[test]
fn filter_region_x_y_w_h_are_captured() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f" x="-5" y="-10" width="120" height="80">
          <feGaussianBlur stdDeviation="1"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.region.x, Some(-5.0));
    assert_eq!(g.region.y, Some(-10.0));
    assert_eq!(g.region.width, Some(120.0));
    assert_eq!(g.region.height, Some(80.0));
}
