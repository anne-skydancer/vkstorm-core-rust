//! `LLDataPackerBinaryBuffer` and `LLDataPackerAsciiBuffer` (`lldatapacker.cpp`):
//! the binary and text serialisation of the viewer's asset and object data.
//!
//! Both work on a caller-owned buffer with a cursor, a size and the
//! `mWriteEnabled` flag (false for a packer that only measures); the viewer
//! keeps those in its `LLDataPacker` objects and hands them over per call.
//! The binary format is the host (little-endian) bytes of each value, strings
//! with their NUL, binary data behind a 4-byte size. The text format is one
//! line per value (see [`ascii`](crate::ascii) for the conversions), each
//! preceded by `name<tab>` when names are included.
//!
//! The C++ bounds rule is kept: a write is refused when the cursor would pass
//! the end, and only when writes are enabled; reads were unchecked in the
//! C++ when writes were disabled (the default-constructed packer), which this
//! refuses instead of reading past the buffer. `packS16`/`unpackS16` advance
//! the cursor even when they fail, as the C++ did. A text buffer write that
//! does not fit is cut as `snprintf` cut it (the cursor advances by the
//! room there was) and reported.

use crate::ascii;

/// `DP_BUFSIZE`: the text packers' line buffer.
pub const DP_BUFSIZE: usize = 512;

/// A binary packer's view of its buffer for one call.
pub struct BinaryBuffer<'a> {
    pub data: &'a mut [u8],
    /// `mCurBufferp - mBufferp`.
    pub cur: i32,
    /// `mBufferSize`.
    pub size: i32,
    pub write_enabled: bool,
}

impl BinaryBuffer<'_> {
    /// `verifyLength`.
    fn verify(&self, n: i32) -> bool {
        !(self.write_enabled && self.cur > self.size - n)
    }

    fn readable(&self, n: usize) -> bool {
        self.cur >= 0
            && (self.cur as usize)
                .checked_add(n)
                .is_some_and(|e| e <= self.data.len())
    }

    /// A fixed-size value's bytes (every `pack*` but strings and sized data).
    pub fn pack_bytes(&mut self, bytes: &[u8]) -> bool {
        let n = bytes.len() as i32;
        if !self.verify(n) {
            return false;
        }
        if self.write_enabled {
            if !self.readable(bytes.len()) {
                return false;
            }
            let c = self.cur as usize;
            self.data[c..c + bytes.len()].copy_from_slice(bytes);
        }
        self.cur += n;
        true
    }

    /// A fixed-size value's bytes.
    pub fn unpack_bytes(&mut self, n: usize) -> Option<&[u8]> {
        if !self.verify(n as i32) || !self.readable(n) {
            return None;
        }
        let c = self.cur as usize;
        self.cur += n as i32;
        Some(&self.data[c..c + n])
    }

    /// `packS16`: advances even when refused.
    pub fn pack_s16(&mut self, v: i16) -> bool {
        let ok = self.verify(2);
        if self.write_enabled && ok && self.readable(2) {
            let c = self.cur as usize;
            self.data[c..c + 2].copy_from_slice(&v.to_le_bytes());
        }
        self.cur += 2;
        ok
    }

    /// `unpackS16`: advances even when refused.
    pub fn unpack_s16(&mut self) -> Option<i16> {
        let ok = self.verify(2) && self.readable(2);
        let r = if ok {
            let c = self.cur as usize;
            Some(i16::from_le_bytes([self.data[c], self.data[c + 1]]))
        } else {
            None
        };
        self.cur += 2;
        r
    }

    /// `packString`: the bytes and a NUL.
    pub fn pack_string(&mut self, s: &[u8]) -> bool {
        let s = s.split(|&b| b == 0).next().unwrap_or(&[]);
        let length = s.len() as i32 + 1;
        if !self.verify(length) {
            return false;
        }
        if self.write_enabled {
            if !self.readable(s.len() + 1) {
                return false;
            }
            let c = self.cur as usize;
            self.data[c..c + s.len()].copy_from_slice(s);
            self.data[c + s.len()] = 0;
        }
        self.cur += length;
        true
    }

    /// `unpackString`: up to the NUL, which must be inside the buffer.
    pub fn unpack_string(&mut self) -> Option<&[u8]> {
        if self.cur < 0 || self.cur as usize > self.data.len() {
            return None;
        }
        let c = self.cur as usize;
        let len = self.data[c..].iter().position(|&b| b == 0)?;
        if !self.verify(len as i32 + 1) {
            return None;
        }
        self.cur += len as i32 + 1;
        Some(&self.data[c..c + len])
    }

    /// `packBinaryData`: a 4-byte size then the bytes.
    pub fn pack_binary(&mut self, bytes: &[u8]) -> bool {
        let size = bytes.len() as i32;
        if !self.verify(size + 4) {
            return false;
        }
        if self.write_enabled {
            if !self.readable(bytes.len() + 4) {
                return false;
            }
            let c = self.cur as usize;
            self.data[c..c + 4].copy_from_slice(&size.to_le_bytes());
            self.data[c + 4..c + 4 + bytes.len()].copy_from_slice(bytes);
        }
        self.cur += 4 + size;
        true
    }

    /// `unpackBinaryData`: the bytes behind their size. A negative size, or
    /// one past the end, is refused; the cursor then stays before the size
    /// (the C++ advanced past a size that failed its own bounds check, and
    /// then refused; nothing reads on after a refusal).
    pub fn unpack_binary(&mut self) -> Option<&[u8]> {
        if !self.verify(4) || !self.readable(4) {
            return None;
        }
        let c = self.cur as usize;
        let size = i32::from_le_bytes([
            self.data[c],
            self.data[c + 1],
            self.data[c + 2],
            self.data[c + 3],
        ]);
        if size < 0 {
            return None;
        }
        let start = self.cur + 4;
        let n = size as usize;
        if (self.write_enabled && start > self.size - size)
            || (start as usize)
                .checked_add(n)
                .is_none_or(|e| e > self.data.len())
        {
            self.cur = start;
            return None;
        }
        self.cur = start + size;
        Some(&self.data[start as usize..start as usize + n])
    }
}

