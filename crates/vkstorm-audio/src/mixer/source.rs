//! Derived from SoLoud (Copyright (c) 2013-2020 Jari Komppa), zlib licence;
//! altered: rewritten in Rust. See LICENSE-soloud in this crate.
//!
//! Audio sources: SoLoud's `Wav` (planar float sample data with the
//! `AudioSource` attributes voices inherit) and the voice-side instances
//! (`WavInstance`, the adapter's wind instance).

#![forbid(unsafe_code)]

use std::sync::Arc;

use super::wind::WindInstance;

/// SoLoud's `AudioSource::ATTENUATION_MODELS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Attenuation {
    None = 0,
    InverseDistance = 1,
    LinearDistance = 2,
    ExponentialDistance = 3,
}

impl Attenuation {
    pub(crate) fn from_u32(v: u32) -> Attenuation {
        match v {
            1 => Attenuation::InverseDistance,
            2 => Attenuation::LinearDistance,
            3 => Attenuation::ExponentialDistance,
            _ => Attenuation::None,
        }
    }
}

/// The 3D defaults the viewer's buffers carry (`LLAudioBufferSoLoud::loadWAV`):
/// inverse distance, rolloff 1, min 1 m, max 512 m.
pub const VIEWER_MAX_AUDIBLE_DISTANCE: f32 = 512.0;

/// Samples of the head/tail fade the adapter bakes into every buffer of more
/// than four times that many frames.
pub const VIEWER_FADE_SAMPLES: u32 = 64;

/// Errors building a [`Wav`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavError {
    /// No frames, no channels, more than 8 channels, a non-positive or
    /// non-finite rate, or a data length that does not match.
    InvalidParameter,
}

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid sample data")
    }
}

impl std::error::Error for WavError {}

/// A loaded sound (SoLoud `Wav`): planar `f32` data (channel `c` at
/// `c * sample_count`), its base rate and channel count, and the
/// `AudioSource` attributes new voices inherit. Immutable once built and
/// shared with the voices playing it (`Arc`).
#[derive(Clone, Debug)]
pub struct Wav {
    data: Vec<f32>,
    sample_count: u32,
    channels: u32,
    base_samplerate: f32,
    /// `AudioSource::mVolume` (default volume for `play` with a negative volume).
    pub volume: f32,
    pub looping: bool,
    pub min_distance: f32,
    pub max_distance: f32,
    pub attenuation: Attenuation,
    pub rolloff: f32,
    pub doppler_factor: f32,
    /// `INAUDIBLE_TICK` / `INAUDIBLE_KILL` source flags (`setInaudibleBehavior`
    /// on the source; the viewer sets them per voice instead).
    pub inaudible_tick: bool,
    pub inaudible_kill: bool,
}

impl Wav {
    /// `Wav::loadRawWave(data, len, rate, channels, copy)`: `data` is taken
    /// as planar, `sample_count = data.len() / channels`. SoLoud's
    /// `AudioSource` defaults: volume 1, no attenuation, min 1, max 1e6,
    /// rolloff 1, Doppler factor 1.
    pub fn from_planar(
        data: Vec<f32>,
        base_samplerate: f32,
        channels: u32,
    ) -> Result<Wav, WavError> {
        if data.is_empty()
            || base_samplerate.is_nan()
            || base_samplerate <= 0.0
            || !base_samplerate.is_finite()
            || !(1..=8).contains(&channels)
        {
            return Err(WavError::InvalidParameter);
        }
        let sample_count = data.len() / channels as usize;
        if sample_count == 0 || sample_count > u32::MAX as usize {
            return Err(WavError::InvalidParameter);
        }
        Ok(Wav {
            data,
            sample_count: sample_count as u32,
            channels,
            base_samplerate,
            volume: 1.0,
            looping: false,
            min_distance: 1.0,
            max_distance: 1_000_000.0,
            attenuation: Attenuation::None,
            rolloff: 1.0,
            doppler_factor: 1.0,
            inaudible_tick: false,
            inaudible_kill: false,
        })
    }

