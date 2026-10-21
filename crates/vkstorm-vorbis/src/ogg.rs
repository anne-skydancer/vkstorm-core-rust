//! Ogg framing, decode side: a port of libogg 1.3.5's `framing.c`
//! (`ogg_sync_*`, `ogg_stream_*` pagein/packetout, the page accessors and
//! the page CRC).
//!
//! The behaviour, return values and state transitions follow the C function
//! for function, including its tolerance of damaged input (lost sync,
//! CRC mismatches, sequence gaps that surface as holes). Pages handed out by
//! [`SyncState::pageout`] are copies (the C hands out pointers into the sync
//! buffer); the bytes are the same.
//!
//! Original code: Copyright (c) 1994-2018 Xiph.Org Foundation, BSD-3-Clause
//! (see `LICENSE-xiph`). Rust port: Vulkanstorm.

use crate::clong;

/// The CRC lookup tables of `crctable.h` (polynomial 0x04c11db7, unreflected,
/// initial and final value 0), generated at compile time; slicing-by-8 as in
/// `_os_update_crc`.
static CRC_LOOKUP: [[u32; 256]; 8] = make_crc_tables();

const fn make_crc_tables() -> [[u32; 256]; 8] {
    let mut t = [[0u32; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u32) << 24;
        let mut j = 0;
        while j < 8 {
            crc = (crc << 1) ^ if crc & (1 << 31) != 0 { 0x04c1_1db7 } else { 0 };
            j += 1;
        }
        t[0][i] = crc;
        i += 1;
    }
    let mut i = 0;
    while i < 256 {
        let mut j = 1;
        while j < 8 {
            t[j][i] = t[0][((t[j - 1][i] >> 24) & 0xff) as usize] ^ (t[j - 1][i] << 8);
            j += 1;
        }
        i += 1;
    }
    t
}

fn update_crc(mut crc: u32, mut buf: &[u8]) -> u32 {
    while buf.len() >= 8 {
        crc ^= u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
        crc = CRC_LOOKUP[7][(crc >> 24) as usize]
            ^ CRC_LOOKUP[6][((crc >> 16) & 0xff) as usize]
            ^ CRC_LOOKUP[5][((crc >> 8) & 0xff) as usize]
            ^ CRC_LOOKUP[4][(crc & 0xff) as usize]
            ^ CRC_LOOKUP[3][buf[4] as usize]
            ^ CRC_LOOKUP[2][buf[5] as usize]
            ^ CRC_LOOKUP[1][buf[6] as usize]
            ^ CRC_LOOKUP[0][buf[7] as usize];
        buf = &buf[8..];
    }
    for &b in buf {
        crc = (crc << 8) ^ CRC_LOOKUP[0][(((crc >> 24) & 0xff) as u8 ^ b) as usize];
    }
    crc
}

/// The page checksum with the checksum field taken as zero
/// (`ogg_page_checksum_set` without writing it back).
fn page_crc(header: &[u8], body: &[u8]) -> u32 {
    let mut crc = update_crc(0, &header[..22]);
    crc = update_crc(crc, &[0, 0, 0, 0]);
    crc = update_crc(crc, &header[26..]);
    update_crc(crc, body)
}

/// One Ogg page (`ogg_page`): the header (27 bytes plus the segment table)
/// followed by the body, owned.
#[derive(Clone, Debug, Default)]
pub struct Page {
    data: Vec<u8>,
    header_len: usize,
}

impl Page {
    /// An empty page (`header_len == 0`, as after `memset(og,0,...)`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a page from its raw bytes (header then body); the caller
    /// guarantees the layout (used by tests and tools).
    pub fn from_parts(header: &[u8], body: &[u8]) -> Self {
        let mut data = Vec::with_capacity(header.len() + body.len());
        data.extend_from_slice(header);
        data.extend_from_slice(body);
        Self {
            data,
            header_len: header.len(),
        }
    }

    fn set(&mut self, header: &[u8], body: &[u8]) {
        self.data.clear();
        self.data.extend_from_slice(header);
        self.data.extend_from_slice(body);
        self.header_len = header.len();
    }

