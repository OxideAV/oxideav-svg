//! Round 115 — SVG 2 §16.5 `<a>` hyperlink element.
//!
//! `<a>` is categorised as both a *container element* and a *renderable
//! element*: it groups and renders its children exactly like `<g>`
//! (transform / opacity / paint cascade) and additionally establishes a
//! hyperlink. Round 115 renders the children into a `Node::Group` and
//! preserves the hyperlink target + its HTML companion attributes via
//! the `PreservedExtras::links` side-channel so a
//! `parse_svg_with_extras → write_svg_with_extras` round-trip re-wraps
//! the group in its `<a href="…">…</a>` element.

use oxideav_core::{Node, Paint, PathNode, Rgba};
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Find every `Node::Path` in the scene graph, in pre-order.
fn all_paths(frame: &oxideav_core::VectorFrame) -> Vec<&PathNode> {
    fn walk<'a>(n: &'a Node, out: &mut Vec<&'a PathNode>) {
        match n {
            Node::Path(p) => out.push(p),
            Node::Group(g) => {
                for c in &g.children {
                    walk(c, out);
                }
            }
            Node::SoftMask { content, .. } => walk(content, out),
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
fn anchor_renders_its_children() {
    // The shape inside `<a>` must still appear in the scene graph
    // (round 114 and earlier dropped the whole `<a>` subtree).
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <a href="https://example.com/">
    <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
  </a>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1, "<a> must render its child shape");
    let c = fill_rgba(paths[0]).expect("solid fill");
    assert_eq!((c.r, c.g, c.b), (0xff, 0x00, 0x00));
}

#[test]
fn anchor_is_a_group_node() {
    // `<a>` produces a `Node::Group` (it's a container element), so its
    // direct scene child is a group, not a bare path.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <a href="#frag">
    <circle cx="5" cy="5" r="3" fill="#00ff00"/>
    <rect x="0" y="0" width="2" height="2" fill="#0000ff"/>
  </a>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(frame.root.children.len(), 1);
    match &frame.root.children[0] {
        Node::Group(g) => assert_eq!(g.children.len(), 2, "both children render"),
        other => panic!("expected Group for <a>, got {other:?}"),
    }
}

#[test]
fn anchor_transform_applies_to_group() {
    // `transform` on `<a>` is a presentation property (§8.5) and lands
    // on the group, like `<g transform=...>`.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <a href="https://x.test/" transform="translate(10 20)">
    <rect x="0" y="0" width="5" height="5" fill="#abcdef"/>
  </a>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    match &frame.root.children[0] {
        Node::Group(g) => assert!(
            !g.transform.is_identity(),
            "<a transform=...> must land on the group transform"
        ),
        other => panic!("expected Group, got {other:?}"),
    }
}

#[test]
fn anchor_opacity_applies_to_group() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <a href="https://x.test/" opacity="0.5">
    <rect x="0" y="0" width="5" height="5" fill="#abcdef"/>
  </a>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    match &frame.root.children[0] {
        Node::Group(g) => assert!(
            (g.opacity - 0.5).abs() < 1e-4,
            "expected opacity 0.5, got {}",
            g.opacity
        ),
        other => panic!("expected Group, got {other:?}"),
    }
}

#[test]
fn anchor_link_binding_captured() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <a href="https://example.com/page" target="_blank" rel="noopener" type="text/html">
    <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
  </a>
</svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.links.len(), 1, "one <a> binding captured");
    let link = &extras.links[0];
    assert_eq!(link.href.as_deref(), Some("https://example.com/page"));
    assert_eq!(link.target.as_deref(), Some("_blank"));
    assert_eq!(link.rel.as_deref(), Some("noopener"));
    assert_eq!(link.type_.as_deref(), Some("text/html"));
    assert_eq!(link.download, None);
}

#[test]
fn anchor_xlink_href_fallback() {
    // SVG 1.1 documents use the deprecated `xlink:href`.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="100" viewBox="0 0 100 100">
  <a xlink:href="https://legacy.test/">
    <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
  </a>
</svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.links.len(), 1);
    assert_eq!(
        extras.links[0].href.as_deref(),
        Some("https://legacy.test/"),
        "xlink:href must populate the href binding"
    );
}

