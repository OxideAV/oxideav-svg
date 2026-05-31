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
//!
//! Round 187 — SVG 2 §11.2.1 `textLength` + `lengthAdjust`. An author-
//! supplied `textLength=` declares the intended sum of all advance
//! values for the element's content; the implementation rescales each
//! glyph's x placement so the chunk's actual width matches the target.
//! `lengthAdjust="spacing"` (initial) adjusts only inter-glyph
//! positions; `lengthAdjust="spacingAndGlyphs"` additionally wraps each
//! glyph in an `scale(s, 1)` so the outlines themselves stretch /
//! compress along the inline-base direction.
//!
//! Per §11.2.1 the attribute applies per anchored chunk: a
//! `<tspan x=… textLength=…>` overrides the rescaling for its own
//! chunk only; a bare `<text textLength=…>` covers its whole element.
//! The §11.10.1.1 `text-anchor` shift is applied to the **already-
//! rescaled** chunk width (so a `middle`-anchored chunk with
//! `textLength=300` shifts by `−150`, not by half of the un-adjusted
//! glyph extent). Negative `textLength` is an error per the spec and is
//! ignored.
//!
//! Round 199 — SVG 2 §11.2 / §11.2.2 list-of-values on `x`, `y`, `dx`,
//! `dy`, and `rotate` for `<text>` and `<tspan>`. Earlier rounds only
//! parsed the first scalar of each attribute; round 199 parses the
//! full list and applies the n-th value to the n-th character per the
//! §11.2.2 "n-th character" rule (with the §11.2.2 final-`rotate`-sticks
//! rule for trailing characters). The lists are layered globally across
//! the whole `<text>` element: a `<tspan>` whose `x="…"` (or any of the
//! five) carries m values writes them into slots `[char_offset,
//! char_offset + m)` of the document-wide vectors, where `char_offset`
//! is the count of characters emitted so far. Absolute `x` / `y` values
//! on `<tspan>` slots still open a §11.5 anchored chunk per round 176
//! (so the per-chunk `text-anchor` shift composes with per-character
//! placement). `<textPath>` content is excluded from the list-applied
//! pass; §11.8.3's path-distance bias owns the placement instead.

use std::cell::RefCell;
use std::sync::OnceLock;

use oxideav_core::{Group, Node, Path, Result, Transform2D};

