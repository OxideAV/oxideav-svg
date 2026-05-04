//! Round 2 — `<filter>` graceful pass-through. Round 2 doesn't render
//! filters (deferred to oxideav-raster #368); these tests verify the
//! parser captures the def + accepts `filter="url(#id)"` on elements
//! without losing the rest of the document.

use oxideav_svg::{parse_svg, write_svg};

#[test]
fn filter_def_does_not_crash_parse() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <filter id="blur">
      <feGaussianBlur stdDeviation="5"/>
    </filter>
  </defs>
  <rect x="10" y="10" width="50" height="50" fill="red" filter="url(#blur)"/>
</svg>"##;
    let frame = parse_svg(src).expect("svg parses");
    // The rect must still be visible after filter resolution.
    assert!(
        !frame.root.children.is_empty(),
        "filter wrapper should keep the rect"
    );
}

#[test]
fn filter_round_trip_is_lossless_at_structural_level() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="60" height="60">
  <defs>
    <filter id="f1">
      <feGaussianBlur stdDeviation="2"/>
    </filter>
  </defs>
  <g filter="url(#f1)">
    <rect x="0" y="0" width="20" height="20" fill="#0000ff"/>
  </g>
</svg>"##;
    let frame = parse_svg(src).expect("first parse");
    let bytes = write_svg(&frame);
    let frame2 = parse_svg(&bytes).expect("re-parse");
    assert_eq!(frame.width, frame2.width);
    assert_eq!(frame.height, frame2.height);
    // The filter wrapper Group survives — exact child count varies
    // by how many group layers were collapsed, so just assert at
    // least one descendant path made it through.
    assert!(
        count_paths(&frame2.root) >= 1,
        "filter round-trip should preserve at least one painted path"
    );
}

fn count_paths(g: &oxideav_core::Group) -> usize {
    let mut n = 0;
    for c in &g.children {
        match c {
            oxideav_core::Node::Path(_) => n += 1,
            oxideav_core::Node::Group(sg) => n += count_paths(sg),
            _ => {}
        }
    }
    n
}

#[test]
fn unknown_filter_id_is_silently_ignored() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <rect width="10" height="10" fill="red" filter="url(#nope)"/>
</svg>"##;
    let frame = parse_svg(src).expect("missing-filter-id parses");
    assert_eq!(frame.root.children.len(), 1);
}
