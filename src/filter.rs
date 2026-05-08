//! Typed parsing of `<filter>` primitive graphs.
//!
//! Round 2-4 captured `<filter>` element trees verbatim and round-tripped
//! them through the encoder, but never inspected the primitives inside.
//! Round 7 added typed parsing for the six most common primitives
//! (`<feGaussianBlur>`, `<feOffset>`, `<feFlood>`, `<feComposite>`,
//! `<feBlend>`, `<feMorphology>`); round 8 extended that to
//! `<feColorMatrix>`, `<feMerge>` (with `<feMergeNode>` children),
//! `<feComponentTransfer>` (with `<feFuncR/G/B/A>` children) and
//! `<feDropShadow>` (a composite primitive the W3C Filter Effects spec
//! defines as syntactic sugar over GaussianBlur + Offset + Flood +
//! Composite); round 9 extended it again to `<feConvolveMatrix>`,
//! `<feTurbulence>` and `<feDisplacementMap>`; round 10 finishes the
//! long tail of common primitives by typing `<feDiffuseLighting>` and
//! `<feSpecularLighting>` (sharing a [`LightSource`] enum that
//! captures `<feDistantLight>` / `<fePointLight>` / `<feSpotLight>`
//! children); round 11 closes the W3C Filter Effects §11 short-name set
//! by typing `<feImage>` (per §21) and `<feTile>` (per §20). Allowlist
//! count: 17 of the W3C Filter Effects §11 set — every short-name
//! primitive is now typed.
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
    /// `<feDiffuseLighting>` — Lambertian-diffuse lighting model.
    /// Per W3C Filter Effects §18.
    ///
    /// `in` supplies the alpha-only height-map; the lit colour is
    /// `lighting_color * diffuse_constant * (N · L)` where `N` is the
    /// surface normal derived from the height-map and `L` is the
    /// per-pixel light vector.  The spec lists `kernelUnitLength` as
    /// the (dx, dy) finite-difference spacing used to compute `N`.
    DiffuseLighting {
        input: FilterInput,
        surface_scale: f32,
        diffuse_constant: f32,
        /// `kernelUnitLength = (dx, dy)`.  Per spec §18 default is one
        /// pixel along each axis (signalled by `None` so the rasterizer
        /// can substitute the user-coordinate-system value).
        kernel_unit_length: Option<(f32, f32)>,
        /// `lighting-color` resolved to RGBA.  CSS default is `white`
        /// per spec §21.
        lighting_color: FloodColor,
        /// One of `<feDistantLight>` / `<fePointLight>` / `<feSpotLight>`.
        /// Missing child collapses to a default distant light at
        /// (azimuth=0, elevation=0) per spec §18.5.
        light_source: LightSource,
    },
    /// `<feSpecularLighting>` — Phong-specular lighting model.
    /// Per W3C Filter Effects §19.
    ///
    /// `in` supplies the alpha-only height-map; the highlight is
    /// `lighting_color * specular_constant * (N · H)^specular_exponent`
    /// where `H` is the half-vector between viewer and light.
    SpecularLighting {
        input: FilterInput,
        surface_scale: f32,
        specular_constant: f32,
        /// Phong exponent.  Per spec §19 default is 1.0; the spec
        /// clamps the value to [1, 128] for typical hardware paths,
        /// but we record the unclamped float so the rasterizer can
        /// pick its own clamp policy.
        specular_exponent: f32,
        kernel_unit_length: Option<(f32, f32)>,
        lighting_color: FloodColor,
        light_source: LightSource,
    },
    /// `<feImage href preserveAspectRatio crossorigin>` — sources an
    /// external image (or document fragment) as a filter input. Per
    /// W3C Filter Effects §21.
    ///
    /// The `href` (or legacy `xlink:href`) string is recorded verbatim
    /// — the rasterizer is responsible for resolving it (`data:` URI,
    /// document-fragment `#id`, or external HTTP(S) URL); a `#id`
    /// fragment may target any element in the same document.
    Image {
        /// `href` value verbatim. Empty string when absent (per §21
        /// the primitive is then a no-op transparent black).
        href: String,
        /// `preserveAspectRatio` — defaults to `xMidYMid meet` per
        /// SVG 2 §8.10.
        preserve_aspect_ratio: PreserveAspectRatio,
        /// `crossorigin="anonymous|use-credentials"` (HTML CORS
        /// attribute spec). Absent → `None`.
        crossorigin: Option<CrossOrigin>,
    },
    /// `<feTile in>` — tiles `in`'s rectangle to fill the primitive
    /// sub-region. Per W3C Filter Effects §20.
    ///
    /// The only attribute is `in` (no parameters of its own); the
    /// shared `region` on [`FilterPrimitiveNode`] determines the area
    /// being filled.
    Tile { input: FilterInput },
}

