//! Inherited text properties (SVG 2 §11.10, CSS Fonts / CSS Text /
//! CSS Writing Modes as adopted by SVG 2).
//!
//! These ride on [`crate::element::PaintState`] so a `font-family` on
//! the root `<svg>` or a `writing-mode` on a `<g>` reaches every
//! descendant `<text>` through the ordinary cascade. The module has no
//! shaping dependency: it only parses and carries values; the `text`
//! feature's layout engine consumes them.

use std::sync::Arc;

use crate::length::{Length, parse_length};

/// `font-style` (CSS Fonts): `normal | italic | oblique`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
    Oblique,
}

/// `writing-mode` (SVG 2 §11.10.2.3). The SVG 1.1 keywords map onto
/// the CSS ones: `lr` / `lr-tb` / `rl` / `rl-tb` → `horizontal-tb`,
/// `tb` / `tb-rl` → `vertical-rl`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum WritingMode {
    #[default]
    HorizontalTb,
    VerticalRl,
    VerticalLr,
    /// `sideways-rl`: vertical, every glyph set sideways.
    SidewaysRl,
    /// `sideways-lr`: vertical, bottom to top, every glyph set sideways
    /// rotated counter-clockwise.
    SidewaysLr,
}

impl WritingMode {
    /// True for every mode whose inline direction is vertical.
    pub fn is_vertical(self) -> bool {
        !matches!(self, WritingMode::HorizontalTb)
    }
}

/// `text-orientation` (CSS Writing Modes): glyph orientation within a
/// vertical line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum TextOrientation {
    #[default]
    Mixed,
    Upright,
    Sideways,
}

/// `direction` (SVG 2 §11.10.2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Direction {
    #[default]
    Ltr,
    Rtl,
}

/// `white-space` (SVG 2 §11.10.3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WhiteSpace {
    Normal,
    NoWrap,
    Pre,
    PreWrap,
    PreLine,
    BreakSpaces,
}

impl WhiteSpace {
    /// White-space runs collapse (`normal`, `nowrap`, `pre-line`).
    pub fn collapses(self) -> bool {
        matches!(
            self,
            WhiteSpace::Normal | WhiteSpace::NoWrap | WhiteSpace::PreLine
        )
    }

    /// A line feed is a forced line break (§11.6.1).
    pub fn preserves_breaks(self) -> bool {
        !matches!(self, WhiteSpace::Normal | WhiteSpace::NoWrap)
    }
}

/// `line-height` (SVG 2 §11.10.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum LineHeight {
    #[default]
    Normal,
    /// A multiple of the element's font size.
    Number(f32),
    Length(Length),
}

/// `text-decoration-line` bits.
pub const DECORATION_UNDERLINE: u8 = 1;
/// See [`DECORATION_UNDERLINE`].
pub const DECORATION_OVERLINE: u8 = 2;
/// See [`DECORATION_UNDERLINE`].
pub const DECORATION_LINE_THROUGH: u8 = 4;

/// The inherited text properties of one element.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct TextProps {
    /// Raw `font-family` list (comma separated, quotes kept); `None`
    /// until some ancestor sets one.
    pub font_family: Option<Arc<str>>,
    pub font_style: FontStyle,
    /// `font-weight` as a 1..=1000 number (`normal` 400, `bold` 700).
    pub font_weight: u16,
    /// `font-variant: small-caps` (or `font-variant-caps`).
    pub small_caps: bool,
    /// `letter-spacing`; `None` is `normal`.
    pub letter_spacing: Option<Length>,
    /// `word-spacing`; `None` is `normal`.
    pub word_spacing: Option<Length>,
    pub writing_mode: WritingMode,
    pub text_orientation: TextOrientation,
    pub direction: Direction,
    /// `white-space`; `None` while unset, in which case `xml:space`
    /// governs (§11.10.3.3).
    pub white_space: Option<WhiteSpace>,
    /// `xml:space="preserve"` in effect.
    pub xml_space_preserve: bool,
    pub line_height: LineHeight,
    /// Decoration lines declared by an ancestor outside the `<text>`
    /// element (`DECORATION_*` bits).
    pub decoration: u8,
}

