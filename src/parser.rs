//! Hand-rolled SAX-style XML parser. The SVG-relevant XML subset is
//! small (tags and attributes and text content and comments and CDATA
//! and entities and processing instructions and DOCTYPE) so this avoids
//! pulling `xml-rs` / `quick-xml` into the dependency tree.
//!
//! Tag names are kept verbatim (including any `prefix:`) so namespace-
//! aware tooling can inspect them; matching helpers compare the local
//! part case-insensitively (SVG, like HTML, accepts mixed-case markup
//! in some real-world inputs).

use oxideav_core::{Error, Result};

/// One XML element parsed from the input.
#[derive(Clone, Debug)]
pub struct Element {
    /// Original tag name as it appeared in the source (e.g. `svg`,
    /// `xlink:href`).
    pub name: String,
    /// `(name, value)` pairs in source order. Values have already had
    /// their entity references decoded.
    pub attrs: Vec<(String, String)>,
    /// Element children — interleaved elements and text runs in source
    /// order.
    pub children: Vec<Node>,
}

/// One node in the XML tree.
#[derive(Clone, Debug)]
pub enum Node {
    Element(Element),
    Text(String),
}

/// Parse an XML document into a sequence of top-level nodes. SVG files
/// always contain a single `<svg>` root, but the parser doesn't
/// hard-code that — the caller is responsible for picking the right
/// root.
pub fn parse_xml(src: &str) -> Result<Vec<Node>> {
    let mut p = XmlParser {
        src: src.as_bytes(),
        pos: 0,
    };
    p.skip_prolog();
    let mut out: Vec<Node> = Vec::new();
    while p.pos < p.src.len() {
        match p.parse_node()? {
            Some(node) => out.push(node),
            None => break,
        }
    }
    Ok(out)
}

