//! [`VectorFrame`] → SVG bytes encoder.
//!
//! Round 1 emits one `<path>` per `PathNode` (lossless preservation of
//! the exact command sequence the decoder produces) plus a flat
//! `<defs>` block at the top of the document carrying every gradient
//! used by any descendant. Groups round-trip as `<g>` with their
//! transform and opacity.

use std::collections::HashMap;

use oxideav_core::{
    DashPattern, Encoder, Error, FillRule, Frame, Group, LineCap, LineJoin, LinearGradient,
    MaskKind, Node, Packet, Paint, Path, PathCommand, PathNode, Point, RadialGradient, Result,
    Rgba, SpreadMethod, TimeBase, Transform2D, VectorFrame,
};

use crate::decoder::CODEC_ID_STR;
use crate::parser::{escape_attr, Element, Node as XmlNode};
use crate::preserved::{
    AnimationFragment, DescriptiveBinding, DescriptiveText, LinkBinding, PreservedExtras,
};

/// Round 3: serialise a [`VectorFrame`] into a gzip-compressed
/// `.svgz` byte buffer. Equivalent to `gzip(write_svg(frame))`.
pub fn write_svgz(frame: &VectorFrame) -> Result<Vec<u8>> {
    let xml = write_svg(frame);
    crate::parser::deflate_gzip(&xml)
}

/// Serialise a [`VectorFrame`] into a UTF-8 SVG byte buffer.
///
/// Equivalent to `write_svg_with_extras(frame, &PreservedExtras::default())`.
pub fn write_svg(frame: &VectorFrame) -> Vec<u8> {
    write_svg_with_extras(frame, &PreservedExtras::default())
}

/// Round 449 — the read-only lookup tables threaded through the
/// recursive scene-graph emission (`write_group_children` /
/// `write_node`). Each map indexes a [`PreservedExtras`] side-channel
/// by scene-graph tree-path (or by parent id for the round-13
/// animation routing) exactly as the per-round build comments in
/// [`write_svg_with_extras`] describe; bundling them in one struct
/// keeps the recursion signature stable as side-channels accrue.
#[derive(Default)]
struct EmitIndex<'a> {
    path_to_id: HashMap<Vec<usize>, String>,
    path_to_path_length: HashMap<Vec<usize>, f32>,
    path_to_link: HashMap<Vec<usize>, &'a LinkBinding>,
    path_to_paint_order: HashMap<Vec<usize>, &'a str>,
    path_to_vector_effect: HashMap<Vec<usize>, &'a str>,
    path_to_shape_rendering: HashMap<Vec<usize>, &'a str>,
    path_to_text_rendering: HashMap<Vec<usize>, &'a str>,
    path_to_color_rendering: HashMap<Vec<usize>, &'a str>,
    path_to_color_interpolation: HashMap<Vec<usize>, &'a str>,
    path_to_overflow: HashMap<Vec<usize>, &'a str>,
    path_to_pointer_events: HashMap<Vec<usize>, &'a str>,
    path_to_cursor: HashMap<Vec<usize>, &'a str>,
    path_to_dominant_baseline: HashMap<Vec<usize>, &'a str>,
    path_to_use: HashMap<Vec<usize>, &'a crate::preserved::UseBinding>,
    path_to_switch: HashMap<Vec<usize>, &'a crate::preserved::SwitchBinding>,
    path_to_text: HashMap<Vec<usize>, &'a crate::preserved::TextBinding>,
    path_to_filter_ref: HashMap<Vec<usize>, &'a str>,
    path_to_marker: HashMap<Vec<usize>, &'a crate::preserved::MarkerRefBinding>,
    parent_to_titles: HashMap<Vec<usize>, &'a DescriptiveBinding>,
    parent_to_descs: HashMap<Vec<usize>, &'a DescriptiveBinding>,
    anim_by_parent: HashMap<String, Vec<&'a AnimationFragment>>,
}

