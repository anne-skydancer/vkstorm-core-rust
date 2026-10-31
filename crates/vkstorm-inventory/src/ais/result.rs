//! The decisions of `AISAPI::InvokeAISCommandCoro` around the update
//! (`llaisapi.cpp:856-1044`): the status of a result, the error branch, the
//! update, and the completion callback ids.
//!
//! The coroutine itself stays C++: the `gDisconnected` check (which calls
//! the callback once with the null id and does nothing else), the request
//! through the adapter, `doUpdate()` (the executor of
//! [`ApplyOp`](super::ApplyOp)s) and firing the callbacks after it. A
//! shutdown during the update ends the coroutine before any callback.
//!
//! The 403 notification is chosen by a function-static flag in the C++ (the
//! first 403 at depth 0 in the process gets the alert); here it is the
//! caller's [`AlertState`].

use vkstorm_llsd::Value;

use super::command::CommandType;
use super::update::{Model, ParseContext, ParsedUpdate};
use crate::llsd::coerce::{as_integer, as_uuid, get, has};
use crate::llsd::item::entry;
use crate::perms::{NULL_ID, Uuid};

/// `HttpCoroutineAdapter::HTTP_RESULTS`: the key of the adapter's status map.
pub const HTTP_RESULTS: &str = "http_result";
/// The notification of the first 403 at depth 0.
pub const ALERT_FIRST: &str = "InventoryLimitReachedAISAlert";
/// The notification of every later 403 at depth 0.
pub const ALERT_LATER: &str = "InventoryLimitReachedAIS";
/// `HTTP_FORBIDDEN`.
pub const HTTP_FORBIDDEN: u16 = 403;
/// `HTTP_GONE`, the "item does not exist or was already deleted" status.
pub const HTTP_GONE: u16 = 410;
/// `HTTP_INTERNAL_ERROR`.
pub const HTTP_INTERNAL_ERROR: u16 = 500;

/// `LLCore::HttpStatus`: a type (an HTTP status, or a library's error
/// domain) and a status (0 for success).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HttpStatus {
    /// `mType`.
    pub kind: u16,
    /// `mStatus`: `HE_SUCCESS` (0) or an error code.
    pub status: i16,
}

impl HttpStatus {
    /// `getStatusFromLLSD`: `type` and `status` of the adapter's status map
    /// by `asInteger`, cut to `unsigned short` and `short`.
    pub fn from_results(results: &Value) -> HttpStatus {
        HttpStatus {
            kind: as_integer(get(results, "type")) as u16,
            status: as_integer(get(results, "status")) as i16,
        }
    }

    /// `HttpStatus(int http_status)`: success for 200-299, else
    /// `HE_REPLY_ERROR` (1).
    pub fn http(code: u16) -> HttpStatus {
        HttpStatus {
            kind: code,
            status: if (200..=299).contains(&code) { 0 } else { 1 },
        }
    }

    /// `operator bool`: the status is 0.
    pub fn is_ok(self) -> bool {
        self.status == 0
    }
}

/// The 403 alert flag (`static bool first_call` of the coroutine): true
/// until the first 403 at depth 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlertState {
    pub first_call: bool,
}

impl Default for AlertState {
    fn default() -> Self {
        AlertState { first_call: true }
    }
}

/// The coroutine's arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandContext {
    pub command: CommandType,
    /// `targetId`.
    pub target_id: Uuid,
    /// `body`, also `AISUpdate`'s `request_body`.
    pub body: Value,
    /// `gAgent.getID()`.
    pub agent_id: Uuid,
    /// `LLPermissions::getIsInOpenSim()`.
    pub in_open_sim: bool,
}

/// What the coroutine decided for one result.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandResult {
    /// The result as the coroutine holds it: its `result[HTTP_RESULTS]` on
    /// the non-const value turned a result that is not a map into
    /// `{http_result: undefined}` and adds an undefined `http_result` to a
    /// map without one.
    pub result: Value,
    pub status: HttpStatus,
    /// The parse; apply its [`ApplyOp`](super::ApplyOp)s unless
    /// [`stopped`](ParsedUpdate::stopped).
    pub update: ParsedUpdate,
    /// The completion callback ids, in order, for a coroutine with a
    /// callback (never empty).
    pub callbacks: Vec<Uuid>,
}

