//! Windows bitmaps, the port of `LLImageBMP`.
//!
//! Accepted: `BM` files with a 40-byte (BITMAPINFOHEADER), 108-byte
//! (BITMAPV4HEADER) or larger header, 8 bits per pixel with a 256-entry
//! colour table, or 24 and 32 bits per pixel, uncompressed or with bitfield
//! masks (compression 3); either row order. 1, 4 and 16-bit images, run-length
//! compression and OS/2 headers are rejected with the viewer's messages. The
//! decoded raster always has 3 components; 32-bit alpha is dropped.

use crate::{check_dimensions, fail, Error, Raster};

const FILE_HEADER_SIZE: usize = 14;
const BITMAP_HEADER_SIZE: usize = 40;
/// `sizeof(Win95BmpHeaderExtension)` as the C++ compiler lays it out.
const WIN95_EXTENSION_SIZE: usize = 52;

fn i32_at(d: &[u8], at: usize) -> i32 {
    i32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}
fn i16_at(d: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([d[at], d[at + 1]])
}

/// What `LLImageBMP::updateData` learns from the headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    /// Always 3 for the decoded raster; the header's own channel count.
    pub components: u32,
    pub bitmap_offset: i32,
    pub bits_per_pixel: i32,
    /// Bitfield masks (r, g, b, a) when compression 3 supplied them.
    pub masks: [u32; 4],
    pub origin_at_top: bool,
    /// Colour table entries (BGRx), 4 bytes each.
    pub palette: Vec<u8>,
    pub palette_colors: i32,
}

/// Parse the file and bitmap headers (`LLImageBMP::updateData`).
pub fn parse(data: &[u8]) -> Result<Info, Error> {
    if data.is_empty() {
        return fail("Uninitialized instance of LLImageBMP");
    }
    if data.len() < FILE_HEADER_SIZE + BITMAP_HEADER_SIZE {
        return fail("Does not appear to be a bitmap file");
    }
    if data[0] != b'B' || data[1] != b'M' {
        // The viewer's two messages, in its order.
        if data[0] != b'B' || data[1] != b'A' {
            return fail("OS/2 bitmap array BMP files are not supported");
        }
        return fail("Does not appear to be a bitmap file");
    }
    let bitmap_offset = i32_at(data, 10);

    let h = &data[FILE_HEADER_SIZE..];
    let size = i32_at(h, 0);
    let width = i32_at(h, 4);
    let mut height = i32_at(h, 8);
    let bits_per_pixel = i16_at(h, 14) as i32;
    // The viewer reads the 32-bit compression field as a 16-bit value.
    let compression = i16_at(h, 16) as i32;
    let num_colors = i32_at(h, 32);

    let mut windows_nt_version = false;
    let mut windows_95_version = false;
    if size == 12 {
        return fail("Windows 2.x and OS/2 1.x BMP files are not supported");
    } else if size == 40 {
        windows_nt_version = compression == 3;
    } else if (12..=64).contains(&size) {
        return fail("OS/2 2.x BMP files are not supported");
    } else if size >= 108 {
        // BITMAPV4HEADER, BITMAPV5HEADER or later
        windows_95_version = true;
    }

    let origin_at_top = height < 0;
    if origin_at_top {
        height = -height;
    }

    let components = match bits_per_pixel {
        8 => 1,
        24 | 32 => 3,
        1 | 4 | 16 => return fail("Unsupported bit depth"),
        _ => return fail("Unrecognized bit depth"),
    };
    let (width, height) = check_dimensions(width as i64, height as i64, "LLImageBMP")?;

    match compression {
        0 | 3 => {}
        1 => return fail("8 bit RLE compression not supported."),
        2 => return fail("4 bit RLE compression not supported."),
        _ => return fail("Unsupported compression format."),
    }

    let ext_at = FILE_HEADER_SIZE + BITMAP_HEADER_SIZE;
    let mut masks = [0u32; 4];
    let mut extension_size = 0usize;
    if windows_nt_version {
        if bits_per_pixel != 16 && bits_per_pixel != 32 {
            return fail("Bitfield encoding requires 16 or 32 bits per pixel.");
        }
        if num_colors != 0 {
            return fail("Bitfield encoding is not compatible with a color table.");
        }
        extension_size = 12;
        if data.len() < ext_at + extension_size {
            return fail("LLImageBMP: bitfield masks are truncated");
        }
        for (i, m) in masks.iter_mut().take(3).enumerate() {
            *m = i32_at(data, ext_at + 4 * i) as u32;
        }
    } else if windows_95_version {
        extension_size = WIN95_EXTENSION_SIZE;
        if data.len() < ext_at + extension_size {
            return fail("LLImageBMP: header extension is truncated");
        }
        if compression == 3 {
            for (i, m) in masks.iter_mut().enumerate() {
                *m = i32_at(data, ext_at + 4 * i) as u32;
            }
        }
    }

    let mut palette = Vec::new();
    let mut palette_colors = 0;
    if bits_per_pixel < 16 {
        palette_colors = if num_colors == 0 { 1 << bits_per_pixel } else { num_colors };
        if palette_colors < 0 {
            return fail("LLImageBMP: invalid colour table size");
        }
        let at = ext_at + extension_size;
        let bytes = palette_colors as usize * 4;
        let Some(table) = data.get(at..at + bytes) else {
            return fail("LLImageBMP: colour table is truncated");
        };
        palette = table.to_vec();
    }

    Ok(Info {
        width,
        height,
        components,
        bitmap_offset,
        bits_per_pixel,
        masks,
        origin_at_top,
        palette,
        palette_colors,
    })
}

