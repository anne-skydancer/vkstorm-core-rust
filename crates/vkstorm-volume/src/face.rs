//! `LLVolumeFace` building: `createSide`, `createCap` and
//! `createUnCutCubeCap`, writing straight into the caller's vertex, normal,
//! texture-coordinate and index buffers (the viewer's `LLVolumeFace` arrays),
//! plus the face descriptors `LLVolume::createVolumeFaces` derives from the
//! profile.

use crate::params::*;
use crate::path::PathPt;
use crate::profile::Profile;
use crate::simd::{V4, llmax, llmin, update_min_max, update_min_max2};

/// What `LLVolume::createVolumeFaces` sets on each `LLVolumeFace` before
/// building it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceDesc {
    pub id: i32,
    pub type_mask: u32,
    pub begin_s: i32,
    pub num_s: i32,
    pub begin_t: i32,
    pub num_t: i32,
}

/// The face descriptors for a generated profile and a path of `path_len`
/// points, as `LLVolume::createVolumeFaces` computes them.
pub fn face_descriptors(profile: &Profile, path_len: i32, hollow: f32) -> Vec<FaceDesc> {
    profile
        .faces
        .iter()
        .enumerate()
        .map(|(i, face)| {
            let mut type_mask = 0u32;
            let mut num_s = face.count;
            if hollow > 0.0 {
                type_mask |= HOLLOW_MASK;
            }
            if profile.open {
                type_mask |= OPEN_MASK;
            }
            if face.cap != 0 {
                type_mask |= CAP_MASK;
                if face.face_id == FACE_PATH_BEGIN {
                    type_mask |= TOP_MASK;
                } else {
                    type_mask |= BOTTOM_MASK;
                }
            } else if face.face_id & (FACE_PROFILE_BEGIN | FACE_PROFILE_END) != 0 {
                type_mask |= FLAT_MASK | END_MASK;
            } else {
                type_mask |= SIDE_MASK;
                if face.flat != 0 {
                    type_mask |= FLAT_MASK;
                }
                if face.face_id & FACE_INNER_SIDE != 0 {
                    type_mask |= INNER_MASK;
                    if face.flat != 0 && num_s > 2 {
                        // Flat inner faces have to copy vertex normals.
                        num_s *= 2;
                    }
                } else {
                    type_mask |= OUTER_MASK;
                }
            }
            FaceDesc {
                id: i as i32,
                type_mask,
                begin_s: face.index,
                num_s,
                begin_t: 0,
                num_t: path_len,
            }
        })
        .collect()
}

/// Everything a face build reads: the volume's mesh grid, profile and path,
/// the face descriptor and the volume parameters the builders consult.
#[derive(Clone, Copy)]
pub struct FaceParams<'a> {
    pub mesh: &'a [V4],
    pub profile: &'a [V4],
    /// `LLProfile::getTotal()`.
    pub profile_total: i32,
    pub profile_open: bool,
    pub path: &'a [PathPt],
    pub path_open: bool,
    pub face: FaceDesc,
    /// `LLProfileParams::getCurveType()`, hole bits included.
    pub profile_curve: u8,
    pub path_curve: u8,
    pub sculpt_type: u8,
    pub path_begin: f32,
    pub path_end: f32,
}

/// Which builder `LLVolumeFace::create` dispatches to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceKind {
    Side = 0,
    Cap = 1,
    UnCutCubeCap = 2,
}

/// The builder for a face, with `createCap`'s uncut-cube shortcut.
pub fn face_kind(p: &FaceParams) -> FaceKind {
    let mask = p.face.type_mask;
    if mask & CAP_MASK != 0 {
        if mask & HOLLOW_MASK == 0
            && mask & OPEN_MASK == 0
            && p.path_begin == 0.0
            && p.path_end == 1.0
            && p.profile_curve == PROFILE_SQUARE
            && p.path_curve == PATH_LINE
        {
            FaceKind::UnCutCubeCap
        } else {
            FaceKind::Cap
        }
    } else {
        FaceKind::Side
    }
}

