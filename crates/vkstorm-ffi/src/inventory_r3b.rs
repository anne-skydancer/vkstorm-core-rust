//! Rung seven, sub-rung R3b: the inventory background fetch's scheduler via
//! `vkstorm_inventory::fetch` (`vks_inv_fetch_*`), the decisions of
//! `LLInventoryModelBackgroundFetch`: the folder and item queues, the AIS
//! dispatch loop with its concurrency limit and frame budget, the AIS
//! completions, the legacy capability batches with their 403 split and
//! re-queue, and the public fetch state.
//!
//! The C++ keeps the model, the HTTP requests (`AISAPI`, `requestPost`) and
//! the legacy reply applier. It holds one [`VksInvFetch`] and calls it at
//! each C++ entry point; every effect on the outside comes back as an
//! operation list ([`VksInvFetchOps`], null when there is none), which the
//! C++ carries out in order and frees with `vks_inv_fetch_ops_free`. An
//! operation that calls back into the scheduler (an AIS completion, a legacy
//! handler) is a new call with its own list.
//!
//! The model is read through a host table ([`VksInvFetchHost`]): the C++
//! getters in the C++ order, `getDirectDescendentsOf`'s category array in
//! model order (the `std::unordered_map` order of the platform's standard
//! library), the category fetch state, and the frame budget's clock
//! (`LLTimer::getTotalSeconds()`, read where `bulkFetchViaAis` reads it).
//! The host functions must not call back into the scheduler.
//!
//! A caller's callback (`fetchCOF`, `fetchFolderAndLinks`) is
//! [`VKS_INV_FETCH_OP_CALLBACK`], always the last operation of its list: the
//! C++ runs it and then calls `vks_inv_fetch_resume`, which runs the rest of
//! the completion, as the C++ lambda runs `callback()` first.
//!
//! The environment of a call (settings, region, capabilities, session
//! state) is a [`VksInvFetchEnv`] the C++ fills; its strings are borrowed for
//! the call.
//!
//! Every call catches panics at the boundary: it returns null or 0 (or does
//! nothing) and leaves the text in `vks_inv_last_error`.

use std::slice;

use vkstorm_inventory::fetch::ais::AisFetch;
use vkstorm_inventory::fetch::scheduler::{
    Action, BackgroundFetch, BudgetClock, CategoryInfo, Ctx, Env, FetchModel, ItemInfo,
};
use vkstorm_inventory::fetch::{CategoryFetch, FetchType, QueueEntry, Uuid};

use crate::inventory::{VksInvBytes, guard};

// ---------------------------------------------------------------------------
// The host

/// `LLViewerInventoryCategory`'s fields the scheduler reads.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvFetchCategory {
    pub version: i32,
    /// `LLFolderType::EType`
    pub preferred_type: i32,
    pub owner_id: Uuid,
}

/// `LLViewerInventoryItem`'s fields the scheduler reads.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvFetchItem {
    /// `isFinished()` (`mIsComplete`)
    pub complete: bool,
    /// `getPermissions().getOwner()`
    pub owner_id: Uuid,
}

/// `VksInvFetchHost::model_id`'s ids.
pub const VKS_INV_FETCH_ROOT: i32 = 0;
pub const VKS_INV_FETCH_LIBRARY_ROOT: i32 = 1;
pub const VKS_INV_FETCH_LIBRARY_OWNER: i32 = 2;
pub const VKS_INV_FETCH_MARKETPLACE: i32 = 3;
pub const VKS_INV_FETCH_CURRENT_OUTFIT: i32 = 4;
pub const VKS_INV_FETCH_BASE_OUTFIT: i32 = 5;

/// The model as the scheduler reads it. UUIDs are 16 bytes.
#[repr(C)]
pub struct VksInvFetchHost {
    pub ctx: *mut core::ffi::c_void,
    /// One of the `VKS_INV_FETCH_*` ids (`getRootFolderID()`, ...).
    pub model_id: extern "C" fn(ctx: *mut core::ffi::c_void, which: i32, out: *mut u8),
    /// `gInventory.getCategory(id)`: false when there is none.
    pub category: extern "C" fn(
        ctx: *mut core::ffi::c_void,
        id: *const u8,
        out: *mut VksInvFetchCategory,
    ) -> bool,
    /// The category ids of `getDirectDescendentsOf(id)` in the model's
    /// order: their count, with `*ids` pointing to `count` × 16 bytes valid
    /// until the next call; -1 when the model has no array for the id.
    pub child_categories:
        extern "C" fn(ctx: *mut core::ffi::c_void, id: *const u8, ids: *mut *const u8) -> isize,
    /// `gInventory.getItem(id)`: false when there is none.
    pub item: extern "C" fn(
        ctx: *mut core::ffi::c_void,
        id: *const u8,
        out: *mut VksInvFetchItem,
    ) -> bool,
    /// `gInventory.isObjectDescendentOf(obj, cat)`
    pub is_object_descendent_of:
        extern "C" fn(ctx: *mut core::ffi::c_void, obj: *const u8, cat: *const u8) -> bool,
    /// `LLViewerInventoryCategory::getFetching()` (`EFetchType`) of a
    /// category the model has.
    pub get_fetching: extern "C" fn(ctx: *mut core::ffi::c_void, id: *const u8) -> i32,
    /// `setFetching(state)` of a category the model has.
    pub set_fetching: extern "C" fn(ctx: *mut core::ffi::c_void, id: *const u8, state: i32),
    /// `setVersion(version)` of a category the model has.
    pub set_version: extern "C" fn(ctx: *mut core::ffi::c_void, id: *const u8, version: i32),
    /// `LLTimer::getTotalSeconds()`
    pub total_seconds: extern "C" fn(ctx: *mut core::ffi::c_void) -> f64,
}

