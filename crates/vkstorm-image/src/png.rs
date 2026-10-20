//! PNG, the port of `LLImagePNG` and `LLPngWrapper`.
//!
//! Decoding reproduces the libpng read transformations the viewer requested:
//! palettes expanded to RGB, grey below 8 bits expanded to 8, grey converted
//! to RGB (so every decode yields 3 or 4 components), tRNS converted to an
//! alpha channel, 16-bit samples reduced to their high byte, and gamma
//! correction to a 2.2 screen whenever the file gamma makes it significant
//! (libpng's 5% threshold), computed with libpng's fixed-point arithmetic and
//! its 8 and 16-bit tables. Chunk parsing, unfiltering and inflate come from
//! the `png` crate; the transformations are implemented here so that the
//! order and the rounding match libpng exactly.
//!
//! Encoding writes what the viewer's libpng wrote: 8-bit samples, the colour
//! type matching the component count, libpng's minimum-sum-of-absolute-
//! differences filter choice per row, deflate at libpng's settings (level 6,
//! filtered strategy, window reduced for small images) through zlib-rs, the
//! Rust port of the zlib-ng the viewer links, and IDAT chunks of 8192 bytes.

use std::io::Cursor;

use png::{ColorType, Decoder, Limits, Transformations};
use zlib_rs::{DeflateConfig, Method, Strategy};
use zlib_rs::{Deflate, DeflateFlush, Status};

use crate::{check_dimensions, fail, Error, Header, Raster};

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// `LLPngWrapper::isValidPng`: the 8-byte signature.
pub fn is_valid(data: &[u8]) -> bool {
    data.len() >= 8 && data[..8] == SIGNATURE
}

// ---------------------------------------------------------------------------
// Gamma, after libpng's fixed-point rules (PNG_FP_1 = 100000).
// ---------------------------------------------------------------------------

const PNG_FP_1: i64 = 100_000;
/// `PNG_GAMMA_THRESHOLD_FIXED`: 5%.
const GAMMA_THRESHOLD_FIXED: i64 = 5_000;
/// The viewer's display gamma.
pub const SCREEN_GAMMA_FIXED: i64 = 220_000;
/// `PNG_MAX_GAMMA_8`: 16-bit gamma tables keep at most 11 significant bits.
const MAX_GAMMA_8: u32 = 11;

fn gamma_significant(gamma_val: i64) -> bool {
    gamma_val < PNG_FP_1 - GAMMA_THRESHOLD_FIXED || gamma_val > PNG_FP_1 + GAMMA_THRESHOLD_FIXED
}

/// `png_muldiv` in its floating-point form.
fn muldiv(a: i64, times: i64, divisor: i64) -> i64 {
    let mut r = a as f64;
    r *= times as f64;
    r /= divisor as f64;
    (r + 0.5).floor() as i64
}

/// `png_reciprocal2`: 1/(a*b) in fixed point.
fn reciprocal2(a: i64, b: i64) -> i64 {
    let mut r = 1e15 / a as f64;
    r /= b as f64;
    (r + 0.5).floor() as i64
}

/// The file gamma libpng ends up with: the gAMA chunk value, else the
/// reciprocal of the screen gamma (`png_set_gamma(2.2, 1/2.2)`).
fn file_gamma_fixed(gama_chunk: Option<u32>) -> i64 {
    match gama_chunk {
        Some(g) => g as i64,
        None => (1.0f64 / 2.2 * 100_000.0 + 0.5).floor() as i64,
    }
}

/// Whether libpng would apply gamma correction (`png_gamma_threshold`).
fn gamma_correction_needed(file_gamma: i64) -> bool {
    gamma_significant(muldiv(SCREEN_GAMMA_FIXED, file_gamma, PNG_FP_1))
}

/// `png_build_8bit_table`.
fn gamma_table8(gamma_val: i64) -> [u8; 256] {
    let mut table = [0u8; 256];
    for (i, t) in table.iter_mut().enumerate() {
        *t = i as u8;
    }
    if gamma_significant(gamma_val) {
        let g = gamma_val as f64 * 0.00001;
        for (i, t) in table.iter_mut().enumerate().take(255).skip(1) {
            *t = (255.0 * (i as f64 / 255.0).powf(g) + 0.5).floor() as u8;
        }
    }
    table
}

/// `png_product2`: a*b in fixed point.
fn product2(a: i64, b: i64) -> i64 {
    let mut r = a as f64;
    r *= b as f64;
    r /= 100_000.0;
    (r + 0.5).floor() as i64
}

