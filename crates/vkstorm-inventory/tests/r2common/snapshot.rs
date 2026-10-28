//! A test-only stand-in for the parts of `LLInventoryModel` that
//! `validate()` reads, and the recorder's model states built on it: the
//! standard world of `llinventorymodel_parity_test.cpp` (`standard_world`,
//! the fields `validate()` reads), its `populate` (a login without a cache,
//! `buildParentChildMap`'s child vectors, `updateItem` of every item, the
//! server's versions and the counts of the loaded children), and the model
//! summary `dump_model(..., false)` writes. The summary lines are compared
//! with the fixture to show that the state handed to `validate` is the one
//! the C++ validated.
//!
//! What it does not model is left out of that comparison: the lock maps'
//! entry counts (`locks cat_entries=`) and the pending notification batch,
//! which record C++ container internals `validate()` does not read.

use std::collections::{BTreeMap, BTreeSet};

use vkstorm_inventory::perms::{Uuid, NULL_ID};
use vkstorm_inventory::types::{AssetType, FolderType};
use vkstorm_inventory::validate::{self, QaRandom, SnapCategory, SnapItem, Snapshot, Validation};

use super::uuid;
use crate::common::sha256_hex;

pub fn pid(tag: u32, n: u64) -> Uuid {
    let mut id = [0u8; 16];
    id[..4].copy_from_slice(&tag.to_be_bytes());
    id[6] = 0x40;
    id[8] = 0x80;
    for k in 0..6 {
        id[15 - k] = (n >> (8 * k)) as u8;
    }
    id
}

pub const TAG_FOLDER: u32 = 0xf000_0000;
pub const TAG_ITEM: u32 = 0xa000_0000;
pub const TAG_ASSET: u32 = 0xa500_0000;
pub const TAG_LIBF: u32 = 0xf100_0000;
pub const TAG_LIBI: u32 = 0xa100_0000;

pub fn f(n: u64) -> Uuid {
    pid(TAG_FOLDER, n)
}
pub fn i(n: u64) -> Uuid {
    pid(TAG_ITEM, n)
}
pub fn a(n: u64) -> Uuid {
    pid(TAG_ASSET, n)
}
pub fn lf(n: u64) -> Uuid {
    pid(TAG_LIBF, n)
}
pub fn li(n: u64) -> Uuid {
    pid(TAG_LIBI, n)
}

pub fn agent() -> Uuid {
    pid(0xa9e0_0000, 1)
}
pub fn lib_owner() -> Uuid {
    pid(0x11b0_0000, 1)
}

// The standard folders (`StdFolder`).
pub const SF_ROOT: u64 = 1;
pub const SF_TEXTURES: u64 = 2;
pub const SF_OBJECTS: u64 = 7;
pub const SF_NOTECARDS: u64 = 8;
pub const SF_TRASH: u64 = 11;
pub const SF_COF: u64 = 17;
pub const SF_INBOX: u64 = 19;
pub const SF_SETTINGS: u64 = 20;

#[derive(Clone, Debug)]
pub struct Folder {
    pub id: Uuid,
    pub parent: Uuid,
    pub ftype: i32,
    pub version: i32,
}

#[derive(Clone, Debug)]
pub struct WItem {
    pub id: Uuid,
    pub parent: Uuid,
    pub atype: AssetType,
    pub asset: Uuid,
}

/// A world: the server's view of an agent inventory and a library.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub agent_root: Uuid,
    pub lib_root: Uuid,
    pub agent: Vec<Folder>,
    pub library: Vec<Folder>,
    pub items: Vec<WItem>,
}

impl World {
    pub fn with_folder(mut self, id: Uuid, parent: Uuid, ftype: i32, version: i32) -> World {
        self.agent.push(Folder { id, parent, ftype, version });
        self
    }

    /// `without_folder(n)`: the folder and its items.
    pub fn without_folder(mut self, id: Uuid) -> World {
        self.agent.retain(|fo| fo.id != id);
        self.items.retain(|it| it.parent != id);
        self
    }
}

