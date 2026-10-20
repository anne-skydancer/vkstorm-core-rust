//! The text forms of `LLDataPackerAsciiBuffer` and `LLDataPackerAsciiFile`:
//! the C `printf` conversions the packers wrote with and the `sscanf`
//! conversions they read with, settled as one behaviour on every platform.
//!
//! Writing: `%d`/`%u` as the decimal digits; `%f` as the correctly rounded
//! six-decimal form (both C runtimes round correctly), with `inf`, `-inf`,
//! `nan` and `-nan` for the non-finite values (glibc's spellings; the MSVC
//! runtime wrote `-nan(ind)` for the indeterminate NaN); `%010d` sign-aware
//! zero padding; `%02x` two lowercase hex digits.
//!
//! Reading: `%d` and `%u` skip leading white space, take an optional sign and
//! decimal digits, and saturate at the 32-bit range (the MSVC runtime's
//! result; glibc went through a 64-bit `long` and truncated); `%f` takes the
//! longest decimal floating-point prefix (`inf`/`nan` spellings included,
//! hexadecimal floats not) and rounds correctly; a conversion that finds no
//! number assigns nothing, so the caller's value is unchanged (the C++ left
//! an uninitialised local in some of those cases). `%02x` takes at most two
//! hex digits. `%511[^\n]` takes up to 511 bytes before a newline and
//! assigns nothing when the first byte is a newline; `%511s` skips white
//! space and takes up to 511 non-white bytes.

use std::io::Write as _;

/// `%d` of an `int`.
pub fn fmt_i32(out: &mut Vec<u8>, v: i32) {
    let _ = write!(out, "{v}");
}

/// `%u` of an `unsigned`.
pub fn fmt_u32(out: &mut Vec<u8>, v: u32) {
    let _ = write!(out, "{v}");
}

/// `%f` of a float promoted to double.
pub fn fmt_f32(out: &mut Vec<u8>, v: f32) {
    let d = v as f64;
    if d.is_nan() {
        if d.is_sign_negative() {
            out.extend_from_slice(b"-nan");
        } else {
            out.extend_from_slice(b"nan");
        }
    } else if d.is_infinite() {
        if d < 0.0 {
            out.extend_from_slice(b"-inf");
        } else {
            out.extend_from_slice(b"inf");
        }
    } else {
        let _ = write!(out, "{d:.6}");
    }
}

/// `%010d`.
pub fn fmt_i32_zero10(out: &mut Vec<u8>, v: i32) {
    let _ = write!(out, "{v:010}");
}

/// `%02x` of a byte.
pub fn fmt_hex2(out: &mut Vec<u8>, v: u8) {
    let _ = write!(out, "{v:02x}");
}

fn is_c_space(c: u8) -> bool {
    c == b' ' || (b'\t'..=b'\r').contains(&c)
}

fn skip_space(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && is_c_space(s[i]) {
        i += 1;
    }
    i
}

/// `%d`: the value and the index after it, or `None` when there is no
/// number (nothing assigned).
pub fn scan_i32(s: &[u8], i: usize) -> Option<(i32, usize)> {
    let mut i = skip_space(s, i);
    let mut negative = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        negative = s[i] == b'-';
        i += 1;
    }
    let start = i;
    let mut value: i64 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        value = (value * 10 + (s[i] - b'0') as i64).min(1 << 40);
        i += 1;
    }
    if i == start {
        return None;
    }
    let value = if negative { -value } else { value };
    Some((value.clamp(i32::MIN as i64, i32::MAX as i64) as i32, i))
}

/// `%u`: an optional sign (negated modulo 2^32), digits, saturating.
pub fn scan_u32(s: &[u8], i: usize) -> Option<(u32, usize)> {
    let mut i = skip_space(s, i);
    let mut negative = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        negative = s[i] == b'-';
        i += 1;
    }
    let start = i;
    let mut value: u64 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        value = (value * 10 + (s[i] - b'0') as u64).min(1 << 40);
        i += 1;
    }
    if i == start {
        return None;
    }
    let value = value.min(u32::MAX as u64) as u32;
    Some((
        if negative {
            value.wrapping_neg()
        } else {
            value
        },
        i,
    ))
}

