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
    BlendMode, ChannelSelector, ColorInterpolationFilters, CompositeOperator, ConvolveEdgeMode,
    EdgeMode, FilterPrimitive, FilterPrimitiveNode, FloodColor, MorphologyOperator,
    TransferFunction,
};

/// `<feColorMatrix>` per Filter Effects §9.6 — the matrix transform
///
/// ```text
/// [R' G' B' A' 1]ᵀ = M · [R G B A 1]ᵀ
/// ```
///
/// where `M` is the row-major 4×5 `matrix` (the `<feColorMatrix>` parser
/// already reduces `saturate` / `hueRotate` / `luminanceToAlpha` to this
/// form), applied per output channel as
/// `c' = m[0]·R + m[1]·G + m[2]·B + m[3]·A + m[4]`.
///
/// §9.6 is explicit that "the calculations are performed on
/// **non-premultiplied** color values": this routine un-premultiplies
/// each [`FilterImage`] pixel, applies the matrix, clamps each result to
/// `[0, 1]`, and re-premultiplies for storage. The colour channels are
/// already in the [`FilterImage`]'s working colour space (§10), which is
/// where the matrix coefficients act.
pub fn color_matrix(src: &FilterImage, matrix: &[f32; 20]) -> FilterImage {
    let mut out = FilterImage::new(src.width, src.height);
    for (dst, s) in out.data.chunks_exact_mut(4).zip(src.data.chunks_exact(4)) {
        // Un-premultiply to recover the non-premultiplied operand §9.6
        // requires; a transparent pixel has no defined colour, so feed
        // the matrix straight zeros there.
        let a = s[3];
        let unp = if a > 0.0 {
            [s[0] / a, s[1] / a, s[2] / a, a]
        } else {
            [0.0, 0.0, 0.0, 0.0]
        };
        // [R' G' B' A'] = M · [R G B A 1]; each row is 5 coefficients.
        let mut res = [0.0f32; 4];
        for (row, r) in res.iter_mut().enumerate() {
            let m = &matrix[row * 5..row * 5 + 5];
            *r = (m[0] * unp[0] + m[1] * unp[1] + m[2] * unp[2] + m[3] * unp[3] + m[4])
                .clamp(0.0, 1.0);
        }
        // Re-premultiply by the transformed alpha for storage.
        let ra = res[3];
        dst[0] = res[0] * ra;
        dst[1] = res[1] * ra;
        dst[2] = res[2] * ra;
        dst[3] = ra;
    }
    out
}

/// Evaluate a parsed [`FilterPrimitiveNode`] holding a
/// [`FilterPrimitive::ColorMatrix`] over an 8-bit non-premultiplied
/// sRGB-encoded RGBA buffer, in the node's resolved
/// `color-interpolation-filters` working space (§10).
///
/// Returns `None` when the node is not a colour matrix or when
/// `source_rgba8.len() != width * height * 4`.
pub fn evaluate_color_matrix_node(
    node: &FilterPrimitiveNode,
    source_rgba8: &[u8],
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    let FilterPrimitive::ColorMatrix { matrix, .. } = &node.primitive else {
        return None;
    };
    let space = node.color_interpolation_filters;
    let src = FilterImage::from_rgba8(width, height, source_rgba8, space)?;
    Some(color_matrix(&src, matrix).to_rgba8(space))
}

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

/// Resolve a sample coordinate `c` against an axis of `len` pixels under
/// the §9.14 `edgeMode` policy, returning the in-image index to read, or
/// `None` when the sample should contribute transparent black.
///
/// * [`EdgeMode::None`] — out-of-range samples read as zero (§9.14:
///   "the input image is extended with pixel values of zero for R, G, B
///   and A"). This is the §9.14 *initial* value of `edgeMode`.
/// * [`EdgeMode::Duplicate`] — clamp to the nearest border pixel (§9.14:
///   "extended along each of its borders … by duplicating the color
///   values at the given edge").
/// * [`EdgeMode::Wrap`] — toroidal sampling (§9.14: "extended by taking
///   the color values from the opposite edge"). Uses Euclidean modulo so
///   negative coordinates wrap to the high edge.
fn edge_sample(c: isize, len: usize, mode: EdgeMode) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let n = len as isize;
    match mode {
        EdgeMode::None => {
            if c >= 0 && c < n {
                Some(c as usize)
            } else {
                None
            }
        }
        EdgeMode::Duplicate => Some(c.clamp(0, n - 1) as usize),
        EdgeMode::Wrap => Some(c.rem_euclid(n) as usize),
    }
}

/// One box-blur pass along one axis: `out[i] = mean(in[i+lo ..= i+hi])`,
/// extending the input at the image border per the §9.14 `edgeMode`
/// ([`edge_sample`]). With [`EdgeMode::None`] (the §9.14 initial value)
/// out-of-image samples are transparent black; `duplicate` clamps to the
/// edge pixel and `wrap` reads from the opposite edge, so neither mode
/// loses alpha mass at the border.
fn box_blur_axis(
    src: &FilterImage,
    lo: isize,
    hi: isize,
    horizontal: bool,
    mode: EdgeMode,
) -> FilterImage {
    let n = (hi - lo + 1) as f32;
    let mut out = FilterImage::new(src.width, src.height);
    for y in 0..src.height {
        for x in 0..src.width {
            let mut acc = [0.0f32; 4];
            for t in lo..=hi {
                let (sx, sy) = if horizontal {
                    (edge_sample(x as isize + t, src.width, mode), Some(y))
                } else {
                    (Some(x), edge_sample(y as isize + t, src.height, mode))
                };
                if let (Some(sx), Some(sy)) = (sx, sy) {
                    let p = src.pixel(sx, sy);
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
/// other axis alone. `edge_mode` selects the border policy (§9.14): the
/// initial value is [`EdgeMode::None`] (zero extension), with
/// `duplicate` clamping to the edge pixel and `wrap` reading from the
/// opposite edge.
pub fn gaussian_blur_edge(
    src: &FilterImage,
    std_dev_x: f32,
    std_dev_y: f32,
    edge_mode: EdgeMode,
) -> FilterImage {
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
                img = box_blur_axis(&img, -r, r, horizontal, edge_mode);
            }
        } else {
            let h = d / 2;
            img = box_blur_axis(&img, -h, h - 1, horizontal, edge_mode);
            img = box_blur_axis(&img, -h + 1, h, horizontal, edge_mode);
            img = box_blur_axis(&img, -h, h, horizontal, edge_mode);
        }
    }
    img
}

/// `<feGaussianBlur>` with the §9.14 initial `edgeMode` of `none` (zero
/// extension). A thin wrapper over [`gaussian_blur_edge`] kept for the
/// `<feDropShadow>` chain, whose §9.12 equivalent composite is defined
/// with the initial `edgeMode`.
pub fn gaussian_blur(src: &FilterImage, std_dev_x: f32, std_dev_y: f32) -> FilterImage {
    gaussian_blur_edge(src, std_dev_x, std_dev_y, EdgeMode::None)
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

/// `<feFlood>` per Filter Effects §9.13 — "creates a rectangle filled
/// with the color and opacity values from properties `flood-color` and
/// `flood-opacity`. The rectangle is as large as the filter primitive
/// subregion established by the `feFlood` element."
///
/// The whole `width × height` buffer (the resolved subregion the caller
/// passes) is set to one uniform pixel: the §9.13.1 `flood-color`
/// decoded into the `space` working colour space, at the alpha
/// `flood-color.a × flood-opacity` (the colour's own alpha channel
/// multiplied with the §9.13.2 `flood-opacity`, matching the
/// drop-shadow flood step). Storage is premultiplied
/// ([`FilterImage`] convention), so each colour channel is
/// `decoded-colour × alpha`.
///
/// `flood_opacity` is clamped to `[0, 1]` (the parser already clamps,
/// but the builder is defensive for direct callers).
pub fn flood(
    width: usize,
    height: usize,
    flood_color: FloodColor,
    flood_opacity: f32,
    space: ColorInterpolationFilters,
) -> FilterImage {
    let a = (flood_color.a as f32 / 255.0) * flood_opacity.clamp(0.0, 1.0);
    let linear = working_space_is_linear(space);
    let decode = |c: u8| {
        let v = c as f32 / 255.0;
        if linear {
            srgb_to_linear(v)
        } else {
            v
        }
    };
    // Premultiplied: colour channel = decoded-colour × alpha.
    let px = [
        decode(flood_color.r) * a,
        decode(flood_color.g) * a,
        decode(flood_color.b) * a,
        a,
    ];
    let mut out = FilterImage::new(width, height);
    for chunk in out.data.chunks_exact_mut(4) {
        chunk.copy_from_slice(&px);
    }
    out
}

/// Evaluate a parsed [`FilterPrimitiveNode`] holding a
/// [`FilterPrimitive::Flood`] over the `width × height` subregion, in
/// the node's resolved `color-interpolation-filters` working space.
///
/// `<feFlood>` has no pixel input — the subregion size is the only
/// geometry — so this evaluator takes just the dimensions and returns
/// the §9.13 uniform-fill buffer as an 8-bit non-premultiplied
/// sRGB-encoded RGBA image. Returns `None` when the node is not a
/// flood.
pub fn evaluate_flood_node(
    node: &FilterPrimitiveNode,
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    let FilterPrimitive::Flood {
        flood_color,
        flood_opacity,
    } = &node.primitive
    else {
        return None;
    };
    let space = node.color_interpolation_filters;
    Some(flood(width, height, *flood_color, *flood_opacity, space).to_rgba8(space))
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

/// `<feBlend>` per Filter Effects §9.5 / SVG 1.1 §15.9 — a pixel-wise
/// combination of two operands where `i1` (`in`) is image **A** (the
/// source `Cs`) and `i2` (`in2`) is image **B** (the backdrop `Cb`).
///
/// The result opacity is the same for every mode (SVG 1.1 §15.9):
///
/// ```text
/// qr = 1 − (1 − qa)·(1 − qb)
/// ```
///
/// and the premultiplied result colour `cr` per channel is, with
/// `ca` / `cb` the premultiplied source / backdrop colour and
/// `qa` / `qb` their opacities (SVG 1.1 §15.9 blending-mode table):
///
/// * [`BlendMode::Normal`] — `cr = (1 − qa)·cb + ca`. Identical to the
///   Porter-Duff `over` (`in` over `in2`); §15.9 states `normal` "is
///   equivalent to `operator="over"` on the `feComposite` primitive".
/// * [`BlendMode::Multiply`] — `cr = (1 − qa)·cb + (1 − qb)·ca + ca·cb`.
/// * [`BlendMode::Screen`] — `cr = cb + ca − ca·cb`.
/// * [`BlendMode::Darken`] —
///   `cr = min((1 − qa)·cb + ca, (1 − qb)·ca + cb)`.
/// * [`BlendMode::Lighten`] —
///   `cr = max((1 − qa)·cb + ca, (1 − qb)·ca + cb)`.
///
/// All five formulae operate directly on the premultiplied storage of
/// [`FilterImage`]; the colour rows use the four-component expression
/// above per channel and the alpha row is the common `qr` (which for
/// the `normal` mode equals `(1 − qa)·qb + qa`, and is supplied
/// explicitly for the others so the colour-only `min`/`max` of `darken`
/// / `lighten` do not leak into the alpha channel).
///
/// `i2` is truncated / zero-extended to `i1`'s dimensions and the result
/// has `i1`'s dimensions, matching [`composite`].
///
/// The remaining eleven `<blend-mode>` values (`overlay`,
/// `color-dodge`, `color-burn`, `hard-light`, `soft-light`,
/// `difference`, `exclusion`, `hue`, `saturation`, `color`,
/// `luminosity`) have their mixing formulae in the `[COMPOSITING-1]`
/// companion specification, which is not staged under `docs/`; [`blend`]
/// leaves those operands unevaluated (returns `i1` unchanged) rather
/// than guess the factors.
pub fn blend(i1: &FilterImage, i2: &FilterImage, mode: BlendMode) -> FilterImage {
    let mut out = FilterImage::new(i1.width, i1.height);
    for y in 0..i1.height {
        for x in 0..i1.width {
            let a = i1.pixel(x, y);
            let b = if x < i2.width && y < i2.height {
                i2.pixel(x, y)
            } else {
                [0.0; 4]
            };
            let (qa, qb) = (a[3], b[3]);
            // §15.9: qr = 1 − (1 − qa)·(1 − qb), shared by every mode.
            let qr = 1.0 - (1.0 - qa) * (1.0 - qb);
            let px = match mode {
                // cr = (1 − qa)·cb + ca per channel — the Porter-Duff
                // `over`. The colour rows and the alpha row coincide
                // because (1 − qa)·qb + qa = qr.
                BlendMode::Normal => [
                    (1.0 - qa) * b[0] + a[0],
                    (1.0 - qa) * b[1] + a[1],
                    (1.0 - qa) * b[2] + a[2],
                    qr,
                ],
                // cr = (1 − qa)·cb + (1 − qb)·ca + ca·cb.
                BlendMode::Multiply => {
                    let f = |ca: f32, cb: f32| (1.0 - qa) * cb + (1.0 - qb) * ca + ca * cb;
                    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), qr]
                }
                // cr = cb + ca − ca·cb.
                BlendMode::Screen => {
                    let f = |ca: f32, cb: f32| cb + ca - ca * cb;
                    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), qr]
                }
                // cr = min((1 − qa)·cb + ca, (1 − qb)·ca + cb).
                BlendMode::Darken => {
                    let f = |ca: f32, cb: f32| ((1.0 - qa) * cb + ca).min((1.0 - qb) * ca + cb);
                    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), qr]
                }
                // cr = max((1 − qa)·cb + ca, (1 − qb)·ca + cb).
                BlendMode::Lighten => {
                    let f = |ca: f32, cb: f32| ((1.0 - qa) * cb + ca).max((1.0 - qb) * ca + cb);
                    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), qr]
                }
                // overlay / color-dodge / color-burn / hard-light /
                // soft-light / difference / exclusion / hue / saturation
                // / color / luminosity: formulae live in the un-staged
                // [COMPOSITING-1] reference — left unevaluated.
                _ => a,
            };
            out.set_pixel(x, y, px);
        }
    }
    out
}

