//! Round 4 — CSS cascade.
//!
//! Verifies that `<style>` blocks + `style="..."` inline declarations
//! are folded into the resolved presentation state per SVG 1.1 §6.

use oxideav_core::{Node, Paint, Rgba};
use oxideav_svg::parse_svg;

fn first_path_fill(frame: &oxideav_core::VectorFrame) -> Option<Paint> {
    fn find(g: &oxideav_core::Group) -> Option<Paint> {
        for c in &g.children {
            match c {
                Node::Path(p) => return p.fill.clone(),
                Node::Group(sg) => {
                    if let Some(p) = find(sg) {
                        return Some(p);
                    }
                }
                _ => {}
            }
        }
        None
    }
    find(&frame.root)
}

fn solid_rgba(p: Paint) -> Rgba {
    match p {
        Paint::Solid(c) => c,
        other => panic!("expected Paint::Solid, got {other:?}"),
    }
}

#[test]
fn class_selector_applies_fill() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
  <defs>
    <style>.red { fill: #ff0000 }</style>
  </defs>
  <rect class="red" x="0" y="0" width="20" height="20"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(255, 0, 0));
}

#[test]
fn id_selector_outranks_class() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
  <defs>
    <style>
      .red { fill: #ff0000 }
      #target { fill: #00ff00 }
    </style>
  </defs>
  <rect class="red" id="target" x="0" y="0" width="20" height="20"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(0, 255, 0));
}

#[test]
fn inline_style_overrides_stylesheet() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
  <defs>
    <style>.x { fill: #ff0000 }</style>
  </defs>
  <rect class="x" style="fill: #000000" x="0" y="0" width="20" height="20"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(0, 0, 0));
}

#[test]
fn tag_selector_matches() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <style>rect { fill: #123456 }</style>
  <rect width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(0x12, 0x34, 0x56));
}

#[test]
fn comma_selector_lists_multiple_targets() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
  <defs>
    <style>rect, circle { fill: #abcdef }</style>
  </defs>
  <rect x="0" y="0" width="10" height="10"/>
  <circle cx="15" cy="5" r="5"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    // Both shapes parse — both are filled.
    assert_eq!(frame.root.children.len(), 2);
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(0xab, 0xcd, 0xef));
}

#[test]
fn style_block_outside_defs_still_works() {
    // <style> need not live inside <defs> — the cascade walks the whole
    // document.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <style>rect { fill: red }</style>
  <rect width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(255, 0, 0));
}

#[test]
fn comments_in_style_block_dont_break_parsing() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <style>
    /* big comment */
    rect /* between */ { fill: #335577 /* trailing */ }
  </style>
  <rect width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(0x33, 0x55, 0x77));
}

#[test]
fn cdata_wrapped_style_body_parses() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <style><![CDATA[.x { fill: #ffffff }]]></style>
  <rect class="x" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(255, 255, 255));
}

#[test]
fn inherits_cascaded_value_to_child_through_group() {
    // CSS on a <g> propagates to its <rect> child.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <style>g { fill: #abcdef }</style>
  <g>
    <rect width="10" height="10"/>
  </g>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(0xab, 0xcd, 0xef));
}

#[test]
fn presentation_attr_lower_priority_than_css_rule() {
    // SVG 1.1 §6.4: a `style="..."` declaration wins over the
    // presentation attribute. A class-selector rule ALSO wins because
    // (a) presentation attrs are treated as the lowest-specificity
    // entry in the cascade and (b) the selector specificity beats it.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <style>.x { fill: #00ff00 }</style>
  <rect class="x" fill="#ff0000" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(0, 255, 0));
}

#[test]
fn unknown_property_is_ignored_not_fatal() {
    // `font-family` isn't modelled by the round-4 paint state — we
    // should silently drop it, not error out.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <style>.x { font-family: "Helvetica"; fill: #321 }</style>
  <rect class="x" width="10" height="10"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(0x33, 0x22, 0x11));
}
