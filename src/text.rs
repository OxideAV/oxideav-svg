//! `<text>` / `<tspan>` / `<textPath>` layout (SVG 2 chapter 11) on
//! top of `oxideav-scribe`'s vector-first shaping API.
//!
//! The crate does not own a font registry: the caller installs a font
//! hook once ([`set_font_provider`], or the older
//! [`set_font_resolver`]). Without one every `<text>` parses to an
//! empty group and the rest of the document still loads.
//!
//! The engine follows the §11.5 layout algorithm:
//!
//! 1. **Collect** — the element tree is flattened into styled spans
//!    and one character array, applying §11.10.3 white-space handling
//!    (`xml:space` and `white-space`, collapsing across element
//!    boundaries, forced line breaks under `pre` / `pre-line`).
//!    Position lists (`x` / `y` / `dx` / `dy` / `rotate`) are resolved
//!    per addressable character, descendants overriding ancestors.
//! 2. **Shape** — each line is run through the Unicode bidirectional
//!    algorithm (`direction`, `unicode-bidi`), split into runs of one
//!    font / direction / orientation, shaped, and grouped into
//!    typographic characters. `letter-spacing` / `word-spacing` adjust
//!    the advances; a vertical `writing-mode` sets glyphs upright or
//!    sideways per `text-orientation`.
//! 3. **Position** — `dx` / `dy`, then `textLength` / `lengthAdjust`,
//!    then absolute `x` / `y`, then `text-anchor` per anchored chunk,
//!    then `<textPath>` (`startOffset`, `side`, `method`, the closed
//!    path wrap-around rule, the continue-from-the-end-point rule).
//! 4. **Paint** — every span is painted with its own cascaded `fill` /
//!    `stroke` / `paint-order`; gradient and pattern paint servers bind
//!    `objectBoundingBox` units to the bounding box of the whole
//!    `<text>` element (§11.1). `text-decoration` lines are drawn with
//!    the paint of the element that declared them (§11.11).
//!
//! A span whose paint is a plain colour with no stroke is emitted as
//! one placement group per glyph (so glyph nodes stay shareable); any
//! other paint flattens the span's outlines into a single path in the
//! text element's user space and routes it through the same paint
//! machinery shapes use.

mod path;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use oxideav_scribe::{FaceChain, PositionedGlyph};

use crate::bbox::BBox;
use crate::css::{MatchContext, declarations_for};
use crate::element::{
    DominantBaseline, PaintState, ParseContext, TextAnchor, TextLengthAdjust, Visibility,
    derive_child_ctx, paint_text_geometry,
};
use crate::error::Result;
use crate::length::{LengthAxis, ResolveContext, parse_length};
use crate::model::{Group, Node, Paint, Path, PathCommand, PathNode, Point, Stroke, Transform2D};
use crate::parser::{Element, Node as XmlNode, attr, tag_local};
use crate::text_style::{
    DECORATION_LINE_THROUGH, DECORATION_OVERLINE, DECORATION_UNDERLINE, Direction, FontStyle,
    LineHeight, TextOrientation, WritingMode, parse_decoration_lines,
};

use path::PathSampler;

// ───────────────────────── font hooks ─────────────────────────

type FontResolver = dyn Fn(&str, f32) -> Option<FaceChain> + Send + Sync + 'static;

static FONT_RESOLVER: OnceLock<Box<FontResolver>> = OnceLock::new();

/// The global font hooks are one-shot — only the first
/// [`set_font_resolver`] / [`set_font_provider`] call of each kind
/// wins. Subsequent calls return this error without overwriting the
/// original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolverAlreadySet;

impl std::fmt::Display for ResolverAlreadySet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("font resolver was already set")
    }
}

impl std::error::Error for ResolverAlreadySet {}

/// Install a font resolver keyed by `(font-family, font-size)`. The
/// family string is first the element's raw `font-family` value (comma
/// separated list, quotes kept); when the resolver answers `None` it
/// is asked again for each family of the list in order, and finally
/// for `sans-serif`, so a missing family falls back instead of
/// dropping the text.
///
/// The resolver is called once per distinct `(family, size)` of a
/// document, not once per text run. Prefer [`set_font_provider`],
/// which also receives the weight and style and whose answer is shared
/// across sizes.
///
/// Returns [`ResolverAlreadySet`] if a resolver was already set (the
/// hook is one-shot — applications register at startup). With no hook
/// installed every `<text>` parses to an empty group.
pub fn set_font_resolver<F>(resolver: F) -> std::result::Result<(), ResolverAlreadySet>
where
    F: Fn(&str, f32) -> Option<FaceChain> + Send + Sync + 'static,
{
    FONT_RESOLVER
        .set(Box::new(resolver))
        .map_err(|_| ResolverAlreadySet)
}

/// What a [`set_font_provider`] hook is asked for.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FontQuery {
    family: String,
    size_px: f32,
    weight: u16,
    italic: bool,
}

impl FontQuery {
    /// Build a query (useful for testing a provider).
    pub fn new(family: impl Into<String>, size_px: f32) -> Self {
        Self {
            family: family.into(),
            size_px,
            weight: 400,
            italic: false,
        }
    }

    /// Set the requested weight (1..=1000).
    pub fn with_weight(mut self, weight: u16) -> Self {
        self.weight = weight.clamp(1, 1000);
        self
    }

    /// Set the requested slant.
    pub fn with_italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }

    /// The family being asked for: first the raw `font-family` value
    /// (the whole comma separated list), then — if the provider
    /// declined — each family of the list on its own, unquoted, then
    /// `sans-serif`.
    pub fn family(&self) -> &str {
        &self.family
    }

    /// The families of [`Self::family`], split at commas and unquoted.
    pub fn families(&self) -> impl Iterator<Item = &str> {
        split_families(&self.family)
    }

    /// `font-size` in user units of the first run that needs the font.
    /// The provider's answer is reused for every size.
    pub fn size_px(&self) -> f32 {
        self.size_px
    }

    /// `font-weight` (400 normal, 700 bold).
    pub fn weight(&self) -> u16 {
        self.weight
    }

    /// `font-style` is `italic` or `oblique`.
    pub fn italic(&self) -> bool {
        self.italic
    }
}

type FontProvider = dyn Fn(&FontQuery) -> Option<Arc<FaceChain>> + Send + Sync + 'static;

static FONT_PROVIDER: OnceLock<Box<FontProvider>> = OnceLock::new();

/// Install a font provider: asked once per distinct
/// `(family, weight, style)` of a document, with the answer shared by
/// every size and every run — a provider can hand out the same
/// `Arc<FaceChain>` forever and no font is parsed twice.
///
/// When the returned primary face is upright but the query is italic,
/// the outlines are sheared; when it is not bold but the query asks for
/// 600 or more, opaque solid text is emboldened by a thin outline.
///
/// Takes precedence over [`set_font_resolver`]. One-shot, like it.
pub fn set_font_provider<F>(provider: F) -> std::result::Result<(), ResolverAlreadySet>
where
    F: Fn(&FontQuery) -> Option<Arc<FaceChain>> + Send + Sync + 'static,
{
    FONT_PROVIDER
        .set(Box::new(provider))
        .map_err(|_| ResolverAlreadySet)
}

fn split_families(list: &str) -> impl Iterator<Item = &str> {
    list.split(',')
        .map(|f| f.trim().trim_matches(|c| c == '"' || c == '\'').trim())
        .filter(|f| !f.is_empty())
}

/// Font-unit metrics of a chain's primary face, per 1 px of font size.
#[derive(Clone, Copy, Debug)]
struct UnitMetrics {
    ascent: f32,
    /// Positive distance below the baseline.
    descent: f32,
    line_height: f32,
    x_height: f32,
}

/// Glyph outline nodes by `(face index, glyph id, size bits)`.
type GlyphCache = HashMap<(u16, u16, u32), Option<Rc<Node>>>;

/// One resolved font: the face chain plus per-document caches.
pub(crate) struct FontEntry {
    chain: Arc<FaceChain>,
    /// Synthetic-oblique shear (`x += shear · height above baseline`).
    shear: f32,
    /// Requested bold on a non-bold face.
    embolden: bool,
    /// The primary face publishes the small-capitals feature.
    has_smcp: bool,
    unit: UnitMetrics,
    /// Glyph outline nodes by `(face index, glyph id, size bits)`.
    glyphs: RefCell<GlyphCache>,
    /// The single glyph a character shapes to on its own, if any.
    isolated: RefCell<HashMap<(char, bool), Option<u16>>>,
}

impl std::fmt::Debug for FontEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontEntry")
            .field("faces", &self.chain.len())
            .field("shear", &self.shear)
            .field("embolden", &self.embolden)
            .finish()
    }
}

impl FontEntry {
    fn new(chain: Arc<FaceChain>, weight: u16, italic: bool) -> Option<Self> {
        if chain.is_empty() {
            return None;
        }
        let face = chain.primary();
        let ascent = face.ascent_px(1.0);
        let descent = -face.descent_px(1.0);
        let line_height = face.line_height_px(1.0);
        // x-height: the top of the `x` outline (font units, y up).
        let x_height = chain
            .shape("x", 1000.0)
            .ok()
            .and_then(|g| g.first().copied())
            .and_then(|g| {
                let f = chain.face(g.face_idx);
                let p = f.glyph_path(g.glyph_id)?;
                let b = oxideav_core_path_top(&p)?;
                Some(b / f.units_per_em().max(1) as f32)
            })
            .filter(|h| h.is_finite() && *h > 0.0)
            .unwrap_or(0.5);
        let shear = if italic && face.italic_angle().abs() <= 0.5 {
            12.0f32.to_radians().tan()
        } else {
            0.0
        };
        let (ascent, descent) = if ascent.is_finite() && descent.is_finite() && ascent > 0.0 {
            (ascent, descent.max(0.0))
        } else {
            (0.8, 0.2)
        };
        Some(Self {
            has_smcp: [*b"latn", *b"DFLT", *b"cyrl", *b"grek"]
                .iter()
                .any(|script| face.has_gsub_feature(*script, *b"smcp")),
            embolden: weight >= 600 && face.weight_class() < 600,
            shear,
            unit: UnitMetrics {
                ascent,
                descent,
                line_height: if line_height.is_finite() && line_height > 0.0 {
                    line_height
                } else {
                    1.2
                },
                x_height,
            },
            chain,
            glyphs: RefCell::new(HashMap::new()),
            isolated: RefCell::new(HashMap::new()),
        })
    }

    /// The outline node of one glyph at `size`, shared across uses.
    fn glyph(&self, face_idx: u16, glyph_id: u16, size: f32) -> Option<Rc<Node>> {
        let key = (face_idx, glyph_id, size.to_bits());
        if let Some(hit) = self.glyphs.borrow().get(&key) {
            return hit.clone();
        }
        let node = self
            .chain
            .face(face_idx)
            .glyph_node(glyph_id, size)
            .map(|n| Rc::new(crate::registry::node_from_core(&n)));
        self.glyphs.borrow_mut().insert(key, node.clone());
        node
    }

    fn shape(&self, text: &str, size: f32, features: Option<&[[u8; 4]]>) -> Vec<PositionedGlyph> {
        match features {
            Some(f) => self.chain.shape_with_features(text, size, f),
            None => self.chain.shape(text, size),
        }
        .unwrap_or_default()
    }

    /// The glyph `c` shapes to in isolation, when it is exactly one.
    fn isolated_glyph(&self, c: char, features: Option<&[[u8; 4]]>) -> Option<u16> {
        let key = (c, features.is_some());
        if let Some(hit) = self.isolated.borrow().get(&key) {
            return *hit;
        }
        let mut buf = [0u8; 4];
        let g = self.shape(c.encode_utf8(&mut buf), 16.0, features);
        let id = (g.len() == 1).then(|| g[0].glyph_id);
        self.isolated.borrow_mut().insert(key, id);
        id
    }
}

