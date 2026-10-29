//! Giving inventory (W7): `LLGiveInventory` (`llgiveinventory.cpp`), the
//! give and drop collect functors and their predicates
//! (`llinventoryfunctions.cpp`, `lltooldraganddrop.cpp`),
//! `LLToolDragAndDrop::willObjectAcceptInventory`, `handleGiveDragAndDrop`
//! and the `dad3dGiveInventory*` handlers, `move_inv_category_world_to_agent`
//! (`llinventorybridge.cpp`) and the share dialog's callback
//! (`llavataractions.cpp`).
//!
//! Items are read through the viewer item rules: `getType()` and
//! `getName()` follow a link to its target (a link to a link stops at the
//! link, a folder link reads its category: `AT_CATEGORY` and its name),
//! permissions are the item's own.

use vkstorm_llsd::Value;

use crate::llsd::coerce::{as_string, as_uuid, get, insert};
use crate::perms::{Uuid, NULL_ID, PERM_COPY, PERM_ITEM_UNRESTRICTED, PERM_TRANSFER};
use crate::types::{AssetType, FolderType};

use super::host::{Action, Handler, Host, ModelCat, ModelItem, MoveInv, NameThen, PackArgs, RlvBehaviour, TaskObject, ViewerObject, LL_PCODE_VOLUME};
use super::im::*;
use super::names::{build_username, find_bytes};
use super::notify::{force_response, selected_option};
use super::offer::build_fullname;

/// `MAX_ITEMS` (the Second Life folder give limit; `LLWorld` reports the
/// grid's, 42 unless an OpenSim grid says otherwise).
pub const MAX_ITEMS: i32 = 42;

/// `LLMuteList::AR_INVENTORY`.
pub const AR_INVENTORY: i32 = 2;

// ------------------------------------------------------ viewer item rules

enum Linked {
    Item(ModelItem),
    Category(ModelCat),
    None,
}

fn linked(host: &dyn Host, item: &ModelItem) -> Linked {
    if item.ty == AssetType::LINK {
        if let Some(target) = host.item(&item.asset) {
            if !target.is_link() {
                return Linked::Item(target);
            }
        }
        return Linked::None;
    }
    if item.ty == AssetType::LINK_FOLDER {
        if let Some(c) = host.category(&item.asset) {
            return Linked::Category(c);
        }
    }
    Linked::None
}

/// `LLViewerInventoryItem::getType()`.
pub fn item_type(host: &dyn Host, item: &ModelItem) -> AssetType {
    match linked(host, item) {
        Linked::Item(t) => t.ty,
        Linked::Category(_) => AssetType::CATEGORY,
        Linked::None => item.ty,
    }
}

/// `LLViewerInventoryItem::getName()`.
pub fn item_name(host: &dyn Host, item: &ModelItem) -> Vec<u8> {
    match linked(host, item) {
        Linked::Item(t) => t.name,
        Linked::Category(c) => c.name,
        Linked::None => item.name.clone(),
    }
}

/// `get_is_item_worn(id, item)`.
pub fn is_item_worn_item(host: &dyn Host, item: Option<&ModelItem>) -> bool {
    let Some(item) = item else { return false };
    if item.is_link() && host.item(&item.linked_uuid()).is_none() {
        return false;
    }
    let linked_id = item.linked_uuid();
    match item_type(host, item) {
        AssetType::OBJECT => host.avatar_valid() && host.is_wearing_attachment(&linked_id),
        AssetType::BODYPART | AssetType::CLOTHING => host.is_wearing_item(&linked_id),
        AssetType::GESTURE => host.is_gesture_active(&linked_id),
        _ => false,
    }
}

/// `get_is_item_worn(id)`: the model's item with that id.
pub fn is_item_worn(host: &dyn Host, id: &Uuid) -> bool {
    let item = host.item(id);
    is_item_worn_item(host, item.as_ref())
}

fn can_transfer(host: &dyn Host, item: &ModelItem) -> bool {
    item.perm.allow_operation_by(PERM_TRANSFER, &host.agent_id(), &NULL_ID)
}

fn can_copy(host: &dyn Host, item: &ModelItem) -> bool {
    item.perm.allow_operation_by(PERM_COPY, &host.agent_id(), &NULL_ID)
}

// ------------------------------------------------------------ predicates

/// `LLInventoryCollectFunctor::itemTransferCommonlyAllowed`: objects, body
/// parts and clothing only when not worn; everything else.
pub fn transfer_commonly_allowed(host: &dyn Host, item: Option<&ModelItem>) -> bool {
    let Some(item) = item else { return false };
    match item_type(host, item) {
        AssetType::OBJECT | AssetType::BODYPART | AssetType::CLOTHING => !is_item_worn(host, &item.id),
        _ => true,
    }
}