/// Parse and decode (`LLImageBMP::updateData` then `decode`).
pub fn decode(data: &[u8]) -> Result<Raster, Error> {
    let info = parse(data)?;
    decode_with(&info, data)
}

/// Decode with an already parsed header (`LLImageBMP::decode`). The raster
/// has 3 components.
pub fn decode_with(info: &Info, data: &[u8]) -> Result<Raster, Error> {
    if data.is_empty() {
        return fail("llimagebmp trying to decode an image with no data!");
    }
    if info.bitmap_offset < 0 || info.bitmap_offset as usize > data.len() {
        return fail("LLImageBMP: bitmap offset lies outside the file");
    }
    let src = &data[info.bitmap_offset as usize..];
    let width = info.width as usize;
    let height = info.height as usize;
    let mut raster = Raster::new(info.width, info.height, 3);
    let dst = &mut raster.data;

    match info.bits_per_pixel {
        8 => {
            if info.palette_colors < 256 {
                return fail("LLImageBMP: 8 bit images need a 256 entry colour table");
            }
            let alignment = (3 * width) % 4;
            if width * height + height * alignment > src.len() {
                return fail("LLImageBMP: image data is truncated");
            }
            let stride = width + alignment;
            for (row, d) in dst.chunks_exact_mut(width * 3).enumerate() {
                let s = &src[row * stride..row * stride + width];
                for (px, &index) in d.chunks_exact_mut(3).zip(s) {
                    let i = 4 * index as usize;
                    px[0] = info.palette[i + 2];
                    px[1] = info.palette[i + 1];
                    px[2] = info.palette[i];
                }
            }
        }
        24 => {
            let alignment = (9 * width) % 4;
            if width * height * 3 + height * alignment > src.len() {
                return fail("LLImageBMP: image data is truncated");
            }
            let stride = width * 3 + alignment;
            for (row, d) in dst.chunks_exact_mut(width * 3).enumerate() {
                let s = &src[row * stride..row * stride + width * 3];
                for (px, bgr) in d.chunks_exact_mut(3).zip(s.chunks_exact(3)) {
                    px[0] = bgr[2];
                    px[1] = bgr[1];
                    px[2] = bgr[0];
                }
            }
        }
        32 => {
            // Alpha is not supported.
            let mut masks = info.masks;
            if masks[0] == 0 && masks[1] == 0 && masks[2] == 0 {
                masks = [0x00FF0000, 0x0000FF00, 0x000000FF, 0];
            }
            if width * height * 4 > src.len() {
                return fail("LLImageBMP: image data is truncated");
            }
            let shift = |m: u32| if m == 0 { 0 } else { m.trailing_zeros() };
            let (rs, gs, bs) = (shift(masks[0]), shift(masks[1]), shift(masks[2]));
            for (px, s) in dst.chunks_exact_mut(3).zip(src.chunks_exact(4)) {
                let value = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                px[0] = ((value & masks[0]) >> rs) as u8;
                px[1] = ((value & masks[1]) >> gs) as u8;
                px[2] = ((value & masks[2]) >> bs) as u8;
            }
        }
        _ => return fail("LLImageBMP: unsupported bit depth"),
    }

    if info.origin_at_top {
        raster.vertical_flip();
    }
    Ok(raster)
}

