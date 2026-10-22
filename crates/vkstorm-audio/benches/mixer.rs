//! Mix cost per 1024-frame block: 64 active 3D voices (mono 44.1 kHz
//! sources, moving every block), at 48 kHz with 2, 6 and 8 output channels,
//! through the adapter's call sequences (`tests/mixer_common`).
//!
//! `cargo bench -p vkstorm-audio --features link-curl --bench mixer`
//!
//! With `SOLOUD_ORACLE=<path to soloud_oracle>` the same scene also runs
//! through the viewer's SoLoud (tools/soloud-oracle), interleaved with the
//! Rust runs; each side reports the minimum of 3 runs of the mean time per
//! `mix` call. Only the `mix` calls are timed on either side (the 3D
//! updates between blocks are not).

#[path = "../tests/mixer_common/mod.rs"]
mod mixer_common;

use mixer_common::{material, scene};

fn main() {
    let dir = std::env::temp_dir().join(format!("vkstorm-audio-bench-{}", std::process::id()));
    let fixtures = material::write_fixtures(&dir.join("fixtures")).expect("fixtures");
    let oracle = std::env::var("SOLOUD_ORACLE").ok();
    let periods = 300;
    println!("64 moving 3D voices, 48 kHz, ns per 1024-frame mix (min of 3 interleaved runs)");
    println!(
        "{:>9} {:>12} {:>12} {:>8}",
        "channels", "rust", "soloud", "ratio"
    );
    for ch in [2u32, 6, 8] {
        let script = material::bench_scene(48000, ch, &fixtures, periods);
        let path = dir.join(format!("bench-{ch}.txt"));
        std::fs::write(&path, &script).expect("scene");
        let mut rust = f64::INFINITY;
        let mut soloud = f64::INFINITY;
        // Warm-up.
        let _ = scene::run(&script, false);
        for _ in 0..3 {
            if let Some(o) = &oracle {
                let out = std::process::Command::new(o)
                    .arg("bench")
                    .arg(&path)
                    .arg("1")
                    .output()
                    .expect("oracle");
                if let Ok(v) = String::from_utf8_lossy(&out.stdout).trim().parse::<f64>() {
                    soloud = soloud.min(v);
                }
            }
            let r = scene::run(&script, false).expect("scene");
            rust = rust.min(r.mix_ns / r.mix_calls as f64);
        }
        if soloud.is_finite() {
            println!("{ch:>9} {rust:>12.0} {soloud:>12.0} {:>8.3}", rust / soloud);
        } else {
            println!("{ch:>9} {rust:>12.0} {:>12} {:>8}", "-", "-");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
