//! `LLPath`: the sweep path. A port of `LLPath::generate`, `genNGon` and the
//! point-count mirror, with the `LLQuaternion`/`LLMatrix3` arithmetic the
//! C++ goes through for each point's rotation.

use crate::params::*;

/// `LLPath::PathPt`, same layout as the C++ (112 bytes, 16-byte aligned):
/// the rotation as an `LLMatrix4a` (row 3 is `(0, 0, 0, 1)`), position,
/// scale, texture T and the debugging pad.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathPt {
    pub rot: [[f32; 4]; 4],
    pub pos: [f32; 4],
    pub scale: [f32; 4],
    pub tex_t: f32,
    pub pad: [f32; 3],
}

impl Default for PathPt {
    fn default() -> Self {
        PathPt {
            rot: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            pos: [0.0; 4],
            scale: [0.0; 4],
            tex_t: 0.0,
            pad: [3.14159, -3.14159, 0.585],
        }
    }
}

/// The generated path and the flags `LLPath` keeps.
#[derive(Clone, Debug)]
pub struct Path {
    pub points: Vec<PathPt>,
    pub open: bool,
    pub step: f32,
}

impl Default for Path {
    fn default() -> Self {
        Path {
            points: Vec::new(),
            open: false,
            step: 1.0,
        }
    }
}

/// `LLQuaternion::setQuat(angle, x, y, z)`.
#[inline]
fn set_quat(angle: f32, x: f32, y: f32, z: f32) -> [f32; 4] {
    let mag = (x * x + y * y + z * z).sqrt();
    if mag > FP_MAG_THRESHOLD {
        let angle = angle * 0.5;
        let c = angle.cos();
        let s = angle.sin() / mag;
        [x * s, y * s, z * s, c]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// `operator*(const LLQuaternion& a, const LLQuaternion& b)`.
#[inline]
fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        b[3] * a[0] + b[0] * a[3] + b[1] * a[2] - b[2] * a[1],
        b[3] * a[1] + b[1] * a[3] + b[2] * a[0] - b[0] * a[2],
        b[3] * a[2] + b[2] * a[3] + b[0] * a[1] - b[1] * a[0],
        b[3] * a[3] - b[0] * a[0] - b[1] * a[1] - b[2] * a[2],
    ]
}

