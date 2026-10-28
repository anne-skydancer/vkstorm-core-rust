//! `indra/media_plugins/libvlc/tests/vlc_speakerfill_test.cpp` transcribed
//! for the music bus (`vkstorm_audio::mixer::music`).
//!
//! Case map (C++ -> here):
//! - `negotiation` (7 scenarios x 7 endpoint masks x layouts 0/21/41/51/71/99):
//!   [`negotiation`]. Scenario 0 (speakers) fills exactly when the C++
//!   expected it; 1 (headphones), 2 (mix format query failed: mask unknown)
//!   and 4 (form factor query failed) never fill; 3 (5.1 source) passes
//!   through; 5 (initialisation failed: "failed output publishes no mask")
//!   is a bus without a device layout. Scenario 6 (the endpoint rejects the
//!   surround format at IsFormatSupported) has no counterpart: the mixer
//!   renders in the endpoint's own mix format and never asks for another.
//! - filter samples, LFE silence, passthrough, timing metadata (2.0 and 5.1
//!   sources x the filter's layouts): [`fill_samples`], [`passthrough`]. The
//!   filter also accepted 4.0 and 2.0+sides without LFE, which no setting
//!   requests (the negotiation always adds LFE); those layouts are not
//!   reachable here. Timestamps and block flags have no counterpart (no
//!   blocks); the frame count is preserved ([`no_time_stretch`]).
//! - full-duration fade (3 s, partitions 37 and 4096, completion only after
//!   the final zero sample, fence, flush): [`three_second_fade`].
//! - per-sample gain (2/6/8 channels x partitions 1/37/512, retarget up and
//!   down, exact zero, initial gain without transient): [`gain_ramp`].
//! - protocol (cubic curve .125 -> .001953125, .105 -> .001157625, invalid
//!   gain and layout rejected, mute independent of the transition,
//!   publication is not completion, only the matching serial completes,
//!   flush cancels): [`protocol`]. "Output failure cannot complete a fade"
//!   is the device clock not advancing (no frames played, no completion).
//! - module DLL loading, VLC module discovery, option inheritance on the
//!   media player, the patched VLC output's `Play`/`TimeGet`/`Flush`
//!   against fake WASAPI clients: no counterpart (no plugin, no VLC); the
//!   device clock replaces TimeGet (helper D's device tests).

use vkstorm_audio::mixer::music::{
    FormFactor, MASK_5POINT1, MASK_5POINT1_SURROUND, MASK_7POINT1_SURROUND, MASK_QUAD, MusicBus,
    OutputLayout, TransitionState, fill_mask,
};
use vkstorm_audio::mixer::{Mixer, MixerConfig};
use vkstorm_audio::music::{
    ChannelLayout, MusicSource, Pulled, SPEAKER_BACK_LEFT, SPEAKER_BACK_RIGHT,
    SPEAKER_FRONT_CENTER, SPEAKER_FRONT_LEFT, SPEAKER_FRONT_RIGHT, SPEAKER_LOW_FREQUENCY,
    SPEAKER_SIDE_LEFT, SPEAKER_SIDE_RIGHT,
};

const FL: u32 = SPEAKER_FRONT_LEFT;
const FR: u32 = SPEAKER_FRONT_RIGHT;
const FC: u32 = SPEAKER_FRONT_CENTER;
const LFE: u32 = SPEAKER_LOW_FREQUENCY;
const BL: u32 = SPEAKER_BACK_LEFT;
const BR: u32 = SPEAKER_BACK_RIGHT;
const SL: u32 = SPEAKER_SIDE_LEFT;
const SR: u32 = SPEAKER_SIDE_RIGHT;

/// A source producing frames from a closure, `total` frames then ended.
struct Source<F: FnMut(usize, usize) -> f32 + Send> {
    layout: ChannelLayout,
    produced: usize,
    total: usize,
    value: F,
}

impl<F: FnMut(usize, usize) -> f32 + Send> MusicSource for Source<F> {
    fn pull(&mut self, out: &mut [f32], max_frames: usize) -> Pulled {
        let n = max_frames.min(self.total - self.produced);
        let ch = self.layout.channels as usize;
        for f in 0..n {
            for c in 0..ch {
                out[f * ch + c] = (self.value)(self.produced + f, c);
            }
        }
        self.produced += n;
        Pulled {
            frames: n,
            layout: self.layout,
            ended: self.produced == self.total,
        }
    }
}

fn source(
    channels: u16,
    mask: u32,
    total: usize,
    value: impl FnMut(usize, usize) -> f32 + Send + 'static,
) -> Box<dyn MusicSource> {
    Box::new(Source {
        layout: ChannelLayout { channels, mask },
        produced: 0,
        total,
        value,
    })
}

