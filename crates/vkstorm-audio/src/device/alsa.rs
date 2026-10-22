//! ALSA output (Linux), libasound loaded at run time; used when PulseAudio
//! is unavailable.
//!
//! Reproduces miniaudio 0.10.42's ALSA backend for the default device:
//!
//! - `libasound.so.2` (or `libasound.so`) opened with dlopen.
//! - The PCM: "default", else "dmix", "dmix:0", "dmix:0,0", "hw", "hw:0",
//!   "hw:0,0" (shared-mode playback list), opened blocking.
//! - Hardware parameters in miniaudio's order: interleaved read/write
//!   access; the requested float format if the PCM takes it, else the first
//!   of s16, f32, s32, s24 (packed), u8; channels near 2 (miniaudio's
//!   `MA_DEFAULT_CHANNELS`: with `channels = 0` it asked ALSA for stereo,
//!   unlike the other backends); ALSA resampling off and rate near 48000;
//!   periods near 3; buffer near 3 x 40 ms; period = buffer / periods.
//!   Software parameters: avail_min = the period rounded down to a power of
//!   two, start threshold 2 periods, stop threshold = boundary (an xrun
//!   loops instead of stopping).
//! - The channel map from `snd_pcm_get_chmap`, else miniaudio's standard
//!   ALSA map for the count; channels are reordered from the mixer's mask
//!   order accordingly (see `layout.rs`).
//! - Writing: per period, chunks of at most 4096 bytes of device frames
//!   (miniaudio's stack buffer), each rendered then written; an underrun
//!   (-EPIPE) is recovered and the PCM restarted. Instead of miniaudio's
//!   poll + eventfd, the thread waits with `snd_pcm_wait` in 100 ms slices
//!   and only writes what is free, so stopping never blocks in writei.
//! - Device clock: frames written minus `snd_pcm_delay`.
//! - Headphones: unknown on ALSA (false).

use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::Arc;

use libloading::Library;

use super::convert::SampleFormat;
use super::layout::{
    DeviceOrder, alsa_position_bit, alsa_standard_positions, is_supported_channel_count,
};
use super::render::{CONVERTER_BUFFER_BYTES, Renderer};
use super::{
    DeviceError, DeviceFormat, DeviceState, OpenedDevice, PERIOD_MILLISECONDS, PERIODS,
    RenderCallback, Shared, StartCommand, ThreadContext, frames_from_ms, spawn_backend,
};

const BACKEND: &str = "ALSA";

// alsa/pcm.h
const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
const SND_PCM_FORMAT_U8: c_int = 1;
const SND_PCM_FORMAT_S16_LE: c_int = 2;
const SND_PCM_FORMAT_S32_LE: c_int = 10;
const SND_PCM_FORMAT_FLOAT_LE: c_int = 14;
const SND_PCM_FORMAT_S24_3LE: c_int = 32;
// errno (Linux)
const EAGAIN: c_long = 11;
const ENODEV: c_long = 19;
const EPIPE: c_long = 32;
const ESTRPIPE: c_long = 86;

/// Shared-mode default PCM names, in miniaudio's order.
const DEFAULT_PCMS: [&CStr; 7] = [
    c"default",
    c"dmix",
    c"dmix:0",
    c"dmix:0,0",
    c"hw",
    c"hw:0",
    c"hw:0,0",
];

/// `g_maFormatPriorities` (after the requested f32).
const FORMAT_PRIORITIES: [(c_int, SampleFormat); 5] = [
    (SND_PCM_FORMAT_S16_LE, SampleFormat::S16),
    (SND_PCM_FORMAT_FLOAT_LE, SampleFormat::F32),
    (SND_PCM_FORMAT_S32_LE, SampleFormat::S32),
    (SND_PCM_FORMAT_S24_3LE, SampleFormat::S24),
    (SND_PCM_FORMAT_U8, SampleFormat::U8),
];

type Pcm = c_void;
type HwParams = c_void;
type SwParams = c_void;

