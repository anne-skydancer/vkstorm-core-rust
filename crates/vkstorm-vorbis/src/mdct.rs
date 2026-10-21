//! The inverse MDCT of libvorbis 1.3.7 (`mdct.c`, float build): trig and
//! bit-reverse tables, butterflies and `mdct_backward`, with the C's
//! operation order (every product and sum in float, no fused
//! multiply-add).
//!
//! The trig table is built with libm `cos`/`sin` in double as in
//! `mdct_init`. With MSVC, `M_PI` is `os.h`'s float `3.1415926536f`
//! (MSVC's `math.h` defines no `M_PI` without `_USE_MATH_DEFINES`), so
//! `(M_PI/n)*(4*i)` is evaluated in float before the double `cos`; with
//! GCC/glibc `M_PI` is a double and the argument is computed in double.
//! The tables, and through them the decoded PCM, therefore differ in the
//! last bit between the Windows and Linux builds of the C library; this
//! port follows the platform it is built for.
//!
//! Original code: Copyright (c) 1994-2009 Xiph.Org Foundation,
//! BSD-3-Clause (see `LICENSE-xiph`). Rust port: Vulkanstorm.

const C_PI3_8: f32 = 0.382_683_432_365_089_771_75;
const C_PI2_8: f32 = 0.707_106_781_186_547_524_41;
const C_PI1_8: f32 = 0.923_879_532_511_286_756_13;

/// `mdct_lookup`.
#[derive(Clone, Debug)]
pub(crate) struct MdctLookup {
    n: usize,
    log2n: u32,
    trig: Vec<f32>,
    bitrev: Vec<i32>,
}

/// The `cos`/`sin` arguments of `mdct_init`: `(M_PI/den)*mul` as the C
/// computes them on this platform.
#[cfg(all(windows, target_env = "msvc"))]
#[inline]
fn arg(den: usize, mul: usize) -> f64 {
    ((3.141_592_653_6_f32 / den as f32) * mul as f32) as f64
}
#[cfg(not(all(windows, target_env = "msvc")))]
#[inline]
fn arg(den: usize, mul: usize) -> f64 {
    (std::f64::consts::PI / den as f64) * mul as f64
}

impl MdctLookup {
    /// `mdct_init` for a power-of-two size `n` (64..=8192).
    pub fn new(n: usize) -> Self {
        let n2 = n >> 1;
        let log2n = n.trailing_zeros();
        let mut t = vec![0f32; n + n / 4];
        for i in 0..n / 4 {
            t[i * 2] = arg(n, 4 * i).cos() as f32;
            t[i * 2 + 1] = (-arg(n, 4 * i).sin()) as f32;
            t[n2 + i * 2] = arg(2 * n, 2 * i + 1).cos() as f32;
            t[n2 + i * 2 + 1] = arg(2 * n, 2 * i + 1).sin() as f32;
        }
        for i in 0..n / 8 {
            t[n + i * 2] = (arg(n, 4 * i + 2).cos() * 0.5) as f32;
            t[n + i * 2 + 1] = (-arg(n, 4 * i + 2).sin() * 0.5) as f32;
        }
        let mut bitrev = vec![0i32; n / 4];
        let mask: i32 = (1 << (log2n - 1)) - 1;
        let msb: i32 = 1 << (log2n - 2);
        for i in 0..n / 8 {
            let mut acc: i32 = 0;
            let mut j = 0;
            while (msb >> j) != 0 {
                if (msb >> j) & i as i32 != 0 {
                    acc |= 1 << j;
                }
                j += 1;
            }
            bitrev[i * 2] = ((!acc) & mask) - 1;
            bitrev[i * 2 + 1] = acc;
        }
        Self {
            n,
            log2n,
            trig: t,
            bitrev,
        }
    }

    /// The trig table (for parity tests against the C).
    #[allow(dead_code)]
    pub fn trig(&self) -> &[f32] {
        &self.trig
    }

