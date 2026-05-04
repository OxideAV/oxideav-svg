//! Round 3 — `<animate>` / `<set>` snapshot at `t=0`.
//!
//! The animation's `from` value (or first `values` entry, or `to`)
//! is folded into the parent element's attribute set so the static
//! render matches what most browsers paint on first frame.

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

/// Extract the solid `Rgba` from a `Paint::Solid` — `Paint` doesn't
/// derive `PartialEq` upstream, so we destructure for assertions.
fn solid_rgba(paint: Paint) -> Rgba {
    match paint {
        Paint::Solid(c) => c,
        other => panic!("expected Paint::Solid, got {other:?}"),
    }
}

#[test]
fn animate_from_overrides_static_attr_at_t0() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <rect x="0" y="0" width="50" height="50" fill="red">
    <animate attributeName="fill" from="#00ff00" to="#0000ff" dur="2s"/>
  </rect>
</svg>"##;
    let frame = parse_svg(src).expect("parse");
    let fill = first_path_fill(&frame).expect("fill resolved");
    // Snapshot at t=0 → from="#00ff00".
    assert_eq!(solid_rgba(fill), Rgba::opaque(0, 255, 0));
}

#[test]
fn animate_first_values_entry_used_when_no_from() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <rect x="0" y="0" width="50" height="50" fill="red">
    <animate attributeName="fill" values="#aabbcc;#112233;#445566" dur="3s"/>
  </rect>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(0xaa, 0xbb, 0xcc));
}

#[test]
fn animate_to_used_when_no_from_or_values() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <rect x="0" y="0" width="50" height="50" fill="red">
    <animate attributeName="fill" to="#ffaabb" dur="1s"/>
  </rect>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(0xff, 0xaa, 0xbb));
}

#[test]
fn set_overrides_at_t0() {
    // <set> is the simplest animation: assign a value, hold it.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <rect width="50" height="50" fill="red">
    <set attributeName="fill" to="#000000"/>
  </rect>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(0, 0, 0));
}

#[test]
fn animate_without_attribute_name_is_dropped() {
    // Malformed: no attributeName → no override; original fill survives.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
  <rect width="10" height="10" fill="red">
    <animate from="#000000" to="#ffffff" dur="1s"/>
  </rect>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid_rgba(fill), Rgba::opaque(255, 0, 0));
}

#[test]
fn animate_x_attribute_repositions_rect() {
    // The animation targets `x` — the snapshot should move the rect.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
  <rect x="0" y="0" width="20" height="20" fill="black">
    <animate attributeName="x" from="40" to="80" dur="2s"/>
  </rect>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    // Find the path's first MoveTo to see where the rect started.
    let first_x = match &frame.root.children[0] {
        Node::Path(p) => match p.path.commands.first() {
            Some(oxideav_core::PathCommand::MoveTo(pt)) => pt.x,
            _ => panic!("expected MoveTo"),
        },
        other => panic!("expected Path, got {:?}", other),
    };
    assert!(
        (first_x - 40.0).abs() < f32::EPSILON,
        "rect should be at x=40 (animate from), got {first_x}"
    );
}
