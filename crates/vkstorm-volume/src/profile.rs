//! `LLProfile`: the cross-section swept along the path. A port of
//! `LLProfile::generate`, `genNGon`, `addHole`, `addCap`, `addFace` and the
//! point-count mirrors, in the C++ operation order.

use crate::params::*;
use crate::simd::V4;

/// `LLProfile::Face`, same layout as the C++ struct (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProfileFace {
    pub index: i32,
    pub count: i32,
    pub scale_u: f32,
    pub cap: u8,
    pub flat: u8,
    pub face_id: u16,
}

/// The generated profile: points, faces and the flags `LLProfile` keeps.
#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub points: Vec<V4>,
    pub faces: Vec<ProfileFace>,
    pub open: bool,
    pub concave: bool,
    pub total: i32,
    pub total_out: i32,
}

impl Profile {
    fn add_cap(&mut self, face_id: u16) {
        self.faces.push(ProfileFace {
            index: 0,
            count: self.total,
            scale_u: 1.0,
            cap: 1,
            flat: 0,
            face_id,
        });
    }

    fn add_face(&mut self, index: i32, count: i32, scale_u: f32, face_id: u16, flat: bool) {
        self.faces.push(ProfileFace {
            index,
            count,
            scale_u,
            cap: 0,
            flat: flat as u8,
            face_id,
        });
    }

    /// `LLProfile::genNGon`: an n-sided "circular" path from (1, 0)
    /// counter-clockwise, cut at `begin`/`end`, optionally split.
    fn gen_ngon(
        &mut self,
        params: &ProfileParams,
        sides: i32,
        offset: f32,
        ang_scale: f32,
        split: i32,
    ) {
        const TABLE_SCALE: [f32; 8] = [1.0, 1.0, 1.0, 0.5, 0.707107, 0.53, 0.525, 0.5];
        let mut scale = 0.5f32;

        let begin = params.begin;
        let end = params.end;

        let t_step = 1.0f32 / sides as f32;
        let ang_step = 2.0 * F_PI * t_step * ang_scale;

        // Scale to have size "match" scale: compensates to get the object to
        // generally fill the bounding box.
        let total_sides = ll_round(sides as f32 / ang_scale);
        if total_sides < 8 {
            scale = TABLE_SCALE[total_sides.max(0) as usize];
        }

        let t_first = (begin * sides as f32).floor() / sides as f32;

        // pt1 is the first point on the fractional face.
        let mut t = t_first;
        let mut ang = 2.0 * F_PI * (t * ang_scale + offset);
        let mut pt1 = V4::set3(ang.cos() * scale, ang.sin() * scale, t);

        // pt2 is the end point on the fractional face.
        t += t_step;
        ang += ang_step;
        let mut pt2 = V4::set3(ang.cos() * scale, ang.sin() * scale, t);

        let mut t_fraction = (begin - t_first) * sides as f32;

        // Only use if it's not almost exactly on an edge.
        if t_fraction < 0.9999 {
            self.points.push(V4::lerp(pt1, pt2, t_fraction));
        }

        while t < end {
            // Iterate through all the integer steps of t.
            pt1 = V4::set3(ang.cos() * scale, ang.sin() * scale, t);

            if let Some(&p) = self.points.last() {
                for i in 0..split {
                    let s = 1.0f32 / (split + 1) as f32 * (i + 1) as f32;
                    self.points.push(pt1.sub(p).mul_s(s).add(p));
                }
            }
            self.points.push(pt1);

            t += t_step;
            ang += ang_step;
        }

        // pt1 is the first point on the fractional face, pt2 the end point.
        pt2 = V4::set3(ang.cos() * scale, ang.sin() * scale, t);

        // Find the fraction that we need to add to the end point.
        t_fraction = (end - (t - t_step)) * sides as f32;
        if t_fraction > 0.0001 {
            let new_pt = V4::lerp(pt1, pt2, t_fraction);

            if let Some(&p) = self.points.last() {
                for i in 0..split {
                    let s = 1.0f32 / (split + 1) as f32 * (i + 1) as f32;
                    self.points.push(new_pt.sub(p).mul_s(s).add(p));
                }
            }
            self.points.push(new_pt);
        }

        // If we're sliced, the profile is open.
        if (end - begin) * ang_scale < 0.99 {
            self.concave = (end - begin) * ang_scale > 0.5;
            self.open = true;
            if params.hollow <= 0.0 {
                // Put a center point if not hollow.
                self.points.push(V4::ZERO);
            }
        } else {
            self.open = false;
            self.concave = false;
        }

        self.total = self.points.len() as i32;
    }

