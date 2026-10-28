//! Replay support for the R3b fetch wire corpus (`fixtures/ais/fetch`,
//! README there): the synthetic inventories of `llinvfetch_parity_scenarios.cpp`,
//! a replay model standing in for the C++ model, the frame loop and AIS pool
//! of the recorder, and the comparison.
//!
//! What is under test is `vkstorm_inventory::fetch`. Everything else here is
//! scaffolding that stands in for C++ the port keeps:
//!
//! - the replay model replaces `LLInventoryModel` and `AISUpdate` for what
//!   the scheduler reads: it applies a recorded AIS reply with the parse
//!   rules of `AISUpdate::parseCategory`/`parseItem`/`parseLink` (fetch
//!   form: versions only with a known descendent count at depth >= 0; the
//!   subset's top skipped; created categories applied in id order and
//!   appended to their parent) and `updateCategory`/`updateItem`'s child
//!   lists. It does not reproduce the model dump, observer batches,
//!   marketplace hooks or UDP messages (those lines are not compared);
//! - the frame loop runs the idle callback, then the coroutines: a reply is
//!   processed one frame after its request was sent, in send order, and a
//!   free pool coroutine takes the next queued command at once;
//! - the transport: a request is matched with the next recorded request; a
//!   503 answered request repeated identically is llcorehttp's retry (at
//!   most 6 attempts); request heads come from the two recorded header
//!   templates (AIS coroutine adapter, legacy `requestPost`). Status codes
//!   and replies are inputs.
#![allow(dead_code)]

pub mod ais_standin;
pub mod mock;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;

use ais_standin::{AisCaps, AisNotices, AisReply, AisRequest, Command};
use vkstorm_inventory::fetch::scheduler::{
    self, Action, BackgroundFetch, BudgetClock, CategoryInfo, Ctx, Env, FetchModel, FetchPath, ItemInfo,
};
use vkstorm_inventory::fetch::timer::{CategoryFetchState, FETCH_DEFAULT_EXPIRY};
use vkstorm_inventory::fetch::{CategoryFetch, NULL_ID, Uuid, uuid_string};
use vkstorm_inventory::llsd::coerce::{as_integer, as_uuid, get, has};
use vkstorm_llsd::Value;

// ---------------------------------------------------------------------------
// ids (vks_inv::pid and the scenario constants)
// ---------------------------------------------------------------------------

/// `<tag as 8 hex digits>-0000-4000-8000-<n as 12 hex digits>`
pub fn pid(tag: u32, n: u64) -> Uuid {
    let mut u = [0u8; 16];
    u[0..4].copy_from_slice(&tag.to_be_bytes());
    u[6] = 0x40;
    u[8] = 0x80;
    u[10..16].copy_from_slice(&n.to_be_bytes()[2..8]);
    u
}

pub fn parse_uuid(s: &str) -> Uuid {
    vkstorm_llsd::text::uuid_set(s.as_bytes())
}

pub fn agent() -> Uuid {
    pid(0xa9e00000, 1)
}
pub fn library_owner() -> Uuid {
    scheduler::ALEXANDRIA_LINDEN_ID
}
pub const ROOT_TAG: u32 = 0xc0000000;
pub fn root() -> Uuid {
    pid(ROOT_TAG, 1)
}
pub fn libroot() -> Uuid {
    pid(0xd0000000, 1)
}
pub fn sys(t: i32) -> Uuid {
    pid(0xc0000001, (t + 2) as u64)
}
pub fn usr(n: u64) -> Uuid {
    pid(0xc0100000, n)
}
pub fn itm(n: u64) -> Uuid {
    pid(0xa0000000, n)
}
pub fn lnk(n: u64) -> Uuid {
    pid(0xb0000000, n)
}
pub fn libf(n: u64) -> Uuid {
    pid(0xd0000000, n)
}
pub fn libi(n: u64) -> Uuid {
    pid(0xd1000000, n)
}

pub fn ver(n: u64) -> i32 {
    1 + ((n * 7) % 13) as i32
}

// folder types used by the scenarios
pub const FT_NONE: i32 = -1;
pub const FT_ROOT_INVENTORY: i32 = 8;
pub const FT_TEXTURE: i32 = 0;
pub const FT_CLOTHING: i32 = 5;
pub const FT_OBJECT: i32 = 6;
pub const FT_NOTECARD: i32 = 7;
pub const FT_LSL_TEXT: i32 = 10;
pub const FT_BODYPART: i32 = 13;
pub const FT_TRASH: i32 = 14;
pub const FT_SNAPSHOT: i32 = 15;
pub const FT_LOST_AND_FOUND: i32 = 16;
pub const FT_ANIMATION: i32 = 20;
pub const FT_GESTURE: i32 = 21;
pub const FT_CURRENT_OUTFIT: i32 = 46;
pub const FT_OUTFIT: i32 = 47;
pub const FT_MY_OUTFITS: i32 = 48;
pub const FT_MARKETPLACE_LISTINGS: i32 = 53;
pub const LANDMARK: i32 = 3;

/// The 21 system folders, in the order the Builder creates them.
pub const SYSTEM: [i32; 21] = [0, 1, 2, 3, 5, 6, 7, 10, 13, 14, 15, 16, 20, 21, 23, 46, 48, 50, 53, 56, 57];

// ---------------------------------------------------------------------------
// The synthetic inventories (Builder, basic)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct WFolder {
    pub id: Uuid,
    pub parent: Uuid,
    pub owner: Uuid,
    pub ftype: i32,
    pub version: i32,
}

#[derive(Clone, Debug)]
pub struct WItem {
    pub id: Uuid,
    pub parent: Uuid,
    pub owner: Uuid,
    /// A link's target.
    pub linked: Option<Uuid>,
}

#[derive(Clone, Debug, Default)]
pub struct World {
    pub agent: Uuid,
    pub libowner: Uuid,
    pub root: Uuid,
    pub libroot: Uuid,
    pub folders: Vec<WFolder>,
    pub items: Vec<WItem>,
    /// Folders only in the login skeleton, unknown to the server.
    pub not_on_server: Vec<Uuid>,
}

impl World {
    pub fn add_folder(&mut self, id: Uuid, parent: Uuid, ftype: i32, version: i32, library: bool) {
        let owner = if library { self.libowner } else { self.agent };
        self.folders.push(WFolder { id, parent, owner, ftype, version });
    }
    pub fn add_item(&mut self, id: Uuid, parent: Uuid, library: bool) {
        let owner = if library { self.libowner } else { self.agent };
        self.items.push(WItem { id, parent, owner, linked: None });
    }
}

#[derive(Clone, Debug, Default)]
pub struct Ids {
    pub user: Vec<Uuid>,
    pub user_sub: Vec<Uuid>,
    pub user_leaf: Vec<Uuid>,
    pub lib_folders: Vec<Uuid>,
    pub lib_items: Vec<Uuid>,
    pub mkt: Vec<Uuid>,
    pub items_in: BTreeMap<Uuid, Vec<Uuid>>,
    pub shirt: Uuid,
    pub pants: Uuid,
    pub jacket: Uuid,
    pub shape: Uuid,
    pub skin: Uuid,
    pub tex1: Uuid,
    pub obj1: Uuid,
    pub obj2: Uuid,
    pub note1: Uuid,
    pub trash1: Uuid,
    pub outfit_a: Uuid,
    pub outfit_b: Uuid,
    pub cof_shirt_link: Uuid,
}

pub struct Builder {
    pub w: World,
    pub ids: Ids,
    pub n_user: u64,
    pub n_item: u64,
    pub n_link: u64,
    pub n_lib: u64,
    pub n_libitem: u64,
}

impl Builder {
    pub fn new(library: bool) -> Self {
        let mut w = World {
            agent: agent(),
            libowner: library_owner(),
            root: root(),
            libroot: if library { libroot() } else { NULL_ID },
            ..Default::default()
        };
        w.add_folder(root(), NULL_ID, FT_ROOT_INVENTORY, 30, false);
        for t in SYSTEM {
            w.add_folder(sys(t), root(), t, ver((t + 2) as u64), false);
        }
        if library {
            w.add_folder(libroot(), NULL_ID, FT_ROOT_INVENTORY, 12, true);
        }
        Builder { w, ids: Ids::default(), n_user: 1, n_item: 1, n_link: 1, n_lib: 2, n_libitem: 1 }
    }
    pub fn item(&mut self, parent: Uuid) -> Uuid {
        let id = itm(self.n_item);
        self.w.add_item(id, parent, false);
        self.n_item += 1;
        self.ids.items_in.entry(parent).or_default().push(id);
        id
    }
    pub fn libitem(&mut self, parent: Uuid) -> Uuid {
        let id = libi(self.n_libitem);
        self.w.add_item(id, parent, true);
        self.n_libitem += 1;
        self.ids.lib_items.push(id);
        id
    }
    pub fn folder(&mut self, parent: Uuid, ftype: i32) -> Uuid {
        let id = usr(self.n_user);
        self.w.add_folder(id, parent, ftype, ver(self.n_user), false);
        self.n_user += 1;
        id
    }
    pub fn libfolder(&mut self, parent: Uuid) -> Uuid {
        let id = libf(self.n_lib);
        self.w.add_folder(id, parent, FT_NONE, ver(self.n_lib), true);
        self.n_lib += 1;
        self.ids.lib_folders.push(id);
        id
    }
    pub fn link(&mut self, parent: Uuid, target: Uuid) -> Uuid {
        let id = lnk(self.n_link);
        self.n_link += 1;
        self.w.items.push(WItem { id, parent, owner: self.w.agent, linked: Some(target) });
        id
    }
}

/// `basic(b, user_folders, orphan, lib_folders)`: only ids, parents, types
/// and owners matter here (the replies come from the recording).
pub fn basic(b: &mut Builder, user_folders: usize, orphan: bool, lib_folders: usize) {
    b.ids.tex1 = b.item(sys(FT_TEXTURE));
    b.item(sys(FT_TEXTURE));
    b.ids.shirt = b.item(sys(FT_CLOTHING));
    b.ids.pants = b.item(sys(FT_CLOTHING));
    b.ids.jacket = b.item(sys(FT_CLOTHING));
    b.ids.shape = b.item(sys(FT_BODYPART));
    b.ids.skin = b.item(sys(FT_BODYPART));
    b.ids.obj1 = b.item(sys(FT_OBJECT));
    b.ids.obj2 = b.item(sys(FT_OBJECT));
    b.ids.note1 = b.item(sys(FT_NOTECARD));
    b.item(sys(FT_LSL_TEXT));
    b.item(sys(LANDMARK));
    b.item(sys(FT_GESTURE));
    b.item(sys(FT_ANIMATION));
    b.item(sys(FT_SNAPSHOT));
    b.ids.trash1 = b.item(sys(FT_TRASH));
    b.item(sys(FT_TRASH));
    b.item(root());
    b.item(root());
    for _u in 1..=user_folders {
        let u = b.folder(root(), FT_NONE);
        b.ids.user.push(u);
        for _k in 1..=3 {
            b.item(u);
        }
        for _s in 1..=2 {
            let s = b.folder(u, FT_NONE);
            b.ids.user_sub.push(s);
            b.item(s);
            b.item(s);
            let l = b.folder(s, FT_NONE);
            b.ids.user_leaf.push(l);
            b.item(l);
        }
    }
    b.ids.outfit_a = b.folder(sys(FT_MY_OUTFITS), FT_OUTFIT);
    let (a, d) = (b.ids.outfit_a, b.ids.clone());
    b.link(a, d.shirt);
    b.link(a, d.pants);
    b.link(a, d.shape);
    b.ids.outfit_b = b.folder(sys(FT_MY_OUTFITS), FT_OUTFIT);
    let ob = b.ids.outfit_b;
    b.link(ob, d.jacket);
    let cof = sys(FT_CURRENT_OUTFIT);
    b.ids.cof_shirt_link = b.link(cof, d.shirt);
    b.link(cof, d.pants);
    b.link(cof, d.shape);
    b.link(cof, d.skin);
    b.link(cof, a);
    for _m in 1..=2 {
        let m = b.folder(sys(FT_MARKETPLACE_LISTINGS), FT_NONE);
        b.ids.mkt.push(m);
        b.item(m);
    }
    if b.w.libroot != NULL_ID {
        b.libitem(libroot());
        b.libitem(libroot());
        for _l in 1..=lib_folders {
            let f = b.libfolder(libroot());
            b.libitem(f);
            b.libitem(f);
            for _s in 1..=2 {
                let s = b.libfolder(f);
                b.libitem(s);
                b.libitem(s);
            }
        }
    }
    if orphan {
        let o = pid(0xa0f00000, 1);
        let agent = b.w.agent;
        b.w.items.push(WItem { id: o, parent: NULL_ID, owner: agent, linked: None });
    }
}

