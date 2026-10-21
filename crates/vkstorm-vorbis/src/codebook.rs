//! Codebooks: header unpacking (`codebook.c` `vorbis_staticbook_unpack`),
//! decode setup (`sharedbook.c`: `_make_words`, `_book_unquantize`,
//! `vorbis_book_init_decode`) and entry/vector decoding (`codebook.c`),
//! ported from libvorbis 1.3.7.
//!
//! The value lists are computed with the C's mixed precision: the
//! dequantisation `fabs(val)*delta+mindel+last` runs in double (C's `fabs`
//! returns double) and is rounded to float once.
//!
//! Original code: Copyright (c) 1994-2015 Xiph.Org Foundation, BSD-3-Clause
//! (see `LICENSE-xiph`). Rust port: Vulkanstorm.

use crate::bitpack::BitReader;

/// `ov_ilog`.
#[inline]
pub(crate) fn ov_ilog(v: u32) -> i32 {
    (32 - v.leading_zeros()) as i32
}

const VQ_FMAN: i64 = 21;
const VQ_FEXP_BIAS: i64 = 768;

/// `_float32_unpack`: the codebook's packed float. Exact (`ldexp` of a
/// 21-bit mantissa with the exponent clamped to +-63).
pub(crate) fn float32_unpack(val: i64) -> f32 {
    let mut mant = (val & 0x1f_ffff) as f64;
    let sign = val & 0x8000_0000 != 0;
    let mut exp = (val & 0x7fe0_0000) >> VQ_FMAN;
    if sign {
        mant = -mant;
    }
    exp = exp - (VQ_FMAN - 1) - VQ_FEXP_BIAS;
    exp = exp.clamp(-63, 63);
    // ldexp: an exact power-of-two scaling within the double range.
    (mant * f64::powi(2.0, exp as i32)) as f32
}

/// `static_codebook`: a codebook as transmitted in the setup header.
#[derive(Clone, Debug, Default)]
pub(crate) struct StaticCodebook {
    pub dim: i64,
    pub entries: i64,
    pub lengthlist: Vec<i8>,
    pub maptype: i32,
    pub q_min: i64,
    pub q_delta: i64,
    pub q_quant: i32,
    pub q_sequencep: i32,
    pub quantlist: Vec<i64>,
}

/// `_book_maptype1_quantvals`: the largest `vals` with
/// `vals^dim <= entries`. The `pow` guess is corrected by integer search, so
/// libm differences do not change the result.
pub(crate) fn book_maptype1_quantvals(b: &StaticCodebook) -> i64 {
    if b.entries < 1 {
        return 0;
    }
    let guess = ((b.entries as f32) as f64)
        .powf((1.0f32 / b.dim as f32) as f64)
        .floor();
    let mut vals: i64 = if guess.is_finite() { guess as i64 } else { 1 };
    if vals < 1 {
        vals = 1;
    }
    loop {
        let mut acc: i64 = 1;
        let mut acc1: i64 = 1;
        let mut i = 0i64;
        while i < b.dim {
            if b.entries / vals < acc {
                break;
            }
            acc *= vals;
            if i64::MAX / (vals + 1) < acc1 {
                acc1 = i64::MAX;
            } else {
                acc1 *= vals + 1;
            }
            i += 1;
        }
        if i >= b.dim && acc <= b.entries && acc1 > b.entries {
            return vals;
        } else if i < b.dim || acc > b.entries {
            vals -= 1;
        } else {
            vals += 1;
        }
    }
}

