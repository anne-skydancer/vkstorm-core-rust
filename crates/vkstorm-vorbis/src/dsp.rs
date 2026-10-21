//! The synthesis state: `vorbis_dsp_state` + `vorbis_block` of the
//! decoder (`block.c`: `_vds_shared_init` decode side,
//! `vorbis_synthesis_blockin`, `_pcmout`, `_read`, `_restart`;
//! `synthesis.c`: `vorbis_synthesis`; `mapping0.c`: `mapping0_inverse`),
//! ported from libvorbis 1.3.7. Half-rate decoding (`vorbis_synthesis_halfrate`)
//! is not supported (the viewer never enables it), so `hs` is always 0.
//!
//! Granule bookkeeping keeps the C's integer widths, including the
//! `long extra` of `vorbis_synthesis_blockin`, which is 32-bit on Windows.
//!
//! Original code: Copyright (c) 1994-2015 Xiph.Org Foundation, BSD-3-Clause
//! (see `LICENSE-xiph`). Rust port: Vulkanstorm.

use crate::bitpack::BitReader;
use crate::clong;
use crate::codebook::{Codebook, ov_ilog};
use crate::floor::{
    FloorInfo, FloorLook, FloorMemo, FloorScratch, floor_look, floor0_inverse1, floor0_inverse2,
    floor1_inverse1, floor1_inverse2,
};
use crate::info::{CodecSetup, Info, Mapping0};
use crate::mdct::MdctLookup;
use crate::residue::{ResidueLook, ResidueScratch, res_inverse, res0_look};
use crate::window::window;
use crate::{OV_EBADPACKET, OV_EINVAL, OV_ENOTAUDIO};

/// `vorbis_block`, decode side, with reusable buffers.
#[derive(Debug)]
struct Block {
    pcm: Vec<Vec<f32>>,
    w: i64,
    pcmend: i64,
    eofflag: bool,
    granulepos: i64,
    sequence: i64,
    /// `vb->pcm != NULL` (a packet was synthesised into the block).
    has_pcm: bool,
    floor_scratch: Vec<FloorScratch>,
    memo: Vec<FloorMemo>,
    nonzero: Vec<bool>,
    res_scratch: ResidueScratch,
}

/// `vorbis_dsp_state` + its `private_state` + the `vorbis_block`.
#[derive(Debug)]
pub(crate) struct DspState {
    channels: usize,
    pcm: Vec<Vec<f32>>,
    pcm_current: i32,
    pcm_returned: i32,
    lw: i64,
    w: i64,
    center_w: i64,
    granulepos: i64,
    sequence: i64,
    eofflag: bool,
    modebits: i32,
    window: [i32; 2],
    transform: [MdctLookup; 2],
    flr: Vec<FloorLook>,
    residue: Vec<ResidueLook>,
    sample_count: i64,
    vb: Block,
}

/// Builds the decode codebooks the first time (`_vds_shared_init`'s
/// "finish the codebooks"); false when one fails (`abort_books`), leaving
/// zeroed books from the failing one on and every static book dropped.
fn finish_books(ci: &mut CodecSetup) -> bool {
    if ci.fullbooks.is_some() {
        return true;
    }
    let n = ci.books.max(0) as usize;
    let mut full: Vec<Codebook> = vec![Codebook::default(); n];
    let mut ok = true;
    for i in 0..n {
        let Some(sb) = ci.book_param.get(i).and_then(|b| b.as_ref()) else {
            ok = false;
            break;
        };
        match Codebook::init_decode(sb) {
            Ok(b) => full[i] = b,
            Err(zeroed) => {
                full[i] = zeroed;
                ok = false;
                break;
            }
        }
        ci.book_param[i] = None;
    }
    ci.fullbooks = Some(full);
    if !ok {
        for b in ci.book_param.iter_mut() {
            *b = None;
        }
    }
    ok
}

