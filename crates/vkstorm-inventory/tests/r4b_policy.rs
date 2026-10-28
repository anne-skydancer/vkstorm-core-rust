//! R4b offer policy tables: regenerates the canonical streams of
//! `fixtures/offer/policy_present`, `policy_response` and `policy` from the
//! crate's offer decisions run against the harness's fakes, and checks each
//! against its digest (whole stream and chunks), its sample (aligned by
//! case id; notes are not compared) and, for the two policy tables, every
//! outcome class (`policy.outcomes.txt` in full, `policy_response.outcomes.txt`
//! by each class's SHA-256).

mod r4b_common;

use r4b_common::incoming::*;
use r4b_common::*;

#[test]
fn policy_present() {
    let mut col = Collect::new(8, 512, false);
    for_each_present_case(|pc| col.lines_from(run_policy_case(pc, None)));
    check_sweep("offer", "policy_present", col, None);
}

#[test]
fn policy_response() {
    let mut col = Collect::new(1201, 2048, true);
    for_each_response_case(|pc| col.lines_from(run_policy_case(pc, None)));
    check_sweep("offer", "policy_response", col, Some(true));
}

#[test]
fn policy() {
    let mut col = Collect::new(8009, 8192, true);
    for_each_policy_case(|pc| col.lines_from(run_policy_case(pc, None)));
    check_sweep("offer", "policy", col, Some(false));
}
