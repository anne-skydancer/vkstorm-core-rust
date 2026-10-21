//! `OggVorbis_File` over an in-memory file: a port of libvorbisfile 1.3.7's
//! seekable open (`ov_open_callbacks` with seek and tell callbacks: link
//! discovery by bisection, per-link headers, PCM lengths from the last
//! granule positions, the initial `ov_raw_seek`) and of `ov_read`
//! (`_fetch_and_process_packet`, holes, chained links, the float to
//! integer conversion of `vorbis_ftoi`, clipping and interleaving).
//!
//! The data source behaves like the viewer's `LLFileSystem` callbacks in
//! `llaudiodecodemgr.cpp`: reads return what is left, a seek outside
//! `[0, size]` fails (after clamping the position), `tell` reports the
//! position, and a read never sets `errno`.
//!
//! The non-seekable (streaming) paths of vorbisfile are not ported; streams
//! use the packet API ([`crate::Decoder`] with [`crate::ogg`]).
//!
//! Original code: Copyright (c) 1994-2015 Xiph.Org Foundation, BSD-3-Clause
//! (see `LICENSE-xiph`). Rust port: Vulkanstorm.

use crate::dsp::DspState;
use crate::info::{Comment, Info, packet_blocksize, synthesis_headerin, synthesis_idheader};
use crate::ogg::{Page, StreamState, SyncState};
use crate::{
    Error, OV_EBADHEADER, OV_EBADLINK, OV_EFAULT, OV_EINVAL, OV_ENOTVORBIS, OV_EOF, OV_EREAD,
    OV_FALSE, OV_HOLE,
};

const CHUNKSIZE: i64 = 65536;
const READSIZE: usize = 2048;

const PARTOPEN: i32 = 1;
const OPENED: i32 = 2;
const STREAMSET: i32 = 3;
const INITSET: i32 = 4;

const SEEK_SET: i32 = 0;
const SEEK_CUR: i32 = 1;
const SEEK_END: i32 = 2;

/// The viewer's `LLFileSystem` cache file, in memory.
#[derive(Debug)]
struct MemSource {
    data: Vec<u8>,
    pos: i32,
}

impl MemSource {
    /// `cache_read(ptr, 1, n)`.
    fn read(&mut self, buf: &mut [u8]) -> usize {
        let size = self.data.len() as i64;
        let pos = self.pos as i64;
        if pos >= size || pos < 0 {
            return 0;
        }
        let n = (buf.len() as i64).min(size - pos) as usize;
        buf[..n].copy_from_slice(&self.data[pos as usize..pos as usize + n]);
        self.pos += n as i32;
        n
    }

    /// `cache_seek` + `LLFileSystem::seek`: 0 or -1.
    fn seek(&mut self, offset: i64, whence: i32) -> i32 {
        if offset > i32::MAX as i64 {
            return -1;
        }
        let size = self.data.len() as i32;
        let origin = match whence {
            SEEK_SET => 0,
            SEEK_END => size,
            SEEK_CUR => self.pos,
            _ => return -1,
        };
        let new_pos = origin.wrapping_add(offset as i32);
        if new_pos > size {
            self.pos = size;
            -1
        } else if new_pos < 0 {
            self.pos = 0;
            -1
        } else {
            self.pos = new_pos;
            0
        }
    }

    fn tell(&self) -> i64 {
        self.pos as i64
    }
}

/// An open Ogg Vorbis file (`OggVorbis_File`), seekable, in memory.
#[derive(Debug)]
pub struct VorbisFile {
    src: MemSource,
    offset: i64,
    end: i64,
    oy: SyncState,
    links: i32,
    offsets: Vec<i64>,
    dataoffsets: Vec<i64>,
    serialnos: Vec<i64>,
    pcmlengths: Vec<i64>,
    vi: Vec<Info>,
    vc: Vec<Comment>,
    pcm_offset: i64,
    ready_state: i32,
    current_serialno: i64,
    current_link: i32,
    bittrack: f64,
    samptrack: f64,
    os: StreamState,
    vd: Option<DspState>,
    page: Page,
}

