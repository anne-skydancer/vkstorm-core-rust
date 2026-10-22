//! Elementary stream demuxers: MPEG audio, AAC in ADTS (with the LC /
//! HE-AAC routing) and native FLAC. Each runs until its input ends and
//! hands decoded audio and tags to a [`PcmSink`].
//!
//! # AAC routing
//! ADTS cannot signal HE-AAC: with implicit signalling the header says
//! AAC-LC at the core (half) rate, and SBR/PS only show up as extension
//! payloads inside the raw data blocks. The first [`PROBE_FRAMES`] frames
//! are therefore decoded with FFmpeg; when its output rate is twice the
//! header's rate (SBR) or it outputs a different channel count (PS: a mono
//! core decoded to stereo), the stream is HE-AAC and stays on FFmpeg (the
//! probe decoder's output is kept, nothing is decoded twice). Otherwise it
//! is AAC-LC and goes to symphonia (the probe frames are decoded again
//! there). FFmpeg also takes what symphonia's AAC-LC decoder does not do:
//! other object types (Main, LTP), more than two channels, in-band channel
//! configuration (PCE) and several raw data blocks per frame. Without
//! FFmpeg, LC-compatible streams play through symphonia (an HE-AAC stream
//! then plays its AAC-LC core: correct speed, half the bandwidth, mono for
//! v2) and the rest fail with the reason.

#![forbid(unsafe_code)]

use super::codec::{InBand, PcmSink, StreamError, SymDecoder, default_layout};
use super::decode::DecodeCtx;
use super::ffmpeg::FfDecoder;
use super::input::Input;
use super::mpeg::{AdtsHeader, MpegHeader, id3v2_artist_title, id3v2_len, parse_adts, parse_mpeg};
use crate::music::ChannelLayout;

/// ADTS frames decoded with FFmpeg to classify an AAC stream.
pub(crate) const PROBE_FRAMES: usize = 4;
/// ID3 tags larger than this are skipped without reading them.
const MAX_ID3: usize = 1 << 20;

/// Skips (and reads, when small) an ID3v2 tag at the input position.
/// Returns false when there is none.
pub(crate) fn skip_id3(input: &mut Input, sink: &mut dyn PcmSink) -> Result<bool, StreamError> {
    input.fill(10)?;
    let Some(len) = id3v2_len(input.data()) else { return Ok(false) };
    if len <= MAX_ID3 && input.fill(len)? {
        let (artist, title) = id3v2_artist_title(&input.data()[..len]);
        if !artist.is_empty() || !title.is_empty() {
            sink.metadata(InBand::Tags { artist, title });
        }
        input.consume(len);
    } else {
        input.skip(len as u64)?;
    }
    Ok(true)
}

/// Frames a stream of `parse`-able headers: returns the next frame's
/// length at the input position, resynchronising over garbage. `None` at
/// the end of the input. `locked` says the previous frame was in sync, in
/// which case a frame with the same key is taken without looking ahead.
fn next_frame<H: Copy, K: PartialEq>(
    input: &mut Input,
    sink: &mut dyn PcmSink,
    parse: fn(&[u8]) -> Option<H>,
    len_of: fn(&H) -> usize,
    key_of: fn(&H) -> K,
    locked: &mut Option<K>,
) -> Result<Option<H>, StreamError> {
    loop {
        if !input.fill(10)? && input.len() < 4 {
            return Ok(None);
        }
        if input.data().starts_with(b"ID3") && skip_id3(input, sink)? {
            *locked = None;
            continue;
        }
        let Some(h) = parse(input.data()) else {
            *locked = None;
            let d = input.data();
            let skip = d[1..].iter().position(|&b| b == 0xFF).map_or(d.len(), |p| p + 1);
            input.consume(skip);
            continue;
        };
        let len = len_of(&h);
        let key = key_of(&h);
        let in_sync = locked.as_ref() == Some(&key);
        // Out of sync: confirm with the next header (unless the input ends
        // right after this frame).
        let need = if in_sync { len } else { len + 4 };
        input.fill(need)?;
        if input.len() < len {
            return Ok(None); // a truncated last frame
        }
        if !in_sync && input.len() >= len + 4 {
            let next = &input.data()[len..];
            let ok = parse(next).is_some_and(|n| key_of(&n) == key) || next.starts_with(b"ID3");
            if !ok {
                input.consume(1);
                continue;
            }
        }
        *locked = Some(key);
        return Ok(Some(h));
    }
}

