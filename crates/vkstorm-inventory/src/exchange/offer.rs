//! Received inventory offers (W6): the `IM_INVENTORY_OFFERED`,
//! `IM_TASK_INVENTORY_OFFERED`, `IM_GROUP_NOTICE` and
//! `IM_GROUP_NOTICE_REQUESTED` cases of `LLIMProcessing::processNewMessage`
//! (`llimprocessing.cpp`), `inventory_offer_handler`, the ReadOfflineMsgs
//! message loop, and `LLOfferInfo`'s responses and callbacks
//! (`llviewermessage.cpp`).
//!
//! Each function stands for one `r4b_boundary::Decisions` method. The offer
//! a call works on is passed as `&mut OfferInfo`; every change to it is also
//! sent to the executor ([`Action::OfferNew`], [`Action::OfferUpdate`],
//! [`Action::OfferDelete`]) at the point the C++ makes it, so the viewer's
//! copy is current whenever the executor looks at it.

use vkstorm_llsd::Value;

use crate::llsd::coerce::{as_binary, as_integer, as_real, as_string, as_uuid, get, has, insert, is_integer};
use crate::perms::{Uuid, NULL_ID};
use crate::text::trim;
use crate::types::{AssetType, FolderType};

use super::host::{string_field, Action, Host, ImPacket, NameThen, ObserverKind, OfflineMessage, PackArgs, Route};
use super::im::*;
use super::info::{NetHost, OfferInfo};
use super::names::{
    build_legacy_name, c_str, clean_full_name, find_bytes, is_linden, parse_uuid, rfind_bytes, stream_read_s32, uri_escape,
    uuid_validate, PipeTokens,
};
use super::notify::{force_response as notify_force_response, selected_option};
use super::throttle::check_offer_throttle;
use super::xxh3::uuid_from_string;

/// `RLV_PUTINV_PREFIX` (`rlvdefines.h`).
pub const RLV_PUTINV_PREFIX: &[u8] = b"#RLV/~";

/// `LLMute::flagTextChat`.
pub const MUTE_FLAG_TEXT_CHAT: u32 = 0x1;

/// What the legacy task bucket's type reads as when its text holds nothing
/// but white space before `'|'` or its end: the C++ leaves `S32 type`
/// uninitialised (the stack's value; undefined behaviour). This port uses
/// `AT_NONE`, which declines the offer as a bad type.
pub const UNINITIALISED_LEGACY_TYPE: AssetType = AssetType::NONE;

fn sync(host: &mut dyn Host, info: &OfferInfo) {
    host.act(Action::OfferUpdate(info.clone()));
}

fn find_from(hay: &[u8], needle: &[u8], from: Option<usize>) -> Option<usize> {
    let from = from?;
    if from > hay.len() {
        return None;
    }
    find_bytes(&hay[from..], needle).map(|i| i + from)
}

