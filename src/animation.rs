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
//! 4 shipped `linear` (the SMIL default) and `discrete`; round 7 fills
//! in `paced` and `spline`.
//!
//! - `paced` redistributes `keyTimes` so each segment is traversed at
//!   constant attribute-space speed. Numeric values (and colour values
//!   in 4-component RGBA space) get a real distance metric; non-numeric
//!   values fall back to uniform spacing.
//! - `spline` reads `keySplines="x1 y1 x2 y2 ; ..."` (one quadruple
//!   per segment) and remaps the per-segment local `t` through the
//!   cubic Bézier `(0,0)→(x1,y1)→(x2,y2)→(1,1)`.  Resolved with a few
//!   Newton-Raphson iterations on the x curve to invert `x(s)→s`,
//!   then `y(s)` gives the eased fraction.
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
    let mut frames = collect_frames(el)?;
    if frames.is_empty() {
        return None;
    }
    let mode = calc_mode(el);
    if matches!(mode, CalcMode::Paced) {
        repace_frames(&mut frames);
    }
    let key_splines = if matches!(mode, CalcMode::Spline) {
        parse_key_splines(attr(el, "keySplines"), frames.len().saturating_sub(1))
    } else {
        None
    };
    let raw = interpolate_frames(&frames, local_t, dur, mode, key_splines.as_deref());
    Some((attr_name, raw))
}

/// Evaluate every animation child of `parent` at `t_seconds` and return
/// the merged attribute overrides. Designed for the parser to call
/// before walking the parent element's own attrs.
///
/// Round 125: this is the legacy entry-point. `<animateMotion>`
/// instances that carry an `<mpath xlink:href="#id">` child resolve
/// **only** if the caller routes the call through
/// [`snapshot_children_with_resolver`] and supplies an id lookup; a
/// bare [`snapshot_children`] call falls back to evaluating the
/// motion element's own `path=` / `values=` / `from`-`to`-`by`
/// attributes (per the §19.2.14 override precedence: mpath overrides
/// path overrides values overrides from/by/to).
pub fn snapshot_children(parent: &Element, t_seconds: f32) -> Vec<(String, String)> {
    snapshot_children_with_resolver(parent, t_seconds, &|_| None)
}

