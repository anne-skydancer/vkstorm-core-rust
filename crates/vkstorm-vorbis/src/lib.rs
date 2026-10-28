//! Ogg Vorbis decoding for Vulkanstorm: a port of the decoder of libogg
//! 1.3.5 and libvorbis 1.3.7 (with libvorbisfile's seekable reader), whose
//! output is bit for bit that of the C library as the viewer uses it.
//!
//! - [`ogg`]: page sync, logical stream reassembly ([`ogg::SyncState`],
//!   [`ogg::StreamState`], [`ogg::Page`], [`ogg::Packet`]), for streams
//!   such as internet radio (chained links: a new serial number starts a
//!   new [`Decoder`]).
//! - [`Decoder`]: the packet API (`vorbis_synthesis_headerin`,
//!   `vorbis_synthesis`/`_blockin`, `_pcmout`, `_read`, `_restart`),
//!   per-channel `f32` output exactly as `vorbis_synthesis_pcmout`.
//! - [`VorbisFile`]: `ov_open_callbacks` over an in-memory file with the
//!   viewer's seekable callbacks, `ov_pcm_total`, `ov_info`, `ov_comment`
//!   and `ov_read` (return values, holes, chained links, 8/16-bit packing).
//!
//! # Bit exactness
//! Every float operation keeps the C's order and precision (float vs
//! double, constants, no fused multiply-add; Rust never contracts, and
//! MSVC `/fp:precise` and GCC without `-mfma` do not either). Where the C
//! builds tables or curves with libm (`cos`/`sin` for the MDCT twiddles;
//! `atan`, `cos`, `exp`, `sqrt` for floor 0), the same double-precision
//! libm functions are called, which Rust links from the platform C runtime
//! (the UCRT on Windows, glibc on Linux), as the C library does.
//!
//! Two platform differences of the C library are reproduced: with MSVC
//! `M_PI` is `os.h`'s float `3.1415926536f` (MSVC's `math.h` has no
//! `M_PI` without `_USE_MATH_DEFINES`), so MDCT twiddle and floor 0
//! arguments are computed in float there and in double with glibc; and C
//! `long` is 32-bit on Windows, which changes some header checks and
//! granule arithmetic. The Windows and Linux builds of the C library
//! therefore decode some streams differently in the last bit, and this
//! crate matches the one of the platform it is compiled for.
//!
//! One thing is not reproduced: the sign bit of NaN samples. Malformed
//! floor 0 data can drive `exp`/`sqrt` out of range; the port yields NaN
//! at exactly the samples the C does, but Rust gives no guarantee about NaN
//! sign and payload (LLVM may commute or rewrite the operations that
//! propagate them), and in practice some come out positive where the C's
//! are negative. Integer output (`ov_read`) is unaffected: every NaN
//! converts to `INT_MIN` and clips to -32768 (8-bit: -128), as in the C.
//!
//! Where the C crashes or hangs on malicious input (a zero-dimension
//! codebook, decoding after a failed codebook setup), this port returns an
//! error or skips the step instead; such streams are not bit-compared.
//!
//! Original code: Copyright (c) 1994-2020 Xiph.Org Foundation,
//! BSD-3-Clause (see `LICENSE-xiph`). Rust port: Vulkanstorm.

#![forbid(unsafe_code)]
// The port keeps the C's literals (float constants and tables verbatim,
// `M_PI` as `os.h` spells it), its statement shapes (`p*=p*(...)`) and its
// indexed loops, so that it can be audited against the C line by line.
#![allow(
    clippy::excessive_precision,
    clippy::approx_constant,
    clippy::misrefactored_assign_op,
    clippy::needless_range_loop,
    clippy::manual_range_contains,
    clippy::too_many_arguments
)]

mod bitpack;
mod codebook;
mod decoder;
mod dsp;
mod file;
mod floor;
mod info;
mod mdct;
pub mod ogg;
mod residue;
mod window;

pub use decoder::{Decoder, PcmOut};
pub use file::VorbisFile;
pub use info::{Comment, Info, synthesis_idheader};

