//! Fast inverse 5/3 (reversible) wavelet lifting, ported from OpenJPEG's
//! `dwt.c` non-"STANDARD_SLOW_VERSION" paths: single-pass lifting without
//! explicit interleaving, an AVX2 batch for the horizontal pass, and SSE2 /
//! AVX2 multi-column vertical passes. Integer arithmetic only, so every path
//! is bit-identical to the scalar one and to the C library.
//!
//! The original port compiled OpenJPEG's documentation ("slow") version:
//! interleave, generic lift, copy back. On lossless textures that was 4.5x
//! slower than the C fork's AVX2 build.

#![allow(clippy::missing_safety_doc)]

use core::ptr::{copy_nonoverlapping, read_unaligned, write_unaligned};

/// Largest column batch any vertical path uses (AVX2: two 8-lane registers).
pub const PARALLEL_COLS_MAX: usize = 16;

/// Column batch for the vertical pass on this machine: 16 with AVX2, 8 with
/// SSE2 (always present on x86-64), 8 on other targets (scalar per column).
#[inline]
pub(crate) fn parallel_cols() -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        if has_avx2() { 16 } else { 8 }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        8
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
pub(crate) fn has_avx2() -> bool {
    use core::sync::atomic::{AtomicU8, Ordering};
    static CACHED: AtomicU8 = AtomicU8::new(0); // 0 unknown, 1 no, 2 yes
    match CACHED.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let yes = std::is_x86_feature_detected!("avx2");
            CACHED.store(if yes { 2 } else { 1 }, Ordering::Relaxed);
            yes
        }
    }
}

#[inline(always)]
fn add_nowrap(a: i32, b: i32) -> i32 {
    a.wrapping_add(b) // opj_int_add_no_overflow
}

// ---------------------------------------------------------------------------
// Horizontal pass: one row of `len` samples, even part at tiledp[0..sn),
// odd part at tiledp[sn..len) (cas 0) or the reverse (cas 1).
// ---------------------------------------------------------------------------

/// Inverse horizontal 5/3 lifting of one row in place, using `tmp` (at least
/// `len` entries) as scratch.
pub(crate) unsafe fn idwt53_h(tmp: *mut i32, sn: i32, dn: i32, cas: i32, tiledp: *mut i32) {
    let len = sn + dn;
    if cas == 0 {
        if len > 1 {
            #[cfg(target_arch = "x86_64")]
            {
                if has_avx2() && len >= 18 {
                    h_cas0_avx2(tmp, sn, len, tiledp);
                    return;
                }
            }
            h_cas0_scalar(tmp, sn, len, tiledp);
        }
        // len == 1: unmodified value
    } else if len == 1 {
        *tiledp /= 2;
    } else if len == 2 {
        let in_even = tiledp.add(sn as usize);
        let in_odd = tiledp;
        let o1 = *in_odd - ((*in_even + 1) >> 1);
        let o0 = *in_even + o1;
        *tiledp = o0;
        *tiledp.add(1) = o1;
    } else {
        h_cas1_scalar(tmp, sn, len, tiledp);
    }
}

unsafe fn h_cas0_scalar(tmp: *mut i32, sn: i32, len: i32, tiledp: *mut i32) {
    let in_even = tiledp as *const i32;
    let in_odd = tiledp.add(sn as usize) as *const i32;
    debug_assert!(len > 1);
    let mut s1n = *in_even;
    let mut d1n = *in_odd;
    let mut s0n = s1n - ((d1n + 1) >> 1);
    let mut i = 0i32;
    let mut j = 1i32;
    while i < len - 3 {
        let d1c = d1n;
        let s0c = s0n;
        s1n = *in_even.add(j as usize);
        d1n = *in_odd.add(j as usize);
        s0n = s1n - ((d1c + d1n + 2) >> 2);
        *tmp.add(i as usize) = s0c;
        *tmp.add(i as usize + 1) = add_nowrap(d1c, add_nowrap(s0c, s0n) >> 1);
        i += 2;
        j += 1;
    }
    *tmp.add(i as usize) = s0n;
    if len & 1 != 0 {
        let last = *in_even.add(((len - 1) / 2) as usize) - ((d1n + 1) >> 1);
        *tmp.add((len - 1) as usize) = last;
        *tmp.add((len - 2) as usize) = d1n + ((s0n + last) >> 1);
    } else {
        *tmp.add((len - 1) as usize) = d1n + s0n;
    }
    copy_nonoverlapping(tmp as *const i32, tiledp, len as usize);
}

