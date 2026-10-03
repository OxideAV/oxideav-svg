//! The image-crate contract's pixel-side records, specialised to SVG.
//!
//! SVG is a vector format, so this crate never *produces* an
//! [`SvgImage`] — rasterising lives in `oxideav-raster`, through the
//! framework. The types exist so the contract vocabulary is complete
//! and identical across crates: [`crate::info`] describes the canvas a
//! rasteriser would produce ([`ImageInfo`]), [`SvgImage`] is the raster
//! record a caller may assemble (e.g. from a framework frame via the
//! `registry` bridge) and hand to [`crate::encode`], which answers
//! `Unsupported` because an SVG is not a raster target.

use crate::error::{Result, SvgError};

/// Pixel layouts the SVG contract surface speaks. Variant names mirror
/// `oxideav_core::PixelFormat`.
///
/// `Rgba` is the layout a rasteriser produces for an SVG canvas
/// (transparent where nothing is painted); `Rgb24` is accepted by the
/// [`SvgImage`] constructors so a caller-assembled opaque raster has a
/// contract shape too. Neither is ever written by this crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SvgPixelFormat {
    /// Packed 8-bit RGBA, straight alpha, 4 bytes per pixel.
    Rgba,
    /// Packed 8-bit RGB, 3 bytes per pixel.
    Rgb24,
}

/// Contract alias: `oxideav_svg::PixelFormat`.
pub type PixelFormat = SvgPixelFormat;

impl SvgPixelFormat {
    /// Bytes per pixel of the packed layout.
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba => 4,
            Self::Rgb24 => 3,
        }
    }

    /// `true` when the layout carries alpha.
    pub fn has_alpha(self) -> bool {
        matches!(self, Self::Rgba)
    }
}

/// One image plane: row stride in bytes plus the bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Plane {
    /// Bytes from one row to the next.
    pub stride: usize,
    /// Row-major samples; at least `stride × height` bytes.
    pub data: Vec<u8>,
}

impl Plane {
    /// Build a plane.
    pub fn new(stride: usize, data: Vec<u8>) -> Self {
        Self { stride, data }
    }
}

/// Signalled sample range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ColorRange {
    /// Not signalled.
    #[default]
    Unspecified,
    /// Limited (studio) range.
    Limited,
    /// Full range.
    Full,
}

/// Colour signalling as H.273 code points plus the range.
///
/// SVG colours are sRGB by definition (SVG 1.1 §4.2 / CSS Color: the
/// `<color>` type is in the sRGB colour space), so the SVG default is
/// [`ColorInfo::srgb`] — full range, BT.709 primaries (`1`), the
/// IEC 61966-2-1 transfer (`13`) and the identity matrix (`0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ColorInfo {
    /// Sample range.
    pub range: ColorRange,
    /// H.273 colour primaries code point.
    pub primaries: u8,
    /// H.273 transfer characteristics code point.
    pub transfer: u8,
    /// H.273 matrix coefficients code point.
    pub matrix: u8,
}

impl ColorInfo {
    /// H.273 "unspecified" code point.
    pub const UNSPECIFIED: u8 = 2;
    /// H.273 identity (RGB) matrix.
    pub const MATRIX_IDENTITY: u8 = 0;
    /// H.273 BT.709 / sRGB primaries.
    pub const PRIMARIES_BT709: u8 = 1;
    /// H.273 IEC 61966-2-1 (sRGB) transfer.
    pub const TRANSFER_SRGB: u8 = 13;

    /// Build a record from its four fields.
    pub const fn new(range: ColorRange, primaries: u8, transfer: u8, matrix: u8) -> Self {
        Self {
            range,
            primaries,
            transfer,
            matrix,
        }
    }

    /// Everything unspecified.
    pub const fn unspecified() -> Self {
        Self::new(
            ColorRange::Unspecified,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
        )
    }

    /// sRGB — the SVG colour space: full range, primaries `1`,
    /// transfer `13`, matrix `0`.
    pub const fn srgb() -> Self {
        Self::new(
            ColorRange::Full,
            Self::PRIMARIES_BT709,
            Self::TRANSFER_SRGB,
            Self::MATRIX_IDENTITY,
        )
    }

    /// Set the range.
    pub fn with_range(mut self, range: ColorRange) -> Self {
        self.range = range;
        self
    }

    /// Set the primaries code point.
    pub fn with_primaries(mut self, primaries: u8) -> Self {
        self.primaries = primaries;
        self
    }

    /// Set the transfer code point.
    pub fn with_transfer(mut self, transfer: u8) -> Self {
        self.transfer = transfer;
        self
    }