/// `%f` into a float.
pub fn scan_f32(s: &[u8], i: usize) -> Option<(f32, usize)> {
    let i = skip_space(s, i);
    let mut end = i;
    if end < s.len() && (s[end] == b'+' || s[end] == b'-') {
        end += 1;
    }
    let rest = &s[end..];
    let lower = |n: usize| rest.get(..n).map(|p| p.to_ascii_lowercase());
    if lower(8).as_deref() == Some(b"infinity") || lower(3).as_deref() == Some(b"inf") {
        let n = if lower(8).as_deref() == Some(b"infinity") {
            8
        } else {
            3
        };
        let v = if s[i] == b'-' {
            f32::NEG_INFINITY
        } else {
            f32::INFINITY
        };
        return Some((v, end + n));
    }
    if lower(3).as_deref() == Some(b"nan") {
        let v = if s[i] == b'-' { -f32::NAN } else { f32::NAN };
        return Some((v, end + 3));
    }
    let mut digits = 0;
    while end < s.len() && s[end].is_ascii_digit() {
        end += 1;
        digits += 1;
    }
    if end < s.len() && s[end] == b'.' {
        end += 1;
        while end < s.len() && s[end].is_ascii_digit() {
            end += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    if end < s.len() && (s[end] == b'e' || s[end] == b'E') {
        let mut e = end + 1;
        if e < s.len() && (s[e] == b'+' || s[e] == b'-') {
            e += 1;
        }
        if e < s.len() && s[e].is_ascii_digit() {
            while e < s.len() && s[e].is_ascii_digit() {
                e += 1;
            }
            end = e;
        }
    }
    let text = std::str::from_utf8(&s[i..end]).ok()?;
    let v: f32 = text.parse().ok()?;
    Some((v, end))
}

/// `%02x`: up to two hex digits after white space.
pub fn scan_hex2(s: &[u8], i: usize) -> Option<(u8, usize)> {
    let mut i = skip_space(s, i);
    let mut value: u32 = 0;
    let mut n = 0;
    // sscanf %x accepts an optional sign and 0x prefix within the width;
    // the packers never write them and two digits leave no room for them
    while n < 2 && i < s.len() {
        let d = match s[i] {
            c @ b'0'..=b'9' => c - b'0',
            c @ b'a'..=b'f' => c - b'a' + 10,
            c @ b'A'..=b'F' => c - b'A' + 10,
            _ => break,
        };
        value = value * 16 + d as u32;
        i += 1;
        n += 1;
    }
    if n == 0 {
        return None;
    }
    Some((value as u8, i))
}

/// `%<width>s`: skips white space, then up to `width` non-white bytes.
pub fn scan_token(s: &[u8], i: usize, width: usize) -> Option<(&[u8], usize)> {
    let start = skip_space(s, i);
    let mut end = start;
    while end < s.len() && end - start < width && !is_c_space(s[end]) && s[end] != 0 {
        end += 1;
    }
    if end == start {
        return None;
    }
    Some((&s[start..end], end))
}

/// `%<width>[^\n]`: up to `width` bytes before a newline (or NUL); nothing
/// when the first byte is a newline.
pub fn scan_line(s: &[u8], i: usize, width: usize) -> Option<(&[u8], usize)> {
    let mut end = i;
    while end < s.len() && end - i < width && s[end] != b'\n' && s[end] != 0 {
        end += 1;
    }
    if end == i {
        return None;
    }
    Some((&s[i..end], end))
}

/// `sscanf(line, "%511s %511[^\n]", keyword, value)`: the keyword and the
/// value (empty when absent).
pub fn split_keyword_value(line: &[u8]) -> (&[u8], &[u8]) {
    let Some((keyword, after)) = scan_token(line, 0, 511) else {
        return (&[], &[]);
    };
    let i = skip_space(line, after);
    let value = scan_line(line, i, 511).map_or(&line[..0], |(v, _)| v);
    (keyword, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut v = Vec::new();
        f(&mut v);
        String::from_utf8(v).unwrap()
    }

    #[test]
    fn formats() {
        assert_eq!(s(|o| fmt_f32(o, 4354355.5)), "4354355.500000");
        assert_eq!(s(|o| fmt_f32(o, 0.1)), "0.100000");
        assert_eq!(s(|o| fmt_f32(o, -0.0)), "-0.000000");
        assert_eq!(s(|o| fmt_f32(o, f32::NAN)), "nan");
        assert_eq!(s(|o| fmt_f32(o, f32::NEG_INFINITY)), "-inf");
        assert_eq!(s(|o| fmt_i32_zero10(o, 28)), "0000000028");
        assert_eq!(s(|o| fmt_i32_zero10(o, -5)), "-000000005");
        assert_eq!(s(|o| fmt_hex2(o, 0xab)), "ab");
        assert_eq!(s(|o| fmt_u32(o, u32::MAX)), "4294967295");
    }

    #[test]
    fn scans() {
        assert_eq!(scan_i32(b"  -94967295 x", 0), Some((-94967295, 11)));
        assert_eq!(scan_i32(b"99999999999", 0), Some((i32::MAX, 11)));
        assert_eq!(scan_i32(b"x", 0), None);
        assert_eq!(scan_u32(b"4294967295", 0), Some((u32::MAX, 10)));
        assert_eq!(scan_u32(b"-1", 0), Some((u32::MAX, 2)));
        assert_eq!(scan_f32(b"4354355.500000\n", 0), Some((4354355.5, 14)));
        assert_eq!(scan_f32(b"1e3", 0), Some((1000.0, 3)));
        assert_eq!(scan_f32(b"-inf", 0), Some((f32::NEG_INFINITY, 4)));
        assert!(scan_f32(b"nan", 0).unwrap().0.is_nan());
        assert_eq!(scan_f32(b".", 0), None);
        assert_eq!(scan_hex2(b" ff 0", 0), Some((255, 3)));
        assert_eq!(scan_hex2(b"abc", 0), Some((0xab, 2)));
        assert_eq!(scan_hex2(b"zz", 0), None);
        assert_eq!(scan_token(b"  key\tvalue", 0, 511), Some((&b"key"[..], 5)));
        assert_eq!(scan_line(b"\nx", 0, 511), None);
        assert_eq!(scan_line(b"a b\nc", 0, 511), Some((&b"a b"[..], 3)));
        assert_eq!(
            split_keyword_value(b"name\t1 2 3"),
            (&b"name"[..], &b"1 2 3"[..])
        );
        assert_eq!(split_keyword_value(b"name"), (&b"name"[..], &b""[..]));
        assert_eq!(split_keyword_value(b""), (&b""[..], &b""[..]));
    }
}
