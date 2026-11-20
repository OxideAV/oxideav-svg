//! SVG 2 chapter 11 layout: white space, position lists, spacing,
//! baselines, vertical writing, bidirectional text, decorations, font
//! fallback and `<textPath>`.
//!
//! Own integration-test binary: the font hook is process-global. The
//! resolver here only knows the family `Fixture Mono` (and the generic
//! `sans-serif`), so family fallback is observable.

#![cfg(feature = "text")]

use std::sync::atomic::{AtomicUsize, Ordering};

use oxideav_scribe::{Face, FaceChain};
use oxideav_svg::{Group, Node, Paint, SvgDocument, Transform2D, parse, text::set_font_resolver};

const FONT: &[u8] = include_bytes!("fixtures/DejaVuSansMono.ttf");

static RESOLVER_CALLS: AtomicUsize = AtomicUsize::new(0);

fn install() {
    let _ = set_font_resolver(|family, _size| {
        // Only the family one test reserves is counted, so tests
        // running in parallel do not disturb the count.
        if family == "Counted Mono" {
            RESOLVER_CALLS.fetch_add(1, Ordering::SeqCst);
        }
        if family == "Fixture Mono" || family == "Counted Mono" || family == "sans-serif" {
            Face::from_ttf_bytes(FONT.to_vec()).ok().map(FaceChain::new)
        } else {
            None
        }
    });
}

fn doc(body: &str) -> SvgDocument {
    install();
    let src = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="600" height="400" font-family="Fixture Mono" font-size="16">{body}</svg>"##
    );
    parse(src.as_bytes()).expect("parse")
}

fn text_group(d: &SvgDocument) -> &Group {
    match d.root.children.last().expect("text node") {
        Node::Group(g) => g,
        other => panic!("expected the text group, got {other:?}"),
    }
}

/// The placement transform of every plainly painted glyph, in order.
fn glyphs(d: &SvgDocument) -> Vec<Transform2D> {
    text_group(d)
        .children
        .iter()
        .filter_map(|c| match c {
            Node::Group(g) if g.children.len() == 1 => Some(g.transform),
            _ => None,
        })
        .collect()
}

fn xs(d: &SvgDocument) -> Vec<f32> {
    glyphs(d).iter().map(|t| t.e).collect()
}

fn ys(d: &SvgDocument) -> Vec<f32> {
    glyphs(d).iter().map(|t| t.f).collect()
}

