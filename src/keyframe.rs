//! Round 16 — CSS Animations L1 keyframe evaluation at a runtime
//! `t_seconds`.
//!
//! Round 15 captured `@keyframes` blocks into
//! [`crate::css::Stylesheet::keyframes`] but never evaluated them
//! against the current animation timeline. Round 16 closes that gap:
//! given an element whose effective CSS declarations include
//! `animation-name: <kf>` + `animation-duration: <s>`, this module
//! looks up the matching [`crate::css::KeyframesRule`], computes the
//! active selector pair bracketing the normalised time
//! `t / duration % 1.0`, lerps each keyframe property between the
//! `from` + `to` declarations, and returns the merged property
//! overrides ready to splice into the element's effective property
//! map.
//!
//! Supported animation longhand declarations (per CSS Animations L1):
//!
//! - `animation-name` (single name; comma-separated lists pick the
//!   first entry — round-17 candidate to honour multiple).
//! - `animation-duration` (one value, in `s` or `ms`).
//! - `animation-iteration-count` (numeric or `infinite`; defaults to
//!   `1`).
//! - `animation-delay` (single value, in `s` or `ms`; defaults to `0`).
//!
//! Lerp coverage:
//!
//! - `transform: rotate(<deg>)` — angle interpolation.
//! - `transform: translate(<x> [<y>])` / `translateX` / `translateY` —
//!   per-component scalar interpolation.
//! - `transform: scale(<sx> [<sy>])` — per-component scalar.
//! - `opacity`, `fill-opacity`, `stroke-opacity`, `stroke-width` —
//!   scalar interpolation.
//! - `fill`, `stroke`, `stop-color` — colour interpolation via the
//!   shared `crate::color` parser (same rules as SMIL `<animate>` per
//!   `crate::animation::lerp_string`).
//! - everything else — discrete (snap to the `from` keyframe).

use crate::css::{declarations_for, KeyframesRule, MatchContext, Stylesheet};

/// Evaluate every `animation-name`-declared keyframe animation on the
/// element identified by `mctx` at the runtime `t_seconds`, and return
/// the resulting property overrides in source order.
///
/// Returns an empty vector when the element has no `animation-name` or
/// the named `@keyframes` rule isn't present in `sheet.keyframes`.
pub fn evaluate_at(
    mctx: &MatchContext<'_>,
    sheet: &Stylesheet,
    t_seconds: f32,
) -> Vec<(String, String)> {
    let decls = declarations_for(mctx, sheet);
    let kf_name = match find_decl(&decls, "animation-name") {
        Some(v) => first_csv(v),
        None => return Vec::new(),
    };
    if kf_name.is_empty() || kf_name.eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    let dur = parse_clock(find_decl(&decls, "animation-duration")).unwrap_or(0.0);
    if dur <= 0.0 {
        // Without a duration the animation is dormant per §3.
        return Vec::new();
    }
    let delay = parse_clock(find_decl(&decls, "animation-delay")).unwrap_or(0.0);
    let iter_count = parse_iter_count(find_decl(&decls, "animation-iteration-count"));

    // Find the matching @keyframes rule. Names are compared case-
    // insensitively per §3.
    let rule = match sheet
        .keyframes
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case(kf_name))
    {
        Some(r) => r,
        None => return Vec::new(),
    };

    // Map global t into the active normalised position [0, 1].
    let t01 = match normalised_position(t_seconds, delay, dur, iter_count) {
        Some(t) => t,
        None => return Vec::new(),
    };

    interpolate_rule(rule, t01)
}

