//! `oxideav-core` integration layer — everything that needs the
//! framework, gated behind the default-on `registry` feature so
//! image-library consumers can depend on `oxideav-svg` with
//! `default-features = false` and skip the framework dependency tree.
//!
//! The module exposes:
//! * [`register`] (the fleet `RuntimeContext` entry point, also what
//!   the `oxideav_core::register!` macro dispatches),
//!   [`register_codecs`] / [`register_containers`] for callers holding
//!   the sub-registries;
//! * [`make_decoder`] / [`make_encoder`] — the codec factories; the
//!   framework `Decoder` / `Encoder` are thin adapters over
//!   [`crate::parse`] / [`crate::write`] (one implementation) that
//!   speak `Frame::Vector`;
//! * the document bridge: `From<SvgDocument> for VectorFrame` and
//!   `From<VectorFrame> for SvgDocument` (field-for-field; the frame's
//!   `Node::Image` raster placements are dropped, see
//!   [`node_from_core`]), plus the node-level [`node_to_core`] /
//!   [`node_from_core`];
//! * the raster bridge every image crate has: `From<SvgImage> for
//!   VideoFrame`, [`SvgImage::from_video_frame`] and
//!   `TryFrom<(&VideoFrame, &CodecParameters)>`, with the 1:1
//!   [`SvgPixelFormat`] ↔ `oxideav_core::PixelFormat` name mapping;
//! * the `From<SvgError> for oxideav_core::Error` conversion;
//! * the deprecated `parse_svg*` / `write_svg*` wrappers that keep
//!   returning / taking `VectorFrame` for one release.

use oxideav_core::{
    CodecCapabilities, CodecId, CodecInfo, CodecParameters, CodecRegistry, ColorPrimaries,
    ColorSignal, ContainerRegistry, Decoder, Encoder, Frame, MatrixCoefficients, Packet,
    PixelFormat, RuntimeContext, TimeBase, TransferCharacteristics, VectorFrame, VideoFrame,
    VideoPlane,
};

use crate::decoder::CODEC_ID_STR;
use crate::error::SvgError;
use crate::model::{self, SvgDocument};
use crate::picture::{ColorInfo, ColorRange, Plane, SvgImage, SvgPixelFormat};
use crate::preserved::PreservedExtras;

// ---- Error mapping ----

impl From<SvgError> for oxideav_core::Error {
    fn from(e: SvgError) -> Self {
        match e {
            SvgError::InvalidData(s) => oxideav_core::Error::InvalidData(s),
            SvgError::Unsupported(s) => oxideav_core::Error::Unsupported(s),
            SvgError::LimitExceeded(s) => oxideav_core::Error::ResourceExhausted(s),
            SvgError::Io(e) => oxideav_core::Error::Io(e),
        }
    }
}

// ---- Document bridge (vector IR, field for field) ----

fn point_to_core(p: model::Point) -> oxideav_core::Point {
    oxideav_core::Point::new(p.x, p.y)
}

fn point_from_core(p: oxideav_core::Point) -> model::Point {
    model::Point::new(p.x, p.y)
}

fn transform_to_core(t: &model::Transform2D) -> oxideav_core::Transform2D {
    oxideav_core::Transform2D {
        a: t.a,
        b: t.b,
        c: t.c,
        d: t.d,
        e: t.e,
        f: t.f,
    }
}

fn transform_from_core(t: &oxideav_core::Transform2D) -> model::Transform2D {
    model::Transform2D {
        a: t.a,
        b: t.b,
        c: t.c,
        d: t.d,
        e: t.e,
        f: t.f,
    }
}

fn rgba_to_core(c: model::Rgba) -> oxideav_core::Rgba {
    oxideav_core::Rgba::new(c.r, c.g, c.b, c.a)
}

fn rgba_from_core(c: oxideav_core::Rgba) -> model::Rgba {
    model::Rgba::new(c.r, c.g, c.b, c.a)
}

fn command_to_core(c: &model::PathCommand) -> oxideav_core::PathCommand {
    use model::PathCommand as M;
    use oxideav_core::PathCommand as C;
    match *c {
        M::MoveTo(p) => C::MoveTo(point_to_core(p)),
        M::LineTo(p) => C::LineTo(point_to_core(p)),
        M::QuadCurveTo { control, end } => C::QuadCurveTo {
            control: point_to_core(control),
            end: point_to_core(end),
        },
        M::CubicCurveTo { c1, c2, end } => C::CubicCurveTo {
            c1: point_to_core(c1),
            c2: point_to_core(c2),
            end: point_to_core(end),
        },
        M::ArcTo {
            rx,
            ry,
            x_axis_rot,
            large_arc,
            sweep,
            end,
        } => C::ArcTo {
            rx,
            ry,
            x_axis_rot,
            large_arc,
            sweep,
            end: point_to_core(end),
        },
        M::Close => C::Close,
    }
}

