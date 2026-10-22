//! Real output devices (opt-in: `VKS_AUDIO_DEVICE_TESTS=1`, run with
//! `--nocapture` to see the report). Plays only silence.
//!
//! Enumerates the endpoints, opens the default one for 300 ms and checks
//! that the callback contract holds and the device clock advances, stops
//! it (no callback after stop), restarts it, and opens every enumerated
//! endpoint explicitly (Windows) for 300 ms each. On Linux it also opens
//! PulseAudio and ALSA individually.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use vkstorm_audio::device::{self, DeviceFormat, MAX_CALLBACK_FRAMES, OpenedDevice, Target};

fn enabled() -> bool {
    let on = std::env::var("VKS_AUDIO_DEVICE_TESTS").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set VKS_AUDIO_DEVICE_TESTS=1 to test real audio devices");
    }
    on
}

#[derive(Default)]
struct Counters {
    frames: AtomicU64,
    calls: AtomicUsize,
    max_call: AtomicUsize,
    bad: AtomicBool,
    sizes: std::sync::Mutex<Vec<usize>>,
}

fn describe(f: &DeviceFormat) -> String {
    format!(
        "{} | {} Hz | {} ch | mask {:#x} | {} | period {} | buffer {} | headphones {} | \"{}\"",
        f.backend,
        f.rate,
        f.channels,
        f.channel_mask,
        f.sample_format.name(),
        f.period_frames,
        f.buffer_frames,
        f.headphones,
        f.device_name
    )
}

/// Runs an opened device for `ms` milliseconds of silence and checks the
/// contract; returns frames rendered.
fn run_silence(opened: OpenedDevice, ms: u64) -> u64 {
    let format = opened.format().clone();
    let channels = usize::from(format.channels);
    let counters = Arc::new(Counters::default());
    let c = counters.clone();
    let callback = move |out: &mut [f32], frames: usize| {
        if out.len() != frames * channels
            || frames > MAX_CALLBACK_FRAMES
            || out.iter().any(|&s| s != 0.0)
        {
            c.bad.store(true, Ordering::Relaxed);
        }
        c.frames.fetch_add(frames as u64, Ordering::Relaxed);
        c.calls.fetch_add(1, Ordering::Relaxed);
        c.max_call.fetch_max(frames, Ordering::Relaxed);
        if let Ok(mut s) = c.sizes.try_lock()
            && s.len() < 12
        {
            s.push(frames);
        }
    };
    let started = Instant::now();
    let running = opened.start(Box::new(callback)).expect("start");
    let clock = running.clock();
    let first = clock.frames_played();
    std::thread::sleep(Duration::from_millis(ms));
    let played = clock.frames_played();
    let state = running.state();
    let events = running.take_events();
    let returned = running.stop();
    let elapsed = started.elapsed();
    let after_stop = counters.calls.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        counters.calls.load(Ordering::Relaxed),
        after_stop,
        "callback ran after stop returned"
    );
    assert!(returned.is_some(), "callback handed back");
    assert!(
        !counters.bad.load(Ordering::Relaxed),
        "callback contract violated"
    );

    let rendered = counters.frames.load(Ordering::Relaxed);
    let expected = format.rate as f64 * elapsed.as_secs_f64();
    eprintln!(
        "    ran {:.0} ms: state {:?}, events {:?}, clock {} -> {} frames ({:.0} ms), rendered {} frames in {} calls (max {}), first calls {:?}",
        elapsed.as_secs_f64() * 1000.0,
        state,
        events,
        first,
        played,
        played as f64 * 1000.0 / format.rate as f64,
        rendered,
        counters.calls.load(Ordering::Relaxed),
        counters.max_call.load(Ordering::Relaxed),
        counters.sizes.lock().map(|s| s.clone()).unwrap_or_default(),
    );
    assert_eq!(state, device::DeviceState::Running);
    assert!(played > 0, "device clock did not advance");
    assert!(played <= rendered, "clock ahead of rendered frames");
    // Played time cannot exceed wall time by more than scheduling slack.
    assert!(
        (played as f64) < expected + format.rate as f64 * 0.05,
        "clock ran faster than real time"
    );
    rendered
}

#[test]
fn real_output_devices() {
    if !enabled() {
        return;
    }
    let devices = device::enumerate().expect("enumerate");
    eprintln!("enumerated {} output endpoint(s):", devices.len());
    for d in &devices {
        eprintln!(
            "  {} default={} headphones={} \"{}\" id={}",
            d.uuid,
            d.is_default,
            d.headphones,
            d.name,
            d.endpoint_id()
        );
    }

    eprintln!("default endpoint:");
    let opened = device::open(Target::Default).expect("open default");
    eprintln!("  {}", describe(opened.format()));
    run_silence(opened, 300);

    eprintln!("default endpoint, reopened:");
    let opened = device::open(Target::Default).expect("reopen default");
    eprintln!("  {}", describe(opened.format()));
    run_silence(opened, 300);

    // Open and close without starting.
    drop(device::open(Target::Default).expect("open default unstarted"));

    eprintln!("default endpoint, five quick restarts:");
    for _ in 0..5 {
        run_silence(device::open(Target::Default).expect("restart default"), 120);
    }

    for d in &devices {
        eprintln!("endpoint \"{}\":", d.name);
        match device::open(Target::Endpoint(d)) {
            Ok(opened) => {
                eprintln!("  {}", describe(opened.format()));
                run_silence(opened, 300);
            }
            Err(e) => eprintln!("  open failed: {e}"),
        }
    }

    #[cfg(target_os = "linux")]
    {
        type Open = fn() -> Result<OpenedDevice, device::DeviceError>;
        let backends: [(&str, Open); 2] = [
            ("PulseAudio", device::linux::open_pulse_default),
            ("ALSA", device::linux::open_alsa_default),
        ];
        for (name, open) in backends {
            eprintln!("{name} default:");
            match open() {
                Ok(opened) => {
                    eprintln!("  {}", describe(opened.format()));
                    run_silence(opened, 300);
                }
                Err(e) => eprintln!("  open failed: {e}"),
            }
        }
    }
}
