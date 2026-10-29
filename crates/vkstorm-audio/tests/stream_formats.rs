//! Every supported format end to end: served by the mock radio server,
//! fetched with libcurl, demuxed, decoded, converted to the device rate and
//! pulled like the mixer pulls. Checks per format: the codec chosen, the
//! layout, each channel's tone (frequency and level) over the whole stream
//! and over its first second (a rate error at the start, the defect being
//! replaced, would shift the frequency), the tags, and the frame
//! accounting (each segment delivers exactly ceil(in * device / source)
//! frames, and the mixer pulled exactly their sum).
//!
//! Needs `--features link-curl` (and VKS_FFMPEG_DIR for HE-AAC and Opus).

#![cfg(feature = "link-curl")]

mod stream_support;

use std::time::Duration;

use stream_support::*;
use vkstorm_audio::stream::resample::expected_out_frames;
use vkstorm_audio::stream::{MusicPlayer, PlayerEvent, PlayerState};

struct Expect {
    codec: &'static str,
    channels: u16,
    mask: u32,
    /// (frequency, amplitude) per channel; amplitude 0 skips the level.
    tones: &'static [(f64, f64)],
    /// Decoded source frames: exact, or a range for codecs with encoder
    /// delay and padding.
    frames: (u64, u64),
    source_rate: u32,
    tags: Option<(&'static str, &'static str)>,
}

fn run(file: &str, rate: u32, e: &Expect) {
    let srv = server();
    let player = MusicPlayer::new(config(rate));
    player.start(&srv.url(&format!("/media/{file}")));
    let mut reader = player.reader();
    let p = pull(&mut reader, 480, None, Duration::from_secs(60));
    let state = player.state();
    assert!(p.ended, "{file}: not ended, state {state:?}");
    assert_eq!(state, PlayerState::Ended, "{file}");
    let stats = player.stats();
    assert_eq!(stats.codec, e.codec, "{file}: codec");

    // Frame accounting.
    assert!(!stats.segments.is_empty(), "{file}: no segments");
    let mut total = 0;
    for s in &stats.segments {
        assert_eq!(s.out_rate, rate);
        assert_eq!(
            s.out_frames,
            expected_out_frames(s.in_frames, s.source_rate, rate),
            "{file}: segment {s:?}"
        );
        total += s.out_frames;
    }
    assert_eq!(p.frames, total, "{file}: pulled frames");
    assert_eq!(stats.pulled_frames, total);
    let decoded: u64 = stats.segments.iter().map(|s| s.in_frames).sum();
    assert!(
        decoded >= e.frames.0 && decoded <= e.frames.1,
        "{file}: decoded {decoded} frames, expected {:?}",
        e.frames
    );
    let seg = stats.segments[0];
    assert_eq!(seg.source_rate, e.source_rate, "{file}: source rate");
    assert_eq!(
        (seg.layout.channels, seg.layout.mask),
        (e.channels, e.mask),
        "{file}: layout"
    );

    // Content.
    for (c, &(freq, amp)) in e.tones.iter().enumerate() {
        let x = p.channel(e.channels, c);
        let x = trim_start(&x);
        let r = rate as usize;
        assert!(x.len() > r, "{file}: channel {c} too short");
        let first = &x[..r];
        let f1 = frequency(first, rate);
        assert!(
            (f1 - freq).abs() < 1.0,
            "{file}: channel {c} first second at {f1:.2} Hz, expected {freq}"
        );
        let mid = &x[r / 4..x.len() - r / 4];
        let f = frequency(mid, rate);
        assert!(
            (f - freq).abs() < 0.5,
            "{file}: channel {c} at {f:.3} Hz, expected {freq}"
        );
        if amp > 0.0 {
            let a = amplitude(mid, rate, freq);
            assert!(
                (a / amp - 1.0).abs() < 0.1,
                "{file}: channel {c} level {a:.3}, expected {amp}"
            );
        }
    }

    let ev = events(&player);
    if let Some((artist, title)) = e.tags {
        assert_eq!(ev, vec![meta(artist, title)], "{file}: metadata");
    } else {
        assert!(ev.is_empty(), "{file}: unexpected metadata {ev:?}");
    }
    player.stop();
    assert_eq!(
        events(&player),
        if e.tags.is_some() {
            vec![PlayerEvent::MetadataCleared]
        } else {
            vec![]
        }
    );
}

