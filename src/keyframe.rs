//! Round 16/17 — CSS Animations L1 keyframe evaluation at a runtime
//! `t_seconds`.
//!
//! Round 15 captured `@keyframes` blocks into
//! [`crate::css::Stylesheet::keyframes`] but never evaluated them
//! against the current animation timeline. Round 16 closed that gap:
//! given an element whose effective CSS declarations include
//! `animation-name: <kf>` + `animation-duration: <s>`, this module
//! looks up the matching [`crate::css::KeyframesRule`], computes the
//! active selector pair bracketing the normalised time
//! `t / duration % 1.0`, lerps each keyframe property between the
//! `from` + `to` declarations, and returns the merged property
//! overrides ready to splice into the element's effective property
//! map.
//!
//! **Round 17 long-tail additions** (per CSS Animations L1 §3 / §4
//! / §6):
//!
//! - `animation-timing-function` — `linear` / `ease` / `ease-in` /
//!   `ease-out` / `ease-in-out` / `cubic-bezier(x1,y1,x2,y2)` /
//!   `steps(N, start|end)`. The named easings expand to the standard
//!   cubic-bezier curves per CSS Easing Functions L1 §3.1.
//! - **multi-name `animation-name`** — `animation-name: a, b, c`
//!   evaluates each animation independently per L1 §6 and returns
//!   the merged property map (later animations win on shared
//!   properties — the standard "multiple animations" cascade).
//! - `animation-direction` — `normal` / `reverse` / `alternate` /
//!   `alternate-reverse` per L1 §4.4. Affects how the per-iteration
//!   normalised position maps onto the keyframe timeline.
//! - `animation-fill-mode` — `none` / `forwards` / `backwards` /
//!   `both` per L1 §4.7. Decides what to apply before
//!   `animation-delay` and after the iteration count completes.
//!
//! Per-property pairing (L1 §6): the parser indexes each
//! comma-separated entry against the corresponding entry in
//! `animation-name`. Shorter lists wrap (the spec mandates indexed
//! list lookup with modulo addressing — every browser does this).
//!
//! Supported animation longhand declarations (per CSS Animations L1):
//!
//! - `animation-name` (one or many; comma-separated lists evaluate
//!   each entry independently).
//! - `animation-duration` (one or many, in `s` or `ms`).
//! - `animation-iteration-count` (numeric or `infinite`; defaults to
//!   `1`).
//! - `animation-delay` (one or many, in `s` or `ms`; defaults to `0`).
//! - `animation-timing-function` (one or many; defaults to `ease`).
//! - `animation-direction` (one or many; defaults to `normal`).
//! - `animation-fill-mode` (one or many; defaults to `none`).
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

/// CSS Easing function — `animation-timing-function` per CSS Easing
/// Functions L1.
///
/// Round 17 supports the full enum surface: linear / the four named
/// `ease*` curves (which expand to the standard cubic-bezier control
/// points per L1 §3.1) / explicit `cubic-bezier(x1,y1,x2,y2)` / the
/// stepping function `steps(N, start|end)` per §4.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TimingFunction {
    /// Identity — `output = input`.
    Linear,
    /// `cubic-bezier(x1, y1, x2, y2)` per L1 §3. The `ease*` named
    /// keywords expand to instances of this variant during parsing
    /// (so the [`TimingFunction::compute_progress`] solver only has
    /// to handle the parametric form).
    CubicBezier { x1: f32, y1: f32, x2: f32, y2: f32 },
    /// `steps(<count>, start|end)` per L1 §4.
    Steps { count: u32, position: StepPosition },
}