/// `ov_read`'s float to integer conversion, `vorbis_ftoi`: on x86 and
/// x86-64 (MSVC x64 and GCC with SSE2 math) `_mm_cvtsd_si32` of the double,
/// i.e. round half to even with out-of-range and NaN giving `INT_MIN`;
/// elsewhere `(int)floor(f+.5)`.
#[inline(always)]
fn vorbis_ftoi(f: f64) -> i32 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        let r = f.round_ties_even();
        if (-2_147_483_648.0..=2_147_483_647.0).contains(&r) {
            r as i32
        } else {
            i32::MIN
        }
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        (f + 0.5).floor() as i32
    }
}

fn lookup_serialno(s: i64, list: &[i64]) -> bool {
    list.contains(&s)
}

/// One level of `_bisect_forward_serialno`'s recursion, kept for the
/// unwinding assignments.
struct Level {
    next: i64,
    serialno: i64,
    dataoffset: i64,
    vi: Info,
    vc: Comment,
    searchgran: i64,
    pcmoffset: i64,
}

impl VorbisFile {
    /// `ov_open_callbacks` over a copy of `data` with the viewer's seekable
    /// callbacks; `Err` carries the negative `OV_*` result.
    pub fn open(data: &[u8]) -> Result<Self, Error> {
        Self::open_owned(data.to_vec())
    }

    /// [`VorbisFile::open`] taking ownership of the bytes.
    pub fn open_owned(data: Vec<u8>) -> Result<Self, Error> {
        let mut vf = VorbisFile {
            src: MemSource { data, pos: 0 },
            offset: 0,
            end: 0,
            oy: SyncState::new(),
            links: 1,
            offsets: Vec::new(),
            dataoffsets: Vec::new(),
            serialnos: Vec::new(),
            pcmlengths: Vec::new(),
            vi: vec![Info::default()],
            vc: vec![Comment::default()],
            pcm_offset: 0,
            ready_state: 0,
            current_serialno: 0,
            current_link: 0,
            bittrack: 0.0,
            samptrack: 0.0,
            os: StreamState::new(-1),
            vd: None,
            page: Page::new(),
        };
        // _ov_open1: the seek test (`seek_func(f,0,SEEK_CUR)`) succeeds, so
        // the file is seekable.
        vf.src.seek(0, SEEK_CUR);
        let mut serialno_list: Vec<i64> = Vec::new();
        let mut vi = std::mem::take(&mut vf.vi[0]);
        let mut vc = std::mem::take(&mut vf.vc[0]);
        let ret = vf.fetch_headers(&mut vi, &mut vc, Some(&mut serialno_list));
        vf.vi[0] = vi;
        vf.vc[0] = vc;
        if ret < 0 {
            return Err(Error::from_code(ret).unwrap_or(Error::Fault));
        }
        vf.current_serialno = vf.os.serialno() as i64;
        vf.serialnos = Vec::with_capacity(serialno_list.len() + 2);
        vf.serialnos.push(vf.current_serialno);
        vf.serialnos.push(serialno_list.len() as i64);
        vf.serialnos.extend_from_slice(&serialno_list);
        vf.offsets = vec![0];
        vf.dataoffsets = vec![vf.offset];
        vf.ready_state = PARTOPEN;

        // _ov_open2
        vf.ready_state = OPENED;
        let ret = vf.open_seekable2();
        if ret != 0 {
            return Err(Error::from_code(ret).unwrap_or(Error::Fault));
        }
        Ok(vf)
    }

    // ---------------------------------------------------------------
    // I/O helpers

    /// `_get_data`.
    fn get_data(&mut self) -> i64 {
        let Some(buf) = self.oy.buffer(READSIZE) else {
            return -1;
        };
        let bytes = self.src.read(buf);
        if bytes > 0 {
            self.oy.wrote(bytes);
        }
        bytes as i64
    }

    /// `_seek_helper`.
    fn seek_helper(&mut self, offset: i64) -> i32 {
        if self.offset != offset {
            if self.src.seek(offset, SEEK_SET) == -1 {
                return OV_EREAD;
            }
            self.offset = offset;
            self.oy.reset();
        }
        0
    }

    /// `_get_next_page` into `self.page`.
    fn get_next_page(&mut self, boundary: i64) -> i64 {
        let mut boundary = boundary;
        if boundary > 0 {
            boundary += self.offset;
        }
        loop {
            if boundary > 0 && self.offset >= boundary {
                return OV_FALSE as i64;
            }
            let more = self.oy.pageseek(Some(&mut self.page));
            if more < 0 {
                self.offset -= more;
            } else if more == 0 {
                if boundary == 0 {
                    return OV_FALSE as i64;
                }
                let ret = self.get_data();
                if ret == 0 {
                    return OV_EOF as i64;
                }
                if ret < 0 {
                    return OV_EREAD as i64;
                }
            } else {
                let ret = self.offset;
                self.offset += more;
                return ret;
            }
        }
    }