/// The vertex and index counts a face build produces.
pub fn face_sizes(p: &FaceParams) -> (FaceKind, i32, i32) {
    let kind = face_kind(p);
    match kind {
        FaceKind::UnCutCubeCap => {
            let grid_size = (p.profile.len().wrapping_sub(1) / 4) as i32;
            (
                kind,
                (grid_size + 1) * (grid_size + 1),
                grid_size * grid_size * 6,
            )
        }
        FaceKind::Cap => {
            let n = p.profile.len() as i32;
            let (nv, ni) = (n, (n - 2) * 3);
            if p.face.type_mask & HOLLOW_MASK == 0 && p.face.type_mask & OPEN_MASK == 0 {
                (kind, nv + 1, ni + 3)
            } else {
                (kind, nv, ni)
            }
        }
        FaceKind::Side => {
            let (s, t) = (p.face.num_s, p.face.num_t);
            (kind, s * t, (s - 1) * (t - 1) * 6)
        }
    }
}

/// The caller's buffers. Their lengths are the C++ `mNumVertices` and
/// `mNumIndices` (the allocated counts, which a partial rebuild may leave
/// larger than this face's counts); `texcoords` includes the even-count
/// padding slot the C++ allocates.
pub struct FaceBuffers<'a> {
    pub positions: &'a mut [V4],
    pub normals: &'a mut [V4],
    pub texcoords: &'a mut [[f32; 2]],
    pub indices: &'a mut [u16],
    /// `!partial_build`: whether to (re)write the index buffer.
    pub write_indices: bool,
}

/// What a face build sets besides the buffers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FaceResult {
    pub extents_min: V4,
    pub extents_max: V4,
    pub center: V4,
    pub tc_min: [f32; 2],
    pub tc_max: [f32; 2],
    /// `createSide` sets the texture-coordinate extents; the caps leave them.
    pub tc_extents_set: bool,
}

/// `LLVolumeFace::create`: builds the face into `b`. Returns `false` where
/// the C++ would read outside its inputs (degenerate volumes).
pub fn build_face(p: &FaceParams, b: &mut FaceBuffers, out: &mut FaceResult) -> bool {
    match face_kind(p) {
        FaceKind::UnCutCubeCap => create_uncut_cube_cap(p, b, out),
        FaceKind::Cap => create_cap(p, b, out),
        FaceKind::Side => create_side(p, b, out),
    }
}

struct VertexData {
    pos: V4,
    tc: [f32; 2],
}

/// `LerpPlanarVertex`.
#[inline]
fn lerp_planar_vertex(
    v0: &VertexData,
    v1: &VertexData,
    v2: &VertexData,
    coef01: f32,
    coef02: f32,
) -> VertexData {
    let lhs = v1.pos.sub(v0.pos).mul_s(coef01);
    let mut rhs = v2.pos.sub(v0.pos).mul_s(coef02);
    rhs = rhs.add(lhs);
    rhs = rhs.add(v0.pos);
    let tc = [
        v0.tc[0] + ((v1.tc[0] - v0.tc[0]) * coef01) + ((v2.tc[0] - v0.tc[0]) * coef02),
        v0.tc[1] + ((v1.tc[1] - v0.tc[1]) * coef01) + ((v2.tc[1] - v0.tc[1]) * coef02),
    ];
    VertexData { pos: rhs, tc }
}

