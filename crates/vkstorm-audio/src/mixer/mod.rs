//! Derived from SoLoud (Copyright (c) 2013-2020 Jari Komppa), zlib licence;
//! altered: rewritten in Rust. See LICENSE-soloud in this crate.
//!
//! The sound-effect mixer: a port of the parts of SoLoud 202002
//! (`e82fd32c1f`, with the viewer package's patches) that the viewer's
//! SoLoud adapter exercises, plus the parcel music bus.
//!
//! # What is ported
//! - Voices and handles (`play`, `play3d` paused, `stop`, `seek`, pause,
//!   protect, looping, loop counts, stream position, volume, `fadeVolume`,
//!   `scheduleStop`, inaudible behaviour), handle generations
//!   (`slot + 1 | play_index << 12`), `findFreeVoice` (oldest unprotected
//!   voice is stolen when all 1024 slots are taken).
//! - The active-voice selection with `setMaxActiveVoiceCount(64)`: voices
//!   that must tick (`INAUDIBLE_TICK`) first, then SoLoud's partial
//!   quicksort by overall volume, and the resample-buffer pool assignment
//!   (a voice that gains a buffer starts from zeroed blocks; stale entries of
//!   the active list keep buffers alive exactly as in SoLoud).
//! - `mix`: faders and the stop scheduler in stream time, the 512-sample
//!   source blocks with the linear resampler (SoLoud's default), inaudible
//!   voices that still tick, `panAndExpand` for 1/2/4/6/8 output channels,
//!   the global volume ramp and the `CLIP_ROUNDOFF` clipper with the 0.95
//!   post-clip scaler, and the interleave. [`Mixer::render`] splits a device
//!   period into chunks of at most 1024 frames as the viewer's patched
//!   miniaudio callback did, and adds the music bus.
//! - 3D: listener, source position/velocity, inverse/linear/exponential
//!   distance attenuation with rolloff and min/max distance, Doppler
//!   (factor, speed of sound 343.3), per-speaker panning with SoLoud's
//!   speaker positions; [`Mixer::new`] applies the adapter's 7.1 override
//!   (speakers 4/5 back at (±2, 0, -1), 6/7 side at (±2, 0, 0)).
//! - The adapter's wind voice ([`wind`]) and buffer defaults ([`Wav`]).
//!
//! Not ported (the viewer never uses them): filters, buses, voice groups,
//! visualization, LFO oscillators, play speed and pan faders,
//! `playClocked`, distance delay, colliders and custom attenuators, the
//! point and Catmull-Rom resamplers.
//!
//! # Arithmetic
//! Operation order and precision follow the C++ as MSVC and GCC compile it
//! for x86-64 (SSE, no FMA contraction; Rust never contracts): float vs
//! double per expression, the SSE lanes of the clipper and of the 1->2 and
//! 2->2 pans (see `bus`). The only libm calls are `sqrt` (exact) for the
//! 3D vectors, `cos`/`sin` in double for the 2-D pan of `play`, and the
//! wind's `cosf`/`sqrtf`/`expf`; Rust's `f64::cos`, `f32::cos`, ... call
//! the same C runtime functions (UCRT on Windows, glibc on Linux).
//! SoLoud sets flush-to-zero and denormals-are-zero on its mixing thread;
//! [`Mixer::mix`] does the same for the duration of the call (and restores
//! the caller's MXCSR, where SoLoud leaves it set).
//!
//! # Threading
//! `Mixer` is a plain value; the engine owns it behind one mutex that both
//! the API calls and the device callback take (the callback per chunk of at
//! most 1024 frames), as SoLoud serialises its API and `mix` on its audio
//! mutex. The mix path does not allocate: buffers are preallocated at
//! construction, and sources released by voices that end inside `mix` are
//! parked in a preallocated list and dropped by the next API call.

// The port keeps SoLoud's statement shapes (nested ifs, negated float
// comparisons in the quicksort, indexed loops) so it can be audited against
// the C++ line by line.
#![allow(
    clippy::collapsible_if,
    clippy::neg_cmp_op_on_partial_ord,
    clippy::needless_range_loop,
    clippy::too_many_arguments
)]

pub(crate) mod bus;
mod fader;
mod fpenv;
pub mod music;
mod source;
mod space;
pub mod wind;

use std::sync::Arc;

use bus::{FIXPOINT_FRAC_BITS, FIXPOINT_FRAC_MUL, MAX_CHANNELS, SAMPLE_GRANULARITY};
use fader::Fader;
pub use music::{FormFactor, MusicBus, OutputLayout, TransitionState};
use source::Instance;
pub use source::{Attenuation, VIEWER_FADE_SAMPLES, VIEWER_MAX_AUDIBLE_DISTANCE, Wav, WavError};
use space::Vec3;
pub use wind::{WindGen, WindNoise, WindTargets};

/// A voice handle (`SoLoud::handle`): `(slot + 1) | (play_index << 12)`;
/// 0 is never valid.
pub type Handle = u32;

/// Voice slots (`VOICE_COUNT`).
pub const VOICE_COUNT: usize = 1024;

/// The adapter's active voice limit.
pub const VIEWER_MAX_ACTIVE_VOICES: u32 = 64;

/// The largest chunk [`Mixer::render`] mixes in one `mix` call (the
/// viewer's patched miniaudio callback).
pub const MIX_CHUNK_FRAMES: usize = 1024;

/// Floats added between the planar channel buffers of the mix. SoLoud
/// spaced them by the rounded frame count (1024 floats, 4 KiB, for a
/// 1024-frame mix), which puts every plane at the same offset modulo 4 KiB,
/// where the loops that read and write all output planes per sample can
/// stall on false store-to-load dependencies (4K aliasing). 64 bytes apart
/// they cannot; measured, the mix at 6 and 8 channels is 3-8 % faster. The
/// layout does not change any arithmetic.
const PLANE_PAD: usize = 16;

// AudioSourceInstance::FLAGS
const LOOPING: u32 = 1;
const PROTECTED: u32 = 2;
const PAUSED: u32 = 4;
const PROCESS_3D: u32 = 8;
const LISTENER_RELATIVE: u32 = 16;
const INAUDIBLE: u32 = 32;
const INAUDIBLE_KILL: u32 = 64;
const INAUDIBLE_TICK: u32 = 128;
const DISABLE_AUTOSTOP: u32 = 256;

/// `M_PI` as SoLoud sees it: MSVC's `math.h` has none without
/// `_USE_MATH_DEFINES`, so `soloud.h` defines `3.14159265359`; glibc's is the
/// full constant.
#[cfg(windows)]
#[allow(clippy::approx_constant, clippy::excessive_precision)]
const SOLOUD_M_PI: f64 = 3.14159265359;
#[cfg(not(windows))]
const SOLOUD_M_PI: f64 = std::f64::consts::PI;

/// Mixer errors (SoLoud's `INVALID_PARAMETER`, `NOT_IMPLEMENTED`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixerError {
    InvalidParameter,
    NotImplemented,
}

impl std::fmt::Display for MixerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MixerError::InvalidParameter => f.write_str("invalid parameter"),
            MixerError::NotImplemented => f.write_str("not implemented"),
        }
    }
}

impl std::error::Error for MixerError {}

/// x86 `cvttss2si`/`cvttsd2si`: truncation, `i32::MIN` when out of range or
/// NaN (Rust's `as` saturates instead).
#[inline(always)]
fn cvtt_i32(x: f64) -> i32 {
    if (-2_147_483_648.0..2_147_483_648.0).contains(&x) {
        x as i32
    } else {
        i32::MIN
    }
}

#[derive(Clone, Copy, Debug)]
struct Voice3d {
    position: Vec3,
    velocity: Vec3,
    min_distance: f32,
    max_distance: f32,
    rolloff: f32,
    model: u32,
    doppler_factor: f32,
    doppler_value: f32,
    volume: f32,
    channel_volume: [f32; MAX_CHANNELS],
    flags: u32,
    handle: Handle,
}