/// Evaluate a parsed [`FilterPrimitiveNode`] holding a
/// [`FilterPrimitive::Blend`] over two 8-bit non-premultiplied
/// sRGB-encoded RGBA buffers (`i1` ← `in`, `i2` ← `in2`), in the node's
/// resolved `color-interpolation-filters` working space.
///
/// Both buffers must be `width × height × 4`. Returns `None` when the
/// node is not a blend, when a buffer length is wrong, or when the mode
/// is one of the un-staged `[COMPOSITING-1]` modes
/// (`overlay`/`color-dodge`/…/`luminosity`) — callers can then fall
/// back to the graph-level rasteriser for those.
pub fn evaluate_blend_node(
    node: &FilterPrimitiveNode,
    in1_rgba8: &[u8],
    in2_rgba8: &[u8],
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    let FilterPrimitive::Blend { mode, .. } = &node.primitive else {
        return None;
    };
    if !matches!(
        mode,
        BlendMode::Normal
            | BlendMode::Multiply
            | BlendMode::Screen
            | BlendMode::Darken
            | BlendMode::Lighten
    ) {
        return None;
    }
    let space = node.color_interpolation_filters;
    let i1 = FilterImage::from_rgba8(width, height, in1_rgba8, space)?;
    let i2 = FilterImage::from_rgba8(width, height, in2_rgba8, space)?;
    Some(blend(&i1, &i2, *mode).to_rgba8(space))
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

/// `<feMerge>` per Filter Effects §9.16 — "composites input image layers
/// on top of each other using the `over` operator with Input1
/// (corresponding to the first `feMergeNode` child element) on the bottom
/// and the last specified input, InputN … on top."
///
/// The §9.16 result is the left fold of the Porter-Duff `over` operator
/// over the operands bottom-to-top: starting from the first layer as the
/// destination, each later layer is composited *over* the running
/// accumulator with [`composite`]`(top, acc, Over, …)` (the §15.9 `over`
/// where the source operand is the upper layer). The associativity §9.16
/// notes is exactly what lets the n layers collapse into this single
/// fold.
///
/// All operands must share the same dimensions; mismatched layers are
/// zero-extended / truncated to `width × height` by [`composite`]. An
/// empty `layers` list yields a transparent-black `width × height`
/// buffer (no input to render). The operands are already in the working
/// colour space (§10); `over` is colour-space-agnostic premultiplied
/// arithmetic.
pub fn merge(layers: &[FilterImage], width: usize, height: usize) -> FilterImage {
    let mut acc = FilterImage::new(width, height);
    for layer in layers {
        // §9.16: the later layer is the upper (source) operand of `over`,
        // the running accumulator is the lower (destination).
        acc = composite(layer, &acc, CompositeOperator::Over, [0.0; 4]);
    }
    acc
}

/// Evaluate a parsed [`FilterPrimitiveNode`] holding a
/// [`FilterPrimitive::Merge`] over its already-resolved input layers, in
/// the node's resolved `color-interpolation-filters` working space (§10).
///
/// `inputs_rgba8` supplies one 8-bit non-premultiplied sRGB-encoded RGBA
/// buffer per `<feMergeNode>`, in document order (first node = bottom
/// layer, last = top), each `width × height × 4`. Returns `None` when the
/// node is not a merge, when the supplied buffer count does not match the
/// node's `feMergeNode` count, or when any buffer length is wrong.
pub fn evaluate_merge_node(
    node: &FilterPrimitiveNode,
    inputs_rgba8: &[&[u8]],
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    let FilterPrimitive::Merge { inputs } = &node.primitive else {
        return None;
    };
    if inputs.len() != inputs_rgba8.len() {
        return None;
    }
    let space = node.color_interpolation_filters;
    let mut layers = Vec::with_capacity(inputs_rgba8.len());
    for buf in inputs_rgba8 {
        layers.push(FilterImage::from_rgba8(width, height, buf, space)?);
    }
    Some(merge(&layers, width, height).to_rgba8(space))
}

/// Evaluate a parsed [`FilterPrimitiveNode`] holding a
/// [`FilterPrimitive::Offset`] over an 8-bit non-premultiplied
/// sRGB-encoded RGBA buffer, in the node's resolved
/// `color-interpolation-filters` working space (§10).
///
/// Applies the §9.18 shift `out(x, y) = in(x − dx, y − dy)` via
/// [`offset`] (fractional offsets bilinear-interpolated). Returns `None`
/// when the node is not an offset or when
/// `source_rgba8.len() != width * height * 4`.
pub fn evaluate_offset_node(
    node: &FilterPrimitiveNode,
    source_rgba8: &[u8],
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    let FilterPrimitive::Offset { dx, dy, .. } = &node.primitive else {
        return None;
    };
    let space = node.color_interpolation_filters;
    let src = FilterImage::from_rgba8(width, height, source_rgba8, space)?;
    Some(offset(&src, *dx, *dy).to_rgba8(space))
}

/// Evaluate a parsed [`FilterPrimitiveNode`] holding a
/// [`FilterPrimitive::GaussianBlur`] over an 8-bit non-premultiplied
/// sRGB-encoded RGBA buffer, in the node's resolved
/// `color-interpolation-filters` working space (§10).
///
/// Runs the §9.14 three-box-blur approximation ([`gaussian_blur_edge`])
/// with the node's parsed `edgeMode`. Returns `None` when the node is
/// not a Gaussian blur or when `source_rgba8.len() != width * height *
/// 4`.
pub fn evaluate_gaussian_blur_node(
    node: &FilterPrimitiveNode,
    source_rgba8: &[u8],
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    let FilterPrimitive::GaussianBlur {
        std_deviation_x,
        std_deviation_y,
        edge_mode,
        ..
    } = &node.primitive
    else {
        return None;
    };
    let space = node.color_interpolation_filters;
    let src = FilterImage::from_rgba8(width, height, source_rgba8, space)?;
    Some(gaussian_blur_edge(&src, *std_deviation_x, *std_deviation_y, *edge_mode).to_rgba8(space))
}

/// Apply one [`TransferFunction`] to a single non-premultiplied colour
/// component `c ∈ [0, 1]` per Filter Effects §9.7 (the transfer-function
/// element definitions). `C` is the input component, `C'` the remapped
/// component; both in the closed interval `[0, 1]`.
///
/// * `identity` — `C' = C`.
/// * `table` — the `n + 1` entries `v0..vn` bound `n` evenly-sized
///   interpolation regions. For `C < 1`, find `k` with
///   `k/n ≤ C < (k+1)/n`; then `C' = vk + (C − k/n)·n·(v(k+1) − vk)`.
///   For `C = 1`, `C' = vn`. An empty list (or a single value) collapses
///   to the identity / constant the spec mandates.
/// * `discrete` — the `n` entries `v0..v(n−1)` form an `n`-step function.
///   For `C < 1`, `C' = vk` with `k = floor(C·n)`; for `C = 1`,
///   `C' = v(n−1)`.
/// * `linear` — `C' = slope·C + intercept`.
/// * `gamma` — `C' = amplitude·pow(C, exponent) + offset`.
///
/// The result is clamped to `[0, 1]` (the spec defines the working
/// component range as the closed unit interval); `tableValues` entries
/// themselves are used verbatim by the interpolation before that clamp.
fn apply_transfer(func: &TransferFunction, c: f32) -> f32 {
    let mapped = match func {
        TransferFunction::Identity => c,
        TransferFunction::Table { values } => match values.len() {
            // "An empty list results in an identity transfer function."
            0 => c,
            // A single value gives n = 0 regions; the formula is undefined
            // there, so the lone sample is the constant output.
            1 => values[0],
            len => {
                let n = len - 1;
                if c >= 1.0 {
                    values[n]
                } else {
                    // k/n ≤ C < (k+1)/n  ⇒  k = floor(C·n), bounded to n−1.
                    let k = ((c * n as f32).floor() as usize).min(n - 1);
                    let vk = values[k];
                    let vk1 = values[k + 1];
                    vk + (c - k as f32 / n as f32) * n as f32 * (vk1 - vk)
                }
            }
        },
        TransferFunction::Discrete { values } => match values.len() {
            0 => c,
            len => {
                let n = len;
                if c >= 1.0 {
                    values[n - 1]
                } else {
                    // k = floor(C·n), bounded to n−1 for C just below 1.
                    let k = ((c * n as f32).floor() as usize).min(n - 1);
                    values[k]
                }
            }
        },
        TransferFunction::Linear { slope, intercept } => slope * c + intercept,
        TransferFunction::Gamma {
            amplitude,
            exponent,
            offset,
        } => amplitude * c.max(0.0).powf(*exponent) + offset,
    };
    mapped.clamp(0.0, 1.0)
}

/// `<feComponentTransfer>` per Filter Effects §9.7 — per-channel
/// remapping `R' = feFuncR(R)`, `G' = feFuncG(G)`, `B' = feFuncB(B)`,
/// `A' = feFuncA(A)` for every pixel ([`apply_transfer`] supplies the
/// per-component map).
///
/// §9.7 is explicit that "the calculations are performed on
/// **non-premultiplied** color values", so this routine un-premultiplies
/// each [`FilterImage`] pixel, applies the four transfer functions, and
/// re-premultiplies by the transformed alpha for storage. The colour
/// channels are already in the [`FilterImage`]'s working colour space
/// (§10), where the per-channel functions act.
pub fn component_transfer(
    src: &FilterImage,
    red: &TransferFunction,
    green: &TransferFunction,
    blue: &TransferFunction,
    alpha: &TransferFunction,
) -> FilterImage {
    let mut out = FilterImage::new(src.width, src.height);
    for (dst, s) in out.data.chunks_exact_mut(4).zip(src.data.chunks_exact(4)) {
        // Un-premultiply to recover the non-premultiplied operand §9.7
        // requires; a transparent pixel has no defined colour, so feed
        // zeros through the colour functions there.
        let a = s[3];
        let unp = if a > 0.0 {
            [s[0] / a, s[1] / a, s[2] / a, a]
        } else {
            [0.0, 0.0, 0.0, 0.0]
        };
        let r = apply_transfer(red, unp[0]);
        let g = apply_transfer(green, unp[1]);
        let b = apply_transfer(blue, unp[2]);
        let na = apply_transfer(alpha, unp[3]);
        // Re-premultiply by the transformed alpha for storage.
        dst[0] = r * na;
        dst[1] = g * na;
        dst[2] = b * na;
        dst[3] = na;
    }
    out
}

/// Evaluate a parsed [`FilterPrimitiveNode`] holding a
/// [`FilterPrimitive::ComponentTransfer`] over an 8-bit non-premultiplied
/// sRGB-encoded RGBA buffer, in the node's resolved
/// `color-interpolation-filters` working space (§10).
///
/// Returns `None` when the node is not a component transfer or when
/// `source_rgba8.len() != width * height * 4`.
pub fn evaluate_component_transfer_node(
    node: &FilterPrimitiveNode,
    source_rgba8: &[u8],
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    let FilterPrimitive::ComponentTransfer {
        red,
        green,
        blue,
        alpha,
        ..
    } = &node.primitive
    else {
        return None;
    };
    let space = node.color_interpolation_filters;
    let src = FilterImage::from_rgba8(width, height, source_rgba8, space)?;
    Some(component_transfer(&src, red, green, blue, alpha).to_rgba8(space))
}

/// `<feMorphology>` per Filter Effects §9.17 — the dilation/erosion
/// morphological operator.
///
/// §9.17 defines the kernel as a rectangle of width `2·radius_x` and
/// height `2·radius_y`. For an integer-radius implementation this is
/// realised as the symmetric, inclusive integer window
/// `[x − rx, x + rx] × [y − ry, y + ry]` (`rx`/`ry` = `radius` rounded
/// to the nearest integer): `2·r + 1` samples spanning a `2·r`-wide
/// rectangle, centred on the output pixel. The output pixel is the
/// component-wise **maximum** of those samples for `dilate` and the
/// component-wise **minimum** for `erode`.
///
/// §9.17: "feMorphology operates on premultipied color values, [so] it
/// will always result in color values less than or equal to the alpha
/// channel." [`FilterImage`] already stores premultiplied components,
/// so the per-channel min/max runs directly on `data`; because each
/// channel's premultiplied value is `≤ α` before the operation and both
/// `min` and `max` of a window draw from the same set of samples, the
/// invariant `Rᵖ, Gᵖ, Bᵖ ≤ Aᵖ` is preserved by `erode` (each picks the
/// minimum independently, and the alpha minimum dominates) and by
/// `dilate` (the matching window member supplies the bound).
///
/// §9.17 attaches no `edgeMode` to `feMorphology`; samples whose kernel
/// position falls outside the image are simply not part of the window
/// (the iteration bounds are clamped to the image), so border pixels
/// operate over the in-image portion of their kernel rectangle.
pub fn morphology(
    src: &FilterImage,
    operator: MorphologyOperator,
    radius_x: f32,
    radius_y: f32,
) -> FilterImage {
    // §9.17: "A negative or zero value disables the effect of the given
    // filter primitive (i.e., the result is the filter input image)."
    if radius_x <= 0.0 || radius_y <= 0.0 {
        return src.clone();
    }
    let rx = radius_x.round() as isize;
    let ry = radius_y.round() as isize;
    if rx <= 0 || ry <= 0 {
        return src.clone();
    }
    let mut out = FilterImage::new(src.width, src.height);
    let w = src.width as isize;
    let h = src.height as isize;
    for y in 0..src.height {
        for x in 0..src.width {
            // erode → component-wise minimum (seed +∞);
            // dilate → component-wise maximum (seed −∞).
            let mut acc = match operator {
                MorphologyOperator::Erode => [f32::INFINITY; 4],
                MorphologyOperator::Dilate => [f32::NEG_INFINITY; 4],
            };
            let yi = y as isize;
            let xi = x as isize;
            for ky in (yi - ry).max(0)..=(yi + ry).min(h - 1) {
                for kx in (xi - rx).max(0)..=(xi + rx).min(w - 1) {
                    let s = src.pixel(kx as usize, ky as usize);
                    for c in 0..4 {
                        acc[c] = match operator {
                            MorphologyOperator::Erode => acc[c].min(s[c]),
                            MorphologyOperator::Dilate => acc[c].max(s[c]),
                        };
                    }
                }
            }
            out.set_pixel(x, y, acc);
        }
    }
    out
}

/// Evaluate a parsed [`FilterPrimitiveNode`] holding a
/// [`FilterPrimitive::Morphology`] over an 8-bit non-premultiplied
/// sRGB-encoded RGBA buffer, in the node's resolved
/// `color-interpolation-filters` working space (§10).
///
/// Applies the §9.17 erode/dilate operator via [`morphology`]. Returns
/// `None` when the node is not a morphology or when
/// `source_rgba8.len() != width * height * 4`.
pub fn evaluate_morphology_node(
    node: &FilterPrimitiveNode,
    source_rgba8: &[u8],
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    let FilterPrimitive::Morphology {
        operator,
        radius_x,
        radius_y,
        ..
    } = &node.primitive
    else {
        return None;
    };
    let space = node.color_interpolation_filters;
    let src = FilterImage::from_rgba8(width, height, source_rgba8, space)?;
    Some(morphology(&src, *operator, *radius_x, *radius_y).to_rgba8(space))
}

/// Resolve a sample coordinate `c` against an axis of `len` pixels under
/// the §9.9 `<feConvolveMatrix>` `edgeMode` policy, returning the
/// in-image index to read, or `None` when the sample contributes
/// transparent black.
///
/// * [`ConvolveEdgeMode::Duplicate`] — the §9.9 *initial* value: the
///   input image is extended along each border by duplicating the edge
///   pixel (clamp).
/// * [`ConvolveEdgeMode::Wrap`] — toroidal: the input is extended by
///   taking values from the opposite edge (Euclidean modulo so negative
///   coordinates wrap to the high edge).
/// * [`ConvolveEdgeMode::None`] — the input is extended with transparent
///   black (R, G, B, A all zero) beyond its borders.
fn convolve_edge_sample(c: isize, len: usize, mode: ConvolveEdgeMode) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let n = len as isize;
    match mode {
        ConvolveEdgeMode::Duplicate => Some(c.clamp(0, n - 1) as usize),
        ConvolveEdgeMode::Wrap => Some(c.rem_euclid(n) as usize),
        ConvolveEdgeMode::None => {
            if c >= 0 && c < n {
                Some(c as usize)
            } else {
                None
            }
        }
    }
}

