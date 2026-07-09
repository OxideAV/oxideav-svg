//! Round 403 — hostile-input hardening: the public parsers must return
//! a typed error (or a value) on *any* input and never panic / abort.
//!
//! Covers the full parse surface — the whole-document decoder plus the
//! standalone grammar parsers (`d=` path data, `transform`, `<length>`,
//! paint) — with two input sources: a curated corpus of adversarial
//! strings that target specific grammar edge cases, and a deterministic
//! byte-fuzzer that mutates those seeds so a regression is reproducible
//! from the printed seed index alone.

use oxideav_svg::{
    color::{parse_opacity, parse_paint},
    length::parse_length,
    parser::parse_xml,
    path_data::parse_path_data,
    transform::parse_transform,
};

/// Adversarial grammar fragments. Each targets a place where a naive
/// parser indexes past the end of a token stream, divides by a
/// zero-extent, or trusts an attacker-supplied repeat count.
const HOSTILE: &[&str] = &[
    // --- path data: truncated / malformed command streams ---
    "",
    "M",
    "M ",
    "M,",
    "L 1",
    "C 1 2 3",
    "c",
    "A 1 1 0 1",
    "a 0 0 0 0 0 0 0", // zero-radius arc (degenerate ellipse)
    "A 1 1 0 5 9 2 2", // out-of-range flag digits
    "M 0 0 A",
    "M 0 0 A 1",
    "Q 1 2",
    "S 1 2",
    "T",
    "H",
    "V",
    "Z Z Z Z",
    "z",
    "m 1 1 z z z z z z",
    "M 1e999 1e-999 L NaN inf",
    "M .. . 1..2 3.4.5",
    "M -+-+1 2",
    "M 0 0 l 1e400 1e400", // overflow to inf
    "M0.0.0.0.0",          // sticky decimal points
    // --- transforms ---
    "matrix",
    "matrix(",
    "matrix()",
    "matrix(1)",
    "matrix(1 2 3 4 5)",
    "matrix(1 2 3 4 5 6 7)",
    "rotate()",
    "rotate(45 1)",
    "rotate(1 2 3 4)",
    "translate(",
    "scale(,)",
    "skewX()",
    "unknown(1 2)",
    "translate(1e999)scale(1e-999)",
    "rotate(NaN)",
    ")(",
    "matrix(1,2,3,4,5,6)matrix(", // valid then truncated
    // --- lengths ---
    "px",
    "e",
    "1e",
    "1e+",
    ".",
    "-",
    "+.e",
    "1.2.3px",
    "99999999999999999999999999px",
    "1e999em",
    "%",
    "12zz",
    "  ",
    // --- paint / opacity ---
    "#",
    "#z",
    "#1",
    "#12",
    "#12345",
    "rgb(",
    "rgb()",
    "rgb(1)",
    "rgb(1,2)",
    "rgb(999,999,999)",
    "rgba(1,2,3)",
    "rgb(1e9%,2%,3%)",
    "url(",
    "url(#)",
    "url()",
    "currentColor",
    "hsl(400,200%,-5%)",
    "not-a-color",
];

fn exercise_fragment(s: &str) {
    // None of these may panic. Return values are irrelevant.
    let _ = parse_path_data(s);
    let _ = parse_transform(s);
    let _ = parse_length(s);
    let _ = parse_paint(s);
    let _ = parse_opacity(s);
}

