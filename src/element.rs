//! Element-specific parsers — turn an [`Element`] into an
//! [`oxideav_core::Node`].
//!
//! Each shape parser builds a `PathNode` (rectangles / circles /
//! ellipses / lines / polylines / polygons all reduce to a `Path`).
//! `<g>` is mapped to a [`Group`] node that recurses into its children.
//! `<linearGradient>` / `<radialGradient>` are collected into a
//! `gradient table` keyed by `id`, then resolved when a paint
//! attribute references one via `url(#id)`.

use std::collections::HashMap;

use oxideav_core::{
    DashPattern, Error, FillRule, GradientStop, Group, LineCap, LineJoin, LinearGradient, Node,
    Paint, Path, PathCommand, PathNode, Point, RadialGradient, Result, Rgba, SpreadMethod, Stroke,
    Transform2D,
};

use crate::color::{parse_opacity, parse_paint, PaintValue};
use crate::parser::{attr, tag_local, Element, Node as XmlNode};
use crate::path_data::parse_path_data;
use crate::transform::parse_transform;

/// Inherited paint / stroke / opacity / fill-rule state. Round 1
/// keeps this minimal — full CSS inheritance lives in round 2 (text /
/// `<style>`).
#[derive(Clone, Debug)]
pub struct PaintState {
    pub fill: PaintValue,
    pub fill_opacity: f32,
    pub stroke: PaintValue,
    pub stroke_opacity: f32,
    pub stroke_width: f32,
    pub stroke_linecap: LineCap,
    pub stroke_linejoin: LineJoin,
    pub stroke_miterlimit: f32,
    pub stroke_dasharray: Option<Vec<f32>>,
    pub stroke_dashoffset: f32,
    pub opacity: f32,
    pub fill_rule: FillRule,
}

impl Default for PaintState {
    fn default() -> Self {
        Self {
            // Per SVG 1.1 §11.3 — default fill is opaque black, no
            // stroke.
            fill: PaintValue::Color(Rgba::opaque(0, 0, 0)),
            fill_opacity: 1.0,
            stroke: PaintValue::None,
            stroke_opacity: 1.0,
            stroke_width: 1.0,
            stroke_linecap: LineCap::Butt,
            stroke_linejoin: LineJoin::Miter,
            stroke_miterlimit: 4.0,
            stroke_dasharray: None,
            stroke_dashoffset: 0.0,
            opacity: 1.0,
            fill_rule: FillRule::NonZero,
        }
    }
}

