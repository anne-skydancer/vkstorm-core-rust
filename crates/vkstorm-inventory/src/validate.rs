//! `LLInventoryModel::validate()` and `LLInventoryValidationInfo`
//! (`newview/llinventorymodel.{h,cpp}`): the integrity checks run at login
//! (from `buildParentChildMap`) and on demand.
//!
//! The model containers stay C++. [`Snapshot`] is what `validate()` reads
//! of them, flat: the folder and item records (the fields the checks use),
//! the two maps by key, the parent-to-children vectors, the backlink
//! multimap, the set locks and the root ids. Object identity (the C++
//! compares pointers) is the record index, so a map entry and a vector entry
//! are the same object when they hold the same index. The C++ wrapper builds
//! the snapshot, calls [`validate`] and copies the [`ValidationInfo`] back.
//!
//! `getItem` keeps a one-entry cache (`mLastItem`) that answers for an id
//! even after the item left the map; `validate()` reads and moves it, so the
//! snapshot carries it in and [`Validation::last_item`] carries it out.
//!
//! `QAModeFakeSystemFolderIssues` draws from a function-static
//! `std::default_random_engine`, which is `mt19937` in MSVC's standard
//! library and `minstd_rand0` in libstdc++, through
//! `std::uniform_int_distribution<>(0, 1)`, whose algorithm also differs by
//! library. [`QaRandom`] reproduces both (the R2 C4 runtime edge, open
//! decision 4); the engine's state carries over between calls, so the
//! caller keeps one for the process. The crate's default
//! ([`QaRandom::default`]) is MSVC's, the library of the committed
//! recording, as R1 settled its runtime values on one platform
//! (`RT(...;crate=16)`); [`QaRandom::native`] picks the build's own.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use vkstorm_llsd::Value;

use crate::llsd::coerce::insert;
use crate::perms::{Uuid, NULL_ID};
use crate::types::folder::BAD_LOOKUP;
use crate::types::{AssetType, FolderType};

/// A folder as `validate()` reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapCategory {
    pub id: Uuid,
    pub parent: Uuid,
    pub preferred_type: FolderType,
    /// `getVersion()`.
    pub version: i32,
    /// `getDescendentCount()`.
    pub descendent_count: i32,
}

/// An item as `validate()` reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapItem {
    pub id: Uuid,
    pub parent: Uuid,
    /// `getActualType()` (the stored type).
    pub asset_type: AssetType,
    /// The stored asset id (`getLinkedUUID()` of a link).
    pub asset: Uuid,
}

impl SnapItem {
    fn is_link(&self) -> bool {
        self.asset_type.is_link_type()
    }
    fn linked_id(&self) -> Uuid {
        if self.is_link() { self.asset } else { self.id }
    }
}

/// The model as `validate()` reads it. Indices refer to `categories` and
/// `items`; `None` is a null pointer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// `getRootFolderID()`.
    pub root: Uuid,
    /// `getLibraryRootFolderID()`.
    pub library_root: Uuid,
    /// The folder records.
    pub categories: Vec<SnapCategory>,
    /// The item records.
    pub items: Vec<SnapItem>,
    /// `mCategoryMap`: key to folder (a null pointer is a `null_cat`).
    pub category_map: BTreeMap<Uuid, Option<usize>>,
    /// `mItemMap`: key to item.
    pub item_map: BTreeMap<Uuid, usize>,
    /// `mParentChildCategoryTree`: parent key to the child folder vector.
    pub category_tree: BTreeMap<Uuid, Vec<Option<usize>>>,
    /// `mParentChildItemTree`: parent key to the child item vector.
    pub item_tree: BTreeMap<Uuid, Vec<Option<usize>>>,
    /// `mBacklinkMMap`: (target, link) pairs.
    pub backlinks: Vec<(Uuid, Uuid)>,
    /// The keys of `mCategoryLock` set to true.
    pub category_locks: BTreeSet<Uuid>,
    /// The keys of `mItemLock` set to true.
    pub item_locks: BTreeSet<Uuid>,
    /// `mLastItem` (an item index), the `getItem` cache.
    pub last_item: Option<usize>,
}

/// `LLInventoryValidationInfo`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValidationInfo {
    pub initialized: bool,
    pub warning_count: i32,
    pub warnings: BTreeMap<String, u32>,
    pub loop_count: i32,
    pub orphaned_count: i32,
    pub fatal_error_count: i32,
    pub fatal_no_root_folder: bool,
    pub fatal_system_duplicate: i32,
    pub fatal_no_library_root_folder: bool,
    pub fatal_qa_debug_mode: bool,
    /// `std::set<LLFolderType::EType>`: by value.
    pub missing_required_system_folders: BTreeSet<i32>,
    pub duplicate_required_system_folders: BTreeSet<i32>,
    /// `mLog` (`<FS:Beq>` FIRE-31634): each entry ends with a newline.
    pub log: String,
}

