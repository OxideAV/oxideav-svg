//! [`VectorFrame`] → SVG bytes encoder.
//!
//! Round 1 emits one `<path>` per `PathNode` (lossless preservation of
//! the exact command sequence the decoder produces) plus a flat
//! `<defs>` block at the top of the document carrying every gradient
//! used by any descendant. Groups round-trip as `<g>` with their
//! transform and opacity.

use std::collections::HashMap;

use oxideav_core::{
    DashPattern, Encoder, Error, FillRule, Frame, Group, LineCap, LineJoin, LinearGradient, Node,
    Packet, Paint, PathCommand, PathNode, Point, RadialGradient, Result, Rgba, SpreadMethod,
    TimeBase, Transform2D, VectorFrame,
};

use crate::decoder::CODEC_ID_STR;
use crate::parser::escape_attr;

/// Serialise a [`VectorFrame`] into a UTF-8 SVG byte buffer.
pub fn write_svg(frame: &VectorFrame) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<svg xmlns=\"http://www.w3.org/2000/svg\"");
    out.push_str(&format!(" width=\"{}\"", trim_float(frame.width)));
    out.push_str(&format!(" height=\"{}\"", trim_float(frame.height)));
    if let Some(vb) = &frame.view_box {
        out.push_str(&format!(
            " viewBox=\"{} {} {} {}\"",
            trim_float(vb.min_x),
            trim_float(vb.min_y),
            trim_float(vb.width),
            trim_float(vb.height)
        ));
    }
    out.push_str(">\n");

    // Collect every gradient referenced inside the tree so we can emit
    // a `<defs>` block once at the top.
    let mut gradients: GradientCollector = GradientCollector::default();
    collect_paints_in_group(&frame.root, &mut gradients);

    if !gradients.entries.is_empty() {
        out.push_str("  <defs>\n");
        for (id, paint) in &gradients.entries {
            write_gradient(&mut out, id, paint);
        }
        out.push_str("  </defs>\n");
    }

    write_group_children(&mut out, &frame.root, 1, &gradients);

    out.push_str("</svg>\n");
    out.into_bytes()
}

fn write_group_children(
    out: &mut String,
    group: &Group,
    depth: usize,
    gradients: &GradientCollector,
) {
    for child in &group.children {
        write_node(out, child, depth, gradients);
    }
}

fn write_node(out: &mut String, node: &Node, depth: usize, gradients: &GradientCollector) {
    let indent = "  ".repeat(depth);
    match node {
        Node::Group(g) => {
            out.push_str(&indent);
            out.push_str("<g");
            if !g.transform.is_identity() {
                out.push_str(&format!(
                    " transform=\"{}\"",
                    format_transform(&g.transform)
                ));
            }
            if (g.opacity - 1.0).abs() > f32::EPSILON {
                out.push_str(&format!(" opacity=\"{}\"", trim_float(g.opacity)));
            }
            out.push_str(">\n");
            write_group_children(out, g, depth + 1, gradients);
            out.push_str(&indent);
            out.push_str("</g>\n");
        }
        Node::Path(p) => {
            out.push_str(&indent);
            out.push_str("<path d=\"");
            write_path_d(out, &p.path.commands);
            out.push('"');
            write_paint_attrs(out, p, gradients);
            out.push_str("/>\n");
        }
        Node::Image(_) => {
            // Round 1: serialising embedded raster images would
            // require base64 + a `<image>` href — defer.
        }
        // `Node` is `#[non_exhaustive]` upstream; future variants
        // (text, masks, filters) are silently dropped in round 1.
        _ => {}
    }
}

fn write_paint_attrs(out: &mut String, node: &PathNode, gradients: &GradientCollector) {
    match &node.fill {
        Some(p) => out.push_str(&format!(" fill=\"{}\"", paint_to_attr(p, gradients))),
        None => out.push_str(" fill=\"none\""),
    }
    if let Some(stroke) = &node.stroke {
        out.push_str(&format!(
            " stroke=\"{}\"",
            paint_to_attr(&stroke.paint, gradients)
        ));
        if (stroke.width - 1.0).abs() > f32::EPSILON {
            out.push_str(&format!(" stroke-width=\"{}\"", trim_float(stroke.width)));
        }
        if stroke.cap != LineCap::Butt {
            out.push_str(&format!(" stroke-linecap=\"{}\"", linecap_str(stroke.cap)));
        }
        if stroke.join != LineJoin::Miter {
            out.push_str(&format!(
                " stroke-linejoin=\"{}\"",
                linejoin_str(stroke.join)
            ));
        }
        if (stroke.miter_limit - 4.0).abs() > f32::EPSILON {
            out.push_str(&format!(
                " stroke-miterlimit=\"{}\"",
                trim_float(stroke.miter_limit)
            ));
        }
        if let Some(dash) = &stroke.dash {
            write_dash(out, dash);
        }
    }
    if node.fill_rule == FillRule::EvenOdd {
        out.push_str(" fill-rule=\"evenodd\"");
    }
}

