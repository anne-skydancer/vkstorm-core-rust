//! The LSb-first bit reader of libogg 1.3.5's `bitwise.c` (`oggpack_*`,
//! read side), with its exact end-of-packet behaviour: a read past the end
//! returns -1 and leaves the reader in the overflow state, in which every
//! later read (even of zero bits) fails too.
//!
//! Values are returned as the C `long` would hold them: a 32-bit read of a
//! value with the top bit set is negative on Windows (32-bit `long`) and
//! positive elsewhere, which some header checks depend on.
//!
//! Original code: Copyright (c) 1994-2014 Xiph.Org Foundation, BSD-3-Clause
//! (see `LICENSE-xiph`). Rust port: Vulkanstorm.

use crate::clong;

const MASK: [u32; 33] = {
    let mut m = [0u32; 33];
    let mut i = 1;
    while i < 33 {
        m[i] = if i == 32 {
            0xffff_ffff
        } else {
            (1u32 << i) - 1
        };
        i += 1;
    }
    m
};

/// `oggpack_buffer` in read mode over a borrowed packet.
#[derive(Clone, Debug)]
pub(crate) struct BitReader<'a> {
    buf: &'a [u8],
    endbyte: usize,
    endbit: u32,
}

impl<'a> BitReader<'a> {
    /// `oggpack_readinit`.
    #[inline]
    pub fn new(buf: &'a [u8]) -> Self {
        Self {
            buf,
            endbyte: 0,
            endbit: 0,
        }
    }

    /// `opb->storage`.
    #[inline]
    pub fn storage(&self) -> i64 {
        self.buf.len() as i64
    }

    /// The not-main-path availability check shared by look/read/adv:
    /// true when `bits` (already including `endbit`) cannot be read.
    #[inline(always)]
    fn short(&self, total_bits: u32) -> bool {
        self.endbyte as i64 > self.buf.len() as i64 - ((total_bits as i64 + 7) >> 3)
    }

    /// The bits at the read position (up to 32 + 7 needed), the bytes past
    /// the buffer read as zero; callers have checked availability.
    #[inline(always)]
    fn peek_raw(&self) -> u64 {
        let p = self.endbyte;
        if let Some(b) = self.buf.get(p..p + 8) {
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            u64::from_le_bytes(a) >> self.endbit
        } else {
            let mut a = [0u8; 8];
            let avail = self.buf.len().saturating_sub(p).min(8);
            a[..avail].copy_from_slice(&self.buf[p..p + avail]);
            u64::from_le_bytes(a) >> self.endbit
        }
    }

    #[inline(always)]
    fn overflow(&mut self) {
        self.endbyte = self.buf.len();
        self.endbit = 1;
    }

    /// `oggpack_look`: the next `bits` (0..=32) without advancing, -1 when
    /// they are not all available.
    #[inline]
    pub fn look(&self, bits: i32) -> i64 {
        if !(0..=32).contains(&bits) {
            return -1;
        }
        let m = MASK[bits as usize];
        let total = bits as u32 + self.endbit;
        if self.endbyte as i64 >= self.buf.len() as i64 - 4 {
            if self.short(total) {
                return -1;
            } else if total == 0 {
                return 0;
            }
        }
        clong((self.peek_raw() as u32 & m) as i64)
    }

    /// `oggpack_adv`.
    #[inline]
    pub fn adv(&mut self, bits: i32) {
        let total = (bits + self.endbit as i32) as u32;
        if self.short(total) {
            self.overflow();
            return;
        }
        self.endbyte += (total / 8) as usize;
        self.endbit = total & 7;
    }

    /// `oggpack_read`: the next `bits` (0..=32), -1 past the end.
    #[inline]
    pub fn read(&mut self, bits: i32) -> i64 {
        if !(0..=32).contains(&bits) {
            self.overflow();
            return -1;
        }
        let m = MASK[bits as usize];
        let total = bits as u32 + self.endbit;
        if self.endbyte as i64 >= self.buf.len() as i64 - 4 {
            if self.short(total) {
                self.overflow();
                return -1;
            } else if total == 0 {
                return 0;
            }
        }
        let ret = self.peek_raw() as u32 & m;
        self.endbyte += (total / 8) as usize;
        self.endbit = total & 7;
        clong(ret as i64)
    }

    /// `oggpack_bytes`.
    #[inline]
    pub fn bytes(&self) -> i64 {
        self.endbyte as i64 + self.endbit.div_ceil(8) as i64
    }

    /// `oggpack_bits`.
    #[allow(dead_code)]
    #[inline]
    pub fn bits(&self) -> i64 {
        self.endbyte as i64 * 8 + self.endbit as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lsb_first_and_overflows_like_libogg() {
        let data = [0b1010_1100u8, 0xff, 0x01];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(2), 0b00);
        assert_eq!(r.read(3), 0b011);
        assert_eq!(r.read(3), 0b101);
        assert_eq!(r.read(9), 0x1ff);
        assert_eq!(r.read(7), 0);
        assert_eq!(r.read(0), 0);
        assert_eq!(r.read(1), -1);
        // overflow state: even a zero-bit read fails now
        assert_eq!(r.read(0), -1);
        assert_eq!(r.bytes(), 4);
    }

    #[test]
    fn thirty_two_bit_reads_follow_c_long() {
        let data = [0xff, 0xff, 0xff, 0xff, 0x80];
        let mut r = BitReader::new(&data);
        let v = r.read(32);
        if cfg!(windows) {
            assert_eq!(v, -1);
        } else {
            assert_eq!(v, 0xffff_ffff);
        }
    }
}
