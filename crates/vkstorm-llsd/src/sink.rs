//! Parse-event receivers. The parsers stream their results as events in
//! document order; [`ValueBuilder`] turns them into a [`Value`] tree and
//! [`FlatSink`] into the flat, tagged node buffer the viewer's C ABI hands to
//! C++ for construction of its own `LLSD` objects.

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
            doc: FlatDoc::default(),
            open: Vec::new(),
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

/// Rebuilds a [`Value`] from a flat document (used by tests to check the
/// buffer the viewer consumes).
pub fn value_from_flat(doc: &FlatDoc, policy: KeyPolicy) -> Option<Value> {
    let mut b = ValueBuilder::new(policy);
    let mut i = 0;
    let bytes = |n: &FlatNode| -> &[u8] {
        let o = n.payload as usize;
        &doc.bytes[o..o + n.len as usize]
    };
    // Explicit stack of remaining-entries counts for the open containers.
    let mut remaining: Vec<(u8, u32)> = Vec::new();
    while i < doc.nodes.len() {
        let n = &doc.nodes[i];
        i += 1;
        match n.kind {
            kind::UNDEF => b.undef(),
            kind::BOOLEAN => b.boolean(n.payload != 0),
            kind::INTEGER => b.integer(n.payload as i64 as i32),
            kind::REAL => b.real(f64::from_bits(n.payload)),
            kind::STRING => b.string(bytes(n)),
            kind::UUID => {
                let mut u = [0u8; 16];
                u.copy_from_slice(bytes(n));
                b.uuid(&u)
            }
            kind::DATE => b.date(f64::from_bits(n.payload)),
            kind::URI => b.uri(bytes(n)),
            kind::BINARY => b.binary(bytes(n)),
            kind::KEY => {
                b.key(bytes(n));
                continue;
            }
            kind::MAP => {
                b.begin_map();
                remaining.push((kind::MAP, n.len));
                if n.len == 0 {
                    b.end_map();
                    remaining.pop();
                } else {
                    continue;
                }
            }
            kind::ARRAY => {
                b.begin_array();
                remaining.push((kind::ARRAY, n.len));
                if n.len == 0 {
                    b.end_array();
                    remaining.pop();
                } else {
                    continue;
                }
            }
            _ => return None,
        }
        // A value completed: close every container that just became full.
        while let Some((k, left)) = remaining.last_mut() {
            *left -= 1;
            if *left == 0 {
                let k = *k;
                remaining.pop();
                if k == kind::MAP {
                    b.end_map();
                } else {
                    b.end_array();
                }
            } else {
                break;
            }
        }
    }
    b.finish()
}
