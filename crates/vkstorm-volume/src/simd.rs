//! `LLVector4a`: a 16-byte aligned four-lane float vector with the lane-wise
//! operations the mesher uses. On x86 the operations are the SSE
//! instructions the C++ issues (`_mm_add_ps`, `_mm_mul_ps`, `_mm_min_ps`,
//! the `setCross3` shuffles, `_mm_rsqrt_ps`), which keeps both the results
//! and the memory traffic identical: one 16-byte load or store per vector.
//! The scalar fallback for other architectures performs the same IEEE
//! operation per lane in the same order (`_mm_min_ps`/`_mm_max_ps` return
//! the second operand when a lane compares unordered, which
//! `if a < b { a } else { b }` reproduces); only `rsqrt` differs there, as a
//! hardware approximation has no scalar equivalent.

#[cfg(target_arch = "x86")]
use core::arch::x86::*;
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

/// `LLVector4a`: x, y, z, w.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V4(pub [f32; 4]);

#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
mod sse {
    use super::*;

    impl V4 {
        #[inline(always)]
        pub(super) fn q(self) -> __m128 {
            // SAFETY: V4 is 16-byte aligned and SSE is part of the baseline.
            unsafe { _mm_load_ps(self.0.as_ptr()) }
        }

        #[inline(always)]
        pub(super) fn from_q(q: __m128) -> V4 {
            let mut v = V4::ZERO;
            // SAFETY: as above.
            unsafe { _mm_store_ps(v.0.as_mut_ptr(), q) };
            v
        }
    }

    #[inline(always)]
    pub fn add(a: V4, b: V4) -> V4 {
        V4::from_q(unsafe { _mm_add_ps(a.q(), b.q()) })
    }

    #[inline(always)]
    pub fn sub(a: V4, b: V4) -> V4 {
        V4::from_q(unsafe { _mm_sub_ps(a.q(), b.q()) })
    }

    #[inline(always)]
    pub fn mul(a: V4, b: V4) -> V4 {
        V4::from_q(unsafe { _mm_mul_ps(a.q(), b.q()) })
    }

    #[inline(always)]
    pub fn min(a: V4, b: V4) -> V4 {
        V4::from_q(unsafe { _mm_min_ps(a.q(), b.q()) })
    }

    #[inline(always)]
    pub fn max(a: V4, b: V4) -> V4 {
        V4::from_q(unsafe { _mm_max_ps(a.q(), b.q()) })
    }

    /// `LLVector4a::setCross3`, shuffle for shuffle.
    #[inline(always)]
    pub fn cross3(a: V4, b: V4) -> V4 {
        unsafe {
            let a = a.q();
            let b = b.q();
            // Set vector1 = { a[W], a[X], a[Z], a[Y] }
            let vector1 = _mm_shuffle_ps::<{ shuffle(3, 0, 2, 1) }>(a, a);
            // Set vector2 = { b[W], b[Y], b[X], b[Z] }
            let vector2 = _mm_shuffle_ps::<{ shuffle(3, 1, 0, 2) }>(b, b);
            // mQ = { a[W]*b[W], a[X]*b[Y], a[Z]*b[X], a[Y]*b[Z] }
            let m = _mm_mul_ps(vector1, vector2);
            // vector3 = { a[W], a[Y], a[X], a[Z] }
            let vector3 = _mm_shuffle_ps::<{ shuffle(3, 1, 0, 2) }>(a, a);
            // vector4 = { b[W], b[X], b[Z], b[Y] }
            let vector4 = _mm_shuffle_ps::<{ shuffle(3, 0, 2, 1) }>(b, b);
            V4::from_q(_mm_sub_ps(m, _mm_mul_ps(vector3, vector4)))
        }
    }

    /// `LLVector4a::dot3(...).getF32()`: `(x + y) + z` through the same
    /// splats.
    #[inline(always)]
    pub fn dot3(a: V4, b: V4) -> f32 {
        unsafe {
            let ab = _mm_mul_ps(a.q(), b.q());
            let splat_y = _mm_shuffle_ps::<{ shuffle(1, 1, 1, 1) }>(ab, ab);
            let splat_z = _mm_shuffle_ps::<{ shuffle(2, 2, 2, 2) }>(ab, ab);
            let x_plus_y = _mm_add_ps(ab, splat_y);
            _mm_cvtss_f32(_mm_add_ps(x_plus_y, splat_z))
        }
    }

