//! The AIS command types and the requests the `AISAPI` command functions
//! build (`llaisapi.cpp:64-788`).
//!
//! Each command function looks up a capability URL (`getInvCap` or
//! `getLibCap`, empty without a region), refuses with a null completion
//! callback when it is empty, else builds the URL and binds the coroutine
//! `InvokeAISCommandCoro` with a method, a target id, a body and a command
//! type, and enqueues it as `AIS(<name>)`. [`Command::build`] is that
//! decision; the lookup, the pool (`EnqueueAISCommand`, `onIdle`, the 2048
//! limit), the transport and the coroutine stay C++. A write's transaction
//! id (`LLUUID::generate()`) is the caller's input.

use vkstorm_llsd::Value;
use vkstorm_llsd::text::uuid_to_hex;

use crate::llsd::coerce::insert;
use crate::perms::{NULL_ID, Uuid};

/// `AISAPI::INVENTORY_CAP_NAME`.
pub const INVENTORY_CAP_NAME: &str = "InventoryAPIv3";
/// `AISAPI::LIBRARY_CAP_NAME`.
pub const LIBRARY_CAP_NAME: &str = "LibraryAPIv3";
/// `AISAPI::HTTP_TIMEOUT`: the request timeout the coroutine sets, seconds.
pub const HTTP_TIMEOUT: i32 = 180;
/// `MAX_SIMULTANEOUS_COROUTINES`: `EnqueueAISCommand` enqueues while the
/// pool holds fewer pending commands, and postpones otherwise.
pub const MAX_SIMULTANEOUS_COROUTINES: i32 = 2048;
/// `MAX_FOLDER_DEPTH_REQUEST`: the depth of a recursive fetch, the cap of a
/// non-recursive one, and `AISUpdate`'s fetch depth when the request body
/// names none.
pub const MAX_FOLDER_DEPTH_REQUEST: i32 = 50;
/// `MAX_URL_LENGH` of `FetchCategorySubset`: a longer URL is only logged.
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

impl CommandType {
    /// Every command type, in value order.
    pub const ALL: [CommandType; 16] = [
        CommandType::CopyInventory,
        CommandType::SlamFolder,
        CommandType::RemoveCategory,
        CommandType::RemoveItem,
        CommandType::PurgeDescendents,
        CommandType::UpdateCategory,
        CommandType::UpdateItem,
        CommandType::CopyLibraryCategory,
        CommandType::CreateInventory,
        CommandType::FetchItem,
        CommandType::FetchCategoryChildren,
        CommandType::FetchCategoryCategories,
        CommandType::FetchCategorySubset,
        CommandType::FetchCof,
        CommandType::FetchOrphans,
        CommandType::FetchCategoryLinks,
    ];

    /// The type of a C++ value, `None` outside the enumeration.
    pub fn from_i32(v: i32) -> Option<CommandType> {
        usize::try_from(v)
            .ok()
            .and_then(|i| Self::ALL.get(i).copied())
    }

