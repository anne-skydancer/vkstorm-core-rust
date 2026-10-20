//! The encoder must be byte-identical to the viewer's OpenJPEG encoder given the
//! same raster and parameters. Fixtures were produced by
//! tools/j2c-eval/c/opj_enc.c (the Vulkanstorm OpenJPEG fork, AVX2 build),
//! which mirrors LLImageJ2COJ::encodeImpl exactly.

use vkstorm_j2c::{comment, decode, encode, layers_for_surface, compression_ratios, resolutions_for};

const J2C: &[u8] = include_bytes!("fixtures/synth_256.j2c");

fn synthetic(w: u32, h: u32, c: u32) -> Vec<u8> {
    let mut v = vec![0u8; (w * h * c) as usize];
    for y in 0..h {
        for x in 0..w {
            for k in 0..c {
                v[((y * w + x) * c + k) as usize] = ((x * 7 + y * 3 + k * 50) % 256) as u8;
            }
        }
    }
    v
}

fn check(w: u32, h: u32, c: u32, rev: bool, fixture: &[u8]) {
    let ours = encode(&synthetic(w, h, c), w, h, c, None, rev).unwrap();
    assert!(ours == fixture, "{w}x{h}x{c} rev={rev}: {} bytes vs C {} bytes, differs", ours.len(), fixture.len());
}

#[test]
fn viewer_rules() {
    assert_eq!(layers_for_surface(32 * 32), 2);
    assert_eq!(layers_for_surface(128 * 128), 3);
    assert_eq!(layers_for_surface(512 * 512), 4);
    assert_eq!(layers_for_surface(1024 * 1024), 5);
    assert_eq!(layers_for_surface(2048 * 2048), 6);
    assert_eq!(compression_ratios(5), vec![2048, 512, 128, 32, 8]);
    assert_eq!(resolutions_for(256, 256), None);
    assert_eq!(resolutions_for(32, 32), Some(6));
    assert_eq!(resolutions_for(1024, 16), Some(5));
}

#[test]
fn small_images_match_c_encoder() {
    check(8, 8, 1, false, include_bytes!("fixtures/enc_synth_8x8x1_rev0.j2c"));
    check(16, 16, 4, false, include_bytes!("fixtures/enc_synth_16x16x4_rev0.j2c"));
    check(16, 16, 4, true, include_bytes!("fixtures/enc_synth_16x16x4_rev1.j2c"));
    check(32, 32, 3, false, include_bytes!("fixtures/enc_synth_32x32x3_rev0.j2c"));
    check(64, 64, 3, true, include_bytes!("fixtures/enc_synth_64x64x3_rev1.j2c"));
    check(100, 100, 4, false, include_bytes!("fixtures/enc_synth_100x100x4_rev0.j2c"));
}

#[test]
fn real_raster_matches_c_encoder() {
    // The decoder is bit-identical to OpenJPEG, so decoding the fixture gives
    // the same raster the C side was fed.
    let src = decode(J2C, 0, 0, 0, 4).unwrap();
    let lossy = encode(&src.data, src.width, src.height, src.channels, Some("vkstorm test"), false).unwrap();
    assert!(lossy == include_bytes!("fixtures/enc_synth_256_lossy.j2c"), "lossy 256 differs from C");
    assert_eq!(comment(&lossy).as_deref(), Some("vkstorm test"));
    let lossless = encode(&src.data, src.width, src.height, src.channels, None, true).unwrap();
    assert!(lossless == include_bytes!("fixtures/enc_synth_256_lossless.j2c"), "lossless 256 differs from C");
    assert_eq!(comment(&lossless).as_deref(), Some("no comment"));

    // And both round-trip through our decoder: lossless exactly, lossy at the
    // viewer's 1:8 quality, with discard levels and truncation available.
    let back = decode(&lossless, 0, 0, 0, 4).unwrap();
    assert_eq!(back.data, src.data);
    let back = decode(&lossy, 0, 0, 0, 4).unwrap();
    assert_eq!((back.width, back.height, back.channels), (256, 256, 3));
    let half = decode(&lossy, 0, 1, 0, 4).unwrap();
    assert_eq!((half.width, half.height), (128, 128));
    assert!(decode(&lossy, lossy.len() / 3, 0, 0, 4).is_ok());
}

#[test]
fn invalid_input_is_an_error() {
    assert!(encode(&[0u8; 16], 0, 16, 4, None, false).is_err());
    assert!(encode(&[0u8; 16], 4, 4, 5, None, false).is_err());
    assert!(encode(&[0u8; 16], 4, 4, 4, None, false).is_err(), "too few bytes");
}
