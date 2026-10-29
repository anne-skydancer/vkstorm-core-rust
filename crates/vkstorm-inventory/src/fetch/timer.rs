//! `LLFrameTimer` and the fetch state every `LLViewerInventoryCategory`
//! carries (`mFetching`, `mDescendentsRequested`): the 10 s of `fetch()`, the
//! 180 s / 30 s marks of `setFetching`, the 60 s back-off of `FETCH_FAILED`
//! and the 360 s of a `-1` version (`llviewerinventory.cpp:745-837`).
//!
//! The state stays on the C++ category in this rung (the model containers
//! stay C++); these are its rules, for a C++ category to delegate to and for
//! the crate's replay model. Times are `LLFrameTimer`'s frame time in seconds.

use super::CategoryFetch;

/// `LLFrameTimer` over an explicit frame time (`sFrameTime`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameTimer {
    pub start: f64,
    pub expiry: f64,
    pub started: bool,
}

impl FrameTimer {
    /// The constructor: started at `now`, expiry 0 (so already expired).
    pub fn new(now: f64) -> Self {
        FrameTimer {
            start: now,
            expiry: 0.0,
            started: true,
        }
    }

    /// `reset()`: start and expiry at `now`.
    pub fn reset(&mut self, now: f64) {
        self.start = now;
        self.expiry = now;
    }

    /// `start()`
    pub fn start(&mut self, now: f64) {
        self.reset(now);
        self.started = true;
    }

    /// `stop()`: only clears the started flag; the expiry stays (R3b
    /// finding 4).
    pub fn stop(&mut self) {
        self.started = false;
    }

    /// `setTimerExpirySec(F32)`: relative to the start.
    pub fn set_timer_expiry_sec(&mut self, seconds: f32) {
        self.expiry = seconds as f64 + self.start;
    }

    /// `hasExpired()`
    pub fn has_expired(&self, now: f64) -> bool {
        now >= self.expiry
    }
}

/// `AISAPI::HTTP_TIMEOUT`: the `FETCH_RECURSIVE`/`FETCH_NORMAL` mark with AIS.
pub const AIS_HTTP_TIMEOUT: f32 = 180.0;
/// `setFetching`'s mark without AIS.
pub const FETCH_TIMER_EXPIRY: f32 = 30.0;
/// `FETCH_FAILURE_EXPIRY`
pub const FETCH_FAILURE_EXPIRY: f32 = 60.0;
/// `fetch()`'s default expiry.
pub const FETCH_DEFAULT_EXPIRY: i32 = 10;
/// `AISUpdate::doUpdate`'s `LONG_EXPIRY` for a `-1` version from AIS.
pub const FETCH_LONG_EXPIRY: i32 = 360;

/// A category's fetch state: `mFetching` and `mDescendentsRequested`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CategoryFetchState {
    pub fetching: CategoryFetch,
    pub requested: FrameTimer,
}

impl CategoryFetchState {
    /// A new category: `FETCH_NONE`, the timer reset at `now` (expired).
    pub fn new(now: f64) -> Self {
        let mut requested = FrameTimer::new(now);
        requested.reset(now);
        CategoryFetchState {
            fetching: CategoryFetch::None,
            requested,
        }
    }

    /// `getFetching()`: an expired mark reads (and becomes) `FETCH_NONE`.
    pub fn get_fetching(&mut self, now: f64) -> CategoryFetch {
        if self.requested.has_expired(now) {
            self.fetching = CategoryFetch::None;
        }
        self.fetching
    }

    /// `setFetching()`: `ais_available` is `AISAPI::isAvailable()` at the call.
    pub fn set_fetching(&mut self, fetching: CategoryFetch, now: f64, ais_available: bool) {
        if fetching == CategoryFetch::Failed {
            self.requested.set_timer_expiry_sec(FETCH_FAILURE_EXPIRY);
            self.fetching = fetching;
        } else if fetching > self.fetching {
            // allow a switch from normal to recursive
            if self.requested.has_expired(now) || self.fetching == CategoryFetch::None {
                self.requested.reset(now);
                if ais_available {
                    self.requested.set_timer_expiry_sec(AIS_HTTP_TIMEOUT);
                } else {
                    self.requested.set_timer_expiry_sec(FETCH_TIMER_EXPIRY);
                }
            }
            self.fetching = fetching;
        } else if fetching == CategoryFetch::None {
            self.requested.stop();
            self.fetching = fetching;
        }
    }

    /// The gate of `fetch(expiry_seconds)`: with an unknown version and an
    /// expired mark, re-arm the mark and return true (the caller then starts
    /// the fetch, see [`super::scheduler::fetch_path`]); otherwise false.
    pub fn fetch_gate(&mut self, version: i32, expiry_seconds: i32, now: f64) -> bool {
        if version == super::VERSION_UNKNOWN && self.requested.has_expired(now) {
            self.requested.reset(now);
            self.requested.set_timer_expiry_sec(expiry_seconds as f32);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_category_mark_is_expired() {
        let mut s = CategoryFetchState::new(1000.0);
        assert!(s.requested.has_expired(1000.0));
        assert_eq!(s.get_fetching(1000.0), CategoryFetch::None);
    }

    #[test]
    fn recursive_mark_holds_180_s_and_none_keeps_expiry() {
        let mut s = CategoryFetchState::new(1000.0);
        s.set_fetching(CategoryFetch::Recursive, 1000.0, true);
        assert_eq!(s.get_fetching(1179.0), CategoryFetch::Recursive);
        // FETCH_NONE stops the timer without clearing the expiry (finding 4).
        s.set_fetching(CategoryFetch::None, 1001.0, true);
        assert!(!s.fetch_gate(-1, 10, 1100.0));
        assert!(s.fetch_gate(-1, 10, 1180.0));
        assert_eq!(s.requested.expiry, 1190.0);
    }

    #[test]
    fn failed_backs_off_60_s_from_the_last_start() {
        let mut s = CategoryFetchState::new(1000.0);
        s.set_fetching(CategoryFetch::Recursive, 1000.0, true);
        s.set_fetching(CategoryFetch::Failed, 1005.0, true);
        assert_eq!(s.get_fetching(1059.0), CategoryFetch::Failed);
        assert_eq!(s.get_fetching(1060.0), CategoryFetch::None);
    }

    #[test]
    fn without_ais_the_mark_is_30_s() {
        let mut s = CategoryFetchState::new(1000.0);
        s.set_fetching(CategoryFetch::Normal, 1000.0, false);
        assert_eq!(s.requested.expiry, 1030.0);
        // normal -> recursive within the mark keeps the timer
        s.set_fetching(CategoryFetch::Recursive, 1010.0, false);
        assert_eq!(s.requested.expiry, 1030.0);
        assert_eq!(s.fetching, CategoryFetch::Recursive);
    }
}
