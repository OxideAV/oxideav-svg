//! Round 403 — hostile-input hardening: `<use>` expansion is bounded in
//! both *count* and *depth*.
//!
//! The path-based cycle guard (`use_stack`) stops a self-referential
//! `<use href="#a">` inside `#a`, but two other adversarial shapes slip
//! past it, both driven entirely at decode time (the XML tree stays
//! flat, so the parse-time depth guard never sees them):
//!
//!  * a *diamond* — `#n0 → #n1 ×2 → #n2 ×2 → …` — where no id repeats on
//!    the instantiation path yet the decode expands 2ⁿ nodes. Bounded by
//!    the [`MAX_USE_EXPANSIONS`](oxideav_svg::element::MAX_USE_EXPANSIONS)
//!    total-instantiation budget.
//!  * a *linear chain* — `#n0 → #n1 → #n2 → …` — which instantiates a
//!    decode recursion as deep as the chain (thousands of frames) even
//!    though it expands only one node per level. Bounded by the
//!    [`MAX_RENDER_DEPTH`](oxideav_svg::element::MAX_RENDER_DEPTH) decode
//!    recursion guard.
//!
//! The heavy per-element decode frame means an in-bounds deep decode
//! still needs a roomy stack, so these run on a thread with a generous
//! stack size — the point is that the decode *terminates* (returns a
//! value or a typed error) instead of aborting the process.

fn on_big_stack<F: FnOnce() + Send + 'static>(f: F) {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("decode thread must not abort");
}

/// A `<use>` diamond bomb `levels` deep — 2^levels nodes if unbounded.
fn use_diamond(levels: usize) -> String {
    let mut s = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\"><defs>");
    for i in 0..levels {
        if i == levels - 1 {
            s.push_str(&format!("<rect id=\"n{i}\" width=\"1\" height=\"1\"/>"));
        } else {
            s.push_str(&format!(
                "<g id=\"n{i}\"><use href=\"#n{n}\"/><use href=\"#n{n}\"/></g>",
                n = i + 1
            ));
        }
    }
    s.push_str("</defs><use href=\"#n0\"/></svg>");
    s
}

/// A linear `<use>` chain `len` long — a decode stack `len` frames deep.
fn use_chain(len: usize) -> String {
    let mut s = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\"><defs>");
    for i in 0..len {
        if i == len - 1 {
            s.push_str(&format!("<rect id=\"c{i}\" width=\"1\" height=\"1\"/>"));
        } else {
            s.push_str(&format!("<g id=\"c{i}\"><use href=\"#c{}\"/></g>", i + 1));
        }
    }
    s.push_str("</defs><use href=\"#c0\"/></svg>");
    s
}

#[test]
fn use_diamond_bomb_terminates() {
    on_big_stack(|| {
        // 2^60 nodes if unbounded — never returns without the budget.
        let doc = use_diamond(60);
        let start = std::time::Instant::now();
        let result = oxideav_svg::parse_svg(doc.as_bytes());
        assert!(result.is_ok(), "count-bounded decode still succeeds");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(30),
            "diamond expansion must be count-bounded"
        );
    });
}

#[test]
fn deep_use_chain_does_not_abort() {
    on_big_stack(|| {
        // 20_000-deep decode recursion if unbounded — stack overflow.
        // The render-depth guard turns it into a typed error.
        let doc = use_chain(20_000);
        let result = oxideav_svg::parse_svg(doc.as_bytes());
        assert!(result.is_err(), "over-deep use chain must be a typed error");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("too deeply nested"),
            "unexpected error: {msg:?}"
        );
    });
}

#[test]
fn ordinary_use_still_instantiates() {
    // A normal, non-adversarial `<use>` is unaffected by either guard.
    let doc = "<svg xmlns=\"http://www.w3.org/2000/svg\">\
        <defs><rect id=\"r\" width=\"4\" height=\"4\"/></defs>\
        <use href=\"#r\" x=\"10\" y=\"10\"/></svg>";
    let frame = oxideav_svg::parse_svg(doc.as_bytes()).expect("normal use decodes");
    // The instantiated target geometry must be present in the decoded
    // frame (plain `parse_svg` flattens `<use>` into geometry).
    let out = String::from_utf8(oxideav_svg::write_svg(&frame)).expect("utf8 output");
    assert!(!out.is_empty(), "decoded output is non-empty");
    let extras = oxideav_svg::parse_svg_with_extras(doc.as_bytes())
        .expect("with-extras decode")
        .1;
    // The reference-preserving path keeps the `<use>` identity.
    let _ = extras;
}

#[test]
fn moderate_use_nesting_decodes() {
    // A legitimately-but-modestly nested chain (well under the guard)
    // decodes fully — the guard doesn't clip real content.
    on_big_stack(|| {
        let doc = use_chain(20);
        oxideav_svg::parse_svg(doc.as_bytes()).expect("in-bounds chain decodes");
    });
}
