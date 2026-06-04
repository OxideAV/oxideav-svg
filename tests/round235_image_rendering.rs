//! Round 235 — SVG 2 §13.10.4 `image-rendering` integration tests.
//!
//! `image-rendering: auto | optimizeQuality | optimizeSpeed` is a
//! *hint* to the user agent about quality vs speed tradeoffs when
//! sampling a raster `<image>` into vector space — it never alters
//! the source bytes. Per §13.10.4:
//!
//! * Initial value `auto` — UA balances speed / quality with a
//!   quality bias (at least nearest-neighbour, bilinear preferred).
//! * Applies to images (`<image>`).
//! * Inherited — a `<g image-rendering=…>` propagates down the
//!   normal cascade.
//!
//! Round 235 ships parse + inherited cascade + round-trip
//! preservation. The actual resampling-algorithm selection
//! (nearest-neighbour, bilinear, …) lives in `oxideav-raster`; this
//! crate exposes the resolved value on `PaintState::image_rendering`
//! and round-trips the source attribute via
//! `SvgImage::image_rendering`.

use oxideav_svg::element::{ImageRendering, PaintState};
use oxideav_svg::parser::{parse_xml, Element, Node as XmlNode};
use oxideav_svg::{parse_svg, parse_svg_with_extras, write_svg_with_extras};

/// Default `image-rendering` is `auto` (per §13.10.4 Initial table).
#[test]
fn default_image_rendering_is_auto() {
    let s = PaintState::default();
    assert_eq!(s.image_rendering, ImageRendering::Auto);
}

/// Round 235: a document without `image-rendering=` parses cleanly
/// and the captured `<image>` records no binding.
#[test]
fn baseline_no_image_rendering_attr_no_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="0" y="0" width="50" height="50"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.images.len(), 1);
    assert!(
        extras.images[0].image_rendering.is_none(),
        "round 235: an <image> without image-rendering= must not record a binding"
    );
}

/// Round 235: `image-rendering="optimizeQuality"` on an `<image>`
/// records a binding with the canonicalised camelCase keyword.
#[test]
fn optimize_quality_on_image_records_binding() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="0" y="0" width="50" height="50"
               image-rendering="optimizeQuality"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.images.len(), 1);
    assert_eq!(
        extras.images[0].image_rendering.as_deref(),
        Some("optimizeQuality")
    );
}

/// Round 235: each of the three §13.10.4 keywords parses + records.
#[test]
fn each_keyword_records_canonical_form() {
    for (input, expected) in [
        ("auto", "auto"),
        ("optimizeQuality", "optimizeQuality"),
        ("optimizeSpeed", "optimizeSpeed"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <image href="logo.png" x="0" y="0" width="50" height="50"
                       image-rendering="{}"/>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.images.len(), 1, "input={}", input);
        assert_eq!(
            extras.images[0].image_rendering.as_deref(),
            Some(expected),
            "input={}",
            input
        );
    }
}

/// Round 235: source keywords are matched case-insensitively per CSS
/// rules. The binding canonicalises to the spec's camelCase spelling.
#[test]
fn keyword_matching_is_case_insensitive() {
    for (input, expected) in [
        ("AUTO", "auto"),
        ("OPTIMIZEQUALITY", "optimizeQuality"),
        ("OptimizeSpeed", "optimizeSpeed"),
    ] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
                <image href="logo.png" x="0" y="0" width="50" height="50"
                       image-rendering="{}"/>
            </svg>"#,
            input
        );
        let (_frame, extras) = parse_svg_with_extras(src.as_bytes()).unwrap();
        assert_eq!(extras.images.len(), 1, "input={}", input);
        assert_eq!(
            extras.images[0].image_rendering.as_deref(),
            Some(expected),
            "input={}",
            input
        );
    }
}

/// Round 235: `image-rendering="inherit"` keeps the cascade's
/// inherited value and skips recording (no canonical token to emit).
#[test]
fn inherit_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="0" y="0" width="50" height="50"
               image-rendering="inherit"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.images.len(), 1);
    assert!(
        extras.images[0].image_rendering.is_none(),
        "round 235: `inherit` keeps the inherited value and skips recording"
    );
}