/// Highest y of a font-unit outline (y up).
fn oxideav_core_path_top(p: &oxideav_core::Path) -> Option<f32> {
    let mut top = f32::NEG_INFINITY;
    for c in &p.commands {
        match c {
            oxideav_core::PathCommand::MoveTo(p) | oxideav_core::PathCommand::LineTo(p) => {
                top = top.max(p.y);
            }
            // On-curve ends bound the `x` glyph closely enough.
            oxideav_core::PathCommand::QuadCurveTo { end, .. }
            | oxideav_core::PathCommand::CubicCurveTo { end, .. } => top = top.max(end.y),
            _ => {}
        }
    }
    top.is_finite().then_some(top)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FontKey {
    family: Arc<str>,
    weight: u16,
    italic: bool,
    /// Size bits under the legacy `(family, size)` resolver; 0 under a
    /// provider (its answer is size independent).
    size_bits: u32,
}

/// Per-document text caches: resolved fonts (and, inside each, glyph
/// outlines). Lives on the [`ParseContext`] so a font is resolved once
/// per document rather than once per run.
#[derive(Debug, Default)]
pub(crate) struct TextCaches {
    fonts: HashMap<FontKey, Option<Rc<FontEntry>>>,
}

impl TextCaches {
    fn resolve(
        &mut self,
        family: Option<&Arc<str>>,
        weight: u16,
        italic: bool,
        size: f32,
    ) -> Option<Rc<FontEntry>> {
        let provider = FONT_PROVIDER.get();
        let resolver = FONT_RESOLVER.get();
        if provider.is_none() && resolver.is_none() {
            return None;
        }
        let family: Arc<str> = family.cloned().unwrap_or_else(|| Arc::from("sans-serif"));
        let key = FontKey {
            family: family.clone(),
            weight,
            italic,
            size_bits: if provider.is_some() {
                0
            } else {
                size.to_bits()
            },
        };
        if let Some(hit) = self.fonts.get(&key) {
            return hit.clone();
        }
        // The whole list first, then each family, then the generic
        // fallback (CSS Fonts font matching: the first family that
        // resolves wins).
        let mut candidates: Vec<&str> = vec![&family];
        let singles: Vec<&str> = split_families(&family).collect();
        if singles.len() > 1 || singles.first().is_some_and(|s| *s != &*family) {
            candidates.extend(singles.iter().copied());
        }
        if !singles.iter().any(|s| s.eq_ignore_ascii_case("sans-serif")) {
            candidates.push("sans-serif");
        }
        let mut found: Option<Arc<FaceChain>> = None;
        for cand in candidates {
            found = if let Some(p) = provider {
                p(&FontQuery {
                    family: cand.to_string(),
                    size_px: size,
                    weight,
                    italic,
                })
            } else {
                #[allow(clippy::arc_with_non_send_sync)]
                resolver.and_then(|r| r(cand, size)).map(Arc::new)
            };
            if found.is_some() {
                break;
            }
        }
        let entry = found
            .and_then(|chain| FontEntry::new(chain, weight, italic))
            .map(Rc::new);
        self.fonts.insert(key, entry.clone());
        entry
    }
}

// ───────────────────────── collected tree ─────────────────────────

/// Which baseline of a span's glyphs sits on the text position
/// (`dominant-baseline` / `alignment-baseline`, SVG 2 §11.10.2.5–6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Baseline {
    Alphabetic,
    Ideographic,
    Hanging,
    Mathematical,
    Central,
    Middle,
    TextTop,
    TextBottom,
}

impl Baseline {
    fn from_alignment(v: &str) -> Option<Self> {
        Some(match v.trim().to_ascii_lowercase().as_str() {
            "alphabetic" => Baseline::Alphabetic,
            "ideographic" => Baseline::Ideographic,
            "hanging" => Baseline::Hanging,
            "mathematical" => Baseline::Mathematical,
            "central" => Baseline::Central,
            "middle" => Baseline::Middle,
            "text-top" | "text-before-edge" | "before-edge" => Baseline::TextTop,
            "text-bottom" | "text-after-edge" | "after-edge" => Baseline::TextBottom,
            _ => return None,
        })
    }

    fn from_dominant(d: DominantBaseline) -> Option<Self> {
        Some(match d {
            DominantBaseline::Alphabetic => Baseline::Alphabetic,
            DominantBaseline::Ideographic => Baseline::Ideographic,
            DominantBaseline::Hanging => Baseline::Hanging,
            DominantBaseline::Mathematical => Baseline::Mathematical,
            DominantBaseline::Central => Baseline::Central,
            DominantBaseline::Middle => Baseline::Middle,
            DominantBaseline::TextBeforeEdge => Baseline::TextTop,
            DominantBaseline::TextAfterEdge => Baseline::TextBottom,
            _ => return None,
        })
    }