    /// `LLVector4a::normalize3fast`: `setAllDot3`, `_mm_rsqrt_ps`, multiply.
    #[inline(always)]
    pub fn normalize3fast(v: V4) -> V4 {
        unsafe {
            let q = v.q();
            // ab = { a[W]*b[W], a[Z]*b[Z], a[Y]*b[Y], a[X]*b[X] }
            let ab = _mm_mul_ps(q, q);
            // wzxy = { a[W]*b[W], a[Z]*b[Z], a[X]*b[X], a[Y]*b[Y] }
            let wzxy = _mm_shuffle_ps::<{ shuffle(3, 2, 0, 1) }>(ab, ab);
            let x_plus_y = _mm_add_ps(ab, wzxy);
            let x_plus_y_splat = _mm_movelh_ps(x_plus_y, x_plus_y);
            let z_splat = _mm_shuffle_ps::<{ shuffle(2, 2, 2, 2) }>(ab, ab);
            let len_sqrd = _mm_add_ps(z_splat, x_plus_y_splat);
            V4::from_q(_mm_mul_ps(q, _mm_rsqrt_ps(len_sqrd)))
        }
    }

    /// `_mm_rsqrt_ps` on one value.
    #[inline(always)]
    pub fn rsqrt(x: f32) -> f32 {
        unsafe { _mm_cvtss_f32(_mm_rsqrt_ps(_mm_set1_ps(x))) }
    }

    /// `LLMatrix4a::rotate`.
    #[inline(always)]
    pub fn rotate(rows: &[V4; 4], v: V4) -> V4 {
        unsafe {
            let q = v.q();
            let mut res = _mm_shuffle_ps::<{ shuffle(0, 0, 0, 0) }>(q, q);
            let mut y = _mm_shuffle_ps::<{ shuffle(1, 1, 1, 1) }>(q, q);
            let mut z = _mm_shuffle_ps::<{ shuffle(2, 2, 2, 2) }>(q, q);
            res = _mm_mul_ps(res, rows[0].q());
            y = _mm_mul_ps(y, rows[1].q());
            z = _mm_mul_ps(z, rows[2].q());
            res = _mm_add_ps(res, y);
            V4::from_q(_mm_add_ps(res, z))
        }
    }