/// Fill colour of every plainly painted glyph.
fn colours(d: &SvgDocument) -> Vec<(u8, u8, u8)> {
    text_group(d)
        .children
        .iter()
        .filter_map(|c| match c {
            Node::Group(g) => match g.children.first() {
                Some(Node::Path(p)) => match &p.fill {
                    Some(Paint::Solid(c)) => Some((c.r, c.g, c.b)),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn adv() -> f32 {
    let d = doc(r#"<text x="0" y="50">AB</text>"#);
    let x = xs(&d);
    x[1] - x[0]
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.05
}

// ───────────── white space (§11.10.3) ─────────────

#[test]
fn xml_space_default_strips_newlines_trims_and_collapses() {
    let a = adv();
    let d = doc("<text x=\"10\" y=\"50\">\n   A  \t B\n  C   </text>");
    // "A B C": newlines removed, runs of spaces collapsed, ends trimmed.
    let x = xs(&d);
    assert_eq!(x.len(), 3);
    assert!(
        close(x[0], 10.0) && close(x[1], 10.0 + 2.0 * a) && close(x[2], 10.0 + 4.0 * a),
        "{x:?}"
    );
}

#[test]
fn xml_space_preserve_keeps_every_space() {
    let a = adv();
    let d = doc("<text x=\"10\" y=\"50\" xml:space=\"preserve\">  A   B</text>");
    let x = xs(&d);
    assert!(
        close(x[0], 10.0 + 2.0 * a) && close(x[1], 10.0 + 6.0 * a),
        "{x:?}"
    );
}

#[test]
fn collapsing_spans_element_boundaries() {
    let a = adv();
    let d = doc("<text x=\"10\" y=\"50\">A <tspan> B </tspan> C</text>");
    // One space between each pair, wherever the markup put them.
    let x = xs(&d);
    assert!(
        close(x[1] - x[0], 2.0 * a) && close(x[2] - x[1], 2.0 * a),
        "{x:?}"
    );
}

#[test]
fn position_lists_index_addressable_characters() {
    // §11.5 example: collapsed white space is not addressable, so the
    // third `x` lands on "B" (A, one space, B).
    let d = doc("<text x=\"100 200 300\" y=\"50\">\n  A\n  B\n  </text>");
    let x = xs(&d);
    assert!(close(x[0], 100.0) && close(x[1], 300.0), "{x:?}");
}

#[test]
fn white_space_pre_breaks_lines() {
    let d = doc(
        "<text x=\"10\" y=\"50\" style=\"white-space: pre; line-height: 20px\">A\n B\n\nC</text>",
    );
    let (x, y) = (xs(&d), ys(&d));
    assert_eq!(x.len(), 3);
    // Line 2 keeps its leading space; the blank line still takes a row.
    assert!(
        close(x[0], 10.0) && close(x[1], 10.0 + adv()) && close(x[2], 10.0),
        "{x:?}"
    );
    assert!(
        close(y[0], 50.0) && close(y[1], 70.0) && close(y[2], 110.0),
        "{y:?}"
    );
}

#[test]
fn white_space_pre_line_collapses_but_keeps_breaks() {
    let d = doc(
        "<text x=\"10\" y=\"50\" style=\"white-space: pre-line; line-height: 2\">A   B  \n   C</text>",
    );
    let (x, y) = (xs(&d), ys(&d));
    assert!(
        close(x[1], 10.0 + 2.0 * adv()) && close(x[2], 10.0),
        "{x:?}"
    );
    assert!(close(y[2] - y[0], 32.0), "{y:?}");
}

#[test]
fn each_line_of_preformatted_text_is_anchored_on_its_own() {
    let a = adv();
    let d = doc(
        "<text x=\"300\" y=\"50\" text-anchor=\"middle\" style=\"white-space: pre\">AB\nABCD</text>",
    );
    let x = xs(&d);
    assert!(
        close(x[0], 300.0 - a) && close(x[2], 300.0 - 2.0 * a),
        "{x:?}"
    );
}

#[test]
fn tab_advances_to_the_next_tab_stop() {
    let a = adv();
    let d = doc("<text x=\"0\" y=\"50\" style=\"white-space: pre\">AB\tC</text>");
    let x = xs(&d);
    assert!(close(x[2], 8.0 * a), "{x:?}");
}

#[test]
fn display_none_span_is_neither_rendered_nor_addressable() {
    let d = doc(r#"<text x="10 110 210" y="50">A<tspan display="none">XYZ</tspan>BC</text>"#);
    let x = xs(&d);
    assert_eq!(x.len(), 3);
    assert!(close(x[1], 110.0) && close(x[2], 210.0), "{x:?}");
}

// ───────────── positioning (§11.2.2, §11.5) ─────────────

#[test]
fn relative_shifts_accumulate_and_add_to_absolute_positions() {
    let a = adv();
    let d = doc(r#"<text x="10" y="50" dx="5 5 5" dy="0 -3 -3">ABCD</text>"#);
    let (x, y) = (xs(&d), ys(&d));
    assert!(close(x[0], 15.0) && close(x[1], 20.0 + a) && close(x[2], 25.0 + 2.0 * a));
    assert!(close(x[3], 25.0 + 3.0 * a), "{x:?}");
    assert!(
        close(y[0], 50.0) && close(y[1], 47.0) && close(y[2], 44.0) && close(y[3], 44.0),
        "{y:?}"
    );
}

#[test]
fn descendant_lists_override_ancestor_lists() {
    let d = doc(r#"<text x="10 20 30 40" y="50">A<tspan x="200">B</tspan>CD</text>"#);
    let x = xs(&d);
    assert!(
        close(x[0], 10.0) && close(x[1], 200.0) && close(x[2], 30.0) && close(x[3], 40.0),
        "{x:?}"
    );
}

#[test]
fn last_rotate_value_applies_to_the_rest_of_the_element() {
    let d = doc(r#"<text x="10" y="50" rotate="0 90">ABC<tspan rotate="0">D</tspan></text>"#);
    let g = glyphs(&d);
    let turned = |t: &Transform2D| (t.b - 1.0).abs() < 1e-4 && t.a.abs() < 1e-4;
    assert!(
        !turned(&g[0]) && turned(&g[1]) && turned(&g[2]) && !turned(&g[3]),
        "{g:?}"
    );
}

#[test]
fn units_resolve_in_position_lists() {
    let d = doc(r#"<text x="50%" y="2em" dx="1em">A</text>"#);
    let g = glyphs(&d);
    assert!(close(g[0].e, 300.0 + 16.0) && close(g[0].f, 32.0), "{g:?}");
}

#[test]
fn text_length_on_a_tspan_stretches_it_and_pushes_what_follows() {
    let a = adv();
    let d = doc(r#"<text x="10" y="50">A<tspan textLength="100">BCD</tspan>E</text>"#);
    let x = xs(&d);
    // B..D spread so their advance sum is 100; E follows right after.
    let step = (100.0 - 3.0 * a) / 2.0 + a;
    assert!(
        close(x[1], 10.0 + a) && close(x[2], 10.0 + a + step) && close(x[3], 10.0 + a + 2.0 * step)
    );
    assert!(close(x[4], 10.0 + a + 100.0), "{x:?}");
}

#[test]
fn text_length_accepts_units_and_anchors_on_the_adjusted_width() {
    let d = doc(r#"<text x="300" y="50" textLength="50%" text-anchor="end">ABCD</text>"#);
    let x = xs(&d);
    assert!(close(x[0], 0.0), "{x:?}");
}

#[test]
fn letter_and_word_spacing_widen_advances() {
    let a = adv();
    let d = doc(r#"<text x="0" y="50" letter-spacing="2" word-spacing="10">AB CD</text>"#);
    let x = xs(&d);
    // Every character gains 2; the space gains 10 more.
    assert!(
        close(x[1], a + 2.0)
            && close(x[2], 3.0 * (a + 2.0) + 10.0)
            && close(x[3], 4.0 * (a + 2.0) + 10.0),
        "{x:?}"
    );
    // `em` resolves against the element's own font size.
    let d = doc(r#"<text x="0" y="50" font-size="20" letter-spacing="0.5em">AB</text>"#);
    let x = xs(&d);
    assert!(close(x[1], a * 20.0 / 16.0 + 10.0), "{x:?}");
}

#[test]
fn spacing_counts_toward_anchoring() {
    let a = adv();
    let d = doc(r#"<text x="300" y="50" letter-spacing="4" text-anchor="end">AB</text>"#);
    let x = xs(&d);
    assert!(close(x[0], 300.0 - 2.0 * (a + 4.0)), "{x:?}");
}

// ───────────── fonts ─────────────

#[test]
fn font_size_cascades_through_groups_css_and_the_font_shorthand() {
    let a = adv();
    let d = doc(r#"<g font-size="32"><text x="0" y="50">AB</text></g>"#);
    let Node::Group(text) = &text_group(&d).children[0] else {
        panic!("the <g> holds the text group");
    };
    let Node::Group(second) = &text.children[1] else {
        panic!("glyph placement");
    };
    assert!(close(second.transform.e, 2.0 * a));
    let d = doc(
        r#"<style>.big { font-size: 200% }</style><text x="0" y="50"><tspan class="big">AB</tspan></text>"#,
    );
    assert!(close(xs(&d)[1], 2.0 * a));
    let d = doc(r#"<text x="0" y="50" style="font: italic 8px 'Fixture Mono'">AB</text>"#);
    assert!(close(xs(&d)[1], 0.5 * a));
    let d = doc(r#"<text x="0" y="50" font-size="x-large">AB</text>"#);
    assert!(close(xs(&d)[1], 1.5 * a));
}

#[test]
fn missing_family_falls_back_down_the_list_then_to_sans_serif() {
    // The resolver declines the whole list and "No Such Font", then
    // answers for the second family.
    let d = doc(r#"<text x="0" y="50" font-family="No Such Font, 'Fixture Mono'">AB</text>"#);
    assert_eq!(xs(&d).len(), 2);
    // Nothing in the list resolves: the generic sans-serif does.
    let d = doc(r#"<text x="0" y="50" font-family="Nope, Neither">AB</text>"#);
    assert_eq!(xs(&d).len(), 2);
}

#[test]
fn a_font_is_resolved_once_per_document() {
    let mut body = String::from("<g font-family=\"Counted Mono\">");
    for i in 0..50 {
        body.push_str(&format!(
            "<text x=\"0\" y=\"{}\">row <tspan fill=\"red\">{i}</tspan> text</text>",
            10 * i
        ));
    }
    body.push_str("</g>");
    let d = doc(&body);
    let Node::Group(g) = &d.root.children[0] else {
        panic!("the wrapping group");
    };
    assert_eq!(g.children.len(), 50);
    // 150 spans, one font, one size: one resolver call.
    assert_eq!(RESOLVER_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn synthetic_small_caps_shrink_lower_case_to_capitals() {
    let a = adv();
    let d = doc(r#"<text x="0" y="50" font-variant="small-caps">Ab</text>"#);
    let g = text_group(&d);
    // "b" becomes a reduced "B": the next glyph origin is 0.8 advances on.
    let d2 = doc(r#"<text x="0" y="50" font-variant="small-caps">Abb</text>"#);
    let x = xs(&d2);
    assert!(close(x[2] - x[1], 0.8 * a), "{x:?}");
    assert_eq!(g.children.len(), 2);
}

// ───────────── baselines (§11.10.2.5–7) ─────────────

#[test]
fn dominant_baseline_moves_the_glyphs_under_the_text_position() {
    let y_of = |attr: &str| ys(&doc(&format!(r#"<text x="0" y="100" {attr}>A</text>"#)))[0];
    let alphabetic = y_of("");
    assert!(close(alphabetic, 100.0));
    // Baselines above the alphabetic one push the glyph down…
    let middle = y_of(r#"dominant-baseline="middle""#);
    let central = y_of(r#"dominant-baseline="central""#);
    let hanging = y_of(r#"dominant-baseline="hanging""#);
    let top = y_of(r#"dominant-baseline="text-before-edge""#);
    assert!(
        100.0 < middle && middle < central + 2.0 && central < hanging && hanging < top,
        "{middle} {central} {hanging} {top}"
    );
    assert!(top - 100.0 > 10.0 && top - 100.0 < 20.0);
    // …and the ones below pull it up.
    let bottom = y_of(r#"dominant-baseline="text-after-edge""#);
    let ideographic = y_of(r#"dominant-baseline="ideographic""#);
    assert!(bottom < 100.0 && close(bottom, ideographic));
}

#[test]
fn baseline_is_inherited_by_spans_and_overridden_by_alignment_baseline() {
    let d = doc(
        r#"<text x="0" y="100" dominant-baseline="hanging">A<tspan>B</tspan><tspan alignment-baseline="alphabetic">C</tspan></text>"#,
    );
    let y = ys(&d);
    assert!(
        y[0] > 105.0 && close(y[0], y[1]) && close(y[2], 100.0),
        "{y:?}"
    );
}

#[test]
fn baseline_shift_raises_lowers_and_accumulates() {
    let d = doc(
        r#"<text x="0" y="100">A<tspan baseline-shift="super">B<tspan baseline-shift="super">C</tspan></tspan><tspan baseline-shift="sub">D</tspan><tspan baseline-shift="5">E</tspan><tspan baseline-shift="-50%">F</tspan></text>"#,
    );
    let y = ys(&d);
    assert!(close(y[0], 100.0));
    assert!(
        close(y[1], 100.0 - 16.0 / 3.0) && close(y[2], 100.0 - 32.0 / 3.0),
        "{y:?}"
    );
    assert!(
        close(y[3], 100.0 + 3.2) && close(y[4], 95.0) && close(y[5], 108.0),
        "{y:?}"
    );
}

// ───────────── vertical writing (§11.10.2.3) ─────────────

#[test]
fn vertical_text_runs_down_with_latin_set_sideways() {
    let a = adv();
    let d = doc(r#"<text x="100" y="20" writing-mode="vertical-rl">AB</text>"#);
    let g = glyphs(&d);
    // Sideways: a quarter turn clockwise, advancing down by the
    // horizontal advance.
    assert!(
        g.iter()
            .all(|t| (t.b - 1.0).abs() < 1e-4 && (t.c + 1.0).abs() < 1e-4),
        "{g:?}"
    );
    assert!(close(g[0].f, 20.0) && close(g[1].f, 20.0 + a), "{g:?}");
    // The legacy keyword means the same.
    let d = doc(r#"<text x="100" y="20" writing-mode="tb">AB</text>"#);
    assert!(close(glyphs(&d)[1].f, 20.0 + a));
}

#[test]
fn upright_orientation_stacks_em_boxes() {
    let a = adv();
    let d = doc(
        r#"<text x="100" y="20" writing-mode="vertical-rl" text-orientation="upright">AB</text>"#,
    );
    let g = glyphs(&d);
    // Upright glyphs, one em apart, centred on the line.
    assert!(
        g.iter()
            .all(|t| (t.a - 1.0).abs() < 1e-4 && t.b.abs() < 1e-4),
        "{g:?}"
    );
    assert!(
        close(g[1].f - g[0].f, 16.0) && close(g[0].e, 100.0 - a / 2.0),
        "{g:?}"
    );
    assert!(
        g[0].f > 20.0 && g[0].f < 36.0,
        "the em box hangs from y: {g:?}"
    );
}

#[test]
fn vertical_anchor_and_absolute_positions_use_the_y_axis() {
    let a = adv();
    let d = doc(
        r#"<text x="100" y="200" writing-mode="vertical-rl" text-anchor="end">AB<tspan y="300">C</tspan></text>"#,
    );
    let g = glyphs(&d);
    assert!(
        close(g[0].f, 200.0 - 2.0 * a) && close(g[2].f, 300.0 - a),
        "{g:?}"
    );
}

#[test]
fn vertical_lines_stack_right_to_left() {
    let d = doc(
        "<text x=\"100\" y=\"20\" style=\"writing-mode: vertical-rl; white-space: pre; line-height: 24px\">A\nB</text>",
    );
    let g = glyphs(&d);
    assert!(
        close(g[0].e - g[1].e, 24.0) && close(g[0].f, g[1].f),
        "{g:?}"
    );
    let d = doc(
        "<text x=\"100\" y=\"20\" style=\"writing-mode: vertical-lr; white-space: pre; line-height: 24px\">A\nB</text>",
    );
    let g = glyphs(&d);
    assert!(close(g[1].e - g[0].e, 24.0), "{g:?}");
}

// ───────────── bidirectional text (§11.10.2.4) ─────────────

#[test]
fn right_to_left_run_is_laid_out_in_visual_order() {
    let a = adv();
    // Latin, two Hebrew letters (red, blue in logical order), Latin.
    let d = doc(
        "<text x=\"0\" y=\"50\">A<tspan fill=\"red\">\u{05D0}</tspan><tspan fill=\"blue\">\u{05D1}</tspan>B</text>",
    );
    let (x, c) = (xs(&d), colours(&d));
    assert_eq!(c[1], (255, 0, 0));
    // The first Hebrew letter sits to the RIGHT of the second.
    assert!(
        close(x[1], 2.0 * a) && close(x[2], a) && close(x[3], 3.0 * a),
        "{x:?}"
    );
}

#[test]
fn rtl_direction_anchors_at_the_right_edge() {
    let a = adv();
    // direction=rtl, anchor start: the text ENDS at x (§11.10.1.1).
    let d = doc(r#"<text x="300" y="50" direction="rtl">AB</text>"#);
    let x = xs(&d);
    assert!(
        close(x[0], 300.0 - 2.0 * a) && close(x[1], 300.0 - a),
        "{x:?}"
    );
    // …and `end` starts there.
    let d = doc(r#"<text x="300" y="50" direction="rtl" text-anchor="end">AB</text>"#);
    assert!(close(xs(&d)[0], 300.0));
}

#[test]
fn bidi_override_reverses_latin() {
    let a = adv();
    let d = doc(
        r#"<text x="0" y="50"><tspan fill="red">A</tspan><tspan direction="rtl" unicode-bidi="bidi-override">BC</tspan></text>"#,
    );
    let x = xs(&d);
    // B is drawn after C.
    assert!(
        close(x[0], 0.0) && close(x[1], 2.0 * a) && close(x[2], a),
        "{x:?}"
    );
}

// ───────────── decorations (§11.11) ─────────────

/// (bounding box, solid fill colour, index among the text's children)
/// of one decoration geometry.
type Bar = (oxideav_svg::bbox::BBox, Option<(u8, u8, u8)>, usize);

fn decoration_boxes(d: &SvgDocument) -> Vec<Bar> {
    text_group(d)
        .children
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match c {
            Node::Path(p) => Some((
                oxideav_svg::bbox::BBox::of_path(&p.path)?,
                match &p.fill {
                    Some(Paint::Solid(c)) => Some((c.r, c.g, c.b)),
                    _ => None,
                },
                i,
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn underline_spans_the_element_in_the_declaring_paint() {
    let a = adv();
    let d = doc(
        r#"<text x="10" y="50" fill="green" text-decoration="underline">AB<tspan fill="red">CD</tspan></text>"#,
    );
    let bars = decoration_boxes(&d);
    assert_eq!(bars.len(), 1, "one merged bar: {bars:?}");
    let (b, colour, index) = &bars[0];
    // Under the baseline, across all four glyphs, in the text's green —
    // not the child's red — and beneath the glyphs.
    assert!(
        close(b.x, 10.0) && close(b.max_x(), 10.0 + 4.0 * a),
        "{b:?}"
    );
    assert!(b.y > 50.0 && b.max_y() < 55.0, "{b:?}");
    assert_eq!(*colour, Some((0, 128, 0)));
    assert_eq!(*index, 0);
}

#[test]
fn line_through_paints_over_the_glyphs_and_overline_above() {
    let d = doc(r#"<text x="10" y="50" text-decoration="overline line-through">AB</text>"#);
    let bars = decoration_boxes(&d);
    assert_eq!(bars.len(), 2);
    let over = bars.iter().find(|b| b.2 == 0).expect("overline first");
    let through = bars.iter().find(|b| b.2 == 3).expect("line-through last");
    assert!(
        over.0.max_y() < 40.0 && through.0.y > 40.0 && through.0.max_y() < 50.0,
        "{bars:?}"
    );
}

#[test]
fn nested_decorations_and_decoration_colour() {
    let a = adv();
    let d = doc(
        r#"<text x="10" y="50">AB<tspan style="text-decoration: underline double blue">CD</tspan></text>"#,
    );
    let bars = decoration_boxes(&d);
    assert_eq!(bars.len(), 1);
    assert_eq!(bars[0].1, Some((0, 0, 255)));
    assert!(close(bars[0].0.x, 10.0 + 2.0 * a) && close(bars[0].0.width, 2.0 * a));
    // `double`: two closed bars.
    let Node::Path(p) = &text_group(&d).children[0] else {
        panic!()
    };
    let closes = p
        .path
        .commands
        .iter()
        .filter(|c| matches!(c, oxideav_svg::PathCommand::Close))
        .count();
    assert_eq!(closes, 2);
}

#[test]
fn decoration_keeps_the_declaring_baseline_under_shifted_children() {
    let d = doc(
        r#"<text x="10" y="50" text-decoration="underline">A<tspan baseline-shift="super">B</tspan></text>"#,
    );
    let bars = decoration_boxes(&d);
    assert_eq!(
        bars.len(),
        1,
        "the raised child keeps the parent's line: {bars:?}"
    );
}

// ───────────── text on a path (§11.8) ─────────────

const LINE: &str = r#"<path id="line" d="M 100 200 L 500 200" pathLength="200"/>"#;

#[test]
fn start_offset_number_percentage_and_path_length_scaling() {
    let d = doc(&format!(
        r##"{LINE}<text><textPath href="#line">AB</textPath></text>"##
    ));
    assert!(close(xs(&d)[0], 100.0) && close(ys(&d)[0], 200.0));
    let d = doc(&format!(
        r##"{LINE}<text><textPath href="#line" startOffset="25%">AB</textPath></text>"##
    ));
    assert!(close(xs(&d)[0], 200.0));
    // A length is in the path's own `pathLength` scale: 50 of 200 = 25 %.
    let d = doc(&format!(
        r##"{LINE}<text><textPath href="#line" startOffset="50">AB</textPath></text>"##
    ));
    assert!(close(xs(&d)[0], 200.0));
}

#[test]
fn glyphs_whose_midpoint_leaves_the_path_are_dropped() {
    let d = doc(r#"<text><textPath path="M 0 100 L 25 100">ABCDE</textPath></text>"#);
    // 25 units hold the midpoints of the first three 9.6-unit glyphs.
    assert_eq!(xs(&d).len(), 3);
    let d =
        doc(r#"<text><textPath path="M 0 100 L 25 100" startOffset="-12">ABCDE</textPath></text>"#);
    assert_eq!(xs(&d).len(), 3, "A is off the start, E off the end");
}

#[test]
fn path_text_follows_the_tangent_and_anchors_about_the_offset() {
    let a = adv();
    let d = doc(
        r#"<text text-anchor="middle"><textPath path="M 100 0 L 100 400" startOffset="50%">AB</textPath></text>"#,
    );
    let g = glyphs(&d);
    // Heading straight down: rotated a quarter turn, centred on y=200.
    assert!((g[0].b - 1.0).abs() < 1e-4 && close(g[0].e, 100.0), "{g:?}");
    assert!(close(g[0].f, 200.0 - a) && close(g[1].f, 200.0), "{g:?}");
}

#[test]
fn side_right_runs_the_path_backwards() {
    let d = doc(r#"<text><textPath path="M 100 200 L 500 200" side="right">AB</textPath></text>"#);
    let g = glyphs(&d);
    // From the far end, upside down relative to the original direction.
    assert!(
        close(g[0].e, 500.0) && (g[0].a + 1.0).abs() < 1e-4 && g[1].e < g[0].e,
        "{g:?}"
    );
}

#[test]
fn dy_offsets_along_the_normal_and_x_along_the_path() {
    let d = doc(
        r#"<text><textPath path="M 100 200 L 500 200">A<tspan dy="-10" x="50">B</tspan></textPath></text>"#,
    );
    let g = glyphs(&d);
    assert!(close(g[1].e, 150.0) && close(g[1].f, 190.0), "{g:?}");
}

#[test]
fn text_after_a_path_continues_from_its_end_point() {
    let a = adv();
    let d = doc(r#"<text><textPath path="M 100 200 L 300 250">AB</textPath>CD</text>"#);
    let g = glyphs(&d);
    assert!(
        close(g[2].e, 300.0) && close(g[2].f, 250.0) && close(g[3].e, 300.0 + a),
        "{g:?}"
    );
    // Unrotated again.
    assert!((g[2].a - 1.0).abs() < 1e-4 && g[2].b.abs() < 1e-4);
}

#[test]
fn closed_path_wraps_once_around() {
    // A square of perimeter 400 starting at its top-left corner.
    let sq = r#"<path id="sq" d="M 100 100 h 100 v 100 h -100 z"/>"#;
    // Starting 30 before the end: the text wraps past the start point.
    let d = doc(&format!(
        r##"{sq}<text><textPath href="#sq" startOffset="370">ABCDEF</textPath></text>"##
    ));
    let g = glyphs(&d);
    assert_eq!(g.len(), 6, "nothing is cut at the seam of a closed path");
    // The first glyph is on the left edge heading up, the last on the
    // top edge heading right.
    assert!(close(g[0].e, 100.0) && g[0].b < -0.99, "{:?}", g[0]);
    assert!(
        close(g[5].f, 100.0) && (g[5].a - 1.0).abs() < 1e-4,
        "{:?}",
        g[5]
    );
    // An open path of the same shape cuts them off instead.
    let open = r#"<path id="op" d="M 100 100 h 100 v 100 h -100 v -100"/>"#;
    let d = doc(&format!(
        r##"{open}<text><textPath href="#op" startOffset="370">ABCDEF</textPath></text>"##
    ));
    assert_eq!(glyphs(&d).len(), 3);
}

#[test]
fn basic_shapes_and_the_target_transform_define_the_path() {
    let d = doc(
        r##"<circle id="c" cx="200" cy="200" r="100"/><text><textPath href="#c">A</textPath></text>"##,
    );
    let g = glyphs(&d);
    // The circle's equivalent path starts at its rightmost point.
    assert!(
        (g[0].e - 300.0).abs() < 1.0 && (g[0].f - 200.0).abs() < 6.0,
        "{g:?}"
    );
    // §11.8.2: the target's own transform scales the path, not the text.
    let d = doc(
        r##"<path id="p" transform="scale(2)" d="M 50 100 L 250 100"/><text><textPath href="#p">AB</textPath></text>"##,
    );
    let g = glyphs(&d);
    assert!(
        close(g[0].e, 100.0) && close(g[0].f, 200.0) && close(g[1].e - g[0].e, adv()),
        "{g:?}"
    );
}

#[test]
fn stretch_method_warps_outlines_onto_the_path() {
    let d = doc(
        r#"<text><textPath path="M 100 200 Q 300 0 500 200" method="stretch">AB</textPath></text>"#,
    );
    let g = text_group(&d);
    // No rigid placements: each glyph is its own bent outline.
    assert_eq!(g.children.len(), 2);
    assert!(g.children.iter().all(|c| matches!(c, Node::Path(_))));
    let Node::Path(p) = &g.children[0] else {
        unreachable!()
    };
    let b = oxideav_svg::bbox::BBox::of_path(&p.path).unwrap();
    // "A" hugs the start of the curve, leaning with its tangent.
    assert!(
        b.x > 85.0 && b.max_x() < 115.0 && b.max_y() <= 201.0,
        "{b:?}"
    );
    // A rigidly placed "A" has straight legs; the warped one is made of
    // short chords along the curve.
    assert!(p.path.commands.len() > 12, "{}", p.path.commands.len());
}

#[test]
fn invalid_path_reference_renders_nothing_for_the_span() {
    let d = doc(r##"<text x="10" y="50">A<textPath href="#nowhere">XYZ</textPath></text>"##);
    assert_eq!(xs(&d).len(), 1);
}
