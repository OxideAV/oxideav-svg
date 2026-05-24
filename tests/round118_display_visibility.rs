//! Round 118 — SVG 1.1 §11.5 `display` + `visibility` presentation
//! properties.
//!
//! - `display: none` removes the element AND its children from the
//!   rendering tree (no scene node at all). It is NOT inherited; it
//!   does not prevent the element from being *referenced* (a `<use>` of
//!   a `display:none` definition still renders).
//! - `visibility: hidden | collapse` keeps the element in the rendering
//!   tree (its geometry still contributes to bbox) but paints nothing.
//!   `visibility` IS inherited, so a descendant may flip it back to
//!   `visible`.
//!
//! Source of truth: `docs/image/svg/svg11-second-edition.pdf` §11.5
//! ("Controlling visibility").

use oxideav_core::{Node, PathNode};
use oxideav_svg::parse_svg;

/// Find every `Node::Path` in the scene graph, in pre-order.
fn all_paths(frame: &oxideav_core::VectorFrame) -> Vec<&PathNode> {
    fn walk<'a>(n: &'a Node, out: &mut Vec<&'a PathNode>) {
        match n {
            Node::Path(p) => out.push(p),
            Node::Group(g) => {
                for c in &g.children {
                    walk(c, out);
                }
            }
            Node::SoftMask { content, .. } => walk(content, out),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &frame.root.children {
        walk(c, &mut out);
    }
    out
}

// ---------------------------------------------------------------------
// display: none
// ---------------------------------------------------------------------

#[test]
fn display_none_attribute_drops_shape() {
    // §11.5: "A value of display: none indicates that the given element
    // and its children shall not be rendered directly (i.e., those
    // elements are not present in the rendering tree)."
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
  <rect x="0" y="0" width="10" height="10" fill="#00ff00" display="none"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1, "display:none rect must not be in the tree");
}

#[test]
fn display_inline_renders_normally() {
    // Any value other than `none`/`inherit` means "rendered".
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="10" height="10" fill="#ff0000" display="inline"/>
  <rect x="0" y="0" width="10" height="10" fill="#00ff00" display="block"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(all_paths(&frame).len(), 2);
}

#[test]
fn display_none_on_group_drops_whole_subtree() {
    // "When applied to a container element, setting display to none
    // causes the container and all of its children to be invisible."
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <g display="none">
    <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
    <circle cx="5" cy="5" r="3" fill="#00ff00"/>
  </g>
  <rect x="0" y="0" width="10" height="10" fill="#0000ff"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1, "only the sibling rect survives");
}

#[test]
fn display_none_via_css_drops_shape() {
    // The cascade (CSS rule) must drive `display` exactly like the
    // presentation attribute.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <style>.hidden { display: none; }</style>
  <rect class="hidden" x="0" y="0" width="10" height="10" fill="#ff0000"/>
  <rect x="0" y="0" width="10" height="10" fill="#00ff00"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(all_paths(&frame).len(), 1);
}

#[test]
fn display_is_not_inherited() {
    // §11.5: `display` Inherited: no. A `<g display="none">` removes the
    // group, so a child cannot un-hide via the subtree — but a child of
    // a *rendered* group must NOT inherit a `display:none` from a
    // distant ancestor scope. Here a plain group sets display:none on
    // itself only; a deeper rect with no display renders fine in a
    // sibling group.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <g>
    <rect x="0" y="0" width="10" height="10" fill="#ff0000" display="none"/>
    <rect x="0" y="0" width="10" height="10" fill="#00ff00"/>
  </g>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    // The display:none rect is dropped; its sibling (which does NOT
    // inherit display:none) renders.
    assert_eq!(all_paths(&frame).len(), 1);
}

#[test]
fn display_none_definition_still_referenced_by_use() {
    // §11.5: "setting display: none on a path element will prevent that
    // element from getting rendered directly onto the canvas, but the
    // path element can still be referenced." A `<use>` of a
    // display:none definition must render.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <defs>
    <rect id="box" x="0" y="0" width="10" height="10" fill="#ff0000" display="none"/>
  </defs>
  <use href="#box"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(
        paths.len(),
        1,
        "a <use> of a display:none definition still renders"
    );
}

#[test]
fn use_instance_root_exempt_but_nested_display_none_still_drops() {
    // The §11.5 exemption applies to the *referenced* element (the
    // instance root). A `display:none` *descendant* inside the
    // instantiated group still drops, exactly as a direct render would.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <defs>
    <g id="grp" display="none">
      <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
      <rect x="0" y="0" width="10" height="10" fill="#00ff00" display="none"/>
    </g>
  </defs>
  <use href="#grp"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    // The group root is exempt (renders); its inner display:none rect
    // still drops, so only the first rect survives.
    assert_eq!(paths.len(), 1);
}

// ---------------------------------------------------------------------
// visibility: hidden / collapse
// ---------------------------------------------------------------------

#[test]
fn visibility_hidden_keeps_node_but_paints_nothing() {
    // §11.5: hidden element "is invisible (i.e., nothing is painted on
    // the canvas)" but "processing occurs as if the element were part
    // of the rendering tree" — so the Path node survives with no paint.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="10" height="10" fill="#ff0000" visibility="hidden"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1, "hidden node stays in the tree");
    assert!(paths[0].fill.is_none(), "hidden element paints no fill");
    assert!(paths[0].stroke.is_none(), "hidden element paints no stroke");
}

#[test]
fn visibility_collapse_is_treated_as_hidden() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="10" height="10" fill="#ff0000" stroke="#00ff00" stroke-width="2" visibility="collapse"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1);
    assert!(paths[0].fill.is_none());
    assert!(paths[0].stroke.is_none());
}

#[test]
fn visibility_inherits_then_child_overrides_to_visible() {
    // §11.5: "Setting visibility:hidden on a g will make its children
    // invisible as long as the children do not specify their own
    // visibility properties as visible."
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <g visibility="hidden">
    <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
    <rect x="0" y="0" width="10" height="10" fill="#00ff00" visibility="visible"/>
  </g>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 2, "both shapes stay in the tree");
    // First inherits hidden → no fill; second overrides to visible.
    let painted: Vec<bool> = paths.iter().map(|p| p.fill.is_some()).collect();
    assert!(
        painted.contains(&false) && painted.contains(&true),
        "one inherited-hidden (no fill), one visible (painted): {painted:?}"
    );
}

#[test]
fn visibility_hidden_via_css() {
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <style>rect { visibility: hidden; }</style>
  <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1);
    assert!(
        paths[0].fill.is_none(),
        "CSS visibility:hidden paints nothing"
    );
}

#[test]
fn visibility_visible_default_paints() {
    // Sanity: an element with no visibility (default visible) still
    // paints — confirms the suppression is gated correctly.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="10" height="10" fill="#ff0000"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    let paths = all_paths(&frame);
    assert_eq!(paths.len(), 1);
    assert!(
        paths[0].fill.is_some(),
        "default visible element is painted"
    );
}

#[test]
fn display_none_beats_visibility_on_same_element() {
    // display:none removes the node entirely, regardless of visibility.
    let src = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
  <rect x="0" y="0" width="10" height="10" fill="#ff0000" display="none" visibility="visible"/>
</svg>"##;
    let frame = parse_svg(src).unwrap();
    assert_eq!(all_paths(&frame).len(), 0);
}
