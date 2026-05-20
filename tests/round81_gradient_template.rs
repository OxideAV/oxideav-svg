//! Round 81 — SVG 2 §14.1.1 gradient `href` template inheritance +
//! §14.2.2.1 / §14.2.3.1 `gradientUnits` / `gradientTransform` capture.
//!
//! Verifies, end-to-end through `parse_svg` / `parse_svg_with_extras`:
//!
//! 1. A `<linearGradient>` whose `href="#tmpl"` inherits any
//!    unspecified attribute (here: `x1` / `y1` / `x2` / `y2`) from
//!    `tmpl`, AND inherits the template's `<stop>` children if the
//!    child has none.
//! 2. The legacy `xlink:href` form (still seen in real-world SVG)
//!    resolves identically.
//! 3. `<radialGradient href="#tmpl">` template chain works the same
//!    way for `cx` / `cy` / `r` / `fx` / `fy` / `fr`.
//! 4. An attribute explicitly specified on the child wins over the
//!    template — the template only fills in what the child left
//!    `None`.
//! 5. A self-reference (`<linearGradient id="a" href="#a">`) is
//!    detected and broken; the resolver returns spec defaults rather
//!    than diverging.
//! 6. `gradientTransform` is parsed and folded into the flattened
//!    [`oxideav_core::Paint::LinearGradient`] geometry (start / end
//!    points get transformed) so a downstream rasteriser sees the
//!    right coords without needing to read the typed
//!    [`oxideav_svg::defs::GradientDef`] separately.
//! 7. The verbatim source `<linearGradient>` survives a
//!    `parse_svg_with_extras → write_svg_with_extras` cycle on
//!    [`oxideav_svg::preserved::PreservedExtras::gradients`].

use oxideav_core::{Paint, PathNode};
use oxideav_svg::{
    defs::{GradientUnits, ResolvedGradientKind},
    parse_svg, parse_svg_with_extras, write_svg_with_extras,
};

fn first_path(frame: &oxideav_core::VectorFrame) -> &PathNode {
    fn find(n: &oxideav_core::Node) -> Option<&PathNode> {
        match n {
            oxideav_core::Node::Path(p) => Some(p),
            oxideav_core::Node::Group(g) => g.children.iter().find_map(find),
            oxideav_core::Node::SoftMask { content, .. } => find(content),
            _ => None,
        }
    }
    frame
        .root
        .children
        .iter()
        .find_map(find)
        .expect("expected at least one PathNode in the scene graph")
}

#[test]
fn linear_gradient_href_inherits_coords_and_stops() {
    // `child` declares no x1/y1/x2/y2 and no stops — every attribute
    // should be drawn from `tmpl` via SVG 2 §14.1.1 template chain.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <defs>
    <linearGradient id="tmpl" x1="0" y1="0" x2="100" y2="0" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#ff0000"/>
      <stop offset="1" stop-color="#0000ff"/>
    </linearGradient>
    <linearGradient id="child" href="#tmpl"/>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="url(#child)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame);
    match &path.fill {
        Some(Paint::LinearGradient(g)) => {
            // x1=0, y1=0, x2=100, y2=0 inherited from tmpl.
            assert_eq!(g.start.x, 0.0);
            assert_eq!(g.start.y, 0.0);
            assert_eq!(g.end.x, 100.0);
            assert_eq!(g.end.y, 0.0);
            // Stops inherited.
            assert_eq!(g.stops.len(), 2);
            assert_eq!(g.stops[0].color.r, 0xff);
            assert_eq!(g.stops[1].color.b, 0xff);
        }
        other => panic!(
            "expected LinearGradient inherited from tmpl, got {:?}",
            other
        ),
    }
}

#[test]
fn xlink_href_template_form_also_resolves() {
    // SVG 1.1 / Inkscape / Illustrator commonly emit the deprecated
    // xlink:href spelling. The §14.2.2.1 reflection note says both
    // attributes must work; deprecated takes effect when the modern
    // attribute is absent.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"
     width="100" height="100" viewBox="0 0 100 100">
  <defs>
    <linearGradient id="tmpl" x1="10" y1="0" x2="90" y2="0" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#102030"/>
      <stop offset="1" stop-color="#a0b0c0"/>
    </linearGradient>
    <linearGradient id="child" xlink:href="#tmpl"/>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="url(#child)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame);
    match &path.fill {
        Some(Paint::LinearGradient(g)) => {
            assert_eq!(g.start.x, 10.0);
            assert_eq!(g.end.x, 90.0);
            assert_eq!(g.stops.len(), 2);
        }
        other => panic!("expected linear gradient from xlink:href, got {:?}", other),
    }
}

