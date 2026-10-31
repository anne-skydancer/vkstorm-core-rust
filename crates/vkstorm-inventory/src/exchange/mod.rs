//! Inventory offers and giving (rung seven, sub-rung R4b): the decision code
//! of `LLOfferInfo`, `LLIMProcessing::processNewMessage`'s offer and group
//! notice cases, `inventory_offer_handler`, the offer throttle, the
//! ReadOfflineMsgs loop, `LLGiveInventory`, the give and drop predicates,
//! `willObjectAcceptInventory`, `move_inv_category_world_to_agent`, the share
//! dialog's callback and the drag-and-drop give handlers
//! (`llviewermessage.cpp`, `llimprocessing.cpp`, `llgiveinventory.cpp`,
//! `llinventoryfunctions.cpp`, `lltooldraganddrop.cpp`,
//! `llinventorybridge.cpp`, `llavataractions.cpp`).
//!
//! # The seam
//!
//! The viewer reaches this code through `r4b_boundary::Decisions`
//! (`indra/newview/tests/llinventory_offer_oracle_boundary.h`); every method
//! there has a function here ([`offer`] and [`give`]). A decision reads the
//! viewer's state through [`Host`] queries and tells the viewer what to do as
//! an ordered list of [`Action`]s, handed to [`Host::act`] one at a time in
//! the order the C++ does them. The executor (C++ in the viewer, the fakes'
//! counterpart in this crate's tests) carries each one out: messages,
//! notifications, observers, model and mute list changes, RLVa notices, offer
//! state.
//!
//! Some queries are effects too, because the viewer logs or changes something
//! when they run, and the order of those effects is part of the behaviour:
//! [`Host::find_category_for_type`], [`Host::generate_uuid`] and the observer
//! calls ([`Host::observer_new`], [`Host::observer_start`]). They are host
//! methods with a result rather than actions.
//!
//! State changes that a later decision in the same call depends on are made by
//! the executor as the action runs (a mute removed by `autoRemove`, an item
//! deleted after a no-copy give), so the queries after it see them.
//!
//! # What stays C++
//!
//! The mute list (`LLMuteList::isMuted`, `add`, `remove`, `autoRemove`), the
//! name caches and their callbacks, the notification system (forms, posting,
//! postponed name lookups, toasts), the inventory model and its fetch
//! observers (`LLInventoryFetchItemsObserver`, `LLOpenAgentOffer`,
//! `LLDiscardAgentOffer`, the RLVa give-to-#RLV observers), the message
//! system, `LLTrans`, `LLSLURL` parsing, the IM floaters and logs, the HUD
//! effect, the antispam queue and the Omnifilter (filters in front of this
//! area). This module decides; the executor does.

pub mod give;
pub mod host;
pub mod im;
pub mod info;
pub mod names;
pub mod notify;
pub mod offer;
pub mod throttle;
pub mod xxh3;

pub use host::{
    Action, Handler, Host, ImPacket, LogLevel, NameThen, ObserverKind, PackArgs, Route, log,
};
pub use im::*;
pub use info::{NetHost, OfferInfo};
pub use throttle::Throttle;
