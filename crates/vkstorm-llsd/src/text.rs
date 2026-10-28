//! Lexical helpers that reproduce the C and C++ library behaviour the viewer's
//! LLSD code relied on: `printf` real formatting, `std::istream` number
//! extraction, APR base64, `LLDate` and `LLUUID` text forms, and the escape
//! tables of the notation and XML formatters.

use crate::reader::{Reader, SliceSource};

// ---------------------------------------------------------------------------
// Character classes ("C" locale)

/// `isspace` in the "C" locale.
#[inline]
pub fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `isalpha` in the "C" locale.
#[inline]
pub fn is_alpha(b: u8) -> bool {
    b.is_ascii_alphabetic()
}

/// `hex_as_nybble` from llstring.cpp: non-hex characters count as 0.
#[inline]
pub fn hex_nybble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => 10 + b - b'a',
        b'A'..=b'F' => 10 + b - b'A',
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Real formatting

/// The `std::ios_base::floatfield` of the destination stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FloatField {
    /// Neither `fixed` nor `scientific`: `%.{p}g`.
    #[default]
    General,
    /// `std::fixed`: `%.{p}f`.
    Fixed,
    /// `std::scientific`: `%.{p}e`.
    Scientific,
}

fn fmt_nonfinite(v: f64, out: &mut Vec<u8>) {
    if v.is_infinite() {
        if v < 0.0 {
            out.push(b'-');
        }
        out.extend_from_slice(b"inf");
        return;
    }
    // NaN: the C runtime's spelling of the platform the viewer formats on.
    let bits = v.to_bits();
    let negative = bits >> 63 != 0;
    if cfg!(windows) {
        const QUIET: u64 = 0x0008_0000_0000_0000;
        let signalling = bits & QUIET == 0;
        if signalling {
            if negative {
                out.push(b'-');
            }
            out.extend_from_slice(b"nan(snan)");
        } else if bits == 0xFFF8_0000_0000_0000 {
            out.extend_from_slice(b"-nan(ind)");
        } else {
            if negative {
                out.push(b'-');
            }
            out.extend_from_slice(b"nan");
        }
    } else {
        if negative {
            out.push(b'-');
        }
        out.extend_from_slice(b"nan");
    }
}

fn trim_zeros(d: &[u8]) -> &[u8] {
    let mut n = d.len();
    while n > 0 && d[n - 1] == b'0' {
        n -= 1;
    }
    &d[..n]
}

fn push_exponent(x: i32, out: &mut Vec<u8>) {
    out.push(b'e');
    out.push(if x < 0 { b'-' } else { b'+' });
    let ax = x.unsigned_abs();
    if ax < 10 {
        out.push(b'0');
    }
    out.extend_from_slice(ax.to_string().as_bytes());
}

/// `printf("%.*g", prec, v)`.
pub fn fmt_g(v: f64, prec: usize, out: &mut Vec<u8>) {
    if !v.is_finite() {
        return fmt_nonfinite(v, out);
    }
    let p = prec.max(1);
    if v == 0.0 {
        if v.is_sign_negative() {
            out.push(b'-');
        }
        out.push(b'0');
        return;
    }
    // Correctly rounded to p significant digits, like the C runtime.
    let s = format!("{:.*e}", p - 1, v.abs());
    let (mant, exp) = s.split_once('e').expect("exponent form");
    let x: i32 = exp.parse().expect("exponent value");
    let digits: Vec<u8> = mant.bytes().filter(|b| *b != b'.').collect();
    if v.is_sign_negative() {
        out.push(b'-');
    }
    if x < -4 || x >= p as i32 {
        out.push(digits[0]);
        let frac = trim_zeros(&digits[1..]);
        if !frac.is_empty() {
            out.push(b'.');
            out.extend_from_slice(frac);
        }
        push_exponent(x, out);
    } else if x >= 0 {
        let ip = x as usize + 1;
        out.extend_from_slice(&digits[..ip]);
        let frac = trim_zeros(&digits[ip..]);
        if !frac.is_empty() {
            out.push(b'.');
            out.extend_from_slice(frac);
        }
    } else {
        out.extend_from_slice(b"0.");
        for _ in 0..(-x - 1) {
            out.push(b'0');
        }
        out.extend_from_slice(trim_zeros(&digits));
    }
}

/// `printf("%.*f", prec, v)`.
pub fn fmt_f(v: f64, prec: usize, out: &mut Vec<u8>) {
    if !v.is_finite() {
        return fmt_nonfinite(v, out);
    }
    out.extend_from_slice(format!("{:.*}", prec, v).as_bytes());
}

