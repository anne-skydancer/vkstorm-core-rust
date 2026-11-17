//! `LLUZipHelper::unzip_llsd`: a zlib-compressed binary LLSD block is
//! inflated, its deprecated `<? LLSD/Binary ?>` header stripped if present,
//! and the binary LLSD parsed with the viewer's depth limit.

use vkstorm_llsd::Value;
use zlib_rs::{Inflate, InflateError, InflateFlush, Status};

use super::llsd::parse_binary;

/// The names and numbers of `LLUZipHelper::EZipRresult`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ZipResult {
    Ok = 0,
    MemError = 1,
    /// The output would pass the cap (`ZR_SIZE_ERROR`): a decompression bomb.
    SizeError = 2,
    DataError = 3,
    ParseError = 4,
    BufferError = 5,
    VersionError = 6,
}

/// `UNZIP_LLSD_MAX_DEPTH`.
pub const UNZIP_LLSD_MAX_DEPTH: i32 = 96;
/// `UNZIP_ASSET_MAX_OUT`: the output cap for mesh LODs, skin and
/// decomposition blocks. The largest real LOD block is about
/// 15 to 22 MB inflated; 64 MiB leaves a wide margin and stops a bomb early.
pub const UNZIP_ASSET_MAX_OUT: usize = 64 * 1024 * 1024;
/// The inflate output chunk of `unzip_llsd` (`CHUNK = 1024 * 512`).
const CHUNK: usize = 1024 * 512;

const DEPRECATED_HEADER: &[u8] = b"<? LLSD/Binary ?>";

/// `strip_deprecated_header`: the slice after the legacy header when
/// present, and the header size the C++ reported (one more than the header
/// is long, as it counted).
pub fn strip_deprecated_header(data: &[u8]) -> (&[u8], usize) {
    if data.len() > DEPRECATED_HEADER.len() && &data[..DEPRECATED_HEADER.len()] == DEPRECATED_HEADER
    {
        (
            &data[DEPRECATED_HEADER.len()..],
            DEPRECATED_HEADER.len() + 1,
        )
    } else {
        (data, 0)
    }
}

/// Inflates a zlib stream completely with the C++ loop's result mapping: a
/// corrupt stream or a dictionary request is `DataError`, a stream that
/// ends before its end marker `BufferError` (zlib's `Z_BUF_ERROR` when the
/// input runs out), and a stream that stops being `Z_OK` without reaching
/// the end `DataError`.
///
/// The output is capped at [`UNZIP_ASSET_MAX_OUT`]; more is `SizeError`.
pub fn inflate_all(input: &[u8]) -> Result<Vec<u8>, ZipResult> {
    inflate_capped(input, UNZIP_ASSET_MAX_OUT)
}

