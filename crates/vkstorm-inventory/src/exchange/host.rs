//! The seam between the decisions and the viewer: what a decision asks
//! ([`Host`]) and what it tells the viewer to do ([`Action`]).

use vkstorm_llsd::Value;

use crate::perms::{Permissions, Uuid};
use crate::types::{AssetType, FolderType};

use super::info::{NetHost, OfferInfo};
use super::throttle::Throttle;

/// An inventory item as the model holds it (`LLViewerInventoryItem`): the
/// stored type (`getActualType`), the asset id (a link's target), the raw
/// stored name, the item's own permissions and `isFinished()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelItem {
    pub id: Uuid,
    pub parent: Uuid,
    pub ty: AssetType,
    pub asset: Uuid,
    pub name: Vec<u8>,
    pub perm: Permissions,
    pub finished: bool,
}

impl ModelItem {
    /// `getIsLinkType()`: the stored type is `AT_LINK` or `AT_LINK_FOLDER`.
    pub fn is_link(&self) -> bool {
        self.ty.is_link_type()
    }

    /// `LLInventoryItem::getLinkedUUID()`: the asset id of a link, else the
    /// item's id.
    pub fn linked_uuid(&self) -> Uuid {
        if self.is_link() { self.asset } else { self.id }
    }
}

/// An inventory category as the model holds it: the preferred type and the
/// raw stored name. Its asset type (`getType`) is always `AT_CATEGORY`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelCat {
    pub id: Uuid,
    pub parent: Uuid,
    pub preferred: FolderType,
    pub name: Vec<u8>,
}

/// `LLViewerObject` as the drop tests see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewerObject {
    pub id: Uuid,
    pub perm_modify: bool,
    pub flag_allow_inventory_add: bool,
    pub perm_you_owner: bool,
    pub pcode: u8,
    pub is_attachment: bool,
    /// `getRootEdit()`'s id (the object's own id when it is the root).
    pub root: Uuid,
}

/// `LL_PCODE_VOLUME`.
pub const LL_PCODE_VOLUME: u8 = 9;

/// One entry of an object's inventory (`LLInventoryObject::object_list_t`):
/// `perm` is the item's permissions, `None` for an object that is not an
/// `LLInventoryItem`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskObject {
    pub id: Uuid,
    pub ty: AssetType,
    pub perm: Option<Permissions>,
}

/// RLVa behaviours the drop test reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RlvBehaviour {
    Unsit,
    SitTp,
}

/// The fetch observers and openers the offer code creates (their classes
/// stay C++).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObserverKind {
    /// `LLInventoryFetchItemsObserver(id)`.
    FetchItems { id: Uuid },
    /// `LLOpenAgentOffer(object_id, from_name, manual)`.
    OpenAgentOffer {
        id: Uuid,
        from_name: Vec<u8>,
        manual: bool,
    },
    /// `LLDiscardAgentOffer(folder_id, object_id)`.
    DiscardAgentOffer { folder: Uuid, object: Uuid },
    /// `RlvGiveToRLVAgentOffer(folder)`.
    RlvGiveToRlvAgentOffer { folder: Uuid },
    /// `RlvGiveToRLVTaskOffer(transaction)`.
    RlvGiveToRlvTaskOffer { transaction: Uuid },
    /// `LLOpenTaskGroupOffer`.
    OpenTaskGroupOffer,
}

/// A handle to an observer [`Host::observer_new`] made.
pub type ObserverId = u32;

/// Where a message goes: `msg->sendReliable(host)` or
/// `gAgent.sendReliableMessage()` (the agent's region).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Host(NetHost),
    AgentRegion,
}

/// An `ImprovedInstantMessage` as its fields are added. The two string
/// fields hold the exact field data `addStringFast` adds: for a
/// `std::string` its bytes and a NUL (no bytes when empty), for a C string
/// its bytes up to the first NUL and a NUL (no bytes for `NULL`); see
/// [`string_field`] and [`cstring_field`].
#[derive(Clone, Debug, PartialEq)]
pub struct ImPacket {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub from_group: bool,
    pub to_agent_id: Uuid,
    pub parent_estate_id: u32,
    pub region_id: Uuid,
    pub position: [f32; 3],
    pub offline: u8,
    pub dialog: u8,
    pub id: Uuid,
    pub timestamp: u32,
    pub from_agent_name: Vec<u8>,
    pub message: Vec<u8>,
    pub binary_bucket: Vec<u8>,
}

