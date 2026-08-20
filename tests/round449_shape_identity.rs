//! Round 449 — native shape identity round-trip (SVG 2 §9.2–§9.7).
//!
//! The decoder flattens every basic shape into path commands, so the
//! encoder used to emit `<path d="…">` — geometrically identical but
//! losing the element identity, which broke every consumer addressing
//! the shape *as* a shape: an inlined `<animate attributeName="x">`
//! re-attached to a `<path>` targets an attribute the element doesn't
//! have, and `rect { … }` type selectors stop matching after a
//! round-trip. `PreservedExtras::shapes` now records the source tag +
//! verbatim geometry attributes keyed by the inner geometry node's
//! scene-graph tree-path; the encoder emits the native tag instead of
//! the flattened `d`.

use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg, write_svg_with_extras};

fn roundtrip(src: &[u8]) -> String {
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    let out = write_svg_with_extras(&frame, &extras);
    String::from_utf8(out).expect("utf8")
}

/// `<rect>` round-trips as a native `<rect>` with its geometry
/// attributes verbatim.
#[test]
fn rect_keeps_native_identity() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <rect x="1" y="2" width="30" height="40" rx="3" ry="4" fill="blue"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains(r#"<rect x="1" y="2" width="30" height="40" rx="3" ry="4""#),
        "native rect with verbatim geometry:\n{out}"
    );
    assert!(!out.contains("<path"), "no flattened path remains:\n{out}");
}

/// Each of the six basic shapes keeps its native tag.
#[test]
fn all_basic_shapes_keep_identity() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <rect x="1" y="1" width="5" height="5"/>
  <circle cx="10" cy="10" r="4"/>
  <ellipse cx="20" cy="20" rx="6" ry="3"/>
  <line x1="0" y1="0" x2="9" y2="9" stroke="black"/>
  <polyline points="0,0 5,5 10,0" fill="none" stroke="black"/>
  <polygon points="0,0 8,0 4,8"/>
</svg>"##;
    let out = roundtrip(src);
    for needle in [
        r#"<rect x="1" y="1" width="5" height="5""#,
        r#"<circle cx="10" cy="10" r="4""#,
        r#"<ellipse cx="20" cy="20" rx="6" ry="3""#,
        r#"<line x1="0" y1="0" x2="9" y2="9""#,
        r#"<polyline points="0,0 5,5 10,0""#,
        r#"<polygon points="0,0 8,0 4,8""#,
    ] {
        assert!(out.contains(needle), "{needle} survives:\n{out}");
    }
}

/// Percentage / unit-bearing geometry survives verbatim — the viewport
/// attributes round-trip alongside, so a re-parse resolves to the same
/// user-space geometry.
#[test]
fn percentage_geometry_survives_verbatim() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <rect x="10%" y="0" width="50%" height="100%"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains(r#"<rect x="10%" y="0" width="50%" height="100%""#),
        "percentages survive verbatim:\n{out}"
    );
}

/// A transformed shape keeps its native identity inside the transform
/// wrapper group.
#[test]
fn transformed_shape_keeps_identity() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <circle cx="5" cy="5" r="3" transform="translate(10 10)"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains(r#"<circle cx="5" cy="5" r="3""#),
        "the circle survives inside its wrapper:\n{out}"
    );
    assert!(out.contains("transform="), "the transform survives:\n{out}");
}

/// A masked shape keeps its native identity inside the emitted
/// `<g mask="url(#…)">` wrapper.
#[test]
fn masked_shape_keeps_identity() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs><mask id="m"><rect width="100" height="100" fill="white"/></mask></defs>
  <circle cx="5" cy="5" r="3" mask="url(#m)"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains(r#"<circle cx="5" cy="5" r="3""#),
        "the masked circle survives natively:\n{out}"
    );
    assert!(
        out.contains("mask=\"url(#"),
        "the mask reference survives:\n{out}"
    );
}

/// The re-parsed output resolves to the same scene geometry as the
/// source (semantic fixed point), and the write is byte-idempotent.
#[test]
fn shape_roundtrip_is_idempotent_and_geometry_stable() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <rect x="1" y="2" width="30" height="40"/>
  <circle cx="10" cy="10" r="4" fill="red"/>
</svg>"##;
    let w1 = roundtrip(src);
    let w2 = roundtrip(w1.as_bytes());
    assert_eq!(w1, w2, "write(parse(write(x))) == write(x)");
    // Semantic check: flatten both the source and the round-tripped
    // output through the plain (extras-free) writer — the flattened
    // geometry must be identical, proving the verbatim geometry
    // attributes resolve to the same user-space shapes.
    let flat_src = String::from_utf8(write_svg(&parse_svg(src).unwrap())).unwrap();
    let flat_rt = String::from_utf8(write_svg(&parse_svg(w1.as_bytes()).unwrap())).unwrap();
    assert_eq!(
        flat_src, flat_rt,
        "re-parsed native shapes flatten to identical geometry"
    );
}

/// The §13.8 stroke-first `paint-order` split produces two
/// single-purpose geometry nodes for one source shape — an ambiguous
/// emit site, so the native-identity carrier declines and the
/// flattened-path emission (with the `paint-order` carrier) is kept.
#[test]
fn paint_order_split_declines_native_identity() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <rect x="1" y="2" width="30" height="40" fill="blue" stroke="black" stroke-width="2" paint-order="stroke"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        !out.contains("<rect"),
        "the split shape declines native identity:\n{out}"
    );
    assert!(out.contains("<path"), "the flattened paths remain:\n{out}");
    let w2 = roundtrip(out.as_bytes());
    assert_eq!(out, w2, "the declined form is still idempotent");
}

/// The extras-free `write_svg` path is unchanged — without bindings the
/// flattened `<path>` emission remains.
#[test]
fn plain_write_svg_still_flattens() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <rect x="1" y="2" width="30" height="40"/>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let out = String::from_utf8(write_svg(&frame)).expect("utf8");
    assert!(out.contains("<path"), "extras-free write flattens:\n{out}");
    assert!(!out.contains("<rect"), "no binding, no native tag:\n{out}");
}

/// A gradient-filled shape keeps both its native identity and the
/// paint-server reference.
#[test]
fn gradient_filled_shape_keeps_identity() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs><linearGradient id="lg"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient></defs>
  <ellipse cx="20" cy="20" rx="6" ry="3" fill="url(#lg)"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains(r#"<ellipse cx="20" cy="20" rx="6" ry="3""#),
        "the ellipse survives natively:\n{out}"
    );
    assert!(
        out.contains("fill=\"url(#"),
        "the gradient reference survives:\n{out}"
    );
}

/// `pathLength` (SVG 2 §9.6.1 — valid on every basic shape) rides the
/// native tag.
#[test]
fn path_length_rides_native_tag() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <circle cx="10" cy="10" r="4" pathLength="100" stroke="black" stroke-dasharray="10 10" fill="none"/>
</svg>"##;
    let out = roundtrip(src);
    let p_open = out.find("<circle").expect("native circle");
    let p_pl = out.find("pathLength=").expect("pathLength carried");
    let p_end = out[p_open..].find('>').map(|i| p_open + i).unwrap();
    assert!(
        p_open < p_pl && p_pl < p_end,
        "pathLength rides on the circle tag:\n{out}"
    );
}
