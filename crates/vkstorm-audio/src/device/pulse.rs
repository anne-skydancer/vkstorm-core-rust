//! PulseAudio output (Linux), libpulse loaded at run time.
//!
//! Reproduces miniaudio 0.10.42's PulseAudio backend for the default sink:
//!
//! - `libpulse.so.0` (or the `libpulse.so` development name) opened with
//!   dlopen; no link-time dependency. If it is missing, or no server
//!   answers, ALSA is tried next (`super::open`).
//! - A plain `pa_mainloop` driven by the render thread (miniaudio dropped
//!   `pa_threaded_mainloop`); context connected without autospawn.
//! - The default sink's sample spec and channel map are used as they are
//!   (`channels = 0, sampleRate = 0` meant native): unknown sample formats
//!   become float32, rate 0 becomes 48000, channels 0 become 2. The stream
//!   is created in exactly that spec with `FIX_FORMAT | FIX_RATE |
//!   FIX_CHANNELS | ADJUST_LATENCY | START_CORKED`, buffer attributes
//!   `maxlength = 3 periods, tlength = fragsize = 1 period (40 ms),
//!   prebuf = minreq = -1`. `AUTO_TIMING_UPDATE | INTERPOLATE_TIMING` are
//!   added for the device clock; they do not change the audio.
//! - Rendering happens when PulseAudio requests data (write callback): the
//!   writable size is filled with `pa_stream_begin_write`/`pa_stream_write`
//!   until the request is met; the first fill precedes uncorking.
//! - Channels are reordered from the mixer's mask order to the sink's
//!   channel map (see `layout.rs`: miniaudio passed SoLoud's slots through
//!   unmapped, which misrouted centre/LFE/rears on ALSA-ordered surround
//!   sinks; that is corrected).
//! - The stream follows the default sink: it is not created with
//!   `DONT_MOVE` (miniaudio added that only for named sinks), so the
//!   server moves it.
//! - Device clock: frames written minus `pa_stream_get_latency` (write
//!   index - read index + sink latency), refreshed on every loop pass.
//! - Thread priority is left as created: miniaudio asked for "highest"
//!   through `sched_get_priority_max/min` of the current policy, which is
//!   SCHED_OTHER (range 0..0), so it never changed anything on Linux.
//! - Stop corks without draining (miniaudio drained up to one buffer, 120
//!   ms, before corking; nothing audible is lost when the device closes).

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use libloading::Library;

use super::convert::SampleFormat;
use super::layout::{DeviceOrder, is_supported_channel_count, pulse_position_bit};
use super::render::Renderer;
use super::{
    DeviceError, DeviceFormat, DeviceState, OpenedDevice, PERIOD_MILLISECONDS, PERIODS,
    RenderCallback, Shared, StartCommand, ThreadContext, frames_from_ms, spawn_backend,
};

const BACKEND: &str = "PulseAudio";
/// How long connecting and each setup round trip may take.
const SETUP_TIMEOUT: Duration = Duration::from_secs(5);

// pulse/def.h, pulse/sample.h
const PA_SAMPLE_U8: c_int = 0;
const PA_SAMPLE_S16LE: c_int = 3;
const PA_SAMPLE_FLOAT32LE: c_int = 5;
const PA_SAMPLE_S32LE: c_int = 7;
const PA_SAMPLE_S24LE: c_int = 9;
const PA_CONTEXT_READY: c_int = 4;
const PA_CONTEXT_FAILED: c_int = 5;
const PA_CONTEXT_TERMINATED: c_int = 6;
const PA_STREAM_READY: c_int = 2;
const PA_STREAM_FAILED: c_int = 3;
const PA_STREAM_TERMINATED: c_int = 4;
const PA_OPERATION_RUNNING: c_int = 0;
const PA_CONTEXT_NOAUTOSPAWN: c_int = 0x1;
const PA_STREAM_START_CORKED: c_int = 0x1;
const PA_STREAM_INTERPOLATE_TIMING: c_int = 0x2;
const PA_STREAM_AUTO_TIMING_UPDATE: c_int = 0x8;
const PA_STREAM_FIX_FORMAT: c_int = 0x40;
const PA_STREAM_FIX_RATE: c_int = 0x80;
const PA_STREAM_FIX_CHANNELS: c_int = 0x100;
const PA_STREAM_ADJUST_LATENCY: c_int = 0x2000;
const PA_SEEK_RELATIVE: c_int = 0;
const PA_CHANNELS_MAX: usize = 32;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct PaSampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct PaChannelMap {
    channels: u8,
    map: [c_int; PA_CHANNELS_MAX],
}