/// `LLGiveInventory::isInventoryGiveAcceptable`.
pub fn is_give_acceptable(host: &dyn Host, item: Option<&ModelItem>) -> bool {
    let Some(item) = item else { return false };
    if !host.avatar_valid() || !can_transfer(host, item) {
        return false;
    }
    match item_type(host, item) {
        AssetType::OBJECT | AssetType::BODYPART | AssetType::CLOTHING => !is_item_worn(host, &item.id),
        _ => true,
    }
}

/// `LLGiveInventory::isInventoryGroupGiveAcceptable`.
pub fn is_group_give_acceptable(host: &dyn Host, item: Option<&ModelItem>) -> bool {
    let Some(item) = item else { return false };
    if !host.avatar_valid() || !can_transfer(host, item) || !can_copy(host, item) {
        return false;
    }
    !(item_type(host, item) == AssetType::OBJECT && host.is_wearing_attachment(&item.id))
}

/// The collect functors of `llgiveinventory.cpp` and `lltooldraganddrop.cpp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Functor {
    /// `LLGiveable`: every category; items commonly transferable and
    /// transferable by the agent, counting the uncopyable ones.
    Giveable,
    /// `LLUncopyableItems`: items commonly transferable but not copyable.
    Uncopyable,
    /// `LLDroppableItem(true)`.
    DroppableTransfer,
    /// `LLDroppableItem(false)`.
    DroppableNoTransfer,
    /// `LLDropCopyableItems`.
    DropCopyable,
}

impl Functor {
    /// By the harness name (`giveable`, `uncopyable`, `droppable_transfer`,
    /// `droppable_no_transfer`, `drop_copyable`).
    pub fn from_name(name: &str) -> Option<Functor> {
        Some(match name {
            "giveable" => Functor::Giveable,
            "uncopyable" => Functor::Uncopyable,
            "droppable_transfer" => Functor::DroppableTransfer,
            "droppable_no_transfer" => Functor::DroppableNoTransfer,
            "drop_copyable" => Functor::DropCopyable,
            _ => return None,
        })
    }

    /// Whether the functor counts the uncopyable items (`countNoCopy`).
    pub fn counts(self) -> bool {
        matches!(self, Functor::Giveable | Functor::DroppableTransfer | Functor::DroppableNoTransfer)
    }
}

/// A functor with its count.
#[derive(Clone, Copy, Debug)]
pub struct Collector {
    pub functor: Functor,
    pub count_no_copy: i32,
}

impl Collector {
    pub fn new(functor: Functor) -> Collector {
        Collector { functor, count_no_copy: 0 }
    }

    /// `operator()(cat, item)`.
    pub fn call(&mut self, host: &dyn Host, cat: Option<&ModelCat>, item: Option<&ModelItem>) -> bool {
        match self.functor {
            Functor::Giveable => {
                if cat.is_some() {
                    return true;
                }
                let Some(item) = item else { return false };
                let mut allowed = transfer_commonly_allowed(host, Some(item));
                if allowed && !can_transfer(host, item) {
                    allowed = false;
                }
                if allowed && !can_copy(host, item) {
                    self.count_no_copy += 1;
                }
                allowed
            }
            Functor::Uncopyable => item.is_some_and(|i| transfer_commonly_allowed(host, Some(i)) && !can_copy(host, i)),
            Functor::DroppableTransfer | Functor::DroppableNoTransfer => {
                let Some(item) = item else { return false };
                let mut allowed = transfer_commonly_allowed(host, Some(item));
                if allowed && self.functor == Functor::DroppableTransfer && !can_transfer(host, item) {
                    allowed = false;
                }
                if allowed && !can_copy(host, item) {
                    self.count_no_copy += 1;
                }
                allowed
            }
            Functor::DropCopyable => {
                let Some(item) = item else { return false };
                let mut allowed = transfer_commonly_allowed(host, Some(item));
                if allowed && !can_copy(host, item) {
                    allowed = false;
                }
                allowed
            }
        }
    }
}

/// One functor call on its own (the seam's `collect_functor`): the verdict
/// and, for the counting functors, `countNoCopy()`.
pub fn collect_functor(host: &dyn Host, functor: Functor, cat: Option<&ModelCat>, item: Option<&ModelItem>) -> (bool, Option<i32>) {
    let mut c = Collector::new(functor);
    let r = c.call(host, cat, item);
    (r, functor.counts().then_some(c.count_no_copy))
}

/// `LLInventoryModel::collectDescendentsIf(id, cats, items, EXCLUDE_TRASH,
/// functor)`: nothing under the Trash; categories (each followed by its own
/// descendents) then the folder's items, in model order.
pub fn collect_descendents(host: &mut dyn Host, id: &Uuid, c: &mut Collector, cats: &mut Vec<ModelCat>, items: &mut Vec<ModelItem>) {
    let trash = host.find_category_for_type(FolderType::TRASH);
    if trash != NULL_ID && trash == *id {
        return;
    }
    let (child_cats, child_items) = host.children(id);
    for cat in child_cats {
        if c.call(host, Some(&cat), None) {
            cats.push(cat.clone());
        }
        collect_descendents(host, &cat.id, c, cats, items);
    }
    for item in child_items {
        if c.call(host, None, Some(&item)) {
            items.push(item);
        }
    }
}

