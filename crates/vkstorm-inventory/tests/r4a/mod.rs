//! The replay harness of the task fixtures (`fixtures/task/README.md`): the
//! crate's decisions ([`vkstorm_inventory::task`]) driven as
//! `INTEGRATION_TEST_llinventory_task_oracle` drives the frozen C++, with
//! the same fakes, and the recorder's text.
//!
//! What is the crate and what is the harness:
//! - every decision and lookup the fixtures record comes from the crate;
//! - [`Scene`] is the C++ side: the `LLViewerObject` state the actions are
//!   applied to (its executor, [`Scene::run`], is the reference for the
//!   C++ one), the listeners, `gObjectList`, the region, the selection
//!   manager and `permYouOwner`, the coroutines (a queue of suspended HTTP
//!   requests and timers, continued as `launch::dispatch` runs them), the
//!   fake xfer manager and a model of the real `LLXferManager`'s request
//!   and abort paths, the cache directory (in memory), and the message
//!   system (the `vkstorm-messages` builder, reader and zero-coding, and a
//!   capture of the datagrams in place of the loopback receiver);
//! - the text is the recorder's (`llinventory_task_oracle_test.cpp`).
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use vkstorm_inventory::item::Item;
use vkstorm_inventory::llsd::coerce::{as_integer, as_string, as_uuid, get, insert};
use vkstorm_inventory::message::{add_string, get_string};
use vkstorm_inventory::object::Object;
use vkstorm_inventory::perms::{
    NULL_ID, PERM_ALL, PERM_COPY, PERM_MOVE, PERM_NONE, PERM_TRANSFER, Permissions, Uuid,
};
use vkstorm_inventory::sale::{SaleInfo, SaleType};
use vkstorm_inventory::task::{
    self, Action, Agent, CreateRequest, Entry, Env, FloaterKey, LocalizedNames, Region,
    RequestState, Selection, TASK_INVENTORY_ASSET_KEY, TASK_INVENTORY_ITEM_KEY, TaskState,
    TaskView, ViewerItem, lookup,
};
use vkstorm_inventory::types::{AssetType, InventoryType};
use vkstorm_llsd::{Format, Options, Value};
use vkstorm_messages::{
    Builder, Encoding, MAX_BUFFER_SIZE, Reader, Template, Tokenizer, VarType, parse_template,
};

use crate::common::Sha256;

// ---------------------------------------------------------------------------
// Constants of the recording
// ---------------------------------------------------------------------------

pub const SEED_FILES: u64 = 0x5234_4154;
pub const SEED_REPLIES: u64 = 0x5234_4152;
pub const FILE_SWEEP_COUNT: usize = 3000;
pub const REPLY_SWEEP_COUNT: usize = 2000;
pub const SAMPLE_EVERY: u64 = 97;

pub fn uid(s: &str) -> Uuid {
    vkstorm_llsd::text::uuid_set(s.as_bytes())
}

pub fn agent() -> Uuid {
    uid("a0000000-0000-4000-8000-000000000001")
}
pub fn session() -> Uuid {
    uid("a0000000-0000-4000-8000-000000000002")
}
pub fn agent_group() -> Uuid {
    uid("a0000000-0000-4000-8000-000000000003")
}
pub fn other() -> Uuid {
    uid("a0000000-0000-4000-8000-000000000004")
}
pub fn other_group() -> Uuid {
    uid("a0000000-0000-4000-8000-000000000005")
}
pub fn creator() -> Uuid {
    uid("a0000000-0000-4000-8000-000000000006")
}
pub fn obj_id() -> Uuid {
    uid("0b000000-0000-4000-8000-0000000000aa")
}
pub fn obj2_id() -> Uuid {
    uid("0b000000-0000-4000-8000-0000000000bb")
}
pub fn item(c: char) -> Uuid {
    uid(&format!("1a000000-0000-4000-8000-00000000000{c}"))
}
pub fn asset(c: char) -> Uuid {
    uid(&format!("a5000000-0000-4000-8000-00000000000{c}"))
}
pub fn asset_pending() -> Uuid {
    uid("a5000000-0000-4000-8000-0000000000ff")
}
pub fn thumb() -> Uuid {
    uid("7b000000-0000-4000-8000-000000000001")
}
pub fn txn() -> Uuid {
    uid("7a000000-0000-4000-8000-000000000001")
}
pub fn folder() -> Uuid {
    uid("f0000000-0000-4000-8000-000000000001")
}
pub fn template_item() -> Uuid {
    uid("7e000000-0000-4000-8000-000000000001")
}

pub const CAP_RTI: &str = "https://sim.example/cap/rti";
pub const CAP_CREATE: &str = "https://sim.example/cap/ctii";
pub const LOCAL_ID: u32 = 4321;

fn aliases() -> &'static HashMap<Uuid, &'static str> {
    static A: OnceLock<HashMap<Uuid, &'static str>> = OnceLock::new();
    A.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert(agent(), "agent");
        m.insert(session(), "session");
        m.insert(agent_group(), "agentgroup");
        m.insert(other(), "other");
        m.insert(other_group(), "othergroup");
        m.insert(creator(), "creator");
        m.insert(obj_id(), "obj");
        m.insert(obj2_id(), "obj2");
        for (c, n) in [('a', "A"), ('b', "B"), ('c', "C"), ('d', "D"), ('e', "E")] {
            m.insert(item(c), n);
        }
        for (c, n) in [
            ('a', "assetA"),
            ('b', "assetB"),
            ('c', "assetC"),
            ('d', "assetD"),
            ('e', "assetE"),
        ] {
            m.insert(asset(c), n);
        }
        m.insert(asset_pending(), "assetP");
        m.insert(thumb(), "thumb");
        m.insert(txn(), "txn");
        m.insert(folder(), "folder");
        m.insert(template_item(), "template");
        m
    })
}

/// `LLUUID::asString`.
pub fn uuid_str(id: &Uuid) -> String {
    let mut out = Vec::new();
    vkstorm_llsd::text::uuid_to_hex(id, &mut out);
    String::from_utf8(out).unwrap()
}

/// `uuid_name`: the alias, `null`, or the id.
pub fn nm(id: &Uuid) -> String {
    if let Some(a) = aliases().get(id) {
        return (*a).to_owned();
    }
    if *id == NULL_ID {
        "null".to_owned()
    } else {
        uuid_str(id)
    }
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

/// The recorder's `esc`: quoted; `"` and `\` escaped; other bytes outside
/// printable ASCII as `\xHH`.
pub fn esc(s: &[u8]) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for &c in s {
        if c == b'"' || c == b'\\' {
            out.push('\\');
            out.push(c as char);
        } else if (0x20..0x7f).contains(&c) {
            out.push(c as char);
        } else {
            let _ = write!(out, "\\x{c:02x}");
        }
    }
    out.push('"');
    out
}

/// The fakes' `printable`: printable ASCII except `\` as is, the rest `\xHH`.
pub fn printable(s: &[u8]) -> String {
    let mut out = String::new();
    for &c in s {
        if (0x20..0x7f).contains(&c) && c != b'\\' {
            out.push(c as char);
        } else {
            let _ = write!(out, "\\x{c:02x}");
        }
    }
    out
}