/// `printf("%.*e", prec, v)`.
pub fn fmt_e(v: f64, prec: usize, out: &mut Vec<u8>) {
    if !v.is_finite() {
        return fmt_nonfinite(v, out);
    }
    let s = format!("{:.*e}", prec, v);
    let (mant, exp) = s.split_once('e').expect("exponent form");
    out.extend_from_slice(mant.as_bytes());
    push_exponent(exp.parse().expect("exponent value"), out);
}

/// `ostream << v` with the stream's precision and floatfield.
pub fn fmt_real(v: f64, prec: usize, ff: FloatField, out: &mut Vec<u8>) {
    match ff {
        FloatField::General => fmt_g(v, prec, out),
        FloatField::Fixed => fmt_f(v, prec, out),
        FloatField::Scientific => fmt_e(v, prec, out),
    }
}

// ---------------------------------------------------------------------------
// istream-style extraction

fn skip_ws(r: &mut Reader<'_>) {
    while let Some(b) = r.peek() {
        if !is_space(b) {
            break;
        }
        r.get();
    }
}

/// `istream >> int` (decimal, "C" locale): skips whitespace, takes an optional
/// sign and digits, stops at the first other byte. `None` when no digit was
/// read or the value does not fit an `i32` (the stream's failbit).
pub fn extract_int(r: &mut Reader<'_>) -> Option<i32> {
    skip_ws(r);
    let mut negative = false;
    match r.peek() {
        Some(b'-') => {
            negative = true;
            r.get();
        }
        Some(b'+') => {
            r.get();
        }
        _ => {}
    }
    let mut digits = 0usize;
    let mut acc: i64 = 0;
    let mut overflow = false;
    while let Some(b) = r.peek() {
        if !b.is_ascii_digit() {
            break;
        }
        r.get();
        digits += 1;
        if !overflow {
            acc = acc * 10 + (b - b'0') as i64;
            if acc > i32::MAX as i64 + 1 {
                overflow = true;
            }
        }
    }
    if digits == 0 || overflow {
        return None;
    }
    let v = if negative { -acc } else { acc };
    i32::try_from(v).ok()
}

/// `istream >> double` ("C" locale): skips whitespace, then an optional sign,
/// digits with an optional point, and an optional exponent; a `0x` prefix
/// starts a hexadecimal mantissa with a binary `p` exponent, as the MSVC
/// runtime's extraction accepts. `None` sets the failbit in C++: no mantissa
/// digit, an exponent marker without digits, or a value outside the finite
/// range.
pub fn extract_real(r: &mut Reader<'_>) -> Option<f64> {
    let (v, ok) = extract_real_stored(r);
    if ok { Some(v) } else { None }
}

/// As [`extract_real`], also returning the value the C++ extraction stores
/// when it fails: +/-infinity on overflow, otherwise 0.
///
/// A mantissa that underflows (`1e-999`) is accepted with the value
/// `strtod` gives (0 or a denormal), as glibc's extraction does; the MSVC
/// runtime rejected it. Accepting it is the product behaviour on every
/// platform: failing a whole document over an underflowing real is the
/// worse outcome, and no formatter produces such a value.
pub fn extract_real_stored(r: &mut Reader<'_>) -> (f64, bool) {
    skip_ws(r);
    let mut text: Vec<u8> = Vec::with_capacity(32);
    let mut negative = false;
    if let Some(b @ (b'-' | b'+')) = r.peek() {
        negative = b == b'-';
        text.push(b);
        r.get();
    }
    let mut mantissa_digits = 0;
    let mut mantissa_nonzero = false;
    if r.peek() == Some(b'0') {
        r.get();
        if matches!(r.peek(), Some(b'x' | b'X')) {
            r.get();
            return extract_hex_real(r, negative);
        }
        text.push(b'0');
        mantissa_digits += 1;
    }
    let fail = |v: f64| (v, false);
    while let Some(b) = r.peek() {
        if !b.is_ascii_digit() {
            break;
        }
        text.push(b);
        r.get();
        mantissa_digits += 1;
        mantissa_nonzero |= b != b'0';
    }
    if r.peek() == Some(b'.') {
        text.push(b'.');
        r.get();
        while let Some(b) = r.peek() {
            if !b.is_ascii_digit() {
                break;
            }
            text.push(b);
            r.get();
            mantissa_digits += 1;
            mantissa_nonzero |= b != b'0';
        }
    }
    if mantissa_digits == 0 {
        return fail(0.0);
    }
    if let Some(b'e' | b'E') = r.peek() {
        text.push(b'e');
        r.get();
        if let Some(b @ (b'-' | b'+')) = r.peek() {
            text.push(b);
            r.get();
        }
        let mut exp_digits = 0;
        while let Some(b) = r.peek() {
            if !b.is_ascii_digit() {
                break;
            }
            text.push(b);
            r.get();
            exp_digits += 1;
        }
        if exp_digits == 0 {
            return fail(0.0);
        }
    }
    let Ok(s) = std::str::from_utf8(&text) else {
        return fail(0.0);
    };
    let Ok(v) = s.parse::<f64>() else {
        return fail(0.0);
    };
    if v.is_infinite() {
        return fail(v);
    }
    let _ = mantissa_nonzero; // underflow is accepted; see above
    (v, true)
}

