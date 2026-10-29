//! The mesh unpacking corpus recorded from the viewer's C++ before the port
//! (`indra/llprimitive/tests/fixtures/llmeshunpack_corpus.vksa` in the
//! viewer, copied here): mesh assets packed by `LLModel::writeModel` and,
//! per LOD, every face buffer as `LLVolume::unpackVolumeFaces` left it
//! before the vertex cache optimisation (the "raw" faces), plus the skin
//! info and the decomposition. Everything the crate produces must be bit
//! identical.

use std::path::PathBuf;

use vkstorm_assets::mesh::llsd::parse_binary;
use vkstorm_assets::mesh::skin::{Decomposition, SkinInfo};
use vkstorm_assets::mesh::unpack::{Face, unpack_volume_faces};

const FIXTURE_MAGIC: u32 = 0x4853_4d56;
const FIXTURE_VERSION: u32 = 2;
pub const NUM_PARTS: usize = 5;
const FULL_BUFFER_LIMIT: i32 = 300;

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> u8 {
        let v = self.b[self.pos];
        self.pos += 1;
        v
    }
    fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes(self.b[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        v
    }
    fn i32(&mut self) -> i32 {
        self.u32() as i32
    }
    fn u64(&mut self) -> u64 {
        let v = u64::from_le_bytes(self.b[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        v
    }
    fn f32(&mut self) -> f32 {
        f32::from_bits(self.u32())
    }
    fn bytes(&mut self) -> Vec<u8> {
        let n = self.u32() as usize;
        let v = self.b[self.pos..self.pos + n].to_vec();
        self.pos += n;
        v
    }
    fn str(&mut self) -> String {
        String::from_utf8_lossy(&self.bytes()).into_owned()
    }
}

#[derive(Debug, PartialEq)]
pub struct FaceRecord {
    pub num_vertices: i32,
    pub num_indices: i32,
    pub has_weights: bool,
    pub has_tangents: bool,
    pub hash_positions: u64,
    pub hash_normals: u64,
    pub hash_texcoords: u64,
    pub hash_indices: u64,
    pub hash_weights: u64,
    pub hash_tangents: u64,
    pub extents: [f32; 8],
    pub tc_extents: [f32; 4],
    pub normalized_scale: [f32; 3],
    pub positions: Vec<u8>,
    pub normals: Vec<u8>,
    pub texcoords: Vec<u8>,
    pub indices: Vec<u8>,
    pub weights: Vec<u8>,
    pub tangents: Vec<u8>,
}

fn read_face(r: &mut Reader) -> FaceRecord {
    let num_vertices = r.i32();
    let num_indices = r.i32();
    let has_weights = r.u8() != 0;
    let has_tangents = r.u8() != 0;
    let hash_positions = r.u64();
    let hash_normals = r.u64();
    let hash_texcoords = r.u64();
    let hash_indices = r.u64();
    let hash_weights = r.u64();
    let hash_tangents = r.u64();
    let mut extents = [0f32; 8];
    for v in extents.iter_mut() {
        *v = r.f32();
    }
    let mut tc_extents = [0f32; 4];
    for v in tc_extents.iter_mut() {
        *v = r.f32();
    }
    let mut normalized_scale = [0f32; 3];
    for v in normalized_scale.iter_mut() {
        *v = r.f32();
    }
    FaceRecord {
        num_vertices,
        num_indices,
        has_weights,
        has_tangents,
        hash_positions,
        hash_normals,
        hash_texcoords,
        hash_indices,
        hash_weights,
        hash_tangents,
        extents,
        tc_extents,
        normalized_scale,
        positions: r.bytes(),
        normals: r.bytes(),
        texcoords: r.bytes(),
        indices: r.bytes(),
        weights: r.bytes(),
        tangents: r.bytes(),
    }
}

fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 1469598103934665603;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

fn floats(v: &[[f32; 4]]) -> Vec<u8> {
    v.iter()
        .flat_map(|q| q.iter().flat_map(|f| f.to_le_bytes()))
        .collect()
}

fn floats2(v: &[[f32; 2]]) -> Vec<u8> {
    v.iter()
        .flat_map(|q| q.iter().flat_map(|f| f.to_le_bytes()))
        .collect()
}

/// The face as the C++ test recorded it from the unpacked `LLVolumeFace`.
fn record(face: &Face) -> FaceRecord {
    let n = face.num_vertices as usize;
    let positions = floats(&face.positions[..n.min(face.positions.len())]);
    let normals = floats(&face.normals[..n.min(face.normals.len())]);
    let texcoords = floats2(&face.texcoords[..n.min(face.texcoords.len())]);
    let indices: Vec<u8> = face.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    let weights = face
        .weights
        .as_ref()
        .map(|w| floats(&w[..n.min(w.len())]))
        .unwrap_or_default();
    let full = face.num_vertices <= FULL_BUFFER_LIMIT;
    let have_verts = face.num_vertices > 0 && !face.positions.is_empty();
    let have_idx = face.num_indices > 0 && !face.indices.is_empty();
    FaceRecord {
        num_vertices: face.num_vertices,
        num_indices: face.num_indices,
        has_weights: face.weights.is_some(),
        has_tangents: false,
        hash_positions: if have_verts { fnv1a(&positions) } else { 0 },
        hash_normals: if have_verts { fnv1a(&normals) } else { 0 },
        hash_texcoords: if have_verts { fnv1a(&texcoords) } else { 0 },
        hash_indices: if have_idx { fnv1a(&indices) } else { 0 },
        hash_weights: if face.weights.is_some() && have_verts {
            fnv1a(&weights)
        } else {
            0
        },
        hash_tangents: 0,
        extents: [
            face.extents[0][0],
            face.extents[0][1],
            face.extents[0][2],
            face.extents[0][3],
            face.extents[1][0],
            face.extents[1][1],
            face.extents[1][2],
            face.extents[1][3],
        ],
        tc_extents: [
            face.tc_extents[0][0],
            face.tc_extents[0][1],
            face.tc_extents[1][0],
            face.tc_extents[1][1],
        ],
        normalized_scale: face.normalized_scale,
        positions: if full && have_verts {
            positions
        } else {
            Vec::new()
        },
        normals: if full && have_verts {
            normals
        } else {
            Vec::new()
        },
        texcoords: if full && have_verts {
            texcoords
        } else {
            Vec::new()
        },
        indices: if full && have_idx {
            indices
        } else {
            Vec::new()
        },
        weights: if full && face.weights.is_some() && have_verts {
            weights
        } else {
            Vec::new()
        },
        tangents: Vec::new(),
    }
}

#[derive(Debug, PartialEq)]
pub struct SkinRecord {
    present: bool,
    ok: bool,
    joint_names: Vec<String>,
    joint_nums: Vec<i32>,
    inv_bind: Vec<u8>,
    alt_bind: Vec<u8>,
    bind_shape: Vec<u8>,
    bind_pose: Vec<u8>,
    pelvis_offset: u32,
    lock_scale: bool,
}

fn read_skin(r: &mut Reader) -> SkinRecord {
    let present = r.u8() != 0;
    let ok = r.u8() != 0;
    let n = r.u32();
    let joint_names = (0..n).map(|_| r.str()).collect();
    let n = r.u32();
    let joint_nums = (0..n).map(|_| r.i32()).collect();
    let inv_bind = r.bytes();
    let alt_bind = r.bytes();
    let bind_shape = r.bytes();
    let bind_pose = r.bytes();
    let pelvis_offset = r.u32();
    let lock_scale = r.u8() != 0;
    let _hash = r.u64();
    SkinRecord {
        present,
        ok,
        joint_names,
        joint_nums,
        inv_bind,
        alt_bind,
        bind_shape,
        bind_pose,
        pelvis_offset,
        lock_scale,
    }
}

fn mats(m: &[[[f32; 4]; 4]]) -> Vec<u8> {
    m.iter()
        .flat_map(|x| {
            x.iter()
                .flat_map(|row| row.iter().flat_map(|f| f.to_le_bytes()))
        })
        .collect()
}

fn record_skin(s: &SkinInfo, present: bool, ok: bool) -> SkinRecord {
    SkinRecord {
        present,
        ok,
        joint_names: s
            .joint_names
            .iter()
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .collect(),
        joint_nums: s.joint_nums.clone(),
        inv_bind: mats(&s.inv_bind_matrix),
        alt_bind: mats(&s.alternate_bind_matrix),
        bind_shape: mats(std::slice::from_ref(&s.bind_shape_matrix)),
        bind_pose: mats(&s.bind_pose_matrix),
        pelvis_offset: s.pelvis_offset.to_bits(),
        lock_scale: s.lock_scale_if_joint_position,
    }
}

#[derive(Debug, PartialEq)]
pub struct DecompRecord {
    present: bool,
    ok: bool,
    hulls: Vec<Vec<u8>>,
    base_hull: Vec<u8>,
    base_mesh_cleared: bool,
}

fn read_decomp(r: &mut Reader) -> DecompRecord {
    let present = r.u8() != 0;
    let ok = r.u8() != 0;
    let n = r.u32();
    let hulls = (0..n).map(|_| r.bytes()).collect();
    let base_hull = r.bytes();
    let base_mesh_cleared = r.u8() != 0;
    DecompRecord {
        present,
        ok,
        hulls,
        base_hull,
        base_mesh_cleared,
    }
}

fn vec3s(v: &[[f32; 3]]) -> Vec<u8> {
    v.iter()
        .flat_map(|p| p.iter().flat_map(|f| f.to_le_bytes()))
        .collect()
}

fn record_decomp(d: &Decomposition, present: bool, ok: bool) -> DecompRecord {
    DecompRecord {
        present,
        ok,
        hulls: d.hulls.iter().map(|h| vec3s(h)).collect(),
        base_hull: vec3s(&d.base_hull),
        base_mesh_cleared: d.base_hull_mesh_cleared,
    }
}

pub struct Case {
    pub name: String,
    pub sculpt_type: u8,
    pub asset: Vec<u8>,
    pub raw_faces: [Vec<FaceRecord>; NUM_PARTS],
    pub present: [bool; NUM_PARTS],
    pub ok: [bool; NUM_PARTS],
    pub skin: SkinRecord,
    pub decomp: DecompRecord,
}

pub fn read_corpus(bytes: &[u8]) -> Vec<Case> {
    let mut r = Reader { b: bytes, pos: 0 };
    assert_eq!(r.u32(), FIXTURE_MAGIC, "fixture magic");
    assert_eq!(r.u32(), FIXTURE_VERSION, "fixture version");
    let n = r.u32();
    let mut cases = Vec::new();
    for _ in 0..n {
        let name = r.str();
        let sculpt_type = r.u8();
        let asset = r.bytes();
        let mut raw_faces: [Vec<FaceRecord>; NUM_PARTS] = Default::default();
        let mut present = [false; NUM_PARTS];
        let mut ok = [false; NUM_PARTS];
        for p in 0..NUM_PARTS {
            present[p] = r.u8() != 0;
            ok[p] = r.u8() != 0;
            let faces = r.u32();
            for _ in 0..faces {
                let _ = read_face(&mut r); // after cacheOptimize: the viewer compares these
            }
            let raw = r.u32();
            for _ in 0..raw {
                raw_faces[p].push(read_face(&mut r));
            }
        }
        let skin = read_skin(&mut r);
        let decomp = read_decomp(&mut r);
        cases.push(Case {
            name,
            sculpt_type,
            asset,
            raw_faces,
            present,
            ok,
            skin,
            decomp,
        });
    }
    assert_eq!(r.pos, bytes.len(), "fixture read completely");
    cases
}

/// The header of a packed asset: `(header_size, parts[(offset, size)], skin, convex)`.
pub fn parse_asset_header(
    asset: &[u8],
) -> (
    usize,
    [Option<(i32, i32)>; NUM_PARTS],
    Option<(i32, i32)>,
    Option<(i32, i32)>,
) {
    use vkstorm_assets::mesh::llsd::{as_integer, get, has};
    let (value, consumed) =
        parse_binary(asset, asset.len() as i64, -1).expect("asset header parses");
    const NAMES: [&str; NUM_PARTS] = [
        "lowest_lod",
        "low_lod",
        "medium_lod",
        "high_lod",
        "physics_mesh",
    ];
    let mut parts = [None; NUM_PARTS];
    for (i, name) in NAMES.iter().enumerate() {
        if has(&value, name) {
            parts[i] = Some((
                as_integer(get(get(&value, name), "offset")),
                as_integer(get(get(&value, name), "size")),
            ));
        }
    }
    let skin = has(&value, "skin").then(|| {
        (
            as_integer(get(get(&value, "skin"), "offset")),
            as_integer(get(get(&value, "skin"), "size")),
        )
    });
    let convex = has(&value, "physics_convex").then(|| {
        (
            as_integer(get(get(&value, "physics_convex"), "offset")),
            as_integer(get(get(&value, "physics_convex"), "size")),
        )
    });
    (consumed, parts, skin, convex)
}

pub fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/llmeshunpack_corpus.vksa")
}

#[test]
fn corpus_unpacks_bit_identically() {
    let bytes = std::fs::read(fixture_path()).expect("corpus fixture present");
    let cases = read_corpus(&bytes);
    assert!(!cases.is_empty());
    let mut problems = Vec::new();
    let mut faces_compared = 0usize;
    for c in &cases {
        let (header_size, parts, skin, convex) = parse_asset_header(&c.asset);
        for p in 0..NUM_PARTS {
            let Some((offset, size)) = parts[p] else {
                assert!(
                    !c.present[p],
                    "{}: part {} unexpectedly recorded",
                    c.name, p
                );
                continue;
            };
            if offset < 0 || size <= 0 {
                assert!(!c.present[p]);
                continue;
            }
            let start = header_size + offset as usize;
            let block = &c.asset[start..start + size as usize];
            let sculpt = if p == 4 { 5 } else { c.sculpt_type };
            match unpack_volume_faces(block, sculpt) {
                Ok(faces) => {
                    if !c.ok[p] {
                        problems.push(format!("{}/{}: unpacked but the C++ failed", c.name, p));
                        continue;
                    }
                    if faces.len() != c.raw_faces[p].len() {
                        problems.push(format!(
                            "{}/{}: {} faces, expected {}",
                            c.name,
                            p,
                            faces.len(),
                            c.raw_faces[p].len()
                        ));
                        continue;
                    }
                    for (i, (face, want)) in faces.iter().zip(c.raw_faces[p].iter()).enumerate() {
                        let got = record(face);
                        faces_compared += 1;
                        if got != *want {
                            let mut what = Vec::new();
                            if got.num_vertices != want.num_vertices
                                || got.num_indices != want.num_indices
                            {
                                what.push("counts");
                            }
                            if got.hash_positions != want.hash_positions {
                                what.push("positions");
                            }
                            if got.hash_normals != want.hash_normals {
                                what.push("normals");
                            }
                            if got.hash_texcoords != want.hash_texcoords {
                                what.push("texcoords");
                            }
                            if got.hash_indices != want.hash_indices {
                                what.push("indices");
                            }
                            if got.hash_weights != want.hash_weights
                                || got.has_weights != want.has_weights
                            {
                                what.push("weights");
                            }
                            if got.extents != want.extents {
                                what.push("extents");
                            }
                            if got.tc_extents != want.tc_extents {
                                what.push("tc_extents");
                            }
                            if got.normalized_scale != want.normalized_scale {
                                what.push("normalized_scale");
                            }
                            problems.push(format!(
                                "{}/{} face {}: {}",
                                c.name,
                                p,
                                i,
                                what.join(" ")
                            ));
                        }
                    }
                }
                Err(e) => {
                    if c.ok[p] {
                        problems.push(format!("{}/{}: failed to unpack: {:?}", c.name, p, e));
                    }
                }
            }
        }
        match skin {
            Some((offset, size)) if offset >= 0 && size > 0 => {
                let start = header_size + offset as usize;
                let block = &c.asset[start..start + size as usize];
                let got = match SkinInfo::from_zipped(block) {
                    Ok(s) => record_skin(&s, true, true),
                    Err(_) => record_skin(&SkinInfo::default(), true, false),
                };
                if got != c.skin {
                    problems.push(format!("{}: skin info differs", c.name));
                }
            }
            _ => assert!(!c.skin.present, "{}: skin unexpectedly recorded", c.name),
        }
        match convex {
            Some((offset, size)) if offset >= 0 && size > 0 => {
                let start = header_size + offset as usize;
                let block = &c.asset[start..start + size as usize];
                let got = match Decomposition::from_zipped(block) {
                    Ok(d) => record_decomp(&d, true, true),
                    Err(_) => record_decomp(&Decomposition::default(), true, false),
                };
                if got != c.decomp {
                    problems.push(format!("{}: decomposition differs", c.name));
                }
            }
            _ => assert!(
                !c.decomp.present,
                "{}: decomposition unexpectedly recorded",
                c.name
            ),
        }
    }
    for p in &problems {
        eprintln!("PARITY: {p}");
    }
    eprintln!(
        "compared {} cases, {} raw faces, {} differences",
        cases.len(),
        faces_compared,
        problems.len()
    );
    assert!(problems.is_empty(), "{} differences", problems.len());
}
