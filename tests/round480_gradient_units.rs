//! Round 480 — SVG 2 §8.11 / §14.2 `gradientUnits` binding (issue #13).
//!
//! `gradientUnits` defaults to `objectBoundingBox`: `x1` / `y1` / `x2`
//! / `y2` and `cx` / `cy` / `r` / `fx` / `fy` are fractions of the
//! referencing element's object bounding box (bare numbers AND
//! percentages), `gradientTransform` applies in that box space, and the
//! scene graph carries the result in the shape's own user space so a
//! rasteriser that knows nothing about SVG paints the right thing.
//!
//! Exact-structure checks on the scene graph; the pixel-level
//! black-box comparison against `rsvg-convert` lives in
//! `round480_gradient_pixels.rs`.

use oxideav_svg::bbox::BBox;
use oxideav_svg::defs::{
    DefsTables, GradientDef, GradientKind, GradientPaint, GradientUnits, ResolvedGradientKind,
    bind_gradient, resolve_gradient_chain,
};
use oxideav_svg::length::Length;
use oxideav_svg::{
    GradientStop, Node, Paint, PathNode, Point, Rgba, SpreadMethod, Transform2D, parse,
    parse_with_extras, write, write_with_extras,
};

fn paths(frame: &oxideav_svg::SvgDocument) -> Vec<&PathNode> {
    fn walk<'a>(n: &'a Node, out: &mut Vec<&'a PathNode>) {
        match n {
            Node::Path(p) => out.push(p),
            Node::Group(g) => g.children.iter().for_each(|c| walk(c, out)),
            Node::SoftMask { content, .. } => walk(content, out),
            _ => {}
        }
    }
    let mut out = Vec::new();
    frame.root.children.iter().for_each(|c| walk(c, &mut out));
    out
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

fn linear(p: &PathNode) -> &oxideav_svg::LinearGradient {
    match &p.fill {
        Some(Paint::LinearGradient(g)) => g,
        other => panic!("expected LinearGradient fill, got {other:?}"),
    }
}

fn radial(p: &PathNode) -> &oxideav_svg::RadialGradient {
    match &p.fill {
        Some(Paint::RadialGradient(g)) => g,
        other => panic!("expected RadialGradient fill, got {other:?}"),
    }
}

const STOPS: &str =
    r##"<stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/>"##;

fn doc(defs: &str, body: &str) -> Vec<u8> {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100"><defs>{defs}</defs>{body}</svg>"##
    )
    .into_bytes()
}

