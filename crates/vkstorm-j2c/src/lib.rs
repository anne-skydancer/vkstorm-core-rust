//! JPEG 2000 (J2C) texture decoding and encoding for Vulkanstorm.
//!
//! One codec for both directions: `vkstorm-openjp2`, the port of OpenJPEG owned
//! in this repository. This crate is the viewer-facing contract, mirroring what
//! `LLImageJ2COJ` needs:
//!
//! - [`metadata`]: header-only inspection (dimensions, component count).
//! - [`decode`]: decode at a discard level (resolution reduction), from a
//!   possibly truncated codestream (progressive fetch), selecting a channel
//!   range, producing 8-bit interleaved rows in the viewer's bottom-up order.
//! - [`comment`]: the codestream COM marker text the viewer surfaces as the
//!   texture comment.
//! - [`encode`]: the viewer's upload encoding (RLCP, 9/7 with layered rates or
//!   reversible 5/3, resolution rule for small images, COM comment).
//!
//! Everything crosses into the port through its C-shaped API with memory
//! streams; no file I/O.

use std::ffi::{c_void, CString};

use vkstorm_openjp2::openjpeg::*;

/// Codec name and version, for the viewer engine-info display.
pub const ENGINE_INFO: &str = "vkstorm-openjp2 2.5.2 (Rust)";

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
    /// Encoding failed or the input was invalid.
    Encode(String),
    /// The requested channel range selects no channels.
    NoChannels { components: u32, first_channel: u32 },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Header(e) => write!(f, "j2c header: {e}"),
            Error::Decode(e) => write!(f, "j2c decode: {e}"),
            Error::Encode(e) => write!(f, "j2c encode: {e}"),
            Error::NoChannels { components, first_channel } => write!(
                f,
                "j2c: no channels selected (components {components}, first channel {first_channel})"
            ),
        }
    }
}

impl std::error::Error for Error {}

// ---------------------------------------------------------------------------
// Memory streams over the port's stream callbacks.
// ---------------------------------------------------------------------------

struct ReadMem<'a> {
    data: &'a [u8],
    pos: usize,
}

unsafe extern "C" fn read_fn(buf: *mut c_void, n: usize, ud: *mut c_void) -> usize {
    let m = unsafe { &mut *(ud as *mut ReadMem) };
    if m.pos >= m.data.len() {
        return usize::MAX; // (OPJ_SIZE_T)-1: end of stream
    }
    let n = n.min(m.data.len() - m.pos);
    unsafe { std::ptr::copy_nonoverlapping(m.data.as_ptr().add(m.pos), buf as *mut u8, n) };
    m.pos += n;
    n
}

unsafe extern "C" fn read_skip_fn(n: i64, ud: *mut c_void) -> i64 {
    let m = unsafe { &mut *(ud as *mut ReadMem) };
    let new = (m.pos as i64 + n).clamp(0, m.data.len() as i64);
    let moved = new - m.pos as i64;
    m.pos = new as usize;
    moved
}

unsafe extern "C" fn read_seek_fn(n: i64, ud: *mut c_void) -> i32 {
    let m = unsafe { &mut *(ud as *mut ReadMem) };
    if n < 0 || n as usize > m.data.len() {
        return 0;
    }
    m.pos = n as usize;
    1
}

unsafe extern "C" fn no_free(_ud: *mut c_void) {}

/// In-memory sink for the encoder, shared so the bytes can be taken back
/// after the port's stream (which owns a boxed writer) is dropped.
#[derive(Clone, Default)]
struct SharedSink(std::rc::Rc<std::cell::RefCell<(Vec<u8>, usize)>>);

impl std::io::Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut g = self.0.borrow_mut();
        let (data, pos) = &mut *g;
        let end = *pos + buf.len();
        if end > data.len() {
            data.resize(end, 0);
        }
        data[*pos..end].copy_from_slice(buf);
        *pos = end;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl std::io::Seek for SharedSink {
    fn seek(&mut self, from: std::io::SeekFrom) -> std::io::Result<u64> {
        let mut g = self.0.borrow_mut();
        let (data, pos) = &mut *g;
        let new = match from {
            std::io::SeekFrom::Start(o) => o as i64,
            std::io::SeekFrom::Current(o) => *pos as i64 + o,
            std::io::SeekFrom::End(o) => data.len() as i64 + o,
        };
        if new < 0 {
            return Err(std::io::Error::other("seek before start"));
        }
        let new = new as usize;
        if new > data.len() {
            data.resize(new, 0);
        }
        *pos = new;
        Ok(new as u64)
    }
}

