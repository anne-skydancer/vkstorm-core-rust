//! Rung seven, sub-rung R3a: AIS replies via `vkstorm-inventory`'s `ais`
//! module (`vks_inv_ais_*`): what the coroutine adapter makes of an AIS
//! response, the coroutine's error branch and callback ids, and
//! `AISUpdate`'s parse.
//!
//! How a reply travels:
//!
//! 1. The viewer's AIS-only adapter handler hands the raw response (status,
//!    message, URL, headers, body) to [`vks_inv_ais_reply_build`], which
//!    builds the result the C++ adapter would have posted (the `http_result`
//!    block, the 499 of an unparsable `application/llsd+xml` body, the depth
//!    cap) as a Rust value in a [`VksInvAisReply`]. No C++ `LLSD` tree is
//!    built; [`vks_inv_ais_reply_to_binary`] gives the value as binary LLSD
//!    for the C++ code that still wants one (logging, the oracle).
//! 2. The coroutine calls [`vks_inv_ais_error_branch`] (the status, the 410
//!    and 403 handling, the 403 alert flag as explicit state), then
//!    [`vks_inv_ais_parse`]: `AISUpdate`'s parse over the reply, querying the
//!    inventory model through the host table ([`VksInvAisHost`]) one call at
//!    a time, in the C++ order, and returning what `doUpdate()` applies (the
//!    parse's item and folder records and the member maps, by id) and the
//!    completion callback ids, in a [`VksInvAisUpdate`] block.
//! 3. C++ materializes one viewer object per record and runs `doUpdate()`,
//!    which stays C++ with its yields and its two apply-time reads.
//!
//! Host callbacks must not unwind into Rust: C++ catches its exceptions
//! (`LLCoros::Stopping` at a checkpoint among them), answers "not found" /
//! stop, and rethrows after the call. Strings a callback hands over borrow
//! the C++ object's and are copied before the next callback.
//!
//! Every call catches panics at the boundary: it returns null, an error
//! status or does nothing, with the text in `vks_inv_last_error`.

use std::ffi::{CStr, c_char, c_void};
use std::{ptr, slice};

use vkstorm_inventory::ais::reply::{self, Response};
use vkstorm_inventory::ais::result::{AlertState, HttpStatus, begin, callback_ids, error_branch};
use vkstorm_inventory::ais::update::{
    CategoryRecord, FetchTimer, Flow, ItemRecord, Model, ModelCategory, ModelRef, ParseContext,
    ParsedUpdate,
};
use vkstorm_inventory::ais::{CommandContext, CommandType};
use vkstorm_inventory::category::Category;
use vkstorm_inventory::llsd::coerce::get;
use vkstorm_inventory::perms::Uuid;
use vkstorm_inventory::types::FolderType;
use vkstorm_llsd::{Format, Limits, Options, Value};

use crate::inventory::{
    VksInvBytes, VksInvCategory, VksInvItem, bytes, guard, item_from, item_view, object_from,
    object_view, set_error,
};

// ---------------------------------------------------------------------------
// Replies

/// An HTTP status: `LLCore::HttpStatus`'s type and status, and its message
/// (borrowed on the way in; on the way out it points into the reply).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvAisStatus {
    pub kind: u16,
    pub status: i16,
    pub message: VksInvBytes,
}

/// A response header, borrowed.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvAisHeader {
    pub name: VksInvBytes,
    pub value: VksInvBytes,
}

/// An HTTP response as the adapter's handler sees it, borrowed for the call.
#[repr(C)]
pub struct VksInvAisResponse {
    pub status: VksInvAisStatus,
    pub url: VksInvBytes,
    pub headers: *const VksInvAisHeader,
    pub header_count: usize,
    /// Whether the response has a body buffer at all.
    pub has_body: bool,
    pub body: VksInvBytes,
    /// The LLSD depth limit (`AIS_MAX_REPLY_DEPTH` for AIS; -1 unlimited).
    pub max_depth: i32,
}

