//! Round 403 — hostile-input hardening: `.svgz` decompression-bomb guard.
//!
//! A gzip stream can expand by ~1000×, so a few-KiB `.svgz` could inflate
//! to gigabytes and exhaust memory before the XML parser ran.
//! [`oxideav_svg::parser::inflate_gzip`] now caps the inflated size at
//! [`oxideav_svg::parser::MAX_SVGZ_INFLATED`] and refuses to materialise
//! anything larger.

use oxideav_svg::parser::{deflate_gzip, inflate_gzip, is_gzip, MAX_SVGZ_INFLATED};

#[test]
fn oversized_inflation_is_rejected() {
    // A highly-compressible payload just over the cap: a run of a single
    // byte gzips to a tiny stream but inflates past MAX_SVGZ_INFLATED.
    // Built as a multi-member gzip stream (RFC 1952 §2.2: members are
    // simply concatenated) from one 1 MiB member, so the test does not
    // pay for deflating 128 MiB itself.
    let member = deflate_gzip(&vec![b' '; 1 << 20]).expect("deflate");
    let members = (MAX_SVGZ_INFLATED >> 20) as usize + 1;
    let bomb: Vec<u8> = member.repeat(members);
    assert!(is_gzip(&bomb), "payload carries the gzip magic");
    assert!(
        bomb.len() < 1_000_000,
        "bomb is tiny relative to its inflated size ({} bytes)",
        bomb.len()
    );
    let err = inflate_gzip(&bomb).expect_err("over-cap inflation must be rejected");
    assert!(
        format!("{err}").contains("decompression-size limit"),
        "unexpected error: {err}"
    );
}

#[test]
fn in_bounds_svgz_still_round_trips() {
    // An ordinary SVG compresses + inflates losslessly, unaffected.
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"4\" height=\"4\"/></svg>";
    let gz = deflate_gzip(svg).expect("deflate");
    assert!(is_gzip(&gz));
    let back = inflate_gzip(&gz).expect("inflate");
    assert_eq!(back, svg);
    // And the whole-document sniff path decodes the compressed form.
    oxideav_svg::parse(&gz).expect("svgz decodes through parse");
}

#[test]
fn malformed_gzip_is_a_typed_error() {
    // Gzip magic followed by garbage must not panic.
    let mut junk = vec![0x1f, 0x8b, 0x08, 0x00];
    junk.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef, 0x00, 0xff, 0x13, 0x37]);
    assert!(is_gzip(&junk));
    let _ = inflate_gzip(&junk); // Err, not a panic.
    let _ = oxideav_svg::parse(&junk);
}
