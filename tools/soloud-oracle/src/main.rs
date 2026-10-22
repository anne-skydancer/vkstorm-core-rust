//! soloud-oracle: parity and timing harness for vkstorm-audio's mixer, WAV
//! reader and asset decoder against the viewer's C/C++ (see README.md).
//!
//! ```text
//! soloud-oracle gen <dir>                               fixtures + scenes
//! soloud-oracle scene <scene.txt> <out.f32> <out.log>   Rust runner
//! soloud-oracle compare <soloud_oracle> <dir>           C++ vs Rust, every scene
//! soloud-oracle bench <scene.txt> <reps>                min ns per mix call
//! soloud-oracle bench-scene <dir> <rate> <channels> <periods> <out.txt>
//! soloud-oracle expected <soloud_oracle> <dir> <out.txt> C hashes for the parity test
//! soloud-oracle wav-compare <soloud_oracle> <dir>       dr_wav vs wav.rs
//! soloud-oracle asset-compare <asset_oracle> <fixtures-dir> <workdir>
//! soloud-oracle asset-expected <asset_oracle> <fixtures-dir> <workdir> <out.txt>
//! soloud-oracle asset-bench <list.txt> <reps>
//! ```

#[path = "../../../crates/vkstorm-audio/tests/mixer_common/mod.rs"]
mod mixer_common;

use mixer_common::{material, scene};

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn read_f32(path: &Path) -> Result<Vec<f32>, String> {
    let b = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect())
}

fn write_f32(path: &Path, v: &[f32]) -> Result<(), String> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for x in v {
        b.extend_from_slice(&x.to_le_bytes());
    }
    fs::write(path, b).map_err(|e| e.to_string())
}

/// Bit-identical count, max abs error and RMS error.
fn diff(a: &[f32], b: &[f32]) -> (usize, usize, f64, f64, Option<usize>) {
    let n = a.len().min(b.len());
    let mut same = 0;
    let mut max = 0.0f64;
    let mut sq = 0.0f64;
    let mut first = None;
    for i in 0..n {
        if a[i].to_bits() == b[i].to_bits() {
            same += 1;
        } else {
            first.get_or_insert(i);
            let d = (a[i] as f64 - b[i] as f64).abs();
            max = max.max(if d.is_nan() { f64::INFINITY } else { d });
            sq += d * d;
        }
    }
    (
        n,
        same,
        max,
        if n > 0 { (sq / n as f64).sqrt() } else { 0.0 },
        first,
    )
}

fn cmd_gen(dir: &Path) -> Result<(), String> {
    let dir = std::path::absolute(dir).map_err(|e| e.to_string())?;
    let fixtures = material::write_fixtures(&dir.join("fixtures")).map_err(|e| e.to_string())?;
    let scenes =
        material::write_scenes(&dir.join("scenes"), &fixtures).map_err(|e| e.to_string())?;
    println!(
        "{} fixtures, {} scenes in {}",
        fixtures.len(),
        scenes.len(),
        dir.display()
    );
    Ok(())
}