    /// The enumerator's name (`COPYINVENTORY` ... `FETCHCATEGORYLINKS`).
    pub fn name(self) -> &'static str {
        match self {
            CommandType::CopyInventory => "COPYINVENTORY",
            CommandType::SlamFolder => "SLAMFOLDER",
            CommandType::RemoveCategory => "REMOVECATEGORY",
            CommandType::RemoveItem => "REMOVEITEM",
            CommandType::PurgeDescendents => "PURGEDESCENDENTS",
            CommandType::UpdateCategory => "UPDATECATEGORY",
            CommandType::UpdateItem => "UPDATEITEM",
            CommandType::CopyLibraryCategory => "COPYLIBRARYCATEGORY",
            CommandType::CreateInventory => "CREATEINVENTORY",
            CommandType::FetchItem => "FETCHITEM",
            CommandType::FetchCategoryChildren => "FETCHCATEGORYCHILDREN",
            CommandType::FetchCategoryCategories => "FETCHCATEGORYCATEGORIES",
            CommandType::FetchCategorySubset => "FETCHCATEGORYSUBSET",
            CommandType::FetchCof => "FETCHCOF",
            CommandType::FetchOrphans => "FETCHORPHANS",
            CommandType::FetchCategoryLinks => "FETCHCATEGORYLINKS",
        }
    }

    /// The type named by [`name`](Self::name).
    pub fn from_name(name: &str) -> Option<CommandType> {
        Self::ALL.into_iter().find(|t| t.name() == name)
    }

    /// `AISUpdate::mFetch`: the fetch commands, whose replies create what
    /// they carry rather than update what the model holds.
    pub fn is_fetch(self) -> bool {
        matches!(
            self,
            CommandType::FetchItem
                | CommandType::FetchCategoryChildren
                | CommandType::FetchCategoryCategories
                | CommandType::FetchCategorySubset
                | CommandType::FetchCof
                | CommandType::FetchCategoryLinks
                | CommandType::FetchOrphans
        )
    }
}

/// The adapter method a command binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Post,
    Put,
    Patch,
    Get,
    Delete,
    Copy,
}

impl Method {
    /// The HTTP method's name.
    pub fn name(self) -> &'static str {
        match self {
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Get => "GET",
            Method::Delete => "DELETE",
            Method::Copy => "COPY",
        }
    }
}

/// Which capability a command uses (`ITEM_TYPE`: `INVENTORY` or `LIBRARY`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CapKind {
    /// `getInvCap()`: [`INVENTORY_CAP_NAME`].
    Inventory,
    /// `getLibCap()`: [`LIBRARY_CAP_NAME`].
    Library,
}

impl CapKind {
    /// The capability's name.
    pub fn cap_name(self) -> &'static str {
        match self {
            CapKind::Inventory => INVENTORY_CAP_NAME,
            CapKind::Library => LIBRARY_CAP_NAME,
        }
    }

    /// `type == INVENTORY ? getInvCap() : getLibCap()`.
    fn of(library: bool) -> CapKind {
        if library {
            CapKind::Library
        } else {
            CapKind::Inventory
        }
    }
}

/// One call of an `AISAPI` command function, with its arguments. `library`
/// is `ITEM_TYPE` `LIBRARY`.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    CreateInventory {
        parent_id: Uuid,
        new_inventory: Value,
    },
    SlamFolder {
        folder_id: Uuid,
        new_inventory: Value,
    },
    RemoveCategory {
        category_id: Uuid,
    },
    RemoveItem {
        item_id: Uuid,
    },
    PurgeDescendents {
        category_id: Uuid,
    },
    UpdateCategory {
        category_id: Uuid,
        updates: Value,
    },
    UpdateItem {
        item_id: Uuid,
        updates: Value,
    },
    FetchItem {
        item_id: Uuid,
        library: bool,
    },
    FetchCategoryChildren {
        category_id: Uuid,
        library: bool,
        recursive: bool,
        depth: i32,
    },
    /// The `FetchCategoryChildren(const std::string& identifier, ...)`
    /// overload (folders by name: `current`, `root`, `lstndfnd`, ...).
    FetchCategoryChildrenByName {
        identifier: String,
        recursive: bool,
        depth: i32,
    },
    FetchCategoryCategories {
        category_id: Uuid,
        library: bool,
        recursive: bool,
        depth: i32,
    },
    FetchCategorySubset {
        category_id: Uuid,
        children: Vec<Uuid>,
        library: bool,
        recursive: bool,
        depth: i32,
    },
    FetchCof,
    FetchCategoryLinks {
        category_id: Uuid,
    },
    FetchOrphans,
    CopyLibraryCategory {
        source_id: Uuid,
        dest_id: Uuid,
        copy_subfolders: bool,
    },
}