struct Host<'a>(&'a VksInvFetchHost);

impl Host<'_> {
    fn id(&self, which: i32) -> Uuid {
        let mut out = [0u8; 16];
        (self.0.model_id)(self.0.ctx, which, out.as_mut_ptr());
        out
    }
}

fn category_fetch(v: i32) -> CategoryFetch {
    match v {
        1 => CategoryFetch::Normal,
        2 => CategoryFetch::Recursive,
        3 => CategoryFetch::Failed,
        _ => CategoryFetch::None,
    }
}

impl FetchModel for Host<'_> {
    fn root_folder_id(&self) -> Uuid {
        self.id(VKS_INV_FETCH_ROOT)
    }
    fn library_root_folder_id(&self) -> Uuid {
        self.id(VKS_INV_FETCH_LIBRARY_ROOT)
    }
    fn library_owner_id(&self) -> Uuid {
        self.id(VKS_INV_FETCH_LIBRARY_OWNER)
    }
    fn marketplace_listings_id(&self) -> Uuid {
        self.id(VKS_INV_FETCH_MARKETPLACE)
    }
    fn current_outfit_id(&self) -> Uuid {
        self.id(VKS_INV_FETCH_CURRENT_OUTFIT)
    }
    fn base_outfit_id(&self) -> Uuid {
        self.id(VKS_INV_FETCH_BASE_OUTFIT)
    }
    fn category(&self, id: &Uuid) -> Option<CategoryInfo> {
        let mut out = VksInvFetchCategory {
            version: 0,
            preferred_type: 0,
            owner_id: [0; 16],
        };
        (self.0.category)(self.0.ctx, id.as_ptr(), &mut out).then_some(CategoryInfo {
            version: out.version,
            preferred_type: out.preferred_type as i8,
            owner_id: out.owner_id,
        })
    }
    fn child_categories(&self, id: &Uuid) -> Option<Vec<Uuid>> {
        let mut ids: *const u8 = std::ptr::null();
        let n = (self.0.child_categories)(self.0.ctx, id.as_ptr(), &mut ids);
        if n < 0 {
            return None;
        }
        if n == 0 || ids.is_null() {
            return Some(Vec::new());
        }
        // SAFETY: the host's array of n UUIDs, valid until its next call.
        let raw = unsafe { slice::from_raw_parts(ids as *const Uuid, n as usize) };
        Some(raw.to_vec())
    }
    fn item(&self, id: &Uuid) -> Option<ItemInfo> {
        let mut out = VksInvFetchItem {
            complete: false,
            owner_id: [0; 16],
        };
        (self.0.item)(self.0.ctx, id.as_ptr(), &mut out).then_some(ItemInfo {
            complete: out.complete,
            owner_id: out.owner_id,
        })
    }
    fn is_object_descendent_of(&self, obj: &Uuid, cat: &Uuid) -> bool {
        (self.0.is_object_descendent_of)(self.0.ctx, obj.as_ptr(), cat.as_ptr())
    }
    fn get_fetching(&mut self, id: &Uuid) -> CategoryFetch {
        category_fetch((self.0.get_fetching)(self.0.ctx, id.as_ptr()))
    }
    fn set_fetching(&mut self, id: &Uuid, state: CategoryFetch) {
        (self.0.set_fetching)(self.0.ctx, id.as_ptr(), state as i32)
    }
    fn set_version(&mut self, id: &Uuid, version: i32) {
        (self.0.set_version)(self.0.ctx, id.as_ptr(), version)
    }
}

impl BudgetClock for Host<'_> {
    fn total_seconds(&mut self) -> f64 {
        (self.0.total_seconds)(self.0.ctx)
    }
}

