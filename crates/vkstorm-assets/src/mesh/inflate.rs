//! `LLUZipHelper::unzip_llsd`: a zlib-compressed binary LLSD block is
//! inflated, its deprecated `<? LLSD/Binary ?>` header stripped if present,
//! and the binary LLSD parsed with the viewer's depth limit.

use vkstorm_llsd::Value;
use zlib_rs::{Inflate, InflateError, InflateFlush, Status};

use super::llsd::parse_binary;

/// `LLUZipHelper::EZipRresult`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ZipResult {
    Ok = 0,
    MemError = 1,
    BufferError = 2,
    VersionError = 3,
    DataError = 4,
    ParseError = 5,
}

/// `UNZIP_LLSD_MAX_DEPTH`.
pub const UNZIP_LLSD_MAX_DEPTH: i32 = 96;
/// The inflate output chunk of `unzip_llsd` (`CHUNK = 1024 * 512`).
const CHUNK: usize = 1024 * 512;

const DEPRECATED_HEADER: &[u8] = b"<? LLSD/Binary ?>";

/// `strip_deprecated_header`: the slice after the legacy header when
/// present, and the header size the C++ reported (one more than the header
/// is long, as it counted).
pub fn strip_deprecated_header(data: &[u8]) -> (&[u8], usize) {
    if data.len() > DEPRECATED_HEADER.len() && &data[..DEPRECATED_HEADER.len()] == DEPRECATED_HEADER {
        (&data[DEPRECATED_HEADER.len()..], DEPRECATED_HEADER.len() + 1)
    } else {
        (data, 0)
    }
}

/// Inflates a zlib stream completely with the C++ loop's result mapping: a
/// corrupt stream or a dictionary request is `DataError`, a stream that
/// ends before its end marker `BufferError` (zlib's `Z_BUF_ERROR` when the
/// input runs out), and a stream that stops being `Z_OK` without reaching
/// the end `DataError`.
pub fn inflate_all(input: &[u8]) -> Result<Vec<u8>, ZipResult> {
    let mut inflate = Inflate::new(true, 15);
    let mut result: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; CHUNK];
    let mut consumed = 0usize;
    loop {
        let before_in = inflate.total_in();
        let before_out = inflate.total_out();
        let status = inflate.decompress(&input[consumed..], &mut chunk, InflateFlush::NoFlush);
        consumed += (inflate.total_in() - before_in) as usize;
        let have = (inflate.total_out() - before_out) as usize;
        match status {
            Err(InflateError::NeedDict { .. }) | Err(InflateError::DataError) => return Err(ZipResult::DataError),
            Err(InflateError::StreamError) => return Err(ZipResult::BufferError),
            Ok(Status::BufError) => return Err(ZipResult::BufferError),
            Err(InflateError::MemError) => return Err(ZipResult::MemError),
            Ok(Status::Ok) => {
                result.extend_from_slice(&chunk[..have]);
            }
            Ok(Status::StreamEnd) => {
                result.extend_from_slice(&chunk[..have]);
                return Ok(result);
            }
        }
    }
}

/// `unzip_llsd`: inflate, strip the deprecated header, parse.
pub fn unzip_llsd(input: &[u8]) -> Result<Value, ZipResult> {
    let inflated = inflate_all(input)?;
    let (body, _) = strip_deprecated_header(&inflated);
    match parse_binary(body, body.len() as i64, UNZIP_LLSD_MAX_DEPTH) {
        Some((v, _)) => Ok(v),
        None => Err(ZipResult::ParseError),
    }
}

/// `zip_llsd`'s deflate for the tests and the corpus tools: the bytes
/// deflated at the best compression level with a zlib header, as the
/// viewer's packer did. Byte equality with zlib-ng's output is neither
/// claimed nor needed: the viewer only inflates.
pub fn zip_bytes(bytes: &[u8]) -> Vec<u8> {
    use zlib_rs::{Deflate, DeflateFlush};
    let mut deflate = Deflate::new(9, true, 15);
    let mut out: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 65536];
    let mut consumed = 0usize;
    loop {
        let before_in = deflate.total_in();
        let before_out = deflate.total_out();
        let status = deflate.compress(&bytes[consumed..], &mut chunk, DeflateFlush::Finish).expect("deflate");
        consumed += (deflate.total_in() - before_in) as usize;
        let have = (deflate.total_out() - before_out) as usize;
        out.extend_from_slice(&chunk[..have]);
        if status == Status::StreamEnd {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_errors() {
        let value = Value::Map([(b"a".to_vec(), Value::Integer(5)), (b"b".to_vec(), Value::Binary(vec![1, 2, 3]))].into_iter().collect());
        let (bytes, _) = vkstorm_llsd::format(vkstorm_llsd::Format::Binary, &value, Default::default());
        let zipped = zip_bytes(&bytes);
        let back = unzip_llsd(&zipped).unwrap();
        assert_eq!(back, value);
        // Truncated input: the stream runs out.
        let cut = &zipped[..zipped.len() / 2];
        assert_eq!(unzip_llsd(cut), Err(ZipResult::BufferError));
        // Garbage.
        assert_eq!(unzip_llsd(b"not zlib at all"), Err(ZipResult::DataError));
        // Legacy header.
        let mut with_header = DEPRECATED_HEADER.to_vec();
        with_header.extend_from_slice(&bytes);
        assert_eq!(unzip_llsd(&zip_bytes(&with_header)).unwrap(), value);
        // Valid zlib, not LLSD.
        assert_eq!(unzip_llsd(&zip_bytes(b"\xffnot llsd")), Err(ZipResult::ParseError));
    }

    #[test]
    fn large_output_grows() {
        let big: Vec<u8> = (0..2_000_000u32).map(|i| (i % 7) as u8).collect();
        let zipped = zip_bytes(&big);
        assert!(zipped.len() < 50_000);
        assert_eq!(inflate_all(&zipped).unwrap(), big);
    }
}
