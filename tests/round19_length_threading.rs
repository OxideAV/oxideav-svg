//! Round 19 — end-to-end length-resolution wiring.
//!
//! Round 18 added the typed [`oxideav_svg::length::Length`] surface
//! and verified the unit factors / resolve-context plumbing in
//! isolation. Round 19 wires the typed parser through `element.rs` /
//! `decoder.rs` so per-element coordinate parsing actually consults
//! the per-element [`oxideav_svg::length::ResolveContext`] —
//! covering:
//!
//!   - root viewport seeds `vw` / `vh` / `vmin` / `vmax`;
//!   - root font-size seeds `em` / `rem` (default 16 px per CSS Values
//!     L4 §6.1.2; explicit `<svg font-size="...">` cascades);
//!   - per-element `font-size` cascade — a `<g font-size="32">`
//!     re-bases its descendants' `em` resolution to 32 px;
//!   - axis-specific percentage basis per SVG 2 §7.10 — `width="50%"`
//!     resolves against the viewport width, `height="50%"` against
//!     the viewport height, `r="50%"` against the diagonal.
//!
//! Bare-numeric coordinate values (`<rect x="100">`) round-trip
//! bit-for-bit identical to the round-1 path because
//! [`oxideav_svg::length::Length::resolve`] is the identity for
//! [`oxideav_svg::length::LengthUnit::UserUnit`].

use oxideav_core::{Node, PathCommand, Point};
use oxideav_svg::parse_svg;

/// Walk the scene graph and find the first `Path` (skipping any
/// wrapping `Group`s the encoder inserts for transform / opacity).
fn first_path(node: &Node) -> &oxideav_core::Path {
    match node {
        Node::Path(p) => &p.path,
        Node::Group(g) => {
            for c in &g.children {
                if let Some(p) = try_first_path(c) {
                    return p;
                }
            }
            panic!("no path under group");
        }
        other => panic!("unexpected node {other:?}"),
    }
}

fn try_first_path(node: &Node) -> Option<&oxideav_core::Path> {
    match node {
        Node::Path(p) => Some(&p.path),
        Node::Group(g) => g.children.iter().find_map(try_first_path),
        _ => None,
    }
}

#[test]
fn rect_em_resolves_against_default_font_size_on_root() {
    // Without an explicit `font-size` on the root, the spec default
    // of 16 px applies — `1em` → 16 px.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
        <rect x="1em" y="0" width="2em" height="1em"/>
    </svg>"#;
    let frame = parse_svg(src).unwrap();
    assert_eq!(frame.root.children.len(), 1);
    let path = first_path(&frame.root.children[0]);
    // Rect emits MoveTo(x, y) first, then LineTo(x+w, y).
    match path.commands[0] {
        PathCommand::MoveTo(p) => {
            assert!((p.x - 16.0).abs() < 1e-3, "rect.x: got {}", p.x);
        }
        _ => panic!("expected MoveTo"),
    }
    match path.commands[1] {
        PathCommand::LineTo(p) => {
            // x + w = 16 + 32 = 48.
            assert!((p.x - 48.0).abs() < 1e-3, "rect.x+w: got {}", p.x);
        }
        _ => panic!("expected LineTo"),
    }
}

#[test]
fn group_font_size_cascades_to_descendant_em_resolution() {
    // <g font-size="32"> overrides the root's default 16 px for its
    // descendants' `em` — `<rect width="2em">` resolves to 64.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
        <g font-size="32">
            <rect x="0" y="0" width="2em" height="1em"/>
        </g>
    </svg>"#;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame.root.children[0]);
    // Find the LineTo after MoveTo — its x is x+w = 64.
    match path.commands[1] {
        PathCommand::LineTo(p) => {
            assert!(
                (p.x - 64.0).abs() < 1e-3,
                "group em-cascade x+w: got {}",
                p.x
            );
        }
        _ => panic!("expected LineTo"),
    }
}

#[test]
fn outer_em_unaffected_when_inner_group_overrides() {
    // Sibling rect outside the `<g>` keeps the root's 16 px em basis.
    // `<rect width="2em">` outside the group → 32 px;
    // `<rect width="2em">` inside the `<g font-size="40">` → 80 px.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="500" height="500">
        <rect x="0" y="0" width="2em" height="1em"/>
        <g font-size="40">
            <rect x="0" y="100" width="2em" height="1em"/>
        </g>
    </svg>"#;
    let frame = parse_svg(src).unwrap();
    assert_eq!(frame.root.children.len(), 2);
    let outer = first_path(&frame.root.children[0]);
    match outer.commands[1] {
        PathCommand::LineTo(p) => {
            assert!((p.x - 32.0).abs() < 1e-3, "outer rect x+w: got {}", p.x);
        }
        _ => panic!("expected LineTo"),
    }
    let inner = first_path(&frame.root.children[1]);
    match inner.commands[1] {
        PathCommand::LineTo(p) => {
            assert!((p.x - 80.0).abs() < 1e-3, "inner rect x+w: got {}", p.x);
        }
        _ => panic!("expected LineTo"),
    }
}