/// Round 125 — variant of [`snapshot_children`] that wires an id
/// lookup into `<animateMotion>` evaluation so an `<mpath
/// xlink:href="#path1">` child can resolve to the referenced
/// `<path>` element's `d` attribute per SVG 1.1 §19.2.14.
///
/// Pass a closure that returns the source [`Element`] for a bare id
/// (no leading `#`). [`crate::element::ParseContext::defs::elements`]
/// is the natural source: it's the id-to-element table the rest of
/// the decoder already populates during the pre-walk for `<use>` /
/// `<linearGradient href>` etc.
pub fn snapshot_children_with_resolver<'a>(
    parent: &Element,
    t_seconds: f32,
    id_lookup: &dyn Fn(&str) -> Option<&'a Element>,
) -> Vec<(String, String)> {
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
            let evaluated = if local == "animatemotion" {
                evaluate_motion_at(c, t_seconds, id_lookup).map(|v| ("transform".into(), v))
            } else {
                evaluate_at(c, t_seconds)
            };
            if let Some((name, value)) = evaluated {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CalcMode {
    Discrete,
    Linear,
    /// `calcMode="paced"` — redistributes keyTimes by attribute-space
    /// distance so segments traverse at constant speed. Round 7.
    Paced,
    /// `calcMode="spline"` — eases each segment through a cubic Bézier
    /// from `keySplines`. Round 7.
    Spline,
}

fn calc_mode(el: &Element) -> CalcMode {
    match attr(el, "calcMode").map(str::trim) {
        Some("discrete") => CalcMode::Discrete,
        Some("paced") => CalcMode::Paced,
        Some("spline") => CalcMode::Spline,
        _ => CalcMode::Linear,
    }
}

fn interpolate_frames(
    frames: &[Frame],
    local_t: f32,
    dur: Option<f32>,
    mode: CalcMode,
    key_splines: Option<&[KeySpline]>,
) -> String {
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
        CalcMode::Linear | CalcMode::Paced => lerp_string(&a.value, &b.value, local),
        CalcMode::Spline => {
            // One spline per *segment* (frames.len() - 1 quadruples).
            let eased = match key_splines.and_then(|s| s.get(idx)) {
                Some(spline) => spline.ease(local),
                // Missing / malformed keySplines → linear within the
                // segment (matches the SMIL "value not animated" fallback
                // mandated by spec).
                None => local,
            };
            lerp_string(&a.value, &b.value, eased)
        }
    }
}

/// One cubic Bézier easing segment from `keySplines="x1 y1 x2 y2"`.
#[derive(Clone, Copy, Debug)]
struct KeySpline {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
}

impl KeySpline {
    /// Map a linear segment-local `t in [0,1]` through the cubic Bézier
    /// curve `(0,0) → (x1,y1) → (x2,y2) → (1,1)`. Newton-Raphson on
    /// `x(s) = t` (3 iterations is plenty since the curve is monotone
    /// in x for valid splines per the SMIL spec).
    fn ease(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        // Bezier coefficients in the polynomial form:
        // x(s) = 3(1-s)^2 s · x1 + 3(1-s) s^2 · x2 + s^3
        let mut s = t;
        for _ in 0..6 {
            let xs = bezier_axis(s, self.x1, self.x2);
            let dx = bezier_axis_d(s, self.x1, self.x2);
            if dx.abs() < 1e-6 {
                break;
            }
            s -= (xs - t) / dx;
            s = s.clamp(0.0, 1.0);
        }
        bezier_axis(s, self.y1, self.y2).clamp(0.0, 1.0)
    }
}

fn bezier_axis(s: f32, p1: f32, p2: f32) -> f32 {
    let one = 1.0 - s;
    3.0 * one * one * s * p1 + 3.0 * one * s * s * p2 + s * s * s
}

fn bezier_axis_d(s: f32, p1: f32, p2: f32) -> f32 {
    let one = 1.0 - s;
    3.0 * one * one * p1 + 6.0 * one * s * (p2 - p1) + 3.0 * s * s * (1.0 - p2)
}

/// Parse `keySplines="x1 y1 x2 y2 ; x1 y1 x2 y2 ; ..."`. Returns
/// `None` when malformed or when the segment count doesn't match.
fn parse_key_splines(raw: Option<&str>, expected: usize) -> Option<Vec<KeySpline>> {
    if expected == 0 {
        return None;
    }
    let raw = raw?;
    let segments: Vec<KeySpline> = raw
        .split(';')
        .filter_map(|seg| {
            let nums: Result<Vec<f32>, _> = seg
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|p| !p.is_empty())
                .map(|p| p.parse::<f32>())
                .collect();
            let nums = nums.ok()?;
            if nums.len() != 4 {
                return None;
            }
            Some(KeySpline {
                x1: nums[0],
                y1: nums[1],
                x2: nums[2],
                y2: nums[3],
            })
        })
        .collect();
    if segments.len() != expected {
        return None;
    }
    Some(segments)
}

/// Redistribute `frame.time` values for `calcMode="paced"`. Each
/// segment's time becomes proportional to its attribute-space
/// distance.  Numeric and colour values get a real metric; non-numeric
/// values fall back to uniform spacing.
fn repace_frames(frames: &mut [Frame]) {
    if frames.len() < 2 {
        return;
    }
    let n = frames.len();
    let mut dists = Vec::with_capacity(n - 1);
    let mut total = 0.0_f32;
    for w in frames.windows(2) {
        let d = paced_distance(&w[0].value, &w[1].value);
        total += d;
        dists.push(d);
    }
    if total <= 0.0 {
        // No usable distance metric — keep uniform spacing (the round-4
        // default).
        for (i, f) in frames.iter_mut().enumerate() {
            f.time = i as f32 / (n - 1) as f32;
        }
        return;
    }
    let mut acc = 0.0_f32;
    frames[0].time = 0.0;
    for (i, d) in dists.iter().enumerate() {
        acc += *d;
        frames[i + 1].time = (acc / total).clamp(0.0, 1.0);
    }
    // Floating-point safety: pin the last entry to exactly 1.0.
    if let Some(last) = frames.last_mut() {
        last.time = 1.0;
    }
}