fn uuid_text(id: &Uuid) -> Vec<u8> {
    let mut out = Vec::with_capacity(36);
    vkstorm_llsd::text::uuid_to_hex(id, &mut out);
    out
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// `LLAgentUI::buildFullname`: the avatar's full name when it is valid, else
/// empty.
pub fn build_fullname(host: &dyn Host) -> Vec<u8> {
    if host.avatar_valid() { host.avatar_fullname() } else { Vec::new() }
}

/// The RLVa name check the offer code repeats (`fRlvCanShowName`): RLVa off,
/// or names may be shown, or the sender is not nearby, or an IM or profile
/// of them is open.
fn rlv_can_show_name(host: &dyn Host, id: &Uuid) -> bool {
    !host.rlv_enabled()
        || host.rlv_can_show_name(id)
        || !host.rlv_is_nearby(id)
        || host.rlv_has_open_im(id)
        || host.rlv_has_open_profile(id)
}

/// `RlvInventory::isGiveToRLVOffer`.
pub fn is_give_to_rlv_offer(host: &dyn Host, info: &OfferInfo) -> bool {
    if !host.rlv_forbid_give_to_rlv() && host.rlv_has_shared_root() {
        if info.from_object {
            info.im == IM_TASK_INVENTORY_OFFERED && info.ty == AssetType::CATEGORY && find_bytes(&info.desc, RLV_PUTINV_PREFIX) == Some(1)
        } else {
            info.im == IM_INVENTORY_OFFERED && info.ty == AssetType::CATEGORY && find_bytes(&info.desc, RLV_PUTINV_PREFIX) == Some(0)
        }
    } else {
        false
    }
}

/// `LLOfferInfo::getSanitizedDescription`: the text between the first and
/// the last `'` wrapped in `<nolink>`, when they differ.
pub fn sanitized_description(desc: &[u8]) -> Vec<u8> {
    let start = desc.iter().position(|&c| c == b'\'');
    let end = desc.iter().rposition(|&c| c == b'\'');
    match (start, end) {
        (Some(s), Some(e)) if s != e => {
            let mut d = desc.to_vec();
            d.splice(s..s, b"<nolink>".iter().copied());
            let at = e + 8;
            d.splice(at..at, b"</nolink>".iter().copied());
            d
        }
        _ => desc.to_vec(),
    }
}

// ------------------------------------------------------------- responses

/// `LLOfferInfo::sendReceiveResponse(accept, destination_folder_id)`.
pub fn send_receive_response(host: &mut dyn Host, info: &mut OfferInfo, accept: bool, folder: &Uuid) {
    if info.im == IM_INVENTORY_OFFERED && rlv_can_show_name(host, &info.from_id) {
        host.act(Action::RecentPeopleAdd(info.from_id));
    }
    if info.transaction_id == NULL_ID {
        return;
    }
    let im = if info.im == IM_GROUP_NOTICE_REQUESTED { IM_GROUP_NOTICE } else { info.im };
    let (dialog, bucket) = if accept { (im + 1, folder.to_vec()) } else { (im + 2, vec![0]) };
    let packet = response_packet(host, info, dialog, bucket);
    host.act(Action::SendReliable { packet, route: Route::Host(info.host) });
    info.transaction_id = NULL_ID;
    sync(host, info);
}

fn response_packet(host: &dyn Host, info: &OfferInfo, dialog: i32, bucket: Vec<u8>) -> ImPacket {
    ImPacket {
        agent_id: host.agent_id(),
        session_id: host.agent_session_id(),
        from_group: false,
        to_agent_id: info.from_id,
        parent_estate_id: 0,
        region_id: NULL_ID,
        position: host.agent_position(),
        offline: IM_ONLINE,
        dialog: dialog as u8,
        id: info.transaction_id,
        timestamp: NO_TIMESTAMP,
        from_agent_name: string_field(&build_fullname(host)),
        // addStringFast(_PREHASH_Message, ""): a C string, so one NUL
        message: vec![0],
        binary_bucket: bucket,
    }
}

/// `LLOfferInfo::send_auto_receive_response()`: accept into the offer's
/// folder.
pub fn send_auto_receive_response(host: &mut dyn Host, info: &mut OfferInfo) {
    let folder = info.folder_id;
    send_receive_response(host, info, true, &folder);
}

/// `LLOfferInfo::send_decline_response()` (Firestorm's V1-like decline):
/// dialog `mIM + 2` with the Trash folder, no null-transaction guard, the
/// transaction kept.
pub fn send_decline_response(host: &mut dyn Host, info: &mut OfferInfo) {
    let trash = host.find_category_for_type(FolderType::TRASH);
    let packet = response_packet(host, info, info.im + 2, trash.to_vec());
    host.act(Action::SendReliable { packet, route: Route::Host(info.host) });
}

/// `send_do_not_disturb_message(msg, from_id, session_id)`.
pub fn send_do_not_disturb_message(host: &mut dyn Host, from_id: &Uuid, session_id: &Uuid) {
    if !host.is_do_not_disturb() {
        return;
    }
    let name = build_fullname(host);
    let response = host.autoresponse_text(from_id);
    host.act(Action::PackInstantMessage(PackArgs {
        from_id: host.agent_id(),
        from_group: false,
        session_id: host.agent_session_id(),
        to_id: *from_id,
        name,
        message: response,
        offline: IM_ONLINE,
        dialog: IM_DO_NOT_DISTURB_AUTO_RESPONSE,
        id: *session_id,
        parent_estate_id: 0,
        region_id: NULL_ID,
        position: [0.0; 3],
        timestamp: NO_TIMESTAMP,
        bucket: Some(vec![0]),
    }));
    host.act(Action::SendPackedToRegion);
}

// ---------------------------------------------------------- the callbacks

/// `LLOfferInfo::forceResponse(response)`: the `UserGiveItem` form element
/// (not button) the response names (ACCEPT 1, DECLINE 2, MUTE 3; any other
/// response passes -1 and nothing is called), answered through
/// [`inventory_offer_callback`] whatever kind of offer it is.
pub fn force_response(host: &mut dyn Host, info: &mut OfferInfo, response: i32) {
    let element = match response {
        IOR_ACCEPT => 1,
        IOR_DECLINE => 2,
        IOR_MUTE => 3,
        _ => -1,
    };
    if let Some((notification, resp)) = notify_force_response(host, "UserGiveItem", element) {
        inventory_offer_callback(host, info, &notification, &resp, false);
    }
}

/// `LLOfferInfo::handleRespond(notification, response)`: the callback the
/// notification's name selects (`ObjectGiveItem` and `OwnObjectGiveItem`:
/// [`inventory_task_offer_callback`]; `UserGiveItem` and
/// `UserGiveItemLegacy`: [`inventory_offer_callback`]); an unknown name does
/// nothing.
pub fn respond(host: &mut dyn Host, info: &mut OfferInfo, notification: &Value, response: &Value) {
    let name = as_string(get(notification, "name"));
    let live = host.notification_live(&as_uuid(get(notification, "id")));
    match name.as_slice() {
        b"ObjectGiveItem" | b"OwnObjectGiveItem" => inventory_task_offer_callback(host, info, notification, response, live),
        b"UserGiveItem" | b"UserGiveItemLegacy" => inventory_offer_callback(host, info, notification, response, live),
        _ => {}
    }
}

fn start_observer(host: &mut dyn Host, kind: ObserverKind) -> (super::host::ObserverId, bool) {
    let o = host.observer_new(kind);
    let finished = host.observer_start(o);
    (o, finished)
}

fn give_to_rlv_agent_offer(host: &mut dyn Host, info: &OfferInfo) {
    if host.rlv_enabled() && !host.rlv_forbid_give_to_rlv() && info.ty == AssetType::CATEGORY && find_bytes(&info.desc, RLV_PUTINV_PREFIX) == Some(0) {
        let (o, finished) = start_observer(host, ObserverKind::RlvGiveToRlvAgentOffer { folder: info.object_id });
        host.act(if finished { Action::ObserverDone(o) } else { Action::ObserverAdd(o) });
    }
}

fn system_message_tip(host: &mut dyn Host, message: Vec<u8>) {
    let mut args = Value::Undefined;
    insert(&mut args, "MESSAGE", Value::String(message));
    host.act(Action::Notify { name: "SystemMessageTip".to_owned(), substitutions: args, payload: Value::Undefined, handler: None });
}

/// `LLOfferInfo::inventory_offer_callback` (agent offers and group notices,
/// and every forced response). `live` is whether the notification is in the
/// store (`LLNotifications::find`), which a Mute needs to look the sender up.
pub fn inventory_offer_callback(host: &mut dyn Host, info: &mut OfferInfo, notification: &Value, response: &Value, live: bool) {
    let button = selected_option(notification, response);
    let catp = host.category(&info.object_id).is_some();
    let itemp = if catp { None } else { host.item(&info.object_id) };

    if button == IOR_MUTE && live {
        if info.from_group {
            host.act(Action::GroupNameLookup { id: info.from_id, then: NameThen::OfferMuteGroup });
        } else {
            host.act(Action::AvatarNameLookup { id: info.from_id, then: NameThen::OfferMuteAvatar });
        }
    }

    let chat_history = info.from_name.clone();
    let mut accept_to_trash = true;
    let mut opener = None;
    let legacy = host.setting_bool("FSUseLegacyInventoryAcceptMessages");

    match button {
        IOR_SHOW | IOR_SHOW_SILENT => match info.im {
            IM_INVENTORY_OFFERED => {
                give_to_rlv_agent_offer(host, info);
                let (o, _) = start_observer(
                    host,
                    ObserverKind::OpenAgentOffer { id: info.object_id, from_name: chat_history.clone(), manual: true },
                );
                if catp || itemp.as_ref().is_some_and(|i| i.finished) {
                    host.act(Action::ObserverDone(o));
                } else {
                    opener = Some(o);
                }
                if host.setting_bool("FSUseLegacyInventoryAcceptMessages") && button == IOR_SHOW {
                    send_auto_receive_response(host, info);
                }
            }
            IM_GROUP_NOTICE | IM_GROUP_NOTICE_REQUESTED => {
                opener = Some(host.observer_new(ObserverKind::OpenTaskGroupOffer));
                let folder = info.folder_id;
                send_receive_response(host, info, true, &folder);
            }
            _ => {}
        },
        IOR_ACCEPT | IOR_ACCEPT_SILENT => {
            if check_offer_throttle(host, &info.from_name, true) {
                let log_message = cat(&[b" ", &host.trans("InvOfferGaveYou", &[]), b" ", &sanitized_description(&info.desc), &host.trans(".", &[])]);
                let sep = info.from_name.iter().position(|&c| c == b'|');
                let parsed = sep.and_then(|s| parse_uuid(&info.from_name[..s]).map(|id| (s, id)));
                match parsed {
                    Some((s, id)) if info.from_name.len() > s + 1 => {
                        host.act(Action::AvatarNameLookup {
                            id,
                            then: NameThen::SystemMessageTip { format: info.from_name[s + 1..].to_vec(), message: log_message },
                        });
                    }
                    _ => system_message_tip(host, cat(&[&chat_history, &log_message])),
                }
            }
            if info.im == IM_GROUP_NOTICE || info.im == IM_GROUP_NOTICE_REQUESTED {
                opener = Some(host.observer_new(ObserverKind::OpenTaskGroupOffer));
                let folder = info.folder_id;
                send_receive_response(host, info, true, &folder);
            } else if info.im == IM_INVENTORY_OFFERED {
                give_to_rlv_agent_offer(host, info);
                if legacy && button == IOR_ACCEPT {
                    send_auto_receive_response(host, info);
                }
                if host.setting_bool("ShowInInventory") {
                    host.act(Action::InventoryPanelSelect { auto_open: true, id: info.object_id });
                }
            }
        }
        IOR_MUTE | IOR_DECLINE | IOR_DECLINE_SILENT => {
            if button == IOR_MUTE {
                accept_to_trash = false;
            }
            if host.is_muted(&info.from_id, b"", 0) && !is_linden(&info.from_name) {
                accept_to_trash = false;
            }
            if info.object_id != NULL_ID {
                let (o, _) = start_observer(host, ObserverKind::DiscardAgentOffer { folder: info.folder_id, object: info.object_id });
                if (catp && host.is_category_complete(&info.object_id)) || itemp.as_ref().is_some_and(|i| i.finished) {
                    host.act(Action::ObserverDone(o));
                } else {
                    opener = Some(o);
                }
            } else if info.im == IM_GROUP_NOTICE && host.setting_bool("FSDismissGroupNoticeAttachmentsToTrash") {
                let trash = host.find_category_for_type(FolderType::TRASH);
                send_receive_response(host, info, accept_to_trash, &trash);
            }
            if host.setting_bool("FSUseLegacyInventoryAcceptMessages") && button == IOR_DECLINE && info.im == IM_INVENTORY_OFFERED {
                send_decline_response(host, info);
            }
        }
        _ => {
            if info.im == IM_GROUP_NOTICE && host.setting_bool("FSDismissGroupNoticeAttachmentsToTrash") {
                let trash = host.find_category_for_type(FolderType::TRASH);
                send_receive_response(host, info, true, &trash);
            }
        }
    }

    if let Some(o) = opener {
        host.act(Action::ObserverAdd(o));
    }
    if !info.persist {
        host.act(Action::OfferDelete);
    }
}

/// `LLOfferInfo::inventory_task_offer_callback` (object offers answered
/// through their notification).
pub fn inventory_task_offer_callback(host: &mut dyn Host, info: &mut OfferInfo, notification: &Value, response: &Value, live: bool) {
    let button = selected_option(notification, response);
    if button == 2 && live {
        if info.from_group {
            host.act(Action::GroupNameLookup { id: info.from_id, then: NameThen::OfferMuteGroup });
        } else {
            host.act(Action::AvatarNameLookup { id: info.from_id, then: NameThen::OfferMuteAvatar });
        }
    }

    let chat_history = if info.from_object {
        let quot = host.trans("'", &[]);
        if info.from_group {
            match host.group_name(&info.from_id) {
                Some(group_name) => cat(&[&info.from_name, b" ", &host.trans("InvOfferOwnedByGroup", &[]), b" ", &quot, &group_name, &quot]),
                None => cat(&[&info.from_name, b" ", &host.trans("InvOfferOwnedByUnknownGroup", &[])]),
            }
        } else {
            let mut name_slurl = host.slurl("agent", &info.from_id, b"about");
            if !rlv_can_show_name(host, &info.from_id) {
                name_slurl = host.slurl("agent", &info.from_id, b"rlvanonym");
            }
            cat(&[&info.from_name, b" ", &host.trans("InvOfferOwnedBy", &[]), b" ", &name_slurl])
        }
    } else {
        info.from_name.clone()
    };

    let give_to_rlv = |host: &dyn Host, info: &OfferInfo| {
        host.rlv_enabled() && info.im == IM_TASK_INVENTORY_OFFERED && info.ty == AssetType::CATEGORY && find_bytes(&info.desc, RLV_PUTINV_PREFIX) == Some(1)
    };
    let rlv_object_name = |info: &OfferInfo| find_bytes(&info.desc, b"'  ( http://").map(|i| info.desc[1..i.max(1)].to_vec());

    let destination;
    let mut accept = true;
    if button == IOR_ACCEPT {
        if give_to_rlv(host, info) {
            if !host.rlv_forbid_give_to_rlv() {
                let root = host.rlv_shared_root_id();
                if root != NULL_ID {
                    info.folder_id = root;
                    sync(host, info);
                }
                let o = host.observer_new(ObserverKind::RlvGiveToRlvTaskOffer { transaction: info.transaction_id });
                host.act(Action::ObserverAdd(o));
            } else if let Some(name) = rlv_object_name(info) {
                host.act(Action::RlvSendNotification(cat(&[b"accepted_in_inv inv_offer ", &name])));
            }
        }
        destination = info.folder_id;
        if check_offer_throttle(host, &info.from_name, true) {
            let log = cat(&[&chat_history, b" ", &host.trans("InvOfferGaveYou", &[]), b" ", &sanitized_description(&info.desc), &host.trans(".", &[])]);
            system_message_tip(host, log);
        }
    } else {
        if button == IOR_MUTE {
            accept = false;
        }
        destination = host.find_category_for_type(FolderType::TRASH);
        if give_to_rlv(host, info) {
            if let Some(name) = rlv_object_name(info) {
                host.act(Action::RlvSendNotification(cat(&[b"declined inv_offer ", &name])));
            }
        }
        if host.setting_bool("LogInventoryDecline") {
            let log = host.trans("InvOfferDecline", &[("DESC", info.desc.clone()), ("NAME", info.from_name.clone())]);
            system_message_tip(host, log);
        }
        if accept && host.is_muted(&info.from_id, &info.from_name, 0) {
            accept = false;
        }
    }

    send_receive_response(host, info, accept, &destination);
    if !info.persist {
        host.act(Action::OfferDelete);
    }
}

// -------------------------------------------------------------- arrival

/// The `processNewMessage` arguments of an offer or group notice IM.
#[derive(Clone, Debug)]
pub struct ImArgs<'a> {
    pub from_id: Uuid,
    pub from_group: bool,
    pub dialog: i32,
    pub session_id: Uuid,
    pub timestamp: u32,
    pub name: &'a [u8],
    pub message: &'a [u8],
    /// The buffer the bucket was copied into (`U8[MTUBYTES]` from
    /// `process_improved_im`, or the offline loop's vector): C-string reads
    /// of the legacy formats stop at its first NUL or its end.
    pub buffer: &'a [u8],
    /// The size the message declared (`binary_bucket_size`).
    pub bucket_size: i32,
    pub sender: NetHost,
    pub aux_id: Uuid,
}

