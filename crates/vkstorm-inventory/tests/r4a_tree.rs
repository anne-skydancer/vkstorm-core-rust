//! The task fixture tree holds exactly the files the R4a replays read, plus
//! `README.md` and `.gitattributes` (the crate's counterpart of the
//! oracle's test 7). The replays themselves check each input byte for byte
//! and the number of inputs in `files/`, `replies/`, `create/`,
//! `random/` and `replies_random/`; this test checks the names.

mod common;
mod r4a;

use std::collections::BTreeSet;

use r4a::*;

fn names_in(dir: &str) -> BTreeSet<String> {
    std::fs::read_dir(fixture_path(dir))
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect()
}

#[test]
fn fixture_tree_is_exact() {
    let top: BTreeSet<String> = [
        ".gitattributes",
        "README.md",
        "cap_replies.txt",
        "cap_replies_random_samples.txt",
        "create",
        "create_item.txt",
        "digests_set1.txt",
        "digests_set2.txt",
        "digests_set3.txt",
        "files",
        "messages.txt",
        "random",
        "replies",
        "replies_random",
        "scrub.txt",
        "serial_traces_hand.txt",
        "serial_traces_len1_2.txt",
        "serial_traces_len3_samples.txt",
        "serial_traces_len4_samples.txt",
        "task_files.txt",
        "task_files_random_samples.txt",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    assert_eq!(names_in(""), top);
    let samples = |n: u64, ext: &str| -> BTreeSet<String> {
        (0..n).filter(|i| i % SAMPLE_EVERY == 0).map(|i| format!("random_{i:04}.{ext}")).collect()
    };
    assert_eq!(names_in("random"), samples(FILE_SWEEP_COUNT as u64, "inv"));
    assert_eq!(names_in("replies_random"), samples(REPLY_SWEEP_COUNT as u64, "llsd"));
    let create: BTreeSet<String> = ["script", "lua_script_params", "notecard", "server_says_no", "http_error"]
        .iter()
        .map(|s| format!("{s}.llsd"))
        .collect();
    assert_eq!(names_in("create"), create);
    assert!(names_in("files").iter().all(|n| n.ends_with(".inv")));
    assert!(names_in("replies").iter().all(|n| n.ends_with(".llsd")));
}
