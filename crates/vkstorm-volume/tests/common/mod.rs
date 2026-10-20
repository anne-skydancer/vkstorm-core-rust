//! The recorded corpus (`fixtures/llvolume_corpus.vksv`, written by the
//! viewer's `INTEGRATION_TEST_llvolume` from the original C++ mesher) and
//! the Rust build of each of its cases, shared by the parity test and the
//! bench.
#![allow(clippy::chunks_exact_to_as_chunks)] // fixed-size record parsing
#![allow(dead_code)]

use vkstorm_volume::params::*;
use vkstorm_volume::*;

pub const CORPUS: &[u8] = include_bytes!("../fixtures/llvolume_corpus.vksv");

pub const FIXTURE_MAGIC: u32 = 0x5653_4B56; // "VKSV"
pub const FIXTURE_VERSION: u32 = 1;

pub const FLAG_FULL: u32 = 1;
pub const FLAG_FLEX: u32 = 2;
pub const FLAG_SCULPT: u32 = 4;
pub const FLAG_TANGENTS: u32 = 8;

pub const PATHPT_FLOATS: usize = 21;

pub const RSQRT_SAMPLES: [f32; 32] = [
    1., 2., 3., 0.5, 0.25, 0.1, 10., 100., 1e-3, 1e3, 7.77, 0.333, 12345.678, 1.5e-5, 3.3e6,
    0.98765, 0.0625, 0.75, 1.0001, 0.9999, 42., 0.01234, 5.5e-2, 8.125, 1.7e-8, 2.5e9, 0.6, 0.7,
    0.8, 0.9, 1.1, 1.2,
];

pub fn rsqrt_signature() -> [u32; 32] {
    let mut out = [0u32; 32];
    for (o, s) in out.iter_mut().zip(RSQRT_SAMPLES.iter()) {
        *o = simd::rsqrt(*s).to_bits();
    }
    out
}

pub fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 1469598103934665603;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

pub struct Reader<'a> {
    pub p: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(p: &'a [u8]) -> Self {
        Reader { p, pos: 0 }
    }
    pub fn bytes(&mut self, n: usize) -> &'a [u8] {
        let s = &self.p[self.pos..self.pos + n];
        self.pos += n;
        s
    }
    pub fn u8(&mut self) -> u8 {
        self.bytes(1)[0]
    }
    pub fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.bytes(2).try_into().unwrap())
    }
    pub fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.bytes(4).try_into().unwrap())
    }
    pub fn i32(&mut self) -> i32 {
        i32::from_le_bytes(self.bytes(4).try_into().unwrap())
    }
    pub fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.bytes(8).try_into().unwrap())
    }
    pub fn f32(&mut self) -> f32 {
        f32::from_le_bytes(self.bytes(4).try_into().unwrap())
    }
    pub fn str(&mut self) -> String {
        let n = self.u16() as usize;
        String::from_utf8_lossy(self.bytes(n)).into_owned()
    }
    pub fn f32s(&mut self, n: usize) -> Vec<f32> {
        self.bytes(n * 4)
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }
    pub fn block(&mut self) -> Block {
        let hash = self.u64();
        let has_data = self.u8() != 0;
        let data = if has_data {
            let len = self.u32() as usize;
            Some(self.bytes(len).to_vec())
        } else {
            None
        };
        Block { hash, data }
    }
}