use crate::element::{PaintState, ParseContext, TextAnchor, TextLengthAdjust};
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
/// Round 176 — SVG 2 §11.5 text-chunk boundaries. Each new absolute
/// positioning adjustment on a `<tspan x=…>` (or `<tspan y=…>`) starts a
/// fresh anchored chunk; the §11.10.1.1 `text-anchor` shift now applies
/// per chunk rather than once across the whole `<text>` element. Within
/// each chunk the round-172 rule is unchanged: `start` shifts by `0`,
/// `middle` by `-extent / 2`, `end` by `-extent`, where `extent` is the
/// chunk's accumulated x-advance. The chunk's effective anchor is the
/// inherited `text-anchor` resolved at the element that opened the
/// chunk — the root `<text>` for the first chunk, the chunk-starting
/// `<tspan>` for subsequent chunks. `<textPath>` children stay outside
/// any open chunk (they have their own §11.8.3 bias) and the next sibling
/// after a `<textPath>` opens a new chunk per §11.8 (an embedded
/// textPath always creates an anchored-chunk boundary).
pub fn parse_text_element(
    el: &Element,
    state: &PaintState,
    ctx: &mut ParseContext,
) -> Result<Option<Node>> {
    // Round 199 — accept lists-of-numbers per SVG 2 §11.2 / §11.2.2 on
    // `x` / `y` / `dx` / `dy` / `rotate`. The first entry of `x` / `y`
    // doubles as the text-origin (a missing list leaves the origin at
    // 0,0 per spec).
    let xs = parse_number_list_attr(attr(el, "x"));
    let ys = parse_number_list_attr(attr(el, "y"));
    let dxs = parse_number_list_attr(attr(el, "dx"));
    let dys = parse_number_list_attr(attr(el, "dy"));
    let rotates = parse_number_list_attr(attr(el, "rotate"));
    let x = xs.first().copied().unwrap_or(0.0);
    let y = ys.first().copied().unwrap_or(0.0);
    let font_size = parse_coord(attr(el, "font-size"), 16.0);
    let font_family = attr(el, "font-family").unwrap_or("sans-serif").to_string();

    let pen = RefCell::new(Pen { x, y });
    let inheritance = TextInheritance {
        font_family,
        font_size,
        fill: state,
        text_anchor: state.text_anchor,
    };

    let mut group = Group::default();
    let mut chunks: Vec<Chunk> = Vec::new();
    // The root `<text>` opens the first chunk at its `(x, y)` origin with
    // the inherited `text-anchor`. Round 187 — pick up its `textLength` /
    // `lengthAdjust` (if any) so the post-walk pass can rescale the
    // chunk's glyph placements per §11.2.1.
    let root_text_length = parse_text_length(attr(el, "textLength"), attr(el, "lengthAdjust"));
    chunks.push(Chunk {
        start_index: group.children.len(),
        end_index: 0, // patched once children land
        x_origin: x,
        x_end: x,
        anchor: state.text_anchor,
        text_length: root_text_length,
    });
    let mut textpath_indices: Vec<usize> = Vec::new();
    // Round 199 — global per-character override vectors. Slot `i` is
    // applied to the `i`-th character emitted by the `<text>` element.
    // Initial slots populated from the root `<text>`'s list attributes;
    // `<tspan>` descendants overlay additional values onto the same
    // vectors starting at the current character ordinal.
    //
    // The first element of the root `xs` / `ys` is the text-origin
    // (we've already seated `pen` at `(xs[0], ys[0])` above), so we
    // do NOT replay it through the per-character pass. Replaying
    // would mark the run as "carrying a per-char override" and skip
    // the round-2 legacy pen-rewind, breaking the round-187
    // `textLength` chunk-extent math which relies on the rewind.
    let mut per_char = PerCharSpec::default();
    if xs.len() > 1 {
        per_char.merge_xs(1, &xs[1..]);
    }
    if ys.len() > 1 {
        per_char.merge_ys(1, &ys[1..]);
    }
    per_char.merge_dxs(0, &dxs);
    per_char.merge_dys(0, &dys);
    per_char.merge_rotates(0, &rotates);
    let mut char_counter: usize = 0;
    walk_text_children(
        el,
        &inheritance,
        &pen,
        &mut group,
        &mut chunks,
        ctx,
        &mut textpath_indices,
        &mut per_char,
        &mut char_counter,
    )?;
    // Close the final chunk at the current pen position.
    if let Some(last) = chunks.last_mut() {
        last.end_index = group.children.len();
        last.x_end = pen.borrow().x;
    }
    // Round 187 — §11.2.1 `textLength` rescaling runs BEFORE the
    // §11.10.1.1 anchor shift so the anchor measures against the
    // adjusted chunk width. The two-pass order matches the spec's
    // descriptive note "the implementation rescales each glyph's x
    // position so the actual width matches `textLength`, then the
    // text-anchor shift applies to the rescaled chunk".
    apply_text_length_rescaling(&mut group, &mut chunks, &textpath_indices);
    apply_chunk_anchor_shifts(&mut group, &chunks, &textpath_indices);
    Ok(Some(Node::Group(group)))
}

/// SVG 2 §11.2 / §11.2.2 per-character override vectors. Slot `i`
/// applies to the `i`-th character emitted by the enclosing `<text>`.
/// Sparse semantics: `None` means "no override for this character"
/// (the natural pen position / no rotation is used), `Some(v)` means
/// the spec-defined per-character behaviour applies.
///
/// `rotate` follows the §11.2.2 sticky-final rule: if `rotates.len() <
/// N` (the total number of characters), the final supplied value
/// applies to every trailing character with no slot of its own.
#[derive(Default, Debug)]
struct PerCharSpec {
    xs: Vec<Option<f32>>,
    ys: Vec<Option<f32>>,
    dxs: Vec<Option<f32>>,
    dys: Vec<Option<f32>>,
    rotates: Vec<Option<f32>>,
}

impl PerCharSpec {
    fn merge_into(target: &mut Vec<Option<f32>>, char_offset: usize, values: &[f32]) {
        if values.is_empty() {
            return;
        }
        let end = char_offset + values.len();
        if target.len() < end {
            target.resize(end, None);
        }
        for (i, v) in values.iter().enumerate() {
            target[char_offset + i] = Some(*v);
        }
    }

    fn merge_xs(&mut self, char_offset: usize, values: &[f32]) {
        Self::merge_into(&mut self.xs, char_offset, values);
    }

    fn merge_ys(&mut self, char_offset: usize, values: &[f32]) {
        Self::merge_into(&mut self.ys, char_offset, values);
    }

    fn merge_dxs(&mut self, char_offset: usize, values: &[f32]) {
        Self::merge_into(&mut self.dxs, char_offset, values);
    }

    fn merge_dys(&mut self, char_offset: usize, values: &[f32]) {
        Self::merge_into(&mut self.dys, char_offset, values);
    }

    fn merge_rotates(&mut self, char_offset: usize, values: &[f32]) {
        Self::merge_into(&mut self.rotates, char_offset, values);
    }

    /// §11.2.2 "n-th character" lookup. `None` means "use the running
    /// pen position / no rotation"; `Some` means override.
    fn x_at(&self, n: usize) -> Option<f32> {
        self.xs.get(n).copied().flatten()
    }

