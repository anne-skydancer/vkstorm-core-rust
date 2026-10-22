//! Wind: the viewer's `LLWindGen<F32>` (`indra/llaudio/llwindgen.h`) and the
//! SoLoud adapter's wind source (`LLSoLoudWindSource`/`LLSoLoudWindInstance`
//! in `llaudioengine_soloud.cpp`).
//!
//! The generator is pink-filtered white noise through a resonant low-pass
//! whose centre frequency, gain and right-pan gain glide towards targets
//! (`0.999 * current + 0.001 * target` per synthesized sample). It
//! synthesizes at half the rate (`mSubSamples = 2`) and linearly
//! interpolates. Float operation order and constants are the C++'s; the
//! libm calls are `cosf`, `sqrtf` and `expf`, which Rust's `f32::cos`,
//! `f32::sqrt` and `f32::exp` reach in the same C runtime (UCRT, glibc).
//!
//! The adapter's source: stereo at the generator's 44.1 kHz, rendered in
//! chunks of at most 512 frames into a 1024-float interleaved scratch and
//! deinterleaved into SoLoud's planar buffer; the targets cross from the
//! viewer's thread as relaxed atomics read once per `getAudio`.
//!
//! The noise: the C++ calls `ll_frand() - 0.5f`, a thread-local lagged
//! Fibonacci generator seeded randomly per thread, so its sequence is not
//! reproducible anyway. [`WindNoise`] is a small seedable uniform generator
//! (xorshift64*, 24-bit floats in [0, 1)); the parity oracle uses the same
//! generator in C so the comparison is deterministic.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// `F_TWO_PI` from `llmath.h` (a float constant).
const F_TWO_PI: f32 = std::f32::consts::TAU;

/// Uniform noise in [0, 1): xorshift64* with the top 24 bits as the
/// mantissa (exactly representable floats).
#[derive(Clone, Debug)]
pub struct WindNoise {
    state: u64,
}

impl WindNoise {
    /// A generator from a seed (0 is replaced by a fixed non-zero value:
    /// xorshift has an all-zero fixed point).
    pub fn new(seed: u64) -> Self {
        WindNoise {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    /// A generator seeded from the clock and the process (the viewer's
    /// `ll_frand` is randomly seeded per thread too).
    pub fn from_entropy() -> Self {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        WindNoise::new(h.finish())
    }

    /// The next value in [0, 1).
    #[inline(always)]
    pub fn next_f32(&mut self) -> f32 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        let r = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (r >> 40) as f32 * (1.0 / 16_777_216.0)
    }
}

/// `LLWindGen<F32>`.
#[derive(Clone, Debug)]
pub struct WindGen {
    pub target_gain: f32,
    pub target_freq: f32,
    pub target_pan_gain_r: f32,
    input_sampling_rate: u32,
    sub_samples: u8,
    sample_period: f32,
    filter_band_width: f32,
    b2: f32,
    buf0: f32,
    buf1: f32,
    buf2: f32,
    y0: f32,
    y1: f32,
    current_gain: f32,
    current_freq: f32,
    current_pan_gain_r: f32,
    last_sample: f32,
    noise: WindNoise,
}

impl WindGen {
    /// `LLWindGen(sample_rate)` with its noise source.
    pub fn new(sample_rate: u32, noise: WindNoise) -> Self {
        let sub_samples: u8 = 2;
        let sample_period = sub_samples as f32 / sample_rate as f32;
        let filter_band_width = 50.0f32;
        let b2 = (-F_TWO_PI * filter_band_width * sample_period).exp();
        WindGen {
            target_gain: 0.0,
            target_freq: 100.0,
            target_pan_gain_r: 0.5,
            input_sampling_rate: sample_rate,
            sub_samples,
            sample_period,
            filter_band_width,
            b2,
            buf0: 0.0,
            buf1: 0.0,
            buf2: 0.0,
            y0: 0.0,
            y1: 0.0,
            current_gain: 0.0,
            current_freq: 100.0,
            current_pan_gain_r: 0.5,
            last_sample: 0.0,
            noise,
        }
    }

    pub fn input_sampling_rate(&self) -> u32 {
        self.input_sampling_rate
    }

    /// The filter band width (fixed 50 Hz; kept as a field like the C++).
    pub fn filter_band_width(&self) -> f32 {
        self.filter_band_width
    }

    #[inline(always)]
    fn coefficients(&self) -> (f32, f32) {
        let b1 = (-4.0f32 * self.b2) / (1.0f32 + self.b2)
            * (F_TWO_PI * (self.current_freq * self.sample_period)).cos();
        let a0 = (1.0f32 - self.b2) * (1.0f32 - (b1 * b1) / (4.0f32 * self.b2)).sqrt();
        (a0, b1)
    }