/// Round 235: unrecognised tokens fall back to the cascade's
/// inherited value and skip recording (matches the tolerant policy of
/// `text-rendering` / `shape-rendering` / `text-anchor`).
#[test]
fn unknown_keyword_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="0" y="0" width="50" height="50"
               image-rendering="someFutureKeyword"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.images.len(), 1);
    assert!(
        extras.images[0].image_rendering.is_none(),
        "round 235: unrecognised keyword keeps inherited value and skips recording"
    );
    // The document still loads (no parse failure).
    let _ = parse_svg(src).unwrap();
}

/// Round 235: empty `image-rendering=""` skips recording (no keyword
/// to canonicalise).
#[test]
fn empty_value_skips_recording() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="0" y="0" width="50" height="50"
               image-rendering=""/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.images.len(), 1);
    assert!(extras.images[0].image_rendering.is_none());
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
    let child_el: &Element = svg_el
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
        .merged_with_css(child_el, &sheet)
        .expect("merged paint state")
}

/// Round 235: a presentation attribute resolves into PaintState
/// (presentation-attribute lane of the cascade).
#[test]
fn presentation_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <image href="logo.png" image-rendering="optimizeSpeed"/>
            </svg>"#,
        "image",
    );
    assert_eq!(s.image_rendering, ImageRendering::OptimizeSpeed);
}

/// Round 235: an inline `style="…"` declaration resolves into
/// PaintState (style-attribute lane wins over presentation attribute
/// per the round-4 cascade order).
#[test]
fn style_attribute_resolves_in_cascade() {
    let s = shape_state(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <image href="logo.png" style="image-rendering: optimizeQuality"/>
            </svg>"#,
        "image",
    );
    assert_eq!(s.image_rendering, ImageRendering::OptimizeQuality);
}

/// Round 235: the property IS inherited per §13.10.4 — a child of a
/// `<g image-rendering=…>` picks up the resolved value via the cascade.
/// Verify via PaintState merge.
#[test]
fn property_is_inherited_through_cascade() {
    let parent = PaintState {
        image_rendering: ImageRendering::OptimizeQuality,
        ..PaintState::default()
    };
    // Child element without its own image-rendering should inherit.
    let nodes = parse_xml(r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="x.png"/></svg>"#)
        .expect("xml parse");
    let svg_el: &Element = nodes
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name.ends_with("svg") => Some(e),
            _ => None,
        })
        .expect("svg root");
    let image_el: &Element = svg_el
        .children
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name == "image" => Some(e),
            _ => None,
        })
        .expect("image");
    let sheet = oxideav_svg::css::Stylesheet::new();
    let child = parent.merged_with_css(image_el, &sheet).unwrap();
    assert_eq!(
        child.image_rendering,
        ImageRendering::OptimizeQuality,
        "round 235: image-rendering IS inherited per §13.10.4"
    );
}

/// Round 235: a child can override an inherited value back to a
/// different keyword via its own `image-rendering=`.
#[test]
fn child_can_override_inherited_value() {
    let parent = PaintState {
        image_rendering: ImageRendering::OptimizeQuality,
        ..PaintState::default()
    };
    let nodes = parse_xml(
        r#"<svg xmlns="http://www.w3.org/2000/svg">
              <image href="x.png" image-rendering="optimizeSpeed"/>
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
    let image_el: &Element = svg_el
        .children
        .iter()
        .find_map(|n| match n {
            XmlNode::Element(e) if e.name == "image" => Some(e),
            _ => None,
        })
        .expect("image");
    let sheet = oxideav_svg::css::Stylesheet::new();
    let child = parent.merged_with_css(image_el, &sheet).unwrap();
    assert_eq!(child.image_rendering, ImageRendering::OptimizeSpeed);
}

/// Round 235: round-trip preserves `image-rendering=` on an
/// `<image>` — a `parse_svg_with_extras → write_svg_with_extras`
/// cycle re-emits the attribute on the matching element.
#[test]
fn roundtrip_emits_attribute_on_image() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="0" y="0" width="50" height="50"
               image-rendering="optimizeSpeed"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let out_s = String::from_utf8(out).unwrap();
    assert!(
        out_s.contains("image-rendering=\"optimizeSpeed\""),
        "round-trip output should re-emit image-rendering: {}",
        out_s
    );
}