/// The environment of a call: the settings, the region and its
/// capabilities, the session state. The legacy capability URLs are read
/// only by the legacy path (empty is "no capability").
#[repr(C)]
pub struct VksInvFetchEnv {
    /// `gAgent.getID()`
    pub agent_id: Uuid,
    /// `use_http_inventory()`
    pub use_http_inventory: bool,
    /// `AISAPI::isAvailable()`
    pub ais_available: bool,
    /// `gAgent.getRegion()` is set.
    pub region: bool,
    /// `capabilitiesReceived()`
    pub capabilities_received: bool,
    /// `gDisconnected`
    pub disconnected: bool,
    /// `LLApp::isExiting()`
    pub exiting: bool,
    /// `LLGridManager::isInSecondLife()`
    pub in_second_life: bool,
    /// `LLStartUp::getStartupState() > STATE_WEARABLES_WAIT`
    pub past_wearables_wait: bool,
    /// `AISAPI::getInvCap()` is not empty.
    pub inventory_cap: bool,
    /// `AISAPI::getLibCap()` is not empty.
    pub library_cap: bool,
    /// `PoolSizeAIS`
    pub pool_size_ais: u32,
    /// `BatchSizeAIS3`
    pub batch_size_ais3: i32,
    /// `InventorySortOrder`
    pub sort_order: u32,
    /// `FetchInventoryDescendents2`
    pub cap_fid2: VksInvBytes,
    /// `FetchLibDescendents2`
    pub cap_fld2: VksInvBytes,
    /// `FetchInventory2`
    pub cap_fi2: VksInvBytes,
    /// `FetchLib2`
    pub cap_fl2: VksInvBytes,
}

fn cap(b: &VksInvBytes) -> String {
    if b.ptr.is_null() || b.len == 0 {
        return String::new();
    }
    // SAFETY: borrowed from the caller for this call.
    let s = unsafe { slice::from_raw_parts(b.ptr, b.len) };
    String::from_utf8_lossy(s).into_owned()
}

impl VksInvFetchEnv {
    fn env(&self) -> Env {
        Env {
            use_http_inventory: self.use_http_inventory,
            ais_available: self.ais_available,
            region: self.region,
            capabilities_received: self.capabilities_received,
            disconnected: self.disconnected,
            exiting: self.exiting,
            in_second_life: self.in_second_life,
            pool_size_ais: self.pool_size_ais,
            batch_size_ais3: self.batch_size_ais3,
            past_wearables_wait: self.past_wearables_wait,
            agent_id: self.agent_id,
            sort_order: self.sort_order,
            inventory_cap: self.inventory_cap,
            library_cap: self.library_cap,
            cap_fid2: cap(&self.cap_fid2),
            cap_fld2: cap(&self.cap_fld2),
            cap_fi2: cap(&self.cap_fi2),
            cap_fl2: cap(&self.cap_fl2),
        }
    }
}

// ---------------------------------------------------------------------------
// The operation list

pub const VKS_INV_FETCH_OP_ISSUE_AIS: i32 = 1;
pub const VKS_INV_FETCH_OP_CALLBACK: i32 = 2;
pub const VKS_INV_FETCH_OP_NOTIFY: i32 = 3;
pub const VKS_INV_FETCH_OP_FETCH_COMPLETED: i32 = 4;
pub const VKS_INV_FETCH_OP_ADD_IDLE: i32 = 5;
pub const VKS_INV_FETCH_OP_LEGACY_POST: i32 = 6;
pub const VKS_INV_FETCH_OP_LEGACY_PUMP: i32 = 7;
pub const VKS_INV_FETCH_OP_NOTIFY_OBSERVERS: i32 = 8;
pub const VKS_INV_FETCH_OP_FETCH_STARTED: i32 = 9;
pub const VKS_INV_FETCH_OP_INITIAL_FETCH_DURATION: i32 = 10;
pub const VKS_INV_FETCH_OP_FETCH_TIMER_RESET: i32 = 11;
pub const VKS_INV_FETCH_OP_WARN: i32 = 12;

/// One operation; which fields mean something depends on `kind`
/// (`VKS_INV_FETCH_OP_*`, see the header).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvFetchOp {
    pub kind: i32,
    /// ISSUE_AIS: `AISAPI::COMMAND_TYPE`; WARN: the warning; LEGACY_POST:
    /// 1 for a folder request, 0 for an item request; LEGACY_PUMP: 1 when
    /// `notifyObservers()` follows.
    pub code: i32,
    /// ISSUE_AIS: the ticket; CALLBACK: the token; LEGACY_POST: the handler.
    pub ticket: u64,
    /// ISSUE_AIS: the category or item; WARN: the id it names.
    pub id: Uuid,
    /// ISSUE_AIS: `AISAPI::LIBRARY` rather than `INVENTORY`.
    pub library: bool,
    /// ISSUE_AIS: the `recursive` argument.
    pub recursive: bool,
    /// ISSUE_AIS: the `depth` argument.
    pub depth: i32,
    /// ISSUE_AIS (FETCHCATEGORYSUBSET): the children, `len / 16` UUIDs.
    pub children: VksInvBytes,
    /// NOTIFY: the notification; LEGACY_POST: the URL.
    pub text: VksInvBytes,
    /// LEGACY_POST: the body (LLSD XML, as `requestPostWithLLSD` writes it).
    pub body: VksInvBytes,
    /// LEGACY_POST: the request's label ("Inventory Folder", ...).
    pub label: VksInvBytes,
}