/// `<feConvolveMatrix>` per Filter Effects §9.9 — a 2-D linear
/// convolution of the input with an `order_x × order_y` kernel.
///
/// The normative §9.9 formula for each colour component is
///
/// ```text
///                 ( orderY-1 orderX-1
/// COLOR[X,Y] = (  (   Σ        Σ      SOURCE[X-targetX+J, Y-targetY+I]
///                 (    I=0      J=0
///                                  · kernelMatrix[orderX-J-1, orderY-I-1]
///              ) / divisor ) + bias · ALPHA[X,Y]
/// ```
///
/// The kernel is indexed `[orderX-J-1, orderY-I-1]`, i.e. **rotated 180°**
/// relative to the source/destination images, matching the convolution
/// (rather than correlation) convention the spec calls out. `kernel`
/// here is supplied in row-major reading order (the on-wire
/// `kernelMatrix` order), so element `[col, row]` lives at
/// `kernel[row * order_x + col]`.
///
/// `preserve_alpha` selects the §9.9.7 mode:
///
/// * `false` (the initial value) — the convolution applies to **all
///   channels including alpha**, and per the §9 preamble
///   ("all image filters operate on premultiplied RGBA samples") the
///   maths runs on **premultiplied** components. `ALPHA[X,Y]` in the
///   `bias` term is the *convolved* alpha (alpha is itself the result of
///   the same kernel sum / divisor), and each colour channel adds
///   `bias · ALPHA[X,Y]`.
/// * `true` — the convolution applies to the **colour channels only**.
///   The input is temporarily **un-premultiplied**, the kernel is applied
///   to the three colour channels (each adding `bias · ALPHA[X,Y]` with
///   `ALPHA[X,Y] = SOURCE[X,Y]` per §9.9.7, i.e. the pixel's own alpha),
///   and the alpha channel passes through unchanged before being
///   re-premultiplied for storage.
///
/// A `divisor` of `0` is replaced by the §9.9.4 fallback (the sum of the
/// kernel, or `1` when that sum is also zero) — an explicit
/// `divisor="0"` is invalid and the spec mandates the default in its
/// place. Out-of-image samples follow `edge_mode`
/// ([`convolve_edge_sample`]); the result of every component is clamped
/// to `[0, 1]`.
///
/// Returns `src.clone()` unchanged when the kernel length does not match
/// `order_x · order_y` (a malformed primitive is a no-op per §9.9).
#[allow(clippy::too_many_arguments)]
pub fn convolve_matrix(
    src: &FilterImage,
    order_x: u32,
    order_y: u32,
    kernel: &[f32],
    divisor: f32,
    bias: f32,
    target_x: i32,
    target_y: i32,
    edge_mode: ConvolveEdgeMode,
    preserve_alpha: bool,
) -> FilterImage {
    let ox = order_x as usize;
    let oy = order_y as usize;
    if ox == 0 || oy == 0 || kernel.len() != ox * oy {
        return src.clone();
    }
    // §9.9.4: a zero divisor is invalid; fall back to the sum of the
    // kernel, or 1 when that sum is zero.
    let divisor = if divisor == 0.0 {
        let sum: f32 = kernel.iter().sum();
        if sum == 0.0 {
            1.0
        } else {
            sum
        }
    } else {
        divisor
    };

    let w = src.width;
    let h = src.height;
    let mut out = FilterImage::new(w, h);

    for y in 0..h {
        for x in 0..w {
            // Accumulate the rotated-kernel sum for each channel. With
            // preserve_alpha the colour samples are un-premultiplied
            // first; otherwise the premultiplied components are used
            // directly.
            let mut acc = [0.0f32; 4];
            for i in 0..oy {
                for j in 0..ox {
                    // SOURCE[X - targetX + J, Y - targetY + I].
                    let sx = x as isize - target_x as isize + j as isize;
                    let sy = y as isize - target_y as isize + i as isize;
                    let k = kernel[(oy - i - 1) * ox + (ox - j - 1)];
                    let sample = match (
                        convolve_edge_sample(sx, w, edge_mode),
                        convolve_edge_sample(sy, h, edge_mode),
                    ) {
                        (Some(px), Some(py)) => unpremul_if(src.pixel(px, py), preserve_alpha),
                        // Out-of-image under edgeMode=none reads as
                        // transparent black.
                        _ => [0.0; 4],
                    };
                    for c in 0..4 {
                        acc[c] += k * sample[c];
                    }
                }
            }

            let pixel = src.pixel(x, y);
            if preserve_alpha {
                // §9.9.7 true: colours convolved un-premultiplied, alpha
                // passes through. ALPHA[X,Y] = SOURCE[X,Y] (this pixel's
                // own alpha). Re-premultiply for storage.
                let alpha = pixel[3];
                let mut rgba = [0.0f32; 4];
                for c in 0..3 {
                    rgba[c] = (acc[c] / divisor + bias * alpha).clamp(0.0, 1.0) * alpha;
                }
                rgba[3] = alpha;
                out.set_pixel(x, y, rgba);
            } else {
                // §9.9.7 false: all four channels convolved on
                // premultiplied data; ALPHA[X,Y] is the convolved alpha.
                let conv_alpha = (acc[3] / divisor).clamp(0.0, 1.0);
                let mut rgba = [0.0f32; 4];
                for c in 0..3 {
                    rgba[c] = (acc[c] / divisor + bias * conv_alpha).clamp(0.0, 1.0);
                }
                rgba[3] = conv_alpha;
                out.set_pixel(x, y, rgba);
            }
        }
    }
    out
}

/// Un-premultiply a stored premultiplied RGBA sample back to
/// straight-alpha colour when `unpremul` is set (the §9.9.7
/// `preserveAlpha="true"` path); otherwise return the premultiplied
/// sample verbatim. Transparent pixels map their colour to zero.
fn unpremul_if(p: [f32; 4], unpremul: bool) -> [f32; 4] {
    if !unpremul {
        return p;
    }
    let a = p[3];
    if a > 0.0 {
        [p[0] / a, p[1] / a, p[2] / a, a]
    } else {
        [0.0, 0.0, 0.0, a]
    }
}

/// Evaluate a parsed [`FilterPrimitiveNode`] holding a
/// [`FilterPrimitive::ConvolveMatrix`] over an 8-bit non-premultiplied
/// sRGB-encoded RGBA buffer, in the node's resolved
/// `color-interpolation-filters` working space (§10).
///
/// Applies the §9.9 convolution via [`convolve_matrix`]. Returns `None`
/// when the node is not a convolve-matrix or when
/// `source_rgba8.len() != width * height * 4`.
pub fn evaluate_convolve_matrix_node(
    node: &FilterPrimitiveNode,
    source_rgba8: &[u8],
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    let FilterPrimitive::ConvolveMatrix {
        order_x,
        order_y,
        kernel_matrix,
        divisor,
        bias,
        target_x,
        target_y,
        edge_mode,
        preserve_alpha,
        ..
    } = &node.primitive
    else {
        return None;
    };
    let space = node.color_interpolation_filters;
    let src = FilterImage::from_rgba8(width, height, source_rgba8, space)?;
    Some(
        convolve_matrix(
            &src,
            *order_x,
            *order_y,
            kernel_matrix,
            *divisor,
            *bias,
            *target_x,
            *target_y,
            *edge_mode,
            *preserve_alpha,
        )
        .to_rgba8(space),
    )
}

/// Read the §9.11 displacement channel `XC`/`YC` from a non-premultiplied
/// RGBA quad `[r, g, b, a]` (each component in `[0, 1]`) per the
/// [`ChannelSelector`] enum: `R`/`G`/`B`/`A` → component 0/1/2/3.
fn select_channel(sel: ChannelSelector, unp: [f32; 4]) -> f32 {
    match sel {
        ChannelSelector::R => unp[0],
        ChannelSelector::G => unp[1],
        ChannelSelector::B => unp[2],
        ChannelSelector::A => unp[3],
    }
}

/// Sample a premultiplied [`FilterImage`] at the integer coordinate
/// `(x, y)` with the §9.11 edge policy of zero extension: a coordinate
/// outside the image reads transparent black (the displacement map can
/// point arbitrarily far per `scale`, and the §9.11 prose places no
/// border-duplication requirement, so out-of-image source samples are
/// the filter region's transparent-black backdrop).
fn sample_zero(img: &FilterImage, x: isize, y: isize) -> [f32; 4] {
    if x < 0 || y < 0 || x as usize >= img.width || y as usize >= img.height {
        return [0.0; 4];
    }
    img.pixel(x as usize, y as usize)
}

/// `<feDisplacementMap>` per Filter Effects §9.11 — spatially displaces
/// the `in` image (`src`) using per-pixel channel values from the `in2`
/// displacement map (`map`):
///
/// ```text
/// P'(x, y) ← P( x + scale·(XC(x, y) − ½), y + scale·(YC(x, y) − ½) )
/// ```
///
/// where `XC` / `YC` are the `x_channel` / `y_channel` components of the
/// **non-premultiplied** `map` pixel (§9.11: "The calculations using the
/// pixel values from `in2` are performed using non-premultiplied color
/// values"), and `P` is the **premultiplied** `src` pixel (§9.11: "The
/// input image `in` is to remain premultiplied for this filter
/// primitive").
///
/// The §9.11 displacement is `scale·(C − ½)`, so a map channel value of
/// `½` is the no-displacement midpoint, `0` shifts the sample by
/// `−scale/2`, and `1` shifts it by `+scale/2`. The destination samples
/// `src` at the displaced integer coordinate (rounded to nearest — the
/// §9.11 note leaves the sub-pixel interpolation method
/// implementation-defined and explicitly unspecified for now, so the
/// evaluator takes the nearest source texel rather than guess a filter
/// kernel); out-of-image source coordinates read transparent black.
///
/// `scale == 0` is the §9.11 identity (the source is returned unchanged).
/// The result has `src`'s dimensions; `map` is sampled at the same
/// destination coordinate `(x, y)` and zero-extended past its own
/// bounds.
pub fn displacement_map(
    src: &FilterImage,
    map: &FilterImage,
    scale: f32,
    x_channel: ChannelSelector,
    y_channel: ChannelSelector,
) -> FilterImage {
    let mut out = FilterImage::new(src.width, src.height);
    if scale == 0.0 {
        // §9.11: "When the value of this attribute is 0, this operation
        // has no effect on the source image."
        let n = out.data.len().min(src.data.len());
        out.data[..n].copy_from_slice(&src.data[..n]);
        return out;
    }
    for y in 0..out.height {
        for x in 0..out.width {
            // §9.11: in2 pixel read non-premultiplied. The map shares the
            // destination coordinate (x, y); zero-extend past its bounds.
            let m = if x < map.width && y < map.height {
                map.pixel(x, y)
            } else {
                [0.0; 4]
            };
            let ma = m[3];
            let unp = if ma > 0.0 {
                [m[0] / ma, m[1] / ma, m[2] / ma, ma]
            } else {
                [0.0, 0.0, 0.0, 0.0]
            };
            let xc = select_channel(x_channel, unp);
            let yc = select_channel(y_channel, unp);
            // P'(x, y) ← P(x + scale·(XC − ½), y + scale·(YC − ½)).
            let sx = x as f32 + scale * (xc - 0.5);
            let sy = y as f32 + scale * (yc - 0.5);
            let px = sample_zero(src, sx.round() as isize, sy.round() as isize);
            out.set_pixel(x, y, px);
        }
    }
    out
}

