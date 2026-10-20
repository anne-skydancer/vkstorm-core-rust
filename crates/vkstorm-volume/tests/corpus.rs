//! Bit-for-bit parity with the viewer's C++ mesher over the recorded corpus:
//! 881 cases over every profile and path type, hole shape, cut, twist,
//! taper, shear, radius offset, revolutions, skew and LOD the viewer uses,
//! plus flexible paths and the sculpt stitching branch of the face builder.
//!
//! There is one corpus per platform toolchain, each recorded from the C++
//! mesher built by that toolchain on an AMD CPU: Windows/MSVC/UCRT and
//! Linux/GCC/glibc (`libm` and `std::lerp` differ between them, as they did
//! for the C++). On those platforms every block must match exactly. One
//! thing is inherently CPU-dependent and is handled rather than hidden:
//! `_mm_rsqrt_ps` tables differ between CPU vendors, so the fixture carries
//! a signature of the recording CPU, and when it differs the rsqrt-derived
//! arrays (cap normals, tangents, normalised normals) are compared with a
//! tolerance. A platform without a recording gets the Windows corpus with a
//! tolerance on the float arrays and exact integer data.
#![allow(clippy::chunks_exact_to_as_chunks)] // fixed-size record parsing

mod common;

use common::*;
use vkstorm_volume::params::CAP_MASK;

struct Check {
    same_rsqrt: bool,
    mismatches: Vec<String>,
    mismatch_count: usize,
    exact_blocks: usize,
    tolerance_blocks: usize,
    unverifiable_blocks: usize,
}

impl Check {
    fn add(&mut self, s: String) {
        self.mismatch_count += 1;
        if self.mismatches.len() < 40 {
            self.mismatches.push(s);
        }
    }

    fn scalar<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, expected: T, actual: T) {
        if expected != actual {
            self.add(format!("{what}: expected {expected:?} got {actual:?}"));
        }
    }

    fn f32_bits(&mut self, what: &str, expected: &[f32], actual: &[f32]) {
        let same = expected.len() == actual.len()
            && expected
                .iter()
                .zip(actual)
                .all(|(a, b)| a.to_bits() == b.to_bits());
        if same {
            return;
        }
        if !STRICT_PLATFORM && within(expected, actual, 1e-4) {
            return;
        }
        self.add(format!("{what}: expected {expected:?} got {actual:?}"));
    }

    /// Compares a recorded array block with the Rust bytes. `rsqrt` marks
    /// values that went through `_mm_rsqrt_ps`.
    fn block(&mut self, what: &str, block: &Block, actual: &[u8], elem: usize, rsqrt: bool) {
        if let Some(d) = &block.data
            && d.len() != actual.len()
        {
            self.add(format!(
                "{what}: length {} vs recorded {}",
                actual.len(),
                d.len()
            ));
            return;
        }
        if fnv1a(actual) == block.hash {
            self.exact_blocks += 1;
            return;
        }
        let loose = (rsqrt && !self.same_rsqrt) || !STRICT_PLATFORM;
        if loose {
            match &block.data {
                Some(d) => {
                    self.tolerance_blocks += 1;
                    let tol = if rsqrt { 1e-3 } else { 1e-4 };
                    if elem == 2 || !within(&floats(d), &floats(actual), tol) {
                        self.add(format!(
                            "{what}: outside tolerance{}",
                            first_diff(d, actual, elem)
                        ));
                    }
                }
                None => self.unverifiable_blocks += 1,
            }
            return;
        }
        match &block.data {
            Some(d) => self.add(format!("{what}{}", first_diff(d, actual, elem))),
            None => self.add(format!("{what}: hash mismatch over {} bytes", actual.len())),
        }
    }
}

