//! Rung seven, sub-rung R4a: object contents ("task inventory") via
//! `vkstorm-inventory`'s `task` module (`vks_inv_task_*`): the decisions of
//! `LLViewerObject`'s task-inventory code (the request and serial state
//! machine, the task file loader, the capability reply, the local mutations
//! with the owner guess and the messages they send, `createInventoryItem`)
//! and the lookups over the contents.
//!
//! The object's state stays C++ (`mInventory` and its entries, the serials,
//! the request state, the xfer id, the dirty flag, the pending assets, the
//! listeners). Each decision takes:
//!
//! - a [`VksInvTaskView`]: the scalar state, the pending asset ids, and one
//!   [`VksInvTaskEntry`] key per entry of `mInventory` in list order (its id
//!   and type, and for an `LLViewerInventoryItem` its asset and a pointer to
//!   its `LLPermissions`, in place; all as the C++ getters return them);
//! - a [`VksInvTaskEnv`]: the agent, the region's capabilities, and the two
//!   inputs of the owner guess (`permYouOwner`, the selection's owner and
//!   group);
//!
//! and returns a [`VksInvTaskResult`] block: the ordered actions the C++
//! executor carries out (state assignments, list edits, listener
//! notifications, messages, xfers, coroutine launches, files, floaters,
//! callbacks), with the entries to push as flat records and the messages as
//! flat records (the C++ keeps the `add*Fast` field I/O). Read it through
//! [`vks_inv_task_result_view`], free it with `vks_inv_task_result_free`.
//!
//! The task file crosses whole: the C++ reads the cache file through the
//! same text-mode stream the loader used and hands the bytes over once; the
//! entries come back as records in one block (no crossing per entry).
//!
//! The capability reply's entries are an `LLSD` form, which stays C++ (R1's
//! decision: flattening a C++ `LLSD` tree across the ABI costs more than the
//! rules save): the reply decision takes `inventory_serial` (`asInteger`)
//! and whether `contents` is present, and leaves the entries to the C++
//! ([`VKS_INV_TASK_PUSH_FRONT_REPLY_CONTENTS`]). The create request's
//! `params` and the POST body cross as binary LLSD (one cold call each).

use std::sync::OnceLock;
use std::{ptr, slice};

use vkstorm_inventory::item::Item;
use vkstorm_inventory::object::Object;
use vkstorm_inventory::perms::{Permissions, Uuid};
use vkstorm_inventory::task::{
    self, Action, Agent, CapReply, CreateRequest, Entry, Env, FloaterKey, LocalizedNames, Outgoing,
    Region, RequestState, Selection, TaskEntry, TaskState, TaskView, ViewerItem, lookup,
};
use vkstorm_inventory::types::{AssetType, InventoryType};
use vkstorm_llsd::{Format, Limits, Options, Value};

use crate::inventory::{
    VksInvBytes, VksInvItem, VksInvItemBlock, VksInvObject, VksInvPerms, bytes, guard, item_from,
    item_view, object_view, uuid_arg,
};

// ---------------------------------------------------------------------------
// Inputs

/// One entry of `mInventory`, as the decisions read it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvTaskEntry {
    /// `getUUID()`.
    pub id: Uuid,
    /// `getType()`, any value.
    pub asset_type: i32,
    /// The entry is an `LLViewerInventoryItem`.
    pub is_item: bool,
    /// `getAssetUUID()` of an item.
    pub asset: Uuid,
    /// `getPermissions()` of an item, in place; null for a plain object.
    pub perms: *const VksInvPerms,
}

impl TaskEntry for VksInvTaskEntry {
    fn id(&self) -> Uuid {
        self.id
    }

    fn asset_type(&self) -> AssetType {
        AssetType(self.asset_type)
    }

    fn item_asset(&self) -> Option<Uuid> {
        self.is_item.then_some(self.asset)
    }

    fn item_permissions(&self) -> Option<&Permissions> {
        if self.is_item && !self.perms.is_null() {
            // The C++ object outlives the call that passed the key.
            Some(unsafe { &*self.perms })
        } else {
            None
        }
    }
}

/// The decisions' entries: the C++ object's (as keys), and the entries a
/// decision created (as records, boxed so that a key stays small: the
/// decisions copy the list of keys).
#[derive(Clone)]
enum Key {
    Host(VksInvTaskEntry),
    New(Box<Entry>),
}

impl From<Entry> for Key {
    fn from(e: Entry) -> Key {
        Key::New(Box::new(e))
    }
}

impl TaskEntry for Key {
    fn id(&self) -> Uuid {
        match self {
            Key::Host(k) => k.id,
            Key::New(e) => e.id(),
        }
    }

    fn asset_type(&self) -> AssetType {
        match self {
            Key::Host(k) => TaskEntry::asset_type(k),
            Key::New(e) => e.asset_type(),
        }
    }

    fn item_asset(&self) -> Option<Uuid> {
        match self {
            Key::Host(k) => k.item_asset(),
            Key::New(e) => TaskEntry::item_asset(&**e),
        }
    }

    fn item_permissions(&self) -> Option<&Permissions> {
        match self {
            Key::Host(k) => k.item_permissions(),
            Key::New(e) => TaskEntry::item_permissions(&**e),
        }
    }
}

/// `EInventoryRequestState` values (the C++ enum's order).
pub const VKS_INV_TASK_REQUEST_STOPPED: i32 = 0;
pub const VKS_INV_TASK_REQUEST_WAIT: i32 = 1;
pub const VKS_INV_TASK_REQUEST_PENDING: i32 = 2;
pub const VKS_INV_TASK_XFER: i32 = 3;

fn request_from(v: i32) -> RequestState {
    match v {
        VKS_INV_TASK_REQUEST_WAIT => RequestState::Wait,
        VKS_INV_TASK_REQUEST_PENDING => RequestState::Pending,
        VKS_INV_TASK_XFER => RequestState::Xfer,
        _ => RequestState::Stopped,
    }
}

fn request_to(v: RequestState) -> i32 {
    match v {
        RequestState::Stopped => VKS_INV_TASK_REQUEST_STOPPED,
        RequestState::Wait => VKS_INV_TASK_REQUEST_WAIT,
        RequestState::Pending => VKS_INV_TASK_REQUEST_PENDING,
        RequestState::Xfer => VKS_INV_TASK_XFER,
    }
}

