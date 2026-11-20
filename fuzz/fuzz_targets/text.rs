#![no_main]

//! Hostile-input liveness of the text layout engine. With a font hook
//! installed every `<text>` shapes and positions real glyphs, so the
//! white-space, bidirectional, positioning, `<textPath>`, paint-server
//! and decoration code all run on the fuzzer's markup. Nothing may
//! panic, hang or allocate without bound.

use libfuzzer_sys::fuzz_target;
use oxideav_scribe::{Face, FaceChain};

const FONT: &[u8] = include_bytes!("../../tests/fixtures/DejaVuSansMono.ttf");

/// Attribute sets the second half of the target wraps the input in.
const WRAPPERS: &[&str] = &[
    r#"x="10" y="20""#,
    r#"x="10 20 30" y="5 6" dx="1 2 3" dy="-1" rotate="10 20""#,
    r#"textLength="120" lengthAdjust="spacingAndGlyphs" text-anchor="middle""#,
    r#"writing-mode="vertical-rl" text-orientation="mixed" style="white-space:pre""#,
    r#"direction="rtl" unicode-bidi="bidi-override" letter-spacing="2" word-spacing="-1""#,
    r##"fill="url(#g)" stroke="url(#p)" stroke-width="1" text-decoration="underline overline line-through wavy""##,
    r#"style="inline-size: 60px; font: italic bold 9px serif" xml:space="preserve""#,
    r#"dominant-baseline="middle" baseline-shift="super" font-variant="small-caps""#,
    r#"x="5" y="9" style="white-space: pre; inline-size: 30px; text-overflow: ellipsis""#,
    r#"textLength="0" lengthAdjust="spacingAndGlyphs" font-size="0.01" letter-spacing="-9""#,
    r#"text-anchor="end" style="white-space: nowrap; inline-size: 2em; text-overflow: clip; writing-mode: vertical-lr""#,
];

fuzz_target!(|data: &[u8]| {
    let _ = oxideav_svg::text::set_font_resolver(|_family, _size| {
        Face::from_ttf_bytes(FONT.to_vec()).ok().map(FaceChain::new)
    });
    // 1. The input as a whole document.
    if let Ok((doc, extras)) = oxideav_svg::parse_with_extras(data) {
        let _ = oxideav_svg::write_with_extras(&doc, &extras);
    }
    // 2. The input as the content of a text element, plain and on a
    //    path, under one of the attribute sets.
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let body = String::from_utf8_lossy(rest);
    let attrs = WRAPPERS[selector as usize % WRAPPERS.len()];
    let doc = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><defs><linearGradient id="g"><stop offset="0" stop-color="red"/><stop offset="1"/></linearGradient><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="2" height="2"/></pattern><path id="c" d="M10 50 a40 40 0 1 1 80 0 a40 40 0 1 1 -80 0 z"/></defs><text {attrs}>{body}</text><text {attrs}><textPath href="#c" startOffset="30%" side="right">{body}</textPath>tail</text></svg>"##
    );
    let _ = oxideav_svg::parse(doc.as_bytes());
});