/// `large(b)`: 5,000 folders, 300 under the root, a chain 60 deep, 60,000
/// items (`llinvfetch_parity_scenarios.cpp`).
pub fn large(b: &mut Builder) {
    basic(b, 0, false, 3);
    let top = 300 - SYSTEM.len();
    let mut all_user = Vec::new();
    for _u in 1..=top {
        let u = b.folder(root(), FT_NONE);
        b.ids.user.push(u);
        all_user.push(u);
        for _c in 1..=4 {
            let c = b.folder(u, FT_NONE);
            all_user.push(c);
            for _g in 1..=3 {
                let g = b.folder(c, FT_NONE);
                all_user.push(g);
            }
        }
    }
    let mut parent = b.ids.user[0];
    for _k in 1..=60 {
        parent = b.folder(parent, FT_NONE);
        all_user.push(parent);
        b.ids.user_leaf.push(parent);
    }
    let mut k = 0;
    while b.w.folders.len() < 5000 {
        let p = b.ids.user[1 + (k % 4)];
        let f = b.folder(p, FT_NONE);
        all_user.push(f);
        k += 1;
    }
    for f in &all_user {
        for _ in 0..12 {
            if b.w.items.len() >= 60000 {
                break;
            }
            b.item(*f);
        }
    }
    while b.w.items.len() < 60000 {
        b.item(sys(FT_OBJECT));
    }
}

// ---------------------------------------------------------------------------
// The replay model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct MCat {
    pub id: Uuid,
    pub parent: Uuid,
    pub pref: i8,
    pub owner: Uuid,
    pub version: i32,
    pub count: i32,
    pub fetch: CategoryFetchState,
}

#[derive(Clone, Debug)]
pub struct MItem {
    pub id: Uuid,
    pub parent: Uuid,
    pub owner: Uuid,
    pub complete: bool,
}

#[derive(Debug, Default)]
pub struct ReplayModel {
    /// `LLFrameTimer`'s frame time.
    pub now: f64,
    pub ais_available: bool,
    pub usable: bool,
    pub agent: Uuid,
    pub root: Uuid,
    pub libroot: Uuid,
    pub libowner: Uuid,
    pub base_outfit: Uuid,
    pub cats: BTreeMap<Uuid, MCat>,
    pub cat_children: BTreeMap<Uuid, Vec<Uuid>>,
    pub items: BTreeMap<Uuid, MItem>,
    pub item_children: BTreeMap<Uuid, Vec<Uuid>>,
}

/// An inventory cache file as the replay model needs it: the cached
/// folders' versions and the cached items.
#[derive(Clone, Debug, Default)]
pub struct CacheFile {
    pub cats: BTreeMap<Uuid, i32>,
    pub items: Vec<MItem>,
}

impl ReplayModel {
    fn viewer_count(&self, id: &Uuid) -> i32 {
        (self.cat_children.get(id).map_or(0, Vec::len) + self.item_children.get(id).map_or(0, Vec::len)) as i32
    }

    /// `cache(root)` and `cache(library root)` at logout: a folder is saved
    /// when its version is known and its server count equals its viewer
    /// count (`LLCanCache`); an item when its parent was saved.
    pub fn save_cache(&self) -> CacheFile {
        let mut f = CacheFile::default();
        for top in [self.root, self.libroot] {
            let Some(c) = self.cats.get(&top) else { continue };
            let mut cached: BTreeSet<Uuid> = BTreeSet::new();
            let can = |c: &MCat| c.version != -1 && c.count == self.viewer_count(&c.id);
            if can(c) {
                cached.insert(top);
                f.cats.insert(top, c.version);
            }
            let mut stack = vec![top];
            while let Some(id) = stack.pop() {
                for k in self.cat_children.get(&id).cloned().unwrap_or_default() {
                    if let Some(c) = self.cats.get(&k) {
                        if can(c) {
                            cached.insert(k);
                            f.cats.insert(k, c.version);
                        }
                    }
                    stack.push(k);
                }
                for i in self.item_children.get(&id).cloned().unwrap_or_default() {
                    if let Some(it) = self.items.get(&i) {
                        if cached.contains(&it.parent) {
                            f.items.push(it.clone());
                        }
                    }
                }
            }
        }
        f
    }

    /// `loadSkeleton` with a cache: a skeleton folder keeps its version when
    /// the cache has it at the same version; cached items under a folder
    /// with a version are added; a folder with a version gets the count of
    /// its loaded children.
    pub fn load_warm(&mut self, w: &World, cache: &CacheFile) {
        let mut counts: BTreeMap<Uuid, i32> = BTreeMap::new();
        for f in &w.folders {
            let owner = if f.owner == w.libowner && w.libroot != NULL_ID { w.libowner } else { w.agent };
            let version = match cache.cats.get(&f.id) {
                Some(v) if *v == f.version => f.version,
                _ => -1,
            };
            self.cats.insert(
                f.id,
                MCat {
                    id: f.id,
                    parent: f.parent,
                    pref: f.ftype as i8,
                    owner,
                    version,
                    count: -1,
                    fetch: CategoryFetchState::new(self.now),
                },
            );
            *counts.entry(f.parent).or_default() += 1;
        }
        for it in &cache.items {
            if self.cats.get(&it.parent).is_some_and(|c| c.version != -1) {
                self.items.insert(it.id, it.clone());
                *counts.entry(it.parent).or_default() += 1;
            }
        }
        for c in self.cats.values_mut() {
            if c.version != -1 {
                c.count = counts.get(&c.id).copied().unwrap_or(0);
            }
        }
    }

    /// `findCategoryUUIDForType`: the first direct child of the root with
    /// that preferred type.
    pub fn find_category_for_type(&self, t: i32) -> Uuid {
        if let Some(kids) = self.cat_children.get(&self.root) {
            for k in kids {
                if self.cats.get(k).is_some_and(|c| c.pref as i32 == t) {
                    return *k;
                }
            }
        }
        NULL_ID
    }

    /// `loadSkeleton` of a cold login (no cache: every version unknown);
    /// `buildParentChildMap` then lists children in id order.
    pub fn load_cold(&mut self, w: &World) {
        for f in &w.folders {
            let owner = if f.owner == w.libowner && w.libroot != NULL_ID { w.libowner } else { w.agent };
            self.cats.insert(
                f.id,
                MCat {
                    id: f.id,
                    parent: f.parent,
                    pref: f.ftype as i8,
                    owner,
                    version: -1,
                    count: -1,
                    fetch: CategoryFetchState::new(self.now),
                },
            );
        }
    }

    /// `buildParentChildMap`: existing arrays are kept, so a second call
    /// lists every child twice (s21 calls it before the login step does).
    pub fn build_parent_child_map(&mut self) {
        self.cat_children.entry(NULL_ID).or_default();
        for id in self.cats.keys() {
            self.cat_children.entry(*id).or_default();
            self.item_children.entry(*id).or_default();
        }
        let cats: Vec<(Uuid, Uuid)> = self.cats.values().map(|c| (c.id, c.parent)).collect();
        for (id, parent) in cats {
            if let Some(v) = self.cat_children.get_mut(&parent) {
                v.push(id);
            }
        }
        let items: Vec<(Uuid, Uuid)> = self.items.values().map(|i| (i.id, i.parent)).collect();
        for (id, parent) in items {
            if let Some(v) = self.item_children.get_mut(&parent) {
                v.push(id);
            }
        }
        // validate(): a missing library root makes the inventory unusable
        self.usable = self.root != NULL_ID && self.libroot != NULL_ID;
    }

    fn update_category(&mut self, c: MCat) {
        if !self.usable {
            return;
        }
        if let Some(old) = self.cats.get(&c.id).cloned() {
            if old.parent != c.parent {
                if let Some(v) = self.cat_children.get_mut(&old.parent) {
                    vector_replace_with_last(v, &c.id);
                }
                if let Some(v) = self.cat_children.get_mut(&c.parent) {
                    v.push(c.id);
                }
            }
            let fetching = old.fetch.fetching;
            let mut n = c;
            n.fetch.fetching = fetching;
            self.cats.insert(n.id, n);
        } else {
            let id = c.id;
            let parent = c.parent;
            let mut n = c;
            n.fetch.fetching = CategoryFetch::None;
            self.cats.insert(id, n);
            if let Some(v) = self.cat_children.get_mut(&parent) {
                v.push(id);
            }
            self.cat_children.insert(id, Vec::new());
            self.item_children.insert(id, Vec::new());
        }
    }

    fn update_item(&mut self, i: MItem) {
        if !self.usable {
            return;
        }
        if let Some(old) = self.items.get(&i.id).cloned() {
            if old.parent != i.parent {
                if let Some(v) = self.item_children.get_mut(&old.parent) {
                    vector_replace_with_last(v, &i.id);
                }
                if let Some(v) = self.item_children.get_mut(&i.parent) {
                    v.push(i.id);
                }
            }
            self.items.insert(i.id, i);
        } else {
            let (id, parent) = (i.id, i.parent);
            self.items.insert(id, i);
            if let Some(v) = self.item_children.get_mut(&parent) {
                v.push(id);
            }
        }
    }
}

impl ReplayModel {
    pub fn update_category_pub(&mut self, c: MCat) {
        self.update_category(c);
    }

    /// `isCategoryComplete`
    pub fn is_category_complete(&self, id: &Uuid) -> bool {
        self.cats.get(id).is_some_and(|c| c.version != -1 && c.count == self.viewer_count(id))
    }

    /// `accountForUpdate(LLCategoryUpdate(id, delta))`
    pub fn account_for_update(&mut self, id: &Uuid, delta: i32) {
        let actual = self.viewer_count(id);
        if let Some(c) = self.cats.get_mut(id) {
            if c.version != -1 && c.count == actual {
                c.count = actual + delta;
                c.version += 1;
            }
        }
    }

    /// An item of a legacy reply into the model (`unpackMessage`,
    /// `updateItem`), optionally moved to `parent`.
    pub fn legacy_item(&mut self, it: &Value, parent: Option<Uuid>) {
        let id = as_uuid(get(it, "item_id"));
        let mut i = self.items.get(&id).cloned().unwrap_or(MItem { id, parent: NULL_ID, owner: NULL_ID, complete: true });
        i.parent = parent.unwrap_or_else(|| as_uuid(get(it, "parent_id")));
        let perms = get(it, "permissions");
        if has(perms, "owner_id") {
            i.owner = as_uuid(get(perms, "owner_id"));
        } else if has(it, "agent_id") {
            i.owner = as_uuid(get(it, "agent_id"));
        }
        i.complete = true;
        self.update_item(i);
    }
}

fn vector_replace_with_last(v: &mut Vec<Uuid>, id: &Uuid) {
    if let Some(p) = v.iter().position(|x| x == id) {
        v.swap_remove(p);
    }
}

