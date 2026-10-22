//! The mixer against the viewer's SoLoud, without the C oracle: every scene
//! of `mixer_common::material` (15 scenes x 44.1/48 kHz x 1/2/4/6/8
//! channels) is rendered through the Rust mixer with the adapter's call
//! sequences and hashed; the hashes must equal those of the SoLoud oracle
//! (`tools/soloud-oracle expected`: the viewer's 3p-soloud package with
//! MSVC on Windows, the same patched source with gcc 14/15 and glibc 2.43
//! on Linux), sample bits and query logs alike.
//!
//! The scripts and fixtures are generated without libm, so they are the
//! same bytes everywhere. The mixer's own libm calls are the wind
//! generator's `cosf`/`expf`/`sqrtf` and `cos`/`sin` of the constant 2-D
//! pan; the Rust port calls the platform's C runtime as SoLoud does, so it
//! matches SoLoud built on the same platform bit for bit. Between C runtimes
//! the wind can differ by an ulp of `cosf` (UCRT vs glibc 2.43: the 48 kHz
//! `winddecay` scenes, from 7.05 s on, max 3.9e-7, RMS 1.3e-8 at a peak of
//! 0.27), hence one list per platform. Should another C runtime (another
//! glibc, another Windows build) round some `cosf`/`expf` differently, only
//! scenes that carry the wind can change; for those, a hash mismatch is
//! accepted when the energy is within 1e-6 (relative) and the peak within
//! 2e-6 of SoLoud's and the query log is identical, and the test says so.
//! Every other scene must match exactly.

mod mixer_common;

use mixer_common::{energy_peak, fnv1a, hash_samples, material, scene};

#[cfg(windows)]
const EXPECTED: &str = include_str!("mixer_expected/scenes-windows-msvc.txt");
#[cfg(not(windows))]
const EXPECTED: &str = include_str!("mixer_expected/scenes-linux-gcc-glibc.txt");

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("vkstorm-audio-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

struct Case {
    name: String,
    rate: u32,
    ch: u32,
    len: usize,
    hash: u64,
    log: u64,
    energy: f64,
    peak: f64,
}

#[test]
fn scenes_match_soloud() {
    let dir = temp_dir("parity");
    let fixtures = material::write_fixtures(&dir.join("fixtures")).expect("fixtures");
    let cases: Vec<Case> = EXPECTED
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            Case {
                name: f[0].to_string(),
                rate: f[1].parse().unwrap(),
                ch: f[2].parse().unwrap(),
                len: f[3].parse().unwrap(),
                hash: u64::from_str_radix(f[4], 16).unwrap(),
                log: u64::from_str_radix(f[5], 16).unwrap(),
                energy: f[6].parse().unwrap(),
                peak: f[7].parse().unwrap(),
            }
        })
        .collect();
    assert_eq!(cases.len(), material::SCENES.len() * 10);
    let failures = std::sync::Mutex::new(Vec::new());
    let tolerated = std::sync::Mutex::new(Vec::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(16);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(c) = cases.get(i) else { break };
                let script = material::scene(&c.name, c.rate, c.ch, &fixtures);
                let out = scene::run(&script, true).expect("scene runs");
                let hash = hash_samples(&out.samples);
                let log = fnv1a(out.log.bytes());
                if out.samples.len() == c.len && hash == c.hash && log == c.log {
                    continue;
                }
                let (energy, peak) = energy_peak(&out.samples);
                let libm_dependent = script.contains("wind_on");
                let close = out.samples.len() == c.len
                    && log == c.log
                    && (energy - c.energy).abs() <= 1e-6 * c.energy.max(1e-12)
                    && (peak - c.peak).abs() <= 2e-6;
                let line = format!(
                    "{} {} {}: hash {hash:016x} vs {:016x}, energy {energy:e} vs {:e}, peak {peak:e} vs {:e}",
                    c.name, c.rate, c.ch, c.hash, c.energy, c.peak
                );
                if libm_dependent && close {
                    tolerated.lock().unwrap().push(line);
                } else {
                    failures.lock().unwrap().push(line);
                }
            });
        }
    });
    let _ = std::fs::remove_dir_all(&dir);
    for t in tolerated.into_inner().unwrap() {
        eprintln!("within the libm tolerance (not bit-identical): {t}");
    }
    let failures = failures.into_inner().unwrap();
    assert!(
        failures.is_empty(),
        "{} scenes differ from SoLoud:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
