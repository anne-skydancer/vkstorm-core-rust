//! MPEG audio (layers I, II, III; MPEG-1, -2 and 2.5) and AAC in ADTS:
//! frame sync, header validation and framing for the decoders, with
//! resynchronisation after garbage (streams joined mid-frame, ID3 tags
//! between tracks) and ID3v2 tag reading.

#![forbid(unsafe_code)]

/// A parsed MPEG audio frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MpegHeader {
    /// 1 = MPEG-1, 2 = MPEG-2, 25 = MPEG-2.5.
    pub version: u8,
    pub layer: u8,
    pub sample_rate: u32,
    pub channels: u16,
    pub frame_len: usize,
}

const BITRATES: [[[u16; 16]; 3]; 2] = [
    // MPEG-1: layer I, II, III
    [
        [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, 0],
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0],
        [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0],
    ],
    // MPEG-2/2.5: layer I, II, III
    [
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 0],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0],
    ],
];

/// Parses an MPEG audio header (free-format bitrate is not supported).
pub(crate) fn parse_mpeg(b: &[u8]) -> Option<MpegHeader> {
    if b.len() < 4 || b[0] != 0xFF || b[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = match (b[1] >> 3) & 3 {
        0 => 25,
        2 => 2,
        3 => 1,
        _ => return None,
    };
    let layer = match (b[1] >> 1) & 3 {
        1 => 3,
        2 => 2,
        3 => 1,
        _ => return None, // 0 is ADTS's layer field
    };
    let br_idx = usize::from(b[2] >> 4);
    let sr_idx = usize::from((b[2] >> 2) & 3);
    if br_idx == 0 || br_idx == 15 || sr_idx == 3 || b[3] & 3 == 2 {
        return None;
    }
    let base = [44100, 48000, 32000][sr_idx];
    let sample_rate = match version {
        1 => base,
        2 => base / 2,
        _ => base / 4,
    };
    let bitrate = u32::from(BITRATES[usize::from(version != 1)][usize::from(layer - 1)][br_idx]) * 1000;
    let pad = u32::from((b[2] >> 1) & 1);
    let frame_len = match layer {
        1 => (12 * bitrate / sample_rate + pad) * 4,
        2 => 144 * bitrate / sample_rate + pad,
        _ if version == 1 => 144 * bitrate / sample_rate + pad,
        _ => 72 * bitrate / sample_rate + pad,
    } as usize;
    let channels = if b[3] >> 6 == 3 { 1 } else { 2 };
    if frame_len < 4 {
        return None;
    }
    Some(MpegHeader { version, layer, sample_rate, channels, frame_len })
}

/// A parsed ADTS header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AdtsHeader {
    /// MPEG-4 audio object type (profile + 1; 2 = AAC LC).
    pub object_type: u8,
    pub sample_rate: u32,
    /// Channel configuration (0 = defined in-band by a PCE).
    pub channel_config: u8,
    pub header_len: usize,
    pub frame_len: usize,
    pub raw_blocks: u8,
}

pub(crate) const ADTS_RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

pub(crate) fn parse_adts(b: &[u8]) -> Option<AdtsHeader> {
    if b.len() < 7 || b[0] != 0xFF || b[1] & 0xF6 != 0xF0 {
        return None;
    }
    let protection_absent = b[1] & 1 == 1;
    let object_type = (b[2] >> 6) + 1;
    let sr_idx = usize::from((b[2] >> 2) & 0xF);
    let sample_rate = *ADTS_RATES.get(sr_idx)?;
    let channel_config = ((b[2] & 1) << 2) | (b[3] >> 6);
    let frame_len = (usize::from(b[3] & 3) << 11) | (usize::from(b[4]) << 3) | usize::from(b[5] >> 5);
    let header_len = if protection_absent { 7 } else { 9 };
    if frame_len <= header_len {
        return None;
    }
    Some(AdtsHeader { object_type, sample_rate, channel_config, header_len, frame_len, raw_blocks: b[6] & 3 })
}

/// The size of an ID3v2 tag at the start of `b` (header, body and footer),
/// or None when `b` does not start with one.
pub(crate) fn id3v2_len(b: &[u8]) -> Option<usize> {
    if b.len() < 10 || &b[..3] != b"ID3" || b[3] == 0xFF || b[4] == 0xFF || b[6..10].iter().any(|&x| x & 0x80 != 0) {
        return None;
    }
    let size = b[6..10].iter().fold(0usize, |a, &x| (a << 7) | usize::from(x));
    let footer = if b[3] >= 4 && b[5] & 0x10 != 0 { 10 } else { 0 };
    Some(10 + size + footer)
}

