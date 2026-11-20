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

#[test]
fn use_element_css_is_inherited_by_the_instance() {
    // SVG 2 §5.6 example: the re-used circle takes the fill and stroke
    // the `use` rule sets (orange / purple); `.special circle` matches
    // only the original, which sits inside the `.special` group.
    let d = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
  <style>
    circle          { stroke-opacity: 0.7; }
    .special circle { stroke: green; }
    use             { stroke: purple; fill: orange; }
  </style>
  <g class="special" style="fill: blue"><circle id="c" cy="50" cx="50" r="40" stroke-width="20"/></g>
  <use href="#c" x="100"/>
</svg>"##,
    )
    .unwrap();
    fn all<'a>(n: &'a Node, out: &mut Vec<&'a PathNode>) {
        match n {
            Node::Path(p) => out.push(p),
            Node::Group(g) => g.children.iter().for_each(|c| all(c, out)),
            _ => {}
        }
    }
    let mut paths = Vec::new();
    d.root.children.iter().for_each(|c| all(c, &mut paths));
    assert_eq!(paths.len(), 2);
    assert_eq!(rgb(paths[0].fill.as_ref().unwrap()), (0, 0, 255));
    assert_eq!(rgb(&paths[0].stroke.as_ref().unwrap().paint), (0, 128, 0));
    assert_eq!(rgb(paths[1].fill.as_ref().unwrap()), (255, 165, 0));
    let s = paths[1].stroke.as_ref().unwrap();
    assert_eq!(rgb(&s.paint), (128, 0, 128));
    assert_eq!(s.width, 20.0);
    let Paint::Solid(c) = &s.paint else {
        unreachable!()
    };
    assert!((177..=180).contains(&c.a), "stroke-opacity 0.7: {}", c.a);
}

#[test]
fn stroke_width_takes_units_and_percentages() {
    // SVG 2 §8.9 "Units" example: every row of a column draws the same
    // stroke.
    let d = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="400px" height="200px" viewBox="0 0 4000 2000">
<g stroke="red" font-size="150">
  <rect width="4in" height="2in" stroke-width=".4in"/>
  <rect width="2.5em" height="1.25em" stroke-width=".25em"/>
  <rect width="10%" height="10%" stroke-width="1%"/>
</g></svg>"##,
    )
    .unwrap();
    fn all<'a>(n: &'a Node, out: &mut Vec<&'a PathNode>) {
        match n {
            Node::Path(p) => out.push(p),
            Node::Group(g) => g.children.iter().for_each(|c| all(c, out)),
            _ => {}
        }
    }
    let mut paths = Vec::new();
    d.root.children.iter().for_each(|c| all(c, &mut paths));
    let widths: Vec<f32> = paths
        .iter()
        .map(|p| p.stroke.as_ref().unwrap().width)
        .collect();
    assert!((widths[0] - 38.4).abs() < 1e-3, "{widths:?}");
    assert!((widths[1] - 37.5).abs() < 1e-3, "{widths:?}");
    // 1 % of the normalised viewBox diagonal sqrt((4000² + 2000²) / 2).
    assert!((widths[2] - 31.62).abs() < 0.01, "{widths:?}");
    // Percentage geometry resolves against the viewBox, not the 400 px
    // canvas: 10 % of 4000 × 10 % of 2000.
    let b = oxideav_svg::bbox::BBox::of_path(&paths[2].path).unwrap();
    assert!(
        (b.width - 400.0).abs() < 1e-3 && (b.height - 200.0).abs() < 1e-3,
        "{b:?}"
    );
}

#[test]
fn nested_svg_clips_to_its_viewport() {
    let d = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
<svg x="10" y="20" width="80" height="40" viewBox="0 0 10 10" preserveAspectRatio="xMidYMid slice"><rect width="10" height="10"/></svg>
<svg x="100" width="50" height="50" overflow="visible"><rect width="80" height="80"/></svg>
</svg>"##,
    )
    .unwrap();
    let Node::Group(sliced) = &d.root.children[0] else {
        panic!("nested svg group")
    };
    // The 80 × 40 viewport in the content's coordinates: slice scales
    // by 8, so 10 units wide and 5 tall, centred vertically.
    let clip = sliced
        .clip
        .as_ref()
        .expect("a nested <svg> clips to its viewport");
    let b = oxideav_svg::bbox::BBox::of_path(clip).unwrap();
    assert!(
        (b.x - 0.0).abs() < 1e-4 && (b.y - 2.5).abs() < 1e-4,
        "{b:?}"
    );
    assert!(
        (b.width - 10.0).abs() < 1e-4 && (b.height - 5.0).abs() < 1e-4,
        "{b:?}"
    );
    let Node::Group(open) = &d.root.children[1] else {
        panic!("nested svg group")
    };
    assert!(open.clip.is_none(), "overflow=visible lifts the clip");
}

#[test]
fn target_pseudo_class_does_not_match_a_static_document() {
    // SVG 2 §16.3 example: `#target:target { fill: red }` only applies
    // when the URL's fragment names the element.
    let d = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="30">
<style>#target:target { fill: red; }</style>
<circle id="target" stroke="red" stroke-width="5" fill="none" cx="25" cy="15" r="12"/></svg>"##,
    )
    .unwrap();
    assert!(first_path(&d).fill.is_none());
}

#[test]
fn a_sizeless_document_resolves_percentages_against_the_default_viewport() {
    let d = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg"><rect width="100%" height="100%"/></svg>"##,
    )
    .unwrap();
    assert_eq!(
        (d.width, d.height),
        (0.0, 0.0),
        "no intrinsic size is still reported"
    );
    let b = oxideav_svg::bbox::BBox::of_path(&first_path(&d).path).unwrap();
    assert_eq!((b.width, b.height), (300.0, 150.0));
}
