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
    DashPattern, Error, FillRule, GradientStop, Group, LineCap, LineJoin, LinearGradient, MaskKind,
    Node, Paint, Path, PathCommand, PathNode, Point, RadialGradient, Result, Rgba, SpreadMethod,
    Stroke, Transform2D,
};

use crate::color::{parse_opacity, parse_paint, PaintValue};
use crate::defs::{parse_url_ref, ClipPathDef, DefsTables, FilterDef, MaskDef, SymbolDef};
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

/// Mutable parse-time context: gradient table (resolved on the fly)
/// and the pre-walked round-2 defs tables (filter / mask / clipPath /
/// symbol). Threaded through every `parse_element_to_node` call so
/// nested elements can resolve `url(#id)` references in any of them.
#[derive(Debug)]
pub struct ParseContext {
    pub gradients: GradientTable,
    pub defs: DefsTables,
}

impl Default for ParseContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ParseContext {
    pub fn new() -> Self {
        Self {
            gradients: GradientTable::new(),
            defs: DefsTables::new(),
        }
    }
}

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
    ctx: &mut ParseContext,
) -> Result<Option<Node>> {
    let local = tag_local(&el.name);
    let node_opt = match local.as_str() {
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
                cache_key: None,
            };
            for child in &el.children {
                if let XmlNode::Element(c) = child {
                    if let Some(node) = parse_element_to_node(c, &state, ctx)? {
                        group.children.push(node);
                    }
                }
            }
            Some(Node::Group(group))
        }
        "defs" => {
            // Walk children — gradient defs (round 1) plus filter /
            // mask / clipPath / symbol defs (round 2). Pre-walk in
            // `decoder::register_all_defs` already populated the
            // round-2 tables; this pass picks up gradients (which
            // depend on inheritance and are cheap to re-resolve).
            for child in &el.children {
                if let XmlNode::Element(c) = child {
                    register_def(c, &mut ctx.gradients)?;
                }
            }
            None
        }
        "lineargradient" | "radialgradient" => {
            register_def(el, &mut ctx.gradients)?;
            None
        }
        // Round-2: filter / mask / clipPath / symbol definitions don't
        // produce visible output by themselves — they're consumed via
        // url(#id) references on other elements. The pre-walk in
        // `decoder::register_all_defs` already captured them, so just
        // return None here.
        "filter" | "mask" | "clippath" | "symbol" => None,
        // Round-2: <foreignObject> and <animate> are documented as
        // graceful skips for round 2 (full support deferred to round
        // 3+). They're emitted as empty groups so a downstream walker
        // sees the element existed without crashing.
        "foreignobject" => Some(Node::Group(Group::default())),
        "animate" | "animatetransform" | "animatemotion" | "set" => None,
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
            let fill = state.solid_fill(&ctx.gradients);
            let stroke = state.solid_stroke(&ctx.gradients);
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
            let inner = if needs_wrap {
                Node::Group(Group {
                    transform: transform.unwrap_or_else(Transform2D::identity),
                    opacity: state.opacity,
                    clip: None,
                    children: vec![Node::Path(path_node)],
                    cache_key: None,
                })
            } else {
                Node::Path(path_node)
            };
            Some(inner)
        }
        #[cfg(feature = "text")]
        "text" => {
            let state = parent_state.merged_with(el)?;
            crate::text::parse_text_element(el, &state, ctx)?
        }
        // <text> when text feature is disabled — silently skip.
        #[cfg(not(feature = "text"))]
        "text" => None,
        // Round-2 deferral list — silently skip <use>, <script>, etc.
        // so the rest of the document still loads.
        _ => None,
    };
    // Round-2: apply mask / clip-path / filter wrappers from this
    // element's attributes to whatever node we just produced. The
    // wrapping order is filter(mask(clip(node))) — outer-most last.
    let node = match node_opt {
        Some(n) => n,
        None => return Ok(None),
    };
    Ok(Some(apply_referenced_defs(el, node, &ctx.defs)))
}

