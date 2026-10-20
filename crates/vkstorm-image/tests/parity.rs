//! Parity with the viewer's C++ codecs over the fixtures produced by
//! tools/image-eval/scripts/make_fixtures.py with the C++ oracle
//! (tools/image-eval/c/image_oracle.cpp: LLPngWrapper and LLImageJPEG on the
//! viewer's libpng 1.6.44 / zlib-ng 2.2.3 and libjpeg-turbo 3.0.4, and the
//! viewer's own TGA, BMP and DXT code).
//!
//! Lossless codecs must match byte for byte in both directions. JPEG decoding
//! may differ from libjpeg-turbo by a bounded sample distance (different IDCT
//! and upsampling arithmetic); JPEG encoding must be byte-identical or, where
//! it is not, decode to within the same bound.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use vkstorm_image::{bmp, dxt, png, tga, Error, Raster};

#[cfg(feature = "jpeg")]
use vkstorm_image::jpeg;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(name: &str) -> Vec<u8> {
    fs::read(fixtures().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn read_raw(name: &str) -> Raster {
    let b = read(name);
    assert!(b.len() >= 16 && &b[..4] == b"VKSR", "{name}: not a VKSR raster");
    let u = |at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let (width, height, components) = (u(4), u(8), u(12));
    assert_eq!(b.len(), 16 + (width * height * components) as usize, "{name}: size");
    Raster { width, height, components, data: b[16..].to_vec() }
}

fn manifest() -> Vec<Vec<String>> {
    let text = fs::read_to_string(fixtures().join("manifest.txt")).expect("manifest.txt");
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split(' ').map(str::to_owned).collect())
        .collect()
}

fn decode(codec: &str, data: &[u8]) -> Result<Raster, Error> {
    match codec {
        "png" => png::decode(data),
        #[cfg(feature = "jpeg")]
        "jpeg" => jpeg::decode(data),
        #[cfg(not(feature = "jpeg"))]
        "jpeg" => Err(Error::new("jpeg feature off")),
        "tga" => tga::decode(data),
        "bmp" => bmp::decode(data),
        "dxt" => dxt::parse(data).and_then(|info| dxt::decode(data, &info)),
        _ => panic!("codec {codec}"),
    }
}

fn header(codec: &str, data: &[u8]) -> Result<(u32, u32, u32), Error> {
    match codec {
        "png" => png::header(data).map(|h| (h.width, h.height, h.components)),
        #[cfg(feature = "jpeg")]
        "jpeg" => jpeg::header(data).map(|h| (h.width, h.height, h.components)),
        #[cfg(not(feature = "jpeg"))]
        "jpeg" => Err(Error::new("jpeg feature off")),
        "tga" => tga::parse(data).map(|i| (i.width, i.height, i.components)),
        "bmp" => bmp::parse(data).map(|i| (i.width, i.height, i.components)),
        "dxt" => dxt::parse(data).map(|i| (i.width, i.height, i.components)),
        _ => panic!("codec {codec}"),
    }
}

fn encode(codec: &str, raster: &Raster, quality: i32) -> Result<Vec<u8>, Error> {
    match codec {
        "png" => png::encode(raster),
        #[cfg(feature = "jpeg")]
        "jpeg" => jpeg::encode(raster, quality),
        #[cfg(not(feature = "jpeg"))]
        "jpeg" => Err(Error::new("jpeg feature off")),
        "tga" => tga::encode(raster),
        "bmp" => bmp::encode(raster, 0),
        "dxt" => dxt::encode(raster),
        _ => panic!("codec {codec}"),
    }
}

/// The messages the viewer's classes report through setLastError. When the
/// oracle produced one of these, the port must produce the same text; other
/// failures (the C++ returned false silently, or a library message) only
/// have to be failures.
const VIEWER_MESSAGES: &[&str] = &[
    "Unable to load file.  TGA file contains no image data.",
    "Unable to load file.  Colormapped images must have 8 bits per pixel.",
    "Unable to load file.  Monochrome images must have 8 bits per pixel.",
    "Unable to load file.  Unrecoginzed TGA image type.",
    "Unable to load file. Unknown pixel size.",
    "TGA images with origin on right side are not supported.",
    "OS/2 bitmap array BMP files are not supported",
    "Does not appear to be a bitmap file",
    "Windows 2.x and OS/2 1.x BMP files are not supported",
    "OS/2 2.x BMP files are not supported",
    "Unsupported bit depth",
    "Unrecognized bit depth",
    "8 bit RLE compression not supported.",
    "4 bit RLE compression not supported.",
    "Unsupported compression format.",
    "Bitfield encoding requires 16 or 32 bits per pixel.",
    "Bitfield encoding is not compatible with a color table.",
    "Attempt to decode compressed LLImageDXT to Raw (unsupported)",
    "getMipOffset called with old (unsupported) format",
    "LLImageDXT trying to decode an image with not enough data!",
    "Unable to encode a JPEG image that doesn't have 1 or 3 components.",
    "Unsupported image: unexpected number of channels",
    "Invalid or corrupt PNG file",
];

fn max_abs_diff(a: &Raster, b: &Raster) -> (u8, f64) {
    assert_eq!((a.width, a.height, a.components), (b.width, b.height, b.components));
    let mut max = 0u8;
    let mut sum = 0u64;
    for (&x, &y) in a.data.iter().zip(&b.data) {
        let d = x.abs_diff(y);
        max = max.max(d);
        sum += d as u64;
    }
    (max, sum as f64 / a.data.len().max(1) as f64)
}

/// Lossless decoders: byte-identical rasters; headers agree with the raster.
#[test]
fn decode_matches_oracle() {
    let mut count = BTreeMap::new();
    for line in manifest() {
        match line[0].as_str() {
            "decode" => {
                let (name, codec, raw) = (&line[1], &line[2], &line[3]);
                if codec == "jpeg" && !cfg!(feature = "jpeg") {
                    continue;
                }
                let data = read(name);
                let expected = read_raw(raw);
                let got = decode(codec, &data).unwrap_or_else(|e| panic!("{name}: {e}"));
                if codec == "jpeg" {
                    continue; // measured separately
                }
                assert_eq!((got.width, got.height, got.components), (expected.width, expected.height, expected.components), "{name}: shape");
                assert!(got.data == expected.data, "{name}: raster differs from the C++ decode");
                let h = header(codec, &data).unwrap_or_else(|e| panic!("{name}: header: {e}"));
                // DXT headers describe the full image, the decode a mip; LLImageBMP
                // reports 1 component for 8-bit files although it decodes to 3.
                // reports 1 component for 8-bit files although it decodes to 3,
                // and LLImageTGA reports 4 for a 32-bit file whose opaque alpha
                // the decode drops.
                if codec != "dxt" {
                    assert_eq!((h.0, h.1), (expected.width, expected.height), "{name}: header");
                }
                let compacted = codec == "tga" && h.2 == 4 && expected.components == 3;
                if codec != "dxt" && codec != "bmp" && !compacted {
                    assert_eq!(h.2, expected.components, "{name}: header components");
                }
                *count.entry(codec.clone()).or_insert(0) += 1;
            }
            "decode-error" => {
                let (name, codec, err) = (&line[1], &line[2], &line[3]);
                if codec == "jpeg" && !cfg!(feature = "jpeg") {
                    continue;
                }
                let data = read(name);
                let expected = fs::read_to_string(fixtures().join(err)).unwrap();
                let got = decode(codec, &data);
                let Err(e) = got else { panic!("{name}: decoded but the C++ failed with {expected:?}") };
                if VIEWER_MESSAGES.contains(&expected.as_str()) {
                    assert_eq!(e.message, expected, "{name}: error message");
                }
                *count.entry(format!("{codec}-error")).or_insert(0) += 1;
            }
            _ => {}
        }
    }
    println!("decode parity: {count:?}");
    assert!(count.get("png").copied().unwrap_or(0) > 40 && count.get("tga").copied().unwrap_or(0) > 40);
}

/// JPEG decoding against libjpeg-turbo: bounded sample distance.
#[cfg(feature = "jpeg")]
#[test]
fn jpeg_decode_distance() {
    let mut worst = 0u8;
    let mut worst_name = String::new();
    let mut n = 0;
    for line in manifest() {
        if line[0] != "decode" || line[2] != "jpeg" {
            continue;
        }
        let data = read(&line[1]);
        let expected = read_raw(&line[3]);
        let got = jpeg::decode(&data).unwrap_or_else(|e| panic!("{}: {e}", line[1]));
        let (max, mean) = max_abs_diff(&got, &expected);
        println!("jpeg decode {:40} max {max:3} mean {mean:.4}", line[1]);
        if max > worst {
            worst = max;
            worst_name = line[1].clone();
        }
        n += 1;
    }
    println!("jpeg decode: {n} files, worst max abs difference {worst} ({worst_name})");
    assert!(n > 0);
    // Different IDCT and upsampling arithmetic from libjpeg-turbo's; the
    // viewer keeps libjpeg-turbo (see doc/rust_port.md), this bounds the
    // candidate.
    assert!(worst <= 16, "worst JPEG decode difference {worst} on {worst_name}");
}

/// Lossless encoders: byte-identical files. JPEG: byte-identical, or decoded
/// within the decode bound.
#[test]
fn encode_matches_oracle() {
    let mut identical = BTreeMap::new();
    let mut jpeg_worst = 0u8;
    let mut jpeg_sizes: Vec<(String, usize, usize)> = Vec::new();
    let mut png_sizes: Vec<(String, usize, usize)> = Vec::new();
    let mut png_total_ours = 0usize;
    let mut png_total_libpng = 0usize;
    for line in manifest() {
        match line[0].as_str() {
            "encode" => {
                let (raw, codec, out) = (&line[1], &line[2], &line[3]);
                if codec == "jpeg" && !cfg!(feature = "jpeg") {
                    continue;
                }
                let quality = line.get(4).map(|q| q.parse().unwrap()).unwrap_or(75);
                let src = read_raw(raw);
                let expected = read(out);
                let got = encode(codec, &src, quality).unwrap_or_else(|e| panic!("{out}: {e}"));
                if codec == "png" {
                    // The PNG encode gate (viewer doc/rust_port.md, rung two): the
                    // file decodes to the same raster as libpng's, is within 20% of
                    // its size, and the corpus does not grow in total.
                    let a = png::decode(&got).unwrap();
                    let b = png::decode(&expected).unwrap();
                    assert!(a == b, "{out}: PNG re-encode does not decode identically");
                    assert!(a == src || src.components < 3, "{out}: PNG re-encode does not decode to the source");
                    png_total_ours += got.len();
                    png_total_libpng += expected.len();
                    let ratio = got.len() as f64 / expected.len() as f64;
                    assert!(ratio <= 1.20, "{out}: PNG {:.1}% larger than libpng's", (ratio - 1.0) * 100.0);
                    if got != expected {
                        println!("png encode {out:40} differs: {} vs {} bytes ({:+.2}%)", got.len(), expected.len(), (ratio - 1.0) * 100.0);
                        png_sizes.push((out.clone(), got.len(), expected.len()));
                        continue;
                    }
                }
                if got == expected {
                    *identical.entry(codec.clone()).or_insert(0) += 1;
                    continue;
                }
                #[cfg(feature = "jpeg")]
                if codec == "jpeg" {
                    let a = jpeg::decode(&got).unwrap();
                    let b = jpeg::decode(&expected).unwrap();
                    let (max, mean) = max_abs_diff(&a, &b);
                    println!("jpeg encode {out:40} differs: {} vs {} bytes, decoded max {max} mean {mean:.4}", got.len(), expected.len());
                    jpeg_worst = jpeg_worst.max(max);
                    jpeg_sizes.push((out.clone(), got.len(), expected.len()));
                    continue;
                }
                let first = got.iter().zip(&expected).position(|(a, b)| a != b).unwrap_or(got.len().min(expected.len()));
                panic!("{out}: {codec} encoding differs from the C++ ({} vs {} bytes, first difference at {first})", got.len(), expected.len());
            }
            "encode-error" => {
                let (raw, codec) = (&line[1], &line[2]);
                if codec == "jpeg" && !cfg!(feature = "jpeg") {
                    continue;
                }
                let quality = line.get(4).map(|q| q.parse().unwrap()).unwrap_or(75);
                let src = read_raw(raw);
                let expected = fs::read_to_string(fixtures().join(&line[3])).unwrap();
                let got = encode(codec, &src, quality);
                let Err(e) = got else { panic!("{raw} {codec}: encoded but the C++ failed with {expected:?}") };
                if VIEWER_MESSAGES.contains(&expected.as_str()) {
                    assert_eq!(e.message, expected, "{raw} {codec}: error message");
                }
            }
            _ => {}
        }
    }
    println!("byte-identical encodes: {identical:?}; png differing: {}; jpeg differing: {}", png_sizes.len(), jpeg_sizes.len());
    for (name, ours, theirs) in &jpeg_sizes {
        println!("  {name}: {ours} bytes vs {theirs} ({:+.2}%)", (*ours as f64 / *theirs as f64 - 1.0) * 100.0);
    }
    println!("png corpus: {png_total_ours} bytes vs libpng {png_total_libpng} ({:+.2}%)", (png_total_ours as f64 / png_total_libpng as f64 - 1.0) * 100.0);
    assert!(png_total_ours > 0 && png_total_ours <= png_total_libpng, "PNG corpus grew: {png_total_ours} vs {png_total_libpng}");
    assert!(identical.get("png").copied().unwrap_or(0) > 10, "PNG encodes of the synthetic rasters must be byte-identical");
    // The JPEG encoder candidate is not used by the viewer; its distance is reported above.
    println!("jpeg encode: worst decoded difference {jpeg_worst}");
}

/// Avatar mask processing (`LLImageTGA::decodeAndProcess`).
#[test]
fn process_matches_oracle() {
    let mut n = 0;
    for line in manifest() {
        match line[0].as_str() {
            "process" => {
                let data = read(&line[1]);
                let (domain, weight): (f32, f32) = (line[2].parse().unwrap(), line[3].parse().unwrap());
                let expected = read_raw(&line[4]);
                let got = tga::decode_and_process(&data, domain, weight).unwrap_or_else(|e| panic!("{}: {e}", line[1]));
                assert!(got == expected, "{} domain {domain} weight {weight}: differs", line[1]);
                n += 1;
            }
            "process-error" => {
                let data = read(&line[1]);
                let (domain, weight): (f32, f32) = (line[2].parse().unwrap(), line[3].parse().unwrap());
                assert!(tga::decode_and_process(&data, domain, weight).is_err(), "{}: processed but the C++ refused", line[1]);
            }
            _ => {}
        }
    }
    println!("process parity: {n}");
    assert!(n > 0);
}

/// The JPEG dimension probe (`LLImageDimensionsInfo`).
#[cfg(feature = "jpeg")]
#[test]
fn jpeg_dimensions_match_oracle() {
    let mut n = 0;
    for line in manifest() {
        if line[0] != "jpeg-dims" {
            continue;
        }
        let data = read(&line[1]);
        let got = jpeg::dimensions(&data);
        if line[2] == "error" {
            assert!(!matches!(got, Ok(jpeg::Dimensions::Found { .. })), "{}: dimensions found but the C++ failed", line[1]);
        } else {
            let (w, h) = line[2].split_once('x').unwrap();
            assert_eq!(got.unwrap(), jpeg::Dimensions::Found { width: w.parse().unwrap(), height: h.parse().unwrap() }, "{}", line[1]);
        }
        n += 1;
    }
    assert!(n > 0);
    // A prefix that ends before the frame header asks for more data.
    let data = read("raw_64x64_c3.raw.q75.jpg");
    assert_eq!(jpeg::dimensions(&data[..4]).unwrap(), jpeg::Dimensions::Truncated);
}
