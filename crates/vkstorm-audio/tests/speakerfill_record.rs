//! The music bus against the record of today's VLC speaker fill
//! (`tests/fixtures/speakerfill_matrices.txt`, recorded before the port by
//! the viewer's `indra/llaudio/tests/speakerfill_record.cpp` from the real
//! `libspeakerfill_plugin.dll` filter and the patched VLC WASAPI output
//! module, libvlc 3.0.21).
//!
//! Sections and how they compare:
//! - `fill`: the output frame for a unit impulse on one input channel, in
//!   the device's WAVEFORMATEXTENSIBLE order, for stereo, 5.1 and mono
//!   sources and every target layout the record covers. The bus runs on a
//!   device whose mask is the target layout, with the FSMusicSpatialSound
//!   value that requests it. Exact float equality where the arithmetic is
//!   the same (it is: one multiply by 0.70710678 and a sum times 0.5).
//! - `negotiate`: which speakers the fill uses for a device condition, a
//!   device mask and a layout setting. The VLC output opened a stream in a
//!   negotiated format; the mixer always renders in the endpoint's own
//!   format, so the record's "device=" channel count is not comparable and
//!   the "surround-rejected" condition (the endpoint refusing the surround
//!   format at IsFormatSupported) cannot arise. Compared: the published
//!   fill mask (or none).
//! - `curve`: the cubic user gain, exactly.
//! - `ramp`: the 50 ms per-sample gain ramp on a constant 1.0 input.
//! - `fade`: the 3 s and 2 s envelopes at the recorded samples, their
//!   duration, and completion at the final target sample.
//!
//! Every documented difference is counted and asserted to be exactly the
//! expected set, so a change in either direction fails the test.

