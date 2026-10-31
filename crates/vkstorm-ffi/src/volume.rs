//! Rung: the parametric prim mesher via `vkstorm-volume` (`vks_volume_*`).
//!
//! `LLVolume`, `LLProfile`, `LLPath` and `LLVolumeFace` stay the C++
//! representation. Profile and path generation return pointers into
//! per-thread scratch storage that stays valid until the next generate call
//! on the same thread (the C++ copies the few hundred bytes out at once);
//! the mesh sweep, the face builders and the tangents write straight into
//! the caller's 16-byte aligned buffers. Panics are caught at the boundary
//! and reported as -1.

use std::cell::RefCell;
use std::mem::MaybeUninit;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;

use vkstorm_volume::{
    FaceBuffers, FaceDesc, FaceParams, FaceResult, Path, PathParams, PathPt, Profile, ProfileFace,
    ProfileParams, V4, build_face, calculate_tangents, face_sizes, generate_path, generate_profile,
    path_num_points, profile_num_points, sweep_uninit,
};

thread_local! {
    static PROFILE: RefCell<Profile> = RefCell::new(Profile::default());
    static PATH: RefCell<Path> = RefCell::new(Path::default());
}

/// The generated profile: pointers into this thread's scratch storage.
#[repr(C)]
pub struct VksProfileOut {
    pub points: *const V4,
    pub point_count: u32,
    pub faces: *const ProfileFace,
    pub face_count: u32,
    pub total: i32,
    pub total_out: i32,
    pub open: u8,
    pub concave: u8,
}

/// The generated path: pointers into this thread's scratch storage.
#[repr(C)]
pub struct VksPathOut {
    pub points: *const PathPt,
    pub count: u32,
    pub open: u8,
    pub step: f32,
}

/// `LLProfile::generate`: 1 when generated, 0 when the cut is invalid
/// (begin > end - 0.01; the profile is then empty), -1 on bad arguments or
/// an internal failure.
///
/// # Safety
/// `params` and `out` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_volume_profile_generate(
    params: *const ProfileParams,
    path_open: i32,
    detail: f32,
    split: i32,
    is_sculpted: i32,
    sculpt_size: i32,
    out: *mut VksProfileOut,
) -> i32 {
    if params.is_null() || out.is_null() {
        return -1;
    }
    let params = unsafe { *params };
    let r = catch_unwind(AssertUnwindSafe(|| {
        PROFILE.with(|cell| {
            let mut p = cell.borrow_mut();
            let ok = generate_profile(
                &params,
                path_open != 0,
                detail,
                split,
                is_sculpted != 0,
                sculpt_size,
                &mut p,
            );
            unsafe {
                *out = VksProfileOut {
                    points: p.points.as_ptr(),
                    point_count: p.points.len() as u32,
                    faces: p.faces.as_ptr(),
                    face_count: p.faces.len() as u32,
                    total: p.total,
                    total_out: p.total_out,
                    open: p.open as u8,
                    concave: p.concave as u8,
                };
            }
            ok as i32
        })
    }));
    r.unwrap_or(-1)
}

/// `LLPath::generate`: 1 when generated, -1 on bad arguments or an internal
/// failure.
///
/// # Safety
/// `params` and `out` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_volume_path_generate(
    params: *const PathParams,
    detail: f32,
    split: i32,
    is_sculpted: i32,
    sculpt_size: i32,
    out: *mut VksPathOut,
) -> i32 {
    if params.is_null() || out.is_null() {
        return -1;
    }
    let params = unsafe { *params };
    let r = catch_unwind(AssertUnwindSafe(|| {
        PATH.with(|cell| {
            let mut p = cell.borrow_mut();
            p.step = 1.0;
            generate_path(
                &params,
                detail,
                split,
                is_sculpted != 0,
                sculpt_size,
                &mut p,
            );
            unsafe {
                *out = VksPathOut {
                    points: p.points.as_ptr(),
                    count: p.points.len() as u32,
                    open: p.open as u8,
                    step: p.step,
                };
            }
            1
        })
    }));
    r.unwrap_or(-1)
}

/// `LLProfile::getNumPoints`.
///
/// # Safety
/// `params` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_volume_profile_num_points(
    params: *const ProfileParams,
    path_open: i32,
    detail: f32,
    split: i32,
    is_sculpted: i32,
    sculpt_size: i32,
) -> i32 {
    if params.is_null() {
        return 0;
    }
    let params = unsafe { *params };
    catch_unwind(AssertUnwindSafe(|| {
        profile_num_points(
            &params,
            path_open != 0,
            detail,
            split,
            is_sculpted != 0,
            sculpt_size,
        )
    }))
    .unwrap_or(0)
}