/// `standard_world()`.
pub fn standard_world() -> World {
    let mut w = World { agent_root: f(SF_ROOT), lib_root: lf(1), ..World::default() };
    let folders: &[(u64, u64, i32, i32)] = &[
        (1, 0, 8, 40),
        (2, 1, 0, 3),
        (3, 1, 1, 1),
        (4, 1, 2, 2),
        (5, 1, 3, 5),
        (6, 1, 5, 4),
        (7, 1, 6, 12),
        (8, 1, 7, 2),
        (9, 1, 10, 2),
        (10, 1, 13, 6),
        (11, 1, 14, 9),
        (12, 1, 15, 1),
        (13, 1, 16, 1),
        (14, 1, 20, 1),
        (15, 1, 21, 2),
        (16, 1, 23, 3),
        (17, 1, 46, 30),
        (18, 1, 48, 4),
        (19, 1, 50, 2),
        (20, 1, 56, 1),
        (21, 1, 57, 1),
        (40, 18, 47, 3),
        (41, 18, 47, 2),
        (42, 7, -1, 7),
        (43, 42, -1, 2),
        (44, 11, -1, 3),
        (45, 44, -1, 1),
    ];
    for &(n, p, ftype, version) in folders {
        w.agent.push(Folder { id: f(n), parent: if p == 0 { NULL_ID } else { f(p) }, ftype, version });
    }
    w.library.push(Folder { id: lf(1), parent: NULL_ID, ftype: 8, version: 11 });
    w.library.push(Folder { id: lf(2), parent: lf(1), ftype: 5, version: 5 });
    w.library.push(Folder { id: lf(3), parent: lf(1), ftype: 6, version: 4 });
    w.library.push(Folder { id: lf(4), parent: lf(3), ftype: -1, version: 2 });
    let items: &[(u64, u64, i32)] = &[
        (1, 2, 0),
        (2, 2, 0),
        (3, 5, 3),
        (4, 6, 5),
        (5, 6, 5),
        (6, 10, 13),
        (7, 10, 13),
        (8, 10, 13),
        (9, 10, 13),
        (10, 7, 6),
        (11, 42, 6),
        (12, 43, 6),
        (13, 8, 7),
        (14, 9, 10),
        (15, 15, 21),
        (16, 11, 7),
        (17, 44, 6),
        (18, 45, 0),
        (19, 4, 2),
        (20, 20, 56),
    ];
    for &(n, p, t) in items {
        w.items.push(WItem { id: i(n), parent: f(p), atype: AssetType(t), asset: a(n) });
    }
    let links: &[(u64, u64, Uuid, bool)] = &[
        (30, 40, i(4), false),
        (31, 40, i(6), false),
        (32, 41, i(5), false),
        (33, SF_COF, i(4), false),
        (34, SF_COF, i(6), false),
        (35, SF_COF, i(7), false),
        (36, SF_COF, i(8), false),
        (37, SF_COF, i(9), false),
        (38, SF_COF, f(40), true),
        (39, 16, i(3), false),
    ];
    for &(n, p, target, folder) in links {
        w.items.push(link(i(n), f(p), target, folder));
    }
    w.items.push(WItem { id: li(1), parent: lf(2), atype: AssetType::CLOTHING, asset: a(1001) });
    w.items.push(WItem { id: li(2), parent: lf(3), atype: AssetType::OBJECT, asset: a(1002) });
    w.items.push(WItem { id: li(3), parent: lf(4), atype: AssetType::OBJECT, asset: a(1003) });
    w
}

pub fn link(id: Uuid, parent: Uuid, target: Uuid, folder: bool) -> WItem {
    WItem { id, parent, atype: if folder { AssetType::LINK_FOLDER } else { AssetType::LINK }, asset: target }
}

/// The model `validate()` reads, with the summary fields of `dump_model`.
#[derive(Clone, Debug, Default)]
pub struct TestModel {
    pub library_owner: Uuid,
    pub usable: bool,
    pub snap: Snapshot,
    /// `mCategoryMap`'s and `mItemMap`'s iteration order (they are
    /// `std::unordered_map`s since upstream 28fee038e0), after the inserts
    /// and erases the C++ makes; see `support_unordered`.
    pub cat_order: super::support_unordered::UnorderedOrder,
    pub item_order: super::support_unordered::UnorderedOrder,
}

impl TestModel {
    pub fn cat_index(&self, id: &Uuid) -> usize {
        self.snap.category_map[id].unwrap()
    }
    pub fn cat_mut(&mut self, id: &Uuid) -> &mut SnapCategory {
        let k = self.cat_index(id);
        &mut self.snap.categories[k]
    }
    pub fn item_mut(&mut self, id: &Uuid) -> &mut SnapItem {
        let k = self.snap.item_map[id];
        &mut self.snap.items[k]
    }

