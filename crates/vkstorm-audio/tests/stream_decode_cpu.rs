//! The decode cost of each music format: demux + decode + conversion to the
//! device rate (48 kHz) on one thread, as thread CPU time per second of
//! audio (the best of three runs of about 600 s of audio each: Windows
//! counts thread time in 15.6 ms ticks, so a run must take far longer). The same
//! pipeline as a stream's session thread, minus the network.
//!
//! Run: `cargo test -p vkstorm-audio --release --features link-curl --test
//! stream_decode_cpu -- --ignored --nocapture` (VKS_FFMPEG_DIR for HE-AAC
//! and Opus). The network is not used, but the crate's stream code links
//! libcurl, so the feature is needed to link the test.

#![cfg(feature = "link-curl")]

mod stream_support;

use stream_support::{ffmpeg_dir, fixture};
use vkstorm_audio::stream::decode_bytes;
use vkstorm_audio::stream::resample::Resampler;

/// This thread's CPU time in seconds.
#[cfg(windows)]
fn thread_cpu() -> f64 {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        lo: u32,
        hi: u32,
    }
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut std::ffi::c_void;
        fn GetThreadTimes(
            t: *mut std::ffi::c_void,
            c: *mut FileTime,
            e: *mut FileTime,
            k: *mut FileTime,
            u: *mut FileTime,
        ) -> i32;
    }
    let (mut c, mut e, mut k, mut u) = (
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
    );
    // SAFETY: the pseudo-handle of the current thread and four out-params.
    let ok = unsafe { GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) };
    assert!(ok != 0);
    let t = |f: &FileTime| ((u64::from(f.hi) << 32) | u64::from(f.lo)) as f64 * 1e-7;
    t(&k) + t(&u)
}

#[cfg(not(windows))]
fn thread_cpu() -> f64 {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, ts: *mut Timespec) -> i32;
    }
    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    let mut ts = Timespec { sec: 0, nsec: 0 };
    // SAFETY: a valid clock id and out-param.
    assert_eq!(
        unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut ts) },
        0
    );
    ts.sec as f64 + ts.nsec as f64 * 1e-9
}

#[test]
#[ignore = "measurement; run with --release --ignored --nocapture"]
fn decode_cpu_per_second_of_audio() {
    let files = [
        ("tone.mp3", "MP3 128k 44.1k st"),
        ("tone_mono22.mp3", "MP3 64k 22k mono"),
        ("tone.mp2", "MP2 192k 48k st"),
        ("tone.ogg", "Vorbis q3 44.1k st"),
        ("chain.ogg", "Vorbis chain"),
        ("tone_lc.aac", "AAC-LC 128k 44.1k"),
        ("tone_he1.aac", "HE-AAC 64k 48k"),
        ("tone_he2.aac", "HE-AACv2 32k 44.1k"),
        ("tone.opus", "Opus 96k 48k st"),
        ("tone.flac", "FLAC 44.1k st"),
        ("tone_flac.oga", "Ogg FLAC 44.1k st"),
    ];
    let ff = ffmpeg_dir();
    println!(
        "{:<16} {:<20} {:>12} {:>8}",
        "file", "format", "ms CPU / s", "% core"
    );
    for (file, what) in files {
        if ff.is_none() && (file.ends_with(".opus") || file.contains("he")) {
            println!("{file:<16} {what:<20} skipped (no VKS_FFMPEG_DIR)");
            continue;
        }
        let data = fixture(file);
        let once = decode_bytes(&data, 48000, ff.clone(), false).expect(file);
        let secs: f64 = once
            .segments
            .iter()
            .map(|(s, _)| s.in_frames as f64 / f64::from(s.source_rate))
            .sum();
        let reps = (600.0 / secs).ceil() as usize;
        let mut best = f64::MAX;
        for _ in 0..3 {
            let t0 = thread_cpu();
            for _ in 0..reps {
                decode_bytes(&data, 48000, ff.clone(), false).expect(file);
            }
            best = best.min((thread_cpu() - t0) / (reps as f64 * secs));
        }
        println!(
            "{file:<16} {what:<20} {:>12.3} {:>8.3}",
            best * 1e3,
            best * 100.0
        );
    }
    // The converter alone, 44.1 kHz stereo to 48 kHz (64 taps per phase).
    let n = 44100 * 30;
    let passes = 20; // 600 s of audio
    let l: Vec<f32> = (0..n).map(|i| (i as f32 * 0.0627).sin() * 0.5).collect();
    let r: Vec<f32> = (0..n).map(|i| (i as f32 * 0.0941).sin() * 0.25).collect();
    let mut best = f64::MAX;
    for _ in 0..3 {
        let mut rs = Resampler::new(44100, 48000, 2);
        let mut out = Vec::with_capacity(48000 * 2 * 31);
        let t0 = thread_cpu();
        for _ in 0..passes {
            for k in (0..n).step_by(1152) {
                let e = (k + 1152).min(n);
                rs.process(&[&l[k..e], &r[k..e]], e - k, &mut out);
                if out.len() > 1 << 20 {
                    out.clear();
                }
            }
        }
        rs.flush(&mut out);
        best = best.min((thread_cpu() - t0) / (30.0 * f64::from(passes)));
    }
    println!(
        "{:<16} {:<20} {:>12.3} {:>8.3}",
        "resampler",
        "44.1k->48k stereo",
        best * 1e3,
        best * 100.0
    );
}
