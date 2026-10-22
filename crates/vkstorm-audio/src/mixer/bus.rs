//! Derived from SoLoud (Copyright (c) 2013-2020 Jari Komppa), zlib licence;
//! altered: rewritten in Rust. See LICENSE-soloud in this crate.
//!
//! SoLoud's per-voice mixing arithmetic (`soloud.cpp`): the linear
//! resampler, `panAndExpand` (channel volume ramps and source-to-output
//! channel maps) and `clip_internal` (global volume ramp, round-off clipper,
//! post-clip scaler).
//!
//! SoLoud is built with `SOLOUD_SSE_INTRINSICS` on x86-64, so the 1->2 and
//! 2->2 pans and the clipper run four-lane SSE code whose per-lane volume
//! ramps differ in rounding from the scalar fallbacks. Those paths are
//! emulated here lane by lane (`[f32; 4]` arrays: each lane is the same IEEE
//! single operation sequence as the SSE register lane), including the
//! leftover loop's quirk of stepping by four times the per-sample increment
//! from the un-advanced start volume. Everything else is the scalar C code
//! with its grouping (`a += b + c` is `a + (b + c)`).

#![forbid(unsafe_code)]

pub(crate) const SAMPLE_GRANULARITY: usize = 512;
pub(crate) const FIXPOINT_FRAC_BITS: u32 = 20;
pub(crate) const FIXPOINT_FRAC_MUL: u32 = 1 << FIXPOINT_FRAC_BITS;
pub(crate) const FIXPOINT_FRAC_MASK: u32 = FIXPOINT_FRAC_MUL - 1;
pub(crate) const MAX_CHANNELS: usize = 8;

/// `resample_linear`: `src` is the current 512-sample block of one channel,
/// `src1` the previous one (for the sample before index 0).
///
/// The caller guarantees every position stays inside the block (SoLoud's
/// `writesamples` computation), so masking the index with 511 changes
/// nothing and lets the compiler drop the bounds checks.
#[inline]
pub(crate) fn resample_linear(
    src: &[f32],
    src1: &[f32],
    dst: &mut [f32],
    src_offset: u32,
    dst_sample_count: usize,
    step_fixed: u32,
) {
    const MASK: usize = SAMPLE_GRANULARITY - 1;
    let src: &[f32; SAMPLE_GRANULARITY] = src[..SAMPLE_GRANULARITY]
        .try_into()
        .expect("512-sample block");
    let prev = src1[SAMPLE_GRANULARITY - 1];
    let mut pos = src_offset as i32;
    let step = step_fixed as i32;
    let dst = &mut dst[..dst_sample_count];
    // The first samples may sit before index 1 and read the previous block's
    // last sample; positions only grow, so split them off and keep the main
    // loop free of that branch.
    let mut i = 0usize;
    while i < dst.len() && (pos >> FIXPOINT_FRAC_BITS) == 0 {
        let f = pos & FIXPOINT_FRAC_MASK as i32;
        let s1 = prev;
        let s2 = src[0];
        dst[i] = s1 + (s2 - s1) * f as f32 * (1.0f32 / FIXPOINT_FRAC_MUL as f32);
        pos = pos.wrapping_add(step);
        i += 1;
    }
    for d in dst[i..].iter_mut() {
        let p = (pos >> FIXPOINT_FRAC_BITS) as usize;
        let f = pos & FIXPOINT_FRAC_MASK as i32;
        // s1 + (s2 - s1) * f * (1 / (float)FIXPOINT_FRAC_MUL), left to right.
        let s1 = src[p.wrapping_sub(1) & MASK];
        let s2 = src[p & MASK];
        *d = s1 + (s2 - s1) * f as f32 * (1.0f32 / FIXPOINT_FRAC_MUL as f32);
        pos = pos.wrapping_add(step);
    }
}