struct XmlParser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> XmlParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.src.len()
            && matches!(self.src[self.pos], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.pos += 1;
        }
    }

    /// Eat the XML declaration (`<?xml ...?>`), processing instructions,
    /// `<!DOCTYPE ...>`, and any leading whitespace.
    fn skip_prolog(&mut self) {
        self.skip_ws();
        while self.pos < self.src.len() {
            if self.src[self.pos..].starts_with(b"<?") {
                let end = find_seq(self.src, self.pos, b"?>")
                    .map(|e| e + 2)
                    .unwrap_or(self.src.len());
                self.pos = end;
            } else if self.src[self.pos..].starts_with(b"<!--") {
                let end = find_seq(self.src, self.pos, b"-->")
                    .map(|e| e + 3)
                    .unwrap_or(self.src.len());
                self.pos = end;
            } else if self.src[self.pos..].starts_with(b"<!DOCTYPE")
                || self.src[self.pos..].starts_with(b"<!")
            {
                let end = find_seq(self.src, self.pos, b">")
                    .map(|e| e + 1)
                    .unwrap_or(self.src.len());
                self.pos = end;
            } else {
                break;
            }
            self.skip_ws();
        }
    }

    fn parse_node(&mut self) -> Result<Option<Node>> {
        // Collect leading text up to `<`.
        let start = self.pos;
        while self.pos < self.src.len() && self.src[self.pos] != b'<' {
            self.pos += 1;
        }
        if self.pos > start {
            let raw = std::str::from_utf8(&self.src[start..self.pos])
                .map_err(|_| Error::invalid("XML: bad UTF-8 in text"))?;
            let decoded = decode_entities(raw);
            // Whitespace-only runs are kept (significant inside <text>),
            // but the SVG round-1 builder discards them since it never
            // looks at text.
            return Ok(Some(Node::Text(decoded)));
        }
        if self.pos >= self.src.len() {
            return Ok(None);
        }
        if self.src[self.pos..].starts_with(b"<!--") {
            let end = find_seq(self.src, self.pos, b"-->")
                .map(|e| e + 3)
                .unwrap_or(self.src.len());
            self.pos = end;
            return self.parse_node();
        }
        if self.src[self.pos..].starts_with(b"<![CDATA[") {
            let data_start = self.pos + b"<![CDATA[".len();
            let end = find_seq(self.src, data_start, b"]]>").unwrap_or(self.src.len());
            let raw = std::str::from_utf8(&self.src[data_start..end])
                .map_err(|_| Error::invalid("XML: bad UTF-8 in CDATA"))?;
            self.pos = end + 3;
            return Ok(Some(Node::Text(raw.to_string())));
        }
        if self.src[self.pos..].starts_with(b"<?") {
            // Stray processing instruction inside the body.
            let end = find_seq(self.src, self.pos, b"?>")
                .map(|e| e + 2)
                .unwrap_or(self.src.len());
            self.pos = end;
            return self.parse_node();
        }
        if self.src[self.pos..].starts_with(b"</") {
            // Caller will handle the close.
            return Ok(None);
        }
        let element = self.parse_element()?;
        Ok(Some(Node::Element(element)))
    }

    fn parse_element(&mut self) -> Result<Element> {
        debug_assert_eq!(self.src[self.pos], b'<');
        self.pos += 1;
        let name_start = self.pos;
        while self.pos < self.src.len()
            && !matches!(
                self.src[self.pos],
                b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/'
            )
        {
            self.pos += 1;
        }
        let name = std::str::from_utf8(&self.src[name_start..self.pos])
            .map_err(|_| Error::invalid("XML: bad UTF-8 in tag name"))?
            .to_string();
        if name.is_empty() {
            return Err(Error::invalid("XML: empty tag name"));
        }
        let mut attrs: Vec<(String, String)> = Vec::new();
        self.skip_ws();
        while self.pos < self.src.len() {
            let b = self.src[self.pos];
            if b == b'>' {
                self.pos += 1;
                let children = self.parse_children(&name)?;
                return Ok(Element {
                    name,
                    attrs,
                    children,
                });
            }
            if b == b'/' {
                self.pos += 1;
                self.skip_ws();
                if self.pos < self.src.len() && self.src[self.pos] == b'>' {
                    self.pos += 1;
                    return Ok(Element {
                        name,
                        attrs,
                        children: Vec::new(),
                    });
                }
                return Err(Error::invalid("XML: malformed self-closing tag"));
            }
            // Attribute: name = value.
            let attr_name_start = self.pos;
            while self.pos < self.src.len()
                && !matches!(
                    self.src[self.pos],
                    b' ' | b'\t' | b'\n' | b'\r' | b'=' | b'>' | b'/'
                )
            {
                self.pos += 1;
            }
            let attr_name = std::str::from_utf8(&self.src[attr_name_start..self.pos])
                .map_err(|_| Error::invalid("XML: bad UTF-8 in attr name"))?
                .to_string();
            self.skip_ws();
            if self.pos >= self.src.len() || self.src[self.pos] != b'=' {
                if !attr_name.is_empty() {
                    attrs.push((attr_name, String::new()));
                }
                self.skip_ws();
                continue;
            }
            self.pos += 1; // skip '='
            self.skip_ws();
            if self.pos >= self.src.len() {
                return Err(Error::invalid("XML: attribute missing value"));
            }
            let quote = self.src[self.pos];
            let (val_start, val_end) = if quote == b'"' || quote == b'\'' {
                self.pos += 1;
                let start = self.pos;
                while self.pos < self.src.len() && self.src[self.pos] != quote {
                    self.pos += 1;
                }
                let end = self.pos;
                if self.pos < self.src.len() {
                    self.pos += 1;
                }
                (start, end)
            } else {
                // Unquoted attribute (rare, but tolerated).
                let start = self.pos;
                while self.pos < self.src.len()
                    && !matches!(
                        self.src[self.pos],
                        b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/'
                    )
                {
                    self.pos += 1;
                }
                (start, self.pos)
            };
            let raw = std::str::from_utf8(&self.src[val_start..val_end])
                .map_err(|_| Error::invalid("XML: bad UTF-8 in attr value"))?;
            attrs.push((attr_name, decode_entities(raw)));
            self.skip_ws();
        }
        Err(Error::invalid("XML: truncated element"))
    }

    fn parse_children(&mut self, name: &str) -> Result<Vec<Node>> {
        let mut children: Vec<Node> = Vec::new();
        while self.pos < self.src.len() {
            if self.src[self.pos..].starts_with(b"</") {
                let tag_end = find_seq(self.src, self.pos, b">")
                    .ok_or_else(|| Error::invalid("XML: truncated close tag"))?;
                let close_name = std::str::from_utf8(&self.src[self.pos + 2..tag_end])
                    .map_err(|_| Error::invalid("XML: bad UTF-8 in close tag"))?
                    .trim();
                self.pos = tag_end + 1;
                if close_name.eq_ignore_ascii_case(name) {
                    return Ok(children);
                }
                // Mismatched close — tolerate by stopping here. Strict
                // SVG validators would reject; we'd rather load the
                // document than fail the round-trip on a stray tag.
                return Ok(children);
            }
            match self.parse_node()? {
                Some(node) => children.push(node),
                None => break,
            }
        }
        Ok(children)
    }
}

fn find_seq(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

/// Strip the optional UTF-8 BOM and decode the bytes as UTF-8 (lossy
/// for invalid sequences — SVG inputs are universally UTF-8 in
/// practice).
pub fn decode_utf8_lossy_stripping_bom(bytes: &[u8]) -> String {
    let trimmed = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    };
    String::from_utf8_lossy(trimmed).into_owned()
}

/// `true` when `bytes` starts with the RFC 1952 gzip magic (`1f 8b`),
/// indicating an `.svgz` (gzip-compressed SVG) payload that needs to
/// be inflated before XML parsing.
pub fn is_gzip(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b
}

/// Inflate a gzip-compressed SVG payload (`.svgz`) into the raw XML
/// bytes the rest of the parser expects. Returns an error when the
/// input is not a valid gzip stream.
pub fn inflate_gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(bytes);
    let mut out = Vec::with_capacity(bytes.len() * 4);
    decoder
        .read_to_end(&mut out)
        .map_err(|e| Error::invalid(format!("SVG: gzip inflate failed: {e}")))?;
    Ok(out)
}

