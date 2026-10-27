//! The line format of the recorded model corpora (`fixtures/model/README.md`,
//! "Line format", "Records" and "Marked values"), shared by the replay tests
//! of C1-C10: value formatting as the recorder (`llinventory_tool.cpp`) writes
//! it, the fixed inputs, the recorder's random generator, and the comparer.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::path::PathBuf;

use vkstorm_llsd::{Format, Options, Value};

// ---------------------------------------------------------------------------
// Value formatting (the recorder's esc, hexbytes, x8, x2, d, bl, cstr)
// ---------------------------------------------------------------------------

/// `esc`: bytes 0x20-0x7e as they are except `\` as `\\`; every other byte
/// as `\xHH`.
pub fn esc(s: &[u8]) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for &c in s {
        match c {
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(c as char),
            _ => {
                let _ = write!(out, "\\x{c:02x}");
            }
        }
    }
    out
}

/// `esc` of a `&str`.
pub fn esc_str(s: &str) -> String {
    esc(s.as_bytes())
}

/// `cstr`: a C string that may be `NULL`, which is written `NULL`.
pub fn cstr(s: Option<&str>) -> String {
    s.map_or_else(|| "NULL".to_owned(), esc_str)
}

/// `hexbytes`: lower-case hex.
pub fn hex(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for b in data {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// `x8`: 8 lower-case hex digits.
pub fn x8(v: u32) -> String {
    format!("{v:08x}")
}

/// `x2`: 2 lower-case hex digits of the low byte.
pub fn x2(v: u32) -> String {
    format!("{:02x}", v & 0xff)
}

/// `d`: a decimal integer.
pub fn d(v: impl Into<i64>) -> String {
    v.into().to_string()
}

/// `bl`: `0`/`1`.
pub fn bl(v: bool) -> String {
    if v { "1" } else { "0" }.to_owned()
}

/// A UUID, lower-case and dashed.
pub fn uuid(id: &[u8; 16]) -> String {
    let h = hex(id);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// `llsd_bin`: the binary serialization (`LLSDSerialize::toBinary`, no
/// header) in hex.
pub fn llsd_bin(v: &Value) -> String {
    hex(&vkstorm_llsd::format(Format::Binary, v, Options::default()).0)
}

/// `llsd_txt`: the notation serialization (`LLSDSerialize::toNotation`),
/// escaped.
pub fn llsd_txt(v: &Value) -> String {
    esc(&vkstorm_llsd::format(Format::Notation, v, Options::default()).0)
}

// ---------------------------------------------------------------------------
// Fixed inputs (README "Fixed inputs")
// ---------------------------------------------------------------------------

/// `U(tag)`: `tag 01 23 45 67 89 ab cd ef 10 32 54 76 98 ba tag`.
pub fn u(tag: u8) -> [u8; 16] {
    [
        tag, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba, tag,
    ]
}

pub const NUL: [u8; 16] = [0; 16];

/// An owner.
pub fn id_a() -> [u8; 16] {
    u(0x0a)
}
/// Another agent.
pub fn id_b() -> [u8; 16] {
    u(0x0b)
}
/// A creator.
pub fn id_c() -> [u8; 16] {
    u(0x0c)
}
/// A group.
pub fn id_g() -> [u8; 16] {
    u(0x06)
}
/// Another group.
pub fn id_h() -> [u8; 16] {
    u(0x07)
}

/// `MAGIC`: `3c115e51-04f4-523c-9fa6-98aff1034730`.
pub const MAGIC: [u8; 16] = [
    0x3c, 0x11, 0x5e, 0x51, 0x04, 0xf4, 0x52, 0x3c, 0x9f, 0xa6, 0x98, 0xaf, 0xf1, 0x03, 0x47, 0x30,
];

/// `NOT_MAGIC`: the byte complement of [`MAGIC`].
pub fn not_magic() -> [u8; 16] {
    MAGIC.map(|b| !b)
}

/// The mask edges (`kMaskEdges`), in the README order.
pub const MASK_EDGES: [u32; 18] = [
    0x00000000, 0x7fffffff, 0xffffffff, 0x0000e000, 0x00002000, 0x00004000, 0x00008000, 0x00010000, 0x00080000,
    0x00000001, 0x80000000, 0x7fffbfff, 0x7fff7fff, 0x7fff3fff, 0x7fffdfff, 0x7fff9fff, 0x7fff5fff, 0x7fff1fff,
];

/// The recorder's generator (`Rng`, xorshift64*; README "Random streams").
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed })
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: u32) -> u32 {
        (self.next() % u64::from(n)) as u32
    }

    pub fn coin(&mut self) -> bool {
        self.next() & 1 != 0
    }
}

