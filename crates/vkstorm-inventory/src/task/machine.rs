//! The serial and request-state machine and the local mutations of
//! `LLViewerObject`'s task inventory, as decisions: a [`TaskView`] and an
//! [`Env`] in, the ordered [`Action`]s out (see the [module
//! documentation](super)).
//!
//! Each function is the C++ member of the same name, statement by
//! statement, over a working copy of the view; every assignment to the
//! object's state and every side effect becomes an action, in the C++
//! order. Serials are `S16` and wrap (`32767 + 1` is `-32768`); they are
//! compared signed, without wrap handling, as the C++ does.

use vkstorm_llsd::Value;

use super::create::{CreateRequest, create_body, create_succeeded};
use super::file::read_task_inventory;
use super::llsd::read_reply_contents;
use super::lookup;
use super::message::Outgoing;
use super::{
    Action, Entry, Env, FloaterKey, INVENTORY_UPDATE_WAIT_TIME_DESYNC,
    INVENTORY_UPDATE_WAIT_TIME_OUTDATED, RequestState, TASK_INVENTORY_ASSET_KEY,
    TASK_INVENTORY_ITEM_KEY, TaskEntry, TaskState, TaskView, ViewerItem, scrubbed_file_name,
};
use crate::item::Item;
use crate::llsd::coerce::{as_integer, get, has};
use crate::message::ItemBlock;
use crate::perms::{NULL_ID, Uuid};
use crate::types::AssetType;

/// The object during one decision: the view's state, changed as the
/// actions change it, and the actions so far.
struct Work<'a, E: TaskEntry> {
    id: Uuid,
    local_id: u32,
    st: TaskState,
    inv: Option<Vec<E>>,
    pending: Vec<Uuid>,
    has_listeners: bool,
    env: &'a Env<'a>,
    out: Vec<Action>,
}

