//! Stream information, comments and the codec setup: the three Vorbis
//! header packets (`info.c`: `vorbis_synthesis_headerin` and its
//! unpackers, `vorbis_comment_query`), the mapping and mode unpacking
//! (`mapping0.c`), ported from libvorbis 1.3.7.
//!
//! Error behaviour follows the C: a failed identification or setup header
//! clears the whole `vorbis_info` (`vorbis_info_clear`, which also drops
//! the codec setup, so later headers fail with `OV_EFAULT`), a failed
//! comment header clears the comment.
//!
//! Original code: Copyright (c) 1994-2015 Xiph.Org Foundation, BSD-3-Clause
//! (see `LICENSE-xiph`). Rust port: Vulkanstorm.

use std::borrow::Cow;

use crate::bitpack::BitReader;
use crate::codebook::{Codebook, StaticCodebook, ov_ilog, staticbook_unpack};
use crate::floor::{FloorInfo, floor0_unpack, floor1_unpack};
use crate::residue::{ResidueInfo, res0_unpack};
use crate::{OV_EBADHEADER, OV_EFAULT, OV_ENOTVORBIS, OV_EVERSION};

/// `vorbis_info`: the identification header.
#[derive(Clone, Debug, Default)]
pub struct Info {
    /// Vorbis version (always 0 for a valid stream).
    pub version: i32,
    /// Channel count.
    pub channels: i32,
    /// Sample rate, with the platform's C `long` width (a 32-bit value
    /// above 2^31 is negative on Windows, as in the C, and rejected).
    pub rate: i64,
    /// Bitrate hints from the header (`ogg_int32_t` values).
    pub bitrate_upper: i64,
    /// Nominal bitrate hint.
    pub bitrate_nominal: i64,
    /// Lower bitrate hint.
    pub bitrate_lower: i64,
    /// Unused by the decoder (0).
    pub bitrate_window: i64,
    pub(crate) codec_setup: Option<Box<CodecSetup>>,
}

impl Info {
    /// `vorbis_info_init`: an empty info with an allocated codec setup.
    pub fn new() -> Self {
        Self {
            codec_setup: Some(Box::default()),
            ..Default::default()
        }
    }

    /// `vorbis_info_clear`: everything zeroed, no codec setup.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// `vorbis_info_blocksize`: the short (0) or long (1) block size, -1
    /// without a codec setup.
    pub fn blocksize(&self, zo: usize) -> i64 {
        self.codec_setup
            .as_ref()
            .map_or(-1, |ci| ci.blocksizes[zo & 1])
    }
}

/// `vorbis_comment`: the comment header. Strings are kept as the raw bytes
/// of the stream (Vorbis says UTF-8, but nothing enforces it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Comment {
    /// The vendor string; `None` until a comment header was read (the C's
    /// NULL `vendor`).
    pub vendor: Option<Vec<u8>>,
    /// The user comments (`TAG=value`), raw bytes.
    pub user_comments: Vec<Vec<u8>>,
}

impl Comment {
    /// `vorbis_comment_init`.
    pub fn new() -> Self {
        Self::default()
    }

    /// The vendor string, lossily decoded as UTF-8 (empty when absent).
    pub fn vendor_str(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(self.vendor.as_deref().unwrap_or(&[]))
    }

