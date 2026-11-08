// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026, Anne Skydancer (Vulkanstorm)
//! Reply decoding as the C++ llcorehttp does it (vulkanstorm #338 and #343,
//! `_httplibcurl.cpp` `inflate_reply`/`decode_reply`), which is how libcurl
//! 7.54.1 decoded (`lib/http.c`, `lib/content_encoding.c`) less two of its
//! defects (output lost when it was still pending in zlib at the end of the
//! input, and gzip members after the first dropped):
//! - the Content-Encoding value is matched by a case-insensitive prefix
//!   after leading white space, so only a list's first coding counts:
//!   `identity`, `deflate`, `gzip`, `x-gzip`; anything else (the asset
//!   CDN's `binary/octet-stream`) passes through;
//! - gzip detects a gzip or zlib wrapper per member (zlib's
//!   `MAX_WBITS + 32`), deflate is zlib; for either, data that does not
//!   start a valid stream is retried as raw deflate. Only a format error
//!   before any output counts (the first decompress call of the reply fails
//!   having produced nothing), as libcurl 7.54.1 restarted only before
//!   inflate() had returned Z_OK; a stream that fails later (corrupt data, a
//!   bad CRC-32 or Adler-32) is refused, never raw-decoded (vulkanstorm #354
//!   review);
//! - every member of a concatenation is decoded, bytes after a complete
//!   stream that do not start another member are ignored, a truncated
//!   stream keeps what decoded; data that does not decode is refused (the
//!   caller keeps the original body).

use flate2::{Decompress, FlushDecompress, Status};

