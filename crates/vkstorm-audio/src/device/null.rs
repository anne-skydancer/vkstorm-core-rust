//! The null device: renders on demand, never touches hardware.
//!
//! For the engine's tests and the parity recordings: the caller picks the
//! format (rate, channels, mask, headphones), pulls frames with
//! [`NullDevice::render`] and sets the device clock by hand. The callback
//! contract is the hardware devices': zeroed slices, at most
//! [`MAX_CALLBACK_FRAMES`](super::MAX_CALLBACK_FRAMES) frames per call,
//! output clamped to [-1, 1] afterwards (switchable off to compare with
//! SoLoud's own null driver, which mixed without miniaudio's clip).

#![forbid(unsafe_code)]

use super::convert::{self, SampleFormat};
use super::layout::{default_mask, is_supported_channel_count};
use super::{DeviceClock, DeviceError, DeviceFormat, MAX_CALLBACK_FRAMES, RenderCallback};

/// The null device's format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NullConfig {
    pub rate: u32,
    /// 1, 2, 4, 6 or 8.
    pub channels: u16,
    /// Speaker bits; 0 = the default layout for the channel count.
    pub channel_mask: u32,
    pub headphones: bool,
    /// Clamp the callback's output to [-1, 1] as the hardware devices do.
    pub clip: bool,
}

impl Default for NullConfig {
    fn default() -> Self {
        NullConfig {
            rate: 48000,
            channels: 2,
            channel_mask: 0,
            headphones: false,
            clip: true,
        }
    }
}

pub struct NullDevice {
    format: DeviceFormat,
    callback: Box<dyn RenderCallback>,
    buffer: Vec<f32>,
    clock: DeviceClock,
    rendered: u64,
    clip: bool,
}

impl NullDevice {
    pub fn new(
        config: NullConfig,
        callback: Box<dyn RenderCallback>,
    ) -> Result<NullDevice, DeviceError> {
        let channels = u32::from(config.channels);
        if !is_supported_channel_count(channels) {
            return Err(DeviceError::UnsupportedChannels(channels));
        }
        if config.rate == 0 {
            return Err(DeviceError::InvalidConfig("rate 0".into()));
        }
        let mask = if config.channel_mask == 0 {
            default_mask(channels)
        } else {
            config.channel_mask
        };
        if mask.count_ones() != channels {
            return Err(DeviceError::InvalidConfig(format!(
                "mask {mask:#x} does not name {channels} speakers"
            )));
        }
        let period = super::frames_from_ms(super::PERIOD_MILLISECONDS, config.rate);
        Ok(NullDevice {
            format: DeviceFormat {
                rate: config.rate,
                channels: config.channels,
                channel_mask: mask,
                headphones: config.headphones,
                backend: "Null",
                period_frames: period,
                buffer_frames: period * super::PERIODS,
                sample_format: SampleFormat::F32,
                device_name: "Null".into(),
            },
            callback,
            buffer: Vec::new(),
            clock: DeviceClock::new(),
            rendered: 0,
            clip: config.clip,
        })
    }

    pub fn format(&self) -> &DeviceFormat {
        &self.format
    }

    /// Renders `frames` frames and returns them (interleaved,
    /// `frames * channels` samples, valid until the next call).
    pub fn render(&mut self, frames: usize) -> &[f32] {
        let channels = usize::from(self.format.channels);
        self.buffer.clear();
        self.buffer.resize(frames * channels, 0.0);
        for chunk in self.buffer.chunks_mut(MAX_CALLBACK_FRAMES * channels) {
            let n = chunk.len() / channels;
            self.callback.render(chunk, n);
        }
        if self.clip {
            convert::clip(&mut self.buffer);
        }
        self.rendered += frames as u64;
        &self.buffer
    }

    /// Frames rendered so far (the "written" side of the clock).
    pub fn frames_rendered(&self) -> u64 {
        self.rendered
    }

    /// The device clock; it only moves through [`NullDevice::set_clock`].
    pub fn clock(&self) -> DeviceClock {
        self.clock.clone()
    }

    /// Sets frames played (tests simulate device latency with
    /// `frames_rendered() - queued`). Unlike hardware clocks this may move
    /// backwards if the test says so.
    pub fn set_clock(&self, frames_played: u64) {
        self.clock.set(frames_played);
    }

    pub fn callback_mut(&mut self) -> &mut dyn RenderCallback {
        self.callback.as_mut()
    }

    pub fn into_callback(self) -> Box<dyn RenderCallback> {
        self.callback
    }
}