impl<'a> ImArgs<'a> {
    fn byte(&self, i: usize) -> u8 {
        self.buffer.get(i).copied().unwrap_or(0)
    }

    fn sized(&self) -> &'a [u8] {
        let n = (self.bucket_size.max(0) as usize).min(self.buffer.len());
        &self.buffer[..n]
    }
}

/// `processNewMessage` for dialogs 4, 9, 32 and 37: the name trimmed (empty:
/// the `Unnamed` string) and, for the dialogs `clean_name_from_im` covers,
/// cleaned of `" Resident"`; the mute verdict (`flagTextChat` exempt); then
/// the dialog's case. Other dialogs do nothing here.
pub fn receive_im(host: &mut dyn Host, im: &ImArgs<'_>) {
    let mut name = trim(im.name).to_vec();
    if name.is_empty() {
        name = host.trans("Unnamed", &[]);
    }
    let original_name = name.clone();
    if im_name_is_cleaned(im.dialog) {
        name = clean_full_name(&name, host.trim_resident_surname());
    }
    let is_do_not_disturb = host.is_do_not_disturb();
    let is_muted = host.is_muted(&im.from_id, &name, MUTE_FLAG_TEXT_CHAT)
        || (im.dialog == IM_FROM_TASK && host.is_muted(&im.session_id, b"", 0));

    match im.dialog {
        IM_GROUP_NOTICE | IM_GROUP_NOTICE_REQUESTED => group_notice(host, im, &name, original_name),
        IM_INVENTORY_OFFERED | IM_TASK_INVENTORY_OFFERED => inventory_offered(host, im, name, is_muted, is_do_not_disturb),
        _ => {}
    }
}

