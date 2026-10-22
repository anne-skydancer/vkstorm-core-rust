//! The music bus: parcel music mixed into the device layout after the
//! effects (the effects' global volume does not apply; music has its own
//! gain), with the behaviour of the viewer's VLC `speakerfill` filter
//! (`indra/media_plugins/libvlc/vlc_speakerfill.cpp`) and its output-side
//! layout negotiation (`prepare_speakerfill.cmake`):
//!
//! - **Gain**: the user volume through VLC's cubic curve (`v * v * v`),
//!   reached by a per-sample linear ramp over `rate / 20` frames (50 ms)
//!   from the current gain; a newly attached source starts at the target
//!   without a ramp. **Hard mute** zeroes samples independently of gain and
//!   envelope.
//! - **Envelope**: [`MusicBus::transition`] ramps per sample from the
//!   current envelope value to a target over `round(seconds * rate)`
//!   frames; [`MusicBus::set_envelope`] sets the value before the first
//!   sample (the login hold at 0, fade-ins from exactly 0). Gain changes
//!   never restart it. The frame that first carries the target is the
//!   *final target sample*; its absolute mixer frame index is recorded, and
//!   the transition is [`TransitionState::Complete`] once the device clock
//!   (frames played) has passed it.
//! - **Speaker fill** (Windows and Linux): stereo input only; left/right
//!   feed every main pair the fill layout includes (front, back or side) at
//!   0.70710678, the centre gets their average (`(L+R) x 0.3536`), LFE is
//!   silent. Layout selection from `FSMusicSpatialSound` 0/21/41/51/71:
//!   2.1 = FL FR LFE; 4.1 and 5.1 add the back pair when the device has it,
//!   else the side pair; 5.1 adds the centre; 7.1 is the full 7.1 mask. The
//!   requested mask must be a subset of the device mask; headphones/headset
//!   endpoints and endpoints whose form factor is unknown get no fill (the
//!   VLC output bypassed fill when the property query failed). The setting
//!   applies at the next sample.
//! - Without fill: stereo plays on FL/FR at unity; mono input is played as
//!   dual mono (L = R), which is what VLC's channel mixer did for a mono
//!   stream on a stereo endpoint; native multichannel input passes through
//!   by mask (absent back/side pairs fall back to the other pair, then to
//!   the fronts at 0.7071; an absent centre goes to FL/FR at 0.7071; an
//!   absent LFE is dropped).
//! - **No time stretching**: frames are pulled at the device rate; a short
//!   pull is silence for the rest of the period, and input, output and
//!   underrun frames are counted.
//!
//! Everything here runs inside [`super::Mixer::render`] under the engine's
//! mutex; nothing allocates while mixing (the pull buffer is preallocated).

#![forbid(unsafe_code)]
#![allow(clippy::collapsible_if)]

use crate::music::{
    ChannelLayout, MusicSource, SPEAKER_BACK_LEFT, SPEAKER_BACK_RIGHT, SPEAKER_FRONT_CENTER,
    SPEAKER_FRONT_LEFT, SPEAKER_FRONT_RIGHT, SPEAKER_LOW_FREQUENCY, SPEAKER_SIDE_LEFT,
    SPEAKER_SIDE_RIGHT,
};

/// `KSAUDIO_SPEAKER_7POINT1_SURROUND`.
pub const MASK_7POINT1_SURROUND: u32 = 0x63F;
/// `KSAUDIO_SPEAKER_5POINT1` (back pair).
pub const MASK_5POINT1: u32 = 0x3F;
/// `KSAUDIO_SPEAKER_5POINT1_SURROUND` (side pair).
pub const MASK_5POINT1_SURROUND: u32 = 0x60F;
/// `KSAUDIO_SPEAKER_QUAD`.
pub const MASK_QUAD: u32 = 0x33;

/// VLC's `.70710678f` (the same float as `FRAC_1_SQRT_2`).
const HALF_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
const MAX_CH: usize = 8;

/// The endpoint form factor (`PKEY_AudioEndpoint_FormFactor` on Windows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormFactor {
    /// Speakers or any other non-headphone endpoint.
    Speakers,
    /// Headphones or a headset: no speaker fill.
    Headphones,
    /// The query failed: no speaker fill (as the VLC output did).
    Unknown,
}

/// The device's channel layout as the music bus sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputLayout {
    pub channels: u32,
    /// WAVEFORMATEXTENSIBLE mask; 0 when unknown (the default layout for the
    /// channel count is assumed for routing, and no fill happens).
    pub mask: u32,
    pub form_factor: FormFactor,
}