/// The object's task-inventory state.
#[repr(C)]
pub struct VksInvTaskView {
    /// `mID`.
    pub id: Uuid,
    /// `mLocalID`.
    pub local_id: u32,
    /// `mInventorySerialNum`, `mExpectedInventorySerialNum`.
    pub serial: i16,
    pub expected: i16,
    /// `mInvRequestState` (`VKS_INV_TASK_REQUEST_*`).
    pub request: i32,
    /// `mInvRequestXFerId`.
    pub xfer_id: u64,
    /// `mInventoryDirty`.
    pub dirty: bool,
    /// `mInventory != NULL`.
    pub has_inventory: bool,
    /// `!mInventoryCallbacks.empty()`.
    pub has_listeners: bool,
    /// `mInventory` front to back (ignored without an inventory).
    pub entries: *const VksInvTaskEntry,
    pub entry_count: usize,
    /// `mPendingInventoryItemsIDs` front to back, 16 bytes each.
    pub pending: *const u8,
    pub pending_count: usize,
}

/// The inputs from outside the object.
#[repr(C)]
pub struct VksInvTaskEnv {
    /// `gAgent`: `getID`, `getSessionID`, `getGroupID`.
    pub agent: Uuid,
    pub session: Uuid,
    pub group: Uuid,
    /// `mRegionp != NULL`.
    pub has_region: bool,
    /// The region's `RequestTaskInventory` and `CreateTaskInventoryItem`
    /// capabilities (empty: none).
    pub cap_request_task_inventory: VksInvBytes,
    pub cap_create_task_inventory_item: VksInvBytes,
    /// `permYouOwner()`.
    pub you_owner: bool,
    /// `LLSelectMgr::findObjectPermissions(this) != NULL`, and its owner
    /// and group.
    pub has_selection: bool,
    pub selection_owner: Uuid,
    pub selection_group: Uuid,
}

/// An `LLViewerInventoryItem`: the `LLInventoryItem` fields and the
/// viewer's own.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvTaskItem {
    pub item: VksInvItem,
    /// `mTransactionID`.
    pub transaction: Uuid,
    /// `mIsComplete`, `mIsRunning`, `mIsFaulted`.
    pub complete: bool,
    pub running: bool,
    pub faulted: bool,
}

unsafe fn viewer_item_from(v: &VksInvTaskItem) -> ViewerItem {
    ViewerItem {
        item: unsafe { item_from(&v.item) },
        complete: v.complete,
        transaction: v.transaction,
        running: v.running,
        faulted: v.faulted,
    }
}

fn viewer_item_view(v: &ViewerItem) -> VksInvTaskItem {
    VksInvTaskItem {
        item: item_view(&v.item),
        transaction: v.transaction,
        complete: v.complete,
        running: v.running,
        faulted: v.faulted,
    }
}

fn no_names() -> &'static LocalizedNames {
    static N: OnceLock<LocalizedNames> = OnceLock::new();
    N.get_or_init(LocalizedNames::none)
}

/// The owned inputs of one decision: the keys and pending ids copied from
/// the C++ arrays, and the capabilities as text.
struct Inputs {
    id: Uuid,
    local_id: u32,
    state: TaskState,
    keys: Option<Vec<Key>>,
    pending: Vec<Uuid>,
    has_listeners: bool,
    agent: Agent,
    region: Option<(String, String)>,
    you_owner: bool,
    selection: Option<Selection>,
}

impl Inputs {
    unsafe fn read(view: *const VksInvTaskView, env: *const VksInvTaskEnv) -> Inputs {
        let v = unsafe { &*view };
        let e = unsafe { &*env };
        let keys = v.has_inventory.then(|| {
            let entries: &[VksInvTaskEntry] = if v.entries.is_null() || v.entry_count == 0 {
                &[]
            } else {
                unsafe { slice::from_raw_parts(v.entries, v.entry_count) }
            };
            entries.iter().map(|k| Key::Host(*k)).collect()
        });
        let pending = unsafe { bytes(v.pending, v.pending_count * 16) }
            .chunks_exact(16)
            .map(|c| {
                let mut u = [0u8; 16];
                u.copy_from_slice(c);
                u
            })
            .collect();
        let text = |b: &VksInvBytes| String::from_utf8_lossy(unsafe { b.get() }).into_owned();
        Inputs {
            id: v.id,
            local_id: v.local_id,
            state: TaskState {
                serial: v.serial,
                expected: v.expected,
                request: request_from(v.request),
                xfer_id: v.xfer_id,
                dirty: v.dirty,
            },
            keys,
            pending,
            has_listeners: v.has_listeners,
            agent: Agent {
                id: e.agent,
                session: e.session,
                group: e.group,
            },
            region: e.has_region.then(|| {
                (
                    text(&e.cap_request_task_inventory),
                    text(&e.cap_create_task_inventory_item),
                )
            }),
            you_owner: e.you_owner,
            selection: e.has_selection.then_some(Selection {
                owner: e.selection_owner,
                group: e.selection_group,
            }),
        }
    }

    fn view(&self) -> TaskView<'_, Key> {
        TaskView {
            id: self.id,
            local_id: self.local_id,
            state: self.state,
            inventory: self.keys.as_deref(),
            pending: &self.pending,
            has_listeners: self.has_listeners,
        }
    }

    fn env(&self) -> Env<'_> {
        Env {
            agent: self.agent,
            region: self.region.as_ref().map(|(rti, create)| Region {
                request_task_inventory: rti,
                create_task_inventory_item: create,
            }),
            you_owner: self.you_owner,
            selection: self.selection,
            in_open_sim: false,
            names: no_names(),
        }
    }
}

// ---------------------------------------------------------------------------
// Results

