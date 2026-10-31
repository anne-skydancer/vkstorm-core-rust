//! The mesh asset header (`LLMeshHeader`, `LLMeshRepoThread::headerReceived`)
//! and its disk-cache preamble.

use vkstorm_llsd::Value;

use super::inflate::strip_deprecated_header;
use super::llsd::{as_integer, as_uuid, get, has, is_map, is_uuid, parse_binary};
use crate::Uuid;

/// `LLModel::NUM_LODS` (the four render LODs).
pub const NUM_LODS: usize = 4;
/// `LLModel::LOD_PHYSICS`.
pub const LOD_PHYSICS: usize = 4;
/// `MAX_MESH_VERSION`.
pub const MAX_MESH_VERSION: i32 = 999;
/// `MESH_HEADER_SIZE`: the first fetch of a mesh asset.
pub const MESH_HEADER_SIZE: u32 = 4096;
/// `CACHE_PREAMBLE_VERSION` and `CACHE_PREAMBLE_SIZE` (version, header
/// size, flags).
pub const CACHE_PREAMBLE_VERSION: u32 = 1;
pub const CACHE_PREAMBLE_SIZE: i32 = 12;
/// `LARGE_MESH_FETCH_THRESHOLD`: requests at or above go to the large
/// policy class.
pub const LARGE_MESH_FETCH_THRESHOLD: u32 = 1 << 21;

const LOD_NAMES: [&str; NUM_LODS] = ["lowest_lod", "low_lod", "medium_lod", "high_lod"];

/// The preamble's flag bits as `LLMeshHeader::getFlags` lays them out:
/// `LLModel::NUM_LODS` is 5 (the physics mesh counts), so the render LODs
/// take bits 0..3, bit 4 is never set and the skin, convex and physics
/// mesh flags are bits 5, 6 and 7.
const CPP_NUM_LODS: u32 = 5;
const FLAG_SKIN: u32 = 1 << CPP_NUM_LODS;
const FLAG_PHYSCONVEX: u32 = 1 << (CPP_NUM_LODS + 1);
const FLAG_PHYSMESH: u32 = 1 << (CPP_NUM_LODS + 2);

/// `LLMeshHeader`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshHeader {
    pub version: i32,
    pub skin_offset: i32,
    pub skin_size: i32,
    pub skin_in_cache: bool,
    pub physics_convex_offset: i32,
    pub physics_convex_size: i32,
    pub physics_convex_in_cache: bool,
    pub physics_mesh_offset: i32,
    pub physics_mesh_size: i32,
    pub physics_mesh_in_cache: bool,
    pub lod_offset: [i32; NUM_LODS],
    pub lod_size: [i32; NUM_LODS],
    pub lod_in_cache: [bool; NUM_LODS],
    /// The bytes the binary LLSD header occupies in the asset.
    pub header_size: i32,
    pub not_found: bool,
    pub creator_id: Uuid,
}

impl Default for MeshHeader {
    fn default() -> Self {
        MeshHeader {
            version: -1,
            skin_offset: -1,
            skin_size: -1,
            skin_in_cache: false,
            physics_convex_offset: -1,
            physics_convex_size: -1,
            physics_convex_in_cache: false,
            physics_mesh_offset: -1,
            physics_mesh_size: -1,
            physics_mesh_in_cache: false,
            lod_offset: [-1, 0, 0, 0],
            lod_size: [-1, 0, 0, 0],
            lod_in_cache: [false; NUM_LODS],
            header_size: -1,
            not_found: false,
            creator_id: [0; 16],
        }
    }
}

impl MeshHeader {
    /// `LLMeshHeader::fromLLSD`.
    pub fn from_llsd(header: &Value) -> MeshHeader {
        let mut h = MeshHeader::default();
        h.version = as_integer(get(header, "version"));
        for i in 0..NUM_LODS {
            let lod = get(header, LOD_NAMES[i]);
            h.lod_offset[i] = as_integer(get(lod, "offset"));
            h.lod_size[i] = as_integer(get(lod, "size"));
        }
        h.skin_offset = as_integer(get(get(header, "skin"), "offset"));
        h.skin_size = as_integer(get(get(header, "skin"), "size"));
        h.physics_convex_offset = as_integer(get(get(header, "physics_convex"), "offset"));
        h.physics_convex_size = as_integer(get(get(header, "physics_convex"), "size"));
        h.physics_mesh_offset = as_integer(get(get(header, "physics_mesh"), "offset"));
        h.physics_mesh_size = as_integer(get(get(header, "physics_mesh"), "size"));
        h.not_found = has(header, "404");
        let creator = get(header, "creator");
        if has(header, "creator") && is_uuid(creator) {
            h.creator_id = as_uuid(creator);
        }
        h
    }

