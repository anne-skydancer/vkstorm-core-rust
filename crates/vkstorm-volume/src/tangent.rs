//! `LLCalculateTangentArray` (Lengyel's per-triangle tangent accumulation
//! with Gram-Schmidt orthogonalisation) and the normal normalisation
//! `LLVolumeFace::createTangents` does after it.

use crate::params::F_APPROXIMATELY_ZERO;
use crate::simd::V4;

/// `LLCalculateTangentArray`: fills `tangent` (one per vertex, handedness in
/// w) from the positions, normals, texture coordinates and triangles.
pub fn calculate_tangent_array(
    vertex: &[V4],
    normal: &[V4],
    texcoord: &[[f32; 2]],
    indices: &[u16],
    tangent: &mut [V4],
) {
    let vertex_count = vertex.len();
    assert!(
        normal.len() >= vertex_count
            && texcoord.len() >= vertex_count
            && tangent.len() >= vertex_count
    );

    // The per-thread accumulators (the C++ allocates tan1 per call; a
    // reused buffer avoids the allocator in the hot path).
    SCRATCH.with(|cell| {
        let mut scratch = cell.borrow_mut();
        scratch.clear();
        scratch.resize(vertex_count * 2, V4::ZERO);
        let (tan1, tan2) = scratch.split_at_mut(vertex_count);
        accumulate(vertex, normal, texcoord, indices, tangent, tan1, tan2);
    });
}

thread_local! {
    static SCRATCH: std::cell::RefCell<Vec<V4>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn accumulate(
    vertex: &[V4],
    normal: &[V4],
    texcoord: &[[f32; 2]],
    indices: &[u16],
    tangent: &mut [V4],
    tan1: &mut [V4],
    tan2: &mut [V4],
) {
    let vertex_count = vertex.len();

    for tri in indices.as_chunks::<3>().0 {
        let (i1, i2, i3) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        if i1 >= vertex_count || i2 >= vertex_count || i3 >= vertex_count {
            continue;
        }
        let v1 = vertex[i1].0;
        let v2 = vertex[i2].0;
        let v3 = vertex[i3].0;
        let w1 = texcoord[i1];
        let w2 = texcoord[i2];
        let w3 = texcoord[i3];

        let x1 = v2[0] - v1[0];
        let x2 = v3[0] - v1[0];
        let y1 = v2[1] - v1[1];
        let y2 = v3[1] - v1[1];
        let z1 = v2[2] - v1[2];
        let z2 = v3[2] - v1[2];

        let s1 = w2[0] - w1[0];
        let s2 = w3[0] - w1[0];
        let t1 = w2[1] - w1[1];
        let t2 = w3[1] - w1[1];

        let rd = s1 * t2 - s2 * t1;

        // Some made up large ratio for division by zero.
        let r = if rd * rd > f32::EPSILON {
            1.0 / rd
        } else if rd > 0.0 {
            1024.0
        } else {
            -1024.0
        };

        let sdir = V4::set3(
            (t2 * x1 - t1 * x2) * r,
            (t2 * y1 - t1 * y2) * r,
            (t2 * z1 - t1 * z2) * r,
        );
        let tdir = V4::set3(
            (s1 * x2 - s2 * x1) * r,
            (s1 * y2 - s2 * y1) * r,
            (s1 * z2 - s2 * z1) * r,
        );

        tan1[i1] = tan1[i1].add(sdir);
        tan1[i2] = tan1[i2].add(sdir);
        tan1[i3] = tan1[i3].add(sdir);

        tan2[i1] = tan2[i1].add(tdir);
        tan2[i2] = tan2[i2].add(tdir);
        tan2[i3] = tan2[i3].add(tdir);
    }

    for a in 0..vertex_count {
        let mut n = normal[a];
        let t = tan1[a];

        let ncrosst = V4::cross3(n, t);

        // Gram-Schmidt orthogonalize.
        n = n.mul_s(V4::dot3(n, t));

        let mut tsubn = t.sub(n);

        if V4::dot3(tsubn, tsubn) > F_APPROXIMATELY_ZERO {
            tsubn = tsubn.normalize3fast();

            // Calculate handedness.
            let handedness = if V4::dot3(ncrosst, tan2[a]) < 0.0 {
                -1.0
            } else {
                1.0
            };
            tsubn.0[3] = handedness;

            tangent[a] = tsubn;
        } else {
            // Degenerate, make up a value.
            tangent[a] = V4::new(0.0, 0.0, 1.0, 1.0);
        }
    }
}

/// `LLVolumeFace::createTangents`: the tangents, then every normal
/// normalised in place (the bump-map and planar-projection code requires
/// unit normals).
pub fn calculate_tangents(
    vertex: &[V4],
    normal: &mut [V4],
    texcoord: &[[f32; 2]],
    indices: &[u16],
    tangent: &mut [V4],
) {
    for t in tangent.iter_mut() {
        *t = V4::ZERO;
    }
    calculate_tangent_array(vertex, normal, texcoord, indices, tangent);
    for n in normal.iter_mut() {
        *n = n.normalize3fast();
    }
}
