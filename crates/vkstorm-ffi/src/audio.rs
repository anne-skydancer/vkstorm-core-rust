//! Rung six: the audio engine (`vks_audio_*`, `vks_music_*`), the C ABI over
//! `vkstorm-audio` behind `LLAudioEngine_VKS`, `LLAudioChannelVKS`,
//! `LLAudioBufferVKS`, `LLListener_VKS`, `LLStreamingAudio_VKS` and the
//! sound asset decoder of `LLAudioDecodeMgr`.
//!
//! One `VksAudio` object per engine: the output device and its mixer, plus
//! the parcel music player whose reader feeds the mixer's music bus. Mixer
//! calls map one to one onto the calls `LLAudioEngine_SoLoud` made into
//! SoLoud, each taking the mixer lock as SoLoud's calls took its mutex.
//! Voice handles are the mixer's `u32` handles (0 = none). Buffers are
//! reference-counted `Wav`s; releasing one stops the voices still playing
//! it, as `SoLoud::Wav`'s destructor did.
//!
//! Strings come back through callbacks (`VksAudioStrFn`) so no Rust
//! allocation crosses the boundary; the C++ callbacks must not throw.
//! Panics are caught at the boundary and reported as failure values.

use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;

use vkstorm_audio::asset::decode_sound_asset;
use vkstorm_audio::device::{DeviceState, DeviceUuid, NullConfig};
use vkstorm_audio::engine::{DeviceSelection, Engine};
use vkstorm_audio::mixer::{Attenuation, TransitionState, Wav};
use vkstorm_audio::stream::{MusicPlayer, PlayerEvent, PlayerState, StreamConfig};

thread_local! {
    static LAST_ERROR: RefCell<std::ffi::CString> = RefCell::new(std::ffi::CString::default());
}

fn set_error(msg: impl std::fmt::Display) {
    let s = std::ffi::CString::new(msg.to_string().replace('\0', " ")).unwrap_or_default();
    LAST_ERROR.with(|e| *e.borrow_mut() = s);
}

/// The last error of a failed `vks_audio_*` call on this thread (empty if
/// none). Valid until the next failing call on this thread.
#[unsafe(no_mangle)]
pub extern "C" fn vks_audio_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

/// The engine's name, "SoLoud 202002 (Rust)" (`vkstorm_audio::ENGINE_INFO`).
/// Static, NUL-terminated.
#[unsafe(no_mangle)]
pub extern "C" fn vks_audio_engine_info() -> *const c_char {
    static INFO: std::sync::OnceLock<std::ffi::CString> = std::sync::OnceLock::new();
    INFO.get_or_init(|| std::ffi::CString::new(vkstorm_audio::ENGINE_INFO).unwrap_or_default())
        .as_ptr()
}

/// The SoLoud release the mixer is a port of, "202002"
/// (`vkstorm_audio::UPSTREAM_BASELINE.version`). Static, NUL-terminated.
#[unsafe(no_mangle)]
pub extern "C" fn vks_audio_engine_version() -> *const c_char {
    static VERSION: std::sync::OnceLock<std::ffi::CString> = std::sync::OnceLock::new();
    VERSION
        .get_or_init(|| {
            std::ffi::CString::new(vkstorm_audio::UPSTREAM_BASELINE.version).unwrap_or_default()
        })
        .as_ptr()
}

/// Receives a UTF-8 string (not NUL-terminated) of `len` bytes.
pub type VksAudioStrFn =
    Option<unsafe extern "C" fn(user: *mut c_void, s: *const c_char, len: usize)>;

fn give_str(cb: VksAudioStrFn, user: *mut c_void, s: &str) {
    if let Some(f) = cb {
        unsafe { f(user, s.as_ptr() as *const c_char, s.len()) };
    }
}

