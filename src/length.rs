//! CSS Values L4 length-unit aware coordinate parsing — round 18.
//!
//! Per CSS Values and Units L4 §6, an SVG coordinate (`x`, `y`,
//! `width`, …) accepts:
//!
//!   - `<number>`     — bare numeric, treated as user units (the SVG
//!     2 default unit; one user unit equals one CSS px when no
//!     transform stack is in effect);
//!   - `<length>`     — number + absolute or relative unit suffix
//!     (`px`, `pt`, `pc`, `cm`, `mm`, `q`, `in`, `em`, `rem`);
//!   - `<percentage>` — `<number>%`, resolved against the bracketing
//!     viewport's relevant axis;
//!   - viewport-relative units (`vw`, `vh`, `vmin`, `vmax`) per CSS
//!     Values L4 §6.1.3.
//!
//! Pre-round-18 the parser folded every unit suffix to user units by
//! stripping the trailing letters and keeping the numeric prefix —
//! correct for `100` and the implicit-unit shape attributes (`<rect
//! x="100">`), but wrong for `1em`, `50%`, `2vw`, etc. when an SVG
//! consumer wants the resolved px value.
//!
//! The new typed [`Length`] preserves the unit on parse so callers
//! can resolve later against the appropriate context (current
//! `font-size`, viewport dimensions, root `font-size`). The existing
//! [`crate::element::parse_number`] / [`crate::decoder`] paths stay
//! bit-for-bit identical for bare numeric inputs (they go through
//! [`LengthUnit::UserUnit`]) — round 18 adds capability without
//! changing the round-trip for existing fixtures.

/// A CSS / SVG length unit per CSS Values L4 §6.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LengthUnit {
    /// Bare number — SVG user units. One user unit ≡ 1 CSS px when
    /// no transform stack is active. Default per §6.4 ("If a length
    /// is given as a number alone, the assumed unit is `px` per CSS
    /// 2.1 §4.3.2 — but SVG 2 §10.2 calls this a 'user unit'").
    #[default]
    UserUnit,
    /// Absolute pixel unit per §6.1.1 — 1 px ≡ 1/96 in.
    Px,
    /// Font-relative — 1 em ≡ current element font-size.
    Em,
    /// Font-relative — 1 rem ≡ root element font-size.
    Rem,
    /// `<percentage>` per §6.4 — resolved against the appropriate
    /// viewport axis (handled by the caller of [`Length::resolve`]).
    Percent,
    /// Viewport-relative per §6.1.3 — 1 vw ≡ 1% of viewport width.
    Vw,
    /// Viewport-relative — 1 vh ≡ 1% of viewport height.
    Vh,
    /// Viewport-relative — 1 vmin ≡ 1% of `min(viewport_w, viewport_h)`.
    Vmin,
    /// Viewport-relative — 1 vmax ≡ 1% of `max(viewport_w, viewport_h)`.
    Vmax,
    /// Absolute typographic — 1 pt ≡ 1/72 in ≡ 4/3 px per §6.1.1.
    Pt,
    /// Absolute physical — 1 cm ≡ 96/2.54 px per §6.1.1.
    Cm,
    /// Absolute physical — 1 mm ≡ 96/25.4 px per §6.1.1.
    Mm,
    /// Absolute physical — 1 in ≡ 96 px per §6.1.1.
    In,
    /// Absolute typographic — 1 pc ≡ 12 pt ≡ 16 px per §6.1.1.
    Pc,
    /// Absolute physical — 1 q ≡ 1/4 mm per §6.1.1.
    Q,
}

/// A CSS Values L4 typed length — numeric value plus unit. Parse via
/// [`parse_length`]; resolve to a px value via [`Length::resolve`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Length {
    pub value: f32,
    pub unit: LengthUnit,
}

impl Length {
    /// Construct a length from a value and unit.
    pub const fn new(value: f32, unit: LengthUnit) -> Self {
        Self { value, unit }
    }

    /// Bare-number user-unit length — round-trips bit-for-bit with
    /// the pre-round-18 parser when the source attribute had no unit
    /// suffix (`<rect x="100">`).
    pub const fn user_units(value: f32) -> Self {
        Self {
            value,
            unit: LengthUnit::UserUnit,
        }
    }

