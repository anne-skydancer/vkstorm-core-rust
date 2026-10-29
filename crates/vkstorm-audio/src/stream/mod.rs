//! Parcel music: the in-process replacement of the VLC (and FMOD) internet
//! stream player, filling the engine's streaming slot.
//!
//! [`MusicPlayer`] is the control side the engine owns (start, stop, pause,
//! state, metadata events, device rate); [`MusicReader`] is the mixer's
//! side, a [`MusicSource`] that delivers the decoded stream at the device
//! rate in the source's channel layout.
//!
//! # Threads
//! Per stream: a session thread (connect, sniff, demux, decode, convert to
//! the device rate, push into the ring; reconnect), a network thread per
//! connection (libcurl into a byte pipe, ICY metadata stripped) and a
//! status thread (the Icecast `status-json.xsl` side channel). The mixer's
//! pull only copies out of the ring under a brief lock.
//!
//! # Buffering
//! The ring holds up to [`StreamConfig::ring_capacity`] (10 s) of decoded
//! audio at the device rate; playback starts once
//! [`StreamConfig::start_buffer`] (3 s) is buffered, and after an underrun
//! (the ring ran dry: the mixer gets silence, never stretched time) it
//! resumes at the same level. Why these numbers: VLC was given 5 s of
//! network cache because SLPlugin ran at background priority and starved
//! whenever the viewer loaded the machine; FMOD had a 7 s compressed-byte
//! buffer plus 1 s of decoded audio. Here decoding runs in the viewer
//! process at normal priority and costs well under 1% of a core, so the
//! remaining risk is the network: 3 s rides out ordinary TCP stalls and
//! Wi-Fi hiccups while keeping the start latency within the 3 s fade-in;
//! Icecast's connect burst (typically 64 KB, about 4 s at 128 kbit/s)
//! usually primes it at once. The 10 s ring absorbs bursts from servers
//! that send faster than real time and leaves room above the start level
//! for jitter; beyond it TCP flow control holds the server back.
//!
//! # States
//! `Idle` (never started) -> `start` -> `Connecting` (resolving playlists,
//! connecting, detecting the format) -> `Buffering` (decoding, below the
//! start level) -> `Playing` (the mixer gets audio); an underrun returns to
//! `Buffering`; a disconnect of a live stream reconnects (`Buffering` or
//! `Playing` while the buffered audio plays out and the ring refills). A
//! finite stream that ends goes to `Ended` once its last frame was pulled.
//! A failure goes to `Failed(reason)`; `stop` to `Stopped`. `pause(true)`
//! is `Paused` from any active state: the mixer gets nothing; a live
//! stream's connection is closed and its buffer dropped (FMOD's pause
//! stopped the stream and resumed it fresh; a live stream resumes live,
//! without stale audio or a server dropping a stalled listener), while a
//! finite stream keeps its position (as VLC's pause did) with the decoder
//! held back by the full ring. [`MusicPlayer::is_playing`] gives the
//! viewer's `isPlaying()`: 1 connecting/buffering/playing, 2 paused, 0
//! otherwise.

pub(crate) mod codec;
pub(crate) mod curl;
pub mod decode;
pub(crate) mod demux;
pub(crate) mod ffmpeg;
pub(crate) mod icy;
pub(crate) mod input;
pub(crate) mod json;
pub(crate) mod meta;
pub(crate) mod mpeg;
pub(crate) mod ogg;
pub(crate) mod pipe;
pub(crate) mod playlist;
pub mod resample;
pub(crate) mod ring;
pub(crate) mod session;
pub(crate) mod sniff;
pub mod status;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::music::{ChannelLayout, MusicSource, Pulled};
pub use curl::{CurlApi, set_curl_api};
pub use decode::{OfflineDecoded, SegmentStats, decode_bytes};
pub use status::StatusPolicy;

use codec::StreamError;
use decode::{Converter, DecodeCtx};
use meta::Meta;
use ring::Ring;

