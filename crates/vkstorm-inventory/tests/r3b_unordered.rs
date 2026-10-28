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

/// s12/o12: the region changes under the walk: AIS, no region, capabilities
/// not received, legacy only (the AIS-typed queue goes out as legacy
/// POSTs), AIS back with the legacy reply unpumped (finding 2), legacy
/// again, capabilities without inventory capabilities (the queue drained,
/// finding 3), AIS back.
fn region_change(id: &str, other_grid: bool) {
    let mut b = Builder::new(true);
    basic(&mut b, 6, false, 3);
    let lines = scenario_lines(id);
    let mut r = Run::new(id, b.w.clone(), &lines);
    r.unordered(Mode::Phased, 2);
    if other_grid {
        r.in_second_life = false;
        r.refresh_env();
    }
    r.login();
    r.set_startup(Startup::Started);
    r.until(|r| r.requests() >= 2 && r.pool_count() == 0 && r.sched.state().fetch_count == 0);
    r.step("ais_two_requests");
    r.set_region(false, CAPS_NONE, true);
    r.frames(30, true);
    r.step("no_region");
    r.set_region(true, CAPS_NONE, false);
    r.frames(30, true);
    r.step("caps_not_received");
    r.set_region(true, CAPS_LEGACY, true);
    r.until(|r| r.sched.state().fetch_count > 0);
    r.step("legacy_batch_sent");
    r.set_region(true, CAPS_ALL, true);
    r.settle_ex(true, false, false);
    r.step("ais_with_legacy_reply_unpumped");
    r.set_region(true, CAPS_LEGACY, true);
    r.settle(true, false);
    r.step("legacy_again");
    let (u5, s10) = (b.ids.user[5], b.ids.user_sub[10]);
    r.set_version(u5, -1);
    r.set_version(s10, -1);
    r.schedule_folder(u5, false);
    r.schedule_folder(s10, false);
    r.set_region(true, CAPS_NONE, true);
    r.frames(30, true);
    r.step("caps_without_inventory");
    r.set_region(true, CAPS_ALL, true);
    r.frames(30, true);
    r.start(vkstorm_inventory::fetch::NULL_ID, true);
    r.settle(true, false);
    r.step("ais_back_start");
    r.fetch_descendents_of(u5);
    r.settle(true, false);
    r.step("ais_back_fetch");
    report(id, &mut r);
}

#[test]
fn s12_region_change() {
    region_change("s12_region_change", false);
}

#[cfg(not(feature = "opensim"))]
#[test]
fn o12_region_change() {
    region_change("o12_region_change", true);
}

/// s10a/o10a: the legacy scheduler without AIS: the walk (root and library
/// only, finding 5), the COF observer, lost and found, a 403 split
/// (finding 1, survey bug 7), a 200-with-error re-issued until it
/// succeeds, bad_folders, the forced set kept for the session (finding 8).
fn legacy_caps(id: &str, other_grid: bool) {
    let mut b = Builder::new(true);
    basic(&mut b, 3, true, 3);
    let lib4 = pid(0xf4000000, 3);
    let e0 = pid(0xe0000000, 1);
    let bad = pid(0xc0500000, 1);
    let x4 = pid(0xf4000000, 2);
    b.w.add_folder(lib4, libroot(), FT_NONE, 3, true);
    b.w.add_folder(e0, root(), FT_NONE, 4, false);
    b.item(e0);
    b.w.add_folder(bad, root(), FT_NONE, 2, false);
    b.w.not_on_server.push(bad);
    b.w.add_folder(x4, root(), FT_NONE, 5, false);
    let mut r = phased(id, &b, CAPS_LEGACY, other_grid);
    r.mock.as_mut().unwrap().set_mode(0, 404, 0, 3);
    r.pool_size = 2;
    r.pool_setting = 2;
    r.refresh_env();
    r.login();
    r.walk();
    r.cof_without_ais("cof");
    r.settle(true, true);
    r.step("cof");
    r.find_lost_items();
    r.settle(true, true);
    r.step("lost_and_found");
    for f in [b.ids.user[0], x4, b.ids.user[1], b.ids.lib_folders[0], lib4] {
        r.fetch_descendents_of(f);
    }
    r.settle(true, true);
    r.step("forbidden_split");
    r.fetch_descendents_of(e0);
    r.settle(true, true);
    r.step("error_200");
    r.fetch_descendents_of(bad);
    r.settle(true, true);
    r.step("bad_folder");
    r.schedule_folder(b.ids.user[2], true);
    r.settle(true, true);
    r.step("forced_unknown");
    r.set_version(b.ids.user[2], -1);
    r.schedule_folder(b.ids.user[2], true);
    r.settle(true, true);
    r.step("forced_again");
    r.schedule_folder(root(), true);
    r.settle(true, true);
    r.step("forced_known");
    report(id, &mut r);
}