    /// Set the matrix code point.
    pub fn with_matrix(mut self, matrix: u8) -> Self {
        self.matrix = matrix;
        self
    }

    /// `true` when primaries and transfer are both signalled.
    pub fn is_specified(&self) -> bool {
        self.primaries != Self::UNSPECIFIED && self.transfer != Self::UNSPECIFIED
    }
}

impl Default for ColorInfo {
    /// [`ColorInfo::srgb`].
    fn default() -> Self {
        Self::srgb()
    }
}

/// Embedded metadata blobs. SVG carries no ICC or Exif payload; an
/// XMP packet may ride inside `<metadata>` and is surfaced verbatim.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Metadata {
    /// ICC profile — never set for SVG.
    pub icc: Option<Vec<u8>>,
    /// Exif payload — never set for SVG.
    pub exif: Option<Vec<u8>>,
    /// XMP packet (`<x:xmpmeta>…</x:xmpmeta>` serialised as UTF-8).
    pub xmp: Option<Vec<u8>>,
    /// Encoding gamma as an exponent — SVG expresses none.
    pub gamma: Option<f32>,
}

impl Metadata {
    /// Empty metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the ICC profile.
    pub fn with_icc(mut self, icc: impl Into<Option<Vec<u8>>>) -> Self {
        self.icc = icc.into();
        self
    }

    /// Set the Exif payload.
    pub fn with_exif(mut self, exif: impl Into<Option<Vec<u8>>>) -> Self {
        self.exif = exif.into();
        self
    }

    /// Set the XMP packet.
    pub fn with_xmp(mut self, xmp: impl Into<Option<Vec<u8>>>) -> Self {
        self.xmp = xmp.into();
        self
    }

    /// Set the gamma exponent.
    pub fn with_gamma(mut self, gamma: impl Into<Option<f32>>) -> Self {
        self.gamma = gamma.into();
        self
    }

    /// `true` when no field is set.
    pub fn is_empty(&self) -> bool {
        self.icc.is_none() && self.exif.is_none() && self.xmp.is_none() && self.gamma.is_none()
    }
}

/// A raster image in the contract shape (one packed plane).
///
/// This crate never decodes into one — see the module docs — but a
/// caller can assemble one (or convert a framework `VideoFrame` under
/// `registry`) and hold it in the same shape every image crate uses.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SvgImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Packed layout of the single plane.
    pub format: PixelFormat,
    /// Exactly one plane.
    pub planes: Vec<Plane>,
    /// Colour signalling; [`ColorInfo::srgb`] by default.
    pub color: ColorInfo,
    /// Embedded metadata.
    pub metadata: Metadata,
}

impl SvgImage {
    /// Assemble an image from its geometry, layout and planes (exactly
    /// one). The geometry is validated so an invalid image cannot be
    /// built: non-zero dimensions, one plane, a stride of at least
    /// `width × bytes_per_pixel`, and at least `stride × height` bytes
    /// of data ([`SvgError::InvalidData`] otherwise).
    pub fn new(width: u32, height: u32, format: PixelFormat, planes: Vec<Plane>) -> Result<Self> {
        let img = Self {
            width,
            height,
            format,
            planes,
            color: ColorInfo::srgb(),
            metadata: Metadata::default(),
        };
        img.validate()?;
        Ok(img)
    }

    /// A packed image over `data` with the layout's tight stride,
    /// validated like [`SvgImage::new`].
    pub fn packed(width: u32, height: u32, format: PixelFormat, data: Vec<u8>) -> Result<Self> {
        let stride = (width as usize)
            .checked_mul(format.bytes_per_pixel())
            .ok_or_else(|| SvgError::invalid("SVG: row size overflows"))?;
        Self::new(width, height, format, vec![Plane::new(stride, data)])
    }

    /// A packed `Rgb24` image over `data` (`3 × width × height` bytes).
    pub fn from_rgb8(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        Self::packed(width, height, PixelFormat::Rgb24, data)
    }

    /// A packed `Rgba` image over `data` (`4 × width × height` bytes).
    pub fn from_rgba8(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        Self::packed(width, height, PixelFormat::Rgba, data)
    }

    /// Set the colour signalling.
    pub fn with_color(mut self, color: ColorInfo) -> Self {
        self.color = color;
        self
    }

    /// Set the metadata record.
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The packed layout.
    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// Bytes per pixel of the layout.
    pub fn bytes_per_pixel(&self) -> usize {
        self.format.bytes_per_pixel()
    }

    /// Row stride of the plane.
    pub fn stride(&self) -> usize {
        self.planes
            .first()
            .map(|p| p.stride)
            .unwrap_or(self.width as usize * self.bytes_per_pixel())
    }

