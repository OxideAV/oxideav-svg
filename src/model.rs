//! The crate-local vector model — what [`crate::parse`] produces and
//! [`crate::write`] consumes.
//!
//! SVG is a vector format: the parsed document is a scene graph
//! (paths, paints, strokes, transforms, groups, soft masks), not a
//! pixel buffer. This module is the framework-free home of that scene
//! graph so the crate builds with `default-features = false`. Its
//! shape mirrors `oxideav_core`'s vector IR field for field, and with
//! the `registry` feature on, [`SvgDocument`] converts losslessly to
//! and from `oxideav_core::VectorFrame` (`From` in both directions; see
//! [`crate::registry`]). The one asymmetry is `oxideav_core::Node::Image`
//! (an embedded raster placement): the SVG model has no raster node —
//! `<image>` elements ride the [`crate::preserved::PreservedExtras`]
//! side-channel instead — so a frame-to-document conversion drops it,
//! exactly as the SVG writer always has.
//!
//! Coordinates are `f32` user units; colours are straight (non-
//! premultiplied) 8-bit RGBA; transforms are the SVG `matrix(a b c d e
//! f)` affine form.

/// A parsed SVG document: viewport, optional `viewBox` and the scene
/// graph under `root`.
///
/// `width` / `height` are the root `<svg>` `width` / `height`
/// attributes as user units (percentages and missing values fall back
/// to the `viewBox` size); `view_box` is the root `viewBox`. The
/// SVG 2 §8.2 `preserveAspectRatio` viewport mapping is folded into
/// `root.transform` so a rasteriser that maps the view box onto the
/// viewport with a plain stretch paints the spec result.
#[derive(Clone, Debug)]
pub struct SvgDocument {
    /// Viewport width in user units.
    pub width: f32,
    /// Viewport height in user units.
    pub height: f32,
    /// Optional view box. `None` means `(0, 0, width, height)`.
    pub view_box: Option<ViewBox>,
    /// Root group of the scene.
    pub root: Group,
}

impl SvgDocument {
    /// A document of the given canvas size with an empty root group
    /// and no view box.
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            view_box: None,
            root: Group::default(),
        }
    }

    /// Set the view box.
    pub fn with_view_box(mut self, view_box: ViewBox) -> Self {
        self.view_box = Some(view_box);
        self
    }

    /// Replace the root group.
    pub fn with_root(mut self, root: Group) -> Self {
        self.root = root;
        self
    }
}

impl Default for SvgDocument {
    /// A `0 × 0` document with an empty root.
    fn default() -> Self {
        Self::new(0.0, 0.0)
    }
}

/// The user-coordinate rectangle mapped onto the viewport (the SVG
/// `viewBox` attribute).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewBox {
    /// Left edge of the view box in user units.
    pub min_x: f32,
    /// Top edge of the view box in user units.
    pub min_y: f32,
    /// View-box width in user units.
    pub width: f32,
    /// View-box height in user units.
    pub height: f32,
}

impl ViewBox {
    /// Build a view box from its origin and size.
    pub const fn new(min_x: f32, min_y: f32, width: f32, height: f32) -> Self {
        Self {
            min_x,
            min_y,
            width,
            height,
        }
    }
}

/// One node in the scene tree.
///
/// `#[non_exhaustive]` so a text or filter variant can be added later
/// without breaking downstream `match` arms.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Node {
    /// A drawn path with optional fill and stroke.
    Path(PathNode),
    /// A nested group applying transform / opacity / clip to its children.
    Group(Group),
    /// A soft-mask composite: the `mask` subtree is rasterised and
    /// converted to a per-pixel alpha multiplier (luminance or alpha,
    /// per [`MaskKind`]) applied to the rasterised `content` subtree.
    /// Mirrors SVG `<mask>`.
    SoftMask {
        /// Subtree rasterised to produce the per-pixel opacity
        /// modulator.
        mask: Box<Node>,
        /// How to convert the rasterised mask to a coverage value.
        mask_kind: MaskKind,
        /// Subtree whose pixels are modulated by the mask.
        content: Box<Node>,
    },
}

/// How a soft mask's rasterised pixels become a coverage modulator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaskKind {
    /// Convert the mask's RGB to luminance (ITU-R BT.709 coefficients)
    /// and use it as the per-pixel alpha multiplier — SVG `<mask>`
    /// default (`mask-type="luminance"`).
    #[default]
    Luminance,
    /// Use the mask's own alpha channel — SVG `<mask mask-type="alpha">`.
    Alpha,
}