    /// `addCategory` with the folder's child vectors (as `buildParentChildMap`
    /// or `updateCategory` of a new folder make them), at the end of its
    /// parent's vector.
    pub fn add_category(&mut self, id: Uuid, parent: Uuid, ftype: i32) -> usize {
        let k = self.snap.categories.len();
        self.snap.categories.push(SnapCategory {
            id,
            parent,
            preferred_type: FolderType(ftype),
            version: -1,
            descendent_count: -1,
        });
        self.snap.category_map.insert(id, Some(k));
        self.cat_order.insert(id);
        self.snap.category_tree.entry(id).or_default();
        self.snap.item_tree.entry(id).or_default();
        if let Some(v) = self.snap.category_tree.get_mut(&parent) {
            v.push(Some(k));
        }
        k
    }

    /// `updateItem` of a new item: the map, its parent's vector, and the
    /// backlink of a link.
    pub fn update_item_new(&mut self, it: &WItem) {
        let k = self.snap.items.len();
        self.snap.items.push(SnapItem { id: it.id, parent: it.parent, asset_type: it.atype, asset: it.asset });
        self.snap.item_map.insert(it.id, k);
        self.item_order.insert(it.id);
        if let Some(v) = self.snap.item_tree.get_mut(&it.parent) {
            v.push(Some(k));
        }
        if it.atype.is_link_type() && !self.snap.backlinks.contains(&(it.asset, it.id)) {
            self.snap.backlinks.push((it.asset, it.id));
        }
    }

    /// `remove_from_vector` of the C4 recorder.
    pub fn remove_from_vectors(&mut self, parent: &Uuid, child: &Uuid) {
        let s = &mut self.snap;
        let (items, cats) = (&s.items, &s.categories);
        if let Some(v) = s.item_tree.get_mut(parent) {
            v.retain(|e| e.map(|k| items[k].id != *child).unwrap_or(true));
        }
        if let Some(v) = s.category_tree.get_mut(parent) {
            v.retain(|e| e.map(|k| cats[k].id != *child).unwrap_or(true));
        }
    }

    pub fn validate(&self, qa: Option<&mut QaRandom>) -> Validation {
        let mut s = self.snap.clone();
        s.category_order = synced(&self.cat_order, s.category_map.keys());
        s.item_order = synced(&self.item_order, s.item_map.keys());
        validate::validate(&s, validate::Options { qa, in_open_sim: false })
    }

    /// `dump_model(out, false)` without the lock entry counts and the
    /// pending batch.
    pub fn summary(&self) -> Vec<String> {
        let s = &self.snap;
        let mut out = vec![format!(
            "model root={} library_root={} library_owner={} usable={} cats={} items={} cat_trees={} item_trees={}",
            uuid(&s.root),
            uuid(&s.library_root),
            uuid(&self.library_owner),
            u8::from(self.usable),
            s.category_map.len(),
            s.item_map.len(),
            s.category_tree.len(),
            s.item_tree.len()
        )];
        let parents: BTreeSet<Uuid> = s.category_tree.keys().chain(s.item_tree.keys()).copied().collect();
        let list = |v: Option<&Vec<Option<usize>>>, id: &dyn Fn(usize) -> Uuid| match v {
            None => "none".to_owned(),
            Some(v) => {
                let names: Vec<String> = v.iter().map(|e| e.map(|k| uuid(&id(k))).unwrap_or_else(|| "null".into())).collect();
                format!("[{}]", names.join(","))
            }
        };
        for p in parents {
            let cats = s.category_tree.get(&p);
            let items = s.item_tree.get(&p);
            if cats.is_some_and(|v| v.is_empty()) && items.is_some_and(|v| v.is_empty()) {
                continue;
            }
            out.push(format!(
                "children {} cats={} items={}",
                uuid(&p),
                list(cats, &|k| s.categories[k].id),
                list(items, &|k| s.items[k].id)
            ));
        }
        let mut backlinks = s.backlinks.clone();
        backlinks.sort_by(|x, y| x.0.cmp(&y.0)); // stable: insertion order within a target
        for (t, l) in backlinks {
            out.push(format!("backlink {} {}", uuid(&t), uuid(&l)));
        }
        for id in &s.category_locks {
            out.push(format!("lock cat {}", uuid(id)));
        }
        for id in &s.item_locks {
            out.push(format!("lock item {}", uuid(id)));
        }
        out
    }
}