impl PaintState {
    /// Override fields from `el`'s presentation attributes. Anything
    /// the element doesn't set inherits unchanged from `self`.
    pub fn merged_with(&self, el: &Element) -> Result<Self> {
        let mut s = self.clone();
        if let Some(v) = attr(el, "fill") {
            s.fill = parse_paint(v)?;
        }
        if let Some(v) = attr(el, "fill-opacity") {
            s.fill_opacity = parse_opacity(v)?;
        }
        if let Some(v) = attr(el, "stroke") {
            s.stroke = parse_paint(v)?;
        }
        if let Some(v) = attr(el, "stroke-opacity") {
            s.stroke_opacity = parse_opacity(v)?;
        }
        if let Some(v) = attr(el, "stroke-width") {
            s.stroke_width = v
                .trim()
                .parse::<f32>()
                .map_err(|_| Error::invalid("SVG: malformed stroke-width"))?;
        }
        if let Some(v) = attr(el, "stroke-linecap") {
            s.stroke_linecap = match v.trim() {
                "butt" => LineCap::Butt,
                "round" => LineCap::Round,
                "square" => LineCap::Square,
                _ => return Err(Error::invalid("SVG: bad stroke-linecap")),
            };
        }
        if let Some(v) = attr(el, "stroke-linejoin") {
            s.stroke_linejoin = match v.trim() {
                "miter" => LineJoin::Miter,
                "round" => LineJoin::Round,
                "bevel" => LineJoin::Bevel,
                _ => return Err(Error::invalid("SVG: bad stroke-linejoin")),
            };
        }
        if let Some(v) = attr(el, "stroke-miterlimit") {
            s.stroke_miterlimit = v
                .trim()
                .parse::<f32>()
                .map_err(|_| Error::invalid("SVG: malformed stroke-miterlimit"))?;
        }
        if let Some(v) = attr(el, "stroke-dasharray") {
            let trimmed = v.trim();
            if trimmed.eq_ignore_ascii_case("none") {
                s.stroke_dasharray = None;
            } else {
                let arr: Result<Vec<f32>> = trimmed
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|p| !p.is_empty())
                    .map(|n| {
                        n.parse::<f32>()
                            .map_err(|_| Error::invalid("SVG: malformed stroke-dasharray"))
                    })
                    .collect();
                s.stroke_dasharray = Some(arr?);
            }
        }
        if let Some(v) = attr(el, "stroke-dashoffset") {
            s.stroke_dashoffset = v
                .trim()
                .parse::<f32>()
                .map_err(|_| Error::invalid("SVG: malformed stroke-dashoffset"))?;
        }
        if let Some(v) = attr(el, "opacity") {
            s.opacity = parse_opacity(v)?;
        }
        if let Some(v) = attr(el, "fill-rule") {
            s.fill_rule = match v.trim() {
                "nonzero" => FillRule::NonZero,
                "evenodd" => FillRule::EvenOdd,
                _ => return Err(Error::invalid("SVG: bad fill-rule")),
            };
        }
        Ok(s)
    }

    fn solid_fill(&self, gradients: &GradientTable) -> Option<Paint> {
        resolve_paint(&self.fill, self.fill_opacity, gradients)
    }

    fn solid_stroke(&self, gradients: &GradientTable) -> Option<Stroke> {
        let paint = resolve_paint(&self.stroke, self.stroke_opacity, gradients)?;
        Some(Stroke {
            width: self.stroke_width,
            paint,
            cap: self.stroke_linecap,
            join: self.stroke_linejoin,
            miter_limit: self.stroke_miterlimit,
            dash: self.stroke_dasharray.as_ref().map(|arr| DashPattern {
                array: arr.clone(),
                offset: self.stroke_dashoffset,
            }),
        })
    }
}

fn apply_alpha(color: Rgba, alpha: f32) -> Rgba {
    let a = ((color.a as f32 / 255.0) * alpha.clamp(0.0, 1.0)) * 255.0;
    Rgba::new(color.r, color.g, color.b, a.round() as u8)
}

fn resolve_paint(value: &PaintValue, opacity: f32, gradients: &GradientTable) -> Option<Paint> {
    match value {
        PaintValue::None => None,
        PaintValue::Color(c) => Some(Paint::Solid(apply_alpha(*c, opacity))),
        PaintValue::Reference(id) => gradients.get(id).cloned(),
    }
}

/// Look-up table of `id` → resolved [`Paint`] (gradient). Built up by
/// the decoder while it walks `<defs>` / inline gradient elements.
pub type GradientTable = HashMap<String, Paint>;

/// Parse `<linearGradient id="...">` into a [`Paint`] entry. Returns
/// `Some((id, paint))` on success, `None` if the element lacks an `id`
/// (in which case it can't be referenced).
pub fn parse_linear_gradient(el: &Element) -> Result<Option<(String, Paint)>> {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    let x1 = parse_coord(attr(el, "x1"), 0.0)?;
    let y1 = parse_coord(attr(el, "y1"), 0.0)?;
    let x2 = parse_coord(attr(el, "x2"), 1.0)?;
    let y2 = parse_coord(attr(el, "y2"), 0.0)?;
    let spread = parse_spread_method(attr(el, "spreadMethod"))?;
    let stops = collect_stops(el)?;
    Ok(Some((
        id,
        Paint::LinearGradient(LinearGradient {
            start: Point::new(x1, y1),
            end: Point::new(x2, y2),
            stops,
            spread,
        }),
    )))
}

/// Parse `<radialGradient id="...">` into a [`Paint`] entry.
pub fn parse_radial_gradient(el: &Element) -> Result<Option<(String, Paint)>> {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    let cx = parse_coord(attr(el, "cx"), 0.5)?;
    let cy = parse_coord(attr(el, "cy"), 0.5)?;
    let r = parse_coord(attr(el, "r"), 0.5)?;
    let fx = attr(el, "fx")
        .map(|v| parse_coord(Some(v), cx))
        .transpose()?;
    let fy = attr(el, "fy")
        .map(|v| parse_coord(Some(v), cy))
        .transpose()?;
    let focal = match (fx, fy) {
        (Some(x), Some(y)) => Some(Point::new(x, y)),
        (Some(x), None) => Some(Point::new(x, cy)),
        (None, Some(y)) => Some(Point::new(cx, y)),
        (None, None) => None,
    };
    let spread = parse_spread_method(attr(el, "spreadMethod"))?;
    let stops = collect_stops(el)?;
    Ok(Some((
        id,
        Paint::RadialGradient(RadialGradient {
            center: Point::new(cx, cy),
            radius: r,
            focal,
            stops,
            spread,
        }),
    )))
}

