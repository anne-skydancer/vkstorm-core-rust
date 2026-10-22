//! The contract between parcel music decoding ([`crate::stream`]) and the
//! mixer's music bus ([`crate::mixer`]).
//!
//! The stream side decodes on its own threads and resamples to the output
//! device's rate; the mixer pulls frames on the audio thread, applies the
//! fade envelope, the user gain and the speaker fill, and mixes them into
//! the device layout. Pulling never blocks and never stretches time: a short
//! read is an underrun the mixer fills with silence, so the output carries
//! exactly the decoded frames, in order, at the device rate (the defect this
//! replaces was VLC speeding up playback at start to catch up its clock).

/// A channel layout as the WAVEFORMATEXTENSIBLE `dwChannelMask` bits
/// (`SPEAKER_FRONT_LEFT` = 0x1, `SPEAKER_FRONT_RIGHT` = 0x2,
/// `SPEAKER_FRONT_CENTER` = 0x4, `SPEAKER_LOW_FREQUENCY` = 0x8,
/// `SPEAKER_BACK_LEFT` = 0x10, `SPEAKER_BACK_RIGHT` = 0x20,
/// `SPEAKER_SIDE_LEFT` = 0x200, `SPEAKER_SIDE_RIGHT` = 0x400, ...). Samples
/// of a frame are interleaved in ascending bit order, as WASAPI orders them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelLayout {
    pub channels: u16,
    /// 0 when the source does not say (treated as the default layout for
    /// the channel count).
    pub mask: u32,
}

pub const SPEAKER_FRONT_LEFT: u32 = 0x1;
pub const SPEAKER_FRONT_RIGHT: u32 = 0x2;
pub const SPEAKER_FRONT_CENTER: u32 = 0x4;
pub const SPEAKER_LOW_FREQUENCY: u32 = 0x8;
pub const SPEAKER_BACK_LEFT: u32 = 0x10;
pub const SPEAKER_BACK_RIGHT: u32 = 0x20;
pub const SPEAKER_SIDE_LEFT: u32 = 0x200;
pub const SPEAKER_SIDE_RIGHT: u32 = 0x400;

impl ChannelLayout {
    pub const MONO: ChannelLayout = ChannelLayout { channels: 1, mask: SPEAKER_FRONT_CENTER };
    pub const STEREO: ChannelLayout =
        ChannelLayout { channels: 2, mask: SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT };
}

/// The result of one pull.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pulled {
    /// Frames written to the front of the output slice.
    pub frames: usize,
    /// The layout of those frames (one layout per pull; a layout change
    /// mid-stream ends the pull early).
    pub layout: ChannelLayout,
    /// True once the source will never produce another frame (stopped,
    /// failed or finished); `frames` may still be non-zero on that pull.
    pub ended: bool,
}

/// Decoded music at the device rate, pulled by the mixer on the audio
/// thread. Implementations must be wait-free or lock briefly (no I/O, no
/// allocation on the steady path).
pub trait MusicSource: Send {
    /// Fills `out` with up to `max_frames` frames, interleaved with
    /// `Pulled::layout.channels` samples per frame (`out` holds at least
    /// `max_frames * 8` samples). Returns how many were written.
    fn pull(&mut self, out: &mut [f32], max_frames: usize) -> Pulled;
}
