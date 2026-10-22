//! WASAPI shared-mode output (Windows).
//!
//! Reproduces miniaudio 0.10.42's WASAPI backend as the 3p-soloud patches
//! configured it (`format = f32, channels = 0, sampleRate = 0,
//! periodSizeInMilliseconds = 40`, default device config otherwise):
//!
//! - Enumeration: `IMMDeviceEnumerator::EnumAudioEndpoints(eRender,
//!   DEVICE_STATE_ACTIVE)` in collection order, the default flag from
//!   `GetDefaultAudioEndpoint(eRender, eConsole)`, names from
//!   `PKEY_Device_FriendlyName`, plus `PKEY_AudioEndpoint_FormFactor` for
//!   the headphone check. (miniaudio dropped endpoints whose id did not fit
//!   its 64-unit buffer; real ids are 55 units, so nothing is dropped
//!   here.)
//! - Opening: the selected endpoint (`GetDevice`) or the default render
//!   endpoint of the console role; hardware offload requested when the
//!   endpoint is offload-capable (`IAudioClient2`, category Other);
//!   `GetMixFormat` used unchanged (shared mode: rate, channels, mask and
//!   sample type are the endpoint's; an integer mix format is written by
//!   converting in software as miniaudio did, see `convert.rs`); 1/2/4/6/8
//!   channels only; period = 40 ms of frames; `IAudioClient3` low-latency
//!   init only if its clamped engine period is at least that period
//!   (practically never), otherwise `Initialize(SHARED,
//!   EVENTCALLBACK, 3 periods, 0)`; the period actually used is a third of
//!   the buffer.
//! - Rendering, on a dedicated thread at `THREAD_PRIORITY_HIGHEST`
//!   (miniaudio's `ma_thread_priority_highest`; 0.10.42 does not use MMCSS):
//!   on each wake, free = buffer - padding; when at least a period is free
//!   the whole free space is rendered and released; the client starts
//!   after the first fill (a full buffer of prefill); waits on the event
//!   with miniaudio's 5 s timeout (a timeout is a failure).
//! - Default following: a stream opened on the default endpoint follows a
//!   new default of the console render role (`IMMNotificationClient::
//!   OnDefaultDeviceChanged`), and recovers when its endpoint becomes
//!   active again, as miniaudio's automatic stream routing did. miniaudio
//!   reopened with the new endpoint's mix format at the old rate (WASAPI
//!   `AUTOCONVERTPCM` resampling) and channel-mapped in software; here the
//!   stream reopens in the old format with `AUTOCONVERTPCM |
//!   SRC_DEFAULT_QUALITY`, so WASAPI also does any channel-count conversion.
//!   Same-layout moves (the usual case) are identical. Rerouting runs on
//!   the render thread, not in the notification callback (miniaudio's
//!   callback-thread reinit leaked the old client and raced the data loop).
//! - An explicitly selected endpoint is never moved: if it is invalidated
//!   or leaves the active state the device reports `Lost` (miniaudio just
//!   stopped, silently) and waits for the engine to stop it.
//! - The device clock is frames released minus `GetCurrentPadding`,
//!   updated at every wake (each engine period, typically 10 ms).
//! - Stop: `IAudioClient::Stop` then `Reset`. miniaudio's pre-stop drain
//!   loop waited `buffer / rate` milliseconds in integer arithmetic (0 ms),
//!   so it ended at once; no drain is done here either.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, PROPERTYKEY, RPC_E_CHANGED_MODE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Media::Audio::{
    AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    AudioCategory_Other, AudioClientProperties, DEVICE_STATE, DEVICE_STATE_ACTIVE, EDataFlow,
    ERole, Headphones, Headset, IAudioClient, IAudioClient2, IAudioClient3, IAudioRenderClient,
    IMMDevice, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
    MMDeviceEnumerator, PKEY_AudioEndpoint_FormFactor, WAVE_FORMAT_PCM, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE, eConsole, eRender,
};
use windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, STGM_READ,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentThread, INFINITE, SetEvent, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
    WaitForMultipleObjects,
};
use windows::Win32::System::Variant::{VT_LPWSTR, VT_UI4};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::core::{Interface, PCWSTR};

use super::convert::SampleFormat;
use super::identity::DeviceUuid;
use super::layout::{DeviceOrder, is_supported_channel_count, wasapi_effective_mask};
use super::render::Renderer;
use super::{
    DeviceError, DeviceFormat, DeviceInfo, DeviceState, OpenedDevice, PERIOD_MILLISECONDS, PERIODS,
    RenderCallback, Shared, Target, ThreadContext, frames_from_ms, spawn_backend,
};

