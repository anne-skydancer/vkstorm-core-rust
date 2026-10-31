//! The viewer's mipped DXT container, the port of `LLImageDXT`.
//!
//! Two headers: the viewer's old 16-byte one (format code, max level, width,
//! height) and a 128-byte DDS header (`DDS ` four-cc, mip count, dimensions,
//! pixel-format four-cc). Mips are stored smallest first, each padded to a
//! 4-byte boundary. Only the uncompressed formats (I8, A8, RGB8, RGBA8) and
//! the DXR formats decode; the old DXT1-5 four-ccs are refused as the viewer
//! refused them (it aborted where the port returns an error).

use crate::{Error, Raster, check_dimensions, fail, generate_mip_into};

/// `MAX_IMAGE_MIP` of the viewer: 4096x4096.
pub const MAX_IMAGE_MIP: i32 = 12;
/// `LLImageDXT::calcHeaderSize`: the larger of the two header layouts.
pub const HEADER_SIZE: i32 = 128;
const OLD_HEADER_SIZE: i32 = 16;
const DDS_FOURCC: i32 = 0x20534444;

/// `LLImageDXT::EFileFormat`.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Unknown = 0,
    I8 = 1,
    A8 = 2,
    Rgb8 = 3,
    Rgba8 = 4,
    Dxt1 = 5,
    Dxt2 = 6,
    Dxt3 = 7,
    Dxt4 = 8,
    Dxt5 = 9,
    Dxr1 = 10,
    Dxr2 = 11,
    Dxr3 = 12,
    Dxr4 = 13,
    Dxr5 = 14,
    NoFile = 0xff,
}

impl Format {
    /// From the numeric code (the old header's format field).
    pub fn from_code(code: i32) -> Format {
        use Format::*;
        match code {
            1 => I8,
            2 => A8,
            3 => Rgb8,
            4 => Rgba8,
            5 => Dxt1,
            6 => Dxt2,
            7 => Dxt3,
            8 => Dxt4,
            9 => Dxt5,
            10 => Dxr1,
            11 => Dxr2,
            12 => Dxr3,
            13 => Dxr4,
            14 => Dxr5,
            0xff => NoFile,
            _ => Unknown,
        }
    }

    pub fn code(self) -> i32 {
        self as i32
    }

    /// `LLImageDXT::getFormat`.
    pub fn from_fourcc(fourcc: i32) -> Format {
        use Format::*;
        match fourcc {
            0x20203849 => I8,
            0x20203841 => A8,
            0x20424752 => Rgb8,
            0x41424752 => Rgba8,
            0x31525844 => Dxr1,
            0x32525844 => Dxr2,
            0x33525844 => Dxr3,
            0x34525844 => Dxr4,
            0x35525844 => Dxr5,
            0x31545844 => Dxt1,
            0x32545844 => Dxt2,
            0x33545844 => Dxt3,
            0x34545844 => Dxt4,
            0x35545844 => Dxt5,
            _ => Unknown,
        }
    }

    /// `LLImageDXT::getFourCC`.
    pub fn fourcc(self) -> i32 {
        use Format::*;
        match self {
            I8 => 0x20203849,
            A8 => 0x20203841,
            Rgb8 => 0x20424752,
            Rgba8 => 0x41424752,
            Dxr1 => 0x31525844,
            Dxr2 => 0x32525844,
            Dxr3 => 0x33525844,
            Dxr4 => 0x34525844,
            Dxr5 => 0x35525844,
            Dxt1 => 0x31545844,
            Dxt2 => 0x32545844,
            Dxt3 => 0x33545844,
            Dxt4 => 0x34545844,
            Dxt5 => 0x35545844,
            Unknown | NoFile => 0,
        }
    }

    /// `LLImageDXT::isCompressed`: DXT1 through DXR5.
    pub fn is_compressed(self) -> bool {
        (Format::Dxt1 as i32..=Format::Dxr5 as i32).contains(&(self as i32))
    }

    /// The old four-ccs `getMipOffset` refuses.
    fn is_old_dxt(self) -> bool {
        (Format::Dxt1 as i32..=Format::Dxt5 as i32).contains(&(self as i32))
    }
}

fn unknown_format<T>(format: Format) -> Result<T, Error> {
    fail(format!("LLImageDXT::Unknown format: {}", format.code()))
}