/// Round 4 — serialise a [`VectorFrame`] *and* re-emit every preserved
/// `<style>` / `<filter>` / `<animate>` / `<foreignObject>` fragment
/// supplied in `extras`. Pair with
/// [`crate::decoder::parse_svg_with_extras`] for a structural
/// round-trip that doesn't lose CSS / filter / animation definitions.
///
/// Round 13 — when `extras.id_paths` is populated, each scene-graph
/// node whose tree-path matches a recorded entry is emitted with the
/// original `id="..."` attribute and any captured `<animate>` /
/// `<set>` / `<animateTransform>` whose `parent_id == id` is inlined
/// as a child of that node (instead of dumped at the trailing edge of
/// the document with a parent-id comment hint).
pub fn write_svg_with_extras(frame: &VectorFrame, extras: &PreservedExtras) -> Vec<u8> {
    // Round 13 — index id_paths by `Vec<usize>` for O(1) per-node
    // lookup, and group animations by `parent_id` so we can drain a
    // single id's children inline.
    let mut path_to_id: HashMap<Vec<usize>, String> = HashMap::new();
    for entry in &extras.id_paths {
        path_to_id.insert(entry.path.clone(), entry.id.clone());
    }
    // Round 21 — index the side-channel `pathLength` bindings by
    // scene-graph tree-path so `write_node` can re-emit
    // `pathLength="..."` on the matching `<path>` / `<rect>` /
    // `<circle>` / `<ellipse>` / `<line>` / `<polyline>` /
    // `<polygon>` emit site. The dasharray on the shape was already
    // rescaled to user-units at parse time, so emitting the original
    // `pathLength` lets a downstream renderer that consumes both
    // attributes still produce the spec-correct dash pattern after
    // re-normalising.
    let mut path_to_path_length: HashMap<Vec<usize>, f32> = HashMap::new();
    for entry in &extras.path_lengths {
        path_to_path_length.insert(entry.path.clone(), entry.path_length);
    }
    // Round 115 — index `<a>` hyperlink bindings (SVG 2 §16.5) by
    // scene-graph tree-path so `write_node`'s `Node::Group` arm can
    // re-wrap the matching `<g>` in its `<a href="…">…</a>` element on
    // round-trip.
    let mut path_to_link: HashMap<Vec<usize>, &LinkBinding> = HashMap::new();
    for entry in &extras.links {
        path_to_link.insert(entry.path.clone(), entry);
    }
    // Round 205 — index `paint-order` side-channel bindings by
    // scene-graph tree-path so `write_node` can re-emit
    // `paint-order="..."` on the matching shape on round-trip.
    let mut path_to_paint_order: HashMap<Vec<usize>, &str> = HashMap::new();
    for entry in &extras.paint_orders {
        path_to_paint_order.insert(entry.path.clone(), entry.paint_order.as_str());
    }
    // Round 209 — index `vector-effect` side-channel bindings (SVG 2
    // §8.13) by scene-graph tree-path so `write_node` can re-emit
    // `vector-effect="..."` on the matching shape / `<use>` group on
    // round-trip. Same per-path index layout as the round-205
    // `paint-order` map; same routing through `write_node` /
    // `write_group_children`.
    let mut path_to_vector_effect: HashMap<Vec<usize>, &str> = HashMap::new();
    for entry in &extras.vector_effects {
        path_to_vector_effect.insert(entry.path.clone(), entry.vector_effect.as_str());
    }
    // Round 221 — index `shape-rendering` side-channel bindings (SVG 2
    // §13.10.2) by scene-graph tree-path so `write_node` can re-emit
    // `shape-rendering="..."` on the matching shape / `<g>` on
    // round-trip. Same per-path index layout as the round-205 /
    // round-209 maps; same routing through `write_node` /
    // `write_group_children`.
    let mut path_to_shape_rendering: HashMap<Vec<usize>, &str> = HashMap::new();
    for entry in &extras.shape_renderings {
        path_to_shape_rendering.insert(entry.path.clone(), entry.shape_rendering.as_str());
    }
    // Round 228 — index `text-rendering` side-channel bindings (SVG 2
    // §13.10.3) by scene-graph tree-path so `write_node` can re-emit
    // `text-rendering="..."` on the matching `<text>` / `<g>` on
    // round-trip. Same per-path index layout as the round-221
    // `shape-rendering` map; same routing through `write_node` /
    // `write_group_children`.
    let mut path_to_text_rendering: HashMap<Vec<usize>, &str> = HashMap::new();
    for entry in &extras.text_renderings {
        path_to_text_rendering.insert(entry.path.clone(), entry.text_rendering.as_str());
    }
    // Round 247 — index `color-rendering` side-channel bindings (SVG 2
    // §13.10.1) by scene-graph tree-path so `write_node` can re-emit
    // `color-rendering="..."` on the matching shape / `<g>` on
    // round-trip. Same per-path index layout as the round-221 /
    // round-228 maps; same routing through `write_node` /
    // `write_group_children`.
    let mut path_to_color_rendering: HashMap<Vec<usize>, &str> = HashMap::new();
    for entry in &extras.color_renderings {
        path_to_color_rendering.insert(entry.path.clone(), entry.color_rendering.as_str());
    }
    // Round 252 — index `color-interpolation` side-channel bindings
    // (SVG 2 §13.9) by scene-graph tree-path so `write_node` can
    // re-emit `color-interpolation="..."` on the matching shape /
    // `<g>` on round-trip. Same per-path index layout as the
    // round-247 `color-rendering` map; same routing through
    // `write_node` / `write_group_children`.
    let mut path_to_color_interpolation: HashMap<Vec<usize>, &str> = HashMap::new();
    for entry in &extras.color_interpolations {
        path_to_color_interpolation.insert(entry.path.clone(), entry.color_interpolation.as_str());
    }
    // Round 257 — index `overflow` side-channel bindings (SVG 2
    // §3.11) by scene-graph tree-path so `write_node` can re-emit
    // `overflow="..."` on the matching shape / `<g>` on round-trip.
    // Same per-path index layout as the round-252 `color-interpolation`
    // map; same routing through `write_node` / `write_group_children`.
    let mut path_to_overflow: HashMap<Vec<usize>, &str> = HashMap::new();
    for entry in &extras.overflows {
        path_to_overflow.insert(entry.path.clone(), entry.overflow.as_str());
    }
    // Round 260 — index `pointer-events` side-channel bindings (SVG 2
    // §15.6) by scene-graph tree-path so `write_node` can re-emit
    // `pointer-events="..."` on the matching shape / `<g>` on
    // round-trip. Same per-path index layout as the round-257
    // `overflow` map; same routing through `write_node` /
    // `write_group_children`.
    let mut path_to_pointer_events: HashMap<Vec<usize>, &str> = HashMap::new();
    for entry in &extras.pointer_eventss {
        path_to_pointer_events.insert(entry.path.clone(), entry.pointer_events.as_str());
    }
    // Round 261 — index `cursor` side-channel bindings (SVG 1.1
    // §16.8.2) by scene-graph tree-path so `write_node` can re-emit
    // `cursor="..."` on the matching shape / `<g>` on round-trip.
    // Same per-path index layout as the round-260 `pointer-events`
    // map; same routing through `write_node` / `write_group_children`.
    let mut path_to_cursor: HashMap<Vec<usize>, &str> = HashMap::new();
    for entry in &extras.cursors {
        path_to_cursor.insert(entry.path.clone(), entry.cursor.as_str());
    }
    // Round 291 — index `dominant-baseline` side-channel bindings
    // (SVG 1.1 §10.9.2) by scene-graph tree-path so `write_node` can
    // re-emit `dominant-baseline="..."` on the matching shape / `<g>`
    // on round-trip. Same per-path index layout as the round-257
    // `overflow` map; same routing through `write_node` /
    // `write_group_children`.
    let mut path_to_dominant_baseline: HashMap<Vec<usize>, &str> = HashMap::new();
    for entry in &extras.dominant_baselines {
        path_to_dominant_baseline.insert(entry.path.clone(), entry.dominant_baseline.as_str());
    }
    // Round 372 — index `<use>` reference bindings (SVG 2 §5.6) by
    // scene-graph tree-path so `write_node` can collapse the matching
    // instantiated `Node::Group` back to `<use href="#id" …/>` (and
    // skip its flattened children) on round-trip.
    let mut path_to_use: HashMap<Vec<usize>, &crate::preserved::UseBinding> = HashMap::new();
    for entry in &extras.uses {
        path_to_use.insert(entry.path.clone(), entry);
    }
    // Round 372 — index `<switch>` verbatim bindings (SVG 2 §5.7) by
    // scene-graph tree-path so `write_node` can collapse the selected-
    // branch `Node::Group` back to the verbatim `<switch>` (all
    // alternatives) on round-trip.
    let mut path_to_switch: HashMap<Vec<usize>, &crate::preserved::SwitchBinding> = HashMap::new();
    for entry in &extras.switches {
        path_to_switch.insert(entry.path.clone(), entry);
    }
    // Round 449 — index `<text>` verbatim bindings (SVG 2 §11.2) by
    // scene-graph tree-path so `write_node` can replace the matching
    // flattened glyph-outline node with the source `<text>…</text>`
    // (string content, font properties, `<tspan>` positioning arrays,
    // `<textPath>`, animation children) on round-trip.
    let mut path_to_text: HashMap<Vec<usize>, &crate::preserved::TextBinding> = HashMap::new();
    for entry in &extras.texts {
        path_to_text.insert(entry.path.clone(), entry);
    }
    // Round 372 — index `filter="url(#id)"` reference bindings (SVG 1.1
    // §15) by scene-graph tree-path so `write_node`'s `Node::Group` arm
    // can re-emit `filter=` on the matching filter-wrapper `<g>`,
    // reconnecting the graphics element to its preserved `<filter>` def.
    let mut path_to_filter_ref: HashMap<Vec<usize>, &str> = HashMap::new();
    for entry in &extras.filter_refs {
        path_to_filter_ref.insert(entry.path.clone(), entry.filter.as_str());
    }
    // Round 372 — index `marker-*` reference bindings (SVG 2 §13.7.4) by
    // scene-graph tree-path so `write_node` can re-emit the vertex-marker
    // references on the matching shape / `<g>`, reconnecting it to its
    // preserved `<marker>` def.
    let mut path_to_marker: HashMap<Vec<usize>, &crate::preserved::MarkerRefBinding> =
        HashMap::new();
    for entry in &extras.marker_refs {
        path_to_marker.insert(entry.path.clone(), entry);
    }
    // Round 122 — index `<title>` / `<desc>` bindings (SVG 2 §5.8) by
    // their *parent* container's scene-graph tree-path so `write_node`
    // can re-emit them as the first children of the matching `<g>` on
    // round-trip. The root-`<svg>` slot (empty path) is handled
    // separately below so descriptive children of the root render at
    // the top of the output document.
    let mut parent_to_titles: HashMap<Vec<usize>, &DescriptiveBinding> = HashMap::new();
    for entry in &extras.titles {
        parent_to_titles.insert(entry.parent_path.clone(), entry);
    }
    let mut parent_to_descs: HashMap<Vec<usize>, &DescriptiveBinding> = HashMap::new();
    for entry in &extras.descs {
        parent_to_descs.insert(entry.parent_path.clone(), entry);
    }
    let mut anim_by_parent: HashMap<String, Vec<&AnimationFragment>> = HashMap::new();
    let mut anim_orphan: Vec<&AnimationFragment> = Vec::new();
    // Set of ids that actually appear in id_paths — used to decide
    // whether an animation can be inlined or must be appended at the
    // trailing edge of the document (back-compat for callers that
    // didn't pre-populate id_paths or whose target id wasn't tracked).
    let mut known_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for entry in &extras.id_paths {
        known_ids.insert(entry.id.as_str());
    }
    for anim in &extras.animations {
        match &anim.parent_id {
            Some(pid) if known_ids.contains(pid.as_str()) => {
                anim_by_parent.entry(pid.clone()).or_default().push(anim);
            }
            _ => anim_orphan.push(anim),
        }
    }

    // Round 449 — bundle every per-path lookup table into the shared
    // [`EmitIndex`] the recursive emission reads from.
    let idx = EmitIndex {
        path_to_id,
        path_to_path_length,
        path_to_link,
        path_to_paint_order,
        path_to_vector_effect,
        path_to_shape_rendering,
        path_to_text_rendering,
        path_to_color_rendering,
        path_to_color_interpolation,
        path_to_overflow,
        path_to_pointer_events,
        path_to_cursor,
        path_to_dominant_baseline,
        path_to_use,
        path_to_switch,
        path_to_text,
        path_to_filter_ref,
        path_to_marker,
        parent_to_titles,
        parent_to_descs,
        anim_by_parent,
    };

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<svg xmlns=\"http://www.w3.org/2000/svg\"");
    out.push_str(&format!(" width=\"{}\"", trim_float(frame.width)));
    out.push_str(&format!(" height=\"{}\"", trim_float(frame.height)));
    if let Some(vb) = &frame.view_box {
        out.push_str(&format!(
            " viewBox=\"{} {} {} {}\"",
            trim_float(vb.min_x),
            trim_float(vb.min_y),
            trim_float(vb.width),
            trim_float(vb.height)
        ));
    }
    // Round-12: re-emit the root's `preserveAspectRatio` keyword pair
    // verbatim from the side-channel extras (the decoder bakes the
    // mapping into root.transform; this attribute is metadata so
    // downstream tools see what the source intended).
    if let Some(par) = &extras.root_preserve_aspect_ratio {
        out.push_str(&format!(" preserveAspectRatio=\"{}\"", escape_attr(par)));
    }
    out.push_str(">\n");

    // Collect every gradient referenced inside the tree (for round 1
    // output) plus every mask / clipPath the round-2 walker emitted —
    // each gets a `<defs>`-level entry with an auto-generated id so
    // the corresponding child can reference it by `mask=` /
    // `clip-path=` attribute.
    let mut gradients: GradientCollector = GradientCollector::default();
    let mut clips: ClipPathCollector = ClipPathCollector::default();
    let mut masks: MaskCollector = MaskCollector::default();
    walk_collect_defs(&frame.root, &mut gradients, &mut clips, &mut masks);

    // Round 372 — SVG 1.1 §14.3.1 / §14.4: map each synthesised
    // `clip{N}` / `mask{N}` id to the original `clip-path` / `mask`
    // reference id (when the decoder recorded a fingerprint binding for
    // it). Built from the collectors' fingerprint indices so the
    // mapping is exact. `clip_synth_to_orig[synth]` → original ref id;
    // used both to substitute the reference attribute in `write_node`
    // and to re-emit the verbatim source def in the `<defs>` block.
    let clip_orig_by_fp: HashMap<&str, &str> = extras
        .clip_refs
        .iter()
        .map(|b| (b.fingerprint.as_str(), b.ref_id.as_str()))
        .collect();
    let mask_orig_by_fp: HashMap<&str, &str> = extras
        .mask_refs
        .iter()
        .map(|b| (b.fingerprint.as_str(), b.ref_id.as_str()))
        .collect();
    // Only bind to a verbatim source def when that def was actually
    // captured (it carries an `id`); otherwise fall back to synthesis so
    // the reference still resolves.
    let clip_raw_ids: std::collections::HashSet<&str> = extras
        .clip_paths_raw
        .iter()
        .filter_map(|el| {
            el.attrs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("id"))
                .map(|(_, v)| v.as_str())
        })
        .collect();
    let mask_raw_ids: std::collections::HashSet<&str> = extras
        .masks_raw
        .iter()
        .filter_map(|el| {
            el.attrs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("id"))
                .map(|(_, v)| v.as_str())
        })
        .collect();
    let clip_entry_ids: Vec<(String, String)> = clips
        .entries
        .iter()
        .filter_map(|(synth_id, path)| {
            clip_orig_by_fp
                .get(path_fingerprint(path).as_str())
                .filter(|orig| clip_raw_ids.contains(**orig))
                .map(|orig| (synth_id.clone(), orig.to_string()))
        })
        .collect();
    for (synth, orig) in clip_entry_ids {
        clips.id_override.insert(synth, orig);
    }
    let mask_entry_ids: Vec<(String, String)> = masks
        .entries
        .iter()
        .filter_map(|(synth_id, kind, content)| {
            let fp = mask_fingerprint(*kind, content);
            mask_orig_by_fp
                .get(fp.as_str())
                .filter(|orig| mask_raw_ids.contains(**orig))
                .map(|orig| (synth_id.clone(), orig.to_string()))
        })
        .collect();
    for (synth, orig) in mask_entry_ids {
        masks.id_override.insert(synth, orig);
    }

    // Round 81 — collect the set of gradient ids carried by the
    // preserved-extras side-channel so we skip the scene-walk's
    // flattened emission for any id the author originally provided
    // verbatim. Without this guard, a `parse → write_svg_with_extras`
    // would emit each gradient twice (once verbatim from extras, once
    // flattened from the scene-walk).
    let extras_gradient_ids: std::collections::HashSet<&str> = extras
        .gradients
        .iter()
        .filter_map(|el| {
            el.attrs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("id"))
                .map(|(_, v)| v.as_str())
        })
        .collect();

    let has_defs = !gradients.entries.is_empty()
        || !clips.entries.is_empty()
        || !masks.entries.is_empty()
        || !extras.styles.is_empty()
        || !extras.filters.is_empty()
        || !extras.patterns.is_empty()
        || !extras.markers.is_empty()
        || !extras.gradients.is_empty()
        || !extras.defs_targets.is_empty();
    if has_defs {
        out.push_str("  <defs>\n");
        // Round 372 — re-emit verbatim `<defs>`-housed reference targets
        // (id-bearing shapes / `<g>` / `<symbol>`) first so a `<use>`
        // the scene-walk produces below resolves against a defined
        // target. SVG resolves `url(#id)` / `href="#id"` references
        // document-wide regardless of declaration order, but emitting
        // the targets at the head of `<defs>` keeps the output's
        // structure aligned with typical authoring.
        for target in &extras.defs_targets {
            write_raw_element(&mut out, target, 2);
        }
        for body in &extras.styles {
            // Wrap in CDATA so any `>` / `&` inside the CSS body
            // doesn't trip the parser. Rare for plain selectors, but
            // common in `content: "..."` declarations.
            out.push_str("    <style><![CDATA[");
            out.push_str(body.trim());
            out.push_str("]]></style>\n");
        }
        for filter in &extras.filters {
            write_raw_element(&mut out, filter, 2);
        }
        // Round 20 — `<pattern>` paint-server definitions, re-emitted
        // verbatim from the side-channel so a `parse → write`
        // round-trip preserves them.
        for pattern in &extras.patterns {
            write_raw_element(&mut out, pattern, 2);
        }
        // Round 104 — `<marker>` definitions, re-emitted verbatim from
        // the side-channel so a `parse → write` round-trip preserves
        // them (SVG 2 §13.7.1). Markers are never-rendered defs, so
        // they only appear here, not in the scene-walk output.
        for marker in &extras.markers {
            write_raw_element(&mut out, marker, 2);
        }
        // Round 81 — preserved-extras gradients re-emitted verbatim
        // *before* the flattened scene-walk gradients. Carrying the
        // author's original element means `gradientUnits` /
        // `gradientTransform` / `href` survive the round-trip even
        // though the flattened legacy [`Paint`] dropped them.
        for grad in &extras.gradients {
            write_raw_element(&mut out, grad, 2);
        }
        for (id, paint) in &gradients.entries {
            if extras_gradient_ids.contains(id.as_str()) {
                // Already emitted verbatim above — don't duplicate.
                continue;
            }
            write_gradient(&mut out, id, paint);
        }
        // Round 215 — SVG 1.1 §14.3.5 `clip-rule` lookup. Build a
        // fingerprint → keyword index from the side-channel so the
        // inner `<path>` of each emitted clipPath picks up the
        // author's original rule on round-trip. The binding key
        // matches `path_fingerprint`, which the encoder also uses for
        // its own clipPath dedup, so the lookup is direct.
        let clip_rule_by_fp: HashMap<&str, &str> = extras
            .clip_rules
            .iter()
            .map(|b| (b.path_fingerprint.as_str(), b.clip_rule.as_str()))
            .collect();
        // Round 372 — SVG 1.1 §14.3.1 / §14.4: index the verbatim
        // `<clipPath>` / `<mask>` defs by id, and the reference bindings
        // by fingerprint. When a synthesised clip / mask's fingerprint
        // is bound, re-emit the verbatim source def (preserving the
        // original id / `clipPathUnits` / multi-shape structure /
        // `maskUnits` / region) instead of the flattened synthesis, and
        // the matching `clip-path=` / `mask=` reference below substitutes
        // the original id (see `clip_synth_to_orig` / `mask_synth_to_orig`).
        let clip_raw_by_id: HashMap<&str, &Element> = extras
            .clip_paths_raw
            .iter()
            .filter_map(|el| {
                el.attrs
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("id"))
                    .map(|(_, v)| (v.as_str(), el))
            })
            .collect();
        let mask_raw_by_id: HashMap<&str, &Element> = extras
            .masks_raw
            .iter()
            .filter_map(|el| {
                el.attrs
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("id"))
                    .map(|(_, v)| (v.as_str(), el))
            })
            .collect();
        for (id, path) in &clips.entries {
            // Round 372 — bound to a verbatim source `<clipPath>` def?
            // Re-emit it verbatim (original id / units / shapes) and the
            // `clip-path=` reference attribute already substitutes the
            // same original id via `ClipPathCollector::lookup`.
            if let Some(orig) = clips.id_override.get(id) {
                if let Some(raw) = clip_raw_by_id.get(orig.as_str()) {
                    write_raw_element(&mut out, raw, 2);
                    continue;
                }
            }
            let rule = clip_rule_by_fp
                .get(path_fingerprint(path).as_str())
                .copied();
            write_clip_path(&mut out, id, path, rule);
        }
        for (id, kind, content) in &masks.entries {
            if let Some(orig) = masks.id_override.get(id) {
                if let Some(raw) = mask_raw_by_id.get(orig.as_str()) {
                    write_raw_element(&mut out, raw, 2);
                    continue;
                }
            }
            write_mask(&mut out, id, *kind, content, &gradients);
        }
        out.push_str("  </defs>\n");
    }

    // Round 122 — SVG 2 §5.8 root-level `<title>` / `<desc>`. The empty
    // parent_path (`[]`) keys the descriptive elements that were
    // direct children of the source root `<svg>`. They're emitted
    // before the scene-walk children so the output's first child of
    // `<svg>` is `<title>` (matching authoring guidance "Authors
    // should provide a `<title>` child element to the root svg
    // element"). `<title>` is emitted before `<desc>` per the §5.8
    // example structure.
    let root_path: Vec<usize> = Vec::new();
    if let Some(b) = idx.parent_to_titles.get(&root_path) {
        for item in &b.items {
            write_descriptive(&mut out, "title", item, 1);
        }
    }
    if let Some(b) = idx.parent_to_descs.get(&root_path) {
        for item in &b.items {
            write_descriptive(&mut out, "desc", item, 1);
        }
    }

    let mut path_stack: Vec<usize> = Vec::new();
    write_group_children(
        &mut out,
        &frame.root,
        1,
        &gradients,
        &clips,
        &masks,
        &idx,
        &mut path_stack,
    );

    // Round-4 extras that don't belong in <defs>: <foreignObject> and
    // animations get emitted at the trailing edge of the document so
    // the static scene stays visually identical.
    for fo in &extras.foreign_objects {
        write_raw_element(&mut out, fo, 1);
    }
    // Round 95 — re-emit captured `<view>` elements verbatim at the
    // trailing edge. Per SVG 2 §16.3.3 `<view>` itself contributes no
    // pixels to the rendered scene (it's a fragment-identifier
    // target), so positioning has no visual effect; emitting at the
    // trailing edge keeps the defs block tidy.
    for view in &extras.views {
        write_raw_element(&mut out, view, 1);
    }
    // Round 122 — re-emit captured `<metadata>` elements verbatim at
    // the trailing edge. Per SVG 2 §5.9 metadata content is opaque
    // foreign-namespace markup (RDF / Dublin Core / authoring-tool
    // extensions) and the UA stylesheet forces `display:none`, so
    // positioning has no visual effect.
    for md in &extras.metadata {
        write_raw_element(&mut out, md, 1);
    }
    // Round-12: re-emit captured <script> elements. The body is
    // wrapped in `<![CDATA[...]]>` so unescaped `<` characters
    // (common in real-world JS) don't poison the XML on the next
    // parse. The decoder strips CDATA on the way in, so this is a
    // canonicalising round-trip rather than a literal byte mirror.
    for script in &extras.scripts {
        write_script_element(&mut out, script, 1);
    }
    // Round-15: re-emit captured <image> elements. Each image carries
    // its href verbatim (data URI re-encoded from the decoded bytes
    // for inline; URL preserved for external) plus x/y/width/height
    // and an optional transform.
    for img in &extras.images {
        img.write_to(&mut out, "  ");
    }
    // Round 13: animations whose parent_id was tracked in
    // `extras.id_paths` are inlined inside the matching scene-graph
    // emit site by `write_node`. Anything that didn't match (no
    // parent_id, or the parent_id wasn't recorded — happens for
    // documents constructed without `parse_svg_with_extras`, or for
    // animations whose parent didn't survive the scene-graph build)
    // falls back to the round-4 trailing-edge emission with a parent
    // comment hint so it isn't lost.
    for anim in &anim_orphan {
        if let Some(id) = &anim.parent_id {
            out.push_str(&format!("  <!-- animation parent: #{} -->\n", id));
        }
        write_raw_element(&mut out, &anim.element, 1);
    }

    out.push_str("</svg>\n");
    out.into_bytes()
}

