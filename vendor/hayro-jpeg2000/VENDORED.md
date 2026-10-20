# Vendored hayro-jpeg2000 0.4.1

Source: crates.io `hayro-jpeg2000` 0.4.1 (https://github.com/LaurenzV/hayro),
Apache-2.0 OR MIT; both licence texts are kept beside this file.

Local change: `src/j2c/tile.rs` accepts a tile-part whose declared length
exceeds the bytes present when strict mode is off, so truncated codestreams
from progressive fetches decode as far as the data goes (patch:
`tools/j2c-eval/patches/hayro-jpeg2000-0.4.1-truncated-tilepart.diff`). The
copy is wired in through `[patch.crates-io]` in the workspace `Cargo.toml` and
goes away once the change is upstreamed.