    /// `_MM_SHUFFLE(z, y, x, w)`.
    pub const fn shuffle(z: i32, y: i32, x: i32, w: i32) -> i32 {
        (z << 6) | (y << 4) | (x << 2) | w
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
mod sse {
    use super::*;

    #[inline(always)]
    pub fn add(a: V4, b: V4) -> V4 {
        V4([
            a.0[0] + b.0[0],
            a.0[1] + b.0[1],
            a.0[2] + b.0[2],
            a.0[3] + b.0[3],
        ])
    }

    #[inline(always)]
    pub fn sub(a: V4, b: V4) -> V4 {
        V4([
            a.0[0] - b.0[0],
            a.0[1] - b.0[1],
            a.0[2] - b.0[2],
            a.0[3] - b.0[3],
        ])
    }

    #[inline(always)]
    pub fn mul(a: V4, b: V4) -> V4 {
        V4([
            a.0[0] * b.0[0],
            a.0[1] * b.0[1],
            a.0[2] * b.0[2],
            a.0[3] * b.0[3],
        ])
    }

    #[inline(always)]
    pub fn min(a: V4, b: V4) -> V4 {
        let mut r = [0.0f32; 4];
        for i in 0..4 {
            r[i] = if a.0[i] < b.0[i] { a.0[i] } else { b.0[i] };
        }
        V4(r)
    }

    #[inline(always)]
    pub fn max(a: V4, b: V4) -> V4 {
        let mut r = [0.0f32; 4];
        for i in 0..4 {
            r[i] = if a.0[i] > b.0[i] { a.0[i] } else { b.0[i] };
        }
        V4(r)
    }

    #[inline(always)]
    pub fn cross3(a: V4, b: V4) -> V4 {
        let [ax, ay, az, aw] = a.0;
        let [bx, by, bz, bw] = b.0;
        V4([
            ay * bz - az * by,
            az * bx - ax * bz,
            ax * by - ay * bx,
            aw * bw - aw * bw,
        ])
    }

    #[inline(always)]
    pub fn dot3(a: V4, b: V4) -> f32 {
        let ab = mul(a, b);
        (ab.0[0] + ab.0[1]) + ab.0[2]
    }

    #[inline(always)]
    pub fn normalize3fast(v: V4) -> V4 {
        let ab = mul(v, v);
        let len_sqrd = ab.0[2] + (ab.0[0] + ab.0[1]);
        mul(v, V4::splat(rsqrt(len_sqrd)))
    }

    #[inline(always)]
    pub fn rsqrt(x: f32) -> f32 {
        1.0 / x.sqrt()
    }

    #[inline(always)]
    pub fn rotate(rows: &[V4; 4], v: V4) -> V4 {
        let res = mul(V4::splat(v.0[0]), rows[0]);
        let y = mul(V4::splat(v.0[1]), rows[1]);
        let z = mul(V4::splat(v.0[2]), rows[2]);
        add(add(res, y), z)
    }
}

impl V4 {
    pub const ZERO: V4 = V4([0.0; 4]);

    #[inline(always)]
    pub fn new(x: f32, y: f32, z: f32, w: f32) -> V4 {
        V4([x, y, z, w])
    }

    /// `LLVector4a::set(x, y, z)`: w is zero.
    #[inline(always)]
    pub fn set3(x: f32, y: f32, z: f32) -> V4 {
        V4([x, y, z, 0.0])
    }

    #[inline(always)]
    pub fn splat(x: f32) -> V4 {
        V4([x; 4])
    }

    #[inline(always)]
    pub fn x(self) -> f32 {
        self.0[0]
    }
    #[inline(always)]
    pub fn y(self) -> f32 {
        self.0[1]
    }
    #[inline(always)]
    pub fn z(self) -> f32 {
        self.0[2]
    }

    #[inline(always)]
    pub fn add(self, b: V4) -> V4 {
        sse::add(self, b)
    }

    #[inline(always)]
    pub fn sub(self, b: V4) -> V4 {
        sse::sub(self, b)
    }

    #[inline(always)]
    pub fn mul(self, b: V4) -> V4 {
        sse::mul(self, b)
    }

    /// `LLVector4a::mul(F32)`: every lane, w included.
    #[inline(always)]
    pub fn mul_s(self, s: f32) -> V4 {
        sse::mul(self, V4::splat(s))
    }

    /// `_mm_min_ps`: the second operand wins when the comparison is unordered.
    #[inline(always)]
    pub fn min(self, b: V4) -> V4 {
        sse::min(self, b)
    }

    /// `_mm_max_ps`.
    #[inline(always)]
    pub fn max(self, b: V4) -> V4 {
        sse::max(self, b)
    }

    /// `LLVector4a::setLerp`: `a + (b - a) * c`, every lane.
    #[inline(always)]
    pub fn lerp(a: V4, b: V4, c: f32) -> V4 {
        a.add(b.sub(a).mul_s(c))
    }

    /// `LLVector4a::setCross3`, including the w lane (`a.w*b.w - a.w*b.w`).
    #[inline(always)]
    pub fn cross3(a: V4, b: V4) -> V4 {
        sse::cross3(a, b)
    }

    /// `LLVector4a::dot3(...).getF32()`: `(x + y) + z`.
    #[inline(always)]
    pub fn dot3(a: V4, b: V4) -> f32 {
        sse::dot3(a, b)
    }

    /// `LLVector4a::normalize3fast`: multiply every lane by the approximate
    /// reciprocal square root of the 3D length squared.
    #[inline(always)]
    pub fn normalize3fast(self) -> V4 {
        sse::normalize3fast(self)
    }

    /// `LLVector4a::isFinite3`.
    #[inline(always)]
    pub fn is_finite3(self) -> bool {
        self.0[..3]
            .iter()
            .all(|v| v.to_bits() & 0x7f80_0000 != 0x7f80_0000)
    }
}

/// `_mm_rsqrt_ps` on one value: the hardware approximation the C++ uses.
#[inline(always)]
pub fn rsqrt(x: f32) -> f32 {
    sse::rsqrt(x)
}

/// `LLMatrix4a::rotate`: `x*row0 + y*row1 + z*row2` with the w lane along.
#[inline(always)]
pub fn rotate(rows: &[V4; 4], v: V4) -> V4 {
    sse::rotate(rows, v)
}

/// `update_min_max` on `LLVector4a`.
#[inline(always)]
pub fn update_min_max(min: &mut V4, max: &mut V4, p: V4) {
    *min = min.min(p);
    *max = max.max(p);
}

/// `update_min_max` on `LLVector2`: scalar comparisons, lane by lane.
#[inline(always)]
pub fn update_min_max2(min: &mut [f32; 2], max: &mut [f32; 2], p: [f32; 2]) {
    for i in 0..2 {
        if min[i] > p[i] {
            min[i] = p[i];
        }
        if max[i] < p[i] {
            max[i] = p[i];
        }
    }
}

/// `llmin`: `(a < b) ? a : b`.
#[inline(always)]
pub fn llmin(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

/// `llmax`: `(a > b) ? a : b`.
#[inline(always)]
pub fn llmax(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}