impl ValidationInfo {
    /// `asLLSD`. The warnings start as an empty array and the first warning
    /// written by key turns it into a map (`LLSD::operator[]`).
    pub fn to_llsd(&self) -> Value {
        let mut sd = Value::map();
        insert(&mut sd, "fatal_error_count", Value::Integer(self.fatal_error_count));
        insert(&mut sd, "loop_count", Value::Integer(self.loop_count));
        insert(&mut sd, "orphaned_count", Value::Integer(self.orphaned_count));
        insert(&mut sd, "initialized", Value::Boolean(self.initialized));
        insert(
            &mut sd,
            "missing_system_folders_count",
            Value::Integer(self.missing_required_system_folders.len() as i32),
        );
        insert(&mut sd, "fatal_no_root_folder", Value::Boolean(self.fatal_no_root_folder));
        insert(&mut sd, "fatal_no_library_root_folder", Value::Boolean(self.fatal_no_library_root_folder));
        insert(&mut sd, "fatal_qa_debug_mode", Value::Boolean(self.fatal_qa_debug_mode));
        insert(&mut sd, "warning_count", Value::Integer(self.warning_count));
        if self.warning_count > 0 {
            let mut warnings = Value::array();
            for (k, &v) in &self.warnings {
                let val = v as i32;
                if val > 0 {
                    if !matches!(warnings, Value::Map(_)) {
                        warnings = Value::map();
                    }
                    insert(&mut warnings, k, Value::Integer(val));
                }
            }
            insert(&mut sd, "warnings", warnings);
        }
        let names = |set: &BTreeSet<i32>| {
            Value::Array(set.iter().map(|&ft| Value::String(FolderType(ft).lookup().as_bytes().to_vec())).collect())
        };
        if !self.missing_required_system_folders.is_empty() {
            insert(&mut sd, "missing_system_folders", names(&self.missing_required_system_folders));
        }
        insert(
            &mut sd,
            "duplicate_system_folders_count",
            Value::Integer(self.duplicate_required_system_folders.len() as i32),
        );
        if !self.duplicate_required_system_folders.is_empty() {
            insert(&mut sd, "duplicate_system_folders", names(&self.duplicate_required_system_folders));
        }
        sd
    }
}

/// Which standard library's `std::default_random_engine` and
/// `std::uniform_int_distribution` the QA draws follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QaLibrary {
    /// MSVC: `mt19937` (default seed 5489); the distribution maps a draw to
    /// its top bit (Lemire's multiply for a range of two).
    Msvc,
    /// libstdc++: `minstd_rand0` (default seed 1); the distribution's
    /// downscaling with rejection.
    Libstdcxx,
}

/// The function-static engine of `validate()`'s QA mode, default
/// constructed, and its `uniform_int_distribution<>(0, 1)`.
#[derive(Clone, Debug)]
pub struct QaRandom {
    library: QaLibrary,
    mt: Option<Box<Mt19937>>,
    minstd: u32,
}

impl Default for QaRandom {
    /// MSVC's engine, the library of the committed recording.
    fn default() -> Self {
        QaRandom::new(QaLibrary::Msvc)
    }
}

impl QaRandom {
    /// A default-constructed engine of `library`.
    pub fn new(library: QaLibrary) -> QaRandom {
        match library {
            QaLibrary::Msvc => QaRandom { library, mt: Some(Box::new(Mt19937::new(5489))), minstd: 0 },
            QaLibrary::Libstdcxx => QaRandom { library, mt: None, minstd: 1 },
        }
    }

    /// The engine of the standard library this crate is built with (MSVC
    /// for a `target_env = "msvc"` build, else libstdc++), as the viewer
    /// linked beside it would draw.
    pub fn native() -> QaRandom {
        if cfg!(target_env = "msvc") { QaRandom::new(QaLibrary::Msvc) } else { QaRandom::new(QaLibrary::Libstdcxx) }
    }

    /// The library this engine follows.
    pub fn library(&self) -> QaLibrary {
        self.library
    }