/// The hexadecimal form after `0x`: hex digits with an optional point, then
/// an optional `p` exponent in decimal.
fn extract_hex_real(r: &mut Reader<'_>, negative: bool) -> (f64, bool) {
    let mut mantissa = 0.0f64;
    let mut digits = 0;
    let mut frac_digits = 0i32;
    let mut in_frac = false;
    loop {
        match r.peek() {
            Some(b) if b.is_ascii_hexdigit() => {
                r.get();
                mantissa = mantissa * 16.0 + hex_nybble(b) as f64;
                digits += 1;
                if in_frac {
                    frac_digits += 1;
                }
            }
            Some(b'.') if !in_frac => {
                r.get();
                in_frac = true;
            }
            _ => break,
        }
    }
    if digits == 0 {
        return (0.0, false);
    }
    let mut exp: i32 = 0;
    if let Some(b'p' | b'P') = r.peek() {
        r.get();
        let mut exp_negative = false;
        if let Some(b @ (b'-' | b'+')) = r.peek() {
            exp_negative = b == b'-';
            r.get();
        }
        let mut exp_digits = 0;
        while let Some(b) = r.peek() {
            if !b.is_ascii_digit() {
                break;
            }
            r.get();
            exp = exp.saturating_mul(10).saturating_add((b - b'0') as i32);
            exp_digits += 1;
        }
        if exp_digits == 0 {
            return (0.0, false);
        }
        if exp_negative {
            exp = -exp;
        }
    }
    let v = mantissa * 2f64.powi(exp.saturating_sub(4 * frac_digits));
    let v = if negative { -v } else { v };
    (v, v.is_finite())
}

/// `LLSD(string).asReal()` as the MSVC runtime computes it:
/// `istringstream >> v`, then `get()` must hit the end of the string, else
/// 0.0. After a failed extraction `get()` reports EOF at once, so the value
/// the extraction stored (infinity on overflow) is what comes back.
///
/// The LLSD XML parser uses this on every platform (the serializer rung's
/// qualified behaviour). Code that reproduces the viewer's own C++ `LLSD`
/// coercions uses [`string_as_real_native`] instead.
pub fn string_as_real(s: &[u8]) -> f64 {
    let mut src = SliceSource::new(s);
    let mut r = Reader::new(&mut src);
    let (v, ok) = extract_real_stored(&mut r);
    if !ok {
        return v;
    }
    if r.peek().is_none() { v } else { 0.0 }
}

/// The MSVC form of `LLSD(string).asReal()`; the same function as
/// [`string_as_real`], named for the runtime it reproduces.
pub fn string_as_real_msvc(s: &[u8]) -> f64 {
    string_as_real(s)
}

/// `LLSD(string).asReal()` as libstdc++ computes it.
///
/// libstdc++'s `num_get<double>` (`_M_extract_float`) collects, after the
/// sentry skips whitespace: an optional sign, then digits, at most one `.`
/// (not after the exponent marker) and at most one `e`/`E`, accepted only
/// after a mantissa digit and followed by an optional sign. It stops at the
/// first other byte; there is no hexadecimal, infinity or NaN form. The
/// collected text then goes to `strtod` (`__convert_to_v`): unless `strtod`
/// consumes all of it the result is 0 with the failbit set, and a result of
/// +/-infinity becomes +/-`DBL_MAX` with the failbit set. Underflow is
/// accepted with the value `strtod` gives. After a failure `get()` reports
/// EOF, so the stored value comes back; after a success the string must be
/// exhausted, else 0.0.
pub fn string_as_real_libstdcxx(s: &[u8]) -> f64 {
    let mut i = 0;
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    if i == s.len() {
        return 0.0; // the sentry fails; the value keeps its 0.0
    }
    let mut text: Vec<u8> = Vec::with_capacity(32);
    if s[i] == b'+' || s[i] == b'-' {
        text.push(s[i]);
        i += 1;
    }
    let mut found_mantissa = false;
    let mut found_dec = false;
    let mut found_sci = false;
    while i < s.len() {
        let c = s[i];
        if c.is_ascii_digit() {
            text.push(c);
            found_mantissa = true;
        } else if c == b'.' && !found_dec && !found_sci {
            text.push(b'.');
            found_dec = true;
        } else if (c == b'e' || c == b'E') && !found_sci && found_mantissa {
            text.push(b'e');
            found_sci = true;
            if i + 1 < s.len() && (s[i + 1] == b'+' || s[i + 1] == b'-') {
                i += 1;
                text.push(s[i]);
            }
        } else {
            break;
        }
        i += 1;
    }
    let at_end = i == s.len();
    let (v, ok) = libstdcxx_convert(&text);
    if !ok {
        return v;
    }
    if at_end { v } else { 0.0 }
}

