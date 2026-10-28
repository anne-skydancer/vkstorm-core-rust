//! A stand-in, in the test, for the AIS command layer of sub-rung R3a
//! (`ais::command::Command::build`, `ais::result::callback_ids`, the 403
//! alert of `error_branch`), which this branch does not contain: it turns
//! the scheduler's [`AisFetch`] decisions and the scenarios' direct
//! `AISAPI` calls into requests and reads the replies' callback ids, as
//! `llaisapi.cpp:104-1044` does. It is scaffolding: on integration the
//! executor calls R3a's code instead. Because it is a pure function of the
//! command, a request matching the recording shows that the scheduler chose
//! the right command, target, children, capability and depth.


use vkstorm_inventory::fetch::ais::AisFetch;
use vkstorm_inventory::fetch::{NULL_ID, Uuid, uuid_string};

/// `MAX_FOLDER_DEPTH_REQUEST`
pub const MAX_FOLDER_DEPTH_REQUEST: i32 = 50;
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

/// The scheduler's decision as the command function call it stands for.
pub fn from_fetch(f: &AisFetch) -> Command {
    let t = |library: bool| if library { ItemType::Library } else { ItemType::Inventory };
    match f.clone() {
        AisFetch::FetchItem { item_id, library } => Command::FetchItem { item: item_id, item_type: t(library) },
        AisFetch::FetchCategoryChildren { category_id, library, recursive, depth } => {
            Command::FetchCategoryChildren { category: category_id, item_type: t(library), recursive, depth }
        }
        AisFetch::FetchCategorySubset { category_id, children, library, recursive, depth } => {
            Command::FetchCategorySubset { category: category_id, children, item_type: t(library), recursive, depth }
        }
        AisFetch::FetchCof => Command::FetchCof,
        AisFetch::FetchCategoryLinks { category_id } => Command::FetchCategoryLinks { category: category_id },
        AisFetch::FetchOrphans => Command::FetchOrphans,
    }
}