    /// `distrib(e)`: 0 or 1.
    pub fn draw(&mut self) -> i32 {
        match self.library {
            QaLibrary::Msvc => {
                let x = self.mt.as_mut().map(|m| m.next()).unwrap_or(0);
                (((u64::from(x)) * 2) >> 32) as i32
            }
            QaLibrary::Libstdcxx => {
                // urngrange = (2^31 - 2) - 1, urange = 1: scaling and the
                // rejection bound of the downscaling branch.
                const URNG_MIN: u32 = 1;
                const URNG_RANGE: u32 = 2_147_483_646 - 1;
                const SCALING: u32 = URNG_RANGE / 2;
                const PAST: u32 = 2 * SCALING;
                loop {
                    self.minstd = ((u64::from(self.minstd) * 16807) % 2_147_483_647) as u32;
                    let ret = self.minstd - URNG_MIN;
                    if ret < PAST {
                        return (ret / SCALING) as i32;
                    }
                }
            }
        }
    }
}

/// `std::mt19937`.
#[derive(Clone, Debug)]
struct Mt19937 {
    state: [u32; 624],
    index: usize,
}

impl Mt19937 {
    fn new(seed: u32) -> Mt19937 {
        let mut state = [0u32; 624];
        state[0] = seed;
        for i in 1..624 {
            state[i] = 1_812_433_253u32.wrapping_mul(state[i - 1] ^ (state[i - 1] >> 30)).wrapping_add(i as u32);
        }
        Mt19937 { state, index: 624 }
    }

    fn next(&mut self) -> u32 {
        if self.index >= 624 {
            for i in 0..624 {
                let y = (self.state[i] & 0x8000_0000) | (self.state[(i + 1) % 624] & 0x7fff_ffff);
                let mut v = self.state[(i + 397) % 624] ^ (y >> 1);
                if y & 1 != 0 {
                    v ^= 0x9908_b0df;
                }
                self.state[i] = v;
            }
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }
}

/// Options of a `validate()` call.
#[derive(Debug, Default)]
pub struct Options<'a> {
    /// `QAModeFakeSystemFolderIssues` set: the engine to draw from.
    pub qa: Option<&'a mut QaRandom>,
    /// OpenSim builds (`--features opensim`): the grid is not Second Life,
    /// so suitcase folders and their descendants are not checked
    /// (`isInSuitcase`, `<FS:Beq>` FIRE-31674). Ignored without the
    /// feature, as the C++ compiles it out.
    pub in_open_sim: bool,
}

/// What `validate()` returns and leaves behind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validation {
    pub info: ValidationInfo,
    /// `mLastItem` afterwards.
    pub last_item: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ancestor {
    Ok(Uuid),
    Missing,
    Loop,
}

#[derive(Clone, Copy)]
enum Obj {
    Cat(usize),
    Item(usize),
}

struct Model<'a> {
    s: &'a Snapshot,
    last_item: Option<usize>,
}

impl Model<'_> {
    fn cat(&self, i: usize) -> &SnapCategory {
        &self.s.categories[i]
    }

    fn item(&self, i: usize) -> &SnapItem {
        &self.s.items[i]
    }

    /// `getItem`, with `mLastItem`.
    fn get_item(&mut self, id: &Uuid) -> Option<usize> {
        if let Some(last) = self.last_item {
            if self.s.items[last].id == *id {
                return Some(last);
            }
        }
        let found = self.s.item_map.get(id).copied();
        if found.is_some() {
            self.last_item = found;
        }
        found
    }

    /// `getCategory`.
    fn get_category(&self, id: &Uuid) -> Option<usize> {
        self.s.category_map.get(id).copied().flatten()
    }

    /// `getObject`: the folder, else the item.
    fn get_object(&mut self, id: &Uuid) -> Option<Obj> {
        if let Some(c) = self.get_category(id) {
            return Some(Obj::Cat(c));
        }
        self.get_item(id).map(Obj::Item)
    }

    fn parent_of(&self, o: Obj) -> Uuid {
        match o {
            Obj::Cat(c) => self.cat(c).parent,
            Obj::Item(i) => self.item(i).parent,
        }
    }

    fn id_of(&self, o: Obj) -> Uuid {
        match o {
            Obj::Cat(c) => self.cat(c).id,
            Obj::Item(i) => self.item(i).id,
        }
    }

    /// `getObjectTopmostAncestor`.
    fn topmost_ancestor(&mut self, object_id: &Uuid) -> Ancestor {
        let Some(mut object) = self.get_object(object_id) else {
            return Ancestor::Missing;
        };
        let mut seen: BTreeSet<Uuid> = BTreeSet::new();
        seen.insert(*object_id);
        loop {
            let parent = self.parent_of(object);
            if parent == NULL_ID {
                break;
            }
            if !seen.insert(parent) {
                return Ancestor::Loop;
            }
            match self.get_object(&parent) {
                Some(p) => object = p,
                None => return Ancestor::Missing,
            }
        }
        Ancestor::Ok(self.id_of(object))
    }

    fn has_backlink(&self, link: &Uuid, target: &Uuid) -> bool {
        self.s.backlinks.iter().any(|(t, l)| t == target && l == link)
    }

    /// `isInSuitcase` (OpenSim only).
    #[cfg(feature = "opensim")]
    fn in_suitcase(&self, cat: usize) -> bool {
        let mut c = self.cat(cat);
        let mut seen: BTreeSet<Uuid> = BTreeSet::new();
        seen.insert(c.id);
        if c.preferred_type == FolderType::MY_SUITCASE {
            return true;
        }
        while c.parent != NULL_ID {
            let parent = c.parent;
            if !seen.insert(parent) {
                return false;
            }
            let Some(p) = self.get_category(&parent) else {
                return false;
            };
            if self.cat(p).preferred_type == FolderType::MY_SUITCASE {
                return true;
            }
            c = self.cat(p);
        }
        false
    }
}