impl Default for TextProps {
    fn default() -> Self {
        Self {
            font_family: None,
            font_style: FontStyle::Normal,
            font_weight: 400,
            small_caps: false,
            letter_spacing: None,
            word_spacing: None,
            writing_mode: WritingMode::HorizontalTb,
            text_orientation: TextOrientation::Mixed,
            direction: Direction::Ltr,
            white_space: None,
            xml_space_preserve: false,
            line_height: LineHeight::Normal,
            decoration: 0,
        }
    }
}

impl TextProps {
    /// Apply one property; returns `true` when `name` is a text
    /// property this struct models (whether or not `value` was valid —
    /// an invalid value leaves the inherited one in place).
    pub(crate) fn apply(&mut self, name: &str, value: &str) -> bool {
        let v = value.trim();
        match name {
            "font-family" => {
                if !v.is_empty() && !v.eq_ignore_ascii_case("inherit") {
                    self.font_family = Some(Arc::from(v));
                }
            }
            "font-style" => {
                if let Some(s) = parse_font_style(v) {
                    self.font_style = s;
                }
            }
            "font-weight" => {
                if let Some(w) = parse_font_weight(v, self.font_weight) {
                    self.font_weight = w;
                }
            }
            "font-variant" | "font-variant-caps" => {
                if v.eq_ignore_ascii_case("small-caps") {
                    self.small_caps = true;
                } else if v.eq_ignore_ascii_case("normal") {
                    self.small_caps = false;
                }
            }
            "font" => self.apply_font_shorthand(v),
            "letter-spacing" => {
                if let Some(s) = parse_spacing(v) {
                    self.letter_spacing = s;
                }
            }
            "word-spacing" => {
                if let Some(s) = parse_spacing(v) {
                    self.word_spacing = s;
                }
            }
            "writing-mode" => {
                if let Some(m) = parse_writing_mode(v) {
                    self.writing_mode = m;
                }
            }
            "text-orientation" => {
                self.text_orientation = match v.to_ascii_lowercase().as_str() {
                    "mixed" => TextOrientation::Mixed,
                    "upright" => TextOrientation::Upright,
                    "sideways" | "sideways-right" => TextOrientation::Sideways,
                    _ => self.text_orientation,
                };
            }
            // SVG 1.1 §10.7.3: `0` sets glyphs upright, `90` sideways
            // (SVG 2 §11.10.1.3 maps the property onto
            // `text-orientation`).
            "glyph-orientation-vertical" => {
                let t = v.trim_end_matches("deg").trim();
                if t == "0" {
                    self.text_orientation = TextOrientation::Upright;
                } else if t == "90" {
                    self.text_orientation = TextOrientation::Sideways;
                } else if t.eq_ignore_ascii_case("auto") {
                    self.text_orientation = TextOrientation::Mixed;
                }
            }
            "direction" => {
                if v.eq_ignore_ascii_case("rtl") {
                    self.direction = Direction::Rtl;
                } else if v.eq_ignore_ascii_case("ltr") {
                    self.direction = Direction::Ltr;
                }
            }
            "white-space" => {
                self.white_space = match v.to_ascii_lowercase().as_str() {
                    "normal" => Some(WhiteSpace::Normal),
                    "nowrap" => Some(WhiteSpace::NoWrap),
                    "pre" => Some(WhiteSpace::Pre),
                    "pre-wrap" => Some(WhiteSpace::PreWrap),
                    "pre-line" => Some(WhiteSpace::PreLine),
                    "break-spaces" => Some(WhiteSpace::BreakSpaces),
                    _ => self.white_space,
                };
            }
            "xml:space" => {
                if v == "preserve" {
                    self.xml_space_preserve = true;
                } else if v == "default" {
                    self.xml_space_preserve = false;
                }
            }
            "line-height" => {
                if let Some(lh) = parse_line_height(v) {
                    self.line_height = lh;
                }
            }
            "text-decoration" | "text-decoration-line" => {
                if let Some(bits) = parse_decoration_lines(v) {
                    self.decoration = bits;
                }
            }
            _ => return false,
        }
        true
    }