/// Action kinds of [`VksInvTaskAction`]. The fields each one uses follow
/// its name.
pub const VKS_INV_TASK_SET_SERIAL: u32 = 1; // value
pub const VKS_INV_TASK_SET_EXPECTED: u32 = 2; // value
pub const VKS_INV_TASK_SET_REQUEST_STATE: u32 = 3; // value
pub const VKS_INV_TASK_SET_XFER_ID: u32 = 4; // id
pub const VKS_INV_TASK_SET_DIRTY: u32 = 5; // value
pub const VKS_INV_TASK_DELETE_INVENTORY: u32 = 6;
pub const VKS_INV_TASK_RESET_INVENTORY: u32 = 7;
pub const VKS_INV_TASK_PUSH_FRONT_ITEM: u32 = 8; // value: index into items
pub const VKS_INV_TASK_PUSH_FRONT_OBJECT: u32 = 9; // value: index into objects
pub const VKS_INV_TASK_PUSH_FRONT_REPLY_CONTENTS: u32 = 10;
pub const VKS_INV_TASK_ERASE: u32 = 11; // value: index
pub const VKS_INV_TASK_PENDING_PUSH: u32 = 12; // uuid
pub const VKS_INV_TASK_PENDING_ERASE: u32 = 13; // value: index
pub const VKS_INV_TASK_NOTIFY_LISTENERS: u32 = 14;
pub const VKS_INV_TASK_SEND: u32 = 15; // value: index into messages
pub const VKS_INV_TASK_LAUNCH_CAP_FETCH: u32 = 16;
pub const VKS_INV_TASK_HTTP_GET: u32 = 17; // text: url
pub const VKS_INV_TASK_LAUNCH_DELAYED_FETCH: u32 = 18; // seconds
pub const VKS_INV_TASK_REQUEST_XFER: u32 = 19; // text: file name, value: serial
pub const VKS_INV_TASK_ABORT_XFER: u32 = 20; // id
pub const VKS_INV_TASK_REMOVE_CACHE_FILE: u32 = 21; // text: file name
pub const VKS_INV_TASK_HIDE_FLOATER: u32 = 22; // text: name; value 0: key uuid, 1: {id: uuid, object: uuid2}
pub const VKS_INV_TASK_LAUNCH_CREATE: u32 = 23; // text: url, text2: binary LLSD body
pub const VKS_INV_TASK_CREATE_CALLBACK: u32 = 24; // value: success
pub const VKS_INV_TASK_CREATE_FAILED: u32 = 25; // text: message
pub const VKS_INV_TASK_ADD_EXPECTED: u32 = 26; // value (wrapping S16 add)
pub const VKS_INV_TASK_NOTIFY_LISTENERS_KEEP_REQUEST: u32 = 27;
pub const VKS_INV_TASK_CLEAR_LOADED_PENDING: u32 = 28;

/// One action.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvTaskAction {
    pub kind: u32,
    pub value: i32,
    pub id: u64,
    pub seconds: f64,
    pub text: VksInvBytes,
    pub text2: VksInvBytes,
    pub uuid: Uuid,
    pub uuid2: Uuid,
}

/// Message kinds of [`VksInvTaskMessage`].
pub const VKS_INV_TASK_MSG_REQUEST_TASK_INVENTORY: u32 = 1;
pub const VKS_INV_TASK_MSG_UPDATE_TASK_INVENTORY: u32 = 2;
pub const VKS_INV_TASK_MSG_REMOVE_TASK_INVENTORY: u32 = 3;
pub const VKS_INV_TASK_MSG_MOVE_TASK_INVENTORY: u32 = 4;
pub const VKS_INV_TASK_MSG_REZ_SCRIPT: u32 = 5;

/// A message to send reliably to the region: every value the C++ sender
/// adds. `item` is the `InventoryData`/`InventoryBlock` of
/// `LLViewerInventoryItem::packMessage` (the transaction id in the `asset`
/// slot, the CRC computed); unused fields are zero.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvTaskMessage {
    pub kind: u32,
    pub local_id: u32,
    pub agent: Uuid,
    pub session: Uuid,
    pub group: Uuid,
    pub folder_id: Uuid,
    pub item_id: Uuid,
    pub template_id: Uuid,
    pub key: u8,
    pub enabled: bool,
    pub has_template: bool,
    pub item: VksInvItemBlock,
}

/// What a decision returned. Every pointer points into the block and lives
/// until `vks_inv_task_result_free`.
#[repr(C)]
pub struct VksInvTaskResultView {
    pub actions: *const VksInvTaskAction,
    pub action_count: usize,
    pub items: *const VksInvTaskItem,
    pub item_count: usize,
    pub objects: *const VksInvObject,
    pub object_count: usize,
    pub messages: *const VksInvTaskMessage,
    pub message_count: usize,
    /// `loadTaskInvFile`'s result.
    pub ok: bool,
}

const _: () = {
    use std::mem::size_of;
    if size_of::<usize>() == 8 {
        assert!(size_of::<VksInvTaskEntry>() == 48);
        assert!(size_of::<VksInvTaskView>() == 80);
        assert!(size_of::<VksInvTaskEnv>() == 128);
        assert!(size_of::<VksInvTaskItem>() == 256);
        assert!(size_of::<VksInvTaskAction>() == 88);
        assert!(size_of::<VksInvTaskMessage>() == 280);
    }
};

/// A result block.
pub struct VksInvTaskResult {
    actions: Vec<VksInvTaskAction>,
    items: Vec<ViewerItem>,
    objects: Vec<Object>,
    messages: Vec<Outgoing>,
    texts: Vec<Vec<u8>>,
    item_views: Vec<VksInvTaskItem>,
    object_views: Vec<VksInvObject>,
    message_views: Vec<VksInvTaskMessage>,
    view: VksInvTaskResultView,
}

const ZERO_BYTES: VksInvBytes = VksInvBytes {
    ptr: ptr::null(),
    len: 0,
};

fn action(kind: u32) -> VksInvTaskAction {
    VksInvTaskAction {
        kind,
        value: 0,
        id: 0,
        seconds: 0.0,
        text: ZERO_BYTES,
        text2: ZERO_BYTES,
        uuid: [0; 16],
        uuid2: [0; 16],
    }
}