/// Find the bracketing `(from, to)` selector pair around `t01` and
/// lerp each property the pair declares.
fn interpolate_rule(rule: &KeyframesRule, t01: f32) -> Vec<(String, String)> {
    if rule.selectors.is_empty() {
        return Vec::new();
    }
    // Sort indices by normalised offset so source-order doesn't trip
    // the bracketing logic. We keep the source-order entries intact
    // (so equal-offset keyframes still resolve to "last one wins"
    // declaration-wise per the spec's `from = 0%` rule).
    let mut sorted: Vec<usize> = (0..rule.selectors.len()).collect();
    sorted.sort_by(|&a, &b| {
        rule.selectors[a]
            .offset
            .as_normalised()
            .partial_cmp(&rule.selectors[b].offset.as_normalised())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Locate the segment.
    let last_sorted_idx = sorted.len() - 1;
    let first_offset = rule.selectors[sorted[0]].offset.as_normalised();
    let last_offset = rule.selectors[sorted[last_sorted_idx]]
        .offset
        .as_normalised();
    if t01 <= first_offset {
        return rule.selectors[sorted[0]].declarations.clone();
    }
    if t01 >= last_offset {
        return rule.selectors[sorted[last_sorted_idx]].declarations.clone();
    }
    // Find the first sorted index whose offset is strictly greater
    // than `t01`; the segment is [prev, this].
    let mut hi = last_sorted_idx;
    for (i, &si) in sorted.iter().enumerate() {
        if rule.selectors[si].offset.as_normalised() > t01 {
            hi = i;
            break;
        }
    }
    let lo = hi.saturating_sub(1);
    let from = &rule.selectors[sorted[lo]];
    let to = &rule.selectors[sorted[hi]];
    let span = (to.offset.as_normalised() - from.offset.as_normalised()).max(f32::EPSILON);
    let local = ((t01 - from.offset.as_normalised()) / span).clamp(0.0, 1.0);

    // Build a property map from the `from` keyframe; for each property
    // declared on `to`, lerp against the `from` value (or use `to`'s
    // value verbatim if `from` doesn't declare the same property —
    // matches the spec's "implicit keyframe" semantics on a per-
    // property basis).
    let mut out: Vec<(String, String)> = Vec::with_capacity(from.declarations.len());
    for (name, from_val) in &from.declarations {
        let to_val = find_decl(&to.declarations, name).unwrap_or(from_val.as_str());
        out.push((name.clone(), lerp_property(name, from_val, to_val, local)));
    }
    // Properties in `to` that aren't in `from` — append with discrete
    // snap to `to` (since `from` doesn't carry a starting value to
    // lerp from).
    for (name, to_val) in &to.declarations {
        if find_decl(&from.declarations, name).is_none() {
            out.push((name.clone(), to_val.clone()));
        }
    }
    out
}

/// Lerp one CSS property between two keyframe values. Falls back to
/// the existing `crate::animation::lerp_string` helper for scalars and
/// colours; transforms get a dedicated path so each component
/// (`rotate`, `translate`, `scale`) interpolates correctly.
fn lerp_property(name: &str, a: &str, b: &str, t: f32) -> String {
    let lower = name.to_ascii_lowercase();
    if lower == "transform" {
        if let Some(out) = lerp_transform(a, b, t) {
            return out;
        }
        // Fallback to discrete snap.
        return if t < 0.5 {
            a.to_string()
        } else {
            b.to_string()
        };
    }
    crate::animation::lerp_string_public(a, b, t)
}

/// Lerp a CSS `transform: <fn>(...)` value. Returns `None` when the
/// two strings aren't structurally compatible (different functions,
/// argument count mismatch, etc.); the caller falls back to a
/// discrete snap.
fn lerp_transform(a: &str, b: &str, t: f32) -> Option<String> {
    let (an, aargs) = split_transform(a)?;
    let (bn, bargs) = split_transform(b)?;
    if !an.eq_ignore_ascii_case(&bn) {
        return None;
    }
    let n = aargs.len().min(bargs.len());
    if n == 0 {
        return None;
    }
    let mut interp = Vec::with_capacity(n);
    for i in 0..n {
        let av = parse_transform_number(&aargs[i])?;
        let bv = parse_transform_number(&bargs[i])?;
        let v = av + (bv - av) * t;
        // Preserve the unit suffix from the `to` value when present
        // (e.g. `rotate(360deg)` should stay in degrees, not lose the
        // suffix mid-animation).
        let unit = unit_suffix(&bargs[i]).unwrap_or_else(|| unit_suffix(&aargs[i]).unwrap_or(""));
        interp.push(format!("{v}{unit}"));
    }
    Some(format!("{}({})", an, interp.join(" ")))
}

fn split_transform(s: &str) -> Option<(String, Vec<String>)> {
    let s = s.trim();
    let open = s.find('(')?;
    let close = s.rfind(')')?;
    if close <= open {
        return None;
    }
    let name = s[..open].trim().to_string();
    let inner = &s[open + 1..close];
    let args: Vec<String> = inner
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect();
    Some((name, args))
}

fn parse_transform_number(s: &str) -> Option<f32> {
    let s = s.trim();
    let bytes = s.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let mut saw_digit = false;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
        saw_digit = true;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
            saw_digit = true;
        }
    }
    if !saw_digit {
        return None;
    }
    s[..i].parse::<f32>().ok()
}