    /// The single plane's bytes (always `Some` for a packed layout).
    pub fn as_bytes(&self) -> Option<&[u8]> {
        self.planes.first().map(|p| p.data.as_slice())
    }

    /// The plane's bytes (empty when the image has no plane).
    pub fn data(&self) -> &[u8] {
        self.planes
            .first()
            .map(|p| p.data.as_slice())
            .unwrap_or(&[])
    }

    /// Consume the image and return its plane bytes.
    pub fn into_raw(self) -> Vec<u8> {
        let mut planes = self.planes;
        match planes.len() {
            1 => planes.pop().map(|p| p.data).unwrap_or_default(),
            _ => planes.into_iter().flat_map(|p| p.data).collect(),
        }
    }

    /// `true` when the layout carries alpha.
    pub fn has_alpha(&self) -> bool {
        self.format.has_alpha()
    }

    /// Re-check the geometry [`SvgImage::new`] enforced.
    pub fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err(SvgError::invalid(format!(
                "SVG: degenerate image {}×{}",
                self.width, self.height
            )));
        }
        if self.planes.len() != 1 {
            return Err(SvgError::invalid(format!(
                "SVG: packed layout needs exactly one plane, got {}",
                self.planes.len()
            )));
        }
        let plane = &self.planes[0];
        let min_stride = (self.width as usize)
            .checked_mul(self.bytes_per_pixel())
            .ok_or_else(|| SvgError::invalid("SVG: row size overflows"))?;
        if plane.stride < min_stride {
            return Err(SvgError::invalid(format!(
                "SVG: stride {} shorter than {} bytes per row",
                plane.stride, min_stride
            )));
        }
        let need = plane
            .stride
            .checked_mul(self.height as usize)
            .ok_or_else(|| SvgError::invalid("SVG: plane size overflows"))?;
        if plane.data.len() < need {
            return Err(SvgError::invalid(format!(
                "SVG: plane has {} bytes, geometry needs {need}",
                plane.data.len()
            )));
        }
        Ok(())
    }

    /// Tightly packed RGBA8 (`4 × width` bytes per row); `Rgb24`
    /// sources get alpha `255`.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let stride = self.stride();
        let data = self.data();
        let mut out = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            let row = &data[y * stride..];
            match self.format {
                PixelFormat::Rgba => out.extend_from_slice(&row[..w * 4]),
                PixelFormat::Rgb24 => {
                    for px in row[..w * 3].chunks_exact(3) {
                        out.extend_from_slice(&[px[0], px[1], px[2], 255]);
                    }
                }
            }
        }
        out
    }

    /// Tightly packed RGB8 (`3 × width` bytes per row); `Rgba` sources
    /// drop alpha.
    pub fn to_rgb8(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let stride = self.stride();
        let data = self.data();
        let mut out = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            let row = &data[y * stride..];
            match self.format {
                PixelFormat::Rgba => {
                    for px in row[..w * 4].chunks_exact(4) {
                        out.extend_from_slice(&px[..3]);
                    }
                }
                PixelFormat::Rgb24 => out.extend_from_slice(&row[..w * 3]),
            }
        }
        out
    }

    /// [`SvgImage::to_rgba8`] after re-validating.
    pub fn try_to_rgba8(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(self.to_rgba8())
    }

    /// [`SvgImage::to_rgb8`] after re-validating.
    pub fn try_to_rgb8(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(self.to_rgb8())
    }
}

/// Tightly packed RGB8, 3 bytes per pixel, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RgbImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `3 × width × height` bytes.
    pub data: Vec<u8>,
}

impl RgbImage {
    /// Build from parts.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            data,
        }
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Consume into the bytes.
    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }

    /// `3 × width`.
    pub fn stride(&self) -> usize {
        self.width as usize * 3
    }
}

/// Tightly packed RGBA8, 4 bytes per pixel, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `4 × width × height` bytes.
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// Build from parts.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            data,
        }
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Consume into the bytes.
    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }

    /// `4 × width`.
    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }
}