    /// `mdct_backward(init, buf, buf)`: in place on `buf[..n]`, reading the
    /// first `n/2` spectral values.
    pub fn backward(&self, buf: &mut [f32]) {
        let n = self.n;
        let n2 = n >> 1;
        let n4 = n >> 2;
        let out = &mut buf[..n];
        let t = &self.trig[..];

        // rotate: reads [0,n2), writes [n2,n)
        {
            let (inp, hi) = out.split_at_mut(n2);
            let mut ix = n2 as isize - 7;
            let mut ox = n4; // relative to hi (out+n2)
            let mut ti = n4;
            while ix >= 0 {
                let i = ix as usize;
                ox -= 4;
                let tt = &t[ti..ti + 4];
                hi[ox] = -inp[i + 2] * tt[3] - inp[i] * tt[2];
                hi[ox + 1] = inp[i] * tt[3] - inp[i + 2] * tt[2];
                hi[ox + 2] = -inp[i + 6] * tt[1] - inp[i + 4] * tt[0];
                hi[ox + 3] = inp[i + 4] * tt[1] - inp[i + 6] * tt[0];
                ix -= 8;
                ti += 4;
            }
            let mut ix = n2 as isize - 8;
            let mut ox = n4;
            let mut ti = n4;
            while ix >= 0 {
                let i = ix as usize;
                ti -= 4;
                let tt = &t[ti..ti + 4];
                hi[ox] = inp[i + 4] * tt[3] + inp[i + 6] * tt[2];
                hi[ox + 1] = inp[i + 4] * tt[2] - inp[i + 6] * tt[3];
                hi[ox + 2] = inp[i] * tt[1] + inp[i + 2] * tt[0];
                hi[ox + 3] = inp[i] * tt[0] - inp[i + 2] * tt[1];
                ix -= 8;
                ox += 4;
            }
        }

        self.butterflies(&mut out[n2..], n2);
        self.bitreverse(out);

        // rotate + window
        {
            let (lo, hi) = out.split_at_mut(n2);
            // oX1 counts down from n4 (relative to hi), oX2 up from n4
            let mut ox1 = n4;
            let mut ox2 = n4;
            let mut ix = 0usize;
            let mut ti = n2;
            loop {
                ox1 -= 4;
                let x = &lo[ix..ix + 8];
                let tt = &t[ti..ti + 8];
                hi[ox1 + 3] = x[0] * tt[1] - x[1] * tt[0];
                hi[ox2] = -(x[0] * tt[0] + x[1] * tt[1]);
                hi[ox1 + 2] = x[2] * tt[3] - x[3] * tt[2];
                hi[ox2 + 1] = -(x[2] * tt[2] + x[3] * tt[3]);
                hi[ox1 + 1] = x[4] * tt[5] - x[5] * tt[4];
                hi[ox2 + 2] = -(x[4] * tt[4] + x[5] * tt[5]);
                hi[ox1] = x[6] * tt[7] - x[7] * tt[6];
                hi[ox2 + 3] = -(x[6] * tt[6] + x[7] * tt[7]);
                ox2 += 4;
                ix += 8;
                ti += 8;
                if ix >= ox1 + n2 {
                    break;
                }
            }

            // reads [n2, n2+n4), writes [0, n2)
            let mut ix = n4; // relative to hi
            let mut ox1 = n4;
            let mut ox2 = n4;
            loop {
                ox1 -= 4;
                ix -= 4;
                lo[ox1 + 3] = hi[ix + 3];
                lo[ox2] = -hi[ix + 3];
                lo[ox1 + 2] = hi[ix + 2];
                lo[ox2 + 1] = -hi[ix + 2];
                lo[ox1 + 1] = hi[ix + 1];
                lo[ox2 + 2] = -hi[ix + 1];
                lo[ox1] = hi[ix];
                lo[ox2 + 3] = -hi[ix];
                ox2 += 4;
                if ox2 >= ix + n2 {
                    break;
                }
            }

            // reads [n2+n4, n), writes [n2, n2+n4)
            let (dst, src) = hi.split_at_mut(n4);
            let mut ix = 0usize;
            let mut ox1 = n4;
            loop {
                ox1 -= 4;
                dst[ox1] = src[ix + 3];
                dst[ox1 + 1] = src[ix + 2];
                dst[ox1 + 2] = src[ix + 1];
                dst[ox1 + 3] = src[ix];
                ix += 4;
                if ox1 == 0 {
                    break;
                }
            }
        }
    }

    fn butterflies(&self, x: &mut [f32], points: usize) {
        let t = &self.trig[..];
        let mut stages = self.log2n as i32 - 5;
        stages -= 1;
        if stages > 0 {
            butterfly_first(t, x, points);
        }
        let mut i = 1;
        loop {
            stages -= 1;
            if stages <= 0 {
                break;
            }
            for j in 0..(1usize << i) {
                let p = points >> i;
                butterfly_generic(t, &mut x[p * j..p * j + p], p, 4 << i);
            }
            i += 1;
        }
        for chunk in x[..points].as_chunks_mut::<32>().0 {
            butterfly_32(chunk);
        }
    }