/// MPEG audio layers I-III.
pub(crate) fn run_mpeg(input: &mut Input, sink: &mut dyn PcmSink) -> Result<(), StreamError> {
    let mut dec: Option<(SymDecoder, (u8, u32, u16))> = None;
    let mut locked = None;
    while let Some(h) = next_frame(
        input,
        sink,
        parse_mpeg,
        |h: &MpegHeader| h.frame_len,
        |h: &MpegHeader| (h.layer, h.sample_rate),
        &mut locked,
    )? {
        let key = (h.layer, h.sample_rate, h.channels);
        if dec.as_ref().is_none_or(|(_, k)| *k != key) {
            let d = SymDecoder::mpeg(h.layer, h.sample_rate, h.channels).map_err(StreamError::Failed)?;
            sink.codec(d.name());
            dec = Some((d, key));
        }
        if let Some((d, _)) = &mut dec {
            d.decode(&input.data()[..h.frame_len], sink)?;
        }
        input.consume(h.frame_len);
    }
    Ok(())
}

enum AacRoute {
    Sym(SymDecoder),
    Ff(FfDecoder),
}

/// Collects a probe decoder's output for replay.
struct Captured {
    chunks: Vec<(Vec<Vec<f32>>, usize, u32, ChannelLayout)>,
}

/// AAC in ADTS.
pub(crate) fn run_adts(input: &mut Input, ctx: &DecodeCtx, sink: &mut dyn PcmSink) -> Result<(), StreamError> {
    let mut route: Option<(AacRoute, (u32, u8))> = None;
    let mut probe: Vec<(AdtsHeader, Vec<u8>)> = Vec::new();
    let mut locked = None;
    loop {
        let h = next_frame(
            input,
            sink,
            parse_adts,
            |h: &AdtsHeader| h.frame_len,
            |h: &AdtsHeader| (h.sample_rate, h.channel_config),
            &mut locked,
        )?;
        let key = h.map(|h| (h.sample_rate, h.channel_config));
        // A configuration change re-routes (like a new stream).
        if let (Some((_, k)), Some(nk)) = (&route, key)
            && *k != nk
        {
            route = None;
        }
        if route.is_none() {
            if let Some(h) = h {
                probe.push((h, input.data()[..h.frame_len].to_vec()));
                input.consume(h.frame_len);
                if probe.len() < PROBE_FRAMES {
                    continue;
                }
            }
            if probe.is_empty() {
                return Ok(());
            }
            let r = choose_aac_route(&probe, ctx, sink)?;
            route = Some((r, (probe[0].0.sample_rate, probe[0].0.channel_config)));
            probe.clear();
            if h.is_none() {
                return Ok(());
            }
            continue;
        }
        let Some(h) = h else { return Ok(()) };
        let frame = &input.data()[..h.frame_len];
        match &mut route {
            Some((AacRoute::Sym(d), _)) => d.decode(&frame[h.header_len..], sink)?,
            Some((AacRoute::Ff(d), _)) => d.decode_into(frame, sink)?,
            None => {}
        }
        input.consume(h.frame_len);
    }
}

