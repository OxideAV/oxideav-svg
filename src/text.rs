//! `<text>` / `<tspan>` parsing via the round-7 vector-first scribe API.
//!
//! Round 2 scope:
//!
//! - Parse `<text x y font-family font-size>{content}</text>` and the
//!   nested-`<tspan dx dy>` shape. Each tspan's `dx` / `dy` is added to
//!   the running pen position before its glyphs are emitted.
//! - Shape the text against a caller-supplied [`scribe::FaceChain`]
//!   (font bytes loaded out-of-band — the SVG parser does not own a
//!   font loader). The default behaviour when no chain is supplied is
//!   to emit the text as an empty `Group` so the rest of the document
//!   still loads.
//! - Build positioned glyph PathNodes wrapped in a Group at the text's
//!   `(x, y)` origin. The wrapping group inherits the text element's
//!   paint / opacity / transform.
//!
//! Out of scope (deferred):
//!
//! - Full CSS font-family fallback chains. Caller is responsible for
//!   wiring the chain.
//! - Bidi.
//!
//! Round 128 — `<textPath>` (SVG 2 §11.8) now lays a text run along a
//! referenced path. The path is resolved per the §11.8.1 precedence
//! (`path=` attribute > `href` > legacy `xlink:href`) against the
//! pre-walked id table; each glyph's midpoint is placed on the path at
//! its accumulated run-relative x advance (plus an optional
//! `startOffset`) using the new
//! [`crate::path_length::sample_path_at_distance`] arc-length sampler,
//! and the glyph is rotated by the path tangent at that point. Glyphs
//! whose mid-points fall outside `[0, total_length]` (negative or past
//! the end) are not rendered, per §11.8.2's startOffset rule. Without a
//! font resolver every `<textPath>` collapses to an empty group, same
//! as the bare `<text>` case.
//!
//! Round 172 — SVG 2 §11.10.1.1 `text-anchor` (`start | middle | end`).
//! The property is inherited via [`PaintState::text_anchor`] (parsed by
//! the round-118-style cascade in [`crate::element`]) and consumed
//! here:
//!
//! - For a plain `<text>` element the whole element forms a single
//!   text chunk: we collect every emitted glyph during the walk, then
//!   shift each glyph horizontally by `0` / `-W/2` / `-W` (where `W` is
//!   the chunk's pre-anchor run width) for `start` / `middle` / `end`
//!   respectively.
//! - For `<textPath>`, §11.8.3 biases the start-point-on-the-path by
//!   the same `0` / `-W/2` / `-W` term, so we fold the shift directly
//!   into `start_offset` before laying glyphs along the curve.

use std::cell::RefCell;
use std::sync::OnceLock;

use oxideav_core::{Group, Node, Path, Result, Transform2D};

use crate::element::{PaintState, ParseContext, TextAnchor};
use crate::parser::{attr, tag_local, Element, Node as XmlNode};
use crate::path_length::{compute_path_length, sample_path_at_distance};

/// Caller hook so applications can supply a font-resolution callback
/// without the SVG crate having to manage font registries itself.
///
/// The callback receives `(font_family, font_size_px)` (the family is
/// the raw `font-family` attribute value, comma-separated families
/// preserved verbatim). A return value of `None` causes the text to
/// be emitted as an empty `Group` (text will not render but the
/// document still parses).
///
/// Set once via [`set_font_resolver`]. Each call wraps the supplied
/// closure in an `Arc` and stores it globally.
type FontResolver = dyn Fn(&str, f32) -> Option<oxideav_scribe::FaceChain> + Send + Sync + 'static;

static FONT_RESOLVER: OnceLock<Box<FontResolver>> = OnceLock::new();

/// The global font resolver hook is one-shot — only the first
/// [`set_font_resolver`] call wins. Subsequent calls return this error
/// without overwriting the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolverAlreadySet;

impl std::fmt::Display for ResolverAlreadySet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("font resolver was already set")
    }
}

impl std::error::Error for ResolverAlreadySet {}

/// Install a font resolver. Subsequent `<text>` elements will call the
/// resolver to obtain a [`scribe::FaceChain`] keyed by `(font-family,
/// font-size)`. Returns [`ResolverAlreadySet`] if a resolver was
/// already set (the hook is one-shot — applications register at
/// startup).
///
/// If no resolver is installed, every `<text>` parses to an empty
/// group. This keeps round-2 behaviour predictable: text simply does
/// not render unless the caller opts in.
pub fn set_font_resolver<F>(resolver: F) -> std::result::Result<(), ResolverAlreadySet>
where
    F: Fn(&str, f32) -> Option<oxideav_scribe::FaceChain> + Send + Sync + 'static,
{
    FONT_RESOLVER
        .set(Box::new(resolver))
        .map_err(|_| ResolverAlreadySet)
}

