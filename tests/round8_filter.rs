//! Round 8 — long-tail filter primitives: `<feColorMatrix>`,
//! `<feMerge>`, `<feComponentTransfer>`, `<feDropShadow>`.
//!
//! These tests verify the public-API surface that consumers
//! (e.g. oxideav-raster) will use to drive the new primitives. The
//! parser is exercised through `parse_svg_with_extras` →
//! `filter::parse_filter_graph`, mirroring the round-7 test layout.

use oxideav_svg::filter::{
    BlendMode, CompositeOperator, FilterInput, FilterPrimitive, FloodColor, TransferFunction,
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

#[test]
fn color_matrix_explicit_4x5_is_recorded_in_row_major() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f"><feColorMatrix type="matrix" values="
            1 0 0 0 0
            0 1 0 0 0
            0 0 1 0 0
            0 0 0 1 0"/></filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 1);
    let FilterPrimitive::ColorMatrix { matrix, .. } = &g.primitives[0].primitive else {
        panic!("not ColorMatrix");
    };
    // Identity diagonal.
    assert_eq!(matrix[0], 1.0);
    assert_eq!(matrix[6], 1.0);
    assert_eq!(matrix[12], 1.0);
    assert_eq!(matrix[18], 1.0);
}

#[test]
fn color_matrix_saturate_default_is_identity() {
    // values="" → s=1 (identity per spec §13.2.4).
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f"><feColorMatrix type="saturate"/></filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::ColorMatrix { matrix, .. } = &g.primitives[0].primitive else {
        panic!("not ColorMatrix");
    };
    // Diagonal (R, G, B) close to 1 with s=1.
    assert!((matrix[0] - 1.0).abs() < 1e-3);
    assert!((matrix[6] - 1.0).abs() < 1e-3);
    assert!((matrix[12] - 1.0).abs() < 1e-3);
}

#[test]
fn color_matrix_huerotate_180_swaps_color_axes() {
    // hue-rotate by 180 degrees should give a non-identity matrix
    // whose R-row is no longer (1,0,0) — easiest to assert.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f"><feColorMatrix type="hueRotate" values="180"/></filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::ColorMatrix { matrix, .. } = &g.primitives[0].primitive else {
        panic!("not ColorMatrix");
    };
    // R coefficient must have moved away from 1.
    assert!((matrix[0] - 1.0).abs() > 0.1, "hue-rotate 180 left R as 1");
}

#[test]
fn color_matrix_luminance_to_alpha_zeroes_color_rows() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f"><feColorMatrix type="luminanceToAlpha"/></filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::ColorMatrix { matrix, .. } = &g.primitives[0].primitive else {
        panic!("not ColorMatrix");
    };
    // Rows R, G, B all zero.
    for (i, v) in matrix.iter().take(15).enumerate() {
        assert_eq!(*v, 0.0, "row/col {} expected 0", i);
    }
    // A row weights luminance per spec.
    assert!((matrix[15] - 0.2125).abs() < 1e-4);
    assert!((matrix[16] - 0.7154).abs() < 1e-4);
    assert!((matrix[17] - 0.0721).abs() < 1e-4);
}

#[test]
fn merge_records_each_merge_node_input_in_source_order() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feGaussianBlur in="SourceAlpha" stdDeviation="3" result="blur"/>
          <feOffset in="blur" dx="4" dy="4" result="offsetBlur"/>
          <feMerge>
            <feMergeNode in="offsetBlur"/>
            <feMergeNode in="SourceGraphic"/>
          </feMerge>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 3);
    let FilterPrimitive::Merge { inputs } = &g.primitives[2].primitive else {
        panic!("not Merge");
    };
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0], FilterInput::Reference("offsetBlur".into()));
    assert_eq!(inputs[1], FilterInput::SourceGraphic);
}

#[test]
fn merge_with_no_merge_nodes_is_an_empty_input_list() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feMerge/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::Merge { inputs } = &g.primitives[0].primitive else {
        panic!("not Merge");
    };
    assert!(inputs.is_empty());
}

#[test]
fn component_transfer_routes_each_func_to_its_channel() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feComponentTransfer>
            <feFuncR type="linear" slope="2" intercept="0"/>
            <feFuncG type="gamma" amplitude="1" exponent="2.2" offset="0"/>
            <feFuncB type="discrete" tableValues="0 0.5 1"/>
            <feFuncA type="table" tableValues="0 1"/>
          </feComponentTransfer>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::ComponentTransfer {
        red,
        green,
        blue,
        alpha,
        ..
    } = &g.primitives[0].primitive
    else {
        panic!("not ComponentTransfer");
    };
    assert!(matches!(red, TransferFunction::Linear { .. }));
    assert!(matches!(green, TransferFunction::Gamma { .. }));
    match blue {
        TransferFunction::Discrete { values } => {
            assert_eq!(values, &vec![0.0, 0.5, 1.0])
        }
        _ => panic!("blue not discrete"),
    }
    match alpha {
        TransferFunction::Table { values } => assert_eq!(values, &vec![0.0, 1.0]),
        _ => panic!("alpha not table"),
    }
}

#[test]
fn component_transfer_default_input_is_source_graphic() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feComponentTransfer>
            <feFuncR type="identity"/>
          </feComponentTransfer>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::ComponentTransfer { input, .. } = &g.primitives[0].primitive else {
        panic!("not ComponentTransfer");
    };
    assert_eq!(*input, FilterInput::SourceGraphic);
}

