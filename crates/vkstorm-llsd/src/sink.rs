//! Parse-event receivers. The parsers stream their results as events in
//! document order; [`ValueBuilder`] turns them into a [`Value`] tree and
//! [`FlatSink`] into the flat, tagged node buffer the viewer's C ABI hands to
//! C++ for construction of its own `LLSD` objects. [`flat_from_value`] and
//! [`value_from_flat_parts`] convert between that buffer and a [`Value`], for
//! the C ABI calls that take an `LLSD` value from C++ or return one.

use std::collections::BTreeMap;

use crate::value::Value;

/// Receives one parsed LLSD document as events.
///
/// Container events bracket their children. A map key event always precedes
/// the value it names; a parser never emits a key without a following value.
pub trait Sink {
    fn undef(&mut self);
    fn boolean(&mut self, v: bool);
    fn integer(&mut self, v: i32);
    fn real(&mut self, v: f64);
    fn uuid(&mut self, v: &[u8; 16]);
    fn string(&mut self, v: &[u8]);
    fn date(&mut self, seconds: f64);
    fn uri(&mut self, v: &[u8]);
    fn binary(&mut self, v: &[u8]);
    fn begin_map(&mut self);
    fn key(&mut self, k: &[u8]);
    fn end_map(&mut self);
    fn begin_array(&mut self);
    fn end_array(&mut self);
}

/// What a map does with a repeated key. The viewer's notation and binary
/// parsers call `LLSD::insert` (`std::map::emplace`: the first value stays);
/// its XML parser assigns through `operator[]` (the last value wins).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPolicy {
    FirstWins,
    LastWins,
}

enum Frame {
    Map {
        map: BTreeMap<Vec<u8>, Value>,
        key: Option<Vec<u8>>,
    },
    Array(Vec<Value>),
}

/// Builds a [`Value`] from parse events.
pub struct ValueBuilder {
    policy: KeyPolicy,
    stack: Vec<Frame>,
    root: Option<Value>,
}

impl ValueBuilder {
    pub fn new(policy: KeyPolicy) -> Self {
        ValueBuilder {
            policy,
            stack: Vec::new(),
            root: None,
        }
    }

    /// The finished tree, or `None` when no value was delivered.
    pub fn finish(self) -> Option<Value> {
        self.root
    }

    fn place(&mut self, v: Value) {
        match self.stack.last_mut() {
            None => self.root = Some(v),
            Some(Frame::Array(a)) => a.push(v),
            Some(Frame::Map { map, key }) => {
                let k = key.take().expect("map value without key");
                match self.policy {
                    KeyPolicy::FirstWins => {
                        map.entry(k).or_insert(v);
                    }
                    KeyPolicy::LastWins => {
                        map.insert(k, v);
                    }
                }
            }
        }
    }
}

impl Sink for ValueBuilder {
    fn undef(&mut self) {
        self.place(Value::Undefined)
    }
    fn boolean(&mut self, v: bool) {
        self.place(Value::Boolean(v))
    }
    fn integer(&mut self, v: i32) {
        self.place(Value::Integer(v))
    }
    fn real(&mut self, v: f64) {
        self.place(Value::Real(v))
    }
    fn uuid(&mut self, v: &[u8; 16]) {
        self.place(Value::Uuid(*v))
    }
    fn string(&mut self, v: &[u8]) {
        self.place(Value::String(v.to_vec()))
    }
    fn date(&mut self, seconds: f64) {
        self.place(Value::Date(seconds))
    }
    fn uri(&mut self, v: &[u8]) {
        self.place(Value::Uri(v.to_vec()))
    }
    fn binary(&mut self, v: &[u8]) {
        self.place(Value::Binary(v.to_vec()))
    }
    fn begin_map(&mut self) {
        self.stack.push(Frame::Map {
            map: BTreeMap::new(),
            key: None,
        });
    }
    fn key(&mut self, k: &[u8]) {
        if let Some(Frame::Map { key, .. }) = self.stack.last_mut() {
            *key = Some(k.to_vec());
        }
    }
    fn end_map(&mut self) {
        if let Some(Frame::Map { map, .. }) = self.stack.pop() {
            self.place(Value::Map(map));
        }
    }
    fn begin_array(&mut self) {
        self.stack.push(Frame::Array(Vec::new()));
    }
    fn end_array(&mut self) {
        if let Some(Frame::Array(a)) = self.stack.pop() {
            self.place(Value::Array(a));
        }
    }
}

