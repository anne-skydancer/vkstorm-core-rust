//! The command side of `AISAPI` (`newview/llaisapi.cpp:104-1044`): which
//! capability each command uses and what it does without one, the request
//! URL, method and depth, the `MAX_FOLDER_DEPTH_REQUEST` clamp, the
//! postponed queue of `EnqueueAISCommand` (2,048 pending coroutines), the
//! 403 notifications, and which ids a reply calls back with.
//!
//! The coroutine itself (`InvokeAISCommandCoro`), the HTTP adapter and the
//! update applier stay C++. A command's reply reaches the crate as an
//! [`AisReply`]: the HTTP status and the few top-level keys the callback
//! contract reads. R3a's parse (`ais/update`) has these keys at hand; until
//! it lands, the C++ coroutine reads them off the `LLSD` result.

use std::collections::VecDeque;

use super::{Uuid, NULL_ID, uuid_string};

/// `MAX_FOLDER_DEPTH_REQUEST`
pub const MAX_FOLDER_DEPTH_REQUEST: i32 = 50;
/// `MAX_SIMULTANEOUS_COROUTINES`
pub const MAX_SIMULTANEOUS_COROUTINES: usize = 2048;
/// `MAX_URL_LENGH` (only logged when exceeded).
pub const MAX_URL_LENGTH: usize = 2000;

/// `AISAPI::COMMAND_TYPE`, with the C++ values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum CommandType {
    CopyInventory = 0,
    SlamFolder = 1,
    RemoveCategory = 2,
    RemoveItem = 3,
    PurgeDescendents = 4,
    UpdateCategory = 5,
    UpdateItem = 6,
    CopyLibraryCategory = 7,
    CreateInventory = 8,
    FetchItem = 9,
    FetchCategoryChildren = 10,
    FetchCategoryCategories = 11,
    FetchCategorySubset = 12,
    FetchCof = 13,
    FetchOrphans = 14,
    FetchCategoryLinks = 15,
}

/// `AISAPI::ITEM_TYPE`: which capability a fetch uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemType {
    /// `INVENTORY`: `InventoryAPIv3`.
    Inventory,
    /// `LIBRARY`: `LibraryAPIv3`.
    Library,
}

/// The HTTP method the coroutine adapter uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Copy,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Copy => "COPY",
        }
    }
}

/// The two AIS capabilities of the agent's region (`getInvCap`, `getLibCap`):
/// empty when the region has none (or there is no region).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AisCaps {
    pub inventory: String,
    pub library: String,
}

impl AisCaps {
    fn get(&self, t: ItemType) -> &str {
        match t {
            ItemType::Inventory => &self.inventory,
            ItemType::Library => &self.library,
        }
    }
}

/// One `AISAPI` command with its arguments. Write commands carry the `tid`
/// the C++ generated (`LLUUID::generate`) where the URL has one; their bodies
/// are the caller's `LLSD` and are not built here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    CreateInventory { parent: Uuid, tid: Uuid },
    SlamFolder { folder: Uuid, tid: Uuid },
    RemoveCategory { category: Uuid },
    RemoveItem { item: Uuid },
    CopyLibraryCategory { source: Uuid, dest: Uuid, copy_subfolders: bool, tid: Uuid },
    PurgeDescendents { category: Uuid },
    UpdateCategory { category: Uuid },
    UpdateItem { item: Uuid },
    FetchItem { item: Uuid, item_type: ItemType },
    FetchCategoryChildren { category: Uuid, item_type: ItemType, recursive: bool, depth: i32 },
    /// The by-name form (`current`, `root`, ...): always `InventoryAPIv3`.
    FetchCategoryChildrenNamed { identifier: String, recursive: bool, depth: i32 },
    FetchCategoryCategories { category: Uuid, item_type: ItemType, recursive: bool, depth: i32 },
    FetchCategorySubset { category: Uuid, children: Vec<Uuid>, item_type: ItemType, recursive: bool, depth: i32 },
    FetchCof,
    FetchCategoryLinks { category: Uuid },
    FetchOrphans,
}

/// A request ready for the coprocedure pool: what `InvokeAISCommandCoro`
/// receives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AisRequest {
    pub command: CommandType,
    pub method: Method,
    pub url: String,
    /// `targetId`: what the 410 handling of a removal looks at.
    pub target: Uuid,
    /// The request body's `depth` (`AISUpdate`'s parse depth, the 403 rule);
    /// `None` when the command has none (writes, items, orphans).
    pub depth: Option<i32>,
    /// `COPY`'s `Destination` (the destination folder id as text).
    pub destination: Option<String>,
    /// The URL is longer than `MAX_URL_LENGH`: the C++ logs a warning and
    /// sends it all the same.
    pub url_too_long: bool,
}

