// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026, Anne Skydancer (Vulkanstorm)
//! The node budget: a payload of tiny values fails once it builds more than
//! `Limits::max_nodes` units (a map costs 4, every other value 1), cheaply,
//! in every format and from every kind of source; payloads within the budget
//! parse as before.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use vkstorm_llsd::{
    ASSET_MAX_NODES, DEFAULT_MAX_NODES, Format, Limits, NODE_WEIGHT_MAP,
    NODE_WEIGHT_SKIPPED_ELEMENT, PARSE_FAILURE, Reader, SliceSource, Source, Value, ValueBuilder,
    XmlMode, http_max_nodes, key_policy, parse, parse_into,
};

fn limits(max_nodes: usize) -> Limits {
    Limits {
        max_bytes: None,
        max_depth: -1,
        max_nodes,
    }
}

/// `[` count, `count` undefs, `]`: `count + 1` units.
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

/// `count` nested one-element arrays around an undef: `count + 1` units.
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

/// A map of `count` keys, each an undef: `4 + count` units.
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

/// An array of `count` empty maps: `1 + 4 * count` units.
fn binary_array_of_maps(count: usize) -> Vec<u8> {
    let mut v = vec![b'['];
    v.extend_from_slice(&(count as i32).to_be_bytes());
    for _ in 0..count {
        v.push(b'{');
        v.extend_from_slice(&0i32.to_be_bytes());
        v.push(b'}');
    }
    v.push(b']');
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
    ] {
        // `n` undefs and their array: n + 1 units.
        let at = run(format, &make(99), 100);
        assert!(at.0 > 0 && at.1.is_some(), "{format:?}: 100 units in 100");
        let over = run(format, &make(100), 100);
        assert_eq!(
            (over.0, over.1),
            (PARSE_FAILURE, None),
            "{format:?}: 101 units in 100"
        );
    }
}

#[test]
fn a_map_costs_four_units_everything_else_one() {
    assert_eq!(NODE_WEIGHT_MAP, 4);
    // A map of 96 undefs: 4 + 96 = 100 units.
    let at = run(Format::Binary, &binary_map_of_undefs(96), 100);
    assert!(at.0 > 0, "100 units in 100");
    let over = run(Format::Binary, &binary_map_of_undefs(97), 100);
    assert_eq!(over.0, PARSE_FAILURE, "101 units in 100");
    // An array of 24 empty maps: 1 + 4 * 24 = 97 units; 25 maps: 101.
    assert!(run(Format::Binary, &binary_array_of_maps(24), 100).0 > 0);
    assert_eq!(
        run(Format::Binary, &binary_array_of_maps(25), 100).0,
        PARSE_FAILURE
    );
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
    // through the whole payload; it ends within a buffered window.
    for (format, input) in [
        (Format::Binary, binary_array_of_undefs(1_000_000)),
        (Format::Notation, notation_array_of_undefs(1_000_000)),
        (Format::Xml, xml_array_of_undefs(1_000_000)),
    ] {
        let (n, _, consumed) = run(format, &input, 10);
        assert_eq!(n, PARSE_FAILURE, "{format:?}");
        assert!(
            consumed < 4 * 1024 + 256,
            "{format:?}: read {consumed} of {}",
            input.len()
        );
    }
}

#[test]
fn xml_lines_mode_over_budget_fails() {
    let input = xml_array_of_undefs(1_000);
    let mut src = SliceSource::new(&input);
    let mut r = Reader::new(&mut src);
    let mut b = ValueBuilder::new(key_policy(Format::Xml));
    let n = parse_into(Format::Xml, &mut r, &mut b, limits(100), XmlMode::Lines);
    assert_eq!(n, PARSE_FAILURE);
    // And within the budget it parses in the same mode.
    let mut src = SliceSource::new(&input);
    let mut r = Reader::new(&mut src);
    let mut b = ValueBuilder::new(key_policy(Format::Xml));
    let n = parse_into(Format::Xml, &mut r, &mut b, limits(2_000), XmlMode::Lines);
    assert!(n > 0);
}

#[test]
fn a_second_parse_gets_a_fresh_budget() {
    let input = binary_array_of_undefs(60);
    for _ in 0..3 {
        // Each parse spends 61 units of 100; none inherits the last one's.
        let (n, v, _) = run(Format::Binary, &input, 100);
        assert!(n > 0 && v.is_some());
    }
    // The same Reader reused for a second document gets a fresh budget too.
    let two = [binary_array_of_undefs(60), binary_array_of_undefs(60)].concat();
    let mut src = SliceSource::new(&two);
    let mut r = Reader::new(&mut src);
    for _ in 0..2 {
        let mut b = ValueBuilder::new(key_policy(Format::Binary));
        let n = parse_into(Format::Binary, &mut r, &mut b, limits(100), XmlMode::Parse);
        assert!(n > 0 && b.finish().is_some());
    }
}

#[test]
fn the_default_budget_stops_a_large_payload_of_tiny_values() {
    // 8 MiB of `!` would build 2^23 + 1 units (hundreds of MiB of heap).
    let n = DEFAULT_MAX_NODES;
    for input in [binary_array_of_undefs(n), binary_nested(n)] {
        let (count, v) = parse(Format::Binary, &input, Limits::for_len(input.len()));
        assert_eq!((count, v), (PARSE_FAILURE, None));
    }
}

#[test]
fn a_realistic_large_document_is_within_the_default_budget() {
    // About 2M units, as a large AIS reply: parses under the default.
    let input = binary_array_of_undefs(2_000_000);
    let (count, v) = parse(Format::Binary, &input, Limits::for_len(input.len()));
    assert!(count > 0);
    assert!(matches!(&v, Some(Value::Array(a)) if a.len() == 2_000_000));
}

