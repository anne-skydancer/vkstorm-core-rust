# vkstorm-core-rust licensing notice

These crates are the Rust core of the [Vulkanstorm](https://github.com/anne-skydancer/vulkanstorm)
viewer. Since 2026-11-03 Vulkanstorm is offered under the GNU General Public
License, version 3 or later, **as well as** the GNU Lesser General Public
License, version 2.1; this repository follows it. The licence texts are
[`LICENSE-LGPL-2.1`](LICENSE-LGPL-2.1) and [`LICENSE-GPL-3.0`](LICENSE-GPL-3.0)
(verbatim from <https://www.gnu.org/licenses/gpl-3.0.txt>). The viewer's
`NOTICE.md` and `doc/licensing.md` cover the viewer as a whole, including the
compatibility audit of what it links with.

## Crates

| Crate | SPDX expression (`Cargo.toml`) | Why |
|---|---|---|
| vkstorm-assets, vkstorm-core, vkstorm-ffi, vkstorm-http, vkstorm-image, vkstorm-inventory, vkstorm-j2c, vkstorm-llsd, vkstorm-messages, vkstorm-volume | `LGPL-2.1-only OR GPL-3.0-or-later` (inherited from `[workspace.package]`) | Ports of the viewer's C++ (Linden Lab and Firestorm code): each crate states that it mirrors, and is behaviour- or byte-identical to, the C++ it replaces. `vkstorm-ffi` is the C ABI over those ports, shaped by the C++ callers |
| vkstorm-audio | `(LGPL-2.1-only OR GPL-3.0-or-later) AND Zlib` | The viewer's audio engine port, with a mixer derived from SoLoud (Zlib, `LICENSE-soloud`) |
| vkstorm-vorbis | `BSD-3-Clause AND (LGPL-2.1-only OR GPL-3.0-or-later)` | A port of Xiph's libogg/libvorbis decoder (BSD-3-Clause, `LICENSE-xiph`) behind the viewer's decode contract |
| vkstorm-openjp2 | `BSD-2-Clause` | A port of OpenJPEG via the `openjp2` crate; keeps its own licence (`LICENSE`, `NOTICE.md` there) |
| tools: image-eval, j2c-eval, soloud-oracle, vorbis-oracle | `LGPL-2.1-only OR GPL-3.0-or-later` (inherited) | Harnesses that build the viewer's C++ code as the parity oracle |

No crate here is wholly original code without a Linden Lab or Firestorm
counterpart, so none uses the `LGPL-2.1-or-later OR GPL-3.0-or-later`
expression that the viewer gives its original Vulkanstorm code. A future
crate that is wholly original should use that expression in its own
`Cargo.toml` instead of inheriting the workspace one.

## Why GPL-3.0 can be offered for ported Linden Lab and Firestorm code

Linden Lab and Firestorm license their code under LGPL-2.1 only. Section 3
of LGPL-2.1:

> 3. You may opt to apply the terms of the ordinary GNU General Public
> License instead of this License to a given copy of the Library.  To do
> this, you must alter all the notices that refer to this License, so
> that they refer to the ordinary GNU General Public License, version 2,
> instead of to this License.  (If a newer version than version 2 of the
> ordinary GNU General Public License has appeared, then you can specify
> that version instead if you wish.)  Do not make any other change in
> these notices.
>
> Once this change is made in a given copy, it is irreversible for
> that copy, so the ordinary GNU General Public License applies to all
> subsequent copies and derivative works made from that copy.

This repository does not make that change itself, so every recipient keeps
both options; the GPL-3.0-or-later option is the section 3 conversion each
recipient may make for their own copy.

## How to choose

- **LGPL-2.1**: use the crates as they are.
- **GPL-3.0 or later**: make the section 3 change in your copy; it cannot be
  undone for that copy and the copies made from it.

The BSD-2-Clause, BSD-3-Clause and Zlib parts stay under those licences
either way; all three are compatible with both options.

## Dependencies

The crates.io dependencies compiled into `vkstorm-ffi` (all features; the
packaged library lists them in `LICENSES/third-party.txt`) are under MIT,
Apache-2.0, Zlib, ISC, BSD, Unlicense, Unicode-3.0, IJG and MPL-2.0
(Symphonia). MPL-2.0 is GPL-compatible through its section 3.3 (Secondary
Licenses). None is GPL-only or proprietary.

vkstorm-http's reply decoding brings `flate2` (MIT OR Apache-2.0, used
under MIT) built on `zlib-rs` (Zlib) and nothing else (`cargo tree -p
vkstorm-http -e normal`: flate2 1.1.10, zlib-rs 0.6.8). MIT and Zlib are
permissive and compatible with both LGPL-2.1 and GPL-3.0. With flate2 in
`vkstorm-ffi`, the `png` crate's flate2 (vkstorm-image) builds on zlib-rs
too; inflate output is the same either way.

## GPL-only code

There is none, and there must be none: code available only under the GPL
(for example the Aurora Viewer, GPL-3.0-or-later only) would remove the
LGPL option. Aurora is a design reference only; its code is never copied.

## Contributions

Contributions are accepted under the expression of the crate they change
(table above).
