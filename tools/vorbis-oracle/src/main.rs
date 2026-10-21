//! vorbis-oracle: the Rust side of the vkstorm-vorbis parity and timing
//! harness. Every command mirrors `c/vorbis_oracle.c` and writes the same
//! trace format (`crates/vkstorm-vorbis/tests/trace/mod.rs`, shared with
//! the crate's tests), so the two can be compared line for line.
//!
//! ```text
//! vorbis-oracle file <in.ogg> <trace> [--keep-going] [--word W] [--be] [--unsigned] [--len L] [--pcm out.raw]
//! vorbis-oracle packets <in.ogg> <trace> [--chunk N]
//! vorbis-oracle tables <out>
//! vorbis-oracle bench <listfile> <reps>
//! vorbis-oracle compare <c-oracle> <corpus-dir> <workdir> [--jobs N] [--quick]
//! vorbis-oracle expected <c-oracle> <fixtures-dir> <out.txt>
//! vorbis-oracle fuzz <corpus-dir> <iterations> [seed]
//! ```
//!
//! `compare` runs the C oracle (a subprocess per file and mode, so a crash
//! of the C library on a malicious input is contained) and the Rust port
//! in-process over every `.ogg` under the corpus directory in all trace
//! modes (`Mode::all`: the viewer's ov_read loop, continuing past holes,
//! 8-bit unsigned, big-endian unsigned with an odd buffer length, the
//! packet API with 4096- and 37-byte reads; `--quick` keeps the first
//! three) and reports every trace that differs. `expected` writes the C
//! traces of the committed fixtures (`crates/vkstorm-vorbis/tests`).
//! `fuzz` decodes random mutations of the corpus files with the Rust port
//! only (byte flips, truncations, splices, page-sized cuts) and reports any
//! panic; the C library is not involved (it crashes on some of these).

#[path = "../../../crates/vkstorm-vorbis/tests/trace/mod.rs"]
mod trace;

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use trace::{FileOpts, Mode, c_trace, collect_ogg, run_compare};
use vkstorm_vorbis::VorbisFile;

fn run_tables() -> String {
    let mut o = String::new();
    let mut n = 64;
    while n <= 8192 {
        let _ = write!(o, "mdct {n}");
        for v in vkstorm_vorbis::__internal::mdct_trig(n) {
            let _ = write!(o, " {:08x}", v.to_bits());
        }
        o.push('\n');
        n *= 2;
    }
    for i in 0..8 {
        let _ = write!(o, "window {i}");
        for v in vkstorm_vorbis::__internal::window(i) {
            let _ = write!(o, " {:08x}", v.to_bits());
        }
        o.push('\n');
    }
    o
}

/// The viewer's decode loop, as timed by the C oracle's `bench`.
fn decode_all(data: &[u8]) -> i64 {
    let Ok(mut vf) = VorbisFile::open(data) else {
        return -1;
    };
    let mut buf = [0u8; 4096];
    let mut section = 0;
    let mut total = 0i64;
    loop {
        let r = vf.read(&mut buf, false, 2, true, &mut section);
        if r <= 0 {
            break;
        }
        total += r as i64;
    }
    total
}

fn run_bench(listfile: &str, reps: usize) -> Result<(), String> {
    let list = fs::read_to_string(listfile).map_err(|e| e.to_string())?;
    let files: Vec<(String, Vec<u8>)> = list
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| {
            fs::read(l)
                .map(|d| (l.to_string(), d))
                .map_err(|e| format!("{l}: {e}"))
        })
        .collect::<Result<_, _>>()?;
    let mut best = vec![f64::INFINITY; files.len()];
    let mut best_total = f64::INFINITY;
    for _ in 0..reps {
        let mut total = 0.0;
        for (i, (_, d)) in files.iter().enumerate() {
            let t0 = Instant::now();
            std::hint::black_box(decode_all(std::hint::black_box(d)));
            let dt = t0.elapsed().as_secs_f64();
            best[i] = best[i].min(dt);
            total += dt;
        }
        best_total = best_total.min(total);
    }
    for (i, (name, _)) in files.iter().enumerate() {
        println!("file {:.9} {}", best[i], name);
    }
    println!("total {best_total:.9}");
    Ok(())
}

fn run_expected(oracle: &str, dir: &str, out: &str) -> Result<(), String> {
    let mut files = Vec::new();
    collect_ogg(Path::new(dir), &mut files);
    let tmp = PathBuf::from(format!("{out}.tmp"));
    let mut text = String::new();
    for f in &files {
        let rel = f
            .strip_prefix(dir)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        for mode in Mode::all() {
            let c = c_trace(oracle, f, &mode, &tmp)
                .map_err(|e| format!("{rel} [{}]: {e}", mode.name()))?;
            let _ = writeln!(text, "== {rel} {}", mode.name());
            text.push_str(&c);
        }
    }
    let _ = fs::remove_file(&tmp);
    fs::write(out, text).map_err(|e| e.to_string())?;
    println!(
        "{} fixtures, {} traces",
        files.len(),
        files.len() * Mode::all().len()
    );
    Ok(())
}