    /// `getFlags`: the in-cache bits of the disk preamble.
    pub fn flags(&self) -> u32 {
        let mut flags = 0;
        for i in 0..NUM_LODS {
            if self.lod_in_cache[i] {
                flags |= 1 << i;
            }
        }
        if self.skin_in_cache {
            flags |= FLAG_SKIN;
        }
        if self.physics_convex_in_cache {
            flags |= FLAG_PHYSCONVEX;
        }
        if self.physics_mesh_in_cache {
            flags |= FLAG_PHYSMESH;
        }
        flags
    }

    /// `setFromFlags`.
    pub fn set_from_flags(&mut self, flags: u32) {
        for i in 0..NUM_LODS {
            self.lod_in_cache[i] = (flags & (1 << i)) != 0;
        }
        self.skin_in_cache = (flags & FLAG_SKIN) != 0;
        self.physics_convex_in_cache = (flags & FLAG_PHYSCONVEX) != 0;
        self.physics_mesh_in_cache = (flags & FLAG_PHYSMESH) != 0;
    }

    /// Marks everything as not in the cache (a cache mismatch).
    pub fn clear_in_cache(&mut self) {
        self.skin_in_cache = false;
        self.physics_convex_in_cache = false;
        self.physics_mesh_in_cache = false;
        self.lod_in_cache = [false; NUM_LODS];
    }

    /// `LLMeshRepository::getActualMeshLOD(header, lod)`: the LOD to load
    /// for the one asked (the nearest lower, else the nearest higher), or
    /// -1 when the asset has none (the header is then marked 404).
    pub fn actual_lod(&mut self, lod: i32) -> i32 {
        let lod = lod.clamp(0, 3) as usize;
        if self.not_found {
            return -1;
        }
        if self.version > MAX_MESH_VERSION {
            return -1;
        }
        if self.lod_size[lod] > 0 {
            return lod as i32;
        }
        for i in (0..lod).rev() {
            if self.lod_size[i] > 0 {
                return i as i32;
            }
        }
        for i in lod + 1..NUM_LODS {
            if self.lod_size[i] > 0 {
                return i as i32;
            }
        }
        self.not_found = true;
        -1
    }

    /// The byte count the header handler reserves in the cache file:
    /// the furthest end of the render LODs, the skin and the convex
    /// decomposition, plus the header and the preamble.
    pub fn cache_file_size(&self) -> i32 {
        let mut lod_bytes = 0;
        for i in 0..LOD_PHYSICS.min(NUM_LODS) {
            lod_bytes = lod_bytes.max(self.lod_offset[i] + self.lod_size[i]);
        }
        lod_bytes = lod_bytes.max(self.skin_offset + self.skin_size);
        lod_bytes = lod_bytes.max(self.physics_convex_offset + self.physics_convex_size);
        lod_bytes + self.header_size + CACHE_PREAMBLE_SIZE
    }
}

/// `EMeshProcessingResult`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ProcessingResult {
    Ok = 0,
    NoData = 1,
    OutOfMemory = 2,
    HttpResponseFailed = 3,
    ParseFailure = 4,
    Invalid = 5,
    Unknown = 6,
}

/// What `headerReceived` derives from a header fetch (or a cache read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderReceipt {
    pub header: MeshHeader,
    /// `header_size` as the C++ accumulated it: the deprecated header's
    /// reported size plus the LLSD header's; offsets into the received
    /// buffer are `combined_header_size + part offset`.
    pub combined_header_size: i64,
    /// The received buffer's size after stripping the deprecated header.
    pub data_size: i32,
    /// The skin block when the received buffer holds it whole:
    /// `(offset, size)` into the original buffer.
    pub skin_in_data: Option<(usize, usize)>,
    /// Each render LOD when the buffer holds it whole, for pending LOD
    /// requests to be served from the same bytes.
    pub lods_in_data: [Option<(usize, usize)>; NUM_LODS],
}

