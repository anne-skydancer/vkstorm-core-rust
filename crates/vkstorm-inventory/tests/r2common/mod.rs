//! The line format of the R2 corpora (`fixtures/cache/README.md`, "Line
//! format"), shared by the `r2_*` replay tests: `== <case>` sections, the
//! recorder's quoting and record dumps (`llinventorymodel_parity_harness.cpp`
//! `q`, `dump_category`, `dump_item`, `dump_validation`), and llcommon's
//! `gunzip_file` as the C++ glue around the cache codec does it.
#![allow(dead_code)]

pub mod fixture_model;
pub mod gz;
pub mod snapshot;

use std::fmt::Write as _;
use std::path::PathBuf;

use vkstorm_inventory::category::ViewerCategory;
use vkstorm_inventory::item::Item;
use vkstorm_inventory::validate::ValidationInfo;
use vkstorm_llsd::{Format, Value};

use crate::common::sha256_hex;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The path of an R2 fixture (`cache/...`, `msgs/...`).
pub fn path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)
}

pub fn read_bytes(rel: &str) -> Vec<u8> {
    let p = path(rel);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// One `== <case>` section.
#[derive(Clone, Debug)]
pub struct Case {
    pub id: String,
    pub lines: Vec<String>,
}

/// `parse_cases`: `#` lines skipped, a trailing `\r` dropped.
pub fn read_cases(rel: &str) -> Vec<Case> {
    let text = String::from_utf8(read_bytes(rel)).expect("fixture lines are escaped ASCII");
    let mut cases: Vec<Case> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(id) = line.strip_prefix("== ") {
            cases.push(Case { id: id.to_owned(), lines: Vec::new() });
        } else if line.starts_with('#') {
            continue;
        } else if let Some(c) = cases.last_mut() {
            c.lines.push(line.to_owned());
        }
    }
    // The final newline does not start a line.
    if text.ends_with('\n') {
        if let Some(c) = cases.last_mut() {
            if c.lines.last().is_some_and(|l| l.is_empty()) {
                c.lines.pop();
            }
        }
    }
    cases
}

// ---------------------------------------------------------------------------
// Counting and comparing
// ---------------------------------------------------------------------------

/// Lines replayed in the crate and lines left to the viewer test, per kind.
#[derive(Default, Debug)]
pub struct Tally {
    pub replayed: usize,
    pub left: usize,
    pub kinds: std::collections::BTreeMap<String, (usize, usize)>,
}

impl Tally {
    pub fn replayed(&mut self, kind: &str, n: usize) {
        self.replayed += n;
        self.kinds.entry(kind.to_owned()).or_default().0 += n;
    }
    pub fn left(&mut self, kind: &str, n: usize) {
        self.left += n;
        self.kinds.entry(kind.to_owned()).or_default().1 += n;
    }
    pub fn print(&self, label: &str) {
        eprintln!("{label}: {} lines replayed, {} left to the viewer test", self.replayed, self.left);
        for (k, (r, l)) in &self.kinds {
            eprintln!("  {k}: replayed {r}, left {l}");
        }
    }
}

/// Compares the produced lines of a case with the fixture's, reporting the
/// first difference.
pub fn assert_case(file: &str, case: &str, expected: &[String], actual: &[String]) {
    let n = expected.len().max(actual.len());
    for i in 0..n {
        let e = expected.get(i).map(String::as_str).unwrap_or("<end of case>");
        let a = actual.get(i).map(String::as_str).unwrap_or("<end of case>");
        if e != a {
            panic!("{file}: case {case} line {}\n  expected: {}\n  actual:   {}", i + 1, clip(e), clip(a));
        }
    }
}

fn clip(s: &str) -> String {
    if s.len() > 600 { format!("{}...({} chars)", &s[..600], s.len()) } else { s.to_owned() }
}

// ---------------------------------------------------------------------------
// Values as the recorder writes them
// ---------------------------------------------------------------------------

/// `q`: quoted, `\\` and `\"` escaped, bytes below 0x20 and from 0x7f as
/// `\xNN`.
pub fn q(s: &[u8]) -> String {
    let mut out = String::from("\"");
    for &c in s {
        if c == b'\\' || c == b'"' {
            out.push('\\');
            out.push(c as char);
        } else if !(0x20..0x7f).contains(&c) {
            let _ = write!(out, "\\x{c:02x}");
        } else {
            out.push(c as char);
        }
    }
    out.push('"');
    out
}

/// `LLUUID::asString`.
pub fn uuid(id: &[u8; 16]) -> String {
    vkstorm_inventory::cache::uuid_text(id)
}

/// Parses a hyphenated UUID.
pub fn parse_uuid(s: &str) -> [u8; 16] {
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    assert_eq!(hex.len(), 32, "bad uuid {s}");
    let mut id = [0u8; 16];
    for (i, b) in id.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
    }
    id
}