/// The default `dwChannelMask` for a channel count (as Windows assumes).
pub fn default_mask(channels: u32) -> u32 {
    match channels {
        1 => SPEAKER_FRONT_CENTER,
        2 => SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT,
        3 => SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT | SPEAKER_FRONT_CENTER,
        4 => MASK_QUAD,
        5 => MASK_QUAD | SPEAKER_FRONT_CENTER,
        6 => MASK_5POINT1,
        7 => MASK_5POINT1 | 0x100,
        8 => MASK_7POINT1_SURROUND,
        _ => 0,
    }
}

impl OutputLayout {
    /// The default layout for a channel count, form factor unknown.
    pub fn default_for(channels: u32) -> Self {
        OutputLayout {
            channels,
            mask: 0,
            form_factor: FormFactor::Unknown,
        }
    }

    fn effective_mask(&self) -> u32 {
        if self.mask != 0 && self.mask.count_ones() == self.channels {
            self.mask
        } else {
            default_mask(self.channels)
        }
    }
}

/// The `FSMusicSpatialSound` values.
pub const SPATIAL_LAYOUTS: [i32; 5] = [0, 21, 41, 51, 71];

/// The speaker-fill mask for a setting, device mask and form factor: the
/// requested layout when it is a subset of the device's, else 0 (no fill).
pub fn fill_mask(setting: i32, device_mask: u32, form_factor: FormFactor) -> u32 {
    if form_factor != FormFactor::Speakers || device_mask == 0 {
        return 0;
    }
    let mut requested = SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT | SPEAKER_LOW_FREQUENCY;
    match setting {
        21 => {}
        41 | 51 => {
            let back = SPEAKER_BACK_LEFT | SPEAKER_BACK_RIGHT;
            requested |= if device_mask & back == back {
                back
            } else {
                SPEAKER_SIDE_LEFT | SPEAKER_SIDE_RIGHT
            };
            if setting == 51 {
                requested |= SPEAKER_FRONT_CENTER;
            }
        }
        71 => requested = MASK_7POINT1_SURROUND,
        _ => return 0,
    }
    if requested & device_mask == requested {
        requested
    } else {
        0
    }
}

/// A transition's progress for [`MusicBus::transition_state`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionState {
    /// No transition with that serial (none issued, replaced, or cancelled
    /// by detaching the source).
    Idle,
    /// Issued; the final target sample has not been played yet.
    Pending,
    /// The final target sample has passed through the device clock.
    Complete,
}

#[derive(Clone, Copy, Debug)]
struct Command {
    serial: u32,
    origin: f32,
    destination: f32,
    position: u64,
    duration: u64,
    /// Absolute mixer frame index of the final target sample, once mixed.
    fence: Option<u64>,
}

/// One routing term: `out[out_index] += in[in_index] * coef * factor`.
#[derive(Clone, Copy, Debug, Default)]
struct Route {
    out_index: u8,
    in_index: u8,
    coef: f32,
}

/// The music bus. See the module documentation.
pub struct MusicBus {
    samplerate: u32,
    source: Option<Box<dyn MusicSource>>,
    ended: bool,
    pull: Vec<f32>,
    // gain (cubic user volume, 50 ms ramp)
    gain: f32,
    target: f32,
    step: f32,
    remaining: u32,
    muted: bool,
    // envelope
    envelope: f32,
    command: Option<Command>,
    // layout
    output: OutputLayout,
    spatial: i32,
    fill: u32,
    routes: [Route; MAX_CH * MAX_CH],
    route_count: usize,
    route_key: Option<(u16, u32, u32, u32)>,
    // clock and counters
    frames_mixed: u64,
    input_frames: u64,
    output_frames: u64,
    underrun_frames: u64,
}

impl MusicBus {
    /// A bus for a device at `samplerate` (the mixer owns one; the engine
    /// moves it between mixers across restarts).
    pub fn new(samplerate: u32) -> Self {
        MusicBus {
            samplerate,
            source: None,
            ended: false,
            pull: vec![0.0; super::MIX_CHUNK_FRAMES * MAX_CH],
            gain: 1.0,
            target: 1.0,
            step: 0.0,
            remaining: 0,
            muted: false,
            envelope: 1.0,
            command: None,
            output: OutputLayout::default_for(2),
            spatial: 0,
            fill: 0,
            routes: [Route::default(); MAX_CH * MAX_CH],
            route_count: 0,
            route_key: None,
            frames_mixed: 0,
            input_frames: 0,
            output_frames: 0,
            underrun_frames: 0,
        }
    }

    // ------------------------------------------------------------------
    // source