/// The legacy task bucket's type: the C string up to `'|'` read with
/// `istream >> S32` (`None`: the C++ leaves it uninitialised).
pub fn legacy_task_type(buffer: &[u8]) -> Option<i32> {
    let s = c_str(buffer);
    let t = &s[..s.iter().position(|&c| c == b'|').unwrap_or(s.len())];
    stream_read_s32(t)
}

fn inventory_offered(host: &mut dyn Host, im: &ImArgs<'_>, name: Vec<u8>, is_muted: bool, is_do_not_disturb: bool) {
    let mut info = OfferInfo::default();
    host.act(Action::OfferNew(info.clone()));
    if im.dialog == IM_INVENTORY_OFFERED {
        if im.bucket_size != 17 {
            host.act(Action::OfferDelete);
            return;
        }
        info.ty = AssetType(im.byte(0) as i8 as i32);
        let mut id = [0u8; 16];
        for (i, b) in id.iter_mut().enumerate() {
            *b = im.byte(1 + i);
        }
        info.object_id = id;
        info.from_object = false;
    } else {
        if im.bucket_size == 1 {
            info.ty = AssetType(im.byte(0) as i32);
        } else {
            info.ty = legacy_task_type(im.buffer).map_or(UNINITIALISED_LEGACY_TYPE, AssetType);
        }
        info.object_id = NULL_ID;
        info.from_object = true;
    }
    info.im = im.dialog;
    info.from_id = im.from_id;
    info.from_group = im.from_group;
    info.folder_id = host.find_category_for_type(FolderType::from_asset_type(info.ty));
    info.transaction_id = if im.session_id != NULL_ID { im.session_id } else { im.aux_id };
    info.from_name = name;
    info.desc = im.message.to_vec();
    info.host = im.sender;
    sync(host, &info);

    if is_muted {
        if im.dialog == IM_INVENTORY_OFFERED {
            let o = host.observer_new(ObserverKind::FetchItems { id: info.object_id });
            host.observer_start(o);
            host.act(Action::ObserverDrop(o));
            force_response(host, &mut info, IOR_DECLINE);
        } else {
            force_response(host, &mut info, IOR_MUTE);
        }
    } else if is_do_not_disturb && im.dialog == IM_TASK_INVENTORY_OFFERED {
        force_response(host, &mut info, IOR_DECLINE);
    } else {
        inventory_offer_handler(host, &mut info);
    }
}

