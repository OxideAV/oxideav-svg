//! Round 122 — SVG 2 §5.8 `<title>` / `<desc>` and §5.9 `<metadata>`
//! descriptive elements.
//!
//! `<title>`, `<desc>`, and `<metadata>` are *never-rendered* elements
//! per the §5.8 / §5.9 dfn blocks (the UA stylesheet forces
//! `display:none` with importance over any other CSS rule). They MUST
//! NOT contribute scene-graph nodes — they're accessibility metadata
//! consumed by assistive technologies. Round 122 captures them on
//! `PreservedExtras::titles` / `descs` / `metadata` so a
//! `parse_svg_with_extras → write_svg_with_extras` cycle round-trips
//! the descriptive content (text body + optional `lang` selection key
//! per §5.8 multilingual alternatives).
//!
//! Wall: read only `docs/image/svg/svg2-candidate-recommendation-single.html`
//! §5.8 (anchor `struct-DescriptionAndTitleElements`) and §5.9
//! (anchor `struct-MetadataElement`). No web, no external libs.

use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg, write_svg_with_extras};

/// Helper — find the first descriptive-binding entry for the given
/// parent-path. Returns the entry by reference so the test can poke
/// at `items` without copying.
fn binding_at<'a>(
    bindings: &'a [oxideav_svg::DescriptiveBinding],
    parent_path: &[usize],
) -> &'a oxideav_svg::DescriptiveBinding {
    bindings
        .iter()
        .find(|b| b.parent_path.as_slice() == parent_path)
        .expect("descriptive binding for the expected parent path")
}

#[test]
fn title_at_root_captured_with_text() {
    // The most basic SVG 2 §5.8 case: a `<title>` child of the root
    // `<svg>` captures its text into `extras.titles[0].items[0]` keyed
    // by the empty parent path (= the root).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <title>Hello SVG</title>
  <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
</svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.titles.len(), 1, "one parent has a <title> child");
    let root = binding_at(&extras.titles, &[]);
    assert_eq!(root.items.len(), 1);
    assert_eq!(root.items[0].text, "Hello SVG");
    assert!(
        root.items[0].lang.is_none(),
        "no explicit lang on this title"
    );
}

#[test]
fn desc_at_root_captured_with_text() {
    // Mirror of the title test but for `<desc>`; same path + same
    // shape.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <desc>A 10x10 red square</desc>
  <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
</svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.descs.len(), 1);
    let root = binding_at(&extras.descs, &[]);
    assert_eq!(root.items.len(), 1);
    assert_eq!(root.items[0].text, "A 10x10 red square");
}

#[test]
fn title_and_desc_do_not_render() {
    // §5.8: "must always be set to display:none ... with importance
    // over any other CSS rule or presentation attribute". The
    // scene graph must contain only the rectangle, not the title or
    // desc.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <title>not in the scene</title>
  <desc>also not in the scene</desc>
  <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    // Exactly one scene child — the rect — and it's a Path.
    assert_eq!(frame.root.children.len(), 1);
    match &frame.root.children[0] {
        oxideav_core::Node::Path(_) => {}
        other => panic!("expected only the rect to render, got {other:?}"),
    }
}

#[test]
fn title_lang_attribute_captured() {
    // §5.8 multilingual alternatives use `lang`; the side-channel
    // surfaces it so a downstream language-match consumer can pick the
    // best one. SVG-2 `lang` is the canonical form.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <title lang="en">Favorite</title>
  <title lang="nl">Favoriet</title>
</svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    let root = binding_at(&extras.titles, &[]);
    assert_eq!(root.items.len(), 2, "both language variants preserved");
    assert_eq!(root.items[0].lang.as_deref(), Some("en"));
    assert_eq!(root.items[0].text, "Favorite");
    assert_eq!(root.items[1].lang.as_deref(), Some("nl"));
    assert_eq!(root.items[1].text, "Favoriet");
}