/// `png_gamma_16bit_correct`.
fn gamma_16bit_correct(value: u32, gamma_val: i64) -> u32 {
    if value > 0 && value < 65535 {
        (65535.0 * (value as f64 / 65535.0).powf(gamma_val as f64 * 0.00001) + 0.5).floor() as u32
    } else {
        value
    }
}

/// `png_build_16to8_table`, flattened over the input value shifted right by
/// `shift`: the 8-bit output libpng stores (as `out * 257`) when 16-bit data
/// is gamma corrected and then reduced to 8 bits. `gamma_val` is the
/// *inverse* of the overall correction (file gamma times screen gamma): the
/// table is filled by finding, for each output value, the input value at the
/// boundary to the next one.
fn gamma_table16to8(shift: u32, gamma_val: i64) -> Vec<u8> {
    let max = (1u64 << (16 - shift)) - 1;
    let size = 1usize << (16 - shift);
    let mut table = vec![255u8; size];
    let mut last = 0usize;
    for i in 0..255u32 {
        let out = i * 257;
        let bound = gamma_16bit_correct(out + 128, gamma_val) as u64;
        let bound = ((bound * max + 32768) / 65535 + 1) as usize;
        while last < bound && last < size {
            table[last] = i as u8;
            last += 1;
        }
    }
    table
}

