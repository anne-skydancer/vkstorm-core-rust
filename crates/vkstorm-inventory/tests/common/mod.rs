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
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// `llsd_bin`: the binary serialization (`LLSDSerialize::toBinary`, no
/// header) in hex.
pub fn llsd_bin(v: &Value) -> String {
    hex(&vkstorm_llsd::format(Format::Binary, v, Options::default()).0)
}

/// `llsd_txt`: the notation serialization (`LLSDSerialize::toNotation`),
/// escaped. `toNotation` formats with `OPTIONS_NONE`, so Binary is written
/// raw (`b(4)"..."`), not as `b16"..."`.
pub fn llsd_txt(v: &Value) -> String {
    let opts = Options {
        pretty_binary: false,
        ..Options::default()
    };
    esc(&vkstorm_llsd::format(Format::Notation, v, opts).0)
}

// ---------------------------------------------------------------------------
// Fixed inputs (README "Fixed inputs")
// ---------------------------------------------------------------------------

/// `U(tag)`: `tag 01 23 45 67 89 ab cd ef 10 32 54 76 98 ba tag`.
pub fn u(tag: u8) -> [u8; 16] {
    [
        tag, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba,
        tag,
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
    0x00000000, 0x7fffffff, 0xffffffff, 0x0000e000, 0x00002000, 0x00004000, 0x00008000, 0x00010000,
    0x00080000, 0x00000001, 0x80000000, 0x7fffbfff, 0x7fff7fff, 0x7fff3fff, 0x7fffdfff, 0x7fff9fff,
    0x7fff5fff, 0x7fff1fff,
];

/// The recorder's generator (`Rng`, xorshift64*; README "Random streams").
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/model")
        .join(name)
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
const VCAT_COLS: &str =
    "id parent type pref_type thumbnail favorite date runtime name owner version";
const OBJ_COLS: &str = "id parent type thumbnail favorite date runtime name";
const PERM_COLS: &str =
    "creator owner last_owner group base owner_mask group_mask everyone next group_owned";
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
        if n == 4 {
            pre("ret", cols(STREAM_COLS))
        } else {
            cols(STREAM_COLS)
        }
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
        if n == 2 {
            cols("key nokey")
        } else {
            cols("outcome")
        }
    } else if field.ends_with("invariants") {
        cols(
            "owner_in_base group_in_owner everyone_in_owner next_in_base everyone_no_modify no_transfer_no_copy",
        )
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
    if v.len() == n {
        v
    } else {
        (0..n).map(|i| format!("v{i}")).collect()
    }
}

