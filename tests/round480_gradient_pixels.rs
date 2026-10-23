//! Round 480 — black-box pixel truth for SVG 2 §8.11 / §14.2 gradient
//! binding (issue #13), against `rsvg-convert`.
//!
//! The crate's writer re-emits every gradient in user space
//! (`gradientUnits="userSpaceOnUse"` with the geometry the scene graph
//! carries), so `write(parse(x))` is an SVG whose paint servers no
//! longer depend on the referencing shape's bounding box. If the §8.11
//! binding is right, an independent renderer paints that document
//! pixel-for-pixel like the original. Each fixture below is rendered
//! twice by `rsvg-convert` — source and re-written — and compared with
//! a tolerance that only absorbs edge anti-aliasing (the masked
//! representation draws the same outline through a `<mask>`).
//!
//! ImageMagick is used purely to unpack the PNGs into raw RGBA (its own
//! `MSVG` renderer does not implement gradients, so it is not a
//! rendering oracle here). The test skips (passing) when either tool
//! is not installed.

use std::process::Command;

use oxideav_svg::{parse, write};

const W: u32 = 200;
const H: u32 = 100;

fn tool_available(name: &str, arg: &str) -> bool {
    Command::new(name)
        .arg(arg)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("oxideav-svg-r480-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Render `svg` with rsvg-convert at W×H and return straight RGBA bytes.
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

/// Compare two RGBA buffers: mean absolute channel difference (over
/// the whole image, alpha-weighted so transparent pixels compare
/// equal) and the fraction of pixels whose worst channel differs by
/// more than `edge` levels.
fn compare(a: &[u8], b: &[u8]) -> (f64, f64) {
    let mut sum = 0.0_f64;
    let mut bad = 0usize;
    let n = (W * H) as usize;
    for i in 0..n {
        let pa = &a[i * 4..i * 4 + 4];
        let pb = &b[i * 4..i * 4 + 4];
        let mut worst = 0i32;
        for c in 0..4 {
            // Premultiply so a fully transparent pixel's colour is moot.
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

fn doc(defs: &str, body: &str) -> Vec<u8> {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}"><defs>{defs}</defs>{body}</svg>"##
    )
    .into_bytes()
}

const STOPS: &str = r##"<stop offset="0" stop-color="#ff0000"/><stop offset="0.5" stop-color="#00ff00"/><stop offset="1" stop-color="#0000ff"/>"##;

/// Every fixture: (name, defs, body, max mean diff, max bad fraction).
fn fixtures() -> Vec<(&'static str, String, String)> {
    vec![
        (
            "obb_linear_default",
            format!(r##"<linearGradient id="g">{STOPS}</linearGradient>"##),
            r##"<rect x="20" y="10" width="160" height="80" fill="url(#g)"/>"##.into(),
        ),
        (
            "obb_linear_percent_vertical",
            format!(
                r##"<linearGradient id="g" x1="0" y1="10%" x2="0" y2="90%">{STOPS}</linearGradient>"##
            ),
            r##"<rect x="20" y="10" width="160" height="80" fill="url(#g)"/>"##.into(),
        ),
        (
            "obb_linear_diagonal_non_square",
            format!(r##"<linearGradient id="g" x2="1" y2="1">{STOPS}</linearGradient>"##),
            r##"<rect x="10" y="20" width="180" height="60" fill="url(#g)"/>"##.into(),
        ),
        (
            "obb_linear_gradient_transform_reflect",
            format!(
                r##"<linearGradient id="g" x1="0.4" x2="0.6" spreadMethod="reflect" gradientTransform="rotate(30 0.5 0.5)">{STOPS}</linearGradient>"##
            ),
            r##"<rect x="10" y="10" width="180" height="80" fill="url(#g)"/>"##.into(),
        ),
        (
            "obb_linear_on_transformed_shapes",
            format!(r##"<linearGradient id="g">{STOPS}</linearGradient>"##),
            r##"<g transform="translate(100 50) rotate(20) scale(0.8)"><rect x="-60" y="-30" width="120" height="60" fill="url(#g)"/></g>
                <polygon points="10,90 60,60 40,95" fill="url(#g)" transform="translate(5 0)"/>"##
                .into(),
        ),
        (
            "obb_linear_stroke_round_caps",
            format!(r##"<linearGradient id="g" x1="0" y1="0" x2="1" y2="0">{STOPS}</linearGradient>"##),
            r##"<path d="M 20 20 C 60 90 140 10 180 80" fill="none" stroke="url(#g)" stroke-width="14" stroke-linecap="round"/>"##
                .into(),
        ),
        (
            "obb_linear_via_use",
            format!(
                r##"<linearGradient id="g">{STOPS}</linearGradient><rect id="r" width="60" height="30" fill="url(#g)"/>"##
            ),
            r##"<use href="#r" x="10" y="10"/><use href="#r" x="120" y="60"/>"##.into(),
        ),
        (
            "obb_radial_circle_focal_repeat",
            format!(
                r##"<radialGradient id="g" r="0.3" fx="0.3" fy="0.35" spreadMethod="repeat">{STOPS}</radialGradient>"##
            ),
            r##"<circle cx="100" cy="50" r="45" fill="url(#g)"/>"##.into(),
        ),
        (
            "obb_radial_non_square_rect",
            format!(r##"<radialGradient id="g">{STOPS}</radialGradient>"##),
            r##"<rect x="10" y="10" width="180" height="80" fill="url(#g)"/>"##.into(),
        ),
        (
            "obb_radial_ellipse_with_stroke",
            format!(r##"<radialGradient id="g" fx="0.35">{STOPS}</radialGradient>"##),
            r##"<ellipse cx="100" cy="50" rx="80" ry="35" fill="url(#g)" stroke="#000000" stroke-width="3"/>"##
                .into(),
        ),
        (
            "usu_radial_skewed_stroke",
            format!(
                r##"<radialGradient id="g" gradientUnits="userSpaceOnUse" cx="100" cy="50" r="40" gradientTransform="matrix(2 0.3 0 1 -100 0)">{STOPS}</radialGradient>"##
            ),
            r##"<path d="M 20 50 L 180 50" fill="none" stroke="url(#g)" stroke-width="30"/>"##
                .into(),
        ),
        (
            "usu_percentages_and_href_chain",
            format!(
                r##"<linearGradient id="t" gradientUnits="userSpaceOnUse" x1="10%" x2="90%">{STOPS}</linearGradient>
                    <linearGradient id="g" href="#t" spreadMethod="repeat" x2="50%"/>"##
            ),
            r##"<rect x="0" y="0" width="200" height="100" fill="url(#g)"/>"##.into(),
        ),
        (
            "obb_fill_opacity_over_background",
            format!(r##"<linearGradient id="g">{STOPS}</linearGradient>"##),
            r##"<rect width="200" height="100" fill="#ffffff"/><rect x="20" y="10" width="160" height="80" fill="url(#g)" fill-opacity="0.4"/>"##
                .into(),
        ),
        (
            "obb_zero_height_line_paints_nothing",
            format!(r##"<linearGradient id="g">{STOPS}</linearGradient>"##),
            r##"<line x1="20" y1="50" x2="180" y2="50" stroke="url(#g)" stroke-width="20"/>"##
                .into(),
        ),
    ]
}

#[test]
fn rewritten_documents_render_like_their_source_in_rsvg() {
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
        if mean > 0.6 || bad > 0.004 {
            failures.push(format!(
                "{name}: mean {mean:.3} bad {bad:.4}\n{}",
                String::from_utf8_lossy(&rewritten)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn source_renders_are_not_trivially_blank() {
    // Guard against the comparison passing because rsvg painted
    // nothing on both sides: the default-units fixture must show the
    // first and last stop colours at the shape's left and right edges.
    if !tool_available("rsvg-convert", "--version") || !tool_available("magick", "-version") {
        eprintln!("rsvg-convert / magick not installed — black-box pixel check skipped");
        return;
    }
    let (_, defs, body) = fixtures().into_iter().next().unwrap();
    let frame = parse(&doc(&defs, &body)).unwrap();
    let px = rsvg_rgba(&write(&frame), "blank-guard");
    let at = |x: u32, y: u32| {
        let i = ((y * W + x) * 4) as usize;
        (px[i], px[i + 1], px[i + 2], px[i + 3])
    };
    let (r, _, b, a) = at(22, 50);
    assert!(a == 255 && r > 240 && b < 15, "left edge {:?}", at(22, 50));
    let (r, _, b, a) = at(177, 50);
    assert!(
        a == 255 && b > 240 && r < 15,
        "right edge {:?}",
        at(177, 50)
    );
    let (_, g, _, _) = at(100, 50);
    assert!(g > 240, "middle stop {:?}", at(100, 50));
}