/// Distance metric between two animation values for `calcMode="paced"`.
/// Numeric → absolute difference; RGBA colour → Euclidean in
/// 4-component space (each component in 0..=255). Anything else → 0.
fn paced_distance(a: &str, b: &str) -> f32 {
    if let (Ok(av), Ok(bv)) = (a.trim().parse::<f32>(), b.trim().parse::<f32>()) {
        return (bv - av).abs();
    }
    if let (Some(ca), Some(cb)) = (parse_color_for_lerp(a), parse_color_for_lerp(b)) {
        let dr = ca.0 as f32 - cb.0 as f32;
        let dg = ca.1 as f32 - cb.1 as f32;
        let db = ca.2 as f32 - cb.2 as f32;
        let da = ca.3 as f32 - cb.3 as f32;
        return (dr * dr + dg * dg + db * db + da * da).sqrt();
    }
    0.0
}

/// Public wrapper for [`lerp_string`] so the round-16 keyframe
/// evaluator can reuse the same colour + scalar interpolation rules
/// without duplicating the parser. Crate-public to keep the surface
/// area small.
pub(crate) fn lerp_string_public(a: &str, b: &str, t: f32) -> String {
    lerp_string(a, b, t)
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

// ---------------------------------------------------------------------
// Round 125 — SVG 1.1 §19.2.14 `<animateMotion>` + `<mpath>` evaluator.
// ---------------------------------------------------------------------

use oxideav_core::{Path, PathCommand, Point};

/// Round 125 — evaluate an `<animateMotion>` element at `t_seconds`.
///
/// Returns the supplemental `transform=` value that should be folded
/// into the parent element's attribute set per SVG 1.1 §19.2.14:
///
/// > The effect of a motion path animation is to add a supplemental
/// > transformation matrix onto the CTM for the referenced object
/// > which causes a translation along the x- and y-axes of the
/// > current user coordinate system by the computed X and Y values
/// > computed over time.
///
/// The string is always of the form
/// `translate(<x>,<y>) rotate(<angle>)` (the rotate term is omitted
/// when `rotate="0"` is the resolved value, keeping output lean for
/// the common no-rotate case).
///
/// `id_lookup` resolves a bare id (no leading `#`) to its source
/// [`Element`], used to find the `<path>` an inner `<mpath>` points
/// at. Pass `&|_| None` if you want to ignore mpath references.
///
/// Returns `None` when the animation isn't active at `t_seconds`
/// (before `begin`) or the motion path is empty / unparseable. The
/// `fill="freeze"` default means a finished animation continues to
/// report the last-frame transform.
pub fn evaluate_motion_at<'a>(
    el: &Element,
    t_seconds: f32,
    id_lookup: &dyn Fn(&str) -> Option<&'a Element>,
) -> Option<String> {
    let begin = parse_clock(attr(el, "begin")).unwrap_or(0.0);
    let dur = parse_clock(attr(el, "dur"));
    let repeat = parse_repeat_count(attr(el, "repeatCount"));
    let local_t = local_time(t_seconds, begin, dur, repeat)?;

    // Resolve the motion path per §19.2.14 precedence:
    //   <mpath>  >  path=  >  values=  >  from/by/to
    let path = motion_path_from_element(el, id_lookup)?;
    if path.commands.len() < 2 {
        // A bare MoveTo (or empty path) has no defined direction or
        // length — no motion to apply.
        let pos = match path.commands.first() {
            Some(PathCommand::MoveTo(p)) => *p,
            _ => return None,
        };
        return Some(format_motion_transform(pos, 0.0));
    }

    // Default calcMode for animateMotion is `paced` (§19.2.14 — the
    // only difference from animateTransform). `paced` walks the path
    // at constant velocity, which is the natural arc-length sampling.
    let mode = calc_mode_with_default(el, CalcMode::Paced);

    // Fraction along the motion path (0..=1).
    let t01 = match dur {
        Some(d) if d > 0.0 => (local_t / d).clamp(0.0, 1.0),
        _ => 0.0,
    };

    // `keyPoints` overrides every other position->time mapping per
    // §19.2.14. When present together with `keyTimes` the (keyTimes,
    // keyPoints) pair maps document time to path-fraction.
    let frac = if let Some(kp_frac) = key_points_mapping(el, t01, mode) {
        kp_frac
    } else {
        // Without keyPoints the fraction along the path is just t01
        // for the paced default. For discrete/linear/spline, the
        // segment boundaries fall on equally-spaced keyTimes by
        // default, but for a single supplemental matrix this still
        // reduces to t01 along the arc length per the "find the
        // point (x,y) which is t/dur distance along the motion path"
        // rule at the end of §19.2.14.
        match mode {
            CalcMode::Discrete => {
                // Snap to the start of the path segment in
                // path-distance space.
                (t01 - t01.fract()).max(0.0)
            }
            _ => t01,
        }
    };

    let (pos, tangent_angle_deg) = sample_path_at_fraction(&path, frac);

    let rot_attr = attr(el, "rotate").map(str::trim).unwrap_or("0");
    let rotation_deg = match rot_attr {
        "auto" => tangent_angle_deg,
        "auto-reverse" => tangent_angle_deg + 180.0,
        s => s.parse::<f32>().unwrap_or(0.0),
    };

    Some(format_motion_transform(pos, rotation_deg))
}