/// `panAndExpand`. `buffer` is the planar output accumulation (`channels`
/// planes of `buffer_size`), `scratch` the voice's resampled planar samples.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pan_and_expand(
    voice_channels: u32,
    current_channel_volume: &mut [f32; MAX_CHANNELS],
    channel_volume: &[f32; MAX_CHANNELS],
    overall_volume: f32,
    buffer: &mut [f32],
    samples: usize,
    bs: usize,
    scratch: &[f32],
    channels: u32,
) {
    let mut pan = [0.0f32; MAX_CHANNELS];
    let mut pand = [0.0f32; MAX_CHANNELS];
    let mut pani = [0.0f32; MAX_CHANNELS];
    let nch = channels as usize;
    for k in 0..nch {
        pan[k] = current_channel_volume[k];
        pand[k] = channel_volume[k] * overall_volume;
        pani[k] = (pand[k] - pan[k]) / samples as f32;
    }
    let n = samples;

    match channels {
        1 => {
            // Sum every source channel; the ramp restarts per source channel.
            for j in 0..voice_channels as usize {
                let ofs = j * bs;
                pan[0] = current_channel_volume[0];
                for k in 0..n {
                    pan[0] += pani[0];
                    buffer[k] += scratch[ofs + k] * pan[0];
                }
            }
        }
        2 => match voice_channels {
            8 => {
                for j in 0..n {
                    pan[0] += pani[0];
                    pan[1] += pani[1];
                    let s = |c: usize| scratch[bs * c + j];
                    let (s1, s2, s3, s4, s5, s6, s7, s8) =
                        (s(0), s(1), s(2), s(3), s(4), s(5), s(6), s(7));
                    buffer[j] += 0.2f32 * (s1 + s3 + s4 + s5 + s7) * pan[0];
                    buffer[j + bs] += 0.2f32 * (s2 + s3 + s4 + s6 + s8) * pan[1];
                }
            }
            6 => {
                for j in 0..n {
                    pan[0] += pani[0];
                    pan[1] += pani[1];
                    let s = |c: usize| scratch[bs * c + j];
                    let (s1, s2, s3, s4, s5, s6) = (s(0), s(1), s(2), s(3), s(4), s(5));
                    buffer[j] += 0.3f32 * (s1 + s3 + s4 + s5) * pan[0];
                    buffer[j + bs] += 0.3f32 * (s2 + s3 + s4 + s6) * pan[1];
                }
            }
            4 => {
                for j in 0..n {
                    pan[0] += pani[0];
                    pan[1] += pani[1];
                    let s = |c: usize| scratch[bs * c + j];
                    let (s1, s2, s3, s4) = (s(0), s(1), s(2), s(3));
                    buffer[j] += 0.5f32 * (s1 + s3) * pan[0];
                    buffer[j + bs] += 0.5f32 * (s2 + s4) * pan[1];
                }
            }
            2 => pan_sse_2(buffer, scratch, bs, n, &mut pan, &mut pani, true),
            1 => pan_sse_2(buffer, scratch, bs, n, &mut pan, &mut pani, false),
            _ => {}
        },
        4 => match voice_channels {
            8 => {
                for j in 0..n {
                    for k in 0..4 {
                        pan[k] += pani[k];
                    }
                    let s = |c: usize| scratch[bs * c + j];
                    let (s1, s2, s3, s4, s5, s6, s7, s8) =
                        (s(0), s(1), s(2), s(3), s(4), s(5), s(6), s(7));
                    let c = (s3 + s4) * 0.7f32;
                    buffer[j] += s1 * pan[0] + c;
                    buffer[j + bs] += s2 * pan[1] + c;
                    buffer[j + bs * 2] += 0.5f32 * (s5 + s7) * pan[2];
                    buffer[j + bs * 3] += 0.5f32 * (s6 + s8) * pan[3];
                }
            }
            6 => {
                for j in 0..n {
                    for k in 0..4 {
                        pan[k] += pani[k];
                    }
                    let s = |c: usize| scratch[bs * c + j];
                    let (s1, s2, s3, s4, s5, s6) = (s(0), s(1), s(2), s(3), s(4), s(5));
                    let c = (s3 + s4) * 0.7f32;
                    buffer[j] += s1 * pan[0] + c;
                    buffer[j + bs] += s2 * pan[1] + c;
                    buffer[j + bs * 2] += s5 * pan[2];
                    buffer[j + bs * 3] += s6 * pan[3];
                }
            }
            4 => {
                for j in 0..n {
                    for k in 0..4 {
                        pan[k] += pani[k];
                    }
                    for c in 0..4 {
                        buffer[j + bs * c] += scratch[bs * c + j] * pan[c];
                    }
                }
            }
            2 => {
                for j in 0..n {
                    for k in 0..4 {
                        pan[k] += pani[k];
                    }
                    let s1 = scratch[j];
                    let s2 = scratch[bs + j];
                    buffer[j] += s1 * pan[0];
                    buffer[j + bs] += s2 * pan[1];
                    buffer[j + bs * 2] += s1 * pan[2];
                    buffer[j + bs * 3] += s2 * pan[3];
                }
            }
            1 => pan_mono::<4>(buffer, scratch, bs, n, &mut pan, &pani),
            _ => {}
        },
        6 => match voice_channels {
            8 => {
                for j in 0..n {
                    for k in 0..6 {
                        pan[k] += pani[k];
                    }
                    let s = |c: usize| scratch[bs * c + j];
                    let (s1, s2, s3, s4, s5, s6, s7, s8) =
                        (s(0), s(1), s(2), s(3), s(4), s(5), s(6), s(7));
                    buffer[j] += s1 * pan[0];
                    buffer[j + bs] += s2 * pan[1];
                    buffer[j + bs * 2] += s3 * pan[2];
                    buffer[j + bs * 3] += s4 * pan[3];
                    buffer[j + bs * 4] += 0.5f32 * (s5 + s7) * pan[4];
                    buffer[j + bs * 5] += 0.5f32 * (s6 + s8) * pan[5];
                }
            }
            6 => {
                for j in 0..n {
                    for k in 0..6 {
                        pan[k] += pani[k];
                    }
                    for c in 0..6 {
                        buffer[j + bs * c] += scratch[bs * c + j] * pan[c];
                    }
                }
            }
            4 => {
                for j in 0..n {
                    for k in 0..6 {
                        pan[k] += pani[k];
                    }
                    let s = |c: usize| scratch[bs * c + j];
                    let (s1, s2, s3, s4) = (s(0), s(1), s(2), s(3));
                    buffer[j] += s1 * pan[0];
                    buffer[j + bs] += s2 * pan[1];
                    buffer[j + bs * 2] += 0.5f32 * (s1 + s2) * pan[2];
                    buffer[j + bs * 3] += 0.25f32 * (s1 + s2 + s3 + s4) * pan[3];
                    buffer[j + bs * 4] += s3 * pan[4];
                    buffer[j + bs * 5] += s4 * pan[5];
                }
            }
            2 => {
                for j in 0..n {
                    for k in 0..6 {
                        pan[k] += pani[k];
                    }
                    let s1 = scratch[j];
                    let s2 = scratch[bs + j];
                    buffer[j] += s1 * pan[0];
                    buffer[j + bs] += s2 * pan[1];
                    buffer[j + bs * 2] += 0.5f32 * (s1 + s2) * pan[2];
                    buffer[j + bs * 3] += 0.5f32 * (s1 + s2) * pan[3];
                    buffer[j + bs * 4] += s1 * pan[4];
                    buffer[j + bs * 5] += s2 * pan[5];
                }
            }
            1 => pan_mono::<6>(buffer, scratch, bs, n, &mut pan, &pani),
            _ => {}
        },
        8 => match voice_channels {
            8 => {
                for j in 0..n {
                    for k in 0..8 {
                        pan[k] += pani[k];
                    }
                    for c in 0..8 {
                        buffer[j + bs * c] += scratch[bs * c + j] * pan[c];
                    }
                }
            }
            6 => {
                for j in 0..n {
                    for k in 0..8 {
                        pan[k] += pani[k];
                    }
                    let s = |c: usize| scratch[bs * c + j];
                    let (s1, s2, s3, s4, s5, s6) = (s(0), s(1), s(2), s(3), s(4), s(5));
                    buffer[j] += s1 * pan[0];
                    buffer[j + bs] += s2 * pan[1];
                    buffer[j + bs * 2] += s3 * pan[2];
                    buffer[j + bs * 3] += s4 * pan[3];
                    buffer[j + bs * 4] += 0.5f32 * (s5 + s1) * pan[4];
                    buffer[j + bs * 5] += 0.5f32 * (s6 + s2) * pan[5];
                    buffer[j + bs * 6] += s5 * pan[6];
                    buffer[j + bs * 7] += s6 * pan[7];
                }
            }
            4 => {
                for j in 0..n {
                    for k in 0..8 {
                        pan[k] += pani[k];
                    }
                    let s = |c: usize| scratch[bs * c + j];
                    let (s1, s2, s3, s4) = (s(0), s(1), s(2), s(3));
                    buffer[j] += s1 * pan[0];
                    buffer[j + bs] += s2 * pan[1];
                    buffer[j + bs * 2] += 0.5f32 * (s1 + s2) * pan[2];
                    buffer[j + bs * 3] += 0.25f32 * (s1 + s2 + s3 + s4) * pan[3];
                    buffer[j + bs * 4] += 0.5f32 * (s1 + s3) * pan[4];
                    buffer[j + bs * 5] += 0.5f32 * (s2 + s4) * pan[5];
                    // SoLoud uses pan[4]/pan[5] for the last pair here.
                    buffer[j + bs * 6] += s3 * pan[4];
                    buffer[j + bs * 7] += s4 * pan[5];
                }
            }
            2 => {
                for j in 0..n {
                    for k in 0..8 {
                        pan[k] += pani[k];
                    }
                    let s1 = scratch[j];
                    let s2 = scratch[bs + j];
                    buffer[j] += s1 * pan[0];
                    buffer[j + bs] += s2 * pan[1];
                    buffer[j + bs * 2] += 0.5f32 * (s1 + s2) * pan[2];
                    buffer[j + bs * 3] += 0.5f32 * (s1 + s2) * pan[3];
                    buffer[j + bs * 4] += s1 * pan[4];
                    buffer[j + bs * 5] += s2 * pan[5];
                    buffer[j + bs * 6] += s1 * pan[6];
                    buffer[j + bs * 7] += s2 * pan[7];
                }
            }
            1 => pan_mono::<8>(buffer, scratch, bs, n, &mut pan, &pani),
            _ => {}
        },
        _ => {}
    }

    current_channel_volume[..nch].copy_from_slice(&pand[..nch]);
}

