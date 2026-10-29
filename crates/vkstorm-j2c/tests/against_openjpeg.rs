//! Sample-for-sample comparison against OpenJPEG (the Vulkanstorm fork, AVX2
//! build, strict mode off), on a 256x256 texture encoded with the viewer's
//! upload rules. Reference outputs were produced by `tools/j2c-eval/c/opj_bench`
//! and are bottom-up interleaved 8-bit, converted here from OpenJPEG's
//! top-down order.
//!
//! Tolerance is one code value: the two decoders differ only in rounding of
//! the inverse wavelet and colour transforms.

use vkstorm_j2c::{comment, decode, metadata};

const J2C: &[u8] = include_bytes!("fixtures/synth_256.j2c");

fn flip_rows(top_down: &[u8], width: usize, channels: usize) -> Vec<u8> {
    let row = width * channels;
    let mut out = Vec::with_capacity(top_down.len());
    for r in top_down.chunks(row).rev() {
        out.extend_from_slice(r);
    }
    out
}

fn assert_close(actual: &[u8], expected: &[u8], max_abs: u8, what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: length");
    let worst = actual
        .iter()
        .zip(expected)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(worst <= max_abs, "{what}: max abs diff {worst} > {max_abs}");
}

#[test]
fn header() {
    let m = metadata(J2C).unwrap();
    assert_eq!((m.width, m.height, m.components), (256, 256, 3));
    assert_eq!(
        comment(J2C).as_deref(),
        Some("Created by OpenJPEG version 2.5.4")
    );
}

#[test]
fn full_decode_matches_openjpeg() {
    let d = decode(J2C, 0, 0, 0, 4).unwrap();
    assert_eq!((d.width, d.height, d.channels), (256, 256, 3));
    let expected = flip_rows(include_bytes!("fixtures/synth_256.r0.opj.raw"), 256, 3);
    assert_close(&d.data, &expected, 1, "discard 0");
}

#[test]
fn discard_levels_match_openjpeg() {
    let d1 = decode(J2C, 0, 1, 0, 4).unwrap();
    assert_eq!((d1.width, d1.height), (128, 128));
    assert_close(
        &d1.data,
        &flip_rows(include_bytes!("fixtures/synth_256.r1.opj.raw"), 128, 3),
        1,
        "discard 1",
    );

    let d2 = decode(J2C, 0, 2, 0, 4).unwrap();
    assert_eq!((d2.width, d2.height), (64, 64));
    assert_close(
        &d2.data,
        &flip_rows(include_bytes!("fixtures/synth_256.r2.opj.raw"), 64, 3),
        1,
        "discard 2",
    );
}

#[test]
fn truncated_stream_decodes() {
    // 25% of the bytes, the share the viewer fetches for a low discard level.
    let max_bytes = J2C.len() / 4;
    let d = decode(J2C, max_bytes, 0, 0, 4).unwrap();
    assert_eq!((d.width, d.height, d.channels), (256, 256, 3));

    // Both decoders reconstruct from the same partial packets; they may differ
    // in how the incomplete last packet is treated, so compare against the
    // complete decode instead: the truncated image must be a plausible
    // approximation of it (as OpenJPEG's own truncated decode is).
    let full = decode(J2C, 0, 0, 0, 4).unwrap();
    let mse = d
        .data
        .iter()
        .zip(&full.data)
        .map(|(a, b)| {
            let e = f64::from(a.abs_diff(*b));
            e * e
        })
        .sum::<f64>()
        / d.data.len() as f64;
    let psnr = 10.0 * (255.0f64 * 255.0 / mse).log10();
    assert!(
        psnr > 30.0,
        "truncated decode PSNR {psnr:.1} dB vs complete decode"
    );

    // And OpenJPEG's truncated decode of the same bytes is in the same place.
    let opj = flip_rows(include_bytes!("fixtures/synth_256.r0.t25.opj.raw"), 256, 3);
    let mse_opj = opj
        .iter()
        .zip(&full.data)
        .map(|(a, b)| {
            let e = f64::from(a.abs_diff(*b));
            e * e
        })
        .sum::<f64>()
        / opj.len() as f64;
    let psnr_opj = 10.0 * (255.0f64 * 255.0 / mse_opj).log10();
    assert!(
        (psnr - psnr_opj).abs() < 3.0,
        "hayro {psnr:.1} dB vs OpenJPEG {psnr_opj:.1} dB"
    );
}

#[test]
fn channel_selection() {
    let d = decode(J2C, 0, 0, 1, 1).unwrap();
    assert_eq!(d.channels, 1);
    let full = decode(J2C, 0, 0, 0, 4).unwrap();
    let green: Vec<u8> = full.data.chunks(3).map(|p| p[1]).collect();
    assert_eq!(d.data, green);
    assert!(
        decode(J2C, 0, 0, 3, 1).is_err(),
        "first channel past the end"
    );
}

#[test]
fn garbage_is_an_error() {
    assert!(metadata(&[0u8; 64]).is_err());
    assert!(decode(&[0u8; 64], 0, 0, 0, 4).is_err());
}
