//! The instant message dialog values (`EInstantMessage`, `llinstantmessage.h`),
//! the offer responses (`InventoryOfferResponse`, `llviewermessage.h`) and
//! the drag-and-drop enums (`EDragAndDropType`, `EAcceptance`, `llui.h`).
//!
//! Dialogs travel as a raw `i32`: the viewer casts any received byte to the
//! enum, and so does this crate.

pub const IM_NOTHING_SPECIAL: i32 = 0;
pub const IM_MESSAGEBOX: i32 = 1;
pub const IM_GROUP_INVITATION: i32 = 3;
pub const IM_INVENTORY_OFFERED: i32 = 4;
pub const IM_INVENTORY_ACCEPTED: i32 = 5;
pub const IM_INVENTORY_DECLINED: i32 = 6;
pub const IM_GROUP_VOTE: i32 = 7;
pub const IM_GROUP_MESSAGE_DEPRECATED: i32 = 8;
pub const IM_TASK_INVENTORY_OFFERED: i32 = 9;
pub const IM_TASK_INVENTORY_ACCEPTED: i32 = 10;
pub const IM_TASK_INVENTORY_DECLINED: i32 = 11;
pub const IM_NEW_USER_DEFAULT: i32 = 12;
pub const IM_SESSION_INVITE: i32 = 13;
pub const IM_SESSION_P2P_INVITE: i32 = 14;
pub const IM_SESSION_GROUP_START: i32 = 15;
pub const IM_SESSION_CONFERENCE_START: i32 = 16;
pub const IM_SESSION_SEND: i32 = 17;
pub const IM_SESSION_LEAVE: i32 = 18;
pub const IM_FROM_TASK: i32 = 19;
pub const IM_DO_NOT_DISTURB_AUTO_RESPONSE: i32 = 20;
pub const IM_CONSOLE_AND_CHAT_HISTORY: i32 = 21;
pub const IM_LURE_USER: i32 = 22;
pub const IM_LURE_ACCEPTED: i32 = 23;
pub const IM_LURE_DECLINED: i32 = 24;
pub const IM_GODLIKE_LURE_USER: i32 = 25;
pub const IM_TELEPORT_REQUEST: i32 = 26;
pub const IM_GROUP_ELECTION_DEPRECATED: i32 = 27;
pub const IM_GOTO_URL: i32 = 28;
pub const IM_FROM_TASK_AS_ALERT: i32 = 31;
pub const IM_GROUP_NOTICE: i32 = 32;
pub const IM_GROUP_NOTICE_INVENTORY_ACCEPTED: i32 = 33;
pub const IM_GROUP_NOTICE_INVENTORY_DECLINED: i32 = 34;
pub const IM_GROUP_INVITATION_ACCEPT: i32 = 35;
pub const IM_GROUP_INVITATION_DECLINE: i32 = 36;
pub const IM_GROUP_NOTICE_REQUESTED: i32 = 37;
pub const IM_FRIENDSHIP_OFFERED: i32 = 38;
pub const IM_FRIENDSHIP_ACCEPTED: i32 = 39;
pub const IM_FRIENDSHIP_DECLINED_DEPRECATED: i32 = 40;
pub const IM_TYPING_START: i32 = 41;
pub const IM_TYPING_STOP: i32 = 42;

/// `IM_ONLINE`.
pub const IM_ONLINE: u8 = 0;
/// `IM_OFFLINE`.
pub const IM_OFFLINE: u8 = 1;
/// `NO_TIMESTAMP`.
pub const NO_TIMESTAMP: u32 = 0;
/// `MTUBYTES`: the instant message bucket budget.
pub const MTUBYTES: usize = 1200;

/// `InventoryOfferResponse` (with Firestorm's silent variants, FIRE-3832).
pub const IOR_ACCEPT: i32 = 0;
pub const IOR_DECLINE: i32 = 1;
pub const IOR_MUTE: i32 = 2;
pub const IOR_SHOW: i32 = 3;
pub const IOR_ACCEPT_SILENT: i32 = 4;
pub const IOR_DECLINE_SILENT: i32 = 5;
pub const IOR_SHOW_SILENT: i32 = 6;

