//! Device sample formats and the float conversions miniaudio 0.10.42
//! applied between the mixer and the device.
//!
//! The mixer always renders `f32`. When the device's native format is not
//! float (a WASAPI mix format that is integer PCM, a PulseAudio sink or an
//! ALSA PCM without float support) miniaudio converted in software with
//! its reference conversions and no dithering (`ma_dither_mode_none`, the
//! device default): clip to [-1, 1], scale by the type's positive maximum
//! and truncate toward zero. Those formulas are reproduced exactly.
//! Before any conversion miniaudio clipped the float output of every data
//! callback to [-1, 1] (`noClip` false), which [`clip`] reproduces.

#![forbid(unsafe_code)]

/// A device-side sample format (all little-endian, interleaved).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    F32,
    S16,
    /// Packed 24-bit (3 bytes per sample).
    S24,
    S32,
    U8,
}

impl SampleFormat {
    pub fn bytes_per_sample(self) -> usize {
        match self {
            SampleFormat::F32 | SampleFormat::S32 => 4,
            SampleFormat::S24 => 3,
            SampleFormat::S16 => 2,
            SampleFormat::U8 => 1,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SampleFormat::F32 => "f32",
            SampleFormat::S16 => "s16",
            SampleFormat::S24 => "s24",
            SampleFormat::S32 => "s32",
            SampleFormat::U8 => "u8",
        }
    }
}

/// `ma_clip_samples_f32`: clamp to [-1, 1]; NaN passes through as in C.
#[inline]
pub fn clip(samples: &mut [f32]) {
    for s in samples {
        *s = s.clamp(-1.0, 1.0);
    }
}

#[inline]
fn clip1(x: f32) -> f32 {
    x.clamp(-1.0, 1.0)
}

/// Encodes `src` into `dst` in `format` (`dst.len()` must be
/// `src.len() * format.bytes_per_sample()`).
pub fn encode(format: SampleFormat, src: &[f32], dst: &mut [u8]) {
    debug_assert_eq!(dst.len(), src.len() * format.bytes_per_sample());
    match format {
        SampleFormat::F32 => {
            for (s, d) in src.iter().zip(dst.as_chunks_mut::<4>().0) {
                *d = s.to_le_bytes();
            }
        }
        SampleFormat::S16 => {
            // ma_pcm_f32_to_s16__reference ("the fast way").
            for (s, d) in src.iter().zip(dst.as_chunks_mut::<2>().0) {
                *d = ((clip1(*s) * 32767.0) as i16).to_le_bytes();
            }
        }
        SampleFormat::S24 => {
            // ma_pcm_f32_to_s24__reference.
            for (s, d) in src.iter().zip(dst.as_chunks_mut::<3>().0) {
                let b = ((clip1(*s) * 8388607.0) as i32).to_le_bytes();
                *d = [b[0], b[1], b[2]];
            }
        }
        SampleFormat::S32 => {
            // ma_pcm_f32_to_s32__reference: in double precision.
            for (s, d) in src.iter().zip(dst.as_chunks_mut::<4>().0) {
                let x = f64::from(clip1(*s));
                *d = ((x * 2147483647.0) as i32).to_le_bytes();
            }
        }
        SampleFormat::U8 => {
            // ma_pcm_f32_to_u8__reference.
            for (s, d) in src.iter().zip(dst.iter_mut()) {
                *d = ((clip1(*s) + 1.0) * 127.5) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_matches_miniaudio() {
        let mut v = [
            -2.0,
            -1.0,
            -0.5,
            0.0,
            0.5,
            1.0,
            1.5,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ];
        clip(&mut v);
        assert_eq!(v, [-1.0, -1.0, -0.5, 0.0, 0.5, 1.0, 1.0, 1.0, -1.0]);
        let mut n = [f32::NAN];
        clip(&mut n);
        assert!(n[0].is_nan());
    }

    #[test]
    fn integer_encodings() {
        let src = [-1.5f32, -1.0, -0.5, 0.0, 0.25, 1.0, 2.0];
        let mut d = vec![0u8; src.len() * 2];
        encode(SampleFormat::S16, &src, &mut d);
        let got: Vec<i16> = d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect();
        assert_eq!(got, [-32767, -32767, -16383, 0, 8191, 32767, 32767]);

        let mut d = vec![0u8; src.len() * 3];
        encode(SampleFormat::S24, &src, &mut d);
        let got: Vec<i32> = d
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], 0]) << 8 >> 8)
            .collect();
        assert_eq!(
            got,
            [-8388607, -8388607, -4194303, 0, 2097151, 8388607, 8388607]
        );

        let mut d = vec![0u8; src.len() * 4];
        encode(SampleFormat::S32, &src, &mut d);
        let got: Vec<i32> = d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes(*c))
            .collect();
        assert_eq!(
            got,
            [
                -2147483647,
                -2147483647,
                -1073741823,
                0,
                536870911,
                2147483647,
                2147483647
            ]
        );

        let mut d = vec![0u8; src.len()];
        encode(SampleFormat::U8, &src, &mut d);
        assert_eq!(d, [0, 0, 63, 127, 159, 255, 255]);

        let mut d = vec![0u8; 8];
        encode(SampleFormat::F32, &[0.5, -0.25], &mut d);
        assert_eq!(f32::from_le_bytes([d[0], d[1], d[2], d[3]]), 0.5);
        assert_eq!(f32::from_le_bytes([d[4], d[5], d[6], d[7]]), -0.25);
    }
}