fn current_resolver() -> Option<&'static FontResolver> {
    FONT_RESOLVER.get().map(|b| b.as_ref())
}

/// Walk a `<text>` element + its nested `<tspan>` children, tracking a
/// pen position that starts at the text's `(x, y)`. Returns the
/// wrapped Group node.
///
/// Round 172 — after the walk, the SVG 2 §11.10.1.1 `text-anchor` shift
/// is applied to every glyph emitted into the chunk. The chunk's pre-
/// anchor x extent is `pen.x − x` (round-2 has one chunk per `<text>`
/// because we don't yet support multi-x `<tspan>` chunk boundaries);
/// `start` keeps the shift at 0, `middle` shifts by `-extent/2`, `end`
/// shifts by `-extent`. `<textPath>` children opt out of the shared
/// shift — they apply their own §11.8.3 bias inline via `emit_text_path`,
/// since the shift biases the start-point-on-the-path rather than a
/// post-hoc x translation.
pub fn parse_text_element(
    el: &Element,
    state: &PaintState,
    ctx: &mut ParseContext,
) -> Result<Option<Node>> {
    let x = parse_coord(attr(el, "x"), 0.0);
    let y = parse_coord(attr(el, "y"), 0.0);
    let font_size = parse_coord(attr(el, "font-size"), 16.0);
    let font_family = attr(el, "font-family").unwrap_or("sans-serif").to_string();

    let pen = RefCell::new(Pen { x, y });
    let inheritance = TextInheritance {
        font_family,
        font_size,
        fill: state,
    };

    let mut group = Group::default();
    // Track child indices that belong to the chunk so the §11.10.1.1
    // text-anchor shift can rewrite their transforms after the walk.
    // `<textPath>` glyphs land in the same `group` but exit the chunk
    // (they're stamped with their own path-relative placement), so the
    // walker pushes their indices into a parallel skip-set.
    let chunk_start_index = group.children.len();
    let mut textpath_indices: Vec<usize> = Vec::new();
    walk_text_children(
        el,
        &inheritance,
        &pen,
        &mut group,
        ctx,
        &mut textpath_indices,
    )?;
    // Compute pre-anchor chunk extent and apply the shift in place.
    // The chunk's geometric extent is `pen.x − x` for round 172's
    // single-chunk model (any author-supplied `<tspan x=…>` would start
    // a new chunk per §11.5; the current walker simply moves the pen,
    // which approximates the chunk-boundary semantics for the common
    // case of one `<text>` = one chunk).
    let extent = pen.borrow().x - x;
    let shift = match state.text_anchor {
        TextAnchor::Start => 0.0,
        TextAnchor::Middle => -extent * 0.5,
        TextAnchor::End => -extent,
    };
    if shift.abs() > 0.0 {
        for (idx, child) in group
            .children
            .iter_mut()
            .enumerate()
            .skip(chunk_start_index)
        {
            if textpath_indices.contains(&idx) {
                continue;
            }
            if let Node::Group(g) = child {
                // Each emitted glyph is wrapped in a placement Group
                // whose transform is `translate(origin_x + glyph_x,
                // origin_y + glyph_y)`. A pure x-translate composes by
                // adding to `e`.
                g.transform = Transform2D::translate(shift, 0.0).compose(&g.transform);
            }
        }
    }
    Ok(Some(Node::Group(group)))
}

#[derive(Clone, Debug)]
struct Pen {
    x: f32,
    y: f32,
}

#[derive(Clone, Debug)]
struct TextInheritance<'a> {
    font_family: String,
    font_size: f32,
    fill: &'a PaintState,
}