    /// The page header bytes.
    pub fn header(&self) -> &[u8] {
        &self.data[..self.header_len]
    }
    /// The page body bytes.
    pub fn body(&self) -> &[u8] {
        &self.data[self.header_len..]
    }
    /// `og->header_len`.
    pub fn header_len(&self) -> usize {
        self.header_len
    }
    /// `og->body_len`.
    pub fn body_len(&self) -> usize {
        self.data.len() - self.header_len
    }
    /// `ogg_page_version`.
    pub fn version(&self) -> i32 {
        self.data[4] as i32
    }
    /// `ogg_page_continued` (non-zero: the page continues a packet).
    pub fn continued(&self) -> bool {
        self.data[5] & 0x01 != 0
    }
    /// `ogg_page_bos`.
    pub fn bos(&self) -> bool {
        self.data[5] & 0x02 != 0
    }
    /// `ogg_page_eos`.
    pub fn eos(&self) -> bool {
        self.data[5] & 0x04 != 0
    }
    /// `ogg_page_granulepos`.
    pub fn granulepos(&self) -> i64 {
        i64::from_le_bytes(self.data[6..14].try_into().unwrap_or([0; 8]))
    }
    /// `ogg_page_serialno` (a C `int`).
    pub fn serialno(&self) -> i32 {
        i32::from_le_bytes(self.data[14..18].try_into().unwrap_or([0; 4]))
    }
    /// `ogg_page_pageno`, with the C `long` width of the platform (the
    /// sequence number wraps to negative above 2^31 on Windows).
    pub fn pageno(&self) -> i64 {
        clong(u32::from_le_bytes(self.data[18..22].try_into().unwrap_or([0; 4])) as i64)
    }
    /// `ogg_page_packets`: packets completed on this page.
    pub fn packets(&self) -> i32 {
        let n = self.data[26] as usize;
        self.data[27..27 + n].iter().filter(|&&v| v < 255).count() as i32
    }
}

/// `ogg_sync_state`: buffers raw bytes and finds pages in them.
#[derive(Debug, Default)]
pub struct SyncState {
    /// The buffer; `storage` is its length.
    data: Vec<u8>,
    fill: usize,
    returned: usize,
    unsynced: bool,
    headerbytes: usize,
    bodybytes: usize,
    /// `storage < 0`: the state was cleared after a failed allocation
    /// check (`ogg_sync_check` fails).
    broken: bool,
}

impl SyncState {
    /// `ogg_sync_init`.
    pub fn new() -> Self {
        Self::default()
    }

    /// `ogg_sync_clear`, then init (`memset` 0 leaves a usable state in C).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// `ogg_sync_reset`.
    pub fn reset(&mut self) {
        if self.broken {
            return;
        }
        self.fill = 0;
        self.returned = 0;
        self.unsynced = false;
        self.headerbytes = 0;
        self.bodybytes = 0;
    }

    /// `ogg_sync_buffer`: exposes at least `size` writable bytes at the fill
    /// mark; `None` when the C would return NULL.
    pub fn buffer(&mut self, size: usize) -> Option<&mut [u8]> {
        if self.broken {
            return None;
        }
        if self.returned != 0 {
            self.fill -= self.returned;
            if self.fill > 0 {
                self.data
                    .copy_within(self.returned..self.returned + self.fill, 0);
            }
            self.returned = 0;
        }
        if size > self.data.len() - self.fill {
            if size > (i32::MAX as usize).saturating_sub(4096 + self.fill) {
                self.clear();
                self.broken = true;
                return None;
            }
            let newsize = size + self.fill + 4096;
            self.data.resize(newsize, 0);
        }
        let fill = self.fill;
        Some(&mut self.data[fill..fill + size])
    }

    /// `ogg_sync_wrote`: 0, or -1 when `bytes` exceeds the exposed space.
    pub fn wrote(&mut self, bytes: usize) -> i32 {
        if self.broken {
            return -1;
        }
        if self.fill + bytes > self.data.len() {
            return -1;
        }
        self.fill += bytes;
        0
    }

    /// Convenience: `ogg_sync_buffer` + copy + `ogg_sync_wrote`.
    pub fn write(&mut self, bytes: &[u8]) -> i32 {
        match self.buffer(bytes.len()) {
            Some(buf) => {
                buf.copy_from_slice(bytes);
                self.wrote(bytes.len())
            }
            None => -1,
        }
    }

