//! R4a set 2: `RequestTaskInventory` capability replies
//! (`fixtures/task/README.md`, "Set 2").
//!
//! - `cap_replies.txt`: every named reply of `replies/` (binary LLSD, read
//!   with `vkstorm-llsd`; its notation compared with the case's `body`
//!   line), delivered as HTTP 200 after `requestInventory()` on an object
//!   whose region has the capability;
//! - `cap_replies_random_samples.txt` and `digests_set2.txt`: the 2,000
//!   seeded bodies built here (`LLInventoryItem::asLLSD` of R1), the
//!   samples' binary LLSD byte for byte, the samples' text, and every case
//!   by digest.

mod common;
mod r4a;

use r4a::*;
use vkstorm_inventory::llsd::coerce::insert;
use vkstorm_inventory::perms::NULL_ID;
use vkstorm_inventory::sale::SaleType;
use vkstorm_inventory::types::{AssetType, InventoryType};
use vkstorm_llsd::{Format, Limits, Value};

/// The named cases in the recorder's order (`reply_cases()`), with the
/// expected serial before the request; `body_empty` has no body.
const REPLY_CASES: &[(&str, i16)] = &[
    ("standard", 0),
    ("standard_stale", 6),
    ("contents_missing", 0),
    ("contents_undefined", 0),
    ("contents_empty", 0),
    ("contents_map", 0),
    ("contents_string", 0),
    ("entries_without_item_id", 0),
    ("running_faulted", 0),
    ("serial_int_0", 0),
    ("serial_int_1", 0),
    ("serial_int_32767", 0),
    ("serial_int_32768", 0),
    ("serial_int_65535", 0),
    ("serial_int_65536", 0),
    ("serial_int_-1", 0),
    ("serial_int_-32768", 0),
    ("serial_int_-32769", 0),
    ("serial_int_2147483647", 0),
    ("serial_int_-2147483648", 0),
    ("serial_missing", 0),
    ("serial_string", 0),
    ("serial_string_junk", 0),
    ("serial_real", 0),
    ("serial_real_negative", 0),
    ("serial_bool", 0),
    ("serial_uuid", 0),
    ("serial_below_expected_wrap", -32768),
    ("localized_names", 0),
    ("item_variants", 0),
    ("item_malformed", 0),
    ("body_not_map", 0),
    ("body_empty", 0),
];

/// `run_reply_case`.
fn run_reply_case(label: &str, body: Option<&Value>, input: &str, expected: i16) -> String {
    let mut out = format!("case {label} input={} expected_before={expected}\n", if body.is_some() { input } else { "-" });
    out += &format!("  body {}\n", body.map_or("<no body>".to_owned(), notation));
    let mut s = Scene::new(true, 1, false);
    s.obj.st.expected = expected;
    s.request_inventory();
    let line = s.state_line();
    let actions = s.take();
    out += &format!(" request {line} | {actions}\n");
    s.complete_http(body.cloned(), 200, None);
    let line = s.state_line();
    let actions = s.take();
    out += &format!(" reply {line} | {actions}\n");
    out += &s.entries_text(" ");
    out
}

fn parse_binary(bytes: &[u8]) -> Value {
    let (n, v) = vkstorm_llsd::parse(Format::Binary, bytes, Limits::for_len(bytes.len()));
    assert!(n > 0, "binary LLSD parses");
    v.expect("a value")
}

#[test]
fn set2_cap_replies() {
    let mut text = String::new();
    let mut files = 0;
    for &(label, expected) in REPLY_CASES {
        let input = format!("replies/{label}.llsd");
        let body = (label != "body_empty").then(|| parse_binary(&read_bytes(&input)));
        if let Some(b) = &body {
            // the input is the recorder's body, byte for byte
            assert_bytes(&input, &binary_llsd(b));
            files += 1;
        }
        text += &run_reply_case(label, body.as_ref(), &input, expected);
    }
    assert_text("cap_replies.txt", text.as_bytes());
    assert_eq!(std::fs::read_dir(fixture_path("replies")).unwrap().count(), files);
}