/// `LLPath::getNumPoints`.
///
/// # Safety
/// `params` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_volume_path_num_points(params: *const PathParams, detail: f32) -> i32 {
    if params.is_null() {
        return 0;
    }
    let params = unsafe { *params };
    catch_unwind(AssertUnwindSafe(|| path_num_points(&params, detail))).unwrap_or(0)
}

unsafe fn slice_or_empty<'a, T>(p: *const T, n: usize) -> &'a [T] {
    if p.is_null() || n == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(p, n) }
    }
}

unsafe fn slice_mut_or_empty<'a, T>(p: *mut T, n: usize) -> &'a mut [T] {
    if p.is_null() || n == 0 {
        &mut []
    } else {
        unsafe { slice::from_raw_parts_mut(p, n) }
    }
}

/// The mesh sweep of `LLVolume::generate`: writes `path_count *
/// profile_count` points into `dst`, which need not be initialised. 1 on
/// success.
///
/// # Safety
/// `path` and `profile` must point to initialised arrays of the given sizes;
/// `dst` must point to `path_count * profile_count` 16-byte aligned
/// vectors of writable storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_volume_sweep(
    path: *const PathPt,
    path_count: u32,
    profile: *const V4,
    profile_count: u32,
    dst: *mut V4,
) -> i32 {
    let path = unsafe { slice_or_empty(path, path_count as usize) };
    let profile = unsafe { slice_or_empty(profile, profile_count as usize) };
    let n = path_count as usize * profile_count as usize;
    let dst: &mut [MaybeUninit<V4>] = if dst.is_null() || n == 0 {
        &mut []
    } else {
        unsafe { slice::from_raw_parts_mut(dst as *mut MaybeUninit<V4>, n) }
    };
    catch_unwind(AssertUnwindSafe(|| {
        sweep_uninit(path, profile, dst);
        1
    }))
    .unwrap_or(-1)
}

/// Everything a face build reads, mirrored from `LLVolume` and the
/// `LLVolumeFace` being built.
#[repr(C)]
pub struct VksFaceParams {
    pub mesh: *const V4,
    pub mesh_count: u32,
    pub profile: *const V4,
    pub profile_count: u32,
    pub profile_total: i32,
    pub profile_open: u8,
    pub path: *const PathPt,
    pub path_count: u32,
    pub path_open: u8,
    pub id: i32,
    pub type_mask: u32,
    pub begin_s: i32,
    pub num_s: i32,
    pub begin_t: i32,
    pub num_t: i32,
    pub profile_curve: u8,
    pub path_curve: u8,
    pub sculpt_type: u8,
    pub path_begin: f32,
    pub path_end: f32,
}

/// The `LLVolumeFace` buffers: `num_vertices`/`num_indices` are the
/// allocated counts (`mNumVertices`, `mNumIndices`); the texture coordinate
/// buffer has the even-count padding slot the viewer allocates. The builder
/// forms slices over them, so they must be initialised memory: the viewer
/// zeroes freshly allocated storage (`LLVolumeFace::create`) and a partial
/// rebuild reuses storage it wrote before.
#[repr(C)]
pub struct VksFaceBuffers {
    pub positions: *mut V4,
    pub normals: *mut V4,
    pub texcoords: *mut [f32; 2],
    pub indices: *mut u16,
    pub num_vertices: i32,
    pub num_indices: i32,
    pub write_indices: u8,
}

/// What a face build sets besides the buffers.
#[repr(C)]
#[derive(Default)]
pub struct VksFaceResult {
    pub extents_min: [f32; 4],
    pub extents_max: [f32; 4],
    pub center: [f32; 4],
    pub tc_min: [f32; 2],
    pub tc_max: [f32; 2],
    pub tc_extents_set: u8,
}

unsafe fn face_params<'a>(p: &VksFaceParams) -> FaceParams<'a> {
    FaceParams {
        mesh: unsafe { slice_or_empty(p.mesh, p.mesh_count as usize) },
        profile: unsafe { slice_or_empty(p.profile, p.profile_count as usize) },
        profile_total: p.profile_total,
        profile_open: p.profile_open != 0,
        path: unsafe { slice_or_empty(p.path, p.path_count as usize) },
        path_open: p.path_open != 0,
        face: FaceDesc {
            id: p.id,
            type_mask: p.type_mask,
            begin_s: p.begin_s,
            num_s: p.num_s,
            begin_t: p.begin_t,
            num_t: p.num_t,
        },
        profile_curve: p.profile_curve,
        path_curve: p.path_curve,
        sculpt_type: p.sculpt_type,
        path_begin: p.path_begin,
        path_end: p.path_end,
    }
}

