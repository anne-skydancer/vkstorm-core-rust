# j2c-eval: JPEG 2000 decoder evaluation harness

Compares the Vulkanstorm OpenJPEG fork (CPU build) against the pure-Rust
decoders `hayro-jpeg2000` and `j2k`, decoding the way the Second Life viewer
does: J2K codestream, lenient mode, resolution reduction by discard level, and
truncated input from progressive fetches.

Results and conclusions are in
the vkstorm-aleph study, [docs/upstream-study/07-j2c-decoder-evaluation.md](https://github.com/anne-skydancer/vkstorm-aleph/blob/main/docs/upstream-study/07-j2c-decoder-evaluation.md).

## Layout

- `src/main.rs`: Rust harness. Decode subcommands `vks` (this repository's
  codec, `vkstorm-j2c` on `vkstorm-openjp2`, including the conversion to the
  viewer's 8-bit layout), `vksdec` (the codec alone, which is what
  `c/opj_bench.c` times, so use it for like-for-like ratios), `openjp2` (the
  upstream crate the port started from, via `jpeg2k`), `hayro`, `j2k`
  (single-threaded) and `j2k-auto` (crate-chosen parallelism), plus `compare`
  for raw output diffs (max abs, mean abs, PSNR). `vksraw` drives the port
  through its raw API with `c/opj_bench.c`'s argument layout and modes
  (`src/vksraw.rs`), for the paths the viewer API never reaches.
- `examples/encode_bench.rs`: times `vkstorm_j2c::encode` on the synthetic
  pattern shared with `c/opj_enc.c`, which times the same work when
  `OPJ_ENC_ITERS=<n>` is set; both print `<bytes> <best_ms>` and write the
  codestream, so speed and byte parity come from one run per case.
- `c/opj_bench.c`: C harness that mirrors `LLImageJ2COJ::decode` (memory stream
  truncated to `max_bytes`, `opj_decoder_set_strict_mode(false)`,
  `opj_set_decoded_resolution_factor`). An optional sixth argument selects
  `tiles` (every tile in turn through `opj_get_decoded_tile` on one codec,
  tiles written back to back) or `area=x0,y0,x1,y1` (`opj_set_decode_area`
  before `opj_decode`); `-` as the output path writes nothing.
- `c/opj_enc.c`: C harness that mirrors `LLImageJ2COJ::encodeImpl` (J2K, RLCP,
  layered rates, resolution rule under 64 px, COM comment); its output is the
  byte-parity oracle for the Rust encoder (`examples/encode_probe.rs` produces
  the matching input).
- `run.sh`: runs every decoder over a corpus directory and all scenarios,
  writes a CSV.
- `scripts/gen_corpus.py`, `scripts/mosaic.py`: synthetic source images.
- `scripts/baseline_fixtures.py`: the codestreams in
  `crates/vkstorm-openjp2/tests/fixtures` (tiles with TLM, interleaved
  tile-parts, MCO, SOP/EPH, Cinema 2K and their crafted variants), from the
  fork's `opj_compress` at the baseline commit.
- `patches/hayro-jpeg2000-0.4.1-truncated-tilepart.diff`: the change that lets
  hayro decode truncated tile-parts in non-strict mode.
- `results/`: CSVs from recorded runs, named by date and CPU.

## Reproduce

1. Build the fork's OpenJPEG twice (clang + ninja on Windows, gcc on
   Linux), GPU backends off, at the commit the port corresponds to
   (`vkstorm_openjp2::UPSTREAM_BASELINE.fork`; the release viewer's
   3p-openjpeg tag `v2.5.4-cpu-avx2.1` points at it):

   ```sh
   git clone --recurse-submodules https://github.com/anne-skydancer/3p-openjpeg.git
   cd 3p-openjpeg && git -C openjpeg checkout 3b1a6a96 && mkdir build-avx2 && cd build-avx2
   cmake -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_C_COMPILER=clang \
     -DBUILD_SHARED_LIBS=OFF -DBUILD_CODEC=ON -DBUILD_TESTING=OFF -DBUILD_THIRDPARTY=ON \
     -DOPJ_ENABLE_CUDA=OFF -DOPJ_ENABLE_OPENCL=OFF -DOPJ_PRECISE_FP=ON \
     -DCMAKE_C_FLAGS="-O2 -mavx2 -ffp-contract=off" ../openjpeg && ninja
   ```

   Repeat as `build-sse2` with `-DCMAKE_C_FLAGS="-O2 -ffp-contract=off"` and
   `-DBUILD_CODEC=OFF`.