/// The first `N` planes of a planar buffer, each cut to `n` samples (lets
/// the compiler drop the per-sample bounds checks of the scalar loops).
#[inline(always)]
fn planes_mut<const N: usize>(buffer: &mut [f32], bs: usize, n: usize) -> [&mut [f32]; N] {
    let mut it = buffer.chunks_exact_mut(bs).map(|p| &mut p[..n]);
    std::array::from_fn(|_| it.next().expect("output plane"))
}

/// Mono source to `N` output planes (SoLoud's scalar 1->4/6/8 loops).
#[inline(always)]
fn pan_mono<const N: usize>(
    buffer: &mut [f32],
    scratch: &[f32],
    bs: usize,
    n: usize,
    pan: &mut [f32; MAX_CHANNELS],
    pani: &[f32; MAX_CHANNELS],
) {
    let o: [&mut [f32]; N] = planes_mut(buffer, bs, n);
    let src = &scratch[..n];
    let mut p = [0.0f32; N];
    p.copy_from_slice(&pan[..N]);
    let mut d = [0.0f32; N];
    d.copy_from_slice(&pani[..N]);
    for (j, &s) in src.iter().enumerate() {
        for k in 0..N {
            p[k] += d[k];
        }
        for c in 0..N {
            o[c][j] += s * p[c];
        }
    }
    pan[..N].copy_from_slice(&p);
}