impl<'a, E: TaskEntry> Work<'a, E> {
    fn new(view: &TaskView<'_, E>, env: &'a Env<'a>) -> Work<'a, E> {
        Work {
            id: view.id,
            local_id: view.local_id,
            st: view.state,
            inv: view.inventory.map(<[E]>::to_vec),
            pending: view.pending.to_vec(),
            has_listeners: view.has_listeners,
            env,
            out: Vec::new(),
        }
    }

    fn finish(self) -> Vec<Action> {
        self.out
    }

    // --- state -------------------------------------------------------------

    fn set_serial(&mut self, v: i16) {
        self.st.serial = v;
        self.out.push(Action::SetSerial(v));
    }

    fn set_expected(&mut self, v: i16) {
        self.st.expected = v;
        self.out.push(Action::SetExpected(v));
    }

    fn add_expected(&mut self, delta: i16) {
        self.set_expected(self.st.expected.wrapping_add(delta));
    }

    fn set_request(&mut self, v: RequestState) {
        self.st.request = v;
        self.out.push(Action::SetRequestState(v));
    }

    fn set_xfer(&mut self, v: u64) {
        self.st.xfer_id = v;
        self.out.push(Action::SetXferId(v));
    }

    fn set_dirty(&mut self, v: bool) {
        self.st.dirty = v;
        self.out.push(Action::SetDirty(v));
    }

    // --- the list ----------------------------------------------------------

    /// `mInventory->clear(); delete mInventory; mInventory = NULL` (nothing
    /// to do for a NULL list).
    fn delete_inventory(&mut self) {
        if self.inv.take().is_some() {
            self.out.push(Action::DeleteInventory);
        }
    }

    /// `if (mInventory) mInventory->clear(); else mInventory = new list`.
    fn reset_inventory(&mut self) {
        self.inv = Some(Vec::new());
        self.out.push(Action::ResetInventory);
    }

    fn push_front(&mut self, e: Entry) {
        if let Some(inv) = self.inv.as_mut() {
            inv.insert(0, E::from(e.clone()));
            self.out.push(Action::PushFront(e));
        }
    }

    /// `push_front` of each entry in turn, as the loaders' loops do: the
    /// same [`Action::PushFront`]s in the same order, and the list the C++
    /// `std::list` ends with (the batch reversed, before the old entries),
    /// built in one pass instead of shifting the vector once per entry.
    fn push_front_all(&mut self, entries: impl IntoIterator<Item = Entry>) {
        let Some(inv) = self.inv.as_mut() else {
            return;
        };
        let mut front = Vec::new();
        for e in entries {
            front.push(E::from(e.clone()));
            self.out.push(Action::PushFront(e));
        }
        front.reverse();
        front.append(inv);
        *inv = front;
    }

    fn entries(&self) -> &[E] {
        self.inv.as_deref().unwrap_or(&[])
    }

    // --- members -------------------------------------------------------------

    fn is_inventory_pending(&self) -> bool {
        self.st.request != RequestState::Stopped
    }

    /// `doInventoryCallback`.
    fn do_inventory_callback(&mut self) {
        self.out.push(Action::NotifyListeners);
        self.set_xfer(0);
        self.set_request(RequestState::Stopped);
    }

    /// `deleteInventoryItem`: the first entry with the id is erased; when
    /// none has it, the listeners are called back (and the request
    /// released).
    fn delete_inventory_item(&mut self, item_id: &Uuid) {
        if self.inv.is_none() {
            return;
        }
        if let Some(i) = lookup::object_by_id(self.entries(), item_id) {
            if let Some(inv) = self.inv.as_mut() {
                inv.remove(i);
            }
            self.out.push(Action::Erase(i));
            return;
        }
        self.do_inventory_callback();
    }

    /// `dirtyInventory`.
    fn dirty_inventory(&mut self) {
        if self.inv.is_some() && self.has_listeners {
            self.delete_inventory();
        }
        self.set_dirty(true);
    }

    /// `requestInventory`.
    fn request_inventory(&mut self) {
        if self.st.dirty && self.inv.is_some() && self.has_listeners {
            self.delete_inventory();
        }
        if self.inv.is_some() {
            if self.is_inventory_pending() {
                // Bug 6: the listeners get the contents; the state and xfer
                // id that doInventoryCallback() releases are put back, so
                // only the notification is left.
                self.out.push(Action::NotifyListeners);
            } else {
                self.do_inventory_callback();
            }
        } else {
            self.set_dirty(false);
            self.fetch_inventory_from_server();
        }
    }

    /// `fetchInventoryFromServer`.
    fn fetch_inventory_from_server(&mut self) {
        if self.is_inventory_pending() {
            return;
        }
        self.delete_inventory();
        self.set_request(RequestState::Pending);
        match self.env.region {
            Some(r) if !r.request_task_inventory.is_empty() => {
                self.out.push(Action::LaunchCapFetch)
            }
            None => self.set_request(RequestState::Stopped),
            Some(_) => {
                let a = self.env.agent;
                self.out.push(Action::Send(Outgoing::RequestTaskInventory {
                    agent: a.id,
                    session: a.session,
                    local_id: self.local_id,
                }));
            }
        }
    }

    /// `fetchInventoryDelayed`.
    fn fetch_inventory_delayed(&mut self, seconds: f64) {
        if self.st.request == RequestState::Wait {
            return;
        }
        if self.st.xfer_id != 0 {
            self.out.push(Action::AbortXfer {
                id: self.st.xfer_id,
            });
            self.set_xfer(0);
        }
        self.set_request(RequestState::Wait);
        self.out.push(Action::LaunchDelayedFetch { seconds });
    }

    /// `loadTaskInvFile` after the file was opened (`file` its bytes) or
    /// failed to open (`None`).
    fn load_task_inv_file(&mut self, filename: &[u8], file: Option<&[u8]>) -> bool {
        let Some(bytes) = file else {
            // LL_WARNS "unable to load task inventory".
            return false;
        };
        self.reset_inventory();
        self.push_front_all(read_task_inventory(bytes));
        self.out.push(Action::RemoveCacheFile {
            filename: filename.to_vec(),
        });
        self.do_inventory_callback();
        true
    }

    /// `loadTaskInvLLSD` for a reply with or without `contents`; the
    /// entries are [`Action::PushFrontReplyContents`].
    fn load_task_inv_llsd(&mut self, has_contents: bool) {
        if has_contents {
            self.reset_inventory();
            self.push_front(Entry::contents_category(&self.id));
            self.out.push(Action::PushFrontReplyContents);
            self.do_inventory_callback();
        } else {
            // Bug 5: no contents fails the request as an HTTP error does.
            self.set_request(RequestState::Stopped);
        }
    }

    /// The `Contents` drop target of a reply without a file name.
    fn mock_contents(&mut self) {
        self.reset_inventory();
        self.push_front(Entry::contents_category(&self.id));
        self.do_inventory_callback();
    }

    /// `doUpdateInventory`: the local update and the owner guess.
    fn do_update_inventory(&mut self, item: &ViewerItem, key: u8, is_new: bool) {
        let entries = self.entries();
        let old = match key {
            TASK_INVENTORY_ITEM_KEY => lookup::object_by_id(entries, &item.item.object.id),
            TASK_INVENTORY_ASSET_KEY => lookup::item_by_asset(entries, &item.item.asset),
            _ => None,
        }
        .and_then(|i| Some((entries[i].id(), entries[i].item_permissions()?)));
        let (item_id, new_owner, new_group, group_owned) = match old {
            Some((id, p)) => (id, p.owner, p.group, p.is_group_owned()),
            None => (item.item.object.id, NULL_ID, NULL_ID, false),
        };
        if !is_new && self.inv.is_some() {
            self.delete_inventory_item(&item_id);
            let mut perm = item.item.permissions;
            let is_atomic = item.item.object.asset_type != AssetType::OBJECT;
            if let Some(sel) = self.env.selection {
                perm.set_owner_and_group(&NULL_ID, &sel.owner, &sel.group, is_atomic);
            } else if group_owned || new_owner != NULL_ID {
                perm.set_owner_and_group(&NULL_ID, &new_owner, &new_group, is_atomic);
            } else if self.env.you_owner {
                perm.set_owner_and_group(
                    &NULL_ID,
                    &self.env.agent.id,
                    &item.item.permissions.group,
                    is_atomic,
                );
                self.add_expected(-1);
            } else {
                perm.set_owner_and_group(&NULL_ID, &NULL_ID, &NULL_ID, is_atomic);
                self.add_expected(-1);
            }
            let mut new_item = ViewerItem::copy_from(item);
            new_item.item.set_permissions(&perm);
            self.push_front(Entry::Item(new_item));
            self.do_inventory_callback();
            self.add_expected(1);
        } else if is_new {
            self.add_expected(1);
        }
    }

    /// `updateInventory` for the task copy of the caller's item.
    fn update_inventory(&mut self, task_item: &ViewerItem, key: u8, is_new: bool) {
        if self.env.region.is_none() {
            return;
        }
        let a = self.env.agent;
        self.out.push(Action::Send(Outgoing::UpdateTaskInventory {
            agent: a.id,
            session: a.session,
            local_id: self.local_id,
            key,
            item: ItemBlock::from_viewer_item(&task_item.item, &task_item.transaction),
        }));
        self.do_update_inventory(task_item, key, is_new);
    }

    /// `isAssetInInventory`.
    fn is_asset_in_inventory(&self, asset: &Uuid, asset_type: AssetType) -> bool {
        lookup::is_asset_in_inventory(self.inv.as_deref(), &self.pending, asset, asset_type)
    }
}

/// The task copy `updateInventory` and `saveScript` make: the full
/// constructor with the object as parent, and the caller's transaction id.
fn task_item_with_transaction(item: &ViewerItem, object_id: &Uuid) -> ViewerItem {
    let mut t = ViewerItem::task_copy(&item.item, object_id);
    t.transaction = item.transaction;
    t
}

/// `requestInventory`.
pub fn request_inventory<E: TaskEntry>(view: &TaskView<'_, E>, env: &Env<'_>) -> Vec<Action> {
    let mut w = Work::new(view, env);
    w.request_inventory();
    w.finish()
}

/// `dirtyInventory`.
pub fn dirty_inventory<E: TaskEntry>(view: &TaskView<'_, E>, env: &Env<'_>) -> Vec<Action> {
    let mut w = Work::new(view, env);
    w.dirty_inventory();
    w.finish()
}

/// `deleteInventoryItem` (no message).
pub fn delete_inventory_item<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    item_id: &Uuid,
) -> Vec<Action> {
    let mut w = Work::new(view, env);
    w.delete_inventory_item(item_id);
    w.finish()
}

