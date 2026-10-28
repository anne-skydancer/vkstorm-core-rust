//! Replays the sequence-mode (SEQ) scenarios of the R3b fetch wire corpus
//! (`fixtures/ais/fetch`): the scheduler and request building of
//! `vkstorm_inventory::fetch` driven through the recorded scenario steps and
//! replies, reproducing every request (method, target, head, body, request
//! counts), the scheduler state lines, completion signals, callbacks and
//! notifications byte for byte. See `r3b_support` for what stands in for the
//! C++ model and transport, and which lines are left to the viewer test.

mod r3b_support;

use r3b_support::*;

fn run_world(id: &str, b: &Builder) -> Run {
    let lines = scenario_lines(id);
    Run::new(id, b.w.clone(), &lines)
}

fn none(_: &str) -> bool {
    false
}

#[test]
fn s01_login_cold() {
    let id = "s01_login_cold";
    let mut b = Builder::new(true);
    basic(&mut b, 6, true, 3);
    let mut r = run_world(id, &b);
    r.login();
    r.cof(b.ids.outfit_a);
    r.walk();
    r.find_lost_items();
    r.settle(true, true);
    r.step("orphans");
    r.step("logout_cache");
    let (n, total) = check(id, &mut r, &none);
    eprintln!("{id}: {n} of {total} lines compared");
}
