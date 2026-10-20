//! JPEG 2000 (J2C) texture decoding.
//!
//! Wraps `hayro-jpeg2000` behind the contract the viewer's texture pipeline
//! needs: header-only metadata, decode at a discard level (resolution
//! reduction), and lenient decode of truncated codestreams from progressive
//! fetches. The decoder choice and the truncation patch it requires are
//! documented in the vkstorm-aleph study, `07-j2c-decoder-evaluation.md`, and
//! measured by `tools/j2c-eval` in this repository.
//!
//! Rung one of the `vkstorm-rust` port: this crate replaces `LLImageJ2COJ`
//! through a C-ABI shim.