fn message_view(m: &Outgoing) -> VksInvTaskMessage {
    let zero_block = VksInvItemBlock {
        item_id: [0; 16],
        folder_id: [0; 16],
        creator_id: [0; 16],
        owner_id: [0; 16],
        group_id: [0; 16],
        base_mask: 0,
        owner_mask: 0,
        group_mask: 0,
        everyone_mask: 0,
        next_owner_mask: 0,
        group_owned: false,
        asset_type: 0,
        inv_type: 0,
        sale_type: 0,
        asset: [0; 16],
        flags: 0,
        sale_price: 0,
        creation_date: 0,
        crc: 0,
        name: ZERO_BYTES,
        description: ZERO_BYTES,
    };
    let mut v = VksInvTaskMessage {
        kind: 0,
        local_id: 0,
        agent: [0; 16],
        session: [0; 16],
        group: [0; 16],
        folder_id: [0; 16],
        item_id: [0; 16],
        template_id: [0; 16],
        key: 0,
        enabled: false,
        has_template: false,
        item: zero_block,
    };
    let block = |b: &vkstorm_inventory::message::ItemBlock| VksInvItemBlock {
        item_id: b.item_id,
        folder_id: b.folder_id,
        creator_id: b.creator_id,
        owner_id: b.owner_id,
        group_id: b.group_id,
        base_mask: b.base_mask,
        owner_mask: b.owner_mask,
        group_mask: b.group_mask,
        everyone_mask: b.everyone_mask,
        next_owner_mask: b.next_owner_mask,
        group_owned: b.group_owned,
        asset_type: b.ty,
        inv_type: b.inv_type,
        sale_type: b.sale_type,
        asset: b.asset,
        flags: b.flags,
        sale_price: b.sale_price,
        creation_date: b.creation_date,
        crc: b.crc,
        name: VksInvBytes::of(&b.name),
        description: VksInvBytes::of(&b.description),
    };
    match m {
        Outgoing::RequestTaskInventory {
            agent,
            session,
            local_id,
        } => {
            v.kind = VKS_INV_TASK_MSG_REQUEST_TASK_INVENTORY;
            v.agent = *agent;
            v.session = *session;
            v.local_id = *local_id;
        }
        Outgoing::UpdateTaskInventory {
            agent,
            session,
            local_id,
            key,
            item,
        } => {
            v.kind = VKS_INV_TASK_MSG_UPDATE_TASK_INVENTORY;
            v.agent = *agent;
            v.session = *session;
            v.local_id = *local_id;
            v.key = *key;
            v.item = block(item);
        }
        Outgoing::RemoveTaskInventory {
            agent,
            session,
            local_id,
            item_id,
        } => {
            v.kind = VKS_INV_TASK_MSG_REMOVE_TASK_INVENTORY;
            v.agent = *agent;
            v.session = *session;
            v.local_id = *local_id;
            v.item_id = *item_id;
        }
        Outgoing::MoveTaskInventory {
            agent,
            session,
            folder_id,
            local_id,
            item_id,
        } => {
            v.kind = VKS_INV_TASK_MSG_MOVE_TASK_INVENTORY;
            v.agent = *agent;
            v.session = *session;
            v.folder_id = *folder_id;
            v.local_id = *local_id;
            v.item_id = *item_id;
        }
        Outgoing::RezScript {
            agent,
            session,
            group,
            local_id,
            enabled,
            item,
            template,
        } => {
            v.kind = VKS_INV_TASK_MSG_REZ_SCRIPT;
            v.agent = *agent;
            v.session = *session;
            v.group = *group;
            v.local_id = *local_id;
            v.enabled = *enabled;
            v.item = block(item);
            if let Some(t) = template {
                v.has_template = true;
                v.template_id = *t;
            }
        }
    }
    v
}

impl VksInvTaskResult {
    /// Lays the actions out as records. The strings are collected first and
    /// the views built last, so every pointer stays valid (the vectors are
    /// not touched again).
    fn new(actions: Vec<Action>, ok: bool) -> VksInvTaskResult {
        let mut r = VksInvTaskResult {
            actions: Vec::with_capacity(actions.len()),
            items: Vec::new(),
            objects: Vec::new(),
            messages: Vec::new(),
            texts: Vec::new(),
            item_views: Vec::new(),
            object_views: Vec::new(),
            message_views: Vec::new(),
            view: VksInvTaskResultView {
                actions: ptr::null(),
                action_count: 0,
                items: ptr::null(),
                item_count: 0,
                objects: ptr::null(),
                object_count: 0,
                messages: ptr::null(),
                message_count: 0,
                ok,
            },
        };
        // Text slots: (action index, which, text index), resolved at the end.
        let mut text_slots: Vec<(usize, bool, usize)> = Vec::new();
        for a in actions {
            let i = r.actions.len();
            let mut text = |r: &mut VksInvTaskResult, second: bool, t: Vec<u8>| {
                text_slots.push((i, second, r.texts.len()));
                r.texts.push(t);
            };
            let rec = match a {
                Action::SetSerial(v) => VksInvTaskAction {
                    value: i32::from(v),
                    ..action(VKS_INV_TASK_SET_SERIAL)
                },
                Action::SetExpected(v) => VksInvTaskAction {
                    value: i32::from(v),
                    ..action(VKS_INV_TASK_SET_EXPECTED)
                },
                Action::AddExpected(v) => VksInvTaskAction {
                    value: i32::from(v),
                    ..action(VKS_INV_TASK_ADD_EXPECTED)
                },
                Action::NotifyListenersKeepRequest => {
                    action(VKS_INV_TASK_NOTIFY_LISTENERS_KEEP_REQUEST)
                }
                Action::ClearLoadedPending => action(VKS_INV_TASK_CLEAR_LOADED_PENDING),
                Action::SetRequestState(v) => VksInvTaskAction {
                    value: request_to(v),
                    ..action(VKS_INV_TASK_SET_REQUEST_STATE)
                },
                Action::SetXferId(v) => VksInvTaskAction {
                    id: v,
                    ..action(VKS_INV_TASK_SET_XFER_ID)
                },
                Action::SetDirty(v) => VksInvTaskAction {
                    value: i32::from(v),
                    ..action(VKS_INV_TASK_SET_DIRTY)
                },
                Action::DeleteInventory => action(VKS_INV_TASK_DELETE_INVENTORY),
                Action::ResetInventory => action(VKS_INV_TASK_RESET_INVENTORY),
                Action::PushFront(Entry::Item(v)) => {
                    r.items.push(v);
                    VksInvTaskAction {
                        value: (r.items.len() - 1) as i32,
                        ..action(VKS_INV_TASK_PUSH_FRONT_ITEM)
                    }
                }
                Action::PushFront(Entry::Object(o)) => {
                    r.objects.push(o);
                    VksInvTaskAction {
                        value: (r.objects.len() - 1) as i32,
                        ..action(VKS_INV_TASK_PUSH_FRONT_OBJECT)
                    }
                }
                Action::PushFrontReplyContents => action(VKS_INV_TASK_PUSH_FRONT_REPLY_CONTENTS),
                Action::Erase(n) => VksInvTaskAction {
                    value: n as i32,
                    ..action(VKS_INV_TASK_ERASE)
                },
                Action::PendingPush(id) => VksInvTaskAction {
                    uuid: id,
                    ..action(VKS_INV_TASK_PENDING_PUSH)
                },
                Action::PendingErase(n) => VksInvTaskAction {
                    value: n as i32,
                    ..action(VKS_INV_TASK_PENDING_ERASE)
                },
                Action::NotifyListeners => action(VKS_INV_TASK_NOTIFY_LISTENERS),
                Action::Send(m) => {
                    r.messages.push(m);
                    VksInvTaskAction {
                        value: (r.messages.len() - 1) as i32,
                        ..action(VKS_INV_TASK_SEND)
                    }
                }
                Action::LaunchCapFetch => action(VKS_INV_TASK_LAUNCH_CAP_FETCH),
                Action::HttpGet { url } => {
                    text(&mut r, false, url.into_bytes());
                    action(VKS_INV_TASK_HTTP_GET)
                }
                Action::LaunchDelayedFetch { seconds } => VksInvTaskAction {
                    seconds,
                    ..action(VKS_INV_TASK_LAUNCH_DELAYED_FETCH)
                },
                Action::RequestXfer { filename, serial } => {
                    text(&mut r, false, filename);
                    VksInvTaskAction {
                        value: i32::from(serial),
                        ..action(VKS_INV_TASK_REQUEST_XFER)
                    }
                }
                Action::AbortXfer { id } => VksInvTaskAction {
                    id,
                    ..action(VKS_INV_TASK_ABORT_XFER)
                },
                Action::RemoveCacheFile { filename } => {
                    text(&mut r, false, filename);
                    action(VKS_INV_TASK_REMOVE_CACHE_FILE)
                }
                Action::HideFloater { name, key } => {
                    text(&mut r, false, name.as_bytes().to_vec());
                    match key {
                        FloaterKey::Id(id) => VksInvTaskAction {
                            uuid: id,
                            ..action(VKS_INV_TASK_HIDE_FLOATER)
                        },
                        FloaterKey::ItemAndObject { id, object } => VksInvTaskAction {
                            value: 1,
                            uuid: id,
                            uuid2: object,
                            ..action(VKS_INV_TASK_HIDE_FLOATER)
                        },
                    }
                }
                Action::LaunchCreate { url, body } => {
                    text(&mut r, false, url.into_bytes());
                    let (bin, _) = vkstorm_llsd::format(Format::Binary, &body, Options::default());
                    text(&mut r, true, bin);
                    action(VKS_INV_TASK_LAUNCH_CREATE)
                }
                Action::CreateCallback { success } => VksInvTaskAction {
                    value: i32::from(success),
                    ..action(VKS_INV_TASK_CREATE_CALLBACK)
                },
                Action::CreateFailed { message } => {
                    text(&mut r, false, message.as_bytes().to_vec());
                    action(VKS_INV_TASK_CREATE_FAILED)
                }
            };
            r.actions.push(rec);
        }
        for (i, second, t) in text_slots {
            let b = VksInvBytes::of(&r.texts[t]);
            if second {
                r.actions[i].text2 = b;
            } else {
                r.actions[i].text = b;
            }
        }
        r.item_views = r.items.iter().map(viewer_item_view).collect();
        r.object_views = r.objects.iter().map(object_view).collect();
        r.message_views = r.messages.iter().map(message_view).collect();
        let v = &mut r.view;
        v.actions = r.actions.as_ptr();
        v.action_count = r.actions.len();
        v.items = r.item_views.as_ptr();
        v.item_count = r.item_views.len();
        v.objects = r.object_views.as_ptr();
        v.object_count = r.object_views.len();
        v.messages = r.message_views.as_ptr();
        v.message_count = r.message_views.len();
        r
    }