/// `LLQuaternion::getMatrix3` loaded into an `LLMatrix4a` with
/// `loadu(const LLMatrix3&)`: rows with w = 0 and `(0, 0, 0, 1)` last.
#[inline]
fn quat_rot(q: [f32; 4]) -> [[f32; 4]; 4] {
    let xx = q[0] * q[0];
    let xy = q[0] * q[1];
    let xz = q[0] * q[2];
    let xw = q[0] * q[3];
    let yy = q[1] * q[1];
    let yz = q[1] * q[2];
    let yw = q[1] * q[3];
    let zz = q[2] * q[2];
    let zw = q[2] * q[3];
    [
        [1.0 - 2.0 * (yy + zz), 2.0 * (xy + zw), 2.0 * (xz - yw), 0.0],
        [2.0 * (xy - zw), 1.0 - 2.0 * (xx + zz), 2.0 * (yz + xw), 0.0],
        [2.0 * (xz + yw), 2.0 * (yz - xw), 1.0 - 2.0 * (xx + yy), 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// `LLPath::genNGon`: a circular path from (1, 0, 0) counter-clockwise in
/// the xz plane, with radius offset, taper, skew, shear and twist.
fn gen_ngon(out: &mut Path, params: &PathParams, sides: i32, end_scale: f32, twist_scale: f32) {
    const TABLE_SCALE: [f32; 8] = [1.0, 1.0, 1.0, 0.5, 0.707107, 0.53, 0.525, 0.5];

    let revolutions = params.revolutions;
    let skew = params.skew;
    let skew_mag = skew.abs();
    let hole_x = params.scale_x * (1.0 - skew_mag);
    let hole_y = params.scale_y;

    // Calculate taper begin/end for x,y (negative means taper the beginning).
    let mut taper_x_begin = 1.0f32;
    let mut taper_x_end = 1.0 - params.taper_x;
    let mut taper_y_begin = 1.0f32;
    let mut taper_y_end = 1.0 - params.taper_y;

    if taper_x_end > 1.0 {
        // Flip tapering.
        taper_x_begin = 2.0 - taper_x_end;
        taper_x_end = 1.0;
    }
    if taper_y_end > 1.0 {
        taper_y_begin = 2.0 - taper_y_end;
        taper_y_end = 1.0;
    }

    // For spheres, the radius is usually zero.
    let mut radius_start = 0.5f32;
    if sides < 8 {
        radius_start = TABLE_SCALE[sides.max(0) as usize];
    }

    // Scale the radius to take the hole size into account.
    radius_start *= 1.0 - hole_y;

    // Radius offset: negative decreases the start radius instead.
    let mut radius_end = radius_start;
    let radius_offset = params.radius_offset;
    if radius_offset < 0.0 {
        radius_start *= 1.0 + radius_offset;
    } else {
        radius_end *= 1.0 - radius_offset;
    }

    // Is the path NOT a closed loop?
    out.open = (params.end * end_scale - params.begin < 1.0)
        || (skew_mag > 0.001)
        || ((taper_x_end - taper_x_begin).abs() > 0.001)
        || ((taper_y_end - taper_y_begin).abs() > 0.001)
        || ((radius_end - radius_start).abs() > 0.001);

    let twist_begin = params.twist_begin * twist_scale;
    let twist_end = params.twist_end * twist_scale;

    let step = 1.0f32 / sides as f32;

    let shear_x = params.shear_x;
    let shear_y = params.shear_y;

    let make_point = |t: f32| -> PathPt {
        let ang = 2.0 * F_PI * revolutions * t;
        let s = sin_mul(ang, lerp(radius_start, radius_end, t));
        let c = cos_mul(ang, lerp(radius_start, radius_end, t));

        let pos = [
            0.0 + lerp(0.0, shear_x, s) + lerp(-skew, skew, t) * 0.5,
            c + lerp(0.0, shear_y, s),
            s,
            0.0,
        ];
        let scale = [
            hole_x * lerp(taper_x_begin, taper_x_end, t),
            hole_y * lerp(taper_y_begin, taper_y_end, t),
            0.0,
            1.0,
        ];

        // Twist rotates the path along the x,y plane; then rotate the point
        // around the circle's center.
        let twist = set_quat(
            lerp(twist_begin, twist_end, t) * 2.0 * F_PI - F_PI,
            0.0,
            0.0,
            1.0,
        );
        let qang = set_quat(ang, 1.0, 0.0, 0.0);
        let rot = quat_rot(quat_mul(twist, qang));

        PathPt {
            rot,
            pos,
            scale,
            tex_t: t,
            pad: [0.0; 3],
        }
    };

    // Once before the main loop, so the path begins at the correct cut.
    let mut t = params.begin;
    out.points.push(make_point(t));

    t += step;

    // Snap to a quantized parameter, so that cut does not affect most
    // sample points.
    t = ((t * sides as f32) as i32) as f32 / sides as f32;

    // Run through the non-cut dependent points.
    while t < params.end {
        out.points.push(make_point(t));
        t += step;
    }

    // Make one final pass for the end cut.
    t = params.end;
    out.points.push(make_point(t));
}

/// `LLPath::generate`.
pub fn generate_path(
    params: &PathParams,
    detail: f32,
    split: i32,
    is_sculpted: bool,
    sculpt_size: i32,
    out: &mut Path,
) {
    let detail = if detail < MIN_LOD { MIN_LOD } else { detail };

    out.points.clear();
    out.open = true;

    match params.curve & 0xf0 {
        PATH_CIRCLE => {
            // Increase the detail as the revolutions and twist increase.
            let twist_mag = (params.twist_begin - params.twist_end).abs();
            let mut sides = llfloor(
                llfloor(MIN_DETAIL_FACES as f32 * detail + twist_mag * 3.5 * (detail - 0.5)) as f32
                    * params.revolutions,
            );
            if is_sculpted {
                sides = sculpt_size.max(1);
            }
            if 0 < sides {
                gen_ngon(out, params, sides, 1.0, 1.0);
            }
        }
        PATH_CIRCLE2 => {
            if params.end - params.begin >= 0.99 && params.scale_x >= 0.99 {
                out.open = false;
            }
            gen_ngon(
                out,
                params,
                llfloor(MIN_DETAIL_FACES as f32 * detail),
                1.0,
                1.0,
            );

            let mut toggle = 0.5f32;
            for pt in &mut out.points {
                pt.pos[0] = toggle;
                toggle = if toggle == 0.5 { -0.5 } else { 0.5 };
            }
        }
        PATH_TEST => {
            let np = 5;
            out.step = 1.0 / (np - 1) as f32;
            let twist = params.twist_end;
            for i in 0..np {
                let t = i as f32 * out.step;
                let (y, z) = test_path_yz(F_PI * twist * t, t);
                let pos = [0.0, y, z, 0.0];
                let scale = [
                    lerp(1.0, params.scale_x, t),
                    lerp(1.0, params.scale_y, t),
                    0.0,
                    1.0,
                ];
                let rot = quat_rot(set_quat(F_PI * twist * t, 1.0, 0.0, 0.0));
                out.points.push(PathPt {
                    rot,
                    pos,
                    scale,
                    tex_t: t,
                    pad: [0.0; 3],
                });
            }
        }
        _ => {
            // LL_PCODE_PATH_LINE and anything unknown.
            // Take the begin/end twist into account for detail.
            let mut np =
                llfloor_fabs_mul_mul(params.twist_begin - params.twist_end, 3.5, detail - 0.5) + 2;
            if np < split + 2 {
                np = split + 2;
            }

            out.step = 1.0 / (np - 1) as f32;

            let start_scale = params.begin_scale();
            let end_scale = params.end_scale();

            for i in 0..np {
                let t = lerp(params.begin, params.end, i as f32 * out.step);
                let pos = [
                    lerp(0.0, params.shear_x, t),
                    lerp(0.0, params.shear_y, t),
                    t - 0.5,
                    0.0,
                ];
                let rot = quat_rot(set_quat(
                    lerp(F_PI * params.twist_begin, F_PI * params.twist_end, t),
                    0.0,
                    0.0,
                    1.0,
                ));
                let scale = [
                    lerp(start_scale[0], end_scale[0], t),
                    lerp(start_scale[1], end_scale[1], t),
                    0.0,
                    1.0,
                ];
                out.points.push(PathPt {
                    rot,
                    pos,
                    scale,
                    tex_t: t,
                    pad: [0.0; 3],
                });
            }
        }
    }

    if params.twist_end != params.twist_begin {
        out.open = true;
    }
}

/// `LLPath::getNumNGonPoints`.
fn num_ngon_points(params: &PathParams, sides: i32) -> i32 {
    let step = 1.0f32 / sides as f32;
    let mut t = params.begin;
    let mut ret = 1;

    t += step;
    t = ((t * sides as f32) as i32) as f32 / sides as f32;

    while t < params.end {
        ret += 1;
        t += step;
    }

    ret + 1
}

/// `LLPath::getNumPoints`: the point count estimate used for LOD triangle
/// counts (for circles it is the side count, not the generated count).
pub fn path_num_points(params: &PathParams, detail: f32) -> i32 {
    let detail = if detail < MIN_LOD { MIN_LOD } else { detail };
    match params.curve & 0xf0 {
        PATH_CIRCLE => {
            let twist_mag = (params.twist_begin - params.twist_end).abs();
            llfloor(
                llfloor(MIN_DETAIL_FACES as f32 * detail + twist_mag * 3.5 * (detail - 0.5)) as f32
                    * params.revolutions,
            )
        }
        PATH_CIRCLE2 => num_ngon_points(params, llfloor(MIN_DETAIL_FACES as f32 * detail)),
        PATH_TEST => 5,
        _ => llfloor_fabs_mul_mul(params.twist_begin - params.twist_end, 3.5, detail - 0.5) + 2,
    }
}