/// The result the coroutine receives, Rust-owned: read with the
/// `vks_inv_ais_reply_*` calls, freed with `vks_inv_ais_reply_free`.
pub struct VksInvAisReply {
    value: Value,
    status: HttpStatus,
    message: Vec<u8>,
}

/// Builds the result of a response (`HttpCoroHandler::onCompleted` with the
/// LLSD handler, minus its scheduling). Null when nothing is posted (the
/// reply to a canceled request, `HE_HANDLE_NOT_FOUND`) or on failure. Safe
/// on any thread.
///
/// # Safety
/// `r` must be readable, its byte ranges and header array valid for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_ais_reply_build(
    r: *const VksInvAisResponse,
) -> *mut VksInvAisReply {
    if r.is_null() {
        return ptr::null_mut();
    }
    guard(ptr::null_mut(), || {
        let r = unsafe { &*r };
        let headers: Vec<(&[u8], &[u8])> = if r.headers.is_null() || r.header_count == 0 {
            Vec::new()
        } else {
            unsafe { slice::from_raw_parts(r.headers, r.header_count) }
                .iter()
                .map(|h| unsafe { (h.name.get(), h.value.get()) })
                .collect()
        };
        let resp = Response {
            status: HttpStatus {
                kind: r.status.kind,
                status: r.status.status,
            },
            message: unsafe { r.status.message.get() },
            url: unsafe { r.url.get() },
            headers: &headers,
            body: if r.has_body {
                Some(unsafe { r.body.get() })
            } else {
                None
            },
            max_depth: r.max_depth,
        };
        match reply::adapter_result(&resp) {
            Some(rep) => Box::into_raw(Box::new(VksInvAisReply {
                value: rep.value,
                status: rep.status,
                message: rep.message,
            })),
            None => ptr::null_mut(),
        }
    })
}

/// A reply from the binary LLSD of a result the adapter produced as a C++
/// `LLSD` (`LLSDSerialize::toBinary`, which crosses without loss): the
/// coroutine's fallback when the adapter returned an immediate error without
/// calling its handler, and the R3a oracle's scripted results. Its status is
/// read from its `http_result` as the coroutine reads it. Null when the
/// bytes do not parse.
///
/// # Safety
/// `data` must be readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_ais_reply_from_binary(
    data: *const u8,
    len: usize,
) -> *mut VksInvAisReply {
    guard(ptr::null_mut(), || {
        let input = unsafe { bytes(data, len) };
        let (n, v) = vkstorm_llsd::parse(Format::Binary, input, Limits::for_len(input.len()));
        if n < 0 {
            set_error("vks_inv_ais_reply_from_binary: the bytes do not parse");
            return ptr::null_mut();
        }
        let value = v.unwrap_or(Value::Undefined);
        // The status as getStatusFromLLSD reads it, without changing the
        // value (the coroutine's own begin() does that).
        let mut probe = value.clone();
        let status = begin(&mut probe);
        let message = match get(get(&probe, "http_result"), "message") {
            Value::String(s) => s.clone(),
            _ => Vec::new(),
        };
        drop(probe);
        Box::into_raw(Box::new(VksInvAisReply {
            value,
            status,
            message,
        }))
    })
}

/// The reply's status after the adapter (and, after
/// `vks_inv_ais_error_branch`, the coroutine's): type, status, message
/// (pointing into the reply).
///
/// # Safety
/// `r` must be a live reply, `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_ais_reply_status(
    r: *const VksInvAisReply,
    out: *mut VksInvAisStatus,
) {
    if r.is_null() || out.is_null() {
        return;
    }
    let r = unsafe { &*r };
    unsafe {
        *out = VksInvAisStatus {
            kind: r.status.kind,
            status: r.status.status,
            message: VksInvBytes::of(&r.message),
        }
    };
}

/// Bytes owned by Rust, freed with `vks_inv_ais_bytes_free`.
pub struct VksInvAisBytesBlock {
    data: Vec<u8>,
    view: VksInvBytes,
}