    /// The user comments, lossily decoded as UTF-8.
    pub fn user_comment_strs(&self) -> impl Iterator<Item = Cow<'_, str>> {
        self.user_comments
            .iter()
            .map(|c| String::from_utf8_lossy(c))
    }

    /// `vorbis_comment_query`: the value of the `count`-th comment whose
    /// name matches `tag` (ASCII case-insensitive), i.e. the bytes after
    /// `TAG=`. The C returns a C string, which stops at an embedded NUL;
    /// this returns the bytes up to the first NUL as well.
    pub fn query(&self, tag: &str, count: usize) -> Option<&[u8]> {
        let tag = tag.as_bytes();
        let taglen = tag.len() + 1;
        let mut found = 0usize;
        for c in &self.user_comments {
            if c.len() >= taglen && c[..tag.len()].eq_ignore_ascii_case(tag) && c[tag.len()] == b'='
            {
                if found == count {
                    let v = &c[taglen..];
                    let end = v.iter().position(|&b| b == 0).unwrap_or(v.len());
                    return Some(&v[..end]);
                }
                found += 1;
            }
        }
        None
    }

    /// [`Comment::query`] decoded lossily as UTF-8 (`TITLE`, `ARTIST`...).
    pub fn query_str(&self, tag: &str, count: usize) -> Option<Cow<'_, str>> {
        self.query(tag, count).map(String::from_utf8_lossy)
    }

    /// `vorbis_comment_query_count`.
    pub fn query_count(&self, tag: &str) -> usize {
        let tag = tag.as_bytes();
        self.user_comments
            .iter()
            .filter(|c| {
                c.len() > tag.len()
                    && c[..tag.len()].eq_ignore_ascii_case(tag)
                    && c[tag.len()] == b'='
            })
            .count()
    }
}

/// `vorbis_info_mode`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Mode {
    pub blockflag: i32,
    pub mapping: i32,
}

/// `vorbis_info_mapping0`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Mapping0 {
    pub submaps: i32,
    pub chmuxlist: Vec<i32>,
    pub floorsubmap: [i32; 16],
    pub residuesubmap: [i32; 16],
    pub coupling_steps: i32,
    pub coupling_mag: Vec<i32>,
    pub coupling_ang: Vec<i32>,
}

/// `codec_setup_info`, decode side.
#[derive(Clone, Debug, Default)]
pub(crate) struct CodecSetup {
    pub blocksizes: [i64; 2],
    pub modes: Vec<Mode>,
    pub maps: Vec<Mapping0>,
    pub floors: Vec<FloorInfo>,
    pub residues: Vec<ResidueInfo>,
    /// `ci->books`.
    pub books: i32,
    /// `book_param`: dropped (None) once the decode books are built.
    pub book_param: Vec<Option<StaticCodebook>>,
    /// `fullbooks`: built by the first synthesis init and kept; a failed
    /// codebook init leaves zeroed books here, as the C does.
    pub fullbooks: Option<Vec<Codebook>>,
}

fn read_bytes(opb: &mut BitReader, n: usize) -> Vec<u8> {
    (0..n).map(|_| opb.read(8) as u8).collect()
}

fn unpack_info(vi: &mut Info, opb: &mut BitReader) -> i32 {
    let Some(ci) = vi.codec_setup.as_mut() else {
        return OV_EFAULT;
    };
    vi.version = opb.read(32) as i32;
    if vi.version != 0 {
        return OV_EVERSION;
    }
    vi.channels = opb.read(8) as i32;
    vi.rate = opb.read(32);
    vi.bitrate_upper = opb.read(32) as i32 as i64;
    vi.bitrate_nominal = opb.read(32) as i32 as i64;
    vi.bitrate_lower = opb.read(32) as i32 as i64;

    let ok = (|| {
        let bs = opb.read(4) as i32;
        if bs < 0 {
            return false;
        }
        ci.blocksizes[0] = 1 << bs;
        let bs = opb.read(4) as i32;
        if bs < 0 {
            return false;
        }
        ci.blocksizes[1] = 1 << bs;
        if vi.rate < 1 || vi.channels < 1 {
            return false;
        }
        if ci.blocksizes[0] < 64 || ci.blocksizes[1] < ci.blocksizes[0] || ci.blocksizes[1] > 8192 {
            return false;
        }
        opb.read(1) == 1
    })();
    if ok {
        0
    } else {
        vi.clear();
        OV_EBADHEADER
    }
}

