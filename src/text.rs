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
//! - `text-anchor="middle|end"` (we always anchor at the start glyph).
//! - `<textPath href="#path">` (round 3+).
//! - Bidi.

use std::cell::RefCell;
use std::sync::OnceLock;

use oxideav_core::{Group, Node, Result, Transform2D};

use crate::element::{PaintState, ParseContext};
use crate::parser::{attr, tag_local, Element, Node as XmlNode};

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
pub fn parse_text_element(
    el: &Element,
    state: &PaintState,
    ctx: &mut ParseContext,
) -> Result<Option<Node>> {
    let _ = ctx; // unused; reserved for future paint-server resolution.
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
    walk_text_children(el, &inheritance, &pen, &mut group)?;
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
                walk_text_children(c, &sub, pen, out)?;
            }
            _ => {
                // Unknown nested element (a, textPath, …) — silently
                // skipped in round 2.
            }
        }
    }
    Ok(())
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
            crate::color::PaintValue::Reference(_) => None,
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