fn command_from_core(c: &oxideav_core::PathCommand) -> Option<model::PathCommand> {
    use model::PathCommand as M;
    use oxideav_core::PathCommand as C;
    Some(match *c {
        C::MoveTo(p) => M::MoveTo(point_from_core(p)),
        C::LineTo(p) => M::LineTo(point_from_core(p)),
        C::QuadCurveTo { control, end } => M::QuadCurveTo {
            control: point_from_core(control),
            end: point_from_core(end),
        },
        C::CubicCurveTo { c1, c2, end } => M::CubicCurveTo {
            c1: point_from_core(c1),
            c2: point_from_core(c2),
            end: point_from_core(end),
        },
        C::ArcTo {
            rx,
            ry,
            x_axis_rot,
            large_arc,
            sweep,
            end,
        } => M::ArcTo {
            rx,
            ry,
            x_axis_rot,
            large_arc,
            sweep,
            end: point_from_core(end),
        },
        C::Close => M::Close,
        // `oxideav_core::PathCommand` is `#[non_exhaustive]`; a command
        // this crate does not know cannot be written as SVG `d` data.
        _ => return None,
    })
}

/// [`model::Path`] → `oxideav_core::Path`.
pub fn path_to_core(p: &model::Path) -> oxideav_core::Path {
    oxideav_core::Path {
        commands: p.commands.iter().map(command_to_core).collect(),
    }
}

/// `oxideav_core::Path` → [`model::Path`] (unknown future commands are
/// skipped).
pub fn path_from_core(p: &oxideav_core::Path) -> model::Path {
    model::Path {
        commands: p.commands.iter().filter_map(command_from_core).collect(),
    }
}

fn stop_to_core(s: &model::GradientStop) -> oxideav_core::GradientStop {
    oxideav_core::GradientStop::new(s.offset, rgba_to_core(s.color))
}

fn stop_from_core(s: &oxideav_core::GradientStop) -> model::GradientStop {
    model::GradientStop::new(s.offset, rgba_from_core(s.color))
}

fn spread_to_core(s: model::SpreadMethod) -> oxideav_core::SpreadMethod {
    match s {
        model::SpreadMethod::Pad => oxideav_core::SpreadMethod::Pad,
        model::SpreadMethod::Reflect => oxideav_core::SpreadMethod::Reflect,
        model::SpreadMethod::Repeat => oxideav_core::SpreadMethod::Repeat,
    }
}

fn spread_from_core(s: oxideav_core::SpreadMethod) -> model::SpreadMethod {
    match s {
        oxideav_core::SpreadMethod::Pad => model::SpreadMethod::Pad,
        oxideav_core::SpreadMethod::Reflect => model::SpreadMethod::Reflect,
        oxideav_core::SpreadMethod::Repeat => model::SpreadMethod::Repeat,
    }
}

/// [`model::Paint`] → `oxideav_core::Paint`.
pub fn paint_to_core(p: &model::Paint) -> oxideav_core::Paint {
    match p {
        model::Paint::Solid(c) => oxideav_core::Paint::Solid(rgba_to_core(*c)),
        model::Paint::LinearGradient(g) => {
            oxideav_core::Paint::LinearGradient(oxideav_core::LinearGradient {
                start: point_to_core(g.start),
                end: point_to_core(g.end),
                stops: g.stops.iter().map(stop_to_core).collect(),
                spread: spread_to_core(g.spread),
            })
        }
        model::Paint::RadialGradient(g) => {
            oxideav_core::Paint::RadialGradient(oxideav_core::RadialGradient {
                center: point_to_core(g.center),
                radius: g.radius,
                focal: g.focal.map(point_to_core),
                stops: g.stops.iter().map(stop_to_core).collect(),
                spread: spread_to_core(g.spread),
            })
        }
    }
}