/// The builder a face takes and the vertex and index counts it produces:
/// 0 side, 1 cap, 2 uncut cube cap; -1 on bad arguments.
///
/// # Safety
/// `p`, `num_vertices` and `num_indices` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_volume_face_sizes(
    p: *const VksFaceParams,
    num_vertices: *mut i32,
    num_indices: *mut i32,
) -> i32 {
    if p.is_null() || num_vertices.is_null() || num_indices.is_null() {
        return -1;
    }
    let p = unsafe { &*p };
    catch_unwind(AssertUnwindSafe(|| {
        let fp = unsafe { face_params(p) };
        let (kind, nv, ni) = face_sizes(&fp);
        unsafe {
            *num_vertices = nv;
            *num_indices = ni;
        }
        kind as i32
    }))
    .unwrap_or(-1)
}

/// `LLVolumeFace::create`: builds the face into the buffers. 1 on success,
/// 0 when the inputs are degenerate (nothing written), -1 on bad arguments
/// or an internal failure.
///
/// # Safety
/// `p`, `b` and `out` must be valid pointers; the buffers in `b` must hold
/// the counts it states.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_volume_face_build(
    p: *const VksFaceParams,
    b: *const VksFaceBuffers,
    out: *mut VksFaceResult,
) -> i32 {
    if p.is_null() || b.is_null() || out.is_null() {
        return -1;
    }
    let p = unsafe { &*p };
    let b = unsafe { &*b };
    let nv = b.num_vertices.max(0) as usize;
    let ni = b.num_indices.max(0) as usize;
    catch_unwind(AssertUnwindSafe(|| {
        let fp = unsafe { face_params(p) };
        let mut bufs = FaceBuffers {
            positions: unsafe { slice_mut_or_empty(b.positions, nv) },
            normals: unsafe { slice_mut_or_empty(b.normals, nv) },
            texcoords: unsafe { slice_mut_or_empty(b.texcoords, nv + (nv & 1)) },
            indices: unsafe { slice_mut_or_empty(b.indices, ni) },
            write_indices: b.write_indices != 0,
        };
        let mut r = FaceResult::default();
        let ok = build_face(&fp, &mut bufs, &mut r);
        unsafe {
            *out = VksFaceResult {
                extents_min: r.extents_min.0,
                extents_max: r.extents_max.0,
                center: r.center.0,
                tc_min: r.tc_min,
                tc_max: r.tc_max,
                tc_extents_set: r.tc_extents_set as u8,
            };
        }
        ok as i32
    }))
    .unwrap_or(-1)
}

/// `LLVolumeFace::createTangents` / `LLCalculateTangentArray`: fills
/// `tangents` (handedness in w) and, with `normalize_normals`, normalises
/// the normals in place as `createTangents` does. 1 on success.
///
/// # Safety
/// All arrays must hold `num_vertices` (or `num_indices`) elements of
/// initialised memory (the viewer zeroes a fresh `tangents` buffer).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_volume_tangents(
    num_vertices: i32,
    positions: *const V4,
    normals: *mut V4,
    texcoords: *const [f32; 2],
    num_indices: i32,
    indices: *const u16,
    tangents: *mut V4,
    normalize_normals: i32,
) -> i32 {
    let nv = num_vertices.max(0) as usize;
    let ni = num_indices.max(0) as usize;
    let positions = unsafe { slice_or_empty(positions, nv) };
    let normals = unsafe { slice_mut_or_empty(normals, nv) };
    let texcoords = unsafe { slice_or_empty(texcoords, nv) };
    let indices = unsafe { slice_or_empty(indices, ni) };
    let tangents = unsafe { slice_mut_or_empty(tangents, nv) };
    catch_unwind(AssertUnwindSafe(|| {
        if normalize_normals != 0 {
            calculate_tangents(positions, normals, texcoords, indices, tangents);
        } else {
            for t in tangents.iter_mut() {
                *t = V4::ZERO;
            }
            vkstorm_volume::tangent::calculate_tangent_array(
                positions, normals, texcoords, indices, tangents,
            );
        }
        1
    }))
    .unwrap_or(-1)
}
