//! Round 4 — encoder-side preservation of source XML elements that
//! `oxideav_core::Node` doesn't represent natively.
//!
//! The decoder produces a [`VectorFrame`] whose scene graph holds only
//! shapes / groups / soft-masks / images. Round 1-3 already throws
//! away the original `<style>`, `<filter>`, `<animate>` definitions —
//! they're either consumed (snapshot at t=0) or held only as parser
//! side tables. After `parse → write_svg`, a round-trip therefore loses
//! the dynamic / filter / CSS pieces.
//!
//! Round 4 introduces an out-of-band [`PreservedExtras`] container.
//! Callers who care about lossless round-tripping use the paired API:
//!
//! ```ignore
//! let (frame, extras) = oxideav_svg::parse_svg_with_extras(bytes)?;
//! // ...mutate frame...
//! let bytes = oxideav_svg::write_svg_with_extras(&frame, &extras);
//! ```
//!
//! `extras` carries each `<style>`, `<filter>`, `<animate>` (and
//! related) element in serialised XML form. The encoder splices them
//! back into the output document so the rasterised representation +
//! the dynamic definitions both survive.

use crate::parser::Element;

/// Side-channel buffer of source-XML fragments the encoder needs to
/// re-emit alongside the [`VectorFrame`] scene graph.
///
/// Populated by [`crate::decoder::parse_svg_with_extras`] during the
/// document pre-walk; consumed by
/// [`crate::encoder::write_svg_with_extras`].
#[derive(Clone, Debug, Default)]
pub struct PreservedExtras {
    /// `<style>` element bodies (CSS source). Kept verbatim so an
    /// inline `<style>` block survives round-trip even though the
    /// decoder also fold the rules into [`crate::css::Stylesheet`].
    pub styles: Vec<String>,
    /// `<filter>` element trees, captured verbatim. Filter primitives
    /// (`feGaussianBlur`, `feColorMatrix`, …) aren't yet rasterised
    /// here, but preserving them means downstream consumers still see
    /// the definition.
    pub filters: Vec<Element>,
    /// `<animate>` / `<set>` / `<animateTransform>` elements that were
    /// children of an id-bearing parent in the source document. Stored
    /// alongside the parent's id so the encoder can re-attach them to
    /// the correct emitted node.
    ///
    /// `parent_path` is a list of (tag_local, optional id) pairs from
    /// the document root down to the immediate parent of the animation
    /// — used by the encoder to route animation re-emission. Round 4
    /// only attaches animations to root-level path / group emit sites
    /// keyed by id; deeper or unkeyed parents are dropped (a
    /// followup).
    pub animations: Vec<AnimationFragment>,
    /// `<foreignObject>` element trees, captured verbatim. Round 4
    /// renders them as an empty group on the rasterised side; this
    /// preserves the source so XHTML embeddings survive.
    pub foreign_objects: Vec<Element>,
}

/// One captured animation child of a known-id parent element.
#[derive(Clone, Debug)]
pub struct AnimationFragment {
    /// `id` of the parent SVG element, e.g. the `id="rect1"` of the
    /// `<rect>` whose `<animate>` we captured. `None` if the parent
    /// had no id (in which case round 4 drops the fragment).
    pub parent_id: Option<String>,
    /// The animation element itself (one of `<animate>`, `<set>`,
    /// `<animateTransform>`, `<animateMotion>`).
    pub element: Element,
}

impl PreservedExtras {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` when the buffer holds nothing the encoder would emit.
    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
            && self.filters.is_empty()
            && self.animations.is_empty()
            && self.foreign_objects.is_empty()
    }
}
