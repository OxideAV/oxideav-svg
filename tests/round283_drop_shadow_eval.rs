//! Round 283 — pixel-level `<feDropShadow>` evaluation per the W3C
//! Filter Effects Module Level 1 §9.12 normative equivalent composite
//! (alpha → §9.14 Gaussian blur → §9.18 offset → §9.13 flood
//! composited `in` per §9.8 → §9.16 merge `over` with the input on
//! top), with the working colour space resolved from
//! `color-interpolation-filters` per §10 (initial `linearRGB`; the
//! sRGB ↔ linear transfer is the SVG 2 §13.9 formula).
//!
//! Every test pins rendered output bytes (8-bit non-premultiplied
//! sRGB-encoded RGBA), hand-derived from the spec maths in comments.

use oxideav_svg::filter::{ColorInterpolationFilters, FilterPrimitiveNode};
use oxideav_svg::filter_eval::{
    drop_shadow, evaluate_drop_shadow_node, DropShadowParams, FilterImage,
};
use oxideav_svg::parse_svg_with_extras;

/// Parse `src` and return the single primitive node of `<filter id=>`.
fn node_for_filter(src: &[u8], filter_id: &str) -> FilterPrimitiveNode {
    let (_frame, extras) = parse_svg_with_extras(src).expect("parse_svg_with_extras");
    for el in &extras.filters {
        if el
            .attrs
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("id") && v == filter_id)
        {
            let g = oxideav_svg::filter::parse_filter_graph(el);
            assert_eq!(g.primitives.len(), 1, "expected exactly one primitive");
            return g.primitives.into_iter().next().unwrap();
        }
    }
    panic!("no <filter id=\"{filter_id}\"> in extras");
}

/// A `w × h` transparent RGBA8 canvas with one pixel set.
fn canvas_with_pixel(w: usize, h: usize, x: usize, y: usize, rgba: [u8; 4]) -> Vec<u8> {
    let mut buf = vec![0u8; w * h * 4];
    buf[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&rgba);
    buf
}

fn px(buf: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
    let i = (y * w + x) * 4;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

// ---- end-to-end: parsed node → evaluated pixels ----

// stdDeviation=0 disables the blur (§9.14), so the shadow is an exact
// offset copy of the input alpha tinted with the default opaque-black
// flood (§9.13): source pixel stays put, shadow lands at (+2, +1).
#[test]
fn node_eval_no_blur_integer_offset_pins() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="5" height="5">
        <defs><filter id="f" color-interpolation-filters="sRGB">
          <feDropShadow dx="2" dy="1" stdDeviation="0"/>
        </filter></defs>
        <rect width="5" height="5" filter="url(#f)"/>
      </svg>"##;
    let node = node_for_filter(src, "f");
    let source = canvas_with_pixel(5, 5, 1, 1, [255, 0, 0, 255]);
    let out = evaluate_drop_shadow_node(&node, &source, 5, 5).expect("drop shadow node");
    // Source on top, untouched.
    assert_eq!(px(&out, 5, 1, 1), [255, 0, 0, 255]);
    // Shadow: opaque black at (1+2, 1+1).
    assert_eq!(px(&out, 5, 3, 2), [0, 0, 0, 255]);
    // Everything else transparent black.
    for y in 0..5 {
        for x in 0..5 {
            if (x, y) != (1, 1) && (x, y) != (3, 2) {
                assert_eq!(px(&out, 5, x, y), [0, 0, 0, 0], "({x},{y})");
            }
        }
    }
}

// §9.14 even-d impulse pins: s=0.8 → d = floor(0.8·3·sqrt(2π)/4 + 0.5)
// = 2, so the three boxes are size 2 (left-boundary-centred), size 2
// (right-boundary-centred) and size 3 (centred). The resulting 1-D
// impulse response is k = [1/12, 1/4, 1/3, 1/4, 1/12]; the 2-D shadow
// alpha is the separable product k(x)·k(y).
#[test]
fn node_eval_blurred_shadow_alpha_pins() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9">
        <defs><filter id="f" color-interpolation-filters="sRGB">
          <feDropShadow dx="0" dy="0" stdDeviation="0.8"/>
        </filter></defs>
        <rect width="9" height="9" filter="url(#f)"/>
      </svg>"##;
    let node = node_for_filter(src, "f");
    let source = canvas_with_pixel(9, 9, 4, 4, [255, 255, 255, 255]);
    let out = evaluate_drop_shadow_node(&node, &source, 9, 9).expect("drop shadow node");
    // Opaque source on top hides the shadow centre.
    assert_eq!(px(&out, 9, 4, 4), [255, 255, 255, 255]);
    // k(2)·k(0) = (1/12)(1/3) = 1/36 → round(255/36) = 7.
    assert_eq!(px(&out, 9, 6, 4), [0, 0, 0, 7]);
    assert_eq!(px(&out, 9, 4, 6), [0, 0, 0, 7]);
    // k(1)·k(1) = 1/16 → round(255/16) = 16.
    assert_eq!(px(&out, 9, 5, 5), [0, 0, 0, 16]);
    // k(2)·k(2) = 1/144 → round(255/144) = 2.
    assert_eq!(px(&out, 9, 2, 2), [0, 0, 0, 2]);
    // Outside the kernel support (radius 2): empty.
    assert_eq!(px(&out, 9, 7, 4), [0, 0, 0, 0]);
}

