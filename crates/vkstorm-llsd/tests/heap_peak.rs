// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026, Anne Skydancer (Vulkanstorm)
//! The peak heap of a parse that runs into its node budget, per payload
//! shape and tier. Run with `--nocapture` to print the table that
//! `DEFAULT_MAX_NODES` documents (its doc table). One test in its own binary: the counting
//! allocator is process-wide.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use vkstorm_llsd::{
    ASSET_MAX_NODES, DEFAULT_MAX_NODES, FlatSink, Format, Limits, Reader, SliceSource, Value,
    ValueBuilder, XmlMode, key_policy, parse_into,
};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = CURRENT.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        CURRENT.fetch_sub(l.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            if new >= l.size() {
                let now = CURRENT.fetch_add(new - l.size(), Ordering::Relaxed) + new - l.size();
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                CURRENT.fetch_sub(l.size() - new, Ordering::Relaxed);
            }
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

fn array_head(count: usize) -> Vec<u8> {
    let mut v = vec![b'['];
    v.extend_from_slice(&(count as i32).to_be_bytes());
    v
}

/// A payload of `units` or more budget units of one shape.
fn payload(shape: &str, units: usize) -> (Format, Vec<u8>) {
    let n = units + 16;
    match shape {
        "array of undefs" => {
            let mut v = array_head(n);
            v.resize(v.len() + n, b'!');
            (Format::Binary, v)
        }
        "nested arrays" => {
            let mut v = Vec::new();
            for _ in 0..n {
                v.push(b'[');
                v.extend_from_slice(&1i32.to_be_bytes());
            }
            v.push(b'!');
            (Format::Binary, v)
        }
        "map of undefs" => {
            let mut v = vec![b'{'];
            v.extend_from_slice(&(n as i32).to_be_bytes());
            for i in 0..n {
                v.push(b'k');
                v.extend_from_slice(&4i32.to_be_bytes());
                v.extend_from_slice(&(i as u32).to_be_bytes());
                v.push(b'!');
            }
            (Format::Binary, v)
        }
        "array of empty maps" => {
            let m = n / 4 + 1;
            let mut v = array_head(m);
            for _ in 0..m {
                v.push(b'{');
                v.extend_from_slice(&0i32.to_be_bytes());
                v.push(b'}');
            }
            (Format::Binary, v)
        }
        "array of 1-key maps" => {
            let m = n / 5 + 1;
            let mut v = array_head(m);
            for _ in 0..m {
                v.push(b'{');
                v.extend_from_slice(&1i32.to_be_bytes());
                v.extend_from_slice(b"k\x00\x00\x00\x01a!}");
            }
            (Format::Binary, v)
        }
        "nested maps" => {
            let m = n / 4 + 1;
            let mut v = Vec::new();
            for _ in 0..m {
                v.push(b'{');
                v.extend_from_slice(&1i32.to_be_bytes());
                v.extend_from_slice(b"k\x00\x00\x00\x01a");
            }
            v.push(b'!');
            (Format::Binary, v)
        }
        "xml skipped <a>" => {
            let mut v = b"<llsd><integer>".to_vec();
            // Each skipped descendant costs 2 units.
            for _ in 0..n / 2 {
                v.extend_from_slice(b"<a>");
            }
            (Format::Xml, v)
        }
        _ => unreachable!(),
    }
}

/// Peak heap above the payload's, MiB, of parsing it with `max_nodes` units.
fn peak_mib(format: Format, input: &[u8], max_nodes: usize, flat: bool) -> f64 {
    let limits = Limits {
        max_bytes: None,
        max_depth: -1,
        max_nodes,
    };
    let base = CURRENT.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let mut src = SliceSource::new(input);
    let mut r = Reader::new(&mut src);
    let n = if flat {
        let mut s = FlatSink::new();
        let n = parse_into(format, &mut r, &mut s, limits, XmlMode::Parse);
        drop(s);
        n
    } else {
        let mut b = ValueBuilder::new(key_policy(format));
        let n = parse_into(format, &mut r, &mut b, limits, XmlMode::Parse);
        let v: Option<Value> = b.finish();
        drop(v);
        n
    };
    assert_eq!(n, -1, "the payload must run into the budget");
    (PEAK.load(Ordering::Relaxed) - base) as f64 / (1024.0 * 1024.0)
}

#[test]
fn peak_heap_per_shape_and_tier() {
    let shapes = [
        "array of undefs",
        "nested arrays",
        "map of undefs",
        "array of empty maps",
        "array of 1-key maps",
        "nested maps",
        "xml skipped <a>",
    ];
    eprintln!(
        "{:<22} {:>10} {:>12} {:>12}",
        "shape (peak MiB)", "tier", "ValueBuilder", "FlatSink"
    );
    let mut worst = 0.0f64;
    for shape in shapes {
        for (tier, units) in [("2^20", ASSET_MAX_NODES), ("2^23", DEFAULT_MAX_NODES)] {
            let (format, input) = payload(shape, units);
            let value = peak_mib(format, &input, units, false);
            let flat = if format == Format::Xml {
                f64::NAN
            } else {
                peak_mib(format, &input, units, true)
            };
            eprintln!("{shape:<22} {tier:>10} {value:>12.1} {flat:>12.1}");
            worst = worst.max(value);
        }
    }
    // The default tier stays well inside a couple of GiB for every shape.
    assert!(worst < 2048.0, "worst peak {worst} MiB");
}
