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
| 8 | Bring the port to the fork's 2.5.4-plus baseline (upstream diff 2.5.2 to 2.5.4+) | parity | truncated-packet handling (`corrupted` code-blocks) and the end-of-stream check in the tile-part read ported 2026-10-20; remaining diff open |
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