pub fn hex(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub fn sha256(data: &[u8]) -> String {
    let mut h = Sha256::default();
    h.update(data);
    h.hex()
}

/// `notation`: one line, binary as `b16"..."`.
pub fn notation(v: &Value) -> String {
    String::from_utf8_lossy(&vkstorm_llsd::format(Format::Notation, v, Options::default()).0)
        .into_owned()
}

/// The notation as bytes (it can hold any byte of a string).
pub fn notation_bytes(v: &Value) -> Vec<u8> {
    vkstorm_llsd::format(Format::Notation, v, Options::default()).0
}

pub fn binary_llsd(v: &Value) -> Vec<u8> {
    vkstorm_llsd::format(Format::Binary, v, Options::default()).0
}

pub fn xml_llsd(v: &Value) -> Vec<u8> {
    vkstorm_llsd::format(Format::Xml, v, Options::default()).0
}

/// `%g` of a float (`fmt_g` of the fakes).
pub fn fmt_g(v: f64) -> String {
    let mut out = Vec::new();
    vkstorm_llsd::text::fmt_g(v, 6, &mut out);
    String::from_utf8(out).unwrap()
}

// ---------------------------------------------------------------------------
// The sweeps' generator and digests
// ---------------------------------------------------------------------------

/// xorshift64* as the task README states it (`coin` is the top bit,
/// `below(0)` is 0).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    pub fn next(&mut self) -> u64 {
        let mut s = self.0;
        s ^= s >> 12;
        s ^= s << 25;
        s ^= s >> 27;
        self.0 = s;
        s.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            (self.next() % u64::from(n)) as u32
        }
    }

    pub fn coin(&mut self) -> bool {
        self.next() >> 63 != 0
    }
}

/// `DigestBuilder`: SHA-256 of a sweep's units, whole and per block of 97.
pub struct DigestBuilder {
    name: String,
    total: Sha256,
    chunk: Sha256,
    count: u64,
    chunks: Vec<String>,
}

impl DigestBuilder {
    pub fn new(name: &str) -> DigestBuilder {
        DigestBuilder {
            name: name.to_owned(),
            total: Sha256::default(),
            chunk: Sha256::default(),
            count: 0,
            chunks: Vec::new(),
        }
    }

    pub fn add(&mut self, unit: &[u8]) {
        self.total.update(unit);
        self.chunk.update(unit);
        self.count += 1;
        if self.count % SAMPLE_EVERY == 0 {
            self.flush();
        }
    }

    fn flush(&mut self) {
        let k = (self.count - 1) / SAMPLE_EVERY;
        let chunk = std::mem::take(&mut self.chunk);
        self.chunks.push(format!(
            "{} chunk {} units {}-{} {}",
            self.name,
            k,
            k * SAMPLE_EVERY,
            self.count - 1,
            chunk.hex()
        ));
    }

    /// The `<name> <count> <sha>` line and the chunk lines.
    pub fn finish(mut self) -> (String, Vec<String>) {
        if self.count % SAMPLE_EVERY != 0 {
            self.flush();
        }
        let total = std::mem::take(&mut self.total);
        (
            format!("{} {} {}", self.name, self.count, total.hex()),
            self.chunks,
        )
    }
}

/// The digest file's content lines for these sweeps.
pub fn digest_lines(digests: Vec<(String, Vec<String>)>) -> Vec<String> {
    let mut out: Vec<String> = digests.iter().map(|d| d.0.clone()).collect();
    for d in digests {
        out.extend(d.1);
    }
    out
}

// ---------------------------------------------------------------------------
// Fixtures and the comparison
// ---------------------------------------------------------------------------

pub fn fixture_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/task")
        .join(rel)
}

pub fn read_bytes(rel: &str) -> Vec<u8> {
    std::fs::read(fixture_path(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The replay's lines of a text: every line, a CR before the newline
/// removed, `#` lines skipped.
pub fn content_lines(text: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for mut line in text.split(|&b| b == b'\n') {
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        if line.first() != Some(&b'#') {
            out.push(line.to_vec());
        }
    }
    // the text ends in a newline: the empty piece after it is not a line
    if text.last() == Some(&b'\n') {
        out.pop();
    }
    out
}

/// Compares the produced text with the fixture's content lines; panics with
/// the first differences (file, content line, the case or trace, expected
/// and actual).
pub fn assert_text(fixture: &str, produced: &[u8]) {
    let want = content_lines(&read_bytes(fixture));
    let got = content_lines(produced);
    assert!(!want.is_empty(), "{fixture} has no content lines");
    let mut report = String::new();
    let mut shown = 0;
    let mut last_case = String::new();
    let n = want.len().max(got.len());
    for i in 0..n {
        if shown >= 5 {
            break;
        }
        let w = want
            .get(i)
            .map_or_else(|| b"<end of fixture>".to_vec(), Clone::clone);
        let g = got
            .get(i)
            .map_or_else(|| b"<end of output>".to_vec(), Clone::clone);
        if i < want.len()
            && (w.starts_with(b"case ") || w.starts_with(b"trace ") || w.starts_with(b"scrub "))
        {
            last_case = String::from_utf8_lossy(&w).into_owned();
        }
        if w != g {
            let _ = writeln!(
                report,
                "difference in {fixture} at content line {} (in: {last_case})\n  expected: {}\n  actual:   {}",
                i + 1,
                String::from_utf8_lossy(&w),
                String::from_utf8_lossy(&g)
            );
            shown += 1;
        }
    }
    assert!(report.is_empty(), "{report}");
}

/// Compares produced lines (no `#` lines) with a fixture's content lines.
pub fn assert_lines(fixture: &str, produced: &[String]) {
    let mut text = Vec::new();
    for l in produced {
        text.extend_from_slice(l.as_bytes());
        text.push(b'\n');
    }
    assert_text(fixture, &text);
}

pub fn assert_bytes(fixture: &str, produced: &[u8]) {
    let want = read_bytes(fixture);
    assert!(
        want == produced,
        "input bytes differ: {fixture} ({} vs {} bytes)",
        want.len(),
        produced.len()
    );
}

// ---------------------------------------------------------------------------
// Items (ItemSpec, make_item, the standard contents)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ItemSpec {
    pub id: Uuid,
    pub parent: Uuid,
    pub creator: Uuid,
    pub owner: Uuid,
    pub last_owner: Uuid,
    pub group: Uuid,
    pub group_owned: bool,
    pub base: u32,
    pub owner_mask: u32,
    pub group_mask: u32,
    pub everyone: u32,
    pub next: u32,
    pub asset: Uuid,
    pub ty: AssetType,
    pub inv_type: InventoryType,
    pub name: Vec<u8>,
    pub desc: Vec<u8>,
    pub sale_type: SaleType,
    pub sale_price: i32,
    pub flags: u32,
    pub date: i64,
    pub txn: Uuid,
}

impl Default for ItemSpec {
    fn default() -> Self {
        ItemSpec {
            id: NULL_ID,
            parent: obj_id(),
            creator: creator(),
            owner: agent(),
            last_owner: NULL_ID,
            group: NULL_ID,
            group_owned: false,
            base: PERM_ALL,
            owner_mask: PERM_ALL,
            group_mask: PERM_NONE,
            everyone: PERM_NONE,
            next: PERM_MOVE | PERM_TRANSFER,
            asset: NULL_ID,
            ty: AssetType::NOTECARD,
            inv_type: InventoryType::NOTECARD,
            name: b"Item".to_vec(),
            desc: Vec::new(),
            sale_type: SaleType::NOT,
            sale_price: 10,
            flags: 0,
            date: 1_700_000_000,
            txn: NULL_ID,
        }
    }
}

pub fn make_perm(s: &ItemSpec) -> Permissions {
    let mut p = Permissions::DEFAULT;
    p.init(&s.creator, &s.owner, &s.last_owner, &s.group);
    if s.group_owned {
        p.set_owner_and_group(&NULL_ID, &NULL_ID, &s.group, true);
    }
    p.init_masks(s.base, s.owner_mask, s.everyone, s.group_mask, s.next);
    p
}

pub fn make_item(s: &ItemSpec) -> ViewerItem {
    let mut v = ViewerItem::new(
        &s.id,
        &s.parent,
        &make_perm(s),
        &s.asset,
        s.ty,
        s.inv_type,
        &s.name,
        &s.desc,
        &SaleInfo::new(s.sale_type, s.sale_price),
        s.flags,
        s.date,
    );
    if s.txn != NULL_ID {
        v.transaction = s.txn;
    }
    v
}

pub fn spec_a() -> ItemSpec {
    ItemSpec {
        id: item('a'),
        asset: asset('a'),
        name: b"Notecard A".to_vec(),
        desc: b"copyable".to_vec(),
        ..ItemSpec::default()
    }
}

pub fn spec_b() -> ItemSpec {
    ItemSpec {
        id: item('b'),
        asset: asset('b'),
        ty: AssetType::OBJECT,
        inv_type: InventoryType::OBJECT,
        base: PERM_ALL & !PERM_COPY,
        owner_mask: PERM_ALL & !PERM_COPY,
        next: PERM_MOVE,
        name: b"Object B".to_vec(),
        desc: b"no copy".to_vec(),
        ..ItemSpec::default()
    }
}

pub fn legacy_object(id: &Uuid, name: &[u8], ty: AssetType, parent: &Uuid) -> Vec<u8> {
    let o = Object::new(id, parent, ty, name);
    let mut out = Vec::new();
    o.export_legacy_stream(&mut out);
    out
}

pub fn legacy_item(s: &ItemSpec) -> Vec<u8> {
    let mut out = Vec::new();
    make_item(s).item.export_legacy_stream(&mut out, true);
    out
}

/// Contents (obj), A, B.
pub fn standard_file() -> &'static [u8] {
    static F: OnceLock<Vec<u8>> = OnceLock::new();
    F.get_or_init(|| {
        let mut f = legacy_object(&obj_id(), b"Contents", AssetType::CATEGORY, &NULL_ID);
        f.extend(legacy_item(&spec_a()));
        f.extend(legacy_item(&spec_b()));
        f
    })
}

pub fn item_llsd(v: &ViewerItem) -> Value {
    v.item.to_llsd()
}

pub fn standard_reply(serial: i32) -> Value {
    static C: OnceLock<Value> = OnceLock::new();
    let contents = C.get_or_init(|| {
        Value::Array(vec![
            item_llsd(&make_item(&spec_a())),
            item_llsd(&make_item(&spec_b())),
        ])
    });
    let mut body = Value::map();
    insert(&mut body, "inventory_serial", Value::Integer(serial));
    insert(&mut body, "contents", contents.clone());
    body
}

// ---------------------------------------------------------------------------
// The message system
// ---------------------------------------------------------------------------

pub struct Messages {
    pub by_name: HashMap<String, Arc<Template>>,
    pub by_number: HashMap<u32, Arc<Template>>,
}

pub fn messages() -> &'static Messages {
    static M: OnceLock<Messages> = OnceLock::new();
    M.get_or_init(|| {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../vkstorm-messages/tests/fixtures/message_template.msg"
        );
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let file = parse_template(&mut Tokenizer::new(&text)).expect("message template parses");
        let mut by_name = HashMap::new();
        let mut by_number = HashMap::new();
        for t in file.messages {
            let t = Arc::new(t.with_index_keys());
            by_number.insert(t.number, t.clone());
            by_name.insert(t.name.clone(), t);
        }
        Messages { by_name, by_number }
    })
}