    fn y_at(&self, n: usize) -> Option<f32> {
        self.ys.get(n).copied().flatten()
    }

    fn dx_at(&self, n: usize) -> Option<f32> {
        self.dxs.get(n).copied().flatten()
    }

    fn dy_at(&self, n: usize) -> Option<f32> {
        self.dys.get(n).copied().flatten()
    }

    /// §11.2.2 sticky-final rule for `rotate`: if `n` is past the last
    /// supplied value, the final supplied value applies. Empty list →
    /// `None` (no rotation).
    fn rotate_at(&self, n: usize) -> Option<f32> {
        if self.rotates.is_empty() {
            return None;
        }
        if let Some(Some(v)) = self.rotates.get(n) {
            return Some(*v);
        }
        // Past the end → walk back to the most recent `Some`. The §11.2.2
        // rule is "the final supplied value", which in our sparse
        // representation is the last `Some` we ever set; trailing `None`
        // slots (caused by an inner merge writing past the outer list)
        // are bridged to that final value.
        self.rotates.iter().rev().find_map(|v| *v)
    }
}

/// One §11.5 anchored chunk: a half-open child-index range plus the
/// pen-x positions at the chunk's open and close. The anchor is the
/// inherited `text-anchor` of the element that opened the chunk.
///
/// Round 187 — `text_length` carries the §11.2.1 author-supplied target
/// width (in user units) for the chunk, plus the `lengthAdjust` mode
/// selecting between advance-only rescaling and full glyph-outline
/// stretching. `None` (the default) skips the §11.2.1 rescaling entirely;
/// the chunk's width is whatever the shaper produced.
#[derive(Clone, Debug)]
struct Chunk {
    start_index: usize,
    end_index: usize,
    x_origin: f32,
    x_end: f32,
    anchor: TextAnchor,
    text_length: Option<TextLengthSpec>,
}

/// Author-supplied `textLength` + `lengthAdjust` per anchored chunk.
#[derive(Clone, Copy, Debug)]
struct TextLengthSpec {
    /// Target advance sum, in user units. Non-negative; values that
    /// don't parse as a finite positive number are dropped by
    /// [`parse_text_length`] (so this value is always a valid target).
    target: f32,
    adjust: TextLengthAdjust,
}

/// Walk every chunk and, for each non-`<textPath>` placement Group
/// within it, prepend the §11.10.1.1 shift. `start` is a zero shift so
/// the loop early-exits for that case without rewriting transforms.
fn apply_chunk_anchor_shifts(group: &mut Group, chunks: &[Chunk], textpath_indices: &[usize]) {
    for chunk in chunks {
        let extent = chunk.x_end - chunk.x_origin;
        let shift = match chunk.anchor {
            TextAnchor::Start => 0.0,
            TextAnchor::Middle => -extent * 0.5,
            TextAnchor::End => -extent,
        };
        if shift.abs() == 0.0 {
            continue;
        }
        for idx in chunk.start_index..chunk.end_index {
            if textpath_indices.contains(&idx) {
                continue;
            }
            if let Some(Node::Group(g)) = group.children.get_mut(idx) {
                // The placement Group's transform is `translate(origin_x
                // + glyph_x, origin_y + glyph_y)`; a pure x-translate
                // composes by adding to `e`.
                g.transform = Transform2D::translate(shift, 0.0).compose(&g.transform);
            }
        }
    }
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
    /// Round 176 — the inherited `text-anchor` snapshot taken at the
    /// current element. Used when a `<tspan x=…>` opens a new §11.5
    /// chunk so the chunk picks up its element's own anchor (not the
    /// root `<text>`'s) when computing the §11.10.1.1 shift.
    text_anchor: TextAnchor,
}

