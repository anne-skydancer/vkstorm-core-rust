//! The output device: enumeration by endpoint identity, opening in the
//! endpoint's native format, and a render thread that pulls interleaved
//! float frames from the engine.
//!
//! Backends: WASAPI shared mode on Windows, PulseAudio then ALSA (loaded
//! at run time) on Linux, and a null device that renders on demand for
//! tests and offline rendering. The behaviour reproduces what the viewer's
//! SoLoud backend got from miniaudio 0.10.42 with the 3p-soloud patches
//! (`miniaudio-device-selection.patch`, `miniaudio-bounded-mix.patch`);
//! each backend file says where and how.
//!
//! CONTRACT (shared with the mixer; change only together):
//! - The render callback receives an interleaved `f32` slice of
//!   `frames * format.channels` samples and must fill it completely. It is
//!   called with at most [`MAX_CALLBACK_FRAMES`] frames per call (a larger
//!   device period is split, as the viewer's SoLoud package did).
//! - The device runs at its native rate and channel count (the endpoint's
//!   mix format); only 1, 2, 4, 6 and 8 channels are accepted, anything
//!   else fails to open.
//! - The device clock (frames played) is the number of frames the device
//!   has played out of everything the callback produced (written minus
//!   what is still queued in the device), monotonic, readable from any
//!   thread.
//!
//! Additions (compatible with the above):
//! - The slice handed to the callback is zeroed before each call, and the
//!   device clamps what the callback wrote to [-1, 1] afterwards
//!   (miniaudio's pre-zeroed output buffer and `noClip = false`).
//! - Samples are in the order of [`DeviceFormat::channel_mask`]'s set bits,
//!   ascending (FL FR FC LFE BL BR SL SR); the backend reorders for the
//!   device. The mask always has exactly `channels` bits.
//! - Calls happen only on the device's render thread, one at a time, in
//!   stream order. After [`RunningDevice::stop`] returns no call runs.
//! - The format chosen at open never changes while the device runs: when
//!   Windows moves the default endpoint the stream follows it in the same
//!   format (WASAPI converts), see [`DeviceEvents::rerouted`].
//! - A panic in the callback is contained: the device outputs silence and
//!   reports [`DeviceState::Failed`].
//! - Call sizes follow miniaudio's: on a float device, the frames the
//!   device asks for (WASAPI: whatever is free once at least a period is,
//!   e.g. 1920 at 48 kHz, split 1024 + 896); on a converting device
//!   4096 / (4 x channels) frames per call (ALSA: per write chunk).

pub mod convert;
pub mod identity;
pub mod layout;
pub mod md5;
mod null;
mod render;

#[cfg(target_os = "linux")]
mod alsa;
#[cfg(target_os = "linux")]
mod pulse;
#[cfg(windows)]
mod wasapi;

use std::any::Any;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

pub use convert::SampleFormat;
pub use identity::DeviceUuid;
pub use null::{NullConfig, NullDevice};
pub use render::render_split;

/// Most frames handed to the render callback in one call.
pub const MAX_CALLBACK_FRAMES: usize = 1024;

/// The device period miniaudio was asked for (3p-soloud patch:
/// `periodSizeInMilliseconds = 40`).
pub const PERIOD_MILLISECONDS: u32 = 40;

/// miniaudio's default period count (`MA_DEFAULT_PERIODS`).
pub const PERIODS: u32 = 3;

/// The engine's audio producer, called on the render thread.
///
/// `Any` lets the engine get its concrete mixer back from
/// [`RunningDevice::stop`] (`Box<dyn RenderCallback>` upcasts to
/// `Box<dyn Any>`).
pub trait RenderCallback: Any + Send {
    /// Fill `out` (`frames * channels` interleaved samples, zeroed on entry,
    /// `frames <= MAX_CALLBACK_FRAMES`).
    fn render(&mut self, out: &mut [f32], frames: usize);
}

impl<F> RenderCallback for F
where
    F: FnMut(&mut [f32], usize) + Send + 'static,
{
    fn render(&mut self, out: &mut [f32], frames: usize) {
        self(out, frames)
    }
}