const BACKEND: &str = "WASAPI";
/// `MA_WASAPI_WAIT_TIMEOUT_MILLISECONDS`.
const WAIT_TIMEOUT_MS: u32 = 5000;
/// How often a detached default stream looks for a default endpoint
/// besides reacting to endpoint notifications.
const DETACHED_RETRY_MS: u32 = 1000;
/// `E_NOTFOUND` (`HRESULT_FROM_WIN32(ERROR_NOT_FOUND)`) from `GetDevice`.
const E_NOTFOUND: i32 = 0x8007_0490_u32 as i32;

fn backend_error(call: &'static str, e: &windows::core::Error) -> DeviceError {
    DeviceError::Backend {
        call,
        code: i64::from(e.code().0 as u32),
    }
}

/// COM for the calling thread, released on drop if this guard initialized
/// it. A thread already in a single-threaded apartment keeps it (MMDevice
/// works in either apartment).
struct ComGuard {
    uninitialize: bool,
}

impl ComGuard {
    fn new() -> Result<ComGuard, DeviceError> {
        // SAFETY: CoInitializeEx has no memory preconditions; a success
        // (S_OK/S_FALSE) is balanced by CoUninitialize in Drop on the same
        // thread (the guard is never sent: it is not Send).
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_ok() {
            Ok(ComGuard { uninitialize: true })
        } else if hr == RPC_E_CHANGED_MODE {
            Ok(ComGuard {
                uninitialize: false,
            })
        } else {
            Err(DeviceError::Backend {
                call: "CoInitializeEx",
                code: i64::from(hr.0 as u32),
            })
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.uninitialize {
            // SAFETY: balances the successful CoInitializeEx of this thread;
            // every COM object this guard covered was dropped before (field
            // and local drop order in the callers).
            unsafe { CoUninitialize() };
        }
    }
}

/// An auto-reset Win32 event, closed on drop.
struct Event(HANDLE);

// SAFETY: an event handle is a process-wide kernel object reference; any
// thread may signal or wait on it. The handle stays open until the last
// Arc<Event> is dropped, so no thread can use it after CloseHandle.
unsafe impl Send for Event {}
// SAFETY: as above; SetEvent/WaitForMultipleObjects are thread-safe.
unsafe impl Sync for Event {}

impl Event {
    fn new() -> Result<Event, DeviceError> {
        // SAFETY: no security attributes, unnamed; the returned handle is
        // owned by the Event and closed exactly once in Drop.
        let handle = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map_err(|e| backend_error("CreateEventW", &e))?;
        Ok(Event(handle))
    }

