//! Derived from SoLoud (Copyright (c) 2013-2020 Jari Komppa), zlib licence;
//! altered: rewritten in Rust. See LICENSE-soloud in this crate.
//!
//! The WAV reader the viewer's sound buffers load with: what SoLoud's
//! `Wav::load` produces for a RIFF file (dr_wav 0.13.2's
//! `drwav_init_memory` + `drwav_read_pcm_frames_f32`, then SoLoud's
//! conversion to planar float in 512-frame blocks), sample for sample.
//!
//! The viewer only ever loads its decoded-sound cache files, which
//! `LLVorbisDecodeState` writes with a 44-byte header that always says mono,
//! 44100 Hz, 16-bit, even when the Ogg Vorbis asset was stereo: the stereo
//! samples then load (here as in SoLoud) as one interleaved mono channel of
//! twice the frames, played at 44.1 kHz. This module reads what the header
//! says, like dr_wav.
//!
//! Supported as dr_wav does: RIFF/WAVE with PCM of any width up to 64 bits
//! (8-bit unsigned, 16/24/32-bit signed, other widths through dr_wav's
//! generic path), IEEE float 32/64, A-law and µ-law, `WAVE_FORMAT_EXTENSIBLE`
//! (subformat), odd chunk padding, chunks before and after `fmt `/`data`.
//! Differences from SoLoud, all on files the viewer never writes:
//! - Only the RIFF container: SoLoud dispatches on the first four bytes and
//!   hands `OggS`, `fLaC` and anything else (MP3 by probing) to other
//!   decoders; those return [`WavLoadError::NotRiff`] here.
//! - MS-ADPCM and IMA-ADPCM and unknown format tags are rejected
//!   ([`WavLoadError::Unsupported`]); dr_wav decodes the ADPCMs, and for
//!   unknown tags SoLoud keeps uninitialised memory as samples.
//! - More than 8 channels is rejected (SoLoud overruns its 8-channel stack
//!   block); an A-law/µ-law file whose frame size is not one byte per
//!   channel is rejected (dr_wav divides by zero); a header claiming more
//!   than 8 frames per file byte is rejected (SoLoud would allocate it).
//! - A data chunk longer than the file: SoLoud ignores the short read and
//!   keeps whatever its stack block held; the missing samples are zero here.
//!   dr_wav's byte accounting is kept, including the loss of a partial
//!   frame's bytes on a short read and the 32-bit float fast path's
//!   partial-frame samples.

#![forbid(unsafe_code)]
#![allow(clippy::needless_range_loop, clippy::excessive_precision)]

use std::sync::Arc;

use crate::mixer::Wav;

/// Why a file could not be loaded (SoLoud reports `FILE_LOAD_FAILED` for
/// all of them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavLoadError {
    /// Empty, or not starting with `RIFF`.
    NotRiff,
    /// A malformed or truncated header, no `fmt ` or `data` chunk, invalid
    /// rate/channels/bits/block alignment.
    Invalid,
    /// A format SoLoud would not decode faithfully (see the module docs).
    Unsupported(u16),
    /// More than 8 channels.
    TooManyChannels(u16),
    /// No frames.
    Empty,
}

impl std::fmt::Display for WavLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WavLoadError::NotRiff => f.write_str("not a RIFF/WAVE file"),
            WavLoadError::Invalid => f.write_str("invalid WAV header"),
            WavLoadError::Unsupported(tag) => write!(f, "unsupported WAV format 0x{tag:04x}"),
            WavLoadError::TooManyChannels(n) => write!(f, "{n} channels (at most 8)"),
            WavLoadError::Empty => f.write_str("no audio frames"),
        }
    }
}

impl std::error::Error for WavLoadError {}

/// Decoded audio: planar `f32` (channel `c` at `c * sample_count`).
#[derive(Clone, Debug, PartialEq)]
pub struct WavData {
    pub channels: u32,
    pub sample_rate: u32,
    /// Frames per channel.
    pub sample_count: u32,
    pub data: Vec<f32>,
}

const FORMAT_PCM: u16 = 0x1;
const FORMAT_ADPCM: u16 = 0x2;
const FORMAT_IEEE_FLOAT: u16 = 0x3;
const FORMAT_ALAW: u16 = 0x6;
const FORMAT_MULAW: u16 = 0x7;
const FORMAT_DVI_ADPCM: u16 = 0x11;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

