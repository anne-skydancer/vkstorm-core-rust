# vkstorm-core-rust licensing notice

These crates are the Rust core of the [Vulkanstorm](https://github.com/anne-skydancer/vulkanstorm)
viewer. Since 2026-11-08 Vulkanstorm is licensed under the **GNU General
Public License, version 3 or (at your option) any later version**
(`GPL-3.0-or-later`); this repository follows it. The text is in
[`LICENSE`](LICENSE), verbatim from <https://www.gnu.org/licenses/gpl-3.0.txt>.
The LGPL-2.1 option of the dual licence of 2026-11-03 is dropped; the LGPL-2.1
text stays in [`LICENSE-LGPL-2.1`](LICENSE-LGPL-2.1) because the ported Linden
Lab and Firestorm code refers to it. The viewer's `NOTICE.md` and
`doc/licensing.md` cover the viewer as a whole, including the compatibility
audit of what it links with.

## Crates

| Crate | `license` (`Cargo.toml`) | Why |
|---|---|---|
| vkstorm-assets, vkstorm-core, vkstorm-ffi, vkstorm-http, vkstorm-image, vkstorm-inventory, vkstorm-j2c, vkstorm-llsd, vkstorm-messages, vkstorm-volume | `GPL-3.0-or-later` (inherited from `[workspace.package]`) | Ports of the viewer's C++ (Linden Lab and Firestorm code, LGPL-2.1) and original code: each crate states that it mirrors, and is behaviour- or byte-identical to, the C++ it replaces. `vkstorm-ffi` is the C ABI over those ports, shaped by the C++ callers |
| vkstorm-audio | `GPL-3.0-or-later AND Zlib` | The viewer's audio engine port, with a mixer derived from SoLoud (Zlib, `LICENSE-soloud`) |
| vkstorm-vorbis | `BSD-3-Clause AND GPL-3.0-or-later` | A port of Xiph's libogg/libvorbis decoder (BSD-3-Clause, `LICENSE-xiph`) behind the viewer's decode contract |
| vkstorm-openjp2 | `BSD-2-Clause` | A port of OpenJPEG via the `openjp2` crate; keeps its own licence (`LICENSE`, `NOTICE.md` there). BSD-2-Clause is GPL-3.0 compatible, so it can be linked into a GPL-3.0 library |
| tools: image-eval, j2c-eval, soloud-oracle, vorbis-oracle | `GPL-3.0-or-later` (inherited) | Harnesses that build the viewer's C++ code as the parity oracle |

New source files carry `SPDX-License-Identifier: GPL-3.0-or-later` where a
file header is used; the crate's `Cargo.toml` states the expression either way.
The ported Linden Lab and Firestorm code keeps its headers (the few that exist,
such as `$LicenseInfo` in `crates/vkstorm-ffi/include/vkstorm_ffi.h`) unchanged.

## Linden Lab and Firestorm code

Linden Lab and Firestorm code stays LGPL-2.1, with its headers unchanged, and is combined into Vulkanstorm, which as a whole is licensed GPL-3.0-or-later (LGPL-2.1 is compatible with inclusion in a GPL-3.0 work).

The BSD-2-Clause, BSD-3-Clause and Zlib parts stay under those licences; all
three are compatible with GPL-3.0.

## Dependencies

The crates.io dependencies compiled into `vkstorm-ffi` (all features; the
packaged library lists them in `LICENSES/third-party.txt`) are under MIT,
Apache-2.0, Zlib, ISC, BSD, Unlicense, Unicode-3.0, IJG and MPL-2.0
(Symphonia). MPL-2.0 is GPL-compatible through its section 3.3 (Secondary
Licenses). None is GPL-2.0-only or proprietary; all are compatible with
GPL-3.0 (Apache-2.0 is compatible with GPL-3.0, not with GPL-2.0).

vkstorm-http's reply decoding brings `flate2` (MIT OR Apache-2.0, used
under MIT) built on `zlib-rs` (Zlib) and nothing else (`cargo tree -p
vkstorm-http -e normal`: flate2 1.1.10, zlib-rs 0.6.8). With flate2 in
`vkstorm-ffi`, the `png` crate's flate2 (vkstorm-image) builds on zlib-rs
too; inflate output is the same either way.

The decoder-only FFmpeg that `vkstorm-audio` loads at run time
(`LGPL-2.1-or-later`, the viewer's `ffmpeg` package) is a separate shared
library, compatible with GPL-3.0.

## Packaging

`packaging/build_package.py` ships `LICENSES/vkstorm-core-rust.txt` (this
notice, the GPL-3.0 text and the LGPL-2.1 text), `LICENSES/third-party.txt`,
and the notices of the vendored ports: `vkstorm-openjp2-LICENSE.txt`,
`vkstorm-openjp2-NOTICE.txt`, `vkstorm-vorbis-LICENSE-xiph.txt` and
`vkstorm-audio-LICENSE-soloud.txt`.

## Other people's GPL code

Code available only under the GPL (for example the Aurora Viewer,
GPL-3.0-or-later) can be used where its authors permit. Aurora is still a
design reference first.

## Contributions

Contributions are accepted under `GPL-3.0-or-later`, or under the licence of
the vendored port they change (table above).