unsafe fn opt_path(p: *const c_char) -> Option<PathBuf> {
    if p.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

unsafe fn opt_uuid(p: *const u8) -> Option<DeviceUuid> {
    if p.is_null() {
        return None;
    }
    let mut b = [0u8; 16];
    b.copy_from_slice(unsafe { std::slice::from_raw_parts(p, 16) });
    let u = DeviceUuid(b);
    if u.is_null() { None } else { Some(u) }
}

unsafe fn vec3(p: *const f32) -> [f32; 3] {
    if p.is_null() {
        return [0.0; 3];
    }
    let s = unsafe { std::slice::from_raw_parts(p, 3) };
    [s[0], s[1], s[2]]
}

/// The engine object handed to C++.
pub struct VksAudio {
    engine: Engine,
    music: Option<MusicPlayer>,
    /// The serial of the last music transition issued (0 = none).
    serial: u32,
}

impl VksAudio {
    /// Follows the device's rate in the music player after a restart.
    fn sync_music_rate(&self) {
        if let (Some(m), Some(f)) = (&self.music, self.engine.format()) {
            m.set_device_rate(f.rate);
        }
    }
}

macro_rules! with_audio {
    ($a:expr, $default:expr, |$x:ident| $body:expr) => {{
        if $a.is_null() {
            return $default;
        }
        let $x: &mut VksAudio = unsafe { &mut *$a };
        catch_unwind(AssertUnwindSafe(|| $body)).unwrap_or_else(|_| {
            set_error("panic in the audio engine");
            $default
        })
    }};
}

/// Creates the engine on the endpoint `uuid` (16 bytes as `LLUUID::mData`;
/// null or the null UUID = the default endpoint). Null on failure (see
/// `vks_audio_last_error`).
///
/// # Safety
/// `uuid` is null or points to 16 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_create(uuid: *const u8) -> *mut VksAudio {
    let endpoint = unsafe { opt_uuid(uuid) };
    match catch_unwind(|| Engine::new(endpoint)) {
        Ok(Ok(engine)) => Box::into_raw(Box::new(VksAudio {
            engine,
            music: None,
            serial: 0,
        })),
        Ok(Err(e)) => {
            set_error(e);
            std::ptr::null_mut()
        }
        Err(_) => {
            set_error("panic while opening the audio device");
            std::ptr::null_mut()
        }
    }
}

/// Creates the engine on the null device (tests, offline rendering): no
/// hardware, frames rendered on demand by `vks_audio_render_null`.
#[unsafe(no_mangle)]
pub extern "C" fn vks_audio_create_null(
    rate: u32,
    channels: u32,
    mask: u32,
    headphones: i32,
) -> *mut VksAudio {
    let config = NullConfig {
        rate,
        channels: channels as u16,
        channel_mask: mask,
        headphones: headphones != 0,
        // The SoLoud null driver handed back the mixer's output as is.
        clip: false,
    };
    match catch_unwind(|| Engine::new_null(config)) {
        Ok(Ok(engine)) => Box::into_raw(Box::new(VksAudio {
            engine,
            music: None,
            serial: 0,
        })),
        Ok(Err(e)) => {
            set_error(e);
            std::ptr::null_mut()
        }
        Err(_) => {
            set_error("panic while creating the null audio device");
            std::ptr::null_mut()
        }
    }
}

/// Stops the music, the device (its thread joined) and frees the engine.
///
/// # Safety
/// `a` is null or a pointer from `vks_audio_create*`, not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_destroy(a: *mut VksAudio) {
    if a.is_null() {
        return;
    }
    let boxed = unsafe { Box::from_raw(a) };
    let _ = catch_unwind(AssertUnwindSafe(move || {
        let mut boxed = boxed;
        if let Some(m) = boxed.music.take() {
            m.stop();
        }
        boxed.engine.shutdown();
        drop(boxed);
    }));
}

/// `reinitBackend` on the current endpoint. 1 on success.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_restart(a: *mut VksAudio) -> i32 {
    with_audio!(a, 0, |x| {
        let r = x.engine.restart(None);
        x.sync_music_rate();
        match r {
            Ok(()) => 1,
            Err(e) => {
                set_error(e);
                0
            }
        }
    })
}

/// `setDevice`: 0 unchanged, 1 not found (output untouched), 2 switched,
/// 3 requested endpoint failed and the previous one was restored, 4 both
/// failed (no output).
///
/// # Safety
/// `a` is null or a live engine; `uuid` null or 16 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_set_device(a: *mut VksAudio, uuid: *const u8) -> i32 {
    let endpoint = unsafe { opt_uuid(uuid) };
    with_audio!(a, 4, |x| {
        let r = x.engine.set_device(endpoint);
        x.sync_music_rate();
        match r {
            DeviceSelection::Unchanged => 0,
            DeviceSelection::NotFound => 1,
            DeviceSelection::Switched => 2,
            DeviceSelection::RestoredPrevious => 3,
            DeviceSelection::Unavailable => 4,
        }
    })
}

