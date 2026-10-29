//! A stream session: the thread that connects, resolves playlists, detects
//! the format, decodes into the ring and reconnects.
//!
//! Per connection a network thread runs libcurl into a [`Pipe`]; the
//! session thread reads it, decodes, converts to the device rate and
//! pushes into the ring, waiting for room first (so a fast server is held
//! back by TCP once the ring is full).
//!
//! Reconnect policy: VLC (the fork's music path) did not reconnect (its
//! HTTP access has `http-reconnect` off by default; a dropped stream went
//! silent); FMOD restarted a stream that had been playing once, at the next
//! update, without delay or limit on the first failure. Here a live stream
//! (no Content-Length, not chunked) that played and then disconnects is
//! retried after 1, 2, 4 and 8 s (configurable); buffered audio keeps
//! playing meanwhile, a connection that plays again resets the sequence,
//! and when every attempt fails the stream fails. A failure before any
//! audio is a failure at once (no retry). A finite body (known length or
//! chunked) that ends is the end of the stream.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::codec::{InBand, PcmSink, StreamError};
use super::curl::{self, NetConfig};
use super::decode::{Converter, DecodeCtx, run_format};
use super::input::Input;
use super::pipe::{Pipe, PipeEnd};
use super::playlist::{self, Playlist};
use super::sniff::{Detected, detect};
use super::{Phase, Shared};
use crate::music::ChannelLayout;

/// Bytes buffered between the network and the decoder.
const PIPE_BYTES: usize = 256 * 1024;
/// Largest playlist body read.
const PLAYLIST_BYTES: usize = 256 * 1024;
/// Playlists inside playlists.
const MAX_DEPTH: u32 = 4;
/// How long the decoder sleeps while the ring has no room.
const ROOM_WAIT: Duration = Duration::from_millis(20);

pub(crate) struct SessionCtx {
    pub shared: Arc<Shared>,
    pub generation: u64,
    pub id: u64,
    pub cancel: Arc<AtomicBool>,
    pub url: String,
    pub net: NetConfig,
    pub decode: DecodeCtx,
    pub reconnect_delays: Vec<Duration>,
    pub conv: Arc<Mutex<Converter>>,
}

impl SessionCtx {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Sleeps `d`, returning false when cancelled meanwhile.
    fn sleep(&self, d: Duration) -> bool {
        let step = Duration::from_millis(50);
        let mut left = d;
        while !left.is_zero() {
            if self.cancelled() {
                return false;
            }
            let s = left.min(step);
            std::thread::sleep(s);
            left -= s;
        }
        !self.cancelled()
    }
}

/// The session's view of the ring: waits for room, converts, pushes.
struct Sink<'a> {
    ctx: &'a SessionCtx,
    produced: u64,
}

impl Sink<'_> {
    fn end_segment(&mut self) {
        let mut conv = self.ctx.conv.lock().unwrap_or_else(|e| e.into_inner());
        let shared = &self.ctx.shared;
        let id = self.ctx.id;
        conv.end_segment(&mut |layout, data| shared.push(id, layout, data));
    }
}

impl PcmSink for Sink<'_> {
    fn pcm(
        &mut self,
        planes: &[&[f32]],
        frames: usize,
        rate: u32,
        layout: ChannelLayout,
    ) -> Result<(), StreamError> {
        // Wait until the ring can take this packet (with margin for the
        // resampler's tail and a rate up to 8x).
        let need = frames.saturating_mul(8) + 1024;
        loop {
            if self.ctx.cancelled() {
                return Err(StreamError::Cancelled);
            }
            match self.ctx.shared.room(self.ctx.id) {
                None => return Err(StreamError::Cancelled),
                Some(room) if room >= need.min(self.ctx.shared.ring_capacity() / 2) => break,
                Some(_) => std::thread::sleep(ROOM_WAIT),
            }
        }
        let mut conv = self.ctx.conv.lock().unwrap_or_else(|e| e.into_inner());
        let shared = &self.ctx.shared;
        let id = self.ctx.id;
        conv.feed(planes, frames, rate, layout, &mut |layout, data| {
            shared.push(id, layout, data)
        });
        self.produced += frames as u64;
        Ok(())
    }

    fn metadata(&mut self, m: InBand) {
        self.ctx.shared.meta().in_band(self.ctx.generation, m);
    }

    fn codec(&mut self, name: &'static str) {
        self.ctx.shared.set_codec(self.ctx.id, name);
    }
}

/// One connection that played (or at least decoded) to its end.
struct Played {
    url: String,
    end: PipeEnd,
    live: bool,
    produced: bool,
}

fn fail<T>(s: impl Into<String>) -> Result<T, StreamError> {
    Err(StreamError::Failed(s.into()))
}

