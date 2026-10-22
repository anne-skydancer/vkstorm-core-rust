//! The decoded-audio ring between a stream's decode thread and the mixer's
//! pull on the audio thread.
//!
//! A briefly locked ring: the decode thread copies one converted packet in
//! (microseconds under the lock), the audio thread copies a callback's worth
//! out; neither allocates or blocks on anything else under the lock. Frames
//! are at the device rate, interleaved in the layout they were decoded in;
//! layout changes (a chained Ogg link with another channel count) are kept
//! as segments so that a pull never mixes two layouts.
//!
//! Buffering policy: playback starts (`primed`) once [`Ring::start_frames`]
//! are buffered or the producer has finished; when the ring runs dry while
//! the producer is still active the pull reports an underrun, `primed`
//! drops and the mixer gets silence until the ring is back at the start
//! level. Time is never stretched: every frame is delivered exactly once,
//! in order.

#![forbid(unsafe_code)]

use std::collections::VecDeque;

use crate::music::{ChannelLayout, Pulled};

#[derive(Clone, Copy, Debug)]
struct Segment {
    layout: ChannelLayout,
    /// Samples of this segment still in the ring.
    samples: usize,
}

#[derive(Debug)]
pub(crate) struct Ring {
    buf: Vec<f32>,
    head: usize,
    len: usize,
    segments: VecDeque<Segment>,
    frames: usize,
    capacity_frames: usize,
    start_frames: usize,
    rate: u32,
    /// The session generation whose audio the ring holds.
    pub(crate) generation: u64,
    /// The session allowed to push (a cancelled session's late packets are
    /// dropped).
    pub(crate) writer: u64,
    pub(crate) primed: bool,
    /// The producer delivered its last frame (stream ended, failed or the
    /// session stopped).
    pub(crate) finished: bool,
    pub(crate) paused: bool,
    pub(crate) underruns: u64,
    pub(crate) pulled_frames: u64,
    pub(crate) pushed_frames: u64,
    last_layout: ChannelLayout,
}

impl Ring {
    pub(crate) fn new(rate: u32, capacity_ms: u32, start_ms: u32) -> Ring {
        let mut r = Ring {
            buf: Vec::new(),
            head: 0,
            len: 0,
            segments: VecDeque::new(),
            frames: 0,
            capacity_frames: 0,
            start_frames: 0,
            rate,
            generation: 0,
            writer: 0,
            primed: false,
            finished: true,
            paused: false,
            underruns: 0,
            pulled_frames: 0,
            pushed_frames: 0,
            last_layout: ChannelLayout::STEREO,
        };
        r.set_sizes(rate, capacity_ms, start_ms);
        r.buf = vec![0.0; r.capacity_frames * 2];
        r
    }

    fn set_sizes(&mut self, rate: u32, capacity_ms: u32, start_ms: u32) {
        self.rate = rate;
        self.capacity_frames = (u64::from(rate) * u64::from(capacity_ms) / 1000) as usize;
        self.start_frames = (u64::from(rate) * u64::from(start_ms) / 1000).min(self.capacity_frames as u64 / 2) as usize;
    }

    pub(crate) fn frames(&self) -> usize {
        self.frames
    }

    /// Empties the ring for a new session generation.
    pub(crate) fn reset(&mut self, generation: u64) {
        self.head = 0;
        self.len = 0;
        self.segments.clear();
        self.frames = 0;
        self.generation = generation;
        self.primed = false;
        self.finished = false;
        self.paused = false;
        self.underruns = 0;
        self.pulled_frames = 0;
        self.pushed_frames = 0;
    }

