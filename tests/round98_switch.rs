//! Round 98 — SVG 2 §5.7 conditional processing: the `<switch>`
//! element + `requiredFeatures` / `requiredExtensions` /
//! `systemLanguage` test attributes.
//!
//! The default user-language preference is `["en"]` (the global
//! [`oxideav_svg::set_system_languages`] hook is one-shot and left
//! unset here so these tests are deterministic across test binaries).

use oxideav_core::{Node, Paint, Rgba};
use oxideav_svg::{parse_svg, write_svg};

/// Walk to the single PathNode the document produced and return its
/// solid fill colour. Panics if the shape isn't a single filled path
/// (possibly wrapped in groups).
fn only_fill(frame: &oxideav_core::VectorFrame) -> Rgba {
    fn find(node: &Node) -> Option<Rgba> {
        match node {
            Node::Path(p) => match p.fill {
                Some(Paint::Solid(c)) => Some(c),
                _ => None,
            },
            Node::Group(g) => g.children.iter().find_map(find),
            Node::SoftMask { content, .. } => find(content),
            _ => None,
        }
    }
    frame
        .root
        .children
        .iter()
        .find_map(find)
        .expect("expected a single filled path")
}

fn count_paths(node: &Node) -> usize {
    match node {
        Node::Path(_) => 1,
        Node::Group(g) => g.children.iter().map(count_paths).sum(),
        Node::SoftMask { content, .. } => count_paths(content),
        _ => 0,
    }
}

#[test]
fn switch_renders_only_first_matching_branch() {
    // Branch 1 fails (systemLanguage="fr" vs default "en"); branch 2 is
    // the catch-all and wins. Branch 3 is never reached.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20">
  <switch>
    <rect systemLanguage="fr" x="0" y="0" width="20" height="20" fill="#ff0000"/>
    <rect x="0" y="0" width="20" height="20" fill="#00ff00"/>
    <rect x="0" y="0" width="20" height="20" fill="#0000ff"/>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    // Exactly one path renders.
    let total: usize = frame.root.children.iter().map(count_paths).sum();
    assert_eq!(total, 1, "switch must render exactly one branch");
    assert_eq!(only_fill(&frame), Rgba::opaque(0, 255, 0));
}

#[test]
fn switch_picks_system_language_prefix_match() {
    // Default pref "en" prefix-matches "en-GB"; that branch wins over
    // the later catch-all.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20">
  <switch>
    <rect systemLanguage="de" x="0" y="0" width="20" height="20" fill="#ff0000"/>
    <rect systemLanguage="en-GB" x="0" y="0" width="20" height="20" fill="#00ff00"/>
    <rect x="0" y="0" width="20" height="20" fill="#0000ff"/>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(only_fill(&frame), Rgba::opaque(0, 255, 0));
}

#[test]
fn switch_required_extensions_always_fail() {
    // No extensions are supported, so the first branch is bypassed; the
    // bare second branch renders.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20">
  <switch>
    <rect requiredExtensions="http://example.org/Ext/1.0" x="0" y="0" width="20" height="20" fill="#ff0000"/>
    <rect x="0" y="0" width="20" height="20" fill="#00ff00"/>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(only_fill(&frame), Rgba::opaque(0, 255, 0));
}

#[test]
fn switch_required_features_pass_when_nonempty() {
    // requiredFeatures is removed in SVG 2 / always-true in modern UAs;
    // a non-empty list passes, so the first branch wins.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20">
  <switch>
    <rect requiredFeatures="http://www.w3.org/TR/SVG11/feature#Shape" x="0" y="0" width="20" height="20" fill="#ff0000"/>
    <rect x="0" y="0" width="20" height="20" fill="#00ff00"/>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(only_fill(&frame), Rgba::opaque(255, 0, 0));
}

#[test]
fn switch_with_no_matching_branch_renders_nothing() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20">
  <switch>
    <rect systemLanguage="fr" x="0" y="0" width="20" height="20" fill="#ff0000"/>
    <rect requiredExtensions="http://example.org/X" x="0" y="0" width="20" height="20" fill="#00ff00"/>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let total: usize = frame.root.children.iter().map(count_paths).sum();
    assert_eq!(total, 0, "no branch matches → nothing renders");
}

#[test]
fn switch_selects_container_subtree() {
    // The chosen branch is a `<g>` with two shapes; the entire subtree
    // renders per §5.7.3 ("the entire subtree is either processed/
    // rendered or bypassed").
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20">
  <switch>
    <g systemLanguage="fr">
      <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
    </g>
    <g>
      <rect x="0" y="0" width="10" height="10" fill="#00ff00"/>
      <circle cx="15" cy="15" r="5" fill="#0000ff"/>
    </g>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let total: usize = frame.root.children.iter().map(count_paths).sum();
    assert_eq!(total, 2, "the matching <g> subtree renders both shapes");
}

#[test]
fn switch_honours_own_transform_on_round_trip() {
    // The `<switch>`'s own `transform` survives by collapsing into the
    // wrapping `<g>` of the static snapshot, so a parse → write → parse
    // round-trip preserves the single rendered branch.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" viewBox="0 0 40 40">
  <switch transform="translate(5,5)">
    <rect systemLanguage="zz" x="0" y="0" width="10" height="10" fill="#ff0000"/>
    <rect x="0" y="0" width="10" height="10" fill="#00ff00"/>
  </switch>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let out = write_svg(&frame);
    let frame2 = parse_svg(&out).unwrap();
    assert_eq!(only_fill(&frame2), Rgba::opaque(0, 255, 0));
    let total: usize = frame2.root.children.iter().map(count_paths).sum();
    assert_eq!(total, 1);
}
