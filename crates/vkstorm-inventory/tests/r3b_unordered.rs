//! Replays the R3b fetch scenarios recorded without reply bodies: LOCKSTEP
//! (c04-c07, the AIS scheduler at production concurrency, replies held and
//! released one at a time in sorted order), on replies from the reply
//! generator (`r3b_support::mock`, held to the recording by
//! `r3b_seq.rs::generated_replies`). The wire lines are compared per round,
//! sorted, as recorded; their reply length and digest are the recording's.
//!
//! Each scenario follows its C++ script in `llinvfetch_parity_scenarios.cpp`.

mod r3b_support;

use r3b_support::*;

fn lockstep(id: &str, b: &Builder) -> Run {
    let lines = scenario_lines(id);
    let mut r = Run::new(id, b.w.clone(), &lines);
    r.unordered(Mode::Lockstep, 20);
    r
}

fn none(_: &str) -> bool {
    false
}

fn report(id: &str, r: &mut Run) {
    let (n, total) = check(id, r, &none);
    let (m, bad) = check_model_fetch_state(id, r);
    eprintln!("{id}: {n} of {total} lines compared; folder fetch state {}/{m}", m - bad.len());
    assert!(bad.is_empty(), "{id}: folder fetch state differs from the model dump:\n  {}", bad.join("\n  "));
}

fn lockstep_walk(r: &mut Run) {
    r.set_startup(Startup::Started);
    r.drain_rounds("walk");
    r.step("walk");
}

#[test]
fn c05_lockstep_login() {
    let id = "c05_lockstep_login";
    let mut b = Builder::new(true);
    basic(&mut b, 6, false, 3);
    let mut r = lockstep(id, &b);
    r.login();
    lockstep_walk(&mut r);
    report(id, &mut r);
}

#[test]
fn c04_ais_limit() {
    let id = "c04_ais_limit";
    let mut b = Builder::new(true);
    basic(&mut b, 10, false, 1);
    let mut r = lockstep(id, &b);
    r.login();
    lockstep_walk(&mut r);
    r.step("held_replies");
    let f = b.ids.user_sub.clone();
    r.clock_advance(181.0);
    for n in [18usize, 19, 20] {
        for x in f.iter().take(n) {
            r.set_version(*x, -1);
            r.schedule_folder(*x, false);
        }
        r.frame_note(&format!("queued_{n}_first"));
        r.frame_note(&format!("queued_{n}_second"));
        r.drain_rounds(&format!("queued_{n}"));
        r.step(&format!("queued_{n}"));
    }
    let mut items = Vec::new();
    for v in b.ids.items_in.values() {
        for id in v {
            if r.model.items.contains_key(id) && items.len() < 25 {
                items.push(*id);
            }
        }
    }
    for i in &items {
        r.set_complete(*i, false);
        r.schedule_item(*i, false);
    }
    r.frame_note("items_25_first");
    if let Some(h) = r.held().first().copied() {
        r.release_one(h);
    }
    r.frame_note("items_one_reply_processed");
    r.drain_rounds("items");
    r.step("items");
    items.truncate(19);
    for i in &items {
        r.set_complete(*i, false);
        r.schedule_item(*i, false);
    }
    r.frame_note("items_19");
    let t = r.token("folder_and_links");
    let a = b.ids.outfit_a;
    r.with(|s, cx| s.fetch_folder_and_links(cx, a, Some(t)));
    r.frame_note("folder_and_links_on_top");
    if let Some(l) = r.held_matching("/links") {
        r.release_one(l);
    }
    r.frame_note("links_reply_processed");
    let n = r.journal_count("fetch.completed");
    r.fact(&format!("completion with 19 items still out: fetch.completed={n}"));
    r.drain_rounds("items_and_links");
    r.step("items_and_links");
    report(id, &mut r);
}

#[test]
fn c06_lockstep_priority() {
    let id = "c06_lockstep_priority";
    let mut b = Builder::new(true);
    basic(&mut b, 20, false, 1);
    let mut r = lockstep(id, &b);
    r.login();
    lockstep_walk(&mut r);
    r.clock_advance(181.0);
    for i in 0..40 {
        let x = b.ids.user_sub[i];
        r.set_version(x, -1);
        r.schedule_folder(x, false);
    }
    r.round("first");
    for i in 9..=11 {
        r.set_version(b.ids.user_leaf[i], -1);
    }
    let item = b.ids.items_in[&b.ids.user_leaf[11]][0];
    r.set_complete(item, false);
    // priority_steps
    r.schedule_folder(b.ids.user_leaf[9], false);
    r.start(b.ids.user_leaf[10], false);
    r.schedule_item(item, false);
    r.snapshot();
    let q = r.sched_text();
    r.fact(&format!("priority inserted {q}"));
    r.drain_rounds("after_priority");
    r.step("priority");
    report(id, &mut r);
}