    /// Height of this baseline above the alphabetic one, in px.
    fn height(self, m: &UnitMetrics, size: f32) -> f32 {
        size * match self {
            Baseline::Alphabetic => 0.0,
            Baseline::Ideographic | Baseline::TextBottom => -m.descent,
            // Without a font baseline table: the hanging baseline sits
            // at 0.8 of the ascent, the mathematical one at half of it.
            Baseline::Hanging => m.ascent * 0.8,
            Baseline::Mathematical => m.ascent * 0.5,
            Baseline::Central => (m.ascent - m.descent) * 0.5,
            Baseline::Middle => m.x_height * 0.5,
            Baseline::TextTop => m.ascent,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct TextLengthSpec {
    target: f32,
    adjust: TextLengthAdjust,
}

/// A resolved `<textPath>`.
#[derive(Debug)]
struct TextPathInfo {
    /// `None`: the reference is invalid — the content does not render.
    sampler: Option<PathSampler>,
    start_offset: f32,
    stretch: bool,
}

/// `text-decoration-style` (CSS Text Decoration, adopted by §11.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecorationStyle {
    Solid,
    Double,
    Dotted,
    Dashed,
    Wavy,
}

impl DecorationStyle {
    fn parse(tok: &str) -> Option<Self> {
        Some(match tok.to_ascii_lowercase().as_str() {
            "solid" => DecorationStyle::Solid,
            "double" => DecorationStyle::Double,
            "dotted" => DecorationStyle::Dotted,
            "dashed" => DecorationStyle::Dashed,
            "wavy" => DecorationStyle::Wavy,
            _ => return None,
        })
    }
}

/// A decoration declared somewhere up the tree (§11.11): painted with
/// the declaring element's paint and metrics.
#[derive(Debug)]
struct Decoration {
    lines: u8,
    style: DecorationStyle,
    state: PaintState,
    font_size: f32,
    unit: UnitMetrics,
    /// Block-axis displacement of the declaring span's baseline, so a
    /// shifted child keeps the declaring element's line.
    perp: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpanKind {
    Text,
    Tspan,
    TextPath,
    Other,
}

#[derive(Debug)]
struct Span {
    parent: Option<usize>,
    state: PaintState,
    font: Option<Rc<FontEntry>>,
    font_size: f32,
    letter_spacing: f32,
    word_spacing: f32,
    line_height: f32,
    xs: Vec<f32>,
    ys: Vec<f32>,
    dxs: Vec<f32>,
    dys: Vec<f32>,
    rotates: Vec<f32>,
    text_length: Option<TextLengthSpec>,
    text_path: Option<Rc<TextPathInfo>>,
    /// The enclosing `<textPath>` span, if any (itself for one).
    path_owner: Option<usize>,
    /// The baseline named by this span or the nearest ancestor.
    named_baseline: Option<Baseline>,
    /// Accumulated `baseline-shift` (positive raises the glyphs).
    shift: f32,
    /// Displacement of this span's alphabetic baseline from the text
    /// position along the block axis, in user units (positive = down
    /// for horizontal text, = right for vertical).
    perp: f32,
    decorations: Vec<usize>,
    /// Inclusive character range of the subtree; `first > last` when
    /// empty.
    first: usize,
    last: usize,
}

#[derive(Clone, Copy, Debug)]
struct Ch {
    c: char,
    span: u32,
    /// Forced line breaks immediately before this character.
    breaks: u16,
    /// A bidi formatting character: takes part in the bidirectional
    /// algorithm only (not addressable, not rendered).
    ctrl: bool,
}

struct Collector<'a> {
    ctx: &'a ParseContext,
    caches: TextCaches,
    spans: Vec<Span>,
    chars: Vec<Ch>,
    decos: Vec<Decoration>,
    /// A collapsible space seen but not yet committed (it is dropped at
    /// a line end), with the span it belongs to.
    pending_space: Option<u32>,
    at_line_start: bool,
    pending_breaks: u16,
    vertical: bool,
    /// `inline-size` wrapping is requested.
    wrapping: bool,
}

fn own_prop<'a>(el: &'a Element, decls: &'a [(String, String)], name: &str) -> Option<&'a str> {
    decls
        .iter()
        .rev()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
        .or_else(|| attr(el, name))
}

const FALLBACK_METRICS: UnitMetrics = UnitMetrics {
    ascent: 0.8,
    descent: 0.2,
    line_height: 1.2,
    x_height: 0.5,
};

impl Collector<'_> {
    fn open_span(
        &mut self,
        el: &Element,
        mctx: &MatchContext<'_>,
        mut state: PaintState,
        rctx: &ResolveContext,
        parent: Option<usize>,
        kind: SpanKind,
    ) -> usize {
        state.resolve_relative_lengths(rctx);
        let decls = declarations_for(mctx, &self.ctx.stylesheet);
        let size = rctx.font_size_px;
        let tp = &state.text;
        let font = self.caches.resolve(
            tp.font_family.as_ref(),
            tp.font_weight,
            tp.font_style != FontStyle::Normal,
            size,
        );
        let unit = font.as_ref().map(|f| f.unit).unwrap_or(FALLBACK_METRICS);
        // `em` and `%` in the spacing properties refer to the font size.
        let font_rel = ResolveContext {
            percentage_basis_px: size,
            ..*rctx
        };
        let letter_spacing = tp
            .letter_spacing
            .map(|l| l.resolve(font_rel))
            .unwrap_or(0.0);
        let word_spacing = tp.word_spacing.map(|l| l.resolve(font_rel)).unwrap_or(0.0);
        let line_height = match tp.line_height {
            LineHeight::Normal => unit.line_height * size,
            LineHeight::Number(n) => n * size,
            LineHeight::Length(l) => l.resolve(font_rel),
        };
        let positioned = matches!(kind, SpanKind::Text | SpanKind::Tspan);
        let list = |name: &str, axis: LengthAxis| -> Vec<f32> {
            if positioned {
                parse_length_list(attr(el, name), axis, rctx)
            } else {
                Vec::new()
            }
        };
        let xs = list("x", LengthAxis::X);
        let ys = list("y", LengthAxis::Y);
        let dxs = list("dx", LengthAxis::X);
        let dys = list("dy", LengthAxis::Y);
        let rotates = if positioned {
            parse_number_list(attr(el, "rotate"))
        } else {
            Vec::new()
        };
        let inline_axis = if self.vertical {
            LengthAxis::Y
        } else {
            LengthAxis::X
        };
        let text_length = parse_text_length(
            attr(el, "textLength"),
            attr(el, "lengthAdjust"),
            inline_axis,
            rctx,
        );

        // Baselines (§11.10.2.5–7). `alignment-baseline` on the element
        // wins over `dominant-baseline`; `auto` continues the parent's
        // choice. `baseline-shift` accumulates down the tree.
        let parent_span = parent.map(|p| &self.spans[p]);
        let named_baseline = own_prop(el, &decls, "alignment-baseline")
            .and_then(Baseline::from_alignment)
            .or_else(|| Baseline::from_dominant(state.dominant_baseline))
            .or_else(|| parent_span.and_then(|p| p.named_baseline));
        let shift = parent_span.map(|p| p.shift).unwrap_or(0.0)
            + own_prop(el, &decls, "baseline-shift")
                .map(|v| parse_baseline_shift(v, size, &font_rel))
                .unwrap_or(0.0);
        // Horizontal: aligning a baseline `h` above the alphabetic one
        // with the text position lowers the glyphs by `h`; a shift
        // raises them. Vertical: the block axis is x, the glyph frame
        // centres the glyphs on it and a shift moves toward the
        // line-over side (+x).
        let perp = if self.vertical {
            shift
        } else {
            named_baseline
                .unwrap_or(Baseline::Alphabetic)
                .height(&unit, size)
                - shift
        };

        // Decorations (§11.11): the lines an element declares decorate
        // its whole subtree, painted with that element's paint.
        let mut decorations = parent_span
            .map(|p| p.decorations.clone())
            .unwrap_or_default();
        let own_lines = own_prop(el, &decls, "text-decoration-line")
            .or_else(|| own_prop(el, &decls, "text-decoration"))
            .and_then(parse_decoration_lines)
            .or((parent.is_none() && tp.decoration != 0).then_some(tp.decoration))
            .unwrap_or(0);
        if own_lines != 0 {
            let mut deco_state = state.clone();
            let shorthand_color = own_prop(el, &decls, "text-decoration").and_then(|v| {
                v.split_whitespace().find(|t| {
                    !matches!(
                        t.to_ascii_lowercase().as_str(),
                        "none"
                            | "underline"
                            | "overline"
                            | "line-through"
                            | "blink"
                            | "solid"
                            | "double"
                            | "dotted"
                            | "dashed"
                            | "wavy"
                    )
                })
            });
            for (prop, is_stroke) in [
                ("text-decoration-color", false),
                ("text-decoration-fill", false),
                ("text-decoration-stroke", true),
            ] {
                let value = own_prop(el, &decls, prop)
                    .or(shorthand_color.filter(|_| prop == "text-decoration-color"));
                if let Some(paint) = value.and_then(|c| crate::color::parse_paint(c).ok()) {
                    if is_stroke {
                        deco_state.stroke = paint;
                    } else {
                        deco_state.fill = paint;
                    }
                }
            }
            let style = own_prop(el, &decls, "text-decoration-style")
                .and_then(|v| DecorationStyle::parse(v.trim()))
                .or_else(|| {
                    own_prop(el, &decls, "text-decoration")
                        .and_then(|v| v.split_whitespace().find_map(DecorationStyle::parse))
                })
                .unwrap_or(DecorationStyle::Solid);
            self.decos.push(Decoration {
                lines: own_lines,
                style,
                state: deco_state,
                font_size: size,
                unit,
                perp,
            });
            decorations.push(self.decos.len() - 1);
        }

        let idx = self.spans.len();
        let (text_path, path_owner) = if kind == SpanKind::TextPath {
            (
                Some(Rc::new(resolve_text_path(el, self.ctx, rctx))),
                Some(idx),
            )
        } else {
            (None, parent_span.and_then(|p| p.path_owner))
        };
        self.spans.push(Span {
            parent,
            state,
            font,
            font_size: size,
            letter_spacing,
            word_spacing,
            line_height,
            xs,
            ys,
            dxs,
            dys,
            rotates,
            text_length,
            text_path,
            path_owner,
            named_baseline,
            shift,
            perp,
            decorations,
            first: usize::MAX,
            last: 0,
        });
        idx
    }

    /// Walk the children of a text content element.
    fn walk(
        &mut self,
        el: &Element,
        span: usize,
        mctx: &MatchContext<'_>,
        rctx: &ResolveContext,
        depth: usize,
    ) -> Result<()> {
        // Sibling bookkeeping for structural selectors on the children.
        let mut total = 0usize;
        let mut tag_totals: HashMap<String, usize> = HashMap::new();
        for c in &el.children {
            if let XmlNode::Element(e) = c {
                total += 1;
                *tag_totals.entry(tag_local(&e.name)).or_insert(0) += 1;
            }
        }
        let mut child_idx = 0usize;
        let mut tag_seen: HashMap<String, usize> = HashMap::new();
        for child in &el.children {
            match child {
                XmlNode::Text(t) => self.push_text(t, span),
                XmlNode::Element(c) => {
                    let local = tag_local(&c.name);
                    let of_idx = *tag_seen.entry(local.clone()).or_insert(0);
                    *tag_seen.get_mut(&local).unwrap() += 1;
                    let cm = MatchContext {
                        el: c,
                        child_index: child_idx,
                        of_type_index: of_idx,
                        sibling_count: total,
                        of_type_count: *tag_totals.get(&local).unwrap_or(&0),
                        parent: Some(mctx),
                    };
                    child_idx += 1;
                    // §11.5: only text content child elements and `a`
                    // render inside a `<text>`; anything else is
                    // `display: none`.
                    let kind = match local.as_str() {
                        "tspan" => SpanKind::Tspan,
                        "textpath" => SpanKind::TextPath,
                        "a" => SpanKind::Other,
                        _ => continue,
                    };
                    if depth >= crate::element::MAX_RENDER_DEPTH {
                        continue;
                    }
                    let state = self.spans[span]
                        .state
                        .merged_with_mctx(&cm, &self.ctx.stylesheet)?;
                    if !state.display {
                        continue;
                    }
                    let child_rctx = derive_child_ctx(c, &cm, &self.ctx.stylesheet, rctx);
                    // `unicode-bidi` (CSS Writing Modes §2.2): an
                    // embedding / override / isolate is expressed to the
                    // bidirectional algorithm with the matching
                    // formatting characters around the element.
                    let decls = declarations_for(&cm, &self.ctx.stylesheet);
                    let (open, close) = bidi_controls(
                        own_prop(c, &decls, "unicode-bidi"),
                        state.text.direction == Direction::Rtl,
                    );
                    let idx = self.open_span(c, &cm, state, &child_rctx, Some(span), kind);
                    // An absolute `x` / `y` repositions the text like a
                    // line start, and a `<textPath>` begins its own
                    // line: collapsible white space before either is
                    // line-end white space and is trimmed (§11.5
                    // "trimmed from the start or end of a line"), so it
                    // cannot skew the previous chunk's `text-anchor`.
                    let sp = &self.spans[idx];
                    // (Not in auto-wrapped text, where those positions
                    // are ignored.)
                    if kind == SpanKind::TextPath
                        || (!self.wrapping && (!sp.xs.is_empty() || !sp.ys.is_empty()))
                    {
                        self.pending_space = None;
                        self.at_line_start = true;
                    }
                    if let Some(o) = open {
                        self.flush_space();
                        self.push_ctrl(o, idx);
                    }
                    self.walk(c, idx, &cm, &child_rctx, depth + 1)?;
                    if open.is_some() {
                        self.push_ctrl(close, idx);
                    }
                }
            }
        }
        Ok(())
    }

    fn push_ctrl(&mut self, c: char, span: usize) {
        self.chars.push(Ch {
            c,
            span: span as u32,
            breaks: 0,
            ctrl: true,
        });
    }

    /// Commit a pending collapsible space before a visible character.
    fn flush_space(&mut self) {
        if let Some(span) = self.pending_space.take() {
            self.chars.push(Ch {
                c: ' ',
                span,
                breaks: std::mem::take(&mut self.pending_breaks),
                ctrl: false,
            });
        }
    }

    fn push_visible(&mut self, c: char, span: usize) {
        self.flush_space();
        self.chars.push(Ch {
            c,
            span: span as u32,
            breaks: std::mem::take(&mut self.pending_breaks),
            ctrl: false,
        });
        self.at_line_start = false;
    }

    /// §11.10.3 white-space processing of one text node.
    fn push_text(&mut self, text: &str, span: usize) {
        let tp = &self.spans[span].state.text;
        // §11.10.3.3: `white-space`, once set, wins over `xml:space`.
        let (css, xml_preserve) = (tp.white_space, tp.xml_space_preserve);
        for c in text.chars() {
            let kind = match (css, c) {
                // Soft hyphens are discarded (§11.5: not addressable);
                // no line is ever broken at one here.
                (_, '\u{00AD}') => Kind::Drop,
                (None, '\n' | '\r') if !xml_preserve => Kind::Drop,
                (None, '\t' | ' ') if !xml_preserve => Kind::Collapsible,
                (None, '\n' | '\r' | '\t') => Kind::Keep(' '),
                (Some(ws), '\n' | '\r') if ws.preserves_breaks() => Kind::Break,
                (Some(ws), '\n' | '\r' | '\t' | ' ') if ws.collapses() => Kind::Collapsible,
                _ => Kind::Keep(c),
            };
            match kind {
                Kind::Drop => {}
                Kind::Collapsible => {
                    if !self.at_line_start && self.pending_space.is_none() {
                        self.pending_space = Some(span as u32);
                    }
                }
                Kind::Break => {
                    // Collapsible space around a forced break goes.
                    self.pending_space = None;
                    self.pending_breaks = self.pending_breaks.saturating_add(1);
                    self.at_line_start = true;
                }
                Kind::Keep(k) => {
                    if is_bidi_control(k) {
                        self.flush_space();
                        self.push_ctrl(k, span);
                    } else {
                        self.push_visible(k, span);
                    }
                }
            }
        }
    }
}

/// `unicode-bidi` (CSS Writing Modes §2.2): an embedding / override /
/// isolate is expressed to the bidirectional algorithm with the
/// matching formatting characters around the element's content.
/// Returns the opening character (if any) and the closing one.
fn bidi_controls(value: Option<&str>, rtl: bool) -> (Option<char>, char) {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("embed") => (Some(if rtl { '\u{202B}' } else { '\u{202A}' }), '\u{202C}'),
        Some("bidi-override") => (Some(if rtl { '\u{202E}' } else { '\u{202D}' }), '\u{202C}'),
        Some("isolate") => (Some(if rtl { '\u{2067}' } else { '\u{2066}' }), '\u{2069}'),
        Some("isolate-override") => (Some(if rtl { '\u{202E}' } else { '\u{202D}' }), '\u{202C}'),
        Some("plaintext") => (Some('\u{2068}'), '\u{2069}'),
        _ => (None, '\u{202C}'),
    }
}

/// What white-space processing turns one character into.
enum Kind {
    Collapsible,
    Break,
    Keep(char),
    Drop,
}

fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{200E}' | '\u{200F}' | '\u{061C}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// A list of lengths (`x` / `y` / `dx` / `dy`), each resolved to user
/// units — `em` against the element's font size, `%` against the
/// viewport axis.
fn parse_length_list(v: Option<&str>, axis: LengthAxis, rctx: &ResolveContext) -> Vec<f32> {
    let Some(raw) = v else {
        return Vec::new();
    };
    let basis = ResolveContext {
        percentage_basis_px: rctx.percentage_basis_for(axis),
        ..*rctx
    };
    raw.split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .filter_map(|t| match parse_length(t) {
            Ok(l) => Some(l.resolve(basis)),
            Err(_) => crate::element::parse_number(Some(t), f32::NAN).ok(),
        })
        .filter(|v| v.is_finite())
        .collect()
}

/// A list of numbers (`rotate`, in degrees).
fn parse_number_list(v: Option<&str>) -> Vec<f32> {
    let Some(raw) = v else {
        return Vec::new();
    };
    raw.split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .filter_map(|t| crate::element::parse_number(Some(t), f32::NAN).ok())
        .filter(|v| v.is_finite())
        .collect()
}

/// §11.2.1 `textLength` + `lengthAdjust`. A negative length is an
/// error and the attribute is ignored.
fn parse_text_length(
    text_length: Option<&str>,
    length_adjust: Option<&str>,
    axis: LengthAxis,
    rctx: &ResolveContext,
) -> Option<TextLengthSpec> {
    let raw = text_length?.trim();
    let basis = ResolveContext {
        percentage_basis_px: rctx.percentage_basis_for(axis),
        ..*rctx
    };
    let target = parse_length(raw).ok()?.resolve(basis);
    if !target.is_finite() || target < 0.0 {
        return None;
    }
    let adjust = match length_adjust.map(str::trim) {
        Some(s) if s.eq_ignore_ascii_case("spacingandglyphs") => TextLengthAdjust::SpacingAndGlyphs,
        _ => TextLengthAdjust::Spacing,
    };
    Some(TextLengthSpec { target, adjust })
}

