//! Round 12 — filter-element coordinate-system and colour-space
//! attributes. The typed primitive set was complete after round 11, but
//! the `<filter>` element's own `filterUnits` / `primitiveUnits` /
//! `color-interpolation-filters` attributes — which govern how the
//! region and the per-primitive length values are interpreted, and in
//! which colour space the primitive maths runs — were not captured in
//! the typed graph. This round adds them per SVG 1.1 §15.7.2 and
//! §11.7.1.
//!
//! Mirrors the round-8..11 layout — typed-graph parsing assertions plus
//! a verbatim-XML round-trip check.

use oxideav_svg::filter::{ColorInterpolationFilters, FilterUnits};
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

// ---- filterUnits / primitiveUnits defaults ----

#[test]
fn units_default_when_absent() {
    // Per SVG 1.1 §15.7.2 the two attributes have *different* defaults:
    // filterUnits → objectBoundingBox, primitiveUnits → userSpaceOnUse.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f"><feFlood flood-color="red"/></filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.filter_units, FilterUnits::ObjectBoundingBox);
    assert_eq!(g.primitive_units, FilterUnits::UserSpaceOnUse);
}

#[test]
fn units_explicit_values() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f" filterUnits="userSpaceOnUse"
                       primitiveUnits="objectBoundingBox">
          <feFlood flood-color="red"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.filter_units, FilterUnits::UserSpaceOnUse);
    assert_eq!(g.primitive_units, FilterUnits::ObjectBoundingBox);
}

#[test]
fn units_unknown_value_falls_back_to_default() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f" filterUnits="bogus" primitiveUnits="nonsense">
          <feFlood flood-color="red"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.filter_units, FilterUnits::ObjectBoundingBox);
    assert_eq!(g.primitive_units, FilterUnits::UserSpaceOnUse);
}

// ---- color-interpolation-filters ----

#[test]
fn cif_initial_value_is_linear_rgb() {
    // §11.7.1: the initial value of color-interpolation-filters is
    // linearRGB (note this differs from color-interpolation's sRGB).
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f"><feFlood flood-color="red"/></filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.color_interpolation_filters, None);
    assert_eq!(
        g.primitives[0].color_interpolation_filters,
        ColorInterpolationFilters::LinearRgb
    );
    // The enum's Default also reflects the initial value.
    assert_eq!(
        ColorInterpolationFilters::default(),
        ColorInterpolationFilters::LinearRgb
    );
}

#[test]
fn cif_on_filter_inherited_by_primitive() {
    // Set on <filter>; the primitive has no own value, so it inherits.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f" color-interpolation-filters="sRGB">
          <feFlood flood-color="red"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    assert_eq!(
        g.color_interpolation_filters,
        Some(ColorInterpolationFilters::Srgb)
    );
    assert_eq!(
        g.primitives[0].color_interpolation_filters,
        ColorInterpolationFilters::Srgb
    );
}

#[test]
fn cif_primitive_own_value_wins_over_filter() {
    // <filter> says sRGB but the primitive overrides with linearRGB.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f" color-interpolation-filters="sRGB">
          <feFlood flood-color="red" color-interpolation-filters="linearRGB"/>
          <feGaussianBlur stdDeviation="2"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    // First primitive overrides; second inherits the filter's sRGB.
    assert_eq!(
        g.primitives[0].color_interpolation_filters,
        ColorInterpolationFilters::LinearRgb
    );
    assert_eq!(
        g.primitives[1].color_interpolation_filters,
        ColorInterpolationFilters::Srgb
    );
}

#[test]
fn cif_auto_value() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feFlood flood-color="red" color-interpolation-filters="auto"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    assert_eq!(
        g.primitives[0].color_interpolation_filters,
        ColorInterpolationFilters::Auto
    );
}

#[test]
fn cif_inherit_falls_back_to_initial() {
    // `inherit` with no cascade context resolves to the initial value
    // linearRGB (the per-primitive override collapses to None → default).
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feFlood flood-color="red" color-interpolation-filters="inherit"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    assert_eq!(
        g.primitives[0].color_interpolation_filters,
        ColorInterpolationFilters::LinearRgb
    );
}

// ---- verbatim round-trip ----

#[test]
fn filter_units_survive_xml_round_trip() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><filter id="f" filterUnits="userSpaceOnUse" primitiveUnits="objectBoundingBox" color-interpolation-filters="sRGB"><feFlood flood-color="red"/></filter></defs><rect width="10" height="10" filter="url(#f)"/></svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    let out = write_svg_with_extras(&frame, &extras);
    let out_str = String::from_utf8(out).expect("utf8");
    assert!(out_str.contains("filterUnits=\"userSpaceOnUse\""));
    assert!(out_str.contains("primitiveUnits=\"objectBoundingBox\""));
    assert!(out_str.contains("color-interpolation-filters=\"sRGB\""));
}
