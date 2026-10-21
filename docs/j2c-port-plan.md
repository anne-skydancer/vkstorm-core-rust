# JPEG 2000 codec: port of OpenJPEG, plan and status

Decision (2026-10-20): Vulkanstorm uses one Rust crate for JPEG 2000 encoding
and decoding, a port of OpenJPEG owned in this repository, with no runtime
choice of codec and no fallback in the viewer. The acceptance gates set for it:

1. **Performance at least equal to the Vulkanstorm OpenJPEG fork** (C, AVX2
   build) on the evaluation corpus, measured by `tools/j2c-eval` against the
   C harness, single-threaded like for like, and with the crate's own
   multithreading as an additional result.
2. **Small images are a first-class case, decoding and encoding.** UI
   textures, badges and icons are 16 to 256 px and decode thousands of times
   per session; small uploads (profile and classified pictures, snapshot
   thumbnails, baked-texture pieces) go through the encoder, which is why the
   viewer's encoder already has a special resolution rule under 64 px. The
   corpus includes the viewer's bundled skin textures, and the measurements
   report small images separately for both directions, since per-call setup
   cost dominates there.
3. **Output parity**: bit-identical decodes to the C fork at every discard
   level, truncated decodes behaving as OpenJPEG 2.5.4 does in non-strict
   mode, and uploads byte-identical to the viewer's current OpenJPEG encoder
   given the same parameters.

## Starting point

`crates/vkstorm-openjp2` is a copy of the `openjp2` crate 0.6.1 (c2rust
lineage, OpenJPEG 2.5.2, BSD-2-Clause). Measured on 2026-10-20 against the C
fork (Ryzen 9 9950X3D):

| Case | C fork | port | output |
|---|---|---|---|
| 2048² lossy full | 220 ms | 237 ms | identical |
| 1024² lossy full | 52 ms | 58 ms | identical |
| 1024² RGBA full | 82 ms | 88 ms | identical |
| 1024² lossless (5/3) | 12 ms | 54 ms | identical |
| discard 2, 2048² | 26 ms | 30 ms | identical |
| truncated input, any | decodes | fails | |

No SIMD (`std::arch` unused), thread setter stubbed, lenient mode broken.

## Work items