/// `validate()` over a snapshot.
pub fn validate(snapshot: &Snapshot, options: Options<'_>) -> Validation {
    let Options { mut qa, in_open_sim } = options;
    let skip_suitcase = cfg!(feature = "opensim") && in_open_sim;
    let s = snapshot;
    let mut m = Model { s, last_item: s.last_item };
    let mut info = ValidationInfo::default();
    let mut fatal_errs = 0;
    let mut warning_count = 0;
    let mut loop_count = 0;
    let mut orphaned_count = 0;
    let warn = |info: &mut ValidationInfo, key: &str, count: &mut i32| {
        *info.warnings.entry(key.to_owned()).or_insert(0) += 1;
        *count += 1;
    };

    if s.root == NULL_ID {
        info.fatal_no_root_folder = true;
        fatal_errs += 1;
    }
    if s.library_root == NULL_ID {
        info.log.push_str("No library root folder id\n");
        info.fatal_no_library_root_folder = true;
        fatal_errs += 1;
    }
    if s.category_map.len() + 1 != s.category_tree.len() {
        warn(&mut info, "category_map_size", &mut warning_count);
    }

    let mut under_root: BTreeMap<i32, i32> = BTreeMap::new();
    let mut elsewhere: BTreeMap<i32, i32> = BTreeMap::new();

    for (cat_id, &slot) in &s.category_map {
        let Some(ci) = slot else {
            warn(&mut info, "null_cat", &mut warning_count);
            continue;
        };
        let res = m.topmost_ancestor(cat_id);
        let topmost = match res {
            Ancestor::Missing => {
                orphaned_count += 1;
                NULL_ID
            }
            Ancestor::Loop => {
                loop_count += 1;
                NULL_ID
            }
            Ancestor::Ok(id) => id,
        };
        let cat = m.cat(ci).clone();
        if *cat_id != cat.id {
            warn(&mut info, "cat_id_index_mismatch", &mut warning_count);
        }
        if cat.parent == NULL_ID && *cat_id != s.root && *cat_id != s.library_root {
            warn(&mut info, "null_parent", &mut warning_count);
        }
        #[cfg(feature = "opensim")]
        if skip_suitcase && m.in_suitcase(ci) {
            continue;
        }
        let (Some(cats), Some(items)) = (s.category_tree.get(cat_id), s.item_tree.get(cat_id)) else {
            warn(&mut info, "direct_descendents", &mut warning_count);
            continue;
        };
        if cat.descendent_count != crate::model::DESCENDENT_COUNT_UNKNOWN
            && (cats.len() + items.len()) as i64 != i64::from(cat.descendent_count)
            && (topmost == NULL_ID || topmost != s.library_root)
        {
            warn(&mut info, "invalid_descendent_count", &mut warning_count);
        }
        for entry in items {
            let Some(ii) = *entry else {
                warn(&mut info, "null_item_at_index", &mut warning_count);
                continue;
            };
            let item_id = m.item(ii).id;
            if m.item(ii).parent != *cat_id {
                warn(&mut info, "wrong_parent_for_item", &mut warning_count);
            }
            if !s.item_map.contains_key(&item_id) {
                warn(&mut info, "item_not_in_top_map", &mut warning_count);
            }
            match m.topmost_ancestor(&item_id) {
                Ancestor::Ok(top) => {
                    if top != s.root && top != s.library_root {
                        warn(&mut info, "topmost_ancestor_not_recognized", &mut warning_count);
                    }
                }
                _ => warn(&mut info, "topmost_ancestor_not_found", &mut warning_count),
            }
        }
        if cat.parent != NULL_ID {
            match s.category_tree.get(&cat.parent) {
                None => orphaned_count += 1,
                Some(siblings) => {
                    if !siblings.iter().any(|e| *e == Some(ci)) {
                        orphaned_count += 1;
                    }
                }
            }
        }
        let in_library = matches!(m.topmost_ancestor(&cat.id), Ancestor::Ok(top) if top == s.library_root);
        if !in_library {
            #[cfg(feature = "opensim")]
            if skip_suitcase && m.in_suitcase(ci) {
                continue;
            }
            if s.root != NULL_ID && (cat.id == s.root || cat.parent == s.root) {
                *under_root.entry(cat.preferred_type.0).or_insert(0) += 1;
            } else {
                *elsewhere.entry(cat.preferred_type.0).or_insert(0) += 1;
            }
        }
    }
    let _ = skip_suitcase;

    for (item_id, &ii) in &s.item_map {
        let item = m.item(ii).clone();
        if item.id != *item_id {
            warn(&mut info, "item_id_mismatch", &mut warning_count);
        }
        if item.parent == NULL_ID {
            orphaned_count += 1;
        } else {
            match s.item_tree.get(&item.parent) {
                None => orphaned_count += 1,
                Some(siblings) => {
                    if !siblings.iter().any(|e| *e == Some(ii)) {
                        orphaned_count += 1;
                    }
                }
            }
        }
        if item.is_link() {
            let link_id = item.id;
            let target_id = item.linked_id();
            let target_item = m.get_item(&target_id);
            let target_cat = m.get_category(&target_id);
            if !m.has_backlink(&link_id, &target_id) {
                orphaned_count += 1;
            }
            if item.asset_type == AssetType::LINK && target_item.is_none() {
                orphaned_count += 1;
            } else if item.asset_type == AssetType::LINK_FOLDER && target_cat.is_none() {
                orphaned_count += 1;
            }
        } else {
            // The backlinks of a non-link item are only logged, but each
            // lookup moves mLastItem.
            let target_id = item.id;
            let links: Vec<Uuid> = s.backlinks.iter().filter(|(t, _)| *t == target_id).map(|(_, l)| *l).collect();
            for link_id in links {
                m.get_item(&link_id);
            }
        }
    }

    // System folders.
    for ft in FolderType::TEXTURE.0..FolderType::COUNT.0 {
        let folder_type = FolderType(ft);
        if folder_type.lookup() == BAD_LOOKUP || folder_type == FolderType::MY_SUITCASE {
            continue;
        }
        let is_automatic = folder_type.is_automatic();
        let is_singleton = folder_type.is_singleton();
        let mut count_under_root = under_root.get(&ft).copied().unwrap_or(0);
        let mut count_elsewhere = elsewhere.get(&ft).copied().unwrap_or(0);
        if let Some(rng) = qa.as_deref_mut() {
            count_under_root = 2 * rng.draw();
            count_elsewhere = 2 * rng.draw();
            info.fatal_qa_debug_mode = true;
        }
        if !is_singleton {
            continue;
        }
        if count_under_root == 0 {
            if is_automatic {
                let _ = writeln!(info.log, "Cannot create system folder of type {}({})", folder_type.lookup(), ft);
                info.missing_required_system_folders.insert(ft);
                fatal_errs += 1;
            } else {
                warn(&mut info, "missing_system_folder_can_create", &mut warning_count);
            }
        } else if count_under_root > 1 {
            info.duplicate_required_system_folders.insert(ft);
            if !is_automatic && folder_type != FolderType::SETTINGS && folder_type != FolderType::MATERIAL {
                let _ = writeln!(
                    info.log,
                    "System folder type has excess copies under root, type {}({}) count {}",
                    folder_type.lookup(),
                    ft,
                    count_under_root
                );
                info.fatal_system_duplicate += 1;
                fatal_errs += 1;
            } else {
                warn(&mut info, "non_fatal_system_duplicate_under_root", &mut warning_count);
            }
        }
        if count_elsewhere > 0 {
            warn(&mut info, "non_fatal_system_duplicate_elsewhere", &mut warning_count);
        }
    }

    info.fatal_error_count = fatal_errs;
    info.warning_count = warning_count;
    info.loop_count = loop_count;
    info.orphaned_count = orphaned_count;
    Validation { info, last_item: m.last_item }
}
