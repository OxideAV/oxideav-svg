//! `transform` attribute parser.
//!
//! Implements the SVG 1.1 §7.6 transform list: a whitespace-separated
//! sequence of `name(args)` items. Each item produces a [`Transform2D`];
//! the items are composed left-to-right per §7.6.1 (the leftmost
//! transform is the outermost, applied last to a point).

use oxideav_core::{Error, Result, Transform2D};

/// Parse a `transform` attribute. An empty / whitespace-only string
/// returns the identity transform.
pub fn parse_transform(src: &str) -> Result<Transform2D> {
    let bytes = src.as_bytes();
    let mut pos = 0;
    let mut acc = Transform2D::identity();
    loop {
        skip_separators(bytes, &mut pos);
        if pos >= bytes.len() {
            break;
        }
        let name_start = pos;
        while pos < bytes.len() && bytes[pos].is_ascii_alphabetic() {
            pos += 1;
        }
        if pos == name_start {
            return Err(Error::invalid("SVG transform: expected function name"));
        }
        let name = std::str::from_utf8(&bytes[name_start..pos])
            .map_err(|_| Error::invalid("SVG transform: bad UTF-8"))?
            .to_ascii_lowercase();
        skip_separators(bytes, &mut pos);
        if pos >= bytes.len() || bytes[pos] != b'(' {
            return Err(Error::invalid(
                "SVG transform: expected '(' after function name",
            ));
        }
        pos += 1;
        let args = read_args(bytes, &mut pos)?;
        if pos >= bytes.len() || bytes[pos] != b')' {
            return Err(Error::invalid(
                "SVG transform: expected ')' to close function call",
            ));
        }
        pos += 1;

        let t = build_transform(&name, &args)?;
        acc = acc.compose(&t);
    }
    Ok(acc)
}

fn build_transform(name: &str, args: &[f32]) -> Result<Transform2D> {
    Ok(match name {
        "matrix" => {
            if args.len() != 6 {
                return Err(Error::invalid("SVG transform: matrix() needs 6 numbers"));
            }
            Transform2D {
                a: args[0],
                b: args[1],
                c: args[2],
                d: args[3],
                e: args[4],
                f: args[5],
            }
        }
        "translate" => {
            // §7.6.5: translate(<tx> [<ty>]); ty defaults to 0.
            let tx = *args.first().ok_or_else(|| {
                Error::invalid("SVG transform: translate() needs at least 1 number")
            })?;
            let ty = args.get(1).copied().unwrap_or(0.0);
            Transform2D::translate(tx, ty)
        }
        "scale" => {
            // §7.6.6: scale(<sx> [<sy>]); sy defaults to sx.
            let sx = *args
                .first()
                .ok_or_else(|| Error::invalid("SVG transform: scale() needs at least 1 number"))?;
            let sy = args.get(1).copied().unwrap_or(sx);
            Transform2D::scale(sx, sy)
        }
        "rotate" => {
            // §7.6.7: rotate(<angle> [<cx> <cy>]); about origin or
            // about (cx, cy). Angle is in degrees.
            let angle_deg = *args.first().ok_or_else(|| {
                Error::invalid("SVG transform: rotate() needs at least the angle")
            })?;
            let angle_rad = angle_deg.to_radians();
            if args.len() == 1 {
                Transform2D::rotate(angle_rad)
            } else if args.len() == 3 {
                let cx = args[1];
                let cy = args[2];
                // rotate(a, cx, cy) ≡ translate(cx, cy) rotate(a) translate(-cx, -cy).
                let t1 = Transform2D::translate(cx, cy);
                let r = Transform2D::rotate(angle_rad);
                let t2 = Transform2D::translate(-cx, -cy);
                t1.compose(&r).compose(&t2)
            } else {
                return Err(Error::invalid(
                    "SVG transform: rotate() takes 1 or 3 numbers",
                ));
            }
        }
        "skewx" => {
            let a = *args
                .first()
                .ok_or_else(|| Error::invalid("SVG transform: skewX() needs an angle"))?;
            Transform2D::skew_x(a.to_radians())
        }
        "skewy" => {
            let a = *args
                .first()
                .ok_or_else(|| Error::invalid("SVG transform: skewY() needs an angle"))?;
            Transform2D::skew_y(a.to_radians())
        }
        _ => return Err(Error::invalid("SVG transform: unknown function")),
    })
}