/// The negotiated output format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceFormat {
    /// Frames per second (the endpoint's native rate).
    pub rate: u32,
    /// 1, 2, 4, 6 or 8.
    pub channels: u16,
    /// WAVEFORMATEXTENSIBLE speaker bits, `channels` of them; the
    /// interleave order of the callback's slice.
    pub channel_mask: u32,
    /// The endpoint is headphones or a headset (WASAPI
    /// `PKEY_AudioEndpoint_FormFactor` Headphones/Headset; PulseAudio
    /// `device.form_factor` headphone/headset or a headphone/headset active
    /// port; ALSA: never known, false). The music speaker fill is skipped
    /// on headphones.
    pub headphones: bool,
    /// miniaudio's backend names: "WASAPI", "PulseAudio", "ALSA", "Null".
    pub backend: &'static str,
    /// The device's period in frames (what SoLoud was given as its buffer
    /// size: miniaudio's `internalPeriodSizeInFrames`).
    pub period_frames: u32,
    /// The device's whole buffer in frames.
    pub buffer_frames: u32,
    /// The sample format the device takes (the callback always renders
    /// `f32`; the backend converts).
    pub sample_format: SampleFormat,
    /// The endpoint's friendly name (sink description, PCM name).
    pub device_name: String,
}

/// An output endpoint as enumerated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The identity the viewer stores ([`identity`]).
    pub uuid: DeviceUuid,
    /// Friendly name (`PKEY_Device_FriendlyName`).
    pub name: String,
    /// The current default render endpoint (console role).
    pub is_default: bool,
    /// Form factor Headphones or Headset.
    pub headphones: bool,
    /// The backend's endpoint id (WASAPI: `IMMDevice::GetId`, UTF-16).
    endpoint_id: Vec<u16>,
}

impl DeviceInfo {
    /// The endpoint id as text, for logs.
    pub fn endpoint_id(&self) -> String {
        String::from_utf16_lossy(&self.endpoint_id)
    }
}

/// Which endpoint to open.
#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    /// The default render endpoint (WASAPI `eConsole` role, as miniaudio
    /// used); the stream follows the default when it changes.
    Default,
    /// An enumerated endpoint; if it goes away the device reports
    /// [`DeviceState::Lost`] and stays silent until the engine reopens.
    Endpoint(&'a DeviceInfo),
}

#[derive(Debug)]
pub enum DeviceError {
    /// No audio backend could be loaded or connected.
    NoBackend(String),
    /// The requested endpoint is not present.
    NotFound,
    /// The endpoint's native channel count is not 1, 2, 4, 6 or 8.
    UnsupportedChannels(u32),
    /// The endpoint's native sample format cannot be written.
    UnsupportedFormat(String),
    /// A backend call failed (API name and code).
    Backend { call: &'static str, code: i64 },
    /// The render thread could not be created or ended unexpectedly.
    Thread(String),
    /// The configuration given to the null device is invalid.
    InvalidConfig(String),
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceError::NoBackend(why) => write!(f, "no audio backend available: {why}"),
            DeviceError::NotFound => f.write_str("output device not found"),
            DeviceError::UnsupportedChannels(n) => {
                write!(
                    f,
                    "output device has {n} channels; 1, 2, 4, 6 or 8 are supported"
                )
            }
            DeviceError::UnsupportedFormat(what) => write!(f, "unsupported output format: {what}"),
            DeviceError::Backend { call, code } => write!(f, "{call} failed ({code:#x})"),
            DeviceError::Thread(why) => write!(f, "audio render thread: {why}"),
            DeviceError::InvalidConfig(why) => write!(f, "invalid device configuration: {why}"),
        }
    }
}

impl std::error::Error for DeviceError {}

/// The running device's condition, for the engine's idle poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceState {
    Running,
    /// Default endpoint mode with no usable default render endpoint (all
    /// unplugged/disabled, or the endpoint failed): silent, waiting for a
    /// new default (checked on every endpoint notification and once a
    /// second).
    Detached,
    /// The selected endpoint was unplugged, disabled or invalidated: the
    /// callback is no longer called. Reopen to recover.
    Lost,
    /// The backend failed (timeouts, errors, a panicking callback): the
    /// callback is no longer called. Reopen to recover.
    Failed,
}

/// Edge-triggered events since the last [`RunningDevice::take_events`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceEvents {
    /// Times the stream moved to a new default endpoint.
    pub rerouted: u32,
    /// The device entered [`DeviceState::Lost`].
    pub lost: bool,
    /// The device entered [`DeviceState::Failed`].
    pub failed: bool,
}