fn cmd_compare(oracle: &str, dir: &Path) -> Result<bool, String> {
    let scenes_dir = dir.join("scenes");
    let work = dir.join("work");
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let mut files: Vec<PathBuf> = fs::read_dir(&scenes_dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    files.sort();
    let mut all_ok = true;
    println!(
        "{:<26} {:>9} {:>9} {:>12} {:>12} {:>5}",
        "scene", "samples", "differ", "max_abs", "rms", "log"
    );
    for f in &files {
        let stem = f
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let c_out = work.join(format!("{stem}.c.f32"));
        let c_log = work.join(format!("{stem}.c.log"));
        let st = Command::new(oracle)
            .arg("scene")
            .arg(f)
            .arg(&c_out)
            .arg(&c_log)
            .status()
            .map_err(|e| e.to_string())?;
        if !st.success() {
            println!("{stem:<26} C oracle failed: {st}");
            all_ok = false;
            continue;
        }
        let script = fs::read_to_string(f).map_err(|e| e.to_string())?;
        let r = scene::run(&script, true)?;
        let c = read_f32(&c_out)?;
        let clog = fs::read_to_string(&c_log).map_err(|e| e.to_string())?;
        let (n, same, max, rms, first) = diff(&c, &r.samples);
        let log_ok = clog == r.log;
        let len_ok = c.len() == r.samples.len();
        let ok = same == n && len_ok && log_ok;
        all_ok &= ok;
        println!(
            "{stem:<26} {n:>9} {:>9} {max:>12.3e} {rms:>12.3e} {:>5}{}{}",
            n - same,
            if log_ok { "same" } else { "DIFF" },
            if len_ok {
                String::new()
            } else {
                format!(" length {} vs {}", c.len(), r.samples.len())
            },
            first.map(|i| format!(" first@{i}")).unwrap_or_default()
        );
        if !ok {
            write_f32(&work.join(format!("{stem}.rs.f32")), &r.samples)?;
            fs::write(work.join(format!("{stem}.rs.log")), &r.log).map_err(|e| e.to_string())?;
        }
    }
    Ok(all_ok)
}

/// Writes the C oracle's hashes of every scene (the crate's parity test
/// checks the Rust mixer against them without the oracle).
fn cmd_expected(oracle: &str, dir: &Path, out_file: &Path) -> Result<(), String> {
    let dir = std::path::absolute(dir).map_err(|e| e.to_string())?;
    let fixtures = material::write_fixtures(&dir.join("fixtures")).map_err(|e| e.to_string())?;
    let work = dir.join("expected-work");
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let mut text = String::from(
        "# scene rate channels samples fnv64(sample bits) fnv64(query log) sum(x^2) max|x|; generated by tools/soloud-oracle expected\n",
    );
    for &rate in &[44100u32, 48000] {
        for &ch in &[1u32, 2, 4, 6, 8] {
            for name in material::SCENES {
                let script = material::scene(name, rate, ch, &fixtures);
                let sp = work.join("scene.txt");
                fs::write(&sp, &script).map_err(|e| e.to_string())?;
                let (o, l) = (work.join("out.f32"), work.join("out.log"));
                let st = Command::new(oracle)
                    .arg("scene")
                    .arg(&sp)
                    .arg(&o)
                    .arg(&l)
                    .status()
                    .map_err(|e| e.to_string())?;
                if !st.success() {
                    return Err(format!("{name} {rate} {ch}: oracle failed {st}"));
                }
                let samples = read_f32(&o)?;
                let log = fs::read(&l).map_err(|e| e.to_string())?;
                let (energy, peak) = mixer_common::energy_peak(&samples);
                text.push_str(&format!(
                    "{name} {rate} {ch} {} {:016x} {:016x} {energy:e} {peak:e}\n",
                    samples.len(),
                    mixer_common::hash_samples(&samples),
                    mixer_common::fnv1a(log)
                ));
            }
        }
    }
    fs::write(out_file, text).map_err(|e| e.to_string())
}

/// Writes the C asset oracle's result for every .ogg under `fixtures`:
/// `relative-path size fnv64` or `relative-path error <name>`.
fn cmd_asset_expected(
    oracle: &str,
    fixtures: &Path,
    work: &Path,
    out_file: &Path,
) -> Result<(), String> {
    let mut files = Vec::new();
    collect_ogg(fixtures, &mut files);
    fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let mut text = String::from(
        "# path size fnv64(.dsf bytes) | path error <name>; generated by tools/soloud-oracle asset-expected\n",
    );
    for f in &files {
        let out = work.join("asset.out");
        let _ = fs::remove_file(&out);
        let st = Command::new(oracle)
            .arg(f)
            .arg(&out)
            .status()
            .map_err(|e| e.to_string())?;
        let c = fs::read(&out).unwrap_or_default();
        let rel = f
            .strip_prefix(fixtures)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/");
        match st.code() {
            Some(0) => text.push_str(&format!(
                "{rel} {} {:016x}\n",
                c.len(),
                mixer_common::fnv1a(c.iter().copied())
            )),
            Some(3) => text.push_str(&format!(
                "{rel} error {}\n",
                String::from_utf8_lossy(&c).trim()
            )),
            other => return Err(format!("{}: oracle exit {other:?}", f.display())),
        }
    }
    fs::write(out_file, text).map_err(|e| e.to_string())
}

fn cmd_bench(scene_path: &Path, reps: usize) -> Result<(), String> {
    let script = fs::read_to_string(scene_path).map_err(|e| e.to_string())?;
    let mut best = f64::INFINITY;
    for _ in 0..reps {
        let r = scene::run(&script, false)?;
        best = best.min(r.mix_ns / r.mix_calls.max(1) as f64);
    }
    println!("{best:.1}");
    Ok(())
}

/// WAV variants for the reader comparison.
fn wav_corpus(dir: &Path) -> Result<Vec<PathBuf>, String> {
    use material::Enc;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let encs = [
        ("u8", Enc::U8),
        ("s16", Enc::S16),
        ("s24", Enc::S24),
        ("s32", Enc::S32),
        ("f32", Enc::F32),
        ("f64", Enc::F64),
        ("alaw", Enc::Alaw),
        ("mulaw", Enc::Mulaw),
        ("ext16", Enc::Ext16),
    ];
    let mut seed = 100;
    for (name, enc) in encs {
        for ch in [1u16, 2, 3, 6, 8] {
            for &(frames, rate) in &[
                (1usize, 44100u32),
                (511, 22050),
                (512, 44100),
                (513, 48000),
                (3001, 11025),
                (44_100, 44100),
            ] {
                for extra in [false, true] {
                    seed += 1;
                    let mut sig = material::signal(
                        frames,
                        ch as usize,
                        rate as f64,
                        300.0 + seed as f64,
                        0.9,
                        0.3,
                        seed,
                    );
                    if seed % 5 == 0 {
                        // Full-scale and over-range values.
                        for (i, s) in sig.iter_mut().enumerate() {
                            if i % 17 == 0 {
                                *s = if i % 2 == 0 { 1.0 } else { -1.0 };
                            }
                        }
                    }
                    let p = dir.join(format!(
                        "{name}_{ch}ch_{frames}_{rate}{}.wav",
                        if extra { "_x" } else { "" }
                    ));
                    fs::write(&p, material::wav_bytes(&sig, ch, rate, enc, extra))
                        .map_err(|e| e.to_string())?;
                    out.push(p);
                }
            }
        }
    }
    // Header oddities.
    let base = material::wav_bytes(
        &material::signal(1000, 2, 44100.0, 400.0, 0.5, 0.1, 9),
        2,
        44100,
        Enc::S16,
        false,
    );
    let mut odd = |name: &str, bytes: Vec<u8>| -> Result<(), String> {
        let p = dir.join(name);
        fs::write(&p, bytes).map_err(|e| e.to_string())?;
        out.push(p);
        Ok(())
    };
    odd("not_riff.wav", b"OggS....".to_vec())?;
    odd("empty.wav", Vec::new())?;
    odd("riff_small_size.wav", {
        let mut b = base.clone();
        b[4..8].copy_from_slice(&20u32.to_le_bytes());
        b
    })?;
    odd("no_data.wav", base[..36].to_vec())?;
    odd("zero_rate.wav", {
        let mut b = base.clone();
        b[24..28].copy_from_slice(&0u32.to_le_bytes());
        b
    })?;
    odd("zero_channels.wav", {
        let mut b = base.clone();
        b[22..24].copy_from_slice(&0u16.to_le_bytes());
        b
    })?;
    odd("adpcm_tag.wav", {
        let mut b = base.clone();
        b[20..22].copy_from_slice(&2u16.to_le_bytes());
        b
    })?;
    odd("data_size_zero.wav", {
        let mut b = base.clone();
        b[40..44].copy_from_slice(&0u32.to_le_bytes());
        b
    })?;
    odd("fmt18.wav", {
        // fmt chunk of 18 bytes (cbSize 0), as many writers emit.
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&18u32.to_le_bytes());
        b.extend_from_slice(&base[20..36]);
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&base[36..]);
        let n = (b.len() - 8) as u32;
        b[4..8].copy_from_slice(&n.to_le_bytes());
        b
    })?;
    odd("bits12.wav", {
        let mut b = base.clone();
        b[34..36].copy_from_slice(&12u16.to_le_bytes());
        b
    })?;
    odd("fact_chunk.wav", {
        let mut b = Vec::new();
        b.extend_from_slice(&base[..36]);
        b.extend_from_slice(b"fact");
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&1000u32.to_le_bytes());
        b.extend_from_slice(&base[36..]);
        let n = (b.len() - 8) as u32;
        b[4..8].copy_from_slice(&n.to_le_bytes());
        b
    })?;
    Ok(out)
}