/// `LLImageBMP::encode`: 8-bit greyscale with a colour table for 1 and 2
/// components (2 writes intensity times alpha), 24-bit for 3 and 4 (alpha
/// dropped). `pels_per_meter` fills the resolution fields (the viewer writes
/// 2834 on macOS and 0 elsewhere).
pub fn encode(raster: &Raster, pels_per_meter: i32) -> Result<Vec<u8>, Error> {
    let src_components = raster.components as usize;
    if !(1..=4).contains(&src_components) {
        return fail("LLImageBMP: unsupported number of components");
    }
    let dst_components = if src_components < 3 { 1usize } else { 3 };
    let width = raster.width as usize;
    let height = raster.height as usize;
    if raster.data.len() < width * height * src_components {
        return fail("LLImageBMP: raster is shorter than its dimensions");
    }

    let mut header_bytes = FILE_HEADER_SIZE + BITMAP_HEADER_SIZE;
    if dst_components == 1 {
        header_bytes += 1024; // colour table
    }
    let mut line_bytes = dst_components * width;
    let alignment = (3 * line_bytes) % 4;
    line_bytes += alignment;
    let file_bytes = line_bytes * height + header_bytes;

    let mut out = Vec::with_capacity(file_bytes);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_bytes as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(header_bytes as u32).to_le_bytes());
    out.extend_from_slice(&40i32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&1i16.to_le_bytes());
    out.extend_from_slice(&(if dst_components == 1 { 8i16 } else { 24 }).to_le_bytes());
    out.extend_from_slice(&0i16.to_le_bytes()); // compression
    out.extend_from_slice(&0i16.to_le_bytes()); // alignment padding
    out.extend_from_slice(&0i32.to_le_bytes()); // image size
    out.extend_from_slice(&pels_per_meter.to_le_bytes());
    out.extend_from_slice(&pels_per_meter.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes()); // colours
    out.extend_from_slice(&0i32.to_le_bytes()); // important colours
    if dst_components == 1 {
        for n in 0..=255u8 {
            out.extend_from_slice(&[n, n, n, 0]);
        }
    }

    let src = &raster.data[..width * height * src_components];
    let start = out.len();
    out.resize(file_bytes, 0);
    let dst = &mut out[start..];
    for (row, d) in src.chunks_exact(width * src_components).zip(dst.chunks_exact_mut(line_bytes)) {
        match src_components {
            1 => d[..width].copy_from_slice(row),
            2 => {
                for (o, px) in d.iter_mut().zip(row.chunks_exact(2)) {
                    *o = (px[0] as u32 * px[1] as u32 / 255) as u8;
                }
            }
            _ => {
                for (o, px) in d.chunks_exact_mut(3).zip(row.chunks_exact(src_components)) {
                    o.copy_from_slice(&[px[2], px[1], px[0]]);
                }
            }
        }
    }
    Ok(out)
}
