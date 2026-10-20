//! LLSD serialization for Vulkanstorm: the notation, binary and XML formats.
//!
//! This crate is the Rust implementation behind the viewer's `LLSDSerialize`,
//! `LLSDParser` and `LLSDFormatter` classes (vkstorm-rust rung two). Its
//! output is byte-identical to the C++ formatters it replaces and its parsers
//! give the same results, counts and failures as the C++ parsers, including
//! their size and depth limits and their quirks, which are documented on each
//! module.
//!
//! The viewer keeps its own in-memory `LLSD` class. Parsing therefore streams
//! events into a [`Sink`] ([`FlatSink`] for the C ABI, [`ValueBuilder`] for a
//! Rust [`Value`]) and formatting is driven by the caller walking its tree and
//! calling one [`Formatter`] method per node.
//!
//! Input comes through a [`Reader`] over any [`Source`], so a C++ `istream`
//! can feed the parsers through a callback without being read to the end.

pub mod binary;
pub mod format;
pub mod notation;
pub mod reader;
pub mod sink;
pub mod text;
pub mod value;
pub mod xml;

pub use format::{Format, Formatter, Options, Out};
pub use reader::{Reader, SliceSource, Source};
pub use sink::{FlatDoc, FlatNode, FlatSink, KeyPolicy, Sink, ValueBuilder};
pub use text::FloatField;
pub use value::Value;

/// The parsers' failure return, as `LLSDParser::PARSE_FAILURE`.
pub const PARSE_FAILURE: i32 = -1;

/// The headers `LLSDSerialize::serialize` writes (`<? ... ?>\n`).
pub const BINARY_HEADER: &str = "LLSD/Binary";
pub const XML_HEADER: &str = "LLSD/XML";
pub const NOTATION_HEADER: &str = "llsd/notation";

/// Parse limits: `max_bytes` (`None` for `SIZE_UNLIMITED`) and `max_depth`
/// (-1 for unlimited), with the viewer's semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_bytes: Option<usize>,
    pub max_depth: i32,
}

impl Limits {
    pub const UNLIMITED: Limits = Limits {
        max_bytes: None,
        max_depth: -1,
    };

    /// Limits for a complete in-memory document of `len` bytes.
    pub fn for_len(len: usize) -> Limits {
        Limits {
            max_bytes: Some(len),
            max_depth: -1,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits::UNLIMITED
    }
}

/// How the XML parser reads: `LLSDParser::parse` (an unterminated document
/// fails) or `parseLines` (a complete document without `</llsd>` returns
/// what it found).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum XmlMode {
    #[default]
    Parse,
    Lines,
}

/// Parses one value of `format` from `r` into `sink`. Returns the number of
/// values parsed, 0 when nothing was there, or [`PARSE_FAILURE`].
pub fn parse_into<S: Sink>(
    format: Format,
    r: &mut Reader<'_>,
    sink: &mut S,
    limits: Limits,
    xml_mode: XmlMode,
) -> i32 {
    match format {
        Format::Notation => notation::parse(r, sink, limits),
        Format::Binary => binary::parse(r, sink, limits),
        Format::Xml => xml::parse(r, sink, xml_mode),
    }
}

/// The map key policy the viewer's parser of `format` has.
pub fn key_policy(format: Format) -> KeyPolicy {
    match format {
        Format::Xml => KeyPolicy::LastWins,
        _ => KeyPolicy::FirstWins,
    }
}

/// Parses one value from a byte slice into a [`Value`]. The count is as
/// [`parse_into`]; the value is `None` unless the count is positive.
pub fn parse(format: Format, input: &[u8], limits: Limits) -> (i32, Option<Value>) {
    let mut src = SliceSource::new(input);
    let mut r = Reader::new(&mut src);
    let mut b = ValueBuilder::new(key_policy(format));
    let n = parse_into(format, &mut r, &mut b, limits, XmlMode::Parse);
    if n > 0 { (n, b.finish()) } else { (n, None) }
}

/// Formats a [`Value`]. Returns the bytes and the number of values written.
pub fn format(format: Format, value: &Value, opts: Options) -> (Vec<u8>, i32) {
    let mut f = Formatter::new(format, opts, Vec::new());
    value.emit(&mut f);
    let (n, out) = f.finish();
    (out, n)
}
