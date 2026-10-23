//! Round 480 — `<pattern>` paint servers (SVG 2 §14.3, issue #13).
//!
//! The scene graph has no `Paint::Pattern`, so a pattern-filled shape is
//! expanded into the tiles that cover it, painted through a soft mask
//! of the shape (`element::expand_pattern`). Exact-structure checks
//! here; the rsvg-convert pixel gate is at the bottom (skipped when the
//! tool is absent).

use std::process::Command;

use oxideav_svg::bbox::BBox;
use oxideav_svg::defs::{DefsTables, PatternDef, PatternUnits, resolve_pattern_chain};
use oxideav_svg::element::{MAX_PATTERN_TILES, expand_pattern};
use oxideav_svg::length::{Length, LengthUnit};
use oxideav_svg::{Group, Node, Paint, Path, PathNode, Rgba, Transform2D, parse, write};

const W: u32 = 200;
const H: u32 = 100;

fn doc(defs: &str, body: &str) -> Vec<u8> {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}"><defs>{defs}</defs>{body}</svg>"##
    )
    .into_bytes()
}

/// `(mask path, tile group)` of the shape's first pattern layer.
fn pattern_layer(frame: &oxideav_svg::SvgDocument, shape_idx: usize) -> (&PathNode, &Group) {
    let layers = match &frame.root.children[shape_idx] {
        Node::Group(g) => g,
        other => panic!("expected layer group, got {other:?}"),
    };
    let sm = layers
        .children
        .iter()
        .find(|c| matches!(c, Node::SoftMask { .. }))
        .expect("a SoftMask layer");
    match sm {
        Node::SoftMask { mask, content, .. } => {
            let m = match &**mask {
                Node::Path(p) => p,
                other => panic!("mask must be the shape path, got {other:?}"),
            };
            let t = match &**content {
                Node::Group(g) => g,
                other => panic!("content must be the tile group, got {other:?}"),
            };
            (m, t)
        }
        _ => unreachable!(),
    }
}