fn cmd_wav_compare(oracle: &str, dir: &Path) -> Result<bool, String> {
    let files = wav_corpus(&dir.join("wavs"))?;
    let work = dir.join("wavwork");
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let mut ok = 0;
    let mut bad = 0;
    for f in &files {
        let out = work.join(format!(
            "{}.bin",
            f.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        ));
        let st = Command::new(oracle)
            .arg("wavdump")
            .arg(f)
            .arg(&out)
            .status()
            .map_err(|e| e.to_string())?;
        if !st.success() {
            println!("{}: oracle failed {st}", f.display());
            bad += 1;
            continue;
        }
        let b = fs::read(&out).map_err(|e| e.to_string())?;
        let res = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let bytes = fs::read(f).map_err(|e| e.to_string())?;
        let rust = vkstorm_audio::wav::decode(&bytes);
        let same = match (&rust, res) {
            (Err(_), r) if r != 0 => true,
            (Ok(d), 0) => {
                let ch = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                let count = u32::from_le_bytes([b[8], b[9], b[10], b[11]]);
                let rate = f32::from_le_bytes([b[12], b[13], b[14], b[15]]);
                let data: Vec<f32> = b[16..]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes(*c))
                    .collect();
                ch == d.channels
                    && count == d.sample_count
                    && rate.to_bits() == (d.sample_rate as f32).to_bits()
                    && data.len() == d.data.len()
                    && data
                        .iter()
                        .zip(&d.data)
                        .all(|(a, b)| a.to_bits() == b.to_bits())
            }
            _ => false,
        };
        if same {
            ok += 1;
        } else {
            bad += 1;
            println!(
                "DIFF {}: soloud result {res}, rust {:?}",
                f.display(),
                rust.as_ref().map(|d| (d.channels, d.sample_count))
            );
        }
    }
    println!(
        "wav-compare: {ok} identical, {bad} differ ({} files)",
        files.len()
    );
    Ok(bad == 0)
}

