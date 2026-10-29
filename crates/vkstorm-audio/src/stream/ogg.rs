//! Ogg: page sync and packet reassembly with `vkstorm_vorbis::ogg`, chained
//! links (internet radio starts a new logical stream, with new headers and
//! comments, at each track), and the codecs: Vorbis (vkstorm-vorbis, the
//! product's one Vorbis decoder), Opus (FFmpeg, with the OpusHead as extra
//! data, pre-skip removal and end trimming from the final granule
//! position) and FLAC (symphonia).
//!
//! A beginning-of-stream page starts a new link and resets the decoder; in
//! a group of BOS pages (multiplexed streams) the first audio stream is
//! played and the others ignored, and any video stream in the group
//! (Theora, VP8, Dirac, Daala, OGM video) fails the stream as video before
//! any audio is produced.

#![forbid(unsafe_code)]

use vkstorm_vorbis::Decoder as VorbisDecoder;
use vkstorm_vorbis::ogg::{Packet, PacketOut, Page, StreamState, SyncState};

use super::codec::{InBand, PcmSink, StreamError, SymDecoder, VORBIS_TO_WAVE, default_layout};
use super::decode::DecodeCtx;
use super::demux::{ogg_flac_decoder, vorbis_comments};
use super::ffmpeg::FfDecoder;
use super::input::Input;
use crate::music::ChannelLayout;

enum Codec {
    Vorbis {
        dec: Box<VorbisDecoder>,
        headers: u8,
    },
    Opus {
        dec: Option<FfDecoder>,
        headers: u8,
        skip_left: usize,
        decoded: u64,
    },
    Flac {
        dec: Option<(SymDecoder, ChannelLayout)>,
    },
}

struct Link {
    serial: i32,
    stream: StreamState,
    codec: Codec,
}