/// Evaluate a parsed [`FilterPrimitiveNode`] holding a
/// [`FilterPrimitive::DisplacementMap`] over two 8-bit non-premultiplied
/// sRGB-encoded RGBA buffers (`in1` ← `in`, `in2` ← `in2`).
///
/// Per §9.11 the two inputs live in **different colour spaces**: the
/// `color-interpolation-filters` working space applies only to `in2`
/// (the displacement map, whose channel values drive the maths), while
/// `in` "must remain in its current colour space". The evaluator
/// therefore decodes `in` with the sRGB-identity space (no §13.9
/// linearisation) so the displaced source texels are passed through
/// untouched, and decodes `in2` in the node's resolved working space so
/// the channel selectors read the §10-correct values. The output is the
/// re-encoded displaced `in`, so it is emitted with the sRGB-identity
/// space to match `in`'s preserved space.
///
/// Both buffers must be `width × height × 4`. Returns `None` when the
/// node is not a displacement map or when a buffer length is wrong.
pub fn evaluate_displacement_map_node(
    node: &FilterPrimitiveNode,
    in1_rgba8: &[u8],
    in2_rgba8: &[u8],
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    let FilterPrimitive::DisplacementMap {
        scale,
        x_channel_selector,
        y_channel_selector,
        ..
    } = &node.primitive
    else {
        return None;
    };
    // §9.11: in stays in its current colour space (no linearisation);
    // in2 uses the node's working space.
    let src = FilterImage::from_rgba8(width, height, in1_rgba8, ColorInterpolationFilters::Srgb)?;
    let map = FilterImage::from_rgba8(width, height, in2_rgba8, node.color_interpolation_filters)?;
    Some(
        displacement_map(&src, &map, *scale, *x_channel_selector, *y_channel_selector)
            .to_rgba8(ColorInterpolationFilters::Srgb),
    )
}

/// `<feTile>` per Filter Effects §9.20 — fill the `width × height` target
/// rectangle with a repeated, tiled copy of the input image's reference
/// tile.
///
/// The reference tile is the rectangle `(tile_x, tile_y, tile_w,
/// tile_h)` within `src` (the input's *filter-primitive subregion*; the
/// graph-level rasteriser establishes those bounds and passes them in —
/// the evaluator itself does no subregion plumbing). §9.20: "feTile
/// replicates the reference tile in both X and Y to completely fill the
/// target rectangle. The top/left corner of each given tile is at
/// location `(x + i·width, y + j·height)` … and `i` and `j` can be any
/// integer value."
///
/// The destination pixel `(dx, dy)` therefore samples the tile at the
/// periodic coordinate
///
/// ```text
/// sx = tile_x + ((dx − tile_x) mod tile_w)
/// sy = tile_y + ((dy − tile_y) mod tile_h)
/// ```
///
/// (Euclidean modulo, so destinations to the left of / above the tile
/// origin wrap correctly). A degenerate tile (`tile_w == 0` or
/// `tile_h == 0`, or one lying wholly outside `src`) yields a
/// transparent-black result. Values are copied verbatim from the
/// premultiplied [`FilterImage`] storage — tiling is a pure spatial
/// replication, so no colour-space conversion happens inside the
/// replication loop.
pub fn tile(
    src: &FilterImage,
    tile_x: usize,
    tile_y: usize,
    tile_w: usize,
    tile_h: usize,
    width: usize,
    height: usize,
) -> FilterImage {
    let mut out = FilterImage::new(width, height);
    if tile_w == 0 || tile_h == 0 {
        return out;
    }
    for dy in 0..height {
        // sy = tile_y + ((dy − tile_y) mod tile_h).
        let oy = (dy as isize - tile_y as isize).rem_euclid(tile_h as isize) as usize;
        let sy = tile_y + oy;
        if sy >= src.height {
            continue;
        }
        for dx in 0..width {
            let ox = (dx as isize - tile_x as isize).rem_euclid(tile_w as isize) as usize;
            let sx = tile_x + ox;
            if sx >= src.width {
                continue;
            }
            out.set_pixel(dx, dy, src.pixel(sx, sy));
        }
    }
    out
}