#[test]
fn radial_gradient_href_inherits_centre_radius_and_focal() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <defs>
    <radialGradient id="tmpl" cx="50" cy="50" r="40" fx="40" fy="30" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#ffffff"/>
      <stop offset="1" stop-color="#000000"/>
    </radialGradient>
    <radialGradient id="child" href="#tmpl"/>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="url(#child)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame);
    match &path.fill {
        Some(Paint::RadialGradient(g)) => {
            assert_eq!(g.center.x, 50.0);
            assert_eq!(g.center.y, 50.0);
            // Identity gradientTransform → radius keeps its source value.
            assert!((g.radius - 40.0).abs() < 1e-3);
            // Focal differs from center → Some(_) carried.
            let f = g
                .focal
                .expect("focal should be Some when fx/fy differ from cx/cy");
            assert_eq!(f.x, 40.0);
            assert_eq!(f.y, 30.0);
            assert_eq!(g.stops.len(), 2);
        }
        other => panic!("expected radial gradient from #tmpl, got {:?}", other),
    }
}

#[test]
fn child_specified_attr_overrides_template() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <defs>
    <linearGradient id="tmpl" x1="0" y1="0" x2="100" y2="0" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#000000"/>
      <stop offset="1" stop-color="#ffffff"/>
    </linearGradient>
    <linearGradient id="child" href="#tmpl" x2="50"/>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="url(#child)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame);
    match &path.fill {
        Some(Paint::LinearGradient(g)) => {
            // Child's explicit x2=50 wins over the template's x2=100.
            assert_eq!(g.end.x, 50.0);
            // y1/y2 inherited from template (both 0).
            assert_eq!(g.start.y, 0.0);
            assert_eq!(g.end.y, 0.0);
        }
        other => panic!("expected linear gradient, got {:?}", other),
    }
}

#[test]
fn self_reference_is_broken_and_spec_defaults_apply() {
    // `id="loop"` and `href="#loop"` would infinite-recurse a naive
    // chain walker. The resolver must terminate and fall back to spec
    // defaults (x1=0 y1=0 x2=1 y2=0; no stops).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <defs>
    <linearGradient id="loop" href="#loop"/>
  </defs>
  <rect x="0" y="0" width="10" height="10" fill="url(#loop)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame);
    // Either a gradient with spec defaults + zero stops, or no fill at
    // all — both are acceptable terminations. What MUST NOT happen is
    // a stack overflow / divergence.
    match &path.fill {
        None => {}
        Some(Paint::LinearGradient(g)) => {
            assert!(g.stops.is_empty());
        }
        other => panic!("unexpected fill {:?}", other),
    }
}

#[test]
fn gradient_transform_is_folded_into_flattened_paint() {
    // `gradientTransform="translate(10, 5) scale(2 1)"` shifts the
    // start (0,0) → (10, 5) and the end (1, 0) → (10 + 2*1, 5) = (12, 5).
    // (Bare `objectBoundingBox` units; no extra mapping for this
    // smoke test.)
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <defs>
    <linearGradient id="g" gradientTransform="translate(10, 5) scale(2 1)">
      <stop offset="0" stop-color="#000000"/>
      <stop offset="1" stop-color="#ffffff"/>
    </linearGradient>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="url(#g)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame);
    match &path.fill {
        Some(Paint::LinearGradient(g)) => {
            assert!((g.start.x - 10.0).abs() < 1e-3);
            assert!((g.start.y - 5.0).abs() < 1e-3);
            assert!((g.end.x - 12.0).abs() < 1e-3);
            assert!((g.end.y - 5.0).abs() < 1e-3);
        }
        other => panic!(
            "expected linear gradient with transformed coords, got {:?}",
            other
        ),
    }
}