const MAX_SAMPLE_RATE: u32 = 384_000;
const MAX_CHANNELS: u16 = 256;
const MAX_BITS_PER_SAMPLE: u16 = 64;

/// dr_wav's memory stream (`drwav__on_read_memory`/`_on_seek_memory`).
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn read(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.data.len() - self.pos);
        out[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        n
    }

    fn seek_current(&mut self, offset: i32) -> bool {
        if offset > 0 {
            if self.pos + offset as usize > self.data.len() {
                return false;
            }
        } else if self.pos < offset.unsigned_abs() as usize {
            return false;
        }
        self.pos = (self.pos as i64 + offset as i64) as usize;
        true
    }

    fn seek_start(&mut self, offset: i32) -> bool {
        if (offset as u32 as usize) <= self.data.len() {
            self.pos = offset as u32 as usize;
            true
        } else {
            false
        }
    }

    /// `drwav__seek_forward`.
    fn seek_forward(&mut self, offset: u64) -> bool {
        let mut remaining = offset;
        while remaining > 0 {
            let step = remaining.min(0x7FFF_FFFF);
            if !self.seek_current(step as i32) {
                return false;
            }
            remaining -= step;
        }
        true
    }
}

struct ChunkHeader {
    id: [u8; 4],
    size: u64,
    padding: u64,
}

/// `drwav__read_chunk_header` (RIFF).
fn read_chunk_header(r: &mut Reader, cursor: &mut u64) -> Option<ChunkHeader> {
    let mut id = [0u8; 4];
    if r.read(&mut id) != 4 {
        return None;
    }
    let mut size = [0u8; 4];
    if r.read(&mut size) != 4 {
        return None;
    }
    let size = u32::from_le_bytes(size) as u64;
    *cursor = cursor.wrapping_add(8);
    Some(ChunkHeader {
        id,
        size,
        padding: size % 2,
    })
}

#[derive(Clone, Copy)]
struct Fmt {
    format_tag: u16,
    channels: u16,
    sample_rate: u32,
    block_align: u16,
    bits_per_sample: u16,
    sub_format_tag: u16,
}

/// `drwav__read_fmt` (RIFF).
fn read_fmt(r: &mut Reader, cursor: &mut u64) -> Option<Fmt> {
    let mut header = read_chunk_header(r, cursor)?;
    while &header.id != b"fmt " {
        if !r.seek_forward(header.size + header.padding) {
            return None;
        }
        *cursor = cursor.wrapping_add(header.size + header.padding);
        header = read_chunk_header(r, cursor)?;
    }
    let mut fmt = [0u8; 16];
    if r.read(&mut fmt) != 16 {
        return None;
    }
    *cursor = cursor.wrapping_add(16);
    let mut out = Fmt {
        format_tag: u16::from_le_bytes([fmt[0], fmt[1]]),
        channels: u16::from_le_bytes([fmt[2], fmt[3]]),
        sample_rate: u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]),
        block_align: u16::from_le_bytes([fmt[12], fmt[13]]),
        bits_per_sample: u16::from_le_bytes([fmt[14], fmt[15]]),
        sub_format_tag: 0,
    };
    if header.size > 16 {
        let mut cb = [0u8; 2];
        if r.read(&mut cb) != 2 {
            return None;
        }
        *cursor = cursor.wrapping_add(2);
        let mut bytes_read_so_far: i32 = 18;
        let extended = u16::from_le_bytes(cb);
        if extended > 0 {
            if out.format_tag == FORMAT_EXTENSIBLE {
                if extended != 22 {
                    return None;
                }
                let mut ext = [0u8; 22];
                if r.read(&mut ext) != 22 {
                    return None;
                }
                out.sub_format_tag = u16::from_le_bytes([ext[6], ext[7]]);
            } else if !r.seek_current(extended as i32) {
                return None;
            }
            *cursor = cursor.wrapping_add(extended as u64);
            bytes_read_so_far += extended as i32;
        }
        // `(int)(sizeInBytes - bytesReadSoFar)`: may be negative.
        let rest = header.size.wrapping_sub(bytes_read_so_far as i64 as u64);
        if !r.seek_current(rest as i32) {
            return None;
        }
        *cursor = cursor.wrapping_add(rest);
    }
    if header.padding > 0 {
        if !r.seek_current(header.padding as i32) {
            return None;
        }
        *cursor = cursor.wrapping_add(header.padding);
    }
    Some(out)
}