/// Round 115 — emit the SVG 2 §16.5 `<a>` hyperlink attributes onto an
/// already-opened `<a` tag (caller has written `<a`, this appends
/// ` href="…" target="…" …` and the caller closes with `>`). Each
/// attribute is emitted only when the source `<a>` carried it, so a
/// bare `<a>` round-trips as `<a>` and an `<a href>` round-trips with
/// just its href.
fn write_link_attrs(out: &mut String, link: &LinkBinding) {
    if let Some(href) = &link.href {
        out.push_str(&format!(" href=\"{}\"", escape_attr(href)));
    }
    if let Some(target) = &link.target {
        out.push_str(&format!(" target=\"{}\"", escape_attr(target)));
    }
    if let Some(download) = &link.download {
        out.push_str(&format!(" download=\"{}\"", escape_attr(download)));
    }
    if let Some(ping) = &link.ping {
        out.push_str(&format!(" ping=\"{}\"", escape_attr(ping)));
    }
    if let Some(rel) = &link.rel {
        out.push_str(&format!(" rel=\"{}\"", escape_attr(rel)));
    }
    if let Some(hreflang) = &link.hreflang {
        out.push_str(&format!(" hreflang=\"{}\"", escape_attr(hreflang)));
    }
    if let Some(type_) = &link.type_ {
        out.push_str(&format!(" type=\"{}\"", escape_attr(type_)));
    }
    if let Some(rp) = &link.referrerpolicy {
        out.push_str(&format!(" referrerpolicy=\"{}\"", escape_attr(rp)));
    }
    // Round 382 — re-emit the verbatim-captured attributes the typed link
    // fields don't model (`id`, `class`, `style`, `transform`,
    // presentation properties, conditional-processing, …) in their
    // original document order.
    for (k, v) in &link.extra_attrs {
        out.push_str(&format!(" {}=\"{}\"", k, escape_attr(v)));
    }
}

/// Round 372 — emit the SVG 2 §13.7.4 `marker-start` / `marker-mid` /
/// `marker-end` references recorded for a shape onto an already-opened
/// element tag (caller has written `<path` / `<g`, this appends the
/// attributes, and the caller closes the tag). Each is emitted only when
/// the source carried it (a `marker` shorthand was expanded into the
/// three slots at capture time, so the longhand form round-trips).
fn write_marker_attrs(out: &mut String, binding: &crate::preserved::MarkerRefBinding) {
    if let Some(v) = &binding.marker_start {
        out.push_str(&format!(" marker-start=\"{}\"", escape_attr(v)));
    }
    if let Some(v) = &binding.marker_mid {
        out.push_str(&format!(" marker-mid=\"{}\"", escape_attr(v)));
    }
    if let Some(v) = &binding.marker_end {
        out.push_str(&format!(" marker-end=\"{}\"", escape_attr(v)));
    }
}

/// Round 122 — emit a `<title>` or `<desc>` descriptive element per SVG
/// 2 §5.8. `tag` is `"title"` or `"desc"`; the optional `lang` attribute
/// is round-tripped if the source carried one (SVG-2 `lang`, not the
/// deprecated `xml:lang`, on round-trip per the §5.12.3 normative
/// guidance). An empty text body emits as self-closing (matches the
/// canonicalising behaviour the rest of the encoder uses for empty
/// containers).
fn write_descriptive(out: &mut String, tag: &str, item: &DescriptiveText, depth: usize) {
    let indent = "  ".repeat(depth);
    out.push_str(&indent);
    out.push('<');
    out.push_str(tag);
    if let Some(lang) = &item.lang {
        out.push_str(&format!(" lang=\"{}\"", escape_attr(lang)));
    }
    if item.text.is_empty() {
        out.push_str("/>\n");
        return;
    }
    out.push('>');
    out.push_str(&escape_text(&item.text));
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
}

