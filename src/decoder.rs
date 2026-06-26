//! Top-level SVG → [`VectorFrame`] entry point and the
//! pipeline-friendly [`Decoder`] adapter.

use oxideav_core::{
    CodecId, CodecParameters, Decoder, Error, Frame, Group, Packet, Result, TimeBase, Transform2D,
    VectorFrame, ViewBox,
};

use crate::css::MatchContext;
use crate::element::{
    derive_child_ctx, flatten_gradient_to_paint, parse_clip_path_def, parse_clip_rule_attr,
    parse_element_to_node_ctx, parse_filter_def, parse_linear_gradient_def, parse_marker_def,
    parse_mask_def, parse_number, parse_pattern_def, parse_radial_gradient_def, parse_symbol_def,
    parse_view_def, PaintState, ParseContext,
};
use crate::filter::{MeetOrSlice, PreserveAspectRatio, PreserveAspectRatioAlign};
use crate::length::ResolveContext;
use crate::parser::{
    attr, decode_utf8_lossy_stripping_bom, inflate_gzip, is_gzip, parse_xml, tag_local, Element,
    Node as XmlNode,
};
use crate::preserved::{
    AnimationFragment, ClipRuleBinding, ColorInterpolationBinding, ColorRenderingBinding,
    CursorBinding, DescriptiveBinding, DominantBaselineBinding, IdScenePath, LinkBinding,
    OverflowBinding, PaintOrderBinding, PathLengthBinding, PointerEventsBinding, PreservedExtras,
    ShapeRenderingBinding, SwitchBinding, TextRenderingBinding, UseBinding, VectorEffectBinding,
};

/// Codec id string for SVG vector frames.
pub const CODEC_ID_STR: &str = "svg";

/// Parse a complete SVG document into a [`VectorFrame`].
///
/// Round 3: transparently inflates `.svgz` (gzip-compressed) input —
/// the magic-bytes sniff (`1f 8b`) means callers can hand us either
/// flavour without having to pre-decompress.
///
/// Equivalent to `parse_svg_at(bytes, 0.0)` — animations snapshot at
/// `t=0` to reproduce first-paint behaviour.
pub fn parse_svg(bytes: &[u8]) -> Result<VectorFrame> {
    parse_svg_at(bytes, 0.0)
}

/// Round 4 — parse a complete SVG document at a specific timeline
/// point `t_seconds`. Every `<animate>` / `<set>` / `<animateTransform>`
/// is evaluated at the requested time using the full SMIL timing model
/// (begin / dur / repeatCount / keyTimes / values / from-to-by) and
/// folded into its parent's attribute set before the scene graph is
/// built. `t_seconds = 0.0` matches `parse_svg`.
pub fn parse_svg_at(bytes: &[u8], t_seconds: f32) -> Result<VectorFrame> {
    parse_svg_at_with_languages(bytes, t_seconds, &[])
}

/// Round 98 — parse with an explicit user-preferred language list, used
/// by SVG 2 §5.7.3 `<switch>` conditional processing when it evaluates
/// a child's `systemLanguage` (§5.7.5) test attribute.
///
/// `system_language` carries the "language tags indicated by user
/// preferences" the spec matches against (oxideav owns no user-agent
/// locale registry, so the caller supplies it — e.g. `&["en", "fr"]`).
/// `parse_svg` / `parse_svg_at` pass an empty list: an absent
/// `systemLanguage` still implicitly evaluates to true, but a present,
/// non-empty one then matches nothing, so a `<switch>` falls through to
/// the first child without a language test (the spec-recommended
/// "catch-all" choice).
pub fn parse_svg_at_with_languages(
    bytes: &[u8],
    t_seconds: f32,
    system_language: &[&str],
) -> Result<VectorFrame> {
    let inflated;
    let raw: &[u8] = if is_gzip(bytes) {
        inflated = inflate_gzip(bytes)?;
        &inflated
    } else {
        bytes
    };
    let text = decode_utf8_lossy_stripping_bom(raw);
    let nodes = parse_xml(&text)?;
    let svg =
        find_svg_root(&nodes).ok_or_else(|| Error::invalid("SVG: missing <svg> root element"))?;
    let langs: Vec<String> = system_language.iter().map(|s| s.to_string()).collect();
    let (frame, _, _, _, _, _, _, _, _, _, _, _, _, _, _, _, _, _) =
        parse_svg_root(svg, t_seconds, false, &langs)?;
    Ok(frame)
}

