# vorbis-oracle: Ogg Vorbis parity and timing harness

Compares `crates/vkstorm-vorbis` with the C libraries the viewer decodes
sound assets with: libogg 1.3.5 + libvorbis 1.3.7 + libvorbisfile from the
prebuilt secondlife `3p-ogg_vorbis` package v1.3.5-1.3.7-r2 on Windows
(MSVC 14.42, `/O2 /Ob2 /MD`, no `/arch`, `/fp:precise`), and the same
upstream sources built with that package's Linux flags on Linux
(`-m64 -O3 -g -fPIC -DPIC -DNDEBUG ...`, CMake Release, gcc 14 and 15).

Nothing here is built by `cargo build` of the workspace except the Rust
driver; the C programs are compiled on demand.

## Layout

- `c/vorbis_oracle.c`: the C reference. `file` decodes like
  `LLVorbisDecodeState` (`indra/llaudio/llaudiodecodemgr.cpp`):
  `ov_open_callbacks` over the file in memory with `LLFileSystem`'s
  read/seek/tell semantics, `ov_streams`/`ov_pcm_total`/`ov_info`/
  `ov_comment`/`ov_serialnumber` for every link, then the
  `ov_read(vf, buf, 4096, 0, 2, 1, &section)` loop; it writes a text
  trace with every return value (run-length coded), the stream information,
  the comments and the FNV-1a 64 hash of the PCM. Options vary the loop
  (`--keep-going` past `OV_HOLE`, `--word 1`, `--be`, `--unsigned`,
  `--len`). `packets` drives the packet API as a stream player does
  (`ogg_sync` in N-byte reads, a new decoder per BOS page, headers,
  `vorbis_synthesis` + `_blockin`, `_pcmout`/`_read`) and hashes the float
  samples bit for bit. `tables` dumps the MDCT trig and window tables.
  `bench` times the viewer's decode loop.
- `c/vorbis_corpus.c`: the corpus encoder (libvorbisenc; `--viewer`
  reproduces `LLVorbisEncode` exactly).
- `scripts/gen_corpus.py`: builds the test corpus (clean, unusual,
  damaged and synthetic streams) with `vorbis_corpus`; the synthetic
  streams have hand-built setup headers (floor 0, residue 0, many submaps,
  modes and codebook shapes libvorbisenc never writes) and random audio
  packets.
- `scripts/select_fixtures.py`: copies the committed fixture subset.
- `src/main.rs`: the Rust side with the same commands, plus `compare`
  (C oracle vs port over a corpus, every trace mode), `expected` (writes
  the C traces of the crate's committed fixtures) and `fuzz` (random
  mutations of corpus files, Rust only, reports panics). The trace code is
  shared with the crate's tests (`crates/vkstorm-vorbis/tests/trace`).

## Reproduce

1. C oracle and encoder on Windows, from a VS 2022 x64 prompt
   (`P` = `<viewer>\build-vc170-64\packages`):

   ```bat
   cl /nologo /O2 /MD /D_CRT_SECURE_NO_WARNINGS /D_CRT_NONSTDC_NO_DEPRECATE /I%P%\include c\vorbis_oracle.c /Fe:vorbis_oracle.exe /link /LIBPATH:%P%\lib\release libvorbisfile.lib libvorbis.lib libogg.lib
   cl /nologo /O2 /MD /D_CRT_SECURE_NO_WARNINGS /I%P%\include c\vorbis_corpus.c /Fe:vorbis_corpus.exe /link /LIBPATH:%P%\lib\release libvorbisenc.lib libvorbis.lib libogg.lib
   ```

   On Linux, build libogg 1.3.5/libvorbis 1.3.7 with the 3p package's
   `build-cmd.sh` flags into a prefix `L`, then
   `gcc -O2 -I$L/include c/vorbis_oracle.c -o vorbis_oracle $L/lib/release/libvorbisfile.a $L/lib/release/libvorbis.a $L/lib/release/libogg.a -lm`.

2. Corpus: `python -I scripts/gen_corpus.py vorbis_corpus.exe <corpus>`
   (`--quick` for about 60 files). The encoder output depends on the
   library build, so generate it with the Windows prebuilt encoder and copy
   it to Linux.

3. Compare: `cargo run --release -p vorbis-oracle -- compare vorbis_oracle.exe <corpus> <workdir>`
   (or the crate's opt-in test: `VORBIS_C_ORACLE=... VORBIS_CORPUS=... cargo test -p vkstorm-vorbis -- --ignored`).

4. Committed fixtures: `python -I scripts/select_fixtures.py <corpus> crates/vkstorm-vorbis/tests/fixtures`, then
   `vorbis-oracle expected <oracle> crates/vkstorm-vorbis/tests/fixtures crates/vkstorm-vorbis/tests/fixtures/expected-windows-msvc.txt`
   on Windows and `... expected-linux-x86_64.txt` on Linux; then
   `cargo test -p vkstorm-vorbis` checks them with no C involved.

5. Timing: build the driver with the viewer's release profile
   (`CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1`,
   the workspace's `target-cpu=x86-64-v3`) and run `bench <list> 3` of the
   C oracle and of `vorbis-oracle` alternately, three times each, keeping
   the minimum.

Packet-mode traces hash every NaN sample as one canonical NaN: the C and
the port produce NaN at the same samples (malformed floor 0 data), but the
sign bit of a NaN is not reproducible in Rust (LLVM may commute or rewrite
the float operations that propagate it). `VORBIS_ORACLE_DUMP=<file>` makes
both sides' `packets` command write the raw samples for inspection.

## Results

See the crate documentation (`crates/vkstorm-vorbis/src/lib.rs`) for the
platform notes; the measured parity and timing are in `results/`.
