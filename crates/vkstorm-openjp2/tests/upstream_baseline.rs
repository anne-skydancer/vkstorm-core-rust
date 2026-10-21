//! Codestreams that exercise the OpenJPEG changes between v2.5.2 and the
//! fork baseline (`UPSTREAM_BASELINE`), driven through the raw API the way
//! `tools/j2c-eval/c/opj_bench.c` drives the C fork. The `.opj.raw` files are
//! that harness's output (8-bit interleaved, top-down) for the fork built at
//! the baseline; the other oracles are identities the fixes guarantee.
//!
//! Fixtures (`fixtures/`, made with the fork's `opj_compress` and the
//! crafting script described in `tools/j2c-eval/README.md`):
//! - `tlm_tiles_256`: 4x4 tiles of 64 px, one tile-part per resolution, a
//!   TLM marker (tile-part index from TLM, seeks through it).
//! - `tlm_invalid_256`: the same with Stlm's ST field set to 3 (the table is
//!   ignored with a warning instead of failing the decode).
//! - `tp_interleaved_64`: 2x2 tiles whose tile-parts are interleaved across
//!   tiles with TNsot only in each tile's last tile-part (upstream #1558).
//! - `tnsot_one_256`: one tile, four tile-parts, every SOT declaring TNsot 1
//!   (the TPsot==TNsot probe is skipped for single tile-part tiles).
//! - `mco_zero_256`: `plain_256` with an MCO marker of zero stages in the
//!   main header (must not reset the DC level shift).
//! - `eph_sop_256` / `eph_missing_256`: SOP and EPH markers, and the same
//!   with the first EPH overwritten (EPH is required, the decode fails).
//! - `cinema2k_256`: 12-bit RGB encoded with `-cinema2K 24` (one guard bit).

use std::ffi::c_void;
use vkstorm_openjp2::openjpeg::*;
use vkstorm_openjp2::Stream;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).expect(name)
}

// ---------------------------------------------------------------------------
// Decoding through the raw API, mirroring c/opj_bench.c.
// ---------------------------------------------------------------------------

enum Mode {
    Whole,
    Tiles,
    Area(i32, i32, i32, i32),
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

/// A decoded image or tile: component 0 size, origin, and 8-bit interleaved rows.
struct Raster {
    w: u32,
    h: u32,
    c: u32,
    x0: u32,
    y0: u32,
    data: Vec<u8>,
}

unsafe fn raster(img: *const opj_image_t) -> Raster {
    let img = unsafe { &*img };
    let comps = unsafe { std::slice::from_raw_parts(img.comps, img.numcomps as usize) };
    let (w, h) = (comps[0].w as usize, comps[0].h as usize);
    let mut data = Vec::with_capacity(w * h * comps.len());
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
                data.push(v.clamp(0, 255) as u8);
            }
        }
    }
    Raster { w: w as u32, h: h as u32, c: comps.len() as u32, x0: comps[0].x0, y0: comps[0].y0, data }
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

/// Lenient decode (strict mode off, as the viewer) of `data`, returning one
/// raster for the whole image or the area, or one per tile in index order.
fn decode(data: &[u8], reduce: u32, mode: Mode) -> Result<Vec<Raster>, String> {
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
        let mut h = Handles { codec, stream, image: std::ptr::null_mut() };
        let mut params: opj_dparameters_t = std::mem::zeroed();
        opj_set_default_decoder_parameters(&mut params);
        params.cp_reduce = reduce;
        assert!(opj_setup_decoder(codec, &mut params) != 0);
        opj_decoder_set_strict_mode(codec, 0);
        if opj_read_header(stream, codec, &mut h.image) == 0 || h.image.is_null() {
            return Err("header".into());
        }
        if reduce > 0 && opj_set_decoded_resolution_factor(codec, reduce) == 0 {
            return Err("reduce".into());
        }
        match mode {
            Mode::Tiles => {
                let mut info = opj_get_cstr_info(codec);
                assert!(!info.is_null());
                let ntiles = (*info).tw * (*info).th;
                opj_destroy_cstr_info(&mut info);
                for t in 0..ntiles {
                    if opj_get_decoded_tile(codec, stream, h.image, t) == 0 {
                        return Err(format!("tile {t}"));
                    }
                    out.push(raster(h.image));
                }
            }
            Mode::Whole | Mode::Area(..) => {
                if let Mode::Area(x0, y0, x1, y1) = mode {
                    if opj_set_decode_area(codec, h.image, x0, y0, x1, y1) == 0 {
                        return Err("area".into());
                    }
                }
                if opj_decode(codec, stream, h.image) == 0 {
                    return Err("decode".into());
                }
                out.push(raster(h.image));
            }
        }
        opj_end_decompress(codec, stream);
    }
    Ok(out)
}

fn whole(name: &str, reduce: u32) -> Raster {
    decode(&fixture(name), reduce, Mode::Whole).expect(name).remove(0)
}