/// One of the three SVG light-source elements
/// (`<feDistantLight>` / `<fePointLight>` / `<feSpotLight>`).
/// Per W3C Filter Effects §18.5 / §18.6 / §18.7 — shared by
/// `<feDiffuseLighting>` and `<feSpecularLighting>`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightSource {
    /// `<feDistantLight azimuth elevation>` — parallel rays from a
    /// direction at infinity. Both angles are in degrees.
    Distant { azimuth: f32, elevation: f32 },
    /// `<fePointLight x y z>` — omnidirectional point source.
    Point { x: f32, y: f32, z: f32 },
    /// `<feSpotLight x y z pointsAtX pointsAtY pointsAtZ
    /// specularExponent limitingConeAngle>`. `limiting_cone_angle` is
    /// in degrees; `None` means "no cone clipping" per §18.7.
    Spot {
        x: f32,
        y: f32,
        z: f32,
        points_at_x: f32,
        points_at_y: f32,
        points_at_z: f32,
        /// `specularExponent` on `<feSpotLight>` shapes the cone
        /// fall-off (independent from `<feSpecularLighting>`'s own
        /// `specularExponent`). Default 1 per spec §18.7.
        specular_exponent: f32,
        /// `limitingConeAngle` (degrees). Beyond this angle the
        /// contribution is zero. `None` (attribute absent) means the
        /// cone is unbounded per spec §18.7.
        limiting_cone_angle: Option<f32>,
    },
}

impl Default for LightSource {
    fn default() -> Self {
        // Per spec §18 the default when no light-source child is
        // supplied is implementation-defined; we pick a flat distant
        // light along +Z (azimuth=0, elevation=0) so a missing child
        // still produces a deterministic graph.
        Self::Distant {
            azimuth: 0.0,
            elevation: 0.0,
        }
    }
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

/// `preserveAspectRatio` on `<feImage>` (and elsewhere in SVG 2 §8.10).
///
/// The attribute combines an alignment keyword (`xMin/Mid/MaxYMin/Mid/Max`,
/// or `none`) with an optional `meet`/`slice` modifier defaulting to
/// `meet` per spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreserveAspectRatio {
    pub align: PreserveAspectRatioAlign,
    pub meet_or_slice: MeetOrSlice,
}

impl Default for PreserveAspectRatio {
    /// Per SVG 2 §8.10 default is `xMidYMid meet`.
    fn default() -> Self {
        Self {
            align: PreserveAspectRatioAlign::XMidYMid,
            meet_or_slice: MeetOrSlice::Meet,
        }
    }
}

impl PreserveAspectRatio {
    fn from_str(s: &str) -> Self {
        let mut parts = s.split_whitespace();
        let align = parts
            .next()
            .map(PreserveAspectRatioAlign::from_str)
            .unwrap_or_default();
        let meet_or_slice = parts.next().map(MeetOrSlice::from_str).unwrap_or_default();
        Self {
            align,
            meet_or_slice,
        }
    }
}

/// Alignment keyword for `preserveAspectRatio` per SVG 2 §8.10.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreserveAspectRatioAlign {
    /// `none` — stretch independently on each axis (no aspect-ratio
    /// preservation).
    None,
    XMinYMin,
    XMidYMin,
    XMaxYMin,
    XMinYMid,
    /// Default per spec — `xMidYMid`.
    #[default]
    XMidYMid,
    XMaxYMid,
    XMinYMax,
    XMidYMax,
    XMaxYMax,
}

impl PreserveAspectRatioAlign {
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "none" => Self::None,
            "xMinYMin" => Self::XMinYMin,
            "xMidYMin" => Self::XMidYMin,
            "xMaxYMin" => Self::XMaxYMin,
            "xMinYMid" => Self::XMinYMid,
            "xMidYMid" => Self::XMidYMid,
            "xMaxYMid" => Self::XMaxYMid,
            "xMinYMax" => Self::XMinYMax,
            "xMidYMax" => Self::XMidYMax,
            "xMaxYMax" => Self::XMaxYMax,
            _ => Self::XMidYMid,
        }
    }
}