fn parse_coord(v: Option<&str>, default: f32) -> Result<f32> {
    match v {
        None => Ok(default),
        Some(s) => {
            // Strip optional `%` — round 1 ignores user-units and just
            // takes the numeric value (gradients in SVG default to
            // objectBoundingBox where 0..1 spans the bounding box).
            let trimmed = s.trim().trim_end_matches('%');
            trimmed
                .parse::<f32>()
                .map_err(|_| Error::invalid("SVG gradient: malformed coordinate"))
        }
    }
}

fn parse_spread_method(v: Option<&str>) -> Result<SpreadMethod> {
    Ok(match v.map(str::trim) {
        None | Some("pad") => SpreadMethod::Pad,
        Some("reflect") => SpreadMethod::Reflect,
        Some("repeat") => SpreadMethod::Repeat,
        Some(_) => return Err(Error::invalid("SVG gradient: bad spreadMethod")),
    })
}

fn collect_stops(parent: &Element) -> Result<Vec<GradientStop>> {
    let mut stops = Vec::new();
    for child in &parent.children {
        if let XmlNode::Element(el) = child {
            if tag_local(&el.name) == "stop" {
                let offset = attr(el, "offset")
                    .map(|v| {
                        let trimmed = v.trim().trim_end_matches('%');
                        let raw = trimmed
                            .parse::<f32>()
                            .map_err(|_| Error::invalid("SVG gradient stop: malformed offset"))?;
                        Ok::<f32, Error>(if v.contains('%') { raw / 100.0 } else { raw })
                    })
                    .transpose()?
                    .unwrap_or(0.0);
                let color = match attr(el, "stop-color") {
                    Some(v) => match parse_paint(v)? {
                        PaintValue::Color(c) => c,
                        _ => Rgba::opaque(0, 0, 0),
                    },
                    None => Rgba::opaque(0, 0, 0),
                };
                let opacity = attr(el, "stop-opacity")
                    .map(parse_opacity)
                    .transpose()?
                    .unwrap_or(1.0);
                stops.push(GradientStop::new(offset, apply_alpha(color, opacity)));
            }
        }
    }
    Ok(stops)
}

/// Parse a `<rect>`. Returns `None` for degenerate (zero-size) rects.
pub fn parse_rect(el: &Element) -> Result<Option<Path>> {
    let x = parse_number(attr(el, "x"), 0.0)?;
    let y = parse_number(attr(el, "y"), 0.0)?;
    let w = parse_number(attr(el, "width"), 0.0)?;
    let h = parse_number(attr(el, "height"), 0.0)?;
    if w <= 0.0 || h <= 0.0 {
        return Ok(None);
    }
    let rx_attr = attr(el, "rx")
        .map(|v| parse_number(Some(v), 0.0))
        .transpose()?;
    let ry_attr = attr(el, "ry")
        .map(|v| parse_number(Some(v), 0.0))
        .transpose()?;
    // Per §9.2: if only one of rx/ry is given, the other defaults to it.
    let (rx, ry) = match (rx_attr, ry_attr) {
        (Some(rx), Some(ry)) => (rx, ry),
        (Some(rx), None) => (rx, rx),
        (None, Some(ry)) => (ry, ry),
        (None, None) => (0.0, 0.0),
    };
    let rx = rx.min(w * 0.5).max(0.0);
    let ry = ry.min(h * 0.5).max(0.0);

    let mut path = Path::new();
    if rx == 0.0 && ry == 0.0 {
        path.move_to(Point::new(x, y));
        path.line_to(Point::new(x + w, y));
        path.line_to(Point::new(x + w, y + h));
        path.line_to(Point::new(x, y + h));
        path.close();
    } else {
        // Rounded rect — emit the §9.2 path: 4 lines + 4 elliptic arcs.
        path.move_to(Point::new(x + rx, y));
        path.line_to(Point::new(x + w - rx, y));
        path.commands.push(PathCommand::ArcTo {
            rx,
            ry,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(x + w, y + ry),
        });
        path.line_to(Point::new(x + w, y + h - ry));
        path.commands.push(PathCommand::ArcTo {
            rx,
            ry,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(x + w - rx, y + h),
        });
        path.line_to(Point::new(x + rx, y + h));
        path.commands.push(PathCommand::ArcTo {
            rx,
            ry,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(x, y + h - ry),
        });
        path.line_to(Point::new(x, y + ry));
        path.commands.push(PathCommand::ArcTo {
            rx,
            ry,
            x_axis_rot: 0.0,
            large_arc: false,
            sweep: true,
            end: Point::new(x + rx, y),
        });
        path.close();
    }
    Ok(Some(path))
}