/// An operation list: owns the records and the bytes they point to.
pub struct VksInvFetchOps {
    ops: Vec<VksInvFetchOp>,
    _children: Vec<Vec<Uuid>>,
    _bytes: Vec<Vec<u8>>,
}

const NO_BYTES: VksInvBytes = VksInvBytes {
    ptr: std::ptr::null(),
    len: 0,
};

fn static_bytes(s: &'static str) -> VksInvBytes {
    VksInvBytes {
        ptr: s.as_ptr(),
        len: s.len(),
    }
}

fn op(kind: i32) -> VksInvFetchOp {
    VksInvFetchOp {
        kind,
        code: 0,
        ticket: 0,
        id: [0; 16],
        library: false,
        recursive: false,
        depth: 0,
        children: NO_BYTES,
        text: NO_BYTES,
        body: NO_BYTES,
        label: NO_BYTES,
    }
}

fn ops_block(actions: Vec<Action>) -> *mut VksInvFetchOps {
    if actions.is_empty() {
        return std::ptr::null_mut();
    }
    let mut block = VksInvFetchOps {
        ops: Vec::with_capacity(actions.len()),
        _children: Vec::new(),
        _bytes: Vec::new(),
    };
    for a in actions {
        let o = match a {
            Action::IssueAis { ticket, command } => {
                let mut o = op(VKS_INV_FETCH_OP_ISSUE_AIS);
                o.code = command.command_type();
                o.ticket = ticket;
                match command {
                    AisFetch::FetchItem { item_id, library } => {
                        o.id = item_id;
                        o.library = library;
                    }
                    AisFetch::FetchCategoryChildren {
                        category_id,
                        library,
                        recursive,
                        depth,
                    } => {
                        o.id = category_id;
                        o.library = library;
                        o.recursive = recursive;
                        o.depth = depth;
                    }
                    AisFetch::FetchCategorySubset {
                        category_id,
                        children,
                        library,
                        recursive,
                        depth,
                    } => {
                        o.id = category_id;
                        o.library = library;
                        o.recursive = recursive;
                        o.depth = depth;
                        // the heap buffer stays put when the Vec moves
                        o.children = VksInvBytes {
                            ptr: children.as_ptr() as *const u8,
                            len: children.len() * 16,
                        };
                        block._children.push(children);
                    }
                    AisFetch::FetchCategoryLinks { category_id } => o.id = category_id,
                    AisFetch::FetchCof | AisFetch::FetchOrphans => {}
                }
                o
            }
            Action::Callback { token } => {
                let mut o = op(VKS_INV_FETCH_OP_CALLBACK);
                o.ticket = token;
                o
            }
            Action::Notify { name } => {
                let mut o = op(VKS_INV_FETCH_OP_NOTIFY);
                o.text = static_bytes(name);
                o
            }
            Action::FetchCompleted => op(VKS_INV_FETCH_OP_FETCH_COMPLETED),
            Action::AddIdle => op(VKS_INV_FETCH_OP_ADD_IDLE),
            Action::LegacyPost {
                handler,
                folder,
                url,
                body,
                label,
            } => {
                let mut o = op(VKS_INV_FETCH_OP_LEGACY_POST);
                o.code = folder as i32;
                o.ticket = handler;
                let url = url.into_bytes();
                o.text = VksInvBytes {
                    ptr: url.as_ptr(),
                    len: url.len(),
                };
                o.body = VksInvBytes {
                    ptr: body.as_ptr(),
                    len: body.len(),
                };
                o.label = static_bytes(label);
                block._bytes.push(url);
                block._bytes.push(body);
                o
            }
            Action::LegacyPump { notify_observers } => {
                let mut o = op(VKS_INV_FETCH_OP_LEGACY_PUMP);
                o.code = notify_observers as i32;
                o
            }
            Action::NotifyObservers => op(VKS_INV_FETCH_OP_NOTIFY_OBSERVERS),
            Action::FetchStarted => op(VKS_INV_FETCH_OP_FETCH_STARTED),
            Action::InitialFetchDuration => op(VKS_INV_FETCH_OP_INITIAL_FETCH_DURATION),
            Action::FetchTimerReset => op(VKS_INV_FETCH_OP_FETCH_TIMER_RESET),
            Action::Warn { warning, id } => {
                let mut o = op(VKS_INV_FETCH_OP_WARN);
                o.code = warning as i32;
                o.id = id;
                o
            }
        };
        block.ops.push(o);
    }
    Box::into_raw(Box::new(block))
}

/// The operations of a list and their count; null and 0 for a null list.
///
/// # Safety
/// `ops` is null or a list from a `vks_inv_fetch_*` call, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_ops_view(
    ops: *const VksInvFetchOps,
    count: *mut usize,
) -> *const VksInvFetchOp {
    let (p, n) = match unsafe { ops.as_ref() } {
        Some(b) => (b.ops.as_ptr(), b.ops.len()),
        None => (std::ptr::null(), 0),
    };
    if !count.is_null() {
        unsafe { *count = n };
    }
    p
}

