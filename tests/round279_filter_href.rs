//! Round 279 — the `<filter>` element's remaining SVG 1.1 §15.3
//! attributes on the typed graph: `filterRes` (§15.5 intermediate-image
//! resolution) and `xlink:href` / `href` cross-filter inheritance
//! ("any attributes which are defined on the referenced 'filter'
//! element which are not defined on this element are inherited"; an
//! element with no filter nodes inherits the referenced element's
//! filter nodes; indirect to an arbitrary level).
//!
//! Mirrors the round-7..12 layout — typed-graph assertions via
//! `parse_svg_with_extras` + `parse_filter_graph`, plus a verbatim-XML
//! round-trip check.

use oxideav_svg::filter::{
    parse_filter_graph, resolve_filter_element_chain, ColorInterpolationFilters, FilterGraph,
    FilterPrimitive, FilterRes, FilterUnits,
};
use oxideav_svg::parser::Element;
use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

fn filter_element<'a>(filters: &'a [Element], filter_id: &str) -> &'a Element {
    filters
        .iter()
        .find(|el| {
            el.attrs
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("id") && v == filter_id)
        })
        .unwrap_or_else(|| panic!("no <filter id=\"{filter_id}\"> in extras"))
}

/// Typed graph straight off the element — no href resolution.
fn raw_graph(src: &[u8], filter_id: &str) -> FilterGraph {
    let (_frame, extras) = parse_svg_with_extras(src).expect("parse_svg_with_extras");
    parse_filter_graph(filter_element(&extras.filters, filter_id))
}

/// Typed graph after §15.3 cross-filter `href` chain resolution.
fn resolved_graph(src: &[u8], filter_id: &str) -> FilterGraph {
    let (_frame, extras) = parse_svg_with_extras(src).expect("parse_svg_with_extras");
    let el = filter_element(&extras.filters, filter_id);
    let merged = resolve_filter_element_chain(el, &extras.filters);
    parse_filter_graph(&merged)
}

fn doc(defs: &str) -> Vec<u8> {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10">
        <defs>{defs}</defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##
    )
    .into_bytes()
}

// ---- filterRes (SVG 1.1 §15.5) ----

