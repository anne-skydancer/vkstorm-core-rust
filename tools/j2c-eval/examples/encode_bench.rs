//! Time the vkstorm encoder the way `LLImageJ2COJ::encodeImpl` runs it, on the
//! synthetic pattern shared with tools/j2c-eval/c/opj_enc.c (which times the
//! same work with OPJ_ENC_ITERS set). Prints `<bytes> <best_ms>`.
//! usage: encode_bench <w> <h> <c> <reversible 0|1> <iters> [out.j2c]
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (w, h, c): (u32, u32, u32) = (a[1].parse().unwrap(), a[2].parse().unwrap(), a[3].parse().unwrap());
    let rev = a[4] == "1";
    let iters: usize = a[5].parse().unwrap();
    let mut px = vec![0u8; (w * h * c) as usize];
    for y in 0..h { for x in 0..w { for k in 0..c { px[((y * w + x) * c + k) as usize] = ((x * 7 + y * 3 + k * 50) % 256) as u8; } } }
    let mut best = f64::MAX;
    let mut out = Vec::new();
    for _ in 0..iters {
        let t0 = std::time::Instant::now();
        out = vkstorm_j2c::encode(&px, w, h, c, Some("no comment"), rev).expect("encode");
        best = best.min(t0.elapsed().as_secs_f64() * 1000.0);
    }
    println!("{} {:.3}", out.len(), best);
    if let Some(o) = a.get(6) { std::fs::write(o, &out).unwrap(); }
}
