//! Top-level SVG → [`VectorFrame`] entry point and the
//! pipeline-friendly [`Decoder`] adapter.

use oxideav_core::{
    CodecId, CodecParameters, Decoder, Error, Frame, Group, Packet, Result, TimeBase, VectorFrame,
    ViewBox,
};

use crate::element::{parse_element_to_node, parse_number, GradientTable, PaintState};
use crate::parser::{
    attr, decode_utf8_lossy_stripping_bom, parse_xml, tag_local, Element, Node as XmlNode,
};

/// Codec id string for SVG vector frames.
pub const CODEC_ID_STR: &str = "svg";

/// Parse a complete SVG document into a [`VectorFrame`].
pub fn parse_svg(bytes: &[u8]) -> Result<VectorFrame> {
    let text = decode_utf8_lossy_stripping_bom(bytes);
    let nodes = parse_xml(&text)?;
    let svg =
        find_svg_root(&nodes).ok_or_else(|| Error::invalid("SVG: missing <svg> root element"))?;
    parse_svg_root(svg)
}

fn find_svg_root(nodes: &[XmlNode]) -> Option<&Element> {
    for n in nodes {
        if let XmlNode::Element(e) = n {
            if tag_local(&e.name) == "svg" {
                return Some(e);
            }
        }
    }
    None
}

fn parse_svg_root(svg: &Element) -> Result<VectorFrame> {
    let view_box = match attr(svg, "viewBox") {
        Some(v) => Some(parse_view_box(v)?),
        None => None,
    };

    // `width` / `height` default to 100% — but round 1 needs concrete
    // numbers to populate the frame, so fall back to the viewBox size
    // when the attributes are missing or are percentages.
    let width = parse_length_or_default(
        attr(svg, "width"),
        view_box.map(|vb| vb.width).unwrap_or(0.0),
    )?;
    let height = parse_length_or_default(
        attr(svg, "height"),
        view_box.map(|vb| vb.height).unwrap_or(0.0),
    )?;

    let parent_state = PaintState::default();
    let mut gradients: GradientTable = GradientTable::new();

    // First pass: register every <defs> child + every gradient seen
    // anywhere in the tree, so forward references inside the doc work
    // regardless of declaration order.
    register_all_defs(svg, &mut gradients)?;

    // Second pass: walk the tree and build the scene graph. Gradients
    // are now resolvable.
    let mut root = Group::default();
    for child in &svg.children {
        if let XmlNode::Element(c) = child {
            if let Some(node) = parse_element_to_node(c, &parent_state, &mut gradients)? {
                root.children.push(node);
            }
        }
    }

    Ok(VectorFrame {
        width,
        height,
        view_box,
        root,
        pts: None,
        time_base: TimeBase::new(1, 1),
    })
}

fn register_all_defs(el: &Element, gradients: &mut GradientTable) -> Result<()> {
    match tag_local(&el.name).as_str() {
        "lineargradient" => {
            if let Some((id, p)) = crate::element::parse_linear_gradient(el)? {
                gradients.insert(id, p);
            }
        }
        "radialgradient" => {
            if let Some((id, p)) = crate::element::parse_radial_gradient(el)? {
                gradients.insert(id, p);
            }
        }
        _ => {}
    }
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            register_all_defs(c, gradients)?;
        }
    }
    Ok(())
}

fn parse_view_box(s: &str) -> Result<ViewBox> {
    let nums: Result<Vec<f32>> = s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(|n| {
            n.parse::<f32>()
                .map_err(|_| Error::invalid("SVG: malformed viewBox number"))
        })
        .collect();
    let nums = nums?;
    if nums.len() != 4 {
        return Err(Error::invalid("SVG: viewBox must be 4 numbers"));
    }
    Ok(ViewBox {
        min_x: nums[0],
        min_y: nums[1],
        width: nums[2],
        height: nums[3],
    })
}

fn parse_length_or_default(v: Option<&str>, default: f32) -> Result<f32> {
    let s = match v {
        None => return Ok(default),
        Some(s) => s.trim(),
    };
    if s.is_empty() || s.ends_with('%') {
        return Ok(default);
    }
    parse_number(Some(s), default)
}

/// Codec-registry adapter. Consumes one packet (the entire SVG file)
/// and produces one [`Frame::Vector`].
pub fn make_decoder(_params: &CodecParameters) -> Result<Box<dyn Decoder>> {
    Ok(Box::new(SvgDecoder {
        codec_id: CodecId::new(CODEC_ID_STR),
        pending: None,
        eof: false,
    }))
}

struct SvgDecoder {
    codec_id: CodecId,
    pending: Option<VectorFrame>,
    eof: bool,
}

impl Decoder for SvgDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        let frame = parse_svg(&packet.data)?;
        self.pending = Some(frame);
        Ok(())
    }
    fn receive_frame(&mut self) -> Result<Frame> {
        match self.pending.take() {
            Some(f) => Ok(Frame::Vector(f)),
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

    #[test]
    fn parses_minimal_svg_with_rect() {
        let src = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50" viewBox="0 0 100 50">
  <rect x="10" y="10" width="80" height="30" fill="red"/>
</svg>"#;
        let frame = parse_svg(src).unwrap();
        assert_eq!(frame.width, 100.0);
        assert_eq!(frame.height, 50.0);
        assert!(frame.view_box.is_some());
        assert_eq!(frame.root.children.len(), 1);
    }

    #[test]
    fn parses_svg_without_explicit_dimensions_falls_back_to_viewbox() {
        let src = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"></svg>"#;
        let frame = parse_svg(src).unwrap();
        assert_eq!(frame.width, 64.0);
        assert_eq!(frame.height, 64.0);
    }

    #[test]
    fn rejects_non_svg_input() {
        let src = b"<html><body/></html>";
        assert!(parse_svg(src).is_err());
    }

    #[test]
    fn parses_gradient_def_and_resolves_url_reference() {
        let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <defs>
                <linearGradient id="g">
                    <stop offset="0" stop-color="#ff0000"/>
                    <stop offset="1" stop-color="#0000ff"/>
                </linearGradient>
            </defs>
            <rect x="0" y="0" width="10" height="10" fill="url(#g)"/>
        </svg>"##;
        let frame = parse_svg(src).unwrap();
        let path = match &frame.root.children[0] {
            oxideav_core::Node::Path(p) => p,
            _ => panic!("expected path"),
        };
        match &path.fill {
            Some(oxideav_core::Paint::LinearGradient(g)) => {
                assert_eq!(g.stops.len(), 2);
            }
            other => panic!("expected linear gradient, got {:?}", other),
        }
    }
}