struct Decoder<'a> {
    r: Reader<'a>,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    block_align: u16,
    format: u16,
    bytes_remaining: u64,
    total_frames: u64,
}

impl Decoder<'_> {
    /// `drwav_get_bytes_per_pcm_frame`.
    fn bytes_per_frame(&self) -> u32 {
        let bpf = if self.bits_per_sample & 7 == 0 {
            (self.bits_per_sample as u32 * self.channels as u32) >> 3
        } else {
            self.block_align as u32
        };
        if (self.format == FORMAT_ALAW || self.format == FORMAT_MULAW)
            && bpf != self.channels as u32
        {
            return 0;
        }
        bpf
    }

    /// `drwav_read_raw` into `out`; returns bytes read.
    fn read_raw(&mut self, out: &mut [u8]) -> usize {
        let want = (out.len() as u64).min(self.bytes_remaining) as usize;
        if want == 0 {
            return 0;
        }
        let n = self.r.read(&mut out[..want]);
        self.bytes_remaining -= n as u64;
        n
    }
}

/// `drwav_init_memory` for a RIFF file.
fn init(data: &[u8]) -> Result<Decoder<'_>, WavLoadError> {
    if data.is_empty() {
        return Err(WavLoadError::NotRiff);
    }
    let mut r = Reader { data, pos: 0 };
    let mut cursor: u64 = 0;
    let mut riff = [0u8; 4];
    if r.read(&mut riff) != 4 {
        return Err(WavLoadError::NotRiff);
    }
    cursor += 4;
    if &riff != b"RIFF" {
        return Err(WavLoadError::NotRiff);
    }
    let mut size = [0u8; 4];
    if r.read(&mut size) != 4 {
        return Err(WavLoadError::Invalid);
    }
    cursor += 4;
    if u32::from_le_bytes(size) < 36 {
        return Err(WavLoadError::Invalid);
    }
    let mut wave = [0u8; 4];
    if r.read(&mut wave) != 4 || &wave != b"WAVE" {
        return Err(WavLoadError::Invalid);
    }
    cursor += 4;

    let fmt = read_fmt(&mut r, &mut cursor).ok_or(WavLoadError::Invalid)?;
    if fmt.sample_rate == 0
        || fmt.sample_rate > MAX_SAMPLE_RATE
        || fmt.channels == 0
        || fmt.channels > MAX_CHANNELS
        || fmt.bits_per_sample == 0
        || fmt.bits_per_sample > MAX_BITS_PER_SAMPLE
        || fmt.block_align == 0
    {
        return Err(WavLoadError::Invalid);
    }
    let format = if fmt.format_tag == FORMAT_EXTENSIBLE {
        fmt.sub_format_tag
    } else {
        fmt.format_tag
    };

    let mut found_data = false;
    let mut data_chunk_size: u64 = 0;
    let mut data_pos: u64 = 0;
    loop {
        let Some(header) = read_chunk_header(&mut r, &mut cursor) else {
            if !found_data {
                return Err(WavLoadError::Invalid);
            }
            break;
        };
        if !found_data {
            data_pos = cursor;
        }
        let mut chunk_size = header.size;
        if &header.id == b"data" {
            found_data = true;
            data_chunk_size = chunk_size;
        }
        if &header.id == b"fact" {
            // dr_wav reads the sample count but, its format tag not being set
            // yet at this point, never uses it for RIFF.
            let mut count = [0u8; 4];
            let n = r.read(&mut count);
            cursor += n as u64;
            if n != 4 {
                return Err(WavLoadError::Invalid);
            }
            chunk_size = chunk_size.wrapping_sub(4);
            if !found_data {
                data_pos = cursor;
            }
        }
        chunk_size = chunk_size.wrapping_add(header.padding);
        if !r.seek_forward(chunk_size) {
            break;
        }
        cursor = cursor.wrapping_add(chunk_size);
        if !found_data {
            data_pos = cursor;
        }
    }
    if !found_data {
        return Err(WavLoadError::Invalid);
    }
    // drwav__seek_from_start
    if data_pos > 0x7FFF_FFFF || !r.seek_start(data_pos as i32) {
        return Err(WavLoadError::Invalid);
    }

    let mut dec = Decoder {
        r,
        channels: fmt.channels,
        sample_rate: fmt.sample_rate,
        bits_per_sample: fmt.bits_per_sample,
        block_align: fmt.block_align,
        format,
        bytes_remaining: data_chunk_size,
        total_frames: 0,
    };
    if format == FORMAT_ADPCM || format == FORMAT_DVI_ADPCM {
        return Err(WavLoadError::Unsupported(format));
    }
    let bpf = dec.bytes_per_frame();
    if bpf == 0 {
        // dr_wav divides by this before checking it.
        return Err(WavLoadError::Invalid);
    }
    dec.total_frames = data_chunk_size / bpf as u64;
    Ok(dec)
}

