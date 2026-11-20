//! Pipeline benchmark: parse → style cascade → layout → raster
//! hand-off over four representative document sets (icons, a large
//! map-like path document, a text-heavy document, a filter-heavy one),
//! plus the in-crate filter evaluators at 1× and 4× scale.
//!
//! Dependency-free (`harness = false`): each stage is timed with
//! `std::time::Instant` over several runs and the median reported.
//!
//! ```text
//! cargo bench --bench pipeline            # the table
//! cargo bench --bench pipeline -- map 20  # one set, 20 runs
//! ```
//!
//! Stages:
//! - `xml`      — the XML tokeniser + tree builder alone;
//! - `parse`    — `oxideav_svg::parse`: XML, cascade, geometry, text
//!   layout, scene graph;
//! - `hand-off` — `VectorFrame::from(&doc)`: the scene handed to a
//!   rasteriser;
//! - `write`    — `oxideav_svg::write`.

use std::fmt::Write as _;
use std::hint::black_box;
use std::time::Instant;

/// Deterministic 64-bit LCG (multiplier from Knuth's MMIX).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.next() % 10_000) as f32 / 10_000.0 * (hi - lo)
    }
}

/// `n` stroke icons in the common "everything on the root" idiom.
fn icons(n: usize) -> Vec<String> {
    let mut rng = Rng(7);
    (0..n)
        .map(|i| {
            let mut s = String::from(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"##,
            );
            for k in 0..(4 + i % 5) {
                let (a, b, c, d) = (
                    rng.range(2.0, 22.0),
                    rng.range(2.0, 22.0),
                    rng.range(1.0, 8.0),
                    rng.range(1.0, 8.0),
                );
                match k % 4 {
                    0 => write!(s, r#"<path d="M{a:.1} {b:.1}c{c:.1} 0 {d:.1} {c:.1} {d:.1} {d:.1}s-{c:.1} {d:.1}-{d:.1} {c:.1}a{c:.1} {c:.1} 0 0 1 {d:.1}-{c:.1}z"/>"#),
                    1 => write!(s, r#"<circle cx="{a:.1}" cy="{b:.1}" r="{c:.1}"/>"#),
                    2 => write!(s, r#"<g transform="rotate({d:.0} 12 12)"><rect x="{a:.1}" y="{b:.1}" width="{c:.1}" height="{d:.1}" rx="1"/></g>"#),
                    _ => write!(s, r#"<polyline points="{a:.1},{b:.1} {c:.1},{d:.1} {b:.1},{a:.1}"/><line x1="{a:.1}" y1="{c:.1}" x2="{b:.1}" y2="{d:.1}" style="stroke-width:1.5"/>"#),
                }
                .unwrap();
            }
            s.push_str("</svg>");
            s
        })
        .collect()
}

/// A map-like document: a stylesheet of class rules, layer groups and
/// many long paths, plus `<use>`d point symbols.
fn map(layers: usize, paths_per_layer: usize) -> String {
    let mut rng = Rng(11);
    let mut s = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="768" viewBox="0 0 4096 3072"><style>"##,
    );
    for c in 0..24 {
        write!(
            s,
            ".c{c}{{fill:#{:06x};stroke:#{:06x};stroke-width:{}}} g.l{c} > path.minor{{stroke-dasharray:4 2}} ",
            rng.next() & 0xffffff,
            rng.next() & 0xffffff,
            1 + c % 4
        )
        .unwrap();
    }
    s.push_str(
        ".water{fill:#9cf;stroke:none} .road{fill:none;stroke:#555;stroke-linejoin:round}</style>",
    );
    s.push_str(r##"<defs><symbol id="poi" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4" fill="#c33"/><path d="M5 2v6M2 5h6" stroke="#fff"/></symbol></defs>"##);
    for l in 0..layers {
        write!(s, r#"<g class="l{} layer" id="layer{l}">"#, l % 24).unwrap();
        for p in 0..paths_per_layer {
            let class = match p % 5 {
                0 => "water".to_string(),
                1 => "road minor".to_string(),
                _ => format!("c{}", (l + p) % 24),
            };
            write!(
                s,
                r#"<path class="{class}" d="M{:.1} {:.1}"#,
                rng.range(0.0, 4096.0),
                rng.range(0.0, 3072.0)
            )
            .unwrap();
            for k in 0..36 {
                if k % 6 == 5 {
                    write!(
                        s,
                        "c{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}",
                        rng.range(-9.0, 9.0),
                        rng.range(-9.0, 9.0),
                        rng.range(-9.0, 9.0),
                        rng.range(-9.0, 9.0),
                        rng.range(-9.0, 9.0),
                        rng.range(-9.0, 9.0)
                    )
                    .unwrap();
                } else {
                    write!(
                        s,
                        "l{:.1} {:.1}",
                        rng.range(-12.0, 12.0),
                        rng.range(-12.0, 12.0)
                    )
                    .unwrap();
                }
            }
            s.push_str(if p % 5 == 1 { "\"/>" } else { "z\"/>" });
        }
        for _ in 0..4 {
            write!(
                s,
                r##"<use href="#poi" x="{:.0}" y="{:.0}" width="24" height="24"/>"##,
                rng.range(0.0, 4000.0),
                rng.range(0.0, 3000.0)
            )
            .unwrap();
        }
        s.push_str("</g>");
    }
    s.push_str("</svg>");
    s
}

/// A text-heavy document: `rows` lines of styled, positioned text.
fn text(rows: usize) -> String {
    let words = [
        "vector", "graphics", "render", "the", "quick", "brown", "glyph", "outline", "baseline",
        "anchor", "kerning", "of", "a", "path",
    ];
    let mut rng = Rng(13);
    let mut s = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="1400" font-family="Bench Mono" font-size="12"><style>.h{font-size:18px;font-weight:bold} .em{fill:#06c;text-decoration:underline} tspan.sup{baseline-shift:super;font-size:8px}</style><defs><linearGradient id="g"><stop offset="0" stop-color="#c00"/><stop offset="1" stop-color="#00c"/></linearGradient></defs>"##,
    );
    for r in 0..rows {
        let y = 16 + r * 14;
        let anchor = ["start", "middle", "end"][r % 3];
        let x = [10, 500, 990][r % 3];
        write!(s, r#"<text x="{x}" y="{y}" text-anchor="{anchor}""#).unwrap();
        if r % 20 == 0 {
            s.push_str(r#" class="h""#);
        }
        if r % 50 == 7 {
            s.push_str(r##" fill="url(#g)""##);
        }
        s.push('>');
        for w in 0..12 {
            let word = words[(rng.next() as usize) % words.len()];
            match w % 6 {
                2 => write!(s, r#"<tspan class="em">{word}</tspan> "#),
                4 => write!(s, r#"{word}<tspan class="sup">{w}</tspan> "#),
                _ => write!(s, "{word} "),
            }
            .unwrap();
        }
        s.push_str("</text>");
    }
    s.push_str("</svg>");
    s
}

/// A filter-heavy document: many filtered elements over a set of
/// multi-primitive filters.
fn filters(n: usize) -> String {
    let mut s = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="800" height="600"><defs>"##,
    );
    for f in 0..12 {
        write!(
            s,
            r##"<filter id="f{f}" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur in="SourceAlpha" stdDeviation="{}" result="b"/><feOffset in="b" dx="{}" dy="{}" result="o"/><feFlood flood-color="#036" flood-opacity="0.5" result="c"/><feComposite in="c" in2="o" operator="in" result="s"/><feColorMatrix in="SourceGraphic" type="saturate" values="0.{}" result="m"/><feMerge><feMergeNode in="s"/><feMergeNode in="m"/></feMerge></filter>"##,
            1 + f % 4,
            f % 5,
            1 + f % 3,
            3 + f % 6
        )
        .unwrap();
    }
    s.push_str("</defs>");
    let mut rng = Rng(17);
    for i in 0..n {
        write!(
            s,
            r##"<g filter="url(#f{})"><rect x="{:.0}" y="{:.0}" width="60" height="40" rx="6" fill="#{:06x}"/><circle cx="{:.0}" cy="{:.0}" r="12" fill="#fff"/></g>"##,
            i % 12,
            rng.range(0.0, 740.0),
            rng.range(0.0, 560.0),
            rng.next() & 0xffffff,
            rng.range(0.0, 800.0),
            rng.range(0.0, 600.0)
        )
        .unwrap();
    }
    s.push_str("</svg>");
    s
}

fn median_ms(runs: usize, mut f: impl FnMut()) -> f64 {
    let mut t: Vec<f64> = (0..runs)
        .map(|_| {
            let t0 = Instant::now();
            f();
            t0.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    t[t.len() / 2]
}

fn stages(name: &str, docs: &[String], runs: usize) {
    let bytes: usize = docs.iter().map(String::len).sum();
    // `OXIDEAV_SVG_BENCH_DUMP=<dir>` writes the generated documents
    // out (to feed a rasteriser at 1× / 4×) instead of timing them.
    if let Ok(dir) = std::env::var("OXIDEAV_SVG_BENCH_DUMP") {
        for (i, d) in docs.iter().take(8).enumerate() {
            let path = format!("{dir}/{name}-{i}.svg");
            std::fs::write(&path, d).unwrap_or_else(|e| panic!("{path}: {e}"));
        }
        return;
    }
    // `OXIDEAV_SVG_BENCH_STAGE=parse` loops the parse stage alone, for
    // a profiler to attach to.
    if std::env::var("OXIDEAV_SVG_BENCH_STAGE").as_deref() == Ok("parse") {
        let parse = median_ms(runs, || {
            for d in docs {
                black_box(oxideav_svg::parse(d.as_bytes()).unwrap());
            }
        });
        println!("{name:<10} parse {parse:>9.3} ms");
        return;
    }
    let xml = median_ms(runs, || {
        for d in docs {
            black_box(oxideav_svg::parser::parse_xml(d).unwrap());
        }
    });
    let parse = median_ms(runs, || {
        for d in docs {
            black_box(oxideav_svg::parse(d.as_bytes()).unwrap());
        }
    });
    let parsed: Vec<oxideav_svg::SvgDocument> = docs
        .iter()
        .map(|d| oxideav_svg::parse(d.as_bytes()).unwrap())
        .collect();
    #[cfg(feature = "registry")]
    let handoff = median_ms(runs, || {
        for d in &parsed {
            black_box(oxideav_core::VectorFrame::from(d));
        }
    });
    #[cfg(not(feature = "registry"))]
    let handoff = f64::NAN;
    let write = median_ms(runs, || {
        for d in &parsed {
            black_box(oxideav_svg::write(d));
        }
    });
    println!(
        "{name:<10} {:>4} doc {:>9} B | xml {xml:>9.3} ms | parse {parse:>9.3} ms | hand-off {handoff:>8.3} ms | write {write:>8.3} ms",
        docs.len(),
        bytes
    );
}

/// The filter evaluators on a synthetic source at `side × side`, serial
/// (the default) and with a caller-granted budget of four threads.
fn filter_eval(side: usize, runs: usize) {
    use oxideav_svg::filter_eval::{
        FilterSources, evaluate_filter_graph, evaluate_filter_graph_threads,
    };
    let src = format!(
        r##"<filter id="f"><feGaussianBlur in="SourceAlpha" stdDeviation="{0}" result="b"/><feOffset in="b" dx="{0}" dy="{0}" result="o"/><feFlood flood-color="#036" flood-opacity="0.5" result="c"/><feComposite in="c" in2="o" operator="in" result="s"/><feColorMatrix in="SourceGraphic" type="hueRotate" values="90" result="m"/><feMorphology in="m" operator="dilate" radius="{1}" result="d"/><feComponentTransfer in="d" result="t"><feFuncR type="gamma" exponent="1.5"/><feFuncA type="linear" slope="0.9"/></feComponentTransfer><feMerge><feMergeNode in="s"/><feMergeNode in="t"/></feMerge></filter>"##,
        side as f32 / 64.0,
        (side / 128).max(1)
    );
    let nodes = oxideav_svg::parser::parse_xml(&src).unwrap();
    let filter = oxideav_svg::parser::find_element(&nodes, "filter").unwrap();
    let graph = oxideav_svg::filter::parse_filter_graph(filter);
    // A disc on a transparent ground.
    let mut rgba = vec![0u8; side * side * 4];
    let (c, r) = (side as f32 / 2.0, side as f32 / 3.0);
    for y in 0..side {
        for x in 0..side {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            if dx * dx + dy * dy < r * r {
                rgba[(y * side + x) * 4..][..4].copy_from_slice(&[200, 80, 40, 255]);
            }
        }
    }
    let sources = FilterSources::from_source_graphic(rgba);
    let ms = median_ms(runs, || {
        black_box(evaluate_filter_graph(&graph, &sources, side, side));
    });
    let ms4 = median_ms(runs, || {
        black_box(evaluate_filter_graph_threads(
            &graph,
            &sources,
            side,
            side,
            &[],
            4,
        ));
    });
    println!(
        "filter-eval {side:>4} x {side:<4} (8 primitives) | {ms:>9.3} ms | 4 threads {ms4:>9.3} ms"
    );
}

fn main() {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .collect();
    let only = args
        .first()
        .map(String::as_str)
        .filter(|a| a.parse::<usize>().is_err());
    let runs: usize = args.iter().find_map(|a| a.parse().ok()).unwrap_or(9);

    #[cfg(feature = "text")]
    {
        use std::sync::Arc;
        let font = include_bytes!("../tests/fixtures/DejaVuSansMono.ttf");
        #[allow(clippy::arc_with_non_send_sync)]
        let chain = Arc::new(oxideav_scribe::FaceChain::new(
            oxideav_scribe::Face::from_ttf_bytes(font.to_vec()).expect("fixture font"),
        ));
        let _ = oxideav_svg::text::set_font_provider(move |_| Some(chain.clone()));
    }

    let want = |n: &str| only.is_none_or(|o| o == n);
    if want("icons") {
        stages("icons", &icons(200), runs);
    }
    if want("map") {
        stages("map", &[map(120, 150)], runs);
    }
    if want("text") {
        stages("text", &[text(100)], runs);
    }
    if want("filters") {
        stages("filters", &[filters(400)], runs);
    }
    if want("filter-eval") {
        // `OXIDEAV_SVG_BENCH_FILTER_SIDE=1024` loops one size, for a
        // profiler to attach to.
        match std::env::var("OXIDEAV_SVG_BENCH_FILTER_SIDE")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            Some(side) => filter_eval(side, runs),
            None => {
                filter_eval(256, runs);
                filter_eval(1024, runs.min(5));
            }
        }
    }
}
