//! The mesh repository thread's fetch planning: capability URLs, the policy
//! class of a byte-range request, and which range a header, LOD, skin,
//! decomposition or physics-shape request asks for, from the cache or the
//! server.

use super::header::{
    CACHE_PREAMBLE_SIZE, LARGE_MESH_FETCH_THRESHOLD, MAX_MESH_VERSION, MeshHeader,
};
use crate::{Uuid, uuid_to_string};

/// `constructUrl`'s result: the URL (empty when no capability) and the
/// legacy capability version (0 for ViewerAsset, 1 or 2 for the GetMesh
/// capabilities OpenSim regions offer).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshUrl {
    pub url: String,
    pub legacy_version: i32,
}

/// `constructUrl`: prefers the ViewerAsset capability, then GetMesh2, then
/// GetMesh (`legacy_pref_version` is the host's `mLegacyGetMeshVersion`).
pub fn construct_url(
    mesh_cap: &str,
    legacy_get_mesh1: &str,
    legacy_get_mesh2: &str,
    legacy_pref_version: i32,
    mesh_id: &Uuid,
) -> MeshUrl {
    let (mut url, version) = if !mesh_cap.is_empty() && legacy_pref_version == 0 {
        (mesh_cap.to_string(), 0)
    } else if !legacy_get_mesh2.is_empty() && legacy_pref_version > 1 {
        (legacy_get_mesh2.to_string(), 2)
    } else {
        (legacy_get_mesh1.to_string(), 1)
    };
    if !url.is_empty() {
        url.push_str("/?mesh_id=");
        url.push_str(&uuid_to_string(mesh_id));
    }
    MeshUrl {
        url,
        legacy_version: version,
    }
}

/// The HTTP policy class a mesh byte range goes to (`getByteRange`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyChoice {
    /// `mHttpPolicyClass` (ViewerAsset or GetMesh2).
    Mesh,
    /// `mHttpLegacyPolicyClass` (GetMesh).
    Legacy,
    /// `mHttpLargePolicyClass`.
    Large,
}

pub fn policy_for(len: u32, legacy_cap_version: i32) -> PolicyChoice {
    if len < LARGE_MESH_FETCH_THRESHOLD {
        if legacy_cap_version == 0 || legacy_cap_version == 2 {
            PolicyChoice::Mesh
        } else {
            PolicyChoice::Legacy
        }
    } else {
        PolicyChoice::Large
    }
}

/// A part of the asset to fetch: the byte range in the asset (`offset`
/// from the start of the asset, after the header) and whether the cache
/// is believed to hold it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartRequest {
    pub offset: i32,
    pub size: i32,
    pub in_cache: bool,
    /// The offset in the cache file (`offset + CACHE_PREAMBLE_SIZE`).
    pub disk_offset: i32,
}

fn part(header: &MeshHeader, part_offset: i32, size: i32, in_cache: bool) -> Option<PartRequest> {
    if header.header_size <= 0 {
        return None;
    }
    let offset = header.header_size + part_offset;
    if header.version <= MAX_MESH_VERSION && offset >= 0 && size > 0 {
        Some(PartRequest {
            offset,
            size,
            in_cache,
            disk_offset: offset + CACHE_PREAMBLE_SIZE,
        })
    } else {
        None
    }
}

/// `fetchMeshLOD`'s range for `lod`; `None` when the header has no such
/// LOD (the request is unavailable) or no header size yet.
pub fn lod_request(header: &MeshHeader, lod: usize) -> Option<PartRequest> {
    part(
        header,
        header.lod_offset[lod],
        header.lod_size[lod],
        header.lod_in_cache[lod],
    )
}

/// `fetchMeshSkinInfo`'s range.
pub fn skin_request(header: &MeshHeader) -> Option<PartRequest> {
    part(
        header,
        header.skin_offset,
        header.skin_size,
        header.skin_in_cache,
    )
}