/// `oxideav_core::Paint` → [`model::Paint`]; `None` for a paint
/// variant this crate does not know (the enum is `#[non_exhaustive]`).
pub fn paint_from_core(p: &oxideav_core::Paint) -> Option<model::Paint> {
    Some(match p {
        oxideav_core::Paint::Solid(c) => model::Paint::Solid(rgba_from_core(*c)),
        oxideav_core::Paint::LinearGradient(g) => {
            model::Paint::LinearGradient(model::LinearGradient {
                start: point_from_core(g.start),
                end: point_from_core(g.end),
                stops: g.stops.iter().map(stop_from_core).collect(),
                spread: spread_from_core(g.spread),
            })
        }
        oxideav_core::Paint::RadialGradient(g) => {
            model::Paint::RadialGradient(model::RadialGradient {
                center: point_from_core(g.center),
                radius: g.radius,
                focal: g.focal.map(point_from_core),
                stops: g.stops.iter().map(stop_from_core).collect(),
                spread: spread_from_core(g.spread),
            })
        }
        _ => return None,
    })
}

fn stroke_to_core(s: &model::Stroke) -> oxideav_core::Stroke {
    oxideav_core::Stroke {
        width: s.width,
        paint: paint_to_core(&s.paint),
        cap: match s.cap {
            model::LineCap::Butt => oxideav_core::LineCap::Butt,
            model::LineCap::Round => oxideav_core::LineCap::Round,
            model::LineCap::Square => oxideav_core::LineCap::Square,
        },
        join: match s.join {
            model::LineJoin::Miter => oxideav_core::LineJoin::Miter,
            model::LineJoin::Round => oxideav_core::LineJoin::Round,
            model::LineJoin::Bevel => oxideav_core::LineJoin::Bevel,
        },
        miter_limit: s.miter_limit,
        dash: s.dash.as_ref().map(|d| oxideav_core::DashPattern {
            array: d.array.clone(),
            offset: d.offset,
        }),
    }
}

fn stroke_from_core(s: &oxideav_core::Stroke) -> Option<model::Stroke> {
    Some(model::Stroke {
        width: s.width,
        paint: paint_from_core(&s.paint)?,
        cap: match s.cap {
            oxideav_core::LineCap::Butt => model::LineCap::Butt,
            oxideav_core::LineCap::Round => model::LineCap::Round,
            oxideav_core::LineCap::Square => model::LineCap::Square,
        },
        join: match s.join {
            oxideav_core::LineJoin::Miter => model::LineJoin::Miter,
            oxideav_core::LineJoin::Round => model::LineJoin::Round,
            oxideav_core::LineJoin::Bevel => model::LineJoin::Bevel,
        },
        miter_limit: s.miter_limit,
        dash: s.dash.as_ref().map(|d| model::DashPattern {
            array: d.array.clone(),
            offset: d.offset,
        }),
    })
}

fn fill_rule_to_core(r: model::FillRule) -> oxideav_core::FillRule {
    match r {
        model::FillRule::NonZero => oxideav_core::FillRule::NonZero,
        model::FillRule::EvenOdd => oxideav_core::FillRule::EvenOdd,
    }
}

fn fill_rule_from_core(r: oxideav_core::FillRule) -> model::FillRule {
    match r {
        oxideav_core::FillRule::NonZero => model::FillRule::NonZero,
        oxideav_core::FillRule::EvenOdd => model::FillRule::EvenOdd,
    }
}

fn mask_kind_to_core(k: model::MaskKind) -> oxideav_core::MaskKind {
    match k {
        model::MaskKind::Luminance => oxideav_core::MaskKind::Luminance,
        model::MaskKind::Alpha => oxideav_core::MaskKind::Alpha,
    }
}

fn mask_kind_from_core(k: oxideav_core::MaskKind) -> model::MaskKind {
    match k {
        oxideav_core::MaskKind::Luminance => model::MaskKind::Luminance,
        oxideav_core::MaskKind::Alpha => model::MaskKind::Alpha,
    }
}

/// [`model::Group`] → `oxideav_core::Group`.
pub fn group_to_core(g: &model::Group) -> oxideav_core::Group {
    oxideav_core::Group {
        transform: transform_to_core(&g.transform),
        opacity: g.opacity,
        clip: g.clip.as_ref().map(path_to_core),
        children: g.children.iter().map(node_to_core).collect(),
        cache_key: g.cache_key,
    }
}