/// Connects to `url` and plays it (resolving playlists) until its body
/// ends. `Err(Failed)` when it produced no audio.
fn play(
    ctx: &SessionCtx,
    sink: &mut Sink<'_>,
    url: &str,
    depth: u32,
) -> Result<Played, StreamError> {
    if depth > MAX_DEPTH {
        return fail("playlists nested too deeply");
    }
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return fail(format!("unsupported URL (only http and https): {url}"));
    }
    let pipe = Arc::new(Pipe::new(PIPE_BYTES));
    let net = {
        let pipe = pipe.clone();
        let cancel = ctx.cancel.clone();
        let shared = ctx.shared.clone();
        let generation = ctx.generation;
        let cfg = ctx.net.clone();
        let url = url.to_owned();
        std::thread::Builder::new()
            .name("vks-music-net".into())
            .spawn(move || {
                let abort = || cancel.load(Ordering::Relaxed);
                curl::stream(&url, &cfg, &pipe, &abort, &mut |t| {
                    shared.meta().in_band(generation, InBand::NowPlaying(t))
                });
            })
            .map_err(|e| StreamError::Failed(format!("cannot start the network thread: {e}")))?
    };
    let finish = |pipe: &Pipe, net: std::thread::JoinHandle<()>| {
        pipe.cancel();
        let _ = net.join();
    };
    let head = match pipe.wait_head() {
        Ok(h) => h,
        Err(end) => {
            finish(&pipe, net);
            return match end {
                PipeEnd::Cancelled => Err(StreamError::Cancelled),
                PipeEnd::Error(e) => fail(format!("cannot connect: {e}")),
                _ => fail("the server sent no response"),
            };
        }
    };
    if !(200..300).contains(&head.status) {
        finish(&pipe, net);
        let line = head.status_line.trim();
        return fail(if line.is_empty() {
            format!("HTTP {}", head.status)
        } else {
            format!("the server answered \"{line}\"")
        });
    }
    let mut input = Input::from_pipe(pipe.clone(), ctx.cancel.clone());
    let detected = match detect(&mut input, &head, url, sink) {
        Ok(d) => d,
        Err(e) => {
            finish(&pipe, net);
            return Err(e);
        }
    };
    match detected {
        Detected::Refused(reason) => {
            finish(&pipe, net);
            fail(reason)
        }
        Detected::Playlist { pls } => {
            let body = input.read_all(PLAYLIST_BYTES);
            finish(&pipe, net);
            let body = body?;
            let base = if head.effective_url.is_empty() {
                url
            } else {
                head.effective_url.as_str()
            };
            let entries = match playlist::parse(&body, pls, base) {
                Playlist::Hls => return fail("HLS streams are not supported"),
                Playlist::Entries(e) => e,
            };
            if entries.is_empty() {
                return fail("the playlist has no entries");
            }
            let mut last = String::new();
            for e in entries {
                match play(ctx, sink, &e, depth + 1) {
                    Ok(p) => return Ok(p),
                    Err(StreamError::Cancelled) => return Err(StreamError::Cancelled),
                    Err(StreamError::Failed(r)) => last = r,
                }
            }
            fail(format!("no playlist entry could be played (last: {last})"))
        }
        Detected::Audio(format) => {
            ctx.shared.set_streaming(ctx.id, head.is_live());
            let before = sink.produced;
            let r = run_format(format, &mut input, &ctx.decode, sink);
            let produced = sink.produced > before;
            let end = pipe.end();
            finish(&pipe, net);
            match r {
                Err(StreamError::Cancelled) => Err(StreamError::Cancelled),
                Err(StreamError::Failed(reason)) if !produced => fail(reason),
                Err(StreamError::Failed(reason)) => Ok(Played {
                    url: url.to_owned(),
                    end: PipeEnd::Error(reason),
                    live: head.is_live(),
                    produced,
                }),
                Ok(()) if !produced => fail(match end {
                    Some(PipeEnd::Error(e)) => {
                        format!("the connection failed before any audio: {e}")
                    }
                    _ => "the stream ended before any audio".to_owned(),
                }),
                Ok(()) => Ok(Played {
                    url: url.to_owned(),
                    end: end.unwrap_or(PipeEnd::Eof),
                    live: head.is_live(),
                    produced,
                }),
            }
        }
    }
}

/// The session thread's body.
pub(crate) fn run(ctx: SessionCtx) {
    let mut sink = Sink {
        ctx: &ctx,
        produced: 0,
    };
    let outcome = run_inner(&ctx, &mut sink);
    sink.end_segment();
    ctx.shared.session_ended(ctx.id, &ctx.conv, outcome);
}

/// Ok(()) when the stream ended, Err(Cancelled) when stopped, Err(Failed)
/// when it failed.
fn run_inner(ctx: &SessionCtx, sink: &mut Sink<'_>) -> Result<(), StreamError> {
    let mut played = play(ctx, sink, &ctx.url, 0)?;
    loop {
        // The audio up to the disconnect plays out continuously; the next
        // connection starts a new segment.
        sink.end_segment();
        let reason = match &played.end {
            PipeEnd::Complete => return Ok(()),
            PipeEnd::Cancelled => return Err(StreamError::Cancelled),
            PipeEnd::Eof if !played.live => return Ok(()),
            PipeEnd::Eof => "the server closed the stream".to_owned(),
            PipeEnd::Error(e) if !played.live => {
                return fail(format!("the connection failed: {e}"));
            }
            PipeEnd::Error(e) => e.clone(),
        };
        if !played.produced {
            return fail(reason);
        }
        ctx.shared.set_phase(ctx.id, Phase::Reconnecting);
        let mut last = reason;
        let mut again = None;
        for delay in &ctx.reconnect_delays {
            if !ctx.sleep(*delay) {
                return Err(StreamError::Cancelled);
            }
            ctx.shared.count_reconnect(ctx.id);
            match play(ctx, sink, &played.url, 0) {
                Ok(p) if p.produced => {
                    again = Some(p);
                    break;
                }
                Ok(p) => last = format!("{:?}", p.end),
                Err(StreamError::Cancelled) => return Err(StreamError::Cancelled),
                Err(StreamError::Failed(r)) => last = r,
            }
        }
        match again {
            Some(p) => played = p,
            None => {
                return fail(format!(
                    "the stream was lost and could not be reconnected: {last}"
                ));
            }
        }
    }
}