/// Node kinds of the flat buffer. Values are stable: the C++ side switches on them.
pub mod kind {
    pub const UNDEF: u8 = 0;
    pub const BOOLEAN: u8 = 1;
    pub const INTEGER: u8 = 2;
    pub const REAL: u8 = 3;
    pub const STRING: u8 = 4;
    pub const UUID: u8 = 5;
    pub const DATE: u8 = 6;
    pub const URI: u8 = 7;
    pub const BINARY: u8 = 8;
    pub const MAP: u8 = 9;
    pub const ARRAY: u8 = 10;
    pub const KEY: u8 = 11;
}

/// One node of the flat buffer: 16 bytes, C layout.
///
/// `len` is the entry count of a map or array and the byte length of a
/// string, URI, binary, key or UUID (16). `payload` is the integer or boolean
/// (as `i64`), the real or date (`f64` bits), or the byte offset into the
/// document's byte arena for the length-carrying kinds. A map's `len` entries
/// follow it as `KEY` node, value subtree pairs; an array's as value subtrees.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FlatNode {
    pub kind: u8,
    pub pad: [u8; 3],
    pub len: u32,
    pub payload: u64,
}

/// A parsed document as a node buffer plus a byte arena.
#[derive(Default, Debug)]
pub struct FlatDoc {
    pub nodes: Vec<FlatNode>,
    pub bytes: Vec<u8>,
}

/// Builds a [`FlatDoc`] from parse events.
pub struct FlatSink {
    doc: FlatDoc,
    /// Index of the open containers, to patch their counts at close.
    open: Vec<(usize, u32)>,
}

impl FlatSink {
    pub fn new() -> Self {
        FlatSink {
            doc: FlatDoc {
                nodes: Vec::with_capacity(64),
                bytes: Vec::with_capacity(512),
            },
            open: Vec::with_capacity(8),
        }
    }

    pub fn finish(self) -> FlatDoc {
        self.doc
    }

    fn push(&mut self, kind: u8, len: u32, payload: u64) {
        self.doc.nodes.push(FlatNode {
            kind,
            pad: [0; 3],
            len,
            payload,
        });
    }

    fn push_bytes(&mut self, kind: u8, bytes: &[u8]) {
        let ofs = self.doc.bytes.len() as u64;
        self.doc.bytes.extend_from_slice(bytes);
        self.push(kind, bytes.len() as u32, ofs);
    }

    fn count_value(&mut self) {
        if let Some((_, n)) = self.open.last_mut() {
            *n += 1;
        }
    }
}

impl Default for FlatSink {
    fn default() -> Self {
        Self::new()
    }
}

impl Sink for FlatSink {
    fn undef(&mut self) {
        self.count_value();
        self.push(kind::UNDEF, 0, 0);
    }
    fn boolean(&mut self, v: bool) {
        self.count_value();
        self.push(kind::BOOLEAN, 0, v as u64);
    }
    fn integer(&mut self, v: i32) {
        self.count_value();
        self.push(kind::INTEGER, 0, v as i64 as u64);
    }
    fn real(&mut self, v: f64) {
        self.count_value();
        self.push(kind::REAL, 0, v.to_bits());
    }
    fn uuid(&mut self, v: &[u8; 16]) {
        self.count_value();
        self.push_bytes(kind::UUID, v);
    }
    fn string(&mut self, v: &[u8]) {
        self.count_value();
        self.push_bytes(kind::STRING, v);
    }
    fn date(&mut self, seconds: f64) {
        self.count_value();
        self.push(kind::DATE, 0, seconds.to_bits());
    }
    fn uri(&mut self, v: &[u8]) {
        self.count_value();
        self.push_bytes(kind::URI, v);
    }
    fn binary(&mut self, v: &[u8]) {
        self.count_value();
        self.push_bytes(kind::BINARY, v);
    }
    fn begin_map(&mut self) {
        self.count_value();
        self.open.push((self.doc.nodes.len(), 0));
        self.push(kind::MAP, 0, 0);
    }
    fn key(&mut self, k: &[u8]) {
        self.push_bytes(kind::KEY, k);
    }
    fn end_map(&mut self) {
        if let Some((at, n)) = self.open.pop() {
            self.doc.nodes[at].len = n;
        }
    }
    fn begin_array(&mut self) {
        self.count_value();
        self.open.push((self.doc.nodes.len(), 0));
        self.push(kind::ARRAY, 0, 0);
    }
    fn end_array(&mut self) {
        if let Some((at, n)) = self.open.pop() {
            self.doc.nodes[at].len = n;
        }
    }
}