fn choose_aac_route(
    probe: &[(AdtsHeader, Vec<u8>)],
    ctx: &DecodeCtx,
    sink: &mut dyn PcmSink,
) -> Result<AacRoute, StreamError> {
    let h = probe[0].0;
    let lc_ok = h.object_type == 2 && (1..=2).contains(&h.channel_config) && probe.iter().all(|(f, _)| f.raw_blocks == 0);
    match ctx.ffmpeg() {
        Ok(ff) => {
            let mut d = FfDecoder::aac(ff).map_err(StreamError::Failed)?;
            let mut cap = Captured { chunks: Vec::new() };
            for (_, f) in probe {
                d.decode(f, &mut |planes, frames, rate, layout| {
                    cap.chunks.push((planes.iter().map(|p| p.to_vec()).collect(), frames, rate, layout));
                    Ok(())
                })?;
            }
            let out = cap.chunks.first().map(|(_, _, r, l)| (*r, l.channels));
            let sbr = out.is_some_and(|(r, _)| r == 2 * h.sample_rate);
            let ps = out.is_some_and(|(_, c)| u8::try_from(c).ok() != Some(h.channel_config));
            if lc_ok && !sbr && !ps && out.is_some() {
                let mut s = SymDecoder::aac_lc(h.sample_rate, u16::from(h.channel_config)).map_err(StreamError::Failed)?;
                sink.codec(s.name());
                for (f, bytes) in probe {
                    s.decode(&bytes[f.header_len..], sink)?;
                }
                return Ok(AacRoute::Sym(s));
            }
            sink.codec(if ps { "he-aac-v2" } else if sbr { "he-aac" } else { "aac-ffmpeg" });
            for (planes, frames, rate, layout) in cap.chunks {
                let refs: Vec<&[f32]> = planes.iter().map(Vec::as_slice).collect();
                sink.pcm(&refs, frames, rate, layout)?;
            }
            Ok(AacRoute::Ff(d))
        }
        Err(why) if lc_ok => {
            let _ = why;
            let mut s = SymDecoder::aac_lc(h.sample_rate, u16::from(h.channel_config)).map_err(StreamError::Failed)?;
            sink.codec(s.name());
            for (f, bytes) in probe {
                s.decode(&bytes[f.header_len..], sink)?;
            }
            Ok(AacRoute::Sym(s))
        }
        Err(why) => Err(StreamError::Failed(format!(
            "this AAC stream (object type {}, channel configuration {}) needs FFmpeg, which is unavailable: {why}",
            h.object_type, h.channel_config
        ))),
    }
}

/// The artist and title of a Vorbis comment structure (vendor, count,
/// `KEY=value` entries; the layout of Vorbis comment headers after their
/// 7-byte prefix, OpusTags after `OpusTags`, and FLAC VORBIS_COMMENT
/// blocks). The first ARTIST and TITLE, case-insensitive.
pub(crate) fn vorbis_comments(b: &[u8]) -> (String, String) {
    let mut artist = String::new();
    let mut title = String::new();
    let rd = |p: usize| b.get(p..p + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]) as usize);
    let Some(vlen) = rd(0) else { return (artist, title) };
    let mut p = 4usize.saturating_add(vlen);
    let Some(count) = rd(p) else { return (artist, title) };
    p += 4;
    for _ in 0..count.min(4096) {
        let Some(len) = rd(p) else { break };
        p += 4;
        let Some(c) = b.get(p..p.saturating_add(len)) else { break };
        p += len;
        let Some(eq) = c.iter().position(|&x| x == b'=') else { continue };
        let (k, v) = (&c[..eq], &c[eq + 1..]);
        let v = String::from_utf8_lossy(v).into_owned();
        if k.eq_ignore_ascii_case(b"ARTIST") && artist.is_empty() {
            artist = v;
        } else if k.eq_ignore_ascii_case(b"TITLE") && title.is_empty() {
            title = v;
        }
    }
    (artist, title)
}

// --- FLAC ---------------------------------------------------------------

const fn crc8_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u8;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

const fn crc16_table() -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u16) << 8;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC8: [u8; 256] = crc8_table();
static CRC16: [u16; 256] = crc16_table();