    /// `windGenerate`: `numsamples` interleaved stereo frames into `out`
    /// (at least `2 * numsamples` floats).
    pub fn generate(&mut self, out: &mut [f32], numsamples: usize) {
        let mut numsamples = numsamples;
        let mut cursor = 0usize;
        let mut a0 = 0.0f32;
        let mut b1 = 0.0f32;
        // getClampedSample<F32> never clamps; `clip` is computed but unused.
        let mut interp_freq = false;
        // llabs(F32) vs (F32 * double): the comparison is in double.
        if ((self.target_freq - self.current_freq).abs() as f64)
            < (self.current_freq as f64 * 0.112)
        {
            self.current_freq = self.target_freq;
            (a0, b1) = self.coefficients();
        } else {
            interp_freq = true;
        }

        while numsamples != 0 {
            let mut next_sample = self.noise.next_f32() - 0.5f32;

            self.buf0 = self.buf0 * 0.99765f32 + next_sample * 0.0990460f32;
            self.buf1 = self.buf1 * 0.96300f32 + next_sample * 0.2965164f32;
            self.buf2 = self.buf2 * 0.57000f32 + next_sample * 1.0526913f32;

            next_sample = self.buf0 + self.buf1 + self.buf2 + next_sample * 0.1848f32;

            if interp_freq {
                self.current_freq = (0.999f32 * self.current_freq) + (0.001f32 * self.target_freq);
                (a0, b1) = self.coefficients();
            }

            next_sample = a0 * next_sample - b1 * self.y0 - self.b2 * self.y1;
            self.y1 = self.y0;
            self.y0 = next_sample;

            self.current_gain = (0.999f32 * self.current_gain) + (0.001f32 * self.target_gain);
            self.current_pan_gain_r =
                (0.999f32 * self.current_pan_gain_r) + (0.001f32 * self.target_pan_gain_r);

            next_sample *= self.current_gain;

            let delta = (next_sample - self.last_sample) / self.sub_samples as f32;

            let mut i = self.sub_samples;
            while i != 0 && numsamples != 0 {
                self.last_sample += delta;
                let sample_right = self.last_sample * self.current_pan_gain_r;
                let sample_left = self.last_sample - sample_right;
                out[cursor] = sample_left;
                out[cursor + 1] = sample_right;
                cursor += 2;
                i -= 1;
                numsamples -= 1;
            }
        }
    }
}

/// The wind targets the viewer's thread sets (`setTargets`); relaxed
/// atomics like the adapter's `std::atomic<F32>`.
#[derive(Debug)]
pub struct WindTargets {
    frequency: AtomicU32,
    gain: AtomicU32,
    pan: AtomicU32,
}

impl Default for WindTargets {
    fn default() -> Self {
        WindTargets {
            frequency: AtomicU32::new(100.0f32.to_bits()),
            gain: AtomicU32::new(0.0f32.to_bits()),
            pan: AtomicU32::new(0.5f32.to_bits()),
        }
    }
}

impl WindTargets {
    pub fn new() -> Arc<Self> {
        Arc::new(WindTargets::default())
    }

    /// `LLSoLoudWindSource::setTargets(freq, gain, pan_gain_r)`.
    pub fn set(&self, freq: f32, gain: f32, pan_gain_r: f32) {
        self.frequency.store(freq.to_bits(), Ordering::Relaxed);
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
        self.pan.store(pan_gain_r.to_bits(), Ordering::Relaxed);
    }

    pub fn get(&self) -> (f32, f32, f32) {
        (
            f32::from_bits(self.frequency.load(Ordering::Relaxed)),
            f32::from_bits(self.gain.load(Ordering::Relaxed)),
            f32::from_bits(self.pan.load(Ordering::Relaxed)),
        )
    }
}

/// The wind voice's instance state: the generator and its bounded scratch.
pub(crate) struct WindInstance {
    pub(crate) targets: Arc<WindTargets>,
    generator: WindGen,
    scratch: [f32; 1024],
}

impl WindInstance {
    pub(crate) fn new(targets: Arc<WindTargets>, noise: WindNoise) -> Self {
        WindInstance {
            targets,
            generator: WindGen::new(44100, noise),
            scratch: [0.0; 1024],
        }
    }

    /// `LLSoLoudWindInstance::getAudio`: planar stereo, channel 1 at
    /// `buffer_size`.
    pub(crate) fn get_audio(
        &mut self,
        buffer: &mut [f32],
        samples_to_read: usize,
        buffer_size: usize,
    ) -> usize {
        let (freq, gain, pan) = self.targets.get();
        self.generator.target_freq = freq;
        self.generator.target_gain = gain;
        self.generator.target_pan_gain_r = pan;
        let mut offset = 0usize;
        while offset < samples_to_read {
            let frames = (samples_to_read - offset).min(512);
            self.generator.generate(&mut self.scratch, frames);
            for frame in 0..frames {
                buffer[offset + frame] = self.scratch[frame * 2];
                buffer[buffer_size + offset + frame] = self.scratch[frame * 2 + 1];
            }
            offset += frames;
        }
        samples_to_read
    }
}