/// Parse a `<circle>`. Returns `None` for r ≤ 0.
pub fn parse_circle(el: &Element) -> Result<Option<Path>> {
    let cx = parse_number(attr(el, "cx"), 0.0)?;
    let cy = parse_number(attr(el, "cy"), 0.0)?;
    let r = parse_number(attr(el, "r"), 0.0)?;
    if r <= 0.0 {
        return Ok(None);
    }
    Ok(Some(ellipse_path(cx, cy, r, r)))
}

/// Parse an `<ellipse>`. Returns `None` for rx ≤ 0 or ry ≤ 0.
pub fn parse_ellipse(el: &Element) -> Result<Option<Path>> {
    let cx = parse_number(attr(el, "cx"), 0.0)?;
    let cy = parse_number(attr(el, "cy"), 0.0)?;
    let rx = parse_number(attr(el, "rx"), 0.0)?;
    let ry = parse_number(attr(el, "ry"), 0.0)?;
    if rx <= 0.0 || ry <= 0.0 {
        return Ok(None);
    }
    Ok(Some(ellipse_path(cx, cy, rx, ry)))
}

fn ellipse_path(cx: f32, cy: f32, rx: f32, ry: f32) -> Path {
    // Two half-arcs starting at the rightmost point; round-trips
    // cleanly through any rasterizer that flattens `ArcTo`.
    let mut path = Path::new();
    path.move_to(Point::new(cx + rx, cy));
    path.commands.push(PathCommand::ArcTo {
        rx,
        ry,
        x_axis_rot: 0.0,
        large_arc: false,
        sweep: true,
        end: Point::new(cx - rx, cy),
    });
    path.commands.push(PathCommand::ArcTo {
        rx,
        ry,
        x_axis_rot: 0.0,
        large_arc: false,
        sweep: true,
        end: Point::new(cx + rx, cy),
    });
    path.close();
    path
}

/// Parse a `<line>`.
pub fn parse_line(el: &Element) -> Result<Path> {
    let x1 = parse_number(attr(el, "x1"), 0.0)?;
    let y1 = parse_number(attr(el, "y1"), 0.0)?;
    let x2 = parse_number(attr(el, "x2"), 0.0)?;
    let y2 = parse_number(attr(el, "y2"), 0.0)?;
    let mut path = Path::new();
    path.move_to(Point::new(x1, y1));
    path.line_to(Point::new(x2, y2));
    Ok(path)
}

/// Parse `<polyline>` / `<polygon>`. `closed=true` appends a `Close`.
pub fn parse_polyline(el: &Element, closed: bool) -> Result<Option<Path>> {
    let points = match attr(el, "points") {
        Some(v) => parse_points(v)?,
        None => return Ok(None),
    };
    if points.is_empty() {
        return Ok(None);
    }
    let mut path = Path::new();
    path.move_to(points[0]);
    for p in points.iter().skip(1) {
        path.line_to(*p);
    }
    if closed {
        path.close();
    }
    Ok(Some(path))
}

fn parse_points(s: &str) -> Result<Vec<Point>> {
    let nums: Result<Vec<f32>> = s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(|n| {
            n.parse::<f32>()
                .map_err(|_| Error::invalid("SVG: malformed number in points"))
        })
        .collect();
    let nums = nums?;
    if nums.len() % 2 != 0 {
        return Err(Error::invalid("SVG: odd number count in points"));
    }
    Ok(nums
        .chunks_exact(2)
        .map(|c| Point::new(c[0], c[1]))
        .collect())
}

