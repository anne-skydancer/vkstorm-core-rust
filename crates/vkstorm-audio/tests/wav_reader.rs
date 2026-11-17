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
fn truncated_data_ends_at_the_last_whole_frame() {
    let mut b = wav_bytes(&vec![0.25; 1000], 1, 44100, Enc::S16, false);
    b.truncate(44 + 600 * 2 + 1);
    let d = decode(&b).unwrap();
    assert_eq!(d.sample_count, 600);
    assert!(d.data.iter().all(|&x| x != 0.0));
}

#[test]
fn multichannel_file_truncated_mid_frame_keeps_the_whole_frames() {
    let sig: Vec<f64> = (0..600).map(|i| ((i % 97) as f64 - 48.0) / 50.0).collect();
    let full = wav_bytes(&sig, 6, 44100, Enc::S16, false);
    let whole = decode(&full).unwrap();
    assert_eq!(whole.sample_count, 100);
    // 37 whole frames of 12 bytes and 7 bytes of the next.
    let cut = &full[..44 + 37 * 12 + 7];
    let d = decode(cut).unwrap();
    assert_eq!(d.sample_count, 37);
    assert_eq!(d.data.len(), 37 * 6);
    for ch in 0..6 {
        assert_eq!(
            d.data[ch * 37..(ch + 1) * 37],
            whole.data[ch * 100..ch * 100 + 37]
        );
    }
    // No whole frame: nothing to play.
    assert_eq!(decode(&full[..44 + 11]), Err(WavLoadError::Empty));
}

/// Offset of the data chunk's size field (after the `data` tag).
fn data_size_at(b: &[u8]) -> usize {
    b.windows(4).position(|w| w == b"data").unwrap() + 4
}

#[test]
fn header_frame_claim_is_bounded_by_the_data_present() {
    // 8 channels, 10 frames present, a data chunk claiming 4 GiB, for every
    // encoding: PCM 8/16/24/32, float 32/64, A-law, mu-law, extensible.
    let sig: Vec<f64> = (0..80)
        .map(|i| ((i * 7 % 31) as f64 - 15.0) / 16.0)
        .collect();
    for enc in [
        Enc::U8,
        Enc::S16,
        Enc::S24,
        Enc::S32,
        Enc::F32,
        Enc::F64,
        Enc::Alaw,
        Enc::Mulaw,
        Enc::Ext16,
    ] {
        let honest = wav_bytes(&sig, 8, 44100, enc, false);
        let mut lying = honest.clone();
        let at = data_size_at(&lying);
        lying[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let d = decode(&lying).unwrap();
        assert_eq!(d.sample_count, 10);
        assert_eq!(d.data.len(), 80);
        // Same samples as the honest file.
        assert_eq!(d, decode(&honest).unwrap());
    }
}

#[test]
fn odd_bit_pcm_in_a_wider_container_still_loads() {
    // 12-bit PCM in 16-bit blocks: block alignment 2 bytes per sample.
    let mut b = wav_bytes(&[0.25; 20], 2, 44100, Enc::S16, false);
    b[34..36].copy_from_slice(&12u16.to_le_bytes());
    let d = decode(&b).unwrap();
    assert_eq!(d.sample_count, 10);
}

#[test]
fn a_block_smaller_than_its_samples_is_invalid() {
    // 8 channels of 1-bit samples in a 1-byte block would decode to 32 bytes
    // per input byte.
    let mut b = wav_bytes(&vec![0.25; 64], 8, 44100, Enc::U8, false);
    b[34..36].copy_from_slice(&1u16.to_le_bytes());
    b[32..34].copy_from_slice(&1u16.to_le_bytes());
    assert_eq!(decode(&b), Err(WavLoadError::Invalid));
    // 16-bit stereo with a 2-byte block.
    let mut c = wav_bytes(&[0.25; 20], 2, 44100, Enc::S16, false);
    c[32..34].copy_from_slice(&2u16.to_le_bytes());
    assert_eq!(decode(&c), Err(WavLoadError::Invalid));
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
