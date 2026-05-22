//! Round 95 — SVG 2 §16.3 fragment-identifier routing.
//!
//! The URL fragment that follows the `#` in an SVG reference
//! (`MyDrawing.svg#MyView`) selects which "initial view" into the
//! document the renderer should use. The full SVG 2 §16.3.2 grammar
//! covers four input shapes:
//!
//! 1. **Bare name** (`#MyView`) — addresses an element by its `id`.
//!    When that element is a `<view>`, the `<view>`'s `viewBox` /
//!    `preserveAspectRatio` / `zoomAndPan` override the root `<svg>`
//!    attributes per §16.3.2 / §16.3.3.
//! 2. **`svgView(...)` specification** —
//!    `#svgView(viewBox(0,0,200,200);preserveAspectRatio(xMidYMid))`
//!    spells the four `SVGViewAttribute`s inline (`viewBox(...)` /
//!    `preserveAspectRatio(...)` / `transform(...)` /
//!    `zoomAndPan(...)`), semicolon-separated, in any order, each at
//!    most once. Specified attributes override the root `<svg>`; any
//!    unspecified attribute inherits the root value (per the §16.3.2
//!    "unspecified parameters of the svgView specification don't reset
//!    the values defined on the root `<svg>`" rule).
//! 3. **Spatial media fragment** (`#xywh=...`) — out of scope for
//!    round 95; spec also references this case as a separate space
//!    segment.
//! 4. **Temporal media fragment** (`#t=...`) — out of scope.
//!
//! [`resolve_fragment`] returns a typed [`ResolvedView`] for the two
//! supported shapes plus the no-op default (empty / `#t=` / `#xywh=`
//! fall through with the document's root view attributes).
//!
//! The function is wall-clean: it consumes only the [`VectorFrame`]
//! (for root `viewBox` + dimensions) and the
//! [`PreservedExtras::typed_views`](crate::preserved::PreservedExtras::typed_views)
//! map (for `<view>` lookups). It performs no XML re-parse — the
//! decoder already extracted everything it needs.

use oxideav_core::{Transform2D, VectorFrame, ViewBox};

use crate::defs::ZoomAndPan;
use crate::filter::PreserveAspectRatio;
use crate::preserved::PreservedExtras;

/// Resolved initial-view parameters per SVG 2 §16.3.2. Returned by
/// [`resolve_fragment`].
///
/// Every field falls back to the document root's value when the
/// fragment didn't specify it (`#MyView` populates only the
/// attributes the matching `<view>` carried; `#svgView(viewBox(...))`
/// only fills `view_box`; etc.). Callers that want a "spec-correct
/// initial render" treat the returned values as overrides for the
/// matching root attributes.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedView {
    /// Resolved `viewBox` after applying the fragment. Falls back to
    /// the document root's `viewBox` when neither the fragment nor the
    /// addressed `<view>` overrides it.
    pub view_box: Option<ViewBox>,
    /// Resolved `preserveAspectRatio` after applying the fragment.
    /// Defaults to the SVG 2 §8.2 spec default (`xMidYMid meet`) when
    /// the root `<svg>` had no `preserveAspectRatio` attribute and no
    /// override is supplied.
    pub preserve_aspect_ratio: PreserveAspectRatio,
    /// Resolved `zoomAndPan` after applying the fragment. Defaults to
    /// [`ZoomAndPan::Magnify`] per SVG 2 §16.3.3.
    pub zoom_and_pan: ZoomAndPan,
    /// Extra `transform(...)` from the `svgView` specification, when
    /// supplied. Composed *after* the viewBox mapping per §16.3.2
    /// ("SVG view box parameters are applied in order ... as defined
    /// in ViewBoxParams, then as defined in TransformParams").
    /// `None` when no `transform(...)` was specified.
    pub transform: Option<Transform2D>,
}

impl Default for ResolvedView {
    fn default() -> Self {
        Self {
            view_box: None,
            preserve_aspect_ratio: PreserveAspectRatio::default(),
            zoom_and_pan: ZoomAndPan::Magnify,
            transform: None,
        }
    }
}