/// Parse `<path d="...">` directly into [`Path`].
pub fn parse_path(el: &Element) -> Result<Option<Path>> {
    let d = match attr(el, "d") {
        Some(v) => v,
        None => return Ok(None),
    };
    let cmds = parse_path_data(d)?;
    if cmds.is_empty() {
        return Ok(None);
    }
    Ok(Some(Path { commands: cmds }))
}

/// Parse a parsed `Element` into an `oxideav-core` `Node`. Returns
/// `Ok(None)` when the element produces no visible output (e.g. a rect
/// of width 0, an unknown element).
pub fn parse_element_to_node(
    el: &Element,
    parent_state: &PaintState,
    gradients: &mut GradientTable,
) -> Result<Option<Node>> {
    let local = tag_local(&el.name);
    match local.as_str() {
        "g" => {
            let state = parent_state.merged_with(el)?;
            let transform = match attr(el, "transform") {
                Some(v) => parse_transform(v)?,
                None => Transform2D::identity(),
            };
            let mut group = Group {
                transform,
                opacity: state.opacity,
                clip: None,
                children: Vec::new(),
            };
            for child in &el.children {
                if let XmlNode::Element(c) = child {
                    if let Some(node) = parse_element_to_node(c, &state, gradients)? {
                        group.children.push(node);
                    }
                }
            }
            Ok(Some(Node::Group(group)))
        }
        "defs" => {
            // Walk children — round 1 only cares about gradient defs.
            for child in &el.children {
                if let XmlNode::Element(c) = child {
                    register_def(c, gradients)?;
                }
            }
            Ok(None)
        }
        "lineargradient" | "radialgradient" => {
            register_def(el, gradients)?;
            Ok(None)
        }
        "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "path" => {
            let state = parent_state.merged_with(el)?;
            let path_opt = match local.as_str() {
                "rect" => parse_rect(el)?,
                "circle" => parse_circle(el)?,
                "ellipse" => parse_ellipse(el)?,
                "line" => Some(parse_line(el)?),
                "polyline" => parse_polyline(el, false)?,
                "polygon" => parse_polyline(el, true)?,
                "path" => parse_path(el)?,
                _ => unreachable!(),
            };
            let path = match path_opt {
                Some(p) => p,
                None => return Ok(None),
            };
            // For an open `<line>` / `<polyline>`, default fill of
            // black would paint a triangle — that's surprising and
            // wrong. Per SVG 1.1 §11.4 fill *does* apply, but the
            // common-sense default is "no fill on lines unless asked".
            // We follow the spec literally; users who don't want a
            // fill set fill="none".
            let transform = attr(el, "transform").map(parse_transform).transpose()?;
            let fill = state.solid_fill(gradients);
            let stroke = state.solid_stroke(gradients);
            let path_node = PathNode {
                path,
                fill,
                stroke,
                fill_rule: state.fill_rule,
            };
            // If element-level transform or opacity differs from
            // parent's, wrap in a tiny one-child group so the
            // round-trip preserves them.
            let needs_wrap =
                transform.is_some() || (state.opacity - parent_state.opacity).abs() > f32::EPSILON;
            if needs_wrap {
                let group = Group {
                    transform: transform.unwrap_or_else(Transform2D::identity),
                    opacity: state.opacity,
                    clip: None,
                    children: vec![Node::Path(path_node)],
                };
                Ok(Some(Node::Group(group)))
            } else {
                Ok(Some(Node::Path(path_node)))
            }
        }
        // Round-1 deferral list — silently skip text, filter, mask,
        // foreignObject, animate, etc. so the rest of the document
        // still loads.
        _ => Ok(None),
    }
}

