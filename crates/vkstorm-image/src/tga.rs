//! Truevision TGA, the port of `LLImageTGA`.
//!
//! Accepted: image types 1 (colour-mapped), 2 (true colour), 3 (monochrome)
//! and their run-length forms 9, 10, 11; 8, 15/16, 24 and 32 bits per pixel;
//! colour maps of 8, 15/16, 24 or 32 bits; bottom-left and top-left origins.
//! A 32-bit image whose alpha is entirely opaque decodes to 3 components, as
//! the viewer always did. Run-length colour-mapped images are flipped
//! unconditionally, also as the viewer always did.

use crate::{Error, Raster, check_dimensions, fail};

/// 5-bit to 8-bit expansion with best rounding (`LLImageTGA::s5to8bits`).
const S5TO8BITS: [u8; 32] = [
    0, 8, 16, 25, 33, 41, 49, 58, 66, 74, 82, 90, 99, 107, 115, 123, 132, 140, 148, 156, 165, 173,
    181, 189, 197, 206, 214, 222, 230, 239, 247, 255,
];

/// Expand a 16-bit little-endian xRRRRRGGGGGBBBBB pixel to RGB.
#[inline]
fn pixel15(dst: &mut [u8], src: &[u8]) {
    let t = src[0] as u32 + ((src[1] as u32) << 8);
    dst[2] = S5TO8BITS[(t & 0x1f) as usize];
    dst[1] = S5TO8BITS[((t >> 5) & 0x1f) as usize];
    dst[0] = S5TO8BITS[((t >> 10) & 0x1f) as usize];
}

/// A colour map the decoder will use (only read for image types 1 and 9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorMap {
    pub start: i32,
    pub length: i32,
    pub bytes_per_entry: usize,
    pub data: Vec<u8>,
}

/// What `LLImageTGA::updateData` learns from the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    /// Components of the decoded raster: 1, 3 or 4.
    pub components: u32,
    /// Offset of the pixel data (after the ID field and any colour map).
    pub data_offset: usize,
    pub image_type: u8,
    pub pixel_size: u8,
    pub attribute_bits: u8,
    pub origin_right: bool,
    pub origin_top: bool,
    pub color_map: Option<ColorMap>,
    pub is_15bit: bool,
}

impl Info {
    /// Run-length encoded image types (9, 10, 11).
    pub fn is_rle(&self) -> bool {
        self.image_type & 0x08 != 0
    }
}

/// Parse the header (`LLImageTGA::updateData`).
pub fn parse(data: &[u8]) -> Result<Info, Error> {
    if data.is_empty() {
        return fail("LLImageTGA uninitialized");
    }
    if data.len() < 18 {
        return fail("Unable to load file.  TGA header is truncated.");
    }
    let id_length = data[0] as usize;
    let color_map_type = data[1];
    let image_type = data[2];
    let color_map_index = data[3] as i32 + ((data[4] as i32) << 8);
    let color_map_length = data[5] as i32 + ((data[6] as i32) << 8);
    let color_map_depth = data[7];
    let width = data[12] as i64 + ((data[13] as i64) << 8);
    let height = data[14] as i64 + ((data[15] as i64) << 8);
    let pixel_size = data[16];
    let flags = data[17];
    let mut attribute_bits = flags & 0xf;
    let origin_right = flags & 0x10 != 0;
    let origin_top = flags & 0x20 != 0;

    match image_type {
        0 => return fail("Unable to load file.  TGA file contains no image data."),
        1 => {
            if pixel_size != 8 {
                return fail(
                    "Unable to load file.  Colormapped images must have 8 bits per pixel.",
                );
            }
        }
        2 | 9 | 10 => {}
        3 | 11 => {
            if pixel_size != 8 {
                return fail("Unable to load file.  Monochrome images must have 8 bits per pixel.");
            }
        }
        _ => return fail("Unable to load file.  Unrecoginzed TGA image type."),
    }

    let mut data_offset = 18 + id_length;

    let mut color_map = None;
    if color_map_type == 1 && color_map_depth > 0 {
        let bytes_per_entry = if color_map_depth > 24 {
            4
        } else if color_map_depth > 16 {
            3
        } else if color_map_depth > 8 {
            2
        } else {
            1
        };
        let color_map_bytes = color_map_length as usize * bytes_per_entry;
        // A colour map is only kept when the image indexes it; true-colour
        // files may carry one for other purposes.
        if image_type == 1 || image_type == 9 {
            let Some(map) = data.get(data_offset..data_offset + color_map_bytes) else {
                return fail("Unable to load file.  TGA colour map is truncated.");
            };
            color_map = Some(ColorMap {
                start: color_map_index,
                length: color_map_length,
                bytes_per_entry,
                data: map.to_vec(),
            });
        }
        data_offset += color_map_bytes;
    }

    let bits_per_pixel = if color_map.is_some() {
        color_map_depth
    } else {
        pixel_size
    };
    let mut is_15bit = false;
    let components = match bits_per_pixel {
        24 => 3,
        32 => {
            // Not enforced: some writers do not set the attribute bits.
            attribute_bits = 8;
            4
        }
        15 | 16 => {
            is_15bit = true; // the 16th bit is a Targa hardware interrupt flag, ignored
            3
        }
        8 => 1,
        _ => return fail("Unable to load file. Unknown pixel size."),
    };
    let (width, height) = check_dimensions(width, height, "LLImageTGA")?;

    Ok(Info {
        width,
        height,
        components,
        data_offset,
        image_type,
        pixel_size,
        attribute_bits,
        origin_right,
        origin_top,
        color_map,
        is_15bit,
    })
}