/// Frees an operation list (null is fine).
///
/// # Safety
/// `ops` is null or a list from a `vks_inv_fetch_*` call, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_ops_free(ops: *mut VksInvFetchOps) {
    if !ops.is_null() {
        drop(unsafe { Box::from_raw(ops) });
    }
}

// ---------------------------------------------------------------------------
// The scheduler

/// `LLInventoryModelBackgroundFetch`'s state.
pub struct VksInvFetch(BackgroundFetch);

unsafe fn uuid(p: *const u8) -> Uuid {
    if p.is_null() {
        [0; 16]
    } else {
        unsafe { std::ptr::read(p as *const Uuid) }
    }
}

/// Runs `f` on the scheduler and returns the operations it produced.
unsafe fn with(f: *mut VksInvFetch, run: impl FnOnce(&mut BackgroundFetch)) -> *mut VksInvFetchOps {
    let Some(f) = (unsafe { f.as_mut() }) else {
        return std::ptr::null_mut();
    };
    guard(std::ptr::null_mut(), || {
        run(&mut f.0);
        ops_block(f.0.take_actions())
    })
}

/// Runs `f` with the model and the environment.
unsafe fn with_cx(
    f: *mut VksInvFetch,
    host: *const VksInvFetchHost,
    env: *const VksInvFetchEnv,
    run: impl FnOnce(&mut BackgroundFetch, &mut Ctx<'_>),
) -> *mut VksInvFetchOps {
    let (Some(host), Some(env)) = (unsafe { host.as_ref() }, unsafe { env.as_ref() }) else {
        return std::ptr::null_mut();
    };
    unsafe {
        with(f, |s| {
            let env = env.env();
            let mut model = Host(host);
            let mut cx = Ctx::new(&mut model, &env);
            run(s, &mut cx);
        })
    }
}

/// A new scheduler (`LLInventoryModelBackgroundFetch`'s constructor).
#[unsafe(no_mangle)]
pub extern "C" fn vks_inv_fetch_new() -> *mut VksInvFetch {
    guard(std::ptr::null_mut(), || {
        Box::into_raw(Box::new(VksInvFetch(BackgroundFetch::new())))
    })
}

/// Frees a scheduler (null is fine). Open AIS tickets and legacy handlers
/// are dropped: their completions do nothing on another scheduler.
///
/// # Safety
/// `f` is null or from `vks_inv_fetch_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_free(f: *mut VksInvFetch) {
    if !f.is_null() {
        drop(unsafe { Box::from_raw(f) });
    }
}

/// `start(id, recursive)`
///
/// # Safety
/// Valid scheduler, host and environment; `id` is 16 bytes or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_start(
    f: *mut VksInvFetch,
    host: *const VksInvFetchHost,
    env: *const VksInvFetchEnv,
    id: *const u8,
    recursive: bool,
) -> *mut VksInvFetchOps {
    let id = unsafe { uuid(id) };
    unsafe { with_cx(f, host, env, |s, cx| s.start(cx, id, recursive)) }
}

/// `scheduleFolderFetch(id, forced)`
///
/// # Safety
/// Valid scheduler; `id` is 16 bytes or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_schedule_folder(
    f: *mut VksInvFetch,
    id: *const u8,
    forced: bool,
) -> *mut VksInvFetchOps {
    let id = unsafe { uuid(id) };
    unsafe { with(f, |s| s.schedule_folder_fetch(id, forced)) }
}

/// `scheduleItemFetch(id, forced)`
///
/// # Safety
/// Valid scheduler; `id` is 16 bytes or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_schedule_item(
    f: *mut VksInvFetch,
    id: *const u8,
    forced: bool,
) -> *mut VksInvFetchOps {
    let id = unsafe { uuid(id) };
    unsafe { with(f, |s| s.schedule_item_fetch(id, forced)) }
}

/// `fetchFolderAndLinks(id, callback)`; `token` 0 is `no_op`.
///
/// # Safety
/// Valid scheduler, host and environment; `id` is 16 bytes or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_folder_and_links(
    f: *mut VksInvFetch,
    host: *const VksInvFetchHost,
    env: *const VksInvFetchEnv,
    id: *const u8,
    token: u64,
) -> *mut VksInvFetchOps {
    let id = unsafe { uuid(id) };
    let token = (token != 0).then_some(token);
    unsafe {
        with_cx(f, host, env, |s, cx| {
            s.fetch_folder_and_links(cx, id, token)
        })
    }
}