fn alaw_to_f32(x: u8) -> f32 {
    ALAW_TABLE[x as usize] as i16 as f32 / 32768.0
}

fn mulaw_to_f32(x: u8) -> f32 {
    MULAW_TABLE[x as usize] as i16 as f32 / 32768.0
}

/// `drwav__pcm_to_f32`.
fn pcm_to_f32(out: &mut [f32], input: &[u8], count: usize, bytes_per_sample: usize) {
    match bytes_per_sample {
        1 => {
            for i in 0..count {
                let mut x = input[i] as f32;
                x *= 0.007_843_137_254_901_960_78_f32;
                x -= 1.0;
                out[i] = x;
            }
        }
        2 => {
            for i in 0..count {
                out[i] = i16::from_le_bytes([input[2 * i], input[2 * i + 1]]) as f32
                    * 0.000_030_517_578_125_f32;
            }
        }
        3 => {
            for i in 0..count {
                let a = (input[i * 3] as u32) << 8;
                let b = (input[i * 3 + 1] as u32) << 16;
                let c = (input[i * 3 + 2] as u32) << 24;
                let x = ((a | b | c) as i32 >> 8) as f64;
                out[i] = (x * 0.000_000_119_209_289_550_781_25) as f32;
            }
        }
        4 => {
            for i in 0..count {
                let v = i32::from_le_bytes([
                    input[4 * i],
                    input[4 * i + 1],
                    input[4 * i + 2],
                    input[4 * i + 3],
                ]);
                out[i] = (v as f64 / 2_147_483_648.0) as f32;
            }
        }
        b if b > 8 => out[..count].fill(0.0),
        b => {
            let mut p = 0usize;
            for o in out[..count].iter_mut() {
                let mut sample: u64 = 0;
                let mut shift = (8 - b as u32) * 8;
                for j in 0..b {
                    sample |= (input[p + j] as u64) << shift;
                    shift += 8;
                }
                p += b;
                *o = (sample as i64 as f64 / 9_223_372_036_854_775_807.0) as f32;
            }
        }
    }
}

/// `drwav__ieee_to_f32`.
fn ieee_to_f32(out: &mut [f32], input: &[u8], count: usize, bytes_per_sample: usize) {
    match bytes_per_sample {
        4 => {
            for i in 0..count {
                out[i] = f32::from_le_bytes([
                    input[4 * i],
                    input[4 * i + 1],
                    input[4 * i + 2],
                    input[4 * i + 3],
                ]);
            }
        }
        8 => {
            for i in 0..count {
                let mut b = [0u8; 8];
                b.copy_from_slice(&input[8 * i..8 * i + 8]);
                out[i] = f64::from_le_bytes(b) as f32;
            }
        }
        _ => out[..count].fill(0.0),
    }
}

