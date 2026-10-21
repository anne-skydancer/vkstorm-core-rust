//! The mesh repository's data side: the asset header and its disk-cache
//! preamble ([`header`]), the fetch planning ([`plan`]), the LLSD mesh LOD
//! unpacking into face buffers and the physics shape ([`unpack`]), the skin
//! info and the convex decomposition ([`skin`]), and the zlib-LLSD block
//! codec they share ([`inflate`]).
//!
//! What stays in the viewer: the repository thread and its queues, the HTTP
//! handlers, the cache file I/O (through [`crate::diskcache`]), the
//! `LLVolume`/`LLVolumeFace` storage and `cacheOptimize`, the rigging
//! updates that need the avatar, and the upload path.

pub mod header;
pub mod inflate;
pub mod llsd;
pub mod plan;
pub mod skin;
pub mod unpack;

pub use header::{header_received, HeaderReceipt, MeshHeader, ProcessingResult};
pub use inflate::{unzip_llsd, ZipResult};
pub use plan::{construct_url, lod_request, skin_request, MeshUrl, PartRequest, PolicyChoice};
pub use skin::{Decomposition, SkinInfo};
pub use unpack::{physics_shape, unpack_volume_faces, Face, UnpackError};
