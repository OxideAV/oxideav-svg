//! `fill` / `stroke` value parser.
//!
//! Implements the SVG 1.1 §4.4 paint-value subset that doesn't require
//! `<defs>` lookup:
//!
//! - `none` → `None`
//! - `#RGB` / `#RGBA` / `#RRGGBB` / `#RRGGBBAA` hex (CSS Color L4)
//! - `rgb(r, g, b)` and `rgba(r, g, b, a)` — `r`/`g`/`b` accept either
//!   `0..=255` integers or `0%..=100%` percentages; `a` is `0.0..=1.0`.
//! - All 147 named CSS colors (the CSS Color L3 named-color set).
//!
//! Returns the parsed [`Rgba`] paired with whether the input was the
//! literal `none` keyword. Gradient `url(#id)` references are handled
//! separately by [`parse_paint_ref`].

use oxideav_core::{Error, Result, Rgba};

/// Result of parsing a `fill` or `stroke` value.
#[derive(Clone, Debug, PartialEq)]
pub enum PaintValue {
    /// `none` — no paint.
    None,
    /// A solid colour.
    Color(Rgba),
    /// A `url(#id)` reference to a gradient / pattern, with an optional
    /// fallback per SVG 2 §13.2 (`<paint> = url(...) [none | <color>]?`).
    /// The `id` is stored verbatim (without the leading `#`). The
    /// fallback is taken when the reference resolves to an unknown id
    /// or to an element that isn't a valid paint server.
    ///
    /// `fallback == Some(None)` encodes the explicit `none` token (paint
    /// suppressed if the reference is invalid); `fallback == None`
    /// indicates the SVG-1.1-style bare reference with no fallback
    /// token. The two are distinct on round-trip so the encoder can
    /// re-emit the source verbatim instead of injecting a synthetic
    /// `none`.
    Reference {
        id: String,
        fallback: Option<Option<Rgba>>,
    },
}

impl PaintValue {
    /// Backwards-compat constructor matching the pre-round-20
    /// [`PaintValue::Reference(String)`] shape (no fallback token).
    /// Round 20 widened the variant to carry the SVG 2 paint-list
    /// fallback; this helper keeps the legacy call sites compact.
    pub fn reference(id: impl Into<String>) -> Self {
        PaintValue::Reference {
            id: id.into(),
            fallback: None,
        }
    }
}

/// Parse a paint value (`fill`/`stroke` attribute). Whitespace is
/// trimmed; matching is case-insensitive for the keyword and named-
/// colour forms.
pub fn parse_paint(src: &str) -> Result<PaintValue> {
    let s = src.trim();
    if s.is_empty() {
        return Err(Error::invalid("SVG paint: empty"));
    }
    if s.eq_ignore_ascii_case("none") {
        return Ok(PaintValue::None);
    }
    if s.eq_ignore_ascii_case("currentColor") {
        // `currentColor` resolves to the inherited `color` property at
        // render time — round 1 doesn't model inheritance, so fall
        // back to opaque black (the default `color` value per CSS).
        return Ok(PaintValue::Color(Rgba::opaque(0, 0, 0)));
    }
    if let Some(stripped) = s.strip_prefix('#') {
        return parse_hex(stripped).map(PaintValue::Color);
    }
    if let Some(rest) = strip_func(s, "rgb") {
        return parse_rgb_args(rest, false).map(PaintValue::Color);
    }
    if let Some(rest) = strip_func(s, "rgba") {
        return parse_rgb_args(rest, true).map(PaintValue::Color);
    }
    if let Some((rest, tail)) = strip_func_with_tail(s, "url") {
        // url(#id) [none | <color>]? — strip optional surrounding quotes
        // and the leading '#' on the id; the remainder, if any, is the
        // SVG 2 §13.2 fallback token.
        let inner = rest.trim().trim_matches(|c: char| c == '\'' || c == '"');
        let id = inner.strip_prefix('#').unwrap_or(inner).to_string();
        let fallback = parse_paint_fallback(tail)?;
        return Ok(PaintValue::Reference { id, fallback });
    }
    if let Some(rgb) = lookup_named_color(s) {
        return Ok(PaintValue::Color(Rgba::opaque(rgb.0, rgb.1, rgb.2)));
    }
    Err(Error::invalid("SVG paint: unrecognised value"))
}