    fn into_raw(self) -> *mut VksInvTaskResult {
        Box::into_raw(Box::new(self))
    }
}

/// Runs a decision over the inputs; null (with `vks_inv_last_error`) when
/// it panics.
unsafe fn decide(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    f: impl FnOnce(&TaskView<'_, Key>, &Env<'_>) -> (bool, Vec<Action>),
) -> *mut VksInvTaskResult {
    guard(ptr::null_mut(), || {
        let inputs = unsafe { Inputs::read(view, env) };
        let (ok, actions) = f(&inputs.view(), &inputs.env());
        VksInvTaskResult::new(actions, ok).into_raw()
    })
}

/// The view of a result block.
///
/// # Safety
/// `r` must come from a `vks_inv_task_*` decision and be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_result_view(
    r: *const VksInvTaskResult,
) -> *const VksInvTaskResultView {
    if r.is_null() {
        ptr::null()
    } else {
        unsafe { &(*r).view }
    }
}

/// Releases a result block. Null is ignored.
///
/// # Safety
/// `r` must come from a `vks_inv_task_*` decision and not be used
/// afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_result_free(r: *mut VksInvTaskResult) {
    if !r.is_null() {
        drop(unsafe { Box::from_raw(r) });
    }
}

// ---------------------------------------------------------------------------
// The decisions. Every one takes a valid view and env (see their records)
// and returns a result block, null on a failure.

/// `requestInventory`.
///
/// # Safety
/// `view` and `env` must be valid, with their arrays and strings readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_request_inventory(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
) -> *mut VksInvTaskResult {
    unsafe { decide(view, env, |v, e| (true, task::request_inventory(v, e))) }
}

/// `dirtyInventory`.
///
/// # Safety
/// As [`vks_inv_task_request_inventory`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_dirty_inventory(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
) -> *mut VksInvTaskResult {
    unsafe { decide(view, env, |v, e| (true, task::dirty_inventory(v, e))) }
}

/// `deleteInventoryItem(item_id)`.
///
/// # Safety
/// As [`vks_inv_task_request_inventory`]; `item_id` 16 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_delete_inventory_item(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    item_id: *const u8,
) -> *mut VksInvTaskResult {
    let id = unsafe { uuid_arg(item_id) };
    unsafe {
        decide(view, env, |v, e| {
            (true, task::delete_inventory_item(v, e, &id))
        })
    }
}

/// `processTaskInv` for an object `gObjectList` found: the message's
/// `Serial` and `Filename` (as read, before scrubbing). A
/// `VKS_INV_TASK_REQUEST_XFER` action is continued by
/// `vks_inv_task_xfer_requested` with the id `requestFile` returned.
///
/// # Safety
/// As [`vks_inv_task_request_inventory`]; `filename` readable for `len`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_reply(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    serial: i16,
    filename: *const u8,
    len: usize,
) -> *mut VksInvTaskResult {
    let name = unsafe { bytes(filename, len) };
    unsafe {
        decide(view, env, |v, e| {
            (true, task::reply_task_inventory(v, e, serial, name))
        })
    }
}