impl DspState {
    /// `vorbis_synthesis_init`: `None` where the C returns non-zero.
    pub fn init(vi: &mut Info) -> Option<Self> {
        let channels = vi.channels.max(0) as usize;
        let ci = vi.codec_setup.as_mut()?;
        if ci.modes.is_empty() || ci.blocksizes[0] < 64 || ci.blocksizes[1] < ci.blocksizes[0] {
            return None;
        }
        let modebits = ov_ilog((ci.modes.len() - 1) as u32);
        let transform = [
            MdctLookup::new(ci.blocksizes[0] as usize),
            MdctLookup::new(ci.blocksizes[1] as usize),
        ];
        let window = [
            ov_ilog(ci.blocksizes[0] as u32) - 7,
            ov_ilog(ci.blocksizes[1] as u32) - 7,
        ];
        if !finish_books(ci) {
            return None;
        }
        let books = ci.fullbooks.as_deref().unwrap_or(&[]);
        let bs1 = ci.blocksizes[1] as usize;
        let flr = ci.floors.iter().map(floor_look).collect();
        let residue = ci.residues.iter().map(|r| res0_look(r, books)).collect();
        let mut d = DspState {
            channels,
            pcm: vec![vec![0f32; bs1]; channels],
            pcm_current: 0,
            pcm_returned: 0,
            lw: 0,
            w: 0,
            center_w: ci.blocksizes[1] / 2,
            granulepos: 0,
            sequence: 0,
            eofflag: false,
            modebits,
            window,
            transform,
            flr,
            residue,
            sample_count: 0,
            vb: Block {
                pcm: vec![vec![0f32; bs1]; channels],
                w: 0,
                pcmend: 0,
                eofflag: false,
                granulepos: 0,
                sequence: 0,
                has_pcm: false,
                floor_scratch: vec![FloorScratch::default(); channels],
                memo: vec![FloorMemo::Unused; channels],
                nonzero: vec![false; channels],
                res_scratch: ResidueScratch::default(),
            },
        };
        d.pcm_current = d.center_w as i32;
        d.restart(ci);
        Some(d)
    }

    /// `vorbis_synthesis_restart`.
    pub fn restart(&mut self, ci: &CodecSetup) {
        self.center_w = ci.blocksizes[1] >> 1;
        self.pcm_current = self.center_w as i32;
        self.pcm_returned = -1;
        self.granulepos = -1;
        self.sequence = -1;
        self.eofflag = false;
        self.sample_count = -1;
    }

    /// `vorbis_synthesis`: decodes a packet into the block; 0 or
    /// `OV_ENOTAUDIO`/`OV_EBADPACKET`.
    pub fn synthesis(
        &mut self,
        ci: &CodecSetup,
        packet: &[u8],
        granulepos: i64,
        packetno: i64,
        e_o_s: bool,
    ) -> i32 {
        let mut opb = BitReader::new(packet);
        if opb.read(1) != 0 {
            return OV_ENOTAUDIO;
        }
        let mode = opb.read(self.modebits);
        if mode == -1 {
            return OV_EBADPACKET;
        }
        let Some(m) = ci.modes.get(mode as usize) else {
            return OV_EBADPACKET;
        };
        self.vb.w = m.blockflag as i64;
        if self.vb.w != 0 {
            let _lw = opb.read(1);
            let nw = opb.read(1);
            if nw == -1 {
                return OV_EBADPACKET;
            }
        }
        self.vb.granulepos = granulepos;
        self.vb.sequence = packetno;
        self.vb.eofflag = e_o_s;
        self.vb.pcmend = ci.blocksizes[self.vb.w as usize];
        self.vb.has_pcm = true;
        let map = &ci.maps[m.mapping as usize];
        self.mapping0_inverse(ci, map, &mut opb);
        0
    }