fn mixer(rate: u32, mask: u32, form_factor: FormFactor) -> Mixer {
    let channels = mask.count_ones();
    let mut m = Mixer::new(MixerConfig::viewer(rate, channels)).unwrap();
    m.set_global_volume(0.0); // effects silent: the output is the music bus
    m.music.set_output(OutputLayout {
        channels,
        mask,
        form_factor,
    });
    m
}

/// Renders `frames` in calls of `partition` frames.
fn render(m: &mut Mixer, frames: usize, partition: usize) -> Vec<f32> {
    let ch = m.channels() as usize;
    let mut out = vec![0.0f32; frames * ch];
    let mut done = 0;
    while done < frames {
        let n = partition.min(frames - done);
        m.render(&mut out[done * ch..(done + n) * ch], n);
        done += n;
    }
    out
}

const ENDPOINT_MASKS: [u32; 7] = [
    FL | FR,
    FL | FR | LFE,
    FL | FR | BL | BR | LFE,
    FL | FR | BL | BR,
    MASK_5POINT1,
    MASK_5POINT1_SURROUND,
    MASK_7POINT1_SURROUND,
];

#[test]
fn negotiation() {
    for scenario in 0..7 {
        if scenario == 6 {
            continue; // no counterpart, see the module docs
        }
        for mask in ENDPOINT_MASKS {
            for layout in [0, 21, 41, 51, 71, 99] {
                let mut requested = FL | FR | LFE;
                if layout == 41 || layout == 51 {
                    let rear = mask & (BL | BR) == (BL | BR);
                    requested |= if rear { BL | BR } else { SL | SR };
                    if layout == 51 {
                        requested |= FC;
                    }
                } else if layout == 71 {
                    requested = MASK_7POINT1_SURROUND;
                }
                let fill = scenario == 0
                    && [21, 41, 51, 71].contains(&layout)
                    && requested & mask == requested;
                let form = match scenario {
                    1 => FormFactor::Headphones,
                    4 => FormFactor::Unknown,
                    _ => FormFactor::Speakers,
                };
                let device_mask = if scenario == 2 { 0 } else { mask };
                let mut bus = MusicBus::new(48000);
                if scenario != 5 {
                    bus.set_output(OutputLayout {
                        channels: mask.count_ones(),
                        mask: device_mask,
                        form_factor: form,
                    });
                }
                let accepted = bus.set_spatial_layout(layout);
                assert_eq!(accepted, layout != 99, "invalid selection rejected");
                let expected = if fill && scenario != 3 { requested } else { 0 };
                if scenario == 3 {
                    // A 5.1 source is never filled, whatever the mask says.
                    let mut m = mixer(48000, MASK_5POINT1, FormFactor::Speakers);
                    m.music
                        .set_spatial_layout(if layout == 99 { 0 } else { layout });
                    m.music
                        .attach(source(6, MASK_5POINT1, 64, |_, c| c as f32 * 0.1));
                    let out = render(&mut m, 64, 64);
                    assert!(
                        (0..6).all(|c| out[c] == c as f32 * 0.1),
                        "5.1 passes through"
                    );
                    continue;
                }
                assert_eq!(
                    bus.fill_mask(),
                    expected,
                    "scenario {scenario} mask {mask:#x} layout {layout}"
                );
                if scenario != 5 {
                    assert_eq!(fill_mask(layout, device_mask, form), expected);
                }
            }
        }
    }
}

#[test]
fn fill_samples() {
    // (setting, endpoint mask, fill layout): every layout the settings
    // reach; endpoint channels outside the fill layout stay silent.
    let cases = [
        (21, MASK_5POINT1, FL | FR | LFE),
        (21, MASK_7POINT1_SURROUND, FL | FR | LFE),
        (41, MASK_5POINT1, FL | FR | BL | BR | LFE),
        (41, MASK_5POINT1_SURROUND, FL | FR | SL | SR | LFE),
        (51, MASK_5POINT1, MASK_5POINT1),
        (51, MASK_5POINT1_SURROUND, MASK_5POINT1_SURROUND),
        (51, MASK_7POINT1_SURROUND, MASK_5POINT1),
        (71, MASK_7POINT1_SURROUND, MASK_7POINT1_SURROUND),
    ];
    for (setting, mask, fill) in cases {
        let mut m = mixer(48000, mask, FormFactor::Speakers);
        assert!(m.music.set_spatial_layout(setting));
        assert_eq!(m.music.fill_mask(), fill);
        // L = 1, R = -0.5 as the C++ test feeds.
        m.music.attach(source(
            2,
            FL | FR,
            128,
            |_, c| if c == 0 { 1.0 } else { -0.5 },
        ));
        let out = render(&mut m, 128, 128);
        let ch = mask.count_ones() as usize;
        let mut idx = 0;
        for bit in (0..32).map(|b| 1u32 << b) {
            if mask & bit == 0 {
                continue;
            }
            let expected = if fill & bit == 0 {
                0.0
            } else if bit & (FL | SL | BL) != 0 {
                std::f32::consts::FRAC_1_SQRT_2 // VLC's .70710678f
            } else if bit & (FR | SR | BR) != 0 {
                -0.353_553_4
            } else if bit == FC {
                0.176_776_7
            } else {
                0.0
            };
            for f in 0..128 {
                assert!(
                    (out[f * ch + idx] - expected).abs() < 1e-6,
                    "routing, headroom, silent LFE"
                );
            }
            idx += 1;
        }
        assert_eq!(m.music.input_frames(), 128);
    }
}