/// Rebuilds a [`Value`] from a flat document: [`value_from_flat_parts`] over
/// its nodes and arena.
pub fn value_from_flat(doc: &FlatDoc, policy: KeyPolicy) -> Option<Value> {
    value_from_flat_parts(&doc.nodes, &doc.bytes, policy)
}

/// Rebuilds a [`Value`] from a node buffer and its byte arena (the layout of
/// [`FlatNode`]): the inverse of [`flat_from_value`] and of a [`FlatSink`].
///
/// This is how the viewer's C ABI takes an `LLSD` value from C++ without
/// coercing it: the C++ side flattens its tree into this layout and the Rust
/// side rebuilds the tree. `policy` decides a repeated map key.
///
/// The buffer must hold exactly one well-formed value; otherwise the result
/// is `None`: no nodes, an unknown kind, a byte range outside the arena, a
/// UUID that is not 16 bytes, a key where a value belongs or a value where a
/// key belongs, a container whose entries run past the last node, or nodes
/// after the root value. The `pad` bytes are ignored. The walk is iterative,
/// so the depth is bounded by memory only.
pub fn value_from_flat_parts(nodes: &[FlatNode], bytes: &[u8], policy: KeyPolicy) -> Option<Value> {
    /// An open container: a map or an array, the entries still to come, and
    /// for a map whether its next node is a key.
    struct Open {
        map: bool,
        left: u32,
        want_key: bool,
    }
    let mut b = ValueBuilder::new(policy);
    let mut open: Vec<Open> = Vec::new();
    let mut done = false;
    for n in nodes {
        if done {
            return None;
        }
        let data = || -> Option<&[u8]> {
            let start = usize::try_from(n.payload).ok()?;
            let end = start.checked_add(usize::try_from(n.len).ok()?)?;
            bytes.get(start..end)
        };
        let in_map_wanting_key = matches!(open.last(), Some(o) if o.map && o.want_key);
        if n.kind == kind::KEY {
            if !in_map_wanting_key {
                return None;
            }
            b.key(data()?);
            if let Some(o) = open.last_mut() {
                o.want_key = false;
            }
            continue;
        }
        if in_map_wanting_key {
            return None;
        }
        match n.kind {
            kind::UNDEF => b.undef(),
            kind::BOOLEAN => b.boolean(n.payload != 0),
            kind::INTEGER => b.integer(n.payload as i64 as i32),
            kind::REAL => b.real(f64::from_bits(n.payload)),
            kind::STRING => b.string(data()?),
            kind::UUID => b.uuid(data()?.try_into().ok()?),
            kind::DATE => b.date(f64::from_bits(n.payload)),
            kind::URI => b.uri(data()?),
            kind::BINARY => b.binary(data()?),
            kind::MAP | kind::ARRAY => {
                let map = n.kind == kind::MAP;
                if map {
                    b.begin_map();
                } else {
                    b.begin_array();
                }
                if n.len > 0 {
                    open.push(Open {
                        map,
                        left: n.len,
                        want_key: map,
                    });
                    continue;
                }
                if map {
                    b.end_map();
                } else {
                    b.end_array();
                }
            }
            _ => return None,
        }
        // A value completed: close every container that just became full.
        loop {
            let Some(o) = open.last_mut() else {
                done = true;
                break;
            };
            o.left -= 1;
            o.want_key = o.map;
            if o.left > 0 {
                break;
            }
            let map = o.map;
            open.pop();
            if map {
                b.end_map();
            } else {
                b.end_array();
            }
        }
    }
    if done { b.finish() } else { None }
}

/// The events of a value that is not a container.
fn emit_scalar<S: Sink>(sink: &mut S, v: &Value) {
    match v {
        Value::Undefined => sink.undef(),
        Value::Boolean(b) => sink.boolean(*b),
        Value::Integer(i) => sink.integer(*i),
        Value::Real(r) => sink.real(*r),
        Value::String(s) => sink.string(s),
        Value::Uuid(u) => sink.uuid(u),
        Value::Date(d) => sink.date(*d),
        Value::Uri(u) => sink.uri(u),
        Value::Binary(b) => sink.binary(b),
        Value::Map(_) | Value::Array(_) => unreachable!("flat_from_value walks the containers"),
    }
}

