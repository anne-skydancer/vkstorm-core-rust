//! The viewer's UDP message system, data side: the message template language
//! (`message_template.msg`), the template message builder and reader (packing
//! and unpacking of every variable type, block repeat rules, the size fields of
//! variable-length fields), zero-coding in both directions and the `LLDataPacker`
//! binary and ASCII formats.
//!
//! This is the Rust implementation behind the viewer's `LLTemplateParser`,
//! `LLTemplateMessageBuilder`, `LLTemplateMessageReader`,
//! `LLMessageSystem::zeroCodeExpand` and `LLDataPacker*` classes (vkstorm-rust
//! rung three). Its wire bytes are identical to the C++ it replaces and its
//! unpacking gives the same values, sizes, counts and failures, including on
//! truncated and malformed packets, where the C++ read past the packet; those
//! cases are settled as one stated behaviour (see [`reader`]).
//!
//! The crate has no I/O: sockets, circuits, reliability and the dispatch of
//! decoded messages to handlers stay with the caller. Blocks and variables are
//! identified by caller-supplied keys (the viewer passes its canonical string
//! pointers), so lookups are integer comparisons.
//!
//! The `opensim` feature marks the legacy protocol paths a Second Life-only
//! build does not contain. The template language and the packing rules are
//! the same on both grids; the feature exists so that the grid profile chosen
//! at login is a compile-time fact of the crate, matching the viewer's CMake
//! option.

#![forbid(unsafe_code)]

pub mod ascii;
pub mod builder;
pub mod datapacker;
pub mod parser;
pub mod reader;
pub mod template;
pub mod zerocode;

pub use builder::{BuildError, Builder, BuilderError};
pub use parser::{
    ParseError, TemplateFile, Tokenizer, parse_block, parse_message, parse_template, parse_variable,
};
pub use reader::{DecodeOutcome, GetError, NumberError, RanOffEnd, Reader};
pub use template::{
    Block, BlockType, Deprecation, Encoding, Frequency, Template, Trust, VarType, Variable,
};

/// `LL_PACKET_ID_SIZE`: flags byte, 4-byte packet id, 1-byte offset.
pub const PACKET_ID_SIZE: usize = 6;
/// `PHL_OFFSET`: the header byte holding the extra offset before the data.
pub const PHL_OFFSET: usize = 5;
/// `LL_ZERO_CODE_FLAG` in the flags byte.
pub const ZERO_CODE_FLAG: u8 = 0x80;
/// `LL_MINIMUM_VALID_PACKET_SIZE`: the packet id plus one message-number byte.
pub const MINIMUM_VALID_PACKET_SIZE: usize = PACKET_ID_SIZE + 1;
/// `MAX_BUFFER_SIZE` (`NET_BUFFER_SIZE`): the viewer's packet buffers.
pub const MAX_BUFFER_SIZE: usize = 0x2000;
/// `MAX_BLOCKS`: the most repeats of a variable block (its count is one byte).
pub const MAX_BLOCKS: i32 = 255;
/// `MTUBYTES`: the builder reports a message "full" beyond this many data bytes.
pub const MTUBYTES: i32 = 1200;
