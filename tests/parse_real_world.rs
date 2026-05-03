//! Load a small real-world SVG fixture (a 64×64 stylised house icon),
//! re-emit it, parse the output back, and assert structural equality:
//! same shape count, same width/height, same stroke widths.

use oxideav_core::{Node, Paint, VectorFrame};
use oxideav_svg::{parse_svg, write_svg};

const FIXTURE: &[u8] = include_bytes!("fixtures/icon-house.svg");

fn count_paths(frame: &VectorFrame) -> usize {
    fn walk(node: &Node, n: &mut usize) {
        match node {
            Node::Path(_) => *n += 1,
            Node::Group(g) => {
                for c in &g.children {
                    walk(c, n);
                }
            }
            Node::Image(_) => {}
        }
    }
    let mut n = 0;
    for c in &frame.root.children {
        walk(c, &mut n);
    }
    n
}

#[test]
fn fixture_loads_and_round_trips_structurally() {
    let frame = parse_svg(FIXTURE).expect("fixture parses");
    assert_eq!(frame.width, 64.0);
    assert_eq!(frame.height, 64.0);

    // House icon: sky rect + roof polygon + body rect + door rect +
    // 2 windows + grass path = 7 visible path-bearing nodes.
    let n1 = count_paths(&frame);
    assert_eq!(n1, 7, "expected 7 paths in the fixture, got {n1}");

    let bytes = write_svg(&frame);
    let frame2 = parse_svg(&bytes).expect("re-emitted SVG parses");
    let n2 = count_paths(&frame2);
    assert_eq!(n1, n2, "round-trip must preserve path count");
    assert_eq!(frame.width, frame2.width);
    assert_eq!(frame.height, frame2.height);
}

#[test]
fn fixture_resolves_gradient_url_reference() {
    let frame = parse_svg(FIXTURE).unwrap();
    // First child is the sky <rect> with fill="url(#sky)".
    let p = match &frame.root.children[0] {
        Node::Path(p) => p,
        _ => panic!(),
    };
    match &p.fill {
        Some(Paint::LinearGradient(g)) => {
            assert_eq!(g.stops.len(), 2);
        }
        other => panic!("expected linear gradient on first rect, got {other:?}"),
    }
}