/// Apply `clip-path="url(#id)"` / `mask="url(#id)"` / `filter="url(#id)"`
/// from `el` to `node`, looking the ids up in `defs`. Missing refs are
/// silently dropped (the rest of the document still renders).
fn apply_referenced_defs(el: &Element, mut node: Node, defs: &DefsTables) -> Node {
    if let Some(id) = attr(el, "clip-path").and_then(parse_url_ref) {
        if let Some(cp) = defs.clip_paths.get(id) {
            node = wrap_with_clip(node, cp.path.clone());
        }
    }
    if let Some(id) = attr(el, "mask").and_then(parse_url_ref) {
        if let Some(m) = defs.masks.get(id) {
            node = Node::SoftMask {
                mask: Box::new(Node::Group(m.content.clone())),
                mask_kind: m.mask_kind,
                content: Box::new(node),
            };
        }
    }
    // Filter is graceful pass-through in round 2 — wrap content in a
    // single-child Group so the structural intent ("these children are
    // filtered") survives the round-trip even though the actual
    // rasterisation is deferred to oxideav-raster.
    if let Some(id) = attr(el, "filter").and_then(parse_url_ref) {
        if defs.filters.contains_key(id) {
            // TODO(round-3 / oxideav-raster #368): rasterise the
            // referenced filter graph (Gaussian blur, color matrix,
            // …). Round 2 just preserves the children so a
            // parse → encode round-trip succeeds.
            node = Node::Group(Group {
                children: vec![node],
                ..Group::default()
            });
        }
    }
    node
}

/// Wrap `node` so the rasterizer applies `clip` to its children.
/// Existing groups gain the clip directly (avoiding an extra layer);
/// other node kinds get a fresh single-child group.
fn wrap_with_clip(node: Node, clip: Path) -> Node {
    match node {
        Node::Group(mut g) if g.clip.is_none() => {
            g.clip = Some(clip);
            Node::Group(g)
        }
        other => Node::Group(Group {
            clip: Some(clip),
            children: vec![other],
            ..Group::default()
        }),
    }
}

// ---------------------------------------------------------------------------
// Round-2 def parsers — invoked by the decoder's pre-walk so forward
// references resolve regardless of source order.
// ---------------------------------------------------------------------------

/// Parse `<filter id="...">` into a [`FilterDef`]. Returns `None` if
/// the element has no `id` (then it can't be referenced).
pub fn parse_filter_def(el: &Element) -> Option<(String, FilterDef)> {
    let id = attr(el, "id")?.to_string();
    Some((
        id,
        FilterDef {
            element: el.clone(),
        },
    ))
}

/// Parse `<mask id="..." mask-type="luminance|alpha">` into a
/// [`MaskDef`]. The mask subtree is parsed into a [`Group`] using the
/// provided gradient context (so gradient-filled masks work).
pub fn parse_mask_def(el: &Element, ctx: &mut ParseContext) -> Result<Option<(String, MaskDef)>> {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    let mask_kind = match attr(el, "mask-type").map(str::trim) {
        Some("alpha") => MaskKind::Alpha,
        _ => MaskKind::Luminance,
    };
    let parent_state = PaintState::default();
    let mut group = Group::default();
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            if let Some(node) = parse_element_to_node(c, &parent_state, ctx)? {
                group.children.push(node);
            }
        }
    }
    Ok(Some((
        id,
        MaskDef {
            mask_kind,
            content: group,
        },
    )))
}