/// `packFixed`'s width: `int_bits + frac_bits`, plus one when signed.
pub fn fixed_total_bits(is_signed: bool, int_bits: u32, frac_bits: u32) -> i32 {
    (int_bits + frac_bits) as i32 + is_signed as i32
}

/// `LLDataPacker::packFixed`'s conversion of a float to its fixed-point
/// integer: clamped to `[-2^int_bits, 2^int_bits]` (`[0, 2^int_bits]`
/// unsigned), offset by `2^int_bits` when signed, scaled by `2^frac_bits`
/// and truncated. Returns the width (which chooses U8, U16 or U32; over 31
/// the C++ aborted) and the integer, which the C++ cast to the width's
/// type (`2^(int+frac)` wraps to 0 in 8 and 16 bits as it did there).
pub fn pack_fixed(value: f32, is_signed: bool, int_bits: u32, frac_bits: u32) -> (i32, u32) {
    let total_bits = fixed_total_bits(is_signed, int_bits, frac_bits);
    let max_val: u32 = 1u32 << int_bits;
    let min_val: i32 = if is_signed { -(max_val as i32) } else { 0 };
    let mut fixed_val = value.clamp(min_val as f32, max_val as f32);
    if is_signed {
        fixed_val += max_val as f32;
    }
    fixed_val *= (1u32 << frac_bits) as f32;
    // a float to unsigned conversion in C++: truncation toward zero, then
    // the width's wrap
    let as_int = fixed_val as i64;
    let fixed = if total_bits <= 8 {
        (as_int as u8) as u32
    } else if total_bits <= 16 {
        (as_int as u16) as u32
    } else {
        as_int as u32
    };
    (total_bits, fixed)
}