/// `baseline-shift` (§11.10.2.7): `sub` / `super` / a length / a
/// percentage of the font size. Positive raises.
fn parse_baseline_shift(v: &str, size: f32, font_rel: &ResolveContext) -> f32 {
    let t = v.trim();
    match t.to_ascii_lowercase().as_str() {
        "baseline" | "" => 0.0,
        // A subscript sits a fifth of an em low, a superscript a third
        // of an em high.
        "sub" => -0.2 * size,
        "super" => size / 3.0,
        _ => parse_length(t).map(|l| l.resolve(*font_rel)).unwrap_or(0.0),
    }
}

/// Resolve a `<textPath>` (§11.8.1): the `path` attribute, else the
/// `href` / `xlink:href` target — a `<path>` or a basic shape (its
/// equivalent path), with the target's own `transform` applied.
fn resolve_text_path(el: &Element, ctx: &ParseContext, rctx: &ResolveContext) -> TextPathInfo {
    let from_attr = attr(el, "path").and_then(|d| {
        let cmds = crate::path_data::parse_path_data(d).ok()?;
        (!cmds.is_empty()).then(|| (Path { commands: cmds }, Transform2D::identity(), None))
    });
    let resolved = from_attr.or_else(|| {
        // `path=""` present but empty/invalid falls through to `href`
        // (§11.8.1: "If the path attribute contains an error, the href
        // attribute must be used").
        let href = attr(el, "href").or_else(|| attr(el, "xlink:href"))?;
        let id = href.trim().strip_prefix('#')?;
        let target = ctx.defs.elements.get(id)?;
        let path = match tag_local(&target.name).as_str() {
            "path" => {
                let cmds = crate::path_data::parse_path_data(attr(target, "d")?).ok()?;
                Path { commands: cmds }
            }
            "rect" => crate::element::parse_rect(target, rctx).ok()??,
            "circle" => crate::element::parse_circle(target, rctx).ok()??,
            "ellipse" => crate::element::parse_ellipse(target, rctx).ok()??,
            "line" => crate::element::parse_line(target, rctx).ok()?,
            "polyline" => crate::element::parse_polyline(target, false).ok()??,
            "polygon" => crate::element::parse_polyline(target, true).ok()??,
            _ => return None,
        };
        let transform = attr(target, "transform")
            .and_then(|t| crate::transform::parse_transform(t).ok())
            .unwrap_or_else(Transform2D::identity);
        let author_len = crate::path_length::parse_path_length(attr(target, "pathLength"));
        Some((path, transform, author_len))
    });
    let Some((path, transform, author_len)) = resolved else {
        return TextPathInfo {
            sampler: None,
            start_offset: 0.0,
            stretch: false,
        };
    };
    let mut sampler = PathSampler::new(&path, &transform);
    if attr(el, "side").map(str::trim) == Some("right") {
        sampler = sampler.map(|s| s.reversed());
    }
    let length = sampler.as_ref().map(|s| s.length()).unwrap_or(0.0);
    // §11.8.2 `startOffset`: a percentage is of the path length; a
    // length is scaled by the referenced element's `pathLength`.
    let start_offset = match attr(el, "startOffset").map(str::trim) {
        None | Some("") => 0.0,
        Some(raw) => {
            if let Some(pct) = raw.strip_suffix('%') {
                pct.trim().parse::<f32>().unwrap_or(0.0) * 0.01 * length
            } else {
                let v = parse_length(raw).map(|l| l.resolve(*rctx)).unwrap_or(0.0);
                match author_len {
                    Some(al) if al > 0.0 => v * length / al,
                    _ => v,
                }
            }
        }
    };
    TextPathInfo {
        sampler,
        start_offset: if start_offset.is_finite() {
            start_offset
        } else {
            0.0
        },
        stretch: attr(el, "method").map(str::trim) == Some("stretch"),
    }
}

// ───────────────────────── layout ─────────────────────────

/// Glyph orientation of a typographic character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Orient {
    /// Horizontal writing mode.
    Horizontal,
    /// Vertical line, glyph upright (advance = one em down the line).
    Upright,
    /// Vertical line, glyph rotated a quarter turn.
    Sideways,
}

#[derive(Clone, Copy, Debug)]
struct GlyphRef {
    face_idx: u16,
    glyph_id: u16,
    /// Offset of the glyph origin from the cluster origin.
    x: f32,
    y: f32,
}

/// One typographic character (§11.1.1): one or more characters shaped
/// to one or more glyphs, positioned as a unit.
#[derive(Debug)]
struct Cluster {
    /// Character range `[first, end)`.
    first: usize,
    end: usize,
    span: usize,
    font: Rc<FontEntry>,
    size: f32,
    glyphs: std::ops::Range<usize>,
    /// Sum of the glyph advances (unscaled).
    natural: f32,
    /// Signed advance along the inline axis (negative: the character
    /// progresses toward −inline, i.e. right-to-left).
    adv: f32,
    orient: Orient,
    seg: usize,
    tab: bool,
    /// Alignment point in (inline, block) coordinates.
    inline: f32,
    perp: f32,
    /// Final alignment point and rotation (degrees) in user space.
    x: f32,
    y: f32,
    angle: f32,
    /// `lengthAdjust="spacingAndGlyphs"` stretch along the inline axis.
    scale: f32,
    hidden: bool,
    chunk: bool,
    /// On a `method="stretch"` path: (path span, distance along the
    /// path of the cluster's start edge).
    warp: Option<(usize, f32)>,
}

#[derive(Clone, Copy, Debug, Default)]
struct CharAttr {
    x: Option<f32>,
    y: Option<f32>,
    dx: f32,
    dy: f32,
    rotate: Option<f32>,
}

#[derive(Debug)]
struct Segment {
    start: usize,
    end: usize,
    /// First segment of a line box.
    line_start: bool,
    breaks: u16,
    /// The `<textPath>` span the segment lies on.
    path: Option<usize>,
    /// The segment directly follows this `<textPath>` on the same line.
    after_path: Option<usize>,
    /// Block-axis offset of the line box.
    block: f32,
    clusters: std::ops::Range<usize>,
}

struct Layout {
    spans: Vec<Span>,
    chars: Vec<Ch>,
    decos: Vec<Decoration>,
    attrs: Vec<CharAttr>,
    segments: Vec<Segment>,
    clusters: Vec<Cluster>,
    glyphs: Vec<GlyphRef>,
    mode: WritingMode,
}

/// The properties a shaping run is uniform in.
#[derive(Clone, Debug)]
struct RunKey {
    font: Rc<FontEntry>,
    size: f32,
    level: u8,
    orient: Orient,
    /// `letter-spacing` is non-zero.
    spaced: bool,
    /// The font's own small capitals are requested.
    smcp: bool,
    /// Small capitals synthesised from reduced capitals.
    synth_caps: bool,
}

impl PartialEq for RunKey {
    fn eq(&self, o: &Self) -> bool {
        Rc::ptr_eq(&self.font, &o.font)
            && self.size == o.size
            && self.level == o.level
            && self.orient == o.orient
            && self.spaced == o.spaced
            && self.smcp == o.smcp
            && self.synth_caps == o.synth_caps
    }
}

/// Characters set upright in a vertical line under
/// `text-orientation: mixed`: the scripts written vertically (CJK
/// ideographs, kana, Hangul, Yi, their punctuation and full-width
/// forms) plus pictographs.
fn is_upright_in_vertical(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF
        | 0x2E80..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7FF
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE1F
        | 0xFE30..=0xFE6F
        | 0xFF01..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1B000..=0x1B2FF
        | 0x1F000..=0x1FAFF
        | 0x20000..=0x3FFFD)
}

/// Closing punctuation and marks a line may not begin with (the
/// line-start prohibition of the scripts written without spaces).
fn is_line_start_forbidden(c: char) -> bool {
    matches!(
        c,
        '\u{3001}'
            | '\u{3002}'
            | '\u{FF0C}'
            | '\u{FF0E}'
            | '\u{FF01}'
            | '\u{FF1F}'
            | '\u{FF09}'
            | '\u{3009}'
            | '\u{300B}'
            | '\u{300D}'
            | '\u{300F}'
            | '\u{3011}'
            | '\u{3015}'
            | '\u{30FC}'
            | '\u{3005}'
            | '\u{FF1A}'
            | '\u{FF1B}'
    )
}

fn is_word_separator(c: char) -> bool {
    matches!(c, ' ' | '\u{00A0}' | '\u{1361}')
}

/// Group a shaped run into typographic characters: pairs of
/// (character range, glyph range) in logical order. When the shaper
/// returns one glyph per character the mapping is the identity;
/// otherwise characters are matched against the glyph they shape to on
/// their own, and each stretch that differs (a ligature, a composed
/// mark, a decomposition) is found by shaping short windows.
fn map_clusters(
    chars: &[char],
    glyphs: &[PositionedGlyph],
    font: &FontEntry,
    size: f32,
    features: Option<&[[u8; 4]]>,
) -> Vec<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let (n, m) = (chars.len(), glyphs.len());
    if n == m {
        return (0..n).map(|i| (i..i + 1, i..i + 1)).collect();
    }
    let mut out = Vec::with_capacity(n.min(m));
    let (mut i, mut g) = (0usize, 0usize);
    const WINDOW: usize = 8;
    while i < n {
        // Remaining counts equal: identity from here on.
        if n - i == m - g {
            out.extend((0..n - i).map(|k| (i + k..i + k + 1, g + k..g + k + 1)));
            return out;
        }
        if g < m && font.isolated_glyph(chars[i], features) == Some(glyphs[g].glyph_id) {
            out.push((i..i + 1, g..g + 1));
            i += 1;
            g += 1;
            continue;
        }
        // Find the shortest window of characters whose shaping equals a
        // prefix of the remaining glyphs and after which the two
        // sequences line up again.
        let mut found = None;
        'search: for len in 1..=WINDOW.min(n - i) {
            let text: String = chars[i..i + len].iter().collect();
            let shaped = font.shape(&text, size, features);
            let q = shaped.len();
            if g + q > m
                || !shaped
                    .iter()
                    .zip(&glyphs[g..g + q])
                    .all(|(a, b)| a.glyph_id == b.glyph_id)
            {
                continue;
            }
            let aligned = match (i + len < n, g + q < m) {
                (false, false) => true,
                (true, true) => {
                    n - (i + len) == m - (g + q)
                        || font.isolated_glyph(chars[i + len], features)
                            == Some(glyphs[g + q].glyph_id)
                }
                _ => false,
            };
            if aligned {
                found = Some((len, q));
                break 'search;
            }
        }
        match found {
            Some((len, q)) => {
                out.push((i..i + len, g..g + q));
                i += len;
                g += q;
            }
            None => {
                // No local alignment: keep the rest together.
                out.push((i..n, g..m));
                return out;
            }
        }
    }
    // Glyphs left over with no character: attach to the last cluster.
    if g < m
        && let Some(last) = out.last_mut()
    {
        last.1.end = m;
    }
    out
}

impl Layout {
    fn vertical(&self) -> bool {
        self.mode.is_vertical()
    }

    /// Record every span's inclusive character range (its subtree).
    fn compute_span_ranges(&mut self) {
        for (i, ch) in self.chars.iter().enumerate() {
            let mut s = Some(ch.span as usize);
            while let Some(idx) = s {
                let span = &mut self.spans[idx];
                span.first = span.first.min(i);
                span.last = span.last.max(i);
                s = span.parent;
            }
        }
    }

    /// §11.5 "resolve character positioning": spread the `x` / `y` /
    /// `dx` / `dy` / `rotate` lists over the addressable characters,
    /// spans in document order so descendants override ancestors.
    fn resolve_char_attrs(&mut self) {
        let mut attrs = vec![CharAttr::default(); self.chars.len()];
        let vertical = self.vertical();
        for span in &self.spans {
            if span.first > span.last
                || (span.xs.is_empty()
                    && span.ys.is_empty()
                    && span.dxs.is_empty()
                    && span.dys.is_empty()
                    && span.rotates.is_empty())
            {
                continue;
            }
            let mut i = 0usize;
            for j in span.first..=span.last {
                if self.chars[j].ctrl {
                    continue;
                }
                let a = &mut attrs[j];
                if let Some(&v) = span.xs.get(i) {
                    a.x = Some(v);
                }
                if let Some(&v) = span.ys.get(i) {
                    a.y = Some(v);
                }
                if let Some(&v) = span.dxs.get(i) {
                    a.dx = v;
                }
                if let Some(&v) = span.dys.get(i) {
                    a.dy = v;
                }
                // The last `rotate` value applies to every remaining
                // character of the element (§11.2.2).
                if let Some(&v) = span.rotates.get(i).or(span.rotates.last()) {
                    a.rotate = Some(v);
                }
                i += 1;
            }
        }
        // On a path only the along-the-path coordinate is absolute:
        // `y` is ignored for horizontal text, `x` for vertical.
        for (a, ch) in attrs.iter_mut().zip(&self.chars) {
            if self.spans[ch.span as usize].path_owner.is_some() {
                if vertical {
                    a.x = None;
                } else {
                    a.y = None;
                }
            }
        }
        self.attrs = attrs;
    }