/// splitmix64, as the corpus generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

fn mutate(rng: &mut Rng, src: &[u8], other: &[u8]) -> Vec<u8> {
    let mut d = src.to_vec();
    let rounds = 1 + rng.below(4);
    for _ in 0..rounds {
        match rng.below(7) {
            0 | 1 => {
                // flip bytes, biased to the headers at the start
                let n = 1 + rng.below(8);
                for _ in 0..n {
                    if d.is_empty() {
                        break;
                    }
                    let lim = if rng.below(2) == 0 {
                        d.len().min(4096)
                    } else {
                        d.len()
                    };
                    let i = rng.below(lim);
                    d[i] ^= 1 << rng.below(8);
                }
            }
            2 => {
                let n = rng.below(d.len() + 1);
                d.truncate(n);
            }
            3 => {
                // delete a range
                if !d.is_empty() {
                    let a = rng.below(d.len());
                    let b = (a + 1 + rng.below(4096)).min(d.len());
                    d.drain(a..b);
                }
            }
            4 => {
                // splice in a piece of another file
                if !other.is_empty() {
                    let a = rng.below(other.len());
                    let b = (a + 1 + rng.below(8192)).min(other.len());
                    let at = rng.below(d.len() + 1);
                    let piece = other[a..b].to_vec();
                    d.splice(at..at, piece);
                }
            }
            5 => {
                // random byte values in a run
                if !d.is_empty() {
                    let a = rng.below(d.len());
                    let b = (a + 1 + rng.below(64)).min(d.len());
                    for x in &mut d[a..b] {
                        *x = rng.next() as u8;
                    }
                }
            }
            _ => {
                // duplicate a range
                if !d.is_empty() {
                    let a = rng.below(d.len());
                    let b = (a + 1 + rng.below(8192)).min(d.len());
                    let piece = d[a..b].to_vec();
                    let at = rng.below(d.len() + 1);
                    d.splice(at..at, piece);
                }
            }
        }
    }
    d
}

/// Recomputes the CRC of every complete page found in `d`, so that
/// mutated pages reach the codec instead of being dropped by the framing.
fn fix_crcs(d: &mut [u8]) {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = (i as u32) << 24;
        for _ in 0..8 {
            c = (c << 1) ^ if c & 0x8000_0000 != 0 { 0x04c1_1db7 } else { 0 };
        }
        *t = c;
    }
    let mut i = 0;
    while i + 27 <= d.len() {
        if &d[i..i + 4] != b"OggS" {
            i += 1;
            continue;
        }
        let nseg = d[i + 26] as usize;
        if i + 27 + nseg > d.len() {
            break;
        }
        let body: usize = d[i + 27..i + 27 + nseg].iter().map(|&b| b as usize).sum();
        let end = i + 27 + nseg + body;
        if end > d.len() {
            break;
        }
        d[i + 22..i + 26].fill(0);
        let mut crc = 0u32;
        for &b in &d[i..end] {
            crc = (crc << 8) ^ table[(((crc >> 24) as u8) ^ b) as usize];
        }
        d[i + 22..i + 26].copy_from_slice(&crc.to_le_bytes());
        i = end;
    }
}

