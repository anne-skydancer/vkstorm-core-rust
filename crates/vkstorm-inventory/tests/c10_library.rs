//! C10 the Linden Library cache: regenerates every line of
//! `fixtures/model/c10_library.txt.gz` from the crate, as `c10_library()` in
//! `llinventory_tool.cpp` writes it from the frozen C++. The input is the
//! committed cache `library/ba2a564a-....inv.llsd.gz` (a 4-byte big-endian
//! version, then binary LLSD `{categories, items}`): every category through
//! the viewer subclass's `importLLSDMap` and `exportLLSD`, every item through
//! `fromLLSD` and `asLLSD`, with the byte identity of each re-serialization.
//!
//! Both files are gzip. No workspace crate gunzips (vkstorm-assets inflates
//! zlib streams only), so this test carries a small RFC 1951 inflate and an
//! RFC 1952 reader that checks the CRC-32 and the length.

mod common;

use common::*;
use vkstorm_inventory::category::{Category, ViewerCategory};
use vkstorm_inventory::item::Item;
use vkstorm_inventory::llsd::coerce::get;
use vkstorm_inventory::object::Object;
use vkstorm_inventory::types::{AssetType, FolderType};
use vkstorm_llsd::{Format, Limits, Value};

// ---------------------------------------------------------------------------
// gunzip (RFC 1951 inflate, RFC 1952 member)
// ---------------------------------------------------------------------------

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
    nbits: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits {
            data,
            pos: 0,
            bit: 0,
            nbits: 0,
        }
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

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn inflate_codes(
    b: &mut Bits<'_>,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
) -> Result<(), String> {
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
                out.extend_from_slice(
                    data.get(p + 4..p + 4 + len)
                        .ok_or("stored block truncated")?,
                );
                b.pos = p + 4 + len;
            }
            1 => {
                let mut l = [0u8; 288];
                l[..144].fill(8);
                l[144..256].fill(9);
                l[256..280].fill(7);
                l[280..].fill(8);
                inflate_codes(
                    &mut b,
                    &mut out,
                    &Huffman::new(&l),
                    &Huffman::new(&[5u8; 30]),
                )?;
            }
            2 => {
                let nlen = b.bits(5)? as usize + 257;
                let ndist = b.bits(5)? as usize + 1;
                let ncode = b.bits(4)? as usize + 4;
                const ORDER: [usize; 19] = [
                    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                ];
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
                            let prev = *lengths[..i]
                                .last()
                                .ok_or("repeat with no previous length")?;
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
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
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
fn gunzip(mut data: &[u8]) -> Result<Vec<u8>, String> {
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
                p += data[p..]
                    .iter()
                    .position(|&c| c == 0)
                    .ok_or("unterminated header string")?
                    + 1;
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

fn read_gz(name: &str) -> Vec<u8> {
    let path = fixture_path(name);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    gunzip(&data).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn gunzip_reads_stored_and_fixed_blocks() {
    // zlib (wbits 31) at level 0 and 9; the library files use dynamic blocks.
    assert_eq!(
        gunzip(&unhex(
            "1f8b080000000000040a010300fcff616263c241243503000000"
        ))
        .unwrap(),
        b"abc"
    );
    assert_eq!(
        gunzip(&unhex(
            "1f8b080000000000020acb48cdc9c957c840905c003b7c8adf12000000"
        ))
        .unwrap(),
        b"hello hello hello
"
    );
    assert!(
        gunzip(&unhex(
            "1f8b080000000000020acb48cdc9c957c840905c003b7c8adf13000000"
        ))
        .is_err()
    );
}

// ---------------------------------------------------------------------------
// Columns (the tool's item_cols, cat_cols, vcat_cols)
// ---------------------------------------------------------------------------

fn item_cols(it: &Item) -> Vec<String> {
    let o = &it.object;
    let mut v = vec![
        uuid(&o.id),
        uuid(&o.parent),
        uuid(&it.asset),
        uuid(&o.thumbnail),
        d(o.asset_type.0),
        d(it.inv_type.0),
        x8(it.flags),
    ];
    v.extend(perm_cols(&it.permissions));
    v.extend([
        d(it.sale_info.sale_type.0),
        d(it.sale_info.price),
        d(o.creation_date),
        bl(o.favorite),
        esc(&o.runtime),
        esc(&o.name),
        esc(&it.description),
    ]);
    v
}

fn vcat_cols(c: &ViewerCategory) -> Vec<String> {
    let o = &c.category.object;
    vec![
        uuid(&o.id),
        uuid(&o.parent),
        d(o.asset_type.0),
        d(c.category.preferred_type.0),
        uuid(&o.thumbnail),
        bl(o.favorite),
        d(o.creation_date),
        esc(&o.runtime),
        esc(&o.name),
        uuid(&c.owner),
        d(c.version),
    ]
}

// ---------------------------------------------------------------------------
// C10 (the tool's load_cache and c10_library)
// ---------------------------------------------------------------------------

const LIBRARY_FILE: &str = "library/ba2a564a-f0f1-4b82-9c61-b7520bfcd09f.inv.llsd.gz";

/// `load_cache`: the version, then the binary LLSD.
fn load_cache() -> (i32, Value) {
    let data = read_gz(LIBRARY_FILE);
    assert!(data.len() >= 4, "{LIBRARY_FILE}: shorter than its version");
    let version = i32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let (n, v) = vkstorm_llsd::parse(Format::Binary, &data[4..], Limits::UNLIMITED);
    assert!(n > 0, "{LIBRARY_FILE}: binary LLSD does not parse ({n})");
    (version, v.expect("a parsed value"))
}

fn elements(v: &Value) -> &[Value] {
    match v {
        Value::Array(a) => a,
        _ => &[],
    }
}

/// `make_vcat<I>(CatF())`: id, parent and thumbnail null, `AT_CATEGORY`,
/// `FT_NONE`, not favorite, date 0, empty strings, null owner, version -1.
fn default_vcat() -> ViewerCategory {
    ViewerCategory {
        category: Category {
            object: Object {
                asset_type: AssetType(8),
                ..Object::default()
            },
            preferred_type: FolderType(-1),
        },
        owner: [0; 16],
        version: -1,
    }
}

fn c10_lines(inventory: &Value) -> Lines {
    let mut s = Lines::default();
    let (mut cats, mut cats_same, mut items, mut items_same) = (0i64, 0i64, 0i64, 0i64);
    for (k, cat) in elements(get(inventory, "categories")).iter().enumerate() {
        let mut v = default_vcat();
        v.version = -1; // setVersion(-1)
        v.import_llsd_map(cat);
        let mut again = Value::Undefined;
        v.export_llsd(&mut again);
        let same = llsd_bin(&again) == llsd_bin(cat);
        cats += 1;
        cats_same += i64::from(same);
        let mut cols = vcat_cols(&v);
        cols.insert(0, bl(same));
        s.emit(&format!("c10.cat.{k}"), "same+vcat", &cols);
    }
    for (k, sd) in elements(get(inventory, "items")).iter().enumerate() {
        let mut it = Item::default();
        let ok = it.from_llsd(sd, true, false);
        let same = llsd_bin(&it.to_llsd()) == llsd_bin(sd);
        items += 1;
        items_same += i64::from(same);
        let mut cols = item_cols(&it);
        cols.insert(0, bl(same));
        cols.insert(0, bl(ok));
        s.emit(&format!("c10.item.{k}"), "ret+same+item", &cols);
    }
    s.emit(
        "c10.summary",
        "counts",
        &[d(cats), d(cats_same), d(items), d(items_same)],
    );
    s
}

/// The data lines of the gzip fixture, as `read_fixture` reads a plain one.
fn read_gz_fixture(name: &str) -> Vec<String> {
    let text = String::from_utf8(read_gz(name)).expect("fixture lines are escaped ASCII");
    let mut body: Vec<String> = text
        .split('\n')
        .filter(|l| !l.starts_with('#'))
        .map(str::to_owned)
        .collect();
    if text.ends_with('\n') || text.is_empty() {
        body.pop();
    }
    body
}

#[test]
fn c10_library() {
    let (version, inventory) = load_cache();
    assert_eq!(version, 5, "{LIBRARY_FILE}: cache version");
    let expected = read_gz_fixture("c10_library.txt.gz");
    let actual = c10_lines(&inventory);
    assert_replay("c10_library.txt.gz", &expected, &actual.0);
}