/// `vorbis_staticbook_unpack`; `None` on any error (`_eofout`/`_errout`).
pub(crate) fn staticbook_unpack(opb: &mut BitReader) -> Option<StaticCodebook> {
    let mut s = StaticCodebook::default();

    if opb.read(24) != 0x564342 {
        return None;
    }
    s.dim = opb.read(16);
    s.entries = opb.read(24);
    if s.entries == -1 {
        return None;
    }
    if ov_ilog(s.dim as u32) + ov_ilog(s.entries as u32) > 24 {
        return None;
    }

    match opb.read(1) as i32 {
        0 => {
            let unused = opb.read(1);
            let per = if unused != 0 { 1 } else { 5 };
            if (s.entries * per + 7) >> 3 > opb.storage() - opb.bytes() {
                return None;
            }
            s.lengthlist = vec![0i8; s.entries as usize];
            if unused != 0 {
                for i in 0..s.entries as usize {
                    if opb.read(1) != 0 {
                        let num = opb.read(5);
                        if num == -1 {
                            return None;
                        }
                        s.lengthlist[i] = (num + 1) as i8;
                    } else {
                        s.lengthlist[i] = 0;
                    }
                }
            } else {
                for i in 0..s.entries as usize {
                    let num = opb.read(5);
                    if num == -1 {
                        return None;
                    }
                    s.lengthlist[i] = (num + 1) as i8;
                }
            }
        }
        1 => {
            let mut length = opb.read(5) + 1;
            if length == 0 {
                return None;
            }
            s.lengthlist = vec![0i8; s.entries as usize];
            let mut i: i64 = 0;
            while i < s.entries {
                let num = opb.read(ov_ilog((s.entries - i) as u32));
                if num == -1 {
                    return None;
                }
                if length > 32
                    || num > s.entries - i
                    || (num > 0 && ((num - 1) >> (length - 1)) > 1)
                {
                    return None;
                }
                for _ in 0..num {
                    s.lengthlist[i as usize] = length as i8;
                    i += 1;
                }
                length += 1;
            }
        }
        _ => return None,
    }

    s.maptype = opb.read(4) as i32;
    match s.maptype {
        0 => {}
        1 | 2 => {
            s.q_min = opb.read(32);
            s.q_delta = opb.read(32);
            s.q_quant = (opb.read(4) + 1) as i32;
            s.q_sequencep = opb.read(1) as i32;
            if s.q_sequencep == -1 {
                return None;
            }
            let quantvals: i32 = match s.maptype {
                1 => {
                    if s.dim == 0 {
                        0
                    } else {
                        book_maptype1_quantvals(&s) as i32
                    }
                }
                _ => (s.entries * s.dim) as i32,
            };
            if ((quantvals as i64 * s.q_quant as i64 + 7) >> 3) > opb.storage() - opb.bytes() {
                return None;
            }
            s.quantlist = Vec::with_capacity(quantvals.max(0) as usize);
            for _ in 0..quantvals {
                s.quantlist.push(opb.read(s.q_quant));
            }
            if quantvals != 0 && s.quantlist[quantvals as usize - 1] == -1 {
                return None;
            }
        }
        _ => return None,
    }
    Some(s)
}

/// `_make_words`: canonical codewords from the length list (`None` for an
/// over- or under-populated tree).
fn make_words(l: &[i8], n: i64, sparsecount: i64) -> Option<Vec<u32>> {
    let mut count: usize = 0;
    let mut marker = [0u32; 33];
    let mut r = vec![0u32; if sparsecount != 0 { sparsecount } else { n } as usize];

    for i in 0..n as usize {
        let length = l[i] as i64;
        if length > 0 {
            let mut entry = marker[length as usize];
            if length < 32 && (entry >> length) != 0 {
                return None;
            }
            r[count] = entry;
            count += 1;

            let mut j = length as usize;
            while j > 0 {
                if marker[j] & 1 != 0 {
                    if j == 1 {
                        marker[1] = marker[1].wrapping_add(1);
                    } else {
                        marker[j] = marker[j - 1] << 1;
                    }
                    break;
                }
                marker[j] = marker[j].wrapping_add(1);
                j -= 1;
            }

            for j in (length as usize + 1)..33 {
                if (marker[j] >> 1) == entry {
                    entry = marker[j];
                    marker[j] = marker[j - 1] << 1;
                } else {
                    break;
                }
            }
        } else if sparsecount == 0 {
            count += 1;
        }
    }

    if !(count == 1 && marker[2] == 2) {
        for i in 1..33 {
            if marker[i] & (0xffff_ffffu32 >> (32 - i)) != 0 {
                return None;
            }
        }
    }

    count = 0;
    for i in 0..n as usize {
        let mut temp: u32 = 0;
        for j in 0..l[i].max(0) as u32 {
            temp <<= 1;
            temp |= (r[count] >> j) & 1;
        }
        if sparsecount != 0 {
            if l[i] != 0 {
                r[count] = temp;
                count += 1;
            }
        } else {
            r[count] = temp;
            count += 1;
        }
    }
    Some(r)
}

