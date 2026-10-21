//! Decode timing (harness = false): the viewer's `ov_read` loop over Ogg
//! Vorbis files held in memory, best of N runs per file.
//!
//! ```text
//! cargo bench -p vkstorm-vorbis --bench decode                 # the clean committed fixtures
//! cargo bench -p vkstorm-vorbis --bench decode -- a.ogg b.ogg  # given files
//! VORBIS_BENCH_REPS=10 cargo bench -p vkstorm-vorbis --bench decode
//! ```
//!
//! For the comparison with the C library, time the same files with
//! `tools/vorbis-oracle` (`vorbis-oracle bench` and the C oracle's
//! `bench`), interleaved, with the viewer's release profile (fat LTO,
//! `target-cpu=x86-64-v3`); see that tool's README.

use std::path::{Path, PathBuf};
use std::time::Instant;

use vkstorm_vorbis::VorbisFile;

/// Decodes the whole file to 16-bit PCM; returns (bytes, channels, rate).
fn decode(data: &[u8]) -> (u64, i32, i64) {
    let Ok(mut vf) = VorbisFile::open(data) else {
        return (0, 0, 0);
    };
    let (ch, rate) = vf.info(-1).map_or((0, 0), |i| (i.channels, i.rate));
    let mut buf = [0u8; 4096];
    let mut section = 0;
    let mut total = 0u64;
    loop {
        let r = vf.read(&mut buf, false, 2, true, &mut section);
        if r <= 0 {
            break;
        }
        total += r as u64;
    }
    (total, ch, rate)
}

fn fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/clean");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "ogg"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn main() {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .collect();
    let files: Vec<PathBuf> = if args.is_empty() {
        fixtures()
    } else {
        args.iter().map(PathBuf::from).collect()
    };
    let reps: usize = std::env::var("VORBIS_BENCH_REPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let inputs: Vec<(PathBuf, Vec<u8>)> = files
        .into_iter()
        .filter_map(|p| std::fs::read(&p).ok().map(|d| (p, d)))
        .collect();
    let mut best = vec![f64::INFINITY; inputs.len()];
    let mut best_total = f64::INFINITY;
    let mut info = vec![(0u64, 0i32, 0i64); inputs.len()];
    for _ in 0..reps {
        let mut total = 0.0;
        for (i, (_, d)) in inputs.iter().enumerate() {
            let t0 = Instant::now();
            info[i] = std::hint::black_box(decode(std::hint::black_box(d)));
            let dt = t0.elapsed().as_secs_f64();
            best[i] = best[i].min(dt);
            total += dt;
        }
        best_total = best_total.min(total);
    }
    let mut audio_s = 0.0;
    for (i, (p, _)) in inputs.iter().enumerate() {
        let (bytes, ch, rate) = info[i];
        let secs = if ch > 0 && rate > 0 {
            bytes as f64 / (2.0 * ch as f64 * rate as f64)
        } else {
            0.0
        };
        audio_s += secs;
        println!(
            "{:>10.3} ms  {:>8.1}x realtime  {}",
            best[i] * 1e3,
            if best[i] > 0.0 { secs / best[i] } else { 0.0 },
            p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned()
            )
        );
    }
    println!(
        "total {:.3} ms for {} files ({:.1} s of audio, {:.0}x realtime), best of {reps}",
        best_total * 1e3,
        inputs.len(),
        audio_s,
        if best_total > 0.0 {
            audio_s / best_total
        } else {
            0.0
        }
    );
}