// ----------------------------------------------------------------- gives

/// `LLIMMgr::computeSessionID(IM_NOTHING_SPECIAL, other)`: the XOR of the two
/// ids, or the agent's id for the agent itself.
pub fn compute_session_id(agent: &Uuid, other: &Uuid) -> Uuid {
    if other == agent {
        return *agent;
    }
    let mut s = [0u8; 16];
    for i in 0..16 {
        s[i] = other[i] ^ agent[i];
    }
    s
}

fn rlv_share_blocked(host: &dyn Host, to: &Uuid) -> bool {
    host.rlv_enabled() && !host.rlv_can_give_inventory(to)
}

fn recent_people_allowed(host: &dyn Host, to: &Uuid, im_session: &Uuid) -> bool {
    !host.rlv_enabled() || host.rlv_can_show_name(to) || *im_session != NULL_ID || !host.rlv_is_nearby(to) || host.rlv_has_open_profile(to)
}

fn notify(host: &mut dyn Host, name: &str, substitutions: Value, payload: Value, handler: Option<Handler>) {
    host.act(Action::Notify { name: name.to_owned(), substitutions, payload, handler });
}

/// `LLGiveInventory::logInventoryOffer(to, im_session, item_name, is_folder)`.
pub fn log_inventory_offer(host: &mut dyn Host, to: &Uuid, im_session: &Uuid, name: &[u8], is_folder: bool) {
    let session_id = compute_session_id(&host.agent_id(), to);
    let mut args = Value::Undefined;
    insert(&mut args, "user_id", Value::Uuid(*to));
    insert(&mut args, "ITEM_NAME", Value::String(name.to_vec()));
    let message_name = if is_folder { "inventory_folder_offered" } else { "inventory_item_offered" };
    if *im_session != NULL_ID {
        host.act(Action::ImSystemMessage { session: *im_session, name: message_name.to_owned(), args });
    } else if host.rlv_enabled() && !host.rlv_can_show_name(to) && host.rlv_is_nearby(to) && !host.rlv_has_open_profile(to) {
        let mut rlv_args = Value::Undefined;
        let mut msg_name = "inventory_item_offered-im";
        if let Some(anon) = host.rlv_anonym_avatar(to) {
            insert(&mut rlv_args, "NAME", Value::String(anon));
            msg_name = "inventory_item_offered_rlv";
        }
        host.act(Action::ImSystemMessage { session: NULL_ID, name: msg_name.to_owned(), args: rlv_args });
    } else if host.im_session_name(&session_id).is_some() {
        host.act(Action::ImSystemMessage { session: session_id, name: message_name.to_owned(), args });
    } else if let Some(user) = host.avatar_user_name(to) {
        let file = if host.setting_bool("UseLegacyIMLogNames") {
            match find_bytes(&user, b" Resident") {
                Some(i) => user[..i].to_vec(),
                None => user,
            }
        } else {
            build_username(&user, host.trim_resident_surname())
        };
        let template = host.trans(&format!("{message_name}-im"), &[]);
        let from = host.trans("SECOND_LIFE", &[]);
        host.act(Action::ImLogToFile { file, from, id: *im_session, template, args });
    }
}

fn beam_and_unmute(host: &mut dyn Host, to: &Uuid) {
    if host.setting_bool("FSCreateGiveInventoryParticleEffect") {
        host.act(Action::HudBeam { to: *to });
    }
    if host.have_floater_tools() {
        host.act(Action::FloaterToolsDirty);
    }
    host.act(Action::MuteAutoRemove { id: *to, reason: AR_INVENTORY });
}

fn pack_offer(host: &mut dyn Host, to: &Uuid, message: Vec<u8>, txn: Uuid, bucket: Vec<u8>) {
    host.act(Action::PackInstantMessage(PackArgs {
        from_id: host.agent_id(),
        from_group: false,
        session_id: host.agent_session_id(),
        to_id: *to,
        name: build_fullname(host),
        message,
        offline: IM_ONLINE,
        dialog: IM_INVENTORY_OFFERED,
        id: txn,
        parent_estate_id: 0,
        region_id: NULL_ID,
        position: host.agent_position(),
        timestamp: NO_TIMESTAMP,
        bucket: Some(bucket),
    }));
    host.act(Action::SendPackedToRegion);
}