    /// Split the characters into segments: a new one at every forced
    /// line break and wherever text enters or leaves a `<textPath>`.
    fn build_segments(&mut self) {
        let mut prev_owner: Option<usize> = None;
        for (i, ch) in self.chars.iter().enumerate() {
            let owner = self.spans[ch.span as usize].path_owner;
            let fresh = i == 0 || ch.breaks > 0 || owner != prev_owner;
            if fresh {
                let line_start = i == 0 || ch.breaks > 0;
                self.segments.push(Segment {
                    start: i,
                    end: i + 1,
                    line_start,
                    breaks: ch.breaks,
                    path: owner,
                    after_path: if !line_start && owner.is_none() {
                        prev_owner
                    } else {
                        None
                    },
                    block: 0.0,
                    clusters: 0..0,
                });
            } else if let Some(last) = self.segments.last_mut() {
                last.end = i + 1;
            }
            prev_owner = owner;
        }
        // Stack the line boxes (§11.6.1): each forced break advances
        // the block position by the line height.
        let root_lh = self.spans[0].line_height;
        let mut block = 0.0f32;
        for s in 0..self.segments.len() {
            if self.segments[s].line_start && self.segments[s].breaks > 0 {
                // Tallest line height among the new line's content.
                let mut lh = 0.0f32;
                for seg in &self.segments[s..] {
                    if seg.start != self.segments[s].start && seg.line_start {
                        break;
                    }
                    for ch in &self.chars[seg.start..seg.end] {
                        lh = lh.max(self.spans[ch.span as usize].line_height);
                    }
                }
                block += root_lh * (self.segments[s].breaks - 1) as f32 + lh;
            }
            self.segments[s].block = block;
        }
    }

    /// Resolved embedding levels of one segment (UAX #9), after the L1
    /// trailing-whitespace reset.
    fn bidi_levels(&self, start: usize, end: usize) -> Vec<u8> {
        let base = u8::from(self.spans[0].state.text.direction == Direction::Rtl);
        let slice = &self.chars[start..end];
        // Left-to-right scripts under an LTR base need no resolution.
        if base == 0 && slice.iter().all(|c| !c.ctrl && (c.c as u32) < 0x0590) {
            return vec![0; slice.len()];
        }
        let cs: Vec<char> = slice.iter().map(|c| c.c).collect();
        let classes: Vec<_> = cs.iter().copied().map(oxideav_scribe::bidi_class).collect();
        let p = oxideav_scribe::process_paragraph_classes_with_brackets(&classes, &cs, Some(base));
        let mut levels = p.levels.clone();
        oxideav_scribe::reset_trailing_levels(&p.classes, &mut levels, p.paragraph_level);
        levels
    }

    fn orient_of(&self, ch: &Ch) -> Orient {
        match self.mode {
            WritingMode::HorizontalTb => Orient::Horizontal,
            WritingMode::SidewaysRl | WritingMode::SidewaysLr => Orient::Sideways,
            _ => match self.spans[ch.span as usize].state.text.text_orientation {
                TextOrientation::Upright => Orient::Upright,
                TextOrientation::Sideways => Orient::Sideways,
                _ if is_upright_in_vertical(ch.c) => Orient::Upright,
                _ => Orient::Sideways,
            },
        }
    }

    /// Shape one segment into clusters with their CSS inline positions.
    fn shape_segment(&mut self, seg_idx: usize) {
        let (start, end) = (self.segments[seg_idx].start, self.segments[seg_idx].end);
        let levels = self.bidi_levels(start, end);
        let cl_start = self.clusters.len();
        let mut i = start;
        while i < end {
            let Some(key) = self.run_key(i, levels[i - start]) else {
                i += 1;
                continue;
            };
            let tab = self.chars[i].c == '\t';
            let mut j = i + 1;
            while j < end
                && !tab
                && self.chars[j].c != '\t'
                && self.run_key(j, levels[j - start]).as_ref() == Some(&key)
            {
                j += 1;
            }
            self.shape_run(i, j, &key, seg_idx);
            i = j;
        }
        let cl_end = self.clusters.len();
        self.segments[seg_idx].clusters = cl_start..cl_end;

        // Visual order (UAX #9 L2), then run the pen along the line.
        let mut order: Vec<usize> = (cl_start..cl_end).collect();
        if levels.iter().any(|&l| l != 0) {
            let visual = oxideav_scribe::reorder_line(&levels);
            let mut vis_pos = vec![0usize; levels.len()];
            for (pos, &logical) in visual.iter().enumerate() {
                if let Some(slot) = vis_pos.get_mut(logical) {
                    *slot = pos;
                }
            }
            order.sort_by_key(|&ci| {
                let c = &self.clusters[ci];
                (c.first..c.end)
                    .map(|k| vis_pos[k - start])
                    .min()
                    .unwrap_or(0)
            });
        }
        let flip = self.mode == WritingMode::SidewaysLr;
        let mut pen = 0.0f32;
        for ci in order {
            let c = &mut self.clusters[ci];
            let rtl = c.adv < 0.0;
            let mut a = c.adv.abs();
            if c.tab {
                // CSS Text `tab-size: 8`: advance to the next multiple
                // of eight space widths.
                let stop = (c.natural * 8.0).max(f32::EPSILON);
                a = ((pen / stop).floor() + 1.0) * stop - pen;
            }
            let (mut inline, mut adv) = if rtl { (pen + a, -a) } else { (pen, a) };
            pen += a;
            if flip {
                inline = -inline;
                adv = -adv;
            }
            c.inline = inline;
            c.adv = adv;
        }
    }

    /// What must stay constant across a shaping run; `None` for a
    /// character that shapes to nothing (a bidi control, or no font).
    fn run_key(&self, idx: usize, level: u8) -> Option<RunKey> {
        let ch = self.chars[idx];
        if ch.ctrl {
            return None;
        }
        let span = &self.spans[ch.span as usize];
        let font = span.font.clone()?;
        let small_caps = span.state.text.small_caps;
        // A face without the small-capitals feature gets them
        // synthesised: lower-case letters as reduced capitals.
        let synth_caps = small_caps && !font.has_smcp && ch.c.is_lowercase();
        Some(RunKey {
            size: if synth_caps {
                span.font_size * 0.8
            } else {
                span.font_size
            },
            level,
            orient: self.orient_of(&ch),
            spaced: span.letter_spacing != 0.0,
            smcp: small_caps && font.has_smcp,
            synth_caps,
            font,
        })
    }

    fn shape_run(&mut self, i: usize, j: usize, key: &RunKey, seg: usize) {
        let (font, size, orient) = (&key.font, key.size, key.orient);
        let rtl = key.level & 1 == 1;
        let run: Vec<char> = self.chars[i..j]
            .iter()
            .map(|c| match c.c {
                '\t' => ' ',
                // UAX #9 L4: a character with a mirrored counterpart is
                // drawn mirrored in a right-to-left run.
                ch if rtl => oxideav_scribe::mirrored_glyph(ch).unwrap_or(ch),
                ch if key.synth_caps => {
                    let mut up = ch.to_uppercase();
                    match (up.next(), up.next()) {
                        (Some(u), None) => u,
                        _ => ch,
                    }
                }
                ch => ch,
            })
            .collect();
        let text: String = run.iter().collect();
        // §11.10.2.8: optional ligatures are not formed when
        // `letter-spacing` separates the characters.
        let plain = !key.spaced && !key.smcp;
        let mut feature_list: Vec<[u8; 4]> = Vec::new();
        if !plain {
            feature_list.push(*b"ccmp");
            if !key.spaced {
                feature_list.extend([*b"liga", *b"calt"]);
            }
            if key.smcp {
                feature_list.push(*b"smcp");
            }
        }
        let features = (!plain).then_some(feature_list.as_slice());
        let shaped = font.shape(&text, size, features);
        let font = &key.font;
        for (cr, gr) in map_clusters(&run, &shaped, font, size, features) {
            let first = i + cr.start;
            let end = i + cr.end;
            let ch = self.chars[first];
            let span_idx = ch.span as usize;
            let span = &self.spans[span_idx];
            let g_start = self.glyphs.len();
            let mut pen = 0.0f32;
            for g in &shaped[gr] {
                self.glyphs.push(GlyphRef {
                    face_idx: g.face_idx,
                    glyph_id: g.glyph_id,
                    x: pen + g.x_offset,
                    y: g.y_offset,
                });
                pen += g.x_advance;
            }
            let mut spacing = span.letter_spacing;
            if end - first == 1 && is_word_separator(ch.c) {
                spacing += span.word_spacing;
            }
            let extent = match orient {
                Orient::Upright => size,
                _ => pen,
            };
            let adv = (extent + spacing).max(0.0);
            self.clusters.push(Cluster {
                first,
                end,
                span: span_idx,
                font: font.clone(),
                size,
                glyphs: g_start..self.glyphs.len(),
                natural: pen,
                adv: if rtl { -adv } else { adv },
                orient,
                seg,
                tab: ch.c == '\t',
                inline: 0.0,
                perp: 0.0,
                x: 0.0,
                y: 0.0,
                angle: 0.0,
                scale: 1.0,
                hidden: false,
                chunk: false,
                warp: None,
            });
        }
    }

    /// §11.4.1 `inline-size`: break the single line into lines no wider
    /// than `limit`. Lines break after white space (which then hangs —
    /// it is not rendered at the line end) and between characters of
    /// the scripts written without spaces; a word wider than the limit
    /// overflows rather than being split. Records the breaks on the
    /// characters; returns whether any was made (the caller reshapes).
    fn wrap(&mut self, limit: f32) -> bool {
        let breakable_alone = |c: char| is_upright_in_vertical(c) && !is_line_start_forbidden(c);
        let mut changed = false;
        for seg in &self.segments {
            let range = seg.clusters.clone();
            let mut line_start = range.start;
            let mut last_break: Option<usize> = None;
            let mut width = 0.0f32;
            for k in range.clone() {
                let ch = self.chars[self.clusters[k].first].c;
                let space = matches!(ch, ' ' | '\t' | '\u{200B}');
                if k > line_start {
                    let prev = self.chars[self.clusters[k - 1].first].c;
                    let after_space = matches!(prev, ' ' | '\t' | '\u{200B}');
                    if !space
                        && (after_space
                            || ((breakable_alone(ch) || is_upright_in_vertical(prev))
                                && !is_line_start_forbidden(ch)))
                    {
                        last_break = Some(k);
                    }
                }
                let adv = self.clusters[k].adv.abs();
                if !space
                    && width + adv > limit + 1e-3
                    && let Some(b) = last_break.filter(|b| *b > line_start)
                {
                    let first_char = self.clusters[b].first;
                    self.chars[first_char].breaks = self.chars[first_char].breaks.saturating_add(1);
                    // The white space the line ends with hangs.
                    let mut t = b;
                    while t > line_start {
                        let c = &self.clusters[t - 1];
                        if !matches!(self.chars[c.first].c, ' ' | '\t' | '\u{200B}') {
                            break;
                        }
                        for hung in &mut self.chars[c.first..c.end] {
                            hung.ctrl = true;
                        }
                        t -= 1;
                    }
                    width = self.clusters[b..k].iter().map(|c| c.adv.abs()).sum();
                    line_start = b;
                    last_break = None;
                    changed = true;
                }
                width += adv;
            }
        }
        changed
    }

