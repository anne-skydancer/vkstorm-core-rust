//! Sound asset decoding: the byte-exact output of the viewer's
//! `LLVorbisDecodeState` (`indra/llaudio/llaudiodecodemgr.cpp`) on
//! vkstorm-vorbis's `VorbisFile`, i.e. the decoded-sound cache file
//! (`<uuid>.dsf`) the buffers then load.
//!
//! - `initDecode`: an empty asset fails; `ov_open_callbacks` must succeed;
//!   `sample_count = (S32)ov_pcm_total(-1)`; `size_guess = (size_t)
//!   sample_count * channels * 2 + 2048`; the asset is rejected when the
//!   channel count is not 1 or 2 (`LLVORBIS_CLIP_MAX_CHANNELS`, or there
//!   is no default bitstream), when the sample count is 0 or exceeds
//!   `LLVORBIS_CLIP_REJECT_SAMPLES` (30 s x 44100 x 2 x 3 = 7 938 000; the
//!   OpenSim 60 s limit is an upload-side rule and does not apply to
//!   decoding), or when the size guess exceeds `LLVORBIS_CLIP_REJECT_SIZE`
//!   (15 876 000 bytes). The C++ logs every failing check; the error here
//!   is the first in that order.
//! - The 44-byte header, always "mono, 44100 Hz, 16-bit PCM" whatever the
//!   stream says (a stereo asset's interleaved samples follow it as is).
//! - `decodeSection`: `ov_read(4096, little-endian, 16-bit, signed)` until
//!   it returns 0; a negative return fails the decode.
//! - `finishDecode`: the RIFF and data lengths, then the "cheap-and-cheesy
//!   crossfade": the first `min(128, data_bytes / 8)` 16-bit samples are
//!   scaled by `llfloor((F32)s * ((F32)i / (F32)fade_length))`. The tail
//!   fade is computed by the C++ but never written back (its guard
//!   `near_end + 2 * fade_length < size` is always false), so the tail is
//!   unchanged. An empty decode (`36 == data_length`) fails.

#![forbid(unsafe_code)]

use vkstorm_vorbis::VorbisFile;

/// `LLVORBIS_CLIP_MAX_CHANNELS`.
pub const CLIP_MAX_CHANNELS: i32 = 2;
/// `LLVORBIS_CLIP_REJECT_SAMPLES`: 3 x the 30 s stereo 44.1 kHz maximum.
pub const CLIP_REJECT_SAMPLES: u64 = (30.0f32 * 44100.0) as u64 * 2 * 3;
/// `LLVORBIS_CLIP_REJECT_SIZE`: 3 x the maximum 16-bit sample data.
pub const CLIP_REJECT_SIZE: u64 = (30.0f32 * 44100.0) as u64 * 2 * 2 * 3;

const WAV_HEADER_SIZE: usize = 44;

/// Why an asset was not decoded (the C++'s log messages).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetDecodeError {
    /// "unable to open vorbis source vfile for reading" (empty asset).
    EmptyAsset,
    /// "Input to vorbis decode does not appear to be an Ogg bitstream"
    /// (the `ov_open_callbacks` result).
    NotOgg(i32),
    /// "Bad channel count".
    BadChannelCount(i32),
    /// "No default bitstream found".
    NoDefaultBitstream,
    /// "Illegal sample count" (the S32 sample count).
    IllegalSampleCount(i32),
    /// "Illegal sample size" (the size guess).
    IllegalSampleSize(u64),
    /// "BAD vorbis decode in decodeSection." (the `ov_read` result).
    DecodeError(i32),
    /// "BAD Vorbis decode in finishDecode!" (no samples).
    EmptyDecode,
}

impl AssetDecodeError {
    /// A stable text form (also printed by the C oracle).
    pub fn code_name(&self) -> String {
        match self {
            AssetDecodeError::EmptyAsset => "EmptyAsset".into(),
            AssetDecodeError::NotOgg(r) => format!("NotOgg({r})"),
            AssetDecodeError::BadChannelCount(n) => format!("BadChannelCount({n})"),
            AssetDecodeError::NoDefaultBitstream => "NoDefaultBitstream".into(),
            AssetDecodeError::IllegalSampleCount(n) => format!("IllegalSampleCount({n})"),
            AssetDecodeError::IllegalSampleSize(n) => format!("IllegalSampleSize({n})"),
            AssetDecodeError::DecodeError(r) => format!("DecodeError({r})"),
            AssetDecodeError::EmptyDecode => "EmptyDecode".into(),
        }
    }
}