/// `LLGiveInventory::commitGiveInventoryItem` (RLVa's variant returning
/// whether it gave).
pub fn commit_give_item(host: &mut dyn Host, to: &Uuid, item: &ModelItem, im_session: &Uuid) -> bool {
    if rlv_share_blocked(host, to) {
        return false;
    }
    let name = item_name(host, item);
    let txn = host.generate_uuid();
    let mut bucket = Vec::with_capacity(17);
    bucket.push(item_type(host, item).0 as u8);
    bucket.extend_from_slice(&item.id);
    pack_offer(host, to, name.clone(), txn, bucket);
    beam_and_unmute(host, to);
    log_inventory_offer(host, to, im_session, &name, false);
    if recent_people_allowed(host, to, im_session) {
        host.act(Action::RecentPeopleAdd(*to));
    }
    true
}

/// `LLGiveInventory::doGiveInventoryItem(to, item, im_session)`: a copyable
/// item is given at once; an uncopyable one asks first (`CannotCopyWarning`)
/// and the call reports false.
pub fn give_item(host: &mut dyn Host, to: &Uuid, item: Option<&ModelItem>, im_session: &Uuid) -> bool {
    if !is_give_acceptable(host, item) {
        return false;
    }
    let item = item.expect("acceptable");
    if can_copy(host, item) {
        commit_give_item(host, to, item, im_session)
    } else {
        let mut subs = Value::Undefined;
        insert(&mut subs, "ITEMS", Value::String(item_name(host, item)));
        let mut payload = Value::Undefined;
        insert(&mut payload, "agent_id", Value::Uuid(*to));
        insert(&mut payload, "items", Value::Array(vec![Value::Uuid(item.id)]));
        notify(host, "CannotCopyWarning", subs, payload, Some(Handler::CopyProtectedItem));
        false
    }
}

/// `LLGiveInventory::doGiveInventoryCategory(to, cat, im_session,
/// success_notification)`.
pub fn give_category(host: &mut dyn Host, to: &Uuid, cat: Option<&ModelCat>, im_session: &Uuid, success_notification: &[u8]) -> bool {
    let Some(cat) = cat else { return false };
    if !host.avatar_valid() {
        return false;
    }
    let mut give_successful = true;
    let mut collector = Collector::new(Functor::Giveable);
    let (mut cats, mut items) = (Vec::new(), Vec::new());
    collect_descendents(host, &cat.id, &mut collector, &mut cats, &mut items);
    let complete = cats.iter().all(|c| host.is_category_complete(&c.id));
    if !complete {
        notify(host, "IncompleteInventory", Value::Undefined, Value::Undefined, None);
        give_successful = false;
    }
    let count = items.len() + cats.len();
    let limit = host.max_inventory_items_transfer();
    if count as i64 > limit as i64 {
        notify(host, "TooManyItems", Value::Undefined, Value::Undefined, None);
        give_successful = false;
    } else if count == 0 {
        notify(host, "NoItems", Value::Undefined, Value::Undefined, None);
        give_successful = false;
    } else if give_successful {
        if collector.count_no_copy == 0 {
            give_successful = commit_give_category(host, to, cat, im_session);
        } else {
            let mut args = Value::Undefined;
            insert(&mut args, "COUNT", Value::String(collector.count_no_copy.to_string().into_bytes()));
            let mut payload = Value::Undefined;
            insert(&mut payload, "agent_id", Value::Uuid(*to));
            insert(&mut payload, "folder_id", Value::Uuid(cat.id));
            if !success_notification.is_empty() {
                insert(&mut payload, "success_notification", Value::String(success_notification.to_vec()));
            }
            notify(host, "CannotCopyCountItems", args, payload, Some(Handler::CopyProtectedCategory));
            give_successful = false;
        }
    }
    give_successful
}

/// `LLGiveInventory::commitGiveInventoryCategory`: the folder IM. Its bucket
/// is the root, then the sub-categories in collection order (each with the
/// root's type byte), then the items (each with its own type byte), 17 bytes
/// an entry; the count excludes the root.
pub fn commit_give_category(host: &mut dyn Host, to: &Uuid, cat: &ModelCat, im_session: &Uuid) -> bool {
    if rlv_share_blocked(host, to) {
        return false;
    }
    if recent_people_allowed(host, to, im_session) {
        host.act(Action::RecentPeopleAdd(*to));
    }
    let mut collector = Collector::new(Functor::Giveable);
    let (mut cats, mut items) = (Vec::new(), Vec::new());
    collect_descendents(host, &cat.id, &mut collector, &mut cats, &mut items);
    let count = items.len() + cats.len();
    let limit = host.max_inventory_items_transfer();
    if count as i64 > limit as i64 {
        notify(host, "TooManyItems", Value::Undefined, Value::Undefined, None);
        return false;
    }
    if count == 0 {
        notify(host, "NoItems", Value::Undefined, Value::Undefined, None);
        return false;
    }
    let txn = host.generate_uuid();
    let root_type = AssetType::CATEGORY.0 as u8;
    let mut bucket = Vec::with_capacity(17 * (count + 1));
    bucket.push(root_type);
    bucket.extend_from_slice(&cat.id);
    for c in &cats {
        bucket.push(root_type);
        bucket.extend_from_slice(&c.id);
    }
    for i in &items {
        bucket.push(item_type(host, i).0 as u8);
        bucket.extend_from_slice(&i.id);
    }
    pack_offer(host, to, cat.name.clone(), txn, bucket);
    beam_and_unmute(host, to);
    log_inventory_offer(host, to, im_session, &cat.name, true);
    true
}

