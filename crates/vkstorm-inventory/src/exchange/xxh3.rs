//! XXH3-128 with the default secret and seed 0 (`XXH3_128bits`, xxHash
//! 0.8.2, the scalar path), for `LLUUID::generate(const std::string&)`
//! (`HBXXH128::digest`, `hbxxh.cpp`): the hash's low word in the UUID's first
//! eight bytes, the high word in the last eight, both little-endian.

const P32_1: u64 = 0x9E37_79B1;
const P32_2: u64 = 0x85EB_CA77;
const P32_3: u64 = 0xC2B2_AE3D;
const P64_1: u64 = 0x9E37_79B1_85EB_CA87;
const P64_2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const P64_3: u64 = 0x1656_67B1_9E37_79F9;
const P64_4: u64 = 0x85EB_CA77_C2B2_AE63;
const P64_5: u64 = 0x27D4_EB2F_1656_67C5;
const MX1: u64 = 0x1656_6791_9E37_79F9;
const MX2: u64 = 0x9FB2_1C65_1E98_DF25;

#[rustfmt::skip]
const SECRET: [u8; 192] = [
    0xb8, 0xfe, 0x6c, 0x39, 0x23, 0xa4, 0x4b, 0xbe, 0x7c, 0x01, 0x81, 0x2c, 0xf7, 0x21, 0xad, 0x1c,
    0xde, 0xd4, 0x6d, 0xe9, 0x83, 0x90, 0x97, 0xdb, 0x72, 0x40, 0xa4, 0xa4, 0xb7, 0xb3, 0x67, 0x1f,
    0xcb, 0x79, 0xe6, 0x4e, 0xcc, 0xc0, 0xe5, 0x78, 0x82, 0x5a, 0xd0, 0x7d, 0xcc, 0xff, 0x72, 0x21,
    0xb8, 0x08, 0x46, 0x74, 0xf7, 0x43, 0x24, 0x8e, 0xe0, 0x35, 0x90, 0xe6, 0x81, 0x3a, 0x26, 0x4c,
    0x3c, 0x28, 0x52, 0xbb, 0x91, 0xc3, 0x00, 0xcb, 0x88, 0xd0, 0x65, 0x8b, 0x1b, 0x53, 0x2e, 0xa3,
    0x71, 0x64, 0x48, 0x97, 0xa2, 0x0d, 0xf9, 0x4e, 0x38, 0x19, 0xef, 0x46, 0xa9, 0xde, 0xac, 0xd8,
    0xa8, 0xfa, 0x76, 0x3f, 0xe3, 0x9c, 0x34, 0x3f, 0xf9, 0xdc, 0xbb, 0xc7, 0xc7, 0x0b, 0x4f, 0x1d,
    0x8a, 0x51, 0xe0, 0x4b, 0xcd, 0xb4, 0x59, 0x31, 0xc8, 0x9f, 0x7e, 0xc9, 0xd9, 0x78, 0x73, 0x64,
    0xea, 0xc5, 0xac, 0x83, 0x34, 0xd3, 0xeb, 0xc3, 0xc5, 0x81, 0xa0, 0xff, 0xfa, 0x13, 0x63, 0xeb,
    0x17, 0x0d, 0xdd, 0x51, 0xb7, 0xf0, 0xda, 0x49, 0xd3, 0x16, 0x55, 0x26, 0x29, 0xd4, 0x68, 0x9e,
    0x2b, 0x16, 0xbe, 0x58, 0x7d, 0x47, 0xa1, 0xfc, 0x8f, 0xf8, 0xb8, 0xd1, 0x7a, 0xd0, 0x31, 0xce,
    0x45, 0xcb, 0x3a, 0x8f, 0x95, 0x16, 0x04, 0x28, 0xaf, 0xd7, 0xfb, 0xca, 0xbb, 0x4b, 0x40, 0x7e,
];

fn r64(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}

fn r32(b: &[u8], i: usize) -> u64 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as u64
}

fn mul128(a: u64, b: u64) -> (u64, u64) {
    let p = (a as u128) * (b as u128);
    (p as u64, (p >> 64) as u64)
}

fn fold64(a: u64, b: u64) -> u64 {
    let (lo, hi) = mul128(a, b);
    lo ^ hi
}

