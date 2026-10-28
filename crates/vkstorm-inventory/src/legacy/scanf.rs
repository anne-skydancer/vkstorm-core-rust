//! The `sscanf` conversions the legacy importers use (`%s`, `%[...]`, `%x`,
//! `%d`), with the C runtime behaviour they depend on.
//!
//! [`sscanf`] interprets a format the way the C library does for the
//! directives the readers use: white space (skips any amount, none
//! included), ordinary bytes (must match), `%<width>s`, `%<width>[set]` and
//! `%<width>[^set]`, `%d` and `%x`. It returns the C return value (the
//! number of conversions stored, or `EOF` = -1 when the input ends before the
//! first one) and the converted values in order; a conversion that fails
//! stores nothing, so the caller's previous value stands, as in C.
//!
//! Numeric overflow (runtime edge 5): both recorded C runtimes convert
//! through a saturating 64-bit value and store its low 32 bits, so
//! `fffffffff` reads `ffffffff`, `123456789` (hex) `23456789`,
//! `2147483648` -2147483648, `4294967301` 5, `-2147483649` 2147483647 and
//! `99999999999999999999` -1.

use super::reader::c_str;

/// The C `EOF` return of `sscanf`.
pub const EOF: i32 = -1;

/// One stored conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    /// `%s` or `%[...]`: the bytes stored, without the terminating NUL.
    Str(Vec<u8>),
    /// `%d`.
    Int(i32),
    /// `%x` (stored into an unsigned 32-bit variable).
    Hex(u32),
}

impl Arg {
    /// The bytes of a string conversion.
    pub fn bytes(&self) -> &[u8] {
        match self {
            Arg::Str(s) => s,
            _ => &[],
        }
    }
}

/// The result of [`sscanf`]: its return value and the stored conversions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scan {
    /// The C return value: conversions stored, or [`EOF`].
    pub ret: i32,
    /// The stored conversions, in format order.
    pub args: Vec<Arg>,
}

impl Scan {
    /// The `i`-th stored conversion's bytes, if it was stored.
    pub fn str(&self, i: usize) -> Option<&[u8]> {
        self.args.get(i).map(Arg::bytes)
    }
}

/// C `isspace` in the "C" locale: space, `\t`, `\n`, `\v`, `\f`, `\r`.
pub fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

fn skip_space(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    i
}

/// A scan set: `[set]` or `[^set]`, with `]` first taken literally and
/// `a-b` ranges as glibc and the MSVC runtime read them.
struct ScanSet {
    member: [bool; 256],
}

impl ScanSet {
    /// Parses the set after `[`; returns it and the index after `]`.
    fn parse(fmt: &[u8], mut i: usize) -> (ScanSet, usize) {
        let mut member = [false; 256];
        let negate = fmt.get(i) == Some(&b'^');
        if negate {
            i += 1;
        }
        let start = i;
        while i < fmt.len() && (fmt[i] != b']' || i == start) {
            let c = fmt[i];
            if i + 2 < fmt.len() && fmt[i + 1] == b'-' && fmt[i + 2] != b']' && fmt[i + 2] >= c {
                for b in c..=fmt[i + 2] {
                    member[b as usize] = true;
                }
                i += 3;
            } else {
                member[c as usize] = true;
                i += 1;
            }
        }
        if negate {
            for m in member.iter_mut() {
                *m = !*m;
            }
        }
        (ScanSet { member }, (i + 1).min(fmt.len()))
    }
}

/// Reads decimal or hexadecimal digits after an optional sign (and, for
/// hex, an optional `0x`), up to `width` bytes. Returns the stored 32-bit
/// value and the index after it, or `None` for a matching failure.
fn scan_number(s: &[u8], mut i: usize, width: usize, hex: bool) -> Option<(u32, usize)> {
    let end = s.len().min(i.saturating_add(width));
    let mut negative = false;
    if i < end && (s[i] == b'-' || s[i] == b'+') {
        negative = s[i] == b'-';
        i += 1;
    }
    let mut digits = 0usize;
    let radix: u64 = if hex { 16 } else { 10 };
    let mut acc: u64 = 0;
    let mut saturated = false;
    if hex && i + 1 < end && s[i] == b'0' && (s[i + 1] == b'x' || s[i + 1] == b'X') {
        // "0x": the '0' is a digit, so "0x" alone reads 0.
        digits = 1;
        i += 2;
    }
    while i < end {
        let d = match (s[i] as char).to_digit(radix as u32) {
            Some(d) => u64::from(d),
            None => break,
        };
        match acc.checked_mul(radix).and_then(|v| v.checked_add(d)) {
            Some(v) => acc = v,
            None => saturated = true,
        }
        digits += 1;
        i += 1;
    }
    if digits == 0 {
        return None;
    }
    let value: u64 = if hex {
        // strtoull: an overflow is U64_MAX whatever the sign; otherwise the
        // sign negates.
        if saturated {
            u64::MAX
        } else if negative {
            acc.wrapping_neg()
        } else {
            acc
        }
    } else {
        // strtoll: saturates at the S64 limits.
        let v: i64 = if saturated || acc > i64::MAX as u64 + u64::from(negative) {
            if negative { i64::MIN } else { i64::MAX }
        } else if negative {
            (acc as i64).wrapping_neg()
        } else {
            acc as i64
        };
        v as u64
    };
    Some((value as u32, i))
}