/// The rest of `processTaskInv` once `requestFile` returned `new_id`.
///
/// # Safety
/// As [`vks_inv_task_request_inventory`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_xfer_requested(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    new_id: u64,
) -> *mut VksInvTaskResult {
    unsafe { decide(view, env, |v, e| (true, task::xfer_requested(v, e, new_id))) }
}

/// `processTaskInvFile` for an object `gObjectList` found: the transfer's
/// serial and file name, its error code, and the cache file's bytes as the
/// text-mode stream reads them (`has_file` false when it cannot be opened).
///
/// # Safety
/// As [`vks_inv_task_request_inventory`]; `filename` and `file` readable
/// for their lengths.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn vks_inv_task_xfer_completed(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    serial: i16,
    filename: *const u8,
    filename_len: usize,
    error_code: i32,
    has_file: bool,
    file: *const u8,
    file_len: usize,
) -> *mut VksInvTaskResult {
    let name = unsafe { bytes(filename, filename_len) };
    let data = has_file.then(|| unsafe { bytes(file, file_len) });
    unsafe {
        decide(view, env, |v, e| {
            (
                true,
                task::xfer_completed(v, e, serial, name, error_code, data),
            )
        })
    }
}

/// The pending-list cleanup after a successful load
/// (`VKS_INV_TASK_CLEAR_LOADED_PENDING`), over the object as it is now.
///
/// # Safety
/// As [`vks_inv_task_request_inventory`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_clear_loaded_pending(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
) -> *mut VksInvTaskResult {
    unsafe { decide(view, env, |v, e| (true, task::clear_loaded_pending(v, e))) }
}

/// `loadTaskInvFile(filename)` with the cache file's bytes as the text-mode
/// stream reads them (`has_file` false when it cannot be opened); `ok` is
/// its result.
///
/// # Safety
/// As [`vks_inv_task_xfer_completed`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_load_file(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    filename: *const u8,
    filename_len: usize,
    has_file: bool,
    file: *const u8,
    file_len: usize,
) -> *mut VksInvTaskResult {
    let name = unsafe { bytes(filename, filename_len) };
    let data = has_file.then(|| unsafe { bytes(file, file_len) });
    unsafe { decide(view, env, |v, e| task::load_task_inv_file(v, e, name, data)) }
}

/// `fetchInventoryFromCapCoro` up to its suspension, for an object
/// `gObjectList` found; ends with `VKS_INV_TASK_HTTP_GET`.
///
/// # Safety
/// As [`vks_inv_task_request_inventory`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_cap_fetch_begin(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
) -> *mut VksInvTaskResult {
    unsafe { decide(view, env, |v, e| (true, task::cap_fetch_begin(v, e))) }
}

/// `fetchInventoryFromCapCoro` after the reply, for an object `gObjectList`
/// still has: `success` the status's verdict, `http_status` its type,
/// `serial` `result["inventory_serial"].asInteger()`, `has_contents`
/// `result.has("contents")`.
///
/// # Safety
/// As [`vks_inv_task_request_inventory`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_cap_fetch_reply(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    success: bool,
    http_status: i32,
    serial: i32,
    has_contents: bool,
) -> *mut VksInvTaskResult {
    let reply = CapReply {
        serial,
        has_contents,
    };
    unsafe {
        decide(view, env, |v, e| {
            (
                true,
                task::cap_fetch_reply_summary(v, e, success, http_status, reply),
            )
        })
    }
}

/// `fetchInventoryDelayedCoro` after its timeout, for an object
/// `gObjectList` still has.
///
/// # Safety
/// As [`vks_inv_task_request_inventory`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_delayed_fetch_due(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
) -> *mut VksInvTaskResult {
    unsafe { decide(view, env, |v, e| (true, task::delayed_fetch_due(v, e))) }
}

/// `updateInventory(item, key, is_new)`; `item` as its getters return it.
///
/// # Safety
/// As [`vks_inv_task_request_inventory`]; `item` valid with its strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_update_inventory(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    item: *const VksInvTaskItem,
    key: u8,
    is_new: bool,
) -> *mut VksInvTaskResult {
    unsafe {
        decide(view, env, |v, e| {
            let it = viewer_item_from(&*item);
            (true, task::update_inventory(v, e, &it, key, is_new))
        })
    }
}

/// `updateInventoryLocal(item, key)`; `item` as its getters return it.
///
/// # Safety
/// As [`vks_inv_task_update_inventory`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_update_inventory_local(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    item: *const VksInvItem,
    key: u8,
) -> *mut VksInvTaskResult {
    unsafe {
        decide(view, env, |v, e| {
            let it: Item = item_from(&*item);
            (true, task::update_inventory_local(v, e, &it, key))
        })
    }
}

/// `updateViewerInventoryAsset(item, new_asset)`; `item` its fields (the
/// copy constructor copies them).
///
/// # Safety
/// As [`vks_inv_task_update_inventory`]; `new_asset` 16 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_update_viewer_inventory_asset(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    item: *const VksInvTaskItem,
    new_asset: *const u8,
) -> *mut VksInvTaskResult {
    let asset = unsafe { uuid_arg(new_asset) };
    unsafe {
        decide(view, env, |v, e| {
            let it = viewer_item_from(&*item);
            (true, task::update_viewer_inventory_asset(v, e, &it, &asset))
        })
    }
}

/// `updateMaterialInventory(item, key, is_new)` for a non-null item.
///
/// # Safety
/// As [`vks_inv_task_update_inventory`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_update_material_inventory(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    item: *const VksInvTaskItem,
    key: u8,
    is_new: bool,
) -> *mut VksInvTaskResult {
    unsafe {
        decide(view, env, |v, e| {
            let it = viewer_item_from(&*item);
            (
                true,
                task::update_material_inventory(v, e, &it, key, is_new),
            )
        })
    }
}

/// `removeInventory(item_id)`.
///
/// # Safety
/// As [`vks_inv_task_delete_inventory_item`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_remove_inventory(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    item_id: *const u8,
) -> *mut VksInvTaskResult {
    let id = unsafe { uuid_arg(item_id) };
    unsafe { decide(view, env, |v, e| (true, task::remove_inventory(v, e, &id))) }
}

/// `moveInventory(folder_id, item_id)`.
///
/// # Safety
/// As [`vks_inv_task_delete_inventory_item`]; `folder_id` 16 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_move_inventory(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    folder_id: *const u8,
    item_id: *const u8,
) -> *mut VksInvTaskResult {
    let folder = unsafe { uuid_arg(folder_id) };
    let id = unsafe { uuid_arg(item_id) };
    unsafe {
        decide(view, env, |v, e| {
            (true, task::move_inventory(v, e, &folder, &id))
        })
    }
}

