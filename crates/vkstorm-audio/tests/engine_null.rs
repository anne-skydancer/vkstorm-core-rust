//! The engine on the null device: music through the device callback with a
//! settable device clock, the login hold and fade-in, device selection of
//! an unknown endpoint.

use vkstorm_audio::device::{DeviceUuid, NullConfig};
use vkstorm_audio::engine::{DeviceSelection, Engine};
use vkstorm_audio::mixer::TransitionState;
use vkstorm_audio::music::{ChannelLayout, MusicSource, Pulled};

struct Constant(f32);

impl MusicSource for Constant {
    fn pull(&mut self, out: &mut [f32], max_frames: usize) -> Pulled {
        out[..max_frames * 2].fill(self.0);
        Pulled {
            frames: max_frames,
            layout: ChannelLayout::STEREO,
            ended: false,
        }
    }
}

fn engine() -> Engine {
    Engine::new_null(NullConfig {
        rate: 48000,
        channels: 2,
        channel_mask: 0,
        headphones: false,
        clip: true,
    })
    .unwrap()
}

#[test]
fn login_hold_then_fade_in_completes_on_the_device_clock() {
    let mut e = engine();
    e.set_global_volume(1.0);
    {
        let mut m = e.mixer();
        assert!(m.music.set_envelope(0.0), "login hold at zero");
        assert!(m.music.set_volume(1.0));
    }
    assert!(e.attach_music(Box::new(Constant(0.5))).is_none());
    let held = e.render_null(2048);
    assert!(held.iter().all(|&s| s == 0.0), "held at exactly zero");
    assert!(e.mixer().music.transition(7, 1.0, 0.1));
    // 4800 frames of ramp; the 4801st carries the target.
    let ramp = e.render_null(4801);
    assert_eq!(ramp[0], 0.0, "fade-in starts at exactly zero");
    assert!(ramp[2 * 2400] > 0.24 && ramp[2 * 2400] < 0.26);
    assert_eq!(ramp[2 * 4800], 0.5);
    // The fence is in mixer frames: 2048 + 4800.
    assert_eq!(e.mixer().music.transition_fence(), Some(2048 + 4800));
    e.set_null_clock(2048 + 4800);
    assert_eq!(
        e.music_transition_state(7),
        TransitionState::Pending,
        "queued, not yet played"
    );
    e.set_null_clock(2048 + 4801);
    assert_eq!(e.music_transition_state(7), TransitionState::Complete);
    assert_eq!(e.music_transition_state(6), TransitionState::Idle);
    drop(e.detach_music());
    assert_eq!(
        e.music_transition_state(7),
        TransitionState::Idle,
        "stop cancels"
    );
}

#[test]
fn music_is_independent_of_the_effects_volume() {
    let mut e = engine();
    e.set_global_volume(0.0);
    e.mixer().music.set_volume(1.0);
    e.attach_music(Box::new(Constant(0.25)));
    let out = e.render_null(64);
    assert!(out.iter().all(|&s| s == 0.25));
}

#[test]
fn unknown_endpoint_leaves_output_unchanged() {
    let mut e = engine();
    let r = e.set_device(Some(DeviceUuid([0x5a; 16])));
    assert_eq!(r, DeviceSelection::NotFound);
    assert!(e.is_available());
    assert_eq!(e.format().unwrap().backend, "Null");
}
