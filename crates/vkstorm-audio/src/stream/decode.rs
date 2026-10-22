//! The decode side of a connection: dispatch to the demuxer of the detected
//! format, and the conversion of decoded audio to the device rate
//! ([`Converter`]), segment by segment (a segment ends where the source rate
//! or channel layout changes, e.g. at a chained Ogg link with another rate).
//!
//! Also an offline entry point ([`decode_bytes`]) that runs the same
//! pipeline over a byte buffer, for tests and the CPU measurements.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;

use super::codec::{InBand, PcmSink, StreamError};
use super::ffmpeg::{self, Ffmpeg};
use super::input::Input;
use super::pipe::Head;
use super::resample::Resampler;
use super::sniff::{Detected, Format, detect};
use crate::music::ChannelLayout;

/// What the decoders need from the configuration.
#[derive(Clone, Debug, Default)]
pub(crate) struct DecodeCtx {
    pub ffmpeg_dir: Option<PathBuf>,
}

impl DecodeCtx {
    /// The FFmpeg libraries, loaded on first use.
    pub(crate) fn ffmpeg(&self) -> Result<Arc<Ffmpeg>, String> {
        match &self.ffmpeg_dir {
            Some(d) => ffmpeg::load(d),
            None => Err("no FFmpeg directory is configured".into()),
        }
    }
}

/// Runs the demuxer and decoder for `format` until the input ends.
pub(crate) fn run_format(format: Format, input: &mut Input, ctx: &DecodeCtx, sink: &mut dyn PcmSink) -> Result<(), StreamError> {
    match format {
        Format::Mpeg => super::demux::run_mpeg(input, sink),
        Format::Adts => super::demux::run_adts(input, ctx, sink),
        Format::Flac => super::demux::run_flac(input, sink),
        Format::Ogg => super::ogg::run_ogg(input, ctx, sink),
    }
}

/// One stretch of constant source rate and layout, converted to the
/// device rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentStats {
    pub source_rate: u32,
    pub out_rate: u32,
    pub layout: ChannelLayout,
    /// Decoded frames at the source rate.
    pub in_frames: u64,
    /// Frames delivered at the device rate.
    pub out_frames: u64,
    /// The device rate changed during the segment (the count is then
    /// exact only from the change on).
    pub rate_changed: bool,
}

/// Converts decoded audio to the device rate and interleaves it.
pub(crate) struct Converter {
    out_rate: u32,
    current: Option<(u32, ChannelLayout, Resampler, bool)>,
    scratch: Vec<f32>,
    pub(crate) segments: Vec<SegmentStats>,
}

impl Converter {
    pub(crate) fn new(out_rate: u32) -> Converter {
        Converter { out_rate, current: None, scratch: Vec::new(), segments: Vec::new() }
    }

    /// Converts a packet; the interleaved result (in `layout`) is handed to
    /// `push`. A rate or layout change first ends the current segment.
    pub(crate) fn feed(
        &mut self,
        planes: &[&[f32]],
        frames: usize,
        rate: u32,
        layout: ChannelLayout,
        push: &mut dyn FnMut(ChannelLayout, &[f32]),
    ) {
        if self.current.as_ref().is_some_and(|(r, l, _, _)| *r != rate || *l != layout) {
            self.end_segment(push);
        }
        let out_rate = self.out_rate;
        let (_, _, rs, _) =
            self.current.get_or_insert_with(|| (rate, layout, Resampler::new(rate, out_rate, planes.len()), false));
        self.scratch.clear();
        rs.process(planes, frames, &mut self.scratch);
        if !self.scratch.is_empty() {
            push(layout, &self.scratch);
        }
    }

    /// Ends the current segment: the resampler's tail is pushed and the
    /// segment recorded.
    pub(crate) fn end_segment(&mut self, push: &mut dyn FnMut(ChannelLayout, &[f32])) {
        if let Some((rate, layout, mut rs, changed)) = self.current.take() {
            self.scratch.clear();
            rs.flush(&mut self.scratch);
            if !self.scratch.is_empty() {
                push(layout, &self.scratch);
            }
            self.segments.push(SegmentStats {
                source_rate: rate,
                out_rate: rs.out_rate(),
                layout,
                in_frames: rs.in_frames(),
                out_frames: rs.out_frames(),
                rate_changed: changed,
            });
        }
    }