/// `__convert_to_v` for double: `strtod` must consume the whole collected
/// text, which only holds `[+-]`, digits, `.` and `e`.
fn libstdcxx_convert(text: &[u8]) -> (f64, bool) {
    let mut i = 0;
    if i < text.len() && (text[i] == b'+' || text[i] == b'-') {
        i += 1;
    }
    let mut digits = 0;
    while i < text.len() && text[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < text.len() && text[i] == b'.' {
        i += 1;
        while i < text.len() && text[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return (0.0, false);
    }
    if i < text.len() && text[i] == b'e' {
        i += 1;
        if i < text.len() && (text[i] == b'+' || text[i] == b'-') {
            i += 1;
        }
        let mut exp_digits = 0;
        while i < text.len() && text[i].is_ascii_digit() {
            i += 1;
            exp_digits += 1;
        }
        if exp_digits == 0 {
            return (0.0, false); // strtod stops before the `e`
        }
    }
    if i != text.len() {
        return (0.0, false);
    }
    let Ok(v) = std::str::from_utf8(text).unwrap_or("").parse::<f64>() else {
        return (0.0, false);
    };
    if v == f64::INFINITY {
        (f64::MAX, false)
    } else if v == f64::NEG_INFINITY {
        (f64::MIN, false)
    } else {
        (v, true)
    }
}

/// `LLSD(string).asReal()` as the C++ runtime of this build computes it: the
/// MSVC form under the MSVC target environment, the libstdc++ form
/// otherwise. This is what the viewer's own `LLSD` class returns on the same
/// platform (port rule 1).
pub fn string_as_real_native(s: &[u8]) -> f64 {
    #[cfg(target_env = "msvc")]
    {
        string_as_real_msvc(s)
    }
    #[cfg(not(target_env = "msvc"))]
    {
        string_as_real_libstdcxx(s)
    }
}

/// `(int)` of a double as x86 does it: out-of-range and NaN give `i32::MIN`.
pub fn real_as_integer(v: f64) -> i32 {
    if v.is_nan() || v >= 2147483648.0 || v < -2147483648.0 {
        i32::MIN
    } else {
        v as i32
    }
}

/// `sscanf(s, "%d", &i) == 1`: leading whitespace, optional sign, decimal
/// digits; stops at the first other byte. The UCRT converts through a
/// saturating 64-bit signed value and stores its low 32 bits.
pub fn sscanf_int(s: &[u8]) -> Option<i32> {
    let mut i = 0;
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    let mut negative = false;
    if i < s.len() && (s[i] == b'-' || s[i] == b'+') {
        negative = s[i] == b'-';
        i += 1;
    }
    let mut digits = 0;
    let mut acc: i64 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        let d = (s[i] - b'0') as i64;
        acc = if negative {
            acc.saturating_mul(10).saturating_sub(d)
        } else {
            acc.saturating_mul(10).saturating_add(d)
        };
        digits += 1;
        i += 1;
    }
    if digits == 0 {
        return None;
    }
    Some(acc as i32)
}

/// `strtol(s, NULL, 0)`: leading whitespace, sign, then `0x` hex, leading-`0`
/// octal or decimal digits. Stops at the first invalid byte; 0 when none.
pub fn strtol0(s: &[u8]) -> i64 {
    let mut i = 0;
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    let mut negative = false;
    if i < s.len() && (s[i] == b'-' || s[i] == b'+') {
        negative = s[i] == b'-';
        i += 1;
    }
    let mut base = 10u32;
    if i + 1 < s.len()
        && s[i] == b'0'
        && (s[i + 1] == b'x' || s[i + 1] == b'X')
        && i + 2 < s.len()
        && s[i + 2].is_ascii_hexdigit()
    {
        base = 16;
        i += 2;
    } else if i < s.len() && s[i] == b'0' {
        base = 8;
    }
    let mut acc: i64 = 0;
    while i < s.len() {
        let d = match (s[i] as char).to_digit(base) {
            Some(d) => d as i64,
            None => break,
        };
        acc = acc.saturating_mul(base as i64).saturating_add(d);
        i += 1;
    }
    if negative { -acc } else { acc }
}

// ---------------------------------------------------------------------------
// UUID

/// Lower-case hyphenated hex, as `LLUUID::toString`.
pub fn uuid_to_hex(u: &[u8; 16], out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, b) in u.iter().enumerate() {
        if i == 4 || i == 6 || i == 8 || i == 10 {
            out.push(b'-');
        }
        out.push(HEX[(b >> 4) as usize]);
        out.push(HEX[(b & 15) as usize]);
    }
}