/// `drwav_read_pcm_frames_f32` into `out` (interleaved); returns the number
/// of samples written (dr_wav returns frames, but its 32-bit float fast path
/// also writes the complete samples of a trailing partial frame).
fn read_f32(dec: &mut Decoder, frames: u64, out: &mut [f32], raw: &mut [u8]) -> usize {
    let ch = dec.channels as usize;
    let bpf = dec.bytes_per_frame() as usize;
    if bpf == 0 {
        return 0;
    }
    if dec.format == FORMAT_IEEE_FLOAT && dec.bits_per_sample == 32 {
        let bytes = &mut raw[..frames as usize * bpf];
        let n = dec.read_raw(bytes);
        let samples = n / 4;
        ieee_to_f32(out, bytes, samples, 4);
        return samples;
    }
    let mut sample_data = [0u8; 4096];
    let mut to_read = frames;
    let mut written = 0usize;
    while to_read > 0 {
        let req = to_read.min((sample_data.len() / bpf) as u64) as usize;
        if req == 0 {
            break;
        }
        let n = dec.read_raw(&mut sample_data[..req * bpf]);
        let frames_read = n / bpf;
        if frames_read == 0 {
            break;
        }
        let count = frames_read * ch;
        let dst = &mut out[written..written + count];
        match dec.format {
            FORMAT_PCM => pcm_to_f32(dst, &sample_data, count, bpf / ch),
            FORMAT_IEEE_FLOAT => ieee_to_f32(dst, &sample_data, count, bpf / ch),
            FORMAT_ALAW => {
                for (o, &b) in dst.iter_mut().zip(&sample_data[..count]) {
                    *o = alaw_to_f32(b);
                }
            }
            FORMAT_MULAW => {
                for (o, &b) in dst.iter_mut().zip(&sample_data[..count]) {
                    *o = mulaw_to_f32(b);
                }
            }
            _ => return written,
        }
        written += count;
        to_read -= frames_read as u64;
    }
    written
}

/// What SoLoud's `Wav::loadwav` leaves in `mData` for a RIFF file.
pub fn decode(bytes: &[u8]) -> Result<WavData, WavLoadError> {
    let mut dec = init(bytes)?;
    if !matches!(
        dec.format,
        FORMAT_PCM | FORMAT_IEEE_FLOAT | FORMAT_ALAW | FORMAT_MULAW
    ) {
        return Err(WavLoadError::Unsupported(dec.format));
    }
    if dec.channels > 8 {
        return Err(WavLoadError::TooManyChannels(dec.channels));
    }
    let samples = dec.total_frames;
    if samples == 0 {
        return Err(WavLoadError::Empty);
    }
    let ch = dec.channels as usize;
    // SoLoud stores the count in an unsigned int and allocates it up front;
    // a header claiming far more than the file can hold is refused here.
    if samples * ch as u64 > u32::MAX as u64 || samples > (bytes.len() as u64).saturating_mul(8) {
        return Err(WavLoadError::Invalid);
    }
    let sample_count = samples as usize;
    let mut data = vec![0.0f32; sample_count * ch];
    let mut tmp = [0.0f32; 512 * 8];
    let mut raw = vec![0u8; 512 * 8 * 4];
    let mut i = 0usize;
    while i < sample_count {
        let block = (sample_count - i).min(512);
        let got = read_f32(&mut dec, block as u64, &mut tmp, &mut raw);
        tmp[got.min(block * ch)..block * ch].fill(0.0);
        for j in 0..block {
            for k in 0..ch {
                data[k * sample_count + i + j] = tmp[j * ch + k];
            }
        }
        i += 512;
    }
    Ok(WavData {
        channels: dec.channels as u32,
        sample_rate: dec.sample_rate,
        sample_count: sample_count as u32,
        data,
    })
}

/// Errors of [`load_viewer_buffer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferLoadError {
    Wav(WavLoadError),
    Source(crate::mixer::WavError),
}