    /// Resolve this length to a CSS px value given the resolution
    /// context. SVG renderers can then apply their natural transform
    /// stack — at this point the value is in the same coordinate
    /// space the legacy parse_number path has always produced.
    ///
    /// Conversion factors per CSS Values L4 §6.1.1:
    ///
    ///   - 1 in = 96 px
    ///   - 1 pt = 4/3 px (= 1/72 in)
    ///   - 1 pc = 16 px (= 12 pt = 1/6 in)
    ///   - 1 cm = 96/2.54 px
    ///   - 1 mm = 96/25.4 px
    ///   - 1 q  = 1/4 mm
    ///
    /// Relative units use the supplied context:
    ///
    ///   - `em`  → `value * font_size_px`
    ///   - `rem` → `value * root_font_size_px`
    ///   - `%`   → `value * percentage_basis_px / 100`
    ///   - `vw`  → `value * viewport_w / 100`
    ///   - `vh`  → `value * viewport_h / 100`
    ///   - `vmin`→ `value * min(viewport_w, viewport_h) / 100`
    ///   - `vmax`→ `value * max(viewport_w, viewport_h) / 100`
    ///
    /// `percentage_basis_px` is the bracketing viewport axis the
    /// percentage resolves against (per SVG 2 §7.10: width attributes
    /// resolve against the viewport width, height against height,
    /// other coordinates against the diagonal; the caller picks).
    pub fn resolve(&self, ctx: ResolveContext) -> f32 {
        // Percentage-style units divide by 100 rather than multiplying
        // by 0.01 — the latter introduces a 1-ULP error per the f32
        // representation of 0.01 (`50% × 800 = 400` survives, but
        // `10% × 800` lands at 79.99999 instead of 80).
        match self.unit {
            LengthUnit::UserUnit | LengthUnit::Px => self.value,
            LengthUnit::Em => self.value * ctx.font_size_px,
            LengthUnit::Rem => self.value * ctx.root_font_size_px,
            LengthUnit::Percent => self.value * ctx.percentage_basis_px / 100.0,
            LengthUnit::Vw => self.value * ctx.viewport_w / 100.0,
            LengthUnit::Vh => self.value * ctx.viewport_h / 100.0,
            LengthUnit::Vmin => self.value * ctx.viewport_w.min(ctx.viewport_h) / 100.0,
            LengthUnit::Vmax => self.value * ctx.viewport_w.max(ctx.viewport_h) / 100.0,
            LengthUnit::Pt => self.value * (4.0 / 3.0),
            LengthUnit::Pc => self.value * 16.0,
            LengthUnit::Cm => self.value * (96.0 / 2.54),
            LengthUnit::Mm => self.value * (96.0 / 25.4),
            LengthUnit::In => self.value * 96.0,
            LengthUnit::Q => self.value * (96.0 / 25.4) * 0.25,
        }
    }
}

/// Per-resolve context carrying the unit-resolution inputs CSS Values
/// L4 §6 needs.
///
/// [`ResolveContext::default`] returns the SVG-2-spec-recommended
/// defaults: 16 px font-size for both element and root, a 0×0
/// viewport (so any `vw` / `vh` resolves to 0 — the caller usually
/// overrides this), and a 0 px percentage basis. Most callers will
/// build via [`ResolveContext::with_viewport`] /
/// [`ResolveContext::with_font_size`] etc.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolveContext {
    pub font_size_px: f32,
    pub root_font_size_px: f32,
    pub viewport_w: f32,
    pub viewport_h: f32,
    pub percentage_basis_px: f32,
}

impl Default for ResolveContext {
    fn default() -> Self {
        // CSS Values L4 §6.1.2 — 1 em / 1 rem default to 16 px when
        // no explicit `font-size` cascade has resolved.
        Self {
            font_size_px: 16.0,
            root_font_size_px: 16.0,
            viewport_w: 0.0,
            viewport_h: 0.0,
            percentage_basis_px: 0.0,
        }
    }
}

impl ResolveContext {
    /// Bind the viewport dimensions used by `vw` / `vh` / `vmin` /
    /// `vmax`. Does not touch the percentage basis (that's a separate
    /// axis-specific input — the caller knows whether the property
    /// resolves percentages against the width, height, or diagonal).
    pub fn with_viewport(mut self, w: f32, h: f32) -> Self {
        self.viewport_w = w;
        self.viewport_h = h;
        self
    }

