//! The byte pipe between a connection's network thread (libcurl writes the
//! response body into it, ICY metadata already stripped) and the session's
//! decode thread (which reads it through [`crate::stream::input::Input`]).
//!
//! Bounded: the network thread blocks when the pipe is full, so a server
//! sending faster than real time is held back by TCP flow control once the
//! decoded ring is full too. Also carries the response head, delivered when
//! the first body bytes (or the end of the transfer) arrive.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// The response head of a stream connection.
#[derive(Clone, Debug, Default)]
pub(crate) struct Head {
    /// HTTP status (200 for `ICY 200 OK`).
    pub status: u32,
    /// The status line as received (`HTTP/1.0 200 OK`, `ICY 200 OK`).
    pub status_line: String,
    pub content_type: String,
    pub content_length: Option<u64>,
    pub chunked: bool,
    pub icy_metaint: Option<usize>,
    /// The URL after redirects.
    pub effective_url: String,
}

impl Head {
    /// A live stream: no length and not chunked, so the end of the body is a
    /// disconnect, not the end of the media.
    pub(crate) fn is_live(&self) -> bool {
        self.content_length.is_none() && !self.chunked
    }
}

/// How the body ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PipeEnd {
    /// All of a known-length (or chunked) body arrived.
    Complete,
    /// The server closed a body of unknown length.
    Eof,
    /// The transfer failed.
    Error(String),
    /// The session cancelled the transfer.
    Cancelled,
}

struct State {
    data: VecDeque<u8>,
    head: Option<Head>,
    end: Option<PipeEnd>,
    cancelled: bool,
}

pub(crate) struct Pipe {
    st: Mutex<State>,
    cv: Condvar,
    capacity: usize,
}

/// The wait slice for blocked readers and writers (they re-check the
/// cancel state at this interval even without a notification).
const WAIT: Duration = Duration::from_millis(100);

impl Pipe {
    pub(crate) fn new(capacity: usize) -> Pipe {
        Pipe {
            st: Mutex::new(State {
                data: VecDeque::with_capacity(capacity.min(1 << 16)),
                head: None,
                end: None,
                cancelled: false,
            }),
            cv: Condvar::new(),
            capacity,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn set_head(&self, head: Head) {
        self.lock().head = Some(head);
        self.cv.notify_all();
    }

    /// Waits for the response head; `Err` with the end reason when the
    /// transfer ended without one.
    pub(crate) fn wait_head(&self) -> Result<Head, PipeEnd> {
        let mut st = self.lock();
        loop {
            if st.cancelled {
                return Err(PipeEnd::Cancelled);
            }
            if let Some(h) = &st.head {
                return Ok(h.clone());
            }
            if let Some(e) = &st.end {
                return Err(e.clone());
            }
            st = self.cv.wait_timeout(st, WAIT).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// Writes body bytes, blocking while the pipe is full; false when the
    /// reader cancelled (the transfer should abort).
    pub(crate) fn write(&self, mut bytes: &[u8]) -> bool {
        let mut st = self.lock();
        while !bytes.is_empty() {
            if st.cancelled {
                return false;
            }
            let room = self.capacity.saturating_sub(st.data.len());
            if room == 0 {
                st = self.cv.wait_timeout(st, WAIT).unwrap_or_else(|e| e.into_inner()).0;
                continue;
            }
            let n = room.min(bytes.len());
            st.data.extend(&bytes[..n]);
            bytes = &bytes[n..];
            self.cv.notify_all();
        }
        true
    }

    /// Ends the body (no effect after a cancel or an earlier end).
    pub(crate) fn close(&self, end: PipeEnd) {
        let mut st = self.lock();
        if st.end.is_none() {
            st.end = Some(end);
        }
        drop(st);
        self.cv.notify_all();
    }

    /// The reader gives up: the writer aborts at its next write or
    /// progress callback.
    pub(crate) fn cancel(&self) {
        self.lock().cancelled = true;
        self.cv.notify_all();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.lock().cancelled
    }

    /// Reads up to `out.len()` bytes, blocking until some arrive. `Ok(0)`
    /// at the end of the body; `Err` when cancelled. `abort` is polled
    /// while waiting (the session's stop flag).
    pub(crate) fn read(&self, out: &mut [u8], abort: &dyn Fn() -> bool) -> Result<usize, ()> {
        let mut st = self.lock();
        loop {
            if st.cancelled || abort() {
                return Err(());
            }
            if !st.data.is_empty() {
                let n = out.len().min(st.data.len());
                let (a, b) = st.data.as_slices();
                let na = n.min(a.len());
                out[..na].copy_from_slice(&a[..na]);
                if n > na {
                    out[na..n].copy_from_slice(&b[..n - na]);
                }
                st.data.drain(..n);
                drop(st);
                self.cv.notify_all();
                return Ok(n);
            }
            if st.end.is_some() {
                return Ok(0);
            }
            st = self.cv.wait_timeout(st, WAIT).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// How the body ended (None while it is still coming).
    pub(crate) fn end(&self) -> Option<PipeEnd> {
        self.lock().end.clone()
    }
}
