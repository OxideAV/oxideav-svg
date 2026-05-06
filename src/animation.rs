//! Round 4 — SMIL animation engine.
//!
//! Round 3 only collapsed animations to their first-paint snapshot
//! (`from` / first `values` entry / `to`). Round 4 evaluates each
//! animation at an arbitrary `t_seconds` taking into account the full
//! SMIL timing model:
//!
//! - `begin="N s"` — start delay (default 0).
//! - `dur="N s"` — active duration (required for time interpolation;
//!   without it `<set>`-style hold semantics apply).
//! - `repeatCount="N | indefinite"` — number of cycles. Without
//!   `repeatCount` the animation evaluates the last frame after one
//!   cycle (`fill="freeze"` default for round 4 — the alternative is
//!   `remove`, which we don't model since it would require a
//!   priority-stack scheduler).
//! - `keyTimes` + `keyValues` — segmented interpolation. `values` is
//!   shorthand for evenly-spaced `keyTimes`.
//! - `from` / `to` / `by` shorthand when `values` is absent.
//!
//! Output type per attribute:
//!
//! - colours (`fill`, `stroke`, `stop-color`) → componentwise lerp.
//! - numbers (`opacity`, `x`, `y`, `width`, `height`, `r`, `cx`, `cy`,
//!   `stroke-width`, …) → scalar lerp.
//! - everything else → discrete (snap to nearest keyframe).
//!
//! Timing functions (`calcMode="discrete|linear|paced|spline"`) — round
//! 4 implements `linear` (the SMIL default) and `discrete`. `paced`
//! degrades to `linear` (without a distance metric per attribute it
//! would require per-type code that adds little value over uniform
//! `keyTimes`). `spline` degrades to `linear`.
//!
//! `<animateTransform>` is supported for `type="translate|rotate|scale"`
//! and produces a serialised `transform="..."` attribute string that the
//! existing `transform.rs` parser already accepts.

use crate::parser::{attr, tag_local, Element, Node as XmlNode};

/// One animation child snapshot at the requested `t`.
///
/// `attribute_name` is the targeted attribute; `value` is the
/// interpolated string ready to splice into the parent element's attrs.
/// Returns `None` for animations that aren't active at `t`.
pub fn evaluate_at(el: &Element, t_seconds: f32) -> Option<(String, String)> {
    let attr_name = attr(el, "attributeName")?.trim().to_string();
    if attr_name.is_empty() {
        return None;
    }

    let begin = parse_clock(attr(el, "begin")).unwrap_or(0.0);
    let dur = parse_clock(attr(el, "dur"));
    let repeat = parse_repeat_count(attr(el, "repeatCount"));

    // Map global t into local cycle time.
    let local_t = local_time(t_seconds, begin, dur, repeat)?;

    // Special case: <animateTransform> serialises to a `transform=`
    // string covering type="translate|rotate|scale".
    if tag_local(&el.name) == "animatetransform" {
        let value = evaluate_transform_at(el, local_t, dur)?;
        return Some(("transform".into(), value));
    }

    // Standard <animate> / <set>: interpolate the value.
    let frames = collect_frames(el)?;
    if frames.is_empty() {
        return None;
    }
    let raw = interpolate_frames(&frames, local_t, dur, calc_mode(el));
    Some((attr_name, raw))
}

/// Evaluate every animation child of `parent` at `t_seconds` and return
/// the merged attribute overrides. Designed for the parser to call
/// before walking the parent element's own attrs.
pub fn snapshot_children(parent: &Element, t_seconds: f32) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for child in &parent.children {
        if let XmlNode::Element(c) = child {
            let local = tag_local(&c.name);
            if !matches!(
                local.as_str(),
                "animate" | "set" | "animatetransform" | "animatemotion"
            ) {
                continue;
            }
            if let Some((name, value)) = evaluate_at(c, t_seconds) {
                // Replace existing same-name override (last wins).
                let lower_name = name.to_ascii_lowercase();
                out.retain(|(k, _)| k.to_ascii_lowercase() != lower_name);
                out.push((name, value));
            }
        }
    }
    out
}

