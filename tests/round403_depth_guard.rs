//! Round 403 — hostile-input hardening: bounded XML nesting depth.
//!
//! Both the SAX parser (`parse_xml`) and the model-builder that walks
//! its output recurse per element, so a document with tens of thousands
//! of nested elements would overflow the native stack and *abort* the
//! process (SIGABRT, not a catchable panic). The parser now refuses to
//! descend past [`oxideav_svg::parser::MAX_XML_DEPTH`] and returns a
//! typed [`oxideav_core::Error`] instead. Because that guard fires
//! during the lightweight tree build — before the much heavier decode
//! descent — bounding the parse depth transitively bounds the decode
//! depth: an over-limit document is rejected before the model-builder
//! ever recurses on it.

use oxideav_svg::parser::{parse_xml, MAX_XML_DEPTH};

/// Build `<svg>` with `depth` nested `<g>` elements around an inner
/// `<rect>`.
fn nested_svg(depth: usize) -> String {
    let mut s =
        String::from("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\">");
    for _ in 0..depth {
        s.push_str("<g>");
    }
    s.push_str("<rect width=\"1\" height=\"1\"/>");
    for _ in 0..depth {
        s.push_str("</g>");
    }
    s.push_str("</svg>");
    s
}

#[test]
fn parse_xml_rejects_pathological_nesting() {
    // Far past the guard — the classic "many nested groups" stack bomb.
    // These run on the default (small) test-thread stack precisely to
    // prove the guard rejects *before* any deep recursion happens; if
    // the guard were absent this would abort the test runner.
    for depth in [MAX_XML_DEPTH + 1, 10_000, 250_000] {
        let doc = nested_svg(depth);
        let err = parse_xml(&doc).expect_err("deep nesting must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("nesting too deep"),
            "depth {depth}: unexpected error {msg:?}"
        );
    }
}

#[test]
fn parse_xml_accepts_nesting_at_the_limit() {
    // A tree exactly at the limit is still accepted (the guard is
    // `depth >= MAX`, and the outer `<svg>` occupies level 0, so
    // `MAX_XML_DEPTH - 1` nested `<g>` is the deepest legal group run).
    let doc = nested_svg(MAX_XML_DEPTH - 2);
    let nodes = parse_xml(&doc).expect("at-limit nesting must parse");
    assert_eq!(nodes.len(), 1, "one <svg> root");
}

#[test]
fn deep_but_legal_decode_does_not_abort() {
    // Full `parse_svg` decode of a legitimately-but-deeply nested
    // document. The model-builder frame is heavy, so this is exercised
    // on a thread with a generous stack — the point is that an
    // in-bounds document *decodes* rather than being spuriously
    // rejected, and never aborts.
    let handle = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let doc = nested_svg(MAX_XML_DEPTH - 2);
            oxideav_svg::parse_svg(doc.as_bytes()).expect("in-bounds deep document must decode");
        })
        .expect("spawn decode thread");
    handle.join().expect("decode thread must not abort");
}

#[test]
fn shallow_nesting_is_unaffected() {
    // Ordinary depth round-trips exactly as before.
    let doc = nested_svg(8);
    let frame = oxideav_svg::parse_svg(doc.as_bytes()).expect("shallow decode");
    let out = oxideav_svg::write_svg(&frame);
    assert!(!out.is_empty());
}