fn unit_suffix(s: &str) -> Option<&str> {
    let s = s.trim();
    let bytes = s.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    let unit = &s[i..];
    if unit.is_empty() {
        None
    } else {
        Some(unit)
    }
}

fn find_decl<'a>(decls: &'a [(String, String)], name: &str) -> Option<&'a str> {
    decls
        .iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn first_csv(v: &str) -> &str {
    match v.find(',') {
        Some(c) => v[..c].trim(),
        None => v.trim(),
    }
}

fn parse_clock(s: Option<&str>) -> Option<f32> {
    let s = s?.trim();
    if s.is_empty() {
        return None;
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let mut saw_digit = false;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
        saw_digit = true;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
            saw_digit = true;
        }
    }
    if !saw_digit {
        return None;
    }
    let v: f32 = s[..i].parse().ok()?;
    let unit = s[i..].trim().to_ascii_lowercase();
    Some(match unit.as_str() {
        "ms" => v / 1000.0,
        "s" | "" => v,
        _ => v,
    })
}

fn parse_iter_count(s: Option<&str>) -> f32 {
    let s = match s {
        Some(s) => s.trim(),
        None => return 1.0,
    };
    if s.eq_ignore_ascii_case("infinite") {
        return f32::INFINITY;
    }
    s.parse::<f32>().ok().filter(|v| *v > 0.0).unwrap_or(1.0)
}

