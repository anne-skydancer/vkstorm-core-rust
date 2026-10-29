//! R4a set 1: task inventory files (`fixtures/task/README.md`, "Set 1").
//!
//! - `task_files.txt`: every named case of `files/` through
//!   `loadTaskInvFile`, the three `via_xfer` cases through
//!   `ReplyTaskInventory` -> xfer -> `processTaskInvFile` with a pending
//!   list, and `missing_file`;
//! - `scrub.txt`: the `Filename` values through the builder and reader,
//!   `getScrubbedFileName` and `processTaskInv` with (a model of) the real
//!   `LLXferManager`, and the `RequestXfer` datagram;
//! - `task_files_random_samples.txt` and `digests_set1.txt`: the 3,000
//!   seeded mutations of `files/normal.inv`, the samples' inputs byte for
//!   byte, the samples' text, and every case by digest.

mod common;
mod r4a;

use r4a::*;

/// The named cases, in the recorder's order (`file_cases()`), and whether
/// each also runs through the xfer.
const FILE_CASES: &[(&str, bool)] = &[
    ("empty", false),
    ("whitespace_only", false),
    ("newline_only", false),
    ("object_only", false),
    ("normal", true),
    ("item_before_object", false),
    ("items_only", false),
    ("two_objects", false),
    ("object_renamed", false),
    ("object_texture_type", false),
    ("blank_line_between_blocks", false),
    ("whitespace_line_between_blocks", false),
    ("blank_line_after_permissions", false),
    ("blank_line_after_sale_info", false),
    ("crlf", false),
    ("no_trailing_newline", false),
    ("eof_inside_item", false),
    ("eof_after_keyword", false),
    ("keyword_without_block", false),
    ("desc_line_253", false),
    ("desc_line_254", false),
    ("desc_line_255", false),
    ("desc_line_300", false),
    ("toplevel_line_253", false),
    ("toplevel_line_254", false),
    ("toplevel_line_255", false),
    ("toplevel_line_256", false),
    ("toplevel_line_300", false),
    ("item_type_missing_shares_asset", false),
    ("duplicate_item_id", false),
    ("metadata_line_over_254", false),
    ("unknown_24", false),
    ("unknown_24_between", false),
    ("unknown_25", false),
    ("unknown_26", false),
    ("unknown_25_between", false),
    ("unknown_26_between", false),
    ("unknown_inside_block", false),
    ("open_brace_top_level", false),
    ("close_brace_top_level", false),
    ("keyword_with_suffix", false),
    ("missing_close_brace", false),
    ("nul_in_line", false),
    ("shadow_id", false),
    ("shadow_then_asset", false),
    ("asset_then_shadow", false),
    ("metadata_all_268_bytes", false),
    ("metadata_thumbnail_favorite", false),
    ("metadata_runtime", false),
    ("metadata_written_by_exporter", false),
    ("metadata_not_llsd", false),
    ("metadata_malformed", false),
    ("object_metadata", false),
    ("name_empty", false),
    ("name_pipe_only", false),
    ("name_no_pipe", false),
    ("name_space_separator", false),
    ("name_leading_tabs", false),
    ("name_leading_spaces", false),
    ("name_inner_tab", false),
    ("name_utf8", false),
    ("name_control", false),
    ("name_long_80", false),
    ("name_localizable", false),
    ("desc_empty", false),
    ("desc_no_pipe", false),
    ("desc_pipe_inside", false),
    ("desc_utf8", false),
    ("object_name_empty", false),
    ("object_name_spaces", false),
    ("invtype_mismatch", false),
    ("type_keyword_only", false),
    ("name_keyword_only", false),
    ("desc_keyword_only", false),
    ("invtype_missing", false),
    ("invtype_unknown", false),
    ("type_unknown", false),
    ("sale_perm_mask_none", false),
    ("sale_perm_mask_nocopy", false),
    ("sale_perm_mask_full", false),
    ("sale_for_sale", false),
    ("sale_price_negative", false),
    ("flags_0x", false),
    ("flags_minus_one", false),
    ("flags_ffffffff", false),
    ("flags_nonhex", false),
    ("date_negative", false),
    ("date_int_max", false),
    ("date_over_int_max", false),
    ("perm_masks_raw", false),
    ("group_owned", false),
    ("uuid_uppercase", false),
    ("uuid_bad", false),
    ("pending_cleared", true),
    ("pending_none", true),
    ("hundred_items", false),
];