impl Default for TimingFunction {
    fn default() -> Self {
        // CSS Animations L1 §3.4 — the initial value is `ease`.
        // (Not derivable: the `ease` variant is a CubicBezier with
        // explicit control points, not a unit variant.)
        Self::ease()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepPosition {
    Start,
    End,
}

impl TimingFunction {
    pub const fn linear() -> Self {
        TimingFunction::Linear
    }
    /// `ease` — `cubic-bezier(0.25, 0.1, 0.25, 1)` per L1 §3.1.
    pub const fn ease() -> Self {
        TimingFunction::CubicBezier {
            x1: 0.25,
            y1: 0.1,
            x2: 0.25,
            y2: 1.0,
        }
    }
    /// `ease-in` — `cubic-bezier(0.42, 0, 1, 1)`.
    pub const fn ease_in() -> Self {
        TimingFunction::CubicBezier {
            x1: 0.42,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
        }
    }
    /// `ease-out` — `cubic-bezier(0, 0, 0.58, 1)`.
    pub const fn ease_out() -> Self {
        TimingFunction::CubicBezier {
            x1: 0.0,
            y1: 0.0,
            x2: 0.58,
            y2: 1.0,
        }
    }
    /// `ease-in-out` — `cubic-bezier(0.42, 0, 0.58, 1)`.
    pub const fn ease_in_out() -> Self {
        TimingFunction::CubicBezier {
            x1: 0.42,
            y1: 0.0,
            x2: 0.58,
            y2: 1.0,
        }
    }

    /// Map the linear `t_normalised` ∈ [0,1] to the eased output ∈
    /// [0,1] per CSS Easing Functions L1.
    ///
    /// For [`TimingFunction::CubicBezier`] this solves the parametric
    /// curve `B(s) = (Bx(s), By(s))` for `Bx(s) = t` then returns
    /// `By(s)` — the standard CSS easing solver. For
    /// [`TimingFunction::Steps`] this picks the step bucket per L1
    /// §4.
    pub fn compute_progress(&self, t_normalised: f32) -> f32 {
        let t = t_normalised.clamp(0.0, 1.0);
        match *self {
            TimingFunction::Linear => t,
            TimingFunction::CubicBezier { x1, y1, x2, y2 } => bezier_y_at_x(t, x1, y1, x2, y2),
            TimingFunction::Steps { count, position } => {
                if count == 0 {
                    return t;
                }
                let n = count as f32;
                let raw = t * n;
                let step = match position {
                    StepPosition::Start => raw.ceil(),
                    StepPosition::End => raw.floor(),
                };
                (step / n).clamp(0.0, 1.0)
            }
        }
    }

    /// Parse a CSS `<easing-function>` value per L1 §3 / §4. Returns
    /// `None` for an empty or unrecognised input (caller falls back
    /// to the default `ease`).
    pub fn parse(s: &str) -> Option<Self> {
        let t = s.trim();
        if t.is_empty() {
            return None;
        }
        let lower = t.to_ascii_lowercase();
        match lower.as_str() {
            "linear" => return Some(Self::linear()),
            "ease" => return Some(Self::ease()),
            "ease-in" => return Some(Self::ease_in()),
            "ease-out" => return Some(Self::ease_out()),
            "ease-in-out" => return Some(Self::ease_in_out()),
            // CSS Easing L1 §4 step-keyword shorthands.
            "step-start" => {
                return Some(TimingFunction::Steps {
                    count: 1,
                    position: StepPosition::Start,
                })
            }
            "step-end" => {
                return Some(TimingFunction::Steps {
                    count: 1,
                    position: StepPosition::End,
                })
            }
            _ => {}
        }
        // cubic-bezier(x1, y1, x2, y2)
        if let Some(args) = strip_call(&lower, "cubic-bezier") {
            let parts: Vec<&str> = args.split(',').map(|p| p.trim()).collect();
            if parts.len() == 4 {
                let x1 = parts[0].parse::<f32>().ok()?;
                let y1 = parts[1].parse::<f32>().ok()?;
                let x2 = parts[2].parse::<f32>().ok()?;
                let y2 = parts[3].parse::<f32>().ok()?;
                // x1, x2 must be in [0,1] per L1 §3; clamp rather
                // than reject to stay tolerant.
                let x1 = x1.clamp(0.0, 1.0);
                let x2 = x2.clamp(0.0, 1.0);
                return Some(TimingFunction::CubicBezier { x1, y1, x2, y2 });
            }
        }
        // steps(N) / steps(N, start|end|jump-start|jump-end)
        if let Some(args) = strip_call(&lower, "steps") {
            let parts: Vec<&str> = args.split(',').map(|p| p.trim()).collect();
            if parts.is_empty() {
                return None;
            }
            let count = parts[0].parse::<u32>().ok()?;
            let position = if parts.len() >= 2 {
                match parts[1] {
                    "start" | "jump-start" => StepPosition::Start,
                    // "end", "jump-end", "jump-none", "jump-both",
                    // and absent default to End per L1 §4.
                    _ => StepPosition::End,
                }
            } else {
                StepPosition::End
            };
            return Some(TimingFunction::Steps { count, position });
        }
        None
    }
}

fn strip_call<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let s = s.trim();
    let head = s.get(..name.len())?;
    if !head.eq_ignore_ascii_case(name) {
        return None;
    }
    let after = s[name.len()..].trim_start();
    let after = after.strip_prefix('(')?;
    let after = after.strip_suffix(')')?;
    Some(after)
}

/// Solve `Bx(s) = x` for parametric s, then evaluate `By(s)`. The
/// curve passes through (0,0) and (1,1) with control points (x1,y1),
/// (x2,y2). Standard CSS bezier solver — bisection seeded with a
/// linear guess; converges in <= 16 iterations for sub-1e-5 absolute
/// error which is far tighter than CSS Animations needs.
fn bezier_y_at_x(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let bx = |s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * x1 + 3.0 * u * s * s * x2 + s * s * s
    };
    let by = |s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * y1 + 3.0 * u * s * s * y2 + s * s * s
    };
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let mut lo = 0.0f32;
    let mut hi = 1.0f32;
    let mut s = x; // linear seed
    for _ in 0..32 {
        let cur = bx(s);
        let err = cur - x;
        if err.abs() < 1e-5 {
            break;
        }
        if err > 0.0 {
            hi = s;
        } else {
            lo = s;
        }
        s = 0.5 * (lo + hi);
    }
    by(s)
}

