//! Round 449 — write-side conformance gate: read → write → read
//! fixed-point invariants over a per-feature corpus
//! (`tests/fixtures/corpus/*.svg`).
//!
//! For every corpus document `x` the gate enforces:
//!
//! 1. **Parse** — `parse_svg_with_extras(x)` succeeds, and so does a
//!    re-parse of the writer's output.
//! 2. **Byte fixed point** — `write(parse(write(parse(x)))) ==
//!    write(parse(x))`: the writer's output is *immediately* stable
//!    under its own round-trip. (This is the invariant that caught the
//!    round-449 fixes: the §9.6.1 dash-rescale compounding, the
//!    `<a><g>` and `<g mask>` wrapper growth, and the
//!    `<foreignObject>` placeholder accumulation.)
//! 3. **Element census** — for a fixed list of semantically countable
//!    tags, the writer's output carries exactly as many occurrences as
//!    the source: nothing is lost, nothing is duplicated. (This is the
//!    invariant that caught the orphaned-gradient twin emission.)
//! 4. **Scene equivalence** — flattening the source and the written
//!    output through the extras-free pipeline
//!    (`write_svg(parse_svg(·))`) yields byte-identical scene
//!    serialisations, so the round-tripped document resolves to the
//!    same rendered geometry.
//!
//! Documented exemption: `hints.svg` exercises the §13.8 stroke-first
//! `paint-order` split, whose shape deliberately declines native
//! identity (two single-purpose `<path>`s replace one `<rect>`), so
//! shape tags are exempt from its census.

use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg, write_svg_with_extras, write_svgz};

fn rt(src: &[u8]) -> Vec<u8> {
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    write_svg_with_extras(&frame, &extras)
}

/// Tags whose occurrence count must survive a round-trip exactly.
/// Structural containers the encoder legitimately reshapes (`<g>`,
/// `<defs>`, `<path>`) are not counted.
const CENSUS: &[&str] = &[
    "<animate",
    "<set",
    "<animateTransform",
    "<animateMotion",
    "<tspan",
    "<textPath",
    "<pattern",
    "<marker ",
    "<linearGradient",
    "<radialGradient",
    "<filter",
    "<clipPath",
    "<mask",
    "<symbol",
    "<use",
    "<switch",
    "<view",
    "<image",
    "<text",
    "<foreignObject",
    "<script",
    "<metadata",
    "<title",
    "<desc",
    "<a ",
];

/// Shape tags — counted for every corpus doc except the ones that
/// exercise a documented native-identity decline.
const SHAPE_CENSUS: &[&str] = &[
    "<rect",
    "<circle",
    "<ellipse",
    "<line ",
    "<polyline",
    "<polygon",
];

fn gate(name: &str, src: &[u8], census_shapes: bool) {
    // 1. Parse + re-parse.
    let w1 = rt(src);
    // 2. Immediate byte fixed point.
    let w2 = rt(&w1);
    assert_eq!(
        String::from_utf8_lossy(&w1),
        String::from_utf8_lossy(&w2),
        "{name}: write(parse(write(x))) must equal write(x)"
    );
    // 3. Element census.
    let s = String::from_utf8_lossy(src).to_string();
    let s1 = String::from_utf8_lossy(&w1).to_string();
    let mut tags: Vec<&str> = CENSUS.to_vec();
    if census_shapes {
        tags.extend_from_slice(SHAPE_CENSUS);
    }
    for tag in tags {
        assert_eq!(
            s.matches(tag).count(),
            s1.matches(tag).count(),
            "{name}: occurrence count of {tag:?} must survive the round-trip:\n{s1}"
        );
    }
    // 4. Scene equivalence through the extras-free pipeline.
    let flat_src = write_svg(&parse_svg(src).expect("plain parse"));
    let flat_rt = write_svg(&parse_svg(&w1).expect("plain re-parse"));
    assert_eq!(
        String::from_utf8_lossy(&flat_src),
        String::from_utf8_lossy(&flat_rt),
        "{name}: source and round-tripped documents must flatten to the same scene"
    );
}

macro_rules! corpus_gate {
    ($test:ident, $file:literal) => {
        #[test]
        fn $test() {
            gate(
                $file,
                include_bytes!(concat!("fixtures/corpus/", $file, ".svg")),
                true,
            );
        }
    };
    ($test:ident, $file:literal, no_shape_census) => {
        #[test]
        fn $test() {
            gate(
                $file,
                include_bytes!(concat!("fixtures/corpus/", $file, ".svg")),
                false,
            );
        }
    };
}

corpus_gate!(gate_shapes, "shapes");
corpus_gate!(gate_gradients, "gradients");
corpus_gate!(gate_pattern, "pattern");
corpus_gate!(gate_use_symbol, "use_symbol");
corpus_gate!(gate_switch, "switch");
corpus_gate!(gate_filter, "filter");
corpus_gate!(gate_clip_mask, "clip_mask");
corpus_gate!(gate_markers, "markers");
corpus_gate!(gate_text, "text");
corpus_gate!(gate_animation, "animation");
corpus_gate!(gate_image, "image");
corpus_gate!(gate_nested_svg, "nested_svg");
corpus_gate!(gate_css, "css");
// §13.8 stroke-first paint-order split: native shape identity is
// deliberately declined (see module doc), so shape tags are exempt.
corpus_gate!(gate_hints, "hints", no_shape_census);
corpus_gate!(gate_links_desc, "links_desc");
corpus_gate!(gate_view_script_fo, "view_script_fo");

/// The pre-round-449 real-world fixture rides the same gate.
#[test]
fn gate_icon_house() {
    gate(
        "icon-house",
        include_bytes!("fixtures/icon-house.svg"),
        true,
    );
}

/// `.svgz` write: the gzip output is sniffed transparently on read and
/// carries the same document as the plain write.
#[test]
fn gate_svgz_roundtrip() {
    let src = include_bytes!("fixtures/corpus/shapes.svg");
    let (frame, _extras) = parse_svg_with_extras(src).expect("parse");
    let gz = write_svgz(&frame).expect("svgz write");
    assert!(
        gz.len() >= 2 && gz[0] == 0x1f && gz[1] == 0x8b,
        "RFC 1952 magic"
    );
    let plain = write_svg(&frame);
    let reparsed = write_svg(&parse_svg(&gz).expect("gz sniff + parse"));
    assert_eq!(
        String::from_utf8_lossy(&plain),
        String::from_utf8_lossy(&reparsed),
        "the .svgz payload flattens to the same scene as the plain write"
    );
}
