//! JPEG 2000 (J2C) texture decoding for Vulkanstorm.
//!
//! Wraps `hayro-jpeg2000` behind the contract the viewer's texture pipeline
//! needs, mirroring `LLImageJ2COJ` so the two can be compared sample for sample:
//!
//! - [`metadata`]: header-only inspection (dimensions, component count).
//! - [`decode`]: decode at a discard level (resolution reduction), from a
//!   possibly truncated codestream (progressive fetch), selecting a channel
//!   range, producing 8-bit interleaved rows in the viewer's bottom-up order.
//! - [`comment`]: the codestream COM marker text the viewer surfaces as the
//!   texture comment.
//!
//! The decoder choice and the truncation change it depends on are documented
//! in the vkstorm-aleph study (`07-j2c-decoder-evaluation.md`) and measured by
//! `tools/j2c-eval` in this repository.

use hayro_jpeg2000::{DecodeSettings, DecoderContext, Image};

/// Decoder name and version, for the viewer's engine-info display.
pub const ENGINE_INFO: &str = "hayro-jpeg2000 0.4.1 (Rust)";

/// Header-level facts about a codestream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metadata {
    pub width: u32,
    pub height: u32,
    /// Number of image components (channels) in the codestream.
    pub components: u32,
}

/// A decoded image: 8-bit samples, `channels` interleaved per pixel, rows
/// ordered bottom-up as `LLImageRaw` stores them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub channels: u32,
    /// Components present in the codestream (before channel selection).
    pub components: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The codestream header could not be parsed.
    Header(String),
    /// The codestream parsed but decoding failed.
    Decode(String),
    /// The requested channel range selects no channels.
    NoChannels { components: u32, first_channel: u32 },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Header(e) => write!(f, "j2c header: {e}"),
            Error::Decode(e) => write!(f, "j2c decode: {e}"),
            Error::NoChannels { components, first_channel } => write!(
                f,
                "j2c: no channels selected (components {components}, first channel {first_channel})"
            ),
        }
    }
}

impl std::error::Error for Error {}

fn settings(target: Option<(u32, u32)>) -> DecodeSettings {
    let mut s = DecodeSettings::default();
    // Lenient, like `opj_decoder_set_strict_mode(false)`: decode what is present.
    s.strict = false;
    s.target_resolution = target;
    s
}

fn components_of(img: &Image<'_>) -> u32 {
    u32::from(img.color_space().num_channels()) + u32::from(img.has_alpha())
}

/// Parse the main header only. Equivalent to the viewer's `getMetadata`.
pub fn metadata(data: &[u8]) -> Result<Metadata, Error> {
    let img = Image::new(data, &settings(None)).map_err(|e| Error::Header(format!("{e:?}")))?;
    Ok(Metadata { width: img.width(), height: img.height(), components: components_of(&img) })
}

/// Reduced dimension at a discard level, as JPEG 2000 defines it
/// (ceil division by 2^level), matching OpenJPEG's component size.
fn reduced(dim: u32, discard: u32) -> u32 {
    dim.div_ceil(1u32 << discard)
}

/// Decode `data` (which may be truncated: only the first `max_bytes` bytes are
/// used when `max_bytes` is non-zero and smaller than the slice) at
/// `discard_level`, keeping at most `max_channels` channels starting at
/// `first_channel`.
///
/// Output rows are bottom-up, matching how `LLImageJ2COJ::decodeImpl` fills
/// `LLImageRaw`. Samples are converted to 8 bits the way hayro converts them.
pub fn decode(
    data: &[u8],
    max_bytes: usize,
    discard_level: u32,
    first_channel: u32,
    max_channels: u32,
) -> Result<Decoded, Error> {
    let data = if max_bytes > 0 && max_bytes < data.len() { &data[..max_bytes] } else { data };

    let probe = Image::new(data, &settings(None)).map_err(|e| Error::Header(format!("{e:?}")))?;
    let (full_w, full_h) = (probe.width(), probe.height());
    let components = components_of(&probe);

    let channels = components.saturating_sub(first_channel).min(max_channels);
    if channels == 0 {
        return Err(Error::NoChannels { components, first_channel });
    }

    let target = (discard_level > 0).then(|| (reduced(full_w, discard_level), reduced(full_h, discard_level)));
    let img = match target {
        Some(t) => Image::new(data, &settings(Some(t))).map_err(|e| Error::Header(format!("{e:?}")))?,
        None => probe,
    };

    let mut ctx = DecoderContext::default();
    let decoded = img.decode(&mut ctx).map_err(|e| Error::Decode(format!("{e:?}")))?;
    let n_comp = decoded.components().len() as u32;
    let interleaved = decoded.data_u8();

    // hayro reports full-size dimensions on `Image`; derive the decoded size
    // from the sample count, which is exact for power-of-two reductions.
    let samples = (interleaved.len() as u32) / n_comp.max(1);
    let (width, height) = match target {
        Some((w, h)) if w * h == samples => (w, h),
        None if full_w * full_h == samples => (full_w, full_h),
        _ => return Err(Error::Decode(format!("unexpected sample count {samples}"))),
    };

    let (w, h, c, nc) = (width as usize, height as usize, channels as usize, n_comp as usize);
    let fc = first_channel as usize;
    let mut out = vec![0u8; w * h * c];
    for y in 0..h {
        let src_row = &interleaved[y * w * nc..(y + 1) * w * nc];
        let dst_row = &mut out[(h - 1 - y) * w * c..(h - y) * w * c];
        for x in 0..w {
            dst_row[x * c..x * c + c].copy_from_slice(&src_row[x * nc + fc..x * nc + fc + c]);
        }
    }

    Ok(Decoded { width, height, channels, components, data: out })
}

/// The COM marker comment in the first 1024 bytes, with the viewer's sanity
/// rules (`LLImageJ2COJ::decodeImpl`): length at most 200, must fit in the
/// data, and the next marker must start with 0xFF.
pub fn comment(data: &[u8]) -> Option<String> {
    let limit = data.len().saturating_sub(7).min(1024);
    let mut pos = 0;
    while pos < limit {
        if data[pos] == 0xff && data[pos + 1] == 0x64 {
            let len = usize::from(data[pos + 2]) * 256 + usize::from(data[pos + 3]);
            if len > 200 || len < 4 || pos + 2 + len > data.len() {
                return None;
            }
            if pos + 2 + len < data.len() && data[pos + 2 + len] != 0xff {
                return None;
            }
            return Some(String::from_utf8_lossy(&data[pos + 6..pos + 2 + len]).into_owned());
        }
        pos += 1;
    }
    None
}