fn unpack_comment(vc: &mut Comment, opb: &mut BitReader) -> i32 {
    let ok = (|| {
        let vendorlen = opb.read(32) as i32;
        if vendorlen < 0 {
            return false;
        }
        if vendorlen as i64 > opb.storage() - 8 {
            return false;
        }
        vc.vendor = Some(read_bytes(opb, vendorlen as usize));
        let i = opb.read(32) as i32;
        if i < 0 {
            return false;
        }
        if i as i64 > ((opb.storage() - opb.bytes()) >> 2) {
            return false;
        }
        vc.user_comments.clear();
        vc.user_comments.reserve(i as usize);
        for _ in 0..i {
            let len = opb.read(32) as i32;
            if len < 0 {
                return false;
            }
            if len as i64 > opb.storage() - opb.bytes() {
                return false;
            }
            vc.user_comments.push(read_bytes(opb, len as usize));
        }
        opb.read(1) == 1
    })();
    if ok {
        0
    } else {
        *vc = Comment::default();
        OV_EBADHEADER
    }
}

/// `mapping0_unpack`.
fn mapping0_unpack(
    channels: i32,
    floors: i32,
    residues: i32,
    opb: &mut BitReader,
) -> Option<Mapping0> {
    let mut info = Mapping0 {
        chmuxlist: vec![0; channels.max(0) as usize],
        ..Default::default()
    };
    if channels <= 0 {
        return None;
    }
    let b = opb.read(1);
    if b < 0 {
        return None;
    }
    if b != 0 {
        info.submaps = (opb.read(4) + 1) as i32;
        if info.submaps <= 0 {
            return None;
        }
    } else {
        info.submaps = 1;
    }

    let b = opb.read(1);
    if b < 0 {
        return None;
    }
    if b != 0 {
        info.coupling_steps = (opb.read(8) + 1) as i32;
        if info.coupling_steps <= 0 {
            return None;
        }
        let bits = ov_ilog((channels - 1) as u32);
        for _ in 0..info.coupling_steps {
            let test_m = opb.read(bits) as i32;
            let test_a = opb.read(bits) as i32;
            info.coupling_mag.push(test_m);
            info.coupling_ang.push(test_a);
            if test_m < 0
                || test_a < 0
                || test_m == test_a
                || test_m >= channels
                || test_a >= channels
            {
                return None;
            }
        }
    }

    if opb.read(2) != 0 {
        return None;
    }

    if info.submaps > 1 {
        for i in 0..channels as usize {
            info.chmuxlist[i] = opb.read(4) as i32;
            if info.chmuxlist[i] >= info.submaps || info.chmuxlist[i] < 0 {
                return None;
            }
        }
    }
    for i in 0..info.submaps as usize {
        opb.read(8); // time submap unused
        info.floorsubmap[i] = opb.read(8) as i32;
        if info.floorsubmap[i] >= floors || info.floorsubmap[i] < 0 {
            return None;
        }
        info.residuesubmap[i] = opb.read(8) as i32;
        if info.residuesubmap[i] >= residues || info.residuesubmap[i] < 0 {
            return None;
        }
    }
    Some(info)
}

