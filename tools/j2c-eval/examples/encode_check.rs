fn main() {
    let j2c = std::fs::read("crates/vkstorm-j2c/tests/fixtures/synth_256.j2c").unwrap();
    let src = vkstorm_j2c::decode(&j2c, 0, 0, 0, 4).unwrap();
    let lossy = vkstorm_j2c::encode(
        &src.data,
        src.width,
        src.height,
        src.channels,
        Some("vkstorm encoder check"),
        false,
    )
    .unwrap();
    let lossless =
        vkstorm_j2c::encode(&src.data, src.width, src.height, src.channels, None, true).unwrap();
    std::fs::write("C:/Users/ANNESK~1/AppData/Local/Temp/claude/c--Dev-vulkanstorm/31a47609-92fa-4989-be81-1c70a9f6678a/scratchpad/enc/rust_lossy.j2c", &lossy).unwrap();
    std::fs::write("C:/Users/ANNESK~1/AppData/Local/Temp/claude/c--Dev-vulkanstorm/31a47609-92fa-4989-be81-1c70a9f6678a/scratchpad/enc/rust_lossless.j2c", &lossless).unwrap();
    std::fs::write("C:/Users/ANNESK~1/AppData/Local/Temp/claude/c--Dev-vulkanstorm/31a47609-92fa-4989-be81-1c70a9f6678a/scratchpad/enc/src.raw", &src.data).unwrap();
    println!(
        "lossy {} bytes, lossless {} bytes",
        lossy.len(),
        lossless.len()
    );
}