/// What became of a reply body.
#[derive(Debug, PartialEq, Eq)]
pub enum Decoded {
    /// Not an encoding llcorehttp undoes: the body is used as it came.
    Passthrough,
    /// The decoded body.
    Body(Vec<u8>),
    /// gzip/deflate data that does not decode (`CURLE_BAD_CONTENT_ENCODING`).
    Refused,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Wrapper {
    /// gzip or zlib, detected per member as zlib's `MAX_WBITS + 32` does.
    Auto,
    Zlib,
    Raw,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Coding {
    None,
    Gzip,
    Deflate,
}

/// libcurl 7.54.1's matching: white space skipped, then a case-insensitive
/// prefix (its `checkprefix`), in its order.
fn coding(value: &str) -> Coding {
    let v = value
        .trim_start_matches(|c: char| c.is_ascii_whitespace())
        .as_bytes();
    let prefix = |p: &str| v.len() >= p.len() && v[..p.len()].eq_ignore_ascii_case(p.as_bytes());
    if prefix("identity") {
        Coding::None
    } else if prefix("deflate") {
        Coding::Deflate
    } else if prefix("gzip") || prefix("x-gzip") {
        Coding::Gzip
    } else {
        Coding::None
    }
}

/// `decode_reply()`: `encoding` is the reply's (last) Content-Encoding value.
pub fn decode_reply(encoding: &str, body: &[u8]) -> Decoded {
    let coding = coding(encoding);
    if coding == Coding::None {
        return Decoded::Passthrough;
    }
    if body.is_empty() {
        return Decoded::Body(Vec::new());
    }
    let wrapper = if coding == Coding::Gzip {
        Wrapper::Auto
    } else {
        Wrapper::Zlib
    };
    match inflate_reply(body, wrapper) {
        Ok(out) => Decoded::Body(out),
        Err(Failure::NotAStream) => {
            inflate_reply(body, Wrapper::Raw).map_or(Decoded::Refused, Decoded::Body)
        }
        Err(Failure::Bad) => Decoded::Refused,
    }
}

/// Why a stream did not decode.
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    /// The first decompress call failed with no output: not a zlib or gzip
    /// stream (the raw-deflate fallback's case).
    NotAStream,
    /// Any later failure.
    Bad,
}

fn new_member(rest: &[u8], wrapper: Wrapper) -> Member {
    match wrapper {
        Wrapper::Raw => Member::Flate(Decompress::new(false)),
        Wrapper::Zlib => Member::Flate(Decompress::new(true)),
        Wrapper::Auto if rest.starts_with(&[0x1f, 0x8b]) => gzip_member(),
        Wrapper::Auto => Member::Flate(Decompress::new(true)),
    }
}

enum Member {
    Flate(Decompress),
}

fn gzip_member() -> Member {
    Member::Flate(Decompress::new_gzip(15))
}

/// Decodes one member from `input` into `out`. Returns the bytes consumed,
/// whether the member ended (`Some(true)`), ran out of input
/// (`Some(false)`), or is bad data (`None`), and whether bad data failed on
/// the first decompress call with no output.
fn run_member(member: Member, input: &[u8], out: &mut Vec<u8>) -> (usize, Option<bool>, bool) {
    match member {
        Member::Flate(mut d) => {
            let mut used = 0usize;
            let mut first = true;
            loop {
                // grow only when full: a size taken from the gzip trailer is exact
                if out.capacity() == out.len() {
                    out.reserve(out.len().max(16384));
                }
                let (in0, out0) = (d.total_in(), d.total_out());
                let status = d.decompress_vec(&input[used..], out, FlushDecompress::None);
                let consumed = (d.total_in() - in0) as usize;
                let produced = (d.total_out() - out0) as usize;
                used += consumed;
                let at_start = first && produced == 0;
                first = false;
                match status {
                    // A preset-dictionary request has a valid zlib header: it is
                    // refused, never raw-decoded (zlib's Z_NEED_DICT, which curl
                    // 7.54.1 did not fall back on either).
                    Err(e) => return (used, None, at_start && e.needs_dictionary().is_none()),
                    Ok(Status::StreamEnd) => return (used, Some(true), false),
                    Ok(Status::Ok) | Ok(Status::BufError) => {
                        if used == input.len() && produced == 0 {
                            return (used, Some(false), false); // all input used: truncated keeps what decoded
                        }
                        if consumed == 0 && produced == 0 {
                            return (used, None, false); // no progress with input left
                        }
                    }
                }
            }
        }
    }
}

/// The decoded size to reserve: a gzip stream's trailer holds it (ISIZE,
/// modulo 2^32; exact for one member, a lower bound for several) when it is
/// at most 64 times the input; otherwise twelve times the input (LLSD XML
/// deflates about tenfold), the vector growing from there. Capped at 64 MiB.
fn size_hint(body: &[u8], wrapper: Wrapper) -> usize {
    const CAP: usize = 64 << 20;
    let fallback = body.len().saturating_mul(12).min(CAP);
    if wrapper == Wrapper::Auto && body.len() >= 18 && body.starts_with(&[0x1f, 0x8b]) {
        let n = body.len();
        let isize =
            u32::from_le_bytes([body[n - 4], body[n - 3], body[n - 2], body[n - 1]]) as usize;
        // The last four bytes may be trailing bytes, not a trailer: a size
        // beyond 64 times the input (well above what LLSD and XML deflate
        // to) is not taken from them, so a short reply cannot reserve a
        // large buffer.
        if isize >= body.len() / 2 && isize <= body.len().saturating_mul(64).min(CAP) {
            return isize;
        }
    }
    fallback
}

/// `inflate_reply()`: an error when the data does not decode.
fn inflate_reply(body: &[u8], wrapper: Wrapper) -> Result<Vec<u8>, Failure> {
    let mut out = Vec::with_capacity(size_hint(body, wrapper));
    let mut pos = 0usize;
    let mut next_member = false;
    loop {
        let rest = &body[pos..];
        let start = out.len();
        let (used, end, at_start) = run_member(new_member(rest, wrapper), rest, &mut out);
        match end {
            None if next_member && out.len() == start => {
                out.truncate(start);
                return Ok(out); // trailing bytes that are not another member: ignored
            }
            None if !next_member && at_start => return Err(Failure::NotAStream),
            None => return Err(Failure::Bad),
            Some(false) => return Ok(out),
            Some(true) => {
                pos += used;
                if pos >= body.len() {
                    return Ok(out);
                }
                next_member = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};
    use std::io::Write;

    fn text() -> Vec<u8> {
        b"Vulkanstorm content encoding test body. ".repeat(40)
    }
    fn gz(data: &[u8]) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), Compression::best());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }
    fn zl(data: &[u8]) -> Vec<u8> {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }
    fn raw(data: &[u8]) -> Vec<u8> {
        let mut e = DeflateEncoder::new(Vec::new(), Compression::best());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn decodes_what_the_cpp_decodes() {
        let t = text();
        let half = t.len() / 2;
        for (name, enc, body) in [
            ("gzip", "gzip", gz(&t)),
            ("x-gzip", "x-gzip", gz(&t)),
            ("padded upper case", " GZIP\t", gz(&t)),
            ("gzip label, zlib wrapper", "gzip", zl(&t)),
            (
                "two gzip members",
                "gzip",
                [gz(&t[..half]), gz(&t[half..])].concat(),
            ),
            ("trailing padding", "gzip", [gz(&t), vec![0u8; 4]].concat()),
            ("deflate (zlib)", "deflate", zl(&t)),
            ("deflate (raw)", "deflate", raw(&t)),
            ("raw deflate labelled gzip", "gzip", raw(&t)),
            ("a coding list: the first counts", "gzip, identity", gz(&t)),
            ("a prefix, as checkprefix", "gzipped", gz(&t)),
        ] {
            assert_eq!(decode_reply(enc, &body), Decoded::Body(t.clone()), "{name}");
        }
    }

    #[test]
    fn drains_output_pending_at_the_end_of_the_input() {
        let a = vec![b'A'; 16400];
        assert_eq!(decode_reply("deflate", &raw(&a)), Decoded::Body(a.clone()));
        let b = vec![b'A'; 70000];
        assert_eq!(decode_reply("gzip", &gz(&b)), Decoded::Body(b));
    }

    #[test]
    fn trailing_bytes_do_not_inflate_the_reservation() {
        // a one-byte gzip body followed by bytes that read as ISIZE 64 MiB
        let mut body = gz(b"x");
        body.extend_from_slice(&(64u32 << 20).to_le_bytes());
        assert!(size_hint(&body, Wrapper::Auto) <= body.len() * 64);
        assert_eq!(decode_reply("gzip", &body), Decoded::Body(b"x".to_vec()));
    }

    #[test]
    fn passes_other_encodings_through() {
        for enc in [
            "",
            "identity",
            "identity, gzip",
            "binary/octet-stream",
            "br",
            "compress",
        ] {
            assert_eq!(decode_reply(enc, &text()), Decoded::Passthrough, "{enc:?}");
        }
    }

    #[test]
    fn refuses_bad_data_and_keeps_a_truncated_stream() {
        assert_eq!(
            decode_reply("gzip", b"this is not gzip data at all"),
            Decoded::Refused
        );
        assert_eq!(
            decode_reply("deflate", b"this is not deflate data at all"),
            Decoded::Refused
        );
        assert_eq!(decode_reply("gzip", b""), Decoded::Body(Vec::new()));
        // A truncated stream keeps what decoded, as libcurl 7.54.1 did.
        let long = text().repeat(50);
        let cut = gz(&long);
        let Decoded::Body(part) = decode_reply("gzip", &cut[..cut.len() / 2]) else {
            panic!("a truncated stream is not refused");
        };
        assert!(!part.is_empty() && long.starts_with(&part));
    }

    /// A zlib stream (one 65,534-byte stored block) whose bytes, read as raw
    /// deflate, are also a complete stream: the two bytes 01 58.
    fn zlib_twin(break_adler: bool) -> (Vec<u8>, Vec<u8>) {
        let mut text = b"\x01\x00\xfe\xffX\x01\x00\x00\xff\xff".to_vec();
        text.resize(65534, b'A');
        let (mut s1, mut s2) = (1u32, 0u32);
        for &b in &text {
            s1 = (s1 + u32::from(b)) % 65521;
            s2 = (s2 + s1) % 65521;
        }
        let adler = ((s2 << 16) | s1) ^ u32::from(break_adler);
        let mut body = vec![0x78, 0x01, 0x00, 0xfe, 0xff, 0x01, 0x00];
        body.extend_from_slice(&text);
        body.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
        body.extend_from_slice(&adler.to_be_bytes());
        (body, text)
    }

    #[test]
    fn falls_back_to_raw_deflate_only_at_the_start() {
        let t = text();
        // a failure after the header is refused, not raw-decoded
        let (bad, _) = zlib_twin(true);
        assert_eq!(
            decode_reply("deflate", &bad),
            Decoded::Refused,
            "corrupt twin"
        );
        let mut z = zl(&t);
        *z.last_mut().unwrap() ^= 1;
        assert_eq!(
            decode_reply("deflate", &z),
            Decoded::Refused,
            "bad Adler-32"
        );
        let mut g = gz(&t);
        let n = g.len();
        g[n - 8] ^= 1;
        assert_eq!(decode_reply("gzip", &g), Decoded::Refused, "bad CRC-32");
        // a valid header asking for a preset dictionary (FDICT) is refused: its
        // data also reads as a 32-byte raw stored block, which must not be taken
        let mut dict = vec![0x78, 0x20, 0x00, 0xdf, 0xff];
        dict.extend_from_slice(&[b'A'; 32]);
        dict.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
        assert_eq!(
            decode_reply("deflate", &dict),
            Decoded::Refused,
            "preset dictionary"
        );
        // a valid zlib stream decodes as zlib; raw deflate still decodes
        let (good, text65534) = zlib_twin(false);
        assert_eq!(
            decode_reply("deflate", &good),
            Decoded::Body(text65534),
            "valid twin"
        );
        assert_eq!(
            decode_reply("deflate", &raw(&t)),
            Decoded::Body(t.clone()),
            "raw deflate"
        );
        assert_eq!(
            decode_reply("gzip", &raw(&t)),
            Decoded::Body(t.clone()),
            "raw labelled gzip"
        );
        assert_eq!(decode_reply("deflate", &zl(&t)), Decoded::Body(t), "zlib");
    }
}