/// The coroutine after the request, in its order: [`begin`],
/// [`error_branch`], the parse ([`ParsedUpdate::parse`], which calls
/// [`Model::begin_update`] where `onUpdateReceived` constructs the
/// `AISUpdate`), and [`callback_ids`]. The caller applies the update's
/// [`ApplyOp`](super::ApplyOp)s (unless stopped) and then fires the callbacks.
pub fn process_result<M: Model + ?Sized>(
    ctx: &CommandContext,
    mut result: Value,
    alert: &mut AlertState,
    model: &mut M,
) -> CommandResult {
    let status = begin(&mut result);
    let status = error_branch(ctx, &result, status, alert, model);
    let parse_ctx = ParseContext {
        command: ctx.command,
        agent_id: ctx.agent_id,
        in_open_sim: ctx.in_open_sim,
    };
    let update = ParsedUpdate::parse(&result, &ctx.body, &parse_ctx, model);
    let callbacks = callback_ids(&result, ctx.command);
    CommandResult {
        result,
        status,
        update,
        callbacks,
    }
}

/// `httpResults = result[HTTP_RESULTS]` on the coroutine's non-const
/// result, and `getStatusFromLLSD(httpResults)`. The `operator[]` makes a
/// result that is not a map an empty map, and adds the key (undefined) to a
/// map without it; so every result is a map afterwards.
pub fn begin(result: &mut Value) -> HttpStatus {
    let http_results = entry(result, HTTP_RESULTS).clone();
    HttpStatus::from_results(&http_results)
}

/// The error branch, for a status that is not success (its model calls and
/// notification go to `model` at once). Returns the status, which a result
/// that is not a map would turn into 500 (unreachable after [`begin`]).
///
/// - 410 on `REMOVECATEGORY`: when the folder is known, fetch its parent's
///   descendents (the folder is kept: the model is out of sync).
/// - 410 on `REMOVEITEM`: when the item is known, fetch its parent's
///   descendents and delete it (`onObjectDeletedFromServer(id, true, true,
///   true)`, which bumps the parent's version; finding 5).
/// - 403 on `FETCHCATEGORYCHILDREN` with a body `depth` of 0 (a folder too
///   big for one fetch): the alert the first time ([`AlertState`]), the plain
///   notification after.
///
/// Every result is parsed afterwards, error bodies included (finding 6).
pub fn error_branch<M: Model + ?Sized>(
    ctx: &CommandContext,
    result: &Value,
    mut status: HttpStatus,
    alert: &mut AlertState,
    model: &mut M,
) -> HttpStatus {
    let is_map = matches!(result, Value::Map(_));
    if status.is_ok() && is_map {
        return status;
    }
    if !is_map {
        status = HttpStatus::http(HTTP_INTERNAL_ERROR);
    } else if status.kind == HTTP_GONE {
        match ctx.command {
            CommandType::RemoveCategory => {
                if let Some(cat) = model.get_category(&ctx.target_id) {
                    model.fetch_descendents_of(&cat.record.category.object.parent);
                }
            }
            CommandType::RemoveItem => {
                if let Some(item) = model.get_item(&ctx.target_id) {
                    model.fetch_descendents_of(&item.item.object.parent);
                    model.on_object_deleted_from_server(&ctx.target_id, true, true, true);
                }
            }
            _ => {}
        }
    } else if status == HttpStatus::http(HTTP_FORBIDDEN)
        && ctx.command == CommandType::FetchCategoryChildren
        && has(&ctx.body, "depth")
        && as_integer(get(&ctx.body, "depth")) == 0
    {
        if alert.first_call {
            alert.first_call = false;
            model.notify(ALERT_FIRST);
        } else {
            model.notify(ALERT_LATER);
        }
    }
    status
}