/// `processTaskInv` (`ReplyTaskInventory`) for an object `gObjectList`
/// found: the message's `Serial` and `Filename`.
///
/// When the region has the `RequestTaskInventory` capability the reply only
/// sets the expected serial and fetches through the capability. Otherwise:
/// a serial equal to the current one and below the expected one is a loop
/// (refetch in 5 s, expected set to it); one below the expected one is out
/// of date (refetch in 1 s, current set to it); else it is loaded: both
/// serials set, and a file name that scrubs to empty mocks up the
/// `Contents` drop target, otherwise the file is requested
/// ([`Action::RequestXfer`], continued by [`xfer_requested`]).
pub fn reply_task_inventory<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    serial: i16,
    filename: &[u8],
) -> Vec<Action> {
    let mut w = Work::new(view, env);
    if env
        .region
        .is_some_and(|r| !r.request_task_inventory.is_empty())
    {
        w.set_expected(serial);
        w.fetch_inventory_from_server();
        return w.finish();
    }
    if serial == w.st.serial && serial < w.st.expected {
        w.set_expected(serial);
        w.fetch_inventory_delayed(INVENTORY_UPDATE_WAIT_TIME_DESYNC);
    } else if serial < w.st.expected {
        w.set_serial(serial);
        w.fetch_inventory_delayed(INVENTORY_UPDATE_WAIT_TIME_OUTDATED);
    } else {
        w.set_serial(serial);
        w.set_expected(serial);
        let name = scrubbed_file_name(filename);
        if name.is_empty() {
            w.mock_contents();
        } else if env.region.is_none() {
            if w.st.request == RequestState::Pending {
                w.set_request(RequestState::Stopped);
            }
        } else {
            w.out.push(Action::RequestXfer {
                filename: name,
                serial,
            });
        }
    }
    w.finish()
}