#[test]
fn passthrough() {
    // Fill off: stereo to FL/FR at unity; a 5.1 source on a 5.1 endpoint
    // passes through untouched, authored LFE included.
    let mut m = mixer(48000, MASK_5POINT1, FormFactor::Speakers);
    m.music.attach(source(
        2,
        FL | FR,
        16,
        |_, c| if c == 0 { 1.0 } else { -0.5 },
    ));
    let out = render(&mut m, 16, 16);
    assert_eq!(&out[..6], &[1.0, -0.5, 0.0, 0.0, 0.0, 0.0]);
    let mut m = mixer(48000, MASK_5POINT1, FormFactor::Speakers);
    m.music.set_spatial_layout(51);
    m.music.attach(source(6, MASK_5POINT1, 16, |f, c| {
        (f * 6 + c) as f32 * 0.01
    }));
    let out = render(&mut m, 16, 16);
    for (i, s) in out.iter().enumerate() {
        assert_eq!(*s, i as f32 * 0.01);
    }
    // Mono is dual mono.
    let mut m = mixer(48000, FL | FR, FormFactor::Speakers);
    m.music.attach(source(1, FC, 4, |_, _| 0.25));
    let out = render(&mut m, 4, 4);
    assert_eq!(&out[..2], &[0.25, 0.25]);
}

#[test]
fn three_second_fade() {
    for partition in [37usize, 4096] {
        let mut m = mixer(48000, FL | FR, FormFactor::Headphones);
        m.music.set_volume(0.5f32.cbrt());
        m.music.set_envelope(1.0);
        m.music.attach(source(2, FL | FR, 144_001, |_, _| 1.0));
        assert!(
            m.music.transition(9, 0.0, 3.0),
            "one command, full duration"
        );
        let gain = m.music.target_gain();
        assert!((gain - 0.5).abs() < 1e-6);
        let mut position = 0usize;
        while position < 144_001 {
            let n = partition.min(144_001 - position);
            let out = render(&mut m, n, n);
            for f in 0..n {
                let expected = 0.5 * (1.0 - (position + f) as f32 / 144_000.0);
                assert!(
                    (out[f * 2] - expected).abs() < 1e-6,
                    "three-second gradient without viewer updates"
                );
            }
            position += n;
            // The device clock: every rendered frame played.
            let played = position as u64;
            let state = m.music.transition_state(9, played);
            assert_eq!(
                state == TransitionState::Complete,
                position == 144_001,
                "completion after the final zero sample"
            );
        }
        assert_eq!(
            m.music.transition_fence(),
            Some(144_000),
            "the final target sample's frame"
        );
        assert_eq!(
            m.music.transition_state(9, 144_000),
            TransitionState::Pending,
            "not before it has played"
        );
        assert_eq!(
            m.music.transition_state(8, 144_001),
            TransitionState::Idle,
            "only the matching serial"
        );
        drop(m.music.detach());
        assert_eq!(
            m.music.transition_state(9, 1 << 40),
            TransitionState::Idle,
            "flush cancels the unplayed completion"
        );
    }
}

