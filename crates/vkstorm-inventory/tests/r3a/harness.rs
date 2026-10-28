//! The recording environment of the R3a corpus, in Rust: what
//! `llaisupdate_parity_stubs.cpp` and the C++ that stays in the viewer do
//! around the crate's AIS code.
//!
//! - The skeleton model (`gInventory` over the scripted skeleton) and the
//!   viewer objects' recording stubs, as the stub file emulates them: the
//!   queries, `updateCategory`, `updateItem` (mesh skip, bug 23 refusal, the
//!   null-parent, Lost And Found and default-folder moves and their
//!   accounting), `onObjectDeletedFromServer`, `fetchDescendentsOf`,
//!   `accountForUpdate` (the frozen copy), `notifyObservers`, the changed-id
//!   set with the link propagation, `fetch()`'s request-timer gate,
//!   `getVersion`/`setVersion`, `updateParentOnServer` and its message.
//! - What stays C++ in the port, written here as the C++ does it: the
//!   coroutine (`gDisconnected`, firing the callbacks), the executor of the
//!   crate's `ApplyOp`s (`doUpdate()`'s body), the timers and
//!   `checkTimeout()` with its yields, the coprocedure pool
//!   (`EnqueueAISCommand`, `onIdle`) and the recording HTTP adapter.
//! - The harness's own machinery: the injected clock and frame counter, the
//!   hooks, the scripted shutdown, the op list.
//!
//! The crate supplies the decisions: `ais::result` (status, error branch,
//! callback ids), `ais::update` (the parse and the op list) and
//! `ais::command` (the command functions' requests).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use vkstorm_inventory::ais::command::{Command, Method, Request, MAX_SIMULTANEOUS_COROUTINES};
use vkstorm_inventory::ais::result::{begin, callback_ids, error_branch, AlertState, CommandContext};
use vkstorm_inventory::ais::update::{
    ApplyOp, CategoryRecord, FetchTimer, Flow, ItemRecord, Model, ModelCategory, ModelRef, ParseContext,
    ParsedUpdate, MAX_UPDATE_BACKLOG,
};
use vkstorm_inventory::ais::CommandType;
use vkstorm_inventory::category::{Category, VERSION_UNKNOWN};
use vkstorm_inventory::llsd::coerce::{as_boolean, as_integer, as_string, as_uuid, get, has};
use vkstorm_inventory::perms::{Uuid, NULL_ID};
use vkstorm_inventory::types::{AssetType, FolderType};
use vkstorm_llsd::{Format, Options, Value};
use vkstorm_messages::{parse_template, Builder, Template, Tokenizer, VarType, MAX_BUFFER_SIZE};

// LLInventoryObserver masks.
const LABEL: u32 = 1;
const INTERNAL: u32 = 2;
const ADD: u32 = 4;
const REMOVE: u32 = 8;
const STRUCTURE: u32 = 16;
const CALLING_CARD: u32 = 32;
const GESTURE: u32 = 64;

/// `CATEGORIZE_LOST_AND_FOUND_ID` (`00000000-0000-0000-0000-000000000010`).
const CATEGORIZE_LOST_AND_FOUND_ID: Uuid = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10];

/// `AISUpdate::AIS_TASK_EXPIRY_SECONDS` and `AIS_BATCH_EXPIRY_SECONDS`.
const AIS_TASK_EXPIRY_SECONDS: f32 = 0.008;
const AIS_BATCH_EXPIRY_SECONDS: f32 = 0.010;

/// The two runs of a case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The timers never report expiry.
    Never,
    /// `LLTimer`'s arithmetic over the injected clock.
    Sched,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Never => "never",
            Mode::Sched => "sched",
        }
    }
}

/// `vks_r3a_LLTimer`: `LLTimer`'s arithmetic over the injected clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timer {
    last: u64,
    expiry: u64,
}

/// `LLCoros::Stopping`, thrown by the scripted `checkStop()`.
#[derive(Debug)]
pub struct Stopped;

/// A postponed command procedure (`sPostponedQuery`).
struct Postponed {
    name: String,
    req: Request,
    with_callback: bool,
}

/// A folder of the skeleton, with the serial the parse's handles name.
struct ModelCat {
    rec: CategoryRecord,
    serial: u64,
}

/// The request timer of an object: `LLFrameTimer` over a frame time that
/// never advances in the test (0). Fresh (`reset()`) is expired; `fetch()`
/// arms it for `expiry_seconds`.
const FRESH_TIMER: FetchTimer = FetchTimer { start_time: 0.0, expiry: 0.0, started: true };

fn timer_of(t: &Option<FetchTimer>) -> FetchTimer {
    t.unwrap_or(FRESH_TIMER)
}

// ---------------------------------------------------------------------------
// LLSD helpers
// ---------------------------------------------------------------------------

pub fn map(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = std::collections::BTreeMap::new();
    for (k, v) in pairs {
        m.insert(k.as_bytes().to_vec(), v);
    }
    Value::Map(m)
}

fn s(v: &str) -> Value {
    Value::String(v.as_bytes().to_vec())
}

fn i(v: i32) -> Value {
    Value::Integer(v)
}

fn b(v: bool) -> Value {
    Value::Boolean(v)
}

fn id(v: &Uuid) -> Value {
    Value::Uuid(*v)
}

fn set_key(v: &mut Value, k: &str, x: Value) {
    if let Value::Map(m) = v {
        m.insert(k.as_bytes().to_vec(), x);
    }
}

pub fn to_binary(v: &Value) -> Vec<u8> {
    vkstorm_llsd::format(Format::Binary, v, Options::default()).0
}

/// `item_fields`: every member of an item, raw.
pub fn item_fields(r: &ItemRecord) -> Value {
    let it = &r.item;
    let o = &it.object;
    let p = &it.permissions;
    map(vec![
        ("item_id", id(&o.id)),
        ("parent_id", id(&o.parent)),
        ("asset_id", id(&it.asset)),
        ("type", i(o.asset_type.0)),
        ("inv_type", i(it.inv_type.0)),
        ("flags", i(it.flags as i32)),
        ("name", Value::String(o.name.clone())),
        ("desc", Value::String(it.description.clone())),
        ("created_at", i(o.creation_date as i32)),
        ("thumbnail_id", id(&o.thumbnail)),
        ("favorite", b(o.favorite)),
        ("runtime", Value::String(o.runtime.clone())),
        (
            "permissions",
            map(vec![
                ("creator_id", id(&p.creator)),
                ("owner_id", id(&p.owner)),
                ("last_owner_id", id(&p.last_owner)),
                ("group_id", id(&p.group)),
                ("is_owner_group", b(p.group_owned)),
                ("base_mask", i(p.base as i32)),
                ("owner_mask", i(p.owner_mask as i32)),
                ("group_mask", i(p.group_mask as i32)),
                ("everyone_mask", i(p.everyone as i32)),
                ("next_owner_mask", i(p.next as i32)),
            ]),
        ),
        ("sale_info", map(vec![("sale_type", i(it.sale_info.sale_type.0)), ("sale_price", i(it.sale_info.price))])),
        ("complete", b(r.complete)),
        ("transaction_id", id(&r.transaction_id)),
        ("running", b(r.running)),
        ("faulted", b(r.faulted)),
    ])
}

/// `category_fields`: every member of a folder, raw (`mFetching` is
/// `FETCH_NONE` in every object of the test).
pub fn category_fields(r: &CategoryRecord) -> Value {
    let o = &r.category.object;
    map(vec![
        ("category_id", id(&o.id)),
        ("parent_id", id(&o.parent)),
        ("name", Value::String(o.name.clone())),
        ("type_default", i(r.category.preferred_type.0)),
        ("type", i(o.asset_type.0)),
        ("thumbnail_id", id(&o.thumbnail)),
        ("favorite", b(o.favorite)),
        ("owner_id", id(&r.owner)),
        ("version", i(r.version)),
        ("descendents", i(r.descendents)),
        ("fetching", i(0)),
    ])
}