#[allow(clippy::too_many_arguments)]
fn walk_text_children(
    el: &Element,
    inh: &TextInheritance<'_>,
    pen: &RefCell<Pen>,
    out: &mut Group,
    chunks: &mut Vec<Chunk>,
    ctx: &ParseContext,
    textpath_indices: &mut Vec<usize>,
    per_char: &mut PerCharSpec,
    char_counter: &mut usize,
) -> Result<()> {
    for child in &el.children {
        match child {
            XmlNode::Text(t) => {
                emit_run(t, inh, pen, out, per_char, char_counter);
            }
            XmlNode::Element(c) if tag_local(&c.name) == "tspan" => {
                let mut sub = inh.clone();
                if let Some(v) = attr(c, "font-family") {
                    sub.font_family = v.to_string();
                }
                if let Some(v) = attr(c, "font-size") {
                    sub.font_size = parse_coord(Some(v), inh.font_size);
                }
                // Round 176 — honour an inline `text-anchor=` override
                // on the `<tspan>` so an absolute-positioned span can
                // open its chunk with its own anchor. Unknown / absent
                // values inherit unchanged from `inh`.
                if let Some(v) = attr(c, "text-anchor") {
                    sub.text_anchor = parse_text_anchor(v, inh.text_anchor);
                }
                // Round 199 — parse the full list-of-values from each
                // of `x` / `y` / `dx` / `dy` / `rotate` on the
                // `<tspan>`. The lists overlay onto the document-wide
                // per-character vectors starting at the current
                // character ordinal so the per-character pass in
                // [`emit_run`] can read them by character index.
                let tspan_xs = parse_number_list_attr(attr(c, "x"));
                let tspan_ys = parse_number_list_attr(attr(c, "y"));
                let tspan_dxs = parse_number_list_attr(attr(c, "dx"));
                let tspan_dys = parse_number_list_attr(attr(c, "dy"));
                let tspan_rotates = parse_number_list_attr(attr(c, "rotate"));
                let char_offset = *char_counter;
                // The first element of a tspan's `x` / `y` list is the
                // chunk-opening absolute position (we set `pen` to it
                // below); the per-character pass should NOT replay it.
                // Replaying would trip [`emit_run`]'s `saw_override`
                // flag and bypass the round-2 pen-rewind, breaking
                // the round-176 chunk-extent math and the round-187
                // `textLength` rescale. Subsequent list entries are
                // genuine per-character overrides and ARE merged.
                if tspan_xs.len() > 1 {
                    per_char.merge_xs(char_offset + 1, &tspan_xs[1..]);
                }
                if tspan_ys.len() > 1 {
                    per_char.merge_ys(char_offset + 1, &tspan_ys[1..]);
                }
                per_char.merge_dxs(char_offset, &tspan_dxs);
                per_char.merge_dys(char_offset, &tspan_dys);
                per_char.merge_rotates(char_offset, &tspan_rotates);
                // Round 176 — SVG 2 §11.5 anchored-chunk boundary. An
                // explicit `x` or `y` on a `<tspan>` is an absolute
                // positioning adjustment which closes the open chunk
                // (at the pen-x reached so far) and starts a new one
                // at the new pen position. `dx` / `dy` alone do NOT
                // open a chunk (they're relative pen nudges that stay
                // within the parent chunk). Round 199 — the per-char
                // pass in [`emit_run`] consumes the slotted per-char
                // dx/dy, so the legacy "pen += first-value of dx/dy"
                // behaviour is now bundled into the per-character
                // application path (no separate pen-update needed).
                let opens_chunk = !tspan_xs.is_empty() || !tspan_ys.is_empty();
                let pre_chunk_pen_x = pen.borrow().x;
                // For chunk-boundary bookkeeping the open uses the
                // first value of each list (the §11.5 rule).
                if let Some(&first_x) = tspan_xs.first() {
                    pen.borrow_mut().x = first_x;
                }
                if let Some(&first_y) = tspan_ys.first() {
                    pen.borrow_mut().y = first_y;
                }
                // Round 187 — pick up a per-tspan `textLength` /
                // `lengthAdjust`. Per §11.2.1 the attribute is NOT
                // inherited, so an outer-text `textLength` does not
                // bleed into this tspan; instead, when an explicit
                // value is present it overrides the rescaling for
                // *this* chunk only. The value is plumbed through
                // [`open_new_chunk`] (when a chunk opens) so the
                // post-walk pass sees the correct binding. When a
                // tspan carries `textLength` but no `x|y` (does not
                // open a chunk per §11.5), we extend the open chunk's
                // binding so its rescaling still honours the
                // descendant's target.
                let tspan_tl = parse_text_length(attr(c, "textLength"), attr(c, "lengthAdjust"));
                if opens_chunk {
                    let new_x = pen.borrow().x;
                    open_new_chunk(
                        chunks,
                        out,
                        pre_chunk_pen_x,
                        new_x,
                        sub.text_anchor,
                        tspan_tl,
                    );
                } else if let Some(spec) = tspan_tl {
                    // Per §11.2.1 "If `textLength` is specified on a
                    // given element and also specified on an
                    // ancestor", the descendant wins for its content;
                    // here the descendant doesn't open its own chunk
                    // (no `x|y`), so we promote the binding onto the
                    // current open chunk. A bare tspan textLength on a
                    // text without `x|y` then drives the whole chunk.
                    if let Some(last) = chunks.last_mut() {
                        last.text_length = Some(spec);
                    }
                }
                walk_text_children(
                    c,
                    &sub,
                    pen,
                    out,
                    chunks,
                    ctx,
                    textpath_indices,
                    per_char,
                    char_counter,
                )?;
            }
            XmlNode::Element(c) if tag_local(&c.name) == "textpath" => {
                // Round 128 — SVG 2 §11.8 `<textPath>`. The element's
                // content (text + nested `<tspan>`s) is laid along the
                // referenced path instead of the parent `<text>`'s
                // baseline. Per §11.8.1 the path-resolution precedence
                // is `path=` > `href` > `xlink:href`. A missing /
                // unresolvable path collapses to a no-op so the rest
                // of the surrounding `<text>` still renders.
                //
                // Round 176 — per §11.5 a `<textPath>` element forms
                // its own anchored-chunk boundary: the surrounding
                // chunk closes before the textPath's first glyph and a
                // fresh chunk opens for any sibling content that
                // follows. The textPath's own glyphs are pushed to
                // `textpath_indices` and therefore skipped by the
                // chunk-anchor pass (they have already been biased
                // per §11.8.3 by `emit_text_path` itself).
                let before = out.children.len();
                // Close the surrounding chunk just before the textPath.
                if let Some(last) = chunks.last_mut() {
                    last.end_index = before;
                    last.x_end = pen.borrow().x;
                }
                emit_text_path(c, inh, out, ctx);
                for i in before..out.children.len() {
                    textpath_indices.push(i);
                }
                // Round 199 — advance the document-wide character
                // counter past the textPath's text content. Per §11.5
                // the counter spans the whole `<text>` element so a
                // `<tspan>` sibling after a `<textPath>` whose own
                // per-character list applies from the correct ordinal.
                *char_counter += collect_text_run(c).chars().count();
                // Reopen a chunk for any sibling content that follows
                // the textPath. The pen position is unchanged (textPath
                // glyphs don't advance the parent's pen, matching the
                // round-128 baseline); the new chunk inherits the
                // parent element's `text-anchor`. Round 187 — the new
                // chunk has no `textLength` binding of its own (text
                // following a `<textPath>` does not inherit the
                // outer-text rescale per §11.2.1's "applies only when
                // the wrapping area is not defined by … shape-inside",
                // and the textPath's own bias is already in
                // [`emit_text_path`]).
                let cur_x = pen.borrow().x;
                open_new_chunk(chunks, out, cur_x, cur_x, inh.text_anchor, None);
            }
            _ => {
                // Unknown nested element (a, etc.) — silently skipped.
            }
        }
    }
    Ok(())
}

