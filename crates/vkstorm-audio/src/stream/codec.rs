//! What the demuxers hand decoded audio and metadata to ([`PcmSink`]), the
//! symphonia decoder wrapper (MPEG audio, AAC-LC, FLAC) and the channel
//! order helpers.

#![forbid(unsafe_code)]

use symphonia_core::audio::{Channels, layouts};
use symphonia_core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_FLAC, CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3,
};
use symphonia_core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia_core::errors::Error as SymError;
use symphonia_core::packet::PacketRef;
use symphonia_core::units::{Duration, Timestamp};

use super::input::Cancelled;
use crate::music::{
    ChannelLayout, SPEAKER_BACK_LEFT, SPEAKER_BACK_RIGHT, SPEAKER_FRONT_CENTER, SPEAKER_FRONT_LEFT,
    SPEAKER_FRONT_RIGHT, SPEAKER_LOW_FREQUENCY, SPEAKER_SIDE_LEFT, SPEAKER_SIDE_RIGHT,
};

/// Why decoding stopped early.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StreamError {
    Cancelled,
    Failed(String),
}

impl From<Cancelled> for StreamError {
    fn from(_: Cancelled) -> Self {
        StreamError::Cancelled
    }
}

/// In-band metadata as the demuxers find it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InBand {
    /// ICY `StreamTitle` ("Artist - Title").
    NowPlaying(String),
    /// Tags: Vorbis comments, OpusTags, FLAC comments, ID3v2.
    Tags { artist: String, title: String },
}

/// A callback receiving decoded audio: planes, frames, rate, layout.
pub(crate) type PcmFn<'a> =
    dyn FnMut(&[&[f32]], usize, u32, ChannelLayout) -> Result<(), StreamError> + 'a;

/// Receives decoded audio (planar, any rate and layout) and metadata.
pub(crate) trait PcmSink {
    fn pcm(
        &mut self,
        planes: &[&[f32]],
        frames: usize,
        rate: u32,
        layout: ChannelLayout,
    ) -> Result<(), StreamError>;
    fn metadata(&mut self, m: InBand);
    /// The codec now decoding (for diagnostics and the tests).
    fn codec(&mut self, name: &'static str);
}

const SPEAKER_BACK_CENTER: u32 = 0x100;

/// The WAVE default layout for a channel count (FLAC's fixed order; Vorbis
/// and Opus after [`VORBIS_TO_WAVE`]).
pub(crate) fn default_layout(channels: u16) -> ChannelLayout {
    let (fl, fr, fc, lfe) = (
        SPEAKER_FRONT_LEFT,
        SPEAKER_FRONT_RIGHT,
        SPEAKER_FRONT_CENTER,
        SPEAKER_LOW_FREQUENCY,
    );
    let (bl, br, sl, sr) = (
        SPEAKER_BACK_LEFT,
        SPEAKER_BACK_RIGHT,
        SPEAKER_SIDE_LEFT,
        SPEAKER_SIDE_RIGHT,
    );
    let mask = match channels {
        1 => fc,
        2 => fl | fr,
        3 => fl | fr | fc,
        4 => fl | fr | bl | br,
        5 => fl | fr | fc | bl | br,
        6 => fl | fr | fc | lfe | bl | br,
        7 => fl | fr | fc | lfe | SPEAKER_BACK_CENTER | sl | sr,
        8 => fl | fr | fc | lfe | bl | br | sl | sr,
        _ => 0,
    };
    ChannelLayout { channels, mask }
}

/// For each WAVE-ordered output channel, the Vorbis-ordered input channel
/// (Vorbis I spec 4.3.9; Opus mapping family 1 uses the same order).
pub(crate) const VORBIS_TO_WAVE: [&[usize]; 9] = [
    &[],
    &[0],
    &[0, 1],
    &[0, 2, 1],
    &[0, 1, 2, 3],
    &[0, 2, 1, 3, 4],
    &[0, 2, 1, 5, 3, 4],
    &[0, 2, 1, 6, 5, 3, 4],
    &[0, 2, 1, 7, 5, 6, 3, 4],
];

/// The layout symphonia reports, as a WAVE mask (symphonia's position bits
/// are the WAVE bits; planes are in ascending bit order).
fn layout_of(ch: &Channels) -> ChannelLayout {
    let n = ch.count() as u16;
    match ch {
        Channels::Positioned(p) if n == 1 => {
            let _ = p;
            ChannelLayout::MONO
        }
        Channels::Positioned(p) if p.bits() <= u64::from(u32::MAX) => ChannelLayout {
            channels: n,
            mask: p.bits() as u32,
        },
        _ => default_layout(n),
    }
}

/// A symphonia decoder fed one frame per packet.
pub(crate) struct SymDecoder {
    dec: Box<dyn AudioDecoder>,
    planes: Vec<Vec<f32>>,
    name: &'static str,
    errors: u32,
}

fn options() -> AudioDecoderOptions {
    // No gapless trimming: a live stream has no delay/padding information,
    // and the frame accounting counts every decoded frame.
    let mut o = AudioDecoderOptions::default();
    o.gapless = false;
    o
}

impl SymDecoder {
    pub(crate) fn mpeg(layer: u8, rate: u32, channels: u16) -> Result<SymDecoder, String> {
        let (id, name) = match layer {
            1 => (CODEC_ID_MP1, "mp1"),
            2 => (CODEC_ID_MP2, "mp2"),
            _ => (CODEC_ID_MP3, "mp3"),
        };
        let mut p = AudioCodecParameters::new();
        p.for_codec(id)
            .with_sample_rate(rate)
            .with_channels(if channels == 1 {
                layouts::CHANNEL_LAYOUT_MONO
            } else {
                layouts::CHANNEL_LAYOUT_STEREO
            });
        let dec =
            symphonia_bundle_mp3::MpaDecoder::try_new(&p, &options()).map_err(|e| e.to_string())?;
        Ok(SymDecoder {
            dec: Box::new(dec),
            planes: Vec::new(),
            name,
            errors: 0,
        })
    }