    /// `ogg_sync_pageseek`: `-n` skipped n bytes, 0 needs more data, `n` a
    /// page of n bytes was found (copied into `og` when given).
    pub fn pageseek(&mut self, og: Option<&mut Page>) -> i64 {
        if self.broken {
            return 0;
        }
        let page_start = self.returned;
        let bytes = self.fill - self.returned;

        // `goto sync_fail` paths fall through to the capture search below
        let synced = 'check: {
            if self.headerbytes == 0 {
                if bytes < 27 {
                    return 0;
                }
                if &self.data[page_start..page_start + 4] != b"OggS" {
                    break 'check false;
                }
                let headerbytes = self.data[page_start + 26] as usize + 27;
                if bytes < headerbytes {
                    return 0;
                }
                for i in 0..self.data[page_start + 26] as usize {
                    self.bodybytes += self.data[page_start + 27 + i] as usize;
                }
                self.headerbytes = headerbytes;
            }
            if self.bodybytes + self.headerbytes > bytes {
                return 0;
            }
            let header = &self.data[page_start..page_start + self.headerbytes];
            let body = &self.data
                [page_start + self.headerbytes..page_start + self.headerbytes + self.bodybytes];
            let stored = u32::from_le_bytes([header[22], header[23], header[24], header[25]]);
            page_crc(header, body) == stored
        };

        if synced {
            if let Some(og) = og {
                let header = &self.data[page_start..page_start + self.headerbytes];
                let body = &self.data
                    [page_start + self.headerbytes..page_start + self.headerbytes + self.bodybytes];
                og.set(header, body);
            }
            self.unsynced = false;
            let n = self.headerbytes + self.bodybytes;
            self.returned += n;
            self.headerbytes = 0;
            self.bodybytes = 0;
            return n as i64;
        }

        // sync_fail: search for a possible capture after the first byte.
        self.headerbytes = 0;
        self.bodybytes = 0;
        let search = &self.data[page_start + 1..page_start + bytes];
        let next = match search.iter().position(|&b| b == b'O') {
            Some(p) => page_start + 1 + p,
            None => self.fill,
        };
        self.returned = next;
        -((next - page_start) as i64)
    }

    /// `ogg_sync_pageout`: 1 page returned, 0 need more data, -1 lost sync
    /// (reported once; later skips are silent).
    pub fn pageout(&mut self, og: &mut Page) -> i32 {
        if self.broken {
            return 0;
        }
        loop {
            let ret = self.pageseek(Some(og));
            if ret > 0 {
                return 1;
            }
            if ret == 0 {
                return 0;
            }
            if !self.unsynced {
                self.unsynced = true;
                return -1;
            }
        }
    }
}

/// A packet from [`StreamState::packetout`] (`ogg_packet`), borrowing the
/// stream's body buffer.
#[derive(Clone, Copy, Debug)]
pub struct Packet<'a> {
    /// The packet bytes.
    pub data: &'a [u8],
    /// First packet of the logical stream.
    pub b_o_s: bool,
    /// Last packet of the logical stream.
    pub e_o_s: bool,
    /// Granule position of the page the packet ends on, -1 when the packet
    /// is not the last one completed on its page.
    pub granulepos: i64,
    /// Sequence number (counts holes).
    pub packetno: i64,
}

/// Result of [`StreamState::packetout`].
#[derive(Debug)]
pub enum PacketOut<'a> {
    /// No complete packet buffered (C return 0).
    None,
    /// A gap in the data (C return -1; the gap marker is consumed).
    Hole,
    /// A packet (C return 1).
    Packet(Packet<'a>),
}

/// Location of a packet inside the stream state's buffer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PacketLoc {
    pub start: usize,
    pub len: usize,
    pub b_o_s: bool,
    pub e_o_s: bool,
    pub granulepos: i64,
    pub packetno: i64,
}

