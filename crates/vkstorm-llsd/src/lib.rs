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

use std::cell::Cell;
use std::rc::Rc;
use std::sync::OnceLock;

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
    /// The node budget in units, 0 for unlimited: a map costs
    /// [`NODE_WEIGHT_MAP`] units, every other value 1, and every element
    /// inside a skipped XML subtree [`NODE_WEIGHT_SKIPPED_ELEMENT`] (the
    /// skipped root itself is free). See [`DEFAULT_MAX_NODES`].
    pub max_nodes: usize,
}

/// The cost of a map in node-budget units; every other value costs 1. A
/// map is the heaviest value (a `BTreeMap` leaf is about 640 bytes), so the
/// weight keeps the heap per unit about level across shapes. The C++
/// `LLSDParser` charges the same.
pub const NODE_WEIGHT_MAP: usize = 4;

/// The cost of each element inside a skipped XML subtree (a value nested in a
/// scalar element, say): the parser keeps it on its element stack. The
/// skipped root is not charged, only its descendants, as in the C++.
pub const NODE_WEIGHT_SKIPPED_ELEMENT: usize = 2;

/// The default node budget, in units (see [`NODE_WEIGHT_MAP`]) of one parse
/// of data that is not trusted local data (`LLSDParser::MAX_NODES_DEFAULT`
/// in the C++). A value costs tens of bytes of heap and can take one byte of
/// input (`!`, an empty array), so a payload of a few hundred MiB would
/// otherwise build several GiB. A real item (an inventory item or folder in
/// an AIS reply) is about 30 values, 33 units, so 2^23 units are about 250k
/// items in one reply; for compressed HTTP replies the 256 MiB decode cap
/// binds first.
///
/// Peak heap of a parse that runs into the budget, into a [`ValueBuilder`]
/// (the [`FlatSink`] of the C ABI needs 0.3 to 1 times as much), measured by
/// `tests/heap_peak.rs` (release build, Windows):
///
/// | shape                   | 2^20 units | 2^23 units |
/// |-------------------------|-----------:|-----------:|
/// | array of undefs         |     32 MiB |    256 MiB |
/// | nested one-element arrays |   88 MiB |    704 MiB |
/// | map of 4-byte keys, undefs | 112 MiB |    893 MiB |
/// | array of one-key maps   |    135 MiB |   1077 MiB |
/// | array of empty maps     |      8 MiB |     64 MiB |
/// | nested maps             |     22 MiB |    178 MiB |
/// | XML skipped descendants |     25 MiB |    200 MiB |
///
/// The weight of a map levels most shapes but not the one-key maps (about
/// 134 bytes of heap per unit, against 32 for a flat array): the worst case
/// at the default is about 1.1 GiB. Trusted local data that needs more uses
/// [`Limits::UNLIMITED`].
pub const DEFAULT_MAX_NODES: usize = 1 << 23;

/// The budget for assets whose inflated size the caller caps (mesh headers
/// and bodies, skins, decomposition): a real asset has a few thousand values
/// around large binary leaves.
pub const ASSET_MAX_NODES: usize = 1 << 20;

/// The budget for an HTTP or AIS reply body of `body_len` bytes: the default,
/// or one unit per 16 body bytes when that is more, so a reply that really is
/// large is not cut off by a ceiling sized for small ones.
pub fn http_max_nodes(body_len: usize) -> usize {
    DEFAULT_MAX_NODES.max(body_len / 16)
}

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

    /// Limits from the C++ arguments: `max_bytes` -1 is unlimited, and
    /// `max_nodes` 0 is unlimited (`LLSDParser::MAX_NODES_UNLIMITED`).
    pub fn from_cpp(max_bytes: i64, max_depth: i32, max_nodes: usize) -> Limits {
        Limits {
            max_bytes: if max_bytes == -1 {
                None
            } else {
                Some(max_bytes)
            },
            max_depth,
            max_nodes,
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
///
/// On [`PARSE_FAILURE`] the sink holds whatever prefix of the document the
/// parser had delivered (with a node budget, at most `max_nodes` units of
/// it); callers must discard it.
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
    // stops within one buffered window (or at its next read of an exact
    // source) instead of reading on to the end.
    let abort = Rc::new(Cell::new(false));
    let mut guard = AbortGuard::new(r, abort.clone());
    let mut budget = Budget {
        inner: sink,
        left: limits.max_nodes,
        abort: &abort,
    };
    let n = parse_format(format, guard.reader(), &mut budget, limits, xml_mode);
    drop(guard);
    if abort.get() {
        warn(&format!(
            "LLSD parse stopped: more than {} values",
            limits.max_nodes
        ));
        return PARSE_FAILURE;
    }
    n
}

/// Arms a reader's abort flag for one parse and disarms it on drop, also
/// when the parser panics.
struct AbortGuard<'r, 'a> {
    r: &'r mut Reader<'a>,
}

impl<'r, 'a> AbortGuard<'r, 'a> {
    fn new(r: &'r mut Reader<'a>, flag: Rc<Cell<bool>>) -> Self {
        r.set_abort(Some(flag));
        AbortGuard { r }
    }

    fn reader(&mut self) -> &mut Reader<'a> {
        self.r
    }
}

impl Drop for AbortGuard<'_, '_> {
    fn drop(&mut self) {
        self.r.set_abort(None);
    }
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

/// Counts the units a parse builds against [`Limits::max_nodes`]. Once the
/// budget is spent it sets the abort flag and forwards nothing more, so the
/// receiving sink holds at most `max_nodes` units.
struct Budget<'a, S: Sink> {
    inner: &'a mut S,
    left: usize,
    abort: &'a Cell<bool>,
}

impl<S: Sink> Budget<'_, S> {
    /// Spends `weight` units; false (and the abort flag set) when fewer are
    /// left.
    #[inline]
    fn charge(&mut self, weight: usize) -> bool {
        if self.left < weight {
            self.left = 0;
            self.abort.set(true);
            return false;
        }
        self.left -= weight;
        true
    }

    #[inline]
    fn live(&self) -> bool {
        !self.abort.get()
    }
}

impl<S: Sink> Sink for Budget<'_, S> {
    fn undef(&mut self) {
        if self.charge(1) {
            self.inner.undef();
        }
    }
    fn boolean(&mut self, v: bool) {
        if self.charge(1) {
            self.inner.boolean(v);
        }
    }
    fn integer(&mut self, v: i32) {
        if self.charge(1) {
            self.inner.integer(v);
        }
    }
    fn real(&mut self, v: f64) {
        if self.charge(1) {
            self.inner.real(v);
        }
    }
    fn uuid(&mut self, v: &[u8; 16]) {
        if self.charge(1) {
            self.inner.uuid(v);
        }
    }
    fn string(&mut self, v: &[u8]) {
        if self.charge(1) {
            self.inner.string(v);
        }
    }
    fn date(&mut self, seconds: f64) {
        if self.charge(1) {
            self.inner.date(seconds);
        }
    }
    fn uri(&mut self, v: &[u8]) {
        if self.charge(1) {
            self.inner.uri(v);
        }
    }
    fn binary(&mut self, v: &[u8]) {
        if self.charge(1) {
            self.inner.binary(v);
        }
    }
    fn begin_map(&mut self) {
        if self.charge(NODE_WEIGHT_MAP) {
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
        if self.charge(1) {
            self.inner.begin_array();
        }
    }
    fn end_array(&mut self) {
        if self.live() {
            self.inner.end_array();
        }
    }
    fn skipped(&mut self) {
        self.charge(NODE_WEIGHT_SKIPPED_ELEMENT);
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