/// Resolve the motion path for an `<animateMotion>` element per the
/// §19.2.14 precedence: `<mpath>` (inner element) > `path=` attr >
/// `values=` (polyline) > `from`/`by`/`to` (single segment).
fn motion_path_from_element<'a>(
    el: &Element,
    id_lookup: &dyn Fn(&str) -> Option<&'a Element>,
) -> Option<Path> {
    // 1. <mpath xlink:href="#path1"> → look up the referenced <path>
    //    element's `d` attribute.
    for child in &el.children {
        if let XmlNode::Element(c) = child {
            if tag_local(&c.name) == "mpath" {
                let href = attr(c, "href").or_else(|| attr(c, "xlink:href"))?;
                let id = href.trim().strip_prefix('#')?;
                let target = id_lookup(id)?;
                let d = attr(target, "d")?;
                return path_from_d(d);
            }
        }
    }
    // 2. path="..."
    if let Some(d) = attr(el, "path") {
        return path_from_d(d);
    }
    // 3. values="x0,y0; x1,y1; ..." → polyline.
    if let Some(values) = attr(el, "values") {
        let pts: Vec<Point> = values
            .split(';')
            .filter_map(|seg| parse_xy(seg.trim()))
            .collect();
        if pts.is_empty() {
            return None;
        }
        let mut p = Path::new();
        p.move_to(pts[0]);
        for pt in &pts[1..] {
            p.line_to(*pt);
        }
        return Some(p);
    }
    // 4. from/to (or from/by, or to-only).
    let from = attr(el, "from").and_then(parse_xy);
    let to = attr(el, "to").and_then(parse_xy);
    let by = attr(el, "by").and_then(parse_xy);
    let mut p = Path::new();
    match (from, to, by) {
        (Some(f), Some(t), _) => {
            p.move_to(f);
            p.line_to(t);
            Some(p)
        }
        (Some(f), None, Some(b)) => {
            p.move_to(f);
            p.line_to(Point::new(f.x + b.x, f.y + b.y));
            Some(p)
        }
        (None, Some(t), _) => {
            // to-only: SMIL describes this as "from the underlying
            // value to t". The underlying value for animateMotion is
            // the origin offset (0,0).
            p.move_to(Point::new(0.0, 0.0));
            p.line_to(t);
            Some(p)
        }
        (Some(f), None, None) => {
            // from-only: degenerate — single position, no motion.
            p.move_to(f);
            Some(p)
        }
        _ => None,
    }
}

