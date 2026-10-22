//! `indra/llaudio/tests/llsoloud_adapter_test.cpp` transcribed: the same
//! checks against the engine on the null device at 2, 6 and 8 channels
//! (the C++ test opened SoLoud's null backend at those counts). The
//! adapter's channel logic (startVoicePaused, playSynced, update3DPosition,
//! declicked stops) is the lead's C++ side; here it is spelled out with
//! the mixer calls it makes.
//!
//! Not transcribed: `createDefaultStreamingAudioImpl() == nullptr` (music
//! now plays through the engine) and "media updates survive an unavailable
//! effects device" (an LLAudioEngine::idle behaviour, C++ side).

use std::sync::Arc;

use vkstorm_audio::device::NullConfig;
use vkstorm_audio::engine::Engine;
use vkstorm_audio::mixer::{Attenuation, Handle, Mixer, Wav};

fn null(channels: u16) -> NullConfig {
    NullConfig {
        rate: 44100,
        channels,
        channel_mask: 0,
        headphones: false,
        clip: false,
    }
}

/// The C++ test's TestBuffer: 512 samples of 0.1 at 44.1 kHz, mono,
/// inverse distance with rolloff 1 (min/max distance left at SoLoud's
/// defaults, 1 and 1e6).
fn test_buffer() -> Arc<Wav> {
    let mut w = Wav::from_planar(vec![0.1; 512], 44100.0, 1).unwrap();
    w.attenuation = Attenuation::InverseDistance;
    w.rolloff = 1.0;
    Arc::new(w)
}

/// LLAudioChannelSoLoud::startVoicePaused for a positional source.
fn play_3d(
    m: &mut Mixer,
    wav: &Arc<Wav>,
    gain: f32,
    looping: bool,
    pos: [f32; 3],
    doppler: f32,
    rolloff: f32,
) -> Handle {
    let h = m.play_3d(wav, pos, [0.0; 3], gain, true);
    m.set_3d_source_doppler_factor(h, doppler);
    if rolloff != 1.0 {
        m.set_3d_source_attenuation(h, Attenuation::InverseDistance, rolloff);
    }
    m.set_looping(h, looping);
    m.set_inaudible_behavior(h, true, false);
    m.update_3d_audio();
    h
}

fn declick(m: &mut Mixer, h: Handle) {
    m.fade_volume(h, 0.0, 0.02);
    m.schedule_stop(h, 0.02);
}

