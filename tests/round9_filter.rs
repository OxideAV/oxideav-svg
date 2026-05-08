//! Round 9 — long-tail filter primitives, part 2: `<feConvolveMatrix>`,
//! `<feTurbulence>`, `<feDisplacementMap>`.
//!
//! Mirrors the round-8 layout: each primitive has typed-graph parsing
//! tests + a verbatim-XML round-trip test (the rasterizer does not yet
//! consume the new primitives but the round-trip path must keep them
//! intact for downstream consumers).

use oxideav_svg::filter::{
    ChannelSelector, ConvolveEdgeMode, FilterInput, FilterPrimitive, TurbulenceKind,
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

// ---- feConvolveMatrix ----

#[test]
fn convolve_matrix_3x3_sharpen_kernel_is_recorded_row_major() {
    // Classic 3×3 sharpen kernel — sums to 1 so divisor defaults to 1.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feConvolveMatrix order="3" kernelMatrix="0 -1 0  -1 5 -1  0 -1 0"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 1);
    let FilterPrimitive::ConvolveMatrix {
        order_x,
        order_y,
        kernel_matrix,
        divisor,
        bias,
        target_x,
        target_y,
        edge_mode,
        preserve_alpha,
        ..
    } = &g.primitives[0].primitive
    else {
        panic!("not ConvolveMatrix");
    };
    assert_eq!(*order_x, 3);
    assert_eq!(*order_y, 3);
    assert_eq!(kernel_matrix.len(), 9);
    assert_eq!(kernel_matrix[4], 5.0);
    // Sum is 1, so default divisor is 1.
    assert_eq!(*divisor, 1.0);
    assert_eq!(*bias, 0.0);
    assert_eq!(*target_x, 1);
    assert_eq!(*target_y, 1);
    assert_eq!(*edge_mode, ConvolveEdgeMode::Duplicate);
    assert!(!*preserve_alpha);
}

#[test]
fn convolve_matrix_with_explicit_target_and_edge_mode() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feConvolveMatrix order="3" kernelMatrix="1 1 1  1 1 1  1 1 1" divisor="9" bias="0.1" targetX="0" targetY="2" edgeMode="wrap" preserveAlpha="true"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::ConvolveMatrix {
        divisor,
        bias,
        target_x,
        target_y,
        edge_mode,
        preserve_alpha,
        ..
    } = &g.primitives[0].primitive
    else {
        panic!("not ConvolveMatrix");
    };
    assert_eq!(*divisor, 9.0);
    assert!((*bias - 0.1).abs() < 1e-6);
    assert_eq!(*target_x, 0);
    assert_eq!(*target_y, 2);
    assert_eq!(*edge_mode, ConvolveEdgeMode::Wrap);
    assert!(*preserve_alpha);
}

#[test]
fn convolve_matrix_threads_input_chain_from_previous_result() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feGaussianBlur stdDeviation="1" result="b"/>
          <feConvolveMatrix order="3" kernelMatrix="0 0 0  0 1 0  0 0 0"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::ConvolveMatrix { input, .. } = &g.primitives[1].primitive else {
        panic!("expected ConvolveMatrix");
    };
    assert_eq!(*input, FilterInput::Reference("b".into()));
}

#[test]
fn convolve_matrix_zero_sum_kernel_falls_back_to_divisor_one() {
    // Edge-detect kernel sums to 0; per spec §15.2 divisor defaults
    // to 1 in that case (avoids divide-by-zero).
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feConvolveMatrix order="3" kernelMatrix="-1 -1 -1  -1 8 -1  -1 -1 -1"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::ConvolveMatrix { divisor, .. } = &g.primitives[0].primitive else {
        panic!("not ConvolveMatrix");
    };
    assert_eq!(*divisor, 1.0);
}

// ---- feTurbulence ----

#[test]
fn turbulence_records_base_frequency_octaves_and_seed() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feTurbulence baseFrequency="0.05" numOctaves="3" seed="7"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::Turbulence {
        base_frequency_x,
        base_frequency_y,
        num_octaves,
        seed,
        stitch_tiles,
        kind,
    } = &g.primitives[0].primitive
    else {
        panic!("not Turbulence");
    };
    assert!((*base_frequency_x - 0.05).abs() < 1e-6);
    // Single value mirrors fy=fx per §16.3.
    assert!((*base_frequency_y - 0.05).abs() < 1e-6);
    assert_eq!(*num_octaves, 3);
    assert_eq!(*seed, 7);
    assert!(!*stitch_tiles);
    assert_eq!(*kind, TurbulenceKind::Turbulence);
}

#[test]
fn turbulence_distinct_two_axis_base_frequency() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feTurbulence baseFrequency="0.05 0.2"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::Turbulence {
        base_frequency_x,
        base_frequency_y,
        ..
    } = &g.primitives[0].primitive
    else {
        panic!("not Turbulence");
    };
    assert!((*base_frequency_x - 0.05).abs() < 1e-6);
    assert!((*base_frequency_y - 0.2).abs() < 1e-6);
}