#[test]
fn xml_lang_falls_back_when_lang_absent() {
    // SVG 1.1 / XML 1.0 documents carry `xml:lang`; round 122 records
    // it when `lang` (the SVG-2 form) is absent.
    let src = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xml="http://www.w3.org/XML/1998/namespace" width="10" height="10">
  <title xml:lang="fr">Étoile</title>
</svg>"##
        .as_bytes();
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    let root = binding_at(&extras.titles, &[]);
    assert_eq!(root.items[0].lang.as_deref(), Some("fr"));
    assert_eq!(root.items[0].text, "Étoile");
}

#[test]
fn title_on_nested_group_keyed_by_group_path() {
    // The parent-path key follows the *scene-graph* layout, not the
    // source XML. A `<title>` inside a `<g>` keys to the `<g>`'s own
    // scene-graph index (the source `<svg>`'s first child).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <g>
    <title>The group title</title>
    <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
  </g>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    // The scene-walk produces one root child (the group).
    assert_eq!(frame.root.children.len(), 1);
    // Binding keys at `[0]` — the group's slot in `root.children`.
    let g_binding = binding_at(&extras.titles, &[0]);
    assert_eq!(g_binding.items.len(), 1);
    assert_eq!(g_binding.items[0].text, "The group title");
}

#[test]
fn metadata_captured_verbatim() {
    // §5.9 `<metadata>` content is opaque foreign-namespace XML — we
    // capture the whole element. The element survives the round-trip
    // intact.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Sample</dc:title>
  </metadata>
</svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.metadata.len(), 1, "metadata captured");
    // The captured element has a `<dc:title>` child carrying "Sample".
    let md = &extras.metadata[0];
    let has_dc = md.children.iter().any(|n| match n {
        oxideav_svg::parser::Node::Element(e) => e.name.ends_with("title"),
        _ => false,
    });
    assert!(has_dc, "metadata preserves its child element");
}

#[test]
fn metadata_is_never_rendered() {
    // Like title / desc, metadata must not appear in the scene graph.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <metadata><foo/></metadata>
  <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(frame.root.children.len(), 1, "only the rect renders");
    match &frame.root.children[0] {
        oxideav_core::Node::Path(_) => {}
        other => panic!("expected Path for the rect, got {other:?}"),
    }
}

#[test]
fn write_svg_with_extras_emits_root_title_first() {
    // The encoder places root-level `<title>` / `<desc>` at the top of
    // the output document so an SVG 1.1 reader that "may not recognize
    // a title element that is not the first child of its parent" still
    // sees them.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <title>Top of doc</title>
  <desc>Tooltip body</desc>
  <rect x="0" y="0" width="10" height="10" fill="#000000"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let bytes = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(bytes).unwrap();
    let title_pos = s.find("<title>").expect("title emitted");
    let desc_pos = s.find("<desc>").expect("desc emitted");
    let rect_pos = s.find("<path").expect("rect emitted as <path>");
    assert!(
        title_pos < desc_pos && desc_pos < rect_pos,
        "title precedes desc precedes rect: {title_pos} {desc_pos} {rect_pos}"
    );
    assert!(s.contains("Top of doc"));
    assert!(s.contains("Tooltip body"));
}

#[test]
fn write_svg_with_extras_emits_group_title_inside_group() {
    // A `<g><title>...</title>...</g>` round-trip emits the `<title>`
    // as the first child of the matching `<g>` in the output.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <g>
    <title>Inner title</title>
    <rect x="0" y="0" width="10" height="10" fill="#000000"/>
  </g>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let bytes = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(bytes).unwrap();
    let g_pos = s.find("<g>").expect("<g> emitted");
    let title_pos = s.find("<title>").expect("title emitted");
    let rect_pos = s.find("<path").expect("rect emitted as <path>");
    assert!(g_pos < title_pos);
    assert!(title_pos < rect_pos);
    assert!(s.contains("Inner title"));
}