/// TIT2/TPE1 (TT2/TP1 in v2.2) of a whole ID3v2 tag: (artist, title).
pub(crate) fn id3v2_artist_title(tag: &[u8]) -> (String, String) {
    let mut artist = String::new();
    let mut title = String::new();
    if tag.len() < 10 {
        return (artist, title);
    }
    let ver = tag[3];
    let flags = tag[5];
    let mut body: Vec<u8> = tag[10..].to_vec();
    if ver >= 4 && flags & 0x10 != 0 && body.len() >= 10 {
        body.truncate(body.len() - 10);
    }
    // Whole-tag unsynchronisation (v2.2/2.3): FF 00 -> FF.
    if flags & 0x80 != 0 && ver < 4 {
        let mut out = Vec::with_capacity(body.len());
        let mut i = 0;
        while i < body.len() {
            out.push(body[i]);
            if body[i] == 0xFF && body.get(i + 1) == Some(&0) {
                i += 1;
            }
            i += 1;
        }
        body = out;
    }
    let mut p = 0;
    if flags & 0x40 != 0 && ver >= 3 && body.len() >= 4 {
        let ext = if ver >= 4 {
            body[..4].iter().fold(0usize, |a, &x| (a << 7) | usize::from(x & 0x7F))
        } else {
            4 + u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize
        };
        p = ext;
    }
    let (id_len, hdr_len) = if ver == 2 { (3, 6) } else { (4, 10) };
    while p + hdr_len <= body.len() {
        let id = &body[p..p + id_len];
        if id[0] == 0 {
            break;
        }
        let size = match ver {
            2 => (usize::from(body[p + 3]) << 16) | (usize::from(body[p + 4]) << 8) | usize::from(body[p + 5]),
            3 => u32::from_be_bytes([body[p + 4], body[p + 5], body[p + 6], body[p + 7]]) as usize,
            _ => body[p + 4..p + 8].iter().fold(0usize, |a, &x| (a << 7) | usize::from(x & 0x7F)),
        };
        let start = p + hdr_len;
        let Some(end) = start.checked_add(size).filter(|&e| e <= body.len()) else { break };
        let frame = &body[start..end];
        match id {
            b"TIT2" | b"TT2" => title = id3_text(frame),
            b"TPE1" | b"TP1" => artist = id3_text(frame),
            _ => {}
        }
        p = end;
    }
    (artist, title)
}

/// An ID3v2 text frame's first value.
fn id3_text(frame: &[u8]) -> String {
    let Some((&enc, data)) = frame.split_first() else { return String::new() };
    let s = match enc {
        0 => data.iter().take_while(|&&b| b != 0).map(|&b| char::from(b)).collect(),
        3 => {
            let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
            String::from_utf8_lossy(&data[..end]).into_owned()
        }
        1 | 2 => {
            let mut be = enc == 2;
            let mut d = data;
            if d.len() >= 2 && enc == 1 {
                match (d[0], d[1]) {
                    (0xFE, 0xFF) => { be = true; d = &d[2..]; }
                    (0xFF, 0xFE) => { be = false; d = &d[2..]; }
                    _ => {}
                }
            }
            let units: Vec<u16> = d
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| if be { u16::from_be_bytes(c) } else { u16::from_le_bytes(c) })
                .take_while(|&u| u != 0)
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => String::new(),
    };
    s.trim_end_matches('\0').to_owned()
}

/// The outcome of [`chain_frames`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chain {
    Yes,
    No,
    /// The frames run past the buffered data.
    NeedMore,
}

/// Whether `b` starts with `n` consecutive valid frames of one stream (MPEG
/// audio, or ADTS when `adts`). When the data ends inside the chain, at
/// least one whole valid frame counts if `eof` (a short final stream).
pub(crate) fn chain_frames(b: &[u8], n: usize, adts: bool, eof: bool) -> Chain {
    let mut at = 0;
    let mut first: Option<(u32, u8)> = None;
    for k in 0..n {
        if at + 7 > b.len() {
            return if !eof { Chain::NeedMore } else if k > 0 { Chain::Yes } else { Chain::No };
        }
        let (len, key) = if adts {
            match parse_adts(&b[at..]) {
                Some(h) => (h.frame_len, (h.sample_rate, h.object_type)),
                None => return Chain::No,
            }
        } else {
            match parse_mpeg(&b[at..]) {
                Some(h) => (h.frame_len, (h.sample_rate, h.layer)),
                None => return Chain::No,
            }
        };
        if *first.get_or_insert(key) != key {
            return Chain::No;
        }
        at += len;
    }
    Chain::Yes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpeg_headers() {
        // MPEG-1 layer III 128 kbit/s 44.1 kHz, stereo, no padding.
        let h = parse_mpeg(&[0xFF, 0xFB, 0x90, 0x00]).unwrap();
        assert_eq!((h.version, h.layer, h.sample_rate, h.channels, h.frame_len), (1, 3, 44100, 2, 417));
        // MPEG-2 layer III 64 kbit/s 22.05 kHz mono.
        let h = parse_mpeg(&[0xFF, 0xF3, 0x80, 0xC0]).unwrap();
        assert_eq!((h.version, h.sample_rate, h.channels, h.frame_len), (2, 22050, 1, 208));
        assert!(parse_mpeg(&[0xFF, 0xF1, 0x50, 0x80]).is_none(), "ADTS is not MPEG audio");
    }

    #[test]
    fn adts_header() {
        let h = parse_adts(&[0xFF, 0xF1, 0x50, 0x80, 0x2E, 0x7F, 0xFC]).unwrap();
        assert_eq!((h.object_type, h.sample_rate, h.channel_config, h.header_len), (2, 44100, 2, 7));
        assert_eq!(h.frame_len, 0x173);
    }

    #[test]
    fn id3() {
        let mut tag = b"ID3\x03\x00\x00".to_vec();
        let mut frames = Vec::new();
        for (id, text) in [(b"TIT2", "Title"), (b"TPE1", "Artist")] {
            frames.extend_from_slice(id);
            frames.extend_from_slice(&((text.len() + 1) as u32).to_be_bytes());
            frames.extend_from_slice(&[0, 0, 0]);
            frames.extend_from_slice(text.as_bytes());
        }
        let n = frames.len();
        tag.extend_from_slice(&[(n >> 21) as u8 & 0x7F, (n >> 14) as u8 & 0x7F, (n >> 7) as u8 & 0x7F, n as u8 & 0x7F]);
        tag.extend_from_slice(&frames);
        assert_eq!(id3v2_len(&tag), Some(tag.len()));
        assert_eq!(id3v2_artist_title(&tag), ("Artist".into(), "Title".into()));
    }
}