fn write_dash(out: &mut String, dash: &DashPattern) {
    if !dash.array.is_empty() {
        let arr: Vec<String> = dash.array.iter().map(|n| trim_float(*n)).collect();
        out.push_str(&format!(" stroke-dasharray=\"{}\"", arr.join(",")));
    }
    if dash.offset.abs() > f32::EPSILON {
        out.push_str(&format!(
            " stroke-dashoffset=\"{}\"",
            trim_float(dash.offset)
        ));
    }
}

fn paint_to_attr(p: &Paint, gradients: &GradientCollector) -> String {
    match p {
        Paint::Solid(c) => color_to_attr(*c),
        Paint::LinearGradient(_) | Paint::RadialGradient(_) => {
            // We registered every gradient already in the collection
            // pass; look it up by pointer-or-content to find its id.
            match gradients.lookup(p) {
                Some(id) => format!("url(#{})", escape_attr(id)),
                None => "none".to_string(),
            }
        }
        // `Paint` is `#[non_exhaustive]` upstream; unknown future
        // paint servers serialise as `none` rather than failing.
        _ => "none".to_string(),
    }
}

fn color_to_attr(c: Rgba) -> String {
    if c.a == 255 {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        // SVG accepts `#rrggbbaa` in CSS Color L4 / SVG 2; for
        // maximum interop we emit `rgba(r,g,b,a)`.
        format!(
            "rgba({},{},{},{})",
            c.r,
            c.g,
            c.b,
            trim_float(c.a as f32 / 255.0)
        )
    }
}

fn linecap_str(c: LineCap) -> &'static str {
    match c {
        LineCap::Butt => "butt",
        LineCap::Round => "round",
        LineCap::Square => "square",
    }
}

fn linejoin_str(j: LineJoin) -> &'static str {
    match j {
        LineJoin::Miter => "miter",
        LineJoin::Round => "round",
        LineJoin::Bevel => "bevel",
    }
}

fn format_transform(t: &Transform2D) -> String {
    format!(
        "matrix({} {} {} {} {} {})",
        trim_float(t.a),
        trim_float(t.b),
        trim_float(t.c),
        trim_float(t.d),
        trim_float(t.e),
        trim_float(t.f)
    )
}

fn write_path_d(out: &mut String, cmds: &[PathCommand]) {
    let mut first = true;
    for cmd in cmds {
        if !first {
            out.push(' ');
        }
        first = false;
        match cmd {
            PathCommand::MoveTo(p) => write_pt(out, "M", *p),
            PathCommand::LineTo(p) => write_pt(out, "L", *p),
            PathCommand::QuadCurveTo { control, end } => {
                out.push_str(&format!(
                    "Q {} {} {} {}",
                    trim_float(control.x),
                    trim_float(control.y),
                    trim_float(end.x),
                    trim_float(end.y)
                ));
            }
            PathCommand::CubicCurveTo { c1, c2, end } => {
                out.push_str(&format!(
                    "C {} {} {} {} {} {}",
                    trim_float(c1.x),
                    trim_float(c1.y),
                    trim_float(c2.x),
                    trim_float(c2.y),
                    trim_float(end.x),
                    trim_float(end.y)
                ));
            }
            PathCommand::ArcTo {
                rx,
                ry,
                x_axis_rot,
                large_arc,
                sweep,
                end,
            } => {
                out.push_str(&format!(
                    "A {} {} {} {} {} {} {}",
                    trim_float(*rx),
                    trim_float(*ry),
                    trim_float(x_axis_rot.to_degrees()),
                    if *large_arc { 1 } else { 0 },
                    if *sweep { 1 } else { 0 },
                    trim_float(end.x),
                    trim_float(end.y)
                ));
            }
            PathCommand::Close => out.push('Z'),
            // `PathCommand` is `#[non_exhaustive]` upstream; future
            // shorthand variants are dropped from the serialisation.
            _ => {}
        }
    }
}

fn write_pt(out: &mut String, cmd: &str, p: Point) {
    out.push_str(cmd);
    out.push(' ');
    out.push_str(&trim_float(p.x));
    out.push(' ');
    out.push_str(&trim_float(p.y));
}

fn trim_float(v: f32) -> String {
    // Print with up to 6 significant decimals, trim trailing zeros.
    let s = format!("{v:.6}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".into()
    } else {
        trimmed.to_string()
    }
}