    /// Drops the buffered audio but keeps the generation (pause of a live
    /// stream, which reconnects on resume).
    pub(crate) fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
        self.segments.clear();
        self.frames = 0;
        self.primed = false;
    }

    /// Room left below the capacity, in frames. The producer waits for
    /// room before it converts a packet; [`Ring::push`] itself never drops.
    pub(crate) fn room(&self) -> usize {
        self.capacity_frames.saturating_sub(self.frames)
    }

    /// Appends interleaved frames (all of them: the store grows past the
    /// capacity if a producer overshoots, which the room check before each
    /// packet keeps to a packet's worth).
    pub(crate) fn push(&mut self, layout: ChannelLayout, data: &[f32]) {
        let ch = usize::from(layout.channels.max(1));
        let n = data.len() / ch;
        if n == 0 {
            return;
        }
        let samples = n * ch;
        let need = (self.len + samples).max(self.capacity_frames * ch);
        if need > self.buf.len() {
            self.grow(need);
        }
        let cap = self.buf.len();
        let mut tail = (self.head + self.len) % cap;
        let mut src = &data[..samples];
        while !src.is_empty() {
            let run = src.len().min(cap - tail);
            self.buf[tail..tail + run].copy_from_slice(&src[..run]);
            src = &src[run..];
            tail = (tail + run) % cap;
        }
        self.len += samples;
        self.frames += n;
        self.pushed_frames += n as u64;
        match self.segments.back_mut() {
            Some(s) if s.layout == layout => s.samples += samples,
            _ => self.segments.push_back(Segment { layout, samples }),
        }
        if !self.primed && self.frames >= self.start_frames {
            self.primed = true;
        }
    }

    /// Reallocates the sample store to `samples`, keeping the content.
    fn grow(&mut self, samples: usize) {
        let mut v = Vec::with_capacity(samples);
        let cap = self.buf.len();
        for i in 0..self.len {
            v.push(self.buf[(self.head + i) % cap]);
        }
        v.resize(samples, 0.0);
        self.buf = v;
        self.head = 0;
    }

    /// The producer is done: whatever is buffered plays out.
    pub(crate) fn finish(&mut self) {
        self.finished = true;
        self.primed = true;
    }

    /// The mixer's pull (see [`crate::music::MusicSource::pull`]).
    pub(crate) fn pull(&mut self, out: &mut [f32], max_frames: usize) -> Pulled {
        let layout = self.segments.front().map_or(self.last_layout, |s| s.layout);
        if self.paused || (!self.primed && !self.finished) {
            return Pulled { frames: 0, layout, ended: false };
        }
        let mut frames = 0;
        if let Some(seg) = self.segments.front_mut() {
            let ch = usize::from(seg.layout.channels.max(1));
            let room = (out.len() / ch).min(max_frames);
            let n = (seg.samples / ch).min(room);
            let samples = n * ch;
            let cap = self.buf.len();
            let mut done = 0;
            while done < samples {
                let run = (samples - done).min(cap - self.head);
                out[done..done + run].copy_from_slice(&self.buf[self.head..self.head + run]);
                done += run;
                self.head = (self.head + run) % cap;
            }
            seg.samples -= samples;
            self.len -= samples;
            self.frames -= n;
            frames = n;
            self.last_layout = seg.layout;
            if seg.samples == 0 {
                self.segments.pop_front();
            }
        }
        self.pulled_frames += frames as u64;
        if self.len == 0 {
            self.head = 0;
        }
        let ended = self.finished && self.len == 0;
        if !ended && self.len == 0 && frames < max_frames {
            // Ran dry while the stream is still coming: buffer again.
            self.primed = false;
            self.underruns += 1;
        }
        Pulled { frames, layout, ended }
    }

    /// Converts the buffered audio to a new device rate (device change) and
    /// resizes the ring for it. Runs with the decode side quiescent (the
    /// caller holds the output stage's lock).
    pub(crate) fn change_rate(&mut self, rate: u32, capacity_ms: u32, start_ms: u32) {
        use super::resample::Resampler;
        if rate == self.rate {
            return;
        }
        let old_rate = self.rate;
        // Take the content segment by segment and convert it.
        let cap = self.buf.len();
        let mut converted: Vec<(ChannelLayout, Vec<f32>)> = Vec::new();
        let mut at = self.head;
        for seg in &self.segments {
            let ch = usize::from(seg.layout.channels.max(1));
            let n = seg.samples / ch;
            let mut planes = vec![Vec::with_capacity(n); ch];
            for f in 0..n {
                for (c, p) in planes.iter_mut().enumerate() {
                    p.push(self.buf[(at + f * ch + c) % cap]);
                }
            }
            at = (at + seg.samples) % cap;
            let mut r = Resampler::new(old_rate, rate, ch);
            let refs: Vec<&[f32]> = planes.iter().map(Vec::as_slice).collect();
            let mut out = Vec::new();
            r.process(&refs, n, &mut out);
            r.flush(&mut out);
            converted.push((seg.layout, out));
        }
        let (generation, primed, finished, paused) = (self.generation, self.primed, self.finished, self.paused);
        let (underruns, pulled, pushed) = (self.underruns, self.pulled_frames, self.pushed_frames);
        self.set_sizes(rate, capacity_ms, start_ms);
        let widest = converted.iter().map(|(l, _)| usize::from(l.channels)).max().unwrap_or(2).max(2);
        self.buf = vec![0.0; self.capacity_frames.max(1) * widest];
        self.head = 0;
        self.len = 0;
        self.frames = 0;
        self.segments.clear();
        for (layout, data) in converted {
            self.push(layout, &data);
        }
        self.generation = generation;
        self.primed = primed;
        self.finished = finished;
        self.paused = paused;
        self.underruns = underruns;
        self.pulled_frames = pulled;
        self.pushed_frames = pushed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONO: ChannelLayout = ChannelLayout::MONO;
    const ST: ChannelLayout = ChannelLayout::STEREO;

    #[test]
    fn primes_then_plays_in_order() {
        let mut r = Ring::new(1000, 1000, 100); // 1000 frames, start at 100
        r.reset(1);
        let data: Vec<f32> = (0..200).map(|i| i as f32).collect();
        r.push(ST, &data[..100]);
        assert_eq!(r.frames(), 50);
        let mut out = vec![0.0; 64];
        assert_eq!(r.pull(&mut out, 8).frames, 0, "not primed yet");
        r.push(ST, &data[100..]);
        let p = r.pull(&mut out, 8);
        assert_eq!(p.frames, 8);
        assert_eq!(&out[..4], &[0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn underrun_rebuffers_and_finish_drains() {
        let mut r = Ring::new(1000, 1000, 10);
        r.reset(1);
        r.push(MONO, &[1.0; 10]);
        let mut out = vec![0.0; 64];
        let p = r.pull(&mut out, 16);
        assert_eq!((p.frames, p.ended), (10, false));
        assert_eq!(r.underruns, 1);
        r.push(MONO, &[2.0; 5]);
        assert_eq!(r.pull(&mut out, 16).frames, 0, "below the start level again");
        r.finish();
        let p = r.pull(&mut out, 16);
        assert_eq!((p.frames, p.ended), (5, true));
    }

    #[test]
    fn layout_change_ends_pull() {
        let mut r = Ring::new(1000, 1000, 1);
        r.reset(1);
        r.push(ST, &[1.0; 8]);
        r.push(MONO, &[2.0; 3]);
        r.finish();
        let mut out = vec![0.0; 64];
        let p = r.pull(&mut out, 16);
        assert_eq!((p.frames, p.layout), (4, ST));
        let p = r.pull(&mut out, 16);
        assert_eq!((p.frames, p.layout, p.ended), (3, MONO, true));
    }

    #[test]
    fn room_and_overshoot() {
        let mut r = Ring::new(1000, 100, 10); // 100 frames
        r.reset(1);
        r.push(ST, &[1.0; 160]);
        assert_eq!(r.room(), 20);
        let data: Vec<f32> = (0..100).map(|i| i as f32).collect();
        r.push(ST, &data); // 130 frames: past the capacity
        assert_eq!(r.room(), 0);
        let mut out = vec![0.0; 400];
        assert_eq!(r.pull(&mut out, 200).frames, 130);
        assert_eq!(&out[160..260], &data[..]);
    }

    #[test]
    fn wider_layout_grows_store() {
        let mut r = Ring::new(1000, 100, 1);
        r.reset(1);
        r.push(ST, &[1.0; 20]);
        let six = ChannelLayout { channels: 6, mask: 0x3f };
        r.push(six, &[2.0; 540]);
        r.finish();
        let mut out = vec![0.0; 800];
        assert_eq!(r.pull(&mut out, 100).frames, 10);
        let p = r.pull(&mut out, 100);
        assert_eq!((p.frames, p.layout.channels), (90, 6));
        assert!(out[..540].iter().all(|&x| x == 2.0));
    }
}