    /// Bind the current element's resolved font-size (used by `em`).
    pub fn with_font_size(mut self, px: f32) -> Self {
        self.font_size_px = px;
        self
    }

    /// Bind the root element's resolved font-size (used by `rem`).
    pub fn with_root_font_size(mut self, px: f32) -> Self {
        self.root_font_size_px = px;
        self
    }

    /// Bind the percentage-resolution basis used by the `%` unit.
    pub fn with_percentage_basis(mut self, px: f32) -> Self {
        self.percentage_basis_px = px;
        self
    }
}

/// SVG 2 §7.10 — which viewport axis a length-percentage resolves
/// against. Used by the round-19 element-side helpers in
/// [`crate::element`] when threading a [`ResolveContext`] into the
/// shape parsers — `<rect width="50%">` resolves the `width` against
/// the viewport's *width* axis, `<rect height="50%">` against the
/// *height* axis, and other coordinates against the viewport diagonal
/// (`sqrt(w² + h²) / sqrt(2)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LengthAxis {
    /// Resolve `%` against the viewport's width.
    X,
    /// Resolve `%` against the viewport's height.
    Y,
    /// Resolve `%` against the SVG-spec "normalized diagonal" —
    /// `sqrt(w² + h²) / sqrt(2)` per SVG 2 §7.10. Used by `r` /
    /// `font-size` / `stroke-width` / etc.
    Diagonal,
}

impl ResolveContext {
    /// Convenience — return the percentage basis (in CSS px) for the
    /// given axis, derived from `viewport_w` / `viewport_h`. The
    /// result feeds straight into [`Self::with_percentage_basis`].
    ///
    /// Per SVG 2 §7.10:
    ///   - `LengthAxis::X` → viewport width
    ///   - `LengthAxis::Y` → viewport height
    ///   - `LengthAxis::Diagonal` → `sqrt(w² + h²) / sqrt(2)`
    pub fn percentage_basis_for(&self, axis: LengthAxis) -> f32 {
        match axis {
            LengthAxis::X => self.viewport_w,
            LengthAxis::Y => self.viewport_h,
            LengthAxis::Diagonal => {
                let w = self.viewport_w;
                let h = self.viewport_h;
                ((w * w + h * h).sqrt()) / std::f32::consts::SQRT_2
            }
        }
    }
}

/// Errors produced by [`parse_length`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Empty / whitespace-only input.
    Empty,
    /// The numeric prefix didn't parse as `f32`.
    BadNumber,
    /// The unit suffix isn't a CSS Values L4 unit.
    UnknownUnit,
}

