//! Paint servers on text (SVG 2 §11.1 / §13 / §14): gradients and
//! patterns as `fill` and `stroke` of `<text>`, `<tspan>` and
//! `<textPath>`, bound to the bounding box of the whole `<text>`
//! element under `objectBoundingBox` units.
//!
//! Own integration-test binary: the font hook is process-global.

#![cfg(feature = "text")]

use oxideav_scribe::{Face, FaceChain};
use oxideav_svg::{Group, Node, Paint, SvgDocument, parse, text::set_font_resolver};

const FONT: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");

fn install() {
    let _ = set_font_resolver(|_family, _size| {
        Face::from_ttf_bytes(FONT.to_vec()).ok().map(FaceChain::new)
    });
}

fn doc(body: &str) -> SvgDocument {
    install();
    let src = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="600" height="300">
<defs>
<linearGradient id="lg"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient>
<linearGradient id="lgu" gradientUnits="userSpaceOnUse" x1="10" y1="20" x2="310" y2="20"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient>
<radialGradient id="rg"><stop offset="0" stop-color="white"/><stop offset="1" stop-color="black"/></radialGradient>
<pattern id="pat" width="8" height="8" patternUnits="userSpaceOnUse"><rect width="4" height="4" fill="green"/></pattern>
<path id="line" d="M 50 200 L 550 200"/>
</defs>{body}</svg>"##
    );
    parse(src.as_bytes()).expect("parse")
}

fn text_group(d: &SvgDocument) -> &Group {
    match d.root.children.last().expect("text node") {
        Node::Group(g) => g,
        other => panic!("expected the text group, got {other:?}"),
    }
}