/// `addStringFast(var, const std::string&)`'s data: the bytes (embedded NULs
/// included) and a NUL; nothing for an empty string.
pub fn string_field(s: &[u8]) -> Vec<u8> {
    if s.is_empty() {
        Vec::new()
    } else {
        let mut v = s.to_vec();
        v.push(0);
        v
    }
}

/// `addStringFast(var, const char*)`'s data for a non-null pointer: the
/// bytes up to the first NUL and a NUL.
pub fn cstring_field(s: &[u8]) -> Vec<u8> {
    let mut v = s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())].to_vec();
    v.push(0);
    v
}

/// `pack_instant_message`'s arguments as the caller passed them (the bucket
/// before its truncation; `None` for a null pointer).
#[derive(Clone, Debug, PartialEq)]
pub struct PackArgs {
    pub from_id: Uuid,
    pub from_group: bool,
    pub session_id: Uuid,
    pub to_id: Uuid,
    pub name: Vec<u8>,
    pub message: Vec<u8>,
    pub offline: u8,
    pub dialog: i32,
    pub id: Uuid,
    pub parent_estate_id: u32,
    pub region_id: Uuid,
    pub position: [f32; 3],
    pub timestamp: u32,
    pub bucket: Option<Vec<u8>>,
}

impl PackArgs {
    /// `pack_instant_message_block` (`llinstantmessage.cpp`): the message is
    /// cut at its first NUL and to `MTUBYTES - 1` bytes (`snprintf`), the
    /// bucket to what is left of `MTUBYTES`; a null bucket is the one-byte
    /// `EMPTY_BINARY_BUCKET`.
    pub fn packet(&self) -> ImPacket {
        let mtu = super::im::MTUBYTES;
        let (message, bytes_left) = if self.message.is_empty() {
            (Vec::new(), mtu)
        } else {
            let m = &self.message[..self
                .message
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(self.message.len())];
            let written = m.len().min(mtu - 1);
            (cstring_field(&m[..written]), mtu - written)
        };
        let binary_bucket = match &self.bucket {
            Some(b) => b[..b.len().min(bytes_left)].to_vec(),
            None => vec![0],
        };
        ImPacket {
            agent_id: self.from_id,
            session_id: self.session_id,
            from_group: self.from_group,
            to_agent_id: self.to_id,
            parent_estate_id: self.parent_estate_id,
            region_id: self.region_id,
            position: self.position,
            offline: self.offline,
            dialog: self.dialog as u8,
            id: self.id,
            timestamp: self.timestamp,
            from_agent_name: string_field(&self.name),
            message,
            binary_bucket,
        }
    }
}

/// What runs when a name lookup answers (the callbacks the C++ binds).
#[derive(Clone, Debug, PartialEq)]
pub enum NameThen {
    /// `inventory_offer_mute_avatar_callback`: mute the sender by the user
    /// name, show the blocked list, kill the queued offer toasts from them.
    OfferMuteAvatar,
    /// `inventory_offer_mute_callback` through `gCacheName->getGroup`: the
    /// group (the callback's `is_group` argument decides the mute type).
    OfferMuteGroup,
    /// `inventory_offer_name_callback`: a `SystemMessageTip` with
    /// `llformat(format, user_name) + message`.
    SystemMessageTip { format: Vec<u8>, message: Vec<u8> },
    /// `notification_group_name_cb(group_name, sender, subject, message,
    /// payload, timestamp)`: the `GroupNotice` notification.
    GroupNotice {
        sender: Vec<u8>,
        subject: Vec<u8>,
        message: Vec<u8>,
        payload: Value,
        timestamp: u32,
    },
    /// `get_name_cb` (`lltooldraganddrop.cpp`): the share confirmation for
    /// the dragged object, by the looked-up user name
    /// ([`super::give::show_object_sharing_confirmation`]).
    ShareConfirmation {
        item_id: Uuid,
        item_name: Vec<u8>,
        dest: Vec<u8>,
    },
    /// `notify_automute_callback` (the executor's `LLMuteList::autoRemove`
    /// looking up a name it does not have): the auto-unmute notice.
    AutoUnmuteNotice { reason: i32 },
}

/// The handlers the give code registers with a notification
/// (`LLNotificationsUtil::add(..., functor)`); the answer comes back through
/// [`super::give::answer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handler {
    /// `LLGiveInventory::handleCopyProtectedItem`.
    CopyProtectedItem,
    /// `LLGiveInventory::handleCopyProtectedCategory`.
    CopyProtectedCategory,
    /// `give_inventory_cb` of `lltooldraganddrop.cpp` (`ShareItemsConfirmation`).
    DragGiveInventory,
}