/// The player's configuration.
#[derive(Clone, Debug)]
pub struct StreamConfig {
    /// The output device's sample rate.
    pub device_rate: u32,
    /// The CA bundle for TLS verification (the viewer's `ca-bundle.crt`).
    pub ca_bundle: Option<PathBuf>,
    /// The HTTP User-Agent (must not claim to be a browser: SHOUTcast 2
    /// serves its HTML status page to "Mozilla" agents).
    pub user_agent: String,
    /// The directory of the FFmpeg libraries (HE-AAC, Opus).
    pub ffmpeg_dir: Option<PathBuf>,
    /// Decoded audio buffered before playback starts or resumes.
    pub start_buffer: Duration,
    /// The decoded-audio ring's capacity.
    pub ring_capacity: Duration,
    /// TCP/TLS connection timeout.
    pub connect_timeout: Duration,
    /// A connection that receives nothing for this long is dropped.
    pub stall_timeout: Duration,
    /// Delays before the reconnect attempts of a live stream that played.
    pub reconnect_delays: Vec<Duration>,
    /// The status-json side channel's timing.
    pub status: StatusPolicy,
}

impl Default for StreamConfig {
    fn default() -> Self {
        StreamConfig {
            device_rate: 48000,
            ca_bundle: None,
            user_agent: "Vulkanstorm".into(),
            ffmpeg_dir: None,
            start_buffer: Duration::from_millis(3000),
            ring_capacity: Duration::from_millis(10000),
            connect_timeout: Duration::from_secs(15),
            stall_timeout: Duration::from_secs(20),
            reconnect_delays: [1, 2, 4, 8].map(Duration::from_secs).to_vec(),
            status: StatusPolicy::default(),
        }
    }
}

/// The player's state (see the module documentation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlayerState {
    Idle,
    Connecting,
    Buffering,
    Playing,
    Paused,
    /// A finite stream played to its end.
    Ended,
    Failed(String),
    Stopped,
}

/// What the viewer's metadata signal carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlayerEvent {
    /// New track information: the LLSD map `{"ARTIST": artist, "TITLE":
    /// title}` (both keys, either may be empty).
    Metadata { artist: String, title: String },
    /// The previous stream's information no longer applies: an empty map.
    MetadataCleared,
}

/// Counters for diagnostics and the tests.
#[derive(Clone, Debug, Default)]
pub struct StreamStats {
    /// The codec of the current (or last) connection.
    pub codec: &'static str,
    /// Closed segments and the one in progress, oldest first.
    pub segments: Vec<SegmentStats>,
    pub buffered_frames: usize,
    pub pushed_frames: u64,
    pub pulled_frames: u64,
    pub underruns: u64,
    pub reconnects: u32,
}

/// The session phase (the state before the ring and pause are applied).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Idle,
    Stopped,
    Connecting,
    Streaming,
    Reconnecting,
    Finished,
    Failed(String),
}

struct Control {
    generation: u64,
    /// The running session's id (0: none).
    session: u64,
    next_session: u64,
    cancel: Option<Arc<AtomicBool>>,
    conv: Option<Arc<Mutex<Converter>>>,
    url: String,
    phase: Phase,
    paused: bool,
    /// Pause closed the connection: resume reconnects.
    resume: bool,
    live: Option<bool>,
    codec: &'static str,
    segments: Vec<SegmentStats>,
    reconnects: u32,
    status_stop: Option<Arc<AtomicBool>>,
    threads: Vec<JoinHandle<()>>,
}

pub(crate) struct Shared {
    cfg: Mutex<StreamConfig>,
    ctrl: Mutex<Control>,
    ring: Mutex<Ring>,
    meta: Mutex<Meta>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn ms(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
}

// Lock order: ctrl, then a session's converter, then ring; meta and cfg
// are never held while taking another lock.
impl Shared {
    pub(crate) fn meta(&self) -> MutexGuard<'_, Meta> {
        lock(&self.meta)
    }

    fn generation(&self) -> u64 {
        lock(&self.ctrl).generation
    }

    pub(crate) fn ring_capacity(&self) -> usize {
        let cfg = lock(&self.cfg);
        (u64::from(cfg.device_rate) * u64::from(ms(cfg.ring_capacity)) / 1000) as usize
    }

    /// The ring's room for session `id` (None when it is no longer the
    /// writer).
    pub(crate) fn room(&self, id: u64) -> Option<usize> {
        let r = lock(&self.ring);
        (r.writer == id).then(|| r.room())
    }

