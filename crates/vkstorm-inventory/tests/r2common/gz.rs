//! llcommon's `gunzip_file` as the C++ glue runs it around the cache codec:
//! zlib's `gzread` over the whole file. A file that does not start with the
//! gzip magic is copied as it is (zlib's transparent read, so a 0-byte
//! `.gz` inflates to an empty file); gzip members are inflated one after
//! another, each checked against its CRC-32 and length; bytes after a member
//! that do not start another member are ignored (zlib's "trailing garbage");
//! a truncated or corrupt member fails (`Z_BUF_ERROR`, `Z_DATA_ERROR`).
//!
//! No workspace crate gunzips without an external dependency, so this is a
//! test-only RFC 1951 inflate (table-driven: a 10-bit first-level lookup and
//! a canonical walk for longer codes) and RFC 1952 reader.

/// `gunzip_file`'s outcome over the bytes of a `.gz` file: the plain bytes,
/// or `None` when it fails.
pub fn gunzip_file(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 2 || data[0] != 0x1f || data[1] != 0x8b {
        return Some(data.to_vec());
    }
    let mut out = Vec::new();
    let mut rest = data;
    loop {
        let used = member(rest, &mut out).ok()?;
        rest = &rest[used..];
        if !(rest.len() > 1 && rest[0] == 0x1f && rest[1] == 0x8b) {
            return Some(out);
        }
    }
}

/// Every member of a well-formed gzip file (fixtures the recorder wrote).
pub fn gunzip_strict(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let used = member(rest, &mut out).expect("gzip member");
        rest = &rest[used..];
    }
    out
}

/// One gzip member appended to `out`; the bytes it used.
fn member(data: &[u8], out: &mut Vec<u8>) -> Result<usize, String> {
    if data.len() < 10 || data[2] != 8 {
        return Err("gzip header".into());
    }
    let flags = data[3];
    let mut p = 10;
    if flags & 4 != 0 {
        let x = data.get(p..p + 2).ok_or("header truncated")?;
        p += 2 + usize::from(u16::from_le_bytes([x[0], x[1]]));
    }
    for bit in [8u8, 16] {
        if flags & bit != 0 {
            p += data.get(p..).ok_or("header truncated")?.iter().position(|&c| c == 0).ok_or("header truncated")? + 1;
        }
    }
    if flags & 2 != 0 {
        p += 2;
    }
    let body = data.get(p..).ok_or("header truncated")?;
    let start = out.len();
    let used = inflate(body, out)?;
    let t = p + used;
    let trailer = data.get(t..t + 8).ok_or("trailer truncated")?;
    let crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let isize = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
    if crc != crc32(&out[start..]) || isize != (out.len() - start) as u32 {
        return Err("incorrect data check".into());
    }
    Ok(t + 8)
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    n: u32,
    /// Bits supplied past the end (as zeros); using any is a truncation.
    over: u32,
}

impl<'a> Bits<'a> {
    fn fill(&mut self) {
        while self.n <= 56 {
            let b = if self.pos < self.data.len() {
                self.data[self.pos]
            } else {
                self.over += 8;
                0
            };
            self.pos += 1;
            self.buf |= u64::from(b) << self.n;
            self.n += 8;
        }
    }

    fn peek(&mut self, k: u32) -> u32 {
        if self.n < k {
            self.fill();
        }
        (self.buf & ((1u64 << k) - 1)) as u32
    }

    fn consume(&mut self, k: u32) -> Result<(), String> {
        self.buf >>= k;
        self.n -= k;
        // Bits past the end are only an error once consumed.
        let real = (self.data.len() as u64 * 8) as i64 - (self.pos as i64 * 8 - i64::from(self.n));
        if real < 0 { Err("unexpected end of file".into()) } else { Ok(()) }
    }

    fn bits(&mut self, k: u32) -> Result<u32, String> {
        if k == 0 {
            return Ok(0);
        }
        let v = self.peek(k);
        self.consume(k)?;
        Ok(v)
    }

    /// Drops to the byte boundary; the byte position of the next unread byte.
    fn align(&mut self) -> usize {
        let drop = self.n % 8;
        self.buf >>= drop;
        self.n -= drop;
        self.pos - (self.n / 8) as usize
    }

    fn reset_at(&mut self, pos: usize) {
        self.pos = pos;
        self.buf = 0;
        self.n = 0;
    }

    /// The byte position just after the consumed bits (the final block's
    /// padding dropped).
    fn byte_pos(&self) -> usize {
        self.pos - (self.n / 8) as usize
    }
}

const FAST: u32 = 10;