/// `oxideav_core::Group` → [`model::Group`] (see [`node_from_core`] for
/// what is dropped).
pub fn group_from_core(g: &oxideav_core::Group) -> model::Group {
    model::Group {
        transform: transform_from_core(&g.transform),
        opacity: g.opacity,
        clip: g.clip.as_ref().map(path_from_core),
        children: g.children.iter().filter_map(node_from_core_opt).collect(),
        cache_key: g.cache_key,
    }
}

/// [`model::Node`] → `oxideav_core::Node`, structure preserved one to
/// one (scene-graph tree-paths stay valid, so a
/// [`PreservedExtras`] side-channel keyed on them applies to the
/// converted tree).
pub fn node_to_core(n: &model::Node) -> oxideav_core::Node {
    match n {
        model::Node::Path(p) => oxideav_core::Node::Path(oxideav_core::PathNode {
            path: path_to_core(&p.path),
            fill: p.fill.as_ref().map(paint_to_core),
            stroke: p.stroke.as_ref().map(stroke_to_core),
            fill_rule: fill_rule_to_core(p.fill_rule),
        }),
        model::Node::Group(g) => oxideav_core::Node::Group(group_to_core(g)),
        model::Node::SoftMask {
            mask,
            mask_kind,
            content,
        } => oxideav_core::Node::SoftMask {
            mask: Box::new(node_to_core(mask)),
            mask_kind: mask_kind_to_core(*mask_kind),
            content: Box::new(node_to_core(content)),
        },
    }
}

/// `oxideav_core::Node` → [`model::Node`].
///
/// The SVG model has no raster node, so `Node::Image` (and any future
/// framework variant) converts to an **empty group** rather than
/// vanishing: tree-paths of the surrounding siblings stay aligned
/// with the source frame, and the SVG writer — which never serialised
/// `Node::Image` — emits nothing for an empty group. A stroke or fill
/// whose paint is a variant this crate does not know is dropped.
pub fn node_from_core(n: &oxideav_core::Node) -> model::Node {
    node_from_core_opt(n).unwrap_or_else(|| model::Node::Group(model::Group::default()))
}

fn node_from_core_opt(n: &oxideav_core::Node) -> Option<model::Node> {
    Some(match n {
        oxideav_core::Node::Path(p) => model::Node::Path(model::PathNode {
            path: path_from_core(&p.path),
            fill: p.fill.as_ref().and_then(paint_from_core),
            stroke: p.stroke.as_ref().and_then(stroke_from_core),
            fill_rule: fill_rule_from_core(p.fill_rule),
        }),
        oxideav_core::Node::Group(g) => model::Node::Group(group_from_core(g)),
        oxideav_core::Node::SoftMask {
            mask,
            mask_kind,
            content,
        } => model::Node::SoftMask {
            mask: Box::new(node_from_core(mask)),
            mask_kind: mask_kind_from_core(*mask_kind),
            content: Box::new(node_from_core(content)),
        },
        _ => model::Node::Group(model::Group::default()),
    })
}

fn view_box_to_core(vb: model::ViewBox) -> oxideav_core::ViewBox {
    oxideav_core::ViewBox::new(vb.min_x, vb.min_y, vb.width, vb.height)
}

fn view_box_from_core(vb: oxideav_core::ViewBox) -> model::ViewBox {
    model::ViewBox::new(vb.min_x, vb.min_y, vb.width, vb.height)
}

/// [`SvgDocument`] → `VectorFrame` with `pts` stamped (`time_base`
/// `1/1`).
pub fn document_into_frame(doc: &SvgDocument, pts: Option<i64>) -> VectorFrame {
    VectorFrame {
        width: doc.width,
        height: doc.height,
        view_box: doc.view_box.map(view_box_to_core),
        root: group_to_core(&doc.root),
        pts,
        time_base: TimeBase::new(1, 1),
    }
}

/// `VectorFrame` → [`SvgDocument`] (timing is dropped; see
/// [`node_from_core`] for the raster-node rule).
pub fn document_from_frame(frame: &VectorFrame) -> SvgDocument {
    SvgDocument {
        width: frame.width,
        height: frame.height,
        view_box: frame.view_box.map(view_box_from_core),
        root: group_from_core(&frame.root),
    }
}

impl From<SvgDocument> for VectorFrame {
    fn from(doc: SvgDocument) -> Self {
        document_into_frame(&doc, None)
    }
}

impl From<&SvgDocument> for VectorFrame {
    fn from(doc: &SvgDocument) -> Self {
        document_into_frame(doc, None)
    }
}

impl From<VectorFrame> for SvgDocument {
    fn from(frame: VectorFrame) -> Self {
        document_from_frame(&frame)
    }
}