fn run_fuzz(corpus: &str, iterations: usize, seed: u64) -> Result<bool, String> {
    let mut files = Vec::new();
    collect_ogg(Path::new(corpus), &mut files);
    let datas: Vec<Vec<u8>> = files
        .iter()
        .filter_map(|f| fs::read(f).ok())
        .filter(|d| d.len() < 2_000_000)
        .collect();
    if datas.is_empty() {
        return Err("no input files".into());
    }
    let jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let datas = Arc::new(datas);
    let done = Arc::new(AtomicUsize::new(0));
    let panics = Arc::new(Mutex::new(Vec::<(u64, usize)>::new()));
    let decoded = Arc::new(AtomicUsize::new(0));
    std::panic::set_hook(Box::new(|_| {}));
    let mut handles = Vec::new();
    for t in 0..jobs {
        let datas = datas.clone();
        let done = done.clone();
        let panics = panics.clone();
        let decoded = decoded.clone();
        handles.push(std::thread::spawn(move || {
            loop {
                let i = done.fetch_add(1, Ordering::Relaxed);
                if i >= iterations {
                    break;
                }
                let s = seed ^ (i as u64).wrapping_mul(0x2545_F491_4F6C_DD1D) ^ t as u64;
                let mut rng = Rng(s);
                let src = &datas[rng.below(datas.len())];
                let other = &datas[rng.below(datas.len())];
                let mut d = mutate(&mut rng, src, other);
                if rng.below(4) != 0 {
                    fix_crcs(&mut d);
                }
                let mode = match rng.below(4) {
                    0 => Mode::Packets(1 + rng.below(5000)),
                    _ => Mode::File(FileOpts {
                        keep_going: true,
                        ..Default::default()
                    }),
                };
                match std::panic::catch_unwind(|| mode.trace(&d)) {
                    Err(_) => {
                        if let Ok(mut p) = panics.lock() {
                            p.push((s, i));
                        }
                    }
                    Ok(t) => {
                        // some audio came out
                        let produced = t.lines().any(|l| {
                            (l.starts_with("read ")
                                && !l.starts_with("read 0 ")
                                && !l.starts_with("read -"))
                                || (l.starts_with("pkt ") && !l.contains(" 0 x"))
                        });
                        if produced {
                            decoded.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    let _ = std::panic::take_hook();
    let panics = panics.lock().map_err(|_| "poisoned".to_string())?;
    println!(
        "fuzz iterations {iterations} with audio output {} panics {}",
        decoded.load(Ordering::Relaxed),
        panics.len()
    );
    for (s, i) in panics.iter().take(20) {
        println!("  panic at iteration {i} (rng state {s:#x})");
    }
    Ok(panics.is_empty())
}

fn parse_file_opts(args: &[String]) -> Result<(FileOpts, Option<String>), String> {
    let mut o = FileOpts::default();
    let mut pcm = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--keep-going" => o.keep_going = true,
            "--word" => {
                i += 1;
                o.word = args.get(i).and_then(|s| s.parse().ok()).ok_or("--word N")?;
            }
            "--be" => o.bigendian = true,
            "--unsigned" => o.signed = false,
            "--len" => {
                i += 1;
                o.len = args.get(i).and_then(|s| s.parse().ok()).ok_or("--len N")?;
            }
            "--pcm" => {
                i += 1;
                pcm = args.get(i).cloned();
            }
            a => return Err(format!("unknown option {a}")),
        }
        i += 1;
    }
    Ok((o, pcm))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r: Result<i32, String> = (|| {
        match args.get(1).map(String::as_str) {
        Some("file") if args.len() >= 4 => {
            let (opt, pcm_path) = parse_file_opts(&args[4..])?;
            let data = fs::read(&args[2]).map_err(|e| e.to_string())?;
            let mut pcm = Vec::new();
            let t = trace::run_file(&data, &opt, pcm_path.as_ref().map(|_| &mut pcm));
            fs::write(&args[3], t).map_err(|e| e.to_string())?;
            if let Some(p) = pcm_path {
                fs::write(p, pcm).map_err(|e| e.to_string())?;
            }
            Ok(0)
        }
        Some("packets") if args.len() >= 4 => {
            let chunk = if args.get(4).map(String::as_str) == Some("--chunk") {
                args.get(5).and_then(|s| s.parse().ok()).ok_or("--chunk N")?
            } else {
                4096
            };
            let data = fs::read(&args[2]).map_err(|e| e.to_string())?;
            fs::write(&args[3], trace::run_packets(&data, chunk)).map_err(|e| e.to_string())?;
            Ok(0)
        }
        Some("tables") if args.len() >= 3 => {
            fs::write(&args[2], run_tables()).map_err(|e| e.to_string())?;
            Ok(0)
        }
        Some("bench") if args.len() >= 4 => {
            run_bench(&args[2], args[3].parse().map_err(|_| "reps")?)?;
            Ok(0)
        }
        Some("compare") if args.len() >= 5 => {
            let mut jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
            let mut quick = false;
            let mut i = 5;
            while i < args.len() {
                match args[i].as_str() {
                    "--jobs" => {
                        i += 1;
                        jobs = args.get(i).and_then(|s| s.parse().ok()).ok_or("--jobs N")?;
                    }
                    "--quick" => quick = true,
                    a => return Err(format!("unknown option {a}")),
                }
                i += 1;
            }
            Ok(if run_compare(&args[2], &args[3], &args[4], jobs, quick)? { 0 } else { 1 })
        }
        Some("fuzz") if args.len() >= 4 => {
            let iterations = args[3].parse().map_err(|_| "iterations")?;
            let seed = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1);
            Ok(if run_fuzz(&args[2], iterations, seed)? { 0 } else { 1 })
        }
        Some("expected") if args.len() >= 5 => {
            run_expected(&args[2], &args[3], &args[4])?;
            Ok(0)
        }
        _ => Err("usage: vorbis-oracle file|packets|tables|bench|compare|expected|fuzz ... (see the source header)".into()),
    }
    })();
    match r {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    }
}
