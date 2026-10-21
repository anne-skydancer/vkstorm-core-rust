//! `LLVolume::unpackVolumeFaces`: a zlib-compressed LLSD mesh LOD unpacked
//! into face buffers laid out as `LLVolumeFace` holds them (16-byte
//! `LLVector4a` positions and normals, `LLVector2` texture coordinates,
//! `U16` indices, `LLVector4a` joint weights), with the same float
//! operation order as the SSE code it replaces.
//!
//! The caller copies the buffers into its `LLVolumeFace` storage and runs
//! `cacheOptimize` (kept in C++) on them.

use vkstorm_llsd::Value;

use super::inflate::{unzip_llsd, ZipResult};
use super::llsd::{as_binary, get, has, index, size, vector2, vector3};

/// `LL_SCULPT_FLAG_INVERT` and `LL_SCULPT_FLAG_MIRROR` of the sculpt type.
pub const SCULPT_FLAG_INVERT: u8 = 64;
pub const SCULPT_FLAG_MIRROR: u8 = 128;

/// One unpacked face.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Face {
    /// `mNumVertices` and `mNumIndices` as the C++ left them.
    pub num_vertices: i32,
    pub num_indices: i32,
    /// Positions and normals, 4 floats each (the fourth lane as the SSE
    /// code left it: 0 for positions, -1 for normals from data).
    pub positions: Vec<[f32; 4]>,
    pub normals: Vec<[f32; 4]>,
    /// Texture coordinates in pairs: `num_vertices` rounded up to even
    /// entries, the padding entry carrying what the pairwise SSE code wrote
    /// there.
    pub texcoords: Vec<[f32; 2]>,
    pub indices: Vec<u16>,
    /// Joint weights when the face has them (`allocateWeights`).
    pub weights: Option<Vec<[f32; 4]>>,
    /// `mExtents[0..2]` and `mTexCoordExtents`.
    pub extents: [[f32; 4]; 2],
    pub tc_extents: [[f32; 2]; 2],
    pub normalized_scale: [f32; 3],
    /// A face the C++ skipped with `continue` after allocating (an empty
    /// index list or an allocation failure): its buffers hold what the
    /// allocation left, zero here.
    pub incomplete: bool,
}

impl Face {
    fn fresh() -> Face {
        Face {
            extents: [[-0.5; 4], [0.5; 4]],
            normalized_scale: [1.0, 1.0, 1.0],
            ..Default::default()
        }
    }
}

/// A failure of `unpackVolumeFaces`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnpackError {
    /// `unzip_llsd` failed with the viewer's code.
    Zip(ZipResult),
    /// "found no faces!"
    NoFaces,
    /// A buffer shorter than the vertex count implies (the C++ read past
    /// it).
    ShortBuffer(&'static str),
}

#[inline]
fn min_lane(a: f32, b: f32) -> f32 {
    // _mm_min_ps: the second operand when equal or unordered.
    if a < b { a } else { b }
}

