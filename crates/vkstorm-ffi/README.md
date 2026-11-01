# vkstorm-ffi

The C ABI the Vulkanstorm viewer's C++ calls into the Rust port: `extern "C"`
functions over the vkstorm-core-rust crates, declared in
[`include/vkstorm_ffi.h`](include/vkstorm_ffi.h). Each function catches panics
at the boundary and uses only plain types; keep the Rust and the header in step.

It moved here from the viewer's `indra/rust/vkstorm-ffi` unchanged in code, so
that it can be built and shipped as a package: viewer builds link a prebuilt
static library and need no Rust toolchain.

## Rungs are features

| Feature | Rung | Over |
|---|---|---|
| `j2c` | one | `vkstorm-j2c`: JPEG 2000 decode and encode (`vks_j2c_*`) |
| `llsd` | two | `vkstorm-llsd`: LLSD serialization (`vks_llsd_*`) |
| `volume` | two | `vkstorm-volume`: the parametric prim mesher |
| `image` | two | `vkstorm-image`: PNG, TGA, BMP and DXT |
| `messages` | three | `vkstorm-messages`: the message template system and data packers |
| `http` | four | `vkstorm-http`: the HTTP layer |
| `assets` | five | `vkstorm-assets`: the asset pipeline |
| `audio` | six | `vkstorm-audio` and `vkstorm-vorbis`: the audio engine, sound decoding, parcel music |
| `inventory` | seven | `vkstorm-inventory`: the inventory rung: R1 data model, R2 cache file, login load and validate() (`vks_inv_*`) |
| `full` | all | every rung above |
| `opensim` | | the legacy UDP paths of the crates that carry them and the OpenSim-only inventory rules (the viewer's `OPENSIM` option) |

The header declares every rung. A build without a rung does not define its
functions, so a viewer line must not call a rung it has not adopted; linking
would fail if it did.

## The package

`packaging/build_package.py` (run by `autobuild build`, see the repository's
`autobuild.xml`) builds the library with the `ffi-package` profile (fat LTO, one
codegen unit, line tables) for the viewer's CPU baseline (`x86-64-v3`, its
`USE_AVX2_OPTIMIZATION`), with the rungs named in `VKS_FFI_FEATURES`
(default `full`), and stages:

| Path | Contents |
|---|---|
| `lib/release/vkstorm_ffi.lib`, `libvkstorm_ffi.a` | the static library (windows64, linux64) |
| `lib/release/vkstorm_ffi.native-libs.txt` | the system libraries it leaves to the final link, as rustc reports them |
| `include/vkstorm_ffi.h` | the C ABI |
| `LICENSES/vkstorm-core-rust.txt`, `LICENSES/third-party.txt` | this repository's licence; every crate compiled in, with its licence |
| `VERSION.txt` | `<crate version>.<commit>.<rungs>`, for example `0.1.0.89ca6ca3dc.j2c` |

By hand:

    VKS_FFI_FEATURES=j2c autobuild build -A 64 -c release
    autobuild package -A 64 -c release --archive-format tzst

CI (`.github/workflows/package-ffi.yaml`) builds every rung set the viewer lines
use on windows-2022 and ubuntu-24.04 for each push and pull request; a tag
`ffi-v*` publishes the archives and `SHA256SUMS` as a GitHub release, which the
viewer lines pin by URL and hash.
