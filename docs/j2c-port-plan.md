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
| 4 | Profile the lossless 5/3 path (4x gap) and the general 10% gap; remove translation overhead (bounds checks in hot loops, indirection) | perf | 5/3 done 2026-10-20 (`dwt53.rs`: lossless 1024² 54 to 15.7 ms, C fork 12.8 ms); 9/7 and Tier-1 gap open |
| 5 | AVX2 wavelet (9/7 and 5/3) and MCT with `std::arch` plus runtime detection, mirroring upstream's intrinsics; SSE2 fallback | perf | 5/3 done (SSE2 and AVX2 vertical passes, AVX2 horizontal); 9/7 and MCT open |
| 6 | Code-block thread pool for Tier-1 (upstream's design, `opj_thread_pool`), bounded by a caller-set worker count | perf | open |
| 7 | Small-image path: measure 16 to 256 px decodes and encodes, cut per-call allocation and setup, keep the under-64 px resolution rule | small images | open |
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