/// Receives one output endpoint: its 16-byte identity and display name.
pub type VksAudioDeviceFn = Option<
    unsafe extern "C" fn(user: *mut c_void, uuid: *const u8, name: *const c_char, name_len: usize),
>;

/// Enumerates the output endpoints (`getDevices`). Returns the count, or -1
/// if enumeration failed.
#[unsafe(no_mangle)]
pub extern "C" fn vks_audio_devices(cb: VksAudioDeviceFn, user: *mut c_void) -> i32 {
    let r = catch_unwind(vkstorm_audio::device::enumerate);
    match r {
        Ok(Ok(list)) => {
            if let Some(f) = cb {
                for d in &list {
                    unsafe {
                        f(
                            user,
                            d.uuid.0.as_ptr(),
                            d.name.as_ptr() as *const c_char,
                            d.name.len(),
                        )
                    };
                }
            }
            list.len() as i32
        }
        Ok(Err(e)) => {
            set_error(e);
            -1
        }
        Err(_) => -1,
    }
}

/// The running output's format.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct VksAudioFormat {
    pub rate: u32,
    pub channels: u32,
    pub channel_mask: u32,
    pub headphones: i32,
    pub period_frames: u32,
    pub buffer_frames: u32,
}

/// Fills `out` and returns 1 when an output runs; 0 otherwise. The backend
/// and device names come through `name_cb` as "backend\0device".
///
/// # Safety
/// `a` is null or a live engine; `out` is valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_format(
    a: *mut VksAudio,
    out: *mut VksAudioFormat,
    name_cb: VksAudioStrFn,
    user: *mut c_void,
) -> i32 {
    if out.is_null() {
        return 0;
    }
    with_audio!(a, 0, |x| match x.engine.format() {
        Some(f) => {
            unsafe {
                *out = VksAudioFormat {
                    rate: f.rate,
                    channels: u32::from(f.channels),
                    channel_mask: f.channel_mask,
                    headphones: f.headphones as i32,
                    period_frames: f.period_frames,
                    buffer_frames: f.buffer_frames,
                }
            };
            give_str(name_cb, user, &format!("{}\0{}", f.backend, f.device_name));
            1
        }
        None => 0,
    })
}

/// Polls the device (call from the main thread's idle): -1 no output,
/// 0 running, 1 detached (default endpoint mode, no endpoint), 2 the
/// selected endpoint was lost, 3 failed.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_poll(a: *mut VksAudio) -> i32 {
    with_audio!(a, -1, |x| match x.engine.poll() {
        None => -1,
        Some(DeviceState::Running) => 0,
        Some(DeviceState::Detached) => 1,
        Some(DeviceState::Lost) => 2,
        Some(DeviceState::Failed) => 3,
    })
}

/// `setInternalGain`: the effects' global volume.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_set_global_volume(a: *mut VksAudio, volume: f32) {
    with_audio!(a, (), |x| x.engine.set_global_volume(volume))
}

/// The mixer's global volume.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_global_volume(a: *mut VksAudio) -> f32 {
    with_audio!(a, 0.0, |x| x.engine.mixer().global_volume())
}

/// `initWind`: 1 when the wind voice runs.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_wind_init(a: *mut VksAudio) -> i32 {
    with_audio!(a, 0, |x| x.engine.init_wind() as i32)
}

/// `cleanupWind`.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_wind_cleanup(a: *mut VksAudio) {
    with_audio!(a, (), |x| x.engine.cleanup_wind())
}

/// The wind generator's targets (lock-free; the audio thread picks them up).
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_wind_targets(
    a: *mut VksAudio,
    freq: f32,
    gain: f32,
    pan_gain_r: f32,
) {
    with_audio!(a, (), |x| x.engine.set_wind_targets(freq, gain, pan_gain_r))
}

/// Renders `frames` on the null device into `out` (interleaved, frames x
/// channels floats). Returns the frames rendered (0 without a null device).
///
/// # Safety
/// `a` is null or a live engine; `out` holds frames x channels floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_render_null(
    a: *mut VksAudio,
    out: *mut f32,
    frames: u32,
) -> u32 {
    if out.is_null() {
        return 0;
    }
    with_audio!(a, 0, |x| {
        let channels = x.engine.format().map_or(0, |f| usize::from(f.channels));
        let v = x.engine.render_null(frames as usize);
        if channels == 0 || v.len() != frames as usize * channels {
            return 0;
        }
        unsafe { std::ptr::copy_nonoverlapping(v.as_ptr(), out, v.len()) };
        frames
    })
}