#[test]
fn drop_shadow_records_dx_dy_blur_color_and_opacity() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
        <defs><filter id="f">
          <feDropShadow dx="3" dy="4" stdDeviation="2 2" flood-color="#0080ff" flood-opacity="0.75"/>
        </filter></defs>
        <rect width="20" height="20" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::DropShadow {
        dx,
        dy,
        std_deviation_x,
        std_deviation_y,
        flood_color,
        flood_opacity,
        ..
    } = &g.primitives[0].primitive
    else {
        panic!("not DropShadow");
    };
    assert_eq!(*dx, 3.0);
    assert_eq!(*dy, 4.0);
    assert_eq!(*std_deviation_x, 2.0);
    assert_eq!(*std_deviation_y, 2.0);
    assert_eq!(flood_color.r, 0x00);
    assert_eq!(flood_color.g, 0x80);
    assert_eq!(flood_color.b, 0xff);
    assert!((*flood_opacity - 0.75).abs() < 1e-6);
}

#[test]
fn drop_shadow_no_attrs_uses_spec_defaults() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
        <defs><filter id="f"><feDropShadow/></filter></defs>
        <rect width="20" height="20" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::DropShadow {
        dx,
        dy,
        std_deviation_x,
        std_deviation_y,
        flood_color,
        flood_opacity,
        ..
    } = &g.primitives[0].primitive
    else {
        panic!("not DropShadow");
    };
    assert_eq!(*dx, 2.0);
    assert_eq!(*dy, 2.0);
    assert_eq!(*std_deviation_x, 2.0);
    assert_eq!(*std_deviation_y, 2.0);
    assert_eq!(flood_color, &FloodColor::default());
    assert_eq!(*flood_opacity, 1.0);
}

#[test]
fn round_trip_preserves_round8_primitives_verbatim() {
    // The typed graph should now recognise these — but the verbatim
    // round-trip path should still emit them character-for-character
    // (modulo whitespace), so existing renderers don't regress.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feColorMatrix type="hueRotate" values="90"/>
          <feMerge>
            <feMergeNode in="SourceGraphic"/>
          </feMerge>
          <feComponentTransfer>
            <feFuncR type="linear" slope="1.5" intercept="0"/>
          </feComponentTransfer>
          <feDropShadow dx="2" dy="2" stdDeviation="1"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("feColorMatrix"));
    assert!(s.contains("feMerge"));
    assert!(s.contains("feMergeNode"));
    assert!(s.contains("feComponentTransfer"));
    assert!(s.contains("feFuncR"));
    assert!(s.contains("feDropShadow"));
}

#[test]
fn mixed_pipeline_round8_with_round7_primitives() {
    // Realistic recipe — drop-shadow expressed as a chain (the
    // syntactic-sugar form), then a feColorMatrix tint applied to
    // the merged result. Verifies primitive ordering + result chain.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
        <defs><filter id="f">
          <feGaussianBlur in="SourceAlpha" stdDeviation="3" result="blur"/>
          <feOffset in="blur" dx="4" dy="4" result="off"/>
          <feFlood flood-color="#000000" flood-opacity="0.5" result="bg"/>
          <feComposite in="bg" in2="off" operator="in" result="tinted"/>
          <feMerge>
            <feMergeNode in="tinted"/>
            <feMergeNode in="SourceGraphic"/>
          </feMerge>
        </filter></defs>
        <rect width="50" height="50" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 5);
    let FilterPrimitive::Composite { operator, .. } = &g.primitives[3].primitive else {
        panic!("expected composite at slot 3");
    };
    assert_eq!(*operator, CompositeOperator::In);
    let FilterPrimitive::Merge { inputs } = &g.primitives[4].primitive else {
        panic!("expected merge at slot 4");
    };
    assert_eq!(inputs.len(), 2);
}

#[test]
fn drop_shadow_stays_inside_filter_chain_threading() {
    // feDropShadow as the second primitive — its `in` should default
    // to the previous primitive's result (per Filter Effects §6.2),
    // not to SourceGraphic.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feGaussianBlur stdDeviation="1" result="b"/>
          <feDropShadow dx="2" dy="2" stdDeviation="2"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    let FilterPrimitive::DropShadow { input, .. } = &g.primitives[1].primitive else {
        panic!("expected drop-shadow");
    };
    assert_eq!(*input, FilterInput::Reference("b".into()));
}

#[test]
fn blend_inside_round8_pipeline_still_parses() {
    // Sanity — the round-7 primitives still resolve correctly when
    // mixed with round-8 ones.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <defs><filter id="f">
          <feColorMatrix type="saturate" values="0" result="gray"/>
          <feBlend in="gray" in2="SourceGraphic" mode="multiply"/>
        </filter></defs>
        <rect width="10" height="10" filter="url(#f)"/>
      </svg>"##;
    let g = graph_for_filter(src, "f");
    assert_eq!(g.primitives.len(), 2);
    let FilterPrimitive::Blend { mode, input, .. } = &g.primitives[1].primitive else {
        panic!("expected Blend");
    };
    assert_eq!(*mode, BlendMode::Multiply);
    assert_eq!(*input, FilterInput::Reference("gray".into()));
}