/// `run_file_case`.
fn run_file_case(label: &str, bytes: &[u8], input: &str, via_xfer: bool) -> String {
    let mut out = format!(
        "case {label} input={input} bytes={} sha256={}\n",
        bytes.len(),
        sha256(bytes)
    );
    {
        let mut s = Scene::new(false, 1, false);
        let name = format!("task_{label}.inv").into_bytes();
        s.write_file(&name, bytes);
        let returned = s.load_task_inv_file(&name);
        let removed = !s.exists(&name);
        let line = s.state_line();
        let actions = s.take();
        out += &format!(
            " direct returned={} removed={} {} | {}\n",
            i32::from(returned),
            i32::from(removed),
            line,
            actions
        );
        out += &s.entries_text(" ");
    }
    if via_xfer {
        let mut s = Scene::new(false, 1, false);
        s.obj.pending = vec![asset_pending(), asset('e'), asset('a')];
        s.request_inventory();
        s.deliver_reply(1, format!("task_{label}.inv").as_bytes());
        if let Some(i) = s.live_xfer(false) {
            let x = s.take_xfer(i);
            s.write_file(&x.local, bytes);
            s.xfer_callback(&x.ft, 0);
        }
        let pending: Vec<String> = s.obj.pending.iter().map(nm).collect();
        let line = s.state_line();
        let actions = s.take();
        out += &format!(
            " via_xfer {} pending=[{}] | {}\n",
            line,
            pending.join(","),
            actions
        );
    }
    out
}

fn run_missing_file_case() -> String {
    let mut s = Scene::new(false, 1, false);
    let returned = s.load_task_inv_file(b"task_absent.inv");
    let line = s.state_line();
    let actions = s.take();
    format!(
        "case missing_file input=- bytes=-1\n direct returned={} {} | {}\n",
        i32::from(returned),
        line,
        actions
    ) + &s.entries_text(" ")
}

#[test]
fn set1_task_files() {
    let mut text = String::new();
    for &(label, via) in FILE_CASES {
        let input = format!("files/{label}.inv");
        let bytes = read_bytes(&input);
        text += &run_file_case(label, &bytes, &input, via);
    }
    text += &run_missing_file_case();
    assert_text("task_files.txt", text.as_bytes());
    // the fixture tree holds exactly these inputs
    let count = std::fs::read_dir(fixture_path("files")).unwrap().count();
    assert_eq!(count, FILE_CASES.len(), "files/ holds {count} inputs");
}

/// The inputs the recorder writes are the ones its own writers make: the
/// standard file is `pending_none`'s input (Contents, A, B).
#[test]
fn set1_standard_file_matches_recorder() {
    assert_bytes("files/pending_none.inv", standard_file());
}

/// `scrub_names`.
fn scrub_names() -> Vec<Vec<u8>> {
    let mut n: Vec<Vec<u8>> = [
        "",
        "a",
        "ab",
        "abc",
        "abcd",
        ".tmp",
        "x.tm",
        "a.tmp",
        ".",
        "..",
        "...",
        "../x",
        "..\\x",
        "a/..",
        "../../etc/passwd",
        "inventory_task.tmp",
    ]
    .iter()
    .map(|s| s.as_bytes().to_vec())
    .collect();
    let forbidden = b"\\/:*?\"<>|";
    for &c in forbidden {
        let mut v = b"inv".to_vec();
        v.push(c);
        v.extend_from_slice(b"x.tmp");
        n.push(v);
    }
    let mut all = b"all".to_vec();
    all.extend_from_slice(forbidden);
    all.extend_from_slice(b".tmp");
    n.push(all);
    n.push(forbidden.to_vec());
    n.push(b"|".to_vec());
    n.push(b"::".to_vec());
    n.push(b"C:\\x.tmp".to_vec());
    n.push(b"tab\tname.tmp".to_vec());
    n.push(b"ctl\x01.tmp".to_vec());
    n.push(vec![b'f'; 254]);
    n.push(vec![b'f'; 255]);
    n.push(vec![b'f'; 300]);
    n
}

/// `run_scrub_case`.
fn run_scrub_case(raw: &[u8]) -> String {
    let mut out = format!("scrub raw={} rawlen={}", esc(raw), raw.len());
    let (_, _, read_back) = reply_round_trip(&obj_id(), 0, raw);
    let scrubbed = vkstorm_inventory::task::scrubbed_file_name(&read_back);
    out += &format!(
        " received={} scrubbed={} len={}\n",
        esc(&read_back),
        esc(&scrubbed),
        scrubbed.len()
    );
    let mut s = Scene::new(false, 1, true);
    s.capture = Some(Vec::new());
    let mut planted = false;
    if !scrubbed.is_empty()
        && !scrubbed.iter().any(|&c| c == b'\t' || c == 1)
        && scrubbed.len() < 200
        && scrubbed.iter().any(|&c| c != b'.')
        && !scrubbed.windows(2).any(|w| w == b"..")
    {
        s.write_file(&scrubbed, b"sentinel");
        planted = s.exists(&scrubbed);
    }
    s.deliver_reply(0, raw);
    let remaining = planted && s.exists(&scrubbed);
    let line = s.state_line();
    let actions = s.take();
    out += &format!(
        " outcome returned planted={} remaining={} {} | {}\n",
        i32::from(planted),
        i32::from(remaining),
        line,
        actions
    );
    let ids = s.real_ids();
    out += &s.entries_text(" ");
    let packets = s.capture.take().unwrap();
    out += &packets_text(&packets, &ids);
    out
}

