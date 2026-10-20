# vkstorm-openjp2: provenance

This crate started as a copy of the `openjp2` crate, version 0.6.1
(https://github.com/Neopallium/openjp2, "Rust port of Openjpeg", BSD-2-Clause),
itself a port of OpenJPEG 2.5.2 (https://github.com/uclouvain/openjpeg,
BSD-2-Clause). Both licence texts are preserved in `LICENSE`.

It is maintained here as Vulkanstorm's single JPEG 2000 codec (decode and
encode) and diverges from its origins deliberately:

- the `file-io`/libc dependency is dropped; the viewer only uses memory streams;
- non-strict (lenient) decoding of truncated codestreams is being brought to
  OpenJPEG 2.5.4 behaviour, which the viewer's progressive texture fetch needs;
- SIMD (AVX2) paths for the wavelet and colour transforms, and a code-block
  thread pool, are being added to reach at least the C fork's performance;
- the C-shaped internals are made safe incrementally.

The Vulkanstorm OpenJPEG fork (https://github.com/anne-skydancer/openjpeg)
remains the reference oracle in `tools/j2c-eval`.