/// The rest of `processTaskInv` once `requestFile` returned `new_id`: a
/// transfer in progress is replaced by a new one (the old one aborted),
/// otherwise the state becomes `XFER` with the new id.
pub fn xfer_requested<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    new_id: u64,
) -> Vec<Action> {
    let mut w = Work::new(view, env);
    if w.st.request == RequestState::Xfer {
        if new_id > 0 && new_id != w.st.xfer_id {
            w.out.push(Action::AbortXfer { id: w.st.xfer_id });
            w.set_xfer(new_id);
        }
    } else {
        w.set_request(RequestState::Xfer);
        w.set_xfer(new_id);
    }
    w.finish()
}

/// `processTaskInvFile` for an object `gObjectList` found: the transfer's
/// `serial` and `filename` (its `LLFilenameAndTask`), its error code, and
/// the cache file's bytes (`None` when it cannot be opened).
///
/// A completion without error and with a serial not below the current one
/// records the serial and loads the file; a successful load then takes the
/// assets of the items (not `AT_CATEGORY`) out of the pending list, front to
/// back, while it is not empty.
pub fn xfer_completed<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    serial: i16,
    filename: &[u8],
    error_code: i32,
    file: Option<&[u8]>,
) -> Vec<Action> {
    let mut w = Work::new(view, env);
    if error_code != 0 || serial < w.st.serial {
        return w.finish();
    }
    w.set_serial(serial);
    if w.load_task_inv_file(filename, file) {
        let Some(inv) = w.inv.clone() else {
            return w.finish();
        };
        for e in &inv {
            if w.pending.is_empty() {
                break;
            }
            if let Some(asset) = e.item_asset() {
                if e.asset_type() != AssetType::CATEGORY {
                    if let Some(i) = w.pending.iter().position(|a| *a == asset) {
                        w.pending.remove(i);
                        w.out.push(Action::PendingErase(i));
                    }
                }
            }
        }
    }
    w.finish()
}

