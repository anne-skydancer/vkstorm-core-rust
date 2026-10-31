//! The login load and discard rules of `LLInventoryModel::loadSkeleton`
//! (`newview/llinventorymodel.cpp`): the login response's folder skeleton
//! reconciled with the cache file.
//!
//! The C++ keeps the file-system steps and the model; this module decides.
//! A login load runs as:
//!
//! 1. [`purge_names`]: when the `<agent>_DELETE_INV_GZ` marker is a file,
//!    the C++ removes each of the listed files that is a file, then the
//!    marker (`<FS:Zi>`; checked on every call, so the library's call, the
//!    first of the login, purges both caches before anything is read, bug
//!    27).
//! 2. [`Skeleton::import`]: the skeleton entries as the C++ walker extracts
//!    them (port rule 1: `isDefined` and the C++ `LLSD` coercions), into the
//!    folders of the skeleton (UUID order, the first entry of an id wins) and
//!    `loadSkeleton`'s return value.
//! 3. When [`Skeleton::wants_cache`], the C++ gunzips `<name>.gz` into the
//!    plain name (a temporary name in a second instance) with llcommon's
//!    `gunzip_file` and reads the plain file's bytes, giving a
//!    [`CacheInput`].
//! 4. [`Skeleton::load`] reconciles them over the model as it is
//!    ([`ModelLookup`]), returning a [`LoadPlan`]: the folders to add with
//!    their final version, count, thumbnail and favorite, the cached items
//!    to add in order, the model folders to invalidate, and which files to
//!    remove. The C++ executor calls `addCategory` for each folder (which
//!    skips `FT_MESH` and localizes the name), `addItem` for each item, sets
//!    the invalidated versions, and removes the files.
//!
//! The rules, as the C++ applies them (survey 2 sections 2.4, 2.5; the C2
//! corpus enumerates them):
//! - a skeleton folder keeps its version only when the cache holds it with
//!   the same version and no item of it has an unknown type (survey 2 Q10);
//!   a kept folder takes the cached thumbnail and favorite;
//! - a cached folder the skeleton does not have is ignored; a skeleton
//!   folder the cache does not have, and every folder when the cache is
//!   missing, obsolete or does not gunzip (bug 20), has an unknown version;
//! - a cached item is added when its parent is a folder of the model with a
//!   known version; a link whose target is not in the model yet is retried
//!   after every other item, and a link still broken then invalidates its
//!   folder (the COF cascade);
//! - each kept folder's descendent count becomes its skeleton child folders
//!   plus its added items (so a root saved without its items reloads as
//!   complete, survey 2 Q8);
//! - the plain file is removed after a gunzip (successful or not) and in a
//!   second instance; an obsolete or corrupt `.gz` is removed, except in a
//!   second instance.

use std::collections::{BTreeMap, BTreeSet};

use vkstorm_llsd::Value;

use crate::cache::{self, Decoded};
use crate::category::{VERSION_UNKNOWN, ViewerCategory};
use crate::item::Item;
use crate::llsd::coerce::{as_integer, as_string, as_uuid, get};
use crate::model::{DESCENDENT_COUNT_UNKNOWN, ModelLookup, is_broken_link, viewer_item_type};
use crate::perms::{NULL_ID, Uuid};
use crate::types::{AssetType, FolderType};

/// The cache files the clear-cache marker purges, in the C++ order, and the
/// marker itself (removed last). All are names in the cache directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PurgeNames {
    /// The agent's plain and `.gz` cache, then the library owner's.
    pub files: [Vec<u8>; 4],
    /// `<agent>_DELETE_INV_GZ`.
    pub marker: Vec<u8>,
}

/// The names [`PurgeNames`] lists for an agent and a library owner (null
/// when the login gave none) on a grid (see [`cache::cache_file_name`]).
pub fn purge_names(agent: &Uuid, library_owner: &Uuid, grid: Option<&[u8]>) -> PurgeNames {
    let gz = |mut n: Vec<u8>| {
        n.extend_from_slice(b".gz");
        n
    };
    let a = cache::cache_file_name(agent, grid);
    let l = cache::cache_file_name(library_owner, grid);
    PurgeNames {
        files: [a.clone(), gz(a), l.clone(), gz(l)],
        marker: cache::delete_marker_name(agent),
    }
}

/// One entry of the login response's skeleton (`inventory-skeleton` or
/// `inventory-skel-lib`), as the C++ walker extracts it: `None` for a key
/// that is not defined, else the value through `asString`, `asUUID` or
/// `asInteger`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkeletonEntry {
    pub name: Option<Vec<u8>>,
    pub folder_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub version: Option<i32>,
    pub type_default: Option<i32>,
}