fn floats(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn within(expected: &[f32], actual: &[f32], rel: f32) -> bool {
    expected.len() == actual.len()
        && expected.iter().zip(actual).all(|(e, a)| {
            if e.to_bits() == a.to_bits() {
                return true;
            }
            (e - a).abs() <= rel * e.abs().max(1.0)
        })
}

fn first_diff(expected: &[u8], actual: &[u8], elem: usize) -> String {
    for (i, (e, a)) in expected
        .chunks_exact(elem)
        .zip(actual.chunks_exact(elem))
        .enumerate()
    {
        if e != a {
            if elem == 2 {
                return format!(
                    " first difference at element {i}: expected {} got {}",
                    u16::from_le_bytes([e[0], e[1]]),
                    u16::from_le_bytes([a[0], a[1]])
                );
            }
            return format!(
                " first difference at element {i}: expected {:?} got {:?}",
                floats(e),
                floats(a)
            );
        }
    }
    String::from(" (no element differs; hash mismatch)")
}

#[test]
fn corpus_parity() {
    let corpus = load_corpus();
    let same_rsqrt = corpus.rsqrt_signature == rsqrt_signature();
    if !same_rsqrt {
        eprintln!(
            "this CPU's _mm_rsqrt_ps differs from the recording machine; rsqrt-derived arrays are compared with a tolerance"
        );
    }
    if !STRICT_PLATFORM {
        eprintln!(
            "not Windows/MSVC: float arrays are compared with a tolerance (libm differs from the recording platform)"
        );
    }
    let mut ck = Check {
        same_rsqrt,
        mismatches: Vec::new(),
        mismatch_count: 0,
        exact_blocks: 0,
        tolerance_blocks: 0,
        unverifiable_blocks: 0,
    };

    let mut faces_checked = 0usize;
    let mut full_cases = 0usize;
    for c in &corpus.cases {
        let tag = format!("[{}] {}", c.id, c.name);
        if c.flags & FLAG_FULL != 0 {
            full_cases += 1;
        }
        let b = build_case(c, true);

        ck.scalar(&format!("{tag} face mask"), c.face_mask, b.face_mask);
        if c.flags & FLAG_SCULPT == 0 {
            ck.scalar(
                &format!("{tag} LOD triangle counts"),
                c.lod_counts,
                lod_triangle_counts(&c.profile_params, &c.path_params),
            );
        }

        ck.scalar(
            &format!("{tag} profile point count"),
            c.profile_count,
            b.profile.points.len() as u32,
        );
        ck.scalar(
            &format!("{tag} profile total"),
            c.profile_total,
            b.profile.total,
        );
        ck.scalar(
            &format!("{tag} profile total_out"),
            c.profile_total_out,
            b.profile.total_out,
        );
        ck.scalar(
            &format!("{tag} profile open"),
            c.profile_open,
            b.profile.open,
        );
        ck.scalar(
            &format!("{tag} profile concave"),
            c.profile_concave,
            b.profile.concave,
        );
        ck.block(
            &format!("{tag} profile points"),
            &c.profile_points,
            bytes_of(&b.profile.points),
            16,
            false,
        );
        ck.scalar(
            &format!("{tag} profile face count"),
            c.profile_face_count,
            b.profile.faces.len() as u32,
        );
        ck.block(
            &format!("{tag} profile faces"),
            &c.profile_faces,
            &profile_faces_canonical(&b.profile),
            16,
            false,
        );

        ck.scalar(
            &format!("{tag} path point count"),
            c.path_count,
            b.path.points.len() as u32,
        );
        ck.scalar(&format!("{tag} path open"), c.path_open, b.path.open);
        ck.f32_bits(&format!("{tag} path step"), &[c.path_step], &[b.path.step]);
        let canon = path_canonical(&b.path);
        ck.block(
            &format!("{tag} path points"),
            &c.path_points,
            bytes_of(&canon),
            PATHPT_FLOATS * 4,
            false,
        );

        ck.scalar(
            &format!("{tag} mesh point count"),
            c.mesh_count,
            b.mesh.len() as u32,
        );
        ck.block(
            &format!("{tag} mesh"),
            &c.mesh,
            bytes_of(&b.mesh),
            16,
            false,
        );

        ck.scalar(
            &format!("{tag} volume face count"),
            c.faces.len(),
            b.faces.len(),
        );
        for (i, (rf, bf)) in c.faces.iter().zip(&b.faces).enumerate() {
            faces_checked += 1;
            let ft = format!("{tag} face {i}");
            ck.scalar(&format!("{ft} id"), rf.id, bf.desc.id);
            ck.scalar(&format!("{ft} type mask"), rf.type_mask, bf.desc.type_mask);
            ck.scalar(&format!("{ft} beginS"), rf.begin_s, bf.desc.begin_s);
            ck.scalar(&format!("{ft} numS"), rf.num_s, bf.desc.num_s);
            ck.scalar(&format!("{ft} beginT"), rf.begin_t, bf.desc.begin_t);
            ck.scalar(&format!("{ft} numT"), rf.num_t, bf.desc.num_t);
            ck.scalar(
                &format!("{ft} vertex count"),
                rf.num_vertices,
                bf.num_vertices as u32,
            );
            ck.scalar(
                &format!("{ft} index count"),
                rf.num_indices,
                bf.num_indices as u32,
            );
            if rf.num_vertices != bf.num_vertices as u32 || rf.num_indices != bf.num_indices as u32
            {
                continue;
            }
            ck.f32_bits(
                &format!("{ft} extents min"),
                &rf.extents[..4],
                &bf.result.extents_min.0,
            );
            ck.f32_bits(
                &format!("{ft} extents max"),
                &rf.extents[4..],
                &bf.result.extents_max.0,
            );
            ck.f32_bits(&format!("{ft} center"), &rf.center, &bf.result.center.0);
            let tc_ext = if bf.result.tc_extents_set {
                [
                    bf.result.tc_min[0],
                    bf.result.tc_min[1],
                    bf.result.tc_max[0],
                    bf.result.tc_max[1],
                ]
            } else {
                [0.0; 4] // caps leave LLVolumeFace::mTexCoordExtents at its zero initial value
            };
            ck.f32_bits(&format!("{ft} texcoord extents"), &rf.tc_extents, &tc_ext);
            let cap = rf.type_mask & CAP_MASK != 0;
            ck.block(
                &format!("{ft} positions"),
                &rf.positions,
                bytes_of(&bf.positions),
                16,
                false,
            );
            ck.block(
                &format!("{ft} normals"),
                &rf.normals,
                bytes_of(&bf.normals),
                16,
                cap,
            );
            ck.block(
                &format!("{ft} texcoords"),
                &rf.texcoords,
                bytes_of(&bf.texcoords[..bf.positions.len()]),
                8,
                false,
            );
            ck.block(
                &format!("{ft} indices"),
                &rf.indices,
                bytes_of(&bf.indices),
                2,
                false,
            );
            ck.block(
                &format!("{ft} tangents"),
                &rf.tangents,
                bytes_of(&bf.tangents),
                16,
                true,
            );
            ck.block(
                &format!("{ft} normalized normals"),
                &rf.normals_after,
                bytes_of(&bf.normals_after),
                16,
                true,
            );
        }
    }

    eprintln!(
        "corpus: {} cases ({} with full data), {} faces, {} blocks exact, {} within tolerance, {} unverifiable (hash only, loose mode), {} mismatches",
        corpus.cases.len(),
        full_cases,
        faces_checked,
        ck.exact_blocks,
        ck.tolerance_blocks,
        ck.unverifiable_blocks,
        ck.mismatch_count
    );
    for m in &ck.mismatches {
        eprintln!("  {m}");
    }
    assert_eq!(
        ck.mismatch_count, 0,
        "mesher output differs from the recorded C++ corpus"
    );
    if STRICT_PLATFORM && same_rsqrt {
        assert_eq!(
            ck.tolerance_blocks + ck.unverifiable_blocks,
            0,
            "every block must be exact on the recording platform"
        );
    }
}

#[test]
fn corpus_covers_the_parameter_space() {
    let corpus = load_corpus();
    assert!(corpus.cases.len() >= 800);
    let mut profiles = std::collections::BTreeSet::new();
    let mut paths = std::collections::BTreeSet::new();
    let mut holes = std::collections::BTreeSet::new();
    let mut details = std::collections::BTreeSet::new();
    let (
        mut flex,
        mut sculpt,
        mut hollow,
        mut cut_s,
        mut cut_t,
        mut twist,
        mut taper,
        mut shear,
        mut radius,
        mut rev,
        mut skew,
    ) = (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    for c in &corpus.cases {
        profiles.insert(c.profile_params.curve & 0x0f);
        holes.insert(c.profile_params.curve & 0xf0);
        paths.insert(c.path_params.curve);
        details.insert(c.detail.to_bits());
        flex += (c.flags & FLAG_FLEX != 0) as i32;
        sculpt += (c.flags & FLAG_SCULPT != 0) as i32;
        hollow += (c.profile_params.hollow > 0.0) as i32;
        cut_s += (c.profile_params.begin > 0.0 || c.profile_params.end < 1.0) as i32;
        cut_t += (c.path_params.begin > 0.0 || c.path_params.end < 1.0) as i32;
        twist += (c.path_params.twist_begin != 0.0 || c.path_params.twist_end != 0.0) as i32;
        taper += (c.path_params.taper_x != 0.0 || c.path_params.taper_y != 0.0) as i32;
        shear += (c.path_params.shear_x != 0.0 || c.path_params.shear_y != 0.0) as i32;
        radius += (c.path_params.radius_offset != 0.0) as i32;
        rev += (c.path_params.revolutions != 1.0) as i32;
        skew += (c.path_params.skew != 0.0) as i32;
    }
    assert_eq!(profiles.len(), 6, "all six profile curves");
    assert_eq!(holes.len(), 4, "all four hole shapes");
    assert_eq!(
        paths.len(),
        5,
        "line, circle, circle2, test and flexible paths"
    );
    assert!(details.len() >= 5, "detail 0, 1, 1.5, 2.5 and 4");
    for (name, n) in [
        ("flexible", flex),
        ("sculpt", sculpt),
        ("hollow", hollow),
        ("profile cut", cut_s),
        ("path cut", cut_t),
        ("twist", twist),
        ("taper", taper),
        ("shear", shear),
        ("radius offset", radius),
        ("revolutions", rev),
        ("skew", skew),
    ] {
        assert!(n >= 8, "{name}: {n} cases");
    }
}