/// `animation-direction` per CSS Animations L1 §4.4.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnimationDirection {
    #[default]
    Normal,
    Reverse,
    Alternate,
    AlternateReverse,
}

impl AnimationDirection {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "normal" => Some(Self::Normal),
            "reverse" => Some(Self::Reverse),
            "alternate" => Some(Self::Alternate),
            "alternate-reverse" => Some(Self::AlternateReverse),
            _ => None,
        }
    }
}

/// `animation-fill-mode` per CSS Animations L1 §4.7.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnimationFillMode {
    #[default]
    None,
    Forwards,
    Backwards,
    Both,
}

impl AnimationFillMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "forwards" => Some(Self::Forwards),
            "backwards" => Some(Self::Backwards),
            "both" => Some(Self::Both),
            _ => None,
        }
    }
}

/// One resolved per-animation parameter set — extracted from the
/// element's CSS cascade by [`evaluate_at`] for each entry in the
/// comma-separated `animation-name` list.
#[derive(Clone, Debug)]
struct AnimationInstance<'a> {
    name: &'a str,
    duration: f32,
    delay: f32,
    iter_count: f32,
    timing: TimingFunction,
    direction: AnimationDirection,
    fill: AnimationFillMode,
}

/// Evaluate every `animation-name`-declared keyframe animation on the
/// element identified by `mctx` at the runtime `t_seconds`, and return
/// the resulting property overrides in source order.
///
/// **Round 17** — `animation-name` values are now comma-split; each
/// animation evaluates independently against the element's other
/// per-animation longhand lists (`animation-duration`,
/// `animation-delay`, `animation-iteration-count`,
/// `animation-timing-function`, `animation-direction`,
/// `animation-fill-mode`). Per L1 §6, when a longhand list is
/// shorter than the name list the shorter list wraps with modulo
/// addressing.
///
/// Returns an empty vector when the element has no `animation-name`
/// or no named animation has a matching `@keyframes` rule plus a
/// non-zero duration (and no fill-mode is keeping it visible).
pub fn evaluate_at(
    mctx: &MatchContext<'_>,
    sheet: &Stylesheet,
    t_seconds: f32,
) -> Vec<(String, String)> {
    let decls = declarations_for(mctx, sheet);
    let names_raw = match find_decl(&decls, "animation-name") {
        Some(v) => v,
        None => return Vec::new(),
    };
    let names: Vec<&str> = split_csv(names_raw)
        .into_iter()
        .filter(|n| !n.eq_ignore_ascii_case("none") && !n.is_empty())
        .collect();
    if names.is_empty() {
        return Vec::new();
    }
    let durations: Vec<f32> = split_csv_f(find_decl(&decls, "animation-duration"), parse_clock_str);
    let delays: Vec<f32> = split_csv_f(find_decl(&decls, "animation-delay"), parse_clock_str);
    let iters: Vec<f32> = split_csv_f(find_decl(&decls, "animation-iteration-count"), |s| {
        Some(parse_iter_count_str(s))
    });
    let timings: Vec<TimingFunction> =
        split_csv(find_decl(&decls, "animation-timing-function").unwrap_or(""))
            .into_iter()
            .filter_map(TimingFunction::parse)
            .collect();
    let directions: Vec<AnimationDirection> =
        split_csv(find_decl(&decls, "animation-direction").unwrap_or(""))
            .into_iter()
            .filter_map(AnimationDirection::parse)
            .collect();
    let fills: Vec<AnimationFillMode> =
        split_csv(find_decl(&decls, "animation-fill-mode").unwrap_or(""))
            .into_iter()
            .filter_map(AnimationFillMode::parse)
            .collect();

    // Build one AnimationInstance per name with mod-indexing on
    // every other longhand list per L1 §6.
    let mut merged: Vec<(String, String)> = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let inst = AnimationInstance {
            name,
            duration: pick_or(&durations, i, 0.0),
            delay: pick_or(&delays, i, 0.0),
            iter_count: pick_or(&iters, i, 1.0),
            timing: pick_or_default(&timings, i),
            direction: pick_or_default(&directions, i),
            fill: pick_or_default(&fills, i),
        };
        let rule = match sheet
            .keyframes
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(inst.name))
        {
            Some(r) => r,
            None => continue,
        };
        let snap = evaluate_one(rule, &inst, t_seconds);
        // Later animations override earlier ones on shared property
        // names (the L1 §6 cascade — last-listed wins on ties).
        for (k, v) in snap {
            if let Some(existing) = merged
                .iter_mut()
                .find(|(ek, _)| ek.eq_ignore_ascii_case(&k))
            {
                existing.1 = v;
            } else {
                merged.push((k, v));
            }
        }
    }
    merged
}