/// `ogg_stream_state`, decode side: reassembles packets of one logical
/// stream from its pages.
#[derive(Debug)]
pub struct StreamState {
    body: Vec<u8>,
    body_returned: usize,
    lacing_vals: Vec<i32>,
    granule_vals: Vec<i64>,
    lacing_packet: usize,
    lacing_returned: usize,
    e_o_s: bool,
    serialno: i32,
    pageno: i64,
    packetno: i64,
    /// `ogg_stream_check` passes (false after `ogg_stream_clear`).
    valid: bool,
}

impl StreamState {
    /// `ogg_stream_init`.
    pub fn new(serialno: i32) -> Self {
        Self {
            body: Vec::with_capacity(16 * 1024),
            body_returned: 0,
            lacing_vals: Vec::with_capacity(1024),
            granule_vals: Vec::with_capacity(1024),
            lacing_packet: 0,
            lacing_returned: 0,
            e_o_s: false,
            serialno,
            pageno: 0,
            packetno: 0,
            valid: true,
        }
    }

    /// `ogg_stream_clear`: the state becomes unusable (every call fails as
    /// `ogg_stream_check` would) until re-initialised.
    pub fn clear(&mut self) {
        *self = Self::new(0);
        self.valid = false;
    }

    /// `ogg_stream_check == 0`.
    pub fn is_valid(&self) -> bool {
        self.valid
    }

    /// The serial number this state accepts (`os->serialno`).
    pub fn serialno(&self) -> i32 {
        self.serialno
    }

    /// `ogg_stream_eos`.
    pub fn eos(&self) -> bool {
        !self.valid || self.e_o_s
    }

    /// `ogg_stream_reset`.
    pub fn reset(&mut self) -> i32 {
        if !self.valid {
            return -1;
        }
        self.body.clear();
        self.body_returned = 0;
        self.lacing_vals.clear();
        self.granule_vals.clear();
        self.lacing_packet = 0;
        self.lacing_returned = 0;
        self.e_o_s = false;
        self.pageno = -1;
        self.packetno = 0;
        0
    }

    /// `ogg_stream_reset_serialno`.
    pub fn reset_serialno(&mut self, serialno: i32) -> i32 {
        if !self.valid {
            return -1;
        }
        self.reset();
        self.serialno = serialno;
        0
    }

    /// `ogg_stream_pagein`: 0, or -1 (wrong serial number or version, or an
    /// unusable state).
    pub fn pagein(&mut self, og: &Page) -> i32 {
        if !self.valid {
            return -1;
        }
        let header = og.header();
        let mut body = og.body();
        let version = og.version();
        let continued = og.continued();
        let mut bos = og.bos();
        let eos = og.eos();
        let granulepos = og.granulepos();
        let serialno = og.serialno();
        let pageno = og.pageno();
        let segments = header[26] as usize;
        let mut segptr = 0usize;

        // clean up 'returned data'
        {
            let lr = self.lacing_returned;
            let br = self.body_returned;
            if br != 0 {
                self.body.drain(..br);
                self.body_returned = 0;
            }
            if lr != 0 {
                self.lacing_vals.drain(..lr);
                self.granule_vals.drain(..lr);
                self.lacing_packet -= lr;
                self.lacing_returned = 0;
            }
        }

        if serialno != self.serialno {
            return -1;
        }
        if version > 0 {
            return -1;
        }

        // are we in sequence?
        if pageno != self.pageno {
            // unroll previous partial packet (if any)
            let mut unroll = 0usize;
            for &v in &self.lacing_vals[self.lacing_packet..] {
                unroll += (v & 0xff) as usize;
            }
            let new_len = self.body.len() - unroll;
            self.body.truncate(new_len);
            self.lacing_vals.truncate(self.lacing_packet);
            self.granule_vals.truncate(self.lacing_packet);
            // make a note of dropped data in segment table
            if self.pageno != -1 {
                self.lacing_vals.push(0x400);
                self.granule_vals.push(-1);
                self.lacing_packet += 1;
            }
        }

        // a 'continued packet' page: we may need to skip some segments
        if continued {
            let fill = self.lacing_vals.len();
            if fill < 1
                || (self.lacing_vals[fill - 1] & 0xff) < 255
                || self.lacing_vals[fill - 1] == 0x400
            {
                bos = false;
                while segptr < segments {
                    let val = header[27 + segptr] as usize;
                    body = &body[val.min(body.len())..];
                    if val < 255 {
                        segptr += 1;
                        break;
                    }
                    segptr += 1;
                }
            }
        }

        if !body.is_empty() {
            self.body.extend_from_slice(body);
        }

        {
            let mut saved: Option<usize> = None;
            while segptr < segments {
                let val = header[27 + segptr] as i32;
                let mut lv = val;
                if bos {
                    lv |= 0x100;
                    bos = false;
                }
                self.lacing_vals.push(lv);
                self.granule_vals.push(-1);
                if val < 255 {
                    saved = Some(self.lacing_vals.len() - 1);
                }
                segptr += 1;
                if val < 255 {
                    self.lacing_packet = self.lacing_vals.len();
                }
            }
            // set the granulepos on the last granuleval of the last full packet
            if let Some(s) = saved {
                self.granule_vals[s] = granulepos;
            }
        }

        if eos {
            self.e_o_s = true;
            if let Some(last) = self.lacing_vals.last_mut() {
                *last |= 0x200;
            }
        }

        self.pageno = clong(pageno.wrapping_add(1));
        0
    }

