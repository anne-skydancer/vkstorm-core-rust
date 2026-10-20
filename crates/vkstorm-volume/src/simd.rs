//! `LLVector4a`: a 16-byte aligned four-lane float vector with the lane-wise
//! operations the mesher uses. Every operation is one IEEE operation per lane
//! in the same order as the SSE code it mirrors, so the scalar form here is
//! bit-identical to the C++ (`_mm_min_ps`/`_mm_max_ps` return the second
//! operand when a lane compares unordered, which `if a < b { a } else { b }`
//! reproduces). The one approximate instruction, `_mm_rsqrt_ps`, is used
//! directly on x86-64; other architectures fall back to `1 / sqrt`.

/// `LLVector4a`: x, y, z, w.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V4(pub [f32; 4]);

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
        V4([
            self.0[0] + b.0[0],
            self.0[1] + b.0[1],
            self.0[2] + b.0[2],
            self.0[3] + b.0[3],
        ])
    }

    #[inline(always)]
    pub fn sub(self, b: V4) -> V4 {
        V4([
            self.0[0] - b.0[0],
            self.0[1] - b.0[1],
            self.0[2] - b.0[2],
            self.0[3] - b.0[3],
        ])
    }

    #[inline(always)]
    pub fn mul(self, b: V4) -> V4 {
        V4([
            self.0[0] * b.0[0],
            self.0[1] * b.0[1],
            self.0[2] * b.0[2],
            self.0[3] * b.0[3],
        ])
    }

    /// `LLVector4a::mul(F32)`: every lane, w included.
    #[inline(always)]
    pub fn mul_s(self, s: f32) -> V4 {
        self.mul(V4::splat(s))
    }

    /// `_mm_min_ps`: the second operand wins when the comparison is unordered.
    #[inline(always)]
    pub fn min(self, b: V4) -> V4 {
        let mut r = [0.0f32; 4];
        for i in 0..4 {
            r[i] = if self.0[i] < b.0[i] {
                self.0[i]
            } else {
                b.0[i]
            };
        }
        V4(r)
    }

    /// `_mm_max_ps`.
    #[inline(always)]
    pub fn max(self, b: V4) -> V4 {
        let mut r = [0.0f32; 4];
        for i in 0..4 {
            r[i] = if self.0[i] > b.0[i] {
                self.0[i]
            } else {
                b.0[i]
            };
        }
        V4(r)
    }

    /// `LLVector4a::setLerp`: `a + (b - a) * c`, every lane.
    #[inline(always)]
    pub fn lerp(a: V4, b: V4, c: f32) -> V4 {
        a.add(b.sub(a).mul_s(c))
    }

    /// `LLVector4a::setCross3`, including the w lane (`a.w*b.w - a.w*b.w`).
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

    /// `LLVector4a::dot3(...).getF32()`: `(x + y) + z`.
    #[inline(always)]
    pub fn dot3(a: V4, b: V4) -> f32 {
        let ab = a.mul(b);
        (ab.0[0] + ab.0[1]) + ab.0[2]
    }

    /// `LLVector4a::normalize3fast`: multiply every lane by the approximate
    /// reciprocal square root of the 3D length squared.
    #[inline(always)]
    pub fn normalize3fast(self) -> V4 {
        // setAllDot3 computes z*z + (x*x + y*y) in every lane; the sum is
        // commutative so it equals dot3.
        let ab = self.mul(self);
        let len_sqrd = ab.0[2] + (ab.0[0] + ab.0[1]);
        self.mul_s(rsqrt(len_sqrd))
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
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        #[cfg(target_arch = "x86")]
        use core::arch::x86::{_mm_cvtss_f32, _mm_rsqrt_ps, _mm_set1_ps};
        #[cfg(target_arch = "x86_64")]
        use core::arch::x86_64::{_mm_cvtss_f32, _mm_rsqrt_ps, _mm_set1_ps};
        // SAFETY: SSE is part of the x86-64 baseline (and required by the
        // viewer on 32-bit x86).
        unsafe { _mm_cvtss_f32(_mm_rsqrt_ps(_mm_set1_ps(x))) }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        1.0 / x.sqrt()
    }
}

/// `LLMatrix4a::rotate`: `x*row0 + y*row1 + z*row2` with the w lane along.
#[inline(always)]
pub fn rotate(rows: &[V4; 4], v: V4) -> V4 {
    let res = V4::splat(v.0[0]).mul(rows[0]);
    let y = V4::splat(v.0[1]).mul(rows[1]);
    let z = V4::splat(v.0[2]).mul(rows[2]);
    res.add(y).add(z)
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
