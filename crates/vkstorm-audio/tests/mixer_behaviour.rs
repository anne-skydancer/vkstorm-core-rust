//! Mixer behaviour beyond the oracle scenes: handles, the voice limit with
//! ticking voices (where SoLoud itself crashes), chunked rendering, the
//! floating-point mode, declicked stops.

use std::sync::Arc;

use vkstorm_audio::mixer::{Attenuation, Mixer, MixerConfig, Wav};

fn tone(frames: usize) -> Arc<Wav> {
    let data: Vec<f32> = (0..frames)
        .map(|i| ((i as f32) * 0.05).sin() * 0.5)
        .collect();
    let mut w = Wav::from_planar(data, 44100.0, 1).unwrap();
    w.apply_viewer_defaults();
    Arc::new(w)
}

#[test]
fn handles_carry_generations() {
    let mut m = Mixer::new(MixerConfig::viewer(48000, 2)).unwrap();
    let w = tone(1000);
    let a = m.play(&w, 1.0, 0.0, false);
    assert_eq!(a & 0xfff, 1, "slot 0");
    m.stop(a);
    let b = m.play(&w, 1.0, 0.0, false);
    assert_eq!(b & 0xfff, 1, "slot reused");
    assert_ne!(a, b);
    assert!(!m.is_valid_voice_handle(a) && m.is_valid_voice_handle(b));
    assert!(!m.is_valid_voice_handle(0));
    // A replacement mixer continues the play index: old handles stay
    // invalid there too.
    let mut c = MixerConfig::viewer(48000, 2);
    c.first_play_index = m.play_index();
    let mut m2 = Mixer::new(c).unwrap();
    let d = m2.play(&w, 1.0, 0.0, false);
    assert!(!m2.is_valid_voice_handle(b) && m2.is_valid_voice_handle(d));
}

#[test]
fn more_than_64_ticking_voices_do_not_crash() {
    // The adapter makes every channel voice tick when inaudible; SoLoud
    // crashes once more than 64 of them coexist (null resample buffer).
    let mut m = Mixer::new(MixerConfig::viewer(44100, 2)).unwrap();
    let w = tone(4410);
    for i in 0..80 {
        let h = m.play_3d(&w, [i as f32, 0.0, -1.0], [0.0; 3], 0.5, true);
        m.set_looping(h, true);
        m.set_inaudible_behavior(h, true, false);
        m.set_pause(h, false);
    }
    m.update_3d_audio();
    let mut out = vec![0.0f32; 1024 * 2];
    for _ in 0..10 {
        m.mix(&mut out, 1024);
        assert!(out.iter().all(|s| s.is_finite()));
    }
    assert_eq!(m.active_voice_count(), 64);
    assert!(out.iter().any(|&s| s != 0.0));
}

#[test]
fn render_is_mix_in_1024_frame_chunks() {
    let w = tone(20000);
    let setup = |m: &mut Mixer| {
        let h = m.play_3d(&w, [3.0, 0.0, -2.0], [0.0; 3], 0.8, true);
        m.set_3d_source_attenuation(h, Attenuation::InverseDistance, 1.0);
        m.set_pause(h, false);
    };
    let mut a = Mixer::new(MixerConfig::viewer(48000, 6)).unwrap();
    let mut b = Mixer::new(MixerConfig::viewer(48000, 6)).unwrap();
    setup(&mut a);
    setup(&mut b);
    let mut ra = vec![0.0f32; 1920 * 6];
    a.render(&mut ra, 1920);
    let mut rb = vec![0.0f32; 1920 * 6];
    b.mix(&mut rb[..1024 * 6], 1024);
    b.mix(&mut rb[1024 * 6..], 896);
    assert_eq!(ra, rb);
}

#[test]
fn mix_restores_the_callers_float_mode() {
    let mut m = Mixer::new(MixerConfig::viewer(48000, 2)).unwrap();
    let mut out = vec![0.0f32; 64];
    m.mix(&mut out, 32);
    // A denormal survives arithmetic again after the mix.
    let tiny = std::hint::black_box(f32::MIN_POSITIVE);
    assert!(tiny / std::hint::black_box(4.0f32) != 0.0);
}

#[test]
fn declicked_stop_fades_then_retires() {
    let mut m = Mixer::new(MixerConfig::viewer(48000, 2)).unwrap();
    let w = tone(48000);
    let h = m.play(&w, 1.0, 0.0, false);
    let mut out = vec![0.0f32; 480 * 2];
    m.mix(&mut out, 480);
    m.fade_volume(h, 0.0, 0.02);
    m.schedule_stop(h, 0.02);
    m.mix(&mut out, 480); // 10 ms: still fading
    assert!(m.is_valid_voice_handle(h));
    assert!(m.volume(h) > 0.0 && m.volume(h) < 1.0);
    m.mix(&mut out, 480);
    m.mix(&mut out, 480); // past 20 ms of stream time
    assert!(!m.is_valid_voice_handle(h));
}

#[test]
fn length_is_computed_in_float() {
    let w = Wav::from_planar(vec![0.0; 22051], 44100.0, 1).unwrap();
    assert_eq!(w.length(), (22051.0f32 / 44100.0f32) as f64);
}