    /// `_packetout`: 0 none, -1 hole, 1 packet (with its location when
    /// `want` is set).
    pub(crate) fn packetout_loc(&mut self, want: bool, adv: bool) -> (i32, Option<PacketLoc>) {
        if !self.valid {
            return (0, None);
        }
        let mut ptr = self.lacing_returned;
        if self.lacing_packet <= ptr {
            return (0, None);
        }
        if self.lacing_vals[ptr] & 0x400 != 0 {
            self.lacing_returned += 1;
            self.packetno += 1;
            return (-1, None);
        }
        if !want && !adv {
            return (1, None);
        }
        let mut size = self.lacing_vals[ptr] & 0xff;
        let mut bytes = size as usize;
        let mut eos = self.lacing_vals[ptr] & 0x200 != 0;
        let bos = self.lacing_vals[ptr] & 0x100 != 0;
        while size == 255 {
            ptr += 1;
            let val = self.lacing_vals[ptr];
            size = val & 0xff;
            if val & 0x200 != 0 {
                eos = true;
            }
            bytes += size as usize;
        }
        let loc = PacketLoc {
            start: self.body_returned,
            len: bytes,
            b_o_s: bos,
            e_o_s: eos,
            granulepos: self.granule_vals[ptr],
            packetno: self.packetno,
        };
        if adv {
            self.body_returned += bytes;
            self.lacing_returned = ptr + 1;
            self.packetno += 1;
        }
        (1, Some(loc))
    }

    pub(crate) fn packet_at(&self, loc: &PacketLoc) -> Packet<'_> {
        Packet {
            data: &self.body[loc.start..loc.start + loc.len],
            b_o_s: loc.b_o_s,
            e_o_s: loc.e_o_s,
            granulepos: loc.granulepos,
            packetno: loc.packetno,
        }
    }

    /// `ogg_stream_packetout`.
    pub fn packetout(&mut self) -> PacketOut<'_> {
        match self.packetout_loc(true, true) {
            (1, Some(loc)) => PacketOut::Packet(self.packet_at(&loc)),
            (-1, _) => PacketOut::Hole,
            _ => PacketOut::None,
        }
    }

    /// `ogg_stream_packetout(os, NULL)`: advances past the next packet (or
    /// hole) without returning it; returns the C result.
    pub fn packetout_skip(&mut self) -> i32 {
        self.packetout_loc(false, true).0
    }

    /// `ogg_stream_packetpeek`.
    pub fn packetpeek(&mut self) -> PacketOut<'_> {
        match self.packetout_loc(true, false) {
            (1, Some(loc)) => PacketOut::Packet(self.packet_at(&loc)),
            (-1, _) => PacketOut::Hole,
            _ => PacketOut::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_table_matches_libogg() {
        // First entries of crctable.h.
        assert_eq!(
            &CRC_LOOKUP[0][..4],
            &[0x00000000, 0x04c11db7, 0x09823b6e, 0x0d4326d9]
        );
        assert_eq!(CRC_LOOKUP[0][255], 0xb1f740b4);
    }
}