/// `sub` (at its origin, which OpenJPEG reports in full-resolution
/// coordinates) must equal the same window of `full`, both decoded at `reduce`.
fn assert_window(full: &Raster, sub: &Raster, reduce: u32, what: &str) {
    assert_eq!(full.c, sub.c, "{what}: channels");
    let c = full.c as usize;
    let ceildivpow2 = |v: u32| ((v as u64 + (1u64 << reduce) - 1) >> reduce) as usize;
    for y in 0..sub.h as usize {
        let fy = ceildivpow2(sub.y0) - ceildivpow2(full.y0) + y;
        let fx = ceildivpow2(sub.x0) - ceildivpow2(full.x0);
        let want = &full.data[(fy * full.w as usize + fx) * c..(fy * full.w as usize + fx + sub.w as usize) * c];
        let got = &sub.data[y * sub.w as usize * c..(y + 1) * sub.w as usize * c];
        assert!(want == got, "{what}: row {y} differs");
    }
}

fn assert_matches_fork(r: &Raster, raw_name: &str) {
    let want = fixture(raw_name);
    assert_eq!(r.data.len(), want.len(), "{raw_name}: length");
    let worst = r.data.iter().zip(&want).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    assert_eq!(worst, 0, "{raw_name}: max abs diff");
}

// ---------------------------------------------------------------------------
// TLM index, tile-by-tile and area decoding (954c6e3c, ed3a8183, 1bb9e915)
// ---------------------------------------------------------------------------

#[test]
fn tlm_tiles_whole_matches_fork() {
    let r = whole("tlm_tiles_256.j2c", 0);
    assert_eq!((r.w, r.h, r.c), (256, 256, 3));
    assert_matches_fork(&r, "tlm_tiles_256.r0.opj.raw");
}

#[test]
fn tlm_tiles_tile_by_tile_matches_fork_and_whole() {
    let full = whole("tlm_tiles_256.j2c", 0);
    let tiles = decode(&fixture("tlm_tiles_256.j2c"), 0, Mode::Tiles).unwrap();
    assert_eq!(tiles.len(), 16);
    let mut cat = Vec::new();
    for (i, t) in tiles.iter().enumerate() {
        assert_eq!((t.w, t.h), (64, 64), "tile {i}");
        assert_window(&full, t, 0, &format!("tile {i}"));
        cat.extend_from_slice(&t.data);
    }
    assert!(cat == fixture("tlm_tiles_256.tiles.opj.raw"), "concatenated tiles differ from the fork");
    // reduced
    let full1 = whole("tlm_tiles_256.j2c", 1);
    for (i, t) in decode(&fixture("tlm_tiles_256.j2c"), 1, Mode::Tiles).unwrap().iter().enumerate() {
        assert_eq!((t.w, t.h), (32, 32), "tile {i} r1");
        assert_window(&full1, t, 1, &format!("tile {i} r1"));
    }
}

#[test]
fn tlm_tiles_decode_area_matches_whole() {
    let full = whole("tlm_tiles_256.j2c", 0);
    for (x0, y0, x1, y1) in [(10, 10, 100, 90), (130, 70, 250, 250), (64, 64, 128, 128), (0, 0, 256, 64)] {
        let a = decode(&fixture("tlm_tiles_256.j2c"), 0, Mode::Area(x0, y0, x1, y1)).unwrap().remove(0);
        assert_eq!((a.x0, a.y0, a.w, a.h), (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32));
        assert_window(&full, &a, 0, &format!("area {x0},{y0},{x1},{y1}"));
    }
    let full1 = whole("tlm_tiles_256.j2c", 1);
    let a = decode(&fixture("tlm_tiles_256.j2c"), 1, Mode::Area(64, 64, 192, 192)).unwrap().remove(0);
    assert_eq!((a.w, a.h), (64, 64));
    assert_window(&full1, &a, 1, "area r1");
}

#[test]
fn invalid_tlm_is_ignored() {
    // Same codestream as tlm_tiles_256 except Stlm's ST = 3: the decoder
    // warns, drops the table and decodes identically, in every mode.
    let want = whole("tlm_tiles_256.j2c", 0);
    let got = whole("tlm_invalid_256.j2c", 0);
    assert!(want.data == got.data);
    let tiles = decode(&fixture("tlm_invalid_256.j2c"), 0, Mode::Tiles).unwrap();
    assert_eq!(tiles.len(), 16);
    for (i, t) in tiles.iter().enumerate() {
        assert_window(&want, t, 0, &format!("tile {i}"));
    }
    let a = decode(&fixture("tlm_invalid_256.j2c"), 0, Mode::Area(130, 70, 250, 250)).unwrap().remove(0);
    assert_window(&want, &a, 0, "area");
}

#[test]
fn interleaved_tile_parts_decode_tile_by_tile() {
    // upstream tests/test_tile_part_interleaved.c: every tile through
    // opj_get_decoded_tile on one codec must match the whole-image decode.
    let full = whole("tp_interleaved_64.j2c", 0);
    assert_eq!((full.w, full.h, full.c), (64, 64, 1));
    assert_matches_fork(&full, "tp_interleaved_64.r0.opj.raw");
    let tiles = decode(&fixture("tp_interleaved_64.j2c"), 0, Mode::Tiles).expect("issue #1558");
    assert_eq!(tiles.len(), 4);
    for (i, t) in tiles.iter().enumerate() {
        assert_eq!((t.w, t.h), (32, 32), "tile {i}");
        assert_window(&full, t, 0, &format!("tile {i}"));
    }
}