pub fn template(name: &str) -> Arc<Template> {
    messages()
        .by_name
        .get(name)
        .unwrap_or_else(|| panic!("no message {name}"))
        .clone()
}

fn key_of(t: &Template, block: &str, var: &str) -> u64 {
    t.block_by_name(block)
        .and_then(|b| b.variable_by_name(var))
        .unwrap_or_else(|| panic!("{block}.{var}"))
        .key
}

fn block_key(t: &Template, block: &str) -> u64 {
    t.block_by_name(block)
        .unwrap_or_else(|| panic!("{block}"))
        .key
}

/// `LL_RELIABLE_FLAG`.
const RELIABLE_FLAG: u8 = 0x40;

/// The datagram of a built message as `LLMessageSystem::sendMessage` sends
/// it: the flags byte (reliable), zero-coded when the template is and when
/// coding makes it smaller (then the zero-code flag). The packet id is left
/// zero (the recording zeroes it).
pub fn datagram(builder: &mut Builder, reliable: bool) -> Vec<u8> {
    let zerocoded = builder
        .template()
        .is_some_and(|t| t.encoding == Encoding::ZeroCoded);
    let mut buf = vec![0u8; MAX_BUFFER_SIZE];
    let n = builder.build_message(&mut buf, 0).expect("message builds");
    builder.clear_message();
    buf.truncate(n);
    if reliable {
        buf[0] |= RELIABLE_FLAG;
    }
    if zerocoded {
        let mut out = vec![0u8; buf.len() * 2 + 2];
        if let Some(len) = vkstorm_messages::zerocode::zero_code(&buf, &mut out) {
            out.truncate(len);
            return out;
        }
    }
    buf
}

/// `ReplyTaskInventory` built and read back: what `processTaskInv` reads
/// (`TaskID`, `Serial`, `Filename`).
pub fn reply_round_trip(task: &Uuid, serial: i16, filename: &[u8]) -> (Uuid, i16, Vec<u8>) {
    let t = template("ReplyTaskInventory");
    let mut b = Builder::new();
    b.new_message(t.clone());
    b.next_block(block_key(&t, "InventoryData")).unwrap();
    b.add_data(key_of(&t, "InventoryData", "TaskID"), task, VarType::LLUUID)
        .unwrap();
    b.add_data(
        key_of(&t, "InventoryData", "Serial"),
        &serial.to_le_bytes(),
        VarType::S16,
    )
    .unwrap();
    add_string(&mut b, key_of(&t, "InventoryData", "Filename"), filename).unwrap();
    let mut buf = vec![0u8; MAX_BUFFER_SIZE];
    let n = b.build_message(&mut buf, 0).unwrap();
    let mut r = Reader::new();
    r.set_message(t.clone(), n as i32);
    r.decode_data(&buf[..n]);
    let blk = block_key(&t, "InventoryData");
    let task_id: [u8; 16] = r
        .get(blk, 0, key_of(&t, "InventoryData", "TaskID"), 16)
        .unwrap()
        .try_into()
        .unwrap();
    let s = r
        .get(blk, 0, key_of(&t, "InventoryData", "Serial"), 2)
        .unwrap();
    let serial = i16::from_le_bytes([s[0], s[1]]);
    let name = get_string(&r, blk, 0, key_of(&t, "InventoryData", "Filename")).unwrap();
    (task_id, serial, name)
}

/// `packet_text`: one captured datagram.
pub fn packet_text(wire_in: &[u8], xfer_ids: &HashMap<u64, String>) -> String {
    if wire_in.is_empty() {
        return "  packet <MARKER NOT RECEIVED WITHIN 30 s>\n".to_owned();
    }
    let mut wire = wire_in.to_vec();
    for b in wire.iter_mut().take(5).skip(1) {
        *b = 0;
    }
    let zerocoded = wire[0] & vkstorm_messages::ZERO_CODE_FLAG != 0;
    let mut body = if zerocoded {
        let mut out = vec![0u8; MAX_BUFFER_SIZE * 2];
        let e = vkstorm_messages::zerocode::zero_code_expand(&wire, &mut out);
        out.truncate(e.len);
        out
    } else {
        wire.clone()
    };
    let number = vkstorm_messages::reader::decode_number(&body).ok();
    let t = number.and_then(|n| messages().by_number.get(&n).cloned());
    let mut fields = String::new();
    let mut random_id = false;
    let name = t.as_ref().map_or("?".to_owned(), |t| t.name.clone());
    if let Some(t) = &t {
        let mut r = Reader::new();
        r.set_message(t.clone(), body.len() as i32);
        r.decode_data(&body);
        for b in &t.blocks {
            let count = r.number_of_blocks(b.key).unwrap_or(0);
            let _ = writeln!(fields, "    {} count={}", b.name, count);
            for rep in 0..count {
                for v in &b.variables {
                    let size = r.size(b.key, rep, v.key).unwrap_or(0);
                    let raw = if size > 0 {
                        r.get(b.key, rep, v.key, size).unwrap().to_vec()
                    } else {
                        Vec::new()
                    };
                    let mut shown = hex(&raw);
                    if (name == "RequestXfer" || name == "AbortXfer")
                        && v.name == "ID"
                        && raw.len() == 8
                    {
                        let id = u64::from_le_bytes(raw[..8].try_into().unwrap());
                        shown = format!(
                            "<xfer id {}>",
                            xfer_ids.get(&id).map_or("unknown", String::as_str)
                        );
                        if body.len() >= 18 && body[10..18] == raw[..] {
                            body[10..18].fill(0);
                            random_id = true;
                        }
                    } else if v.ty == VarType::Variable {
                        shown.push(' ');
                        shown.push_str(&esc(&raw));
                    }
                    let _ = writeln!(fields, "      [{}] {} = {}", rep, v.name, shown);
                }
            }
        }
    }
    let wirelen: i64 = if random_id && zerocoded {
        -1
    } else {
        wire.len() as i64
    };
    let mut out = format!(
        "  packet {} flags={:02x} zerocoded={} wirelen={}\n",
        name,
        wire[0],
        i32::from(zerocoded),
        wirelen
    );
    if random_id && zerocoded {
        out.push_str("    wire <zero-coded with the random xfer id: see body>\n");
    } else {
        if random_id {
            wire[10..18].fill(0);
        }
        let _ = writeln!(out, "    wire {}", hex(&wire));
    }
    let _ = writeln!(out, "    body {}", hex(&body));
    out + &fields
}