/// Serialise an [`Element`] verbatim. Used to re-emit preserved-XML
/// fragments. `depth` is the indent level (in `"  "` units).
/// Round 382 — crate-internal verbatim element serialiser, exposed so
/// `image::SvgImage::write_to` can re-emit captured `<image>` children
/// (`<title>` / `<desc>` / animation elements) with the same escaping
/// and indentation rules the rest of the encoder uses.
pub(crate) fn write_element_verbatim(out: &mut String, el: &Element, depth: usize) {
    write_raw_element(out, el, depth);
}

/// Round 449 — serialise an [`Element`] whose subtree carries
/// *mixed content* (character data interleaved with child elements —
/// the `<text>` / `<tspan>` / `<textPath>` content model of SVG 2
/// §11.1). [`write_raw_element`] pretty-prints: it trims each text run
/// and inserts indentation/newline runs between children, which is
/// safe for element-only content but corrupts text layout — a trimmed
/// `"Hello "` before a `<tspan>` loses the inter-word break, adjacent
/// spans gain synthetic whitespace, and a run following a close tag
/// gains a leading space after `xml:space="default"` collapsing. This
/// writer emits the element's entire content inline: character data
/// escaped but otherwise byte-verbatim, child elements recursively
/// inline with no inserted whitespace.
fn write_mixed_content_element(out: &mut String, el: &Element, depth: usize) {
    out.push_str(&"  ".repeat(depth));
    write_inline_element(out, el);
    out.push('\n');
}

/// Inline (no indentation, no inserted whitespace) serialisation of an
/// element and its subtree. See [`write_mixed_content_element`].
fn write_inline_element(out: &mut String, el: &Element) {
    out.push('<');
    out.push_str(&el.name);
    for (k, v) in &el.attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        out.push_str(&escape_attr(v));
        out.push('"');
    }
    if el.children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for child in &el.children {
        match child {
            XmlNode::Element(c) => write_inline_element(out, c),
            XmlNode::Text(t) => out.push_str(&escape_text(t)),
        }
    }
    out.push_str("</");
    out.push_str(&el.name);
    out.push('>');
}

fn write_raw_element(out: &mut String, el: &Element, depth: usize) {
    let indent = "  ".repeat(depth);
    out.push_str(&indent);
    out.push('<');
    out.push_str(&el.name);
    for (k, v) in &el.attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        out.push_str(&escape_attr(v));
        out.push('"');
    }
    if el.children.is_empty() {
        out.push_str("/>\n");
        return;
    }
    out.push_str(">\n");
    for child in &el.children {
        match child {
            XmlNode::Element(c) => write_raw_element(out, c, depth + 1),
            XmlNode::Text(t) => {
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    out.push_str(&"  ".repeat(depth + 1));
                    out.push_str(&escape_text(trimmed));
                    out.push('\n');
                }
            }
        }
    }
    out.push_str(&indent);
    out.push_str("</");
    out.push_str(&el.name);
    out.push_str(">\n");
}

/// Round 12 — emit a `<script>` element with a CDATA-wrapped body so
/// unescaped `<` / `&` characters in the JavaScript don't trip the
/// XML parser on a subsequent round-trip. Empty bodies emit
/// self-closing.
fn write_script_element(out: &mut String, el: &Element, depth: usize) {
    let indent = "  ".repeat(depth);
    out.push_str(&indent);
    out.push('<');
    out.push_str(&el.name);
    for (k, v) in &el.attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        out.push_str(&escape_attr(v));
        out.push('"');
    }
    let mut body = String::new();
    for child in &el.children {
        if let XmlNode::Text(t) = child {
            body.push_str(t);
        }
    }
    if body.trim().is_empty() {
        out.push_str("/>\n");
        return;
    }
    out.push_str("><![CDATA[");
    // Defensively split a stray `]]>` inside the body across two
    // CDATA sections so the inner ]]> doesn't terminate ours early.
    out.push_str(&body.replace("]]>", "]]]]><![CDATA[>"));
    out.push_str("]]></");
    out.push_str(&el.name);
    out.push_str(">\n");
}

fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            other => out.push(other),
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn write_group_children(
    out: &mut String,
    group: &Group,
    depth: usize,
    gradients: &GradientCollector,
    clips: &ClipPathCollector,
    masks: &MaskCollector,
    idx: &EmitIndex,
    path_stack: &mut Vec<usize>,
) {
    for (i, child) in group.children.iter().enumerate() {
        path_stack.push(i);
        write_node(out, child, depth, gradients, clips, masks, idx, path_stack);
        path_stack.pop();
    }
}

