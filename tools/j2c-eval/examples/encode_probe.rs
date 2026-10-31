//! Encode a synthetic image (or a raw raster) with the vkstorm codec, write the
//! codestream, and report the round-trip quality. The synthetic pattern matches
//! tools/j2c-eval/c/opj_enc.c so the two outputs can be compared byte for byte.
//! usage: encode_probe synth <w> <h> <c> <reversible 0|1> [out.j2c] [comment]
//!        encode_probe raw <in.raw> <w> <h> <c> <reversible> [out.j2c] [comment]
fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse = a
        .iter()
        .zip(b)
        .map(|(x, y)| {
            let e = f64::from(x.abs_diff(*y));
            e * e
        })
        .sum::<f64>()
        / a.len() as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let raw_mode = a[1] == "raw";
    let ai = if raw_mode { 3 } else { 2 };
    let (w, h, c): (u32, u32, u32) = (
        a[ai].parse().unwrap(),
        a[ai + 1].parse().unwrap(),
        a[ai + 2].parse().unwrap(),
    );
    let rev = a[ai + 3] == "1";
    let out = a.get(ai + 4).cloned();
    let comment = a.get(ai + 5).map(|s| s.as_str());
    let px: Vec<u8> = if raw_mode {
        std::fs::read(&a[2]).unwrap()
    } else {
        let mut v = vec![0u8; (w * h * c) as usize];
        for y in 0..h {
            for x in 0..w {
                for k in 0..c {
                    v[((y * w + x) * c + k) as usize] = ((x * 7 + y * 3 + k * 50) % 256) as u8;
                }
            }
        }
        v
    };
    match vkstorm_j2c::encode(&px, w, h, c, comment, rev) {
        Ok(b) => {
            let d = vkstorm_j2c::decode(&b, 0, 0, 0, 4).unwrap();
            println!(
                "ok {} bytes, decoded {}x{}x{}, psnr {:.2}",
                b.len(),
                d.width,
                d.height,
                d.channels,
                psnr(&px, &d.data)
            );
            if let Some(o) = out {
                std::fs::write(o, &b).unwrap();
            }
        }
        Err(e) => println!("err {e}"),
    }
}
