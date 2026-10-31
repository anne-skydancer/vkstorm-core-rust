//! Header text handling: the normalisation the C++ header callback applied
//! to every received header line (`HttpHeaders::appendNormal` rules), the
//! `Content-Range` and `Retry-After` parsers, the `Range:` request line and
//! the `name: value` form of caller headers.

/// A received header line split as the C++ did: the name is everything
/// before the first colon, ASCII lower-cased and trimmed of spaces and tabs
/// on both sides; the value is everything after it, trimmed on the left.
/// Without a colon the whole line is the name, left-trimmed, and there is
/// no value. The trailing CR/LF of the line are removed first. Returns
/// `None` for a line whose normalised name is empty (the C++ ignored it).
pub fn split_header(line: &[u8]) -> Option<(String, Option<String>)> {
    let mut end = line.len();
    if end > 0 && line[end - 1] == b'\n' {
        end -= 1;
        if end > 0 && line[end - 1] == b'\r' {
            end -= 1;
        }
    }
    let line = &line[..end];
    // The C++ worked on a NUL-terminated copy, so a NUL in the line ended it.
    let line = match line.iter().position(|&b| b == 0) {
        Some(p) => &line[..p],
        None => line,
    };
    let (name, value) = match line.iter().position(|&b| b == b':') {
        Some(p) => {
            let name = trim_both(&line[..p]).to_ascii_lowercase();
            let value = trim_left(&line[p + 1..]);
            (name, Some(latin1(value)))
        }
        None => (trim_left(line).to_vec(), None),
    };
    if name.is_empty() {
        return None;
    }
    Some((latin1(&name), value))
}

fn is_blank(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

fn trim_left(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&b| !is_blank(b)).unwrap_or(s.len());
    &s[start..]
}

fn trim_both(s: &[u8]) -> &[u8] {
    let s = trim_left(s);
    let end = s.iter().rposition(|&b| !is_blank(b)).map_or(0, |p| p + 1);
    &s[..end]
}

/// Bytes to a string one char per byte, as the C++ `std::string` carried
/// them (header text is ASCII in practice).
fn latin1(s: &[u8]) -> String {
    s.iter().map(|&b| b as char).collect()
}

/// The result of parsing a `Content-Range` value.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ContentRange {
    /// `bytes first-last/length` (length 0 when the server sent `*`).
    Valid { first: u32, last: u32, length: u32 },
    /// Parsed but inconsistent (`first > last` or `last >= length`): the
    /// response is to be rejected.
    Invalid,
    /// Not in a form the C++ understood: ignored.
    Unparsed,
}

/// `parse_content_range_header`: tokenises on spaces and tabs, wants a
/// case-insensitive `bytes` unit then `%u-%u/%u` or `%u-%u/*` read with
/// `sscanf` semantics (optional sign, leading whitespace, trailing text
/// ignored).
pub fn parse_content_range(value: &str) -> ContentRange {
    let mut tokens = value.split([' ', '\t']).filter(|t| !t.is_empty());
    let unit = match tokens.next() {
        Some(u) => u,
        None => return ContentRange::Unparsed,
    };
    if !unit.eq_ignore_ascii_case("bytes") {
        return ContentRange::Unparsed;
    }
    let spec = match tokens.next() {
        Some(s) => s,
        None => return ContentRange::Unparsed,
    };
    let mut rest = spec;
    let first = match scan_u32(&mut rest) {
        Some(v) => v,
        None => return ContentRange::Unparsed,
    };
    if !rest.starts_with('-') {
        return ContentRange::Unparsed;
    }
    rest = &rest[1..];
    let last = match scan_u32(&mut rest) {
        Some(v) => v,
        None => return ContentRange::Unparsed,
    };
    if !rest.starts_with('/') {
        return ContentRange::Unparsed;
    }
    rest = &rest[1..];
    if let Some(length) = scan_u32(&mut rest) {
        if first > last || last >= length {
            return ContentRange::Invalid;
        }
        return ContentRange::Valid {
            first,
            last,
            length,
        };
    }
    if rest.starts_with('*') {
        if first > last {
            return ContentRange::Invalid;
        }
        return ContentRange::Valid {
            first,
            last,
            length: 0,
        };
    }
    ContentRange::Unparsed
}

/// `sscanf("%u")`: skips leading whitespace, accepts an optional sign and
/// digits, wraps as `strtoul` does; `None` without digits.
fn scan_u32(s: &mut &str) -> Option<u32> {
    let t = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (neg, t) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let mut v: u64 = 0;
    for b in t[..digits].bytes() {
        v = v.wrapping_mul(10).wrapping_add((b - b'0') as u64);
        v &= 0xFFFF_FFFF;
    }
    let v = v as u32;
    *s = &t[digits..];
    Some(if neg { v.wrapping_neg() } else { v })
}