#[test]
fn round_trip_preserves_lang_attribute() {
    // The encoder re-emits the `lang` attribute so a re-parse recovers
    // the same multilingual selection.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <title lang="en-us">Color</title>
  <title lang="en-gb">Colour</title>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let bytes = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(bytes.clone()).unwrap();
    assert!(s.contains("lang=\"en-us\""));
    assert!(s.contains("lang=\"en-gb\""));
    // Round-trip back through the parser — both title entries survive.
    let (_f2, extras2) = parse_svg_with_extras(&bytes).unwrap();
    let root = binding_at(&extras2.titles, &[]);
    assert_eq!(root.items.len(), 2);
    assert_eq!(root.items[0].lang.as_deref(), Some("en-us"));
    assert_eq!(root.items[1].lang.as_deref(), Some("en-gb"));
}

#[test]
fn round_trip_preserves_metadata() {
    // `<metadata>` survives a parse → write cycle; the encoder
    // re-emits at the trailing edge of the document (between scene
    // children and `</svg>`).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <metadata>
    <foo xmlns="urn:example">bar</foo>
  </metadata>
  <rect x="0" y="0" width="10" height="10" fill="#000000"/>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let bytes = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(bytes.clone()).unwrap();
    assert!(s.contains("<metadata>"), "metadata wrapper preserved");
    assert!(s.contains("<foo"));
    assert!(s.contains("bar"));
    // Re-parse the output — the metadata count is stable across round
    // trips (the encoder doesn't duplicate, the parser doesn't drop).
    let (_f2, extras2) = parse_svg_with_extras(&bytes).unwrap();
    assert_eq!(extras2.metadata.len(), 1);
}

#[test]
fn bare_parse_svg_does_not_populate_extras_paths() {
    // The id-path tracking gate (`track_id_paths`) keeps `parse_svg`'s
    // hot path zero-allocation for the side-channel buffers; only
    // `parse_svg_with_extras` opts in. This guards the gate so a
    // future refactor doesn't accidentally allocate per shape on the
    // bare path.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <title>doc title</title>
  <g><desc>group desc</desc></g>
</svg>"##;
    // bare parse_svg builds the scene graph but doesn't expose the
    // side-channel. We can only check that the scene graph is
    // structurally correct — no title / desc nodes in the tree.
    let frame = parse_svg(src).unwrap();
    // Root has zero scene children (the `<title>` doesn't render and
    // the `<g>` only contained a `<desc>` which doesn't render either,
    // so the group ends up empty but still emitted as a Group node).
    assert_eq!(frame.root.children.len(), 1);
    if let oxideav_core::Node::Group(g) = &frame.root.children[0] {
        assert!(g.children.is_empty(), "empty group (desc doesn't render)");
    } else {
        panic!("expected Group for the <g>");
    }
}

#[test]
fn write_svg_without_extras_drops_descriptive() {
    // The bare `write_svg(&frame)` API uses an empty `PreservedExtras`
    // so descriptive content is dropped on the way out. This is the
    // pre-round-122 behaviour; callers who care use the `_with_extras`
    // API.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <title>Lost</title>
  <rect x="0" y="0" width="10" height="10" fill="#000000"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let bytes = write_svg(&frame);
    let s = String::from_utf8(bytes).unwrap();
    assert!(!s.contains("<title>"), "bare write_svg drops descriptive");
    assert!(!s.contains("Lost"));
}

#[test]
fn empty_title_round_trips() {
    // §5.8 authoring guidance says authors should not emit empty
    // title / desc, but tolerant round-trip means a hand-authored
    // empty element still re-emits cleanly. The encoder elects self-
    // closing for empty bodies (matches the rest of the encoder's
    // empty-element style).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <title></title>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.titles.len(), 1);
    let root = binding_at(&extras.titles, &[]);
    assert_eq!(root.items.len(), 1);
    assert_eq!(root.items[0].text, "");
    let bytes = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(bytes).unwrap();
    assert!(s.contains("<title/>"), "self-closing for empty body");
}