#[test]
fn turbulence_fractal_noise_with_stitch_tiles() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feTurbulence type="fractalNoise" baseFrequency="0.02" stitchTiles="stitch"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::Turbulence {
        kind, stitch_tiles, ..
    } = &g.primitives[0].primitive
    else {
        panic!("not Turbulence");
    };
    assert_eq!(*kind, TurbulenceKind::FractalNoise);
    assert!(*stitch_tiles);
}

#[test]
fn turbulence_default_num_octaves_is_one() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feTurbulence baseFrequency="0.1"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::Turbulence { num_octaves, .. } = &g.primitives[0].primitive else {
        panic!("not Turbulence");
    };
    assert_eq!(*num_octaves, 1);
}

// ---- feDisplacementMap ----

#[test]
fn displacement_map_records_scale_and_channel_selectors() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feTurbulence baseFrequency="0.05" result="noise"/>
          <feDisplacementMap in="SourceGraphic" in2="noise" scale="20" xChannelSelector="R" yChannelSelector="G"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 2);
    let FilterPrimitive::DisplacementMap {
        input,
        input2,
        scale,
        x_channel_selector,
        y_channel_selector,
    } = &g.primitives[1].primitive
    else {
        panic!("not DisplacementMap");
    };
    assert_eq!(*input, FilterInput::SourceGraphic);
    assert_eq!(*input2, FilterInput::Reference("noise".into()));
    assert_eq!(*scale, 20.0);
    assert_eq!(*x_channel_selector, ChannelSelector::R);
    assert_eq!(*y_channel_selector, ChannelSelector::G);
}

#[test]
fn displacement_map_default_channel_selectors_are_alpha() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feDisplacementMap scale="5"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::DisplacementMap {
        x_channel_selector,
        y_channel_selector,
        scale,
        ..
    } = &g.primitives[0].primitive
    else {
        panic!("not DisplacementMap");
    };
    assert_eq!(*x_channel_selector, ChannelSelector::A);
    assert_eq!(*y_channel_selector, ChannelSelector::A);
    assert_eq!(*scale, 5.0);
}

#[test]
fn displacement_map_threads_input_chain_from_previous_result() {
    // No `in=` -> defaults to previous primitive's `result`.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feGaussianBlur stdDeviation="1" result="b"/>
          <feDisplacementMap scale="5"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::DisplacementMap { input, .. } = &g.primitives[1].primitive else {
        panic!("not DisplacementMap");
    };
    assert_eq!(*input, FilterInput::Reference("b".into()));
}

// ---- Round-trip preservation ----

#[test]
fn round_trip_preserves_round9_primitives_verbatim() {
    // The verbatim-XML round-trip path must keep the new primitives
    // intact even though the typed-graph allowlist now recognises
    // them — encoding still re-emits the original element trees.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
        <defs><filter id="f">
          <feTurbulence type="fractalNoise" baseFrequency="0.05" numOctaves="2" seed="3" result="noise"/>
          <feDisplacementMap in="SourceGraphic" in2="noise" scale="10" xChannelSelector="R" yChannelSelector="G" result="warp"/>
          <feConvolveMatrix in="warp" order="3" kernelMatrix="0 -1 0  -1 5 -1  0 -1 0"/>
        </filter></defs>
        <rect width="50" height="50" filter="url(#f)"/>
      </svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    let bytes = write_svg_with_extras(&frame, &extras);
    let s = std::str::from_utf8(&bytes).unwrap();
    assert!(s.contains("feTurbulence"));
    assert!(s.contains("feDisplacementMap"));
    assert!(s.contains("feConvolveMatrix"));
    // Re-parse to confirm a second round still parses to typed graph.
    let (_frame2, extras2) = parse_svg_with_extras(&bytes).expect("re-parse");
    assert_eq!(extras2.filters.len(), 1);
}

#[test]
fn mixed_pipeline_round9_with_round7_round8_primitives() {
    // Realistic recipe — turbulence-driven displacement of a colour-
    // matrix-tinted SourceGraphic, followed by convolve-matrix sharpen.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
        <defs><filter id="f">
          <feTurbulence baseFrequency="0.04" seed="1" result="n"/>
          <feColorMatrix in="SourceGraphic" type="saturate" values="0.5" result="c"/>
          <feDisplacementMap in="c" in2="n" scale="3" xChannelSelector="R" yChannelSelector="G" result="d"/>
          <feConvolveMatrix in="d" order="3" kernelMatrix="0 -1 0  -1 5 -1  0 -1 0"/>
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
        FilterPrimitive::ColorMatrix { .. }
    ));
    assert!(matches!(
        g.primitives[2].primitive,
        FilterPrimitive::DisplacementMap { .. }
    ));
    assert!(matches!(
        g.primitives[3].primitive,
        FilterPrimitive::ConvolveMatrix { .. }
    ));
}