/// `loadTaskInvFile(filename)` with the cache file's bytes (`None` when it
/// cannot be opened): whether it loaded, and the actions.
pub fn load_task_inv_file<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    filename: &[u8],
    file: Option<&[u8]>,
) -> (bool, Vec<Action>) {
    let mut w = Work::new(view, env);
    let ok = w.load_task_inv_file(filename, file);
    (ok, w.finish())
}

/// `fetchInventoryFromCapCoro` up to its suspension, for an object
/// `gObjectList` found: the state becomes `XFER` and the capability is
/// requested with `?task_id=<id>` (and `&inventory_serial=<serial>` when
/// the serial is not 0 and the object has contents, which never holds
/// here: `fetchInventoryFromServer` deleted them; README "Recorded
/// quirks").
pub fn cap_fetch_begin<E: TaskEntry>(view: &TaskView<'_, E>, env: &Env<'_>) -> Vec<Action> {
    let mut w = Work::new(view, env);
    let cap = env.region.map_or("", |r| r.request_task_inventory);
    let mut url = format!("{cap}?task_id={}", uuid_string(&w.id));
    if w.st.serial != 0 && w.inv.is_some() {
        url.push_str(&format!("&inventory_serial={}", w.st.serial));
    }
    w.set_request(RequestState::Xfer);
    w.out.push(Action::HttpGet { url });
    w.finish()
}

/// `fetchInventoryFromCapCoro` after the reply, for an object `gObjectList`
/// still has: `success` is the HTTP status's verdict, `http_status` its
/// type, `result` the map the HTTP adapter hands on.
///
/// A success sets both serials to `(S16)result["inventory_serial"]` and
/// loads the reply; a 304 releases the request and sets the expected serial
/// to the current one; any other status releases the request. A received
/// (or kept, on 304) serial below the expected one is stale: refetch in 1 s.
///
/// The entries of `contents` come back as [`Action::PushFront`]s; see
/// [`cap_fetch_reply_summary`] for a caller that decodes them itself.
pub fn cap_fetch_reply<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    success: bool,
    http_status: i32,
    result: &Value,
) -> Vec<Action> {
    let reply = CapReply {
        serial: as_integer(get(result, "inventory_serial")),
        has_contents: has(result, "contents"),
    };
    let acts = cap_fetch_reply_summary(view, env, success, http_status, reply);
    let mut out = Vec::with_capacity(acts.len());
    for a in acts {
        if a == Action::PushFrontReplyContents {
            let entries = read_reply_contents(result, env.in_open_sim, env.names);
            out.extend(
                entries
                    .unwrap_or_default()
                    .into_iter()
                    .map(Action::PushFront),
            );
        } else {
            out.push(a);
        }
    }
    out
}

/// What the capability reply's decisions read of the reply map, for a
/// caller that keeps the reply as C++ `LLSD` (the viewer: the entries are
/// an `LLSD` form, decoded by `LLViewerInventoryItem::unpackMessage(LLSD)`,
/// which stays C++ until the reply body reaches Rust unparsed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapReply {
    /// `result["inventory_serial"].asInteger()`.
    pub serial: i32,
    /// `result.has("contents")`.
    pub has_contents: bool,
}