    /// Attaches a source (replacing and returning any previous one). The
    /// gain starts at its target without a ramp; the envelope keeps its
    /// value (set it first with [`MusicBus::set_envelope`]).
    pub fn attach(&mut self, source: Box<dyn MusicSource>) -> Option<Box<dyn MusicSource>> {
        let old = self.detach();
        self.source = Some(source);
        self.ended = false;
        self.gain = self.target;
        self.remaining = 0;
        old
    }

    /// Detaches the source (call on the API thread: dropping it may join
    /// decoder threads). Like VLC's flush, a pending transition is cancelled.
    pub fn detach(&mut self) -> Option<Box<dyn MusicSource>> {
        self.command = None;
        self.ended = false;
        self.source.take()
    }

    pub fn has_source(&self) -> bool {
        self.source.is_some()
    }

    /// True once the attached source reported its end.
    pub fn is_ended(&self) -> bool {
        self.ended
    }

    // ------------------------------------------------------------------
    // gain and mute

    /// The user music volume (0..1), applied through the cubic curve with a
    /// 50 ms ramp from the current gain. Rejects values outside 0..1.
    pub fn set_volume(&mut self, volume: f32) -> bool {
        if !(volume.is_finite() && (0.0..=1.0).contains(&volume)) {
            return false;
        }
        let requested = volume * volume * volume;
        if requested != self.target {
            self.target = requested;
            let mut remaining = self.samplerate / 20;
            if remaining == 0 {
                remaining = 1;
            }
            self.remaining = remaining;
            self.step = (self.target - self.gain) / remaining as f32;
        }
        true
    }

    /// The current (ramping) linear gain.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// The linear gain the ramp is heading to (`volume^3`).
    pub fn target_gain(&self) -> f32 {
        self.target
    }

    /// Hard mute: zero output regardless of gain and envelope.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    // ------------------------------------------------------------------
    // envelope

    /// Sets the envelope value now (0..1) and cancels any transition: the
    /// login hold at 0, and the starting point of a fade-in.
    pub fn set_envelope(&mut self, value: f32) -> bool {
        if !(value.is_finite() && (0.0..=1.0).contains(&value)) {
            return false;
        }
        self.envelope = value;
        self.command = None;
        true
    }

    pub fn envelope(&self) -> f32 {
        self.envelope
    }

    /// Starts transition `serial`: per-sample linear from the current
    /// envelope value to `target` (0..1) over `round(seconds * rate)`
    /// frames (`seconds` 0..60). Replaces any running transition.
    pub fn transition(&mut self, serial: u32, target: f32, seconds: f32) -> bool {
        if !(target.is_finite() && (0.0..=1.0).contains(&target))
            || !(seconds.is_finite() && (0.0..=60.0).contains(&seconds))
        {
            return false;
        }
        self.command = Some(Command {
            serial,
            origin: self.envelope,
            destination: target,
            position: 0,
            duration: (seconds as f64 * self.samplerate as f64).round() as u64,
            fence: None,
        });
        true
    }

    /// The state of transition `serial` given the device clock (frames the
    /// device has played since this bus started counting).
    pub fn transition_state(&self, serial: u32, device_frames_played: u64) -> TransitionState {
        match self.command {
            Some(c) if c.serial == serial => match c.fence {
                Some(f) if device_frames_played > f => TransitionState::Complete,
                _ => TransitionState::Pending,
            },
            _ => TransitionState::Idle,
        }
    }

    /// The mixer frame index of the final target sample of the running
    /// transition, once it has been mixed.
    pub fn transition_fence(&self) -> Option<u64> {
        self.command.and_then(|c| c.fence)
    }

    /// A new device (restart) starts its clock at 0: the frame counter
    /// restarts, and a final target sample already mixed into the old
    /// device's queue counts as played once the new device has played a
    /// frame.
    pub fn device_restarted(&mut self) {
        self.frames_mixed = 0;
        if let Some(c) = self.command.as_mut() {
            if c.fence.is_some() {
                c.fence = Some(0);
            }
        }
    }

    // ------------------------------------------------------------------
    // layout

    /// The device layout (form factor included); refreshes the fill.
    pub fn set_output(&mut self, output: OutputLayout) {
        self.output = output;
        self.refresh_fill();
    }

    pub fn output(&self) -> OutputLayout {
        self.output
    }

    /// `FSMusicSpatialSound` (0, 21, 41, 51, 71); anything else is rejected
    /// and leaves the setting unchanged. Applies at the next sample.
    pub fn set_spatial_layout(&mut self, setting: i32) -> bool {
        if !SPATIAL_LAYOUTS.contains(&setting) {
            return false;
        }
        self.spatial = setting;
        self.refresh_fill();
        true
    }