2. Compile the C harness against each library:

   ```sh
   cd tools/j2c-eval/c
   clang -O2 -DOPJ_STATIC -I<fork>/build-avx2/src/lib/openjp2 -I<fork>/openjpeg/src/lib/openjp2 \
     opj_bench.c <fork>/build-avx2/bin/libopenjp2.a -o opj_bench_avx2.exe
   clang -O2 -DOPJ_STATIC -I<fork>/build-sse2/src/lib/openjp2 -I<fork>/openjpeg/src/lib/openjp2 \
     opj_bench.c <fork>/build-sse2/bin/libopenjp2.a -o opj_bench_sse2.exe
   ```

3. Build the Rust harness from the workspace root: `cargo build --release -p j2c-eval`
   (the workspace `.cargo/config.toml` sets `target-cpu=x86-64-v3`, the AVX2
   level, to match the C build). The binary lands in `target/release/` at the
   workspace root.

4. Make a corpus. Synthetic sources:

   ```sh
   python -I scripts/gen_corpus.py <corpus>/src          # synth_256..2048.ppm
   python -I scripts/mosaic.py <photo.ppm> <corpus>/src/landmosaic_1024.ppm 1024
   ```

   Encode with the viewer's rules (`LLImageJ2COJ::encodeImpl`): J2K, RLCP,
   irreversible, layers by surface area (2 up to 32², 3 up to 128², 4 up to
   512², 5 up to 1024², 6 above) with rates `8 * 4^i`, for example a 1024²
   image is `-r 2048,512,128,32,8 -I -p RLCP`:

   ```sh
   opj_compress -i synth_1024.ppm -o synth_1024.j2k -r 2048,512,128,32,8 -I -p RLCP
   mv synth_1024.j2k synth_1024.j2c
   ```

   Real Second Life textures: the viewer skins ship 21 unique `.j2c` files
   under `indra/newview/skins/*/textures/` (Linden and Firestorm artwork, not
   redistributed here).

5. Run: `./run.sh <corpus_dir> results/<date>-<cpu>.csv 5`.

## Parity with the fork on the baseline fixtures

For every fixture, every scenario (reduce 0 to 3, truncated input, tile by
tile, decode areas) must give `max_abs 0` between the two harnesses, or
fail on both sides:

```sh
C=c/opj_bench_avx2.exe; R=../../target/release/j2c-eval.exe; f=<fixture>
$C $f 0 0 1 c.raw tiles; $R vksraw $f 0 0 1 r.raw tiles; $R compare r.raw c.raw
```

`tools/j2c-eval/results/` keeps the recorded sweeps. To regenerate the
fixtures after an upstream change, run `scripts/baseline_fixtures.py` with
the fork's `opj_compress` built at the new commit, then write the
`.opj.raw` references with `c/opj_bench` (`<name>.r0.opj.raw` is the whole
decode at reduce 0, `tlm_tiles_256.tiles.opj.raw` the `tiles` mode).

Scenarios in `run.sh`: full decode at reduce 0 to 3, and truncated input at
25% of bytes for reduce 0, 50% for reduce 1, 25% for reduce 2, 12% for
reduce 3. The OpenJPEG AVX2 output is the reference for the PSNR columns.
