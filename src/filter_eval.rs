//! Round 283 — pixel-level evaluation of `<feDropShadow>` per the W3C
//! Filter Effects Module Level 1 §9.12 normative equivalent composite.
//!
//! §9.12 defines `feDropShadow` as a shorthand primitive whose result
//! is *equivalent to* the five-step composite
//!
//! ```text
//! <feGaussianBlur in="alpha-channel-of-feDropShadow-in"
//!                 stdDeviation="stdDeviation-of-feDropShadow"/>
//! <feOffset dx="dx-of-feDropShadow" dy="dy-of-feDropShadow"
//!           result="offsetblur"/>
//! <feFlood flood-color="flood-color-of-feDropShadow"
//!          flood-opacity="flood-opacity-of-feDropShadow"/>
//! <feComposite in2="offsetblur" operator="in"/>
//! <feMerge>
//!   <feMergeNode/>
//!   <feMergeNode in="in-of-feDropShadow"/>
//! </feMerge>
//! ```
//!
//! [`drop_shadow`] evaluates exactly those steps over a
//! [`FilterImage`] pixel buffer (the spec notes an implementation need
//! not materialise the tree; this one fuses steps 3–5 into one pass):
//!
//! 1. Take the alpha channel of the input.
//! 2. Blur it per §9.14 — the three-box-blur approximation of the
//!    Gaussian kernel ([`gaussian_blur`]), with the §9.14 initial
//!    `edgeMode` of `none` (out-of-image pixels read as transparent
//!    black).
//! 3. Offset the result by `dx` / `dy` per §9.18 ([`offset`]) —
//!    fractional offsets use bilinear interpolation, the §9.18
//!    recommendation for sub-pixel destinations.
//! 4. Flood with `flood-color` × `flood-opacity` (§9.13; per §9.13.2
//!    the flood colour's own alpha channel is *multiplied* with the
//!    computed flood-opacity) and composite it into the offset blur
//!    with the Porter-Duff `in` operator (§9.8: `in` = source where
//!    `in2` represents the destination, i.e. premultiplied
//!    `result = flood × blur-alpha`).
//! 5. Merge (§9.16): composite the input on top of the shadow with the
//!    Porter-Duff `over` operator (first merge node on the bottom).
//!
//! ## Colour space
//!
//! Per Filter Effects §10 the working colour space of a primitive is
//! its resolved `color-interpolation-filters` value, whose *initial*
//! value is `linearRGB`. The sRGB ↔ linearised-RGB transfer is the SVG
//! 2 §13.9 formula (`C_lin = C/12.92` for `C ≤ 0.04045`, else
//! `((C+0.055)/1.055)^2.4`; [`linear_to_srgb`] is its inverse with the
//! matching threshold `0.04045/12.92`). `auto` lets the implementation
//! choose either space (§10); this module resolves `auto` to the
//! initial `linearRGB`.
//!
//! ## Scope
//!
//! Input *resolution* (`in="SourceGraphic"` vs a `result` reference,
//! filter-region clipping, `primitiveUnits` scaling) is graph-level
//! plumbing that stays on the rasteriser side; the evaluator takes the
//! already-resolved input pixels for the primitive's `in` slot. Note
//! that per §9.12 that same input feeds *both* the alpha→blur chain
//! and the topmost merge node.

use crate::filter::{
    ColorInterpolationFilters, CompositeOperator, FilterPrimitive, FilterPrimitiveNode, FloodColor,
};

/// An RGBA pixel buffer used as a filter-primitive operand.
///
/// Pixels are stored row-major as **premultiplied** `f32` RGBA in the
/// nominal `[0, 1]` range, expressed in whatever working colour space
/// the caller decoded into (see [`FilterImage::from_rgba8`]).
/// Premultiplied storage is what makes the §9.14 blur and the §9.8 /
/// §9.16 Porter-Duff compositing direct per-channel arithmetic.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterImage {
    width: usize,
    height: usize,
    /// `width * height * 4` premultiplied RGBA components.
    data: Vec<f32>,
}