const ST: &[(f64, f64)] = &[(440.0, 0.5), (660.0, 0.25)];

#[test]
fn mp3() {
    run(
        "tone.mp3",
        48000,
        &Expect {
            codec: "mp3",
            channels: 2,
            mask: 3,
            tones: ST,
            frames: (176400, 176400 + 3000),
            source_rate: 44100,
            tags: None,
        },
    );
}

#[test]
fn mp3_mono_mpeg2_with_id3_rate_44100() {
    run(
        "tone_mono22.mp3",
        44100,
        &Expect {
            codec: "mp3",
            channels: 1,
            mask: 4,
            tones: &[(440.0, 0.5)],
            frames: (44100, 44100 + 3000),
            source_rate: 22050,
            tags: None,
        },
    );
    run(
        "tone_id3.mp3",
        44100,
        &Expect {
            codec: "mp3",
            channels: 2,
            mask: 3,
            tones: ST,
            frames: (88200, 88200 + 3000),
            source_rate: 44100,
            tags: Some(("Tag Artist", "Tag Title")),
        },
    );
}

#[test]
fn mp2() {
    run(
        "tone.mp2",
        48000,
        &Expect {
            codec: "mp2",
            channels: 2,
            mask: 3,
            tones: ST,
            frames: (192000, 192000 + 3000),
            source_rate: 48000,
            tags: None,
        },
    );
}

#[test]
fn vorbis() {
    run(
        "tone.ogg",
        48000,
        &Expect {
            codec: "vorbis",
            channels: 2,
            mask: 3,
            tones: ST,
            frames: (176400, 176400),
            source_rate: 44100,
            tags: Some(("Plain Artist", "Plain Title")),
        },
    );
}

#[test]
fn vorbis_5_1_channel_order() {
    let tones: &[(f64, f64)] = &[
        (300.0, 0.3),
        (400.0, 0.3),
        (500.0, 0.3),
        (50.0, 0.0),
        (700.0, 0.3),
        (800.0, 0.3),
    ];
    // 1 s: skip the first-second check by analysing with the full length.
    let srv = server();
    let player = MusicPlayer::new(config(48000));
    player.start(&srv.url("/media/tone51.ogg"));
    let mut reader = player.reader();
    let p = pull(&mut reader, 480, None, Duration::from_secs(30));
    assert!(p.ended);
    let stats = player.stats();
    let seg = stats.segments[0];
    assert_eq!(
        (seg.layout.channels, seg.layout.mask, seg.in_frames),
        (6, 0x3F, 48000)
    );
    for (c, &(freq, amp)) in tones.iter().enumerate() {
        let x = p.channel(6, c);
        let mid = &x[4800..x.len() - 4800];
        let f = frequency(mid, 48000);
        assert!((f - freq).abs() < 0.5, "channel {c}: {f} Hz");
        // The LFE level is the encoder's (libvorbis codes it at another
        // level at this quality; FFmpeg's decoder reads the same).
        if amp > 0.0 {
            let a = amplitude(mid, 48000, freq);
            assert!((a / amp - 1.0).abs() < 0.1, "channel {c}: level {a}");
        }
    }
}

#[test]
fn aac_lc() {
    run(
        "tone_lc.aac",
        48000,
        &Expect {
            codec: "aac-lc",
            channels: 2,
            mask: 3,
            tones: ST,
            frames: (176400, 176400 + 4096),
            source_rate: 44100,
            tags: None,
        },
    );
}

#[test]
fn he_aac_v1() {
    if ffmpeg_dir().is_none() {
        eprintln!("VKS_FFMPEG_DIR not set: skipped");
        return;
    }
    run(
        "tone_he1.aac",
        48000,
        &Expect {
            codec: "he-aac",
            channels: 2,
            mask: 3,
            tones: &[(440.0, 0.5)],
            frames: (192000, 192000 + 8192),
            source_rate: 48000,
            tags: None,
        },
    );
}