// ---------------------------------------------------------------------------
// Emitting lines
// ---------------------------------------------------------------------------

/// The lines a replay regenerates, in the recorder's `emit` format.
#[derive(Default)]
pub struct Lines(pub Vec<String>);

impl Lines {
    /// `emit`: `<case>\t<field>\t<value>...`.
    pub fn emit<S: AsRef<str>>(&mut self, case: &str, field: &str, values: &[S]) {
        let mut line = format!("{case}\t{field}");
        for v in values {
            line.push('\t');
            line.push_str(v.as_ref());
        }
        self.0.push(line);
    }

    /// `emit_llsd`: the `<field>.bin` and `<field>.txt` pair.
    pub fn emit_llsd(&mut self, case: &str, field: &str, v: &Value) {
        self.emit(case, &format!("{field}.bin"), &[llsd_bin(v)]);
        self.emit(case, &format!("{field}.txt"), &[llsd_txt(v)]);
    }

    /// `emit_llsd` for a field with only `.bin`.
    pub fn emit_llsd_bin(&mut self, case: &str, field: &str, v: &Value) {
        self.emit(case, &format!("{field}.bin"), &[llsd_bin(v)]);
    }
}

// ---------------------------------------------------------------------------
// Reading fixtures
// ---------------------------------------------------------------------------

/// The path of a model fixture.
pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/model").join(name)
}

/// The data lines of a (plain text) model fixture: every line that does not
/// start with `#`, as `read_lines` in the recorder.
pub fn read_fixture(name: &str) -> Vec<String> {
    let path = fixture_path(name);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let text = String::from_utf8(data).expect("fixture lines are escaped ASCII");
    let mut body: Vec<String> = text
        .split('\n')
        .filter(|l| !l.starts_with('#'))
        .map(str::to_owned)
        .collect();
    // A final newline ends the last line; it does not start an empty one.
    if text.ends_with('\n') || text.is_empty() {
        body.pop();
    }
    body
}

/// `split_tabs`.
pub fn split_tabs(line: &str) -> Vec<&str> {
    line.split('\t').collect()
}

// ---------------------------------------------------------------------------
// Comparing
// ---------------------------------------------------------------------------

const ITEM_COLS: &str = "id parent asset thumbnail type inv_type flags creator owner last_owner group base owner_mask group_mask everyone next group_owned sale_type sale_price date favorite runtime name desc";
const CAT_COLS: &str = "id parent type pref_type thumbnail favorite date runtime name";
const VCAT_COLS: &str = "id parent type pref_type thumbnail favorite date runtime name owner version";
const OBJ_COLS: &str = "id parent type thumbnail favorite date runtime name";
const PERM_COLS: &str = "creator owner last_owner group base owner_mask group_mask everyone next group_owned";
const SALE_COLS: &str = "type price";
const STREAM_COLS: &str = "state pos rest";