    fn bitreverse(&self, x: &mut [f32]) {
        let n = self.n;
        let n2 = n >> 1;
        let (w, xs) = x.split_at_mut(n2); // writes go to w = [0,n2), reads from xs = x+n2
        let t = &self.trig[n..];
        let bit = &self.bitrev[..];
        let mut w0 = 0usize;
        let mut w1 = n2;
        let mut ti = 0usize;
        let mut bi = 0usize;
        loop {
            let x0 = bit[bi] as usize;
            let x1 = bit[bi + 1] as usize;
            let r0 = xs[x0 + 1] - xs[x1 + 1];
            let r1 = xs[x0] + xs[x1];
            let r2 = r1 * t[ti] + r0 * t[ti + 1];
            let r3 = r1 * t[ti + 1] - r0 * t[ti];
            w1 -= 4;
            let r0 = (xs[x0 + 1] + xs[x1 + 1]) * 0.5f32;
            let r1 = (xs[x0] - xs[x1]) * 0.5f32;
            w[w0] = r0 + r2;
            w[w1 + 2] = r0 - r2;
            w[w0 + 1] = r1 + r3;
            w[w1 + 3] = r3 - r1;

            let x0 = bit[bi + 2] as usize;
            let x1 = bit[bi + 3] as usize;
            let r0 = xs[x0 + 1] - xs[x1 + 1];
            let r1 = xs[x0] + xs[x1];
            let r2 = r1 * t[ti + 2] + r0 * t[ti + 3];
            let r3 = r1 * t[ti + 3] - r0 * t[ti + 2];
            let r0 = (xs[x0 + 1] + xs[x1 + 1]) * 0.5f32;
            let r1 = (xs[x0] - xs[x1]) * 0.5f32;
            w[w0 + 2] = r0 + r2;
            w[w1] = r0 - r2;
            w[w0 + 3] = r1 + r3;
            w[w1 + 1] = r3 - r1;

            ti += 4;
            bi += 4;
            w0 += 4;
            if w0 >= w1 {
                break;
            }
        }
    }
}

#[inline(always)]
fn butterfly_8(x: &mut [f32; 8]) {
    let r0 = x[6] + x[2];
    let r1 = x[6] - x[2];
    let r2 = x[4] + x[0];
    let r3 = x[4] - x[0];

    x[6] = r0 + r2;
    x[4] = r0 - r2;

    let r0 = x[5] - x[1];
    let r2 = x[7] - x[3];
    x[0] = r1 + r0;
    x[2] = r1 - r0;

    let r0 = x[5] + x[1];
    let r1 = x[7] + x[3];
    x[3] = r2 + r3;
    x[1] = r2 - r3;
    x[7] = r1 + r0;
    x[5] = r1 - r0;
}

#[inline(always)]
fn butterfly_16(x: &mut [f32; 16]) {
    let r0 = x[1] - x[9];
    let r1 = x[0] - x[8];
    x[8] += x[0];
    x[9] += x[1];
    x[0] = (r0 + r1) * C_PI2_8;
    x[1] = (r0 - r1) * C_PI2_8;

    let r0 = x[3] - x[11];
    let r1 = x[10] - x[2];
    x[10] += x[2];
    x[11] += x[3];
    x[2] = r0;
    x[3] = r1;

    let r0 = x[12] - x[4];
    let r1 = x[13] - x[5];
    x[12] += x[4];
    x[13] += x[5];
    x[4] = (r0 - r1) * C_PI2_8;
    x[5] = (r0 + r1) * C_PI2_8;

    let r0 = x[14] - x[6];
    let r1 = x[15] - x[7];
    x[14] += x[6];
    x[15] += x[7];
    x[6] = r0;
    x[7] = r1;

    let (a, b) = x.split_at_mut(8);
    butterfly_8(a.try_into().expect("8"));
    butterfly_8(b.try_into().expect("8"));
}