pub const NULL_UUID: [u8; 16] = [0; 16];

/// `LLUUID::set(string)`: 36 characters with the hyphen slots skipped but not
/// checked, the historical 35-character form without the fourth hyphen,
/// or the empty string; anything else (or a non-hex digit) is the null UUID.
pub fn uuid_set(s: &[u8]) -> [u8; 16] {
    if s.is_empty() {
        return NULL_UUID;
    }
    let broken = match s.len() {
        36 => false,
        35 => true,
        _ => return NULL_UUID,
    };
    let mut out = [0u8; 16];
    let mut pos = 0usize;
    for (i, slot) in out.iter_mut().enumerate() {
        if i == 4 || i == 6 || i == 8 || i == 10 {
            pos += 1;
            if broken && i == 10 {
                pos -= 1;
            }
        }
        let hi = s[pos];
        let lo = s[pos + 1];
        if !hi.is_ascii_hexdigit() || !lo.is_ascii_hexdigit() {
            return NULL_UUID;
        }
        *slot = hex_nybble(hi) << 4 | hex_nybble(lo);
        pos += 2;
    }
    out
}

// ---------------------------------------------------------------------------
// Dates (LLDate on APR)

const USEC_PER_SEC: i64 = 1_000_000;

/// Proleptic Gregorian civil date from days since 1970-01-01 (floor based).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn push_padded(v: i64, width: usize, out: &mut Vec<u8>) {
    let s = v.to_string();
    for _ in s.len()..width {
        out.push(b'0');
    }
    out.extend_from_slice(s.as_bytes());
}

/// `LLDate::toStream`: `YYYY-MM-DDTHH:MM:SS[.FF]Z` for seconds since the epoch.
/// The microsecond count is the C++ cast `(apr_time_t)(seconds * 1e6)`, the
/// seconds field C-truncated and the calendar fields `gmtime`'s.
pub fn date_format(seconds: f64, out: &mut Vec<u8>) {
    let scaled = seconds * USEC_PER_SEC as f64;
    let usec_total: i64 = if scaled.is_nan() { 0 } else { scaled as i64 };
    let sec = usec_total / USEC_PER_SEC;
    let usec = usec_total % USEC_PER_SEC;
    let days = sec.div_euclid(86400);
    let sod = sec.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    if y < i32::MIN as i64 || y > i32::MAX as i64 {
        out.extend_from_slice(b"1970-01-01T00:00:00Z");
        return;
    }
    push_padded(y, 4, out);
    out.push(b'-');
    push_padded(m as i64, 2, out);
    out.push(b'-');
    push_padded(d as i64, 2, out);
    out.push(b'T');
    push_padded(sod / 3600, 2, out);
    out.push(b':');
    push_padded((sod / 60) % 60, 2, out);
    out.push(b':');
    push_padded(sod % 60, 2, out);
    if usec > 0 {
        out.push(b'.');
        push_padded(usec / 10000, 2, out);
    }
    out.push(b'Z');
}

/// `LLDate::fromStream` on `apr_time_exp_gmt_get`: `YYYY-MM-DDTHH:MM:SS`, an
/// optional `.fraction`, then `Z` or a `+HH[:MM]`/`-HH[:MM]` offset. Each
/// number is an `istream >> int` extraction. Dates before the epoch fail.
pub fn date_parse(s: &[u8]) -> Option<f64> {
    let mut src = SliceSource::new(s);
    let mut r = Reader::new(&mut src);
    let year = extract_int(&mut r)? as i64;
    if r.get()? != b'-' {
        return None;
    }
    let month = extract_int(&mut r)? as i64;
    if r.get()? != b'-' {
        return None;
    }
    let day = extract_int(&mut r)? as i64;
    if r.get()? != b'T' {
        return None;
    }
    let hour = extract_int(&mut r)? as i64;
    if r.get()? != b':' {
        return None;
    }
    let min = extract_int(&mut r)? as i64;
    if r.get()? != b':' {
        return None;
    }
    let sec = extract_int(&mut r)? as i64;

    // apr_time_exp_get: days since 1 March 1900 with the year shifted so leap
    // days fall at the end.
    const DAYOFFSET: [i64; 12] = [306, 337, 0, 31, 61, 92, 122, 153, 184, 214, 245, 275];
    let mon = month - 1;
    if !(0..12).contains(&mon) {
        return None;
    }
    let mut y = year - 1900;
    if mon < 2 {
        y -= 1;
    }
    let mut days = y * 365 + y / 4 - y / 100 + (y / 100 + 3) / 4;
    days += DAYOFFSET[mon as usize] + day - 1;
    days -= 25508;
    let total = ((days * 24 + hour) * 60 + min) * 60 + sec;
    if total < 0 {
        return None;
    }
    let mut seconds = total as f64;

    if r.peek() == Some(b'.') {
        // `s >> fractional` on ".FF": the point starts the mantissa.
        let frac = extract_real(&mut r)?;
        seconds += frac;
    }
    match r.peek() {
        Some(c @ (b'+' | b'-')) => {
            let sign: i64 = if c == b'+' { 1 } else { -1 };
            let hours = extract_int(&mut r)? as i64;
            let mut minutes = 0i64;
            if r.get() == Some(b':') {
                minutes = extract_int(&mut r)? as i64;
            }
            let offset = (hours * 60 + sign * minutes) * 60;
            seconds -= offset as f64;
        }
        Some(b'Z') => {}
        _ => return None,
    }
    Some(seconds)
}