    /// `_get_prev_page_serial`.
    fn get_prev_page_serial(
        &mut self,
        begin: i64,
        serial_list: &[i64],
        serialno: &mut i32,
        granpos: &mut i64,
    ) -> i64 {
        let end = begin;
        let mut begin = begin;
        let mut prefoffset: i64 = -1;
        let mut offset: i64 = -1;
        let mut ret_serialno: i64 = -1;
        let mut ret_gran: i64 = -1;

        while offset == -1 {
            begin -= CHUNKSIZE;
            if begin < 0 {
                begin = 0;
            }
            let ret = self.seek_helper(begin);
            if ret != 0 {
                return ret as i64;
            }
            while self.offset < end {
                let ret = self.get_next_page(end - self.offset);
                if ret == OV_EREAD as i64 {
                    return OV_EREAD as i64;
                }
                if ret < 0 {
                    break;
                }
                ret_serialno = self.page.serialno() as i64;
                ret_gran = self.page.granulepos();
                offset = ret;
                if ret_serialno == *serialno as i64 {
                    prefoffset = ret;
                    *granpos = ret_gran;
                }
                if !lookup_serialno(ret_serialno, serial_list) {
                    prefoffset = -1;
                }
            }
            if begin == 0 && self.offset < 0 {
                return OV_EBADLINK as i64;
            }
            if begin == 0 && offset == -1 {
                // The C rescans from 0 forever here (no page before
                // `end`); stop instead.
                return OV_EBADLINK as i64;
            }
        }
        if prefoffset >= 0 {
            return prefoffset;
        }
        *serialno = ret_serialno as i32;
        *granpos = ret_gran;
        offset
    }

    /// `_fetch_headers` (seekable use: always reads the first page itself).
    fn fetch_headers(
        &mut self,
        vi: &mut Info,
        vc: &mut Comment,
        mut serialno_list: Option<&mut Vec<i64>>,
    ) -> i32 {
        let llret = self.get_next_page(CHUNKSIZE);
        if llret == OV_EREAD as i64 {
            return OV_EREAD;
        }
        if llret < 0 {
            return OV_ENOTVORBIS;
        }

        *vi = Info::new();
        *vc = Comment::new();
        self.ready_state = OPENED;
        let mut allbos = false;

        let ret: i32 = 'bail: {
            while self.page.bos() {
                if let Some(list) = serialno_list.as_deref_mut() {
                    if lookup_serialno(self.page.serialno() as i64, list) {
                        list.clear();
                        break 'bail OV_EBADHEADER;
                    }
                    list.push(self.page.serialno() as i64);
                }

                if self.ready_state < STREAMSET {
                    self.os.reset_serialno(self.page.serialno());
                    self.os.pagein(&self.page);
                    if let (1, Some(loc)) = self.os.packetout_loc(true, true) {
                        let p = self.os.packet_at(&loc);
                        if synthesis_idheader(p.data, p.b_o_s) {
                            self.ready_state = STREAMSET;
                            if synthesis_headerin(vi, vc, p.data, p.b_o_s) != 0 {
                                break 'bail OV_EBADHEADER;
                            }
                        }
                    }
                }

                let llret = self.get_next_page(CHUNKSIZE);
                if llret == OV_EREAD as i64 {
                    break 'bail OV_EREAD;
                }
                if llret < 0 {
                    break 'bail OV_ENOTVORBIS;
                }
                if self.ready_state == STREAMSET && self.os.serialno() == self.page.serialno() {
                    self.os.pagein(&self.page);
                    break;
                }
            }

            if self.ready_state != STREAMSET {
                break 'bail OV_ENOTVORBIS;
            }

            let mut i = 0;
            while i < 2 {
                while i < 2 {
                    let (result, loc) = self.os.packetout_loc(true, true);
                    if result == 0 {
                        break;
                    }
                    if result == -1 {
                        break 'bail OV_EBADHEADER;
                    }
                    let Some(loc) = loc else {
                        break 'bail OV_EFAULT;
                    };
                    let p = self.os.packet_at(&loc);
                    let r = synthesis_headerin(vi, vc, p.data, p.b_o_s);
                    if r != 0 {
                        break 'bail r;
                    }
                    i += 1;
                }
                // get a page of this stream (the C's second `while(i<2)`,
                // which only leaves through `break` or an error)
                if i < 2 {
                    loop {
                        if self.get_next_page(CHUNKSIZE) < 0 {
                            break 'bail OV_EBADHEADER;
                        }
                        if self.os.serialno() == self.page.serialno() {
                            self.os.pagein(&self.page);
                            break;
                        }
                        if self.page.bos() {
                            if allbos {
                                break 'bail OV_EBADHEADER;
                            }
                            allbos = true;
                        }
                    }
                }
            }
            return 0;
        };