/// A grouping node — transform / opacity / optional clip applied to
/// all descendants. Mirrors SVG `<g>`.
#[derive(Clone, Debug)]
pub struct Group {
    /// Coordinate transform applied to children. Identity by default.
    pub transform: Transform2D,
    /// Group opacity in `0.0..=1.0`. `1.0` is fully opaque.
    pub opacity: f32,
    /// Optional clip path: children are clipped to its interior (with
    /// the path's own fill rule). `None` means "no clip".
    pub clip: Option<Path>,
    /// Child nodes, painted in order (later children over earlier ones).
    pub children: Vec<Node>,
    /// Opaque cache key a rasteriser may memoise the group's rendered
    /// bitmap under. `None` (the default) means "do not cache".
    pub cache_key: Option<u64>,
}

impl Default for Group {
    fn default() -> Self {
        Self {
            transform: Transform2D::identity(),
            opacity: 1.0,
            clip: None,
            children: Vec::new(),
            cache_key: None,
        }
    }
}

impl Group {
    /// An empty group: identity transform, opacity `1.0`, no clip, no
    /// children, no cache key. Same as [`Group::default`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the transform.
    pub fn with_transform(mut self, transform: Transform2D) -> Self {
        self.transform = transform;
        self
    }

    /// Set the group opacity in `0.0..=1.0`.
    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    /// Set the clip path.
    pub fn with_clip(mut self, clip: Path) -> Self {
        self.clip = Some(clip);
        self
    }

    /// Append a child node.
    pub fn with_child(mut self, child: Node) -> Self {
        self.children.push(child);
        self
    }

    /// Replace the children list wholesale.
    pub fn with_children(mut self, children: Vec<Node>) -> Self {
        self.children = children;
        self
    }

    /// Set the rasteriser cache key. See [`Group::cache_key`].
    pub fn with_cache_key(mut self, key: u64) -> Self {
        self.cache_key = Some(key);
        self
    }
}

/// A drawn path with optional fill and stroke.
#[derive(Clone, Debug)]
pub struct PathNode {
    /// The geometry.
    pub path: Path,
    /// Fill paint; `None` means "no fill".
    pub fill: Option<Paint>,
    /// Stroke style; `None` means "no stroke".
    pub stroke: Option<Stroke>,
    /// Fill rule for self-intersecting / compound paths.
    pub fill_rule: FillRule,
}

impl PathNode {
    /// Build a `PathNode` with `path`, no fill, no stroke, and
    /// `FillRule::NonZero`.
    pub fn new(path: Path) -> Self {
        Self {
            path,
            fill: None,
            stroke: None,
            fill_rule: FillRule::NonZero,
        }
    }

    /// Set the fill paint.
    pub fn with_fill(mut self, fill: Paint) -> Self {
        self.fill = Some(fill);
        self
    }

    /// Set the stroke style.
    pub fn with_stroke(mut self, stroke: Stroke) -> Self {
        self.stroke = Some(stroke);
        self
    }

    /// Set the fill rule.
    pub fn with_fill_rule(mut self, fill_rule: FillRule) -> Self {
        self.fill_rule = fill_rule;
        self
    }
}

/// A geometric path expressed as a sequence of drawing commands, in
/// the local user space of the enclosing group.
#[derive(Clone, Debug, Default)]
pub struct Path {
    /// Drawing commands, executed in order.
    pub commands: Vec<PathCommand>,
}

impl Path {
    /// An empty path with no commands.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a [`PathCommand::MoveTo`].
    pub fn move_to(&mut self, p: Point) -> &mut Self {
        self.commands.push(PathCommand::MoveTo(p));
        self
    }

    /// Append a [`PathCommand::LineTo`].
    pub fn line_to(&mut self, p: Point) -> &mut Self {
        self.commands.push(PathCommand::LineTo(p));
        self
    }

    /// Append a [`PathCommand::QuadCurveTo`].
    pub fn quad_to(&mut self, control: Point, end: Point) -> &mut Self {
        self.commands
            .push(PathCommand::QuadCurveTo { control, end });
        self
    }

    /// Append a [`PathCommand::CubicCurveTo`].
    pub fn cubic_to(&mut self, c1: Point, c2: Point, end: Point) -> &mut Self {
        self.commands
            .push(PathCommand::CubicCurveTo { c1, c2, end });
        self
    }