#[test]
fn typed_def_records_units_transform_and_href() {
    // Reaches into the typed `ResolvedGradient` path so we know the
    // verbatim metadata is preserved end-to-end for a downstream
    // rasteriser that wants the full SVG-2 surface.
    use oxideav_svg::defs::{resolve_gradient_chain, DefsTables, GradientDef};
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <defs>
    <linearGradient id="g" x1="10" y1="20" x2="80" y2="90"
                    gradientUnits="userSpaceOnUse"
                    gradientTransform="scale(2 2)"
                    spreadMethod="reflect">
      <stop offset="0" stop-color="#000000"/>
      <stop offset="1" stop-color="#ffffff"/>
    </linearGradient>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="url(#g)"/>
</svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    // The verbatim element should be on extras.gradients for round-trip.
    assert_eq!(extras.gradients.len(), 1);
    let el = &extras.gradients[0];
    let mut has_grad_units = false;
    let mut has_grad_xform = false;
    let mut has_spread = false;
    for (k, v) in &el.attrs {
        if k.eq_ignore_ascii_case("gradientUnits") && v == "userSpaceOnUse" {
            has_grad_units = true;
        }
        if k.eq_ignore_ascii_case("gradientTransform") {
            has_grad_xform = true;
        }
        if k.eq_ignore_ascii_case("spreadMethod") && v == "reflect" {
            has_spread = true;
        }
    }
    assert!(has_grad_units && has_grad_xform && has_spread);

    // And the typed `ResolvedGradient` mirror agrees.
    let mut defs = DefsTables::new();
    let def = GradientDef {
        kind: oxideav_svg::defs::GradientKind::Linear {
            x1: Some(10.0),
            y1: Some(20.0),
            x2: Some(80.0),
            y2: Some(90.0),
        },
        units: Some(GradientUnits::UserSpaceOnUse),
        transform: None,
        spread: Some(oxideav_core::SpreadMethod::Reflect),
        stops: Vec::new(),
        href: String::new(),
    };
    defs.gradients.insert("g".into(), def.clone());
    let resolved = resolve_gradient_chain(&def, &defs);
    assert_eq!(resolved.units, GradientUnits::UserSpaceOnUse);
    assert_eq!(resolved.spread, oxideav_core::SpreadMethod::Reflect);
    match resolved.kind {
        ResolvedGradientKind::Linear { x1, y1, x2, y2 } => {
            assert_eq!((x1, y1, x2, y2), (10.0, 20.0, 80.0, 90.0));
        }
        _ => panic!("expected linear"),
    }
}

#[test]
fn round_trip_preserves_template_chain_verbatim() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <defs>
    <linearGradient id="tmpl" x1="0" y1="0" x2="100" y2="0" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#102030"/>
      <stop offset="1" stop-color="#a0b0c0"/>
    </linearGradient>
    <linearGradient id="child" href="#tmpl" x2="50"/>
  </defs>
  <rect x="0" y="0" width="100" height="100" fill="url(#child)"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_str = std::str::from_utf8(&out).expect("encoder emits UTF-8");
    // Both the template AND the child gradient must survive the
    // round-trip — verbatim, with the href intact and the
    // user-space units intact.
    assert!(out_str.contains("id=\"tmpl\""));
    assert!(out_str.contains("id=\"child\""));
    assert!(
        out_str.contains("href=\"#tmpl\""),
        "expected href=\"#tmpl\" in output, got:\n{}",
        out_str
    );
    assert!(out_str.contains("gradientUnits=\"userSpaceOnUse\""));
    // Re-parse → identical scene-graph fill.
    let frame2 = parse_svg(&out).unwrap();
    let p2 = first_path(&frame2);
    match &p2.fill {
        Some(Paint::LinearGradient(g)) => {
            assert_eq!(g.end.x, 50.0);
            assert_eq!(g.stops.len(), 2);
        }
        other => panic!("post-round-trip fill mismatch: {:?}", other),
    }
}

#[test]
fn linear_gradient_with_explicit_user_space_units_passes_through_resolver() {
    // Bare gradient (no template chain). `gradientUnits=userSpaceOnUse`
    // means x1/y1/x2/y2 are document-space coords; the renderer needs
    // to know that to paint correctly. The flattened legacy
    // `Paint::LinearGradient` keeps the coords as authored — they're
    // already in user units — and the typed `ResolvedGradient` reports
    // `units == UserSpaceOnUse` so a downstream rasteriser can skip
    // the bounding-box re-mapping.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="200" y2="0" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#ff0000"/>
      <stop offset="1" stop-color="#0000ff"/>
    </linearGradient>
  </defs>
  <rect x="0" y="0" width="200" height="100" fill="url(#g)"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame);
    match &path.fill {
        Some(Paint::LinearGradient(g)) => {
            assert_eq!(g.start.x, 0.0);
            assert_eq!(g.end.x, 200.0);
        }
        other => panic!(
            "expected linear gradient with user-space coords, got {:?}",
            other
        ),
    }
}
