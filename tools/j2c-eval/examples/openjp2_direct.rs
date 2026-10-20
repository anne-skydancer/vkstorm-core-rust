//! Drive the openjp2 port directly through its C-style API (what the viewer
//! would do): lenient decode of a possibly truncated codestream at a discard
//! level. usage: openjp2_direct <file> <reduce> <max_bytes|0> <iters> [out.raw]
use openjp2::openjpeg::*;
use std::ffi::c_void;
use std::time::Instant;

struct Mem { data: Vec<u8>, pos: usize }

unsafe extern "C" fn mem_read(buf: *mut c_void, n: usize, ud: *mut c_void) -> usize {
    let m = unsafe { &mut *(ud as *mut Mem) };
    if m.pos >= m.data.len() { return usize::MAX; }
    let n = n.min(m.data.len() - m.pos);
    unsafe { std::ptr::copy_nonoverlapping(m.data.as_ptr().add(m.pos), buf as *mut u8, n) };
    m.pos += n; n
}
unsafe extern "C" fn mem_skip(n: i64, ud: *mut c_void) -> i64 {
    let m = unsafe { &mut *(ud as *mut Mem) };
    let new = (m.pos as i64 + n).clamp(0, m.data.len() as i64);
    let moved = new - m.pos as i64; m.pos = new as usize; moved
}
unsafe extern "C" fn mem_seek(n: i64, ud: *mut c_void) -> i32 {
    let m = unsafe { &mut *(ud as *mut Mem) };
    if n < 0 || n as usize > m.data.len() { return 0; }
    m.pos = n as usize; 1
}
unsafe extern "C" fn mem_free(_ud: *mut c_void) {}

fn decode_once(data: &[u8], reduce: u32) -> Option<(u32, u32, u32, Vec<u8>)> {
    unsafe {
        let mut mem = Box::new(Mem { data: data.to_vec(), pos: 0 });
        let stream = opj_stream_create(1 << 20, 1);
        opj_stream_set_read_function(stream, Some(mem_read));
        opj_stream_set_skip_function(stream, Some(mem_skip));
        opj_stream_set_seek_function(stream, Some(mem_seek));
        opj_stream_set_user_data(stream, &mut *mem as *mut Mem as *mut c_void, Some(mem_free));
        opj_stream_set_user_data_length(stream, data.len() as u64);

        let codec = opj_create_decompress(CODEC_FORMAT::OPJ_CODEC_J2K);
        let mut params: opj_dparameters_t = std::mem::zeroed();
        opj_set_default_decoder_parameters(&mut params);
        params.cp_reduce = reduce;
        let fail = |codec, stream| { opj_destroy_codec(codec); opj_stream_destroy(stream); None };
        if opj_setup_decoder(codec, &mut params) == 0 { return fail(codec, stream); }
        opj_decoder_set_strict_mode(codec, 0);
        let mut image: *mut opj_image_t = std::ptr::null_mut();
        if opj_read_header(stream, codec, &mut image) == 0 { return fail(codec, stream); }
        if reduce > 0 && opj_set_decoded_resolution_factor(codec, reduce) == 0 { opj_image_destroy(image); return fail(codec, stream); }
        if opj_decode(codec, stream, image) == 0 { opj_image_destroy(image); return fail(codec, stream); }
        opj_end_decompress(codec, stream);
        let img = &*image;
        let comps = std::slice::from_raw_parts(img.comps, img.numcomps as usize);
        let (w, h, nc) = (comps[0].w, comps[0].h, img.numcomps);
        let mut out = vec![0u8; (w * h * nc) as usize];
        for (ci, comp) in comps.iter().enumerate() {
            let d = std::slice::from_raw_parts(comp.data, (w * h) as usize);
            for i in 0..(w * h) as usize { out[i * nc as usize + ci] = d[i].clamp(0, 255) as u8; }
        }
        opj_image_destroy(image);
        opj_destroy_codec(codec);
        opj_stream_destroy(stream);
        Some((w, h, nc, out))
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut data = std::fs::read(&a[1]).unwrap();
    let reduce: u32 = a[2].parse().unwrap(); let max: usize = a[3].parse().unwrap(); let iters: usize = a[4].parse().unwrap();
    if max > 0 && max < data.len() { data.truncate(max); }
    let mut best = f64::MAX; let mut last = None;
    for _ in 0..iters {
        let t = Instant::now();
        match decode_once(&data, reduce) { Some(r) => { best = best.min(t.elapsed().as_secs_f64() * 1e3); last = Some(r); } None => { println!("FAIL"); return; } }
    }
    let (w, h, c, out) = last.unwrap();
    println!("{w} {h} {c} {best:.3}");
    if a.len() > 5 { std::fs::write(&a[5], out).unwrap(); }
}