pub fn packets_text(packets: &[Vec<u8>], xfer_ids: &HashMap<u64, String>) -> String {
    let mut out = format!("  packets {}\n", packets.len());
    for p in packets {
        out.push_str(&packet_text(p, xfer_ids));
    }
    out
}

// ---------------------------------------------------------------------------
// The scene: the C++ side
// ---------------------------------------------------------------------------

/// The `LLFilenameAndTask` of a transfer.
#[derive(Clone, Debug)]
pub struct Ft {
    pub filename: Vec<u8>,
    pub serial: i16,
}

#[derive(Clone, Debug)]
pub struct FakeXfer {
    pub id: u64,
    pub local: Vec<u8>,
    pub ft: Ft,
    pub aborted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RealStatus {
    Pending,
    InProgress,
    Aborted,
}

#[derive(Clone, Debug)]
pub struct RealXfer {
    id: u64,
    remote: Vec<u8>,
    status: RealStatus,
}

pub enum Xfers {
    /// The fake of sets 1 (main) and 3: ids 1001, 1002, ...; an abort marks
    /// the transfer, which never completes.
    Fake { next: u64, list: Vec<FakeXfer> },
    /// The real `LLXferManager` (scrub corpus, set 4): its receive list, the
    /// ids in request order (`g_real_ids`), and the next id.
    Real {
        list: Vec<RealXfer>,
        ids: Vec<u64>,
        next: u64,
    },
}

/// How a create callback records (the trace's or set 4's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateCallback {
    Trace,
    Body,
}

enum HttpKind {
    Cap,
    Create { callback: CreateCallback },
}

struct PendingHttp {
    method: &'static str,
    url: String,
    body: Value,
    kind: HttpKind,
}

/// The C++ `LLViewerObject`'s task-inventory state.
pub struct Obj {
    pub id: Uuid,
    pub local_id: u32,
    pub st: TaskState,
    pub inv: Option<Vec<Entry>>,
    pub pending: Vec<Uuid>,
    /// `mInventoryCallbacks`, front to back: listener numbers.
    pub callbacks: VecDeque<u32>,
    pub you_owner: bool,
}

pub struct Scene {
    pub obj: Obj,
    pub alive: bool,
    pub caps: bool,
    pub cap_rti: bool,
    pub cap_create: bool,
    pub region: bool,
    pub selected: bool,
    /// The listeners the scene made, in order (`s.listeners`).
    pub listeners: Vec<u32>,
    pub rec: Vec<String>,
    http: VecDeque<PendingHttp>,
    timers: VecDeque<f64>,
    pub xfers: Xfers,
    /// The cache directory, by file name.
    pub cache: BTreeMap<Vec<u8>, Vec<u8>>,
    /// The cache files the scene wrote, in order.
    pub written: Vec<Vec<u8>>,
    /// The datagrams sent while capturing; `None` when not capturing.
    pub capture: Option<Vec<Vec<u8>>>,
    /// The reply of the create request being continued.
    create_result: Option<Value>,
    create_callback: CreateCallback,
}

pub fn names() -> &'static LocalizedNames {
    static N: OnceLock<LocalizedNames> = OnceLock::new();
    N.get_or_init(|| LocalizedNames::new(|desc| format!("{{L10N:{desc}}}").into_bytes()))
}

impl Scene {
    pub fn new(caps: bool, listener_count: u32, real_xfer: bool) -> Scene {
        let mut s = Scene {
            obj: Obj {
                id: obj_id(),
                local_id: LOCAL_ID,
                st: TaskState::default(),
                inv: None,
                pending: Vec::new(),
                callbacks: VecDeque::new(),
                you_owner: true,
            },
            alive: true,
            caps,
            cap_rti: caps,
            cap_create: caps,
            region: true,
            selected: false,
            listeners: Vec::new(),
            rec: Vec::new(),
            http: VecDeque::new(),
            timers: VecDeque::new(),
            xfers: if real_xfer {
                Xfers::Real {
                    list: Vec::new(),
                    ids: Vec::new(),
                    next: 0x5a17_0000_0000_0001,
                }
            } else {
                Xfers::Fake {
                    next: 1001,
                    list: Vec::new(),
                }
            },
            cache: BTreeMap::new(),
            written: Vec::new(),
            capture: None,
            create_result: None,
            create_callback: CreateCallback::Trace,
        };
        for i in 0..listener_count {
            s.listen_new(i + 1);
        }
        s
    }

    fn listen_new(&mut self, n: u32) {
        self.listeners.push(n);
        self.obj.callbacks.push_front(n);
    }

