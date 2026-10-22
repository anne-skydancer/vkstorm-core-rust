//! What a response body is: an audio stream we decode, a playlist to
//! resolve, or something to refuse with a reason (video, a web page, a
//! container or codec we do not play).
//!
//! The body decides first, the content type second: servers mislabel
//! streams often (`text/html`, `application/octet-stream`, `audio/x-mpegurl`
//! for the stream itself), and a parcel's music URL may lead to a video.
//! Order:
//! 1. Strong signatures at the start of the body: `OggS`, `fLaC`, ID3v2
//!    (skipped, its tags read, then the rest examined), `#EXTM3U`,
//!    `[playlist]`, HTML/XML, and the video and unsupported containers:
//!    MP4/MOV (`ftyp`; a `vide` handler in the `moov` means video), FLV
//!    (video flag), MPEG-TS (video stream types in the PMT), MPEG-PS,
//!    Matroska/WebM (a `V_` codec), AVI, ASF, WAV.
//! 2. MPEG audio or ADTS sync, confirmed by three consecutive frames (the
//!    body may start mid-frame: a server's burst starts anywhere), which a
//!    web page or a video container does not pass.
//! 3. A `video/*` or `text/html` content type refuses the body.
//! 4. A playlist content type, or a `.m3u`/`.pls` URL with a text body,
//!    is a plain playlist.
//!
//! What the Ogg demuxer finds (a Theora BOS page) is refused there, before
//! any audio. VLC played video URLs' audio tracks in the native music path
//! (`:no-video`); the fork's PCM path rejected them
//! (`pcm_video_not_qualified`), which is the rule kept here.

#![forbid(unsafe_code)]

use super::codec::{PcmSink, StreamError};
use super::demux::skip_id3;
use super::input::Input;
use super::mpeg::{Chain, chain_frames};
use super::pipe::Head;
use super::playlist::content_type_kind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Mpeg,
    Adts,
    Flac,
    Ogg,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Detected {
    Audio(Format),
    Playlist { pls: bool },
    Refused(String),
}

/// How far into the body the MPEG/ADTS sync search looks.
const SCAN_LIMIT: usize = 64 * 1024;