#[test]
fn filter_res_absent_is_none() {
    let src = doc(r#"<filter id="f"><feOffset dx="1" dy="1"/></filter>"#);
    assert_eq!(raw_graph(&src, "f").filter_res, None);
}

#[test]
fn filter_res_two_numbers() {
    let src = doc(r#"<filter id="f" filterRes="200 100"><feOffset/></filter>"#);
    assert_eq!(
        raw_graph(&src, "f").filter_res,
        Some(FilterRes {
            x_pixels: 200.0,
            y_pixels: 100.0,
        })
    );
}

#[test]
fn filter_res_single_number_expands_to_both_axes() {
    // §15.3 (under `primitiveUnits`): "if only one number was specified
    // in a <number-optional-number> value this number is expanded out".
    let src = doc(r#"<filter id="f" filterRes="64"><feOffset/></filter>"#);
    assert_eq!(
        raw_graph(&src, "f").filter_res,
        Some(FilterRes {
            x_pixels: 64.0,
            y_pixels: 64.0,
        })
    );
}

#[test]
fn filter_res_comma_separated_pair() {
    let src = doc(r#"<filter id="f" filterRes="100, 50"><feOffset/></filter>"#);
    assert_eq!(
        raw_graph(&src, "f").filter_res,
        Some(FilterRes {
            x_pixels: 100.0,
            y_pixels: 50.0,
        })
    );
}

#[test]
fn filter_res_non_integer_truncates_toward_zero() {
    // §15.5: "Non-integer values are truncated, i.e rounded to the
    // closest integer value towards zero."
    let src = doc(r#"<filter id="f" filterRes="10.7 5.9"><feOffset/></filter>"#);
    assert_eq!(
        raw_graph(&src, "f").filter_res,
        Some(FilterRes {
            x_pixels: 10.0,
            y_pixels: 5.0,
        })
    );
}

#[test]
fn filter_res_negative_truncates_toward_zero_and_is_captured() {
    // Negative values are an error per §15.5 (error processing is the
    // rasteriser's job); truncation toward zero still applies at the
    // value level: -2.5 → -2.
    let src = doc(r#"<filter id="f" filterRes="-2.5 3"><feOffset/></filter>"#);
    assert_eq!(
        raw_graph(&src, "f").filter_res,
        Some(FilterRes {
            x_pixels: -2.0,
            y_pixels: 3.0,
        })
    );
}

#[test]
fn filter_res_zero_is_captured_not_dropped() {
    // §15.5: zero values disable rendering of the referencing element —
    // captured so the rasteriser can apply that semantics.
    let src = doc(r#"<filter id="f" filterRes="0"><feOffset/></filter>"#);
    assert_eq!(
        raw_graph(&src, "f").filter_res,
        Some(FilterRes {
            x_pixels: 0.0,
            y_pixels: 0.0,
        })
    );
}

#[test]
fn filter_res_malformed_is_none() {
    let src = doc(r#"<filter id="f" filterRes="auto"><feOffset/></filter>"#);
    assert_eq!(raw_graph(&src, "f").filter_res, None);
}

// ---- href capture ----

#[test]
fn xlink_href_captured_as_bare_id() {
    let src = doc(r##"<filter id="a"><feOffset/></filter>
            <filter id="f" xlink:href="#a"><feFlood/></filter>"##);
    assert_eq!(raw_graph(&src, "f").href.as_deref(), Some("a"));
}

#[test]
fn svg2_href_spelling_captured() {
    let src = doc(r##"<filter id="a"><feOffset/></filter>
            <filter id="f" href="#a"><feFlood/></filter>"##);
    assert_eq!(raw_graph(&src, "f").href.as_deref(), Some("a"));
}

#[test]
fn href_absent_is_none() {
    let src = doc(r#"<filter id="f"><feOffset/></filter>"#);
    assert_eq!(raw_graph(&src, "f").href, None);
}

// ---- §15.3 attribute inheritance ----

#[test]
fn attributes_inherited_from_referenced_filter() {
    let src = doc(r##"<filter id="a" x="1" y="2" width="30" height="40"
                    filterUnits="userSpaceOnUse" primitiveUnits="objectBoundingBox"
                    filterRes="128">
              <feOffset dx="1" dy="1"/>
            </filter>
            <filter id="f" xlink:href="#a"><feFlood/></filter>"##);
    let g = resolved_graph(&src, "f");
    assert_eq!(g.region.x, Some(1.0));
    assert_eq!(g.region.y, Some(2.0));
    assert_eq!(g.region.width, Some(30.0));
    assert_eq!(g.region.height, Some(40.0));
    assert_eq!(g.filter_units, FilterUnits::UserSpaceOnUse);
    assert_eq!(g.primitive_units, FilterUnits::ObjectBoundingBox);
    assert_eq!(
        g.filter_res,
        Some(FilterRes {
            x_pixels: 128.0,
            y_pixels: 128.0,
        })
    );
    // `f` has its own filter node, so primitives are NOT inherited.
    assert_eq!(g.primitives.len(), 1);
    assert!(matches!(
        g.primitives[0].primitive,
        FilterPrimitive::Flood { .. }
    ));
}

#[test]
fn own_attribute_wins_over_referenced() {
    let src = doc(
        r##"<filter id="a" x="1" filterRes="128"><feOffset/></filter>
            <filter id="f" xlink:href="#a" x="5"><feFlood/></filter>"##,
    );
    let g = resolved_graph(&src, "f");
    assert_eq!(g.region.x, Some(5.0));
    // Absent on `f` → inherited from `a`.
    assert_eq!(
        g.filter_res,
        Some(FilterRes {
            x_pixels: 128.0,
            y_pixels: 128.0,
        })
    );
}

#[test]
fn color_interpolation_filters_inherited_via_href() {
    let src = doc(
        r##"<filter id="a" color-interpolation-filters="sRGB"><feOffset/></filter>
            <filter id="f" xlink:href="#a"><feFlood/></filter>"##,
    );
    let g = resolved_graph(&src, "f");
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
fn spec_defaults_survive_a_silent_chain() {
    // Neither element sets the units → §15.7.2 defaults stand
    // (filterUnits → objectBoundingBox, primitiveUnits → userSpaceOnUse).
    let src = doc(r##"<filter id="a"><feOffset/></filter>
            <filter id="f" xlink:href="#a"><feFlood/></filter>"##);
    let g = resolved_graph(&src, "f");
    assert_eq!(g.filter_units, FilterUnits::ObjectBoundingBox);
    assert_eq!(g.primitive_units, FilterUnits::UserSpaceOnUse);
    assert_eq!(g.filter_res, None);
}

// ---- §15.3 filter-node inheritance ----

#[test]
fn filter_nodes_inherited_when_self_has_none() {
    let src = doc(
        r##"<filter id="a"><feGaussianBlur stdDeviation="2"/></filter>
            <filter id="f" xlink:href="#a"/>"##,
    );
    let g = resolved_graph(&src, "f");
    assert_eq!(g.primitives.len(), 1);
    assert!(matches!(
        g.primitives[0].primitive,
        FilterPrimitive::GaussianBlur { .. }
    ));
}

#[test]
fn filter_nodes_not_inherited_when_self_has_nodes() {
    let src = doc(
        r##"<filter id="a"><feGaussianBlur stdDeviation="2"/></filter>
            <filter id="f" xlink:href="#a"><feOffset dx="3" dy="3"/></filter>"##,
    );
    let g = resolved_graph(&src, "f");
    assert_eq!(g.primitives.len(), 1);
    assert!(matches!(
        g.primitives[0].primitive,
        FilterPrimitive::Offset { .. }
    ));
}

#[test]
fn unrecognised_fe_child_still_counts_as_filter_nodes() {
    // §15.3 speaks of "defined filter nodes", not "nodes this
    // implementation types" — an unknown fe* child blocks node
    // inheritance even though the typed graph skips it.
    let src = doc(
        r##"<filter id="a"><feGaussianBlur stdDeviation="2"/></filter>
            <filter id="f" xlink:href="#a"><feFutureThing/></filter>"##,
    );
    let g = resolved_graph(&src, "f");
    assert!(g.primitives.is_empty());
}

#[test]
fn indirect_chain_resolves_attributes_and_nodes() {
    // c → b → a: attributes accumulate nearest-wins; the filter nodes
    // come from the nearest chain member that has any (`a`).
    let src = doc(
        r##"<filter id="a" x="1"><feGaussianBlur stdDeviation="2"/></filter>
            <filter id="b" xlink:href="#a" filterRes="64"/>
            <filter id="f" xlink:href="#b" y="9"/>"##,
    );
    let g = resolved_graph(&src, "f");
    assert_eq!(g.region.x, Some(1.0));
    assert_eq!(g.region.y, Some(9.0));
    assert_eq!(
        g.filter_res,
        Some(FilterRes {
            x_pixels: 64.0,
            y_pixels: 64.0,
        })
    );
    assert_eq!(g.primitives.len(), 1);
    assert!(matches!(
        g.primitives[0].primitive,
        FilterPrimitive::GaussianBlur { .. }
    ));
}

#[test]
fn reference_cycle_terminates_with_own_state() {
    let src = doc(
        r##"<filter id="a" xlink:href="#f" x="1"><feGaussianBlur stdDeviation="2"/></filter>
            <filter id="f" xlink:href="#a"><feFlood/></filter>"##,
    );
    let g = resolved_graph(&src, "f");
    // Own node kept; `a`'s attribute still inherited from the one hop
    // that completed before the cycle was detected.
    assert_eq!(g.primitives.len(), 1);
    assert!(matches!(
        g.primitives[0].primitive,
        FilterPrimitive::Flood { .. }
    ));
    assert_eq!(g.region.x, Some(1.0));
}

#[test]
fn self_reference_terminates() {
    let src = doc(r##"<filter id="f" xlink:href="#f"><feFlood/></filter>"##);
    let g = resolved_graph(&src, "f");
    assert_eq!(g.primitives.len(), 1);
    assert_eq!(g.href.as_deref(), Some("f"));
}

#[test]
fn dangling_reference_is_inert() {
    let src = doc(r##"<filter id="f" xlink:href="#nothere" x="3"><feFlood/></filter>"##);
    let g = resolved_graph(&src, "f");
    assert_eq!(g.region.x, Some(3.0));
    assert_eq!(g.href.as_deref(), Some("nothere"));
    assert_eq!(g.primitives.len(), 1);
}

// ---- round-trip ----

#[test]
fn filter_res_and_href_survive_verbatim_round_trip() {
    let src = doc(
        r##"<filter id="a" filterRes="200 100"><feOffset dx="1" dy="1"/></filter>
            <filter id="f" xlink:href="#a"/>"##,
    );
    let (frame, extras) = parse_svg_with_extras(&src).expect("parse");
    let out = write_svg_with_extras(&frame, &extras);
    let text = String::from_utf8(out).expect("utf8");
    assert!(text.contains(r#"filterRes="200 100""#), "{text}");
    assert!(text.contains(r##"xlink:href="#a""##), "{text}");
    // And the re-parsed document still resolves the chain.
    let g = resolved_graph(text.as_bytes(), "f");
    assert_eq!(
        g.filter_res,
        Some(FilterRes {
            x_pixels: 200.0,
            y_pixels: 100.0,
        })
    );
    assert_eq!(g.primitives.len(), 1);
}