fn tile(t: &Group, i: usize) -> (&Group, &Group) {
    match &t.children[i] {
        Node::Group(outer) => match &outer.children[0] {
            Node::Group(inner) => (outer, inner),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn user_space_tiles_cover_the_shape_and_clip_to_the_tile() {
    let src = doc(
        r##"<pattern id="p" patternUnits="userSpaceOnUse" x="5" y="5" width="20" height="10"><rect width="25" height="5" fill="#ff0000"/></pattern>"##,
        r##"<rect x="20" y="10" width="160" height="80" fill="url(#p)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let (mask, tiles) = pattern_layer(&frame, 0);
    // Mask = the rect, opaque white.
    assert!(matches!(mask.fill, Some(Paint::Solid(c)) if c == Rgba::opaque(255, 255, 255)));
    assert_eq!(
        BBox::of_path(&mask.path).unwrap(),
        BBox::new(20.0, 10.0, 160.0, 80.0)
    );
    // Columns: tiles start at x = 5 + 20m; the box spans 20..180 →
    // m = 0..=8 (9 columns); rows: y = 5 + 10n over 10..90 → n = 0..=8
    // (9 rows).
    assert_eq!(tiles.children.len(), 81);
    let (outer, inner) = tile(tiles, 0);
    assert_eq!(outer.transform, Transform2D::translate(5.0, 5.0));
    let clip = outer.clip.as_ref().expect("overflow: hidden clip");
    assert_eq!(
        BBox::of_path(clip).unwrap(),
        BBox::new(0.0, 0.0, 20.0, 10.0)
    );
    assert!(inner.transform.is_identity());
    assert!(inner.cache_key.is_some());
    assert_eq!(inner.children.len(), 1);
    let (outer2, inner2) = tile(tiles, 1);
    assert_eq!(outer2.transform, Transform2D::translate(25.0, 5.0));
    assert_eq!(inner2.cache_key, inner.cache_key);
}

#[test]
fn object_bounding_box_tiles_are_fractions_of_the_shape_box() {
    // Default patternUnits: a 0.25 × 0.5 tile on a 160 × 80 box is
    // 40 × 40 user units, anchored at the box origin; percentages mean
    // the same fraction.
    let src = doc(
        r##"<pattern id="a" width="0.25" height="0.5"><circle cx="5" cy="5" r="4"/></pattern>
            <pattern id="b" width="25%" height="50%" x="10%"><circle cx="5" cy="5" r="4"/></pattern>"##,
        r##"<rect x="20" y="10" width="160" height="80" fill="url(#a)"/>
            <rect x="20" y="10" width="160" height="80" fill="url(#b)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let (_, ta) = pattern_layer(&frame, 0);
    assert_eq!(ta.children.len(), 4 * 2);
    let (outer, inner) = tile(ta, 0);
    assert_eq!(outer.transform, Transform2D::translate(20.0, 10.0));
    // The dot stays inside its 40 × 40 tile: no per-tile clip needed.
    assert!(outer.clip.is_none());
    assert_eq!(tile(ta, 1).0.transform, Transform2D::translate(60.0, 10.0));
    assert_eq!(tile(ta, 4).0.transform, Transform2D::translate(20.0, 50.0));
    // patternContentUnits default userSpaceOnUse: content untouched.
    assert!(inner.transform.is_identity());
    let (_, tb) = pattern_layer(&frame, 1);
    // x = 10% of 160 = 16 → tiles at 36, 76, 116, 156 and the one
    // starting at -4 that still touches the box: 5 columns.
    assert_eq!(tb.children.len(), 5 * 2);
    let (outer_b, _) = tile(tb, 0);
    assert!(close(outer_b.transform.e, -4.0), "{:?}", outer_b.transform);
}

#[test]
fn content_units_object_bounding_box_scale_the_content() {
    let src = doc(
        r##"<pattern id="p" patternContentUnits="objectBoundingBox" width="0.5" height="0.5"><rect width="0.25" height="0.25"/></pattern>"##,
        r##"<rect x="0" y="0" width="200" height="100" fill="url(#p)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let (_, tiles) = pattern_layer(&frame, 0);
    assert_eq!(tiles.children.len(), 4);
    let (_, inner) = tile(tiles, 0);
    assert_eq!(inner.transform, Transform2D::scale(200.0, 100.0));
}

#[test]
fn view_box_fits_the_content_into_the_tile() {
    // viewBox 0 0 10 10 into a 50 × 50 tile: uniform scale 5; the
    // content-units keyword is ignored when a viewBox is given.
    let src = doc(
        r##"<pattern id="p" patternUnits="userSpaceOnUse" width="50" height="50" viewBox="0 0 10 10" patternContentUnits="objectBoundingBox"><path d="M 0 0 L 7 0 L 3.5 7 z"/></pattern>"##,
        r##"<ellipse cx="100" cy="50" rx="90" ry="40" fill="url(#p)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let (_, tiles) = pattern_layer(&frame, 0);
    let (_, inner) = tile(tiles, 0);
    assert_eq!(inner.transform, Transform2D::scale(5.0, 5.0));
    // Non-uniform viewBox with preserveAspectRatio="none" stretches.
    let src = doc(
        r##"<pattern id="p" patternUnits="userSpaceOnUse" width="50" height="20" viewBox="0 0 10 10" preserveAspectRatio="none"><rect width="10" height="10"/></pattern>"##,
        r##"<rect width="200" height="100" fill="url(#p)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let (_, tiles) = pattern_layer(&frame, 0);
    let (_, inner) = tile(tiles, 0);
    assert_eq!(inner.transform, Transform2D::scale(5.0, 2.0));
}

#[test]
fn pattern_transform_maps_pattern_space_and_widens_the_tile_cover() {
    let src = doc(
        r##"<pattern id="p" patternUnits="userSpaceOnUse" width="20" height="20" patternTransform="rotate(45)"><circle cx="10" cy="10" r="5"/></pattern>"##,
        r##"<rect x="50" y="20" width="100" height="60" fill="url(#p)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let (_, tiles) = pattern_layer(&frame, 0);
    let (outer, _) = tile(tiles, 0);
    // Every tile transform is rotate(45) ∘ translate(m·20, n·20): the
    // linear part is the rotation.
    let rot = Transform2D::rotate(std::f32::consts::FRAC_PI_4);
    assert!(close(outer.transform.a, rot.a) && close(outer.transform.b, rot.b));
    // The rotated box's pattern-space bounds span more tiles than the
    // axis-aligned 5 × 3 would.
    assert!(tiles.children.len() > 15, "{}", tiles.children.len());
    // Every point of the shape is covered by some tile.
    let inv = rot.invert().unwrap();
    for gx in 0..=10 {
        for gy in 0..=6 {
            let p = oxideav_svg::Point::new(50.0 + gx as f32 * 10.0, 20.0 + gy as f32 * 10.0);
            let q = inv.apply(p);
            let covered = (0..tiles.children.len()).any(|i| {
                let o = tile(tiles, i).0;
                // Tile origin in pattern space = rot⁻¹ ∘ transform applied
                // to (0,0).
                let origin = inv
                    .compose(&o.transform)
                    .apply(oxideav_svg::Point::new(0.0, 0.0));
                q.x >= origin.x - 1e-3
                    && q.x <= origin.x + 20.0 + 1e-3
                    && q.y >= origin.y - 1e-3
                    && q.y <= origin.y + 20.0 + 1e-3
            });
            assert!(covered, "shape point {p:?} is not covered by any tile");
        }
    }
    // The dot stays inside its tile, so no per-tile clip is needed (and
    // no anti-aliased seams appear between tiles).
    assert!(outer.clip.is_none());
}

#[test]
fn view_box_mid_alignment_centres_the_content() {
    // SVG 2 §14.3.2 / §8.2: `xMidYMid meet` of a 10 × 10 viewBox into a
    // 50 × 30 tile scales by 3 and offsets by (50 − 30) / 2 = 10. (The
    // round-480 black-box sweep found rsvg-convert 2.62 placing pattern
    // content at the full 20 for xMid — its nested-`<svg>` path centres
    // correctly — so this case is pinned here instead of in the pixel
    // gate.)
    let src = doc(
        r##"<pattern id="p" patternUnits="userSpaceOnUse" width="50" height="30" viewBox="0 0 10 10"><rect width="10" height="10"/></pattern>"##,
        r##"<rect width="200" height="100" fill="url(#p)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let (_, tiles) = pattern_layer(&frame, 0);
    let (_, inner) = tile(tiles, 0);
    assert_eq!(
        inner.transform,
        Transform2D::translate(10.0, 0.0).compose(&Transform2D::scale(3.0, 3.0))
    );
}

#[test]
fn stroke_pattern_masks_with_the_stroked_outline() {
    let src = doc(
        r##"<pattern id="p" patternUnits="userSpaceOnUse" width="10" height="10"><rect width="5" height="10" fill="#0000ff"/></pattern>"##,
        r##"<path d="M 20 50 L 180 50" fill="none" stroke="url(#p)" stroke-width="20" stroke-linecap="square"/>"##,
    );
    let frame = parse(&src).unwrap();
    let (mask, tiles) = pattern_layer(&frame, 0);
    assert!(mask.fill.is_none());
    let s = mask.stroke.as_ref().unwrap();
    assert_eq!(s.width, 20.0);
    assert_eq!(s.cap, oxideav_svg::LineCap::Square);
    assert!(matches!(s.paint, Paint::Solid(c) if c == Rgba::opaque(255, 255, 255)));
    // The tile cover reaches past the zero-height geometry box by the
    // stroke reach: tiles above and below y = 50 exist.
    let ys: Vec<f32> = (0..tiles.children.len())
        .map(|i| tile(tiles, i).0.transform.f)
        .collect();
    assert!(
        ys.iter().any(|&y| y < 40.0) && ys.iter().any(|&y| y >= 50.0),
        "{ys:?}"
    );
}

#[test]
fn zero_size_or_empty_pattern_paints_nothing_without_fallback() {
    // §14.3.1: width / height of zero disables rendering; the paint
    // server resolved, so the §13.2 fallback does not apply either.
    let src = doc(
        r##"<pattern id="z" patternUnits="userSpaceOnUse" width="0" height="10"><rect width="5" height="5"/></pattern>
            <pattern id="e" patternUnits="userSpaceOnUse" width="10" height="10"/>"##,
        r##"<rect width="50" height="50" fill="url(#z) red"/>
            <rect width="50" height="50" fill="url(#e)"/>"##,
    );
    let frame = parse(&src).unwrap();
    match &frame.root.children[0] {
        Node::Path(p) => assert!(p.fill.is_none() && p.stroke.is_none()),
        other => panic!("{other:?}"),
    }
    // Empty content: the layer group is built but the pattern layer
    // yields no node — the shape has no painted layer at all.
    match &frame.root.children[1] {
        Node::Group(g) => assert!(g.children.is_empty(), "{g:?}"),
        Node::Path(p) => assert!(p.fill.is_none()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn href_chain_inherits_attributes_and_content() {
    // The child specifies only a different tile size; units, transform
    // and content come from the template (§14.1.1 / §14.3.1).
    let src = doc(
        r##"<pattern id="t" patternUnits="userSpaceOnUse" width="10" height="10" patternTransform="translate(3 0)"><rect width="5" height="5" fill="#00ff00"/></pattern>
            <pattern id="c" href="#t" width="20"/>"##,
        r##"<rect width="100" height="100" fill="url(#c)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let (_, tiles) = pattern_layer(&frame, 0);
    let (outer, inner) = tile(tiles, 0);
    // 20 × 10 tiles (the child's width, the template's height), the
    // template's translate(3 0) and content.
    assert_eq!(inner.children.len(), 1);
    let first_col_x: f32 = outer.transform.e;
    assert!(
        close(first_col_x, 3.0 - 20.0) || close(first_col_x, 3.0),
        "{first_col_x}"
    );
    let (second, _) = tile(tiles, 1);
    assert!(close(second.transform.e - first_col_x, 20.0));
    let cols = (0..tiles.children.len())
        .filter(|&i| close(tile(tiles, i).0.transform.f, outer.transform.f))
        .count();
    let (below, _) = tile(tiles, cols);
    assert!(close(below.transform.f - outer.transform.f, 10.0));
}

#[test]
fn typed_chain_resolver_applies_defaults_and_percentages() {
    let mut defs = DefsTables::new();
    let tmpl = PatternDef::new()
        .with_tile(
            Length::new(10.0, LengthUnit::Percent),
            Length::user_units(0.0),
            Length::new(50.0, LengthUnit::Percent),
            Length::user_units(0.25),
        )
        .with_content(Group {
            children: vec![Node::Path(PathNode::new(Path::new()))],
            ..Group::default()
        });
    defs.patterns.insert("t".into(), tmpl);
    let child = PatternDef::new().with_href("t");
    let r = resolve_pattern_chain(&child, &defs);
    assert_eq!(r.pattern_units, PatternUnits::ObjectBoundingBox);
    assert_eq!(r.pattern_content_units, PatternUnits::UserSpaceOnUse);
    assert!(close(r.x, 0.1) && close(r.width, 0.5) && close(r.height, 0.25));
    assert_eq!(r.content.children.len(), 1);
    // userSpaceOnUse with a viewport: % against the axis.
    let child = PatternDef::new()
        .with_href("t")
        .with_pattern_units(PatternUnits::UserSpaceOnUse);
    let ctx = oxideav_svg::length::ResolveContext::default().with_viewport(200.0, 100.0);
    let r = oxideav_svg::defs::resolve_pattern_chain_with(&child, &defs, &ctx);
    assert!(close(r.x, 20.0) && close(r.width, 100.0) && close(r.height, 0.25));
}

#[test]
fn expansion_is_capped() {
    let pat = resolve_pattern_chain(
        &PatternDef::new()
            .with_pattern_units(PatternUnits::UserSpaceOnUse)
            .with_tile(
                Length::user_units(0.0),
                Length::user_units(0.0),
                Length::user_units(1.0),
                Length::user_units(1.0),
            )
            .with_content(Group {
                children: vec![Node::Path(PathNode::new(Path::new()))],
                ..Group::default()
            }),
        &DefsTables::new(),
    );
    let mut budget = usize::MAX;
    let tiles =
        expand_pattern(&pat, None, BBox::new(0.0, 0.0, 1000.0, 1000.0), &mut budget).unwrap();
    assert_eq!(tiles.children.len(), MAX_PATTERN_TILES);
    // A strip keeps its full length within the cap.
    let mut budget = usize::MAX;
    let strip = expand_pattern(&pat, None, BBox::new(0.0, 0.0, 1000.0, 1.0), &mut budget).unwrap();
    assert_eq!(strip.children.len(), 1000);
    let mut budget = 10;
    let tiles =
        expand_pattern(&pat, None, BBox::new(0.0, 0.0, 1000.0, 1000.0), &mut budget).unwrap();
    assert!(tiles.children.len() <= 10 && tiles.children.len() >= 9);
    assert_eq!(budget, 10 - tiles.children.len());
    let mut budget = 0;
    assert!(expand_pattern(&pat, None, BBox::new(0.0, 0.0, 10.0, 10.0), &mut budget).is_none());
}

#[test]
fn fill_opacity_applies_to_the_tile_group() {
    let src = doc(
        r##"<pattern id="p" patternUnits="userSpaceOnUse" width="10" height="10"><rect width="5" height="5"/></pattern>"##,
        r##"<rect width="50" height="50" fill="url(#p)" fill-opacity="0.5"/>"##,
    );
    let frame = parse(&src).unwrap();
    let (_, tiles) = pattern_layer(&frame, 0);
    assert!((tiles.opacity - 0.5).abs() < 1e-6);
}

#[test]
fn writer_output_round_trips_the_expansion() {
    let src = doc(
        r##"<pattern id="p" patternUnits="userSpaceOnUse" width="10" height="10"><rect width="15" height="5" fill="#ff0000"/></pattern>"##,
        r##"<rect x="10" y="10" width="30" height="30" fill="url(#p)"/>"##,
    );
    let frame = parse(&src).unwrap();
    let out = write(&frame);
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("<mask"), "{s}");
    assert!(s.contains("<clipPath"), "{s}");
    assert_eq!(s.matches("clip-path=\"url(#clip1)\"").count(), 9, "{s}");
    let again = write(&parse(&out).unwrap());
    assert_eq!(s, std::str::from_utf8(&again).unwrap());
}

// ---------------------------------------------------------------------
// Black-box pixel truth against rsvg-convert: the writer's expansion
// (masks + clipped tile groups) must render like the source pattern.
// ---------------------------------------------------------------------

fn tool_available(name: &str, arg: &str) -> bool {
    Command::new(name)
        .arg(arg)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("oxideav-svg-r480p-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn rsvg_rgba(svg: &[u8], tag: &str) -> Vec<u8> {
    let svg_path = scratch(&format!("{tag}.svg"));
    let png_path = scratch(&format!("{tag}.png"));
    std::fs::write(&svg_path, svg).unwrap();
    let st = Command::new("rsvg-convert")
        .args(["-w", &W.to_string(), "-h", &H.to_string(), "-o"])
        .arg(&png_path)
        .arg(&svg_path)
        .status()
        .expect("run rsvg-convert");
    assert!(st.success(), "rsvg-convert failed on {tag}");
    let out = Command::new("magick")
        .arg(format!("png:{}", png_path.display()))
        .args(["-depth", "8", "rgba:-"])
        .output()
        .expect("run magick");
    assert!(out.status.success(), "magick failed on {tag}");
    assert_eq!(out.stdout.len(), (W * H * 4) as usize, "raw size for {tag}");
    out.stdout
}

fn compare(a: &[u8], b: &[u8]) -> (f64, f64) {
    let mut sum = 0.0_f64;
    let mut bad = 0usize;
    let n = (W * H) as usize;
    for i in 0..n {
        let pa = &a[i * 4..i * 4 + 4];
        let pb = &b[i * 4..i * 4 + 4];
        let mut worst = 0i32;
        for c in 0..4 {
            let va = pa[c] as i32 * if c < 3 { pa[3] as i32 } else { 255 } / 255;
            let vb = pb[c] as i32 * if c < 3 { pb[3] as i32 } else { 255 } / 255;
            let d = (va - vb).abs();
            sum += d as f64;
            worst = worst.max(d);
        }
        if worst > 24 {
            bad += 1;
        }
    }
    (sum / (n * 4) as f64, bad as f64 / n as f64)
}

fn fixtures() -> Vec<(&'static str, String, String)> {
    vec![
        (
            "usu_dots",
            r##"<pattern id="p" patternUnits="userSpaceOnUse" width="20" height="20"><circle cx="10" cy="10" r="6" fill="#204060"/></pattern>"##.into(),
            r##"<rect x="15" y="10" width="170" height="80" fill="url(#p)"/>"##.into(),
        ),
        (
            "usu_offset_tile_content_crossing",
            r##"<pattern id="p" patternUnits="userSpaceOnUse" x="7" y="3" width="24" height="16"><rect x="-4" y="4" width="16" height="8" fill="#c03030"/><rect x="12" y="0" width="12" height="16" fill="#3060c0" fill-opacity="0.5"/></pattern>"##.into(),
            r##"<rect x="10" y="10" width="180" height="80" fill="url(#p)"/>"##.into(),
        ),
        (
            "obb_tiles_on_ellipse",
            r##"<pattern id="p" width="0.2" height="0.25"><rect width="18" height="10" fill="#ff8800"/></pattern>"##.into(),
            r##"<ellipse cx="100" cy="50" rx="90" ry="40" fill="url(#p)"/>"##.into(),
        ),
        (
            "obb_content_units",
            r##"<pattern id="p" width="0.25" height="0.5" patternContentUnits="objectBoundingBox"><rect width="0.125" height="0.25" fill="#008800"/><circle cx="0.18" cy="0.35" r="0.05" fill="#880088"/></pattern>"##.into(),
            r##"<rect x="20" y="10" width="160" height="80" fill="url(#p)"/>"##.into(),
        ),
        (
            "viewbox_meet_square_spec_example",
            r##"<pattern id="p" patternUnits="userSpaceOnUse" width="50" height="50" viewBox="0 0 10 10"><path d="M 0 0 L 7 0 L 3.5 7 z" fill="plum" stroke="blue"/></pattern>"##.into(),
            r##"<ellipse cx="100" cy="50" rx="95" ry="45" fill="url(#p)" stroke="black" stroke-width="2"/>"##.into(),
        ),
        (
            "viewbox_meet_xmin",
            r##"<pattern id="p" patternUnits="userSpaceOnUse" width="50" height="30" viewBox="0 0 10 10" preserveAspectRatio="xMinYMid meet"><path d="M 0 0 L 7 0 L 3.5 7 z" fill="plum" stroke="blue"/></pattern>"##.into(),
            r##"<rect x="0" y="0" width="200" height="100" fill="url(#p)"/>"##.into(),
        ),
        (
            "viewbox_none_slice_align",
            r##"<pattern id="p" patternUnits="userSpaceOnUse" width="40" height="20" viewBox="0 0 10 10" preserveAspectRatio="xMaxYMin slice"><rect width="5" height="5" fill="#ff0000"/><rect x="5" y="5" width="5" height="5" fill="#0000ff"/></pattern>"##.into(),
            r##"<rect x="0" y="0" width="200" height="100" fill="url(#p)"/>"##.into(),
        ),
        (
            "pattern_transform_rotate_scale",
            r##"<pattern id="p" patternUnits="userSpaceOnUse" width="16" height="16" patternTransform="rotate(30) scale(1.5)"><circle cx="8" cy="8" r="5" fill="#333333"/></pattern>"##.into(),
            r##"<rect x="20" y="10" width="160" height="80" fill="url(#p)"/>"##.into(),
        ),
        (
            "stroke_pattern",
            r##"<pattern id="p" patternUnits="userSpaceOnUse" width="10" height="10"><rect width="5" height="10" fill="#0000ff"/></pattern>"##.into(),
            r##"<path d="M 20 20 C 60 90 140 10 180 80" fill="none" stroke="url(#p)" stroke-width="16" stroke-linecap="round"/>"##.into(),
        ),
        (
            "href_chain_and_gradient_content",
            r##"<linearGradient id="g" x2="1" y2="0"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient>
                <pattern id="t" patternUnits="userSpaceOnUse" width="25" height="25"><rect x="2" y="2" width="21" height="21" fill="url(#g)"/></pattern>
                <pattern id="p" href="#t" patternTransform="translate(5 5)"/>"##.into(),
            r##"<rect x="10" y="10" width="180" height="80" fill="url(#p)"/>"##.into(),
        ),
        (
            "fill_opacity_with_fallback_syntax",
            r##"<pattern id="p" patternUnits="userSpaceOnUse" width="20" height="20"><rect width="10" height="20" fill="#000000"/></pattern>"##.into(),
            r##"<rect width="200" height="100" fill="#ffffff"/><rect x="20" y="10" width="160" height="80" fill="url(#p) red" fill-opacity="0.5"/>"##.into(),
        ),
        (
            "transformed_shape_with_pattern",
            r##"<pattern id="p" width="0.25" height="0.25"><rect width="10" height="10" fill="#aa0000"/></pattern>"##.into(),
            r##"<g transform="translate(100 50) rotate(15)"><rect x="-70" y="-30" width="140" height="60" fill="url(#p)" stroke="#000000" stroke-width="2"/></g>"##.into(),
        ),
    ]
}

#[test]
fn rewritten_pattern_documents_render_like_their_source_in_rsvg() {
    if !tool_available("rsvg-convert", "--version") || !tool_available("magick", "-version") {
        eprintln!("rsvg-convert / magick not installed — black-box pixel check skipped");
        return;
    }
    let mut failures = Vec::new();
    for (name, defs, body) in fixtures() {
        let src = doc(&defs, &body);
        let frame = parse(&src).expect(name);
        let rewritten = write(&frame);
        let a = rsvg_rgba(&src, &format!("{name}-src"));
        let b = rsvg_rgba(&rewritten, &format!("{name}-out"));
        let (mean, bad) = compare(&a, &b);
        eprintln!(
            "{name}: mean diff {mean:.3}, pixels off by >24: {:.3}%",
            bad * 100.0
        );
        // rsvg renders a rotated / scaled pattern by resampling a
        // rasterised tile surface (softer dot edges); our expansion
        // draws every tile as vectors, so that fixture gets more
        // edge-pixel room.
        let (max_mean, max_bad) = if name.starts_with("pattern_transform") {
            (1.2, 0.04)
        } else {
            (0.8, 0.006)
        };
        if mean > max_mean || bad > max_bad {
            failures.push(format!("{name}: mean {mean:.3} bad {bad:.4}"));
        }
        // The source must actually paint something (guards against a
        // blank-vs-blank pass).
        let painted = a.chunks(4).filter(|p| p[3] > 0).count();
        assert!(painted > 500, "{name}: source renders blank");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn pattern_pixels_land_where_the_tiles_say() {
    if !tool_available("rsvg-convert", "--version") || !tool_available("magick", "-version") {
        eprintln!("rsvg-convert / magick not installed — black-box pixel check skipped");
        return;
    }
    // userSpaceOnUse 20 × 20 tiles with a 10 × 20 black bar on the left
    // half of each tile, over a white background, on a rect from x = 20:
    // x = 25 is in a bar, x = 35 is not, both in rsvg and in our
    // expansion.
    let src = doc(
        r##"<pattern id="p" patternUnits="userSpaceOnUse" width="20" height="20"><rect width="10" height="20" fill="#000000"/></pattern>"##,
        r##"<rect width="200" height="100" fill="#ffffff"/><rect x="20" y="10" width="160" height="80" fill="url(#p)"/>"##,
    );
    let frame = parse(&src).unwrap();
    for (tag, bytes) in [("src", src.clone()), ("out", write(&frame))] {
        let px = rsvg_rgba(&bytes, &format!("bars-{tag}"));
        let at = |x: u32, y: u32| {
            let i = ((y * W + x) * 4) as usize;
            px[i]
        };
        assert!(at(25, 50) < 10, "{tag}: bar at x=25 → {}", at(25, 50));
        assert!(at(35, 50) > 245, "{tag}: gap at x=35 → {}", at(35, 50));
        assert!(at(45, 50) < 10, "{tag}: bar at x=45 → {}", at(45, 50));
        assert!(
            at(10, 50) > 245,
            "{tag}: outside the shape → {}",
            at(10, 50)
        );
    }
}