/// Why a command function sends nothing: it calls the completion callback
/// once with the null id and returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The capability URL is empty ("Inventory cap not found!").
    NoCapability,
    /// `FetchCategorySubset` with no children ("Empty request!").
    EmptyRequest,
}

/// What a command function enqueues: the coroutine's arguments and the
/// request the bound adapter method sends.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// The procedure name; the pool sees `AIS(<proc_name>)`.
    pub proc_name: &'static str,
    pub method: Method,
    pub url: String,
    /// What a POST, PUT or PATCH sends; `None` for the other methods.
    pub http_body: Option<Value>,
    /// The `Destination` of a COPY.
    pub destination: Option<String>,
    /// The coroutine's `targetId`.
    pub target_id: Uuid,
    /// The coroutine's `body` (and `AISUpdate`'s `request_body`): the write's
    /// LLSD, `{depth: N}` for the folder fetches, undefined otherwise.
    pub body: Value,
    pub command: CommandType,
    /// `FetchCategorySubset`'s URL is over [`MAX_URL_LENGTH`] (logged only).
    pub url_too_long: bool,
}

/// `uuid.asString()`.
fn id_text(id: &Uuid) -> String {
    let mut out = Vec::with_capacity(36);
    uuid_to_hex(id, &mut out);
    String::from_utf8(out).expect("hex is ASCII")
}

/// `{depth: N}`, the body a folder fetch passes to the coroutine.
fn depth_body(depth: i32) -> Value {
    let mut body = Value::Undefined;
    insert(&mut body, "depth", Value::Integer(depth));
    body
}

/// The depth a folder fetch requests: [`MAX_FOLDER_DEPTH_REQUEST`] when
/// recursive, else `llmin(depth, MAX_FOLDER_DEPTH_REQUEST)`.
fn request_depth(recursive: bool, depth: i32) -> i32 {
    if recursive {
        MAX_FOLDER_DEPTH_REQUEST
    } else {
        depth.min(MAX_FOLDER_DEPTH_REQUEST)
    }
}

impl Command {
    /// The capability the command looks up first.
    pub fn cap(&self) -> CapKind {
        match self {
            Command::FetchItem { library, .. }
            | Command::FetchCategoryChildren { library, .. }
            | Command::FetchCategoryCategories { library, .. }
            | Command::FetchCategorySubset { library, .. } => CapKind::of(*library),
            Command::CopyLibraryCategory { .. } => CapKind::Library,
            _ => CapKind::Inventory,
        }
    }

    /// Whether the URL carries a transaction id (`?tid=`): the writes
    /// `CreateInventory`, `SlamFolder` and `CopyLibraryCategory`.
    pub fn uses_tid(&self) -> bool {
        matches!(
            self,
            Command::CreateInventory { .. }
                | Command::SlamFolder { .. }
                | Command::CopyLibraryCategory { .. }
        )
    }