/// Parse a number in `0.0..=1.0` for `opacity` / `fill-opacity` /
/// `stroke-opacity`. Out-of-range values are clamped per §11.2.
pub fn parse_opacity(src: &str) -> Result<f32> {
    let s = src.trim();
    let v = s
        .parse::<f32>()
        .map_err(|_| Error::invalid("SVG opacity: malformed number"))?;
    Ok(v.clamp(0.0, 1.0))
}

fn strip_func<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let lower = s.to_ascii_lowercase();
    if !lower.starts_with(name) {
        return None;
    }
    let rest = &s[name.len()..];
    let rest = rest.trim_start();
    let inner = rest.strip_prefix('(')?;
    let close = inner.rfind(')')?;
    Some(&inner[..close])
}

/// Like [`strip_func`] but also returns the (trimmed) tail after the
/// closing parenthesis. Used by the SVG 2 §13.2 paint-list parser to
/// pick up the optional `[none | <color>]` fallback after `url(...)`.
fn strip_func_with_tail<'a>(s: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    let lower = s.to_ascii_lowercase();
    if !lower.starts_with(name) {
        return None;
    }
    let rest = &s[name.len()..];
    let rest = rest.trim_start();
    let inner = rest.strip_prefix('(')?;
    let close = inner.find(')')?;
    let payload = &inner[..close];
    let tail = inner[close + 1..].trim();
    Some((payload, tail))
}

/// Parse the optional fallback after a `url(...)` reference per SVG 2
/// §13.2. Returns:
///   - `Ok(None)`             — no fallback token (legacy bare-reference)
///   - `Ok(Some(None))`       — explicit `none` (suppress paint on
///     resolution failure)
///   - `Ok(Some(Some(Rgba)))` — explicit colour fallback
fn parse_paint_fallback(tail: &str) -> Result<Option<Option<Rgba>>> {
    if tail.is_empty() {
        return Ok(None);
    }
    if tail.eq_ignore_ascii_case("none") {
        return Ok(Some(None));
    }
    // Per SVG 2 §13.2 only a <color> may follow `none` is its own
    // sibling. Reuse parse_paint recursively but reject another url(...)
    // (no chained paint servers).
    let inner = parse_paint(tail)?;
    match inner {
        PaintValue::Color(c) => Ok(Some(Some(c))),
        PaintValue::None => Ok(Some(None)),
        PaintValue::Reference { .. } => Err(Error::invalid(
            "SVG paint: fallback after url(...) must be `none` or a colour",
        )),
    }
}

fn parse_hex(rest: &str) -> Result<Rgba> {
    let bytes = rest.as_bytes();
    let parse_nyb = |b: u8| -> Result<u8> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            b'A'..=b'F' => Ok(b - b'A' + 10),
            _ => Err(Error::invalid("SVG paint: bad hex digit")),
        }
    };
    match bytes.len() {
        3 => {
            // #RGB — each digit is doubled.
            let r = parse_nyb(bytes[0])?;
            let g = parse_nyb(bytes[1])?;
            let b = parse_nyb(bytes[2])?;
            Ok(Rgba::opaque(r * 17, g * 17, b * 17))
        }
        4 => {
            // #RGBA.
            let r = parse_nyb(bytes[0])?;
            let g = parse_nyb(bytes[1])?;
            let b = parse_nyb(bytes[2])?;
            let a = parse_nyb(bytes[3])?;
            Ok(Rgba::new(r * 17, g * 17, b * 17, a * 17))
        }
        6 => {
            let r = parse_nyb(bytes[0])? * 16 + parse_nyb(bytes[1])?;
            let g = parse_nyb(bytes[2])? * 16 + parse_nyb(bytes[3])?;
            let b = parse_nyb(bytes[4])? * 16 + parse_nyb(bytes[5])?;
            Ok(Rgba::opaque(r, g, b))
        }
        8 => {
            let r = parse_nyb(bytes[0])? * 16 + parse_nyb(bytes[1])?;
            let g = parse_nyb(bytes[2])? * 16 + parse_nyb(bytes[3])?;
            let b = parse_nyb(bytes[4])? * 16 + parse_nyb(bytes[5])?;
            let a = parse_nyb(bytes[6])? * 16 + parse_nyb(bytes[7])?;
            Ok(Rgba::new(r, g, b, a))
        }
        _ => Err(Error::invalid(
            "SVG paint: hex color must be 3/4/6/8 digits",
        )),
    }
}