/// Round 4 — parse and *also* return a [`PreservedExtras`] side-channel
/// holding `<style>`, `<filter>`, `<animate>`, and `<foreignObject>`
/// element trees the scene-graph representation can't fully express.
///
/// Pair with [`crate::encoder::write_svg_with_extras`] for a structural
/// round-trip that doesn't drop the dynamic / filter / CSS pieces.
pub fn parse_svg_with_extras(bytes: &[u8]) -> Result<(VectorFrame, PreservedExtras)> {
    let inflated;
    let raw: &[u8] = if is_gzip(bytes) {
        inflated = inflate_gzip(bytes)?;
        &inflated
    } else {
        bytes
    };
    let text = decode_utf8_lossy_stripping_bom(raw);
    let nodes = parse_xml(&text)?;
    let svg =
        find_svg_root(&nodes).ok_or_else(|| Error::invalid("SVG: missing <svg> root element"))?;
    let mut extras = PreservedExtras::new();
    collect_extras(svg, &mut extras, None);
    // Round 215 — SVG 1.1 §14.3.5 `clip-rule` side-channel collection.
    // The binding records the resolved rule (and path-bytes
    // fingerprint) for every `<clipPath>` whose rule deviates from the
    // §14.3.5 initial value `nonzero` OR whose author explicitly wrote
    // a `clip-rule=` keyword anywhere in the subtree.
    collect_clip_rule_bindings(svg, &mut extras);
    extras.root_preserve_aspect_ratio = attr(svg, "preserveAspectRatio").map(str::to_string);
    // Round 95 — independent of the scene-graph build, walk the source
    // XML for every id-bearing `<view>` so a caller resolving an SVG
    // fragment identifier (`MyDrawing.svg#MyView`) has the typed view
    // parameters at hand without re-parsing the document. The verbatim
    // `<view>` capture in [`collect_extras`] takes care of round-trip
    // emission; this pass populates the typed mirror keyed by id.
    collect_typed_views(svg, &mut extras.typed_views);
    let (
        frame,
        id_paths,
        path_lengths,
        links,
        titles,
        descs,
        paint_orders,
        vector_effects,
        shape_renderings,
        text_renderings,
        color_renderings,
        color_interpolations,
        overflows,
        pointer_eventss,
        cursors,
        dominant_baselines,
        uses,
        switches,
    ) = parse_svg_root(svg, 0.0, true, &[])?;
    extras.id_paths = id_paths;
    extras.path_lengths = path_lengths;
    extras.links = links;
    extras.titles = titles;
    extras.descs = descs;
    extras.paint_orders = paint_orders;
    extras.vector_effects = vector_effects;
    extras.shape_renderings = shape_renderings;
    extras.text_renderings = text_renderings;
    extras.color_renderings = color_renderings;
    extras.color_interpolations = color_interpolations;
    extras.overflows = overflows;
    extras.pointer_eventss = pointer_eventss;
    extras.cursors = cursors;
    extras.dominant_baselines = dominant_baselines;
    extras.uses = uses;
    extras.switches = switches;
    Ok((frame, extras))
}

/// Round 95 — walk the source XML for every `<view id="...">` and
/// populate the typed mirror on the caller's `extras.typed_views` map.
/// Mirrors the verbatim collection in [`collect_extras`] but emits the
/// typed [`crate::defs::ViewDef`] shape.
fn collect_typed_views(
    el: &Element,
    out: &mut std::collections::HashMap<String, crate::defs::ViewDef>,
) {
    if tag_local(&el.name) == "view" {
        if let Some((id, def)) = crate::element::parse_view_def(el) {
            out.insert(id, def);
        }
    }
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            collect_typed_views(c, out);
        }
    }
}