/// `LLImageDXT::checkMinWidthHeight`: 4x4 minimum for compressed formats, 1x1 otherwise.
pub fn check_min_width_height(format: Format, width: &mut i32, height: &mut i32) {
    let mindim = if format.is_compressed() { 4 } else { 1 };
    *width = (*width).max(mindim);
    *height = (*height).max(mindim);
}

/// `LLImageDXT::formatBits`.
pub fn format_bits(format: Format) -> Result<i32, Error> {
    use Format::*;
    Ok(match format {
        Dxt1 | Dxr1 => 4,
        I8 | A8 | Dxt3 | Dxr3 | Dxr5 | Dxt5 => 8,
        Rgb8 => 24,
        Rgba8 => 32,
        _ => return unknown_format(format),
    })
}

/// `LLImageDXT::formatBytes`: the padded byte size of one mip.
pub fn format_bytes(format: Format, mut width: i32, mut height: i32) -> Result<i32, Error> {
    check_min_width_height(format, &mut width, &mut height);
    let bytes = (width * height * format_bits(format)? + 7) >> 3;
    Ok((bytes + 3) & !3)
}

/// `LLImageDXT::formatComponents`.
pub fn format_components(format: Format) -> Result<i32, Error> {
    use Format::*;
    Ok(match format {
        Dxt1 | Dxr1 | Rgb8 => 3,
        I8 | A8 => 1,
        Dxt3 | Dxr3 | Dxt5 | Dxr5 | Rgba8 => 4,
        _ => return unknown_format(format),
    })
}

/// `LLImageDXT::calcDiscardWidthHeight`.
pub fn calc_discard_width_height(
    mut discard_level: i32,
    format: Format,
    width: &mut i32,
    height: &mut i32,
) {
    while discard_level > 0 && *width > 1 && *height > 1 {
        discard_level -= 1;
        *width >>= 1;
        *height >>= 1;
    }
    check_min_width_height(format, width, height);
}

/// `LLImageDXT::calcNumMips`.
pub fn calc_num_mips(mut width: i32, mut height: i32) -> i32 {
    let mut nmips = 0;
    while width > 0 && height > 0 {
        width >>= 1;
        height >>= 1;
        nmips += 1;
    }
    nmips
}

/// `LLImageDXT::getMipOffset`: byte offset of the mip at `discard` (0 is the
/// largest, stored last) for an image of the given full size.
pub fn mip_offset(
    format: Format,
    header_size: i32,
    mut width: i32,
    mut height: i32,
    discard: i32,
) -> Result<i32, Error> {
    if format.is_old_dxt() {
        return fail("getMipOffset called with old (unsupported) format");
    }
    let num_mips = calc_num_mips(width, height);
    let discard = discard.clamp(0, (num_mips - 1).max(0));
    let last_mip = num_mips - 1 - discard;
    let mut offset = header_size;
    for mipidx in (0..num_mips).rev() {
        if mipidx < last_mip {
            offset += format_bytes(format, width, height)?;
        }
        width >>= 1;
        height >>= 1;
    }
    Ok(offset)
}

/// What `LLImageDXT::updateData` learns from the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub format: Format,
    pub header_size: i32,
    pub width: u32,
    pub height: u32,
    pub components: u32,
    /// The finest mip the data holds, capped by the header's mip count; -1
    /// when even the smallest mip is incomplete.
    pub discard_level: i32,
}

/// `LLImageDXT::calcDataSize` for a parsed image.
pub fn data_size(info: &Info, discard_level: i32) -> Result<i32, Error> {
    if info.format == Format::Unknown {
        return fail("calcDataSize called with unloaded LLImageDXT");
    }
    let discard_level = if discard_level < 0 {
        info.discard_level
    } else {
        discard_level
    };
    data_size_at(
        info.format,
        info.header_size,
        info.width as i32,
        info.height as i32,
        discard_level,
    )
}

fn data_size_at(
    format: Format,
    header_size: i32,
    width: i32,
    height: i32,
    discard_level: i32,
) -> Result<i32, Error> {
    let discard_level = discard_level.max(0);
    let mut bytes = mip_offset(format, header_size, width, height, discard_level)?;
    bytes += format_bytes(format, width >> discard_level, height >> discard_level)?;
    Ok(bytes)
}