#[allow(clippy::too_many_arguments)]
fn write_node(
    out: &mut String,
    node: &Node,
    depth: usize,
    gradients: &GradientCollector,
    clips: &ClipPathCollector,
    masks: &MaskCollector,
    idx: &EmitIndex,
    path_stack: &mut Vec<usize>,
) {
    // Destructure the shared lookup index so the per-round emit logic
    // below keeps addressing each table by its historical name.
    let EmitIndex {
        path_to_id,
        path_to_path_length,
        path_to_link,
        path_to_paint_order,
        path_to_vector_effect,
        path_to_shape_rendering,
        path_to_text_rendering,
        path_to_color_rendering,
        path_to_color_interpolation,
        path_to_overflow,
        path_to_pointer_events,
        path_to_cursor,
        path_to_dominant_baseline,
        path_to_use,
        path_to_switch,
        path_to_text,
        path_to_filter_ref,
        path_to_marker,
        parent_to_titles,
        parent_to_descs,
        anim_by_parent,
    } = idx;
    let indent = "  ".repeat(depth);
    // Round 372 — does this scene-graph position carry recorded
    // `marker-*` references (SVG 2 §13.7.4)? If so the `Node::Group` /
    // `Node::Path` arm re-emits `marker-start` / `marker-mid` /
    // `marker-end` on the matching shape.
    let marker_here: Option<&crate::preserved::MarkerRefBinding> =
        path_to_marker.get(path_stack.as_slice()).copied();
    // Round 372 — does this scene-graph position carry a recorded
    // `filter="url(#id)"` reference (SVG 1.1 §15)? If so the
    // `Node::Group` arm re-emits `filter=` on the filter-wrapper `<g>`.
    let filter_ref_here: Option<&str> = path_to_filter_ref.get(path_stack.as_slice()).copied();
    // Round 372 — does this scene-graph position carry a recorded
    // `<switch>` (SVG 2 §5.7)? If so the `Node::Group` arm emits the
    // verbatim `<switch>` (all alternatives) and skips the selected
    // child, collapsing the decode-time selection back to the
    // first-match container. Checked alongside `use_here`.
    let switch_here: Option<&crate::preserved::SwitchBinding> =
        path_to_switch.get(path_stack.as_slice()).copied();
    // Round 372 — does this scene-graph position carry a recorded
    // `<use>` reference (SVG 2 §5.6)? If so the `Node::Group` arm emits
    // `<use href="#id" …/>` and skips re-walking the instantiated
    // children, collapsing the flattened geometry back to the source
    // reference. Checked before the per-attribute lookups so the use
    // short-circuit can return early.
    let use_here: Option<&crate::preserved::UseBinding> =
        path_to_use.get(path_stack.as_slice()).copied();
    // Round 13 — does this scene-graph position carry a recorded
    // source id? If so we emit `id="..."` and inline its
    // `<animate>` / `<set>` / `<animateTransform>` fragments.
    let id_here: Option<&str> = path_to_id.get(path_stack.as_slice()).map(String::as_str);
    // Round 21 — does this scene-graph position carry a recorded
    // author `pathLength`? If so the corresponding `<path>` emission
    // below carries `pathLength="..."` so a re-parse of the output
    // recovers the same calibration.
    let path_length_here: Option<f32> = path_to_path_length.get(path_stack.as_slice()).copied();
    // Round 115 — does this scene-graph position carry a recorded
    // `<a>` hyperlink (SVG 2 §16.5)? If so the `Node::Group` arm wraps
    // the emitted `<g>` in `<a href="…">…</a>`.
    let link_here: Option<&LinkBinding> = path_to_link.get(path_stack.as_slice()).copied();
    // Round 205 — does this scene-graph position carry a recorded
    // `paint-order` attribute (SVG 2 §13.8)? If so the corresponding
    // shape emission carries `paint-order="..."` on round-trip.
    let paint_order_here: Option<&str> = path_to_paint_order.get(path_stack.as_slice()).copied();
    // Round 209 — does this scene-graph position carry a recorded
    // `vector-effect` attribute (SVG 2 §8.13)? If so the corresponding
    // shape / group emission carries `vector-effect="..."` on
    // round-trip. Mirrors the round-205 `paint-order` lookup above.
    let vector_effect_here: Option<&str> =
        path_to_vector_effect.get(path_stack.as_slice()).copied();
    // Round 221 — does this scene-graph position carry a recorded
    // `shape-rendering` attribute (SVG 2 §13.10.2)? If so the
    // corresponding shape / group emission carries
    // `shape-rendering="..."` on round-trip. Mirrors the round-205
    // `paint-order` / round-209 `vector-effect` lookups above.
    let shape_rendering_here: Option<&str> =
        path_to_shape_rendering.get(path_stack.as_slice()).copied();
    // Round 228 — does this scene-graph position carry a recorded
    // `text-rendering` attribute (SVG 2 §13.10.3)? If so the
    // corresponding `<text>` / `<g>` emission carries
    // `text-rendering="..."` on round-trip. Mirrors the round-221
    // `shape-rendering` lookup above.
    let text_rendering_here: Option<&str> =
        path_to_text_rendering.get(path_stack.as_slice()).copied();
    // Round 247 — does this scene-graph position carry a recorded
    // `color-rendering` attribute (SVG 2 §13.10.1)? If so the
    // corresponding shape / `<g>` emission carries
    // `color-rendering="..."` on round-trip. Mirrors the round-221
    // `shape-rendering` / round-228 `text-rendering` lookups above.
    let color_rendering_here: Option<&str> =
        path_to_color_rendering.get(path_stack.as_slice()).copied();
    // Round 252 — does this scene-graph position carry a recorded
    // `color-interpolation` attribute (SVG 2 §13.9)? If so the
    // corresponding shape / `<g>` emission carries
    // `color-interpolation="..."` on round-trip. Mirrors the round-247
    // `color-rendering` lookup above.
    let color_interpolation_here: Option<&str> = path_to_color_interpolation
        .get(path_stack.as_slice())
        .copied();
    // Round 257 — does this scene-graph position carry a recorded
    // `overflow` attribute (SVG 2 §3.11)? If so the corresponding
    // shape / `<g>` emission carries `overflow="..."` on round-trip.
    // Mirrors the round-252 `color-interpolation` lookup above.
    let overflow_here: Option<&str> = path_to_overflow.get(path_stack.as_slice()).copied();
    // Round 260 — does this scene-graph position carry a recorded
    // `pointer-events` attribute (SVG 2 §15.6)? If so the corresponding
    // shape / `<g>` emission carries `pointer-events="..."` on
    // round-trip. Mirrors the round-257 `overflow` lookup above.
    let pointer_events_here: Option<&str> =
        path_to_pointer_events.get(path_stack.as_slice()).copied();
    // Round 261 — does this scene-graph position carry a recorded
    // `cursor` attribute (SVG 1.1 §16.8.2)? If so the corresponding
    // shape / `<g>` emission carries `cursor="..."` on round-trip.
    // Mirrors the round-260 `pointer-events` lookup above.
    let cursor_here: Option<&str> = path_to_cursor.get(path_stack.as_slice()).copied();
    // Round 291 — does this scene-graph position carry a recorded
    // `dominant-baseline` attribute (SVG 1.1 §10.9.2)? If so the
    // corresponding shape / `<g>` emission carries
    // `dominant-baseline="..."` on round-trip. Mirrors the round-261
    // `cursor` lookup above.
    let dominant_baseline_here: Option<&str> = path_to_dominant_baseline
        .get(path_stack.as_slice())
        .copied();
    let inline_anims: &[&AnimationFragment] = id_here
        .and_then(|id| anim_by_parent.get(id))
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    // Round 449 — SVG 2 §11.2: this scene-graph position is a
    // flattened `<text>` element. Re-emit the verbatim source markup
    // (string content, font selection properties, `<tspan>`
    // per-character positioning arrays, `<textPath>`, descriptive and
    // animation children, and every styling / conditional attribute)
    // and skip the shaped glyph-outline children entirely. Checked
    // before the node-kind dispatch so the replacement applies whether
    // the decoder produced a bare group, a filter-wrapper group, a
    // `Node::SoftMask`, or a single path for the text — the verbatim
    // element carries its own `clip-path=` / `mask=` / `filter=`
    // attributes, so a re-parse rebuilds the identical wrappers.
    if let Some(t) = path_to_text.get(path_stack.as_slice()) {
        write_mixed_content_element(out, &t.element, depth);
        return;
    }
    match node {
        Node::Group(g) => {
            // Round 372 — SVG 2 §5.7: if this group is the selected
            // branch of a `<switch>`, re-emit the verbatim `<switch>`
            // (every conditional alternative) and skip the selected
            // child. Re-parsing the output re-runs the conditional
            // selection, so the round-trip preserves the first-match
            // container rather than freezing the decode-time choice.
            // Checked before the `<use>` collapse: a `<switch>` whose
            // selected branch is itself a `<use>` instance still
            // round-trips as `<switch>` (the inner `<use>` rides inside
            // the verbatim element).
            if let Some(s) = switch_here {
                write_raw_element(out, &s.element, depth);
                return;
            }
            // Round 372 — SVG 2 §5.6: if this group is the instantiated
            // body of a `<use>` reference, collapse it back to a single
            // `<use href="#id" …/>` element and skip re-walking the
            // flattened children entirely. The decoder baked the
            // reference target's geometry into `g.children`; re-emitting
            // them would (a) lose the reference identity and (b) inline
            // the target N times for an N-instance document. Emitting
            // `<use>` instead keeps the output structurally faithful and
            // compact. Any `<animate>` whose parent id is the `<use>`'s
            // own id is inlined as a child so SMIL on the instance
            // survives.
            if let Some(u) = use_here {
                out.push_str(&indent);
                out.push_str("<use");
                // Emit ONLY the `<use>`'s own `id` (from the binding) —
                // NOT the generic `id_here` from the round-13 id_paths
                // table. When the `<use>` instantiates an id-bearing
                // target, the id_paths recorder fires on this same
                // scene-graph slot carrying the *target's* id (e.g.
                // `#r1`); emitting that here would make the round-tripped
                // `<use href="#r1" id="r1">` self-reference, which the
                // re-parse drops as a cycle. The use's structural
                // identity is the binding's `id` field alone.
                if let Some(id) = &u.id {
                    out.push_str(&format!(" id=\"{}\"", escape_attr(id)));
                }
                out.push_str(&format!(" href=\"{}\"", escape_attr(&u.href)));
                if let Some(x) = &u.x {
                    out.push_str(&format!(" x=\"{}\"", escape_attr(x)));
                }
                if let Some(y) = &u.y {
                    out.push_str(&format!(" y=\"{}\"", escape_attr(y)));
                }
                if let Some(w) = &u.width {
                    out.push_str(&format!(" width=\"{}\"", escape_attr(w)));
                }
                if let Some(h) = &u.height {
                    out.push_str(&format!(" height=\"{}\"", escape_attr(h)));
                }
                if let Some(t) = &u.transform {
                    out.push_str(&format!(" transform=\"{}\"", escape_attr(t)));
                }
                // Round 382 — re-emit the verbatim-captured attributes
                // the typed slots don't model (`class`, `style`,
                // presentation properties, conditional-processing, …) in
                // their original document order.
                for (k, v) in &u.extra_attrs {
                    out.push_str(&format!(" {}=\"{}\"", k, escape_attr(v)));
                }
                if inline_anims.is_empty() {
                    out.push_str("/>\n");
                } else {
                    out.push_str(">\n");
                    for anim in inline_anims {
                        write_raw_element(out, &anim.element, depth + 1);
                    }
                    out.push_str(&indent);
                    out.push_str("</use>\n");
                }
                return;
            }
            // Round 115 — open the wrapping `<a>` element if this group
            // came from an `<a>` in the source. The hyperlink attributes
            // ride on the `<a>`; the group's own transform / opacity /
            // clip stay on the inner `<g>` so the visual nesting is
            // identical to the source.
            let a_indent = if let Some(link) = link_here {
                out.push_str(&indent);
                out.push_str("<a");
                write_link_attrs(out, link);
                out.push_str(">\n");
                "  ".repeat(depth + 1)
            } else {
                indent.clone()
            };
            let g_depth = if link_here.is_some() {
                depth + 1
            } else {
                depth
            };
            out.push_str(&a_indent);
            out.push_str("<g");
            if let Some(id) = id_here {
                out.push_str(&format!(" id=\"{}\"", escape_attr(id)));
            }
            if !g.transform.is_identity() {
                out.push_str(&format!(
                    " transform=\"{}\"",
                    format_transform(&g.transform)
                ));
            }
            if (g.opacity - 1.0).abs() > f32::EPSILON {
                out.push_str(&format!(" opacity=\"{}\"", trim_float(g.opacity)));
            }
            if let Some(clip) = &g.clip {
                if let Some(id) = clips.lookup(clip) {
                    out.push_str(&format!(" clip-path=\"url(#{})\"", escape_attr(id)));
                }
            }
            // Round 372 — SVG 1.1 §15 `filter="url(#id)"`. The decoder
            // wraps a filtered element in a pass-through `<g>`; this
            // re-emits the source `filter=` reference on that wrapper so
            // the preserved `<filter>` def (in <defs>) stays connected to
            // its graphics element after round-trip. Emitted verbatim
            // (preserves a chained `filter="url(#a) url(#b)"` list).
            if let Some(fr) = filter_ref_here {
                out.push_str(&format!(" filter=\"{}\"", escape_attr(fr)));
            }
            // Round 372 — SVG 2 §13.7.4 `marker-*` references on a `<g>`
            // carrier (when the shape's outer-most emit site is a clip /
            // mask / filter wrapper group, the source marker attributes
            // ride the wrapping group on round-trip).
            if let Some(m) = marker_here {
                write_marker_attrs(out, m);
            }
            // Round 205 — SVG 2 §13.8 `paint-order` attribute. When
            // the shape's outer-most emit site is a `<g>` (clip /
            // mask / filter wrapper, or the round-205 split into two
            // PathNodes), the source `paint-order=` lives on the
            // wrapping group; the `<path>` arm below picks it up when
            // the shape lands as a bare `<path>`.
            if let Some(po) = paint_order_here {
                out.push_str(&format!(" paint-order=\"{}\"", escape_attr(po)));
            }
            // Round 209 — SVG 2 §8.13 `vector-effect`. Mirrors the
            // round-205 `paint-order` emission above; a `<g
            // vector-effect=…>` ancestor round-trips on the same `<g>`
            // emit site even though the property does not cascade.
            if let Some(ve) = vector_effect_here {
                out.push_str(&format!(" vector-effect=\"{}\"", escape_attr(ve)));
            }
            // Round 221 — SVG 2 §13.10.2 `shape-rendering`. Same emit
            // slot as the round-205 / round-209 attributes above.
            if let Some(sr) = shape_rendering_here {
                out.push_str(&format!(" shape-rendering=\"{}\"", escape_attr(sr)));
            }
            // Round 228 — SVG 2 §13.10.3 `text-rendering`. Same emit
            // slot as the round-221 `shape-rendering` attribute above;
            // both attributes can coexist on a single `<g>` carrier when
            // the source author wrote both.
            if let Some(tr) = text_rendering_here {
                out.push_str(&format!(" text-rendering=\"{}\"", escape_attr(tr)));
            }
            // Round 247 — SVG 2 §13.10.1 `color-rendering`. Same emit
            // slot as the round-221 / round-228 attributes above; all
            // three rendering-hint attributes can coexist on a single
            // `<g>` carrier when the source author wrote them together.
            if let Some(cr) = color_rendering_here {
                out.push_str(&format!(" color-rendering=\"{}\"", escape_attr(cr)));
            }
            // Round 252 — SVG 2 §13.9 `color-interpolation`. Same emit
            // slot as the §13.10.x rendering hints above; §13.9 is the
            // working-colour-space selector (orthogonal to the §13.10.1
            // quality hint), so both attributes can coexist on the same
            // `<g>` carrier when the source author wrote them together.
            if let Some(ci) = color_interpolation_here {
                out.push_str(&format!(" color-interpolation=\"{}\"", escape_attr(ci)));
            }
            // Round 257 — SVG 2 §3.11 `overflow`. Same emit slot as
            // the §13.x attributes above; the property is NOT
            // inherited per CSS 2.1 §11.1.1, so the round-trip
            // carrier is the only mechanism that preserves a
            // hand-authored `<g overflow="hidden">` on the same
            // group element (the cascade itself would have already
            // reset descendants to `visible`).
            if let Some(o) = overflow_here {
                out.push_str(&format!(" overflow=\"{}\"", escape_attr(o)));
            }
            // Round 260 — SVG 2 §15.6 `pointer-events`. Same emit
            // slot as the §3.11 `overflow` attribute above; §15.6
            // selects the hit-test gate for pointer events. The
            // property IS inherited, so a `<g pointer-events="none">`
            // ancestor cascades the value to descendants — the
            // round-trip carrier emits on the topmost emit site (the
            // group) without redundantly recording on every cascaded
            // child.
            if let Some(pe) = pointer_events_here {
                out.push_str(&format!(" pointer-events=\"{}\"", escape_attr(pe)));
            }
            // Round 261 — SVG 1.1 §16.8.2 `cursor`. Same emit slot as
            // the §15.6 `pointer-events` attribute above; §16.8.2
            // selects the cursor displayed while the pointing device
            // hovers the element. The property IS inherited, so a
            // `<g cursor="wait">` ancestor cascades the value to
            // descendants — the round-trip carrier emits on the
            // topmost emit site (the group) without redundantly
            // recording on every cascaded child.
            if let Some(c) = cursor_here {
                out.push_str(&format!(" cursor=\"{}\"", escape_attr(c)));
            }
            // Round 291 — SVG 1.1 §10.9.2 `dominant-baseline`. Same emit
            // slot as the §16.8.2 `cursor` attribute above; §10.9.2
            // selects the scaled-baseline-table for a text content
            // element. The property is NOT inherited, so a
            // `<text dominant-baseline="hanging">` carrier round-trips
            // on its own emit slot (the cascade itself would have reset
            // descendant runs to `auto`); the lexical side-channel
            // preserves the source attribute regardless.
            if let Some(db) = dominant_baseline_here {
                out.push_str(&format!(" dominant-baseline=\"{}\"", escape_attr(db)));
            }
            out.push_str(">\n");
            // Round 122 — SVG 2 §5.8: emit captured `<title>` /
            // `<desc>` children of this group as the *first* children
            // of `<g>`, matching the spec's "the rendered element is
            // of semantic importance" placement and the SVG 1.1
            // legacy rule that the user agent "may not recognize a
            // title element that is not the first child of its
            // parent". Title precedes desc per the §5.8 example.
            if let Some(b) = parent_to_titles.get(path_stack.as_slice()) {
                for item in &b.items {
                    write_descriptive(out, "title", item, g_depth + 1);
                }
            }
            if let Some(b) = parent_to_descs.get(path_stack.as_slice()) {
                for item in &b.items {
                    write_descriptive(out, "desc", item, g_depth + 1);
                }
            }
            write_group_children(
                out,
                g,
                g_depth + 1,
                gradients,
                clips,
                masks,
                idx,
                path_stack,
            );
            // Round 13 — animation children come AFTER the group's
            // own children. SMIL doesn't dictate an ordering between
            // sibling shapes and animation elements; emitting last
            // matches the order browsers typically produce on
            // serialisation and keeps the static visual identical.
            for anim in inline_anims {
                write_raw_element(out, &anim.element, g_depth + 1);
            }
            out.push_str(&a_indent);
            out.push_str("</g>\n");
            // Round 115 — close the wrapping `<a>`.
            if link_here.is_some() {
                out.push_str(&indent);
                out.push_str("</a>\n");
            }
        }
        Node::Path(p) => {
            // If we have inline animations for this path, emit it as
            // `<path ...>...</path>` (with an explicit close so
            // children fit). Otherwise self-close.
            out.push_str(&indent);
            out.push_str("<path");
            if let Some(id) = id_here {
                out.push_str(&format!(" id=\"{}\"", escape_attr(id)));
            }
            out.push_str(" d=\"");
            write_path_d(out, &p.path.commands);
            out.push('"');
            write_paint_attrs(out, p, gradients);
            // Round 21 — re-emit the author's `pathLength` (SVG 2 §9.6.1).
            if let Some(pl) = path_length_here {
                out.push_str(&format!(" pathLength=\"{}\"", trim_float(pl)));
            }
            // Round 372 — SVG 2 §13.7.4 `marker-*` references on a bare
            // `<path>` (the common case: a hand-authored `<path
            // marker-end="url(#arrow)">` lands as a single PathNode). The
            // `<marker>` def rides `extras.markers` verbatim; this
            // reconnects the shape to it on round-trip.
            if let Some(m) = marker_here {
                write_marker_attrs(out, m);
            }
            // Round 205 — re-emit the author's `paint-order` keyword
            // string (SVG 2 §13.8) when the shape's outer-most emit
            // site is a bare `<path>`. The cascade has already
            // arranged the scene-graph paint order (single PathNode
            // for the `normal` / fill-first orders; a wrapping group
            // with two single-purpose PathNodes for the stroke-first
            // case) so this attribute is purely a round-trip carrier.
            if let Some(po) = paint_order_here {
                out.push_str(&format!(" paint-order=\"{}\"", escape_attr(po)));
            }
            // Round 209 — SVG 2 §8.13 `vector-effect`. Same emit slot
            // as the round-205 `paint-order` attribute above; both are
            // purely round-trip carriers, the §8.13 transform
            // suppression itself happens in the renderer.
            if let Some(ve) = vector_effect_here {
                out.push_str(&format!(" vector-effect=\"{}\"", escape_attr(ve)));
            }
            // Round 221 — SVG 2 §13.10.2 `shape-rendering`. Same emit
            // slot as the round-205 / round-209 attributes above.
            if let Some(sr) = shape_rendering_here {
                out.push_str(&format!(" shape-rendering=\"{}\"", escape_attr(sr)));
            }
            // Round 228 — SVG 2 §13.10.3 `text-rendering`. Same emit
            // slot as `shape-rendering` — a bare `<path>` produced by
            // a hand-authored `<text text-rendering=…>` wrapped in a
            // shape branch is unusual, but the carrier rides through
            // here for completeness so a misplaced source attribute
            // still round-trips.
            if let Some(tr) = text_rendering_here {
                out.push_str(&format!(" text-rendering=\"{}\"", escape_attr(tr)));
            }
            // Round 247 — SVG 2 §13.10.1 `color-rendering`. Same emit
            // slot as the round-221 / round-228 hints above; the cascade
            // already resolved the property onto the carried PaintState,
            // so this attribute is purely a round-trip carrier.
            if let Some(cr) = color_rendering_here {
                out.push_str(&format!(" color-rendering=\"{}\"", escape_attr(cr)));
            }
            // Round 252 — SVG 2 §13.9 `color-interpolation`. Same emit
            // slot as the rendering-hint attributes above; §13.9 is the
            // working-colour-space selector, the cascade already
            // resolved the property onto the carried PaintState, so
            // this attribute is purely a round-trip carrier.
            if let Some(ci) = color_interpolation_here {
                out.push_str(&format!(" color-interpolation=\"{}\"", escape_attr(ci)));
            }
            // Round 257 — SVG 2 §3.11 `overflow`. Same emit slot as
            // the §13.x attributes above; the carrier survives a
            // round-trip on a bare `<path>` even though the §3.11
            // applies-to list does not formally include `<path>` —
            // we keep the carrier slot symmetric with the §13.x
            // hints so a misplaced source attribute still round-trips.
            if let Some(o) = overflow_here {
                out.push_str(&format!(" overflow=\"{}\"", escape_attr(o)));
            }
            // Round 260 — SVG 2 §15.6 `pointer-events`. Same emit
            // slot as the §3.11 `overflow` attribute above; the
            // carrier rides on the bare `<path>` when a shape carries
            // the attribute directly (rather than via a wrapping
            // group). The cascade already resolved the property onto
            // the carried PaintState, so this attribute is purely a
            // round-trip carrier.
            if let Some(pe) = pointer_events_here {
                out.push_str(&format!(" pointer-events=\"{}\"", escape_attr(pe)));
            }
            // Round 261 — SVG 1.1 §16.8.2 `cursor`. Same emit slot as
            // the §15.6 `pointer-events` attribute above; the carrier
            // rides on the bare `<path>` when a shape carries the
            // attribute directly (rather than via a wrapping group).
            // The cascade already resolved the property onto the
            // carried PaintState, so this attribute is purely a
            // round-trip carrier.
            if let Some(c) = cursor_here {
                out.push_str(&format!(" cursor=\"{}\"", escape_attr(c)));
            }
            // Round 291 — SVG 1.1 §10.9.2 `dominant-baseline`. Same
            // emit slot as the §16.8.2 `cursor` attribute above; the
            // carrier rides on the bare `<path>` when a shape (or a
            // hand-authored `<text dominant-baseline=…>` that fell into
            // the shape branch) carries the attribute directly. The
            // cascade already resolved the property onto the carried
            // PaintState, so this attribute is purely a round-trip
            // carrier.
            if let Some(db) = dominant_baseline_here {
                out.push_str(&format!(" dominant-baseline=\"{}\"", escape_attr(db)));
            }
            if inline_anims.is_empty() {
                out.push_str("/>\n");
            } else {
                out.push_str(">\n");
                for anim in inline_anims {
                    write_raw_element(out, &anim.element, depth + 1);
                }
                out.push_str(&indent);
                out.push_str("</path>\n");
            }
        }
        Node::SoftMask {
            mask,
            mask_kind,
            content,
        } => {
            // Wrap content in a `<g mask="url(#id)">` and emit; the
            // mask itself was already collected into the defs block.
            let mask_node = (**mask).clone();
            let id = masks
                .lookup(*mask_kind, &mask_node)
                .map(String::from)
                .unwrap_or_default();
            out.push_str(&indent);
            // Round 13: if a source id was attached here (e.g. the
            // source `<rect id="r1" mask="url(#m)">` becomes
            // `Node::SoftMask { content: Path(r1) }`), surface it on
            // the wrapping group so downstream tooling can address
            // the masked rect by its source name.
            let id_attr = id_here
                .map(|i| format!(" id=\"{}\"", escape_attr(i)))
                .unwrap_or_default();
            if id.is_empty() {
                out.push_str(&format!("<g{}>\n", id_attr));
            } else {
                out.push_str(&format!(
                    "<g{} mask=\"url(#{})\">\n",
                    id_attr,
                    escape_attr(&id)
                ));
            }
            // The masked content's scene-graph position is unchanged
            // (SoftMask is a wrapper that doesn't add to the path
            // index space). Don't push an extra index — the inner
            // node sits at the same path as the SoftMask itself.
            write_node(
                out,
                content,
                depth + 1,
                gradients,
                clips,
                masks,
                idx,
                path_stack,
            );
            for anim in inline_anims {
                write_raw_element(out, &anim.element, depth + 1);
            }
            out.push_str(&indent);
            out.push_str("</g>\n");
        }
        Node::Image(_) => {
            // Round 1: serialising embedded raster images would
            // require base64 + a `<image>` href — defer.
        }
        // `Node` is `#[non_exhaustive]` upstream; future variants are
        // silently dropped.
        _ => {}
    }
}