| # | Item | Gate it serves | Status |
|---|---|---|---|
| 1 | `vkstorm-j2c` API (metadata, decode, comment, encode) on the port; hayro and j2k removed from product code | parity | done 2026-10-20 |
| 2 | Lenient decode of truncated codestreams (ported the 2.5.4 non-strict return in `opj_j2k_decode_tile`, and the strict-mode setter) | parity | done 2026-10-20 |
| 3 | Encoder parameters mirroring `LLImageJ2COJ::encodeImpl`; byte-identical to the C encoder on fixtures | parity | done 2026-10-20: identical in 44/44 synthetic cases (8 to 512 px, 1/3/4 channels, lossy and lossless) and on the real raster, `tests/encode_parity.rs` |
| 4 | Profile the lossless 5/3 path (4x gap) and the general 10% gap; remove translation overhead (bounds checks in hot loops, indirection) | perf | done 2026-10-20: 5/3 wavelet (`dwt53.rs`), Tier-1 pass bodies specialised per block size, MQ coder registers (`MqRegs`) for decode and encode, per-sample bounds checks and debug logging out of the coding passes, tight 8-bit conversion; tables below |
| 5 | AVX2 wavelet (9/7 and 5/3) and MCT with `std::arch` plus runtime detection, mirroring upstream's intrinsics; SSE2 fallback | perf | done 2026-10-20 (`dwt53.rs`, `simd.rs`: 9/7 lifting, irreversible MCT, Tier-1 copy-out); forward 9/7 lifting is scalar in the fork too |
| 6 | Code-block thread pool for Tier-1 (upstream's design, `opj_thread_pool`), bounded by a caller-set worker count | perf | open |
| 7 | Small-image path: measure 16 to 256 px decodes and encodes, cut per-call allocation and setup, keep the under-64 px resolution rule | small images | measured 2026-10-20 (tables below): 8 to 256 px encodes at 0.62 to 1.00 of the C time on Windows, 0.80 to 1.08 on Linux (the 1.08 is 0.001 ms); 256 px decodes 0.88 to 1.02; no allocation work needed |
| 8 | Bring the port to the fork's 2.5.4-plus baseline (upstream diff 2.5.2 to 2.5.4+) | parity | done 2026-10-21: the port corresponds to fork commit 3b1a6a96 = upstream 8314119b (`UPSTREAM_BASELINE`); every CPU change in v2.5.2..8314119b ported, fixtures and tests for each, parity and timing recorded (section below) |
| 9 | Test suite: fixtures against the C fork at all discard levels and truncations; encode fixtures; fuzz-style garbage inputs | parity | decode and encode fixtures in place; truncation and garbage covered; fuzzing open |
| 10 | Incremental safety: convert hot modules from raw pointers to slices where it does not cost performance | maintainability | open |

## Port defects found and fixed

Found with the `guard-alloc` feature (canary-checked allocations) on
2026-10-20; all three caused heap corruption on Windows:

- `tcd.rs` `opj_tcd_code_block_dec_deallocate` and `opj_tcd_code_block_dec_allocate`
  deallocated `decoded_data` unconditionally, including null pointers with a
  stale layout (code-blocks never decoded: empty, truncated or discarded).
- `opj_cparameters` has a `Drop` that frees `cp_comment`, `cp_matrice` and
  `mct_data` with the port's free; C leaves these to the caller. `vkstorm-j2c`
  detaches its comment pointer after `opj_setup_encoder` has copied it.
- `opj_j2k_decode_tile` treated a short stream as an error in non-strict mode
  (OpenJPEG 2.5.4 returns what was decoded).

Found on 2026-10-20 through nondeterministic truncated decodes under
concurrency (`tools/j2c-eval/examples/thread_stress.rs`) and a mismatch
against the C fork on truncated input:

- `Stream::read` used `read_exact` and, when the stream ended mid-read, retried
  with a plain `read` that failed; the caller saw `(size_t)-1` although the
  bytes were already in its buffer, and `opj_j2k_read_sod` added that to the
  tile length, which wrapped. Tier-2 then read past the tile buffer into
  whatever followed on the heap. Now reads return the short count, and the
  tile-part read rejects a true end of stream like OpenJPEG 2.5.4.
- The 2.5.2 handling of a packet that runs past the available data left the
  code-block's segments counting passes it had no data for; later layers then
  appended chunks to a misaligned segment list. Ported the 2.5.3+ `corrupted`
  flag: the block decodes as zeros for the rest of the tile.
- With these, truncated decodes are bit-identical to the C fork at every cut
  point and discard level tried (`j2c-eval compare`, max_abs 0), and 2560
  concurrent truncated decodes gave 0 mismatches.

The `guard-alloc` feature now also poisons fresh `opj_malloc` memory (0xA5)
so uninitialised reads differ from a zeroed heap; `vkstorm-j2c` and `j2c-eval`
forward the feature.

## Upstream baseline (item 8)

Closed 2026-10-21 (core branch `codex/openjp2-baseline`). The port's CPU
decode and encode paths correspond to the Vulkanstorm OpenJPEG fork at
commit **3b1a6a96** (https://github.com/anne-skydancer/openjpeg, the
submodule of 3p-openjpeg tag `v2.5.4-cpu-avx2.1`, which the release viewer
ships), whose CPU code is upstream OpenJPEG commit **8314119b**
(`v2.5.4-29-g8314119b`, the merge of PR #1666 on 2026-09-04). Both hashes
are in `vkstorm_openjp2::UPSTREAM_BASELINE`.

The fork's own 14 commits over that upstream commit add the OpenCL and CUDA
backends (`opencl_backend.c`, `cuda_backend.c`, `accel_*.c`, the
`OPJ_HAVE_ACCEL_DECODE` / `OPJ_HAVE_OPENCL` hooks in `tcd.c`, an
`OPJ_CAPTURE_T1_REFERENCES` probe in `t1.c`) and the `OPJ_PRECISE_FP` CMake
option, which only adds `-fno-fast-math -ffp-contract=off` to the GNU/Clang
compile options. No fork commit changes a CPU decode or encode path, so the
baseline the port has to match is the upstream commit; the CPU AVX2 build
is upstream 8314119b compiled with those flags.

### Upstream commits v2.5.2..8314119b

73 non-merge commits. Classes: **ported** (this branch), **present** (already
in the port before it), **perf-only** (SIMD on x86 or ARM; the port has its
own `simd.rs`/`dwt53.rs` and is measured against the AVX2 build), **build**
(CMake, CI, packaging, release prep), **tools** (`src/bin`, `thirdparty`,
`bench_dwt`), **tests**, **docs** (comments, typos, README). Rust locations
are in `crates/vkstorm-openjp2/src`.

| upstream | subject | class | in the port |
|---|---|---|---|
| b0ce41d3, 0411252b, d369958f, e8b9d927, 1632f084, 761e49cb, a9fc407c, a332a1ab, bfa990d9, ab3d7eaa, 7c1246c9, e6497283, 4e3412d0, e2cf50f6, 210a8a56, 672ecbb1, eb25a5ec, 93c6e599, 917b2109, b3306cb1, 4a530300, 662ae8d0 | GitHub workflows, ABI check script, known-failure lists | build | n/a |
| 4b8b3970, c28f012d, b06eb2a8, d42966d0, a71509aa, 5e258319, 7b508bb0, a70b8f13 | OPJ_STATIC export, shared/static link refactor, cmake_minimum_required, pkg-config deps | build | n/a |
| 6e92b398, f173b559, 4bfe68eb | 2.5.3 and 2.5.4 release prep, CHANGELOG | build | `OPJ_VERSION` "2.5.4" (`openjpeg.rs:89`), so the default COM comment matches the fork's encoder |
| 2f267202, 92d9ce65, 060caa93 | thirdparty libtiff 4.6.0, libpng 1.6.43, zlib 1.3.1 | tools | n/a |
| 3beb2e1c, 995d8c96, 98592ee6, ebbd7a1f, f6ea97aa | line endings, OpenBSD big-endian in convert.c, sycc422_to_rgb bounds in color.c, opj_compress leak, bench_dwt assert | tools | n/a (not library code) |
| ce51a217, cc6d0c8d, 06bbae8b | typos in comments and messages, header docs, README "unmaintained" | docs | PTERM message text `t1.rs:2433`; the rest is comments |
| 1d4a883e, 206ede76 | non-regression tests for TLM files | tests | covered by `tests/upstream_baseline.rs` on our own TLM fixtures |
| 7d952024 | zero-length packet header in a non-zero-length packet is an error | ported | `t2.rs:1398` (`opj_t2_read_packet_header`) |
| 3b0c9b91 | `opj_j2k_decode_tiles`: honour `l_go_on` on the single-tile path | ported | `j2k.rs:12347` |
| 2c0823cb, c34e4412 | Psot smaller than the marker it contains is an error (#1530), only when Psot is non-zero | ported | `j2k.rs:9767` (`opj_j2k_read_tile_header`) |
| 2f5a299c | JP2 colr enumcs for CMYK (12) and e-YCC (24) | ported | `jp2.rs:1340-1349` (`opj_jp2_setup_encoder`); `tests/jp2_header.rs` |
| dea92eea | `opj_j2k_read_sod`: a read returning -1 is "Stream too short" (#1533) | present | `j2k.rs:4819` (ported 2026-10-20) |
| f3b28c5e | non-strict mode: `corrupted` code-blocks, no synthetic marker on empty chunks, skipped partial packet reports the whole length | present / ported | `t2.rs:1460,1500`, `t1.rs:2290,2297` (2026-10-20); `opj_t2_skip_packet_data` `t2.rs:1637` (this branch) |
| 954c6e3c | TLM markers index the tile-parts for `opj_get_decoded_tile`; index allocated in `read_siz`; lenient TLM parsing | ported | `opj_j2k_read_tlm` `j2k.rs:3058`, `opj_j2k_build_tp_index_from_tlm` `j2k.rs:8748` (called `j2k.rs:9069`), `opj_j2k_read_sot` `j2k.rs:4020`, `opj_j2k_read_siz` `j2k.rs:1280`, `opj_j2k_read_tile_header` `j2k.rs:9913`, `opj_j2k_decode_one_tile` `j2k.rs:12552`, `Drop` `j2k.rs:9266` |
| ed3a8183 | TLM index plans the tile-parts to visit when `opj_set_decode_area` covers a subset of tiles | ported | `opj_j2k_decode_tiles` `j2k.rs:12323` (planning, `12456` sort), `opj_j2k_read_tile_header` (seek at the top of the loop) |
| e492644f | `opj_j2k_add_tlmarker`: tile-part number must be inside the index | ported | `j2k.rs:8178` |
| 91d08b11 | `opj_j2k_read_sod`: only record tile-part positions inside the index | ported | `j2k.rs:4642` |
| 1bb9e915 | interleaved tile-parts with TNsot only in the last tile-part (#1558): seek to the first tile-part header, not the last SOT read | ported | `j2k.rs:12580` (`opj_j2k_decode_one_tile`); fixture `tp_interleaved_64` |
| 497d58c1, 2be5f148 | skip the TPsot==TNsot probe when a tile declares one tile-part; strict mode disables the probe | ported | `j2k.rs:9985`, `opj_j2k_decoder_set_strict_mode` `j2k.rs:6570`; fixture `tnsot_one_256` |
| c58bc128 | EPH markers are required when announced | ported | `t2.rs:1144,1384`; fixtures `eph_sop_256`, `eph_missing_256` |
| 40707fd5 | Cinema 2K encodes with one guard bit | ported | `j2k.rs:8054` (`opj_j2k_setup_encoder`); fixture `cinema2k_256` byte-identical |
| f809b80c | `opj_jp2_read_header`: colour space only when the codestream header parsed | ported | `jp2.rs:1969` |
| 578c67b3 | ICC profile copied into the JP2 header on write | ported | `jp2.rs:1334`; `tests/jp2_header.rs` |
| 839936aa | `opj_pi_initialise_encode`: include-table size overflow guard | ported | `pi.rs:2303` |
| 91966314 | MCO marker with zero stages keeps the DC level shift | ported | `j2k.rs:6252` (`opj_j2k_read_mco`); fixture `mco_zero_256` |
| e0e0c802 | AVX2/AVX-512 in the 5/3 inverse wavelet, the Tier-1 decode copy-out and the encoder's zigzag transpose | perf-only | the port's own `dwt53.rs` and `simd.rs` (`half_i32_avx2` `simd.rs:31`) cover the decode side; the encoder transpose is scalar in the port and the encode table is at parity |
| 0186472b, 0fc6e243 | NEON 5/3 and 9/7 inverse wavelet | perf-only | ARM only; the port targets x86-64-v3 |
| 6f31fc6f, a6ec867c, 94f42939, 6d52531c | unused `opj_mqc_renorme`, `l_img_comp`, `l_tccp` removed | docs (dead code) | the port's `mqc.rs` has no such function; the unused locals remain as c2rust left them (`tcd.rs:2623`, `pi.rs:1961`, `pi.rs:2234`), no behaviour |

Not ported, stated plainly: the `OJP_DO_NOT_DISPLAY_TILE_INDEX_IF_TLM`
environment check in `opj_j2k_dump_MH_index` (part of 954c6e3c) exists to
keep upstream's `opj_dump` test expectations stable and has no decode effect.

### Fixtures added

`crates/vkstorm-openjp2/tests/fixtures`, made by
`tools/j2c-eval/scripts/baseline_fixtures.py` with the fork's `opj_compress`
at 3b1a6a96 (1.3 MB with the C references):

| fixture | exercises |
|---|---|
| `tlm_tiles_256` (4x4 tiles, one tile-part per resolution, TLM) | TLM table and tile-part index; tile-by-tile decode seeks; decode-area planning |
| `tlm_invalid_256` (Stlm ST = 3) | lenient TLM parsing: warning, table dropped, same output |
| `tp_interleaved_64` (2x2 tiles, tile-parts interleaved, TNsot in the last tile-part only) | upstream #1558 |
| `tnsot_one_256` (one tile, four tile-parts, every SOT says TNsot 1) | the skipped TPsot==TNsot probe (497d58c1) |
| `mco_zero_256` (`plain_256` plus an MCO of zero stages) | 91966314; must decode identically to `plain_256` |
| `eph_sop_256`, `eph_missing_256` | EPH accepted; missing EPH fails on both sides |
| `cinema2k_256` (12-bit RGB, `-cinema2K 24`) | one guard bit; the port's encode is byte-identical |

Before this branch the port differed on them (old harness, whole decode at
discard 0 against the C fork): `mco_zero_256` max_abs 128, `tnsot_one_256`
max_abs 176, `tlm_invalid_256` failed in the port and decoded in the fork,
`eph_missing_256` decoded in the port and failed in the fork.

### Tracking upstream

1. `git -C <fork clone> log --oneline <UPSTREAM_BASELINE.upstream>..<new upstream> -- src/lib/openjp2`
   and classify each commit as above; note any fork commit that touches a
   CPU path (`git diff <upstream base> <fork commit> -- src/lib/openjp2` minus
   the GPU files).
2. Port the CPU decode and encode changes function by function into the
   mirroring Rust file, keeping the C control flow.
3. Build the fork at the new commit (`tools/j2c-eval/README.md`, step 1),
   rebuild the harnesses, and add fixtures that trigger each change
   (`scripts/baseline_fixtures.py`, then the `.opj.raw` references).
4. Gates: parity (`max_abs 0` for every fixture at every discard level, on
   truncated input, tile by tile and with decode areas; encodes
   byte-identical) and performance (the decode and encode tables below, both
   platforms, at or above the fork).
5. Update `UPSTREAM_BASELINE`, this table, and the results.

## Measurement

`tools/j2c-eval` holds the C harness (`c/opj_bench.c`) built against the fork,
the Rust harness with modes for this crate and for the comparison decoders,
and `run.sh` producing the CSV. Recorded results live in `results/`.

The C harness times `opj_decode` alone, so the like-for-like decode mode is
`vksdec` (codec only); `vks` adds the conversion to the viewer's 8-bit
interleaved layout, which the C++ side of the viewer did in its own loop.
Encodes are timed by `examples/encode_bench.rs` and `c/opj_enc.c` with
`OPJ_ENC_ITERS` on the same synthetic pattern, so one run gives speed and byte
parity.

### Results, 2026-10-20 (core fb9ee75)

Single thread, best of 9 (decode) or 60 (encode), Ryzen 9 9950X3D. C fork
built `-O2 -mavx2 -ffp-contract=off` (clang on Windows, gcc on Ubuntu 26.04
WSL); the port with `target-cpu=x86-64-v3`, release profile. Ratio is port
time over C time; `max_abs 0` means bit-identical decodes, `identical`
byte-identical codestreams.

Decode, codec only (3 components):

| image | Windows C ms | port ms | ratio | Linux C ms | port ms | ratio |
|---|---:|---:|---:|---:|---:|---:|
| 2048² lossless | 465.6 | 459.7 | 0.99 | 492.9 | 492.8 | 1.00 |
| 2048² lossy | 210.9 | 190.2 | 0.90 | 200.1 | 205.5 | 1.03 |
| 1024² lossless | 117.9 | 115.0 | 0.98 | | | |
| 1024² lossy | 50.9 | 45.5 | 0.89 | | | |
| 512² lossless | 30.0 | 29.3 | 0.98 | 31.1 | 30.7 | 0.99 |
| 512² lossy | 12.9 | 11.5 | 0.89 | 11.7 | 11.3 | 0.97 |
| 256² lossless | 7.83 | 7.98 | 1.02 | 8.57 | 8.36 | 0.98 |
| 256² lossy | 3.27 | 2.90 | 0.88 | 3.10 | 2.86 | 0.92 |

Windows, discard levels and truncated input (the viewer's progressive case):
2048² lossy at discard 1: 0.94, discard 2: 0.96; 512² lossy at discard 1:
0.93, discard 2: 0.93; first 6128 bytes of 2048² lossy: 0.72, lossless: 0.96;
first 24000 bytes of 512² lossy: 0.79, lossless: 0.97. All max_abs 0.
With the conversion to the viewer layout included (`vks`), 2048² lossy 0.92,
2048² lossless 1.01, 512² 0.92, 256² 0.94 of the C decode alone.

Encode (viewer rules: RLCP, layered 9/7 or single-layer 5/3, MCT, under-64 px
resolution rule), 3 and 4 components; worst and best ratio per size over the
four cases, all byte-identical:

| size | Windows ratio | Linux ratio |
|---|---|---|
| 8² | 0.62 to 0.83 | 0.91 to 1.00 |
| 16² | 0.74 to 0.89 | 0.95 to 1.08 (0.013 vs 0.014 ms) |
| 32² | 0.70 to 0.88 | 0.80 to 0.91 |
| 64² | 0.92 to 0.96 | 0.91 to 0.93 |
| 128² | 0.91 to 0.99 | 0.94 |
| 256² | 0.96 to 1.00 | 0.94 to 0.99 |
| 512² | 0.99 to 1.02 | 0.91 to 0.96 |
| 1024² | 0.95 to 1.00 | 0.94 to 1.01 |

Where the ratio is above 1.00 the difference is 2 to 3 percent at the
sub-millisecond level (or 5 ms of 200 for the Linux 2048² lossy decode), which
is within the spread between runs on this machine. The remaining measurable
difference between the two builds is in the Tier-1 passes, where the port and
the C fork now execute the same algorithm with the same register allocation
strategy; the forward 9/7 lifting step (`opj_dwt_encode_step2`) is scalar in
both and takes a slightly larger share in the port.

What closed the gap, in order of effect: the 5/3 SIMD wavelet; AVX2 for the
9/7 lifting, MCT and Tier-1 copy-out; inlining the Tier-1 pass bodies into
their 64x64 and generic wrappers so block size, flags stride and VSC mode are
constants (the C build gets this from `static INLINE`); and keeping the MQ
coder's `a`, `c`, `ct` and current context in a struct of plain scalars for a
whole pass, because a local copy of the whole `opj_mqc` cannot be scalarised
(its context array is indexed at runtime) and left those registers on the
stack on every decision. The same treatment, plus removing per-sample
`log::debug!` calls and a raw-pointer `nmsedec` accumulator, took the encoder
from 1.1 to 1.6 times the C time to parity.

### Results, 2026-10-21 (core 061be2b, the upstream baseline branch)

Same method as above; the C fork rebuilt from commit 3b1a6a96 with the
release flags (`-O2 -mavx2 -ffp-contract=off`, `OPJ_PRECISE_FP=ON`, GPU
backends off; clang 22 on Windows, gcc 15 on Ubuntu 26.04 WSL), the port at
the branch head. Raw tables and the parity sweep over the new fixtures are in
`tools/j2c-eval/results/2026-10-21-ryzen9950x3d-baseline.txt`.

Decode, codec only (3 components), best of 9, `max_abs 0` throughout:

| image | Windows C ms | port ms | ratio | Linux C ms | port ms | ratio |
|---|---:|---:|---:|---:|---:|---:|
| 2048² lossless | 456.6 | 459.3 | 1.01 | 503.5 | 469.7 | 0.93 |
| 2048² lossy | 208.0 | 186.5 | 0.90 | 209.9 | 196.6 | 0.94 |
| 1024² lossless | 114.9 | 115.2 | 1.00 | | | |
| 1024² lossy | 49.7 | 45.2 | 0.91 | | | |
| 512² lossless | 29.2 | 29.0 | 0.99 | 30.3 | 29.0 | 0.96 |
| 512² lossy | 12.4 | 11.1 | 0.90 | 11.6 | 11.2 | 0.96 |
| 256² lossless | 7.91 | 7.89 | 1.00 | 8.17 | 8.13 | 1.00 |
| 256² lossy | 3.15 | 2.87 | 0.91 | 3.04 | 2.83 | 0.93 |

Discard levels and truncated input: Windows 2048² lossy at discard 1: 0.92,
discard 2: 0.95; 512² lossy discard 1: 0.93, discard 2: 0.96; first 6128
bytes of 2048² lossy: 0.70, lossless: 0.90; first 24000 bytes of 512² lossy:
0.77, lossless: 0.96. Linux 2048² lossy discard 1: 0.92, discard 2: 0.93;
512² lossy discard 1: 0.94, discard 2: 0.94; first 6128 bytes of 2048²
lossy: 0.92, lossless: **1.04** (29.7 vs 28.6 ms; five repeats gave 1.01 to
1.05, so the port is 0.5 to 1.3 ms slower on this one case, a tile setup
dominated decode of 6 KB); first 24000 bytes of 512² lossy: 0.90, lossless:
0.73. With the conversion to the viewer layout (`vks`, Windows): 2048² lossy
0.93, lossless 1.01, 512² 0.93, 256² 0.93 of the C decode alone.

Encode (viewer rules), 3 and 4 components, worst and best ratio per size over
the four cases, best of 60 (15 at 512² and above), all byte-identical:

| size | Windows ratio | Linux ratio |
|---|---|---|
| 8² | 0.80 to 0.87 | 0.91 to 0.95 |
| 16² | 0.69 to 0.89 | 0.95 to 1.08 (0.014 vs 0.013 ms) |
| 32² | 0.72 to 0.82 | 0.72 to 0.91 |
| 64² | 0.92 to 0.97 | 0.90 to 0.93 |
| 128² | 0.91 to 0.99 | 0.92 to 0.94 |
| 256² | 0.97 to 1.00 | 0.94 to 0.99 |
| 512² | 0.97 to 1.00 | 0.93 to 0.96 |
| 1024² | 0.95 to 0.99 | 0.91 to 0.98 |

Compared with the 2026-10-20 tables the port is unchanged within run spread:
the tile-part index work adds a table build per header (no TLM in the
viewer's streams, so it only marks the table invalid) and removes the
TPsot==TNsot probe's extra read for single tile-part tiles.