/// Parse `<clipPath id="...">` into a [`ClipPathDef`]. Multiple child
/// shapes are concatenated into one path — successive shapes start
/// with a fresh `MoveTo` so the union under the non-zero / even-odd
/// fill rule reproduces the SVG semantic of "the union of every
/// child's filled interior". `<use>` references inside `<clipPath>`
/// are deferred (round 3).
pub fn parse_clip_path_def(
    el: &Element,
    ctx: &mut ParseContext,
) -> Result<Option<(String, ClipPathDef)>> {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    let mut path = Path::new();
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            // Re-use the shape parsers — they already handle rect /
            // circle / ellipse / polyline / polygon / line / path.
            let local = tag_local(&c.name);
            let sub = match local.as_str() {
                "rect" => parse_rect(c)?,
                "circle" => parse_circle(c)?,
                "ellipse" => parse_ellipse(c)?,
                "line" => Some(parse_line(c)?),
                "polyline" => parse_polyline(c, false)?,
                "polygon" => parse_polyline(c, true)?,
                "path" => parse_path(c)?,
                _ => None,
            };
            if let Some(p) = sub {
                // Apply per-element transform if present.
                let transformed = match attr(c, "transform") {
                    Some(v) => transform_path(p, &parse_transform(v)?),
                    None => p,
                };
                path.commands.extend(transformed.commands);
            }
        }
    }
    if path.commands.is_empty() {
        return Ok(None);
    }
    let _ = ctx;
    Ok(Some((id, ClipPathDef { path })))
}

/// Parse `<symbol id="...">` into a [`SymbolDef`]. Captured here for
/// the round-3 `<use>` resolver; round 2 doesn't yet render symbols.
pub fn parse_symbol_def(
    el: &Element,
    ctx: &mut ParseContext,
) -> Result<Option<(String, SymbolDef)>> {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return Ok(None),
    };
    let parent_state = PaintState::default();
    let mut group = Group::default();
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            if let Some(node) = parse_element_to_node(c, &parent_state, ctx)? {
                group.children.push(node);
            }
        }
    }
    Ok(Some((id, SymbolDef { content: group })))
}

/// Apply a 2D affine to every coordinate in `path`. Used by
/// `<clipPath>` resolution where each child shape may carry its own
/// `transform=` attribute.
fn transform_path(path: Path, t: &Transform2D) -> Path {
    let map = |p: Point| t.apply(p);
    let cmds = path
        .commands
        .into_iter()
        .map(|c| match c {
            PathCommand::MoveTo(p) => PathCommand::MoveTo(map(p)),
            PathCommand::LineTo(p) => PathCommand::LineTo(map(p)),
            PathCommand::QuadCurveTo { control, end } => PathCommand::QuadCurveTo {
                control: map(control),
                end: map(end),
            },
            PathCommand::CubicCurveTo { c1, c2, end } => PathCommand::CubicCurveTo {
                c1: map(c1),
                c2: map(c2),
                end: map(end),
            },
            // ArcTo end point is a coordinate; the radii / rotation
            // would also need adjustment under a non-uniform scale —
            // that's out of scope for round 2's clipPath transform.
            PathCommand::ArcTo {
                rx,
                ry,
                x_axis_rot,
                large_arc,
                sweep,
                end,
            } => PathCommand::ArcTo {
                rx,
                ry,
                x_axis_rot,
                large_arc,
                sweep,
                end: map(end),
            },
            PathCommand::Close => PathCommand::Close,
            other => other,
        })
        .collect();
    Path { commands: cmds }
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
/// `em`, `%`, etc.). Round 1 treats every unit as user units, so the
/// numeric value is preserved as f32 with no scaling.
pub fn parse_number(v: Option<&str>, default: f32) -> Result<f32> {
    let s = match v {
        None => return Ok(default),
        Some(s) => s.trim(),
    };
    if s.is_empty() {
        return Ok(default);
    }
    // Find the longest prefix that parses as f32. This handles unit
    // suffixes like "12px", "3.5em", "50%" — and avoids treating the
    // 'e' in "3.5em" as the start of an exponent (which would consume
    // "3.5e" and then fail to parse).
    let bytes = s.as_bytes();
    let mut best: Option<f32> = None;
    let mut i = 1;
    while i <= bytes.len() {
        // Only consider extending while the prefix could still be a
        // valid number character; once we hit a unit char we stop.
        let c = bytes[i - 1] as char;
        let numeric =
            c.is_ascii_digit() || c == '.' || c == '+' || c == '-' || c == 'e' || c == 'E';
        if !numeric {
            break;
        }
        if let Ok(v) = s[..i].parse::<f32>() {
            best = Some(v);
        }
        i += 1;
    }
    best.ok_or_else(|| Error::invalid("SVG: malformed number"))
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