impl Default for Voice3d {
    fn default() -> Self {
        Voice3d {
            position: [0.0; 3],
            velocity: [0.0; 3],
            min_distance: 0.0,
            max_distance: 1_000_000.0,
            rolloff: 1.0,
            model: 0,
            doppler_factor: 1.0,
            doppler_value: 0.0,
            volume: 0.0,
            channel_volume: [0.0; MAX_CHANNELS],
            flags: 0,
            handle: 0,
        }
    }
}

const NO_BUFFER: usize = usize::MAX;

struct Voice {
    /// Identity of the instance (SoLoud compares instance pointers).
    id: u64,
    play_index: u32,
    loop_count: u32,
    flags: u32,
    pan: f32,
    channel_volume: [f32; MAX_CHANNELS],
    set_volume: f32,
    overall_volume: f32,
    base_samplerate: f32,
    samplerate: f32,
    channels: u32,
    set_relative_play_speed: f32,
    overall_relative_play_speed: f32,
    stream_time: f64,
    stream_position: f64,
    volume_fader: Fader,
    pause_scheduler: Fader,
    stop_scheduler: Fader,
    active_fader: i32,
    current_channel_volume: [f32; MAX_CHANNELS],
    /// Resample pool buffers (ping-pong), [`NO_BUFFER`] when none.
    resample: [usize; 2],
    src_offset: u32,
    leftover_samples: u32,
    delay_samples: u32,
    loop_point: f64,
    instance: Instance,
}

impl Voice {
    #[inline]
    fn has_ended(&self) -> bool {
        self.instance.has_ended(self.flags & LOOPING != 0)
    }
}

/// What a voice plays, for `play`: SoLoud's `AudioSource` side.
struct SourceAttrs {
    base_samplerate: f32,
    channels: u32,
    volume: f32,
    flags: u32,
    min_distance: f32,
    max_distance: f32,
    rolloff: f32,
    model: u32,
    doppler_factor: f32,
}

/// A source released inside `mix`, dropped later on an API call (never
/// read: it exists to defer the deallocation).
#[allow(dead_code)]
enum Retired {
    Wav(Arc<Wav>),
    Wind(Box<wind::WindInstance>),
}

/// The mixer configuration (what SoLoud's `postinit_internal` receives).
#[derive(Clone, Copy, Debug)]
pub struct MixerConfig {
    pub samplerate: u32,
    /// 1, 2, 4, 6 or 8.
    pub channels: u32,
    /// `CLIP_ROUNDOFF` (the viewer's choice) or the hard clipper.
    pub roundoff_clip: bool,
    /// `setMaxActiveVoiceCount` (the viewer uses 64; below 256).
    pub max_active_voices: u32,
    /// Apply the adapter's 7.1 speaker override for 8 channels.
    pub viewer_speaker_layout: bool,
    /// The first play index (handles of a replaced mixer stay invalid when
    /// the new one continues the count; SoLoud restarts at 0).
    pub first_play_index: u32,
    /// Largest `mix` call in frames (SoLoud's scratch size: the device buffer
    /// rounded to 16, at least 4096).
    pub max_mix_frames: u32,
}

impl MixerConfig {
    /// The viewer's settings for a device format.
    pub fn viewer(samplerate: u32, channels: u32) -> Self {
        MixerConfig {
            samplerate,
            channels,
            roundoff_clip: true,
            max_active_voices: VIEWER_MAX_ACTIVE_VOICES,
            viewer_speaker_layout: true,
            first_play_index: 0,
            max_mix_frames: 4096,
        }
    }
}

/// The mixer: SoLoud's core state plus the music bus.
pub struct Mixer {
    samplerate: u32,
    channels: u32,
    roundoff: bool,
    global_volume: f32,
    post_clip_scaler: f32,
    global_volume_fader: Fader,
    stream_time: f64,
    play_index: u32,
    next_instance_id: u64,
    voices: Vec<Option<Voice>>,
    /// The instance id of each slot (0 = empty): a compact copy for the
    /// resample buffer mapping's identity scan (SoLoud compares a pointer
    /// array).
    voice_ids: Vec<u64>,
    m3d: Vec<Voice3d>,
    highest_voice: u32,
    max_active_voices: u32,
    active_voice: Vec<u32>,
    active_voice_count: u32,
    active_voice_dirty: bool,
    resample_pool: Vec<f32>,
    /// Instance id owning each pool channel (0 = none) and its slot.
    resample_owner: Vec<(u64, u32)>,
    scratch: Vec<f32>,
    output_scratch: Vec<f32>,
    scratch_size: u32,
    listener_position: Vec3,
    listener_at: Vec3,
    listener_up: Vec3,
    listener_velocity: Vec3,
    sound_speed: f32,
    speaker_position: [f32; 3 * MAX_CHANNELS],
    voice_list: Vec<u32>,
    retired: Vec<Retired>,
    /// The parcel music bus, mixed after the effects by [`Mixer::render`].
    pub music: MusicBus,
}