/// The reply's value as binary LLSD (`LLSDSerialize::fromBinary` reads it
/// back exactly): for the C++ code that wants an `LLSD` tree, built only
/// when it does. Null on failure.
///
/// # Safety
/// `r` must be a live reply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_ais_reply_to_binary(
    r: *const VksInvAisReply,
) -> *mut VksInvAisBytesBlock {
    if r.is_null() {
        return ptr::null_mut();
    }
    guard(ptr::null_mut(), || {
        let r = unsafe { &*r };
        let (data, _) = vkstorm_llsd::format(Format::Binary, &r.value, Options::default());
        let mut b = Box::new(VksInvAisBytesBlock {
            data,
            view: VksInvBytes {
                ptr: ptr::null(),
                len: 0,
            },
        });
        b.view = VksInvBytes::of(&b.data);
        Box::into_raw(b)
    })
}

/// The bytes of a block.
///
/// # Safety
/// `b` must be a live block.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_ais_bytes_view(b: *const VksInvAisBytesBlock) -> VksInvBytes {
    if b.is_null() {
        VksInvBytes {
            ptr: ptr::null(),
            len: 0,
        }
    } else {
        unsafe { (*b).view }
    }
}

/// Releases a block. Null is ignored.
///
/// # Safety
/// `b` must come from a `vks_inv_ais_*` call and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_ais_bytes_free(b: *mut VksInvAisBytesBlock) {
    if !b.is_null() {
        drop(unsafe { Box::from_raw(b) });
    }
}

/// Releases a reply. Null is ignored. Safe on any thread.
///
/// # Safety
/// `r` must come from a `vks_inv_ais_reply_*` call and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_ais_reply_free(r: *mut VksInvAisReply) {
    if !r.is_null() {
        drop(unsafe { Box::from_raw(r) });
    }
}

// ---------------------------------------------------------------------------
// The model, through the host

/// `LLViewerInventoryCategory::mDescendentsRequested`, an `LLFrameTimer`:
/// carried from a model folder into the parse's copy and back.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VksInvAisFetchTimer {
    pub start_time: f64,
    pub expiry: f64,
    pub started: bool,
}

/// An `LLViewerInventoryItem`: the item record and the viewer fields.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvAisItem {
    pub item: VksInvItem,
    pub complete: bool,
    pub transaction_id: Uuid,
    pub running: bool,
    pub faulted: bool,
}

/// An `LLViewerInventoryCategory` as the parse uses it. `category.version`
/// is not read from the host (the parse asks `get_version`, as the C++ calls
/// `getVersion()`); on the way out it is the record's version.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvAisCategory {
    pub category: VksInvCategory,
    pub descendents: i32,
    /// Whether `requested` holds the folder's timer (a copy of a model
    /// folder); a new folder's is a fresh one (reset at construction).
    pub has_requested: bool,
    pub requested: VksInvAisFetchTimer,
}

/// The inventory model and the coroutine's other services, in C++. Each
/// function is one C++ call, made where and as often as the C++ makes it.
/// None may be null.
#[repr(C)]
pub struct VksInvAisHost {
    pub ctx: *mut c_void,
    /// `gInventory.getCategory(id)`: false when unknown; else fills `out`
    /// (strings borrowed until the next call) and a handle on the folder,
    /// valid for the parse.
    pub get_category:
        unsafe extern "C" fn(*mut c_void, *const u8, *mut VksInvAisCategory, *mut u64) -> bool,
    /// `gInventory.getItem(id)`: false when unknown; else fills `out`.
    pub get_item: unsafe extern "C" fn(*mut c_void, *const u8, *mut VksInvAisItem) -> bool,
    /// `getVersion()` on a model folder.
    pub get_version: unsafe extern "C" fn(*mut c_void, u64) -> i32,
    /// `setVersion(version)` on a model folder.
    pub set_version: unsafe extern "C" fn(*mut c_void, u64, i32),
    /// `fetch(expiry_seconds)` on a model folder.
    pub fetch: unsafe extern "C" fn(*mut c_void, u64, i32) -> bool,
    /// `gInventory.fetchDescendentsOf(folder_id)`.
    pub fetch_descendents_of: unsafe extern "C" fn(*mut c_void, *const u8) -> bool,
    /// `gInventory.onObjectDeletedFromServer(id, fix_broken_links,
    /// update_parent_version, do_notify_observers)`.
    pub on_object_deleted_from_server:
        unsafe extern "C" fn(*mut c_void, *const u8, bool, bool, bool),
    /// `LLNotificationsUtil::add(name)` (NUL-terminated, static).
    pub notify: unsafe extern "C" fn(*mut c_void, *const c_char),
    /// `checkTimeout()` at the start of each `parseEmbedded()`: true to go
    /// on, false when the coroutine is stopping (the parse ends there).
    pub checkpoint: unsafe extern "C" fn(*mut c_void) -> bool,
}