impl std::fmt::Display for BufferLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BufferLoadError::Wav(e) => e.fmt(f),
            BufferLoadError::Source(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for BufferLoadError {}

/// `LLAudioBufferSoLoud::loadWAV` on file contents: decode, then the
/// adapter's source defaults and 64-sample head/tail fade.
pub fn load_viewer_buffer(bytes: &[u8]) -> Result<Arc<Wav>, BufferLoadError> {
    let d = decode(bytes).map_err(BufferLoadError::Wav)?;
    let mut wav = Wav::from_planar(d.data, d.sample_rate as f32, d.channels)
        .map_err(BufferLoadError::Source)?;
    wav.apply_viewer_defaults();
    Ok(Arc::new(wav))
}

#[rustfmt::skip]
const ALAW_TABLE: [u16; 256] = [
    0xEA80, 0xEB80, 0xE880, 0xE980, 0xEE80, 0xEF80, 0xEC80, 0xED80, 0xE280, 0xE380, 0xE080, 0xE180, 0xE680, 0xE780, 0xE480, 0xE580,
    0xF540, 0xF5C0, 0xF440, 0xF4C0, 0xF740, 0xF7C0, 0xF640, 0xF6C0, 0xF140, 0xF1C0, 0xF040, 0xF0C0, 0xF340, 0xF3C0, 0xF240, 0xF2C0,
    0xAA00, 0xAE00, 0xA200, 0xA600, 0xBA00, 0xBE00, 0xB200, 0xB600, 0x8A00, 0x8E00, 0x8200, 0x8600, 0x9A00, 0x9E00, 0x9200, 0x9600,
    0xD500, 0xD700, 0xD100, 0xD300, 0xDD00, 0xDF00, 0xD900, 0xDB00, 0xC500, 0xC700, 0xC100, 0xC300, 0xCD00, 0xCF00, 0xC900, 0xCB00,
    0xFEA8, 0xFEB8, 0xFE88, 0xFE98, 0xFEE8, 0xFEF8, 0xFEC8, 0xFED8, 0xFE28, 0xFE38, 0xFE08, 0xFE18, 0xFE68, 0xFE78, 0xFE48, 0xFE58,
    0xFFA8, 0xFFB8, 0xFF88, 0xFF98, 0xFFE8, 0xFFF8, 0xFFC8, 0xFFD8, 0xFF28, 0xFF38, 0xFF08, 0xFF18, 0xFF68, 0xFF78, 0xFF48, 0xFF58,
    0xFAA0, 0xFAE0, 0xFA20, 0xFA60, 0xFBA0, 0xFBE0, 0xFB20, 0xFB60, 0xF8A0, 0xF8E0, 0xF820, 0xF860, 0xF9A0, 0xF9E0, 0xF920, 0xF960,
    0xFD50, 0xFD70, 0xFD10, 0xFD30, 0xFDD0, 0xFDF0, 0xFD90, 0xFDB0, 0xFC50, 0xFC70, 0xFC10, 0xFC30, 0xFCD0, 0xFCF0, 0xFC90, 0xFCB0,
    0x1580, 0x1480, 0x1780, 0x1680, 0x1180, 0x1080, 0x1380, 0x1280, 0x1D80, 0x1C80, 0x1F80, 0x1E80, 0x1980, 0x1880, 0x1B80, 0x1A80,
    0x0AC0, 0x0A40, 0x0BC0, 0x0B40, 0x08C0, 0x0840, 0x09C0, 0x0940, 0x0EC0, 0x0E40, 0x0FC0, 0x0F40, 0x0CC0, 0x0C40, 0x0DC0, 0x0D40,
    0x5600, 0x5200, 0x5E00, 0x5A00, 0x4600, 0x4200, 0x4E00, 0x4A00, 0x7600, 0x7200, 0x7E00, 0x7A00, 0x6600, 0x6200, 0x6E00, 0x6A00,
    0x2B00, 0x2900, 0x2F00, 0x2D00, 0x2300, 0x2100, 0x2700, 0x2500, 0x3B00, 0x3900, 0x3F00, 0x3D00, 0x3300, 0x3100, 0x3700, 0x3500,
    0x0158, 0x0148, 0x0178, 0x0168, 0x0118, 0x0108, 0x0138, 0x0128, 0x01D8, 0x01C8, 0x01F8, 0x01E8, 0x0198, 0x0188, 0x01B8, 0x01A8,
    0x0058, 0x0048, 0x0078, 0x0068, 0x0018, 0x0008, 0x0038, 0x0028, 0x00D8, 0x00C8, 0x00F8, 0x00E8, 0x0098, 0x0088, 0x00B8, 0x00A8,
    0x0560, 0x0520, 0x05E0, 0x05A0, 0x0460, 0x0420, 0x04E0, 0x04A0, 0x0760, 0x0720, 0x07E0, 0x07A0, 0x0660, 0x0620, 0x06E0, 0x06A0,
    0x02B0, 0x0290, 0x02F0, 0x02D0, 0x0230, 0x0210, 0x0270, 0x0250, 0x03B0, 0x0390, 0x03F0, 0x03D0, 0x0330, 0x0310, 0x0370, 0x0350,
];

#[rustfmt::skip]
const MULAW_TABLE: [u16; 256] = [
    0x8284, 0x8684, 0x8A84, 0x8E84, 0x9284, 0x9684, 0x9A84, 0x9E84, 0xA284, 0xA684, 0xAA84, 0xAE84, 0xB284, 0xB684, 0xBA84, 0xBE84,
    0xC184, 0xC384, 0xC584, 0xC784, 0xC984, 0xCB84, 0xCD84, 0xCF84, 0xD184, 0xD384, 0xD584, 0xD784, 0xD984, 0xDB84, 0xDD84, 0xDF84,
    0xE104, 0xE204, 0xE304, 0xE404, 0xE504, 0xE604, 0xE704, 0xE804, 0xE904, 0xEA04, 0xEB04, 0xEC04, 0xED04, 0xEE04, 0xEF04, 0xF004,
    0xF0C4, 0xF144, 0xF1C4, 0xF244, 0xF2C4, 0xF344, 0xF3C4, 0xF444, 0xF4C4, 0xF544, 0xF5C4, 0xF644, 0xF6C4, 0xF744, 0xF7C4, 0xF844,
    0xF8A4, 0xF8E4, 0xF924, 0xF964, 0xF9A4, 0xF9E4, 0xFA24, 0xFA64, 0xFAA4, 0xFAE4, 0xFB24, 0xFB64, 0xFBA4, 0xFBE4, 0xFC24, 0xFC64,
    0xFC94, 0xFCB4, 0xFCD4, 0xFCF4, 0xFD14, 0xFD34, 0xFD54, 0xFD74, 0xFD94, 0xFDB4, 0xFDD4, 0xFDF4, 0xFE14, 0xFE34, 0xFE54, 0xFE74,
    0xFE8C, 0xFE9C, 0xFEAC, 0xFEBC, 0xFECC, 0xFEDC, 0xFEEC, 0xFEFC, 0xFF0C, 0xFF1C, 0xFF2C, 0xFF3C, 0xFF4C, 0xFF5C, 0xFF6C, 0xFF7C,
    0xFF88, 0xFF90, 0xFF98, 0xFFA0, 0xFFA8, 0xFFB0, 0xFFB8, 0xFFC0, 0xFFC8, 0xFFD0, 0xFFD8, 0xFFE0, 0xFFE8, 0xFFF0, 0xFFF8, 0x0000,
    0x7D7C, 0x797C, 0x757C, 0x717C, 0x6D7C, 0x697C, 0x657C, 0x617C, 0x5D7C, 0x597C, 0x557C, 0x517C, 0x4D7C, 0x497C, 0x457C, 0x417C,
    0x3E7C, 0x3C7C, 0x3A7C, 0x387C, 0x367C, 0x347C, 0x327C, 0x307C, 0x2E7C, 0x2C7C, 0x2A7C, 0x287C, 0x267C, 0x247C, 0x227C, 0x207C,
    0x1EFC, 0x1DFC, 0x1CFC, 0x1BFC, 0x1AFC, 0x19FC, 0x18FC, 0x17FC, 0x16FC, 0x15FC, 0x14FC, 0x13FC, 0x12FC, 0x11FC, 0x10FC, 0x0FFC,
    0x0F3C, 0x0EBC, 0x0E3C, 0x0DBC, 0x0D3C, 0x0CBC, 0x0C3C, 0x0BBC, 0x0B3C, 0x0ABC, 0x0A3C, 0x09BC, 0x093C, 0x08BC, 0x083C, 0x07BC,
    0x075C, 0x071C, 0x06DC, 0x069C, 0x065C, 0x061C, 0x05DC, 0x059C, 0x055C, 0x051C, 0x04DC, 0x049C, 0x045C, 0x041C, 0x03DC, 0x039C,
    0x036C, 0x034C, 0x032C, 0x030C, 0x02EC, 0x02CC, 0x02AC, 0x028C, 0x026C, 0x024C, 0x022C, 0x020C, 0x01EC, 0x01CC, 0x01AC, 0x018C,
    0x0174, 0x0164, 0x0154, 0x0144, 0x0134, 0x0124, 0x0114, 0x0104, 0x00F4, 0x00E4, 0x00D4, 0x00C4, 0x00B4, 0x00A4, 0x0094, 0x0084,
    0x0078, 0x0070, 0x0068, 0x0060, 0x0058, 0x0050, 0x0048, 0x0040, 0x0038, 0x0030, 0x0028, 0x0020, 0x0018, 0x0010, 0x0008, 0x0000,
];
