//! An owned LLSD value tree. The viewer keeps its own in-memory `LLSD` class;
//! this tree exists for the crate's tests, tools and any Rust-side user. The
//! parsers stream into a [`Sink`](crate::Sink) and do not require it.

use std::collections::BTreeMap;

use crate::format::{Formatter, Out};

/// One LLSD value. Strings are byte strings: the viewer's `LLSD::String` is a
/// `std::string`, and the notation format can carry arbitrary bytes through
/// `\xHH` escapes. Map keys are ordered byte-wise, as `std::map<std::string, LLSD>`.
#[derive(Clone, Debug)]
pub enum Value {
    Undefined,
    Boolean(bool),
    Integer(i32),
    Real(f64),
    String(Vec<u8>),
    Uuid([u8; 16]),
    /// Seconds since the Unix epoch, as `LLDate` stores it.
    Date(f64),
    Uri(Vec<u8>),
    Binary(Vec<u8>),
    Map(BTreeMap<Vec<u8>, Value>),
    Array(Vec<Value>),
}

impl PartialEq for Value {
    /// Reals and dates compare bit-wise, so NaN payloads round-trip in tests.
    fn eq(&self, other: &Self) -> bool {
        use Value::*;
        match (self, other) {
            (Undefined, Undefined) => true,
            (Boolean(a), Boolean(b)) => a == b,
            (Integer(a), Integer(b)) => a == b,
            (Real(a), Real(b)) => a.to_bits() == b.to_bits(),
            (String(a), String(b)) => a == b,
            (Uuid(a), Uuid(b)) => a == b,
            (Date(a), Date(b)) => a.to_bits() == b.to_bits(),
            (Uri(a), Uri(b)) => a == b,
            (Binary(a), Binary(b)) => a == b,
            (Map(a), Map(b)) => a == b,
            (Array(a), Array(b)) => a == b,
            _ => false,
        }
    }
}

impl Value {
    /// Builds an empty map.
    pub fn map() -> Value {
        Value::Map(BTreeMap::new())
    }

    /// Builds an empty array.
    pub fn array() -> Value {
        Value::Array(Vec::new())
    }

    /// A string value from `&str`.
    pub fn string(s: &str) -> Value {
        Value::String(s.as_bytes().to_vec())
    }

    /// The number of entries of a map or array, else 0.
    pub fn len(&self) -> usize {
        match self {
            Value::Map(m) => m.len(),
            Value::Array(a) => a.len(),
            _ => 0,
        }
    }

    /// True for an empty container or any scalar.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Streams this value into a formatter, in document order.
    pub fn emit<O: Out>(&self, f: &mut Formatter<O>) {
        match self {
            Value::Undefined => f.undef(),
            Value::Boolean(b) => f.boolean(*b),
            Value::Integer(i) => f.integer(*i),
            Value::Real(r) => f.real(*r),
            Value::String(s) => f.string(s),
            Value::Uuid(u) => f.uuid(u),
            Value::Date(d) => f.date(*d),
            Value::Uri(u) => f.uri(u),
            Value::Binary(b) => f.binary(b),
            Value::Map(m) => {
                f.begin_map(m.len());
                for (k, v) in m {
                    f.key(k);
                    v.emit(f);
                }
                f.end_map();
            }
            Value::Array(a) => {
                f.begin_array(a.len());
                for v in a {
                    v.emit(f);
                }
                f.end_array();
            }
        }
    }
}