/// Parse the header (`LLImageDXT::updateData`).
pub fn parse(data: &[u8]) -> Result<Info, Error> {
    if data.is_empty() {
        return fail("LLImageDXT uninitialized");
    }
    let i32_at = |at: usize| -> Result<i32, Error> {
        data.get(at..at + 4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| Error::new("LLImageDXT: not enough data"))
    };
    let first = i32_at(0)?;
    let (format, header_size, miplevelmax, width, height) = if first != DDS_FOURCC {
        // Old header: format code, max level, width, height.
        (
            Format::from_code(first),
            OLD_HEADER_SIZE,
            i32_at(4)?.min(MAX_IMAGE_MIP),
            i32_at(8)?,
            i32_at(12)?,
        )
    } else {
        // DDS header: mip count at 28, height at 12, width at 16, pixel-format four-cc at 84.
        (
            Format::from_fourcc(i32_at(84)?),
            HEADER_SIZE,
            (i32_at(28)? - 1).min(MAX_IMAGE_MIP),
            i32_at(16)?,
            i32_at(12)?,
        )
    };
    if (data.len() as i64) < header_size as i64 {
        return fail("LLImageDXT: not enough data");
    }
    let components = format_components(format)?;
    let (w, h) = check_dimensions(width as i64, height as i64, "LLImageDXT")?;

    // calcDiscardLevelBytes: the first discard level whose data is complete.
    let bytes = data.len().min(i32::MAX as usize) as i32;
    let mut discard = 0;
    loop {
        if data_size_at(format, header_size, width, height, discard)? <= bytes {
            break;
        }
        discard += 1;
        if discard > MAX_IMAGE_MIP {
            discard = -1;
            break;
        }
    }
    let discard = discard.min(miplevelmax);

    Ok(Info {
        format,
        header_size,
        width: w,
        height: h,
        components: components as u32,
        discard_level: discard,
    })
}

/// `LLImageDXT::decode`: the mip at the image's discard level, uncompressed formats only.
pub fn decode(data: &[u8], info: &Info) -> Result<Raster, Error> {
    if info.format.is_compressed() {
        return fail("Attempt to decode compressed LLImageDXT to Raw (unsupported)");
    }
    let mut width = info.width as i32;
    let mut height = info.height as i32;
    let offset = if info.discard_level >= 0 {
        let offset = mip_offset(
            info.format,
            info.header_size,
            width,
            height,
            info.discard_level,
        )?;
        calc_discard_width_height(info.discard_level, info.format, &mut width, &mut height);
        offset
    } else {
        mip_offset(info.format, info.header_size, width, height, 0)?
    };
    let image_size = format_bytes(info.format, width, height)?;
    if data.is_empty() || (offset as i64 + image_size as i64) > data.len() as i64 {
        return fail("LLImageDXT trying to decode an image with not enough data!");
    }
    let mut raster = Raster::new(width as u32, height as u32, info.components);
    let n = raster.data.len().min(image_size as usize);
    raster.data[..n].copy_from_slice(&data[offset as usize..offset as usize + n]);
    Ok(raster)
}

/// `LLImageDXT::getMipData`: the raster the viewer built for a discard level
/// (it starts its dimension computation from zero, so this is always a
/// single pixel at the mip's offset, as in the C++).
pub fn mip_data(data: &[u8], info: &Info, discard: i32) -> Result<Raster, Error> {
    let discard = if discard < 0 {
        info.discard_level
    } else {
        discard
    };
    if discard < info.discard_level {
        return fail("Request for invalid discard level");
    }
    let offset = mip_offset(
        info.format,
        info.header_size,
        info.width as i32,
        info.height as i32,
        discard,
    )?;
    let mut width = 0;
    let mut height = 0;
    calc_discard_width_height(discard, info.format, &mut width, &mut height);
    let mut raster = Raster::new(width as u32, height as u32, info.components);
    let n = raster.data.len();
    let Some(src) = data.get(offset as usize..offset as usize + n) else {
        return fail("LLImageDXT: not enough data for the requested mip");
    };
    raster.data.copy_from_slice(src);
    Ok(raster)
}