    /// The cluster index range whose characters lie in `[first, last]`.
    fn clusters_in(&self, first: usize, last: usize) -> std::ops::Range<usize> {
        let lo = self.clusters.partition_point(|c| c.first < first);
        let hi = self.clusters.partition_point(|c| c.first <= last);
        lo..hi
    }

    /// §11.5 "apply textLength": children before parents.
    fn apply_text_length(&mut self, children: &[Vec<usize>], idx: usize) {
        for &c in &children[idx] {
            self.apply_text_length(children, c);
        }
        let span = &self.spans[idx];
        let Some(spec) = span.text_length else {
            return;
        };
        if span.first > span.last {
            return;
        }
        let range = self.clusters_in(span.first, span.last);
        if range.is_empty() {
            return;
        }
        // No adjustment across a forced line break or a path boundary.
        let seg = self.clusters[range.start].seg;
        if self.clusters[range.clone()].iter().any(|c| c.seg != seg) {
            return;
        }
        let (mut a, mut b) = (f32::INFINITY, f32::NEG_INFINITY);
        for c in &self.clusters[range.clone()] {
            a = a.min(c.inline).min(c.inline + c.adv);
            b = b.max(c.inline).max(c.inline + c.adv);
        }
        let extent = b - a;
        if !extent.is_finite() {
            return;
        }
        let delta = spec.target - extent;
        // Positive inline for LTR, negative for RTL (and for the
        // bottom-to-top `sideways-lr`).
        let dir = if (span.state.text.direction == Direction::Rtl)
            != (self.mode == WritingMode::SidewaysLr)
        {
            -1.0
        } else {
            1.0
        };
        if spec.adjust == TextLengthAdjust::SpacingAndGlyphs {
            if extent <= f32::EPSILON {
                return;
            }
            let s = spec.target / extent;
            // Scale about the edge the text starts from.
            let origin = if dir > 0.0 { a } else { b };
            for c in &mut self.clusters[range.clone()] {
                c.inline = origin + (c.inline - origin) * s;
                c.adv *= s;
                c.scale *= s;
            }
        } else {
            // A descendant with its own `textLength` moves as one unit.
            let unit_of = |this: &Self, c: &Cluster| -> Option<usize> {
                let mut s = c.span;
                let mut resolved = None;
                while s != idx {
                    if this.spans[s].text_length.is_some() {
                        resolved = Some(s);
                    }
                    s = this.spans[s].parent?;
                }
                resolved
            };
            let units: Vec<Option<usize>> = self.clusters[range.clone()]
                .iter()
                .map(|c| unit_of(self, c))
                .collect();
            let mut n = 0usize;
            let mut prev: Option<usize> = None;
            for u in &units {
                if u.is_none() || *u != prev {
                    n += 1;
                }
                prev = *u;
            }
            if n < 2 {
                return;
            }
            let step = delta / (n - 1) as f32;
            // Walk in visual (inline) order so the gaps open between
            // neighbours on the line, whatever the logical order.
            let mut order: Vec<usize> = range.clone().collect();
            order.sort_by(|&p, &q| {
                let key = |k: usize| {
                    let c = &self.clusters[k];
                    c.inline.min(c.inline + c.adv) * dir
                };
                key(p)
                    .partial_cmp(&key(q))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let mut shift = 0.0f32;
            let mut prev: Option<Option<usize>> = None;
            for k in order {
                let u = units[k - range.start];
                if let Some(p) = prev
                    && (u.is_none() || u != p)
                {
                    shift += step;
                }
                prev = Some(u);
                self.clusters[k].inline += shift * dir;
            }
        }
        // What follows on the same line keeps its distance to the
        // adjusted element.
        let seg_end = self.segments[seg].clusters.end;
        for c in &mut self.clusters[range.end..seg_end] {
            c.inline += delta * dir;
        }
    }

    /// §11.5 positioning: `dx` / `dy`, `textLength`, `x` / `y`,
    /// `text-anchor`, then `<textPath>`.
    fn position(&mut self) {
        let vertical = self.vertical();
        let block_sign = match self.mode {
            WritingMode::VerticalRl | WritingMode::SidewaysRl => -1.0,
            _ => 1.0,
        };
        // Cumulative relative shifts in (inline, block) axes.
        let mut cum: Vec<(f32, f32)> = Vec::with_capacity(self.attrs.len());
        let mut acc = (0.0f32, 0.0f32);
        for a in &self.attrs {
            if vertical {
                acc.0 += a.dy;
                acc.1 += a.dx;
            } else {
                acc.0 += a.dx;
                acc.1 += a.dy;
            }
            cum.push(acc);
        }
        for c in &mut self.clusters {
            let seg = &self.segments[c.seg];
            let (di, dp) = cum[c.first];
            c.inline += di;
            c.perp = seg.block * block_sign + self.spans[c.span].perp + dp;
        }

        // textLength.
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); self.spans.len()];
        for (i, s) in self.spans.iter().enumerate() {
            if let Some(p) = s.parent {
                children[p].push(i);
            }
        }
        if self.spans.iter().any(|s| s.text_length.is_some()) {
            self.apply_text_length(&children, 0);
        }

        // Absolute x / y and chunk starts.
        let mut shift = (0.0f32, 0.0f32);
        let mut carry_chunk = false;
        let mut prev_seg = usize::MAX;
        for k in 0..self.clusters.len() {
            let (first, end, seg_idx) = {
                let c = &self.clusters[k];
                (c.first, c.end, c.seg)
            };
            let seg = &self.segments[seg_idx];
            let seg_first = seg_idx != prev_seg;
            if seg_first && (seg.path.is_some() || seg.after_path.is_some()) {
                // Text on a path is positioned along the path, text
                // after it from the path's end: earlier absolute
                // positions do not carry over.
                shift = (0.0, 0.0);
            }
            let a = self.attrs[first];
            let (abs_inline, abs_perp) = if vertical { (a.y, a.x) } else { (a.x, a.y) };
            let c = &mut self.clusters[k];
            // §11.6.2: "Absolute repositioning … is done before
            // relative repositioning" — the character's own `dx` / `dy`
            // still applies on top of its absolute coordinate.
            let (rel_inline, rel_perp) = if vertical { (a.dy, a.dx) } else { (a.dx, a.dy) };
            if let Some(v) = abs_inline {
                shift.0 = v + rel_inline - c.inline;
            }
            if let Some(v) = abs_perp {
                // The absolute coordinate names the text position; the
                // span's baseline displacement stays on top of it.
                shift.1 = v + rel_perp - (c.perp - self.spans[c.span].perp);
            }
            c.inline += shift.0;
            c.perp += shift.1;
            let explicit = abs_inline.is_some() || abs_perp.is_some() || carry_chunk;
            c.chunk = explicit || (seg_first && seg.after_path.is_none());
            // An absolute position inside a typographic character
            // starts the chunk at the next one.
            carry_chunk = (first + 1..end).any(|m| {
                let am = self.attrs[m];
                am.x.is_some() || am.y.is_some()
            });
            c.angle = a.rotate.unwrap_or(0.0);
            prev_seg = seg_idx;
        }

        // text-anchor per anchored chunk.
        let n = self.clusters.len();
        let mut i = 0usize;
        while i < n {
            let mut j = i + 1;
            while j < n && !self.clusters[j].chunk && self.clusters[j].seg == self.clusters[i].seg {
                j += 1;
            }
            // Text continuing after a path is placed from the path's
            // end point, not anchored.
            if self.clusters[i].chunk {
                let (mut a, mut b) = (f32::INFINITY, f32::NEG_INFINITY);
                for c in &self.clusters[i..j] {
                    a = a.min(c.inline).min(c.inline + c.adv);
                    b = b.max(c.inline).max(c.inline + c.adv);
                }
                let span = &self.spans[self.clusters[i].span];
                let rtl = (span.state.text.direction == Direction::Rtl)
                    != (self.mode == WritingMode::SidewaysLr);
                let origin = self.clusters[i].inline;
                let delta = match (span.state.text_anchor, rtl) {
                    (TextAnchor::Start, false) | (TextAnchor::End, true) => origin - a,
                    (TextAnchor::Start, true) | (TextAnchor::End, false) => origin - b,
                    (TextAnchor::Middle, _) => origin - (a + b) * 0.5,
                };
                if delta != 0.0 && delta.is_finite() {
                    for c in &mut self.clusters[i..j] {
                        c.inline += delta;
                    }
                }
            }
            i = j;
        }

        // Position on path; text after a path continues from its end.
        let mut after: Option<(f32, f32)> = None;
        let mut prev_seg = usize::MAX;
        for k in 0..n {
            let seg_idx = self.clusters[k].seg;
            let seg_first = seg_idx != prev_seg;
            prev_seg = seg_idx;
            let (seg_path, seg_after) = {
                let s = &self.segments[seg_idx];
                (s.path, s.after_path)
            };
            if let Some(p) = seg_path {
                after = None;
                let info = self.spans[p].text_path.clone();
                let span = &self.spans[self.clusters[k].span];
                let anchor = span.state.text_anchor;
                let rtl = span.state.text.direction == Direction::Rtl;
                let c = &mut self.clusters[k];
                let Some((info, sampler)) = info
                    .as_ref()
                    .and_then(|i| i.sampler.as_ref().map(|s| (i, s)))
                else {
                    c.hidden = true;
                    continue;
                };
                let length = sampler.length();
                let offset = info.start_offset;
                let mut mid = c.inline + c.adv * 0.5 + offset;
                if sampler.is_closed() {
                    // §11.8.2: one circuit of a closed path, from the
                    // anchor position.
                    let rel = mid - offset;
                    let out = match (anchor, rtl) {
                        (TextAnchor::Start, false) | (TextAnchor::End, true) => {
                            rel < 0.0 || rel > length
                        }
                        (TextAnchor::Middle, _) => rel < -length * 0.5 || rel > length * 0.5,
                        (TextAnchor::Start, true) | (TextAnchor::End, false) => {
                            rel < -length || rel > 0.0
                        }
                    };
                    if out {
                        c.hidden = true;
                        continue;
                    }
                    mid = mid.rem_euclid(length);
                } else if mid < 0.0 || mid > length {
                    c.hidden = true;
                    continue;
                }
                let (point, t) = sampler.sample(mid);
                let half = c.adv * 0.5;
                if vertical {
                    // n = t − 90°.
                    let nrm = (t.1, -t.0);
                    c.x = point.x - half * t.0 + c.perp * nrm.0;
                    c.y = point.y - half * t.1 + c.perp * nrm.1;
                    c.angle += t.1.atan2(t.0).to_degrees() - 90.0;
                } else {
                    // n = t + 90°.
                    let nrm = (-t.1, t.0);
                    c.x = point.x - half * t.0 + c.perp * nrm.0;
                    c.y = point.y - half * t.1 + c.perp * nrm.1;
                    c.angle += t.1.atan2(t.0).to_degrees();
                }
                if info.stretch && !vertical {
                    c.warp = Some((p, mid - c.adv.abs() * 0.5));
                }
            } else {
                let c = &mut self.clusters[k];
                let (x, y) = if vertical {
                    (c.perp, c.inline)
                } else {
                    (c.inline, c.perp)
                };
                c.x = x;
                c.y = y;
                if seg_first {
                    after = match seg_after {
                        Some(p) if !c.chunk => {
                            let end = self.spans[p]
                                .text_path
                                .as_ref()
                                .and_then(|i| i.sampler.as_ref())
                                .map(|s| s.end_point())
                                .unwrap_or_default();
                            Some((end.x - x, end.y - y))
                        }
                        _ => None,
                    };
                } else if c.chunk {
                    after = None;
                }
                if let Some((ox, oy)) = after {
                    c.x += ox;
                    c.y += oy;
                }
            }
        }
    }

    /// The frame of a cluster's line position: origin at the alignment
    /// point, x along the (possibly rotated) inline direction for
    /// horizontal text, unrotated axes plus `rotate` for vertical.
    fn line_frame(&self, c: &Cluster) -> Transform2D {
        let t = Transform2D::translate(c.x, c.y);
        if c.angle != 0.0 {
            t.compose(&Transform2D::rotate(c.angle.to_radians()))
        } else {
            t
        }
    }