#[derive(Default)]
struct GradientCollector {
    entries: Vec<(String, Paint)>,
    /// Map an opaque "address" (hash of fingerprint) to the id we
    /// assigned. Round 1 dedupes by structural equality so a
    /// gradient referenced twice serialises once.
    by_fingerprint: HashMap<String, String>,
}

impl GradientCollector {
    fn ensure(&mut self, paint: &Paint) {
        let fp = match paint {
            Paint::LinearGradient(g) => linear_fingerprint(g),
            Paint::RadialGradient(g) => radial_fingerprint(g),
            _ => return,
        };
        if self.by_fingerprint.contains_key(&fp) {
            return;
        }
        let id = format!("grad{}", self.entries.len() + 1);
        self.entries.push((id.clone(), paint.clone()));
        self.by_fingerprint.insert(fp, id);
    }

    fn lookup(&self, paint: &Paint) -> Option<&str> {
        let fp = match paint {
            Paint::LinearGradient(g) => linear_fingerprint(g),
            Paint::RadialGradient(g) => radial_fingerprint(g),
            _ => return None,
        };
        self.by_fingerprint.get(&fp).map(String::as_str)
    }
}

fn linear_fingerprint(g: &LinearGradient) -> String {
    let mut s = format!(
        "L:{}:{}:{}:{}:{}:",
        trim_float(g.start.x),
        trim_float(g.start.y),
        trim_float(g.end.x),
        trim_float(g.end.y),
        spread_str(g.spread),
    );
    for stop in &g.stops {
        s.push_str(&format!(
            "{}:{},{},{},{};",
            trim_float(stop.offset),
            stop.color.r,
            stop.color.g,
            stop.color.b,
            stop.color.a
        ));
    }
    s
}

fn radial_fingerprint(g: &RadialGradient) -> String {
    let mut s = format!(
        "R:{}:{}:{}:{}:",
        trim_float(g.center.x),
        trim_float(g.center.y),
        trim_float(g.radius),
        spread_str(g.spread),
    );
    if let Some(f) = g.focal {
        s.push_str(&format!("{},{}", trim_float(f.x), trim_float(f.y)));
    }
    s.push(':');
    for stop in &g.stops {
        s.push_str(&format!(
            "{}:{},{},{},{};",
            trim_float(stop.offset),
            stop.color.r,
            stop.color.g,
            stop.color.b,
            stop.color.a
        ));
    }
    s
}

fn collect_paints_in_group(group: &Group, gradients: &mut GradientCollector) {
    for child in &group.children {
        match child {
            Node::Path(p) => {
                if let Some(paint) = &p.fill {
                    gradients.ensure(paint);
                }
                if let Some(s) = &p.stroke {
                    gradients.ensure(&s.paint);
                }
            }
            Node::Group(g) => collect_paints_in_group(g, gradients),
            Node::Image(_) => {}
            // `Node` is `#[non_exhaustive]` upstream; ignore unknown
            // variants when collecting referenced paints.
            _ => {}
        }
    }
}

fn write_gradient(out: &mut String, id: &str, paint: &Paint) {
    match paint {
        Paint::LinearGradient(g) => {
            out.push_str(&format!(
                "    <linearGradient id=\"{}\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" gradientUnits=\"userSpaceOnUse\"",
                escape_attr(id),
                trim_float(g.start.x),
                trim_float(g.start.y),
                trim_float(g.end.x),
                trim_float(g.end.y)
            ));
            if g.spread != SpreadMethod::Pad {
                out.push_str(&format!(" spreadMethod=\"{}\"", spread_str(g.spread)));
            }
            out.push_str(">\n");
            for stop in &g.stops {
                write_stop(out, *stop);
            }
            out.push_str("    </linearGradient>\n");
        }
        Paint::RadialGradient(g) => {
            out.push_str(&format!(
                "    <radialGradient id=\"{}\" cx=\"{}\" cy=\"{}\" r=\"{}\" gradientUnits=\"userSpaceOnUse\"",
                escape_attr(id),
                trim_float(g.center.x),
                trim_float(g.center.y),
                trim_float(g.radius)
            ));
            if let Some(f) = g.focal {
                out.push_str(&format!(
                    " fx=\"{}\" fy=\"{}\"",
                    trim_float(f.x),
                    trim_float(f.y)
                ));
            }
            if g.spread != SpreadMethod::Pad {
                out.push_str(&format!(" spreadMethod=\"{}\"", spread_str(g.spread)));
            }
            out.push_str(">\n");
            for stop in &g.stops {
                write_stop(out, *stop);
            }
            out.push_str("    </radialGradient>\n");
        }
        _ => {}
    }
}