fn ct_main(ct: &str) -> String {
    ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

pub(crate) fn detect(input: &mut Input, head: &Head, url: &str, sink: &mut dyn PcmSink) -> Result<Detected, StreamError> {
    let ct = ct_main(&head.content_type);
    // Leading ID3v2 tags (MP3, sometimes AAC or FLAC behind them).
    while input.fill(10)? && input.data().starts_with(b"ID3") {
        if !skip_id3(input, sink)? {
            break;
        }
    }
    input.fill(16)?;
    if let Some(d) = strong(input, &ct)? {
        return Ok(d);
    }
    // MPEG audio or ADTS: three frames in a row somewhere near the start.
    let mut off = 0usize;
    loop {
        let eof = input.at_eof();
        let d = input.data();
        let mut need_more = false;
        while off + 4 <= d.len() && off < SCAN_LIMIT {
            if d[off] == 0xFF && d[off + 1] & 0xE0 == 0xE0 {
                let adts = d[off + 1] & 0x06 == 0;
                match chain_frames(&d[off..], 3, adts, eof) {
                    Chain::Yes => {
                        input.consume(off);
                        return Ok(Detected::Audio(if adts { Format::Adts } else { Format::Mpeg }));
                    }
                    Chain::NeedMore => {
                        need_more = true;
                        break;
                    }
                    Chain::No => {}
                }
            }
            off += 1;
        }
        if eof || (off >= SCAN_LIMIT && !need_more) {
            break;
        }
        input.more()?;
    }
    if ct.starts_with("video/") {
        return Ok(Detected::Refused(format!("the URL serves video ({ct}), not audio")));
    }
    if ct == "text/html" || ct == "application/xhtml+xml" {
        return Ok(Detected::Refused("the URL is a web page, not an audio stream".into()));
    }
    if let Some(pls) = content_type_kind(&ct) {
        return Ok(Detected::Playlist { pls });
    }
    let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    let texty = input.data().iter().take(512).all(|&b| b >= 0x09 && b != 0x7F);
    if texty && (path.ends_with(".m3u") || path.ends_with(".m3u8")) {
        return Ok(Detected::Playlist { pls: false });
    }
    if texty && path.ends_with(".pls") {
        return Ok(Detected::Playlist { pls: true });
    }
    if input.len() == 0 {
        return Ok(Detected::Refused("the stream is empty".into()));
    }
    let shown = if ct.is_empty() { "no content type".to_owned() } else { ct };
    Ok(Detected::Refused(format!("unrecognised stream format ({shown})")))
}

fn strong(input: &mut Input, ct: &str) -> Result<Option<Detected>, StreamError> {
    let d = input.data();
    if d.starts_with(b"OggS") {
        return Ok(Some(Detected::Audio(Format::Ogg)));
    }
    if d.starts_with(b"fLaC") {
        return Ok(Some(Detected::Audio(Format::Flac)));
    }
    // Text: skip a BOM and white space.
    let text = d.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(d);
    let t = &text[text.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(text.len())..];
    if starts_ci(t, b"#EXTM3U") {
        return Ok(Some(Detected::Playlist { pls: false }));
    }
    if starts_ci(t, b"[playlist]") {
        return Ok(Some(Detected::Playlist { pls: true }));
    }
    if t.starts_with(b"<") {
        input.fill(4096)?;
        let d = input.data();
        let lower: Vec<u8> = d.iter().take(4096).map(u8::to_ascii_lowercase).collect();
        let has = |n: &[u8]| lower.windows(n.len()).any(|w| w == n);
        if has(b"<html") || has(b"<!doctype html") || has(b"<head") || has(b"<body") {
            return Ok(Some(Detected::Refused("the URL is a web page, not an audio stream".into())));
        }
        if has(b"<playlist") {
            return Ok(Some(Detected::Refused("XSPF playlists are not supported".into())));
        }
        if has(b"<asx") {
            return Ok(Some(Detected::Refused("ASX playlists are not supported".into())));
        }
        return Ok(Some(Detected::Refused("the URL serves an XML document, not audio".into())));
    }
    if d.len() >= 8 && &d[4..8] == b"ftyp" {
        return mp4(input, ct).map(Some);
    }
    if d.starts_with(b"FLV\x01") && d.len() > 4 {
        return Ok(Some(Detected::Refused(if d[4] & 1 != 0 {
            "the stream is video (FLV with a video track), not audio".into()
        } else {
            "FLV streams are not supported".into()
        })));
    }
    if d.starts_with(&[0x00, 0x00, 0x01, 0xBA]) || d.starts_with(&[0x00, 0x00, 0x01, 0xB3]) {
        return Ok(Some(Detected::Refused("the stream is video (MPEG program stream), not audio".into())));
    }
    if d.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        input.fill(64 * 1024)?;
        let video = input.data().windows(4).any(|w| w[0] == 0x86 && w[2] == b'V' && w[3] == b'_');
        return Ok(Some(Detected::Refused(if video {
            "the stream is video (Matroska/WebM with a video track), not audio".into()
        } else {
            "Matroska/WebM streams are not supported".into()
        })));
    }
    if d.len() >= 12 && &d[..4] == b"RIFF" {
        return Ok(Some(Detected::Refused(match &d[8..12] {
            b"AVI " => "the stream is video (AVI), not audio".into(),
            b"WAVE" => "WAV streams are not supported".into(),
            _ => "RIFF streams are not supported".into(),
        })));
    }
    if d.starts_with(&[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11]) {
        return Ok(Some(Detected::Refused("ASF/WMA streams are not supported".into())));
    }
    if d.first() == Some(&0x47) {
        input.fill(188 * 64)?;
        let d = input.data();
        if d.len() > 188 * 2 && d[188] == 0x47 && d[376] == 0x47 {
            return Ok(Some(Detected::Refused(if ts_has_video(d) || ct.starts_with("video/") {
                "the stream is video (MPEG-TS with a video stream), not audio".into()
            } else {
                "MPEG-TS streams are not supported".into()
            })));
        }
    }
    Ok(None)
}