/// `parse_retry_after_header`: only the delta-seconds form, read with
/// `strtol` and accepted when it consumed the whole value and is positive.
pub fn parse_retry_after(value: &str) -> Option<i32> {
    let t = value.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (neg, t) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits != t.len() {
        return None;
    }
    let mut v: i64 = 0;
    for b in t[..digits].bytes() {
        v = v.saturating_mul(10).saturating_add((b - b'0') as i64);
    }
    let v = if neg { -v } else { v };
    if v > 0 {
        Some(v.min(i32::MAX as i64) as i32)
    } else {
        None
    }
}

/// The `Range:` request header of a byte-range request, as the C++ printed
/// it with `%lu` formats (`bytes=first-last` or `bytes=first-`), cut to its
/// 63-character buffer.
pub fn range_line(offset: u64, length: u64) -> String {
    let mut line = if length != 0 {
        format!(
            "Range: bytes={}-{}",
            offset,
            offset.wrapping_add(length).wrapping_sub(1)
        )
    } else {
        format!("Range: bytes={offset}-")
    };
    line.truncate(63);
    line
}

/// `name: value` for a caller-supplied header.
pub fn header_line(name: &str, value: &str) -> String {
    let mut s = String::with_capacity(name.len() + value.len() + 2);
    s.push_str(name);
    s.push_str(": ");
    s.push_str(value);
    s
}

/// The C++ `debugCallback` escaping of libcurl debug data: non-printing and
/// non-ASCII bytes become spaces (`scrub`) or `%XX`; at most 200 bytes.
pub fn escape_debug(data: &[u8], scrub: bool) -> String {
    let mut out = String::with_capacity(3 * data.len().min(200));
    for &b in data.iter().take(200) {
        if !(32..=126).contains(&b) {
            if scrub {
                out.push(' ');
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(b as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_normalises() {
        assert_eq!(
            split_header(b"Content-Type: text/plain\r\n"),
            Some(("content-type".into(), Some("text/plain".into())))
        );
        assert_eq!(
            split_header(b"  X-Odd \t:\t  v a l  \r\n"),
            Some(("x-odd".into(), Some("v a l  ".into())))
        );
        assert_eq!(
            split_header(b"HTTP/1.1 200 OK\r\n"),
            Some(("HTTP/1.1 200 OK".into(), None))
        );
        assert_eq!(split_header(b"\r\n"), None);
        assert_eq!(split_header(b"   : v"), None);
        assert_eq!(
            split_header(b"Empty:"),
            Some(("empty".into(), Some(String::new())))
        );
    }

    #[test]
    fn content_range_forms() {
        assert_eq!(
            parse_content_range("bytes 0-75/2983"),
            ContentRange::Valid {
                first: 0,
                last: 75,
                length: 2983
            }
        );
        assert_eq!(
            parse_content_range("BYTES  10-19/*"),
            ContentRange::Valid {
                first: 10,
                last: 19,
                length: 0
            }
        );
        assert_eq!(parse_content_range("bytes 10-5/100"), ContentRange::Invalid);
        assert_eq!(
            parse_content_range("bytes 0-100/100"),
            ContentRange::Invalid
        );
        assert_eq!(parse_content_range("bytes 7-5/*"), ContentRange::Invalid);
        assert_eq!(parse_content_range("bytes */100"), ContentRange::Unparsed);
        assert_eq!(parse_content_range("items 0-1/2"), ContentRange::Unparsed);
        assert_eq!(parse_content_range(""), ContentRange::Unparsed);
        assert_eq!(
            parse_content_range("bytes 0-75/2983trailing"),
            ContentRange::Valid {
                first: 0,
                last: 75,
                length: 2983
            }
        );
        assert_eq!(parse_content_range("bytes 0-75"), ContentRange::Unparsed);
    }

    #[test]
    fn retry_after_forms() {
        assert_eq!(parse_retry_after("2"), Some(2));
        assert_eq!(parse_retry_after(" 2"), Some(2));
        assert_eq!(parse_retry_after("+7"), Some(7));
        assert_eq!(parse_retry_after("0"), None);
        assert_eq!(parse_retry_after("-3"), None);
        assert_eq!(parse_retry_after("2 3"), None);
        assert_eq!(parse_retry_after("Thu, 31 Dec 2043 23:59:59 GMT"), None);
        assert_eq!(parse_retry_after(""), None);
    }

    #[test]
    fn range_lines() {
        assert_eq!(range_line(0, 47), "Range: bytes=0-46");
        assert_eq!(range_line(500, 0), "Range: bytes=500-");
        assert_eq!(range_line(0, 0), "Range: bytes=0-");
    }

    #[test]
    fn debug_escaping() {
        assert_eq!(escape_debug(b"ab\r\nc", true), "ab  c");
        assert_eq!(escape_debug(b"ab\r\nc", false), "ab%0D%0Ac");
    }
}