#[derive(Clone, Debug)]
pub struct Block {
    pub hash: u64,
    pub data: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct RecordedFace {
    pub id: i32,
    pub type_mask: u32,
    pub begin_s: i32,
    pub num_s: i32,
    pub begin_t: i32,
    pub num_t: i32,
    pub num_vertices: u32,
    pub num_indices: u32,
    pub extents: [f32; 8],
    pub center: [f32; 4],
    pub tc_extents: [f32; 4],
    pub positions: Block,
    pub normals: Block,
    pub texcoords: Block,
    pub indices: Block,
    pub tangents: Block,
    pub normals_after: Block,
}

/// One recorded case: its inputs and the C++ outputs.
#[derive(Clone, Debug)]
pub struct Case {
    pub id: u32,
    pub name: String,
    pub profile_params: ProfileParams,
    pub path_params: PathParams,
    pub sculpt_type: u8,
    pub detail: f32,
    pub flags: u32,
    pub face_mask: u32,
    pub lod_counts: [i32; 4],
    pub flex_path: Vec<f32>,
    pub sculpt_size: (u16, u16),
    pub profile_count: u32,
    pub profile_total: i32,
    pub profile_total_out: i32,
    pub profile_open: bool,
    pub profile_concave: bool,
    pub profile_points: Block,
    pub profile_face_count: u32,
    pub profile_faces: Block,
    pub path_count: u32,
    pub path_open: bool,
    pub path_step: f32,
    pub path_points: Block,
    pub mesh_count: u32,
    pub mesh: Block,
    pub faces: Vec<RecordedFace>,
}

pub struct Corpus {
    pub rsqrt_signature: [u32; 32],
    pub cases: Vec<Case>,
}

pub fn load_corpus() -> Corpus {
    let mut r = Reader::new(CORPUS);
    assert_eq!(r.u32(), FIXTURE_MAGIC, "fixture magic");
    assert_eq!(r.u32(), FIXTURE_VERSION, "fixture version");
    let n = r.u32();
    assert_eq!(n, 32);
    let mut sig = [0u32; 32];
    for s in sig.iter_mut() {
        *s = r.u32();
    }
    let count = r.u32();
    let mut cases = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let id = r.u32();
        let name = r.str();
        let profile_params = ProfileParams {
            curve: r.u8(),
            begin: r.f32(),
            end: r.f32(),
            hollow: r.f32(),
        };
        let path_params = PathParams {
            curve: r.u8(),
            begin: r.f32(),
            end: r.f32(),
            scale_x: r.f32(),
            scale_y: r.f32(),
            shear_x: r.f32(),
            shear_y: r.f32(),
            twist_begin: r.f32(),
            twist_end: r.f32(),
            radius_offset: r.f32(),
            taper_x: r.f32(),
            taper_y: r.f32(),
            revolutions: r.f32(),
            skew: r.f32(),
        };
        let sculpt_type = r.u8();
        let detail = r.f32();
        let flags = r.u32();
        let face_mask = r.u32();
        let lod_counts = [r.i32(), r.i32(), r.i32(), r.i32()];
        let mut flex_path = Vec::new();
        if flags & FLAG_FLEX != 0 {
            let n = r.u32() as usize;
            flex_path = r.f32s(n * PATHPT_FLOATS);
        }
        let mut sculpt_size = (0u16, 0u16);
        if flags & FLAG_SCULPT != 0 {
            sculpt_size = (r.u16(), r.u16());
            let _map = r.bytes(sculpt_size.0 as usize * sculpt_size.1 as usize * 3);
        }
        let profile_count = r.u32();
        let profile_total = r.i32();
        let profile_total_out = r.i32();
        let profile_open = r.u8() != 0;
        let profile_concave = r.u8() != 0;
        let profile_points = r.block();
        let profile_face_count = r.u32();
        let profile_faces = r.block();
        let path_count = r.u32();
        let path_open = r.u8() != 0;
        let path_step = r.f32();
        let path_points = r.block();
        let mesh_count = r.u32();
        let mesh = r.block();
        let face_count = r.u32();
        let mut faces = Vec::with_capacity(face_count as usize);
        for _ in 0..face_count {
            let id = r.i32();
            let type_mask = r.u32();
            let begin_s = r.i32();
            let num_s = r.i32();
            let begin_t = r.i32();
            let num_t = r.i32();
            let num_vertices = r.u32();
            let num_indices = r.u32();
            let e = r.f32s(8);
            let c = r.f32s(4);
            let t = r.f32s(4);
            faces.push(RecordedFace {
                id,
                type_mask,
                begin_s,
                num_s,
                begin_t,
                num_t,
                num_vertices,
                num_indices,
                extents: e.try_into().unwrap(),
                center: c.try_into().unwrap(),
                tc_extents: t.try_into().unwrap(),
                positions: r.block(),
                normals: r.block(),
                texcoords: r.block(),
                indices: r.block(),
                tangents: r.block(),
                normals_after: r.block(),
            });
        }
        cases.push(Case {
            id,
            name,
            profile_params,
            path_params,
            sculpt_type,
            detail,
            flags,
            face_mask,
            lod_counts,
            flex_path,
            sculpt_size,
            profile_count,
            profile_total,
            profile_total_out,
            profile_open,
            profile_concave,
            profile_points,
            profile_face_count,
            profile_faces,
            path_count,
            path_open,
            path_step,
            path_points,
            mesh_count,
            mesh,
            faces,
        });
    }
    assert_eq!(r.pos, CORPUS.len(), "fixture read completely");
    Corpus {
        rsqrt_signature: sig,
        cases,
    }
}

