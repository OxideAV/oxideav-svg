//! Exercise every path command (M, L, H, V, C, S, Q, T, A, Z) and
//! both absolute / relative / smooth-shorthand variants.

use oxideav_core::{Node, PathCommand};
use oxideav_svg::parse_svg;

fn first_path_commands(src: &[u8]) -> Vec<PathCommand> {
    let frame = parse_svg(src).expect("parses");
    let path = match &frame.root.children[0] {
        Node::Path(p) => p,
        other => panic!("expected path, got {other:?}"),
    };
    path.path.commands.clone()
}

#[test]
fn M_L_H_V_Z_round_trip() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <path d="M 0 0 L 5 0 H 10 V 10 Z"/>
    </svg>"#;
    let cmds = first_path_commands(src);
    assert_eq!(cmds.len(), 5);
    assert!(matches!(cmds[0], PathCommand::MoveTo(_)));
    // H translates to a horizontal LineTo, V to a vertical LineTo.
    assert!(matches!(cmds[2], PathCommand::LineTo(_)));
    assert!(matches!(cmds[3], PathCommand::LineTo(_)));
    assert_eq!(cmds[4], PathCommand::Close);
}

#[test]
fn cubic_smooth_quad_smooth_arc_roundtrip() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
        <path d="M 0 0 C 10 0 10 10 0 10 S -10 20 0 20 Q 5 25 10 20 T 20 20 A 5 5 0 1 0 25 25 Z"/>
    </svg>"#;
    let cmds = first_path_commands(src);
    // M, C, S, Q, T, A, Z = 7 commands.
    assert_eq!(cmds.len(), 7);
    assert!(matches!(cmds[1], PathCommand::CubicCurveTo { .. }));
    assert!(matches!(cmds[2], PathCommand::CubicCurveTo { .. }));
    assert!(matches!(cmds[3], PathCommand::QuadCurveTo { .. }));
    assert!(matches!(cmds[4], PathCommand::QuadCurveTo { .. }));
    assert!(matches!(cmds[5], PathCommand::ArcTo { .. }));
    assert_eq!(cmds[6], PathCommand::Close);
}

#[test]
fn relative_commands_translate_against_current_point() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
        <path d="M 10 10 l 5 0 l 0 5"/>
    </svg>"#;
    let cmds = first_path_commands(src);
    assert_eq!(cmds.len(), 3);
    if let PathCommand::LineTo(p) = cmds[1] {
        assert_eq!(p.x, 15.0);
        assert_eq!(p.y, 10.0);
    }
    if let PathCommand::LineTo(p) = cmds[2] {
        assert_eq!(p.x, 15.0);
        assert_eq!(p.y, 15.0);
    }
}

#[test]
fn implicit_lineto_after_moveto() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
        <path d="M 0 0 10 10 20 20"/>
    </svg>"#;
    let cmds = first_path_commands(src);
    // M + 2 implicit L = 3 commands.
    assert_eq!(cmds.len(), 3);
    assert!(matches!(cmds[0], PathCommand::MoveTo(_)));
    assert!(matches!(cmds[1], PathCommand::LineTo(_)));
    assert!(matches!(cmds[2], PathCommand::LineTo(_)));
}

#[test]
fn arc_flags_preserved() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
        <path d="M 0 0 A 5 10 90 1 0 10 0"/>
    </svg>"#;
    let cmds = first_path_commands(src);
    if let PathCommand::ArcTo {
        rx,
        ry,
        large_arc,
        sweep,
        ..
    } = cmds[1]
    {
        assert_eq!(rx, 5.0);
        assert_eq!(ry, 10.0);
        assert!(large_arc);
        assert!(!sweep);
    } else {
        panic!("expected arc command");
    }
}
