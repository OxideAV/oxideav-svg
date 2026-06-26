//! Round 372 — `filter="url(#id)"` reference round-trip fidelity
//! (SVG 1.1 §15).
//!
//! The decoder wraps a filtered element in a pass-through `Group` (the
//! actual rasterisation is `oxideav-raster` work) and preserves the
//! `<filter>` def verbatim in `PreservedExtras::filters` — but before
//! this round the *reference* from the graphics element to the filter
//! was dropped on write, leaving the `<filter>` def orphaned. This
//! round records the source `filter=` attribute per scene-graph
//! tree-path and re-emits it on the filter-wrapper `<g>` on write.

use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

fn roundtrip(src: &[u8]) -> String {
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    let out = write_svg_with_extras(&frame, &extras);
    String::from_utf8(out).expect("utf8")
}

#[test]
fn filter_reference_survives_round_trip() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <defs>
    <filter id="f"><feGaussianBlur stdDeviation="2"/></filter>
  </defs>
  <rect width="50" height="50" filter="url(#f)" fill="red"/>
</svg>"##;
    let out = roundtrip(src);
    // The <filter> def survives (existing behaviour) ...
    assert!(
        out.contains("<filter") && out.contains("id=\"f\""),
        "filter def:\n{out}"
    );
    // ... AND the graphics element re-references it.
    assert!(
        out.contains("filter=\"url(#f)\""),
        "the filter reference is re-attached on the wrapper <g>:\n{out}"
    );
}

#[test]
fn filter_reference_reconnects_after_reparse() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <defs>
    <filter id="blur"><feGaussianBlur stdDeviation="3"/></filter>
  </defs>
  <g filter="url(#blur)">
    <rect width="20" height="20" fill="blue"/>
  </g>
</svg>"##;
    let out = roundtrip(src);
    assert!(out.contains("filter=\"url(#blur)\""), "ref present:\n{out}");
    // Re-parse: the binding must reappear, proving the reference is a
    // first-class round-trip carrier rather than a one-shot.
    let (_, extras) = parse_svg_with_extras(out.as_bytes()).expect("reparse");
    assert_eq!(extras.filter_refs.len(), 1, "one filter-ref on re-parse");
    assert_eq!(extras.filter_refs[0].filter, "url(#blur)");
}

#[test]
fn unresolved_filter_reference_records_nothing() {
    // A `filter="url(#missing)"` whose def doesn't exist must not record
    // a binding (no wrapper group is produced for an unresolved ref).
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <rect width="50" height="50" filter="url(#missing)" fill="red"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).expect("parse");
    assert!(
        extras.filter_refs.is_empty(),
        "no binding for an unresolved filter reference"
    );
}

#[test]
fn document_without_filter_is_unaffected() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <rect width="10" height="10" fill="red"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).expect("parse");
    assert!(extras.filter_refs.is_empty(), "no filter refs");
    let out = roundtrip(src);
    assert!(
        !out.contains("filter=\"url("),
        "no spurious filter ref:\n{out}"
    );
}