/// `fetchCOF(callback)`; `token` 0 is `no_op`.
///
/// # Safety
/// Valid scheduler, host and environment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_cof(
    f: *mut VksInvFetch,
    host: *const VksInvFetchHost,
    env: *const VksInvFetchEnv,
    token: u64,
) -> *mut VksInvFetchOps {
    let token = (token != 0).then_some(token);
    unsafe { with_cx(f, host, env, |s, cx| s.fetch_cof(cx, token)) }
}

/// `findLostItems()`
///
/// # Safety
/// Valid scheduler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_find_lost_items(f: *mut VksInvFetch) -> *mut VksInvFetchOps {
    unsafe { with(f, |s| s.find_lost_items()) }
}

/// `setAllFoldersFetched()`
///
/// # Safety
/// Valid scheduler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_set_all_folders_fetched(
    f: *mut VksInvFetch,
) -> *mut VksInvFetchOps {
    unsafe { with(f, |s| s.set_all_folders_fetched()) }
}

/// `incrFetchCount(n)` (`folder` false) or `incrFetchFolderCount(n)`.
///
/// # Safety
/// Valid scheduler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_incr_count(
    f: *mut VksInvFetch,
    n: i32,
    folder: bool,
) -> *mut VksInvFetchOps {
    unsafe {
        with(f, |s| {
            if folder {
                s.incr_fetch_folder_count(n)
            } else {
                s.incr_fetch_count(n)
            }
        })
    }
}

/// `addRequestAtFront` (`front`) or `addRequestAtBack`.
///
/// # Safety
/// Valid scheduler; `id` is 16 bytes or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_add_request(
    f: *mut VksInvFetch,
    id: *const u8,
    recursive: bool,
    is_category: bool,
    front: bool,
) {
    let id = unsafe { uuid(id) };
    let Some(f) = (unsafe { f.as_mut() }) else {
        return;
    };
    guard((), || {
        if front {
            f.0.add_request_at_front(id, recursive, is_category)
        } else {
            f.0.add_request_at_back(id, recursive, is_category)
        }
    })
}

/// `mBackgroundFetchActive`: whether the idle callback has work. Cheap: the
/// C++ asks it every frame before it builds an environment.
///
/// # Safety
/// Valid scheduler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_active(f: *const VksInvFetch) -> bool {
    match unsafe { f.as_ref() } {
        Some(f) => f.0.background_fetch_active(),
        None => false,
    }
}

/// `backgroundFetch()`, the idle callback. On the legacy path with requests
/// in flight its list ends with LEGACY_PUMP: the C++ pumps the handlers,
/// then calls `vks_inv_fetch_after_pump`.
///
/// # Safety
/// Valid scheduler, host and environment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_idle(
    f: *mut VksInvFetch,
    host: *const VksInvFetchHost,
    env: *const VksInvFetchEnv,
) -> *mut VksInvFetchOps {
    let Some(h) = (unsafe { host.as_ref() }) else {
        return std::ptr::null_mut();
    };
    unsafe {
        with_cx(f, host, env, |s, cx| {
            let mut clock = Host(h);
            s.background_fetch(cx, &mut clock)
        })
    }
}

/// The rest of `bulkFetch()` after LEGACY_PUMP.
///
/// # Safety
/// Valid scheduler, host and environment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_after_pump(
    f: *mut VksInvFetch,
    host: *const VksInvFetchHost,
    env: *const VksInvFetchEnv,
) -> *mut VksInvFetchOps {
    unsafe { with_cx(f, host, env, |s, cx| s.bulk_fetch_after_pump(cx)) }
}

/// The rest of a completion after its CALLBACK ran.
///
/// # Safety
/// Valid scheduler, host and environment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_resume(
    f: *mut VksInvFetch,
    host: *const VksInvFetchHost,
    env: *const VksInvFetchEnv,
) -> *mut VksInvFetchOps {
    unsafe { with_cx(f, host, env, |s, cx| s.resume(cx)) }
}

/// An ISSUE_AIS command's completion was called with `id`.
///
/// # Safety
/// Valid scheduler, host and environment; `id` is 16 bytes or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_ais_complete(
    f: *mut VksInvFetch,
    host: *const VksInvFetchHost,
    env: *const VksInvFetchEnv,
    ticket: u64,
    id: *const u8,
) -> *mut VksInvFetchOps {
    let id = unsafe { uuid(id) };
    unsafe { with_cx(f, host, env, |s, cx| s.ais_complete(cx, ticket, &[id])) }
}

/// `BGFolderHttpHandler::getIsRecursive(id)` of a LEGACY_POST handler.
///
/// # Safety
/// Valid scheduler; `id` is 16 bytes or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_legacy_is_recursive(
    f: *const VksInvFetch,
    handler: u64,
    id: *const u8,
) -> bool {
    let id = unsafe { uuid(id) };
    match unsafe { f.as_ref() } {
        Some(f) => guard(false, || f.0.legacy_is_recursive(handler, &id)),
        None => false,
    }
}