impl SkeletonEntry {
    /// The extraction over a `vkstorm_llsd::Value` with this crate's
    /// coercions ([`crate::llsd::coerce`]). The viewer extracts with the C++
    /// `LLSD` coercions instead (port rule 1); they differ only on the
    /// runtime edge the R1 README lists (`"0x10"` as an integer).
    pub fn from_llsd(sd: &Value) -> SkeletonEntry {
        let defined = |k: &str| {
            let v = get(sd, k);
            if matches!(v, Value::Undefined) {
                None
            } else {
                Some(v)
            }
        };
        SkeletonEntry {
            name: defined("name").map(as_string),
            folder_id: defined("folder_id").map(as_uuid),
            parent_id: defined("parent_id").map(as_uuid),
            version: defined("version").map(as_integer),
            type_default: defined("type_default").map(as_integer),
        }
    }

    /// The entries of a skeleton array (`beginArray`/`endArray`: none for a
    /// value that is not an array).
    pub fn list_from_llsd(skeleton: &Value) -> Vec<SkeletonEntry> {
        match skeleton {
            Value::Array(a) => a.iter().map(SkeletonEntry::from_llsd).collect(),
            _ => Vec::new(),
        }
    }
}

/// The folders of a skeleton, before the cache is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skeleton {
    /// The owner the folders are made for (`LLViewerInventoryCategory(owner_id)`).
    pub owner: Uuid,
    /// `temp_cats`: one folder per id, in UUID order.
    pub folders: BTreeMap<Uuid, ViewerCategory>,
    /// `loadSkeleton`'s return value: false when an entry lacked a key or
    /// had a null folder id ("Unable to import near ...").
    pub rv: bool,
    /// The entries that were not imported (each logs "Unable to import near
    /// {name}"), by index into the entry list, in order.
    pub rejected: Vec<usize>,
}

impl Skeleton {
    /// The import loop: an entry with `name`, `folder_id`, `parent_id` and
    /// `version` defined and a non-null folder id becomes a folder of the
    /// owner, renamed (`rename`: `correctInventoryName`, the empty result
    /// keeps the empty name), with the parent, the preferred type
    /// (`type_default`, else `FT_NONE`) and the version. A later entry of
    /// the same id is dropped (`std::set::insert`).
    pub fn import(owner: &Uuid, entries: &[SkeletonEntry]) -> Skeleton {
        let mut folders = BTreeMap::new();
        let mut rv = true;
        let mut rejected = Vec::new();
        for (index, e) in entries.iter().enumerate() {
            match (&e.name, e.folder_id, e.parent_id, e.version) {
                (Some(name), Some(id), Some(parent), Some(version)) if id != NULL_ID => {
                    let mut cat = ViewerCategory {
                        owner: *owner,
                        ..ViewerCategory::default()
                    };
                    cat.category.object.rename(name);
                    cat.category.object.id = id;
                    cat.category.object.parent = parent;
                    cat.category.preferred_type =
                        FolderType(e.type_default.unwrap_or(FolderType::NONE.0));
                    cat.version = version;
                    folders.entry(id).or_insert(cat);
                }
                _ => {
                    rv = false;
                    rejected.push(index);
                }
            }
        }
        Skeleton {
            owner: *owner,
            folders,
            rv,
            rejected,
        }
    }

    /// Whether `loadSkeleton` reads the cache: only when the skeleton has a
    /// folder. Otherwise no file is touched and [`load`](Self::load) adds
    /// nothing.
    pub fn wants_cache(&self) -> bool {
        !self.folders.is_empty()
    }

