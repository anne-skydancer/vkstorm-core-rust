//! A buffered pull reader over a connection's [`Pipe`] (or over memory, for
//! offline decoding): the demuxers ask for "at least n bytes" and consume
//! what they parsed.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::pipe::Pipe;

/// The session was stopped (or the connection cancelled) while waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cancelled;

/// The largest amount a demuxer may ask to have buffered at once (a FLAC
/// frame or an Ogg page is far smaller; this bounds memory on garbage).
pub(crate) const MAX_WINDOW: usize = 4 << 20;

enum Source {
    Pipe { pipe: Arc<Pipe>, abort: Arc<AtomicBool> },
    Memory,
}

pub(crate) struct Input {
    src: Source,
    buf: Vec<u8>,
    start: usize,
    eof: bool,
}

impl Input {
    pub(crate) fn from_pipe(pipe: Arc<Pipe>, abort: Arc<AtomicBool>) -> Input {
        Input { src: Source::Pipe { pipe, abort }, buf: Vec::with_capacity(1 << 16), start: 0, eof: false }
    }

    pub(crate) fn from_memory(data: &[u8]) -> Input {
        Input { src: Source::Memory, buf: data.to_vec(), start: 0, eof: true }
    }

    /// The buffered bytes.
    pub(crate) fn data(&self) -> &[u8] {
        &self.buf[self.start..]
    }

    pub(crate) fn len(&self) -> usize {
        self.buf.len() - self.start
    }

    /// The input ended (nothing more will be buffered).
    pub(crate) fn at_eof(&self) -> bool {
        self.eof
    }

    /// Ensures at least `n` bytes are buffered; `Ok(false)` when the input
    /// ends first (whatever arrived stays buffered).
    pub(crate) fn fill(&mut self, n: usize) -> Result<bool, Cancelled> {
        let n = n.min(MAX_WINDOW);
        while self.len() < n {
            if !self.more()? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Reads one more chunk; `Ok(false)` at the end.
    pub(crate) fn more(&mut self) -> Result<bool, Cancelled> {
        if self.eof {
            return Ok(false);
        }
        let Source::Pipe { pipe, abort } = &self.src else {
            self.eof = true;
            return Ok(false);
        };
        if self.start > 0 && self.start >= self.buf.len() / 2 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        let old = self.buf.len();
        self.buf.resize(old + 16384, 0);
        let abort = abort.clone();
        let r = pipe.read(&mut self.buf[old..], &move || abort.load(Ordering::Relaxed));
        match r {
            Ok(0) => {
                self.buf.truncate(old);
                self.eof = true;
                Ok(false)
            }
            Ok(k) => {
                self.buf.truncate(old + k);
                Ok(true)
            }
            Err(()) => {
                self.buf.truncate(old);
                Err(Cancelled)
            }
        }
    }

    pub(crate) fn consume(&mut self, n: usize) {
        let n = n.min(self.len());
        self.start += n;
        if self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        }
    }

    /// Consumes `n` bytes, reading as needed (for skipping large blocks
    /// without buffering them whole). `Ok(false)` when the input ended.
    pub(crate) fn skip(&mut self, mut n: u64) -> Result<bool, Cancelled> {
        loop {
            let k = (self.len() as u64).min(n) as usize;
            self.consume(k);
            n -= k as u64;
            if n == 0 {
                return Ok(true);
            }
            if !self.more()? {
                return Ok(false);
            }
        }
    }

    /// Reads everything up to `limit` bytes (playlists, status documents).
    pub(crate) fn read_all(&mut self, limit: usize) -> Result<Vec<u8>, Cancelled> {
        while self.len() < limit && self.more()? {}
        let n = self.len().min(limit);
        let v = self.data()[..n].to_vec();
        self.consume(n);
        Ok(v)
    }
}