/// `hexmask`: `0x%x`.
pub fn hexmask(v: u32) -> String {
    format!("0x{v:x}")
}

pub fn hex(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub fn unhex(h: &str) -> Vec<u8> {
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()
}

/// `ids`: `[a,b,...]`.
pub fn ids<'a>(set: impl IntoIterator<Item = &'a [u8; 16]>) -> String {
    let v: Vec<String> = set.into_iter().map(uuid).collect();
    format!("[{}]", v.join(","))
}

/// The fetch fields of a folder the test never fetched: not fetching, the
/// `mDescendentsRequested` timer as constructed.
pub const FRESH_FETCH: &str = "fetching=0 timer{started=1 expired=1 to_expire=0.000}";

/// `dump_category` for a folder record and its descendent count.
pub fn dump_category(c: &ViewerCategory, count: i32) -> String {
    let o = &c.category.object;
    format!(
        "cat {} parent={} pref={} atype={} name={} owner={} version={} count={} thumb={} fav={} {}",
        uuid(&o.id),
        uuid(&o.parent),
        c.category.preferred_type.0,
        o.asset_type.0,
        q(&o.name),
        uuid(&c.owner),
        c.version,
        count,
        uuid(&o.thumbnail),
        u8::from(o.favorite),
        FRESH_FETCH
    )
}

/// `dump_item` for an item the viewer made from a record (`mIsComplete`
/// false, no transaction, not running or faulted, as the default
/// `LLViewerInventoryItem`).
pub fn dump_item(it: &Item) -> String {
    let o = &it.object;
    let p = &it.permissions;
    format!(
        "item {} parent={} type={} inv={} flags=0x{:08x} asset={} name={} desc={} created={} thumb={} fav={} runtime={} \
         perms={},{},{},{},{},{},{},{},{},{} sale={},{} complete=0 txn=00000000-0000-0000-0000-000000000000 running=0 faulted=0",
        uuid(&o.id),
        uuid(&o.parent),
        o.asset_type.0,
        it.inv_type.0,
        it.flags,
        uuid(&it.asset),
        q(&o.name),
        q(&it.description),
        o.creation_date,
        uuid(&o.thumbnail),
        u8::from(o.favorite),
        q(&o.runtime),
        uuid(&p.creator),
        uuid(&p.owner),
        uuid(&p.last_owner),
        uuid(&p.group),
        hexmask(p.base),
        hexmask(p.owner_mask),
        hexmask(p.group_mask),
        hexmask(p.everyone),
        hexmask(p.next),
        u8::from(p.group_owned),
        it.sale_info.sale_type.0,
        it.sale_info.price
    )
}

/// `llsd_hex`: binary LLSD (no header) as hex.
pub fn llsd_hex(v: &Value) -> String {
    hex(&vkstorm_llsd::format(Format::Binary, v, Default::default()).0)
}

/// `llsd_notation`: one line, binary as `b16"..."`.
pub fn llsd_notation(v: &Value) -> String {
    String::from_utf8(vkstorm_llsd::format(Format::Notation, v, Default::default()).0).unwrap()
}

/// `dump_validation`.
pub fn dump_validation(info: &ValidationInfo) -> Vec<String> {
    let sd = info.to_llsd();
    let mut out = vec![
        format!("validation llsd={}", llsd_hex(&sd)),
        format!("validation notation={}", llsd_notation(&sd)),
        format!(
            "validation fatal={} warnings={} loops={} orphans={} fatal_system_duplicate={} no_root={} no_library_root={} qa={} initialized={}",
            info.fatal_error_count,
            info.warning_count,
            info.loop_count,
            info.orphaned_count,
            info.fatal_system_duplicate,
            u8::from(info.fatal_no_root_folder),
            u8::from(info.fatal_no_library_root_folder),
            u8::from(info.fatal_qa_debug_mode),
            u8::from(info.initialized)
        ),
    ];
    for (k, v) in &info.warnings {
        out.push(format!("validation warning {k} {v}"));
    }
    out.push(format!("validation log={}", q(info.log.as_bytes())));
    out
}

/// `digest_lines`: the SHA-256 of the lines, each followed by `\n`.
pub fn digest_lines(lines: &[String]) -> String {
    let mut all = String::new();
    for l in lines {
        all.push_str(l);
        all.push('\n');
    }
    sha256_hex(all.as_bytes())
}

/// `chunked_digests`.
pub fn chunked_digests(label: &str, lines: &[String], chunk: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut first = 0;
    while first < lines.len() {
        let last = lines.len().min(first + chunk);
        out.push(format!("{label}[{first}..{}] sha256={}", last - 1, digest_lines(&lines[first..last])));
        first += chunk;
    }
    out
}
