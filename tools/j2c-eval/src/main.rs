//! j2c-eval: decode JPEG 2000 codestreams with the pure-Rust decoders the way the
//! Second Life viewer uses OpenJPEG (lenient, resolution-reduced, truncated input),
//! time them, and compare raw outputs.
//!
//! usage:
//!   j2c-eval hayro <file> <reduce> <max_bytes|0> <iters> [out.raw]
//!   j2c-eval j2k   <file> <reduce> <max_bytes|0> <iters> [out.raw]   (single-threaded)
//!   j2c-eval j2k-auto <file> <reduce> <max_bytes|0> <iters> [out.raw] (crate-chosen parallelism)
//!   j2c-eval compare <a.raw> <b.raw>
//! prints for decoders: w h comps best_ms mean_ms   (or FAIL <reason>)
use std::time::Instant;

fn load(path: &str, max_bytes: usize) -> Vec<u8> {
    let mut d = std::fs::read(path).expect("read");
    if max_bytes > 0 && max_bytes < d.len() { d.truncate(max_bytes); }
    d
}

struct Out { w: u32, h: u32, c: u32, data: Vec<u8> }

fn hayro_decode(data: &[u8], reduce: u32) -> Result<Out, String> {
    use hayro_jpeg2000::{DecodeSettings, DecoderContext, Image};
    let mut settings = DecodeSettings::default();
    settings.strict = false;
    let img = Image::new(data, &settings).map_err(|e| format!("{e:?}"))?;
    let (fw, fh) = (img.width(), img.height());
    let img = if reduce > 0 {
        settings.target_resolution = Some((fw >> reduce, fh >> reduce));
        Image::new(data, &settings).map_err(|e| format!("{e:?}"))?
    } else { img };
    let mut ctx = DecoderContext::default();
    let dec = img.decode(&mut ctx).map_err(|e| format!("{e:?}"))?;
    let c = dec.components().len() as u32;
    let data = dec.data_u8();
    // derive output dimensions from sample count (hayro reports full-size width/height)
    let n = (data.len() as u32) / c;
    let (mut w, mut h) = (fw, fh);
    if n != w * h { w = fw >> reduce; h = fh >> reduce; if n != w * h { w = (n as f64).sqrt() as u32; h = n / w.max(1); } }
    Ok(Out { w, h, c, data })
}

fn j2k_decode(data: &[u8], reduce: u32, serial: bool) -> Result<Out, String> {
    use j2k::{CpuDecodeParallelism, DecodeSettings, J2kDecoder};
    let mut dec = J2kDecoder::new_with_settings(data, DecodeSettings::lenient()).map_err(|e| format!("{e}"))?;
    if serial { dec.set_cpu_decode_parallelism(CpuDecodeParallelism::Serial); }
    // decode_srgb8 rejects 4-component (RGBA) input, so always go through the
    // native-components path, which handles any component count and reduction level.
    {
        let comps = dec.decode_native_components_at_reduction(reduce as u8).map_err(|e| format!("{e}"))?;
        let planes = comps.planes();
        let (w, h) = planes[0].dimensions();
        let c = planes.len() as u32;
        let mut out = vec![0u8; (w * h * c) as usize];
        for (ci, p) in planes.iter().enumerate() {
            let bps = p.bytes_per_sample() as usize;
            let d = p.data();
            for i in 0..(w * h) as usize {
                // take the most significant byte for >8-bit samples (little-endian packing assumed)
                let v = if bps == 1 { d[i] } else { d[i * bps + bps - 1] };
                out[i * c as usize + ci] = v;
            }
        }
        Ok(Out { w, h, c, data: out })
    }
}