/// `LLDataPacker::unpackFixed`'s conversion back.
pub fn unpack_fixed(fixed: u32, is_signed: bool, int_bits: u32, frac_bits: u32) -> f32 {
    let max_val: u32 = 1u32 << int_bits;
    let mut fixed_val = fixed as f32;
    fixed_val /= (1u32 << frac_bits) as f32;
    if is_signed {
        fixed_val -= max_val as f32;
    }
    fixed_val
}

/// A text packer's view of its buffer for one call.
pub struct AsciiBuffer<'a> {
    pub data: &'a mut [u8],
    pub cur: i32,
    pub size: i32,
    pub write_enabled: bool,
    pub include_names: bool,
}

/// The value kinds the text format distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKind {
    /// `packString`.
    String,
    /// `packBinaryData`.
    Binary,
    /// `packBinaryDataFixed`.
    BinaryFixed,
    /// `packU8`, `packU16`, `packS16`, `packS32` (`%d`).
    Int,
    /// `packU32` (`%u`).
    Uint,
    /// `packF32`, `packVector2/3/4`, `packColor4` (`%f` each).
    Floats,
    /// `packColor4U` (`%d` each).
    Bytes,
    /// `packUUID`.
    Uuid,
    /// The stream form of `LLDataPackerAsciiFile`'s floats: each `%f`
    /// through a 20-byte buffer, so cut to 19 characters.
    FloatsShort,
}

/// The line text (with its newline, without the name) the text packers
/// write for a value. `floats` carries the components of `Floats`, `bytes`
/// the data of `Binary`, `BinaryFixed`, `Bytes`, `String` and `Uuid`.
pub fn format_value(kind: TextKind, ints: &[i64], floats: &[f32], bytes: &[u8], out: &mut Vec<u8>) {
    match kind {
        TextKind::String | TextKind::Uuid => {
            let s = bytes.split(|&b| b == 0).next().unwrap_or(&[]);
            out.extend_from_slice(s);
            out.push(b'\n');
        }
        TextKind::Binary => {
            ascii::fmt_i32_zero10(out, bytes.len() as i32);
            out.push(b' ');
            for &b in bytes {
                ascii::fmt_hex2(out, b);
                out.push(b' ');
            }
            out.push(b'\n');
        }
        TextKind::BinaryFixed => {
            for &b in bytes {
                ascii::fmt_hex2(out, b);
                out.push(b' ');
            }
            out.push(b'\n');
        }
        TextKind::Int => {
            ascii::fmt_i32(out, ints.first().copied().unwrap_or(0) as i32);
            out.push(b'\n');
        }
        TextKind::Uint => {
            ascii::fmt_u32(out, ints.first().copied().unwrap_or(0) as u32);
            out.push(b'\n');
        }
        TextKind::Floats | TextKind::FloatsShort => {
            for (i, &f) in floats.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                let start = out.len();
                ascii::fmt_f32(out, f);
                if kind == TextKind::FloatsShort {
                    out.truncate(start + 19.min(out.len() - start));
                }
            }
            out.push(b'\n');
        }
        TextKind::Bytes => {
            for (i, &b) in bytes.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                ascii::fmt_i32(out, b as i32);
            }
            out.push(b'\n');
        }
    }
}

/// What a text read produced: the scalar slots the kind uses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub ints: Vec<i64>,
    pub floats: Vec<f32>,
    pub bytes: Vec<u8>,
    /// `Binary`: the size the text declared (`bytes` holds what was there,
    /// never more than fit the 512-byte line).
    pub size: i32,
    /// How many of the expected scalars were assigned; the rest keep the
    /// caller's previous values.
    pub assigned: usize,
}