#[test]
fn the_budget_tiers() {
    assert_eq!(DEFAULT_MAX_NODES, 1 << 23);
    assert_eq!(ASSET_MAX_NODES, 1 << 20);
    assert_eq!(http_max_nodes(0), DEFAULT_MAX_NODES);
    assert_eq!(http_max_nodes(1 << 20), DEFAULT_MAX_NODES);
    assert_eq!(http_max_nodes(256 << 20), 16 << 20);
    assert_eq!(Limits::UNLIMITED.max_nodes, 0);
    assert_eq!(Limits::for_len(10).max_nodes, DEFAULT_MAX_NODES);
    assert_eq!(Limits::from_cpp(-1, -1, 0).max_nodes, 0);
    assert_eq!(Limits::from_cpp(-1, -1, 77).max_nodes, 77);
}

#[test]
fn unlimited_limits_have_no_budget() {
    let input = binary_array_of_undefs(1_000);
    let (count, v) = parse(Format::Binary, &input, Limits::UNLIMITED);
    assert!(count > 0);
    assert!(v.is_some());
}

// ----- sources that never end ---------------------------------------------

/// An exact source (never reads ahead of the parser) that yields `head`, then
/// `cycle` over and over, for ever.
struct Endless {
    head: Vec<u8>,
    cycle: &'static [u8],
    pos: usize,
}

impl Endless {
    fn new(head: &[u8], cycle: &'static [u8]) -> Self {
        Endless {
            head: head.to_vec(),
            cycle,
            pos: 0,
        }
    }

    fn at(&self, i: usize) -> u8 {
        if i < self.head.len() {
            self.head[i]
        } else {
            self.cycle[(i - self.head.len()) % self.cycle.len()]
        }
    }
}

impl Source for Endless {
    fn read(&mut self, buf: &mut [u8]) -> usize {
        for b in buf.iter_mut() {
            *b = self.at(self.pos);
            self.pos += 1;
        }
        buf.len()
    }

    fn peek(&mut self) -> Option<u8> {
        Some(self.at(self.pos))
    }

    fn exact(&self) -> bool {
        true
    }
}

/// Parses an endless exact source under `limits` on a thread, failing the
/// test (instead of hanging it) when the parse does not end in time.
fn endless(format: Format, head: &'static [u8], cycle: &'static [u8], limits: Limits) -> i32 {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut src = Endless::new(head, cycle);
        let mut r = Reader::new(&mut src);
        let mut b = ValueBuilder::new(key_policy(format));
        let n = parse_into(format, &mut r, &mut b, limits, XmlMode::Parse);
        let _ = tx.send(n);
    });
    rx.recv_timeout(Duration::from_secs(20))
        .unwrap_or_else(|_| panic!("{format:?}: the parse of an endless source did not stop"))
}

#[test]
fn an_endless_exact_source_stops_at_the_budget_in_every_format() {
    // A binary array claiming 2^31 - 1 undefs, notation and XML arrays of
    // undefs without end.
    let cases: [(Format, &[u8], &[u8]); 3] = [
        (Format::Binary, b"[\x7f\xff\xff\xff", b"!"),
        (Format::Notation, b"[", b"!,"),
        (Format::Xml, b"<llsd><array>", b"<undef/>"),
    ];
    for (format, head, cycle) in cases {
        assert_eq!(
            endless(format, head, cycle, limits(1_000)),
            PARSE_FAILURE,
            "{format:?}"
        );
    }
}

#[test]
fn an_endless_exact_source_of_maps_stops_too() {
    // Map keys and string reads go through read_into / read_exact / read_until.
    let cases: [(Format, &[u8], &[u8]); 3] = [
        (Format::Binary, b"{\x7f\xff\xff\xff", b"k\x00\x00\x00\x01a!"),
        (Format::Notation, b"{", b"'a':!,"),
        (Format::Xml, b"<llsd><map>", b"<key>a</key><undef/>"),
    ];
    for (format, head, cycle) in cases {
        assert_eq!(
            endless(format, head, cycle, limits(1_000)),
            PARSE_FAILURE,
            "{format:?}"
        );
    }
}

#[test]
fn xml_elements_skipped_under_a_scalar_count_against_the_budget() {
    // `<integer>` ends the values; every `<a>` inside it is skipped but still
    // holds the parser's element stack. Endless under the default budget (an
    // exact source), and 16M of them in a slice.
    let head: &'static [u8] = b"<llsd><integer>";
    assert_eq!(
        endless(Format::Xml, head, b"<a>", limits(DEFAULT_MAX_NODES)),
        PARSE_FAILURE
    );
    let mut doc = head.to_vec();
    for _ in 0..(1 << 24) {
        doc.extend_from_slice(b"<a>");
    }
    let (n, v) = parse(Format::Xml, &doc, Limits::for_len(doc.len()));
    assert_eq!((n, v), (PARSE_FAILURE, None));
}

#[test]
fn skipped_descendants_cost_two_units_the_root_none() {
    assert_eq!(NODE_WEIGHT_SKIPPED_ELEMENT, 2);
    // 60 nested <a> under <integer>: the integer (1) plus 59 descendants of
    // the skipped root (2 each) is 119 units, as in the C++ test.
    let mut doc = b"<llsd><integer>".to_vec();
    for _ in 0..60 {
        doc.extend_from_slice(b"<a>");
    }
    for _ in 0..60 {
        doc.extend_from_slice(b"</a>");
    }
    doc.extend_from_slice(b"1</integer></llsd>");
    assert!(run(Format::Xml, &doc, 119).0 > 0, "119 units in 119");
    assert_eq!(
        run(Format::Xml, &doc, 118).0,
        PARSE_FAILURE,
        "119 units in 118"
    );
}