/// `_book_unquantize` with a sparse map (decode side).
fn book_unquantize(b: &StaticCodebook, n: usize, sparsemap: &[i32]) -> Vec<f32> {
    if b.maptype != 1 && b.maptype != 2 {
        return Vec::new();
    }
    if b.dim <= 0 {
        // The C computes `_book_maptype1_quantvals` first, which never
        // terminates for a zero dimension; nothing would be stored anyway.
        return Vec::new();
    }
    let dim = b.dim as usize;
    let mindel = float32_unpack(b.q_min);
    let delta = float32_unpack(b.q_delta);
    let mut r = vec![0f32; n * dim];
    let mut count = 0usize;
    // val=fabs(val)*delta+mindel+last, in double (C's fabs returns double).
    let deq = |q: i64, last: f32| -> f32 {
        let val = q as f32;
        ((val.abs() as f64) * (delta as f64) + mindel as f64 + last as f64) as f32
    };
    match b.maptype {
        1 => {
            let quantvals = book_maptype1_quantvals(b);
            for j in 0..b.entries {
                if b.lengthlist[j as usize] != 0 {
                    let mut last = 0f32;
                    let mut indexdiv: i64 = 1;
                    for k in 0..dim {
                        let index = ((j / indexdiv) % quantvals) as usize;
                        let val = deq(b.quantlist[index], last);
                        if b.q_sequencep != 0 {
                            last = val;
                        }
                        r[sparsemap[count] as usize * dim + k] = val;
                        indexdiv = (indexdiv as i32).wrapping_mul(quantvals as i32) as i64;
                    }
                    count += 1;
                }
            }
        }
        _ => {
            for j in 0..b.entries as usize {
                if b.lengthlist[j] != 0 {
                    let mut last = 0f32;
                    for k in 0..dim {
                        let val = deq(b.quantlist[j * dim + k], last);
                        if b.q_sequencep != 0 {
                            last = val;
                        }
                        r[sparsemap[count] as usize * dim + k] = val;
                    }
                    count += 1;
                }
            }
        }
    }
    r
}

#[inline]
fn bitreverse(x: u32) -> u32 {
    x.reverse_bits()
}

/// `codebook`, decode layout: only used entries, ordered by bit-reversed
/// codeword. The default value is the zeroed book a failed
/// `vorbis_book_init_decode` leaves behind.
#[derive(Clone, Debug, Default)]
pub(crate) struct Codebook {
    pub dim: i64,
    pub entries: i64,
    pub used_entries: i64,
    pub valuelist: Vec<f32>,
    codelist: Vec<u32>,
    dec_index: Vec<i32>,
    dec_codelengths: Vec<i8>,
    dec_firsttable: Vec<u32>,
    dec_firsttablen: i32,
    dec_maxlength: i32,
}

