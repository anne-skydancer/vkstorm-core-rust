# vkstorm-core-rust

The Rust crates shared by both Vulkanstorm tracks:

- `vkstorm-rust`, the modernisation track: an incremental port of the existing
  Vulkanstorm viewer to Rust on its current OpenGL pipeline, in the
  [vulkanstorm](https://github.com/anne-skydancer/vulkanstorm) repository.
- [vkstorm-aleph](https://github.com/anne-skydancer/vkstorm-aleph), the
  future-proofing track: a ground-up Rust and Vulkan viewer.

Both consume these crates as git dependencies pinned by commit. This
repository holds protocol, codec, asset, audio and inventory logic only: no
rendering, no UI. The design study that these crates implement lives in
vkstorm-aleph under `docs/upstream-study/`.

## Crates

| Crate | Contents |
|---|---|
| `vkstorm-core` | LLSD and codecs, UUID, coordinate frames, asset and inventory enums, binary packers. Carries the `opensim` feature. |
| `vkstorm-openjp2` | The JPEG 2000 codec core: a port of OpenJPEG (from the `openjp2` crate, c2rust lineage) maintained and optimised here. See `docs/j2c-port-plan.md`. |
| `vkstorm-j2c` | The viewer-facing JPEG 2000 contract on that core: metadata, discard-level and truncated-stream decode, upload encoding with the viewer's rules. |

Planned, in port order: `vkstorm-messages`, `vkstorm-http`, `vkstorm-assets`,
`vkstorm-audio`, `vkstorm-inventory`, `vkstorm-world`, `vkstorm-avatar`,
`vkstorm-comms`, `vkstorm-rlv`, `vkstorm-scripting`.

## Grid support

OpenSim support is the `opensim` Cargo feature on `vkstorm-core`. Without it
the build is Second Life only: there is no OpenSim grid profile and none of
the legacy UDP protocol paths exist. With it, the grid profile is chosen at
login and fixed for the whole session. Second Life and OpenSim are never
mixed in one session, and nothing is negotiated per region on Second Life.

## Tools

- `tools/j2c-eval`: the decoder evaluation harness comparing the Vulkanstorm
  OpenJPEG fork with the pure-Rust decoders. See its README.

## Licence

GNU Lesser General Public License, version 2.1. See [LICENSE](LICENSE).
