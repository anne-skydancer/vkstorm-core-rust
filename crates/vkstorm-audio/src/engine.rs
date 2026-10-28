//! The engine: the mixer behind its mutex, the output device that drives
//! it, the music bus, restarts and device selection. A thin layer for the
//! FFI, keeping the semantics of `LLAudioEngine_SoLoud`
//! (`indra/llaudio/llaudioengine_soloud.cpp`):
//!
//! - **One mutex** ([`Engine::mixer`]) serialises every mixer call and the
//!   device callback, which locks it once per chunk of at most
//!   [`MAX_CALLBACK_FRAMES`] frames, as SoLoud's audio mutex did.
//! - **Restart** ([`Engine::restart`], `reinitBackend`): the wind is
//!   stopped, the device is stopped and its render thread joined (no
//!   callback runs afterwards), the mixer is replaced (every voice handle
//!   becomes invalid; the replacement continues the play-index sequence so
//!   an old handle can never name a new voice), buffers (`Arc<Wav>`, owned
//!   by the caller) are kept, a new device is opened and started, the global
//!   volume and the wind are restored. The music bus moves to the new mixer
//!   with its source, gain and envelope; its clock restarts with the new
//!   device's.
//! - **Device selection by endpoint** ([`Engine::set_device`], `setDevice`):
//!   the current endpoint again is no change; an endpoint that is not
//!   enumerated leaves the output unchanged; otherwise the engine restarts
//!   on it, and if that fails restarts on the previous target; if that fails
//!   too the engine is unavailable. No other engine is ever substituted.
//! - **Null output** ([`Engine::new_null`]): the same engine rendering on
//!   demand ([`Engine::render_null`]) with a settable device clock, for
//!   tests and offline rendering.
//!
//! # Thread safety
//! `Engine` is `Send` and is meant to be owned by one thread (the viewer's
//! main thread); its methods take `&mut self` where they change the device.
//! The mixer it shares with the render thread is an `Arc<Mutex<Mixer>>`; a
//! poisoned lock (a panic while holding it, which the device contains) is
//! recovered rather than propagated: the mixer's state stays consistent
//! because nothing in it panics halfway through an update by design.

#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex, MutexGuard};

use crate::device::{
    self, DeviceClock, DeviceError, DeviceFormat, DeviceInfo, DeviceState, DeviceUuid,
    MAX_CALLBACK_FRAMES, NullConfig, NullDevice, RenderCallback, RunningDevice, Target,
};
use crate::mixer::{
    FormFactor, Handle, Mixer, MixerConfig, MixerError, MusicBus, OutputLayout, TransitionState,
    WindNoise, WindTargets,
};
use crate::music::MusicSource;

/// The device callback: renders the mixer under its mutex.
struct MixerCallback {
    mixer: Arc<Mutex<Mixer>>,
}

impl RenderCallback for MixerCallback {
    fn render(&mut self, out: &mut [f32], frames: usize) {
        let mut m = lock(&self.mixer);
        m.render(out, frames);
    }
}

fn lock(m: &Mutex<Mixer>) -> MutexGuard<'_, Mixer> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Where the mixer's output goes.
enum Output {
    /// No device (startup failed, or a restart failed): unavailable.
    None,
    Device(RunningDevice),
    Null(NullDevice),
}