struct Huffman {
    /// For each 10-bit (bit-reversed) prefix: symbol << 4 | length, or 0 for
    /// a longer code.
    table: Vec<u16>,
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Huffman {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        let mut offs = [0u16; 16];
        for i in 1..16 {
            offs[i] = offs[i - 1] + counts[i - 1];
        }
        let mut symbols = vec![0u16; lengths.len()];
        let mut next = offs;
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[next[l as usize] as usize] = sym as u16;
                next[l as usize] += 1;
            }
        }
        // Canonical codes, then the reversed-prefix table.
        let mut table = vec![0u16; 1 << FAST];
        let mut code = 0u32;
        let mut idx = 0usize;
        for len in 1..16u32 {
            for _ in 0..counts[len as usize] {
                let sym = symbols[idx];
                idx += 1;
                if len <= FAST {
                    let mut rev = 0u32;
                    for b in 0..len {
                        rev |= ((code >> b) & 1) << (len - 1 - b);
                    }
                    let mut j = rev;
                    while j < (1 << FAST) {
                        table[j as usize] = (sym << 4) | len as u16;
                        j += 1 << len;
                    }
                }
                code += 1;
            }
            code <<= 1;
        }
        Huffman { table, counts, symbols }
    }

    fn decode(&self, b: &mut Bits<'_>) -> Result<u16, String> {
        let e = self.table[b.peek(FAST) as usize];
        if e != 0 {
            b.consume(u32::from(e & 15))?;
            return Ok(e >> 4);
        }
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.bits(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - count < first {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("invalid code".into())
    }
}

const LEN_BASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

fn codes(b: &mut Bits<'_>, out: &mut Vec<u8>, start: usize, lit: &Huffman, dist: &Huffman) -> Result<(), String> {
    loop {
        let sym = lit.decode(b)? as usize;
        if sym < 256 {
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(());
        } else {
            let i = sym - 257;
            if i >= 29 {
                return Err("invalid length".into());
            }
            let len = LEN_BASE[i] as usize + b.bits(u32::from(LEN_EXTRA[i]))? as usize;
            let d = dist.decode(b)? as usize;
            if d >= 30 {
                return Err("invalid distance".into());
            }
            let distance = DIST_BASE[d] as usize + b.bits(u32::from(DIST_EXTRA[d]))? as usize;
            if distance > out.len() - start {
                return Err("distance too far back".into());
            }
            let from = out.len() - distance;
            for k in 0..len {
                let c = out[from + k];
                out.push(c);
            }
        }
    }
}

/// A raw deflate stream appended to `out`; the bytes it used.
fn inflate(data: &[u8], out: &mut Vec<u8>) -> Result<usize, String> {
    let start = out.len();
    let mut b = Bits { data, pos: 0, buf: 0, n: 0, over: 0 };
    loop {
        let last = b.bits(1)?;
        match b.bits(2)? {
            0 => {
                let p = b.align();
                let hdr = data.get(p..p + 4).ok_or("unexpected end of file")?;
                let len = usize::from(u16::from_le_bytes([hdr[0], hdr[1]]));
                if len != usize::from(!u16::from_le_bytes([hdr[2], hdr[3]])) {
                    return Err("invalid stored block lengths".into());
                }
                out.extend_from_slice(data.get(p + 4..p + 4 + len).ok_or("unexpected end of file")?);
                b.reset_at(p + 4 + len);
            }
            1 => {
                let mut l = [0u8; 288];
                l[..144].fill(8);
                l[144..256].fill(9);
                l[256..280].fill(7);
                l[280..].fill(8);
                codes(&mut b, out, start, &Huffman::new(&l), &Huffman::new(&[5u8; 30]))?;
            }
            2 => {
                let nlen = b.bits(5)? as usize + 257;
                let ndist = b.bits(5)? as usize + 1;
                let ncode = b.bits(4)? as usize + 4;
                const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                let mut cl = [0u8; 19];
                for &o in ORDER.iter().take(ncode) {
                    cl[o] = b.bits(3)? as u8;
                }
                let clh = Huffman::new(&cl);
                let mut lengths = vec![0u8; nlen + ndist];
                let mut i = 0;
                while i < nlen + ndist {
                    let sym = clh.decode(&mut b)?;
                    let (val, rep) = match sym {
                        0..=15 => (sym as u8, 1),
                        16 => (*lengths[..i].last().ok_or("invalid bit length repeat")?, 3 + b.bits(2)? as usize),
                        17 => (0, 3 + b.bits(3)? as usize),
                        _ => (0, 11 + b.bits(7)? as usize),
                    };
                    if i + rep > lengths.len() {
                        return Err("invalid bit length repeat".into());
                    }
                    lengths[i..i + rep].fill(val);
                    i += rep;
                }
                codes(&mut b, out, start, &Huffman::new(&lengths[..nlen]), &Huffman::new(&lengths[nlen..]))?;
            }
            _ => return Err("invalid block type".into()),
        }
        if last == 1 {
            let _ = b.over;
            b.align();
            return Ok(b.byte_pos());
        }
    }
}

pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (n, e) in t.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    let mut c = !0u32;
    for &b in data {
        c = table[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}
