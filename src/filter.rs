//! Typed parsing of `<filter>` primitive graphs.
//!
//! Round 2-4 captured `<filter>` element trees verbatim and round-tripped
//! them through the encoder, but never inspected the primitives inside.
//! Round 7 added typed parsing for the six most common primitives
//! (`<feGaussianBlur>`, `<feOffset>`, `<feFlood>`, `<feComposite>`,
//! `<feBlend>`, `<feMorphology>`); round 8 extends that to the long
//! tail: `<feColorMatrix>`, `<feMerge>` (with `<feMergeNode>`
//! children), `<feComponentTransfer>` (with `<feFuncR/G/B/A>`
//! children) and `<feDropShadow>` (a composite primitive that the
//! W3C Filter Effects spec defines as a syntactic sugar over
//! GaussianBlur + Offset + Flood + Composite).
//!
//! The graph model mirrors the W3C Filter Effects spec
//! (drafts.fxtf.org/filter-effects-1, referenced from SVG 2 §15):
//!
//! - Each primitive has a region (`x` / `y` / `width` / `height`),
//!   a result label (`result="..."`) addressable by later primitives in
//!   the chain, and one or two named inputs (`in="SourceGraphic"`,
//!   `in2="..."`).
//! - Inputs default to `SourceGraphic` for the first primitive and to
//!   the previous primitive's `result` thereafter, per spec §6.2.
//! - Unknown primitives are skipped (the round-trip preserved-XML path
//!   keeps them via [`crate::preserved::PreservedExtras`]).
//!
//! The parser does *not* perform layout-time region resolution; it just
//! captures the user-supplied numbers. Rasterization-time clipping
//! against the filter region is the rasterizer's job.

use crate::element::parse_number;
use crate::parser::{attr, tag_local, Element, Node as XmlNode};

/// One source of pixels feeding a primitive's `in` / `in2` slot.
#[derive(Clone, Debug, PartialEq)]
pub enum FilterInput {
    /// `SourceGraphic` — the rasterised version of the element the
    /// filter is applied to.
    SourceGraphic,
    /// `SourceAlpha` — `SourceGraphic` reduced to its alpha channel.
    SourceAlpha,
    /// `BackgroundImage` — pixel buffer behind the filter region.
    BackgroundImage,
    /// `BackgroundAlpha` — `BackgroundImage` reduced to its alpha channel.
    BackgroundAlpha,
    /// `FillPaint` — paint server resolved from the filtered element's
    /// `fill` attribute.
    FillPaint,
    /// `StrokePaint` — paint server resolved from the filtered element's
    /// `stroke` attribute.
    StrokePaint,
    /// Reference to an earlier primitive's `result="name"`.
    Reference(String),
}

impl FilterInput {
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "SourceGraphic" => Self::SourceGraphic,
            "SourceAlpha" => Self::SourceAlpha,
            "BackgroundImage" => Self::BackgroundImage,
            "BackgroundAlpha" => Self::BackgroundAlpha,
            "FillPaint" => Self::FillPaint,
            "StrokePaint" => Self::StrokePaint,
            other => Self::Reference(other.to_string()),
        }
    }
}

/// A single filter-primitive node in a filter graph.
///
/// Each variant carries primitive-specific parameters; shared
/// attributes (`x` / `y` / `width` / `height` / `result`) live on the
/// surrounding [`FilterPrimitiveNode`].
#[derive(Clone, Debug, PartialEq)]
pub enum FilterPrimitive {
    /// `<feGaussianBlur stdDeviation="sx [sy]">`. Per Filter Effects §16.
    GaussianBlur {
        input: FilterInput,
        std_deviation_x: f32,
        std_deviation_y: f32,
        edge_mode: EdgeMode,
    },
    /// `<feOffset dx dy>`. Per Filter Effects §17.
    Offset {
        input: FilterInput,
        dx: f32,
        dy: f32,
    },
    /// `<feFlood flood-color flood-opacity>`. Per Filter Effects §15.
    Flood {
        flood_color: FloodColor,
        flood_opacity: f32,
    },
    /// `<feComposite in in2 operator>`. Per Filter Effects §18 + W3C
    /// Compositing & Blending L1.
    Composite {
        input: FilterInput,
        input2: FilterInput,
        operator: CompositeOperator,
        // For `arithmetic`, the four scalars k1..k4. Default 0 each per
        // spec.
        k1: f32,
        k2: f32,
        k3: f32,
        k4: f32,
    },
    /// `<feBlend in in2 mode>`. Per Filter Effects §14.
    Blend {
        input: FilterInput,
        input2: FilterInput,
        mode: BlendMode,
    },
    /// `<feMorphology in operator radius>`. Per Filter Effects §20.
    Morphology {
        input: FilterInput,
        operator: MorphologyOperator,
        radius_x: f32,
        radius_y: f32,
    },
    /// `<feColorMatrix in type values>`. Per Filter Effects §13.
    ///
    /// All four type variants reduce to a flat 4×5 RGBA-bias matrix
    /// — `saturate`, `hueRotate` and `luminanceToAlpha` are computed
    /// at parse time from their respective scalar / fixed templates,
    /// per W3C Filter Effects §13.2.4 / §13.2.5 / §13.2.6.
    ColorMatrix {
        input: FilterInput,
        /// Row-major 4×5 RGBA-bias matrix M.
        /// `out = clamp(M * (R, G, B, A, 1)^T)` per row.
        matrix: [f32; 20],
    },
    /// `<feMerge>` — composites a list of inputs in z-order, oldest
    /// first. Each entry corresponds to one `<feMergeNode in="..."/>`
    /// child. Per Filter Effects §19.
    Merge { inputs: Vec<FilterInput> },
    /// `<feComponentTransfer>` — per-channel transfer function applied
    /// to the input. Each channel inherits a default identity transfer
    /// function when the corresponding `<feFuncR/G/B/A>` child is
    /// missing, per Filter Effects §12.
    ComponentTransfer {
        input: FilterInput,
        red: TransferFunction,
        green: TransferFunction,
        blue: TransferFunction,
        alpha: TransferFunction,
    },
    /// `<feDropShadow dx dy stdDeviation flood-color flood-opacity>`.
    /// Per Filter Effects §22 — equivalent to
    /// `feGaussianBlur(SourceAlpha) → feOffset → feFlood-tinted →
    /// feComposite(in, SourceGraphic, over)`. Stored as a single
    /// primitive so the rasterizer can implement it directly without
    /// synthesising 4 intermediate buffers.
    DropShadow {
        input: FilterInput,
        dx: f32,
        dy: f32,
        std_deviation_x: f32,
        std_deviation_y: f32,
        flood_color: FloodColor,
        flood_opacity: f32,
    },
    /// `<feConvolveMatrix>` — applies a 2-D linear convolution kernel to
    /// the input. Per W3C Filter Effects §15.
    ///
    /// `kernel_matrix` is row-major with `order_x * order_y` entries.
    /// The convolution is `out[x,y] = (1/divisor) * Σ kernel[i,j] *
    /// in[x+targetX-i, y+targetY-j] + bias` per spec §15.5 (with the
    /// flip relative to texture coordinates that the spec mandates).
    ConvolveMatrix {
        input: FilterInput,
        order_x: u32,
        order_y: u32,
        kernel_matrix: Vec<f32>,
        divisor: f32,
        bias: f32,
        target_x: i32,
        target_y: i32,
        edge_mode: ConvolveEdgeMode,
        preserve_alpha: bool,
    },
    /// `<feTurbulence>` — Perlin-noise / fractal-noise primitive.
    /// Per W3C Filter Effects §16.
    ///
    /// `base_frequency` is `(fx, fy)`; if the source attribute supplied
    /// only one number then `fy = fx` per spec §16.3.
    Turbulence {
        base_frequency_x: f32,
        base_frequency_y: f32,
        num_octaves: u32,
        seed: i32,
        stitch_tiles: bool,
        kind: TurbulenceKind,
    },
    /// `<feDisplacementMap>` — uses a channel of `in2` to displace the
    /// pixels of `in`. Per W3C Filter Effects §17.
    ///
    /// The displacement vector at each output pixel is
    /// `(scale * (channel_x(in2) - 0.5), scale * (channel_y(in2) - 0.5))`
    /// per spec §17.5.
    DisplacementMap {
        input: FilterInput,
        input2: FilterInput,
        scale: f32,
        x_channel_selector: ChannelSelector,
        y_channel_selector: ChannelSelector,
    },
}

