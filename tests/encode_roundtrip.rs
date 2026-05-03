//! Build a `VectorFrame` programmatically, write it as SVG, parse the
//! result back, and assert structural equality of the round-tripped
//! scene graph.

use oxideav_core::{
    FillRule, GradientStop, Group, LineCap, LineJoin, LinearGradient, Node, Paint, Path, PathNode,
    Point, Rgba, SpreadMethod, Stroke, TimeBase, Transform2D, VectorFrame, ViewBox,
};
use oxideav_svg::{parse_svg, write_svg};

fn red_triangle() -> PathNode {
    let mut path = Path::new();
    path.move_to(Point::new(0.0, 0.0));
    path.line_to(Point::new(10.0, 0.0));
    path.line_to(Point::new(5.0, 10.0));
    path.close();
    PathNode {
        path,
        fill: Some(Paint::Solid(Rgba::opaque(255, 0, 0))),
        stroke: Some(Stroke {
            width: 2.0,
            paint: Paint::Solid(Rgba::opaque(0, 0, 0)),
            cap: LineCap::Round,
            join: LineJoin::Bevel,
            miter_limit: 4.0,
            dash: None,
        }),
        fill_rule: FillRule::EvenOdd,
    }
}

#[test]
fn solid_paint_round_trip_preserves_shape_and_attrs() {
    let frame = VectorFrame {
        width: 20.0,
        height: 20.0,
        view_box: Some(ViewBox {
            min_x: 0.0,
            min_y: 0.0,
            width: 20.0,
            height: 20.0,
        }),
        root: Group {
            children: vec![Node::Path(red_triangle())],
            ..Group::default()
        },
        pts: None,
        time_base: TimeBase::new(1, 1),
    };

    let bytes = write_svg(&frame);
    let frame2 = parse_svg(&bytes).unwrap();

    assert_eq!(frame.width, frame2.width);
    assert_eq!(frame.height, frame2.height);
    assert_eq!(frame.root.children.len(), frame2.root.children.len());

    let p1 = match &frame.root.children[0] {
        Node::Path(p) => p,
        _ => panic!(),
    };
    let p2 = match &frame2.root.children[0] {
        Node::Path(p) => p,
        _ => panic!(),
    };
    assert_eq!(p1.path.commands, p2.path.commands);
    assert_eq!(p1.fill_rule, p2.fill_rule);
    // Stroke width / cap / join survive.
    let s1 = p1.stroke.as_ref().unwrap();
    let s2 = p2.stroke.as_ref().unwrap();
    assert_eq!(s1.width, s2.width);
    assert_eq!(s1.cap, s2.cap);
    assert_eq!(s1.join, s2.join);
}

#[test]
fn linear_gradient_round_trips_through_url_reference() {
    let lg = LinearGradient {
        start: Point::new(0.0, 0.0),
        end: Point::new(20.0, 0.0),
        stops: vec![
            GradientStop::new(0.0, Rgba::opaque(255, 0, 0)),
            GradientStop::new(1.0, Rgba::opaque(0, 0, 255)),
        ],
        spread: SpreadMethod::Pad,
    };

    let mut path = Path::new();
    path.move_to(Point::new(0.0, 0.0));
    path.line_to(Point::new(20.0, 0.0));
    path.line_to(Point::new(20.0, 20.0));
    path.close();

    let frame = VectorFrame {
        width: 20.0,
        height: 20.0,
        view_box: None,
        root: Group {
            children: vec![Node::Path(PathNode {
                path,
                fill: Some(Paint::LinearGradient(lg)),
                stroke: None,
                fill_rule: FillRule::NonZero,
            })],
            ..Group::default()
        },
        pts: None,
        time_base: TimeBase::new(1, 1),
    };

    let bytes = write_svg(&frame);
    let s = std::str::from_utf8(&bytes).unwrap();
    assert!(s.contains("<linearGradient"));
    assert!(s.contains("url(#grad1)"));

    let frame2 = parse_svg(&bytes).unwrap();
    let p2 = match &frame2.root.children[0] {
        Node::Path(p) => p,
        _ => panic!(),
    };
    match &p2.fill {
        Some(Paint::LinearGradient(g)) => {
            assert_eq!(g.stops.len(), 2);
            assert_eq!(g.stops[0].color, Rgba::opaque(255, 0, 0));
            assert_eq!(g.stops[1].color, Rgba::opaque(0, 0, 255));
        }
        other => panic!("expected linear gradient, got {other:?}"),
    }
}

#[test]
fn group_with_transform_survives() {
    let inner = PathNode {
        path: {
            let mut p = Path::new();
            p.move_to(Point::new(0.0, 0.0));
            p.line_to(Point::new(1.0, 1.0));
            p
        },
        fill: Some(Paint::Solid(Rgba::opaque(0, 128, 0))),
        stroke: None,
        fill_rule: FillRule::NonZero,
    };
    let group = Group {
        transform: Transform2D::translate(10.0, 20.0),
        opacity: 0.5,
        clip: None,
        children: vec![Node::Path(inner)],
        cache_key: None,
    };
    let frame = VectorFrame {
        width: 50.0,
        height: 50.0,
        view_box: None,
        root: Group {
            children: vec![Node::Group(group)],
            ..Group::default()
        },
        pts: None,
        time_base: TimeBase::new(1, 1),
    };
    let bytes = write_svg(&frame);
    let s = std::str::from_utf8(&bytes).unwrap();
    assert!(s.contains("<g"));
    assert!(s.contains("transform="));
    assert!(s.contains("opacity="));

    let frame2 = parse_svg(&bytes).unwrap();
    let g2 = match &frame2.root.children[0] {
        Node::Group(g) => g,
        _ => panic!(),
    };
    // Translation should be ~(10, 20).
    assert!((g2.transform.e - 10.0).abs() < 1e-3);
    assert!((g2.transform.f - 20.0).abs() < 1e-3);
    assert!((g2.opacity - 0.5).abs() < 1e-3);
}