/// Engine errors.
#[derive(Debug)]
pub enum EngineError {
    Device(DeviceError),
    Mixer(MixerError),
    /// The engine has no output device.
    Unavailable,
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Device(e) => write!(f, "{e}"),
            EngineError::Mixer(e) => write!(f, "{e}"),
            EngineError::Unavailable => f.write_str("audio output unavailable"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<DeviceError> for EngineError {
    fn from(e: DeviceError) -> Self {
        EngineError::Device(e)
    }
}

impl From<MixerError> for EngineError {
    fn from(e: MixerError) -> Self {
        EngineError::Mixer(e)
    }
}

/// The result of [`Engine::set_device`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceSelection {
    /// Already on that endpoint.
    Unchanged,
    /// The endpoint is not enumerated; the output was not touched.
    NotFound,
    /// Now playing on the requested endpoint.
    Switched,
    /// The requested endpoint failed; the previous one was restored.
    RestoredPrevious,
    /// Both failed: no output.
    Unavailable,
}

/// The wind voice the engine keeps across restarts (`initWind`).
struct Wind {
    targets: Arc<WindTargets>,
    handle: Handle,
}

/// The audio engine. See the module documentation.
pub struct Engine {
    mixer: Arc<Mutex<Mixer>>,
    output: Output,
    /// The endpoint in use (`None` = the default endpoint).
    selected: Option<DeviceUuid>,
    global_volume: f32,
    wind: Option<Wind>,
    format: Option<DeviceFormat>,
}

fn layout_of(format: &DeviceFormat) -> OutputLayout {
    OutputLayout {
        channels: u32::from(format.channels),
        mask: format.channel_mask,
        form_factor: if format.headphones {
            FormFactor::Headphones
        } else {
            FormFactor::Speakers
        },
    }
}

fn new_mixer(
    format: &DeviceFormat,
    first_play_index: u32,
    music: Option<MusicBus>,
) -> Result<Mixer, MixerError> {
    let mut config = MixerConfig::viewer(format.rate, u32::from(format.channels));
    config.first_play_index = first_play_index;
    // SoLoud sized its scratch from the device buffer; at least 4096.
    config.max_mix_frames = format.period_frames.max(4096);
    let mut m = Mixer::new(config)?;
    if let Some(mut bus) = music {
        bus.device_restarted();
        m.music = bus;
    }
    m.music.set_output(layout_of(format));
    Ok(m)
}

impl Engine {
    /// Opens the output (`None` = the default endpoint, else the endpoint
    /// with that identity) and starts mixing. Global volume starts at 0
    /// (the viewer's engine sets it, starting muted).
    pub fn new(endpoint: Option<DeviceUuid>) -> Result<Engine, EngineError> {
        let mixer = Arc::new(Mutex::new(Mixer::new(MixerConfig::viewer(48000, 2))?));
        let mut engine = Engine {
            mixer,
            output: Output::None,
            selected: endpoint,
            global_volume: 0.0,
            wind: None,
            format: None,
        };
        engine.start_output(endpoint)?;
        Ok(engine)
    }

    /// An engine on the null device (renders only through
    /// [`Engine::render_null`]).
    pub fn new_null(config: NullConfig) -> Result<Engine, EngineError> {
        let probe = Arc::new(Mutex::new(Mixer::new(MixerConfig::viewer(48000, 2))?));
        let mut engine = Engine {
            mixer: probe,
            output: Output::None,
            selected: None,
            global_volume: 0.0,
            wind: None,
            format: None,
        };
        engine.start_null(config)?;
        Ok(engine)
    }

    fn take_music(&mut self) -> Option<MusicBus> {
        let mut m = lock(&self.mixer);
        let rate = m.samplerate();
        Some(std::mem::replace(&mut m.music, MusicBus::new(rate)))
    }

    /// Opens and starts a device on `endpoint` with a fresh mixer.
    fn start_output(&mut self, endpoint: Option<DeviceUuid>) -> Result<(), EngineError> {
        let target_info: Option<DeviceInfo> = match endpoint {
            None => None,
            Some(uuid) if uuid.is_null() => None,
            Some(uuid) => {
                let list = device::enumerate()?;
                Some(
                    list.into_iter()
                        .find(|d| d.uuid == uuid)
                        .ok_or(EngineError::Device(DeviceError::NotFound))?,
                )
            }
        };
        let target = match &target_info {
            None => Target::Default,
            Some(info) => Target::Endpoint(info),
        };
        let opened = device::open(target)?;
        let format = opened.format().clone();
        let first = lock(&self.mixer).play_index();
        let music = self.take_music();
        let mixer = Arc::new(Mutex::new(new_mixer(&format, first, music)?));
        lock(&mixer).set_global_volume(self.global_volume);
        let callback = Box::new(MixerCallback {
            mixer: mixer.clone(),
        });
        match opened.start(callback) {
            Ok(running) => {
                self.mixer = mixer;
                self.output = Output::Device(running);
                self.format = Some(format);
                self.restore_wind();
                Ok(())
            }
            Err(e) => {
                // Keep the music bus for the next attempt.
                let bus = std::mem::replace(&mut lock(&mixer).music, MusicBus::new(format.rate));
                lock(&self.mixer).music = bus;
                Err(EngineError::Device(e.error))
            }
        }
    }

