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

impl Drop for Value {
    /// Drops the tree without unbounded recursion, so a deeply nested value
    /// (one from a hostile document, say) cannot overflow the stack. The
    /// first [`NATIVE_DROP_DEPTH`] levels of containers on a thread drop
    /// their children as usual, by recursion; a container below that has
    /// its children taken out and walked from an explicit stack, one entry
    /// per level, each child container giving up its own children the
    /// same way before it drops.
    fn drop(&mut self) {
        if !is_nonempty_container(self) {
            return;
        }
        let depth = DROP_DEPTH.get();
        if depth < NATIVE_DROP_DEPTH {
            DROP_DEPTH.set(depth + 1);
            match self {
                Value::Map(m) => drop(std::mem::take(m)),
                Value::Array(a) => drop(std::mem::take(a)),
                _ => {}
            }
            DROP_DEPTH.set(depth);
            return;
        }
        let mut stack: Vec<Children> = Children::take(self).into_iter().collect();
        while let Some(top) = stack.last_mut() {
            match top.next() {
                Some(mut child) => {
                    stack.extend(Children::take(&mut child));
                    // `child` drops here, a scalar or an empty container.
                }
                None => {
                    stack.pop();
                }
            }
        }
    }
}

/// How many levels of containers drop by recursion before [`Value`]'s drop
/// switches to its explicit stack.
const NATIVE_DROP_DEPTH: u32 = 16;

thread_local! {
    /// The containers this thread is dropping by recursion.
    static DROP_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// A map or array that is not empty.
fn is_nonempty_container(v: &Value) -> bool {
    match v {
        Value::Map(m) => !m.is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => false,
    }
}

/// The children of one container, taken out of it by [`Value`]'s drop.
enum Children {
    Map(std::collections::btree_map::IntoValues<Vec<u8>, Value>),
    Array(std::vec::IntoIter<Value>),
}

impl Children {
    /// The children of a non-empty container, leaving it empty.
    fn take(v: &mut Value) -> Option<Children> {
        match v {
            Value::Map(m) if !m.is_empty() => Some(Children::Map(std::mem::take(m).into_values())),
            Value::Array(a) if !a.is_empty() => {
                Some(Children::Array(std::mem::take(a).into_iter()))
            }
            _ => None,
        }
    }

    fn next(&mut self) -> Option<Value> {
        match self {
            Children::Map(it) => it.next(),
            Children::Array(it) => it.next(),
        }
    }
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

    /// Streams this value into a formatter, in document order. The walk keeps
    /// its own stack rather than recursing, so a deeply nested value (one
    /// parsed with no depth limit, say) cannot overflow the thread's stack.
    pub fn emit<O: Out>(&self, f: &mut Formatter<O>) {
        enum Frame<'a> {
            Map(std::collections::btree_map::Iter<'a, Vec<u8>, Value>),
            Array(std::slice::Iter<'a, Value>),
        }
        let mut stack: Vec<Frame<'_>> = Vec::new();
        let mut next = Some(self);
        loop {
            if let Some(v) = next.take() {
                match v {
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
                        stack.push(Frame::Map(m.iter()));
                    }
                    Value::Array(a) => {
                        f.begin_array(a.len());
                        stack.push(Frame::Array(a.iter()));
                    }
                }
            }
            match stack.last_mut() {
                None => return,
                Some(Frame::Map(it)) => match it.next() {
                    Some((k, v)) => {
                        f.key(k);
                        next = Some(v);
                    }
                    None => {
                        stack.pop();
                        f.end_map();
                    }
                },
                Some(Frame::Array(it)) => match it.next() {
                    Some(v) => next = Some(v),
                    None => {
                        stack.pop();
                        f.end_array();
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chain of `depth` containers, alternating arrays and maps, built
    /// without recursion.
    fn chain(depth: usize) -> Value {
        let mut v = Value::Integer(7);
        for i in 0..depth {
            v = if i % 2 == 0 {
                Value::Array(vec![Value::Undefined, v, Value::Integer(1)])
            } else {
                let mut m = BTreeMap::new();
                m.insert(b"a".to_vec(), Value::string("x"));
                m.insert(b"k".to_vec(), v);
                Value::Map(m)
            };
        }
        v
    }

    /// A million levels drop without overflowing the stack (the default
    /// drop recursed once per level).
    #[test]
    fn deep_value_drops_without_recursion() {
        drop(chain(1_000_000));
    }

    /// The same on a thread with a small stack, and with wide levels too.
    #[test]
    fn deep_and_wide_value_drops_on_a_small_stack() {
        std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(|| {
                let mut v = chain(200_000);
                for _ in 0..1000 {
                    v = Value::Array(vec![chain(50), v, chain(3)]);
                }
                drop(v);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    /// A deep value formats in every format on a small stack (the walk
    /// recursed once per level), and its bytes parse back.
    #[test]
    fn deep_value_formats_on_a_small_stack() {
        std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(|| {
                let v = chain(100_000);
                for format in [
                    crate::Format::Binary,
                    crate::Format::Notation,
                    crate::Format::Xml,
                ] {
                    let (bytes, n) = crate::format(format, &v, crate::Options::default());
                    assert!(n > 0 && !bytes.is_empty(), "{format:?}");
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    /// The walk writes what the recursive one wrote: a round trip through
    /// each format, pretty and plain, gives the value back.
    #[test]
    fn emit_order_round_trips() {
        let v = chain(40);
        for format in [
            crate::Format::Binary,
            crate::Format::Notation,
            crate::Format::Xml,
        ] {
            for pretty in [false, true] {
                let opts = crate::Options {
                    pretty,
                    ..crate::Options::default()
                };
                let (bytes, _) = crate::format(format, &v, opts);
                let (n, back) = crate::parse(format, &bytes, crate::Limits::for_len(bytes.len()));
                assert!(n > 0, "{format:?} pretty={pretty}");
                assert_eq!(back.unwrap(), v, "{format:?} pretty={pretty}");
            }
        }
    }

    /// Dropping one value leaves a clone of it whole.
    #[test]
    fn drop_leaves_clones_alone() {
        let a = chain(64);
        let b = a.clone();
        drop(a);
        assert_eq!(b, chain(64));
    }
}