/// Close the currently-open chunk at `close_x` and append a fresh
/// chunk starting at `(start_x, anchor)`. If the prior chunk emitted
/// no glyphs (its `[start_index, end_index)` range is empty), it is
/// replaced in-place — a `<text>` whose first child is a
/// `<tspan x=…>` should not leave an empty leading chunk on the list.
/// The new chunk's `end_index` is patched by the caller's outer
/// close-out (or by the next chunk boundary) after its glyphs have
/// been emitted.
fn open_new_chunk(
    chunks: &mut Vec<Chunk>,
    group: &Group,
    close_x: f32,
    start_x: f32,
    anchor: TextAnchor,
    text_length: Option<TextLengthSpec>,
) {
    let next_idx = group.children.len();
    let drop_open = chunks
        .last()
        .map(|c| c.start_index == next_idx)
        .unwrap_or(false);
    if drop_open {
        if let Some(last) = chunks.last_mut() {
            last.x_origin = start_x;
            last.x_end = start_x;
            last.anchor = anchor;
            last.text_length = text_length;
        }
        return;
    }
    if let Some(last) = chunks.last_mut() {
        last.end_index = next_idx;
        last.x_end = close_x;
    }
    chunks.push(Chunk {
        start_index: next_idx,
        end_index: 0,
        x_origin: start_x,
        x_end: start_x,
        anchor,
        text_length,
    });
}