    fn start_null(&mut self, config: NullConfig) -> Result<(), EngineError> {
        // The null device validates the format; build it with a temporary
        // callback, then hand it the mixer's.
        let probe = NullDevice::new(config.clone(), Box::new(|_: &mut [f32], _: usize| {}))?;
        let format = probe.format().clone();
        drop(probe);
        let first = lock(&self.mixer).play_index();
        let music = self.take_music();
        let mixer = Arc::new(Mutex::new(new_mixer(&format, first, music)?));
        lock(&mixer).set_global_volume(self.global_volume);
        let device = NullDevice::new(
            config,
            Box::new(MixerCallback {
                mixer: mixer.clone(),
            }),
        )?;
        self.mixer = mixer;
        self.output = Output::Null(device);
        self.format = Some(format);
        self.restore_wind();
        Ok(())
    }

    /// Stops the output: the render thread is joined, no callback runs
    /// after this returns.
    fn stop_output(&mut self) {
        match std::mem::replace(&mut self.output, Output::None) {
            Output::Device(running) => {
                drop(running.stop());
            }
            Output::Null(null) => drop(null),
            Output::None => {}
        }
    }

    fn restore_wind(&mut self) {
        if let Some(w) = self.wind.as_mut() {
            let mut m = lock(&self.mixer);
            let h = m.play_wind(&w.targets, WindNoise::from_entropy(), 1.0);
            m.set_protect_voice(h, true);
            w.handle = h;
        }
    }

    /// `reinitBackend`: restarts on the current endpoint (or the null
    /// device with `null`). Every voice handle becomes invalid.
    pub fn restart(&mut self, null: Option<NullConfig>) -> Result<(), EngineError> {
        let wind_targets = self.wind.as_ref().map(|w| w.targets.clone());
        self.cleanup_wind();
        self.stop_output();
        // The old mixer's voices end here; buffers survive in their owners.
        lock(&self.mixer).stop_all();
        if let Some(t) = wind_targets {
            self.wind = Some(Wind {
                targets: t,
                handle: 0,
            });
        }
        let result = match null {
            Some(config) => self.start_null(config),
            None => self.start_output(self.selected),
        };
        if result.is_err() {
            self.format = None;
        }
        result
    }

    /// `setDevice`: switches to `endpoint` (`None` or the null identity =
    /// the default endpoint).
    pub fn set_device(&mut self, endpoint: Option<DeviceUuid>) -> DeviceSelection {
        let endpoint = endpoint.filter(|u| !u.is_null());
        if self.is_available() && endpoint == self.selected {
            return DeviceSelection::Unchanged;
        }
        if let Some(uuid) = endpoint {
            match device::enumerate() {
                Ok(list) if list.iter().any(|d| d.uuid == uuid) => {}
                _ => return DeviceSelection::NotFound,
            }
        }
        let previous = self.selected;
        self.selected = endpoint;
        if self.restart(None).is_ok() {
            return DeviceSelection::Switched;
        }
        self.selected = previous;
        if self.restart(None).is_ok() {
            DeviceSelection::RestoredPrevious
        } else {
            DeviceSelection::Unavailable
        }
    }

    /// The endpoint in use (`None` = default).
    pub fn selected_device(&self) -> Option<DeviceUuid> {
        self.selected
    }

    /// The output endpoints (`getDevices`).
    pub fn devices(&self) -> Result<Vec<DeviceInfo>, EngineError> {
        Ok(device::enumerate()?)
    }

    /// True while an output (device or null) is running.
    pub fn is_available(&self) -> bool {
        !matches!(self.output, Output::None)
    }

    /// The running device's format.
    pub fn format(&self) -> Option<&DeviceFormat> {
        self.format.as_ref()
    }

    /// The device's state (`None` without a device; the null device is
    /// always running).
    pub fn device_state(&self) -> Option<DeviceState> {
        match &self.output {
            Output::Device(d) => Some(d.state()),
            Output::Null(_) => Some(DeviceState::Running),
            Output::None => None,
        }
    }