/// `columns_for`: the column names of a field (README "Records"), by case,
/// field and width; `v0`, `v1`, ... when none are known.
pub fn columns_for(case: &str, field: &str, n: usize) -> Vec<String> {
    let cols = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
    let pre = |first: &str, mut v: Vec<String>| {
        v.insert(0, first.to_owned());
        v
    };
    let corpus = |p: &str| case.starts_with(p);
    let v = if field.ends_with("stream") {
        if n == 4 { pre("ret", cols(STREAM_COLS)) } else { cols(STREAM_COLS) }
    } else if n == 24 {
        cols(ITEM_COLS)
    } else if n == 25 {
        pre("ret", cols(ITEM_COLS))
    } else if n == 26 {
        pre("ret", pre("same", cols(ITEM_COLS)))
    } else if field == "queries" {
        cols("allow helpers crc owned owner_id owner_is_group is_owned is_group_owned")
    } else if field == "agg" {
        if corpus("c9") {
            cols("u8 copy modify transfer empty repacked")
        } else {
            cols("u8 copy modify transfer move empty")
        }
    } else if field == "notecard" {
        cols("ret version embedded_version items text")
    } else if field == "legacy.outcome" {
        if n == 2 { cols("key nokey") } else { cols("outcome") }
    } else if field.ends_with("invariants") {
        cols("owner_in_base group_in_owner everyone_in_owner next_in_base everyone_no_modify no_transfer_no_copy")
    } else if field == "matrix" {
        cols(
            "legacy_type legacy_inv_type llsd_type llsd_inv_type crc reimport_type reimport_inv_type reimport_base \
             fromllsd_type fromllsd_inv_type fromllsd_base fromllsd_everyone linked_uuid",
        )
    } else if corpus("c2") && field == "out" {
        cols("name desc rna_pipe rna rename ctor_name ctor_desc set_desc cat_ctor")
    } else if corpus("c1") && field == "at" {
        cols("type_name human desc can_link can_fetch can_know is_link")
    } else if corpus("c1") && field == "it" {
        cols("name human default_for_asset cannot_restrict show_in_world")
    } else if corpus("c1") && field == "ft" {
        cols("name protected automatic singleton ensemble to_asset from_asset")
    } else if corpus("c1") && field == "lookup" {
        cols("name at at_cstr human human_cstr get_type it ft sale")
    } else if corpus("c1") && field == "category" {
        cols("u32 subcount name sub0 sub1 sub5 sub255")
    } else if field == "sale" && corpus("c6") {
        cols("type price has_perm_mask perm_mask ret")
    } else if field == "sale" && corpus("c9") {
        cols("type price multi_type multi_price")
    } else if field == "sale" && corpus("c4.acc") {
        cols("type price eq ne")
    } else if field == "sale+perm_mask" {
        cols("type price has_perm_mask perm_mask")
    } else if corpus("c4.") && n == 5 {
        cols("type price ret has_perm_mask perm_mask")
    } else if corpus("c4") && n == 2 && field != "crc" {
        cols(SALE_COLS)
    } else if n == 10
        && (corpus("c3") || corpus("c7") || field == "setpermissions" || field == "perm_from_sd")
    {
        cols(PERM_COLS)
    } else if n == 11 && (corpus("c3") || corpus("c6")) {
        pre("ret", cols(PERM_COLS))
    } else if n == 11 {
        cols(VCAT_COLS)
    } else if n == 12 {
        pre(if corpus("c10") { "same" } else { "ret" }, cols(VCAT_COLS))
    } else if n == 9 {
        cols(CAT_COLS)
    } else if n == 10 {
        if field == "fromllsd.asllsd" || field == "importmap" {
            let mut v = cols(CAT_COLS);
            v.push("ret".to_owned());
            v
        } else {
            pre("ret", cols(CAT_COLS))
        }
    } else if n == 8 {
        cols(OBJ_COLS)
    } else {
        Vec::new()
    };
    if v.len() == n { v } else { (0..n).map(|i| format!("v{i}")).collect() }
}

