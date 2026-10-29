//! The R3a corpus runner (`llaisupdate_parity_test.cpp` in Rust): the
//! fixture tree, the case loader, the two runs of each case in index order,
//! the large case's digests, and a readable first difference.

#![allow(dead_code)]

pub mod gzip;
pub mod harness;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use harness::{map, to_binary, Harness, Mode};
use vkstorm_inventory::llsd::coerce::{as_boolean, as_integer, as_string, get, has};
use vkstorm_llsd::{Format, Limits, Options, Value};

use crate::common::sha256_hex;

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ais/update"))
}

fn read(path: &PathBuf) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A reply or case file through the LLSD XML parser (duplicate map keys:
/// the last wins, as the viewer's parser assigns them).
fn parse_xml_file(name: &str) -> Value {
    let path = fixtures_dir().join("cases").join(name);
    let mut bytes = read(&path);
    if name.ends_with(".gz") {
        bytes = gzip::gunzip(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    let (n, v) = vkstorm_llsd::parse(Format::Xml, &bytes, Limits::UNLIMITED);
    assert!(n > 0, "XML parse failure in {}", path.display());
    v.unwrap()
}

/// `cases/index.txt`: the case ids in run order.
pub fn read_index() -> Vec<String> {
    let text = String::from_utf8(read(&fixtures_dir().join("cases/index.txt"))).unwrap();
    text.lines()
        .map(|l| l.trim_end_matches([' ', '\r']).to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

pub struct Case {
    pub id: String,
    pub def: Value,
    pub responses: Vec<Value>,
    pub files: BTreeSet<String>,
}

fn str_of(v: &Value) -> String {
    String::from_utf8(as_string(v)).unwrap()
}

/// `load_case`: the case file, each step's reply, and the replies of the
/// steps hooks run (put into the action as `result`).
pub fn load_case(id: &str) -> Case {
    let mut files = BTreeSet::new();
    let name = format!("{id}.case.xml");
    files.insert(name.clone());
    let mut def = parse_xml_file(&name);
    assert_eq!(str_of(get(&def, "id")), id, "case file names another id");
    let mut responses = Vec::new();
    let Value::Map(dm) = &mut def else { panic!("case not a map") };
    let Some(Value::Array(steps)) = dm.get_mut(b"steps".as_slice()) else { panic!("no steps") };
    for step in steps.iter_mut() {
        let mut result = Value::Undefined;
        if has(step, "response") {
            let r = str_of(get(step, "response"));
            files.insert(r.clone());
            result = parse_xml_file(&r);
        } else {
            assert!(has(step, "call"), "{id}: a step without a response");
        }
        if let Value::Map(sm) = step
            && let Some(Value::Array(hooks)) = sm.get_mut(b"hooks".as_slice())
        {
            for hook in hooks.iter_mut() {
                let Value::Map(hm) = hook else { continue };
                let Some(Value::Array(actions)) = hm.get_mut(b"actions".as_slice()) else { continue };
                for a in actions.iter_mut() {
                    if has(a, "run_step") {
                        let r = str_of(get(get(a, "run_step"), "response"));
                        files.insert(r.clone());
                        let nested = parse_xml_file(&r);
                        if let Value::Map(am) = a {
                            am.insert(b"result".to_vec(), nested);
                        }
                    }
                }
            }
        }
        responses.push(result);
    }
    Case { id: id.to_string(), def, responses, files }
}

fn u64_list(v: &Value) -> Vec<u64> {
    match v {
        Value::Array(a) => a.iter().map(|x| as_integer(x) as u64).collect(),
        _ => Vec::new(),
    }
}

/// `run_mode`: one run of a case, `{ops, final}`.
pub fn run_mode(h: &mut Harness, c: &Case, mode: Mode) -> Value {
    let sched = get(&c.def, "sched");
    let call_us = if has(sched, "call_us") { as_integer(get(sched, "call_us")) as u64 } else { 100 };
    let frame_us = if has(sched, "frame_us") { as_integer(get(sched, "frame_us")) as u64 } else { 16667 };
    h.set_case(&c.id);
    h.skeleton_load(&c.def);
    h.begin_run(mode, call_us, frame_us, u64_list(get(sched, "others_us")), u64_list(get(sched, "yield_extra_us")));
    let Value::Array(steps) = get(&c.def, "steps") else { panic!("no steps") };
    for (i, step) in steps.iter().enumerate() {
        if i > 0 && !as_boolean(get(step, "same_frame")) {
            h.next_frame();
        }
        let ty = if has(step, "call") { get(step, "call") } else { get(step, "type") };
        h.step_marker(i as i32, &str_of(ty));
        h.set_disconnected(as_boolean(get(step, "disconnected")));
        h.run_step(step, &c.responses[i]);
        h.set_disconnected(false);
    }
    let ops = h.take_ops();
    map(vec![("ops", Value::Array(ops)), ("final", h.skeleton_dump())])
}

// ---------------------------------------------------------------------------
// The large case: digests and samples
// ---------------------------------------------------------------------------

const CHUNK: usize = 2000;

fn sha(v: &Value) -> Value {
    Value::String(sha256_hex(&to_binary(v)).into_bytes())
}

fn is_event(op: &[u8]) -> bool {
    !matches!(op, b"updateItem" | b"updateCategory" | b"q.getItem" | b"q.getCategory" | b"q.getVersion")
}

/// `digest_of_run`.
pub fn digest_of_run(run: &Value) -> Value {
    let Value::Array(ops) = get(run, "ops") else { panic!("no ops") };
    let fin = get(run, "final");
    let n = ops.len();
    let mut counts: BTreeMap<Vec<u8>, Value> = BTreeMap::new();
    let (mut head, mut tail, mut events, mut chunks) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (i, op) in ops.iter().enumerate() {
        let kind = as_string(get(op, "op"));
        let c = counts.entry(kind.clone()).or_insert(Value::Integer(0));
        *c = Value::Integer(as_integer(c) + 1);
        if i < 60 {
            head.push(op.clone());
        }
        if i + 40 >= n {
            tail.push(op.clone());
        }
        if is_event(&kind) {
            let mut e = op.clone();
            if let Value::Map(m) = &mut e {
                m.insert(b"index".to_vec(), Value::Integer(i as i32));
            }
            events.push(e);
        }
    }
    let mut first = 0;
    while first < n {
        let last = (first + CHUNK).min(n) - 1;
        let part = Value::Array(ops[first..=last].to_vec());
        chunks.push(map(vec![
            ("first", Value::Integer(first as i32)),
            ("last", Value::Integer(last as i32)),
            ("sha256", sha(&part)),
        ]));
        first += CHUNK;
    }
    map(vec![
        ("ops_sha256", sha(get(run, "ops"))),
        ("final_sha256", sha(fin)),
        ("op_count", Value::Integer(n as i32)),
        ("final_categories", Value::Integer(get(fin, "categories").len() as i32)),
        ("final_items", Value::Integer(get(fin, "items").len() as i32)),
        ("counts", Value::Map(counts)),
        ("head", Value::Array(head)),
        ("tail", Value::Array(tail)),
        ("events", Value::Array(events)),
        ("chunks", Value::Array(chunks)),
    ])
}

// ---------------------------------------------------------------------------
// Readable differences
// ---------------------------------------------------------------------------

pub fn short(v: &Value) -> String {
    let t = String::from_utf8_lossy(&vkstorm_llsd::format(Format::Notation, v, Options::default()).0).into_owned();
    if t.len() > 400 { format!("{}...", &t[..400]) } else { t }
}

fn same_kind(a: &Value, b: &Value) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// The first path at which two values differ.
pub fn first_diff(e: &Value, a: &Value, path: &str) -> Option<String> {
    if !same_kind(e, a) {
        return Some(format!("{path}: expected {} actual {}", short(e), short(a)));
    }
    match (e, a) {
        (Value::Map(em), Value::Map(am)) => {
            for (k, ev) in em {
                let kp = format!("{path}/{}", String::from_utf8_lossy(k));
                match am.get(k) {
                    None => return Some(format!("{kp}: expected {} actual (missing)", short(ev))),
                    Some(av) => {
                        if let Some(d) = first_diff(ev, av, &kp) {
                            return Some(d);
                        }
                    }
                }
            }
            for (k, av) in am {
                if !em.contains_key(k) {
                    let k = String::from_utf8_lossy(k);
                    return Some(format!("{path}/{k}: expected (missing) actual {}", short(av)));
                }
            }
            None
        }
        (Value::Array(ea), Value::Array(aa)) => {
            let n = ea.len().min(aa.len());
            for i in 0..n {
                let label = match get(&ea[i], "op") {
                    Value::String(op) => format!("{path}[{i}] ({})", String::from_utf8_lossy(op)),
                    _ => format!("{path}[{i}]"),
                };
                if let Some(d) = first_diff(&ea[i], &aa[i], &label) {
                    return Some(d);
                }
            }
            if ea.len() != aa.len() {
                let more = if ea.len() > n {
                    format!("first missing: {}", short(&ea[n]))
                } else {
                    format!("first extra: {}", short(&aa[n]))
                };
                return Some(format!("{path}: expected {} entries, actual {}; {more}", ea.len(), aa.len()));
            }
            None
        }
        _ => {
            if to_binary(e) != to_binary(a) {
                Some(format!("{path}: expected {} actual {}", short(e), short(a)))
            } else {
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------

pub struct Record {
    pub id: String,
    pub large: bool,
    /// `{case, never, sched}`, or the large case's digests of it.
    pub actual: Value,
    pub files: BTreeSet<String>,
    /// The number of steps and of command-function calls.
    pub steps: usize,
    pub calls: usize,
    pub hooks: usize,
}

pub struct Corpus {
    pub records: Vec<Record>,
    pub tid_failures: Vec<String>,
    pub apply_op_counts: BTreeMap<&'static str, usize>,
    pub phase_counts: BTreeMap<(&'static str, harness::Phase, String), usize>,
    pub never_ms: f64,
    pub sched_ms: f64,
}

/// Every case, `never` then `sched`, in index order in one harness (the
/// 403 alert flag, the batch timer and the frame counter are process
/// statics in the C++).
pub fn run_corpus() -> Corpus {
    let mut h = Harness::new();
    let mut records = Vec::new();
    let (mut never_ms, mut sched_ms) = (0.0, 0.0);
    for id in read_index() {
        let c = load_case(&id);
        let t0 = std::time::Instant::now();
        let never = run_mode(&mut h, &c, Mode::Never);
        let t1 = std::time::Instant::now();
        let sched = run_mode(&mut h, &c, Mode::Sched);
        never_ms += (t1 - t0).as_secs_f64() * 1000.0;
        sched_ms += t1.elapsed().as_secs_f64() * 1000.0;
        let large = as_boolean(get(&c.def, "large"));
        let actual = if large {
            let full = map(vec![("case", Value::String(id.clone().into_bytes())), ("never", never), ("sched", sched)]);
            map(vec![
                ("case", Value::String(id.clone().into_bytes())),
                ("sha256", sha(&full)),
                ("never", digest_of_run(get(&full, "never"))),
                ("sched", digest_of_run(get(&full, "sched"))),
            ])
        } else {
            map(vec![("case", Value::String(id.clone().into_bytes())), ("never", never), ("sched", sched)])
        };
        let Value::Array(steps) = get(&c.def, "steps") else { panic!() };
        let calls = steps.iter().filter(|s| has(s, "call")).count();
        let hooks = steps.iter().map(|s| get(s, "hooks").len()).sum();
        records.push(Record { id, large, actual, files: c.files, steps: steps.len(), calls, hooks });
    }
    Corpus {
        records,
        tid_failures: h.tid_failures.clone(),
        apply_op_counts: h.apply_op_counts.clone(),
        phase_counts: h.phase_counts.clone(),
        never_ms,
        sched_ms,
    }
}

/// The expected file of a record, parsed.
pub fn expected_of(r: &Record) -> Value {
    let suffix = if r.large { "digest" } else { "expected" };
    let path = fixtures_dir().join("expected").join(format!("{}.{suffix}.llsd", r.id));
    let bytes = read(&path);
    let (n, v) = vkstorm_llsd::parse(Format::Binary, &bytes, Limits::for_len(bytes.len()));
    assert!(n > 0, "{}: not binary LLSD", path.display());
    v.unwrap()
}

/// The number of ops of one run over the corpus (the large case by its
/// digest's `op_count`).
pub fn op_total(corpus: &Corpus, mode: &str) -> usize {
    corpus
        .records
        .iter()
        .map(|r| {
            let run = get(&r.actual, mode);
            if r.large { as_integer(get(run, "op_count")) as usize } else { get(run, "ops").len() }
        })
        .sum()
}

/// Compares one run of every case; returns the failure lines.
pub fn compare_runs(corpus: &Corpus, mode: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for r in &corpus.records {
        let expected = expected_of(r);
        let (e, a) = (get(&expected, mode), get(&r.actual, mode));
        let key = if r.large { "ops_sha256" } else { "ops" };
        assert!(has(e, key) && has(e, "final") == !r.large, "{}: the expected file has no {mode} run", r.id);
        if to_binary(e) == to_binary(a) && str_of(get(&expected, "case")) == r.id {
            continue;
        }
        let detail = if r.large {
            first_diff(e, a, mode)
        } else {
            first_diff(get(e, "ops"), get(a, "ops"), &format!("{mode} ops"))
                .or_else(|| first_diff(get(e, "final"), get(a, "final"), &format!("{mode} final")))
                .or_else(|| first_diff(e, a, mode))
        };
        failures.push(format!("case {}: {}", r.id, detail.unwrap_or_else(|| "binary bytes differ".into())));
    }
    failures
}

fn list_dir(p: &PathBuf) -> BTreeSet<String> {
    std::fs::read_dir(p)
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// The fixture tree holds exactly what the index names, and the manifest
/// lists the digest of every expected file.
pub fn check_tree(corpus: &Corpus) -> Vec<String> {
    let root = fixtures_dir();
    let mut out = Vec::new();
    let want_top: BTreeSet<String> =
        [".gitattributes", "README.md", "cases", "expected"].iter().map(|s| s.to_string()).collect();
    let mut want_cases: BTreeSet<String> = corpus.records.iter().flat_map(|r| r.files.iter().cloned()).collect();
    want_cases.insert("index.txt".into());
    let mut want_expected: BTreeSet<String> = BTreeSet::from(["manifest.txt".to_string()]);
    for r in &corpus.records {
        let suffix = if r.large { "digest" } else { "expected" };
        want_expected.insert(format!("{}.{suffix}.llsd", r.id));
        want_expected.insert(format!("{}.{suffix}.txt", r.id));
    }
    for (dir, want) in [("", &want_top), ("cases/", &want_cases), ("expected/", &want_expected)] {
        let have = list_dir(&root.join(dir));
        for f in want.difference(&have) {
            out.push(format!("missing fixture file {dir}{f}"));
        }
        for f in have.difference(want) {
            out.push(format!("unexpected fixture file {dir}{f}"));
        }
    }
    let manifest = String::from_utf8(read(&root.join("expected/manifest.txt"))).unwrap();
    let mut listed = 0;
    for line in manifest.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let cols: Vec<&str> = line.split(' ').collect();
        assert_eq!(cols.len(), 3, "manifest line {line}");
        let bytes = read(&root.join("expected").join(cols[1]));
        if sha256_hex(&bytes) != cols[2] {
            out.push(format!("manifest: {} does not match its digest", cols[1]));
        }
        listed += 1;
    }
    if listed != corpus.records.len() {
        out.push(format!("manifest lists {listed} files for {} cases", corpus.records.len()));
    }
    out
}
