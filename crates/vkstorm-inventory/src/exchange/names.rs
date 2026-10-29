//! Pure string helpers the offer and give code calls: `LLCacheName`'s name
//! helpers (`llcachename.cpp`), `LLMuteList::isLinden` (`llmutelist.cpp`),
//! `LLURI::escape` (`lluri.cpp`), `LLUUID::validate` and `parseUUID`
//! (`lluuid.cpp`), and the text readers the bucket parsers use (`atoi`,
//! `istream >> S32`, boost's `char_separator` with kept empty tokens).

use crate::perms::Uuid;
use crate::text::{is_space, trim};

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `std::string::find` from position 0.
pub fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    find(hay, needle)
}

/// `std::string::rfind`.
pub fn rfind_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).rev().find(|&i| &hay[i..i + needle.len()] == needle)
}

/// `LLCacheName::cleanFullName`: with `LLAvatarName::trimResidentSurname()`
/// the name up to the first `" Resident"`, else the name.
pub fn clean_full_name(full_name: &[u8], trim_resident: bool) -> Vec<u8> {
    if !trim_resident {
        return full_name.to_vec();
    }
    match find(full_name, b" Resident") {
        Some(i) => full_name[..i].to_vec(),
        None => full_name.to_vec(),
    }
}

/// `LLCacheName::buildUsername`.
pub fn build_username(full_name: &[u8], trim_resident: bool) -> Vec<u8> {
    if full_name == b"(???) (???)" {
        return b"(???)".to_vec();
    }
    if let Some(i) = full_name.iter().position(|&c| c == b' ') {
        let mut username = full_name[..i].to_vec();
        let last = &full_name[i + 1..];
        if last != b"Resident" {
            username.push(b'.');
            username.extend_from_slice(last);
        }
        username.make_ascii_lowercase();
        return username;
    }
    let mut clean = clean_full_name(full_name, trim_resident);
    clean.make_ascii_lowercase();
    clean
}

/// `LLCacheName::buildLegacyName`: `"Display (first.last)"` to `"First
/// Last"`; any other name unchanged.
pub fn build_legacy_name(complete: &[u8]) -> Vec<u8> {
    let open = rfind_bytes(complete, b" (");
    let close = complete.iter().rposition(|&c| c == b')');
    if let (Some(open), Some(close)) = (open, close) {
        if close == complete.len() - 1 {
            let start = open + 2;
            let length = close.wrapping_sub(open).wrapping_sub(2);
            let end = if start > complete.len() { complete.len() } else { start.saturating_add(length).min(complete.len()) };
            let legacy = if start > complete.len() { &complete[0..0] } else { &complete[start..end] };
            if !legacy.is_empty() {
                let mut out = Vec::new();
                out.push(legacy[0].to_ascii_uppercase());
                out.extend_from_slice(&legacy[1..]);
                if let Some(sep) = out.iter().position(|&c| c == b'.') {
                    let last = out[sep + 1..].to_vec();
                    out.truncate(sep);
                    if !last.is_empty() {
                        out.push(b' ');
                        out.push(last[0].to_ascii_uppercase());
                        out.extend_from_slice(&last[1..]);
                    }
                }
                return out;
            }
        }
    }
    complete.to_vec()
}

/// `LLMuteList::isLinden`: the second space-separated token (dots read as
/// spaces), lower-cased, is `linden`.
pub fn is_linden(name: &[u8]) -> bool {
    let replaced: Vec<u8> = name.iter().map(|&c| if c == b'.' { b' ' } else { c }).collect();
    let mut tokens = replaced.split(|&c| c == b' ').filter(|t| !t.is_empty());
    if tokens.next().is_none() {
        return false;
    }
    match tokens.next() {
        Some(t) => t.to_ascii_lowercase() == b"linden",
        None => false,
    }
}

/// `LLURI::escape(str)`: the unreserved characters (`A-Z a-z 0-9 - . _ ~`)
/// kept, every other byte as `%XX` (upper-case hex).
pub fn uri_escape(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for &c in s {
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'_' | b'~') {
            out.push(c);
        } else {
            out.extend_from_slice(format!("%{c:02X}").as_bytes());
        }
    }
    out
}

