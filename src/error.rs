//! The crate's error type — the image-crate contract shape
//! (`InvalidData` / `Unsupported` / `LimitExceeded` / `Io`), usable
//! without `oxideav-core`. With the `registry` feature the
//! [`crate::registry`] adapter maps it onto `oxideav_core::Error`.

use std::fmt;

/// Error type for every fallible function in this crate.
#[derive(Debug)]
#[non_exhaustive]
pub enum SvgError {
    /// The input is not a well-formed SVG document (XML syntax,
    /// missing `<svg>` root, malformed attribute grammar, bad gzip
    /// framing on an `.svgz`).
    InvalidData(String),
    /// The request cannot be served by this crate — notably every
    /// raster path (`decode*` / `encode*`): SVG is a vector format and
    /// rasterising it is `oxideav-raster`'s job through the framework.
    Unsupported(String),
    /// A [`crate::DecodeOptions`] limit (input bytes, inflated size,
    /// element count, nesting depth, canvas geometry) would be
    /// exceeded; nothing past the limit was allocated.
    LimitExceeded(String),
    /// A read / write on a caller-supplied stream failed
    /// ([`crate::decode_from`] / [`crate::encode_to`]).
    Io(std::io::Error),
}

/// Alias every image crate exposes: `oxideav_svg::Error`.
pub type Error = SvgError;

/// `Result` specialised to [`SvgError`].
pub type Result<T> = std::result::Result<T, SvgError>;

impl SvgError {
    /// Construct an [`SvgError::InvalidData`].
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidData(msg.into())
    }

    /// Construct an [`SvgError::Unsupported`].
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }

    /// Construct an [`SvgError::LimitExceeded`].
    pub fn limit(msg: impl Into<String>) -> Self {
        Self::LimitExceeded(msg.into())
    }
}

impl From<std::io::Error> for SvgError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl fmt::Display for SvgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(s) => write!(f, "invalid data: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
            Self::LimitExceeded(s) => write!(f, "limit exceeded: {s}"),
            Self::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for SvgError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_prefixes_variant() {
        assert_eq!(
            SvgError::invalid("x").to_string(),
            "invalid data: x".to_string()
        );
        assert_eq!(SvgError::unsupported("y").to_string(), "unsupported: y");
        assert_eq!(SvgError::limit("z").to_string(), "limit exceeded: z");
        let io: SvgError = std::io::Error::other("boom").into();
        assert!(matches!(io, SvgError::Io(_)));
        assert!(std::error::Error::source(&io).is_some());
    }
}