/// `copyViewerCategory(other)` into `dst`: `copyCategory` (the date stays),
/// the owner, the version, the count and the request timer.
fn copy_viewer_category(dst: &mut CategoryRecord, other: &CategoryRecord) {
    dst.category.object.copy_object(&other.category.object);
    dst.category.preferred_type = other.category.preferred_type;
    dst.owner = other.owner;
    dst.version = other.version;
    dst.descendents = other.descendents;
    dst.requested = Some(timer_of(&other.requested));
}

// ---------------------------------------------------------------------------
// The message system's outgoing side
// ---------------------------------------------------------------------------

struct Messages {
    move_item: Arc<Template>,
    builder: Builder,
}

impl Messages {
    fn load() -> Messages {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../vkstorm-messages/tests/fixtures/message_template.msg");
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let file = parse_template(&mut Tokenizer::new(&text)).expect("message template parses");
        let move_item = file
            .messages
            .into_iter()
            .find(|t| t.name == "MoveInventoryItem")
            .map(|t| Arc::new(t.with_index_keys()))
            .expect("MoveInventoryItem in the template");
        Messages { move_item, builder: Builder::new() }
    }

    fn add(&mut self, block: &str, var: &str, data: &[u8], ty: VarType) {
        let t = self.move_item.clone();
        let key = t.block_by_name(block).unwrap().variable_by_name(var).unwrap().key;
        self.builder.add_data(key, data, ty).unwrap();
    }

    fn next_block(&mut self, block: &str) {
        let key = self.move_item.block_by_name(block).unwrap().key;
        self.builder.next_block(key).unwrap();
    }

    /// The frozen `updateParentOnServer`'s `MoveInventoryItem`, as
    /// `buildMessage(buf, size, 0)` encodes it (6 zero header bytes first).
    fn move_inventory_item(
        &mut self,
        agent: &Uuid,
        session: &Uuid,
        restamp: bool,
        item: &Uuid,
        folder: &Uuid,
    ) -> Vec<u8> {
        self.builder.new_message(self.move_item.clone());
        self.next_block("AgentData");
        self.add("AgentData", "AgentID", agent, VarType::LLUUID);
        self.add("AgentData", "SessionID", session, VarType::LLUUID);
        self.add("AgentData", "Stamp", &[u8::from(restamp)], VarType::Bool);
        self.next_block("InventoryData");
        self.add("InventoryData", "ItemID", item, VarType::LLUUID);
        self.add("InventoryData", "FolderID", folder, VarType::LLUUID);
        // addString("NewName", NULL): no bytes.
        self.add("InventoryData", "NewName", &[], VarType::Variable);
        let mut buf = vec![0u8; MAX_BUFFER_SIZE];
        let n = self.builder.build_message(&mut buf, 0).expect("message builds");
        buf.truncate(n);
        self.builder.clear_message();
        buf
    }
}

// ---------------------------------------------------------------------------
// The harness
// ---------------------------------------------------------------------------

pub struct Harness {
    // the skeleton model
    cats: BTreeMap<Uuid, ModelCat>,
    by_serial: BTreeMap<u64, Uuid>,
    next_serial: u64,
    items: BTreeMap<Uuid, ItemRecord>,
    root: Uuid,
    changed: BTreeSet<Uuid>,
    agent_id: Uuid,
    session_id: Uuid,
    internal: u32,
    // the run
    mode: Mode,
    clock: u64,
    clock_base: u64,
    frame: i32,
    frame_base: i32,
    call_us: u64,
    frame_us: u64,
    others_us: Vec<u64>,
    yield_extra_us: Vec<u64>,
    yields: usize,
    ops: Vec<Value>,
    // the step being run
    hooks: Vec<Value>,
    in_apply: bool,
    parse_checkpoints: i32,
    apply_checkpoints: i32,
    recheck_pending: bool,
    force_expiry: bool,
    step_yields: i32,
    nest: i32,
    stop_armed: bool,
    stop_dump: Vec<u8>,
    stop_op_index: usize,
    // process statics
    batch: Timer,
    batch_frame: u32,
    alert: AlertState,
    disconnected: bool,
    // the command functions' environment
    has_region: bool,
    inv_cap: String,
    lib_cap: String,
    pending: i32,
    next_result: Value,
    postponed: VecDeque<Postponed>,
    idle_registered: bool,
    tid_counter: u64,
    tids: BTreeSet<String>,
    pub tid_failures: Vec<String>,
    case_id: String,
    messages: Messages,
    /// Every `ApplyOp` kind the executor ran (for the coverage report).
    pub apply_op_counts: BTreeMap<&'static str, usize>,
    /// Where the op being recorded comes from (see [`Phase`]).
    phase: Phase,
    /// Recorded ops by run, phase and op kind (for the coverage report).
    pub phase_counts: BTreeMap<(&'static str, Phase, String), usize>,
}

/// The code that records an op: what the crate decides, and what stays C++
/// (or is the harness's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Step and hook markers, hook bodies outside a nested update.
    Harness,
    /// A command function (`ais::command` builds the request; the
    /// capability lookup, the pool and the adapter are C++).
    Command,
    /// The coroutine's error branch (`ais::result::error_branch`).
    ErrorBranch,
    /// The parse (`ais::update::ParsedUpdate::parse`): its queries, its
    /// model calls, and the yields of its checkpoints.
    Parse,
    /// The executor of `ApplyOp`s (`doUpdate()`), and its yields.
    Apply,
    /// The completion callbacks (`ais::result::callback_ids`), and the
    /// coroutine's `gDisconnected` callback.
    Callbacks,
}

impl Harness {
    pub fn new() -> Harness {
        Harness {
            cats: BTreeMap::new(),
            by_serial: BTreeMap::new(),
            next_serial: 1,
            items: BTreeMap::new(),
            root: NULL_ID,
            changed: BTreeSet::new(),
            agent_id: NULL_ID,
            session_id: NULL_ID,
            internal: 0,
            mode: Mode::Never,
            clock: 1_000_000,
            clock_base: 0,
            frame: 0,
            frame_base: 0,
            call_us: 100,
            frame_us: 16667,
            others_us: Vec::new(),
            yield_extra_us: Vec::new(),
            yields: 0,
            ops: Vec::new(),
            hooks: Vec::new(),
            in_apply: false,
            parse_checkpoints: 0,
            apply_checkpoints: 0,
            recheck_pending: false,
            force_expiry: false,
            step_yields: 0,
            nest: 0,
            stop_armed: false,
            stop_dump: Vec::new(),
            stop_op_index: 0,
            batch: Timer::default(),
            batch_frame: 0,
            alert: AlertState::default(),
            disconnected: false,
            has_region: true,
            inv_cap: String::new(),
            lib_cap: String::new(),
            pending: 0,
            next_result: Value::Undefined,
            postponed: VecDeque::new(),
            idle_registered: false,
            tid_counter: 0,
            tids: BTreeSet::new(),
            tid_failures: Vec::new(),
            case_id: String::new(),
            messages: Messages::load(),
            apply_op_counts: BTreeMap::new(),
            phase: Phase::Harness,
            phase_counts: BTreeMap::new(),
        }
    }

    // ---- the recorder ------------------------------------------------------

    fn recording(&self) -> bool {
        self.internal == 0
    }

    fn record(&mut self, op: Value, costs_time: bool) {
        let kind = String::from_utf8_lossy(&as_string(get(&op, "op"))).into_owned();
        *self.phase_counts.entry((self.mode.name(), self.phase, kind)).or_insert(0) += 1;
        self.ops.push(op);
        if costs_time && self.mode == Mode::Sched {
            self.clock += self.call_us;
        }
    }