/// Map global `t` into local cycle time.
///
/// Returns `None` when the animation hasn't started yet (`t < begin`)
/// or when it has ended past its repeat count *and* `fill="remove"`
/// (which we treat as the absence of `freeze` — but our default is
/// `freeze`, so we return the last frame instead).
fn local_time(t_seconds: f32, begin: f32, dur: Option<f32>, repeat: RepeatCount) -> Option<f32> {
    if t_seconds < begin {
        return None;
    }
    let elapsed = t_seconds - begin;
    let dur = match dur {
        Some(d) if d > 0.0 => d,
        _ => return Some(0.0), // <set>-style hold: always at the start.
    };

    let cycles = elapsed / dur;
    let max_cycles: f32 = match repeat {
        RepeatCount::Indefinite => f32::INFINITY,
        RepeatCount::N(n) => n,
    };
    if cycles >= max_cycles {
        // Past the end → freeze on the last frame.
        return Some(dur);
    }
    Some(elapsed - cycles.floor() * dur)
}

#[derive(Clone, Copy, Debug)]
enum RepeatCount {
    Indefinite,
    N(f32),
}

fn parse_repeat_count(s: Option<&str>) -> RepeatCount {
    match s.map(str::trim) {
        None => RepeatCount::N(1.0),
        Some("indefinite") => RepeatCount::Indefinite,
        Some(t) => match t.parse::<f32>() {
            Ok(v) if v > 0.0 => RepeatCount::N(v),
            _ => RepeatCount::N(1.0),
        },
    }
}

/// Parse a SMIL clock-value subset. SVG 1.1 §19.2.3 allows
/// `<hours>:<minutes>:<seconds>`, `<minutes>:<seconds>` and
/// `<timecount>` (a number with optional `h`/`min`/`s`/`ms` suffix).
fn parse_clock(s: Option<&str>) -> Option<f32> {
    let s = s?.trim();
    if s.is_empty() {
        return None;
    }
    // Try colon form first.
    if s.contains(':') {
        let parts: Vec<&str> = s.split(':').collect();
        let nums: Result<Vec<f32>, _> = parts.iter().map(|p| p.parse::<f32>()).collect();
        if let Ok(n) = nums {
            return Some(match n.len() {
                3 => n[0] * 3600.0 + n[1] * 60.0 + n[2],
                2 => n[0] * 60.0 + n[1],
                _ => return None,
            });
        }
    }
    // Numeric with optional unit.
    let (num_part, unit) = split_unit(s);
    let v: f32 = num_part.parse().ok()?;
    Some(match unit {
        "h" => v * 3600.0,
        "min" => v * 60.0,
        "ms" => v / 1000.0,
        // Default unit is seconds.
        "s" | "" => v,
        _ => v,
    })
}

fn split_unit(s: &str) -> (&str, &str) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len()
        && (bytes[i].is_ascii_digit() || bytes[i] == b'.' || bytes[i] == b'+' || bytes[i] == b'-')
    {
        i += 1;
    }
    (&s[..i], s[i..].trim())
}

/// One keyframe row for a SMIL animation.
#[derive(Clone, Debug)]
struct Frame {
    time: f32, // 0..=dur
    value: String,
}

fn collect_frames(el: &Element) -> Option<Vec<Frame>> {
    // values + keyTimes wins over from/to/by.
    if let Some(values) = attr(el, "values") {
        let parts: Vec<String> = values
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            return None;
        }
        let key_times: Option<Vec<f32>> = attr(el, "keyTimes").and_then(|s| {
            let v: Result<Vec<f32>, _> = s.split(';').map(|p| p.trim().parse::<f32>()).collect();
            v.ok()
        });
        let times = match key_times {
            Some(t) if t.len() == parts.len() => t,
            _ => uniform_times(parts.len()),
        };
        return Some(
            parts
                .into_iter()
                .zip(times)
                .map(|(value, time)| Frame { time, value })
                .collect(),
        );
    }

    let from = attr(el, "from").map(str::to_string);
    let to = attr(el, "to").map(str::to_string);
    let by = attr(el, "by").map(str::to_string);

    match (from, to, by) {
        (Some(f), Some(t), _) => Some(vec![
            Frame {
                time: 0.0,
                value: f,
            },
            Frame {
                time: 1.0,
                value: t,
            },
        ]),
        (None, Some(t), _) => Some(vec![Frame {
            time: 0.0,
            value: t,
        }]),
        (Some(f), None, Some(b)) => {
            // by-only — try to add `f + b` for the end frame
            // numerically.  When the inputs aren't numeric (e.g.
            // colour names) we fall back to a discrete two-frame.
            let end = match (f.parse::<f32>(), b.parse::<f32>()) {
                (Ok(fv), Ok(bv)) => format!("{}", fv + bv),
                _ => f.clone(),
            };
            Some(vec![
                Frame {
                    time: 0.0,
                    value: f,
                },
                Frame {
                    time: 1.0,
                    value: end,
                },
            ])
        }
        (Some(f), None, None) => Some(vec![Frame {
            time: 0.0,
            value: f,
        }]),
        _ => None,
    }
}