fn parse_rgb_args(args: &str, with_alpha: bool) -> Result<Rgba> {
    let parts: Vec<&str> = args
        .split([',', '/', ' ', '\t', '\n', '\r'])
        .filter(|p| !p.is_empty())
        .collect();
    let expected = if with_alpha { 4 } else { 3 };
    if parts.len() != expected {
        return Err(Error::invalid("SVG paint: rgb()/rgba() arg count mismatch"));
    }
    let r = parse_channel(parts[0])?;
    let g = parse_channel(parts[1])?;
    let b = parse_channel(parts[2])?;
    let a = if with_alpha {
        let raw: f32 = parts[3]
            .parse()
            .map_err(|_| Error::invalid("SVG paint: malformed alpha"))?;
        (raw.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        255
    };
    Ok(Rgba::new(r, g, b, a))
}

fn parse_channel(s: &str) -> Result<u8> {
    if let Some(p) = s.strip_suffix('%') {
        let v: f32 = p
            .trim()
            .parse()
            .map_err(|_| Error::invalid("SVG paint: malformed percent"))?;
        Ok((v.clamp(0.0, 100.0) / 100.0 * 255.0).round() as u8)
    } else {
        let v: f32 = s
            .trim()
            .parse()
            .map_err(|_| Error::invalid("SVG paint: malformed channel"))?;
        Ok(v.clamp(0.0, 255.0).round() as u8)
    }
}

/// Look up a CSS named color. Returns `None` for unknown names.
pub fn lookup_named_color(name: &str) -> Option<(u8, u8, u8)> {
    let key = name.trim().to_ascii_lowercase();
    NAMED_COLORS
        .iter()
        .find(|(n, _)| *n == key.as_str())
        .map(|(_, rgb)| *rgb)
}

/// CSS Color L3 named-color table. Source: CSS Color Module Level 3
/// §4.3 (W3C Recommendation, 2022). All names are lower-case.
const NAMED_COLORS: &[(&str, (u8, u8, u8))] = &[
    ("aliceblue", (240, 248, 255)),
    ("antiquewhite", (250, 235, 215)),
    ("aqua", (0, 255, 255)),
    ("aquamarine", (127, 255, 212)),
    ("azure", (240, 255, 255)),
    ("beige", (245, 245, 220)),
    ("bisque", (255, 228, 196)),
    ("black", (0, 0, 0)),
    ("blanchedalmond", (255, 235, 205)),
    ("blue", (0, 0, 255)),
    ("blueviolet", (138, 43, 226)),
    ("brown", (165, 42, 42)),
    ("burlywood", (222, 184, 135)),
    ("cadetblue", (95, 158, 160)),
    ("chartreuse", (127, 255, 0)),
    ("chocolate", (210, 105, 30)),
    ("coral", (255, 127, 80)),
    ("cornflowerblue", (100, 149, 237)),
    ("cornsilk", (255, 248, 220)),
    ("crimson", (220, 20, 60)),
    ("cyan", (0, 255, 255)),
    ("darkblue", (0, 0, 139)),
    ("darkcyan", (0, 139, 139)),
    ("darkgoldenrod", (184, 134, 11)),
    ("darkgray", (169, 169, 169)),
    ("darkgreen", (0, 100, 0)),
    ("darkgrey", (169, 169, 169)),
    ("darkkhaki", (189, 183, 107)),
    ("darkmagenta", (139, 0, 139)),
    ("darkolivegreen", (85, 107, 47)),
    ("darkorange", (255, 140, 0)),
    ("darkorchid", (153, 50, 204)),
    ("darkred", (139, 0, 0)),
    ("darksalmon", (233, 150, 122)),
    ("darkseagreen", (143, 188, 143)),
    ("darkslateblue", (72, 61, 139)),
    ("darkslategray", (47, 79, 79)),
    ("darkslategrey", (47, 79, 79)),
    ("darkturquoise", (0, 206, 209)),
    ("darkviolet", (148, 0, 211)),
    ("deeppink", (255, 20, 147)),
    ("deepskyblue", (0, 191, 255)),
    ("dimgray", (105, 105, 105)),
    ("dimgrey", (105, 105, 105)),
    ("dodgerblue", (30, 144, 255)),
    ("firebrick", (178, 34, 34)),
    ("floralwhite", (255, 250, 240)),
    ("forestgreen", (34, 139, 34)),
    ("fuchsia", (255, 0, 255)),
    ("gainsboro", (220, 220, 220)),
    ("ghostwhite", (248, 248, 255)),
    ("gold", (255, 215, 0)),
    ("goldenrod", (218, 165, 32)),
    ("gray", (128, 128, 128)),
    ("green", (0, 128, 0)),
    ("greenyellow", (173, 255, 47)),
    ("grey", (128, 128, 128)),
    ("honeydew", (240, 255, 240)),
    ("hotpink", (255, 105, 180)),
    ("indianred", (205, 92, 92)),
    ("indigo", (75, 0, 130)),
    ("ivory", (255, 255, 240)),
    ("khaki", (240, 230, 140)),
    ("lavender", (230, 230, 250)),
    ("lavenderblush", (255, 240, 245)),
    ("lawngreen", (124, 252, 0)),
    ("lemonchiffon", (255, 250, 205)),
    ("lightblue", (173, 216, 230)),
    ("lightcoral", (240, 128, 128)),
    ("lightcyan", (224, 255, 255)),
    ("lightgoldenrodyellow", (250, 250, 210)),
    ("lightgray", (211, 211, 211)),
    ("lightgreen", (144, 238, 144)),
    ("lightgrey", (211, 211, 211)),
    ("lightpink", (255, 182, 193)),
    ("lightsalmon", (255, 160, 122)),
    ("lightseagreen", (32, 178, 170)),
    ("lightskyblue", (135, 206, 250)),
    ("lightslategray", (119, 136, 153)),
    ("lightslategrey", (119, 136, 153)),
    ("lightsteelblue", (176, 196, 222)),
    ("lightyellow", (255, 255, 224)),
    ("lime", (0, 255, 0)),
    ("limegreen", (50, 205, 50)),
    ("linen", (250, 240, 230)),
    ("magenta", (255, 0, 255)),
    ("maroon", (128, 0, 0)),
    ("mediumaquamarine", (102, 205, 170)),
    ("mediumblue", (0, 0, 205)),
    ("mediumorchid", (186, 85, 211)),
    ("mediumpurple", (147, 112, 219)),
    ("mediumseagreen", (60, 179, 113)),
    ("mediumslateblue", (123, 104, 238)),
    ("mediumspringgreen", (0, 250, 154)),
    ("mediumturquoise", (72, 209, 204)),
    ("mediumvioletred", (199, 21, 133)),
    ("midnightblue", (25, 25, 112)),
    ("mintcream", (245, 255, 250)),
    ("mistyrose", (255, 228, 225)),
    ("moccasin", (255, 228, 181)),
    ("navajowhite", (255, 222, 173)),
    ("navy", (0, 0, 128)),
    ("oldlace", (253, 245, 230)),
    ("olive", (128, 128, 0)),
    ("olivedrab", (107, 142, 35)),
    ("orange", (255, 165, 0)),
    ("orangered", (255, 69, 0)),
    ("orchid", (218, 112, 214)),
    ("palegoldenrod", (238, 232, 170)),
    ("palegreen", (152, 251, 152)),
    ("paleturquoise", (175, 238, 238)),
    ("palevioletred", (219, 112, 147)),
    ("papayawhip", (255, 239, 213)),
    ("peachpuff", (255, 218, 185)),
    ("peru", (205, 133, 63)),
    ("pink", (255, 192, 203)),
    ("plum", (221, 160, 221)),
    ("powderblue", (176, 224, 230)),
    ("purple", (128, 0, 128)),
    ("rebeccapurple", (102, 51, 153)),
    ("red", (255, 0, 0)),
    ("rosybrown", (188, 143, 143)),
    ("royalblue", (65, 105, 225)),
    ("saddlebrown", (139, 69, 19)),
    ("salmon", (250, 128, 114)),
    ("sandybrown", (244, 164, 96)),
    ("seagreen", (46, 139, 87)),
    ("seashell", (255, 245, 238)),
    ("sienna", (160, 82, 45)),
    ("silver", (192, 192, 192)),
    ("skyblue", (135, 206, 235)),
    ("slateblue", (106, 90, 205)),
    ("slategray", (112, 128, 144)),
    ("slategrey", (112, 128, 144)),
    ("snow", (255, 250, 250)),
    ("springgreen", (0, 255, 127)),
    ("steelblue", (70, 130, 180)),
    ("tan", (210, 180, 140)),
    ("teal", (0, 128, 128)),
    ("thistle", (216, 191, 216)),
    ("tomato", (255, 99, 71)),
    ("transparent", (0, 0, 0)),
    ("turquoise", (64, 224, 208)),
    ("violet", (238, 130, 238)),
    ("wheat", (245, 222, 179)),
    ("white", (255, 255, 255)),
    ("whitesmoke", (245, 245, 245)),
    ("yellow", (255, 255, 0)),
    ("yellowgreen", (154, 205, 50)),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_none_keyword() {
        assert_eq!(parse_paint("none").unwrap(), PaintValue::None);
        assert_eq!(parse_paint("  None  ").unwrap(), PaintValue::None);
    }

    #[test]
    fn parses_three_and_six_digit_hex() {
        assert_eq!(
            parse_paint("#abc").unwrap(),
            PaintValue::Color(Rgba::opaque(0xaa, 0xbb, 0xcc))
        );
        assert_eq!(
            parse_paint("#aabbcc").unwrap(),
            PaintValue::Color(Rgba::opaque(0xaa, 0xbb, 0xcc))
        );
    }

    #[test]
    fn parses_eight_digit_hex_with_alpha() {
        assert_eq!(
            parse_paint("#11223344").unwrap(),
            PaintValue::Color(Rgba::new(0x11, 0x22, 0x33, 0x44))
        );
        assert_eq!(
            parse_paint("#1234").unwrap(),
            PaintValue::Color(Rgba::new(0x11, 0x22, 0x33, 0x44))
        );
    }

    #[test]
    fn parses_rgb_and_rgba_with_int_and_percent() {
        assert_eq!(
            parse_paint("rgb(10, 20, 30)").unwrap(),
            PaintValue::Color(Rgba::opaque(10, 20, 30))
        );
        assert_eq!(
            parse_paint("rgba(10, 20, 30, 0.5)").unwrap(),
            PaintValue::Color(Rgba::new(10, 20, 30, 128))
        );
        assert_eq!(
            parse_paint("rgb(50%, 50%, 50%)").unwrap(),
            PaintValue::Color(Rgba::opaque(128, 128, 128))
        );
    }

    #[test]
    fn parses_named_colors_case_insensitively() {
        assert_eq!(
            parse_paint("Red").unwrap(),
            PaintValue::Color(Rgba::opaque(255, 0, 0))
        );
        assert_eq!(
            parse_paint("rebeccapurple").unwrap(),
            PaintValue::Color(Rgba::opaque(102, 51, 153))
        );
        assert_eq!(
            parse_paint("REBECCAPURPLE").unwrap(),
            PaintValue::Color(Rgba::opaque(102, 51, 153))
        );
    }

    #[test]
    fn parses_url_reference() {
        assert_eq!(
            parse_paint("url(#grad1)").unwrap(),
            PaintValue::Reference {
                id: "grad1".into(),
                fallback: None,
            }
        );
        assert_eq!(
            parse_paint("url('#g')").unwrap(),
            PaintValue::Reference {
                id: "g".into(),
                fallback: None,
            }
        );
    }

    // Round 20 — SVG 2 §13.2 paint-list with fallback.
    #[test]
    fn parses_url_reference_with_colour_fallback() {
        assert_eq!(
            parse_paint("url(#p1) red").unwrap(),
            PaintValue::Reference {
                id: "p1".into(),
                fallback: Some(Some(Rgba::opaque(255, 0, 0))),
            }
        );
        assert_eq!(
            parse_paint("url(#p1) #00ff00").unwrap(),
            PaintValue::Reference {
                id: "p1".into(),
                fallback: Some(Some(Rgba::opaque(0, 255, 0))),
            }
        );
    }

    #[test]
    fn parses_url_reference_with_none_fallback() {
        assert_eq!(
            parse_paint("url(#p1) none").unwrap(),
            PaintValue::Reference {
                id: "p1".into(),
                fallback: Some(None),
            }
        );
        // Case-insensitive on the `none` token.
        assert_eq!(
            parse_paint("url(#p1) NONE").unwrap(),
            PaintValue::Reference {
                id: "p1".into(),
                fallback: Some(None),
            }
        );
    }

    #[test]
    fn rejects_chained_paint_server_fallback() {
        // `url(#a) url(#b)` is not a valid SVG 2 paint-list — the
        // fallback must be `none` or a <color>.
        assert!(parse_paint("url(#a) url(#b)").is_err());
    }

    #[test]
    fn opacity_is_clamped() {
        assert_eq!(parse_opacity("0.5").unwrap(), 0.5);
        assert_eq!(parse_opacity("-0.1").unwrap(), 0.0);
        assert_eq!(parse_opacity("1.5").unwrap(), 1.0);
    }

    #[test]
    fn unknown_value_is_error() {
        assert!(parse_paint("not-a-color").is_err());
    }
}