/// `edgeMode` on `<feGaussianBlur>` (Filter Effects §16). Determines how
/// the convolution behaves at the filter region's edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EdgeMode {
    /// Repeat the edge pixel value (default per spec for `feGaussianBlur`
    /// is `"none"`, but `none` and `duplicate` differ only in a small
    /// number of pixels — we treat absent / unknown as `Duplicate` since
    /// that is the most common visually-correct interpretation).
    #[default]
    Duplicate,
    /// Wrap around (toroidal sampling).
    Wrap,
    /// Sample beyond-edge pixels as transparent black.
    None,
}

impl EdgeMode {
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "wrap" => Self::Wrap,
            "none" => Self::None,
            "duplicate" => Self::Duplicate,
            _ => Self::default(),
        }
    }
}

/// `edgeMode` on `<feConvolveMatrix>` (Filter Effects §15) — same
/// three modes as `<feGaussianBlur>` but the spec defines a different
/// default (`duplicate` for blur, `duplicate` for convolve too — but
/// it's a separate enum because future spec drafts could diverge).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConvolveEdgeMode {
    /// Default per Filter Effects §15: `duplicate`.
    #[default]
    Duplicate,
    /// Toroidal sampling.
    Wrap,
    /// Sample beyond-edge pixels as transparent black.
    None,
}

impl ConvolveEdgeMode {
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "wrap" => Self::Wrap,
            "none" => Self::None,
            "duplicate" => Self::Duplicate,
            _ => Self::default(),
        }
    }
}

/// `<feTurbulence type>` per Filter Effects §16. `Turbulence` uses
/// `|noise|`; `FractalNoise` uses `(noise + 1) / 2`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TurbulenceKind {
    /// Default per spec — `turbulence`.
    #[default]
    Turbulence,
    /// `fractalNoise` — smooth fractal noise.
    FractalNoise,
}

impl TurbulenceKind {
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "fractalNoise" => Self::FractalNoise,
            // `turbulence` and any unknown value default to Turbulence.
            _ => Self::Turbulence,
        }
    }
}

/// `xChannelSelector` / `yChannelSelector` on `<feDisplacementMap>`
/// per Filter Effects §17 — picks which channel of `in2` drives the
/// X / Y displacement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChannelSelector {
    R,
    G,
    B,
    /// Default per spec §17 — `A`.
    #[default]
    A,
}

impl ChannelSelector {
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "R" => Self::R,
            "G" => Self::G,
            "B" => Self::B,
            "A" => Self::A,
            _ => Self::default(),
        }
    }
}

/// `flood-color` on `<feFlood>` — either a CSS colour or `currentColor`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloodColor {
    /// 0..=255 R/G/B/A. `currentColor` resolves to opaque black per the
    /// SVG-1.1 default-foreground convention used elsewhere in this
    /// crate.
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Default for FloodColor {
    fn default() -> Self {
        // Spec default is opaque black.
        Self {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        }
    }
}

/// `<feComposite operator>` per Compositing & Blending §3.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompositeOperator {
    #[default]
    Over,
    In,
    Out,
    Atop,
    Xor,
    /// `arithmetic` — out = k1*i1*i2 + k2*i1 + k3*i2 + k4.
    Arithmetic,
}

impl CompositeOperator {
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "in" => Self::In,
            "out" => Self::Out,
            "atop" => Self::Atop,
            "xor" => Self::Xor,
            "arithmetic" => Self::Arithmetic,
            _ => Self::Over,
        }
    }
}

/// `<feBlend mode>` per Compositing & Blending §6 (CSS Compositing 1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Darken,
    Lighten,
    Overlay,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "multiply" => Self::Multiply,
            "screen" => Self::Screen,
            "darken" => Self::Darken,
            "lighten" => Self::Lighten,
            "overlay" => Self::Overlay,
            "color-dodge" => Self::ColorDodge,
            "color-burn" => Self::ColorBurn,
            "hard-light" => Self::HardLight,
            "soft-light" => Self::SoftLight,
            "difference" => Self::Difference,
            "exclusion" => Self::Exclusion,
            "hue" => Self::Hue,
            "saturation" => Self::Saturation,
            "color" => Self::Color,
            "luminosity" => Self::Luminosity,
            _ => Self::Normal,
        }
    }
}

/// `<feMorphology operator>` per Filter Effects §20 — `erode` shrinks
/// the source by `radius`, `dilate` expands it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MorphologyOperator {
    #[default]
    Erode,
    Dilate,
}

impl MorphologyOperator {
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "dilate" => Self::Dilate,
            _ => Self::Erode,
        }
    }
}

/// Per-channel transfer function for `<feComponentTransfer>` —
/// each `<feFuncR/G/B/A>` child supplies one of these. Per Filter
/// Effects §12.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum TransferFunction {
    /// `type="identity"` — pass-through. Default when no `<feFunc*>`
    /// child is present (per spec §12 default behaviour).
    #[default]
    Identity,
    /// `type="table"` — piecewise-linear lookup table. `values`
    /// supplies n samples in [0,1]; intermediate channel values
    /// linearly interpolate between adjacent samples.
    Table { values: Vec<f32> },
    /// `type="discrete"` — step function. `values` supplies n bins;
    /// the output is `values[floor(c * n)]`.
    Discrete { values: Vec<f32> },
    /// `type="linear"` — `out = slope * c + intercept`.
    Linear { slope: f32, intercept: f32 },
    /// `type="gamma"` — `out = amplitude * pow(c, exponent) + offset`.
    Gamma {
        amplitude: f32,
        exponent: f32,
        offset: f32,
    },
}

