//! R4a set 3: serial and request-state traces (`fixtures/task/README.md`,
//! "Set 3").
//!
//! - `serial_traces_hand.txt`: the 54 hand-written traces;
//! - `serial_traces_len1_2.txt`: every sequence of 1 and 2 events per
//!   configuration (udp/L0, udp/L1, cap/L0, cap/L1), in full;
//! - `serial_traces_len3_samples.txt`, `serial_traces_len4_samples.txt`
//!   and `digests_set3.txt`: every sequence of 3 and 4 events, every 97th
//!   kept as a sample, and all four lengths by digest (66, 1,090, 18,018
//!   and 298,114 traces), whole and per block of 97.

mod common;
mod r4a;

use r4a::*;

const UDP_ALPHABET: [&str; 17] = [
    "req",
    "dirty",
    "rep.lo",
    "rep.cur",
    "rep.exp",
    "rep.hi",
    "rep.none",
    "xfer.ok",
    "xfer.err",
    "xfer.nofile",
    "timer",
    "upd.known",
    "upd.guess",
    "upd.new",
    "remove",
    "move.nocopy",
    "create.ok",
];

const CAP_ALPHABET: [&str; 16] = [
    "req",
    "dirty",
    "rep.lo",
    "rep.exp",
    "cap.ok",
    "cap.stale",
    "cap.304",
    "cap.err",
    "cap.nocontents",
    "timer",
    "upd.known",
    "upd.guess",
    "upd.new",
    "remove",
    "move.nocopy",
    "create.ok",
];

/// `hand_traces()`: label, capability, listeners, events.
const HAND_TRACES: &[(&str, bool, u32, &str)] = &[
    ("udp_load", false, 1, "req rep.exp xfer.ok"),
    ("udp_load_no_listener", false, 0, "req rep.exp xfer.ok req"),
    ("cap_load", true, 1, "req cap.ok"),
    ("cap_url_never_has_serial", true, 1, "req cap=7 dirty req"),
    (
        "cap_url_after_local_edits",
        true,
        0,
        "req cap=7 upd.known req dirty req",
    ),
    (
        "wrap_update_new",
        false,
        1,
        "set.serial=32767 set.expected=32767 upd.new rep=-32768 xfer.ok",
    ),
    (
        "wrap_old_reply_loaded",
        false,
        1,
        "set.serial=32767 set.expected=32767 upd.new rep=32767 xfer.ok",
    ),
    (
        "wrap_reply_minus_one",
        false,
        1,
        "set.serial=-32768 set.expected=-32768 rep=32767 xfer.ok",
    ),
    (
        "wrap_loop_protection",
        false,
        1,
        "set.serial=32767 set.expected=-32768 rep=32767 timer",
    ),
    (
        "wrap_cap",
        true,
        1,
        "set.serial=32767 set.expected=32767 upd.new req cap=-32768",
    ),
    (
        "wrap_cap_stale",
        true,
        1,
        "set.serial=32767 set.expected=32767 upd.new req cap=32767 timer",
    ),
    ("wrap_cap_out_of_range", true, 1, "req cap=32768"),
    (
        "wrap_remove",
        false,
        1,
        "req rep.exp xfer.ok set.expected=32767 remove",
    ),
    (
        "wrap_guess_down",
        false,
        1,
        "set.expected=-32768 req rep=-32768 xfer.ok upd.guess",
    ),
    (
        "stuck_xfer_error",
        false,
        1,
        "req rep.exp xfer.err req req timer",
    ),
    (
        "stuck_xfer_error_unstuck_by_reply",
        false,
        1,
        "req rep.exp xfer.err rep.exp xfer.ok",
    ),
    (
        "older_serial_file_ignored",
        false,
        1,
        "req rep=1 rep.none rep=2 xfer.ok req xfer.ok",
    ),
    (
        "stuck_older_serial_file_then_error",
        false,
        1,
        "req rep=1 rep.none rep=2 xfer.ok xfer.err req timer",
    ),
    (
        "newer_reply_aborts_older_xfer",
        false,
        1,
        "req rep=1 rep=2 xfer.ok.newest xfer.ok",
    ),
    (
        "stuck_older_serial_file_2",
        false,
        1,
        "req rep=2 xfer.ok rep=1 xfer.ok req",
    ),
    (
        "stuck_missing_file",
        false,
        1,
        "req rep.exp xfer.nofile req",
    ),
    (
        "stuck_object_gone_during_xfer",
        false,
        1,
        "req rep.exp die xfer.ok revive req",
    ),
    (
        "stuck_cap_no_contents",
        true,
        1,
        "req cap.nocontents req dirty req",
    ),
    (
        "stuck_cap_no_contents_reply_does_not_unstick",
        true,
        1,
        "req cap.nocontents rep.exp rep.lo rep.hi timer req",
    ),
    (
        "stuck_cap_object_gone",
        true,
        1,
        "req die cap.ok revive req",
    ),
    (
        "stuck_aborted_xfer",
        false,
        1,
        "req rep.exp rep.lo xfer.ok timer",
    ),
    (
        "xfer_completion_records_serial",
        false,
        1,
        "rep.hi rep.none rep.lo xfer.ok",
    ),
    (
        "object_gone_during_delay_udp",
        false,
        1,
        "req upd.new rep.lo die timer revive req",
    ),
    (
        "object_gone_during_delay_cap",
        true,
        1,
        "req upd.new rep.lo die timer revive req",
    ),
    ("cap_error_not_stuck", true, 1, "req cap.err req cap.ok"),
    ("cap_304", true, 1, "req cap.304 req cap.ok"),
    (
        "cap_304_after_edit",
        true,
        1,
        "req cap.ok upd.new dirty req cap.304 timer cap.ok",
    ),
    (
        "loop_protection",
        false,
        1,
        "req rep.exp xfer.ok upd.new rep.cur timer rep.exp xfer.ok",
    ),
    ("out_of_date_reply", false, 1, "req upd.new rep.lo timer"),
    (
        "newer_reply_replaces_xfer",
        false,
        1,
        "req rep.exp rep.hi xfer.ok xfer.ok",
    ),
    (
        "same_reply_twice",
        false,
        1,
        "req rep.exp rep.exp xfer.ok xfer.ok",
    ),
    (
        "empty_filename_mock",
        false,
        1,
        "req rep.none upd.new remove",
    ),
    (
        "unsolicited_reply_cap_lowers_expected",
        true,
        1,
        "req cap=5 upd.new upd.new rep.lo",
    ),
    ("unsolicited_reply_udp", false, 1, "rep.exp xfer.ok"),
    (
        "delayed_twice",
        false,
        1,
        "req upd.new rep.lo rep.lo timer timer",
    ),
    (
        "update_known_owner",
        false,
        1,
        "req rep.exp xfer.ok upd.known",
    ),
    (
        "update_guess_owner",
        false,
        1,
        "req rep.exp xfer.ok upd.guess",
    ),
    (
        "update_guess_not_owner",
        false,
        1,
        "notowner req rep.exp xfer.ok upd.guess",
    ),
    (
        "update_selected",
        false,
        1,
        "select req rep.exp xfer.ok upd.known upd.guess",
    ),
    ("update_by_asset", false, 1, "req rep.exp xfer.ok upd.asset"),
    (
        "update_no_inventory",
        false,
        1,
        "upd.known upd.guess upd.new",
    ),
    ("remove_absent", false, 1, "req rep.none remove"),
    (
        "move_copy_and_nocopy",
        false,
        1,
        "req rep.exp xfer.ok move.copy move.nocopy move.nocopy",
    ),
    ("create_ok_udp_no_cap", false, 1, "create.ok"),
    ("create_ok_cap", true, 1, "req cap.ok create.ok cap.ok"),
    ("create_fail_cap", true, 1, "req cap.ok create.fail"),
    (
        "dirty_with_listener",
        false,
        1,
        "req rep.exp xfer.ok dirty req",
    ),
    (
        "dirty_without_listener",
        false,
        0,
        "req rep.exp xfer.ok dirty req listen req",
    ),
    (
        "two_listeners",
        false,
        2,
        "req rep.exp xfer.ok unlisten upd.new",
    ),
    (
        "clear_listeners",
        false,
        2,
        "req rep.exp xfer.ok clearlisteners upd.new remove req",
    ),
];