fn write_paint_attrs(out: &mut String, node: &PathNode, gradients: &GradientCollector) {
    match &node.fill {
        Some(p) => out.push_str(&format!(" fill=\"{}\"", paint_to_attr(p, gradients))),
        None => out.push_str(" fill=\"none\""),
    }
    if let Some(stroke) = &node.stroke {
        out.push_str(&format!(
            " stroke=\"{}\"",
            paint_to_attr(&stroke.paint, gradients)
        ));
        if (stroke.width - 1.0).abs() > f32::EPSILON {
            out.push_str(&format!(" stroke-width=\"{}\"", trim_float(stroke.width)));
        }
        if stroke.cap != LineCap::Butt {
            out.push_str(&format!(" stroke-linecap=\"{}\"", linecap_str(stroke.cap)));
        }
        if stroke.join != LineJoin::Miter {
            out.push_str(&format!(
                " stroke-linejoin=\"{}\"",
                linejoin_str(stroke.join)
            ));
        }
        if (stroke.miter_limit - 4.0).abs() > f32::EPSILON {
            out.push_str(&format!(
                " stroke-miterlimit=\"{}\"",
                trim_float(stroke.miter_limit)
            ));
        }
        if let Some(dash) = &stroke.dash {
            write_dash(out, dash);
        }
    }
    if node.fill_rule == FillRule::EvenOdd {
        out.push_str(" fill-rule=\"evenodd\"");
    }
}