/// `sscanf(input, format, ...)` for the directives the legacy readers use
/// (see the module documentation). `input` is read as a C string (it ends
/// at its first NUL); a `%s`/`%[` width is the C width (the buffer size
/// minus one), and none means unlimited.
pub fn sscanf(input: &[u8], format: &str) -> Scan {
    let s = c_str(input);
    let fmt = format.as_bytes();
    let mut args = Vec::new();
    let mut i = 0usize; // input
    let mut f = 0usize; // format
    let mut input_failure = false;
    while f < fmt.len() {
        let c = fmt[f];
        if is_space(c) {
            i = skip_space(s, i);
            f += 1;
            continue;
        }
        if c != b'%' {
            if i >= s.len() {
                input_failure = true;
                break;
            }
            if s[i] != c {
                break;
            }
            i += 1;
            f += 1;
            continue;
        }
        // A conversion: %[width](s|d|x|[set]).
        f += 1;
        let mut width = 0usize;
        while f < fmt.len() && fmt[f].is_ascii_digit() {
            width = width * 10 + usize::from(fmt[f] - b'0');
            f += 1;
        }
        let width = if width == 0 { usize::MAX } else { width };
        let Some(&conv) = fmt.get(f) else { break };
        f += 1;
        match conv {
            b's' => {
                i = skip_space(s, i);
                if i >= s.len() {
                    input_failure = true;
                    break;
                }
                let start = i;
                while i < s.len() && !is_space(s[i]) && i - start < width {
                    i += 1;
                }
                args.push(Arg::Str(s[start..i].to_vec()));
            }
            b'[' => {
                let (set, next) = ScanSet::parse(fmt, f);
                f = next;
                if i >= s.len() {
                    input_failure = true;
                    break;
                }
                let start = i;
                while i < s.len() && set.member[s[i] as usize] && i - start < width {
                    i += 1;
                }
                if i == start {
                    break;
                }
                args.push(Arg::Str(s[start..i].to_vec()));
            }
            b'd' | b'x' => {
                i = skip_space(s, i);
                if i >= s.len() {
                    input_failure = true;
                    break;
                }
                match scan_number(s, i, width, conv == b'x') {
                    Some((v, next)) => {
                        i = next;
                        args.push(if conv == b'x' { Arg::Hex(v) } else { Arg::Int(v as i32) });
                    }
                    None => break,
                }
            }
            _ => break,
        }
    }
    let ret = if args.is_empty() && input_failure { EOF } else { args.len() as i32 };
    Scan { ret, args }
}

/// `1 == sscanf(s, "%x", &u32)`: the value, or `None` (the variable keeps
/// its value).
pub fn scan_hex(s: &[u8]) -> Option<u32> {
    match sscanf(s, "%x").args.first() {
        Some(Arg::Hex(v)) => Some(*v),
        _ => None,
    }
}

/// `1 == sscanf(s, "%d", &s32)`: the value, or `None` (the variable keeps
/// its value).
pub fn scan_int(s: &[u8]) -> Option<i32> {
    match sscanf(s, "%d").args.first() {
        Some(Arg::Int(v)) => Some(*v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_keeps_low_bits() {
        assert_eq!(scan_hex(b"fffffffff"), Some(0xffff_ffff));
        assert_eq!(scan_hex(b"123456789"), Some(0x2345_6789));
        assert_eq!(scan_hex(b"-1"), Some(0xffff_ffff));
        assert_eq!(scan_hex(b"0x"), Some(0));
        assert_eq!(scan_hex(b"g"), None);
        assert_eq!(scan_int(b"2147483648"), Some(i32::MIN));
        assert_eq!(scan_int(b"4294967301"), Some(5));
        assert_eq!(scan_int(b"-2147483649"), Some(i32::MAX));
        assert_eq!(scan_int(b"99999999999999999999"), Some(-1));
        assert_eq!(scan_int(b"-9223372036854775808"), Some(0));
        assert_eq!(scan_int(b"12abc"), Some(12));
        assert_eq!(scan_int(b"-"), None);
    }

    #[test]
    fn tokens_and_sets() {
        let r = sscanf(b"   ", " %255s %255s");
        assert_eq!(r.ret, EOF);
        let r = sscanf(b"\tkey\tvalue more", " %255s %255s");
        assert_eq!((r.ret, r.str(0), r.str(1)), (2, Some(&b"key"[..]), Some(&b"value"[..])));
        let r = sscanf(b"abcdef", " %3s %3s");
        assert_eq!((r.ret, r.str(0), r.str(1)), (2, Some(&b"abc"[..]), Some(&b"def"[..])));
        let r = sscanf(b"\t\tname\tA b|", " %254s%254[\t]%254[^|]");
        assert_eq!((r.ret, r.str(2)), (3, Some(&b"A b"[..])));
        let r = sscanf(b"\t\tname\t|", " %254s%254[\t]%254[^|]");
        assert_eq!(r.ret, 2);
        let r = sscanf(b"Text length 12", "Text length %d");
        assert_eq!(r.args, vec![Arg::Int(12)]);
    }
}
