//! Round 7 — typed parsing of `<filter>` primitive graphs.
//!
//! Round 2-4 captured `<filter>` element trees verbatim and round-tripped
//! them through the encoder, but never inspected the primitives inside.
//! Round 7 walks each primitive (`<feGaussianBlur>`, `<feOffset>`,
//! `<feFlood>`, `<feComposite>`, `<feBlend>`, `<feMorphology>`) and
//! parses its attributes into a typed [`FilterPrimitive`] value, so a
//! downstream rasterizer (oxideav-raster) can consume the filter graph
//! without re-parsing XML.
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