    /// Polls the device: follows default-endpoint moves (headphone state
    /// for the speaker fill) and returns the state.
    pub fn poll(&mut self) -> Option<DeviceState> {
        if let Output::Device(d) = &self.output {
            let events = d.take_events();
            if events.rerouted > 0 {
                let headphones = d.headphones();
                let mut m = lock(&self.mixer);
                let mut layout = m.music.output();
                layout.form_factor = if headphones {
                    FormFactor::Headphones
                } else {
                    FormFactor::Speakers
                };
                m.music.set_output(layout);
            }
        }
        self.device_state()
    }

    /// The device clock (frames played), for transition completion.
    pub fn clock(&self) -> Option<DeviceClock> {
        match &self.output {
            Output::Device(d) => Some(d.clock()),
            Output::Null(n) => Some(n.clock()),
            Output::None => None,
        }
    }

    fn frames_played(&self) -> u64 {
        self.clock().map_or(0, |c| c.frames_played())
    }

    /// The mixer, locked: every voice, 3D, wind and music operation goes
    /// through this guard (hold it briefly: the device callback waits on it).
    pub fn mixer(&self) -> MutexGuard<'_, Mixer> {
        lock(&self.mixer)
    }

    /// `setInternalGain`: the effects' global volume (kept across restarts).
    pub fn set_global_volume(&mut self, volume: f32) {
        self.global_volume = volume;
        lock(&self.mixer).set_global_volume(volume);
    }

    pub fn global_volume(&self) -> f32 {
        self.global_volume
    }

    // ------------------------------------------------------------------
    // wind

    /// `initWind`: starts the protected wind voice (no-op when running).
    pub fn init_wind(&mut self) -> bool {
        if !self.is_available() {
            return false;
        }
        if self.wind.is_none() {
            self.wind = Some(Wind {
                targets: WindTargets::new(),
                handle: 0,
            });
            self.restore_wind();
        }
        true
    }

    /// `cleanupWind`.
    pub fn cleanup_wind(&mut self) {
        if let Some(w) = self.wind.take()
            && w.handle != 0
        {
            lock(&self.mixer).stop(w.handle);
        }
    }

    /// `LLSoLoudWindSource::setTargets` (lock-free).
    pub fn set_wind_targets(&self, freq: f32, gain: f32, pan_gain_r: f32) {
        if let Some(w) = &self.wind {
            w.targets.set(freq, gain, pan_gain_r);
        }
    }

    pub fn wind_handle(&self) -> Option<Handle> {
        self.wind.as_ref().map(|w| w.handle)
    }

    // ------------------------------------------------------------------
    // music

    /// Attaches parcel music (replacing and returning the previous source;
    /// drop the returned source on this thread).
    pub fn attach_music(&self, source: Box<dyn MusicSource>) -> Option<Box<dyn MusicSource>> {
        lock(&self.mixer).music.attach(source)
    }

    /// Detaches parcel music (cancels a pending transition).
    pub fn detach_music(&self) -> Option<Box<dyn MusicSource>> {
        lock(&self.mixer).music.detach()
    }

    /// Transition `serial`'s state against the device clock.
    pub fn music_transition_state(&self, serial: u32) -> TransitionState {
        let played = self.frames_played();
        lock(&self.mixer).music.transition_state(serial, played)
    }

    // ------------------------------------------------------------------
    // null device

    /// Renders `frames` on the null device (interleaved; empty without one).
    pub fn render_null(&mut self, frames: usize) -> Vec<f32> {
        match &mut self.output {
            Output::Null(n) => n.render(frames).to_vec(),
            _ => Vec::new(),
        }
    }

    /// Sets the null device's clock (frames played).
    pub fn set_null_clock(&self, frames_played: u64) {
        if let Output::Null(n) = &self.output {
            n.set_clock(frames_played);
        }
    }

    /// Frames per device callback chunk (the mixer's bound).
    pub const MAX_CHUNK: usize = MAX_CALLBACK_FRAMES;

    /// Stops the output and drops the music source (`shutdown`).
    pub fn shutdown(&mut self) {
        self.cleanup_wind();
        self.stop_output();
        let mut m = lock(&self.mixer);
        m.stop_all();
        drop(m.music.detach());
        self.format = None;
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop_output();
    }
}