/// Sets the null device's clock (frames played).
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_set_null_clock(a: *mut VksAudio, frames_played: u64) {
    with_audio!(a, (), |x| x.engine.set_null_clock(frames_played))
}

/// The device clock (frames played; 0 without an output).
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_frames_played(a: *mut VksAudio) -> u64 {
    with_audio!(a, 0, |x| x.engine.clock().map_or(0, |c| c.frames_played()))
}

// ---------------------------------------------------------------------------
// Buffers

/// A decoded sound (`LLAudioBufferVKS`).
pub struct VksAudioWav(Arc<Wav>);

/// `loadWAV`: reads and decodes the WAV file at `path` (UTF-8), with the
/// viewer's source defaults and 64-sample head/tail fade. Null on failure;
/// `*missing` is set to 1 when the file does not exist (the C++ logs that at
/// debug level, as before).
///
/// # Safety
/// `path` is a NUL-terminated string; `missing` is null or valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_wav_load(
    path: *const c_char,
    missing: *mut i32,
) -> *mut VksAudioWav {
    let Some(p) = (unsafe { opt_path(path) }) else {
        return std::ptr::null_mut();
    };
    let r = catch_unwind(AssertUnwindSafe(|| match std::fs::read(&p) {
        Err(e) => {
            if !missing.is_null() {
                unsafe { *missing = (e.kind() == std::io::ErrorKind::NotFound) as i32 };
            }
            set_error(e);
            None
        }
        Ok(bytes) => match vkstorm_audio::wav::load_viewer_buffer(&bytes) {
            Ok(w) => Some(w),
            Err(e) => {
                set_error(e);
                None
            }
        },
    }));
    match r {
        Ok(Some(w)) => Box::into_raw(Box::new(VksAudioWav(w))),
        _ => std::ptr::null_mut(),
    }
}

/// Releases a buffer; voices still playing it on `a`'s mixer stop (as
/// `~SoLoud::Wav` did). `a` may be null (engine gone).
///
/// # Safety
/// `a` is null or a live engine; `w` null or from `vks_audio_wav_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_wav_free(a: *mut VksAudio, w: *mut VksAudioWav) {
    if w.is_null() {
        return;
    }
    let w = unsafe { Box::from_raw(w) };
    if !a.is_null() {
        let x: &mut VksAudio = unsafe { &mut *a };
        let _ = catch_unwind(AssertUnwindSafe(|| {
            x.engine.mixer().stop_audio_source(&w.0)
        }));
    }
    drop(w);
}

/// The sample count per channel (`getLength`).
///
/// # Safety
/// `w` is null or a live buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_wav_sample_count(w: *const VksAudioWav) -> u32 {
    if w.is_null() {
        0
    } else {
        unsafe { &*w }.0.sample_count()
    }
}

/// The length in seconds, computed as SoLoud's `Wav::getLength`.
///
/// # Safety
/// `w` is null or a live buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_wav_length(w: *const VksAudioWav) -> f64 {
    if w.is_null() {
        0.0
    } else {
        unsafe { &*w }.0.length()
    }
}

// ---------------------------------------------------------------------------
// Voices

/// `Soloud::play` (2-D): returns the voice handle (0 if none).
///
/// # Safety
/// `a` is null or a live engine; `w` a live buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_play(
    a: *mut VksAudio,
    w: *const VksAudioWav,
    volume: f32,
    paused: i32,
) -> u32 {
    if w.is_null() {
        return 0;
    }
    let wav = unsafe { &*w }.0.clone();
    with_audio!(a, 0, |x| x.engine.mixer().play(
        &wav,
        volume,
        0.0,
        paused != 0
    ))
}

/// `Soloud::play3d`: returns the voice handle (0 if none).
///
/// # Safety
/// `a` is null or a live engine; `w` a live buffer; `pos`/`vel` 3 floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_play_3d(
    a: *mut VksAudio,
    w: *const VksAudioWav,
    pos: *const f32,
    vel: *const f32,
    volume: f32,
    paused: i32,
) -> u32 {
    if w.is_null() {
        return 0;
    }
    let wav = unsafe { &*w }.0.clone();
    let (p, v) = unsafe { (vec3(pos), vec3(vel)) };
    with_audio!(a, 0, |x| x.engine.mixer().play_3d(
        &wav,
        p,
        v,
        volume,
        paused != 0
    ))
}