impl FetchModel for ReplayModel {
    fn root_folder_id(&self) -> Uuid {
        self.root
    }
    fn library_root_folder_id(&self) -> Uuid {
        self.libroot
    }
    fn library_owner_id(&self) -> Uuid {
        self.libowner
    }
    fn marketplace_listings_id(&self) -> Uuid {
        self.find_category_for_type(FT_MARKETPLACE_LISTINGS)
    }
    fn current_outfit_id(&self) -> Uuid {
        self.find_category_for_type(FT_CURRENT_OUTFIT)
    }
    fn base_outfit_id(&self) -> Uuid {
        self.base_outfit
    }
    fn category(&self, id: &Uuid) -> Option<CategoryInfo> {
        self.cats
            .get(id)
            .map(|c| CategoryInfo { version: c.version, preferred_type: c.pref, owner_id: c.owner })
    }
    fn child_categories(&self, id: &Uuid) -> Option<Vec<Uuid>> {
        self.cat_children.get(id).cloned()
    }
    fn item(&self, id: &Uuid) -> Option<ItemInfo> {
        self.items.get(id).map(|i| ItemInfo { complete: i.complete, owner_id: i.owner })
    }
    fn is_object_descendent_of(&self, obj: &Uuid, cat: &Uuid) -> bool {
        if obj == cat {
            return true;
        }
        let mut parent = if let Some(c) = self.cats.get(obj) {
            c.parent
        } else if let Some(i) = self.items.get(obj) {
            i.parent
        } else {
            return false;
        };
        let mut steps = 0;
        loop {
            if parent == NULL_ID {
                return false;
            }
            if parent == *cat {
                return true;
            }
            match self.cats.get(&parent) {
                Some(c) => {
                    steps += 1;
                    if steps > self.cats.len() {
                        return false;
                    }
                    parent = c.parent;
                }
                None => return false,
            }
        }
    }
    fn get_fetching(&mut self, id: &Uuid) -> CategoryFetch {
        let now = self.now;
        self.cats.get_mut(id).map_or(CategoryFetch::None, |c| c.fetch.get_fetching(now))
    }
    fn set_fetching(&mut self, id: &Uuid, state: CategoryFetch) {
        let (now, ais) = (self.now, self.ais_available);
        if let Some(c) = self.cats.get_mut(id) {
            c.fetch.set_fetching(state, now, ais);
        }
    }
    fn set_version(&mut self, id: &Uuid, version: i32) {
        if let Some(c) = self.cats.get_mut(id) {
            c.version = version;
        }
    }
}

// ---------------------------------------------------------------------------
// The AIS reply applier (fetch form of AISUpdate), for the replay model
// ---------------------------------------------------------------------------

fn map_len(v: &Value) -> usize {
    match v {
        Value::Map(m) => m.len(),
        Value::Array(a) => a.len(),
        _ => 0,
    }
}

#[derive(Default)]
struct Upd {
    depth0: i32,
    subset: bool,
    known: BTreeMap<Uuid, i32>,
    cats: BTreeMap<Uuid, MCat>,
    items: BTreeMap<Uuid, MItem>,
    lost: BTreeSet<Uuid>,
    /// `fetch()` calls made while parsing (stale versions) and applying.
    fetches: Vec<(Uuid, i32)>,
}

impl ReplayModel {
    /// Applies a fetch reply as `AISUpdate(update, type, request_body).doUpdate()`
    /// does for the scheduler's view; returns the `fetch(expiry)` calls the
    /// update made, in order.
    pub fn apply_fetch_reply(&mut self, subset: bool, depth: i32, v: &Value) -> Vec<(Uuid, i32)> {
        let mut u = Upd { depth0: depth, subset, ..Default::default() };
        // parseContent
        if has(v, "linked_id") && has(v, "parent_id") {
            self.parse_link(&mut u, v, depth);
        } else if has(v, "item_id") && has(v, "parent_id") {
            self.parse_item(&mut u, v);
        }
        if u.subset {
            if has(v, "_embedded") {
                self.parse_embedded(&mut u, get(v, "_embedded"), depth - 1);
            }
        } else if has(v, "category_id") && has(v, "parent_id") {
            self.parse_category(&mut u, v, depth);
        } else if has(v, "_embedded") {
            self.parse_embedded(&mut u, get(v, "_embedded"), depth);
        }
        // doUpdate
        let cats = std::mem::take(&mut u.cats);
        for (_, c) in cats {
            self.update_category(c);
        }
        if !u.lost.is_empty() {
            let lf = self.find_category_for_type(FT_LOST_AND_FOUND);
            if lf != NULL_ID {
                for id in &u.lost {
                    if let Some(i) = u.items.get_mut(id) {
                        i.parent = lf;
                    }
                }
            }
        }
        let items = std::mem::take(&mut u.items);
        for (_, i) in items {
            self.update_item(i);
        }
        self.apply_version_updates(&mut u, v);
        u.fetches
    }

    /// `parseMeta`'s `_updated_category_versions` and `doUpdate`'s last loop
    /// (FIRE-33455: versions only rise; a -1 refetches with 360 s).
    fn apply_version_updates(&mut self, u: &mut Upd, v: &Value) {
        let Value::Map(m) = get(v, "_updated_category_versions") else { return };
        let mut ucv: BTreeMap<Uuid, i32> = BTreeMap::new();
        for (k, val) in m {
            ucv.insert(vkstorm_llsd::text::uuid_set(k), as_integer(val));
        }
        for (id, version) in ucv {
            let Some(c) = self.cats.get_mut(&id) else { continue };
            if c.version != version {
                if version != -1 {
                    if c.version == -1 || version > c.version {
                        c.version = version;
                    }
                } else {
                    u.fetches.push((id, vkstorm_inventory::fetch::timer::FETCH_LONG_EXPIRY));
                }
            }
        }
    }

    /// A write's reply (no `mFetch`): only the version updates matter to the
    /// scheduler here.
    pub fn apply_write_reply(&mut self, v: &Value) -> Vec<(Uuid, i32)> {
        let mut u = Upd::default();
        self.apply_version_updates(&mut u, v);
        u.fetches
    }

    fn parse_embedded(&mut self, u: &mut Upd, emb: &Value, depth: i32) {
        if let Value::Map(links) = get(emb, "links") {
            for l in links.values() {
                self.parse_link(u, l, depth);
            }
        }
        if let Value::Map(items) = get(emb, "items") {
            for i in items.values() {
                self.parse_item(u, i);
            }
        }
        let item = get(emb, "item");
        if has(item, "item_id") {
            self.parse_item(u, item);
        }
        if let Value::Map(cats) = get(emb, "categories") {
            for c in cats.values() {
                self.parse_category(u, c, depth);
            }
        }
        let cat = get(emb, "category");
        if has(cat, "category_id") {
            self.parse_category(u, cat, depth);
        }
    }

    fn parse_item(&mut self, u: &mut Upd, m: &Value) {
        let id = as_uuid(get(m, "item_id"));
        let mut it = match self.items.get(&id) {
            Some(c) => c.clone(),
            None => MItem { id, parent: NULL_ID, owner: NULL_ID, complete: false },
        };
        it.id = id;
        it.parent = as_uuid(get(m, "parent_id"));
        let perms = get(m, "permissions");
        if has(perms, "owner_id") {
            it.owner = as_uuid(get(perms, "owner_id"));
        }
        it.complete = true;
        if it.parent == NULL_ID {
            u.lost.insert(id);
        }
        u.items.insert(id, it);
    }

    fn parse_link(&mut self, u: &mut Upd, m: &Value, depth: i32) {
        let id = as_uuid(get(m, "item_id"));
        let mut it = match self.items.get(&id) {
            Some(c) => c.clone(),
            None => MItem { id, parent: NULL_ID, owner: NULL_ID, complete: false },
        };
        it.id = id;
        it.parent = as_uuid(get(m, "parent_id"));
        // fetch: default permissions, owned by the agent
        it.owner = self.agent;
        it.complete = true;
        if it.parent == NULL_ID {
            u.lost.insert(id);
        }
        u.items.insert(id, it);
        if has(m, "_embedded") {
            self.parse_embedded(u, get(m, "_embedded"), depth);
        }
    }

    fn parse_category(&mut self, u: &mut Upd, m: &Value, depth: i32) {
        let id = as_uuid(get(m, "category_id"));
        let version = if has(m, "version") { as_integer(get(m, "version")) } else { -1 };
        if let Some(curr) = self.cats.get_mut(&id) {
            if curr.version > -1 && curr.count != -1 && version > -1 && version < curr.version {
                // stale folder (FIRE-33337): unknown again, fetch()
                curr.version = -1;
                u.fetches.push((id, FETCH_DEFAULT_EXPIRY));
                return;
            }
        }
        let mut c = match self.cats.get(&id) {
            Some(curr) => {
                let mut c = curr.clone();
                c.fetch.fetching = CategoryFetch::None;
                c
            }
            None => MCat {
                id,
                parent: NULL_ID,
                pref: -1,
                owner: if has(m, "agent_id") { as_uuid(get(m, "agent_id")) } else { NULL_ID },
                version: -1,
                count: -1,
                fetch: CategoryFetchState::new(self.now),
            },
        };
        c.id = id;
        c.parent = as_uuid(get(m, "parent_id"));
        if has(m, "type_default") {
            c.pref = as_integer(get(m, "type_default")) as i8;
        }
        if has(m, "_embedded") {
            let emb = get(m, "_embedded");
            if has(emb, "categories") && has(emb, "links") && has(emb, "items") {
                let n = map_len(get(emb, "categories")) + map_len(get(emb, "links")) + map_len(get(emb, "items"));
                u.known.insert(id, n as i32);
            } else if has(emb, "links") && (c.pref as i32 == FT_CURRENT_OUTFIT || c.pref as i32 == FT_OUTFIT) {
                u.known.insert(id, map_len(get(emb, "links")) as i32);
            }
        }
        if let Some(&n) = u.known.get(&id) {
            c.count = n;
            if version > -1 && depth >= 0 {
                c.version = version;
            }
        }
        u.cats.insert(id, c);
        if has(m, "_embedded") {
            self.parse_embedded(u, get(m, "_embedded"), depth - 1);
        }
    }
}

/// The reply's ids for the callback contract (as the stand-in reads them).
pub fn reply_of(status: i32, v: Option<&Value>) -> AisReply {
    let mut r = AisReply { status, ..Default::default() };
    let Some(v) = v else { return r };
    if !matches!(v, Value::Map(_)) {
        return r;
    }
    let id = |k: &str| if has(v, k) { Some(as_uuid(get(v, k))) } else { None };
    let arr = |k: &str| {
        if has(v, k) {
            Some(match get(v, k) {
                Value::Array(a) => a.iter().map(as_uuid).collect(),
                _ => Vec::new(),
            })
        } else {
            None
        }
    };
    r.category_id = id("category_id");
    r.item_id = id("item_id");
    r.linked_id = id("linked_id");
    r.created_categories = arr("_created_categories");
    r.created_items = arr("_created_items");
    r.updated_categories = arr("_updated_categories");
    r
}

// ---------------------------------------------------------------------------
// The recording
// ---------------------------------------------------------------------------

pub fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ais/fetch")
}

/// A scenario's compared lines (after `== <id>`).
pub fn scenario_lines(id: &str) -> Vec<String> {
    let p = fixture_dir().join("scenarios").join(format!("{id}.txt"));
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let mut out = Vec::new();
    let mut started = false;
    for l in text.split('\n') {
        if !started {
            if l.starts_with("== ") {
                started = true;
            }
            continue;
        }
        if !l.is_empty() {
            out.push(l.to_owned());
        }
    }
    out
}

/// One recorded request.
#[derive(Clone, Debug)]
pub struct RecWire {
    pub method: String,
    pub target: String,
    pub status: i32,
    /// `head=... body=...` as recorded.
    pub request: String,
    /// `resp=... rtype=...`
    pub tail: String,
    /// The recorded reply, parsed.
    pub value: Option<Value>,
}