/// Parse a CSS Values L4 `<length-percentage>` per §6.4.
///
/// Accepts a leading sign (`+` / `-`), an integer or decimal mantissa
/// with optional scientific exponent (the f32 grammar), and one of
/// the documented unit suffixes (case-insensitive — CSS unit suffixes
/// are ASCII-only and case-insensitive per §6.1).
///
/// Returns [`Length::user_units`] when no unit suffix is present —
/// preserving the existing parser's behaviour for bare numeric SVG
/// attributes (`<rect x="100">`).
pub fn parse_length(input: &str) -> Result<Length, ParseError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    // Find the boundary between the numeric prefix and the unit
    // suffix. Walk the char indices and pick the longest prefix that
    // parses as f32 — same logic as element::parse_number, lifted
    // into a typed return so we can keep the unit suffix.
    let bytes = s.as_bytes();
    let mut split_at: usize = 0;
    let mut best: Option<f32> = None;
    let mut i = 1;
    while i <= bytes.len() {
        let c = bytes[i - 1] as char;
        // Numeric grammar — same set as f32::from_str but stop at
        // any unit char (the 'e' in '3.5em' is ambiguous; we accept
        // it as the exponent marker only when the next char is also
        // numeric, otherwise it's the start of an `em` suffix).
        let numeric =
            c.is_ascii_digit() || c == '.' || c == '+' || c == '-' || c == 'e' || c == 'E';
        if !numeric {
            break;
        }
        if let Ok(v) = s[..i].parse::<f32>() {
            best = Some(v);
            split_at = i;
        }
        i += 1;
    }
    let value = best.ok_or(ParseError::BadNumber)?;
    let suffix = s[split_at..].trim().to_ascii_lowercase();
    let unit = match suffix.as_str() {
        "" => LengthUnit::UserUnit,
        "px" => LengthUnit::Px,
        "em" => LengthUnit::Em,
        "rem" => LengthUnit::Rem,
        "%" => LengthUnit::Percent,
        "vw" => LengthUnit::Vw,
        "vh" => LengthUnit::Vh,
        "vmin" => LengthUnit::Vmin,
        "vmax" => LengthUnit::Vmax,
        "pt" => LengthUnit::Pt,
        "pc" => LengthUnit::Pc,
        "cm" => LengthUnit::Cm,
        "mm" => LengthUnit::Mm,
        "in" => LengthUnit::In,
        "q" => LengthUnit::Q,
        _ => return Err(ParseError::UnknownUnit),
    };
    Ok(Length { value, unit })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_number_is_user_units() {
        // Existing call sites pass bare numbers — they must stay on
        // the UserUnit variant so the resolver's `value` == legacy
        // f32 parse for the same input.
        let l = parse_length("100").unwrap();
        assert_eq!(l, Length::user_units(100.0));
        assert!((l.resolve(ResolveContext::default()) - 100.0).abs() < 1e-6);
    }

    #[test]
    fn px_suffix_is_identity_in_resolve() {
        // 1 px = 1 user unit per the SVG 2 / CSS bridge.
        let l = parse_length("42px").unwrap();
        assert_eq!(l.unit, LengthUnit::Px);
        assert!((l.resolve(ResolveContext::default()) - 42.0).abs() < 1e-6);
    }

    #[test]
    fn em_resolves_against_current_font_size() {
        // 2em with font-size 24px → 48 px.
        let l = parse_length("2em").unwrap();
        assert_eq!(l.unit, LengthUnit::Em);
        let ctx = ResolveContext::default().with_font_size(24.0);
        assert!((l.resolve(ctx) - 48.0).abs() < 1e-6);
    }

    #[test]
    fn rem_resolves_against_root_font_size() {
        // 1.5rem with root font-size 20px → 30 px (independent of
        // the per-element font-size).
        let l = parse_length("1.5rem").unwrap();
        assert_eq!(l.unit, LengthUnit::Rem);
        let ctx = ResolveContext::default()
            .with_font_size(99.0)
            .with_root_font_size(20.0);
        assert!((l.resolve(ctx) - 30.0).abs() < 1e-6);
    }

    #[test]
    fn percentage_uses_basis() {
        // 50% of 200 px basis → 100 px.
        let l = parse_length("50%").unwrap();
        assert_eq!(l.unit, LengthUnit::Percent);
        let ctx = ResolveContext::default().with_percentage_basis(200.0);
        assert!((l.resolve(ctx) - 100.0).abs() < 1e-6);
    }

    #[test]
    fn vw_vh_use_viewport() {
        let l = parse_length("10vw").unwrap();
        let ctx = ResolveContext::default().with_viewport(800.0, 600.0);
        // 10vw of an 800px viewport → 80 px (within f32 precision —
        // `0.01 * 800.0 = 7.9999995` after the `* 0.01` rounding).
        let r = l.resolve(ctx);
        assert!((r - 80.0).abs() < 1e-3, "expected 80, got {r}");
        let l = parse_length("10vh").unwrap();
        // 10vh of a 600px viewport → 60 px.
        let r = l.resolve(ctx);
        assert!((r - 60.0).abs() < 1e-3, "expected 60, got {r}");
    }

    #[test]
    fn vmin_vmax_track_smaller_and_larger_axis() {
        let ctx = ResolveContext::default().with_viewport(1000.0, 500.0);
        // vmin → 1% of min(1000, 500) = 5 px per 1vmin.
        let l = parse_length("4vmin").unwrap();
        assert!((l.resolve(ctx) - 20.0).abs() < 1e-6);
        // vmax → 1% of max(1000, 500) = 10 px per 1vmax.
        let l = parse_length("4vmax").unwrap();
        assert!((l.resolve(ctx) - 40.0).abs() < 1e-6);
    }

    #[test]
    fn absolute_units_match_css_values_l4_factors() {
        // Canonical CSS Values L4 §6.1.1 conversions.
        let ctx = ResolveContext::default();
        // 1 in = 96 px.
        assert!((parse_length("1in").unwrap().resolve(ctx) - 96.0).abs() < 1e-4);
        // 1 pt = 4/3 px.
        assert!((parse_length("1pt").unwrap().resolve(ctx) - (4.0 / 3.0)).abs() < 1e-4);
        // 1 pc = 16 px.
        assert!((parse_length("1pc").unwrap().resolve(ctx) - 16.0).abs() < 1e-4);
        // 1 cm = 96/2.54 px ≈ 37.7953.
        assert!((parse_length("1cm").unwrap().resolve(ctx) - (96.0 / 2.54)).abs() < 1e-3);
        // 1 mm = 96/25.4 px ≈ 3.7795.
        assert!((parse_length("1mm").unwrap().resolve(ctx) - (96.0 / 25.4)).abs() < 1e-3);
        // 1 q = (96/25.4) * 0.25 px ≈ 0.9449.
        assert!((parse_length("1q").unwrap().resolve(ctx) - (96.0 / 25.4) * 0.25).abs() < 1e-3);
    }

    #[test]
    fn case_insensitive_suffix() {
        // CSS Values L4 §6.1 — unit suffixes are case-insensitive.
        for src in ["12PX", "12Px", "12pX", "12px"] {
            assert_eq!(parse_length(src).unwrap().unit, LengthUnit::Px);
        }
    }

    #[test]
    fn signed_numbers_round_trip() {
        let l = parse_length("-12.5px").unwrap();
        assert!((l.resolve(ResolveContext::default()) + 12.5).abs() < 1e-6);
        let l = parse_length("+5em").unwrap();
        let ctx = ResolveContext::default().with_font_size(8.0);
        assert!((l.resolve(ctx) - 40.0).abs() < 1e-6);
    }

    #[test]
    fn scientific_notation_in_value() {
        // f32::from_str accepts `1.5e2` → 150.
        let l = parse_length("1.5e2px").unwrap();
        assert!((l.resolve(ResolveContext::default()) - 150.0).abs() < 1e-4);
    }

    #[test]
    fn empty_and_invalid_inputs_error() {
        assert_eq!(parse_length(""), Err(ParseError::Empty));
        assert_eq!(parse_length("   "), Err(ParseError::Empty));
        assert_eq!(parse_length("abc"), Err(ParseError::BadNumber));
        assert_eq!(parse_length("12foo"), Err(ParseError::UnknownUnit));
    }

    #[test]
    fn em_does_not_steal_e_exponent_marker() {
        // The ambiguous `e` in `3.5em` must be parsed as the unit
        // `em`, not as an exponent (`3.5e + m...`).
        let l = parse_length("3.5em").unwrap();
        assert_eq!(l.unit, LengthUnit::Em);
        assert!((l.value - 3.5).abs() < 1e-6);
    }

    #[test]
    fn synthetic_svg_resolution_at_two_viewports() {
        // Round-18 acceptance test from the dispatch report:
        // synthetic <rect x="1em"> / <line x1="50%"> / <rect x="2vw">
        // resolved at multiple viewport sizes.
        let lx_em = parse_length("1em").unwrap();
        let lx_pct = parse_length("50%").unwrap();
        let lx_vw = parse_length("2vw").unwrap();
        // Viewport A: 100 × 200, font-size 16, percentage basis = 100.
        let ctx_a = ResolveContext::default()
            .with_viewport(100.0, 200.0)
            .with_font_size(16.0)
            .with_percentage_basis(100.0);
        assert!((lx_em.resolve(ctx_a) - 16.0).abs() < 1e-6);
        assert!((lx_pct.resolve(ctx_a) - 50.0).abs() < 1e-6);
        assert!((lx_vw.resolve(ctx_a) - 2.0).abs() < 1e-6);
        // Viewport B: 1000 × 1000, font-size 24, percentage basis = 1000.
        let ctx_b = ResolveContext::default()
            .with_viewport(1000.0, 1000.0)
            .with_font_size(24.0)
            .with_percentage_basis(1000.0);
        assert!((lx_em.resolve(ctx_b) - 24.0).abs() < 1e-6);
        assert!((lx_pct.resolve(ctx_b) - 500.0).abs() < 1e-6);
        assert!((lx_vw.resolve(ctx_b) - 20.0).abs() < 1e-6);
    }

    #[test]
    fn user_unit_default() {
        let l = Length::user_units(7.0);
        assert_eq!(l.unit, LengthUnit::UserUnit);
        assert_eq!(l, Length::new(7.0, LengthUnit::UserUnit));
    }
}