/// One node of a parsed filter graph — a [`FilterPrimitive`] plus the
/// shared region / result attributes.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterPrimitiveNode {
    /// Primitive sub-region. `None` means "use the parent filter's
    /// region" — concretely, every primitive defaults to filling the
    /// filter's own region.
    pub region: PrimitiveRegion,
    /// Optional `result="name"` — addressable by `in=`/`in2=` of later
    /// primitives.
    pub result: Option<String>,
    /// The primitive itself.
    pub primitive: FilterPrimitive,
}

/// Sub-region for one primitive. Each component is `None` when the
/// attribute was absent (so the rasterizer can fall back to the parent
/// `<filter>`'s region).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PrimitiveRegion {
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
}

/// A complete `<filter>` element parsed into typed primitives.
///
/// Round-trip emission still uses the original XML in
/// [`crate::defs::FilterDef::element`] — the typed graph is a *parallel*
/// representation for downstream rasterization, not a replacement.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilterGraph {
    /// `<filter x>` / `y` / `width` / `height` — the overall filter
    /// region.  None per-component when the attribute was absent so the
    /// rasterizer can apply the spec defaults (`-10% -10% 120% 120%` of
    /// the bounding box).
    pub region: PrimitiveRegion,
    /// Primitives in source order. Empty means "no recognised
    /// primitives" (a `<filter>` with only unknown children).
    pub primitives: Vec<FilterPrimitiveNode>,
}

/// Walk a `<filter>` element and parse every recognised primitive child.
/// Unknown primitives are silently skipped.
pub fn parse_filter_graph(el: &Element) -> FilterGraph {
    let region = PrimitiveRegion {
        x: parse_attr_number(el, "x"),
        y: parse_attr_number(el, "y"),
        width: parse_attr_number(el, "width"),
        height: parse_attr_number(el, "height"),
    };
    let mut primitives = Vec::new();
    let mut prev_result: Option<String> = None;
    for child in &el.children {
        let XmlNode::Element(c) = child else { continue };
        let local = tag_local(&c.name).to_ascii_lowercase();
        let primitive = match local.as_str() {
            "fegaussianblur" => parse_gaussian_blur(c, &prev_result),
            "feoffset" => parse_offset(c, &prev_result),
            "feflood" => parse_flood(c),
            "fecomposite" => parse_composite(c, &prev_result),
            "feblend" => parse_blend(c, &prev_result),
            "femorphology" => parse_morphology(c, &prev_result),
            "fecolormatrix" => parse_color_matrix(c, &prev_result),
            "femerge" => parse_merge(c, &prev_result),
            "fecomponenttransfer" => parse_component_transfer(c, &prev_result),
            "fedropshadow" => parse_drop_shadow(c, &prev_result),
            "feconvolvematrix" => parse_convolve_matrix(c, &prev_result),
            "feturbulence" => parse_turbulence(c),
            "fedisplacementmap" => parse_displacement_map(c, &prev_result),
            _ => continue,
        };
        let prim_region = PrimitiveRegion {
            x: parse_attr_number(c, "x"),
            y: parse_attr_number(c, "y"),
            width: parse_attr_number(c, "width"),
            height: parse_attr_number(c, "height"),
        };
        let result = attr(c, "result").map(|s| s.trim().to_string());
        if let Some(r) = result.as_deref() {
            prev_result = Some(r.to_string());
        }
        primitives.push(FilterPrimitiveNode {
            region: prim_region,
            result,
            primitive,
        });
    }
    FilterGraph { region, primitives }
}

fn parse_attr_number(el: &Element, name: &str) -> Option<f32> {
    let raw = attr(el, name)?;
    parse_number(Some(raw), 0.0).ok()
}

fn input_or_default(el: &Element, prev: &Option<String>) -> FilterInput {
    match attr(el, "in") {
        Some(s) => FilterInput::from_str(s),
        None => match prev {
            Some(r) => FilterInput::Reference(r.clone()),
            None => FilterInput::SourceGraphic,
        },
    }
}

fn parse_gaussian_blur(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    let (sx, sy) = parse_two_numbers(attr(el, "stdDeviation"));
    let edge_mode = attr(el, "edgeMode")
        .map(EdgeMode::from_str)
        .unwrap_or_default();
    FilterPrimitive::GaussianBlur {
        input: input_or_default(el, prev),
        std_deviation_x: sx,
        std_deviation_y: sy.unwrap_or(sx),
        edge_mode,
    }
}

fn parse_offset(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    FilterPrimitive::Offset {
        input: input_or_default(el, prev),
        dx: parse_number(attr(el, "dx"), 0.0).unwrap_or(0.0),
        dy: parse_number(attr(el, "dy"), 0.0).unwrap_or(0.0),
    }
}

fn parse_flood(el: &Element) -> FilterPrimitive {
    let flood_color = attr(el, "flood-color")
        .map(parse_flood_color)
        .unwrap_or_default();
    let flood_opacity = parse_number(attr(el, "flood-opacity"), 1.0)
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    FilterPrimitive::Flood {
        flood_color,
        flood_opacity,
    }
}

fn parse_composite(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    let operator = attr(el, "operator")
        .map(CompositeOperator::from_str)
        .unwrap_or_default();
    FilterPrimitive::Composite {
        input: input_or_default(el, prev),
        input2: attr(el, "in2")
            .map(FilterInput::from_str)
            .unwrap_or(FilterInput::SourceGraphic),
        operator,
        k1: parse_number(attr(el, "k1"), 0.0).unwrap_or(0.0),
        k2: parse_number(attr(el, "k2"), 0.0).unwrap_or(0.0),
        k3: parse_number(attr(el, "k3"), 0.0).unwrap_or(0.0),
        k4: parse_number(attr(el, "k4"), 0.0).unwrap_or(0.0),
    }
}

fn parse_blend(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    FilterPrimitive::Blend {
        input: input_or_default(el, prev),
        input2: attr(el, "in2")
            .map(FilterInput::from_str)
            .unwrap_or(FilterInput::SourceGraphic),
        mode: attr(el, "mode")
            .map(BlendMode::from_str)
            .unwrap_or_default(),
    }
}

fn parse_morphology(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    let (rx, ry) = parse_two_numbers(attr(el, "radius"));
    FilterPrimitive::Morphology {
        input: input_or_default(el, prev),
        operator: attr(el, "operator")
            .map(MorphologyOperator::from_str)
            .unwrap_or_default(),
        radius_x: rx,
        radius_y: ry.unwrap_or(rx),
    }
}