/// Walk the source XML once to populate `extras` with every preservable
/// element. `current_id` carries the nearest ancestor's id so animation
/// fragments know which emit-site they belong to.
fn collect_extras(el: &Element, extras: &mut PreservedExtras, current_id: Option<&str>) {
    let local = tag_local(&el.name);
    let id = attr(el, "id").map(str::to_string);
    let next_id = id.as_deref().or(current_id);
    match local.as_str() {
        "style" => {
            // Body of the <style> element is its concatenated text
            // children.
            let mut body = String::new();
            for child in &el.children {
                if let XmlNode::Text(t) = child {
                    body.push_str(t);
                }
            }
            if !body.trim().is_empty() {
                extras.styles.push(body);
            }
        }
        "filter" => {
            extras.filters.push(el.clone());
        }
        "pattern" => {
            // Round 20 — capture the verbatim <pattern> for round-trip
            // re-emission. The decoder's pre-walk separately builds a
            // typed [`crate::defs::PatternDef`] for downstream
            // consumers, but the verbatim XML is the round-trip
            // source of truth.
            extras.patterns.push(el.clone());
        }
        "marker" => {
            // Round 104 — capture the verbatim <marker> for round-trip
            // re-emission (SVG 2 §13.7.1). The pre-walk separately
            // builds a typed [`crate::defs::MarkerDef`]; this verbatim
            // element is the round-trip source of truth (attribute
            // ordering, descriptive children, content shapes).
            extras.markers.push(el.clone());
        }
        "lineargradient" | "radialgradient" => {
            // Round 81 — verbatim gradient capture. The typed view on
            // `DefsTables::gradients` carries the resolved geometry +
            // template chain for downstream consumers; this verbatim
            // element is the round-trip source of truth so an author's
            // `gradientUnits` / `gradientTransform` / `href` survive a
            // `parse_svg_with_extras → write_svg_with_extras` cycle.
            extras.gradients.push(el.clone());
        }
        "foreignobject" => {
            extras.foreign_objects.push(el.clone());
        }
        "animate" | "set" | "animatetransform" | "animatemotion" => {
            extras.animations.push(AnimationFragment {
                parent_id: current_id.map(str::to_string),
                element: el.clone(),
            });
        }
        "script" => {
            // Round 12: capture <script> verbatim so the round-trip
            // preserves it. The decoder NEVER executes the body.
            extras.scripts.push(el.clone());
        }
        "image" => {
            // Round 15: capture <image> with parsed href (inline data
            // URI decoded, external URL captured) + dimensions. The
            // encoder re-emits each on round-trip.
            if let Some(img) = crate::image::SvgImage::from_element(el, current_id) {
                extras.images.push(img);
            }
        }
        "view" => {
            // Round 95 — verbatim <view> capture per SVG 2 §16.3.3.
            // The typed parse on `DefsTables::views` lets a caller
            // resolve a fragment identifier; this side-channel makes
            // sure the source XML (descriptive children, attribute
            // ordering, any attributes the typed view doesn't model)
            // round-trips byte-faithfully on
            // `write_svg_with_extras`.
            extras.views.push(el.clone());
        }
        "defs" => {
            // Round 372 — capture id-bearing reference targets housed in
            // `<defs>` that produce no scene-graph node and have no other
            // typed round-trip carrier (SVG 1.1 §5.5). The canonical
            // `<use>` target is a plain shape / `<g>` / `<symbol>` inside
            // `<defs>`; without this capture a round-tripped
            // `<use href="#id">` would dangle. Gradients / filters /
            // patterns / markers / `<style>` are skipped by
            // `capture_defs_target` because they already ride their own
            // side-channels (the recursion below still reaches them).
            for child in &el.children {
                if let XmlNode::Element(c) = child {
                    capture_defs_target(c, extras);
                }
            }
            // Fall through to the generic recursion below so nested
            // `<style>` / `<filter>` inside `<defs>` reach their own
            // arms.
        }
        // Round 372 — a `<symbol>` is itself a valid `<use>` target
        // (SVG 2 §5.6) and produces no scene-graph node, so capture it
        // whole (its children come along verbatim) when it has an `id`.
        // A bare id-less `<symbol>` cannot be referenced, so it is
        // skipped (the match guard). The recursion below still descends
        // so nested `<style>` / `<filter>` inside the symbol reach their
        // arms, but the symbol's own shape children are NOT recorded as
        // independent targets (they're part of the captured symbol).
        "symbol" if attr(el, "id").is_some() => {
            extras.defs_targets.push(el.clone());
        }
        "metadata" => {
            // Round 122 — SVG 2 §5.9 `<metadata>` content model is "any
            // elements or character data" (typically RDF / Dublin Core
            // / foreign-namespace markup from upstream authoring
            // tooling). A structured parse is out of scope —
            // we capture the whole element verbatim and re-emit at
            // the trailing edge of the document so a `parse → write`
            // round-trip preserves embedded provenance / licensing /
            // catalogue metadata. Per §5.9 the UA stylesheet forces
            // `display:none`, so the element never enters the
            // rendering tree.
            extras.metadata.push(el.clone());
        }
        _ => {}
    }
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            collect_extras(c, extras, next_id);
        }
    }
}

/// Round 215 — SVG 1.1 §14.3.5 `clip-rule` side-channel collection.
/// Walks the source XML for every `<clipPath id="...">` and, when the
/// resolved rule diverges from the §14.3.5 initial value `nonzero` OR
/// the author explicitly wrote a `clip-rule=` keyword, parses the
/// clipPath into a [`crate::defs::ClipPathDef`] and records a
/// [`ClipRuleBinding`] keyed by the path-bytes fingerprint the encoder
/// uses for its own clip-path dedup.
///
/// Resolution (matching [`crate::element::parse_clip_path_def`]):
///   1. The `<clipPath>` element's own `clip-rule=` (if any) sets the
///      inherited default for children.
///   2. The first contributing shape child's `clip-rule=` (if any)
///      overrides the inherited default.
///
/// Recording rule: a binding is emitted when the resolved rule is
/// non-default (`evenodd`) OR when the author explicitly wrote a
/// `clip-rule=` keyword anywhere in the clipPath subtree. An
/// initial-value (`nonzero`) document with no explicit attribute
/// records nothing.
fn collect_clip_rule_bindings(el: &Element, extras: &mut PreservedExtras) {
    if tag_local(&el.name) == "clippath" {
        collect_one_clip_rule_binding(el, extras);
    }
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            collect_clip_rule_bindings(c, extras);
        }
    }
}

