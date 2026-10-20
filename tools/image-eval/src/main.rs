//! image-eval: the Rust side of the vkstorm-image measurements, mirroring
//! c/image_oracle.cpp's commands so run_bench.py can drive both.
//!
//!   image-eval bench <codec> decode|encode|header <file> <iters> [quality]
//!   image-eval <codec>-decode <in> <out.raw> | <codec>-encode <in.raw> <out> [quality]
//!   image-eval compare <a.raw> <b.raw>
//!
//! Rasters on disk use the oracle's VKSR layout.

use std::time::Instant;

use vkstorm_image::{bmp, dxt, jpeg, png, tga, Error, Raster};

fn read_raw(path: &str) -> Result<Raster, String> {
    let b = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if b.len() < 16 || &b[..4] != b"VKSR" {
        return Err(format!("{path}: not a VKSR raster"));
    }
    let u = |at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let (width, height, components) = (u(4), u(8), u(12));
    Ok(Raster { width, height, components, data: b[16..].to_vec() })
}

fn write_raw(path: &str, r: &Raster) -> Result<(), String> {
    let mut b = Vec::with_capacity(16 + r.data.len());
    b.extend_from_slice(b"VKSR");
    b.extend_from_slice(&r.width.to_le_bytes());
    b.extend_from_slice(&r.height.to_le_bytes());
    b.extend_from_slice(&r.components.to_le_bytes());
    b.extend_from_slice(&r.data);
    std::fs::write(path, b).map_err(|e| format!("{path}: {e}"))
}

fn decode(codec: &str, data: &[u8]) -> Result<Raster, Error> {
    match codec {
        "png" => png::decode(data),
        "jpeg" => jpeg::decode(data),
        "tga" => tga::decode(data),
        "bmp" => bmp::decode(data),
        "dxt" => dxt::parse(data).and_then(|i| dxt::decode(data, &i)),
        _ => Err(Error::new(format!("unknown codec {codec}"))),
    }
}

fn header(codec: &str, data: &[u8]) -> Result<usize, Error> {
    Ok(match codec {
        "png" => png::header(data).map(|h| (h.width * h.height * h.components) as usize)?,
        "jpeg" => jpeg::header(data).map(|h| (h.width * h.height * h.components) as usize)?,
        "tga" => tga::parse(data).map(|i| (i.width * i.height * i.components) as usize)?,
        "bmp" => bmp::parse(data).map(|i| (i.width * i.height * i.components) as usize)?,
        "dxt" => dxt::parse(data).map(|i| (i.width * i.height * i.components) as usize)?,
        _ => return Err(Error::new(format!("unknown codec {codec}"))),
    })
}

fn encode(codec: &str, raster: &Raster, quality: i32) -> Result<Vec<u8>, Error> {
    match codec {
        "png" => png::encode(raster),
        "jpeg" => jpeg::encode(raster, quality),
        "tga" => tga::encode(raster),
        "bmp" => bmp::encode(raster, 0),
        "dxt" => dxt::encode(raster),
        _ => Err(Error::new(format!("unknown codec {codec}"))),
    }
}

fn run(args: &[String]) -> Result<String, String> {
    let cmd = args.first().ok_or("usage: see source")?.as_str();
    match cmd {
        "bench" => {
            let [codec, op, path, iters] = [&args[1], &args[2], &args[3], &args[4]];
            let iters: usize = iters.parse().map_err(|_| "iters")?;
            let quality: i32 = args.get(5).map(|q| q.parse().unwrap_or(75)).unwrap_or(75);
            let mut best = f64::MAX;
            let mut total = 0.0;
            let mut out_size = 0usize;
            let (data, raw) = if op == "encode" {
                (Vec::new(), Some(read_raw(path)?))
            } else {
                (std::fs::read(path).map_err(|e| e.to_string())?, None)
            };
            for _ in 0..iters {
                let t0 = Instant::now();
                let r: Result<usize, Error> = match op.as_str() {
                    "encode" => encode(codec, raw.as_ref().unwrap(), quality).map(|v| v.len()),
                    "header" => header(codec, &data),
                    _ => decode(codec, &data).map(|r| r.data.len()),
                };
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                out_size = r.map_err(|e| format!("FAIL: {e}"))?;
                best = best.min(ms);
                total += ms;
            }
            Ok(format!("{best:.4} {:.4} {out_size}", total / iters as f64))
        }
        "crc-list" => {
            // One line per path in the list file, as the oracle's crc-list:
            // "<crc32> <w> <h> <c> <path>" or "FAIL <path>".
            let list = std::fs::read_to_string(&args[1]).map_err(|e| e.to_string())?;
            let mut out = String::new();
            for path in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let codec = match path.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
                    Some("jpg") | Some("jpeg") => "jpeg",
                    Some("mip") => "dxt",
                    Some(e) => e,
                    None => "",
                }
                .to_owned();
                let r = std::fs::read(path).map_err(|e| e.to_string()).and_then(|d| decode(&codec, &d).map_err(|e| e.to_string()));
                match r {
                    Ok(r) => out.push_str(&format!("{:08x} {} {} {} {path}
", crc32(&r.data), r.width, r.height, r.components)),
                    Err(_) => out.push_str(&format!("FAIL {path}
")),
                }
            }
            Ok(out.trim_end().to_owned())
        }
        "compare" => {
            let a = read_raw(&args[1])?;
            let b = read_raw(&args[2])?;
            if (a.width, a.height, a.components) != (b.width, b.height, b.components) {
                return Err(format!("shape differs: {}x{}x{} vs {}x{}x{}", a.width, a.height, a.components, b.width, b.height, b.components));
            }
            let mut max = 0u8;
            let mut sum = 0u64;
            let mut sq = 0f64;
            for (&x, &y) in a.data.iter().zip(&b.data) {
                let d = x.abs_diff(y);
                max = max.max(d);
                sum += d as u64;
                sq += (d as f64) * (d as f64);
            }
            let n = a.data.len().max(1) as f64;
            let mse = sq / n;
            let psnr = if mse <= 0.0 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() };
            Ok(format!("max_abs {max} mean_abs {:.5} psnr {psnr:.2}", sum as f64 / n))
        }
        _ => {
            let (codec, op) = cmd.split_once('-').ok_or("usage: see source")?;
            match op {
                "decode" => {
                    let data = std::fs::read(&args[1]).map_err(|e| e.to_string())?;
                    let r = decode(codec, &data).map_err(|e| format!("FAIL: {e}"))?;
                    write_raw(&args[2], &r)?;
                    Ok(format!("OK {} {} {}", r.width, r.height, r.components))
                }
                "encode" => {
                    let raw = read_raw(&args[1])?;
                    let quality: i32 = args.get(3).map(|q| q.parse().unwrap_or(75)).unwrap_or(75);
                    let out = encode(codec, &raw, quality).map_err(|e| format!("FAIL: {e}"))?;
                    std::fs::write(&args[2], &out).map_err(|e| e.to_string())?;
                    Ok(format!("OK {}", out.len()))
                }
                _ => Err("usage: see source".into()),
            }
        }
    }
}

/// CRC-32 (IEEE, as zlib's crc32) of a buffer.
fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(s) => println!("{s}"),
        Err(e) => {
            println!("{e}");
            std::process::exit(1);
        }
    }
}
