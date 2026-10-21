//! Residues 0, 1 and 2 (`res0.c`), unpack, look-up and inverse, ported
//! from libvorbis 1.3.7.
//!
//! Original code: Copyright (c) 1994-2010 Xiph.Org Foundation, BSD-3-Clause
//! (see `LICENSE-xiph`). Rust port: Vulkanstorm.

use crate::bitpack::BitReader;
use crate::codebook::{Codebook, StaticCodebook, ov_ilog};

/// `vorbis_info_residue0` plus its type (0, 1 or 2).
#[derive(Clone, Debug)]
pub(crate) struct ResidueInfo {
    pub rtype: i32,
    begin: i64,
    end: i64,
    grouping: i32,
    partitions: i32,
    partvals: i32,
    groupbook: i32,
    secondstages: [i32; 64],
    booklist: Vec<i32>,
}

fn icount(mut v: u32) -> i32 {
    let mut ret = 0;
    while v != 0 {
        ret += (v & 1) as i32;
        v >>= 1;
    }
    ret
}

/// `res0_unpack` (shared by the three types).
pub(crate) fn res0_unpack(
    rtype: i32,
    books: &[Option<StaticCodebook>],
    nbooks: i32,
    opb: &mut BitReader,
) -> Option<ResidueInfo> {
    let mut info = ResidueInfo {
        rtype,
        begin: opb.read(24),
        end: opb.read(24),
        grouping: (opb.read(24) + 1) as i32,
        partitions: (opb.read(6) + 1) as i32,
        partvals: 0,
        groupbook: 0,
        secondstages: [0; 64],
        booklist: Vec::new(),
    };
    info.groupbook = opb.read(8) as i32;
    if info.groupbook < 0 {
        return None;
    }
    let mut acc = 0;
    for j in 0..info.partitions as usize {
        let mut cascade = opb.read(3) as i32;
        let cflag = opb.read(1) as i32;
        if cflag < 0 {
            return None;
        }
        if cflag != 0 {
            let c = opb.read(5) as i32;
            if c < 0 {
                return None;
            }
            cascade |= c << 3;
        }
        info.secondstages[j] = cascade;
        acc += icount(cascade as u32);
    }
    for _ in 0..acc {
        let book = opb.read(8) as i32;
        if book < 0 {
            return None;
        }
        info.booklist.push(book);
    }

    if info.groupbook >= nbooks {
        return None;
    }
    for &b in &info.booklist {
        if b >= nbooks {
            return None;
        }
        if books[b as usize].as_ref()?.maptype == 0 {
            return None;
        }
    }

    // verify the phrasebook is not specifying an impossible or
    // inconsistent partitioning scheme
    let gb = books[info.groupbook as usize].as_ref()?;
    let entries = gb.entries as i32;
    let mut dim = gb.dim as i32;
    let mut partvals: i32 = 1;
    if dim < 1 {
        return None;
    }
    while dim > 0 {
        partvals = partvals.wrapping_mul(info.partitions);
        if partvals > entries {
            return None;
        }
        dim -= 1;
    }
    info.partvals = partvals;
    Some(info)
}

/// `vorbis_look_residue0`.
#[derive(Clone, Debug)]
pub(crate) struct ResidueLook {
    stages: i32,
    phrasebook: usize,
    /// Per partition class, the book of each stage (`partbooks`).
    partbooks: Vec<[Option<usize>; 8]>,
    /// `decodemap`, flattened: `partvals` rows of `dim` classes.
    decodemap: Vec<i32>,
    dim: usize,
}

/// `res0_look`.
pub(crate) fn res0_look(info: &ResidueInfo, fullbooks: &[Codebook]) -> ResidueLook {
    let parts = info.partitions;
    let phrasebook = info.groupbook as usize;
    let dim = fullbooks[phrasebook].dim.max(0) as usize;
    let mut maxstage = 0;
    let mut acc = 0usize;
    let mut partbooks = vec![[None; 8]; parts as usize];
    for j in 0..parts as usize {
        let stages = ov_ilog(info.secondstages[j] as u32);
        if stages != 0 {
            if stages > maxstage {
                maxstage = stages;
            }
            for k in 0..stages as usize {
                if info.secondstages[j] & (1 << k) != 0 {
                    partbooks[j][k] = Some(info.booklist[acc] as usize);
                    acc += 1;
                }
            }
        }
    }
    let mut partvals: i32 = 1;
    for _ in 0..dim {
        partvals = partvals.wrapping_mul(parts);
    }
    let mut decodemap = vec![0i32; partvals.max(0) as usize * dim];
    for j in 0..partvals.max(0) as usize {
        let mut val = j as i64;
        let mut mult = (partvals / parts) as i64;
        for k in 0..dim {
            let deco = if mult != 0 { val / mult } else { 0 };
            val -= deco * mult;
            mult /= parts as i64;
            decodemap[j * dim + k] = deco as i32;
        }
    }
    ResidueLook {
        stages: maxstage,
        phrasebook,
        partbooks,
        decodemap,
        dim,
    }
}

/// Reusable buffers for the residue decode.
#[derive(Debug, Default)]
pub(crate) struct ResidueScratch {
    partword: Vec<i32>,
    entries: Vec<usize>,
}

