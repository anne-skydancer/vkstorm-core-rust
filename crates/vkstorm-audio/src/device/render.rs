//! The data callback path shared by every backend: pre-zeroing, splitting
//! into at most [`MAX_CALLBACK_FRAMES`] frames per call, clipping, channel
//! reordering and sample conversion, and panic containment.
//!
//! Call sizes reproduce miniaudio 0.10.42 + the 3p-soloud bounded-mix
//! patch: when the device takes `f32` (miniaudio's passthrough case) the
//! whole device request is rendered in place, split into 1024-frame calls;
//! otherwise miniaudio rendered into a 4096-byte stack buffer in the client
//! format (`MA_DATA_CONVERTER_STACK_BUFFER_SIZE`, i.e. 4096 / (4 x channels)
//! frames per call) and converted each piece, which [`Renderer`] does too.

#![forbid(unsafe_code)]

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::convert::{self, SampleFormat};
use super::layout::DeviceOrder;
use super::{MAX_CALLBACK_FRAMES, RenderCallback};

/// miniaudio's intermediary buffer for converted output, in bytes.
pub const CONVERTER_BUFFER_BYTES: usize = 4096;

/// Renders `out` (interleaved, `channels` per frame) the way the device's
/// data callback does: zero it, call `callback` with at most
/// [`MAX_CALLBACK_FRAMES`] frames per call in order, then clip to [-1, 1].
/// Nothing outside `out` is touched (the 3p-soloud callback test's guard
/// samples).
pub fn render_split(callback: &mut dyn RenderCallback, out: &mut [f32], channels: usize) {
    if channels == 0 {
        return;
    }
    let frames = out.len() / channels;
    let out = &mut out[..frames * channels];
    out.fill(0.0);
    for chunk in out.chunks_mut(MAX_CALLBACK_FRAMES * channels) {
        let n = chunk.len() / channels;
        callback.render(chunk, n);
    }
    convert::clip(out);
}

/// The render state a backend thread owns while running.
pub(crate) struct Renderer {
    callback: Box<dyn RenderCallback>,
    channels: usize,
    format: SampleFormat,
    order: DeviceOrder,
    staging: Vec<f32>,
    failed: bool,
}

impl Renderer {
    pub(crate) fn new(
        callback: Box<dyn RenderCallback>,
        channels: usize,
        format: SampleFormat,
        order: DeviceOrder,
    ) -> Renderer {
        let staging_frames = (CONVERTER_BUFFER_BYTES / (4 * channels.max(1))).max(1);
        Renderer {
            callback,
            channels,
            format,
            order,
            staging: vec![0.0; staging_frames * channels],
            failed: false,
        }
    }

    /// True once the callback panicked; from then on only silence is
    /// produced and the backend reports the device as failed.
    pub(crate) fn failed(&self) -> bool {
        self.failed
    }

    pub(crate) fn into_callback(self) -> Box<dyn RenderCallback> {
        self.callback
    }

    fn call(&mut self, out: &mut [f32]) {
        if self.failed {
            out.fill(0.0);
            return;
        }
        let channels = self.channels;
        let callback = &mut self.callback;
        let result = catch_unwind(AssertUnwindSafe(|| {
            render_split(callback.as_mut(), out, channels)
        }));
        if result.is_err() {
            self.failed = true;
            out.fill(0.0);
        }
    }

    /// Fills a float device buffer (`format` is [`SampleFormat::F32`]) in
    /// place: miniaudio's passthrough path.
    pub(crate) fn render_f32(&mut self, out: &mut [f32]) {
        debug_assert_eq!(self.format, SampleFormat::F32);
        self.call(out);
        self.order.apply(out, self.channels);
    }

    /// Fills a device buffer of any format through the 4096-byte staging
    /// buffer: miniaudio's converting path.
    pub(crate) fn render_bytes(&mut self, out: &mut [u8]) {
        let channels = self.channels.max(1);
        let frame_bytes = channels * self.format.bytes_per_sample();
        let staging_frames = self.staging.len() / channels;
        let frames = out.len() / frame_bytes;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(staging_frames);
            let mut staging = std::mem::take(&mut self.staging);
            let piece = &mut staging[..n * channels];
            self.call(piece);
            self.order.apply(piece, channels);
            convert::encode(
                self.format,
                piece,
                &mut out[done * frame_bytes..(done + n) * frame_bytes],
            );
            self.staging = staging;
            done += n;
        }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn format(&self) -> SampleFormat {
        self.format
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Recorder {
        calls: Vec<usize>,
        value: f32,
    }
    impl RenderCallback for Recorder {
        fn render(&mut self, out: &mut [f32], frames: usize) {
            assert!(out.iter().all(|&s| s == 0.0), "output is pre-zeroed");
            self.calls.push(frames);
            out.fill(self.value);
        }
    }

    #[test]
    fn converting_path_uses_the_4096_byte_staging_buffer() {
        let rec = Recorder {
            calls: vec![],
            value: 0.5,
        };
        let mut r = Renderer::new(
            Box::new(rec),
            2,
            SampleFormat::S16,
            DeviceOrder {
                mask: 3,
                source: None,
            },
        );
        let mut out = vec![0u8; 1300 * 2 * 2];
        r.render_bytes(&mut out);
        assert!(
            out.as_chunks::<2>()
                .0
                .iter()
                .all(|c| i16::from_le_bytes(*c) == 16383)
        );
        let any: Box<dyn std::any::Any> = r.into_callback();
        let rec = any.downcast::<Recorder>().expect("recorder");
        assert_eq!(rec.calls, [512, 512, 276]);
    }

    #[test]
    fn panicking_callback_is_contained() {
        struct Bad;
        impl RenderCallback for Bad {
            fn render(&mut self, out: &mut [f32], _frames: usize) {
                out.fill(0.9);
                panic!("mixer bug");
            }
        }
        let mut r = Renderer::new(
            Box::new(Bad),
            2,
            SampleFormat::F32,
            DeviceOrder {
                mask: 3,
                source: None,
            },
        );
        let mut out = vec![1.0f32; 64];
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        r.render_f32(&mut out);
        std::panic::set_hook(hook);
        assert!(r.failed());
        assert!(out.iter().all(|&s| s == 0.0));
        r.render_f32(&mut out);
        assert!(out.iter().all(|&s| s == 0.0));
    }
}