    pub fn spatial_layout(&self) -> i32 {
        self.spatial
    }

    /// The fill mask in effect (0 = no fill).
    pub fn fill_mask(&self) -> u32 {
        self.fill
    }

    fn refresh_fill(&mut self) {
        let mask = if self.output.mask != 0 && self.output.mask.count_ones() == self.output.channels
        {
            self.output.mask
        } else {
            0
        };
        self.fill = fill_mask(self.spatial, mask, self.output.form_factor);
        self.route_key = None;
    }

    // ------------------------------------------------------------------
    // counters

    /// Frames pulled from the source.
    pub fn input_frames(&self) -> u64 {
        self.input_frames
    }

    /// Frames rendered while a source was attached (input + underrun).
    pub fn output_frames(&self) -> u64 {
        self.output_frames
    }

    /// Silent frames rendered because the source had nothing ready.
    pub fn underrun_frames(&self) -> u64 {
        self.underrun_frames
    }

    /// Frames the mixer has rendered (the clock fences are expressed in).
    pub fn frames_mixed(&self) -> u64 {
        self.frames_mixed
    }

    // ------------------------------------------------------------------
    // mixing

    fn build_routes(&mut self, layout: ChannelLayout) {
        let out_mask = self.output.effective_mask();
        let in_mask = if layout.mask != 0 && layout.mask.count_ones() == layout.channels as u32 {
            layout.mask
        } else {
            default_mask(layout.channels as u32)
        };
        let out_index = |bit: u32| -> Option<u8> {
            if out_mask & bit != 0 {
                Some((out_mask & (bit - 1)).count_ones() as u8)
            } else {
                None
            }
        };
        let mut routes = [Route::default(); MAX_CH * MAX_CH];
        let mut n = 0usize;
        let mut push = |o: Option<u8>, i: usize, coef: f32| {
            if let Some(o) = o {
                if n < routes.len() {
                    routes[n] = Route {
                        out_index: o,
                        in_index: i as u8,
                        coef,
                    };
                    n += 1;
                }
            }
        };
        let fl = out_index(SPEAKER_FRONT_LEFT);
        let fr = out_index(SPEAKER_FRONT_RIGHT);
        let fc = out_index(SPEAKER_FRONT_CENTER);
        // Mono input and mono output are special cases of the stereo pair.
        if in_mask == SPEAKER_FRONT_CENTER && layout.channels == 1 {
            if fl.is_some() || fr.is_some() {
                push(fl, 0, 1.0);
                push(fr, 0, 1.0);
            } else {
                push(fc, 0, 1.0);
            }
        } else {
            let mut i = 0usize;
            for bit_index in 0..32 {
                let bit = 1u32 << bit_index;
                if in_mask & bit == 0 {
                    continue;
                }
                if i >= MAX_CH {
                    break;
                }
                if let Some(o) = out_index(bit) {
                    push(Some(o), i, 1.0);
                } else {
                    let left = bit == SPEAKER_BACK_LEFT || bit == SPEAKER_SIDE_LEFT;
                    let right = bit == SPEAKER_BACK_RIGHT || bit == SPEAKER_SIDE_RIGHT;
                    let alt = match bit {
                        SPEAKER_BACK_LEFT => out_index(SPEAKER_SIDE_LEFT),
                        SPEAKER_BACK_RIGHT => out_index(SPEAKER_SIDE_RIGHT),
                        SPEAKER_SIDE_LEFT => out_index(SPEAKER_BACK_LEFT),
                        SPEAKER_SIDE_RIGHT => out_index(SPEAKER_BACK_RIGHT),
                        _ => None,
                    };
                    if alt.is_some() {
                        push(alt, i, 1.0);
                    } else if left {
                        push(fl, i, HALF_SQRT2);
                    } else if right {
                        push(fr, i, HALF_SQRT2);
                    } else if bit == SPEAKER_FRONT_CENTER {
                        push(fl, i, HALF_SQRT2);
                        push(fr, i, HALF_SQRT2);
                    } else if (bit == SPEAKER_FRONT_LEFT || bit == SPEAKER_FRONT_RIGHT)
                        && fl.is_none()
                        && fr.is_none()
                    {
                        push(fc, i, 0.5);
                    }
                    // LFE and other speakers absent from the device: dropped.
                }
                i += 1;
            }
        }
        self.routes = routes;
        self.route_count = n;
    }