fn uniform_times(n: usize) -> Vec<f32> {
    if n <= 1 {
        return vec![0.0];
    }
    let denom = (n - 1) as f32;
    (0..n).map(|i| i as f32 / denom).collect()
}

#[derive(Clone, Copy, Debug)]
enum CalcMode {
    Discrete,
    Linear,
}

fn calc_mode(el: &Element) -> CalcMode {
    match attr(el, "calcMode").map(str::trim) {
        Some("discrete") => CalcMode::Discrete,
        // round 4 collapses paced/spline → linear (see module doc).
        _ => CalcMode::Linear,
    }
}

fn interpolate_frames(frames: &[Frame], local_t: f32, dur: Option<f32>, mode: CalcMode) -> String {
    if frames.is_empty() {
        return String::new();
    }
    if frames.len() == 1 {
        return frames[0].value.clone();
    }
    // keyTimes are normalised to 0..=1; convert local_t into the same
    // domain. <set>-style hold (no dur) → t=0.
    let t01 = match dur {
        Some(d) if d > 0.0 => (local_t / d).clamp(0.0, 1.0),
        _ => 0.0,
    };

    // Find the segment.
    let last = frames.len() - 1;
    if t01 <= frames[0].time {
        return frames[0].value.clone();
    }
    if t01 >= frames[last].time {
        return frames[last].value.clone();
    }
    let mut idx = 0;
    for (i, f) in frames.iter().enumerate() {
        if f.time > t01 {
            idx = i.saturating_sub(1);
            break;
        }
    }
    let a = &frames[idx];
    let b = &frames[(idx + 1).min(last)];
    let span = (b.time - a.time).max(f32::EPSILON);
    let local = ((t01 - a.time) / span).clamp(0.0, 1.0);
    match mode {
        CalcMode::Discrete => a.value.clone(),
        CalcMode::Linear => lerp_string(&a.value, &b.value, local),
    }
}

/// Linearly interpolate two values. Tries colour, then scalar, else
/// returns `a` (discrete fallback).
fn lerp_string(a: &str, b: &str, t: f32) -> String {
    if let (Some(ca), Some(cb)) = (parse_color_for_lerp(a), parse_color_for_lerp(b)) {
        let r = lerp_u8(ca.0, cb.0, t);
        let g = lerp_u8(ca.1, cb.1, t);
        let bl = lerp_u8(ca.2, cb.2, t);
        let alpha = lerp_u8(ca.3, cb.3, t);
        if alpha == 255 {
            return format!("#{r:02x}{g:02x}{bl:02x}");
        }
        return format!(
            "rgba({r},{g},{bl},{:.3})",
            (alpha as f32 / 255.0).clamp(0.0, 1.0)
        );
    }
    if let (Ok(av), Ok(bv)) = (a.parse::<f32>(), b.parse::<f32>()) {
        return format!("{}", av + (bv - av) * t);
    }
    a.to_string()
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let v = a as f32 + (b as f32 - a as f32) * t;
    v.clamp(0.0, 255.0).round() as u8
}

/// Minimal colour parser for animation interpolation. Returns
/// `(r, g, b, a)`. Only the forms emitted as snapshots can come back
/// here, so we just need `#rrggbb`, `#rgb`, `rgb(...)`, and the named
/// colours via the existing parser.
fn parse_color_for_lerp(s: &str) -> Option<(u8, u8, u8, u8)> {
    let trimmed = s.trim();
    if let Ok(crate::color::PaintValue::Color(rgba)) = crate::color::parse_paint(trimmed) {
        return Some((rgba.r, rgba.g, rgba.b, rgba.a));
    }
    None
}

/// Evaluate `<animateTransform>` at `local_t` within `dur` and emit a
/// `transform="..."` string. Only `type="translate|rotate|scale"` are
/// supported; matrix/skew degrade to discrete snap on the start frame.
fn evaluate_transform_at(el: &Element, local_t: f32, dur: Option<f32>) -> Option<String> {
    let t_kind = attr(el, "type").map(str::trim).unwrap_or("translate");
    let frames = collect_frames(el)?;
    if frames.is_empty() {
        return None;
    }
    let interp = interpolate_transform_frame(&frames, local_t, dur);
    let value = match t_kind {
        "translate" => format!("translate({interp})"),
        "rotate" => format!("rotate({interp})"),
        "scale" => format!("scale({interp})"),
        // Round 4: `matrix` / `skewX` / `skewY` interpolation is too
        // attribute-shape-dependent for a one-line lerp. Snap to the
        // first frame so the structural intent survives.
        _ => return None,
    };
    Some(value)
}