fn collect_one_clip_rule_binding(el: &Element, extras: &mut PreservedExtras) {
    let id = match attr(el, "id") {
        Some(v) => v.to_string(),
        None => return, // Id-less clipPath can't be referenced.
    };
    let inherited = attr(el, "clip-rule");
    // First child shape's explicit `clip-rule=`, if any.
    let mut first_child_rule: Option<&str> = None;
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            let local = tag_local(&c.name);
            if matches!(
                local.as_str(),
                "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "path"
            ) {
                first_child_rule = attr(c, "clip-rule");
                break;
            }
        }
    }
    let explicit = first_child_rule.or(inherited);
    let resolved = parse_clip_rule_attr(explicit);
    let author_explicit = explicit.is_some_and(|v| parse_clip_rule_attr(Some(v)).is_some());
    let keyword = match resolved {
        Some(oxideav_core::FillRule::EvenOdd) => "evenodd",
        Some(oxideav_core::FillRule::NonZero) if author_explicit => "nonzero",
        // Initial value with no explicit author keyword — nothing to
        // record (the round-trip honours the §14.3.5 initial silently).
        _ => return,
    };
    // Re-parse the clipPath subtree to compute the path-bytes
    // fingerprint the encoder uses for its dedup. We construct a
    // fresh [`ParseContext`] for this side pass (independent of the
    // main scene-walk's context) so the binding collection doesn't
    // mutate `ctx.defs`.
    let mut ctx = ParseContext::new();
    let Ok(Some((_, def))) = parse_clip_path_def(el, &mut ctx) else {
        return;
    };
    let path_fingerprint = crate::encoder::path_fingerprint(&def.path);
    extras.clip_rules.push(ClipRuleBinding {
        clip_path_id: id,
        path_fingerprint,
        clip_rule: keyword.to_string(),
    });
}

/// Round 372 — record a single `<defs>` / `<symbol>` child as a verbatim
/// reference target when it is an id-bearing shape / container that has
/// no other typed round-trip carrier. Used by [`collect_extras`] so a
/// `<use href="#id">` the encoder re-emits still resolves after a
/// `parse_svg_with_extras → write_svg_with_extras` cycle.
///
/// The typed kinds (`linearGradient` / `radialGradient` / `filter` /
/// `pattern` / `marker` / `mask` / `clipPath` / `style` / `view`) are
/// skipped — they already round-trip via their own side-channels and
/// the [`collect_extras`] recursion reaches them independently. An
/// id-less child is skipped (it can never be a `<use>` target).
fn capture_defs_target(el: &Element, extras: &mut PreservedExtras) {
    let local = tag_local(&el.name);
    // Skip kinds that already have a dedicated verbatim carrier, plus
    // never-target metadata kinds.
    if matches!(
        local.as_str(),
        "lineargradient"
            | "radialgradient"
            | "filter"
            | "pattern"
            | "marker"
            | "mask"
            | "clippath"
            | "style"
            | "view"
            | "metadata"
            | "title"
            | "desc"
            // `<symbol>` rides the dedicated `"symbol"` arm in
            // `collect_extras` (the recursion reaches it), so skip it
            // here to avoid a duplicate capture.
            | "symbol"
            // These ride their own verbatim carriers + the recursion
            // reaches them independently.
            | "foreignobject"
            | "script"
            | "image"
            | "animate"
            | "set"
            | "animatetransform"
            | "animatemotion"
            // A nested `<defs>` is walked by the recursion's `"defs"`
            // arm (its own id-bearing children get captured there).
            | "defs"
    ) {
        return;
    }
    // Only id-bearing elements can be referenced by `<use>`.
    if attr(el, "id").is_none() {
        return;
    }
    extras.defs_targets.push(el.clone());
}

fn find_svg_root(nodes: &[XmlNode]) -> Option<&Element> {
    for n in nodes {
        if let XmlNode::Element(e) = n {
            if tag_local(&e.name) == "svg" {
                return Some(e);
            }
        }
    }
    None
}