fn parse_color_matrix(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    let kind = attr(el, "type").map(|s| s.trim().to_ascii_lowercase());
    let values_attr = attr(el, "values");
    // Per Filter Effects §13, `type` defaults to `matrix`.
    let matrix = match kind.as_deref() {
        Some("saturate") => {
            // Per §13.2.4, s defaults to 1 (identity) when values is
            // absent. Clamped to [0,1] per spec.
            let s = values_attr
                .and_then(|v| v.split_whitespace().next())
                .and_then(|p| p.parse::<f32>().ok())
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            saturate_matrix(s)
        }
        Some("huerotate") => {
            // Per §13.2.5, theta defaults to 0 (identity) and is in
            // degrees.
            let degrees = values_attr
                .and_then(|v| v.split_whitespace().next())
                .and_then(|p| p.parse::<f32>().ok())
                .unwrap_or(0.0);
            hue_rotate_matrix(degrees.to_radians())
        }
        Some("luminancetoalpha") => luminance_to_alpha_matrix(),
        // `matrix` (default) — parse 20 floats. Missing / malformed
        // values fall back to identity per spec §13.2.3.
        _ => {
            let floats = parse_number_list(values_attr);
            if floats.len() == 20 {
                let mut m = [0.0f32; 20];
                m.copy_from_slice(&floats);
                m
            } else {
                identity_matrix()
            }
        }
    };
    FilterPrimitive::ColorMatrix {
        input: input_or_default(el, prev),
        matrix,
    }
}

fn parse_merge(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    let mut inputs = Vec::new();
    for child in &el.children {
        let XmlNode::Element(c) = child else { continue };
        if !tag_local(&c.name).eq_ignore_ascii_case("feMergeNode") {
            continue;
        }
        let in_attr = attr(c, "in").map(FilterInput::from_str);
        // Per Filter Effects §19, feMergeNode without `in=` defaults
        // to the "previous result" rule of §6.2 — same as any other
        // primitive's first input slot.
        let resolved = in_attr.unwrap_or_else(|| match prev {
            Some(r) => FilterInput::Reference(r.clone()),
            None => FilterInput::SourceGraphic,
        });
        inputs.push(resolved);
    }
    FilterPrimitive::Merge { inputs }
}

fn parse_component_transfer(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    let mut red = TransferFunction::Identity;
    let mut green = TransferFunction::Identity;
    let mut blue = TransferFunction::Identity;
    let mut alpha = TransferFunction::Identity;
    for child in &el.children {
        let XmlNode::Element(c) = child else { continue };
        let local = tag_local(&c.name).to_ascii_lowercase();
        let f = match local.as_str() {
            "fefuncr" | "fefuncg" | "fefuncb" | "fefunca" => parse_transfer_function(c),
            _ => continue,
        };
        match local.as_str() {
            "fefuncr" => red = f,
            "fefuncg" => green = f,
            "fefuncb" => blue = f,
            "fefunca" => alpha = f,
            _ => {}
        }
    }
    FilterPrimitive::ComponentTransfer {
        input: input_or_default(el, prev),
        red,
        green,
        blue,
        alpha,
    }
}

fn parse_transfer_function(el: &Element) -> TransferFunction {
    let kind = attr(el, "type")
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "identity".to_string());
    match kind.as_str() {
        "table" => TransferFunction::Table {
            values: parse_number_list(attr(el, "tableValues")),
        },
        "discrete" => TransferFunction::Discrete {
            values: parse_number_list(attr(el, "tableValues")),
        },
        "linear" => TransferFunction::Linear {
            slope: parse_number(attr(el, "slope"), 1.0).unwrap_or(1.0),
            intercept: parse_number(attr(el, "intercept"), 0.0).unwrap_or(0.0),
        },
        "gamma" => TransferFunction::Gamma {
            amplitude: parse_number(attr(el, "amplitude"), 1.0).unwrap_or(1.0),
            exponent: parse_number(attr(el, "exponent"), 1.0).unwrap_or(1.0),
            offset: parse_number(attr(el, "offset"), 0.0).unwrap_or(0.0),
        },
        // `identity` (default) and any unknown type — pass through.
        _ => TransferFunction::Identity,
    }
}

fn parse_drop_shadow(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    // Per Filter Effects §22, `stdDeviation` defaults to "2 2", `dx`
    // and `dy` default to 2.
    let (sx, sy) = parse_two_numbers(attr(el, "stdDeviation"));
    let (sx, sy_resolved) = if attr(el, "stdDeviation").is_some() {
        (sx, sy.unwrap_or(sx))
    } else {
        (2.0, 2.0)
    };
    let dx = parse_number(attr(el, "dx"), 2.0).unwrap_or(2.0);
    let dy = parse_number(attr(el, "dy"), 2.0).unwrap_or(2.0);
    let flood_color = attr(el, "flood-color")
        .map(parse_flood_color)
        .unwrap_or_default();
    let flood_opacity = parse_number(attr(el, "flood-opacity"), 1.0)
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    FilterPrimitive::DropShadow {
        input: input_or_default(el, prev),
        dx,
        dy,
        std_deviation_x: sx,
        std_deviation_y: sy_resolved,
        flood_color,
        flood_opacity,
    }
}

/// Parse `<feConvolveMatrix>` per Filter Effects §15. The kernel
/// matrix is `order_x * order_y` row-major numbers; `divisor` defaults
/// to the sum of the kernel (or 1 if that sum is zero); `bias` defaults
/// to 0; `targetX` / `targetY` default to `floor(order/2)`; `edgeMode`
/// defaults to `duplicate`; `preserveAlpha` defaults to `false`.
fn parse_convolve_matrix(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    let (order_x_f, order_y_f) = parse_two_numbers(attr(el, "order"));
    // Per spec §15.2 default order is 3 (per-axis). We treat absent /
    // non-positive as 3.
    let order_x = if order_x_f >= 1.0 {
        order_x_f as u32
    } else {
        3
    };
    let order_y = match order_y_f {
        Some(v) if v >= 1.0 => v as u32,
        Some(_) => 3,
        None => order_x,
    };
    let kernel_matrix = parse_number_list(attr(el, "kernelMatrix"));
    // Spec §15.2 — `divisor` default is the sum of the matrix, or 1 if
    // the sum is zero. Bias default is 0.
    let kernel_sum: f32 = kernel_matrix.iter().sum();
    let divisor_default = if kernel_sum == 0.0 { 1.0 } else { kernel_sum };
    let divisor = parse_number(attr(el, "divisor"), divisor_default).unwrap_or(divisor_default);
    let bias = parse_number(attr(el, "bias"), 0.0).unwrap_or(0.0);
    // Per spec §15.2 — targetX / targetY default to `floor(orderX/2)`
    // / `floor(orderY/2)`.
    let target_x = parse_number(attr(el, "targetX"), (order_x / 2) as f32)
        .map(|v| v as i32)
        .unwrap_or((order_x / 2) as i32);
    let target_y = parse_number(attr(el, "targetY"), (order_y / 2) as f32)
        .map(|v| v as i32)
        .unwrap_or((order_y / 2) as i32);
    let edge_mode = attr(el, "edgeMode")
        .map(ConvolveEdgeMode::from_str)
        .unwrap_or_default();
    let preserve_alpha = attr(el, "preserveAlpha")
        .map(|s| matches!(s.trim(), "true"))
        .unwrap_or(false);
    FilterPrimitive::ConvolveMatrix {
        input: input_or_default(el, prev),
        order_x,
        order_y,
        kernel_matrix,
        divisor,
        bias,
        target_x,
        target_y,
        edge_mode,
        preserve_alpha,
    }
}