fn skip_separators(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() {
        match bytes[*pos] {
            b' ' | b'\t' | b'\n' | b'\r' | b',' => *pos += 1,
            _ => break,
        }
    }
}

fn read_args(bytes: &[u8], pos: &mut usize) -> Result<Vec<f32>> {
    let mut out = Vec::new();
    loop {
        skip_separators(bytes, pos);
        if *pos >= bytes.len() {
            return Err(Error::invalid("SVG transform: unterminated arg list"));
        }
        if bytes[*pos] == b')' {
            return Ok(out);
        }
        let start = *pos;
        if bytes[*pos] == b'+' || bytes[*pos] == b'-' {
            *pos += 1;
        }
        let mut saw_digit = false;
        while *pos < bytes.len() && bytes[*pos].is_ascii_digit() {
            *pos += 1;
            saw_digit = true;
        }
        if *pos < bytes.len() && bytes[*pos] == b'.' {
            *pos += 1;
            while *pos < bytes.len() && bytes[*pos].is_ascii_digit() {
                *pos += 1;
                saw_digit = true;
            }
        }
        if *pos < bytes.len() && (bytes[*pos] == b'e' || bytes[*pos] == b'E') {
            *pos += 1;
            if *pos < bytes.len() && (bytes[*pos] == b'+' || bytes[*pos] == b'-') {
                *pos += 1;
            }
            while *pos < bytes.len() && bytes[*pos].is_ascii_digit() {
                *pos += 1;
            }
        }
        if !saw_digit {
            return Err(Error::invalid("SVG transform: expected number"));
        }
        let s = std::str::from_utf8(&bytes[start..*pos])
            .map_err(|_| Error::invalid("SVG transform: bad UTF-8 in number"))?;
        let v = s
            .parse::<f32>()
            .map_err(|_| Error::invalid("SVG transform: malformed number"))?;
        out.push(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::Point;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn approx_pt(a: Point, b: Point) -> bool {
        approx(a.x, b.x) && approx(a.y, b.y)
    }

    #[test]
    fn empty_string_is_identity() {
        let t = parse_transform("").unwrap();
        assert!(t.is_identity());
        let t = parse_transform("   \n\t").unwrap();
        assert!(t.is_identity());
    }

    #[test]
    fn matrix_six_args() {
        let t = parse_transform("matrix(1 0 0 1 5 7)").unwrap();
        assert_eq!(t, Transform2D::translate(5.0, 7.0));
    }

    #[test]
    fn translate_one_or_two_args() {
        let t = parse_transform("translate(10)").unwrap();
        assert_eq!(t, Transform2D::translate(10.0, 0.0));
        let t = parse_transform("translate(10, 20)").unwrap();
        assert_eq!(t, Transform2D::translate(10.0, 20.0));
    }

    #[test]
    fn scale_with_uniform_or_split_factors() {
        let t = parse_transform("scale(2)").unwrap();
        assert_eq!(t, Transform2D::scale(2.0, 2.0));
        let t = parse_transform("scale(2,3)").unwrap();
        assert_eq!(t, Transform2D::scale(2.0, 3.0));
    }

    #[test]
    fn rotate_about_origin() {
        let t = parse_transform("rotate(90)").unwrap();
        let p = t.apply(Point::new(1.0, 0.0));
        assert!(approx_pt(p, Point::new(0.0, 1.0)));
    }

    #[test]
    fn rotate_about_arbitrary_point() {
        // rotate(180, 5, 5) should send (5, 5) → (5, 5) and (10, 5) → (0, 5).
        let t = parse_transform("rotate(180 5 5)").unwrap();
        assert!(approx_pt(
            t.apply(Point::new(5.0, 5.0)),
            Point::new(5.0, 5.0)
        ));
        assert!(approx_pt(
            t.apply(Point::new(10.0, 5.0)),
            Point::new(0.0, 5.0)
        ));
    }

    #[test]
    fn skewx_and_skewy_round_trip() {
        let t = parse_transform("skewX(45)").unwrap();
        assert!(approx(t.c, 1.0));
        let t = parse_transform("skewY(45)").unwrap();
        assert!(approx(t.b, 1.0));
    }

    #[test]
    fn composes_left_to_right() {
        // translate(10,0) scale(2)  →  apply scale first, then translate.
        // (1,0) → (2,0) → (12,0).
        let t = parse_transform("translate(10 0) scale(2)").unwrap();
        let out = t.apply(Point::new(1.0, 0.0));
        assert!(approx_pt(out, Point::new(12.0, 0.0)));
    }
}