/// C4's long-line rule: a line over 4,000 characters by its key, length and
/// digest.
pub fn long_line(l: &str) -> String {
    if l.len() > 4000 {
        let sp = l[9..].find(' ').map(|p| p + 9).unwrap_or(l.len());
        format!("{} long={} sha256={}", &l[..sp], l.len(), sha256_hex(l.as_bytes()))
    } else {
        l.to_owned()
    }
}

/// `populate(w)`: a login without a cache (every folder of unknown version
/// and count; `buildParentChildMap` puts each folder in its parent's vector
/// in folder-id order and gives every folder an empty item vector), its
/// `validate()` (drawing from `qa` when set) deciding whether the inventory
/// is usable; then, when it is, `updateItem` of every item in world order;
/// then the server's versions and the counts of the loaded children.
/// The emulated order brought in line with the map's keys: a C4 case that
/// writes a map directly (one change, then `validate()`) erased or inserted
/// that key just before.
fn synced<'a>(order: &super::support_unordered::UnorderedOrder, keys: impl Iterator<Item = &'a Uuid>) -> Vec<Uuid> {
    let keys: BTreeSet<Uuid> = keys.copied().collect();
    let mut o = order.clone();
    for k in o.order() {
        if !keys.contains(&k) {
            o.erase(&k);
        }
    }
    for k in &keys {
        o.insert(*k);
    }
    o.order()
}

pub fn populate(w: &World, qa: Option<&mut QaRandom>) -> TestModel {
    let mut m = TestModel { library_owner: lib_owner(), ..TestModel::default() };
    m.snap.root = w.agent_root;
    m.snap.library_root = w.lib_root;
    let mut folders: BTreeMap<Uuid, &Folder> = BTreeMap::new();
    for fo in w.library.iter().chain(w.agent.iter()) {
        folders.entry(fo.id).or_insert(fo);
    }
    // `login`: `loadSkeleton` of the library, then of the agent, each adding
    // its folders through the `temp_cats` set (by id); a folder already in
    // the map keeps its place.
    for list in [&w.library, &w.agent] {
        let mut ids: Vec<Uuid> = list.iter().map(|fo| fo.id).collect();
        ids.sort();
        for id in ids {
            m.cat_order.insert(id);
        }
    }
    m.snap.category_tree.insert(NULL_ID, Vec::new());
    for fo in folders.values() {
        let k = m.snap.categories.len();
        m.snap.categories.push(SnapCategory {
            id: fo.id,
            parent: fo.parent,
            preferred_type: FolderType(fo.ftype),
            version: -1,
            descendent_count: -1,
        });
        m.snap.category_map.insert(fo.id, Some(k));
        m.snap.category_tree.insert(fo.id, Vec::new());
        m.snap.item_tree.insert(fo.id, Vec::new());
    }
    // Only the root folders may be children of null; the others, and those
    // whose parent is missing, are lost: an untyped one goes to Lost And
    // Found, a typed one to the root, after the first pass.
    // `buildParentChildMap` walks `mCategoryMap` in its order.
    let mut lost = Vec::new();
    for id in m.cat_order.order() {
        let fo = folders[&id];
        let k = m.cat_index(&id);
        match m.snap.category_tree.get_mut(&fo.parent) {
            Some(v) if fo.parent != NULL_ID || fo.ftype == 8 => v.push(Some(k)),
            _ => lost.push(k),
        }
    }
    for k in lost {
        let ftype = m.snap.categories[k].preferred_type.0;
        let parent = match ftype {
            -1 => m
                .snap
                .categories
                .iter()
                .filter(|c| c.preferred_type.0 == 16 && c.parent == m.snap.root)
                .map(|c| c.id)
                .min()
                .unwrap_or(NULL_ID),
            8 => NULL_ID,
            _ => m.snap.root,
        };
        m.snap.categories[k].parent = parent;
        if let Some(v) = m.snap.category_tree.get_mut(&parent) {
            v.push(Some(k));
        }
    }
    m.usable = m.validate(qa).info.fatal_error_count == 0;
    if m.usable {
        for it in &w.items {
            m.update_item_new(it);
        }
    }
    for (id, fo) in &folders {
        let k = m.cat_index(id);
        let count = m.snap.category_tree[id].len() + m.snap.item_tree[id].len();
        let c = &mut m.snap.categories[k];
        c.version = fo.version;
        c.descendent_count = count as i32;
    }
    m
}