#[inline(always)]
fn butterfly_32(x: &mut [f32; 32]) {
    let r0 = x[30] - x[14];
    let r1 = x[31] - x[15];
    x[30] += x[14];
    x[31] += x[15];
    x[14] = r0;
    x[15] = r1;

    let r0 = x[28] - x[12];
    let r1 = x[29] - x[13];
    x[28] += x[12];
    x[29] += x[13];
    x[12] = r0 * C_PI1_8 - r1 * C_PI3_8;
    x[13] = r0 * C_PI3_8 + r1 * C_PI1_8;

    let r0 = x[26] - x[10];
    let r1 = x[27] - x[11];
    x[26] += x[10];
    x[27] += x[11];
    x[10] = (r0 - r1) * C_PI2_8;
    x[11] = (r0 + r1) * C_PI2_8;

    let r0 = x[24] - x[8];
    let r1 = x[25] - x[9];
    x[24] += x[8];
    x[25] += x[9];
    x[8] = r0 * C_PI3_8 - r1 * C_PI1_8;
    x[9] = r1 * C_PI3_8 + r0 * C_PI1_8;

    let r0 = x[22] - x[6];
    let r1 = x[7] - x[23];
    x[22] += x[6];
    x[23] += x[7];
    x[6] = r1;
    x[7] = r0;

    let r0 = x[4] - x[20];
    let r1 = x[5] - x[21];
    x[20] += x[4];
    x[21] += x[5];
    x[4] = r1 * C_PI1_8 + r0 * C_PI3_8;
    x[5] = r1 * C_PI3_8 - r0 * C_PI1_8;

    let r0 = x[2] - x[18];
    let r1 = x[3] - x[19];
    x[18] += x[2];
    x[19] += x[3];
    x[2] = (r1 + r0) * C_PI2_8;
    x[3] = (r1 - r0) * C_PI2_8;

    let r0 = x[0] - x[16];
    let r1 = x[1] - x[17];
    x[16] += x[0];
    x[17] += x[1];
    x[0] = r1 * C_PI3_8 + r0 * C_PI1_8;
    x[1] = r1 * C_PI1_8 - r0 * C_PI3_8;

    let (a, b) = x.split_at_mut(16);
    butterfly_16(a.try_into().expect("16"));
    butterfly_16(b.try_into().expect("16"));
}

/// One 8-value step shared by the first-stage and generic butterflies:
/// `x1`/`x2` are the upper and lower halves' 8-blocks.
#[inline(always)]
fn butterfly_step(x1: &mut [f32], x2: &mut [f32], k: usize, t0: f32, t1: f32) {
    let r0 = x1[k] - x2[k];
    let r1 = x1[k + 1] - x2[k + 1];
    x1[k] += x2[k];
    x1[k + 1] += x2[k + 1];
    x2[k] = r1 * t1 + r0 * t0;
    x2[k + 1] = r1 * t0 - r0 * t1;
}

/// `mdct_butterfly_first`.
fn butterfly_first(t: &[f32], x: &mut [f32], points: usize) {
    let half = points >> 1;
    let (lo, hi) = x[..points].split_at_mut(half);
    let iters = points / 16;
    for it in 0..iters {
        let off = half - 8 - 8 * it;
        let ti = 16 * it;
        let x1 = &mut hi[off..off + 8];
        let x2 = &mut lo[off..off + 8];
        butterfly_step(x1, x2, 6, t[ti], t[ti + 1]);
        butterfly_step(x1, x2, 4, t[ti + 4], t[ti + 5]);
        butterfly_step(x1, x2, 2, t[ti + 8], t[ti + 9]);
        butterfly_step(x1, x2, 0, t[ti + 12], t[ti + 13]);
    }
}

/// `mdct_butterfly_generic`.
fn butterfly_generic(t: &[f32], x: &mut [f32], points: usize, trigint: usize) {
    let half = points >> 1;
    let (lo, hi) = x[..points].split_at_mut(half);
    let iters = points / 16;
    let mut ti = 0usize;
    for it in 0..iters {
        let off = half - 8 - 8 * it;
        let x1 = &mut hi[off..off + 8];
        let x2 = &mut lo[off..off + 8];
        butterfly_step(x1, x2, 6, t[ti], t[ti + 1]);
        ti += trigint;
        butterfly_step(x1, x2, 4, t[ti], t[ti + 1]);
        ti += trigint;
        butterfly_step(x1, x2, 2, t[ti], t[ti + 1]);
        ti += trigint;
        butterfly_step(x1, x2, 0, t[ti], t[ti + 1]);
        ti += trigint;
    }
}