/// The coroutine's arguments as the reply's handling needs them.
#[repr(C)]
pub struct VksInvAisCall {
    /// `AISAPI::COMMAND_TYPE`.
    pub command: i32,
    /// `targetId`.
    pub target_id: Uuid,
    /// `gAgent.getID()`.
    pub agent_id: Uuid,
    /// `LLPermissions::getIsInOpenSim()`.
    pub in_open_sim: bool,
    /// `body.has("depth")` and `body["depth"].asInteger()`, read by C++ (the
    /// only part of the request body the decisions read).
    pub has_depth: bool,
    pub depth: i32,
}

fn timer_from(t: &VksInvAisFetchTimer) -> FetchTimer {
    FetchTimer {
        start_time: t.start_time,
        expiry: t.expiry,
        started: t.started,
    }
}

fn timer_view(t: &FetchTimer) -> VksInvAisFetchTimer {
    VksInvAisFetchTimer {
        start_time: t.start_time,
        expiry: t.expiry,
        started: t.started,
    }
}

unsafe fn item_record_from(v: &VksInvAisItem) -> ItemRecord {
    ItemRecord {
        item: unsafe { item_from(&v.item) },
        complete: v.complete,
        transaction_id: v.transaction_id,
        running: v.running,
        faulted: v.faulted,
    }
}

fn item_record_view(r: &ItemRecord) -> VksInvAisItem {
    VksInvAisItem {
        item: item_view(&r.item),
        complete: r.complete,
        transaction_id: r.transaction_id,
        running: r.running,
        faulted: r.faulted,
    }
}

unsafe fn category_record_from(v: &VksInvAisCategory) -> CategoryRecord {
    CategoryRecord {
        category: Category {
            object: unsafe { object_from(&v.category.object) },
            preferred_type: FolderType(v.category.preferred_type),
        },
        owner: v.category.owner,
        version: v.category.version,
        descendents: v.descendents,
        requested: v.has_requested.then(|| timer_from(&v.requested)),
    }
}

fn category_record_view(r: &CategoryRecord) -> VksInvAisCategory {
    VksInvAisCategory {
        category: VksInvCategory {
            object: object_view(&r.category.object),
            preferred_type: r.category.preferred_type.0,
            owner: r.owner,
            version: r.version,
        },
        descendents: r.descendents,
        has_requested: r.requested.is_some(),
        requested: r.requested.as_ref().map(timer_view).unwrap_or_default(),
    }
}

fn empty_item() -> VksInvAisItem {
    item_record_view(&ItemRecord::default())
}

fn empty_category() -> VksInvAisCategory {
    category_record_view(&CategoryRecord::new_owned(&[0; 16]))
}

struct Host<'h>(&'h VksInvAisHost);