    /// Append a [`PathCommand::Close`].
    pub fn close(&mut self) -> &mut Self {
        self.commands.push(PathCommand::Close);
        self
    }
}

/// A single path-construction command (the SVG `d` grammar's absolute
/// forms; relative and shorthand forms are resolved by the parser).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum PathCommand {
    /// Start a new subpath (SVG `M`).
    MoveTo(Point),
    /// Straight line from the current point (SVG `L`).
    LineTo(Point),
    /// Quadratic Bezier segment (SVG `Q`).
    QuadCurveTo {
        /// The single quadratic control point.
        control: Point,
        /// Segment end point.
        end: Point,
    },
    /// Cubic Bezier segment (SVG `C`).
    CubicCurveTo {
        /// First control point.
        c1: Point,
        /// Second control point.
        c2: Point,
        /// Segment end point.
        end: Point,
    },
    /// Elliptic arc segment (SVG `A`). `x_axis_rot` is in radians.
    ArcTo {
        /// Ellipse radius along its X axis, in user units.
        rx: f32,
        /// Ellipse radius along its Y axis, in user units.
        ry: f32,
        /// Rotation of the ellipse's X axis, in radians.
        x_axis_rot: f32,
        /// SVG `large-arc-flag`.
        large_arc: bool,
        /// SVG `sweep-flag`.
        sweep: bool,
        /// Arc end point.
        end: Point,
    },
    /// Close the current subpath (SVG `Z`).
    Close,
}

/// 2D point in user-space coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    /// Horizontal coordinate in user units.
    pub x: f32,
    /// Vertical coordinate in user units.
    pub y: f32,
}

impl Point {
    /// Build a point.
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl From<[f32; 2]> for Point {
    fn from([x, y]: [f32; 2]) -> Self {
        Self { x, y }
    }
}

impl From<(f32, f32)> for Point {
    fn from((x, y): (f32, f32)) -> Self {
        Self { x, y }
    }
}

/// A paint server — what fills the inside of a path or strokes its
/// outline.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Paint {
    /// A single flat RGBA colour.
    Solid(Rgba),
    /// Colour stops swept along a straight line.
    LinearGradient(LinearGradient),
    /// Colour stops swept outward from a focal point to a circle.
    RadialGradient(RadialGradient),
}

/// 32-bit straight (non-premultiplied) RGBA colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgba {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Alpha; `255` is opaque.
    pub a: u8,
}

impl Rgba {
    /// Build a colour from its four channels (straight alpha).
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Fully-opaque colour with the given RGB triple.
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
}

impl From<(u8, u8, u8, u8)> for Rgba {
    fn from((r, g, b, a): (u8, u8, u8, u8)) -> Self {
        Self { r, g, b, a }
    }
}

impl From<(u8, u8, u8)> for Rgba {
    fn from((r, g, b): (u8, u8, u8)) -> Self {
        Self { r, g, b, a: 255 }
    }
}

impl From<[u8; 4]> for Rgba {
    fn from([r, g, b, a]: [u8; 4]) -> Self {
        Self { r, g, b, a }
    }
}

impl From<Rgba> for Paint {
    fn from(color: Rgba) -> Self {
        Paint::Solid(color)
    }
}

/// A linear gradient: colour stops sweep along `start` → `end`.
#[derive(Clone, Debug)]
pub struct LinearGradient {
    /// Gradient line start, in user units.
    pub start: Point,
    /// Gradient line end, in user units.
    pub end: Point,
    /// Colour stops, by increasing offset.
    pub stops: Vec<GradientStop>,
    /// What happens outside `0.0..=1.0`.
    pub spread: SpreadMethod,
}

impl LinearGradient {
    /// A gradient from `start` to `end` with no stops and `Pad` spread.
    pub fn new(start: Point, end: Point) -> Self {
        Self {
            start,
            end,
            stops: Vec::new(),
            spread: SpreadMethod::Pad,
        }
    }

    /// Replace the gradient stops.
    pub fn with_stops(mut self, stops: Vec<GradientStop>) -> Self {
        self.stops = stops;
        self
    }

    /// Append a single stop.
    pub fn with_stop(mut self, stop: GradientStop) -> Self {
        self.stops.push(stop);
        self
    }

    /// Set the spread method.
    pub fn with_spread(mut self, spread: SpreadMethod) -> Self {
        self.spread = spread;
        self
    }
}