/// Resolve an SVG fragment identifier against the document's root view
/// parameters and the side-channel `<view>` table.
///
/// `fragment` is the substring of the URL after the `#` character (the
/// caller strips the leading `#` themselves). An empty fragment, a
/// spatial / temporal media fragment that this function doesn't yet
/// recognise, or a `svgView(...)` whose interior is malformed, all
/// degrade gracefully to a [`ResolvedView`] holding the document root's
/// own attributes (matching the spec's "if the SVG fragment identifier
/// addresses a time segment ... the initial view ... is established as
/// if no fragment identifier was provided" lenient fallback).
///
/// The function is pure — it never mutates `frame` or `extras`.
pub fn resolve_fragment(
    frame: &VectorFrame,
    extras: &PreservedExtras,
    fragment: &str,
) -> ResolvedView {
    let frag = fragment.trim();
    // Strip a leading `#` so callers can pass either the raw URL fragment
    // or the post-`#` payload.
    let frag = frag.strip_prefix('#').unwrap_or(frag);
    // Root-attribute baseline. Populated for every code path so an
    // unspecified attribute on a `<view>` (or omitted `svgView`
    // parameter) inherits the root value per §16.3.2.
    let root_par = extras
        .root_preserve_aspect_ratio
        .as_deref()
        .map(PreserveAspectRatio::from_str)
        .unwrap_or_default();
    let baseline = ResolvedView {
        view_box: frame.view_box,
        preserve_aspect_ratio: root_par,
        zoom_and_pan: ZoomAndPan::Magnify,
        transform: None,
    };
    if frag.is_empty() {
        return baseline;
    }
    // §16.3.2 — `svgView(...)` form.
    if let Some(inner) = frag
        .strip_prefix("svgView(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return apply_svg_view_spec(baseline, inner);
    }
    // §16.3.2 spatial / temporal media-fragment forms — out of scope
    // for round 95; degrade to the baseline (matching the spec's
    // temporal-segment rule "as if no fragment identifier was
    // provided"). We probe for the well-known `xywh=` / `t=` /
    // `track=` / `id=` prefixes so a future round can pick them off
    // without disturbing the existing dispatch.
    if frag.starts_with("xywh=")
        || frag.starts_with("t=")
        || frag.starts_with("track=")
        || frag.starts_with("id=")
    {
        return baseline;
    }
    // §16.3.2 — bare-name form. The name addresses an element by id.
    // When it's a `<view>`, the typed table tells us which root
    // attributes to override; otherwise the spec says "the initial
    // view ... is established using the view specification attributes
    // on the outermost svg element," so we return the baseline.
    if let Some(view) = extras.typed_views.get(frag) {
        return apply_view_def(baseline, view);
    }
    baseline
}

/// Apply a typed [`crate::defs::ViewDef`] over the baseline. Any
/// attribute the `<view>` specified replaces the baseline's; anything
/// the `<view>` left out inherits the baseline (per §16.3.2 "Any view
/// specification attributes included on the given `<view>` element
/// override the corresponding view specification attributes on the
/// root `<svg>` element").
fn apply_view_def(mut baseline: ResolvedView, view: &crate::defs::ViewDef) -> ResolvedView {
    if let Some(vb) = view.view_box {
        baseline.view_box = Some(vb);
    }
    if let Some(par) = view.preserve_aspect_ratio {
        baseline.preserve_aspect_ratio = par;
    }
    if let Some(zap) = view.zoom_and_pan {
        baseline.zoom_and_pan = zap;
    }
    baseline
}

/// Parse the interior of an `svgView(...)` spec and apply each typed
/// attribute over the baseline. Per §16.3.2 the four attribute forms
/// (`viewBox` / `preserveAspectRatio` / `transform` / `zoomAndPan`)
/// are semicolon-separated, may appear in any order, and each at most
/// once. Whitespace + url-escaped (`%XX`) bytes are tolerated; an
/// unknown attribute name is dropped silently (matches the spec's
/// "tolerant ignore unknown" rule).
fn apply_svg_view_spec(mut baseline: ResolvedView, inner: &str) -> ResolvedView {
    // The spec accepts both literal `;` and `%3B` as separators
    // (CSSOM-style url-escaping). Normalising here keeps the splitter
    // simple.
    let normalised = inner.replace("%3B", ";").replace("%3b", ";");
    for attr in normalised.split(';') {
        let attr = attr.trim();
        if attr.is_empty() {
            continue;
        }
        let (name, payload) = match attr.split_once('(') {
            Some((n, p)) => (n.trim(), p),
            None => continue, // malformed; skip
        };
        let payload = match payload.strip_suffix(')') {
            Some(p) => p.trim(),
            None => continue,
        };
        match name {
            "viewBox" => {
                if let Some(vb) = parse_view_box_payload(payload) {
                    baseline.view_box = Some(vb);
                }
            }
            "preserveAspectRatio" => {
                baseline.preserve_aspect_ratio = PreserveAspectRatio::from_str(payload);
            }
            "zoomAndPan" => {
                baseline.zoom_and_pan = ZoomAndPan::from_str(payload);
            }
            "transform" => {
                // Round 95 honours the spec's `transform(...)` only for
                // the canonical translate/rotate/scale set the existing
                // [`crate::transform::parse_transform`] already
                // understands. Anything it doesn't recognise drops
                // silently to the baseline.
                if let Ok(t) = crate::transform::parse_transform(payload) {
                    baseline.transform = Some(t);
                }
            }
            _ => {}
        }
    }
    baseline
}