#[test]
fn adapter_behaviour_at_2_6_8_channels() {
    for channels in [2u16, 6, 8] {
        let mut engine = Engine::new_null(null(channels)).expect("null device");
        assert_eq!(
            engine.format().unwrap().backend,
            "Null",
            "no physical device"
        );
        assert_eq!(
            engine.mixer().global_volume(),
            0.0,
            "startup must be silent"
        );
        engine.set_global_volume(0.5);
        assert_eq!(engine.mixer().global_volume(), 0.5, "master gain");
        engine.set_global_volume(0.0);
        assert_eq!(engine.mixer().global_volume(), 0.0, "mute");
        engine.set_global_volume(0.5);
        assert_eq!(engine.mixer().global_volume(), 0.5, "unmute");

        if channels == 8 {
            assert_eq!(
                engine.mixer().speaker_position(4).unwrap()[2],
                -1.0,
                "Windows 7.1 back channel order"
            );
            assert_eq!(
                engine.mixer().speaker_position(6).unwrap()[2],
                0.0,
                "Windows 7.1 side channel order"
            );
        }

        // setListener(zero, velocity (1,2,3), up, at) + commitDeferredChanges
        {
            let mut m = engine.mixer();
            m.set_3d_listener_parameters(
                [0.0; 3],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 2.0, 3.0],
            );
            m.update_3d_audio();
            let v = m.listener_velocity();
            assert!(v[0] == 1.0 && v[2] == 3.0, "listener velocity");
        }

        assert!(engine.init_wind(), "wind initialization");
        engine.set_wind_targets(120.0, 0.4, 0.3);
        let frames = 1024usize;
        let ch = channels as usize;
        let mut samples = vec![12345.0f32; frames * ch + 2];
        engine.mixer().mix(&mut samples[1..1 + frames * ch], frames);
        assert!(
            samples[0] == 12345.0 && samples[frames * ch + 1] == 12345.0,
            "mix bounds"
        );
        assert!(
            samples[1..1 + frames * ch].iter().all(|s| s.is_finite()),
            "finite wind output"
        );
        engine.cleanup_wind();
        assert_eq!(engine.mixer().voice_count(), 0, "wind retirement");

        {
            let buffer = test_buffer();
            let mut m = engine.mixer();
            // source gain 0.8 x secondary 0.5; rolloff 2; Doppler 0
            let master = play_3d(&mut m, &buffer, 0.4, true, [10.0, 0.0, 0.0], 0.0, 2.0);
            m.set_pause(master, false);
            assert!(
                m.is_valid_voice_handle(master) && m.looping(master),
                "looping channel start"
            );
            assert!(
                (m.volume(master) - 0.4).abs() < 0.0001,
                "category gain at start"
            );
            assert!(
                (m.overall_volume(master) - 0.4 / 19.0).abs() < 0.0001,
                "rolloff committed before unpause"
            );
            // secondary gain 0.25, rolloff 1: update3DPosition + commit
            m.set_3d_source_parameters(master, [10.0, 0.0, 0.0], [0.0; 3]);
            m.set_3d_source_doppler_factor(master, 0.0);
            m.set_3d_source_attenuation(master, Attenuation::InverseDistance, 1.0);
            m.set_volume(master, 0.2);
            m.update_3d_audio();
            assert!(
                (m.overall_volume(master) - 0.02).abs() < 0.0001,
                "live category gain and rolloff"
            );
            let loops_before = m.loop_count(master);
            m.mix(&mut samples[1..1 + frames * ch], frames);
            assert_ne!(
                m.loop_count(master),
                loops_before,
                "loop completion detection"
            );
            // playSynced
            let mut offset = m.stream_position(master);
            let length = buffer.length();
            if length > 0.0 {
                offset %= length;
            }
            let slave = play_3d(&mut m, &buffer, 0.2, true, [10.0, 0.0, 0.0], 0.0, 1.0);
            m.set_volume(slave, 0.0);
            m.seek(slave, offset).unwrap();
            m.fade_volume(slave, 0.2, 0.02);
            m.set_pause(slave, false);
            assert!(m.is_valid_voice_handle(slave), "synchronized channel start");
            m.set_looping(master, false);
            assert!(!m.looping(master), "loop toggle");
            declick(&mut m, master);
            declick(&mut m, slave);
            // ~Wav stops any voices still playing this buffer.
            m.stop_audio_source(&buffer);
            assert_eq!(
                m.voice_count(),
                0,
                "buffer destruction retires faded voices"
            );
        }

        let retained = test_buffer();
        for _ in 0..5 {
            let voice = engine.mixer().play(&retained, -1.0, 0.0, false);
            assert!(
                engine.mixer().is_valid_voice_handle(voice),
                "retained WAV playback"
            );
            engine
                .restart(Some(null(channels)))
                .expect("null backend restart");
            assert_eq!(
                engine.mixer().voice_count(),
                0,
                "restart retires old voices"
            );
            assert!(
                !engine.mixer().is_valid_voice_handle(voice),
                "old handles stay invalid"
            );
            assert_eq!(engine.mixer().global_volume(), 0.5, "restart restores gain");
        }
        engine.shutdown();
        assert!(!engine.is_available(), "engine retirement");
    }
}

#[test]
fn restart_restores_wind_and_moves_music() {
    let mut engine = Engine::new_null(null(2)).unwrap();
    engine.init_wind();
    let wind = engine.wind_handle().unwrap();
    assert!(engine.mixer().protect_voice(wind));
    engine.set_wind_targets(200.0, 0.5, 0.5);
    engine.mixer().music.set_envelope(0.0);
    engine.restart(Some(null(6))).unwrap();
    assert_eq!(engine.format().unwrap().channels, 6);
    let wind2 = engine.wind_handle().unwrap();
    assert!(engine.mixer().is_valid_voice_handle(wind2) && engine.mixer().protect_voice(wind2));
    assert_ne!(wind, wind2);
    assert_eq!(
        engine.mixer().music.envelope(),
        0.0,
        "music bus state survives a restart"
    );
    let out = engine.render_null(4096);
    assert_eq!(out.len(), 4096 * 6);
    assert!(out.iter().all(|s| s.is_finite()));
}