/// `LLVolumeFace::createUnCutCubeCap`.
fn create_uncut_cube_cap(p: &FaceParams, b: &mut FaceBuffers, out: &mut FaceResult) -> bool {
    let mesh = p.mesh;
    let profile = p.profile;
    let max_s = p.profile_total;
    let max_t = p.path.len() as i32;
    let top = p.face.type_mask & TOP_MASK != 0;

    if profile.is_empty() {
        return false;
    }
    let grid_size = (profile.len() - 1) / 4;

    let offset = if top {
        ((max_t - 1) * max_s) as usize
    } else {
        p.face.begin_s as usize
    };

    let mut corners: [VertexData; 4] = [
        VertexData {
            pos: V4::ZERO,
            tc: [0.0; 2],
        },
        VertexData {
            pos: V4::ZERO,
            tc: [0.0; 2],
        },
        VertexData {
            pos: V4::ZERO,
            tc: [0.0; 2],
        },
        VertexData {
            pos: V4::ZERO,
            tc: [0.0; 2],
        },
    ];
    for (t, c) in corners.iter_mut().enumerate() {
        let Some(&m) = mesh.get(offset + grid_size * t) else {
            return false;
        };
        let Some(pr) = profile.get(grid_size * t) else {
            return false;
        };
        c.pos = m;
        c.tc = [pr.0[0] + 0.5, 0.5 - pr.0[1]];
    }

    let lhs = corners[1].pos.sub(corners[0].pos);
    let rhs = corners[2].pos.sub(corners[1].pos);
    let mut base_normal = V4::cross3(lhs, rhs).normalize3fast();

    if !top {
        // A real multiply, not a sign flip: they differ for the NaN a
        // zero-area cap produces, and the C++ multiplies.
        base_normal = base_normal.mul_s(std::hint::black_box(-1.0));
    } else {
        // Swap the UVs on the U(X) axis for the top face.
        let swap = corners[0].tc;
        corners[0].tc = corners[3].tc;
        corners[3].tc = swap;
        let swap = corners[1].tc;
        corners[1].tc = corners[2].tc;
        corners[2].tc = swap;
    }

    let size = (grid_size + 1) * (grid_size + 1);
    if b.positions.len() < size || b.normals.len() < size || b.texcoords.len() < size {
        return false;
    }

    let mut min = V4::ZERO;
    let mut max = V4::ZERO;
    let mut k = 0usize;
    for gx in 0..=grid_size {
        for gy in 0..=grid_size {
            let v = lerp_planar_vertex(
                &corners[0],
                &corners[1],
                &corners[3],
                gx as f32 / grid_size as f32,
                gy as f32 / grid_size as f32,
            );
            b.positions[k] = v.pos;
            b.normals[k] = base_normal;
            b.texcoords[k] = v.tc;
            k += 1;

            if gx == 0 && gy == 0 {
                min = v.pos;
                max = min;
            } else {
                min = min.min(v.pos);
                max = max.max(v.pos);
            }
        }
    }

    out.extents_min = min;
    out.extents_max = max;
    out.center = min.add(max).mul_s(0.5);
    out.tc_extents_set = false;

    if b.write_indices {
        let ni = grid_size * grid_size * 6;
        if b.indices.len() < ni {
            return false;
        }
        let g1 = grid_size + 1;
        let idxs = [0, 1, g1 + 1, g1 + 1, g1, 0];
        let mut o = 0usize;
        for gx in 0..grid_size {
            for gy in 0..grid_size {
                if top {
                    for i in (0..6).rev() {
                        b.indices[o] = (gy * g1 + gx + idxs[i]) as u16;
                        o += 1;
                    }
                } else {
                    for i in 0..6 {
                        b.indices[o] = (gy * g1 + gx + idxs[i]) as u16;
                        o += 1;
                    }
                }
            }
        }
    }
    true
}

/// The signed-area test `createCap` uses to pick the next triangle of a
/// hollow cap: returns `use_tri1a2`.
#[inline]
fn hollow_cap_choice(profile: &[V4], pt1: usize, pt2: usize) -> bool {
    let p1 = profile[pt1].0;
    let p2 = profile[pt2].0;
    let pa = profile[pt1 + 1].0;
    let pb = profile[pt2 - 1].0;

    // Use area of triangle to determine backfacing.
    let area_1a2 = (p1[0] * pa[1] - pa[0] * p1[1])
        + (pa[0] * p2[1] - p2[0] * pa[1])
        + (p2[0] * p1[1] - p1[0] * p2[1]);
    let area_1ba = (p1[0] * pb[1] - pb[0] * p1[1])
        + (pb[0] * pa[1] - pa[0] * pb[1])
        + (pa[0] * p1[1] - p1[0] * pa[1]);
    let area_21b = (p2[0] * p1[1] - p1[0] * p2[1])
        + (p1[0] * pb[1] - pb[0] * p1[1])
        + (pb[0] * p2[1] - p2[0] * pb[1]);
    let area_2ab = (p2[0] * pa[1] - pa[0] * p2[1])
        + (pa[0] * pb[1] - pb[0] * pa[1])
        + (pb[0] * p2[1] - p2[0] * pb[1]);

    let mut tri_1a2 = true;
    let mut tri_21b = true;
    if area_1a2 < 0.0 {
        tri_1a2 = false;
    }
    if area_2ab < 0.0 {
        // Can't use, because it contains point b.
        tri_1a2 = false;
    }
    if area_21b < 0.0 {
        tri_21b = false;
    }
    if area_1ba < 0.0 {
        tri_21b = false;
    }

    if !tri_1a2 {
        false
    } else if !tri_21b {
        true
    } else {
        let d1 = profile[pt1].sub(profile[pt1 + 1]);
        let d2 = profile[pt2].sub(profile[pt2 - 1]);
        V4::dot3(d1, d1) < V4::dot3(d2, d2)
    }
}