    /// CSS `font` shorthand: `[style || variant || weight]? size
    /// [/ line-height]? family`. Every sub-property not named resets to
    /// its initial value; `font-size` itself is picked up by the length
    /// cascade ([`font_shorthand_size`]).
    fn apply_font_shorthand(&mut self, v: &str) {
        let Some(parts) = split_font_shorthand(v) else {
            return;
        };
        self.font_style = FontStyle::Normal;
        self.font_weight = 400;
        self.small_caps = false;
        self.line_height = LineHeight::Normal;
        for tok in parts.prefix {
            if let Some(s) = parse_font_style(tok) {
                if !tok.eq_ignore_ascii_case("normal") {
                    self.font_style = s;
                }
            } else if tok.eq_ignore_ascii_case("small-caps") {
                self.small_caps = true;
            } else if let Some(w) = parse_font_weight(tok, 400) {
                self.font_weight = w;
            }
        }
        if let Some(lh) = parts.line_height.and_then(parse_line_height) {
            self.line_height = lh;
        }
        if !parts.family.is_empty() {
            self.font_family = Some(Arc::from(parts.family));
        }
    }
}

/// The pieces of a `font` shorthand value.
struct FontShorthand<'a> {
    prefix: Vec<&'a str>,
    size: &'a str,
    line_height: Option<&'a str>,
    family: &'a str,
}

fn split_font_shorthand(v: &str) -> Option<FontShorthand<'_>> {
    let mut prefix = Vec::new();
    let mut rest = v.trim();
    loop {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let tok = &rest[..end];
        if tok.is_empty() {
            return None;
        }
        let (size_tok, lh) = match tok.split_once('/') {
            Some((s, l)) => (s, Some(l)),
            None => (tok, None),
        };
        // A bare `100`..`1000` followed by another size token is the
        // weight (`600 12pt Arial`), not the size.
        let is_weight_number = lh.is_none()
            && matches!(size_tok.parse::<u16>(), Ok(1..=1000))
            && looks_like_size(rest[end..].trim());
        let is_size = !is_weight_number
            && size_tok
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit() || c == '.')
            && parse_length(size_tok).is_ok();
        if is_size {
            let mut after = rest[end..].trim_start();
            let mut line_height = lh.filter(|l| !l.is_empty());
            if line_height.is_none()
                && let Some(stripped) = after.strip_prefix('/')
            {
                let s = stripped.trim_start();
                let e = s.find(char::is_whitespace).unwrap_or(s.len());
                line_height = Some(&s[..e]);
                after = s[e..].trim_start();
            }
            return Some(FontShorthand {
                prefix,
                size: size_tok,
                line_height,
                family: after,
            });
        }
        prefix.push(tok);
        rest = rest[end..].trim_start();
        if rest.is_empty() || prefix.len() > 4 {
            return None;
        }
    }
}

fn looks_like_size(s: &str) -> bool {
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    let tok = s[..end].split('/').next().unwrap_or("");
    tok.chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == '.')
        && parse_length(tok).is_ok()
}

/// The `font-size` component of a `font` shorthand value, if the value
/// parses as one.
pub(crate) fn font_shorthand_size(v: &str) -> Option<&str> {
    split_font_shorthand(v).map(|p| p.size)
}

fn parse_font_style(v: &str) -> Option<FontStyle> {
    let lower = v.to_ascii_lowercase();
    let first = lower.split_whitespace().next()?;
    match first {
        "normal" => Some(FontStyle::Normal),
        "italic" => Some(FontStyle::Italic),
        "oblique" => Some(FontStyle::Oblique),
        _ => None,
    }
}

fn parse_font_weight(v: &str, inherited: u16) -> Option<u16> {
    match v.to_ascii_lowercase().as_str() {
        "normal" => Some(400),
        "bold" => Some(700),
        // CSS Fonts relative weights, resolved against the inherited
        // value.
        "bolder" => Some(match inherited {
            0..=349 => 400,
            350..=549 => 700,
            _ => 900,
        }),
        "lighter" => Some(match inherited {
            0..=549 => 100,
            550..=749 => 400,
            _ => 700,
        }),
        other => {
            let n: f32 = other.parse().ok()?;
            (1.0..=1000.0).contains(&n).then_some(n.round() as u16)
        }
    }
}