    pub(crate) fn push(&self, id: u64, layout: ChannelLayout, data: &[f32]) {
        let mut r = lock(&self.ring);
        if r.writer == id {
            r.push(layout, data);
        }
    }

    pub(crate) fn set_phase(&self, id: u64, phase: Phase) {
        let mut c = lock(&self.ctrl);
        if c.session == id {
            c.phase = phase;
        }
    }

    /// A connection's audio format was detected.
    pub(crate) fn set_streaming(&self, id: u64, live: bool) {
        let mut c = lock(&self.ctrl);
        if c.session == id {
            c.phase = Phase::Streaming;
            c.live = Some(live);
        }
    }

    pub(crate) fn set_codec(&self, id: u64, codec: &'static str) {
        let mut c = lock(&self.ctrl);
        if c.session == id {
            c.codec = codec;
        }
    }

    pub(crate) fn count_reconnect(&self, id: u64) {
        let mut c = lock(&self.ctrl);
        if c.session == id {
            c.reconnects += 1;
        }
    }

    /// The session thread finished (its last converted audio is pushed).
    pub(crate) fn session_ended(
        &self,
        id: u64,
        conv: &Mutex<Converter>,
        outcome: Result<(), StreamError>,
    ) {
        let segments = lock(conv).segments.clone();
        let mut c = lock(&self.ctrl);
        if c.session != id {
            return;
        }
        c.segments.extend(segments);
        c.conv = None;
        match outcome {
            // Stopped or paused: whoever cancelled set the state.
            Err(StreamError::Cancelled) => return,
            Ok(()) => c.phase = Phase::Finished,
            Err(StreamError::Failed(r)) => c.phase = Phase::Failed(r),
        }
        c.session = 0;
        c.cancel = None;
        drop(c);
        let mut r = lock(&self.ring);
        if r.writer == id {
            r.finish();
        }
    }

    fn state(&self) -> PlayerState {
        let c = lock(&self.ctrl);
        if c.paused {
            return PlayerState::Paused;
        }
        match &c.phase {
            Phase::Idle => PlayerState::Idle,
            Phase::Stopped => PlayerState::Stopped,
            Phase::Failed(r) => PlayerState::Failed(r.clone()),
            Phase::Connecting => PlayerState::Connecting,
            Phase::Streaming | Phase::Reconnecting => {
                if lock(&self.ring).primed {
                    PlayerState::Playing
                } else {
                    PlayerState::Buffering
                }
            }
            Phase::Finished => {
                if lock(&self.ring).frames() == 0 {
                    PlayerState::Ended
                } else {
                    PlayerState::Playing
                }
            }
        }
    }

    fn is_playing_code(&self) -> i32 {
        match self.state() {
            PlayerState::Connecting | PlayerState::Buffering | PlayerState::Playing => 1,
            PlayerState::Paused => 2,
            _ => 0,
        }
    }
}

/// Strips what FMOD let people append to stream URLs (a label after a
/// space), as the viewer's adapter did.
pub fn sanitize_url(url: &str) -> String {
    url.trim().split(' ').next().unwrap_or("").to_owned()
}

/// The control side of parcel music.
pub struct MusicPlayer {
    shared: Arc<Shared>,
}

impl MusicPlayer {
    pub fn new(cfg: StreamConfig) -> MusicPlayer {
        let ring = Ring::new(cfg.device_rate, ms(cfg.ring_capacity), ms(cfg.start_buffer));
        MusicPlayer {
            shared: Arc::new(Shared {
                cfg: Mutex::new(cfg),
                ctrl: Mutex::new(Control {
                    generation: 0,
                    session: 0,
                    next_session: 1,
                    cancel: None,
                    conv: None,
                    url: String::new(),
                    phase: Phase::Idle,
                    paused: false,
                    resume: false,
                    live: None,
                    codec: "",
                    segments: Vec::new(),
                    reconnects: 0,
                    status_stop: None,
                    threads: Vec::new(),
                }),
                ring: Mutex::new(ring),
                meta: Mutex::new(Meta::new()),
            }),
        }
    }