/// `LLVolumeFace::createCap`.
fn create_cap(p: &FaceParams, b: &mut FaceBuffers, out: &mut FaceResult) -> bool {
    let mesh = p.mesh;
    let profile = p.profile;
    let mask = p.face.type_mask;
    let hollow = mask & HOLLOW_MASK != 0;
    let open = mask & OPEN_MASK != 0;
    let top = mask & TOP_MASK != 0;

    // All types of caps have the same number of vertices and indices.
    let mut num_vertices = profile.len();
    if num_vertices < 2 {
        return false;
    }
    let num_indices = (num_vertices - 2) * 3;
    let (need_v, need_i) = if !hollow && !open {
        (num_vertices + 1, num_indices + 3)
    } else {
        (num_vertices, num_indices)
    };
    if b.positions.len() < need_v
        || b.normals.len() < need_v
        || b.texcoords.len() < need_v
        || b.indices.len() < need_i
    {
        return false;
    }

    let max_s = p.profile_total;
    let max_t = p.path.len() as i32;

    let offset = if top {
        ((max_t - 1) * max_s) as usize
    } else {
        p.face.begin_s as usize
    };
    if offset + num_vertices > mesh.len() {
        return false;
    }
    let src = &mesh[offset..offset + num_vertices];

    let mut min = src[0];
    let mut max = min;
    let mut min_uv;
    let mut max_uv;

    if top {
        min_uv = [profile[0].0[0] + 0.5, profile[0].0[1] + 0.5];
        max_uv = min_uv;
        for i in 0..num_vertices {
            let tc = [profile[i].0[0] + 0.5, profile[i].0[1] + 0.5];
            update_min_max(&mut min, &mut max, src[i]);
            update_min_max2(&mut min_uv, &mut max_uv, tc);
            b.positions[i] = src[i];
            b.texcoords[i] = tc;
        }
    } else {
        min_uv = [profile[0].0[0] + 0.5, 0.5 - profile[0].0[1]];
        max_uv = min_uv;
        for i in 0..num_vertices {
            // Mirror for underside.
            let tc = [profile[i].0[0] + 0.5, 0.5 - profile[i].0[1]];
            update_min_max(&mut min, &mut max, src[i]);
            update_min_max2(&mut min_uv, &mut max_uv, tc);
            b.positions[i] = src[i];
            b.texcoords[i] = tc;
        }
    }

    let center = min.add(max).mul_s(0.5);
    let cuv = [(min_uv[0] + max_uv[0]) * 0.5, (min_uv[1] + max_uv[1]) * 0.5];

    if !hollow && !open {
        b.positions[num_vertices] = center;
        b.texcoords[num_vertices] = cuv;
        num_vertices += 1;
    }

    out.extents_min = min;
    out.extents_max = max;
    out.center = center;
    out.tc_extents_set = false;

    let idx = &mut *b.indices;
    if hollow {
        let mut pt1 = 0usize;
        let mut pt2 = num_vertices - 1;
        let mut i = 0usize;
        if top {
            // HOLLOW TOP
            while pt2 - pt1 > 1 {
                if hollow_cap_choice(profile, pt1, pt2) {
                    idx[i] = pt1 as u16;
                    idx[i + 1] = (pt1 + 1) as u16;
                    idx[i + 2] = pt2 as u16;
                    pt1 += 1;
                } else {
                    idx[i] = pt1 as u16;
                    idx[i + 1] = (pt2 - 1) as u16;
                    idx[i + 2] = pt2 as u16;
                    pt2 -= 1;
                }
                i += 3;
            }
        } else {
            // HOLLOW BOTTOM: flipped backfacing from top.
            while pt2 - pt1 > 1 {
                if hollow_cap_choice(profile, pt1, pt2) {
                    idx[i] = pt1 as u16;
                    idx[i + 1] = pt2 as u16;
                    idx[i + 2] = (pt1 + 1) as u16;
                    pt1 += 1;
                } else {
                    idx[i] = pt1 as u16;
                    idx[i + 1] = pt2 as u16;
                    idx[i + 2] = (pt2 - 1) as u16;
                    pt2 -= 1;
                }
                i += 3;
            }
        }
    } else {
        // Not hollow, generate the triangle fan.
        let (v1, v2) = if top {
            (1usize, 2usize)
        } else {
            (2usize, 1usize)
        };
        for i in 0..num_vertices - 2 {
            idx[3 * i] = (num_vertices - 1) as u16;
            idx[3 * i + v1] = i as u16;
            idx[3 * i + v2] = (i + 1) as u16;
        }
    }

    if idx.len() < 3 {
        return false;
    }
    let (i0, i1, i2) = (idx[0] as usize, idx[1] as usize, idx[2] as usize);
    if i0 >= b.positions.len() || i1 >= b.positions.len() || i2 >= b.positions.len() {
        return false;
    }
    let d0 = b.positions[i1].sub(b.positions[i0]);
    let d1 = b.positions[i2].sub(b.positions[i0]);
    let mut normal = V4::cross3(d0, d1);
    if V4::dot3(normal, normal) > F_APPROXIMATELY_ZERO {
        normal = normal.normalize3fast();
    } else {
        // Degenerate, make up a value.
        normal = if normal.0[2] >= 0.0 {
            V4::set3(0.0, 0.0, 1.0)
        } else {
            V4::set3(0.0, 0.0, -1.0)
        };
    }

    for n in &mut b.normals[..num_vertices] {
        *n = normal;
    }
    true
}

