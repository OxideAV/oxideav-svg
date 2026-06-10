//! Round 261 — SVG 1.1 §16.8.2 `cursor` property integration tests.
//!
//! `cursor: [ [<funciri> ,]* [ auto | crosshair | default | pointer |
//! move | e-resize | ne-resize | nw-resize | n-resize | se-resize |
//! sw-resize | s-resize | w-resize | text | wait | help ] ] | inherit`
//! specifies the type of cursor displayed for the pointing device
//! while it hovers the element. Per the §16.8.2 attribute table the
//! initial value is `auto` and the property IS inherited (so a child
//! without its own `cursor=` picks up the parent's resolved value).
//! Zero or more comma-separated `<funciri>` custom-cursor references
//! precede a single mandatory generic keyword — §16.8.2: "If the user
//! agent cannot handle any user-defined cursor, it must use the
//! generic cursor at the end of the list", so a funciri list without
//! a trailing generic keyword is invalid.
//!
//! Round 261 ships parse + inherited cascade + round-trip
//! preservation. The actual cursor display (funciri resolution +
//! generic fallback walk per §16.8.2) is interactive-UA work (a
//! windowing host embedding `oxideav-pipeline`); this crate exposes
//! the resolved value on `PaintState::cursor` and round-trips the
//! source attribute via `PreservedExtras::cursors`. SVG 2 retains
//! `cursor` as a presentation attribute and defers the property
//! definition to CSS; the SVG 1.1 §16.8.2 definition carries the
//! keyword set and cascade rules exercised here.

use oxideav_svg::element::{CursorKeyword, PaintState};
use oxideav_svg::parser::{parse_xml, Element, Node as XmlNode};
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Default `cursor` is `auto` with no custom funciris per the §16.8.2
/// attribute table.
#[test]
fn default_cursor_is_auto() {
    let s = PaintState::default();
    assert_eq!(s.cursor.keyword, CursorKeyword::Auto);
    assert!(s.cursor.funciris.is_empty());
}

/// Round 261: a document without `cursor=` parses cleanly and the
/// decoder does NOT pollute the side-channel.
#[test]
fn baseline_no_cursor_attr_no_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.cursors.is_empty(),
        "round 261: a document without cursor= must not record a binding"
    );
}

/// Round 261: `cursor="wait"` on a `<g>` records a binding with the
/// canonicalised keyword.
#[test]
fn wait_on_g_records_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="wait">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.cursors.len(),
        1,
        "round 261: cursor= must record exactly one binding"
    );
    assert_eq!(extras.cursors[0].cursor, "wait");
}

/// Round 261: each of the sixteen §16.8.2 generic keywords parses +
/// records with the canonical (lowercase / hyphenated) spelling.
#[test]
fn each_keyword_records_canonical_form() {
    for kw in [
        "auto",
        "crosshair",
        "default",
        "pointer",
        "move",
        "e-resize",
        "ne-resize",
        "nw-resize",
        "n-resize",
        "se-resize",
        "sw-resize",
        "s-resize",
        "w-resize",
        "text",
        "wait",
        "help",
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <g cursor="{}">
                    <rect x="10" y="10" width="50" height="50" fill="red"/>
                </g>
            </svg>"#,
            kw
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.cursors.len(), 1, "input={}", kw);
        assert_eq!(extras.cursors[0].cursor, kw, "input={}", kw);
    }
}

/// Round 261: source keywords are matched case-insensitively per CSS.
/// The binding canonicalises to §16.8.2's all-lowercase spelling
/// (hyphens preserved on the eight `*-resize` keywords).
#[test]
fn keyword_matching_is_case_insensitive() {
    for (input, expected) in [
        ("POINTER", "pointer"),
        ("Crosshair", "crosshair"),
        ("E-RESIZE", "e-resize"),
        ("Ne-Resize", "ne-resize"),
        ("SE-RESIZE", "se-resize"),
        ("TEXT", "text"),
        ("Wait", "wait"),
        ("HELP", "help"),
        ("Move", "move"),
        ("DEFAULT", "default"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <g cursor="{}">
                    <rect x="10" y="10" width="50" height="50" fill="red"/>
                </g>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.cursors.len(), 1, "input={}", input);
        assert_eq!(extras.cursors[0].cursor, expected, "input={}", input);
    }
}

