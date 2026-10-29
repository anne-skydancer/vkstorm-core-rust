//! The port driven through its raw (OpenJPEG-shaped) API exactly as
//! `c/opj_bench.c` drives the fork, for the paths the viewer API does not
//! reach: decoding every tile in turn with `opj_get_decoded_tile`, and a
//! decode area with `opj_set_decode_area`. Output is the same 8-bit
//! interleaved layout the C harness writes, so `compare` applies.

use std::ffi::c_void;
use vkstorm_openjp2::openjpeg::*;

pub enum Mode {
    Whole,
    Tiles,
    Area(i32, i32, i32, i32),
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        if s == "tiles" {
            return Some(Mode::Tiles);
        }
        let v = s.strip_prefix("area=")?;
        let n: Vec<i32> = v
            .split(',')
            .map(|x| x.parse().ok())
            .collect::<Option<_>>()?;
        if n.len() != 4 {
            return None;
        }
        Some(Mode::Area(n[0], n[1], n[2], n[3]))
    }
}

struct Mem<'a> {
    data: &'a [u8],
    pos: usize,
}

unsafe extern "C" fn mem_read(buf: *mut c_void, n: usize, ud: *mut c_void) -> usize {
    let m = unsafe { &mut *(ud as *mut Mem) };
    if m.pos >= m.data.len() {
        return usize::MAX;
    }
    let n = n.min(m.data.len() - m.pos);
    unsafe { std::ptr::copy_nonoverlapping(m.data.as_ptr().add(m.pos), buf as *mut u8, n) };
    m.pos += n;
    n
}

unsafe extern "C" fn mem_skip(n: i64, ud: *mut c_void) -> i64 {
    let m = unsafe { &mut *(ud as *mut Mem) };
    let new = (m.pos as i64 + n).clamp(0, m.data.len() as i64);
    let moved = new - m.pos as i64;
    m.pos = new as usize;
    moved
}

unsafe extern "C" fn mem_seek(n: i64, ud: *mut c_void) -> i32 {
    let m = unsafe { &mut *(ud as *mut Mem) };
    if n < 0 || n as usize > m.data.len() {
        return 0;
    }
    m.pos = n as usize;
    1
}

unsafe extern "C" fn no_free(_ud: *mut c_void) {}

/// 8-bit interleaved rows (component 0 dimensions), appended to `out`.
unsafe fn write_image(img: *const opj_image_t, out: &mut Vec<u8>) -> (u32, u32, u32) {
    let img = unsafe { &*img };
    let comps = unsafe { std::slice::from_raw_parts(img.comps, img.numcomps as usize) };
    let (w, h, nc) = (comps[0].w as usize, comps[0].h as usize, comps.len());
    for y in 0..h {
        for x in 0..w {
            for comp in comps {
                let prec = comp.prec as i32;
                let mut v = unsafe { *comp.data.add(y * w + x) };
                if comp.sgnd != 0 {
                    v += 1 << (prec - 1);
                }
                if prec > 8 {
                    v >>= prec - 8;
                } else if prec < 8 {
                    v <<= 8 - prec;
                }
                out.push(v.clamp(0, 255) as u8);
            }
        }
    }
    (w as u32, h as u32, nc as u32)
}

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
            opj_destroy_codec(self.codec);
            opj_stream_destroy(self.stream);
        }
    }
}

/// Mirrors `decode_once` in `c/opj_bench.c`.
pub fn decode(data: &[u8], reduce: u32, mode: &Mode) -> Result<(u32, u32, u32, Vec<u8>), String> {
    let mut mem = Mem { data, pos: 0 };
    let mut out = Vec::new();
    unsafe {
        let stream = opj_stream_create(1 << 16, 1);
        opj_stream_set_read_function(stream, Some(mem_read));
        opj_stream_set_skip_function(stream, Some(mem_skip));
        opj_stream_set_seek_function(stream, Some(mem_seek));
        opj_stream_set_user_data(stream, &mut mem as *mut Mem as *mut c_void, Some(no_free));
        opj_stream_set_user_data_length(stream, data.len() as u64);
        let codec = opj_create_decompress(CODEC_FORMAT::OPJ_CODEC_J2K);
        let mut h = Handles {
            codec,
            stream,
            image: std::ptr::null_mut(),
        };
        let mut params: opj_dparameters_t = std::mem::zeroed();
        opj_set_default_decoder_parameters(&mut params);
        params.cp_reduce = reduce;
        if opj_setup_decoder(codec, &mut params) == 0 {
            return Err("setup".into());
        }
        opj_decoder_set_strict_mode(codec, 0);
        if opj_read_header(stream, codec, &mut h.image) == 0 || h.image.is_null() {
            return Err("header".into());
        }
        if reduce > 0 && opj_set_decoded_resolution_factor(codec, reduce) == 0 {
            return Err("reduce".into());
        }
        let mut dims = (0, 0, 0);
        match mode {
            Mode::Tiles => {
                let mut info = opj_get_cstr_info(codec);
                if info.is_null() {
                    return Err("cstr_info".into());
                }
                let ntiles = (*info).tw * (*info).th;
                opj_destroy_cstr_info(&mut info);
                for t in 0..ntiles {
                    if opj_get_decoded_tile(codec, stream, h.image, t) == 0 {
                        return Err(format!("tile {t}"));
                    }
                    dims = write_image(h.image, &mut out);
                }
            }
            Mode::Whole | Mode::Area(..) => {
                if let Mode::Area(x0, y0, x1, y1) = *mode {
                    if opj_set_decode_area(codec, h.image, x0, y0, x1, y1) == 0 {
                        return Err("area".into());
                    }
                }
                if opj_decode(codec, stream, h.image) == 0 {
                    return Err("decode".into());
                }
                dims = write_image(h.image, &mut out);
            }
        }
        opj_end_decompress(codec, stream);
        Ok((dims.0, dims.1, dims.2, out))
    }
}