#[test]
fn s10a_legacy_caps_sl() {
    legacy_caps("s10a_legacy_caps_sl", false);
}

#[cfg(not(feature = "opensim"))]
#[test]
fn o10a_legacy_caps() {
    legacy_caps("o10a_legacy_caps", true);
}

/// o15b: a library owner other than ALEXANDRIA_LINDEN_ID over the legacy
/// capabilities (another grid, FSUseAis3Api off).
#[cfg(not(feature = "opensim"))]
#[test]
fn o15b_library_owner_legacy() {
    let id = "o15b_library_owner_legacy";
    let mut b = Builder::new(true);
    b.w.libowner = pid(0x0b1b0000, 1);
    b.w.folders.last_mut().unwrap().owner = b.w.libowner;
    basic(&mut b, 2, false, 2);
    let mut r = phased(id, &b, CAPS_LEGACY, true);
    r.pool_size = 2;
    r.pool_setting = 2;
    r.fs_use_ais3_api = false;
    r.refresh_env();
    r.login();
    r.walk();
    r.fetch_descendents_of(b.ids.lib_folders[1]);
    r.settle(true, true);
    r.step("library_folder");
    report(id, &mut r);
}

// ---- CONC: production concurrency, compared as multisets ------------------------

fn conc(id: &str, b: &Builder, caps: u32, other_grid: bool) -> Run {
    let lines = scenario_lines(id);
    let mut r = Run::new(id, b.w.clone(), &lines);
    r.unordered(Mode::Conc, 20);
    if other_grid {
        r.in_second_life = false;
    }
    r.set_region(true, caps, true);
    // the mock's delay_ms: long against a burst of frames
    r.reply_delay = 50;
    r
}

#[test]
fn c01_login_cold() {
    let id = "c01_login_cold";
    let mut b = Builder::new(true);
    basic(&mut b, 6, false, 3);
    let mut r = conc(id, &b, CAPS_ALL, false);
    r.login();
    r.walk();
    report(id, &mut r);
}

#[test]
fn c02_saturate() {
    let id = "c02_saturate";
    let mut b = Builder::new(true);
    basic(&mut b, 40, false, 1);
    b.w.add_folder(pid(0x4b000000, 1), root(), FT_NONE, 3, false);
    let mut r = conc(id, &b, CAPS_ALL, false);
    r.login();
    r.walk();
    report(id, &mut r);
}

#[test]
fn c03_large() {
    let id = "c03_large";
    let mut b = Builder::new(true);
    large(&mut b);
    let mut r = conc(id, &b, CAPS_ALL, false);
    r.reply_delay = 0;
    r.login();
    r.walk();
    let (n, total) = check(id, &mut r, &none);
    eprintln!("{id}: {n} of {total} lines compared; model dump recorded as digests, not checked");
}

/// c08: 2,047, 2,048 and 2,049 AIS commands at once: EnqueueAISCommand's
/// postponed queue (the crate's `fetch::ais::PostponedQueue`).
#[test]
fn c08_postponed() {
    use r3b_support::ais_standin::{Command, ItemType};
    let id = "c08_postponed";
    let mut b = Builder::new(true);
    basic(&mut b, 1, false, 1);
    let mut r = conc(id, &b, CAPS_ALL, false);
    r.reply_delay = 0;
    r.login();
    r.walk();
    for (k, n) in [2047usize, 2048, 2049].into_iter().enumerate() {
        let item = pid(0xa0d00000, k as u64 + 1);
        let before = r.journal_count("callback ");
        for _ in 0..n {
            r.ais(Command::FetchItem { item, item_type: ItemType::Inventory }, Some("postponed"), vec![]);
        }
        let pending = r.ais_pending();
        r.fact(&format!("enqueued {n} ais_pending={pending}"));
        r.settle(true, true);
        let fired = r.journal_count("callback ") - before;
        r.fact(&format!("enqueued {n} callbacks={fired}"));
        r.step(&format!("enqueued_{n}"));
    }
    let (c, total) = check(id, &mut r, &none);
    eprintln!("{id}: {c} of {total} lines compared");
}

/// c10/oc10: a burst of unknown folders, inventory and library, and
/// incomplete items of both owners on the legacy capabilities: up to 16
/// handlers.
fn legacy_burst(id: &str, other_grid: bool) {
    let mut b = Builder::new(true);
    basic(&mut b, 30, false, 8);
    let mut r = conc(id, &b, CAPS_LEGACY, other_grid);
    r.login();
    r.walk();
    let d = b.ids.clone();
    for f in d.user.iter().chain(&d.user_sub).chain(&d.lib_folders) {
        r.fetch_descendents_of(*f);
    }
    let root_items = d.items_in[&root()].clone();
    for i in [root_items[0], root_items[1], d.lib_items[0], d.lib_items[1]] {
        if r.model.items.contains_key(&i) {
            r.set_complete(i, false);
            r.schedule_item(i, false);
        }
    }
    r.settle(true, true);
    r.step("burst");
    report(id, &mut r);
}