    /// `mapping0_inverse`.
    fn mapping0_inverse(&mut self, ci: &CodecSetup, info: &Mapping0, opb: &mut BitReader) {
        let books = ci.fullbooks.as_deref().unwrap_or(&[]);
        let n = self.vb.pcmend as usize;
        let w = self.vb.w as usize;
        let chans = self.channels;
        let vb = &mut self.vb;

        // recover the spectral envelope
        for i in 0..chans {
            let submap = info.chmuxlist[i] as usize;
            let fl = info.floorsubmap[submap] as usize;
            let scratch = &mut vb.floor_scratch[i];
            vb.memo[i] = match (&ci.floors[fl], &self.flr[fl]) {
                (FloorInfo::One(fi), FloorLook::One(look)) => {
                    floor1_inverse1(fi, look, books, opb, &mut scratch.fit)
                }
                (FloorInfo::Zero(fi), FloorLook::Zero(look)) => {
                    floor0_inverse1(fi, look, books, opb, &mut scratch.lsp)
                }
                _ => FloorMemo::Unused,
            };
            vb.nonzero[i] = vb.memo[i] == FloorMemo::Used;
            for v in vb.pcm[i][..n / 2].iter_mut() {
                *v = 0.0;
            }
        }

        // channel coupling can 'dirty' the nonzero listing
        for i in 0..info.coupling_steps as usize {
            let m = info.coupling_mag[i] as usize;
            let a = info.coupling_ang[i] as usize;
            if vb.nonzero[m] || vb.nonzero[a] {
                vb.nonzero[m] = true;
                vb.nonzero[a] = true;
            }
        }

        // recover the residue into our working vectors
        for i in 0..info.submaps as usize {
            let mut bundle: Vec<&mut [f32]> = Vec::with_capacity(chans);
            let mut zerobundle: Vec<bool> = Vec::with_capacity(chans);
            for (j, p) in vb.pcm.iter_mut().enumerate() {
                if info.chmuxlist[j] as usize == i {
                    zerobundle.push(vb.nonzero[j]);
                    bundle.push(&mut p[..n]);
                }
            }
            let r = info.residuesubmap[i] as usize;
            res_inverse(
                &ci.residues[r],
                &self.residue[r],
                books,
                opb,
                &mut bundle,
                &zerobundle,
                n as i64,
                &mut vb.res_scratch,
            );
        }

        // channel coupling
        for i in (0..info.coupling_steps as usize).rev() {
            let mi = info.coupling_mag[i] as usize;
            let ai = info.coupling_ang[i] as usize;
            let (pm, pa) = two_mut(&mut vb.pcm, mi, ai);
            for (m, a) in pm[..n / 2].iter_mut().zip(pa[..n / 2].iter_mut()) {
                let mag = *m;
                let ang = *a;
                if mag > 0.0 {
                    if ang > 0.0 {
                        *m = mag;
                        *a = mag - ang;
                    } else {
                        *a = mag;
                        *m = mag + ang;
                    }
                } else if ang > 0.0 {
                    *m = mag;
                    *a = mag + ang;
                } else {
                    *a = mag;
                    *m = mag - ang;
                }
            }
        }

        // compute and apply spectral envelope
        for i in 0..chans {
            let submap = info.chmuxlist[i] as usize;
            let fl = info.floorsubmap[submap] as usize;
            let memo = vb.memo[i];
            let scratch = &mut vb.floor_scratch[i];
            let pcm = &mut vb.pcm[i];
            match (&ci.floors[fl], &mut self.flr[fl]) {
                (FloorInfo::One(fi), FloorLook::One(look)) => {
                    floor1_inverse2(fi, look, memo, &scratch.fit, (n / 2) as i32, pcm);
                }
                (FloorInfo::Zero(fi), FloorLook::Zero(look)) => {
                    floor0_inverse2(fi, look, ci.blocksizes[w], w, memo, &mut scratch.lsp, pcm);
                }
                _ => {}
            }
        }

        // transform the PCM data
        let t = &self.transform[w];
        for p in vb.pcm.iter_mut() {
            t.backward(&mut p[..n]);
        }
    }