#[test]
fn gain_ramp() {
    for (channels, mask) in [
        (2u16, FL | FR),
        (6, MASK_5POINT1),
        (8, MASK_7POINT1_SURROUND),
    ] {
        for partition in [1usize, 37, 512] {
            let ch = channels as usize;
            let mut m = mixer(48000, mask, FormFactor::Speakers);
            m.music.set_volume(1.0);
            m.music
                .attach(source(channels, mask, usize::MAX / 2, |_, _| 0.5));
            m.music.set_volume(0.005f32.cbrt());
            let out = render(&mut m, 2600, partition);
            for f in 0..2600 {
                let expected = 0.5
                    * if f >= 2400 {
                        0.005
                    } else {
                        1.0 + (0.005 - 1.0) * f as f32 / 2400.0
                    };
                for c in 0..ch {
                    assert!(
                        (out[f * ch + c] - expected).abs() < 3e-5,
                        "continuous per-sample gain"
                    );
                }
            }
            // Retarget up from .005: the first sample keeps the current gain.
            let mut m = mixer(48000, mask, FormFactor::Speakers);
            m.music.set_volume(0.005f32.cbrt());
            m.music
                .attach(source(channels, mask, usize::MAX / 2, |_, _| 1.0));
            m.music.set_volume(1.0);
            let rising = render(&mut m, 1200, partition);
            assert!(
                (rising[0] - 0.005).abs() < 1e-6,
                "rise starts from current gain"
            );
            m.music.set_volume(0.0);
            let falling = render(&mut m, 2401, partition);
            assert!(
                (falling[0] - 0.5025).abs() < 3e-5,
                "mid-ramp retarget preserves current sample gain"
            );
            for c in 0..ch {
                assert_eq!(
                    falling[2400 * ch + c],
                    0.0,
                    "zero endpoint is exact on every channel"
                );
            }
            // A new source starts at its gain without a transient.
            let mut m = mixer(48000, mask, FormFactor::Speakers);
            m.music.set_volume(0.5);
            m.music.attach(source(channels, mask, 1, |_, _| 1.0));
            let first = render(&mut m, 1, 1);
            assert_eq!(
                first[0], 0.125,
                "startup applies initial gain without full-volume transient"
            );
        }
    }
}

#[test]
fn protocol() {
    let mut bus = MusicBus::new(48000);
    assert!(bus.set_volume(0.125));
    assert!(
        (bus.target_gain() - 0.001953125).abs() < 1e-8,
        "VLC cubic curve without percentage rounding"
    );
    assert!(bus.set_volume(0.105));
    assert!(
        (bus.target_gain() - 0.001157625).abs() < 1e-8,
        "fractional target"
    );
    assert!(!bus.set_volume(-1.0), "invalid gain rejected");
    assert!(!bus.set_volume(f32::NAN));
    assert!(!bus.set_spatial_layout(99), "invalid layout rejected");
    assert!(bus.transition(9, 0.0, 3.0), "publish full fade command");
    bus.set_muted(true);
    assert!(bus.set_volume(0.25));
    assert!(bus.muted());
    assert_eq!(
        bus.transition_state(9, 0),
        TransitionState::Pending,
        "publication is not completion"
    );
    assert_eq!(
        bus.transition_fence(),
        None,
        "volume and mute do not retarget the transition"
    );
    assert!(
        !bus.transition(10, 1.5, 1.0) && !bus.transition(10, 1.0, 61.0),
        "out-of-range commands rejected"
    );
}

#[test]
fn mute_gates_samples() {
    let mut m = mixer(48000, FL | FR, FormFactor::Speakers);
    m.music.attach(source(2, FL | FR, 100, |_, _| 1.0));
    m.music.set_muted(true);
    let out = render(&mut m, 50, 50);
    assert!(out.iter().all(|&s| s == 0.0));
    m.music.set_muted(false);
    let out = render(&mut m, 50, 50);
    assert!(out.iter().all(|&s| s == 1.0));
}

#[test]
fn no_time_stretch() {
    // A source that delivers 300 frames per pull with gaps: every input
    // frame is output once, in order; missing frames are silence.
    struct Bursty {
        n: usize,
        calls: usize,
    }
    impl MusicSource for Bursty {
        fn pull(&mut self, out: &mut [f32], max_frames: usize) -> Pulled {
            self.calls += 1;
            let frames = if self.calls.is_multiple_of(3) {
                0
            } else {
                max_frames.min(300)
            };
            for f in 0..frames {
                out[f * 2] = (self.n + f) as f32;
                out[f * 2 + 1] = 0.0;
            }
            self.n += frames;
            Pulled {
                frames,
                layout: ChannelLayout::STEREO,
                ended: false,
            }
        }
    }
    let mut m = mixer(48000, FL | FR, FormFactor::Speakers);
    m.music.attach(Box::new(Bursty { n: 0, calls: 0 }));
    let out = render(&mut m, 4096, 512);
    let values: Vec<f32> = out.chunks(2).map(|f| f[0]).filter(|&v| v != 0.0).collect();
    for w in values.windows(2) {
        assert_eq!(
            w[1],
            w[0] + 1.0,
            "consecutive input frames, no skips or repeats"
        );
    }
    assert_eq!(
        m.music.input_frames() + m.music.underrun_frames(),
        m.music.output_frames()
    );
    assert_eq!(m.music.output_frames(), 4096);
    assert!(m.music.underrun_frames() > 0);
    let quad = OutputLayout {
        channels: 4,
        mask: MASK_QUAD,
        form_factor: FormFactor::Speakers,
    };
    m.music.set_output(quad);
    assert_eq!(m.music.output(), quad);
}