impl Model for Host<'_> {
    fn get_category(&mut self, id: &Uuid) -> Option<ModelCategory> {
        let mut out = empty_category();
        let mut handle = 0u64;
        let found =
            unsafe { (self.0.get_category)(self.0.ctx, id.as_ptr(), &mut out, &mut handle) };
        found.then(|| ModelCategory {
            handle: ModelRef(handle),
            record: unsafe { category_record_from(&out) },
        })
    }
    fn get_item(&mut self, id: &Uuid) -> Option<ItemRecord> {
        let mut out = empty_item();
        let found = unsafe { (self.0.get_item)(self.0.ctx, id.as_ptr(), &mut out) };
        found.then(|| unsafe { item_record_from(&out) })
    }
    fn get_version(&mut self, cat: ModelRef) -> i32 {
        unsafe { (self.0.get_version)(self.0.ctx, cat.0) }
    }
    fn set_version(&mut self, cat: ModelRef, version: i32) {
        unsafe { (self.0.set_version)(self.0.ctx, cat.0, version) }
    }
    fn fetch(&mut self, cat: ModelRef, expiry_seconds: i32) -> bool {
        unsafe { (self.0.fetch)(self.0.ctx, cat.0, expiry_seconds) }
    }
    fn fetch_descendents_of(&mut self, folder_id: &Uuid) -> bool {
        unsafe { (self.0.fetch_descendents_of)(self.0.ctx, folder_id.as_ptr()) }
    }
    fn on_object_deleted_from_server(&mut self, id: &Uuid, fix: bool, update: bool, notify: bool) {
        unsafe {
            (self.0.on_object_deleted_from_server)(self.0.ctx, id.as_ptr(), fix, update, notify)
        }
    }
    fn notify(&mut self, name: &'static str) {
        // The names are literals; a NUL-terminated copy for C.
        let mut z = name.as_bytes().to_vec();
        z.push(0);
        let c = CStr::from_bytes_with_nul(&z).unwrap_or_default();
        unsafe { (self.0.notify)(self.0.ctx, c.as_ptr()) }
    }
    fn checkpoint(&mut self) -> Flow {
        if unsafe { (self.0.checkpoint)(self.0.ctx) } {
            Flow::Continue
        } else {
            Flow::Stop
        }
    }
}

unsafe fn context(call: &VksInvAisCall) -> Option<CommandContext> {
    let command = CommandType::from_i32(call.command)?;
    let body = if call.has_depth {
        let mut b = Value::map();
        if let Value::Map(m) = &mut b {
            m.insert(b"depth".to_vec(), Value::Integer(call.depth));
        }
        b
    } else {
        Value::Undefined
    };
    Some(CommandContext {
        command,
        target_id: call.target_id,
        body,
        agent_id: call.agent_id,
        in_open_sim: call.in_open_sim,
    })
}

/// The 403 alert flag of the coroutine (`static bool first_call`), kept by
/// C++ and passed in and out.
#[repr(C)]
pub struct VksInvAisAlert {
    pub first_call: bool,
}

/// The coroutine after the request, up to the update: `result[HTTP_RESULTS]`
/// on the result (a value that is not a map becomes one) and the status read
/// from it, then the error branch (410 on a removal: refetch the parent, and
/// for an item delete it; 403 at depth 0: the alert the first time, then the
/// plain notification) through the host. Returns the status (the reply's
/// status is set to it); on failure (a null argument, an unknown command,
/// a panic) the 500 of a malformed result.
///
/// # Safety
/// `r` must be a live reply; `call`, `alert` and `host` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_ais_error_branch(
    r: *mut VksInvAisReply,
    call: *const VksInvAisCall,
    alert: *mut VksInvAisAlert,
    host: *const VksInvAisHost,
) -> VksInvAisStatus {
    let failed = VksInvAisStatus {
        kind: 500,
        status: 1,
        message: VksInvBytes {
            ptr: ptr::null(),
            len: 0,
        },
    };
    if r.is_null() || call.is_null() || alert.is_null() || host.is_null() {
        return failed;
    }
    guard(failed, || {
        let r = unsafe { &mut *r };
        let Some(ctx) = (unsafe { context(&*call) }) else {
            set_error("vks_inv_ais_error_branch: unknown command type");
            return failed;
        };
        let mut model = Host(unsafe { &*host });
        let mut state = AlertState {
            first_call: unsafe { (*alert).first_call },
        };
        let status = begin(&mut r.value);
        let status = error_branch(&ctx, &r.value, status, &mut state, &mut model);
        unsafe { (*alert).first_call = state.first_call };
        r.status = status;
        VksInvAisStatus {
            kind: status.kind,
            status: status.status,
            message: VksInvBytes::of(&r.message),
        }
    })
}