/// `saveScript(item, active, is_new, template_id)`; `item` as its getters
/// return it.
///
/// # Safety
/// As [`vks_inv_task_update_inventory`]; `template_id` 16 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_save_script(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    item: *const VksInvTaskItem,
    active: bool,
    is_new: bool,
    template_id: *const u8,
) -> *mut VksInvTaskResult {
    let template = unsafe { uuid_arg(template_id) };
    unsafe {
        decide(view, env, |v, e| {
            let it = viewer_item_from(&*item);
            (
                true,
                task::save_script(v, e, &it, active, is_new, &template),
            )
        })
    }
}

/// `createInventoryItem(...)`: `params` as binary LLSD (`has_params` false
/// for an undefined `LLSD`). The `VKS_INV_TASK_LAUNCH_CREATE` action's
/// `text2` is the POST body as binary LLSD.
///
/// # Safety
/// As [`vks_inv_task_request_inventory`]; the strings readable for their
/// lengths, `perms` valid.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn vks_inv_task_create_inventory_item(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    asset_type: i32,
    inventory_type: i32,
    sub_type: u8,
    name: *const u8,
    name_len: usize,
    description: *const u8,
    description_len: usize,
    perms: *const VksInvPerms,
    has_params: bool,
    params: *const u8,
    params_len: usize,
) -> *mut VksInvTaskResult {
    let name = unsafe { bytes(name, name_len) };
    let description = unsafe { bytes(description, description_len) };
    let params_bin = unsafe { bytes(params, params_len) };
    unsafe {
        decide(view, env, |v, e| {
            let params = if has_params {
                let (_, value) = vkstorm_llsd::parse(
                    Format::Binary,
                    params_bin,
                    Limits::for_len(params_bin.len()),
                );
                value
            } else {
                None
            };
            let perms: Permissions = *perms;
            let req = CreateRequest {
                asset_type: AssetType(asset_type),
                inventory_type: InventoryType(inventory_type),
                sub_type,
                name,
                description,
                permissions: &perms,
                params: params.as_ref(),
            };
            (true, task::create_inventory_item(v, e, &req))
        })
    }
}

/// `createInventoryItemCoro` after the reply: `http_ok` the status's
/// verdict, `result_success` `result["success"].asBoolean()`; `view` null
/// when `gObjectList` no longer has the object. Ends with
/// `VKS_INV_TASK_CREATE_CALLBACK`.
///
/// # Safety
/// `env` valid; `view` null or valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_create_item_completed(
    view: *const VksInvTaskView,
    env: *const VksInvTaskEnv,
    http_ok: bool,
    result_success: bool,
) -> *mut VksInvTaskResult {
    guard(ptr::null_mut(), || {
        let mut result = Value::map();
        if let Value::Map(m) = &mut result {
            m.insert(b"success".to_vec(), Value::Boolean(result_success));
        }
        let success = task::create_item_succeeded(http_ok, &result);
        let actions = if view.is_null() {
            let empty = VksInvTaskView {
                id: [0; 16],
                local_id: 0,
                serial: 0,
                expected: 0,
                request: 0,
                xfer_id: 0,
                dirty: false,
                has_inventory: false,
                has_listeners: false,
                entries: ptr::null(),
                entry_count: 0,
                pending: ptr::null(),
                pending_count: 0,
            };
            let inputs = unsafe { Inputs::read(&empty, env) };
            task::create_item_completed::<Key>(None, &inputs.env(), success)
        } else {
            let inputs = unsafe { Inputs::read(view, env) };
            task::create_item_completed(Some(&inputs.view()), &inputs.env(), success)
        };
        VksInvTaskResult::new(actions, success).into_raw()
    })
}

// ---------------------------------------------------------------------------
// The lookups over the contents (no result block).

/// Lookup kinds of [`vks_inv_task_find`].
pub const VKS_INV_TASK_FIND_OBJECT: u32 = 1; // getInventoryObject(id)
pub const VKS_INV_TASK_FIND_ITEM: u32 = 2; // getInventoryItem(id)
pub const VKS_INV_TASK_FIND_ITEM_BY_ASSET: u32 = 3; // getInventoryItemByAsset(id)
pub const VKS_INV_TASK_FIND_ITEM_BY_ASSET_AND_TYPE: u32 = 4; // getInventoryItemByAsset(id, type)
pub const VKS_INV_TASK_FIND_ROOT: u32 = 5; // getInventoryRoot()

unsafe fn entries<'a>(p: *const VksInvTaskEntry, count: usize) -> &'a [VksInvTaskEntry] {
    if p.is_null() || count == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(p, count) }
    }
}

/// The index of the entry a lookup finds, -1 for none.
///
/// # Safety
/// `entries` readable for `count` keys; `id` 16 bytes or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_find(
    entries_ptr: *const VksInvTaskEntry,
    count: usize,
    kind: u32,
    id: *const u8,
    asset_type: i32,
) -> i64 {
    guard(-1, || {
        let e = unsafe { entries(entries_ptr, count) };
        let id = unsafe { uuid_arg(id) };
        let found = match kind {
            VKS_INV_TASK_FIND_OBJECT => lookup::object_by_id(e, &id),
            VKS_INV_TASK_FIND_ITEM => lookup::item_by_id(e, &id),
            VKS_INV_TASK_FIND_ITEM_BY_ASSET => lookup::item_by_asset(e, &id),
            VKS_INV_TASK_FIND_ITEM_BY_ASSET_AND_TYPE => {
                lookup::item_by_asset_and_type(e, &id, AssetType(asset_type))
            }
            VKS_INV_TASK_FIND_ROOT => lookup::root(e),
            _ => None,
        };
        found.map_or(-1, |i| i as i64)
    })
}

/// `countInventoryContents(type)`.
///
/// # Safety
/// As [`vks_inv_task_find`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_count_of_type(
    entries_ptr: *const VksInvTaskEntry,
    count: usize,
    asset_type: i32,
) -> usize {
    guard(0, || {
        lookup::count_of_type(
            unsafe { entries(entries_ptr, count) },
            AssetType(asset_type),
        )
    })
}

