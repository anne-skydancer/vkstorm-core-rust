# vkstorm-core-rust

The Rust crates shared by both Vulkanstorm tracks:

- `vkstorm-rust`, the modernisation track: an incremental port of the existing
  Vulkanstorm viewer to Rust on its current OpenGL pipeline, in the
  [vulkanstorm](https://github.com/anne-skydancer/vulkanstorm) repository.
- [vkstorm-aleph](https://github.com/anne-skydancer/vkstorm-aleph), the
  future-proofing track: a ground-up Rust and Vulkan viewer.

vkstorm-aleph consumes these crates as git dependencies pinned by commit; the
vulkanstorm viewer links them through `vkstorm-ffi`, built here and shipped as an
autobuild package (see below). This
repository holds protocol, codec, asset, audio and inventory logic only: no
rendering, no UI. The design study that these crates implement lives in
vkstorm-aleph under `docs/upstream-study/`.

## Crates

| Crate | Contents |
|---|---|
| `vkstorm-core` | LLSD and codecs, UUID, coordinate frames, asset and inventory enums, binary packers. Carries the `opensim` feature. |
| `vkstorm-openjp2` | The JPEG 2000 codec core: a port of OpenJPEG (from the `openjp2` crate, c2rust lineage) maintained and optimised here. See `docs/j2c-port-plan.md`. |
| `vkstorm-j2c` | The viewer-facing JPEG 2000 contract on that core: metadata, discard-level and truncated-stream decode, upload encoding with the viewer's rules. |
| `vkstorm-llsd` | LLSD serialization: the notation, binary and XML parsers and formatters, byte-compatible with the viewer's C++ `LLSDSerialize`, streaming into a sink so the viewer keeps its own `LLSD` class. |
| `vkstorm-image` | The other raster codecs behind the viewer's `LLImageFormatted` classes: PNG (libpng-equivalent transformations and gamma, libpng-equivalent encoding), TGA, BMP and DXT ports, and the measured JPEG candidate (feature `jpeg`, not adopted). |
| `vkstorm-volume` | The parametric prim mesher behind `LLVolume`: profile and path generation, the mesh sweep, side and cap faces, tangents; float-identical to the viewer's C++ on the recorded corpus. |
| `vkstorm-messages` | The UDP message system's data side behind `LLTemplateParser`, `LLTemplateMessageBuilder`, `LLTemplateMessageReader` and `LLDataPacker`: the template language, packing and unpacking of every variable type with the block and size rules, zero-coding both ways, the binary and text data packers; byte-identical to the viewer's C++ on the recorded corpus. Carries the `opensim` feature. |

| `vkstorm-http` | The viewer's HTTP layer behind `LLCore::HttpRequest`: the service thread with its request and reply queues, policy classes with connection limits and request-rate throttling, the retry queue with its backoff, and the libcurl multi-handle transport (libcurl bound by hand, the viewer's own build with OpenSSL and nghttp2); request-for-request identical to the C++ on the recorded scenarios. |
| `vkstorm-assets` | The asset pipeline's data side behind `LLTextureCache`, `LLTextureFetch`, `LLDiskCache`/`LLFileSystem`, `LLVolume::unpackVolumeFaces`, `LLMeshSkinInfo`, `LLModel::Decomposition`, `LLMeshHeader` and `LLAssetStorage`: the texture cache files (byte-compatible with the C++ cache), the texture fetch state machine (cache, range, decode and write-back decisions, retries, the HTTP resource semaphore) driven by the host's threads, the JPEG 2000 byte budget per discard level, the asset disk cache's naming and purge, mesh asset unpacking into the viewer's face buffers with the skin and physics blocks, mesh fetch planning, and the asset request bookkeeping. Carries the `opensim` feature (the legacy UDP texture and asset paths). |
| `vkstorm-ffi` | The C ABI the C++ viewer calls (`include/vkstorm_ffi.h`), over the crates above. One Cargo feature per rung of the port; built as a static library and shipped to the viewer as the `vkstorm-ffi` autobuild package. See its README. |

Planned, in port order: `vkstorm-audio`, `vkstorm-inventory`,
`vkstorm-world`, `vkstorm-avatar`, `vkstorm-comms`, `vkstorm-rlv`,
`vkstorm-scripting`.

## Grid support

OpenSim support is the `opensim` Cargo feature on `vkstorm-core`. Without it
the build is Second Life only: there is no OpenSim grid profile and none of
the legacy UDP protocol paths exist. With it, the grid profile is chosen at
login and fixed for the whole session. Second Life and OpenSim are never
mixed in one session, and nothing is negotiated per region on Second Life.

## The viewer's package: vkstorm-ffi

Viewer builds need no Rust toolchain. The workflow
`.github/workflows/package-ffi.yaml` builds the `vkstorm-ffi` static library with
the toolchain pinned in `rust-toolchain.toml` and packages it with autobuild
(`autobuild.xml`, `packaging/build_package.py`), once per rung set, for windows64
and linux64. A tag `ffi-v*` publishes the archives and their hashes as a release;
each viewer line pins its archive by URL and hash in its own `autobuild.xml`:
`vkstorm-rust` takes `full`, `vkstorm-release` the rungs promoted to it.

## Tools

- `tools/j2c-eval`: the decoder evaluation harness comparing the Vulkanstorm
  OpenJPEG fork with the pure-Rust decoders. See its README.
- `tools/llsd-fixtures`: regenerates the `vkstorm-llsd` test fixtures from the
  viewer's former C++ LLSD implementation (its `llsdserialize_tool`). See its
  README.
- `tools/image-eval`: the parity oracle (the viewer's C++ raster codecs,
  standalone) and timing harness for `vkstorm-image`. See its README.

## Licence

GNU Lesser General Public License, version 2.1. See [LICENSE](LICENSE).