fn group_notice(host: &mut dyn Host, im: &ImArgs<'_>, name: &[u8], mut original_name: Vec<u8>) {
    let mut from_group = im.from_group;
    let has_inventory: bool;
    let mut asset_type: u8 = 0;
    let group_id: Uuid;
    let mut item_name: Vec<u8> = Vec::new();

    if im.aux_id != NULL_ID {
        group_id = im.aux_id;
        has_inventory = im.bucket_size > 1;
        from_group = true;
        if has_inventory {
            let tokens = PipeTokens::new(im.sized());
            asset_type = super::names::atoi(tokens.token(0)) as u8;
            item_name = c_str(tokens.token(2)).to_vec();
        }
    } else {
        if im.bucket_size < 19 || im.byte(im.bucket_size as usize - 1) != 0 {
            return;
        }
        has_inventory = im.byte(0) != 0;
        asset_type = im.byte(1);
        let mut g = [0u8; 16];
        for (i, b) in g.iter_mut().enumerate() {
            *b = im.byte(2 + i);
        }
        group_id = g;
        item_name = c_str(im.buffer.get(18..).unwrap_or(&[])).to_vec();
    }

    let agent_id = if group_id != im.from_id {
        im.from_id
    } else {
        if let Some(i) = find_bytes(&original_name, b" Resident") {
            original_name.truncate(i);
        }
        let legacy_name = build_legacy_name(&original_name);
        host.find_id_by_name(&legacy_name)
    };

    if agent_id != NULL_ID && host.is_muted(&agent_id, b"", 0) {
        return;
    }

    let mut info: Option<OfferInfo> = None;
    if has_inventory {
        let mut o = OfferInfo::default();
        host.act(Action::OfferNew(o.clone()));
        o.im = im.dialog;
        o.from_id = im.from_id;
        o.from_group = from_group;
        o.transaction_id = im.session_id;
        o.ty = AssetType(asset_type as i32);
        o.folder_id = host.find_category_for_type(FolderType::from_asset_type(o.ty));
        let mut from_name = host.trans("InvOfferGroupNoticeName", &[("NAME", name.to_vec())]);
        if uuid_validate(name) {
            let placeholder = host.trans("InvOfferGroupNoticeName", &[("NAME", b"%s".to_vec())]);
            from_name = cat(&[name, b"|", &placeholder]);
        }
        o.from_name = from_name;
        o.desc = item_name.clone();
        o.host = im.sender;
        sync(host, &o);
        info = Some(o);
    }

    let tokens = PipeTokens::new(im.message);
    let subject = tokens.token(0).to_vec();
    let message = tokens.token(1).to_vec();

    if im.dialog != IM_GROUP_NOTICE_REQUESTED {
        let mut payload = Value::Undefined;
        insert(&mut payload, "subject", Value::String(subject.clone()));
        insert(&mut payload, "message", Value::String(message.clone()));
        insert(&mut payload, "sender_name", Value::String(name.to_vec()));
        insert(&mut payload, "sender_id", Value::Uuid(agent_id));
        insert(&mut payload, "group_id", Value::Uuid(group_id));
        insert(&mut payload, "inventory_name", Value::String(item_name.clone()));
        insert(&mut payload, "received_time", Value::Date(host.now()));
        if let Some(o) = &info {
            insert(&mut payload, "inventory_offer", o.as_llsd());
        }
        host.act(Action::CacheNameGet {
            id: group_id,
            is_group: true,
            then: NameThen::GroupNotice { sender: name.to_vec(), subject: subject.clone(), message: message.clone(), payload, timestamp: im.timestamp },
        });
    }

    if im.dialog == IM_GROUP_NOTICE_REQUESTED {
        host.act(Action::GroupPanelShowNotice {
            subject,
            message,
            group_id,
            has_inventory,
            inventory_name: item_name,
            with_offer: info.is_some(),
        });
    } else if info.is_some() {
        host.act(Action::OfferDelete);
    }
}