    fn set(&self) {
        // SAFETY: self.0 is a valid event handle for the Event's lifetime.
        let _ = unsafe { SetEvent(self.0) };
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle was created by CreateEventW and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn device_enumerator() -> Result<IMMDeviceEnumerator, DeviceError> {
    // SAFETY: COM is initialized on this thread (callers hold a ComGuard).
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(|e| backend_error("CoCreateInstance(MMDeviceEnumerator)", &e))
}

/// `IMMDevice::GetId` as UTF-16 code units without the terminator.
fn endpoint_id(device: &IMMDevice) -> Result<Vec<u16>, DeviceError> {
    // SAFETY: GetId returns a NUL-terminated CoTaskMem string that the
    // caller owns; it is copied and then freed exactly once.
    unsafe {
        let id = device
            .GetId()
            .map_err(|e| backend_error("IMMDevice::GetId", &e))?;
        if id.is_null() {
            return Err(DeviceError::Backend {
                call: "IMMDevice::GetId",
                code: 0,
            });
        }
        let units = id.as_wide().to_vec();
        CoTaskMemFree(Some(id.0 as *const _));
        Ok(units)
    }
}

/// Reads a property, hands it to `read`, and clears it.
fn with_property<T>(
    store: &IPropertyStore,
    key: &PROPERTYKEY,
    read: impl FnOnce(&PROPVARIANT) -> Option<T>,
) -> Option<T> {
    // SAFETY: GetValue fills a PROPVARIANT we own; it is cleared exactly
    // once with PropVariantClear after `read` (which only borrows it).
    unsafe {
        let mut value = store.GetValue(key).ok()?;
        let result = read(&value);
        let _ = PropVariantClear(&mut value);
        result
    }
}

struct EndpointProperties {
    name: String,
    headphones: bool,
}

fn endpoint_properties(device: &IMMDevice) -> EndpointProperties {
    // SAFETY: COM call on a valid IMMDevice.
    let Ok(store) = (unsafe { device.OpenPropertyStore(STGM_READ) }) else {
        return EndpointProperties {
            name: String::new(),
            headphones: false,
        };
    };
    let name = with_property(&store, &PKEY_Device_FriendlyName, |v| {
        // SAFETY: the union is read only after checking the VARENUM tag;
        // VT_LPWSTR holds a NUL-terminated wide string (or null).
        unsafe {
            let inner = &v.Anonymous.Anonymous;
            if inner.vt != VT_LPWSTR {
                return None;
            }
            let p = inner.Anonymous.pwszVal;
            if p.is_null() {
                None
            } else {
                Some(String::from_utf16_lossy(p.as_wide()))
            }
        }
    })
    .unwrap_or_default();
    let form_factor = with_property(&store, &PKEY_AudioEndpoint_FormFactor, |v| {
        // SAFETY: tag checked before reading the VT_UI4 member.
        unsafe {
            let inner = &v.Anonymous.Anonymous;
            if inner.vt == VT_UI4 {
                Some(inner.Anonymous.ulVal)
            } else {
                None
            }
        }
    });
    let headphones =
        matches!(form_factor, Some(f) if f as i32 == Headphones.0 || f as i32 == Headset.0);
    EndpointProperties { name, headphones }
}

pub(super) fn enumerate() -> Result<Vec<DeviceInfo>, DeviceError> {
    let _com = ComGuard::new()?;
    let enumerator = device_enumerator()?;
    // SAFETY: COM calls on a valid enumerator; returned interfaces are
    // reference counted and released by Drop before `_com`.
    let default_id = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .ok()
        .and_then(|d| endpoint_id(&d).ok());
    // SAFETY: as above.
    let collection = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
        .map_err(|e| backend_error("IMMDeviceEnumerator::EnumAudioEndpoints", &e))?;
    // SAFETY: as above.
    let count = unsafe { collection.GetCount() }
        .map_err(|e| backend_error("IMMDeviceCollection::GetCount", &e))?;
    let mut devices = Vec::with_capacity(count as usize);
    for i in 0..count {
        // SAFETY: index within GetCount.
        let Ok(device) = (unsafe { collection.Item(i) }) else {
            continue;
        };
        let Ok(id) = endpoint_id(&device) else {
            continue;
        };
        let props = endpoint_properties(&device);
        devices.push(DeviceInfo {
            uuid: DeviceUuid::from_wasapi_endpoint_id(&id),
            name: props.name,
            is_default: default_id.as_ref() == Some(&id),
            headphones: props.headphones,
            endpoint_id: id,
        });
    }
    Ok(devices)
}

pub(super) fn open(target: Target<'_>) -> Result<OpenedDevice, DeviceError> {
    let explicit = match target {
        Target::Default => None,
        Target::Endpoint(info) => Some(info.endpoint_id.clone()),
    };
    spawn_backend("vks-audio-wasapi", move |ctx| render_thread(explicit, ctx))
}

/// Flags set by the notification client, consumed by the render thread.
struct Notifications {
    wake: Arc<Event>,
    default_changed: AtomicBool,
    endpoint_changed: AtomicBool,
    /// The endpoint the stream is on (compared in OnDeviceStateChanged).
    watched: Mutex<Vec<u16>>,
}

#[windows_core::implement(IMMNotificationClient)]
struct Notifier {
    state: Arc<Notifications>,
}

impl IMMNotificationClient_Impl for Notifier_Impl {
    fn OnDeviceStateChanged(&self, id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
        if id.is_null() {
            return Ok(());
        }
        // SAFETY: the system passes a valid NUL-terminated id for the
        // duration of the call.
        let id = unsafe { id.as_wide() };
        let ours = self
            .state
            .watched
            .lock()
            .map(|w| w.as_slice() == id)
            .unwrap_or(false);
        if ours {
            self.state.endpoint_changed.store(true, Ordering::Release);
            self.state.wake.set();
        }
        Ok(())
    }

    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _id: &PCWSTR,
    ) -> windows::core::Result<()> {
        // miniaudio only followed the console role of its own data flow.
        if flow == eRender && role == eConsole {
            self.state.default_changed.store(true, Ordering::Release);
            self.state.wake.set();
        }
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _id: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

/// The client format the stream was first opened with; reroutes reopen in
/// exactly this format.
#[derive(Clone, Copy)]
struct ClientFormat {
    wf: WAVEFORMATEXTENSIBLE,
    sample_format: SampleFormat,
    channels: u32,
    rate: u32,
    mask: u32,
}

/// One opened WASAPI stream on one endpoint.
struct Stream {
    device: IMMDevice,
    client: IAudioClient,
    render: IAudioRenderClient,
    id: Vec<u16>,
    props: EndpointProperties,
    buffer_frames: u32,
    period_frames: u32,
    frame_bytes: usize,
    started: bool,
}

impl Stream {
    /// One pass of miniaudio's playback data loop: read the padding,
    /// update the clock, render the free space if at least a period is
    /// free, and start the client after its first fill.
    fn pump(
        &mut self,
        renderer: &mut Renderer,
        channels: usize,
        written: &mut u64,
        shared: &Shared,
    ) -> windows::core::Result<()> {
        // SAFETY: COM calls on valid interfaces owned by this stream.
        let padding = unsafe { self.client.GetCurrentPadding() }?;
        shared
            .clock
            .advance_to(written.saturating_sub(u64::from(padding)));
        let available = self.buffer_frames.saturating_sub(padding);
        if available > 0 && available >= self.period_frames {
            // SAFETY: GetBuffer returns a writable buffer for `available`
            // frames of the initialized format (frame_bytes each), valid
            // until ReleaseBuffer; the slices below cover exactly it and
            // are dropped before ReleaseBuffer. f32 slices are made only
            // from a 4-aligned pointer.
            unsafe {
                let ptr = self.render.GetBuffer(available)?;
                let len_bytes = available as usize * self.frame_bytes;
                if renderer.format() == SampleFormat::F32
                    && (ptr as usize).is_multiple_of(std::mem::align_of::<f32>())
                {
                    let out = std::slice::from_raw_parts_mut(
                        ptr.cast::<f32>(),
                        available as usize * channels,
                    );
                    renderer.render_f32(out);
                } else {
                    let out = std::slice::from_raw_parts_mut(ptr, len_bytes);
                    renderer.render_bytes(out);
                }
                self.render.ReleaseBuffer(available, 0)?;
            }
            *written += u64::from(available);
        }
        if !self.started {
            // SAFETY: COM call on a valid, initialized client.
            unsafe { self.client.Start() }?;
            self.started = true;
        }
        Ok(())
    }

    fn is_active(&self) -> bool {
        // SAFETY: COM call on a valid IMMDevice.
        matches!(unsafe { self.device.GetState() }, Ok(s) if s == DEVICE_STATE_ACTIVE)
    }

    fn stop(&mut self) {
        if self.started {
            // SAFETY: COM calls on a valid client; errors are irrelevant
            // when tearing down.
            unsafe {
                let _ = self.client.Stop();
                let _ = self.client.Reset();
            }
            self.started = false;
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Copies a mix format into a WAVEFORMATEXTENSIBLE (a plain WAVEFORMATEX
/// keeps a zero mask and subformat).
///
/// # Safety
/// `mix` must point to a valid WAVEFORMATEX whose `cbSize` extra bytes are
/// readable (GetMixFormat's result).
unsafe fn copy_mix_format(mix: *const WAVEFORMATEX) -> WAVEFORMATEXTENSIBLE {
    // SAFETY: per the contract; the structs are packed, so unaligned reads.
    unsafe {
        let base = std::ptr::read_unaligned(mix);
        let tag = u32::from(base.wFormatTag);
        let extra = usize::from(base.cbSize);
        let ext_extra =
            std::mem::size_of::<WAVEFORMATEXTENSIBLE>() - std::mem::size_of::<WAVEFORMATEX>();
        if tag == WAVE_FORMAT_EXTENSIBLE && extra >= ext_extra {
            std::ptr::read_unaligned(mix.cast::<WAVEFORMATEXTENSIBLE>())
        } else {
            WAVEFORMATEXTENSIBLE {
                Format: base,
                ..Default::default()
            }
        }
    }
}

/// miniaudio's `ma_format_from_WAVEFORMATEX`.
fn sample_format_of(wf: &WAVEFORMATEXTENSIBLE) -> Option<SampleFormat> {
    let tag = u32::from(wf.Format.wFormatTag);
    let bits = wf.Format.wBitsPerSample;
    if tag == WAVE_FORMAT_EXTENSIBLE {
        // SAFETY: every member of this union is a u16.
        let valid = unsafe { wf.Samples.wValidBitsPerSample };
        let sub = wf.SubFormat;
        if sub == KSDATAFORMAT_SUBTYPE_PCM {
            return match (valid, bits) {
                (32, _) => Some(SampleFormat::S32),
                (24, 24) => Some(SampleFormat::S24),
                (16, _) => Some(SampleFormat::S16),
                (8, _) => Some(SampleFormat::U8),
                _ => None,
            };
        }
        if sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT && valid == 32 {
            return Some(SampleFormat::F32);
        }
        return None;
    }
    if tag == WAVE_FORMAT_PCM {
        return match bits {
            32 => Some(SampleFormat::S32),
            24 => Some(SampleFormat::S24),
            16 => Some(SampleFormat::S16),
            8 => Some(SampleFormat::U8),
            _ => None,
        };
    }
    if tag == WAVE_FORMAT_IEEE_FLOAT && bits == 32 {
        return Some(SampleFormat::F32);
    }
    None
}

fn activate_endpoint(
    enumerator: &IMMDeviceEnumerator,
    explicit: Option<&[u16]>,
) -> Result<IMMDevice, DeviceError> {
    match explicit {
        Some(id) => {
            let mut z: Vec<u16> = id.to_vec();
            z.push(0);
            // SAFETY: `z` is NUL-terminated and outlives the call.
            unsafe { enumerator.GetDevice(PCWSTR(z.as_ptr())) }.map_err(|e| {
                if e.code().0 == E_NOTFOUND {
                    DeviceError::NotFound
                } else {
                    backend_error("IMMDeviceEnumerator::GetDevice", &e)
                }
            })
        }
        // SAFETY: COM call on a valid enumerator.
        None => unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }.map_err(|e| {
            if e.code().0 == E_NOTFOUND {
                DeviceError::NotFound
            } else {
                backend_error("GetDefaultAudioEndpoint", &e)
            }
        }),
    }
}

/// Opens a stream on an endpoint: in its mix format (`reopen` None), or
/// in a previous stream's format with WASAPI conversion (a reroute).
fn open_stream(
    enumerator: &IMMDeviceEnumerator,
    explicit: Option<&[u16]>,
    reopen: Option<&ClientFormat>,
    event: &Event,
) -> Result<(Stream, ClientFormat), DeviceError> {
    let device = activate_endpoint(enumerator, explicit)?;
    let id = endpoint_id(&device)?;
    let props = endpoint_properties(&device);
    // SAFETY: COM call on a valid IMMDevice.
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| backend_error("IMMDevice::Activate", &e))?;

    // Hardware offloading, as miniaudio requested it by default.
    if let Ok(client2) = client.cast::<IAudioClient2>() {
        // SAFETY: COM calls on a valid IAudioClient2; the properties struct
        // outlives the call.
        unsafe {
            if let Ok(capable) = client2.IsOffloadCapable(AudioCategory_Other)
                && capable.as_bool()
            {
                let properties = AudioClientProperties {
                    cbSize: std::mem::size_of::<AudioClientProperties>() as u32,
                    bIsOffload: true.into(),
                    eCategory: AudioCategory_Other,
                    ..Default::default()
                };
                let _ = client2.SetClientProperties(&properties);
            }
        }
    }

    // SAFETY: GetMixFormat returns a CoTaskMem WAVEFORMATEX(+extension) we
    // copy and free exactly once.
    let mix = unsafe {
        let p = client
            .GetMixFormat()
            .map_err(|e| backend_error("IAudioClient::GetMixFormat", &e))?;
        if p.is_null() {
            return Err(DeviceError::Backend {
                call: "IAudioClient::GetMixFormat",
                code: 0,
            });
        }
        let wf = copy_mix_format(p);
        CoTaskMemFree(Some(p as *const _));
        wf
    };
    let native_rate = mix.Format.nSamplesPerSec;

    let mut stream_flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
    let client_format = match reopen {
        None => {
            let sample_format = sample_format_of(&mix).ok_or_else(|| {
                DeviceError::UnsupportedFormat(format!(
                    "mix format tag {:#x}, {} bits",
                    { mix.Format.wFormatTag },
                    { mix.Format.wBitsPerSample }
                ))
            })?;
            let channels = u32::from(mix.Format.nChannels);
            if !is_supported_channel_count(channels) {
                return Err(DeviceError::UnsupportedChannels(channels));
            }
            let raw_mask = if u32::from(mix.Format.wFormatTag) == WAVE_FORMAT_EXTENSIBLE {
                mix.dwChannelMask
            } else {
                0
            };
            ClientFormat {
                wf: mix,
                sample_format,
                channels,
                rate: native_rate,
                mask: wasapi_effective_mask(channels, raw_mask),
            }
        }
        Some(previous) => {
            // miniaudio set these whenever the requested rate was non-zero,
            // which it was after the first open.
            stream_flags |=
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            *previous
        }
    };
    let wf = client_format.wf;
    let rate = client_format.rate;
    let mut period_frames = frames_from_ms(PERIOD_MILLISECONDS, rate);
    let period_us = u64::from(period_frames) * 1_000_000 / u64::from(rate.max(1));

    // IAudioClient3 low-latency shared mode, under miniaudio's conditions.
    let mut used_client3 = false;
    if (stream_flags & AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM == 0 || native_rate == rate)
        && let Ok(client3) = client.cast::<IAudioClient3>()
    {
        let (mut default, mut fundamental, mut min, mut max) = (0u32, 0u32, 0u32, 0u32);
        // SAFETY: valid interface; `wf` and the out-params outlive the call.
        let ok = unsafe {
            client3.GetSharedModeEnginePeriod(
                &wf.Format,
                &mut default,
                &mut fundamental,
                &mut min,
                &mut max,
            )
        }
        .is_ok();
        if ok && fundamental > 0 {
            let desired = period_frames;
            let actual = (desired / fundamental * fundamental).clamp(min.min(max), max);
            // miniaudio skipped IAudioClient3 when it would shorten the
            // period (its maximum is usually 10 ms, so normally skipped).
            if actual >= desired {
                let flags = stream_flags
                    & !(AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY);
                // SAFETY: as above.
                if unsafe { client3.InitializeSharedAudioStream(flags, actual, &wf.Format, None) }
                    .is_ok()
                {
                    used_client3 = true;
                    period_frames = actual;
                }
            }
        }
    }
    if !used_client3 {
        let buffer_duration = (period_us * u64::from(PERIODS) * 10) as i64;
        // SAFETY: valid client; `wf` outlives the call.
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                stream_flags,
                buffer_duration,
                0,
                &wf.Format,
                None,
            )
        }
        .map_err(|e| backend_error("IAudioClient::Initialize", &e))?;
        // SAFETY: valid, initialized client.
        let buffer = unsafe { client.GetBufferSize() }
            .map_err(|e| backend_error("IAudioClient::GetBufferSize", &e))?;
        period_frames = buffer / PERIODS;
    }
    // SAFETY: valid, initialized client.
    let render: IAudioRenderClient = unsafe { client.GetService() }
        .map_err(|e| backend_error("IAudioClient::GetService", &e))?;
    // SAFETY: the event handle stays open for the stream's life (the Event
    // is owned by the render thread, which drops streams first).
    unsafe { client.SetEventHandle(event.0) }
        .map_err(|e| backend_error("IAudioClient::SetEventHandle", &e))?;
    // SAFETY: valid, initialized client.
    let buffer_frames = unsafe { client.GetBufferSize() }
        .map_err(|e| backend_error("IAudioClient::GetBufferSize", &e))?;
    let frame_bytes = usize::from(wf.Format.nBlockAlign);
    Ok((
        Stream {
            device,
            client,
            render,
            id,
            props,
            buffer_frames,
            period_frames,
            frame_bytes,
            started: false,
        },
        client_format,
    ))
}

fn render_thread(
    explicit: Option<Vec<u16>>,
    ctx: ThreadContext,
) -> Option<Box<dyn RenderCallback>> {
    let ThreadContext {
        opened,
        start,
        shared,
    } = ctx;
    // SAFETY: changes only this thread's priority (pseudo-handle).
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) };
    let com = match ComGuard::new() {
        Ok(c) => c,
        Err(e) => {
            let _ = opened.send(Err(e));
            return None;
        }
    };
    let result = run(explicit, &opened, start, &shared);
    shared.set_waker(None);
    drop(com);
    result
}

fn run(
    explicit: Option<Vec<u16>>,
    opened: &std::sync::mpsc::SyncSender<Result<DeviceFormat, DeviceError>>,
    start: std::sync::mpsc::Receiver<super::StartCommand>,
    shared: &Arc<Shared>,
) -> Option<Box<dyn RenderCallback>> {
    let setup = (|| -> Result<_, DeviceError> {
        let render_event = Event::new()?;
        let wake = Arc::new(Event::new()?);
        let enumerator = device_enumerator()?;
        let (stream, client_format) =
            open_stream(&enumerator, explicit.as_deref(), None, &render_event)?;
        Ok((render_event, wake, enumerator, stream, client_format))
    })();
    let (render_event, wake, enumerator, stream, client_format) = match setup {
        Ok(s) => s,
        Err(e) => {
            let _ = opened.send(Err(e));
            return None;
        }
    };

    let notifications = Arc::new(Notifications {
        wake: wake.clone(),
        default_changed: AtomicBool::new(false),
        endpoint_changed: AtomicBool::new(false),
        watched: Mutex::new(stream.id.clone()),
    });
    let notifier: IMMNotificationClient = Notifier {
        state: notifications.clone(),
    }
    .into();
    // SAFETY: valid enumerator and client; unregistered below before the
    // enumerator and notifier are released.
    let registered = unsafe { enumerator.RegisterEndpointNotificationCallback(&notifier) }.is_ok();
    {
        let wake = wake.clone();
        shared.set_waker(Some(Box::new(move || wake.set())));
    }

    let format = DeviceFormat {
        rate: client_format.rate,
        channels: client_format.channels as u16,
        channel_mask: client_format.mask,
        headphones: stream.props.headphones,
        backend: BACKEND,
        period_frames: stream.period_frames,
        buffer_frames: stream.buffer_frames,
        sample_format: client_format.sample_format,
        device_name: stream.props.name.clone(),
    };
    let _ = opened.send(Ok(format));

    let callback = match start.recv() {
        Ok(cmd) => {
            let follow_default = explicit.is_none();
            let order = DeviceOrder {
                mask: client_format.mask,
                source: None,
            };
            let renderer = Renderer::new(
                cmd.callback,
                client_format.channels as usize,
                client_format.sample_format,
                order,
            );
            let ctx = Loop {
                enumerator: &enumerator,
                render_event: &render_event,
                wake: &wake,
                notifications: &notifications,
                shared,
                client_format,
                follow_default,
                channels: client_format.channels as usize,
            };
            ctx.run(stream, renderer, cmd.reply)
        }
        Err(_) => {
            drop(stream);
            None
        }
    };

    if registered {
        // SAFETY: the notifier was registered on this enumerator above.
        let _ = unsafe { enumerator.UnregisterEndpointNotificationCallback(&notifier) };
    }
    callback
}

struct Loop<'a> {
    enumerator: &'a IMMDeviceEnumerator,
    render_event: &'a Event,
    wake: &'a Arc<Event>,
    notifications: &'a Arc<Notifications>,
    shared: &'a Arc<Shared>,
    client_format: ClientFormat,
    follow_default: bool,
    channels: usize,
}

impl Loop<'_> {
    fn run(
        &self,
        stream: Stream,
        mut renderer: Renderer,
        reply: std::sync::mpsc::SyncSender<super::StartReply>,
    ) -> Option<Box<dyn RenderCallback>> {
        let mut written = 0u64;
        let mut stream = stream;
        // First pass: fill the whole buffer and start the client, as
        // miniaudio's data loop did before its first wait.
        if let Err(e) = stream.pump(&mut renderer, self.channels, &mut written, self.shared) {
            drop(stream);
            let error = backend_error("IAudioClient first fill/Start", &e);
            let _ = reply.send(Err((error, renderer.into_callback())));
            return None;
        }
        let _ = reply.send(Ok(()));
        self.shared.set_state(DeviceState::Running);

        let mut current = Some(stream);
        loop {
            if self.shared.stop_requested() {
                break;
            }
            if renderer.failed() {
                self.finish(&mut current, DeviceState::Failed);
                break;
            }
            let timeout = if current.is_some() {
                WAIT_TIMEOUT_MS
            } else if self.follow_default {
                DETACHED_RETRY_MS
            } else {
                INFINITE
            };
            let handles = [self.render_event.0, self.wake.0];
            // SAFETY: both handles are open events owned by this thread.
            let woke = unsafe { WaitForMultipleObjects(&handles, false, timeout) };
            if self.shared.stop_requested() {
                break;
            }
            let default_changed = self
                .notifications
                .default_changed
                .swap(false, Ordering::AcqRel);
            let endpoint_changed = self
                .notifications
                .endpoint_changed
                .swap(false, Ordering::AcqRel);

            if endpoint_changed && current.as_ref().is_some_and(|s| !s.is_active()) {
                if self.follow_default {
                    current = None;
                    self.shared.set_state(DeviceState::Detached);
                } else {
                    self.finish(&mut current, DeviceState::Lost);
                    break;
                }
            }
            if self.follow_default
                && (default_changed
                    || (current.is_none() && (endpoint_changed || woke == WAIT_TIMEOUT)))
            {
                // Stop and release the old client first, as miniaudio did.
                drop(current.take());
                current = self.reroute(&mut renderer, &mut written);
                continue;
            }
            if woke == WAIT_TIMEOUT && current.is_some() {
                // miniaudio left its data loop (the device stopped).
                self.finish(&mut current, DeviceState::Failed);
                break;
            }
            if woke != WAIT_OBJECT_0 && woke.0 != WAIT_OBJECT_0.0 + 1 && woke != WAIT_TIMEOUT {
                self.finish(&mut current, DeviceState::Failed);
                break;
            }
            let Some(s) = current.as_mut() else { continue };
            if let Err(e) = s.pump(&mut renderer, self.channels, &mut written, self.shared) {
                if self.follow_default {
                    // Retried on the next notification or detached tick.
                    current = None;
                    self.shared.set_state(DeviceState::Detached);
                } else {
                    let state = if e.code() == AUDCLNT_E_DEVICE_INVALIDATED {
                        DeviceState::Lost
                    } else {
                        DeviceState::Failed
                    };
                    self.finish(&mut current, state);
                    break;
                }
            }
        }
        drop(current);
        if !self.shared.stop_requested() {
            self.shared.wait_for_stop();
        }
        Some(renderer.into_callback())
    }

