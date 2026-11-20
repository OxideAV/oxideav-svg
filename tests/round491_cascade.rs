//! Cascade fixes surfaced by rendering real-world icons: the outermost
//! `<svg>` takes part in the cascade, `opacity` is not inherited,
//! `currentColor` follows the `color` property, and the root size
//! honours CSS units.

use oxideav_svg::{Node, Paint, PathNode, SvgDocument, parse};

fn first_path(d: &SvgDocument) -> &PathNode {
    fn find(n: &Node) -> Option<&PathNode> {
        match n {
            Node::Path(p) => Some(p),
            Node::Group(g) => g.children.iter().find_map(find),
            _ => None,
        }
    }
    d.root.children.iter().find_map(find).expect("a path")
}

fn rgb(p: &Paint) -> (u8, u8, u8) {
    match p {
        Paint::Solid(c) => (c.r, c.g, c.b),
        other => panic!("expected a solid paint, got {other:?}"),
    }
}

#[test]
fn root_svg_presentation_attributes_are_inherited() {
    // The stroke-icon idiom: everything on the root.
    let d = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none"
  stroke="#cc0033" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="10"/></svg>"##,
    )
    .unwrap();
    let p = first_path(&d);
    assert!(p.fill.is_none(), "root fill=none must reach the circle");
    let s = p
        .stroke
        .as_ref()
        .expect("root stroke must reach the circle");
    assert_eq!(rgb(&s.paint), (0xcc, 0x00, 0x33));
    assert_eq!(s.width, 2.0);
    assert_eq!(s.cap, oxideav_svg::LineCap::Round);
}

#[test]
fn root_svg_css_and_opacity_apply() {
    let d = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" opacity="0.25"
  style="fill: #00f"><rect width="5" height="5"/></svg>"##,
    )
    .unwrap();
    assert_eq!(d.root.opacity, 0.25);
    assert_eq!(rgb(first_path(&d).fill.as_ref().unwrap()), (0, 0, 255));
}

#[test]
fn opacity_is_not_inherited() {
    let d = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
<g opacity="0.5"><g><rect width="5" height="5"/></g></g></svg>"##,
    )
    .unwrap();
    let Node::Group(outer) = &d.root.children[0] else {
        panic!("outer group")
    };
    assert_eq!(outer.opacity, 0.5);
    let Node::Group(inner) = &outer.children[0] else {
        panic!("inner group")
    };
    assert_eq!(inner.opacity, 1.0, "the nested group must not repeat 0.5");
}

#[test]
fn a_shape_keeps_its_own_opacity_under_an_equal_group_opacity() {
    let d = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
<g opacity="0.5"><rect width="5" height="5" opacity="0.5"/></g></svg>"##,
    )
    .unwrap();
    let Node::Group(outer) = &d.root.children[0] else {
        panic!("outer group")
    };
    // The rect's own 0.5 needs its own compositing group.
    let Node::Group(wrap) = &outer.children[0] else {
        panic!("the rect's opacity wrapper, got {:?}", outer.children[0]);
    };
    assert_eq!(wrap.opacity, 0.5);
}

#[test]
fn current_color_follows_the_color_property() {
    // `currentColor` set above, `color` set below: the paint follows
    // the `color` in effect on each element (SVG 2 §13.2).
    let d = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" fill="currentColor" stroke="currentColor">
<rect width="5" height="5" color="#ff0000"/>
<g style="color: rgb(0, 128, 0)"><rect width="5" height="5" stroke="blue"/></g>
<rect width="5" height="5" fill="currentColor" color="#0000ff"/>
</svg>"##,
    )
    .unwrap();
    let paths: Vec<&PathNode> = {
        fn all<'a>(n: &'a Node, out: &mut Vec<&'a PathNode>) {
            match n {
                Node::Path(p) => out.push(p),
                Node::Group(g) => g.children.iter().for_each(|c| all(c, out)),
                _ => {}
            }
        }
        let mut v = Vec::new();
        d.root.children.iter().for_each(|c| all(c, &mut v));
        v
    };
    assert_eq!(rgb(paths[0].fill.as_ref().unwrap()), (255, 0, 0));
    assert_eq!(rgb(&paths[0].stroke.as_ref().unwrap().paint), (255, 0, 0));
    assert_eq!(rgb(paths[1].fill.as_ref().unwrap()), (0, 128, 0));
    assert_eq!(rgb(&paths[1].stroke.as_ref().unwrap().paint), (0, 0, 255));
    // Attribute order on one element does not matter.
    assert_eq!(rgb(paths[2].fill.as_ref().unwrap()), (0, 0, 255));
    // With no `color` anywhere, currentColor is the initial black.
    let d = parse(br##"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1" fill="currentColor"/></svg>"##).unwrap();
    assert_eq!(rgb(first_path(&d).fill.as_ref().unwrap()), (0, 0, 0));
}

#[test]
fn root_size_units_are_css_pixels() {
    let d = parse(br##"<svg xmlns="http://www.w3.org/2000/svg" width="12cm" height="1in" viewBox="0 0 1000 300"/>"##).unwrap();
    assert!(
        (d.width - 453.5433).abs() < 1e-2 && (d.height - 96.0).abs() < 1e-4,
        "{} x {}",
        d.width,
        d.height
    );
    let d = parse(br##"<svg xmlns="http://www.w3.org/2000/svg" width="30pt" height="20mm"/>"##)
        .unwrap();
    assert!((d.width - 40.0).abs() < 1e-4 && (d.height - 75.590_55).abs() < 1e-3);
    // A percentage is not an intrinsic size: the viewBox is.
    let d = parse(br##"<svg xmlns="http://www.w3.org/2000/svg" width="100%" height="100%" viewBox="0 0 64 32"/>"##).unwrap();
    assert_eq!((d.width, d.height), (64.0, 32.0));
}