fn crc8(b: &[u8]) -> u8 {
    b.iter().fold(0u8, |c, &x| CRC8[usize::from(c ^ x)])
}

fn crc16(b: &[u8]) -> u16 {
    b.iter().fold(0u16, |c, &x| (c << 8) ^ CRC16[usize::from((c >> 8) as u8 ^ x)])
}

/// The length of a valid FLAC frame header at the start of `b` (CRC-8
/// checked), for a stream of `channels` channels.
fn flac_frame_header(b: &[u8], channels: u16) -> Option<usize> {
    if b.len() < 6 || b[0] != 0xFF || b[1] & 0xFE != 0xF8 {
        return None;
    }
    let bs = b[2] >> 4;
    let sr = b[2] & 0xF;
    let ca = b[3] >> 4;
    let ss = (b[3] >> 1) & 7;
    if bs == 0 || sr == 15 || ca > 10 || ss == 3 || b[3] & 1 != 0 {
        return None;
    }
    let ch = if ca < 8 { u16::from(ca) + 1 } else { 2 };
    if ch != channels {
        return None;
    }
    // The UTF-8-like coded frame/sample number.
    let lead = b[4];
    let extra = match lead {
        0x00..=0x7F => 0,
        0xC0..=0xDF => 1,
        0xE0..=0xEF => 2,
        0xF0..=0xF7 => 3,
        0xF8..=0xFB => 4,
        0xFC..=0xFD => 5,
        0xFE => 6,
        _ => return None,
    };
    let mut p = 5 + extra;
    if b.len() < p || b[5..p].iter().any(|&x| x & 0xC0 != 0x80) {
        return None;
    }
    p += match bs {
        6 => 1,
        7 => 2,
        _ => 0,
    };
    p += match sr {
        12 => 1,
        13 | 14 => 2,
        _ => 0,
    };
    if b.len() <= p || crc8(&b[..p]) != b[p] {
        return None;
    }
    Some(p + 1)
}

/// Native FLAC (`fLaC`, metadata blocks, frames).
pub(crate) fn run_flac(input: &mut Input, sink: &mut dyn PcmSink) -> Result<(), StreamError> {
    if !input.fill(4)? || &input.data()[..4] != b"fLaC" {
        return Err(StreamError::Failed("FLAC: missing stream marker".into()));
    }
    input.consume(4);
    let mut streaminfo: Option<Vec<u8>> = None;
    loop {
        if !input.fill(4)? {
            return Ok(());
        }
        let d = input.data();
        let last = d[0] & 0x80 != 0;
        let kind = d[0] & 0x7F;
        let len = (usize::from(d[1]) << 16) | (usize::from(d[2]) << 8) | usize::from(d[3]);
        input.consume(4);
        match kind {
            0 if len >= 34 => {
                if !input.fill(len)? {
                    return Ok(());
                }
                streaminfo = Some(input.data()[..34].to_vec());
                input.consume(len);
            }
            4 if len <= MAX_ID3 => {
                if !input.fill(len)? {
                    return Ok(());
                }
                let (artist, title) = vorbis_comments(&input.data()[..len]);
                if !artist.is_empty() || !title.is_empty() {
                    sink.metadata(InBand::Tags { artist, title });
                }
                input.consume(len);
            }
            _ => {
                if !input.skip(len as u64)? {
                    return Ok(());
                }
            }
        }
        if last {
            break;
        }
    }
    let si = streaminfo.ok_or_else(|| StreamError::Failed("FLAC: no STREAMINFO".into()))?;
    let channels = u16::from((si[12] >> 1) & 7) + 1;
    let max_frame = (usize::from(si[7]) << 16) | (usize::from(si[8]) << 8) | usize::from(si[9]);
    let max_frame = if max_frame == 0 { 1 << 20 } else { max_frame.min(1 << 22) };
    let mut dec = SymDecoder::flac(&si).map_err(StreamError::Failed)?;
    sink.codec(dec.name());
    run_flac_frames(input, channels, max_frame, &mut |frame, sink| dec.decode(frame, sink), sink)
}