/// Frames played by the device, readable from any thread.
#[derive(Clone, Debug, Default)]
pub struct DeviceClock(Arc<AtomicU64>);

impl DeviceClock {
    pub fn new() -> DeviceClock {
        DeviceClock::default()
    }

    /// Frames played since the device started.
    pub fn frames_played(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    /// Raises the clock to `frames` (never lowers it).
    #[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
    pub(crate) fn advance_to(&self, frames: u64) {
        self.0.fetch_max(frames, Ordering::AcqRel);
    }

    /// Sets the clock (null device only).
    pub(crate) fn set(&self, frames: u64) {
        self.0.store(frames, Ordering::Release);
    }
}

const STATE_RUNNING: u8 = 0;
const STATE_DETACHED: u8 = 1;
const STATE_LOST: u8 = 2;
const STATE_FAILED: u8 = 3;

type Waker = Box<dyn Fn() + Send>;

/// State shared between the render thread and the device handles.
pub(crate) struct Shared {
    pub(crate) clock: DeviceClock,
    state: AtomicU8,
    rerouted: AtomicU32,
    lost: AtomicBool,
    failed: AtomicBool,
    headphones: AtomicBool,
    device_name: Mutex<String>,
    stop: Mutex<bool>,
    stop_cv: Condvar,
    /// Wakes the render thread out of its backend wait (set by the render
    /// thread while its wait object exists, cleared before it is freed).
    waker: Mutex<Option<Waker>>,
}

#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
impl Shared {
    fn new() -> Arc<Shared> {
        Arc::new(Shared {
            clock: DeviceClock::new(),
            state: AtomicU8::new(STATE_RUNNING),
            rerouted: AtomicU32::new(0),
            lost: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            headphones: AtomicBool::new(false),
            device_name: Mutex::new(String::new()),
            stop: Mutex::new(false),
            stop_cv: Condvar::new(),
            waker: Mutex::new(None),
        })
    }

    pub(crate) fn stop_requested(&self) -> bool {
        self.stop.lock().map(|g| *g).unwrap_or(true)
    }

    /// Blocks the render thread until stop is requested (after the device
    /// was lost or failed: nothing left to do, never spins).
    pub(crate) fn wait_for_stop(&self) {
        let Ok(mut stopped) = self.stop.lock() else {
            return;
        };
        while !*stopped {
            match self.stop_cv.wait(stopped) {
                Ok(g) => stopped = g,
                Err(_) => return,
            }
        }
    }

    fn request_stop(&self) {
        if let Ok(mut g) = self.stop.lock() {
            *g = true;
        }
        self.stop_cv.notify_all();
        if let Ok(w) = self.waker.lock()
            && let Some(wake) = w.as_ref()
        {
            wake();
        }
    }

    pub(crate) fn set_waker(&self, waker: Option<Waker>) {
        if let Ok(mut w) = self.waker.lock() {
            *w = waker;
        }
    }