/// One entry of an update's lists: an id and a value (a record index, a
/// descendent delta or a version; 0 where the list has none).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvAisEntry {
    pub id: Uuid,
    pub value: i32,
}

/// A list of entries, pointing into the update block.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvAisList {
    pub entries: *const VksInvAisEntry,
    pub count: usize,
}

/// What `AISUpdate`'s parse produced: the records (one viewer object each)
/// and its member maps in id order, and the callback ids. Every pointer
/// points into the block and lives until `vks_inv_ais_update_free`.
#[repr(C)]
pub struct VksInvAisUpdateView {
    /// The host stopped the parse at a checkpoint: apply nothing, call
    /// nothing back.
    pub stopped: bool,
    pub items: *const VksInvAisItem,
    pub item_count: usize,
    pub categories: *const VksInvAisCategory,
    pub category_count: usize,
    /// `mCatDescendentDeltas` (delta, signed).
    pub descendent_deltas: VksInvAisList,
    /// `mCatVersionsUpdated` (version).
    pub versions_updated: VksInvAisList,
    /// `mItemsCreated`, `mItemsLost`, `mItemsUpdated` (item record index; a
    /// lost item is the same record as its created entry).
    pub items_created: VksInvAisList,
    pub items_lost: VksInvAisList,
    pub items_updated: VksInvAisList,
    /// `mCategoriesCreated`, `mCategoriesUpdated` (category record index).
    pub categories_created: VksInvAisList,
    pub categories_updated: VksInvAisList,
    /// `mObjectsDeletedIds`.
    pub objects_deleted: VksInvAisList,
    /// The completion callback ids, in order (never empty).
    pub callbacks: VksInvAisList,
}

/// The parse's results and their view.
pub struct VksInvAisUpdate {
    _items: Vec<ItemRecord>,
    _categories: Vec<CategoryRecord>,
    _item_views: Vec<VksInvAisItem>,
    _category_views: Vec<VksInvAisCategory>,
    _lists: Vec<Vec<VksInvAisEntry>>,
    view: VksInvAisUpdateView,
}

fn entries<V: Copy>(
    it: impl Iterator<Item = (Uuid, V)>,
    f: impl Fn(V) -> i32,
) -> Vec<VksInvAisEntry> {
    it.map(|(id, v)| VksInvAisEntry { id, value: f(v) })
        .collect()
}

fn list(v: &[VksInvAisEntry]) -> VksInvAisList {
    VksInvAisList {
        entries: v.as_ptr(),
        count: v.len(),
    }
}