pub fn recorded_wires(id: &str, lines: &[String]) -> Vec<RecWire> {
    let mut bodies: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let p = fixture_dir().join("responses").join(format!("{id}.txt"));
    if let Ok(data) = std::fs::read(&p) {
        let mut heads = Vec::new();
        let mut pos = 0;
        while pos < data.len() {
            let end = data[pos..].iter().position(|&b| b == b'\n').map_or(data.len(), |e| pos + e);
            let line = &data[pos..end];
            if line.starts_with(b"--- ") {
                heads.push((pos, end + 1, String::from_utf8_lossy(line).into_owned()));
            }
            pos = end + 1;
        }
        for (k, (_, body_start, head)) in heads.iter().enumerate() {
            let body_end = if k + 1 < heads.len() { heads[k + 1].0 } else { data.len() };
            let n: usize = head.split(' ').nth(2).unwrap().parse().unwrap();
            let mut body = data[*body_start..body_end].to_vec();
            if body.ends_with(b"\n") {
                body.pop();
            }
            bodies.insert(n, body);
        }
    }
    let mut out = Vec::new();
    for l in lines {
        let Some(rest) = l.strip_prefix("wire ") else { continue };
        let (n, rest) = rest.split_once(' ').unwrap();
        let n: usize = n.parse().unwrap();
        let mut f = rest.splitn(3, ' ');
        let method = f.next().unwrap().to_owned();
        let target = f.next().unwrap().to_owned();
        let rest = f.next().unwrap();
        let (status, rest) = rest.split_once(' ').unwrap();
        let status: i32 = status.strip_prefix("status=").unwrap().parse().unwrap();
        let k = rest.rfind(" resp=").unwrap();
        out.push(RecWire {
            method,
            target,
            status,
            request: rest[..k].to_owned(),
            tail: rest[k + 1..].to_owned(),
            value: bodies
                .remove(&n)
                .and_then(|b| vkstorm_llsd::parse(vkstorm_llsd::Format::Xml, &b, Default::default()).1),
        });
    }
    out
}

