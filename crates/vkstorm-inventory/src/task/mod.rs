//! Object contents ("task inventory"): the decisions of `LLViewerObject`'s
//! task-inventory code (`llviewerobject.cpp`) and the `LLViewerInventoryItem`
//! members it uses (`llviewerinventory.cpp`), for the inventory rung
//! (vkstorm-rust rung seven, sub-rung R4a).
//!
//! What is here:
//!
//! - the task inventory file loader (`loadTaskInvFile`: the legacy text
//!   loop over the item and object readers, [`file`]);
//! - the `RequestTaskInventory` capability reply (`loadTaskInvLLSD` and the
//!   reply half of `fetchInventoryFromCapCoro`, [`llsd`]);
//! - the serial and request-state machine: `requestInventory`,
//!   `fetchInventoryFromServer`, `fetchInventoryDelayed` and its coroutine,
//!   `processTaskInv`, `processTaskInvFile`, `doInventoryCallback`, and the
//!   local mutations with the owner guess of `doUpdateInventory`
//!   ([`machine`]);
//! - the lookups (`getInventoryObject`, `getInventoryItemByAsset`, ...,
//!   [`lookup`]);
//! - the messages the viewer sends (`RequestTaskInventory`,
//!   `UpdateTaskInventory`, `RemoveTaskInventory`, `MoveTaskInventory`,
//!   `RezScript`) as flat records with a packer over `vkstorm-messages`
//!   ([`message`]), and the `CreateTaskInventoryItem` POST body
//!   ([`create`]).
//!
//! What stays C++: `LLViewerObject`'s task-inventory state (`mInventory`
//! and its entries, the serials, `mInvRequestState`, `mInvRequestXFerId`,
//! `mInventoryDirty`, `mPendingInventoryItemsIDs`, `mInventoryCallbacks`),
//! the listener bookkeeping, the xfer manager, the message system and its
//! call sites, the coroutines and the HTTP adapter, `gObjectList`, the
//! selection manager, `permYouOwner`, `LLTrans` and the floaters.
//!
//! # The shape: a view in, actions out
//!
//! Every decision takes a [`TaskView`] (a flat, read-only copy of the
//! object's task-inventory state, with its entries as the C++ getters
//! return them) and an [`Env`] (agent, region capabilities, the owner-guess
//! inputs), and returns the ordered [`Action`]s that the C++ carries out:
//! state assignments, list edits, listener notification, messages, xfers,
//! coroutine launches, file removal, floaters and callbacks. Applying the
//! actions in order to the C++ object reproduces every state change and side
//! effect of the C++ function, in the C++ order.
//!
//! Three decisions need an answer from the C++ in the middle of a C++
//! function; each is split at that point, and the executor makes the second
//! call when it has the answer:
//!
//! - [`Action::RequestXfer`] (`gXferManager->requestFile` returns the new
//!   id): the executor requests the file and calls [`xfer_requested`] with
//!   the id;
//! - [`Action::LaunchCapFetch`] (the coroutine runs at once, under
//!   `launch::dispatch`, to its first suspension): the executor launches it,
//!   and inside it, when `gObjectList` still has the object, calls
//!   [`cap_fetch_begin`] and carries out its actions before it continues
//!   with the next action of the outer list;
//! - [`Action::LaunchDelayedFetch`] and [`Action::LaunchCreate`]: the
//!   coroutine suspends at once (a timer, a POST); on the timer the executor
//!   calls [`delayed_fetch_due`], on the POST's reply [`create_item_completed`].
//!
//! An HTTP reply to [`Action::HttpGet`] goes to [`cap_fetch_reply`], an xfer
//! completion to [`xfer_completed`]; in both the executor first checks that
//! `gObjectList` still has the object (the C++ drops the result otherwise).
//!
//! The rules and the quirks the fixtures record are reproduced as they are
//! (`tests/fixtures/task/README.md`, "Recorded quirks"); where the C++ reads
//! an `LLInventoryObject` through an unchecked cast to
//! `LLViewerInventoryItem` (undefined behaviour, never exercised), the
//! decisions treat the plain object as not an item.

pub mod create;
pub mod file;
pub mod llsd;
pub mod lookup;
pub mod machine;
pub mod message;