macro_rules! alsa_api {
    ($($name:ident : fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        /// The libasound entry points used, plus libc `free` for the
        /// channel map ALSA allocates.
        struct Api {
            $($name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
            free: unsafe extern "C" fn(*mut c_void),
            _lib: Library,
        }

        impl Api {
            fn load() -> Result<Api, String> {
                let mut last = String::new();
                for name in ["libasound.so.2", "libasound.so"] {
                    // SAFETY: libasound is a system library meant for dlopen.
                    match unsafe { Library::new(name) } {
                        Ok(lib) => {
                            // SAFETY: symbols resolved with the exact C
                            // signatures of alsa/pcm.h; kept with the Library
                            // that owns them. `free` comes from the process's
                            // libc (the allocator snd_pcm_get_chmap uses).
                            unsafe {
                                $(let $name = *lib
                                    .get::<unsafe extern "C" fn($($arg),*) $(-> $ret)?>(stringify!($name))
                                    .map_err(|e| format!("{}: {e}", stringify!($name)))?;)*
                                let free = *libloading::os::unix::Library::this()
                                    .get::<unsafe extern "C" fn(*mut c_void)>("free")
                                    .map_err(|e| format!("free: {e}"))?;
                                return Ok(Api { $($name,)* free, _lib: lib });
                            }
                        }
                        Err(e) => last = e.to_string(),
                    }
                }
                Err(last)
            }
        }
    };
}

alsa_api! {
    snd_pcm_open: fn(*mut *mut Pcm, *const c_char, c_int, c_int) -> c_int;
    snd_pcm_close: fn(*mut Pcm) -> c_int;
    snd_pcm_name: fn(*mut Pcm) -> *const c_char;
    snd_pcm_hw_params_malloc: fn(*mut *mut HwParams) -> c_int;
    snd_pcm_hw_params_free: fn(*mut HwParams);
    snd_pcm_hw_params_any: fn(*mut Pcm, *mut HwParams) -> c_int;
    snd_pcm_hw_params_set_access: fn(*mut Pcm, *mut HwParams, c_int) -> c_int;
    snd_pcm_hw_params_test_format: fn(*mut Pcm, *mut HwParams, c_int) -> c_int;
    snd_pcm_hw_params_set_format: fn(*mut Pcm, *mut HwParams, c_int) -> c_int;
    snd_pcm_hw_params_set_channels_near: fn(*mut Pcm, *mut HwParams, *mut c_uint) -> c_int;
    snd_pcm_hw_params_set_rate_resample: fn(*mut Pcm, *mut HwParams, c_uint) -> c_int;
    snd_pcm_hw_params_set_rate_near: fn(*mut Pcm, *mut HwParams, *mut c_uint, *mut c_int) -> c_int;
    snd_pcm_hw_params_set_periods_near: fn(*mut Pcm, *mut HwParams, *mut c_uint, *mut c_int) -> c_int;
    snd_pcm_hw_params_set_buffer_size_near: fn(*mut Pcm, *mut HwParams, *mut c_ulong) -> c_int;
    snd_pcm_hw_params: fn(*mut Pcm, *mut HwParams) -> c_int;
    snd_pcm_sw_params_malloc: fn(*mut *mut SwParams) -> c_int;
    snd_pcm_sw_params_free: fn(*mut SwParams);
    snd_pcm_sw_params_current: fn(*mut Pcm, *mut SwParams) -> c_int;
    snd_pcm_sw_params_set_avail_min: fn(*mut Pcm, *mut SwParams, c_ulong) -> c_int;
    snd_pcm_sw_params_get_boundary: fn(*const SwParams, *mut c_ulong) -> c_int;
    snd_pcm_sw_params_set_start_threshold: fn(*mut Pcm, *mut SwParams, c_ulong) -> c_int;
    snd_pcm_sw_params_set_stop_threshold: fn(*mut Pcm, *mut SwParams, c_ulong) -> c_int;
    snd_pcm_sw_params: fn(*mut Pcm, *mut SwParams) -> c_int;
    snd_pcm_get_chmap: fn(*mut Pcm) -> *mut c_uint;
    snd_pcm_prepare: fn(*mut Pcm) -> c_int;
    snd_pcm_start: fn(*mut Pcm) -> c_int;
    snd_pcm_drop: fn(*mut Pcm) -> c_int;
    snd_pcm_writei: fn(*mut Pcm, *const c_void, c_ulong) -> c_long;
    snd_pcm_recover: fn(*mut Pcm, c_int, c_int) -> c_int;
    snd_pcm_wait: fn(*mut Pcm, c_int) -> c_int;
    snd_pcm_avail_update: fn(*mut Pcm) -> c_long;
    snd_pcm_delay: fn(*mut Pcm, *mut c_long) -> c_int;
}

fn alsa_error(call: &'static str, rc: impl Into<i64>) -> DeviceError {
    DeviceError::Backend {
        call,
        code: rc.into(),
    }
}

/// An open PCM, closed on drop. Used only on the render thread.
struct PcmHandle {
    api: Arc<Api>,
    pcm: *mut Pcm,
}

impl Drop for PcmHandle {
    fn drop(&mut self) {
        // SAFETY: the PCM was opened by snd_pcm_open and is closed once.
        unsafe {
            (self.api.snd_pcm_drop)(self.pcm);
            (self.api.snd_pcm_close)(self.pcm);
        }
    }
}

/// Owned hw/sw parameter containers.
struct Params {
    api: Arc<Api>,
    hw: *mut HwParams,
    sw: *mut SwParams,
}

impl Drop for Params {
    fn drop(&mut self) {
        // SAFETY: allocated by the *_malloc calls (null if they failed).
        unsafe {
            if !self.hw.is_null() {
                (self.api.snd_pcm_hw_params_free)(self.hw);
            }
            if !self.sw.is_null() {
                (self.api.snd_pcm_sw_params_free)(self.sw);
            }
        }
    }
}

pub(super) fn open_default() -> Result<OpenedDevice, DeviceError> {
    let api = Arc::new(Api::load().map_err(|e| DeviceError::NoBackend(format!("libasound: {e}")))?);
    spawn_backend("vks-audio-alsa", move |ctx| render_thread(api, ctx))
}

struct Opened {
    pcm: PcmHandle,
    format: DeviceFormat,
    order: DeviceOrder,
    period_frames: u32,
}

/// `ma_prev_power_of_2`.
fn prev_power_of_two(x: u32) -> u32 {
    if x == 0 {
        0
    } else {
        1 << (31 - x.leading_zeros())
    }
}

fn open_pcm(api: &Arc<Api>) -> Result<Opened, DeviceError> {
    let mut pcm: *mut Pcm = std::ptr::null_mut();
    let mut opened_name = None;
    for name in DEFAULT_PCMS {
        // SAFETY: `name` is NUL-terminated; `pcm` receives the handle.
        if unsafe { (api.snd_pcm_open)(&raw mut pcm, name.as_ptr(), SND_PCM_STREAM_PLAYBACK, 0) }
            == 0
        {
            opened_name = Some(name);
            break;
        }
    }
    if opened_name.is_none() || pcm.is_null() {
        return Err(DeviceError::NoBackend(
            "snd_pcm_open: no default PCM".into(),
        ));
    }
    let handle = PcmHandle {
        api: api.clone(),
        pcm,
    };
    let mut params = Params {
        api: api.clone(),
        hw: std::ptr::null_mut(),
        sw: std::ptr::null_mut(),
    };

    // SAFETY: every call below operates on the open PCM and the parameter
    // containers allocated here (freed by Params::drop); out-params are
    // locals.
    unsafe {
        let rc = (api.snd_pcm_hw_params_malloc)(&raw mut params.hw);
        if rc < 0 || params.hw.is_null() {
            return Err(alsa_error("snd_pcm_hw_params_malloc", rc));
        }
        let hw = params.hw;
        let rc = (api.snd_pcm_hw_params_any)(pcm, hw);
        if rc < 0 {
            return Err(alsa_error("snd_pcm_hw_params_any", rc));
        }
        let rc = (api.snd_pcm_hw_params_set_access)(pcm, hw, SND_PCM_ACCESS_RW_INTERLEAVED);
        if rc < 0 {
            return Err(alsa_error("snd_pcm_hw_params_set_access", rc));
        }
        // Format: the requested f32, else the priority list.
        let chosen = if (api.snd_pcm_hw_params_test_format)(pcm, hw, SND_PCM_FORMAT_FLOAT_LE) == 0 {
            Some((SND_PCM_FORMAT_FLOAT_LE, SampleFormat::F32))
        } else {
            FORMAT_PRIORITIES
                .iter()
                .copied()
                .find(|(f, _)| (api.snd_pcm_hw_params_test_format)(pcm, hw, *f) == 0)
        };
        let Some((alsa_format, sample)) = chosen else {
            return Err(DeviceError::UnsupportedFormat(
                "no ALSA sample format miniaudio supports".into(),
            ));
        };
        let rc = (api.snd_pcm_hw_params_set_format)(pcm, hw, alsa_format);
        if rc < 0 {
            return Err(alsa_error("snd_pcm_hw_params_set_format", rc));
        }
        let mut channels: c_uint = 2;
        let rc = (api.snd_pcm_hw_params_set_channels_near)(pcm, hw, &raw mut channels);
        if rc < 0 {
            return Err(alsa_error("snd_pcm_hw_params_set_channels_near", rc));
        }
        (api.snd_pcm_hw_params_set_rate_resample)(pcm, hw, 0);
        let mut rate: c_uint = 48000;
        let rc =
            (api.snd_pcm_hw_params_set_rate_near)(pcm, hw, &raw mut rate, std::ptr::null_mut());
        if rc < 0 {
            return Err(alsa_error("snd_pcm_hw_params_set_rate_near", rc));
        }
        let mut periods: c_uint = PERIODS;
        let rc = (api.snd_pcm_hw_params_set_periods_near)(
            pcm,
            hw,
            &raw mut periods,
            std::ptr::null_mut(),
        );
        if rc < 0 {
            return Err(alsa_error("snd_pcm_hw_params_set_periods_near", rc));
        }
        let periods = periods.max(1);
        let mut buffer: c_ulong =
            c_ulong::from(frames_from_ms(PERIOD_MILLISECONDS, rate) * periods);
        let rc = (api.snd_pcm_hw_params_set_buffer_size_near)(pcm, hw, &raw mut buffer);
        if rc < 0 {
            return Err(alsa_error("snd_pcm_hw_params_set_buffer_size_near", rc));
        }
        let period = (buffer / c_ulong::from(periods)) as u32;
        let rc = (api.snd_pcm_hw_params)(pcm, hw);
        if rc < 0 {
            return Err(alsa_error("snd_pcm_hw_params", rc));
        }

        let rc = (api.snd_pcm_sw_params_malloc)(&raw mut params.sw);
        if rc < 0 || params.sw.is_null() {
            return Err(alsa_error("snd_pcm_sw_params_malloc", rc));
        }
        let sw = params.sw;
        let rc = (api.snd_pcm_sw_params_current)(pcm, sw);
        if rc < 0 {
            return Err(alsa_error("snd_pcm_sw_params_current", rc));
        }
        let rc = (api.snd_pcm_sw_params_set_avail_min)(
            pcm,
            sw,
            c_ulong::from(prev_power_of_two(period)),
        );
        if rc < 0 {
            return Err(alsa_error("snd_pcm_sw_params_set_avail_min", rc));
        }
        let mut boundary: c_ulong = 0;
        if (api.snd_pcm_sw_params_get_boundary)(sw, &raw mut boundary) < 0 {
            boundary = c_ulong::from(period) * c_ulong::from(periods);
        }
        let rc = (api.snd_pcm_sw_params_set_start_threshold)(pcm, sw, c_ulong::from(period) * 2);
        if rc < 0 {
            return Err(alsa_error("snd_pcm_sw_params_set_start_threshold", rc));
        }
        let rc = (api.snd_pcm_sw_params_set_stop_threshold)(pcm, sw, boundary);
        if rc < 0 {
            return Err(alsa_error("snd_pcm_sw_params_set_stop_threshold", rc));
        }
        let rc = (api.snd_pcm_sw_params)(pcm, sw);
        if rc < 0 {
            return Err(alsa_error("snd_pcm_sw_params", rc));
        }

        // Channel map, as miniaudio read it.
        let positions = read_chmap(api, pcm, channels);
        let rc = (api.snd_pcm_prepare)(pcm);
        if rc < 0 {
            return Err(alsa_error("snd_pcm_prepare", rc));
        }

        if !is_supported_channel_count(channels) {
            return Err(DeviceError::UnsupportedChannels(channels));
        }
        let order = DeviceOrder::from_positions(&positions);
        let name_ptr = (api.snd_pcm_name)(pcm);
        let device_name = if name_ptr.is_null() {
            opened_name
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            CStr::from_ptr(name_ptr).to_string_lossy().into_owned()
        };
        let format = DeviceFormat {
            rate,
            channels: channels as u16,
            channel_mask: order.mask,
            headphones: false,
            backend: BACKEND,
            period_frames: period,
            buffer_frames: buffer as u32,
            sample_format: sample,
            device_name,
        };
        drop(params);
        Ok(Opened {
            pcm: handle,
            format,
            order,
            period_frames: period,
        })
    }
}

/// The PCM's channel positions as mask bits (miniaudio's rules: excess
/// map entries dropped; a short map is completed from the standard map and
/// replaced by it if that creates duplicates; no map = standard map).
///
/// # Safety
/// `pcm` must be an open PCM.
unsafe fn read_chmap(api: &Api, pcm: *mut Pcm, channels: c_uint) -> Vec<u32> {
    let standard = alsa_standard_positions(channels);
    // SAFETY: snd_pcm_get_chmap returns null or a malloc'd
    // `{ unsigned channels; unsigned pos[channels]; }` that we free once.
    unsafe {
        let map = (api.snd_pcm_get_chmap)(pcm);
        if map.is_null() {
            return standard;
        }
        let count = *map;
        let pos = std::slice::from_raw_parts(map.add(1), count as usize);
        let mut positions = standard.clone();
        for (c, slot) in positions.iter_mut().enumerate() {
            if let Some(&p) = pos.get(c) {
                *slot = alsa_position_bit(p);
            }
        }
        (api.free)(map.cast());
        if count < channels {
            let mut seen = 0u32;
            for &bit in &positions {
                if bit != 0 && seen & bit != 0 {
                    return standard;
                }
                seen |= bit;
            }
        }
        positions
    }
}

fn render_thread(api: Arc<Api>, ctx: ThreadContext) -> Option<Box<dyn RenderCallback>> {
    let ThreadContext {
        opened,
        start,
        shared,
    } = ctx;
    let o = match open_pcm(&api) {
        Ok(o) => o,
        Err(e) => {
            let _ = opened.send(Err(e));
            return None;
        }
    };
    let _ = opened.send(Ok(o.format.clone()));
    match start.recv() {
        Ok(cmd) => run(&o, cmd, &shared),
        Err(_) => None,
    }
}

enum WriteError {
    Stopped,
    Lost,
    Failed,
}

/// Writes `data` (whole frames) completely, waiting in 100 ms slices for
/// space; recovers underruns like `ma_device_write__alsa`.
fn write_all(
    o: &Opened,
    data: &[u8],
    frame_bytes: usize,
    shared: &Shared,
) -> Result<(), WriteError> {
    let api = &o.pcm.api;
    let pcm = o.pcm.pcm;
    let mut offset = 0usize;
    let total = data.len() / frame_bytes;
    while offset < total {
        if shared.stop_requested() {
            return Err(WriteError::Stopped);
        }
        // SAFETY: open PCM, render thread only.
        let avail = unsafe { (api.snd_pcm_avail_update)(pcm) };
        if avail < 0 {
            recover(o, avail)?;
            continue;
        }
        if avail == 0 {
            // SAFETY: as above.
            let rc = unsafe { (api.snd_pcm_wait)(pcm, 100) };
            if rc < 0 {
                recover(o, c_long::from(rc))?;
            }
            continue;
        }
        let n = (total - offset).min(avail as usize);
        // SAFETY: `data[offset * frame_bytes..]` holds at least n frames.
        let rc = unsafe {
            (api.snd_pcm_writei)(
                pcm,
                data.as_ptr().add(offset * frame_bytes).cast(),
                n as c_ulong,
            )
        };
        if rc >= 0 {
            offset += rc as usize;
        } else if rc != -EAGAIN {
            recover(o, rc)?;
        }
    }
    Ok(())
}

fn recover(o: &Opened, rc: c_long) -> Result<(), WriteError> {
    let api = &o.pcm.api;
    if rc == -ENODEV {
        return Err(WriteError::Lost);
    }
    if rc == -EPIPE || rc == -ESTRPIPE {
        // SAFETY: open PCM, render thread only.
        unsafe {
            if (api.snd_pcm_recover)(o.pcm.pcm, rc as c_int, 1) < 0 {
                return Err(WriteError::Failed);
            }
            if rc == -EPIPE && (api.snd_pcm_start)(o.pcm.pcm) < 0 {
                return Err(WriteError::Failed);
            }
        }
        return Ok(());
    }
    Err(WriteError::Failed)
}

fn run(o: &Opened, cmd: StartCommand, shared: &Arc<Shared>) -> Option<Box<dyn RenderCallback>> {
    let channels = usize::from(o.format.channels);
    let frame_bytes = channels * o.format.sample_format.bytes_per_sample();
    let mut renderer = Renderer::new(
        cmd.callback,
        channels,
        o.format.sample_format,
        o.order.clone(),
    );
    let chunk_frames = (CONVERTER_BUFFER_BYTES / frame_bytes).max(1);
    let mut floats = vec![0f32; chunk_frames * channels];
    let mut bytes = vec![0u8; chunk_frames * frame_bytes];
    let _ = cmd.reply.send(Ok(()));
    shared.set_state(DeviceState::Running);

    let mut written = 0u64;
    let mut end_state = None;
    'outer: while !shared.stop_requested() {
        let mut done = 0usize;
        while done < o.period_frames as usize {
            let n = (o.period_frames as usize - done).min(chunk_frames);
            let out = &mut bytes[..n * frame_bytes];
            if o.format.sample_format == SampleFormat::F32 {
                let f = &mut floats[..n * channels];
                renderer.render_f32(f);
                for (s, d) in f.iter().zip(out.chunks_exact_mut(4)) {
                    d.copy_from_slice(&s.to_le_bytes());
                }
            } else {
                renderer.render_bytes(out);
            }
            match write_all(o, out, frame_bytes, shared) {
                Ok(()) => {}
                Err(WriteError::Stopped) => break 'outer,
                Err(WriteError::Lost) => {
                    end_state = Some(DeviceState::Lost);
                    break 'outer;
                }
                Err(WriteError::Failed) => {
                    end_state = Some(DeviceState::Failed);
                    break 'outer;
                }
            }
            written += n as u64;
            done += n;
            let mut delay: c_long = 0;
            // SAFETY: open PCM; out-param is a local.
            if unsafe { (o.pcm.api.snd_pcm_delay)(o.pcm.pcm, &raw mut delay) } == 0 && delay >= 0 {
                shared
                    .clock
                    .advance_to(written.saturating_sub(delay as u64));
            }
            if renderer.failed() {
                end_state = Some(DeviceState::Failed);
                break 'outer;
            }
        }
    }
    if let Some(state) = end_state {
        shared.set_state(state);
        shared.wait_for_stop();
    }
    Some(renderer.into_callback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_of_two() {
        assert_eq!(prev_power_of_two(1920), 1024);
        assert_eq!(prev_power_of_two(1764), 1024);
        assert_eq!(prev_power_of_two(1024), 1024);
        assert_eq!(prev_power_of_two(1), 1);
        assert_eq!(prev_power_of_two(0), 0);
    }
}