fn openjp2_decode(data: &[u8], reduce: u32) -> Result<Out, String> {
    // The openjp2 crate: Rust port of OpenJPEG (c2rust lineage), via the jpeg2k
    // wrapper; lenient like opj_decoder_set_strict_mode(false), reduce = discard.
    use jpeg2k::{DecodeParameters, Image, ImagePixelData};
    let params = DecodeParameters::new().reduce(reduce).strict(false);
    let img = Image::from_bytes_with(data, params).map_err(|e| format!("{e}"))?;
    let px = img.get_pixels(None).map_err(|e| format!("{e}"))?;
    let (data, c) = match px.data {
        ImagePixelData::L8(v) => (v, 1),
        ImagePixelData::La8(v) => (v, 2),
        ImagePixelData::Rgb8(v) => (v, 3),
        ImagePixelData::Rgba8(v) => (v, 4),
        _ => return Err("16-bit output".into()),
    };
    Ok(Out { w: px.width, h: px.height, c, data })
}

fn vks_decode(data: &[u8], reduce: u32) -> Result<Out, String> {
    // Our codec: vkstorm-j2c on the vkstorm-openjp2 port (bottom-up rows, so
    // flip back to top-down to compare with the other decoders).
    let d = vkstorm_j2c::decode(data, 0, reduce, 0, 4).map_err(|e| e.to_string())?;
    let (w, h, c) = (d.width as usize, d.height as usize, d.channels as usize);
    let mut top = Vec::with_capacity(d.data.len());
    for y in (0..h).rev() { top.extend_from_slice(&d.data[y * w * c..(y + 1) * w * c]); }
    Ok(Out { w: d.width, h: d.height, c: d.channels, data: top })
}

fn run(which: &str, args: &[String]) {
    let path = &args[0]; let reduce: u32 = args[1].parse().unwrap();
    let max_bytes: usize = args[2].parse().unwrap(); let iters: usize = args[3].parse().unwrap();
    let data = load(path, max_bytes);
    let mut best = f64::MAX; let mut total = 0.0; let mut last: Option<Out> = None;
    for _ in 0..iters {
        let t0 = Instant::now();
        let r = match which { "hayro" => hayro_decode(&data, reduce), "openjp2" => openjp2_decode(&data, reduce), "vks" => vks_decode(&data, reduce), "j2k-auto" => j2k_decode(&data, reduce, false), _ => j2k_decode(&data, reduce, true) };
        let dt = t0.elapsed().as_secs_f64() * 1000.0;
        match r { Ok(o) => { if dt < best { best = dt; } total += dt; last = Some(o); }
                  Err(e) => { println!("FAIL {}", e.replace('\n', " ")); return; } }
    }
    let o = last.unwrap();
    println!("{} {} {} {:.3} {:.3}", o.w, o.h, o.c, best, total / iters as f64);
    if args.len() >= 5 { std::fs::write(&args[4], &o.data).unwrap(); }
}

fn compare(a: &str, b: &str) {
    let (x, y) = (std::fs::read(a).unwrap(), std::fs::read(b).unwrap());
    if x.len() != y.len() { println!("LEN_MISMATCH {} {}", x.len(), y.len()); return; }
    let mut maxd = 0u32; let mut sum = 0u64; let mut sq = 0f64;
    for (p, q) in x.iter().zip(y.iter()) { let d = (*p as i32 - *q as i32).unsigned_abs(); maxd = maxd.max(d); sum += d as u64; sq += (d as f64) * (d as f64); }
    let n = x.len() as f64; let mse = sq / n;
    let psnr = if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 / mse).log10() };
    println!("max_abs {} mean_abs {:.4} psnr {:.2}", maxd, sum as f64 / n, psnr);
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(|s| s.as_str()) {
        Some("hayro") | Some("j2k") | Some("j2k-auto") | Some("openjp2") | Some("vks") => run(&a[1], &a[2..]),
        Some("compare") => compare(&a[2], &a[3]),
        _ => { eprintln!("usage: j2c-eval hayro|j2k file reduce max_bytes iters [out.raw] | compare a b"); std::process::exit(2); }
    }
}