/// `random_reply`: the README's generator.
fn random_reply(rng: &mut Rng) -> Value {
    const NAMES: [&[u8]; 9] = [b"New Script", b"Contents", b"Thing", b"", b"  padded  ", b"dance8", b"a|b", b"\xc3\xa9", b"New Folder"];
    const TYPES: [i32; 8] = [7, 6, 10, 0, 24, 8, 57, 56];
    const INV_TYPES: [i32; 6] = [7, 6, 10, 0, -1, 26];
    // `LLSD body;`: undefined until a key is set
    let mut body = Value::Undefined;
    match rng.below(6) {
        0 => {}
        1 => insert(&mut body, "inventory_serial", Value::Integer((rng.next() & 0xffff_ffff) as u32 as i32)),
        2 => {
            let v = rng.below(70000) as i32 - 35000;
            insert(&mut body, "inventory_serial", s(&v.to_string()));
        }
        _ => {
            let v = rng.below(70000) as i32 - 35000;
            insert(&mut body, "inventory_serial", Value::Integer(v));
        }
    }
    if rng.below(10) == 0 {
        return body;
    }
    let mut arr = Vec::new();
    let n = rng.below(6);
    for _ in 0..n {
        let kind = rng.below(10);
        if kind == 0 {
            arr.push(s("junk"));
            continue;
        }
        if kind == 1 {
            arr.push(map(vec![("name", s("no id"))]));
            continue;
        }
        let mut sp = ItemSpec::default();
        sp.id = uid(&format!("1a000000-0000-4000-8000-{:012x}", rng.next() & 0xffff));
        sp.asset = uid(&format!("a5000000-0000-4000-8000-{:012x}", rng.next() & 0xffff));
        sp.ty = AssetType(TYPES[rng.below(8) as usize]);
        sp.inv_type = InventoryType(INV_TYPES[rng.below(6) as usize]);
        sp.name = NAMES[rng.below(9) as usize].to_vec();
        sp.base = rng.next() as u32;
        sp.owner_mask = rng.next() as u32;
        sp.group_mask = rng.next() as u32;
        sp.everyone = rng.next() as u32;
        sp.next = rng.next() as u32;
        sp.owner = if rng.coin() { agent() } else { other() };
        sp.group = if rng.coin() { NULL_ID } else { other_group() };
        sp.flags = rng.next() as u32;
        sp.sale_type = SaleType(rng.below(4) as i32);
        sp.sale_price = rng.below(1000) as i32;
        sp.date = (rng.next() & 0xffff_ffff) as i64;
        let mut it = item_llsd(&make_item(&sp));
        if rng.below(4) == 0 {
            let c = rng.coin();
            insert(&mut it, "running", Value::Boolean(c));
        }
        if rng.below(4) == 0 {
            let c = rng.coin();
            insert(&mut it, "faulted", Value::Boolean(c));
        }
        if rng.below(8) == 0 {
            if let Value::Map(m) = &mut it {
                m.remove(b"permissions".as_slice());
            }
        }
        if rng.below(8) == 0 {
            if let Value::Map(m) = &mut it {
                m.remove(b"sale_info".as_slice());
            }
        }
        arr.push(it);
    }
    insert(&mut body, "contents", Value::Array(arr));
    body
}

#[test]
fn set2_random_sweep() {
    let mut rng = Rng::new(SEED_REPLIES);
    let mut digest = DigestBuilder::new("cap_replies_random");
    let mut samples = String::new();
    let mut kept = 0;
    for i in 0..REPLY_SWEEP_COUNT {
        let label = format!("random_{i:04}");
        let body = random_reply(&mut rng);
        let expected = if rng.below(5) == 0 { (rng.below(70000) as i32 - 35000) as i16 } else { 0 };
        let input = format!("replies_random/{label}.llsd");
        let result = run_reply_case(&label, Some(&body), &input, expected);
        digest.add(result.as_bytes());
        if i as u64 % SAMPLE_EVERY == 0 {
            assert_bytes(&input, &binary_llsd(&body));
            samples += &result;
            kept += 1;
        }
    }
    assert_eq!(std::fs::read_dir(fixture_path("replies_random")).unwrap().count(), kept);
    assert_text("cap_replies_random_samples.txt", samples.as_bytes());
    assert_lines("digests_set2.txt", &digest_lines(vec![digest.finish()]));
}
