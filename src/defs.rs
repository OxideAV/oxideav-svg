//! `<defs>`-resolved tables for round-2 cross-references.
//!
//! Round 1 only tracked gradients; round 2 adds a filter table
//! (and, in subsequent commits, mask / clipPath / symbol) that is
//! populated by a tree walk before the main parse pass so forward
//! references work.
//!
//! Filters (`<filter>` / `filter="url(#id)"`) are stored as captured
//! XML element subtrees. Round 2 doesn't render filters (deferred to
//! oxideav-raster round 3 / #368), but the table lets the encoder
//! re-emit the original definition for lossless round-trip and for
//! downstream consumers that *can* render them.

use std::collections::HashMap;

use crate::parser::Element;

/// Captured `<filter id="...">` element. Round 2 stores the original
/// element verbatim so the encoder can re-emit it; the rendering path
/// (Gaussian blur, color matrix, …) is wired up by oxideav-raster in
/// a later round.
#[derive(Clone, Debug)]
pub struct FilterDef {
    pub element: Element,
}

/// Aggregated tables built during the pre-walk, consumed by the main
/// element parser when it resolves `url(#id)` references.
#[derive(Clone, Debug, Default)]
pub struct DefsTables {
    pub filters: HashMap<String, FilterDef>,
}

impl DefsTables {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Strip a leading `url(` and trailing `)` plus the `#`, returning the
/// referenced id. Returns `None` for any string that doesn't match
/// `url(#...)`.
pub fn parse_url_ref(s: &str) -> Option<&str> {
    let s = s.trim();
    let inner = s.strip_prefix("url(")?.strip_suffix(')')?.trim();
    inner.strip_prefix('#')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_ref_extracts_id() {
        assert_eq!(parse_url_ref("url(#foo)"), Some("foo"));
        assert_eq!(parse_url_ref("  url(#bar)  "), Some("bar"));
        assert_eq!(parse_url_ref("url( #spc )"), Some("spc"));
        assert_eq!(parse_url_ref("none"), None);
        assert_eq!(parse_url_ref("#foo"), None);
        assert_eq!(parse_url_ref("url(foo)"), None);
    }
}