fn collect_ogg(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = fs::read_dir(dir) {
        let mut v: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        v.sort();
        for p in v {
            if p.is_dir() {
                collect_ogg(&p, out);
            } else if p.extension().is_some_and(|x| x == "ogg") {
                out.push(p);
            }
        }
    }
}

fn cmd_asset_compare(oracle: &str, fixtures: &Path, work: &Path) -> Result<bool, String> {
    let mut files = Vec::new();
    collect_ogg(fixtures, &mut files);
    fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let mut ok = 0;
    let mut bad = 0;
    let mut decoded = 0;
    for f in &files {
        let out = work.join("asset.out");
        let _ = fs::remove_file(&out);
        let st = Command::new(oracle)
            .arg(f)
            .arg(&out)
            .status()
            .map_err(|e| e.to_string())?;
        // The oracle exits 0 with the WAV written, or 3 with the error line.
        let bytes = fs::read(f).map_err(|e| e.to_string())?;
        let rust = vkstorm_audio::asset::decode_sound_asset(&bytes);
        let c = fs::read(&out).unwrap_or_default();
        let same = match (&rust, st.code()) {
            (Ok(w), Some(0)) => *w == c,
            (Err(e), Some(3)) => {
                let text = String::from_utf8_lossy(&c);
                text.trim() == e.code_name()
            }
            _ => false,
        };
        if same {
            ok += 1;
            if rust.is_ok() {
                decoded += 1;
            }
        } else {
            bad += 1;
            println!(
                "DIFF {}: C exit {:?} ({} bytes: {}), rust {:?}",
                f.display(),
                st.code(),
                c.len(),
                String::from_utf8_lossy(&c[..c.len().min(60)]),
                rust.as_ref().map(|w| w.len())
            );
        }
    }
    println!(
        "asset-compare: {ok} identical ({decoded} decoded, {} rejected alike), {bad} differ",
        ok - decoded
    );
    Ok(bad == 0)
}

