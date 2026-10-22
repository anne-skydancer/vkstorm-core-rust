//! SoLoud's `Fader` (`soloud_fader.cpp`): a linear ramp in stream time,
//! used for the per-voice volume fade and the scheduled stop.
//!
//! The arithmetic is SoLoud's: the value is computed in double from the
//! float endpoints and cast to float. `set` copies the *previous* `from`
//! into `current` (a SoLoud quirk that only matters for the "time rolled
//! over" branch, which a monotonic stream time never takes); kept as is.

#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Fader {
    from: f32,
    to: f32,
    delta: f32,
    time: f64,
    start_time: f64,
    end_time: f64,
    current: f32,
    /// 0 disabled, 1 active, -1 finished (SoLoud's 2 = LFO is not ported:
    /// the viewer never oscillates).
    pub(crate) active: i32,
}

impl Fader {
    pub(crate) fn set(&mut self, from: f32, to: f32, time: f64, start_time: f64) {
        self.current = self.from;
        self.from = from;
        self.to = to;
        self.time = time;
        self.start_time = start_time;
        self.delta = to - from;
        self.end_time = self.start_time + self.time;
        self.active = 1;
    }

    pub(crate) fn get(&mut self, current_time: f64) -> f32 {
        if self.start_time > current_time {
            // Time rolled over: restart from where we were.
            let p = (self.current - self.from) / self.delta;
            self.from = self.current;
            self.start_time = current_time;
            self.time *= (1.0f32 - p) as f64;
            self.delta = self.to - self.from;
            self.end_time = self.start_time + self.time;
        }
        if current_time > self.end_time {
            self.active = -1;
            return self.to;
        }
        self.current = (self.from as f64
            + self.delta as f64 * ((current_time - self.start_time) / self.time))
            as f32;
        self.current
    }
}