fn walk_text_children(
    el: &Element,
    inh: &TextInheritance<'_>,
    pen: &RefCell<Pen>,
    out: &mut Group,
    ctx: &ParseContext,
    textpath_indices: &mut Vec<usize>,
) -> Result<()> {
    for child in &el.children {
        match child {
            XmlNode::Text(t) => {
                emit_run(t, inh, pen, out);
            }
            XmlNode::Element(c) if tag_local(&c.name) == "tspan" => {
                let mut sub = inh.clone();
                if let Some(v) = attr(c, "font-family") {
                    sub.font_family = v.to_string();
                }
                if let Some(v) = attr(c, "font-size") {
                    sub.font_size = parse_coord(Some(v), inh.font_size);
                }
                let dx = parse_coord(attr(c, "dx"), 0.0);
                let dy = parse_coord(attr(c, "dy"), 0.0);
                {
                    let mut p = pen.borrow_mut();
                    p.x += dx;
                    p.y += dy;
                }
                if let Some(v) = attr(c, "x") {
                    pen.borrow_mut().x = parse_coord(Some(v), pen.borrow().x);
                }
                if let Some(v) = attr(c, "y") {
                    pen.borrow_mut().y = parse_coord(Some(v), pen.borrow().y);
                }
                walk_text_children(c, &sub, pen, out, ctx, textpath_indices)?;
            }
            XmlNode::Element(c) if tag_local(&c.name) == "textpath" => {
                // Round 128 — SVG 2 §11.8 `<textPath>`. The element's
                // content (text + nested `<tspan>`s) is laid along the
                // referenced path instead of the parent `<text>`'s
                // baseline. Per §11.8.1 the path-resolution precedence
                // is `path=` > `href` > `xlink:href`. A missing /
                // unresolvable path collapses to a no-op so the rest
                // of the surrounding `<text>` still renders.
                let before = out.children.len();
                emit_text_path(c, inh, out, ctx);
                // Round 172 — record any glyphs the `<textPath>` just
                // emitted so the outer `text-anchor` post-shift can
                // skip them (they have already been biased per
                // §11.8.3 by `emit_text_path` itself).
                for i in before..out.children.len() {
                    textpath_indices.push(i);
                }
            }
            _ => {
                // Unknown nested element (a, etc.) — silently skipped.
            }
        }
    }
    Ok(())
}

/// Concatenate the immediate text content (and content of nested
/// `<tspan>` children) of a `<textPath>` element into a single string.
/// The shaping is done as one run so cumulative x-advances are
/// well-defined for arc-length placement.
///
/// Nested elements other than `<tspan>` contribute no text in round
/// 128 (matches the baseline `<text>` walker behaviour); a future
/// round can refine this to honour per-tspan inheritance overrides
/// along the path.
fn collect_text_run(el: &Element) -> String {
    let mut s = String::new();
    collect_text_run_into(el, &mut s);
    s
}

fn collect_text_run_into(el: &Element, out: &mut String) {
    for child in &el.children {
        match child {
            XmlNode::Text(t) => out.push_str(t),
            XmlNode::Element(c) if tag_local(&c.name) == "tspan" => collect_text_run_into(c, out),
            _ => {}
        }
    }
}

/// Resolve a `<textPath>`'s path per §11.8.1 precedence:
///
/// 1. `path` attribute — inline `d`-mini-language path data.
/// 2. `href` attribute — id reference (SVG 2 canonical).
/// 3. `xlink:href` — deprecated SVG 1.1 fallback.
///
/// Returns `None` when the element supplies none of the three or the
/// referenced source has no usable `d` attribute (in which case the
/// `<textPath>` renders no glyphs, matching browsers' behaviour).
fn resolve_text_path(el: &Element, ctx: &ParseContext) -> Option<Path> {
    if let Some(d) = attr(el, "path") {
        if let Ok(cmds) = crate::path_data::parse_path_data(d) {
            if !cmds.is_empty() {
                let mut p = Path::new();
                for cmd in cmds {
                    p.commands.push(cmd);
                }
                return Some(p);
            }
        }
    }
    let href = attr(el, "href").or_else(|| attr(el, "xlink:href"))?;
    let id = href.trim().strip_prefix('#')?;
    let target = ctx.defs.elements.get(id)?;
    let d = attr(target, "d")?;
    let cmds = crate::path_data::parse_path_data(d).ok()?;
    if cmds.is_empty() {
        return None;
    }
    let mut p = Path::new();
    for cmd in cmds {
        p.commands.push(cmd);
    }
    Some(p)
}

/// Parse the SVG 2 §11.8.2 `startOffset` attribute. The value can be a
/// `<number>` (user units along the path) or a `<percentage>` of the
/// path's total length. Negative values and percentages > 100 are
/// permitted by the spec but typically yield off-path glyphs (which
/// are then suppressed by the midpoint-on-path rule).
fn parse_start_offset(s: Option<&str>, path_len: f32) -> f32 {
    let Some(raw) = s.map(str::trim) else {
        return 0.0;
    };
    if raw.is_empty() {
        return 0.0;
    }
    if let Some(rest) = raw.strip_suffix('%') {
        return rest
            .trim()
            .parse::<f32>()
            .ok()
            .map(|p| p * 0.01 * path_len)
            .unwrap_or(0.0);
    }
    raw.parse::<f32>().unwrap_or(0.0)
}

