//! The asset pipeline's data side for Vulkanstorm (vkstorm-rust rung five).
//!
//! This crate holds the logic of the viewer's asset pipeline that is pure
//! data transformation and decision making, behind the C++ classes that keep
//! their names and interfaces in the viewer:
//!
//! - [`texcache`]: `LLTextureCache`'s index, entries file, header-data file,
//!   body files and fast cache, byte-compatible with the files the C++ wrote,
//!   with its purge and corruption rules.
//! - [`fetch`]: `LLTextureFetchWorker`'s state machine (the cache, HTTP
//!   range, decode and cache-write decisions, retries, the HTTP resource
//!   semaphore and waiters) as a pure machine driven by the host's threads
//!   through actions and events, the adaptive retry policy and the JPEG 2000
//!   byte budget per discard level.
//! - [`diskcache`]: `LLDiskCache`'s file naming, purge planning and
//!   directory scans, and `LLFileSystem`'s and `LLLFSThread`'s file I/O.
//! - [`mesh`]: the mesh repository's data side: header parsing, cache
//!   preamble and flags, fetch planning, LLSD mesh asset unpacking into the
//!   viewer's face buffers, physics shapes, skin info and decompositions.
//! - [`asset`]: the generic asset fetch bookkeeping: request table, timeouts,
//!   toxic assets, capability URLs and error strings.
//!
//! The crate has no `unsafe` code; the C ABI lives in the viewer's
//! `vkstorm-ffi` crate. Host facilities the C++ supplied (logging, the clock
//! for cache timestamps) are injected (see [`host`]).

#![forbid(unsafe_code)]

pub mod asset;
pub mod diskcache;
pub mod fetch;
pub mod host;
pub mod mesh;
pub mod texcache;

/// A 16-byte UUID, ordered as `LLUUID::operator<` orders it (byte-wise).
pub type Uuid = [u8; 16];

/// The UUID's text form, as `LLUUID::asString` (lower-case hex, dashed).
pub fn uuid_to_string(id: &Uuid) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(36);
    for (i, b) in id.iter().enumerate() {
        if i == 4 || i == 6 || i == 8 || i == 10 {
            s.push('-');
        }
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 15) as usize] as char);
    }
    s
}

/// Parses the dashed text form; `None` unless it is exactly 36 characters.
pub fn uuid_from_string(s: &str) -> Option<Uuid> {
    let b = s.as_bytes();
    if b.len() != 36 {
        return None;
    }
    let mut out = [0u8; 16];
    let mut i = 0;
    let mut pos = 0;
    while pos < 36 {
        if pos == 8 || pos == 13 || pos == 18 || pos == 23 {
            if b[pos] != b'-' {
                return None;
            }
            pos += 1;
            continue;
        }
        let hi = (b[pos] as char).to_digit(16)?;
        let lo = (b[pos + 1] as char).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
        i += 1;
        pos += 2;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_round_trip() {
        let id: Uuid = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe,
        ];
        let s = uuid_to_string(&id);
        assert_eq!(s, "01234567-89ab-cdef-1032-547698badcfe");
        assert_eq!(uuid_from_string(&s), Some(id));
        assert_eq!(uuid_from_string("short"), None);
    }
}