    /// `LLProfile::addHole`: the inner profile, scaled by `box_hollow` and
    /// reversed so that it winds the other way.
    fn add_hole(
        &mut self,
        params: &ProfileParams,
        flat: bool,
        sides: f32,
        offset: f32,
        box_hollow: f32,
        ang_scale: f32,
        split: i32,
    ) {
        self.total_out = self.total;

        self.gen_ngon(params, llfloor(sides), offset, ang_scale, split);

        self.add_face(
            self.total_out,
            self.total - self.total_out,
            0.0,
            FACE_INNER_SIDE,
            flat,
        );

        let (outer, inner) = self.points.split_at_mut(self.total_out as usize);
        let _ = outer;
        let n = inner.len();
        let mut scaled: Vec<V4> = inner.iter().map(|p| p.mul_s(box_hollow)).collect();
        for i in 0..n {
            inner[i] = scaled[n - 1 - i];
        }
        scaled.clear();

        for f in &mut self.faces {
            if f.cap != 0 {
                f.count *= 2;
            }
        }
    }
}

/// `LLProfile::generate`. Returns `false` (with an empty profile) when the
/// cut is invalid (`begin > end - 0.01`), as the C++ does.
pub fn generate_profile(
    params: &ProfileParams,
    path_open: bool,
    detail: f32,
    split: i32,
    is_sculpted: bool,
    sculpt_size: i32,
    out: &mut Profile,
) -> bool {
    let detail = if detail < MIN_LOD { MIN_LOD } else { detail };

    out.points.clear();
    out.faces.clear();

    let begin = params.begin;
    let end = params.end;
    let hollow = params.hollow;

    // Quick validation to eliminate some server crashes.
    if begin > end - 0.01 {
        return false;
    }

    let mut face_num = 0i32;

    match params.curve & PROFILE_MASK {
        PROFILE_SQUARE => {
            out.gen_ngon(params, 4, -0.375, 1.0, split);
            if path_open {
                out.add_cap(FACE_PATH_BEGIN);
            }

            let mut i = llfloor(begin * 4.0);
            while i < llfloor(end * 4.0 + 0.999) {
                out.add_face(
                    face_num * (split + 1),
                    split + 2,
                    1.0,
                    FACE_OUTER_SIDE_0 << i,
                    true,
                );
                face_num += 1;
                i += 1;
            }

            // Scale by 4 to generate proper tex coords.
            let scale = V4::new(1.0, 1.0, 4.0, 1.0);
            for p in &mut out.points {
                *p = p.mul(scale);
            }

            if hollow != 0.0 {
                match params.curve & HOLE_MASK {
                    HOLE_TRIANGLE => out.add_hole(params, true, 3.0, -0.375, hollow, 1.0, split),
                    HOLE_CIRCLE => out.add_hole(
                        params,
                        false,
                        MIN_DETAIL_FACES as f32 * detail,
                        -0.375,
                        hollow,
                        1.0,
                        0,
                    ),
                    _ => out.add_hole(params, true, 4.0, -0.375, hollow, 1.0, split),
                }
            }

            if path_open {
                out.faces[0].count = out.total;
            }
        }
        PROFILE_ISOTRI | PROFILE_RIGHTTRI | PROFILE_EQUALTRI => {
            out.gen_ngon(params, 3, 0.0, 1.0, split);
            // Scale by 3 to generate proper tex coords.
            let scale = V4::new(1.0, 1.0, 3.0, 1.0);
            for p in &mut out.points {
                *p = p.mul(scale);
            }

            if path_open {
                out.add_cap(FACE_PATH_BEGIN);
            }

            let mut i = llfloor(begin * 3.0);
            while i < llfloor(end * 3.0 + 0.999) {
                out.add_face(
                    face_num * (split + 1),
                    split + 2,
                    1.0,
                    FACE_OUTER_SIDE_0 << i,
                    true,
                );
                face_num += 1;
                i += 1;
            }
            if hollow != 0.0 {
                // Swept triangles need smaller hollowness values, because the
                // triangle doesn't fill the bounding box.
                let triangle_hollow = hollow / 2.0;
                match params.curve & HOLE_MASK {
                    HOLE_CIRCLE => out.add_hole(
                        params,
                        false,
                        MIN_DETAIL_FACES as f32 * detail,
                        0.0,
                        triangle_hollow,
                        1.0,
                        0,
                    ),
                    HOLE_SQUARE => {
                        out.add_hole(params, true, 4.0, 0.0, triangle_hollow, 1.0, split)
                    }
                    _ => out.add_hole(params, true, 3.0, 0.0, triangle_hollow, 1.0, split),
                }
            }
        }
        PROFILE_CIRCLE => {
            // If this has a square hollow, adjust the number of faces a bit
            // so that the geometry lines up.
            let mut hole_type = 0u8;
            let mut circle_detail = MIN_DETAIL_FACES as f32 * detail;
            if hollow != 0.0 {
                hole_type = params.curve & HOLE_MASK;
                if hole_type == HOLE_SQUARE {
                    // Snap to the next multiple of four sides.
                    circle_detail = llceil(circle_detail / 4.0) as f32 * 4.0;
                }
            }

            let mut sides = circle_detail as i32;
            if is_sculpted {
                sides = sculpt_size;
            }

            out.gen_ngon(params, sides, 0.0, 1.0, 0);

            if path_open {
                out.add_cap(FACE_PATH_BEGIN);
            }

            if out.open && hollow == 0.0 {
                out.add_face(0, out.total - 1, 0.0, FACE_OUTER_SIDE_0, false);
            } else {
                out.add_face(0, out.total, 0.0, FACE_OUTER_SIDE_0, false);
            }

            if hollow != 0.0 {
                match hole_type {
                    HOLE_SQUARE => out.add_hole(params, true, 4.0, 0.0, hollow, 1.0, split),
                    HOLE_TRIANGLE => out.add_hole(params, true, 3.0, 0.0, hollow, 1.0, split),
                    _ => out.add_hole(params, false, circle_detail, 0.0, hollow, 1.0, 0),
                }
            }
        }
        PROFILE_CIRCLE_HALF => {
            let mut hole_type = 0u8;
            // Number of faces is cut in half because it's only a half-circle.
            let mut circle_detail = MIN_DETAIL_FACES as f32 * detail * 0.5;
            if hollow != 0.0 {
                hole_type = params.curve & HOLE_MASK;
                if hole_type == HOLE_SQUARE {
                    // Snap to the next multiple of four sides (div 2).
                    circle_detail = llceil(circle_detail / 2.0) as f32 * 2.0;
                }
            }
            out.gen_ngon(params, llfloor(circle_detail), 0.5, 0.5, 0);
            if path_open {
                out.add_cap(FACE_PATH_BEGIN);
            }
            if out.open && params.hollow == 0.0 {
                out.add_face(0, out.total - 1, 0.0, FACE_OUTER_SIDE_0, false);
            } else {
                out.add_face(0, out.total, 0.0, FACE_OUTER_SIDE_0, false);
            }

            if hollow != 0.0 {
                match hole_type {
                    HOLE_SQUARE => out.add_hole(params, true, 2.0, 0.5, hollow, 0.5, split),
                    HOLE_TRIANGLE => out.add_hole(params, true, 3.0, 0.5, hollow, 0.5, split),
                    _ => out.add_hole(params, false, circle_detail, 0.5, hollow, 0.5, 0),
                }
            }

            // Special case for openness of sphere.
            if params.end - params.begin < 1.0 {
                out.open = true;
            } else if hollow == 0.0 {
                out.open = false;
                let first = out.points[0];
                out.points.push(first);
                out.total += 1;
            }
        }
        _ => {}
    }

    if path_open {
        out.add_cap(FACE_PATH_END); // bottom
    }

    if out.open {
        // interior edge caps
        out.add_face(out.total - 1, 2, 0.5, FACE_PROFILE_BEGIN, true);

        if hollow != 0.0 {
            out.add_face(out.total_out - 1, 2, 0.5, FACE_PROFILE_END, true);
        } else {
            out.add_face(out.total - 2, 2, 0.5, FACE_PROFILE_END, true);
        }
    }

    true
}

