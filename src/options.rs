//! [`DecodeOptions`] / [`EncodeOptions`] — the image-crate contract's
//! option records, specialised to a vector format.
//!
//! For SVG the hostile-input surface is XML, so the limits that matter
//! most are the ones no raster crate has: element count and nesting
//! depth. The contract's geometry limits (`max_width` / `max_height` /
//! `max_pixels`) describe the raster canvas a rasteriser would allocate
//! and are consulted by the `decode*` functions only; parsing allocates
//! no pixels.

use crate::error::{Result, SvgError};
use crate::parser::{XmlLimits, DEFAULT_MAX_ELEMENTS, MAX_SVGZ_INFLATED, MAX_XML_DEPTH};

/// Limits and strictness for [`crate::parse_with`] / [`crate::decode_with`].
///
/// `None` means unlimited. The defaults are finite: 128 MiB of input /
/// inflated text, 1 M elements, depth 128, 256 M pixels of canvas
/// (1 GiB of RGBA), no per-axis bound, `strict = false`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// Largest canvas width (CSS px, as [`crate::info`] reports it) a
    /// `decode*` call accepts; `None` = unlimited.
    pub max_width: Option<u32>,
    /// Largest canvas height a `decode*` call accepts.
    pub max_height: Option<u32>,
    /// Largest `width × height` a `decode*` call accepts.
    pub max_pixels: Option<u64>,
    /// Largest input the parser accepts — the byte length handed in,
    /// and for an `.svgz` also the inflated size (the inflater stops
    /// at this cap, so a decompression bomb never materialises).
    pub max_bytes: Option<u64>,
    /// Reject what the lenient parser otherwise tolerates: mismatched
    /// or missing close tags, a document element other than `<svg>`,
    /// more than one top-level element.
    pub strict: bool,
    /// Most XML elements accepted in one document.
    pub max_elements: Option<u64>,
    /// Deepest XML nesting accepted (bounds the recursive parser's
    /// stack use).
    pub max_depth: Option<usize>,
}

impl DecodeOptions {
    /// Default `max_bytes`: 128 MiB, the long-standing `.svgz`
    /// inflation ceiling ([`MAX_SVGZ_INFLATED`]).
    pub const DEFAULT_MAX_BYTES: u64 = MAX_SVGZ_INFLATED;
    /// Default `max_pixels`: 256 Mpx — 1 GiB of RGBA8 canvas.
    pub const DEFAULT_MAX_PIXELS: u64 = 1 << 28;
    /// Default `max_elements` ([`DEFAULT_MAX_ELEMENTS`]).
    pub const DEFAULT_MAX_ELEMENTS: u64 = DEFAULT_MAX_ELEMENTS;
    /// Default `max_depth` ([`MAX_XML_DEPTH`]).
    pub const DEFAULT_MAX_DEPTH: usize = MAX_XML_DEPTH;

    /// The defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `max_width`.
    pub fn with_max_width(mut self, max_width: impl Into<Option<u32>>) -> Self {
        self.max_width = max_width.into();
        self
    }

    /// Set `max_height`.
    pub fn with_max_height(mut self, max_height: impl Into<Option<u32>>) -> Self {
        self.max_height = max_height.into();
        self
    }

    /// Set `max_pixels`.
    pub fn with_max_pixels(mut self, max_pixels: impl Into<Option<u64>>) -> Self {
        self.max_pixels = max_pixels.into();
        self
    }

    /// Set `max_bytes`.
    pub fn with_max_bytes(mut self, max_bytes: impl Into<Option<u64>>) -> Self {
        self.max_bytes = max_bytes.into();
        self
    }

    /// Set `strict`.
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Set `max_elements`.
    pub fn with_max_elements(mut self, max_elements: impl Into<Option<u64>>) -> Self {
        self.max_elements = max_elements.into();
        self
    }

    /// Set `max_depth`.
    pub fn with_max_depth(mut self, max_depth: impl Into<Option<usize>>) -> Self {
        self.max_depth = max_depth.into();
        self
    }