fn avalanche64(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(P64_2);
    h ^= h >> 29;
    h = h.wrapping_mul(P64_3);
    h ^ (h >> 32)
}

fn avalanche3(mut h: u64) -> u64 {
    h ^= h >> 37;
    h = h.wrapping_mul(MX1);
    h ^ (h >> 32)
}

fn mix16(input: &[u8], i: usize, s: usize) -> u64 {
    fold64(
        r64(input, i) ^ r64(&SECRET, s),
        r64(input, i + 8) ^ r64(&SECRET, s + 8),
    )
}

fn mix32(acc: (u64, u64), input: &[u8], i1: usize, i2: usize, s: usize) -> (u64, u64) {
    let mut lo = acc.0.wrapping_add(mix16(input, i1, s));
    lo ^= r64(input, i2).wrapping_add(r64(input, i2 + 8));
    let mut hi = acc.1.wrapping_add(mix16(input, i2, s + 16));
    hi ^= r64(input, i1).wrapping_add(r64(input, i1 + 8));
    (lo, hi)
}

fn finish_mid(acc: (u64, u64), len: u64) -> (u64, u64) {
    let lo = acc.0.wrapping_add(acc.1);
    let hi = acc
        .0
        .wrapping_mul(P64_1)
        .wrapping_add(acc.1.wrapping_mul(P64_4))
        .wrapping_add(len.wrapping_mul(P64_2));
    (avalanche3(lo), 0u64.wrapping_sub(avalanche3(hi)))
}

fn len_0to16(input: &[u8]) -> (u64, u64) {
    let len = input.len();
    if len > 8 {
        let flip_lo = r64(&SECRET, 32) ^ r64(&SECRET, 40);
        let flip_hi = r64(&SECRET, 48) ^ r64(&SECRET, 56);
        let in_lo = r64(input, 0);
        let mut in_hi = r64(input, len - 8);
        let (mut mlo, mut mhi) = mul128(in_lo ^ in_hi ^ flip_lo, P64_1);
        mlo = mlo.wrapping_add(((len - 1) as u64) << 54);
        in_hi ^= flip_hi;
        mhi = mhi
            .wrapping_add(in_hi)
            .wrapping_add((in_hi as u32 as u64).wrapping_mul(P32_2 - 1));
        mlo ^= mhi.swap_bytes();
        let (hlo, mut hhi) = mul128(mlo, P64_2);
        hhi = hhi.wrapping_add(mhi.wrapping_mul(P64_2));
        return (avalanche3(hlo), avalanche3(hhi));
    }
    if len >= 4 {
        let in_lo = r32(input, 0);
        let in_hi = r32(input, len - 4);
        let in64 = in_lo.wrapping_add(in_hi << 32);
        let flip = r64(&SECRET, 16) ^ r64(&SECRET, 24);
        let (mut lo, mut hi) = mul128(in64 ^ flip, P64_1.wrapping_add((len as u64) << 2));
        hi = hi.wrapping_add(lo << 1);
        lo ^= hi >> 3;
        lo ^= lo >> 35;
        lo = lo.wrapping_mul(MX2);
        lo ^= lo >> 28;
        return (lo, avalanche3(hi));
    }
    if len > 0 {
        let c1 = input[0] as u32;
        let c2 = input[len >> 1] as u32;
        let c3 = input[len - 1] as u32;
        let combined_lo = (c1 << 16) | (c2 << 24) | c3 | ((len as u32) << 8);
        let combined_hi = combined_lo.swap_bytes().rotate_left(13);
        let flip_lo = r32(&SECRET, 0) ^ r32(&SECRET, 4);
        let flip_hi = r32(&SECRET, 8) ^ r32(&SECRET, 12);
        return (
            avalanche64(combined_lo as u64 ^ flip_lo),
            avalanche64(combined_hi as u64 ^ flip_hi),
        );
    }
    (
        avalanche64(r64(&SECRET, 64) ^ r64(&SECRET, 72)),
        avalanche64(r64(&SECRET, 80) ^ r64(&SECRET, 88)),
    )
}