// §9.13.2: the flood colour's alpha is multiplied with flood-opacity —
// a red shadow at half opacity unpremultiplies back to pure red with
// alpha 128.
#[test]
fn node_eval_flood_color_and_opacity_pins() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4">
        <defs><filter id="f" color-interpolation-filters="sRGB">
          <feDropShadow dx="1" dy="0" stdDeviation="0"
                        flood-color="#ff0000" flood-opacity="0.5"/>
        </filter></defs>
        <rect width="4" height="4" filter="url(#f)"/>
      </svg>"##;
    let node = node_for_filter(src, "f");
    let source = canvas_with_pixel(4, 4, 0, 0, [0, 0, 255, 255]);
    let out = evaluate_drop_shadow_node(&node, &source, 4, 4).expect("drop shadow node");
    assert_eq!(px(&out, 4, 0, 0), [0, 0, 255, 255]);
    // Shadow: premultiplied (0.5, 0, 0, 0.5) → (255, 0, 0, 128).
    assert_eq!(px(&out, 4, 1, 0), [255, 0, 0, 128]);
}

// §9.16 merge with a semi-transparent input over its own shadow
// (dx=dy=0, no blur), in the sRGB working space. With a = 128/255:
//   out_a   = a + a·(1−a)            ≈ 0.751957 → 192
//   out_rgb = a·white / out_a        ≈ 0.667566 → 170
#[test]
fn node_eval_merge_over_semitransparent_srgb_pins() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="3" height="3">
        <defs><filter id="f" color-interpolation-filters="sRGB">
          <feDropShadow dx="0" dy="0" stdDeviation="0"/>
        </filter></defs>
        <rect width="3" height="3" filter="url(#f)"/>
      </svg>"##;
    let node = node_for_filter(src, "f");
    let source = canvas_with_pixel(3, 3, 1, 1, [255, 255, 255, 128]);
    let out = evaluate_drop_shadow_node(&node, &source, 3, 3).expect("drop shadow node");
    assert_eq!(px(&out, 3, 1, 1), [170, 170, 170, 192]);
}

// Same composite in the linearRGB working space (the §10 initial
// value, here resolved from an attribute-free <filter>): white is 1.0
// linear, so the premultiplied maths matches the sRGB case, but the
// unpremultiplied 0.667566 is *linear* and re-encodes per SVG 2 §13.9:
//   1.055·0.667566^(1/2.4) − 0.055 ≈ 0.836509 → 213.
#[test]
fn node_eval_merge_over_semitransparent_linear_pins() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="3" height="3">
        <defs><filter id="f">
          <feDropShadow dx="0" dy="0" stdDeviation="0"/>
        </filter></defs>
        <rect width="3" height="3" filter="url(#f)"/>
      </svg>"##;
    let node = node_for_filter(src, "f");
    assert_eq!(
        node.color_interpolation_filters,
        ColorInterpolationFilters::LinearRgb
    );
    let source = canvas_with_pixel(3, 3, 1, 1, [255, 255, 255, 128]);
    let out = evaluate_drop_shadow_node(&node, &source, 3, 3).expect("drop shadow node");
    assert_eq!(px(&out, 3, 1, 1), [213, 213, 213, 192]);
}

// Bare <feDropShadow/> defaults (§9.12: dx=2, dy=2, stdDeviation=2).
// s=2 → d=4 (even): boxes 4+4+5. The 1-D impulse response after the
// two boundary-centred size-4 boxes is the triangle
// [1,2,3,4,3,2,1]/16; the final size-5 box gives a centre weight of
// (2+3+4+3+2)/80 = 0.175. 2-D centre = 0.175² = 0.030625 → round(255·)
// = 8, landed at source+(2,2). Black flood ⇒ the pin is identical in
// either working space.
#[test]
fn node_eval_default_attributes_pins() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="21" height="21">
        <defs><filter id="f"><feDropShadow/></filter></defs>
        <rect width="21" height="21" filter="url(#f)"/>
      </svg>"##;
    let node = node_for_filter(src, "f");
    let source = canvas_with_pixel(21, 21, 8, 8, [0, 0, 0, 255]);
    let out = evaluate_drop_shadow_node(&node, &source, 21, 21).expect("drop shadow node");
    // Source on top.
    assert_eq!(px(&out, 21, 8, 8), [0, 0, 0, 255]);
    // Shadow centre at (8+2, 8+2).
    assert_eq!(px(&out, 21, 10, 10), [0, 0, 0, 8]);
    // The §9.14 even-d box pairing keeps the kernel symmetric about
    // the shadow centre.
    for k in 1..=5usize {
        assert_eq!(
            px(&out, 21, 10 - k, 10),
            px(&out, 21, 10 + k, 10),
            "k={k} horizontal"
        );
        assert_eq!(
            px(&out, 21, 10, 10 - k),
            px(&out, 21, 10, 10 + k),
            "k={k} vertical"
        );
    }
    // Beyond the radius-5 kernel support: empty.
    assert_eq!(px(&out, 21, 16, 10), [0, 0, 0, 0]);
}