impl Default for PaChannelMap {
    fn default() -> Self {
        PaChannelMap {
            channels: 0,
            map: [0; PA_CHANNELS_MAX],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct PaBufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

#[repr(C)]
struct PaCvolume {
    channels: u8,
    values: [u32; PA_CHANNELS_MAX],
}

#[repr(C)]
struct PaSinkPortInfo {
    name: *const c_char,
    description: *const c_char,
    priority: u32,
    available: c_int,
}

/// `pa_sink_info` up to the fields every libpulse since 1.0 has (the
/// struct only grew at its end since).
#[repr(C)]
struct PaSinkInfo {
    name: *const c_char,
    index: u32,
    description: *const c_char,
    sample_spec: PaSampleSpec,
    channel_map: PaChannelMap,
    owner_module: u32,
    volume: PaCvolume,
    mute: c_int,
    monitor_source: u32,
    monitor_source_name: *const c_char,
    latency: u64,
    driver: *const c_char,
    flags: c_int,
    proplist: *mut c_void,
    configured_latency: u64,
    base_volume: u32,
    state: c_int,
    n_volume_steps: u32,
    card: u32,
    n_ports: u32,
    ports: *mut *mut PaSinkPortInfo,
    active_port: *mut PaSinkPortInfo,
    n_formats: u8,
    formats: *mut *mut c_void,
}

type Opaque = c_void;
type SinkInfoCb = unsafe extern "C" fn(*mut Opaque, *const PaSinkInfo, c_int, *mut c_void);
type StreamRequestCb = unsafe extern "C" fn(*mut Opaque, usize, *mut c_void);
type StreamSuccessCb = unsafe extern "C" fn(*mut Opaque, c_int, *mut c_void);

macro_rules! pulse_api {
    ($($name:ident : fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        /// The libpulse entry points used, resolved once per device.
        struct Api {
            $($name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
            _lib: Library,
        }

        impl Api {
            fn load() -> Result<Api, String> {
                let mut last = String::new();
                for name in ["libpulse.so.0", "libpulse.so"] {
                    // SAFETY: loading libpulse runs its (side-effect free)
                    // initializers; it is a system library meant for dlopen.
                    match unsafe { Library::new(name) } {
                        Ok(lib) => {
                            // SAFETY: each symbol is resolved with the exact
                            // C signature from the pulse/*.h headers; the
                            // pointers are kept with the Library that owns
                            // them (`_lib`) and never outlive it.
                            unsafe {
                                $(let $name = *lib
                                    .get::<unsafe extern "C" fn($($arg),*) $(-> $ret)?>(stringify!($name))
                                    .map_err(|e| format!("{}: {e}", stringify!($name)))?;)*
                                return Ok(Api { $($name,)* _lib: lib });
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

pulse_api! {
    pa_mainloop_new: fn() -> *mut Opaque;
    pa_mainloop_free: fn(*mut Opaque);
    pa_mainloop_get_api: fn(*mut Opaque) -> *mut Opaque;
    pa_mainloop_prepare: fn(*mut Opaque, c_int) -> c_int;
    pa_mainloop_poll: fn(*mut Opaque) -> c_int;
    pa_mainloop_dispatch: fn(*mut Opaque) -> c_int;
    pa_mainloop_wakeup: fn(*mut Opaque);
    pa_context_new: fn(*mut Opaque, *const c_char) -> *mut Opaque;
    pa_context_connect: fn(*mut Opaque, *const c_char, c_int, *const c_void) -> c_int;
    pa_context_get_state: fn(*mut Opaque) -> c_int;
    pa_context_disconnect: fn(*mut Opaque);
    pa_context_unref: fn(*mut Opaque);
    pa_context_get_sink_info_by_name: fn(*mut Opaque, *const c_char, Option<SinkInfoCb>, *mut c_void) -> *mut Opaque;
    pa_operation_get_state: fn(*mut Opaque) -> c_int;
    pa_operation_unref: fn(*mut Opaque);
    pa_stream_new: fn(*mut Opaque, *const c_char, *const PaSampleSpec, *const PaChannelMap) -> *mut Opaque;
    pa_stream_connect_playback: fn(*mut Opaque, *const c_char, *const PaBufferAttr, c_int, *const c_void, *mut Opaque) -> c_int;
    pa_stream_get_state: fn(*mut Opaque) -> c_int;
    pa_stream_set_write_callback: fn(*mut Opaque, Option<StreamRequestCb>, *mut c_void);
    pa_stream_writable_size: fn(*mut Opaque) -> usize;
    pa_stream_begin_write: fn(*mut Opaque, *mut *mut c_void, *mut usize) -> c_int;
    pa_stream_write: fn(*mut Opaque, *const c_void, usize, *const c_void, i64, c_int) -> c_int;
    pa_stream_cork: fn(*mut Opaque, c_int, Option<StreamSuccessCb>, *mut c_void) -> *mut Opaque;
    pa_stream_get_sample_spec: fn(*mut Opaque) -> *const PaSampleSpec;
    pa_stream_get_channel_map: fn(*mut Opaque) -> *const PaChannelMap;
    pa_stream_get_buffer_attr: fn(*mut Opaque) -> *const PaBufferAttr;
    pa_stream_get_latency: fn(*mut Opaque, *mut u64, *mut c_int) -> c_int;
    pa_stream_disconnect: fn(*mut Opaque) -> c_int;
    pa_stream_unref: fn(*mut Opaque);
    pa_proplist_gets: fn(*mut c_void, *const c_char) -> *const c_char;
}

fn sample_format(pa: c_int) -> Option<SampleFormat> {
    // ma_format_from_pulse (little-endian host).
    match pa {
        PA_SAMPLE_U8 => Some(SampleFormat::U8),
        PA_SAMPLE_S16LE => Some(SampleFormat::S16),
        PA_SAMPLE_S24LE => Some(SampleFormat::S24),
        PA_SAMPLE_S32LE => Some(SampleFormat::S32),
        PA_SAMPLE_FLOAT32LE => Some(SampleFormat::F32),
        _ => None,
    }
}

/// What the sink-info callback copies out.
#[derive(Default)]
struct SinkQuery {
    done: bool,
    found: bool,
    spec: PaSampleSpec,
    map: PaChannelMap,
    description: String,
    form_factor: Option<String>,
    active_port: Option<String>,
    /// For `pa_proplist_gets` inside the callback.
    proplist_gets: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *const c_char>,
}

/// # Safety
/// `p` is null or a NUL-terminated string valid for the call.
unsafe fn c_string(p: *const c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        // SAFETY: per the contract.
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

unsafe extern "C" fn sink_info_cb(
    _ctx: *mut Opaque,
    info: *const PaSinkInfo,
    eol: c_int,
    userdata: *mut c_void,
) {
    // SAFETY: userdata is the &mut SinkQuery passed with the request, alive
    // until the operation completes (the caller waits for it); `info` is
    // valid for the call when non-null. Written without panicking paths.
    unsafe {
        let query = &mut *(userdata.cast::<SinkQuery>());
        if eol != 0 || info.is_null() {
            query.done = true;
            return;
        }
        if query.found {
            return;
        }
        let info = &*info;
        query.found = true;
        query.spec = info.sample_spec;
        query.map = info.channel_map;
        query.description = c_string(info.description)
            .or_else(|| c_string(info.name))
            .unwrap_or_default();
        if let Some(gets) = query.proplist_gets
            && !info.proplist.is_null()
        {
            query.form_factor = c_string(gets(info.proplist, c"device.form_factor".as_ptr()));
        }
        if !info.active_port.is_null() {
            query.active_port = c_string((*info.active_port).name);
        }
    }
}

/// Counts bytes PulseAudio asked for (only touched on the render thread,
/// inside dispatch; atomic so it can be shared by reference).
struct Requests {
    bytes: AtomicUsize,
}

unsafe extern "C" fn write_request_cb(_s: *mut Opaque, nbytes: usize, userdata: *mut c_void) {
    // SAFETY: userdata is the Requests the render thread registered and
    // keeps alive until the callback is removed with the stream.
    let requests = unsafe { &*(userdata.cast::<Requests>()) };
    requests.bytes.fetch_add(nbytes, Ordering::AcqRel);
}

unsafe extern "C" fn cork_done_cb(_s: *mut Opaque, success: c_int, userdata: *mut c_void) {
    // SAFETY: userdata is an i32 slot alive until the operation completes.
    unsafe { *(userdata.cast::<c_int>()) = if success != 0 { 1 } else { 0 } };
}

/// The connection: mainloop, context and (once created) stream, torn
/// down in reverse order. Raw pointers are used only on the render thread.
struct Connection {
    api: Arc<Api>,
    mainloop: *mut Opaque,
    context: *mut Opaque,
    stream: *mut Opaque,
}

/// The mainloop pointer for `pa_mainloop_wakeup` from another thread.
struct WakeTarget {
    api: Arc<Api>,
    mainloop: *mut Opaque,
}

// SAFETY: pa_mainloop_wakeup is documented as callable from other threads
// to interrupt a poll; the pointer is only used while the render thread
// keeps the mainloop alive (the waker is removed under the Shared mutex
// before pa_mainloop_free).
unsafe impl Send for WakeTarget {}

impl WakeTarget {
    fn wake(&self) {
        // SAFETY: see the Send impl: the mainloop is alive while the waker
        // is installed, and pa_mainloop_wakeup is thread-safe.
        unsafe { (self.api.pa_mainloop_wakeup)(self.mainloop) }
    }
}

impl Connection {
    fn new(api: Arc<Api>) -> Result<Connection, DeviceError> {
        // SAFETY: plain constructor calls; results are checked for null and
        // released in Drop.
        unsafe {
            let mainloop = (api.pa_mainloop_new)();
            if mainloop.is_null() {
                return Err(DeviceError::NoBackend("pa_mainloop_new failed".into()));
            }
            let mut conn = Connection {
                api: api.clone(),
                mainloop,
                context: std::ptr::null_mut(),
                stream: std::ptr::null_mut(),
            };
            let context =
                (api.pa_context_new)((api.pa_mainloop_get_api)(mainloop), c"Vulkanstorm".as_ptr());
            if context.is_null() {
                return Err(DeviceError::NoBackend("pa_context_new failed".into()));
            }
            conn.context = context;
            if (api.pa_context_connect)(
                context,
                std::ptr::null(),
                PA_CONTEXT_NOAUTOSPAWN,
                std::ptr::null(),
            ) < 0
            {
                return Err(DeviceError::NoBackend("no PulseAudio server".into()));
            }
            Ok(conn)
        }
    }

    /// One mainloop pass, waiting at most `timeout_us` (-1: until an event
    /// or a wakeup).
    fn iterate(&self, timeout_us: c_int) -> Result<(), DeviceError> {
        // SAFETY: the mainloop is alive and used only on this thread.
        unsafe {
            if (self.api.pa_mainloop_prepare)(self.mainloop, timeout_us) < 0
                || (self.api.pa_mainloop_poll)(self.mainloop) < 0
                || (self.api.pa_mainloop_dispatch)(self.mainloop) < 0
            {
                return Err(DeviceError::Backend {
                    call: "pa_mainloop_iterate",
                    code: -1,
                });
            }
        }
        Ok(())
    }

    fn wait_until(
        &self,
        mut done: impl FnMut() -> Result<bool, DeviceError>,
    ) -> Result<(), DeviceError> {
        let deadline = Instant::now() + SETUP_TIMEOUT;
        loop {
            if done()? {
                return Ok(());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(DeviceError::NoBackend("PulseAudio did not answer".into()));
            }
            self.iterate(left.as_micros().min(i32::MAX as u128) as c_int)?;
        }
    }

    fn wait_ready(&self) -> Result<(), DeviceError> {
        self.wait_until(|| {
            // SAFETY: live context, render thread only.
            match unsafe { (self.api.pa_context_get_state)(self.context) } {
                PA_CONTEXT_READY => Ok(true),
                PA_CONTEXT_FAILED | PA_CONTEXT_TERMINATED => Err(DeviceError::NoBackend(
                    "PulseAudio connection failed".into(),
                )),
                _ => Ok(false),
            }
        })
    }

    /// Waits for an operation and releases it.
    fn wait_operation(&self, op: *mut Opaque, call: &'static str) -> Result<(), DeviceError> {
        if op.is_null() {
            return Err(DeviceError::Backend { call, code: -1 });
        }
        let result = self.wait_until(|| {
            // SAFETY: `op` is a live operation reference owned here.
            Ok(unsafe { (self.api.pa_operation_get_state)(op) } != PA_OPERATION_RUNNING)
        });
        // SAFETY: releases the reference returned by the request.
        unsafe { (self.api.pa_operation_unref)(op) };
        result
    }

    fn default_sink(&self) -> Result<SinkQuery, DeviceError> {
        let mut query = SinkQuery {
            proplist_gets: Some(self.api.pa_proplist_gets),
            ..Default::default()
        };
        // SAFETY: a null name asks for the default sink (as miniaudio did);
        // `query` outlives the operation, which wait_operation completes.
        let op = unsafe {
            (self.api.pa_context_get_sink_info_by_name)(
                self.context,
                std::ptr::null(),
                Some(sink_info_cb),
                (&raw mut query).cast(),
            )
        };
        self.wait_operation(op, "pa_context_get_sink_info_by_name")?;
        if !query.found {
            return Err(DeviceError::NotFound);
        }
        Ok(query)
    }

    fn stream_state(&self) -> c_int {
        // SAFETY: live stream, render thread only.
        unsafe { (self.api.pa_stream_get_state)(self.stream) }
    }

    fn cork(&self, cork: bool) -> Result<(), DeviceError> {
        let mut success: c_int = -1;
        // SAFETY: `success` outlives the operation (waited below).
        let op = unsafe {
            (self.api.pa_stream_cork)(
                self.stream,
                c_int::from(cork),
                Some(cork_done_cb),
                (&raw mut success).cast(),
            )
        };
        self.wait_operation(op, "pa_stream_cork")?;
        if success == 1 {
            Ok(())
        } else {
            Err(DeviceError::Backend {
                call: "pa_stream_cork",
                code: i64::from(success),
            })
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: each object is released once, stream before context
        // before mainloop; the write callback is removed first so it cannot
        // reach freed userdata.
        unsafe {
            if !self.stream.is_null() {
                (self.api.pa_stream_set_write_callback)(self.stream, None, std::ptr::null_mut());
                (self.api.pa_stream_disconnect)(self.stream);
                (self.api.pa_stream_unref)(self.stream);
            }
            if !self.context.is_null() {
                (self.api.pa_context_disconnect)(self.context);
                (self.api.pa_context_unref)(self.context);
            }
            (self.api.pa_mainloop_free)(self.mainloop);
        }
    }
}

pub(super) fn open_default() -> Result<OpenedDevice, DeviceError> {
    let api = Arc::new(Api::load().map_err(|e| DeviceError::NoBackend(format!("libpulse: {e}")))?);
    spawn_backend("vks-audio-pulse", move |ctx| render_thread(api, ctx))
}

struct Opened {
    conn: Connection,
    requests: Box<Requests>,
    format: DeviceFormat,
    order: DeviceOrder,
    bytes_per_frame: usize,
}

fn open_stream(api: Arc<Api>) -> Result<Opened, DeviceError> {
    // Declared before `conn` so that on early returns `conn` drops first.
    let requests = Box::new(Requests {
        bytes: AtomicUsize::new(0),
    });
    let mut conn = Connection::new(api.clone())?;
    conn.wait_ready()?;
    let sink = conn.default_sink()?;

    let mut spec = sink.spec;
    if sample_format(spec.format).is_none() {
        spec.format = PA_SAMPLE_FLOAT32LE;
    }
    if spec.rate == 0 {
        spec.rate = 48000;
    }
    if spec.channels == 0 {
        spec.channels = 2;
    }
    let mut map = sink.map;
    if usize::from(map.channels) != usize::from(spec.channels) {
        // Let PulseAudio pick its default map for the count.
        map = PaChannelMap::default();
    }
    if !is_supported_channel_count(u32::from(spec.channels)) {
        return Err(DeviceError::UnsupportedChannels(u32::from(spec.channels)));
    }
    let format = sample_format(spec.format).unwrap_or(SampleFormat::F32);
    let bpf = usize::from(spec.channels) * format.bytes_per_sample();
    let period = frames_from_ms(PERIOD_MILLISECONDS, spec.rate);
    let maxlength = period * PERIODS * bpf as u32;
    let attr = PaBufferAttr {
        maxlength,
        tlength: maxlength / PERIODS,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: maxlength / PERIODS,
    };

    // SAFETY: `spec`/`map`/`attr` outlive the calls; the stream is
    // released by Connection::drop, which first removes the write
    // callback; `requests` is boxed (stable address) and outlives that:
    // Opened declares `conn` before `requests`, so `conn` drops first, and
    // here `requests` was declared before `conn`. (Callbacks only run
    // inside pa_mainloop_dispatch, never during the teardown itself.)
    unsafe {
        let map_ptr = if map.channels == 0 {
            std::ptr::null()
        } else {
            &raw const map
        };
        conn.stream = (api.pa_stream_new)(
            conn.context,
            c"Vulkanstorm".as_ptr(),
            &raw const spec,
            map_ptr,
        );
        if conn.stream.is_null() {
            return Err(DeviceError::Backend {
                call: "pa_stream_new",
                code: -1,
            });
        }
        (api.pa_stream_set_write_callback)(
            conn.stream,
            Some(write_request_cb),
            (&raw const *requests).cast_mut().cast(),
        );
        let flags = PA_STREAM_START_CORKED
            | PA_STREAM_ADJUST_LATENCY
            | PA_STREAM_FIX_FORMAT
            | PA_STREAM_FIX_RATE
            | PA_STREAM_FIX_CHANNELS
            | PA_STREAM_AUTO_TIMING_UPDATE
            | PA_STREAM_INTERPOLATE_TIMING;
        let rc = (api.pa_stream_connect_playback)(
            conn.stream,
            std::ptr::null(),
            &raw const attr,
            flags,
            std::ptr::null(),
            std::ptr::null_mut(),
        );
        if rc < 0 {
            return Err(DeviceError::Backend {
                call: "pa_stream_connect_playback",
                code: i64::from(rc),
            });
        }
    }
    conn.wait_until(|| match conn.stream_state() {
        PA_STREAM_READY => Ok(true),
        PA_STREAM_FAILED | PA_STREAM_TERMINATED => Err(DeviceError::Backend {
            call: "pa_stream_connect_playback",
            code: -1,
        }),
        _ => Ok(false),
    })?;

    // The actual stream format (FIX_* flags: the sink's).
    // SAFETY: getters on a ready stream return pointers valid while it lives;
    // copied immediately.
    let (actual_spec, actual_map, actual_attr) = unsafe {
        let s = (api.pa_stream_get_sample_spec)(conn.stream);
        let m = (api.pa_stream_get_channel_map)(conn.stream);
        let a = (api.pa_stream_get_buffer_attr)(conn.stream);
        (
            if s.is_null() { spec } else { *s },
            if m.is_null() { map } else { *m },
            if a.is_null() { attr } else { *a },
        )
    };
    let channels = u32::from(actual_spec.channels);
    if !is_supported_channel_count(channels) {
        return Err(DeviceError::UnsupportedChannels(channels));
    }
    let sample = sample_format(actual_spec.format).ok_or_else(|| {
        DeviceError::UnsupportedFormat(format!("PulseAudio sample format {}", actual_spec.format))
    })?;
    let bpf = channels as usize * sample.bytes_per_sample();
    let positions: Vec<u32> = (0..channels as usize)
        .map(|c| {
            if c < usize::from(actual_map.channels) {
                pulse_position_bit(actual_map.map[c])
            } else {
                0
            }
        })
        .collect();
    let order = DeviceOrder::from_positions(&positions);
    let periods = (actual_attr.maxlength / actual_attr.tlength.max(1)).max(1);
    let buffer_frames = actual_attr.maxlength / bpf as u32;
    let headphones = matches!(sink.form_factor.as_deref(), Some("headphone" | "headset"))
        || sink.active_port.as_deref().is_some_and(|p| {
            let p = p.to_ascii_lowercase();
            p.contains("headphone") || p.contains("headset")
        });
    let format = DeviceFormat {
        rate: actual_spec.rate,
        channels: channels as u16,
        channel_mask: order.mask,
        headphones,
        backend: BACKEND,
        period_frames: buffer_frames / periods,
        buffer_frames,
        sample_format: sample,
        device_name: sink.description,
    };
    Ok(Opened {
        conn,
        requests,
        format,
        order,
        bytes_per_frame: bpf,
    })
}

fn render_thread(api: Arc<Api>, ctx: ThreadContext) -> Option<Box<dyn RenderCallback>> {
    let ThreadContext {
        opened,
        start,
        shared,
    } = ctx;
    let o = match open_stream(api.clone()) {
        Ok(o) => o,
        Err(e) => {
            let _ = opened.send(Err(e));
            return None;
        }
    };
    {
        let target = WakeTarget {
            api: api.clone(),
            mainloop: o.conn.mainloop,
        };
        shared.set_waker(Some(Box::new(move || target.wake())));
    }
    let _ = opened.send(Ok(o.format.clone()));
    let result = match start.recv() {
        Ok(cmd) => run(&o, cmd, &shared),
        Err(_) => None,
    };
    // The mainloop goes away with `o`: remove the waker first.
    shared.set_waker(None);
    drop(o);
    result
}

/// Fills what the stream can take now (`ma_device_write_to_stream__pulse`);
/// returns frames written (0: nothing writable).
fn write_to_stream(
    o: &Opened,
    renderer: &mut Renderer,
    written: &mut u64,
) -> Result<u64, DeviceError> {
    let api = &o.conn.api;
    let stream = o.conn.stream;
    // SAFETY: live stream, render thread only.
    let writable = unsafe { (api.pa_stream_writable_size)(stream) };
    if writable == usize::MAX {
        return Err(DeviceError::Backend {
            call: "pa_stream_writable_size",
            code: -1,
        });
    }
    if writable == 0 {
        return Ok(0);
    }
    let mut data: *mut c_void = std::ptr::null_mut();
    let mut nbytes = writable;
    // SAFETY: begin_write hands out a server buffer of `nbytes` bytes that
    // stays valid until pa_stream_write (or cancel); the slices below
    // cover only that range and end before the write. f32 slices are made
    // only from a 4-aligned pointer.
    unsafe {
        let rc = (api.pa_stream_begin_write)(stream, &raw mut data, &raw mut nbytes);
        if rc < 0 || data.is_null() {
            return Err(DeviceError::Backend {
                call: "pa_stream_begin_write",
                code: i64::from(rc),
            });
        }
        let frames = nbytes / o.bytes_per_frame;
        let bytes = frames * o.bytes_per_frame;
        let channels = usize::from(o.format.channels);
        if o.format.sample_format == SampleFormat::F32
            && (data as usize).is_multiple_of(std::mem::align_of::<f32>())
        {
            renderer.render_f32(std::slice::from_raw_parts_mut(
                data.cast::<f32>(),
                frames * channels,
            ));
        } else {
            renderer.render_bytes(std::slice::from_raw_parts_mut(data.cast::<u8>(), bytes));
        }
        let rc = (api.pa_stream_write)(stream, data, bytes, std::ptr::null(), 0, PA_SEEK_RELATIVE);
        if rc < 0 {
            return Err(DeviceError::Backend {
                call: "pa_stream_write",
                code: i64::from(rc),
            });
        }
        *written += frames as u64;
        Ok(frames as u64)
    }
}

fn update_clock(o: &Opened, written: u64, shared: &Shared) {
    let mut usec: u64 = 0;
    let mut negative: c_int = 0;
    // SAFETY: live stream; out-params are locals.
    let rc = unsafe {
        (o.conn.api.pa_stream_get_latency)(o.conn.stream, &raw mut usec, &raw mut negative)
    };
    if rc == 0 {
        let queued = if negative != 0 {
            0
        } else {
            usec.saturating_mul(u64::from(o.format.rate)) / 1_000_000
        };
        shared.clock.advance_to(written.saturating_sub(queued));
    }
}

fn run(o: &Opened, cmd: StartCommand, shared: &Arc<Shared>) -> Option<Box<dyn RenderCallback>> {
    let channels = usize::from(o.format.channels);
    let mut renderer = Renderer::new(
        cmd.callback,
        channels,
        o.format.sample_format,
        o.order.clone(),
    );
    let mut written = 0u64;
    // Initial fill, then uncork (ma_device_start__pulse).
    if let Err(e) = write_to_stream(o, &mut renderer, &mut written).and_then(|_| o.conn.cork(false))
    {
        let _ = cmd.reply.send(Err((e, renderer.into_callback())));
        return None;
    }
    let _ = cmd.reply.send(Ok(()));
    shared.set_state(DeviceState::Running);

    let mut end_state = None;
    while !shared.stop_requested() {
        if o.conn.iterate(-1).is_err() {
            end_state = Some(DeviceState::Failed);
            break;
        }
        // SAFETY: live context, render thread only.
        let context_state = unsafe { (o.conn.api.pa_context_get_state)(o.conn.context) };
        let stream_state = o.conn.stream_state();
        if matches!(context_state, PA_CONTEXT_FAILED | PA_CONTEXT_TERMINATED)
            || matches!(stream_state, PA_STREAM_FAILED | PA_STREAM_TERMINATED)
        {
            end_state = Some(DeviceState::Lost);
            break;
        }
        let requested = o.requests.bytes.swap(0, Ordering::AcqRel) as u64;
        let mut processed = 0u64;
        while processed < requested {
            match write_to_stream(o, &mut renderer, &mut written) {
                Ok(0) => break,
                Ok(frames) => processed += frames * o.bytes_per_frame as u64,
                Err(_) => {
                    end_state = Some(DeviceState::Lost);
                    break;
                }
            }
        }
        if end_state.is_some() {
            break;
        }
        update_clock(o, written, shared);
        if renderer.failed() {
            end_state = Some(DeviceState::Failed);
            break;
        }
    }
    if let Some(state) = end_state {
        shared.set_state(state);
        shared.wait_for_stop();
    } else {
        let _ = o.conn.cork(true);
    }
    Some(renderer.into_callback())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// Layout of the libpulse structs on x86_64 (checked against the
    /// pulse 17 headers with offsetof).
    #[test]
    fn struct_layouts_match_libpulse() {
        assert_eq!(size_of::<PaSampleSpec>(), 12);
        assert_eq!(size_of::<PaChannelMap>(), 132);
        assert_eq!(size_of::<PaBufferAttr>(), 20);
        assert_eq!(offset_of!(PaSinkInfo, sample_spec), 24);
        assert_eq!(offset_of!(PaSinkInfo, channel_map), 36);
        assert_eq!(offset_of!(PaSinkInfo, owner_module), 168);
        assert_eq!(offset_of!(PaSinkInfo, volume), 172);
        assert_eq!(offset_of!(PaSinkInfo, mute), 304);
        assert_eq!(offset_of!(PaSinkInfo, proplist), 344);
        assert_eq!(offset_of!(PaSinkInfo, ports), 384);
        assert_eq!(offset_of!(PaSinkInfo, active_port), 392);
        assert_eq!(offset_of!(PaSinkInfo, n_formats), 400);
        assert_eq!(size_of::<PaSinkInfo>(), 416);
    }

    #[test]
    fn sample_formats() {
        assert_eq!(sample_format(PA_SAMPLE_FLOAT32LE), Some(SampleFormat::F32));
        assert_eq!(sample_format(PA_SAMPLE_S16LE), Some(SampleFormat::S16));
        assert_eq!(sample_format(11), None); // S24_32LE: unsupported in 0.10.42
    }
}