        vi.clear();
        *vc = Comment::default();
        self.ready_state = OPENED;
        ret
    }

    /// `_initial_pcmoffset`.
    fn initial_pcmoffset(&mut self, vi: &Info) -> i64 {
        let mut accumulated: i64 = 0;
        let mut lastblock: i64 = -1;
        let serialno = self.os.serialno();
        loop {
            if self.get_next_page(-1) < 0 {
                break;
            }
            if self.page.bos() {
                break;
            }
            if self.page.serialno() != serialno {
                continue;
            }
            self.os.pagein(&self.page);
            loop {
                let (result, loc) = self.os.packetout_loc(true, true);
                if result == 0 {
                    break;
                }
                if let (true, Some(loc)) = (result > 0, loc) {
                    {
                        let thisblock = packet_blocksize(vi, self.os.packet_at(&loc).data);
                        if thisblock >= 0 {
                            if lastblock != -1 {
                                accumulated =
                                    accumulated.wrapping_add((lastblock + thisblock) >> 2);
                            }
                            lastblock = thisblock;
                        }
                    }
                }
            }
            if self.page.granulepos() != -1 {
                accumulated = self.page.granulepos().wrapping_sub(accumulated);
                break;
            }
        }
        if accumulated < 0 {
            accumulated = 0;
        }
        accumulated
    }

    /// `_bisect_forward_serialno`, with the recursion unrolled into a loop
    /// (one level per link) so long chains cannot exhaust the stack.
    fn bisect_forward_serialno(
        &mut self,
        begin: i64,
        searched: i64,
        end: i64,
        endgran: i64,
        endserial: i32,
        currentno_list: Vec<i64>,
        m: i64,
    ) -> i32 {
        let mut begin = begin;
        let mut searched = searched;
        let mut currentno_list = currentno_list;
        let mut m = m;
        let m0 = m;
        let mut levels: Vec<Level> = Vec::new();

        loop {
            let serialno = self.os.serialno();
            let mut endsearched = end;
            let mut next = end;
            let mut searchgran: i64 = -1;

            if lookup_serialno(endserial as i64, &currentno_list) {
                searched = end;
                let mut es = endserial;
                let mut eg = endgran;
                while es != serialno {
                    es = serialno;
                    searched =
                        self.get_prev_page_serial(searched, &currentno_list, &mut es, &mut eg);
                }
                let _ = searched;
                let links = (m + 1) as usize;
                self.links = links as i32;
                self.offsets = vec![0; links + 1];
                self.vi.resize_with(links, Info::default);
                self.vc.resize_with(links, Comment::default);
                self.serialnos = vec![0; links];
                self.dataoffsets = vec![0; links];
                self.pcmlengths = vec![0; links * 2];
                self.offsets[m as usize + 1] = end;
                self.offsets[m as usize] = begin;
                self.pcmlengths[m as usize * 2 + 1] = if eg < 0 { 0 } else { eg };
                break;
            }

            let mut testserial = serialno.wrapping_add(1);
            while searched < endsearched {
                let bisect = if endsearched - searched < CHUNKSIZE {
                    searched
                } else {
                    (searched + endsearched) / 2
                };
                let ret = self.seek_helper(bisect);
                if ret != 0 {
                    return ret;
                }
                let last = self.get_next_page(-1);
                if last == OV_EREAD as i64 {
                    return OV_EREAD;
                }
                if last < 0 || !lookup_serialno(self.page.serialno() as i64, &currentno_list) {
                    endsearched = bisect;
                    if last >= 0 {
                        next = last;
                    }
                } else {
                    searched = self.offset;
                }
            }

            searched = next;
            while testserial != serialno {
                testserial = serialno;
                searched = self.get_prev_page_serial(
                    searched,
                    &currentno_list,
                    &mut testserial,
                    &mut searchgran,
                );
            }

            let ret = self.seek_helper(next);
            if ret != 0 {
                return ret;
            }
            let mut vi = Info::default();
            let mut vc = Comment::default();
            let mut next_list: Vec<i64> = Vec::new();
            let ret = self.fetch_headers(&mut vi, &mut vc, Some(&mut next_list));
            if ret != 0 {
                return ret;
            }
            let serialno = self.os.serialno() as i64;
            let dataoffset = self.offset;
            let pcmoffset = self.initial_pcmoffset(&vi);
            levels.push(Level {
                next,
                serialno,
                dataoffset,
                vi,
                vc,
                searchgran,
                pcmoffset,
            });

            begin = next;
            searched = self.offset;
            currentno_list = next_list;
            m += 1;
        }

        // unwind, deepest level first
        for (k, lv) in levels.into_iter().enumerate().rev() {
            let m = (m0 + k as i64) as usize;
            self.offsets[m + 1] = lv.next;
            self.serialnos[m + 1] = lv.serialno;
            self.dataoffsets[m + 1] = lv.dataoffset;
            self.vi[m + 1] = lv.vi;
            self.vc[m + 1] = lv.vc;
            self.pcmlengths[m * 2 + 1] = lv.searchgran;
            self.pcmlengths[m * 2 + 2] = lv.pcmoffset;
            self.pcmlengths[m * 2 + 3] = self.pcmlengths[m * 2 + 3].wrapping_sub(lv.pcmoffset);
            if self.pcmlengths[m * 2 + 3] < 0 {
                self.pcmlengths[m * 2 + 3] = 0;
            }
        }
        0
    }

    /// `_open_seekable2`.
    fn open_seekable2(&mut self) -> i32 {
        let dataoffset = self.dataoffsets[0];
        let mut endgran: i64 = -1;
        let mut endserial = self.os.serialno();
        let serialno = self.os.serialno();

        let vi0 = std::mem::take(&mut self.vi[0]);
        let pcmoffset = self.initial_pcmoffset(&vi0);
        self.vi[0] = vi0;

        self.src.seek(0, SEEK_END);
        self.end = self.src.tell();
        self.offset = self.end;
        if self.end == -1 {
            return OV_EINVAL;
        }

        let n = self.serialnos[1] as usize;
        let list: Vec<i64> = self.serialnos[2..2 + n].to_vec();
        let end = self.get_prev_page_serial(self.end, &list, &mut endserial, &mut endgran);
        if end < 0 {
            return end as i32;
        }

        if self.bisect_forward_serialno(0, dataoffset, end, endgran, endserial, list, 0) < 0 {
            return OV_EREAD;
        }

        self.offsets[0] = 0;
        self.serialnos[0] = serialno as i64;
        self.dataoffsets[0] = dataoffset;
        self.pcmlengths[0] = pcmoffset;
        self.pcmlengths[1] = self.pcmlengths[1].wrapping_sub(pcmoffset);
        if self.pcmlengths[1] < 0 {
            self.pcmlengths[1] = 0;
        }
        self.raw_seek(dataoffset)
    }

    /// `_make_decode_ready`.
    fn make_decode_ready(&mut self) -> i32 {
        if self.ready_state > STREAMSET {
            return 0;
        }
        if self.ready_state < STREAMSET {
            return OV_EFAULT;
        }
        match DspState::init(&mut self.vi[self.current_link as usize]) {
            Some(vd) => self.vd = Some(vd),
            None => return OV_EBADLINK,
        }
        self.ready_state = INITSET;
        self.bittrack = 0.0;
        self.samptrack = 0.0;
        0
    }

    /// `_decode_clear`.
    fn decode_clear(&mut self) {
        self.vd = None;
        self.ready_state = OPENED;
    }

    /// `ov_raw_seek`: 0 or a negative `OV_*` code.
    pub fn raw_seek(&mut self, pos: i64) -> i32 {
        if self.ready_state < OPENED {
            return OV_EINVAL;
        }
        if pos < 0 || pos > self.end {
            return OV_EINVAL;
        }
        if self.ready_state >= STREAMSET {
            let cl = self.current_link as usize;
            if pos < self.offsets[cl] || pos >= self.offsets[cl + 1] {
                self.decode_clear();
            }
        }

        self.pcm_offset = -1;
        self.os.reset_serialno(self.current_serialno as i32);
        if let (Some(vd), Some(ci)) = (
            self.vd.as_mut(),
            self.vi[self.current_link as usize].codec_setup.as_deref(),
        ) {
            vd.restart(ci);
        }

        if self.seek_helper(pos) != 0 {
            self.pcm_offset = -1;
            self.decode_clear();
            return OV_EBADLINK;
        }

        let mut work_os = StreamState::new(self.current_serialno as i32);
        work_os.reset();
        let mut lastblock: i32 = 0;
        let mut accblock: i32 = 0;
        let mut lastflag = false;
        let mut firstflag = false;
        let mut pagepos: i64;

        loop {
            if self.ready_state >= STREAMSET {
                let (result, loc) = work_os.packetout_loc(true, true);
                if result > 0 {
                    let link = self.current_link as usize;
                    if self.vi[link].codec_setup.is_some() {
                        let loc = loc.expect("packet location");
                        let op = work_os.packet_at(&loc);
                        let mut thisblock = packet_blocksize(&self.vi[link], op.data) as i32;
                        if thisblock < 0 {
                            self.os.packetout_skip();
                            thisblock = 0;
                        } else if lastflag && !firstflag {
                            self.os.packetout_skip();
                        } else if lastblock != 0 {
                            accblock = accblock.wrapping_add((lastblock + thisblock) >> 2);
                        }

                        if op.granulepos != -1 {
                            let mut granulepos =
                                op.granulepos.wrapping_sub(self.pcmlengths[link * 2]);
                            if granulepos < 0 {
                                granulepos = 0;
                            }
                            for i in 0..link {
                                granulepos = granulepos.wrapping_add(self.pcmlengths[i * 2 + 1]);
                            }
                            self.pcm_offset = granulepos.wrapping_sub(accblock as i64);
                            if self.pcm_offset < 0 {
                                self.pcm_offset = 0;
                            }
                            break;
                        }
                        lastblock = thisblock;
                        continue;
                    } else {
                        self.os.packetout_skip();
                    }
                }
            }

            if lastblock == 0 {
                pagepos = self.get_next_page(-1);
                if pagepos < 0 {
                    self.pcm_offset = self.pcm_total(-1);
                    break;
                }
            } else {
                // bogus stream with packets but no granulepos
                self.pcm_offset = -1;
                break;
            }

            if self.ready_state >= STREAMSET
                && self.current_serialno != self.page.serialno() as i64
                && self.page.bos()
            {
                self.decode_clear();
                work_os.clear();
            }

            if self.ready_state < STREAMSET {
                let serialno = self.page.serialno() as i64;
                let Some(link) = (0..self.links as usize).find(|&l| self.serialnos[l] == serialno)
                else {
                    continue;
                };
                self.current_link = link as i32;
                self.current_serialno = serialno;
                self.os.reset_serialno(serialno as i32);
                work_os.reset_serialno(serialno as i32);
                self.ready_state = STREAMSET;
                firstflag = pagepos <= self.dataoffsets[link];
            }

            self.os.pagein(&self.page);
            work_os.pagein(&self.page);
            lastflag = self.page.eos();
        }

        self.bittrack = 0.0;
        self.samptrack = 0.0;
        0
    }

    /// `_fetch_and_process_packet(vf, NULL, 1, 1)`: 1 a packet was
    /// decoded, or a negative `OV_*` code (`OV_EOF` at the end).
    fn fetch_and_process_packet(&mut self) -> i32 {
        loop {
            if self.ready_state == STREAMSET {
                let ret = self.make_decode_ready();
                if ret < 0 {
                    return ret;
                }
            }

            if self.ready_state == INITSET {
                loop {
                    let (result, loc) = self.os.packetout_loc(true, true);
                    if result == -1 {
                        return OV_HOLE;
                    }
                    if result <= 0 {
                        break;
                    }
                    let Some(loc) = loc else { return OV_EFAULT };
                    let link = self.current_link as usize;
                    let (Some(vd), Some(ci)) =
                        (self.vd.as_mut(), self.vi[link].codec_setup.as_deref())
                    else {
                        return OV_EFAULT;
                    };
                    let op = self.os.packet_at(&loc);
                    let granulepos0 = op.granulepos;
                    if vd.synthesis(ci, op.data, op.granulepos, op.packetno, op.e_o_s) == 0 {
                        let oldsamples = vd.pcmout_count();
                        if oldsamples != 0 {
                            return OV_EFAULT;
                        }
                        vd.blockin(ci);
                        self.samptrack += vd.pcmout_count() as f64;
                        self.bittrack += (op.data.len() * 8) as f64;

                        if granulepos0 != -1 && !op.e_o_s {
                            let mut granulepos = granulepos0;
                            if link > 0 {
                                granulepos = granulepos.wrapping_sub(self.pcmlengths[link * 2]);
                            }
                            if granulepos < 0 {
                                granulepos = 0;
                            }
                            let samples = vd.pcmout_count();
                            granulepos = granulepos.wrapping_sub(samples as i64);
                            for i in 0..link {
                                granulepos = granulepos.wrapping_add(self.pcmlengths[i * 2 + 1]);
                            }
                            self.pcm_offset = granulepos;
                        }
                        return 1;
                    }
                }
            }

            if self.ready_state >= OPENED {
                loop {
                    if self.get_next_page(-1) < 0 {
                        return OV_EOF;
                    }
                    self.bittrack += (self.page.header_len() * 8) as f64;
                    if self.ready_state == INITSET
                        && self.current_serialno != self.page.serialno() as i64
                    {
                        if self.page.bos() {
                            self.decode_clear();
                            break;
                        }
                        continue;
                    }
                    break;
                }
            }

            if self.ready_state != INITSET && self.ready_state < STREAMSET {
                let serialno = self.page.serialno() as i64;
                let Some(link) = (0..self.links as usize).find(|&l| self.serialnos[l] == serialno)
                else {
                    continue;
                };
                self.current_serialno = serialno;
                self.current_link = link as i32;
                self.os.reset_serialno(serialno as i32);
                self.ready_state = STREAMSET;
            }

            self.os.pagein(&self.page);
        }
    }

    // ---------------------------------------------------------------
    // public API

    /// `ov_streams`: the number of logical bitstreams (links).
    pub fn streams(&self) -> i32 {
        self.links
    }

    /// `ov_pcm_total`: the PCM length of link `i` in samples, or of the
    /// whole file for `i < 0`; `OV_EINVAL` for a link past the end.
    pub fn pcm_total(&self, i: i32) -> i64 {
        if self.ready_state < OPENED {
            return OV_EINVAL as i64;
        }
        if i >= self.links {
            return OV_EINVAL as i64;
        }
        if i < 0 {
            let mut acc: i64 = 0;
            for l in 0..self.links as usize {
                acc = acc.wrapping_add(self.pcmlengths[l * 2 + 1]);
            }
            acc
        } else {
            self.pcmlengths[i as usize * 2 + 1]
        }
    }

    /// `ov_raw_total`.
    pub fn raw_total(&self, i: i32) -> i64 {
        if self.ready_state < OPENED || i >= self.links {
            return OV_EINVAL as i64;
        }
        if i < 0 {
            (0..self.links as usize)
                .map(|l| self.offsets[l + 1] - self.offsets[l])
                .sum()
        } else {
            self.offsets[i as usize + 1] - self.offsets[i as usize]
        }
    }

    /// `ov_info`: link `link`, or for `link < 0` the link being decoded.
    pub fn info(&self, link: i32) -> Option<&Info> {
        if link < 0 {
            if self.ready_state >= STREAMSET {
                self.vi.get(self.current_link as usize)
            } else {
                self.vi.first()
            }
        } else if link >= self.links {
            None
        } else {
            self.vi.get(link as usize)
        }
    }

    /// `ov_comment`, as [`VorbisFile::info`].
    pub fn comment(&self, link: i32) -> Option<&Comment> {
        if link < 0 {
            if self.ready_state >= STREAMSET {
                self.vc.get(self.current_link as usize)
            } else {
                self.vc.first()
            }
        } else if link >= self.links {
            None
        } else {
            self.vc.get(link as usize)
        }
    }

    /// `ov_serialnumber`.
    pub fn serialnumber(&self, i: i32) -> i64 {
        if i >= self.links {
            return self.serialnumber(self.links - 1);
        }
        if i < 0 {
            self.current_serialno
        } else {
            self.serialnos[i as usize]
        }
    }

    /// `ov_pcm_tell`.
    pub fn pcm_tell(&self) -> i64 {
        if self.ready_state < OPENED {
            OV_EINVAL as i64
        } else {
            self.pcm_offset
        }
    }

    /// `ov_read`: decodes and packs up to `out.len()` bytes of interleaved
    /// integer PCM (`word` 1 = 8-bit, 2 = 16-bit; `bigendian`, `signed` as
    /// the C flags). Returns the byte count, 0 at the end, or a negative
    /// `OV_*` code (`OV_HOLE` for a gap, after which reading may continue).
    /// `bitstream` receives the link being decoded (on success only).
    pub fn read(
        &mut self,
        out: &mut [u8],
        bigendian: bool,
        word: usize,
        signed: bool,
        bitstream: &mut i32,
    ) -> isize {
        if self.ready_state < OPENED {
            return OV_EINVAL as isize;
        }
        if word == 0 || word > i32::MAX as usize {
            return OV_EINVAL as isize;
        }
        let length = out.len().min(i32::MAX as usize) as i64;
        let mut samples: i64;
        loop {
            if self.ready_state == INITSET {
                samples = self.vd.as_ref().map_or(0, |d| d.pcmout_count() as i64);
                if samples != 0 {
                    break;
                }
            }
            let ret = self.fetch_and_process_packet();
            if ret == OV_EOF {
                return 0;
            }
            if ret <= 0 {
                return ret as isize;
            }
        }

        if samples <= 0 {
            return samples as isize;
        }
        let channels = self.info(-1).map_or(0, |i| i.channels) as i64;
        let bytespersample = word as i64 * channels;
        if !(1..=255).contains(&channels) {
            return OV_EINVAL as isize;
        }
        if samples > length / bytespersample {
            samples = length / bytespersample;
        }
        if samples <= 0 {
            return OV_EINVAL as isize;
        }
        let Some(vd) = self.vd.as_mut() else {
            return OV_EFAULT as isize;
        };
        let n = samples as usize;
        let ch = channels as usize;

        if word == 1 {
            let off: i32 = if signed { 0 } else { 128 };
            for c in 0..ch {
                let src = &vd.pcm_channel(c)[..n];
                for (j, &s) in src.iter().enumerate() {
                    let val = vorbis_ftoi((s * 128.0f32) as f64).clamp(-128, 127);
                    out[j * ch + c] = (val + off) as u8;
                }
            }
        } else {
            let off: i32 = if signed { 0 } else { 32768 };
            // Every word size above 1 packs 16-bit samples, `channels`
            // shorts per frame, as the C does.
            for c in 0..ch {
                let src = &vd.pcm_channel(c)[..n];
                for (j, &s) in src.iter().enumerate() {
                    let val = vorbis_ftoi((s * 32768.0f32) as f64).clamp(-32768, 32767) + off;
                    let o = (j * ch + c) * 2;
                    if bigendian {
                        out[o] = (val >> 8) as u8;
                        out[o + 1] = val as u8;
                    } else {
                        out[o] = val as u8;
                        out[o + 1] = (val >> 8) as u8;
                    }
                }
            }
        }

        vd.read(samples as i32);
        self.pcm_offset = self.pcm_offset.wrapping_add(samples);
        *bitstream = self.current_link;
        (samples * bytespersample) as isize
    }
}

#[cfg(test)]
mod tests {
    use super::vorbis_ftoi;

    #[test]
    fn ftoi_matches_cvtsd2si() {
        assert_eq!(vorbis_ftoi(0.5), 0);
        assert_eq!(vorbis_ftoi(1.5), 2);
        assert_eq!(vorbis_ftoi(-0.5), 0);
        assert_eq!(vorbis_ftoi(-1.5), -2);
        assert_eq!(vorbis_ftoi(2.5), 2);
        assert_eq!(vorbis_ftoi(f64::NAN), i32::MIN);
        assert_eq!(vorbis_ftoi(3e9), i32::MIN);
        assert_eq!(vorbis_ftoi(-3e9), i32::MIN);
        assert_eq!(vorbis_ftoi(f64::INFINITY), i32::MIN);
    }
}