/// The end of `BGFolderHttpHandler::processData()`.
///
/// # Safety
/// Valid scheduler, host and environment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_legacy_data_done(
    f: *mut VksInvFetch,
    host: *const VksInvFetchHost,
    env: *const VksInvFetchEnv,
) -> *mut VksInvFetchOps {
    unsafe { with_cx(f, host, env, |s, cx| s.legacy_data_done(cx)) }
}

/// `BGFolderHttpHandler::processFailure(HttpStatus)` with the HTTP status
/// (the 403 split).
///
/// # Safety
/// Valid scheduler, host and environment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_legacy_http_failure(
    f: *mut VksInvFetch,
    host: *const VksInvFetchHost,
    env: *const VksInvFetchEnv,
    handler: u64,
    status: i32,
) -> *mut VksInvFetchOps {
    unsafe {
        with_cx(f, host, env, |s, cx| {
            s.legacy_http_failure(cx, handler, status)
        })
    }
}

/// `BGFolderHttpHandler::processFailure(reason)` (the re-queue).
///
/// # Safety
/// Valid scheduler, host and environment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_legacy_reply_failure(
    f: *mut VksInvFetch,
    host: *const VksInvFetchHost,
    env: *const VksInvFetchEnv,
    handler: u64,
) -> *mut VksInvFetchOps {
    unsafe { with_cx(f, host, env, |s, cx| s.legacy_reply_failure(cx, handler)) }
}

/// A LEGACY_POST handler was destroyed.
///
/// # Safety
/// Valid scheduler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_legacy_released(
    f: *mut VksInvFetch,
    handler: u64,
) -> *mut VksInvFetchOps {
    unsafe { with(f, |s| s.legacy_handler_released(handler)) }
}

pub const VKS_INV_FETCH_BG_ACTIVE: u32 = 1;
pub const VKS_INV_FETCH_FOLDER_ACTIVE: u32 = 2;
pub const VKS_INV_FETCH_INV_STARTED: u32 = 4;
pub const VKS_INV_FETCH_LIB_STARTED: u32 = 8;
pub const VKS_INV_FETCH_MKT_STARTED: u32 = 16;
pub const VKS_INV_FETCH_EVERYTHING: u32 = 32;
pub const VKS_INV_FETCH_BULK_COMPLETE: u32 = 64;
pub const VKS_INV_FETCH_FOLDER_COMPLETE: u32 = 128;

/// The public state as `VKS_INV_FETCH_*` bits.
///
/// # Safety
/// Valid scheduler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_flags(f: *const VksInvFetch) -> u32 {
    let Some(f) = (unsafe { f.as_ref() }) else {
        return 0;
    };
    let s = &f.0;
    let mut b = 0;
    for (on, bit) in [
        (s.background_fetch_active(), VKS_INV_FETCH_BG_ACTIVE),
        (s.folder_fetch_active(), VKS_INV_FETCH_FOLDER_ACTIVE),
        (s.inventory_fetch_started(), VKS_INV_FETCH_INV_STARTED),
        (s.library_fetch_started(), VKS_INV_FETCH_LIB_STARTED),
        (s.marketplace_fetch_started(), VKS_INV_FETCH_MKT_STARTED),
        (s.is_everything_fetched(), VKS_INV_FETCH_EVERYTHING),
        (
            s.is_bulk_fetch_processing_complete(),
            VKS_INV_FETCH_BULK_COMPLETE,
        ),
        (
            s.is_folder_fetch_processing_complete(),
            VKS_INV_FETCH_FOLDER_COMPLETE,
        ),
    ] {
        if on {
            b |= bit;
        }
    }
    b
}

/// `fetchQueueContainsNoDescendentsOf(cat)`
///
/// # Safety
/// Valid scheduler and host; `cat` is 16 bytes or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_queue_has_no_descendents_of(
    f: *const VksInvFetch,
    host: *const VksInvFetchHost,
    cat: *const u8,
) -> bool {
    let cat = unsafe { uuid(cat) };
    let (Some(f), Some(host)) = (unsafe { f.as_ref() }, unsafe { host.as_ref() }) else {
        return true;
    };
    guard(true, || {
        f.0.fetch_queue_contains_no_descendents_of(&Host(host), &cat)
    })
}

/// A queue entry (`FetchQueueInfo`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvFetchEntry {
    pub id: Uuid,
    pub is_category: bool,
    /// `EFetchType`
    pub fetch_type: i32,
}

fn fetch_type(v: i32) -> FetchType {
    match v {
        1 => FetchType::Forced,
        2 => FetchType::ContentRecursive,
        3 => FetchType::FolderAndContent,
        4 => FetchType::Recursive,
        _ => FetchType::Default,
    }
}

fn entry_out(e: &QueueEntry) -> VksInvFetchEntry {
    VksInvFetchEntry {
        id: e.id,
        is_category: e.is_category,
        fetch_type: e.fetch_type as i32,
    }
}

