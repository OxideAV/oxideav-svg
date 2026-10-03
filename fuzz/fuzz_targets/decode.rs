#![no_main]

//! Hostile-input liveness over the standalone contract surface and the
//! vector document API. Every call must return a `Result`; nothing may
//! panic, abort or allocate past the `DecodeOptions` defaults (128 MiB
//! inflate cap, 1 M elements, depth 128).

use libfuzzer_sys::fuzz_target;
use oxideav_svg::{
    decode, decode_rgb8, decode_rgba8, decode_with, info, parse, parse_at, parse_with,
    parse_with_extras, probe, DecodeOptions,
};

fuzz_target!(|data: &[u8]| {
    let _ = probe(data);
    let _ = info(data);
    let _ = decode(data);
    let _ = decode_rgb8(data);
    let _ = decode_rgba8(data);
    // Tight limits so the fuzzer also walks the LimitExceeded arms.
    let tight = DecodeOptions::default()
        .with_max_bytes(64 * 1024u64)
        .with_max_elements(256u64)
        .with_max_depth(16usize)
        .with_max_pixels(1 << 16u64);
    let _ = decode_with(data, &tight);
    let _ = parse_with(data, &tight);
    let _ = parse_with(data, &DecodeOptions::default().with_strict(true));
    if let Ok(doc) = parse(data) {
        let _ = oxideav_svg::write(&doc);
    }
    if let Ok((doc, extras)) = parse_with_extras(data) {
        let _ = oxideav_svg::write_with_extras(&doc, &extras);
        let _ = oxideav_svg::resolve_fragment(&doc, &extras, "svgView(viewBox(0,0,1,1))");
    }
    // A mid-timeline sample exercises the SMIL evaluator.
    let _ = parse_at(data, 1.5);
});