/// `png_build_gamma_table`'s shift for 16-bit data that will be reduced to 8
/// bits: from the significant bits (sBIT), at least 16 - PNG_MAX_GAMMA_8,
/// at most 8.
fn gamma_shift16(sig_bit: u32) -> u32 {
    let mut shift = if sig_bit > 0 && sig_bit < 16 { 16 - sig_bit } else { 0 };
    if shift < 16 - MAX_GAMMA_8 {
        shift = 16 - MAX_GAMMA_8;
    }
    shift.min(8)
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

fn decoder(data: &[u8]) -> Decoder<Cursor<&[u8]>> {
    let mut d = Decoder::new_with_limits(Cursor::new(data), Limits { bytes: usize::MAX });
    d.set_transformations(Transformations::IDENTITY);
    d.set_ignore_text_chunk(true);
    d
}

fn png_error(e: png::DecodingError) -> Error {
    Error::new(e.to_string())
}

/// Components after the viewer's transformations.
fn output_components(color_type: ColorType, has_trns: bool) -> u32 {
    match color_type {
        ColorType::Grayscale | ColorType::Rgb | ColorType::Indexed => 3 + has_trns as u32,
        ColorType::GrayscaleAlpha | ColorType::Rgba => 4,
    }
}

/// `LLImagePNG::updateData`: dimensions and the decoded component count
/// from the chunks before the image data.
pub fn header(data: &[u8]) -> Result<Header, Error> {
    if !is_valid(data) {
        return fail("Invalid or corrupt PNG file");
    }
    let reader = decoder(data).read_info().map_err(png_error)?;
    let info = reader.info();
    let (width, height) = check_dimensions(info.width as i64, info.height as i64, "LLImagePNG")?;
    Ok(Header { width, height, components: output_components(info.color_type, info.trns.is_some()) })
}

/// `LLImagePNG::decode`: the image as the viewer's libpng produced it.
pub fn decode(data: &[u8]) -> Result<Raster, Error> {
    if !is_valid(data) {
        return fail("Invalid or corrupt PNG file");
    }
    let mut reader = decoder(data).read_info().map_err(png_error)?;
    let info = reader.info().clone();
    let (width, height) = check_dimensions(info.width as i64, info.height as i64, "LLImagePNG")?;
    let Some(size) = reader.output_buffer_size() else {
        return fail("LLPngWrapper");
    };
    let mut raw = vec![0u8; size];
    let out = reader.next_frame(&mut raw).map_err(png_error)?;
    reader.finish().map_err(png_error)?;
    let raw = &raw[..out.buffer_size()];

    let bit_depth = info.bit_depth as u32;
    let color_type = info.color_type;
    let trns = info.trns.as_deref();
    let has_trns = trns.is_some();
    let components = output_components(color_type, has_trns);
    let mut raster = Raster::new(width, height, components);

    // Gamma: libpng corrects 16-bit data before reducing it to 8 bits, so the
    // tables are built for the file's depth.
    let file_gamma = file_gamma_fixed(info.gama_chunk.map(|g| g.into_scaled()));
    let correct = gamma_correction_needed(file_gamma);
    let table8 = if correct && bit_depth <= 8 { Some(gamma_table8(reciprocal2(file_gamma, SCREEN_GAMMA_FIXED))) } else { None };
    let (table16, shift16) = if correct && bit_depth == 16 {
        let sig_bit = match (&info.sbit, color_type) {
            (Some(s), ColorType::Grayscale | ColorType::GrayscaleAlpha) => s.first().copied().unwrap_or(0) as u32,
            (Some(s), _) => s.iter().take(3).copied().max().unwrap_or(0) as u32,
            (None, _) => 0,
        };
        let shift = gamma_shift16(sig_bit);
        (Some(gamma_table16to8(shift, product2(file_gamma, SCREEN_GAMMA_FIXED))), shift)
    } else {
        (None, 0)
    };

    let w = width as usize;
    let h = height as usize;
    let line = out.line_size;
    let dst_row = raster.row_bytes();
    let c = components as usize;
    for y in 0..h {
        let src = &raw[y * line..y * line + line];
        // Rows are stored bottom-up.
        let dst = &mut raster.data[(h - 1 - y) * dst_row..(h - y) * dst_row];
        match color_type {
            ColorType::Indexed => {
                let palette = info.palette.as_deref().unwrap_or(&[]);
                let trns = trns.unwrap_or(&[]);
                for (x, px) in dst.chunks_exact_mut(c).enumerate() {
                    let index = unpack(src, x, bit_depth) as usize;
                    let (r, g, b) = match palette.get(3 * index..3 * index + 3) {
                        Some(p) => (p[0], p[1], p[2]),
                        None => (0, 0, 0),
                    };
                    px[0] = r;
                    px[1] = g;
                    px[2] = b;
                    if has_trns {
                        px[3] = trns.get(index).copied().unwrap_or(255);
                    }
                }
            }
            ColorType::Grayscale if bit_depth < 8 => {
                let scale = 255 / ((1u16 << bit_depth) - 1);
                let trans = trns.map(|t| (t[0] as u16 & ((1u16 << bit_depth) - 1)) * scale);
                for (x, px) in dst.chunks_exact_mut(c).enumerate() {
                    let v = unpack(src, x, bit_depth) as u16 * scale;
                    px[0] = v as u8;
                    px[1] = v as u8;
                    px[2] = v as u8;
                    if let Some(t) = trans {
                        px[3] = if v == t { 0 } else { 255 };
                    }
                }
            }
            ColorType::Grayscale if bit_depth == 8 => {
                let trans = trns.map(|t| t[0]);
                for (px, &v) in dst.chunks_exact_mut(c).zip(src) {
                    px[0] = v;
                    px[1] = v;
                    px[2] = v;
                    if let Some(t) = trans {
                        px[3] = if v == t { 0 } else { 255 };
                    }
                }
            }
            ColorType::Grayscale => {
                let trans = trns.map(|t| u16::from_be_bytes([t[0], t[1]]));
                for (px, s) in dst.chunks_exact_mut(c).zip(src.chunks_exact(2)) {
                    let v = u16::from_be_bytes([s[0], s[1]]);
                    let g = reduce16(v, table16.as_deref(), shift16);
                    px[0] = g;
                    px[1] = g;
                    px[2] = g;
                    if let Some(t) = trans {
                        px[3] = if v == t { 0 } else { 255 };
                    }
                }
            }
            ColorType::GrayscaleAlpha if bit_depth == 8 => {
                for (px, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(2)) {
                    px[0] = s[0];
                    px[1] = s[0];
                    px[2] = s[0];
                    px[3] = s[1];
                }
            }
            ColorType::GrayscaleAlpha => {
                for (px, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                    let g = reduce16(u16::from_be_bytes([s[0], s[1]]), table16.as_deref(), shift16);
                    px[0] = g;
                    px[1] = g;
                    px[2] = g;
                    px[3] = s[2];
                }
            }
            ColorType::Rgb if bit_depth == 8 => {
                let trans = trns.map(|t| [t[0], t[1], t[2]]);
                for (px, s) in dst.chunks_exact_mut(c).zip(src.chunks_exact(3)) {
                    px[..3].copy_from_slice(s);
                    if let Some(t) = trans {
                        px[3] = if s == t { 0 } else { 255 };
                    }
                }
            }
            ColorType::Rgb => {
                let trans = trns.map(|t| [t[0], t[1], t[2], t[3], t[4], t[5]]);
                for (px, s) in dst.chunks_exact_mut(c).zip(src.chunks_exact(6)) {
                    for k in 0..3 {
                        px[k] = reduce16(u16::from_be_bytes([s[2 * k], s[2 * k + 1]]), table16.as_deref(), shift16);
                    }
                    if let Some(t) = trans {
                        px[3] = if s == t { 0 } else { 255 };
                    }
                }
            }
            ColorType::Rgba if bit_depth == 8 => dst.copy_from_slice(&src[..w * 4]),
            ColorType::Rgba => {
                for (px, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(8)) {
                    for k in 0..3 {
                        px[k] = reduce16(u16::from_be_bytes([s[2 * k], s[2 * k + 1]]), table16.as_deref(), shift16);
                    }
                    px[3] = s[6];
                }
            }
        }
        if let Some(t) = &table8 {
            // Colour channels only; alpha is never gamma corrected.
            for px in dst.chunks_exact_mut(c) {
                for v in &mut px[..3] {
                    *v = t[*v as usize];
                }
            }
        }
    }
    Ok(raster)
}

/// Sample `x` of a packed row at 1, 2, 4 or 8 bits (most significant first).
#[inline]
fn unpack(row: &[u8], x: usize, bit_depth: u32) -> u8 {
    match bit_depth {
        8 => row[x],
        _ => {
            let per_byte = 8 / bit_depth as usize;
            let byte = row[x / per_byte];
            let shift = 8 - bit_depth as usize * (x % per_byte + 1);
            (byte >> shift) & ((1u8 << bit_depth) - 1)
        }
    }
}

/// A 16-bit colour sample to 8 bits: through the 16-to-8 gamma table when
/// correcting, else the high byte (libpng's strip).
#[inline]
fn reduce16(v: u16, table: Option<&[u8]>, shift: u32) -> u8 {
    match table {
        Some(t) => t[(v >> shift) as usize],
        None => (v >> 8) as u8,
    }
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// libpng's IDAT buffer size.
const ZBUF_SIZE: usize = 8192;

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let crc = zlib_rs::crc32::crc32(0, &out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// `png_write_find_filter`'s metric: bytes as signed distances from zero.
#[inline]
fn filter_sum(row: &[u8]) -> u64 {
    row.iter().map(|&v| (v as i8).unsigned_abs() as u64).sum()
}

/// Choose and apply the filter for one row the way libpng does: the sum of
/// absolute differences for none, sub, up, average and Paeth, the first
/// strictly smallest wins. `out` receives the filter byte and the row.
fn filter_row(row: &[u8], prev: &[u8], bpp: usize, out: &mut Vec<u8>, scratch: &mut Vec<u8>) {
    let n = row.len();
    let mut best = 0u8;
    let mut mins = filter_sum(row);
    scratch.clear();
    scratch.resize(4 * n, 0);
    let (sub, rest) = scratch.split_at_mut(n);
    let (up, rest) = rest.split_at_mut(n);
    let (avg, paeth) = rest.split_at_mut(n);
    for i in 0..n {
        let a = if i >= bpp { row[i - bpp] } else { 0 };
        let b = prev[i];
        let c = if i >= bpp { prev[i - bpp] } else { 0 };
        let x = row[i];
        sub[i] = x.wrapping_sub(a);
        up[i] = x.wrapping_sub(b);
        avg[i] = x.wrapping_sub(((a as u16 + b as u16) >> 1) as u8);
        let p = a as i16 + b as i16 - c as i16;
        let pa = (p - a as i16).abs();
        let pb = (p - b as i16).abs();
        let pc = (p - c as i16).abs();
        let pred = if pa <= pb && pa <= pc {
            a
        } else if pb <= pc {
            b
        } else {
            c
        };
        paeth[i] = x.wrapping_sub(pred);
    }
    for (k, candidate) in [(1u8, &*sub), (2, &*up), (3, &*avg), (4, &*paeth)] {
        let sum = filter_sum(candidate);
        if sum < mins {
            mins = sum;
            best = k;
        }
    }
    out.push(best);
    match best {
        0 => out.extend_from_slice(row),
        1 => out.extend_from_slice(sub),
        2 => out.extend_from_slice(up),
        3 => out.extend_from_slice(avg),
        _ => out.extend_from_slice(paeth),
    }
}

/// `png_deflate_claim`: the window libpng asks for, reduced for small images
/// to the smallest of 8 to 15 bits that holds the whole image data (the rule
/// the viewer's libpng 1.6.44 applies, fitted against its output for every
/// fixture size; zlib-ng keeps a request of 8 bits as 8).
fn window_bits(data_size: usize) -> i32 {
    let mut window_bits = 15;
    if data_size <= 16384 {
        let mut half_window_size = 1usize << (window_bits - 1);
        while data_size <= half_window_size && window_bits > 8 {
            half_window_size >>= 1;
            window_bits -= 1;
        }
    }
    window_bits
}

/// `LLPngWrapper::writePng`: encode a 1 to 4 component raster.
pub fn encode(raster: &Raster) -> Result<Vec<u8>, Error> {
    let color_type: u8 = match raster.components {
        1 => 0,
        2 => 4,
        3 => 2,
        4 => 6,
        _ => return fail("Unsupported image: unexpected number of channels"),
    };
    let w = raster.width as usize;
    let h = raster.height as usize;
    let bpp = raster.components as usize;
    let row_bytes = w * bpp;
    if raster.data.len() < row_bytes * h {
        return fail("LLPngWrapper: raster is shorter than its dimensions");
    }
    if w == 0 || h == 0 {
        return fail("Image width or height is zero in IHDR");
    }

    let mut out = Vec::with_capacity(row_bytes * h / 2 + 1024);
    out.extend_from_slice(&SIGNATURE);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&raster.width.to_be_bytes());
    ihdr.extend_from_slice(&raster.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, color_type, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);

    // Filter every row (top-down from the bottom-up raster) into one buffer.
    let mut filtered = Vec::with_capacity((row_bytes + 1) * h);
    let mut scratch = Vec::new();
    let zero = vec![0u8; row_bytes];
    let mut prev: &[u8] = &zero;
    for y in 0..h {
        let row = &raster.data[(h - 1 - y) * row_bytes..(h - y) * row_bytes];
        filter_row(row, prev, bpp, &mut filtered, &mut scratch);
        prev = row;
    }

    // Deflate at libpng's defaults, then split the stream into IDAT chunks
    // as libpng's 8192-byte buffer would have.
    let wbits = window_bits(filtered.len());
    let config = DeflateConfig {
        level: 6,
        method: Method::Deflated,
        window_bits: wbits,
        mem_level: 8,
        strategy: Strategy::Filtered,
    };
    let mut deflate = Deflate::new_with_config(config);
    let mut stream = vec![0u8; filtered.len() / 2 + 1024];
    let mut consumed = 0usize;
    let mut produced = 0usize;
    loop {
        if produced == stream.len() {
            stream.resize(stream.len() * 2, 0);
        }
        let before_in = deflate.total_in() as usize;
        let before_out = deflate.total_out() as usize;
        let status = deflate
            .compress(&filtered[consumed..], &mut stream[produced..], DeflateFlush::Finish)
            .map_err(|e| Error::new(e.as_str()))?;
        consumed += deflate.total_in() as usize - before_in;
        produced += deflate.total_out() as usize - before_out;
        match status {
            Status::StreamEnd => break,
            Status::Ok => {}
            Status::BufError => return fail("Deflate error"),
        }
    }
    stream.truncate(produced);
    if wbits == 8 && stream.len() >= 2 {
        // zlib-ng honours an 8-bit window and says so in the header; zlib-rs
        // widens the request to 9 bits (zlib's 256-byte window workaround).
        // The compressed data of an image that small is the same, so restore
        // the header zlib-ng wrote: CINFO 0 and a matching check value.
        let cmf = 0x08u16;
        let flg = (stream[1] & 0xe0) as u16;
        let check = 31 - ((cmf << 8) | flg) % 31;
        stream[0] = cmf as u8;
        stream[1] = (flg | check % 31) as u8;
    }
    for piece in stream.chunks(ZBUF_SIZE) {
        chunk(&mut out, b"IDAT", piece);
    }
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamma_threshold_matches_libpng() {
        // sRGB-like file gammas are within 5% of the screen: no correction.
        assert!(!gamma_correction_needed(45455));
        assert!(!gamma_correction_needed(45000));
        assert!(!gamma_correction_needed(file_gamma_fixed(None)));
        // Linear files and the 0.55531 some skin textures carry are corrected.
        assert!(gamma_correction_needed(100000));
        assert!(gamma_correction_needed(55531));
        assert_eq!(reciprocal2(100000, 220000), 45455);
    }

    #[test]
    fn window_bits_shrink_for_small_images() {
        // Values observed from the viewer's libpng + zlib-ng (CINFO of the IDAT stream).
        assert_eq!(window_bits(1 << 20), 15);
        assert_eq!(window_bits(16448), 15);
        assert_eq!(window_bits(16384), 14);
        assert_eq!(window_bits(8256), 14);
        assert_eq!(window_bits(4160), 13);
        assert_eq!(window_bits(578), 10);
        assert_eq!(window_bits(392), 9);
        assert_eq!(window_bits(145), 8);
        assert_eq!(window_bits(2), 8);
    }

    #[test]
    fn unpack_bits() {
        assert_eq!(unpack(&[0b1010_0000], 0, 1), 1);
        assert_eq!(unpack(&[0b1010_0000], 1, 1), 0);
        assert_eq!(unpack(&[0b1011_0000], 1, 2), 3);
        assert_eq!(unpack(&[0x1f], 1, 4), 0xf);
    }
}