/// The value an expected column requires of the crate (README "Marked
/// values"): `None` to skip the column (`UB`), else the value to compare
/// with. `RT(win=..;linux=..;crate=..)` requires its `crate=` value; an `RT`
/// without one, or an `RT-MISMATCH`, can never match.
pub fn expected_value(expected: &str) -> Option<Result<&str, &str>> {
    if expected == "UB" {
        return None;
    }
    if expected.starts_with("RT-MISMATCH(") {
        return Some(Err("the recording itself is an RT-MISMATCH"));
    }
    if let Some(inner) = expected.strip_prefix("RT(").and_then(|s| s.strip_suffix(')')) {
        return Some(
            inner
                .split(';')
                .find_map(|kv| kv.strip_prefix("crate="))
                .ok_or("RT(...) states no crate= value"),
        );
    }
    Some(Ok(expected))
}

fn clip(s: &str) -> String {
    if s.len() <= 200 {
        s.to_owned()
    } else {
        let mut end = 200;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...({} chars)", &s[..end], s.len())
    }
}

/// The outcome of a replay.
pub struct Comparison {
    pub lines: usize,
    pub matched: usize,
    pub diffs: usize,
    pub report: String,
}

const MAX_REPORT: usize = 25;

/// `compare_lines`: compares the regenerated lines with the fixture's, line
/// by line. A line whose case, field and width agree is compared column by
/// column, honouring `UB` and `RT(...)`, and each differing column reported
/// as `<case> field <field>.<column>: expected <x> actual <y>`. Anything else
/// is a whole-line difference; a length difference stops the comparison.
pub fn compare(expected: &[String], actual: &[String], label: &str) -> Comparison {
    let mut diffs = 0;
    let mut report = String::new();
    let n = expected.len().max(actual.len());
    for i in 0..n {
        let (e, a) = (expected.get(i), actual.get(i));
        if let (Some(e), Some(a)) = (e, a) {
            if e == a {
                continue;
            }
            let (ev, av) = (split_tabs(e), split_tabs(a));
            if ev.len() >= 2 && ev.len() == av.len() && ev[0] == av[0] && ev[1] == av[1] {
                let cols = columns_for(ev[0], ev[1], ev.len() - 2);
                let mut text = String::new();
                for k in 2..ev.len() {
                    let ok = match expected_value(ev[k]) {
                        None => true,
                        Some(Ok(want)) => want == av[k],
                        Some(Err(_)) => false,
                    };
                    if !ok {
                        let _ = writeln!(
                            text,
                            "{label}: {} field {}.{}: expected {} actual {}",
                            ev[0],
                            ev[1],
                            cols[k - 2],
                            clip(ev[k]),
                            clip(av[k])
                        );
                    }
                }
                if text.is_empty() {
                    continue;
                }
                if diffs < MAX_REPORT {
                    report.push_str(&text);
                }
                diffs += 1;
                continue;
            }
        }
        if diffs < MAX_REPORT {
            let _ = writeln!(
                report,
                "{label}: line {}: expected {}\n{:w$}actual   {}",
                i + 1,
                e.map_or("<end>".to_owned(), |s| clip(s)),
                "",
                a.map_or("<end>".to_owned(), |s| clip(s)),
                w = label.len() + 2
            );
        }
        diffs += 1;
        if e.is_none() || a.is_none() {
            break;
        }
    }
    Comparison {
        lines: expected.len(),
        matched: expected.len().saturating_sub(diffs),
        diffs,
        report,
    }
}

/// Replays a corpus: reads `fixture`, compares it with `actual`, prints the
/// count and fails with the report on any difference.
pub fn assert_replay(fixture: &str, expected: &[String], actual: &[String]) {
    let c = compare(expected, actual, fixture);
    println!("{fixture}: {}/{} lines match", c.matched, c.lines);
    assert!(
        c.diffs == 0,
        "{fixture}: {} of {} lines differ (first {} shown):\n{}",
        c.diffs,
        c.lines,
        c.diffs.min(MAX_REPORT),
        c.report
    );
}