/// Parse one `x,y` pair (commas and/or whitespace separated). Returns
/// `None` on any parse failure.
fn parse_xy(s: &str) -> Option<Point> {
    let nums = parse_numbers(s);
    if nums.len() < 2 {
        return None;
    }
    Some(Point::new(nums[0], nums[1]))
}

/// Construct a [`Path`] from a `d`-attribute string. Failures (bad
/// command sequence) collapse to `None` rather than poison the
/// document — `<animateMotion>` skips and the parent renders at its
/// declared transform.
fn path_from_d(d: &str) -> Option<Path> {
    let cmds = crate::path_data::parse_path_data(d).ok()?;
    if cmds.is_empty() {
        return None;
    }
    let mut path = Path::new();
    for c in cmds {
        path.commands.push(c);
    }
    Some(path)
}

/// Map document-time fraction `t01` to a path-distance fraction
/// when `keyPoints` is present. Returns `None` if `keyPoints` is
/// absent or doesn't pair correctly with `keyTimes`.
fn key_points_mapping(el: &Element, t01: f32, mode: CalcMode) -> Option<f32> {
    let kp_attr = attr(el, "keyPoints")?;
    let key_points: Vec<f32> = kp_attr
        .split(';')
        .map(|s| s.trim().parse::<f32>().ok())
        .collect::<Option<Vec<_>>>()?;
    if key_points.is_empty() {
        return None;
    }
    // keyTimes is required to be present + same length per §19.2.14:
    // "there must be exactly as many values in the keyPoints list as
    // in the keyTimes list".
    let kt_attr = attr(el, "keyTimes")?;
    let key_times: Vec<f32> = kt_attr
        .split(';')
        .map(|s| s.trim().parse::<f32>().ok())
        .collect::<Option<Vec<_>>>()?;
    if key_times.len() != key_points.len() {
        return None;
    }
    // Find segment containing t01.
    if t01 <= key_times[0] {
        return Some(key_points[0]);
    }
    if let Some(&last) = key_times.last() {
        if t01 >= last {
            return Some(*key_points.last().unwrap());
        }
    }
    let mut idx = 0;
    for (i, kt) in key_times.iter().enumerate() {
        if *kt > t01 {
            idx = i.saturating_sub(1);
            break;
        }
    }
    let next = (idx + 1).min(key_times.len() - 1);
    match mode {
        CalcMode::Discrete => Some(key_points[idx]),
        _ => {
            // Linear / paced / spline all reduce to linear within
            // the (key_times[idx], key_times[next]) segment for the
            // round-125 path-fraction calculation; full spline
            // easing on the keyPoints sweep is a later refinement.
            let span = (key_times[next] - key_times[idx]).max(f32::EPSILON);
            let local = ((t01 - key_times[idx]) / span).clamp(0.0, 1.0);
            Some(key_points[idx] + (key_points[next] - key_points[idx]) * local)
        }
    }
}

/// Like [`calc_mode`] but with a caller-supplied default. Used to
/// match the spec rule that `<animateMotion>` defaults to `paced`
/// while the rest of the SMIL animation family defaults to `linear`.
fn calc_mode_with_default(el: &Element, default: CalcMode) -> CalcMode {
    match attr(el, "calcMode").map(str::trim) {
        Some("discrete") => CalcMode::Discrete,
        Some("linear") => CalcMode::Linear,
        Some("paced") => CalcMode::Paced,
        Some("spline") => CalcMode::Spline,
        _ => default,
    }
}