/// Not true, or no data available.
pub const OV_FALSE: i32 = -1;
/// End of file.
pub const OV_EOF: i32 = -2;
/// A gap (lost page or corrupt packet) in the data; decoding may continue.
pub const OV_HOLE: i32 = -3;
/// A read from the media failed.
pub const OV_EREAD: i32 = -128;
/// Internal logic fault (or a corrupt stream state).
pub const OV_EFAULT: i32 = -129;
/// Feature not implemented.
pub const OV_EIMPL: i32 = -130;
/// Invalid argument or call sequence.
pub const OV_EINVAL: i32 = -131;
/// Not Vorbis data.
pub const OV_ENOTVORBIS: i32 = -132;
/// Invalid Vorbis header.
pub const OV_EBADHEADER: i32 = -133;
/// Vorbis version mismatch.
pub const OV_EVERSION: i32 = -134;
/// Not an audio packet.
pub const OV_ENOTAUDIO: i32 = -135;
/// Invalid packet.
pub const OV_EBADPACKET: i32 = -136;
/// Invalid stream section, or a corrupt link.
pub const OV_EBADLINK: i32 = -137;
/// The stream is not seekable.
pub const OV_ENOSEEK: i32 = -138;

/// The libvorbis error codes as an enum ([`Error::code`] gives the C value).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Error {
    /// `OV_FALSE`
    False,
    /// `OV_EOF`
    Eof,
    /// `OV_HOLE`
    Hole,
    /// `OV_EREAD`
    Read,
    /// `OV_EFAULT`
    Fault,
    /// `OV_EIMPL`
    Impl,
    /// `OV_EINVAL`
    Inval,
    /// `OV_ENOTVORBIS`
    NotVorbis,
    /// `OV_EBADHEADER`
    BadHeader,
    /// `OV_EVERSION`
    Version,
    /// `OV_ENOTAUDIO`
    NotAudio,
    /// `OV_EBADPACKET`
    BadPacket,
    /// `OV_EBADLINK`
    BadLink,
    /// `OV_ENOSEEK`
    NoSeek,
}

impl Error {
    /// The C error code.
    pub fn code(self) -> i32 {
        match self {
            Error::False => OV_FALSE,
            Error::Eof => OV_EOF,
            Error::Hole => OV_HOLE,
            Error::Read => OV_EREAD,
            Error::Fault => OV_EFAULT,
            Error::Impl => OV_EIMPL,
            Error::Inval => OV_EINVAL,
            Error::NotVorbis => OV_ENOTVORBIS,
            Error::BadHeader => OV_EBADHEADER,
            Error::Version => OV_EVERSION,
            Error::NotAudio => OV_ENOTAUDIO,
            Error::BadPacket => OV_EBADPACKET,
            Error::BadLink => OV_EBADLINK,
            Error::NoSeek => OV_ENOSEEK,
        }
    }

    /// The error for a C code (`None` for 0 and unknown values).
    pub fn from_code(code: i32) -> Option<Self> {
        Some(match code {
            OV_FALSE => Error::False,
            OV_EOF => Error::Eof,
            OV_HOLE => Error::Hole,
            OV_EREAD => Error::Read,
            OV_EFAULT => Error::Fault,
            OV_EIMPL => Error::Impl,
            OV_EINVAL => Error::Inval,
            OV_ENOTVORBIS => Error::NotVorbis,
            OV_EBADHEADER => Error::BadHeader,
            OV_EVERSION => Error::Version,
            OV_ENOTAUDIO => Error::NotAudio,
            OV_EBADPACKET => Error::BadPacket,
            OV_EBADLINK => Error::BadLink,
            OV_ENOSEEK => Error::NoSeek,
            _ => return None,
        })
    }

    /// `Ok` for a non-negative C result, the error otherwise.
    pub(crate) fn check(code: i32) -> Result<(), Error> {
        if code >= 0 {
            Ok(())
        } else {
            Err(Error::from_code(code).unwrap_or(Error::Fault))
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} ({})", self, self.code())
    }
}

impl std::error::Error for Error {}

/// A value as the C `long` of the platform holds it: 32-bit (wrapping) on
/// Windows (LLP64), 64-bit elsewhere.
#[inline(always)]
pub(crate) fn clong(v: i64) -> i64 {
    #[cfg(windows)]
    {
        v as i32 as i64
    }
    #[cfg(not(windows))]
    {
        v
    }
}

/// Internal tables, exposed for the parity harness (`tools/vorbis-oracle`).
#[doc(hidden)]
pub mod __internal {
    /// The `mdct_init` trig table for size `n`.
    pub fn mdct_trig(n: usize) -> Vec<f32> {
        crate::mdct::MdctLookup::new(n).trig().to_vec()
    }
    /// The half window `_vorbis_window_get(n)`.
    pub fn window(n: i32) -> &'static [f32] {
        crate::window::window(n)
    }
}
