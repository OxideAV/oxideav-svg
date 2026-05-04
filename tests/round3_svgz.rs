//! Round 3 — `.svgz` (gzip-compressed SVG, RFC 1952) inflation +
//! deflation.

use oxideav_svg::{parse_svg, write_svg, write_svgz};

const SAMPLE: &[u8] = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" viewBox="0 0 40 40">
  <rect x="5" y="5" width="30" height="30" fill="#11aaff"/>
</svg>"##;

#[test]
fn parse_svg_inflates_gzip_input_transparently() {
    let plain = parse_svg(SAMPLE).expect("plain SVG parses");
    // Gzip-compress the same bytes by hand using flate2 directly.
    let gz = gzip_bytes(SAMPLE);
    assert_eq!(&gz[..2], &[0x1f, 0x8b], "header sanity");
    let inflated = parse_svg(&gz).expect("svgz parses");
    assert_eq!(plain.width, inflated.width);
    assert_eq!(plain.height, inflated.height);
    assert_eq!(plain.root.children.len(), inflated.root.children.len());
}

#[test]
fn write_svgz_round_trips_through_parse() {
    let frame = parse_svg(SAMPLE).unwrap();
    let gz = write_svgz(&frame).expect("encode succeeds");
    assert_eq!(&gz[..2], &[0x1f, 0x8b], "write_svgz emits gzip header");
    let frame2 = parse_svg(&gz).expect("svgz round-trip parses");
    assert_eq!(frame.width, frame2.width);
    assert_eq!(frame.root.children.len(), frame2.root.children.len());
}

#[test]
fn svgz_is_smaller_than_plain_for_repetitive_content() {
    // Write a long repetitive SVG so gzip has something to compress.
    let mut s = String::from(
        r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">"##,
    );
    for i in 0..200 {
        s.push_str(&format!(
            "<rect x=\"{}\" y=\"0\" width=\"5\" height=\"5\" fill=\"#abcdef\"/>",
            i % 100
        ));
    }
    s.push_str("</svg>");
    let plain = s.as_bytes().to_vec();
    let frame = parse_svg(&plain).unwrap();
    let xml = write_svg(&frame);
    let gz = write_svgz(&frame).unwrap();
    assert!(
        gz.len() < xml.len(),
        "gzip should compress repetitive XML (xml={} svgz={})",
        xml.len(),
        gz.len()
    );
}

#[test]
fn invalid_gzip_payload_returns_an_error() {
    // First two bytes match the magic but the rest is junk.
    let mut bad = vec![0x1f, 0x8b];
    bad.extend_from_slice(&[0u8; 32]);
    let res = parse_svg(&bad);
    assert!(res.is_err(), "junk gzip payload must error, not panic");
}

fn gzip_bytes(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}