/// Sample a path at fraction `frac` of its total arc length.
/// Returns `(position, tangent_angle_degrees)`.
///
/// The tangent angle is in degrees, measured from +X axis (matches
/// the SVG `rotate(<angle>)` convention).
fn sample_path_at_fraction(path: &Path, frac: f32) -> (Point, f32) {
    let frac = frac.clamp(0.0, 1.0);
    let total = crate::path_length::compute_path_length(path);
    let target = total * frac;

    let mut acc = 0.0_f32;
    let mut cur = Point::default();
    let mut subpath_start = Point::default();
    // Track the previous-segment tangent so a hit at the very start
    // can re-use it (per §19.2.14 the tangent of the first sample
    // matches the first non-zero-length segment).
    let mut last_tangent = 0.0_f32;
    let mut first_segment = true;

    for cmd in &path.commands {
        match *cmd {
            PathCommand::MoveTo(p) => {
                cur = p;
                subpath_start = p;
            }
            PathCommand::LineTo(p) => {
                let seg_len = dist(cur, p);
                if seg_len > 0.0 {
                    let tan = (p.y - cur.y).atan2(p.x - cur.x).to_degrees();
                    first_segment = false;
                    if acc + seg_len >= target {
                        let local = ((target - acc) / seg_len).clamp(0.0, 1.0);
                        let x = cur.x + (p.x - cur.x) * local;
                        let y = cur.y + (p.y - cur.y) * local;
                        return (Point::new(x, y), tan);
                    }
                    acc += seg_len;
                    last_tangent = tan;
                }
                cur = p;
            }
            PathCommand::QuadCurveTo { control, end } => {
                let (hit, tan, advanced) =
                    sample_quad(cur, control, end, target - acc, first_segment, last_tangent);
                if let Some(p) = hit {
                    return (p, tan);
                }
                acc += advanced;
                last_tangent = tan;
                first_segment = false;
                cur = end;
            }
            PathCommand::CubicCurveTo { c1, c2, end } => {
                let (hit, tan, advanced) =
                    sample_cubic(cur, c1, c2, end, target - acc, first_segment, last_tangent);
                if let Some(p) = hit {
                    return (p, tan);
                }
                acc += advanced;
                last_tangent = tan;
                first_segment = false;
                cur = end;
            }
            PathCommand::ArcTo {
                rx,
                ry,
                x_axis_rot,
                large_arc,
                sweep,
                end,
            } => {
                // Round 125: sample the elliptical arc at 64 chord
                // steps and walk segment-by-segment, matching the
                // sampling density `path_length::compute_path_length`
                // uses so the arc-length total agrees with the
                // running accumulator. This keeps `sample_at(0.5)`
                // landing at the geometric midpoint of the arc.
                let arc_pts = flatten_arc(cur, rx, ry, x_axis_rot, large_arc, sweep, end);
                let mut prev = cur;
                let mut hit = None;
                let mut last_tan = last_tangent;
                for p in &arc_pts {
                    let seg = dist(prev, *p);
                    if seg > 0.0 {
                        let tan = (p.y - prev.y).atan2(p.x - prev.x).to_degrees();
                        first_segment = false;
                        last_tan = tan;
                        if acc + seg >= target {
                            let local = ((target - acc) / seg).clamp(0.0, 1.0);
                            let x = prev.x + (p.x - prev.x) * local;
                            let y = prev.y + (p.y - prev.y) * local;
                            hit = Some((Point::new(x, y), tan));
                            break;
                        }
                        acc += seg;
                    }
                    prev = *p;
                }
                if let Some((p, tan)) = hit {
                    return (p, tan);
                }
                last_tangent = last_tan;
                cur = end;
            }
            PathCommand::Close => {
                let p = subpath_start;
                let seg_len = dist(cur, p);
                if seg_len > 0.0 {
                    let tan = (p.y - cur.y).atan2(p.x - cur.x).to_degrees();
                    first_segment = false;
                    if acc + seg_len >= target {
                        let local = ((target - acc) / seg_len).clamp(0.0, 1.0);
                        let x = cur.x + (p.x - cur.x) * local;
                        let y = cur.y + (p.y - cur.y) * local;
                        return (Point::new(x, y), tan);
                    }
                    acc += seg_len;
                    last_tangent = tan;
                }
                cur = p;
            }
            _ => {}
        }
    }
    // Past the end of the path → return the final pen position with
    // the last computed tangent.
    (cur, last_tangent)
}