type SvgRootParse = (
    VectorFrame,
    Vec<IdScenePath>,
    Vec<PathLengthBinding>,
    Vec<LinkBinding>,
    Vec<DescriptiveBinding>,
    Vec<DescriptiveBinding>,
    Vec<PaintOrderBinding>,
    Vec<VectorEffectBinding>,
    Vec<ShapeRenderingBinding>,
    Vec<TextRenderingBinding>,
    Vec<ColorRenderingBinding>,
    Vec<ColorInterpolationBinding>,
    Vec<OverflowBinding>,
    Vec<PointerEventsBinding>,
    Vec<CursorBinding>,
    Vec<DominantBaselineBinding>,
    Vec<UseBinding>,
    Vec<SwitchBinding>,
);

fn parse_svg_root(
    svg: &Element,
    t_seconds: f32,
    track_id_paths: bool,
    system_language: &[String],
) -> Result<SvgRootParse> {
    let view_box = match attr(svg, "viewBox") {
        Some(v) => Some(parse_view_box(v)?),
        None => None,
    };

    // `width` / `height` default to 100% — but round 1 needs concrete
    // numbers to populate the frame, so fall back to the viewBox size
    // when the attributes are missing or are percentages.
    let width = parse_length_or_default(
        attr(svg, "width"),
        view_box.map(|vb| vb.width).unwrap_or(0.0),
    )?;
    let height = parse_length_or_default(
        attr(svg, "height"),
        view_box.map(|vb| vb.height).unwrap_or(0.0),
    )?;

    let parent_state = PaintState::default();
    // Round 19 — seed the root [`ResolveContext`] from the SVG root's
    // viewport (so descendant `vw` / `vh` / `vmin` / `vmax` resolve
    // against the document size) and the spec-default 16 px font-size
    // (CSS Values L4 §6.1.2). The root's own `font-size` cascade — if
    // any — gets folded in below via `derive_child_ctx` so a
    // `<svg font-size="20">` propagates `1em → 20` to descendants.
    let root_ctx = ResolveContext::default().with_viewport(width, height);
    let mut ctx = ParseContext::new()
        .with_time(t_seconds)
        .with_resolve_ctx(root_ctx)
        .with_system_language(system_language.to_vec());
    if track_id_paths {
        ctx.enable_id_path_tracking();
    }

    // Round-4 step 0: collect every `<style>` block's body into the
    // ParseContext stylesheet. Done before the def + element walks so
    // class/id selectors resolve regardless of source order.
    crate::css::collect_stylesheet(svg, &mut ctx.stylesheet);

    // Round 19 — fold any root `<svg font-size="...">` into the
    // resolve context AFTER the stylesheet is collected (so a CSS
    // rule that targets the root element wins over the presentation
    // attr per the round-4 cascade). The root's font-size is also the
    // `rem` basis for every descendant.
    let svg_mctx_seed = MatchContext::root(svg);
    let cascaded = derive_child_ctx(svg, &svg_mctx_seed, &ctx.stylesheet, &ctx.resolve_ctx);
    // Pin the root font-size as `rem`'s basis — every descendant's
    // `1rem` resolves against this.
    let cascaded = ResolveContext {
        root_font_size_px: cascaded.font_size_px,
        ..cascaded
    };
    ctx.resolve_ctx = cascaded;

    // First pass: register every <defs> child + every gradient /
    // filter / mask / clipPath / symbol seen anywhere in the tree, so
    // forward references inside the doc work regardless of declaration
    // order.
    register_all_defs(svg, &mut ctx)?;

    // Round 279 — SVG 1.1 §15.3 cross-filter `xlink:href` inheritance.
    // Done AFTER `register_all_defs` so a forward reference
    // (`<filter id="b" xlink:href="#a">` declared before
    // `<filter id="a">`) resolves correctly, mirroring the gradient
    // template pass below. Only the typed graph is re-parsed from the
    // merged effective element; `FilterDef::element` stays the
    // verbatim source for round-trip emission.
    let filter_elements: Vec<crate::parser::Element> = ctx
        .defs
        .filters
        .values()
        .map(|d| d.element.clone())
        .collect();
    for def in ctx.defs.filters.values_mut() {
        if def.graph.href.is_some() {
            let merged =
                crate::filter::resolve_filter_element_chain(&def.element, &filter_elements);
            def.graph = crate::filter::parse_filter_graph(&merged);
        }
    }

    // Round 81 — flatten every typed [`GradientDef`] (via the §14.1.1
    // template chain) into a legacy [`Paint`] for the round-1 fill
    // resolver. Done AFTER `register_all_defs` so a forward `href`
    // reference (`<linearGradient id="b" href="#a">` declared before
    // `<linearGradient id="a">`) resolves correctly.
    let mut gradient_paints: std::collections::HashMap<String, oxideav_core::Paint> =
        std::collections::HashMap::with_capacity(ctx.defs.gradients.len());
    for (id, def) in &ctx.defs.gradients {
        gradient_paints.insert(id.clone(), flatten_gradient_to_paint(def, &ctx.defs));
    }
    for (id, paint) in gradient_paints {
        // Don't clobber a legacy `Paint` already in the table — the
        // few callers that go directly through `parse_linear_gradient`
        // (round-1 unit tests / external code) bypass the def-table
        // entirely.
        ctx.gradients.entry(id).or_insert(paint);
    }

    // Second pass: walk the tree and build the scene graph. Gradients
    // and round-2 defs are now resolvable. Build a per-child
    // [`MatchContext`] so round-5 structural pseudo-classes
    // (`:first-child`, `:nth-of-type`, …) and sibling combinators
    // (`a + b`, `a ~ b`) match against the document tree.
    let mut root = Group::default();
    let svg_mctx = MatchContext::root(svg);
    let (total, tag_totals) = count_element_children(svg);
    let mut child_idx = 0usize;
    let mut tag_seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for child in &svg.children {
        if let XmlNode::Element(c) = child {
            let lower = tag_local(&c.name).to_ascii_lowercase();
            let of_idx = *tag_seen.entry(lower.clone()).or_insert(0);
            *tag_seen.get_mut(&lower).unwrap() += 1;
            let of_count = *tag_totals.get(&lower).unwrap_or(&0);
            let cmctx = MatchContext {
                el: c,
                child_index: child_idx,
                of_type_index: of_idx,
                sibling_count: total,
                of_type_count: of_count,
                parent: Some(&svg_mctx),
            };
            // Round 13: scene-graph index = number of children already
            // pushed onto root.
            let scene_idx = root.children.len();
            ctx.current_path.push(scene_idx);
            let result = parse_element_to_node_ctx(c, &parent_state, &mut ctx, &cmctx);
            ctx.current_path.pop();
            if let Some(node) = result? {
                root.children.push(node);
            }
            child_idx += 1;
        }
    }

    // Round-12 — apply SVG 2 §8.2 viewport-mapping when the root has
    // a viewBox, an explicit width/height, and a non-`none`
    // preserveAspectRatio (or no preserveAspectRatio attribute, in
    // which case the spec default `xMidYMid meet` applies). The
    // raster's natural mapping
    //   `scale(W/vb.w, H/vb.h) * translate(-vb.minX, -vb.minY)`
    // implements the `none` (stretch) variant; we pre-multiply
    // `root.transform` by a correction so the composed result is the
    // spec-mandated translate+scale.
    if let Some(vb) = view_box {
        if vb.width > 0.0 && vb.height > 0.0 && width > 0.0 && height > 0.0 {
            let par = match attr(svg, "preserveAspectRatio") {
                Some(s) => PreserveAspectRatio::from_str(s),
                None => PreserveAspectRatio::default(),
            };
            if let Some(correction) = viewport_correction_transform(width, height, vb, par) {
                root.transform = correction.compose(&root.transform);
            }
        }
    }

    let frame = VectorFrame {
        width,
        height,
        view_box,
        root,
        pts: None,
        time_base: TimeBase::new(1, 1),
    };
    Ok((
        frame,
        ctx.id_paths,
        ctx.path_lengths,
        ctx.links,
        ctx.titles,
        ctx.descs,
        ctx.paint_orders,
        ctx.vector_effects,
        ctx.shape_renderings,
        ctx.text_renderings,
        ctx.color_renderings,
        ctx.color_interpolations,
        ctx.overflows,
        ctx.pointer_eventss,
        ctx.cursors,
        ctx.dominant_baselines,
        ctx.uses,
        ctx.switches,
    ))
}