/// `LLImageDXT::encode`: a DDS-headered file with the full mip chain, mips
/// generated with the 2x2 box filter. The padding bytes are zero (the viewer
/// left them uninitialised).
pub fn encode(raster: &Raster) -> Result<Vec<u8>, Error> {
    let format = match raster.components {
        1 => Format::A8,
        3 => Format::Rgb8,
        4 => Format::Rgba8,
        n => return fail(format!("LLImageDXT::encode: Unhandled channel number: {n}")),
    };
    let ncomponents = raster.components as usize;
    let width = raster.width as i32;
    let height = raster.height as i32;
    if raster.data.len() < width as usize * height as usize * ncomponents {
        return fail("LLImageDXT: raster is shorter than its dimensions");
    }
    let nmips = calc_num_mips(width, height);
    let mut totbytes = HEADER_SIZE;
    let (mut w, mut h) = (width, height);
    for _ in 0..nmips {
        totbytes += format_bytes(format, w, h)?;
        w >>= 1;
        h >>= 1;
    }
    let mut out = vec![0u8; totbytes as usize];
    out[0..4].copy_from_slice(&DDS_FOURCC.to_le_bytes());
    out[12..16].copy_from_slice(&height.to_le_bytes());
    out[16..20].copy_from_slice(&width.to_le_bytes());
    out[28..32].copy_from_slice(&nmips.to_le_bytes());
    out[84..88].copy_from_slice(&format.fourcc().to_le_bytes());

    let (mut w, mut h) = (width, height);
    let mut prev: Option<(usize, usize)> = None; // offset and padded size of the previous mip
    for mip in 0..nmips {
        let offset = mip_offset(format, HEADER_SIZE, width, height, mip)? as usize;
        let bytes = format_bytes(format, w, h)? as usize;
        let pixels = w as usize * h as usize * ncomponents;
        if mip == 0 {
            out[offset..offset + pixels].copy_from_slice(&raster.data[..pixels]);
        } else {
            // Mips are stored smallest first: the previous (larger) mip lies
            // after this one.
            let (poff, pbytes) = prev.unwrap();
            let (head, tail) = out.split_at_mut(poff);
            if !generate_mip_into(
                &tail[..pbytes],
                &mut head[offset..offset + pixels],
                w as usize,
                h as usize,
                ncomponents,
            ) {
                return fail("LLImageDXT: mip generation failed");
            }
        }
        prev = Some((offset, bytes));
        w >>= 1;
        h >>= 1;
        check_min_width_height(format, &mut w, &mut h);
    }
    Ok(out)
}

/// `LLImageDXT::convertToDXR`: rewrite an old DXT-four-cc file as DXR with
/// the mip order reversed. `Ok(None)` when there is nothing to convert (the
/// viewer returned false), as for DXR files and unknown formats.
pub fn convert_to_dxr(data: &[u8], info: &Info) -> Result<Option<Vec<u8>>, Error> {
    use Format::*;
    let newformat = match info.format {
        Dxr1 | Dxr2 | Dxr3 | Dxr4 | Dxr5 => return Ok(None),
        Dxt1 => Dxr1,
        Dxt2 => Dxr2,
        Dxt3 => Dxr3,
        Dxt4 => Dxr4,
        Dxt5 => Dxr5,
        _ => return Ok(None),
    };
    let width = info.width as i32;
    let height = info.height as i32;
    let nmips = calc_num_mips(width, height);
    let total_bytes = data.len();
    let header_size = info.header_size as usize;
    if total_bytes < header_size {
        return fail("LLImageDXT: not enough data");
    }
    let mut newdata = vec![0u8; total_bytes];
    newdata[..header_size].copy_from_slice(&data[..header_size]);
    let (mut w, mut h) = (width, height);
    for mip in 0..nmips {
        let bytes = format_bytes(newformat, w, h)? as usize;
        let newoffset = mip_offset(newformat, info.header_size, width, height, mip)? as usize;
        let Some(oldoffset) = (total_bytes)
            .checked_sub(newoffset + bytes)
            .map(|r| header_size + r)
        else {
            return fail("LLImageDXT: not enough data to convert");
        };
        let (Some(src), true) = (
            data.get(oldoffset..oldoffset + bytes),
            newoffset + bytes <= total_bytes,
        ) else {
            return fail("LLImageDXT: not enough data to convert");
        };
        newdata[newoffset..newoffset + bytes].copy_from_slice(src);
        w >>= 1;
        h >>= 1;
    }
    if newdata.len() < 88 {
        return fail("LLImageDXT: not enough data to convert");
    }
    newdata[84..88].copy_from_slice(&newformat.fourcc().to_le_bytes());
    Ok(Some(newdata))
}