/// `LLMoveInv`'s object, category and move list (category, item) pairs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveInv {
    pub object_id: Uuid,
    pub category_id: Uuid,
    pub moves: Vec<(Uuid, Uuid)>,
}

/// The arguments the ReadOfflineMsgs loop posts to the main coroutine for
/// `LLIMProcessing::processNewMessage`.
#[derive(Clone, Debug, PartialEq)]
pub struct OfflineMessage {
    pub from_id: Uuid,
    pub from_group: bool,
    pub to_id: Uuid,
    pub offline: u8,
    pub dialog: i32,
    pub session_id: Uuid,
    pub timestamp: u32,
    pub name: Vec<u8>,
    pub message: Vec<u8>,
    pub parent_estate_id: u32,
    pub region_id: Uuid,
    pub position: [f32; 3],
    pub bucket: Vec<u8>,
    pub sender: NetHost,
    pub metadata: Value,
    pub aux_id: Uuid,
}

/// What a decision tells the viewer to do, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// `new LLOfferInfo` with these fields: the offer the call works on.
    OfferNew(OfferInfo),
    /// The offer's fields changed (transaction consumed, folder redirected,
    /// persisted).
    OfferUpdate(OfferInfo),
    /// `delete` the offer.
    OfferDelete,
    /// `msg->sendReliable(host)` / `gAgent.sendReliableMessage()` of a
    /// message built field by field.
    SendReliable { packet: ImPacket, route: Route },
    /// `pack_instant_message(...)` into the message system.
    PackInstantMessage(PackArgs),
    /// `gAgent.sendReliableMessage()` of the packed message.
    SendPackedToRegion,
    /// `LLNotificationsUtil::add(name, substitutions, payload[, functor])`.
    Notify {
        name: String,
        substitutions: Value,
        payload: Value,
        handler: Option<Handler>,
    },
    /// `LLPostponedNotification::add<LLPostponedOfferNotification>`: posted
    /// once the sender's name resolves, with `NAME` set to it; the current
    /// offer is its responder.
    NotifyPostponed {
        name: String,
        substitutions: Value,
        payload: Value,
        offer_from_agent: bool,
        from_id: Uuid,
        is_group: bool,
    },
    /// `LLNotifications::forceResponse`'s temporary notification answered
    /// with `response` (it is never live).
    ForceRespond { name: String, response: Value },
    /// `LLAvatarNameCache::get(id, callback)`.
    AvatarNameLookup { id: Uuid, then: NameThen },
    /// `gCacheName->getGroup(id, callback)`.
    GroupNameLookup { id: Uuid, then: NameThen },
    /// `gCacheName->get(id, is_group, callback)`.
    CacheNameGet {
        id: Uuid,
        is_group: bool,
        then: NameThen,
    },
    /// `observer->done()`.
    ObserverDone(ObserverId),
    /// `gInventory.addObserver(observer)`.
    ObserverAdd(ObserverId),
    /// `delete observer` (without `done`).
    ObserverDrop(ObserverId),
    /// `LLRecentPeople::instance().add(id)`.
    RecentPeopleAdd(Uuid),
    /// `LLFirstUse::newInventory()`.
    FirstUseNewInventory,
    /// `FSCommon::report_to_nearby_chat(message)`.
    NearbyChat(Vec<u8>),
    /// `make_ui_sound(name)`.
    UiSound(&'static str),
    /// `LLInventoryPanel::openInventoryPanelAndSetSelection(auto_open, id)`.
    InventoryPanelSelect { auto_open: bool, id: Uuid },
    /// `LLPanelGroup::showNotice(...)`; with `with_offer` the current offer
    /// is handed to the panel (else `NULL`).
    GroupPanelShowNotice {
        subject: Vec<u8>,
        message: Vec<u8>,
        group_id: Uuid,
        has_inventory: bool,
        inventory_name: Vec<u8>,
        with_offer: bool,
    },
    /// `RlvBehaviourNotifyHandler::sendNotification(text)`.
    RlvSendNotification(Vec<u8>),
    /// `RlvUtil::notifyBlocked(RlvStringKeys::Blocked::Share, args)`.
    RlvNotifyBlockedShare { args: Value },
    /// `LLAppViewer::postToMainCoro(processNewMessage(...))`.
    PostToMainCoro(OfflineMessage),
    /// The give beam: `createViewerEffect(LL_HUD_EFFECT_BEAM)`, source the
    /// avatar, target `gObjectList.findObject(to)`, `LL_HUD_DUR_SHORT`, the
    /// agent's effect colour.
    HudBeam { to: Uuid },
    /// `gFloaterTools->dirty()`.
    FloaterToolsDirty,
    /// `LLMuteList::autoRemove(id, reason)` (`AR_INVENTORY` = 2).
    MuteAutoRemove { id: Uuid, reason: i32 },
    /// `gIMMgr->addSystemMessage(session, name, args)`.
    ImSystemMessage {
        session: Uuid,
        name: String,
        args: Value,
    },
    /// `LLUIString(template).setArgs(args)` logged with
    /// `LLIMModel::logToFile(file, from, id, line)`.
    ImLogToFile {
        file: Vec<u8>,
        from: Vec<u8>,
        id: Uuid,
        template: Vec<u8>,
        args: Value,
    },
    /// `gInventory.deleteObject(id)`.
    DeleteObject(Uuid),
    /// `gInventory.notifyObservers()`.
    NotifyObservers,
    /// `LLFloaterReg::hideInstance(name)`.
    FloaterHide(&'static str),
    /// `warn_move_inventory(object, move_inv)`.
    WarnMoveInventory(MoveInv),
    /// `move_task_inventory_callback(notification, response, move_inv)`
    /// (through `MoveInventoryFromObject`'s forced response).
    MoveTaskInventory {
        notification: Value,
        response: Value,
        mv: MoveInv,
    },
}

/// What a decision reads from the viewer, and the effects with a result.
pub trait Host {
    /// Carries out an action (in order; state changes are visible to the
    /// queries after it).
    fn act(&mut self, action: Action);

    // ------------------------------------------------------------ agent
    fn agent_id(&self) -> Uuid;
    fn agent_session_id(&self) -> Uuid;
    /// `gAgent.getGroupID()`.
    fn agent_group_id(&self) -> Uuid;
    fn agent_position(&self) -> [f32; 3];
    fn agent_region_host(&self) -> NetHost;
    fn is_do_not_disturb(&self) -> bool;
    /// `gAgentAvatarp` is not null.
    fn have_avatar(&self) -> bool;
    /// `isAgentAvatarValid()`.
    fn avatar_valid(&self) -> bool;
    /// `gAgentAvatarp->getFullname()`.
    fn avatar_fullname(&self) -> Vec<u8>;
    fn is_wearing_attachment(&self, id: &Uuid) -> bool;
    /// `gAgentWearables.isWearingItem`.
    fn is_wearing_item(&self, id: &Uuid) -> bool;
    /// `LLGestureMgr::isGestureActive`.
    fn is_gesture_active(&self, id: &Uuid) -> bool;
    fn avatar_sitting(&self) -> bool;
    /// `gAgentAvatarp->getRoot()`'s id.
    fn avatar_root(&self) -> Option<Uuid>;

    // --------------------------------------------------------- settings
    fn setting_bool(&self, name: &str) -> bool;
    fn setting_u32(&self, name: &str) -> u32;
    /// `LLIMProcessing::getAutoresponseTextForAvatar(from, true, ...)`.
    fn autoresponse_text(&self, from: &Uuid) -> Vec<u8>;

    // ---------------------------------------------------------- strings
    /// `LLTrans::getString(key[, args])`; `args` sorted by key (a
    /// `format_map_t`).
    fn trans(&self, key: &str, args: &[(&str, Vec<u8>)]) -> Vec<u8>;
    /// `LLSLURL(url).getLocationString()`.
    fn slurl_location(&self, url: &[u8]) -> Vec<u8>;
    /// `RlvStrings::getAnonym(name)`.
    fn rlv_anonym(&self, name: &[u8]) -> Vec<u8>;
    /// `RlvStrings::getAnonym(av_name)` when `LLAvatarNameCache::get(id,
    /// &av_name)` finds the name, else `None`.
    fn rlv_anonym_avatar(&self, id: &Uuid) -> Option<Vec<u8>>;
    /// `LLSLURL(command, id, verb).getSLURLString()`.
    fn slurl(&self, command: &str, id: &Uuid, verb: &[u8]) -> Vec<u8>;
    /// `LLAppViewer::instance()->getSecondLifeTitle()`.
    fn trans_app_title(&self) -> Vec<u8>;
    /// `LLAvatarName::trimResidentSurname()`.
    fn trim_resident_surname(&self) -> bool;

    // ------------------------------------------------------------- RLVa
    /// `rlv_handler_t::isEnabled()` / `RlvActions::isRlvEnabled()`.
    fn rlv_enabled(&self) -> bool;
    fn rlv_can_show_name(&self, id: &Uuid) -> bool;
    fn rlv_can_give_inventory(&self, id: &Uuid) -> bool;
    fn rlv_is_nearby(&self, id: &Uuid) -> bool;
    fn rlv_has_open_im(&self, id: &Uuid) -> bool;
    fn rlv_has_open_profile(&self, id: &Uuid) -> bool;
    /// `RlvSettings::getForbidGiveToRLV()`.
    fn rlv_forbid_give_to_rlv(&self) -> bool;
    /// `RlvInventory::getSharedRoot()` is not null.
    fn rlv_has_shared_root(&self) -> bool;
    /// `RlvInventory::getSharedRootID()`.
    fn rlv_shared_root_id(&self) -> Uuid;
    /// `gRlvAttachmentLocks.isLockedAttachment(root)`.
    fn rlv_locked_attachment(&self, root: &Uuid) -> bool;
    fn rlv_has_behaviour(&self, b: RlvBehaviour) -> bool;

    // ---------------------------------------------------- mutes, names
    /// `LLMuteList::isMuted(id, name, flags)`.
    fn is_muted(&self, id: &Uuid, name: &[u8], flags: u32) -> bool;
    /// `LLAvatarNameCache::get(id, &av_name)` and `getUserName()`.
    fn avatar_user_name(&self, id: &Uuid) -> Option<Vec<u8>>;
    /// `LLAvatarNameCache::get(id, &av_name)` and `getCompleteName()`.
    fn avatar_complete_name(&self, id: &Uuid) -> Option<Vec<u8>>;
    /// `gCacheName->getGroupName(id, name)`.
    fn group_name(&self, id: &Uuid) -> Option<Vec<u8>>;
    /// `LLAvatarNameCache::findIdByName(legacy_name)`.
    fn find_id_by_name(&self, name: &[u8]) -> Uuid;

    // ---------------------------------------------------- notifications
    /// `LLNotifications::instance().find(id) != NULL`.
    fn notification_live(&self, id: &Uuid) -> bool;
    /// A notification template's form (`LLNotificationForm::asLLSD`), `None`
    /// when it has none.
    fn notification_form(&self, name: &str) -> Option<Value>;

    // ------------------------------------------------------------ model
    fn item(&self, id: &Uuid) -> Option<ModelItem>;
    fn category(&self, id: &Uuid) -> Option<ModelCat>;
    /// The children of `parent` in the order the model holds them
    /// (`mParentChildCategoryTree`, `mParentChildItemTree`): categories,
    /// items.
    fn children(&self, parent: &Uuid) -> (Vec<ModelCat>, Vec<ModelItem>);
    fn is_category_complete(&self, id: &Uuid) -> bool;
    /// `gInventory.findCategoryUUIDForType(type)`.
    fn find_category_for_type(&mut self, ty: FolderType) -> Uuid;

    // ------------------------------------------------------------ world
    /// `LLUUID::generate()` (random).
    fn generate_uuid(&mut self) -> Uuid;
    /// `LLDate::now()`, seconds since the epoch.
    fn now(&self) -> f64;
    /// The frame timer's clock (seconds).
    fn frame_time(&self) -> f64;
    /// `LLStartUp::getStartupState() >= STATE_STARTED`.
    fn startup_started(&self) -> bool;
    /// The offer throttle's state (`gThrottleTimer` and
    /// `check_offer_throttle`'s statics).
    fn throttle(&mut self) -> &mut Throttle;
    /// `LLIMModel::findIMSession(id)`'s session name.
    fn im_session_name(&self, session: &Uuid) -> Option<Vec<u8>>;
    /// `LLWorld::getMaxInventoryItemsTransfer()`.
    fn max_inventory_items_transfer(&self) -> i32;
    /// `gObjectList.findObject(id)`.
    fn object(&self, id: &Uuid) -> Option<ViewerObject>;
    /// The object's inventory list (`mInventory`), every entry.
    fn object_inventory(&self, id: &Uuid) -> Vec<TaskObject>;
    /// `gFloaterTools` is not null.
    fn have_floater_tools(&self) -> bool;

    // -------------------------------------------------------- observers
    /// `new <observer>`.
    fn observer_new(&mut self, kind: ObserverKind) -> ObserverId;
    /// `observer->startFetch()`; returns `isFinished()`.
    fn observer_start(&mut self, observer: ObserverId) -> bool;
}