#[test]
fn c07_pool_clamp() {
    let id = "c07_pool_clamp";
    let mut b = Builder::new(true);
    basic(&mut b, 30, false, 1);
    let mut r = lockstep(id, &b);
    r.login();
    lockstep_walk(&mut r);
    for setting in [0u32, 1, 51, 52] {
        r.clock_advance(181.0);
        for x in b.ids.user_sub.clone() {
            r.set_version(x, -1);
            r.schedule_folder(x, false);
        }
        // the scheduler reads the setting every frame; the pool keeps its 20
        r.pool_setting = setting;
        r.refresh_env();
        r.frame_note(&format!("PoolSizeAIS_{setting}"));
        r.pool_setting = 20;
        r.refresh_env();
        r.drain_rounds(&format!("PoolSizeAIS_{setting}"));
        r.step(&format!("PoolSizeAIS_{setting}"));
    }
    report(id, &mut r);
}

// ---- PHASED: the legacy capability scheduler ------------------------------------

fn phased(id: &str, b: &Builder, caps: u32, other_grid: bool) -> Run {
    let lines = scenario_lines(id);
    let mut r = Run::new(id, b.w.clone(), &lines);
    r.unordered(Mode::Phased, 20);
    if other_grid {
        r.in_second_life = false;
    }
    r.set_region(true, caps, true);
    r
}

/// c11/oc11: batch 10 (9, 10, 11 folders: 1, 1, 2 POSTs), the threshold of
/// 12 outstanding (one POST per frame up to 13), four POSTs per frame up to
/// 16 (inventory and library folders and items).
fn legacy_limits(id: &str, other_grid: bool) {
    let mut b = Builder::new(true);
    basic(&mut b, 50, false, 8);
    let mut r = phased(id, &b, CAPS_LEGACY, other_grid);
    r.login();
    r.walk();
    let d = b.ids.clone();
    let pool: Vec<_> = d.user.iter().chain(&d.user_sub).chain(&d.user_leaf).copied().collect();
    let mut next = 0;
    r.legacy_delay = true;
    r.step("held_replies");
    for n in [9usize, 10, 11] {
        for _ in 0..n {
            r.fetch_descendents_of(pool[next]);
            next += 1;
        }
        r.frame_note(&format!("batch_{n}_first"));
        r.frame_note(&format!("batch_{n}_second"));
        r.settle(true, true);
        r.step(&format!("batch_{n}"));
    }
    for _ in 0..140 {
        r.fetch_descendents_of(pool[next]);
        next += 1;
    }
    for k in 1..=15 {
        r.frame_note(&format!("threshold_{k:02}"));
    }
    r.settle(true, true);
    r.step("threshold");
    for i in 0..8 {
        r.fetch_descendents_of(d.lib_folders[i * 3]);
    }
    r.settle(true, true);
    r.step("library_loaded");
    let mut inv_items = Vec::new();
    for v in d.items_in.values() {
        for id in v {
            if r.model.items.contains_key(id) && inv_items.len() < 30 {
                inv_items.push(*id);
            }
        }
    }
    let lib_items: Vec<_> = d.lib_items.iter().filter(|i| r.model.items.contains_key(*i)).take(30).copied().collect();
    let lib_sub: Vec<_> = d.lib_folders.iter().enumerate().filter(|(i, _)| i % 3 != 0).map(|(_, f)| *f).collect();
    let (mut ni, mut nl, mut nf) = (0, 0, 0);
    for k in 1..=6 {
        for _ in 0..2 {
            r.fetch_descendents_of(pool[next]);
            next += 1;
        }
        for _ in 0..2 {
            r.fetch_descendents_of(lib_sub[nf]);
            nf += 1;
        }
        for _ in 0..3 {
            if ni < inv_items.len() {
                r.set_complete(inv_items[ni], false);
                r.schedule_item(inv_items[ni], false);
                ni += 1;
            }
            if nl < lib_items.len() {
                r.set_complete(lib_items[nl], false);
                r.schedule_item(lib_items[nl], false);
                nl += 1;
            }
        }
        r.frame_note(&format!("four_posts_{k}"));
    }
    r.settle(true, true);
    r.step("sixteen");
    report(id, &mut r);
}

#[test]
fn c11_legacy_limits() {
    legacy_limits("c11_legacy_limits", false);
}

/// Recorded with `OPENSIM` off; with the `opensim` feature `bulkFetch()`
/// descends on other grids, so the other-grid legacy scenarios differ by
/// design.
#[cfg(not(feature = "opensim"))]
#[test]
fn oc11_legacy_limits() {
    legacy_limits("oc11_legacy_limits", true);
}