impl Codebook {
    /// `vorbis_book_init_decode`; on error the book is left zeroed and
    /// `Err` returned (the C returns -1).
    #[allow(clippy::result_large_err)]
    pub fn init_decode(s: &StaticCodebook) -> Result<Self, Self> {
        let mut c = Codebook::default();
        let n = s
            .lengthlist
            .iter()
            .take(s.entries.max(0) as usize)
            .filter(|&&l| l > 0)
            .count();
        c.entries = s.entries;
        c.used_entries = n as i64;
        c.dim = s.dim;

        if n > 0 {
            let Some(mut codes) = make_words(&s.lengthlist, s.entries, n as i64) else {
                return Err(Codebook::default());
            };
            for code in codes.iter_mut() {
                *code = bitreverse(*code);
            }
            // qsort of pointers by code value; the codes are distinct.
            let mut codep: Vec<u32> = (0..n as u32).collect();
            codep.sort_by_key(|&i| codes[i as usize]);
            let mut sortindex = vec![0i32; n];
            for (i, &position) in codep.iter().enumerate() {
                sortindex[position as usize] = i as i32;
            }
            c.codelist = vec![0u32; n];
            for i in 0..n {
                c.codelist[sortindex[i] as usize] = codes[i];
            }

            c.valuelist = book_unquantize(s, n, &sortindex);
            c.dec_index = vec![0i32; n];
            let mut k = 0usize;
            for i in 0..s.entries as usize {
                if s.lengthlist[i] > 0 {
                    c.dec_index[sortindex[k] as usize] = i as i32;
                    k += 1;
                }
            }
            c.dec_codelengths = vec![0i8; n];
            c.dec_maxlength = 0;
            k = 0;
            for i in 0..s.entries as usize {
                if s.lengthlist[i] > 0 {
                    c.dec_codelengths[sortindex[k] as usize] = s.lengthlist[i];
                    k += 1;
                    if s.lengthlist[i] as i32 > c.dec_maxlength {
                        c.dec_maxlength = s.lengthlist[i] as i32;
                    }
                }
            }

            if n == 1 && c.dec_maxlength == 1 {
                c.dec_firsttablen = 1;
                c.dec_firsttable = vec![1, 1];
            } else {
                c.dec_firsttablen = (ov_ilog(c.used_entries as u32) - 4).clamp(5, 8);
                let tabn = 1usize << c.dec_firsttablen;
                c.dec_firsttable = vec![0u32; tabn];
                for i in 0..n {
                    let len = c.dec_codelengths[i] as i32;
                    if len <= c.dec_firsttablen {
                        let orig = bitreverse(c.codelist[i]);
                        for j in 0..(1u32 << (c.dec_firsttablen - len)) {
                            c.dec_firsttable[(orig | (j << len)) as usize] = i as u32 + 1;
                        }
                    }
                }
                let mask: u32 = 0xffff_fffe_u32 << (31 - c.dec_firsttablen);
                let mut lo: usize = 0;
                let mut hi: usize = 0;
                for i in 0..tabn {
                    let word: u32 = (i as u32) << (32 - c.dec_firsttablen);
                    if c.dec_firsttable[bitreverse(word) as usize] == 0 {
                        while lo + 1 < n && c.codelist[lo + 1] <= word {
                            lo += 1;
                        }
                        while hi < n && word >= (c.codelist[hi] & mask) {
                            hi += 1;
                        }
                        let loval = (lo as u32).min(0x7fff);
                        let hival = ((n - hi) as u32).min(0x7fff);
                        c.dec_firsttable[bitreverse(word) as usize] =
                            0x8000_0000 | (loval << 15) | hival;
                    }
                }
            }
        }
        Ok(c)
    }

    /// `decode_packed_entry_number`: the packed (sorted) entry, -1 at the
    /// end of the packet. Callers check `used_entries > 0`.
    #[inline]
    pub fn decode_packed(&self, b: &mut BitReader) -> i64 {
        let mut read = self.dec_maxlength;
        let mut lo: i64;
        let mut hi: i64;
        let lok = b.look(self.dec_firsttablen);
        if lok >= 0 {
            let entry = self.dec_firsttable[lok as usize];
            if entry & 0x8000_0000 != 0 {
                lo = ((entry >> 15) & 0x7fff) as i64;
                hi = self.used_entries - (entry & 0x7fff) as i64;
            } else {
                // entry != 0 for every initialised book
                let e = entry as usize - 1;
                b.adv(self.dec_codelengths[e] as i32);
                return e as i64;
            }
        } else {
            lo = 0;
            hi = self.used_entries;
        }

        let mut lok = b.look(read);
        while lok < 0 && read > 1 {
            read -= 1;
            lok = b.look(read);
        }
        if lok < 0 {
            return -1;
        }

        let testword = bitreverse(lok as u32);
        while hi - lo > 1 {
            let p = (hi - lo) >> 1;
            let test = (self.codelist[(lo + p) as usize] > testword) as i64;
            lo += p & (test - 1);
            hi -= p & (-test);
        }
        if self.dec_codelengths[lo as usize] as i32 <= read {
            b.adv(self.dec_codelengths[lo as usize] as i32);
            return lo;
        }
        b.adv(read);
        -1
    }

    /// `vorbis_book_decode`: the original entry number, -1 on error.
    #[inline]
    pub fn decode(&self, b: &mut BitReader) -> i64 {
        if self.used_entries > 0 {
            let packed = self.decode_packed(b);
            if packed >= 0 {
                return self.dec_index[packed as usize] as i64;
            }
        }
        -1
    }

