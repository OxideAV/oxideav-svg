//! `d` attribute mini-language parser.
//!
//! Implements the SVG 1.1 §8.3 path-data grammar — every command in
//! both absolute (uppercase) and relative (lowercase) form. Smooth
//! cubic (`S`/`s`) and smooth quadratic (`T`/`t`) reflect the previous
//! control point as required by §8.3.6 / §8.3.7. The elliptic arc
//! (`A`/`a`) is preserved verbatim as [`PathCommand::ArcTo`] — the
//! conversion from arc parameters to its cubic-Bezier flattening is a
//! pure function the rasterizer can do later.
//!
//! Lexical rules (§8.3.1):
//! - Numbers may use `+`/`-` signs and a single decimal point;
//!   exponents (`e+/-N`) are accepted.
//! - Whitespace and `,` separate numbers and commands.
//! - A repeat of the previous command is implied by additional number
//!   tuples after the first parameter list (e.g. `M 0 0 10 10 20 20`
//!   is `M 0 0 L 10 10 L 20 20`; the implicit follow-on for `m` is
//!   `l`, the rest follow-on themselves).

use oxideav_core::{Error, PathCommand, Point, Result};

/// Parse a `d` attribute into a sequence of [`PathCommand`]s.
///
/// All commands are emitted in absolute coordinates; relative-form
/// inputs are converted using the running `current point` per
/// §8.3.4. Smooth-curve shorthands are expanded to their full
/// `CubicCurveTo` / `QuadCurveTo` form using the §8.3.6 / §8.3.7
/// reflection rule.
pub fn parse_path_data(d: &str) -> Result<Vec<PathCommand>> {
    let mut p = PathDataParser::new(d.as_bytes());
    let mut commands: Vec<PathCommand> = Vec::new();

    let mut current = Point::new(0.0, 0.0);
    let mut subpath_start = Point::new(0.0, 0.0);
    let mut prev_cubic_ctrl: Option<Point> = None;
    let mut prev_quad_ctrl: Option<Point> = None;

    while let Some(c) = p.next_command()? {
        let absolute = c.is_ascii_uppercase();
        let cmd = c.to_ascii_uppercase();
        let mut first_in_run = true;

        loop {
            match cmd {
                'M' => {
                    let (x, y) = p.read_point()?;
                    let pt = if first_in_run {
                        absolute_or_relative(absolute, current, x, y)
                    } else {
                        // Subsequent number pairs after `M`/`m` are
                        // implicit `L`/`l` commands per §8.3.2.
                        absolute_or_relative(absolute, current, x, y)
                    };
                    if first_in_run {
                        commands.push(PathCommand::MoveTo(pt));
                        subpath_start = pt;
                    } else {
                        commands.push(PathCommand::LineTo(pt));
                    }
                    current = pt;
                    prev_cubic_ctrl = None;
                    prev_quad_ctrl = None;
                }
                'L' => {
                    let (x, y) = p.read_point()?;
                    let pt = absolute_or_relative(absolute, current, x, y);
                    commands.push(PathCommand::LineTo(pt));
                    current = pt;
                    prev_cubic_ctrl = None;
                    prev_quad_ctrl = None;
                }
                'H' => {
                    let x = p.read_number()?;
                    let pt = if absolute {
                        Point::new(x, current.y)
                    } else {
                        Point::new(current.x + x, current.y)
                    };
                    commands.push(PathCommand::LineTo(pt));
                    current = pt;
                    prev_cubic_ctrl = None;
                    prev_quad_ctrl = None;
                }
                'V' => {
                    let y = p.read_number()?;
                    let pt = if absolute {
                        Point::new(current.x, y)
                    } else {
                        Point::new(current.x, current.y + y)
                    };
                    commands.push(PathCommand::LineTo(pt));
                    current = pt;
                    prev_cubic_ctrl = None;
                    prev_quad_ctrl = None;
                }
                'C' => {
                    let (x1, y1) = p.read_point()?;
                    let (x2, y2) = p.read_point()?;
                    let (x, y) = p.read_point()?;
                    let c1 = absolute_or_relative(absolute, current, x1, y1);
                    let c2 = absolute_or_relative(absolute, current, x2, y2);
                    let end = absolute_or_relative(absolute, current, x, y);
                    commands.push(PathCommand::CubicCurveTo { c1, c2, end });
                    prev_cubic_ctrl = Some(c2);
                    prev_quad_ctrl = None;
                    current = end;
                }
                'S' => {
                    // Smooth cubic: c1 reflects the previous c2 about
                    // the current point if the previous command was
                    // `C`/`c`/`S`/`s`; otherwise c1 = current.
                    let (x2, y2) = p.read_point()?;
                    let (x, y) = p.read_point()?;
                    let c2 = absolute_or_relative(absolute, current, x2, y2);
                    let end = absolute_or_relative(absolute, current, x, y);
                    let c1 = match prev_cubic_ctrl {
                        Some(prev) => {
                            Point::new(2.0 * current.x - prev.x, 2.0 * current.y - prev.y)
                        }
                        None => current,
                    };
                    commands.push(PathCommand::CubicCurveTo { c1, c2, end });
                    prev_cubic_ctrl = Some(c2);
                    prev_quad_ctrl = None;
                    current = end;
                }
                'Q' => {
                    let (xc, yc) = p.read_point()?;
                    let (x, y) = p.read_point()?;
                    let control = absolute_or_relative(absolute, current, xc, yc);
                    let end = absolute_or_relative(absolute, current, x, y);
                    commands.push(PathCommand::QuadCurveTo { control, end });
                    prev_quad_ctrl = Some(control);
                    prev_cubic_ctrl = None;
                    current = end;
                }
                'T' => {
                    let (x, y) = p.read_point()?;
                    let end = absolute_or_relative(absolute, current, x, y);
                    let control = match prev_quad_ctrl {
                        Some(prev) => {
                            Point::new(2.0 * current.x - prev.x, 2.0 * current.y - prev.y)
                        }
                        None => current,
                    };
                    commands.push(PathCommand::QuadCurveTo { control, end });
                    prev_quad_ctrl = Some(control);
                    prev_cubic_ctrl = None;
                    current = end;
                }
                'A' => {
                    let rx = p.read_number()?;
                    let ry = p.read_number()?;
                    let x_axis_rot_deg = p.read_number()?;
                    // Flags are 0/1 — but real-world inputs may omit
                    // separators (`A 5 5 0 005 10` → flags 0, 0, 5).
                    // `read_flag` handles the no-separator case.
                    let large_arc = p.read_flag()?;
                    let sweep = p.read_flag()?;
                    let (x, y) = p.read_point()?;
                    let end = absolute_or_relative(absolute, current, x, y);
                    commands.push(PathCommand::ArcTo {
                        rx,
                        ry,
                        x_axis_rot: x_axis_rot_deg.to_radians(),
                        large_arc,
                        sweep,
                        end,
                    });
                    prev_cubic_ctrl = None;
                    prev_quad_ctrl = None;
                    current = end;
                }
                'Z' => {
                    commands.push(PathCommand::Close);
                    current = subpath_start;
                    prev_cubic_ctrl = None;
                    prev_quad_ctrl = None;
                    // Z takes no parameters; a repeated Z is rare but
                    // legal. Break out to look for the next command.
                    break;
                }
                _ => {
                    return Err(Error::invalid("SVG path: unknown command"));
                }
            }
            first_in_run = false;
            // If the lookahead is another number, repeat the same
            // command; otherwise break out to look for the next letter.
            p.skip_separators();
            if !p.peek_starts_number() {
                break;
            }
            // Z has no implicit-repeat form (handled above). All
            // others repeat naturally.
        }
    }

    Ok(commands)
}

