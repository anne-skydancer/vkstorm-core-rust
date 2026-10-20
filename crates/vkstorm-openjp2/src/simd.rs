//! SIMD kernels for the decoder's sample-wide loops, mirroring the SSE and
//! AVX2 paths of the Vulkanstorm OpenJPEG fork: the 9/7 inverse wavelet
//! lifting steps on the 8-wide interleaved layout, the irreversible component
//! transform, and the Tier-1 copy-out (halving for 5/3, dequantisation for
//! 9/7). Every kernel keeps the scalar operation order (separate multiply and
//! add, no fused multiply-add) so the output is identical to the scalar port.
//! AVX2 is detected at runtime; the scalar path is the fallback.

use super::dwt53::has_avx2;
use super::openjpeg::*;

/// `dst[i] = src[i] / 2` (C integer division, truncating toward zero).
/// `src` and `dst` may alias exactly.
#[inline]
pub(crate) unsafe fn half_i32(src: *const OPJ_INT32, dst: *mut OPJ_INT32, n: usize) {
  if has_avx2() {
    half_i32_avx2(src, dst, n)
  } else {
    half_i32_scalar(src, dst, n)
  }
}

#[inline]
unsafe fn half_i32_scalar(src: *const OPJ_INT32, dst: *mut OPJ_INT32, n: usize) {
  for i in 0..n {
    *dst.add(i) = *src.add(i) / 2;
  }
}

#[target_feature(enable = "avx2")]
unsafe fn half_i32_avx2(src: *const OPJ_INT32, dst: *mut OPJ_INT32, n: usize) {
  use core::arch::x86_64::*;
  let mut i = 0usize;
  while i + 8 <= n {
    let v = _mm256_loadu_si256(src.add(i) as *const __m256i);
    // x / 2 with truncation toward zero: (x + (x >>> 31)) >> 1
    let r = _mm256_srai_epi32(_mm256_add_epi32(v, _mm256_srli_epi32(v, 31)), 1);
    _mm256_storeu_si256(dst.add(i) as *mut __m256i, r);
    i += 8;
  }
  while i < n {
    *dst.add(i) = *src.add(i) / 2;
    i += 1;
  }
}

/// `dst[i] = (src[i] as f32) * stepsize`, written as float bits.
/// `src` and `dst` may alias exactly.
#[inline]
pub(crate) unsafe fn scale_i32_to_f32(
  src: *const OPJ_INT32,
  dst: *mut OPJ_FLOAT32,
  n: usize,
  stepsize: OPJ_FLOAT32,
) {
  if has_avx2() {
    scale_i32_to_f32_avx2(src, dst, n, stepsize)
  } else {
    scale_i32_to_f32_scalar(src, dst, n, stepsize)
  }
}

#[inline]
unsafe fn scale_i32_to_f32_scalar(
  src: *const OPJ_INT32,
  dst: *mut OPJ_FLOAT32,
  n: usize,
  stepsize: OPJ_FLOAT32,
) {
  for i in 0..n {
    let v = *src.add(i) as OPJ_FLOAT32 * stepsize;
    *dst.add(i) = v;
  }
}

#[target_feature(enable = "avx2")]
unsafe fn scale_i32_to_f32_avx2(
  src: *const OPJ_INT32,
  dst: *mut OPJ_FLOAT32,
  n: usize,
  stepsize: OPJ_FLOAT32,
) {
  use core::arch::x86_64::*;
  let s = _mm256_set1_ps(stepsize);
  let mut i = 0usize;
  while i + 8 <= n {
    let v = _mm256_cvtepi32_ps(_mm256_loadu_si256(src.add(i) as *const __m256i));
    _mm256_storeu_ps(dst.add(i), _mm256_mul_ps(v, s));
    i += 8;
  }
  while i < n {
    let v = *src.add(i) as OPJ_FLOAT32 * stepsize;
    *dst.add(i) = v;
    i += 1;
  }
}

/// Irreversible component transform, decode direction (YCbCr to RGB), in
/// place over three planes of `n` samples.
#[inline]
pub(crate) unsafe fn mct_decode_real(
  c0: *mut OPJ_FLOAT32,
  c1: *mut OPJ_FLOAT32,
  c2: *mut OPJ_FLOAT32,
  n: usize,
) {
  if has_avx2() {
    mct_decode_real_avx2(c0, c1, c2, n)
  } else {
    mct_decode_real_scalar(c0, c1, c2, n, 0)
  }
}

#[inline]
unsafe fn mct_decode_real_scalar(
  c0: *mut OPJ_FLOAT32,
  c1: *mut OPJ_FLOAT32,
  c2: *mut OPJ_FLOAT32,
  n: usize,
  from: usize,
) {
  for i in from..n {
    let y = *c0.add(i);
    let u = *c1.add(i);
    let v = *c2.add(i);
    let r = y + v * 1.402f32;
    let g = y - u * 0.34413f32 - v * 0.71414f32;
    let b = y + u * 1.772f32;
    *c0.add(i) = r;
    *c1.add(i) = g;
    *c2.add(i) = b;
  }
}