    /// Maps glyph space (origin on the baseline at the cluster's left
    /// edge, y down) to user space.
    fn glyph_frame(&self, c: &Cluster) -> Transform2D {
        let line = self.line_frame(c);
        let u = &c.font.unit;
        let width = c.adv.abs() / c.scale;
        let centre = (u.ascent - u.descent) * c.size * 0.5;
        let local = match c.orient {
            Orient::Horizontal => {
                let back = if c.adv < 0.0 { -width } else { 0.0 };
                scale_x(c.scale).compose(&Transform2D::translate(back, 0.0))
            }
            Orient::Sideways if self.mode == WritingMode::SidewaysLr => {
                // Bottom-to-top line: glyphs turned counter-clockwise.
                let back = if c.adv > 0.0 { -width } else { 0.0 };
                Transform2D::rotate(-std::f32::consts::FRAC_PI_2)
                    .compose(&scale_x(c.scale))
                    .compose(&Transform2D::translate(back, centre))
            }
            Orient::Sideways => {
                let back = if c.adv < 0.0 { -width } else { 0.0 };
                Transform2D::rotate(std::f32::consts::FRAC_PI_2)
                    .compose(&scale_x(c.scale))
                    .compose(&Transform2D::translate(back, centre))
            }
            Orient::Upright => {
                // The em box hangs from the line position, centred on
                // the line.
                let em_ascent = c.size * u.ascent / (u.ascent + u.descent).max(f32::EPSILON);
                Transform2D::scale(1.0, c.scale)
                    .compose(&Transform2D::translate(-c.natural * 0.5, em_ascent))
            }
        };
        line.compose(&local)
    }

    /// The glyph cell of a cluster in glyph space (§8.10: advance ×
    /// em box).
    fn cell(&self, c: &Cluster) -> BBox {
        let u = &c.font.unit;
        match c.orient {
            Orient::Upright => {
                let em_ascent = c.size * u.ascent / (u.ascent + u.descent).max(f32::EPSILON);
                BBox::new(0.0, -em_ascent, c.natural, c.size)
            }
            _ => BBox::new(
                0.0,
                -u.ascent * c.size,
                (c.adv.abs() / c.scale).max(c.natural),
                (u.ascent + u.descent) * c.size,
            ),
        }
    }

    /// Bounding box of the whole text element: the union of the glyph
    /// cells of every rendered typographic character.
    fn bbox(&self) -> Option<BBox> {
        let mut out: Option<BBox> = None;
        for c in self.clusters.iter().filter(|c| !c.hidden) {
            let b = self.cell(c).transformed(&self.glyph_frame(c));
            out = Some(match out {
                Some(o) => o.union(&b),
                None => b,
            });
        }
        out
    }
}

fn scale_x(s: f32) -> Transform2D {
    if s == 1.0 {
        Transform2D::identity()
    } else {
        Transform2D::scale(s, 1.0)
    }
}

// ───────────────────────── painting ─────────────────────────

/// Append `src` to `dst` with every coordinate mapped through `map`.
/// Straight segments longer than `max_line` are subdivided first (a
/// non-affine `map` bends them); `f32::INFINITY` leaves them whole.
fn append_mapped(dst: &mut Path, src: &Path, max_line: f32, map: &dyn Fn(Point) -> Point) {
    let mut cur = Point::default();
    let mut start = Point::default();
    let line = |dst: &mut Path, from: Point, to: Point| {
        let len = ((to.x - from.x).powi(2) + (to.y - from.y).powi(2)).sqrt();
        let n = if max_line.is_finite() && len > max_line {
            ((len / max_line).ceil() as usize).min(256)
        } else {
            1
        };
        for i in 1..=n {
            let t = i as f32 / n as f32;
            dst.commands.push(PathCommand::LineTo(map(Point::new(
                from.x + (to.x - from.x) * t,
                from.y + (to.y - from.y) * t,
            ))));
        }
    };
    for cmd in &src.commands {
        match *cmd {
            PathCommand::MoveTo(p) => {
                dst.commands.push(PathCommand::MoveTo(map(p)));
                cur = p;
                start = p;
            }
            PathCommand::LineTo(p) => {
                line(dst, cur, p);
                cur = p;
            }
            PathCommand::QuadCurveTo { control, end } => {
                dst.commands.push(PathCommand::QuadCurveTo {
                    control: map(control),
                    end: map(end),
                });
                cur = end;
            }
            PathCommand::CubicCurveTo { c1, c2, end } => {
                dst.commands.push(PathCommand::CubicCurveTo {
                    c1: map(c1),
                    c2: map(c2),
                    end: map(end),
                });
                cur = end;
            }
            // Glyph outlines carry no arcs; keep the endpoint.
            PathCommand::ArcTo { end, .. } => {
                line(dst, cur, end);
                cur = end;
            }
            PathCommand::Close => {
                if max_line.is_finite() {
                    line(dst, cur, start);
                }
                dst.commands.push(PathCommand::Close);
                cur = start;
            }
        }
    }
}

/// Append one decoration line spanning `[lo, hi]` along the inline
/// axis, centred on block position `pos`, `t` thick, in the given
/// style. `at(inline, block)` maps to user space.
fn decoration_bar(
    path: &mut Path,
    style: DecorationStyle,
    lo: f32,
    hi: f32,
    pos: f32,
    t: f32,
    at: &dyn Fn(f32, f32) -> Point,
) {
    if hi - lo <= 0.0 || !(hi - lo).is_finite() {
        return;
    }
    let quad = |path: &mut Path, a: f32, b: f32, centre: f32| {
        let (p0, p1) = (centre - t * 0.5, centre + t * 0.5);
        path.move_to(at(a, p0));
        path.line_to(at(b, p0));
        path.line_to(at(b, p1));
        path.line_to(at(a, p1));
        path.close();
    };
    match style {
        DecorationStyle::Solid => quad(path, lo, hi, pos),
        DecorationStyle::Double => {
            // Two lines one thickness apart.
            quad(path, lo, hi, pos - t);
            quad(path, lo, hi, pos + t);
        }
        DecorationStyle::Dashed | DecorationStyle::Dotted => {
            // Dashes three thicknesses long, dots one; the pattern is
            // phased on the inline coordinate so adjoining bars agree.
            let (on, period) = if style == DecorationStyle::Dashed {
                (3.0 * t, 5.0 * t)
            } else {
                (t, 2.0 * t)
            };
            let mut k = (lo / period).floor();
            let mut pieces = 0usize;
            while k * period < hi && pieces < 4096 {
                let a = (k * period).max(lo);
                let b = (k * period + on).min(hi);
                if b > a {
                    quad(path, a, b, pos);
                    pieces += 1;
                }
                k += 1.0;
            }
        }
        DecorationStyle::Wavy => {
            // A band following a sine of amplitude `t`, period `6t`.
            let period = 6.0 * t;
            let steps = (((hi - lo) / period * 16.0).ceil() as usize).clamp(2, 8192);
            let wave = |x: f32| pos + t * (x / period * std::f32::consts::TAU).sin();
            for i in 0..=steps {
                let x = lo + (hi - lo) * i as f32 / steps as f32;
                let p = at(x, wave(x) - t * 0.5);
                if i == 0 {
                    path.move_to(p);
                } else {
                    path.line_to(p);
                }
            }
            for i in (0..=steps).rev() {
                let x = lo + (hi - lo) * i as f32 / steps as f32;
                path.line_to(at(x, wave(x) + t * 0.5));
            }
            path.close();
        }
    }
}

/// Visit every outline of a glyph node with its accumulated transform.
fn for_each_outline(node: &Node, t: &Transform2D, f: &mut dyn FnMut(&Path, &Transform2D)) {
    match node {
        Node::Path(p) => f(&p.path, t),
        Node::Group(g) => {
            let t = t.compose(&g.transform);
            for c in &g.children {
                for_each_outline(c, &t, f);
            }
        }
        _ => {}
    }
}

/// Replace the default glyph fill with the span's paint (and outline).
fn repaint(node: &Node, fill: &Option<Paint>, outline: &Option<Stroke>) -> Node {
    match node {
        Node::Path(p) => Node::Path(PathNode {
            path: p.path.clone(),
            fill: fill.clone(),
            stroke: outline.clone(),
            fill_rule: p.fill_rule,
        }),
        Node::Group(g) => Node::Group(Group {
            children: g
                .children
                .iter()
                .map(|c| repaint(c, fill, outline))
                .collect(),
            ..g.clone()
        }),
        other => other.clone(),
    }
}

fn solid_with_opacity(c: crate::model::Rgba, opacity: f32) -> crate::model::Rgba {
    let a = (c.a as f32 / 255.0) * opacity.clamp(0.0, 1.0) * 255.0;
    crate::model::Rgba::new(c.r, c.g, c.b, a.round() as u8)
}

impl Layout {
    /// The transform of one glyph of a cluster (glyph outline space →
    /// user space), including a synthetic-oblique shear.
    fn glyph_transform(&self, c: &Cluster, frame: &Transform2D, g: &GlyphRef) -> Transform2D {
        let mut t = frame.compose(&Transform2D::translate(g.x, g.y));
        if c.font.shear != 0.0 {
            // x += shear · (height above the baseline); y is down.
            t = t.compose(&Transform2D {
                a: 1.0,
                b: 0.0,
                c: -c.font.shear,
                d: 1.0,
                e: 0.0,
                f: 0.0,
            });
        }
        t
    }

    /// `method="stretch"` (§11.8.2): bend a glyph outline along the
    /// path — every point moves to the path position at its own
    /// distance, offset along the normal by its height.
    fn warp_glyph(
        &self,
        c: &Cluster,
        g: &GlyphRef,
        outline: &Path,
        t: &Transform2D,
        dst: &mut Path,
    ) {
        let Some((path_span, start)) = c.warp else {
            return;
        };
        let Some(sampler) = self.spans[path_span]
            .text_path
            .as_ref()
            .and_then(|i| i.sampler.as_ref())
        else {
            return;
        };
        let length = sampler.length();
        let closed = sampler.is_closed();
        let shear = c.font.shear;
        let (gx, gy, scale, perp) = (g.x, g.y, c.scale, c.perp);
        let map = |p: Point| {
            let p = t.apply(p);
            let along = start + (gx + p.x - shear * p.y) * scale;
            let along = if closed {
                along.rem_euclid(length)
            } else {
                along
            };
            let (pt, tan) = sampler.sample(along);
            // Past an open path's ends, continue along the end tangent.
            let over = along - along.clamp(0.0, length);
            let h = perp + gy + p.y;
            Point::new(
                pt.x + tan.0 * over - tan.1 * h,
                pt.y + tan.1 * over + tan.0 * h,
            )
        };
        append_mapped(dst, outline, (c.size / 6.0).max(0.5), &map);
    }