#[test]
fn he_aac_v2() {
    if ffmpeg_dir().is_none() {
        eprintln!("VKS_FFMPEG_DIR not set: skipped");
        return;
    }
    run(
        "tone_he2.aac",
        48000,
        &Expect {
            codec: "he-aac-v2",
            channels: 2,
            mask: 3,
            tones: &[(440.0, 0.0)],
            frames: (176400, 176400 + 8192),
            source_rate: 44100,
            tags: None,
        },
    );
}

#[test]
fn he_aac_sbr_content() {
    if ffmpeg_dir().is_none() {
        return;
    }
    // The 9 kHz tone of channel 2 lies above the core's bandwidth: only
    // SBR reconstructs it.
    let srv = server();
    let player = MusicPlayer::new(config(48000));
    player.start(&srv.url("/media/tone_he1.aac"));
    let mut reader = player.reader();
    let p = pull(&mut reader, 480, None, Duration::from_secs(30));
    assert!(p.ended);
    let r = p.channel(2, 1);
    let mid = &r[24000..r.len() - 24000];
    let a = amplitude(mid, 48000, 9000.0);
    assert!(a > 0.1, "9 kHz at {a}");
}

#[test]
fn opus_exact_length() {
    if ffmpeg_dir().is_none() {
        eprintln!("VKS_FFMPEG_DIR not set: skipped");
        return;
    }
    // Pre-skip removed, end trimmed at the final granule position: the
    // decoded length is the encoder's input exactly.
    run(
        "tone.opus",
        48000,
        &Expect {
            codec: "opus",
            channels: 2,
            mask: 3,
            tones: ST,
            frames: (192000, 192000),
            source_rate: 48000,
            tags: Some(("Opus Artist", "Opus Title")),
        },
    );
}

#[test]
fn flac() {
    run(
        "tone.flac",
        48000,
        &Expect {
            codec: "flac",
            channels: 2,
            mask: 3,
            tones: ST,
            frames: (88200, 88200),
            source_rate: 44100,
            tags: Some(("Flac Artist", "Flac Title")),
        },
    );
}

#[test]
fn ogg_flac() {
    let srv = server();
    let player = MusicPlayer::new(config(48000));
    player.start(&srv.url("/media/tone_flac.oga"));
    let mut reader = player.reader();
    let p = pull(&mut reader, 480, None, Duration::from_secs(30));
    assert!(p.ended);
    let stats = player.stats();
    assert_eq!(stats.codec, "flac");
    assert_eq!(stats.segments[0].in_frames, 44100);
    let x = p.channel(2, 0);
    assert!((frequency(&x[4800..x.len() - 4800], 48000) - 440.0).abs() < 0.5);
    assert_eq!(events(&player), vec![meta("", "OggFlac Title")]);
}

#[test]
fn chained_ogg_links_rates_and_comments() {
    let srv = server();
    let player = MusicPlayer::new(config(48000));
    player.start(&srv.url("/media/chain.ogg"));
    let mut reader = player.reader();
    let p = pull(&mut reader, 480, None, Duration::from_secs(30));
    assert!(p.ended);
    let stats = player.stats();
    let segs: Vec<(u32, u16, u64)> = stats
        .segments
        .iter()
        .map(|s| (s.source_rate, s.layout.channels, s.in_frames))
        .collect();
    assert_eq!(
        segs,
        vec![(44100, 2, 88200), (48000, 2, 144000), (32000, 1, 64000)]
    );
    let total: u64 = stats
        .segments
        .iter()
        .map(|s| expected_out_frames(s.in_frames, s.source_rate, 48000))
        .sum();
    assert_eq!(p.frames, total);
    // Each link's own layout reached the mixer, in order.
    let layouts: Vec<u16> = p.runs.iter().map(|(l, _)| l.channels).collect();
    assert_eq!(layouts, vec![2, 1]);
    assert_eq!(
        events(&player),
        vec![
            meta("First Artist", "First Title"),
            meta("Second Artist", "Second Title"),
            meta("Third Artist", "Third Title")
        ]
    );
    let mono = p.channel(1, 0);
    assert!((frequency(&mono[4800..mono.len() - 4800], 48000) - 440.0).abs() < 0.5);
}