#[inline]
fn max_lane(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

/// Unpacks the zlib-compressed LLSD block of one LOD.
pub fn unpack_volume_faces(data: &[u8], sculpt_type: u8) -> Result<Vec<Face>, UnpackError> {
    let mdl = unzip_llsd(data).map_err(UnpackError::Zip)?;
    unpack_volume_faces_llsd(&mdl, sculpt_type)
}

/// `unpackVolumeFacesInternal` over the parsed LLSD.
pub fn unpack_volume_faces_llsd(mdl: &Value, sculpt_type: u8) -> Result<Vec<Face>, UnpackError> {
    let face_count = size(mdl);
    if face_count == 0 {
        crate::ll_warns!("found no faces!");
        return Err(UnpackError::NoFaces);
    }
    let do_mirror = sculpt_type & SCULPT_FLAG_MIRROR != 0;
    let do_invert = sculpt_type & SCULPT_FLAG_INVERT != 0;
    let mut faces: Vec<Face> = Vec::with_capacity(face_count);
    for i in 0..face_count {
        let fd = index(mdl, i);
        let mut face = Face::fresh();
        if has(fd, "NoGeometry") {
            face.num_indices = 3;
            face.num_vertices = 1;
            face.positions = vec![[0.0; 4]];
            face.normals = vec![[0.0; 4]];
            face.texcoords = vec![[0.0; 2], [0.0; 2]];
            face.indices = vec![0; 3];
            faces.push(face);
            continue;
        }
        let pos = as_binary(get(fd, "Position"));
        let norm = as_binary(get(fd, "Normal"));
        let tc = as_binary(get(fd, "TexCoord0"));
        let idx = as_binary(get(fd, "TriangleList"));

        let mut num_indices = idx.len() / 2;
        let indices_to_discard = num_indices % 3;
        if indices_to_discard > 0 {
            crate::ll_warns!(
                "Incomplete triangle discarded from face! Indices count {} was not divisible by 3. face index: {} Total: {}",
                num_indices,
                i,
                face_count
            );
            num_indices -= indices_to_discard;
        }
        face.num_indices = num_indices as i32;
        face.indices = vec![0u16; num_indices];
        if idx.is_empty() || face.num_indices < 3 {
            crate::ll_warns!("Empty face present! Face index: {} Total: {}", i, face_count);
            face.incomplete = true;
            faces.push(face);
            continue;
        }
        for j in 0..num_indices {
            face.indices[j] = u16::from_le_bytes([idx[2 * j], idx[2 * j + 1]]);
        }

        let num_verts = pos.len() / 6;
        face.num_vertices = num_verts as i32;
        face.positions = vec![[0.0; 4]; num_verts];
        face.normals = vec![[0.0; 4]; num_verts];
        let tc_entries = num_verts + (num_verts & 1);
        face.texcoords = vec![[0.0; 2]; tc_entries];
        if num_verts == 0 {
            // resizeVertices(0) leaves the counts at zero; the C++ then
            // computed a 1cm bounding box below.
        }

        let minp = vector3(get(get(fd, "PositionDomain"), "Min"));
        let maxp = vector3(get(get(fd, "PositionDomain"), "Max"));
        let min_pos = [minp[0], minp[1], minp[2], 0.0];
        let max_pos = [maxp[0], maxp[1], maxp[2], 0.0];
        let min_tc = vector2(get(get(fd, "TexCoord0Domain"), "Min"));
        let max_tc = vector2(get(get(fd, "TexCoord0Domain"), "Max"));
        if has(fd, "NormalizedScale") {
            face.normalized_scale = vector3(get(fd, "NormalizedScale"));
        } else {
            face.normalized_scale = [1.0, 1.0, 1.0];
        }
        let pos_range = [max_pos[0] - min_pos[0], max_pos[1] - min_pos[1], max_pos[2] - min_pos[2], max_pos[3] - min_pos[3]];
        let tc_range2 = [max_tc[0] - min_tc[0], max_tc[1] - min_tc[1]];
        let tc_range = [tc_range2[0], tc_range2[1], tc_range2[0], tc_range2[1]];
        let min_tc4 = [min_tc[0], min_tc[1], min_tc[0], min_tc[1]];

        for j in 0..num_verts {
            let v0 = u16::from_le_bytes([pos[6 * j], pos[6 * j + 1]]) as f32;
            let v1 = u16::from_le_bytes([pos[6 * j + 2], pos[6 * j + 3]]) as f32;
            let v2 = u16::from_le_bytes([pos[6 * j + 4], pos[6 * j + 5]]) as f32;
            let mut p = [v0, v1, v2, 0.0];
            for k in 0..4 {
                p[k] /= 65535.0;
                p[k] *= pos_range[k];
                p[k] += min_pos[k];
            }
            face.positions[j] = p;
        }

        if !norm.is_empty() {
            if norm.len() < num_verts * 6 {
                return Err(UnpackError::ShortBuffer("Normal"));
            }
            for j in 0..num_verts {
                let n0 = u16::from_le_bytes([norm[6 * j], norm[6 * j + 1]]) as f32;
                let n1 = u16::from_le_bytes([norm[6 * j + 2], norm[6 * j + 3]]) as f32;
                let n2 = u16::from_le_bytes([norm[6 * j + 4], norm[6 * j + 5]]) as f32;
                let mut n = [n0, n1, n2, 0.0];
                for k in 0..4 {
                    n[k] /= 65535.0;
                    n[k] *= 2.0;
                    n[k] -= 1.0;
                }
                face.normals[j] = n;
            }
        }

        if !tc.is_empty() {
            // Pairs of texture coordinates; the last odd one reads two
            // values only.
            let mut j = 0;
            let mut t = 0;
            while j < num_verts {
                let mut v = [0.0f32; 4];
                if j < num_verts - 1 {
                    if tc.len() < (t + 4) * 2 {
                        return Err(UnpackError::ShortBuffer("TexCoord0"));
                    }
                    for k in 0..4 {
                        v[k] = u16::from_le_bytes([tc[2 * (t + k)], tc[2 * (t + k) + 1]]) as f32;
                    }
                } else {
                    if tc.len() < (t + 2) * 2 {
                        return Err(UnpackError::ShortBuffer("TexCoord0"));
                    }
                    v[0] = u16::from_le_bytes([tc[2 * t], tc[2 * t + 1]]) as f32;
                    v[1] = u16::from_le_bytes([tc[2 * (t + 1)], tc[2 * (t + 1) + 1]]) as f32;
                }
                t += 4;
                for k in 0..4 {
                    v[k] /= 65535.0;
                    v[k] *= tc_range[k];
                    v[k] += min_tc4[k];
                }
                face.texcoords[j] = [v[0], v[1]];
                face.texcoords[j + 1] = [v[2], v[3]];
                j += 2;
            }
        }

        if has(fd, "Weights") {
            let weights = as_binary(get(fd, "Weights"));
            let mut w = vec![[0.0f32; 4]; num_verts];
            let mut idx = 0usize;
            let mut cur_vertex = 0usize;
            const END_INFLUENCES: u8 = 0xFF;
            while idx < weights.len() && cur_vertex < num_verts {
                let mut joint = weights[idx];
                idx += 1;
                let mut cur_influence = 0usize;
                let mut wght = [0.0f32; 4];
                let mut joints = [0u32; 4];
                let mut joints_with_weights = [0.0f32; 4];
                while joint != END_INFLUENCES && idx < weights.len() {
                    let lo = weights[idx] as u16;
                    idx += 1;
                    let hi = if idx < weights.len() { weights[idx] as u16 } else { 0 };
                    idx += 1;
                    let influence = lo | (hi << 8);
                    let wv = (influence as f32 / 65535.0).clamp(0.001, 0.999);
                    wght[cur_influence] = wv;
                    joints[cur_influence] = joint as u32;
                    cur_influence += 1;
                    if cur_influence >= 4 {
                        joint = END_INFLUENCES;
                    } else if idx < weights.len() {
                        joint = weights[idx];
                        idx += 1;
                    } else {
                        // The C++ read one byte past the buffer here.
                        joint = END_INFLUENCES;
                        idx += 1;
                    }
                }
                let wsum = wght[0] + wght[1] + wght[2] + wght[3];
                if wsum <= 0.0 {
                    wght = [0.999, 0.0, 0.0, 0.0];
                }
                for k in 0..4 {
                    joints_with_weights[k] = joints[k] as f32 + wght[k];
                }
                w[cur_vertex] = joints_with_weights;
                cur_vertex += 1;
            }
            if cur_vertex != num_verts || idx != weights.len() {
                crate::ll_warns!("Vertex weight count does not match vertex count!");
            }
            face.weights = Some(w);
        }

        let mut do_reflect_x = false;
        let mut do_reverse_triangles = false;
        let mut do_invert_normals = false;
        if do_mirror {
            do_reflect_x = true;
            do_reverse_triangles = !do_reverse_triangles;
        }
        if do_invert {
            do_invert_normals = true;
            do_reverse_triangles = !do_reverse_triangles;
        }
        if do_reflect_x {
            for p in face.positions.iter_mut() {
                for k in 0..4 {
                    p[k] *= -1.0;
                }
            }
            for n in face.normals.iter_mut() {
                for k in 0..4 {
                    n[k] *= -1.0;
                }
            }
        }
        if do_invert_normals {
            for n in face.normals.iter_mut() {
                for k in 0..4 {
                    n[k] *= -1.0;
                }
            }
        }
        if do_reverse_triangles {
            let mut j = 0;
            while j + 2 < face.indices.len() {
                face.indices.swap(j + 1, j + 2);
                j += 3;
            }
        }

        if face.num_vertices < 3 {
            face.extents = [[-0.005; 4], [0.005; 4]];
        } else {
            let mut min = face.positions[0];
            let mut max = face.positions[0];
            for p in face.positions.iter().skip(1) {
                for k in 0..4 {
                    min[k] = min_lane(min[k], p[k]);
                    max[k] = max_lane(max[k], p[k]);
                }
            }
            face.extents = [min, max];
            let mut min_tc = face.texcoords[0];
            let mut max_tc = face.texcoords[0];
            for t in face.texcoords.iter().take(num_verts).skip(1) {
                for k in 0..2 {
                    if t[k] < min_tc[k] {
                        min_tc[k] = t[k];
                    }
                    if t[k] > max_tc[k] {
                        max_tc[k] = t[k];
                    }
                }
            }
            face.tc_extents = [min_tc, max_tc];
        }
        faces.push(face);
    }
    Ok(faces)
}

/// `LLMeshRepoThread::physicsShapeReceived`'s mesh extraction: every
/// index's position and normal, in index order, as `LLVector3`s.
pub fn physics_shape(data: &[u8]) -> Option<(Vec<[f32; 3]>, Vec<[f32; 3]>)> {
    let faces = unpack_volume_faces(data, 0).ok()?;
    let mut pos = Vec::new();
    let mut norm = Vec::new();
    for face in &faces {
        for &i in &face.indices {
            let p = face.positions.get(i as usize).copied().unwrap_or([0.0; 4]);
            let n = face.normals.get(i as usize).copied().unwrap_or([0.0; 4]);
            pos.push([p[0], p[1], p[2]]);
            norm.push([n[0], n[1], n[2]]);
        }
    }
    Some((pos, norm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn face_value(verts: &[[f32; 3]], tris: &[u16], with_normals: bool) -> Value {
        let mut m = BTreeMap::new();
        let mut min = verts[0];
        let mut max = verts[0];
        for v in verts {
            for k in 0..3 {
                min[k] = min[k].min(v[k]);
                max[k] = max[k].max(v[k]);
            }
        }
        let mut pos = Vec::new();
        let mut tc = Vec::new();
        for v in verts {
            for k in 0..3 {
                let r = max[k] - min[k];
                let q = if r > 0.0 { ((v[k] - min[k]) / r * 65535.0) as u16 } else { 0 };
                pos.extend_from_slice(&q.to_le_bytes());
            }
            tc.extend_from_slice(&((v[0] * 1000.0) as u16).to_le_bytes());
            tc.extend_from_slice(&((v[1] * 1000.0) as u16).to_le_bytes());
        }
        let mut idx = Vec::new();
        for t in tris {
            idx.extend_from_slice(&t.to_le_bytes());
        }
        m.insert(b"Position".to_vec(), Value::Binary(pos));
        if with_normals {
            let mut n = Vec::new();
            for _ in verts {
                n.extend_from_slice(&65535u16.to_le_bytes());
                n.extend_from_slice(&0u16.to_le_bytes());
                n.extend_from_slice(&32767u16.to_le_bytes());
            }
            m.insert(b"Normal".to_vec(), Value::Binary(n));
        }
        m.insert(b"TexCoord0".to_vec(), Value::Binary(tc));
        m.insert(b"TriangleList".to_vec(), Value::Binary(idx));
        let dom = |a: [f32; 3], b: [f32; 3]| {
            let mut d = BTreeMap::new();
            d.insert(b"Min".to_vec(), Value::Array(a.iter().map(|x| Value::Real(*x as f64)).collect()));
            d.insert(b"Max".to_vec(), Value::Array(b.iter().map(|x| Value::Real(*x as f64)).collect()));
            Value::Map(d)
        };
        m.insert(b"PositionDomain".to_vec(), dom(min, max));
        let mut d = BTreeMap::new();
        d.insert(b"Min".to_vec(), Value::Array(vec![Value::Real(0.0), Value::Real(0.0)]));
        d.insert(b"Max".to_vec(), Value::Array(vec![Value::Real(1.0), Value::Real(1.0)]));
        m.insert(b"TexCoord0Domain".to_vec(), Value::Map(d));
        Value::Map(m)
    }

    #[test]
    fn unpacks_a_triangle() {
        let verts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.5]];
        let v = Value::Array(vec![face_value(&verts, &[0, 1, 2], true)]);
        let faces = unpack_volume_faces_llsd(&v, 0).unwrap();
        assert_eq!(faces.len(), 1);
        let f = &faces[0];
        assert_eq!(f.num_vertices, 3);
        assert_eq!(f.num_indices, 3);
        assert_eq!(f.positions[1], [1.0, 0.0, 0.0, 0.0]);
        assert!((f.positions[2][2] - 0.5).abs() < 1e-4);
        assert_eq!(f.normals[0][3], -1.0);
        assert!((f.normals[0][0] - 1.0).abs() < 1e-6);
        assert_eq!(f.indices, vec![0, 1, 2]);
        assert_eq!(f.texcoords.len(), 4);
        assert_eq!(f.extents[0], [0.0, 0.0, 0.0, 0.0]);
        assert!((f.extents[1][2] - 0.5).abs() < 1e-4);
        assert_eq!(f.tc_extents[0], [0.0, 0.0]);
        // Mirror + invert: reversed twice, positions negated, normals
        // negated twice.
        let faces = unpack_volume_faces_llsd(&v, SCULPT_FLAG_MIRROR | SCULPT_FLAG_INVERT).unwrap();
        assert_eq!(faces[0].indices, vec![0, 1, 2]);
        assert_eq!(faces[0].positions[1], [-1.0, -0.0, -0.0, -0.0]);
        assert_eq!(faces[0].normals[0][3], -1.0);
        let faces = unpack_volume_faces_llsd(&v, SCULPT_FLAG_MIRROR).unwrap();
        assert_eq!(faces[0].indices, vec![0, 2, 1]);
        assert_eq!(faces[0].normals[0][3], 1.0);
    }

    #[test]
    fn degenerate_faces() {
        assert_eq!(unpack_volume_faces_llsd(&Value::Array(vec![]), 0).unwrap_err(), UnpackError::NoFaces);
        let mut ng = BTreeMap::new();
        ng.insert(b"NoGeometry".to_vec(), Value::Boolean(true));
        let faces = unpack_volume_faces_llsd(&Value::Array(vec![Value::Map(ng)]), 0).unwrap();
        assert_eq!(faces[0].num_vertices, 1);
        assert_eq!(faces[0].num_indices, 3);
        assert_eq!(faces[0].indices, vec![0, 0, 0]);
        // Two indices: an empty face, left incomplete.
        let verts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let v = Value::Array(vec![face_value(&verts, &[0, 1], false)]);
        let faces = unpack_volume_faces_llsd(&v, 0).unwrap();
        assert!(faces[0].incomplete);
        assert_eq!(faces[0].num_indices, 0);
        assert_eq!(faces[0].num_vertices, 0);
    }

    #[test]
    fn weights_decode() {
        let verts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mut fv = match face_value(&verts, &[0, 1, 2], false) {
            Value::Map(m) => m,
            _ => unreachable!(),
        };
        // vertex 0: joint 3 @ 0.5 ; vertex 1: joints 1 @ 0.25, 2 @ 0.75; vertex 2: no influences.
        let mut w = Vec::new();
        w.push(3u8);
        w.extend_from_slice(&((0.5f32 * 65535.0) as u16).to_le_bytes());
        w.push(0xFF);
        w.push(1u8);
        w.extend_from_slice(&((0.25f32 * 65535.0) as u16).to_le_bytes());
        w.push(2u8);
        w.extend_from_slice(&((0.75f32 * 65535.0) as u16).to_le_bytes());
        w.push(0xFF);
        w.push(0xFF);
        fv.insert(b"Weights".to_vec(), Value::Binary(w));
        let faces = unpack_volume_faces_llsd(&Value::Array(vec![Value::Map(fv)]), 0).unwrap();
        let w = faces[0].weights.as_ref().unwrap();
        assert!((w[0][0] - 3.5).abs() < 1e-4 && w[0][1] == 0.0);
        assert!((w[1][0] - 1.25).abs() < 1e-4 && (w[1][1] - 2.75).abs() < 1e-4);
        assert_eq!(w[2], [0.999, 0.0, 0.0, 0.0]);
    }
}