fn write_dash(out: &mut String, dash: &DashPattern) {
    if !dash.array.is_empty() {
        let arr: Vec<String> = dash.array.iter().map(|n| trim_float(*n)).collect();
        out.push_str(&format!(" stroke-dasharray=\"{}\"", arr.join(",")));
    }
    if dash.offset.abs() > f32::EPSILON {
        out.push_str(&format!(
            " stroke-dashoffset=\"{}\"",
            trim_float(dash.offset)
        ));
    }
}

fn paint_to_attr(p: &Paint, gradients: &GradientCollector) -> String {
    match p {
        Paint::Solid(c) => color_to_attr(*c),
        Paint::LinearGradient(_) | Paint::RadialGradient(_) => {
            // We registered every gradient already in the collection
            // pass; look it up by pointer-or-content to find its id.
            match gradients.lookup(p) {
                Some(id) => format!("url(#{})", escape_attr(id)),
                None => "none".to_string(),
            }
        }
        // `Paint` is `#[non_exhaustive]` upstream; unknown future
        // paint servers serialise as `none` rather than failing.
        _ => "none".to_string(),
    }
}

fn color_to_attr(c: Rgba) -> String {
    if c.a == 255 {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        // SVG accepts `#rrggbbaa` in CSS Color L4 / SVG 2; for
        // maximum interop we emit `rgba(r,g,b,a)`.
        format!(
            "rgba({},{},{},{})",
            c.r,
            c.g,
            c.b,
            trim_float(c.a as f32 / 255.0)
        )
    }
}

fn linecap_str(c: LineCap) -> &'static str {
    match c {
        LineCap::Butt => "butt",
        LineCap::Round => "round",
        LineCap::Square => "square",
    }
}

fn linejoin_str(j: LineJoin) -> &'static str {
    match j {
        LineJoin::Miter => "miter",
        LineJoin::Round => "round",
        LineJoin::Bevel => "bevel",
    }
}

fn format_transform(t: &Transform2D) -> String {
    format!(
        "matrix({} {} {} {} {} {})",
        trim_float(t.a),
        trim_float(t.b),
        trim_float(t.c),
        trim_float(t.d),
        trim_float(t.e),
        trim_float(t.f)
    )
}

fn write_path_d(out: &mut String, cmds: &[PathCommand]) {
    let mut first = true;
    for cmd in cmds {
        if !first {
            out.push(' ');
        }
        first = false;
        match cmd {
            PathCommand::MoveTo(p) => write_pt(out, "M", *p),
            PathCommand::LineTo(p) => write_pt(out, "L", *p),
            PathCommand::QuadCurveTo { control, end } => {
                out.push_str(&format!(
                    "Q {} {} {} {}",
                    trim_float(control.x),
                    trim_float(control.y),
                    trim_float(end.x),
                    trim_float(end.y)
                ));
            }
            PathCommand::CubicCurveTo { c1, c2, end } => {
                out.push_str(&format!(
                    "C {} {} {} {} {} {}",
                    trim_float(c1.x),
                    trim_float(c1.y),
                    trim_float(c2.x),
                    trim_float(c2.y),
                    trim_float(end.x),
                    trim_float(end.y)
                ));
            }
            PathCommand::ArcTo {
                rx,
                ry,
                x_axis_rot,
                large_arc,
                sweep,
                end,
            } => {
                out.push_str(&format!(
                    "A {} {} {} {} {} {} {}",
                    trim_float(*rx),
                    trim_float(*ry),
                    trim_float(x_axis_rot.to_degrees()),
                    if *large_arc { 1 } else { 0 },
                    if *sweep { 1 } else { 0 },
                    trim_float(end.x),
                    trim_float(end.y)
                ));
            }
            PathCommand::Close => out.push('Z'),
            // `PathCommand` is `#[non_exhaustive]` upstream; future
            // shorthand variants are dropped from the serialisation.
            _ => {}
        }
    }
}

fn write_pt(out: &mut String, cmd: &str, p: Point) {
    out.push_str(cmd);
    out.push(' ');
    out.push_str(&trim_float(p.x));
    out.push(' ');
    out.push_str(&trim_float(p.y));
}

fn trim_float(v: f32) -> String {
    // Normalise both +0.0 and -0.0 to "0" — the sign is invisible in
    // SVG output and a bare "0" is one byte shorter.
    if v == 0.0 {
        return "0".to_string();
    }
    // Print with up to 6 significant decimals, trim trailing zeros.
    let s = format!("{v:.6}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".into()
    } else {
        trimmed.to_string()
    }
}

#[derive(Default)]
struct GradientCollector {
    entries: Vec<(String, Paint)>,
    /// Map an opaque "address" (hash of fingerprint) to the id we
    /// assigned. Round 1 dedupes by structural equality so a
    /// gradient referenced twice serialises once.
    by_fingerprint: HashMap<String, String>,
}

impl GradientCollector {
    fn ensure(&mut self, paint: &Paint) {
        let fp = match paint {
            Paint::LinearGradient(g) => linear_fingerprint(g),
            Paint::RadialGradient(g) => radial_fingerprint(g),
            _ => return,
        };
        if self.by_fingerprint.contains_key(&fp) {
            return;
        }
        let id = format!("grad{}", self.entries.len() + 1);
        self.entries.push((id.clone(), paint.clone()));
        self.by_fingerprint.insert(fp, id);
    }

    fn lookup(&self, paint: &Paint) -> Option<&str> {
        let fp = match paint {
            Paint::LinearGradient(g) => linear_fingerprint(g),
            Paint::RadialGradient(g) => radial_fingerprint(g),
            _ => return None,
        };
        self.by_fingerprint.get(&fp).map(String::as_str)
    }
}

fn linear_fingerprint(g: &LinearGradient) -> String {
    let mut s = format!(
        "L:{}:{}:{}:{}:{}:",
        trim_float(g.start.x),
        trim_float(g.start.y),
        trim_float(g.end.x),
        trim_float(g.end.y),
        spread_str(g.spread),
    );
    for stop in &g.stops {
        s.push_str(&format!(
            "{}:{},{},{},{};",
            trim_float(stop.offset),
            stop.color.r,
            stop.color.g,
            stop.color.b,
            stop.color.a
        ));
    }
    s
}

fn radial_fingerprint(g: &RadialGradient) -> String {
    let mut s = format!(
        "R:{}:{}:{}:{}:",
        trim_float(g.center.x),
        trim_float(g.center.y),
        trim_float(g.radius),
        spread_str(g.spread),
    );
    if let Some(f) = g.focal {
        s.push_str(&format!("{},{}", trim_float(f.x), trim_float(f.y)));
    }
    s.push(':');
    for stop in &g.stops {
        s.push_str(&format!(
            "{}:{},{},{},{};",
            trim_float(stop.offset),
            stop.color.r,
            stop.color.g,
            stop.color.b,
            stop.color.a
        ));
    }
    s
}

fn walk_collect_defs(
    group: &Group,
    gradients: &mut GradientCollector,
    clips: &mut ClipPathCollector,
    masks: &mut MaskCollector,
) {
    if let Some(clip) = &group.clip {
        clips.ensure(clip);
    }
    for child in &group.children {
        walk_collect_defs_node(child, gradients, clips, masks);
    }
}

fn walk_collect_defs_node(
    node: &Node,
    gradients: &mut GradientCollector,
    clips: &mut ClipPathCollector,
    masks: &mut MaskCollector,
) {
    match node {
        Node::Path(p) => {
            if let Some(paint) = &p.fill {
                gradients.ensure(paint);
            }
            if let Some(s) = &p.stroke {
                gradients.ensure(&s.paint);
            }
        }
        Node::Group(g) => walk_collect_defs(g, gradients, clips, masks),
        Node::SoftMask {
            mask,
            mask_kind,
            content,
        } => {
            let mask_node = (**mask).clone();
            // Recurse into the mask + content subtrees so any nested
            // gradients / clips / masks also get registered.
            walk_collect_defs_node(mask, gradients, clips, masks);
            walk_collect_defs_node(content, gradients, clips, masks);
            masks.ensure(*mask_kind, mask_node);
        }
        Node::Image(_) => {}
        // `Node` is `#[non_exhaustive]` upstream; ignore unknown
        // variants when collecting referenced paints.
        _ => {}
    }
}

/// Collects every distinct clip [`Path`] referenced by a Group's
/// `clip` field so the encoder can emit a single `<clipPath>` def per
/// unique clip.
#[derive(Default)]
struct ClipPathCollector {
    entries: Vec<(String, Path)>,
    by_fp: HashMap<String, String>,
    /// Round 372 — synth-id → original-`clip-path`-ref-id overrides. When
    /// a clip's fingerprint is bound to a source `<clipPath id="...">`,
    /// the encoder re-emits that original id everywhere (reference
    /// attribute + verbatim def) instead of the synthesised `clip{N}`.
    id_override: HashMap<String, String>,
}

impl ClipPathCollector {
    fn ensure(&mut self, path: &Path) {
        let fp = path_fingerprint(path);
        if self.by_fp.contains_key(&fp) {
            return;
        }
        let id = format!("clip{}", self.entries.len() + 1);
        self.entries.push((id.clone(), path.clone()));
        self.by_fp.insert(fp, id);
    }

    fn lookup(&self, path: &Path) -> Option<&str> {
        let synth = self.by_fp.get(&path_fingerprint(path))?;
        // Round 372 — prefer the original source id when this clip is
        // bound to a verbatim `<clipPath>` def.
        Some(self.id_override.get(synth).unwrap_or(synth).as_str())
    }
}

/// Collects every distinct soft-mask subtree referenced inside the
/// scene graph, keyed by `(MaskKind, structural fingerprint)`.
#[derive(Default)]
struct MaskCollector {
    entries: Vec<(String, MaskKind, Node)>,
    by_fp: HashMap<String, String>,
    /// Round 372 — synth-id → original-`mask`-ref-id overrides (see
    /// [`ClipPathCollector::id_override`]).
    id_override: HashMap<String, String>,
}

impl MaskCollector {
    fn ensure(&mut self, kind: MaskKind, node: Node) {
        let fp = format!("{:?}:{}", kind, node_fingerprint(&node));
        if self.by_fp.contains_key(&fp) {
            return;
        }
        let id = format!("mask{}", self.entries.len() + 1);
        self.by_fp.insert(fp, id.clone());
        self.entries.push((id, kind, node));
    }

    fn lookup(&self, kind: MaskKind, node: &Node) -> Option<&str> {
        let fp = format!("{:?}:{}", kind, node_fingerprint(node));
        let synth = self.by_fp.get(&fp)?;
        Some(self.id_override.get(synth).unwrap_or(synth).as_str())
    }
}

/// Round 372 — the [`MaskCollector`] dedup key for a `(kind, mask
/// subtree)` pair, exposed so [`crate::decoder`] can pre-compute the
/// fingerprint for a `mask="url(#id)"` reference binding and route the
/// verbatim-`<mask>`-def substitution. Must stay byte-identical to the
/// `format!` in [`MaskCollector::ensure`] / `lookup`.
pub(crate) fn mask_fingerprint(kind: MaskKind, node: &Node) -> String {
    format!("{:?}:{}", kind, node_fingerprint(node))
}