fn cmd_asset_bench(list: &Path, reps: usize) -> Result<(), String> {
    let text = fs::read_to_string(list).map_err(|e| e.to_string())?;
    let files: Vec<Vec<u8>> = text
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| fs::read(l).unwrap_or_default())
        .collect();
    let mut best = f64::INFINITY;
    for _ in 0..reps {
        let t0 = Instant::now();
        let mut total = 0usize;
        for f in &files {
            if let Ok(w) = vkstorm_audio::asset::decode_sound_asset(f) {
                total += w.len();
            }
        }
        let dt = t0.elapsed().as_secs_f64() * 1000.0;
        std::hint::black_box(total);
        best = best.min(dt);
    }
    println!("{best:.3}");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r: Result<bool, String> = (|| match args.get(1).map(String::as_str) {
        Some("gen") if args.len() >= 3 => cmd_gen(Path::new(&args[2])).map(|_| true),
        Some("scene") if args.len() >= 5 => {
            let script = fs::read_to_string(&args[2]).map_err(|e| e.to_string())?;
            let out = scene::run(&script, true)?;
            write_f32(Path::new(&args[3]), &out.samples)?;
            fs::write(&args[4], out.log).map_err(|e| e.to_string())?;
            Ok(true)
        }
        Some("compare") if args.len() >= 4 => cmd_compare(&args[2], Path::new(&args[3])),
        Some("bench") if args.len() >= 4 => {
            cmd_bench(Path::new(&args[2]), args[3].parse().map_err(|_| "reps")?).map(|_| true)
        }
        Some("bench-scene") if args.len() >= 7 => {
            let dir = std::path::absolute(Path::new(&args[2])).map_err(|e| e.to_string())?;
            let fixtures =
                material::write_fixtures(&dir.join("fixtures")).map_err(|e| e.to_string())?;
            let rate: u32 = args[3].parse().map_err(|_| "rate")?;
            let ch: u32 = args[4].parse().map_err(|_| "channels")?;
            let periods: u32 = args[5].parse().map_err(|_| "periods")?;
            fs::write(
                &args[6],
                material::bench_scene(rate, ch, &fixtures, periods),
            )
            .map_err(|e| e.to_string())?;
            Ok(true)
        }
        Some("expected") if args.len() >= 5 => {
            cmd_expected(&args[2], Path::new(&args[3]), Path::new(&args[4])).map(|_| true)
        }
        Some("asset-expected") if args.len() >= 6 => cmd_asset_expected(
            &args[2],
            Path::new(&args[3]),
            Path::new(&args[4]),
            Path::new(&args[5]),
        )
        .map(|_| true),
        Some("wav-compare") if args.len() >= 4 => cmd_wav_compare(&args[2], Path::new(&args[3])),
        Some("asset-compare") if args.len() >= 5 => {
            cmd_asset_compare(&args[2], Path::new(&args[3]), Path::new(&args[4]))
        }
        Some("asset-bench") if args.len() >= 4 => {
            cmd_asset_bench(Path::new(&args[2]), args[3].parse().map_err(|_| "reps")?).map(|_| true)
        }
        _ => Err("usage: see the module documentation".into()),
    })();
    match r {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    }
}
