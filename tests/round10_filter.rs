//! Round 10 — long-tail filter primitives, part 3:
//! `<feDiffuseLighting>` and `<feSpecularLighting>` plus their three
//! light-source children (`<feDistantLight>` / `<fePointLight>` /
//! `<feSpotLight>`). Per W3C Filter Effects §18 / §19 / §20.
//!
//! Mirrors the round-8 / round-9 layout: each primitive has typed-graph
//! parsing tests + a verbatim-XML round-trip test (the rasterizer does
//! not yet consume the new primitives but the round-trip path must keep
//! them intact for downstream consumers).

use oxideav_svg::filter::{FilterInput, FilterPrimitive, LightSource};
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

// ---- feDiffuseLighting ----

#[test]
fn diffuse_lighting_with_distant_light_records_all_attrs() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feDiffuseLighting in="SourceAlpha" surfaceScale="2" diffuseConstant="0.7" lighting-color="#00ff00">
            <feDistantLight azimuth="135" elevation="55"/>
          </feDiffuseLighting>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 1);
    let FilterPrimitive::DiffuseLighting {
        input,
        surface_scale,
        diffuse_constant,
        lighting_color,
        light_source,
        ..
    } = &g.primitives[0].primitive
    else {
        panic!("not DiffuseLighting");
    };
    assert_eq!(*input, FilterInput::SourceAlpha);
    assert_eq!(*surface_scale, 2.0);
    assert!((*diffuse_constant - 0.7).abs() < 1e-6);
    assert_eq!(lighting_color.g, 0xff);
    match light_source {
        LightSource::Distant { azimuth, elevation } => {
            assert_eq!(*azimuth, 135.0);
            assert_eq!(*elevation, 55.0);
        }
        other => panic!("expected Distant, got {:?}", other),
    }
}

#[test]
fn diffuse_lighting_default_attrs() {
    // Bare element — surfaceScale=1, diffuseConstant=1,
    // kernelUnitLength=None, lighting-color=white, default light.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f"><feDiffuseLighting/></filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::DiffuseLighting {
        surface_scale,
        diffuse_constant,
        kernel_unit_length,
        lighting_color,
        light_source,
        ..
    } = &g.primitives[0].primitive
    else {
        panic!("not DiffuseLighting");
    };
    assert_eq!(*surface_scale, 1.0);
    assert_eq!(*diffuse_constant, 1.0);
    assert_eq!(*kernel_unit_length, None);
    // White (255,255,255,255).
    assert_eq!(lighting_color.r, 255);
    assert_eq!(lighting_color.g, 255);
    assert_eq!(lighting_color.b, 255);
    assert_eq!(*light_source, LightSource::default());
}

// ---- feSpecularLighting ----

#[test]
fn specular_lighting_with_point_light_records_all_attrs() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feSpecularLighting surfaceScale="5" specularConstant="0.4" specularExponent="40" kernelUnitLength="2">
            <fePointLight x="10" y="20" z="30"/>
          </feSpecularLighting>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::SpecularLighting {
        surface_scale,
        specular_constant,
        specular_exponent,
        kernel_unit_length,
        light_source,
        ..
    } = &g.primitives[0].primitive
    else {
        panic!("not SpecularLighting");
    };
    assert_eq!(*surface_scale, 5.0);
    assert!((*specular_constant - 0.4).abs() < 1e-6);
    assert_eq!(*specular_exponent, 40.0);
    // Single number mirrors per spec §19.4.
    assert_eq!(*kernel_unit_length, Some((2.0, 2.0)));
    match light_source {
        LightSource::Point { x, y, z } => {
            assert_eq!(*x, 10.0);
            assert_eq!(*y, 20.0);
            assert_eq!(*z, 30.0);
        }
        other => panic!("expected Point, got {:?}", other),
    }
}

#[test]
fn specular_lighting_default_specular_exponent_is_one() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feSpecularLighting>
            <fePointLight x="1" y="2" z="3"/>
          </feSpecularLighting>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::SpecularLighting {
        specular_constant,
        specular_exponent,
        ..
    } = &g.primitives[0].primitive
    else {
        panic!("not SpecularLighting");
    };
    assert_eq!(*specular_constant, 1.0);
    assert_eq!(*specular_exponent, 1.0);
}

// ---- feSpotLight (full eight-attribute form) ----

#[test]
fn spot_light_records_full_eight_attribute_form() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feSpecularLighting>
            <feSpotLight x="0" y="0" z="100" pointsAtX="50" pointsAtY="50" pointsAtZ="0" specularExponent="3" limitingConeAngle="45"/>
          </feSpecularLighting>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::SpecularLighting { light_source, .. } = &g.primitives[0].primitive else {
        panic!("not SpecularLighting");
    };
    match light_source {
        LightSource::Spot {
            x,
            y,
            z,
            points_at_x,
            points_at_y,
            points_at_z,
            specular_exponent,
            limiting_cone_angle,
        } => {
            assert_eq!(*x, 0.0);
            assert_eq!(*y, 0.0);
            assert_eq!(*z, 100.0);
            assert_eq!(*points_at_x, 50.0);
            assert_eq!(*points_at_y, 50.0);
            assert_eq!(*points_at_z, 0.0);
            assert_eq!(*specular_exponent, 3.0);
            assert_eq!(*limiting_cone_angle, Some(45.0));
        }
        other => panic!("expected Spot, got {:?}", other),
    }
}