/// The SSE 1->2 (`stereo == false`) and 2->2 (`stereo == true`) paths.
#[inline]
fn pan_sse_2(
    buffer: &mut [f32],
    scratch: &[f32],
    bs: usize,
    n: usize,
    pan: &mut [f32; MAX_CHANNELS],
    pani: &mut [f32; MAX_CHANNELS],
    stereo: bool,
) {
    let samplequads = n / 4;
    let mut p0 = [
        pan[0] + pani[0],
        pan[0] + pani[0] * 2.0,
        pan[0] + pani[0] * 3.0,
        pan[0] + pani[0] * 4.0,
    ];
    let mut p1 = [
        pan[1] + pani[1],
        pan[1] + pani[1] * 2.0,
        pan[1] + pani[1] * 3.0,
        pan[1] + pani[1] * 4.0,
    ];
    pani[0] *= 4.0;
    pani[1] *= 4.0;
    let d0 = pani[0];
    let d1 = pani[1];
    let right_src = if stereo { bs } else { 0 };
    {
        let n4 = samplequads * 4;
        let (left, rest) = buffer.split_at_mut(bs);
        let right = &mut rest[..n4];
        let left = &mut left[..n4];
        let f0 = &scratch[..n4];
        let f1 = &scratch[right_src..right_src + n4];
        let (left, _) = left.as_chunks_mut::<4>();
        let (right, _) = right.as_chunks_mut::<4>();
        let (f0, _) = f0.as_chunks::<4>();
        let (f1, _) = f1.as_chunks::<4>();
        // One SSE register per array: _mm_mul_ps, _mm_add_ps lane-wise.
        for (((o0, o1), a), b) in left.iter_mut().zip(right.iter_mut()).zip(f0).zip(f1) {
            *o0 = [
                a[0] * p0[0] + o0[0],
                a[1] * p0[1] + o0[1],
                a[2] * p0[2] + o0[2],
                a[3] * p0[3] + o0[3],
            ];
            *o1 = [
                b[0] * p1[0] + o1[0],
                b[1] * p1[1] + o1[1],
                b[2] * p1[2] + o1[2],
                b[3] * p1[3] + o1[3],
            ];
            p0 = [p0[0] + d0, p0[1] + d0, p0[2] + d0, p0[3] + d0];
            p1 = [p1[0] + d1, p1[1] + d1, p1[2] + d1, p1[3] + d1];
        }
    }
    // Leftovers: the scalar loop steps from the un-advanced pan[] by the
    // already quadrupled increment (SoLoud's code, kept).
    for j in samplequads * 4..n {
        pan[0] += pani[0];
        pan[1] += pani[1];
        let s1 = scratch[j];
        let s2 = scratch[right_src + j];
        buffer[j] += s1 * pan[0];
        buffer[j + bs] += s2 * pan[1];
    }
}