macro_rules! voice_fn {
    ($(#[$m:meta])* $name:ident($($arg:ident: $t:ty),*) -> $ret:ty = $default:expr, |$mx:ident, $h:ident| $body:expr) => {
        $(#[$m])*
        ///
        /// # Safety
        /// `a` is null or a live engine.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(a: *mut VksAudio, $h: u32 $(, $arg: $t)*) -> $ret {
            with_audio!(a, $default, |x| {
                #[allow(unused_mut)]
                let mut $mx = x.engine.mixer();
                $body
            })
        }
    };
}

voice_fn!(/// `setVolume`.
    vks_audio_voice_set_volume(volume: f32) -> () = (), |m, h| m.set_volume(h, volume));
voice_fn!(/// `setLooping`.
    vks_audio_voice_set_looping(looping: i32) -> () = (), |m, h| m.set_looping(h, looping != 0));
voice_fn!(/// `setInaudibleBehavior`.
    vks_audio_voice_set_inaudible_behavior(must_tick: i32, kill: i32) -> () = (),
    |m, h| m.set_inaudible_behavior(h, must_tick != 0, kill != 0));
voice_fn!(/// `setPause`.
    vks_audio_voice_set_pause(pause: i32) -> () = (), |m, h| m.set_pause(h, pause != 0));
voice_fn!(/// `seek`: 1 on success.
    vks_audio_voice_seek(seconds: f64) -> i32 = 0, |m, h| m.seek(h, seconds).is_ok() as i32);
voice_fn!(/// `fadeVolume`.
    vks_audio_voice_fade_volume(to: f32, seconds: f64) -> () = (), |m, h| m.fade_volume(h, to, seconds));
voice_fn!(/// `scheduleStop`.
    vks_audio_voice_schedule_stop(seconds: f64) -> () = (), |m, h| m.schedule_stop(h, seconds));
voice_fn!(/// `stop`.
    vks_audio_voice_stop() -> () = (), |m, h| m.stop(h));
voice_fn!(/// `setProtectVoice`.
    vks_audio_voice_set_protect(protect: i32) -> () = (), |m, h| m.set_protect_voice(h, protect != 0));
voice_fn!(/// `isValidVoiceHandle`.
    vks_audio_voice_is_valid() -> i32 = 0, |m, h| m.is_valid_voice_handle(h) as i32);
voice_fn!(/// `getLoopCount`.
    vks_audio_voice_loop_count() -> u32 = 0, |m, h| m.loop_count(h));
voice_fn!(/// `getStreamPosition`.
    vks_audio_voice_stream_position() -> f64 = 0.0, |m, h| m.stream_position(h));
voice_fn!(/// `set3dSourceDopplerFactor`.
    vks_audio_voice_set_doppler_factor(factor: f32) -> () = (), |m, h| m.set_3d_source_doppler_factor(h, factor));
voice_fn!(/// `set3dSourceAttenuation(h, INVERSE_DISTANCE, rolloff)`.
    vks_audio_voice_set_inverse_distance_rolloff(rolloff: f32) -> () = (),
    |m, h| m.set_3d_source_attenuation(h, Attenuation::InverseDistance, rolloff));

/// `set3dSourceParameters` (position and velocity).
///
/// # Safety
/// `a` is null or a live engine; `pos`/`vel` 3 floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_voice_set_3d(
    a: *mut VksAudio,
    h: u32,
    pos: *const f32,
    vel: *const f32,
) {
    let (p, v) = unsafe { (vec3(pos), vec3(vel)) };
    with_audio!(a, (), |x| x
        .engine
        .mixer()
        .set_3d_source_parameters(h, p, v))
}

/// `set3dListenerParameters`.
///
/// # Safety
/// `a` is null or a live engine; the pointers 3 floats each.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_set_listener(
    a: *mut VksAudio,
    pos: *const f32,
    at: *const f32,
    up: *const f32,
    vel: *const f32,
) {
    let (p, t, u, v) = unsafe { (vec3(pos), vec3(at), vec3(up), vec3(vel)) };
    with_audio!(a, (), |x| x
        .engine
        .mixer()
        .set_3d_listener_parameters(p, t, u, v))
}