/// Sample a quadratic Bezier by chord-walking 32 steps. Returns the
/// hit point + tangent (degrees) when the target distance falls
/// inside this segment; otherwise the full segment length advanced
/// and the segment's end tangent so the outer loop can keep walking.
fn sample_quad(
    p0: Point,
    pc: Point,
    p1: Point,
    target_remaining: f32,
    first_segment: bool,
    last_tangent: f32,
) -> (Option<Point>, f32, f32) {
    let _ = (first_segment, last_tangent);
    const N: usize = 32;
    let mut prev = p0;
    let mut acc = 0.0_f32;
    let mut tan = (p1.y - p0.y).atan2(p1.x - p0.x).to_degrees();
    for i in 1..=N {
        let t = i as f32 / N as f32;
        let omt = 1.0 - t;
        let x = omt * omt * p0.x + 2.0 * omt * t * pc.x + t * t * p1.x;
        let y = omt * omt * p0.y + 2.0 * omt * t * pc.y + t * t * p1.y;
        let p = Point::new(x, y);
        let seg = dist(prev, p);
        if seg > 0.0 {
            tan = (p.y - prev.y).atan2(p.x - prev.x).to_degrees();
        }
        if acc + seg >= target_remaining && target_remaining > 0.0 {
            let local = ((target_remaining - acc) / seg.max(f32::EPSILON)).clamp(0.0, 1.0);
            let x = prev.x + (p.x - prev.x) * local;
            let y = prev.y + (p.y - prev.y) * local;
            return (Some(Point::new(x, y)), tan, acc + seg);
        }
        acc += seg;
        prev = p;
    }
    (None, tan, acc)
}

/// Sample a cubic Bezier by chord-walking 32 steps. Same shape as
/// [`sample_quad`].
fn sample_cubic(
    p0: Point,
    c1: Point,
    c2: Point,
    p1: Point,
    target_remaining: f32,
    first_segment: bool,
    last_tangent: f32,
) -> (Option<Point>, f32, f32) {
    let _ = (first_segment, last_tangent);
    const N: usize = 32;
    let mut prev = p0;
    let mut acc = 0.0_f32;
    let mut tan = (p1.y - p0.y).atan2(p1.x - p0.x).to_degrees();
    for i in 1..=N {
        let t = i as f32 / N as f32;
        let omt = 1.0 - t;
        let omt2 = omt * omt;
        let omt3 = omt2 * omt;
        let t2 = t * t;
        let t3 = t2 * t;
        let x = omt3 * p0.x + 3.0 * omt2 * t * c1.x + 3.0 * omt * t2 * c2.x + t3 * p1.x;
        let y = omt3 * p0.y + 3.0 * omt2 * t * c1.y + 3.0 * omt * t2 * c2.y + t3 * p1.y;
        let p = Point::new(x, y);
        let seg = dist(prev, p);
        if seg > 0.0 {
            tan = (p.y - prev.y).atan2(p.x - prev.x).to_degrees();
        }
        if acc + seg >= target_remaining && target_remaining > 0.0 {
            let local = ((target_remaining - acc) / seg.max(f32::EPSILON)).clamp(0.0, 1.0);
            let x = prev.x + (p.x - prev.x) * local;
            let y = prev.y + (p.y - prev.y) * local;
            return (Some(Point::new(x, y)), tan, acc + seg);
        }
        acc += seg;
        prev = p;
    }
    (None, tan, acc)
}

fn dist(a: Point, b: Point) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}