/// Meet-or-slice modifier on `preserveAspectRatio` per SVG 2 §8.10.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MeetOrSlice {
    /// Default — fit inside the viewport, may letterbox.
    #[default]
    Meet,
    /// Fill the viewport, may overflow.
    Slice,
}

impl MeetOrSlice {
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "slice" => Self::Slice,
            _ => Self::Meet,
        }
    }
}

/// `crossorigin` on `<feImage>` (and other resource-loading elements)
/// per the HTML CORS attribute spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossOrigin {
    /// `crossorigin="anonymous"` — request omits credentials.
    Anonymous,
    /// `crossorigin="use-credentials"` — request includes credentials.
    UseCredentials,
}

impl CrossOrigin {
    fn from_str(s: &str) -> Option<Self> {
        match s.trim() {
            // Per HTML §2.7: empty value or `anonymous` both map to
            // anonymous.
            "" | "anonymous" => Some(Self::Anonymous),
            "use-credentials" => Some(Self::UseCredentials),
            _ => None,
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
            "fediffuselighting" => parse_diffuse_lighting(c, &prev_result),
            "fespecularlighting" => parse_specular_lighting(c, &prev_result),
            "feimage" => parse_image(c),
            "fetile" => parse_tile(c, &prev_result),
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

/// Parse `<feDiffuseLighting>` per Filter Effects §18. Defaults
/// (`surfaceScale=1`, `diffuseConstant=1`, `kernelUnitLength` absent,
/// `lighting-color="white"`) come from spec §18.4.
fn parse_diffuse_lighting(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    FilterPrimitive::DiffuseLighting {
        input: input_or_default(el, prev),
        surface_scale: parse_number(attr(el, "surfaceScale"), 1.0).unwrap_or(1.0),
        diffuse_constant: parse_number(attr(el, "diffuseConstant"), 1.0).unwrap_or(1.0),
        kernel_unit_length: parse_kernel_unit_length(attr(el, "kernelUnitLength")),
        lighting_color: parse_lighting_color(attr(el, "lighting-color")),
        light_source: parse_light_source(el),
    }
}

/// Parse `<feSpecularLighting>` per Filter Effects §19. Defaults
/// (`surfaceScale=1`, `specularConstant=1`, `specularExponent=1`,
/// `kernelUnitLength` absent, `lighting-color="white"`) come from
/// spec §19.4.
fn parse_specular_lighting(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    FilterPrimitive::SpecularLighting {
        input: input_or_default(el, prev),
        surface_scale: parse_number(attr(el, "surfaceScale"), 1.0).unwrap_or(1.0),
        specular_constant: parse_number(attr(el, "specularConstant"), 1.0).unwrap_or(1.0),
        specular_exponent: parse_number(attr(el, "specularExponent"), 1.0).unwrap_or(1.0),
        kernel_unit_length: parse_kernel_unit_length(attr(el, "kernelUnitLength")),
        lighting_color: parse_lighting_color(attr(el, "lighting-color")),
        light_source: parse_light_source(el),
    }
}

/// Parse `<feImage>` per Filter Effects §21. `href` (or legacy
/// `xlink:href`) is recorded verbatim. `preserveAspectRatio` defaults
/// to `xMidYMid meet` per SVG 2 §8.10. `crossorigin` is `None` when
/// the attribute is absent.
fn parse_image(el: &Element) -> FilterPrimitive {
    let href = attr(el, "href")
        .or_else(|| attr(el, "xlink:href"))
        .map(|s| s.to_string())
        .unwrap_or_default();
    let preserve_aspect_ratio = attr(el, "preserveAspectRatio")
        .map(PreserveAspectRatio::from_str)
        .unwrap_or_default();
    let crossorigin = attr(el, "crossorigin").and_then(CrossOrigin::from_str);
    FilterPrimitive::Image {
        href,
        preserve_aspect_ratio,
        crossorigin,
    }
}

/// Parse `<feTile>` per Filter Effects §20. The only attribute is
/// `in`; the primitive's region (already captured on the surrounding
/// [`FilterPrimitiveNode`]) determines the area being tiled.
fn parse_tile(el: &Element, prev: &Option<String>) -> FilterPrimitive {
    FilterPrimitive::Tile {
        input: input_or_default(el, prev),
    }
}

/// Walk children for the first recognised
/// `<feDistantLight>` / `<fePointLight>` / `<feSpotLight>` element.
/// Missing child returns the default distant-light per
/// [`LightSource::default`].
fn parse_light_source(el: &Element) -> LightSource {
    for child in &el.children {
        let XmlNode::Element(c) = child else { continue };
        let local = tag_local(&c.name).to_ascii_lowercase();
        match local.as_str() {
            "fedistantlight" => return parse_distant_light(c),
            "fepointlight" => return parse_point_light(c),
            "fespotlight" => return parse_spot_light(c),
            _ => continue,
        }
    }
    LightSource::default()
}

fn parse_distant_light(el: &Element) -> LightSource {
    LightSource::Distant {
        azimuth: parse_number(attr(el, "azimuth"), 0.0).unwrap_or(0.0),
        elevation: parse_number(attr(el, "elevation"), 0.0).unwrap_or(0.0),
    }
}

fn parse_point_light(el: &Element) -> LightSource {
    LightSource::Point {
        x: parse_number(attr(el, "x"), 0.0).unwrap_or(0.0),
        y: parse_number(attr(el, "y"), 0.0).unwrap_or(0.0),
        z: parse_number(attr(el, "z"), 0.0).unwrap_or(0.0),
    }
}

fn parse_spot_light(el: &Element) -> LightSource {
    // Per Filter Effects §18.7 — `pointsAt{X,Y,Z}` default to 0,
    // `specularExponent` defaults to 1, `limitingConeAngle` is absent
    // by default (no cone clipping).
    LightSource::Spot {
        x: parse_number(attr(el, "x"), 0.0).unwrap_or(0.0),
        y: parse_number(attr(el, "y"), 0.0).unwrap_or(0.0),
        z: parse_number(attr(el, "z"), 0.0).unwrap_or(0.0),
        points_at_x: parse_number(attr(el, "pointsAtX"), 0.0).unwrap_or(0.0),
        points_at_y: parse_number(attr(el, "pointsAtY"), 0.0).unwrap_or(0.0),
        points_at_z: parse_number(attr(el, "pointsAtZ"), 0.0).unwrap_or(0.0),
        specular_exponent: parse_number(attr(el, "specularExponent"), 1.0).unwrap_or(1.0),
        limiting_cone_angle: attr(el, "limitingConeAngle")
            .and_then(|s| parse_number(Some(s), 0.0).ok()),
    }
}

/// Parse `kernelUnitLength="dx [dy]"`. Missing → `None`; one number
/// mirrors per Filter Effects §18.4 (`dy` defaults to `dx`).
fn parse_kernel_unit_length(s: Option<&str>) -> Option<(f32, f32)> {
    let raw = s?;
    let (a, b) = parse_two_numbers(Some(raw));
    // Per spec a non-positive value disables the override; we still
    // record what the source said and let the rasterizer apply the
    // §18.4 fallback.
    Some((a, b.unwrap_or(a)))
}

/// Parse the `lighting-color` CSS property. Default per Filter Effects
/// §21 is opaque white (`#ffffff`).
fn parse_lighting_color(s: Option<&str>) -> FloodColor {
    let Some(raw) = s else {
        return FloodColor {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        };
    };
    use crate::color::{parse_paint, PaintValue};
    match parse_paint(raw) {
        Ok(PaintValue::Color(rgba)) => FloodColor {
            r: rgba.r,
            g: rgba.g,
            b: rgba.b,
            a: rgba.a,
        },
        // `currentColor` / unknown — fall through to opaque white per
        // spec default (the rasterizer would resolve `currentColor`
        // against the inherited fill, but we don't have inheritance
        // context here).
        _ => FloodColor {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        },
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
    fn parses_diffuse_lighting_with_distant_light() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feDiffuseLighting in="SourceAlpha" surfaceScale="2" diffuseConstant="0.8" lighting-color="#ff0000">
                  <feDistantLight azimuth="45" elevation="60"/>
                </feDiffuseLighting>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::DiffuseLighting {
                input,
                surface_scale,
                diffuse_constant,
                kernel_unit_length,
                lighting_color,
                light_source,
            } => {
                assert_eq!(*input, FilterInput::SourceAlpha);
                assert_eq!(*surface_scale, 2.0);
                assert!((*diffuse_constant - 0.8).abs() < 1e-6);
                assert_eq!(*kernel_unit_length, None);
                assert_eq!(lighting_color.r, 0xff);
                assert_eq!(lighting_color.g, 0);
                assert_eq!(lighting_color.b, 0);
                match light_source {
                    LightSource::Distant {
                        azimuth, elevation, ..
                    } => {
                        assert_eq!(*azimuth, 45.0);
                        assert_eq!(*elevation, 60.0);
                    }
                    other => panic!("expected Distant, got {:?}", other),
                }
            }
            other => panic!("expected DiffuseLighting, got {:?}", other),
        }
    }

    #[test]
    fn diffuse_lighting_defaults_match_filter_effects_18() {
        // No attrs / no light child. surfaceScale=1, diffuseConstant=1,
        // kernelUnitLength=None, lighting-color=white, default light.
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feDiffuseLighting/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::DiffuseLighting {
                surface_scale,
                diffuse_constant,
                kernel_unit_length,
                lighting_color,
                light_source,
                ..
            } => {
                assert_eq!(*surface_scale, 1.0);
                assert_eq!(*diffuse_constant, 1.0);
                assert_eq!(*kernel_unit_length, None);
                assert_eq!(
                    *lighting_color,
                    FloodColor {
                        r: 255,
                        g: 255,
                        b: 255,
                        a: 255
                    }
                );
                assert_eq!(*light_source, LightSource::default());
            }
            _ => panic!("not diffuse-lighting"),
        }
    }

    #[test]
    fn parses_specular_lighting_with_point_light() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feSpecularLighting in="SourceAlpha" surfaceScale="3" specularConstant="0.6" specularExponent="20" kernelUnitLength="2 4">
                  <fePointLight x="10" y="20" z="30"/>
                </feSpecularLighting>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::SpecularLighting {
                input,
                surface_scale,
                specular_constant,
                specular_exponent,
                kernel_unit_length,
                light_source,
                ..
            } => {
                assert_eq!(*input, FilterInput::SourceAlpha);
                assert_eq!(*surface_scale, 3.0);
                assert!((*specular_constant - 0.6).abs() < 1e-6);
                assert_eq!(*specular_exponent, 20.0);
                assert_eq!(*kernel_unit_length, Some((2.0, 4.0)));
                match light_source {
                    LightSource::Point { x, y, z } => {
                        assert_eq!(*x, 10.0);
                        assert_eq!(*y, 20.0);
                        assert_eq!(*z, 30.0);
                    }
                    other => panic!("expected Point, got {:?}", other),
                }
            }
            other => panic!("expected SpecularLighting, got {:?}", other),
        }
    }

    #[test]
    fn specular_lighting_defaults_match_filter_effects_19() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f"><feSpecularLighting/></filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        match &g.primitives[0].primitive {
            FilterPrimitive::SpecularLighting {
                surface_scale,
                specular_constant,
                specular_exponent,
                kernel_unit_length,
                lighting_color,
                ..
            } => {
                assert_eq!(*surface_scale, 1.0);
                assert_eq!(*specular_constant, 1.0);
                assert_eq!(*specular_exponent, 1.0);
                assert_eq!(*kernel_unit_length, None);
                assert_eq!(
                    *lighting_color,
                    FloodColor {
                        r: 255,
                        g: 255,
                        b: 255,
                        a: 255
                    }
                );
            }
            _ => panic!("not specular-lighting"),
        }
    }

    #[test]
    fn parses_spot_light_with_full_cone() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feDiffuseLighting>
                  <feSpotLight x="0" y="0" z="100" pointsAtX="50" pointsAtY="50" pointsAtZ="0" specularExponent="2" limitingConeAngle="30"/>
                </feDiffuseLighting>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::DiffuseLighting { light_source, .. } = &g.primitives[0].primitive
        else {
            panic!("not diffuse-lighting");
        };
        match light_source {
            LightSource::Spot {
                x,
                y,
                z,
                points_at_x,
                points_at_y,
                points_at_z,
                specular_exponent,
                limiting_cone_angle,
            } => {
                assert_eq!(*x, 0.0);
                assert_eq!(*y, 0.0);
                assert_eq!(*z, 100.0);
                assert_eq!(*points_at_x, 50.0);
                assert_eq!(*points_at_y, 50.0);
                assert_eq!(*points_at_z, 0.0);
                assert_eq!(*specular_exponent, 2.0);
                assert_eq!(*limiting_cone_angle, Some(30.0));
            }
            other => panic!("expected Spot, got {:?}", other),
        }
    }

    #[test]
    fn spot_light_without_cone_angle_records_none() {
        // limitingConeAngle absent — Option=None per spec §18.7.
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feSpecularLighting>
                  <feSpotLight x="0" y="0" z="50" pointsAtX="0" pointsAtY="0" pointsAtZ="0"/>
                </feSpecularLighting>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::SpecularLighting { light_source, .. } = &g.primitives[0].primitive
        else {
            panic!("not specular-lighting");
        };
        match light_source {
            LightSource::Spot {
                limiting_cone_angle,
                specular_exponent,
                ..
            } => {
                assert_eq!(*limiting_cone_angle, None);
                // Default specularExponent on the spot light is 1.
                assert_eq!(*specular_exponent, 1.0);
            }
            other => panic!("expected Spot, got {:?}", other),
        }
    }

    #[test]
    fn lighting_unknown_or_missing_child_falls_back_to_default() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feDiffuseLighting>
                  <feBogusLight foo="bar"/>
                </feDiffuseLighting>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::DiffuseLighting { light_source, .. } = &g.primitives[0].primitive
        else {
            panic!("not diffuse-lighting");
        };
        assert_eq!(*light_source, LightSource::default());
    }

    #[test]
    fn lighting_kernel_unit_length_one_number_mirrors() {
        // kernelUnitLength="3" → (3, 3) per spec §18.4.
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feDiffuseLighting kernelUnitLength="3">
                  <feDistantLight/>
                </feDiffuseLighting>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::DiffuseLighting {
            kernel_unit_length, ..
        } = &g.primitives[0].primitive
        else {
            panic!("not diffuse-lighting");
        };
        assert_eq!(*kernel_unit_length, Some((3.0, 3.0)));
    }

    #[test]
    fn lighting_threads_input_chain_from_previous_result() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feGaussianBlur stdDeviation="1" result="b"/>
                <feDiffuseLighting>
                  <feDistantLight azimuth="0" elevation="45"/>
                </feDiffuseLighting>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::DiffuseLighting { input, .. } = &g.primitives[1].primitive else {
            panic!("not diffuse-lighting");
        };
        assert_eq!(*input, FilterInput::Reference("b".into()));
    }

    #[test]
    fn parses_lighting_color_named_css() {
        // CSS-named "red" should resolve to (255,0,0).
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feSpecularLighting lighting-color="red">
                  <feDistantLight/>
                </feSpecularLighting>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::SpecularLighting { lighting_color, .. } = &g.primitives[0].primitive
        else {
            panic!("not specular-lighting");
        };
        assert_eq!(lighting_color.r, 255);
        assert_eq!(lighting_color.g, 0);
        assert_eq!(lighting_color.b, 0);
    }

    #[test]
    fn lighting_first_recognised_child_wins() {
        // Per spec there is at most one light source per lighting
        // primitive. We pick the first recognised child if multiple
        // are (illegally) present and ignore the rest.
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feDiffuseLighting>
                  <feDistantLight azimuth="10" elevation="20"/>
                  <fePointLight x="1" y="2" z="3"/>
                </feDiffuseLighting>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::DiffuseLighting { light_source, .. } = &g.primitives[0].primitive
        else {
            panic!("not diffuse-lighting");
        };
        match light_source {
            LightSource::Distant { azimuth, elevation } => {
                assert_eq!(*azimuth, 10.0);
                assert_eq!(*elevation, 20.0);
            }
            other => panic!("expected first child Distant, got {:?}", other),
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

    // ---- Round 11: feImage / feTile typed parsing ----

    #[test]
    fn parses_fe_image_with_href_and_aspect_ratio() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feImage href="texture.png" preserveAspectRatio="xMinYMin slice" crossorigin="anonymous"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        assert_eq!(g.primitives.len(), 1);
        let FilterPrimitive::Image {
            href,
            preserve_aspect_ratio,
            crossorigin,
        } = &g.primitives[0].primitive
        else {
            panic!("not Image");
        };
        assert_eq!(href, "texture.png");
        assert_eq!(
            preserve_aspect_ratio.align,
            PreserveAspectRatioAlign::XMinYMin
        );
        assert_eq!(preserve_aspect_ratio.meet_or_slice, MeetOrSlice::Slice);
        assert_eq!(*crossorigin, Some(CrossOrigin::Anonymous));
    }

    #[test]
    fn fe_image_falls_back_to_xlink_href_legacy() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
              <filter id="f">
                <feImage xlink:href="bg.jpg"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::Image { href, .. } = &g.primitives[0].primitive else {
            panic!("not Image");
        };
        assert_eq!(href, "bg.jpg");
    }

    #[test]
    fn fe_image_default_preserve_aspect_ratio_is_xmidymid_meet() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feImage href="x.png"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::Image {
            preserve_aspect_ratio,
            crossorigin,
            ..
        } = &g.primitives[0].primitive
        else {
            panic!("not Image");
        };
        assert_eq!(*preserve_aspect_ratio, PreserveAspectRatio::default());
        assert_eq!(*crossorigin, None);
    }

    #[test]
    fn fe_image_absent_href_records_empty_string() {
        // Per spec, a `<feImage>` with no href is a no-op transparent
        // black; we record that as `href=""` so the rasterizer can detect
        // the case.
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feImage/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::Image { href, .. } = &g.primitives[0].primitive else {
            panic!("not Image");
        };
        assert!(href.is_empty());
    }

    #[test]
    fn fe_image_crossorigin_use_credentials() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feImage href="cdn.png" crossorigin="use-credentials"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::Image { crossorigin, .. } = &g.primitives[0].primitive else {
            panic!("not Image");
        };
        assert_eq!(*crossorigin, Some(CrossOrigin::UseCredentials));
    }

    #[test]
    fn fe_image_crossorigin_empty_string_maps_to_anonymous() {
        // Per HTML §2.7 the empty value is treated as `anonymous`.
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feImage href="cdn.png" crossorigin=""/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::Image { crossorigin, .. } = &g.primitives[0].primitive else {
            panic!("not Image");
        };
        assert_eq!(*crossorigin, Some(CrossOrigin::Anonymous));
    }

    #[test]
    fn parses_fe_tile_with_explicit_input() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feFlood flood-color="#ff0000" result="rd"/>
                <feTile in="rd"/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        assert_eq!(g.primitives.len(), 2);
        let FilterPrimitive::Tile { input } = &g.primitives[1].primitive else {
            panic!("not Tile");
        };
        assert_eq!(*input, FilterInput::Reference("rd".into()));
    }

    #[test]
    fn fe_tile_implicit_input_threads_previous_result() {
        // No `in=` → defaults to the previous primitive's result per
        // §6.2.
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feGaussianBlur stdDeviation="2" result="b"/>
                <feTile/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::Tile { input } = &g.primitives[1].primitive else {
            panic!("not Tile");
        };
        assert_eq!(*input, FilterInput::Reference("b".into()));
    }

    #[test]
    fn fe_tile_first_primitive_defaults_to_source_graphic() {
        let f = first_filter(
            r##"<svg xmlns="http://www.w3.org/2000/svg">
              <filter id="f">
                <feTile/>
              </filter>
            </svg>"##,
        );
        let g = parse_filter_graph(&f);
        let FilterPrimitive::Tile { input } = &g.primitives[0].primitive else {
            panic!("not Tile");
        };
        assert_eq!(*input, FilterInput::SourceGraphic);
    }

    #[test]
    fn preserve_aspect_ratio_alignment_keywords_round_trip() {
        for (s, want) in [
            ("none", PreserveAspectRatioAlign::None),
            ("xMinYMin", PreserveAspectRatioAlign::XMinYMin),
            ("xMidYMin", PreserveAspectRatioAlign::XMidYMin),
            ("xMaxYMin", PreserveAspectRatioAlign::XMaxYMin),
            ("xMinYMid", PreserveAspectRatioAlign::XMinYMid),
            ("xMidYMid", PreserveAspectRatioAlign::XMidYMid),
            ("xMaxYMid", PreserveAspectRatioAlign::XMaxYMid),
            ("xMinYMax", PreserveAspectRatioAlign::XMinYMax),
            ("xMidYMax", PreserveAspectRatioAlign::XMidYMax),
            ("xMaxYMax", PreserveAspectRatioAlign::XMaxYMax),
        ] {
            assert_eq!(PreserveAspectRatioAlign::from_str(s), want, "for {s}");
        }
        // Unknown → default xMidYMid per spec.
        assert_eq!(
            PreserveAspectRatioAlign::from_str("not-a-real-value"),
            PreserveAspectRatioAlign::XMidYMid
        );
    }
}