// ---------------------------------------------------------------------------
// Base64 (APR) and base16

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `apr_base64_encode_binary`: standard alphabet, `=` padding, no line breaks.
pub fn base64_encode(data: &[u8], out: &mut Vec<u8>) {
    let (chunks, rest) = data.as_chunks::<3>();
    for c in chunks {
        out.push(B64[(c[0] >> 2) as usize]);
        out.push(B64[(((c[0] & 3) << 4) | (c[1] >> 4)) as usize]);
        out.push(B64[(((c[1] & 15) << 2) | (c[2] >> 6)) as usize]);
        out.push(B64[(c[2] & 63) as usize]);
    }
    match rest.len() {
        1 => {
            out.push(B64[(rest[0] >> 2) as usize]);
            out.push(B64[((rest[0] & 3) << 4) as usize]);
            out.extend_from_slice(b"==");
        }
        2 => {
            out.push(B64[(rest[0] >> 2) as usize]);
            out.push(B64[(((rest[0] & 3) << 4) | (rest[1] >> 4)) as usize]);
            out.push(B64[((rest[1] & 15) << 2) as usize]);
            out.push(b'=');
        }
        _ => {}
    }
}

#[inline]
fn pr2six(b: u8) -> u8 {
    match b {
        b'A'..=b'Z' => b - b'A',
        b'a'..=b'z' => b - b'a' + 26,
        b'0'..=b'9' => b - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => 64,
    }
}

/// `apr_base64_decode_binary`: decodes the leading run of alphabet characters
/// (padding or any other byte ends it) and ignores a dangling single sextet.
pub fn base64_decode_apr(coded: &[u8], out: &mut Vec<u8>) {
    let n = coded.iter().take_while(|&&b| pr2six(b) <= 63).count();
    let six = |i: usize| pr2six(coded[i]);
    let mut i = 0;
    let mut left = n;
    while left > 4 {
        out.push(six(i) << 2 | six(i + 1) >> 4);
        out.push(six(i + 1) << 4 | six(i + 2) >> 2);
        out.push(six(i + 2) << 6 | six(i + 3));
        i += 4;
        left -= 4;
    }
    if left > 1 {
        out.push(six(i) << 2 | six(i + 1) >> 4);
    }
    if left > 2 {
        out.push(six(i + 1) << 4 | six(i + 2) >> 2);
    }
    if left > 3 {
        out.push(six(i + 2) << 6 | six(i + 3));
    }
}

/// Upper-case hex pairs, as the notation formatter's `b16` form.
pub fn hex_upper(data: &[u8], out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.reserve(data.len() * 2);
    for b in data {
        out.push(HEX[(b >> 4) as usize]);
        out.push(HEX[(b & 15) as usize]);
    }
}

// ---------------------------------------------------------------------------
// Escapes

/// The notation string escape of `serialize_string`: control bytes and bytes
/// above 0x7e as `\xHH`, the C escapes for 7..13, `\'` and `\\`.
pub fn notation_escape(s: &[u8], out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.reserve(s.len());
    for &b in s {
        match b {
            7 => out.extend_from_slice(b"\\a"),
            8 => out.extend_from_slice(b"\\b"),
            9 => out.extend_from_slice(b"\\t"),
            10 => out.extend_from_slice(b"\\n"),
            11 => out.extend_from_slice(b"\\v"),
            12 => out.extend_from_slice(b"\\f"),
            13 => out.extend_from_slice(b"\\r"),
            b'\'' => out.extend_from_slice(b"\\'"),
            b'\\' => out.extend_from_slice(b"\\\\"),
            0x20..=0x7e => out.push(b),
            _ => {
                out.extend_from_slice(b"\\x");
                out.push(HEX[(b >> 4) as usize]);
                out.push(HEX[(b & 15) as usize]);
            }
        }
    }
}