#[test]
fn anchor_href_wins_over_xlink_href() {
    // When both are present the SVG-2 `href` wins per §16.5.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="100" viewBox="0 0 100 100">
  <a href="https://new.test/" xlink:href="https://old.test/">
    <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
  </a>
</svg>"##;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.links[0].href.as_deref(), Some("https://new.test/"));
}

#[test]
fn anchor_roundtrips_through_extras() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <a href="https://example.com/" target="_blank">
    <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
  </a>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_str = String::from_utf8(out).unwrap();
    assert!(
        out_str.contains("<a href=\"https://example.com/\" target=\"_blank\">"),
        "round-trip must re-emit the <a> wrapper:\n{out_str}"
    );
    assert!(out_str.contains("</a>"), "must close the <a>");
    // Re-parse: the link survives a full cycle.
    let (_frame2, extras2) = parse_svg_with_extras(out_str.as_bytes()).unwrap();
    assert_eq!(extras2.links.len(), 1);
    assert_eq!(
        extras2.links[0].href.as_deref(),
        Some("https://example.com/")
    );
    assert_eq!(extras2.links[0].target.as_deref(), Some("_blank"));
}

#[test]
fn nested_anchor_link_outside_group() {
    // The `<a>` lives inside a `<g transform>`; its tree-path differs
    // from the root, so the encoder must still re-wrap the right group.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <g transform="translate(1 1)">
    <a href="https://deep.test/">
      <rect x="0" y="0" width="4" height="4" fill="#101010"/>
    </a>
  </g>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.links.len(), 1);
    // Path is [0, 0]: root child 0 is the <g>, whose child 0 is the <a>.
    assert_eq!(extras.links[0].path, vec![0usize, 0usize]);
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    assert!(
        out.contains("<a href=\"https://deep.test/\">"),
        "nested <a> must round-trip:\n{out}"
    );
}

#[test]
fn bare_anchor_without_href_still_groups() {
    // A bare `<a>` (no href) still groups its children; the encoder
    // re-emits `<a>` with no attributes.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <a>
    <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
  </a>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(all_paths(&frame).len(), 1, "child still renders");
    let (frame2, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.links.len(), 1);
    assert_eq!(extras.links[0].href, None);
    let out = String::from_utf8(write_svg_with_extras(&frame2, &extras)).unwrap();
    assert!(out.contains("<a>"), "bare <a> round-trips as <a>:\n{out}");
}

#[test]
fn anchor_all_link_attributes_roundtrip() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <a href="https://example.com/file.pdf" target="_top" download="report.pdf" ping="https://t.test/p" rel="noreferrer" hreflang="en" type="application/pdf" referrerpolicy="no-referrer">
    <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
  </a>
</svg>"##;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let link = &extras.links[0];
    assert_eq!(link.download.as_deref(), Some("report.pdf"));
    assert_eq!(link.ping.as_deref(), Some("https://t.test/p"));
    assert_eq!(link.rel.as_deref(), Some("noreferrer"));
    assert_eq!(link.hreflang.as_deref(), Some("en"));
    assert_eq!(link.type_.as_deref(), Some("application/pdf"));
    assert_eq!(link.referrerpolicy.as_deref(), Some("no-referrer"));
    let out = String::from_utf8(write_svg_with_extras(&frame, &extras)).unwrap();
    for needle in [
        "download=\"report.pdf\"",
        "ping=\"https://t.test/p\"",
        "rel=\"noreferrer\"",
        "hreflang=\"en\"",
        "type=\"application/pdf\"",
        "referrerpolicy=\"no-referrer\"",
        "target=\"_top\"",
    ] {
        assert!(out.contains(needle), "missing {needle} in:\n{out}");
    }
}

#[test]
fn anchor_with_multiple_children_groups_all() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <a href="https://x.test/">
    <rect x="0" y="0" width="2" height="2" fill="#ff0000"/>
    <circle cx="5" cy="5" r="1" fill="#00ff00"/>
    <line x1="0" y1="0" x2="9" y2="9" stroke="#0000ff"/>
  </a>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(all_paths(&frame).len(), 3, "all three shapes render");
}
