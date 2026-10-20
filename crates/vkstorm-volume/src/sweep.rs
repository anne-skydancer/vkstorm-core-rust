//! The mesh sweep of `LLVolume::generate`: every profile point transformed
//! by every path point's scale and rotation, offset by its position.

use crate::path::PathPt;
use crate::simd::{V4, rotate};

/// Fills `dst` (`path.len() * profile.len()` points, path-major) exactly as
/// `LLVolume::generate` does: `scale * rot` as a 4x4 product, then
/// `LLMatrix4a::rotate` and the offset, with a non-finite offset cleared.
pub fn sweep(path: &[PathPt], profile: &[V4], dst: &mut [V4]) {
    let size_t = profile.len();
    assert!(dst.len() >= path.len() * size_t, "mesh buffer too small");

    for (s, pt) in path.iter().enumerate() {
        let sc = [
            [pt.scale[0], 0.0, 0.0, 0.0],
            [0.0, pt.scale[1], 0.0, 0.0],
            [0.0, 0.0, pt.scale[2], 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let rot = &pt.rot;
        let mut mat = [[0.0f32; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                mat[j][i] = sc[j][0] * rot[0][i]
                    + sc[j][1] * rot[1][i]
                    + sc[j][2] * rot[2][i]
                    + sc[j][3] * rot[3][i];
            }
        }
        let rows = [V4(mat[0]), V4(mat[1]), V4(mat[2]), V4(mat[3])];

        let mut offset = V4(pt.pos);
        // Work around MAINT-5660 as the C++ does: a path with NaNs in it is
        // treated as centred.
        if !offset.is_finite3() {
            offset = V4::ZERO;
        }

        let row = &mut dst[s * size_t..(s + 1) * size_t];
        for (d, p) in row.iter_mut().zip(profile) {
            *d = rotate(&rows, *p).add(offset);
        }
    }
}