/// A radial gradient: colour stops sweep from `focal` (default
/// `center`) outward to the circle of `radius` around `center`.
#[derive(Clone, Debug)]
pub struct RadialGradient {
    /// Circle centre, in user units.
    pub center: Point,
    /// Circle radius, in user units.
    pub radius: f32,
    /// Focal point; `None` means `center`.
    pub focal: Option<Point>,
    /// Colour stops, by increasing offset.
    pub stops: Vec<GradientStop>,
    /// What happens outside `0.0..=1.0`.
    pub spread: SpreadMethod,
}

impl RadialGradient {
    /// A gradient around `center` with `radius`, no focal point, no
    /// stops and `Pad` spread.
    pub fn new(center: Point, radius: f32) -> Self {
        Self {
            center,
            radius,
            focal: None,
            stops: Vec::new(),
            spread: SpreadMethod::Pad,
        }
    }

    /// Set the focal point.
    pub fn with_focal(mut self, focal: Point) -> Self {
        self.focal = Some(focal);
        self
    }

    /// Replace the gradient stops.
    pub fn with_stops(mut self, stops: Vec<GradientStop>) -> Self {
        self.stops = stops;
        self
    }

    /// Append a single stop.
    pub fn with_stop(mut self, stop: GradientStop) -> Self {
        self.stops.push(stop);
        self
    }

    /// Set the spread method.
    pub fn with_spread(mut self, spread: SpreadMethod) -> Self {
        self.spread = spread;
        self
    }
}

/// One colour stop along a gradient. `offset` is in `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    /// Position along the gradient, `0.0..=1.0`.
    pub offset: f32,
    /// Colour at that position.
    pub color: Rgba,
}

impl GradientStop {
    /// Build a stop.
    pub const fn new(offset: f32, color: Rgba) -> Self {
        Self { offset, color }
    }
}

/// How a gradient paints outside its `0.0..=1.0` range (SVG
/// `spreadMethod`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpreadMethod {
    /// Extend the end colours.
    #[default]
    Pad,
    /// Mirror the stops.
    Reflect,
    /// Repeat the stops.
    Repeat,
}

/// Stroke style.
#[derive(Clone, Debug)]
pub struct Stroke {
    /// Line width in user units.
    pub width: f32,
    /// Stroke paint.
    pub paint: Paint,
    /// Line-cap style.
    pub cap: LineCap,
    /// Line-join style.
    pub join: LineJoin,
    /// Miter limit (SVG `stroke-miterlimit`).
    pub miter_limit: f32,
    /// Dash pattern; `None` means solid.
    pub dash: Option<DashPattern>,
}

impl Stroke {
    /// A solid-colour stroke with butt caps, miter joins, miter limit
    /// `4.0` and no dash.
    pub fn solid(width: f32, color: Rgba) -> Self {
        Self {
            width,
            paint: Paint::Solid(color),
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 4.0,
            dash: None,
        }
    }

    /// A stroke with the given paint and the SVG default style.
    pub fn new(width: f32, paint: Paint) -> Self {
        Self {
            width,
            paint,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 4.0,
            dash: None,
        }
    }

    /// Replace the paint.
    pub fn with_paint(mut self, paint: Paint) -> Self {
        self.paint = paint;
        self
    }

    /// Set the line cap.
    pub fn with_cap(mut self, cap: LineCap) -> Self {
        self.cap = cap;
        self
    }

    /// Set the line join.
    pub fn with_join(mut self, join: LineJoin) -> Self {
        self.join = join;
        self
    }

    /// Set the miter limit.
    pub fn with_miter_limit(mut self, miter_limit: f32) -> Self {
        self.miter_limit = miter_limit;
        self
    }

    /// Set the dash pattern.
    pub fn with_dash(mut self, dash: DashPattern) -> Self {
        self.dash = Some(dash);
        self
    }
}

/// Line-cap style (SVG `stroke-linecap`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineCap {
    /// Flat end at the endpoint.
    #[default]
    Butt,
    /// Semicircular end.
    Round,
    /// Square end extending half the width.
    Square,
}

/// Line-join style (SVG `stroke-linejoin`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineJoin {
    /// Sharp corner, limited by the miter limit.
    #[default]
    Miter,
    /// Rounded corner.
    Round,
    /// Bevelled corner.
    Bevel,
}