/// `inventory_offer_handler(info)`: a muted sender (by id and name, or by
/// name alone) gets a forced Mute; an unknown asset type a forced Decline;
/// an object offer (not auto-accepted) its `ObjectGiveItem` /
/// `OwnObjectGiveItem` notification; an agent offer (or an auto-accepted
/// object offer) the item prefetch, the auto receive (unless legacy mode
/// waits for the answer), the do-not-disturb reply, the `UserGiveItem`
/// notification or the auto-accept log, and the opener.
pub fn inventory_offer_handler(host: &mut dyn Host, info: &mut OfferInfo) {
    if host.is_muted(&info.from_id, &info.from_name, 0) || host.is_muted(&NULL_ID, &info.from_name, 0) {
        force_response(host, info, IOR_MUTE);
        return;
    }

    let auto_accept = host.setting_bool("AutoAcceptNewInventory") && (!host.rlv_enabled() || !is_give_to_rlv_offer(host, info));

    let mut msg = info.desc.clone();
    let indx = find_bytes(&msg, b" ( http://slurl.com/secondlife/")
        .or_else(|| find_bytes(&msg, b" ( https://slurl.com/secondlife/"))
        .or_else(|| find_bytes(&msg, b" ( http://maps.secondlife.com/secondlife/"))
        .or_else(|| find_bytes(&msg, b" ( https://maps.secondlife.com/secondlife/"));
    if let Some(i) = indx {
        msg.truncate(i);
    }

    let mut args = Value::Undefined;
    insert(&mut args, "[OBJECTNAME]", Value::String(msg.clone()));
    let mut payload = Value::Undefined;

    let typestr = info.ty.lookup_human_readable().unwrap_or("");
    if !typestr.is_empty() {
        insert(&mut args, "OBJECTTYPE", Value::String(host.trans(typestr, &[])));
    } else {
        insert(&mut args, "OBJECTTYPE", Value::string(""));
        force_response(host, info, IOR_DECLINE);
        return;
    }

    let object_id = if info.object_id == NULL_ID { uuid_from_string(&msg) } else { info.object_id };

    insert(&mut payload, "from_id", Value::Uuid(info.from_id));
    insert(&mut payload, "object_id", Value::Uuid(object_id));
    insert(&mut payload, "give_inventory_notification", Value::Boolean(false));
    insert(&mut args, "OBJECTFROMNAME", Value::String(info.from_name.clone()));
    insert(&mut args, "NAME", Value::String(info.from_name.clone()));
    if info.from_group {
        insert(&mut args, "NAME_SLURL", Value::String(host.slurl("group", &info.from_id, b"about")));
    } else {
        insert(&mut args, "NAME_LABEL", Value::String(host.slurl("agent", &info.from_id, b"completename")));
        insert(&mut args, "NAME_SLURL", Value::String(host.slurl("agent", &info.from_id, b"about")));
    }
    let verb = cat(&[b"select?name=", &uri_escape(&msg)]);
    insert(&mut args, "ITEM_SLURL", Value::String(host.slurl("inventory", &info.object_id, &verb)));

    let posted = !auto_accept;
    if info.from_object && !auto_accept {
        if host.rlv_enabled() && !host.rlv_can_show_name(&info.from_id) && host.rlv_is_nearby(&info.from_id) {
            insert(&mut payload, "rlv_shownames", Value::Boolean(true));
            insert(&mut args, "NAME_SLURL", Value::String(host.slurl("agent", &info.from_id, b"rlvanonym")));
        }
        insert(&mut args, "ITEM_SLURL", Value::String(msg.clone()));
        info.persist = true;
        sync(host, info);
        let name = if info.from_id == host.agent_id() { "OwnObjectGiveItem" } else { "ObjectGiveItem" };
        host.act(Action::NotifyPostponed {
            name: name.to_owned(),
            substitutions: args,
            payload,
            offer_from_agent: false,
            from_id: info.from_id,
            is_group: info.from_group,
        });
    } else {
        if !rlv_can_show_name(host, &info.from_id) {
            insert(&mut payload, "rlv_shownames", Value::Boolean(true));
            let anon = match host.rlv_anonym_avatar(&info.from_id) {
                Some(a) => a,
                None => host.rlv_anonym(&info.from_name),
            };
            insert(&mut args, "NAME", Value::String(anon));
            insert(&mut args, "NAME_SLURL", Value::String(host.slurl("agent", &info.from_id, b"rlvanonym")));
        }
        info.persist = true;
        sync(host, info);
        let legacy = host.setting_bool("FSUseLegacyInventoryAcceptMessages");
        let name = if legacy { "UserGiveItemLegacy" } else { "UserGiveItem" };

        let (fetch, finished) = start_observer(host, ObserverKind::FetchItems { id: info.object_id });
        host.act(if finished { Action::ObserverDone(fetch) } else { Action::ObserverAdd(fetch) });

        if auto_accept || !host.setting_bool("FSUseLegacyInventoryAcceptMessages") {
            send_auto_receive_response(host, info);
        }
        if host.is_do_not_disturb() {
            let from = info.from_id;
            send_do_not_disturb_message(host, &from, &NULL_ID);
        }

        if !auto_accept {
            insert(&mut payload, "give_inventory_notification", Value::Boolean(true));
            host.act(Action::NotifyPostponed {
                name: name.to_owned(),
                substitutions: args,
                payload,
                offer_from_agent: true,
                from_id: info.from_id,
                is_group: false,
            });
        } else if host.setting_bool("FSLogAutoAcceptInventoryToChat") || host.setting_bool("FSShowAutoAcceptInventoryInNotifications") {
            let object_type = host.trans(typestr, &[]);
            let (message_type, chat_args) = if info.from_object {
                let mut str_pos = Vec::new();
                let idx_start = rfind_bytes(&info.desc, b" ( http://");
                let idx_end = find_from(&info.desc, b" )", idx_start);
                if let (Some(s), Some(e)) = (idx_start, idx_end) {
                    let location = host.slurl_location(&info.desc[s + 3..e]);
                    str_pos = cat(&[b"&slurl=", &uri_escape(&location)]);
                }
                let object_name = cat(&[
                    b"[",
                    &host.slurl("objectim", &info.object_id, b""),
                    b"?name=",
                    &uri_escape(&info.from_name),
                    b"&owner=",
                    &uuid_text(&info.from_id),
                    if info.from_group { b"&groupowned=true" } else { b"" },
                    &str_pos,
                    b" ",
                    &info.from_name,
                    b"]",
                ]);
                ("InvOfferAutoAcceptObject", vec![("DESC", msg.clone()), ("OBJECT_NAME", object_name), ("OBJECT_TYPE", object_type)])
            } else {
                let verb: &[u8] = if rlv_can_show_name(host, &info.from_id) { b"inspect" } else { b"rlvanonym" };
                let name_slurl = host.slurl("agent", &info.from_id, verb);
                ("InvOfferAutoAcceptUser", vec![("DESC", msg.clone()), ("OBJECT_TYPE", object_type), ("USER_NAME", name_slurl)])
            };
            let inv_offer_message = host.trans(message_type, &chat_args);
            if host.setting_bool("FSLogAutoAcceptInventoryToChat") {
                host.act(Action::NearbyChat(inv_offer_message.clone()));
            }
            if host.setting_bool("FSShowAutoAcceptInventoryInNotifications") {
                let mut a = Value::Undefined;
                insert(&mut a, "MESSAGE", Value::String(inv_offer_message));
                host.act(Action::Notify { name: "IMSystemMessageTip".to_owned(), substitutions: a, payload: Value::Undefined, handler: None });
            }
            host.act(Action::UiSound("UISndInventoryOffer"));
        }

        if auto_accept && host.setting_bool("ShowNewInventory") {
            let catp = host.category(&info.object_id).is_some();
            let itemp = if catp { None } else { host.item(&info.object_id) };
            let (o, _) = start_observer(
                host,
                ObserverKind::OpenAgentOffer { id: info.object_id, from_name: info.from_name.clone(), manual: false },
            );
            if catp || itemp.is_some_and(|i| i.finished) {
                host.act(Action::ObserverDone(o));
            } else {
                host.act(Action::ObserverAdd(o));
            }
        }
    }

    host.act(Action::FirstUseNewInventory);
    // The notification parameters own the offer (`functor.responder` is a
    // shared pointer): when no notification took them over (an agent offer
    // auto-accepted), the offer is deleted as the handler returns.
    if !posted {
        host.act(Action::OfferDelete);
    }
}