/// [`cap_fetch_reply`] over what it reads of the reply: the entries of
/// `contents` are one [`Action::PushFrontReplyContents`], which the caller
/// carries out by pushing each element of `contents` that is a map with
/// `item_id` to the front, in reply order, as
/// `LLViewerInventoryItem::unpackMessage(LLSD)` reads it.
pub fn cap_fetch_reply_summary<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    success: bool,
    http_status: i32,
    reply: CapReply,
) -> Vec<Action> {
    let mut w = Work::new(view, env);
    let mut potentially_stale = false;
    if success {
        let serial = reply.serial as i16;
        potentially_stale = serial < w.st.expected;
        w.set_serial(serial);
        w.set_expected(serial);
        w.load_task_inv_llsd(reply.has_contents);
    } else if http_status == 304 {
        w.set_request(RequestState::Stopped);
        potentially_stale = w.st.serial < w.st.expected;
        w.set_expected(w.st.serial);
    } else {
        w.set_request(RequestState::Stopped);
    }
    if potentially_stale {
        w.fetch_inventory_delayed(INVENTORY_UPDATE_WAIT_TIME_OUTDATED);
    }
    w.finish()
}

/// `fetchInventoryDelayedCoro` after its timeout, for an object
/// `gObjectList` still has: the request is released and fetched again.
pub fn delayed_fetch_due<E: TaskEntry>(view: &TaskView<'_, E>, env: &Env<'_>) -> Vec<Action> {
    let mut w = Work::new(view, env);
    w.set_request(RequestState::Stopped);
    w.fetch_inventory_from_server();
    w.finish()
}

/// `updateInventory(item, key, is_new)`: `UpdateTaskInventory` with the
/// task copy of the item (sliced to the viewer's fields, the object as
/// parent, the item's transaction id), then the local update with the
/// owner guess.
pub fn update_inventory<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    item: &ViewerItem,
    key: u8,
    is_new: bool,
) -> Vec<Action> {
    let mut w = Work::new(view, env);
    let task_item = task_item_with_transaction(item, &w.id);
    w.update_inventory(&task_item, key, is_new);
    w.finish()
}

/// `updateInventoryLocal(item, key)`: the local update only (no message,
/// no transaction id), as not new.
pub fn update_inventory_local<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    item: &Item,
    key: u8,
) -> Vec<Action> {
    let mut w = Work::new(view, env);
    let task_item = ViewerItem::task_copy(item, &w.id);
    w.do_update_inventory(&task_item, key, false);
    w.finish()
}

/// `updateViewerInventoryAsset(item, new_asset)`: a copy of the item with
/// the new asset, updated locally by item id, as not new.
pub fn update_viewer_inventory_asset<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    item: &ViewerItem,
    new_asset: &Uuid,
) -> Vec<Action> {
    let mut w = Work::new(view, env);
    let mut task_item = ViewerItem::copy_from(item);
    task_item.item.asset = *new_asset;
    w.do_update_inventory(&task_item, TASK_INVENTORY_ITEM_KEY, false);
    w.finish()
}

/// `updateMaterialInventory(item, key, is_new)`: textures and materials
/// only, not already present or pending, with a region; the asset is added
/// to the pending list, then [`update_inventory`].
pub fn update_material_inventory<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    item: &ViewerItem,
    key: u8,
    is_new: bool,
) -> Vec<Action> {
    let mut w = Work::new(view, env);
    let t = item.item.object.asset_type;
    if t != AssetType::TEXTURE && t != AssetType::MATERIAL {
        return w.finish();
    }
    if w.is_asset_in_inventory(&item.item.asset, t) {
        return w.finish();
    }
    if env.region.is_none() {
        return w.finish();
    }
    w.pending.push(item.item.asset);
    w.out.push(Action::PendingPush(item.item.asset));
    let task_item = task_item_with_transaction(item, &w.id);
    w.update_inventory(&task_item, key, is_new);
    w.finish()
}