fn unpack_books(vi: &mut Info, opb: &mut BitReader) -> i32 {
    let channels = vi.channels;
    let Some(ci) = vi.codec_setup.as_mut() else {
        return OV_EFAULT;
    };
    let ok = (|| {
        ci.books = (opb.read(8) + 1) as i32;
        if ci.books <= 0 {
            return false;
        }
        for _ in 0..ci.books {
            match staticbook_unpack(opb) {
                Some(b) => ci.book_param.push(Some(b)),
                None => return false,
            }
        }

        // time backend settings; hooks are unused
        let times = (opb.read(6) + 1) as i32;
        if times <= 0 {
            return false;
        }
        for _ in 0..times {
            let test = opb.read(16) as i32;
            if test < 0 || test >= 1 {
                return false;
            }
        }

        // floors
        let floors = (opb.read(6) + 1) as i32;
        if floors <= 0 {
            return false;
        }
        for _ in 0..floors {
            let ftype = opb.read(16) as i32;
            let f = match ftype {
                0 => floor0_unpack(&ci.book_param, ci.books, opb).map(FloorInfo::Zero),
                1 => floor1_unpack(ci.books, opb).map(FloorInfo::One),
                _ => return false,
            };
            match f {
                Some(f) => ci.floors.push(f),
                None => return false,
            }
        }

        // residues
        let residues = (opb.read(6) + 1) as i32;
        if residues <= 0 {
            return false;
        }
        for _ in 0..residues {
            let rtype = opb.read(16) as i32;
            if !(0..3).contains(&rtype) {
                return false;
            }
            match res0_unpack(rtype, &ci.book_param, ci.books, opb) {
                Some(r) => ci.residues.push(r),
                None => return false,
            }
        }

        // mappings
        let maps = (opb.read(6) + 1) as i32;
        if maps <= 0 {
            return false;
        }
        for _ in 0..maps {
            let mtype = opb.read(16) as i32;
            if mtype != 0 {
                return false;
            }
            match mapping0_unpack(channels, floors, residues, opb) {
                Some(m) => ci.maps.push(m),
                None => return false,
            }
        }

        // modes
        let modes = (opb.read(6) + 1) as i32;
        if modes <= 0 {
            return false;
        }
        for _ in 0..modes {
            let blockflag = opb.read(1) as i32;
            let windowtype = opb.read(16) as i32;
            let transformtype = opb.read(16) as i32;
            let mapping = opb.read(8) as i32;
            ci.modes.push(Mode { blockflag, mapping });
            if windowtype >= 1 || transformtype >= 1 || mapping >= maps || mapping < 0 {
                return false;
            }
        }

        opb.read(1) == 1
    })();
    if ok {
        0
    } else {
        vi.clear();
        OV_EBADHEADER
    }
}

/// `vorbis_synthesis_idheader`.
pub fn synthesis_idheader(packet: &[u8], b_o_s: bool) -> bool {
    let mut opb = BitReader::new(packet);
    if !b_o_s {
        return false;
    }
    if opb.read(8) != 1 {
        return false;
    }
    let buffer = read_bytes(&mut opb, 6);
    buffer == b"vorbis"
}

/// `vorbis_synthesis_headerin`: 0 or a negative `OV_*` code.
pub(crate) fn synthesis_headerin(
    vi: &mut Info,
    vc: &mut Comment,
    packet: &[u8],
    b_o_s: bool,
) -> i32 {
    let mut opb = BitReader::new(packet);
    let packtype = opb.read(8) as i32;
    let buffer = read_bytes(&mut opb, 6);
    if buffer != b"vorbis" {
        return OV_ENOTVORBIS;
    }
    match packtype {
        0x01 => {
            if !b_o_s {
                return OV_EBADHEADER;
            }
            if vi.rate != 0 {
                return OV_EBADHEADER;
            }
            unpack_info(vi, &mut opb)
        }
        0x03 => {
            if vi.rate == 0 {
                return OV_EBADHEADER;
            }
            if vc.vendor.is_some() {
                return OV_EBADHEADER;
            }
            unpack_comment(vc, &mut opb)
        }
        0x05 => {
            if vi.rate == 0 || vc.vendor.is_none() {
                return OV_EBADHEADER;
            }
            match vi.codec_setup.as_ref() {
                None => return OV_EFAULT,
                Some(ci) if ci.books > 0 => return OV_EBADHEADER,
                _ => {}
            }
            unpack_books(vi, &mut opb)
        }
        _ => OV_EBADHEADER,
    }
}

/// `vorbis_packet_blocksize`.
pub(crate) fn packet_blocksize(vi: &Info, packet: &[u8]) -> i64 {
    let Some(ci) = vi.codec_setup.as_ref() else {
        return OV_EFAULT as i64;
    };
    if ci.modes.is_empty() {
        return OV_EFAULT as i64;
    }
    let mut opb = BitReader::new(packet);
    if opb.read(1) != 0 {
        return crate::OV_ENOTAUDIO as i64;
    }
    let mode = opb.read(ov_ilog((ci.modes.len() - 1) as u32));
    if mode == -1 || mode as usize >= ci.modes.len() {
        return crate::OV_EBADPACKET as i64;
    }
    ci.blocksizes[ci.modes[mode as usize].blockflag as usize & 1]
}