impl From<&VectorFrame> for SvgDocument {
    fn from(frame: &VectorFrame) -> Self {
        document_from_frame(frame)
    }
}

// ---- Deprecated VectorFrame-typed wrappers (one release) ----

/// Parse into a framework `VectorFrame`.
#[deprecated(note = "use oxideav_svg::parse (IMAGE_CRATE_API); VectorFrame::from(doc) converts")]
pub fn parse_svg(bytes: &[u8]) -> oxideav_core::Result<VectorFrame> {
    Ok(crate::decoder::parse(bytes)?.into())
}

/// Parse at a timeline point into a framework `VectorFrame`.
#[deprecated(note = "use oxideav_svg::parse_at (IMAGE_CRATE_API)")]
pub fn parse_svg_at(bytes: &[u8], t_seconds: f32) -> oxideav_core::Result<VectorFrame> {
    Ok(crate::decoder::parse_at(bytes, t_seconds)?.into())
}

/// Parse with a language list into a framework `VectorFrame`.
#[deprecated(note = "use oxideav_svg::parse_at_with_languages (IMAGE_CRATE_API)")]
pub fn parse_svg_at_with_languages(
    bytes: &[u8],
    t_seconds: f32,
    system_language: &[&str],
) -> oxideav_core::Result<VectorFrame> {
    Ok(crate::decoder::parse_at_with_languages(bytes, t_seconds, system_language)?.into())
}

/// Parse into a framework `VectorFrame` plus the side-channel.
#[deprecated(note = "use oxideav_svg::parse_with_extras (IMAGE_CRATE_API)")]
pub fn parse_svg_with_extras(bytes: &[u8]) -> oxideav_core::Result<(VectorFrame, PreservedExtras)> {
    let (doc, extras) = crate::decoder::parse_with_extras(bytes)?;
    Ok((doc.into(), extras))
}

/// Serialise a framework `VectorFrame`.
#[deprecated(note = "use oxideav_svg::write (IMAGE_CRATE_API); SvgDocument::from(frame) converts")]
pub fn write_svg(frame: &VectorFrame) -> Vec<u8> {
    crate::encoder::write(&document_from_frame(frame))
}

/// Serialise a framework `VectorFrame` with the side-channel.
#[deprecated(note = "use oxideav_svg::write_with_extras (IMAGE_CRATE_API)")]
pub fn write_svg_with_extras(frame: &VectorFrame, extras: &PreservedExtras) -> Vec<u8> {
    crate::encoder::write_with_extras(&document_from_frame(frame), extras)
}

// ---- Pixel-format and colour mapping (1:1 by name) ----

/// The 1:1 name mapping from the framework enum to [`SvgPixelFormat`].
pub fn from_core_pixel_format(pf: PixelFormat) -> oxideav_core::Result<SvgPixelFormat> {
    Ok(match pf {
        PixelFormat::Rgba => SvgPixelFormat::Rgba,
        PixelFormat::Rgb24 => SvgPixelFormat::Rgb24,
        other => {
            return Err(oxideav_core::Error::unsupported(format!(
                "SVG: pixel format {other:?} not supported"
            )))
        }
    })
}

/// The 1:1 name mapping from [`SvgPixelFormat`] to the framework enum.
pub fn to_core_pixel_format(pf: SvgPixelFormat) -> PixelFormat {
    match pf {
        SvgPixelFormat::Rgba => PixelFormat::Rgba,
        SvgPixelFormat::Rgb24 => PixelFormat::Rgb24,
    }
}

impl From<SvgPixelFormat> for PixelFormat {
    fn from(pf: SvgPixelFormat) -> Self {
        to_core_pixel_format(pf)
    }
}

impl TryFrom<PixelFormat> for SvgPixelFormat {
    type Error = oxideav_core::Error;
    fn try_from(pf: PixelFormat) -> oxideav_core::Result<Self> {
        from_core_pixel_format(pf)
    }
}

/// [`ColorInfo`] as the framework's [`ColorSignal`] (code points map
/// 1:1; `Unspecified` range stays unspecified).
pub fn to_color_signal(c: &ColorInfo) -> ColorSignal {
    let range = match c.range {
        ColorRange::Unspecified => oxideav_core::ColorRange::Unspecified,
        ColorRange::Limited => oxideav_core::ColorRange::Limited,
        ColorRange::Full => oxideav_core::ColorRange::Full,
    };
    ColorSignal::new(
        range,
        ColorPrimaries(c.primaries),
        TransferCharacteristics(c.transfer),
        MatrixCoefficients(c.matrix),
    )
}

