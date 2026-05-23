//! Round 98 — SVG 2 §5.7.3 `<switch>` conditional processing.
//!
//! The `<switch>` element evaluates the `requiredExtensions` (§5.7.4)
//! and `systemLanguage` (§5.7.5) test attributes on its direct child
//! elements in order, and renders the first child for which all of
//! these attributes test true; all others are bypassed.

use oxideav_core::{Paint, PathNode, Rgba};
use oxideav_svg::{parse_svg, parse_svg_at_with_languages};

/// Find every `Node::Path` in the scene graph, in pre-order.
fn all_paths(frame: &oxideav_core::VectorFrame) -> Vec<&PathNode> {
    fn walk<'a>(n: &'a oxideav_core::Node, out: &mut Vec<&'a PathNode>) {
        match n {
            oxideav_core::Node::Path(p) => out.push(p),
            oxideav_core::Node::Group(g) => {
                for c in &g.children {
                    walk(c, out);
                }
            }
            oxideav_core::Node::SoftMask { content, .. } => walk(content, out),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &frame.root.children {
        walk(c, &mut out);
    }
    out
}

fn fill_rgba(p: &PathNode) -> Option<Rgba> {
    match &p.fill {
        Some(Paint::Solid(c)) => Some(*c),
        _ => None,
    }
}

#[test]
fn switch_renders_only_first_passing_child() {
    // Three rects, none with conditional attributes. §5.7.3: render the
    // FIRST child only; bypass the rest.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <switch>
    <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
    <rect x="0" y="0" width="10" height="10" fill="#00ff00"/>
    <rect x="0" y="0" width="10" height="10" fill="#0000ff"/>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1, "switch must render exactly one child");
    let c = fill_rgba(paths[0]).expect("solid fill");
    assert_eq!((c.r, c.g, c.b), (0xff, 0x00, 0x00), "first child wins");
}

#[test]
fn switch_skips_failing_required_extensions() {
    // First child requires an unsupported extension (→ false); the
    // second child has no test attribute (→ true) and must be chosen.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <switch>
    <rect width="10" height="10" fill="#ff0000" requiredExtensions="http://example.org/Ext/1.0"/>
    <rect width="10" height="10" fill="#00ff00"/>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1);
    let c = fill_rgba(paths[0]).unwrap();
    assert_eq!((c.r, c.g, c.b), (0x00, 0xff, 0x00), "fallback child wins");
}

#[test]
fn switch_empty_required_extensions_is_false() {
    // §5.7.4: a null/empty string value evaluates to "false".
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <switch>
    <rect width="10" height="10" fill="#ff0000" requiredExtensions=""/>
    <rect width="10" height="10" fill="#00ff00"/>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1);
    let c = fill_rgba(paths[0]).unwrap();
    assert_eq!((c.r, c.g, c.b), (0x00, 0xff, 0x00));
}

#[test]
fn switch_system_language_picks_matching_locale() {
    // user prefers "fr"; the French-tagged child must win even though a
    // Maori-tagged child precedes it.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <switch>
    <rect width="10" height="10" fill="#ff0000" systemLanguage="mi"/>
    <rect width="10" height="10" fill="#00ff00" systemLanguage="fr"/>
    <rect width="10" height="10" fill="#0000ff"/>
  </switch>
</svg>"##;
    let frame = parse_svg_at_with_languages(src, 0.0, &["fr"]).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1);
    let c = fill_rgba(paths[0]).unwrap();
    assert_eq!((c.r, c.g, c.b), (0x00, 0xff, 0x00), "fr child wins");
}

#[test]
fn switch_system_language_prefix_match() {
    // §5.7.5: user "en" is a case-insensitive prefix of attribute
    // "en-US" with a "-" boundary, so it matches.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <switch>
    <rect width="10" height="10" fill="#ff0000" systemLanguage="en-US"/>
    <rect width="10" height="10" fill="#00ff00"/>
  </switch>
</svg>"##;
    let frame = parse_svg_at_with_languages(src, 0.0, &["en"]).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1);
    let c = fill_rgba(paths[0]).unwrap();
    assert_eq!(
        (c.r, c.g, c.b),
        (0xff, 0x00, 0x00),
        "en prefix-matches en-US"
    );
}

#[test]
fn switch_falls_through_to_catch_all_when_no_language_matches() {
    // No user-preferred language → both language-tagged children test
    // false; the un-tagged "catch-all" child (spec authoring note) wins.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <switch>
    <rect width="10" height="10" fill="#ff0000" systemLanguage="fr"/>
    <rect width="10" height="10" fill="#00ff00" systemLanguage="de"/>
    <rect width="10" height="10" fill="#0000ff"/>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1);
    let c = fill_rgba(paths[0]).unwrap();
    assert_eq!((c.r, c.g, c.b), (0x00, 0x00, 0xff), "catch-all wins");
}

#[test]
fn switch_renders_nothing_when_no_child_passes() {
    // §5.7.5 authoring note: with no catch-all and no matching language,
    // nothing renders.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <switch>
    <rect width="10" height="10" fill="#ff0000" systemLanguage="fr"/>
    <rect width="10" height="10" fill="#00ff00" systemLanguage="de"/>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert!(paths.is_empty(), "no passing child → no rendered path");
}

#[test]
fn switch_selects_group_subtree_whole() {
    // §5.7.3: "If the child element is a container element such as a `g`,
    // then the entire subtree is either processed/rendered or
    // bypassed/not rendered."
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <switch>
    <g requiredExtensions="urn:nope">
      <rect width="10" height="10" fill="#ff0000"/>
    </g>
    <g>
      <rect width="10" height="10" fill="#00ff00"/>
      <rect width="10" height="10" fill="#0000ff"/>
    </g>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    // The first <g> is bypassed; the second <g> renders BOTH its rects.
    assert_eq!(paths.len(), 2, "whole second-group subtree renders");
    let c0 = fill_rgba(paths[0]).unwrap();
    let c1 = fill_rgba(paths[1]).unwrap();
    assert_eq!((c0.r, c0.g, c0.b), (0x00, 0xff, 0x00));
    assert_eq!((c1.r, c1.g, c1.b), (0x00, 0x00, 0xff));
}

#[test]
fn switch_skips_never_rendered_children_without_consuming_slot() {
    // §5.7.1: conditional processing "does not affect the processing of
    // a `style` or `script` element". A leading <style> / <defs> must
    // NOT be treated as the rendered choice — the first renderable
    // passing child (the rect) wins.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <switch>
    <style>.x { fill: black; }</style>
    <desc>a description</desc>
    <rect width="10" height="10" fill="#00ff00"/>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1);
    let c = fill_rgba(paths[0]).unwrap();
    assert_eq!((c.r, c.g, c.b), (0x00, 0xff, 0x00));
}