pub use create::CreateRequest;
pub use llsd::LocalizedNames;
pub use machine::{
    cap_fetch_begin, cap_fetch_reply, create_inventory_item, create_item_completed, create_item_succeeded,
    delayed_fetch_due, delete_inventory_item, dirty_inventory, load_task_inv_file, move_inventory, remove_inventory,
    reply_task_inventory, request_inventory, save_script, update_inventory, update_inventory_local,
    update_material_inventory, update_viewer_inventory_asset, xfer_completed, xfer_requested,
};
pub use message::Outgoing;

use vkstorm_llsd::Value;

use crate::item::Item;
use crate::object::Object;
use crate::perms::{Permissions, Uuid, NULL_ID};
use crate::sale::SaleInfo;
use crate::types::{AssetType, InventoryType};

/// `TASK_INVENTORY_ITEM_KEY`: an update matched by item id.
pub const TASK_INVENTORY_ITEM_KEY: u8 = 0;
/// `TASK_INVENTORY_ASSET_KEY`: an update matched by asset id.
pub const TASK_INVENTORY_ASSET_KEY: u8 = 1;
/// `MAX_INV_FILE_READ_FAILS`: unknown top-level tokens a file may have.
pub const MAX_INV_FILE_READ_FAILS: u32 = 25;
/// `INVENTORY_UPDATE_WAIT_TIME_DESYNC` (seconds): the delay after a reply
/// that repeats the current serial below the expected one.
pub const INVENTORY_UPDATE_WAIT_TIME_DESYNC: f64 = 5.0;
/// `INVENTORY_UPDATE_WAIT_TIME_OUTDATED` (seconds): the delay after an
/// out-of-date reply or a stale capability reply.
pub const INVENTORY_UPDATE_WAIT_TIME_OUTDATED: f64 = 1.0;
/// `MAX_STRING` (`lldefs.h`, `STD_STRING_BUF_SIZE`): the task file
/// loader's line buffer, so a line holds at most 254 bytes.
pub const MAX_STRING: usize = 255;

/// `EInventoryRequestState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum RequestState {
    /// `INVENTORY_REQUEST_STOPPED`.
    #[default]
    Stopped,
    /// `INVENTORY_REQUEST_PENDING`: requested, no reply yet.
    Pending,
    /// `INVENTORY_REQUEST_WAIT`: a delayed refetch is scheduled.
    Wait,
    /// `INVENTORY_XFER`: a file transfer or a capability request is out.
    Xfer,
}

/// The scalar task-inventory state of an `LLViewerObject`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct TaskState {
    /// `mInventorySerialNum`.
    pub serial: i16,
    /// `mExpectedInventorySerialNum`.
    pub expected: i16,
    /// `mInvRequestState`.
    pub request: RequestState,
    /// `mInvRequestXFerId`.
    pub xfer_id: u64,
    /// `mInventoryDirty`.
    pub dirty: bool,
}

/// An `LLViewerInventoryItem` as a flat record: the `LLInventoryItem`
/// fields and the viewer's own (`mIsComplete`, `mTransactionID`,
/// `mIsRunning`, `mIsFaulted`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ViewerItem {
    /// The `LLInventoryItem` fields.
    pub item: Item,
    /// `mIsComplete`.
    pub complete: bool,
    /// `mTransactionID`.
    pub transaction: Uuid,
    /// `mIsRunning`.
    pub running: bool,
    /// `mIsFaulted`.
    pub faulted: bool,
}

impl Default for ViewerItem {
    /// `LLViewerInventoryItem()`: a default item, not complete.
    fn default() -> Self {
        ViewerItem { item: Item::default(), complete: false, transaction: NULL_ID, running: false, faulted: false }
    }
}