/// Reads a value line (without its name) as the text packers' `unpack*`
/// did. `count` is the number of scalars expected (`Floats`, `Bytes`) or
/// the fixed byte count (`BinaryFixed`).
pub fn parse_value(kind: TextKind, text: &[u8], count: usize) -> Parsed {
    let mut p = Parsed::default();
    match kind {
        TextKind::String => {
            p.bytes = text.to_vec();
            p.assigned = 1;
        }
        TextKind::Uuid => {
            if let Some((tok, _)) = ascii::scan_token(text, 0, 63) {
                p.bytes = tok.to_vec();
                p.assigned = 1;
            }
        }
        TextKind::Int => {
            if let Some((v, _)) = ascii::scan_i32(text, 0) {
                p.ints.push(v as i64);
                p.assigned = 1;
            }
        }
        TextKind::Uint => {
            if let Some((v, _)) = ascii::scan_u32(text, 0) {
                p.ints.push(v as i64);
                p.assigned = 1;
            }
        }
        TextKind::Floats | TextKind::FloatsShort => {
            let mut i = 0;
            for _ in 0..count {
                match ascii::scan_f32(text, i) {
                    Some((v, e)) => {
                        p.floats.push(v);
                        i = e;
                        p.assigned += 1;
                    }
                    None => break,
                }
            }
        }
        TextKind::Bytes => {
            let mut i = 0;
            for _ in 0..count {
                match ascii::scan_i32(text, i) {
                    Some((v, e)) => {
                        p.ints.push(v as i64);
                        i = e;
                        p.assigned += 1;
                    }
                    None => break,
                }
            }
        }
        TextKind::Binary => {
            // "%010d": a width of 10 bytes for the size
            let head = &text[..text.len().min(10)];
            if let Some((size, _)) = ascii::scan_i32(head, 0) {
                p.size = size;
                p.assigned = 1;
                let mut i = 11;
                for _ in 0..size.max(0) {
                    if i >= text.len() {
                        break;
                    }
                    match ascii::scan_hex2(text, i) {
                        Some((v, _)) => p.bytes.push(v),
                        None => break,
                    }
                    i += 3;
                }
            }
        }
        TextKind::BinaryFixed => {
            let mut i = 0;
            for _ in 0..count {
                if i >= text.len() {
                    break;
                }
                match ascii::scan_hex2(text, i) {
                    Some((v, _)) => p.bytes.push(v),
                    None => break,
                }
                i += 3;
            }
            p.assigned = p.bytes.len();
        }
    }
    p
}

