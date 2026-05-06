//! Round 4 — SMIL animation snapshot at arbitrary `t`.

use oxideav_core::{Node, Paint, PathCommand, Rgba};
use oxideav_svg::parse_svg_at;

fn first_path_x(frame: &oxideav_core::VectorFrame) -> Option<f32> {
    fn find(g: &oxideav_core::Group) -> Option<f32> {
        for c in &g.children {
            match c {
                Node::Path(p) => {
                    if let Some(PathCommand::MoveTo(pt)) = p.path.commands.first() {
                        return Some(pt.x);
                    }
                }
                Node::Group(sg) => {
                    if let Some(x) = find(sg) {
                        return Some(x);
                    }
                }
                _ => {}
            }
        }
        None
    }
    find(&frame.root)
}

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

fn solid(p: Paint) -> Rgba {
    match p {
        Paint::Solid(c) => c,
        other => panic!("expected Paint::Solid, got {other:?}"),
    }
}

#[test]
fn animate_at_midpoint_lerps_x() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50">
  <rect width="20" height="20" x="0">
    <animate attributeName="x" from="0" to="100" dur="2s"/>
  </rect>
</svg>"##;
    let frame = parse_svg_at(src, 1.0).unwrap();
    let x = first_path_x(&frame).unwrap();
    assert!((x - 50.0).abs() < 1.0, "expected ~50, got {x}");
}

#[test]
fn animate_at_endpoint_freezes_on_to() {
    // Past dur → freeze on the end frame.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50">
  <rect width="20" height="20" x="0">
    <animate attributeName="x" from="0" to="100" dur="2s"/>
  </rect>
</svg>"##;
    let frame = parse_svg_at(src, 5.0).unwrap();
    let x = first_path_x(&frame).unwrap();
    assert!((x - 100.0).abs() < 0.5, "expected ~100, got {x}");
}

#[test]
fn animate_repeat_count_loops() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50">
  <rect width="20" height="20" x="0">
    <animate attributeName="x" from="0" to="100" dur="2s" repeatCount="3"/>
  </rect>
</svg>"##;
    // After 2.5s we're 0.5s into the second cycle → x ≈ 25.
    let frame = parse_svg_at(src, 2.5).unwrap();
    let x = first_path_x(&frame).unwrap();
    assert!((x - 25.0).abs() < 1.5, "expected ~25, got {x}");
}

#[test]
fn animate_with_keytimes_segments() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50">
  <rect width="20" height="20" x="0">
    <animate attributeName="x" values="0;10;100" keyTimes="0;0.5;1" dur="2s"/>
  </rect>
</svg>"##;
    // 50% through dur → on the keyframe with value 10.
    let frame = parse_svg_at(src, 1.0).unwrap();
    let x = first_path_x(&frame).unwrap();
    assert!((x - 10.0).abs() < 1.0, "expected ~10, got {x}");
}

#[test]
fn animate_color_interpolates_componentwise() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
  <rect width="20" height="20" fill="red">
    <animate attributeName="fill" from="#000000" to="#ffffff" dur="2s"/>
  </rect>
</svg>"##;
    let frame = parse_svg_at(src, 1.0).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    let c = solid(fill);
    // Mid-grey ≈ 128 (lerp permits ±1 for round-mode).
    assert!(
        (c.r as i32 - 128).abs() <= 2,
        "expected ~128 grey, got {c:?}"
    );
    assert_eq!(c.r, c.g);
    assert_eq!(c.g, c.b);
}

#[test]
fn animate_before_begin_keeps_static_value() {
    // `begin="3s"` means at `t=1` the animation hasn't started; the
    // rect should keep its declared `x="42"`.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50">
  <rect width="20" height="20" x="42">
    <animate attributeName="x" from="0" to="100" dur="2s" begin="3s"/>
  </rect>
</svg>"##;
    let frame = parse_svg_at(src, 1.0).unwrap();
    let x = first_path_x(&frame).unwrap();
    assert!((x - 42.0).abs() < f32::EPSILON, "expected 42, got {x}");
}

#[test]
fn animate_indefinite_repeat_loops_at_high_t() {
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50">
  <rect width="20" height="20" x="0">
    <animate attributeName="x" from="0" to="50" dur="1s" repeatCount="indefinite"/>
  </rect>
</svg>"##;
    // 100s in: cycles=100, fractional=0 → snap to start frame (x=0).
    let frame = parse_svg_at(src, 100.0).unwrap();
    let x = first_path_x(&frame).unwrap();
    assert!(x.abs() < 0.5, "expected ~0, got {x}");
}

#[test]
fn animate_clock_with_units() {
    // `dur="500ms"` should be interpreted as 0.5s.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50">
  <rect width="20" height="20" x="0">
    <animate attributeName="x" from="0" to="100" dur="500ms"/>
  </rect>
</svg>"##;
    // At t=0.25s we should be at ~50.
    let frame = parse_svg_at(src, 0.25).unwrap();
    let x = first_path_x(&frame).unwrap();
    assert!((x - 50.0).abs() < 1.0, "expected ~50, got {x}");
}

#[test]
fn animate_t0_matches_round3_behaviour() {
    // Round 3 evaluated at t=0; the round-4 default (parse_svg) should
    // produce identical results.
    let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
  <rect width="50" height="50" fill="red">
    <animate attributeName="fill" from="#00ff00" to="#0000ff" dur="2s"/>
  </rect>
</svg>"##;
    let frame = oxideav_svg::parse_svg(src).unwrap();
    let fill = first_path_fill(&frame).unwrap();
    assert_eq!(solid(fill), Rgba::opaque(0, 255, 0));
}