impl ViewerItem {
    /// The full constructor `LLViewerInventoryItem(uuid, parent_uuid, perm,
    /// asset_uuid, type, inv_type, name, desc, sale_info, flags,
    /// creation_date_utc)`: [`Item::new`] with the date cut to `S32`;
    /// complete, no transaction id.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: &Uuid,
        parent: &Uuid,
        permissions: &Permissions,
        asset: &Uuid,
        asset_type: AssetType,
        inv_type: InventoryType,
        name: &[u8],
        desc: &[u8],
        sale_info: &SaleInfo,
        flags: u32,
        creation_date: i64,
    ) -> ViewerItem {
        ViewerItem {
            item: Item::new(
                id,
                parent,
                permissions,
                asset,
                asset_type,
                inv_type,
                name,
                desc,
                sale_info,
                flags,
                creation_date as i32,
            ),
            complete: true,
            ..ViewerItem::default()
        }
    }

    /// `LLViewerInventoryItem(const LLViewerInventoryItem*)`: a default
    /// item, then `copyViewerItem` (`LLInventoryItem::copyItem` and the
    /// viewer fields).
    pub fn copy_from(other: &ViewerItem) -> ViewerItem {
        ViewerItem {
            item: Item::copy_from(&other.item),
            complete: other.complete,
            transaction: other.transaction,
            running: other.running,
            faulted: other.faulted,
        }
    }

    /// The task-side copy the senders make of the caller's item
    /// (`updateInventory`, `updateInventoryLocal`, `saveScript`): the full
    /// constructor over the item's getters, with the object as parent.
    pub fn task_copy(item: &Item, parent: &Uuid) -> ViewerItem {
        ViewerItem::new(
            &item.object.id,
            parent,
            &item.permissions,
            &item.asset,
            item.object.asset_type,
            item.inv_type,
            &item.object.name,
            &item.description,
            &item.sale_info,
            item.flags,
            item.object.creation_date,
        )
    }
}

/// One entry of `mInventory`: an `LLViewerInventoryItem` or a plain
/// `LLInventoryObject` (the `Contents` category, or an `inv_object` of a
/// task file).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Entry {
    Item(ViewerItem),
    Object(Object),
}

impl Entry {
    /// The `LLInventoryObject` part.
    pub fn object(&self) -> &Object {
        match self {
            Entry::Item(v) => &v.item.object,
            Entry::Object(o) => o,
        }
    }

    /// `getUUID`.
    pub fn id(&self) -> Uuid {
        self.object().id
    }

    /// `getType`.
    pub fn asset_type(&self) -> AssetType {
        self.object().asset_type
    }

    /// `dynamic_cast<LLViewerInventoryItem*>`.
    pub fn as_item(&self) -> Option<&ViewerItem> {
        match self {
            Entry::Item(v) => Some(v),
            Entry::Object(_) => None,
        }
    }

    /// The `Contents` category the loaders synthesise:
    /// `LLInventoryObject(object id, null, AT_CATEGORY, "Contents")`.
    pub fn contents_category(object_id: &Uuid) -> Entry {
        Entry::Object(Object::new(object_id, &NULL_ID, AssetType::CATEGORY, b"Contents"))
    }
}

/// The object's task-inventory state as the decisions read it.
///
/// The entries carry what the C++ getters return (for an
/// `LLViewerInventoryItem` link the viewer's getters follow the link into
/// `gInventory`; the wrapper fills the record from them).
#[derive(Clone, Copy, Debug)]
pub struct TaskView<'a> {
    /// `mID`.
    pub id: Uuid,
    /// `mLocalID`.
    pub local_id: u32,
    /// The serials, request state, xfer id and dirty flag.
    pub state: TaskState,
    /// `mInventory` front to back; `None` for NULL.
    pub inventory: Option<&'a [Entry]>,
    /// `mPendingInventoryItemsIDs` front to back.
    pub pending: &'a [Uuid],
    /// `!mInventoryCallbacks.empty()`.
    pub has_listeners: bool,
}

/// `gAgent`'s ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Agent {
    pub id: Uuid,
    pub session: Uuid,
    pub group: Uuid,
}

/// The object's region (`mRegionp`): the capability URLs, each empty when
/// the region has none (`getCapability` returns `""`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Region<'a> {
    /// `RequestTaskInventory`.
    pub request_task_inventory: &'a str,
    /// `CreateTaskInventoryItem`.
    pub create_task_inventory_item: &'a str,
}

/// The owner and group of the selection's permissions for the object
/// (`LLSelectMgr::findObjectPermissions`, when not NULL).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub owner: Uuid,
    pub group: Uuid,
}