/// Flatten an SVG endpoint-form elliptical arc into a 64-segment
/// polyline (the same sampling density used by
/// [`crate::path_length::compute_path_length`]). Returns the sampled
/// points after the start — i.e. the start point is *not* included.
fn flatten_arc(
    start: Point,
    rx: f32,
    ry: f32,
    x_axis_rot_rad: f32,
    large_arc: bool,
    sweep: bool,
    end: Point,
) -> Vec<Point> {
    // Degenerate cases per §F.6.2 — collapse to a chord.
    if rx == 0.0 || ry == 0.0 || (start.x == end.x && start.y == end.y) {
        return vec![end];
    }
    let rx = rx.abs();
    let ry = ry.abs();

    let cos_phi = x_axis_rot_rad.cos();
    let sin_phi = x_axis_rot_rad.sin();
    let dx = (start.x - end.x) * 0.5;
    let dy = (start.y - end.y) * 0.5;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;

    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    let (rx, ry) = if lambda > 1.0 {
        let s = lambda.sqrt();
        (rx * s, ry * s)
    } else {
        (rx, ry)
    };

    let rx2 = rx * rx;
    let ry2 = ry * ry;
    let x1p2 = x1p * x1p;
    let y1p2 = y1p * y1p;
    let denom = rx2 * y1p2 + ry2 * x1p2;
    let mut factor = (rx2 * ry2 - denom) / denom;
    if factor < 0.0 {
        factor = 0.0;
    }
    let coef = factor.sqrt() * if large_arc == sweep { -1.0 } else { 1.0 };
    let cxp = coef * (rx * y1p) / ry;
    let cyp = coef * -(ry * x1p) / rx;

    let cx = cos_phi * cxp - sin_phi * cyp + (start.x + end.x) * 0.5;
    let cy = sin_phi * cxp + cos_phi * cyp + (start.y + end.y) * 0.5;

    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = arc_angle(1.0, 0.0, ux, uy);
    let mut delta = arc_angle(ux, uy, vx, vy);
    if !sweep && delta > 0.0 {
        delta -= std::f32::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f32::consts::TAU;
    }

    const N: usize = 64;
    let mut pts = Vec::with_capacity(N);
    for i in 1..=N {
        let t = i as f32 / N as f32;
        let theta = theta1 + delta * t;
        let x = cos_phi * rx * theta.cos() - sin_phi * ry * theta.sin() + cx;
        let y = sin_phi * rx * theta.cos() + cos_phi * ry * theta.sin() + cy;
        pts.push(Point::new(x, y));
    }
    pts
}

/// Signed angle between two 2-vectors per SVG 1.1 §F.6.5 (eqn
/// F.6.5.4). Same definition used by [`crate::path_length`].
fn arc_angle(ux: f32, uy: f32, vx: f32, vy: f32) -> f32 {
    let sign = if ux * vy - uy * vx >= 0.0 { 1.0 } else { -1.0 };
    let dot = ux * vx + uy * vy;
    let nu = (ux * ux + uy * uy).sqrt();
    let nv = (vx * vx + vy * vy).sqrt();
    let cos_a = (dot / (nu * nv)).clamp(-1.0, 1.0);
    sign * cos_a.acos()
}

/// Render the supplemental transform as `translate(x,y) rotate(deg)`,
/// dropping the `rotate(0)` term for the common no-rotate case.
fn format_motion_transform(pos: Point, rotation_deg: f32) -> String {
    if rotation_deg.abs() < 1e-3 {
        format!("translate({},{})", trim_f(pos.x), trim_f(pos.y))
    } else {
        format!(
            "translate({},{}) rotate({})",
            trim_f(pos.x),
            trim_f(pos.y),
            trim_f(rotation_deg)
        )
    }
}

/// Format an `f32` without trailing ".0" zeros for compact output.
fn trim_f(v: f32) -> String {
    let s = format!("{:.6}", v);
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".into()
    } else {
        trimmed.into()
    }
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
