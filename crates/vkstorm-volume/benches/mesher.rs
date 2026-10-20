//! Timing of the full build of every corpus case (path, profile, sweep, all
//! faces with their buffers, tangents), single-threaded, measured the same
//! way as the viewer's `INTEGRATION_TEST_llvolume` bench mode so the two can
//! be compared like for like. Run with `cargo bench --bench mesher`;
//! `VKS_VOLUME_BENCH` sets the iteration count (default 20) and
//! `VKS_VOLUME_BENCH_OUT` a CSV path (default: stdout).

#[path = "../tests/common/mod.rs"]
mod common;

use std::io::Write;
use std::time::Instant;

fn main() {
    let iterations: usize = std::env::var("VKS_VOLUME_BENCH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20)
        .max(1);
    let corpus = common::load_corpus();
    let mut out: Box<dyn Write> = match std::env::var("VKS_VOLUME_BENCH_OUT") {
        Ok(path) => Box::new(std::fs::File::create(path).expect("bench output file")),
        Err(_) => Box::new(std::io::stdout()),
    };
    writeln!(out, "name,shape,detail,vertices,triangles,min_us,median_us").unwrap();
    for c in &corpus.cases {
        let mut samples = Vec::with_capacity(iterations);
        let mut verts = 0usize;
        let mut tris = 0usize;
        for it in 0..=iterations {
            let t0 = Instant::now();
            let b = common::build_case(c, true);
            let dt = t0.elapsed();
            if it > 0 {
                samples.push(dt.as_secs_f64() * 1e6);
            }
            verts = b.faces.iter().map(|f| f.positions.len()).sum();
            tris = b.faces.iter().map(|f| f.indices.len() / 3).sum();
            std::hint::black_box(&b);
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let shape = c.name.split('/').next().unwrap_or("");
        writeln!(
            out,
            "{},{},{},{},{},{:.1},{:.1}",
            c.name,
            shape,
            c.detail,
            verts,
            tris,
            samples[0],
            samples[samples.len() / 2]
        )
        .unwrap();
    }
}