impl std::fmt::Display for AssetDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetDecodeError::EmptyAsset => {
                f.write_str("unable to open vorbis source vfile for reading")
            }
            AssetDecodeError::NotOgg(r) => write!(
                f,
                "{r} Input to vorbis decode does not appear to be an Ogg bitstream"
            ),
            AssetDecodeError::BadChannelCount(n) => write!(f, "Bad channel count: {n}"),
            AssetDecodeError::NoDefaultBitstream => f.write_str("No default bitstream found"),
            AssetDecodeError::IllegalSampleCount(n) => write!(f, "Illegal sample count: {n}"),
            AssetDecodeError::IllegalSampleSize(n) => write!(f, "Illegal sample size: {n}"),
            AssetDecodeError::DecodeError(r) => {
                write!(f, "BAD vorbis decode in decodeSection ({r})")
            }
            AssetDecodeError::EmptyDecode => f.write_str("BAD Vorbis decode in finishDecode!"),
        }
    }
}

impl std::error::Error for AssetDecodeError {}

/// Decodes a sound asset (Ogg Vorbis bytes) to the decoded-sound cache file
/// the viewer writes, byte for byte.
pub fn decode_sound_asset(asset: &[u8]) -> Result<Vec<u8>, AssetDecodeError> {
    if asset.is_empty() {
        return Err(AssetDecodeError::EmptyAsset);
    }
    let mut vf = VorbisFile::open(asset).map_err(|e| AssetDecodeError::NotOgg(e.code()))?;

    // initDecode
    let sample_count = vf.pcm_total(-1) as i32;
    let channels = vf.info(-1).map(|vi| vi.channels);
    let mut size_guess = sample_count as i64 as u64;
    size_guess = size_guess.wrapping_mul(channels.unwrap_or(1) as i64 as u64);
    size_guess = size_guess.wrapping_mul(2);
    size_guess = size_guess.wrapping_add(2048);
    match channels {
        Some(c) if !(1..=CLIP_MAX_CHANNELS).contains(&c) => {
            return Err(AssetDecodeError::BadChannelCount(c));
        }
        None => return Err(AssetDecodeError::NoDefaultBitstream),
        _ => {}
    }
    let as_size = sample_count as i64 as u64;
    if as_size > CLIP_REJECT_SAMPLES || as_size == 0 {
        return Err(AssetDecodeError::IllegalSampleCount(sample_count));
    }
    if size_guess > CLIP_REJECT_SIZE {
        return Err(AssetDecodeError::IllegalSampleSize(size_guess));
    }

    let mut wav = Vec::with_capacity(size_guess as usize);
    wav.extend_from_slice(&[
        b'R', b'I', b'F', b'F', 0, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ', 0x10,
        0, 0, 0, // fmt size 16
        0x01, 0x00, // PCM
        0x01, 0x00, // one channel
        0x44, 0xAC, 0x00, 0x00, // 44100 Hz
        0x88, 0x58, 0x01, 0x00, // 88200 bytes/s
        0x02, 0x00, // block align
        0x10, 0x00, // 16 bits
        b'd', b'a', b't', b'a', 0, 0, 0, 0,
    ]);

    // decodeSection
    let mut pcmout = [0u8; 4096];
    let mut section = 0i32;
    loop {
        let ret = vf.read(&mut pcmout, false, 2, true, &mut section);
        if ret == 0 {
            break;
        }
        if ret < 0 {
            return Err(AssetDecodeError::DecodeError(ret as i32));
        }
        wav.extend_from_slice(&pcmout[..ret as usize]);
    }

    // finishDecode
    let mut data_length = wav.len() as i32 - WAV_HEADER_SIZE as i32;
    wav[40..44].copy_from_slice(&data_length.to_le_bytes());
    data_length += 36;
    wav[4..8].copy_from_slice(&data_length.to_le_bytes());

    let fade_length = 128.min((data_length - 36) / 8);
    if WAV_HEADER_SIZE as i32 + 2 * fade_length < wav.len() as i32 {
        for i in 0..fade_length {
            let p = WAV_HEADER_SIZE + 2 * i as usize;
            let s = i16::from_le_bytes([wav[p], wav[p + 1]]);
            let v = (s as f32 * (i as f32 / fade_length as f32)).floor() as i32 as i16;
            wav[p..p + 2].copy_from_slice(&v.to_le_bytes());
        }
    }
    // The tail "crossfade" is never written back by the C++ (see above).

    if data_length == 36 {
        return Err(AssetDecodeError::EmptyDecode);
    }
    Ok(wav)
}