    /// What `LLAudioBufferSoLoud::loadWAV` does after a successful load:
    /// inverse distance with rolloff 1, min/max distance 1/512 m, and the
    /// 64-sample linear head and tail fade (`i / 64`, applied to sample `i`
    /// and to sample `count - 1 - i` of every channel) when the buffer has
    /// more than 256 frames.
    pub fn apply_viewer_defaults(&mut self) {
        self.attenuation = Attenuation::InverseDistance;
        self.rolloff = 1.0;
        self.min_distance = 1.0;
        self.max_distance = VIEWER_MAX_AUDIBLE_DISTANCE;
        let n = self.sample_count as usize;
        if n > (VIEWER_FADE_SAMPLES * 4) as usize {
            for ch in 0..self.channels as usize {
                let chan = &mut self.data[ch * n..(ch + 1) * n];
                for i in 0..VIEWER_FADE_SAMPLES as usize {
                    let fade_gain = i as f32 / VIEWER_FADE_SAMPLES as f32;
                    chan[i] *= fade_gain;
                    chan[n - 1 - i] *= fade_gain;
                }
            }
        }
    }

    /// Frames per channel (`mSampleCount`).
    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }

    pub fn base_samplerate(&self) -> f32 {
        self.base_samplerate
    }

    /// Planar samples.
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// `Wav::getLength()`: `mSampleCount / mBaseSamplerate`, computed in
    /// float (unsigned / float) and widened.
    pub fn length(&self) -> f64 {
        if self.base_samplerate == 0.0 {
            return 0.0;
        }
        (self.sample_count as f32 / self.base_samplerate) as f64
    }
}

/// What a voice plays.
pub(crate) enum Instance {
    Wav { wav: Arc<Wav>, offset: u32 },
    Wind(Box<WindInstance>),
}

impl Instance {
    /// `getAudio(buffer, samples, buffer_size)`; planar, channel `c` at
    /// `c * buffer_size`.
    #[inline]
    pub(crate) fn get_audio(
        &mut self,
        channels: u32,
        buffer: &mut [f32],
        samples_to_read: usize,
        buffer_size: usize,
    ) -> usize {
        match self {
            Instance::Wav { wav, offset } => {
                if wav.data.is_empty() {
                    return 0;
                }
                let dataleft = (wav.sample_count - *offset) as usize;
                let copylen = dataleft.min(samples_to_read);
                let n = wav.sample_count as usize;
                let o = *offset as usize;
                for i in 0..channels as usize {
                    buffer[i * buffer_size..i * buffer_size + copylen]
                        .copy_from_slice(&wav.data[o + i * n..o + i * n + copylen]);
                }
                *offset += copylen as u32;
                copylen
            }
            Instance::Wind(w) => w.get_audio(buffer, samples_to_read, buffer_size),
        }
    }

    /// Advances like `getAudio` without producing output (the generic seek's
    /// discard loop); returns frames consumed.
    pub(crate) fn discard(&mut self, samples: usize, scratch: &mut [f32], channels: u32) -> usize {
        match self {
            Instance::Wav { wav, offset } => {
                let dataleft = (wav.sample_count - *offset) as usize;
                let copylen = dataleft.min(samples);
                *offset += copylen as u32;
                copylen
            }
            Instance::Wind(w) => {
                // Bounded: the C would overrun its scratch on an overflowed
                // (negative) count; the wind is never seeked by the viewer.
                let samples = samples.min(1 << 24);
                let per = scratch.len() / channels.max(1) as usize;
                let mut done = 0;
                while done < samples {
                    let n = (samples - done).min(per);
                    w.get_audio(scratch, n, n);
                    done += n;
                }
                samples
            }
        }
    }

    /// `hasEnded()` (`looping` is the voice's `LOOPING` flag).
    #[inline]
    pub(crate) fn has_ended(&self, looping: bool) -> bool {
        match self {
            Instance::Wav { wav, offset } => !looping && *offset >= wav.sample_count,
            Instance::Wind(_) => false,
        }
    }

    /// `rewind()`: Wav can, the wind cannot (`NOT_IMPLEMENTED`).
    pub(crate) fn rewind(&mut self) -> bool {
        match self {
            Instance::Wav { offset, .. } => {
                *offset = 0;
                true
            }
            Instance::Wind(_) => false,
        }
    }
}