fn starts_ci(b: &[u8], p: &[u8]) -> bool {
    b.len() >= p.len() && b[..p.len()].eq_ignore_ascii_case(p)
}

/// Walks MP4 boxes looking for a video handler in `moov`.
fn mp4(input: &mut Input, ct: &str) -> Result<Detected, StreamError> {
    input.fill(1 << 20)?;
    let d = input.data();
    fn boxes(b: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
        let mut p = 0usize;
        std::iter::from_fn(move || {
            if p + 8 > b.len() {
                return None;
            }
            let size = u32::from_be_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]) as usize;
            let kind = &b[p + 4..p + 8];
            let (hdr, size) = match size {
                0 => (8, b.len() - p),
                1 if p + 16 <= b.len() => {
                    let s = u64::from_be_bytes(b[p + 8..p + 16].try_into().ok()?);
                    (16, usize::try_from(s).ok()?)
                }
                s => (8, s),
            };
            if size < hdr {
                return None;
            }
            let end = (p + size).min(b.len());
            let body = &b[(p + hdr).min(end)..end];
            p += size;
            Some((kind, body))
        })
    }
    let mut saw_moov = false;
    let mut video = false;
    for (k, body) in boxes(d) {
        if k == b"moov" {
            saw_moov = true;
            for (k, trak) in boxes(body) {
                if k != b"trak" {
                    continue;
                }
                for (k, mdia) in boxes(trak) {
                    if k != b"mdia" {
                        continue;
                    }
                    for (k, hdlr) in boxes(mdia) {
                        if k == b"hdlr" && hdlr.len() >= 12 && &hdlr[8..12] == b"vide" {
                            video = true;
                        }
                    }
                }
            }
        }
    }
    Ok(Detected::Refused(if video || (!saw_moov && ct.starts_with("video/")) {
        "the stream is video (MP4 with a video track), not audio".into()
    } else {
        "MP4/M4A streams are not supported".into()
    }))
}