/// Parse and decode (`LLImageTGA::updateData` then `decode`).
pub fn decode(data: &[u8]) -> Result<Raster, Error> {
    let info = parse(data)?;
    decode_with(&info, data)
}

/// Decode with an already parsed header (`LLImageTGA::decode`).
pub fn decode_with(info: &Info, data: &[u8]) -> Result<Raster, Error> {
    if data.is_empty() {
        return fail("LLImageTGA trying to decode an image with no data!");
    }
    if info.components != 1 && info.components != 3 && info.components != 4 {
        return fail(
            "TGA images with a number of components other than 1, 3, and 4 are not supported.",
        );
    }
    if info.origin_right {
        return fail("TGA images with origin on right side are not supported.");
    }
    let Some(src) = data.get(info.data_offset..) else {
        return fail("LLImageTGA: image data is truncated");
    };
    let mut raster = Raster::new(info.width, info.height, info.components);
    if let Some(map) = &info.color_map {
        decode_color_map(info, map, src, &mut raster)?;
    } else {
        decode_truecolor(info, src, &mut raster)?;
    }
    Ok(raster)
}

/// Run a TGA run-length stream producing `pixels` pixels of `bpp` bytes.
/// `emit(src_pixel, repeat)` writes one source pixel `repeat` times.
fn run_length(
    src: &[u8],
    bpp: usize,
    pixels: usize,
    mut emit: impl FnMut(&[u8], usize),
) -> Result<(), Error> {
    let truncated = || Error::new("LLImageTGA: run-length data is truncated");
    let mut pos = 0usize;
    let mut done = 0usize;
    while done < pixels {
        let header = *src.get(pos).ok_or_else(truncated)?;
        pos += 1;
        let count = (header & 0x7f) as usize + 1;
        if count > pixels - done {
            // The C++ would write past the end of the raster here.
            return fail("LLImageTGA: run-length packet overruns the image");
        }
        if header & 0x80 != 0 {
            let px = src.get(pos..pos + bpp).ok_or_else(truncated)?;
            emit(px, count);
            pos += bpp;
        } else {
            let block = src.get(pos..pos + count * bpp).ok_or_else(truncated)?;
            for px in block.chunks_exact(bpp) {
                emit(px, 1);
            }
            pos += count * bpp;
        }
        done += count;
    }
    Ok(())
}