fn evaluate_one(
    rule: &KeyframesRule,
    inst: &AnimationInstance<'_>,
    t_seconds: f32,
) -> Vec<(String, String)> {
    if inst.duration <= 0.0 {
        // Without a duration the animation is dormant per §3 — but
        // the `backwards` / `both` fill mode still pins the first
        // keyframe at all times if the timeline hasn't started.
        if matches!(
            inst.fill,
            AnimationFillMode::Backwards | AnimationFillMode::Both
        ) {
            return interpolate_rule(rule, mapped_position(0.0, 0, inst.direction));
        }
        return Vec::new();
    }
    // Three timeline regions: pre-delay, active, post-end.
    if t_seconds < inst.delay {
        // Backwards/both: pin to the start — direction-mapped from t=0
        // of iteration 0.
        if matches!(
            inst.fill,
            AnimationFillMode::Backwards | AnimationFillMode::Both
        ) {
            let mapped = mapped_position(0.0, 0, inst.direction);
            return interpolate_rule(rule, mapped);
        }
        return Vec::new();
    }
    let elapsed = t_seconds - inst.delay;
    let cycles = elapsed / inst.duration;
    if cycles >= inst.iter_count {
        // Forwards/both: pin to the end of the last iteration —
        // direction-mapped.
        if matches!(
            inst.fill,
            AnimationFillMode::Forwards | AnimationFillMode::Both
        ) {
            // Index of the last completed iteration (one less than
            // the count when the count is a finite integer; for
            // non-integer counts the last partial iteration's index
            // is `floor(iter_count - eps)`).
            let last_iter = if inst.iter_count.is_finite() {
                (inst.iter_count - 1.0).max(0.0).floor() as u32
            } else {
                0
            };
            let mapped = mapped_position(1.0, last_iter, inst.direction);
            return interpolate_rule(rule, mapped);
        }
        return Vec::new();
    }
    // Active phase. Per-iteration normalised position is the
    // fractional cycle count.
    let iter_idx = cycles.floor() as u32;
    let frac = (cycles - cycles.floor()).clamp(0.0, 1.0);
    // Direction maps the raw fractional position into the keyframe
    // timeline — `reverse` flips it, `alternate*` toggles per
    // iteration.
    let mapped = mapped_position(frac, iter_idx, inst.direction);
    // Apply the timing function on top of the direction-mapped value
    // — per CSS Animations L1 §4.5, the timing function is applied
    // per-iteration to the direction-mapped position.
    let eased = inst.timing.compute_progress(mapped);
    interpolate_rule(rule, eased)
}