/// Round-12 — given the root viewport (`width` × `height`), the source
/// `viewBox`, and the parsed `preserveAspectRatio`, return the
/// transform `correction` such that
///   `natural ∘ correction == spec_correct`
/// where `natural = scale(W/vb.w, H/vb.h) * translate(-vb.minX, -vb.minY)`
/// and `spec_correct = translate(tx, ty) * scale(sx, sy) * translate(-vb.minX, -vb.minY)`
/// per SVG 2 §8.2.
///
/// Returns `None` when no correction is needed (the natural mapping
/// already matches the spec — happens when align is `none`, or the
/// viewport already has the same aspect ratio as the viewBox).
fn viewport_correction_transform(
    width: f32,
    height: f32,
    vb: ViewBox,
    par: PreserveAspectRatio,
) -> Option<Transform2D> {
    if matches!(par.align, PreserveAspectRatioAlign::None) {
        // `none` matches the renderer's default — no correction.
        return None;
    }
    let nat_sx = width / vb.width;
    let nat_sy = height / vb.height;
    // Spec algorithm 8.2 steps 5–8.
    let (mut sx, mut sy) = (nat_sx, nat_sy);
    match par.meet_or_slice {
        MeetOrSlice::Meet => {
            // Set the larger of (sx, sy) to the smaller.
            let s = sx.min(sy);
            sx = s;
            sy = s;
        }
        MeetOrSlice::Slice => {
            let s = sx.max(sy);
            sx = s;
            sy = s;
        }
    }
    if (sx - nat_sx).abs() < 1e-6 && (sy - nat_sy).abs() < 1e-6 {
        // Aspects match — translate-x / translate-y both fall to the
        // raster's default of `-vb.min_* * scale_*`. No correction
        // beyond what the natural mapping already does.
        return None;
    }
    // Spec steps 9–14: translate.
    let mut tx = -vb.min_x * sx;
    let mut ty = -vb.min_y * sy;
    let dx = width - vb.width * sx;
    let dy = height - vb.height * sy;
    let x_mid = matches!(
        par.align,
        PreserveAspectRatioAlign::XMidYMin
            | PreserveAspectRatioAlign::XMidYMid
            | PreserveAspectRatioAlign::XMidYMax
    );
    let x_max = matches!(
        par.align,
        PreserveAspectRatioAlign::XMaxYMin
            | PreserveAspectRatioAlign::XMaxYMid
            | PreserveAspectRatioAlign::XMaxYMax
    );
    let y_mid = matches!(
        par.align,
        PreserveAspectRatioAlign::XMinYMid
            | PreserveAspectRatioAlign::XMidYMid
            | PreserveAspectRatioAlign::XMaxYMid
    );
    let y_max = matches!(
        par.align,
        PreserveAspectRatioAlign::XMinYMax
            | PreserveAspectRatioAlign::XMidYMax
            | PreserveAspectRatioAlign::XMaxYMax
    );
    if x_mid {
        tx += dx / 2.0;
    } else if x_max {
        tx += dx;
    }
    if y_mid {
        ty += dy / 2.0;
    } else if y_max {
        ty += dy;
    }
    // spec_correct = translate(tx, ty) * scale(sx, sy) * translate(-vb.min_x, -vb.min_y)
    let spec = Transform2D::translate(tx, ty)
        .compose(&Transform2D::scale(sx, sy))
        .compose(&Transform2D::translate(-vb.min_x, -vb.min_y));
    // natural^{-1} = translate(vb.min_x, vb.min_y) * scale(vb.width/W, vb.height/H)
    let natural_inv = Transform2D::translate(vb.min_x, vb.min_y)
        .compose(&Transform2D::scale(vb.width / width, vb.height / height));
    // correction = natural^{-1} * spec
    let correction = natural_inv.compose(&spec);
    Some(correction)
}