fn decode_truecolor(info: &Info, src: &[u8], raster: &mut Raster) -> Result<(), Error> {
    let pixels = info.width as usize * info.height as usize;
    let c = info.components as usize;
    let mut alpha_opaque = true;
    if info.is_rle() {
        let dst = &mut raster.data;
        let mut at = 0usize;
        match c {
            1 => run_length(src, 1, pixels, |px, n| {
                dst[at..at + n].fill(px[0]);
                at += n;
            })?,
            3 if info.is_15bit => run_length(src, 2, pixels, |px, n| {
                let mut rgb = [0u8; 3];
                pixel15(&mut rgb, px);
                for _ in 0..n {
                    dst[at..at + 3].copy_from_slice(&rgb);
                    at += 3;
                }
            })?,
            3 => run_length(src, 3, pixels, |px, n| {
                let rgb = [px[2], px[1], px[0]];
                for _ in 0..n {
                    dst[at..at + 3].copy_from_slice(&rgb);
                    at += 3;
                }
            })?,
            _ => run_length(src, 4, pixels, |px, n| {
                let rgba = [px[2], px[1], px[0], px[3]];
                if px[3] != 255 {
                    alpha_opaque = false;
                }
                for _ in 0..n {
                    dst[at..at + 4].copy_from_slice(&rgba);
                    at += 4;
                }
            })?,
        }
    } else {
        let needed = pixels * if info.is_15bit { 2 } else { c };
        if needed > src.len() {
            return fail("LLImageTGA: image data is truncated");
        }
        let dst = &mut raster.data;
        match c {
            4 => {
                for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = s[3];
                    if s[3] != 255 {
                        alpha_opaque = false;
                    }
                }
            }
            3 if info.is_15bit => {
                for (d, s) in dst.chunks_exact_mut(3).zip(src.chunks_exact(2)) {
                    pixel15(d, s);
                }
            }
            3 => {
                for (d, s) in dst.chunks_exact_mut(3).zip(src.chunks_exact(3)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                }
            }
            _ => dst.copy_from_slice(&src[..pixels]),
        }
    }
    if c == 4 && alpha_opaque {
        // Alpha entirely opaque: the viewer stores the image as 24-bit.
        *raster = raster.drop_alpha();
    }
    if info.origin_top {
        raster.vertical_flip();
    }
    Ok(())
}

fn decode_color_map(
    info: &Info,
    map: &ColorMap,
    src: &[u8],
    raster: &mut Raster,
) -> Result<(), Error> {
    if info.pixel_size != 8 {
        return fail("LLImageTGA: colour-mapped images must have 8 bits per pixel");
    }
    if map.length <= 0 {
        return fail("LLImageTGA: empty colour map");
    }
    let c = info.components as usize;
    let bpe = map.bytes_per_entry;
    let lookup = |dst: &mut [u8], index: u8| {
        let i = (index as i32 - map.start).clamp(0, map.length - 1) as usize;
        let entry = &map.data[i * bpe..];
        match bpe {
            1 => dst[0] = entry[0],
            2 => pixel15(dst, entry),
            3 => {
                dst[0] = entry[2];
                dst[1] = entry[1];
                dst[2] = entry[0];
            }
            _ => {
                dst[0] = entry[2];
                dst[1] = entry[1];
                dst[2] = entry[0];
                dst[3] = entry[3];
            }
        }
    };
    let width = info.width as usize;
    let height = info.height as usize;
    let pixels = width * height;
    if info.is_rle() {
        let dst = &mut raster.data;
        let mut at = 0usize;
        run_length(src, 1, pixels, |px, n| {
            let mut color = [0u8; 4];
            lookup(&mut color, px[0]);
            for _ in 0..n {
                dst[at..at + c].copy_from_slice(&color[..c]);
                at += c;
            }
        })?;
        // The viewer flips run-length colour-mapped images regardless of
        // the origin bit.
        raster.vertical_flip();
    } else {
        if pixels > src.len() {
            return fail("LLImageTGA: image data is truncated");
        }
        let dst_row = width * c;
        for row in 0..height {
            let src_row = if info.origin_top {
                height - 1 - row
            } else {
                row
            };
            let s = &src[src_row * width..src_row * width + width];
            let d = &mut raster.data[row * dst_row..row * dst_row + dst_row];
            for (dp, &index) in d.chunks_exact_mut(c).zip(s) {
                lookup(dp, index);
            }
        }
    }
    Ok(())
}

