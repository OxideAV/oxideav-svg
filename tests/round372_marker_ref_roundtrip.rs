//! Round 372 — `marker-start` / `marker-mid` / `marker-end` (and the
//! `marker` shorthand) reference round-trip fidelity (SVG 2 §13.7.4).
//!
//! `oxideav_core::Node` has no marker construct (vertex placement is
//! deferred to a core `Marker` node), so a shape's marker references
//! were dropped on write even though the `<marker>` def itself rides
//! `PreservedExtras::markers` verbatim — orphaning the def. This round
//! records the verbatim `marker-*` attribute text per scene-graph
//! tree-path and re-emits it on the shape on write.

use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

fn roundtrip(src: &[u8]) -> String {
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    let out = write_svg_with_extras(&frame, &extras);
    String::from_utf8(out).expect("utf8")
}

#[test]
fn marker_end_reference_survives() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <marker id="arrow" markerWidth="10" markerHeight="10" refX="5" refY="5">
      <path d="M0,0 L10,5 L0,10 Z" fill="black"/>
    </marker>
  </defs>
  <path d="M10,10 L90,90" stroke="black" marker-end="url(#arrow)"/>
</svg>"##;
    let out = roundtrip(src);
    // The <marker> def survives (existing behaviour) ...
    assert!(
        out.contains("<marker") && out.contains("id=\"arrow\""),
        "marker def:\n{out}"
    );
    // ... AND the shape re-references it.
    assert!(
        out.contains("marker-end=\"url(#arrow)\""),
        "the marker-end reference is re-attached:\n{out}"
    );
}

#[test]
fn all_three_marker_positions_survive() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <marker id="m" markerWidth="6" markerHeight="6"><circle cx="3" cy="3" r="3"/></marker>
  </defs>
  <polyline points="10,10 50,50 90,10" fill="none" stroke="black"
            marker-start="url(#m)" marker-mid="url(#m)" marker-end="url(#m)"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(out.contains("marker-start=\"url(#m)\""), "start:\n{out}");
    assert!(out.contains("marker-mid=\"url(#m)\""), "mid:\n{out}");
    assert!(out.contains("marker-end=\"url(#m)\""), "end:\n{out}");
}

#[test]
fn marker_shorthand_expands_to_all_positions() {
    // The `marker` shorthand sets all three position-specific
    // properties; the round-trip re-emits the expanded longhands.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <marker id="dot"><circle cx="2" cy="2" r="2"/></marker>
  </defs>
  <path d="M0,0 L50,50" stroke="black" marker="url(#dot)"/>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains("marker-start=\"url(#dot)\""),
        "start from shorthand:\n{out}"
    );
    assert!(
        out.contains("marker-mid=\"url(#dot)\""),
        "mid from shorthand:\n{out}"
    );
    assert!(
        out.contains("marker-end=\"url(#dot)\""),
        "end from shorthand:\n{out}"
    );
}

#[test]
fn marker_reference_reconnects_after_reparse() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <defs>
    <marker id="arrow"><path d="M0,0 L10,5 L0,10 Z"/></marker>
  </defs>
  <path d="M10,10 L90,90" stroke="black" marker-end="url(#arrow)"/>
</svg>"##;
    let out = roundtrip(src);
    let (_, extras) = parse_svg_with_extras(out.as_bytes()).expect("reparse");
    assert_eq!(extras.marker_refs.len(), 1, "one marker-ref on re-parse");
    assert_eq!(
        extras.marker_refs[0].marker_end.as_deref(),
        Some("url(#arrow)")
    );
}

#[test]
fn document_without_markers_is_unaffected() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <path d="M0,0 L10,10" stroke="black"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).expect("parse");
    assert!(extras.marker_refs.is_empty(), "no marker refs");
    let out = roundtrip(src);
    assert!(!out.contains("marker-"), "no spurious marker attrs:\n{out}");
}