// -------------------------------------------------------- offline offers

/// The ReadOfflineMsgs capability's message loop over its validated
/// `messages` array: each message's `processNewMessage` arguments posted to
/// the main coroutine, with the agent's region host as the sender (replies
/// go there, not to an offer host).
pub fn receive_offline(host: &mut dyn Host, messages: &Value) {
    let sender = host.agent_region_host();
    let Value::Array(list) = messages else { return };
    for m in list {
        let position = if has(m, "position") {
            let p = get(m, "position");
            let at = |i: usize| match p {
                Value::Array(a) => a.get(i).map_or(0.0, as_real),
                _ => 0.0,
            } as f32;
            [at(0), at(1), at(2)]
        } else {
            [as_real(get(m, "local_x")) as f32, as_real(get(m, "local_y")) as f32, as_real(get(m, "local_z")) as f32]
        };
        let bucket = if has(m, "binary_bucket") { as_binary(get(m, "binary_bucket")).to_vec() } else { vec![0] };
        let from_group = if is_integer(get(m, "from_group")) {
            as_integer(get(m, "from_group")) != 0
        } else {
            as_string(get(m, "from_group")) == b"Y"
        };
        let metadata = if has(m, "metadata") { get(m, "metadata").clone() } else { Value::Undefined };
        let dialog = as_integer(get(m, "dialog"));
        let mut session_id = as_uuid(get(m, "transaction-id"));
        if session_id == NULL_ID && dialog == IM_FROM_TASK {
            session_id = as_uuid(get(m, "asset_id"));
        }
        host.act(Action::PostToMainCoro(OfflineMessage {
            from_id: as_uuid(get(m, "from_agent_id")),
            from_group,
            to_id: as_uuid(get(m, "to_agent_id")),
            offline: if has(m, "offline") { as_integer(get(m, "offline")) as u8 } else { IM_OFFLINE },
            dialog,
            session_id,
            timestamp: as_integer(get(m, "timestamp")) as u32,
            name: as_string(get(m, "from_agent_name")),
            message: as_string(get(m, "message")),
            parent_estate_id: if has(m, "parent_estate_id") { as_integer(get(m, "parent_estate_id")) as u32 } else { 1 },
            region_id: as_uuid(get(m, "region_id")),
            position,
            bucket,
            sender,
            metadata,
            aux_id: as_uuid(get(m, "asset_id")),
        }));
    }
}

/// `processNewMessage` for a message the offline loop posted (run from the
/// main coroutine): the bucket is the message's own vector.
pub fn process_offline_message(host: &mut dyn Host, m: &OfflineMessage) {
    receive_im(
        host,
        &ImArgs {
            from_id: m.from_id,
            from_group: m.from_group,
            dialog: m.dialog,
            session_id: m.session_id,
            timestamp: m.timestamp,
            name: &m.name,
            message: &m.message,
            buffer: &m.bucket,
            bucket_size: m.bucket.len() as i32,
            sender: m.sender,
            aux_id: m.aux_id,
        },
    );
}