/// Parse `<feTurbulence>` per Filter Effects §16. `baseFrequency`
/// default is 0 per spec; `numOctaves` defaults to 1; `seed` defaults
/// to 0; `stitchTiles="stitch"` flips a bool; `type` defaults to
/// `turbulence`.
fn parse_turbulence(el: &Element) -> FilterPrimitive {
    let (fx, fy) = parse_two_numbers(attr(el, "baseFrequency"));
    let base_frequency_x = fx;
    let base_frequency_y = fy.unwrap_or(fx);
    let num_octaves = parse_number(attr(el, "numOctaves"), 1.0)
        .unwrap_or(1.0)
        .max(1.0) as u32;
    let seed = parse_number(attr(el, "seed"), 0.0).unwrap_or(0.0) as i32;
    // `stitchTiles="stitch"` enables; `noStitch` (default) disables.
    let stitch_tiles = matches!(attr(el, "stitchTiles").map(str::trim), Some("stitch"));
    let kind = attr(el, "type")
        .map(TurbulenceKind::from_str)
        .unwrap_or_default();
    FilterPrimitive::Turbulence {
        base_frequency_x,
        base_frequency_y,
        num_octaves,
        seed,
        stitch_tiles,
        kind,
    }
}

/// Parse `<feDisplacementMap>` per Filter Effects §17. `scale`
/// defaults to 0; `xChannelSelector` / `yChannelSelector` both default
/// to `A` per spec.
fn parse_displacement_map(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    FilterPrimitive::DisplacementMap {
        input: input_or_default(el, prev),
        input2: attr(el, "in2")
            .map(FilterInput::from_str)
            .unwrap_or(FilterInput::SourceGraphic),
        scale: parse_number(attr(el, "scale"), 0.0).unwrap_or(0.0),
        x_channel_selector: attr(el, "xChannelSelector")
            .map(ChannelSelector::from_str)
            .unwrap_or_default(),
        y_channel_selector: attr(el, "yChannelSelector")
            .map(ChannelSelector::from_str)
            .unwrap_or_default(),
    }
}

/// 4×5 identity matrix per Filter Effects §13.2.3.
fn identity_matrix() -> [f32; 20] {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, // R
        0.0, 1.0, 0.0, 0.0, 0.0, // G
        0.0, 0.0, 1.0, 0.0, 0.0, // B
        0.0, 0.0, 0.0, 1.0, 0.0, // A
    ]
}

/// `type="saturate"` template per Filter Effects §13.2.4. With s=1
/// this is the identity; with s=0 it desaturates to luminance.
/// Coefficients (0.213, 0.715, 0.072) match the spec verbatim.
fn saturate_matrix(s: f32) -> [f32; 20] {
    let r0 = 0.213 + 0.787 * s;
    let r1 = 0.715 - 0.715 * s;
    let r2 = 0.072 - 0.072 * s;
    let g0 = 0.213 - 0.213 * s;
    let g1 = 0.715 + 0.285 * s;
    let g2 = 0.072 - 0.072 * s;
    let b0 = 0.213 - 0.213 * s;
    let b1 = 0.715 - 0.715 * s;
    let b2 = 0.072 + 0.928 * s;
    [
        r0, r1, r2, 0.0, 0.0, // R'
        g0, g1, g2, 0.0, 0.0, // G'
        b0, b1, b2, 0.0, 0.0, // B'
        0.0, 0.0, 0.0, 1.0, 0.0, // A'
    ]
}

/// `type="hueRotate"` template per Filter Effects §13.2.5. `theta`
/// is in radians (the spec gives the formula in radians once the
/// `values` attribute is interpreted as degrees).
fn hue_rotate_matrix(theta: f32) -> [f32; 20] {
    let c = theta.cos();
    let s = theta.sin();
    // The 3x3 matrix below is the spec's equation (13.2.5) with the
    // luminance / chroma decomposition baked in.
    let r0 = 0.213 + c * 0.787 - s * 0.213;
    let r1 = 0.715 - c * 0.715 - s * 0.715;
    let r2 = 0.072 - c * 0.072 + s * 0.928;
    let g0 = 0.213 - c * 0.213 + s * 0.143;
    let g1 = 0.715 + c * 0.285 + s * 0.140;
    let g2 = 0.072 - c * 0.072 - s * 0.283;
    let b0 = 0.213 - c * 0.213 - s * 0.787;
    let b1 = 0.715 - c * 0.715 + s * 0.715;
    let b2 = 0.072 + c * 0.928 + s * 0.072;
    [
        r0, r1, r2, 0.0, 0.0, // R'
        g0, g1, g2, 0.0, 0.0, // G'
        b0, b1, b2, 0.0, 0.0, // B'
        0.0, 0.0, 0.0, 1.0, 0.0, // A'
    ]
}

/// `type="luminanceToAlpha"` template per Filter Effects §13.2.6.
fn luminance_to_alpha_matrix() -> [f32; 20] {
    [
        0.0, 0.0, 0.0, 0.0, 0.0, // R'
        0.0, 0.0, 0.0, 0.0, 0.0, // G'
        0.0, 0.0, 0.0, 0.0, 0.0, // B'
        0.2125, 0.7154, 0.0721, 0.0, 0.0, // A'
    ]
}

/// Parse a whitespace- or comma-separated list of f32s. Missing /
/// malformed entries are skipped. Returns an empty vec when the
/// attribute is absent.
fn parse_number_list(s: Option<&str>) -> Vec<f32> {
    let Some(raw) = s else { return Vec::new() };
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse::<f32>().ok())
        .collect()
}

/// Parse `"sx"` or `"sx sy"` (whitespace- or comma-separated) into a
/// pair. Missing / malformed input gives `(0.0, None)`.
fn parse_two_numbers(s: Option<&str>) -> (f32, Option<f32>) {
    let Some(raw) = s else { return (0.0, None) };
    let parts: Vec<&str> = raw
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    let a = parts
        .first()
        .and_then(|p| p.parse::<f32>().ok())
        .unwrap_or(0.0);
    let b = parts.get(1).and_then(|p| p.parse::<f32>().ok());
    (a, b)
}