/// RAII for the port's codec, stream and image handles.
struct Handles {
    codec: *mut opj_codec_t,
    stream: *mut opj_stream_t,
    image: *mut opj_image_t,
}

impl Drop for Handles {
    fn drop(&mut self) {
        unsafe {
            if !self.image.is_null() {
                opj_image_destroy(self.image);
            }
            if !self.codec.is_null() {
                opj_destroy_codec(self.codec);
            }
            if !self.stream.is_null() {
                opj_stream_destroy(self.stream);
            }
        }
    }
}

const STREAM_CHUNK: usize = 1 << 16;

fn open_decoder<'a>(mem: &mut ReadMem<'a>, reduce: u32) -> Result<Handles, Error> {
    unsafe {
        let stream = opj_stream_create(STREAM_CHUNK, 1);
        opj_stream_set_read_function(stream, Some(read_fn));
        opj_stream_set_skip_function(stream, Some(read_skip_fn));
        opj_stream_set_seek_function(stream, Some(read_seek_fn));
        opj_stream_set_user_data(stream, mem as *mut ReadMem as *mut c_void, Some(no_free));
        opj_stream_set_user_data_length(stream, mem.data.len() as u64);

        let codec = opj_create_decompress(CODEC_FORMAT::OPJ_CODEC_J2K);
        let mut h = Handles { codec, stream, image: std::ptr::null_mut() };
        let mut params: opj_dparameters_t = std::mem::zeroed();
        opj_set_default_decoder_parameters(&mut params);
        params.cp_reduce = reduce;
        if opj_setup_decoder(codec, &mut params) == 0 {
            return Err(Error::Header("decoder setup failed".into()));
        }
        // Lenient: decode what is present, as the viewer always has.
        opj_decoder_set_strict_mode(codec, 0);
        if opj_read_header(stream, codec, &mut h.image) == 0 || h.image.is_null() {
            return Err(Error::Header("not a JPEG 2000 codestream".into()));
        }
        Ok(h)
    }
}

/// Parse the main header only. Equivalent to the viewer's `getMetadata`.
pub fn metadata(data: &[u8]) -> Result<Metadata, Error> {
    let mut mem = ReadMem { data, pos: 0 };
    let h = open_decoder(&mut mem, 0)?;
    let img = unsafe { &*h.image };
    Ok(Metadata {
        width: img.x1 - img.x0,
        height: img.y1 - img.y0,
        components: img.numcomps,
    })
}

/// Decode `data` (which may be truncated: only the first `max_bytes` bytes are
/// used when `max_bytes` is non-zero and smaller than the slice) at
/// `discard_level`, keeping at most `max_channels` channels starting at
/// `first_channel`.
///
/// Output rows are bottom-up, matching how `LLImageJ2COJ::decodeImpl` fills
/// `LLImageRaw`; samples above 8 bits are shifted down, signed samples offset.
pub fn decode(
    data: &[u8],
    max_bytes: usize,
    discard_level: u32,
    first_channel: u32,
    max_channels: u32,
) -> Result<Decoded, Error> {
    let data = if max_bytes > 0 && max_bytes < data.len() { &data[..max_bytes] } else { data };
    let mut mem = ReadMem { data, pos: 0 };
    let h = open_decoder(&mut mem, discard_level)?;
    unsafe {
        let components = (*h.image).numcomps;
        let channels = components.saturating_sub(first_channel).min(max_channels);
        if channels == 0 {
            return Err(Error::NoChannels { components, first_channel });
        }
        run_decode(&h, discard_level)?;
        let (width, height, data) = convert(&*h.image, first_channel as usize, channels as usize)?;
        Ok(Decoded { width, height, channels, components, data })
    }
}

/// Dimensions of a decode without the samples, from [`decode_only`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
    pub components: u32,
}

/// Run the codec alone and discard the samples. This times the same work as
/// an OpenJPEG `opj_decode` call, for like-for-like benchmarks against the C
/// library; [`decode`] adds the conversion to the viewer's 8-bit layout.
pub fn decode_only(data: &[u8], max_bytes: usize, discard_level: u32) -> Result<Dimensions, Error> {
    let data = if max_bytes > 0 && max_bytes < data.len() { &data[..max_bytes] } else { data };
    let mut mem = ReadMem { data, pos: 0 };
    let h = open_decoder(&mut mem, discard_level)?;
    unsafe {
        run_decode(&h, discard_level)?;
        let img = &*h.image;
        let comps = std::slice::from_raw_parts(img.comps, img.numcomps as usize);
        Ok(Dimensions { width: comps[0].w, height: comps[0].h, components: img.numcomps })
    }
}