    /// The reconciliation, over the cache as the C++ found it and the model
    /// as it is before this call (the library's folders and items, when the
    /// agent's skeleton loads second).
    pub fn load(
        self,
        cache: &CacheInput<'_>,
        in_open_sim: bool,
        lookup: &dyn ModelLookup,
    ) -> LoadPlan {
        let mut plan = LoadPlan {
            rv: self.rv,
            ..LoadPlan::default()
        };
        if self.folders.is_empty() {
            return plan;
        }
        let mut is_cache_obsolete = false;
        if cache.gz_present && !cache.gunzip_ok {
            is_cache_obsolete = true;
        }
        let decoded: Option<Decoded> = if is_cache_obsolete {
            None
        } else {
            cache
                .file
                .map(|bytes| cache::decode(bytes, in_open_sim, lookup))
        };
        let loaded = match &decoded {
            Some(d) => {
                is_cache_obsolete = d.obsolete;
                plan.cache_obsolete = d.obsolete;
                plan.null_id_names = d.null_id_names.clone();
                d.rv()
            }
            None => false,
        };
        plan.remove_plain = cache.second_instance || cache.gz_present;
        plan.remove_gz = is_cache_obsolete && !cache.second_instance;

        let mut temp = self.folders;
        if !loaded {
            plan.folders = temp
                .into_values()
                .map(|mut folder| {
                    folder.version = VERSION_UNKNOWN;
                    LoginFolder {
                        folder,
                        descendent_count: DESCENDENT_COUNT_UNKNOWN,
                    }
                })
                .collect();
            return plan;
        }
        let decoded = decoded.unwrap_or_default();

        // Cached folders against the skeleton's.
        let mut cached_ids: BTreeSet<Uuid> = BTreeSet::new();
        for cat in &decoded.categories {
            let id = cat.category.object.id;
            let Some(tcat) = temp.get_mut(&id) else {
                continue; // in the cache, not in the skeleton
            };
            if decoded.cats_to_update.contains(&id) {
                plan.folders_to_update.push(id);
                tcat.version = VERSION_UNKNOWN;
            }
            if cat.version != tcat.version {
                tcat.version = VERSION_UNKNOWN;
            } else {
                cached_ids.insert(id);
                tcat.category.object.thumbnail = cat.category.object.thumbnail;
                tcat.category.object.favorite = cat.category.object.favorite;
            }
        }
        plan.cached_category_count = cached_ids.len();

        // addCategory for every skeleton folder; the model's folders as the
        // item loop sees them.
        let mut model = Overlay::new(lookup);
        let mut child_counts: BTreeMap<Uuid, i32> = BTreeMap::new();
        for (id, tcat) in temp.iter_mut() {
            if !cached_ids.contains(id) {
                tcat.version = VERSION_UNKNOWN;
            }
            if tcat.category.preferred_type != FolderType::MESH {
                model.categories.insert(*id);
            }
            *child_counts.entry(tcat.category.object.parent).or_insert(0) += 1;
        }

        // The cached items whose folder is in the model with a known version.
        let items = decoded.items;
        let mut added: Vec<usize> = Vec::new();
        let mut possibly_broken: Vec<usize> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let parent = item.object.parent;
            let Some(version) = model.version(&parent, &temp) else {
                continue;
            };
            if version == VERSION_UNKNOWN {
                continue;
            }
            if is_broken_link(item, &model.view(&temp)) {
                plan.possibly_broken.push(item.clone());
                possibly_broken.push(index);
                continue;
            }
            if item.is_link_type() {
                plan.good_link_count += 1;
            }
            model.add_item(item, &temp);
            added.push(index);
            plan.cached_item_count += 1;
            *child_counts.entry(parent).or_insert(0) += 1;
        }
        let mut invalid: BTreeSet<Uuid> = BTreeSet::new();
        for &index in &possibly_broken {
            let item = &items[index];
            let parent = item.object.parent;
            if is_broken_link(item, &model.view(&temp)) {
                plan.bad_link_count += 1;
                invalid.insert(parent);
            } else {
                plan.recovered_link_count += 1;
                model.add_item(item, &temp);
                added.push(index);
                plan.cached_item_count += 1;
                *child_counts.entry(parent).or_insert(0) += 1;
            }
        }
        plan.invalidated = invalid.iter().copied().collect();
        for id in &invalid {
            if model.categories.contains(id) {
                if let Some(t) = temp.get_mut(id) {
                    t.version = VERSION_UNKNOWN;
                }
            } else {
                plan.invalidate.push(*id);
            }
        }

        // The known descendent counts of the kept folders.
        plan.folders = temp
            .into_iter()
            .map(|(id, folder)| {
                let descendent_count = if folder.version != VERSION_UNKNOWN {
                    child_counts.get(&id).copied().unwrap_or(0)
                } else {
                    DESCENDENT_COUNT_UNKNOWN
                };
                LoginFolder {
                    folder,
                    descendent_count,
                }
            })
            .collect();
        let mut slots: Vec<Option<Item>> = items.into_iter().map(Some).collect();
        plan.items = added.into_iter().filter_map(|i| slots[i].take()).collect();
        plan
    }
}

/// The ids a [`Skeleton::load`] over `bytes` can read from the model: the
/// items' parents and the links' targets of the cache. A C ABI caller can
/// resolve these into a [`crate::model::ModelObjects`] before the call
/// instead of answering lookups.
pub fn lookup_ids(bytes: &[u8], in_open_sim: bool) -> BTreeSet<Uuid> {
    let d = cache::decode(bytes, in_open_sim, &crate::model::EmptyModel);
    let mut ids = BTreeSet::new();
    for item in &d.items {
        ids.insert(item.object.parent);
        if item.is_link_type() {
            ids.insert(item.asset);
        }
    }
    ids
}

/// How the C++ found the cache file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheInput<'a> {
    /// `<name>.gz` could be opened.
    pub gz_present: bool,
    /// `gunzip_file` into the plain (or temporary) name succeeded; read only
    /// when `gz_present`.
    pub gunzip_ok: bool,
    /// The bytes of the file `loadFromFile` opens (the plain name, or the
    /// temporary name in a second instance), `None` when it cannot be
    /// opened. Not read after a failed gunzip.
    pub file: Option<&'a [u8]>,
    /// `LLAppViewer::isSecondInstance()`.
    pub second_instance: bool,
}