/// The value an expected column requires of the crate (README "Marked
/// values"): `None` to skip the column (`UB`), else the value to compare
/// with. `RT(win=..;linux=..;crate=..)` requires the value of this build's
/// C++ runtime (port rule 1): `win=` under the MSVC target environment,
/// `linux=` otherwise; the `crate=` value is no longer read. An `RT` without
/// that key, or an `RT-MISMATCH`, can never match.
pub fn expected_value(expected: &str) -> Option<Result<&str, &str>> {
    const KEY: &str = if cfg!(target_env = "msvc") {
        "win="
    } else {
        "linux="
    };
    if expected == "UB" {
        return None;
    }
    if expected.starts_with("RT-MISMATCH(") {
        return Some(Err("the recording itself is an RT-MISMATCH"));
    }
    if let Some(inner) = expected
        .strip_prefix("RT(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return Some(
            inner
                .split(';')
                .find_map(|kv| kv.strip_prefix(KEY))
                .ok_or("RT(...) states no value for this platform's C++ runtime"),
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

// ---------------------------------------------------------------------------
// Records (the recorder's perm_cols, sale_cols, stream_cols, idname)
// ---------------------------------------------------------------------------

use vkstorm_inventory::legacy::reader::Stream;
use vkstorm_inventory::perms::Permissions;
use vkstorm_inventory::sale::SaleInfo;

/// `idname`: `null`, `A`, `B`, `C`, `G`, `H`, else the UUID.
pub fn idname(id: &[u8; 16]) -> String {
    let named = [
        (NUL, "null"),
        (id_a(), "A"),
        (id_b(), "B"),
        (id_c(), "C"),
        (id_g(), "G"),
        (id_h(), "H"),
    ];
    named
        .iter()
        .find(|(x, _)| x == id)
        .map_or_else(|| uuid(id), |(_, n)| (*n).to_owned())
}

/// `perm_cols`: the permissions record.
pub fn perm_cols(p: &Permissions) -> Vec<String> {
    vec![
        uuid(&p.creator),
        uuid(&p.owner),
        uuid(&p.last_owner),
        uuid(&p.group),
        x8(p.base),
        x8(p.owner_mask),
        x8(p.group_mask),
        x8(p.everyone),
        x8(p.next),
        bl(p.group_owned),
    ]
}

/// `ret` followed by `perm_cols`.
pub fn ret_perm_cols(ret: bool, p: &Permissions) -> Vec<String> {
    let mut v = perm_cols(p);
    v.insert(0, bl(ret));
    v
}

/// `sale_cols`: the sale record.
pub fn sale_cols(s: &SaleInfo) -> Vec<String> {
    vec![d(s.sale_type.0), d(s.price)]
}

/// `stream_cols`: `state pos rest` of a stream over `src`.
pub fn stream_cols(s: &Stream<'_>, src: &[u8]) -> Vec<String> {
    let pos = s.pos();
    let rest = if pos <= src.len() {
        esc(&src[pos..])
    } else {
        "?".to_owned()
    };
    vec![s.state_letters(), d(pos as i64), rest]
}

/// The text after the first line of a legacy block (after `permissions 0`
/// or `sale_info\t0`), as the recorder hands it to the readers.
pub fn after_first_line(text: &[u8]) -> &[u8] {
    match text.iter().position(|&b| b == b'\n') {
        Some(n) => &text[n + 1..],
        None => text,
    }
}

/// `sampled(i)` of the C3 and C4 streams: `i < 200 || i % 1000 == 0`.
pub fn sampled(i: u64) -> bool {
    i < 200 || i % 1000 == 0
}

// ---------------------------------------------------------------------------
// SHA-256 and chunked digests (the recorder's Sha256 and ChunkedDigest)
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const SHA_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 (FIPS 180-4), the recorder's `Sha256`.
#[derive(Clone)]
pub struct Sha256 {
    h: [u32; 8],
    block: [u8; 64],
    used: usize,
    total: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Sha256 {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            block: [0; 64],
            used: 0,
            total: 0,
        }
    }
}

impl Sha256 {
    fn compress(h: &mut [u32; 8], p: &[u8]) {
        let mut w = [0u32; 64];
        for (i, word) in p.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
        for (k, wi) in SHA_K.iter().zip(w.iter()) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(*k)
                .wrapping_add(*wi);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(v);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        if self.used > 0 {
            let take = data.len().min(64 - self.used);
            self.block[self.used..self.used + take].copy_from_slice(&data[..take]);
            self.used += take;
            data = &data[take..];
            if self.used < 64 {
                return;
            }
            let block = self.block;
            Self::compress(&mut self.h, &block);
            self.used = 0;
        }
        let mut blocks = data.chunks_exact(64);
        for b in &mut blocks {
            Self::compress(&mut self.h, b);
        }
        let rem = blocks.remainder();
        self.block[..rem.len()].copy_from_slice(rem);
        self.used = rem.len();
    }

    /// `hex()`: pads and returns the digest in lower-case hex.
    pub fn hex(mut self) -> String {
        let bits = self.total.wrapping_mul(8);
        self.update(&[0x80]);
        while self.used != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        self.h.iter().map(|w| format!("{w:08x}")).collect()
    }
}

/// `sha256(data)` in lower-case hex.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut s = Sha256::default();
    s.update(data);
    s.hex()
}

/// One `digests.txt` entry: name, cases, lines, SHA-256.
pub type DigestEntry = [String; 4];

/// The recorder's `ChunkedDigest`: a stream's SHA-256 whole and per chunk of
/// `chunk` cases, named `<stream>` and `<stream>@<first>-<end>`.
pub struct ChunkedDigest {
    name: String,
    chunk: u64,
    total: Sha256,
    part: Sha256,
    cases: u64,
    lines: u64,
    part_lines: u64,
    start: u64,
    entries: Vec<DigestEntry>,
}