fn len_17to128(input: &[u8]) -> (u64, u64) {
    let len = input.len();
    let mut acc = ((len as u64).wrapping_mul(P64_1), 0u64);
    if len > 32 {
        if len > 64 {
            if len > 96 {
                acc = mix32(acc, input, 48, len - 64, 96);
            }
            acc = mix32(acc, input, 32, len - 48, 64);
        }
        acc = mix32(acc, input, 16, len - 32, 32);
    }
    acc = mix32(acc, input, 0, len - 16, 0);
    finish_mid(acc, len as u64)
}

fn len_129to240(input: &[u8]) -> (u64, u64) {
    let len = input.len();
    let mut acc = ((len as u64).wrapping_mul(P64_1), 0u64);
    let mut i = 32;
    while i < 160 {
        acc = mix32(acc, input, i - 32, i - 16, i - 32);
        i += 32;
    }
    acc = (avalanche3(acc.0), avalanche3(acc.1));
    let mut i = 160;
    while i <= len {
        acc = mix32(acc, input, i - 32, i - 16, 3 + i - 160);
        i += 32;
    }
    acc = mix32(acc, input, len - 16, len - 32, 136 - 17 - 16);
    finish_mid(acc, len as u64)
}

fn accumulate_512(acc: &mut [u64; 8], input: &[u8], at: usize, s: usize) {
    for lane in 0..8 {
        let v = r64(input, at + lane * 8);
        let k = v ^ r64(&SECRET, s + lane * 8);
        acc[lane ^ 1] = acc[lane ^ 1].wrapping_add(v);
        acc[lane] = (k & 0xffff_ffff)
            .wrapping_mul(k >> 32)
            .wrapping_add(acc[lane]);
    }
}

fn scramble(acc: &mut [u64; 8], s: usize) {
    for (lane, a) in acc.iter_mut().enumerate() {
        let mut v = *a;
        v ^= v >> 47;
        v ^= r64(&SECRET, s + lane * 8);
        *a = v.wrapping_mul(P32_1);
    }
}

fn merge(acc: &[u64; 8], s: usize, start: u64) -> u64 {
    let mut r = start;
    for i in 0..4 {
        r = r.wrapping_add(fold64(
            acc[2 * i] ^ r64(&SECRET, s + 16 * i),
            acc[2 * i + 1] ^ r64(&SECRET, s + 16 * i + 8),
        ));
    }
    avalanche3(r)
}

fn long(input: &[u8]) -> (u64, u64) {
    let len = input.len();
    let mut acc = [P32_3, P64_1, P64_2, P64_3, P64_4, P32_2, P64_5, P32_1];
    let stripes_per_block = (SECRET.len() - 64) / 8;
    let block_len = 64 * stripes_per_block;
    let blocks = (len - 1) / block_len;
    for n in 0..blocks {
        for st in 0..stripes_per_block {
            accumulate_512(&mut acc, input, n * block_len + st * 64, st * 8);
        }
        scramble(&mut acc, SECRET.len() - 64);
    }
    let stripes = ((len - 1) - block_len * blocks) / 64;
    for st in 0..stripes {
        accumulate_512(&mut acc, input, blocks * block_len + st * 64, st * 8);
    }
    accumulate_512(&mut acc, input, len - 64, SECRET.len() - 64 - 7);
    let lo = merge(&acc, 11, (len as u64).wrapping_mul(P64_1));
    let hi = merge(
        &acc,
        SECRET.len() - 64 - 11,
        !(len as u64).wrapping_mul(P64_2),
    );
    (lo, hi)
}

/// `XXH3_128bits(input)`: (low, high).
pub fn xxh3_128(input: &[u8]) -> (u64, u64) {
    match input.len() {
        0..=16 => len_0to16(input),
        17..=128 => len_17to128(input),
        129..=240 => len_129to240(input),
        _ => long(input),
    }
}

/// `LLUUID::generate(const std::string& hash_string)`.
pub fn uuid_from_string(s: &[u8]) -> [u8; 16] {
    let (lo, hi) = xxh3_128(s);
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&lo.to_le_bytes());
    id[8..].copy_from_slice(&hi.to_le_bytes());
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // XXH3_128bits("") (xxHash's sanity table, seed 0)
        assert_eq!(
            xxh3_128(b""),
            (0x6001_C324_468D_497F, 0x99AA_06D3_0147_98D8)
        );
    }
}