/// `update3dAudio`.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_update_3d(a: *mut VksAudio) {
    with_audio!(a, (), |x| x.engine.mixer().update_3d_audio())
}

/// `getVoiceCount`.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_voice_count(a: *mut VksAudio) -> u32 {
    with_audio!(a, 0, |x| x.engine.mixer().voice_count())
}

// ---------------------------------------------------------------------------
// Sound asset decoding

/// Decodes a sound asset (Ogg Vorbis) into the decoded cache file's bytes
/// exactly as `LLVorbisDecodeState` assembled them. Returns 0 and sets
/// `*out`/`*out_len` (free with `vks_audio_bytes_free`), or an error code:
/// 1 empty asset, 2 not an Ogg bitstream, 3 bad channel count, 4 no
/// default bitstream, 5 illegal sample count, 6 illegal sample size,
/// 7 decode error in the stream, 8 no samples; `*detail` gets the
/// libvorbis code or the offending value.
///
/// # Safety
/// `data` points to `len` bytes; the out pointers are valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_decode_asset(
    data: *const u8,
    len: usize,
    out: *mut *mut u8,
    out_len: *mut usize,
    detail: *mut i64,
) -> i32 {
    if out.is_null() || out_len.is_null() || detail.is_null() {
        return 1;
    }
    unsafe {
        *out = std::ptr::null_mut();
        *out_len = 0;
        *detail = 0;
    }
    let bytes: &[u8] = if data.is_null() || len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    use vkstorm_audio::asset::AssetDecodeError as E;
    match catch_unwind(|| decode_sound_asset(bytes)) {
        Ok(Ok(wav)) => {
            let b = wav.into_boxed_slice();
            unsafe {
                *out_len = b.len();
                *out = Box::into_raw(b) as *mut u8;
            }
            0
        }
        Ok(Err(e)) => {
            let (code, d) = match e {
                E::EmptyAsset => (1, 0),
                E::NotOgg(c) => (2, i64::from(c)),
                E::BadChannelCount(c) => (3, i64::from(c)),
                E::NoDefaultBitstream => (4, 0),
                E::IllegalSampleCount(c) => (5, i64::from(c)),
                E::IllegalSampleSize(s) => (6, s as i64),
                E::DecodeError(c) => (7, i64::from(c)),
                E::EmptyDecode => (8, 0),
            };
            unsafe { *detail = d };
            code
        }
        Err(_) => 7,
    }
}

/// Frees bytes returned by `vks_audio_decode_asset`.
///
/// # Safety
/// `p`/`len` come from `vks_audio_decode_asset`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_bytes_free(p: *mut u8, len: usize) {
    if !p.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(p, len)) });
    }
}

/// The vendor string of the asset's Vorbis comment header, for the warning
/// the C++ logs on a rejected asset ("Bad asset encoded by: ...").
///
/// # Safety
/// `data` points to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_audio_asset_vendor(
    data: *const u8,
    len: usize,
    cb: VksAudioStrFn,
    user: *mut c_void,
) -> i32 {
    if data.is_null() || len == 0 {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    let r = catch_unwind(|| {
        let vf = vkstorm_vorbis::VorbisFile::open(bytes).ok()?;
        let c = vf.comment(-1)?;
        Some(c.vendor_str().into_owned())
    });
    match r {
        Ok(Some(v)) => {
            give_str(cb, user, &v);
            1
        }
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Parcel music

/// Creates the music player (once per engine). `ca_bundle`, `user_agent` and
/// `ffmpeg_dir` are NUL-terminated UTF-8 (null = none). 1 on success.
///
/// # Safety
/// `a` is null or a live engine; the strings null or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_create(
    a: *mut VksAudio,
    ca_bundle: *const c_char,
    user_agent: *const c_char,
    ffmpeg_dir: *const c_char,
) -> i32 {
    let ca = unsafe { opt_path(ca_bundle) };
    let ff = unsafe { opt_path(ffmpeg_dir) };
    let ua = if user_agent.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(user_agent) }
            .to_string_lossy()
            .into_owned()
    };
    with_audio!(a, 0, |x| {
        if x.music.is_some() {
            return 1;
        }
        let mut cfg = StreamConfig::default();
        cfg.device_rate = x.engine.format().map_or(48000, |f| f.rate);
        cfg.ca_bundle = ca.clone();
        if !ua.is_empty() {
            cfg.user_agent = ua.clone();
        }
        cfg.ffmpeg_dir = ff.clone();
        x.music = Some(MusicPlayer::new(cfg));
        1
    })
}

