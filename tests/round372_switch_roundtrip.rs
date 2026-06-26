//! Round 372 — `<switch>` verbatim round-trip fidelity (SVG 2 §5.7).
//!
//! The decoder renders the first child whose conditional-processing
//! attributes test true and wraps it in a `Group`, discarding the
//! unselected alternatives + the `<switch>` element identity. Before
//! this round, `parse → write` re-emitted that group as a plain `<g>`
//! with only the selected child — so re-parsing under a *different*
//! `systemLanguage` would have been frozen on the first decode's
//! choice, and the alternatives were lost entirely.
//!
//! This round captures the whole `<switch>` verbatim in
//! `PreservedExtras::switches` and collapses the selected-branch group
//! back to the full `<switch>` on write.

use oxideav_svg::{parse_svg_with_extras, write_svg_with_extras};

fn roundtrip(src: &[u8]) -> String {
    let (frame, extras) = parse_svg_with_extras(src).expect("parse");
    let out = write_svg_with_extras(&frame, &extras);
    String::from_utf8(out).expect("utf8")
}

#[test]
fn switch_round_trips_all_alternatives() {
    // Two language alternatives + a fallback. The decoder selects the
    // fallback (no `systemLanguage` configured), but the round-trip must
    // re-emit every alternative.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <switch>
    <rect systemLanguage="fr" x="0" y="0" width="10" height="10" fill="blue"/>
    <rect systemLanguage="de" x="0" y="0" width="10" height="10" fill="green"/>
    <rect x="0" y="0" width="10" height="10" fill="red"/>
  </switch>
</svg>"##;
    let out = roundtrip(src);
    assert!(out.contains("<switch"), "the <switch> survives:\n{out}");
    assert!(
        out.contains("systemLanguage=\"fr\""),
        "the fr alternative survives:\n{out}"
    );
    assert!(
        out.contains("systemLanguage=\"de\""),
        "the de alternative survives:\n{out}"
    );
    assert!(out.contains("fill=\"red\""), "fallback survives:\n{out}");
    // Exactly one <switch> — not a plain <g> dump of the selected child.
    assert_eq!(
        out.matches("<switch").count(),
        1,
        "exactly one <switch>:\n{out}"
    );
}

#[test]
fn switch_survives_reparse() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <switch>
    <g requiredExtensions="http://example.com/notreal">
      <rect x="0" y="0" width="10" height="10" fill="blue"/>
    </g>
    <rect x="0" y="0" width="10" height="10" fill="red"/>
  </switch>
</svg>"##;
    let out = roundtrip(src);
    // Re-parse the serialised output: the <switch> must still pick the
    // fallback rect (the unsatisfiable requiredExtensions branch is
    // bypassed), proving the conditional structure round-tripped rather
    // than being frozen.
    let (_, extras) = parse_svg_with_extras(out.as_bytes()).expect("reparse");
    assert_eq!(extras.switches.len(), 1, "one switch binding on re-parse");
    assert!(
        out.contains("requiredExtensions"),
        "the unsatisfiable branch survives so re-selection is faithful:\n{out}"
    );
}

#[test]
fn switch_with_transform_preserved() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
  <switch transform="translate(5,5)">
    <rect x="0" y="0" width="10" height="10" fill="red"/>
  </switch>
</svg>"##;
    let out = roundtrip(src);
    assert!(
        out.contains("transform=\"translate(5,5)\""),
        "the switch's own transform survives:\n{out}"
    );
}

#[test]
fn document_without_switch_is_unaffected() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <rect x="0" y="0" width="10" height="10" fill="red"/>
</svg>"##;
    let (_, extras) = parse_svg_with_extras(src).expect("parse");
    assert!(extras.switches.is_empty(), "no switch bindings");
    let out = roundtrip(src);
    assert!(!out.contains("<switch"), "no spurious <switch>:\n{out}");
}