    pub(crate) fn set_state(&self, state: DeviceState) {
        let v = match state {
            DeviceState::Running => STATE_RUNNING,
            DeviceState::Detached => STATE_DETACHED,
            DeviceState::Lost => STATE_LOST,
            DeviceState::Failed => STATE_FAILED,
        };
        let previous = self.state.swap(v, Ordering::AcqRel);
        if previous != v {
            match state {
                DeviceState::Lost => self.lost.store(true, Ordering::Release),
                DeviceState::Failed => self.failed.store(true, Ordering::Release),
                _ => {}
            }
        }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn note_reroute(&self, headphones: bool, name: &str) {
        self.headphones.store(headphones, Ordering::Release);
        self.set_device_name(name);
        self.rerouted.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn set_device_name(&self, name: &str) {
        if let Ok(mut n) = self.device_name.lock() {
            n.clear();
            n.push_str(name);
        }
    }

    fn state(&self) -> DeviceState {
        match self.state.load(Ordering::Acquire) {
            STATE_RUNNING => DeviceState::Running,
            STATE_DETACHED => DeviceState::Detached,
            STATE_LOST => DeviceState::Lost,
            _ => DeviceState::Failed,
        }
    }
}

pub(crate) type StartReply = Result<(), (DeviceError, Box<dyn RenderCallback>)>;

/// The start command a backend thread waits for after opening.
pub(crate) struct StartCommand {
    pub(crate) callback: Box<dyn RenderCallback>,
    pub(crate) reply: mpsc::SyncSender<StartReply>,
}

/// What a backend thread gets: where to report the opened format, where to
/// wait for the start command, and the shared state.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(crate) struct ThreadContext {
    pub(crate) opened: mpsc::SyncSender<Result<DeviceFormat, DeviceError>>,
    pub(crate) start: mpsc::Receiver<StartCommand>,
    pub(crate) shared: Arc<Shared>,
}

/// Spawns a backend render thread and waits for it to report the opened
/// format. `body` returns the callback (if it was ever started) when the
/// thread ends.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(crate) fn spawn_backend<F>(name: &str, body: F) -> Result<OpenedDevice, DeviceError>
where
    F: FnOnce(ThreadContext) -> Option<Box<dyn RenderCallback>> + Send + 'static,
{
    let shared = Shared::new();
    let (opened_tx, opened_rx) = mpsc::sync_channel(1);
    let (start_tx, start_rx) = mpsc::sync_channel(1);
    let ctx = ThreadContext {
        opened: opened_tx,
        start: start_rx,
        shared: shared.clone(),
    };
    let thread = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || body(ctx))
        .map_err(|e| DeviceError::Thread(e.to_string()))?;
    let format = match opened_rx.recv() {
        Ok(Ok(format)) => format,
        Ok(Err(e)) => {
            let _ = thread.join();
            return Err(e);
        }
        Err(_) => {
            let _ = thread.join();
            return Err(DeviceError::Thread(
                "render thread ended while opening".into(),
            ));
        }
    };
    shared
        .headphones
        .store(format.headphones, Ordering::Release);
    shared.set_device_name(&format.device_name);
    Ok(OpenedDevice {
        format,
        shared,
        start: Some(start_tx),
        thread: Some(thread),
    })
}

/// An opened, not yet started output device. Dropping it closes the device.
pub struct OpenedDevice {
    format: DeviceFormat,
    shared: Arc<Shared>,
    start: Option<mpsc::SyncSender<StartCommand>>,
    thread: Option<JoinHandle<Option<Box<dyn RenderCallback>>>>,
}

/// Starting failed; the callback is handed back.
pub struct StartError {
    pub error: DeviceError,
    pub callback: Box<dyn RenderCallback>,
}

impl fmt::Debug for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StartError")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "starting the output device failed: {}", self.error)
    }
}

impl OpenedDevice {
    pub fn format(&self) -> &DeviceFormat {
        &self.format
    }

    /// Starts rendering with `callback`. Fails if the backend refuses to
    /// start the stream (the 3p-soloud patch reported this too).
    pub fn start(mut self, callback: Box<dyn RenderCallback>) -> Result<RunningDevice, StartError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let thread = self.thread.take();
        let Some(start) = self.start.take() else {
            return Err(StartError {
                error: DeviceError::Thread("already started".into()),
                callback,
            });
        };
        if let Err(mpsc::SendError(cmd)) = start.send(StartCommand {
            callback,
            reply: reply_tx,
        }) {
            if let Some(t) = thread {
                let _ = t.join();
            }
            return Err(StartError {
                error: DeviceError::Thread("render thread ended before start".into()),
                callback: cmd.callback,
            });
        }
        match reply_rx.recv() {
            Ok(Ok(())) => Ok(RunningDevice {
                format: self.format.clone(),
                shared: self.shared.clone(),
                thread,
            }),
            Ok(Err((error, callback))) => {
                if let Some(t) = thread {
                    let _ = t.join();
                }
                Err(StartError { error, callback })
            }
            Err(_) => {
                // The thread ended holding the callback (it never panics by
                // design; this is a defensive path).
                let returned = thread.and_then(|t| t.join().ok().flatten());
                let callback = returned.unwrap_or_else(|| Box::new(|_: &mut [f32], _: usize| {}));
                Err(StartError {
                    error: DeviceError::Thread("render thread ended while starting".into()),
                    callback,
                })
            }
        }
    }
}