/// `removeInventory(item_id)`: the item's property floaters closed, then
/// (with a region) `RemoveTaskInventory`, the first entry with the id
/// erased, and the expected serial raised.
pub fn remove_inventory<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    item_id: &Uuid,
) -> Vec<Action> {
    let mut w = Work::new(view, env);
    w.out.push(Action::HideFloater {
        name: "item_properties",
        key: FloaterKey::ItemAndObject {
            id: *item_id,
            object: w.id,
        },
    });
    w.out.push(Action::HideFloater {
        name: "properties",
        key: FloaterKey::Id(*item_id),
    });
    if env.region.is_none() {
        return w.finish();
    }
    let a = env.agent;
    w.out.push(Action::Send(Outgoing::RemoveTaskInventory {
        agent: a.id,
        session: a.session,
        local_id: w.local_id,
        item_id: *item_id,
    }));
    w.delete_inventory_item(item_id);
    w.add_expected(1);
    w.finish()
}

/// `moveInventory(folder_id, item_id)`: (with a region)
/// `MoveTaskInventory`; an item the agent may not copy leaves the contents
/// and raises the expected serial.
pub fn move_inventory<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    folder_id: &Uuid,
    item_id: &Uuid,
) -> Vec<Action> {
    let mut w = Work::new(view, env);
    if env.region.is_none() {
        return w.finish();
    }
    let a = env.agent;
    w.out.push(Action::Send(Outgoing::MoveTaskInventory {
        agent: a.id,
        session: a.session,
        folder_id: *folder_id,
        local_id: w.local_id,
        item_id: *item_id,
    }));
    let no_copy = lookup::object_by_id(w.entries(), item_id)
        .and_then(|i| w.entries()[i].item_permissions())
        .is_some_and(|p| !p.allow_copy_by(&a.id, &NULL_ID));
    if no_copy {
        w.delete_inventory_item(item_id);
        w.add_expected(1);
    }
    w.finish()
}

/// `saveScript(item, active, is_new, template_id)`: (with a region)
/// `RezScript` with the task copy of the item and, for an item without an
/// asset and a template, `NewScriptInfo`; then the local update by item id.
pub fn save_script<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    item: &ViewerItem,
    active: bool,
    is_new: bool,
    template_id: &Uuid,
) -> Vec<Action> {
    let mut w = Work::new(view, env);
    let task_item = task_item_with_transaction(item, &w.id);
    if env.region.is_none() {
        return w.finish();
    }
    let a = env.agent;
    let template =
        (task_item.item.asset == NULL_ID && *template_id != NULL_ID).then_some(*template_id);
    w.out.push(Action::Send(Outgoing::RezScript {
        agent: a.id,
        session: a.session,
        group: a.group,
        local_id: w.local_id,
        enabled: active,
        item: ItemBlock::from_viewer_item(&task_item.item, &task_item.transaction),
        template,
    }));
    w.do_update_inventory(&task_item, TASK_INVENTORY_ITEM_KEY, is_new);
    w.finish()
}

/// `createInventoryItem`: without a region or without the
/// `CreateTaskInventoryItem` capability the callback fails at once;
/// otherwise the coroutine posts the body ([`super::create::create_body`]).
pub fn create_inventory_item<E: TaskEntry>(
    view: &TaskView<'_, E>,
    env: &Env<'_>,
    req: &CreateRequest<'_>,
) -> Vec<Action> {
    let Some(region) = env.region else {
        return vec![Action::CreateFailed {
            message: "No region",
        }];
    };
    if region.create_task_inventory_item.is_empty() {
        return vec![Action::CreateFailed {
            message: "Capability not available",
        }];
    }
    vec![Action::LaunchCreate {
        url: region.create_task_inventory_item.to_owned(),
        body: create_body(&view.id, req),
    }]
}

/// `createInventoryItemCoro`'s verdict on the reply (see
/// [`super::create::create_succeeded`]).
pub fn create_item_succeeded(http_ok: bool, result: &Value) -> bool {
    create_succeeded(http_ok, result)
}