/// Deflate (gzip) the SVG bytes for `.svgz` output. Uses the default
/// compression level which mirrors what the gzip(1) utility produces.
pub fn deflate_gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(
        Vec::with_capacity(bytes.len() / 2),
        flate2::Compression::default(),
    );
    encoder
        .write_all(bytes)
        .map_err(|e| Error::invalid(format!("SVG: gzip deflate failed: {e}")))?;
    encoder
        .finish()
        .map_err(|e| Error::invalid(format!("SVG: gzip finish failed: {e}")))
}

/// Decode the XML predefined entities + numeric character references.
/// Anything we don't recognise is left verbatim — keeps the encoder's
/// round-trip honest when the input contains an unusual entity.
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '&' {
            out.push(c);
            continue;
        }
        // Buffer up to the next ';' or whitespace.
        let mut entity = String::new();
        let mut found_semicolon = false;
        for nc in chars.by_ref() {
            if nc == ';' {
                found_semicolon = true;
                break;
            }
            if entity.len() > 16 {
                // Runaway entity — give up.
                break;
            }
            entity.push(nc);
        }
        if !found_semicolon {
            out.push('&');
            out.push_str(&entity);
            continue;
        }
        match entity.as_str() {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            other => {
                if let Some(rest) = other.strip_prefix('#') {
                    let cp = if let Some(hex) = rest.strip_prefix(['x', 'X']) {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        rest.parse::<u32>().ok()
                    };
                    if let Some(cp) = cp.and_then(char::from_u32) {
                        out.push(cp);
                        continue;
                    }
                }
                out.push('&');
                out.push_str(other);
                out.push(';');
            }
        }
    }
    out
}

/// Find the first child element (any depth = 0) named `name`
/// (case-insensitive on the local part).
pub fn find_element<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Element> {
    for n in nodes {
        if let Node::Element(e) = n {
            if tag_local(&e.name).eq_ignore_ascii_case(name) {
                return Some(e);
            }
        }
    }
    None
}

/// Return the local part of a `prefix:local` qualified name, lower-cased.
pub fn tag_local(name: &str) -> String {
    match name.rsplit_once(':') {
        Some((_, local)) => local.to_ascii_lowercase(),
        None => name.to_ascii_lowercase(),
    }
}

/// Look up an attribute by local name (case-insensitive).
pub fn attr<'a>(el: &'a Element, name: &str) -> Option<&'a str> {
    el.attrs
        .iter()
        .find(|(k, _)| tag_local(k).eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Escape a string for use as XML text content.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a string for use as an XML attribute value (quoted with `"`).
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_self_closing_with_attrs() {
        let nodes = parse_xml(r#"<rect x="1" y="2" width="3" height="4"/>"#).unwrap();
        assert_eq!(nodes.len(), 1);
        if let Node::Element(e) = &nodes[0] {
            assert_eq!(e.name, "rect");
            assert_eq!(e.attrs.len(), 4);
            assert_eq!(e.attrs[0], ("x".into(), "1".into()));
            assert!(e.children.is_empty());
        } else {
            panic!("expected element");
        }
    }

    #[test]
    fn parses_nested_elements_and_text() {
        let src = r#"<g><rect x="0"/></g>"#;
        let nodes = parse_xml(src).unwrap();
        let g = match &nodes[0] {
            Node::Element(e) => e,
            _ => panic!(),
        };
        assert_eq!(g.name, "g");
        let rect = match &g.children[0] {
            Node::Element(e) => e,
            _ => panic!(),
        };
        assert_eq!(rect.name, "rect");
    }

    #[test]
    fn skips_xml_declaration_and_comment() {
        let src = "<?xml version=\"1.0\"?>\n<!-- hello -->\n<svg/>";
        let nodes = parse_xml(src).unwrap();
        assert_eq!(nodes.len(), 1);
        if let Node::Element(e) = &nodes[0] {
            assert_eq!(e.name, "svg");
        }
    }

    #[test]
    fn decodes_named_and_numeric_entities() {
        assert_eq!(decode_entities("&amp;&lt;&gt;&quot;&apos;"), "&<>\"'");
        assert_eq!(decode_entities("&#65;&#x42;"), "AB");
        // Unknown entity passes through verbatim.
        assert_eq!(decode_entities("&unknown;rest"), "&unknown;rest");
    }

    #[test]
    fn escape_helpers_are_minimal_and_round_trip() {
        let attr = escape_attr("a&b<c>d\"e");
        assert_eq!(attr, "a&amp;b&lt;c&gt;d&quot;e");
        let text = escape_text("a&b<c>d");
        assert_eq!(text, "a&amp;b&lt;c&gt;d");
    }

    #[test]
    fn tolerates_namespaced_attrs() {
        let nodes = parse_xml(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"/>"#,
        )
        .unwrap();
        let svg = match &nodes[0] {
            Node::Element(e) => e,
            _ => panic!(),
        };
        assert_eq!(svg.attrs.len(), 2);
    }
}