#[test]
fn set1_scrub() {
    let mut text = String::new();
    for raw in scrub_names() {
        text += &run_scrub_case(&raw);
    }
    assert_text("scrub.txt", text.as_bytes());
}

/// `mutate_file`: the README's generator.
fn mutate_file(rng: &mut Rng, base: &[u8]) -> Vec<u8> {
    let mut lines: Vec<Vec<u8>> = Vec::new();
    let mut p = 0;
    while p < base.len() {
        match base[p..].iter().position(|&b| b == b'\n') {
            None => {
                lines.push(base[p..].to_vec());
                break;
            }
            Some(e) => {
                lines.push(base[p..p + e + 1].to_vec());
                p += e + 1;
            }
        }
    }
    const TOKENS: [&[u8]; 13] = [
        b"\tinv_item\t0\n",
        b"\tinv_object\t0\n",
        b"\t{\n",
        b"\t}\n",
        b"\n",
        b" \n",
        b"\tjunk\t1\n",
        b"\t\tname\tX|\n",
        b"\t\ttype\ttexture\n",
        b"\tpermissions 0\n",
        b"\tsale_info\t0\n",
        b"\t\tflags\t-1\n",
        b"\t\tsale_price\tx\n",
    ];
    const PROTECTED: [&[u8]; 12] = [
        b"creation_date",
        b"base_mask",
        b"owner_mask",
        b"group_mask",
        b"everyone_mask",
        b"next_owner_mask",
        b"creator_mask",
        b"creator_id",
        b"owner_id",
        b"last_owner_id",
        b"group_id",
        b"group_owned",
    ];
    let is_protected = |line: &[u8]| {
        let scan = vkstorm_inventory::legacy::scanf::sscanf(line, " %63s");
        scan.ret == 1 && PROTECTED.contains(&scan.str(0).unwrap_or_default())
    };
    let ops = 1 + rng.below(4);
    let mut k = 0;
    while k < ops && !lines.is_empty() {
        k += 1;
        let at = rng.below(lines.len() as u32) as usize;
        let mut op = rng.below(7);
        if op >= 4 && is_protected(&lines[at]) {
            op = 1;
        }
        match op {
            0 => {
                lines.remove(at);
            }
            1 => {
                let l = lines[at].clone();
                lines.insert(at, l);
            }
            2 => {
                let t = TOKENS[rng.below(TOKENS.len() as u32) as usize].to_vec();
                lines.insert(at, t);
            }
            3 => {
                let j = rng.below(lines.len() as u32) as usize;
                lines.swap(at, j);
            }
            4 => {
                if !lines[at].is_empty() {
                    let value = (0x20 + rng.below(0x5f)) as u8;
                    let pos = rng.below(lines[at].len() as u32) as usize;
                    lines[at][pos] = value;
                }
            }
            5 => {
                let n = rng.below(lines[at].len() as u32 + 1) as usize;
                lines[at].truncate(n);
            }
            _ => {
                let count = rng.below(300) as usize;
                let pos = rng.below(lines[at].len() as u32 + 1) as usize;
                let l = &mut lines[at];
                l.splice(pos..pos, std::iter::repeat_n(b'm', count));
            }
        }
    }
    lines.concat()
}

#[test]
fn set1_random_sweep() {
    let base = read_bytes("files/normal.inv");
    let mut rng = Rng::new(SEED_FILES);
    let mut digest = DigestBuilder::new("task_files_random");
    let mut samples = String::new();
    let mut kept = 0;
    for i in 0..FILE_SWEEP_COUNT {
        let label = format!("random_{i:04}");
        let bytes = mutate_file(&mut rng, &base);
        let input = format!("random/{label}.inv");
        let result = run_file_case(&label, &bytes, &input, false);
        digest.add(result.as_bytes());
        if i as u64 % SAMPLE_EVERY == 0 {
            assert_bytes(&input, &bytes);
            samples += &result;
            kept += 1;
        }
    }
    assert_eq!(
        std::fs::read_dir(fixture_path("random")).unwrap().count(),
        kept
    );
    assert_text("task_files_random_samples.txt", samples.as_bytes());
    assert_lines("digests_set1.txt", &digest_lines(vec![digest.finish()]));
}