    /// Adds `frames` of music into `out` (interleaved, `channels` per frame).
    pub(crate) fn mix_into(&mut self, out: &mut [f32], frames: usize, channels: usize) {
        let start = self.frames_mixed;
        self.frames_mixed += frames as u64;
        if self.source.is_none() {
            return;
        }
        self.output_frames += frames as u64;
        if self.ended {
            return;
        }
        let mut done = 0usize;
        while done < frames {
            let want = (frames - done).min(super::MIX_CHUNK_FRAMES);
            let Some(source) = self.source.as_mut() else {
                break;
            };
            let pulled = source.pull(&mut self.pull, want);
            let n = pulled.frames.min(want);
            if n > 0 {
                self.process(
                    pulled.layout,
                    n,
                    &mut out[done * channels..(done + n) * channels],
                    channels,
                    start + done as u64,
                );
                done += n;
                self.input_frames += n as u64;
            }
            if pulled.ended {
                self.ended = true;
                break;
            }
            if n == 0 {
                break;
            }
        }
        if !self.ended {
            self.underrun_frames += (frames - done) as u64;
        }
    }

    /// Envelope and gain for one frame (the VLC filter's per-frame order:
    /// envelope value, factor, then the gain ramp step).
    #[inline(always)]
    fn frame_factor(&mut self, abs_frame: u64) -> f32 {
        if let Some(c) = self.command.as_mut() {
            self.envelope = if c.position >= c.duration {
                c.destination
            } else {
                c.origin + (c.destination - c.origin) * c.position as f32 / c.duration as f32
            };
            if c.position < c.duration {
                c.position += 1;
            } else if c.fence.is_none() {
                c.fence = Some(abs_frame);
            }
        }
        let factor = if self.muted {
            0.0
        } else {
            self.gain * self.envelope
        };
        if self.remaining != 0 {
            self.gain += self.step;
            self.remaining -= 1;
            if self.remaining == 0 {
                self.gain = self.target;
            }
        }
        factor
    }

    fn process(
        &mut self,
        layout: ChannelLayout,
        frames: usize,
        out: &mut [f32],
        channels: usize,
        first_frame: u64,
    ) {
        let in_ch = (layout.channels as usize).clamp(1, MAX_CH);
        let stereo = in_ch == 2
            && (layout.mask == 0 || layout.mask == (SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT));
        let mono = in_ch == 1;
        if (stereo || mono) && self.fill != 0 {
            // Speaker fill (VLC's Fill(), then the gain of Process()).
            let out_mask = self.output.effective_mask();
            let fill = self.fill;
            for f in 0..frames {
                let factor = self.frame_factor(first_frame + f as u64);
                let (l, r) = if mono {
                    (self.pull[f], self.pull[f])
                } else {
                    (self.pull[f * 2], self.pull[f * 2 + 1])
                };
                let left = l * HALF_SQRT2;
                let right = r * HALF_SQRT2;
                let dst = &mut out[f * channels..(f + 1) * channels];
                let mut idx = 0usize;
                for bit_index in 0..32 {
                    let bit = 1u32 << bit_index;
                    if out_mask & bit == 0 {
                        continue;
                    }
                    if idx >= channels {
                        break;
                    }
                    if fill & bit != 0 {
                        let value = if bit
                            & (SPEAKER_FRONT_LEFT | SPEAKER_SIDE_LEFT | SPEAKER_BACK_LEFT)
                            != 0
                        {
                            left
                        } else if bit
                            & (SPEAKER_FRONT_RIGHT | SPEAKER_SIDE_RIGHT | SPEAKER_BACK_RIGHT)
                            != 0
                        {
                            right
                        } else if bit == SPEAKER_FRONT_CENTER {
                            (left + right) * 0.5
                        } else {
                            0.0
                        };
                        dst[idx] += value * factor;
                    }
                    idx += 1;
                }
            }
            return;
        }
        let key = (
            layout.channels,
            layout.mask,
            self.output.effective_mask(),
            self.output.channels,
        );
        if self.route_key != Some(key) {
            self.build_routes(layout);
            self.route_key = Some(key);
        }
        let routes = self.routes;
        let count = self.route_count;
        for f in 0..frames {
            let factor = self.frame_factor(first_frame + f as u64);
            let src = &self.pull[f * in_ch..(f + 1) * in_ch];
            let dst = &mut out[f * channels..(f + 1) * channels];
            for r in &routes[..count] {
                let o = r.out_index as usize;
                if o < channels {
                    let v = if r.coef == 1.0 {
                        src[r.in_index as usize]
                    } else {
                        src[r.in_index as usize] * r.coef
                    };
                    dst[o] += v * factor;
                }
            }
        }
    }
}