/// Round 235: double round-trip converges — a second parse-then-write
/// of the output produces identical `image-rendering=` content.
#[test]
fn roundtrip_is_idempotent() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="0" y="0" width="50" height="50"
               image-rendering="optimizeQuality"/>
    </svg>"#;
    let (frame1, extras1) = parse_svg_with_extras(src).unwrap();
    let out1 = write_svg_with_extras(&frame1, &extras1);
    let (frame2, extras2) = parse_svg_with_extras(&out1).unwrap();
    let out2 = write_svg_with_extras(&frame2, &extras2);
    assert_eq!(
        out1, out2,
        "round 235: parse → write → parse → write must converge"
    );
    let s2 = String::from_utf8(out2).unwrap();
    assert!(s2.contains("image-rendering=\"optimizeQuality\""));
}

/// Round 235: `parse_svg` (no extras) still loads the document
/// cleanly — the property cascade machinery doesn't require the
/// side-channel.
#[test]
fn parse_svg_without_extras_still_loads() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="0" y="0" width="50" height="50"
               image-rendering="optimizeSpeed"/>
    </svg>"#;
    let _ = parse_svg(src).unwrap();
}

/// Round 235: case-insensitive source canonicalises to the spec
/// camelCase spelling — round-trip emits `optimizeQuality`, not
/// `OPTIMIZEQUALITY`.
#[test]
fn roundtrip_canonicalises_case() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="0" y="0" width="50" height="50"
               image-rendering="OPTIMIZEQUALITY"/>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    let out = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(out).unwrap();
    assert!(
        s.contains("image-rendering=\"optimizeQuality\""),
        "round-trip should canonicalise to camelCase: {}",
        s
    );
    assert!(
        !s.contains("OPTIMIZEQUALITY"),
        "round-trip should NOT preserve the source case: {}",
        s
    );
}

/// Round 235: explicit author `image-rendering="auto"` IS recorded
/// (mirrors the round-221 / round-228 policy — `auto` carries author
/// intent, e.g. an inheritance reset on a descendant of a `<g
/// image-rendering="optimizeSpeed">`).
#[test]
fn explicit_auto_is_recorded() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <image href="logo.png" x="0" y="0" width="50" height="50"
               image-rendering="auto"/>
    </svg>"#;
    let (_frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.images.len(), 1);
    assert_eq!(
        extras.images[0].image_rendering.as_deref(),
        Some("auto"),
        "round 235: explicit `auto` carries author intent and IS recorded"
    );
}

/// Round 235: `image-rendering` coexists with `shape-rendering` on the
/// same sibling subtree — each captures on its own emit slot via its
/// own side-channel.
#[test]
fn coexists_with_shape_rendering() {
    let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"
                       viewBox="0 0 100 100">
        <g shape-rendering="geometricPrecision">
            <rect x="10" y="10" width="50" height="50" fill="red"/>
            <image href="logo.png" x="0" y="0" width="50" height="50"
                   image-rendering="optimizeQuality"/>
        </g>
    </svg>"#;
    let (frame, extras) = parse_svg_with_extras(src).unwrap();
    assert_eq!(extras.shape_renderings.len(), 1);
    assert_eq!(extras.images.len(), 1);
    assert_eq!(
        extras.images[0].image_rendering.as_deref(),
        Some("optimizeQuality")
    );
    let out = write_svg_with_extras(&frame, &extras);
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("shape-rendering=\"geometricPrecision\""));
    assert!(s.contains("image-rendering=\"optimizeQuality\""));
}