    fn finish(&self, current: &mut Option<Stream>, state: DeviceState) {
        *current = None;
        self.shared.set_state(state);
    }

    /// Opens the current default endpoint in the stream's format; None
    /// (detached) if there is none or it fails.
    fn reroute(&self, renderer: &mut Renderer, written: &mut u64) -> Option<Stream> {
        let opened = open_stream(
            self.enumerator,
            None,
            Some(&self.client_format),
            self.render_event,
        );
        let Ok((mut stream, _)) = opened else {
            self.shared.set_state(DeviceState::Detached);
            return None;
        };
        if stream
            .pump(renderer, self.channels, written, self.shared)
            .is_err()
        {
            self.shared.set_state(DeviceState::Detached);
            return None;
        }
        if let Ok(mut watched) = self.notifications.watched.lock() {
            watched.clone_from(&stream.id);
        }
        self.shared
            .note_reroute(stream.props.headphones, &stream.props.name);
        self.shared.set_state(DeviceState::Running);
        Some(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(tag: u32, bits: u16, valid: u16, sub: windows::core::GUID) -> WAVEFORMATEXTENSIBLE {
        let mut wf = WAVEFORMATEXTENSIBLE::default();
        wf.Format.wFormatTag = tag as u16;
        wf.Format.wBitsPerSample = bits;
        wf.Samples.wValidBitsPerSample = valid;
        wf.SubFormat = sub;
        wf
    }

    #[test]
    fn mix_format_sample_types_follow_miniaudio() {
        let e = WAVE_FORMAT_EXTENSIBLE;
        assert_eq!(
            sample_format_of(&ext(e, 32, 32, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT)),
            Some(SampleFormat::F32)
        );
        assert_eq!(
            sample_format_of(&ext(e, 16, 16, KSDATAFORMAT_SUBTYPE_PCM)),
            Some(SampleFormat::S16)
        );
        assert_eq!(
            sample_format_of(&ext(e, 24, 24, KSDATAFORMAT_SUBTYPE_PCM)),
            Some(SampleFormat::S24)
        );
        // 24-in-32 was not supported by miniaudio 0.10.42 (commented out).
        assert_eq!(
            sample_format_of(&ext(e, 32, 24, KSDATAFORMAT_SUBTYPE_PCM)),
            None
        );
        assert_eq!(
            sample_format_of(&ext(e, 32, 32, KSDATAFORMAT_SUBTYPE_PCM)),
            Some(SampleFormat::S32)
        );
        assert_eq!(
            sample_format_of(&ext(WAVE_FORMAT_IEEE_FLOAT, 32, 0, Default::default())),
            Some(SampleFormat::F32)
        );
        assert_eq!(
            sample_format_of(&ext(WAVE_FORMAT_PCM, 16, 0, Default::default())),
            Some(SampleFormat::S16)
        );
        assert_eq!(
            sample_format_of(&ext(WAVE_FORMAT_IEEE_FLOAT, 64, 0, Default::default())),
            None
        );
    }
}
