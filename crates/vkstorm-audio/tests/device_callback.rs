//! The device callback contract without hardware: chunk splitting with
//! guard samples (the 3p-soloud `soloud_callback_bounds` test's cases),
//! pre-zeroing, clipping, and the null device.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use vkstorm_audio::device::{
    DeviceError, MAX_CALLBACK_FRAMES, NullConfig, NullDevice, RenderCallback, SampleFormat,
    render_split,
};

const GUARD: usize = 16;
const GUARD_VALUE: f32 = 1234.0;

/// Records call sizes; checks each slice arrives zeroed and sized
/// `frames * channels`; writes `value` everywhere.
struct Probe {
    channels: usize,
    value: f32,
    calls: Vec<usize>,
}

impl RenderCallback for Probe {
    fn render(&mut self, out: &mut [f32], frames: usize) {
        assert_eq!(out.len(), frames * self.channels);
        assert!(frames <= MAX_CALLBACK_FRAMES);
        assert!(out.iter().all(|&s| s == 0.0), "slice not zeroed on entry");
        self.calls.push(frames);
        out.fill(self.value);
    }
}

#[test]
fn split_bounds_and_guards() {
    assert_eq!(MAX_CALLBACK_FRAMES, 1024);
    for channels in [1usize, 2, 4, 6, 8] {
        for frames in [0usize, 1, 1024, 1025, 32769] {
            let mut buffer = vec![GUARD_VALUE; frames * channels + 2 * GUARD];
            let end = buffer.len() - GUARD;
            let mut probe = Probe {
                channels,
                value: 0.5,
                calls: Vec::new(),
            };
            render_split(&mut probe, &mut buffer[GUARD..end], channels);

            assert!(
                buffer[..GUARD].iter().all(|&s| s == GUARD_VALUE),
                "{channels} ch {frames} frames: head guard"
            );
            assert!(
                buffer[end..].iter().all(|&s| s == GUARD_VALUE),
                "{channels} ch {frames} frames: tail guard"
            );
            assert!(
                buffer[GUARD..end].iter().all(|&s| s == 0.5),
                "{channels} ch {frames} frames: body"
            );

            let expected: Vec<usize> = (0..frames.div_ceil(MAX_CALLBACK_FRAMES))
                .map(|i| (frames - i * MAX_CALLBACK_FRAMES).min(MAX_CALLBACK_FRAMES))
                .collect();
            assert_eq!(
                probe.calls, expected,
                "{channels} ch {frames} frames: calls"
            );
        }
    }
}

#[test]
fn split_zeroes_stale_data_and_clips() {
    // Stale device memory is cleared before the callback sees it, and
    // out-of-range output is clamped as miniaudio's noClip=false did.
    let mut buffer = vec![7.0f32; 2 * 1500];
    let mut probe = Probe {
        channels: 2,
        value: 3.0,
        calls: Vec::new(),
    };
    render_split(&mut probe, &mut buffer, 2);
    assert!(buffer.iter().all(|&s| s == 1.0));
    let mut probe = Probe {
        channels: 2,
        value: -3.0,
        calls: Vec::new(),
    };
    render_split(&mut probe, &mut buffer, 2);
    assert!(buffer.iter().all(|&s| s == -1.0));
    assert_eq!(probe.calls, [1024, 476]);
}

#[test]
fn closures_are_callbacks() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let mut cb = move |out: &mut [f32], frames: usize| {
        c.fetch_add(frames, Ordering::Relaxed);
        out.fill(0.25);
    };
    let mut buffer = vec![0.0f32; 6 * 2000];
    render_split(&mut cb, &mut buffer, 6);
    assert_eq!(count.load(Ordering::Relaxed), 2000);
}