    pub fn view(&self) -> TaskView<'_> {
        TaskView {
            id: self.obj.id,
            local_id: self.obj.local_id,
            state: self.obj.st,
            inventory: self.obj.inv.as_deref(),
            pending: &self.obj.pending,
            has_listeners: !self.obj.callbacks.is_empty(),
        }
    }

    pub fn env(&self) -> Env<'static> {
        Env {
            agent: Agent {
                id: agent(),
                session: session(),
                group: agent_group(),
            },
            region: self.region.then_some(Region {
                request_task_inventory: if self.cap_rti { CAP_RTI } else { "" },
                create_task_inventory_item: if self.cap_create { CAP_CREATE } else { "" },
            }),
            you_owner: self.obj.you_owner,
            selection: self.selected.then_some(Selection {
                owner: other(),
                group: other_group(),
            }),
            in_open_sim: false,
            names: names(),
        }
    }

    pub fn add(&mut self, s: impl Into<String>) {
        self.rec.push(s.into());
    }

    /// `take_actions`.
    pub fn take(&mut self) -> String {
        if self.rec.is_empty() {
            return "-".to_owned();
        }
        let s = self.rec.join("; ");
        self.rec.clear();
        s
    }

    // --- files -------------------------------------------------------------

    pub fn write_file(&mut self, name: &[u8], bytes: &[u8]) {
        self.cache.insert(name.to_vec(), bytes.to_vec());
        self.written.push(name.to_vec());
    }

    pub fn exists(&self, name: &[u8]) -> bool {
        self.cache.contains_key(name)
    }

    /// `cache_text`.
    pub fn cache_text(&self) -> String {
        let mut names: Vec<&[u8]> = Vec::new();
        for f in &self.written {
            if self.exists(f) && !names.contains(&f.as_slice()) {
                names.push(f);
            }
        }
        let parts: Vec<String> = names
            .iter()
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .collect();
        format!("[{}]", parts.join(","))
    }

    // --- the executor ------------------------------------------------------

    /// Carries out the actions on the object, in order, as the C++
    /// executor does.
    pub fn run(&mut self, actions: Vec<Action>) {
        for a in actions {
            self.act(a);
        }
    }

    fn act(&mut self, a: Action) {
        match a {
            Action::SetSerial(v) => self.obj.st.serial = v,
            Action::SetExpected(v) => self.obj.st.expected = v,
            Action::SetRequestState(v) => self.obj.st.request = v,
            Action::SetXferId(v) => self.obj.st.xfer_id = v,
            Action::SetDirty(v) => self.obj.st.dirty = v,
            Action::DeleteInventory => self.obj.inv = None,
            Action::ResetInventory => self.obj.inv = Some(Vec::new()),
            Action::PushFront(e) => self.obj.inv.as_mut().expect("a list").insert(0, e),
            Action::PushFrontReplyContents => {
                unreachable!("cap_fetch_reply expands the reply's entries")
            }
            Action::Erase(i) => {
                self.obj.inv.as_mut().expect("a list").remove(i);
            }
            Action::PendingPush(id) => self.obj.pending.push(id),
            Action::PendingErase(i) => {
                self.obj.pending.remove(i);
            }
            Action::NotifyListeners => {
                let brief = inv_brief(self.obj.inv.as_deref());
                let serial = self.obj.st.serial;
                let lines: Vec<String> = self
                    .obj
                    .callbacks
                    .iter()
                    .map(|l| format!("callback L{l} ser={serial} inv={brief}"))
                    .collect();
                self.rec.extend(lines);
            }
            Action::Send(msg) => {
                self.add(format!("msg.send {}", msg.name()));
                if self.capture.is_some() {
                    let t = template(msg.name());
                    let mut b = Builder::new();
                    task::message::pack(&mut b, &t, &msg).expect("packs");
                    let d = datagram(&mut b, true);
                    self.capture.as_mut().unwrap().push(d);
                }
            }
            Action::LaunchCapFetch => {
                self.add("coro.launch LLViewerObject::fetchInventoryFromCapCoro()");
                if self.alive {
                    let acts = task::cap_fetch_begin(&self.view(), &self.env());
                    self.run(acts);
                }
            }
            Action::HttpGet { url } => {
                self.add(format!("http.get TaskInventoryRequest {url}"));
                self.http.push_back(PendingHttp {
                    method: "GET",
                    url,
                    body: Value::Undefined,
                    kind: HttpKind::Cap,
                });
            }
            Action::LaunchDelayedFetch { seconds } => {
                self.add("coro.launch LLViewerObject::fetchInventoryDelayedCoro()");
                let s = f64::from(seconds as f32);
                self.add(format!("timer.start {}", fmt_g(s)));
                self.timers.push_back(s);
            }
            Action::RequestXfer { filename, serial } => {
                let id = self.request_file(
                    &filename,
                    Ft {
                        filename: filename.clone(),
                        serial,
                    },
                );
                let acts = task::xfer_requested(&self.view(), &self.env(), id);
                self.run(acts);
            }
            Action::AbortXfer { id } => self.abort_xfer(id, -1),
            Action::RemoveCacheFile { filename } => {
                self.cache.remove(&filename);
            }
            Action::HideFloater { name, key } => {
                let k = match key {
                    FloaterKey::Id(id) => nm(&id),
                    FloaterKey::ItemAndObject { id, object } => {
                        format!("{{id={},object={}}}", nm(&id), nm(&object))
                    }
                };
                self.add(format!("floater.hide {name} {k}"));
            }
            Action::LaunchCreate { url, body } => {
                self.add("coro.launch LLViewerObject::createInventoryItemCoro");
                self.add(format!("http.post CreateTaskInventoryItem {url}"));
                let callback = self.create_callback;
                self.http.push_back(PendingHttp {
                    method: "POST",
                    url,
                    body,
                    kind: HttpKind::Create { callback },
                });
            }
            Action::CreateCallback { success } => {
                let result = self.create_result.clone().unwrap_or_else(Value::map);
                self.create_callback_record(success, &result);
            }
            Action::CreateFailed { message } => {
                let mut result = Value::map();
                insert(
                    &mut result,
                    "message",
                    Value::String(message.as_bytes().to_vec()),
                );
                self.create_callback_record(false, &result);
            }
        }
    }

    fn create_callback_record(&mut self, success: bool, result: &Value) {
        match self.create_callback {
            CreateCallback::Trace => {
                let m = as_string(get(result, "message"));
                self.add(format!(
                    "create.callback success={} message={}",
                    i32::from(success),
                    esc(&m)
                ));
            }
            CreateCallback::Body => {
                let mut shown = result.clone();
                if let Value::Map(m) = &mut shown {
                    m.remove(b"http_result".as_slice());
                }
                let http_type = as_integer(get(get(result, "http_result"), "type"));
                self.add(format!(
                    "create.callback success={} result={} http_type={}",
                    i32::from(success),
                    notation(&shown),
                    http_type
                ));
            }
        }
    }

    // --- the xfer manager ----------------------------------------------------

    fn request_file(&mut self, filename: &[u8], ft: Ft) -> u64 {
        let mut local = b"<cache>/".to_vec();
        local.extend_from_slice(filename);
        let args = format!(
            " local={} remote={} path=4 delete=1 priority=1 big=0",
            printable(&local),
            printable(filename)
        );
        match &mut self.xfers {
            Xfers::Fake { next, list } => {
                let id = *next;
                *next += 1;
                list.push(FakeXfer {
                    id,
                    local: filename.to_vec(),
                    ft,
                    aborted: false,
                });
                self.rec.push(format!("xfer.request id={id}{args}"));
                id
            }
            Xfers::Real { .. } => {
                self.rec.push(format!("xfer.request{args}"));
                let id = self.real_request_file(filename);
                let text = self.xfer_id_text(id);
                self.rec.push(format!("xfer.requested id={text}"));
                id
            }
        }
    }

    /// `LLXferManager::requestFile` (never a duplicate: each request has
    /// its own `LLFilenameAndTask`), then `startPendingDownloads` sends
    /// `RequestXfer`.
    fn real_request_file(&mut self, remote: &[u8]) -> u64 {
        let Xfers::Real { list, ids, next } = &mut self.xfers else {
            unreachable!()
        };
        let id = *next;
        *next = next.wrapping_add(0x0101_0101_0101_0101);
        list.push(RealXfer {
            id,
            remote: remote.to_vec(),
            status: RealStatus::Pending,
        });
        ids.push(id);
        if remote.len() >= 4 && remote.ends_with(b".tmp") {
            self.cache.remove(remote);
        }
        self.start_pending_downloads();
        id
    }

    fn start_pending_downloads(&mut self) {
        let Xfers::Real { list, .. } = &mut self.xfers else {
            return;
        };
        let mut started = Vec::new();
        for x in list.iter_mut().rev() {
            if x.status == RealStatus::Pending {
                x.status = RealStatus::InProgress;
                started.push((x.id, x.remote.clone()));
            }
        }
        for (id, remote) in started {
            let t = template("RequestXfer");
            let mut b = Builder::new();
            b.new_message(t.clone());
            b.next_block(block_key(&t, "XferID")).unwrap();
            b.add_data(key_of(&t, "XferID", "ID"), &id.to_le_bytes(), VarType::U64)
                .unwrap();
            add_string(&mut b, key_of(&t, "XferID", "Filename"), &remote).unwrap();
            b.add_data(key_of(&t, "XferID", "FilePath"), &[4], VarType::U8)
                .unwrap();
            b.add_data(
                key_of(&t, "XferID", "DeleteOnCompletion"),
                &[1],
                VarType::Bool,
            )
            .unwrap();
            b.add_data(key_of(&t, "XferID", "UseBigPackets"), &[0], VarType::Bool)
                .unwrap();
            b.add_data(key_of(&t, "XferID", "VFileID"), &NULL_ID, VarType::LLUUID)
                .unwrap();
            b.add_data(
                key_of(&t, "XferID", "VFileType"),
                &(-1i16).to_le_bytes(),
                VarType::S16,
            )
            .unwrap();
            let d = datagram(&mut b, true);
            if let Some(c) = self.capture.as_mut() {
                c.push(d);
            }
        }
    }

    fn abort_xfer(&mut self, id: u64, code: i32) {
        let text = self.xfer_id_text(id);
        self.rec.push(format!("xfer.abort id={text} code={code}"));
        match &mut self.xfers {
            Xfers::Fake { list, .. } => {
                for x in list.iter_mut().filter(|x| x.id == id) {
                    x.aborted = true;
                }
            }
            Xfers::Real { list, .. } => {
                let Some(i) = list.iter().position(|x| x.id == id) else {
                    return;
                };
                if list[i].status == RealStatus::InProgress {
                    list[i].status = RealStatus::Aborted;
                    let t = template("AbortXfer");
                    let mut b = Builder::new();
                    b.new_message(t.clone());
                    b.next_block(block_key(&t, "XferID")).unwrap();
                    b.add_data(key_of(&t, "XferID", "ID"), &id.to_le_bytes(), VarType::U64)
                        .unwrap();
                    b.add_data(
                        key_of(&t, "XferID", "Result"),
                        &code.to_le_bytes(),
                        VarType::S32,
                    )
                    .unwrap();
                    let d = datagram(&mut b, false);
                    if let Some(c) = self.capture.as_mut() {
                        c.push(d);
                    }
                } else {
                    list.remove(i);
                }
                self.start_pending_downloads();
            }
        }
    }

    /// `xfer_id_text`.
    pub fn xfer_id_text(&self, id: u64) -> String {
        match &self.xfers {
            Xfers::Fake { .. } => id.to_string(),
            Xfers::Real { ids, .. } => {
                if id == 0 {
                    return "0".to_owned();
                }
                match ids.iter().position(|&x| x == id) {
                    Some(i) => format!("real#{}", i + 1),
                    None => "real#unknown".to_owned(),
                }
            }
        }
    }

    /// The real manager's receive list as `real#k` names (the packets').
    pub fn real_ids(&self) -> HashMap<u64, String> {
        match &self.xfers {
            Xfers::Real { list, .. } => list
                .iter()
                .enumerate()
                .map(|(i, x)| (x.id, format!("real#{}", i + 1)))
                .collect(),
            Xfers::Fake { .. } => HashMap::new(),
        }
    }

    /// The oldest (or newest) fake transfer neither completed nor aborted.
    pub fn live_xfer(&self, newest: bool) -> Option<usize> {
        let Xfers::Fake { list, .. } = &self.xfers else {
            return None;
        };
        let mut found = None;
        for (i, x) in list.iter().enumerate() {
            if !x.aborted {
                found = Some(i);
                if !newest {
                    break;
                }
            }
        }
        found
    }

    pub fn take_xfer(&mut self, i: usize) -> FakeXfer {
        let Xfers::Fake { list, .. } = &mut self.xfers else {
            unreachable!()
        };
        list.remove(i)
    }

    /// The xfer callback, `processTaskInvFile(ft, code)`.
    pub fn xfer_callback(&mut self, ft: &Ft, code: i32) {
        if !self.alive {
            return;
        }
        let file = self.cache.get(&ft.filename).cloned();
        let acts = task::xfer_completed(
            &self.view(),
            &self.env(),
            ft.serial,
            &ft.filename,
            code,
            file.as_deref(),
        );
        self.run(acts);
    }

    /// `complete_xfer` of the event language.
    pub fn complete_xfer(&mut self, how: &str, newest: bool) {
        let Some(i) = self.live_xfer(newest) else {
            self.add("xfer.complete none-live");
            return;
        };
        let x = self.take_xfer(i);
        let mut code = 0;
        match how {
            "ok" => self.write_file(&x.local, standard_file()),
            "nofile" => {
                self.cache.remove(&x.local);
            }
            _ => code = -1,
        }
        self.add(format!("xfer.complete id={} code={}", x.id, code));
        self.xfer_callback(&x.ft, code);
    }

    // --- messages in -------------------------------------------------------

    /// `deliver_reply`: a `ReplyTaskInventory` through the builder and
    /// reader, to `processTaskInv`.
    pub fn deliver_reply(&mut self, serial: i16, filename: &[u8]) {
        let (task_id, serial, name) = reply_round_trip(&self.obj.id, serial, filename);
        if task_id != self.obj.id || !self.alive {
            return;
        }
        let acts = task::reply_task_inventory(&self.view(), &self.env(), serial, &name);
        self.run(acts);
    }

    // --- coroutines --------------------------------------------------------

    pub fn pending_http_count(&self) -> usize {
        self.http.len()
    }

    pub fn pending_http(&self, i: usize) -> (&'static str, &str, &Value) {
        let p = &self.http[i];
        (p.method, &p.url, &p.body)
    }

    /// `complete_http`: the first pending request (of the method, when
    /// given) answered with `body` (`None` for an undefined `LLSD`) and
    /// `status`, as `HttpCoroLLSDHandler` hands it on.
    pub fn complete_http(&mut self, body: Option<Value>, status: i32, method: Option<&str>) {
        let index = match method {
            Some(m) => self.http.iter().position(|p| p.method == m),
            None => (!self.http.is_empty()).then_some(0),
        };
        let Some(index) = index else {
            self.add(format!(
                "http.complete none-pending{}",
                method.map_or(String::new(), |m| format!(" {m}"))
            ));
            return;
        };
        let p = self.http.remove(index).unwrap();
        self.add(format!("http.complete {} Http_{}", p.method, status));
        let ok = (200..300).contains(&status);
        let body = body.unwrap_or(Value::Undefined);
        let mut result = if !ok {
            let mut r = Value::map();
            if (400..500).contains(&status) && !matches!(body, Value::Undefined) {
                if matches!(body, Value::Map(_)) {
                    r = body;
                } else {
                    insert(&mut r, "content", body);
                }
            }
            r
        } else {
            match body {
                Value::Undefined => Value::map(),
                Value::Map(_) => body,
                other => {
                    let mut r = Value::map();
                    insert(&mut r, "content", other);
                    r
                }
            }
        };
        let mut http = Value::map();
        insert(&mut http, "success", Value::Boolean(ok));
        insert(&mut http, "type", Value::Integer(status));
        insert(&mut http, "status", Value::Integer(status));
        insert(&mut http, "url", Value::String(p.url.clone().into_bytes()));
        insert(&mut http, "headers", Value::map());
        insert(&mut result, "http_result", http);
        match p.kind {
            HttpKind::Cap => {
                if self.alive {
                    let acts =
                        task::cap_fetch_reply(&self.view(), &self.env(), ok, status, &result);
                    self.run(acts);
                }
            }
            HttpKind::Create { callback } => {
                let success = task::create_item_succeeded(ok, &result);
                let alive = self.alive && as_uuid(get(&p.body, "object_id")) == self.obj.id;
                let acts = {
                    let view = self.view();
                    task::create_item_completed(alive.then_some(&view), &self.env(), success)
                };
                let saved = (self.create_result.take(), self.create_callback);
                self.create_result = Some(result);
                self.create_callback = callback;
                self.run(acts);
                self.create_result = saved.0;
                self.create_callback = saved.1;
            }
        }
    }

    /// `fire_timer`.
    pub fn fire_timer(&mut self) {
        let Some(t) = self.timers.pop_front() else {
            self.add("timer.fire none-pending");
            return;
        };
        self.add(format!("timer.fire {}", fmt_g(t)));
        if self.alive {
            let acts = task::delayed_fetch_due(&self.view(), &self.env());
            self.run(acts);
        }
    }

    // --- calls through the boundary -----------------------------------------

    pub fn request_inventory(&mut self) {
        let acts = task::request_inventory(&self.view(), &self.env());
        self.run(acts);
    }

    pub fn dirty_inventory(&mut self) {
        let acts = task::dirty_inventory(&self.view(), &self.env());
        self.run(acts);
    }

    pub fn update_inventory(&mut self, item: &ViewerItem, key: u8, is_new: bool) {
        let acts = task::update_inventory(&self.view(), &self.env(), item, key, is_new);
        self.run(acts);
    }

    pub fn update_item(&mut self, spec: &ItemSpec, key: u8, is_new: bool) {
        self.update_inventory(&make_item(spec), key, is_new);
    }

    pub fn remove_inventory(&mut self, id: &Uuid) {
        let acts = task::remove_inventory(&self.view(), &self.env(), id);
        self.run(acts);
    }

    pub fn move_inventory(&mut self, folder: &Uuid, id: &Uuid) {
        let acts = task::move_inventory(&self.view(), &self.env(), folder, id);
        self.run(acts);
    }

    pub fn delete_inventory_item(&mut self, id: &Uuid) {
        let acts = task::delete_inventory_item(&self.view(), &self.env(), id);
        self.run(acts);
    }

    pub fn load_task_inv_file(&mut self, name: &[u8]) -> bool {
        let file = self.cache.get(name).cloned();
        let (ok, acts) = task::load_task_inv_file(&self.view(), &self.env(), name, file.as_deref());
        self.run(acts);
        ok
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_inventory_item(&mut self, req: &CreateRequest<'_>, callback: CreateCallback) {
        let acts = task::create_inventory_item(&self.view(), &self.env(), req);
        let saved = self.create_callback;
        self.create_callback = callback;
        self.run(acts);
        self.create_callback = saved;
    }

    /// `create_item` of the event language.
    pub fn create_item(&mut self, succeed: bool) {
        let before = self.pending_http_count();
        let mut perm = Permissions::DEFAULT;
        perm.init(&agent(), &agent(), &NULL_ID, &NULL_ID);
        perm.init_masks(
            PERM_ALL,
            PERM_ALL,
            PERM_NONE,
            PERM_NONE,
            PERM_MOVE | PERM_TRANSFER,
        );
        let req = CreateRequest {
            asset_type: AssetType::LSL_TEXT,
            inventory_type: InventoryType::LSL,
            sub_type: 0,
            name: b"New Script",
            description: b"",
            permissions: &perm,
            params: None,
        };
        self.create_inventory_item(&req, CreateCallback::Trace);
        if self.pending_http_count() > before {
            let mut body = Value::map();
            insert(&mut body, "success", Value::Boolean(succeed));
            self.complete_http(Some(body), 200, Some("POST"));
        }
    }

    // --- the event language of the traces ------------------------------------

    /// `apply`: one event; false for an unknown one.
    pub fn apply(&mut self, ev: &str) -> bool {
        match ev {
            "req" => self.request_inventory(),
            "dirty" => self.dirty_inventory(),
            "timer" => self.fire_timer(),
            "die" => {
                self.alive = false;
                self.add("object.gone");
            }
            "revive" => {
                self.alive = true;
                self.add("object.back");
            }
            "unlisten" => {
                if let Some(l) = self.listeners.pop() {
                    if let Some(i) = self.obj.callbacks.iter().position(|&c| c == l) {
                        self.obj.callbacks.remove(i);
                    }
                }
            }
            "listen" => {
                let n = self.listeners.len() as u32 + 1;
                self.listen_new(n);
            }
            "select" => self.selected = true,
            "notowner" => self.obj.you_owner = false,
            "clearlisteners" => self.obj.callbacks.clear(),
            "rep.lo" | "rep.cur" | "rep.exp" | "rep.none" | "rep.hi" => {
                let st = self.obj.st;
                let serial = match ev {
                    "rep.lo" => st.expected.wrapping_sub(1),
                    "rep.cur" => st.serial,
                    "rep.hi" => st.expected.wrapping_add(1),
                    _ => st.expected,
                };
                let name = if ev == "rep.none" {
                    Vec::new()
                } else {
                    serial_file_name(serial)
                };
                self.add(format!(
                    "in ReplyTaskInventory serial={} filename={}",
                    serial,
                    esc(&name)
                ));
                self.deliver_reply(serial, &name);
            }
            "xfer.ok" => self.complete_xfer("ok", false),
            "xfer.ok.newest" => self.complete_xfer("ok", true),
            "xfer.err" => self.complete_xfer("err", false),
            "xfer.nofile" => self.complete_xfer("nofile", false),
            "cap.ok" => {
                let r = standard_reply(i32::from(self.obj.st.expected));
                self.complete_http(Some(r), 200, Some("GET"));
            }
            "cap.stale" => {
                let r = standard_reply(i32::from(self.obj.st.expected.wrapping_sub(1)));
                self.complete_http(Some(r), 200, Some("GET"));
            }
            "cap.304" => self.complete_http(None, 304, Some("GET")),
            "cap.err" => self.complete_http(None, 500, Some("GET")),
            "cap.nocontents" => {
                let mut body = Value::map();
                insert(
                    &mut body,
                    "inventory_serial",
                    Value::Integer(i32::from(self.obj.st.expected)),
                );
                self.complete_http(Some(body), 200, Some("GET"));
            }
            "upd.known" => {
                let a = ItemSpec {
                    desc: b"edited".to_vec(),
                    ..spec_a()
                };
                self.update_item(&a, TASK_INVENTORY_ITEM_KEY, false);
            }
            "upd.guess" => {
                let c = ItemSpec {
                    id: item('c'),
                    asset: asset('c'),
                    name: b"Guessed C".to_vec(),
                    ..spec_a()
                };
                self.update_item(&c, TASK_INVENTORY_ITEM_KEY, false);
            }
            "upd.new" => {
                let d = ItemSpec {
                    id: item('d'),
                    asset: asset('d'),
                    name: b"New D".to_vec(),
                    ..spec_a()
                };
                self.update_item(&d, TASK_INVENTORY_ITEM_KEY, true);
            }
            "upd.asset" => {
                let a = ItemSpec {
                    id: item('e'),
                    desc: b"by asset".to_vec(),
                    ..spec_a()
                };
                self.update_item(&a, TASK_INVENTORY_ASSET_KEY, false);
            }
            "remove" => self.remove_inventory(&item('a')),
            "move.nocopy" => self.move_inventory(&NULL_ID, &item('b')),
            "move.copy" => self.move_inventory(&NULL_ID, &item('a')),
            "create.ok" => self.create_item(true),
            "create.fail" => self.create_item(false),
            _ => {
                if let Some(v) = ev.strip_prefix("rep=") {
                    let serial = atoi(v) as i16;
                    let name = serial_file_name(serial);
                    self.add(format!(
                        "in ReplyTaskInventory serial={} filename={}",
                        serial,
                        esc(&name)
                    ));
                    self.deliver_reply(serial, &name);
                } else if let Some(v) = ev.strip_prefix("cap=") {
                    let r = standard_reply(atoi(v));
                    self.complete_http(Some(r), 200, Some("GET"));
                } else if let Some(v) = ev.strip_prefix("set.serial=") {
                    self.obj.st.serial = atoi(v) as i16;
                } else if let Some(v) = ev.strip_prefix("set.expected=") {
                    self.obj.st.expected = atoi(v) as i16;
                } else {
                    return false;
                }
            }
        }
        true
    }

    // --- the recorder's dumps -------------------------------------------------

    /// `state_line`.
    pub fn state_line(&self) -> String {
        let st = &self.obj.st;
        let letter = match st.request {
            RequestState::Stopped => "S",
            RequestState::Wait => "W",
            RequestState::Pending => "P",
            RequestState::Xfer => "X",
        };
        format!(
            "st={} ser={} exp={} xid={} dirty={} inv={}",
            letter,
            st.serial,
            st.expected,
            self.xfer_id_text(st.xfer_id),
            i32::from(st.dirty),
            inv_brief(self.obj.inv.as_deref())
        )
    }

    /// `helpers_text`.
    pub fn helpers_text(&self, indent: &str) -> String {
        let inv = self.obj.inv.as_deref();
        let entries = inv.unwrap_or(&[]);
        let mut out = format!("{indent}helpers root=");
        out.push_str(&lookup::root(entries).map_or("none".to_owned(), |i| nm(&entries[i].id())));
        let contents: Vec<&Entry> = lookup::contents(entries)
            .into_iter()
            .map(|i| &entries[i])
            .collect();
        out.push_str(" contents=[");
        out.push_str(
            &contents
                .iter()
                .map(|e| nm(&e.id()))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push(']');
        let mut types: Vec<i32> = Vec::new();
        let mut assets: Vec<Uuid> = Vec::new();
        let mut typed: Vec<(Uuid, i32)> = Vec::new();
        let mut plain: Vec<i32> = Vec::new();
        for e in entries {
            let t = e.asset_type().0;
            if !types.contains(&t) {
                types.push(t);
            }
            let Some(it) = e.as_item() else {
                if !plain.contains(&t) {
                    plain.push(t);
                }
                continue;
            };
            let a = it.item.asset;
            if !assets.contains(&a) {
                assets.push(a);
            }
            if !typed.contains(&(a, t)) {
                typed.push((a, t));
            }
        }
        out.push_str(" count{");
        let counts: Vec<String> = types
            .iter()
            .map(|&t| format!("{}:{}", t, lookup::count_of_type(entries, AssetType(t))))
            .collect();
        out.push_str(&counts.join(","));
        out.push('}');
        let safe = plain
            .iter()
            .all(|&t| t == AssetType::CATEGORY.0 || t == AssetType::NONE.0);
        out.push_str(" byasset{");
        if !safe {
            out.push_str("not-called");
        } else {
            let parts: Vec<String> = assets
                .iter()
                .map(|a| {
                    let f = lookup::item_by_asset(entries, a)
                        .map_or("none".to_owned(), |i| nm(&entries[i].id()));
                    format!("{}:{}", nm(a), f)
                })
                .collect();
            out.push_str(&parts.join(","));
        }
        out.push_str("} bytype{");
        let mut first = true;
        for (a, t) in &typed {
            if *t == AssetType::CATEGORY.0 {
                continue;
            }
            if !first {
                out.push(',');
            }
            first = false;
            let _ = write!(out, "{}/{}:", nm(a), t);
            if plain.contains(t) {
                out.push_str("not-called");
                continue;
            }
            out.push_str(
                &lookup::item_by_asset_and_type(entries, a, AssetType(*t))
                    .map_or("none".to_owned(), |i| nm(&entries[i].id())),
            );
        }
        out.push_str("} byid{");
        let mut ids: Vec<Uuid> = Vec::new();
        for e in entries {
            if !ids.contains(&e.id()) {
                ids.push(e.id());
            }
        }
        let parts: Vec<String> = ids
            .iter()
            .map(|id| {
                let o = lookup::object_by_id(entries, id);
                let it = lookup::item_by_id(entries, id);
                let kind = match o {
                    Some(i) if entries[i].as_item().is_some() => "item",
                    Some(_) => "object",
                    None => "none",
                };
                let same = match it {
                    Some(j) if Some(j) == o => "same",
                    Some(_) => "other",
                    None => "null",
                };
                format!("{}:{}/{}", nm(id), kind, same)
            })
            .collect();
        out.push_str(&parts.join(","));
        out.push_str("} inasset{");
        let mut first = true;
        for e in entries {
            let Some(it) = e.as_item() else { continue };
            let t = it.item.object.asset_type.0;
            if !first {
                out.push(',');
            }
            first = false;
            let _ = write!(out, "{}:", nm(&e.id()));
            if t == AssetType::CATEGORY.0 || plain.contains(&t) {
                out.push_str("not-called");
                continue;
            }
            let r =
                lookup::is_asset_in_inventory(inv, &self.obj.pending, &it.item.asset, AssetType(t));
            out.push(if r { '1' } else { '0' });
        }
        let _ = writeln!(
            out,
            "}} state{{listeners={} dirty={} serial={} pending={}}}",
            i32::from(!self.obj.callbacks.is_empty()),
            i32::from(self.obj.st.dirty),
            self.obj.st.serial,
            i32::from(self.obj.st.request != RequestState::Stopped)
        );
        out
    }

    /// `entries_text`.
    pub fn entries_text(&self, indent: &str) -> String {
        let Some(inv) = self.obj.inv.as_deref() else {
            return format!("{indent}inventory null\n") + &self.helpers_text(indent);
        };
        let mut out = format!("{indent}inventory {} entries", inv.len());
        match inv.last() {
            None => out.push_str(" root none\n"),
            Some(e) => {
                let _ = writeln!(out, " root {}", nm(&e.id()));
            }
        }
        for (i, e) in inv.iter().enumerate() {
            let o = e.object();
            let _ = write!(
                out,
                "{indent} [{i}] {} uuid={} parent={} type={} name={} thumb={} fav={} runtime={} created={}",
                if e.as_item().is_some() {
                    "item"
                } else {
                    "object"
                },
                nm(&o.id),
                nm(&o.parent),
                o.asset_type.0,
                esc(&o.name),
                nm(&o.thumbnail),
                i32::from(o.favorite),
                esc(&o.runtime),
                o.creation_date
            );
            if let Some(v) = e.as_item() {
                out.push_str(&item_fields(v));
            }
            out.push('\n');
        }
        out + &self.helpers_text(indent)
    }
}

/// The item part of an entry line.
pub fn item_fields(v: &ViewerItem) -> String {
    let it: &Item = &v.item;
    let p = &it.permissions;
    format!(
        " asset={} desc={} invtype={} flags={:08x} sale={}/{} {} complete={} txn={} running={} faulted={} crc={:08x}",
        nm(&it.asset),
        esc(&it.description),
        it.inv_type.0,
        it.flags,
        it.sale_info.sale_type.0,
        it.sale_info.price,
        perm_text(p),
        i32::from(v.complete),
        nm(&v.transaction),
        i32::from(v.running),
        i32::from(v.faulted),
        it.crc32()
    )
}

/// `perm_text`.
pub fn perm_text(p: &Permissions) -> String {
    format!(
        "base={:08x} owner={:08x} group={:08x} every={:08x} next={:08x} creator={} ownerid={} lastowner={} groupid={} groupowned={}",
        p.base,
        p.owner_mask,
        p.group_mask,
        p.everyone,
        p.next,
        nm(&p.creator),
        nm(&p.owner),
        nm(&p.last_owner),
        nm(&p.group),
        i32::from(p.is_group_owned())
    )
}

/// `inv_brief`.
pub fn inv_brief(inv: Option<&[Entry]>) -> String {
    match inv {
        None => "-".to_owned(),
        Some(inv) => format!(
            "[{}]",
            inv.iter()
                .map(|e| nm(&e.id()))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

pub fn serial_file_name(serial: i16) -> Vec<u8> {
    format!("inv_s{serial}.tmp").into_bytes()
}

/// `atoi`.
pub fn atoi(s: &str) -> i32 {
    let b = s.trim_start().as_bytes();
    let (neg, digits) = match b.first() {
        Some(b'-') => (true, &b[1..]),
        Some(b'+') => (false, &b[1..]),
        _ => (false, b),
    };
    let mut v: i64 = 0;
    for &c in digits.iter().take_while(|c| c.is_ascii_digit()) {
        v = v * 10 + i64::from(c - b'0');
    }
    (if neg { -v } else { v }) as i32
}

/// `run_trace`: one header line and one line per event.
pub fn run_trace(caps: bool, listeners: u32, events: &[&str], label: &str) -> String {
    let mut out = format!("trace {}/L{}", if caps { "cap" } else { "udp" }, listeners);
    if !label.is_empty() {
        out.push(' ');
        out.push_str(label);
    }
    out.push_str(" :");
    for e in events {
        out.push(' ');
        out.push_str(e);
    }
    out.push('\n');
    let mut s = Scene::new(caps, listeners, false);
    for e in events {
        if !s.apply(e) {
            let _ = writeln!(out, " {e} | UNKNOWN EVENT");
            continue;
        }
        let actions = s.take();
        let _ = writeln!(
            out,
            " {} | {} cache={} | {}",
            e,
            s.state_line(),
            s.cache_text(),
            actions
        );
    }
    out
}

/// A `Value` LLSD map from key/value pairs.
pub fn map(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Value::map();
    for (k, v) in pairs {
        insert(&mut m, k, v);
    }
    m
}

pub fn s(v: &str) -> Value {
    Value::String(v.as_bytes().to_vec())
}