    fn rel_clock(&self) -> u64 {
        self.clock - self.clock_base
    }

    fn rel_frame(&self) -> i32 {
        self.frame - self.frame_base
    }

    pub fn take_ops(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.ops)
    }

    // ---- runs --------------------------------------------------------------

    pub fn set_case(&mut self, case_id: &str) {
        self.case_id = case_id.to_string();
    }

    pub fn begin_run(&mut self, mode: Mode, call_us: u64, frame_us: u64, others_us: Vec<u64>, extra: Vec<u64>) {
        self.ops.clear();
        self.mode = mode;
        self.call_us = call_us;
        self.frame_us = frame_us;
        self.others_us = others_us;
        self.yield_extra_us = extra;
        self.yields = 0;
        self.hooks.clear();
        self.stop_armed = false;
        // A frame no earlier run used: the first update restarts the batch timer.
        self.frame += 1;
        self.frame_base = self.frame;
        self.clock += 1_000_000;
        self.clock_base = self.clock;
    }

    pub fn next_frame(&mut self) {
        self.frame += 1;
        if self.mode == Mode::Sched {
            self.clock += self.frame_us;
        }
    }

    /// The step marker (costs no time).
    pub fn step_marker(&mut self, index: i32, ty: &str) {
        self.record(map(vec![("op", s("step")), ("index", i(index)), ("type", s(ty))]), false);
    }

    pub fn set_disconnected(&mut self, d: bool) {
        self.disconnected = d;
    }

    // ---- the skeleton ------------------------------------------------------

    fn insert_cat(&mut self, rec: CategoryRecord) {
        let cid = rec.category.object.id;
        if let Some(old) = self.cats.remove(&cid) {
            self.by_serial.remove(&old.serial);
        }
        let serial = self.next_serial;
        self.next_serial += 1;
        self.by_serial.insert(serial, cid);
        self.cats.insert(cid, ModelCat { rec, serial });
    }

    fn erase_cat(&mut self, cid: &Uuid) {
        if let Some(old) = self.cats.remove(cid) {
            self.by_serial.remove(&old.serial);
        }
    }

    pub fn skeleton_load(&mut self, c: &Value) {
        self.cats.clear();
        self.by_serial.clear();
        self.items.clear();
        self.agent_id = as_uuid(get(c, "agent_id"));
        self.session_id = as_uuid(get(c, "session_id"));
        self.root = as_uuid(get(c, "root_id"));
        let skel = get(c, "skeleton");
        if let Value::Array(cats) = get(skel, "categories") {
            for m in cats {
                let owner = if has(m, "owner_id") { as_uuid(get(m, "owner_id")) } else { self.agent_id };
                let rec = CategoryRecord {
                    category: Category::new(
                        &as_uuid(get(m, "category_id")),
                        &as_uuid(get(m, "parent_id")),
                        FolderType(as_integer(get(m, "type_default"))),
                        &as_string(get(m, "name")),
                    ),
                    owner,
                    version: if has(m, "version") { as_integer(get(m, "version")) } else { VERSION_UNKNOWN },
                    descendents: if has(m, "descendents") { as_integer(get(m, "descendents")) } else { -1 },
                    requested: Some(FRESH_TIMER),
                };
                self.insert_cat(rec);
            }
        }
        if let Value::Array(items) = get(skel, "items") {
            for m in items {
                let mut rec = ItemRecord::default();
                rec.item.from_llsd(m, true, false);
                rec.complete = if has(m, "_complete") { as_boolean(get(m, "_complete")) } else { true };
                if has(m, "_transaction_id") {
                    rec.transaction_id = as_uuid(get(m, "_transaction_id"));
                }
                self.items.insert(rec.item.object.id, rec);
            }
        }
        // notifyObservers() clears the changed-id set (not recorded).
        self.changed.clear();
    }

    pub fn skeleton_dump(&self) -> Value {
        Value::Map(
            [
                (b"categories".to_vec(), Value::Array(self.cats.values().map(|c| category_fields(&c.rec)).collect())),
                (b"items".to_vec(), Value::Array(self.items.values().map(item_fields).collect())),
                (b"changed_ids".to_vec(), Value::Array(self.changed.iter().map(id).collect())),
            ]
            .into_iter()
            .collect(),
        )
    }

    /// The children of a folder in the skeleton (`getViewerDescendentCount`).
    fn child_count(&self, cid: &Uuid) -> i32 {
        if !self.cats.contains_key(cid) {
            return 0;
        }
        let n = self.cats.values().filter(|c| c.rec.category.object.parent == *cid).count()
            + self.items.values().filter(|i| i.item.object.parent == *cid).count();
        n as i32
    }

    fn remove_descendents(&mut self, cid: &Uuid) {
        let sub: Vec<Uuid> =
            self.cats.iter().filter(|(_, c)| c.rec.category.object.parent == *cid).map(|(k, _)| *k).collect();
        for s in &sub {
            self.remove_descendents(s);
            self.erase_cat(s);
        }
        let items: Vec<Uuid> =
            self.items.iter().filter(|(_, i)| i.item.object.parent == *cid).map(|(k, _)| *k).collect();
        for s in &items {
            self.items.remove(s);
        }
    }

    // ---- LLInventoryModel: the recording stub ------------------------------

    fn q_get_item(&mut self, iid: &Uuid) -> Option<ItemRecord> {
        let item = self.items.get(iid).cloned();
        if self.recording() {
            let mut op = map(vec![("op", s("q.getItem")), ("id", id(iid)), ("found", b(item.is_some()))]);
            if let Some(it) = &item {
                set_key(&mut op, "obj", item_fields(it));
            }
            self.record(op, true);
        }
        item
    }

    fn q_get_category(&mut self, cid: &Uuid) -> Option<(u64, CategoryRecord)> {
        let cat = self.cats.get(cid).map(|c| (c.serial, c.rec.clone()));
        if self.recording() {
            let mut op = map(vec![("op", s("q.getCategory")), ("id", id(cid)), ("found", b(cat.is_some()))]);
            if let Some((_, c)) = &cat {
                set_key(&mut op, "obj", category_fields(c));
            }
            self.record(op, true);
        }
        cat
    }

    fn cat_by_serial(&self, serial: u64) -> Uuid {
        *self.by_serial.get(&serial).expect("a model folder the update holds was deleted (a dangling pointer in C++)")
    }

    /// `getVersion()` on a model folder.
    fn model_get_version(&mut self, cid: &Uuid) -> i32 {
        let v = self.cats[cid].rec.version;
        if self.recording() {
            self.record(map(vec![("op", s("q.getVersion")), ("id", id(cid)), ("result", i(v))]), true);
        }
        v
    }

    /// `setVersion()` on a model folder.
    fn model_set_version(&mut self, cid: &Uuid, v: i32) {
        let before = self.cats[cid].rec.version;
        if self.recording() {
            self.record(
                map(vec![("op", s("setVersion")), ("id", id(cid)), ("version", i(v)), ("before", i(before))]),
                true,
            );
        }
        self.cats.get_mut(cid).unwrap().rec.version = v;
    }

    /// `fetch(expiry_seconds)` on a model folder: the request-timer gate.
    fn model_fetch(&mut self, cid: &Uuid, expiry: i32) -> bool {
        let c = &mut self.cats.get_mut(cid).unwrap().rec;
        let t = timer_of(&c.requested);
        let would_start = c.version == VERSION_UNKNOWN && 0.0 >= t.expiry;
        if would_start {
            c.requested = Some(FetchTimer { start_time: 0.0, expiry: f64::from(expiry as f32), started: true });
        }
        let version = c.version;
        if self.recording() {
            self.record(
                map(vec![
                    ("op", s("fetch")),
                    ("id", id(cid)),
                    ("expiry", i(expiry)),
                    ("version", i(version)),
                    ("would_start", b(would_start)),
                    ("is_model_object", b(true)),
                ]),
                true,
            );
        }
        would_start
    }

    /// The real lookup: the root for `FT_ROOT_INVENTORY`, else the smallest
    /// id among the root's direct children of that type, else null.
    fn find_category_uuid_for_type(&mut self, t: FolderType) -> Uuid {
        let mut rv = NULL_ID;
        if t == FolderType::ROOT_INVENTORY {
            rv = self.root;
        } else if self.root != NULL_ID {
            for (k, c) in &self.cats {
                if c.rec.category.object.parent == self.root && c.rec.category.preferred_type == t {
                    rv = *k;
                    break;
                }
            }
        }
        if self.recording() {
            self.record(map(vec![("op", s("q.findCategoryUUIDForType")), ("type", i(t.0)), ("result", id(&rv))]), true);
        }
        rv
    }

    /// `addChangedMask`'s changed-id bookkeeping.
    fn add_changed_mask(&mut self, mask: u32, referent: &Uuid) {
        if *referent == NULL_ID || self.changed.contains(referent) {
            return;
        }
        self.changed.insert(*referent);
        if mask & LABEL != 0 {
            let target_item = self.items.get(referent);
            let exists = target_item.is_some() || self.cats.contains_key(referent);
            if exists && !target_item.is_some_and(|t| t.item.is_link_type()) {
                let links: Vec<Uuid> = self
                    .items
                    .iter()
                    .filter(|(_, i)| i.item.is_link_type() && i.item.asset == *referent)
                    .map(|(k, _)| *k)
                    .collect();
                for l in &links {
                    self.add_changed_mask(LABEL, l);
                }
            }
        }
    }

    fn notify_observers(&mut self) {
        if self.recording() {
            self.record(map(vec![("op", s("notifyObservers")), ("changed", i(self.changed.len() as i32))]), true);
        }
        self.changed.clear();
    }

    fn update_category(&mut self, cat: &CategoryRecord, mut mask: u32) {
        let cid = cat.category.object.id;
        if self.recording() {
            let op = map(vec![
                ("op", s("updateCategory")),
                ("mask", i(mask as i32)),
                ("obj", category_fields(cat)),
                ("exists", b(self.cats.contains_key(&cid))),
                ("parent_known", b(self.cats.contains_key(&cat.category.object.parent))),
            ]);
            self.record(op, true);
        }
        self.internal += 1;
        if cid != NULL_ID {
            if let Some(old) = self.cats.get_mut(&cid) {
                if old.rec.category.object.parent != cat.category.object.parent {
                    mask |= STRUCTURE | INTERNAL;
                }
                if old.rec.category.object.name != cat.category.object.name {
                    mask |= LABEL;
                }
                copy_viewer_category(&mut old.rec, cat);
                self.add_changed_mask(mask, &cid);
            } else {
                let mut new_cat = CategoryRecord::new_owned(&cat.owner);
                new_cat.requested = Some(FRESH_TIMER);
                copy_viewer_category(&mut new_cat, cat);
                self.insert_cat(new_cat);
                self.add_changed_mask(mask | ADD, &cid);
            }
        }
        self.internal -= 1;
    }

    /// `LLViewerInventoryItem::getType()`: a link takes its target's type.
    fn viewer_type(&self, it: &ItemRecord) -> AssetType {
        let t = it.item.object.asset_type;
        if t == AssetType::LINK
            && let Some(target) = self.items.get(&it.item.asset)
            && !target.item.is_link_type()
        {
            return target.item.object.asset_type;
        }
        if t == AssetType::LINK_FOLDER
            && let Some(cat) = self.cats.get(&it.item.asset)
        {
            return cat.rec.category.object.asset_type;
        }
        t
    }

    fn update_item(&mut self, item: &ItemRecord, mut mask: u32) {
        let iid = item.item.object.id;
        let op_index = self.ops.len();
        let record_it = self.recording();
        if record_it {
            let op = map(vec![
                ("op", s("updateItem")),
                ("mask", i(mask as i32)),
                ("obj", item_fields(item)),
                ("exists", b(self.items.contains_key(&iid))),
                ("parent_known", b(self.cats.contains_key(&item.item.object.parent))),
            ]);
            self.record(op, true);
        }
        self.internal += 1;
        let outcome: &str;
        if iid == NULL_ID {
            outcome = "ignored_null_id";
        } else {
            let ty = self.viewer_type(item);
            if ty == AssetType::MESH || ty == AssetType::GLTF || ty == AssetType::GLTF_BIN {
                outcome = "skipped_type";
            } else if self.items.contains_key(&iid) {
                let old_parent = self.items[&iid].item.object.parent;
                let mut new_parent = item.item.object.parent;
                let mut update_parent_on_server = false;
                let mut o = "updated";
                if new_parent == NULL_ID {
                    if old_parent == NULL_ID {
                        new_parent = self.find_category_uuid_for_type(FolderType::LOST_AND_FOUND);
                        o = "updated_null_parent_to_lost_and_found_server_move";
                    } else {
                        new_parent = old_parent;
                        o = "updated_null_parent_kept_old_parent_server_move";
                    }
                    update_parent_on_server = true;
                }
                if update_parent_on_server && old_parent != new_parent && self.cats.contains_key(&new_parent) {
                    self.account_for_update(&new_parent, 1, true);
                }
                if old_parent != new_parent {
                    mask |= STRUCTURE;
                }
                let old = self.items.get_mut(&iid).unwrap();
                if old.item.object.name != item.item.object.name {
                    mask |= LABEL;
                }
                old.copy_viewer_item(item);
                if update_parent_on_server {
                    old.item.object.parent = new_parent;
                }
                mask |= INTERNAL;
                outcome = o;
            } else if ty.0 <= AssetType::NONE.0 {
                outcome = "refused_bad_type";
            } else {
                let mut new_item = ItemRecord::default();
                new_item.copy_viewer_item(item);
                let mut o = "added";
                if item.item.object.parent == NULL_ID {
                    let nt = self.viewer_type(&new_item);
                    let cid = self.find_category_uuid_for_type(FolderType::from_asset_type(nt));
                    new_item.item.object.parent = cid;
                    if self.cats.contains_key(&cid) {
                        o = "added_null_parent_to_default_folder_server_move";
                        self.account_for_update(&cid, 1, true);
                    } else {
                        o = "added_null_parent_no_default_folder";
                    }
                } else if item.item.object.parent == CATEGORIZE_LOST_AND_FOUND_ID {
                    let pid = self.find_category_uuid_for_type(FolderType::LOST_AND_FOUND);
                    new_item.item.object.parent = pid;
                    self.account_for_update(&pid, 1, true);
                    o = if self.cats.contains_key(&pid) {
                        "added_categorize_lost_and_found"
                    } else {
                        "added_categorize_lost_and_found_no_lost_and_found"
                    };
                } else if !self.cats.contains_key(&item.item.object.parent) {
                    let pid = self.find_category_uuid_for_type(FolderType::LOST_AND_FOUND);
                    new_item.item.object.parent = pid;
                    if self.cats.contains_key(&pid) {
                        o = "added_unknown_parent_to_lost_and_found_server_move";
                        self.account_for_update(&pid, 1, true);
                    } else {
                        o = "added_unknown_parent_no_lost_and_found";
                    }
                }
                self.items.insert(iid, new_item);
                mask |= ADD;
                outcome = o;
            }
            if outcome != "refused_bad_type" && outcome != "skipped_type" {
                if ty == AssetType::CALLINGCARD {
                    mask |= CALLING_CARD;
                } else if ty == AssetType::GESTURE {
                    mask |= GESTURE;
                }
                self.add_changed_mask(mask, &iid);
            }
        }
        self.internal -= 1;
        if record_it {
            set_key(&mut self.ops[op_index], "model", s(outcome));
        }
    }

    fn on_object_deleted_from_server(
        &mut self,
        oid: &Uuid,
        fix_broken_links: bool,
        update_parent_version: bool,
        notify: bool,
    ) {
        let is_cat = self.cats.contains_key(oid);
        let is_item = self.items.contains_key(oid);
        if self.recording() {
            let op = map(vec![
                ("op", s("onObjectDeletedFromServer")),
                ("id", id(oid)),
                ("fix_broken_links", b(fix_broken_links)),
                ("update_parent_version", b(update_parent_version)),
                ("do_notify_observers", b(notify)),
                ("exists", s(if is_cat { "category" } else if is_item { "item" } else { "none" })),
            ]);
            self.record(op, true);
        }
        if !is_cat && !is_item {
            return;
        }
        self.internal += 1;
        let parent =
            if is_cat { self.cats[oid].rec.category.object.parent } else { self.items[oid].item.object.parent };
        if is_cat {
            self.remove_descendents(oid);
        }
        if update_parent_version {
            self.account_for_update(&parent, -1, true);
        }
        self.add_changed_mask(REMOVE, oid);
        self.notify_observers();
        self.erase_cat(oid);
        self.items.remove(oid);
        self.add_changed_mask(REMOVE, oid);
        if notify {
            self.notify_observers();
        }
        self.internal -= 1;
    }

    fn fetch_descendents_of(&mut self, folder: &Uuid) -> bool {
        let record_it = self.recording();
        let op_index = self.ops.len();
        if record_it {
            self.record(map(vec![("op", s("fetchDescendentsOf")), ("id", id(folder))]), true);
        }
        self.internal += 1;
        let mut rv = false;
        if *folder != NULL_ID && self.cats.contains_key(folder) {
            rv = self.model_fetch(folder, 10);
        }
        self.internal -= 1;
        if record_it {
            set_key(&mut self.ops[op_index], "would_start", b(rv));
        }
        rv
    }

    /// `LLInventoryModel::accountForUpdate`, the frozen copy.
    fn account_for_update(&mut self, cid: &Uuid, delta: i32, change_version: bool) {
        let record_it = self.recording();
        let op_index = self.ops.len();
        if record_it {
            let op = map(vec![
                ("op", s("accountForUpdate")),
                ("category", id(cid)),
                ("delta", i(delta)),
                ("change_version", b(change_version)),
            ]);
            self.record(op, true);
        }
        self.internal += 1;
        let found = self.cats.contains_key(cid);
        if found {
            let version = self.cats[cid].rec.version;
            if version != VERSION_UNKNOWN {
                let server = self.cats[cid].rec.descendents;
                let mut actual = self.child_count(cid);
                if server == actual {
                    actual = actual.wrapping_add(delta);
                    let c = &mut self.cats.get_mut(cid).unwrap().rec;
                    c.descendents = actual;
                    if change_version {
                        c.version = version + 1;
                    }
                }
            }
        }
        self.internal -= 1;
        if record_it {
            let mut m = map(vec![("found", b(found))]);
            if found {
                let c = &self.cats[cid].rec;
                set_key(&mut m, "version", i(c.version));
                set_key(&mut m, "descendents", i(c.descendents));
            }
            set_key(&mut self.ops[op_index], "model", m);
        }
    }

    /// The frozen `updateParentOnServer(restamp)` of a parse object.
    fn update_parent_on_server(&mut self, item: &ItemRecord, restamp: bool) {
        let o = &item.item.object;
        if self.recording() {
            let op = map(vec![
                ("op", s("updateParentOnServer")),
                ("id", id(&o.id)),
                ("parent_id", id(&o.parent)),
                ("restamp", b(restamp)),
                ("is_model_object", b(false)),
            ]);
            self.record(op, true);
        }
        let (agent, session) = (self.agent_id, self.session_id);
        let body = self.messages.move_inventory_item(&agent, &session, restamp, &o.id, &o.parent);
        let op = map(vec![
            ("op", s("msg.send")),
            ("reliable", b(true)),
            ("name", s("MoveInventoryItem")),
            ("body", Value::Binary(body)),
        ]);
        self.record(op, true);
    }

    // ---- timers, checkTimeout, the yield, the shutdown ---------------------

    fn timer_start(&self, t: &mut Timer) {
        t.last = self.clock;
        t.expiry = 0;
    }

    fn timer_set(&self, t: &mut Timer, seconds: f32) {
        t.expiry = self.clock + ((f64::from(seconds) * 1_000_000.0) as f32) as u64;
    }

    /// The constructor's timer logic: the task timer, and the shared batch
    /// timer when this is the frame's first update.
    fn construct_timers(&mut self, task: &mut Timer) {
        self.timer_start(task);
        self.timer_set(task, AIS_TASK_EXPIRY_SECONDS);
        let current = self.frame as u32;
        if self.batch_frame != current {
            let mut bt = self.batch;
            self.timer_start(&mut bt);
            self.timer_set(&mut bt, AIS_BATCH_EXPIRY_SECONDS);
            self.batch = bt;
            self.batch_frame = current;
        }
    }

    /// The task timer's `hasExpired()`, which is also the checkpoint.
    fn task_has_expired(&mut self, task: &Timer) -> bool {
        if self.nest == 0 {
            if self.recheck_pending {
                self.recheck_pending = false;
            } else if self.in_apply {
                self.apply_checkpoints += 1;
                let n = self.apply_checkpoints;
                self.fire_hooks("apply_checkpoint", n);
                if self.force_expiry {
                    self.force_expiry = false;
                    return true;
                }
            } else {
                self.parse_checkpoints += 1;
                if self.mode == Mode::Sched {
                    let n = self.parse_checkpoints;
                    self.fire_hooks("parse_checkpoint", n);
                    if self.force_expiry {
                        self.force_expiry = false;
                        return true;
                    }
                }
            }
        }
        if self.mode == Mode::Never {
            return false;
        }
        self.clock >= task.expiry
    }

    fn batch_has_expired(&self) -> bool {
        self.mode == Mode::Sched && self.clock >= self.batch.expiry
    }

    /// `AISUpdate::checkTimeout()` (bug 51: loops after resuming).
    fn check_timeout(&mut self, task: &mut Timer) -> Result<(), Stopped> {
        loop {
            let expired = self.task_has_expired(task) || self.batch_has_expired();
            if !expired {
                return Ok(());
            }
            self.suspend_until_next_frame();
            self.check_stop()?;
            self.timer_set(task, AIS_TASK_EXPIRY_SECONDS);
            let current = self.frame as u32;
            if self.batch_frame != current {
                let mut bt = self.batch;
                self.timer_set(&mut bt, AIS_BATCH_EXPIRY_SECONDS);
                self.batch = bt;
                self.batch_frame = current;
            }
        }
    }

    /// `llcoro::suspendUntilNextFrame()`.
    fn suspend_until_next_frame(&mut self) {
        let k = self.yields;
        let other = if self.others_us.is_empty() { 0 } else { self.others_us[k % self.others_us.len()] };
        let extra = self.yield_extra_us.get(k).copied().unwrap_or(0);
        let mut op = map(vec![
            ("op", s("yield")),
            ("frame", i(self.rel_frame())),
            ("clock_us", i(self.rel_clock() as i32)),
            ("others_us", i(other as i32)),
        ]);
        if extra != 0 {
            set_key(&mut op, "extra_us", i(extra as i32));
        }
        self.yields += 1;
        self.record(op, false);
        self.frame += 1;
        self.clock += self.frame_us;
        if other > 0 {
            // Another AIS update resumed first in this frame.
            let mut bt = self.batch;
            self.timer_set(&mut bt, AIS_BATCH_EXPIRY_SECONDS);
            self.batch = bt;
            self.batch_frame = self.frame as u32;
            self.clock += other;
        }
        self.clock += extra;
        if self.nest == 0 {
            self.recheck_pending = true;
            self.step_yields += 1;
            if self.mode == Mode::Sched {
                let n = self.step_yields;
                self.fire_hooks("yield", n);
            }
        }
    }

    fn check_stop(&mut self) -> Result<(), Stopped> {
        if self.stop_armed && self.nest == 0 {
            self.stop_armed = false;
            self.record(map(vec![("op", s("checkStop")), ("throws", s("LLCoros::Stopping"))]), false);
            self.stop_dump = to_binary(&self.skeleton_dump());
            self.stop_op_index = self.ops.len();
            return Err(Stopped);
        }
        Ok(())
    }

    // ---- hooks -------------------------------------------------------------

    fn fire_hooks(&mut self, at: &str, index: i32) {
        if self.nest != 0 {
            return;
        }
        let hooks = self.hooks.clone();
        for h in &hooks {
            if has(h, at) && as_integer(get(h, at)) == index {
                self.run_actions(h, at, index);
            }
        }
    }

    fn run_actions(&mut self, hook: &Value, at: &str, index: i32) {
        let outer = self.phase;
        self.phase = Phase::Harness;
        self.record(map(vec![("op", s("hook")), ("at", s(at)), ("index", i(index))]), false);
        let Value::Array(actions) = get(hook, "actions") else { panic!("hook without actions") };
        for a in actions {
            if has(a, "set_category") {
                let m = get(a, "set_category");
                if let Some(c) = self.cats.get_mut(&as_uuid(get(m, "category_id"))) {
                    let c = &mut c.rec;
                    if has(m, "version") {
                        c.version = as_integer(get(m, "version"));
                    }
                    if has(m, "descendents") {
                        c.descendents = as_integer(get(m, "descendents"));
                    }
                    if has(m, "name") {
                        c.category.object.rename(&as_string(get(m, "name")));
                    }
                    if has(m, "parent_id") {
                        c.category.object.parent = as_uuid(get(m, "parent_id"));
                    }
                }
            } else if has(a, "set_item") {
                let m = get(a, "set_item");
                if let Some(it) = self.items.get_mut(&as_uuid(get(m, "item_id"))) {
                    if has(m, "name") {
                        it.item.rename(&as_string(get(m, "name")));
                    }
                    if has(m, "desc") {
                        it.item.set_description(&as_string(get(m, "desc")));
                    }
                    if has(m, "parent_id") {
                        it.item.object.parent = as_uuid(get(m, "parent_id"));
                    }
                    if has(m, "flags") {
                        it.item.flags = as_integer(get(m, "flags")) as u32;
                    }
                    if has(m, "transaction_id") {
                        it.transaction_id = as_uuid(get(m, "transaction_id"));
                    }
                }
            } else if has(a, "delete") {
                let d = as_uuid(get(a, "delete"));
                if self.cats.contains_key(&d) {
                    self.remove_descendents(&d);
                }
                self.erase_cat(&d);
                self.items.remove(&d);
            } else if has(a, "run_step") {
                self.nest += 1;
                let step = get(a, "run_step").clone();
                let result = get(a, "result").clone();
                self.run_step(&step, &result);
                self.nest -= 1;
            } else if has(a, "construct_update") {
                // Another update constructed now: its constructor's timers.
                let mut other = Timer::default();
                self.construct_timers(&mut other);
            } else if has(a, "stop") {
                self.stop_armed = true;
                if at != "yield" {
                    self.force_expiry = true;
                }
            } else if has(a, "advance_us") && self.mode == Mode::Sched {
                self.clock += as_integer(get(a, "advance_us")) as u64;
            }
        }
        self.record(map(vec![("op", s("hook_end"))]), false);
        self.phase = outer;
    }

    // ---- steps -------------------------------------------------------------

    fn callback(&mut self, cb: &Uuid) {
        self.record(map(vec![("op", s("callback")), ("id", id(cb))]), true);
    }

    /// One step (`run_step` of the stub file).
    pub fn run_step(&mut self, step: &Value, result: &Value) {
        if self.nest == 0 {
            self.hooks = match get(step, "hooks") {
                Value::Array(h) => h.clone(),
                _ => Vec::new(),
            };
            self.in_apply = false;
            self.parse_checkpoints = 0;
            self.apply_checkpoints = 0;
            self.recheck_pending = false;
            self.force_expiry = false;
            self.step_yields = 0;
            self.stop_armed = false;
        }
        let r = if has(step, "call") {
            self.call_command(step, result)
        } else {
            let command = CommandType::from_name(&String::from_utf8_lossy(&as_string(get(step, "type"))))
                .expect("a command type");
            let with_callback = if has(step, "callback") { as_boolean(get(step, "callback")) } else { true };
            self.coroutine(
                command,
                &as_uuid(get(step, "target_id")),
                get(step, "body"),
                with_callback,
                Invoke::Result(result.clone()),
            )
        };
        if r.is_err() {
            let after = (self.ops.len() - self.stop_op_index) as i32;
            let unchanged = to_binary(&self.skeleton_dump()) == self.stop_dump;
            let op = map(vec![
                ("op", s("abort")),
                ("what", s("viewer is stopping (scripted)")),
                ("ops_after_stop", i(after)),
                ("model_unchanged", b(unchanged)),
            ]);
            self.record(op, false);
        }
        if self.nest == 0 {
            self.hooks.clear();
            self.in_apply = false;
            self.stop_armed = false;
            self.force_expiry = false;
            self.recheck_pending = false;
        }
    }

    /// `InvokeAISCommandCoro` with its invoke function.
    fn coroutine(
        &mut self,
        command: CommandType,
        target_id: &Uuid,
        body: &Value,
        with_callback: bool,
        invoke: Invoke,
    ) -> Result<(), Stopped> {
        let outer = self.phase;
        let r = self.coroutine_body(command, target_id, body, with_callback, invoke);
        self.phase = outer;
        r
    }

    fn coroutine_body(
        &mut self,
        command: CommandType,
        target_id: &Uuid,
        body: &Value,
        with_callback: bool,
        invoke: Invoke,
    ) -> Result<(), Stopped> {
        if self.disconnected {
            self.phase = Phase::Callbacks;
            if with_callback {
                self.callback(&NULL_ID);
            }
            return Ok(());
        }
        let mut result = match invoke {
            Invoke::Result(r) => r,
            Invoke::Http(req) => self.http_call(&req),
        };
        let status = begin(&mut result);
        let ctx = CommandContext {
            command,
            target_id: *target_id,
            body: body.clone(),
            agent_id: self.agent_id,
            in_open_sim: false,
        };
        let mut alert = self.alert;
        self.phase = Phase::ErrorBranch;
        {
            let mut task = Timer::default();
            let mut host = Host { h: self, task: &mut task, stopped: false };
            error_branch(&ctx, &result, status, &mut alert, &mut host);
        }
        self.alert = alert;

        // onUpdateReceived: AISUpdate(result, type, body), then doUpdate().
        let mut task = Timer::default();
        let pctx = ParseContext { command, agent_id: self.agent_id, in_open_sim: false };
        self.phase = Phase::Parse;
        let parsed = {
            let mut host = Host { h: self, task: &mut task, stopped: false };
            let p = ParsedUpdate::parse(&result, &ctx.body, &pctx, &mut host);
            if host.stopped || p.stopped {
                return Err(Stopped);
            }
            p
        };
        self.phase = Phase::Apply;
        self.apply(&parsed, &mut task)?;

        self.phase = Phase::Callbacks;
        if with_callback {
            for cb in callback_ids(&result, command) {
                self.callback(&cb);
            }
        }
        Ok(())
    }

    /// The executor: `doUpdate()` over the crate's op list.
    fn apply(&mut self, parsed: &ParsedUpdate, task: &mut Timer) -> Result<(), Stopped> {
        if self.nest == 0 {
            self.in_apply = true;
        }
        // The objects the executor materializes, one per record (the
        // localisation dictionary is empty in the test).
        let mut items = parsed.items.clone();
        let mut cats = parsed.categories.clone();
        for op in parsed.apply_ops() {
            let kind = match &op {
                ApplyOp::Checkpoint => "Checkpoint",
                ApplyOp::AccountDescendents { .. } => "AccountDescendents",
                ApplyOp::CreateCategory { .. } => "CreateCategory",
                ApplyOp::FlushBacklog => "FlushBacklog",
                ApplyOp::UpdateCategory { .. } => "UpdateCategory",
                ApplyOp::MoveLostItems { .. } => "MoveLostItems",
                ApplyOp::CreateItem { .. } => "CreateItem",
                ApplyOp::UpdateItem { .. } => "UpdateItem",
                ApplyOp::DeleteObject { .. } => "DeleteObject",
                ApplyOp::VersionUpdate { .. } => "VersionUpdate",
                ApplyOp::NotifyObservers => "NotifyObservers",
            };
            *self.apply_op_counts.entry(kind).or_insert(0) += 1;
            match op {
                ApplyOp::Checkpoint => self.check_timeout(task)?,
                ApplyOp::AccountDescendents { category, delta } => {
                    if self.q_get_category(&category).is_some() {
                        self.account_for_update(&category, delta, false);
                    }
                }
                ApplyOp::CreateCategory { record } => self.update_category(&cats[record].clone(), 512),
                ApplyOp::FlushBacklog => {
                    if self.changed.len() > MAX_UPDATE_BACKLOG {
                        self.notify_observers();
                        self.check_timeout(task)?;
                    }
                }
                ApplyOp::UpdateCategory { record } => {
                    let cid = cats[record].category.object.id;
                    if let Some((_, curr)) = self.q_get_category(&cid) {
                        cats[record].version = self.model_get_version(&cid);
                        cats[record].descendents = curr.descendents;
                        self.update_category(&cats[record].clone(), 0);
                    }
                }
                ApplyOp::MoveLostItems { records } => {
                    let lost = self.find_category_uuid_for_type(FolderType::LOST_AND_FOUND);
                    if lost != NULL_ID {
                        for r in records {
                            items[r].item.object.parent = lost;
                            let it = items[r].clone();
                            self.update_parent_on_server(&it, false);
                        }
                    }
                }
                ApplyOp::CreateItem { record } => self.update_item(&items[record].clone(), 512),
                ApplyOp::UpdateItem { record } => self.update_item(&items[record].clone(), 0),
                ApplyOp::DeleteObject { id } => self.on_object_deleted_from_server(&id, false, false, false),
                ApplyOp::VersionUpdate { category, version } => {
                    if self.q_get_category(&category).is_some() && self.model_get_version(&category) != version {
                        if version != VERSION_UNKNOWN {
                            if self.model_get_version(&category) == VERSION_UNKNOWN
                                || version > self.model_get_version(&category)
                            {
                                self.model_set_version(&category, version);
                            }
                        } else {
                            self.model_fetch(&category, 360);
                        }
                    }
                }
                ApplyOp::NotifyObservers => self.notify_observers(),
            }
        }
        Ok(())
    }

    // ---- the command functions ----------------------------------------------

    fn get_capability(&mut self, name: &str) -> String {
        let url = match name {
            "InventoryAPIv3" => self.inv_cap.clone(),
            "LibraryAPIv3" => self.lib_cap.clone(),
            _ => String::new(),
        };
        self.record(map(vec![("op", s("q.getCapability")), ("name", s(name)), ("result", s(&url))]), true);
        url
    }

    fn count_pending(&mut self) -> i32 {
        let p = self.pending;
        self.record(map(vec![("op", s("q.countPending")), ("pool", s("AIS")), ("result", i(p))]), true);
        p
    }

    fn record_postponed(&mut self) {
        let op = map(vec![
            ("op", s("postponed")),
            ("count", i(self.postponed.len() as i32)),
            ("idle_registered", b(self.idle_registered)),
        ]);
        self.record(op, false);
    }

    /// One call of an `AISAPI` command function (or `isAvailable`,
    /// `getCapNames`).
    fn call_command(&mut self, step: &Value, result: &Value) -> Result<(), Stopped> {
        self.phase = Phase::Command;
        let r = self.call_command_body(step, result);
        self.phase = Phase::Harness;
        r
    }

    fn call_command_body(&mut self, step: &Value, result: &Value) -> Result<(), Stopped> {
        let caps = get(step, "caps");
        self.has_region = !as_boolean(get(step, "no_region"));
        let cap = |k: &str| {
            if has(caps, k) { String::from_utf8(as_string(get(caps, k))).unwrap() } else { String::new() }
        };
        self.inv_cap = cap("inv");
        self.lib_cap = cap("lib");
        self.pending = as_integer(get(step, "pending"));
        self.next_result = result.clone();
        let args = get(step, "args");
        let name = String::from_utf8(as_string(get(step, "call"))).unwrap();
        let with_callback = if has(step, "callback") { as_boolean(get(step, "callback")) } else { true };
        let uid = |k: &str| as_uuid(get(args, k));
        let library = String::from_utf8_lossy(&as_string(get(args, "item_type"))) == "LIBRARY";
        let recursive = as_boolean(get(args, "recursive"));
        let depth = as_integer(get(args, "depth"));
        let command = match name.as_str() {
            "isAvailable" => {
                // FSUseAis3Api is true in the test.
                let r = self.has_region && !self.get_capability("InventoryAPIv3").is_empty();
                self.record(map(vec![("op", s("isAvailable")), ("result", b(r))]), true);
                None
            }
            "getCapNames" => {
                let names = Value::Array(vec![s("InventoryAPIv3"), s("LibraryAPIv3")]);
                self.record(map(vec![("op", s("getCapNames")), ("result", names)]), true);
                None
            }
            "CreateInventory" => Some(Command::CreateInventory {
                parent_id: uid("parent_id"),
                new_inventory: get(args, "new_inventory").clone(),
            }),
            "SlamFolder" => {
                let new_inventory = get(args, "new_inventory").clone();
                Some(Command::SlamFolder { folder_id: uid("folder_id"), new_inventory })
            }
            "RemoveCategory" => Some(Command::RemoveCategory { category_id: uid("category_id") }),
            "RemoveItem" => Some(Command::RemoveItem { item_id: uid("item_id") }),
            "PurgeDescendents" => Some(Command::PurgeDescendents { category_id: uid("category_id") }),
            "UpdateCategory" => {
                Some(Command::UpdateCategory { category_id: uid("category_id"), updates: get(args, "updates").clone() })
            }
            "UpdateItem" => {
                Some(Command::UpdateItem { item_id: uid("item_id"), updates: get(args, "updates").clone() })
            }
            "FetchItem" => Some(Command::FetchItem { item_id: uid("item_id"), library }),
            "FetchCategoryChildren" => {
                Some(Command::FetchCategoryChildren { category_id: uid("category_id"), library, recursive, depth })
            }
            "FetchCategoryChildrenByName" => Some(Command::FetchCategoryChildrenByName {
                identifier: String::from_utf8(as_string(get(args, "identifier"))).unwrap(),
                recursive,
                depth,
            }),
            "FetchCategoryCategories" => {
                Some(Command::FetchCategoryCategories { category_id: uid("category_id"), library, recursive, depth })
            }
            "FetchCategorySubset" => {
                let children = match get(args, "children") {
                    Value::Array(a) => a.iter().map(as_uuid).collect(),
                    _ => Vec::new(),
                };
                let category_id = uid("category_id");
                Some(Command::FetchCategorySubset { category_id, children, library, recursive, depth })
            }
            "FetchCOF" => Some(Command::FetchCof),
            "FetchCategoryLinks" => Some(Command::FetchCategoryLinks { category_id: uid("category_id") }),
            "FetchOrphans" => Some(Command::FetchOrphans),
            "CopyLibraryCategory" => Some(Command::CopyLibraryCategory {
                source_id: uid("source_id"),
                dest_id: uid("dest_id"),
                copy_subfolders: as_boolean(get(args, "copy_subfolders")),
            }),
            other => panic!("unknown AISAPI call {other}"),
        };
        let mut r = Ok(());
        if let Some(command) = command {
            // getInvCap() / getLibCap(): empty without a region.
            let cap = if self.has_region { self.get_capability(command.cap().cap_name()) } else { String::new() };
            let tid = self.generate_tid();
            match command.build(&cap, &tid) {
                Err(_) => {
                    self.phase = Phase::Callbacks;
                    if with_callback {
                        self.callback(&NULL_ID);
                    }
                    self.phase = Phase::Command;
                }
                Ok(req) => r = self.enqueue_ais_command(req, with_callback),
            }
        }
        self.record_postponed();
        if as_boolean(get(step, "idle")) {
            self.pending = as_integer(get(step, "idle_pending"));
            self.record(map(vec![("op", s("idle"))]), false);
            if self.idle_registered {
                r = r.and(self.on_idle());
            }
            self.record_postponed();
        }
        r
    }

    /// `LLUUID::generate()`: a well-formed, non-null id never used before.
    fn generate_tid(&mut self) -> Uuid {
        self.tid_counter += 1;
        let mut u = [0u8; 16];
        u[0] = 0x7d;
        u[6] = 0x40;
        u[8] = 0x80;
        u[8..16].copy_from_slice(&(0x8000_0000_0000_0000u64 | self.tid_counter).to_be_bytes());
        u
    }

    /// `EnqueueAISCommand`.
    fn enqueue_ais_command(&mut self, req: Request, with_callback: bool) -> Result<(), Stopped> {
        let pending = self.count_pending();
        let name = format!("AIS({})", req.proc_name);
        if pending < MAX_SIMULTANEOUS_COROUTINES {
            self.enqueue_coprocedure(&name, req, with_callback)
        } else {
            if self.postponed.is_empty() {
                self.idle_registered = true;
            }
            self.postponed.push_back(Postponed { name, req, with_callback });
            Ok(())
        }
    }

    /// `AISAPI::onIdle`.
    fn on_idle(&mut self) -> Result<(), Stopped> {
        let mut r = Ok(());
        if !self.postponed.is_empty() {
            let mut pending = self.count_pending();
            while pending < MAX_SIMULTANEOUS_COROUTINES && !self.postponed.is_empty() {
                let p = self.postponed.pop_front().unwrap();
                r = r.and(self.enqueue_coprocedure(&p.name, p.req, p.with_callback));
                pending += 1;
            }
        }
        if self.postponed.is_empty() {
            self.idle_registered = false;
        }
        r
    }

    /// The recording pool's `enqueueCoprocedure`: recorded, run at once.
    fn enqueue_coprocedure(&mut self, name: &str, req: Request, with_callback: bool) -> Result<(), Stopped> {
        self.record(map(vec![("op", s("enqueue")), ("pool", s("AIS")), ("name", s(name))]), true);
        let (command, target, body) = (req.command, req.target_id, req.body.clone());
        self.coroutine(command, &target, &body, with_callback, Invoke::Http(req))
    }

    /// The recording adapter: the request, and the step's result.
    fn http_call(&mut self, req: &Request) -> Value {
        let mut url = req.url.clone();
        let mut op = map(vec![("op", s("http")), ("method", s(req.method.name()))]);
        if let Some(p) = url.find("?tid=") {
            let tid: String = url[p + 5..].chars().take(36).collect();
            let ok = tid.len() == 36 && tid.as_bytes()[8] == b'-' && tid != "00000000-0000-0000-0000-000000000000"
                && self.tids.insert(tid.clone());
            set_key(&mut op, "tid_ok", b(ok));
            if !ok {
                self.tid_failures.push(format!("case {}: tid '{tid}' is malformed, null or reused", self.case_id));
                set_key(&mut op, "tid", s(&tid));
            }
            url.replace_range(p + 5..p + 5 + tid.len(), "<tid>");
        }
        set_key(&mut op, "url", s(&url));
        if let Some(body) = &req.http_body {
            debug_assert!(matches!(req.method, Method::Post | Method::Put | Method::Patch));
            set_key(&mut op, "body", body.clone());
        }
        if let Some(d) = &req.destination {
            set_key(&mut op, "destination", s(d));
        }
        set_key(&mut op, "headers", s("none"));
        set_key(&mut op, "timeout", i(180));
        self.record(op, true);
        let r = self.next_result.clone();
        if matches!(r, Value::Map(_)) && has(&r, "http_result") {
            return r;
        }
        map(vec![(
            "http_result",
            map(vec![
                ("success", b(true)),
                ("type", i(200)),
                ("status", i(0)),
                ("message", s("")),
                ("url", s(&url)),
                ("headers", Value::map()),
            ]),
        )])
    }
}