/// The folder queue's front, for the `#ifdef OPENSIM` UDP loop: false when
/// the queue is empty.
///
/// # Safety
/// Valid scheduler and `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_folder_front(
    f: *const VksInvFetch,
    out: *mut VksInvFetchEntry,
) -> bool {
    let (Some(f), Some(out)) = (unsafe { f.as_ref() }, unsafe { out.as_mut() }) else {
        return false;
    };
    match f.0.folder_queue_front() {
        Some(e) => {
            *out = entry_out(&e);
            true
        }
        None => false,
    }
}

/// `mFetchFolderQueue.pop_front()` (the UDP loop).
///
/// # Safety
/// Valid scheduler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_folder_pop_front(f: *mut VksInvFetch) {
    if let Some(f) = unsafe { f.as_mut() } {
        f.0.folder_queue_pop_front();
    }
}

/// `mFetchFolderQueue.push_back(entry)` (the UDP loop).
///
/// # Safety
/// Valid scheduler and `entry`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_folder_push_back(
    f: *mut VksInvFetch,
    entry: *const VksInvFetchEntry,
) {
    let (Some(f), Some(e)) = (unsafe { f.as_mut() }, unsafe { entry.as_ref() }) else {
        return;
    };
    f.0.folder_queue_push_back(QueueEntry {
        id: e.id,
        is_category: e.is_category,
        fetch_type: fetch_type(e.fetch_type),
    });
}

/// The private state (the R3b recorder's `sch` lines): the queues, the
/// expected folders and the forced set, read with
/// `vks_inv_fetch_state_view` and freed with `vks_inv_fetch_state_free`.
pub struct VksInvFetchState {
    view: VksInvFetchStateView,
    _folders: Vec<VksInvFetchEntry>,
    _items: Vec<VksInvFetchEntry>,
    _expected: Vec<Uuid>,
    _forced: Vec<Uuid>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvFetchStateView {
    /// `VKS_INV_FETCH_*` bits.
    pub flags: u32,
    pub fetch_count: i32,
    pub folder_fetch_count: i32,
    pub folder_queue: *const VksInvFetchEntry,
    pub folder_queue_len: usize,
    pub item_queue: *const VksInvFetchEntry,
    pub item_queue_len: usize,
    /// `mExpectedFolderIds`, in order (16 bytes each).
    pub expected: *const u8,
    pub expected_len: usize,
    /// `mForceFetchSet`, in `LLUUID` order (16 bytes each).
    pub forced: *const u8,
    pub forced_len: usize,
}

/// The private state (see [`VksInvFetchState`]).
///
/// # Safety
/// Valid scheduler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_state(f: *const VksInvFetch) -> *mut VksInvFetchState {
    let flags = unsafe { vks_inv_fetch_flags(f) };
    let Some(f) = (unsafe { f.as_ref() }) else {
        return std::ptr::null_mut();
    };
    guard(std::ptr::null_mut(), || {
        let st = f.0.state();
        let folders: Vec<_> = st.folder_queue.iter().map(entry_out).collect();
        let items: Vec<_> = st.item_queue.iter().map(entry_out).collect();
        let view = VksInvFetchStateView {
            flags,
            fetch_count: st.fetch_count,
            folder_fetch_count: st.folder_fetch_count,
            folder_queue: folders.as_ptr(),
            folder_queue_len: folders.len(),
            item_queue: items.as_ptr(),
            item_queue_len: items.len(),
            expected: st.expected.as_ptr() as *const u8,
            expected_len: st.expected.len(),
            forced: st.forced.as_ptr() as *const u8,
            forced_len: st.forced.len(),
        };
        Box::into_raw(Box::new(VksInvFetchState {
            view,
            _folders: folders,
            _items: items,
            _expected: st.expected,
            _forced: st.forced,
        }))
    })
}

/// # Safety
/// `st` is from `vks_inv_fetch_state`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_state_view(
    st: *const VksInvFetchState,
) -> VksInvFetchStateView {
    match unsafe { st.as_ref() } {
        Some(s) => s.view,
        None => VksInvFetchStateView {
            flags: 0,
            fetch_count: 0,
            folder_fetch_count: 0,
            folder_queue: std::ptr::null(),
            folder_queue_len: 0,
            item_queue: std::ptr::null(),
            item_queue_len: 0,
            expected: std::ptr::null(),
            expected_len: 0,
            forced: std::ptr::null(),
            forced_len: 0,
        },
    }
}

/// Frees a state block (null is fine).
///
/// # Safety
/// `st` is null or from `vks_inv_fetch_state`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_fetch_state_free(st: *mut VksInvFetchState) {
    if !st.is_null() {
        drop(unsafe { Box::from_raw(st) });
    }
}

// The header's layouts (x86-64; the viewer asserts the same sizes).
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<VksInvFetchOp>() == 104);
    assert!(size_of::<VksInvFetchEnv>() == 104);
    assert!(size_of::<VksInvFetchCategory>() == 24);
    assert!(size_of::<VksInvFetchItem>() == 17);
    assert!(size_of::<VksInvFetchEntry>() == 24);
};