/// Whether the PMTs in the first packets declare a video stream.
fn ts_has_video(d: &[u8]) -> bool {
    let mut pmt_pids = Vec::new();
    for pass in 0..2 {
        for pkt in d.as_chunks::<188>().0 {
            if pkt[0] != 0x47 {
                return false;
            }
            let pusi = pkt[1] & 0x40 != 0;
            let pid = (u16::from(pkt[1] & 0x1F) << 8) | u16::from(pkt[2]);
            let afc = (pkt[3] >> 4) & 3;
            let mut p = 4;
            if afc == 2 || afc == 0 {
                continue;
            }
            if afc == 3 {
                p += 1 + usize::from(pkt[4]);
            }
            if !pusi || p >= 188 {
                continue;
            }
            p += 1 + usize::from(pkt[p]); // pointer field
            if p + 8 > 188 {
                continue;
            }
            let sec = &pkt[p..];
            let len = ((usize::from(sec[1] & 0x0F) << 8) | usize::from(sec[2])).min(sec.len().saturating_sub(3));
            if pass == 0 && pid == 0 && sec[0] == 0 {
                let mut q = 8;
                while q + 4 <= 3 + len.saturating_sub(4) {
                    let prog = u16::from_be_bytes([sec[q], sec[q + 1]]);
                    let ppid = (u16::from(sec[q + 2] & 0x1F) << 8) | u16::from(sec[q + 3]);
                    if prog != 0 {
                        pmt_pids.push(ppid);
                    }
                    q += 4;
                }
            } else if pass == 1 && pmt_pids.contains(&pid) && sec[0] == 2 && sec.len() > 12 {
                let info = (usize::from(sec[10] & 0x0F) << 8) | usize::from(sec[11]);
                let mut q = 12 + info;
                while q + 5 <= 3 + len.saturating_sub(4) && q + 5 <= sec.len() {
                    let st = sec[q];
                    if matches!(st, 0x01 | 0x02 | 0x10 | 0x1B | 0x24 | 0x33 | 0x42 | 0xEA) {
                        return true;
                    }
                    let es = (usize::from(sec[q + 3] & 0x0F) << 8) | usize::from(sec[q + 4]);
                    q += 5 + es;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::ChannelLayout;
    use crate::stream::codec::InBand;

    struct Null;
    impl PcmSink for Null {
        fn pcm(&mut self, _: &[&[f32]], _: usize, _: u32, _: ChannelLayout) -> Result<(), StreamError> {
            Ok(())
        }
        fn metadata(&mut self, _: InBand) {}
        fn codec(&mut self, _: &'static str) {}
    }

    fn sniff(body: &[u8], ct: &str, url: &str) -> Detected {
        let head = Head { status: 200, content_type: ct.into(), ..Default::default() };
        detect(&mut Input::from_memory(body), &head, url, &mut Null).expect("detect")
    }

    fn refused(d: Detected, what: &str) {
        match d {
            Detected::Refused(r) => assert!(r.contains(what), "{r}"),
            d => panic!("{d:?}"),
        }
    }

    #[test]
    fn containers_and_text() {
        refused(sniff(b"FLV\x01\x05\0\0\0\x09", "", ""), "FLV with a video track");
        refused(sniff(b"FLV\x01\x04\0\0\0\x09", "", ""), "FLV streams are not supported");
        refused(sniff(&[0, 0, 1, 0xBA, 0x44, 0, 4, 0, 4, 1, 1, 0x89, 0xC3, 0xF8], "", ""), "MPEG program stream");
        let mut mkv = vec![0x1A, 0x45, 0xDF, 0xA3, 0x9F];
        mkv.extend_from_slice(b"\x42\x86\x81\x01....\x86\x8fV_MPEG4/ISO/AVC");
        refused(sniff(&mkv, "", ""), "Matroska/WebM with a video track");
        refused(sniff(b"RIFF\x24\0\0\0AVI LIST", "", ""), "AVI");
        refused(sniff(b"RIFF\x24\0\0\0WAVEfmt ", "", ""), "WAV");
        refused(sniff(&[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9], "", ""), "ASF");
        refused(sniff(b"  <!DOCTYPE html><html></html>", "audio/mpeg", ""), "web page");
        refused(sniff(b"<?xml version=\"1.0\"?><playlist version=\"1\">", "", ""), "XSPF");
        refused(sniff(b"\0\x01\x02 binary junk", "video/mp4", ""), "video");
        refused(sniff(b"plain text", "text/html", ""), "web page");
        refused(sniff(b"", "audio/mpeg", ""), "empty");
        assert_eq!(sniff(b"\xEF\xBB\xBF#EXTM3U\nhttp://a/b\n", "", ""), Detected::Playlist { pls: false });
        assert_eq!(sniff(b"[Playlist]\nFile1=http://a/b\n", "", ""), Detected::Playlist { pls: true });
        assert_eq!(sniff(b"http://a/b\n", "audio/x-mpegurl", ""), Detected::Playlist { pls: false });
        assert_eq!(sniff(b"http://a/b\n", "text/plain", "http://h/list.pls?x"), Detected::Playlist { pls: true });
    }

    #[test]
    fn transport_stream_video() {
        // A PAT pointing at PMT PID 0x100, and a PMT with an H.264 stream.
        let mut ts = Vec::new();
        let mut pat = vec![0x47, 0x40, 0x00, 0x10, 0x00];
        pat.extend_from_slice(&[0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01, 0xE1, 0x00, 0, 0, 0, 0]);
        pat.resize(188, 0xFF);
        let mut pmt = vec![0x47, 0x41, 0x00, 0x10, 0x00];
        pmt.extend_from_slice(&[0x02, 0xB0, 0x12, 0x00, 0x01, 0xC1, 0x00, 0x00, 0xE1, 0x01, 0xF0, 0x00, 0x1B, 0xE1, 0x01, 0xF0, 0x00, 0, 0, 0, 0]);
        pmt.resize(188, 0xFF);
        ts.extend_from_slice(&pat);
        ts.extend_from_slice(&pmt);
        ts.extend_from_slice(&pat);
        refused(sniff(&ts, "", ""), "MPEG-TS with a video stream");
    }
}