#[target_feature(enable = "avx2")]
unsafe fn mct_decode_real_avx2(
  c0: *mut OPJ_FLOAT32,
  c1: *mut OPJ_FLOAT32,
  c2: *mut OPJ_FLOAT32,
  n: usize,
) {
  use core::arch::x86_64::*;
  let vrv = _mm256_set1_ps(1.402f32);
  let vgu = _mm256_set1_ps(0.34413f32);
  let vgv = _mm256_set1_ps(0.71414f32);
  let vbu = _mm256_set1_ps(1.772f32);
  let mut i = 0usize;
  while i + 8 <= n {
    let y = _mm256_loadu_ps(c0.add(i));
    let u = _mm256_loadu_ps(c1.add(i));
    let v = _mm256_loadu_ps(c2.add(i));
    let r = _mm256_add_ps(y, _mm256_mul_ps(v, vrv));
    let g = _mm256_sub_ps(_mm256_sub_ps(y, _mm256_mul_ps(u, vgu)), _mm256_mul_ps(v, vgv));
    let b = _mm256_add_ps(y, _mm256_mul_ps(u, vbu));
    _mm256_storeu_ps(c0.add(i), r);
    _mm256_storeu_ps(c1.add(i), g);
    _mm256_storeu_ps(c2.add(i), b);
    i += 8;
  }
  mct_decode_real_scalar(c0, c1, c2, n, i);
}

/// 9/7 inverse lifting, step 1 on the 8-wide interleaved layout:
/// `w[i*16 .. i*16+8] *= c` for `i` in `start..end`.
#[inline]
pub(crate) unsafe fn v8dwt_decode_step1(fw: *mut OPJ_FLOAT32, start: u32, end: u32, c: OPJ_FLOAT32) {
  if has_avx2() {
    v8dwt_decode_step1_avx2(fw, start, end, c)
  } else {
    for i in start..end {
      let p = fw.add(i as usize * 16);
      for k in 0..8 {
        *p.add(k) = *p.add(k) * c;
      }
    }
  }
}

#[target_feature(enable = "avx2")]
unsafe fn v8dwt_decode_step1_avx2(fw: *mut OPJ_FLOAT32, start: u32, end: u32, c: OPJ_FLOAT32) {
  use core::arch::x86_64::*;
  let vc = _mm256_set1_ps(c);
  for i in start..end {
    let p = fw.add(i as usize * 16);
    _mm256_storeu_ps(p, _mm256_mul_ps(_mm256_loadu_ps(p), vc));
  }
}

/// 9/7 inverse lifting, step 2 on the 8-wide interleaved layout, as
/// `opj_v8dwt_decode_step2`: `fw[-8..0] += (fl[0..8] + fw[0..8]) * c`
/// advancing by 16 floats, with the final half-step when `m < end`.
#[inline]
pub(crate) unsafe fn v8dwt_decode_step2(
  fl: *mut OPJ_FLOAT32,
  fw: *mut OPJ_FLOAT32,
  start: u32,
  end: u32,
  m: u32,
  c: OPJ_FLOAT32,
) {
  if has_avx2() {
    v8dwt_decode_step2_avx2(fl, fw, start, end, m, c)
  } else {
    v8dwt_decode_step2_scalar(fl, fw, start, end, m, c)
  }
}

#[inline]
unsafe fn v8dwt_decode_step2_scalar(
  mut fl: *mut OPJ_FLOAT32,
  mut fw: *mut OPJ_FLOAT32,
  start: u32,
  end: u32,
  m: u32,
  mut c: OPJ_FLOAT32,
) {
  let imax = end.min(m);
  if start > 0 {
    fw = fw.add(16 * start as usize);
    fl = fw.sub(16);
  }
  for _ in start..imax {
    for k in 0..8usize {
      *fw.sub(8 - k) = *fw.sub(8 - k) + (*fl.add(k) + *fw.add(k)) * c;
    }
    fl = fw;
    fw = fw.add(16);
  }
  if m < end {
    debug_assert!(m + 1 == end);
    c += c;
    for k in 0..8usize {
      *fw.sub(8 - k) = *fw.sub(8 - k) + *fl.add(k) * c;
    }
  }
}

#[target_feature(enable = "avx2")]
unsafe fn v8dwt_decode_step2_avx2(
  mut fl: *mut OPJ_FLOAT32,
  mut fw: *mut OPJ_FLOAT32,
  start: u32,
  end: u32,
  m: u32,
  mut c: OPJ_FLOAT32,
) {
  use core::arch::x86_64::*;
  let imax = end.min(m);
  if start > 0 {
    fw = fw.add(16 * start as usize);
    fl = fw.sub(16);
  }
  let vc = _mm256_set1_ps(c);
  for _ in start..imax {
    let prev = _mm256_loadu_ps(fw.sub(8));
    let sum = _mm256_add_ps(_mm256_loadu_ps(fl), _mm256_loadu_ps(fw));
    _mm256_storeu_ps(fw.sub(8), _mm256_add_ps(prev, _mm256_mul_ps(sum, vc)));
    fl = fw;
    fw = fw.add(16);
  }
  if m < end {
    debug_assert!(m + 1 == end);
    c += c;
    let vc2 = _mm256_set1_ps(c);
    let prev = _mm256_loadu_ps(fw.sub(8));
    _mm256_storeu_ps(fw.sub(8), _mm256_add_ps(prev, _mm256_mul_ps(_mm256_loadu_ps(fl), vc2)));
  }
}