fn register_all_defs(el: &Element, ctx: &mut ParseContext) -> Result<()> {
    // Round 3: capture *every* id-bearing element verbatim so
    // `<use href="#id">` can re-instantiate it later. Includes
    // shapes, groups, the special def kinds — anything addressable.
    if let Some(id) = attr(el, "id") {
        ctx.defs.elements.insert(id.to_string(), el.clone());
    }
    match tag_local(&el.name).as_str() {
        "lineargradient" => {
            // Round 81 — capture the typed def for §14.1.1 template
            // chain resolution; the legacy `Paint` flatten happens in
            // a second pass after the whole tree is walked so forward
            // `href` references resolve regardless of source order.
            if let Some((id, def)) = parse_linear_gradient_def(el)? {
                ctx.defs.gradients.insert(id, def);
            }
        }
        "radialgradient" => {
            if let Some((id, def)) = parse_radial_gradient_def(el)? {
                ctx.defs.gradients.insert(id, def);
            }
        }
        "filter" => {
            if let Some((id, def)) = parse_filter_def(el) {
                ctx.defs.filters.insert(id, def);
            }
        }
        "mask" => {
            if let Some((id, def)) = parse_mask_def(el, ctx)? {
                ctx.defs.masks.insert(id, def);
            }
        }
        "clippath" => {
            if let Some((id, def)) = parse_clip_path_def(el, ctx)? {
                ctx.defs.clip_paths.insert(id, def);
            }
        }
        "symbol" => {
            if let Some((id, def)) = parse_symbol_def(el, ctx)? {
                ctx.defs.symbols.insert(id, def);
            }
        }
        "pattern" => {
            // Round 20 — typed <pattern> capture (SVG 2 §14.3).
            if let Some((id, def)) = parse_pattern_def(el, ctx)? {
                ctx.defs.patterns.insert(id, def);
            }
        }
        "marker" => {
            // Round 104 — typed <marker> capture (SVG 2 §13.7.1). The
            // element is never-rendered on its own; capturing the typed
            // def lets a downstream rasterizer paint vertex markers once
            // a `Marker` construct lands in oxideav-core.
            if let Some((id, def)) = parse_marker_def(el, ctx)? {
                ctx.defs.markers.insert(id, def);
            }
        }
        "view" => {
            // Round 95 — typed <view> capture (SVG 2 §16.3.3). The
            // element itself doesn't render anything; capturing the
            // typed def lets [`crate::resolve_fragment`] honour a
            // `MyDrawing.svg#MyView` fragment identifier per §16.3.2.
            if let Some((id, def)) = parse_view_def(el) {
                ctx.defs.views.insert(id, def);
            }
        }
        _ => {}
    }
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            register_all_defs(c, ctx)?;
        }
    }
    Ok(())
}