/// `escape_text` of the recorder.
pub fn escape_text(s: &[u8]) -> String {
    let mut out = String::new();
    for &c in s {
        match c {
            b'\r' => out.push_str("\\r"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(c as char),
            _ => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out
}

/// The AIS coroutine adapter's request head (the transport's header set).
pub fn ais_head(method: &str, target: &str, destination: Option<&str>, body_len: usize) -> String {
    let mut h = format!(
        "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1:{{PORT}}\r\nUser-Agent: Vulkanstorm\r\nAccept-Encoding: deflate, gzip\r\nConnection: keep-alive\r\nKeep-alive: 300\r\n"
    );
    if let Some(d) = destination {
        h.push_str(&format!("Destination: {d}\r\n"));
    }
    h.push_str("Accept: application/llsd+xml\r\nContent-Type: application/llsd+xml\r\nX-SecondLife-UDP-Listen-Port: {UDPPORT}\r\n");
    if body_len > 0 {
        h.push_str(&format!("Content-Length: {body_len}\r\n"));
    }
    h.push_str("\r\n");
    h
}

/// `requestPost`'s request head.
pub fn legacy_head(target: &str, body_len: usize) -> String {
    format!(
        "POST {target} HTTP/1.1\r\nHost: 127.0.0.1:{{PORT}}\r\nUser-Agent: Vulkanstorm\r\nAccept-Encoding: deflate, gzip\r\nConnection: keep-alive\r\nKeep-alive: 300\r\nContent-Type: application/llsd+xml\r\nAccept: application/llsd+xml\r\nContent-Length: {body_len}\r\n\r\n"
    )
}

pub const BASE: &str = "http://127.0.0.1:{PORT}";

pub fn target_of(url: &str) -> &str {
    url.strip_prefix(BASE).unwrap_or(url)
}

// ---------------------------------------------------------------------------
// The run: frames, pool, journal
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Startup {
    None,
    InventorySend,
    InventorySkel,
    InventorySend2,
    InventoryCallbacks,
    WearablesWait,
    Started,
}

/// The recording modes (README "Recording modes").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Seq,
    Phased,
    Lockstep,
    Conc,
}

/// A request of an unordered mode: its phase and line (without the reply's
/// length and digest, which come from the recording).
#[derive(Clone, Debug)]
struct WireRec {
    phase: usize,
    line: String,
}

/// A legacy capability request in flight (a `BGFolderHttpHandler` or
/// `BGItemHttpHandler`).
struct LegacyFlight {
    handler: u64,
    /// A foreground item request (`requestPost(true, ...)`): no handler of
    /// the background fetch.
    foreground: bool,
    folder: bool,
    sent_frame: u64,
    status: i32,
    value: Option<Value>,
}

/// Tickets of commands from callers other than the scheduler.
const EXTERNAL_TICKETS: u64 = 1 << 62;

pub const CAPS_NONE: u32 = 0;
pub const CAPS_AIS: u32 = 1;
pub const CAPS_LEGACY: u32 = 2;
pub const CAPS_ALL: u32 = 3;

/// The scripted budget clock of `llinvfetch_parity_bgf.cpp`.
#[derive(Debug, Clone, Copy)]
pub struct ScriptedClock {
    pub now: f64,
    pub step: f64,
}

impl BudgetClock for ScriptedClock {
    fn total_seconds(&mut self) -> f64 {
        let t = self.now;
        self.now += self.step;
        t
    }
}

struct InFlight {
    ticket: u64,
    req: AisRequest,
    sent_frame: u64,
    status: i32,
    value: Option<Value>,
    subset: bool,
    depth: i32,
    /// LOCKSTEP: arrival number and release order key; held until released.
    seq: usize,
    key: String,
    released: bool,
}

/// A request made by a caller other than the scheduler (writes): its body.
pub struct Run {
    pub id: String,
    pub world: World,
    pub model: ReplayModel,
    pub sched: BackgroundFetch,
    pub env: Env,
    pub clock: ScriptedClock,
    pub startup: Startup,
    // region
    pub region: bool,
    pub caps: u32,
    pub caps_received: bool,
    pub library_cap: bool,
    pub fs_use_ais3_api: bool,
    pub in_second_life: bool,
    pub pool_size: usize,
    pub batch_size: i32,
    // the pool and the wire
    queue: VecDeque<(u64, AisRequest, Vec<u8>)>,
    active: Vec<InFlight>,
    frame_no: u64,
    rec_wires: Vec<RecWire>,
    next_wire: usize,
    pub wires: Vec<String>,
    wire_targets: Vec<(String, String, i32)>,
    tids: Vec<String>,
    next_tid: u64,
    pub journal: Vec<String>,
    pub facts: Vec<String>,
    last_snap: String,
    tokens: BTreeMap<u64, String>,
    next_token: u64,
    pub error: Option<String>,
    /// The `checkStop()` armed for the n-th AIS update (s24).
    pub stop_armed: u32,
    pub stops_thrown: u32,
    /// The cache a warm login loads (s02a, s02b).
    pub cache: Option<CacheFile>,
    /// Fetch replies from the reply generator instead of the recording; in
    /// a SEQ replay each generated status must equal the recorded one.
    pub mock: Option<mock::Mock>,
    pub mode: Mode,
    /// LOCKSTEP: the mock holds every reply until released.
    pub hold: bool,
    /// `PoolSizeAIS` as the scheduler reads it (the pool keeps its size).
    pub pool_setting: u32,
    phases: usize,
    rounds: usize,
    unordered: Vec<WireRec>,
    last_before: i32,
    last_after: i32,
    pub max_in_flight: i32,
    /// The stand-in command layer's capabilities and 403 flag.
    pub ais_caps: AisCaps,
    pub notices: AisNotices,
    /// Commands of other callers (writes, direct `AISAPI` calls): their
    /// callback tokens, by ticket.
    external: BTreeMap<u64, Option<u64>>,
    next_external: u64,
    legacy: Vec<LegacyFlight>,
    /// `EnqueueAISCommand`'s postponed commands (the crate's queue).
    postponed: vkstorm_inventory::fetch::ais::PostponedQueue<(u64, AisRequest, Vec<u8>)>,
    /// CONC: replies come back this many frames after their request (the
    /// mock's `delay_ms`, long against a burst of frames).
    pub reply_delay: u64,
    max_pool: usize,
    /// `CallAfterCategoryFetchStage1` observers waiting: incomplete folders
    /// and the callback's tag.
    stage1: Vec<(Vec<Uuid>, String)>,
    /// The model changed since the observers last ran (`notifyObservers`).
    model_changed: bool,
    /// The mock delays legacy replies (c11's `delay_ms=3000`): they arrive
    /// only while the test settles, never during a burst of frames.
    pub legacy_delay: bool,
    settling: bool,
}

impl Run {
    pub fn new(id: &str, world: World, rec_lines: &[String]) -> Self {
        let model = ReplayModel {
            now: 1000.0,
            agent: world.agent,
            ..Default::default()
        };
        let mut r = Run {
            id: id.to_owned(),
            world,
            model,
            sched: BackgroundFetch::new(),
            env: Env::default(),
            clock: ScriptedClock { now: 1000.0, step: 0.0 },
            startup: Startup::None,
            region: true,
            caps: CAPS_ALL,
            caps_received: true,
            library_cap: true,
            fs_use_ais3_api: true,
            in_second_life: true,
            pool_size: 2,
            batch_size: 20,
            queue: VecDeque::new(),
            active: Vec::new(),
            frame_no: 0,
            rec_wires: recorded_wires(id, rec_lines),
            next_wire: 0,
            wires: Vec::new(),
            wire_targets: Vec::new(),
            tids: Vec::new(),
            next_tid: 1,
            journal: Vec::new(),
            facts: Vec::new(),
            last_snap: String::new(),
            tokens: BTreeMap::new(),
            next_token: 1,
            error: None,
            stop_armed: 0,
            stops_thrown: 0,
            cache: None,
            mock: None,
            mode: Mode::Seq,
            hold: false,
            pool_setting: 2,
            phases: 0,
            rounds: 0,
            unordered: Vec::new(),
            last_before: 0,
            last_after: 0,
            max_in_flight: 0,
            ais_caps: AisCaps::default(),
            notices: AisNotices::default(),
            external: BTreeMap::new(),
            next_external: EXTERNAL_TICKETS,
            legacy: Vec::new(),
            postponed: Default::default(),
            reply_delay: 0,
            max_pool: 0,
            stage1: Vec::new(),
            model_changed: false,
            legacy_delay: false,
            settling: false,
        };
        // The process-wide 403 flag: s04 (index order) raises the alert; every
        // later scenario sees it spent.
        let order = index_order();
        let me = order.iter().position(|s| s == id).expect("scenario in index.txt");
        let s04 = order.iter().position(|s| s == "s04_forbidden").unwrap();
        r.notices.first_depth0_403 = me <= s04;
        r.refresh_env();
        r.note(&format!("step begin {id}"));
        r.snapshot();
        r
    }

    /// Switches a new run to an unordered mode on a pool of `pool`
    /// coroutines, with generated replies.
    pub fn unordered(&mut self, mode: Mode, pool: usize) {
        self.mode = mode;
        self.pool_size = pool;
        self.pool_setting = pool as u32;
        self.hold = mode == Mode::Lockstep;
        if self.mock.is_none() {
            self.mock = Some(mock::Mock::new(&self.world));
        }
        self.refresh_env();
    }

    pub fn refresh_env(&mut self) {
        let ais_caps = if self.region && self.caps & CAPS_AIS != 0 {
            AisCaps {
                inventory: format!("{BASE}/cap/inv"),
                library: if self.library_cap { format!("{BASE}/cap/lib") } else { String::new() },
            }
        } else {
            AisCaps::default()
        };
        let legacy = self.region && self.caps & CAPS_LEGACY != 0;
        let cap = |c: &str| if legacy { format!("{BASE}/cap/{c}") } else { String::new() };
        self.env = Env {
            use_http_inventory: true,
            ais_available: scheduler::ais_available(
                self.region,
                self.fs_use_ais3_api,
                self.in_second_life,
                !ais_caps.inventory.is_empty(),
            ),
            region: self.region,
            capabilities_received: self.caps_received,
            disconnected: self.env.disconnected,
            exiting: false,
            in_second_life: self.in_second_life,
            pool_size_ais: self.pool_setting,
            batch_size_ais3: self.batch_size,
            past_wearables_wait: self.startup > Startup::WearablesWait,
            agent_id: self.world.agent,
            sort_order: 7,
            inventory_cap: !ais_caps.inventory.is_empty(),
            library_cap: !ais_caps.library.is_empty(),
            cap_fid2: cap("fid2"),
            cap_fld2: cap("fld2"),
            cap_fi2: cap("fi2"),
            cap_fl2: cap("fl2"),
        };
        self.model.ais_available = self.env.ais_available;
        self.ais_caps = ais_caps;
    }

    pub fn set_region(&mut self, present: bool, caps: u32, received: bool) {
        self.region = present;
        self.caps = caps;
        self.caps_received = received;
        self.refresh_env();
    }

    pub fn set_disconnected(&mut self, d: bool) {
        self.env.disconnected = d;
        self.refresh_env();
    }

    pub fn set_batch_size(&mut self, n: i32) {
        self.batch_size = n;
        self.refresh_env();
    }

    pub fn set_startup(&mut self, s: Startup) {
        self.startup = s;
        self.refresh_env();
    }

    pub fn note(&mut self, l: &str) {
        self.journal.push(l.to_owned());
    }

    pub fn fact(&mut self, f: &str) {
        self.facts.push(f.to_owned());
    }

    pub fn requests(&self) -> usize {
        self.wires.len()
    }

    pub fn step(&mut self, name: &str) {
        let n = self.requests();
        self.note(&format!("step {name} requests={n}"));
        if self.mode == Mode::Phased {
            self.phases += 1;
            let st = self.sched_text();
            self.note(&format!("sch@step {st}"));
        }
    }

    pub fn sched_text(&self) -> String {
        let s = self.sched.state();
        let q = |v: &[vkstorm_inventory::fetch::QueueEntry]| {
            v.iter().map(|e| format!("{}:{}", uuid_string(&e.id), e.fetch_type as i32)).collect::<Vec<_>>().join(",")
        };
        let ids = |v: &[Uuid]| v.iter().map(uuid_string).collect::<Vec<_>>().join(",");
        let b = |x: bool| if x { "1" } else { "0" };
        format!(
            "act={}{} st={}{}{}{} fc={} ffc={} fq=[{}] iq=[{}] exp=[{}] force=[{}]",
            b(s.bg_active),
            b(s.folder_active),
            b(s.inv_started),
            b(s.lib_started),
            b(s.mkt_started),
            b(s.all_fetched),
            s.fetch_count,
            s.folder_fetch_count,
            q(&s.folder_queue),
            q(&s.item_queue),
            ids(&s.expected),
            ids(&s.forced)
        )
    }

    pub fn snapshot(&mut self) {
        let s = self.sched_text();
        if s != self.last_snap {
            self.last_snap = s.clone();
            self.note(&format!("sch {s}"));
        }
    }

    pub fn token(&mut self, tag: &str) -> u64 {
        let t = self.next_token;
        self.next_token += 1;
        self.tokens.insert(t, tag.to_owned());
        t
    }

    pub fn fresh_tid(&mut self) -> Uuid {
        let t = pid(0x7d000000, self.next_tid);
        self.next_tid += 1;
        t
    }

    // ---- calling the scheduler -------------------------------------------

    pub fn with<R>(&mut self, f: impl FnOnce(&mut BackgroundFetch, &mut Ctx<'_>) -> R) -> R {
        let env = self.env.clone();
        let r = {
            let mut cx = Ctx::new(&mut self.model, &env);
            f(&mut self.sched, &mut cx)
        };
        self.drain();
        r
    }

    pub fn start(&mut self, id: Uuid, recursive: bool) {
        self.with(|s, cx| s.start(cx, id, recursive));
    }
    pub fn schedule_folder(&mut self, id: Uuid, forced: bool) {
        self.with(|s, _| s.schedule_folder_fetch(id, forced));
    }
    pub fn schedule_item(&mut self, id: Uuid, forced: bool) {
        self.with(|s, _| s.schedule_item_fetch(id, forced));
    }
    pub fn find_lost_items(&mut self) {
        self.with(|s, _| s.find_lost_items());
    }

    /// An AISAPI command from outside the scheduler; `body` is the caller's
    /// LLSD body (writes), as recorded.
    pub fn ais(&mut self, cmd: Command, tag: Option<&str>, body: Vec<u8>) {
        let token = tag.map(|t| self.token(t));
        match ais_standin::build(&cmd, &self.ais_caps) {
            Ok(req) => {
                let ticket = self.next_external;
                self.next_external += 1;
                self.external.insert(ticket, token);
                self.enqueue_ais((ticket, req, body));
            }
            // refused: the callback with null, at once
            Err(_) => self.external_callbacks(token, &[NULL_ID]),
        }
    }

    /// `EnqueueAISCommand`: into the pool's pending queue, or postponed.
    fn enqueue_ais(&mut self, c: (u64, AisRequest, Vec<u8>)) {
        let pending = self.queue.len();
        if let Some(c) = self.postponed.enqueue(c, pending) {
            self.queue.push_back(c);
        }
    }

    /// `countPending("AIS")`
    pub fn ais_pending(&self) -> usize {
        self.queue.len()
    }

    fn external_callbacks(&mut self, token: Option<u64>, ids: &[Uuid]) {
        if let Some(t) = token {
            let tag = self.tokens.get(&t).cloned().unwrap_or_default();
            for id in ids {
                self.note(&format!("callback {tag} {}", uuid_string(id)));
            }
        }
    }

    /// A command's completion with `ids`: the scheduler's, or another caller's.
    fn complete_ticket(&mut self, ticket: u64, ids: &[Uuid]) {
        if let Some(token) = self.external.remove(&ticket) {
            self.external_callbacks(token, ids);
            return;
        }
        let env = self.env.clone();
        {
            let mut cx = Ctx::new(&mut self.model, &env);
            self.sched.ais_complete(&mut cx, ticket, ids);
        }
        self.drain();
    }

    /// `LLViewerInventoryCategory::fetch(expiry)`
    pub fn cat_fetch(&mut self, id: Uuid, expiry: i32) -> bool {
        let now = self.model.now;
        let Some(c) = self.model.cats.get_mut(&id) else { return false };
        let version = c.version;
        if !c.fetch.fetch_gate(version, expiry, now) {
            return false;
        }
        match scheduler::fetch_path(&self.env.cap_fid2, self.env.ais_available) {
            FetchPath::Start => self.start(id, false),
            FetchPath::Udp => self.note("msg FetchInventoryDescendents (not modelled)"),
        }
        true
    }

    /// `gInventory.fetchDescendentsOf(id)`
    pub fn fetch_descendents_of(&mut self, id: Uuid) {
        if id != NULL_ID && self.model.cats.contains_key(&id) {
            self.cat_fetch(id, FETCH_DEFAULT_EXPIRY);
        }
    }

    pub fn set_version(&mut self, id: Uuid, v: i32) {
        self.model.cats.get_mut(&id).expect("category").version = v;
    }

    pub fn set_complete(&mut self, id: Uuid, c: bool) {
        self.model.items.get_mut(&id).expect("item").complete = c;
    }

    pub fn clock_advance(&mut self, s: f64) {
        self.model.now += s;
    }

    fn drain(&mut self) {
        for a in self.sched.take_actions() {
            match a {
                Action::IssueAis { ticket, command } => {
                    match ais_standin::build(&ais_standin::from_fetch(&command), &self.ais_caps) {
                        Ok(req) => self.enqueue_ais((ticket, req, Vec::new())),
                        Err(e) => self.fail(format!("the scheduler issued a refused command ({e:?}): {command:?}")),
                    }
                }
                Action::Callback { token } => {
                    let tag = self.tokens.get(&token).cloned().unwrap_or_default();
                    self.note(&format!("callback {tag}"));
                }
                Action::Notify { name } => self.note(&format!("notify {name}")),
                Action::FetchCompleted => self.note("fetch.completed"),
                Action::AddIdle | Action::NotifyObservers => {}
                Action::LegacyPost { handler, folder, url, body, .. } => self.legacy_post(handler, folder, &url, body),
                Action::LegacyPump { .. } => self.legacy_pump(),
            }
        }
    }

    // ---- the legacy capabilities ------------------------------------------

    /// `requestPost`: the request on the wire and its generated reply.
    fn legacy_post(&mut self, handler: u64, folder: bool, url: &str, body: Vec<u8>) {
        let target = target_of(url).to_owned();
        let head = legacy_head(&target, body.len());
        let request = format!("head={} body={}", escape_text(head.as_bytes()), escape_text(&body));
        let v = vkstorm_llsd::parse(vkstorm_llsd::Format::Xml, &body, Default::default()).1.unwrap_or(Value::map());
        let m = self.mock.as_mut().expect("generated replies");
        let (status, value) = if folder { m.legacy_folders(&v) } else { m.legacy_items(&v) };
        let line = format!("POST {target} status={status} {request}");
        self.wires.push(line.clone());
        self.wire_targets.push(("POST".to_owned(), target, status));
        let phase = self.phases;
        self.unordered.push(WireRec { phase, line });
        self.legacy.push(LegacyFlight { handler, foreground: false, folder, sent_frame: self.frame_no, status, value });
    }

    /// A foreground `requestPost` of item bodies (`fetchFromServer`, the item
    /// observer's `fetch_items_from_llsd`): C++ model code, here as
    /// scaffolding; the bodies come from the crate's `fetch::legacy`.
    fn foreground_post(&mut self, cap: &str, body: Vec<u8>) {
        let target = format!("/cap/{cap}");
        let head = legacy_head(&target, body.len());
        let request = format!("head={} body={}", escape_text(head.as_bytes()), escape_text(&body));
        let v = vkstorm_llsd::parse(vkstorm_llsd::Format::Xml, &body, Default::default()).1.unwrap_or(Value::map());
        let (status, value) = self.mock.as_mut().expect("generated replies").legacy_items(&v);
        let line = format!("POST {target} status={status} {request}");
        self.wires.push(line.clone());
        self.wire_targets.push(("POST".to_owned(), target, status));
        let phase = self.phases;
        self.unordered.push(WireRec { phase, line });
        self.legacy.push(LegacyFlight { handler: 0, foreground: true, folder: false, sent_frame: self.frame_no, status, value });
    }

    fn legacy_cap(&self, cap: &str) -> bool {
        match cap {
            "fi2" => !self.env.cap_fi2.is_empty(),
            "fl2" => !self.env.cap_fl2.is_empty(),
            _ => false,
        }
    }

    /// `LLViewerInventoryItem::fetchFromServer()` without AIS.
    pub fn fetch_from_server(&mut self, item: Uuid) {
        let Some(it) = self.model.items.get(&item).cloned() else { return };
        if it.complete {
            return;
        }
        let cap = if self.world.agent != it.owner { "fl2" } else { "fi2" };
        if self.legacy_cap(cap) {
            let body = vkstorm_inventory::fetch::legacy::ItemBody {
                agent_id: Some(self.world.agent),
                items: vec![vkstorm_inventory::fetch::legacy::ItemRequest { item_id: item, owner_id: it.owner }],
            };
            self.foreground_post(cap, body.to_xml());
        }
    }

    /// `LLInventoryFetchItemsObserver::startFetch()` without AIS, then
    /// `fetch_items_from_llsd` (bodies with `cap_name`).
    pub fn items_observer_start_legacy(&mut self, ids: &[Uuid]) {
        let mut inv = Vec::new();
        let mut lib = Vec::new();
        for id in ids {
            if self.model.items.get(id).is_some_and(|i| i.complete) || self.model.cats.contains_key(id) || *id == NULL_ID {
                continue;
            }
            let owner = self.model.items.get(id).map_or(self.world.agent, |i| i.owner);
            let e = Value::Map(
                [(b"item_id".to_vec(), Value::Uuid(*id)), (b"owner_id".to_vec(), Value::Uuid(owner))].into_iter().collect(),
            );
            if owner == self.world.agent {
                inv.push(e);
            } else if owner == self.model.libowner {
                lib.push(e);
            }
        }
        for (cap, name, items) in [("fi2", "FetchInventory2", inv), ("fl2", "FetchLib2", lib)] {
            if items.is_empty() {
                continue;
            }
            if self.legacy_cap(cap) {
                let body = Value::Map(
                    [
                        (b"agent_id".to_vec(), Value::Uuid(self.world.agent)),
                        (b"cap_name".to_vec(), Value::String(name.as_bytes().to_vec())),
                        (b"items".to_vec(), Value::Array(items)),
                    ]
                    .into_iter()
                    .collect(),
                );
                self.foreground_post(cap, vkstorm_inventory::fetch::legacy::llsd_xml(&body));
            } else {
                self.note("msg FetchInventory (not modelled)");
            }
        }
    }

    /// `gInventory.handleResponses(false)` inside `bulkFetch()`: every reply
    /// that has come back, in order, through its handler; then the rest of
    /// `bulkFetch()`.
    fn legacy_pump(&mut self) {
        let frame = self.frame_no;
        let ready = !self.legacy_delay || self.settling;
        let mut k = 0;
        while k < self.legacy.len() {
            if ready && !self.legacy[k].foreground && self.legacy[k].sent_frame + self.reply_delay < frame {
                let f = self.legacy.remove(k);
                self.legacy_completed(f);
            } else {
                k += 1;
            }
        }
        let env = self.env.clone();
        {
            let mut cx = Ctx::new(&mut self.model, &env);
            self.sched.bulk_fetch_after_pump(&mut cx);
        }
        self.drain();
    }

    /// `BGFolderHttpHandler::onCompleted` / `BGItemHttpHandler`, then the
    /// handler's destruction.
    fn legacy_completed(&mut self, f: LegacyFlight) {
        let env = self.env.clone();
        self.model_changed = true;
        if f.foreground {
            if let Some(Value::Array(items)) = f.value.as_ref().map(|v| get(v, "items").clone()) {
                for it in &items {
                    self.model.legacy_item(it, None);
                }
            }
            return;
        }
        if f.folder {
            let is_map = matches!(f.value, Some(Value::Map(_)));
            if f.status != 200 {
                let mut cx = Ctx::new(&mut self.model, &env);
                self.sched.legacy_http_failure(&mut cx, f.handler, f.status);
            } else if !is_map || has(f.value.as_ref().unwrap(), "error") {
                let mut cx = Ctx::new(&mut self.model, &env);
                self.sched.legacy_reply_failure(&mut cx, f.handler);
            } else {
                self.legacy_process_data(f.handler, f.value.as_ref().unwrap());
                let mut cx = Ctx::new(&mut self.model, &env);
                self.sched.legacy_data_done(&mut cx);
            }
            self.drain();
        } else if f.status == 200 {
            // FetchItemHttpHandler::processData: the items into the model
            if let Some(Value::Array(items)) = f.value.as_ref().map(|v| get(v, "items").clone()) {
                for it in &items {
                    self.model.legacy_item(it, None);
                }
            }
        }
        self.sched.legacy_handler_released(f.handler);
    }

    /// `processData()`'s model writes (scaffolding for the C++ applier) with
    /// the scheduler's part: a recursive category goes back to the queue.
    fn legacy_process_data(&mut self, handler: u64, v: &Value) {
        let Value::Array(folders) = get(v, "folders").clone() else { return };
        for f in &folders {
            let parent_id = as_uuid(get(f, "folder_id"));
            let owner_id = as_uuid(get(f, "owner_id"));
            let version = as_integer(get(f, "version"));
            let descendents = as_integer(get(f, "descendents"));
            if parent_id == NULL_ID {
                if let Value::Array(items) = get(f, "items") {
                    for it in items {
                        let lost = self.model.find_category_for_type(FT_LOST_AND_FOUND);
                        if lost != NULL_ID {
                            self.model.account_for_update(&lost, 1);
                            self.model.legacy_item(it, Some(lost));
                            self.note("msg MoveInventoryItem (not modelled)");
                        }
                    }
                }
            }
            if !self.model.cats.contains_key(&parent_id) {
                continue;
            }
            if let Value::Array(cats) = get(f, "categories") {
                // one category object, reused across the array
                let mut t = MCat {
                    id: NULL_ID,
                    parent: NULL_ID,
                    pref: -1,
                    owner: owner_id,
                    version: -1,
                    count: -1,
                    fetch: CategoryFetchState::new(self.model.now),
                };
                for c in cats {
                    t.id = as_uuid(get(c, "category_id"));
                    t.parent = as_uuid(get(c, "parent_id"));
                    t.pref = as_integer(get(c, "type_default")) as i8;
                    if self.sched.legacy_is_recursive(handler, &t.id) {
                        self.sched.add_request_at_back(t.id, true, true);
                    } else if !self.model.is_category_complete(&t.id) {
                        self.model.update_category_pub(t.clone());
                    }
                }
            }
            if let Value::Array(items) = get(f, "items") {
                for it in items {
                    self.model.legacy_item(it, None);
                }
            }
            if let Some(c) = self.model.cats.get_mut(&parent_id) {
                c.version = version;
                c.count = descendents;
            }
        }
    }

    // ---- frames ----------------------------------------------------------

    pub fn frame(&mut self, idle: bool) {
        self.frame_no += 1;
        self.last_before = self.sched.state().fetch_count;
        if idle && self.sched.idle_registered() {
            let env = self.env.clone();
            let mut clock = self.clock;
            {
                let mut cx = Ctx::new(&mut self.model, &env);
                self.sched.background_fetch(&mut cx, &mut clock);
            }
            self.clock = clock;
            self.drain();
        }
        // AISAPI::onIdle, registered while commands are postponed
        if !self.postponed.is_empty() {
            let pending = self.queue.len();
            for c in self.postponed.on_idle(pending) {
                self.queue.push_back(c);
            }
        }
        self.last_after = self.sched.state().fetch_count;
        self.max_in_flight = self.max_in_flight.max(self.last_after);
        self.observers();
        self.mainloop();
        self.max_pool = self.max_pool.max(self.pool_count());
        self.snapshot();
    }

    /// `idleNotifyObservers`: a waiting stage-1 observer's `changed()`
    /// (scaffolding: the observers stay C++).
    fn observers(&mut self) {
        if !std::mem::take(&mut self.model_changed) {
            return;
        }
        let fetcher_done = self.sched.is_everything_fetched() && !self.sched.folder_fetch_active();
        let mut k = 0;
        while k < self.stage1.len() {
            let model = &self.model;
            self.stage1[k].0.retain(|c| model.cats.contains_key(c) && !model.is_category_complete(c));
            // LLInventoryFetchDescendentsObserver::changed: also done when
            // the fetcher is done with folders
            if self.stage1[k].0.is_empty() || fetcher_done {
                let (_, tag) = self.stage1.remove(k);
                self.note(&format!("callback {tag}"));
            } else {
                k += 1;
            }
        }
    }

    /// `callAfterCOFFetch` without AIS: the COF's mark cleared, then
    /// `callAfterCategoryFetch` (the stage-1 observer's `startFetch()`).
    pub fn cof_without_ais(&mut self, tag: &str) {
        let cof = self.model.current_outfit_id();
        self.model.set_fetching(&cof, CategoryFetch::None);
        let mut incomplete = Vec::new();
        if let Some(c) = self.model.cats.get(&cof).cloned() {
            if c.version == -1 {
                self.cat_fetch(cof, FETCH_DEFAULT_EXPIRY);
                incomplete.push(cof);
            } else if !self.model.is_category_complete(&cof) {
                self.schedule_folder(cof, true);
                incomplete.push(cof);
            }
        }
        if incomplete.is_empty() {
            self.note(&format!("callback {tag}"));
        } else {
            self.stage1.push((incomplete, tag.to_owned()));
        }
    }

    /// `frame_note`: a frame and its counts.
    pub fn frame_note(&mut self, label: &str) {
        self.frame(true);
        let pool = self.pool_count();
        self.note(&format!(
            "frame {label} count_before={} count_after_idle={} pool={pool}",
            self.last_before, self.last_after
        ));
    }

    /// LOCKSTEP `held()`: the unreleased requests, sorted by method and
    /// target, then arrival.
    pub fn held(&self) -> Vec<usize> {
        let mut h: Vec<(String, usize)> = self
            .active
            .iter()
            .filter(|f| !f.released && f.sent_frame != u64::MAX)
            .map(|f| (f.key.clone(), f.seq))
            .collect();
        h.sort();
        h.into_iter().map(|(_, s)| s).collect()
    }

    /// The held request whose target contains `needle`.
    pub fn held_matching(&self, needle: &str) -> Option<usize> {
        self.active.iter().find(|f| !f.released && f.key.contains(needle)).map(|f| f.seq)
    }

    /// LOCKSTEP `release_one`: the reply processed, then four more frames
    /// (the update's yields and the callback), without the idle callbacks.
    pub fn release_one(&mut self, seq: usize) {
        if let Some(f) = self.active.iter_mut().find(|f| f.seq == seq) {
            f.released = true;
        }
        for _ in 0..5 {
            self.frame(false);
        }
    }

    /// LOCKSTEP `round`: a phase starts, one dispatch frame, then every
    /// held reply released in order.
    pub fn round(&mut self, label: &str) -> bool {
        self.rounds += 1;
        self.phases += 1;
        self.frame(true);
        let (n, d) = (self.rounds, self.last_after - self.last_before);
        let st = self.sched_text();
        self.note(&format!("round {label} {n} dispatched={d} sch={st}"));
        let h = self.held();
        for q in &h {
            self.release_one(*q);
        }
        !h.is_empty()
    }

    /// LOCKSTEP `drain`: rounds until nothing is held or queued.
    pub fn drain_rounds(&mut self, label: &str) {
        for _ in 0..1000 {
            let any = self.round(label);
            let s = self.sched.state();
            if self.error.is_some() || (!any && s.folder_queue.is_empty() && s.item_queue.is_empty() && s.fetch_count == 0) {
                return;
            }
        }
        self.fail(format!("drain {label}: more than 1000 rounds"));
    }

    pub fn journal_count(&self, prefix: &str) -> usize {
        self.journal.iter().filter(|l| l.starts_with(prefix)).count()
    }

    pub fn frames(&mut self, n: usize, idle: bool) {
        for _ in 0..n {
            self.frame(idle);
        }
    }

    fn mainloop(&mut self) {
        let frame = self.frame_no;
        // foreground item requests: their handlers run with the HTTP pump
        let mut k = 0;
        while k < self.legacy.len() {
            if self.legacy[k].foreground && self.legacy[k].sent_frame + self.reply_delay < frame {
                let f = self.legacy.remove(k);
                self.legacy_completed(f);
            } else {
                k += 1;
            }
        }
        let mut k = 0;
        while k < self.active.len() {
            let f = &self.active[k];
            if f.sent_frame + self.reply_delay < frame && f.sent_frame != u64::MAX && (!self.hold || f.released) {
                let f = self.active.remove(k);
                self.process_reply(f);
                // the freed coroutine takes the next queued command
                self.start_queued();
            } else {
                k += 1;
            }
        }
        self.start_queued();
    }

    fn start_queued(&mut self) {
        while self.active.len() < self.pool_size {
            let Some((ticket, req, body)) = self.queue.pop_front() else { break };
            self.send(ticket, req, body);
        }
    }

    fn send(&mut self, ticket: u64, req: AisRequest, body: Vec<u8>) {
        if self.env.disconnected {
            // InvokeAISCommandCoro: gDisconnected, no request
            self.complete_ticket(ticket, &[NULL_ID]);
            return;
        }
        let target = target_of(&req.url).to_owned();
        let method = req.method.as_str();
        let (norm_target, tid) = normalise_tid(&target);
        if let Some(t) = tid {
            self.tids.push(t);
        }
        let head = ais_head(method, &norm_target, req.destination.as_deref(), body.len());
        let request = format!("head={} body={}", escape_text(head.as_bytes()), escape_text(&body));
        let subset = req.command == ais_standin::CommandType::FetchCategorySubset;
        let depth = req.depth.unwrap_or(ais_standin::MAX_FOLDER_DEPTH_REQUEST);
        if self.mode != Mode::Seq {
            // replies from the generator; the transport's 503 retries
            let mut attempts = 0;
            let (status, value) = loop {
                let (st, v) = self.mock.as_mut().expect("generated replies").ais(method, &norm_target);
                attempts += 1;
                let seq = self.wires.len() + 1;
                self.wires.push(format!("{method} {norm_target} status={st} {request}"));
                self.wire_targets.push((method.to_owned(), norm_target.clone(), st));
                let phase = self.phases;
                self.unordered.push(WireRec { phase, line: format!("{method} {norm_target} status={st} {request}") });
                let _ = seq;
                if !(st == 503 && attempts < 6) {
                    break (st, v);
                }
            };
            let seq = self.wires.len();
            self.active.push(InFlight {
                ticket,
                req: req.clone(),
                sent_frame: self.frame_no,
                status,
                value,
                subset,
                depth,
                seq,
                key: format!("{method} {norm_target}"),
                released: false,
            });
            return;
        }
        let mut attempts = 0;
        let mut last: Option<RecWire> = None;
        loop {
            let Some(w) = self.rec_wires.get(self.next_wire).cloned() else {
                if last.is_none() {
                    self.fail(format!("request {} not recorded: {method} {norm_target}", self.wires.len() + 1));
                }
                break;
            };
            let same = w.method == method && w.target == norm_target && w.request == request;
            if last.is_some() && !same {
                break;
            }
            if !same {
                self.fail(format!(
                    "request {} differs:\n  expected {} {} {}\n  actual   {method} {norm_target} {request}",
                    self.wires.len() + 1,
                    w.method,
                    w.target,
                    w.request
                ));
                return;
            }
            self.next_wire += 1;
            attempts += 1;
            self.wires.push(format!(
                "wire {} {method} {norm_target} status={} {request} {}",
                self.wires.len() + 1,
                w.status,
                w.tail
            ));
            self.wire_targets.push((method.to_owned(), norm_target.clone(), w.status));
            let mut w = w;
            if method == "GET" {
                if let Some(m) = self.mock.as_mut() {
                    let (st, v) = m.ais(method, &norm_target);
                    if st != w.status {
                        let n = self.wires.len();
                        self.fail(format!("request {n}: generated status {st}, recorded {}", w.status));
                    }
                    w.value = v;
                }
            }
            let retry = w.status == 503 && attempts < 6;
            last = Some(w);
            if !retry {
                break;
            }
        }
        let Some(w) = last else { return };
        self.active.push(InFlight {
            ticket,
            req: req.clone(),
            sent_frame: self.frame_no,
            status: w.status,
            value: w.value,
            subset,
            depth,
            seq: self.wires.len(),
            key: format!("{method} {norm_target}"),
            released: false,
        });
    }

    fn fail(&mut self, e: String) {
        if self.error.is_none() {
            self.error = Some(e);
        }
    }

    fn process_reply(&mut self, f: InFlight) {
        if let Some(name) = self.notices.on_reply(&f.req, f.status) {
            self.note(&format!("notify {name}"));
        }
        let value = f.value.clone();
        let is_fetch = (f.req.command as i32) >= 9;
        if self.stop_armed > 0 {
            self.stop_armed -= 1;
            if self.stop_armed == 0 {
                // checkStop() throws at the update's first yield: nothing
                // applied, no callback
                self.stops_thrown += 1;
                self.sched.ais_abandoned(f.ticket);
                // the pool coroutine is gone for good: its slot stays taken
                self.active.push(InFlight { sent_frame: u64::MAX, ..f });
                return;
            }
        }
        self.model_changed = true;
        let mut fetches = Vec::new();
        if let Some(v) = &value {
            fetches = if is_fetch {
                self.model.apply_fetch_reply(f.subset, f.depth, v)
            } else {
                self.model.apply_write_reply(v)
            };
        }
        for (id, exp) in fetches {
            self.cat_fetch(id, exp);
        }
        let reply = reply_of(f.status, value.as_ref());
        let ids = ais_standin::callback_ids(f.req.command, &reply);
        self.complete_ticket(f.ticket, &ids);
    }

    /// A request C++ model code makes outside the fetch code (the
    /// `CreateInventoryCategory` coroutine of s14): consumed from the
    /// recording as an input, so that the request counts stay aligned. Its
    /// line is copied, not built.
    pub fn foreign_request(&mut self) {
        let w = self.rec_wires[self.next_wire].clone();
        self.next_wire += 1;
        self.wires.push(format!(
            "wire {} {} {} status={} {} {}",
            self.wires.len() + 1,
            w.method,
            w.target,
            w.status,
            w.request,
            w.tail
        ));
        self.wire_targets.push((w.method.clone(), w.target.clone(), w.status));
    }

    pub fn pool_count(&self) -> usize {
        self.queue.len() + self.active.len() + self.postponed.len()
    }

    fn idle_now(&self) -> bool {
        self.pool_count() == 0 && self.sched.state().fetch_count == 0
    }

    /// `settle(idle, queues_empty)`: frames until nothing is in flight and
    /// nothing changes for 8 frames.
    pub fn settle(&mut self, idle: bool, queues_empty: bool) {
        self.settle_ex(idle, queues_empty, true);
    }

    pub fn settle_ex(&mut self, idle: bool, queues_empty: bool, fetch_idle: bool) {
        self.settling = true;
        self.settle_inner(idle, queues_empty, fetch_idle);
        self.settling = false;
    }

    fn settle_inner(&mut self, idle: bool, queues_empty: bool, fetch_idle: bool) {
        let mut stable = 0;
        let mut last = String::new();
        for _ in 0..100_000 {
            self.frame(idle);
            if self.error.is_some() {
                return;
            }
            let mut ok = self.pool_count() == 0 && (!fetch_idle || self.sched.state().fetch_count == 0);
            if ok && queues_empty {
                let s = self.sched.state();
                ok = s.folder_queue.is_empty() && s.item_queue.is_empty();
            }
            let now = format!("{}|{}|{}", self.last_snap, self.wires.len(), self.journal.len());
            if ok && now == last {
                stable += 1;
                if stable >= 8 {
                    return;
                }
            } else {
                stable = 0;
            }
            last = now;
        }
        self.fail("settle did not end".into());
    }

    pub fn until(&mut self, mut pred: impl FnMut(&Run) -> bool) {
        for _ in 0..100_000 {
            self.frame(true);
            if self.error.is_some() || pred(self) {
                return;
            }
        }
        self.fail("until did not end".into());
    }

    pub fn idle(&self) -> bool {
        self.idle_now()
    }

    pub fn msg_count(&self) -> usize {
        self.journal.iter().filter(|l| l.starts_with("msg ")).count()
    }

    /// The request lines without their `wire <n> ` prefix.
    pub fn wire_bodies(&self) -> Vec<String> {
        self.wires.iter().map(|l| l.splitn(3, ' ').nth(2).unwrap().to_owned()).collect()
    }

    /// `d2_wait`: frames without capabilities; nothing may be sent.
    pub fn d2_wait(&mut self, name: &str, frames: usize) {
        let (req0, msg0, q0) = (self.requests(), self.msg_count(), self.sched_text());
        self.frames(frames, true);
        let (req1, msg1, q1) = (self.requests(), self.msg_count(), self.sched_text());
        self.fact(&format!(
            "d2 wait {name} frames={frames} requests={} msgs={} queue_retained={}",
            req1 - req0,
            msg1 - msg0,
            if q0 == q1 { "yes" } else { "NO" }
        ));
        self.step(name);
    }

    /// `same_as_s01`: the request log against s01's.
    pub fn same_as_s01(&mut self) {
        let s01: Vec<String> = scenario_lines("s01_login_cold")
            .into_iter()
            .filter_map(|l| l.strip_prefix("wire ").map(|r| r.splitn(2, ' ').nth(1).unwrap().to_owned()))
            .collect();
        let w = self.wire_bodies();
        let verdict = if w.len() != s01.len() {
            format!("NO ({} requests, s01 {})", w.len(), s01.len())
        } else if let Some(i) = (0..w.len()).find(|&i| w[i] != s01[i]) {
            format!("NO (request {} differs)", i + 1)
        } else {
            "yes".to_owned()
        };
        self.fact(&format!("d2 same_requests_as s01_login_cold: {verdict}"));
    }

    /// `LLInventoryFetchItemsObserver::startFetch()` with AIS (scaffolding:
    /// the observer stays C++).
    pub fn items_observer_start(&mut self, ids: &[Uuid]) {
        let mut requests: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
        for id in ids {
            if self.model.items.get(id).is_some_and(|i| i.complete) {
                continue;
            }
            if self.model.cats.contains_key(id) || *id == NULL_ID {
                continue;
            }
            match self.model.items.get(id) {
                Some(i) => requests.entry(i.parent).or_default().push(*id),
                None => self.schedule_item(*id, false),
            }
        }
        for (folder, items) in requests {
            match self.model.cats.get(&folder).cloned() {
                Some(c) => {
                    let viewer_count = self.model.cat_children.get(&folder).map_or(0, Vec::len)
                        + self.model.item_children.get(&folder).map_or(0, Vec::len);
                    if c.version == -1 {
                        self.cat_fetch(folder, FETCH_DEFAULT_EXPIRY);
                    } else if items.len() > 7 {
                        self.schedule_folder(folder, true);
                    } else if viewer_count <= items.len() || (c.count as i64) <= items.len() as i64 {
                        self.schedule_folder(folder, true);
                    } else {
                        for i in items {
                            self.schedule_item(i, false);
                        }
                    }
                }
                None => {
                    for i in items {
                        self.schedule_item(i, false);
                    }
                }
            }
        }
    }

    // ---- the login steps (frozen copies of llstartup.cpp) -------------------

    pub fn login(&mut self) {
        self.model.root = self.world.root;
        self.set_startup(Startup::InventorySend);
        self.inventory_send();
        self.set_startup(Startup::InventorySkel);
        self.inventory_skel();
        self.snapshot();
        self.set_startup(Startup::InventorySend2);
        self.inventory_send2();
        self.snapshot();
        self.set_startup(Startup::InventoryCallbacks);
        self.inventory_callbacks();
        self.step("login");
    }

    pub fn inventory_send(&mut self) {
        if self.world.libroot != NULL_ID {
            self.model.libroot = self.world.libroot;
            self.model.libowner = self.world.libowner;
        }
    }

    pub fn inventory_skel(&mut self) {
        let w = self.world.clone();
        match self.cache.take() {
            Some(c) => self.model.load_warm(&w, &c),
            None => self.model.load_cold(&w),
        }
    }

    pub fn inventory_send2(&mut self) {
        self.model.build_parent_child_map();
        if !self.model.usable {
            self.note("notify InventoryUnusable");
        }
        self.start(NULL_ID, true);
    }

    pub fn inventory_callbacks(&mut self) {
        self.start(NULL_ID, true);
        let cof = self.model.current_outfit_id();
        if self.model.cats.get(&cof).is_some_and(|c| c.version == -1) {
            self.model.set_fetching(&cof, CategoryFetch::Recursive);
        }
    }

    /// STATE_WEARABLES_WAIT: `callAfterCOFFetch` with AIS, pumped without the
    /// idle callbacks.
    pub fn cof(&mut self, base_outfit: Uuid) {
        self.set_startup(Startup::WearablesWait);
        self.model.base_outfit = base_outfit;
        let t = self.token("cof");
        self.with(|s, cx| s.fetch_cof(cx, Some(t)));
        self.settle(false, false);
        self.step("cof");
    }

    pub fn walk(&mut self) {
        self.set_startup(Startup::Started);
        self.settle(true, true);
        self.step("walk");
    }

    pub fn public_state(&self) -> String {
        let b = |x: bool| if x { "1" } else { "0" };
        let s = &self.sched;
        let m = &self.model;
        format!(
            "folder_active={} everything={} bulk_complete={} inv_started={} inv_completed={} inv_in_progress={} lib_started={} lib_completed={} lib_in_progress={}",
            b(s.folder_fetch_active()),
            b(s.is_everything_fetched()),
            b(s.is_bulk_fetch_processing_complete()),
            b(s.inventory_fetch_started()),
            b(s.inventory_fetch_completed(m)),
            b(s.inventory_fetch_in_progress(m)),
            b(s.library_fetch_started()),
            b(s.library_fetch_completed(m)),
            b(s.library_fetch_in_progress(m)),
        )
    }

    /// The compared lines, in the recorder's order.
    pub fn finish(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        let mut caps: BTreeMap<String, usize> = BTreeMap::new();
        let mut methods: BTreeMap<String, usize> = BTreeMap::new();
        let mut s503 = 0;
        for (m, t, st) in &self.wire_targets {
            let cap = match t.strip_prefix("/cap/") {
                Some(r) => r.split(['/', '?']).next().unwrap().to_owned(),
                None => "other".to_owned(),
            };
            *caps.entry(cap).or_default() += 1;
            *methods.entry(m.clone()).or_default() += 1;
            if *st == 503 {
                s503 += 1;
            }
        }
        let j = |m: &BTreeMap<String, usize>| m.iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>().join(",");
        out.push(format!(
            "count requests={} caps={} methods={} status503={}",
            self.wires.len(),
            j(&caps),
            j(&methods),
            s503
        ));
        for f in &self.facts {
            out.push(format!("fact {f}"));
        }
        match self.mode {
            Mode::Seq => out.extend(self.wires.iter().cloned()),
            Mode::Phased | Mode::Lockstep => {
                let mut tails = recorded_tails(&self.id);
                let mut recs = self.unordered.clone();
                for r in &mut recs {
                    let tail = tails
                        .get_mut(&(Some(r.phase), r.line.clone()))
                        .and_then(|v| v.pop())
                        .unwrap_or_else(|| "resp=?".to_owned());
                    r.line = format!("{} {tail}", r.line);
                }
                let mut lines: Vec<(usize, String)> = recs.into_iter().map(|r| (r.phase, r.line)).collect();
                lines.sort();
                out.extend(lines.into_iter().map(|(p, l)| format!("wire@{p} {l}")));
            }
            Mode::Conc => {
                let mut tails = recorded_tails(&self.id);
                let mut ms: BTreeMap<String, usize> = BTreeMap::new();
                for r in &self.unordered {
                    let tail = tails
                        .get_mut(&(None, r.line.clone()))
                        .and_then(|v| v.last().cloned())
                        .unwrap_or_else(|| "resp=?".to_owned());
                    *ms.entry(format!("{} {tail}", r.line)).or_default() += 1;
                }
                out.extend(ms.into_iter().map(|(l, n)| format!("wire* {n}x {l}")));
            }
        }
        let distinct: BTreeSet<&String> = self.tids.iter().collect();
        out.push(format!(
            "tids requests={} distinct={} fresh={}",
            self.tids.len(),
            distinct.len(),
            if self.tids.len() == distinct.len() { "yes" } else { "NO" }
        ));
        if self.mode == Mode::Conc {
            let mut ms: BTreeMap<&String, usize> = BTreeMap::new();
            for l in &self.journal {
                if l.starts_with("msg ") || l.starts_with("notify ") || l.starts_with("callback ") {
                    *ms.entry(l).or_default() += 1;
                }
            }
            out.extend(ms.into_iter().map(|(l, n)| format!("jrn* {n}x {l}")));
        } else if self.mode == Mode::Phased {
            let mut seg: Vec<String> = Vec::new();
            for l in &self.journal {
                if l.starts_with("sch ") {
                    continue;
                }
                if l.starts_with("step ") || l.starts_with("sch@step ") {
                    seg.sort();
                    out.extend(seg.drain(..).map(|l| format!("jrn {l}")));
                    out.push(format!("jrn {l}"));
                } else {
                    seg.push(l.clone());
                }
            }
            seg.sort();
            out.extend(seg.into_iter().map(|l| format!("jrn {l}")));
        } else {
            for l in &self.journal {
                out.push(format!("jrn {l}"));
            }
        }
        out.push(format!("pub {}", self.public_state()));
        if self.mode == Mode::Conc {
            let yn = |b: bool| if b { "yes" } else { "NO" };
            out.push(format!("bound viewer_in_flight<=19 {}", yn(self.max_in_flight <= 19)));
            if !caps.contains_key("inv") && !caps.contains_key("lib") {
                out.push(format!("bound legacy_in_flight<=16 {}", yn(self.max_in_flight <= 16)));
            }
            out.push(format!("bound ais_pool<=20 {}", yn(self.max_pool <= 20)));
        }
        out
    }
}

/// The recorded replies' `resp=... rtype=...` of an unordered scenario, by
/// phase (`None` for CONC) and request line.
fn recorded_tails(id: &str) -> BTreeMap<(Option<usize>, String), Vec<String>> {
    let mut out: BTreeMap<(Option<usize>, String), Vec<String>> = BTreeMap::new();
    for l in scenario_lines(id) {
        let (phase, rest) = if let Some(r) = l.strip_prefix("wire@") {
            let (p, rest) = r.split_once(' ').unwrap();
            (Some(p.parse::<usize>().unwrap()), rest.to_owned())
        } else if let Some(r) = l.strip_prefix("wire* ") {
            (None, r.split_once(' ').unwrap().1.to_owned())
        } else {
            continue;
        };
        let k = rest.rfind(" resp=").unwrap();
        out.entry((phase, rest[..k].to_owned())).or_default().push(rest[k + 1..].to_owned());
    }
    out
}

fn normalise_tid(target: &str) -> (String, Option<String>) {
    if let Some(k) = target.find("tid=") {
        let v = target[k + 4..k + 4 + 36].to_owned();
        (format!("{}tid={{tid}}{}", &target[..k], &target[k + 4 + 36..]), Some(v))
    } else {
        (target.to_owned(), None)
    }
}

pub fn index_order() -> Vec<String> {
    let t = std::fs::read_to_string(fixture_dir().join("index.txt")).unwrap();
    t.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).map(str::to_owned).collect()
}

/// The recorded lines a replay compares: everything but the C++ model's own
/// output (`batch*`, `market*`, `msg`, model dump, scenario notes) and the
/// lines a scenario names as left to the viewer test.
pub fn compared(lines: &[String], skip: &dyn Fn(&str) -> bool) -> Vec<String> {
    lines
        .iter()
        .filter(|l| {
            let l = l.as_str();
            if l.starts_with("mdl ") || l.starts_with("mdl-") || l.starts_with('#') {
                return false;
            }
            if let Some(j) = l.strip_prefix("jrn* ") {
                // CONC: the order-free journal; UDP messages are C++ model code
                let line = j.split_once(' ').map_or("", |x| x.1);
                return (line.starts_with("notify ") || line.starts_with("callback ")) && !skip(l);
            }
            if l.starts_with("jrn-") {
                // large: digests of the whole journal, observer batches included
                return false;
            }
            if let Some(j) = l.strip_prefix("jrn ") {
                let keep = j.starts_with("step ")
                    || j.starts_with("sch ")
                    || j.starts_with("sch@step ")
                    || j == "fetch.completed"
                    || j.starts_with("callback ")
                    || j.starts_with("notify ")
                    || j.starts_with("frame ")
                    || j.starts_with("round ");
                if !keep {
                    return false;
                }
            }
            !skip(l)
        })
        .cloned()
        .collect()
}

/// Compares and reports: `(compared, total)` line counts on success.
pub fn check(id: &str, run: &mut Run, skip: &dyn Fn(&str) -> bool) -> (usize, usize) {
    let lines = scenario_lines(id);
    let expected = compared(&lines, skip);
    let actual: Vec<String> = compared(&run.finish(), skip);
    let mut first = None;
    for i in 0..expected.len().max(actual.len()) {
        if expected.get(i) != actual.get(i) {
            first = Some(i);
            break;
        }
    }
    if let Some(i) = first {
        let cut = |s: Option<&String>| s.map_or("<none>".to_owned(), |s| s.chars().take(700).collect());
        let ctx_from = i.saturating_sub(3);
        let mut msg = format!(
            "{id}: line {} of {} compared lines differs\n  expected: {}\n  actual:   {}\n",
            i + 1,
            expected.len(),
            cut(expected.get(i)),
            cut(actual.get(i))
        );
        for k in ctx_from..i {
            msg.push_str(&format!("  ctx {}: {}\n", k + 1, cut(expected.get(k))));
        }
        if let Some(e) = &run.error {
            msg.push_str(&format!("  replay error: {e}\n"));
        }
        panic!("{msg}");
    }
    if let Some(e) = &run.error {
        panic!("{id}: replay error: {e}");
    }
    (expected.len(), lines.len())
}

/// The fetch state of every folder in the recorded model dump (`mdl cat ...
/// version= count= ... fetching= timer{...}`) against the replay model: the
/// version and descendent count (the replay model's AIS applier) and the
/// fetch state and request timer (the scheduler's `setFetching` calls
/// through `fetch::timer`). Returns `(checked, mismatches)`.
pub fn check_model_fetch_state(id: &str, run: &Run) -> (usize, Vec<String>) {
    let mut checked = 0;
    let mut bad = Vec::new();
    for l in scenario_lines(id) {
        let Some(rest) = l.strip_prefix("mdl cat ") else { continue };
        let cid = parse_uuid(&rest[..36]);
        let field = |k: &str| -> String {
            let a = rest.find(&format!(" {k}=")).unwrap() + k.len() + 2;
            rest[a..].split(' ').next().unwrap().to_owned()
        };
        let version: i32 = field("version").parse().unwrap();
        let count: i32 = field("count").parse().unwrap();
        // CONC dumps leave out the fetch state and timer (which reply touched
        // a folder last depends on timing)
        let has_state = rest.contains(" fetching=");
        let fetching: i32 = if has_state { field("fetching").parse().unwrap() } else { -1 };
        let t = rest.find("timer{").map(|a| rest[a..].to_owned()).unwrap_or_default();
        checked += 1;
        let Some(c) = run.model.cats.get(&cid) else {
            bad.push(format!("{}: not in the replay model", uuid_string(&cid)));
            continue;
        };
        let tm = &c.fetch.requested;
        let mine_t = format!(
            "timer{{started={} expired={} to_expire={:.3}}}",
            tm.started as i32,
            tm.has_expired(run.model.now) as i32,
            (tm.expiry - run.model.now) as f32
        );
        let state_differs = has_state && (c.fetch.fetching as i32 != fetching || mine_t != t);
        if c.version != version || c.count != count || state_differs {
            bad.push(format!(
                "{}: recorded version={version} count={count} fetching={fetching} {t}, replay version={} count={} fetching={} {mine_t}",
                uuid_string(&cid),
                c.version,
                c.count,
                c.fetch.fetching as i32
            ));
        }
    }
    (checked, bad)
}