/// What [`crate::info`] reports: the raster canvas a rasteriser would
/// produce for the document, plus the vector-side facts a caller needs
/// before parsing.
///
/// `width` / `height` are CSS pixels at 96 dpi — the root `width` /
/// `height` with absolute units converted (`in`, `cm`, `mm`, `Q`, `pt`,
/// `pc`, `px`; a bare number is a user unit = 1 px), percentages and
/// font-relative lengths falling back to the `viewBox`, a missing axis
/// derived from the other via the `viewBox` aspect ratio, and the
/// result rounded up to whole pixels. A document with neither an
/// absolute size nor a `viewBox` has no intrinsic size and reports
/// `0 × 0`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ImageInfo {
    /// Canvas width in CSS px (`0` when the document has no intrinsic
    /// size).
    pub width: u32,
    /// Canvas height in CSS px.
    pub height: u32,
    /// Always `Rgba`: the layout a rasteriser produces.
    pub format: PixelFormat,
    /// Always `1`. SMIL animation is a continuous timeline sampled by
    /// [`crate::parse_at`], not a frame sequence; see `animated`.
    pub frames: u32,
    /// Always `true`: a vector canvas is transparent where unpainted.
    pub has_alpha: bool,
    /// Always [`ColorInfo::srgb`]: SVG colours are sRGB by definition.
    pub color: ColorInfo,
    /// Always `false`: SVG embeds no ICC profile.
    pub has_icc: bool,
    /// Always `false`: SVG embeds no Exif payload.
    pub has_exif: bool,
    /// `true` when a `<metadata>` element carries an XMP packet
    /// (`<x:xmpmeta` or `<rdf:RDF`).
    pub has_xmp: bool,
    /// The root `width` as user units before CSS resolution — what
    /// [`crate::parse`] stores in `SvgDocument::width`.
    pub user_width: f32,
    /// The root `height` as user units (see `user_width`).
    pub user_height: f32,
    /// The root `viewBox`, when present.
    pub view_box: Option<crate::model::ViewBox>,
    /// `true` when the document contains SMIL animation elements
    /// (`<animate>`, `<set>`, `<animateTransform>`, `<animateMotion>`)
    /// or CSS `@keyframes`.
    pub animated: bool,
    /// `true` when the input was gzip-compressed (`.svgz`).
    pub compressed: bool,
}

impl ImageInfo {
    /// A record for a `width × height` canvas with the SVG constants
    /// filled in (`Rgba`, one frame, alpha, sRGB, no ICC / Exif / XMP,
    /// no view box, not animated, not compressed).
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            format: PixelFormat::Rgba,
            frames: 1,
            has_alpha: true,
            color: ColorInfo::srgb(),
            has_icc: false,
            has_exif: false,
            has_xmp: false,
            user_width: width as f32,
            user_height: height as f32,
            view_box: None,
            animated: false,
            compressed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_validate_geometry() {
        assert!(SvgImage::from_rgba8(2, 2, vec![0; 16]).is_ok());
        assert!(matches!(
            SvgImage::from_rgba8(2, 2, vec![0; 15]),
            Err(SvgError::InvalidData(_))
        ));
        assert!(matches!(
            SvgImage::from_rgb8(0, 2, vec![]),
            Err(SvgError::InvalidData(_))
        ));
        assert!(matches!(
            SvgImage::new(1, 1, PixelFormat::Rgba, vec![]),
            Err(SvgError::InvalidData(_))
        ));
        assert!(matches!(
            SvgImage::new(2, 1, PixelFormat::Rgba, vec![Plane::new(4, vec![0; 8])]),
            Err(SvgError::InvalidData(_))
        ));
    }

    #[test]
    fn rgb_rgba_conversions_are_exact() {
        let img = SvgImage::from_rgb8(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(img.to_rgba8(), vec![1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(img.to_rgb8(), vec![1, 2, 3, 4, 5, 6]);
        let img = SvgImage::from_rgba8(1, 2, vec![1, 2, 3, 9, 4, 5, 6, 8]).unwrap();
        assert_eq!(img.to_rgb8(), vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(img.as_bytes().unwrap().len(), 8);
        assert_eq!(img.clone().into_raw().len(), 8);
        assert!(img.has_alpha());
        assert_eq!(img.try_to_rgba8().unwrap().len(), 8);
    }

    #[test]
    fn padded_stride_is_honoured() {
        let img = SvgImage::new(
            1,
            2,
            PixelFormat::Rgb24,
            vec![Plane::new(4, vec![1, 2, 3, 0, 4, 5, 6, 0])],
        )
        .unwrap();
        assert_eq!(img.to_rgb8(), vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn defaults() {
        assert_eq!(ColorInfo::default(), ColorInfo::srgb());
        assert!(ColorInfo::srgb().is_specified());
        assert!(!ColorInfo::unspecified().is_specified());
        assert!(Metadata::new().is_empty());
        let i = ImageInfo::new(3, 4);
        assert_eq!((i.width, i.height, i.frames), (3, 4, 1));
        assert!(i.has_alpha && !i.has_icc && !i.has_exif && !i.has_xmp);
        assert_eq!(PixelFormat::Rgba.bytes_per_pixel(), 4);
        assert_eq!(PixelFormat::Rgb24.bytes_per_pixel(), 3);
    }
}