/// Round 261: explicit author `cursor="auto"` IS recorded even though
/// `auto` is the §16.8.2 initial value. Mirrors the round-221 ..
/// round-260 "explicit-initial-value carries intent" policy (e.g. an
/// inheritance reset on a descendant of a `<g cursor="wait">`).
#[test]
fn explicit_auto_is_recorded() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="auto">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.cursors.len(),
        1,
        "round 261: explicit `auto` carries author intent and IS recorded"
    );
    assert_eq!(extras.cursors[0].cursor, "auto");
}

/// Round 261: `cursor="inherit"` keeps the cascade's value and skips
/// recording (no canonical token to emit).
#[test]
fn inherit_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="inherit">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.cursors.is_empty(),
        "round 261: `inherit` keeps the resolved value and skips recording"
    );
}

/// Round 261: unrecognised generic keywords fall back to the cascade's
/// inherited value and skip recording (matches the tolerant policy of
/// the §13.x rendering-hint and §15.6 `pointer-events` branches). The
/// document still loads (no parse failure).
#[test]
fn unknown_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="grabbing">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.cursors.is_empty(),
        "round 261: unrecognised keyword keeps the resolved value and skips recording"
    );
    let _ = parse_svg(src).unwrap();
}

/// Round 261: empty `cursor=""` skips recording.
#[test]
fn empty_value_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(extras.cursors.is_empty());
}

/// Round 261: a single `<funciri>` followed by a generic keyword
/// records the canonical comma-and-space joined list per the §16.8.2
/// value grammar `[ [<funciri> ,]* [ <generic keyword> ] ]`.
#[test]
fn funciri_plus_keyword_records_canonical_list() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="url(#hot), pointer">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.cursors.len(), 1);
    assert_eq!(extras.cursors[0].cursor, "url(#hot), pointer");
}

/// Round 261: multiple funciris canonicalise whitespace to the
/// §16.8.2 example list shape (`url("mything.cur"), url("second.svg#curs"),
/// text`) — one space after each comma, `url` token lowercased, IRI
/// preserved verbatim.
#[test]
fn multiple_funciris_canonicalise_spacing() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="URL( mything.cur ) ,url(second.svg#curs)  ,  TEXT">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.cursors.len(), 1);
    assert_eq!(
        extras.cursors[0].cursor,
        "url(mything.cur), url(second.svg#curs), text"
    );
}

/// Round 261: a funciri whose IRI contains commas (e.g. a data: IRI)
/// stays one item — the list splits on top-level commas only, per the
/// §16.8.2 `<funciri>` production `url(` wsp* IRI wsp* `)`.
#[test]
fn funciri_with_internal_comma_stays_one_item() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="url(data:image/png;base64,AAAA), move">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.cursors.len(), 1);
    assert_eq!(
        extras.cursors[0].cursor,
        "url(data:image/png;base64,AAAA), move"
    );
}

/// Round 261: a funciri list WITHOUT the mandatory trailing generic
/// keyword is invalid per the §16.8.2 grammar ("it must use the
/// generic cursor at the end of the list") — the cascade keeps the
/// inherited value and no binding records.
#[test]
fn funciri_without_trailing_keyword_is_invalid() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="url(#hot)">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.cursors.is_empty(),
        "round 261: a funciri list without a generic fallback keyword is invalid"
    );
    let _ = parse_svg(src).unwrap();
}

/// Round 261: a non-funciri item before the generic keyword is invalid
/// (the §16.8.2 grammar admits only `<funciri>` items before the
/// keyword) — no binding records and the inherited value is kept.
#[test]
fn non_funciri_list_item_is_invalid() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="wait, pointer">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.cursors.is_empty(),
        "round 261: only <funciri> items may precede the generic keyword"
    );
}

/// Helper: extract a PaintState from a parsed XML fragment by finding
/// the named child of the SVG root.
fn shape_state(svg: &str, tag: &str) -> PaintState {
    let nodes = parse_xml(svg).expect("xml parse");
    let svg_el: &Element = nodes
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name.ends_with("svg") => Some(e),
            _ => None,
        })
        .expect("svg root");
    let shape_el: &Element = svg_el
        .children
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name == tag => Some(e),
            _ => None,
        })
        .expect("child element");
    let parent = PaintState::default();
    let sheet = oxideav_svg::css::Stylesheet::new();
    parent
        .merged_with_css(shape_el, &sheet)
        .expect("merged paint state")
}