/// `headerReceived`'s parsing and bookkeeping: `flags` is the preamble's
/// flag word when the data came from the cache (0 from the network).
pub fn header_received(data: &[u8], flags: u32) -> Result<HeaderReceipt, ProcessingResult> {
    let mut header = MeshHeader::default();
    let mut header_size: i64 = 0;
    let mut data_size = data.len() as i32;
    let mut skin_in_data = None;
    let mut lods_in_data = [None; NUM_LODS];
    if data_size > 0 {
        let (body, stripped) = strip_deprecated_header(data);
        header_size = stripped as i64;
        data_size = body.len() as i32;
        let Some((header_data, consumed)) = parse_binary(body, data_size as i64, -1) else {
            return Err(ProcessingResult::ParseFailure);
        };
        if !is_map(&header_data) {
            return Err(ProcessingResult::Invalid);
        }
        header = MeshHeader::from_llsd(&header_data);
        if header.version > MAX_MESH_VERSION {
            header.not_found = true;
        } else if header.actual_lod(0) >= 0 {
            header.header_size = consumed as i32;
            header_size += header.header_size as i64;
            if flags != 0 {
                header.set_from_flags(flags);
            } else {
                if header.skin_size > 0
                    && header_size + (header.skin_offset as i64) + (header.skin_size as i64)
                        < (data_size as i64)
                {
                    header.skin_in_cache = true;
                }
                if header.physics_convex_size > 0
                    && header_size
                        + (header.physics_convex_offset as i64)
                        + (header.physics_convex_size as i64)
                        < (data_size as i64)
                {
                    header.physics_convex_in_cache = true;
                }
                if header.physics_mesh_size > 0
                    && header_size
                        + (header.physics_mesh_offset as i64)
                        + (header.physics_mesh_size as i64)
                        < (data_size as i64)
                {
                    header.physics_mesh_in_cache = true;
                }
                for i in 0..NUM_LODS {
                    if header.lod_size[i] > 0
                        && header_size + (header.lod_offset[i] as i64) + (header.lod_size[i] as i64)
                            < (data_size as i64)
                    {
                        header.lod_in_cache[i] = true;
                    }
                }
            }
            // The skin from the same bytes, when it fits.
            if header.skin_offset >= 0 && header.skin_size > 0 {
                let offset = header_size + header.skin_offset as i64;
                if offset + (header.skin_size as i64) < (data_size as i64) && offset >= 0 {
                    skin_in_data = Some((offset as usize, header.skin_size as usize));
                }
            }
            for i in 0..NUM_LODS {
                if header.lod_size[i] > 0 {
                    let offset = header_size + header.lod_offset[i] as i64;
                    if offset + (header.lod_size[i] as i64) <= (data_size as i64) && offset >= 0 {
                        lods_in_data[i] = Some((offset as usize, header.lod_size[i] as usize));
                    }
                }
            }
        }
    } else {
        header.not_found = true;
    }
    Ok(HeaderReceipt {
        header,
        combined_header_size: header_size,
        data_size,
        skin_in_data,
        lods_in_data,
    })
}

/// `write_preamble`: the 12-byte cache preamble.
pub fn encode_preamble(header_bytes: i32, flags: u32) -> [u8; CACHE_PREAMBLE_SIZE as usize] {
    let mut b = [0u8; CACHE_PREAMBLE_SIZE as usize];
    b[0..4].copy_from_slice(&CACHE_PREAMBLE_VERSION.to_le_bytes());
    b[4..8].copy_from_slice(&header_bytes.to_le_bytes());
    b[8..12].copy_from_slice(&flags.to_le_bytes());
    b
}

/// The preamble read back: `(version, header_size, flags)`; `None` when
/// the buffer is too short.
pub fn decode_preamble(buf: &[u8]) -> Option<(u32, i32, u32)> {
    if buf.len() < CACHE_PREAMBLE_SIZE as usize {
        return None;
    }
    Some((
        u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
        i32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
    ))
}

