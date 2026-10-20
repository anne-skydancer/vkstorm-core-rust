//! Parameter records and the constants of `llvolume.h`, plus the scalar
//! helpers of `llmath.h` the mesher relies on, each with the C++ semantics.

pub const F_PI: f32 = 3.1415926535897932384626433832795;
pub const F_TWO_PI: f32 = 6.283185307179586476925286766559;
pub const F_APPROXIMATELY_ZERO: f32 = 0.00001;
pub const FP_MAG_THRESHOLD: f32 = 0.0000001;
pub const MIN_DETAIL_FACES: i32 = 6;
pub const MIN_LOD: f32 = 0.0;

pub const PROFILE_MASK: u8 = 0x0f;
pub const PROFILE_CIRCLE: u8 = 0x00;
pub const PROFILE_SQUARE: u8 = 0x01;
pub const PROFILE_ISOTRI: u8 = 0x02;
pub const PROFILE_EQUALTRI: u8 = 0x03;
pub const PROFILE_RIGHTTRI: u8 = 0x04;
pub const PROFILE_CIRCLE_HALF: u8 = 0x05;

pub const HOLE_MASK: u8 = 0xf0;
pub const HOLE_SAME: u8 = 0x00;
pub const HOLE_CIRCLE: u8 = 0x10;
pub const HOLE_SQUARE: u8 = 0x20;
pub const HOLE_TRIANGLE: u8 = 0x30;

pub const PATH_LINE: u8 = 0x10;
pub const PATH_CIRCLE: u8 = 0x20;
pub const PATH_CIRCLE2: u8 = 0x30;
pub const PATH_TEST: u8 = 0x40;
pub const PATH_FLEXIBLE: u8 = 0x80;

pub const FACE_PATH_BEGIN: u16 = 1 << 0;
pub const FACE_PATH_END: u16 = 1 << 1;
pub const FACE_INNER_SIDE: u16 = 1 << 2;
pub const FACE_PROFILE_BEGIN: u16 = 1 << 3;
pub const FACE_PROFILE_END: u16 = 1 << 4;
pub const FACE_OUTER_SIDE_0: u16 = 1 << 5;

pub const SCULPT_TYPE_NONE: u8 = 0;
pub const SCULPT_TYPE_SPHERE: u8 = 1;
pub const SCULPT_TYPE_TORUS: u8 = 2;
pub const SCULPT_TYPE_PLANE: u8 = 3;
pub const SCULPT_TYPE_CYLINDER: u8 = 4;
pub const SCULPT_TYPE_MESH: u8 = 5;
pub const SCULPT_TYPE_GLTF: u8 = 6;
pub const SCULPT_TYPE_MASK: u8 = SCULPT_TYPE_SPHERE
    | SCULPT_TYPE_TORUS
    | SCULPT_TYPE_PLANE
    | SCULPT_TYPE_CYLINDER
    | SCULPT_TYPE_MESH
    | SCULPT_TYPE_GLTF;
pub const SCULPT_FLAG_INVERT: u8 = 64;
pub const SCULPT_FLAG_MIRROR: u8 = 128;

// LLVolumeFace type mask bits.
pub const SINGLE_MASK: u32 = 0x0001;
pub const CAP_MASK: u32 = 0x0002;
pub const END_MASK: u32 = 0x0004;
pub const SIDE_MASK: u32 = 0x0008;
pub const INNER_MASK: u32 = 0x0010;
pub const OUTER_MASK: u32 = 0x0020;
pub const HOLLOW_MASK: u32 = 0x0040;
pub const OPEN_MASK: u32 = 0x0080;
pub const FLAT_MASK: u32 = 0x0100;
pub const TOP_MASK: u32 = 0x0200;
pub const BOTTOM_MASK: u32 = 0x0400;

/// `LLProfileParams`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProfileParams {
    pub curve: u8,
    pub begin: f32,
    pub end: f32,
    pub hollow: f32,
}

/// `LLPathParams`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathParams {
    pub curve: u8,
    pub begin: f32,
    pub end: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub shear_x: f32,
    pub shear_y: f32,
    pub twist_begin: f32,
    pub twist_end: f32,
    pub radius_offset: f32,
    pub taper_x: f32,
    pub taper_y: f32,
    pub revolutions: f32,
    pub skew: f32,
}

impl PathParams {
    /// `LLPathParams::getBeginScale`.
    pub fn begin_scale(&self) -> [f32; 2] {
        let mut s = [1.0f32, 1.0];
        if self.scale_x > 1.0 {
            s[0] = 2.0 - self.scale_x;
        }
        if self.scale_y > 1.0 {
            s[1] = 2.0 - self.scale_y;
        }
        s
    }

    /// `LLPathParams::getEndScale`.
    pub fn end_scale(&self) -> [f32; 2] {
        let mut s = [1.0f32, 1.0];
        if self.scale_x < 1.0 {
            s[0] = self.scale_x;
        }
        if self.scale_y < 1.0 {
            s[1] = self.scale_y;
        }
        s
    }
}

/// `std::lerp(float, float, float)` as the viewer's C++ standard library
/// implements it on this platform, which is what `llmath.h`'s `lerp`
/// resolves to: the MSVC STL at run time (`_Common_lerp` with
/// `_Linear_for_lerp` = `fma(t, b - a, a)`), libstdc++ elsewhere
/// (`a + t * (b - a)`, two roundings; the viewer's Linux build is strict
/// `-std=c++20` without `-march`, so GCC neither contracts it nor has FMA).
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    if cfg!(target_env = "msvc") {
        lerp_msvc(a, b, t)
    } else {
        lerp_libstdcxx(a, b, t)
    }
}