/// Why a command was not sent: the C++ then calls back with the null id at
/// once (if there is a callback), inside the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotSent {
    /// "Inventory cap not found!" / "Library cap not found!"
    NoCapability,
    /// `FetchCategorySubset` with no children: "Empty request!"
    EmptySubset,
}

fn clamp_depth(recursive: bool, depth: i32) -> i32 {
    if recursive {
        // can specify depth=*, but server side is going to cap requests
        MAX_FOLDER_DEPTH_REQUEST
    } else {
        depth.min(MAX_FOLDER_DEPTH_REQUEST)
    }
}

/// Builds the request of `cmd` as the `AISAPI` function does, or says why it
/// is not sent.
pub fn build(cmd: &Command, caps: &AisCaps) -> Result<AisRequest, NotSent> {
    use CommandType as T;
    let req = |command, method, url: String, target, depth| AisRequest {
        command,
        method,
        url_too_long: false,
        url,
        target,
        depth,
        destination: None,
    };
    let cap = |t: ItemType| -> Result<&str, NotSent> {
        let c = caps.get(t);
        if c.is_empty() { Err(NotSent::NoCapability) } else { Ok(c) }
    };
    let inv = || cap(ItemType::Inventory);
    let id = uuid_string;
    Ok(match cmd {
        Command::CreateInventory { parent, tid } => req(
            T::CreateInventory,
            Method::Post,
            format!("{}/category/{}?tid={}", inv()?, id(parent), id(tid)),
            *parent,
            None,
        ),
        Command::SlamFolder { folder, tid } => req(
            T::SlamFolder,
            Method::Put,
            format!("{}/category/{}/links?tid={}", inv()?, id(folder), id(tid)),
            *folder,
            None,
        ),
        Command::RemoveCategory { category } => req(
            T::RemoveCategory,
            Method::Delete,
            format!("{}/category/{}", inv()?, id(category)),
            *category,
            None,
        ),
        Command::RemoveItem { item } => {
            req(T::RemoveItem, Method::Delete, format!("{}/item/{}", inv()?, id(item)), *item, None)
        }
        Command::CopyLibraryCategory { source, dest, copy_subfolders, tid } => {
            let mut url = format!("{}/category/{}?tid={}", cap(ItemType::Library)?, id(source), id(tid));
            if !copy_subfolders {
                url.push_str(",depth=0");
            }
            let mut r = req(T::CopyLibraryCategory, Method::Copy, url, *dest, None);
            r.destination = Some(id(dest));
            r
        }
        Command::PurgeDescendents { category } => req(
            T::PurgeDescendents,
            Method::Delete,
            format!("{}/category/{}/children", inv()?, id(category)),
            *category,
            None,
        ),
        Command::UpdateCategory { category } => req(
            T::UpdateCategory,
            Method::Patch,
            format!("{}/category/{}", inv()?, id(category)),
            *category,
            None,
        ),
        Command::UpdateItem { item } => {
            req(T::UpdateItem, Method::Patch, format!("{}/item/{}", inv()?, id(item)), *item, None)
        }
        Command::FetchItem { item, item_type } => req(
            T::FetchItem,
            Method::Get,
            format!("{}/item/{}", cap(*item_type)?, id(item)),
            *item,
            None,
        ),
        Command::FetchCategoryChildren { category, item_type, recursive, depth } => {
            let c = cap(*item_type)?;
            let depth = clamp_depth(*recursive, *depth);
            req(
                T::FetchCategoryChildren,
                Method::Get,
                format!("{}/category/{}/children?depth={}", c, id(category), depth),
                *category,
                Some(depth),
            )
        }
        Command::FetchCategoryChildrenNamed { identifier, recursive, depth } => {
            let c = inv()?;
            let depth = clamp_depth(*recursive, *depth);
            req(
                T::FetchCategoryChildren,
                Method::Get,
                format!("{}/category/{}/children?depth={}", c, identifier, depth),
                NULL_ID,
                Some(depth),
            )
        }
        Command::FetchCategoryCategories { category, item_type, recursive, depth } => {
            let c = cap(*item_type)?;
            let depth = clamp_depth(*recursive, *depth);
            req(
                T::FetchCategoryCategories,
                Method::Get,
                format!("{}/category/{}/categories?depth={}", c, id(category), depth),
                *category,
                Some(depth),
            )
        }
        Command::FetchCategorySubset { category, children, item_type, recursive, depth } => {
            let c = cap(*item_type)?;
            if children.is_empty() {
                return Err(NotSent::EmptySubset);
            }
            let depth = clamp_depth(*recursive, *depth);
            let mut url = format!("{}/category/{}/children?depth={}&children=", c, id(category), depth);
            for (i, k) in children.iter().enumerate() {
                if i > 0 {
                    url.push(',');
                }
                url.push_str(&id(k));
            }
            let mut r = req(T::FetchCategorySubset, Method::Get, url, *category, Some(depth));
            r.url_too_long = r.url.len() > MAX_URL_LENGTH;
            r
        }
        // Only the COF will be full, but it can hold an outfit link with the
        // outfit folder embedded for the parse.
        Command::FetchCof => {
            req(T::FetchCof, Method::Get, format!("{}/category/current/links", inv()?), NULL_ID, Some(0))
        }
        Command::FetchCategoryLinks { category } => req(
            T::FetchCategoryLinks,
            Method::Get,
            format!("{}/category/{}/links", inv()?, id(category)),
            NULL_ID,
            Some(0),
        ),
        Command::FetchOrphans => {
            req(T::FetchOrphans, Method::Get, format!("{}/orphans", inv()?), NULL_ID, None)
        }
    })
}