/// `fetchMeshDecomposition`'s range (the convex decomposition).
pub fn decomposition_request(header: &MeshHeader) -> Option<PartRequest> {
    part(
        header,
        header.physics_convex_offset,
        header.physics_convex_size,
        header.physics_convex_in_cache,
    )
}

/// `fetchMeshPhysicsShape`'s range (the physics mesh).
pub fn physics_shape_request(header: &MeshHeader) -> Option<PartRequest> {
    part(
        header,
        header.physics_mesh_offset,
        header.physics_mesh_size,
        header.physics_mesh_in_cache,
    )
}

/// Whether a cached part can be read: the file is large enough for it.
pub fn cached_part_available(req: &PartRequest, file_size: i32) -> bool {
    req.in_cache && file_size >= req.disk_offset + req.size
}

/// The HTTP response overlap check of `LLMeshHandlerBase::onCompleted`:
/// given the request's origin and the response's range (`offset` is 0 for
/// a 200, the `Content-Range` offset or the request's own for a 206 without
/// one), the body offset at which the requested bytes start, or `None` when
/// the response does not cover the request's origin.
pub fn response_body_offset(
    request_offset: u32,
    response_offset: u32,
    data_size: usize,
) -> Option<usize> {
    if response_offset > request_offset
        || (response_offset as u64 + data_size as u64) <= request_offset as u64
        || (request_offset - response_offset) as u64 >= data_size as u64
    {
        return None;
    }
    Some((request_offset - response_offset) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_policies() {
        let id = crate::uuid_from_string("01234567-89ab-cdef-1032-547698badcfe").unwrap();
        let u = construct_url("http://cap/va", "http://cap/gm1", "http://cap/gm2", 0, &id);
        assert_eq!(
            u,
            MeshUrl {
                url: "http://cap/va/?mesh_id=01234567-89ab-cdef-1032-547698badcfe".into(),
                legacy_version: 0
            }
        );
        let u = construct_url("", "http://cap/gm1", "http://cap/gm2", 2, &id);
        assert_eq!(u.legacy_version, 2);
        assert!(u.url.starts_with("http://cap/gm2/?mesh_id="));
        let u = construct_url("", "http://cap/gm1", "", 2, &id);
        assert_eq!(u.legacy_version, 1);
        let u = construct_url("", "", "", 0, &id);
        assert!(u.url.is_empty());
        assert_eq!(policy_for(100, 0), PolicyChoice::Mesh);
        assert_eq!(policy_for(100, 1), PolicyChoice::Legacy);
        assert_eq!(policy_for(1 << 21, 0), PolicyChoice::Large);
    }

    #[test]
    fn part_requests() {
        let mut h = MeshHeader::default();
        h.version = 1;
        h.header_size = 300;
        h.lod_offset = [0, 100, 300, 700];
        h.lod_size = [100, 200, 400, 0];
        h.lod_in_cache = [true, false, false, false];
        h.skin_offset = 1100;
        h.skin_size = 50;
        let r = lod_request(&h, 0).unwrap();
        assert_eq!(
            r,
            PartRequest {
                offset: 300,
                size: 100,
                in_cache: true,
                disk_offset: 312
            }
        );
        assert!(cached_part_available(&r, 412));
        assert!(!cached_part_available(&r, 411));
        assert_eq!(lod_request(&h, 3), None);
        assert_eq!(skin_request(&h).unwrap().offset, 1400);
        assert_eq!(decomposition_request(&h), None);
        h.header_size = 0;
        assert_eq!(lod_request(&h, 0), None);
    }

    #[test]
    fn overlap() {
        assert_eq!(response_body_offset(4096, 4096, 100), Some(0));
        assert_eq!(response_body_offset(4096, 0, 5000), Some(4096));
        assert_eq!(response_body_offset(4096, 0, 4096), None);
        assert_eq!(response_body_offset(4096, 5000, 100), None);
    }
}