/// `createInventoryItemCoro` after the reply: on success, for the object
/// when `gObjectList` has it (`view`), the expected serial is raised and
/// the contents are made dirty and requested again; then the callback.
pub fn create_item_completed<E: TaskEntry>(
    view: Option<&TaskView<'_, E>>,
    env: &Env<'_>,
    success: bool,
) -> Vec<Action> {
    let mut out = Vec::new();
    if success {
        if let Some(view) = view {
            let mut w = Work::new(view, env);
            w.add_expected(1);
            w.dirty_inventory();
            w.request_inventory();
            out = w.finish();
        }
    }
    out.push(Action::CreateCallback { success });
    out
}

/// `LLUUID::asString`.
fn uuid_string(id: &Uuid) -> String {
    let mut out = Vec::with_capacity(36);
    vkstorm_llsd::text::uuid_to_hex(id, &mut out);
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::{Agent, LocalizedNames, TaskState};
    use super::*;
    use crate::object::Object;

    fn id(n: u32) -> Uuid {
        let mut u = [0u8; 16];
        u[..4].copy_from_slice(&n.to_be_bytes());
        u[15] = 1;
        u
    }

    /// `n` entries, the last seven sharing ids with the first (the first
    /// match is the one `deleteInventoryItem` erases).
    fn synthetic(n: u32) -> Vec<Entry> {
        (0..n)
            .map(|i| {
                let name = format!("item {i}");
                Entry::Object(Object::new(
                    &id(i % (n - 7)),
                    &id(0),
                    AssetType::OBJECT,
                    name.as_bytes(),
                ))
            })
            .collect()
    }

    fn view(object: Uuid) -> TaskView<'static> {
        TaskView {
            id: object,
            local_id: 1,
            state: TaskState::default(),
            inventory: None,
            pending: &[],
            has_listeners: false,
        }
    }

    fn env(names: &LocalizedNames) -> Env<'_> {
        Env {
            agent: Agent::default(),
            region: None,
            you_owner: false,
            selection: None,
            in_open_sim: false,
            names,
        }
    }

    /// The batched load emits the actions, and leaves the list, that one
    /// `push_front` per entry (the C++ loop, and the code before the
    /// batching) does; the later front-based `Erase` indices agree too.
    #[test]
    fn push_front_all_matches_push_front_per_entry() {
        let names = LocalizedNames::default();
        let env = env(&names);
        let object = id(0xffff_ffff);
        let view = view(object);
        let entries = synthetic(5000);

        let mut per_entry = Work::new(&view, &env);
        per_entry.reset_inventory();
        per_entry.push_front(Entry::contents_category(&object));
        for e in entries.clone() {
            per_entry.push_front(e);
        }
        let mut batched = Work::new(&view, &env);
        batched.reset_inventory();
        batched.push_front(Entry::contents_category(&object));
        batched.push_front_all(entries.clone());

        assert_eq!(batched.inv, per_entry.inv);
        assert_eq!(batched.out, per_entry.out);
        assert_eq!(batched.out.len(), 2 + entries.len());
        assert_eq!(batched.inv.as_ref().map(Vec::len), Some(entries.len() + 1));

        for n in [0, 1, 2, 2500, 4992, 4999, 77_777] {
            per_entry.delete_inventory_item(&id(n));
            batched.delete_inventory_item(&id(n));
        }
        assert_eq!(batched.inv, per_entry.inv);
        assert_eq!(batched.out, per_entry.out);
        assert!(
            batched
                .out
                .iter()
                .any(|a| matches!(a, Action::Erase(i) if *i > 0))
        );
    }

    /// With no list (`push_front` on a NULL `mInventory` does nothing) the
    /// batch does nothing either.
    #[test]
    fn push_front_all_without_a_list_does_nothing() {
        let names = LocalizedNames::default();
        let env = env(&names);
        let view = view(id(1));
        let mut w = Work::new(&view, &env);
        w.push_front_all(synthetic(10));
        assert!(w.inv.is_none());
        assert!(w.out.is_empty());
    }
}
