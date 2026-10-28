//! R3a, the AIS update corpus (`fixtures/ais/update`, README "The checks"):
//! every case's `never` and `sched` runs regenerated with the crate's AIS
//! code (`ais::result`, `ais::update`, `ais::command`) inside a Rust copy of
//! the recording environment (`tests/r3a/harness.rs`), compared with the
//! committed binary LLSD byte for byte; the large case by its digests.

mod common;
mod r3a;

use std::sync::OnceLock;

use r3a::{check_tree, compare_runs, op_total, run_corpus, Corpus};
use vkstorm_inventory::llsd::coerce::{as_boolean, as_integer, as_string, get};
use vkstorm_llsd::Value;

fn corpus() -> &'static Corpus {
    static C: OnceLock<Corpus> = OnceLock::new();
    C.get_or_init(run_corpus)
}

fn report(mode: &str, failures: &[String]) {
    let c = corpus();
    eprintln!(
        "[r3a] check {mode}: compared {} cases ({} ops), {} differ",
        c.records.len(),
        op_total(c, mode),
        failures.len()
    );
    let lines = failures.join("\n");
    assert!(failures.is_empty(), "AIS update corpus, check '{mode}', differs from the fixtures:\n{lines}");
}

#[test]
fn never_op_lists_and_query_streams() {
    report("never", &compare_runs(corpus(), "never"));
}

/// Where the replayed ops come from: per run, the ops each phase recorded
/// (crate decisions: the error branch, the parse, the callback ids, the op
/// list the executor walks, the command requests; harness and C++: the
/// markers, the pool, the adapter, the executor's own model calls).
#[test]
fn coverage_report() {
    let c = corpus();
    for mode in ["never", "sched"] {
        let mut by_phase: std::collections::BTreeMap<String, (usize, Vec<String>)> = Default::default();
        for ((m, phase, kind), n) in &c.phase_counts {
            if *m == mode {
                let e = by_phase.entry(format!("{phase:?}")).or_default();
                e.0 += n;
                e.1.push(format!("{kind} {n}"));
            }
        }
        for (phase, (n, kinds)) in &by_phase {
            eprintln!("[r3a] {mode} {phase}: {n} ops ({})", kinds.join(", "));
        }
    }
    let steps: usize = c.records.iter().map(|r| r.steps).sum();
    let calls: usize = c.records.iter().map(|r| r.calls).sum();
    let hooks: usize = c.records.iter().map(|r| r.hooks).sum();
    eprintln!("[r3a] {} cases, {steps} steps ({calls} command-function calls), {hooks} hooks", c.records.len());
    eprintln!("[r3a] executor ops run (both runs): {:?}", c.apply_op_counts);
    eprintln!("[r3a] corpus time: never {:.0} ms, sched {:.0} ms", c.never_ms, c.sched_ms);
}

#[test]
fn sched_yield_points() {
    report("sched", &compare_runs(corpus(), "sched"));
}

#[test]
fn fixture_tree_and_write_tids() {
    let c = corpus();
    let tree = check_tree(c);
    assert!(tree.is_empty(), "fixture tree:\n{}", tree.join("\n"));
    assert!(c.tid_failures.is_empty(), "write tids:\n{}", c.tid_failures.join("\n"));
}

/// The shutdown contract, in both runs: after a stop the update aborts, no
/// callback and no model call follow, the model is as it was at the stop.
#[test]
fn shutdown_contract() {
    let c = corpus();
    let mut aborts = 0;
    for r in c.records.iter().filter(|r| !r.large) {
        for mode in ["never", "sched"] {
            let Value::Array(ops) = get(get(&r.actual, mode), "ops") else { panic!() };
            for (i, op) in ops.iter().enumerate() {
                if as_string(get(op, "op")) == b"abort" {
                    aborts += 1;
                    assert_eq!(as_integer(get(op, "ops_after_stop")), 0, "{} {mode} op {i}", r.id);
                    assert!(as_boolean(get(op, "model_unchanged")), "{} {mode} op {i}", r.id);
                }
            }
        }
    }
    eprintln!("[r3a] shutdown contract: {aborts} aborts checked");
}