/// `normal | <length>`; outer `None` = invalid.
fn parse_spacing(v: &str) -> Option<Option<Length>> {
    if v.eq_ignore_ascii_case("normal") {
        return Some(None);
    }
    parse_length(v).ok().map(Some)
}

fn parse_writing_mode(v: &str) -> Option<WritingMode> {
    Some(match v.to_ascii_lowercase().as_str() {
        "horizontal-tb" | "lr" | "lr-tb" | "rl" | "rl-tb" => WritingMode::HorizontalTb,
        "vertical-rl" | "tb" | "tb-rl" => WritingMode::VerticalRl,
        "vertical-lr" => WritingMode::VerticalLr,
        "sideways-rl" => WritingMode::SidewaysRl,
        "sideways-lr" => WritingMode::SidewaysLr,
        _ => return None,
    })
}

fn parse_line_height(v: &str) -> Option<LineHeight> {
    if v.eq_ignore_ascii_case("normal") {
        return Some(LineHeight::Normal);
    }
    if let Ok(n) = v.parse::<f32>() {
        return (n >= 0.0).then_some(LineHeight::Number(n));
    }
    if let Some(p) = v.strip_suffix('%') {
        let n: f32 = p.trim().parse().ok()?;
        return (n >= 0.0).then_some(LineHeight::Number(n / 100.0));
    }
    parse_length(v).ok().map(LineHeight::Length)
}

/// Parse the line keywords out of a `text-decoration` /
/// `text-decoration-line` value. Colour and style tokens of the
/// shorthand are skipped here (the layout engine reads them from the
/// declaring element). `None` when no line keyword and no `none` is
/// present.
pub(crate) fn parse_decoration_lines(v: &str) -> Option<u8> {
    let mut bits = 0u8;
    let mut seen = false;
    for tok in v.split_whitespace() {
        match tok.to_ascii_lowercase().as_str() {
            "none" => seen = true,
            "underline" => {
                bits |= DECORATION_UNDERLINE;
                seen = true;
            }
            "overline" => {
                bits |= DECORATION_OVERLINE;
                seen = true;
            }
            "line-through" => {
                bits |= DECORATION_LINE_THROUGH;
                seen = true;
            }
            _ => {}
        }
    }
    seen.then_some(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_shorthand_splits_size_line_height_family() {
        let mut p = TextProps::default();
        assert!(p.apply("font", "italic bold 20px/1.5 \"Noto Serif\", serif"));
        assert_eq!(p.font_style, FontStyle::Italic);
        assert_eq!(p.font_weight, 700);
        assert_eq!(p.line_height, LineHeight::Number(1.5));
        assert_eq!(p.font_family.as_deref(), Some("\"Noto Serif\", serif"));
        assert_eq!(
            font_shorthand_size("italic bold 20px/1.5 serif"),
            Some("20px")
        );
        assert_eq!(font_shorthand_size("20px IPAMincho"), Some("20px"));
        assert_eq!(font_shorthand_size("600 12pt Arial"), Some("12pt"));
    }

    #[test]
    fn relative_weights_resolve_against_inherited() {
        let mut p = TextProps::default();
        p.apply("font-weight", "bolder");
        assert_eq!(p.font_weight, 700);
        p.apply("font-weight", "bolder");
        assert_eq!(p.font_weight, 900);
        p.apply("font-weight", "lighter");
        assert_eq!(p.font_weight, 700);
    }

    #[test]
    fn legacy_writing_modes_map_to_css() {
        let mut p = TextProps::default();
        p.apply("writing-mode", "tb");
        assert_eq!(p.writing_mode, WritingMode::VerticalRl);
        p.apply("writing-mode", "lr-tb");
        assert_eq!(p.writing_mode, WritingMode::HorizontalTb);
    }

    #[test]
    fn decoration_lines_parse_from_shorthand() {
        assert_eq!(
            parse_decoration_lines("underline dotted red"),
            Some(DECORATION_UNDERLINE)
        );
        assert_eq!(
            parse_decoration_lines("underline line-through"),
            Some(DECORATION_UNDERLINE | DECORATION_LINE_THROUGH)
        );
        assert_eq!(parse_decoration_lines("none"), Some(0));
        assert_eq!(parse_decoration_lines("red"), None);
    }
}