/// `CONFIGS`: capability, listeners.
const CONFIGS: [(bool, u32); 4] = [(false, 0), (false, 1), (true, 0), (true, 1)];

#[test]
fn set3_hand_traces() {
    let mut text = String::new();
    for &(label, caps, listeners, events) in HAND_TRACES {
        let ev: Vec<&str> = events.split_whitespace().collect();
        text += &run_trace(caps, listeners, &ev, label);
    }
    assert_text("serial_traces_hand.txt", text.as_bytes());
}

/// Every sequence of exactly `length` events, lexicographic by alphabet
/// index (`for_each_sequence`).
fn for_each_sequence(alphabet: &[&'static str], length: usize, mut f: impl FnMut(&[&'static str])) {
    let mut idx = vec![0usize; length];
    let mut seq = vec![""; length];
    loop {
        for i in 0..length {
            seq[i] = alphabet[idx[i]];
        }
        f(&seq);
        let mut k = length;
        loop {
            if k == 0 {
                return;
            }
            k -= 1;
            idx[k] += 1;
            if idx[k] < alphabet.len() {
                break;
            }
            idx[k] = 0;
        }
    }
}

/// One length of the sweep: its digest, and its full text (lengths 1-2)
/// or its samples (3-4).
fn sweep(length: usize) -> ((String, Vec<String>), String, u64) {
    let mut digest = DigestBuilder::new(&format!("serial_traces_len{length}"));
    let mut kept = String::new();
    let mut count = 0u64;
    for (caps, listeners) in CONFIGS {
        let alphabet: &[&'static str] = if caps { &CAP_ALPHABET } else { &UDP_ALPHABET };
        for_each_sequence(alphabet, length, |seq| {
            let t = run_trace(caps, listeners, seq, "");
            digest.add(t.as_bytes());
            if length <= 2 || count % SAMPLE_EVERY == 0 {
                kept += &t;
            }
            count += 1;
        });
    }
    (digest.finish(), kept, count)
}

#[test]
fn set3_sweeps() {
    let mut digests = Vec::new();
    let mut short_text = String::new();
    for length in 1..=4 {
        let (d, kept, count) = sweep(length);
        let want = [66u64, 1090, 18018, 298114][length - 1];
        assert_eq!(count, want, "traces of length {length}");
        if length <= 2 {
            short_text += &kept;
        } else {
            assert_text(
                &format!("serial_traces_len{length}_samples.txt"),
                kept.as_bytes(),
            );
        }
        digests.push(d);
    }
    assert_text("serial_traces_len1_2.txt", short_text.as_bytes());
    assert_lines("digests_set3.txt", &digest_lines(digests));
}