/// [`inflate_all`] with an explicit output cap (`max_out` bytes; an output of
/// exactly `max_out` is accepted, one byte more is `SizeError`). The check
/// runs on every chunk before it is kept, so kept output never passes
/// `max_out`. An allocation that fails is `MemError`, not an abort.
pub fn inflate_capped(input: &[u8], max_out: usize) -> Result<Vec<u8>, ZipResult> {
    let mut inflate = Inflate::new(true, 15);
    let mut result: Vec<u8> = Vec::new();
    let mut chunk: Vec<u8> = Vec::new();
    if chunk.try_reserve_exact(CHUNK).is_err() {
        return Err(ZipResult::MemError);
    }
    chunk.resize(CHUNK, 0);
    let mut consumed = 0usize;
    loop {
        let before_in = inflate.total_in();
        let before_out = inflate.total_out();
        let status = inflate.decompress(&input[consumed..], &mut chunk, InflateFlush::NoFlush);
        consumed += (inflate.total_in() - before_in) as usize;
        let have = (inflate.total_out() - before_out) as usize;
        // The cap is checked where the C++ checks it: once the status is not an error.
        let over_cap = || {
            crate::ll_warns!(
                "Failed to unzip LLSD block: output exceeds the cap of {} bytes; input size: {} bytes.",
                max_out,
                input.len()
            );
            ZipResult::SizeError
        };
        match status {
            Err(InflateError::NeedDict { .. }) | Err(InflateError::DataError) => {
                return Err(ZipResult::DataError);
            }
            Err(InflateError::StreamError) => return Err(ZipResult::BufferError),
            Ok(Status::BufError) => return Err(ZipResult::BufferError),
            Err(InflateError::MemError) => return Err(ZipResult::MemError),
            Ok(Status::Ok) => {
                if result.len().saturating_add(have) > max_out {
                    return Err(over_cap());
                }
                if result.try_reserve(have).is_err() {
                    return Err(ZipResult::MemError);
                }
                result.extend_from_slice(&chunk[..have]);
            }
            Ok(Status::StreamEnd) => {
                if result.len().saturating_add(have) > max_out {
                    return Err(over_cap());
                }
                if result.try_reserve(have).is_err() {
                    return Err(ZipResult::MemError);
                }
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
        let status = deflate
            .compress(&bytes[consumed..], &mut chunk, DeflateFlush::Finish)
            .expect("deflate");
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
        let value = Value::Map(
            [
                (b"a".to_vec(), Value::Integer(5)),
                (b"b".to_vec(), Value::Binary(vec![1, 2, 3])),
            ]
            .into_iter()
            .collect(),
        );
        let (bytes, _) =
            vkstorm_llsd::format(vkstorm_llsd::Format::Binary, &value, Default::default());
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
        assert_eq!(
            unzip_llsd(&zip_bytes(b"\xffnot llsd")),
            Err(ZipResult::ParseError)
        );
    }

    #[test]
    fn large_output_grows() {
        let big: Vec<u8> = (0..2_000_000u32).map(|i| (i % 7) as u8).collect();
        let zipped = zip_bytes(&big);
        assert!(zipped.len() < 50_000);
        assert_eq!(inflate_all(&zipped).unwrap(), big);
    }

    /// Deflates `total` zero bytes fed to the encoder in 1 MiB pieces, so the
    /// bomb is never held inflated.
    fn zero_bomb(total: usize) -> Vec<u8> {
        use zlib_rs::{Deflate, DeflateFlush};
        let mut deflate = Deflate::new(9, true, 15);
        let zeros = vec![0u8; 1 << 20];
        let mut out: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; 65536];
        let mut fed = 0usize;
        loop {
            let piece = &zeros[..(total - fed).min(zeros.len())];
            let flush = if fed + piece.len() == total {
                DeflateFlush::Finish
            } else {
                DeflateFlush::NoFlush
            };
            let mut off = 0usize;
            loop {
                let (bi, bo) = (deflate.total_in(), deflate.total_out());
                let st = deflate
                    .compress(&piece[off..], &mut chunk, flush)
                    .expect("deflate");
                off += (deflate.total_in() - bi) as usize;
                out.extend_from_slice(&chunk[..(deflate.total_out() - bo) as usize]);
                if st == Status::StreamEnd || (off == piece.len() && flush != DeflateFlush::Finish)
                {
                    break;
                }
            }
            fed += piece.len();
            if fed == total {
                return out;
            }
        }
    }

    #[test]
    fn bomb_above_the_cap_is_refused() {
        let bomb = zero_bomb(UNZIP_ASSET_MAX_OUT + 1);
        assert!(bomb.len() < 1 << 20, "the bomb is small: {}", bomb.len());
        assert_eq!(inflate_all(&bomb), Err(ZipResult::SizeError));
        assert_eq!(unzip_llsd(&bomb), Err(ZipResult::SizeError));
    }

    #[test]
    fn consumers_refuse_a_bomb() {
        use crate::mesh::skin::{Decomposition, SkinInfo};
        use crate::mesh::unpack::{UnpackError, unpack_volume_faces};
        let bomb = zero_bomb(UNZIP_ASSET_MAX_OUT + 1);
        assert!(matches!(
            unpack_volume_faces(&bomb, 0),
            Err(UnpackError::Zip(ZipResult::SizeError))
        ));
        assert!(matches!(
            SkinInfo::from_zipped(&bomb),
            Err(ZipResult::SizeError)
        ));
        assert!(matches!(
            Decomposition::from_zipped(&bomb),
            Err(ZipResult::SizeError)
        ));
    }

    #[test]
    fn zip_result_numbers_are_the_cpp_ones() {
        assert_eq!(
            [
                ZipResult::Ok,
                ZipResult::MemError,
                ZipResult::SizeError,
                ZipResult::DataError,
                ZipResult::ParseError,
                ZipResult::BufferError,
                ZipResult::VersionError
            ]
            .map(|z| z as i32),
            [0, 1, 2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn output_exactly_at_the_cap_succeeds() {
        // Multi-chunk, and a size that is not a chunk multiple.
        let cap = 3 * CHUNK + 1234;
        let data: Vec<u8> = (0..cap).map(|i| (i % 251) as u8).collect();
        let zipped = zip_bytes(&data);
        assert_eq!(inflate_capped(&zipped, cap).unwrap(), data);
        assert_eq!(inflate_capped(&zipped, cap - 1), Err(ZipResult::SizeError));
        // The real cap, at the boundary.
        let at = zero_bomb(UNZIP_ASSET_MAX_OUT);
        assert_eq!(inflate_all(&at).unwrap().len(), UNZIP_ASSET_MAX_OUT);
    }
}
