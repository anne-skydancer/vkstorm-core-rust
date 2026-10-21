//! The viewer-facing API over the codestreams that exercise the OpenJPEG
//! changes between v2.5.2 and the fork baseline (fixtures and their
//! description in `crates/vkstorm-openjp2/tests/upstream_baseline.rs`).
//! What the viewer sees: lenient `decode` at the discard levels, and the
//! refusal of a codestream whose required EPH markers are missing.

use vkstorm_j2c::{decode, metadata};

const PLAIN: &[u8] = include_bytes!("../../vkstorm-openjp2/tests/fixtures/plain_256.j2c");
const MCO_ZERO: &[u8] = include_bytes!("../../vkstorm-openjp2/tests/fixtures/mco_zero_256.j2c");
const TLM_TILES: &[u8] = include_bytes!("../../vkstorm-openjp2/tests/fixtures/tlm_tiles_256.j2c");
const TLM_INVALID: &[u8] = include_bytes!("../../vkstorm-openjp2/tests/fixtures/tlm_invalid_256.j2c");
const TNSOT_ONE: &[u8] = include_bytes!("../../vkstorm-openjp2/tests/fixtures/tnsot_one_256.j2c");
const EPH_SOP: &[u8] = include_bytes!("../../vkstorm-openjp2/tests/fixtures/eph_sop_256.j2c");
const EPH_MISSING: &[u8] = include_bytes!("../../vkstorm-openjp2/tests/fixtures/eph_missing_256.j2c");
const INTERLEAVED: &[u8] = include_bytes!("../../vkstorm-openjp2/tests/fixtures/tp_interleaved_64.j2c");

fn flip_rows(top_down: &[u8], width: usize, channels: usize) -> Vec<u8> {
    let row = width * channels;
    let mut out = Vec::with_capacity(top_down.len());
    for r in top_down.chunks(row).rev() {
        out.extend_from_slice(r);
    }
    out
}

#[test]
fn mco_with_zero_stages_decodes_like_the_plain_codestream() {
    for discard in 0..3 {
        let a = decode(PLAIN, 0, discard, 0, 4).unwrap();
        let b = decode(MCO_ZERO, 0, discard, 0, 4).unwrap();
        assert_eq!((a.width, a.height, a.channels), (b.width, b.height, b.channels));
        assert!(a.data == b.data, "discard {discard}");
    }
}

#[test]
fn tiled_codestream_with_tlm_matches_fork() {
    let d = decode(TLM_TILES, 0, 0, 0, 4).unwrap();
    assert_eq!((d.width, d.height, d.channels), (256, 256, 3));
    let want = flip_rows(include_bytes!("../../vkstorm-openjp2/tests/fixtures/tlm_tiles_256.r0.opj.raw"), 256, 3);
    assert!(d.data == want);
    // a malformed TLM is ignored, not fatal
    let e = decode(TLM_INVALID, 0, 0, 0, 4).unwrap();
    assert!(e.data == want);
    assert_eq!(metadata(TLM_INVALID).unwrap().width, 256);
    // truncated progressive fetch of a tiled stream still decodes
    for discard in 1..3 {
        let t = decode(TLM_TILES, TLM_TILES.len() / 2, discard, 0, 4).unwrap();
        assert_eq!(t.width, 256 >> discard);
    }
}

#[test]
fn single_tile_part_declaration_matches_fork() {
    let d = decode(TNSOT_ONE, 0, 0, 0, 4).unwrap();
    let want = flip_rows(include_bytes!("../../vkstorm-openjp2/tests/fixtures/tnsot_one_256.r0.opj.raw"), 256, 3);
    assert!(d.data == want);
}

#[test]
fn eph_required() {
    let d = decode(EPH_SOP, 0, 0, 0, 4).unwrap();
    let want = flip_rows(include_bytes!("../../vkstorm-openjp2/tests/fixtures/eph_sop_256.r0.opj.raw"), 256, 3);
    assert!(d.data == want);
    assert!(decode(EPH_MISSING, 0, 0, 0, 4).is_err());
    assert!(decode(EPH_MISSING, 0, 2, 0, 4).is_err());
}

#[test]
fn interleaved_tile_parts_decode() {
    let d = decode(INTERLEAVED, 0, 0, 0, 4).unwrap();
    assert_eq!((d.width, d.height, d.channels), (64, 64, 1));
    let want = flip_rows(include_bytes!("../../vkstorm-openjp2/tests/fixtures/tp_interleaved_64.r0.opj.raw"), 64, 1);
    assert!(d.data == want);
}