    /// Lift every limit (`strict` is untouched). The parser's stack
    /// guard still applies a depth bound of `usize::MAX`, i.e. none —
    /// use only on trusted input.
    pub fn unlimited(mut self) -> Self {
        self.max_width = None;
        self.max_height = None;
        self.max_pixels = None;
        self.max_bytes = None;
        self.max_elements = None;
        self.max_depth = None;
        self
    }

    /// The XML parser's view of these options.
    pub(crate) fn xml_limits(&self) -> XmlLimits {
        XmlLimits::new()
            .with_max_depth(self.max_depth.unwrap_or(usize::MAX))
            .with_max_elements(self.max_elements)
            .with_strict(self.strict)
    }

    /// Inflation cap for an `.svgz`: `max_bytes`, else the crate ceiling.
    pub(crate) fn inflate_cap(&self) -> u64 {
        self.max_bytes.unwrap_or(MAX_SVGZ_INFLATED)
    }

    /// Canvas-geometry check used by the `decode*` functions.
    pub(crate) fn check_canvas(&self, width: u32, height: u32) -> Result<()> {
        if let Some(m) = self.max_width {
            if width > m {
                return Err(SvgError::limit(format!(
                    "SVG: canvas width {width} exceeds max_width {m}"
                )));
            }
        }
        if let Some(m) = self.max_height {
            if height > m {
                return Err(SvgError::limit(format!(
                    "SVG: canvas height {height} exceeds max_height {m}"
                )));
            }
        }
        if let Some(m) = self.max_pixels {
            let pixels = u64::from(width) * u64::from(height);
            if pixels > m {
                return Err(SvgError::limit(format!(
                    "SVG: canvas of {pixels} pixels exceeds max_pixels {m}"
                )));
            }
        }
        Ok(())
    }
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            max_width: None,
            max_height: None,
            max_pixels: Some(Self::DEFAULT_MAX_PIXELS),
            max_bytes: Some(Self::DEFAULT_MAX_BYTES),
            strict: false,
            max_elements: Some(Self::DEFAULT_MAX_ELEMENTS),
            max_depth: Some(Self::DEFAULT_MAX_DEPTH),
        }
    }
}

/// Options for the SVG writer ([`crate::write_with`]) and the
/// contract's `encode*` functions.
///
/// SVG offers exactly one encoding choice: whether the document is
/// gzip-compressed (`.svgz`). Pretty-printing, precision and the like
/// are fixed by the writer's fixed-point contract (`write(parse(x))`
/// is byte-stable), so there is nothing else to configure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncodeOptions {
    /// Emit a gzip-compressed `.svgz` body instead of plain XML.
    pub compress: bool,
}

impl EncodeOptions {
    /// Plain XML output.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `compress`.
    pub fn with_compress(mut self, compress: bool) -> Self {
        self.compress = compress;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_finite() {
        let o = DecodeOptions::default();
        assert_eq!(o.max_bytes, Some(128 * 1024 * 1024));
        assert_eq!(o.max_elements, Some(1 << 20));
        assert_eq!(o.max_depth, Some(128));
        assert_eq!(o.max_pixels, Some(1 << 28));
        assert!(!o.strict);
        let u = o.unlimited();
        assert_eq!(u.xml_limits().max_depth, usize::MAX);
        assert_eq!(u.xml_limits().max_elements, None);
        assert_eq!(u.inflate_cap(), MAX_SVGZ_INFLATED);
    }

    #[test]
    fn canvas_limits_fire() {
        let o = DecodeOptions::default()
            .with_max_width(10u32)
            .with_max_height(10u32)
            .with_max_pixels(50u64);
        assert!(o.check_canvas(5, 5).is_ok());
        assert!(matches!(
            o.check_canvas(11, 1),
            Err(SvgError::LimitExceeded(_))
        ));
        assert!(matches!(
            o.check_canvas(1, 11),
            Err(SvgError::LimitExceeded(_))
        ));
        assert!(matches!(
            o.check_canvas(8, 8),
            Err(SvgError::LimitExceeded(_))
        ));
    }

    #[test]
    fn encode_options_builder() {
        assert!(!EncodeOptions::default().compress);
        assert!(EncodeOptions::new().with_compress(true).compress);
    }
}
