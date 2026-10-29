//! The AIS (Inventory API v3) decisions of `indra/newview/llaisapi.cpp`
//! (vkstorm-rust rung seven, sub-rung R3a).
//!
//! What moves to Rust, by C++ function:
//!
//! - [`update`]: `AISUpdate`'s parse (`parseMeta`, `parseContent`,
//!   `parseItem`, `parseLink`, `parseCategory`, `parseDescendentCount`,
//!   `parseEmbedded*`, `parseUUIDArray`), producing a [`ParsedUpdate`] and
//!   the ordered list of [`ApplyOp`]s that `doUpdate()` performs;
//! - [`result`]: the decisions of `AISAPI::InvokeAISCommandCoro` around the
//!   update: the HTTP status of a result, the error branch (410 and 403),
//!   the 403 alert flag as explicit state, and the completion callback ids;
//! - [`command`]: the requests the `AISAPI` command functions build
//!   (capability, method, URL, bodies, the coroutine's arguments).
//!
//! What stays C++: the coroutine plumbing (`LLCoros`, `LLCoprocedureManager`
//! and its "AIS" pool with `EnqueueAISCommand`/`onIdle`, the
//! `HttpCoroutineAdapter`), the timers and `checkTimeout()` with its yields,
//! and the applier: a C++ executor that walks the [`ApplyOp`] list in
//! order, as `doUpdate()` walked its maps, with `checkTimeout()` where the
//! list says. The inventory model and its objects stay C++ too; the parse
//! reads them through the [`Model`] trait, whose calls are the queries and
//! the parse-time model calls the recorded corpus lists, in its order.
//!
//! The fixture README (`tests/fixtures/ais/update/README.md`) is the
//! specification; `tests/r3a_*.rs` replay it.

pub mod command;
pub mod result;
pub mod update;

pub use command::{CapKind, Command, CommandType, Method, Refusal, Request};
pub use result::{
    begin, callback_ids, error_branch, process_result, AlertState, CommandContext, CommandResult, HttpStatus,
};
pub use update::{
    ApplyOp, CategoryRecord, FetchTimer, Flow, ItemRecord, Model, ModelCategory, ModelRef, ParseContext, ParsedUpdate,
    UpdateSummary,
};