/// What the coroutine's reply contributes to the callback contract and the
/// 403 rule. `status` is the HTTP status of the final attempt (after the
/// transport's own retries); `0` stands for a transport failure without one.
/// The id fields are the top-level `category_id`, `item_id`, `linked_id`
/// (`asUUID`, so a malformed value reads as null) and the id arrays of
/// `_created_categories`, `_created_items`, `_updated_categories`; all
/// `None`/empty when the result is not a map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AisReply {
    pub status: i32,
    pub category_id: Option<Uuid>,
    pub item_id: Option<Uuid>,
    pub linked_id: Option<Uuid>,
    pub created_categories: Option<Vec<Uuid>>,
    pub created_items: Option<Vec<Uuid>>,
    pub updated_categories: Option<Vec<Uuid>>,
}

impl AisReply {
    /// The reply the coroutine makes up when `gDisconnected` is set before it
    /// sends anything: the callback gets null.
    pub fn disconnected() -> Self {
        AisReply::default()
    }
}

/// The ids `InvokeAISCommandCoro` calls a command's callback with, in order
/// (at least one: "Call callback at least once regardless of failure").
pub fn callback_ids(command: CommandType, reply: &AisReply) -> Vec<Uuid> {
    use CommandType as T;
    let mut id = NULL_ID;
    let mut out = Vec::new();
    match command {
        T::CopyLibraryCategory
        | T::FetchCategoryCategories
        | T::FetchCategoryChildren
        | T::FetchCategorySubset
        | T::FetchCategoryLinks
        | T::FetchCof => {
            if let Some(c) = reply.category_id {
                id = c;
            }
        }
        T::FetchItem => {
            // Error message might contain an item_id!!!
            if let Some(i) = reply.item_id {
                id = i;
            }
            if let Some(l) = reply.linked_id {
                id = l;
            }
        }
        T::CopyInventory | T::CreateInventory => {
            // CREATEINVENTORY can have multiple callbacks
            for ids in [&reply.created_categories, &reply.created_items].into_iter().flatten() {
                out.extend_from_slice(ids);
            }
            if !out.is_empty() {
                return out;
            }
        }
        // <FS:Zi> FIRE-34169
        T::UpdateCategory => {
            if let Some(u) = &reply.updated_categories {
                if !u.is_empty() {
                    return u.clone();
                }
                // an empty array: no callback from the loop, and the
                // `else if` is not reached; the fallback below calls with null
                return vec![NULL_ID];
            }
            if let Some(c) = reply.category_id {
                id = c;
            }
        }
        _ => {}
    }
    vec![id]
}

/// `InvokeAISCommandCoro`'s 403 rule for `FETCHCATEGORYCHILDREN` at depth 0:
/// the first in the process raises `InventoryLimitReachedAISAlert`, every
/// later one `InventoryLimitReachedAIS`. The function-static flag is explicit
/// state here ([`AisNotices`]).
#[derive(Clone, Debug)]
pub struct AisNotices {
    pub first_depth0_403: bool,
}

impl Default for AisNotices {
    fn default() -> Self {
        AisNotices { first_depth0_403: true }
    }
}

