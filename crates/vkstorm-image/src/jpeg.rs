//! JPEG, the port of `LLImageJPEG` and the JPEG case of
//! `LLImageDimensionsInfo`.
//!
//! Decoding always yields RGB (3 components), as the viewer's libjpeg-turbo
//! setup did (`out_color_space = JCS_RGB`); the decoder is zune-jpeg with its
//! x86 SIMD paths. Encoding is baseline sequential with libjpeg's defaults:
//! quality-scaled standard quantisation tables, 2x2 chroma subsampling with
//! box averaging, the standard Huffman tables and a JFIF header; the encoder
//! is jpeg-encoder with its AVX2 paths.

use jpeg_encoder::{ChromaSubsamplingMethod, ColorType, Encoder, SamplingFactor};
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use crate::{check_dimensions, fail, Error, Header, Raster};

/// The result of scanning a prefix of a file for its frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimensions {
    /// The frame header was found.
    Found { width: u32, height: u32 },
    /// The data ended before the frame header; more of the file is needed.
    Truncated,
}

/// Read the image dimensions from the frame header (SOF), scanning the
/// markers the way libjpeg's `jpeg_read_header` does and without decoding
/// anything. `Err` carries libjpeg's message for input that is not a JPEG or
/// uses a process libjpeg-turbo would refuse.
pub fn dimensions(data: &[u8]) -> Result<Dimensions, Error> {
    if data.len() < 2 {
        return Ok(Dimensions::Truncated);
    }
    if data[0] != 0xff || data[1] != 0xd8 {
        return fail(format!("Not a JPEG file: starts with 0x{:02x} 0x{:02x}", data[0], data[1]));
    }
    let mut pos = 2usize;
    loop {
        // Find the next marker, skipping fill bytes.
        let Some(&b) = data.get(pos) else { return Ok(Dimensions::Truncated) };
        if b != 0xff {
            return fail(format!("Bogus marker length"));
        }
        pos += 1;
        let marker = loop {
            let Some(&m) = data.get(pos) else { return Ok(Dimensions::Truncated) };
            pos += 1;
            if m != 0xff {
                break m;
            }
        };
        match marker {
            // Standalone markers.
            0xd8 | 0x01 | 0xd0..=0xd7 => continue,
            0xd9 => return fail("Premature end of JPEG file"),
            0xc0 | 0xc1 | 0xc2 | 0xc3 | 0xc9 | 0xca | 0xcb => {
                let Some(seg) = data.get(pos..pos + 7) else { return Ok(Dimensions::Truncated) };
                let height = u16::from_be_bytes([seg[3], seg[4]]) as u32;
                let width = u16::from_be_bytes([seg[5], seg[6]]) as u32;
                if width == 0 || height == 0 {
                    return fail("Empty JPEG image (DNL not supported)");
                }
                return Ok(Dimensions::Found { width, height });
            }
            0xc5..=0xc7 | 0xcd..=0xcf => {
                return fail(format!("Unsupported JPEG process: SOF type 0x{marker:02x}"));
            }
            _ => {
                let Some(len) = data.get(pos..pos + 2) else { return Ok(Dimensions::Truncated) };
                let len = u16::from_be_bytes([len[0], len[1]]) as usize;
                if len < 2 {
                    return fail("Bogus marker length");
                }
                pos += len;
            }
        }
    }
}

/// `LLImageJPEG::updateData`: dimensions from the frame header; the viewer
/// always decodes to 3 components.
pub fn header(data: &[u8]) -> Result<Header, Error> {
    if data.is_empty() {
        return fail("Uninitialized instance of LLImageJPEG");
    }
    match dimensions(data)? {
        Dimensions::Found { width, height } => {
            let (width, height) = check_dimensions(width as i64, height as i64, "LLImageJPEG")?;
            Ok(Header { width, height, components: 3 })
        }
        Dimensions::Truncated => fail("Premature end of JPEG file"),
    }
}

fn options() -> DecoderOptions {
    DecoderOptions::new_fast()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(u16::MAX as usize)
        .set_max_height(u16::MAX as usize)
        .set_strict_mode(true)
}

/// `LLImageJPEG::decode`: RGB, rows bottom-up. Corrupt or truncated data is
/// an error, as the viewer treated every libjpeg warning as a failed decode.
pub fn decode(data: &[u8]) -> Result<Raster, Error> {
    if data.is_empty() {
        return fail("LLImageJPEG trying to decode an image with no data!");
    }
    let mut decoder = JpegDecoder::new_with_options(std::io::Cursor::new(data), options());
    decoder.decode_headers().map_err(|e| Error::new(e.to_string()))?;
    let Some(info) = decoder.info() else { return fail("Unable to decode JPEG image.") };
    let (width, height) = check_dimensions(info.width as i64, info.height as i64, "LLImageJPEG")?;
    let Some(size) = decoder.output_buffer_size() else { return fail("Unable to decode JPEG image.") };
    let mut top_down = vec![0u8; size];
    decoder.decode_into(&mut top_down).map_err(|e| Error::new(e.to_string()))?;
    if decoder.output_colorspace() != Some(ColorSpace::RGB) {
        return fail("Unsupported color conversion request");
    }
    let row = width as usize * 3;
    if top_down.len() < row * height as usize {
        return fail("Unable to decode JPEG image.");
    }
    top_down.truncate(row * height as usize);
    let mut raster = Raster { width, height, components: 3, data: top_down };
    raster.vertical_flip();
    Ok(raster)
}

/// `LLImageJPEG::encode`: baseline JPEG of a 1 (greyscale) or 3 (RGB)
/// component raster at `quality` 1..=100 (libjpeg's scale).
pub fn encode(raster: &Raster, quality: i32) -> Result<Vec<u8>, Error> {
    let color = match raster.components {
        1 => ColorType::Luma,
        3 => ColorType::Rgb,
        _ => return fail("Unable to encode a JPEG image that doesn't have 1 or 3 components."),
    };
    let w = raster.width as usize;
    let h = raster.height as usize;
    let row = w * raster.components as usize;
    if raster.data.len() < row * h {
        return fail("LLImageJPEG: raster is shorter than its dimensions");
    }
    if w == 0 || h == 0 || w > u16::MAX as usize || h > u16::MAX as usize {
        return fail("Empty JPEG image (DNL not supported)");
    }
    // jpeg_set_quality clamps to 1..100.
    let quality = quality.clamp(1, 100) as u8;

    // The viewer's rows are bottom-up; JPEG wants them top-down.
    let mut top_down = Vec::with_capacity(row * h);
    for y in (0..h).rev() {
        top_down.extend_from_slice(&raster.data[y * row..(y + 1) * row]);
    }

    let mut out = Vec::with_capacity(row * h / 4 + 1024);
    let mut encoder = Encoder::new(&mut out, quality);
    // jpeg_set_defaults: 2x2 chroma sampling for YCbCr at every quality,
    // box-filter downsampling, standard Huffman tables, JFIF 1.01 density 1x1.
    encoder.set_sampling_factor(SamplingFactor::F_2_2);
    encoder.set_chroma_subsampling_method(ChromaSubsamplingMethod::Average);
    encoder
        .encode(&top_down, w as u16, h as u16, color)
        .map_err(|e| Error::new(e.to_string()))?;
    Ok(out)
}