/// Round 261: a presentation attribute resolves into PaintState
/// (presentation-attribute lane of the cascade).
#[test]
fn presentation_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect cursor="crosshair" x="0" y="0" width="10" height="10" fill="red"/>
            </svg>"#,
        "rect",
    );
    assert_eq!(s.cursor.keyword, CursorKeyword::Crosshair);
    assert!(s.cursor.funciris.is_empty());
}

/// Round 261: an inline `style="…"` declaration resolves into
/// PaintState — including the funciri list (style-attribute lane wins
/// over presentation attribute per the round-4 cascade order).
#[test]
fn style_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect style="cursor: url(#c), help" x="0" y="0" width="10" height="10" fill="red"/>
            </svg>"#,
        "rect",
    );
    assert_eq!(s.cursor.keyword, CursorKeyword::Help);
    assert_eq!(s.cursor.funciris, vec!["url(#c)".to_string()]);
}

/// Round 261: the property IS inherited per §16.8.2 — a child of a
/// `<g cursor="wait">` without its own `cursor=` resolves to `wait`
/// (the parent's value flows through the cascade since
/// `merged_with_css` does NOT reset `cursor` before applying the
/// element's own attribute).
#[test]
fn property_is_inherited_through_cascade() {
    let parent = PaintState {
        cursor: oxideav_svg::element::CursorValue {
            funciris: Vec::new(),
            keyword: CursorKeyword::Wait,
        },
        ..PaintState::default()
    };
    let nodes = parse_xml(
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="10" height="10"/></svg>"#,
    )
    .expect("xml parse");
    let svg_el: &Element = nodes
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name.ends_with("svg") => Some(e),
            _ => None,
        })
        .expect("svg root");
    let rect_el: &Element = svg_el
        .children
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name == "rect" => Some(e),
            _ => None,
        })
        .expect("rect");
    let sheet = oxideav_svg::css::Stylesheet::new();
    let child = parent.merged_with_css(rect_el, &sheet).unwrap();
    assert_eq!(
        child.cursor.keyword,
        CursorKeyword::Wait,
        "round 261: cursor IS inherited per §16.8.2 — the child must pick up the parent's value \
         when it has no attribute of its own"
    );
}

/// Round 261: a child with its own `cursor=` overrides the inherited
/// value.
#[test]
fn child_attribute_overrides_inherited_value() {
    let parent = PaintState {
        cursor: oxideav_svg::element::CursorValue {
            funciris: Vec::new(),
            keyword: CursorKeyword::Wait,
        },
        ..PaintState::default()
    };
    let nodes = parse_xml(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <rect cursor="text" x="0" y="0" width="10" height="10"/>
            </svg>"#,
    )
    .expect("xml parse");
    let svg_el: &Element = nodes
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name.ends_with("svg") => Some(e),
            _ => None,
        })
        .expect("svg root");
    let rect_el: &Element = svg_el
        .children
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name == "rect" => Some(e),
            _ => None,
        })
        .expect("rect");
    let sheet = oxideav_svg::css::Stylesheet::new();
    let child = parent.merged_with_css(rect_el, &sheet).unwrap();
    assert_eq!(child.cursor.keyword, CursorKeyword::Text);
}

/// Round 261: a `<style>`-block rule resolves through the round-4
/// cascade onto the carried PaintState and records a binding from the
/// presentation-attribute carrier only (the block rule itself has no
/// source-attribute emit slot — same policy as the round-260
/// `pointer-events` CSS smoke-test).
#[test]
fn style_block_rule_resolves_in_cascade() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <style>rect { cursor: move }</style>
        <rect x="10" y="10" width="50" height="50" fill="red"/>
    </svg>"#;
    // The document loads and the side-channel stays clean (no
    // presentation attribute was written in the source).
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert!(
        extras.cursors.is_empty(),
        "round 261: a <style>-block rule has no source-attribute slot to record"
    );
}