/// `LLSDXMLFormatter::escapeString`: the five XML entities, with control
/// bytes below 20 (decimal, as the viewer's check reads) other than tab, LF
/// and CR replaced by `?`.
pub fn xml_escape(s: &[u8], out: &mut Vec<u8>) {
    out.reserve(s.len());
    for &b in s {
        match b {
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'&' => out.extend_from_slice(b"&amp;"),
            b'\'' => out.extend_from_slice(b"&apos;"),
            b'"' => out.extend_from_slice(b"&quot;"),
            0..=19 if b != 9 && b != 10 && b != 13 => out.push(b'?'),
            _ => out.push(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(v: f64, p: usize) -> String {
        let mut o = Vec::new();
        fmt_g(v, p, &mut o);
        String::from_utf8(o).unwrap()
    }

    #[test]
    fn printf_g() {
        assert_eq!(g(1234.5, 6), "1234.5");
        assert_eq!(g(100000.0, 6), "100000");
        assert_eq!(g(1000000.0, 6), "1e+06");
        assert_eq!(g(0.0001, 6), "0.0001");
        assert_eq!(g(0.00001, 6), "1e-05");
        assert_eq!(g(-0.0, 6), "-0");
        assert_eq!(g(0.1, 25), "0.1000000000000000055511151");
        assert_eq!(g(1e20, 25), "100000000000000000000");
        assert_eq!(g(1.5e300, 6), "1.5e+300");
        assert_eq!(g(123456789.0, 6), "1.23457e+08");
        assert_eq!(g(3.25, 6), "3.25");
        assert_eq!(g(-34379.0438, 25), "-34379.04379999999946448952");
        assert_eq!(g(f64::INFINITY, 6), "inf");
        assert_eq!(g(f64::NEG_INFINITY, 6), "-inf");
    }

    #[test]
    fn printf_f_e() {
        let mut o = Vec::new();
        fmt_f(-34379.0438, 2, &mut o);
        assert_eq!(o, b"-34379.04");
        o.clear();
        fmt_e(1234.5, 3, &mut o);
        assert_eq!(o, b"1.234e+03");
        o.clear();
        fmt_e(0.00015, 1, &mut o);
        assert_eq!(o, b"1.5e-04");
    }

    #[test]
    fn dates() {
        let mut o = Vec::new();
        date_format(0.0, &mut o);
        assert_eq!(o, b"1970-01-01T00:00:00Z");
        o.clear();
        date_format(1198833773.10, &mut o);
        assert_eq!(o, b"2007-12-28T09:22:53.10Z");
        assert_eq!(
            date_parse(b"2007-12-28T09:22:53.10Z"),
            Some(1198833773.0 + 0.1)
        );
        assert_eq!(date_parse(b"2006-04-24T16:11:33Z"), Some(1145895093.0));
        assert_eq!(date_parse(b"1969-12-31T23:59:59Z"), None);
        assert_eq!(
            date_parse(b"2006-04-24T16:11:33+01:30"),
            Some(1145895093.0 - 5400.0)
        );
        assert_eq!(
            date_parse(b"2006-04-24T16:11:33-01:30"),
            Some(1145895093.0 + 5400.0)
        );
        assert_eq!(date_parse(b"garbage"), None);
        o.clear();
        date_format(1145895093.0, &mut o);
        assert_eq!(o, b"2006-04-24T16:11:33Z");
        o.clear();
        date_format(-1.5, &mut o);
        assert_eq!(o, b"1969-12-31T23:59:59Z");
    }

    #[test]
    fn base64() {
        let mut o = Vec::new();
        base64_encode(b"hello", &mut o);
        assert_eq!(o, b"aGVsbG8=");
        o.clear();
        base64_decode_apr(b"aGVsbG8=", &mut o);
        assert_eq!(o, b"hello");
        o.clear();
        base64_decode_apr(b"YWJjMzIx", &mut o);
        assert_eq!(o, b"abc321");
        o.clear();
        base64_decode_apr(b"aGVsbG8", &mut o);
        assert_eq!(o, b"hello");
        o.clear();
        base64_decode_apr(b"aGVsbG8=junk", &mut o);
        assert_eq!(o, b"hello");
        o.clear();
        base64_decode_apr(b"a", &mut o);
        assert_eq!(o, b"");
    }

    #[test]
    fn uuids() {
        let u = uuid_set(b"c96f9b1e-f589-4100-9774-d98643ce0bed");
        let mut o = Vec::new();
        uuid_to_hex(&u, &mut o);
        assert_eq!(o, b"c96f9b1e-f589-4100-9774-d98643ce0bed");
        assert_eq!(uuid_set(b"c96f9b1e-f589-4100-9774d98643ce0bed"), u);
        assert_eq!(uuid_set(b"c96f9b1e-f589-4100-9774-d98643ce0beg"), NULL_UUID);
        assert_eq!(uuid_set(b"123"), NULL_UUID);
    }

    #[test]
    fn extraction() {
        let mut src = SliceSource::new(b"  -42abc");
        let mut r = Reader::new(&mut src);
        assert_eq!(extract_int(&mut r), Some(-42));
        assert_eq!(r.peek(), Some(b'a'));
        let mut src = SliceSource::new(b"2147483648");
        let mut r = Reader::new(&mut src);
        assert_eq!(extract_int(&mut r), None);
        let mut src = SliceSource::new(b"0x1.8p1q");
        let mut r = Reader::new(&mut src);
        assert_eq!(extract_real(&mut r), Some(3.0));
        assert_eq!(r.peek(), Some(b'q'));
        assert_eq!(string_as_real(b"0x1p3"), 8.0);
        assert_eq!(string_as_real(b"0x10"), 16.0);
        assert_eq!(string_as_real(b"007.5"), 7.5);
        let mut src = SliceSource::new(b"1.5e3xyz");
        let mut r = Reader::new(&mut src);
        assert_eq!(extract_real(&mut r), Some(1500.0));
        assert_eq!(r.peek(), Some(b'x'));
        assert_eq!(string_as_real(b"42.0"), 42.0);
        assert_eq!(string_as_real(b"42.0 "), 0.0);
        assert_eq!(string_as_real(b" 42.0"), 42.0);
        assert_eq!(string_as_real(b"nan"), 0.0);
        // libstdc++: no hex, inf or nan; strtod must take the whole collected
        // text; overflow stores +/-DBL_MAX.
        assert_eq!(string_as_real_libstdcxx(b"0x10"), 0.0);
        assert_eq!(string_as_real_libstdcxx(b"0"), 0.0);
        assert_eq!(string_as_real_libstdcxx(b"inf"), 0.0);
        assert_eq!(string_as_real_libstdcxx(b"nan"), 0.0);
        assert_eq!(string_as_real_libstdcxx(b"1e"), 0.0);
        assert_eq!(string_as_real_libstdcxx(b"1e+"), 0.0);
        assert_eq!(string_as_real_libstdcxx(b"1e5x"), 0.0);
        assert_eq!(string_as_real_libstdcxx(b"1e999"), f64::MAX);
        assert_eq!(string_as_real_libstdcxx(b"-1e999"), f64::MIN);
        assert_eq!(string_as_real_libstdcxx(b"1e-999"), 0.0);
        assert_eq!(string_as_real_libstdcxx(b"5."), 5.0);
        assert_eq!(string_as_real_libstdcxx(b".5"), 0.5);
        assert_eq!(string_as_real_libstdcxx(b"1.e5"), 1e5);
        assert_eq!(string_as_real_libstdcxx(b"."), 0.0);
        assert_eq!(string_as_real_libstdcxx(b" 5"), 5.0);
        assert_eq!(string_as_real_libstdcxx(b"5 "), 0.0);
        assert_eq!(string_as_real_libstdcxx(b"1.2.3"), 0.0);
        assert_eq!(string_as_real_libstdcxx(b"007.5"), 7.5);
        assert_eq!(string_as_real_libstdcxx(b"-2.5E-3"), -0.0025);
        assert_eq!(string_as_real_msvc(b"0x10"), 16.0);
        assert_eq!(sscanf_int(b" 12abc"), Some(12));
        assert_eq!(sscanf_int(b"abc"), None);
        assert_eq!(sscanf_int(b"2147483648"), Some(-2147483648));
        assert_eq!(sscanf_int(b"4294967296"), Some(0));
        assert_eq!(sscanf_int(b"8589934593"), Some(1));
        assert_eq!(sscanf_int(b"99999999999999999999"), Some(-1));
        assert_eq!(sscanf_int(b"-18446744073709551616"), Some(0));
        assert_eq!(sscanf_int(b"-2147483649"), Some(2147483647));
        assert_eq!(string_as_real(b"1e999"), f64::INFINITY);
        assert_eq!(string_as_real(b"-1e999"), f64::NEG_INFINITY);
        assert_eq!(string_as_real(b"1e-999"), 0.0);
        let mut src = SliceSource::new(b"1e-999");
        let mut r = Reader::new(&mut src);
        assert_eq!(extract_real(&mut r), Some(0.0));
        let mut src = SliceSource::new(b"5e-324");
        let mut r = Reader::new(&mut src);
        assert_eq!(extract_real(&mut r), Some(5e-324));
        assert_eq!(string_as_real(b"2.5e"), 0.0);
        assert_eq!(strtol0(b"0x10"), 16);
        assert_eq!(strtol0(b"010"), 8);
        assert_eq!(strtol0(b"8)"), 8);
    }
}
