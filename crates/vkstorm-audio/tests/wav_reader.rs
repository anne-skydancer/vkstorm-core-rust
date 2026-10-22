//! The WAV reader: dr_wav's conversions and SoLoud's planar layout, the
//! decoded-cache convention, and the error cases. (Bit parity with SoLoud's
//! `Wav::load` over 551 generated files is checked by
//! `tools/soloud-oracle wav-compare`.)

// dr_wav's constants are spelled as in its source.
#![allow(clippy::excessive_precision)]

mod mixer_common;

use mixer_common::material::{Enc, wav_bytes};
use vkstorm_audio::wav::{WavLoadError, decode, load_viewer_buffer};

#[test]
fn pcm16_values_and_planar_layout() {
    // Two frames, stereo: (L0, R0), (L1, R1).
    let s = [1.0, -1.0, 0.5, -0.25];
    let d = decode(&wav_bytes(&s, 2, 44100, Enc::S16, false)).unwrap();
    assert_eq!((d.channels, d.sample_rate, d.sample_count), (2, 44100, 2));
    // round(x * 32767) * 2^-15
    let q = |x: f64| ((x * 32767.0).round() as i16) as f32 * 0.000_030_517_578_125;
    assert_eq!(d.data, vec![q(1.0), q(0.5), q(-1.0), q(-0.25)]);
}

#[test]
fn conversions_match_dr_wav() {
    let s = [0.0, 0.3, -0.7, 1.0];
    let u8d = decode(&wav_bytes(&s, 1, 8000, Enc::U8, false)).unwrap();
    let raw = [128u8, 166, 39, 255];
    for (i, &b) in raw.iter().enumerate() {
        let mut x = b as f32;
        x *= 0.007_843_137_254_901_960_78_f32;
        x -= 1.0;
        assert_eq!(u8d.data[i].to_bits(), x.to_bits());
    }
    let f = decode(&wav_bytes(&s, 1, 8000, Enc::F32, false)).unwrap();
    assert_eq!(f.data, vec![0.0, 0.3, -0.7, 1.0]);
    let f64d = decode(&wav_bytes(&s, 1, 8000, Enc::F64, false)).unwrap();
    assert_eq!(f64d.data, vec![0.0, 0.3f64 as f32, -0.7f64 as f32, 1.0]);
    let s24 = decode(&wav_bytes(&[1.0, -1.0], 1, 8000, Enc::S24, false)).unwrap();
    assert_eq!(
        s24.data,
        vec![
            (8_388_607.0f64 * 0.000_000_119_209_289_550_781_25) as f32,
            (-8_388_607.0f64 * 0.000_000_119_209_289_550_781_25) as f32
        ]
    );
    let s32 = decode(&wav_bytes(&[1.0], 1, 8000, Enc::S32, false)).unwrap();
    assert_eq!(
        s32.data,
        vec![(2_147_483_647.0f64 / 2_147_483_648.0) as f32]
    );
    // A-law 0xD5 and mu-law 0xFF are the codes for (near) zero.
    let a = decode(&{
        let mut b = wav_bytes(&[0.0], 1, 8000, Enc::Alaw, false);
        b[44] = 0xD5; // the data byte (an odd data chunk is followed by a pad byte)
        b
    })
    .unwrap();
    assert_eq!(a.data, vec![8.0 / 32768.0]);
    let m = decode(&{
        let mut b = wav_bytes(&[0.0], 1, 8000, Enc::Mulaw, false);
        b[44] = 0xFF;
        b
    })
    .unwrap();
    assert_eq!(m.data, vec![0.0]);
    let ext = decode(&wav_bytes(&s, 1, 8000, Enc::Ext16, true)).unwrap();
    assert_eq!(ext.sample_count, 4);
}

#[test]
fn decoded_cache_stereo_reads_as_mono_twice_as_long() {
    // LLVorbisDecodeState's header always says mono 44.1 kHz.
    let frames = 1000;
    let s: Vec<f64> = (0..frames * 2)
        .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
        .collect();
    let d = decode(&wav_bytes(&s, 1, 44100, Enc::S16, false)).unwrap();
    assert_eq!((d.channels, d.sample_count), (1, 2000));
    assert!(d.data[0] > 0.0 && d.data[1] < 0.0);
}

#[test]
fn viewer_buffer_defaults_and_fade() {
    let s = vec![0.5; 300];
    let w = load_viewer_buffer(&wav_bytes(&s, 1, 44100, Enc::S16, false)).unwrap();
    assert_eq!(
        (w.min_distance, w.max_distance, w.rolloff),
        (1.0, 512.0, 1.0)
    );
    assert_eq!(
        w.attenuation,
        vkstorm_audio::mixer::Attenuation::InverseDistance
    );
    let v = (0.5f64 * 32767.0).round() as i16 as f32 * 0.000_030_517_578_125;
    let d = w.data();
    assert_eq!(d[0], 0.0);
    assert_eq!(d[1], v * (1.0 / 64.0));
    assert_eq!(d[299], 0.0);
    assert_eq!(d[150], v);
    // 256 frames or fewer: no fade.
    let short = load_viewer_buffer(&wav_bytes(&vec![0.5; 256], 1, 44100, Enc::S16, false)).unwrap();
    assert_eq!(short.data()[0], v);
    assert_eq!(short.length(), (256.0f32 / 44100.0) as f64);
}

#[test]
fn truncated_data_is_zero_filled() {
    let mut b = wav_bytes(&vec![0.25; 1000], 1, 44100, Enc::S16, false);
    b.truncate(44 + 600 * 2 + 1);
    let d = decode(&b).unwrap();
    assert_eq!(d.sample_count, 1000);
    assert!(d.data[599] != 0.0);
    assert!(d.data[600..].iter().all(|&x| x == 0.0));
}

#[test]
fn errors() {
    assert_eq!(decode(&[]), Err(WavLoadError::NotRiff));
    assert_eq!(decode(b"OggS\0\0\0\0"), Err(WavLoadError::NotRiff));
    let base = wav_bytes(&[0.1; 10], 2, 44100, Enc::S16, false);
    let mut adpcm = base.clone();
    adpcm[20..22].copy_from_slice(&2u16.to_le_bytes());
    assert_eq!(decode(&adpcm), Err(WavLoadError::Unsupported(2)));
    let mut ten = wav_bytes(&[0.1; 20], 10, 44100, Enc::S16, false);
    ten[22..24].copy_from_slice(&10u16.to_le_bytes());
    assert_eq!(decode(&ten), Err(WavLoadError::TooManyChannels(10)));
    let mut empty = base.clone();
    empty[40..44].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(decode(&empty), Err(WavLoadError::Empty));
    assert_eq!(decode(&base[..36]), Err(WavLoadError::Invalid));
    let mut zero_rate = base;
    zero_rate[24..28].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(decode(&zero_rate), Err(WavLoadError::Invalid));
}