/// Parse a CSS-named or `#rrggbb[aa]` flood colour. Anything unknown
/// (including `currentColor`) resolves to opaque black.
fn parse_flood_color(s: &str) -> FloodColor {
    use crate::color::{parse_paint, PaintValue};
    match parse_paint(s) {
        Ok(PaintValue::Color(rgba)) => FloodColor {
            r: rgba.r,
            g: rgba.g,
            b: rgba.b,
            a: rgba.a,
        },
        _ => FloodColor::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_xml;

    fn first_filter(src: &str) -> Element {
        let nodes = parse_xml(src).expect("parse_xml ok");
        // Walk to the first <filter>.
        fn walk(n: &XmlNode) -> Option<Element> {
            match n {
                XmlNode::Element(e) => {
                    if tag_local(&e.name).eq_ignore_ascii_case("filter") {
                        return Some(e.clone());
                    }
                    for c in &e.children {
                        if let Some(f) = walk(c) {
                            return Some(f);
                        }
                    }
                    None
                }
                _ => None,
            }
        }
        for n in &nodes {
            if let Some(f) = walk(n) {
                return f;
            }
        }
        panic!("no <filter> found")
    }

    #[test]
    fn parses_gaussian_blur_with_one_std_deviation() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feGaussianBlur stdDeviation="3"/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        assert_eq!(g.primitives.len(), 1);
        match &g.primitives[0].primitive {
            FilterPrimitive::GaussianBlur {
                std_deviation_x,
                std_deviation_y,
                input,
                ..
            } => {
                assert_eq!(*std_deviation_x, 3.0);
                assert_eq!(*std_deviation_y, 3.0);
                assert_eq!(*input, FilterInput::SourceGraphic);
            }
            other => panic!("expected GaussianBlur, got {:?}", other),
        }
    }

    #[test]
    fn parses_gaussian_blur_with_two_std_deviations() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feGaussianBlur stdDeviation="3 5"/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::GaussianBlur {
                std_deviation_x,
                std_deviation_y,
                ..
            } => {
                assert_eq!(*std_deviation_x, 3.0);
                assert_eq!(*std_deviation_y, 5.0);
            }
            _ => panic!("not blur"),
        }
    }

    #[test]
    fn parses_offset() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feOffset dx="4" dy="-2" in="SourceGraphic"/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::Offset { dx, dy, input } => {
                assert_eq!(*dx, 4.0);
                assert_eq!(*dy, -2.0);
                assert_eq!(*input, FilterInput::SourceGraphic);
            }
            _ => panic!("not offset"),
        }
    }

    #[test]
    fn parses_flood_with_color_and_opacity() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feFlood flood-color="#ff0000" flood-opacity="0.5"/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::Flood {
                flood_color,
                flood_opacity,
            } => {
                assert_eq!(flood_color.r, 0xff);
                assert_eq!(flood_color.g, 0);
                assert_eq!(flood_color.b, 0);
                assert!((*flood_opacity - 0.5).abs() < 1e-6);
            }
            _ => panic!("not flood"),
        }
    }

    #[test]
    fn parses_composite_arithmetic() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feFlood result="bg" flood-color="#000000"/>
                <feComposite in="SourceGraphic" in2="bg" operator="arithmetic" k1="1" k2="0.5" k3="0" k4="0"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        assert_eq!(g.primitives.len(), 2);
        match &g.primitives[1].primitive {
            FilterPrimitive::Composite {
                operator,
                input,
                input2,
                k1,
                k2,
                ..
            } => {
                assert_eq!(*operator, CompositeOperator::Arithmetic);
                assert_eq!(*input, FilterInput::SourceGraphic);
                assert_eq!(*input2, FilterInput::Reference("bg".into()));
                assert_eq!(*k1, 1.0);
                assert!((*k2 - 0.5).abs() < 1e-6);
            }
            _ => panic!("not composite"),
        }
    }

    #[test]
    fn parses_blend_mode() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feBlend in="SourceGraphic" in2="SourceAlpha" mode="multiply"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::Blend {
                mode,
                input,
                input2,
            } => {
                assert_eq!(*mode, BlendMode::Multiply);
                assert_eq!(*input, FilterInput::SourceGraphic);
                assert_eq!(*input2, FilterInput::SourceAlpha);
            }
            _ => panic!("not blend"),
        }
    }

    #[test]
    fn parses_morphology_dilate_two_radii() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feMorphology operator="dilate" radius="2 4"/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::Morphology {
                operator,
                radius_x,
                radius_y,
                ..
            } => {
                assert_eq!(*operator, MorphologyOperator::Dilate);
                assert_eq!(*radius_x, 2.0);
                assert_eq!(*radius_y, 4.0);
            }
            _ => panic!("not morphology"),
        }
    }

    #[test]
    fn implicit_input_chain_threads_previous_result() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feGaussianBlur stdDeviation="3" result="b"/>
                <feOffset dx="5" dy="5"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        assert_eq!(g.primitives.len(), 2);
        match &g.primitives[1].primitive {
            FilterPrimitive::Offset { input, .. } => {
                assert_eq!(*input, FilterInput::Reference("b".into()));
            }
            _ => panic!("not offset"),
        }
    }

    #[test]
    fn unknown_primitive_is_skipped() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feGaussianBlur stdDeviation="2"/>
                <feComposite operator="over"/>
                <feBogusPrimitive/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        assert_eq!(
            g.primitives.len(),
            2,
            "unknown <feBogusPrimitive> should be skipped"
        );
    }

    #[test]
    fn parses_color_matrix_explicit_4x5() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feColorMatrix type="matrix" values="
                  0 1 0 0 0
                  1 0 0 0 0
                  0 0 1 0 0
                  0 0 0 1 0"/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::ColorMatrix { matrix, .. } => {
                // R takes G, G takes R, B/A pass through.
                assert_eq!(matrix[0], 0.0);
                assert_eq!(matrix[1], 1.0);
                assert_eq!(matrix[5], 1.0);
                assert_eq!(matrix[6], 0.0);
                assert_eq!(matrix[12], 1.0);
                assert_eq!(matrix[18], 1.0);
            }
            _ => panic!("not color-matrix"),
        }
    }

    #[test]
    fn color_matrix_saturate_zero_is_luminance_grayscale() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feColorMatrix type="saturate" values="0"/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::ColorMatrix { matrix, .. } = &g.primitives[0].primitive else {
            panic!("not color-matrix");
        };
        // Per spec — every output channel weights to luminance
        // coefficients (0.213, 0.715, 0.072) when s=0.
        for row in 0..3 {
            assert!((matrix[row * 5] - 0.213).abs() < 1e-3);
            assert!((matrix[row * 5 + 1] - 0.715).abs() < 1e-3);
            assert!((matrix[row * 5 + 2] - 0.072).abs() < 1e-3);
        }
    }

    #[test]
    fn color_matrix_huerotate_zero_is_identity() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feColorMatrix type="hueRotate" values="0"/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::ColorMatrix { matrix, .. } = &g.primitives[0].primitive else {
            panic!("not color-matrix");
        };
        // hue-rotate by 0° must equal identity (within FP epsilon).
        let id = identity_matrix();
        for (i, (a, b)) in matrix.iter().zip(id.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-3,
                "row {} col {}: got {} want {}",
                i / 5,
                i % 5,
                a,
                b
            );
        }
    }

    #[test]
    fn color_matrix_luminance_to_alpha_writes_only_alpha_row() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feColorMatrix type="luminanceToAlpha"/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::ColorMatrix { matrix, .. } = &g.primitives[0].primitive else {
            panic!("not color-matrix");
        };
        // R, G, B rows are zero; A row weights luminance.
        for v in matrix.iter().take(15) {
            assert_eq!(*v, 0.0);
        }
        assert!((matrix[15] - 0.2125).abs() < 1e-4);
        assert!((matrix[16] - 0.7154).abs() < 1e-4);
        assert!((matrix[17] - 0.0721).abs() < 1e-4);
    }

    #[test]
    fn color_matrix_default_type_is_matrix() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feColorMatrix values="
                  1 0 0 0 0
                  0 1 0 0 0
                  0 0 1 0 0
                  0 0 0 1 0"/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::ColorMatrix { matrix, .. } = &g.primitives[0].primitive else {
            panic!("not color-matrix");
        };
        assert_eq!(matrix, &identity_matrix());
    }

    #[test]
    fn color_matrix_malformed_values_falls_back_to_identity() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feColorMatrix values="1 2 3"/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::ColorMatrix { matrix, .. } = &g.primitives[0].primitive else {
            panic!("not color-matrix");
        };
        assert_eq!(matrix, &identity_matrix());
    }

    #[test]
    fn parses_merge_in_order() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feFlood result="bg" flood-color="#000000"/>
                <feGaussianBlur in="SourceAlpha" stdDeviation="2" result="blur"/>
                <feMerge>
                  <feMergeNode in="bg"/>
                  <feMergeNode in="blur"/>
                  <feMergeNode in="SourceGraphic"/>
                </feMerge>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        // bg, blur, merge (3 primitives)
        assert_eq!(g.primitives.len(), 3);
        let FilterPrimitive::Merge { inputs } = &g.primitives[2].primitive else {
            panic!("not merge");
        };
        assert_eq!(inputs.len(), 3);
        assert_eq!(inputs[0], FilterInput::Reference("bg".into()));
        assert_eq!(inputs[1], FilterInput::Reference("blur".into()));
        assert_eq!(inputs[2], FilterInput::SourceGraphic);
    }

    #[test]
    fn merge_node_without_in_falls_back_to_previous_result() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feGaussianBlur stdDeviation="2" result="blurred"/>
                <feMerge>
                  <feMergeNode/>
                  <feMergeNode in="SourceGraphic"/>
                </feMerge>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::Merge { inputs } = &g.primitives[1].primitive else {
            panic!("not merge");
        };
        assert_eq!(inputs[0], FilterInput::Reference("blurred".into()));
        assert_eq!(inputs[1], FilterInput::SourceGraphic);
    }

    #[test]
    fn parses_component_transfer_table() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feComponentTransfer>
                  <feFuncR type="table" tableValues="0 0.5 1"/>
                  <feFuncG type="discrete" tableValues="0.25 0.5 0.75"/>
                  <feFuncB type="linear" slope="2" intercept="-0.5"/>
                  <feFuncA type="gamma" amplitude="1" exponent="2.2" offset="0"/>
                </feComponentTransfer>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::ComponentTransfer {
            red,
            green,
            blue,
            alpha,
            ..
        } = &g.primitives[0].primitive
        else {
            panic!("not component-transfer");
        };
        match red {
            TransferFunction::Table { values } => assert_eq!(values, &vec![0.0, 0.5, 1.0]),
            _ => panic!("red not table"),
        }
        match green {
            TransferFunction::Discrete { values } => {
                assert_eq!(values, &vec![0.25, 0.5, 0.75])
            }
            _ => panic!("green not discrete"),
        }
        match blue {
            TransferFunction::Linear { slope, intercept } => {
                assert_eq!(*slope, 2.0);
                assert_eq!(*intercept, -0.5);
            }
            _ => panic!("blue not linear"),
        }
        match alpha {
            TransferFunction::Gamma {
                amplitude,
                exponent,
                offset,
            } => {
                assert_eq!(*amplitude, 1.0);
                assert!((*exponent - 2.2).abs() < 1e-4);
                assert_eq!(*offset, 0.0);
            }
            _ => panic!("alpha not gamma"),
        }
    }

    #[test]
    fn component_transfer_missing_channels_default_to_identity() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feComponentTransfer>
                  <feFuncR type="linear" slope="2" intercept="0"/>
                </feComponentTransfer>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::ComponentTransfer {
            red,
            green,
            blue,
            alpha,
            ..
        } = &g.primitives[0].primitive
        else {
            panic!("not component-transfer");
        };
        assert!(matches!(red, TransferFunction::Linear { .. }));
        assert_eq!(*green, TransferFunction::Identity);
        assert_eq!(*blue, TransferFunction::Identity);
        assert_eq!(*alpha, TransferFunction::Identity);
    }

    #[test]
    fn component_transfer_unknown_type_is_identity() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feComponentTransfer>
                  <feFuncR type="bogus" tableValues="1"/>
                </feComponentTransfer>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::ComponentTransfer { red, .. } = &g.primitives[0].primitive else {
            panic!("not component-transfer");
        };
        assert_eq!(*red, TransferFunction::Identity);
    }

    #[test]
    fn parses_drop_shadow() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feDropShadow dx="3" dy="4" stdDeviation="2" flood-color="#ff0000" flood-opacity="0.5"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::DropShadow {
            dx,
            dy,
            std_deviation_x,
            std_deviation_y,
            flood_color,
            flood_opacity,
            ..
        } = &g.primitives[0].primitive
        else {
            panic!("not drop-shadow");
        };
        assert_eq!(*dx, 3.0);
        assert_eq!(*dy, 4.0);
        assert_eq!(*std_deviation_x, 2.0);
        assert_eq!(*std_deviation_y, 2.0);
        assert_eq!(flood_color.r, 0xff);
        assert!((*flood_opacity - 0.5).abs() < 1e-6);
    }

    #[test]
    fn drop_shadow_defaults_match_filter_effects_22() {
        // No attrs → dx=dy=2, stdDeviation=2 2, flood-color black,
        // flood-opacity 1 (per W3C Filter Effects §22 default values).
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feDropShadow/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::DropShadow {
            dx,
            dy,
            std_deviation_x,
            std_deviation_y,
            flood_color,
            flood_opacity,
            ..
        } = &g.primitives[0].primitive
        else {
            panic!("not drop-shadow");
        };
        assert_eq!(*dx, 2.0);
        assert_eq!(*dy, 2.0);
        assert_eq!(*std_deviation_x, 2.0);
        assert_eq!(*std_deviation_y, 2.0);
        assert_eq!(flood_color, &FloodColor::default());
        assert_eq!(*flood_opacity, 1.0);
    }

    #[test]
    fn drop_shadow_two_axis_std_deviation() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feDropShadow dx="1" dy="2" stdDeviation="3 5"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::DropShadow {
            std_deviation_x,
            std_deviation_y,
            ..
        } = &g.primitives[0].primitive
        else {
            panic!("not drop-shadow");
        };
        assert_eq!(*std_deviation_x, 3.0);
        assert_eq!(*std_deviation_y, 5.0);
    }

    #[test]
    fn parses_convolve_matrix_3x3() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feConvolveMatrix order="3" kernelMatrix="0 -1 0  -1 5 -1  0 -1 0" divisor="1" bias="0"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::ConvolveMatrix {
                order_x,
                order_y,
                kernel_matrix,
                divisor,
                bias,
                target_x,
                target_y,
                edge_mode,
                preserve_alpha,
                ..
            } => {
                assert_eq!(*order_x, 3);
                assert_eq!(*order_y, 3);
                assert_eq!(kernel_matrix.len(), 9);
                assert_eq!(kernel_matrix[4], 5.0);
                assert_eq!(*divisor, 1.0);
                assert_eq!(*bias, 0.0);
                // Default targetX / targetY = floor(3/2) = 1.
                assert_eq!(*target_x, 1);
                assert_eq!(*target_y, 1);
                assert_eq!(*edge_mode, ConvolveEdgeMode::Duplicate);
                assert!(!*preserve_alpha);
            }
            _ => panic!("not convolve-matrix"),
        }
    }

    #[test]
    fn convolve_matrix_default_divisor_is_kernel_sum() {
        // kernel sums to 9, divisor absent -> default to sum.
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feConvolveMatrix order="3" kernelMatrix="1 1 1  1 1 1  1 1 1"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::ConvolveMatrix { divisor, .. } => assert_eq!(*divisor, 9.0),
            _ => panic!("not convolve-matrix"),
        }
    }

    #[test]
    fn convolve_matrix_zero_sum_kernel_falls_back_to_one() {
        // kernel sums to 0, divisor absent -> default to 1 per §15.2.
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feConvolveMatrix order="3" kernelMatrix="-1 -1 -1  -1 8 -1  -1 -1 -1"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::ConvolveMatrix { divisor, .. } => assert_eq!(*divisor, 1.0),
            _ => panic!("not convolve-matrix"),
        }
    }

    #[test]
    fn convolve_matrix_edge_mode_and_preserve_alpha() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feConvolveMatrix order="3" kernelMatrix="0 0 0  0 1 0  0 0 0" edgeMode="wrap" preserveAlpha="true"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::ConvolveMatrix {
                edge_mode,
                preserve_alpha,
                ..
            } => {
                assert_eq!(*edge_mode, ConvolveEdgeMode::Wrap);
                assert!(*preserve_alpha);
            }
            _ => panic!("not convolve-matrix"),
        }
    }

    #[test]
    fn convolve_matrix_non_square_order_5x3() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feConvolveMatrix order="5 3" kernelMatrix="0 0 0 0 0  0 0 1 0 0  0 0 0 0 0"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::ConvolveMatrix {
                order_x,
                order_y,
                kernel_matrix,
                target_x,
                target_y,
                ..
            } => {
                assert_eq!(*order_x, 5);
                assert_eq!(*order_y, 3);
                assert_eq!(kernel_matrix.len(), 15);
                // Default target = floor(order/2).
                assert_eq!(*target_x, 2);
                assert_eq!(*target_y, 1);
            }
            _ => panic!("not convolve-matrix"),
        }
    }

    #[test]
    fn parses_turbulence_default_type_is_turbulence() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feTurbulence baseFrequency="0.05" numOctaves="2" seed="3"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::Turbulence {
                base_frequency_x,
                base_frequency_y,
                num_octaves,
                seed,
                stitch_tiles,
                kind,
            } => {
                assert!((*base_frequency_x - 0.05).abs() < 1e-6);
                assert!((*base_frequency_y - 0.05).abs() < 1e-6);
                assert_eq!(*num_octaves, 2);
                assert_eq!(*seed, 3);
                assert!(!*stitch_tiles);
                assert_eq!(*kind, TurbulenceKind::Turbulence);
            }
            _ => panic!("not turbulence"),
        }
    }

    #[test]
    fn turbulence_two_axis_base_frequency() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feTurbulence baseFrequency="0.05 0.1"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::Turbulence {
                base_frequency_x,
                base_frequency_y,
                ..
            } => {
                assert!((*base_frequency_x - 0.05).abs() < 1e-6);
                assert!((*base_frequency_y - 0.1).abs() < 1e-6);
            }
            _ => panic!("not turbulence"),
        }
    }

    #[test]
    fn turbulence_fractal_noise_with_stitch() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feTurbulence type="fractalNoise" baseFrequency="0.1" stitchTiles="stitch"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::Turbulence {
                kind, stitch_tiles, ..
            } => {
                assert_eq!(*kind, TurbulenceKind::FractalNoise);
                assert!(*stitch_tiles);
            }
            _ => panic!("not turbulence"),
        }
    }

    #[test]
    fn turbulence_unknown_type_defaults_to_turbulence() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feTurbulence type="bogusNoise" baseFrequency="0.1"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::Turbulence { kind, .. } => {
                assert_eq!(*kind, TurbulenceKind::Turbulence);
            }
            _ => panic!("not turbulence"),
        }
    }

    #[test]
    fn parses_displacement_map_explicit_channels() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feTurbulence baseFrequency="0.05" result="noise"/>
                <feDisplacementMap in="SourceGraphic" in2="noise" scale="20" xChannelSelector="R" yChannelSelector="G"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        assert_eq!(g.primitives.len(), 2);
        match &g.primitives[1].primitive {
            FilterPrimitive::DisplacementMap {
                input,
                input2,
                scale,
                x_channel_selector,
                y_channel_selector,
            } => {
                assert_eq!(*input, FilterInput::SourceGraphic);
                assert_eq!(*input2, FilterInput::Reference("noise".into()));
                assert_eq!(*scale, 20.0);
                assert_eq!(*x_channel_selector, ChannelSelector::R);
                assert_eq!(*y_channel_selector, ChannelSelector::G);
            }
            _ => panic!("not displacement-map"),
        }
    }

    #[test]
    fn displacement_map_default_channel_selectors_are_alpha() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feDisplacementMap scale="5"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::DisplacementMap {
                x_channel_selector,
                y_channel_selector,
                scale,
                ..
            } => {
                assert_eq!(*x_channel_selector, ChannelSelector::A);
                assert_eq!(*y_channel_selector, ChannelSelector::A);
                assert_eq!(*scale, 5.0);
            }
            _ => panic!("not displacement-map"),
        }
    }

    #[test]
    fn displacement_map_unknown_channel_falls_back_to_alpha() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feDisplacementMap scale="5" xChannelSelector="Q" yChannelSelector="Z"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::DisplacementMap {
                x_channel_selector,
                y_channel_selector,
                ..
            } => {
                assert_eq!(*x_channel_selector, ChannelSelector::A);
                assert_eq!(*y_channel_selector, ChannelSelector::A);
            }
            _ => panic!("not displacement-map"),
        }
    }

    #[test]
    fn filter_region_attributes_are_captured() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f" x="-5" y="-10" width="120" height="80">
                <feGaussianBlur stdDeviation="1"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        assert_eq!(g.region.x, Some(-5.0));
        assert_eq!(g.region.y, Some(-10.0));
        assert_eq!(g.region.width, Some(120.0));
        assert_eq!(g.region.height, Some(80.0));
    }
}