/// The ids of each element of an array (`asUUID`); none for another value.
fn array_ids(v: &Value, out: &mut Vec<Uuid>) -> bool {
    match v {
        Value::Array(a) if !a.is_empty() => {
            out.extend(a.iter().map(as_uuid));
            true
        }
        _ => false,
    }
}

/// The completion callback contract: the ids the coroutine calls its
/// callback with, in order.
///
/// - The folder fetches and `COPYLIBRARYCATEGORY`: `category_id`, else null.
/// - `FETCHITEM`: `linked_id`, else `item_id` (an error body may hold one),
///   else null.
/// - `CREATEINVENTORY` and `COPYINVENTORY`: every id of
///   `_created_categories`, then of `_created_items`; null when both are
///   absent or empty.
/// - `UPDATECATEGORY` (FIRE-34169): every id of `_updated_categories` when
///   the key is there, else `category_id`, else null.
/// - The others: null.
pub fn callback_ids(result: &Value, command: CommandType) -> Vec<Uuid> {
    let mut ids = Vec::new();
    let mut id = NULL_ID;
    let mut needs_callback = true;
    match command {
        CommandType::CopyLibraryCategory
        | CommandType::FetchCategoryCategories
        | CommandType::FetchCategoryChildren
        | CommandType::FetchCategorySubset
        | CommandType::FetchCategoryLinks
        | CommandType::FetchCof => {
            if has(result, "category_id") {
                id = as_uuid(get(result, "category_id"));
            }
        }
        CommandType::FetchItem => {
            if has(result, "item_id") {
                id = as_uuid(get(result, "item_id"));
            }
            if has(result, "linked_id") {
                id = as_uuid(get(result, "linked_id"));
            }
        }
        CommandType::CopyInventory | CommandType::CreateInventory => {
            if has(result, "_created_categories")
                && array_ids(get(result, "_created_categories"), &mut ids)
            {
                needs_callback = false;
            }
            if has(result, "_created_items") && array_ids(get(result, "_created_items"), &mut ids) {
                needs_callback = false;
            }
        }
        CommandType::UpdateCategory => {
            if has(result, "_updated_categories") {
                if array_ids(get(result, "_updated_categories"), &mut ids) {
                    needs_callback = false;
                }
            } else if has(result, "category_id") {
                id = as_uuid(get(result, "category_id"));
            }
        }
        _ => {}
    }
    if needs_callback {
        ids.push(id);
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ais::update::{ItemRecord, ModelCategory, ModelRef};
    use crate::llsd::coerce::insert;

    /// A model that knows one item and logs every call.
    #[derive(Default)]
    struct Log {
        calls: Vec<String>,
        item: Option<ItemRecord>,
    }

    impl Model for Log {
        fn get_category(&mut self, _: &Uuid) -> Option<ModelCategory> {
            self.calls.push("getCategory".into());
            None
        }
        fn get_item(&mut self, _: &Uuid) -> Option<ItemRecord> {
            self.calls.push("getItem".into());
            self.item.clone()
        }
        fn get_version(&mut self, _: ModelRef) -> i32 {
            unreachable!()
        }
        fn set_version(&mut self, _: ModelRef, _: i32) {
            unreachable!()
        }
        fn fetch(&mut self, _: ModelRef, _: i32) -> bool {
            unreachable!()
        }
        fn fetch_descendents_of(&mut self, id: &Uuid) -> bool {
            self.calls.push(format!("fetchDescendentsOf {}", id[0]));
            false
        }
        fn on_object_deleted_from_server(&mut self, id: &Uuid, a: bool, b: bool, c: bool) {
            self.calls
                .push(format!("onObjectDeletedFromServer {} {a} {b} {c}", id[0]));
        }
        fn notify(&mut self, name: &'static str) {
            self.calls.push(name.into());
        }
        fn begin_update(&mut self) {
            self.calls.push("AISUpdate".into());
        }
    }

    fn status_map(kind: i32, status: i32) -> Value {
        let mut h = Value::map();
        insert(&mut h, "type", Value::Integer(kind));
        insert(&mut h, "status", Value::Integer(status));
        let mut r = Value::map();
        insert(&mut r, HTTP_RESULTS, h);
        r
    }

    fn ctx(command: CommandType, body: Value) -> CommandContext {
        CommandContext {
            command,
            target_id: [0xa0; 16],
            body,
            agent_id: [0x0a; 16],
            in_open_sim: false,
        }
    }

    #[test]
    fn gone_item_is_refetched_and_deleted() {
        let mut item = ItemRecord::default();
        item.item.object.parent = [0xc0; 16];
        let mut m = Log {
            item: Some(item),
            ..Log::default()
        };
        let c = ctx(CommandType::RemoveItem, Value::Undefined);
        let r = process_result(&c, status_map(410, 1), &mut AlertState::default(), &mut m);
        let want = [
            "getItem",
            "fetchDescendentsOf 192",
            "onObjectDeletedFromServer 160 true true true",
            "AISUpdate",
        ];
        assert_eq!(m.calls, want);
        assert_eq!(
            r.status,
            HttpStatus {
                kind: 410,
                status: 1
            }
        );
        assert_eq!(r.callbacks, [NULL_ID]);
        assert!(r.update.apply_ops().len() == 3 && !r.update.stopped);
    }

    #[test]
    fn forbidden_at_depth_zero_alerts_once() {
        let mut body = Value::map();
        insert(&mut body, "depth", Value::Integer(0));
        let mut alert = AlertState::default();
        let mut m = Log::default();
        for _ in 0..2 {
            let c = ctx(CommandType::FetchCategoryChildren, body.clone());
            process_result(&c, status_map(403, 1), &mut alert, &mut m);
        }
        assert_eq!(
            m.calls,
            [ALERT_FIRST, "AISUpdate", ALERT_LATER, "AISUpdate"]
        );
        // Depth 1 is recoverable: no notification.
        insert(&mut body, "depth", Value::Integer(1));
        let mut m = Log::default();
        process_result(
            &ctx(CommandType::FetchCategoryChildren, body),
            status_map(403, 1),
            &mut alert,
            &mut m,
        );
        assert_eq!(m.calls, ["AISUpdate"]);
    }

    #[test]
    fn a_result_that_is_not_a_map_becomes_one() {
        let mut m = Log::default();
        let c = ctx(CommandType::FetchOrphans, Value::Undefined);
        let r = process_result(&c, Value::Integer(7), &mut AlertState::default(), &mut m);
        assert_eq!(r.status, HttpStatus { kind: 0, status: 0 });
        let mut want = Value::map();
        insert(&mut want, HTTP_RESULTS, Value::Undefined);
        assert_eq!(r.result, want);
    }

    #[test]
    fn callbacks() {
        let ids = |v: Vec<Value>| Value::Array(v);
        let mut r = Value::map();
        insert(
            &mut r,
            "_created_categories",
            ids(vec![Value::Uuid([1; 16])]),
        );
        insert(
            &mut r,
            "_created_items",
            ids(vec![Value::Uuid([2; 16]), Value::Uuid([3; 16])]),
        );
        insert(&mut r, "category_id", Value::Uuid([4; 16]));
        assert_eq!(
            callback_ids(&r, CommandType::CreateInventory),
            [[1; 16], [2; 16], [3; 16]]
        );
        assert_eq!(callback_ids(&r, CommandType::FetchCof), [[4; 16]]);
        assert_eq!(callback_ids(&r, CommandType::UpdateItem), [NULL_ID]);
        insert(&mut r, "_updated_categories", ids(vec![]));
        assert_eq!(callback_ids(&r, CommandType::UpdateCategory), [NULL_ID]);
        insert(&mut r, "_created_categories", ids(vec![]));
        insert(&mut r, "_created_items", Value::map());
        assert_eq!(callback_ids(&r, CommandType::CreateInventory), [NULL_ID]);
    }
}
