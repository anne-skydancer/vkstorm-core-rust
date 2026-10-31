//! HE-AAC (v1/v2) and Opus decoding through the viewer's decoder-only LGPL
//! FFmpeg 9.0.2 (avcodec-63, avutil-61), loaded at run time from a
//! directory the engine configures.
//!
//! Only the handful of entry points the send/receive decode API needs are
//! resolved; the few struct fields read or written are addressed by their
//! offsets in FFmpeg 9.0.2's public headers (identical on Windows and
//! Linux x86-64), and the library's major version is checked at load so a
//! different FFmpeg is refused instead of misread.
//!
//! All `unsafe` of the stream module is here.

use std::collections::HashMap;
use std::ffi::{c_int, c_uint, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use libloading::Library;

use super::codec::{PcmFn, PcmSink, StreamError, default_layout};
use crate::music::ChannelLayout;

/// AVCodecID values.
const AV_CODEC_ID_AAC: c_int = 86018;
const AV_CODEC_ID_OPUS: c_int = 86076;
const AVERROR_EAGAIN: c_int = -11;
const AVERROR_EOF: c_int = -541_478_725;
const AV_LOG_ERROR: c_int = 16;
const AV_INPUT_BUFFER_PADDING_SIZE: usize = 64;
const AVCODEC_MAJOR: c_uint = 63;

// Sample formats.
const FMT_S16: c_int = 1;
const FMT_S32: c_int = 2;
const FMT_FLT: c_int = 3;
const FMT_S16P: c_int = 6;
const FMT_S32P: c_int = 7;
const FMT_FLTP: c_int = 8;

// Field offsets (FFmpeg 9.0.2, x86-64).
mod off {
    pub const FRAME_EXTENDED_DATA: usize = 96;
    pub const FRAME_NB_SAMPLES: usize = 112;
    pub const FRAME_FORMAT: usize = 116;
    pub const FRAME_SAMPLE_RATE: usize = 180;
    pub const FRAME_CH_LAYOUT: usize = 384;
    pub const CTX_EXTRADATA: usize = 72;
    pub const CTX_EXTRADATA_SIZE: usize = 80;
    pub const CTX_PKT_TIMEBASE: usize = 92;
    pub const PKT_PTS: usize = 8;
    pub const PKT_DTS: usize = 16;
    pub const PKT_DATA: usize = 24;
    pub const PKT_SIZE: usize = 32;
}

type FnVersion = unsafe extern "C" fn() -> c_uint;
type FnFindDecoder = unsafe extern "C" fn(c_int) -> *const c_void;
type FnAllocContext = unsafe extern "C" fn(*const c_void) -> *mut c_void;
type FnOpen2 = unsafe extern "C" fn(*mut c_void, *const c_void, *mut *mut c_void) -> c_int;
type FnFreeContext = unsafe extern "C" fn(*mut *mut c_void);
type FnSendPacket = unsafe extern "C" fn(*mut c_void, *const c_void) -> c_int;
type FnReceiveFrame = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;
type FnPacketAlloc = unsafe extern "C" fn() -> *mut c_void;
type FnPacketFree = unsafe extern "C" fn(*mut *mut c_void);
type FnFrameAlloc = unsafe extern "C" fn() -> *mut c_void;
type FnFrameFree = unsafe extern "C" fn(*mut *mut c_void);
type FnFrameUnref = unsafe extern "C" fn(*mut c_void);
type FnMallocz = unsafe extern "C" fn(usize) -> *mut c_void;
type FnLogSetLevel = unsafe extern "C" fn(c_int);

/// The loaded libraries and their entry points.
pub(crate) struct Ffmpeg {
    find_decoder: FnFindDecoder,
    alloc_context: FnAllocContext,
    open2: FnOpen2,
    free_context: FnFreeContext,
    send_packet: FnSendPacket,
    receive_frame: FnReceiveFrame,
    packet_alloc: FnPacketAlloc,
    packet_free: FnPacketFree,
    frame_alloc: FnFrameAlloc,
    frame_free: FnFrameFree,
    frame_unref: FnFrameUnref,
    mallocz: FnMallocz,
    // Kept loaded for the process lifetime (the function pointers above
    // point into them).
    _avutil: Library,
    _avcodec: Library,
}

// SAFETY: the FFmpeg functions used are thread-safe for distinct codec
// contexts; the struct only holds function pointers and library handles.
unsafe impl Send for Ffmpeg {}
unsafe impl Sync for Ffmpeg {}

#[cfg(windows)]
fn open_lib(path: &Path) -> Result<Library, String> {
    use libloading::os::windows::{
        LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    };
    // A full backslash path, and the DLL's own directory first for its
    // dependencies (avutil, swresample).
    let p: PathBuf = path.components().collect();
    // SAFETY: loading FFmpeg runs its DLL initialisers, which are benign.
    unsafe {
        libloading::os::windows::Library::load_with_flags(
            &p,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        )
        .map(Library::from)
        .map_err(|e| format!("{}: {e}", p.display()))
    }
}

#[cfg(not(windows))]
fn open_lib(path: &Path) -> Result<Library, String> {
    // SAFETY: as above; libavcodec finds its dependencies through its
    // RUNPATH ($ORIGIN).
    unsafe { Library::new(path).map_err(|e| format!("{}: {e}", path.display())) }
}

fn sym<T: Copy>(lib: &Library, name: &[u8]) -> Result<T, String> {
    // SAFETY: the caller names the function with its C signature as T.
    unsafe {
        lib.get::<T>(name)
            .map(|s| *s)
            .map_err(|e| format!("{}: {e}", String::from_utf8_lossy(&name[..name.len() - 1])))
    }
}

impl Ffmpeg {
    fn load(dir: &Path) -> Result<Ffmpeg, String> {
        let (util, codec) = if cfg!(windows) {
            ("avutil-61.dll", "avcodec-63.dll")
        } else {
            ("libavutil.so.61", "libavcodec.so.63")
        };
        let avutil = open_lib(&dir.join(util))?;
        let avcodec = open_lib(&dir.join(codec))?;
        let version: FnVersion = sym(&avcodec, b"avcodec_version\0")?;
        // SAFETY: avcodec_version takes no arguments.
        let v = unsafe { version() };
        if v >> 16 != AVCODEC_MAJOR {
            return Err(format!(
                "avcodec major version {} (need {AVCODEC_MAJOR})",
                v >> 16
            ));
        }
        let log_level: FnLogSetLevel = sym(&avutil, b"av_log_set_level\0")?;
        // SAFETY: a plain setter. Errors only: the decoders' warnings about
        // stream oddities are not useful in the viewer log.
        unsafe { log_level(AV_LOG_ERROR) };
        Ok(Ffmpeg {
            find_decoder: sym(&avcodec, b"avcodec_find_decoder\0")?,
            alloc_context: sym(&avcodec, b"avcodec_alloc_context3\0")?,
            open2: sym(&avcodec, b"avcodec_open2\0")?,
            free_context: sym(&avcodec, b"avcodec_free_context\0")?,
            send_packet: sym(&avcodec, b"avcodec_send_packet\0")?,
            receive_frame: sym(&avcodec, b"avcodec_receive_frame\0")?,
            packet_alloc: sym(&avcodec, b"av_packet_alloc\0")?,
            packet_free: sym(&avcodec, b"av_packet_free\0")?,
            frame_alloc: sym(&avutil, b"av_frame_alloc\0")?,
            frame_free: sym(&avutil, b"av_frame_free\0")?,
            frame_unref: sym(&avutil, b"av_frame_unref\0")?,
            mallocz: sym(&avutil, b"av_mallocz\0")?,
            _avutil: avutil,
            _avcodec: avcodec,
        })
    }
}

/// The FFmpeg in `dir`, loaded once per process (per directory); the error
/// is remembered too.
pub(crate) fn load(dir: &Path) -> Result<Arc<Ffmpeg>, String> {
    type Cache = Mutex<HashMap<PathBuf, Result<Arc<Ffmpeg>, String>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    cache
        .entry(dir.to_path_buf())
        .or_insert_with(|| Ffmpeg::load(dir).map(Arc::new))
        .clone()
}

unsafe fn put<T>(base: *mut c_void, offset: usize, v: T) {
    // SAFETY: the caller passes a live FFmpeg struct and a field offset of
    // that struct with type T.
    unsafe { (base.cast::<u8>().add(offset).cast::<T>()).write_unaligned(v) }
}

unsafe fn get<T>(base: *const c_void, offset: usize) -> T {
    // SAFETY: as for `put`.
    unsafe { (base.cast::<u8>().add(offset).cast::<T>()).read_unaligned() }
}

/// One FFmpeg audio decoder (AAC or Opus).
pub(crate) struct FfDecoder {
    ff: Arc<Ffmpeg>,
    ctx: *mut c_void,
    pkt: *mut c_void,
    frame: *mut c_void,
    planes: Vec<Vec<f32>>,
    pts: i64,
    name: &'static str,
    errors: u32,
}

// SAFETY: the decoder's FFmpeg objects are used only by the thread that
// owns the FfDecoder.
unsafe impl Send for FfDecoder {}

impl FfDecoder {
    fn open(
        ff: Arc<Ffmpeg>,
        id: c_int,
        extradata: Option<&[u8]>,
        name: &'static str,
    ) -> Result<FfDecoder, String> {
        // SAFETY: standard allocation and open sequence; every pointer is
        // checked, and Drop frees what was allocated.
        unsafe {
            let codec = (ff.find_decoder)(id);
            if codec.is_null() {
                return Err(format!("FFmpeg has no {name} decoder"));
            }
            let ctx = (ff.alloc_context)(codec);
            let pkt = (ff.packet_alloc)();
            let frame = (ff.frame_alloc)();
            let d = FfDecoder {
                ff: ff.clone(),
                ctx,
                pkt,
                frame,
                planes: Vec::new(),
                pts: 0,
                name,
                errors: 0,
            };
            if ctx.is_null() || pkt.is_null() || frame.is_null() {
                return Err("FFmpeg allocation failed".into());
            }
            if let Some(x) = extradata {
                let buf = (ff.mallocz)(x.len() + AV_INPUT_BUFFER_PADDING_SIZE);
                if buf.is_null() {
                    return Err("FFmpeg allocation failed".into());
                }
                std::ptr::copy_nonoverlapping(x.as_ptr(), buf.cast::<u8>(), x.len());
                put(ctx, off::CTX_EXTRADATA, buf);
                put(ctx, off::CTX_EXTRADATA_SIZE, x.len() as c_int);
            }
            if id == AV_CODEC_ID_OPUS {
                // Packet timestamps in 48 kHz samples (avoids a warning).
                put(ctx, off::CTX_PKT_TIMEBASE, [1 as c_int, 48000 as c_int]);
            }
            let r = (ff.open2)(ctx, codec, std::ptr::null_mut());
            if r < 0 {
                return Err(format!("FFmpeg {name} decoder failed to open ({r})"));
            }
            Ok(d)
        }
    }

    /// An AAC decoder for ADTS frames (header included: FFmpeg reads the
    /// configuration from it, and detects implicit SBR and PS).
    pub(crate) fn aac(ff: Arc<Ffmpeg>) -> Result<FfDecoder, String> {
        FfDecoder::open(ff, AV_CODEC_ID_AAC, None, "aac")
    }

    /// An Opus decoder; `opus_head` is the identification header packet.
    pub(crate) fn opus(ff: Arc<Ffmpeg>, opus_head: &[u8]) -> Result<FfDecoder, String> {
        FfDecoder::open(ff, AV_CODEC_ID_OPUS, Some(opus_head), "opus")
    }

    /// Decodes one packet; each decoded frame goes to `out` as planar
    /// f32 (planes, frames, rate, layout).
    pub(crate) fn decode(&mut self, data: &[u8], out: &mut PcmFn<'_>) -> Result<(), StreamError> {
        let size = c_int::try_from(data.len())
            .map_err(|_| StreamError::Failed("packet too large".into()))?;
        // SAFETY: the packet borrows `data` only for the duration of
        // avcodec_send_packet, which copies an unreferenced packet's data
        // (buf is NULL) into its own padded buffer.
        let r = unsafe {
            put(self.pkt, off::PKT_DATA, data.as_ptr());
            put(self.pkt, off::PKT_SIZE, size);
            put(self.pkt, off::PKT_PTS, self.pts);
            put(self.pkt, off::PKT_DTS, self.pts);
            let r = (self.ff.send_packet)(self.ctx, self.pkt);
            put(self.pkt, off::PKT_DATA, std::ptr::null::<u8>());
            put(self.pkt, off::PKT_SIZE, 0 as c_int);
            r
        };
        if r < 0 && r != AVERROR_EAGAIN {
            self.errors += 1;
            if self.errors > 50 {
                return Err(StreamError::Failed(format!(
                    "{}: undecodable stream ({r})",
                    self.name
                )));
            }
            return Ok(());
        }
        loop {
            // SAFETY: ctx and frame are live; the frame is unreferenced
            // after its samples were copied out.
            let r = unsafe { (self.ff.receive_frame)(self.ctx, self.frame) };
            if r == AVERROR_EAGAIN || r == AVERROR_EOF {
                break;
            }
            if r < 0 {
                self.errors += 1;
                if self.errors > 50 {
                    return Err(StreamError::Failed(format!(
                        "{}: undecodable stream ({r})",
                        self.name
                    )));
                }
                break;
            }
            self.errors = 0;
            let (frames, rate, layout) = unsafe { self.copy_frame() };
            // SAFETY: as above.
            unsafe { (self.ff.frame_unref)(self.frame) };
            self.pts += frames as i64;
            if frames > 0 && rate > 0 {
                let refs: Vec<&[f32]> = self.planes.iter().map(|p| &p[..frames]).collect();
                out(&refs, frames, rate, layout)?;
            }
        }
        Ok(())
    }

    /// Copies the received frame's samples into `planes` as f32.
    ///
    /// # Safety
    /// `self.frame` holds a frame returned by avcodec_receive_frame.
    unsafe fn copy_frame(&mut self) -> (usize, u32, ChannelLayout) {
        // SAFETY: field reads of a valid AVFrame; the sample pointers cover
        // nb_samples (x channels when interleaved) of the frame's format.
        unsafe {
            let f = self.frame;
            let frames = get::<c_int>(f, off::FRAME_NB_SAMPLES).max(0) as usize;
            let format = get::<c_int>(f, off::FRAME_FORMAT);
            let rate = get::<c_int>(f, off::FRAME_SAMPLE_RATE).max(0) as u32;
            let order = get::<c_int>(f, off::FRAME_CH_LAYOUT);
            let nch = get::<c_int>(f, off::FRAME_CH_LAYOUT + 4).clamp(0, 8) as usize;
            let mask = get::<u64>(f, off::FRAME_CH_LAYOUT + 8);
            // AV_CHANNEL_ORDER_NATIVE: the mask bits are the WAVE bits and
            // the planes are in ascending bit order.
            let layout =
                if order == 1 && mask.count_ones() as usize == nch && mask <= u64::from(u32::MAX) {
                    if nch == 1 {
                        ChannelLayout::MONO
                    } else {
                        ChannelLayout {
                            channels: nch as u16,
                            mask: mask as u32,
                        }
                    }
                } else {
                    default_layout(nch as u16)
                };
            let data = get::<*const *const u8>(f, off::FRAME_EXTENDED_DATA);
            self.planes.resize(nch, Vec::new());
            for p in &mut self.planes {
                p.clear();
                p.resize(frames, 0.0);
            }
            if data.is_null() || nch == 0 {
                return (0, rate, layout);
            }
            match format {
                FMT_FLTP | FMT_S16P | FMT_S32P => {
                    for (c, p) in self.planes.iter_mut().enumerate() {
                        let src = *data.add(c);
                        for (i, s) in p.iter_mut().enumerate() {
                            *s = match format {
                                FMT_FLTP => src.cast::<f32>().add(i).read_unaligned(),
                                FMT_S16P => {
                                    f32::from(src.cast::<i16>().add(i).read_unaligned()) / 32768.0
                                }
                                _ => {
                                    src.cast::<i32>().add(i).read_unaligned() as f32 / 2147483648.0
                                }
                            };
                        }
                    }
                }
                FMT_FLT | FMT_S16 | FMT_S32 => {
                    let src = *data;
                    for i in 0..frames {
                        for (c, p) in self.planes.iter_mut().enumerate() {
                            let k = i * nch + c;
                            p[i] = match format {
                                FMT_FLT => src.cast::<f32>().add(k).read_unaligned(),
                                FMT_S16 => {
                                    f32::from(src.cast::<i16>().add(k).read_unaligned()) / 32768.0
                                }
                                _ => {
                                    src.cast::<i32>().add(k).read_unaligned() as f32 / 2147483648.0
                                }
                            };
                        }
                    }
                }
                _ => return (0, rate, layout),
            }
            (frames, rate, layout)
        }
    }

    /// Decodes a packet into `sink` (the common case).
    pub(crate) fn decode_into(
        &mut self,
        data: &[u8],
        sink: &mut dyn PcmSink,
    ) -> Result<(), StreamError> {
        self.decode(data, &mut |planes, frames, rate, layout| {
            sink.pcm(planes, frames, rate, layout)
        })
    }
}

impl Drop for FfDecoder {
    fn drop(&mut self) {
        // SAFETY: each pointer is either null or owned by this decoder; the
        // free functions accept pointers to null.
        unsafe {
            if !self.frame.is_null() {
                (self.ff.frame_free)(&mut self.frame);
            }
            if !self.pkt.is_null() {
                (self.ff.packet_free)(&mut self.pkt);
            }
            if !self.ctx.is_null() {
                (self.ff.free_context)(&mut self.ctx);
            }
        }
    }
}