fn register_def(el: &Element, gradients: &mut GradientTable) -> Result<()> {
    match tag_local(&el.name).as_str() {
        "lineargradient" => {
            if let Some((id, paint)) = parse_linear_gradient(el)? {
                gradients.insert(id, paint);
            }
        }
        "radialgradient" => {
            if let Some((id, paint)) = parse_radial_gradient(el)? {
                gradients.insert(id, paint);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Parse a number literal — strips optional unit suffix (`px`, `pt`,
/// etc.). Round 1 treats every unit as user units.
pub fn parse_number(v: Option<&str>, default: f32) -> Result<f32> {
    let s = match v {
        None => return Ok(default),
        Some(s) => s.trim(),
    };
    if s.is_empty() {
        return Ok(default);
    }
    // Strip a trailing unit (px / pt / mm / cm / in / em / ex / % etc).
    let end = s
        .find(|c: char| {
            !(c.is_ascii_digit() || c == '.' || c == '+' || c == '-' || c == 'e' || c == 'E')
        })
        .unwrap_or(s.len());
    let num = &s[..end];
    if num.is_empty() {
        return Err(Error::invalid("SVG: missing numeric value"));
    }
    num.parse::<f32>()
        .map_err(|_| Error::invalid("SVG: malformed number"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elem(name: &str, attrs: &[(&str, &str)]) -> Element {
        Element {
            name: name.to_string(),
            attrs: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            children: Vec::new(),
        }
    }

    #[test]
    fn rect_with_no_radius_is_5_segment_path() {
        let r = parse_rect(&elem(
            "rect",
            &[("x", "1"), ("y", "2"), ("width", "10"), ("height", "5")],
        ))
        .unwrap()
        .unwrap();
        // M, L, L, L, Z = 5 commands.
        assert_eq!(r.commands.len(), 5);
        assert_eq!(r.commands[0], PathCommand::MoveTo(Point::new(1.0, 2.0)));
        assert_eq!(*r.commands.last().unwrap(), PathCommand::Close);
    }

    #[test]
    fn rect_with_radius_uses_arcs() {
        let r = parse_rect(&elem(
            "rect",
            &[
                ("x", "0"),
                ("y", "0"),
                ("width", "10"),
                ("height", "10"),
                ("rx", "2"),
            ],
        ))
        .unwrap()
        .unwrap();
        assert!(r
            .commands
            .iter()
            .any(|c| matches!(c, PathCommand::ArcTo { .. })));
    }

    #[test]
    fn circle_zero_radius_returns_none() {
        let r = parse_circle(&elem("circle", &[("r", "0")])).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn polyline_parses_points() {
        let p = parse_polyline(&elem("polyline", &[("points", "0,0 10,10 20,0")]), false)
            .unwrap()
            .unwrap();
        assert_eq!(p.commands.len(), 3); // M + 2L
    }

    #[test]
    fn polygon_parses_points_and_closes() {
        let p = parse_polyline(&elem("polygon", &[("points", "0,0 10,10 20,0")]), true)
            .unwrap()
            .unwrap();
        assert_eq!(p.commands.len(), 4); // M + 2L + Z
    }

    #[test]
    fn parse_number_strips_unit_suffix() {
        assert_eq!(parse_number(Some("12px"), 0.0).unwrap(), 12.0);
        assert_eq!(parse_number(Some("3.5em"), 0.0).unwrap(), 3.5);
    }

    #[test]
    fn linear_gradient_with_two_stops() {
        let stop_a = Element {
            name: "stop".into(),
            attrs: vec![
                ("offset".into(), "0".into()),
                ("stop-color".into(), "red".into()),
            ],
            children: Vec::new(),
        };
        let stop_b = Element {
            name: "stop".into(),
            attrs: vec![
                ("offset".into(), "1".into()),
                ("stop-color".into(), "blue".into()),
            ],
            children: Vec::new(),
        };
        let lg = Element {
            name: "linearGradient".into(),
            attrs: vec![("id".into(), "g1".into())],
            children: vec![XmlNode::Element(stop_a), XmlNode::Element(stop_b)],
        };
        let (id, paint) = parse_linear_gradient(&lg).unwrap().unwrap();
        assert_eq!(id, "g1");
        match paint {
            Paint::LinearGradient(g) => {
                assert_eq!(g.stops.len(), 2);
                assert_eq!(g.stops[0].color, Rgba::opaque(255, 0, 0));
            }
            _ => panic!("expected linear gradient"),
        }
    }

    #[test]
    fn paint_state_inheritance_overrides_specified_only() {
        let parent = PaintState::default();
        let el = elem("rect", &[("fill", "red")]);
        let child = parent.merged_with(&el).unwrap();
        match child.fill {
            PaintValue::Color(c) => assert_eq!(c, Rgba::opaque(255, 0, 0)),
            _ => panic!(),
        }
        // stroke unchanged.
        assert_eq!(child.stroke, parent.stroke);
    }
}