/// A folder of the skeleton as `loadSkeleton` leaves it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginFolder {
    /// The category: owner, version, name, parent, preferred type, and the
    /// cached thumbnail and favorite when its version was kept.
    pub folder: ViewerCategory,
    /// `mDescendentCount`: set for a folder of known version, else
    /// [`DESCENDENT_COUNT_UNKNOWN`].
    pub descendent_count: i32,
}

/// What `loadSkeleton` does to the model and the cache directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadPlan {
    /// The return value.
    pub rv: bool,
    /// `addCategory` for each, in this order (UUID order). The C++
    /// `addCategory` skips `FT_MESH` folders.
    pub folders: Vec<LoginFolder>,
    /// `addItem` for each, in this order, after the folders.
    pub items: Vec<Item>,
    /// Folders of the model before this call (not of this skeleton) whose
    /// version becomes unknown (a broken link in them), in UUID order.
    pub invalidate: Vec<Uuid>,
    /// Remove the plain (or temporary) file.
    pub remove_plain: bool,
    /// Remove `<name>.gz`.
    pub remove_gz: bool,
    /// The folders whose cached version was kept (the log line's count).
    pub cached_category_count: usize,
    /// The items added (the log line's count).
    pub cached_item_count: usize,
    /// The skeleton folders whose cached items include an unknown type, in
    /// cache file order ("folder to update: {name}").
    pub folders_to_update: Vec<Uuid>,
    /// The cached links the first pass deferred as broken, in order (the
    /// `LL_DEBUGS` "Attempted to add cached link item without baseobj
    /// present" lines).
    pub possibly_broken: Vec<Item>,
    /// The folders invalidated by a link still broken after the second pass,
    /// skeleton and model folders alike, in UUID order ("Invalidating
    /// category name ...").
    pub invalidated: Vec<Uuid>,
    /// Links added in the first pass, links still broken, and links added
    /// in the second (the summary `LL_DEBUGS` line).
    pub good_link_count: usize,
    /// The cache file was read and found obsolete (version, header or
    /// parse; `loadFromFile` logged why).
    pub cache_obsolete: bool,
    /// [`Decoded::null_id_names`] of the file read.
    pub null_id_names: Vec<Vec<u8>>,
    pub bad_link_count: usize,
    pub recovered_link_count: usize,
}

/// The model as the load changes it: the skeleton's folders and the added
/// items over the model before the call.
struct Overlay<'a> {
    base: &'a dyn ModelLookup,
    /// The folder ids under which the model now holds the skeleton's folder
    /// (`addCategory` replaced the model's; `FT_MESH` folders are not added).
    categories: BTreeSet<Uuid>,
    items: BTreeMap<Uuid, AssetType>,
}

impl<'a> Overlay<'a> {
    fn new(base: &'a dyn ModelLookup) -> Self {
        Overlay {
            base,
            categories: BTreeSet::new(),
            items: BTreeMap::new(),
        }
    }

    fn version(&self, id: &Uuid, temp: &BTreeMap<Uuid, ViewerCategory>) -> Option<i32> {
        if self.categories.contains(id) {
            temp.get(id).map(|c| c.version)
        } else {
            self.base.category_version(id)
        }
    }

    /// `addItem`: an item whose viewer type is `AT_NONE` or below is not
    /// added.
    fn add_item(&mut self, item: &Item, temp: &BTreeMap<Uuid, ViewerCategory>) {
        if viewer_item_type(item, &self.view(temp)).0 <= AssetType::NONE.0 {
            return;
        }
        self.items.insert(item.object.id, item.object.asset_type);
    }

    fn view<'b>(&'b self, temp: &'b BTreeMap<Uuid, ViewerCategory>) -> OverlayView<'b> {
        OverlayView {
            overlay: self,
            temp,
        }
    }
}

struct OverlayView<'b> {
    overlay: &'b Overlay<'b>,
    temp: &'b BTreeMap<Uuid, ViewerCategory>,
}

impl ModelLookup for OverlayView<'_> {
    fn item_type(&self, id: &Uuid) -> Option<AssetType> {
        self.overlay
            .items
            .get(id)
            .copied()
            .or_else(|| self.overlay.base.item_type(id))
    }
    fn category_type(&self, id: &Uuid) -> Option<AssetType> {
        if self.overlay.categories.contains(id) {
            self.temp.get(id).map(|c| c.category.object.asset_type)
        } else {
            self.overlay.base.category_type(id)
        }
    }
    fn category_version(&self, id: &Uuid) -> Option<i32> {
        self.overlay.version(id, self.temp)
    }
}