pub(crate) fn path_fingerprint(p: &Path) -> String {
    let mut s = String::with_capacity(p.commands.len() * 12);
    for cmd in &p.commands {
        match cmd {
            PathCommand::MoveTo(p) => {
                s.push_str(&format!("M{},{};", trim_float(p.x), trim_float(p.y)))
            }
            PathCommand::LineTo(p) => {
                s.push_str(&format!("L{},{};", trim_float(p.x), trim_float(p.y)))
            }
            PathCommand::QuadCurveTo { control, end } => s.push_str(&format!(
                "Q{},{},{},{};",
                trim_float(control.x),
                trim_float(control.y),
                trim_float(end.x),
                trim_float(end.y)
            )),
            PathCommand::CubicCurveTo { c1, c2, end } => s.push_str(&format!(
                "C{},{},{},{},{},{};",
                trim_float(c1.x),
                trim_float(c1.y),
                trim_float(c2.x),
                trim_float(c2.y),
                trim_float(end.x),
                trim_float(end.y)
            )),
            PathCommand::ArcTo {
                rx,
                ry,
                x_axis_rot,
                large_arc,
                sweep,
                end,
            } => s.push_str(&format!(
                "A{},{},{},{},{},{},{};",
                trim_float(*rx),
                trim_float(*ry),
                trim_float(*x_axis_rot),
                if *large_arc { 1 } else { 0 },
                if *sweep { 1 } else { 0 },
                trim_float(end.x),
                trim_float(end.y)
            )),
            PathCommand::Close => s.push_str("Z;"),
            _ => s.push('?'),
        }
    }
    s
}

fn node_fingerprint(n: &Node) -> String {
    match n {
        Node::Path(p) => format!("P({})", path_fingerprint(&p.path)),
        Node::Group(g) => {
            let mut s = String::from("G(");
            s.push_str(&format!(
                "t={:?}o={:?};",
                (
                    g.transform.a,
                    g.transform.b,
                    g.transform.c,
                    g.transform.d,
                    g.transform.e,
                    g.transform.f
                ),
                g.opacity
            ));
            for c in &g.children {
                s.push_str(&node_fingerprint(c));
                s.push(',');
            }
            s.push(')');
            s
        }
        Node::SoftMask {
            mask,
            mask_kind,
            content,
        } => format!(
            "SM({:?};{};{})",
            mask_kind,
            node_fingerprint(mask),
            node_fingerprint(content)
        ),
        Node::Image(_) => "I".to_string(),
        _ => "?".to_string(),
    }
}

fn write_clip_path(out: &mut String, id: &str, path: &Path, clip_rule: Option<&str>) {
    out.push_str(&format!(
        "    <clipPath id=\"{}\">\n      <path d=\"",
        escape_attr(id)
    ));
    write_path_d(out, &path.commands);
    // Round 215 — SVG 1.1 §14.3.5 `clip-rule` attribute on the inner
    // `<path>` element. The property only applies to graphics elements
    // inside `<clipPath>`, and the encoder always emits a single
    // `<path>` per clipPath (merged from the source's children), so
    // the rule lives on this child rather than on the `<clipPath>`
    // element itself.
    if let Some(rule) = clip_rule {
        out.push_str("\" clip-rule=\"");
        out.push_str(&escape_attr(rule));
    }
    out.push_str("\"/>\n    </clipPath>\n");
}

fn write_mask(
    out: &mut String,
    id: &str,
    kind: MaskKind,
    content: &Node,
    gradients: &GradientCollector,
) {
    let kind_attr = match kind {
        MaskKind::Luminance => "",
        MaskKind::Alpha => " mask-type=\"alpha\"",
    };
    out.push_str(&format!(
        "    <mask id=\"{}\"{}>\n",
        escape_attr(id),
        kind_attr
    ));
    // Mask content is emitted under the defs section. We pass empty
    // clip / mask collectors here because nested clip-paths / masks
    // inside a mask are an edge case (deferred): downstream rasterizer
    // doesn't support them either. Round 13 — pass empty id_paths
    // / animation maps too (the mask subtree is a def, not part of
    // the live scene graph that animations attach to).
    let empty_clips = ClipPathCollector::default();
    let empty_masks = MaskCollector::default();
    let empty_idx = EmitIndex::default();
    let mut empty_stack: Vec<usize> = Vec::new();
    write_node(
        out,
        content,
        3,
        gradients,
        &empty_clips,
        &empty_masks,
        &empty_idx,
        &mut empty_stack,
    );
    out.push_str("    </mask>\n");
}

fn write_gradient(out: &mut String, id: &str, paint: &Paint) {
    match paint {
        Paint::LinearGradient(g) => {
            out.push_str(&format!(
                "    <linearGradient id=\"{}\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" gradientUnits=\"userSpaceOnUse\"",
                escape_attr(id),
                trim_float(g.start.x),
                trim_float(g.start.y),
                trim_float(g.end.x),
                trim_float(g.end.y)
            ));
            if g.spread != SpreadMethod::Pad {
                out.push_str(&format!(" spreadMethod=\"{}\"", spread_str(g.spread)));
            }
            out.push_str(">\n");
            for stop in &g.stops {
                write_stop(out, *stop);
            }
            out.push_str("    </linearGradient>\n");
        }
        Paint::RadialGradient(g) => {
            out.push_str(&format!(
                "    <radialGradient id=\"{}\" cx=\"{}\" cy=\"{}\" r=\"{}\" gradientUnits=\"userSpaceOnUse\"",
                escape_attr(id),
                trim_float(g.center.x),
                trim_float(g.center.y),
                trim_float(g.radius)
            ));
            if let Some(f) = g.focal {
                out.push_str(&format!(
                    " fx=\"{}\" fy=\"{}\"",
                    trim_float(f.x),
                    trim_float(f.y)
                ));
            }
            if g.spread != SpreadMethod::Pad {
                out.push_str(&format!(" spreadMethod=\"{}\"", spread_str(g.spread)));
            }
            out.push_str(">\n");
            for stop in &g.stops {
                write_stop(out, *stop);
            }
            out.push_str("    </radialGradient>\n");
        }
        _ => {}
    }
}

fn write_stop(out: &mut String, stop: oxideav_core::GradientStop) {
    let color = format!(
        "#{:02x}{:02x}{:02x}",
        stop.color.r, stop.color.g, stop.color.b
    );
    out.push_str(&format!(
        "      <stop offset=\"{}\" stop-color=\"{}\"",
        trim_float(stop.offset),
        color
    ));
    if stop.color.a != 255 {
        out.push_str(&format!(
            " stop-opacity=\"{}\"",
            trim_float(stop.color.a as f32 / 255.0)
        ));
    }
    out.push_str("/>\n");
}

fn spread_str(s: SpreadMethod) -> &'static str {
    match s {
        SpreadMethod::Pad => "pad",
        SpreadMethod::Reflect => "reflect",
        SpreadMethod::Repeat => "repeat",
    }
}

// ---------------------------------------------------------------------------
// Encoder trait adapter
// ---------------------------------------------------------------------------

pub fn make_encoder(_params: &oxideav_core::CodecParameters) -> Result<Box<dyn Encoder>> {
    let mut out_params =
        oxideav_core::CodecParameters::video(oxideav_core::CodecId::new(CODEC_ID_STR));
    out_params.media_type = oxideav_core::MediaType::Video;
    Ok(Box::new(SvgEncoder {
        codec_id: oxideav_core::CodecId::new(CODEC_ID_STR),
        out_params,
        pending: None,
        eof: false,
    }))
}

struct SvgEncoder {
    codec_id: oxideav_core::CodecId,
    out_params: oxideav_core::CodecParameters,
    pending: Option<Vec<u8>>,
    eof: bool,
}

impl Encoder for SvgEncoder {
    fn codec_id(&self) -> &oxideav_core::CodecId {
        &self.codec_id
    }
    fn output_params(&self) -> &oxideav_core::CodecParameters {
        &self.out_params
    }
    fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        let vf = match frame {
            Frame::Vector(v) => v,
            _ => return Err(Error::invalid("SVG encoder: expected vector frame")),
        };
        self.pending = Some(write_svg(vf));
        Ok(())
    }
    fn receive_packet(&mut self) -> Result<Packet> {
        match self.pending.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
            None => {
                if self.eof {
                    Err(Error::Eof)
                } else {
                    Err(Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> Result<()> {
        self.eof = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::{FillRule, GradientStop, Group, Node, Path, PathNode, Point, Rgba, ViewBox};

    fn make_simple_frame() -> VectorFrame {
        let mut path = Path::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(10.0, 0.0));
        path.line_to(Point::new(10.0, 10.0));
        path.close();
        let pn = PathNode {
            path,
            fill: Some(Paint::Solid(Rgba::opaque(255, 0, 0))),
            stroke: None,
            fill_rule: FillRule::NonZero,
        };
        VectorFrame {
            width: 10.0,
            height: 10.0,
            view_box: Some(ViewBox {
                min_x: 0.0,
                min_y: 0.0,
                width: 10.0,
                height: 10.0,
            }),
            root: Group {
                children: vec![Node::Path(pn)],
                ..Group::default()
            },
            pts: None,
            time_base: TimeBase::new(1, 1),
        }
    }

    #[test]
    fn writes_minimal_svg_with_red_triangle() {
        let bytes = write_svg(&make_simple_frame());
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.starts_with("<?xml"));
        assert!(s.contains("<svg"));
        assert!(s.contains("fill=\"#ff0000\""));
        assert!(s.contains("d=\"M 0 0 L 10 0 L 10 10 Z\""));
        assert!(s.ends_with("</svg>\n"));
    }

    #[test]
    fn trim_float_is_compact() {
        assert_eq!(trim_float(1.5), "1.5");
        assert_eq!(trim_float(2.0), "2");
        assert_eq!(trim_float(-0.0), "0");
        assert_eq!(trim_float(0.123456), "0.123456");
    }

    #[test]
    fn writes_gradient_def_when_referenced() {
        let stops = vec![
            GradientStop::new(0.0, Rgba::opaque(255, 0, 0)),
            GradientStop::new(1.0, Rgba::opaque(0, 0, 255)),
        ];
        let lg = LinearGradient {
            start: Point::new(0.0, 0.0),
            end: Point::new(10.0, 0.0),
            stops,
            spread: SpreadMethod::Pad,
        };
        let mut path = Path::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(10.0, 10.0));
        let frame = VectorFrame {
            width: 10.0,
            height: 10.0,
            view_box: None,
            root: Group {
                children: vec![Node::Path(PathNode {
                    path,
                    fill: Some(Paint::LinearGradient(lg)),
                    stroke: None,
                    fill_rule: FillRule::NonZero,
                })],
                ..Group::default()
            },
            pts: None,
            time_base: TimeBase::new(1, 1),
        };
        let s = String::from_utf8(write_svg(&frame)).unwrap();
        assert!(s.contains("<defs>"));
        assert!(s.contains("<linearGradient"));
        assert!(s.contains("url(#grad1)"));
    }
}