impl AsciiBuffer<'_> {
    /// `getBufferSize() - getCurrentSize()`: the room `snprintf` was given.
    fn cap(&self) -> i32 {
        self.size - self.cur - 1
    }

    /// One `snprintf` into the buffer at the cursor: writes what fits (with
    /// the terminating NUL), advances by the C++ `numCopied` after its
    /// clamp, and reports truncation.
    fn piece(&mut self, text: &[u8]) -> bool {
        let cap = self.cap().max(0) as usize;
        let len = text.len();
        if self.write_enabled && self.cur >= 0 && (self.cur as usize) < self.data.len() {
            let c = self.cur as usize;
            let room = cap.min(self.data.len() - c);
            if room > 0 {
                let n = len.min(room - 1);
                self.data[c..c + n].copy_from_slice(&text[..n]);
                self.data[c + n] = 0;
            }
        }
        let truncated = len > cap;
        self.cur += len.min(cap) as i32;
        truncated
    }

    /// Advances by a measured length, clamped to the room, without writing.
    fn count(&mut self, len: usize) {
        let cap = self.cap().max(0) as usize;
        self.cur += len.min(cap) as i32;
    }

    /// `writeIndentedName`.
    fn write_name(&mut self, name: &[u8]) -> bool {
        if !self.include_names {
            return false;
        }
        if self.write_enabled {
            let mut t = name.to_vec();
            t.push(b'\t');
            self.piece(&t)
        } else {
            self.count(name.len() + 1);
            false
        }
    }

    /// A `pack*` of `kind`: writes the name (when included) and the line.
    /// Returns the C++ result (false only for a UUID that did not fit) and
    /// whether anything was cut.
    pub fn pack(
        &mut self,
        kind: TextKind,
        name: &[u8],
        ints: &[i64],
        floats: &[f32],
        bytes: &[u8],
    ) -> (bool, bool) {
        let mut truncated = self.write_name(name);
        if self.write_enabled {
            match kind {
                TextKind::Binary => {
                    let mut t = Vec::new();
                    ascii::fmt_i32_zero10(&mut t, bytes.len() as i32);
                    t.push(b' ');
                    truncated |= self.piece(&t);
                    let mut full = false;
                    for &b in bytes {
                        t.clear();
                        ascii::fmt_hex2(&mut t, b);
                        t.push(b' ');
                        if self.piece(&t) {
                            truncated = true;
                            full = true;
                            break;
                        }
                    }
                    if !full {
                        truncated |= self.piece(b"\n");
                    }
                }
                TextKind::BinaryFixed => {
                    let mut t = Vec::new();
                    let mut full = false;
                    for &b in bytes {
                        t.clear();
                        ascii::fmt_hex2(&mut t, b);
                        t.push(b' ');
                        if self.piece(&t) {
                            truncated = true;
                            full = true;
                            break;
                        }
                    }
                    if !full {
                        truncated |= self.piece(b"\n");
                    }
                }
                _ => {
                    let mut t = Vec::new();
                    format_value(kind, ints, floats, bytes, &mut t);
                    let cut = self.piece(&t);
                    truncated |= cut;
                    if kind == TextKind::Uuid && cut {
                        return (false, true);
                    }
                }
            }
        } else {
            // the measuring estimates of the C++
            let n = match kind {
                TextKind::String => bytes.split(|&b| b == 0).next().unwrap_or(&[]).len() + 1,
                TextKind::Binary => 10 + 1 + bytes.len(),
                TextKind::BinaryFixed => 2 * bytes.len() + 1,
                TextKind::Uuid => 64 + 1,
                _ => {
                    let mut t = Vec::new();
                    format_value(kind, ints, floats, bytes, &mut t);
                    t.len()
                }
            };
            let cap = self.cap();
            if kind == TextKind::Uuid && n as i32 > cap {
                self.count(n);
                return (false, true);
            }
            self.count(n);
        }
        (true, truncated)
    }

    /// `getValueStr`: the next value line (at most 511 bytes), after the
    /// name when names are included (`None` on a name mismatch). The cursor
    /// moves past the line's newline.
    pub fn next_value(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let c = self.cur.max(0) as usize;
        let rest: &[u8] = if c < self.data.len() {
            &self.data[c..]
        } else {
            &[]
        };
        // the C string ends at the first NUL
        let rest = rest.split(|&b| b == 0).next().unwrap_or(&[]);
        if self.include_names {
            let line = ascii::scan_line(rest, 0, 511).map_or(&rest[..0], |(l, _)| l);
            self.cur += line.len() as i32 + 1;
            let (keyword, value) = ascii::split_keyword_value(line);
            if keyword != name {
                return None;
            }
            Some(value.to_vec())
        } else {
            let value = ascii::scan_line(rest, 0, 511).map_or(&rest[..0], |(l, _)| l);
            self.cur += value.len() as i32 + 1;
            Some(value.to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point() {
        assert_eq!(pack_fixed(44.44, false, 8, 8), (16, 11376));
        assert_eq!(unpack_fixed(11376, false, 8, 8), 44.4375);
        assert_eq!(pack_fixed(1e9, false, 8, 8), (16, 0));
        assert_eq!(
            pack_fixed(-100.25, true, 7, 8),
            (16, (27.75f32 * 256.0) as u32)
        );
        assert_eq!(pack_fixed(12.75, false, 15, 16), (31, 12 * 65536 + 49152));
    }

    #[test]
    fn binary_roundtrip() {
        let mut data = vec![0u8; 64];
        let mut b = BinaryBuffer {
            data: &mut data,
            cur: 0,
            size: 64,
            write_enabled: true,
        };
        assert!(b.pack_string(b"hi"));
        assert!(b.pack_bytes(&7u32.to_le_bytes()));
        assert!(b.pack_binary(b"xyz"));
        assert!(b.pack_s16(-2));
        let n = b.cur;
        assert_eq!(n, 3 + 4 + 7 + 2);
        let mut r = BinaryBuffer {
            data: &mut data,
            cur: 0,
            size: n,
            write_enabled: true,
        };
        assert_eq!(r.unpack_string().unwrap(), b"hi");
        assert_eq!(r.unpack_bytes(4).unwrap(), &7u32.to_le_bytes());
        assert_eq!(r.unpack_binary().unwrap(), b"xyz");
        assert_eq!(r.unpack_s16(), Some(-2));
        assert_eq!(r.unpack_bytes(1), None);
        assert_eq!(r.unpack_s16(), None);
        assert_eq!(r.cur, n + 2);
        // measuring
        let mut m = BinaryBuffer {
            data: &mut [],
            cur: 0,
            size: 0,
            write_enabled: false,
        };
        assert!(m.pack_binary(b"abcd"));
        assert_eq!(m.cur, 8);
    }

    #[test]
    fn ascii_roundtrip() {
        let mut data = vec![0u8; 256];
        let mut a = AsciiBuffer {
            data: &mut data,
            cur: 0,
            size: 256,
            write_enabled: true,
            include_names: true,
        };
        assert_eq!(a.pack(TextKind::Int, b"n", &[-5], &[], &[]), (true, false));
        assert_eq!(
            a.pack(TextKind::Floats, b"v", &[], &[1.5, 2.0], &[]),
            (true, false)
        );
        assert_eq!(
            a.pack(TextKind::Binary, b"b", &[], &[], &[1, 0xab]),
            (true, false)
        );
        assert_eq!(
            a.pack(TextKind::String, b"s", &[], &[], b"hello world"),
            (true, false)
        );
        let n = a.cur as usize;
        assert_eq!(
            &data[..n],
            &b"n\t-5\nv\t1.500000 2.000000\nb\t0000000002 01 ab \ns\thello world\n"[..]
        );
        let mut r = AsciiBuffer {
            data: &mut data,
            cur: 0,
            size: n as i32 + 1,
            write_enabled: true,
            include_names: true,
        };
        let v = r.next_value(b"n").unwrap();
        assert_eq!(parse_value(TextKind::Int, &v, 1).ints, vec![-5]);
        let v = r.next_value(b"v").unwrap();
        assert_eq!(parse_value(TextKind::Floats, &v, 2).floats, vec![1.5, 2.0]);
        let v = r.next_value(b"b").unwrap();
        let p = parse_value(TextKind::Binary, &v, 0);
        assert_eq!((p.size, p.bytes), (2, vec![1, 0xab]));
        let v = r.next_value(b"s").unwrap();
        assert_eq!(parse_value(TextKind::String, &v, 0).bytes, b"hello world");
        assert_eq!(r.next_value(b"x"), None);
    }

    #[test]
    fn ascii_truncation() {
        let mut data = vec![0u8; 8];
        let mut a = AsciiBuffer {
            data: &mut data,
            cur: 0,
            size: 8,
            write_enabled: true,
            include_names: false,
        };
        assert_eq!(
            a.pack(TextKind::String, b"", &[], &[], b"0123456789"),
            (true, true)
        );
        assert_eq!(a.cur, 7);
        assert_eq!(&data[..8], b"012345\0\0");
        let mut m = AsciiBuffer {
            data: &mut [],
            cur: 0,
            size: 100,
            write_enabled: false,
            include_names: true,
        };
        m.pack(TextKind::Uuid, b"id", &[], &[], b"");
        assert_eq!(m.cur, 3 + 65);
    }
}