impl ChunkedDigest {
    pub fn new(name: &str, chunk: u64) -> ChunkedDigest {
        ChunkedDigest {
            name: name.to_owned(),
            chunk,
            total: Sha256::default(),
            part: Sha256::default(),
            cases: 0,
            lines: 0,
            part_lines: 0,
            start: 0,
            entries: Vec::new(),
        }
    }

    pub fn begin_case(&mut self, i: u64) {
        if i != 0 && i % self.chunk == 0 {
            self.flush(i);
        }
        self.cases = i + 1;
    }

    /// A line and its `\n`.
    pub fn line(&mut self, l: &str) {
        for p in [l.as_bytes(), b"\n"] {
            self.total.update(p);
            self.part.update(p);
        }
        self.lines += 1;
        self.part_lines += 1;
    }

    fn flush(&mut self, end: u64) {
        let part = std::mem::take(&mut self.part);
        self.entries.push([
            format!("{}@{}-{}", self.name, self.start, end),
            (end - self.start).to_string(),
            self.part_lines.to_string(),
            part.hex(),
        ]);
        self.part_lines = 0;
        self.start = end;
    }

    pub fn finish(mut self) -> Vec<DigestEntry> {
        self.flush(self.cases);
        let total = std::mem::take(&mut self.total);
        self.entries.insert(
            0,
            [
                self.name.clone(),
                self.cases.to_string(),
                self.lines.to_string(),
                total.hex(),
            ],
        );
        self.entries
    }
}

/// `read_digests`: `digests.txt` as name -> (lines, sha256).
pub fn read_digests() -> std::collections::HashMap<String, (String, String)> {
    read_fixture("digests.txt")
        .iter()
        .filter_map(|l| {
            let v = split_tabs(l);
            (v.len() >= 4).then(|| (v[0].to_owned(), (v[2].to_owned(), v[3].to_owned())))
        })
        .collect()
}

/// `check_digests`: compares computed entries with `digests.txt` (a chunk
/// mismatch names its case range) and checks that `digests.txt` holds no
/// other entry for the stream. Returns the matching entries and a report of
/// the others.
pub fn check_digests(stream: &str, entries: &[DigestEntry]) -> (usize, String) {
    let digests = read_digests();
    let mut ok = 0;
    let mut report = String::new();
    for e in entries {
        let what = match e[0].split_once('@') {
            Some((s, range)) => format!("{s} cases [{})", range.replacen('-', ", ", 1)),
            None => format!("{} (whole stream)", e[0]),
        };
        match digests.get(&e[0]) {
            None => {
                let _ = writeln!(report, "{what}: no digest in digests.txt");
            }
            Some((lines, sha)) if *lines == e[2] && *sha == e[3] => ok += 1,
            Some((lines, sha)) => {
                let _ = writeln!(
                    report,
                    "{what}: digest (lines, sha256) expected {lines} {sha} actual {} {}",
                    e[2], e[3]
                );
            }
        }
    }
    let known = digests
        .keys()
        .filter(|k| *k == stream || k.strip_prefix(stream).is_some_and(|r| r.starts_with('@')))
        .count();
    if known != entries.len() {
        let _ = writeln!(
            report,
            "{stream}: digests.txt has {known} entries, computed {}",
            entries.len()
        );
    }
    (ok, report)
}

/// Asserts a stream's digests, printing the count.
pub fn assert_digests(stream: &str, entries: &[DigestEntry]) {
    let (ok, report) = check_digests(stream, entries);
    println!(
        "{stream}: {ok}/{} digest entries match (whole stream + chunks)",
        entries.len()
    );
    assert!(report.is_empty(), "{report}");
}

// ---------------------------------------------------------------------------
// The C5 records and cases, shared by C5 and C9 (the tool's PermF, ItemF,
// CatF, make_*, *_cols, base_item, prev_item, c5_names, c5_item_cases,
// prev_cat, c5_cat_cases)
// ---------------------------------------------------------------------------

use vkstorm_inventory::category::{Category, ViewerCategory};
use vkstorm_inventory::item::Item;
use vkstorm_inventory::object::Object;
use vkstorm_inventory::perms::PERM_ALL;
use vkstorm_inventory::sale::SaleType;
use vkstorm_inventory::types::{AssetType, FolderType, InventoryType};

