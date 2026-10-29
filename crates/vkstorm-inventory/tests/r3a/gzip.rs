//! gunzip for the large case's reply (`s10a_large_recursive.0.xml.gz`): the
//! RFC 1951 inflate and RFC 1952 reader of `c10_library.rs`, copied (that
//! test keeps its own; no workspace crate gunzips without an external
//! dependency).

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
    nbits: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0, bit: 0, nbits: 0 }
    }

    fn need(&mut self, n: u32) -> Result<(), String> {
        while self.nbits < n {
            let b = *self.data.get(self.pos).ok_or("deflate stream truncated")?;
            self.pos += 1;
            self.bit |= u32::from(b) << self.nbits;
            self.nbits += 8;
        }
        Ok(())
    }

    fn bits(&mut self, n: u32) -> Result<u32, String> {
        if n == 0 {
            return Ok(0);
        }
        self.need(n)?;
        let v = self.bit & ((1u32 << n) - 1);
        self.bit >>= n;
        self.nbits -= n;
        Ok(v)
    }

    /// Drops the bits left in the current byte (stored blocks).
    fn align(&mut self) {
        self.bit = 0;
        self.nbits = 0;
    }
}

/// A canonical Huffman code: the count of codes per length and the symbols
/// in code order (zlib's `puff` decoding).
struct Huffman {
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
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        Huffman { counts, symbols }
    }

    fn decode(&self, b: &mut Bits<'_>) -> Result<u16, String> {
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
        Err("invalid Huffman code".into())
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

fn inflate_codes(b: &mut Bits<'_>, out: &mut Vec<u8>, lit: &Huffman, dist: &Huffman) -> Result<(), String> {
    loop {
        let sym = lit.decode(b)? as usize;
        if sym < 256 {
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(());
        } else {
            let i = sym - 257;
            if i >= 29 {
                return Err("invalid length symbol".into());
            }
            let len = LEN_BASE[i] as usize + b.bits(u32::from(LEN_EXTRA[i]))? as usize;
            let d = dist.decode(b)? as usize;
            if d >= 30 {
                return Err("invalid distance symbol".into());
            }
            let distance = DIST_BASE[d] as usize + b.bits(u32::from(DIST_EXTRA[d]))? as usize;
            if distance > out.len() {
                return Err("distance before the start".into());
            }
            let start = out.len() - distance;
            for k in 0..len {
                let c = out[start + k];
                out.push(c);
            }
        }
    }
}

/// RFC 1951: a raw deflate stream. Returns the output and the bytes read.
fn inflate(data: &[u8]) -> Result<(Vec<u8>, usize), String> {
    let mut b = Bits::new(data);
    let mut out = Vec::new();
    loop {
        let last = b.bits(1)?;
        match b.bits(2)? {
            0 => {
                b.align();
                let p = b.pos;
                let hdr = data.get(p..p + 4).ok_or("stored block truncated")?;
                let len = usize::from(u16::from_le_bytes([hdr[0], hdr[1]]));
                if len != usize::from(!u16::from_le_bytes([hdr[2], hdr[3]])) {
                    return Err("stored block length check".into());
                }
                out.extend_from_slice(data.get(p + 4..p + 4 + len).ok_or("stored block truncated")?);
                b.pos = p + 4 + len;
            }
            1 => {
                let mut l = [0u8; 288];
                l[..144].fill(8);
                l[144..256].fill(9);
                l[256..280].fill(7);
                l[280..].fill(8);
                inflate_codes(&mut b, &mut out, &Huffman::new(&l), &Huffman::new(&[5u8; 30]))?;
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
                        16 => {
                            let prev = *lengths[..i].last().ok_or("repeat with no previous length")?;
                            (prev, 3 + b.bits(2)? as usize)
                        }
                        17 => (0, 3 + b.bits(3)? as usize),
                        _ => (0, 11 + b.bits(7)? as usize),
                    };
                    if i + rep > lengths.len() {
                        return Err("too many code lengths".into());
                    }
                    lengths[i..i + rep].fill(val);
                    i += rep;
                }
                let lit = Huffman::new(&lengths[..nlen]);
                let dist = Huffman::new(&lengths[nlen..]);
                inflate_codes(&mut b, &mut out, &lit, &dist)?;
            }
            _ => return Err("invalid block type".into()),
        }
        if last == 1 {
            return Ok((out, b.pos));
        }
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, t) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    let mut c = !0u32;
    for &b in data {
        c = table[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// RFC 1952: every member of a gzip file, concatenated (as `gzread`), each
/// checked against its CRC-32 and length.
pub fn gunzip(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    while !data.is_empty() {
        if data.len() < 18 || data[0] != 0x1f || data[1] != 0x8b || data[2] != 8 {
            return Err("not a gzip member".into());
        }
        let flags = data[3];
        let mut p = 10;
        if flags & 4 != 0 {
            let xlen = usize::from(u16::from_le_bytes([data[p], data[p + 1]]));
            p += 2 + xlen;
        }
        for bit in [8u8, 16] {
            if flags & bit != 0 {
                p += data[p..].iter().position(|&c| c == 0).ok_or("unterminated header string")? + 1;
            }
        }
        if flags & 2 != 0 {
            p += 2;
        }
        let (body, used) = inflate(&data[p..])?;
        let t = p + used;
        let trailer = data.get(t..t + 8).ok_or("gzip trailer truncated")?;
        let crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
        let isize = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
        if crc != crc32(&body) || isize != body.len() as u32 {
            return Err("gzip CRC or length mismatch".into());
        }
        out.extend_from_slice(&body);
        data = &data[t + 8..];
    }
    Ok(out)
}