/// A built face: the buffers as the viewer would hold them.
pub struct BuiltFace {
    pub desc: FaceDesc,
    pub kind: FaceKind,
    pub num_vertices: i32,
    pub num_indices: i32,
    pub positions: Vec<V4>,
    pub normals: Vec<V4>,
    pub texcoords: Vec<[f32; 2]>,
    pub indices: Vec<u16>,
    pub result: FaceResult,
    pub tangents: Vec<V4>,
    pub normals_after: Vec<V4>,
}

pub struct Built {
    pub profile: Profile,
    pub path: Path,
    pub mesh: Vec<V4>,
    pub faces: Vec<BuiltFace>,
    pub face_mask: u32,
}

pub fn bytes_of<T: Copy>(v: &[T]) -> &[u8] {
    // SAFETY: plain-data arrays of f32/u16 with no padding.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

pub fn mesh_from_block(block: &Block) -> Vec<V4> {
    let data = block.data.as_ref().expect("sculpt mesh recorded with data");
    data.chunks_exact(16)
        .map(|c| {
            V4([
                f32::from_le_bytes(c[0..4].try_into().unwrap()),
                f32::from_le_bytes(c[4..8].try_into().unwrap()),
                f32::from_le_bytes(c[8..12].try_into().unwrap()),
                f32::from_le_bytes(c[12..16].try_into().unwrap()),
            ])
        })
        .collect()
}

/// Builds a case exactly as the viewer does through the FFI: path, profile,
/// mesh sweep (or the recorded sculpt mesh), each face, then its tangents.
pub fn build_case(c: &Case, with_tangents: bool) -> Built {
    let pp = &c.profile_params;
    let pa = &c.path_params;
    let split = split_for(pp, pa, c.detail);

    let mut path = Path::default();
    let mut profile = Profile::default();

    let mesh: Vec<V4> = if c.flags & FLAG_SCULPT != 0 {
        let (s_req, t_req) = sculpt::mesh_resolution(c.sculpt_size.0, c.sculpt_size.1, c.detail);
        generate_path(pa, c.detail, 0, true, s_req, &mut path);
        generate_profile(pp, path.open, c.detail, 0, true, t_req, &mut profile);
        mesh_from_block(&c.mesh)
    } else if c.flags & FLAG_FLEX != 0 {
        // LLDynamicPath: the simulation fills the points; the path is open.
        path.open = true;
        for rec in c.flex_path.chunks_exact(PATHPT_FLOATS) {
            let mut pt = PathPt::default();
            for r in 0..3 {
                pt.rot[r] = [rec[r * 4], rec[r * 4 + 1], rec[r * 4 + 2], rec[r * 4 + 3]];
            }
            pt.rot[3] = [0.0, 0.0, 0.0, 1.0];
            pt.pos = [rec[12], rec[13], rec[14], rec[15]];
            pt.scale = [rec[16], rec[17], rec[18], rec[19]];
            pt.tex_t = rec[20];
            path.points.push(pt);
        }
        generate_profile(pp, true, c.detail, split, false, 0, &mut profile);
        let mut m = vec![V4::ZERO; path.points.len() * profile.points.len()];
        sweep(&path.points, &profile.points, &mut m);
        m
    } else {
        generate_path(pa, c.detail, split, false, 0, &mut path);
        generate_profile(pp, path.open, c.detail, split, false, 0, &mut profile);
        let mut m = vec![V4::ZERO; path.points.len() * profile.points.len()];
        sweep(&path.points, &profile.points, &mut m);
        m
    };

    let face_mask = profile.faces.iter().fold(0u32, |m, f| m | f.face_id as u32);
    let descs = face_descriptors(&profile, path.points.len() as i32, pp.hollow);

    let mut faces = Vec::with_capacity(descs.len());
    for desc in descs {
        let fp = FaceParams {
            mesh: &mesh,
            profile: &profile.points,
            profile_total: profile.total,
            profile_open: profile.open,
            path: &path.points,
            path_open: path.open,
            face: desc,
            profile_curve: pp.curve,
            path_curve: pa.curve,
            sculpt_type: c.sculpt_type,
            path_begin: pa.begin,
            path_end: pa.end,
        };
        let (kind, nv, ni) = face_sizes(&fp);
        let nvu = nv.max(0) as usize;
        let niu = ni.max(0) as usize;
        let mut positions = vec![V4::ZERO; nvu];
        let mut normals = vec![V4::ZERO; nvu];
        let mut texcoords = vec![[0.0f32; 2]; nvu + (nvu & 1)];
        let mut indices = vec![0u16; niu];
        let mut result = FaceResult::default();
        {
            let mut bufs = FaceBuffers {
                positions: &mut positions,
                normals: &mut normals,
                texcoords: &mut texcoords,
                indices: &mut indices,
                write_indices: true,
            };
            build_face(&fp, &mut bufs, &mut result);
        }
        let mut tangents = Vec::new();
        let mut normals_after = Vec::new();
        if with_tangents {
            tangents = vec![V4::ZERO; nvu];
            normals_after = normals.clone();
            calculate_tangents(
                &positions,
                &mut normals_after,
                &texcoords,
                &indices,
                &mut tangents,
            );
        }
        faces.push(BuiltFace {
            desc,
            kind,
            num_vertices: nv,
            num_indices: ni,
            positions,
            normals,
            texcoords,
            indices,
            result,
            tangents,
            normals_after,
        });
    }

    Built {
        profile,
        path,
        mesh,
        faces,
        face_mask,
    }
}

/// The canonical 84-byte path point records the fixture uses.
pub fn path_canonical(path: &Path) -> Vec<f32> {
    let mut out = Vec::with_capacity(path.points.len() * PATHPT_FLOATS);
    for pt in &path.points {
        for r in 0..3 {
            out.extend_from_slice(&pt.rot[r]);
        }
        out.extend_from_slice(&pt.pos);
        out.extend_from_slice(&pt.scale);
        out.push(pt.tex_t);
    }
    out
}

/// The canonical 16-byte profile face records the fixture uses.
pub fn profile_faces_canonical(profile: &Profile) -> Vec<u8> {
    let mut out = Vec::with_capacity(profile.faces.len() * 16);
    for f in &profile.faces {
        out.extend_from_slice(&f.index.to_le_bytes());
        out.extend_from_slice(&f.count.to_le_bytes());
        out.extend_from_slice(&f.scale_u.to_le_bytes());
        out.push(f.cap);
        out.push(f.flat);
        out.extend_from_slice(&f.face_id.to_le_bytes());
    }
    out
}

/// `LLVolume::getLoDTriangleCounts`.
pub fn lod_triangle_counts(pp: &ProfileParams, pa: &PathParams) -> [i32; 4] {
    let details = [1.0f32, 1.5, 2.5, 4.0];
    let mut out = [0i32; 4];
    for (o, d) in out.iter_mut().zip(details) {
        let path_points = path_num_points(pa, d);
        let profile_points = profile_num_points(pp, false, d, 0, false, 0);
        *o = (profile_points - 1) * 2 * (path_points - 1) + profile_points * 2;
    }
    out
}