fn blocked_share(host: &mut dyn Host, agent: &Uuid) {
    let mut args = Value::Undefined;
    insert(&mut args, "RECIPIENT", Value::String(host.slurl("agent", agent, b"completename")));
    host.act(Action::RlvNotifyBlockedShare { args });
}

/// `LLGiveInventory::handleCopyProtectedItem`: on Yes, every listed item
/// still in the model is given and deleted locally.
pub fn handle_copy_protected_item(host: &mut dyn Host, notification: &Value, response: &Value) -> bool {
    let option = selected_option(notification, response);
    let payload = get(notification, "payload");
    let mut give_successful = true;
    if option == 0 {
        let agent = as_uuid(get(payload, "agent_id"));
        if rlv_share_blocked(host, &agent) {
            blocked_share(host, &agent);
            return false;
        }
        if let Value::Array(list) = get(payload, "items") {
            for v in list {
                match host.item(&as_uuid(v)) {
                    Some(item) => {
                        commit_give_item(host, &agent, &item, &NULL_ID);
                        host.act(Action::DeleteObject(item.id));
                        host.act(Action::NotifyObservers);
                    }
                    None => {
                        notify(host, "CannotGiveItem", Value::Undefined, Value::Undefined, None);
                        give_successful = false;
                    }
                }
            }
        }
        let success = get(payload, "success_notification");
        if give_successful && !matches!(success, Value::Undefined) {
            let name = String::from_utf8_lossy(&as_string(success)).into_owned();
            notify(host, &name, Value::Undefined, Value::Undefined, None);
        }
    } else {
        notify(host, "TransactionCancelled", Value::Undefined, Value::Undefined, None);
        give_successful = false;
    }
    give_successful
}

/// `LLGiveInventory::handleCopyProtectedCategory`: on Yes the folder is given
/// (with the count checks again) and its uncopyable items are deleted
/// locally whatever the give did.
pub fn handle_copy_protected_category(host: &mut dyn Host, notification: &Value, response: &Value) -> bool {
    let option = selected_option(notification, response);
    let payload = get(notification, "payload");
    if option != 0 {
        notify(host, "TransactionCancelled", Value::Undefined, Value::Undefined, None);
        return false;
    }
    let Some(cat) = host.category(&as_uuid(get(payload, "folder_id"))) else {
        notify(host, "CannotGiveCategory", Value::Undefined, Value::Undefined, None);
        return false;
    };
    let agent = as_uuid(get(payload, "agent_id"));
    if rlv_share_blocked(host, &agent) {
        blocked_share(host, &agent);
        return false;
    }
    let give_successful = commit_give_category(host, &agent, &cat, &NULL_ID);
    let mut remove = Collector::new(Functor::Uncopyable);
    let (mut cats, mut items) = (Vec::new(), Vec::new());
    collect_descendents(host, &cat.id, &mut remove, &mut cats, &mut items);
    for i in &items {
        host.act(Action::DeleteObject(i.id));
    }
    host.act(Action::NotifyObservers);
    let success = get(payload, "success_notification");
    if give_successful && !matches!(success, Value::Undefined) {
        let name = String::from_utf8_lossy(&as_string(success)).into_owned();
        notify(host, &name, Value::Undefined, Value::Undefined, None);
    }
    give_successful
}

/// The answer to a notification the give code posted (the seam's `answer`).
pub fn answer(host: &mut dyn Host, handler: Handler, notification: &Value, response: &Value) {
    match handler {
        Handler::CopyProtectedItem => {
            handle_copy_protected_item(host, notification, response);
        }
        Handler::CopyProtectedCategory => {
            handle_copy_protected_category(host, notification, response);
        }
        Handler::DragGiveInventory => drag_give_inventory_cb(host, notification, response),
    }
}

// ------------------------------------------------------------ the share

