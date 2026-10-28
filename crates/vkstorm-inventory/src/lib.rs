//! The inventory data model's rules for Vulkanstorm (vkstorm-rust rung seven).
//!
//! This crate holds the parts of the viewer's `indra/llinventory` library that
//! are pure data and decision making, behind C++ classes that keep their names
//! and interfaces in the viewer:
//!
//! - the asset, inventory, folder and sale type tables ([`types`]);
//! - item flags and transaction flags ([`flags`]), name and description
//!   normalisation ([`text`]) and `LLCategory` ([`category_obj`]);
//! - `LLPermissions`, `LLAggregatePermissions` and `LLSaleInfo` rules
//!   ([`perms`], [`aggregate`], [`sale`]);
//! - item, category and object records, their CRCs and the asset id
//!   obfuscation ([`item`], [`category`], [`object`], [`shadow`]);
//! - their LLSD forms ([`llsd`]) and legacy text forms ([`legacy`]);
//! - items, folders, aggregate permissions, `LLCategory` and sale info in
//!   UDP message blocks ([`message`]);
//! - from `LLInventoryModel` (`newview`): the cache file codec ([`cache`]),
//!   the login load and discard rules of `loadSkeleton` ([`login`]) and
//!   `validate()` ([`validate`]), over flat views of the C++ model
//!   ([`model`]).
//! - the AIS decisions of `llaisapi.cpp` ([`ais`]): the parse of an AIS reply
//!   into the ordered operations `AISUpdate::doUpdate()` applies, the
//!   coroutine's status, error-branch and callback decisions, and the
//!   requests of the command functions.
//! - the background fetch's scheduler and request building
//!   (`LLInventoryModelBackgroundFetch`, the fetch side of `AISAPI`), driven
//!   by explicit events and producing an ordered action list ([`fetch`]).
//! - the decisions of object contents ("task inventory": the task file
//!   loader, the capability reply, the serial and request-state machine and
//!   the messages it sends, [`task`]).
//! - the offer and give decisions (sub-rung R4b, [`exchange`]): received
//!   offers, their responses and LLSD form, the offer throttle and policy,
//!   offline offers; item and folder gives, the give and drop predicates,
//!   drop acceptance, the share dialog. They read the viewer through a
//!   [`exchange::Host`] and tell it what to do as ordered
//!   [`exchange::Action`]s.
//!
//! It is not the inventory model. In this rung the model containers stay C++:
//! `LLInventoryItem`, `LLInventoryCategory` and `LLInventoryObject` keep their
//! field storage, which the viewer's subclasses touch directly, and the model,
//! cache, fetch and messaging code keeps its containers. The crate works on
//! flat records and views of those objects.
//!
//! The agent-inventory UDP message handlers of `LLInventoryModel`
//! (`UpdateCreateInventoryItem`, `BulkUpdateInventory` in its UDP and
//! GenericMessage forms, the remove messages, `SaveAssetIntoInventory`,
//! `MoveInventoryItem`, `InventoryDescendents`, `FetchInventoryReply`) stay
//! C++ as well: block by block they interleave model lookups with model
//! calls (`accountForUpdate`, `updateItem`, `updateCategory`,
//! `notifyObservers`, the callbacks, the fetch scheduler, the marketplace
//! and RLVa hooks), so they are glue around the model. Their block decoding
//! is [`message`].
//!
//! Type values are raw integer newtypes, never closed enums: the viewer
//! carries any value it receives, including ones it has no name for, and so
//! does this crate. UUID text is parsed with `vkstorm_llsd::text::uuid_set`
//! (`LLUUID::set` semantics).
//!
//! The crate has no `unsafe` code; the C ABI lives in the viewer's
//! `vkstorm-ffi` crate. The `opensim` feature enables the OpenSim-only paths.

#![forbid(unsafe_code)]

pub mod aggregate;
pub mod ais;
pub mod cache;
pub mod category;
pub mod category_obj;
pub mod exchange;
pub mod fetch;
pub mod flags;
pub mod item;
pub mod legacy;
pub mod llsd;
pub mod login;
pub mod message;
pub mod model;
pub mod object;
pub mod perms;
pub mod sale;
pub mod shadow;
pub mod task;
pub mod text;
pub mod types;
pub mod validate;
