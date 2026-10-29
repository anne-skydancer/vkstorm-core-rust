//! Replays the sequence-mode (SEQ) scenarios of the R3b fetch wire corpus
//! (`fixtures/ais/fetch`): the scheduler and request building of
//! `vkstorm_inventory::fetch` driven through the recorded scenario steps and
//! replies, reproducing every request (method, target, head, body, request
//! counts), the scheduler state lines, completion signals, callbacks and
//! notifications byte for byte. See `r3b_support` for what stands in for the
//! C++ model and transport, and which lines are left to the viewer test.
//!
//! Each scenario follows its C++ script in `llinvfetch_parity_scenarios.cpp`.

mod r3b_support;

use r3b_support::*;
use vkstorm_inventory::fetch::{NULL_ID, Uuid};
use vkstorm_inventory::ais::command::Command;
use vkstorm_llsd::Value;

thread_local! {
    /// Fetch replies from the reply generator (`r3b_support::mock`).
    static GENERATED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn new_run(id: &str, w: World) -> Run {
    let lines = scenario_lines(id);
    let mut r = Run::new(id, w.clone(), &lines);
    if GENERATED.with(|g| g.get()) {
        r.mock = Some(r3b_support::mock::Mock::new(&w));
    }
    r
}

fn run_world(id: &str, b: &Builder) -> Run {
    new_run(id, b.w.clone())
}

fn none(_: &str) -> bool {
    false
}

/// Compares the lines only; the model dump is not checked (a scenario whose
/// folders C++ model code changes outside the fetch path).
fn report_lines(id: &str, r: &mut Run, skip: &dyn Fn(&str) -> bool) {
    let (n, total) = check(id, r, skip);
    eprintln!("{id}: {n} of {total} lines compared; model dump not checked");
}

fn report(id: &str, r: &mut Run, skip: &dyn Fn(&str) -> bool) {
    let (n, total) = check(id, r, skip);
    let (m, bad) = check_model_fetch_state(id, r);
    eprintln!("{id}: {n} of {total} lines compared; folder fetch state {}/{m}", m - bad.len());
    assert!(bad.is_empty(), "{id}: folder fetch state differs from the model dump:
  {}", bad.join("
  "));
}

fn std_world(user: usize, orphan: bool, lib: usize) -> Builder {
    let mut b = Builder::new(true);
    basic(&mut b, user, orphan, lib);
    b
}

/// The body of the first recorded request with `method`: a write's LLSD
/// body is the caller's, not built by the crate.
fn recorded_body(id: &str, method: &str) -> Vec<u8> {
    let lines = scenario_lines(id);
    let l = lines
        .iter()
        .find(|l| l.starts_with("wire ") && l.split(' ').nth(2) == Some(method))
        .unwrap();
    let a = l.find(" body=").unwrap() + 6;
    let e = l.rfind(" resp=").unwrap();
    unescape(&l[a..e])
}

/// The body of recorded request `n` (1-based).
fn recorded_body_n(id: &str, n: usize) -> Vec<u8> {
    let lines = scenario_lines(id);
    let prefix = format!("wire {n} ");
    let l = lines.iter().find(|l| l.starts_with(&prefix)).unwrap();
    let a = l.find(" body=").unwrap() + 6;
    let e = l.rfind(" resp=").unwrap();
    unescape(&l[a..e])
}

fn unescape(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                b'n' => out.push(b'\n'),
                b'r' => out.push(b'\r'),
                b't' => out.push(b'\t'),
                b'x' => {
                    out.push(u8::from_str_radix(&s[i + 2..i + 4], 16).unwrap());
                    i += 2;
                }
                c => out.push(c),
            }
            i += 2;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

// ---- 1: login_cold -------------------------------------------------------------

#[test]
fn s01_login_cold() {
    s01_login_cold_body();
}

fn s01_login_cold_body() {
    let id = "s01_login_cold";
    let b = std_world(6, true, 3);
    let mut r = run_world(id, &b);
    r.login();
    r.cof(b.ids.outfit_a);
    r.walk();
    r.find_lost_items();
    r.settle(true, true);
    r.step("orphans");
    r.step("logout_cache");
    report(id, &mut r, &none);
}

// ---- 2: login_warm (s01's logout cache) --------------------------------------------

/// s01's run, for its final model: the cache s02a and s02b load and the
/// folder versions the server has after it.
fn s01_after() -> (Builder, ReplayModel) {
    let id = "s01_login_cold";
    let b = std_world(6, true, 3);
    let mut r = run_world(id, &b);
    r.login();
    r.cof(b.ids.outfit_a);
    r.walk();
    r.find_lost_items();
    r.settle(true, true);
    assert!(r.error.is_none());
    (b, std::mem::take(&mut r.model))
}

/// `s02_world()`: the server after s01: the orphan moved to Lost And Found
/// (the server acted on the viewer's MoveInventoryItem), every folder at the
/// version the viewer cached.
fn s02_world(b: &Builder, m: &ReplayModel) -> World {
    let mut w = b.w.clone();
    let orphan = pid(0xa0f00000, 1);
    for i in &mut w.items {
        if i.id == orphan {
            i.parent = sys(FT_LOST_AND_FOUND);
        }
    }
    for f in &mut w.folders {
        if let Some(c) = m.cats.get(&f.id) {
            if c.version >= 0 {
                f.version = c.version;
            }
        }
    }
    w
}

#[test]
fn s02a_login_warm_full() {
    s02a_login_warm_full_body();
}

fn s02a_login_warm_full_body() {
    let id = "s02a_login_warm_full";
    let (b, m) = s01_after();
    let w = s02_world(&b, &m);
    let mut r = new_run(id, w);
    r.cache = Some(m.save_cache());
    r.login();
    r.cof(NULL_ID);
    r.walk();
    report(id, &mut r, &none);
}

#[test]
fn s02b_login_warm_partial() {
    s02b_login_warm_partial_body();
}

fn s02b_login_warm_partial_body() {
    let id = "s02b_login_warm_partial";
    let (b, m) = s01_after();
    let mut w = s02_world(&b, &m);
    let (tree, leaf) = (b.ids.user[0], b.ids.user_leaf[3]);
    for f in &mut w.folders {
        if f.id == tree || f.id == leaf {
            f.version += 1;
        }
    }
    w.add_item(pid(0xa0100000, 1), tree, false);
    w.add_item(pid(0xa0100000, 2), leaf, false);
    let mut r = new_run(id, w);
    r.cache = Some(m.save_cache());
    r.login();
    r.cof(NULL_ID);
    r.walk();
    report(id, &mut r, &none);
}

// ---- d2: Second Life waits for its capabilities ----------------------------------

fn d2_tail(r: &mut Run) {
    r.walk();
    r.find_lost_items();
    r.settle(true, true);
    r.step("orphans");
    r.same_as_s01();
}

fn walk_started(r: &mut Run) {
    r.set_startup(Startup::Started);
    r.until(|r| r.requests() >= 4 && r.pool_count() == 0 && r.sched.state().fetch_count == 0);
    r.step("walk_started");
}

#[test]
fn d2a_sl_wait_login() {
    d2a_sl_wait_login_body();
}

fn d2a_sl_wait_login_body() {
    let id = "d2a_sl_wait_login";
    let b = std_world(6, true, 3);
    let mut r = run_world(id, &b);
    r.set_region(true, CAPS_NONE, false);
    r.login();
    r.set_startup(Startup::Started);
    r.d2_wait("login_without_caps", 60);
    r.set_region(true, CAPS_ALL, true);
    r.cof(b.ids.outfit_a);
    d2_tail(&mut r);
    report(id, &mut r, &none);
}

#[test]
fn d2b_sl_wait_midwalk() {
    d2b_sl_wait_midwalk_body();
}

fn d2b_sl_wait_midwalk_body() {
    let id = "d2b_sl_wait_midwalk";
    let b = std_world(6, true, 3);
    let mut r = run_world(id, &b);
    r.login();
    r.cof(b.ids.outfit_a);
    walk_started(&mut r);
    r.set_region(true, CAPS_NONE, false);
    r.d2_wait("caps_not_received", 60);
    r.set_region(true, CAPS_ALL, true);
    d2_tail(&mut r);
    report(id, &mut r, &none);
}

#[test]
fn d2c_sl_region_change_wait() {
    d2c_sl_region_change_wait_body();
}

fn d2c_sl_region_change_wait_body() {
    let id = "d2c_sl_region_change_wait";
    let b = std_world(6, true, 3);
    let mut r = run_world(id, &b);
    r.login();
    r.cof(b.ids.outfit_a);
    walk_started(&mut r);
    r.set_region(false, CAPS_NONE, true);
    r.d2_wait("no_region", 30);
    r.set_region(true, CAPS_NONE, false);
    r.d2_wait("new_region_without_ais", 30);
    r.set_region(true, CAPS_ALL, true);
    d2_tail(&mut r);
    report(id, &mut r, &none);
}

#[test]
fn d2d_sl_fsuseais3api_off() {
    d2d_sl_fsuseais3api_off_body();
}

fn d2d_sl_fsuseais3api_off_body() {
    let id = "d2d_sl_fsuseais3api_off";
    let b = std_world(6, true, 3);
    let mut r = run_world(id, &b);
    r.fs_use_ais3_api = false;
    r.refresh_env();
    r.login();
    r.cof(b.ids.outfit_a);
    d2_tail(&mut r);
    report(id, &mut r, &none);
}

// ---- 4: forbidden -------------------------------------------------------------

#[test]
fn s04_forbidden() {
    s04_forbidden_body();
}

fn s04_forbidden_body() {
    let id = "s04_forbidden";
    let mut b = std_world(2, false, 3);
    let x3 = pid(0xf3000000, 1);
    let x4 = pid(0xf4000000, 1);
    b.w.add_folder(x3, root(), FT_NONE, 5, false);
    b.item(x3);
    for _ in 1..=2 {
        let s = b.folder(x3, FT_NONE);
        b.item(s);
    }
    b.w.add_folder(x4, root(), FT_NONE, 6, false);
    b.item(x4);
    for _ in 1..=3 {
        let s = b.folder(x4, FT_NONE);
        b.item(s);
    }
    let mut r = run_world(id, &b);
    r.login();
    r.walk();
    report(id, &mut r, &none);
}

// ---- 5: transient 5xx -----------------------------------------------------------

#[test]
fn s05_transient_5xx() {
    s05_transient_5xx_body();
}

fn s05_transient_5xx_body() {
    let id = "s05_transient_5xx";
    let mut b = std_world(1, false, 1);
    let t52 = pid(0x52000000, 1);
    let t5e = pid(0x5e000000, 1);
    let w53 = pid(0x53000000, 1);
    b.w.add_folder(t52, root(), FT_NONE, 4, false);
    b.w.add_folder(t5e, root(), FT_NONE, 4, false);
    for _ in 1..=2 {
        let f = b.folder(t52, FT_NONE);
        b.item(f);
        let f = b.folder(t5e, FT_NONE);
        b.item(f);
    }
    b.w.add_item(w53, sys(FT_OBJECT), false);
    let mut r = run_world(id, &b);
    r.login();
    r.walk();
    r.set_version(t52, -1);
    r.start(t52, true);
    r.settle(true, true);
    r.step("busy_twice");
    r.set_version(t5e, -1);
    r.start(t5e, true);
    r.settle(true, true);
    r.step("busy_at_depth");
    // update_inventory_item: AISAPI::UpdateItem with the caller's LLSD body
    let body = recorded_body(id, "PATCH");
    r.ais(Command::UpdateItem { item_id: w53, updates: Value::Undefined }, Some("w53"), body);
    r.settle(true, true);
    r.step("write_retry");
    report(id, &mut r, &none);
}

// ---- 6: outage ----------------------------------------------------------------

#[test]
fn s06_outage() {
    s06_outage_body();
}

fn s06_outage_body() {
    let id = "s06_outage";
    let b = std_world(6, false, 3);
    let mut r = run_world(id, &b);
    if let Some(m) = r.mock.as_mut() {
        m.set_mode(4, 404, 0, 0);
    }
    r.login();
    r.walk();
    r.schedule_folder(root(), true);
    r.settle(true, true);
    r.step("forced_in_backoff");
    r.clock_advance(61.0);
    r.schedule_folder(root(), true);
    r.settle(true, true);
    r.step("forced_after_backoff");
    r.start(root(), false);
    r.settle(true, true);
    r.step("start_known");
    report(id, &mut r, &none);
}

// ---- 7: forced -----------------------------------------------------------------

#[test]
fn s07_forced() {
    s07_forced_body();
}

fn s07_forced_body() {
    let id = "s07_forced";
    let b = std_world(3, false, 3);
    let mut r = run_world(id, &b);
    r.login();
    r.walk();
    let (f, g, h) = (b.ids.user[0], b.ids.user[1], b.ids.user[2]);
    r.clock_advance(181.0);
    r.set_version(f, -1);
    r.cat_fetch(f, 10);
    r.frame(true);
    let fc = r.sched.state().fetch_count;
    r.fact(&format!("in flight={fc}"));
    r.schedule_folder(f, true);
    r.schedule_folder(f, true);
    r.snapshot();
    r.settle(true, true);
    r.step("forced_twice_during_normal");
    r.schedule_folder(g, true);
    r.schedule_folder(h, true);
    r.schedule_folder(g, true);
    r.snapshot();
    r.settle(true, true);
    r.step("forced_g_h_g");
    r.schedule_folder(g, false);
    r.settle(true, true);
    r.step("normal_known");
    r.schedule_item(b.ids.obj1, true);
    r.schedule_item(b.ids.obj1, true);
    r.settle(true, true);
    r.step("forced_item_twice");
    r.schedule_item(b.ids.obj1, true);
    r.settle(true, true);
    r.step("forced_item_again");
    let n0 = r.requests();
    r.schedule_item(b.ids.note1, true);
    r.schedule_item(b.ids.tex1, true);
    r.schedule_item(b.ids.note1, true);
    r.snapshot();
    r.settle(true, true);
    let n = r.requests() - n0;
    r.fact(&format!("forced items X Y X requests={n}"));
    r.step("forced_items_x_y_x");
    let n0 = r.requests();
    r.schedule_item(b.ids.trash1, false);
    r.settle(true, true);
    let n = r.requests() - n0;
    r.fact(&format!("normal fetch of a complete item requests={n}"));
    r.step("complete_item_normal");
    let (d1, d2) = (b.ids.user_sub[0], b.ids.user_sub[3]);
    r.set_version(d1, -1);
    r.set_version(d2, -1);
    let n0 = r.requests();
    r.schedule_folder(d1, false);
    r.schedule_folder(d1, false);
    r.start(d2, false);
    r.start(d2, false);
    r.snapshot();
    let q = r.sched_text();
    r.fact(&format!("dedupe queue {q}"));
    r.settle(true, true);
    let n = r.requests() - n0;
    r.fact(&format!("dedupe requests={n}"));
    r.step("dedupe_schedule_start");
    report(id, &mut r, &none);
}

// ---- 8: items ------------------------------------------------------------------

#[test]
fn s08_items() {
    s08_items_body();
}

fn s08_items_body() {
    let id = "s08_items";
    let b = std_world(3, false, 3);
    let mut r = run_world(id, &b);
    r.login();
    r.cof(b.ids.outfit_a);
    r.walk();
    let u0 = b.ids.items_in[&b.ids.user[0]].clone();
    let u1 = b.ids.items_in[&b.ids.user[1]].clone();
    let (a, bb, l, li) = (u0[0], u0[1], b.ids.cof_shirt_link, b.ids.lib_items[0]);
    let (u, u2) = (pid(0xa0e00000, 1), pid(0xa0e00000, 2));
    for i in [a, bb, l, li] {
        r.set_complete(i, false);
    }
    r.start(a, true);
    // fetchFromServer with AIS: scheduleItemFetch
    r.schedule_item(bb, false);
    r.schedule_item(l, false);
    r.schedule_item(li, false);
    r.schedule_item(u, false);
    r.snapshot();
    r.settle(true, true);
    r.step("item_fetches");
    r.set_complete(u1[0], false);
    r.set_complete(u1[1], false);
    r.items_observer_start(&[u1[0], u1[1], u2]);
    r.snapshot();
    r.settle(true, true);
    r.step("observer");
    r.schedule_item(b.ids.obj2, true);
    r.settle(true, true);
    r.step("forced_complete_item");
    report(id, &mut r, &none);
}

// ---- 15: the subset batch size --------------------------------------------------

#[test]
fn s15_batch_boundaries() {
    s15_batch_boundaries_body();
}

fn s15_batch_boundaries_body() {
    let id = "s15_batch_boundaries";
    let mut b = std_world(1, false, 1);
    let parent = b.ids.user[0];
    let mut tops = Vec::new();
    let mut kids: Vec<Vec<Uuid>> = Vec::new();
    for n in [19u64, 20, 21] {
        let top = pid(0xf3000000, n);
        b.w.add_folder(top, parent, FT_NONE, 3, false);
        tops.push(top);
        let mut k = Vec::new();
        for _ in 1..=n {
            let c = b.folder(top, FT_NONE);
            k.push(c);
            b.item(c);
        }
        kids.push(k);
    }
    let mut r = run_world(id, &b);
    r.login();
    r.walk();
    for (i, top) in tops.iter().enumerate() {
        r.set_version(*top, -1);
        for k in &kids[i] {
            r.set_version(*k, -1);
        }
        r.start(*top, true);
        r.settle(true, true);
        r.step(&format!("children_{}", kids[i].len()));
    }
    for setting in [0, 41] {
        let i = if setting == 0 { 0 } else { 2 };
        r.set_batch_size(setting);
        r.clock_advance(181.0);
        r.set_version(tops[i], -1);
        for k in &kids[i] {
            r.set_version(*k, -1);
        }
        let n0 = r.requests();
        r.start(tops[i], true);
        r.settle(true, true);
        let n = r.requests() - n0;
        r.fact(&format!("BatchSizeAIS3={setting} children={} requests={n}", kids[i].len()));
        r.step(&format!("batch_setting_{setting}"));
    }
    r.set_batch_size(20);
    report(id, &mut r, &none);
}

// ---- 16: the depth cut ------------------------------------------------------------

#[test]
fn s16_depth_boundaries() {
    s16_depth_boundaries_body();
}

fn s16_depth_boundaries_body() {
    let id = "s16_depth_boundaries";
    let mut b = std_world(0, false, 0);
    for levels in [48, 49, 50] {
        let mut parent = b.folder(root(), FT_NONE);
        for _ in 1..=levels {
            parent = b.folder(parent, FT_NONE);
        }
        b.item(parent);
    }
    for levels in [49, 50, 51] {
        let mut parent = libroot();
        for _ in 1..=levels {
            parent = b.libfolder(parent);
        }
        b.libitem(parent);
    }
    let mut r = run_world(id, &b);
    r.login();
    r.walk();
    report(id, &mut r, &none);
}

// ---- 17: no LibraryAPIv3; disconnected --------------------------------------------

#[test]
fn s17_no_library_cap_disconnected() {
    s17_no_library_cap_disconnected_body();
}

fn s17_no_library_cap_disconnected_body() {
    let id = "s17_no_library_cap_disconnected";
    let b = std_world(2, false, 2);
    let mut r = run_world(id, &b);
    r.library_cap = false;
    r.refresh_env();
    r.login();
    r.walk();
    r.set_version(b.ids.user[0], -1);
    r.clock_advance(181.0);
    r.set_disconnected(true);
    r.fetch_descendents_of(b.ids.user[0]);
    r.ais(Command::UpdateItem { item_id: b.ids.note1, updates: Value::Undefined }, Some("disconnected_update"), Vec::new());
    r.frames(10, true);
    r.settle_ex(true, false, true);
    r.step("disconnected");
    r.set_disconnected(false);
    r.settle(true, true);
    r.step("reconnected");
    report(id, &mut r, &none);
}

// ---- 18: fetch errors ----------------------------------------------------------------

#[test]
fn s18_fetch_errors() {
    s18_fetch_errors_body();
}

fn s18_fetch_errors_body() {
    let id = "s18_fetch_errors";
    let mut b = std_world(2, false, 1);
    let f404 = pid(0x4c000000, 1);
    let f403 = pid(0xf4000000, 9);
    b.w.add_folder(f404, root(), FT_NONE, 4, false);
    b.w.add_folder(f403, root(), FT_NONE, 4, false);
    let mut r = run_world(id, &b);
    r.login();
    r.walk();
    for (f, code) in [(f404, "404"), (f403, "403")] {
        for forced in [false, true] {
            r.clock_advance(61.0);
            r.set_version(f, -1);
            let n0 = r.requests();
            r.schedule_folder(f, forced);
            r.settle(true, true);
            let n = r.requests() - n0;
            let kind = if forced { "forced" } else { "normal" };
            r.fact(&format!("{kind} fetch answered {code} requests={n}"));
            r.step(&format!("{kind}_{code}"));
        }
    }
    report(id, &mut r, &none);
}

// ---- 19: the frame budget -----------------------------------------------------------

#[test]
fn s19_frame_budget() {
    s19_frame_budget_body();
}

fn s19_frame_budget_body() {
    let id = "s19_frame_budget";
    let b = std_world(100, false, 1);
    let mut r = run_world(id, &b);
    r.login();
    r.walk();
    let mut known: Vec<Uuid> = b.ids.user_sub.iter().chain(&b.ids.user_leaf).chain(&b.ids.user).copied().collect();
    known.truncate(500);
    for phase in 0..2 {
        let during_login = phase == 1;
        r.set_startup(if during_login { Startup::WearablesWait } else { Startup::Started });
        for k in &known {
            r.schedule_folder(*k, false);
        }
        r.clock = ScriptedClock { now: 2000.0, step: 0.0013 };
        let mut sizes = Vec::new();
        for _ in 0..6 {
            r.frame(true);
            sizes.push(r.sched.state().folder_queue.len().to_string());
        }
        let label = if during_login { "during_login" } else { "after_login" };
        r.fact(&format!("budget {label} step_ms=1.3 queued=500 queue_after_frames=[{}]", sizes.join(",")));
        r.clock = ScriptedClock { now: 3000.0, step: 0.0 };
        r.settle(true, true);
        r.step(label);
    }
    r.set_startup(Startup::Started);
    report(id, &mut r, &none);
}

// ---- 20: callback ids ------------------------------------------------------------------

#[test]
fn s20_callback_ids() {
    s20_callback_ids_body();
}

fn s20_callback_ids_body() {
    let id = "s20_callback_ids";
    let b = std_world(2, true, 2);
    let mut r = run_world(id, &b);
    r.login();
    r.cof(b.ids.outfit_a);
    r.walk();
    let d = b.ids.clone();
    let done = |r: &mut Run, name: &str| {
        r.settle(true, true);
        r.step(name);
    };
    let inv = false;
    let lib = true;
    r.ais(Command::FetchItem { item_id: d.cof_shirt_link, library: inv }, Some("fetch_item_link"), vec![]);
    done(&mut r, "fetch_item_link");
    r.ais(Command::FetchItem { item_id: d.note1, library: inv }, Some("fetch_item"), vec![]);
    done(&mut r, "fetch_item");
    r.ais(Command::FetchItem { item_id: d.lib_items[0], library: lib }, Some("fetch_item_library"), vec![]);
    done(&mut r, "fetch_item_library");
    r.ais(Command::FetchItem { item_id: pid(0xa0e00000, 9), library: inv }, Some("fetch_item_unknown"), vec![]);
    done(&mut r, "fetch_item_unknown");
    let c = Command::FetchCategoryChildren { category_id: d.user[0], library: inv, recursive: false, depth: 1 };
    r.ais(c, Some("children_depth1"), vec![]);
    done(&mut r, "children_depth1");
    let c = Command::FetchCategoryChildren { category_id: d.lib_folders[0], library: lib, recursive: true, depth: 0 };
    r.ais(c, Some("children_library_recursive"), vec![]);
    done(&mut r, "children_library_recursive");
    let c = Command::FetchCategoryChildrenByName { identifier: "current".into(), recursive: false, depth: 0 };
    r.ais(c, Some("children_by_name"), vec![]);
    done(&mut r, "children_by_name");
    let c = Command::FetchCategoryCategories { category_id: d.user[1], library: inv, recursive: false, depth: 2 };
    r.ais(c, Some("categories"), vec![]);
    done(&mut r, "categories");
    let c = Command::FetchCategorySubset {
        category_id: root(),
        children: vec![d.user[1], d.user[0]],
        library: inv,
        recursive: false,
        depth: 3,
    };
    r.ais(c, Some("subset_depth3"), vec![]);
    let c = Command::FetchCategorySubset { category_id: root(), children: vec![], library: inv, recursive: false, depth: 3 };
    r.ais(c, Some("subset_empty"), vec![]);
    done(&mut r, "subset");
    r.ais(Command::FetchCof, Some("cof"), vec![]);
    done(&mut r, "cof");
    r.ais(Command::FetchCategoryLinks { category_id: d.outfit_b }, Some("links"), vec![]);
    done(&mut r, "links");
    r.ais(Command::FetchOrphans, Some("orphans"), vec![]);
    done(&mut r, "orphans");
    r.library_cap = false;
    r.refresh_env();
    r.ais(Command::FetchItem { item_id: d.lib_items[1], library: lib }, Some("library_without_cap"), vec![]);
    done(&mut r, "library_without_cap");
    report(id, &mut r, &none);
}

// ---- 21: priorities ------------------------------------------------------------------

#[test]
fn s21_priority() {
    s21_priority_body();
}

fn s21_priority_body() {
    let id = "s21_priority";
    let b = std_world(6, false, 1);
    let mut r = run_world(id, &b);
    r.model.root = b.w.root;
    r.set_startup(Startup::InventorySend);
    r.inventory_send();
    r.set_startup(Startup::InventorySkel);
    r.inventory_skel();
    r.model.build_parent_child_map();
    r.schedule_folder(b.ids.user[2], false);
    r.start(b.ids.user[3], false);
    r.set_startup(Startup::InventorySend2);
    r.inventory_send2();
    let q = r.sched_text();
    r.fact(&format!("before the first frame {q}"));
    r.set_startup(Startup::InventoryCallbacks);
    r.inventory_callbacks();
    r.step("login_with_queue");
    r.set_startup(Startup::Started);
    r.until(|r| r.requests() >= 3 && r.pool_count() == 0 && r.sched.state().fetch_count == 0);
    r.schedule_folder(b.ids.user_leaf[9], false);
    r.start(b.ids.user_leaf[10], false);
    let item = b.ids.items_in[&b.ids.user_leaf[11]][0];
    r.schedule_item(item, false);
    r.snapshot();
    let q = r.sched_text();
    r.fact(&format!("priority inserted {q}"));
    r.step("inserted");
    r.walk();
    report(id, &mut r, &none);
}

// ---- 22, 23: no library; root only --------------------------------------------------

#[test]
fn s22_no_library() {
    s22_no_library_body();
}

fn s22_no_library_body() {
    let id = "s22_no_library";
    let mut b = Builder::new(false);
    basic(&mut b, 2, false, 0);
    let mut r = run_world(id, &b);
    r.login();
    r.set_startup(Startup::Started);
    r.until(|r| r.requests() >= 24 && r.pool_count() == 0 && r.sched.state().fetch_count == 0);
    let usable = if r.model.usable { 1 } else { 0 };
    r.fact(&format!("inventory usable={usable}"));
    r.set_region(false, CAPS_NONE, true);
    r.settle(true, false);
    r.step("loop_stopped_after_24");
    report(id, &mut r, &none);
}

#[test]
fn s23_root_only_start() {
    s23_root_only_start_body();
}

fn s23_root_only_start_body() {
    let id = "s23_root_only_start";
    let b = std_world(2, false, 1);
    let mut r = run_world(id, &b);
    r.model.root = b.w.root;
    r.inventory_send();
    r.inventory_skel();
    r.model.build_parent_child_map();
    r.start(root(), true);
    r.step("root_started");
    r.walk();
    report(id, &mut r, &none);
}

// ---- 24: shutdown abort ---------------------------------------------------------------

#[test]
fn s24_shutdown_abort() {
    s24_shutdown_abort_body();
}

fn s24_shutdown_abort_body() {
    let id = "s24_shutdown_abort";
    let b = std_world(2, false, 1);
    let mut r = run_world(id, &b);
    r.login();
    r.walk();
    r.clock_advance(181.0);
    r.set_version(b.ids.user[0], -1);
    // checkStop() armed to throw at the next update's first yield
    r.stop_armed = 1;
    r.schedule_folder(b.ids.user[0], false);
    r.until(|r| r.stops_thrown > 0);
    r.frames(30, true);
    let v = r.model.cats[&b.ids.user[0]].version;
    // checkstop_calls and ais_pool are the C++ applier's and pool's: here
    // the scaffolding's (one call; the lost coroutine still counted)
    let pool = r.pool_count();
    let thrown = r.stops_thrown;
    r.fact(&format!("abort thrown={thrown} checkstop_calls=1 ais_pool={pool} version={v}"));
    r.step("aborted");
    report(id, &mut r, &none);
}

// ---- o15a: the library owned by someone else, over AIS ----------------------------------

#[test]
fn o15a_library_owner_ais() {
    o15a_library_owner_ais_body();
}

fn o15a_library_owner_ais_body() {
    let id = "o15a_library_owner_ais";
    let mut b = Builder::new(true);
    b.w.libowner = pid(0x0b1b0000, 1);
    b.w.folders.last_mut().unwrap().owner = b.w.libowner;
    basic(&mut b, 2, false, 2);
    let mut r = run_world(id, &b);
    r.in_second_life = false;
    r.refresh_env();
    r.login();
    r.walk();
    r.fetch_descendents_of(b.ids.lib_folders[1]);
    r.settle(true, true);
    r.step("library_folder");
    report(id, &mut r, &none);
}

// ---- 9: AIS writes ---------------------------------------------------------------------

/// Every write's URL, method, tid and callbacks are the crate's (`ais`);
/// each body is the caller's LLSD (C++ model code), taken from the
/// recording. The model refuses `remove_protected` before any request
/// (`CannotRemoveProtectedCategories`, model code: not compared). The
/// writes' effects on the model are C++ (`AISUpdate`): the dump is not
/// checked.
#[test]
fn s09_writes() {
    s09_writes_body();
}

fn s09_writes_body() {
    let id = "s09_writes";
    let b = std_world(3, false, 3);
    let mut r = run_world(id, &b);
    r.login();
    r.walk();
    let d = b.ids.clone();
    let outfit_c = parse_uuid("c1000000-0000-4000-8000-000000000002");
    let gone_item = pid(0xa0c00000, 1);
    let gone_folder = pid(0xc0c00000, 1);
    let first_note = d.items_in[&d.user[0]][0];
    let second = d.items_in[&d.user[0]][1];
    let mut n = 7;
    // a write with a transaction id gets the last fresh_tid(), as the C++
    // generated it
    let mut write = |r: &mut Run, cmd: Command, tag: Option<&str>, step: &str| {
        let body = recorded_body_n(id, n);
        n += 1;
        let t = if cmd.uses_tid() { r.last_tid() } else { NULL_ID };
        r.ais_tid(cmd, t, tag, body);
        r.settle(true, true);
        r.step(step);
    };
    r.fresh_tid();
    write(&mut r, Command::CreateInventory { parent_id: d.user[0], new_inventory: Value::Undefined }, Some("create_folder"), "create_folder");
    r.fresh_tid();
    write(&mut r, Command::CreateInventory { parent_id: sys(FT_MY_OUTFITS), new_inventory: Value::Undefined }, Some("create_outfit"), "create_outfit");
    r.fresh_tid();
    write(&mut r, Command::CreateInventory { parent_id: outfit_c, new_inventory: Value::Undefined }, Some("link_array"), "link_array");
    r.fresh_tid();
    write(&mut r, Command::CreateInventory { parent_id: outfit_c, new_inventory: Value::Undefined }, Some("link_array_2"), "link_array_2");
    let cof = sys(FT_CURRENT_OUTFIT);
    r.fresh_tid();
    write(&mut r, Command::SlamFolder { folder_id: cof, new_inventory: Value::Undefined }, Some("slam_cof"), "slam_cof");
    r.fresh_tid();
    write(&mut r, Command::SlamFolder { folder_id: cof, new_inventory: Value::Undefined }, Some("slam_cof_2"), "slam_cof_2");
    write(&mut r, Command::UpdateItem { item_id: first_note, updates: Value::Undefined }, Some("update_item_llsd"), "update_item_llsd");
    write(&mut r, Command::UpdateItem { item_id: second, updates: Value::Undefined }, Some("update_item_object"), "update_item_object");
    write(&mut r, Command::UpdateCategory { category_id: d.user[1], updates: Value::Undefined }, Some("update_category"), "update_category");
    write(&mut r, Command::UpdateCategory { category_id: d.user[2], updates: Value::Undefined }, None, "category_update_server");
    write(&mut r, Command::UpdateCategory { category_id: d.outfit_b, updates: Value::Undefined }, None, "change_type");
    write(&mut r, Command::UpdateItem { item_id: d.note1, updates: Value::Undefined }, None, "item_update_server");
    write(&mut r, Command::RemoveItem { item_id: d.obj2 }, Some("remove_item"), "remove_item");
    write(&mut r, Command::RemoveItem { item_id: d.trash1 }, Some("remove_item_immediate"), "remove_item_immediate");
    write(&mut r, Command::RemoveItem { item_id: gone_item }, Some("remove_item_410"), "remove_item_410");
    write(&mut r, Command::RemoveCategory { category_id: d.user_sub[2] }, Some("remove_category"), "remove_category");
    write(&mut r, Command::RemoveCategory { category_id: gone_folder }, Some("remove_category_410"), "remove_category_410");
    r.settle(true, true);
    r.step("remove_protected");
    write(&mut r, Command::PurgeDescendents { category_id: sys(FT_TRASH) }, Some("purge_trash"), "purge_trash");
    r.fresh_tid();
    let c = Command::CopyLibraryCategory { source_id: d.lib_folders[0], dest_id: d.user[0], copy_subfolders: false };
    write(&mut r, c, Some("copy_library_depth0"), "copy_library_depth0");
    r.fresh_tid();
    let c = Command::CopyLibraryCategory { source_id: d.lib_folders[3], dest_id: d.user[0], copy_subfolders: true };
    write(&mut r, c, Some("copy_library_recursive"), "copy_library_recursive");
    report_lines(id, &mut r, &|l| l == "jrn notify CannotRemoveProtectedCategories");
}

// ---- 14: Second Life write fallbacks (no AIS) -------------------------------------------

/// The login walk is the scheduler's (AIS). Every write after it is C++
/// model code without AIS (UDP messages, `CreateInventoryCategory`): its
/// one POST is fed as an input (`foreign_request`, not compared) and its
/// callbacks are not compared; the steps' request counts are.
fn write_fallbacks(id: &str, other_grid: bool) {
    let b = std_world(3, false, 3);
    let mut r = run_world(id, &b);
    if other_grid {
        r.in_second_life = false;
        r.refresh_env();
    }
    r.login();
    r.walk();
    r.set_region(true, CAPS_LEGACY, true);
    r.foreign_request();
    r.settle(true, true);
    r.step("create_cap");
    r.set_region(true, CAPS_NONE, true);
    for step in [
        "create_none",
        "item_update_server",
        "category_update_server",
        "change_type",
        "link_array",
        "update_item_llsd",
        "update_item_object",
        "update_category",
        "remove_item",
        "remove_category_with_item",
        "remove_empty_category",
        "purge_trash",
        "slam_cof",
    ] {
        r.settle(true, true);
        r.step(step);
    }
    report_lines(id, &mut r, &|l| l.starts_with("jrn callback ") || l.starts_with("wire 7 "));
}

#[test]
fn s14_sl_write_fallbacks() {
    s14_sl_write_fallbacks_body();
}

fn s14_sl_write_fallbacks_body() {
    write_fallbacks("s14_sl_write_fallbacks", false);
}

#[test]
fn o14_write_fallbacks() {
    o14_write_fallbacks_body();
}

fn o14_write_fallbacks_body() {
    write_fallbacks("o14_write_fallbacks", true);
}

/// Every scenario above again, its fetch replies built by the reply
/// generator instead of read from the recording: the generator that the
/// scenarios recorded without replies depend on is held to the recorded
/// statuses, requests and model state here. Writes keep their recorded
/// replies.
#[test]
fn generated_replies() {
    GENERATED.with(|g| g.set(true));
    s01_login_cold_body();
    s02a_login_warm_full_body();
    s02b_login_warm_partial_body();
    d2a_sl_wait_login_body();
    d2b_sl_wait_midwalk_body();
    d2c_sl_region_change_wait_body();
    d2d_sl_fsuseais3api_off_body();
    s04_forbidden_body();
    s05_transient_5xx_body();
    s06_outage_body();
    s07_forced_body();
    s08_items_body();
    s15_batch_boundaries_body();
    s16_depth_boundaries_body();
    s17_no_library_cap_disconnected_body();
    s18_fetch_errors_body();
    s19_frame_budget_body();
    s20_callback_ids_body();
    s21_priority_body();
    s22_no_library_body();
    s23_root_only_start_body();
    s24_shutdown_abort_body();
    o15a_library_owner_ais_body();
    s09_writes_body();
    s14_sl_write_fallbacks_body();
    o14_write_fallbacks_body();
}

/// s03: 5,000 folders, 60,000 items, 300 under the root: 15 subset batches
/// and the 60-deep chain's follow-up. Recorded without replies (large):
/// the replies are generated, each status held to the recorded one; the
/// journal and model are recorded as digests over the C++ model's lines
/// too (observer batches), so only the requests, counts and public state
/// are compared.
#[test]
fn s03_large() {
    let id = "s03_large";
    let mut b = Builder::new(true);
    large(&mut b);
    let mut r = run_world(id, &b);
    r.mock = Some(r3b_support::mock::Mock::new(&b.w));
    r.login();
    r.cof(b.ids.outfit_a);
    r.walk();
    report_lines(id, &mut r, &|l| l.starts_with("jrn "));
}
