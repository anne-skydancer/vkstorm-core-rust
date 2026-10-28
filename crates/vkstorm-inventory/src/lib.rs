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
//!   UDP message blocks ([`message`]).
//!
//! It is not the inventory model. In this rung the model containers stay C++:
//! `LLInventoryItem`, `LLInventoryCategory` and `LLInventoryObject` keep their
//! field storage, which the viewer's subclasses touch directly, and the model,
//! cache, fetch and messaging code keeps its containers. The crate works on
//! flat records and views of those objects.
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
pub mod category;
pub mod category_obj;
pub mod flags;
pub mod item;
pub mod legacy;
pub mod llsd;
pub mod message;
pub mod object;
pub mod perms;
pub mod sale;
pub mod shadow;
pub mod text;
pub mod types;