/// `LLProfile::getNumNGonPoints`: `genNGon` stripped down to what decides
/// the number of points.
fn num_ngon_points(params: &ProfileParams, sides: i32, ang_scale: f32) -> i32 {
    let mut np = 0;
    let begin = params.begin;
    let end = params.end;

    let t_step = 1.0f32 / sides as f32;
    let t_first = (begin * sides as f32).floor() / sides as f32;

    let mut t = t_first;
    t += t_step;

    let t_fraction = (begin - t_first) * sides as f32;
    if t_fraction < 0.9999 {
        np += 1;
    }

    while t < end {
        np += 1;
        t += t_step;
    }

    let t_fraction = (end - (t - t_step)) * sides as f32;
    if t_fraction > 0.0001 {
        np += 1;
    }

    if (end - begin) * ang_scale < 0.99 && params.hollow <= 0.0 {
        np += 1;
    }

    np
}

/// `LLProfile::getNumPoints`: the point count estimate used for LOD
/// triangle counts.
pub fn profile_num_points(
    params: &ProfileParams,
    _path_open: bool,
    detail: f32,
    _split: i32,
    is_sculpted: bool,
    sculpt_size: i32,
) -> i32 {
    let detail = if detail < MIN_LOD { MIN_LOD } else { detail };
    let hollow = params.hollow;
    let mut np;

    match params.curve & PROFILE_MASK {
        PROFILE_SQUARE => {
            np = num_ngon_points(params, 4, 1.0);
            if hollow != 0.0 {
                np *= 2;
            }
        }
        PROFILE_ISOTRI | PROFILE_RIGHTTRI | PROFILE_EQUALTRI => {
            np = num_ngon_points(params, 3, 1.0);
            if hollow != 0.0 {
                np *= 2;
            }
        }
        PROFILE_CIRCLE => {
            let mut circle_detail = MIN_DETAIL_FACES as f32 * detail;
            if hollow != 0.0 && params.curve & HOLE_MASK == HOLE_SQUARE {
                circle_detail = llceil(circle_detail / 4.0) as f32 * 4.0;
            }
            let mut sides = circle_detail as i32;
            if is_sculpted {
                sides = sculpt_size;
            }
            np = num_ngon_points(params, sides, 1.0);
            if hollow != 0.0 {
                np *= 2;
            }
        }
        PROFILE_CIRCLE_HALF => {
            let mut circle_detail = MIN_DETAIL_FACES as f32 * detail * 0.5;
            if hollow != 0.0 && params.curve & HOLE_MASK == HOLE_SQUARE {
                circle_detail = llceil(circle_detail / 2.0) as f32 * 2.0;
            }
            np = num_ngon_points(params, llfloor(circle_detail), 0.5);
            if hollow != 0.0 {
                np *= 2;
            }
            // Special case for openness of sphere.
            if params.end - params.begin < 1.0 {
            } else if hollow == 0.0 {
                np += 1;
            }
        }
        _ => np = 0,
    }

    np
}