impl Drop for OpenedDevice {
    fn drop(&mut self) {
        // Closing the start channel tells the thread to release the device.
        self.start.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A running output device. Dropping it stops the device (and drops the
/// callback); use [`RunningDevice::stop`] to get the callback back.
pub struct RunningDevice {
    format: DeviceFormat,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<Option<Box<dyn RenderCallback>>>>,
}

impl RunningDevice {
    /// The format the callback renders in (fixed for the device's life).
    pub fn format(&self) -> &DeviceFormat {
        &self.format
    }

    /// The device clock (clone it for other threads).
    pub fn clock(&self) -> DeviceClock {
        self.shared.clock.clone()
    }

    pub fn state(&self) -> DeviceState {
        self.shared.state()
    }

    /// Events since the previous call.
    pub fn take_events(&self) -> DeviceEvents {
        DeviceEvents {
            rerouted: self.shared.rerouted.swap(0, Ordering::AcqRel),
            lost: self.shared.lost.swap(false, Ordering::AcqRel),
            failed: self.shared.failed.swap(false, Ordering::AcqRel),
        }
    }

    /// Whether the endpoint currently playing is headphones/headset (can
    /// change when the default endpoint moves).
    pub fn headphones(&self) -> bool {
        self.shared.headphones.load(Ordering::Acquire)
    }

    /// The endpoint currently playing (changes when the default moves).
    pub fn device_name(&self) -> String {
        self.shared
            .device_name
            .lock()
            .map(|n| n.clone())
            .unwrap_or_default()
    }

    /// Stops the device and joins the render thread: no callback runs
    /// after this returns. Returns the callback (None only if the render
    /// thread itself panicked, which it is written never to do).
    pub fn stop(mut self) -> Option<Box<dyn RenderCallback>> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Option<Box<dyn RenderCallback>> {
        self.shared.request_stop();
        self.thread.take().and_then(|t| t.join().ok().flatten())
    }
}

impl Drop for RunningDevice {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}

/// The output endpoints, as the device selector lists them.
///
/// Windows: active WASAPI render endpoints in `EnumAudioEndpoints` order.
/// Linux: empty, as today (the SoLoud adapter only derived identities for
/// WASAPI, DirectSound and WinMM and skipped every other backend), so the
/// selector offers only the default. PulseAudio could list sinks later
/// (`pa_context_get_sink_info_list`, identity = MD5("PulseAudio" ++ sink
/// name), opened with `DONT_MOVE` as miniaudio did for named sinks), and
/// ALSA PCMs by hint (`snd_device_name_hint`).
pub fn enumerate() -> Result<Vec<DeviceInfo>, DeviceError> {
    #[cfg(windows)]
    {
        wasapi::enumerate()
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

/// Opens an output device (not yet started).
pub fn open(target: Target<'_>) -> Result<OpenedDevice, DeviceError> {
    #[cfg(windows)]
    {
        wasapi::open(target)
    }
    #[cfg(target_os = "linux")]
    {
        match target {
            // Nothing is enumerated on Linux, so no endpoint can be named.
            Target::Endpoint(_) => Err(DeviceError::NotFound),
            // miniaudio's backend order: PulseAudio, then ALSA (then JACK,
            // not carried over: it needs a running JACK server and the
            // viewer never documented or tested it).
            Target::Default => match pulse::open_default() {
                Ok(device) => Ok(device),
                Err(pulse_error) => alsa::open_default().map_err(|alsa_error| {
                    let reason = |e: DeviceError| match e {
                        DeviceError::NoBackend(why) => why,
                        other => other.to_string(),
                    };
                    DeviceError::NoBackend(format!(
                        "PulseAudio: {}; ALSA: {}",
                        reason(pulse_error),
                        reason(alsa_error)
                    ))
                }),
            },
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = target;
        Err(DeviceError::NoBackend(
            "no output backend on this platform".into(),
        ))
    }
}

/// Linux backends opened individually (qualification and tests; the engine
/// calls [`open`]).
#[cfg(target_os = "linux")]
pub mod linux {
    use super::{DeviceError, OpenedDevice};

    /// PulseAudio, default sink.
    pub fn open_pulse_default() -> Result<OpenedDevice, DeviceError> {
        super::pulse::open_default()
    }

    /// ALSA, the "default" PCM (or miniaudio's fallbacks).
    pub fn open_alsa_default() -> Result<OpenedDevice, DeviceError> {
        super::alsa::open_default()
    }
}

/// Frames in `ms` milliseconds at `rate`
/// (`ma_calculate_buffer_size_in_frames_from_milliseconds`).
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(crate) fn frames_from_ms(ms: u32, rate: u32) -> u32 {
    ((u64::from(ms) * u64::from(rate)) / 1000) as u32
}
