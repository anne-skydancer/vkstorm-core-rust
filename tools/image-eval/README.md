# image-eval: raster codec parity and timing harness

Measures `crates/vkstorm-image` (PNG, TGA, BMP, DXT and the JPEG candidate)
against the viewer's C++ codecs, like for like: the same inputs, the viewer's
prebuilt libraries (libpng 1.6.44 on zlib-ng 2.2.3, libjpeg-turbo 3.0.4),
MSVC `/O2 /arch:AVX2` on the C++ side and the cargo release profile with
`target-cpu=x86-64-v3` on the Rust side, one thread, best and mean of N runs.

The conclusions are in the viewer's `doc/rust_port.md` (rung two).

## Layout

- `c/image_oracle.cpp`: the viewer's codecs, standalone. `LLPngWrapper` and
  `LLImageJPEG` verbatim on the prebuilt libraries; `LLImageTGA`,
  `LLImageBMP`, `LLImageDXT` and `LLImageBase::generateMip` copied with a
  minimal `LLImageRaw` stand-in. Commands: `<codec>-decode`, `<codec>-encode`,
  `<codec>-header`, `tga-process`, `jpeg-dims`, `crc`, `crc-list`, `bench`.
  It is the oracle for the crate's fixtures and the C++ side of the benchmark.
- `src/main.rs`: the Rust side with the same commands (`bench`, `crc-list`,
  `compare`, `<codec>-decode`, `<codec>-encode`).
- `scripts/gen_sources.py`: deterministic source images (raw rasters, PNG
  variants of every colour type and bit depth with tRNS, gAMA and sBIT, TGA
  variants of every image type, pixel size, origin and colour map, BMP
  variants); `--big` adds the 2048x2048 benchmark rasters.
- `scripts/make_fixtures.py`: builds `crates/vkstorm-image/tests/fixtures`
  with the oracle: decodes every source, encodes every raster, runs the
  alpha-gradient processing, copies a small real texture of every variant
  class found in the viewer, writes `manifest.txt`.
- `scripts/run_bench.py`: runs both harnesses over the benchmark rasters and
  writes a CSV.
- `scripts/png_reencode_survey.py`: re-encodes real PNG textures with both
  encoders and reports byte identity and size ratios.
- `results/`: CSVs from recorded runs, named by date and CPU.

## Reproduce

1. Configure the viewer once so `build-vc170-64/packages` holds the prebuilt
   libraries, then compile the oracle with MSVC from a Visual Studio x64
   prompt (`P` is the packages directory):

   ```sh
   cl -nologo -O2 -arch:AVX2 -EHs -std:c++17 -MD -DWIN32 \
     -I$P/include/libpng16 -I$P/include/jpeglib -I$P/include/zlib-ng \
     image_oracle.cpp -Fe:image_oracle.exe \
     -link $P/lib/release/libpng16.lib $P/lib/release/jpeg.lib $P/lib/release/zlib.lib
   ```

2. Build the Rust harness: `cargo build --release -p image-eval` (the
   workspace `.cargo/config.toml` sets `target-cpu=x86-64-v3`).

3. Fixtures: `python -I scripts/make_fixtures.py c/image_oracle.exe <viewer>/indra ../../crates/vkstorm-image/tests/fixtures`,
   then `cargo test -p vkstorm-image --features jpeg`.

4. Benchmark: `python -I scripts/gen_sources.py <dir> --big`, then
   `python -I scripts/run_bench.py c/image_oracle.exe ../../target/release/image-eval.exe <dir> results/<date>-<cpu>.csv 20`
   on an otherwise idle machine.

5. Whole-skin parity: list every PNG/TGA under the viewer's skins, run
   `image_oracle.exe crc-list <list>` and `image-eval crc-list <list>` and
   diff the outputs.