/// Map a per-iteration fraction `frac ∈ [0,1]` plus an iteration
/// index plus an [`AnimationDirection`] to the keyframe-timeline
/// position. Per CSS Animations L1 §4.4:
///
/// - `normal` → `frac`
/// - `reverse` → `1 - frac`
/// - `alternate` → `frac` on even iterations, `1 - frac` on odd
/// - `alternate-reverse` → `1 - frac` on even, `frac` on odd
fn mapped_position(frac: f32, iter_idx: u32, dir: AnimationDirection) -> f32 {
    let frac = frac.clamp(0.0, 1.0);
    match dir {
        AnimationDirection::Normal => frac,
        AnimationDirection::Reverse => 1.0 - frac,
        AnimationDirection::Alternate => {
            if iter_idx & 1 == 0 {
                frac
            } else {
                1.0 - frac
            }
        }
        AnimationDirection::AlternateReverse => {
            if iter_idx & 1 == 0 {
                1.0 - frac
            } else {
                frac
            }
        }
    }
}

fn pick_or<T: Copy>(v: &[T], idx: usize, default: T) -> T {
    if v.is_empty() {
        default
    } else {
        v[idx % v.len()]
    }
}

fn pick_or_default<T: Copy + Default>(v: &[T], idx: usize) -> T {
    if v.is_empty() {
        T::default()
    } else {
        v[idx % v.len()]
    }
}

fn split_csv(s: &str) -> Vec<&str> {
    s.split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect()
}

fn split_csv_f<T, F>(s: Option<&str>, f: F) -> Vec<T>
where
    F: Fn(&str) -> Option<T>,
{
    match s {
        Some(v) => split_csv(v).into_iter().filter_map(f).collect(),
        None => Vec::new(),
    }
}

fn parse_clock_str(s: &str) -> Option<f32> {
    parse_clock(Some(s))
}