/// `AISUpdate(result, type, request_body)`'s parse over the reply (after
/// `vks_inv_ais_error_branch`), querying the model through `host`, and the
/// completion callback ids. Null on failure.
///
/// # Safety
/// `r` must be a live reply; `call` and `host` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_ais_parse(
    r: *const VksInvAisReply,
    call: *const VksInvAisCall,
    host: *const VksInvAisHost,
) -> *mut VksInvAisUpdate {
    if r.is_null() || call.is_null() || host.is_null() {
        return ptr::null_mut();
    }
    guard(ptr::null_mut(), || {
        let r = unsafe { &*r };
        let Some(ctx) = (unsafe { context(&*call) }) else {
            set_error("vks_inv_ais_parse: unknown command type");
            return ptr::null_mut();
        };
        let mut model = Host(unsafe { &*host });
        let parse_ctx = ParseContext {
            command: ctx.command,
            agent_id: ctx.agent_id,
            in_open_sim: ctx.in_open_sim,
        };
        let p = ParsedUpdate::parse(&r.value, &ctx.body, &parse_ctx, &mut model);
        let callbacks = callback_ids(&r.value, ctx.command);
        let as_index = |i: usize| i as i32;
        let lists = vec![
            entries(p.descendent_deltas.iter().map(|(k, v)| (*k, *v)), |v| v),
            entries(p.versions_updated.iter().map(|(k, v)| (*k, *v)), |v| v),
            entries(p.items_created.iter().map(|(k, v)| (*k, *v)), as_index),
            entries(p.items_lost.iter().map(|(k, v)| (*k, *v)), as_index),
            entries(p.items_updated.iter().map(|(k, v)| (*k, *v)), as_index),
            entries(p.categories_created.iter().map(|(k, v)| (*k, *v)), as_index),
            entries(p.categories_updated.iter().map(|(k, v)| (*k, *v)), as_index),
            entries(p.objects_deleted.iter().map(|k| (*k, 0)), |v| v),
            entries(callbacks.iter().map(|k| (*k, 0)), |v| v),
        ];
        let item_views: Vec<VksInvAisItem> = p.items.iter().map(item_record_view).collect();
        let category_views: Vec<VksInvAisCategory> =
            p.categories.iter().map(category_record_view).collect();
        let view = VksInvAisUpdateView {
            stopped: p.stopped,
            items: item_views.as_ptr(),
            item_count: item_views.len(),
            categories: category_views.as_ptr(),
            category_count: category_views.len(),
            descendent_deltas: list(&lists[0]),
            versions_updated: list(&lists[1]),
            items_created: list(&lists[2]),
            items_lost: list(&lists[3]),
            items_updated: list(&lists[4]),
            categories_created: list(&lists[5]),
            categories_updated: list(&lists[6]),
            objects_deleted: list(&lists[7]),
            callbacks: list(&lists[8]),
        };
        // The views point into the records' strings and the lists' buffers,
        // which move into the block unchanged (moving a Vec keeps its heap
        // buffer).
        Box::into_raw(Box::new(VksInvAisUpdate {
            _items: p.items,
            _categories: p.categories,
            _item_views: item_views,
            _category_views: category_views,
            _lists: lists,
            view,
        }))
    })
}

/// The view of an update block.
///
/// # Safety
/// `u` must be a live update block.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_ais_update_view(
    u: *const VksInvAisUpdate,
) -> *const VksInvAisUpdateView {
    if u.is_null() {
        ptr::null()
    } else {
        unsafe { &(*u).view }
    }
}

/// Releases an update block. Null is ignored.
///
/// # Safety
/// `u` must come from `vks_inv_ais_parse` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_ais_update_free(u: *mut VksInvAisUpdate) {
    if !u.is_null() {
        drop(unsafe { Box::from_raw(u) });
    }
}

// Layout: checked here and, through the same numbers, by static asserts in
// the header (64-bit targets).
const _: () = {
    use std::mem::{offset_of, size_of};
    if size_of::<usize>() == 8 {
        assert!(size_of::<VksInvAisStatus>() == 24);
        assert!(size_of::<VksInvAisHeader>() == 32);
        assert!(size_of::<VksInvAisResponse>() == 88 && offset_of!(VksInvAisResponse, body) == 64);
        assert!(size_of::<VksInvAisFetchTimer>() == 24);
        assert!(
            size_of::<VksInvAisItem>() == 256 && offset_of!(VksInvAisItem, transaction_id) == 233
        );
        assert!(
            size_of::<VksInvAisCategory>() == 152
                && offset_of!(VksInvAisCategory, requested) == 128
        );
        assert!(size_of::<VksInvAisHost>() == 80);
        assert!(size_of::<VksInvAisCall>() == 44);
        assert!(size_of::<VksInvAisEntry>() == 20);
        assert!(size_of::<VksInvAisList>() == 16);
        assert!(size_of::<VksInvAisUpdateView>() == 184);
    }
};