/// Round 261: round-trip preserves `cursor=` on a `<g>` — a
/// `parse_svg_with_extras → write_svg_with_extras` cycle re-emits the
/// attribute on the matching element.
#[test]
fn roundtrip_emits_attribute_on_group() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="wait">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("cursor=\"wait\""),
        "round-trip output should re-emit cursor: {}",
        out_s
    );
}

/// Round 261: round-trip preserves `cursor=` on a shape — a shape
/// carrying the attribute directly (not via a group) also round-trips
/// on the matching `<path>` emit slot, including the funciri list.
#[test]
fn roundtrip_emits_attribute_on_shape() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <rect x="10" y="10" width="50" height="50" fill="red" cursor="url(#hot), pointer"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.cursors.len(), 1);
    assert_eq!(extras.cursors[0].cursor, "url(#hot), pointer");
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("cursor=\"url(#hot), pointer\""),
        "round-trip should re-emit cursor on the shape: {}",
        out_s
    );
}

/// Round 261: double round-trip converges — a second parse-then-write
/// of the output produces identical `cursor=` content.
#[test]
fn roundtrip_is_idempotent() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="url(second.svg#curs), se-resize">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame1, extras1) = parse_svg_with_extras(src).unwrap();
    let out1 = write_svg_with_extras(&frame1, &extras1);
    let (frame2, extras2) = parse_svg_with_extras(&out1).unwrap();
    let out2 = write_svg_with_extras(&frame2, &extras2);
    assert_eq!(
        out1, out2,
        "round 261: parse → write → parse → write must converge"
    );
    let s2 = String::from_utf8(out2).unwrap();
    assert!(s2.contains("cursor=\"url(second.svg#curs), se-resize\""));
}

/// Round 261: source-case canonicalises through round-trip — uppercase
/// keyword input emits as the §16.8.2 lowercase spelling.
#[test]
fn roundtrip_canonicalises_source_case() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="NW-RESIZE">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("cursor=\"nw-resize\""),
        "round-trip should canonicalise NW-RESIZE to nw-resize: {}",
        out_s
    );
}

/// Round 261: `parse_svg` (no extras) still loads a document carrying
/// `cursor=` — the side-channel is an opt-in of the `_with_extras`
/// entry point only.
#[test]
fn parse_svg_without_extras_still_loads() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="pointer">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let frame = parse_svg(src).unwrap();
    assert_eq!(frame.root.children.len(), 1);
}

/// Round 261: a `<g cursor=…>` ancestor records on the group's own
/// slot — ONE binding for the group, not one per cascaded descendant.
#[test]
fn group_records_once_not_per_child() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="move">
            <rect x="10" y="10" width="20" height="20" fill="red"/>
            <rect x="40" y="10" width="20" height="20" fill="blue"/>
            <rect x="70" y="10" width="20" height="20" fill="green"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.cursors.len(),
        1,
        "round 261: the group carrier records once, not per cascaded child"
    );
    assert_eq!(extras.cursors[0].cursor, "move");
}

/// Round 261: a per-child override records separately from the group
/// carrier — two bindings, each at its own emit slot.
#[test]
fn per_child_override_records_separately() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="wait">
            <rect x="10" y="10" width="20" height="20" fill="red"/>
            <rect cursor="auto" x="40" y="10" width="20" height="20" fill="blue"/>
        </g>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(
        extras.cursors.len(),
        2,
        "round 261: the group carrier and the child override each record once"
    );
    let values: Vec<&str> = extras.cursors.iter().map(|b| b.cursor.as_str()).collect();
    assert!(values.contains(&"wait"));
    assert!(values.contains(&"auto"));
}

/// Round 261: §16.8.2 `cursor` coexists with the §15.6
/// `pointer-events` and §3.11 `overflow` carriers on the same `<g>` —
/// orthogonal properties, independent side-channels, every recognised
/// attribute re-emits on round-trip.
#[test]
fn coexists_with_pointer_events_and_overflow() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g cursor="pointer" pointer-events="all" overflow="hidden">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.cursors.len(), 1);
    assert_eq!(extras.pointer_eventss.len(), 1);
    assert_eq!(extras.overflows.len(), 1);
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(out_s.contains("cursor=\"pointer\""));
    assert!(out_s.contains("pointer-events=\"all\""));
    assert!(out_s.contains("overflow=\"hidden\""));
}