/// `LLImageTGA::decodeAndProcess`: decode a monochrome run-length image
/// through the avatar alpha-gradient function. `domain` is the width of the
/// ramp (0 gives a step at `1 - weight`).
pub fn decode_and_process(data: &[u8], domain: f32, weight: f32) -> Result<Raster, Error> {
    let info = parse(data)?;
    decode_and_process_with(&info, data, domain, weight)
}

/// [`decode_and_process`] with an already parsed header.
pub fn decode_and_process_with(
    info: &Info,
    data: &[u8],
    domain: f32,
    weight: f32,
) -> Result<Raster, Error> {
    if data.is_empty() {
        return fail("LLImageTGA trying to decode an image with no data!");
    }
    if info.components != 1 || info.image_type != 11 || info.origin_top || info.origin_right {
        return fail(format!(
            "LLImageTGA trying to alpha-gradient process an image that's not a standard RLE, one component image \
             (getComponents(): {} mImageType: {} mOriginTopBit: {} mOriginRightBit: {})",
            info.components, info.image_type, info.origin_top as u8, info.origin_right as u8
        ));
    }
    let Some(src) = data.get(info.data_offset..) else {
        return fail("LLImageTGA: image data is truncated");
    };
    let mut raster = Raster::new(info.width, info.height, 1);
    let pixels = info.width as usize * info.height as usize;

    // Build the per-value map first; the run-length pass then only copies.
    let mut lut = [0u8; 256];
    if domain > 0.0 {
        let scale = 1.0f32 / domain;
        let offset = (1.0f32 - domain) * (1.0f32 - weight).clamp(0.0, 1.0);
        let bias = -(scale * offset);
        for (i, v) in lut.iter_mut().enumerate() {
            *v = (255.0f32 * (i as f32 / 255.0f32 * scale + bias)).clamp(0.0, 255.0) as u8;
        }
    } else {
        let threshold = (255.0f32 * (1.0f32 - weight).clamp(0.0, 1.0)) as u8;
        for (i, v) in lut.iter_mut().enumerate() {
            *v = if i as u8 >= threshold { 0xff } else { 0 };
        }
    }
    let dst = &mut raster.data;
    let mut at = 0usize;
    run_length(src, 1, pixels, |px, n| {
        dst[at..at + n].fill(lut[px[0] as usize]);
        at += n;
    })?;
    Ok(raster)
}

/// `LLImageTGA::encode`: uncompressed true colour (2 components are written
/// as intensity plus alpha, 4 bytes per pixel) or monochrome, bottom-left
/// origin.
pub fn encode(raster: &Raster) -> Result<Vec<u8>, Error> {
    let (image_type, bytes_per_pixel) = match raster.components {
        1 => (3u8, 1usize),
        2 => (2, 4),
        3 => (2, 3),
        4 => (2, 4),
        _ => return fail("LLImageTGA: unsupported number of components"),
    };
    let pixels = raster.width as usize * raster.height as usize;
    if raster.data.len() < pixels * raster.components as usize {
        return fail("LLImageTGA: raster is shorter than its dimensions");
    }
    let attribute_bits: u8 = if bytes_per_pixel == 4 { 8 } else { 0 };
    let mut out = Vec::with_capacity(18 + bytes_per_pixel * pixels);
    out.extend_from_slice(&[0, 0, image_type, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    out.extend_from_slice(&(raster.width as u16).to_le_bytes());
    out.extend_from_slice(&(raster.height as u16).to_le_bytes());
    out.push((bytes_per_pixel * 8) as u8);
    // The viewer's descriptor byte: only the attribute bits are ever set.
    out.push(attribute_bits & 0xf);
    let src = &raster.data[..pixels * raster.components as usize];
    let start = out.len();
    out.resize(start + bytes_per_pixel * pixels, 0);
    let dst = &mut out[start..];
    match raster.components {
        1 => dst.copy_from_slice(src),
        2 => {
            for (d, px) in dst.chunks_exact_mut(4).zip(src.chunks_exact(2)) {
                d.copy_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        3 => {
            for (d, px) in dst.chunks_exact_mut(3).zip(src.chunks_exact(3)) {
                d.copy_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        _ => {
            for (d, px) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                d.copy_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        }
    }
    Ok(out)
}