// ---------------------------------------------------------------------------
// Tile-part count probe (497d58c1), MCO (91966314), EPH (c58bc128)
// ---------------------------------------------------------------------------

#[test]
fn single_tile_part_declared_is_taken_as_is() {
    let r = whole("tnsot_one_256.j2c", 0);
    assert_matches_fork(&r, "tnsot_one_256.r0.opj.raw");
}

#[test]
fn mco_with_zero_stages_keeps_dc_shift() {
    let plain = whole("plain_256.j2c", 0);
    let mco = whole("mco_zero_256.j2c", 0);
    assert!(plain.data == mco.data, "an MCO marker with zero stages changed the output");
    assert!(whole("plain_256.j2c", 2).data == whole("mco_zero_256.j2c", 2).data);
}

#[test]
fn eph_markers_are_required() {
    let r = whole("eph_sop_256.j2c", 0);
    assert_matches_fork(&r, "eph_sop_256.r0.opj.raw");
    assert!(decode(&fixture("eph_missing_256.j2c"), 0, Mode::Whole).is_err(), "missing EPH must fail");
    assert!(decode(&fixture("eph_missing_256.j2c"), 2, Mode::Whole).is_err());
}

// ---------------------------------------------------------------------------
// Cinema 2K: one guard bit (40707fd5), byte-identical to the fork's encoder
// ---------------------------------------------------------------------------

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

/// The 12-bit raster of `src12_256.ppm` (integer pattern, see craft.py).
fn raster12() -> Vec<[i32; 3]> {
    let mut v = Vec::with_capacity(256 * 256);
    for y in 0..256i32 {
        for x in 0..256i32 {
            v.push([(x * 13 + y * 5) & 0xFFF, ((x * x + y * y) >> 3) & 0xFFF, ((x ^ y) * 9 + ((x * y) >> 4)) & 0xFFF]);
        }
    }
    v
}

#[test]
fn cinema2k_encode_matches_fork() {
    // opj_compress -cinema2K 24: profile and the 24 fps size limits; the
    // rest (one layer, 9/7, CPRL, ...) is set by opj_j2k_set_cinema_parameters.
    const OPJ_PROFILE_CINEMA_2K: u16 = 0x0003;
    const OPJ_CINEMA_24_CS: i32 = 1302083;
    const OPJ_CINEMA_24_COMP: i32 = 1041666;
    let px = raster12();
    let bytes = unsafe {
        let mut params: opj_cparameters_t = std::mem::zeroed();
        opj_set_default_encoder_parameters(&mut params);
        params.cod_format = 0;
        params.rsiz = OPJ_PROFILE_CINEMA_2K;
        params.max_comp_size = OPJ_CINEMA_24_COMP;
        params.max_cs_size = OPJ_CINEMA_24_CS;
        params.tcp_rates[0] = 0.0;
        params.tcp_numlayers = 1;
        params.cp_disto_alloc = 1;
        params.tcp_mct = 1;
        let mut cmptparm: Vec<vkstorm_openjp2::opj_image_comptparm> = (0..3)
            .map(|_| {
                let mut p: vkstorm_openjp2::opj_image_comptparm = std::mem::zeroed();
                p.prec = 12;
                p.sgnd = 0;
                p.dx = 1;
                p.dy = 1;
                p.w = 256;
                p.h = 256;
                p
            })
            .collect();
        let image = opj_image_create(3, cmptparm.as_mut_ptr(), COLOR_SPACE::OPJ_CLRSPC_SRGB);
        assert!(!image.is_null());
        (*image).x1 = 256;
        (*image).y1 = 256;
        let comps = std::slice::from_raw_parts_mut((*image).comps, 3);
        for (k, comp) in comps.iter_mut().enumerate() {
            let d = std::slice::from_raw_parts_mut(comp.data, 256 * 256);
            for (i, p) in px.iter().enumerate() {
                d[i] = p[k];
            }
        }
        let codec = opj_create_compress(CODEC_FORMAT::OPJ_CODEC_J2K);
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
    };
    let want = fixture("cinema2k_256.j2c");
    // QCD: Sqcd's top three bits are the guard bits, 1 for Cinema 2K.
    let sqcd = |b: &[u8]| {
        let p = b.windows(2).position(|w| w == [0xFF, 0x5C]).expect("QCD");
        b[p + 4] >> 5
    };
    assert_eq!(sqcd(&want), 1, "fork fixture guard bits");
    assert_eq!(sqcd(&bytes), 1, "port guard bits");
    assert!(bytes == want, "cinema2K codestream differs from the fork ({} vs {} bytes)", bytes.len(), want.len());
    // and it decodes like the fork
    let r = whole("cinema2k_256.j2c", 0);
    assert_matches_fork(&r, "cinema2k_256.r0.opj.raw");
}