// ---------------------------------------------------------------------------
// Field records (the tool's PermF, ItemF, CatF) and raw construction
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct PermF {
    pub creator: [u8; 16],
    pub owner: [u8; 16],
    pub last: [u8; 16],
    pub group: [u8; 16],
    pub base: u32,
    pub owner_mask: u32,
    pub group_mask: u32,
    pub everyone: u32,
    pub next: u32,
    pub group_owned: bool,
}

impl Default for PermF {
    fn default() -> Self {
        PermF {
            creator: NUL,
            owner: NUL,
            last: NUL,
            group: NUL,
            base: PERM_ALL,
            owner_mask: PERM_ALL,
            group_mask: PERM_ALL,
            everyone: PERM_ALL,
            next: PERM_ALL,
            group_owned: false,
        }
    }
}

#[derive(Clone)]
pub struct ItemF {
    pub id: [u8; 16],
    pub parent: [u8; 16],
    pub asset: [u8; 16],
    pub thumb: [u8; 16],
    pub ty: i32,
    pub inv_type: i32,
    pub flags: u32,
    pub perm: PermF,
    pub sale_type: i32,
    pub sale_price: i32,
    pub date: i64,
    pub fav: bool,
    pub runtime: Vec<u8>,
    pub name: Vec<u8>,
    pub desc: Vec<u8>,
}