    /// `vorbis_synthesis_blockin`: 0 or `OV_EINVAL` (unread samples
    /// pending).
    pub fn blockin(&mut self, ci: &CodecSetup) -> i32 {
        if self.pcm_current > self.pcm_returned && self.pcm_returned != -1 {
            return OV_EINVAL;
        }
        self.lw = self.w;
        self.w = self.vb.w;

        if self.sequence == -1 || self.sequence.wrapping_add(1) != self.vb.sequence {
            self.granulepos = -1;
            self.sample_count = -1;
        }
        self.sequence = self.vb.sequence;

        let bs = ci.blocksizes;
        if self.vb.has_pcm {
            let n = (bs[self.w as usize] >> 1) as usize;
            let n0 = (bs[0] >> 1) as usize;
            let n1 = (bs[1] >> 1) as usize;
            let (this_center, prev_center) = if self.center_w != 0 { (n1, 0) } else { (0, n1) };

            for j in 0..self.channels {
                let pcm = &mut self.pcm[j];
                let p = &self.vb.pcm[j];
                if self.lw != 0 {
                    if self.w != 0 {
                        // large/large
                        let w = window(self.window[1]);
                        overlap(&mut pcm[prev_center..prev_center + n1], &p[..n1], w);
                    } else {
                        // large/small
                        let w = window(self.window[0]);
                        let s = prev_center + n1 / 2 - n0 / 2;
                        overlap(&mut pcm[s..s + n0], &p[..n0], w);
                    }
                } else if self.w != 0 {
                    // small/large
                    let w = window(self.window[0]);
                    let ps = n1 / 2 - n0 / 2;
                    overlap(&mut pcm[prev_center..prev_center + n0], &p[ps..ps + n0], w);
                    let e = n1 / 2 + n0 / 2;
                    pcm[prev_center + n0..prev_center + e].copy_from_slice(&p[ps + n0..ps + e]);
                } else {
                    // small/small
                    let w = window(self.window[0]);
                    overlap(&mut pcm[prev_center..prev_center + n0], &p[..n0], w);
                }
                // the copy section
                pcm[this_center..this_center + n].copy_from_slice(&p[n..2 * n]);
            }

            self.center_w = if self.center_w != 0 { 0 } else { n1 as i64 };

            if self.pcm_returned == -1 {
                self.pcm_returned = this_center as i32;
                self.pcm_current = this_center as i32;
            } else {
                self.pcm_returned = prev_center as i32;
                self.pcm_current = (prev_center as i64
                    + (bs[self.lw as usize] / 4 + bs[self.w as usize] / 4))
                    as i32;
            }
        }

        let step = bs[self.lw as usize] / 4 + bs[self.w as usize] / 4;
        if self.sample_count == -1 {
            self.sample_count = 0;
        } else {
            self.sample_count = self.sample_count.wrapping_add(step);
        }

        if self.granulepos == -1 {
            if self.vb.granulepos != -1 {
                self.granulepos = self.vb.granulepos;
                if self.sample_count > self.granulepos {
                    // `long extra`: 32-bit on Windows
                    let mut extra = clong(self.sample_count.wrapping_sub(self.vb.granulepos));
                    if extra < 0 {
                        extra = 0;
                    }
                    if self.vb.eofflag {
                        let avail = (self.pcm_current - self.pcm_returned) as i64;
                        if extra > avail {
                            extra = avail;
                        }
                        self.pcm_current = (self.pcm_current as i64 - extra) as i32;
                    } else {
                        self.pcm_returned = (self.pcm_returned as i64).wrapping_add(extra) as i32;
                        if self.pcm_returned > self.pcm_current {
                            self.pcm_returned = self.pcm_current;
                        }
                    }
                }
            }
        } else {
            self.granulepos = self.granulepos.wrapping_add(step);
            if self.vb.granulepos != -1 && self.granulepos != self.vb.granulepos {
                if self.granulepos > self.vb.granulepos {
                    let mut extra = clong(self.granulepos.wrapping_sub(self.vb.granulepos));
                    if extra != 0 && self.vb.eofflag {
                        let avail = (self.pcm_current - self.pcm_returned) as i64;
                        if extra > avail {
                            extra = avail;
                        }
                        if extra < 0 {
                            extra = 0;
                        }
                        self.pcm_current = (self.pcm_current as i64 - extra) as i32;
                    }
                }
                self.granulepos = self.vb.granulepos;
            }
        }

        if self.vb.eofflag {
            self.eofflag = true;
        }
        0
    }

    /// `vorbis_synthesis_pcmout` without the buffers: the number of
    /// samples ready per channel.
    #[inline]
    pub fn pcmout_count(&self) -> i32 {
        if self.pcm_returned > -1 && self.pcm_returned < self.pcm_current {
            self.pcm_current - self.pcm_returned
        } else {
            0
        }
    }

    /// The ready samples of channel `ch` (`(*pcm)[ch][0..count]`).
    #[inline]
    pub fn pcm_channel(&self, ch: usize) -> &[f32] {
        let n = self.pcmout_count();
        if n == 0 {
            return &[];
        }
        let s = self.pcm_returned as usize;
        &self.pcm[ch][s..s + n as usize]
    }

    /// The channel count the state was set up for.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// `vorbis_synthesis_read`.
    pub fn read(&mut self, n: i32) -> i32 {
        if n != 0 && self.pcm_returned.wrapping_add(n) > self.pcm_current {
            return OV_EINVAL;
        }
        self.pcm_returned = self.pcm_returned.wrapping_add(n);
        0
    }

    /// The trig table of the short (0) or long (1) MDCT (parity tests).
    #[allow(dead_code)]
    pub fn mdct_trig(&self, which: usize) -> &[f32] {
        self.transform[which & 1].trig()
    }
}

/// `pcm[i]=pcm[i]*w[n-i-1] + p[i]*w[i]` over `n = pcm.len()`.
#[inline]
fn overlap(pcm: &mut [f32], p: &[f32], w: &[f32]) {
    let n = pcm.len();
    let w = &w[..n];
    let p = &p[..n];
    for i in 0..n {
        pcm[i] = pcm[i] * w[n - i - 1] + p[i] * w[i];
    }
}

fn two_mut<T>(v: &mut [T], a: usize, b: usize) -> (&mut T, &mut T) {
    debug_assert!(a != b);
    if a < b {
        let (x, y) = v.split_at_mut(b);
        (&mut x[a], &mut y[0])
    } else {
        let (x, y) = v.split_at_mut(a);
        (&mut y[0], &mut x[b])
    }
}