/// `LLVolumeFace::createSide`.
fn create_side(p: &FaceParams, b: &mut FaceBuffers, out: &mut FaceResult) -> bool {
    let mask = p.face.type_mask;
    let flat = mask & FLAT_MASK != 0;

    let sculpt_type = p.sculpt_type;
    let sculpt_stitching = sculpt_type & SCULPT_TYPE_MASK;
    let sculpt_invert = sculpt_type & SCULPT_FLAG_INVERT != 0;
    let sculpt_mirror = sculpt_type & SCULPT_FLAG_MIRROR != 0;
    let sculpt_reverse_horizontal = if sculpt_invert {
        !sculpt_mirror
    } else {
        sculpt_mirror
    }; // XOR

    let mesh = p.mesh;
    let profile = p.profile;
    let path = p.path;

    let max_s = p.profile_total;
    let begin_s = p.face.begin_s;
    let num_s_face = p.face.num_s;
    let begin_t = p.face.begin_t;
    let num_t = p.face.num_t;

    let num_vertices = (num_s_face * num_t) as usize;
    let num_indices = ((num_s_face - 1) * (num_t - 1) * 6).max(0) as usize;

    let buf_nv = b.positions.len();
    let buf_ni = b.indices.len();
    if num_vertices == 0
        || buf_nv < num_vertices
        || b.normals.len() < buf_nv
        || b.texcoords.len() < buf_nv + (buf_nv & 1)
        || buf_ni < num_indices
    {
        return false;
    }
    if begin_s < 0
        || begin_s as usize >= profile.len()
        || begin_t < 0
        || (begin_t + num_t) as usize > path.len()
    {
        return false;
    }

    let begin_stex = profile[begin_s as usize].0[2].floor();
    let inner_flat = mask & INNER_MASK != 0 && mask & FLAT_MASK != 0 && num_s_face > 2;
    let num_s = if inner_flat {
        num_s_face / 2
    } else {
        num_s_face
    };

    // Copy the vertices into the array.
    let mut cur_vertex = 0usize;
    let end_t = begin_t + num_t;
    for t in begin_t..end_t {
        let tt = path[t as usize].tex_t;
        for s in 0..num_s {
            let mut ss;
            if mask & END_MASK != 0 {
                ss = if s != 0 { 1.0 } else { 0.0 };
            } else {
                // Get s value for tex-coord.
                let index = begin_s + s;
                if index >= profile.len() as i32 {
                    // edge?
                    ss = if flat { 1.0 - begin_stex } else { 1.0 };
                } else if !flat {
                    ss = profile[index as usize].0[2];
                } else {
                    ss = profile[index as usize].0[2] - begin_stex;
                }
            }

            if sculpt_reverse_horizontal {
                ss = 1.0 - ss;
            }

            // Check to see if this triangle wraps around the array.
            let i = if begin_s + s >= max_s {
                begin_s + s + max_s * (t - 1)
            } else {
                begin_s + s + max_s * t
            };
            let Some(&m) = mesh.get(i as usize) else {
                return false;
            };

            b.positions[cur_vertex] = m;
            b.texcoords[cur_vertex] = [ss, tt];
            cur_vertex += 1;

            if inner_flat && s > 0 {
                b.positions[cur_vertex] = m;
                b.texcoords[cur_vertex] = [ss, tt];
                cur_vertex += 1;
            }
        }

        if inner_flat {
            let s = if mask & OPEN_MASK != 0 { num_s - 1 } else { 0 };
            let i = begin_s + s + max_s * t;
            let Some(&m) = mesh.get(i as usize) else {
                return false;
            };
            let Some(pr) = profile.get((begin_s + s) as usize) else {
                return false;
            };
            let ss = pr.0[2] - begin_stex;
            b.positions[cur_vertex] = m;
            b.texcoords[cur_vertex] = [ss, tt];
            cur_vertex += 1;
        }
    }

    // Bounding box for this side, over the allocated vertex count as the
    // C++ does.
    let pos = &b.positions[..buf_nv];
    let mut face_min = pos[0];
    let mut face_max = face_min;
    for &v in &pos[1..] {
        update_min_max(&mut face_min, &mut face_max, v);
    }
    out.extents_min = face_min;
    out.extents_max = face_max;

    let mut tc_count = buf_nv;
    if tc_count % 2 == 1 {
        // Odd number of texture coordinates: duplicate the last entry into
        // the padded end of the array.
        b.texcoords[buf_nv] = b.texcoords[buf_nv - 1];
        tc_count += 1;
    }
    let tcs = &b.texcoords[..tc_count];
    let pair = |i: usize| V4([tcs[i][0], tcs[i][1], tcs[i + 1][0], tcs[i + 1][1]]);
    let mut tc_min = pair(0);
    let mut tc_max = tc_min;
    let mut i = 2;
    while i < tc_count {
        update_min_max(&mut tc_min, &mut tc_max, pair(i));
        i += 2;
    }
    out.tc_min = [
        llmin(tc_min.0[0], tc_min.0[2]),
        llmin(tc_min.0[1], tc_min.0[3]),
    ];
    out.tc_max = [
        llmax(tc_max.0[0], tc_max.0[2]),
        llmax(tc_max.0[1], tc_max.0[3]),
    ];
    out.tc_extents_set = true;

    out.center = face_min.add(face_max).mul_s(0.5);

    if b.write_indices {
        // Now we generate the indices.
        let mut cur_index = 0usize;
        let ns = num_s_face;
        for t in 0..num_t - 1 {
            for s in 0..ns - 1 {
                b.indices[cur_index] = (s + ns * t) as u16; // bottom left
                b.indices[cur_index + 1] = (s + 1 + ns * (t + 1)) as u16; // top right
                b.indices[cur_index + 2] = (s + ns * (t + 1)) as u16; // top left
                b.indices[cur_index + 3] = (s + ns * t) as u16; // bottom left
                b.indices[cur_index + 4] = (s + 1 + ns * t) as u16; // bottom right
                b.indices[cur_index + 5] = (s + 1 + ns * (t + 1)) as u16; // top right
                cur_index += 6;
            }
        }
    }

    // Clear normals.
    for n in &mut b.normals[..buf_nv] {
        *n = V4::ZERO;
    }

    // Generate normals: one per triangle over the allocated index count,
    // then accumulated on the vertices.
    let count = buf_ni / 3;
    let idx = &b.indices[..count * 3];
    for i in idx {
        if (*i as usize) >= buf_nv {
            return false;
        }
    }
    let pos = &b.positions[..buf_nv];
    TRIANGLE_NORMALS.with(|cell| {
        let mut tn = cell.borrow_mut();
        tn.clear();
        tn.reserve(count);
        for tri in idx.as_chunks::<3>().0 {
            let mut bv = pos[tri[0] as usize];
            let v1 = pos[tri[1] as usize];
            let v2 = pos[tri[2] as usize];
            let a = bv.sub(v1);
            bv = bv.sub(v2);
            tn.push(V4::cross3(a, bv));
        }

        let norm = &mut b.normals[..buf_nv];
        for (i, tri) in idx.as_chunks::<3>().0.iter().enumerate() {
            let c = tn[i];
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let mut n0 = norm[i0].add(c);
            let mut n1 = norm[i1].add(c);
            let mut n2 = norm[i2].add(c);
            // Even out quad contributions.
            match i % 2 + 1 {
                1 => n1 = n1.add(c),
                2 => n2 = n2.add(c),
                _ => n0 = n0.add(c),
            }
            norm[i0] = n0;
            norm[i1] = n1;
            norm[i2] = n2;
        }
    });

    // Adjust normals based on wrapping and stitching.
    let ns = num_s_face as usize;
    let nt = num_t as usize;
    let (pos, norm) = (&b.positions[..buf_nv], &mut b.normals[..buf_nv]);
    let base = ns * nt.wrapping_sub(2);
    let Some(&p_base) = pos.get(base) else {
        return false;
    };
    let Some(&p_base_top) = pos.get(base + ns - 1) else {
        return false;
    };
    let top = pos[0].sub(p_base);
    let s_bottom_converges = V4::dot3(top, top) < 0.000001;
    let top = pos[ns - 1].sub(p_base_top);
    let s_top_converges = V4::dot3(top, top) < 0.000001;

    if sculpt_stitching == SCULPT_TYPE_NONE {
        // Logic for non-sculpt volumes.
        if !p.path_open {
            // Wrap normals on T.
            for i in 0..ns {
                let n = norm[i].add(norm[ns * (nt - 1) + i]);
                norm[i] = n;
                norm[ns * (nt - 1) + i] = n;
            }
        }

        if !p.profile_open && !s_bottom_converges {
            // Wrap normals on S.
            for i in 0..nt {
                let n = norm[ns * i].add(norm[ns * i + ns - 1]);
                norm[ns * i] = n;
                norm[ns * i + ns - 1] = n;
            }
        }

        if p.path_curve == PATH_CIRCLE && (p.profile_curve & PROFILE_MASK) == PROFILE_CIRCLE_HALF {
            if s_bottom_converges {
                // All lower S have the same normal.
                for i in 0..nt {
                    norm[ns * i] = V4::set3(1.0, 0.0, 0.0);
                }
            }
            if s_top_converges {
                // All upper S have the same normal.
                for i in 0..nt {
                    norm[ns * i + ns - 1] = V4::set3(-1.0, 0.0, 0.0);
                }
            }
        }
    } else {
        // Logic for sculpt volumes.
        let average_poles = sculpt_stitching == SCULPT_TYPE_SPHERE;
        let wrap_s = sculpt_stitching == SCULPT_TYPE_SPHERE
            || sculpt_stitching == SCULPT_TYPE_TORUS
            || sculpt_stitching == SCULPT_TYPE_CYLINDER;
        let wrap_t = sculpt_stitching == SCULPT_TYPE_TORUS;

        if average_poles {
            // Average normals for the north pole.
            let mut average = V4::ZERO;
            for i in 0..ns {
                average = average.add(norm[i]);
            }
            for i in 0..ns {
                norm[i] = average;
            }
            // Average normals for the south pole.
            average = V4::ZERO;
            for i in 0..ns {
                average = average.add(norm[i + ns * (nt - 1)]);
            }
            for i in 0..ns {
                norm[i + ns * (nt - 1)] = average;
            }
        }

        if wrap_s {
            for i in 0..nt {
                let n = norm[ns * i].add(norm[ns * i + ns - 1]);
                norm[ns * i] = n;
                norm[ns * i + ns - 1] = n;
            }
        }

        if wrap_t {
            for i in 0..ns {
                let n = norm[i].add(norm[ns * (nt - 1) + i]);
                norm[i] = n;
                norm[ns * (nt - 1) + i] = n;
            }
        }
    }

    true
}

thread_local! {
    /// The per-thread triangle normal scratch array `createSide` keeps.
    static TRIANGLE_NORMALS: std::cell::RefCell<Vec<V4>> = const { std::cell::RefCell::new(Vec::new()) };
}