/// Hostile whole documents — malformed markup that stresses the SAX
/// parser and the model-builder together.
const HOSTILE_DOCS: &[&str] = &[
    "<",
    "<svg",
    "<svg>",
    "<svg xmlns='http://www.w3.org/2000/svg'>",
    "<svg><rect",
    "<svg><rect/></svg",
    "<svg><g><g><g></svg>",
    "<svg><path d='M0 0 A 1 1 0 9 9 z'/></svg>",
    "<svg><rect width='-5' height='NaN'/></svg>",
    "<svg><circle r='1e999'/></svg>",
    "<svg><text>&amp;&lt;&#x110000;&#-1;&;&#;</text></svg>",
    "<svg><!-- unterminated comment <rect/>",
    "<svg><![CDATA[ unterminated",
    "<svg foo:bar:baz='1' ='2' a=></svg>",
    "<svg><use href='#a'/><g id='a'><use href='#a'/></g></svg>", // self-ref cycle
    "<svg><linearGradient id='g'><stop offset='9e9'/></linearGradient></svg>",
    "<svg><style>* { fill: url(#; } @media</style><rect/></svg>",
    "<svg><path d='M0 0 l 1 1'/></svg>\u{0}\u{0}\u{0}",
    "<svg\u{feff}></svg>",
    "<svg><filter id='f'><feGaussianBlur stdDeviation='1e999'/><feColorMatrix values='1 2 3'/></filter></svg>",
    "<svg><filter><feConvolveMatrix order='0' kernelMatrix='1 2 3'/></filter></svg>",
    "<svg><filter><feComponentTransfer><feFuncR type='table' tableValues=''/></feComponentTransfer></filter></svg>",
    "<svg><feTurbulence baseFrequency='-1' numOctaves='999999999'/></svg>",
    "<svg><radialGradient id='g' fx='1e9' fy='NaN' r='-1'><stop offset='2'/><stop/></radialGradient></svg>",
    "<svg><animate attributeName='x' values='1;2;;' keyTimes='0;;1' calcMode='spline' keySplines=''/></svg>",
    "<svg><animateTransform type='rotate' from='' to='' by=''/></svg>",
    "<svg><style>@keyframes k{0%{}from{}to{}999%{fill}}</style></svg>",
    "<svg><path d='M0 0 A 0 0 0 0 0 1 1 A -1 -1 9 2 3 0 0'/></svg>",
    "<svg><text x='1 2 3' dx='' dy='NaN'><tspan rotate='1 2'>hi</tspan></text></svg>",
    "<svg><marker refX='1e9' markerWidth='-1' orient='9deg9'/></svg>",
    "<svg viewBox='0 0 0 0' preserveAspectRatio='xMidYMid slice meet garbage'><rect/></svg>",
    "<svg><pattern patternUnits='' viewBox='a b c d'><rect/></pattern></svg>",
];

fn exercise_doc(bytes: &[u8]) {
    let _ = parse_xml(&String::from_utf8_lossy(bytes));
    if let Ok(frame) = oxideav_svg::parse_svg(bytes) {
        // A successful decode must also survive a write round-trip.
        let _ = oxideav_svg::write_svg(&frame);
    }
    let _ = oxideav_svg::parse_svg_with_extras(bytes);
}

/// Tiny deterministic xorshift PRNG so a failure is reproducible.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

#[test]
fn unterminated_at_rule_block_does_not_panic() {
    // Regression: an `@media`/`@keyframes`/`@font-face`/`@supports`
    // block opened with `{` but never closed before EOF used to slice
    // `stripped[block_body_start..0]` (begin > end) and panic. Each of
    // these must now parse to a (possibly empty) stylesheet.
    let docs = [
        "<svg><style>@media{</style><rect/></svg>",
        "<svg><style>* { fill: url(#; } @media{</style></svg>",
        "<svg><style>@keyframes x {</style></svg>",
        "<svg><style>@font-face { src:</style></svg>",
        "<svg><style>@supports (x:1) {</style></svg>",
        "<svg><style>@media screen and (min-width:1px) { .a { fill</style></svg>",
    ];
    for d in docs {
        // Both the whole-document path and the raw stylesheet path.
        let _ = oxideav_svg::parse_svg(d.as_bytes());
        let _ = oxideav_svg::parse_svg_with_extras(d.as_bytes());
    }
}

#[test]
fn public_grammar_parsers_never_panic() {
    for s in HOSTILE {
        exercise_fragment(s);
    }
}

#[test]
fn whole_document_parsers_never_panic() {
    for d in HOSTILE_DOCS {
        exercise_doc(d.as_bytes());
    }
}

#[test]
fn byte_mutation_fuzz_never_panics() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    // Mutate each seed a fixed number of times: flip / drop / duplicate
    // bytes, then re-parse. Deterministic so any panic reproduces.
    let seeds: Vec<&str> = HOSTILE.iter().chain(HOSTILE_DOCS.iter()).copied().collect();
    for (si, seed) in seeds.iter().enumerate() {
        for iter in 0..2000 {
            let mut buf = seed.as_bytes().to_vec();
            if !buf.is_empty() {
                let ops = 1 + (rng.next() % 6) as usize;
                for _ in 0..ops {
                    if buf.is_empty() {
                        break;
                    }
                    let idx = (rng.next() as usize) % buf.len();
                    match rng.next() % 3 {
                        0 => buf[idx] = (rng.next() & 0xff) as u8,
                        1 => {
                            buf.remove(idx);
                        }
                        _ => buf.insert(idx, (rng.next() & 0xff) as u8),
                    }
                }
            } else {
                buf.push((rng.next() & 0xff) as u8);
            }
            // The seed index + iter localise any discovered crash.
            let _guard = (si, iter);
            exercise_doc(&buf);
            let lossy = String::from_utf8_lossy(&buf);
            exercise_fragment(&lossy);
        }
    }
}