    pub(crate) fn aac_lc(rate: u32, channels: u16) -> Result<SymDecoder, String> {
        let ch = match channels {
            1 => layouts::CHANNEL_LAYOUT_MONO,
            2 => layouts::CHANNEL_LAYOUT_STEREO,
            n => return Err(format!("AAC-LC with {n} channels needs FFmpeg")),
        };
        let mut p = AudioCodecParameters::new();
        p.for_codec(CODEC_ID_AAC)
            .with_sample_rate(rate)
            .with_channels(ch);
        let dec =
            symphonia_codec_aac::AacDecoder::try_new(&p, &options()).map_err(|e| e.to_string())?;
        Ok(SymDecoder {
            dec: Box::new(dec),
            planes: Vec::new(),
            name: "aac-lc",
            errors: 0,
        })
    }

    /// `streaminfo`: the 34-byte STREAMINFO block body.
    pub(crate) fn flac(streaminfo: &[u8]) -> Result<SymDecoder, String> {
        let mut p = AudioCodecParameters::new();
        p.for_codec(CODEC_ID_FLAC)
            .with_extra_data(streaminfo.to_vec().into_boxed_slice());
        let dec = symphonia_bundle_flac::FlacDecoder::try_new(&p, &options())
            .map_err(|e| e.to_string())?;
        Ok(SymDecoder {
            dec: Box::new(dec),
            planes: Vec::new(),
            name: "flac",
            errors: 0,
        })
    }

    pub(crate) fn name(&self) -> &'static str {
        self.name
    }

    /// Decodes one packet into `sink`. A malformed packet is skipped (the
    /// decoder resynchronises on the next); a run of failures is fatal.
    pub(crate) fn decode(
        &mut self,
        data: &[u8],
        sink: &mut dyn PcmSink,
    ) -> Result<(), StreamError> {
        let packet = PacketRef::new(0, Timestamp::new(0), Duration::new(0), data);
        match self.dec.decode_ref(&packet) {
            Ok(buf) => {
                self.errors = 0;
                let frames = buf.frames();
                if frames == 0 {
                    return Ok(());
                }
                let rate = buf.spec().rate();
                let layout = layout_of(buf.spec().channels());
                buf.copy_to_vecs_planar::<f32>(&mut self.planes);
                let refs: Vec<&[f32]> = self.planes.iter().map(|p| &p[..frames]).collect();
                sink.pcm(&refs, frames, rate, layout)
            }
            Err(SymError::DecodeError(_)) | Err(SymError::IoError(_)) => {
                self.errors += 1;
                if self.errors > 50 {
                    return Err(StreamError::Failed(format!(
                        "{}: undecodable stream",
                        self.name
                    )));
                }
                Ok(())
            }
            Err(SymError::ResetRequired) => {
                self.dec.reset();
                Ok(())
            }
            Err(e) => Err(StreamError::Failed(format!("{}: {e}", self.name))),
        }
    }
}