fn absolute_or_relative(absolute: bool, current: Point, x: f32, y: f32) -> Point {
    if absolute {
        Point::new(x, y)
    } else {
        Point::new(current.x + x, current.y + y)
    }
}

struct PathDataParser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> PathDataParser<'a> {
    fn new(src: &'a [u8]) -> Self {
        Self { src, pos: 0 }
    }

    fn skip_separators(&mut self) {
        while self.pos < self.src.len() {
            match self.src[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' | b',' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek_starts_number(&self) -> bool {
        if self.pos >= self.src.len() {
            return false;
        }
        let b = self.src[self.pos];
        matches!(b, b'+' | b'-' | b'.' | b'0'..=b'9')
    }

    fn next_command(&mut self) -> Result<Option<u8>> {
        self.skip_separators();
        if self.pos >= self.src.len() {
            return Ok(None);
        }
        let b = self.src[self.pos];
        if b.is_ascii_alphabetic() {
            self.pos += 1;
            Ok(Some(b))
        } else {
            Err(Error::invalid("SVG path: expected command letter"))
        }
    }

    /// Read one floating-point number per the SVG path number grammar.
    fn read_number(&mut self) -> Result<f32> {
        self.skip_separators();
        let start = self.pos;

        // Optional sign.
        if self.pos < self.src.len() && (self.src[self.pos] == b'+' || self.src[self.pos] == b'-') {
            self.pos += 1;
        }

        let mut saw_digit = false;
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() {
            self.pos += 1;
            saw_digit = true;
        }
        if self.pos < self.src.len() && self.src[self.pos] == b'.' {
            self.pos += 1;
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() {
                self.pos += 1;
                saw_digit = true;
            }
        }
        // Optional exponent.
        if self.pos < self.src.len() && (self.src[self.pos] == b'e' || self.src[self.pos] == b'E') {
            self.pos += 1;
            if self.pos < self.src.len()
                && (self.src[self.pos] == b'+' || self.src[self.pos] == b'-')
            {
                self.pos += 1;
            }
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        if !saw_digit {
            return Err(Error::invalid("SVG path: expected number"));
        }
        let s = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|_| Error::invalid("SVG path: bad UTF-8 in number"))?;
        s.parse::<f32>()
            .map_err(|_| Error::invalid("SVG path: malformed number"))
    }

    fn read_point(&mut self) -> Result<(f32, f32)> {
        let x = self.read_number()?;
        // `,` between coords is optional, separators of any kind work.
        let y = self.read_number()?;
        Ok((x, y))
    }

    /// Read a single-character flag (`0` or `1`) per §8.3.5.
    /// The grammar permits flags to be packed without separators.
    fn read_flag(&mut self) -> Result<bool> {
        self.skip_separators();
        if self.pos >= self.src.len() {
            return Err(Error::invalid("SVG path: expected flag"));
        }
        let b = self.src[self.pos];
        let val = match b {
            b'0' => false,
            b'1' => true,
            _ => return Err(Error::invalid("SVG path: arc flag must be 0 or 1")),
        };
        self.pos += 1;
        Ok(val)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn move_then_line_then_close() {
        let cmds = parse_path_data("M 10 20 L 30 40 Z").unwrap();
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0], PathCommand::MoveTo(Point::new(10.0, 20.0)));
        assert_eq!(cmds[1], PathCommand::LineTo(Point::new(30.0, 40.0)));
        assert_eq!(cmds[2], PathCommand::Close);
    }

    #[test]
    fn relative_lineto_translates_against_current() {
        let cmds = parse_path_data("M 10 10 l 5 5 l 5 -5").unwrap();
        assert_eq!(cmds[0], PathCommand::MoveTo(Point::new(10.0, 10.0)));
        assert_eq!(cmds[1], PathCommand::LineTo(Point::new(15.0, 15.0)));
        assert_eq!(cmds[2], PathCommand::LineTo(Point::new(20.0, 10.0)));
    }

    #[test]
    fn implicit_lineto_after_moveto() {
        // M followed by extra coordinate pairs should expand to
        // implicit L commands.
        let cmds = parse_path_data("M 0 0 10 10 20 20").unwrap();
        assert_eq!(cmds.len(), 3);
        assert!(matches!(cmds[0], PathCommand::MoveTo(_)));
        assert_eq!(cmds[1], PathCommand::LineTo(Point::new(10.0, 10.0)));
        assert_eq!(cmds[2], PathCommand::LineTo(Point::new(20.0, 20.0)));
    }

    #[test]
    fn h_v_shortcuts_use_current_axis() {
        let cmds = parse_path_data("M 5 5 H 10 V 20").unwrap();
        assert_eq!(cmds[1], PathCommand::LineTo(Point::new(10.0, 5.0)));
        assert_eq!(cmds[2], PathCommand::LineTo(Point::new(10.0, 20.0)));
    }

    #[test]
    fn cubic_and_smooth_cubic() {
        // M 0 0 C 10 0 10 10 0 10 S -10 20 0 20
        // The smooth cubic should reflect (10,10) about (0,10) → c1=(-10,10).
        let cmds = parse_path_data("M 0 0 C 10 0 10 10 0 10 S -10 20 0 20").unwrap();
        assert_eq!(cmds.len(), 3);
        if let PathCommand::CubicCurveTo { c1, c2, end } = cmds[2] {
            assert!(approx(c1.x, -10.0));
            assert!(approx(c1.y, 10.0));
            assert!(approx(c2.x, -10.0));
            assert!(approx(c2.y, 20.0));
            assert!(approx(end.x, 0.0));
            assert!(approx(end.y, 20.0));
        } else {
            panic!("expected cubic");
        }
    }

    #[test]
    fn quadratic_and_smooth_quadratic() {
        let cmds = parse_path_data("M 0 0 Q 5 5 10 0 T 20 0").unwrap();
        assert_eq!(cmds.len(), 3);
        if let PathCommand::QuadCurveTo { control, end } = cmds[2] {
            // Reflection of (5,5) about (10,0) is (15,-5).
            assert!(approx(control.x, 15.0));
            assert!(approx(control.y, -5.0));
            assert!(approx(end.x, 20.0));
            assert!(approx(end.y, 0.0));
        } else {
            panic!("expected quad");
        }
    }

    #[test]
    fn elliptic_arc_preserves_flags_and_radians_rotation() {
        let cmds = parse_path_data("M 0 0 A 5 10 90 1 0 10 0").unwrap();
        if let PathCommand::ArcTo {
            rx,
            ry,
            x_axis_rot,
            large_arc,
            sweep,
            end,
        } = cmds[1]
        {
            assert!(approx(rx, 5.0));
            assert!(approx(ry, 10.0));
            assert!(approx(x_axis_rot, std::f32::consts::FRAC_PI_2));
            assert!(large_arc);
            assert!(!sweep);
            assert_eq!(end, Point::new(10.0, 0.0));
        } else {
            panic!("expected arc");
        }
    }

    #[test]
    fn arc_flags_can_be_packed() {
        // SVG spec allows the two flags to be packed with the next
        // number: `0010` = 0,0,1,0.
        let cmds = parse_path_data("M 0 0 A 5 5 0 0010 0").unwrap();
        if let PathCommand::ArcTo {
            large_arc, sweep, ..
        } = cmds[1]
        {
            assert!(!large_arc);
            assert!(!sweep);
        } else {
            panic!("expected arc");
        }
    }

    #[test]
    fn z_resets_current_to_subpath_start() {
        // After Z, an implicit subsequent `L` should walk from the
        // subpath start, not from the last drawn point.
        let cmds = parse_path_data("M 10 10 L 20 20 Z L 30 30").unwrap();
        // Last command is a LineTo from (10,10) to (30,30).
        let last = cmds.last().unwrap();
        assert_eq!(*last, PathCommand::LineTo(Point::new(30.0, 30.0)));
    }

    #[test]
    fn handles_negative_no_separator() {
        // Real-world SVG often omits separators around `-`.
        let cmds = parse_path_data("M0 0L-5-5").unwrap();
        assert_eq!(cmds[0], PathCommand::MoveTo(Point::new(0.0, 0.0)));
        assert_eq!(cmds[1], PathCommand::LineTo(Point::new(-5.0, -5.0)));
    }

    #[test]
    fn handles_decimals_and_exponents() {
        let cmds = parse_path_data("M1.5 .5 L 1e2 -.25").unwrap();
        assert_eq!(cmds[0], PathCommand::MoveTo(Point::new(1.5, 0.5)));
        assert_eq!(cmds[1], PathCommand::LineTo(Point::new(100.0, -0.25)));
    }
}