/// SVG 2 §11.8.2 `side` attribute. `right` flips the layout direction
/// (the spec describes this as reversing the path before placement);
/// for our straight-line + monotonic-curve fixtures it suffices to
/// mirror each glyph's path-distance about the total length.
fn parse_side(s: Option<&str>) -> Side {
    match s.map(str::trim) {
        Some("right") => Side::Right,
        _ => Side::Left,
    }
}

#[derive(Clone, Copy, Debug)]
enum Side {
    Left,
    Right,
}

/// Lay each glyph of `el`'s text run along a resolved path. The output
/// Group's children are one positioned-and-rotated glyph node each;
/// nothing is emitted if the font resolver, path resolution, or shaper
/// produce no glyphs.
fn emit_text_path(el: &Element, inh: &TextInheritance<'_>, out: &mut Group, ctx: &ParseContext) {
    let text = collect_text_run(el);
    if text.is_empty() {
        return;
    }
    let chain = match current_resolver().and_then(|r| r(&inh.font_family, inh.font_size)) {
        Some(c) => c,
        None => return,
    };
    let path = match resolve_text_path(el, ctx) {
        Some(p) => p,
        None => return,
    };
    let total_length = compute_path_length(&path);
    if total_length <= 0.0 {
        return;
    }
    let raw_start_offset = parse_start_offset(attr(el, "startOffset"), total_length);
    let side = parse_side(attr(el, "side"));

    // Shape the text run once. We need per-glyph `x_advance` to compute
    // the midpoint along the run (and therefore the midpoint distance
    // along the path), so we drive shaping at the level below
    // `shape_to_paths` and assemble the placed nodes here.
    if chain.is_empty() {
        return;
    }
    let shaped = match chain.shape(&text, inh.font_size) {
        Ok(g) => g,
        Err(_) => return,
    };

    // Round 172 — §11.8.3 start-point-on-the-path bias by `text-anchor`.
    // For `start`, the start-point is `startOffset` along the path; for
    // `middle`, subtract half the run's total advance; for `end`,
    // subtract the full total. The total advance is the sum of every
    // shaped glyph's `x_advance` (whitespace included — those glyphs
    // still consume horizontal space even when no Path is emitted).
    let total_advance: f32 = shaped.iter().map(|g| g.x_advance).sum();
    let anchor_shift = match inh.fill.text_anchor {
        TextAnchor::Start => 0.0,
        TextAnchor::Middle => -total_advance * 0.5,
        TextAnchor::End => -total_advance,
    };
    let start_offset = raw_start_offset + anchor_shift;

    let fill = inh.fill.solid_fill_public();
    let mut pen_x = 0.0_f32;
    for g in &shaped {
        // Midpoint of this glyph along the run, per §11.8: "midpoint
        // of each typographic character is moved to the corresponding
        // point on the path".
        let mid = pen_x + g.x_offset + g.x_advance * 0.5;
        pen_x += g.x_advance;

        // Path-distance for the midpoint, honouring `startOffset` and
        // `side` per §11.8.2. Glyphs whose midpoint falls off the path
        // (negative or past the end) are not rendered.
        let dist = match side {
            Side::Left => start_offset + mid,
            Side::Right => total_length - (start_offset + mid),
        };
        if dist < 0.0 || dist > total_length {
            continue;
        }

        let face = chain.face(g.face_idx);
        let node = match face.glyph_node(g.glyph_id, inh.font_size) {
            Some(n) => n,
            None => continue,
        };

        let (pos, tangent_deg) = sample_path_at_distance(&path, dist);
        // Build: translate(pos) * rotate(tangent) * translate(-x_advance/2, y_offset)
        // The trailing translate places the glyph's midpoint at the
        // sampled `pos`; the rotate aligns the glyph's baseline with
        // the path tangent; the leading translate puts the sample's
        // origin into world space.
        let rot = Transform2D::rotate(tangent_deg.to_radians());
        let centring = Transform2D::translate(-g.x_advance * 0.5, g.y_offset);
        let placement = Transform2D::translate(pos.x, pos.y)
            .compose(&rot)
            .compose(&centring);
        let painted = repaint_node(node, fill.clone());
        out.children.push(Node::Group(Group {
            transform: placement,
            children: vec![painted],
            ..Group::default()
        }));
    }
}