/// Parse a comma-or-whitespace separated four-number `viewBox` payload.
fn parse_view_box_payload(s: &str) -> Option<ViewBox> {
    let nums: Vec<f32> = s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .filter_map(|n| n.parse::<f32>().ok())
        .collect();
    if nums.len() != 4 {
        return None;
    }
    Some(ViewBox {
        min_x: nums[0],
        min_y: nums[1],
        width: nums[2],
        height: nums[3],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::ViewDef;
    use oxideav_core::{Group, TimeBase};

    fn frame_with_root_viewbox(vb: Option<ViewBox>) -> VectorFrame {
        VectorFrame {
            width: 100.0,
            height: 100.0,
            view_box: vb,
            root: Group::default(),
            pts: None,
            time_base: TimeBase::new(1, 1),
        }
    }

    #[test]
    fn empty_fragment_returns_root_baseline() {
        let frame = frame_with_root_viewbox(Some(ViewBox {
            min_x: 0.0,
            min_y: 0.0,
            width: 100.0,
            height: 100.0,
        }));
        let extras = PreservedExtras::new();
        let r = resolve_fragment(&frame, &extras, "");
        assert_eq!(r.view_box.unwrap().width, 100.0);
        assert_eq!(r.zoom_and_pan, ZoomAndPan::Magnify);
        assert!(r.transform.is_none());
    }

    #[test]
    fn bare_name_overrides_root_viewbox() {
        let frame = frame_with_root_viewbox(Some(ViewBox {
            min_x: 0.0,
            min_y: 0.0,
            width: 100.0,
            height: 100.0,
        }));
        let mut extras = PreservedExtras::new();
        extras.typed_views.insert(
            "zoom".into(),
            ViewDef {
                view_box: Some(ViewBox {
                    min_x: 10.0,
                    min_y: 20.0,
                    width: 30.0,
                    height: 40.0,
                }),
                ..Default::default()
            },
        );
        let r = resolve_fragment(&frame, &extras, "zoom");
        let vb = r.view_box.unwrap();
        assert_eq!(vb.min_x, 10.0);
        assert_eq!(vb.width, 30.0);
        assert_eq!(vb.height, 40.0);
    }

    #[test]
    fn bare_name_inherits_unspecified_attrs_from_root() {
        let frame = frame_with_root_viewbox(Some(ViewBox {
            min_x: 0.0,
            min_y: 0.0,
            width: 100.0,
            height: 100.0,
        }));
        let mut extras = PreservedExtras::new();
        // Root `<svg>` had `preserveAspectRatio="xMaxYMax slice"`.
        extras.root_preserve_aspect_ratio = Some("xMaxYMax slice".into());
        // The `<view>` overrides only viewBox.
        extras.typed_views.insert(
            "v".into(),
            ViewDef {
                view_box: Some(ViewBox {
                    min_x: 5.0,
                    min_y: 5.0,
                    width: 50.0,
                    height: 50.0,
                }),
                ..Default::default()
            },
        );
        let r = resolve_fragment(&frame, &extras, "v");
        // viewBox overridden by the <view>.
        assert_eq!(r.view_box.unwrap().min_x, 5.0);
        // preserveAspectRatio still the root's because the <view>
        // didn't specify it.
        let par = r.preserve_aspect_ratio;
        assert!(matches!(
            par.align,
            crate::filter::PreserveAspectRatioAlign::XMaxYMax
        ));
    }

    #[test]
    fn svg_view_spec_overrides_root_viewbox() {
        let frame = frame_with_root_viewbox(Some(ViewBox {
            min_x: 0.0,
            min_y: 0.0,
            width: 100.0,
            height: 100.0,
        }));
        let extras = PreservedExtras::new();
        let r = resolve_fragment(&frame, &extras, "svgView(viewBox(0,200,1000,1000))");
        let vb = r.view_box.unwrap();
        assert_eq!(vb.min_y, 200.0);
        assert_eq!(vb.width, 1000.0);
    }

    #[test]
    fn svg_view_spec_honours_multiple_attrs_in_any_order() {
        let frame = frame_with_root_viewbox(None);
        let extras = PreservedExtras::new();
        let r = resolve_fragment(
            &frame,
            &extras,
            "svgView(zoomAndPan(disable);viewBox(1 2 3 4);preserveAspectRatio(none))",
        );
        let vb = r.view_box.unwrap();
        assert_eq!(vb.min_x, 1.0);
        assert_eq!(vb.height, 4.0);
        assert_eq!(r.zoom_and_pan, ZoomAndPan::Disable);
        assert!(matches!(
            r.preserve_aspect_ratio.align,
            crate::filter::PreserveAspectRatioAlign::None
        ));
    }

    #[test]
    fn svg_view_spec_tolerates_percent_encoded_semicolons() {
        let frame = frame_with_root_viewbox(None);
        let extras = PreservedExtras::new();
        let r = resolve_fragment(
            &frame,
            &extras,
            "svgView(viewBox(0,0,10,10)%3BzoomAndPan(disable))",
        );
        assert_eq!(r.view_box.unwrap().width, 10.0);
        assert_eq!(r.zoom_and_pan, ZoomAndPan::Disable);
    }

    #[test]
    fn svg_view_spec_drops_unknown_attribute() {
        let frame = frame_with_root_viewbox(None);
        let extras = PreservedExtras::new();
        // `viewport(...)` is not a §16.3.2 attribute — must be dropped
        // silently without affecting the recognised `viewBox(...)`.
        let r = resolve_fragment(
            &frame,
            &extras,
            "svgView(viewport(1,2,3,4);viewBox(5,6,7,8))",
        );
        let vb = r.view_box.unwrap();
        assert_eq!(vb.min_x, 5.0);
        assert_eq!(vb.height, 8.0);
    }

    #[test]
    fn svg_view_spec_drops_malformed_viewbox() {
        let frame = frame_with_root_viewbox(Some(ViewBox {
            min_x: 0.0,
            min_y: 0.0,
            width: 100.0,
            height: 100.0,
        }));
        let extras = PreservedExtras::new();
        // 3 numbers instead of 4 — malformed viewBox payload; baseline
        // root view-box must survive.
        let r = resolve_fragment(&frame, &extras, "svgView(viewBox(1,2,3))");
        assert_eq!(r.view_box.unwrap().width, 100.0);
    }

    #[test]
    fn temporal_or_spatial_media_fragment_falls_back_to_baseline() {
        let frame = frame_with_root_viewbox(Some(ViewBox {
            min_x: 0.0,
            min_y: 0.0,
            width: 100.0,
            height: 100.0,
        }));
        let extras = PreservedExtras::new();
        let r_t = resolve_fragment(&frame, &extras, "t=5,10");
        let r_sp = resolve_fragment(&frame, &extras, "xywh=0,0,50,50");
        assert_eq!(r_t.view_box.unwrap().width, 100.0);
        assert_eq!(r_sp.view_box.unwrap().width, 100.0);
    }

    #[test]
    fn unknown_bare_name_falls_back_to_baseline() {
        let frame = frame_with_root_viewbox(Some(ViewBox {
            min_x: 0.0,
            min_y: 0.0,
            width: 100.0,
            height: 100.0,
        }));
        let extras = PreservedExtras::new();
        let r = resolve_fragment(&frame, &extras, "this-id-does-not-exist");
        // Falls through to the baseline since the spec says "if the
        // SVG fragment identifier addresses a view element ..." — when
        // no view matches the name we behave as if no fragment was
        // provided.
        assert_eq!(r.view_box.unwrap().width, 100.0);
    }

    #[test]
    fn leading_hash_is_stripped() {
        let frame = frame_with_root_viewbox(None);
        let mut extras = PreservedExtras::new();
        extras.typed_views.insert(
            "v".into(),
            ViewDef {
                view_box: Some(ViewBox {
                    min_x: 0.0,
                    min_y: 0.0,
                    width: 7.0,
                    height: 7.0,
                }),
                ..Default::default()
            },
        );
        let r = resolve_fragment(&frame, &extras, "#v");
        assert_eq!(r.view_box.unwrap().width, 7.0);
    }

    #[test]
    fn svg_view_spec_transform_is_captured() {
        let frame = frame_with_root_viewbox(None);
        let extras = PreservedExtras::new();
        let r = resolve_fragment(&frame, &extras, "svgView(transform(scale(5)))");
        let t = r.transform.expect("transform should be captured");
        // SVG `scale(5)` parses to a 5× uniform scale in
        // Transform2D's `a/d` slots (per
        // [`oxideav_core::Transform2D::scale`]). Confirms the
        // `transform(...)` payload reached the matrix parser.
        assert!((t.a - 5.0).abs() < 1e-6);
        assert!((t.d - 5.0).abs() < 1e-6);
    }
}