// §9.18 fractional offset through the full primitive: dx=0.5 splits
// the unblurred shadow alpha 50/50 between two columns
// (round(0.5·255) = 128).
#[test]
fn node_eval_fractional_offset_pins() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="1">
        <defs><filter id="f" color-interpolation-filters="sRGB">
          <feDropShadow dx="0.5" dy="0" stdDeviation="0"/>
        </filter></defs>
        <rect width="4" height="1" filter="url(#f)"/>
      </svg>"##;
    let node = node_for_filter(src, "f");
    let source = canvas_with_pixel(4, 1, 1, 0, [255, 0, 0, 255]);
    let out = evaluate_drop_shadow_node(&node, &source, 4, 1).expect("drop shadow node");
    // The opaque source pixel sits on top of half of its own shadow
    // (§9.16 puts the input on the topmost merge node), so (1,0) is
    // pure red.
    assert_eq!(px(&out, 4, 1, 0), [255, 0, 0, 255]);
    assert_eq!(px(&out, 4, 2, 0), [0, 0, 0, 128]);
    assert_eq!(px(&out, 4, 0, 0), [0, 0, 0, 0]);
    assert_eq!(px(&out, 4, 3, 0), [0, 0, 0, 0]);
}

// ---- direct buffer-level API ----

// drop_shadow() on FilterImage buffers, sRGB space: same pins as the
// node-level no-blur test, exercising the direct path the §9.12 steps
// are implemented on.
#[test]
fn direct_drop_shadow_pins() {
    let source = canvas_with_pixel(5, 5, 1, 1, [255, 0, 0, 255]);
    let img = FilterImage::from_rgba8(5, 5, &source, ColorInterpolationFilters::Srgb)
        .expect("from_rgba8");
    let out = drop_shadow(
        &img,
        &DropShadowParams {
            dx: 2.0,
            dy: 1.0,
            std_deviation_x: 0.0,
            std_deviation_y: 0.0,
            ..DropShadowParams::default()
        },
        ColorInterpolationFilters::Srgb,
    );
    let bytes = out.to_rgba8(ColorInterpolationFilters::Srgb);
    assert_eq!(px(&bytes, 5, 1, 1), [255, 0, 0, 255]);
    assert_eq!(px(&bytes, 5, 3, 2), [0, 0, 0, 255]);
}

// §9.14: a negative stdDeviation disables the blur primitive (its
// result is its input), so the shadow is the unblurred offset alpha.
#[test]
fn direct_negative_std_deviation_disables_blur() {
    let source = canvas_with_pixel(5, 5, 1, 1, [255, 0, 0, 255]);
    let img = FilterImage::from_rgba8(5, 5, &source, ColorInterpolationFilters::Srgb)
        .expect("from_rgba8");
    let out = drop_shadow(
        &img,
        &DropShadowParams {
            dx: 2.0,
            dy: 1.0,
            std_deviation_x: -1.0,
            std_deviation_y: 2.0,
            ..DropShadowParams::default()
        },
        ColorInterpolationFilters::Srgb,
    );
    let bytes = out.to_rgba8(ColorInterpolationFilters::Srgb);
    assert_eq!(px(&bytes, 5, 3, 2), [0, 0, 0, 255]);
    assert_eq!(px(&bytes, 5, 3, 3), [0, 0, 0, 0]);
}

// ---- guard rails ----

#[test]
fn node_eval_rejects_non_drop_shadow_and_bad_lengths() {
    let blur = br##"<svg xmlns="http://www.w3.org/2000/svg" width="3" height="3">
        <defs><filter id="f"><feGaussianBlur stdDeviation="1"/></filter></defs>
        <rect width="3" height="3" filter="url(#f)"/>
      </svg>"##;
    let node = node_for_filter(blur, "f");
    assert_eq!(evaluate_drop_shadow_node(&node, &[0; 36], 3, 3), None);

    let shadow = br##"<svg xmlns="http://www.w3.org/2000/svg" width="3" height="3">
        <defs><filter id="f"><feDropShadow/></filter></defs>
        <rect width="3" height="3" filter="url(#f)"/>
      </svg>"##;
    let node = node_for_filter(shadow, "f");
    // 3×3 needs 36 bytes; 35 is rejected.
    assert_eq!(evaluate_drop_shadow_node(&node, &[0; 35], 3, 3), None);
}
