//! Round 367 — end-to-end §9.4 filter-primitive subregion resolution.
//!
//! Round 361 added the subregion *clip* arithmetic
//! (`evaluate_filter_graph_clipped`) but left resolving each primitive's
//! `x` / `y` / `width` / `height` attributes to `PixelRect`s as a
//! rasteriser concern. Round 367 closes that with `resolve_subregions`,
//! and these tests drive it end-to-end from a parsed `<filter>` document
//! (so the §7 `<length-percentage>` capture in `RegionCoords` is
//! exercised through the real parser, not a hand-built graph).

use oxideav_svg::filter::{FilterCoord, FilterGraph, FilterUnits};
use oxideav_svg::filter_eval::{resolve_subregions, FilterSubregionCtx, PixelRect};
use oxideav_svg::parse_svg_with_extras;

fn graph_for_filter(src: &[u8], filter_id: &str) -> FilterGraph {
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

// A 200×200 user-space filter region mapped 1:1 onto a 200×200 px raster,
// element bbox = (0,0,180,180) — the §9.4 example's circle r=90 around
// (100,100) gives a [10,10]..[190,190] box, but for these unit-style
// assertions a round (0,0,180,180) box keeps the arithmetic legible.
fn ctx(primitive_units: FilterUnits) -> FilterSubregionCtx {
    FilterSubregionCtx {
        primitive_units,
        region_w_px: 200.0,
        region_h_px: 200.0,
        user_scale_x: 1.0,
        user_scale_y: 1.0,
        user_origin_x: 0.0,
        user_origin_y: 0.0,
        bbox_x_px: 0.0,
        bbox_y_px: 0.0,
        bbox_w_px: 180.0,
        bbox_h_px: 180.0,
    }
}

// The §9.4 worked example: a feFlood with x/y=25% width/height=50% inside
// a filter whose region is the whole canvas. Percentages resolve against
// the filter region (200×200) regardless of primitiveUnits.
#[test]
fn spec_example_flood_percentages() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
        <defs>
          <filter id="flood" x="0" y="0" width="100%" height="100%"
                  primitiveUnits="objectBoundingBox">
            <feFlood x="25%" y="25%" width="50%" height="50%"
                     flood-color="green" flood-opacity="0.75"/>
          </filter>
        </defs>
        <circle fill="green" filter="url(#flood)" cx="100" cy="100" r="90"/>
      </svg>"#;
    let g = graph_for_filter(src, "flood");
    // The percentage flag must survive parsing.
    assert_eq!(
        g.primitives[0].region_coords.x,
        Some(FilterCoord::Percentage(25.0))
    );
    let sub = resolve_subregions(&g, &ctx(FilterUnits::ObjectBoundingBox));
    assert_eq!(
        sub[0],
        Some(PixelRect {
            x: 50,
            y: 50,
            width: 100,
            height: 100
        })
    );
}

// A bare number under the default userSpaceOnUse is a user-space length.
#[test]
fn user_space_number_is_length() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
        <defs>
          <filter id="f">
            <feFlood x="20" y="30" width="40" height="50" flood-color="red"/>
          </filter>
        </defs>
        <rect width="180" height="180" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    assert_eq!(
        g.primitives[0].region_coords.x,
        Some(FilterCoord::Number(20.0))
    );
    let sub = resolve_subregions(&g, &ctx(FilterUnits::UserSpaceOnUse));
    assert_eq!(
        sub[0],
        Some(PixelRect {
            x: 20,
            y: 30,
            width: 40,
            height: 50
        })
    );
}

// A bare number under objectBoundingBox is a fraction of the bbox.
// bbox = (0,0,180,180): x=0.25 → 45, width=0.5 → 90.
#[test]
fn object_bounding_box_number_is_fraction() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
        <defs>
          <filter id="f" primitiveUnits="objectBoundingBox">
            <feFlood x="0.25" y="0.25" width="0.5" height="0.5" flood-color="red"/>
          </filter>
        </defs>
        <rect width="180" height="180" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    let sub = resolve_subregions(&g, &ctx(FilterUnits::ObjectBoundingBox));
    assert_eq!(
        sub[0],
        Some(PixelRect {
            x: 45,
            y: 45,
            width: 90,
            height: 90
        })
    );
}

// An feFlood with no subregion attributes defaults to the whole filter
// region (§9.4 — no referenced node).
#[test]
fn flood_no_attrs_defaults_to_full_region() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
        <defs>
          <filter id="f"><feFlood flood-color="red"/></filter>
        </defs>
        <rect width="180" height="180" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    let sub = resolve_subregions(&g, &ctx(FilterUnits::UserSpaceOnUse));
    assert_eq!(
        sub[0],
        Some(PixelRect {
            x: 0,
            y: 0,
            width: 200,
            height: 200
        })
    );
}

// §9.4 union default: a chained primitive with no subregion of its own
// inherits the resolved subregion of the result it references.
#[test]
fn chained_primitive_inherits_referenced_subregion() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
        <defs>
          <filter id="f">
            <feFlood x="20" y="20" width="40" height="40" flood-color="red" result="a"/>
            <feGaussianBlur in="a" stdDeviation="2"/>
          </filter>
        </defs>
        <rect width="180" height="180" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    let sub = resolve_subregions(&g, &ctx(FilterUnits::UserSpaceOnUse));
    // The blur references result "a", inheriting its (20,20,40,40) box.
    assert_eq!(
        sub[1],
        Some(PixelRect {
            x: 20,
            y: 20,
            width: 40,
            height: 40
        })
    );
}

// A primitive that references SourceGraphic defaults to the whole filter
// region even with no explicit attributes.
#[test]
fn source_graphic_reference_defaults_full_region() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
        <defs>
          <filter id="f">
            <feGaussianBlur in="SourceGraphic" stdDeviation="2"/>
          </filter>
        </defs>
        <rect width="180" height="180" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    let sub = resolve_subregions(&g, &ctx(FilterUnits::UserSpaceOnUse));
    assert_eq!(
        sub[0],
        Some(PixelRect {
            x: 0,
            y: 0,
            width: 200,
            height: 200
        })
    );
}

// feTile forces the whole filter region even when referencing a
// small-subregion result (§9.4).
#[test]
fn tile_forces_full_region() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
        <defs>
          <filter id="f">
            <feFlood x="10" y="10" width="20" height="20" flood-color="red" result="a"/>
            <feTile in="a"/>
          </filter>
        </defs>
        <rect width="180" height="180" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    let sub = resolve_subregions(&g, &ctx(FilterUnits::UserSpaceOnUse));
    assert_eq!(
        sub[1],
        Some(PixelRect {
            x: 0,
            y: 0,
            width: 200,
            height: 200
        })
    );
}

// A negative or zero width disables the primitive (§9.4): zero-extent.
#[test]
fn zero_width_disables_primitive() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
        <defs>
          <filter id="f">
            <feFlood x="10" y="10" width="0" height="20" flood-color="red"/>
          </filter>
        </defs>
        <rect width="180" height="180" filter="url(#f)"/>
      </svg>"#;
    let g = graph_for_filter(src, "f");
    let sub = resolve_subregions(&g, &ctx(FilterUnits::UserSpaceOnUse));
    assert_eq!(sub[0].unwrap().width, 0);
}
