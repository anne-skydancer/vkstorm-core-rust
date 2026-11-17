//! The node budget: a payload of tiny values fails once it builds more than
//! `Limits::max_nodes` values, cheaply, in every format; payloads within the
//! budget parse as before.

use vkstorm_llsd::{
    DEFAULT_MAX_NODES, Format, Limits, PARSE_FAILURE, Reader, SliceSource, Value, ValueBuilder,
    XmlMode, key_policy, parse, parse_into,
};

fn limits(max_nodes: usize) -> Limits {
    Limits {
        max_bytes: None,
        max_depth: -1,
        max_nodes,
    }
}

/// `[` count, `count` undefs, `]`: `count + 1` values.
fn binary_array_of_undefs(count: usize) -> Vec<u8> {
    let mut v = vec![b'['];
    v.extend_from_slice(&(count as i32).to_be_bytes());
    v.resize(v.len() + count, b'!');
    v.push(b']');
    v
}

fn notation_array_of_undefs(count: usize) -> Vec<u8> {
    let mut v = vec![b'['];
    for i in 0..count {
        if i > 0 {
            v.push(b',');
        }
        v.push(b'!');
    }
    v.push(b']');
    v
}

fn xml_array_of_undefs(count: usize) -> Vec<u8> {
    let mut v = b"<llsd><array>".to_vec();
    for _ in 0..count {
        v.extend_from_slice(b"<undef/>");
    }
    v.extend_from_slice(b"</array></llsd>");
    v
}

/// `count` nested one-element arrays around an undef: `count + 1` values.
fn binary_nested(count: usize) -> Vec<u8> {
    let mut v = Vec::new();
    for _ in 0..count {
        v.push(b'[');
        v.extend_from_slice(&1i32.to_be_bytes());
    }
    v.push(b'!');
    v.resize(v.len() + count, b']');
    v
}

/// A map of `count` keys, each an undef: `count + 1` values.
fn binary_map_of_undefs(count: usize) -> Vec<u8> {
    let mut v = vec![b'{'];
    v.extend_from_slice(&(count as i32).to_be_bytes());
    for i in 0..count {
        v.push(b'k');
        v.extend_from_slice(&4i32.to_be_bytes());
        v.extend_from_slice(&(i as u32).to_be_bytes());
        v.push(b'!');
    }
    v.push(b'}');
    v
}

fn run(format: Format, input: &[u8], max_nodes: usize) -> (i32, Option<Value>, usize) {
    let mut src = SliceSource::new(input);
    let mut r = Reader::new(&mut src);
    let mut b = ValueBuilder::new(key_policy(format));
    let n = parse_into(format, &mut r, &mut b, limits(max_nodes), XmlMode::Parse);
    let consumed = r.consumed();
    (n, b.finish(), consumed)
}

#[test]
fn exactly_the_budget_parses_one_more_fails() {
    for (format, make) in [
        (
            Format::Binary,
            binary_array_of_undefs as fn(usize) -> Vec<u8>,
        ),
        (Format::Notation, notation_array_of_undefs),
        (Format::Xml, xml_array_of_undefs),
        (Format::Binary, binary_nested),
        (Format::Binary, binary_map_of_undefs),
    ] {
        let at = run(format, &make(99), 100);
        assert!(at.0 > 0 && at.1.is_some(), "{format:?}: 100 values in 100");
        let over = run(format, &make(100), 100);
        assert_eq!(
            (over.0, over.1),
            (PARSE_FAILURE, None),
            "{format:?}: 101 in 100"
        );
    }
}

#[test]
fn zero_is_unlimited() {
    let input = binary_array_of_undefs(300_000);
    let (n, v, _) = run(Format::Binary, &input, 0);
    assert!(n > 0);
    assert!(matches!(&v, Some(Value::Array(a)) if a.len() == 300_000));
}

#[test]
fn an_exhausted_budget_stops_reading_early() {
    // One million undefs against a budget of 10: the parser must not read on
    // through the whole payload (it ends within a buffered window).
    let input = binary_array_of_undefs(1_000_000);
    let (n, _, consumed) = run(Format::Binary, &input, 10);
    assert_eq!(n, PARSE_FAILURE);
    assert!(consumed < 200_000, "read {consumed} of {}", input.len());
}

#[test]
fn the_default_budget_stops_a_large_payload_of_tiny_values() {
    // 8 MiB of `!` would build 2^23 + 1 values (several hundred MiB of heap).
    let n = DEFAULT_MAX_NODES;
    for input in [binary_array_of_undefs(n), binary_nested(n)] {
        let (count, v) = parse(Format::Binary, &input, Limits::for_len(input.len()));
        assert_eq!((count, v), (PARSE_FAILURE, None));
    }
}

#[test]
fn a_realistic_large_document_is_within_the_default_budget() {
    // About 2M values, as a large AIS reply: parses under the default.
    let input = binary_array_of_undefs(2_000_000);
    let (count, v) = parse(Format::Binary, &input, Limits::for_len(input.len()));
    assert!(count > 0);
    assert!(matches!(&v, Some(Value::Array(a)) if a.len() == 2_000_000));
}

#[test]
fn unlimited_limits_have_no_budget() {
    let n = 1_000;
    let input = binary_array_of_undefs(n);
    let (count, v) = parse(Format::Binary, &input, Limits::UNLIMITED);
    assert!(count > 0);
    assert!(v.is_some());
    assert_eq!(Limits::UNLIMITED.max_nodes, 0);
    assert_eq!(Limits::from_cpp(-1, -1).max_nodes, DEFAULT_MAX_NODES);
}