/// What the coroutine's invoke function returns.
enum Invoke {
    /// A scripted result (a step through the coroutine).
    Result(Value),
    /// A request on the recording adapter (a command function's procedure).
    Http(Request),
}

/// The crate's view of the model: the recording stub, plus the update's task
/// timer for its `checkTimeout()` and constructor.
struct Host<'a> {
    h: &'a mut Harness,
    task: &'a mut Timer,
    stopped: bool,
}

impl Model for Host<'_> {
    fn get_category(&mut self, cid: &Uuid) -> Option<ModelCategory> {
        self.h.q_get_category(cid).map(|(serial, record)| ModelCategory { handle: ModelRef(serial), record })
    }

    fn get_item(&mut self, iid: &Uuid) -> Option<ItemRecord> {
        self.h.q_get_item(iid)
    }

    fn get_version(&mut self, cat: ModelRef) -> i32 {
        let cid = self.h.cat_by_serial(cat.0);
        self.h.model_get_version(&cid)
    }

    fn set_version(&mut self, cat: ModelRef, version: i32) {
        let cid = self.h.cat_by_serial(cat.0);
        self.h.model_set_version(&cid, version);
    }

    fn fetch(&mut self, cat: ModelRef, expiry_seconds: i32) -> bool {
        let cid = self.h.cat_by_serial(cat.0);
        self.h.model_fetch(&cid, expiry_seconds)
    }

    fn fetch_descendents_of(&mut self, folder_id: &Uuid) -> bool {
        self.h.fetch_descendents_of(folder_id)
    }

    fn on_object_deleted_from_server(&mut self, oid: &Uuid, fix: bool, update_parent: bool, notify: bool) {
        self.h.on_object_deleted_from_server(oid, fix, update_parent, notify);
    }

    fn notify(&mut self, name: &'static str) {
        self.h.record(map(vec![("op", s("notify")), ("name", s(name))]), true);
    }

    fn begin_update(&mut self) {
        self.h.construct_timers(self.task);
    }

    fn checkpoint(&mut self) -> Flow {
        match self.h.check_timeout(self.task) {
            Ok(()) => Flow::Continue,
            Err(Stopped) => {
                self.stopped = true;
                Flow::Stop
            }
        }
    }
}