#[test]
fn percent_resolves_against_viewport_axis() {
    // <circle cx="50%"> at viewport_w=200 → cx=100 (X axis).
    // <circle cy="50%"> at viewport_h=80  → cy=40  (Y axis).
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="80">
        <circle cx="50%" cy="50%" r="10"/>
    </svg>"#;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame.root.children[0]);
    // First MoveTo of the ellipse-as-circle path is at (cx + r, cy).
    match path.commands[0] {
        PathCommand::MoveTo(p) => {
            // cx + r = 100 + 10 = 110; cy = 40.
            assert!((p.x - 110.0).abs() < 1e-3, "circle.x: got {}", p.x);
            assert!((p.y - 40.0).abs() < 1e-3, "circle.y: got {}", p.y);
        }
        _ => panic!("expected MoveTo"),
    }
}

#[test]
fn vw_vh_resolve_against_root_viewport() {
    // <rect width="10vw"> at viewport_w=400 → width=40.
    // <rect height="50vh"> at viewport_h=200 → height=100.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="200">
        <rect x="0" y="0" width="10vw" height="50vh"/>
    </svg>"#;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame.root.children[0]);
    // Rect path: M(x,y) L(x+w, y) L(x+w, y+h) L(x, y+h) Z
    // commands[1] = LineTo(x+w, y); commands[2] = LineTo(x+w, y+h).
    match path.commands[1] {
        PathCommand::LineTo(p) => {
            assert!((p.x - 40.0).abs() < 1e-3, "x+w: got {}", p.x);
        }
        _ => panic!("expected LineTo"),
    }
    match path.commands[2] {
        PathCommand::LineTo(p) => {
            assert!((p.y - 100.0).abs() < 1e-3, "y+h: got {}", p.y);
        }
        _ => panic!("expected LineTo"),
    }
}

#[test]
fn root_font_size_cascades_to_rem() {
    // <svg font-size="20"> sets the root font-size — every
    // descendant's `1rem` resolves to 20.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200" font-size="20">
        <g font-size="40">
            <rect x="0" y="0" width="2rem" height="1rem"/>
        </g>
    </svg>"#;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame.root.children[0]);
    // 2rem against root font-size 20 → 40, INDEPENDENT of the
    // bracketing `<g font-size="40">` (which only changes `em`).
    match path.commands[1] {
        PathCommand::LineTo(p) => {
            assert!((p.x - 40.0).abs() < 1e-3, "rem-cascade x+w: got {}", p.x);
        }
        _ => panic!("expected LineTo"),
    }
}

#[test]
fn bare_numeric_coords_round_trip_unchanged() {
    // The round-1..18 numeric path must stay bit-for-bit. Any drift
    // here would break every SVG fixture in the wild.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
        <rect x="1.5" y="2.25" width="10" height="20"/>
        <line x1="0" y1="0" x2="100" y2="50"/>
    </svg>"#;
    let frame = parse_svg(src).unwrap();
    assert_eq!(frame.root.children.len(), 2);
    let rect = first_path(&frame.root.children[0]);
    assert_eq!(rect.commands[0], PathCommand::MoveTo(Point::new(1.5, 2.25)));
    let line = first_path(&frame.root.children[1]);
    assert_eq!(line.commands[0], PathCommand::MoveTo(Point::new(0.0, 0.0)));
    assert_eq!(
        line.commands[1],
        PathCommand::LineTo(Point::new(100.0, 50.0))
    );
}

#[test]
fn nested_group_em_inherits_through_intermediate_group() {
    // <g font-size="20"><g><rect width="2em"/></g></g>
    // The inner <g> doesn't set font-size, so the rect inherits 20
    // from the outer <g> and resolves 2em → 40.
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
        <g font-size="20">
            <g>
                <rect x="0" y="0" width="2em" height="1em"/>
            </g>
        </g>
    </svg>"#;
    let frame = parse_svg(src).unwrap();
    let path = first_path(&frame.root.children[0]);
    match path.commands[1] {
        PathCommand::LineTo(p) => {
            assert!((p.x - 40.0).abs() < 1e-3, "inherited em: got {}", p.x);
        }
        _ => panic!("expected LineTo"),
    }
}
