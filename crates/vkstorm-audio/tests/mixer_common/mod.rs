//! Shared by the mixer parity test and tools/soloud-oracle: the scene
//! runner (the adapter's call sequences on the Rust mixer) and the test
//! material (WAV fixtures, scene scripts).
#![allow(dead_code)]

pub mod material;
pub mod scene;

/// FNV-1a 64 over the samples' bits (little-endian) and over the log text.
pub fn fnv1a(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn hash_samples(samples: &[f32]) -> u64 {
    fnv1a(samples.iter().flat_map(|s| s.to_bits().to_le_bytes()))
}

/// Sum of squares (f64) and peak magnitude: the libm-tolerant signature
/// the parity test falls back to for the scenes that carry the wind.
pub fn energy_peak(samples: &[f32]) -> (f64, f64) {
    let mut e = 0.0f64;
    let mut p = 0.0f64;
    for &s in samples {
        let v = s as f64;
        e += v * v;
        p = p.max(v.abs());
    }
    (e, p)
}