impl Default for ItemF {
    fn default() -> Self {
        ItemF {
            id: NUL,
            parent: NUL,
            asset: NUL,
            thumb: NUL,
            ty: -1,
            inv_type: -1,
            flags: 0,
            perm: PermF::default(),
            sale_type: 0,
            sale_price: 10,
            date: 0,
            fav: false,
            runtime: Vec::new(),
            name: Vec::new(),
            desc: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct CatF {
    pub id: [u8; 16],
    pub parent: [u8; 16],
    pub thumb: [u8; 16],
    pub ty: i32,
    pub pref: i32,
    pub fav: bool,
    pub date: i64,
    pub runtime: Vec<u8>,
    pub name: Vec<u8>,
    pub owner: [u8; 16],
    pub version: i32,
}

impl Default for CatF {
    fn default() -> Self {
        CatF {
            id: NUL,
            parent: NUL,
            thumb: NUL,
            ty: 8,
            pref: -1,
            fav: false,
            date: 0,
            runtime: Vec::new(),
            name: Vec::new(),
            owner: NUL,
            version: -1,
        }
    }
}

/// `make_perm`: `init`, the five raw masks, then `yesReallySetOwner`.
pub fn make_perm(f: &PermF) -> Permissions {
    let mut p = Permissions::default();
    p.init(&f.creator, &f.owner, &f.last, &f.group);
    p.base = f.base;
    p.owner_mask = f.owner_mask;
    p.group_mask = f.group_mask;
    p.everyone = f.everyone;
    p.next = f.next;
    p.yes_really_set_owner(&f.owner, f.group_owned);
    p
}

pub fn make_sale(t: i32, p: i32) -> SaleInfo {
    SaleInfo {
        sale_type: SaleType(t),
        price: p,
    }
}

/// `make_item` (`RawItem::assign`): every field raw.
pub fn make_item(f: &ItemF) -> Item {
    Item {
        object: Object {
            id: f.id,
            parent: f.parent,
            thumbnail: f.thumb,
            asset_type: AssetType(f.ty),
            favorite: f.fav,
            creation_date: f.date,
            runtime: f.runtime.clone(),
            name: f.name.clone(),
        },
        permissions: make_perm(&f.perm),
        asset: f.asset,
        description: f.desc.clone(),
        sale_info: make_sale(f.sale_type, f.sale_price),
        inv_type: InventoryType(f.inv_type),
        flags: f.flags,
    }
}

/// `make_obj` (`RawObj::assign`).
pub fn make_obj(f: &CatF) -> Object {
    Object {
        id: f.id,
        parent: f.parent,
        thumbnail: f.thumb,
        asset_type: AssetType(f.ty),
        favorite: f.fav,
        creation_date: f.date,
        runtime: f.runtime.clone(),
        name: f.name.clone(),
    }
}

/// `make_cat` (`RawCat::assign`).
pub fn make_cat(f: &CatF) -> Category {
    Category {
        object: make_obj(f),
        preferred_type: FolderType(f.pref),
    }
}

/// `make_vcat`: `make_cat` plus the owner and version.
pub fn make_vcat(f: &CatF) -> ViewerCategory {
    ViewerCategory {
        category: make_cat(f),
        owner: f.owner,
        version: f.version,
    }
}

// ---------------------------------------------------------------------------
// Columns (the tool's item_cols, cat_cols, vcat_cols, obj_cols)
// ---------------------------------------------------------------------------

pub fn item_cols(it: &Item) -> Vec<String> {
    let o = &it.object;
    let mut v = vec![
        uuid(&o.id),
        uuid(&o.parent),
        uuid(&it.asset),
        uuid(&o.thumbnail),
        d(o.asset_type.0),
        d(it.inv_type.0),
        x8(it.flags),
    ];
    let p = perm_cols(&it.permissions);
    v.extend(p);
    v.extend([
        d(it.sale_info.sale_type.0),
        d(it.sale_info.price),
        d(o.creation_date),
        bl(o.favorite),
        esc(&o.runtime),
        esc(&o.name),
        esc(&it.description),
    ]);
    v
}

pub fn cat_cols(c: &Category) -> Vec<String> {
    let o = &c.object;
    vec![
        uuid(&o.id),
        uuid(&o.parent),
        d(o.asset_type.0),
        d(c.preferred_type.0),
        uuid(&o.thumbnail),
        bl(o.favorite),
        d(o.creation_date),
        esc(&o.runtime),
        esc(&o.name),
    ]
}

pub fn vcat_cols(c: &ViewerCategory) -> Vec<String> {
    let mut v = cat_cols(&c.category);
    v.push(uuid(&c.owner));
    v.push(d(c.version));
    v
}

pub fn obj_cols(o: &Object) -> Vec<String> {
    vec![
        uuid(&o.id),
        uuid(&o.parent),
        d(o.asset_type.0),
        uuid(&o.thumbnail),
        bl(o.favorite),
        d(o.creation_date),
        esc(&o.runtime),
        esc(&o.name),
    ]
}

pub fn with_ret(ret: bool, mut cols: Vec<String>) -> Vec<String> {
    cols.insert(0, bl(ret));
    cols
}

// ---------------------------------------------------------------------------
// The cases (the tool's base_item, prev_item, c5_names, c5_item_cases, ...)
// ---------------------------------------------------------------------------

pub const ASSET_TYPES: [i32; 36] = [
    -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 17, 18, 19, 20, 21, 22, 24, 25, 40, 45, 49, 56,
    57, 58, 59, 255, 26, 60, 127, -2, 50, 9,
];
pub const INV_TYPES: [i32; 27] = [
    0, 1, 2, 3, 6, 7, 8, 9, 10, 15, 17, 18, 19, 20, 22, 23, 24, 25, 26, 27, 28, -1, 255, 4, 5, 11,
    29,
];
pub const BASE_MASKS: [u32; 7] = [
    PERM_ALL, 0x7fffbfff, 0x7fff7fff, 0x7fffdfff, 0x7fff5fff, 0x00000000, 0x0000e000,
];
pub const FLAGS: [u32; 7] = [0, 1, 0xff, 0x100, 0x1f1100, 0x7fffffff, 0xffffffff];
pub const DATES: [i64; 7] = [
    0,
    1,
    i32::MAX as i64,
    i32::MIN as i64,
    2147483648,
    4294967301,
    -1,
];
pub const PRICES: [i32; 5] = [i32::MIN, -1, 0, 10, i32::MAX];
/// `T_ | V_`: transfer and move.
pub const NEXT_DEFAULT: u32 = 0x0008_2000;

pub fn base_item() -> ItemF {
    ItemF {
        id: u(0xa1),
        parent: u(0xa2),
        asset: u(0xa3),
        ty: 6,
        inv_type: 6,
        flags: 0,
        perm: PermF {
            creator: id_c(),
            owner: id_a(),
            last: id_b(),
            group: id_g(),
            base: PERM_ALL,
            owner_mask: PERM_ALL,
            group_mask: 0,
            everyone: 0,
            next: NEXT_DEFAULT,
            group_owned: false,
        },
        sale_type: 0,
        sale_price: 10,
        date: 1700000000,
        name: b"Base item".to_vec(),
        desc: b"Base description".to_vec(),
        ..ItemF::default()
    }
}

pub fn prev_item() -> ItemF {
    ItemF {
        id: u(0x11),
        parent: u(0x12),
        asset: u(0x13),
        thumb: u(0x14),
        ty: 7,
        inv_type: 7,
        flags: 0x101,
        perm: PermF {
            creator: u(0x21),
            owner: u(0x22),
            last: u(0x23),
            group: u(0x24),
            base: PERM_ALL,
            owner_mask: 0x0008e000,
            group_mask: 0x00002000,
            everyone: 0,
            next: 0x00082000,
            group_owned: false,
        },
        sale_type: 2,
        sale_price: 25,
        date: 1234567890,
        fav: true,
        runtime: b"pre-runtime".to_vec(),
        name: b"pre name".to_vec(),
        desc: b"pre desc".to_vec(),
    }
}

pub fn c5_names() -> Vec<(Vec<u8>, Vec<u8>)> {
    let p = |a: &[u8], b: &[u8]| (a.to_vec(), b.to_vec());
    vec![
        p(b"", b""),
        p(b"Plain name", b"Plain description"),
        p(b"pipe|name", b"pipe|desc"),
        p(b"new\nline", b"new\nline"),
        p(b"caf\xc3\xa9 \xf0\x9f\x98\x80", b"\xe6\x97\xa5\xe6\x9c\xac"),
        (vec![b'n'; 63], vec![b'd'; 127]),
        (vec![b'n'; 64], vec![b'd'; 128]),
        (vec![b'n'; 255], vec![b'd'; 300]),
        p(b"  leading and trailing  ", b"  leading and trailing  "),
        p(b"tab\there", b"tab\there"),
        p(b"cr\r", b"cr\r"),
        p(b"del\x7f", b"del\x7f"),
        // The C++ literal "\x01\x02ctrl" reads `\x02c` as one hex escape
        // (0x2c, ','); "\x1f" "ctrl" is split, so it keeps its "c".
        p(b"\x01,trl", b"\x1fctrl"),
        p(b"|", b"|"),
        p(b"name with spaces", b""),
        p(b"<llsd>", b"</llsd>"),
    ]
}

pub fn c5_item_cases() -> Vec<(String, ItemF)> {
    let mut v = Vec::new();
    let b = base_item();
    for at in ASSET_TYPES {
        let inv = if (0..60).contains(&at) {
            InventoryType::default_for_asset_type(AssetType(at)).0
        } else {
            255
        };
        v.push((
            format!("c5.at.{at}"),
            ItemF {
                ty: at,
                inv_type: inv,
                ..b.clone()
            },
        ));
    }
    for it in INV_TYPES {
        v.push((
            format!("c5.it.{it}"),
            ItemF {
                inv_type: it,
                ..b.clone()
            },
        ));
    }
    let assets = [NUL, u(0xa3), MAGIC, not_magic()];
    let asset_names = ["null", "a3", "magic", "notmagic"];
    let types = [(6, 6), (7, 7), (3, 3), (10, 10)];
    for m in BASE_MASKS {
        for ai in 0..4 {
            for (t, it) in types {
                let mut f = b.clone();
                f.perm.base = m;
                f.asset = assets[ai];
                f.ty = t;
                f.inv_type = it;
                v.push((format!("c5.mask.{}.{}.{t}", x8(m), asset_names[ai]), f));
            }
        }
    }
    let runtimes: [Vec<u8>; 4] = [
        Vec::new(),
        b"luau".to_vec(),
        vec![b'r'; 60],
        b"lu au".to_vec(),
    ];
    for th in 0..2 {
        for fv in 0..2 {
            for (ri, rt) in runtimes.iter().enumerate() {
                for t in [10, 7] {
                    let f = ItemF {
                        thumb: if th == 1 { u(0xb1) } else { NUL },
                        fav: fv != 0,
                        runtime: rt.clone(),
                        ty: t,
                        inv_type: t,
                        ..b.clone()
                    };
                    v.push((format!("c5.meta.{th}{fv}{ri}.{t}"), f));
                }
            }
        }
    }
    for fl in FLAGS {
        for (t, it) in [(6, 6), (5, 18), (56, 25)] {
            v.push((
                format!("c5.flags.{}.{t}", x8(fl)),
                ItemF {
                    flags: fl,
                    ty: t,
                    inv_type: it,
                    ..b.clone()
                },
            ));
        }
    }
    for st in 0..=3 {
        for p in PRICES {
            v.push((
                format!("c5.sale.{st}.{p}"),
                ItemF {
                    sale_type: st,
                    sale_price: p,
                    ..b.clone()
                },
            ));
        }
    }
    for st in [-1, 4, 200, 255] {
        v.push((
            format!("c5.sale.{st}.10"),
            ItemF {
                sale_type: st,
                ..b.clone()
            },
        ));
    }
    for (i, (n, ds)) in c5_names().into_iter().enumerate() {
        v.push((
            format!("c5.name.{i}"),
            ItemF {
                name: n,
                desc: ds,
                ..b.clone()
            },
        ));
    }
    for dt in DATES {
        v.push((
            format!("c5.date.{dt}"),
            ItemF {
                date: dt,
                ..b.clone()
            },
        ));
    }
    {
        let mut f = b.clone();
        f.perm.owner = NUL;
        f.perm.group_owned = true;
        v.push(("c5.groupowned".to_owned(), f));
        let mut f = b.clone();
        f.perm = PermF::default();
        v.push(("c5.perm.default".to_owned(), f));
        let mut f = b.clone();
        f.perm.group_mask = PERM_ALL;
        f.perm.everyone = PERM_ALL;
        f.perm.next = PERM_ALL;
        v.push(("c5.perm.unfixed".to_owned(), f));
    }
    {
        let mut f = ItemF {
            ty: 24,
            inv_type: -1,
            asset: u(0xc5),
            ..b.clone()
        };
        v.push(("c5.link".to_owned(), f.clone()));
        f.ty = 25;
        f.inv_type = 8;
        v.push(("c5.folderlink".to_owned(), f));
    }
    v
}

// ---------------------------------------------------------------------------
// Categories and objects
// ---------------------------------------------------------------------------

pub fn prev_cat() -> CatF {
    CatF {
        id: u(0x31),
        parent: u(0x32),
        thumb: u(0x34),
        ty: 8,
        pref: 5,
        fav: true,
        date: 99,
        runtime: b"pre-rt".to_vec(),
        name: b"pre folder".to_vec(),
        owner: u(0x33),
        version: 3,
    }
}

pub fn c5_cat_cases() -> Vec<(String, CatF)> {
    let mut v = Vec::new();
    let b = CatF {
        id: u(0xd1),
        parent: u(0xd2),
        ty: 8,
        name: b"Folder name".to_vec(),
        owner: u(0xd3),
        version: 7,
        ..CatF::default()
    };
    for p in -1..=101 {
        v.push((
            format!("c5c.pref.{p}"),
            CatF {
                pref: p,
                ..b.clone()
            },
        ));
    }
    v.push((
        "c5c.pref.127".to_owned(),
        CatF {
            pref: 127,
            ..b.clone()
        },
    ));
    v.push((
        "c5c.pref.-128".to_owned(),
        CatF {
            pref: -128,
            ..b.clone()
        },
    ));
    for p in [0, 8, 46, -1, 127, 58] {
        for k in 1..4 {
            let f = CatF {
                pref: p,
                thumb: if k & 1 != 0 { u(0xb2) } else { NUL },
                fav: k & 2 != 0,
                ..b.clone()
            };
            v.push((format!("c5c.meta.{p}.{k}"), f));
        }
    }
    for (i, (n, _)) in c5_names().into_iter().enumerate() {
        v.push((
            format!("c5c.name.{i}"),
            CatF {
                pref: 0,
                name: n,
                ..b.clone()
            },
        ));
    }
    for t in [-1, 255, 6, 24] {
        v.push((
            format!("c5c.type.{t}"),
            CatF {
                pref: 0,
                ty: t,
                ..b.clone()
            },
        ));
    }
    v.push((
        "c5c.runtime".to_owned(),
        CatF {
            runtime: b"luau".to_vec(),
            thumb: u(0xb2),
            ..b.clone()
        },
    ));
    v.push((
        "c5c.nullids".to_owned(),
        CatF {
            id: NUL,
            parent: NUL,
            owner: NUL,
            version: -1,
            ..b.clone()
        },
    ));
    v
}