/// The inputs from outside the object.
#[derive(Clone, Copy, Debug)]
pub struct Env<'a> {
    pub agent: Agent,
    /// `None` for a NULL `mRegionp`.
    pub region: Option<Region<'a>>,
    /// `permYouOwner()` (the owner guess of `doUpdateInventory`).
    pub you_owner: bool,
    /// `findObjectPermissions(this)` (the owner guess).
    pub selection: Option<Selection>,
    /// `LLPermissions::getIsInOpenSim()` (`fromLLSD` of capability entries).
    pub in_open_sim: bool,
    /// `LLLocalizedInventoryItemsDictionary`, for capability entries.
    pub names: &'a LocalizedNames,
}

/// The key of a floater the decisions close.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FloaterKey {
    /// An `LLUUID` key.
    Id(Uuid),
    /// The map `{id: item, object: object}`.
    ItemAndObject { id: Uuid, object: Uuid },
}

/// What the C++ carries out, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// `mInventorySerialNum = v`.
    SetSerial(i16),
    /// `mExpectedInventorySerialNum = v`.
    SetExpected(i16),
    /// `mInvRequestState = v`.
    SetRequestState(RequestState),
    /// `mInvRequestXFerId = v`.
    SetXferId(u64),
    /// `mInventoryDirty = v`.
    SetDirty(bool),
    /// `mInventory->clear(); delete mInventory; mInventory = NULL`.
    DeleteInventory,
    /// `mInventory->clear()`, or a new empty list when it is NULL.
    ResetInventory,
    /// `mInventory->push_front(new entry)`.
    PushFront(Entry),
    /// `mInventory->erase(the entry at this index)`.
    Erase(usize),
    /// `mPendingInventoryItemsIDs.push_back(asset)`.
    PendingPush(Uuid),
    /// `mPendingInventoryItemsIDs.erase(the id at this index)`.
    PendingErase(usize),
    /// The listener loop of `doInventoryCallback`: every listener's
    /// `inventoryChanged(this, mInventory, mInventorySerialNum, data)`, in
    /// list order, entries with a NULL listener deleted.
    NotifyListeners,
    /// `msg->sendReliable(mRegionp->getHost())` of the message.
    Send(Outgoing),
    /// `LLCoros::launch("LLViewerObject::fetchInventoryFromCapCoro()", ...)`
    /// for the object: run it at once; while `gObjectList` has the object,
    /// carry out [`cap_fetch_begin`]'s actions before the next action.
    LaunchCapFetch,
    /// `httpAdapter->getAndSuspend(url)` (adapter `TaskInventoryRequest`);
    /// the reply goes to [`cap_fetch_reply`].
    HttpGet { url: String },
    /// `LLCoros::launch("LLViewerObject::fetchInventoryDelayedCoro()", ...)`:
    /// the coroutine suspends at once on `suspendUntilTimeout(seconds)`; then
    /// [`delayed_fetch_due`] while `gObjectList` has the object.
    LaunchDelayedFetch { seconds: f64 },
    /// `gXferManager->requestFile(<cache>/filename, filename, LL_PATH_CACHE,
    /// host, true, processTaskInvFile, {task, filename, serial},
    /// HIGH_PRIORITY)`; then [`xfer_requested`] with the id it returns.
    RequestXfer { filename: Vec<u8>, serial: i16 },
    /// `gXferManager->abortRequestById(id, -1)`.
    AbortXfer { id: u64 },
    /// `LLFile::remove(<cache>/filename)`.
    RemoveCacheFile { filename: Vec<u8> },
    /// `LLFloaterReg::hideInstance(name, key)`.
    HideFloater { name: &'static str, key: FloaterKey },
    /// `LLCoros::launch("LLViewerObject::createInventoryItemCoro", ...)`:
    /// the coroutine posts `body` to `url` (adapter
    /// `CreateTaskInventoryItem`) at once; the reply goes to
    /// [`create_item_completed`].
    LaunchCreate { url: String, body: Value },
    /// The caller's callback of `createInventoryItem` with the reply.
    CreateCallback { success: bool },
    /// The caller's callback with `false` and `{message: message}`.
    CreateFailed { message: &'static str },
}

/// `LLDir::getScrubbedFileName`: every forbidden file name character
/// (`\ / : * ? " < > |`) becomes `_`.
pub fn scrubbed_file_name(name: &[u8]) -> Vec<u8> {
    name.iter().map(|&c| if b"\\/:*?\"<>|".contains(&c) { b'_' } else { c }).collect()
}
