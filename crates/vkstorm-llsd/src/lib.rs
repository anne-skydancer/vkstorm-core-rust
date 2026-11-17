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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

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
pub use sink::{
    FlatDoc, FlatNode, FlatSink, KeyPolicy, Sink, ValueBuilder, flat_from_value, value_from_flat,
    value_from_flat_parts,
};
pub use text::FloatField;
pub use value::Value;

/// The parsers' failure return, as `LLSDParser::PARSE_FAILURE`.
pub const PARSE_FAILURE: i32 = -1;

/// The headers `LLSDSerialize::serialize` writes (`<? ... ?>\n`).
pub const BINARY_HEADER: &str = "LLSD/Binary";
pub const XML_HEADER: &str = "LLSD/XML";
pub const NOTATION_HEADER: &str = "llsd/notation";

/// Parse limits with the viewer's semantics: `max_bytes` is `None` for
/// `SIZE_UNLIMITED` (-1), otherwise the byte budget, which the C++ parsers
/// also accepted negative (any other negative value limits everything);
/// `max_depth` is -1 for unlimited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_bytes: Option<i64>,
    pub max_depth: i32,
    /// The most values (every scalar, map and array) one parse may build,
    /// 0 for unlimited. See [`DEFAULT_MAX_NODES`].
    pub max_nodes: usize,
}

/// The node budget of every parse of data that is not trusted local data
/// (`LLSDParser::MAX_NODES_DEFAULT` in the C++). A value costs tens of bytes
/// of heap and can take one byte of input (`!`, an empty array), so a payload
/// of a few hundred MiB would otherwise build several GiB. 2^23 values bound a
/// parse to about 0.5 GiB, and the largest real payloads (an AIS inventory
/// reply is well under 2M values) stay far below it. Trusted local data that
/// needs more uses [`Limits::UNLIMITED`].
pub const DEFAULT_MAX_NODES: usize = 1 << 23;

impl Limits {
    pub const UNLIMITED: Limits = Limits {
        max_bytes: None,
        max_depth: -1,
        max_nodes: 0,
    };

    /// Limits for a complete in-memory document of `len` bytes.
    pub fn for_len(len: usize) -> Limits {
        Limits {
            max_bytes: Some(len as i64),
            max_depth: -1,
            max_nodes: DEFAULT_MAX_NODES,
        }
    }

    /// Limits from the C++ arguments: `max_bytes` -1 is unlimited.
    pub fn from_cpp(max_bytes: i64, max_depth: i32) -> Limits {
        Limits {
            max_bytes: if max_bytes == -1 {
                None
            } else {
                Some(max_bytes)
            },
            max_depth,
            max_nodes: DEFAULT_MAX_NODES,
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
    if limits.max_nodes == 0 {
        return parse_format(format, r, sink, limits, xml_mode);
    }
    // The budget is spent in a sink wrapper, which stops forwarding once it
    // runs out; the abort flag then ends the reader's input, so the parser
    // stops within one buffered window instead of reading on to the end.
    let abort = Arc::new(AtomicBool::new(false));
    r.set_abort(Some(abort.clone()));
    let mut budget = Budget {
        inner: sink,
        left: limits.max_nodes,
        abort: &abort,
    };
    let n = parse_format(format, r, &mut budget, limits, xml_mode);
    r.set_abort(None);
    if abort.load(Ordering::Relaxed) {
        warn(&format!(
            "LLSD parse stopped: more than {} values",
            limits.max_nodes
        ));
        return PARSE_FAILURE;
    }
    n
}

fn parse_format<S: Sink>(
    format: Format,
    r: &mut Reader<'_>,
    sink: &mut S,
    limits: Limits,
    xml_mode: XmlMode,
) -> i32 {
    match format {
        Format::Notation => notation::parse(r, sink, limits),
        Format::Binary => binary::parse(r, sink, limits),
        Format::Xml => xml::parse(r, sink, xml_mode, limits.max_depth),
    }
}

/// Counts the values a parse builds against [`Limits::max_nodes`]. Once the
/// budget is spent it sets the abort flag and forwards nothing more, so the
/// receiving sink holds at most `max_nodes` values.
struct Budget<'a, S: Sink> {
    inner: &'a mut S,
    left: usize,
    abort: &'a AtomicBool,
}

impl<S: Sink> Budget<'_, S> {
    /// Spends one value of the budget; false (and the abort flag set) when
    /// none is left.
    #[inline]
    fn charge(&mut self) -> bool {
        if self.left == 0 {
            self.abort.store(true, Ordering::Relaxed);
            return false;
        }
        self.left -= 1;
        true
    }

    #[inline]
    fn live(&self) -> bool {
        !self.abort.load(Ordering::Relaxed)
    }
}

impl<S: Sink> Sink for Budget<'_, S> {
    fn undef(&mut self) {
        if self.charge() {
            self.inner.undef();
        }
    }
    fn boolean(&mut self, v: bool) {
        if self.charge() {
            self.inner.boolean(v);
        }
    }
    fn integer(&mut self, v: i32) {
        if self.charge() {
            self.inner.integer(v);
        }
    }
    fn real(&mut self, v: f64) {
        if self.charge() {
            self.inner.real(v);
        }
    }
    fn uuid(&mut self, v: &[u8; 16]) {
        if self.charge() {
            self.inner.uuid(v);
        }
    }
    fn string(&mut self, v: &[u8]) {
        if self.charge() {
            self.inner.string(v);
        }
    }
    fn date(&mut self, seconds: f64) {
        if self.charge() {
            self.inner.date(seconds);
        }
    }
    fn uri(&mut self, v: &[u8]) {
        if self.charge() {
            self.inner.uri(v);
        }
    }
    fn binary(&mut self, v: &[u8]) {
        if self.charge() {
            self.inner.binary(v);
        }
    }
    fn begin_map(&mut self) {
        if self.charge() {
            self.inner.begin_map();
        }
    }
    fn key(&mut self, k: &[u8]) {
        if self.live() {
            self.inner.key(k);
        }
    }
    fn end_map(&mut self) {
        if self.live() {
            self.inner.end_map();
        }
    }
    fn begin_array(&mut self) {
        if self.charge() {
            self.inner.begin_array();
        }
    }
    fn end_array(&mut self) {
        if self.live() {
            self.inner.end_array();
        }
    }
}

type WarnHook = Box<dyn Fn(&str) + Send + Sync>;

static WARN_HOOK: OnceLock<WarnHook> = OnceLock::new();

/// Installs the receiver of the parsers' warnings (the node budget running
/// out), `LL_WARNS` in the viewer. Installed once; a second call is ignored,
/// and warnings before it are dropped.
pub fn set_warn_hook(f: WarnHook) {
    let _ = WARN_HOOK.set(f);
}

fn warn(msg: &str) {
    if let Some(f) = WARN_HOOK.get() {
        f(msg);
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