/// `res0_inverse`/`res1_inverse` (`_01inverse`) and `res2_inverse`.
/// `chans` are the channel vectors of the submap bundle, `nonzero` their
/// flags; `pcmend` is the block size.
pub(crate) fn res_inverse(
    info: &ResidueInfo,
    look: &ResidueLook,
    books: &[Codebook],
    opb: &mut BitReader,
    chans: &mut [&mut [f32]],
    nonzero: &[bool],
    pcmend: i64,
    scratch: &mut ResidueScratch,
) {
    match info.rtype {
        2 => res2_inverse(info, look, books, opb, chans, nonzero, pcmend, scratch),
        _ => {
            // compact to the non-zero channels (`in[used++]=in[i]`)
            let mut used: Vec<&mut [f32]> = Vec::with_capacity(chans.len());
            for (c, nz) in chans.iter_mut().zip(nonzero) {
                if *nz {
                    used.push(&mut **c);
                }
            }
            if !used.is_empty() {
                inverse01(info, look, books, opb, &mut used, pcmend, scratch);
            }
        }
    }
}

fn inverse01(
    info: &ResidueInfo,
    look: &ResidueLook,
    books: &[Codebook],
    opb: &mut BitReader,
    chans: &mut [&mut [f32]],
    pcmend: i64,
    scratch: &mut ResidueScratch,
) {
    let ch = chans.len();
    let samples_per_partition = info.grouping;
    let partitions_per_word = books[look.phrasebook].dim as i32;
    let max = (pcmend >> 1) as i32;
    let end = if info.end < max as i64 {
        info.end as i32
    } else {
        max
    };
    let n = (end as i64 - info.begin) as i32;
    if n <= 0 {
        return;
    }
    if partitions_per_word <= 0 || samples_per_partition <= 0 {
        // the C divides by zero here (a phrasebook left zeroed by a failed
        // codebook init); treat as end of packet
        return;
    }
    let partvals = n / samples_per_partition;
    let partwords = ((partvals + partitions_per_word - 1) / partitions_per_word) as usize;
    let phrasebook = &books[look.phrasebook];
    scratch.partword.clear();
    scratch.partword.resize(ch * partwords, 0);
    let dim = look.dim;

    for s in 0..look.stages {
        let mut i: i32 = 0;
        let mut l: usize = 0;
        while i < partvals {
            if s == 0 {
                for j in 0..ch {
                    let temp = phrasebook.decode(opb) as i32;
                    if temp == -1 || temp >= info.partvals {
                        return;
                    }
                    scratch.partword[j * partwords + l] = temp;
                }
            }
            let mut k = 0;
            while k < partitions_per_word && i < partvals {
                for j in 0..ch {
                    let offset = (info.begin + i as i64 * samples_per_partition as i64) as usize;
                    let row = scratch.partword[j * partwords + l] as usize;
                    let class = look.decodemap[row * dim + k as usize] as usize;
                    if info.secondstages[class] & (1 << s) != 0
                        && let Some(b) = look.partbooks[class][s as usize]
                    {
                        let stagebook = &books[b];
                        let a = &mut chans[j][offset..];
                        let r = if info.rtype == 0 {
                            stagebook.decodevs_add(
                                a,
                                opb,
                                samples_per_partition,
                                &mut scratch.entries,
                            )
                        } else {
                            stagebook.decodev_add(a, opb, samples_per_partition)
                        };
                        if r == -1 {
                            return;
                        }
                    }
                }
                k += 1;
                i += 1;
            }
            l += 1;
        }
    }
}

fn res2_inverse(
    info: &ResidueInfo,
    look: &ResidueLook,
    books: &[Codebook],
    opb: &mut BitReader,
    chans: &mut [&mut [f32]],
    nonzero: &[bool],
    pcmend: i64,
    scratch: &mut ResidueScratch,
) {
    let ch = chans.len() as i32;
    let samples_per_partition = info.grouping;
    let partitions_per_word = books[look.phrasebook].dim as i32;
    let max = ((pcmend * ch as i64) >> 1) as i32;
    let end = if info.end < max as i64 {
        info.end as i32
    } else {
        max
    };
    let n = (end as i64 - info.begin) as i32;
    if n <= 0 {
        return;
    }
    if partitions_per_word <= 0 || samples_per_partition <= 0 {
        return; // divide by zero in the C (see inverse01)
    }
    let partvals = n / samples_per_partition;
    let partwords = ((partvals + partitions_per_word - 1) / partitions_per_word) as usize;
    if !nonzero.iter().any(|&z| z) {
        return;
    }
    let phrasebook = &books[look.phrasebook];
    scratch.partword.clear();
    scratch.partword.resize(partwords, 0);
    let dim = look.dim;

    for s in 0..look.stages {
        let mut i: i32 = 0;
        let mut l: usize = 0;
        while i < partvals {
            if s == 0 {
                let temp = phrasebook.decode(opb) as i32;
                if temp == -1 || temp >= info.partvals {
                    return;
                }
                scratch.partword[l] = temp;
            }
            let mut k = 0;
            while k < partitions_per_word && i < partvals {
                let row = scratch.partword[l] as usize;
                let class = look.decodemap[row * dim + k as usize] as usize;
                if info.secondstages[class] & (1 << s) != 0
                    && let Some(b) = look.partbooks[class][s as usize]
                {
                    let offset = i as i64 * samples_per_partition as i64 + info.begin;
                    if books[b].decodevv_add(chans, offset, ch, opb, samples_per_partition) == -1 {
                        return;
                    }
                }
                k += 1;
                i += 1;
            }
            l += 1;
        }
    }
}