/// `action_give_inventory::give_inventory_cb` (the share dialog): for each
/// avatar, the selection in UUID order: the first folder is given and the
/// loop stops; copyable items are given one IM each; the uncopyable ones
/// are collected into one `CannotCopyWarning` and the avatar loop stops.
/// `selected` must be sorted (a `std::set<LLUUID>`).
pub fn share(host: &mut dyn Host, avatars: &[Uuid], notification: &Value, response: &Value, selected: &[Uuid]) {
    let option = selected_option(notification, response);
    if option == 1 || selected.is_empty() {
        return;
    }
    let mut shared = !avatars.is_empty() && !selected.is_empty();
    for avatar in avatars {
        let session_id = compute_session_id(&host.agent_id(), avatar);
        let separator = host.trans("words_separator", &[]);
        let mut noncopy_names: Vec<u8> = Vec::new();
        let mut noncopy: Vec<Value> = Vec::new();
        for id in selected {
            if let Some(cat) = host.category(id) {
                if !give_category(host, avatar, Some(&cat), &session_id, b"ItemsShared") {
                    shared = false;
                }
                break;
            }
            let Some(item) = host.item(id) else { continue };
            if !can_copy(host, &item) {
                if !noncopy_names.is_empty() {
                    noncopy_names.extend_from_slice(&separator);
                }
                noncopy_names.extend_from_slice(&item_name(host, &item));
                noncopy.push(Value::Uuid(*id));
            } else if !give_item(host, avatar, Some(&item), &session_id) {
                shared = false;
            }
        }
        if !noncopy.is_empty() {
            let mut subs = Value::Undefined;
            insert(&mut subs, "ITEMS", Value::String(noncopy_names));
            let mut payload = Value::Undefined;
            insert(&mut payload, "agent_id", Value::Uuid(*avatar));
            insert(&mut payload, "items", Value::Array(noncopy));
            insert(&mut payload, "success_notification", Value::string("ItemsShared"));
            notify(host, "CannotCopyWarning", subs, payload, Some(Handler::CopyProtectedItem));
            shared = false;
            break;
        }
    }
    if shared {
        host.act(Action::FloaterHide("avatar_picker"));
        notify(host, "ItemsShared", Value::Undefined, Value::Undefined, None);
    }
}

// --------------------------------------------------------- drag and drop

/// `LLToolDragAndDrop::willObjectAcceptInventory(obj, item, type)`.
pub fn will_object_accept(host: &dyn Host, obj: Option<&ViewerObject>, item: Option<&ModelItem>, dad_type: i32) -> Acceptance {
    let (Some(item), Some(obj)) = (item, obj) else { return Acceptance::No };
    if !item.finished && dad_type != DAD_CATEGORY {
        return Acceptance::No;
    }
    if item.is_link() {
        return Acceptance::No;
    }
    if obj.id == item.parent {
        return Acceptance::No;
    }
    let mut worn = false;
    match item_type(host, item) {
        AssetType::OBJECT => {
            if host.have_avatar() && host.is_wearing_attachment(&item.id) {
                worn = true;
            }
        }
        AssetType::BODYPART | AssetType::CLOTHING => {
            if host.is_wearing_item(&item.id) {
                worn = true;
            }
        }
        AssetType::CALLINGCARD => return Acceptance::No,
        _ => {}
    }
    let perm = &item.perm;
    let agent = host.agent_id();
    let modify = obj.perm_modify || obj.flag_allow_inventory_add;
    let transfer = (obj.perm_you_owner && perm.owner == agent) || perm.allow_operation_by(PERM_TRANSFER, &agent, &NULL_ID);
    let volume = obj.pcode == LL_PCODE_VOLUME;
    let attached = obj.is_attachment;
    let unrestricted = (perm.base & PERM_ITEM_UNRESTRICTED) == PERM_ITEM_UNRESTRICTED;

    if host.rlv_enabled() {
        if host.rlv_locked_attachment(&obj.root) {
            return Acceptance::NoLocked;
        } else if (host.rlv_has_behaviour(RlvBehaviour::Unsit) || host.rlv_has_behaviour(RlvBehaviour::SitTp))
            && host.avatar_valid()
            && host.avatar_sitting()
            && host.avatar_root() == Some(obj.root)
        {
            return Acceptance::NoLocked;
        }
    }

    if attached && !unrestricted {
        Acceptance::NoLocked
    } else if modify && transfer && volume && !worn {
        Acceptance::YesMulti
    } else if !modify {
        Acceptance::NoLocked
    } else {
        Acceptance::No
    }
}

/// `LLInventoryFilter` as `move_inv_category_world_to_agent` uses it.
pub trait ItemFilter {
    /// `getFilterTypes()`.
    fn filter_types(&self) -> u64;
    /// `check(item)`.
    fn check(&mut self, item: &TaskObject) -> bool;
}

/// `LLInventoryFilter::FILTERTYPE_OBJECT`.
pub const FILTERTYPE_OBJECT: u64 = 0x1;
/// `LLInventoryFilter::FILTERTYPE_DATE`.
pub const FILTERTYPE_DATE: u64 = 0x1 << 3;