    /// Scene nodes of the text decorations whose line kind is in
    /// `mask`, each decoration painted as one geometry with the paint
    /// of the element that declared it.
    fn decoration_nodes(
        &self,
        mask: u8,
        bbox: Option<BBox>,
        ctx: &mut ParseContext,
        out: &mut Vec<Node>,
    ) {
        let vertical = self.vertical();
        for (di, deco) in self.decos.iter().enumerate() {
            let lines = deco.lines & mask;
            if lines == 0 {
                continue;
            }
            let fs = deco.font_size;
            let thickness = (fs / 16.0).max(0.01);
            // Offsets of each line's centre from the declaring element's
            // alphabetic baseline along the block axis.
            let mut offsets: Vec<f32> = Vec::new();
            if vertical {
                // Vertical lines of text: the underline runs along the
                // left edge of the em box, the overline along the right.
                if lines & DECORATION_UNDERLINE != 0 {
                    offsets.push(-0.5 * fs - thickness);
                }
                if lines & DECORATION_OVERLINE != 0 {
                    offsets.push(0.5 * fs + thickness);
                }
                if lines & DECORATION_LINE_THROUGH != 0 {
                    offsets.push(0.0);
                }
            } else {
                if lines & DECORATION_UNDERLINE != 0 {
                    offsets.push(0.12 * fs);
                }
                if lines & DECORATION_OVERLINE != 0 {
                    offsets.push(-deco.unit.ascent * fs + thickness);
                }
                if lines & DECORATION_LINE_THROUGH != 0 {
                    offsets.push(-deco.unit.x_height * fs * 0.55);
                }
            }
            let mut path = Path::new();
            for offset in offsets {
                // Merge neighbouring unrotated clusters into one bar:
                // (lo, hi, block position) in user space.
                let mut open: Option<(f32, f32, f32)> = None;
                let flush = |path: &mut Path, bar: (f32, f32, f32)| {
                    let (lo, hi, pos) = bar;
                    decoration_bar(path, deco.style, lo, hi, pos, thickness, &|i, p| {
                        if vertical {
                            Point::new(p, i)
                        } else {
                            Point::new(i, p)
                        }
                    });
                };
                for c in &self.clusters {
                    if c.hidden || !self.spans[c.span].decorations.contains(&di) {
                        continue;
                    }
                    // Block-axis position of the bar relative to this
                    // cluster's alignment point.
                    let rel = offset + deco.perp - self.spans[c.span].perp;
                    let (a0, a1) = (c.adv.min(0.0), c.adv.max(0.0));
                    if c.angle == 0.0 {
                        let (inl, blk) = if vertical { (c.y, c.x) } else { (c.x, c.y) };
                        let (lo, hi, pos) = (inl + a0, inl + a1, blk + rel);
                        let eps = 0.02 * fs;
                        match open {
                            Some((olo, ohi, opos))
                                if (opos - pos).abs() < 1e-3
                                    && lo <= ohi + eps
                                    && hi >= olo - eps =>
                            {
                                open = Some((olo.min(lo), ohi.max(hi), opos));
                            }
                            other => {
                                if let Some(bar) = other {
                                    flush(&mut path, bar);
                                }
                                open = Some((lo, hi, pos));
                            }
                        }
                    } else {
                        // Rotated (per-character `rotate`, or on a
                        // path): one bar per typographic character in
                        // its own line frame.
                        if let Some(bar) = open.take() {
                            flush(&mut path, bar);
                        }
                        let frame = self.line_frame(c);
                        decoration_bar(&mut path, deco.style, a0, a1, rel, thickness, &|i, p| {
                            frame.apply(if vertical {
                                Point::new(p, i)
                            } else {
                                Point::new(i, p)
                            })
                        });
                    }
                }
                if let Some(bar) = open {
                    flush(&mut path, bar);
                }
            }
            if let Some(node) = paint_text_geometry(path, &deco.state, bbox, ctx) {
                out.push(node);
            }
        }
    }

    /// Paint the glyphs: consecutive typographic characters of one
    /// span share a paint.
    fn glyph_nodes(&self, bbox: Option<BBox>, ctx: &mut ParseContext, out: &mut Vec<Node>) {
        let n = self.clusters.len();
        let mut i = 0usize;
        while i < n {
            let span_idx = self.clusters[i].span;
            let mut j = i + 1;
            while j < n && self.clusters[j].span == span_idx {
                j += 1;
            }
            let span = &self.spans[span_idx];
            let state = &span.state;
            let hidden = state.visibility == Visibility::Hidden;
            let plain_fill = match &state.fill {
                crate::color::PaintValue::None => Some(None),
                crate::color::PaintValue::Color(c) => Some(Some(Paint::Solid(solid_with_opacity(
                    *c,
                    state.fill_opacity,
                )))),
                crate::color::PaintValue::Reference { .. } => None,
            };
            let no_stroke = matches!(state.stroke, crate::color::PaintValue::None);
            match plain_fill {
                // A plain colour and no stroke: one placement group per
                // glyph around the shared outline. Hidden text keeps
                // its geometry, unpainted (SVG 1.1 §11.5).
                Some(fill) if no_stroke || hidden => {
                    let fill = if hidden { None } else { fill };
                    for c in self.clusters[i..j].iter().filter(|c| !c.hidden) {
                        // A requested bold the face lacks: thicken
                        // opaque glyphs with an outline of the same
                        // colour.
                        let outline = match (&fill, c.font.embolden) {
                            (Some(Paint::Solid(col)), true) if col.a == 255 => {
                                Some(Stroke::solid(c.size * 0.03, *col))
                            }
                            _ => None,
                        };
                        let frame = self.glyph_frame(c);
                        for g in &self.glyphs[c.glyphs.clone()] {
                            let Some(node) = c.font.glyph(g.face_idx, g.glyph_id, c.size) else {
                                continue;
                            };
                            if c.warp.is_some() {
                                let mut path = Path::new();
                                for_each_outline(&node, &Transform2D::identity(), &mut |p, t| {
                                    self.warp_glyph(c, g, p, t, &mut path)
                                });
                                out.push(Node::Path(PathNode {
                                    path,
                                    fill: fill.clone(),
                                    stroke: outline.clone(),
                                    fill_rule: crate::model::FillRule::NonZero,
                                }));
                            } else {
                                out.push(Node::Group(Group {
                                    transform: self.glyph_transform(c, &frame, g),
                                    children: vec![repaint(&node, &fill, &outline)],
                                    ..Group::default()
                                }));
                            }
                        }
                    }
                }
                // A paint server or a stroke: flatten the span's
                // outlines into user space and paint them as one
                // geometry bound to the text element's bounding box.
                _ => {
                    let mut path = Path::new();
                    for c in self.clusters[i..j].iter().filter(|c| !c.hidden) {
                        let frame = self.glyph_frame(c);
                        for g in &self.glyphs[c.glyphs.clone()] {
                            let Some(node) = c.font.glyph(g.face_idx, g.glyph_id, c.size) else {
                                continue;
                            };
                            if c.warp.is_some() {
                                for_each_outline(&node, &Transform2D::identity(), &mut |p, t| {
                                    self.warp_glyph(c, g, p, t, &mut path)
                                });
                            } else {
                                let gt = self.glyph_transform(c, &frame, g);
                                for_each_outline(&node, &gt, &mut |p, t| {
                                    append_mapped(&mut path, p, f32::INFINITY, &|q| t.apply(q));
                                });
                            }
                        }
                    }
                    if let Some(node) = paint_text_geometry(path, state, bbox, ctx) {
                        out.push(node);
                    }
                }
            }
            i = j;
        }
    }
}

// ───────────────────────── entry points ─────────────────────────

/// Lay out a `<text>` element with an isolated cascade context (no
/// ancestor / sibling information for CSS selectors). `state` is the
/// element's own cascaded paint state. Returns the wrapping group.
pub fn parse_text_element(
    el: &Element,
    state: &PaintState,
    ctx: &mut ParseContext,
) -> Result<Option<Node>> {
    let mctx = MatchContext::root(el);
    parse_text_element_ctx(el, state, ctx, &mctx)
}

/// Lay out a `<text>` element. `state` is the element's own cascaded
/// paint state, `mctx` its position in the document for selector
/// matching. The returned group holds the painted glyphs in the text
/// element's user space (the caller applies `transform` / `opacity`).
pub(crate) fn parse_text_element_ctx(
    el: &Element,
    state: &PaintState,
    ctx: &mut ParseContext,
    mctx: &MatchContext<'_>,
) -> Result<Option<Node>> {
    let mode = state.text.writing_mode;
    let rctx = derive_child_ctx(el, mctx, &ctx.stylesheet, &ctx.resolve_ctx);
    // `inline-size` (§11.4.1): a positive length turns on wrapping.
    let inline_axis = if mode.is_vertical() {
        LengthAxis::Y
    } else {
        LengthAxis::X
    };
    let inline_size = own_prop(el, &declarations_for(mctx, &ctx.stylesheet), "inline-size")
        .and_then(|v| parse_length(v.trim()).ok())
        .map(|l| {
            l.resolve(ResolveContext {
                percentage_basis_px: rctx.percentage_basis_for(inline_axis),
                ..rctx
            })
        })
        .filter(|v| v.is_finite() && *v > 0.0);
    let caches = std::mem::take(&mut ctx.text_caches);
    let mut collector = Collector {
        ctx,
        caches,
        spans: Vec::new(),
        chars: Vec::new(),
        decos: Vec::new(),
        pending_space: None,
        at_line_start: true,
        pending_breaks: 0,
        vertical: mode.is_vertical(),
        wrapping: inline_size.is_some(),
    };
    let root = collector.open_span(el, mctx, state.clone(), &rctx, None, SpanKind::Text);
    let root_decls = declarations_for(mctx, &collector.ctx.stylesheet);
    let (open, close) = bidi_controls(
        own_prop(el, &root_decls, "unicode-bidi"),
        state.text.direction == Direction::Rtl,
    );
    if let Some(o) = open {
        collector.push_ctrl(o, root);
    }
    let walked = collector.walk(el, root, mctx, &rctx, 0);
    if open.is_some() {
        collector.push_ctrl(close, root);
    }
    let Collector {
        caches,
        spans,
        chars,
        decos,
        ..
    } = collector;
    ctx.text_caches = caches;
    walked?;

    let mut group = Group::default();
    if chars.is_empty() || spans.iter().all(|s| s.font.is_none()) {
        return Ok(Some(Node::Group(group)));
    }
    let mut layout = Layout {
        spans,
        chars,
        decos,
        attrs: Vec::new(),
        segments: Vec::new(),
        clusters: Vec::new(),
        glyphs: Vec::new(),
        mode,
    };
    // §11.4.1 auto-wrapped text: only the first `x` / `y` of the
    // `<text>` positions the content (§11.7); every other positioning
    // attribute is ignored.
    let wrap = inline_size.filter(|_| layout.spans.iter().all(|s| s.text_path.is_none()));
    if wrap.is_some() {
        for (i, s) in layout.spans.iter_mut().enumerate() {
            let keep = usize::from(i == 0);
            s.xs.truncate(keep);
            s.ys.truncate(keep);
            s.dxs.clear();
            s.dys.clear();
            s.rotates.clear();
            s.text_length = None;
        }
    }
    layout.compute_span_ranges();
    layout.resolve_char_attrs();
    layout.build_segments();
    for s in 0..layout.segments.len() {
        layout.shape_segment(s);
    }
    if let Some(limit) = wrap
        && layout.wrap(limit)
    {
        layout.segments.clear();
        layout.clusters.clear();
        layout.glyphs.clear();
        layout.build_segments();
        for s in 0..layout.segments.len() {
            layout.shape_segment(s);
        }
    }
    layout.position();

    let bbox = layout.bbox();
    // §11.11 / CSS Text Decoration painting order: underlines and
    // overlines beneath the text, line-throughs on top.
    if !layout.decos.is_empty() {
        layout.decoration_nodes(
            DECORATION_UNDERLINE | DECORATION_OVERLINE,
            bbox,
            ctx,
            &mut group.children,
        );
    }
    layout.glyph_nodes(bbox, ctx, &mut group.children);
    if !layout.decos.is_empty() {
        layout.decoration_nodes(DECORATION_LINE_THROUGH, bbox, ctx, &mut group.children);
    }
    Ok(Some(Node::Group(group)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_without_resolver_emits_empty_group() {
        // No font hook set — every <text> should parse to an empty
        // group rather than crashing.
        let el = Element {
            name: "text".into(),
            attrs: vec![
                ("x".into(), "10".into()),
                ("y".into(), "20".into()),
                ("font-size".into(), "12".into()),
            ],
            children: vec![XmlNode::Text("Hi".into())],
        };
        let mut ctx = ParseContext::new();
        let state = PaintState::default();
        let node = parse_text_element(&el, &state, &mut ctx).unwrap().unwrap();
        match node {
            Node::Group(g) => assert!(g.children.is_empty()),
            _ => panic!("expected Group"),
        }
    }

    #[test]
    fn family_lists_split_and_unquote() {
        let q = FontQuery::new("\"Noto Serif\", 'DejaVu Sans' , serif", 16.0);
        let f: Vec<&str> = q.families().collect();
        assert_eq!(f, ["Noto Serif", "DejaVu Sans", "serif"]);
    }
}
