//! The legacy capability requests: the `FetchInventoryDescendents2` /
//! `FetchLibDescendents2` folder bodies and `FetchInventory2` / `FetchLib2`
//! item bodies `bulkFetch()` builds (`llinventorymodelbackgroundfetch.cpp:1220-1449`),
//! the request a `BGFolderHttpHandler` keeps for its 403 split and its
//! re-issue (`:1688-1835`), and `fetchFromServer`'s foreground item body
//! (`llviewerinventory.cpp:517-525`).
//!
//! Bodies are LLSD XML as `LLCoreHttpUtil` serializes them for
//! `requestPost` (`<llsd>...</llsd>` and a newline, no header), through
//! `vkstorm-llsd` (rung two). Map keys are ordered as `std::map`.

use std::collections::BTreeMap;

use vkstorm_llsd::{Format, Options, Value};

use super::{Uuid, uuid_string};

/// One entry of a folder request's `folders` array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderRequest {
    pub folder_id: Uuid,
    /// The lost-and-found entry (DEV-17797) sends `folder_id` as the string
    /// form of the null id, not as a UUID.
    pub id_as_string: bool,
    pub owner_id: Uuid,
    pub sort_order: i32,
    /// `fetch_folders`: false only for the lost-and-found entry.
    pub fetch_folders: bool,
}

impl FolderRequest {
    fn to_value(&self) -> Value {
        let mut m = BTreeMap::new();
        m.insert(
            b"fetch_folders".to_vec(),
            Value::Boolean(self.fetch_folders),
        );
        m.insert(b"fetch_items".to_vec(), Value::Boolean(true));
        m.insert(
            b"folder_id".to_vec(),
            if self.id_as_string {
                Value::String(uuid_string(&self.folder_id).into_bytes())
            } else {
                Value::Uuid(self.folder_id)
            },
        );
        m.insert(b"owner_id".to_vec(), Value::Uuid(self.owner_id));
        m.insert(b"sort_order".to_vec(), Value::Integer(self.sort_order));
        Value::Map(m)
    }
}

/// A folder request: `folders` is `None` for the undefined `folders` of the
/// last POST of a 403 split (R3b finding 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderBody {
    pub folders: Option<Vec<FolderRequest>>,
}

impl FolderBody {
    /// `mRequestSD["folders"].size()`
    pub fn len(&self) -> usize {
        self.folders.as_ref().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn to_value(&self) -> Value {
        let mut m = BTreeMap::new();
        m.insert(
            b"folders".to_vec(),
            match &self.folders {
                Some(f) => Value::Array(f.iter().map(FolderRequest::to_value).collect()),
                None => Value::Undefined,
            },
        );
        Value::Map(m)
    }

    pub fn to_xml(&self) -> Vec<u8> {
        llsd_xml(&self.to_value())
    }
}

/// One `{owner_id, item_id}` of an item request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemRequest {
    pub item_id: Uuid,
    pub owner_id: Uuid,
}

/// `{items: [...]}` of `bulkFetch()`, or, with `agent_id`, the foreground
/// `fetchFromServer` body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemBody {
    pub agent_id: Option<Uuid>,
    pub items: Vec<ItemRequest>,
}

impl ItemBody {
    pub fn to_value(&self) -> Value {
        let items = self
            .items
            .iter()
            .map(|i| {
                let mut m = BTreeMap::new();
                m.insert(b"item_id".to_vec(), Value::Uuid(i.item_id));
                m.insert(b"owner_id".to_vec(), Value::Uuid(i.owner_id));
                Value::Map(m)
            })
            .collect();
        let mut m = BTreeMap::new();
        if let Some(a) = self.agent_id {
            m.insert(b"agent_id".to_vec(), Value::Uuid(a));
        }
        m.insert(b"items".to_vec(), Value::Array(items));
        Value::Map(m)
    }

    pub fn to_xml(&self) -> Vec<u8> {
        llsd_xml(&self.to_value())
    }
}

/// `LLSDSerialize::toXML` of a request body, as `requestPost` sends it.
pub fn llsd_xml(v: &Value) -> Vec<u8> {
    let (mut out, _) = vkstorm_llsd::format(Format::Xml, v, Options::default());
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    out
}

/// The chunks `processFailure` posts for a 403 on a request of more than one
/// folder: a chunk each time the accumulator reaches `size / 2`, then the
/// rest, which is undefined when the last chunk took everything (finding 1).
/// Each chunk carries the recursive ids among its folders, in its order.
pub fn split_403(body: &FolderBody, recursive: &[Uuid]) -> Vec<(FolderBody, Vec<Uuid>)> {
    let folders = body.folders.as_deref().unwrap_or(&[]);
    let half = folders.len() / 2;
    let mut out = Vec::new();
    let mut acc: Option<Vec<FolderRequest>> = None;
    let mut rec = Vec::new();
    for f in folders {
        let a = acc.get_or_insert_with(Vec::new);
        a.push(f.clone());
        // folder_id.asUUID(): the lost-and-found string reads as null
        let id = f.folder_id;
        if recursive.contains(&id) {
            rec.push(id);
        }
        if a.len() == half {
            out.push((
                FolderBody {
                    folders: acc.take(),
                },
                std::mem::take(&mut rec),
            ));
        }
    }
    out.push((FolderBody { folders: acc }, rec));
    out
}