/// `EDragAndDropType` (`llui.h`).
pub const DAD_NONE: i32 = 0;
pub const DAD_TEXTURE: i32 = 1;
pub const DAD_SOUND: i32 = 2;
pub const DAD_CALLINGCARD: i32 = 3;
pub const DAD_LANDMARK: i32 = 4;
pub const DAD_SCRIPT: i32 = 5;
pub const DAD_CLOTHING: i32 = 6;
pub const DAD_OBJECT: i32 = 7;
pub const DAD_NOTECARD: i32 = 8;
pub const DAD_CATEGORY: i32 = 9;
pub const DAD_ROOT_CATEGORY: i32 = 10;
pub const DAD_BODYPART: i32 = 11;
pub const DAD_ANIMATION: i32 = 12;
pub const DAD_GESTURE: i32 = 13;
pub const DAD_LINK: i32 = 14;
pub const DAD_MESH: i32 = 15;
pub const DAD_WIDGET: i32 = 16;
pub const DAD_PERSON: i32 = 17;
pub const DAD_SETTINGS: i32 = 18;
pub const DAD_MATERIAL: i32 = 19;
pub const DAD_GLTF: i32 = 20;
pub const DAD_GLTF_BIN: i32 = 21;

/// `EAcceptance` (`llui.h`), ordered by priority for multi-drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Acceptance {
    Postponed = 0,
    No = 1,
    NoCustom = 2,
    NoLocked = 3,
    YesCopySingle = 4,
    YesSingle = 5,
    YesCopyMulti = 6,
    YesMulti = 7,
}

impl Acceptance {
    /// The enumerator's name.
    pub fn name(self) -> &'static str {
        match self {
            Acceptance::Postponed => "ACCEPT_POSTPONED",
            Acceptance::No => "ACCEPT_NO",
            Acceptance::NoCustom => "ACCEPT_NO_CUSTOM",
            Acceptance::NoLocked => "ACCEPT_NO_LOCKED",
            Acceptance::YesCopySingle => "ACCEPT_YES_COPY_SINGLE",
            Acceptance::YesSingle => "ACCEPT_YES_SINGLE",
            Acceptance::YesCopyMulti => "ACCEPT_YES_COPY_MULTI",
            Acceptance::YesMulti => "ACCEPT_YES_MULTI",
        }
    }
}

/// `clean_name_from_im`'s dialog list (`llimprocessing.cpp`): the dialogs
/// whose sender name goes through `LLCacheName::cleanFullName`. Task
/// inventory offers (9-11), `IM_FROM_TASK`, `IM_GOTO_URL`,
/// `IM_FROM_TASK_AS_ALERT` and the typing dialogs keep their name.
pub fn im_name_is_cleaned(dialog: i32) -> bool {
    matches!(
        dialog,
        IM_NOTHING_SPECIAL
            | IM_MESSAGEBOX
            | IM_GROUP_INVITATION
            | IM_INVENTORY_OFFERED
            | IM_INVENTORY_ACCEPTED
            | IM_INVENTORY_DECLINED
            | IM_GROUP_VOTE
            | IM_GROUP_MESSAGE_DEPRECATED
            | IM_NEW_USER_DEFAULT
            | IM_SESSION_INVITE
            | IM_SESSION_P2P_INVITE
            | IM_SESSION_GROUP_START
            | IM_SESSION_CONFERENCE_START
            | IM_SESSION_SEND
            | IM_SESSION_LEAVE
            | IM_DO_NOT_DISTURB_AUTO_RESPONSE
            | IM_CONSOLE_AND_CHAT_HISTORY
            | IM_LURE_USER
            | IM_LURE_ACCEPTED
            | IM_LURE_DECLINED
            | IM_GODLIKE_LURE_USER
            | IM_TELEPORT_REQUEST
            | IM_GROUP_ELECTION_DEPRECATED
            | IM_GROUP_NOTICE
            | IM_GROUP_NOTICE_INVENTORY_ACCEPTED
            | IM_GROUP_NOTICE_INVENTORY_DECLINED
            | IM_GROUP_INVITATION_ACCEPT
            | IM_GROUP_INVITATION_DECLINE
            | IM_GROUP_NOTICE_REQUESTED
            | IM_FRIENDSHIP_OFFERED
            | IM_FRIENDSHIP_ACCEPTED
            | IM_FRIENDSHIP_DECLINED_DEPRECATED
    )
}