/// Starts `url` (UTF-8, NUL-terminated) and attaches its reader to the
/// mixer's music bus. An empty URL stops.
///
/// # Safety
/// `a` is null or a live engine; `url` NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_start(a: *mut VksAudio, url: *const c_char) {
    if url.is_null() {
        return;
    }
    let url = unsafe { CStr::from_ptr(url) }
        .to_string_lossy()
        .into_owned();
    with_audio!(a, (), |x| {
        if let Some(m) = &x.music {
            m.start(&url);
            let old = x.engine.attach_music(Box::new(m.reader()));
            drop(old);
        }
    })
}

/// Stops the music and detaches it from the bus.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_stop(a: *mut VksAudio) {
    with_audio!(a, (), |x| {
        if let Some(m) = &x.music {
            m.stop();
        }
        drop(x.engine.detach_music());
    })
}

/// Pauses (non-zero) or resumes (0).
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_pause(a: *mut VksAudio, pause: i32) {
    with_audio!(a, (), |x| {
        if let Some(m) = &x.music {
            m.pause(pause != 0);
        }
    })
}

/// The viewer's `isPlaying()`: 0 stopped, 1 playing (or connecting,
/// buffering), 2 paused.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_is_playing(a: *mut VksAudio) -> i32 {
    with_audio!(a, 0, |x| x.music.as_ref().map_or(0, |m| m.is_playing()))
}

/// The player state: 0 idle, 1 connecting, 2 buffering, 3 playing,
/// 4 paused, 5 ended, 6 failed (the reason through `cb`), 7 stopped.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_state(
    a: *mut VksAudio,
    cb: VksAudioStrFn,
    user: *mut c_void,
) -> i32 {
    with_audio!(a, 0, |x| match x.music.as_ref().map(|m| m.state()) {
        None | Some(PlayerState::Idle) => 0,
        Some(PlayerState::Connecting) => 1,
        Some(PlayerState::Buffering) => 2,
        Some(PlayerState::Playing) => 3,
        Some(PlayerState::Paused) => 4,
        Some(PlayerState::Ended) => 5,
        Some(PlayerState::Failed(reason)) => {
            give_str(cb, user, &reason);
            6
        }
        Some(PlayerState::Stopped) => 7,
    })
}

/// The URL playing (through `cb`; nothing when stopped).
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_url(a: *mut VksAudio, cb: VksAudioStrFn, user: *mut c_void) {
    with_audio!(a, (), |x| {
        if let Some(m) = &x.music {
            give_str(cb, user, &m.url());
        }
    })
}

/// Receives metadata: `kind` 1 = artist and title, 0 = cleared (empty map).
pub type VksMusicMetaFn = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        kind: i32,
        artist: *const c_char,
        artist_len: usize,
        title: *const c_char,
        title_len: usize,
    ),
>;

/// Delivers the next metadata event to `cb`; 1 if there was one.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_poll_metadata(
    a: *mut VksAudio,
    cb: VksMusicMetaFn,
    user: *mut c_void,
) -> i32 {
    with_audio!(a, 0, |x| {
        let Some(ev) = x.music.as_ref().and_then(|m| m.poll_event()) else {
            return 0;
        };
        if let Some(f) = cb {
            match ev {
                PlayerEvent::Metadata { artist, title } => unsafe {
                    f(
                        user,
                        1,
                        artist.as_ptr() as *const c_char,
                        artist.len(),
                        title.as_ptr() as *const c_char,
                        title.len(),
                    )
                },
                PlayerEvent::MetadataCleared => unsafe {
                    f(user, 0, std::ptr::null(), 0, std::ptr::null(), 0)
                },
            }
        }
        1
    })
}

/// The current metadata (`getCurrentMetadata`) to `cb`; 0 when empty.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_current_metadata(
    a: *mut VksAudio,
    cb: VksMusicMetaFn,
    user: *mut c_void,
) -> i32 {
    with_audio!(
        a,
        0,
        |x| match x.music.as_ref().and_then(|m| m.current_metadata()) {
            Some((artist, title)) => {
                if let Some(f) = cb {
                    unsafe {
                        f(
                            user,
                            1,
                            artist.as_ptr() as *const c_char,
                            artist.len(),
                            title.as_ptr() as *const c_char,
                            title.len(),
                        )
                    };
                }
                1
            }
            None => 0,
        }
    )
}