/// The inverse of [`to_color_signal`].
pub fn from_color_signal(s: &ColorSignal) -> ColorInfo {
    let range = match s.range {
        oxideav_core::ColorRange::Limited => ColorRange::Limited,
        oxideav_core::ColorRange::Full => ColorRange::Full,
        _ => ColorRange::Unspecified,
    };
    ColorInfo::new(range, s.primaries.0, s.transfer.0, s.matrix.0)
}

// ---- Raster frame bridge ----

/// [`SvgImage`] → `VideoFrame` with `pts` stamped: the single packed
/// plane plus the colour-signal side-channel. SVG defines its colour
/// space (sRGB), so the signal is always stamped.
pub fn image_into_video_frame(mut image: SvgImage, pts: Option<i64>) -> VideoFrame {
    let stride = image.stride();
    let data = if image.planes.is_empty() {
        Vec::new()
    } else {
        std::mem::take(&mut image.planes[0].data)
    };
    let mut frame = VideoFrame {
        pts,
        planes: vec![VideoPlane { stride, data }],
    };
    frame.set_color_signal(to_color_signal(&image.color));
    frame
}

impl From<SvgImage> for VideoFrame {
    fn from(image: SvgImage) -> Self {
        image_into_video_frame(image, None)
    }
}

impl From<&SvgImage> for VideoFrame {
    fn from(image: &SvgImage) -> Self {
        image_into_video_frame(image.clone(), None)
    }
}

impl SvgImage {
    /// Rebuild an image from a framework frame and the stream
    /// parameters that describe it (`width`, `height` required;
    /// `pixel_format` defaults to `Rgba`). The frame's colour-signal
    /// side-channel, when attached, becomes `color`; otherwise the SVG
    /// default (sRGB) applies. The geometry is validated.
    pub fn from_video_frame(frame: &VideoFrame, params: &CodecParameters) -> crate::Result<Self> {
        let width = params
            .width
            .ok_or_else(|| SvgError::invalid("SVG: missing width"))?;
        let height = params
            .height
            .ok_or_else(|| SvgError::invalid("SVG: missing height"))?;
        let pix = from_core_pixel_format(params.pixel_format.unwrap_or(PixelFormat::Rgba))
            .map_err(|e| SvgError::unsupported(e.to_string()))?;
        let plane = frame
            .image_planes()
            .first()
            .ok_or_else(|| SvgError::invalid("SVG: frame has no planes"))?;
        let mut img = SvgImage::new(
            width,
            height,
            pix,
            vec![Plane::new(plane.stride, plane.data.clone())],
        )?;
        if let Some(sig) = frame.color_signal() {
            img.color = from_color_signal(&sig);
        }
        Ok(img)
    }
}

impl TryFrom<(&VideoFrame, &CodecParameters)> for SvgImage {
    type Error = SvgError;
    fn try_from((frame, params): (&VideoFrame, &CodecParameters)) -> crate::Result<Self> {
        SvgImage::from_video_frame(frame, params)
    }
}

// ---- Decoder / Encoder trait adapters + factories ----