/// Count element-only children of `parent` and per-tag totals — used
/// to pre-compute `:nth-child` / `:nth-of-type` denominators for each
/// child's [`MatchContext`].
fn count_element_children(parent: &Element) -> (usize, std::collections::HashMap<String, usize>) {
    let mut total = 0usize;
    let mut tag_totals: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for c in &parent.children {
        if let XmlNode::Element(e) = c {
            total += 1;
            let lower = tag_local(&e.name).to_ascii_lowercase();
            *tag_totals.entry(lower).or_insert(0) += 1;
        }
    }
    (total, tag_totals)
}

fn parse_view_box(s: &str) -> Result<ViewBox> {
    let nums: Result<Vec<f32>> = s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(|n| {
            n.parse::<f32>()
                .map_err(|_| Error::invalid("SVG: malformed viewBox number"))
        })
        .collect();
    let nums = nums?;
    if nums.len() != 4 {
        return Err(Error::invalid("SVG: viewBox must be 4 numbers"));
    }
    Ok(ViewBox {
        min_x: nums[0],
        min_y: nums[1],
        width: nums[2],
        height: nums[3],
    })
}

fn parse_length_or_default(v: Option<&str>, default: f32) -> Result<f32> {
    let s = match v {
        None => return Ok(default),
        Some(s) => s.trim(),
    };
    if s.is_empty() || s.ends_with('%') {
        return Ok(default);
    }
    parse_number(Some(s), default)
}

/// Codec-registry adapter. Consumes one packet (the entire SVG file)
/// and produces one [`Frame::Vector`].
pub fn make_decoder(_params: &CodecParameters) -> Result<Box<dyn Decoder>> {
    Ok(Box::new(SvgDecoder {
        codec_id: CodecId::new(CODEC_ID_STR),
        pending: None,
        eof: false,
    }))
}

struct SvgDecoder {
    codec_id: CodecId,
    pending: Option<VectorFrame>,
    eof: bool,
}

impl Decoder for SvgDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        let frame = parse_svg(&packet.data)?;
        self.pending = Some(frame);
        Ok(())
    }
    fn receive_frame(&mut self) -> Result<Frame> {
        match self.pending.take() {
            Some(f) => Ok(Frame::Vector(f)),
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

    #[test]
    fn parses_minimal_svg_with_rect() {
        let src = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50" viewBox="0 0 100 50">
  <rect x="10" y="10" width="80" height="30" fill="red"/>
</svg>"#;
        let frame = parse_svg(src).unwrap();
        assert_eq!(frame.width, 100.0);
        assert_eq!(frame.height, 50.0);
        assert!(frame.view_box.is_some());
        assert_eq!(frame.root.children.len(), 1);
    }

    #[test]
    fn parses_svg_without_explicit_dimensions_falls_back_to_viewbox() {
        let src = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"></svg>"#;
        let frame = parse_svg(src).unwrap();
        assert_eq!(frame.width, 64.0);
        assert_eq!(frame.height, 64.0);
    }

    #[test]
    fn rejects_non_svg_input() {
        let src = b"<html><body/></html>";
        assert!(parse_svg(src).is_err());
    }

    #[test]
    fn parses_gradient_def_and_resolves_url_reference() {
        let src = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <defs>
                <linearGradient id="g">
                    <stop offset="0" stop-color="#ff0000"/>
                    <stop offset="1" stop-color="#0000ff"/>
                </linearGradient>
            </defs>
            <rect x="0" y="0" width="10" height="10" fill="url(#g)"/>
        </svg>"##;
        let frame = parse_svg(src).unwrap();
        let path = match &frame.root.children[0] {
            oxideav_core::Node::Path(p) => p,
            _ => panic!("expected path"),
        };
        match &path.fill {
            Some(oxideav_core::Paint::LinearGradient(g)) => {
                assert_eq!(g.stops.len(), 2);
            }
            other => panic!("expected linear gradient, got {:?}", other),
        }
    }
}