    /// `vorbis_book_decodevs_add` (residue 0): interleaved vector add.
    /// A zero-dimension book divides by zero in the C; here it is an error.
    pub fn decodevs_add(
        &self,
        a: &mut [f32],
        b: &mut BitReader,
        n: i32,
        entries: &mut Vec<usize>,
    ) -> i64 {
        if self.used_entries > 0 {
            if self.dim <= 0 {
                return -1;
            }
            let dim = self.dim as usize;
            let step = (n as i64 / self.dim) as usize;
            entries.clear();
            for _ in 0..step {
                let e = self.decode_packed(b);
                if e == -1 {
                    return -1;
                }
                entries.push(e as usize * dim);
            }
            let n = n as usize;
            let mut o = 0usize;
            for i in 0..dim {
                let mut j = 0usize;
                while o + j < n && j < step {
                    a[o + j] += self.valuelist[entries[j] + i];
                    j += 1;
                }
                o += step;
            }
        }
        0
    }

    /// `vorbis_book_decodev_add` (residue 1).
    pub fn decodev_add(&self, a: &mut [f32], b: &mut BitReader, n: i32) -> i64 {
        if self.used_entries > 0 {
            let n = n as usize;
            let dim = self.dim.max(0) as usize;
            let mut i = 0usize;
            while i < n {
                let entry = self.decode_packed(b);
                if entry == -1 {
                    return -1;
                }
                let t = &self.valuelist[entry as usize * dim..];
                let mut j = 0;
                while i < n && j < dim {
                    a[i] += t[j];
                    i += 1;
                    j += 1;
                }
            }
        }
        0
    }

    /// `vorbis_book_decodev_set` (floor 0).
    pub fn decodev_set(&self, a: &mut [f32], b: &mut BitReader, n: i32) -> i64 {
        let n = n.max(0) as usize;
        if self.used_entries > 0 {
            let dim = self.dim.max(0) as usize;
            let mut i = 0usize;
            while i < n {
                let entry = self.decode_packed(b);
                if entry == -1 {
                    return -1;
                }
                let t = &self.valuelist[entry as usize * dim..];
                let mut j = 0;
                while i < n && j < dim {
                    a[i] = t[j];
                    i += 1;
                    j += 1;
                }
            }
        } else {
            for v in a[..n].iter_mut() {
                *v = 0.0;
            }
        }
        0
    }

    /// `vorbis_book_decodevv_add` (residue 2): de-interleaves across the
    /// `ch` channel vectors starting at `offset`.
    pub fn decodevv_add(
        &self,
        a: &mut [&mut [f32]],
        offset: i64,
        ch: i32,
        b: &mut BitReader,
        n: i32,
    ) -> i64 {
        if self.used_entries > 0 {
            let ch_u = ch as usize;
            let dim = self.dim.max(0) as usize;
            let m = (offset + n as i64) / ch as i64;
            let mut i = offset / ch as i64;
            let mut chptr = 0usize;
            if ch_u == 1 {
                let out = &mut *a[0];
                while i < m {
                    let entry = self.decode_packed(b);
                    if entry == -1 {
                        return -1;
                    }
                    let t = &self.valuelist[entry as usize * dim..entry as usize * dim + dim];
                    for &v in t {
                        if i >= m {
                            break;
                        }
                        out[i as usize] += v;
                        i += 1;
                    }
                }
                return 0;
            }
            while i < m {
                let entry = self.decode_packed(b);
                if entry == -1 {
                    return -1;
                }
                let t = &self.valuelist[entry as usize * dim..entry as usize * dim + dim];
                for &v in t {
                    if i >= m {
                        break;
                    }
                    a[chptr][i as usize] += v;
                    chptr += 1;
                    if chptr == ch_u {
                        chptr = 0;
                        i += 1;
                    }
                }
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float32_unpack_examples() {
        // mantissa 1, exponent such that value == 1.0: exp field = 768+20
        let v = (788i64 << 21) | 1;
        assert_eq!(float32_unpack(v), 1.0);
        assert_eq!(float32_unpack(v | 0x8000_0000), -1.0);
    }

    #[test]
    fn quantvals_is_integer_root() {
        let mut b = StaticCodebook {
            entries: 81,
            dim: 4,
            ..Default::default()
        };
        assert_eq!(book_maptype1_quantvals(&b), 3);
        b.entries = 80;
        assert_eq!(book_maptype1_quantvals(&b), 2);
        b.entries = 1;
        b.dim = 3;
        assert_eq!(book_maptype1_quantvals(&b), 1);
    }
}