#[test]
fn c10_legacy() {
    legacy_burst("c10_legacy", false);
}

#[cfg(not(feature = "opensim"))]
#[test]
fn oc10_legacy() {
    legacy_burst("oc10_legacy", true);
}

// ---- s13/o13, s10b: fetch fallbacks without AIS -----------------------------------

/// s13/o13: the COF without AIS blocked by the login's 180 s mark (finding
/// 4) then served, a folder over UDP without capabilities (C++ message:
/// not compared), the legacy scheduler, the COF over UDP, fetchFromServer's
/// foreground POSTs (with agent_id) and the item observer's (with
/// cap_name), nothing without capabilities.
fn fetch_fallbacks(id: &str, other_grid: bool) {
    let mut b = Builder::new(true);
    basic(&mut b, 3, false, 3);
    let mut r = phased(id, &b, CAPS_ALL, other_grid);
    r.pool_size = 2;
    r.pool_setting = 2;
    r.refresh_env();
    r.login();
    r.walk();
    let d = b.ids.clone();
    r.set_region(true, CAPS_LEGACY, true);
    let cof = sys(FT_CURRENT_OUTFIT);
    r.set_version(cof, -1);
    r.cof_without_ais("cof_legacy");
    r.settle(true, true);
    r.step("cof_legacy_timer_blocks");
    r.clock_advance(181.0);
    r.fetch_descendents_of(cof);
    r.settle(true, true);
    r.step("cof_legacy_after_timer");
    r.set_region(true, CAPS_NONE, true);
    r.set_version(d.user[0], -1);
    r.fetch_descendents_of(d.user[0]);
    r.settle(true, false);
    r.step("folder_udp");
    r.set_region(true, CAPS_LEGACY, true);
    r.set_version(d.user[1], -1);
    r.fetch_descendents_of(d.user[1]);
    r.settle(true, true);
    r.step("folder_legacy");
    r.set_version(cof, -1);
    r.clock_advance(11.0);
    r.set_region(true, CAPS_NONE, true);
    r.cof_without_ais("cof_udp");
    r.settle(true, false);
    r.step("cof_udp");
    r.set_region(true, CAPS_LEGACY, true);
    r.set_complete(d.obj1, false);
    r.fetch_from_server(d.obj1);
    r.settle(true, false);
    r.step("item_fetch_from_server");
    r.set_complete(d.lib_items[1], false);
    r.fetch_from_server(d.lib_items[1]);
    r.settle(true, false);
    r.step("library_item_fetch_from_server");
    r.set_complete(d.obj2, false);
    r.set_complete(d.lib_items[2], false);
    r.items_observer_start_legacy(&[d.obj2, d.lib_items[2]]);
    r.settle(true, false);
    r.step("item_observer_legacy");
    r.set_region(true, CAPS_NONE, true);
    r.set_complete(d.note1, false);
    r.items_observer_start_legacy(&[d.note1]);
    r.settle(true, false);
    r.step("item_observer_no_caps");
    report(id, &mut r);
}

#[test]
fn s13_sl_fetch_fallbacks() {
    fetch_fallbacks("s13_sl_fetch_fallbacks", false);
}

#[cfg(not(feature = "opensim"))]
#[test]
fn o13_fetch_fallbacks() {
    fetch_fallbacks("o13_fetch_fallbacks", true);
}

/// s10b: another grid with FSUseAis3Api off: the region's InventoryAPIv3 is
/// ignored, the legacy walk; item fetches without capabilities go over UDP
/// (C++ messages: not compared).
#[cfg(not(feature = "opensim"))]
#[test]
fn s10b_legacy_caps_other_grid() {
    let id = "s10b_legacy_caps_other_grid";
    let mut b = Builder::new(true);
    basic(&mut b, 2, true, 3);
    let mut r = phased(id, &b, CAPS_ALL, true);
    r.pool_size = 2;
    r.pool_setting = 2;
    r.fs_use_ais3_api = false;
    r.refresh_env();
    r.login();
    r.walk();
    r.find_lost_items();
    r.settle(true, true);
    r.step("lost_and_found");
    r.set_region(true, CAPS_NONE, true);
    let root1 = b.ids.items_in[&root()][0];
    let lib0 = b.ids.lib_items[0];
    r.set_complete(root1, false);
    r.set_complete(lib0, false);
    r.items_observer_start_legacy(&[root1, lib0]);
    r.settle(true, false);
    r.step("udp_item_fetch");
    report(id, &mut r);
}