#[test]
fn default_units_span_the_shapes_bounding_box() {
    // §14.2.2.1 initial values x1=0% y1=0% x2=100% y2=0% under the
    // default objectBoundingBox: the gradient line runs along the top
    // edge of the shape's box, from its left edge to its right edge.
    let src = doc(
        &format!(r##"<linearGradient id="g">{STOPS}</linearGradient>"##),
        r##"<rect x="20" y="10" width="160" height="80" fill="url(#g)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let g = linear(paths(&frame)[0]);
    assert_eq!(g.start, Point::new(20.0, 10.0));
    assert_eq!(g.end, Point::new(180.0, 10.0));
    assert_eq!(g.stops.len(), 2);
}

#[test]
fn bare_numbers_and_percentages_are_both_box_fractions() {
    // §8.11: "a percentage represents the same value as the
    // corresponding decimal value (e.g., 50% means the same as 0.5)".
    let src = doc(
        &format!(
            r##"<linearGradient id="a" x1="0.25" x2="0.75" y1="0.5" y2="0.5">{STOPS}</linearGradient>
                <linearGradient id="b" x1="25%" x2="75%" y1="50%" y2="50%">{STOPS}</linearGradient>"##
        ),
        r##"<rect x="20" y="10" width="160" height="80" fill="url(#a)"/>
            <rect x="20" y="10" width="160" height="80" fill="url(#b)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let ps = paths(&frame);
    let a = linear(ps[0]);
    let b = linear(ps[1]);
    assert_eq!(a.start, Point::new(60.0, 50.0));
    assert_eq!(a.end, Point::new(140.0, 50.0));
    assert_eq!(b.start, a.start);
    assert_eq!(b.end, a.end);
}

#[test]
fn user_space_percentages_resolve_against_the_viewport() {
    // §14.2.2.1 userSpaceOnUse: "Percentages represent values relative
    // to the current SVG viewport" — x against the width (200), y
    // against the height (100).
    let src = doc(
        &format!(
            r##"<linearGradient id="g" gradientUnits="userSpaceOnUse" x1="10%" y1="20%" x2="50%" y2="20%">{STOPS}</linearGradient>"##
        ),
        r##"<rect x="20" y="10" width="160" height="80" fill="url(#g)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let g = linear(paths(&frame)[0]);
    assert_eq!(g.start, Point::new(20.0, 20.0));
    assert_eq!(g.end, Point::new(100.0, 20.0));
}

#[test]
fn same_gradient_binds_differently_to_each_shape() {
    let src = doc(
        &format!(r##"<linearGradient id="g">{STOPS}</linearGradient>"##),
        r##"<rect x="0" y="0" width="50" height="50" fill="url(#g)"/>
            <circle cx="150" cy="50" r="40" fill="url(#g)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let ps = paths(&frame);
    let a = linear(ps[0]);
    let b = linear(ps[1]);
    assert_eq!(a.start, Point::new(0.0, 0.0));
    assert_eq!(a.end, Point::new(50.0, 0.0));
    // The circle's object bounding box is (110, 10, 80, 80) — two arc
    // commands, so the box must come from the arc extrema rather than
    // the endpoints.
    assert!(close(b.start.x, 110.0) && close(b.start.y, 10.0), "{b:?}");
    assert!(close(b.end.x, 190.0) && close(b.end.y, 10.0), "{b:?}");
}

#[test]
fn diagonal_vector_on_non_square_box_keeps_the_iso_lines() {
    // §14.2.2.1: "When the object's bounding box is not square, the
    // gradient normal which is initially perpendicular to the gradient
    // vector within object bounding box space may render
    // non-perpendicular relative to the gradient vector in user
    // space." The scene-graph line is the perpendicular-normal
    // equivalent of that field: t(x, y) = x/400 + y/200 on a 200×100
    // box, i.e. (0,0) → (80,160).
    let src = doc(
        &format!(r##"<linearGradient id="g" x2="1" y2="1">{STOPS}</linearGradient>"##),
        r##"<rect x="0" y="0" width="200" height="100" fill="url(#g)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let g = linear(paths(&frame)[0]);
    assert!(close(g.start.x, 0.0) && close(g.start.y, 0.0), "{g:?}");
    assert!(close(g.end.x, 80.0) && close(g.end.y, 160.0), "{g:?}");
    // The field evaluates to 0 at the box's top-left and 1 at its
    // bottom-right, and the end-start direction is along the gradient
    // of the field (perpendicular iso-lines).
    let t = |p: Point| {
        let v = Point::new(g.end.x - g.start.x, g.end.y - g.start.y);
        ((p.x - g.start.x) * v.x + (p.y - g.start.y) * v.y) / (v.x * v.x + v.y * v.y)
    };
    assert!(close(t(Point::new(0.0, 0.0)), 0.0));
    assert!(close(t(Point::new(200.0, 100.0)), 1.0));
    assert!(close(t(Point::new(200.0, 0.0)), 0.5));
    assert!(close(t(Point::new(0.0, 100.0)), 0.5));
}

#[test]
fn gradient_transform_applies_in_box_space() {
    // rotate(90) in box space maps the (0,0)→(1,0) vector onto
    // (0,0)→(0,1), i.e. a top-to-bottom gradient in user space.
    let src = doc(
        &format!(
            r##"<linearGradient id="g" gradientTransform="rotate(90)">{STOPS}</linearGradient>"##
        ),
        r##"<rect x="20" y="10" width="160" height="80" fill="url(#g)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let g = linear(paths(&frame)[0]);
    // Iso-lines are horizontal: offset 0 along y = 10, offset 1 along
    // y = 90 (the x position along an iso-line is immaterial).
    assert!(close(g.start.y, 10.0) && close(g.end.y, 90.0), "{g:?}");
    assert!(close(g.start.x, g.end.x), "{g:?}");
}

#[test]
fn radial_on_square_box_is_a_circle_in_user_space() {
    let src = doc(
        &format!(r##"<radialGradient id="g" fx="0.25" fy="0.5">{STOPS}</radialGradient>"##),
        r##"<circle cx="50" cy="50" r="20" fill="url(#g)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let g = radial(paths(&frame)[0]);
    assert!(close(g.center.x, 50.0) && close(g.center.y, 50.0), "{g:?}");
    assert!(close(g.radius, 20.0), "{g:?}");
    let f = g.focal.expect("focal differs from centre");
    assert!(close(f.x, 40.0) && close(f.y, 50.0), "{f:?}");
}

#[test]
fn radial_on_non_square_box_paints_through_a_transformed_mask() {
    // §14.2.3.1: "When the object's bounding box is not square, the
    // rings that are conceptually circular within object bounding box
    // space will render as elliptical". The scene graph has no
    // elliptical radial gradient, so the shape becomes a soft mask
    // whose content is the circular gradient under the box matrix.
    let src = doc(
        &format!(r##"<radialGradient id="g">{STOPS}</radialGradient>"##),
        r##"<rect x="0" y="0" width="200" height="100" fill="url(#g)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let layer = match &frame.root.children[0] {
        Node::Group(g) => {
            assert!(g.transform.is_identity());
            assert_eq!(g.children.len(), 1);
            &g.children[0]
        }
        other => panic!("expected layer group, got {other:?}"),
    };
    match layer {
        Node::SoftMask {
            mask,
            mask_kind,
            content,
        } => {
            assert_eq!(*mask_kind, oxideav_svg::MaskKind::Luminance);
            match &**mask {
                Node::Path(p) => {
                    assert_eq!(p.path.commands.len(), 5, "rect outline as the mask");
                    match &p.fill {
                        Some(Paint::Solid(c)) => assert_eq!(*c, Rgba::opaque(255, 255, 255)),
                        other => panic!("mask must be opaque white, got {other:?}"),
                    }
                    assert!(p.stroke.is_none());
                }
                other => panic!("expected the shape as mask, got {other:?}"),
            }
            match &**content {
                Node::Group(g) => {
                    // Box matrix [200 0 0 100 0 0] — the gradient space is
                    // the unit box.
                    assert_eq!(
                        g.transform,
                        Transform2D {
                            a: 200.0,
                            b: 0.0,
                            c: 0.0,
                            d: 100.0,
                            e: 0.0,
                            f: 0.0
                        }
                    );
                    assert_eq!(g.children.len(), 1);
                    let inner = match &g.children[0] {
                        Node::Path(p) => p,
                        other => panic!("expected gradient rect, got {other:?}"),
                    };
                    let rg = radial(inner);
                    assert!(close(rg.center.x, 0.5) && close(rg.center.y, 0.5), "{rg:?}");
                    assert!(close(rg.radius, 0.5), "{rg:?}");
                    assert!(rg.focal.is_none());
                    // The content rectangle covers the whole shape in
                    // gradient space (the unit box, plus margin).
                    let b = BBox::of_path(&inner.path).unwrap();
                    assert!(b.x <= 0.0 && b.y <= 0.0 && b.max_x() >= 1.0 && b.max_y() >= 1.0);
                }
                other => panic!("expected transformed content group, got {other:?}"),
            }
        }
        other => panic!("expected SoftMask layer, got {other:?}"),
    }
}

#[test]
fn masked_fill_keeps_a_plain_stroke_as_its_own_layer() {
    let src = doc(
        &format!(r##"<radialGradient id="g">{STOPS}</radialGradient>"##),
        r##"<rect x="10" y="10" width="180" height="80" fill="url(#g)" stroke="#000000" stroke-width="4" paint-order="stroke"/>"##,
    );
    let frame = parse(&src).unwrap();
    let layers = match &frame.root.children[0] {
        Node::Group(g) => &g.children,
        other => panic!("expected layer group, got {other:?}"),
    };
    assert_eq!(layers.len(), 2);
    // paint-order="stroke": stroke layer first, masked fill second.
    match &layers[0] {
        Node::Path(p) => {
            assert!(p.fill.is_none());
            assert_eq!(p.stroke.as_ref().unwrap().width, 4.0);
        }
        other => panic!("expected stroke layer first, got {other:?}"),
    }
    assert!(matches!(layers[1], Node::SoftMask { .. }));
}

#[test]
fn non_conformal_stroke_gradient_masks_with_the_stroked_outline() {
    // userSpaceOnUse radial with a non-uniform gradientTransform on a
    // stroke: the mask is the stroke itself (white, same width / caps),
    // the content is the gradient under the transform.
    let src = doc(
        &format!(
            r##"<radialGradient id="g" gradientUnits="userSpaceOnUse" cx="100" cy="50" r="40" gradientTransform="scale(2 1)">{STOPS}</radialGradient>"##
        ),
        r##"<path d="M 20 50 L 180 50" fill="none" stroke="url(#g)" stroke-width="10" stroke-linecap="round"/>"##,
    );
    let frame = parse(&src).unwrap();
    let layer = match &frame.root.children[0] {
        Node::Group(g) => &g.children[0],
        other => panic!("expected layer group, got {other:?}"),
    };
    match layer {
        Node::SoftMask { mask, content, .. } => {
            match &**mask {
                Node::Path(p) => {
                    assert!(p.fill.is_none());
                    let s = p.stroke.as_ref().unwrap();
                    assert_eq!(s.width, 10.0);
                    assert_eq!(s.cap, oxideav_svg::LineCap::Round);
                    assert!(matches!(s.paint, Paint::Solid(c) if c == Rgba::opaque(255, 255, 255)));
                }
                other => panic!("expected stroked mask, got {other:?}"),
            }
            match &**content {
                Node::Group(g) => {
                    assert_eq!(g.transform, Transform2D::scale(2.0, 1.0));
                    let inner = match &g.children[0] {
                        Node::Path(p) => p,
                        other => panic!("{other:?}"),
                    };
                    let rg = radial(inner);
                    assert!(close(rg.center.x, 100.0) && close(rg.radius, 40.0));
                }
                other => panic!("{other:?}"),
            }
        }
        other => panic!("expected SoftMask, got {other:?}"),
    }
}

#[test]
fn uniform_gradient_transform_keeps_radial_exact() {
    // scale(2) rotate(30) is conformal: circles stay circles, so the
    // paint stays a plain RadialGradient with radius × 2.
    let src = doc(
        &format!(
            r##"<radialGradient id="g" gradientUnits="userSpaceOnUse" cx="10" cy="0" r="5" gradientTransform="rotate(90) scale(2)">{STOPS}</radialGradient>"##
        ),
        r##"<rect x="0" y="0" width="200" height="100" fill="url(#g)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let g = radial(paths(&frame)[0]);
    assert!(close(g.center.x, 0.0) && close(g.center.y, 20.0), "{g:?}");
    assert!(close(g.radius, 10.0), "{g:?}");
}

#[test]
fn zero_extent_box_ignores_an_object_bounding_box_gradient() {
    // §8.11: "When the geometry of the applicable element has no width
    // or height and objectBoundingBox is specified, then the given
    // effect (e.g., a gradient or a filter) will be ignored" — a
    // horizontal line's box has no height, "even when the line has
    // actual thickness … since stroke width is ignored". The same line
    // under userSpaceOnUse paints.
    let src = doc(
        &format!(
            r##"<linearGradient id="obb">{STOPS}</linearGradient>
                <linearGradient id="usu" gradientUnits="userSpaceOnUse" x1="20" x2="180">{STOPS}</linearGradient>"##
        ),
        r##"<line x1="20" y1="30" x2="180" y2="30" stroke="url(#obb)" stroke-width="8"/>
            <line x1="20" y1="70" x2="180" y2="70" stroke="url(#usu)" stroke-width="8"/>"##,
    );
    let frame = parse(&src).unwrap();
    let ps = paths(&frame);
    assert!(ps[0].stroke.is_none(), "{:?}", ps[0].stroke);
    let s = ps[1].stroke.as_ref().expect("userSpaceOnUse line paints");
    match &s.paint {
        Paint::LinearGradient(g) => {
            assert_eq!(g.start, Point::new(20.0, 0.0));
            assert_eq!(g.end, Point::new(180.0, 0.0));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn stop_count_edge_cases() {
    // §14.2.4.3: no stops → as if `none`; one stop → solid colour.
    // §14.2.2.2: x1 = x2 and y1 = y2 → the last stop's colour.
    // §14.2.3.2 / §14.2.3.1: r = 0 → nothing painted.
    let src = doc(
        &format!(
            r##"<linearGradient id="none"/>
                <linearGradient id="one"><stop offset="0.3" stop-color="#00ff00" stop-opacity="0.5"/></linearGradient>
                <linearGradient id="zero" x1="0.5" x2="0.5" y1="0.2" y2="0.2">{STOPS}</linearGradient>
                <radialGradient id="r0" r="0">{STOPS}</radialGradient>"##
        ),
        r##"<rect width="10" height="10" fill="url(#none)"/>
            <rect width="10" height="10" fill="url(#one)"/>
            <rect width="10" height="10" fill="url(#zero)"/>
            <rect width="10" height="10" fill="url(#r0)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let ps = paths(&frame);
    assert!(ps[0].fill.is_none());
    assert_eq!(
        ps[1]
            .fill
            .as_ref()
            .map(|p| matches!(p, Paint::Solid(c) if *c == Rgba::new(0, 255, 0, 128))),
        Some(true),
        "{:?}",
        ps[1].fill
    );
    assert!(matches!(ps[2].fill, Some(Paint::Solid(c)) if c == Rgba::opaque(0, 0, 255)));
    assert!(ps[3].fill.is_none());
}

#[test]
fn stops_are_normalised_per_spec() {
    // §14.2.4.3: offsets < 0 → 0, > 1 → 1, non-monotonic → previous max.
    let src = doc(
        r##"<linearGradient id="g">
              <stop offset="-0.5" stop-color="#ff0000"/>
              <stop offset="80%" stop-color="#00ff00"/>
              <stop offset="0.2" stop-color="#0000ff"/>
              <stop offset="1.5" stop-color="#000000"/>
            </linearGradient>"##,
        r##"<rect width="10" height="10" fill="url(#g)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let g = linear(paths(&frame)[0]);
    let offsets: Vec<f32> = g.stops.iter().map(|s| s.offset).collect();
    assert_eq!(offsets, vec![0.0, 0.8, 0.8, 1.0]);
}

#[test]
fn fill_opacity_scales_every_stop() {
    let src = doc(
        &format!(r##"<linearGradient id="g">{STOPS}</linearGradient>"##),
        r##"<rect width="10" height="10" fill="url(#g)" fill-opacity="0.5" stroke="url(#g)" stroke-opacity="0.25"/>"##,
    );
    let frame = parse(&src).unwrap();
    let p = paths(&frame)[0];
    let g = linear(p);
    assert!(g.stops.iter().all(|s| s.color.a == 128), "{:?}", g.stops);
    match &p.stroke.as_ref().unwrap().paint {
        Paint::LinearGradient(sg) => {
            assert!(sg.stops.iter().all(|s| s.color.a == 64), "{:?}", sg.stops)
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn href_chain_inherits_units_and_user_space_geometry() {
    // The child specifies nothing: units (userSpaceOnUse), the
    // geometry, and the stops all come from the template — so the
    // child's gradient is NOT bound to the box.
    let src = doc(
        &format!(
            r##"<linearGradient id="tmpl" gradientUnits="userSpaceOnUse" x1="50" x2="150">{STOPS}</linearGradient>
                <linearGradient id="child" href="#tmpl"/>"##
        ),
        r##"<rect x="0" y="0" width="20" height="20" fill="url(#child)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let g = linear(paths(&frame)[0]);
    assert_eq!(g.start, Point::new(50.0, 0.0));
    assert_eq!(g.end, Point::new(150.0, 0.0));
}

#[test]
fn use_instance_binds_in_the_instances_local_space() {
    // The gradient is bound to the referenced rect's own box (0,0,40,20);
    // the <use> translate lives on the wrapping group, so the paint's
    // user-space geometry is the local one.
    let src = doc(
        &format!(
            r##"<linearGradient id="g">{STOPS}</linearGradient>
                <rect id="r" width="40" height="20" fill="url(#g)"/>"##
        ),
        r##"<use href="#r" x="100" y="50"/>"##,
    );
    let frame = parse(&src).unwrap();
    let g = linear(paths(&frame)[0]);
    assert_eq!(g.start, Point::new(0.0, 0.0));
    assert_eq!(g.end, Point::new(40.0, 0.0));
}

#[test]
fn writer_emits_user_space_geometry_and_round_trips() {
    let src = doc(
        &format!(r##"<linearGradient id="g">{STOPS}</linearGradient>"##),
        r##"<rect x="20" y="10" width="160" height="80" fill="url(#g)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let out = write(&frame);
    let s = std::str::from_utf8(&out).unwrap();
    assert!(
        s.contains(r#"x1="20" y1="10" x2="180" y2="10" gradientUnits="userSpaceOnUse""#),
        "{s}"
    );
    let again = parse(&out).unwrap();
    let g = linear(paths(&again)[0]);
    assert_eq!(g.start, Point::new(20.0, 10.0));
    assert_eq!(g.end, Point::new(180.0, 10.0));
}

#[test]
fn extras_round_trip_keeps_the_source_def_and_reference() {
    // Reference identity: the verbatim objectBoundingBox def is
    // re-emitted once, and the shape still references it (no
    // synthesised `grad1` twin).
    let src = doc(
        &format!(
            r##"<linearGradient id="g" x2="50%" gradientTransform="rotate(10)">{STOPS}</linearGradient>"##
        ),
        r##"<rect x="20" y="10" width="160" height="80" fill="url(#g)"/>
            <circle cx="150" cy="50" r="30" fill="url(#g)"/>"##,
    );
    let (frame, extras) = parse_with_extras(&src).unwrap();
    let out = write_with_extras(&frame, &extras);
    let s = std::str::from_utf8(&out).unwrap();
    assert_eq!(s.matches("<linearGradient").count(), 1, "{s}");
    assert_eq!(s.matches(r##"fill="url(#g)""##).count(), 2, "{s}");
    assert!(!s.contains("grad1"), "{s}");
    // And the re-parse resolves to the same per-shape geometry.
    let again = parse(&out).unwrap();
    let a = paths(&frame);
    let b = paths(&again);
    for (x, y) in a.iter().zip(b.iter()) {
        let (gx, gy) = (linear(x), linear(y));
        assert!(close(gx.start.x, gy.start.x) && close(gx.end.y, gy.end.y));
    }
}

#[test]
fn typed_chain_resolves_percentages_per_units() {
    let mut defs = DefsTables::new();
    let pct = |v: f32| Some(Length::new(v, oxideav_svg::length::LengthUnit::Percent));
    let obb = GradientDef::new(GradientKind::Linear {
        x1: pct(10.0),
        y1: None,
        x2: pct(60.0),
        y2: None,
    });
    let r = resolve_gradient_chain(&obb, &defs);
    assert_eq!(r.units, GradientUnits::ObjectBoundingBox);
    match r.kind {
        ResolvedGradientKind::Linear { x1, x2, y2, .. } => {
            assert!(close(x1, 0.1) && close(x2, 0.6) && close(y2, 0.0));
        }
        _ => panic!(),
    }
    // Same literal under userSpaceOnUse with a 200×100 viewport.
    let usu = obb.clone().with_units(GradientUnits::UserSpaceOnUse);
    defs.gradients.insert("u".into(), usu.clone());
    let ctx = oxideav_svg::length::ResolveContext::default().with_viewport(200.0, 100.0);
    let r = oxideav_svg::defs::resolve_gradient_chain_with(&usu, &defs, &ctx);
    match r.kind {
        ResolvedGradientKind::Linear { x1, x2, .. } => {
            assert!(close(x1, 20.0) && close(x2, 120.0));
        }
        _ => panic!(),
    }
}

#[test]
fn bind_gradient_reports_each_outcome() {
    let stops = vec![
        GradientStop::new(0.0, Rgba::opaque(255, 0, 0)),
        GradientStop::new(1.0, Rgba::opaque(0, 0, 255)),
    ];
    let def = GradientDef::new(GradientKind::radial()).with_stops(stops.clone());
    let r = resolve_gradient_chain(&def, &DefsTables::new());
    assert!(matches!(bind_gradient(&r, None), GradientPaint::None));
    assert!(matches!(
        bind_gradient(&r, Some(BBox::new(0.0, 0.0, 10.0, 0.0))),
        GradientPaint::None
    ));
    assert!(matches!(
        bind_gradient(&r, Some(BBox::new(5.0, 5.0, 10.0, 10.0))),
        GradientPaint::Paint(Paint::RadialGradient(_))
    ));
    assert!(matches!(
        bind_gradient(&r, Some(BBox::new(5.0, 5.0, 10.0, 20.0))),
        GradientPaint::Transformed { .. }
    ));
    let lin = GradientDef::new(GradientKind::linear())
        .with_stops(stops)
        .with_spread(SpreadMethod::Reflect);
    let r = resolve_gradient_chain(&lin, &DefsTables::new());
    match bind_gradient(&r, Some(BBox::new(5.0, 5.0, 10.0, 20.0))) {
        GradientPaint::Paint(Paint::LinearGradient(g)) => {
            assert_eq!(g.start, Point::new(5.0, 5.0));
            assert_eq!(g.end, Point::new(15.0, 5.0));
            assert_eq!(g.spread, SpreadMethod::Reflect);
        }
        other => panic!("{other:?}"),
    }
}