/// The cached-data sanity check of the LOD, skin, decomposition and physics
/// reads: a reserved block that was never written is all zeros in its
/// first kilobyte.
pub fn looks_unwritten(buf: &[u8]) -> bool {
    buf.iter().take(1024).all(|&b| b == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_value(lods: [(i32, i32); 4], skin: Option<(i32, i32)>) -> Value {
        let mut m = std::collections::BTreeMap::new();
        m.insert(b"version".to_vec(), Value::Integer(1));
        for (i, (o, s)) in lods.iter().enumerate() {
            let mut lod = std::collections::BTreeMap::new();
            lod.insert(b"offset".to_vec(), Value::Integer(*o));
            lod.insert(b"size".to_vec(), Value::Integer(*s));
            m.insert(LOD_NAMES[i].as_bytes().to_vec(), Value::Map(lod));
        }
        if let Some((o, s)) = skin {
            let mut sk = std::collections::BTreeMap::new();
            sk.insert(b"offset".to_vec(), Value::Integer(o));
            sk.insert(b"size".to_vec(), Value::Integer(s));
            m.insert(b"skin".to_vec(), Value::Map(sk));
        }
        Value::Map(m)
    }

    #[test]
    fn from_llsd_and_lod_choice() {
        let v = header_value(
            [(0, 100), (100, 0), (100, 300), (400, 500)],
            Some((900, 50)),
        );
        let mut h = MeshHeader::from_llsd(&v);
        assert_eq!(h.lod_size, [100, 0, 300, 500]);
        assert_eq!(h.skin_offset, 900);
        assert_eq!(h.physics_mesh_offset, 0); // absent: asInteger of undef
        assert_eq!(h.actual_lod(1), 0);
        assert_eq!(h.actual_lod(3), 3);
        let empty = MeshHeader::from_llsd(&header_value([(0, 0); 4], None));
        let mut e = empty.clone();
        assert_eq!(e.actual_lod(2), -1);
        assert!(e.not_found);
        h.lod_in_cache = [true, false, true, false];
        h.skin_in_cache = true;
        assert_eq!(h.flags(), 0b10_0101);
        let mut g = MeshHeader::default();
        g.set_from_flags(h.flags());
        assert_eq!(g.lod_in_cache, h.lod_in_cache);
        assert!(g.skin_in_cache);
    }

    #[test]
    fn received_from_network_bytes() {
        let v = header_value(
            [(0, 100), (100, 200), (300, 300), (600, 500)],
            Some((1100, 50)),
        );
        let (bytes, _) = vkstorm_llsd::format(vkstorm_llsd::Format::Binary, &v, Default::default());
        let hs = bytes.len();
        // A 4096-byte first fetch holding the header and the first two LODs.
        let mut data = bytes.clone();
        data.resize(hs + 350, 0xAB);
        let r = header_received(&data, 0).unwrap();
        assert_eq!(r.header.header_size as usize, hs);
        assert_eq!(r.combined_header_size as usize, hs);
        assert_eq!(r.header.lod_in_cache, [true, true, false, false]);
        assert!(!r.header.skin_in_cache);
        assert_eq!(r.lods_in_data[0], Some((hs, 100)));
        assert_eq!(r.lods_in_data[1], Some((hs + 100, 200)));
        assert_eq!(r.lods_in_data[2], None);
        assert_eq!(r.skin_in_data, None);
        assert_eq!(r.header.cache_file_size(), 1150 + hs as i32 + 12);
        // From the cache, the flags come from the preamble.
        let r2 = header_received(&data, 0b0100).unwrap();
        assert_eq!(r2.header.lod_in_cache, [false, false, true, false]);
        // Junk.
        assert_eq!(
            header_received(b"\x00\x01junk", 0).unwrap_err(),
            ProcessingResult::ParseFailure
        );
        assert!(header_received(&[], 0).unwrap().header.not_found);
    }

    #[test]
    fn preamble_round_trip() {
        let p = encode_preamble(123, 0b1011);
        assert_eq!(decode_preamble(&p), Some((1, 123, 0b1011)));
        assert_eq!(decode_preamble(&p[..5]), None);
        assert!(looks_unwritten(&[0; 2000]));
        assert!(!looks_unwritten(&[0, 0, 1]));
    }
}