/// Codec-registry decoder factory. Consumes one packet (the entire SVG
/// file) and produces one `Frame::Vector` — [`crate::parse`] followed
/// by the document bridge.
pub fn make_decoder(_params: &CodecParameters) -> oxideav_core::Result<Box<dyn Decoder>> {
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
    fn send_packet(&mut self, packet: &Packet) -> oxideav_core::Result<()> {
        let doc = crate::decoder::parse(&packet.data)?;
        // `pts` stays `None`, as the adapter has always reported: an
        // SVG is a single still frame without a timeline position.
        self.pending = Some(document_into_frame(&doc, None));
        Ok(())
    }
    fn receive_frame(&mut self) -> oxideav_core::Result<Frame> {
        match self.pending.take() {
            Some(f) => Ok(Frame::Vector(f)),
            None => {
                if self.eof {
                    Err(oxideav_core::Error::Eof)
                } else {
                    Err(oxideav_core::Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> oxideav_core::Result<()> {
        self.eof = true;
        Ok(())
    }
}

/// Codec-registry encoder factory. Consumes one `Frame::Vector` and
/// produces one packet holding [`crate::write`]'s output.
pub fn make_encoder(_params: &CodecParameters) -> oxideav_core::Result<Box<dyn Encoder>> {
    let mut out_params = CodecParameters::video(CodecId::new(CODEC_ID_STR));
    out_params.media_type = oxideav_core::MediaType::Video;
    Ok(Box::new(SvgEncoder {
        codec_id: CodecId::new(CODEC_ID_STR),
        out_params,
        pending: None,
        eof: false,
    }))
}

struct SvgEncoder {
    codec_id: CodecId,
    out_params: CodecParameters,
    pending: Option<Vec<u8>>,
    eof: bool,
}

impl Encoder for SvgEncoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn output_params(&self) -> &CodecParameters {
        &self.out_params
    }
    fn send_frame(&mut self, frame: &Frame) -> oxideav_core::Result<()> {
        let vf = match frame {
            Frame::Vector(v) => v,
            _ => {
                return Err(oxideav_core::Error::invalid(
                    "SVG encoder: expected vector frame",
                ))
            }
        };
        self.pending = Some(crate::encoder::write(&document_from_frame(vf)));
        Ok(())
    }
    fn receive_packet(&mut self) -> oxideav_core::Result<Packet> {
        match self.pending.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
            None => {
                if self.eof {
                    Err(oxideav_core::Error::Eof)
                } else {
                    Err(oxideav_core::Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> oxideav_core::Result<()> {
        self.eof = true;
        Ok(())
    }
}

// ---- Registration ----

/// Register the SVG codec (decoder + encoder) on `reg`.
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("svg_sw")
        .with_intra_only(true)
        .with_lossless(true)
        // SVG is resolution-independent — pick a generous cap that
        // mirrors the rest of the image-format crates so the registry
        // doesn't apply implementation-side limits.
        .with_max_size(65535, 65535);
    reg.register(
        CodecInfo::new(CodecId::new(CODEC_ID_STR))
            .capabilities(caps)
            .decoder(make_decoder)
            .encoder(make_encoder),
    );
}

/// Register the SVG container (demuxer + muxer + extensions + probe).
pub fn register_containers(reg: &mut ContainerRegistry) {
    crate::container::register(reg);
}

/// Unified registration entry point — installs the SVG codec into the
/// codec sub-registry and the SVG container into the container
/// sub-registry of the supplied [`RuntimeContext`].
///
/// Also wired into `oxideav_meta::register_all` via the
/// `oxideav_core::register!` macro.
pub fn register(ctx: &mut RuntimeContext) {
    register_codecs(&mut ctx.codecs);
    register_containers(&mut ctx.containers);
}

oxideav_core::register!("svg", register);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Group, Node, Paint, Path, PathNode, Point, Rgba, Stroke, ViewBox};

    fn sample_doc() -> SvgDocument {
        let mut path = Path::new();
        path.move_to(Point::new(0.0, 0.0))
            .line_to(Point::new(10.0, 0.0))
            .quad_to(Point::new(10.0, 10.0), Point::new(0.0, 10.0))
            .cubic_to(
                Point::new(0.0, 5.0),
                Point::new(5.0, 5.0),
                Point::new(0.0, 0.0),
            )
            .close();
        path.commands.push(model::PathCommand::ArcTo {
            rx: 1.0,
            ry: 2.0,
            x_axis_rot: 0.5,
            large_arc: true,
            sweep: false,
            end: Point::new(3.0, 4.0),
        });
        let node = PathNode::new(path)
            .with_fill(Paint::LinearGradient(
                model::LinearGradient::new(Point::new(0.0, 0.0), Point::new(1.0, 1.0))
                    .with_stop(model::GradientStop::new(0.0, Rgba::opaque(1, 2, 3)))
                    .with_stop(model::GradientStop::new(1.0, Rgba::new(4, 5, 6, 7)))
                    .with_spread(model::SpreadMethod::Reflect),
            ))
            .with_stroke(
                Stroke::solid(2.0, Rgba::opaque(9, 9, 9))
                    .with_cap(model::LineCap::Round)
                    .with_join(model::LineJoin::Bevel)
                    .with_dash(model::DashPattern::new(vec![1.0, 2.0]).with_offset(0.5)),
            )
            .with_fill_rule(model::FillRule::EvenOdd);
        let inner = Group::new()
            .with_transform(model::Transform2D::translate(1.0, 2.0))
            .with_opacity(0.5)
            .with_cache_key(42)
            .with_child(Node::Path(node));
        let mask = Node::SoftMask {
            mask: Box::new(Node::Group(Group::default())),
            mask_kind: model::MaskKind::Alpha,
            content: Box::new(Node::Group(inner)),
        };
        SvgDocument::new(20.0, 10.0)
            .with_view_box(ViewBox::new(0.0, 0.0, 40.0, 20.0))
            .with_root(Group::new().with_child(mask))
    }

    #[test]
    fn document_round_trips_through_vector_frame_byte_exactly() {
        let doc = sample_doc();
        let frame: VectorFrame = (&doc).into();
        assert_eq!(frame.width, 20.0);
        assert_eq!(frame.view_box.unwrap().width, 40.0);
        let back: SvgDocument = frame.into();
        assert_eq!(crate::encoder::write(&doc), crate::encoder::write(&back));
    }

    #[test]
    fn image_nodes_become_empty_groups_keeping_tree_paths() {
        let img = oxideav_core::Node::Image(oxideav_core::ImageRef {
            frame: Box::new(VideoFrame {
                pts: None,
                planes: vec![],
            }),
            bounds: oxideav_core::Rect::new(0.0, 0.0, 1.0, 1.0),
            transform: oxideav_core::Transform2D::identity(),
        });
        let frame = VectorFrame::new(1.0, 1.0).with_root(
            oxideav_core::Group::new()
                .with_child(img)
                .with_child(oxideav_core::Node::Group(oxideav_core::Group::new())),
        );
        let doc = document_from_frame(&frame);
        assert_eq!(doc.root.children.len(), 2);
        assert!(matches!(&doc.root.children[0], Node::Group(g) if g.children.is_empty()));
    }

    #[test]
    fn errors_map_onto_core() {
        let e: oxideav_core::Error = SvgError::limit("x").into();
        assert!(matches!(e, oxideav_core::Error::ResourceExhausted(_)));
        let e: oxideav_core::Error = SvgError::invalid("x").into();
        assert!(matches!(e, oxideav_core::Error::InvalidData(_)));
        let e: oxideav_core::Error = SvgError::unsupported("x").into();
        assert!(matches!(e, oxideav_core::Error::Unsupported(_)));
    }

    #[test]
    fn raster_bridge_round_trips() {
        let img = SvgImage::from_rgba8(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        let frame: VideoFrame = img.clone().into();
        assert_eq!(frame.image_planes().len(), 1);
        assert!(frame.color_signal().is_some());
        let mut params = CodecParameters::video(CodecId::new(CODEC_ID_STR));
        params.width = Some(2);
        params.height = Some(1);
        params.pixel_format = Some(PixelFormat::Rgba);
        let back = SvgImage::try_from((&frame, &params)).unwrap();
        assert_eq!(back, img);
        params.pixel_format = Some(PixelFormat::Yuv420P);
        assert!(matches!(
            SvgImage::from_video_frame(&frame, &params),
            Err(SvgError::Unsupported(_))
        ));
    }

    #[test]
    fn register_via_runtime_context_installs_both_sides() {
        let mut ctx = RuntimeContext::new();
        register(&mut ctx);
        let id = CodecId::new(CODEC_ID_STR);
        assert!(ctx.codecs.has_decoder(&id));
        assert!(ctx.codecs.has_encoder(&id));
        assert_eq!(ctx.containers.container_for_extension("svg"), Some("svg"));
    }

    #[test]
    fn codec_adapters_round_trip_a_vector_frame() {
        let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#0000ff"/></svg>"##;
        let params = CodecParameters::video(CodecId::new(CODEC_ID_STR));
        let mut dec = make_decoder(&params).unwrap();
        let pkt = Packet::new(0, TimeBase::new(1, 1), src.to_vec());
        dec.send_packet(&pkt).unwrap();
        let frame = dec.receive_frame().unwrap();
        let vf = match &frame {
            Frame::Vector(v) => v,
            _ => panic!("expected vector frame"),
        };
        assert_eq!(vf.pts, None);
        assert_eq!(vf.root.children.len(), 1);
        let mut enc = make_encoder(&params).unwrap();
        enc.send_frame(&frame).unwrap();
        let out = enc.receive_packet().unwrap();
        assert_eq!(
            out.data,
            crate::encoder::write(&crate::decoder::parse(src).unwrap())
        );
        #[allow(deprecated)]
        {
            assert_eq!(out.data, write_svg(&parse_svg(src).unwrap()));
        }
    }
}