/// Compute the normalised position `[0, 1]` along the keyframe
/// timeline for `t_seconds`, accounting for `delay` + `duration` +
/// `iter_count`. Returns `None` when the animation hasn't started yet
/// (`t < delay`); past the iteration count the animation freezes on
/// the final keyframe (`Some(1.0)`) per the `forwards` fill-mode
/// default we apply (`none` would return `None` here — round-17
/// candidate).
fn normalised_position(t_seconds: f32, delay: f32, dur: f32, iter_count: f32) -> Option<f32> {
    if t_seconds < delay {
        return None;
    }
    let elapsed = t_seconds - delay;
    let cycles = elapsed / dur;
    if cycles >= iter_count {
        return Some(1.0);
    }
    Some((cycles - cycles.floor()).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::Stylesheet;
    use crate::parser::Element;

    fn elem(name: &str, attrs: &[(&str, &str)]) -> Element {
        Element {
            name: name.into(),
            attrs: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            children: Vec::new(),
        }
    }

    #[test]
    fn no_animation_name_returns_empty() {
        let mut s = Stylesheet::new();
        s.parse_block(
            "@keyframes spin { from { transform: rotate(0) } to { transform: rotate(360deg) } }",
        );
        let el = elem("rect", &[]);
        let mctx = MatchContext::root(&el);
        assert!(evaluate_at(&mctx, &s, 0.5).is_empty());
    }

    #[test]
    fn rotate_at_midpoint_lerps_to_180() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @keyframes spin {
                from { transform: rotate(0deg) }
                to { transform: rotate(360deg) }
            }
            "#,
        );
        let el = elem(
            "g",
            &[("style", "animation-name: spin; animation-duration: 1s")],
        );
        let mctx = MatchContext::root(&el);
        let snap = evaluate_at(&mctx, &s, 0.5);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].0, "transform");
        // Should be roughly rotate(180deg).
        assert!(
            snap[0].1.starts_with("rotate("),
            "expected rotate(...), got {:?}",
            snap[0].1
        );
        assert!(
            snap[0].1.contains("180"),
            "expected midpoint 180, got {:?}",
            snap[0].1
        );
    }

    #[test]
    fn parse_clock_handles_units() {
        assert!((parse_clock(Some("2s")).unwrap() - 2.0).abs() < 1e-6);
        assert!((parse_clock(Some("250ms")).unwrap() - 0.25).abs() < 1e-6);
        assert!(parse_clock(None).is_none());
        assert!(parse_clock(Some("")).is_none());
    }

    #[test]
    fn parse_iter_count_infinite_returns_inf() {
        assert!(parse_iter_count(Some("infinite")).is_infinite());
        assert_eq!(parse_iter_count(Some("3")), 3.0);
        assert_eq!(parse_iter_count(None), 1.0);
    }

    #[test]
    fn missing_at_keyframes_returns_empty() {
        let mut s = Stylesheet::new();
        // No @keyframes "spin" declared.
        s.parse_block("rect { fill: red }");
        let el = elem(
            "g",
            &[("style", "animation-name: spin; animation-duration: 1s")],
        );
        let mctx = MatchContext::root(&el);
        assert!(evaluate_at(&mctx, &s, 0.5).is_empty());
    }

    #[test]
    fn opacity_lerps_at_quarter_point() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @keyframes fade {
                from { opacity: 0 }
                to { opacity: 1 }
            }
            "#,
        );
        let el = elem(
            "g",
            &[("style", "animation-name: fade; animation-duration: 4s")],
        );
        let mctx = MatchContext::root(&el);
        // t=1s of a 4s animation → 25%.
        let snap = evaluate_at(&mctx, &s, 1.0);
        let val: f32 = snap[0].1.parse().unwrap();
        assert!((val - 0.25).abs() < 1e-3, "expected 0.25, got {val}");
    }

    #[test]
    fn looped_animation_wraps_at_iteration_boundary() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @keyframes spin {
                from { transform: rotate(0deg) }
                to { transform: rotate(360deg) }
            }
            "#,
        );
        let el = elem(
            "g",
            &[(
                "style",
                "animation-name: spin; animation-duration: 1s; animation-iteration-count: infinite",
            )],
        );
        let mctx = MatchContext::root(&el);
        // t=2.5s of a 1s indefinite → into the third cycle at 0.5 →
        // 180deg.
        let snap = evaluate_at(&mctx, &s, 2.5);
        assert!(snap[0].1.contains("180"));
    }

    #[test]
    fn delay_skips_evaluation_when_t_below_delay() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @keyframes fade {
                from { opacity: 0 }
                to { opacity: 1 }
            }
            "#,
        );
        let el = elem(
            "g",
            &[(
                "style",
                "animation-name: fade; animation-duration: 1s; animation-delay: 2s",
            )],
        );
        let mctx = MatchContext::root(&el);
        assert!(evaluate_at(&mctx, &s, 1.0).is_empty(), "delay not honoured");
        // After the delay → starts at 0.
        let snap = evaluate_at(&mctx, &s, 2.0);
        let val: f32 = snap[0].1.parse().unwrap();
        assert!(val.abs() < 1e-3);
    }

    #[test]
    fn percent_offsets_lerp_within_segment() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @keyframes pulse {
                0% { opacity: 0 }
                50% { opacity: 1 }
                100% { opacity: 0 }
            }
            "#,
        );
        let el = elem(
            "g",
            &[("style", "animation-name: pulse; animation-duration: 1s")],
        );
        let mctx = MatchContext::root(&el);
        // At t=0.25 → midway through 0%→50% → opacity 0.5.
        let snap = evaluate_at(&mctx, &s, 0.25);
        let val: f32 = snap[0].1.parse().unwrap();
        assert!((val - 0.5).abs() < 1e-3, "expected 0.5, got {val}");
    }
}