/// libstdc++'s `std::__lerp` on doubles: what `lerp(0.f, -sin(x) * 0.5f, t)`
/// in the test path resolves to with GCC, where `sin(float)` is the C double
/// function and the mixed arguments promote to the double overload.
#[inline]
pub fn lerp_libstdcxx_f64(a: f64, b: f64, t: f64) -> f64 {
    if (a <= 0.0 && b >= 0.0) || (a >= 0.0 && b <= 0.0) {
        return t * b + (1.0 - t) * a;
    }
    if t == 1.0 {
        return b;
    }
    let x = a + t * (b - a);
    if (t > 1.0) == (b > a) {
        if b < x { x } else { b }
    } else if b > x {
        x
    } else {
        b
    }
}

/// `llvolume.cpp` calls `sin`, `cos`, `floor` and `fabs` unqualified on
/// floats. With MSVC the global float overloads of `<math.h>` bind (`sinf`
/// and friends); with GCC and libstdc++ only `std::` has the float
/// overloads, so the C double functions bind and the surrounding
/// expression is evaluated in double before the store to `F32`. These
/// helpers reproduce each platform's resolution for the expressions used.
pub const C_MATH_IS_DOUBLE: bool = !cfg!(target_env = "msvc");

/// `sin(ang) * k` stored to an `F32`.
#[inline]
pub fn sin_mul(ang: f32, k: f32) -> f32 {
    if C_MATH_IS_DOUBLE {
        ((ang as f64).sin() * k as f64) as f32
    } else {
        ang.sin() * k
    }
}

/// `cos(ang) * k` stored to an `F32`.
#[inline]
pub fn cos_mul(ang: f32, k: f32) -> f32 {
    if C_MATH_IS_DOUBLE {
        ((ang as f64).cos() * k as f64) as f32
    } else {
        ang.cos() * k
    }
}

/// `floor(x) / d` stored to an `F32`.
#[inline]
pub fn floor_div(x: f32, d: f32) -> f32 {
    if C_MATH_IS_DOUBLE {
        ((x as f64).floor() / d as f64) as f32
    } else {
        x.floor() / d
    }
}

/// `llfloor(fabs(a) * b * c)`.
#[inline]
pub fn llfloor_fabs_mul_mul(a: f32, b: f32, c: f32) -> i32 {
    if C_MATH_IS_DOUBLE {
        ((a.abs() as f64 * b as f64) * c as f64).floor() as i32
    } else {
        llfloor(a.abs() * b * c)
    }
}

/// `lerp(0.f, -sin(x) * 0.5f, t)` and `lerp(-0.5f, cos(x) * 0.5f, t)` of
/// the test path, stored to `F32`.
#[inline]
pub fn test_path_yz(x: f32, t: f32) -> (f32, f32) {
    if C_MATH_IS_DOUBLE {
        let xd = x as f64;
        (
            lerp_libstdcxx_f64(0.0, -xd.sin() * 0.5, t as f64) as f32,
            lerp_libstdcxx_f64(-0.5, xd.cos() * 0.5, t as f64) as f32,
        )
    } else {
        (lerp(0.0, -x.sin() * 0.5, t), lerp(-0.5, x.cos() * 0.5, t))
    }
}

/// libstdc++'s `std::__lerp`.
#[inline]
pub fn lerp_libstdcxx(a: f32, b: f32, t: f32) -> f32 {
    if (a <= 0.0 && b >= 0.0) || (a >= 0.0 && b <= 0.0) {
        return t * b + (1.0 - t) * a;
    }
    if t == 1.0 {
        return b; // exact
    }
    // Exact at t = 0, monotonic except near t = 1, bounded, determinate,
    // and consistent:
    let x = a + t * (b - a);
    if (t > 1.0) == (b > a) {
        if b < x { x } else { b }
    } else if b > x {
        x
    } else {
        b
    } // monotonic near t = 1
}

/// The MSVC STL's `_Common_lerp`.
#[inline]
pub fn lerp_msvc(a: f32, b: f32, t: f32) -> f32 {
    let t_is_finite = t.is_finite();
    if t_is_finite && a.is_finite() && b.is_finite() {
        if (a <= 0.0 && b >= 0.0) || (a >= 0.0 && b <= 0.0) {
            return t * b + (1.0 - t) * a;
        }
        if t == 1.0 {
            return b;
        }
        let candidate = t.mul_add(b - a, a);
        if (t > 1.0) == (b > a) {
            if b > candidate {
                return b;
            }
        } else if candidate > b {
            return b;
        }
        return candidate;
    }
    if a.is_nan() || b.is_nan() {
        return (a + b) + t;
    }
    if t.is_nan() {
        return t + t;
    }
    if t_is_finite {
        if t < 0.0 {
            a - b
        } else if t <= 1.0 {
            t * b + (1.0 - t) * a
        } else {
            b - a
        }
    } else {
        t * (b - a)
    }
}

/// `llfloor`: `(S32)floor(f)`.
#[inline]
pub fn llfloor(f: f32) -> i32 {
    f.floor() as i32
}

/// `llceil`: `(S32)ceil(f)`.
#[inline]
pub fn llceil(f: f32) -> i32 {
    f.ceil() as i32
}

/// `ll_round`: `llfloor(val + 0.5f)`.
#[inline]
pub fn ll_round(f: f32) -> i32 {
    llfloor(f + 0.5)
}