impl AisNotices {
    /// The notification a reply raises before the update is applied.
    pub fn on_reply(&mut self, request: &AisRequest, status: i32) -> Option<&'static str> {
        if status == 403 && request.command == CommandType::FetchCategoryChildren && request.depth == Some(0) {
            if self.first_depth0_403 {
                self.first_depth0_403 = false;
                Some("InventoryLimitReachedAISAlert")
            } else {
                Some("InventoryLimitReachedAIS")
            }
        } else {
            None
        }
    }
}

/// `EnqueueAISCommand` and `onIdle`: commands go to the coprocedure pool
/// while it holds fewer than 2,048 pending; the rest wait here in order.
/// `pending_in_pool` is `LLCoprocedureManager::countPending("AIS")`.
#[derive(Clone, Debug, Default)]
pub struct PostponedQueue<T> {
    queue: VecDeque<T>,
}

impl<T> PostponedQueue<T> {
    pub fn new() -> Self {
        PostponedQueue { queue: VecDeque::new() }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// `EnqueueAISCommand`: `Some(cmd)` to enqueue in the pool now, `None`
    /// when it was postponed. The first postponed command registers
    /// `onIdle` (the caller does when this returns `None` and [`len`] is 1).
    ///
    /// [`len`]: PostponedQueue::len
    pub fn enqueue(&mut self, cmd: T, pending_in_pool: usize) -> Option<T> {
        if pending_in_pool < MAX_SIMULTANEOUS_COROUTINES {
            Some(cmd)
        } else {
            self.queue.push_back(cmd);
            None
        }
    }

    /// `onIdle`: the postponed commands to enqueue now, in order. When the
    /// queue is empty afterwards the C++ removes the idle callback.
    pub fn on_idle(&mut self, mut pending_in_pool: usize) -> Vec<T> {
        let mut out = Vec::new();
        while pending_in_pool < MAX_SIMULTANEOUS_COROUTINES {
            match self.queue.pop_front() {
                Some(c) => {
                    out.push(c);
                    pending_in_pool += 1;
                }
                None => break,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps() -> AisCaps {
        AisCaps { inventory: "http://h/cap/inv".into(), library: "http://h/cap/lib".into() }
    }

    #[test]
    fn depth_clamp_and_subset_url() {
        let a = [0xc0u8; 16];
        let r = build(
            &Command::FetchCategoryChildren { category: a, item_type: ItemType::Inventory, recursive: false, depth: 99 },
            &caps(),
        )
        .unwrap();
        assert!(r.url.ends_with("/children?depth=50"));
        assert_eq!(r.depth, Some(50));
        let e = build(
            &Command::FetchCategorySubset {
                category: a,
                children: vec![],
                item_type: ItemType::Library,
                recursive: true,
                depth: 0,
            },
            &caps(),
        );
        assert_eq!(e, Err(NotSent::EmptySubset));
        let none = AisCaps { inventory: "x".into(), library: String::new() };
        let e = build(
            &Command::FetchCategorySubset {
                category: a,
                children: vec![],
                item_type: ItemType::Library,
                recursive: true,
                depth: 0,
            },
            &none,
        );
        // the capability is checked first
        assert_eq!(e, Err(NotSent::NoCapability));
    }

    #[test]
    fn postponed_queue_drains_in_order() {
        let mut q = PostponedQueue::new();
        assert_eq!(q.enqueue(1, 2047), Some(1));
        assert_eq!(q.enqueue(2, 2048), None);
        assert_eq!(q.enqueue(3, 2049), None);
        assert_eq!(q.on_idle(2048), Vec::<i32>::new());
        assert_eq!(q.on_idle(2047), vec![2]);
        assert_eq!(q.on_idle(0), vec![3]);
        assert!(q.is_empty());
    }

    #[test]
    fn callback_ids_per_command() {
        let c = [1u8; 16];
        let i = [2u8; 16];
        let l = [3u8; 16];
        let r = AisReply { status: 200, item_id: Some(i), linked_id: Some(l), category_id: Some(c), ..Default::default() };
        assert_eq!(callback_ids(CommandType::FetchItem, &r), vec![l]);
        assert_eq!(callback_ids(CommandType::FetchCof, &r), vec![c]);
        assert_eq!(callback_ids(CommandType::UpdateItem, &r), vec![NULL_ID]);
        let w = AisReply { created_categories: Some(vec![c]), created_items: Some(vec![i, l]), ..Default::default() };
        assert_eq!(callback_ids(CommandType::CreateInventory, &w), vec![c, i, l]);
        assert_eq!(callback_ids(CommandType::CreateInventory, &AisReply::default()), vec![NULL_ID]);
    }
}