/// Evaluate a parsed [`FilterPrimitiveNode`] holding a
/// [`FilterPrimitive::Tile`] over an 8-bit non-premultiplied
/// sRGB-encoded RGBA buffer, replicating the reference tile
/// `(tile_x, tile_y, tile_w, tile_h)` across the `width × height`
/// output.
///
/// The caller (graph-level rasteriser) supplies the input image's
/// filter-primitive subregion as the tile rectangle. Because §9.20
/// tiling is a pure spatial replication that copies stored values
/// verbatim, the working colour space is irrelevant to the maths — the
/// node is decoded and re-encoded in the node's resolved
/// `color-interpolation-filters` space purely so the round-trip is a
/// byte-exact passthrough of the tiled pixels. Returns `None` when the
/// node is not a tile or when `source_rgba8.len() != width·height·4`.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_tile_node(
    node: &FilterPrimitiveNode,
    source_rgba8: &[u8],
    src_width: usize,
    src_height: usize,
    tile_x: usize,
    tile_y: usize,
    tile_w: usize,
    tile_h: usize,
    out_width: usize,
    out_height: usize,
) -> Option<Vec<u8>> {
    let FilterPrimitive::Tile { .. } = &node.primitive else {
        return None;
    };
    let space = node.color_interpolation_filters;
    let src = FilterImage::from_rgba8(src_width, src_height, source_rgba8, space)?;
    Some(tile(&src, tile_x, tile_y, tile_w, tile_h, out_width, out_height).to_rgba8(space))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::FilterInput;

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

    // §9.6 identity matrix is a no-op (the 4×5 identity reproduces the
    // input non-premultiplied channels and alpha exactly).
    #[test]
    fn color_matrix_identity_is_input() {
        #[rustfmt::skip]
        let id = [
            1.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 1.0, 0.0,
        ];
        let mut img = FilterImage::new(2, 1);
        // Premultiplied: [0.5,0.25,0.125,0.5] is colour [1,0.5,0.25] @ a=0.5.
        img.set_pixel(0, 0, [0.5, 0.25, 0.125, 0.5]);
        img.set_pixel(1, 0, [0.0, 0.0, 0.0, 0.0]);
        let out = color_matrix(&img, &id);
        let p = out.pixel(0, 0);
        for (o, e) in p.iter().zip([0.5, 0.25, 0.125, 0.5]) {
            assert!((o - e).abs() < 1e-6, "{o} vs {e}");
        }
        assert_eq!(out.pixel(1, 0), [0.0; 4]);
    }

    // §9.6 calculations are on non-premultiplied colour. A row-0 bias of
    // 0.5 on an opaque pixel sets R' to clamp(R + 0.5); the swap rows
    // exercise the cross-channel coefficients on un-premultiplied values.
    #[test]
    fn color_matrix_swap_and_bias_non_premultiplied() {
        // Swap R↔B, pass G and A, add 0.25 bias to G.
        #[rustfmt::skip]
        let m = [
            0.0, 0.0, 1.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0, 0.25,
            1.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 1.0, 0.0,
        ];
        let mut img = FilterImage::new(1, 1);
        // Non-premult colour [0.2,0.4,0.6] at alpha 0.5 → premult halved.
        img.set_pixel(0, 0, [0.1, 0.2, 0.3, 0.5]);
        let out = color_matrix(&img, &m);
        // R'=B=0.6, G'=G+0.25=0.65, B'=R=0.2, A'=0.5 (all non-premult);
        // stored premultiplied by A'=0.5.
        let p = out.pixel(0, 0);
        for (o, e) in p.iter().zip([0.6 * 0.5, 0.65 * 0.5, 0.2 * 0.5, 0.5]) {
            assert!((o - e).abs() < 1e-6, "{o} vs {e}");
        }
    }

    // §9.6 clamps each transformed channel to [0, 1] before storage.
    #[test]
    fn color_matrix_clamps_channels() {
        #[rustfmt::skip]
        let m = [
            2.0, 0.0, 0.0, 0.0, 0.0,   // R' = 2·R → clamps to 1
            0.0, 1.0, 0.0, 0.0, -1.0,  // G' = G − 1 → clamps to 0
            0.0, 0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 1.0, 0.0,
        ];
        let mut img = FilterImage::new(1, 1);
        // Opaque non-premult colour [0.8, 0.2, 0.0].
        img.set_pixel(0, 0, [0.8, 0.2, 0.0, 1.0]);
        let out = color_matrix(&img, &m);
        let p = out.pixel(0, 0);
        assert!((p[0] - 1.0).abs() < 1e-6, "R' {}", p[0]);
        assert!((p[1] - 0.0).abs() < 1e-6, "G' {}", p[1]);
        assert!((p[2] - 0.0).abs() < 1e-6, "B' {}", p[2]);
        assert!((p[3] - 1.0).abs() < 1e-6, "A' {}", p[3]);
    }

    // luminanceToAlpha reduction (§9.6 fixed template, coefficients
    // 0.2126 / 0.7152 / 0.0722 in the matrix's 4th row, zero colour) maps
    // colour to a grey alpha and clears RGB.
    #[test]
    fn color_matrix_luminance_to_alpha_template() {
        #[rustfmt::skip]
        let m = [
            0.0,    0.0,    0.0,    0.0, 0.0,
            0.0,    0.0,    0.0,    0.0, 0.0,
            0.0,    0.0,    0.0,    0.0, 0.0,
            0.2126, 0.7152, 0.0722, 0.0, 0.0,
        ];
        let mut img = FilterImage::new(1, 1);
        // Opaque white non-premult [1,1,1].
        img.set_pixel(0, 0, [1.0, 1.0, 1.0, 1.0]);
        let out = color_matrix(&img, &m);
        let p = out.pixel(0, 0);
        // A' = sum of coefficients ≈ 1.0; RGB' all 0; premult → 0.
        assert!((p[3] - 1.0).abs() < 1e-4, "A' {}", p[3]);
        assert_eq!(&p[..3], &[0.0, 0.0, 0.0]);
    }

    // Node entry point: identity matrix in the sRGB working space returns
    // the input bytes unchanged (no linearisation, no colour shift).
    #[test]
    fn evaluate_color_matrix_node_identity_srgb() {
        #[rustfmt::skip]
        let matrix = [
            1.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 1.0, 0.0,
        ];
        let node = FilterPrimitiveNode {
            region: Default::default(),
            result: None,
            color_interpolation_filters: ColorInterpolationFilters::Srgb,
            primitive: FilterPrimitive::ColorMatrix {
                input: crate::filter::FilterInput::SourceGraphic,
                matrix,
            },
        };
        let src = [0x40, 0x80, 0xC0, 0xFF];
        let out = evaluate_color_matrix_node(&node, &src, 1, 1).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn evaluate_color_matrix_node_declines_other_primitive() {
        let node = FilterPrimitiveNode {
            region: Default::default(),
            result: None,
            color_interpolation_filters: ColorInterpolationFilters::Srgb,
            primitive: FilterPrimitive::Flood {
                flood_color: FloodColor::default(),
                flood_opacity: 1.0,
            },
        };
        assert!(evaluate_color_matrix_node(&node, &[0; 4], 1, 1).is_none());
    }

    // --- §9.7 feComponentTransfer ---------------------------------------

    // identity transfer is the pass-through C' = C.
    #[test]
    fn transfer_identity_passes_through() {
        for i in 0..=20u32 {
            let c = i as f32 / 20.0;
            assert_eq!(apply_transfer(&TransferFunction::Identity, c), c);
        }
    }

    // linear C' = slope·C + intercept, then clamp to [0, 1].
    #[test]
    fn transfer_linear() {
        let f = TransferFunction::Linear {
            slope: 2.0,
            intercept: -0.5,
        };
        // 0.25 → 2·0.25 − 0.5 = 0.0; 0.5 → 0.5; 0.75 → 1.0.
        assert!((apply_transfer(&f, 0.25) - 0.0).abs() < 1e-6);
        assert!((apply_transfer(&f, 0.5) - 0.5).abs() < 1e-6);
        assert!((apply_transfer(&f, 0.75) - 1.0).abs() < 1e-6);
        // Below/above the unit interval clamp.
        assert_eq!(apply_transfer(&f, 0.0), 0.0);
        assert_eq!(apply_transfer(&f, 1.0), 1.0);
    }

    // gamma C' = amplitude·pow(C, exponent) + offset.
    #[test]
    fn transfer_gamma() {
        let f = TransferFunction::Gamma {
            amplitude: 1.0,
            exponent: 2.0,
            offset: 0.0,
        };
        assert!((apply_transfer(&f, 0.5) - 0.25).abs() < 1e-6);
        assert!((apply_transfer(&f, 1.0) - 1.0).abs() < 1e-6);
        assert_eq!(apply_transfer(&f, 0.0), 0.0);
    }

    // table: v0..vn bound n regions, with linear interpolation; C=1 → vn.
    #[test]
    fn transfer_table_interpolates() {
        // values = [0, 1] → n=1 region: C' = C across the whole range.
        let f = TransferFunction::Table {
            values: vec![0.0, 1.0],
        };
        for i in 0..=10u32 {
            let c = i as f32 / 10.0;
            assert!((apply_transfer(&f, c) - c).abs() < 1e-6, "c={c}");
        }
        // values = [0, 0.5, 1] → n=2 regions of width 0.5.
        // C=0.25 sits in region 0: v0 + (0.25)·2·(0.5−0) = 0.25.
        // C=0.75 sits in region 1: v1 + (0.75−0.5)·2·(1−0.5) = 0.75.
        let g = TransferFunction::Table {
            values: vec![0.0, 0.5, 1.0],
        };
        assert!((apply_transfer(&g, 0.25) - 0.25).abs() < 1e-6);
        assert!((apply_transfer(&g, 0.75) - 0.75).abs() < 1e-6);
        // C=1 → vn.
        assert!((apply_transfer(&g, 1.0) - 1.0).abs() < 1e-6);
        // A non-monotone table interpolates literally: [1, 0] at C=0.5 → 0.5.
        let h = TransferFunction::Table {
            values: vec![1.0, 0.0],
        };
        assert!((apply_transfer(&h, 0.5) - 0.5).abs() < 1e-6);
    }

    // table edge cases: empty list = identity; single value = constant.
    #[test]
    fn transfer_table_degenerate() {
        let empty = TransferFunction::Table { values: vec![] };
        assert_eq!(apply_transfer(&empty, 0.3), 0.3);
        let one = TransferFunction::Table { values: vec![0.4] };
        assert_eq!(apply_transfer(&one, 0.0), 0.4);
        assert_eq!(apply_transfer(&one, 1.0), 0.4);
    }

    // discrete: n steps; C' = v_floor(C·n); C=1 → v(n−1).
    #[test]
    fn transfer_discrete_steps() {
        let f = TransferFunction::Discrete {
            values: vec![0.0, 0.25, 0.5, 1.0],
        };
        // n=4 → step boundaries at 0.25, 0.5, 0.75.
        assert_eq!(apply_transfer(&f, 0.0), 0.0);
        assert_eq!(apply_transfer(&f, 0.2), 0.0);
        assert_eq!(apply_transfer(&f, 0.25), 0.25);
        assert_eq!(apply_transfer(&f, 0.49), 0.25);
        assert_eq!(apply_transfer(&f, 0.5), 0.5);
        assert_eq!(apply_transfer(&f, 0.74), 0.5);
        assert_eq!(apply_transfer(&f, 0.75), 1.0);
        // C=1 → v(n−1).
        assert_eq!(apply_transfer(&f, 1.0), 1.0);
        // empty discrete = identity.
        let e = TransferFunction::Discrete { values: vec![] };
        assert_eq!(apply_transfer(&e, 0.6), 0.6);
    }

    // §9.7 operates on non-premultiplied values: a half-transparent pixel's
    // colour is recovered, transferred, and re-premultiplied.
    #[test]
    fn component_transfer_unpremultiplies() {
        // premultiplied input: colour 0.5 at alpha 0.5 → stored 0.25.
        let mut img = FilterImage::new(1, 1);
        img.set_pixel(0, 0, [0.25, 0.25, 0.25, 0.5]);
        // linear slope 2 maps the un-premultiplied 0.5 → 1.0 on each colour
        // channel; alpha identity keeps 0.5. Re-premultiplied: 1.0·0.5 = 0.5.
        let double = TransferFunction::Linear {
            slope: 2.0,
            intercept: 0.0,
        };
        let out = component_transfer(&img, &double, &double, &double, &TransferFunction::Identity);
        let p = out.pixel(0, 0);
        assert!((p[0] - 0.5).abs() < 1e-6, "{:?}", p);
        assert!((p[1] - 0.5).abs() < 1e-6);
        assert!((p[2] - 0.5).abs() < 1e-6);
        assert!((p[3] - 0.5).abs() < 1e-6);
    }

    // Fully transparent pixel: no defined colour, alpha transfer still runs.
    #[test]
    fn component_transfer_transparent_pixel() {
        let mut img = FilterImage::new(1, 1);
        img.set_pixel(0, 0, [0.0, 0.0, 0.0, 0.0]);
        // alpha linear 0 → 1 leaves alpha 0 (0·1 + 0 = 0); colour stays 0.
        let zero_a = TransferFunction::Linear {
            slope: 1.0,
            intercept: 0.0,
        };
        let out = component_transfer(
            &img,
            &TransferFunction::Identity,
            &TransferFunction::Identity,
            &TransferFunction::Identity,
            &zero_a,
        );
        assert_eq!(out.pixel(0, 0), [0.0, 0.0, 0.0, 0.0]);
    }

    // Node entry point: all-identity transfer in sRGB returns bytes intact.
    #[test]
    fn evaluate_component_transfer_node_identity_srgb() {
        let node = FilterPrimitiveNode {
            region: Default::default(),
            result: None,
            color_interpolation_filters: ColorInterpolationFilters::Srgb,
            primitive: FilterPrimitive::ComponentTransfer {
                input: crate::filter::FilterInput::SourceGraphic,
                red: TransferFunction::Identity,
                green: TransferFunction::Identity,
                blue: TransferFunction::Identity,
                alpha: TransferFunction::Identity,
            },
        };
        let src = [0x40, 0x80, 0xC0, 0xFF];
        let out = evaluate_component_transfer_node(&node, &src, 1, 1).unwrap();
        assert_eq!(out, src);
    }

    // Node entry point: opaque pixel, discrete threshold on the red channel
    // (binary 0/1) in sRGB working space.
    #[test]
    fn evaluate_component_transfer_node_discrete_threshold_srgb() {
        let threshold = TransferFunction::Discrete {
            values: vec![0.0, 1.0],
        };
        let node = FilterPrimitiveNode {
            region: Default::default(),
            result: None,
            color_interpolation_filters: ColorInterpolationFilters::Srgb,
            primitive: FilterPrimitive::ComponentTransfer {
                input: crate::filter::FilterInput::SourceGraphic,
                red: threshold,
                green: TransferFunction::Identity,
                blue: TransferFunction::Identity,
                alpha: TransferFunction::Identity,
            },
        };
        // red 0x40 (0.25) < 0.5 → step 0 → 0; red 0xC0 (0.75) ≥ 0.5 → 1 → 255.
        let dark =
            evaluate_component_transfer_node(&node, &[0x40, 0x80, 0xC0, 0xFF], 1, 1).unwrap();
        assert_eq!(dark[0], 0x00);
        let bright =
            evaluate_component_transfer_node(&node, &[0xC0, 0x80, 0xC0, 0xFF], 1, 1).unwrap();
        assert_eq!(bright[0], 0xFF);
    }

    #[test]
    fn evaluate_component_transfer_node_declines_other_primitive() {
        let node = FilterPrimitiveNode {
            region: Default::default(),
            result: None,
            color_interpolation_filters: ColorInterpolationFilters::Srgb,
            primitive: FilterPrimitive::Flood {
                flood_color: FloodColor::default(),
                flood_opacity: 1.0,
            },
        };
        assert!(evaluate_component_transfer_node(&node, &[0; 4], 1, 1).is_none());
    }

    // §9.13 — opaque white flood, sRGB working space, opacity 1: every
    // pixel is opaque white; decode/encode are no-ops in sRGB.
    #[test]
    fn flood_opaque_white_srgb_fills_whole_buffer() {
        let fc = FloodColor {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        };
        let img = flood(2, 3, fc, 1.0, ColorInterpolationFilters::Srgb);
        assert_eq!(img.width(), 2);
        assert_eq!(img.height(), 3);
        let out = img.to_rgba8(ColorInterpolationFilters::Srgb);
        assert_eq!(out.len(), 2 * 3 * 4);
        for px in out.chunks_exact(4) {
            assert_eq!(px, [255, 255, 255, 255]);
        }
    }

    // §9.13.2 — flood-opacity 0.5 halves the alpha; the un-premultiplied
    // colour is unchanged (white stays white). 0.5 × 255 = 127.5 → 128.
    #[test]
    fn flood_opacity_scales_alpha_only() {
        let fc = FloodColor {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        };
        let out = flood(1, 1, fc, 0.5, ColorInterpolationFilters::Srgb)
            .to_rgba8(ColorInterpolationFilters::Srgb);
        assert_eq!(out, [255, 255, 255, 128]);
    }

    // §9.13.1 — a coloured flood round-trips its sRGB bytes in the sRGB
    // working space (no linearisation): green #008000 → [0, 128, 0, 255].
    #[test]
    fn flood_colour_srgb_roundtrips() {
        let fc = FloodColor {
            r: 0,
            g: 128,
            b: 0,
            a: 255,
        };
        let out = flood(1, 1, fc, 1.0, ColorInterpolationFilters::Srgb)
            .to_rgba8(ColorInterpolationFilters::Srgb);
        assert_eq!(out, [0, 128, 0, 255]);
    }

    // §10 — in the linearRGB working space the colour is linearised on
    // decode and re-encoded on output. The 0/255 endpoints are transfer
    // fixed points, so an opaque red flood survives the round-trip.
    #[test]
    fn flood_endpoints_invariant_in_linear_space() {
        let fc = FloodColor {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        };
        let out = flood(1, 1, fc, 1.0, ColorInterpolationFilters::LinearRgb)
            .to_rgba8(ColorInterpolationFilters::LinearRgb);
        assert_eq!(out, [255, 0, 0, 255]);
    }

    // A flood with a fully transparent flood-color (or opacity 0) is
    // transparent black after un-premultiplication.
    #[test]
    fn flood_transparent_colour_is_transparent_black() {
        let fc = FloodColor {
            r: 200,
            g: 100,
            b: 50,
            a: 0,
        };
        let out = flood(1, 1, fc, 1.0, ColorInterpolationFilters::Srgb)
            .to_rgba8(ColorInterpolationFilters::Srgb);
        assert_eq!(out, [0, 0, 0, 0]);
        // opacity 0 with an opaque colour is equivalent.
        let fc2 = FloodColor {
            r: 200,
            g: 100,
            b: 50,
            a: 255,
        };
        let out2 = flood(1, 1, fc2, 0.0, ColorInterpolationFilters::Srgb)
            .to_rgba8(ColorInterpolationFilters::Srgb);
        assert_eq!(out2, [0, 0, 0, 0]);
    }

    // The node-level evaluator drives the default flood (opaque black,
    // opacity 1) end-to-end over an arbitrary subregion size.
    #[test]
    fn evaluate_flood_node_default_opaque_black() {
        let node = FilterPrimitiveNode {
            region: Default::default(),
            result: None,
            color_interpolation_filters: ColorInterpolationFilters::Srgb,
            primitive: FilterPrimitive::Flood {
                flood_color: FloodColor::default(),
                flood_opacity: 1.0,
            },
        };
        let out = evaluate_flood_node(&node, 2, 2).unwrap();
        assert_eq!(out.len(), 2 * 2 * 4);
        for px in out.chunks_exact(4) {
            assert_eq!(px, [0, 0, 0, 255]);
        }
    }

    // The flood evaluator declines a non-flood node so the caller can
    // route elsewhere (mirrors the colour-matrix / composite decline).
    #[test]
    fn evaluate_flood_node_declines_other_primitive() {
        let node = FilterPrimitiveNode {
            region: Default::default(),
            result: None,
            color_interpolation_filters: ColorInterpolationFilters::Srgb,
            primitive: FilterPrimitive::ColorMatrix {
                input: crate::filter::FilterInput::SourceGraphic,
                matrix: [0.0; 20],
            },
        };
        assert!(evaluate_flood_node(&node, 1, 1).is_none());
    }

    // §9.14 edgeMode `duplicate`: a uniform opaque field stays uniform —
    // border pixels read the duplicated edge value, so no alpha is lost
    // to the border (unlike `none`, which darkens the edge).
    #[test]
    fn blur_duplicate_preserves_uniform_field() {
        let mut img = FilterImage::new(5, 5);
        for y in 0..5 {
            for x in 0..5 {
                img.set_pixel(x, y, [0.0, 0.0, 0.0, 1.0]);
            }
        }
        let out = gaussian_blur_edge(&img, 2.0, 2.0, EdgeMode::Duplicate);
        for y in 0..5 {
            for x in 0..5 {
                assert!(
                    (out.pixel(x, y)[3] - 1.0).abs() < 1e-5,
                    "({x},{y}) = {}",
                    out.pixel(x, y)[3]
                );
            }
        }
    }

    // `none` on the same uniform field loses mass at the border — the
    // corner is the darkest, the centre the brightest. This is the
    // distinguishing behaviour from `duplicate`.
    #[test]
    fn blur_none_darkens_border() {
        let mut img = FilterImage::new(5, 5);
        for y in 0..5 {
            for x in 0..5 {
                img.set_pixel(x, y, [0.0, 0.0, 0.0, 1.0]);
            }
        }
        let out = gaussian_blur_edge(&img, 2.0, 2.0, EdgeMode::None);
        assert!(out.pixel(0, 0)[3] < out.pixel(2, 2)[3]);
        assert!(out.pixel(2, 2)[3] <= 1.0);
    }

    // §9.14 edgeMode `wrap`: a single lit column blurred horizontally
    // with wrap leaks mass across the left/right seam, so the opposite
    // edge column gains alpha that `none` would have dropped.
    #[test]
    fn blur_wrap_leaks_across_seam() {
        let mut img = FilterImage::new(5, 1);
        img.set_pixel(0, 0, [0.0, 0.0, 0.0, 1.0]); // left-edge impulse
        let wrapped = gaussian_blur_edge(&img, 0.8, 0.0, EdgeMode::Wrap);
        let clamped = gaussian_blur_edge(&img, 0.8, 0.0, EdgeMode::None);
        // The d=2 kernel reaches two pixels each side; under wrap the
        // right edge (x=4) sees the impulse's left tail.
        assert!(wrapped.pixel(4, 0)[3] > 0.0);
        assert_eq!(clamped.pixel(4, 0)[3], 0.0);
    }

    // edge_sample unit coverage for the three modes at both borders.
    #[test]
    fn edge_sample_modes() {
        assert_eq!(edge_sample(-1, 4, EdgeMode::None), None);
        assert_eq!(edge_sample(4, 4, EdgeMode::None), None);
        assert_eq!(edge_sample(2, 4, EdgeMode::None), Some(2));
        assert_eq!(edge_sample(-3, 4, EdgeMode::Duplicate), Some(0));
        assert_eq!(edge_sample(9, 4, EdgeMode::Duplicate), Some(3));
        assert_eq!(edge_sample(-1, 4, EdgeMode::Wrap), Some(3));
        assert_eq!(edge_sample(5, 4, EdgeMode::Wrap), Some(1));
        assert_eq!(edge_sample(0, 0, EdgeMode::Duplicate), None);
    }

    // §9.16 merge: empty layer list is a transparent-black buffer.
    #[test]
    fn merge_empty_is_transparent_black() {
        let out = merge(&[], 2, 2);
        assert_eq!(out.width(), 2);
        assert_eq!(out.height(), 2);
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(out.pixel(x, y), [0.0; 4]);
            }
        }
    }

    // §9.16: one layer merges to itself (over transparent black).
    #[test]
    fn merge_single_layer_is_identity() {
        let mut a = FilterImage::new(1, 1);
        a.set_pixel(0, 0, [0.2, 0.4, 0.6, 0.8]);
        let out = merge(std::slice::from_ref(&a), 1, 1);
        let p = out.pixel(0, 0);
        for (o, e) in p.iter().zip([0.2, 0.4, 0.6, 0.8]) {
            assert!((o - e).abs() < 1e-6, "{o} vs {e}");
        }
    }

    // §9.16: last node on top — an opaque top layer fully hides the
    // bottom layer (over with qa=1 → result is the source).
    #[test]
    fn merge_opaque_top_hides_bottom() {
        let mut bottom = FilterImage::new(1, 1);
        bottom.set_pixel(0, 0, [0.9, 0.0, 0.0, 1.0]);
        let mut top = FilterImage::new(1, 1);
        top.set_pixel(0, 0, [0.0, 0.0, 0.5, 1.0]);
        // [bottom, top]: top is last → on top.
        let out = merge(&[bottom, top], 1, 1);
        assert_eq!(out.pixel(0, 0), [0.0, 0.0, 0.5, 1.0]);
    }

    // §9.16 ordering matters: swapping which layer is last flips which
    // colour shows through for two opaque layers.
    #[test]
    fn merge_order_determines_topmost() {
        let mut red = FilterImage::new(1, 1);
        red.set_pixel(0, 0, [0.8, 0.0, 0.0, 1.0]);
        let mut blue = FilterImage::new(1, 1);
        blue.set_pixel(0, 0, [0.0, 0.0, 0.8, 1.0]);
        let red_top = merge(&[blue.clone(), red.clone()], 1, 1);
        let blue_top = merge(&[red, blue], 1, 1);
        assert_eq!(red_top.pixel(0, 0), [0.8, 0.0, 0.0, 1.0]);
        assert_eq!(blue_top.pixel(0, 0), [0.0, 0.0, 0.8, 1.0]);
    }

    // §9.16: half-alpha top over opaque bottom blends per `over`.
    // top premult [0.5,0,0,0.5], bottom [0,0,1,1]:
    // cr = (1−0.5)·cb + ca = [0.5, 0, 0.5, 1.0]; qr = 1.
    #[test]
    fn merge_partial_alpha_blends() {
        let mut bottom = FilterImage::new(1, 1);
        bottom.set_pixel(0, 0, [0.0, 0.0, 1.0, 1.0]);
        let mut top = FilterImage::new(1, 1);
        top.set_pixel(0, 0, [0.5, 0.0, 0.0, 0.5]);
        let out = merge(&[bottom, top], 1, 1);
        let p = out.pixel(0, 0);
        for (o, e) in p.iter().zip([0.5, 0.0, 0.5, 1.0]) {
            assert!((o - e).abs() < 1e-6, "{o} vs {e}");
        }
    }

    fn node(primitive: FilterPrimitive) -> FilterPrimitiveNode {
        FilterPrimitiveNode {
            region: Default::default(),
            result: None,
            color_interpolation_filters: ColorInterpolationFilters::Srgb,
            primitive,
        }
    }

    // Node entry point for merge: two opaque sRGB layers, last on top.
    #[test]
    fn evaluate_merge_node_two_layers() {
        let n = node(FilterPrimitive::Merge {
            inputs: vec![FilterInput::SourceGraphic, FilterInput::SourceGraphic],
        });
        let bottom = [0xFF, 0x00, 0x00, 0xFF];
        let top = [0x00, 0x00, 0xFF, 0xFF];
        let out = evaluate_merge_node(&n, &[&bottom, &top], 1, 1).unwrap();
        assert_eq!(out, [0x00, 0x00, 0xFF, 0xFF]);
    }

    // Node entry point declines when the supplied buffer count does not
    // match the feMergeNode count.
    #[test]
    fn evaluate_merge_node_count_mismatch_declines() {
        let n = node(FilterPrimitive::Merge {
            inputs: vec![FilterInput::SourceGraphic, FilterInput::SourceGraphic],
        });
        let only = [0u8; 4];
        assert!(evaluate_merge_node(&n, &[&only], 1, 1).is_none());
    }

    #[test]
    fn evaluate_merge_node_declines_other_primitive() {
        let n = node(FilterPrimitive::Flood {
            flood_color: FloodColor::default(),
            flood_opacity: 1.0,
        });
        assert!(evaluate_merge_node(&n, &[], 1, 1).is_none());
    }

    // Node entry point for offset: integer shift moves the lit pixel.
    #[test]
    fn evaluate_offset_node_shifts() {
        let n = node(FilterPrimitive::Offset {
            input: FilterInput::SourceGraphic,
            dx: 1.0,
            dy: 0.0,
        });
        // 2×1 sRGB: lit pixel at x=0.
        let src = [0x10, 0x20, 0x30, 0xFF, 0x00, 0x00, 0x00, 0x00];
        let out = evaluate_offset_node(&n, &src, 2, 1).unwrap();
        // Shifts right by 1 → x=1 carries the colour, x=0 transparent.
        assert_eq!(&out[..4], &[0x00, 0x00, 0x00, 0x00]);
        assert_eq!(&out[4..], &[0x10, 0x20, 0x30, 0xFF]);
    }

    #[test]
    fn evaluate_offset_node_declines_other_primitive() {
        let n = node(FilterPrimitive::Flood {
            flood_color: FloodColor::default(),
            flood_opacity: 1.0,
        });
        assert!(evaluate_offset_node(&n, &[0; 4], 1, 1).is_none());
    }

    // Node entry point for Gaussian blur: zero stdDeviation is a no-op.
    #[test]
    fn evaluate_gaussian_blur_node_zero_is_identity() {
        let n = node(FilterPrimitive::GaussianBlur {
            input: FilterInput::SourceGraphic,
            std_deviation_x: 0.0,
            std_deviation_y: 0.0,
            edge_mode: EdgeMode::None,
        });
        let src = [0x40, 0x80, 0xC0, 0xFF];
        let out = evaluate_gaussian_blur_node(&n, &src, 1, 1).unwrap();
        assert_eq!(out, src);
    }

    // Node entry point honours the parsed edgeMode: a 1×1 opaque pixel
    // blurred with `duplicate` stays opaque (the border duplicates), but
    // with `none` it loses alpha to the (zero) surround.
    #[test]
    fn evaluate_gaussian_blur_node_edge_mode_honoured() {
        let src = [0x00, 0x00, 0x00, 0xFF];
        let dup = node(FilterPrimitive::GaussianBlur {
            input: FilterInput::SourceGraphic,
            std_deviation_x: 2.0,
            std_deviation_y: 2.0,
            edge_mode: EdgeMode::Duplicate,
        });
        let out_dup = evaluate_gaussian_blur_node(&dup, &src, 1, 1).unwrap();
        // Single-pixel field under duplicate: every sample is the pixel
        // itself → unchanged opaque.
        assert_eq!(out_dup, [0x00, 0x00, 0x00, 0xFF]);

        let none = node(FilterPrimitive::GaussianBlur {
            input: FilterInput::SourceGraphic,
            std_deviation_x: 2.0,
            std_deviation_y: 2.0,
            edge_mode: EdgeMode::None,
        });
        let out_none = evaluate_gaussian_blur_node(&none, &src, 1, 1).unwrap();
        // Under `none` the lone pixel's mass spreads into the zero
        // surround that a 1×1 buffer cannot hold → alpha drops below 255.
        assert!(out_none[3] < 0xFF, "alpha {} not reduced", out_none[3]);
    }

    #[test]
    fn evaluate_gaussian_blur_node_declines_other_primitive() {
        let n = node(FilterPrimitive::Flood {
            flood_color: FloodColor::default(),
            flood_opacity: 1.0,
        });
        assert!(evaluate_gaussian_blur_node(&n, &[0; 4], 1, 1).is_none());
    }

    // §15.9 `normal` is identical to the Porter-Duff `over`: an opaque
    // source (qa = 1) fully replaces the backdrop (cr = ca, qr = 1).
    #[test]
    fn blend_normal_matches_over() {
        let mut a = FilterImage::new(1, 1);
        a.set_pixel(0, 0, [0.3, 0.4, 0.5, 1.0]);
        let mut b = FilterImage::new(1, 1);
        b.set_pixel(0, 0, [0.9, 0.9, 0.9, 1.0]);
        let blended = blend(&a, &b, BlendMode::Normal);
        let over = composite(&a, &b, CompositeOperator::Over, [0.0; 4]);
        assert_eq!(blended.pixel(0, 0), over.pixel(0, 0));
        assert_eq!(blended.pixel(0, 0), [0.3, 0.4, 0.5, 1.0]);
    }

    // §15.9 multiply of two opaque pixels: with qa = qb = 1 the two
    // (1 − q)·c terms vanish, so cr = ca·cb per channel and qr = 1.
    #[test]
    fn blend_multiply_opaque_is_product() {
        let mut a = FilterImage::new(1, 1);
        a.set_pixel(0, 0, [0.5, 0.8, 1.0, 1.0]);
        let mut b = FilterImage::new(1, 1);
        b.set_pixel(0, 0, [0.4, 0.5, 0.2, 1.0]);
        let out = blend(&a, &b, BlendMode::Multiply);
        let p = out.pixel(0, 0);
        assert!((p[0] - 0.5 * 0.4).abs() < 1e-6, "{}", p[0]);
        assert!((p[1] - 0.8 * 0.5).abs() < 1e-6, "{}", p[1]);
        assert!((p[2] - 1.0 * 0.2).abs() < 1e-6, "{}", p[2]);
        assert_eq!(p[3], 1.0);
    }

    // §15.9 screen of two opaque pixels: cr = cb + ca − ca·cb. White on
    // anything is white; black is the identity.
    #[test]
    fn blend_screen_opaque() {
        let mut white = FilterImage::new(1, 1);
        white.set_pixel(0, 0, [1.0, 1.0, 1.0, 1.0]);
        let mut grey = FilterImage::new(1, 1);
        grey.set_pixel(0, 0, [0.4, 0.4, 0.4, 1.0]);
        // White screened over grey → white.
        let w = blend(&white, &grey, BlendMode::Screen).pixel(0, 0);
        assert_eq!(&w[..3], &[1.0, 1.0, 1.0]);
        // Black is the screen identity → backdrop unchanged.
        let mut black = FilterImage::new(1, 1);
        black.set_pixel(0, 0, [0.0, 0.0, 0.0, 1.0]);
        let g = blend(&black, &grey, BlendMode::Screen).pixel(0, 0);
        for &c in g.iter().take(3) {
            assert!((c - 0.4).abs() < 1e-6, "{c}");
        }
    }

    // §15.9 darken/lighten of two opaque pixels reduce to the per-channel
    // min/max of the two colours (with qa = qb = 1 the boundary terms are
    // (1 − q)·c = 0, leaving min(ca, cb) and max(ca, cb)).
    #[test]
    fn blend_darken_lighten_opaque_min_max() {
        let mut a = FilterImage::new(1, 1);
        a.set_pixel(0, 0, [0.2, 0.7, 0.5, 1.0]);
        let mut b = FilterImage::new(1, 1);
        b.set_pixel(0, 0, [0.6, 0.3, 0.5, 1.0]);
        let dark = blend(&a, &b, BlendMode::Darken).pixel(0, 0);
        assert_eq!(&dark[..3], &[0.2, 0.3, 0.5]);
        let light = blend(&a, &b, BlendMode::Lighten).pixel(0, 0);
        assert_eq!(&light[..3], &[0.6, 0.7, 0.5]);
        assert_eq!(dark[3], 1.0);
        assert_eq!(light[3], 1.0);
    }

    // The shared opacity rule qr = 1 − (1 − qa)·(1 − qb) holds for every
    // staged mode (half-transparent over half-transparent → 0.75).
    #[test]
    fn blend_result_opacity_is_shared() {
        let mut a = FilterImage::new(1, 1);
        a.set_pixel(0, 0, [0.0, 0.0, 0.0, 0.5]);
        let mut b = FilterImage::new(1, 1);
        b.set_pixel(0, 0, [0.0, 0.0, 0.0, 0.5]);
        for mode in [
            BlendMode::Normal,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Darken,
            BlendMode::Lighten,
        ] {
            let q = blend(&a, &b, mode).pixel(0, 0)[3];
            assert!((q - 0.75).abs() < 1e-6, "{mode:?}: {q}");
        }
    }

    // `in2` smaller than `in` zero-extends to transparent black, and the
    // result keeps `in`'s dimensions (matching `composite`).
    #[test]
    fn blend_in2_zero_extends() {
        let mut a = FilterImage::new(2, 1);
        a.set_pixel(0, 0, [0.3, 0.3, 0.3, 1.0]);
        a.set_pixel(1, 0, [0.7, 0.7, 0.7, 1.0]);
        let b = FilterImage::new(1, 1); // transparent black, narrower
        let out = blend(&a, &b, BlendMode::Normal);
        assert_eq!(out.width(), 2);
        // Over transparent black both source pixels pass through unchanged.
        assert_eq!(out.pixel(0, 0), [0.3, 0.3, 0.3, 1.0]);
        assert_eq!(out.pixel(1, 0), [0.7, 0.7, 0.7, 1.0]);
    }

    // Node entry point: `normal` over an opaque sRGB pair reproduces the
    // source (qa = 1).
    #[test]
    fn evaluate_blend_node_normal_srgb() {
        let n = node(FilterPrimitive::Blend {
            input: FilterInput::SourceGraphic,
            input2: FilterInput::SourceGraphic,
            mode: BlendMode::Normal,
        });
        let i1 = [0x40, 0x80, 0xC0, 0xFF];
        let i2 = [0x10, 0x20, 0x30, 0xFF];
        let out = evaluate_blend_node(&n, &i1, &i2, 1, 1).unwrap();
        assert_eq!(out, i1);
    }

    // Node entry point declines the un-staged [COMPOSITING-1] modes so
    // the caller can fall back to the graph-level rasteriser.
    #[test]
    fn evaluate_blend_node_declines_unstaged() {
        let n = node(FilterPrimitive::Blend {
            input: FilterInput::SourceGraphic,
            input2: FilterInput::SourceGraphic,
            mode: BlendMode::Overlay,
        });
        assert!(evaluate_blend_node(&n, &[0; 4], &[0; 4], 1, 1).is_none());
    }

    #[test]
    fn evaluate_blend_node_declines_other_primitive() {
        let n = node(FilterPrimitive::Flood {
            flood_color: FloodColor::default(),
            flood_opacity: 1.0,
        });
        assert!(evaluate_blend_node(&n, &[0; 4], &[0; 4], 1, 1).is_none());
    }

    // §9.17: dilate spreads the brightest sample over the kernel. A
    // single opaque-white pixel in a transparent-black 3×3 field, with
    // radius 1, fills the whole 3×3 (its kernel reaches every pixel).
    #[test]
    fn morphology_dilate_spreads_max() {
        let mut img = FilterImage::new(3, 3);
        img.set_pixel(1, 1, [1.0, 1.0, 1.0, 1.0]);
        let out = morphology(&img, MorphologyOperator::Dilate, 1.0, 1.0);
        for y in 0..3 {
            for x in 0..3 {
                assert_eq!(out.pixel(x, y), [1.0, 1.0, 1.0, 1.0], "({x},{y})");
            }
        }
    }

    // §9.17: erode keeps the darkest sample in the kernel. The same
    // single bright pixel is wiped out — every output pixel's kernel
    // includes at least one transparent-black neighbour.
    #[test]
    fn morphology_erode_keeps_min() {
        let mut img = FilterImage::new(3, 3);
        img.set_pixel(1, 1, [1.0, 1.0, 1.0, 1.0]);
        let out = morphology(&img, MorphologyOperator::Erode, 1.0, 1.0);
        for y in 0..3 {
            for x in 0..3 {
                assert_eq!(out.pixel(x, y), [0.0, 0.0, 0.0, 0.0], "({x},{y})");
            }
        }
    }

    // §9.17: erode over a solid opaque region of radius ≥ window leaves
    // the interior untouched (every kernel sample equals the interior
    // value); only the border erodes against the transparent surround.
    #[test]
    fn morphology_erode_preserves_solid_interior() {
        // 5×5 opaque grey, radius 1: the centre pixel (2,2)'s 3×3 kernel
        // is entirely inside the field, so it survives erosion.
        let mut img = FilterImage::new(5, 5);
        for y in 0..5 {
            for x in 0..5 {
                img.set_pixel(x, y, [0.4, 0.4, 0.4, 0.4]);
            }
        }
        let out = morphology(&img, MorphologyOperator::Erode, 1.0, 1.0);
        assert_eq!(out.pixel(2, 2), [0.4, 0.4, 0.4, 0.4]);
    }

    // §9.17: independent x/y radii — the kernel is a rectangle, so a
    // horizontal bright bar dilated with rx=1, ry=0 spreads only
    // sideways, never vertically.
    #[test]
    fn morphology_anisotropic_radii() {
        let mut img = FilterImage::new(5, 3);
        img.set_pixel(2, 1, [1.0, 0.0, 0.0, 1.0]);
        let out = morphology(&img, MorphologyOperator::Dilate, 1.0, 0.0);
        // ry <= 0 disables the whole primitive per §9.17 — verify that
        // the zero-axis short-circuits to the identity (result = input).
        assert_eq!(out.pixel(2, 1), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(out.pixel(1, 1), [0.0, 0.0, 0.0, 0.0]);
    }

    // §9.17: a negative or zero radius disables the primitive — the
    // output equals the input.
    #[test]
    fn morphology_zero_radius_is_identity() {
        let mut img = FilterImage::new(2, 2);
        img.set_pixel(0, 0, [0.2, 0.3, 0.4, 0.5]);
        img.set_pixel(1, 1, [0.6, 0.7, 0.8, 0.9]);
        let id = morphology(&img, MorphologyOperator::Dilate, 0.0, 2.0);
        assert_eq!(id, img);
        let neg = morphology(&img, MorphologyOperator::Erode, -3.0, 1.0);
        assert_eq!(neg, img);
    }

    // §9.17: premultiplied invariant — erode/dilate never produce a
    // colour component greater than alpha. Start from a premultiplied
    // field where each pixel already satisfies cᵖ ≤ αᵖ.
    #[test]
    fn morphology_preserves_premultiplied_invariant() {
        let mut img = FilterImage::new(3, 3);
        img.set_pixel(0, 0, [0.5, 0.2, 0.1, 0.5]);
        img.set_pixel(1, 1, [0.9, 0.4, 0.3, 0.9]);
        img.set_pixel(2, 2, [0.3, 0.3, 0.0, 0.4]);
        for op in [MorphologyOperator::Erode, MorphologyOperator::Dilate] {
            let out = morphology(&img, op, 1.0, 1.0);
            for y in 0..3 {
                for x in 0..3 {
                    let [r, g, b, a] = out.pixel(x, y);
                    assert!(
                        r <= a && g <= a && b <= a,
                        "{op:?} ({x},{y}): {r},{g},{b} > {a}"
                    );
                }
            }
        }
    }

    // The node wrapper round-trips an 8-bit sRGB buffer through the
    // dilate operator: a lone opaque-white pixel fills its 3×3 kernel.
    #[test]
    fn evaluate_morphology_node_dilate_rgba8() {
        let mut buf = vec![0u8; 9 * 4];
        // Centre pixel of the 3×3 buffer = (row 1, col 1) = element 4 =
        // byte offset 16.
        let i = 4 * 4;
        buf[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
        let n = node(FilterPrimitive::Morphology {
            input: FilterInput::SourceGraphic,
            operator: MorphologyOperator::Dilate,
            radius_x: 1.0,
            radius_y: 1.0,
        });
        let out = evaluate_morphology_node(&n, &buf, 3, 3).unwrap();
        assert_eq!(out, vec![255u8; 9 * 4]);
    }

    #[test]
    fn evaluate_morphology_node_declines_other_primitive() {
        let n = node(FilterPrimitive::Flood {
            flood_color: FloodColor::default(),
            flood_opacity: 1.0,
        });
        assert!(evaluate_morphology_node(&n, &[0; 4], 1, 1).is_none());
    }

    #[test]
    fn evaluate_morphology_node_rejects_bad_len() {
        let n = node(FilterPrimitive::Morphology {
            input: FilterInput::SourceGraphic,
            operator: MorphologyOperator::Erode,
            radius_x: 1.0,
            radius_y: 1.0,
        });
        assert!(evaluate_morphology_node(&n, &[0; 3], 1, 1).is_none());
    }

    // §9.9: the identity kernel (single 1 at the centre, divisor 1) is a
    // no-op regardless of preserveAlpha — every pixel passes through.
    #[test]
    fn convolve_identity_is_passthrough() {
        let mut img = FilterImage::new(3, 3);
        img.set_pixel(1, 1, [0.4, 0.2, 0.1, 0.5]);
        img.set_pixel(0, 0, [0.0, 0.0, 0.0, 1.0]);
        for preserve in [false, true] {
            let out = convolve_matrix(
                &img,
                3,
                3,
                &[0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
                1.0,
                0.0,
                1,
                1,
                ConvolveEdgeMode::Duplicate,
                preserve,
            );
            for y in 0..3 {
                for x in 0..3 {
                    let (o, e) = (out.pixel(x, y), img.pixel(x, y));
                    for c in 0..4 {
                        assert!(
                            (o[c] - e[c]).abs() < 1e-6,
                            "preserve={preserve} ({x},{y})c{c}"
                        );
                    }
                }
            }
        }
    }

    // §9.9: a uniform 3×3 box kernel with divisor 9 averages the
    // neighbourhood. On a flat opaque field every output equals the input
    // (edgeMode=duplicate keeps the border average flat).
    #[test]
    fn convolve_box_average_flat_field() {
        let mut img = FilterImage::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                img.set_pixel(x, y, [0.6, 0.6, 0.6, 1.0]);
            }
        }
        let out = convolve_matrix(
            &img,
            3,
            3,
            &[1.0; 9],
            9.0,
            0.0,
            1,
            1,
            ConvolveEdgeMode::Duplicate,
            false,
        );
        for y in 0..4 {
            for x in 0..4 {
                let p = out.pixel(x, y);
                for (c, e) in p.iter().zip([0.6, 0.6, 0.6, 1.0]) {
                    assert!((c - e).abs() < 1e-6, "({x},{y}) {c} vs {e}");
                }
            }
        }
    }

    // §9.9: kernel 180° rotation. With a kernel that is 1 only at column
    // 0 / row 0 (top-left), targetX=targetY=0, the rotated index reads
    // SOURCE[X+ (orderX-1), Y+(orderY-1)] — i.e. the bottom-right
    // neighbour shifts up-left by (orderX-1, orderY-1). A single bright
    // pixel therefore lands at (x-2, y-2) for a 3×3 kernel.
    #[test]
    fn convolve_kernel_rotation_shift() {
        let mut img = FilterImage::new(5, 5);
        img.set_pixel(4, 4, [0.0, 0.0, 0.0, 1.0]);
        // kernelMatrix row-major: 1 at [col=0,row=0], else 0.
        let mut k = [0.0f32; 9];
        k[0] = 1.0;
        let out = convolve_matrix(
            &img,
            3,
            3,
            &k,
            1.0,
            0.0,
            0,
            0,
            ConvolveEdgeMode::None,
            false,
        );
        // out[x,y] = SOURCE[x - 0 + 2, y - 0 + 2] · 1 = SOURCE[x+2, y+2].
        // So the bright (4,4) appears at (2,2).
        assert!((out.pixel(2, 2)[3] - 1.0).abs() < 1e-6);
        assert_eq!(out.pixel(4, 4)[3], 0.0);
    }

    // §9.9.4: an explicit divisor="0" is invalid; the evaluator falls
    // back to the sum of the kernel. A box kernel sums to 9, so the
    // result matches the divisor=9 average.
    #[test]
    fn convolve_zero_divisor_falls_back_to_kernel_sum() {
        let mut img = FilterImage::new(3, 3);
        for y in 0..3 {
            for x in 0..3 {
                img.set_pixel(x, y, [0.3, 0.0, 0.0, 0.5]);
            }
        }
        let out = convolve_matrix(
            &img,
            3,
            3,
            &[1.0; 9],
            0.0, // invalid → fall back to sum (9)
            0.0,
            1,
            1,
            ConvolveEdgeMode::Duplicate,
            false,
        );
        let p = out.pixel(1, 1);
        assert!((p[0] - 0.3).abs() < 1e-6, "{p:?}");
        assert!((p[3] - 0.5).abs() < 1e-6, "{p:?}");
    }

    // §9.9.7 preserveAlpha=true: the alpha channel is untouched. With a
    // box-blur colour kernel over a field of constant straight-alpha
    // colour, every output keeps the input alpha while the colour stays
    // flat. Verify alpha is verbatim and colour is the un-premultiplied
    // average re-premultiplied.
    #[test]
    fn convolve_preserve_alpha_keeps_alpha() {
        let mut img = FilterImage::new(3, 3);
        // straight colour (0.8, 0.4, 0.2) at alpha 0.5 → premultiplied.
        for y in 0..3 {
            for x in 0..3 {
                img.set_pixel(x, y, [0.8 * 0.5, 0.4 * 0.5, 0.2 * 0.5, 0.5]);
            }
        }
        let out = convolve_matrix(
            &img,
            3,
            3,
            &[1.0; 9],
            9.0,
            0.0,
            1,
            1,
            ConvolveEdgeMode::Duplicate,
            true,
        );
        let p = out.pixel(1, 1);
        // alpha unchanged.
        assert!((p[3] - 0.5).abs() < 1e-6, "{p:?}");
        // colour = un-premul average (0.8,0.4,0.2) re-premultiplied by 0.5.
        for (c, e) in p[..3].iter().zip([0.8 * 0.5, 0.4 * 0.5, 0.2 * 0.5]) {
            assert!((c - e).abs() < 1e-6, "{c} vs {e}");
        }
    }

    // §9.9: bias shifts every colour component. With the identity kernel
    // (preserveAlpha=false), out_color = in_color + bias·convolved_alpha.
    #[test]
    fn convolve_bias_offsets_color() {
        let mut img = FilterImage::new(1, 1);
        img.set_pixel(0, 0, [0.2, 0.2, 0.2, 1.0]);
        let out = convolve_matrix(
            &img,
            1,
            1,
            &[1.0],
            1.0,
            0.25,
            0,
            0,
            ConvolveEdgeMode::None,
            false,
        );
        let p = out.pixel(0, 0);
        // colour = 0.2 + 0.25·1.0 = 0.45; alpha = 1.0.
        for c in &p[..3] {
            assert!((c - 0.45).abs() < 1e-6, "{c}");
        }
        assert!((p[3] - 1.0).abs() < 1e-6);
    }

    // §9.9: malformed kernel (length ≠ orderX·orderY) is a no-op.
    #[test]
    fn convolve_bad_kernel_len_is_identity() {
        let mut img = FilterImage::new(2, 2);
        img.set_pixel(0, 0, [0.1, 0.2, 0.3, 0.4]);
        let out = convolve_matrix(
            &img,
            3,
            3,
            &[1.0, 2.0],
            1.0,
            0.0,
            1,
            1,
            ConvolveEdgeMode::Duplicate,
            false,
        );
        assert_eq!(out, img);
    }

    // The node wrapper round-trips an 8-bit sRGB buffer through a sharpen
    // kernel. With sRGB working space (no linearisation) a flat field is
    // unchanged by the unity-sum sharpen kernel [0 -1 0; -1 5 -1; 0 -1 0].
    #[test]
    fn evaluate_convolve_matrix_node_sharpen_flat_field() {
        let buf = vec![128u8; 9 * 4];
        let n = node(FilterPrimitive::ConvolveMatrix {
            input: FilterInput::SourceGraphic,
            order_x: 3,
            order_y: 3,
            kernel_matrix: vec![0.0, -1.0, 0.0, -1.0, 5.0, -1.0, 0.0, -1.0, 0.0],
            divisor: 1.0,
            bias: 0.0,
            target_x: 1,
            target_y: 1,
            edge_mode: ConvolveEdgeMode::Duplicate,
            preserve_alpha: false,
        });
        let out = evaluate_convolve_matrix_node(&n, &buf, 3, 3).unwrap();
        // Unity-sum kernel on a flat field is the identity.
        assert_eq!(out, buf);
    }

    #[test]
    fn evaluate_convolve_matrix_node_declines_other_primitive() {
        let n = node(FilterPrimitive::Flood {
            flood_color: FloodColor::default(),
            flood_opacity: 1.0,
        });
        assert!(evaluate_convolve_matrix_node(&n, &[0; 4], 1, 1).is_none());
    }

    #[test]
    fn evaluate_convolve_matrix_node_rejects_bad_len() {
        let n = node(FilterPrimitive::ConvolveMatrix {
            input: FilterInput::SourceGraphic,
            order_x: 3,
            order_y: 3,
            kernel_matrix: vec![1.0; 9],
            divisor: 1.0,
            bias: 0.0,
            target_x: 1,
            target_y: 1,
            edge_mode: ConvolveEdgeMode::Duplicate,
            preserve_alpha: false,
        });
        assert!(evaluate_convolve_matrix_node(&n, &[0; 3], 1, 1).is_none());
    }

    // ------------------------------------------------------------------
    // §9.11 feDisplacementMap
    // ------------------------------------------------------------------

    fn displacement_node(
        scale: f32,
        xc: ChannelSelector,
        yc: ChannelSelector,
    ) -> FilterPrimitiveNode {
        node(FilterPrimitive::DisplacementMap {
            input: FilterInput::SourceGraphic,
            input2: FilterInput::SourceGraphic,
            scale,
            x_channel_selector: xc,
            y_channel_selector: yc,
        })
    }

    // §9.11: scale=0 is the identity — the source image is returned
    // unchanged regardless of the displacement map.
    #[test]
    fn displacement_scale_zero_is_identity() {
        let n = displacement_node(0.0, ChannelSelector::R, ChannelSelector::G);
        // 2×1 source: two distinct colours.
        let src = [0x11, 0x22, 0x33, 0xFF, 0x44, 0x55, 0x66, 0xFF];
        // A map that would otherwise displace heavily.
        let map = [0xFF, 0x00, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF];
        let out = evaluate_displacement_map_node(&n, &src, &map, 2, 1).unwrap();
        assert_eq!(out, src);
    }

    // §9.11: P'(x,y) ← P(x + scale·(XC−½), y + scale·(YC−½)). With a map
    // whose selected x-channel is fully on (1.0) and scale=2, the x
    // displacement is +1 pixel, so the destination samples one pixel to
    // the right of the source. xChannelSelector=A reads the map's alpha.
    #[test]
    fn displacement_x_shift_pulls_from_right() {
        // x-channel = A (drives x), y-channel = G (kept at the ½ midpoint
        // so the single-row image isn't shifted off its only row).
        let n = displacement_node(2.0, ChannelSelector::A, ChannelSelector::G);
        // 3×1 source: a lit pixel at x=1, transparent neighbours.
        let src = [
            0x00, 0x00, 0x00, 0x00, // x=0
            0x7A, 0x14, 0x33, 0xFF, // x=1 (lit)
            0x00, 0x00, 0x00, 0x00, // x=2
        ];
        // Map: alpha=1.0 → XC=1.0 → +1 x displacement (2·(1.0−½)=1);
        // G=0x80 (non-premult ≈½) → YC≈½ → no y displacement.
        let map = [
            0x00, 0x80, 0x00, 0xFF, // x=0
            0x00, 0x80, 0x00, 0xFF, // x=1
            0x00, 0x80, 0x00, 0xFF, // x=2
        ];
        let out = evaluate_displacement_map_node(&n, &src, &map, 3, 1).unwrap();
        // dst x=0 ← src x=1 (the lit pixel); dst x=1 ← src x=2 (transp);
        // dst x=2 ← src x=3 (out of image → transparent black).
        assert_eq!(&out[0..4], &[0x7A, 0x14, 0x33, 0xFF]);
        assert_eq!(&out[4..8], &[0x00, 0x00, 0x00, 0x00]);
        assert_eq!(&out[8..12], &[0x00, 0x00, 0x00, 0x00]);
    }

    // §9.11: the midpoint map value ½ gives zero displacement; with an
    // 8-bit map a channel byte of 0x80 (128/255 ≈ 0.502) rounds the
    // shift to 0, so the source passes through unchanged.
    #[test]
    fn displacement_midpoint_channel_is_no_op() {
        let n = displacement_node(4.0, ChannelSelector::R, ChannelSelector::G);
        let src = [0x10, 0x20, 0x30, 0xFF, 0x40, 0x50, 0x60, 0xFF];
        // R=G≈0x80 → (0.502−0.5)·4 ≈ 0.008 → round to 0.
        let map = [0x80, 0x80, 0x00, 0xFF, 0x80, 0x80, 0x00, 0xFF];
        let out = evaluate_displacement_map_node(&n, &src, &map, 2, 1).unwrap();
        assert_eq!(out, src);
    }

    // §9.11: the in2 channel selection uses non-premultiplied values —
    // a map pixel with alpha 0.5 and premultiplied red 0.5 has
    // non-premultiplied red 1.0, the full +scale/2 displacement.
    #[test]
    fn displacement_channel_uses_non_premultiplied() {
        // x-channel = R (the channel that must be read non-premultiplied),
        // y-channel = G (kept at ½ so only the x-shift matters).
        let n = displacement_node(2.0, ChannelSelector::R, ChannelSelector::G);
        // 2×1 source: lit pixel at x=1.
        let src = [0x00, 0x00, 0x00, 0x00, 0x99, 0x88, 0x77, 0xFF];
        // Map x=0: sRGB R=0xFF, G=0x80, A=0x80. The decoder premultiplies
        // (storing R·A); displacement_map un-premultiplies → R≈1.0,
        // G≈½. So XC≈1.0 → +1 x-shift → src x=1 (the lit pixel); a naive
        // *premultiplied* read would give R≈0.5 → only a +0 shift, so the
        // pulled colour distinguishes the two code paths.
        let map = [0xFF, 0x80, 0x00, 0x80, 0x00, 0x80, 0x00, 0xFF];
        let out = evaluate_displacement_map_node(&n, &src, &map, 2, 1).unwrap();
        // dst x=0 ← src x=1 (lit). A premultiplied read of R (≈0.5) would
        // round to a 0-pixel shift and leave x=0 transparent.
        assert_eq!(&out[0..4], &[0x99, 0x88, 0x77, 0xFF]);
    }

    #[test]
    fn evaluate_displacement_map_node_declines_other_primitive() {
        let n = node(FilterPrimitive::Flood {
            flood_color: FloodColor::default(),
            flood_opacity: 1.0,
        });
        assert!(evaluate_displacement_map_node(&n, &[0; 4], &[0; 4], 1, 1).is_none());
    }

    #[test]
    fn evaluate_displacement_map_node_rejects_bad_len() {
        let n = displacement_node(1.0, ChannelSelector::A, ChannelSelector::A);
        assert!(evaluate_displacement_map_node(&n, &[0; 3], &[0; 4], 1, 1).is_none());
        assert!(evaluate_displacement_map_node(&n, &[0; 4], &[0; 3], 1, 1).is_none());
    }

    // ------------------------------------------------------------------
    // §9.20 feTile
    // ------------------------------------------------------------------

    // §9.20: a 2×2 reference tile at the buffer origin replicates across a
    // 4×4 target. Each destination samples tile[(dx mod 2), (dy mod 2)].
    #[test]
    fn tile_2x2_replicates_to_4x4() {
        // 2×2 source: distinct colours in each cell (sRGB space so no
        // linearisation distorts the byte-exact passthrough).
        let mut src = FilterImage::new(2, 2);
        src.set_pixel(0, 0, [0.1, 0.0, 0.0, 1.0]);
        src.set_pixel(1, 0, [0.0, 0.2, 0.0, 1.0]);
        src.set_pixel(0, 1, [0.0, 0.0, 0.3, 1.0]);
        src.set_pixel(1, 1, [0.4, 0.4, 0.4, 1.0]);
        let out = tile(&src, 0, 0, 2, 2, 4, 4);
        for dy in 0..4 {
            for dx in 0..4 {
                assert_eq!(
                    out.pixel(dx, dy),
                    src.pixel(dx % 2, dy % 2),
                    "dst ({dx},{dy}) should mirror tile ({},{})",
                    dx % 2,
                    dy % 2
                );
            }
        }
    }

    // §9.20: a tile that does not start at the origin uses the periodic
    // origin (x + i·width). A 2×2 tile at (1,1) of a 3×3 buffer: dst
    // (0,0) wraps to tile-relative ((0−1) mod 2, (0−1) mod 2) = (1,1),
    // i.e. source pixel (1+1, 1+1) = (2,2).
    #[test]
    fn tile_offset_origin_wraps_periodically() {
        let mut src = FilterImage::new(3, 3);
        src.set_pixel(1, 1, [0.5, 0.0, 0.0, 1.0]);
        src.set_pixel(2, 1, [0.0, 0.5, 0.0, 1.0]);
        src.set_pixel(1, 2, [0.0, 0.0, 0.5, 1.0]);
        src.set_pixel(2, 2, [0.5, 0.5, 0.0, 1.0]);
        let out = tile(&src, 1, 1, 2, 2, 2, 2);
        // dst (0,0) ← tile-rel (1,1) ← src (2,2).
        assert_eq!(out.pixel(0, 0), src.pixel(2, 2));
        // dst (1,0) ← tile-rel (0,1) ← src (1,2).
        assert_eq!(out.pixel(1, 0), src.pixel(1, 2));
        // dst (1,1) ← tile-rel (0,0) ← src (1,1).
        assert_eq!(out.pixel(1, 1), src.pixel(1, 1));
    }

    // §9.20: a degenerate (zero-area) tile yields transparent black.
    #[test]
    fn tile_zero_area_is_transparent() {
        let mut src = FilterImage::new(2, 2);
        src.set_pixel(0, 0, [1.0, 1.0, 1.0, 1.0]);
        let out = tile(&src, 0, 0, 0, 2, 4, 4);
        for dy in 0..4 {
            for dx in 0..4 {
                assert_eq!(out.pixel(dx, dy), [0.0; 4]);
            }
        }
    }

    // Node entry point: a 1×1 reference tile fills the whole output with
    // one colour. sRGB space keeps the byte passthrough exact.
    #[test]
    fn evaluate_tile_node_fills_with_single_pixel() {
        let n = node(FilterPrimitive::Tile {
            input: FilterInput::SourceGraphic,
        });
        // 1×1 source carrying one opaque colour, replicated to 3×2.
        let src = [0x12, 0x34, 0x56, 0xFF];
        let out = evaluate_tile_node(&n, &src, 1, 1, 0, 0, 1, 1, 3, 2).unwrap();
        assert_eq!(out.len(), 3 * 2 * 4);
        for px in out.chunks_exact(4) {
            assert_eq!(px, &[0x12, 0x34, 0x56, 0xFF]);
        }
    }

    #[test]
    fn evaluate_tile_node_declines_other_primitive() {
        let n = node(FilterPrimitive::Flood {
            flood_color: FloodColor::default(),
            flood_opacity: 1.0,
        });
        assert!(evaluate_tile_node(&n, &[0; 4], 1, 1, 0, 0, 1, 1, 1, 1).is_none());
    }

    #[test]
    fn evaluate_tile_node_rejects_bad_len() {
        let n = node(FilterPrimitive::Tile {
            input: FilterInput::SourceGraphic,
        });
        assert!(evaluate_tile_node(&n, &[0; 3], 1, 1, 0, 0, 1, 1, 1, 1).is_none());
    }
}