/// The music's user gain (0..1; applied as volume^3 with a 50 ms ramp).
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_set_volume(a: *mut VksAudio, volume: f32) {
    with_audio!(a, (), |x| {
        x.engine.mixer().music.set_volume(volume);
    })
}

/// The music's hard mute (independent of gain and envelope).
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_set_muted(a: *mut VksAudio, muted: i32) {
    with_audio!(a, (), |x| x.engine.mixer().music.set_muted(muted != 0))
}

/// Sets the fade envelope immediately (before the first sample; the login
/// hold). 1 if accepted.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_set_envelope(a: *mut VksAudio, value: f32) -> i32 {
    with_audio!(a, 0, |x| x.engine.mixer().music.set_envelope(value) as i32)
}

/// Starts a fade transition to `target` (0..1) over `seconds` (0..60):
/// returns its serial (non-zero), or 0 if refused.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_transition(a: *mut VksAudio, target: f32, seconds: f32) -> u32 {
    with_audio!(a, 0, |x| {
        let serial = x.serial.wrapping_add(1).max(1);
        if x.engine.mixer().music.transition(serial, target, seconds) {
            x.serial = serial;
            serial
        } else {
            0
        }
    })
}

/// A transition's state against the device clock: 0 none (replaced or
/// cancelled), 1 pending, 2 complete.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_transition_state(a: *mut VksAudio, serial: u32) -> i32 {
    with_audio!(a, 0, |x| match x.engine.music_transition_state(serial) {
        TransitionState::Idle => 0,
        TransitionState::Pending => 1,
        TransitionState::Complete => 2,
    })
}

/// FSMusicSpatialSound (0, 21, 41, 51, 71): the speaker fill applied to
/// stereo music, immediately. 1 if the value is valid.
///
/// # Safety
/// `a` is null or a live engine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_set_spatial_layout(a: *mut VksAudio, layout: i32) -> i32 {
    with_audio!(a, 0, |x| x.engine.mixer().music.set_spatial_layout(layout)
        as i32)
}

/// Music counters for diagnostics and tests.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct VksMusicStats {
    /// Frames the decoder pushed (at the device rate) and the mixer pulled.
    pub pushed_frames: u64,
    pub pulled_frames: u64,
    pub underruns: u64,
    pub reconnects: u32,
    pub buffered_frames: u64,
    /// The bus: frames taken from the source, frames written, silent frames.
    pub bus_input_frames: u64,
    pub bus_output_frames: u64,
    pub bus_underrun_frames: u64,
    /// Over the decoded segments without a device rate change: their
    /// source frames, the frames they produced at the device rate, and what
    /// exact conversion predicts (ceil(source x device / source rate) per
    /// segment).
    pub source_frames: u64,
    pub device_frames: u64,
    pub expected_device_frames: u64,
}

/// Fills `out`; the codec name comes through `cb`.
///
/// # Safety
/// `a` is null or a live engine; `out` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_music_stats(
    a: *mut VksAudio,
    out: *mut VksMusicStats,
    cb: VksAudioStrFn,
    user: *mut c_void,
) -> i32 {
    if out.is_null() {
        return 0;
    }
    with_audio!(a, 0, |x| {
        let mut s = VksMusicStats::default();
        {
            let m = x.engine.mixer();
            s.bus_input_frames = m.music.input_frames();
            s.bus_output_frames = m.music.output_frames();
            s.bus_underrun_frames = m.music.underrun_frames();
        }
        if let Some(m) = &x.music {
            let st = m.stats();
            s.pushed_frames = st.pushed_frames;
            s.pulled_frames = st.pulled_frames;
            s.underruns = st.underruns;
            s.reconnects = st.reconnects;
            s.buffered_frames = st.buffered_frames as u64;
            for seg in st
                .segments
                .iter()
                .filter(|g| g.source_rate > 0 && !g.rate_changed)
            {
                s.source_frames += seg.in_frames;
                s.device_frames += seg.out_frames;
                s.expected_device_frames +=
                    (seg.in_frames * u64::from(seg.out_rate)).div_ceil(u64::from(seg.source_rate));
            }
            give_str(cb, user, st.codec);
        }
        unsafe { *out = s };
        1
    })
}