/// Apply the discard level and decode the codestream into the image planes.
unsafe fn run_decode(h: &Handles, discard_level: u32) -> Result<(), Error> {
    unsafe {
        if discard_level > 0 && opj_set_decoded_resolution_factor(h.codec, discard_level) == 0 {
            return Err(Error::Decode(format!("discard level {discard_level} not available")));
        }
        if opj_decode(h.codec, h.stream, h.image) == 0 {
            return Err(Error::Decode("decode failed".into()));
        }
        opj_end_decompress(h.codec, h.stream);
    }
    Ok(())
}

/// Interleave `c` planes starting at component `fc` into 8-bit bottom-up rows.
/// Samples above 8 bits are shifted down, signed samples offset, and the
/// result clamped, as `LLImageJ2COJ::decodeImpl` does.
unsafe fn convert(img: &opj_image_t, fc: usize, c: usize) -> Result<(u32, u32, Vec<u8>), Error> {
    let comps = unsafe { std::slice::from_raw_parts(img.comps, img.numcomps as usize) };
    let (w, h) = (comps[0].w as usize, comps[0].h as usize);
    let mut out = vec![0u8; w * h * c];
    for (k, comp) in comps[fc..fc + c].iter().enumerate() {
        if comp.data.is_null() {
            return Err(Error::Decode("component without data".into()));
        }
        if comp.w as usize != w || comp.h as usize != h {
            return Err(Error::Decode("subsampled component".into()));
        }
        let d = unsafe { std::slice::from_raw_parts(comp.data, w * h) };
        let prec = comp.prec;
        let offset = if comp.sgnd != 0 { 1i32 << (prec - 1) } else { 0 };
        let (shr, shl) = if prec > 8 { (prec - 8, 0) } else { (0, 8 - prec) };
        for y in 0..h {
            let src = &d[y * w..(y + 1) * w];
            let dst = &mut out[(h - 1 - y) * w * c..(h - y) * w * c];
            debug_assert!(src.len() == w && dst.len() == w * c);
            // SAFETY: src has w samples and dst w * c bytes, so x < w and
            // k < c keep both indices in range; the checked slicing above is
            // what the loop relies on.
            unsafe {
                let s = src.as_ptr();
                let o = dst.as_mut_ptr().add(k);
                for x in 0..w {
                    let v = ((*s.add(x) + offset) >> shr) << shl;
                    *o.add(x * c) = v.clamp(0, 255) as u8;
                }
            }
        }
    }
    Ok((w as u32, h as u32, out))
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

// ---------------------------------------------------------------------------
// Encoding (uploads), mirroring the parameters of LLImageJ2COJ::encodeImpl.
// ---------------------------------------------------------------------------

/// Final compression ratio of the highest-quality layer (LAST_TCP_RATE).
pub const LAST_COMPRESSION_RATIO: u32 = 8;

/// Number of quality layers for a given surface, as the viewer picks it.
pub fn layers_for_surface(surface: u32) -> u32 {
    match surface {
        0..=1024 => 2,
        1025..=16384 => 3,
        16385..=262144 => 4,
        262145..=1048576 => 5,
        _ => 6,
    }
}

/// Compression ratios from lowest to highest quality: 8 * 4^i descending.
pub fn compression_ratios(layers: u32) -> Vec<u32> {
    (0..layers).rev().map(|i| LAST_COMPRESSION_RATIO * (1u32 << (2 * i))).collect()
}

/// Number of resolution levels the viewer requests for small images (either
/// dimension under 64 px): 1 + floor(log2(min_dim)). None keeps the encoder
/// default of 6.
pub fn resolutions_for(width: u32, height: u32) -> Option<u32> {
    const MIN_IMAGE_SIZE: u32 = 4;
    let w_tiles = if width >> 6 == 0 && width >= MIN_IMAGE_SIZE { 1 } else { width >> 6 };
    let h_tiles = if height >> 6 == 0 && height >= MIN_IMAGE_SIZE { 1 } else { height >> 6 };
    if w_tiles == 1 || h_tiles == 1 {
        let min_dim = width.min(height).max(1);
        Some(1 + min_dim.ilog2())
    } else {
        None
    }
}

/// Encode an 8-bit image given as interleaved, bottom-up rows (the LLImageRaw
/// layout) with `channels` components, into a raw J2K codestream.
///
/// `reversible` selects lossless 5/3 coding with a single layer; otherwise
/// 9/7 lossy coding with the viewer layer and ratio rules. `comment` goes
/// into the COM marker, defaulting to "no comment" as the viewer does.
pub fn encode(
    data: &[u8],
    width: u32,
    height: u32,
    channels: u32,
    comment: Option<&str>,
    reversible: bool,
) -> Result<Vec<u8>, Error> {
    let (w, h, c) = (width as usize, height as usize, channels as usize);
    if w == 0 || h == 0 || !(1..=4).contains(&c) || data.len() < w * h * c {
        return Err(Error::Encode(format!("invalid raw image {width}x{height}x{channels} ({} bytes)", data.len())));
    }
    let comment_c = CString::new(comment.unwrap_or("no comment").replace('\0', " "))
        .map_err(|e| Error::Encode(e.to_string()))?;

    unsafe {
        let mut params: opj_cparameters_t = std::mem::zeroed();
        opj_set_default_encoder_parameters(&mut params);
        params.cod_format = 0; // OPJ_CODEC_J2K: raw codestream
        params.prog_order = OPJ_RLCP;
        params.cp_disto_alloc = 1;
        params.max_cs_size = 0;
        if reversible {
            params.irreversible = 0;
            params.tcp_numlayers = 1;
            params.tcp_rates[0] = 0.0;
        } else {
            params.irreversible = 1;
            let ratios = compression_ratios(layers_for_surface((w * h) as u32));
            params.tcp_numlayers = ratios.len() as i32;
            for (i, r) in ratios.iter().enumerate() {
                params.tcp_rates[i] = *r as f32;
            }
        }
        params.cp_comment = comment_c.as_ptr() as *mut _;
        params.tcp_mct = if channels >= 3 { 1 } else { 0 };
        if let Some(res) = resolutions_for(width, height) {
            params.numresolution = res as i32;
        }

        // Components: 8-bit unsigned, no subsampling, top-down rows.
        let mut cmptparm: Vec<vkstorm_openjp2::opj_image_comptparm> = (0..c)
            .map(|_| {
                let mut p: vkstorm_openjp2::opj_image_comptparm = std::mem::zeroed();
                p.prec = 8;
                p.sgnd = 0;
                p.dx = params.subsampling_dx as u32;
                p.dy = params.subsampling_dy as u32;
                p.w = width;
                p.h = height;
                p
            })
            .collect();
        let image = opj_image_create(c as u32, cmptparm.as_mut_ptr(), COLOR_SPACE::OPJ_CLRSPC_SRGB);
        if image.is_null() {
            return Err(Error::Encode("image allocation failed".into()));
        }
        let mut hnd = Handles { codec: std::ptr::null_mut(), stream: std::ptr::null_mut(), image };
        {
            let img = &mut *image;
            img.x0 = 0;
            img.y0 = 0;
            img.x1 = width;
            img.y1 = height;
            let comps = std::slice::from_raw_parts_mut(img.comps, c);
            for (ci, comp) in comps.iter_mut().enumerate() {
                let d = std::slice::from_raw_parts_mut(comp.data, w * h);
                let mut i = 0;
                for y in (0..h).rev() {
                    let row = &data[y * w * c..(y + 1) * w * c];
                    for x in 0..w {
                        d[i] = i32::from(row[x * c + ci]);
                        i += 1;
                    }
                }
            }
        }

        hnd.codec = opj_create_compress(CODEC_FORMAT::OPJ_CODEC_J2K);
        let setup_ok = opj_setup_encoder(hnd.codec, &mut params, image) != 0;
        // The port's opj_cparameters frees cp_comment on drop (C leaves it to the
        // caller); the encoder copied it in setup, so detach our CString pointer.
        params.cp_comment = std::ptr::null_mut();
        if !setup_ok {
            return Err(Error::Encode("encoder setup failed".into()));
        }

        let sink = SharedSink::default();
        sink.0.borrow_mut().0.reserve(w * h * c / 4 + 1024);
        {
            let mut stream = vkstorm_openjp2::Stream::new_writer(STREAM_CHUNK, sink.clone());
            let sp = &mut stream as *mut vkstorm_openjp2::Stream as *mut opj_stream_t;
            if opj_start_compress(hnd.codec, image, sp) == 0
                || opj_encode(hnd.codec, sp) == 0
                || opj_end_compress(hnd.codec, sp) == 0
            {
                return Err(Error::Encode("encoding failed".into()));
            }
            // dropping the stream flushes its buffer into the sink
        }
        drop(hnd);
        let bytes = std::mem::take(&mut sink.0.borrow_mut().0);
        Ok(bytes)
    }
}