/// Flattens a [`Value`] into a [`FlatDoc`]: the events a parser delivering
/// the value would send, into a [`FlatSink`], so the buffer is the one
/// `vks_llsd_parse` hands to C++ for the same document. Map entries come in
/// key order.
///
/// This is how the viewer's C ABI returns an `LLSD` value to C++. The walk is
/// iterative, so the depth is bounded by memory only.
pub fn flat_from_value(value: &Value) -> FlatDoc {
    enum Frame<'a> {
        Map(std::collections::btree_map::Iter<'a, Vec<u8>, Value>),
        Array(std::slice::Iter<'a, Value>),
    }
    let mut sink = FlatSink::new();
    let mut stack: Vec<Frame<'_>> = Vec::new();
    let mut next = Some(value);
    loop {
        if let Some(v) = next.take() {
            match v {
                Value::Map(m) => {
                    sink.begin_map();
                    stack.push(Frame::Map(m.iter()));
                }
                Value::Array(a) => {
                    sink.begin_array();
                    stack.push(Frame::Array(a.iter()));
                }
                scalar => emit_scalar(&mut sink, scalar),
            }
        }
        match stack.last_mut() {
            None => break,
            Some(Frame::Map(entries)) => match entries.next() {
                Some((k, v)) => {
                    sink.key(k);
                    next = Some(v);
                }
                None => {
                    stack.pop();
                    sink.end_map();
                }
            },
            Some(Frame::Array(entries)) => match entries.next() {
                Some(v) => next = Some(v),
                None => {
                    stack.pop();
                    sink.end_array();
                }
            },
        }
    }
    sink.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Format, Limits, Reader, SliceSource, XmlMode, parse_into};

    fn node(kind: u8, len: u32, payload: u64) -> FlatNode {
        FlatNode {
            kind,
            pad: [0; 3],
            len,
            payload,
        }
    }

    fn same_nodes(a: &[FlatNode], b: &[FlatNode]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| x.kind == y.kind && x.len == y.len && x.payload == y.payload)
    }

    fn sample() -> Value {
        let mut inner = BTreeMap::new();
        inner.insert(b"nul\0key".to_vec(), Value::String(b"a\0b\xff".to_vec()));
        inner.insert(b"".to_vec(), Value::map());
        inner.insert(b"empty array".to_vec(), Value::array());
        let mut root = BTreeMap::new();
        root.insert(b"undef".to_vec(), Value::Undefined);
        root.insert(b"bool".to_vec(), Value::Boolean(true));
        root.insert(b"int".to_vec(), Value::Integer(i32::MIN));
        root.insert(
            b"real".to_vec(),
            Value::Real(f64::from_bits(0x7ff8_0000_dead_beef)),
        );
        root.insert(b"string".to_vec(), Value::String(Vec::new()));
        root.insert(b"uuid".to_vec(), Value::Uuid([0xab; 16]));
        root.insert(b"date".to_vec(), Value::Date(-1.5));
        root.insert(b"uri".to_vec(), Value::Uri(b"http://x/\0".to_vec()));
        root.insert(b"binary".to_vec(), Value::Binary((0..=255).collect()));
        root.insert(b"map".to_vec(), Value::Map(inner));
        root.insert(
            b"array".to_vec(),
            Value::Array(vec![
                Value::Integer(1),
                Value::Array(vec![Value::Undefined]),
                Value::map(),
            ]),
        );
        Value::Map(root)
    }

    #[test]
    fn round_trip() {
        let values = [
            sample(),
            Value::Undefined,
            Value::Integer(7),
            Value::map(),
            Value::array(),
            Value::Binary(Vec::new()),
        ];
        for v in values {
            let doc = flat_from_value(&v);
            assert_eq!(value_from_flat(&doc, KeyPolicy::LastWins), Some(v.clone()));
            assert_eq!(value_from_flat(&doc, KeyPolicy::FirstWins), Some(v));
        }
    }

    #[test]
    fn matches_the_parser_buffer() {
        let text: &[u8] = br#"{'a':[i1,r2.5,'x',u01234567-89ab-cdef-0123-456789abcdef,d"2026-10-31T00:00:00Z",!,true,b16"00ff",l"u"],'b':{},'c':[]}"#;
        let mut src = SliceSource::new(text);
        let mut reader = Reader::with_prefix(&[], &mut src);
        let mut sink = FlatSink::new();
        assert!(
            parse_into(
                Format::Notation,
                &mut reader,
                &mut sink,
                Limits::UNLIMITED,
                XmlMode::Parse
            ) > 0
        );
        let parsed = sink.finish();
        let v = value_from_flat(&parsed, KeyPolicy::FirstWins).unwrap();
        let doc = flat_from_value(&v);
        assert!(same_nodes(&doc.nodes, &parsed.nodes));
        assert_eq!(doc.bytes, parsed.bytes);
    }

    #[test]
    fn deep_nesting() {
        // On a large stack: the walks are iterative, but Value's comparison
        // (and, without a custom Drop, its drop) recurse.
        std::thread::Builder::new()
            .stack_size(1 << 30)
            .spawn(|| {
                let depth = 50_000;
                let mut v = Value::Integer(1);
                for i in 0..depth {
                    v = if i % 2 == 0 {
                        Value::Array(vec![v])
                    } else {
                        let mut m = BTreeMap::new();
                        m.insert(b"k".to_vec(), v);
                        Value::Map(m)
                    };
                }
                let doc = flat_from_value(&v);
                // One node per container, one key node per map, the leaf.
                assert_eq!(doc.nodes.len(), depth + depth / 2 + 1);
                assert_eq!(value_from_flat(&doc, KeyPolicy::LastWins), Some(v));
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn repeated_keys_follow_the_policy() {
        let bytes = b"kk";
        let nodes = [
            node(kind::MAP, 2, 0),
            node(kind::KEY, 1, 0),
            node(kind::INTEGER, 0, 1),
            node(kind::KEY, 1, 1),
            node(kind::INTEGER, 0, 2),
        ];
        let one = |i| {
            let mut m = BTreeMap::new();
            m.insert(b"k".to_vec(), Value::Integer(i));
            Value::Map(m)
        };
        assert_eq!(
            value_from_flat_parts(&nodes, bytes, KeyPolicy::FirstWins),
            Some(one(1))
        );
        assert_eq!(
            value_from_flat_parts(&nodes, bytes, KeyPolicy::LastWins),
            Some(one(2))
        );
    }

    #[test]
    fn malformed_buffers_are_refused() {
        let bytes = [0u8; 16];
        let none = |nodes: &[FlatNode]| {
            value_from_flat_parts(nodes, &bytes, KeyPolicy::LastWins).is_none()
        };
        // No value, or more than one.
        assert!(none(&[]));
        assert!(none(&[node(kind::UNDEF, 0, 0), node(kind::UNDEF, 0, 0)]));
        // An unknown kind.
        assert!(none(&[node(12, 0, 0)]));
        // Bytes outside the arena, and an offset that overflows.
        assert!(none(&[node(kind::STRING, 17, 0)]));
        assert!(none(&[node(kind::STRING, 1, 16)]));
        assert!(none(&[node(kind::BINARY, 2, u64::MAX)]));
        // A UUID of the wrong length.
        assert!(none(&[node(kind::UUID, 15, 0)]));
        // Keys outside maps, a value where a key belongs, a key where a value
        // belongs.
        assert!(none(&[node(kind::KEY, 1, 0)]));
        assert!(none(&[node(kind::ARRAY, 1, 0), node(kind::KEY, 1, 0)]));
        assert!(none(&[node(kind::MAP, 1, 0), node(kind::INTEGER, 0, 0)]));
        assert!(none(&[
            node(kind::MAP, 1, 0),
            node(kind::KEY, 1, 0),
            node(kind::KEY, 1, 0),
            node(kind::UNDEF, 0, 0)
        ]));
        // Containers that run past the last node.
        assert!(none(&[node(kind::ARRAY, 2, 0), node(kind::UNDEF, 0, 0)]));
        assert!(none(&[node(kind::MAP, 1, 0), node(kind::KEY, 1, 0)]));
        // Well formed, for contrast.
        assert!(!none(&[node(kind::ARRAY, 1, 0), node(kind::UUID, 16, 0)]));
    }
}