fn emit_run(text: &str, inh: &TextInheritance<'_>, pen: &RefCell<Pen>, out: &mut Group) {
    let trimmed = text;
    if trimmed.is_empty() {
        return;
    }
    let chain = match current_resolver().and_then(|r| r(&inh.font_family, inh.font_size)) {
        Some(c) => c,
        None => return,
    };
    let glyphs = oxideav_scribe::Shaper::shape_to_paths(&chain, trimmed, inh.font_size);
    let (origin_x, origin_y) = {
        let p = pen.borrow();
        (p.x, p.y)
    };
    // The fill from the inherited state — text defaults to black if
    // unset (already encoded in PaintState::default).
    let fill = inh.fill.solid_fill_public();
    let mut max_advance = 0.0_f32;
    for (_face_idx, node, glyph_xform) in glyphs {
        // glyph_xform is a translate(target_x, y_offset) in raster
        // pixels relative to the run start. Add the text origin so the
        // glyph lands at (origin_x + target_x, origin_y + y_offset).
        let absolute = Transform2D::translate(origin_x, origin_y).compose(&glyph_xform);
        // Re-paint the glyph with the inherited fill (scribe gives us
        // black by default).
        let painted = repaint_node(node, fill.clone());
        // Wrap in a tiny group carrying the per-glyph transform.
        out.children.push(Node::Group(Group {
            transform: absolute,
            children: vec![painted],
            ..Group::default()
        }));
        if glyph_xform.e > max_advance {
            max_advance = glyph_xform.e;
        }
    }
    // Advance the pen past the last glyph's pen position. (This is
    // approximate — we don't know the last glyph's own advance from
    // the public scribe API. A subsequent <tspan> with `dx=` corrects
    // for the remaining advance; without `dx=` the next text run
    // overlaps the last glyph slightly. Round 3 will switch to the
    // round-7 `Shaper::shape` measurement API.)
    pen.borrow_mut().x = origin_x + max_advance;
}

fn repaint_node(node: Node, fill: Option<oxideav_core::Paint>) -> Node {
    match node {
        Node::Path(mut p) => {
            // Only override black-default fill — preserve anything the
            // caller explicitly set (scribe currently always sets
            // black, but be defensive against future colour glyphs).
            p.fill = fill.or(p.fill);
            Node::Path(p)
        }
        Node::Group(mut g) => {
            g.children = g
                .children
                .into_iter()
                .map(|c| repaint_node(c, fill.clone()))
                .collect();
            Node::Group(g)
        }
        other => other,
    }
}

fn parse_coord(s: Option<&str>, default: f32) -> f32 {
    crate::element::parse_number(s, default).unwrap_or(default)
}

// Round-trip-friendly accessor on PaintState. Defined here (and
// re-exported as a method on PaintState) so the text module can resolve
// fills without needing a gradient table.
impl crate::element::PaintState {
    pub(crate) fn solid_fill_public(&self) -> Option<oxideav_core::Paint> {
        // Round 118 — SVG 1.1 §11.5 `visibility: hidden | collapse`:
        // hidden text "is invisible but still takes up space in text
        // layout calculations". We emit the glyph geometry but with no
        // fill, mirroring the shape branch (geometry preserved,
        // nothing painted).
        if self.visibility == crate::element::Visibility::Hidden {
            return None;
        }
        // Only solid colours apply to text in round 2; gradient fills
        // on text are deferred (the SVG <text> case is `fill="..."`
        // resolved as a Paint, but with no gradient the round-2 path
        // below works for the common case).
        match &self.fill {
            crate::color::PaintValue::None => None,
            crate::color::PaintValue::Color(c) => Some(oxideav_core::Paint::Solid(apply_alpha(
                *c,
                self.fill_opacity * self.opacity,
            ))),
            crate::color::PaintValue::Reference { .. } => None,
        }
    }
}

fn apply_alpha(c: oxideav_core::Rgba, a: f32) -> oxideav_core::Rgba {
    let alpha = ((c.a as f32 / 255.0) * a.clamp(0.0, 1.0)) * 255.0;
    oxideav_core::Rgba::new(c.r, c.g, c.b, alpha.round() as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_without_resolver_emits_empty_group() {
        // No FONT_RESOLVER set — every <text> should parse to an
        // empty group rather than crashing.
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
}