/// `LLUUID::validate`: 36 bytes in the dashed layout, or 35 with the last
/// dash missing; hex digits of either case.
pub fn uuid_validate(s: &[u8]) -> bool {
    let broken = match s.len() {
        36 => false,
        35 => true,
        _ => return false,
    };
    let hex = |c: u8| c.is_ascii_hexdigit();
    let mut pos = 0usize;
    for i in 0..16 {
        if i == 4 || i == 6 || i == 8 || i == 10 {
            pos += 1;
            if broken && i == 10 {
                pos -= 1;
            }
        }
        if !hex(s[pos]) {
            return false;
        }
        pos += 1;
        if !hex(s[pos]) {
            return false;
        }
        pos += 1;
    }
    true
}

/// `LLUUID::parseUUID(buf, &id)`: the trimmed text if it validates.
pub fn parse_uuid(buf: &[u8]) -> Option<Uuid> {
    if buf.is_empty() {
        return None;
    }
    let t = trim(buf);
    uuid_validate(t).then(|| vkstorm_llsd::text::uuid_set(t))
}

/// `atoi` on a C string: leading white space, a sign, decimal digits,
/// saturating at the 32-bit range (MSVC and glibc then agree on the low
/// byte the callers keep).
pub fn atoi(s: &[u8]) -> i32 {
    let s = &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())];
    let mut i = 0;
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    let mut neg = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        neg = s[i] == b'-';
        i += 1;
    }
    let mut v: i64 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        v = (v * 10 + (s[i] - b'0') as i64).min(1 << 40);
        i += 1;
    }
    let v = if neg { -v } else { v };
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// `std::stringstream(s) >> S32`: `None` when the stream is at its end
/// after the leading white space (the sentry fails and the variable keeps
/// its value); else the value, 0 when no digits follow, saturated on
/// overflow.
pub fn stream_read_s32(s: &[u8]) -> Option<i32> {
    let mut i = 0;
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    if i == s.len() {
        return None;
    }
    let mut neg = false;
    if s[i] == b'+' || s[i] == b'-' {
        neg = s[i] == b'-';
        i += 1;
    }
    let start = i;
    let mut v: i64 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        v = (v * 10 + (s[i] - b'0') as i64).min(1 << 40);
        i += 1;
    }
    if i == start {
        return Some(0);
    }
    let v = if neg { -v } else { v };
    Some(v.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
}

/// boost `tokenizer<char_separator<char>>` with separator `|` and
/// `keep_empty_tokens`: every `|` separates. `token(i)` past the end is the
/// last token (empty for an empty string): the exhausted iterator keeps the
/// token it held, as the separator never clears a string token.
pub struct PipeTokens<'a> {
    tokens: Vec<&'a [u8]>,
}

impl<'a> PipeTokens<'a> {
    pub fn new(s: &'a [u8]) -> PipeTokens<'a> {
        PipeTokens { tokens: s.split(|&c| c == b'|').collect() }
    }

    pub fn token(&self, i: usize) -> &'a [u8] {
        self.tokens.get(i).or(self.tokens.last()).copied().unwrap_or(&[])
    }
}

/// The bytes before the first NUL (`std::string(s.c_str())`).
pub fn c_str(s: &[u8]) -> &[u8] {
    &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert!(is_linden(b"Grid Linden"));
        assert!(is_linden(b"grid.linden"));
        assert!(!is_linden(b"Linden"));
        assert_eq!(clean_full_name(b"Sender Resident", true), b"Sender");
        assert_eq!(build_username(b"Sender Resident", true), b"sender");
        assert_eq!(build_username(b"Other Sender", true), b"other.sender");
        assert_eq!(build_legacy_name(b"Officer"), b"Officer");
        assert_eq!(build_legacy_name(b"Dis (first.last)"), b"First Last");
        assert_eq!(uri_escape(b"a b/c"), b"a%20b%2Fc");
        assert_eq!(stream_read_s32(b" 7"), Some(7));
        assert_eq!(stream_read_s32(b"x"), Some(0));
        assert_eq!(stream_read_s32(b""), None);
        assert_eq!(stream_read_s32(b"99999999999"), Some(i32::MAX));
        assert_eq!(atoi(b"x"), 0);
    }
}