/// What a BOS packet starts.
enum Kind {
    Vorbis,
    Opus,
    Flac,
    Video(&'static str),
    Other,
}

fn identify(p: &[u8]) -> Kind {
    if p.starts_with(b"\x01vorbis") {
        Kind::Vorbis
    } else if p.starts_with(b"OpusHead") {
        Kind::Opus
    } else if p.starts_with(b"\x7FFLAC") {
        Kind::Flac
    } else if p.starts_with(b"\x80theora") {
        Kind::Video("Theora")
    } else if p.starts_with(b"OVP80") {
        Kind::Video("VP8")
    } else if p.starts_with(b"BBCD\0") {
        Kind::Video("Dirac")
    } else if p.starts_with(b"\x80daala") {
        Kind::Video("Daala")
    } else if p.starts_with(b"\x01video\0\0\0") {
        Kind::Video("OGM video")
    } else {
        Kind::Other
    }
}

pub(crate) fn run_ogg(
    input: &mut Input,
    ctx: &DecodeCtx,
    sink: &mut dyn PcmSink,
) -> Result<(), StreamError> {
    let mut sync = SyncState::new();
    let mut page = Page::new();
    let mut link: Option<Link> = None;
    // Inside a group of BOS pages (they precede the group's data pages).
    let mut in_group = false;
    let mut produced = false;
    loop {
        match sync.pageout(&mut page) {
            1 => {}
            0 => {
                if input.len() == 0 && !input.more()? {
                    break;
                }
                if sync.write(input.data()) != 0 {
                    return Err(StreamError::Failed("Ogg: sync buffer failure".into()));
                }
                let n = input.len();
                input.consume(n);
                continue;
            }
            _ => continue, // skipped garbage; sync resumes at the next page
        }
        if page.bos() {
            if !in_group {
                in_group = true;
                // A new group: whatever played before ends here.
                if link
                    .as_ref()
                    .is_some_and(|l| l.serial != page.serialno() || produced)
                {
                    link = None;
                }
            }
            match identify(page.body()) {
                Kind::Video(what) => {
                    return Err(StreamError::Failed(format!(
                        "the stream is video (Ogg with {what}), not audio"
                    )));
                }
                Kind::Other => continue,
                kind if link.is_none() => {
                    let codec = match kind {
                        Kind::Vorbis => Codec::Vorbis {
                            dec: Box::new(VorbisDecoder::new()),
                            headers: 0,
                        },
                        Kind::Opus => Codec::Opus {
                            dec: None,
                            headers: 0,
                            skip_left: 0,
                            decoded: 0,
                        },
                        _ => Codec::Flac { dec: None },
                    };
                    link = Some(Link {
                        serial: page.serialno(),
                        stream: StreamState::new(page.serialno()),
                        codec,
                    });
                    produced = false;
                }
                _ => continue, // a second audio stream of the group
            }
        } else {
            in_group = false;
        }
        let Some(l) = link.as_mut() else { continue };
        if page.serialno() != l.serial {
            continue;
        }
        if l.stream.pagein(&page) != 0 {
            continue;
        }
        loop {
            let pkt = match l.stream.packetout() {
                PacketOut::Packet(p) => p,
                PacketOut::Hole => continue,
                PacketOut::None => break,
            };
            produced |= packet(&mut l.codec, &pkt, ctx, sink)?;
        }
    }
    Ok(())
}

/// Handles one packet of the current link; true when audio came out.
fn packet(
    codec: &mut Codec,
    pkt: &Packet<'_>,
    ctx: &DecodeCtx,
    sink: &mut dyn PcmSink,
) -> Result<bool, StreamError> {
    match codec {
        Codec::Vorbis { dec, headers } => {
            if *headers < 3 {
                dec.headerin_packet(pkt)
                    .map_err(|e| StreamError::Failed(format!("Vorbis: bad header ({e:?})")))?;
                *headers += 1;
                if *headers == 2 {
                    let c = dec.comment();
                    let artist = c
                        .query_str("ARTIST", 0)
                        .map(|s| s.into_owned())
                        .unwrap_or_default();
                    let title = c
                        .query_str("TITLE", 0)
                        .map(|s| s.into_owned())
                        .unwrap_or_default();
                    sink.metadata(InBand::Tags { artist, title });
                }
                if *headers == 3 {
                    sink.codec("vorbis");
                    let ch = dec.info().channels;
                    if !(1..=8).contains(&ch) {
                        return Err(StreamError::Failed(format!(
                            "Vorbis with {ch} channels is not supported"
                        )));
                    }
                }
                return Ok(false);
            }
            if dec.synthesis(pkt).is_err() {
                return Ok(false); // a damaged or non-audio packet: skip
            }
            let rate = u32::try_from(dec.info().rate).unwrap_or(0);
            let pcm = dec.pcmout();
            let n = pcm.len();
            if n == 0 || rate == 0 {
                return Ok(false);
            }
            let ch = pcm.channels();
            let order = VORBIS_TO_WAVE[ch.min(8)];
            let planes: Vec<&[f32]> = order.iter().map(|&c| &pcm.channel(c)[..n]).collect();
            sink.pcm(&planes, n, rate, default_layout(ch as u16))?;
            dec.read(n)
                .map_err(|e| StreamError::Failed(format!("Vorbis: {e:?}")))?;
            Ok(true)
        }
        Codec::Opus {
            dec,
            headers,
            skip_left,
            decoded,
        } => {
            if *headers == 0 {
                let d = pkt.data;
                if d.len() < 19 {
                    return Err(StreamError::Failed("Opus: bad OpusHead".into()));
                }
                *skip_left = usize::from(u16::from_le_bytes([d[10], d[11]]));
                let ff = ctx.ffmpeg().map_err(|e| {
                    StreamError::Failed(format!("Opus needs FFmpeg, which is unavailable: {e}"))
                })?;
                *dec = Some(FfDecoder::opus(ff, d).map_err(StreamError::Failed)?);
                *headers = 1;
                sink.codec("opus");
                return Ok(false);
            }
            if *headers == 1 {
                *headers = 2;
                if let Some(rest) = pkt.data.strip_prefix(b"OpusTags") {
                    let (artist, title) = vorbis_comments(rest);
                    sink.metadata(InBand::Tags { artist, title });
                }
                return Ok(false);
            }
            let Some(d) = dec.as_mut() else {
                return Ok(false);
            };
            let mut out = false;
            let eos_granule = if pkt.e_o_s && pkt.granulepos >= 0 {
                Some(pkt.granulepos as u64)
            } else {
                None
            };
            d.decode(pkt.data, &mut |planes, frames, rate, layout| {
                // Pre-skip at the start of the link; at its end, the final
                // granule position bounds the samples (both in 48 kHz
                // samples counted from the link's first decoded sample).
                let start = (*skip_left).min(frames);
                *skip_left -= start;
                let before = *decoded;
                *decoded += frames as u64;
                let end = match eos_granule {
                    Some(g) => (g.saturating_sub(before)).min(frames as u64) as usize,
                    None => frames,
                };
                if end > start {
                    let parts: Vec<&[f32]> = planes.iter().map(|p| &p[start..end]).collect();
                    out = true;
                    sink.pcm(&parts, end - start, rate, layout)?;
                }
                Ok(())
            })?;
            Ok(out)
        }
        Codec::Flac { dec } => {
            if dec.is_none() {
                let (d, _) = ogg_flac_decoder(pkt.data).map_err(StreamError::Failed)?;
                sink.codec("flac");
                *dec = Some((d, ChannelLayout::STEREO));
                return Ok(false);
            }
            let Some((d, _)) = dec.as_mut() else {
                return Ok(false);
            };
            let data = pkt.data;
            if data.len() >= 2 && data[0] == 0xFF && data[1] & 0xFE == 0xF8 {
                d.decode(data, sink)?;
                return Ok(true);
            }
            // A metadata block header packet.
            if let Some((&t, body)) = data.split_first()
                && t & 0x7F == 4
                && body.len() >= 3
            {
                let (artist, title) = vorbis_comments(&body[3..]);
                sink.metadata(InBand::Tags { artist, title });
            }
            Ok(false)
        }
    }
}