#[test]
fn null_device_formats() {
    let probe = || {
        Box::new(Probe {
            channels: 0,
            value: 0.0,
            calls: vec![],
        }) as Box<dyn RenderCallback>
    };
    for (channels, mask) in [(1u16, 0x4u32), (2, 0x3), (4, 0x33), (6, 0x3f), (8, 0x63f)] {
        let config = NullConfig {
            channels,
            ..NullConfig::default()
        };
        let device = NullDevice::new(config, probe()).expect("supported layout");
        let f = device.format();
        assert_eq!(
            (f.channels, f.channel_mask, f.rate, f.backend),
            (channels, mask, 48000, "Null")
        );
        assert_eq!(f.sample_format, SampleFormat::F32);
        assert_eq!(f.period_frames, 1920);
        assert_eq!(f.buffer_frames, 5760);
        assert!(!f.headphones);
    }
    for channels in [0u16, 3, 5, 7, 9] {
        let config = NullConfig {
            channels,
            ..NullConfig::default()
        };
        assert!(matches!(
            NullDevice::new(config, probe()),
            Err(DeviceError::UnsupportedChannels(_))
        ));
    }
    // Side 5.1 and a mask that does not match the count.
    let config = NullConfig {
        channels: 6,
        channel_mask: 0x60f,
        headphones: true,
        ..NullConfig::default()
    };
    let d = NullDevice::new(config, probe()).expect("5.1 side");
    assert_eq!(d.format().channel_mask, 0x60f);
    assert!(d.format().headphones);
    let config = NullConfig {
        channels: 6,
        channel_mask: 0x3,
        ..NullConfig::default()
    };
    assert!(matches!(
        NullDevice::new(config, probe()),
        Err(DeviceError::InvalidConfig(_))
    ));
}

#[test]
fn null_device_renders_on_demand_with_a_settable_clock() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let mut phase = 0.0f32;
    let cb = move |out: &mut [f32], frames: usize| {
        seen.lock().unwrap().push(frames);
        for frame in out.as_chunks_mut::<8>().0 {
            phase += 1.0;
            frame[0] = phase / 100_000.0;
            frame[7] = 2.0; // clipped
        }
    };
    let config = NullConfig {
        rate: 44100,
        channels: 8,
        ..NullConfig::default()
    };
    let mut device = NullDevice::new(config, Box::new(cb)).expect("7.1");
    assert_eq!(device.format().period_frames, 1764);

    let out = device.render(2500).to_vec();
    assert_eq!(out.len(), 2500 * 8);
    assert_eq!(out[0], 1.0 / 100_000.0);
    assert_eq!(out[7], 1.0);
    assert_eq!(out[(2499) * 8], 2500.0 / 100_000.0);
    assert_eq!(*calls.lock().unwrap(), [1024, 1024, 452]);
    assert_eq!(device.frames_rendered(), 2500);
    assert!(device.render(0).is_empty());

    let clock = device.clock();
    assert_eq!(clock.frames_played(), 0);
    device.set_clock(2500 - 1764);
    assert_eq!(clock.frames_played(), 736);
    // Readable from another thread.
    let c2 = clock.clone();
    assert_eq!(
        std::thread::spawn(move || c2.frames_played())
            .join()
            .unwrap(),
        736
    );

    // Unclipped mode for comparisons with SoLoud's own null driver.
    let config = NullConfig {
        clip: false,
        ..NullConfig::default()
    };
    let mut raw =
        NullDevice::new(config, Box::new(|out: &mut [f32], _: usize| out.fill(1.5))).unwrap();
    assert!(raw.render(10).iter().all(|&s| s == 1.5));
}

#[test]
fn the_engine_gets_its_callback_back() {
    let probe = Probe {
        channels: 2,
        value: 0.1,
        calls: vec![],
    };
    let mut device = NullDevice::new(NullConfig::default(), Box::new(probe)).unwrap();
    device.render(3000);
    let any: Box<dyn std::any::Any> = device.into_callback();
    let probe = any.downcast::<Probe>().expect("same type");
    assert_eq!(probe.calls, [1024, 1024, 952]);
}