#[test]
fn spot_light_without_limiting_cone_angle_is_unbounded() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feDiffuseLighting>
            <feSpotLight x="0" y="0" z="50" pointsAtX="0" pointsAtY="0" pointsAtZ="0"/>
          </feDiffuseLighting>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::DiffuseLighting { light_source, .. } = &g.primitives[0].primitive else {
        panic!("not DiffuseLighting");
    };
    match light_source {
        LightSource::Spot {
            limiting_cone_angle,
            ..
        } => assert_eq!(*limiting_cone_angle, None),
        other => panic!("expected Spot, got {:?}", other),
    }
}

#[test]
fn lighting_threads_input_chain_from_previous_result() {
    // No `in=` -> defaults to previous primitive's `result`.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feGaussianBlur stdDeviation="1" result="b"/>
          <feDiffuseLighting>
            <feDistantLight azimuth="0" elevation="45"/>
          </feDiffuseLighting>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::DiffuseLighting { input, .. } = &g.primitives[1].primitive else {
        panic!("not DiffuseLighting");
    };
    assert_eq!(*input, FilterInput::Reference("b".into()));
}

#[test]
fn lighting_color_currentcolor_resolves_to_opaque_black() {
    // `currentColor` cannot be resolved without inheritance context;
    // we mirror the rest of this crate (color.rs) and fall back to
    // opaque black — the same behaviour `<feFlood flood-color>`
    // already exhibits.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feDiffuseLighting lighting-color="currentColor">
            <feDistantLight/>
          </feDiffuseLighting>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::DiffuseLighting { lighting_color, .. } = &g.primitives[0].primitive else {
        panic!("not DiffuseLighting");
    };
    assert_eq!(lighting_color.r, 0);
    assert_eq!(lighting_color.g, 0);
    assert_eq!(lighting_color.b, 0);
    assert_eq!(lighting_color.a, 255);
}

// ---- Round-trip preservation ----

#[test]
fn round_trip_preserves_round10_lighting_primitives_verbatim() {
    // The verbatim-XML round-trip path must keep the new primitives
    // intact even though the typed-graph allowlist now recognises
    // them — encoding still re-emits the original element trees.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
        <defs><filter id="f">
          <feDiffuseLighting in="SourceAlpha" surfaceScale="2" diffuseConstant="0.8" lighting-color="#ff0000" result="lit">
            <feDistantLight azimuth="45" elevation="60"/>
          </feDiffuseLighting>
          <feSpecularLighting in="SourceAlpha" surfaceScale="3" specularConstant="0.6" specularExponent="20" lighting-color="#0000ff" result="hi">
            <feSpotLight x="0" y="0" z="100" pointsAtX="50" pointsAtY="50" pointsAtZ="0" specularExponent="2" limitingConeAngle="30"/>
          </feSpecularLighting>
          <feMerge>
            <feMergeNode in="lit"/>
            <feMergeNode in="hi"/>
          </feMerge>
        </filter></defs>
        <rect width="50" height="50" filter="url(#f)"/>
      </svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    let bytes = write_svg_with_extras(&frame, &extras);
    let s = std::str::from_utf8(&bytes).unwrap();
    assert!(s.contains("feDiffuseLighting"));
    assert!(s.contains("feSpecularLighting"));
    assert!(s.contains("feDistantLight"));
    assert!(s.contains("feSpotLight"));
    assert!(s.contains("limitingConeAngle"));
    // Re-parse to confirm a second round still parses to typed graph.
    let (_frame2, extras2) = parse_svg_with_extras(&bytes).expect("re-parse");
    assert_eq!(extras2.filters.len(), 1);
}

#[test]
fn mixed_pipeline_round10_with_round7_round8_round9_primitives() {
    // Realistic recipe — turbulence height-map driving displacement,
    // followed by diffuse lighting + colour matrix tint.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
        <defs><filter id="f">
          <feTurbulence baseFrequency="0.04" seed="1" result="n"/>
          <feDisplacementMap in="SourceGraphic" in2="n" scale="3" xChannelSelector="R" yChannelSelector="G" result="d"/>
          <feDiffuseLighting in="d" surfaceScale="2" lighting-color="#ffeeaa" result="lit">
            <fePointLight x="20" y="20" z="50"/>
          </feDiffuseLighting>
          <feColorMatrix in="lit" type="saturate" values="0.7"/>
        </filter></defs>
        <rect width="50" height="50" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 4);
    assert!(matches!(
        g.primitives[0].primitive,
        FilterPrimitive::Turbulence { .. }
    ));
    assert!(matches!(
        g.primitives[1].primitive,
        FilterPrimitive::DisplacementMap { .. }
    ));
    assert!(matches!(
        g.primitives[2].primitive,
        FilterPrimitive::DiffuseLighting { .. }
    ));
    assert!(matches!(
        g.primitives[3].primitive,
        FilterPrimitive::ColorMatrix { .. }
    ));
}

#[test]
fn diffuse_and_specular_share_the_same_light_source_enum() {
    // Sanity: the same LightSource variant carries the same data
    // regardless of which lighting primitive owns it.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs>
          <filter id="d"><feDiffuseLighting><feDistantLight azimuth="30" elevation="60"/></feDiffuseLighting></filter>
          <filter id="s"><feSpecularLighting><feDistantLight azimuth="30" elevation="60"/></feSpecularLighting></filter>
        </defs>
        <rect width="10" height="10" filter="url(#d)"/>
      </svg>"##;
    let dg = graph_for_filter(src, "d");
    let sg = graph_for_filter(src, "s");
    let FilterPrimitive::DiffuseLighting {
        light_source: dl, ..
    } = &dg.primitives[0].primitive
    else {
        panic!("d not DiffuseLighting");
    };
    let FilterPrimitive::SpecularLighting {
        light_source: sl, ..
    } = &sg.primitives[0].primitive
    else {
        panic!("s not SpecularLighting");
    };
    assert_eq!(*dl, *sl);
}
