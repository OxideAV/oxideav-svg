#![no_main]

//! Writer fixed point: whatever the parser accepts, the plain writer's
//! output must be stable under its own round-trip —
//! `write(parse(write(parse(x)))) == write(parse(x))` — and the `.svgz`
//! writer must inflate back to the plain bytes.
//!
//! The `PreservedExtras` path is driven for liveness and re-parseability
//! only: its verbatim carriers (`<view>`, `<script>`, `<foreignObject>`,
//! …) are byte-stable on the conformance corpus (`tests/
//! round449_write_conformance.rs`) but not on arbitrary malformed
//! nestings the lenient parser still accepts (a `<view>` inside a
//! `<view>`, a `<script>` inside a `<view>`), where the verbatim
//! subtree and its separately captured children are both re-emitted.

use libfuzzer_sys::fuzz_target;
use oxideav_svg::{parse, parse_with_extras, write, write_svgz, write_with_extras};

fuzz_target!(|data: &[u8]| {
    let Ok(doc) = parse(data) else {
        return;
    };
    let once = write(&doc);
    let Ok(doc2) = parse(&once) else {
        panic!("writer output must re-parse");
    };
    let twice = write(&doc2);
    assert_eq!(once, twice, "plain writer is not a fixed point");

    if let Ok(gz) = write_svgz(&doc) {
        let Ok(doc3) = parse(&gz) else {
            panic!("svgz output must re-parse");
        };
        assert_eq!(write(&doc3), once, "svgz round-trip changed the scene");
    }

    if let Ok((doc, extras)) = parse_with_extras(data) {
        let once = write_with_extras(&doc, &extras);
        let Ok((doc2, extras2)) = parse_with_extras(&once) else {
            panic!("extras writer output must re-parse");
        };
        let _ = write_with_extras(&doc2, &extras2);
    }
});