impl FilterImage {
    /// A `width × height` buffer of transparent black.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height * 4],
        }
    }

    /// Width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Premultiplied RGBA at `(x, y)`.
    ///
    /// # Panics
    ///
    /// Panics when `(x, y)` is outside the image.
    pub fn pixel(&self, x: usize, y: usize) -> [f32; 4] {
        assert!(x < self.width && y < self.height);
        let i = (y * self.width + x) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    /// Store premultiplied RGBA at `(x, y)`.
    ///
    /// # Panics
    ///
    /// Panics when `(x, y)` is outside the image.
    pub fn set_pixel(&mut self, x: usize, y: usize, rgba: [f32; 4]) {
        assert!(x < self.width && y < self.height);
        let i = (y * self.width + x) * 4;
        self.data[i..i + 4].copy_from_slice(&rgba);
    }

    /// Decode an 8-bit non-premultiplied sRGB-encoded RGBA buffer into
    /// the working colour space selected by `space` (Filter Effects
    /// §10): colour channels are linearised per the SVG 2 §13.9
    /// transfer when the working space is `linearRGB` (or `auto`, which
    /// resolves to the initial `linearRGB`), then premultiplied by
    /// alpha.
    ///
    /// Returns `None` when `rgba.len() != width * height * 4`.
    pub fn from_rgba8(
        width: usize,
        height: usize,
        rgba: &[u8],
        space: ColorInterpolationFilters,
    ) -> Option<Self> {
        if rgba.len() != width * height * 4 {
            return None;
        }
        let linear = working_space_is_linear(space);
        let mut data = Vec::with_capacity(rgba.len());
        for px in rgba.chunks_exact(4) {
            let a = px[3] as f32 / 255.0;
            for &c in &px[..3] {
                let mut v = c as f32 / 255.0;
                if linear {
                    v = srgb_to_linear(v);
                }
                data.push(v * a);
            }
            data.push(a);
        }
        Some(Self {
            width,
            height,
            data,
        })
    }

    /// Encode back to 8-bit non-premultiplied sRGB-encoded RGBA:
    /// un-premultiply, re-apply the SVG 2 §13.9 sRGB transfer when the
    /// working space was linear, clamp to `[0, 1]` and round to the
    /// nearest 8-bit value.
    pub fn to_rgba8(&self, space: ColorInterpolationFilters) -> Vec<u8> {
        let linear = working_space_is_linear(space);
        let mut out = Vec::with_capacity(self.data.len());
        for px in self.data.chunks_exact(4) {
            let a = px[3];
            for &c in &px[..3] {
                let mut v = if a > 0.0 { c / a } else { 0.0 };
                if linear {
                    v = linear_to_srgb(v);
                }
                out.push((v.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            out.push((a.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
        out
    }
}

/// `true` when filter maths happens on linearised values — `linearRGB`
/// is the initial value of `color-interpolation-filters` (Filter
/// Effects §10) and `auto` is resolved to it (§10 allows the
/// implementation to pick either space for `auto`).
fn working_space_is_linear(space: ColorInterpolationFilters) -> bool {
    !matches!(space, ColorInterpolationFilters::Srgb)
}

/// SVG 2 §13.9 sRGB → linearised-RGB transfer for one colour component
/// in `[0, 1]`.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Inverse of [`srgb_to_linear`] (threshold `0.04045 / 12.92` on the
/// linear side so the two branches meet where the forward branches do).
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.04045 / 12.92 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// §9.14 box size for one axis:
/// `d = floor(s * 3 * sqrt(2π) / 4 + 0.5)`.
fn box_size(s: f32) -> u32 {
    (s * 3.0 * (2.0 * std::f32::consts::PI).sqrt() / 4.0 + 0.5).floor() as u32
}

/// One box-blur pass along one axis: `out[i] = mean(in[i+lo ..= i+hi])`
/// with the §9.14 initial `edgeMode` of `none` (out-of-image samples
/// are transparent black).
fn box_blur_axis(src: &FilterImage, lo: isize, hi: isize, horizontal: bool) -> FilterImage {
    let n = (hi - lo + 1) as f32;
    let mut out = FilterImage::new(src.width, src.height);
    for y in 0..src.height {
        for x in 0..src.width {
            let mut acc = [0.0f32; 4];
            for t in lo..=hi {
                let (sx, sy) = if horizontal {
                    (x as isize + t, y as isize)
                } else {
                    (x as isize, y as isize + t)
                };
                if sx >= 0 && (sx as usize) < src.width && sy >= 0 && (sy as usize) < src.height {
                    let p = src.pixel(sx as usize, sy as usize);
                    for (a, v) in acc.iter_mut().zip(p) {
                        *a += v;
                    }
                }
            }
            out.set_pixel(x, y, acc.map(|v| v / n));
        }
    }
    out
}

/// `<feGaussianBlur>` per Filter Effects §9.14, using the spec's
/// three-box-blur piece-wise-quadratic approximation of the Gaussian
/// kernel. For each axis with standard deviation `s`:
///
/// * `d = floor(s * 3 * sqrt(2π) / 4 + 0.5)`;
/// * odd `d` — three box blurs of size `d` centred on the output pixel;
/// * even `d` — one box of size `d` centred on the boundary with the
///   pixel to the left, one centred on the boundary with the pixel to
///   the right, and one of size `d + 1` centred on the output pixel;
/// * `d = 0` — that axis is untouched.
///
/// Per §9.14 a negative `stdDeviation` disables the primitive (the
/// result is the input image), while a zero on only one axis blurs the
/// other axis alone. Edge handling is the §9.14 initial `edgeMode`
/// `none` (zero extension).
pub fn gaussian_blur(src: &FilterImage, std_dev_x: f32, std_dev_y: f32) -> FilterImage {
    let mut img = src.clone();
    if std_dev_x < 0.0 || std_dev_y < 0.0 {
        return img;
    }
    for (s, horizontal) in [(std_dev_x, true), (std_dev_y, false)] {
        let d = box_size(s) as isize;
        if d == 0 {
            continue;
        }
        if d % 2 == 1 {
            let r = (d - 1) / 2;
            for _ in 0..3 {
                img = box_blur_axis(&img, -r, r, horizontal);
            }
        } else {
            let h = d / 2;
            img = box_blur_axis(&img, -h, h - 1, horizontal);
            img = box_blur_axis(&img, -h + 1, h, horizontal);
            img = box_blur_axis(&img, -h, h, horizontal);
        }
    }
    img
}

/// `<feOffset>` per Filter Effects §9.18:
/// `out(x, y) = in(x - dx, y - dy)`.
///
/// Fractional offsets are resolved with bilinear interpolation (§9.18
/// recommends an interpolation technique when the destination is
/// offset by a fraction of a pixel); samples outside the input read as
/// transparent black.
pub fn offset(src: &FilterImage, dx: f32, dy: f32) -> FilterImage {
    let mut out = FilterImage::new(src.width, src.height);
    for y in 0..src.height {
        for x in 0..src.width {
            let fx = x as f32 - dx;
            let fy = y as f32 - dy;
            let x0 = fx.floor();
            let y0 = fy.floor();
            let tx = fx - x0;
            let ty = fy - y0;
            let mut acc = [0.0f32; 4];
            for (ox, oy, w) in [
                (0, 0, (1.0 - tx) * (1.0 - ty)),
                (1, 0, tx * (1.0 - ty)),
                (0, 1, (1.0 - tx) * ty),
                (1, 1, tx * ty),
            ] {
                if w <= 0.0 {
                    continue;
                }
                let sx = x0 as isize + ox;
                let sy = y0 as isize + oy;
                if sx >= 0 && (sx as usize) < src.width && sy >= 0 && (sy as usize) < src.height {
                    let p = src.pixel(sx as usize, sy as usize);
                    for (a, v) in acc.iter_mut().zip(p) {
                        *a += w * v;
                    }
                }
            }
            out.set_pixel(x, y, acc);
        }
    }
    out
}

/// `<feComposite>` per Filter Effects §16 / SVG 1.1 §15.12 — the
/// pixel-wise combination of two operands `i1` (`in`) and `i2` (`in2`).
///
/// `i1` is the **source** and `i2` the **destination** (SVG 1.1 §15.12:
/// "with `in` representing the source and `in2` representing the
/// destination"). Both operands must share the same dimensions; `i2` is
/// truncated/zero-extended to `i1`'s size and the result has `i1`'s
/// dimensions.
///
/// ## Operators
///
/// Only the two operators the staged specifications define **inline**
/// are evaluated here:
///
/// * [`CompositeOperator::Over`] — the Porter-Duff `over`. SVG 1.1
///   §15.9 states "'normal' blend mode is equivalent to
///   `operator="over"`", and gives the premultiplied `normal`/`over`
///   formulae directly: `qr = 1 − (1 − qa)·(1 − qb)` for the result
///   opacity and `cr = (1 − qa)·cb + ca` for each premultiplied colour
///   channel, where image A is the source (`i1`) and image B is the
///   destination (`i2`).
/// * [`CompositeOperator::Arithmetic`] — the component-wise
///   `result = k1·i1·i2 + k2·i1 + k3·i2 + k4`, clamped to `[0, 1]`,
///   given inline in both Filter Effects §16 and SVG 1.1 §15.12 (with
///   `k1..k4` defaulting to `0`). Per the spec the arithmetic operator
///   is applied to every channel including alpha, on the premultiplied
///   operands, then clamped.
///
/// The remaining Porter-Duff operators (`in`, `out`, `atop`, `xor`,
/// `lighter`) have their formula bodies in the referenced
/// `[PORTERDUFF]` / `[COMPOSITING-1]` companion specification, which is
/// not staged under `docs/`; [`composite`] leaves those operands
/// unevaluated (returns `i1` unchanged) rather than guess the factors.
pub fn composite(
    i1: &FilterImage,
    i2: &FilterImage,
    op: CompositeOperator,
    k: [f32; 4],
) -> FilterImage {
    let mut out = FilterImage::new(i1.width, i1.height);
    for y in 0..i1.height {
        for x in 0..i1.width {
            let a = i1.pixel(x, y);
            let b = if x < i2.width && y < i2.height {
                i2.pixel(x, y)
            } else {
                [0.0; 4]
            };
            let px = match op {
                // SVG 1.1 §15.9: cr = (1 − qa)·cb + ca per channel,
                // qr = 1 − (1 − qa)·(1 − qb). With premultiplied storage
                // the colour and alpha rows are the same expression, so
                // the four-component map covers both (the alpha row is
                // 1 − (1 − qa)·(1 − qb) = (1 − qa)·qb + qa).
                CompositeOperator::Over => {
                    let inv_qa = 1.0 - a[3];
                    [
                        (1.0 - a[3]) * b[0] + a[0],
                        (1.0 - a[3]) * b[1] + a[1],
                        (1.0 - a[3]) * b[2] + a[2],
                        inv_qa * b[3] + a[3],
                    ]
                }
                // §16 / §15.12: result = k1·i1·i2 + k2·i1 + k3·i2 + k4,
                // clamped to [0, 1], applied per channel (alpha included).
                CompositeOperator::Arithmetic => {
                    let mut r = [0.0f32; 4];
                    for c in 0..4 {
                        r[c] =
                            (k[0] * a[c] * b[c] + k[1] * a[c] + k[2] * b[c] + k[3]).clamp(0.0, 1.0);
                    }
                    r
                }
                // in / out / atop / xor / lighter: formula bodies live in
                // the un-staged [PORTERDUFF] reference — left unevaluated.
                _ => a,
            };
            out.set_pixel(x, y, px);
        }
    }
    out
}

/// Evaluate a parsed [`FilterPrimitiveNode`] holding a
/// [`FilterPrimitive::Composite`] over two 8-bit non-premultiplied
/// sRGB-encoded RGBA buffers (`i1` ← `in`, `i2` ← `in2`), in the node's
/// resolved `color-interpolation-filters` working space.
///
/// Both buffers must be `width × height × 4`. Returns `None` when the
/// node is not a composite, when a buffer length is wrong, or when the
/// operator is one of the un-staged Porter-Duff factors
/// (`in`/`out`/`atop`/`xor`) — callers can then fall back to the
/// graph-level rasteriser for those.
pub fn evaluate_composite_node(
    node: &FilterPrimitiveNode,
    in1_rgba8: &[u8],
    in2_rgba8: &[u8],
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    let FilterPrimitive::Composite {
        operator,
        k1,
        k2,
        k3,
        k4,
        ..
    } = &node.primitive
    else {
        return None;
    };
    if !matches!(
        operator,
        CompositeOperator::Over | CompositeOperator::Arithmetic
    ) {
        return None;
    }
    let space = node.color_interpolation_filters;
    let i1 = FilterImage::from_rgba8(width, height, in1_rgba8, space)?;
    let i2 = FilterImage::from_rgba8(width, height, in2_rgba8, space)?;
    Some(composite(&i1, &i2, *operator, [*k1, *k2, *k3, *k4]).to_rgba8(space))
}

/// The `<feDropShadow>` attribute set consumed by [`drop_shadow`].
///
/// `Default` carries the §9.12 initial values: `dx = dy = 2`,
/// `stdDeviation = 2` (both axes), opaque-black `flood-color` (§9.13.1
/// initial) and `flood-opacity = 1` (§9.13.2 initial).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropShadowParams {
    /// `dx` — x offset of the shadow, forwarded to the internal
    /// `<feOffset>` (§9.12).
    pub dx: f32,
    /// `dy` — y offset of the shadow.
    pub dy: f32,
    /// `stdDeviation` x component, forwarded to the internal
    /// `<feGaussianBlur>`.
    pub std_deviation_x: f32,
    /// `stdDeviation` y component.
    pub std_deviation_y: f32,
    /// `flood-color`, an sRGB-encoded value (decoded into the working
    /// space by [`drop_shadow`]).
    pub flood_color: FloodColor,
    /// `flood-opacity`, clamped to `[0, 1]` per §9.13.2; multiplied
    /// with the flood colour's own alpha channel.
    pub flood_opacity: f32,
}

impl Default for DropShadowParams {
    fn default() -> Self {
        Self {
            dx: 2.0,
            dy: 2.0,
            std_deviation_x: 2.0,
            std_deviation_y: 2.0,
            flood_color: FloodColor::default(),
            flood_opacity: 1.0,
        }
    }
}

/// Evaluate `<feDropShadow>` per the Filter Effects §9.12 equivalent
/// composite (see the module docs for the five steps).
///
/// `src` is the resolved input of the primitive's `in` slot — it feeds
/// both the alpha→blur→offset shadow chain and the topmost §9.16 merge
/// node. `space` is the resolved `color-interpolation-filters` value
/// (§10) and must match the space `src` was decoded into; it governs
/// how the flood colour (an sRGB-encoded value) is brought into the
/// working space before the §9.8 `in` composite.
pub fn drop_shadow(
    src: &FilterImage,
    params: &DropShadowParams,
    space: ColorInterpolationFilters,
) -> FilterImage {
    let DropShadowParams {
        dx,
        dy,
        std_deviation_x,
        std_deviation_y,
        flood_color,
        flood_opacity,
    } = *params;
    // Step 1 — the alpha channel of the input: transparent-black colour
    // channels, input alpha (already identical in premultiplied form).
    let mut alpha = FilterImage::new(src.width, src.height);
    for (dst, s) in alpha.data.chunks_exact_mut(4).zip(src.data.chunks_exact(4)) {
        dst[3] = s[3];
    }
    // Step 2 — §9.14 Gaussian blur.
    let blurred = gaussian_blur(&alpha, std_deviation_x, std_deviation_y);
    // Step 3 — §9.18 offset.
    let moved = offset(&blurred, dx, dy);
    // Step 4 — §9.13 flood, composited with the §9.8 Porter-Duff `in`
    // operator (flood is the source, the offset blur is `in2`, the
    // destination): premultiplied result = flood × destination-alpha.
    // §9.13.2: the flood colour's alpha channel is multiplied with the
    // computed flood-opacity.
    let fa = (flood_color.a as f32 / 255.0) * flood_opacity.clamp(0.0, 1.0);
    let linear = working_space_is_linear(space);
    let decode = |c: u8| {
        let v = c as f32 / 255.0;
        if linear {
            srgb_to_linear(v)
        } else {
            v
        }
    };
    let flood = [
        decode(flood_color.r),
        decode(flood_color.g),
        decode(flood_color.b),
        1.0,
    ];
    // Step 5 — §9.16 merge: shadow on the bottom, input on top,
    // composited with the Porter-Duff `over` operator
    // (premultiplied: out = top + bottom × (1 − top-alpha)).
    let mut out = FilterImage::new(src.width, src.height);
    for ((dst, top), shadow_in) in out
        .data
        .chunks_exact_mut(4)
        .zip(src.data.chunks_exact(4))
        .zip(moved.data.chunks_exact(4))
    {
        let shadow_a = shadow_in[3] * fa;
        let inv_top_a = 1.0 - top[3];
        for ((d, &t), &f) in dst.iter_mut().zip(top).zip(&flood) {
            *d = t + f * shadow_a * inv_top_a;
        }
    }
    out
}

/// Evaluate a parsed [`FilterPrimitiveNode`] holding a
/// [`FilterPrimitive::DropShadow`] over an 8-bit non-premultiplied
/// sRGB-encoded RGBA buffer, in the node's resolved
/// `color-interpolation-filters` working space.
///
/// Returns `None` when the node is not a drop shadow or when
/// `source_rgba8.len() != width * height * 4`.
pub fn evaluate_drop_shadow_node(
    node: &FilterPrimitiveNode,
    source_rgba8: &[u8],
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    let FilterPrimitive::DropShadow {
        dx,
        dy,
        std_deviation_x,
        std_deviation_y,
        flood_color,
        flood_opacity,
        ..
    } = &node.primitive
    else {
        return None;
    };
    let space = node.color_interpolation_filters;
    let src = FilterImage::from_rgba8(width, height, source_rgba8, space)?;
    let params = DropShadowParams {
        dx: *dx,
        dy: *dy,
        std_deviation_x: *std_deviation_x,
        std_deviation_y: *std_deviation_y,
        flood_color: *flood_color,
        flood_opacity: *flood_opacity,
    };
    Some(drop_shadow(&src, &params, space).to_rgba8(space))
}

#[cfg(test)]
mod tests {
    use super::*;

    // §9.14 — d = floor(s * 3 * sqrt(2π) / 4 + 0.5).
    #[test]
    fn box_size_formula() {
        // 3·sqrt(2π)/4 ≈ 1.8800: s=2 → floor(4.2599) = 4,
        // s=0.8 → floor(2.0040) = 2, s=0.4 → floor(1.2520) = 1,
        // s=0.2 → floor(0.8760) = 0.
        assert_eq!(box_size(2.0), 4);
        assert_eq!(box_size(0.8), 2);
        assert_eq!(box_size(0.4), 1);
        assert_eq!(box_size(0.2), 0);
        assert_eq!(box_size(0.0), 0);
    }

    #[test]
    fn srgb_transfer_round_trips() {
        for i in 0..=255u32 {
            let v = i as f32 / 255.0;
            let rt = linear_to_srgb(srgb_to_linear(v));
            assert!((rt - v).abs() < 1e-5, "{i}: {rt} vs {v}");
        }
    }

    // Three box blurs of size 1 are the identity (s small but > 0).
    #[test]
    fn blur_d1_is_identity() {
        let mut img = FilterImage::new(5, 5);
        img.set_pixel(2, 2, [0.25, 0.5, 0.75, 1.0]);
        let out = gaussian_blur(&img, 0.4, 0.4);
        assert_eq!(out, img);
    }

    // §9.14: negative stdDeviation disables the primitive.
    #[test]
    fn blur_negative_std_dev_is_input() {
        let mut img = FilterImage::new(3, 3);
        img.set_pixel(1, 1, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(gaussian_blur(&img, -1.0, 3.0), img);
        assert_eq!(gaussian_blur(&img, 3.0, -0.5), img);
    }

    // Even-d impulse response, 1-D: d=2 (s=0.8) gives the two
    // boundary-centred size-2 boxes ([1/4, 1/2, 1/4] combined) followed
    // by a size-3 centred box → [1/12, 1/4, 1/3, 1/4, 1/12].
    #[test]
    fn blur_even_d_impulse_kernel() {
        let mut img = FilterImage::new(9, 1);
        img.set_pixel(4, 0, [0.0, 0.0, 0.0, 1.0]);
        let out = gaussian_blur(&img, 0.8, 0.0);
        let expect = [
            0.0,
            0.0,
            1.0 / 12.0,
            0.25,
            1.0 / 3.0,
            0.25,
            1.0 / 12.0,
            0.0,
            0.0,
        ];
        for (x, e) in expect.iter().enumerate() {
            let a = out.pixel(x, 0)[3];
            assert!((a - e).abs() < 1e-6, "x={x}: {a} vs {e}");
        }
    }

    // Separable 2-D: alpha(x, y) = k(x)·k(y) for the same impulse.
    #[test]
    fn blur_even_d_impulse_separable() {
        let mut img = FilterImage::new(9, 9);
        img.set_pixel(4, 4, [0.0, 0.0, 0.0, 1.0]);
        let out = gaussian_blur(&img, 0.8, 0.8);
        let k = [1.0 / 12.0, 0.25, 1.0 / 3.0, 0.25, 1.0 / 12.0];
        for (i, kx) in k.iter().enumerate() {
            for (j, ky) in k.iter().enumerate() {
                let a = out.pixel(2 + i, 2 + j)[3];
                assert!((a - kx * ky).abs() < 1e-6, "({i},{j}): {a}");
            }
        }
        // Off-support pixels stay empty.
        assert_eq!(out.pixel(0, 4)[3], 0.0);
        assert_eq!(out.pixel(4, 8)[3], 0.0);
    }

    // The §9.14 approximation kernel is normalised: away from the image
    // border (edgeMode `none` zero-extension) the blurred alpha mass
    // equals the input alpha mass. s=2 → boxes 4+4+5 → support 11.
    #[test]
    fn blur_default_s2_preserves_mass() {
        let mut img = FilterImage::new(21, 21);
        img.set_pixel(10, 10, [0.0, 0.0, 0.0, 1.0]);
        let out = gaussian_blur(&img, 2.0, 2.0);
        let mass: f32 = (0..21)
            .flat_map(|y| (0..21).map(move |x| (x, y)))
            .map(|(x, y)| out.pixel(x, y)[3])
            .sum();
        assert!((mass - 1.0).abs() < 1e-4, "mass {mass}");
        // Even-d pairing (left + right boundary boxes) keeps the kernel
        // symmetric about the impulse.
        for k in 1..=5 {
            let l = out.pixel(10 - k, 10)[3];
            let r = out.pixel(10 + k, 10)[3];
            assert!((l - r).abs() < 1e-6, "k={k}: {l} vs {r}");
        }
    }

    #[test]
    fn offset_integer_moves_pixels() {
        let mut img = FilterImage::new(4, 4);
        img.set_pixel(1, 1, [0.5, 0.25, 0.125, 0.5]);
        let out = offset(&img, 2.0, 1.0);
        assert_eq!(out.pixel(3, 2), [0.5, 0.25, 0.125, 0.5]);
        assert_eq!(out.pixel(1, 1), [0.0; 4]);
    }

    #[test]
    fn offset_fractional_is_bilinear() {
        let mut img = FilterImage::new(4, 1);
        img.set_pixel(1, 0, [0.0, 0.0, 0.0, 1.0]);
        let out = offset(&img, 0.5, 0.0);
        assert!((out.pixel(1, 0)[3] - 0.5).abs() < 1e-6);
        assert!((out.pixel(2, 0)[3] - 0.5).abs() < 1e-6);
        assert_eq!(out.pixel(0, 0)[3], 0.0);
        assert_eq!(out.pixel(3, 0)[3], 0.0);
    }

    #[test]
    fn offset_out_of_image_is_transparent_black() {
        let mut img = FilterImage::new(2, 2);
        img.set_pixel(0, 0, [0.0, 0.0, 0.0, 1.0]);
        let out = offset(&img, -1.0, 0.0);
        // The lone pixel moved off the left edge; nothing wrapped in.
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(out.pixel(x, y), [0.0; 4], "({x},{y})");
            }
        }
    }

    #[test]
    fn from_rgba8_rejects_bad_length() {
        assert!(FilterImage::from_rgba8(2, 2, &[0; 15], ColorInterpolationFilters::Srgb).is_none());
    }

    // §15.9/§15.12 over: opaque source fully replaces the destination
    // (qr = 1, cr = ca since 1 − qa = 0).
    #[test]
    fn composite_over_opaque_source_replaces() {
        let mut a = FilterImage::new(1, 1);
        a.set_pixel(0, 0, [0.3, 0.4, 0.5, 1.0]);
        let mut b = FilterImage::new(1, 1);
        b.set_pixel(0, 0, [0.9, 0.9, 0.9, 1.0]);
        let out = composite(&a, &b, CompositeOperator::Over, [0.0; 4]);
        assert_eq!(out.pixel(0, 0), [0.3, 0.4, 0.5, 1.0]);
    }

    // over: transparent source leaves the destination untouched
    // (1 − qa = 1, ca = 0 → cr = cb, qr = qb).
    #[test]
    fn composite_over_transparent_source_is_destination() {
        let a = FilterImage::new(1, 1); // transparent black
        let mut b = FilterImage::new(1, 1);
        b.set_pixel(0, 0, [0.2, 0.4, 0.6, 0.8]);
        let out = composite(&a, &b, CompositeOperator::Over, [0.0; 4]);
        let p = out.pixel(0, 0);
        for (o, e) in p.iter().zip([0.2, 0.4, 0.6, 0.8]) {
            assert!((o - e).abs() < 1e-6, "{o} vs {e}");
        }
    }

    // over: half-alpha source over opaque-white destination.
    // qa = 0.5, premult ca = [0.5,0,0,0.5]; cb = [1,1,1,1], qb = 1.
    // cr = (1−0.5)·cb + ca = [1.0, 0.5, 0.5, 1.0]; qr = 1.
    #[test]
    fn composite_over_partial_alpha_blend() {
        let mut a = FilterImage::new(1, 1);
        a.set_pixel(0, 0, [0.5, 0.0, 0.0, 0.5]);
        let mut b = FilterImage::new(1, 1);
        b.set_pixel(0, 0, [1.0, 1.0, 1.0, 1.0]);
        let out = composite(&a, &b, CompositeOperator::Over, [0.0; 4]);
        let p = out.pixel(0, 0);
        for (o, e) in p.iter().zip([1.0, 0.5, 0.5, 1.0]) {
            assert!((o - e).abs() < 1e-6, "{o} vs {e}");
        }
    }

    // §16/§15.12 arithmetic: result = k1·i1·i2 + k2·i1 + k3·i2 + k4.
    // k = (0,1,1,0) is the additive "lighter"-style sum i1 + i2.
    #[test]
    fn composite_arithmetic_add() {
        let mut a = FilterImage::new(1, 1);
        a.set_pixel(0, 0, [0.2, 0.0, 0.0, 0.2]);
        let mut b = FilterImage::new(1, 1);
        b.set_pixel(0, 0, [0.3, 0.0, 0.0, 0.3]);
        let out = composite(&a, &b, CompositeOperator::Arithmetic, [0.0, 1.0, 1.0, 0.0]);
        let p = out.pixel(0, 0);
        for (o, e) in p.iter().zip([0.5, 0.0, 0.0, 0.5]) {
            assert!((o - e).abs() < 1e-6, "{o} vs {e}");
        }
    }

    // arithmetic clamps the per-channel result to [0, 1].
    #[test]
    fn composite_arithmetic_clamps() {
        let mut a = FilterImage::new(1, 1);
        a.set_pixel(0, 0, [0.8, 0.0, 0.0, 0.8]);
        let mut b = FilterImage::new(1, 1);
        b.set_pixel(0, 0, [0.8, 0.0, 0.0, 0.8]);
        // k4 bias pushes channels above 1 (clamp) and the green
        // channel below 0 via negative k4 elsewhere is covered by the
        // clamp branch too.
        let out = composite(&a, &b, CompositeOperator::Arithmetic, [0.0, 1.0, 1.0, 0.5]);
        assert_eq!(out.pixel(0, 0)[0], 1.0);
        // k2·i1 + k4 on the (zero) green channel = 0 + 0.5 = 0.5.
        assert!((out.pixel(0, 0)[1] - 0.5).abs() < 1e-6);
    }

    // k1·i1·i2 product term: i1 = i2 = 1 (alpha) → k1·1·1 = k1.
    #[test]
    fn composite_arithmetic_product_term() {
        let mut a = FilterImage::new(1, 1);
        a.set_pixel(0, 0, [0.0, 0.0, 0.0, 1.0]);
        let mut b = FilterImage::new(1, 1);
        b.set_pixel(0, 0, [0.0, 0.0, 0.0, 1.0]);
        let out = composite(&a, &b, CompositeOperator::Arithmetic, [0.5, 0.0, 0.0, 0.0]);
        assert!((out.pixel(0, 0)[3] - 0.5).abs() < 1e-6);
    }

    // Un-staged Porter-Duff operators pass i1 through unchanged.
    #[test]
    fn composite_unstaged_operator_returns_source() {
        let mut a = FilterImage::new(1, 1);
        a.set_pixel(0, 0, [0.1, 0.2, 0.3, 0.4]);
        let mut b = FilterImage::new(1, 1);
        b.set_pixel(0, 0, [0.9, 0.9, 0.9, 0.9]);
        for op in [
            CompositeOperator::In,
            CompositeOperator::Out,
            CompositeOperator::Atop,
            CompositeOperator::Xor,
        ] {
            assert_eq!(composite(&a, &b, op, [0.0; 4]), a, "{op:?}");
        }
    }

    // Node entry point: arithmetic add of two opaque sRGB greys in the
    // sRGB working space (no linearisation) sums the premultiplied
    // channels. 0x40 + 0x40 = 0x80.
    #[test]
    fn evaluate_composite_node_arithmetic_srgb() {
        let node = FilterPrimitiveNode {
            region: Default::default(),
            result: None,
            color_interpolation_filters: ColorInterpolationFilters::Srgb,
            primitive: FilterPrimitive::Composite {
                input: crate::filter::FilterInput::SourceGraphic,
                input2: crate::filter::FilterInput::SourceGraphic,
                operator: CompositeOperator::Arithmetic,
                k1: 0.0,
                k2: 1.0,
                k3: 1.0,
                k4: 0.0,
            },
        };
        let i1 = [0x40, 0x40, 0x40, 0xFF];
        let i2 = [0x40, 0x40, 0x40, 0xFF];
        let out = evaluate_composite_node(&node, &i1, &i2, 1, 1).unwrap();
        // Premult sum: 0x40/255 + 0x40/255 = 0x80/255 → 0x80; alpha
        // 1 + 1 clamps to 1 → 0xFF.
        assert_eq!(&out[..3], &[0x80, 0x80, 0x80]);
        assert_eq!(out[3], 0xFF);
    }

    // Node entry point declines the un-staged operators so the caller
    // can fall back to the graph-level rasteriser.
    #[test]
    fn evaluate_composite_node_declines_unstaged() {
        let node = FilterPrimitiveNode {
            region: Default::default(),
            result: None,
            color_interpolation_filters: ColorInterpolationFilters::Srgb,
            primitive: FilterPrimitive::Composite {
                input: crate::filter::FilterInput::SourceGraphic,
                input2: crate::filter::FilterInput::SourceGraphic,
                operator: CompositeOperator::In,
                k1: 0.0,
                k2: 0.0,
                k3: 0.0,
                k4: 0.0,
            },
        };
        assert!(evaluate_composite_node(&node, &[0; 4], &[0; 4], 1, 1).is_none());
    }
}