fn write_stop(out: &mut String, stop: oxideav_core::GradientStop) {
    let color = format!(
        "#{:02x}{:02x}{:02x}",
        stop.color.r, stop.color.g, stop.color.b
    );
    out.push_str(&format!(
        "      <stop offset=\"{}\" stop-color=\"{}\"",
        trim_float(stop.offset),
        color
    ));
    if stop.color.a != 255 {
        out.push_str(&format!(
            " stop-opacity=\"{}\"",
            trim_float(stop.color.a as f32 / 255.0)
        ));
    }
    out.push_str("/>\n");
}

fn spread_str(s: SpreadMethod) -> &'static str {
    match s {
        SpreadMethod::Pad => "pad",
        SpreadMethod::Reflect => "reflect",
        SpreadMethod::Repeat => "repeat",
    }
}

// ---------------------------------------------------------------------------
// Encoder trait adapter
// ---------------------------------------------------------------------------

pub fn make_encoder(_params: &oxideav_core::CodecParameters) -> Result<Box<dyn Encoder>> {
    let mut out_params =
        oxideav_core::CodecParameters::video(oxideav_core::CodecId::new(CODEC_ID_STR));
    out_params.media_type = oxideav_core::MediaType::Video;
    Ok(Box::new(SvgEncoder {
        codec_id: oxideav_core::CodecId::new(CODEC_ID_STR),
        out_params,
        pending: None,
        eof: false,
    }))
}

struct SvgEncoder {
    codec_id: oxideav_core::CodecId,
    out_params: oxideav_core::CodecParameters,
    pending: Option<Vec<u8>>,
    eof: bool,
}

impl Encoder for SvgEncoder {
    fn codec_id(&self) -> &oxideav_core::CodecId {
        &self.codec_id
    }
    fn output_params(&self) -> &oxideav_core::CodecParameters {
        &self.out_params
    }
    fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        let vf = match frame {
            Frame::Vector(v) => v,
            _ => return Err(Error::invalid("SVG encoder: expected vector frame")),
        };
        self.pending = Some(write_svg(vf));
        Ok(())
    }
    fn receive_packet(&mut self) -> Result<Packet> {
        match self.pending.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
            None => {
                if self.eof {
                    Err(Error::Eof)
                } else {
                    Err(Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> Result<()> {
        self.eof = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::{FillRule, GradientStop, Group, Node, Path, PathNode, Point, Rgba, ViewBox};

    fn make_simple_frame() -> VectorFrame {
        let mut path = Path::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(10.0, 0.0));
        path.line_to(Point::new(10.0, 10.0));
        path.close();
        let pn = PathNode {
            path,
            fill: Some(Paint::Solid(Rgba::opaque(255, 0, 0))),
            stroke: None,
            fill_rule: FillRule::NonZero,
        };
        VectorFrame {
            width: 10.0,
            height: 10.0,
            view_box: Some(ViewBox {
                min_x: 0.0,
                min_y: 0.0,
                width: 10.0,
                height: 10.0,
            }),
            root: Group {
                children: vec![Node::Path(pn)],
                ..Group::default()
            },
            pts: None,
            time_base: TimeBase::new(1, 1),
        }
    }

    #[test]
    fn writes_minimal_svg_with_red_triangle() {
        let bytes = write_svg(&make_simple_frame());
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.starts_with("<?xml"));
        assert!(s.contains("<svg"));
        assert!(s.contains("fill=\"#ff0000\""));
        assert!(s.contains("d=\"M 0 0 L 10 0 L 10 10 Z\""));
        assert!(s.ends_with("</svg>\n"));
    }

    #[test]
    fn trim_float_is_compact() {
        assert_eq!(trim_float(1.5), "1.5");
        assert_eq!(trim_float(2.0), "2");
        assert_eq!(trim_float(-0.0), "0");
        assert_eq!(trim_float(0.123456), "0.123456");
    }

    #[test]
    fn writes_gradient_def_when_referenced() {
        let stops = vec![
            GradientStop::new(0.0, Rgba::opaque(255, 0, 0)),
            GradientStop::new(1.0, Rgba::opaque(0, 0, 255)),
        ];
        let lg = LinearGradient {
            start: Point::new(0.0, 0.0),
            end: Point::new(10.0, 0.0),
            stops,
            spread: SpreadMethod::Pad,
        };
        let mut path = Path::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(10.0, 10.0));
        let frame = VectorFrame {
            width: 10.0,
            height: 10.0,
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
        let s = String::from_utf8(write_svg(&frame)).unwrap();
        assert!(s.contains("<defs>"));
        assert!(s.contains("<linearGradient"));
        assert!(s.contains("url(#grad1)"));
    }
}