/// Dash pattern (SVG `stroke-dasharray` / `stroke-dashoffset`).
#[derive(Clone, Debug, Default)]
pub struct DashPattern {
    /// Alternating dash / gap lengths in user units.
    pub array: Vec<f32>,
    /// Phase offset from the path start.
    pub offset: f32,
}

impl DashPattern {
    /// A dash pattern with the given lengths and a `0.0` offset.
    pub fn new(array: Vec<f32>) -> Self {
        Self { array, offset: 0.0 }
    }

    /// Set the phase offset.
    pub fn with_offset(mut self, offset: f32) -> Self {
        self.offset = offset;
        self
    }
}

/// Fill rule (SVG `fill-rule`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillRule {
    /// Winding-number rule.
    #[default]
    NonZero,
    /// Even-odd rule.
    EvenOdd,
}

/// 2D affine transform in the SVG `matrix(a b c d e f)` form:
/// `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform2D {
    /// X-scale / rotation term.
    pub a: f32,
    /// Y-skew / rotation term.
    pub b: f32,
    /// X-skew / rotation term.
    pub c: f32,
    /// Y-scale / rotation term.
    pub d: f32,
    /// X translation.
    pub e: f32,
    /// Y translation.
    pub f: f32,
}

impl Transform2D {
    /// The identity transform.
    pub const fn identity() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    /// A translation by `(tx, ty)`.
    pub const fn translate(tx: f32, ty: f32) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: tx,
            f: ty,
        }
    }

    /// A scale by `(sx, sy)` about the origin.
    pub const fn scale(sx: f32, sy: f32) -> Self {
        Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            e: 0.0,
            f: 0.0,
        }
    }

    /// A rotation by `angle_radians` about the origin.
    pub fn rotate(angle_radians: f32) -> Self {
        let (s, c) = angle_radians.sin_cos();
        Self {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        }
    }

    /// A skew along X by `angle_radians`.
    pub fn skew_x(angle_radians: f32) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: angle_radians.tan(),
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    /// A skew along Y by `angle_radians`.
    pub fn skew_y(angle_radians: f32) -> Self {
        Self {
            a: 1.0,
            b: angle_radians.tan(),
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    /// `self ∘ other`: the transform that applies `other` first, then
    /// `self`.
    pub fn compose(&self, other: &Self) -> Self {
        Self {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    /// Apply the transform to a point.
    pub fn apply(&self, p: Point) -> Point {
        Point {
            x: self.a * p.x + self.c * p.y + self.e,
            y: self.b * p.x + self.d * p.y + self.f,
        }
    }

    /// `true` for the exact identity.
    pub fn is_identity(&self) -> bool {
        *self == Self::identity()
    }
}

impl Default for Transform2D {
    fn default() -> Self {
        Self::identity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_applies_right_operand_first() {
        let t = Transform2D::translate(10.0, 0.0).compose(&Transform2D::scale(2.0, 2.0));
        let p = t.apply(Point::new(1.0, 1.0));
        assert_eq!(p, Point::new(12.0, 2.0));
    }

    #[test]
    fn rotate_quarter_turn() {
        let p = Transform2D::rotate(std::f32::consts::FRAC_PI_2).apply(Point::new(1.0, 0.0));
        assert!(p.x.abs() < 1e-6 && (p.y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn path_builder_appends_in_order() {
        let mut p = Path::new();
        p.move_to(Point::new(0.0, 0.0))
            .line_to(Point::new(1.0, 0.0))
            .quad_to(Point::new(1.0, 1.0), Point::new(0.0, 1.0))
            .cubic_to(
                Point::new(0.0, 0.5),
                Point::new(0.5, 0.5),
                Point::new(0.0, 0.0),
            )
            .close();
        assert_eq!(p.commands.len(), 5);
        assert_eq!(p.commands[4], PathCommand::Close);
    }

    #[test]
    fn defaults_match_svg_initial_values() {
        let g = Group::default();
        assert!(g.transform.is_identity());
        assert_eq!(g.opacity, 1.0);
        let s = Stroke::solid(2.0, Rgba::opaque(0, 0, 0));
        assert_eq!(s.cap, LineCap::Butt);
        assert_eq!(s.join, LineJoin::Miter);
        assert_eq!(s.miter_limit, 4.0);
        assert_eq!(FillRule::default(), FillRule::NonZero);
        assert_eq!(SpreadMethod::default(), SpreadMethod::Pad);
        assert_eq!(MaskKind::default(), MaskKind::Luminance);
    }
}