/// Advance of one glyph of the monospaced fixture at 16 px.
fn advance() -> f32 {
    let d = doc(r#"<text x="0" y="50" font-size="16">AB</text>"#);
    let g = text_group(&d);
    let xs: Vec<f32> = g
        .children
        .iter()
        .map(|c| match c {
            Node::Group(p) => p.transform.e,
            other => panic!("plain text is one placement group per glyph, got {other:?}"),
        })
        .collect();
    assert_eq!(xs.len(), 2);
    xs[1] - xs[0]
}

fn only_path(g: &Group) -> &oxideav_svg::PathNode {
    assert_eq!(
        g.children.len(),
        1,
        "one painted geometry: {:?}",
        g.children
    );
    match &g.children[0] {
        Node::Path(p) => p,
        other => panic!("expected a path, got {other:?}"),
    }
}

#[test]
fn plain_colour_text_stays_one_group_per_glyph() {
    let d = doc(r#"<text x="100" y="50" font-size="16" fill="teal">ABCDE</text>"#);
    let g = text_group(&d);
    assert_eq!(g.children.len(), 5);
    assert!(g.children.iter().all(|c| matches!(c, Node::Group(_))));
}

#[test]
fn bounding_box_gradient_spans_the_whole_text_element() {
    let adv = advance();
    let d = doc(r##"<text x="100" y="50" font-size="16" fill="url(#lg)">ABCDE</text>"##);
    let p = only_path(text_group(&d));
    assert!(p.stroke.is_none());
    let Some(Paint::LinearGradient(lg)) = &p.fill else {
        panic!("expected a linear gradient fill, got {:?}", p.fill);
    };
    // x1 = 0 %, x2 = 100 % of the text's bounding box: the five glyph
    // cells from x = 100.
    assert!((lg.start.x - 100.0).abs() < 1e-2, "{:?}", lg.start);
    assert!(
        (lg.end.x - (100.0 + 5.0 * adv)).abs() < 1e-2,
        "{:?}",
        lg.end
    );
    // The box's top is one ascent above the baseline (§8.10: the glyph
    // cell is the full em box), so y1 = y2 = 0 % sits above y = 50.
    assert!(
        lg.start.y < 50.0 - 8.0 && lg.start.y > 50.0 - 20.0,
        "{:?}",
        lg.start
    );
    assert_eq!(lg.stops.len(), 2);
}

#[test]
fn tspans_bind_paint_servers_to_the_text_bounding_box() {
    let adv = advance();
    let d =
        doc(r##"<text x="100" y="50" font-size="16">AB<tspan fill="url(#lg)">CD</tspan>E</text>"##);
    let g = text_group(&d);
    // A B (plain), one gradient geometry for "CD", then E.
    assert_eq!(g.children.len(), 4, "{:?}", g.children);
    let Node::Path(p) = &g.children[2] else {
        panic!(
            "expected the tspan's flattened outline, got {:?}",
            g.children[2]
        );
    };
    let Some(Paint::LinearGradient(lg)) = &p.fill else {
        panic!("expected a gradient");
    };
    // "relative to the entire 'text' element in all cases" (§11.1): the
    // gradient starts at the text's x = 100, not at the tspan's "C".
    assert!((lg.start.x - 100.0).abs() < 1e-2, "{:?}", lg.start);
    assert!(
        (lg.end.x - (100.0 + 5.0 * adv)).abs() < 1e-2,
        "{:?}",
        lg.end
    );
    // The geometry itself is the two glyphs of the tspan.
    let b = oxideav_svg::bbox::BBox::of_path(&p.path).expect("outline bbox");
    assert!(
        b.x >= 100.0 + 2.0 * adv - 0.5 && b.max_x() <= 100.0 + 4.0 * adv + 0.5,
        "{b:?}"
    );
}

#[test]
fn user_space_gradient_keeps_its_own_coordinates() {
    let d = doc(r##"<text x="100" y="50" font-size="16" fill="url(#lgu)">ABCDE</text>"##);
    let p = only_path(text_group(&d));
    let Some(Paint::LinearGradient(lg)) = &p.fill else {
        panic!("expected a gradient");
    };
    assert!((lg.start.x - 10.0).abs() < 1e-4 && (lg.start.y - 20.0).abs() < 1e-4);
    assert!((lg.end.x - 310.0).abs() < 1e-4 && (lg.end.y - 20.0).abs() < 1e-4);
}

#[test]
fn gradient_stroke_on_text() {
    let d = doc(
        r##"<text x="100" y="50" font-size="16" fill="none" stroke="url(#lg)" stroke-width="2">AB</text>"##,
    );
    let p = only_path(text_group(&d));
    assert!(p.fill.is_none());
    let s = p.stroke.as_ref().expect("stroke");
    assert_eq!(s.width, 2.0);
    assert!(matches!(s.paint, Paint::LinearGradient(_)));
}

#[test]
fn solid_stroke_and_fill_share_one_geometry_and_honour_paint_order() {
    let d = doc(
        r##"<text x="100" y="50" font-size="16" fill="gold" stroke="navy" stroke-width="3">AB</text>"##,
    );
    let p = only_path(text_group(&d));
    assert!(p.fill.is_some() && p.stroke.is_some());

    // `paint-order: stroke` paints the stroke beneath the fill: two
    // single-purpose paths, stroke first.
    let d = doc(
        r##"<text x="100" y="50" font-size="16" fill="gold" stroke="navy" stroke-width="3" paint-order="stroke">AB</text>"##,
    );
    let g = text_group(&d);
    let Node::Group(layers) = &g.children[0] else {
        panic!("expected a layer group, got {:?}", g.children[0]);
    };
    let kinds: Vec<(bool, bool)> = layers
        .children
        .iter()
        .map(|c| match c {
            Node::Path(p) => (p.fill.is_some(), p.stroke.is_some()),
            other => panic!("unexpected layer {other:?}"),
        })
        .collect();
    assert_eq!(kinds, [(false, true), (true, false)]);
}

#[test]
fn pattern_fill_tiles_through_the_text_outline() {
    let d = doc(r##"<text x="100" y="50" font-size="32" fill="url(#pat)">AB</text>"##);
    let g = text_group(&d);
    let Node::Group(layers) = &g.children[0] else {
        panic!("expected a layer group, got {:?}", g.children[0]);
    };
    let Node::SoftMask { mask, content, .. } = &layers.children[0] else {
        panic!(
            "a pattern paints through a soft mask, got {:?}",
            layers.children[0]
        );
    };
    // The mask is the glyph outlines painted white; the content the tiles.
    let Node::Path(m) = mask.as_ref() else {
        panic!("mask should be the text outline");
    };
    assert!(m.path.commands.len() > 10);
    let Node::Group(tiles) = content.as_ref() else {
        panic!("content should be the tile group");
    };
    assert!(tiles.children.len() >= 4, "tiles: {}", tiles.children.len());
}

#[test]
fn pattern_stroke_on_text() {
    let d = doc(
        r##"<text x="100" y="50" font-size="32" fill="none" stroke="url(#pat)" stroke-width="3">AB</text>"##,
    );
    let g = text_group(&d);
    let Node::Group(layers) = &g.children[0] else {
        panic!("expected a layer group");
    };
    let Node::SoftMask { mask, .. } = &layers.children[0] else {
        panic!("a pattern stroke paints through a soft mask");
    };
    let Node::Path(m) = mask.as_ref() else {
        panic!("mask should be the stroked outline");
    };
    assert!(m.fill.is_none() && m.stroke.as_ref().is_some_and(|s| s.width == 3.0));
}

#[test]
fn radial_gradient_on_a_wide_text_box_goes_elliptical() {
    // The text box is far from square, so the bounding-box radial
    // gradient is elliptical in user space: painted through a mask of
    // the outline (the same representation shapes use).
    let d = doc(r##"<text x="100" y="50" font-size="16" fill="url(#rg)">ABCDEFGH</text>"##);
    let g = text_group(&d);
    let Node::Group(layers) = &g.children[0] else {
        panic!("expected a layer group, got {:?}", g.children[0]);
    };
    assert!(matches!(layers.children[0], Node::SoftMask { .. }));
}

#[test]
fn text_path_takes_paint_servers_too() {
    let d = doc(
        r##"<text font-size="16"><textPath href="#line" fill="url(#lg)" stroke="url(#lgu)">ABCDE</textPath></text>"##,
    );
    let p = only_path(text_group(&d));
    assert!(matches!(p.fill, Some(Paint::LinearGradient(_))));
    assert!(
        p.stroke
            .as_ref()
            .is_some_and(|s| matches!(s.paint, Paint::LinearGradient(_)))
    );
    // On the straight path the outline sits just above y = 200.
    let b = oxideav_svg::bbox::BBox::of_path(&p.path).unwrap();
    assert!(b.x >= 49.0 && b.max_y() <= 201.0 && b.y > 180.0, "{b:?}");
}

#[test]
fn fill_opacity_scales_the_gradient_stops() {
    let d = doc(
        r##"<text x="100" y="50" font-size="16" fill="url(#lg)" fill-opacity="0.5">AB</text>"##,
    );
    let p = only_path(text_group(&d));
    let Some(Paint::LinearGradient(lg)) = &p.fill else {
        panic!("expected a gradient");
    };
    assert!(
        lg.stops.iter().all(|s| (126..=129).contains(&s.color.a)),
        "{:?}",
        lg.stops
    );
}

#[test]
fn unresolved_reference_uses_the_fallback_colour() {
    let d = doc(r##"<text x="100" y="50" font-size="16" fill="url(#missing) teal">AB</text>"##);
    let p = only_path(text_group(&d));
    let Some(Paint::Solid(c)) = &p.fill else {
        panic!("expected the fallback colour, got {:?}", p.fill);
    };
    assert_eq!((c.r, c.g, c.b, c.a), (0, 128, 128, 255));
    // No fallback: nothing is painted.
    let d = doc(r##"<text x="100" y="50" font-size="16" fill="url(#missing)">AB</text>"##);
    assert!(text_group(&d).children.is_empty());
}

#[test]
fn css_declared_paint_servers_reach_text() {
    let d = doc(
        r##"<style>.t { fill: url(#lg); stroke: url(#lgu); stroke-width: 1.5 }</style>
<text class="t" x="100" y="50" font-size="16">AB</text>"##,
    );
    let p = only_path(text_group(&d));
    assert!(matches!(p.fill, Some(Paint::LinearGradient(_))));
    assert!(p.stroke.as_ref().is_some_and(|s| s.width == 1.5));
}

#[test]
fn hidden_gradient_text_paints_nothing() {
    let d = doc(
        r##"<text x="100" y="50" font-size="16" fill="url(#lg)" stroke="red" visibility="hidden">AB</text>"##,
    );
    let g = text_group(&d);
    // Geometry is kept (SVG 1.1 §11.5), unpainted.
    for c in &g.children {
        let Node::Group(p) = c else {
            panic!("placement groups expected")
        };
        let Node::Path(n) = &p.children[0] else {
            panic!("glyph path")
        };
        assert!(n.fill.is_none() && n.stroke.is_none());
    }
}

#[test]
fn text_transform_and_opacity_wrap_the_whole_element() {
    let d = doc(
        r##"<text x="100" y="50" font-size="16" transform="translate(7 9)" opacity="0.5" fill="url(#lg)">AB</text>"##,
    );
    let g = text_group(&d);
    assert_eq!((g.transform.e, g.transform.f), (7.0, 9.0));
    assert_eq!(g.opacity, 0.5);
    // The paint server is bound in the text's own user space (before
    // the transform).
    let Some(Paint::LinearGradient(lg)) = &only_path(g).fill else {
        panic!("expected a gradient");
    };
    assert!((lg.start.x - 100.0).abs() < 1e-2);
}