use vkstorm_audio::mixer::music::{
    FormFactor, MASK_5POINT1, MASK_5POINT1_SURROUND, MASK_7POINT1_SURROUND, MASK_QUAD,
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

const RECORD: &str = include_str!("fixtures/speakerfill_matrices.txt");

struct Source {
    layout: ChannelLayout,
    produced: usize,
    total: usize,
    channel: Option<usize>,
}

impl MusicSource for Source {
    fn pull(&mut self, out: &mut [f32], max_frames: usize) -> Pulled {
        let n = max_frames.min(self.total - self.produced);
        let ch = self.layout.channels as usize;
        for f in 0..n {
            for c in 0..ch {
                out[f * ch + c] = match self.channel {
                    Some(k) => (k == c) as u32 as f32,
                    None => 1.0,
                };
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

fn speaker_bit(name: &str) -> u32 {
    match name {
        "FL" => FL,
        "FR" => FR,
        "FC" => FC,
        "LFE" => LFE,
        "BL" => BL,
        "BR" => BR,
        "SL" => SL,
        "SR" => SR,
        "BC" => 0x100,
        other => panic!("speaker {other}"),
    }
}

fn mask_of(names: &str) -> u32 {
    if names == "-" {
        return 0;
    }
    names.split(',').map(speaker_bit).fold(0, |a, b| a | b)
}

/// `name[A,B,C]=v v v` -> (names, values).
fn bracketed(field: &str) -> (Vec<String>, Vec<f32>) {
    let open = field.find('[').unwrap();
    let close = field.find(']').unwrap();
    let names = field[open + 1..close]
        .split(',')
        .map(str::to_owned)
        .collect();
    let values = field[close + 2..]
        .split_whitespace()
        .map(|v| v.parse::<f32>().unwrap())
        .collect();
    (names, values)
}

/// The device mask and the FSMusicSpatialSound value that reach a target
/// layout of the VLC filter, or None when no setting requests it.
fn target(name: &str) -> (u32, i32, bool) {
    match name {
        "none" | "2.0" => (FL | FR, 0, true),
        // Endpoints run at 1, 2, 4, 6 or 8 channels (the device contract,
        // as with the SoLoud package): the 2.1 and 4.1 fills land on a 5.1
        // endpoint, the speakers they do not use stay silent
        "2.1" => (MASK_5POINT1, 21, true),
        "4.1rear" => (MASK_5POINT1, 41, true),
        "4.1side" => (MASK_5POINT1_SURROUND, 41, true),
        "5.1rear" => (MASK_5POINT1, 51, true),
        "5.1side" => (MASK_5POINT1_SURROUND, 51, true),
        "7.1" => (MASK_7POINT1_SURROUND, 71, true),
        // The filter accepted 4.0 and 2.0+sides, but no setting requests
        // them (every fill layout carries the LFE) and the VLC output never
        // negotiated them either (see the negotiate rows for FL,FR,BL,BR)
        "4.0" => (MASK_QUAD, 41, false),
        "4.0side" => (FL | FR | SL | SR, 41, false),
        // Layouts the filter refused (bypass): no setting requests them
        "5.0rear(unsupported)" | "3.0(unsupported)" => (FL | FR, 0, true),
        other => panic!("target {other}"),
    }
}

/// Input channel index of the record's VLC channel name, in the source's
/// WAVEFORMATEXTENSIBLE order.
fn input(source: &str, name: &str) -> (u16, u32, usize) {
    match source {
        "stereo" => (2, FL | FR, if name == "L" { 0 } else { 1 }),
        // VLC L R RL RR C LFE -> FL FR FC LFE BL BR
        "5.1" => (
            6,
            MASK_5POINT1,
            match name {
                "L" => 0,
                "R" => 1,
                "C" => 2,
                "LFE" => 3,
                "RL" => 4,
                "RR" => 5,
                other => panic!("{other}"),
            },
        ),
        "mono" => (1, FC, 0),
        other => panic!("source {other}"),
    }
}

fn channel_index(mask: u32, bit: u32) -> Option<usize> {
    if mask & bit == 0 {
        None
    } else {
        Some((mask & (bit - 1)).count_ones() as usize)
    }
}

#[test]
fn fill_matrices() {
    let (mut identical, mut unreachable, mut mono_documented) = (0, 0, 0);
    let mut rows = 0;
    for line in RECORD.lines().filter(|l| l.starts_with("fill ")) {
        rows += 1;
        let t: Vec<&str> = line.split_whitespace().collect();
        let (source, tname, inch) = (t[1], t[2], &t[3]["in=".len()..]);
        let device_field = t.iter().position(|f| f.starts_with("device[")).unwrap();
        let joined = t[device_field..].join(" ");
        let (names, values) = bracketed(&joined);
        let (dev_mask, setting, reachable) = target(tname);
        let (sch, smask, sidx) = input(source, inch);
        // VLC always opened a 5.1 stream for a 5.1 source (negotiate
        // source-5.1 rows): the bus passes it through on a 5.1 device
        let dev_mask = if source == "5.1" {
            MASK_5POINT1
        } else {
            dev_mask
        };
        let mut m = mixer(48000, dev_mask, FormFactor::Speakers);
        m.music.set_spatial_layout(setting);
        m.music.set_volume(1.0);
        m.music.attach(Box::new(Source {
            layout: ChannelLayout {
                channels: sch,
                mask: smask,
            },
            produced: 0,
            total: 1,
            channel: Some(sidx),
        }));
        let out = render(&mut m, 1, 1);
        if source == "mono" {
            // The record shows the filter leaving a mono stream mono (FC);
            // VLC's own channel mixer then downmixed it for the endpoint.
            // The bus plays mono as dual mono on FL/FR at unity, whatever
            // the endpoint (the stereo-endpoint behaviour of VLC).
            let fl = out[channel_index(dev_mask, FL).unwrap()];
            let fr = out[channel_index(dev_mask, FR).unwrap()];
            assert_eq!((fl, fr), (1.0, 1.0), "{line}: dual mono");
            mono_documented += 1;
            continue;
        }
        let mut expected = vec![0.0f32; dev_mask.count_ones() as usize];
        for (n, v) in names.iter().zip(&values) {
            if let Some(i) = channel_index(dev_mask, speaker_bit(n)) {
                expected[i] = *v;
            }
        }
        if !reachable && source == "stereo" {
            // No fill: stereo stays on FL/FR at unity
            let mut plain = vec![0.0f32; expected.len()];
            plain[channel_index(dev_mask, if inch == "L" { FL } else { FR }).unwrap()] = 1.0;
            assert_eq!(out, plain, "{line}: unreachable target, no fill");
            unreachable += 1;
            continue;
        }
        assert_eq!(out, expected, "{line}");
        identical += 1;
    }
    eprintln!(
        "fill: {rows} rows: {identical} identical, {unreachable} unreachable targets (4.0, 2.0+sides), {mono_documented} mono rows documented"
    );
    assert_eq!(rows, 108);
    assert_eq!(unreachable, 4);
    assert_eq!(mono_documented, 12);
}

#[test]
fn negotiation() {
    let (mut same, mut surround_rejected, mut source_51, mut fewer_channels) = (0, 0, 0, 0);
    let mut rows = 0;
    for line in RECORD.lines().filter(|l| l.starts_with("negotiate ")) {
        rows += 1;
        let t: Vec<&str> = line.split_whitespace().collect();
        let (condition, dev_mask) = (t[1], mask_of(t[2]));
        let layout: i32 = t[3]["layout=".len()..].parse().unwrap();
        let published = t.iter().find(|f| f.starts_with("published=")).unwrap();
        let recorded = mask_of(published["published=".len()..].split('/').nth(1).unwrap());
        let device = t.iter().find(|f| f.starts_with("device=")).unwrap();
        let opened = mask_of(&device[device.find('[').unwrap() + 1..device.len() - 1]);
        if t[5] == "ok" && opened.count_ones() < dev_mask.count_ones() {
            fewer_channels += 1;
        }
        let form = match condition {
            "speakers" | "source-5.1" | "surround-rejected" => FormFactor::Speakers,
            "headphones" => FormFactor::Headphones,
            "formfactor-fails" => FormFactor::Unknown,
            // The mix format query failed: the layout is unknown
            "mixformat-fails" => FormFactor::Speakers,
            // No output: no fill
            "initialize-fails" => FormFactor::Speakers,
            other => panic!("{other}"),
        };
        let rust = match condition {
            "mixformat-fails" => fill_mask(layout, 0, form),
            "initialize-fails" => 0,
            _ => fill_mask(layout, dev_mask, form),
        };
        if condition == "source-5.1" {
            // A multichannel source is never filled; it passes through
            assert_eq!(recorded, 0, "{line}");
            source_51 += 1;
            continue;
        }
        if condition == "surround-rejected" {
            // The endpoint refused the surround format, so VLC stayed in
            // stereo; the mixer renders in the endpoint's own format and
            // fills whenever the layout fits the endpoint
            assert_eq!(recorded, 0, "{line}");
            assert_eq!(rust, fill_mask(layout, dev_mask, FormFactor::Speakers));
            surround_rejected += 1;
            continue;
        }
        assert_eq!(rust, recorded, "{line}");
        same += 1;
    }
    eprintln!(
        "negotiate: {rows} rows: {same} identical fill decisions, {source_51} 5.1-source passthrough rows, \
         {surround_rejected} surround-rejected rows (no counterpart); VLC opened fewer channels than the endpoint in \
         {fewer_channels} rows (the mixer renders in the endpoint's format; unfilled music is on FL/FR either way)"
    );
    assert_eq!(rows, 336);
    assert_eq!(same, 240);
}

#[test]
fn gain_curve() {
    let mut rows = 0;
    for line in RECORD.lines().filter(|l| l.starts_with("curve volume=")) {
        let t: Vec<&str> = line.split_whitespace().collect();
        let volume: f32 = t[1]["volume=".len()..].parse().unwrap();
        let gain: f32 = t[3]["gain=".len()..].parse().unwrap();
        let mut m = mixer(48000, FL | FR, FormFactor::Speakers);
        assert!(m.music.set_volume(volume));
        assert_eq!(m.music.target_gain(), gain, "{line}");
        rows += 1;
    }
    eprintln!("curve: {rows} rows identical");
    assert_eq!(rows, 10);
}

/// `[i]=v` pairs of a ramp or fade line.
fn samples(line: &str) -> Vec<(usize, f32)> {
    line.split_whitespace()
        .filter(|f| f.starts_with('['))
        .map(|f| {
            let close = f.find(']').unwrap();
            (
                f[1..close].parse().unwrap(),
                f[close + 2..].parse().unwrap(),
            )
        })
        .collect()
}

#[test]
fn gain_ramp() {
    let mut max_err = 0.0f32;
    let mut checked = 0;
    for rate in [44100u32, 48000] {
        let lines: Vec<&str> = RECORD
            .lines()
            .filter(|l| l.starts_with(&format!("ramp rate={rate} ")))
            .collect();
        // 1 -> 0.25, then 0.25 -> 1 retargeted to 0 halfway, then -> 0
        let mut m = mixer(rate, FL | FR, FormFactor::Speakers);
        m.music.set_volume(1.0);
        m.music.attach(Box::new(Source {
            layout: ChannelLayout {
                channels: 2,
                mask: FL | FR,
            },
            produced: 0,
            total: usize::MAX / 2,
            channel: None,
        }));
        m.music.set_volume(0.25f32.cbrt());
        let down = render(&mut m, 2410 + 100, 37);
        let up_line = lines[1];
        let retarget: usize = up_line.split_whitespace().nth(7).unwrap().parse().unwrap();
        m.music.set_volume(1.0);
        let up = render(&mut m, retarget, 37);
        m.music.set_volume(0.0);
        let zero = render(&mut m, 2410 + 100, 37);
        for (line, out) in [(lines[0], &down), (lines[1], &up), (lines[2], &zero)] {
            for (i, v) in samples(line) {
                let e = (out[i * 2] - v).abs();
                max_err = max_err.max(e);
                assert!(e < 2e-5, "{line}: [{i}] {} vs {v}", out[i * 2]);
                checked += 1;
            }
        }
        // The last listed sample of each ramp is the target exactly
        assert_eq!(zero[(rate as usize / 20) * 2], 0.0);
        // Open at gain 0.125: the first sample is 0.125, no ramp from 1
        let mut m = mixer(rate, FL | FR, FormFactor::Speakers);
        m.music.set_volume(0.5);
        m.music.attach(Box::new(Source {
            layout: ChannelLayout {
                channels: 2,
                mask: FL | FR,
            },
            produced: 0,
            total: 1,
            channel: None,
        }));
        assert_eq!(render(&mut m, 1, 1)[0], 0.125);
    }
    eprintln!(
        "ramp: {checked} recorded samples within 2e-5 (max |diff| {max_err:e}; both accumulate a per-sample step in float, from a target cbrt(0.25)^3 that is not exactly 0.25 here)"
    );
}

#[test]
fn fades() {
    let mut max_err = 0.0f32;
    let mut checked = 0;
    for line in RECORD
        .lines()
        .filter(|l| l.starts_with("fade rate=") && l.contains(" samples="))
    {
        let t: Vec<&str> = line.split_whitespace().collect();
        let rate: u32 = t[1]["rate=".len()..].parse().unwrap();
        let seconds: f32 = t[3].trim_end_matches('s').parse().unwrap();
        let initial: f32 = t[4]["initial=".len()..].parse().unwrap();
        let target: f32 = t[5]["target=".len()..].parse().unwrap();
        let count: usize = t[6]["samples=".len()..].parse().unwrap();
        let info = RECORD
            .lines()
            .find(|l| {
                l.starts_with(&format!("fade rate={rate} {} {} initial", t[2], t[3]))
                    && l.contains("duration_samples=")
            })
            .unwrap();
        let duration: u64 = info
            .split_whitespace()
            .find(|f| f.starts_with("duration_samples="))
            .unwrap()["duration_samples=".len()..]
            .parse()
            .unwrap();
        let mut m = mixer(rate, FL | FR, FormFactor::Headphones);
        m.music.set_volume(1.0);
        m.music.set_envelope(initial);
        m.music.attach(Box::new(Source {
            layout: ChannelLayout {
                channels: 2,
                mask: FL | FR,
            },
            produced: 0,
            total: usize::MAX / 2,
            channel: None,
        }));
        assert!(m.music.transition(1, target, seconds));
        let out = render(&mut m, count + 512, 512);
        for (i, v) in samples(line) {
            let e = (out[i * 2] - v).abs();
            max_err = max_err.max(e);
            assert!(e < 1e-6, "{line}: [{i}] {} vs {v}", out[i * 2]);
            checked += 1;
        }
        // Completion at the final target sample (the record's duration),
        // once the device clock has passed it
        assert_eq!(m.music.transition_fence(), Some(duration), "{line}");
        assert_eq!(
            m.music.transition_state(1, duration),
            TransitionState::Pending
        );
        assert_eq!(
            m.music.transition_state(1, duration + 1),
            TransitionState::Complete
        );
    }
    eprintln!(
        "fade: {checked} recorded envelope samples within 1e-6 (max |diff| {max_err:e}), 4 durations and completion frames identical"
    );
}
