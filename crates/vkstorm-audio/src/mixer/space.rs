//! Derived from SoLoud (Copyright (c) 2013-2020 Jari Komppa), zlib licence;
//! altered: rewritten in Rust. See LICENSE-soloud in this crate.
//!
//! SoLoud's 3D math (`soloud_core_3d.cpp`): vectors, the listener basis,
//! distance attenuation, Doppler and the per-speaker panning, with the C's
//! float operation order (`(x*x + y*y) + z*z`, `sqrt` in double rounded to
//! float, which equals `sqrtf`).

#![forbid(unsafe_code)]

pub(crate) type Vec3 = [f32; 3];

/// SoLoud's `MIN(a,b)`: `(a < b) ? a : b` (NaN picks `b`).
#[inline(always)]
pub(crate) fn c_min(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

/// SoLoud's `MAX(a,b)`: `(a > b) ? a : b`.
#[inline(always)]
pub(crate) fn c_max(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

#[inline(always)]
pub(crate) fn is_null(v: &Vec3) -> bool {
    v[0] == 0.0 && v[1] == 0.0 && v[2] == 0.0
}

#[inline(always)]
pub(crate) fn mag(v: &Vec3) -> f32 {
    // (float)sqrt(double): correctly rounded either way.
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

#[inline(always)]
pub(crate) fn normalize(v: &mut Vec3) {
    let m = mag(v);
    if m == 0.0 {
        *v = [0.0; 3];
        return;
    }
    v[0] /= m;
    v[1] /= m;
    v[2] /= m;
}

#[inline(always)]
pub(crate) fn dot(a: &Vec3, b: &Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline(always)]
pub(crate) fn sub(a: &Vec3, b: &Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline(always)]
fn cross(a: &Vec3, b: &Vec3) -> Vec3 {
    [
        a[1] * b[2] - b[1] * a[2],
        a[2] * b[0] - b[2] * a[0],
        a[0] * b[1] - b[0] * a[1],
    ]
}

/// The listener basis (`mat3::lookatRH`; the viewer never sets
/// `LEFT_HANDED_3D`).
pub(crate) struct Mat3([Vec3; 3]);

impl Mat3 {
    pub(crate) fn lookat_rh(at: &Vec3, up: &Vec3) -> Mat3 {
        let mut z = *at;
        normalize(&mut z);
        let mut x = cross(up, &z);
        normalize(&mut x);
        let y = cross(&z, &x);
        Mat3([x, y, z])
    }

    #[inline(always)]
    pub(crate) fn mul(&self, a: &Vec3) -> Vec3 {
        let m = &self.0;
        [
            m[0][0] * a[0] + m[0][1] * a[1] + m[0][2] * a[2],
            m[1][0] * a[0] + m[1][1] * a[1] + m[1][2] * a[2],
            m[2][0] * a[0] + m[2][1] * a[1] + m[2][2] * a[2],
        ]
    }
}

pub(crate) fn doppler(
    delta_pos: &Vec3,
    src_vel: &Vec3,
    dst_vel: &Vec3,
    factor: f32,
    sound_speed: f32,
) -> f32 {
    let deltamag = mag(delta_pos);
    if deltamag == 0.0 {
        return 1.0;
    }
    let mut vls = dot(delta_pos, dst_vel) / deltamag;
    let mut vss = dot(delta_pos, src_vel) / deltamag;
    let maxspeed = sound_speed / factor;
    vss = c_min(vss, maxspeed);
    vls = c_min(vls, maxspeed);
    (sound_speed - factor * vls) / (sound_speed - factor * vss)
}

pub(crate) fn attenuate_inv_distance(distance: f32, min: f32, max: f32, rolloff: f32) -> f32 {
    let mut d = c_max(distance, min);
    d = c_min(d, max);
    min / (min + rolloff * (d - min))
}

pub(crate) fn attenuate_linear_distance(distance: f32, min: f32, max: f32, rolloff: f32) -> f32 {
    let mut d = c_max(distance, min);
    d = c_min(d, max);
    1.0 - rolloff * (d - min) / (max - min)
}

/// `(float)pow(distance / min, -rolloff)`. Not used by the viewer (it only
/// selects inverse distance); `powf` may differ from the C's overload
/// resolution in the last bit, so this model is not part of the parity
/// qualification.
pub(crate) fn attenuate_exponential_distance(
    distance: f32,
    min: f32,
    max: f32,
    rolloff: f32,
) -> f32 {
    let mut d = c_max(distance, min);
    d = c_min(d, max);
    (d / min).powf(-rolloff)
}