impl Mixer {
    /// `Soloud::init` + `postinit_internal` (+ the adapter's
    /// `setMaxActiveVoiceCount` and 7.1 speaker override when configured).
    /// Global volume starts at 1 as in SoLoud; the engine sets it.
    pub fn new(config: MixerConfig) -> Result<Mixer, MixerError> {
        if !matches!(config.channels, 1 | 2 | 4 | 6 | 8)
            || config.samplerate == 0
            || config.max_active_voices == 0
            || config.max_active_voices >= 256
        {
            return Err(MixerError::InvalidParameter);
        }
        let mut scratch_size = (config.max_mix_frames.min(1 << 20) + 15) & !0xf;
        scratch_size = scratch_size.max((SAMPLE_GRANULARITY * 2) as u32).max(4096);
        let mut m = Mixer {
            samplerate: config.samplerate,
            channels: config.channels,
            roundoff: config.roundoff_clip,
            global_volume: 1.0,
            post_clip_scaler: 0.95,
            global_volume_fader: Fader::default(),
            stream_time: 0.0,
            play_index: config.first_play_index & 0xfffff,
            next_instance_id: 1,
            voices: (0..VOICE_COUNT).map(|_| None).collect(),
            voice_ids: vec![0; VOICE_COUNT],
            m3d: vec![Voice3d::default(); VOICE_COUNT],
            highest_voice: 0,
            max_active_voices: 0,
            active_voice: vec![0; VOICE_COUNT],
            active_voice_count: 0,
            active_voice_dirty: true,
            resample_pool: Vec::new(),
            resample_owner: Vec::new(),
            scratch: vec![0.0; (scratch_size as usize + PLANE_PAD) * MAX_CHANNELS],
            output_scratch: vec![0.0; (scratch_size as usize + PLANE_PAD) * MAX_CHANNELS],
            scratch_size,
            listener_position: [0.0; 3],
            listener_at: [0.0, 0.0, -1.0],
            listener_up: [0.0, 1.0, 0.0],
            listener_velocity: [0.0; 3],
            sound_speed: 343.3,
            speaker_position: [0.0; 3 * MAX_CHANNELS],
            voice_list: Vec::with_capacity(VOICE_COUNT),
            retired: Vec::with_capacity(VOICE_COUNT),
            music: MusicBus::new(config.samplerate),
        };
        m.set_max_active_voice_count(config.max_active_voices)?;
        let defaults: &[[f32; 3]] = match config.channels {
            1 => &[[0.0, 0.0, 1.0]],
            2 => &[[2.0, 0.0, 1.0], [-2.0, 0.0, 1.0]],
            4 => &[
                [2.0, 0.0, 1.0],
                [-2.0, 0.0, 1.0],
                [2.0, 0.0, -1.0],
                [-2.0, 0.0, -1.0],
            ],
            6 => &[
                [2.0, 0.0, 1.0],
                [-2.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 0.0],
                [2.0, 0.0, -1.0],
                [-2.0, 0.0, -1.0],
            ],
            _ => &[
                [2.0, 0.0, 1.0],
                [-2.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [-2.0, 0.0, 0.0],
                [2.0, 0.0, -1.0],
                [-2.0, 0.0, -1.0],
            ],
        };
        for (i, p) in defaults.iter().enumerate() {
            m.speaker_position[i * 3..i * 3 + 3].copy_from_slice(p);
        }
        if config.channels == 8 && config.viewer_speaker_layout {
            // Windows 7.1 order: the back pair before the side pair.
            m.set_speaker_position(4, 2.0, 0.0, -1.0)?;
            m.set_speaker_position(5, -2.0, 0.0, -1.0)?;
            m.set_speaker_position(6, 2.0, 0.0, 0.0)?;
            m.set_speaker_position(7, -2.0, 0.0, 0.0)?;
        }
        m.music
            .set_output(OutputLayout::default_for(config.channels));
        Ok(m)
    }

    pub fn samplerate(&self) -> u32 {
        self.samplerate
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }

    /// The next play index (to continue in a replacement mixer).
    pub fn play_index(&self) -> u32 {
        self.play_index
    }

    /// Drops sources that voices released inside `mix` (call on the API
    /// thread; the mutating API calls do).
    pub fn collect_garbage(&mut self) {
        self.retired.clear();
    }

    // ------------------------------------------------------------------
    // handles

    fn handle_from_voice(&self, slot: usize) -> Handle {
        match &self.voices[slot] {
            None => 0,
            Some(v) => (slot as u32 + 1) | (v.play_index << 12),
        }
    }

    fn voice_from_handle(&self, h: Handle) -> Option<usize> {
        if h == 0 || (h & 0xfffff000) == 0xfffff000 {
            return None;
        }
        let ch = (h & 0xfff) as i32 - 1;
        if ch < 0 || ch as usize >= VOICE_COUNT {
            return None;
        }
        let idx = h >> 12;
        match &self.voices[ch as usize] {
            Some(v) if (v.play_index & 0xfffff) == idx => Some(ch as usize),
            _ => None,
        }
    }

    /// `FOR_ALL_VOICES_PRE_3D`: the slot whose 3D data was last set up for
    /// this handle (SoLoud does not check that the voice still exists).
    fn voice_3d_from_handle(&self, h: Handle) -> Option<usize> {
        let ch = (h & 0xfff) as i32 - 1;
        if ch < 0 || ch as usize >= VOICE_COUNT || self.m3d[ch as usize].handle != h {
            return None;
        }
        Some(ch as usize)
    }

    // ------------------------------------------------------------------
    // internal voice operations (soloud_core_voiceops.cpp)

    fn set_voice_relative_play_speed(&mut self, slot: usize, speed: f32) {
        if speed <= 0.0 {
            return;
        }
        if let Some(v) = self.voices[slot].as_mut() {
            v.set_relative_play_speed = speed;
            self.update_voice_relative_play_speed(slot);
        }
    }

    fn set_voice_pause(&mut self, slot: usize, pause: bool) {
        self.active_voice_dirty = true;
        if let Some(v) = self.voices[slot].as_mut() {
            v.pause_scheduler.active = 0;
            if pause {
                v.flags |= PAUSED;
            } else {
                v.flags &= !PAUSED;
            }
        }
    }

    fn set_voice_pan(&mut self, slot: usize, pan: f32) {
        if let Some(v) = self.voices[slot].as_mut() {
            v.pan = pan;
            let a = (pan + 1.0) as f64 * SOLOUD_M_PI / 4.0;
            let l = a.cos() as f32;
            let r = a.sin() as f32;
            v.channel_volume[0] = l;
            v.channel_volume[1] = r;
            let inv_sqrt2 = 1.0f32 / 2.0f32.sqrt();
            match v.channels {
                4 => {
                    v.channel_volume[2] = l;
                    v.channel_volume[3] = r;
                }
                6 => {
                    v.channel_volume[2] = inv_sqrt2;
                    v.channel_volume[3] = 1.0;
                    v.channel_volume[4] = l;
                    v.channel_volume[5] = r;
                }
                8 => {
                    v.channel_volume[2] = inv_sqrt2;
                    v.channel_volume[3] = 1.0;
                    v.channel_volume[4] = l;
                    v.channel_volume[5] = r;
                    v.channel_volume[6] = l;
                    v.channel_volume[7] = r;
                }
                _ => {}
            }
        }
    }

    fn set_voice_volume(&mut self, slot: usize, volume: f32) {
        self.active_voice_dirty = true;
        if let Some(v) = self.voices[slot].as_mut() {
            v.set_volume = volume;
            self.update_voice_volume(slot);
        }
    }

    fn stop_voice(&mut self, slot: usize) {
        self.active_voice_dirty = true;
        if let Some(v) = self.voices[slot].take() {
            self.voice_ids[slot] = 0;
            for owner in self.resample_owner.iter_mut() {
                if owner.0 == v.id {
                    *owner = (0, 0);
                }
            }
            let retired = match v.instance {
                Instance::Wav { wav, .. } => Retired::Wav(wav),
                Instance::Wind(w) => Retired::Wind(w),
            };
            if self.retired.len() < self.retired.capacity() {
                self.retired.push(retired);
            }
        }
    }

    fn update_voice_relative_play_speed(&mut self, slot: usize) {
        let doppler = self.m3d[slot].doppler_value;
        if let Some(v) = self.voices[slot].as_mut() {
            v.overall_relative_play_speed = doppler * v.set_relative_play_speed;
            v.samplerate = v.base_samplerate * v.overall_relative_play_speed;
        }
    }

    fn update_voice_volume(&mut self, slot: usize) {
        let vol3d = self.m3d[slot].volume;
        if let Some(v) = self.voices[slot].as_mut() {
            v.overall_volume = v.set_volume * vol3d;
            if v.flags & PAUSED != 0 {
                for i in 0..MAX_CHANNELS {
                    v.current_channel_volume[i] = v.channel_volume[i] * v.overall_volume;
                }
            }
        }
    }

    fn find_free_voice(&mut self) -> Option<usize> {
        let mut lowest_play_index_value = u32::MAX;
        let mut lowest_play_index: Option<usize> = None;
        if self.highest_voice > 0 && self.voices[self.highest_voice as usize - 1].is_none() {
            self.highest_voice -= 1;
        }
        for i in 0..VOICE_COUNT {
            match &self.voices[i] {
                None => {
                    if i as u32 + 1 > self.highest_voice {
                        self.highest_voice = i as u32 + 1;
                    }
                    return Some(i);
                }
                Some(v) => {
                    if v.flags & PROTECTED == 0 && v.play_index < lowest_play_index_value {
                        lowest_play_index_value = v.play_index;
                        lowest_play_index = Some(i);
                    }
                }
            }
        }
        // All slots busy: steal the oldest unprotected voice (SoLoud has
        // undefined behaviour when every voice is protected; we fail).
        let slot = lowest_play_index?;
        self.stop_voice(slot);
        Some(slot)
    }

    // ------------------------------------------------------------------
    // playing

    fn play_instance(
        &mut self,
        attrs: SourceAttrs,
        instance: Instance,
        volume: f32,
        pan: f32,
        paused: bool,
    ) -> Handle {
        self.collect_garbage();
        let Some(ch) = self.find_free_voice() else {
            return 0;
        };
        let id = self.next_instance_id;
        self.next_instance_id += 1;
        let mut flags = 0;
        if attrs.flags & 1 != 0 {
            flags |= LOOPING;
        }
        flags |= attrs.flags
            & (PROCESS_3D | LISTENER_RELATIVE | INAUDIBLE_KILL | INAUDIBLE_TICK | DISABLE_AUTOSTOP);
        self.voices[ch] = Some(Voice {
            id,
            play_index: self.play_index,
            loop_count: 0,
            flags,
            pan: 0.0,
            channel_volume: [1.0; MAX_CHANNELS],
            set_volume: 1.0,
            overall_volume: 0.0,
            base_samplerate: attrs.base_samplerate,
            samplerate: attrs.base_samplerate,
            channels: attrs.channels,
            set_relative_play_speed: 1.0,
            overall_relative_play_speed: 1.0,
            stream_time: 0.0,
            stream_position: 0.0,
            volume_fader: Fader::default(),
            pause_scheduler: Fader::default(),
            stop_scheduler: Fader::default(),
            active_fader: 0,
            current_channel_volume: [0.0; MAX_CHANNELS],
            resample: [NO_BUFFER; 2],
            src_offset: 0,
            leftover_samples: 0,
            delay_samples: 0,
            loop_point: 0.0,
            instance,
        });
        self.voice_ids[ch] = id;
        let d = &mut self.m3d[ch];
        d.model = attrs.model;
        d.rolloff = attrs.rolloff;
        d.doppler_factor = attrs.doppler_factor;
        d.max_distance = attrs.max_distance;
        d.min_distance = attrs.min_distance;
        d.volume = 1.0;
        d.doppler_value = 1.0;

        self.play_index += 1;
        if self.play_index == 0xfffff {
            self.play_index = 0;
        }
        if paused {
            if let Some(v) = self.voices[ch].as_mut() {
                v.flags |= PAUSED;
            }
        }
        self.set_voice_pan(ch, pan);
        self.set_voice_volume(ch, if volume < 0.0 { attrs.volume } else { volume });
        if let Some(v) = self.voices[ch].as_mut() {
            for i in 0..MAX_CHANNELS {
                v.current_channel_volume[i] = v.channel_volume[i] * v.overall_volume;
            }
        }
        self.set_voice_relative_play_speed(ch, 1.0);
        self.active_voice_dirty = true;
        self.handle_from_voice(ch)
    }

    fn wav_attrs(wav: &Wav) -> SourceAttrs {
        let mut flags = 0;
        if wav.looping {
            flags |= 1;
        }
        if wav.inaudible_tick {
            flags |= INAUDIBLE_TICK;
        }
        if wav.inaudible_kill {
            flags |= INAUDIBLE_KILL;
        }
        SourceAttrs {
            base_samplerate: wav.base_samplerate(),
            channels: wav.channels(),
            volume: wav.volume,
            flags,
            min_distance: wav.min_distance,
            max_distance: wav.max_distance,
            rolloff: wav.rolloff,
            model: wav.attenuation as u32,
            doppler_factor: wav.doppler_factor,
        }
    }

    /// `Soloud::play(wav, volume, pan, paused)` (a negative volume uses the
    /// source's). Returns 0 when no voice could be found.
    pub fn play(&mut self, wav: &Arc<Wav>, volume: f32, pan: f32, paused: bool) -> Handle {
        let attrs = Self::wav_attrs(wav);
        self.play_instance(
            attrs,
            Instance::Wav {
                wav: Arc::clone(wav),
                offset: 0,
            },
            volume,
            pan,
            paused,
        )
    }

    /// The adapter's wind voice: `play(windSource, volume)` of a stereo
    /// 44.1 kHz single-instance source (another instance of the same
    /// targets is stopped first). The caller protects it.
    pub fn play_wind(
        &mut self,
        targets: &Arc<WindTargets>,
        noise: WindNoise,
        volume: f32,
    ) -> Handle {
        self.stop_wind(targets);
        let attrs = SourceAttrs {
            base_samplerate: 44100.0,
            channels: 2,
            volume: 1.0,
            flags: 0,
            min_distance: 1.0,
            max_distance: 1_000_000.0,
            rolloff: 1.0,
            model: 0,
            doppler_factor: 1.0,
        };
        let inst = Instance::Wind(Box::new(wind::WindInstance::new(
            Arc::clone(targets),
            noise,
        )));
        self.play_instance(attrs, inst, volume, 0.0, false)
    }

    fn stop_wind(&mut self, targets: &Arc<WindTargets>) {
        for i in 0..self.highest_voice as usize {
            let same = matches!(&self.voices[i], Some(Voice { instance: Instance::Wind(w), .. }) if Arc::ptr_eq(&w.targets, targets));
            if same {
                self.stop_voice(i);
            }
        }
        self.collect_garbage();
    }

    /// `Soloud::play3d(wav, pos, vel, volume, paused)`.
    pub fn play_3d(
        &mut self,
        wav: &Arc<Wav>,
        position: Vec3,
        velocity: Vec3,
        volume: f32,
        paused: bool,
    ) -> Handle {
        let h = self.play(wav, volume, 0.0, true);
        let Some(v) = self.voice_from_handle(h) else {
            return h;
        };
        self.m3d[v].handle = h;
        if let Some(voice) = self.voices[v].as_mut() {
            voice.flags |= PROCESS_3D;
        }
        self.set_3d_source_parameters(h, position, velocity);
        self.update_3d_voices(&[v as u32]);
        self.update_voice_relative_play_speed(v);
        let cv = self.m3d[v].channel_volume;
        if let Some(voice) = self.voices[v].as_mut() {
            voice.channel_volume = cv;
        }
        self.update_voice_volume(v);
        let mut kill = false;
        if let Some(voice) = self.voices[v].as_mut() {
            for i in 0..MAX_CHANNELS {
                voice.current_channel_volume[i] = voice.channel_volume[i] * voice.overall_volume;
            }
            if voice.overall_volume < 0.01 {
                voice.flags |= INAUDIBLE;
                kill = voice.flags & INAUDIBLE_KILL != 0;
            } else {
                voice.flags &= !INAUDIBLE;
            }
        }
        if kill {
            self.stop_voice(v);
        }
        self.active_voice_dirty = true;
        self.set_delay_samples(h, 0);
        self.set_pause(h, paused);
        h
    }

    // ------------------------------------------------------------------
    // basic operations

    /// `Soloud::seek`: the generic `AudioSourceInstance::seek` (rewind when
    /// seeking backwards, then discard `floor(samplerate * offset)` frames
    /// at the voice's current, Doppler-shifted, rate).
    pub fn seek(&mut self, h: Handle, seconds: f64) -> Result<(), MixerError> {
        self.collect_garbage();
        let Some(slot) = self.voice_from_handle(h) else {
            return Ok(());
        };
        let scratch_size = self.scratch_size as usize;
        let scratch = &mut self.scratch[..scratch_size * MAX_CHANNELS];
        match self.voices[slot].as_mut() {
            Some(v) => {
                if voice_seek(v, seconds, scratch, scratch_size) {
                    Ok(())
                } else {
                    Err(MixerError::NotImplemented)
                }
            }
            None => Ok(()),
        }
    }

    /// `Soloud::stop`.
    pub fn stop(&mut self, h: Handle) {
        if let Some(slot) = self.voice_from_handle(h) {
            self.stop_voice(slot);
        }
        self.collect_garbage();
    }

    /// `Soloud::stopAll`.
    pub fn stop_all(&mut self) {
        for i in 0..self.highest_voice as usize {
            self.stop_voice(i);
        }
        self.collect_garbage();
    }

    /// `Soloud::stopAudioSource` (what `~Wav` does): stops every voice
    /// playing `wav`.
    pub fn stop_audio_source(&mut self, wav: &Arc<Wav>) {
        for i in 0..self.highest_voice as usize {
            let same = matches!(&self.voices[i], Some(Voice { instance: Instance::Wav { wav: w, .. }, .. }) if Arc::ptr_eq(w, wav));
            if same {
                self.stop_voice(i);
            }
        }
        self.collect_garbage();
    }

    /// `Soloud::countAudioSource`.
    pub fn count_audio_source(&self, wav: &Arc<Wav>) -> u32 {
        (0..self.highest_voice as usize)
            .filter(|&i| matches!(&self.voices[i], Some(Voice { instance: Instance::Wav { wav: w, .. }, .. }) if Arc::ptr_eq(w, wav)))
            .count() as u32
    }

    // ------------------------------------------------------------------
    // setters

    pub fn set_global_volume(&mut self, volume: f32) {
        self.global_volume_fader.active = 0;
        self.global_volume = volume;
    }

    pub fn global_volume(&self) -> f32 {
        self.global_volume
    }

    /// `fadeGlobalVolume` (not used by the viewer).
    pub fn fade_global_volume(&mut self, to: f32, time: f64) {
        let from = self.global_volume;
        if time <= 0.0 || to == from {
            self.set_global_volume(to);
            return;
        }
        self.global_volume_fader
            .set(from, to, time, self.stream_time);
    }

    pub fn set_pause(&mut self, h: Handle, pause: bool) {
        if let Some(slot) = self.voice_from_handle(h) {
            self.set_voice_pause(slot, pause);
        }
    }

    /// `setMaxActiveVoiceCount`: reallocates the resample pool (call
    /// before playing: SoLoud leaves playing voices with dangling buffers;
    /// here they get fresh zeroed buffers on the next mix).
    pub fn set_max_active_voice_count(&mut self, count: u32) -> Result<(), MixerError> {
        if count == 0 || count >= VOICE_COUNT as u32 || count >= 256 {
            return Err(MixerError::InvalidParameter);
        }
        self.max_active_voices = count;
        self.resample_pool = vec![0.0; SAMPLE_GRANULARITY * MAX_CHANNELS * count as usize * 2];
        self.resample_owner = vec![(0, 0); count as usize];
        for v in self.voices.iter_mut().flatten() {
            v.resample = [NO_BUFFER; 2];
        }
        self.active_voice_dirty = true;
        Ok(())
    }

    pub fn max_active_voice_count(&self) -> u32 {
        self.max_active_voices
    }

    pub fn set_protect_voice(&mut self, h: Handle, protect: bool) {
        if let Some(v) = self
            .voice_from_handle(h)
            .and_then(|s| self.voices[s].as_mut())
        {
            if protect {
                v.flags |= PROTECTED;
            } else {
                v.flags &= !PROTECTED;
            }
        }
    }

    pub fn set_pan(&mut self, h: Handle, pan: f32) {
        if let Some(slot) = self.voice_from_handle(h) {
            self.set_voice_pan(slot, pan);
        }
    }

    /// `setInaudibleBehavior(h, must_tick, kill)`.
    pub fn set_inaudible_behavior(&mut self, h: Handle, must_tick: bool, kill: bool) {
        if let Some(v) = self
            .voice_from_handle(h)
            .and_then(|s| self.voices[s].as_mut())
        {
            v.flags &= !(INAUDIBLE_KILL | INAUDIBLE_TICK);
            if must_tick {
                v.flags |= INAUDIBLE_TICK;
            }
            if kill {
                v.flags |= INAUDIBLE_KILL;
            }
        }
    }

    pub fn set_looping(&mut self, h: Handle, looping: bool) {
        if let Some(v) = self
            .voice_from_handle(h)
            .and_then(|s| self.voices[s].as_mut())
        {
            if looping {
                v.flags |= LOOPING;
            } else {
                v.flags &= !LOOPING;
            }
        }
    }

    pub fn set_auto_stop(&mut self, h: Handle, auto_stop: bool) {
        if let Some(v) = self
            .voice_from_handle(h)
            .and_then(|s| self.voices[s].as_mut())
        {
            if auto_stop {
                v.flags &= !DISABLE_AUTOSTOP;
            } else {
                v.flags |= DISABLE_AUTOSTOP;
            }
        }
    }

    /// `Soloud::setVolume` (cancels a running volume fade).
    pub fn set_volume(&mut self, h: Handle, volume: f32) {
        if let Some(slot) = self.voice_from_handle(h) {
            if let Some(v) = self.voices[slot].as_mut() {
                v.volume_fader.active = 0;
            }
            self.set_voice_volume(slot, volume);
        }
    }

    pub fn set_delay_samples(&mut self, h: Handle, samples: u32) {
        if let Some(v) = self
            .voice_from_handle(h)
            .and_then(|s| self.voices[s].as_mut())
        {
            v.delay_samples = samples;
        }
    }

    /// `fadeVolume(h, to, seconds)` in the voice's stream time.
    pub fn fade_volume(&mut self, h: Handle, to: f32, time: f64) {
        let from = self.volume(h);
        if time <= 0.0 || to == from {
            self.set_volume(h, to);
            return;
        }
        if let Some(v) = self
            .voice_from_handle(h)
            .and_then(|s| self.voices[s].as_mut())
        {
            let t = v.stream_time;
            v.volume_fader.set(from, to, time, t);
        }
    }

    /// `schedulePause(h, seconds)`.
    pub fn schedule_pause(&mut self, h: Handle, time: f64) {
        if time <= 0.0 {
            self.set_pause(h, true);
            return;
        }
        if let Some(v) = self
            .voice_from_handle(h)
            .and_then(|s| self.voices[s].as_mut())
        {
            let t = v.stream_time;
            v.pause_scheduler.set(1.0, 0.0, time, t);
        }
    }

    /// `scheduleStop(h, seconds)` in the voice's stream time.
    pub fn schedule_stop(&mut self, h: Handle, time: f64) {
        if time <= 0.0 {
            self.stop(h);
            return;
        }
        if let Some(v) = self
            .voice_from_handle(h)
            .and_then(|s| self.voices[s].as_mut())
        {
            let t = v.stream_time;
            v.stop_scheduler.set(1.0, 0.0, time, t);
        }
    }

    /// `setSpeakerPosition` (fails for a channel the output does not have).
    pub fn set_speaker_position(
        &mut self,
        channel: u32,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<(), MixerError> {
        if channel >= self.channels {
            return Err(MixerError::InvalidParameter);
        }
        let i = channel as usize * 3;
        self.speaker_position[i..i + 3].copy_from_slice(&[x, y, z]);
        Ok(())
    }

    pub fn speaker_position(&self, channel: u32) -> Option<Vec3> {
        if channel >= self.channels {
            return None;
        }
        let i = channel as usize * 3;
        Some([
            self.speaker_position[i],
            self.speaker_position[i + 1],
            self.speaker_position[i + 2],
        ])
    }

    // ------------------------------------------------------------------
    // getters

    /// `isValidVoiceHandle`.
    pub fn is_valid_voice_handle(&self, h: Handle) -> bool {
        self.voice_from_handle(h).is_some()
    }

    fn voice(&self, h: Handle) -> Option<&Voice> {
        self.voice_from_handle(h)
            .and_then(|s| self.voices[s].as_ref())
    }

    /// `getStreamPosition` in seconds (advanced per mix at the voice's play
    /// speed; a looping voice's wrap resets it to the loop point, as SoLoud's
    /// seek does, one source block ahead of playback).
    pub fn stream_position(&self, h: Handle) -> f64 {
        self.voice(h).map_or(0.0, |v| v.stream_position)
    }

    pub fn stream_time(&self, h: Handle) -> f64 {
        self.voice(h).map_or(0.0, |v| v.stream_time)
    }

    pub fn loop_count(&self, h: Handle) -> u32 {
        self.voice(h).map_or(0, |v| v.loop_count)
    }

    /// The set volume (`getVolume`).
    pub fn volume(&self, h: Handle) -> f32 {
        self.voice(h).map_or(0.0, |v| v.set_volume)
    }

    /// Set volume times 3D volume (`getOverallVolume`).
    pub fn overall_volume(&self, h: Handle) -> f32 {
        self.voice(h).map_or(0.0, |v| v.overall_volume)
    }

    pub fn pause(&self, h: Handle) -> bool {
        self.voice(h).is_some_and(|v| v.flags & PAUSED != 0)
    }

    pub fn looping(&self, h: Handle) -> bool {
        self.voice(h).is_some_and(|v| v.flags & LOOPING != 0)
    }

    pub fn protect_voice(&self, h: Handle) -> bool {
        self.voice(h).is_some_and(|v| v.flags & PROTECTED != 0)
    }

    /// The voice's base sample rate (`getSamplerate`).
    pub fn voice_samplerate(&self, h: Handle) -> f32 {
        self.voice(h).map_or(0.0, |v| v.base_samplerate)
    }

    /// `getVoiceCount`.
    pub fn voice_count(&self) -> u32 {
        self.voices[..self.highest_voice as usize]
            .iter()
            .filter(|v| v.is_some())
            .count() as u32
    }

    /// `getActiveVoiceCount` (recomputes the active set when dirty, as
    /// SoLoud does).
    pub fn active_voice_count(&mut self) -> u32 {
        if self.active_voice_dirty {
            self.calc_active_voices();
        }
        self.active_voice_count
    }

    // ------------------------------------------------------------------
    // 3D

    pub fn set_3d_sound_speed(&mut self, speed: f32) -> Result<(), MixerError> {
        if speed <= 0.0 {
            return Err(MixerError::InvalidParameter);
        }
        self.sound_speed = speed;
        Ok(())
    }

    pub fn sound_speed(&self) -> f32 {
        self.sound_speed
    }

    /// `set3dListenerParameters(pos, at, up, velocity)`.
    pub fn set_3d_listener_parameters(
        &mut self,
        position: Vec3,
        at: Vec3,
        up: Vec3,
        velocity: Vec3,
    ) {
        self.listener_position = position;
        self.listener_at = at;
        self.listener_up = up;
        self.listener_velocity = velocity;
    }

    pub fn listener_velocity(&self) -> Vec3 {
        self.listener_velocity
    }

    pub fn set_3d_source_parameters(&mut self, h: Handle, position: Vec3, velocity: Vec3) {
        if let Some(ch) = self.voice_3d_from_handle(h) {
            self.m3d[ch].position = position;
            self.m3d[ch].velocity = velocity;
        }
    }

    pub fn set_3d_source_position(&mut self, h: Handle, position: Vec3) {
        if let Some(ch) = self.voice_3d_from_handle(h) {
            self.m3d[ch].position = position;
        }
    }

    pub fn set_3d_source_velocity(&mut self, h: Handle, velocity: Vec3) {
        if let Some(ch) = self.voice_3d_from_handle(h) {
            self.m3d[ch].velocity = velocity;
        }
    }

    pub fn set_3d_source_min_max_distance(&mut self, h: Handle, min: f32, max: f32) {
        if let Some(ch) = self.voice_3d_from_handle(h) {
            self.m3d[ch].min_distance = min;
            self.m3d[ch].max_distance = max;
        }
    }

    pub fn set_3d_source_attenuation(&mut self, h: Handle, model: Attenuation, rolloff: f32) {
        if let Some(ch) = self.voice_3d_from_handle(h) {
            self.m3d[ch].model = model as u32;
            self.m3d[ch].rolloff = rolloff;
        }
    }

    pub fn set_3d_source_doppler_factor(&mut self, h: Handle, factor: f32) {
        if let Some(ch) = self.voice_3d_from_handle(h) {
            self.m3d[ch].doppler_factor = factor;
        }
    }

    /// `update3dVoices_internal`.
    fn update_3d_voices(&mut self, list: &[u32]) {
        let mut speaker = [[0.0f32; 3]; MAX_CHANNELS];
        for (i, s) in speaker.iter_mut().enumerate().take(self.channels as usize) {
            *s = [
                self.speaker_position[3 * i],
                self.speaker_position[3 * i + 1],
                self.speaker_position[3 * i + 2],
            ];
            space::normalize(s);
        }
        let lpos = self.listener_position;
        let lvel = self.listener_velocity;
        let m = space::Mat3::lookat_rh(&self.listener_at, &self.listener_up);
        let channels = self.channels as usize;
        let sound_speed = self.sound_speed;
        for &slot in list {
            let v = &mut self.m3d[slot as usize];
            let mut vol = 1.0f32;
            let mut pos = v.position;
            let vel = v.velocity;
            if v.flags & LISTENER_RELATIVE == 0 {
                pos = space::sub(&pos, &lpos);
            }
            let dist = space::mag(&pos);
            match Attenuation::from_u32(v.model) {
                Attenuation::InverseDistance => {
                    vol *= space::attenuate_inv_distance(
                        dist,
                        v.min_distance,
                        v.max_distance,
                        v.rolloff,
                    )
                }
                Attenuation::LinearDistance => {
                    vol *= space::attenuate_linear_distance(
                        dist,
                        v.min_distance,
                        v.max_distance,
                        v.rolloff,
                    )
                }
                Attenuation::ExponentialDistance => {
                    vol *= space::attenuate_exponential_distance(
                        dist,
                        v.min_distance,
                        v.max_distance,
                        v.rolloff,
                    )
                }
                Attenuation::None => {}
            }
            v.doppler_value = space::doppler(&pos, &vel, &lvel, v.doppler_factor, sound_speed);
            pos = m.mul(&pos);
            space::normalize(&mut pos);
            for j in 0..MAX_CHANNELS {
                if j < channels {
                    let mut speakervol = (space::dot(&speaker[j], &pos) + 1.0) / 2.0;
                    if space::is_null(&speaker[j]) {
                        speakervol = 1.0;
                    }
                    v.channel_volume[j] = vol * speakervol;
                } else {
                    v.channel_volume[j] = 0.0;
                }
            }
            v.volume = vol;
        }
    }

    /// `update3dAudio`: recomputes attenuation, Doppler and panning of every
    /// 3D voice and marks voices under 0.001 overall volume inaudible.
    pub fn update_3d_audio(&mut self) {
        self.collect_garbage();
        let mut list = std::mem::take(&mut self.voice_list);
        list.clear();
        for i in 0..self.highest_voice as usize {
            if let Some(v) = &self.voices[i] {
                if v.flags & PROCESS_3D != 0 {
                    list.push(i as u32);
                    self.m3d[i].flags = v.flags;
                }
            }
        }
        self.update_3d_voices(&list);
        for &slot in &list {
            let slot = slot as usize;
            if self.voices[slot].is_none() {
                continue;
            }
            self.update_voice_relative_play_speed(slot);
            self.update_voice_volume(slot);
            let cv = self.m3d[slot].channel_volume;
            let mut kill = false;
            if let Some(v) = self.voices[slot].as_mut() {
                v.channel_volume = cv;
                if v.overall_volume < 0.001 {
                    v.flags |= INAUDIBLE;
                    kill = v.flags & INAUDIBLE_KILL != 0;
                } else {
                    v.flags &= !INAUDIBLE;
                }
            }
            if kill {
                self.stop_voice(slot);
            }
        }
        self.voice_list = list;
        self.active_voice_dirty = true;
    }

    // ------------------------------------------------------------------
    // active voices

    fn overall_volume_of_slot(&self, slot: u32) -> f32 {
        self.voices[slot as usize]
            .as_ref()
            .map_or(0.0, |v| v.overall_volume)
    }

    /// `calcActiveVoices_internal`.
    fn calc_active_voices(&mut self) {
        self.active_voice_dirty = false;
        let mut candidates = 0usize;
        let mut mustlive = 0usize;
        for i in 0..self.highest_voice as usize {
            if let Some(v) = &self.voices[i] {
                if v.flags & (INAUDIBLE | PAUSED) == 0 || v.flags & INAUDIBLE_TICK != 0 {
                    self.active_voice[candidates] = i as u32;
                    candidates += 1;
                    if v.flags & INAUDIBLE_TICK != 0 {
                        self.active_voice[candidates - 1] = self.active_voice[mustlive];
                        self.active_voice[mustlive] = i as u32;
                        mustlive += 1;
                    }
                }
            }
        }
        if candidates <= self.max_active_voices as usize {
            self.active_voice_count = candidates as u32;
            self.map_resample_buffers();
            return;
        }
        self.active_voice_count = self.max_active_voices;
        if mustlive >= self.max_active_voices as usize {
            // The ticking voices took every slot. SoLoud returns here without
            // mapping resample buffers, so a voice that just became active
            // reads through a null buffer and the process crashes (the
            // adapter makes every channel voice tick: 65 coexisting channel
            // voices are enough). The buffers are mapped only in that case,
            // which leaves every non-crashing run identical to SoLoud.
            let unmapped = self.active_voice[..self.active_voice_count as usize]
                .iter()
                .any(|&s| {
                    self.voices[s as usize]
                        .as_ref()
                        .is_some_and(|v| v.resample[0] == NO_BUFFER)
                });
            if unmapped {
                self.map_resample_buffers();
            }
            return;
        }

        // SoLoud's iterative partial quicksort (descending overall volume).
        let mut left: i32 = 0;
        let mut stack = [0i32; 24];
        let mut pos: usize = 0;
        let mut len: i32 = (candidates - mustlive) as i32;
        let k = self.active_voice_count as i32;
        let base = mustlive;
        loop {
            while left + 1 < len {
                if pos == 24 {
                    pos = 0;
                    len = stack[0];
                }
                let pivot = self.active_voice[base + left as usize];
                let pivotvol = self.overall_volume_of_slot(pivot);
                stack[pos] = len;
                pos += 1;
                let mut right = left - 1;
                loop {
                    loop {
                        right += 1;
                        if !(self.overall_volume_of_slot(self.active_voice[base + right as usize])
                            > pivotvol)
                        {
                            break;
                        }
                    }
                    loop {
                        len -= 1;
                        if !(pivotvol
                            > self.overall_volume_of_slot(self.active_voice[base + len as usize]))
                        {
                            break;
                        }
                    }
                    if right >= len {
                        break;
                    }
                    self.active_voice
                        .swap(base + right as usize, base + len as usize);
                }
                len += 1;
            }
            if pos == 0 {
                break;
            }
            if left >= k {
                break;
            }
            left = len;
            pos -= 1;
            len = stack[pos];
        }
        self.map_resample_buffers();
    }

    /// `mapResampleBuffers_internal`.
    fn map_resample_buffers(&mut self) {
        let max = self.max_active_voices as usize;
        let mut live = [0u8; 256];
        for i in 0..max {
            let owner = self.resample_owner[i].0;
            if owner == 0 {
                continue;
            }
            for j in 0..max {
                if self.voice_ids[self.active_voice[j] as usize] == owner {
                    live[i] |= 1;
                    live[j] |= 2;
                }
            }
        }
        for i in 0..max {
            let (owner, slot) = self.resample_owner[i];
            if live[i] & 1 == 0 && owner != 0 {
                if let Some(v) = self.voices[slot as usize].as_mut() {
                    if v.id == owner {
                        v.resample = [NO_BUFFER; 2];
                    }
                }
                self.resample_owner[i] = (0, 0);
            }
        }
        let block = SAMPLE_GRANULARITY * MAX_CHANNELS;
        let mut latestfree = 0usize;
        for i in 0..self.active_voice_count as usize {
            let slot = self.active_voice[i] as usize;
            if live[i] & 2 == 0 && self.voices[slot].is_some() {
                let found = (latestfree..max).find(|&j| self.resample_owner[j].0 == 0);
                let Some(found) = found else {
                    // SoLoud asserts here; it cannot happen (the active list
                    // and its stale tail together never exceed the pool).
                    debug_assert!(false, "no free resample buffer");
                    continue;
                };
                let id = self.voices[slot].as_ref().map_or(0, |v| v.id);
                self.resample_owner[found] = (id, slot as u32);
                if let Some(v) = self.voices[slot].as_mut() {
                    v.resample = [found * 2, found * 2 + 1];
                }
                self.resample_pool[found * 2 * block..(found * 2 + 2) * block].fill(0.0);
                latestfree = found + 1;
            }
        }
    }

    // ------------------------------------------------------------------
    // mixing

    /// `Soloud::mix`: `frames` interleaved frames of `channels` samples into
    /// `out` (at most the scratch size). Sets FTZ/DAZ for the duration of
    /// the call, as SoLoud's mixing thread runs.
    pub fn mix(&mut self, out: &mut [f32], frames: usize) {
        let frames = frames.min(self.scratch_size as usize);
        let _ftz = fpenv::DenormalsAreZero::new();
        let stride = (frames + 15) & !0xf;
        let plane = stride + PLANE_PAD;
        self.mix_internal(frames, stride, plane);
        let ch = self.channels as usize;
        for j in 0..ch {
            let src = &self.scratch[j * plane..j * plane + frames];
            for (i, s) in src.iter().enumerate() {
                out[i * ch + j] = *s;
            }
        }
    }

    /// What the device callback does: the viewer package's bounded mixing
    /// (chunks of at most 1024 frames), then the music bus on top.
    pub fn render(&mut self, out: &mut [f32], frames: usize) {
        let ch = self.channels as usize;
        let mut done = 0usize;
        while done < frames {
            let n = (frames - done).min(MIX_CHUNK_FRAMES);
            let chunk = &mut out[done * ch..(done + n) * ch];
            self.mix(chunk, n);
            self.music.mix_into(chunk, n, ch);
            done += n;
        }
    }

    /// `stride` is SoLoud's (the frame count rounded up to 16, which the
    /// clipper's volume ramp divides by); `plane` the distance between the
    /// planar channel buffers (see [`PLANE_PAD`]).
    fn mix_internal(&mut self, samples: usize, stride: usize, plane: usize) {
        let buffertime = samples as f32 / self.samplerate as f32;
        let mut global_volume = [0.0f32; 2];
        self.stream_time += buffertime as f64;
        global_volume[0] = self.global_volume;
        if self.global_volume_fader.active != 0 {
            self.global_volume = self.global_volume_fader.get(self.stream_time);
        }
        global_volume[1] = self.global_volume;
        let global_fader = self.global_volume_fader.active > 0;

        for i in 0..self.highest_voice as usize {
            let mut stop = false;
            let mut pause = false;
            let mut volume_changed = false;
            let Some(v) = self.voices[i].as_mut() else {
                continue;
            };
            if v.flags & PAUSED != 0 {
                continue;
            }
            v.active_fader = 0;
            if global_fader {
                v.active_fader = 1;
            }
            v.stream_time += buffertime as f64;
            v.stream_position += buffertime as f64 * v.overall_relative_play_speed as f64;
            if v.volume_fader.active > 0 {
                v.set_volume = v.volume_fader.get(v.stream_time);
                v.active_fader = 1;
                volume_changed = true;
            }
            if v.pause_scheduler.active != 0 {
                v.pause_scheduler.get(v.stream_time);
                if v.pause_scheduler.active == -1 {
                    v.pause_scheduler.active = 0;
                    pause = true;
                }
            }
            if v.stop_scheduler.active != 0 {
                v.stop_scheduler.get(v.stream_time);
                if v.stop_scheduler.active == -1 {
                    v.stop_scheduler.active = 0;
                    stop = true;
                }
            }
            // The order of SoLoud's loop body: volume update (the voice is
            // not paused yet), then the pause and stop schedulers.
            if volume_changed {
                self.update_voice_volume(i);
                self.active_voice_dirty = true;
            }
            if pause {
                self.set_voice_pause(i, true);
            }
            if stop {
                self.stop_voice(i);
            }
        }

        if self.active_voice_dirty {
            self.calc_active_voices();
        }

        self.mix_bus(samples, plane);

        bus::clip(
            &self.output_scratch,
            &mut self.scratch,
            stride,
            plane,
            global_volume[0],
            global_volume[1],
            self.channels as usize,
            self.roundoff,
            self.post_clip_scaler,
        );
    }

    /// `mixBus_internal` for the root bus.
    fn mix_bus(&mut self, samples: usize, bs: usize) {
        let channels = self.channels;
        let samplerate = self.samplerate as f32;
        for j in 0..channels as usize {
            self.output_scratch[j * bs..j * bs + samples].fill(0.0);
        }
        let block = SAMPLE_GRANULARITY * MAX_CHANNELS;
        let g = SAMPLE_GRANULARITY as u32;
        let gfix = g * FIXPOINT_FRAC_MUL;
        let scratch_size = self.scratch_size as usize;

        for i in 0..self.active_voice_count as usize {
            let slot = self.active_voice[i] as usize;
            let Some(voice) = self.voices[slot].as_mut() else {
                continue;
            };
            let audible = voice.flags & PAUSED == 0 && voice.flags & INAUDIBLE == 0;
            let tick = voice.flags & PAUSED == 0
                && voice.flags & INAUDIBLE != 0
                && voice.flags & INAUDIBLE_TICK != 0;
            if !audible && !tick {
                continue;
            }
            if voice.resample[0] == NO_BUFFER {
                // Cannot happen (see calc_active_voices); never read through
                // a missing buffer.
                continue;
            }
            let mut step = voice.samplerate / samplerate;
            // Only the audible branch guards against step overflow.
            if audible && step > (1u32 << (32 - FIXPOINT_FRAC_BITS)) as f32 {
                step = 0.0;
            }
            let step_fixed = cvtt_i32((step * FIXPOINT_FRAC_MUL as f32).floor() as f64) as u32;
            let mut outofs = 0usize;
            if voice.delay_samples != 0 {
                if voice.delay_samples as usize > samples {
                    outofs = samples;
                    voice.delay_samples -= samples as u32;
                } else {
                    outofs = voice.delay_samples as usize;
                    voice.delay_samples = 0;
                }
                if audible {
                    for k in 0..voice.channels as usize {
                        self.scratch[k * bs..k * bs + outofs].fill(0.0);
                    }
                }
            }

            while step_fixed != 0 && outofs < samples {
                if voice.leftover_samples == 0 {
                    voice.resample.swap(0, 1);
                    let r0 = voice.resample[0];
                    let mut readcount = 0usize;
                    let looping = voice.flags & LOOPING != 0;
                    if !voice.has_ended() || looping {
                        let buf = &mut self.resample_pool[r0 * block..(r0 + 1) * block];
                        readcount = voice.instance.get_audio(
                            voice.channels,
                            buf,
                            SAMPLE_GRANULARITY,
                            SAMPLE_GRANULARITY,
                        );
                        if readcount < SAMPLE_GRANULARITY && looping {
                            let loop_point = voice.loop_point;
                            while readcount < SAMPLE_GRANULARITY
                                && voice_seek(
                                    voice,
                                    loop_point,
                                    &mut self.scratch[..scratch_size * MAX_CHANNELS],
                                    scratch_size,
                                )
                            {
                                voice.loop_count += 1;
                                let buf = &mut self.resample_pool
                                    [r0 * block + readcount..(r0 + 1) * block];
                                let inc = voice.instance.get_audio(
                                    voice.channels,
                                    buf,
                                    SAMPLE_GRANULARITY - readcount,
                                    SAMPLE_GRANULARITY,
                                );
                                readcount += inc;
                                // The ticking branch has no such guard.
                                if audible && inc == 0 {
                                    break;
                                }
                            }
                        }
                    }
                    if audible && readcount < SAMPLE_GRANULARITY {
                        for k in 0..voice.channels as usize {
                            let b = r0 * block + SAMPLE_GRANULARITY * k;
                            self.resample_pool[b + readcount..b + SAMPLE_GRANULARITY].fill(0.0);
                        }
                    }
                    if voice.src_offset < gfix {
                        voice.src_offset = 0;
                    } else {
                        voice.src_offset -= gfix;
                    }
                } else {
                    voice.leftover_samples = 0;
                }

                let mut writesamples: u32 = 0;
                if voice.src_offset < gfix {
                    writesamples = (gfix - voice.src_offset) / step_fixed + 1;
                    if (writesamples
                        .wrapping_mul(step_fixed)
                        .wrapping_add(voice.src_offset)
                        >> FIXPOINT_FRAC_BITS)
                        >= g
                    {
                        writesamples -= 1;
                    }
                }
                if writesamples as usize + outofs > samples {
                    voice.leftover_samples = (writesamples as usize + outofs - samples) as u32;
                    writesamples = (samples - outofs) as u32;
                }
                if audible && writesamples != 0 {
                    let r0 = voice.resample[0];
                    let r1 = voice.resample[1];
                    for j in 0..voice.channels as usize {
                        let s0 = r0 * block + SAMPLE_GRANULARITY * j;
                        let s1 = r1 * block + SAMPLE_GRANULARITY * j;
                        let dst = &mut self.scratch
                            [bs * j + outofs..bs * j + outofs + writesamples as usize];
                        bus::resample_linear(
                            &self.resample_pool[s0..s0 + SAMPLE_GRANULARITY],
                            &self.resample_pool[s1..s1 + SAMPLE_GRANULARITY],
                            dst,
                            voice.src_offset,
                            writesamples as usize,
                            step_fixed,
                        );
                    }
                }
                outofs += writesamples as usize;
                voice.src_offset = voice
                    .src_offset
                    .wrapping_add(writesamples.wrapping_mul(step_fixed));
            }

            if audible {
                bus::pan_and_expand(
                    voice.channels,
                    &mut voice.current_channel_volume,
                    &voice.channel_volume,
                    voice.overall_volume,
                    &mut self.output_scratch,
                    samples,
                    bs,
                    &self.scratch,
                    channels,
                );
            }

            if voice.flags & (LOOPING | DISABLE_AUTOSTOP) == 0 && voice.has_ended() {
                self.stop_voice(slot);
            }
        }
    }
}

/// `AudioSourceInstance::seek` (the generic one; `Wav` and the wind do not
/// override it): rewind when seeking backwards (the wind cannot), then
/// discard `floor(samplerate * offset)` frames in scratch-sized pieces.
/// Returns false for `NOT_IMPLEMENTED`.
fn voice_seek(v: &mut Voice, seconds: f64, scratch: &mut [f32], scratch_size: usize) -> bool {
    let mut offset = seconds - v.stream_position;
    if offset <= 0.0 {
        if !v.instance.rewind() {
            return false;
        }
        v.stream_position = 0.0;
        offset = seconds;
    }
    let mut to_discard = cvtt_i32((v.samplerate as f64 * offset).floor()) as i64;
    let per = (scratch_size / v.channels.max(1) as usize) as i64;
    while to_discard != 0 {
        let samples = if per > to_discard { to_discard } else { per };
        // A negative count (an overflowed conversion) becomes a huge
        // unsigned request in the C: everything left is consumed.
        let n = if samples < 0 {
            usize::MAX / 2
        } else {
            samples as usize
        };
        v.instance.discard(n, scratch, v.channels);
        to_discard -= samples;
    }
    v.stream_position = seconds;
    true
}
