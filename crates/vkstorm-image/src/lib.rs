//! Raster image codecs for Vulkanstorm other than JPEG 2000.
//!
//! Each module mirrors one `LLImageFormatted` subclass of the C++ viewer and
//! keeps its contract: rasters are 8-bit samples, `components` interleaved per
//! pixel, rows ordered bottom-up as `LLImageRaw` stores them; the header
//! functions read dimensions and the component count without decoding; the
//! error messages are the ones the C++ classes reported.
//!
//! - [`png`]: libpng-equivalent decode (palette and low-depth expansion, gray
//!   to RGB, tRNS to alpha, 16-bit strip, gamma to a 2.2 screen) and encode.
//! - [`jpeg`] (feature `jpeg`): decode to RGB and baseline encode at a
//!   quality setting. The candidate measured against libjpeg-turbo; the viewer
//!   keeps libjpeg-turbo.
//! - [`tga`]: the viewer's TGA reader (true-colour, monochrome and colour-
//!   mapped, raw and run-length) and writer, plus the alpha-gradient
//!   processing used for avatar masks.
//! - [`bmp`]: Windows bitmaps, 8/24/32 bit, and the 8/24-bit writer.
//! - [`dxt`]: the viewer's mipped DXT/DDS container.
//!
//! Where the C++ read or wrote outside its buffers on malformed input, the
//! port returns an error instead; valid input decodes byte for byte the same.

pub mod bmp;
pub mod dxt;
#[cfg(feature = "jpeg")]
pub mod jpeg;
mod mip;
pub mod png;
pub mod tga;

pub use mip::{generate_mip, generate_mip_into};

/// An uncompressed image: 8-bit samples, `components` per pixel, rows
/// bottom-up (`LLImageRaw` order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub components: u32,
    pub data: Vec<u8>,
}

impl Raster {
    /// A zero-filled raster.
    pub fn new(width: u32, height: u32, components: u32) -> Raster {
        let size = width as usize * height as usize * components as usize;
        Raster {
            width,
            height,
            components,
            data: vec![0; size],
        }
    }

    /// Bytes per row.
    pub fn row_bytes(&self) -> usize {
        self.width as usize * self.components as usize
    }

    /// `LLImageRaw::verticalFlip`: reverse the row order in place.
    pub fn vertical_flip(&mut self) {
        let row = self.row_bytes();
        if row == 0 {
            return;
        }
        let h = self.height as usize;
        for y in 0..h / 2 {
            let (a, b) = self.data.split_at_mut((h - 1 - y) * row);
            a[y * row..y * row + row].swap_with_slice(&mut b[..row]);
        }
    }

    /// The same image without its fourth channel (`LLImageRaw::copyUnscaled4onto3`).
    pub fn drop_alpha(&self) -> Raster {
        debug_assert_eq!(self.components, 4);
        let mut out = Vec::with_capacity(self.width as usize * self.height as usize * 3);
        for px in self.data.chunks_exact(4) {
            out.extend_from_slice(&px[..3]);
        }
        Raster {
            width: self.width,
            height: self.height,
            components: 3,
            data: out,
        }
    }
}

/// What a header read yields: the decoded raster's dimensions and channel count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub components: u32,
}

/// A codec failure, carrying the message the viewer logs (`setLastError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Error {
        Error {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

pub(crate) fn fail<T>(message: impl Into<String>) -> Result<T, Error> {
    Err(Error::new(message))
}

/// The viewer's U16 dimension limit (`LLImageBase` stores width and height as U16).
pub(crate) fn check_dimensions(width: i64, height: i64, what: &str) -> Result<(u32, u32), Error> {
    if width < 0 || height < 0 || width > u16::MAX as i64 || height > u16::MAX as i64 {
        return fail(format!(
            "{what}: unsupported image dimensions {width}x{height}"
        ));
    }
    Ok((width as u32, height as u32))
}