    /// Plays `url` (a stream or a playlist), replacing whatever played.
    /// An empty URL stops. Metadata is reset (an event tells the viewer).
    pub fn start(&self, url: &str) {
        let url = sanitize_url(url);
        if url.is_empty() {
            self.stop();
            return;
        }
        let mut c = lock(&self.shared.ctrl);
        Self::cancel_session(&mut c);
        if let Some(s) = c.status_stop.take() {
            s.store(true, Ordering::Relaxed);
        }
        c.generation += 1;
        let generation = c.generation;
        c.url = url.clone();
        c.paused = false;
        c.resume = false;
        c.live = None;
        c.codec = "";
        c.segments.clear();
        c.reconnects = 0;
        c.phase = Phase::Connecting;
        lock(&self.shared.ring).reset(generation);
        self.shared.meta().reset(generation, &url);
        self.spawn_session(&mut c);
        let stop = Arc::new(AtomicBool::new(false));
        c.status_stop = Some(stop.clone());
        let (net, policy) = {
            let cfg = lock(&self.shared.cfg);
            (Self::net_config(&cfg), cfg.status)
        };
        let shared = self.shared.clone();
        if let Ok(h) = std::thread::Builder::new()
            .name("vks-music-status".into())
            .spawn(move || status::run(shared, generation, url, net, policy, stop))
        {
            c.threads.push(h);
        }
        Self::reap(&mut c);
    }

    /// Stops playback (the mixer's reader ends at once) and resets the
    /// metadata.
    pub fn stop(&self) {
        let mut c = lock(&self.shared.ctrl);
        Self::cancel_session(&mut c);
        if let Some(s) = c.status_stop.take() {
            s.store(true, Ordering::Relaxed);
        }
        let was = c.phase != Phase::Idle;
        c.generation += 1;
        let generation = c.generation;
        c.url.clear();
        c.paused = false;
        c.resume = false;
        if was {
            c.phase = Phase::Stopped;
        }
        let mut r = lock(&self.shared.ring);
        r.reset(generation);
        r.finish();
        drop(r);
        self.shared.meta().reset(generation, "");
        Self::reap(&mut c);
    }

    /// Pauses (true) or resumes (false) the current stream.
    pub fn pause(&self, pause: bool) {
        let mut c = lock(&self.shared.ctrl);
        // Active: the viewer's isPlaying() == 1 (a finished stream whose
        // buffered audio is still playing counts).
        let active = match c.phase {
            Phase::Connecting | Phase::Streaming | Phase::Reconnecting => true,
            Phase::Finished => lock(&self.shared.ring).frames() > 0,
            _ => false,
        };
        if pause {
            if c.paused || !active {
                return;
            }
            c.paused = true;
            lock(&self.shared.ring).paused = true;
            if c.live != Some(false) && c.session != 0 {
                // A live stream: close the connection, drop the buffer.
                Self::cancel_session(&mut c);
                c.resume = true;
                c.phase = Phase::Connecting;
                lock(&self.shared.ring).clear();
            }
        } else {
            if !c.paused {
                return;
            }
            c.paused = false;
            lock(&self.shared.ring).paused = false;
            if c.resume {
                c.resume = false;
                c.phase = Phase::Connecting;
                self.spawn_session(&mut c);
            }
        }
    }

    /// The current state.
    pub fn state(&self) -> PlayerState {
        self.shared.state()
    }

    /// The viewer's `isPlaying()`: 0 stopped, 1 active, 2 paused.
    pub fn is_playing(&self) -> i32 {
        self.shared.is_playing_code()
    }

    /// The URL being played (sanitised), empty when stopped.
    pub fn url(&self) -> String {
        lock(&self.shared.ctrl).url.clone()
    }

    /// The next metadata event, if any.
    pub fn poll_event(&self) -> Option<PlayerEvent> {
        self.shared.meta().pop()
    }

    /// The current metadata (`getCurrentMetadata`): None for the empty map.
    pub fn current_metadata(&self) -> Option<(String, String)> {
        self.shared.meta().current()
    }

    /// A reader of the current stream for the mixer. It ends when this
    /// stream ends, fails or is stopped or replaced; take a new one after
    /// each [`MusicPlayer::start`].
    pub fn reader(&self) -> MusicReader {
        MusicReader {
            shared: self.shared.clone(),
            generation: self.shared.generation(),
        }
    }