    /// The command function after its capability lookup: `cap` is the URL
    /// the lookup gave ([`cap`](Self::cap); empty without a region), `tid`
    /// the generated transaction id (read only when
    /// [`uses_tid`](Self::uses_tid)).
    pub fn build(&self, cap: &str, tid: &Uuid) -> Result<Request, Refusal> {
        if cap.is_empty() {
            return Err(Refusal::NoCapability);
        }
        let tid = id_text(tid);
        let req = |proc_name, method, url: String, http_body, target_id, body, command| Request {
            proc_name,
            method,
            url,
            http_body,
            destination: None,
            target_id,
            body,
            command,
            url_too_long: false,
        };
        Ok(match self {
            Command::CreateInventory {
                parent_id,
                new_inventory,
            } => req(
                "CreateInventory",
                Method::Post,
                format!("{cap}/category/{}?tid={tid}", id_text(parent_id)),
                Some(new_inventory.clone()),
                *parent_id,
                new_inventory.clone(),
                CommandType::CreateInventory,
            ),
            Command::SlamFolder {
                folder_id,
                new_inventory,
            } => req(
                "SlamFolder",
                Method::Put,
                format!("{cap}/category/{}/links?tid={tid}", id_text(folder_id)),
                Some(new_inventory.clone()),
                *folder_id,
                new_inventory.clone(),
                CommandType::SlamFolder,
            ),
            Command::RemoveCategory { category_id } => req(
                "RemoveCategory",
                Method::Delete,
                format!("{cap}/category/{}", id_text(category_id)),
                None,
                *category_id,
                Value::Undefined,
                CommandType::RemoveCategory,
            ),
            Command::RemoveItem { item_id } => req(
                "RemoveItem",
                Method::Delete,
                format!("{cap}/item/{}", id_text(item_id)),
                None,
                *item_id,
                Value::Undefined,
                CommandType::RemoveItem,
            ),
            Command::PurgeDescendents { category_id } => req(
                "PurgeDescendents",
                Method::Delete,
                format!("{cap}/category/{}/children", id_text(category_id)),
                None,
                *category_id,
                Value::Undefined,
                CommandType::PurgeDescendents,
            ),
            Command::UpdateCategory {
                category_id,
                updates,
            } => req(
                "UpdateCategory",
                Method::Patch,
                format!("{cap}/category/{}", id_text(category_id)),
                Some(updates.clone()),
                *category_id,
                updates.clone(),
                CommandType::UpdateCategory,
            ),
            Command::UpdateItem { item_id, updates } => req(
                "UpdateItem",
                Method::Patch,
                format!("{cap}/item/{}", id_text(item_id)),
                Some(updates.clone()),
                *item_id,
                updates.clone(),
                CommandType::UpdateItem,
            ),
            Command::FetchItem { item_id, .. } => req(
                "FetchItem",
                Method::Get,
                format!("{cap}/item/{}", id_text(item_id)),
                None,
                *item_id,
                Value::Undefined,
                CommandType::FetchItem,
            ),
            Command::FetchCategoryChildren {
                category_id,
                recursive,
                depth,
                ..
            } => {
                let depth = request_depth(*recursive, *depth);
                req(
                    "FetchCategoryChildren",
                    Method::Get,
                    format!(
                        "{cap}/category/{}/children?depth={depth}",
                        id_text(category_id)
                    ),
                    None,
                    *category_id,
                    depth_body(depth),
                    CommandType::FetchCategoryChildren,
                )
            }
            Command::FetchCategoryChildrenByName {
                identifier,
                recursive,
                depth,
            } => {
                let depth = request_depth(*recursive, *depth);
                req(
                    "FetchCategoryChildren",
                    Method::Get,
                    format!("{cap}/category/{identifier}/children?depth={depth}"),
                    None,
                    NULL_ID,
                    depth_body(depth),
                    CommandType::FetchCategoryChildren,
                )
            }
            Command::FetchCategoryCategories {
                category_id,
                recursive,
                depth,
                ..
            } => {
                let depth = request_depth(*recursive, *depth);
                req(
                    "FetchCategoryCategories",
                    Method::Get,
                    format!(
                        "{cap}/category/{}/categories?depth={depth}",
                        id_text(category_id)
                    ),
                    None,
                    *category_id,
                    depth_body(depth),
                    CommandType::FetchCategoryCategories,
                )
            }
            Command::FetchCategorySubset {
                category_id,
                children,
                recursive,
                depth,
                ..
            } => {
                let Some((first, rest)) = children.split_first() else {
                    return Err(Refusal::EmptyRequest);
                };
                let depth = request_depth(*recursive, *depth);
                let mut url = format!(
                    "{cap}/category/{}/children?depth={depth}&children={}",
                    id_text(category_id),
                    id_text(first)
                );
                for child in rest {
                    url.push(',');
                    url.push_str(&id_text(child));
                }
                let mut r = req(
                    "FetchCategorySubset",
                    Method::Get,
                    url,
                    None,
                    *category_id,
                    depth_body(depth),
                    CommandType::FetchCategorySubset,
                );
                r.url_too_long = r.url.len() > MAX_URL_LENGTH;
                r
            }
            Command::FetchCof => req(
                "FetchCOF",
                Method::Get,
                format!("{cap}/category/current/links"),
                None,
                NULL_ID,
                depth_body(0),
                CommandType::FetchCof,
            ),
            Command::FetchCategoryLinks { category_id } => req(
                "FetchCategoryLinks",
                Method::Get,
                format!("{cap}/category/{}/links", id_text(category_id)),
                None,
                NULL_ID,
                depth_body(0),
                CommandType::FetchCategoryLinks,
            ),
            Command::FetchOrphans => req(
                "FetchOrphans",
                Method::Get,
                format!("{cap}/orphans"),
                None,
                NULL_ID,
                Value::Undefined,
                CommandType::FetchOrphans,
            ),
            Command::CopyLibraryCategory {
                source_id,
                dest_id,
                copy_subfolders,
            } => {
                let mut url = format!("{cap}/category/{}?tid={tid}", id_text(source_id));
                if !copy_subfolders {
                    url.push_str(",depth=0");
                }
                let mut r = req(
                    "CopyLibraryCategory",
                    Method::Copy,
                    url,
                    None,
                    *dest_id,
                    Value::Undefined,
                    CommandType::CopyLibraryCategory,
                );
                r.destination = Some(id_text(dest_id));
                r
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_types_round_trip() {
        for (n, t) in CommandType::ALL.into_iter().enumerate() {
            assert_eq!(t as i32, n as i32);
            assert_eq!(CommandType::from_i32(n as i32), Some(t));
            assert_eq!(CommandType::from_name(t.name()), Some(t));
        }
        assert_eq!(CommandType::from_i32(16), None);
        assert_eq!(CommandType::from_i32(-1), None);
        let fetches = CommandType::ALL
            .into_iter()
            .filter(|t| t.is_fetch())
            .count();
        assert_eq!(fetches, 7);
    }

    #[test]
    fn refusals_and_depths() {
        let id = [0xc0; 16];
        assert_eq!(
            Command::FetchOrphans.build("", &NULL_ID),
            Err(Refusal::NoCapability)
        );
        let empty = Command::FetchCategorySubset {
            category_id: id,
            children: vec![],
            library: false,
            recursive: false,
            depth: 3,
        };
        assert_eq!(
            empty.build("https://cap", &NULL_ID),
            Err(Refusal::EmptyRequest)
        );
        let children = |recursive, depth| Command::FetchCategoryChildren {
            category_id: id,
            library: true,
            recursive,
            depth,
        };
        let fetch = |recursive, depth| children(recursive, depth).build("c", &NULL_ID).unwrap();
        assert!(fetch(false, 51).url.ends_with("/children?depth=50"));
        assert!(fetch(false, -1).url.ends_with("/children?depth=-1"));
        assert!(fetch(true, 0).url.ends_with("/children?depth=50"));
        assert_eq!(fetch(true, 0).body, depth_body(50));
        assert_eq!(children(false, 0).cap(), CapKind::Library);
        let copy = Command::CopyLibraryCategory {
            source_id: id,
            dest_id: [0xd0; 16],
            copy_subfolders: false,
        };
        let r = copy.build("c", &[0x7d; 16]).unwrap();
        assert!(
            r.url
                .ends_with("?tid=7d7d7d7d-7d7d-7d7d-7d7d-7d7d7d7d7d7d,depth=0")
        );
        assert_eq!(
            r.destination.as_deref(),
            Some("d0d0d0d0-d0d0-d0d0-d0d0-d0d0d0d0d0d0")
        );
        assert_eq!(r.target_id, [0xd0; 16]);
        assert!(copy.uses_tid());
    }
}