fn parse_iter_count_str(s: &str) -> f32 {
    parse_iter_count(Some(s))
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
        // `linear` timing — round 17 default is `ease` per L1 §3.4.
        let el = elem(
            "g",
            &[(
                "style",
                "animation-name: spin; animation-duration: 1s; animation-timing-function: linear",
            )],
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
            &[(
                "style",
                "animation-name: fade; animation-duration: 4s; animation-timing-function: linear",
            )],
        );
        let mctx = MatchContext::root(&el);
        // t=1s of a 4s animation → 25% of a linear timing.
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
                "animation-name: spin; animation-duration: 1s; animation-iteration-count: infinite; animation-timing-function: linear",
            )],
        );
        let mctx = MatchContext::root(&el);
        // t=2.5s of a 1s indefinite → into the third cycle at 0.5 →
        // 180deg with linear timing.
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
            &[(
                "style",
                "animation-name: pulse; animation-duration: 1s; animation-timing-function: linear",
            )],
        );
        let mctx = MatchContext::root(&el);
        // At t=0.25 with linear timing → midway through 0%→50% →
        // opacity 0.5.
        let snap = evaluate_at(&mctx, &s, 0.25);
        let val: f32 = snap[0].1.parse().unwrap();
        assert!((val - 0.5).abs() < 1e-3, "expected 0.5, got {val}");
    }

    // ----- Round 17: timing-function / direction / fill-mode / multi-name

    #[test]
    fn timing_function_parse_named_keywords() {
        assert_eq!(
            TimingFunction::parse("linear"),
            Some(TimingFunction::Linear)
        );
        match TimingFunction::parse("ease").unwrap() {
            TimingFunction::CubicBezier { x1, y1, x2, y2 } => {
                assert!((x1 - 0.25).abs() < 1e-6);
                assert!((y1 - 0.1).abs() < 1e-6);
                assert!((x2 - 0.25).abs() < 1e-6);
                assert!((y2 - 1.0).abs() < 1e-6);
            }
            other => panic!("expected CubicBezier for ease, got {other:?}"),
        }
        // ease-in-out and ease-out and ease-in should be distinct.
        assert_ne!(
            TimingFunction::parse("ease-in"),
            TimingFunction::parse("ease-out")
        );
        assert_ne!(
            TimingFunction::parse("ease-in"),
            TimingFunction::parse("ease-in-out")
        );
    }

    #[test]
    fn timing_function_parse_cubic_bezier() {
        let tf = TimingFunction::parse("cubic-bezier(0.1, 0.2, 0.3, 0.4)").unwrap();
        match tf {
            TimingFunction::CubicBezier { x1, y1, x2, y2 } => {
                assert!((x1 - 0.1).abs() < 1e-6);
                assert!((y1 - 0.2).abs() < 1e-6);
                assert!((x2 - 0.3).abs() < 1e-6);
                assert!((y2 - 0.4).abs() < 1e-6);
            }
            _ => panic!("expected CubicBezier"),
        }
    }

    #[test]
    fn timing_function_parse_steps() {
        let tf = TimingFunction::parse("steps(4, end)").unwrap();
        assert_eq!(
            tf,
            TimingFunction::Steps {
                count: 4,
                position: StepPosition::End
            }
        );
        // Default is end.
        let tf = TimingFunction::parse("steps(2)").unwrap();
        assert_eq!(
            tf,
            TimingFunction::Steps {
                count: 2,
                position: StepPosition::End
            }
        );
        let tf = TimingFunction::parse("steps(3, start)").unwrap();
        assert_eq!(
            tf,
            TimingFunction::Steps {
                count: 3,
                position: StepPosition::Start
            }
        );
    }

    #[test]
    fn linear_timing_progress_is_identity() {
        let tf = TimingFunction::Linear;
        for &t in &[0.0, 0.25, 0.5, 0.75, 1.0] {
            assert!((tf.compute_progress(t) - t).abs() < 1e-6);
        }
    }

    #[test]
    fn cubic_bezier_progress_endpoints_pin() {
        let tf = TimingFunction::ease_in_out();
        assert!(tf.compute_progress(0.0).abs() < 1e-3);
        assert!((tf.compute_progress(1.0) - 1.0).abs() < 1e-3);
        // Midpoint of ease-in-out is symmetric → 0.5.
        let mid = tf.compute_progress(0.5);
        assert!(
            (mid - 0.5).abs() < 1e-2,
            "ease-in-out midpoint ≈ 0.5, got {mid}"
        );
    }

    #[test]
    fn cubic_bezier_ease_in_decelerates_then_accelerates() {
        let tf = TimingFunction::ease_in();
        // ease-in starts slow → at t=0.25 the progress should be < 0.25
        // (well below the linear value).
        let p = tf.compute_progress(0.25);
        assert!(p < 0.25, "ease-in p({}) should be < 0.25, got {p}", 0.25);
    }

    #[test]
    fn steps_end_buckets_quarters() {
        let tf = TimingFunction::Steps {
            count: 4,
            position: StepPosition::End,
        };
        // steps(4, end): t in [0, 0.25) → 0; [0.25, 0.5) → 0.25; …
        assert!(tf.compute_progress(0.0).abs() < 1e-6);
        assert!((tf.compute_progress(0.249) - 0.0).abs() < 1e-6);
        assert!((tf.compute_progress(0.25) - 0.25).abs() < 1e-6);
        assert!((tf.compute_progress(0.5) - 0.5).abs() < 1e-6);
        assert!((tf.compute_progress(0.99) - 0.75).abs() < 1e-6);
        assert!((tf.compute_progress(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn steps_start_advances_one_bucket_earlier() {
        let tf = TimingFunction::Steps {
            count: 4,
            position: StepPosition::Start,
        };
        // steps(4, start): t=0 → 0.25 (jumps at the start); t=1 → 1.0.
        assert!((tf.compute_progress(0.0)).abs() < 1e-6);
        assert!((tf.compute_progress(0.01) - 0.25).abs() < 1e-6);
        assert!((tf.compute_progress(0.25) - 0.25).abs() < 1e-6);
        assert!((tf.compute_progress(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn animation_direction_reverse_inverts_progress() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @keyframes fade { from { opacity: 0 } to { opacity: 1 } }
            "#,
        );
        let el = elem(
            "g",
            &[(
                "style",
                "animation-name: fade; animation-duration: 1s; animation-timing-function: linear; animation-direction: reverse",
            )],
        );
        let mctx = MatchContext::root(&el);
        // t=0.25, reverse → mapped position 0.75 → opacity 0.75.
        let snap = evaluate_at(&mctx, &s, 0.25);
        let val: f32 = snap[0].1.parse().unwrap();
        assert!((val - 0.75).abs() < 1e-3, "expected 0.75, got {val}");
    }

    #[test]
    fn animation_direction_alternate_flips_on_odd_iter() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @keyframes fade { from { opacity: 0 } to { opacity: 1 } }
            "#,
        );
        let el = elem(
            "g",
            &[(
                "style",
                "animation-name: fade; animation-duration: 1s; animation-iteration-count: infinite; animation-timing-function: linear; animation-direction: alternate",
            )],
        );
        let mctx = MatchContext::root(&el);
        // t=0.25 → iter 0, frac 0.25, alternate even → 0.25.
        let v0: f32 = evaluate_at(&mctx, &s, 0.25)[0].1.parse().unwrap();
        assert!(
            (v0 - 0.25).abs() < 1e-3,
            "iter 0 t=0.25 expect 0.25, got {v0}"
        );
        // t=1.25 → iter 1, frac 0.25, alternate odd → 0.75.
        let v1: f32 = evaluate_at(&mctx, &s, 1.25)[0].1.parse().unwrap();
        assert!(
            (v1 - 0.75).abs() < 1e-3,
            "iter 1 t=1.25 expect 0.75, got {v1}"
        );
    }

    #[test]
    fn animation_fill_mode_forwards_pins_after_end() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @keyframes fade { from { opacity: 0 } to { opacity: 1 } }
            "#,
        );
        let el = elem(
            "g",
            &[(
                "style",
                "animation-name: fade; animation-duration: 1s; animation-timing-function: linear; animation-fill-mode: forwards",
            )],
        );
        let mctx = MatchContext::root(&el);
        // After the iteration completes (t=2s of a 1s anim, count=1 default),
        // forwards keeps the final keyframe pinned.
        let snap = evaluate_at(&mctx, &s, 2.0);
        assert_eq!(snap.len(), 1);
        let val: f32 = snap[0].1.parse().unwrap();
        assert!(
            (val - 1.0).abs() < 1e-3,
            "forwards should pin to 1.0, got {val}"
        );
    }

    #[test]
    fn animation_fill_mode_backwards_pins_before_delay() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @keyframes fade { from { opacity: 0 } to { opacity: 1 } }
            "#,
        );
        let el = elem(
            "g",
            &[(
                "style",
                "animation-name: fade; animation-duration: 1s; animation-delay: 2s; animation-timing-function: linear; animation-fill-mode: backwards",
            )],
        );
        let mctx = MatchContext::root(&el);
        // Before delay → backwards pins to start (opacity 0).
        let snap = evaluate_at(&mctx, &s, 0.5);
        assert_eq!(snap.len(), 1);
        let val: f32 = snap[0].1.parse().unwrap();
        assert!(val.abs() < 1e-3, "backwards should pin to 0.0, got {val}");
    }

    #[test]
    fn animation_fill_mode_none_after_end_returns_empty() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @keyframes fade { from { opacity: 0 } to { opacity: 1 } }
            "#,
        );
        let el = elem(
            "g",
            &[(
                "style",
                "animation-name: fade; animation-duration: 1s; animation-timing-function: linear",
            )],
        );
        let mctx = MatchContext::root(&el);
        // After the iteration completes (t=2s), default fill-mode `none`
        // returns no overrides.
        assert!(evaluate_at(&mctx, &s, 2.0).is_empty());
    }

    #[test]
    fn multi_name_animations_merge_overrides() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @keyframes spin { from { transform: rotate(0deg) } to { transform: rotate(180deg) } }
            @keyframes fade { from { opacity: 0 } to { opacity: 1 } }
            "#,
        );
        let el = elem(
            "g",
            &[(
                "style",
                "animation-name: spin, fade; animation-duration: 1s, 1s; animation-timing-function: linear, linear",
            )],
        );
        let mctx = MatchContext::root(&el);
        let snap = evaluate_at(&mctx, &s, 0.5);
        // Both transform and opacity should appear.
        let has_transform = snap.iter().any(|(k, _)| k == "transform");
        let has_opacity = snap.iter().any(|(k, _)| k == "opacity");
        assert!(has_transform, "transform missing in {snap:?}");
        assert!(has_opacity, "opacity missing in {snap:?}");
        // opacity at midpoint of linear → 0.5.
        let op: f32 = snap
            .iter()
            .find(|(k, _)| k == "opacity")
            .unwrap()
            .1
            .parse()
            .unwrap();
        assert!((op - 0.5).abs() < 1e-3);
    }

    #[test]
    fn multi_name_per_animation_indexed_durations() {
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @keyframes a { from { opacity: 0 } to { opacity: 1 } }
            @keyframes b { from { fill-opacity: 0 } to { fill-opacity: 1 } }
            "#,
        );
        // a: 2s linear, b: 4s linear → at t=1s, a=0.5, b=0.25.
        let el = elem(
            "g",
            &[(
                "style",
                "animation-name: a, b; animation-duration: 2s, 4s; animation-timing-function: linear",
            )],
        );
        let mctx = MatchContext::root(&el);
        let snap = evaluate_at(&mctx, &s, 1.0);
        let op: f32 = snap
            .iter()
            .find(|(k, _)| k == "opacity")
            .unwrap()
            .1
            .parse()
            .unwrap();
        let fop: f32 = snap
            .iter()
            .find(|(k, _)| k == "fill-opacity")
            .unwrap()
            .1
            .parse()
            .unwrap();
        assert!((op - 0.5).abs() < 1e-3, "opacity 0.5, got {op}");
        assert!((fop - 0.25).abs() < 1e-3, "fill-opacity 0.25, got {fop}");
    }

    #[test]
    fn alternate_with_ease_in_out_at_quarter_seconds() {
        // Round-17 dispatch test scenario:
        // animation: spin 1s ease-in-out infinite alternate
        // at t=0.25, 0.5, 0.75 → progress reflects ease curve + dir.
        let mut s = Stylesheet::new();
        s.parse_block(
            r#"
            @keyframes spin { from { transform: rotate(0deg) } to { transform: rotate(360deg) } }
            "#,
        );
        let el = elem(
            "g",
            &[(
                "style",
                "animation-name: spin; animation-duration: 1s; animation-iteration-count: infinite; animation-timing-function: ease-in-out; animation-direction: alternate",
            )],
        );
        let mctx = MatchContext::root(&el);
        let parse_deg = |s: &str| -> f32 {
            // s is "rotate(<num>deg)"
            let s = s.trim_start_matches("rotate(").trim_end_matches(')');
            let s = s.trim_end_matches("deg");
            s.parse().unwrap()
        };
        // Iteration 0 (frac 0.25, mapped 0.25, ease-in-out is monotone
        // and slow at start → eased < 0.25).
        let v25 = parse_deg(&evaluate_at(&mctx, &s, 0.25)[0].1);
        assert!(
            v25 < 0.25 * 360.0,
            "ease at t=0.25 should be < 90deg, got {v25}"
        );
        // Midpoint of ease-in-out is exactly 0.5 → 180deg.
        let v50 = parse_deg(&evaluate_at(&mctx, &s, 0.5)[0].1);
        assert!((v50 - 180.0).abs() < 5.0, "midpoint ≈ 180, got {v50}");
        // t=0.75 → frac 0.75, eased > 0.75 → > 270.
        let v75 = parse_deg(&evaluate_at(&mctx, &s, 0.75)[0].1);
        assert!(
            v75 > 0.75 * 360.0,
            "ease at t=0.75 should be > 270deg, got {v75}"
        );
        // t=1.25 → iter 1, alternate flips → mapped 0.75, then eased.
        // Eased value should equal v75 mapped backward → roughly the
        // mirror of v75 (i.e. = 360 - v25).
        let v125 = parse_deg(&evaluate_at(&mctx, &s, 1.25)[0].1);
        assert!(
            (v125 - (360.0 - v25)).abs() < 5.0,
            "alternate iter1 mirror, got {v125} vs expected {}",
            360.0 - v25
        );
    }
}