/// `move_inv_category_world_to_agent(object, category, drop, NULL, NULL,
/// filter)`: whether the object's contents may come to the agent (copy and
/// transfer, or a move out of an object the agent owns; **the accept flag
/// is sticky**: once an item passed, a later one that passes neither test
/// keeps it). On a drop, a move asks first (`warn_move_inventory`) and a
/// copy goes through `MoveInventoryFromObject`'s forced OK.
pub fn world_to_agent(host: &mut dyn Host, object_id: &Uuid, category_id: &Uuid, drop: bool, mut filter: Option<&mut dyn ItemFilter>) -> bool {
    let Some(object) = host.object(object_id) else { return false };
    let contents: Vec<TaskObject> = host.object_inventory(object_id).into_iter().filter(|o| o.ty != AssetType::CATEGORY).collect();
    if contents.is_empty() {
        return false;
    }
    let mut accept = false;
    let mut is_move = false;
    let use_filter = match &filter {
        Some(f) => {
            let t = f.filter_types();
            t != 0 && ((t & FILTERTYPE_DATE) != 0 || (t & FILTERTYPE_OBJECT) == 0)
        }
        None => false,
    };
    let agent = host.agent_id();
    let group = host.agent_group_id();
    for o in &contents {
        let Some(perm) = &o.perm else { continue };
        if perm.allow_operation_by(PERM_COPY, &agent, &group) && perm.allow_transfer_to(&agent) {
            accept = true;
        } else if object.perm_you_owner {
            is_move = true;
            accept = true;
        }
        if accept && use_filter {
            if let Some(f) = filter.as_mut() {
                accept = f.check(o);
            }
        }
        if !accept {
            break;
        }
    }
    if drop && accept {
        let mv = MoveInv { object_id: *object_id, category_id: *category_id, moves: contents.iter().map(|o| (*category_id, o.id)).collect() };
        if is_move {
            host.act(Action::WarnMoveInventory(mv));
        } else if let Some((notification, response)) = force_response(host, "MoveInventoryFromObject", 0) {
            host.act(Action::MoveTaskInventory { notification, response, mv });
        }
    }
    accept
}

/// The object being dragged (`void* cargo_data`, an `LLInventoryObject*`
/// that need not be in the model).
#[derive(Clone, Debug)]
pub enum Cargo {
    Item(ModelItem),
    Category(ModelCat),
}

impl Cargo {
    pub fn id(&self) -> Uuid {
        match self {
            Cargo::Item(i) => i.id,
            Cargo::Category(c) => c.id,
        }
    }

    /// `inv_obj->getName()` (virtual: the viewer item rules for an item).
    pub fn name(&self, host: &dyn Host) -> Vec<u8> {
        match self {
            Cargo::Item(i) => item_name(host, i),
            Cargo::Category(c) => c.name.clone(),
        }
    }
}

/// `show_object_sharing_confirmation(name, inv_item, dest, dest_agent,
/// session)`: the `ShareItemsConfirmation` question.
pub fn show_object_sharing_confirmation(host: &mut dyn Host, name: &[u8], item_id: &Uuid, item_name: &[u8], dest: &[u8], dest_agent: &Uuid, session: &Uuid) {
    let mut subs = Value::Undefined;
    insert(&mut subs, "RESIDENTS", Value::String(name.to_vec()));
    insert(&mut subs, "ITEMS", Value::String(item_name.to_vec()));
    let mut payload = Value::Undefined;
    insert(&mut payload, "agent_id", Value::Uuid(*dest_agent));
    insert(&mut payload, "item_id", Value::Uuid(*item_id));
    insert(&mut payload, "session_id", Value::Uuid(*session));
    insert(&mut payload, "d&d_dest", Value::String(dest.to_vec()));
    notify(host, "ShareItemsConfirmation", subs, payload, Some(Handler::DragGiveInventory));
}

/// `LLToolDragAndDrop::handleGiveDragAndDrop(dest_agent, session, drop,
/// type, cargo, &accept, dest)`: (handled, accept). RLVa `@share` gives
/// `ACCEPT_NO_LOCKED`; a giveable item or any folder in the model
/// `ACCEPT_YES_COPY_SINGLE`, and a drop asks `ShareItemsConfirmation` (the
/// name from the IM session, the cache, or a lookup).
pub fn give_drag_and_drop(host: &mut dyn Host, dest_agent: &Uuid, session: &Uuid, drop: bool, cargo_type: i32, cargo: &Cargo, dest: &Value) -> (bool, Acceptance) {
    if rlv_share_blocked(host, dest_agent) {
        return (true, Acceptance::NoLocked);
    }
    let accept = match cargo_type {
        DAD_TEXTURE | DAD_SOUND | DAD_LANDMARK | DAD_SCRIPT | DAD_OBJECT | DAD_NOTECARD | DAD_CLOTHING | DAD_BODYPART | DAD_ANIMATION
        | DAD_GESTURE | DAD_CALLINGCARD | DAD_MESH | DAD_CATEGORY | DAD_SETTINGS | DAD_MATERIAL => {
            let id = cargo.id();
            let as_item = match cargo {
                Cargo::Item(i) => Some(i),
                Cargo::Category(_) => None,
            };
            if host.category(&id).is_some() || (host.item(&id).is_some() && is_give_acceptable(host, as_item)) {
                if drop {
                    let dest_str = as_string(dest);
                    let item_name = cargo.name(host);
                    match host.im_session_name(session) {
                        None => {
                            match host.avatar_user_name(dest_agent) {
                                Some(n) => show_object_sharing_confirmation(host, &n, &id, &item_name, &dest_str, dest_agent, &NULL_ID),
                                None => host.act(Action::AvatarNameLookup {
                                    id: *dest_agent,
                                    then: NameThen::ShareConfirmation { item_id: id, item_name, dest: dest_str },
                                }),
                            }
                            return (true, Acceptance::YesCopySingle);
                        }
                        Some(session_name) => {
                            let dest_name = host.avatar_complete_name(dest_agent).unwrap_or(session_name);
                            show_object_sharing_confirmation(host, &dest_name, &id, &item_name, &dest_str, dest_agent, session);
                        }
                    }
                }
                Acceptance::YesCopySingle
            } else {
                Acceptance::No
            }
        }
        _ => Acceptance::No,
    };
    (true, accept)
}