    /// The output device's rate changed: the buffered audio is converted
    /// and decoding continues at the new rate.
    pub fn set_device_rate(&self, rate: u32) {
        if rate == 0 {
            return;
        }
        let (cap, start) = {
            let mut cfg = lock(&self.shared.cfg);
            cfg.device_rate = rate;
            (ms(cfg.ring_capacity), ms(cfg.start_buffer))
        };
        let c = lock(&self.shared.ctrl);
        match &c.conv {
            Some(conv) => {
                let mut conv = lock(conv);
                lock(&self.shared.ring).change_rate(rate, cap, start);
                conv.set_out_rate(rate);
            }
            None => lock(&self.shared.ring).change_rate(rate, cap, start),
        }
    }

    /// Counters for diagnostics and tests.
    pub fn stats(&self) -> StreamStats {
        let c = lock(&self.shared.ctrl);
        let mut segments = c.segments.clone();
        if let Some(conv) = &c.conv {
            let conv = lock(conv);
            segments.extend(conv.segments.iter().copied());
            segments.extend(conv.current());
        }
        let r = lock(&self.shared.ring);
        StreamStats {
            codec: c.codec,
            segments,
            buffered_frames: r.frames(),
            pushed_frames: r.pushed_frames,
            pulled_frames: r.pulled_frames,
            underruns: r.underruns,
            reconnects: c.reconnects,
        }
    }

    fn net_config(cfg: &StreamConfig) -> curl::NetConfig {
        curl::NetConfig {
            user_agent: cfg.user_agent.clone(),
            ca_bundle: cfg.ca_bundle.clone(),
            connect_timeout: cfg.connect_timeout,
            stall_timeout: cfg.stall_timeout,
        }
    }

    fn cancel_session(c: &mut Control) {
        if let Some(cancel) = c.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        c.session = 0;
        c.conv = None;
    }

    /// Starts a session for the current URL and generation.
    fn spawn_session(&self, c: &mut Control) {
        let id = c.next_session;
        c.next_session += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let cfg = lock(&self.shared.cfg).clone();
        let conv = Arc::new(Mutex::new(Converter::new(cfg.device_rate)));
        c.session = id;
        c.cancel = Some(cancel.clone());
        c.conv = Some(conv.clone());
        {
            let mut r = lock(&self.shared.ring);
            r.writer = id;
            r.primed = false;
            r.finished = false;
        }
        let ctx = session::SessionCtx {
            shared: self.shared.clone(),
            generation: c.generation,
            id,
            cancel,
            url: c.url.clone(),
            net: Self::net_config(&cfg),
            decode: DecodeCtx {
                ffmpeg_dir: cfg.ffmpeg_dir.clone(),
            },
            reconnect_delays: cfg.reconnect_delays.clone(),
            conv,
        };
        match std::thread::Builder::new()
            .name("vks-music".into())
            .spawn(move || session::run(ctx))
        {
            Ok(h) => c.threads.push(h),
            Err(e) => c.phase = Phase::Failed(format!("cannot start the stream thread: {e}")),
        }
    }

    /// Joins finished threads.
    fn reap(c: &mut Control) {
        let (done, running): (Vec<_>, Vec<_>) = c.threads.drain(..).partition(|h| h.is_finished());
        c.threads = running;
        for h in done {
            let _ = h.join();
        }
    }
}

impl Drop for MusicPlayer {
    fn drop(&mut self) {
        self.stop();
        let threads = std::mem::take(&mut lock(&self.shared.ctrl).threads);
        for h in threads {
            let _ = h.join();
        }
    }
}

/// The mixer's side: decoded frames at the device rate.
pub struct MusicReader {
    shared: Arc<Shared>,
    generation: u64,
}

impl MusicSource for MusicReader {
    fn pull(&mut self, out: &mut [f32], max_frames: usize) -> Pulled {
        let mut r = lock(&self.shared.ring);
        if r.generation != self.generation {
            return Pulled {
                frames: 0,
                layout: ChannelLayout::STEREO,
                ended: true,
            };
        }
        r.pull(out, max_frames)
    }
}