/// Decodes one FLAC frame into the sink.
type FrameFn<'a> = dyn FnMut(&[u8], &mut dyn PcmSink) -> Result<(), StreamError> + 'a;

/// Splits FLAC frames: a frame runs from a valid header to the next valid
/// header whose preceding bytes pass the frame's CRC-16 (or to the end).
fn run_flac_frames(
    input: &mut Input,
    channels: u16,
    max_frame: usize,
    decode: &mut FrameFn<'_>,
    sink: &mut dyn PcmSink,
) -> Result<(), StreamError> {
    loop {
        if !input.fill(16)? && input.len() == 0 {
            return Ok(());
        }
        let Some(hlen) = flac_frame_header(input.data(), channels) else {
            if input.at_eof() && input.len() < 16 {
                return Ok(());
            }
            let d = input.data();
            let skip = d[1..].iter().position(|&b| b == 0xFF).map_or(d.len(), |p| p + 1);
            input.consume(skip);
            continue;
        };
        // Search for the end.
        let mut scan = hlen + 2;
        let end = loop {
            let d = input.data();
            let mut found = None;
            while scan + 16 <= d.len() {
                if d[scan] == 0xFF
                    && d[scan + 1] & 0xFE == 0xF8
                    && flac_frame_header(&d[scan..], channels).is_some()
                    && crc16(&d[..scan]) == 0
                {
                    found = Some(scan);
                    break;
                }
                scan += 1;
            }
            if let Some(e) = found {
                break Some(e);
            }
            if scan > max_frame + 16 {
                break None; // no frame end within the maximum frame size
            }
            if !input.more()? {
                let d = input.data();
                break if crc16(d) == 0 { Some(d.len()) } else { None };
            }
        };
        match end {
            Some(e) => {
                let frame = input.data()[..e].to_vec();
                decode(&frame, sink)?;
                input.consume(e);
            }
            None => input.consume(1),
        }
    }
}

/// FLAC frames from Ogg packets go straight to the decoder; this builds it
/// from an Ogg FLAC mapping header (`\x7FFLAC`, version, header count,
/// `fLaC`, the STREAMINFO block).
pub(crate) fn ogg_flac_decoder(first: &[u8]) -> Result<(SymDecoder, ChannelLayout), String> {
    if first.len() < 51 || &first[..5] != b"\x7FFLAC" || &first[9..13] != b"fLaC" || first[13] & 0x7F != 0 {
        return Err("Ogg FLAC: bad mapping header".into());
    }
    let si = &first[17..51];
    let channels = u16::from((si[12] >> 1) & 7) + 1;
    Ok((SymDecoder::flac(si)?, default_layout(channels)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crcs() {
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc16(b"123456789"), 0xFEE8);
    }

    #[test]
    fn comments() {
        let mut b = Vec::new();
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(b"ven");
        b.extend_from_slice(&2u32.to_le_bytes());
        for c in [&b"title=T"[..], b"ARTIST=A"] {
            b.extend_from_slice(&(c.len() as u32).to_le_bytes());
            b.extend_from_slice(c);
        }
        assert_eq!(vorbis_comments(&b), ("A".into(), "T".into()));
    }

    #[test]
    fn chain() {
        use super::super::mpeg::{Chain, chain_frames};
        let f = [0xFFu8, 0xFB, 0x90, 0x00];
        let mut b = Vec::new();
        for _ in 0..3 {
            b.extend_from_slice(&f);
            b.resize(b.len() + 413, 0);
        }
        assert_eq!(chain_frames(&b, 3, false, false), Chain::Yes);
        assert_eq!(chain_frames(&b[..800], 3, false, false), Chain::NeedMore);
        assert_eq!(chain_frames(&b[..800], 3, false, true), Chain::Yes);
        assert_eq!(chain_frames(&b[1..], 3, false, true), Chain::No);
    }
}
