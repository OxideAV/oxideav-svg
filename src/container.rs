//! SVG container — one whole `.svg` file becomes one `Packet` on
//! stream `0`. Same one-shot shape as [`oxideav-bmp`] / static
//! [`oxideav-png`]. The probe accepts either an XML-prologue with the
//! `svg` root tag or an `.svg` / `.svgz` file extension.
//!
//! Round 3: `.svgz` (gzip-compressed SVG, RFC 1952 magic `1f 8b`) is
//! transparently inflated by the demuxer; the inflated XML is handed
//! to the codec just like a plain `.svg`.

use std::io::{Read, SeekFrom, Write};

use oxideav_core::{
    CodecId, CodecParameters, CodecResolver, ContainerRegistry, Demuxer, Error, MediaType, Muxer,
    Packet, ProbeData, ProbeScore, ReadSeek, Result, StreamInfo, TimeBase, WriteSeek,
    MAX_PROBE_SCORE, PROBE_SCORE_EXTENSION,
};

use crate::decoder::CODEC_ID_STR;
use crate::parser::{deflate_gzip, inflate_gzip, is_gzip};

pub fn register(reg: &mut ContainerRegistry) {
    reg.register_demuxer("svg", open_demuxer);
    reg.register_muxer("svg", open_muxer);
    // `svgz` shares the demuxer (which sniffs the gzip magic) and a
    // sister muxer that gzip-compresses the produced bytes.
    reg.register_demuxer("svgz", open_demuxer);
    reg.register_muxer("svgz", open_muxer_svgz);
    reg.register_extension("svg", "svg");
    reg.register_extension("svgz", "svgz");
    reg.register_probe("svg", probe);
    reg.register_probe("svgz", probe_svgz);
}

fn probe(data: &ProbeData) -> ProbeScore {
    // Look at the first ~4 KiB for `<svg` (case-insensitive). We allow
    // an XML prologue / DOCTYPE / comments before the root. Hits at
    // top yield max score; an extension-only hit falls back to
    // PROBE_SCORE_EXTENSION.
    let head_len = data.buf.len().min(4096);
    let head = &data.buf[..head_len];
    if contains_token_ci(head, b"<svg") {
        MAX_PROBE_SCORE
    } else if matches!(data.ext, Some("svg") | Some("svgz")) {
        PROBE_SCORE_EXTENSION
    } else {
        0
    }
}

/// Round-3 probe for `.svgz` (gzip-compressed SVG). The gzip magic
/// (`1f 8b`) is unambiguous; we can't peek inside the compressed
/// stream from a `ProbeData` slice (inflating the whole thing per
/// probe would be wasteful), so the magic + extension is enough.
fn probe_svgz(data: &ProbeData) -> ProbeScore {
    if data.buf.len() >= 2 && data.buf[0] == 0x1f && data.buf[1] == 0x8b {
        MAX_PROBE_SCORE
    } else if matches!(data.ext, Some("svgz")) {
        PROBE_SCORE_EXTENSION
    } else {
        0
    }
}

fn contains_token_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

pub fn open_demuxer(
    mut input: Box<dyn ReadSeek>,
    _codecs: &dyn CodecResolver,
) -> Result<Box<dyn Demuxer>> {
    input.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    input.read_to_end(&mut buf)?;
    // Round 3: transparently inflate `.svgz` (gzip RFC 1952). The
    // codec only sees raw XML bytes — `parse_svg` also does this sniff
    // when called directly, so callers using either entry point get
    // gzip handling for free.
    if is_gzip(&buf) {
        buf = inflate_gzip(&buf)?;
    }
    let head_len = buf.len().min(4096);
    if !contains_token_ci(&buf[..head_len], b"<svg") {
        return Err(Error::invalid("SVG: '<svg' tag not found in first 4 KiB"));
    }

    let params = CodecParameters::video(CodecId::new(CODEC_ID_STR));
    let stream = StreamInfo {
        index: 0,
        params,
        time_base: TimeBase::new(1, 1),
        start_time: Some(0),
        duration: None,
    };
    Ok(Box::new(SvgDemuxer {
        streams: vec![stream],
        data: Some(buf),
    }))
}

struct SvgDemuxer {
    streams: Vec<StreamInfo>,
    data: Option<Vec<u8>>,
}

impl Demuxer for SvgDemuxer {
    fn format_name(&self) -> &str {
        "svg"
    }
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }
    fn next_packet(&mut self) -> Result<Packet> {
        match self.data.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.pts = Some(0);
                pkt.dts = Some(0);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
            None => Err(Error::Eof),
        }
    }
}

pub fn open_muxer(output: Box<dyn WriteSeek>, streams: &[StreamInfo]) -> Result<Box<dyn Muxer>> {
    if streams.len() != 1 {
        return Err(Error::invalid("SVG muxer: expected exactly one stream"));
    }
    if streams[0].params.media_type != MediaType::Video {
        return Err(Error::invalid("SVG muxer: stream must be video"));
    }
    Ok(Box::new(SvgMuxer {
        output,
        gzip: false,
    }))
}

/// Round-3: `.svgz` muxer — same packet shape as the plain muxer but
/// the bytes are gzip-compressed before hitting disk.
pub fn open_muxer_svgz(
    output: Box<dyn WriteSeek>,
    streams: &[StreamInfo],
) -> Result<Box<dyn Muxer>> {
    if streams.len() != 1 {
        return Err(Error::invalid("SVGZ muxer: expected exactly one stream"));
    }
    if streams[0].params.media_type != MediaType::Video {
        return Err(Error::invalid("SVGZ muxer: stream must be video"));
    }
    Ok(Box::new(SvgMuxer { output, gzip: true }))
}

struct SvgMuxer {
    output: Box<dyn WriteSeek>,
    /// When `true`, packet bytes are gzip-compressed before being
    /// written (`.svgz` output).
    gzip: bool,
}

impl Muxer for SvgMuxer {
    fn format_name(&self) -> &str {
        if self.gzip {
            "svgz"
        } else {
            "svg"
        }
    }
    fn write_header(&mut self) -> Result<()> {
        Ok(())
    }
    fn write_packet(&mut self, packet: &Packet) -> Result<()> {
        if self.gzip {
            let compressed = deflate_gzip(&packet.data)?;
            self.output.write_all(&compressed)?;
        } else {
            self.output.write_all(&packet.data)?;
        }
        Ok(())
    }
    fn write_trailer(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_data<'a>(buf: &'a [u8], ext: Option<&'a str>) -> ProbeData<'a> {
        ProbeData { buf, ext }
    }

    #[test]
    fn probe_recognises_svg_root() {
        let s = b"<?xml version=\"1.0\"?><svg/>";
        assert_eq!(probe(&probe_data(s, None)), MAX_PROBE_SCORE);
    }

    #[test]
    fn probe_falls_back_to_extension() {
        let s = b"random binary garbage";
        assert_eq!(probe(&probe_data(s, Some("svg"))), PROBE_SCORE_EXTENSION);
    }

    #[test]
    fn probe_rejects_unrelated() {
        assert_eq!(probe(&probe_data(b"<html></html>", None)), 0);
    }
}