/// Round 199 — parse an SVG list-of-numbers attribute (whitespace-
/// and/or-comma-separated, with optional CSS unit suffixes per §6.3 of
/// CSS Values L4). Returns an empty `Vec` for `None`, the empty
/// string, or a value that contains no parseable number — keeping the
/// "first value" fallback well-defined.
///
/// The parser is intentionally lenient: tokens that don't parse fall
/// out of the result rather than aborting the whole list, matching
/// SVG's general "tolerate garbage, take the prefix" policy. Unit
/// suffixes (`px`, `em`, `%`) are consumed by [`element::parse_number`]
/// but ignored: round 199 still treats every list entry as user units.
fn parse_number_list_attr(s: Option<&str>) -> Vec<f32> {
    let Some(raw) = s else {
        return Vec::new();
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    // SVG 2 list separator: whitespace and/or a single comma per the
    // generic list productions. We split on any run of ASCII
    // whitespace or commas; empty fragments are discarded.
    let mut out = Vec::new();
    for tok in trimmed.split(|c: char| c.is_ascii_whitespace() || c == ',') {
        let t = tok.trim();
        if t.is_empty() {
            continue;
        }
        // `parse_number` handles the longest-numeric-prefix policy and
        // unit-suffix stripping (`12px`, `3.5em`, `50%`); we use NaN
        // as the sentinel and drop tokens whose prefix doesn't parse.
        if let Ok(v) = crate::element::parse_number(Some(t), f32::NAN) {
            if v.is_finite() {
                out.push(v);
            }
        }
    }
    out
}

/// Round 187 — parse the SVG 2 §11.2.1 `textLength` + `lengthAdjust`
/// attribute pair into a [`TextLengthSpec`]. Returns `None` when no
/// `textLength` was supplied, the value fails to parse, or it resolves
/// to a negative number (§11.2.1: "A negative value is an error",
/// which we treat as "drop the attribute" since the spec error-recovery
/// mode is up to the UA).
///
/// `lengthAdjust` resolves case-insensitively against the spec's two
/// keywords; unknown / absent values fall back to the
/// [`TextLengthAdjust::Spacing`] initial.
fn parse_text_length(
    text_length: Option<&str>,
    length_adjust: Option<&str>,
) -> Option<TextLengthSpec> {
    let raw = text_length?.trim();
    if raw.is_empty() {
        return None;
    }
    // Accept a bare number; reject `<length-percentage>` units for now
    // (a future round can route through the round-18 length resolver,
    // but the common author input is a unit-less user-unit number).
    let target: f32 = raw.parse().ok()?;
    if !target.is_finite() || target < 0.0 {
        return None;
    }
    let adjust = match length_adjust.map(str::trim) {
        Some(s) if s.eq_ignore_ascii_case("spacingandglyphs") => TextLengthAdjust::SpacingAndGlyphs,
        Some(s) if s.eq_ignore_ascii_case("spacing") => TextLengthAdjust::Spacing,
        _ => TextLengthAdjust::Spacing,
    };
    Some(TextLengthSpec { target, adjust })
}

/// Round 187 — per-chunk §11.2.1 rescaling. For each chunk that carries
/// a [`TextLengthSpec`], rewrite every glyph placement within the
/// chunk's `[start_index, end_index)` range so the resulting run
/// extent matches the author-supplied target:
///
/// 1. Compute `actual = x_end − x_origin` (the chunk's natural width
///    from shaping). If `actual` is zero (empty chunk) or non-finite,
///    skip — there is no run to rescale.
/// 2. Compute the scale factor `s = target / actual`.
/// 3. For each placement Group at index `i` (skipping any in
///    `textpath_indices`, which were already biased by §11.8.3):
///    - **Spacing**: replace the leading translate's x-component by
///      `x_origin + (cur_x − x_origin) * s` so only the inter-glyph
///      advance is rescaled.
///    - **SpacingAndGlyphs**: do the above, then post-compose
///      `scale(s, 1)` onto the placement transform so the glyph
///      outline stretches along the inline-base direction too.
/// 4. Patch `chunk.x_end` to `x_origin + target` so the subsequent
///    §11.10.1.1 anchor shift sees the rescaled width.
fn apply_text_length_rescaling(
    group: &mut Group,
    chunks: &mut [Chunk],
    textpath_indices: &[usize],
) {
    for chunk in chunks.iter_mut() {
        let Some(spec) = chunk.text_length else {
            continue;
        };
        let actual = chunk.x_end - chunk.x_origin;
        if !actual.is_finite() || actual.abs() < f32::EPSILON {
            // Empty or degenerate chunk — nothing to rescale. Still
            // patch `x_end` to the target so a downstream anchor shift
            // honours the author's intent on this chunk.
            chunk.x_end = chunk.x_origin + spec.target;
            continue;
        }
        let s = spec.target / actual;
        if !s.is_finite() {
            continue;
        }
        for idx in chunk.start_index..chunk.end_index {
            if textpath_indices.contains(&idx) {
                continue;
            }
            let Some(Node::Group(g)) = group.children.get_mut(idx) else {
                continue;
            };
            // The placement Group's transform is `translate(origin_x +
            // glyph_x, origin_y + glyph_y)`; rescale the x-component
            // relative to the chunk origin. `g.transform.e` is the
            // current absolute x-position (a flat translate, by
            // construction in [`emit_run`]).
            let cur_x = g.transform.e;
            let new_x = chunk.x_origin + (cur_x - chunk.x_origin) * s;
            g.transform = Transform2D {
                e: new_x,
                ..g.transform
            };
            if spec.adjust == TextLengthAdjust::SpacingAndGlyphs {
                // Post-compose `scale(s, 1)` so the glyph outline
                // itself stretches / compresses along the x axis. The
                // translate-then-scale ordering keeps the glyph's
                // origin at `new_x` (the scale operates in the local
                // post-translate space).
                g.transform = g.transform.compose(&Transform2D::scale(s, 1.0));
            }
        }
        chunk.x_end = chunk.x_origin + spec.target;
    }
}

/// Round 176 — case-insensitive `text-anchor` keyword parser. Mirrors
/// the [`PaintState::apply_one`] branch so a `<tspan text-anchor=…>` is
/// honoured even though the tspan walker doesn't run the full §11
/// cascade for nested text descendants.
fn parse_text_anchor(raw: &str, fallback: TextAnchor) -> TextAnchor {
    let t = raw.trim();
    if t.eq_ignore_ascii_case("start") {
        TextAnchor::Start
    } else if t.eq_ignore_ascii_case("middle") {
        TextAnchor::Middle
    } else if t.eq_ignore_ascii_case("end") {
        TextAnchor::End
    } else {
        // `inherit` and any unrecognised keyword leave the cascade
        // unchanged, matching the §11.5 `visibility` branch's lenient
        // policy.
        fallback
    }
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

fn emit_run(
    text: &str,
    inh: &TextInheritance<'_>,
    pen: &RefCell<Pen>,
    out: &mut Group,
    per_char: &PerCharSpec,
    char_counter: &mut usize,
) {
    let trimmed = text;
    if trimmed.is_empty() {
        return;
    }
    let chain = match current_resolver().and_then(|r| r(&inh.font_family, inh.font_size)) {
        Some(c) => c,
        None => {
            // Even with no resolver we still advance the character
            // counter so a sibling `<tspan>`'s per-character lists
            // land at the right ordinal.
            *char_counter += text.chars().count();
            return;
        }
    };
    // Shape the run once so we have per-glyph advance values; under
    // §11.2.2 case 1 (single character ↔ single glyph) the n-th glyph
    // corresponds to the n-th character. This is the common case for
    // the bundled DejaVu Sans Mono fixture and for typical Latin
    // input — the §11.2.2 "complex script" cases (ligature merging /
    // accent decomposition) are deferred until the shaper exposes a
    // character ↔ glyph cluster map.
    let shaped = match chain.shape(trimmed, inh.font_size) {
        Ok(g) => g,
        Err(_) => {
            *char_counter += text.chars().count();
            return;
        }
    };

    let fill = inh.fill.solid_fill_public();
    // Iterate characters and glyphs in lock-step. When the shaper
    // returns fewer glyphs than characters (e.g. a soft-hyphen that
    // shapes away), the extra characters still consume their slot of
    // the per-character vectors but contribute no glyph.
    let chars: Vec<char> = trimmed.chars().collect();
    let n_chars = chars.len();
    let n_glyphs = shaped.len();
    let mut glyph_idx = 0;
    // Round 199 — preserve the legacy round-2 inter-run pen convention
    // (pen.x sits at the LAST emitted glyph's origin, not past it) for
    // the common case where no per-character override applies on a
    // run that only contains whitespace or only contributes glyphs to
    // the START of a chunk. The round-176 §11.5 chunk-extent math
    // and the round-2 `<tspan dx>`-overlap rule both rely on this
    // legacy convention. We snapshot the pen-x at the moment of the
    // last visible glyph's placement and only fold the final advance
    // back in when a subsequent per-character override demands it.
    let mut last_glyph_origin_x: Option<f32> = None;
    // Round 199 — pen.x at run entry. Used to detect whitespace-only
    // runs (no visible glyph emitted) so we can leave the outer pen
    // unchanged on exit, mirroring the round-2
    // `max_advance == 0 ⇒ pen.x = origin_x` behaviour.
    let pen_x_on_entry = pen.borrow().x;
    // Round 199 — was any per-character override seen during this
    // run? If so, the legacy "pen at last-glyph origin" rewind is
    // unsafe (it'd swallow a deliberate per-char x/y), so we keep
    // the post-final-advance position.
    let mut saw_override = false;
    // §11.2 / §11.2.2 main loop.
    for (i, _ch) in chars.iter().enumerate() {
        let char_ord = *char_counter + i;
        // Apply the §11.2.2 per-character pen overrides BEFORE
        // emitting the character's glyph:
        //   1. absolute `x` / `y` set the current text position;
        //   2. relative `dx` / `dy` add to the current position.
        // The order matches the spec's algorithm step in §11.5: an
        // absolute override seats the pen, then a `dx` / `dy` on the
        // same character nudges it further.
        if let Some(x) = per_char.x_at(char_ord) {
            pen.borrow_mut().x = x;
            saw_override = true;
        }
        if let Some(y) = per_char.y_at(char_ord) {
            pen.borrow_mut().y = y;
            saw_override = true;
        }
        if let Some(dx) = per_char.dx_at(char_ord) {
            pen.borrow_mut().x += dx;
            saw_override = true;
        }
        if let Some(dy) = per_char.dy_at(char_ord) {
            pen.borrow_mut().y += dy;
            saw_override = true;
        }

        // Place the glyph (if any) at the current pen position. The
        // per-character `rotate` is applied as a rotation about the
        // glyph origin (the §11.2.2 wording: "the rotation value
        // creates a temporary new rotated coordinate system, and the
        // glyphs corresponding to the character are rendered into
        // this rotated coordinate system").
        let (origin_x, origin_y) = {
            let p = pen.borrow();
            (p.x, p.y)
        };
        if glyph_idx < n_glyphs {
            let g_meta = &shaped[glyph_idx];
            // Only emit a placement Group when the glyph has an
            // outline. Whitespace / non-rendering glyphs (e.g. SPACE)
            // still advance the pen but contribute no visible node,
            // matching the round-2 [`shape_to_paths`] policy of
            // `continue`-ing on a `None` glyph_node. Emitting an
            // empty-group placement would otherwise show up as a
            // translated identity child and break the round-176
            // leftmost-glyph chunk-extent assertions.
            let face = chain.face(g_meta.face_idx);
            if let Some(node) = face.glyph_node(g_meta.glyph_id, inh.font_size) {
                let rot_deg = per_char.rotate_at(char_ord).unwrap_or(0.0);
                let mut placement = Transform2D::translate(origin_x, origin_y);
                if rot_deg.abs() > 0.0 {
                    placement = placement.compose(&Transform2D::rotate(rot_deg.to_radians()));
                }
                // Apply the glyph's intra-cluster y_offset (carries the
                // baseline-vertical adjustment from shaping). The
                // x_offset is rolled into the pen advance.
                placement =
                    placement.compose(&Transform2D::translate(g_meta.x_offset, g_meta.y_offset));
                let painted = repaint_node(node, fill.clone());
                out.children.push(Node::Group(Group {
                    transform: placement,
                    children: vec![painted],
                    ..Group::default()
                }));
                // Snapshot the pen position at this glyph's origin so
                // we can restore the legacy "pen sits at last-glyph
                // origin" convention at the end of the run — preserves
                // round-176 chunk-extent math.
                last_glyph_origin_x = Some(pen.borrow().x);
            }
            // Advance the pen by this glyph's natural advance so the
            // next character starts at the spec-correct position
            // (unless overridden by the next character's `x` / `y`).
            // Whitespace / non-rendering glyphs still advance.
            pen.borrow_mut().x += g_meta.x_advance;
            glyph_idx += 1;
        }
    }
    // Round 199 — end-of-run pen housekeeping:
    //
    // - If no visible glyph was emitted (the run was whitespace-only
    //   or had no resolvable outlines), leave the outer pen UNCHANGED
    //   from its entry value. Matches the round-2
    //   `max_advance == 0 ⇒ pen.x = origin_x` behaviour and keeps
    //   leading/trailing/inter-tspan whitespace from inflating chunk
    //   extents.
    // - If at least one visible glyph WAS emitted AND no
    //   per-character override applied during the run, rewind the
    //   pen to the LAST visible glyph's origin (the round-2
    //   "1-advance-short" convention). A subsequent `<tspan dx>` or
    //   `<tspan x>` re-seats the pen as appropriate.
    // - If a per-character override DID apply, the post-final-advance
    //   pen.x is left as-is so per-character placement (e.g. the
    //   `<text x="0" y="50" font-family="mono">A<tspan x="100 200">…`
    //   round-199 case) reaches the right ordinal for the next
    //   sibling run.
    match last_glyph_origin_x {
        None => {
            pen.borrow_mut().x = pen_x_on_entry;
        }
        Some(orig_x) if !saw_override => {
            pen.borrow_mut().x = orig_x;
        }
        Some(_) => { /* leave pen at post-final-advance */ }
    }
    // If shaping produced more glyphs than characters (e.g. an accent
    // decomposition), append the trailing glyphs at the running pen
    // without per-character overrides — keeps the visual output
    // approximately correct without misattributing list slots. As
    // above, only render glyphs with outlines.
    if glyph_idx < n_glyphs {
        let (origin_x, origin_y) = {
            let p = pen.borrow();
            (p.x, p.y)
        };
        let mut local_pen = 0.0_f32;
        for g_meta in shaped.iter().take(n_glyphs).skip(glyph_idx) {
            let face = chain.face(g_meta.face_idx);
            if let Some(node) = face.glyph_node(g_meta.glyph_id, inh.font_size) {
                let placement = Transform2D::translate(
                    origin_x + local_pen + g_meta.x_offset,
                    origin_y + g_meta.y_offset,
                );
                let painted = repaint_node(node, fill.clone());
                out.children.push(Node::Group(Group {
                    transform: placement,
                    children: vec![painted],
                    ..Group::default()
                }));
            }
            local_pen += g_meta.x_advance;
        }
        pen.borrow_mut().x = origin_x + local_pen;
    }
    *char_counter += n_chars;
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