/// `clip_internal` (SSE path): `samples` is the stride (multiple of 16).
#[allow(clippy::too_many_arguments)]
pub(crate) fn clip(
    src: &[f32],
    dst: &mut [f32],
    samples: usize,
    volume0: f32,
    volume1: f32,
    channels: usize,
    roundoff: bool,
    post_clip_scaler: f32,
) {
    let mut vd = (volume1 - volume0) / samples as f32;
    let v = volume0;
    let samplequads = samples.div_ceil(4);
    let volumes = [v, v + vd, v + vd + vd, v + vd + vd + vd];
    vd *= 4.0;
    let total = channels * samplequads * 4;
    let src = &src[..total];
    let dst = &mut dst[..total];
    if roundoff {
        const NB: f32 = -1.65;
        const PB: f32 = 1.65;
        const LS: f32 = 0.87;
        const CS: f32 = -0.1;
        const NW: f32 = -0.986_287_5;
        const PW: f32 = 0.986_287_5;
        for ch in 0..channels {
            let mut vol = volumes;
            let base = ch * samplequads * 4;
            for q in 0..samplequads {
                let c = base + q * 4;
                for l in 0..4 {
                    let mut f = src[c + l] * vol[l];
                    vol[l] += vd;
                    let u = f > NB;
                    let o = f < PB;
                    let lin = f * LS;
                    let cubic = f * f * f * CS;
                    f = cubic + lin;
                    // _mm_add_ps(andnot(u, negwall), and(u, f)): exactly one
                    // operand is +0.0.
                    f = if u { 0.0 + f } else { NW + 0.0 };
                    f = if o { 0.0 + f } else { PW + 0.0 };
                    dst[c + l] = f * post_clip_scaler;
                }
            }
        }
    } else {
        for ch in 0..channels {
            let mut vol = volumes;
            let base = ch * samplequads * 4;
            for q in 0..samplequads {
                let c = base + q * 4;
                for l in 0..4 {
                    let mut f = src[c + l] * vol[l];
                    vol[l] += vd;
                    // _mm_max_ps(f, -1) then _mm_min_ps(f, 1): NaN yields the bound.
                    f = if f > -1.0 { f } else { -1.0 };
                    f = if f < 1.0 { f } else { 1.0 };
                    dst[c + l] = f * post_clip_scaler;
                }
            }
        }
    }
}