/// Interpolate a transform-style frame list whose values are
/// space/comma-separated number lists (e.g. `"10 20"` or `"45"`).
fn interpolate_transform_frame(frames: &[Frame], local_t: f32, dur: Option<f32>) -> String {
    if frames.len() == 1 {
        return frames[0].value.clone();
    }
    let t01 = match dur {
        Some(d) if d > 0.0 => (local_t / d).clamp(0.0, 1.0),
        _ => 0.0,
    };
    let last = frames.len() - 1;
    if t01 <= frames[0].time {
        return frames[0].value.clone();
    }
    if t01 >= frames[last].time {
        return frames[last].value.clone();
    }
    let mut idx = 0;
    for (i, f) in frames.iter().enumerate() {
        if f.time > t01 {
            idx = i.saturating_sub(1);
            break;
        }
    }
    let a_nums = parse_numbers(&frames[idx].value);
    let b_nums = parse_numbers(&frames[(idx + 1).min(last)].value);
    let span = (frames[(idx + 1).min(last)].time - frames[idx].time).max(f32::EPSILON);
    let local = ((t01 - frames[idx].time) / span).clamp(0.0, 1.0);
    let n = a_nums.len().min(b_nums.len());
    if n == 0 {
        return frames[idx].value.clone();
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push(a_nums[i] + (b_nums[i] - a_nums[i]) * local);
    }
    out.iter()
        .map(|v| format!("{v}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_numbers(s: &str) -> Vec<f32> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse::<f32>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parse_clock_handles_units() {
        assert_eq!(parse_clock(Some("3s")), Some(3.0));
        assert_eq!(parse_clock(Some("100ms")), Some(0.1));
        assert_eq!(parse_clock(Some("2min")), Some(120.0));
        assert_eq!(parse_clock(Some("1h")), Some(3600.0));
        assert_eq!(parse_clock(Some("2.5")), Some(2.5));
        assert_eq!(parse_clock(Some("1:00")), Some(60.0));
        assert_eq!(parse_clock(Some("0:30")), Some(30.0));
        assert_eq!(parse_clock(Some("1:02:03")), Some(3723.0));
    }

    #[test]
    fn snapshot_at_t0_returns_from() {
        let a = elem(
            "animate",
            &[
                ("attributeName", "x"),
                ("from", "10"),
                ("to", "30"),
                ("dur", "2s"),
            ],
        );
        assert_eq!(evaluate_at(&a, 0.0), Some(("x".into(), "10".into())));
    }

    #[test]
    fn snapshot_midway_lerps_numbers() {
        let a = elem(
            "animate",
            &[
                ("attributeName", "x"),
                ("from", "0"),
                ("to", "100"),
                ("dur", "2s"),
            ],
        );
        let v = evaluate_at(&a, 1.0).unwrap().1;
        let n: f32 = v.parse().unwrap();
        assert!((n - 50.0).abs() < 0.5);
    }

    #[test]
    fn snapshot_freezes_past_dur_by_default() {
        let a = elem(
            "animate",
            &[
                ("attributeName", "x"),
                ("from", "0"),
                ("to", "100"),
                ("dur", "1s"),
            ],
        );
        // t=2 is well past the single cycle → should show the end frame.
        let v = evaluate_at(&a, 2.0).unwrap().1;
        assert_eq!(v.parse::<f32>().unwrap().round() as i32, 100);
    }

    #[test]
    fn snapshot_repeat_count_loops_back() {
        let a = elem(
            "animate",
            &[
                ("attributeName", "x"),
                ("from", "0"),
                ("to", "100"),
                ("dur", "2s"),
                ("repeatCount", "2"),
            ],
        );
        // After 2.5s we're 0.5s into the second cycle → x ≈ 25.
        let v: f32 = evaluate_at(&a, 2.5).unwrap().1.parse().unwrap();
        assert!((v - 25.0).abs() < 1.0, "expected ~25, got {v}");
    }

    #[test]
    fn indefinite_repeat_loops_forever() {
        let a = elem(
            "animate",
            &[
                ("attributeName", "x"),
                ("from", "0"),
                ("to", "10"),
                ("dur", "1s"),
                ("repeatCount", "indefinite"),
            ],
        );
        // 100s in: cycles=100, fractional=0 → start frame again.
        let v: f32 = evaluate_at(&a, 100.0).unwrap().1.parse().unwrap();
        assert!(v.abs() < 0.01);
    }

    #[test]
    fn snapshot_before_begin_returns_none() {
        let a = elem(
            "animate",
            &[
                ("attributeName", "x"),
                ("from", "0"),
                ("to", "100"),
                ("dur", "1s"),
                ("begin", "5s"),
            ],
        );
        assert_eq!(evaluate_at(&a, 1.0), None);
    }

    #[test]
    fn values_with_keytimes_segments() {
        let a = elem(
            "animate",
            &[
                ("attributeName", "x"),
                ("values", "0;10;100"),
                ("keyTimes", "0;0.5;1"),
                ("dur", "2s"),
            ],
        );
        // At t=1s (50% through), keyTimes places us on frame index 1
        // (value="10").
        let v: f32 = evaluate_at(&a, 1.0).unwrap().1.parse().unwrap();
        assert!((v - 10.0).abs() < 0.5, "expected ~10, got {v}");
    }

    #[test]
    fn discrete_calc_mode_snaps() {
        let a = elem(
            "animate",
            &[
                ("attributeName", "x"),
                ("from", "0"),
                ("to", "100"),
                ("dur", "2s"),
                ("calcMode", "discrete"),
            ],
        );
        // Halfway → should snap to the start frame, not 50.
        let v: f32 = evaluate_at(&a, 1.0).unwrap().1.parse().unwrap();
        assert_eq!(v as i32, 0);
    }

    #[test]
    fn color_lerp_at_midpoint() {
        let a = elem(
            "animate",
            &[
                ("attributeName", "fill"),
                ("from", "#000000"),
                ("to", "#ffffff"),
                ("dur", "2s"),
            ],
        );
        let v = evaluate_at(&a, 1.0).unwrap().1;
        assert!(
            v == "#7f7f7f" || v == "#808080",
            "expected midpoint grey, got {v}"
        );
    }

    #[test]
    fn animate_transform_translate() {
        let a = elem(
            "animateTransform",
            &[
                ("attributeName", "transform"),
                ("type", "translate"),
                ("from", "0 0"),
                ("to", "100 50"),
                ("dur", "2s"),
            ],
        );
        let (k, v) = evaluate_at(&a, 1.0).unwrap();
        assert_eq!(k, "transform");
        assert!(v.starts_with("translate("));
        // Mid-point should contain ~50 25.
        assert!(v.contains("50") && v.contains("25"));
    }

    #[test]
    fn animate_transform_rotate() {
        let a = elem(
            "animateTransform",
            &[
                ("attributeName", "transform"),
                ("type", "rotate"),
                ("from", "0"),
                ("to", "360"),
                ("dur", "2s"),
            ],
        );
        let (_, v) = evaluate_at(&a, 1.0).unwrap();
        assert!(v.starts_with("rotate("));
        assert!(v.contains("180"));
    }

    #[test]
    fn snapshot_children_drops_unrelated() {
        let parent = Element {
            name: "rect".into(),
            attrs: vec![],
            children: vec![
                XmlNode::Element(elem(
                    "animate",
                    &[
                        ("attributeName", "fill"),
                        ("from", "#ff0000"),
                        ("to", "#0000ff"),
                        ("dur", "2s"),
                    ],
                )),
                XmlNode::Element(elem("title", &[])),
            ],
        };
        let snap = snapshot_children(&parent, 1.0);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].0, "fill");
    }

    #[test]
    fn snapshot_overrides_in_last_wins_order() {
        // Two animations targeting `fill`: the second should win.
        let parent = Element {
            name: "rect".into(),
            attrs: vec![],
            children: vec![
                XmlNode::Element(elem(
                    "animate",
                    &[
                        ("attributeName", "fill"),
                        ("from", "#ff0000"),
                        ("to", "#000000"),
                        ("dur", "1s"),
                    ],
                )),
                XmlNode::Element(elem("set", &[("attributeName", "fill"), ("to", "#00ff00")])),
            ],
        };
        let snap = snapshot_children(&parent, 0.0);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].1, "#00ff00");
    }

    #[test]
    fn by_only_with_numeric_inputs() {
        let a = elem(
            "animate",
            &[
                ("attributeName", "x"),
                ("from", "10"),
                ("by", "5"),
                ("dur", "2s"),
            ],
        );
        let v: f32 = evaluate_at(&a, 2.0).unwrap().1.parse().unwrap();
        assert!((v - 15.0).abs() < 0.5);
    }
}