/// `give_inventory_cb` of `lltooldraganddrop.cpp`: the confirmed drop is
/// given (an item, else a folder); on success the avatar picker closes (for
/// a drop on it) and `ItemsShared` shows.
pub fn drag_give_inventory_cb(host: &mut dyn Host, notification: &Value, response: &Value) {
    let option = selected_option(notification, response);
    if option == 1 {
        return;
    }
    let payload = get(notification, "payload");
    let session = as_uuid(get(payload, "session_id"));
    let agent = as_uuid(get(payload, "agent_id"));
    let id = as_uuid(get(payload, "item_id"));
    let inv_item = host.item(&id);
    let inv_cat = host.category(&id);
    if inv_item.is_none() && inv_cat.is_none() {
        return;
    }
    let shared = match &inv_item {
        Some(i) => give_item(host, &agent, Some(i), &session),
        None => give_category(host, &agent, inv_cat.as_ref(), &session, b""),
    };
    if shared {
        if as_string(get(payload, "d&d_dest")) == b"avatarpicker" {
            host.act(Action::FloaterHide("avatar_picker"));
        }
        notify(host, "ItemsShared", Value::Undefined, Value::Undefined, None);
    }
}

/// `LLToolDragAndDrop`'s drag source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragSource {
    Agent,
    World,
    Notecard,
    Library,
    Viewer,
    People,
}

/// The drag state the 3D give handlers read (`mSource`, `locateInventory`).
#[derive(Clone, Debug)]
pub struct DragState {
    pub source: DragSource,
    pub item: Option<ModelItem>,
    pub cat: Option<ModelCat>,
}

/// The `dad3dGiveInventory*` handlers for a drop on an avatar in world:
/// kind 0 `dad3dGiveInventoryObject`, 1 `dad3dGiveInventory`, 2
/// `dad3dGiveInventoryCategory`; `obj` is the avatar's object id.
pub fn dad3d_give(host: &mut dyn Host, kind: i32, tool: &DragState, obj: Option<&Uuid>, drop: bool) -> Acceptance {
    match kind {
        0 => {
            if tool.source != DragSource::Agent {
                return Acceptance::No;
            }
            let Some(item) = tool.item.as_ref().filter(|i| i.finished) else { return Acceptance::No };
            if !can_transfer(host, item) {
                return Acceptance::No;
            }
            if host.have_avatar() && host.is_wearing_attachment(&item.id) {
                return Acceptance::No;
            }
            if let (Some(o), true) = (obj, host.have_avatar()) {
                if host.rlv_enabled() && !host.rlv_can_give_inventory(o) {
                    return Acceptance::NoLocked;
                }
                if drop {
                    give_item(host, o, Some(item), &NULL_ID);
                }
                return Acceptance::YesSingle;
            }
            Acceptance::No
        }
        1 => {
            if tool.source != DragSource::Agent {
                return Acceptance::No;
            }
            let Some(item) = tool.item.as_ref().filter(|i| i.finished) else { return Acceptance::No };
            if !is_give_acceptable(host, Some(item)) {
                return Acceptance::No;
            }
            if let Some(o) = obj {
                if host.rlv_enabled() && !host.rlv_can_give_inventory(o) {
                    return Acceptance::NoLocked;
                }
            }
            if let (true, Some(o)) = (drop, obj) {
                give_item(host, o, Some(item), &NULL_ID);
            }
            Acceptance::YesSingle
        }
        _ => {
            if let Some(o) = obj {
                if host.rlv_enabled() && !host.rlv_can_give_inventory(o) {
                    return Acceptance::NoLocked;
                }
            }
            if let (true, Some(o)) = (drop, obj) {
                let Some(cat) = tool.cat.as_ref() else { return Acceptance::No };
                give_category(host, o, Some(cat), &NULL_ID, b"");
            }
            Acceptance::YesSingle
        }
    }
}