unsafe fn h_cas1_scalar(tmp: *mut i32, sn: i32, len: i32, tiledp: *mut i32) {
    let in_even = tiledp.add(sn as usize) as *const i32;
    let in_odd = tiledp as *const i32;
    debug_assert!(len > 2);
    let mut s1 = *in_even.add(1);
    let mut dc = *in_odd - ((*in_even + s1 + 2) >> 2);
    *tmp = *in_even + dc;
    let mut i = 1i32;
    let mut j = 1i32;
    let limit = len - 2 - if len & 1 == 0 { 1 } else { 0 };
    while i < limit {
        let s2 = *in_even.add(j as usize + 1);
        let dn = *in_odd.add(j as usize) - ((s1 + s2 + 2) >> 2);
        *tmp.add(i as usize) = dc;
        *tmp.add(i as usize + 1) = add_nowrap(s1, add_nowrap(dn, dc) >> 1);
        dc = dn;
        s1 = s2;
        i += 2;
        j += 1;
    }
    *tmp.add(i as usize) = dc;
    if len & 1 == 0 {
        let dn = *in_odd.add((len / 2 - 1) as usize) - ((s1 + 1) >> 1);
        *tmp.add((len - 2) as usize) = s1 + ((dn + dc) >> 1);
        *tmp.add((len - 1) as usize) = dn;
    } else {
        *tmp.add((len - 1) as usize) = s1 + dc;
    }
    copy_nonoverlapping(tmp as *const i32, tiledp, len as usize);
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn h_cas0_avx2(tmp: *mut i32, sn: i32, len: i32, tiledp: *mut i32) {
    use core::arch::x86_64::*;
    let mut in_even = tiledp as *const i32;
    let mut in_odd = tiledp.add(sn as usize) as *const i32;
    let mut out = tmp;
    let mut prev_even = *in_even - ((*in_odd + 1) >> 1);
    let move_right = _mm256_setr_epi32(0, 0, 1, 2, 3, 4, 5, 6);
    let two = _mm256_set1_epi32(2);
    let simd_batch = (len - 2) / 16;
    for _ in 0..simd_batch {
        let lf = _mm256_loadu_si256(in_even.add(1) as *const __m256i);
        let hf1 = _mm256_loadu_si256(in_odd as *const __m256i);
        let hf2 = _mm256_loadu_si256(in_odd.add(1) as *const __m256i);
        let mut even = _mm256_add_epi32(hf1, hf2);
        even = _mm256_add_epi32(even, two);
        even = _mm256_srai_epi32::<2>(even);
        even = _mm256_sub_epi32(lf, even);
        let next_even = _mm_extract_epi32::<3>(_mm256_extracti128_si256::<1>(even));
        let mut even_m1 = _mm256_permutevar8x32_epi32(even, move_right);
        even_m1 = _mm256_blend_epi32::<1>(even_m1, _mm256_set1_epi32(prev_even));
        let mut odd = _mm256_add_epi32(even_m1, even);
        odd = _mm256_srai_epi32::<1>(odd);
        odd = _mm256_add_epi32(odd, hf1);
        let unpack1 = _mm256_unpacklo_epi32(even_m1, odd);
        let unpack2 = _mm256_unpackhi_epi32(even_m1, odd);
        _mm_storeu_si128(out as *mut __m128i, _mm256_castsi256_si128(unpack1));
        _mm_storeu_si128(out.add(4) as *mut __m128i, _mm256_castsi256_si128(unpack2));
        _mm_storeu_si128(out.add(8) as *mut __m128i, _mm256_extracti128_si256::<1>(unpack1));
        _mm_storeu_si128(out.add(12) as *mut __m128i, _mm256_extracti128_si256::<1>(unpack2));
        prev_even = next_even;
        out = out.add(16);
        in_even = in_even.add(8);
        in_odd = in_odd.add(8);
    }
    *out = prev_even;
    let mut j = simd_batch * 16 + 1;
    while j < len - 2 {
        *out.add(2) = *in_even.add(1) - ((*in_odd + *in_odd.add(1) + 2) >> 2);
        *out.add(1) = *in_odd + ((*out + *out.add(2)) >> 1);
        in_even = in_even.add(1);
        in_odd = in_odd.add(1);
        out = out.add(2);
        j += 2;
    }
    if len & 1 != 0 {
        *out.add(2) = *in_even.add(1) - ((*in_odd + 1) >> 1);
        *out.add(1) = *in_odd + ((*out + *out.add(2)) >> 1);
    } else {
        *out.add(1) = *in_odd + *out;
    }
    copy_nonoverlapping(tmp as *const i32, tiledp, len as usize);
}

// ---------------------------------------------------------------------------
// Vertical pass: `nb_cols` adjacent columns of `len` samples with `stride`
// between rows; even part in rows [0, sn), odd part in rows [sn, len) (cas 0)
// or the reverse (cas 1). `tmp` holds PARALLEL_COLS_MAX * len entries.
// ---------------------------------------------------------------------------

/// Inverse vertical 5/3 lifting of `nb_cols` columns in place.
pub(crate) unsafe fn idwt53_v(
    tmp: *mut i32,
    sn: i32,
    dn: i32,
    cas: i32,
    tiledp_col: *mut i32,
    stride: usize,
    nb_cols: i32,
) {
    let len = sn + dn;
    if cas == 0 {
        if len > 1 {
            #[cfg(target_arch = "x86_64")]
            {
                if nb_cols as usize == parallel_cols() {
                    if has_avx2() {
                        v_cas0_mcols_avx2(tmp, sn, len, tiledp_col, stride);
                    } else {
                        v_cas0_mcols_sse2(tmp, sn, len, tiledp_col, stride);
                    }
                    return;
                }
            }
            for c in 0..nb_cols as usize {
                v_cas0_col(tmp, sn, len, tiledp_col.add(c), stride);
            }
        }
        // len == 1: unmodified value
    } else if len == 1 {
        for c in 0..nb_cols as usize {
            *tiledp_col.add(c) /= 2;
        }
    } else if len == 2 {
        for c in 0..nb_cols as usize {
            let col = tiledp_col.add(c);
            let in_even = col.add(sn as usize * stride);
            let in_odd = col;
            let o1 = *in_odd - ((*in_even + 1) >> 1);
            let o0 = *in_even + o1;
            *col = o0;
            *col.add(stride) = o1;
        }
    } else {
        #[cfg(target_arch = "x86_64")]
        {
            if nb_cols as usize == parallel_cols() {
                if has_avx2() {
                    v_cas1_mcols_avx2(tmp, sn, len, tiledp_col, stride);
                } else {
                    v_cas1_mcols_sse2(tmp, sn, len, tiledp_col, stride);
                }
                return;
            }
        }
        for c in 0..nb_cols as usize {
            v_cas1_col(tmp, sn, len, tiledp_col.add(c), stride);
        }
    }
}

unsafe fn v_cas0_col(tmp: *mut i32, sn: i32, len: i32, col: *mut i32, stride: usize) {
    debug_assert!(len > 1);
    let at = |r: usize| col.add(r * stride);
    let mut s1n = *at(0);
    let mut d1n = *at(sn as usize);
    let mut s0n = s1n - ((d1n + 1) >> 1);
    let mut i = 0i32;
    let mut j = 0i32;
    while i < len - 3 {
        let d1c = d1n;
        let s0c = s0n;
        s1n = *at((j + 1) as usize);
        d1n = *at((sn + j + 1) as usize);
        s0n = s1n.wrapping_sub(add_nowrap(add_nowrap(d1c, d1n), 2) >> 2);
        *tmp.add(i as usize) = s0c;
        *tmp.add(i as usize + 1) = add_nowrap(d1c, add_nowrap(s0c, s0n) >> 1);
        i += 2;
        j += 1;
    }
    *tmp.add(i as usize) = s0n;
    if len & 1 != 0 {
        let last = *at(((len - 1) / 2) as usize) - ((d1n + 1) >> 1);
        *tmp.add((len - 1) as usize) = last;
        *tmp.add((len - 2) as usize) = d1n + ((s0n + last) >> 1);
    } else {
        *tmp.add((len - 1) as usize) = d1n + s0n;
    }
    for r in 0..len as usize {
        *at(r) = *tmp.add(r);
    }
}

unsafe fn v_cas1_col(tmp: *mut i32, sn: i32, len: i32, col: *mut i32, stride: usize) {
    debug_assert!(len > 2);
    let in_even = |r: usize| *col.add((sn as usize + r) * stride);
    let in_odd = |r: usize| *col.add(r * stride);
    let mut s1 = in_even(1);
    let mut dc = in_odd(0) - ((in_even(0) + s1 + 2) >> 2);
    *tmp = in_even(0) + dc;
    let mut i = 1i32;
    let mut j = 1i32;
    let limit = len - 2 - if len & 1 == 0 { 1 } else { 0 };
    while i < limit {
        let s2 = in_even((j + 1) as usize);
        let dn = in_odd(j as usize) - ((s1 + s2 + 2) >> 2);
        *tmp.add(i as usize) = dc;
        *tmp.add(i as usize + 1) = s1 + ((dn + dc) >> 1);
        dc = dn;
        s1 = s2;
        i += 2;
        j += 1;
    }
    *tmp.add(i as usize) = dc;
    if len & 1 == 0 {
        let dn = in_odd((len / 2 - 1) as usize) - ((s1 + 1) >> 1);
        *tmp.add((len - 2) as usize) = s1 + ((dn + dc) >> 1);
        *tmp.add((len - 1) as usize) = dn;
    } else {
        *tmp.add((len - 1) as usize) = s1 + dc;
    }
    for r in 0..len as usize {
        *col.add(r * stride) = *tmp.add(r);
    }
}

/// The SIMD multi-column kernels are written once over a lane abstraction and
/// instantiated for SSE2 (4 lanes, 8 columns) and AVX2 (8 lanes, 16 columns).
#[cfg(target_arch = "x86_64")]
macro_rules! vertical_mcols {
    ($cas0:ident, $cas1:ident, $final:ident, $feat:literal, $lanes:expr, $reg:ty,
     $load:expr, $store:expr, $add:expr, $sub:expr, $sar2:expr, $sar1:expr, $set1:expr) => {
        #[target_feature(enable = $feat)]
        pub(super) unsafe fn $final(tiledp_col: *mut i32, tmp: *const i32, len: i32, stride: usize) {
            const PC: usize = 2 * $lanes;
            for i in 0..len as usize {
                $store(tiledp_col.add(i * stride), $load(tmp.add(PC * i)));
                $store(tiledp_col.add(i * stride + $lanes), $load(tmp.add(PC * i + $lanes)));
            }
        }

        #[target_feature(enable = $feat)]
        pub(super) unsafe fn $cas0(tmp: *mut i32, sn: i32, len: i32, tiledp_col: *mut i32, stride: usize) {
            const PC: usize = 2 * $lanes;
            let in_even = tiledp_col as *const i32;
            let in_odd = tiledp_col.add(sn as usize * stride) as *const i32;
            let two: $reg = $set1(2);
            debug_assert!(len > 1);
            let mut s1n_0 = $load(in_even);
            let mut s1n_1 = $load(in_even.add($lanes));
            let mut d1n_0 = $load(in_odd);
            let mut d1n_1 = $load(in_odd.add($lanes));
            let mut s0n_0 = $sub(s1n_0, $sar2($add($add(d1n_0, d1n_0), two)));
            let mut s0n_1 = $sub(s1n_1, $sar2($add($add(d1n_1, d1n_1), two)));
            let mut i = 0i32;
            let mut j = 1usize;
            while i < len - 3 {
                let d1c_0 = d1n_0;
                let s0c_0 = s0n_0;
                let d1c_1 = d1n_1;
                let s0c_1 = s0n_1;
                s1n_0 = $load(in_even.add(j * stride));
                s1n_1 = $load(in_even.add(j * stride + $lanes));
                d1n_0 = $load(in_odd.add(j * stride));
                d1n_1 = $load(in_odd.add(j * stride + $lanes));
                s0n_0 = $sub(s1n_0, $sar2($add($add(d1c_0, d1n_0), two)));
                s0n_1 = $sub(s1n_1, $sar2($add($add(d1c_1, d1n_1), two)));
                $store(tmp.add(PC * i as usize), s0c_0);
                $store(tmp.add(PC * i as usize + $lanes), s0c_1);
                $store(tmp.add(PC * (i as usize + 1)), $add(d1c_0, $sar1($add(s0c_0, s0n_0))));
                $store(tmp.add(PC * (i as usize + 1) + $lanes), $add(d1c_1, $sar1($add(s0c_1, s0n_1))));
                i += 2;
                j += 1;
            }
            $store(tmp.add(PC * i as usize), s0n_0);
            $store(tmp.add(PC * i as usize + $lanes), s0n_1);
            if len & 1 != 0 {
                let half = ((len - 1) / 2) as usize;
                s1n_0 = $load(in_even.add(half * stride));
                let last_0 = $sub(s1n_0, $sar2($add($add(d1n_0, d1n_0), two)));
                $store(tmp.add(PC * (len - 1) as usize), last_0);
                $store(tmp.add(PC * (len - 2) as usize), $add(d1n_0, $sar1($add(s0n_0, last_0))));
                s1n_1 = $load(in_even.add(half * stride + $lanes));
                let last_1 = $sub(s1n_1, $sar2($add($add(d1n_1, d1n_1), two)));
                $store(tmp.add(PC * (len - 1) as usize + $lanes), last_1);
                $store(tmp.add(PC * (len - 2) as usize + $lanes), $add(d1n_1, $sar1($add(s0n_1, last_1))));
            } else {
                $store(tmp.add(PC * (len - 1) as usize), $add(d1n_0, s0n_0));
                $store(tmp.add(PC * (len - 1) as usize + $lanes), $add(d1n_1, s0n_1));
            }
            $final(tiledp_col, tmp, len, stride);
        }

        #[target_feature(enable = $feat)]
        pub(super) unsafe fn $cas1(tmp: *mut i32, sn: i32, len: i32, tiledp_col: *mut i32, stride: usize) {
            const PC: usize = 2 * $lanes;
            let two: $reg = $set1(2);
            let in_even = tiledp_col.add(sn as usize * stride) as *const i32;
            let in_odd = tiledp_col as *const i32;
            debug_assert!(len > 2);
            let mut s1_0 = $load(in_even.add(stride));
            let mut dc_0 = $sub($load(in_odd), $sar2($add($add($load(in_even), s1_0), two)));
            $store(tmp, $add($load(in_even), dc_0));
            let mut s1_1 = $load(in_even.add(stride + $lanes));
            let mut dc_1 = $sub($load(in_odd.add($lanes)), $sar2($add($add($load(in_even.add($lanes)), s1_1), two)));
            $store(tmp.add($lanes), $add($load(in_even.add($lanes)), dc_1));
            let mut i = 1i32;
            let mut j = 1usize;
            let limit = len - 2 - if len & 1 == 0 { 1 } else { 0 };
            while i < limit {
                let s2_0 = $load(in_even.add((j + 1) * stride));
                let s2_1 = $load(in_even.add((j + 1) * stride + $lanes));
                let dn_0 = $sub($load(in_odd.add(j * stride)), $sar2($add($add(s1_0, s2_0), two)));
                let dn_1 = $sub($load(in_odd.add(j * stride + $lanes)), $sar2($add($add(s1_1, s2_1), two)));
                $store(tmp.add(PC * i as usize), dc_0);
                $store(tmp.add(PC * i as usize + $lanes), dc_1);
                $store(tmp.add(PC * (i as usize + 1)), $add(s1_0, $sar1($add(dn_0, dc_0))));
                $store(tmp.add(PC * (i as usize + 1) + $lanes), $add(s1_1, $sar1($add(dn_1, dc_1))));
                dc_0 = dn_0;
                s1_0 = s2_0;
                dc_1 = dn_1;
                s1_1 = s2_1;
                i += 2;
                j += 1;
            }
            $store(tmp.add(PC * i as usize), dc_0);
            $store(tmp.add(PC * i as usize + $lanes), dc_1);
            if len & 1 == 0 {
                let half = (len / 2 - 1) as usize;
                let dn_0 = $sub($load(in_odd.add(half * stride)), $sar2($add($add(s1_0, s1_0), two)));
                let dn_1 = $sub($load(in_odd.add(half * stride + $lanes)), $sar2($add($add(s1_1, s1_1), two)));
                $store(tmp.add(PC * (len - 2) as usize), $add(s1_0, $sar1($add(dn_0, dc_0))));
                $store(tmp.add(PC * (len - 2) as usize + $lanes), $add(s1_1, $sar1($add(dn_1, dc_1))));
                $store(tmp.add(PC * (len - 1) as usize), dn_0);
                $store(tmp.add(PC * (len - 1) as usize + $lanes), dn_1);
            } else {
                $store(tmp.add(PC * (len - 1) as usize), $add(s1_0, dc_0));
                $store(tmp.add(PC * (len - 1) as usize + $lanes), $add(s1_1, dc_1));
            }
            $final(tiledp_col, tmp, len, stride);
        }
    };
}

#[cfg(target_arch = "x86_64")]
mod simd {
    use core::arch::x86_64::*;

    vertical_mcols!(
        v_cas0_mcols_sse2, v_cas1_mcols_sse2, v_final_sse2, "sse2", 4, __m128i,
        |p: *const i32| _mm_loadu_si128(p as *const __m128i),
        |p: *mut i32, v: __m128i| _mm_storeu_si128(p as *mut __m128i, v),
        |a: __m128i, b: __m128i| _mm_add_epi32(a, b),
        |a: __m128i, b: __m128i| _mm_sub_epi32(a, b),
        |a: __m128i| _mm_srai_epi32::<2>(a),
        |a: __m128i| _mm_srai_epi32::<1>(a),
        |x: i32| _mm_set1_epi32(x)
    );

    vertical_mcols!(
        v_cas0_mcols_avx2, v_cas1_mcols_avx2, v_final_avx2, "avx2", 8, __m256i,
        |p: *const i32| _mm256_loadu_si256(p as *const __m256i),
        |p: *mut i32, v: __m256i| _mm256_storeu_si256(p as *mut __m256i, v),
        |a: __m256i, b: __m256i| _mm256_add_epi32(a, b),
        |a: __m256i, b: __m256i| _mm256_sub_epi32(a, b),
        |a: __m256i| _mm256_srai_epi32::<2>(a),
        |a: __m256i| _mm256_srai_epi32::<1>(a),
        |x: i32| _mm256_set1_epi32(x)
    );

}

#[cfg(target_arch = "x86_64")]
use simd::*;

// Keep the unaligned helpers referenced on non-x86 builds too.
#[allow(dead_code)]
unsafe fn _unused(p: *mut i32) {
    write_unaligned(p, read_unaligned(p));
}