    /// The device rate changed: later output is at `rate`.
    pub(crate) fn set_out_rate(&mut self, rate: u32) {
        self.out_rate = rate;
        if let Some((_, _, rs, changed)) = &mut self.current
            && rs.out_rate() != rate
        {
            rs.set_out_rate(rate);
            *changed = true;
        }
    }

    /// The segment in progress (source rate, layout, frames in and out).
    pub(crate) fn current(&self) -> Option<SegmentStats> {
        self.current.as_ref().map(|(rate, layout, rs, changed)| SegmentStats {
            source_rate: *rate,
            out_rate: rs.out_rate(),
            layout: *layout,
            in_frames: rs.in_frames(),
            out_frames: rs.out_frames(),
            rate_changed: *changed,
        })
    }
}

/// The result of [`decode_bytes`].
#[derive(Debug, Default)]
pub struct OfflineDecoded {
    /// Each segment with its converted samples (interleaved; empty when the
    /// samples were not kept).
    pub segments: Vec<(SegmentStats, Vec<f32>)>,
    /// In-band metadata in arrival order: (artist, title); ICY-style text
    /// arrives split on " - ".
    pub tags: Vec<(String, String)>,
    /// The codecs used, in order.
    pub codecs: Vec<&'static str>,
}

struct OfflineSink {
    conv: Converter,
    keep: bool,
    current: Vec<f32>,
    done: Vec<Vec<f32>>,
    tags: Vec<(String, String)>,
    codecs: Vec<&'static str>,
}

impl OfflineSink {
    fn push_into(keep: bool, current: &mut Vec<f32>) -> impl FnMut(ChannelLayout, &[f32]) + '_ {
        move |_, data: &[f32]| {
            if keep {
                current.extend_from_slice(data);
            }
        }
    }

    fn close(&mut self) {
        let before = self.conv.segments.len();
        {
            let mut push = Self::push_into(self.keep, &mut self.current);
            self.conv.end_segment(&mut push);
        }
        if self.conv.segments.len() > before {
            self.done.push(std::mem::take(&mut self.current));
        }
    }
}

impl PcmSink for OfflineSink {
    fn pcm(&mut self, planes: &[&[f32]], frames: usize, rate: u32, layout: ChannelLayout) -> Result<(), StreamError> {
        if self.conv.current().is_some_and(|s| s.source_rate != rate || s.layout != layout) {
            self.close();
        }
        let mut push = Self::push_into(self.keep, &mut self.current);
        self.conv.feed(planes, frames, rate, layout, &mut push);
        Ok(())
    }

    fn metadata(&mut self, m: InBand) {
        self.tags.push(match m {
            InBand::NowPlaying(t) => super::icy::split_now_playing(&t),
            InBand::Tags { artist, title } => (artist, title),
        });
    }

    fn codec(&mut self, name: &'static str) {
        self.codecs.push(name);
    }
}

/// Decodes a whole stream held in memory (any supported format, detected
/// as for a connection) and converts it to `out_rate`. `keep_samples`
/// false only counts (for measurements).
#[doc(hidden)]
pub fn decode_bytes(
    data: &[u8],
    out_rate: u32,
    ffmpeg_dir: Option<PathBuf>,
    keep_samples: bool,
) -> Result<OfflineDecoded, String> {
    let ctx = DecodeCtx { ffmpeg_dir };
    let mut input = Input::from_memory(data);
    let mut sink = OfflineSink {
        conv: Converter::new(out_rate),
        keep: keep_samples,
        current: Vec::new(),
        done: Vec::new(),
        tags: Vec::new(),
        codecs: Vec::new(),
    };
    let head = Head { status: 200, ..Default::default() };
    let format = match detect(&mut input, &head, "", &mut sink).map_err(|e| format!("{e:?}"))? {
        Detected::Audio(f) => f,
        Detected::Playlist { .. } => return Err("a playlist".into()),
        Detected::Refused(r) => return Err(r),
    };
    match run_format(format, &mut input, &ctx, &mut sink) {
        Ok(()) => {}
        Err(StreamError::Failed(r)) => return Err(r),
        Err(StreamError::Cancelled) => return Err("cancelled".into()),
    }
    sink.close();
    let segments = sink.conv.segments.iter().copied().zip(sink.done).collect();
    Ok(OfflineDecoded { segments, tags: sink.tags, codecs: sink.codecs })
}