/// `getInventoryContents`: the indices of the entries it returns, in order,
/// into `out` (room for `count`); returns how many.
///
/// # Safety
/// As [`vks_inv_task_find`]; `out` writable for `count` indices.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_contents(
    entries_ptr: *const VksInvTaskEntry,
    count: usize,
    out: *mut usize,
) -> usize {
    guard(0, || {
        let found = lookup::contents(unsafe { entries(entries_ptr, count) });
        if !out.is_null() {
            for (k, i) in found.iter().enumerate() {
                unsafe { *out.add(k) = *i };
            }
        }
        found.len()
    })
}

/// `isAssetInInventory(item, type)` for an item with `asset`.
///
/// # Safety
/// As [`vks_inv_task_find`]; `pending` readable for `pending_count` ids.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_task_is_asset_in_inventory(
    entries_ptr: *const VksInvTaskEntry,
    count: usize,
    has_inventory: bool,
    pending: *const u8,
    pending_count: usize,
    asset: *const u8,
    asset_type: i32,
) -> bool {
    guard(false, || {
        let e = unsafe { entries(entries_ptr, count) };
        let pend: Vec<Uuid> = unsafe { bytes(pending, pending_count * 16) }
            .chunks_exact(16)
            .map(|c| {
                let mut u = [0u8; 16];
                u.copy_from_slice(c);
                u
            })
            .collect();
        lookup::is_asset_in_inventory(
            has_inventory.then_some(e),
            &pend,
            &unsafe { uuid_arg(asset) },
            AssetType(asset_type),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(entries: &[VksInvTaskEntry]) -> VksInvTaskView {
        VksInvTaskView {
            id: [7; 16],
            local_id: 42,
            serial: 0,
            expected: 0,
            request: VKS_INV_TASK_REQUEST_PENDING,
            xfer_id: 0,
            dirty: false,
            has_inventory: true,
            has_listeners: true,
            entries: entries.as_ptr(),
            entry_count: entries.len(),
            pending: ptr::null(),
            pending_count: 0,
        }
    }

    fn env() -> VksInvTaskEnv {
        VksInvTaskEnv {
            agent: [1; 16],
            session: [2; 16],
            group: [3; 16],
            has_region: true,
            cap_request_task_inventory: ZERO_BYTES,
            cap_create_task_inventory_item: ZERO_BYTES,
            you_owner: true,
            has_selection: false,
            selection_owner: [0; 16],
            selection_group: [0; 16],
        }
    }

    unsafe fn kinds(r: *mut VksInvTaskResult) -> Vec<u32> {
        let v = unsafe { &*vks_inv_task_result_view(r) };
        let a = unsafe { slice::from_raw_parts(v.actions, v.action_count) };
        a.iter().map(|a| a.kind).collect()
    }

    /// A task file crosses whole and comes back as records, in the order
    /// the loader pushes them (file order), followed by the notification.
    #[test]
    fn load_file_returns_records_in_push_order() {
        let file = b"\tinv_object\t0\n\t{\n\t\tobj_id\t00000000-0000-0000-0000-000000000007\n\t\tparent_id\t00000000-0000-0000-0000-000000000000\n\t\ttype\tcategory\n\t\tname\tContents|\n\t}\n\tinv_item\t0\n\t{\n\t\titem_id\t00000000-0000-0000-0000-000000000009\n\t\tparent_id\t00000000-0000-0000-0000-000000000007\n\t\ttype\tnotecard\n\t\tinv_type\tnotecard\n\t\tname\tNote|\n\t\tdesc\t|\n\t}\n";
        let v = view(&[]);
        let e = env();
        let name = b"task.inv";
        unsafe {
            let r = vks_inv_task_load_file(
                &v,
                &e,
                name.as_ptr(),
                name.len(),
                true,
                file.as_ptr(),
                file.len(),
            );
            assert!(!r.is_null());
            let rv = &*vks_inv_task_result_view(r);
            assert!(rv.ok);
            assert_eq!(rv.object_count, 1);
            assert_eq!(rv.item_count, 1);
            assert_eq!(
                kinds(r),
                vec![
                    VKS_INV_TASK_RESET_INVENTORY,
                    VKS_INV_TASK_PUSH_FRONT_OBJECT,
                    VKS_INV_TASK_PUSH_FRONT_ITEM,
                    VKS_INV_TASK_REMOVE_CACHE_FILE,
                    VKS_INV_TASK_NOTIFY_LISTENERS,
                    VKS_INV_TASK_SET_XFER_ID,
                    VKS_INV_TASK_SET_REQUEST_STATE,
                ]
            );
            let item = &*rv.items;
            assert_eq!(item.item.object.name.get(), b"Note");
            assert!(item.complete);
            vks_inv_task_result_free(r);

            let r =
                vks_inv_task_load_file(&v, &e, name.as_ptr(), name.len(), false, ptr::null(), 0);
            assert!(!(*vks_inv_task_result_view(r)).ok);
            assert!(kinds(r).is_empty());
            vks_inv_task_result_free(r);
        }
    }

    /// The lookups read the keys: first match, the item rules.
    #[test]
    fn lookups_over_keys() {
        let perms = Permissions::default();
        let key = |id: u8, t: i32, item: bool, asset: u8| VksInvTaskEntry {
            id: [id; 16],
            asset_type: t,
            is_item: item,
            asset: [asset; 16],
            perms: if item { &perms } else { ptr::null() },
        };
        let entries = [
            key(1, 8, false, 0),
            key(2, 7, true, 9),
            key(2, 7, true, 10),
            key(3, 7, false, 9),
        ];
        unsafe {
            let find = |kind, id: u8, t| {
                vks_inv_task_find(entries.as_ptr(), entries.len(), kind, [id; 16].as_ptr(), t)
            };
            assert_eq!(find(VKS_INV_TASK_FIND_OBJECT, 2, 0), 1);
            assert_eq!(find(VKS_INV_TASK_FIND_ITEM, 1, 0), -1);
            assert_eq!(find(VKS_INV_TASK_FIND_ITEM_BY_ASSET, 10, 0), 2);
            assert_eq!(find(VKS_INV_TASK_FIND_ITEM_BY_ASSET_AND_TYPE, 9, 7), 1);
            assert_eq!(find(VKS_INV_TASK_FIND_ROOT, 0, 0), 3);
            assert_eq!(
                vks_inv_task_count_of_type(entries.as_ptr(), entries.len(), 7),
                3
            );
            let mut out = [0usize; 4];
            assert_eq!(
                vks_inv_task_contents(entries.as_ptr(), entries.len(), out.as_mut_ptr()),
                3
            );
            assert_eq!(&out[..3], &[1, 2, 3]);
        }
    }
}
