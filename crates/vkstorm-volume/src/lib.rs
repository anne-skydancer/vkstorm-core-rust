//! The parametric prim mesher of the Second Life viewer, in Rust.
//!
//! This crate reproduces `LLProfile`, `LLPath`, `LLVolume::generate` and the
//! `LLVolumeFace` builders of the viewer's `llvolume.cpp` operation for
//! operation, so that its output is bit-identical to the C++ mesher on the
//! same platform: positions, normals, tangents, texture coordinates, indices,
//! extents and centres. The viewer calls it through `vkstorm-ffi` and keeps
//! `LLVolume`/`LLVolumeFace` as the in-memory representation; the face
//! builders write straight into the caller's buffers.
//!
//! Floating-point rules that make the parity hold (see `doc/rust_port.md` in
//! the viewer): the operation order of the C++ is kept, `lerp` is MSVC's
//! `std::lerp` (an `fma` at its core), `normalize3fast` is `_mm_rsqrt_ps`, and
//! `sin`/`cos` come from the platform's libm as in the C++.
//!
//! Sculpted prims and mesh assets are the next rung: the face builder already
//! takes the mesh grid from the caller and applies the sculpt stitching rules,
//! and [`sculpt::mesh_resolution`] is here for them.

// The code mirrors llvolume.cpp operation for operation; these lints would
// have it drift from the C++ it must match (its constant tables, its
// `a*b - a*b` w lane, its LLVector4a method names and index loops).
#![allow(
    clippy::approx_constant,
    clippy::eq_op,
    clippy::should_implement_trait,
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::manual_slice_size_calculation
)]

pub mod face;
pub mod params;
pub mod path;
pub mod profile;
pub mod sculpt;
pub mod simd;
pub mod sweep;
pub mod tangent;

pub use face::{
    FaceBuffers, FaceDesc, FaceKind, FaceParams, FaceResult, build_face, face_descriptors,
    face_kind, face_sizes,
};
pub use params::{PathParams, ProfileParams};
pub use path::{Path, PathPt, generate_path, path_num_points};
pub use profile::{Profile, ProfileFace, generate_profile, profile_num_points};
pub use simd::V4;
pub use sweep::{sweep, sweep_uninit};
pub use tangent::calculate_tangents;

/// The edge split the viewer derives from the LOD detail in
/// `LLVolume::generate`: profile edges are tessellated so that twisted or
/// scaled sides do not stretch their triangles.
pub fn split_for(profile: &ProfileParams, path: &PathParams, detail: f32) -> i32 {
    let mut split = (detail * 0.66) as i32;
    if path.curve == params::PATH_LINE
        && (path.scale_x != 1.0 || path.scale_y != 1.0)
        && (profile.curve == params::PROFILE_SQUARE
            || profile.curve == params::PROFILE_ISOTRI
            || profile.curve == params::PROFILE_EQUALTRI
            || profile.curve == params::PROFILE_RIGHTTRI)
    {
        split = 0;
    }
    split
}
