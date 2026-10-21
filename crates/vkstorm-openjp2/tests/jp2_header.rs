//! JP2 wrapper changes between OpenJPEG v2.5.2 and the fork baseline:
//! the colr box carries enumcs 12 (CMYK) and 24 (e-YCC) (2f5a299c), an ICC
//! profile set on the image is written into the colr box (578c67b3), and the
//! header reader leaves the colour space alone when the codestream header
//! fails to parse (f809b80c).

use std::ffi::c_void;
use vkstorm_openjp2::openjpeg::*;
use vkstorm_openjp2::{ICCProfileRef, Stream};

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
        assert!(new >= 0);
        let new = new as usize;
        if new > data.len() {
            data.resize(new, 0);
        }
        *pos = new;
        Ok(new as u64)
    }
}

extern "C" fn log_error(msg: *const std::ffi::c_char, _data: *mut c_void) {
    eprintln!("[ERROR] {}", unsafe { std::ffi::CStr::from_ptr(msg) }.to_string_lossy());
}

/// Encode a 16x16 image with `numcomps` 8-bit components as JP2.
fn encode_jp2(numcomps: u32, color_space: COLOR_SPACE, icc: Option<&[u8]>) -> Vec<u8> {
    unsafe {
        let mut params: opj_cparameters_t = std::mem::zeroed();
        opj_set_default_encoder_parameters(&mut params);
        params.cod_format = 1; // JP2
        params.tcp_rates[0] = 0.0;
        params.tcp_numlayers = 1;
        params.cp_disto_alloc = 1;
        params.tcp_mct = 0;
        params.numresolution = 3;
        let mut cmptparm: Vec<vkstorm_openjp2::opj_image_comptparm> = (0..numcomps)
            .map(|_| {
                let mut p: vkstorm_openjp2::opj_image_comptparm = std::mem::zeroed();
                p.prec = 8;
                p.dx = 1;
                p.dy = 1;
                p.w = 16;
                p.h = 16;
                p
            })
            .collect();
        let image = opj_image_create(numcomps, cmptparm.as_mut_ptr(), color_space);
        assert!(!image.is_null());
        (*image).x1 = 16;
        (*image).y1 = 16;
        for (k, comp) in std::slice::from_raw_parts_mut((*image).comps, numcomps as usize).iter_mut().enumerate() {
            for (i, v) in std::slice::from_raw_parts_mut(comp.data, 256).iter_mut().enumerate() {
                *v = ((i * 7 + k * 50) & 0xFF) as i32;
            }
        }
        if let Some(icc) = icc {
            assert!((*image).copy_icc_profile(ICCProfileRef::ICC(icc)));
        }
        let codec = opj_create_compress(CODEC_FORMAT::OPJ_CODEC_JP2);
        opj_set_error_handler(codec, Some(log_error), std::ptr::null_mut());
        assert!(opj_setup_encoder(codec, &mut params, image) != 0);
        let sink = SharedSink::default();
        {
            let mut stream = Stream::new_writer(1 << 16, sink.clone());
            let sp = &mut stream as *mut Stream as *mut opj_stream_t;
            assert!(opj_start_compress(codec, image, sp) != 0);
            assert!(opj_encode(codec, sp) != 0);
            assert!(opj_end_compress(codec, sp) != 0);
        }
        opj_destroy_codec(codec);
        opj_image_destroy(image);
        let b = std::mem::take(&mut sink.0.borrow_mut().0);
        b
    }
}

/// The payload of the first `colr` box (after METH, PREC, APPROX).
fn colr(jp2: &[u8]) -> (u8, &[u8]) {
    let p = jp2.windows(4).position(|w| w == b"colr").expect("colr box");
    let len = u32::from_be_bytes(jp2[p - 4..p].try_into().unwrap()) as usize;
    let body = &jp2[p + 4..p - 4 + len];
    (body[0], &body[3..])
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

/// `opj_read_header` on a JP2 file: the colour space it reports, or None on failure.
fn read_header(data: &[u8]) -> Option<COLOR_SPACE> {
    let mut mem = Mem { data, pos: 0 };
    unsafe {
        let stream = opj_stream_create(1 << 16, 1);
        opj_stream_set_read_function(stream, Some(mem_read));
        opj_stream_set_skip_function(stream, Some(mem_skip));
        opj_stream_set_seek_function(stream, Some(mem_seek));
        opj_stream_set_user_data(stream, &mut mem as *mut Mem as *mut c_void, Some(no_free));
        opj_stream_set_user_data_length(stream, data.len() as u64);
        let codec = opj_create_decompress(CODEC_FORMAT::OPJ_CODEC_JP2);
        let mut params: opj_dparameters_t = std::mem::zeroed();
        opj_set_default_decoder_parameters(&mut params);
        assert!(opj_setup_decoder(codec, &mut params) != 0);
        let mut image: *mut opj_image_t = std::ptr::null_mut();
        let ok = opj_read_header(stream, codec, &mut image) != 0;
        let cs = if ok && !image.is_null() { Some((*image).color_space) } else { None };
        if !image.is_null() {
            opj_image_destroy(image);
        }
        opj_destroy_codec(codec);
        opj_stream_destroy(stream);
        cs
    }
}

#[test]
fn colr_enumcs_for_cmyk_and_eycc() {
    let cmyk = encode_jp2(4, COLOR_SPACE::OPJ_CLRSPC_CMYK, None);
    let (meth, body) = colr(&cmyk);
    assert_eq!((meth, u32::from_be_bytes(body[..4].try_into().unwrap())), (1, 12));
    assert_eq!(read_header(&cmyk), Some(COLOR_SPACE::OPJ_CLRSPC_CMYK));

    let eycc = encode_jp2(3, COLOR_SPACE::OPJ_CLRSPC_EYCC, None);
    let (meth, body) = colr(&eycc);
    assert_eq!((meth, u32::from_be_bytes(body[..4].try_into().unwrap())), (1, 24));
    assert_eq!(read_header(&eycc), Some(COLOR_SPACE::OPJ_CLRSPC_EYCC));

    let srgb = encode_jp2(3, COLOR_SPACE::OPJ_CLRSPC_SRGB, None);
    let (meth, body) = colr(&srgb);
    assert_eq!((meth, u32::from_be_bytes(body[..4].try_into().unwrap())), (1, 16));
}

#[test]
fn icc_profile_is_written() {
    let profile: Vec<u8> = (0..300u32).map(|i| (i * 31 % 251) as u8).collect();
    let jp2 = encode_jp2(3, COLOR_SPACE::OPJ_CLRSPC_SRGB, Some(&profile));
    let (meth, body) = colr(&jp2);
    assert_eq!(meth, 2, "restricted ICC method");
    assert!(body == profile.as_slice(), "the colr box must carry the image's ICC profile");
}

#[test]
fn broken_codestream_header_fails_cleanly() {
    let jp2 = encode_jp2(3, COLOR_SPACE::OPJ_CLRSPC_SRGB, None);
    assert_eq!(read_header(&jp2), Some(COLOR_SPACE::OPJ_CLRSPC_SRGB));
    // Cut inside the codestream's main header: the JP2 boxes parse, the
    // codestream header does not, and opj_read_header reports failure.
    let jp2c = jp2.windows(4).position(|w| w == b"jp2c").expect("jp2c box");
    assert_eq!(read_header(&jp2[..jp2c + 4 + 30]), None);
}
