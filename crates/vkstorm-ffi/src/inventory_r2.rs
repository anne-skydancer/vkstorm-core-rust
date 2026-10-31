//! Rung seven, sub-rung R2: the inventory cache file and the login load of
//! `LLInventoryModel` (`newview/llinventorymodel.cpp`) via
//! `vkstorm-inventory` (`vks_inv_cache_*`, `vks_inv_skeleton_*`).
//!
//! - `loadFromFile`: the bytes of the uncompressed cache file go in whole;
//!   Rust parses the binary LLSD and makes the records (`fromLLSD`,
//!   `importLLSDMap`, the item filter), and they come back as flat
//!   [`VksInvItem`] and [`VksInvCategory`] arrays in a block freed with
//!   `vks_inv_cache_free`. No C++ `LLSD` tree is built.
//! - `saveToFile`: C++ passes flat views of its records in the order it
//!   collected them (since upstream 28fee038e0 the model's child vectors
//!   follow the standard library's `unordered_map` order); Rust writes the
//!   file's bytes in that order into an R1 result block (`text`).
//! - `loadSkeleton`: C++ extracts the skeleton entries with its own `LLSD`
//!   coercions (port rule 1) and keeps the file-system steps (the clear-cache
//!   marker, `gunzip_file`, the removals); Rust reconciles the skeleton with
//!   the cache and returns the plan C++ applies to the model: the folders to
//!   add with their final version and count, the cached items to add in
//!   order, the model folders to invalidate and the files to remove.
//!
//! The model containers stay C++. The rules read them through a
//! [`VksInvModelHost`] callback table (the stored type of an item or folder,
//! a folder's version), called only for the ids a rule needs (a link's
//! target, an item's parent outside the skeleton).

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::ptr;

use vkstorm_inventory::cache::{self, Decoded};
use vkstorm_inventory::category::ViewerCategory;
use vkstorm_inventory::item::Item;
use vkstorm_inventory::login::{CacheInput, LoadPlan, Skeleton, SkeletonEntry};
use vkstorm_inventory::model::ModelLookup;
use vkstorm_inventory::perms::Uuid;
use vkstorm_inventory::types::{AssetType, FolderType};
use vkstorm_inventory::validate::{self, QaRandom, SnapCategory, SnapItem, Snapshot};

use crate::inventory::{
    VksInvBytes, VksInvCategory, VksInvItem, VksInvRecords, bytes, category_from, category_view,
    guard, item_from, item_view, records, uuid_arg,
};

// ---------------------------------------------------------------------------
// The model, read through the host

/// What the rules read from `LLInventoryModel` (`gInventory`). Each callback
/// gets `ctx` and a 16-byte id. A missing callback answers "absent".
#[repr(C)]
pub struct VksInvModelHost {
    pub ctx: *mut c_void,
    /// `getItem(id)`: true with the item's stored type (`getActualType`)
    /// when the model has it.
    pub item_type: Option<unsafe extern "C" fn(*mut c_void, *const u8, *mut i32) -> bool>,
    /// `getCategory(id)`: true with the folder's stored type and version
    /// when the model has it.
    pub category: Option<unsafe extern "C" fn(*mut c_void, *const u8, *mut i32, *mut i32) -> bool>,
}

/// A [`ModelLookup`] over the host (or an empty model for a null host).
struct Host<'a>(Option<&'a VksInvModelHost>);

impl Host<'_> {
    fn category(&self, id: &Uuid) -> Option<(AssetType, i32)> {
        let h = self.0?;
        let f = h.category?;
        let (mut t, mut v) = (0i32, 0i32);
        // SAFETY: the host promises a callable function and context.
        if unsafe { f(h.ctx, id.as_ptr(), &mut t, &mut v) } {
            Some((AssetType(t), v))
        } else {
            None
        }
    }
}

impl ModelLookup for Host<'_> {
    fn item_type(&self, id: &Uuid) -> Option<AssetType> {
        let h = self.0?;
        let f = h.item_type?;
        let mut t = 0i32;
        // SAFETY: the host promises a callable function and context.
        if unsafe { f(h.ctx, id.as_ptr(), &mut t) } {
            Some(AssetType(t))
        } else {
            None
        }
    }
    fn category_type(&self, id: &Uuid) -> Option<AssetType> {
        self.category(id).map(|c| c.0)
    }
    fn category_version(&self, id: &Uuid) -> Option<i32> {
        self.category(id).map(|c| c.1)
    }
}

unsafe fn host<'a>(h: *const VksInvModelHost) -> Host<'a> {
    Host(unsafe { h.as_ref() })
}

// ---------------------------------------------------------------------------
// loadFromFile

/// What `vks_inv_cache_decode` returns. Every pointer points into the block
/// and lives until `vks_inv_cache_free`.
#[repr(C)]
pub struct VksInvCacheView {
    /// `is_cache_obsolete`; `loadFromFile` returns its negation. When set,
    /// there are no records.
    pub obsolete: bool,
    /// The categories, in file order.
    pub categories: *const VksInvCategory,
    pub category_count: usize,
    /// The items, in file order, without those of null id and those whose
    /// viewer type is `AT_UNKNOWN`.
    pub items: *const VksInvItem,
    pub item_count: usize,
    /// `cats_to_update`: the parents of the dropped `AT_UNKNOWN` items, 16
    /// bytes each, in UUID order.
    pub cats_to_update: *const u8,
    pub cats_to_update_count: usize,
}

/// A decoded cache file.
pub struct VksInvCache {
    /// The records the views point into.
    _decoded: Decoded,
    item_views: Vec<VksInvItem>,
    category_views: Vec<VksInvCategory>,
    cats_to_update: Vec<u8>,
    view: VksInvCacheView,
}

/// `loadFromFile` over the bytes of the file it opened: the version header,
/// the binary LLSD and the records. `in_open_sim` is
/// `LLPermissions::getIsInOpenSim()` (the OpenSim branch of `fromLLSD`).
/// The item filter follows links through `host` (`getType()`). Null on a
/// panic (see `vks_inv_last_error`).
///
/// # Safety
/// `data` must point to `len` readable bytes (or be null with `len` 0);
/// `host` must be null or valid for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_cache_decode(
    data: *const u8,
    len: usize,
    in_open_sim: bool,
    model: *const VksInvModelHost,
) -> *mut VksInvCache {
    guard(ptr::null_mut(), || {
        let bytes = unsafe { bytes(data, len) };
        let lookup = unsafe { host(model) };
        let decoded = cache::decode(bytes, in_open_sim, &lookup);
        let item_views: Vec<VksInvItem> = decoded.items.iter().map(item_view).collect();
        let category_views: Vec<VksInvCategory> =
            decoded.categories.iter().map(category_view).collect();
        let cats_to_update: Vec<u8> = decoded.cats_to_update.iter().flatten().copied().collect();
        let mut block = Box::new(VksInvCache {
            view: VksInvCacheView {
                obsolete: decoded.obsolete,
                categories: category_views.as_ptr(),
                category_count: category_views.len(),
                items: item_views.as_ptr(),
                item_count: item_views.len(),
                cats_to_update: cats_to_update.as_ptr(),
                cats_to_update_count: decoded.cats_to_update.len(),
            },
            _decoded: decoded,
            item_views,
            category_views,
            cats_to_update,
        });
        // The vectors moved with their heap buffers; the pointers hold.
        block.view.items = block.item_views.as_ptr();
        block.view.categories = block.category_views.as_ptr();
        block.view.cats_to_update = block.cats_to_update.as_ptr();
        Box::into_raw(block)
    })
}

/// The view of a decoded cache file.
///
/// # Safety
/// `c` must come from `vks_inv_cache_decode` and be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_cache_view(c: *const VksInvCache) -> *const VksInvCacheView {
    if c.is_null() {
        ptr::null()
    } else {
        unsafe { &(*c).view }
    }
}

/// Releases a decoded cache file. Null is ignored.
///
/// # Safety
/// `c` must come from `vks_inv_cache_decode` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_cache_free(c: *mut VksInvCache) {
    if !c.is_null() {
        drop(unsafe { Box::from_raw(c) });
    }
}

// ---------------------------------------------------------------------------
// saveToFile

/// `saveToFile`'s bytes: the version, the binary LLSD of the categories
/// whose version is known (`exportLLSD`, with `owner` and `version`) and of
/// every item (`asLLSD`), in the order given, and the newline. The bytes are
/// the block's `text`. Null on a panic.
///
/// # Safety
/// `categories` and `items` must point to `category_count` and `item_count`
/// views (or be null with a count of 0) whose strings are readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_cache_encode(
    categories: *const VksInvCategory,
    category_count: usize,
    items: *const VksInvItem,
    item_count: usize,
) -> *mut VksInvRecords {
    records(|| {
        let cats: Vec<ViewerCategory> = unsafe { views(categories, category_count) }
            .iter()
            .map(|c| unsafe { category_from(c) })
            .collect();
        let items: Vec<Item> = unsafe { views(items, item_count) }
            .iter()
            .map(|i| unsafe { item_from(i) })
            .collect();
        Some(VksInvRecords::text(cache::encode(&cats, &items)))
    })
}

unsafe fn views<'a, T>(p: *const T, n: usize) -> &'a [T] {
    if p.is_null() || n == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(p, n) }
    }
}

// ---------------------------------------------------------------------------
// loadSkeleton

/// Which keys of a [`VksInvSkeletonEntry`] are defined (`isDefined`).
pub const VKS_INV_SKEL_NAME: u32 = 1;
pub const VKS_INV_SKEL_FOLDER_ID: u32 = 2;
pub const VKS_INV_SKEL_PARENT_ID: u32 = 4;
pub const VKS_INV_SKEL_VERSION: u32 = 8;
pub const VKS_INV_SKEL_TYPE_DEFAULT: u32 = 16;

/// One entry of the login skeleton as the C++ walker extracts it: the
/// defined keys (`VKS_INV_SKEL_*`) and their values through the C++ `LLSD`
/// coercions (`asString`, `asUUID`, `asInteger`). The name is borrowed for
/// the call.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvSkeletonEntry {
    pub defined: u32,
    pub name: VksInvBytes,
    pub folder_id: Uuid,
    pub parent_id: Uuid,
    pub version: i32,
    pub type_default: i32,
}

/// The folders of a skeleton, between the import and the load.
pub struct VksInvSkeleton {
    skeleton: Skeleton,
}

/// The import loop of `loadSkeleton` for `owner`: the folders of the
/// skeleton, one per id. Null on a panic.
///
/// # Safety
/// `owner` points to 16 bytes; `entries` to `count` entries (or null with
/// 0) whose names are readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_skeleton_import(
    owner: *const u8,
    entries: *const VksInvSkeletonEntry,
    count: usize,
) -> *mut VksInvSkeleton {
    guard(ptr::null_mut(), || {
        let owner = unsafe { uuid_arg(owner) };
        let list: Vec<SkeletonEntry> = unsafe { views(entries, count) }
            .iter()
            .map(|e| {
                let has = |bit| e.defined & bit != 0;
                SkeletonEntry {
                    name: has(VKS_INV_SKEL_NAME).then(|| unsafe { e.name.get() }.to_vec()),
                    folder_id: has(VKS_INV_SKEL_FOLDER_ID).then_some(e.folder_id),
                    parent_id: has(VKS_INV_SKEL_PARENT_ID).then_some(e.parent_id),
                    version: has(VKS_INV_SKEL_VERSION).then_some(e.version),
                    type_default: has(VKS_INV_SKEL_TYPE_DEFAULT).then_some(e.type_default),
                }
            })
            .collect();
        Box::into_raw(Box::new(VksInvSkeleton {
            skeleton: Skeleton::import(&owner, &list),
        }))
    })
}

/// Whether `loadSkeleton` reads the cache: the skeleton has a folder.
///
/// # Safety
/// `s` must come from `vks_inv_skeleton_import` and be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_skeleton_wants_cache(s: *const VksInvSkeleton) -> bool {
    match unsafe { s.as_ref() } {
        Some(s) => guard(false, || s.skeleton.wants_cache()),
        None => false,
    }
}

/// `loadSkeleton`'s return value as far as the import decides it: false
/// when an entry lacked a key or had a null folder id. (The load returns the
/// same value in its plan.)
///
/// # Safety
/// `s` must come from `vks_inv_skeleton_import` and be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_skeleton_rv(s: *const VksInvSkeleton) -> bool {
    match unsafe { s.as_ref() } {
        Some(s) => s.skeleton.rv,
        None => false,
    }
}

/// Releases a skeleton that was not loaded. Null is ignored.
///
/// # Safety
/// `s` must come from `vks_inv_skeleton_import` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_skeleton_free(s: *mut VksInvSkeleton) {
    if !s.is_null() {
        drop(unsafe { Box::from_raw(s) });
    }
}

/// How the C++ found the cache file (`CacheInput`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvCacheInput {
    /// `<name>.gz` could be opened.
    pub gz_present: bool,
    /// `gunzip_file` succeeded (read only when `gz_present`).
    pub gunzip_ok: bool,
    /// The file `loadFromFile` would open could be opened; its bytes are
    /// `file`.
    pub file_present: bool,
    pub file: VksInvBytes,
    /// `LLAppViewer::isSecondInstance()`.
    pub second_instance: bool,
}

/// A folder of the plan: the category (owner and version included) and its
/// descendent count (`-1`, unknown, for a folder of unknown version).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvLoginFolder {
    pub category: VksInvCategory,
    pub descendent_count: i32,
}

/// What `loadSkeleton` does. Every pointer points into the block and lives
/// until `vks_inv_login_plan_free`.
#[repr(C)]
pub struct VksInvLoginPlanView {
    /// The return value.
    pub rv: bool,
    /// `addCategory` for each, in this order (UUID order); `addCategory`
    /// itself skips `FT_MESH` folders.
    pub folders: *const VksInvLoginFolder,
    pub folder_count: usize,
    /// `addItem` for each, in this order, after the folders.
    pub items: *const VksInvItem,
    pub item_count: usize,
    /// Folders of the model before the call whose version becomes unknown,
    /// 16 bytes each, in UUID order.
    pub invalidate: *const u8,
    pub invalidate_count: usize,
    /// Remove the plain (or temporary) file; remove `<name>.gz`.
    pub remove_plain: bool,
    pub remove_gz: bool,
    /// The log line's counts.
    pub cached_category_count: usize,
    pub cached_item_count: usize,
}

/// A login plan.
pub struct VksInvLoginPlan {
    /// The records the views point into.
    _plan: LoadPlan,
    _folder_views: Vec<VksInvLoginFolder>,
    _item_views: Vec<VksInvItem>,
    _invalidate: Vec<u8>,
    view: VksInvLoginPlanView,
}

/// The reconciliation of `loadSkeleton` over the cache as the C++ found it
/// and the model as it is before the call (`host`). Takes `skeleton`, which
/// must not be used or freed afterwards. Null on a panic.
///
/// # Safety
/// `skeleton` must come from `vks_inv_skeleton_import`; `input` must be
/// valid with a readable `file`; `host` null or valid for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_skeleton_load(
    skeleton: *mut VksInvSkeleton,
    input: *const VksInvCacheInput,
    in_open_sim: bool,
    model: *const VksInvModelHost,
) -> *mut VksInvLoginPlan {
    if skeleton.is_null() || input.is_null() {
        return ptr::null_mut();
    }
    let skeleton = unsafe { Box::from_raw(skeleton) }.skeleton;
    guard(ptr::null_mut(), || {
        let i = unsafe { &*input };
        let cache = CacheInput {
            gz_present: i.gz_present,
            gunzip_ok: i.gunzip_ok,
            file: i.file_present.then(|| unsafe { i.file.get() }),
            second_instance: i.second_instance,
        };
        let lookup = unsafe { host(model) };
        let plan = skeleton.load(&cache, in_open_sim, &lookup);
        let folder_views: Vec<VksInvLoginFolder> = plan
            .folders
            .iter()
            .map(|f| VksInvLoginFolder {
                category: category_view(&f.folder),
                descendent_count: f.descendent_count,
            })
            .collect();
        let item_views: Vec<VksInvItem> = plan.items.iter().map(item_view).collect();
        let invalidate: Vec<u8> = plan.invalidate.iter().flatten().copied().collect();
        let view = VksInvLoginPlanView {
            rv: plan.rv,
            folders: folder_views.as_ptr(),
            folder_count: folder_views.len(),
            items: item_views.as_ptr(),
            item_count: item_views.len(),
            invalidate: invalidate.as_ptr(),
            invalidate_count: plan.invalidate.len(),
            remove_plain: plan.remove_plain,
            remove_gz: plan.remove_gz,
            cached_category_count: plan.cached_category_count,
            cached_item_count: plan.cached_item_count,
        };
        Box::into_raw(Box::new(VksInvLoginPlan {
            _plan: plan,
            _folder_views: folder_views,
            _item_views: item_views,
            _invalidate: invalidate,
            view,
        }))
    })
}

/// The view of a login plan.
///
/// # Safety
/// `p` must come from `vks_inv_skeleton_load` and be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_login_plan_view(
    p: *const VksInvLoginPlan,
) -> *const VksInvLoginPlanView {
    // The views point into the plan's own records and vectors, whose heap
    // buffers did not move when the block was boxed.
    match unsafe { p.as_ref() } {
        Some(p) => &p.view,
        None => ptr::null(),
    }
}

/// Releases a login plan. Null is ignored.
///
/// # Safety
/// `p` must come from `vks_inv_skeleton_load` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_login_plan_free(p: *mut VksInvLoginPlan) {
    if !p.is_null() {
        drop(unsafe { Box::from_raw(p) });
    }
}

// Layout: checked here and by static asserts in the header (64-bit).
const _: () = {
    use std::mem::size_of;
    if size_of::<usize>() == 8 {
        assert!(size_of::<VksInvModelHost>() == 24);
        assert!(size_of::<VksInvSkeletonEntry>() == 64);
        assert!(size_of::<VksInvCacheInput>() == 32);
        assert!(size_of::<VksInvLoginFolder>() == 128);
    }
};

// ---------------------------------------------------------------------------
// validate()

/// A folder as `validate()` reads it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvSnapCategory {
    pub id: Uuid,
    pub parent: Uuid,
    /// `LLFolderType::EType`, any value.
    pub preferred_type: i32,
    pub version: i32,
    pub descendent_count: i32,
}

/// An item as `validate()` reads it: the stored type (`getActualType`) and
/// the stored asset id.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvSnapItem {
    pub id: Uuid,
    pub parent: Uuid,
    pub asset: Uuid,
    pub asset_type: i32,
}

/// A map entry: the key and the record index (`-1` for a null pointer).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvSnapEntry {
    pub key: Uuid,
    pub index: i64,
}

/// A child vector of a parent-to-children map: `count` record indices
/// (`-1` for a null pointer) from `first` in the snapshot's `children`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvSnapTree {
    pub parent: Uuid,
    pub first: usize,
    pub count: usize,
}

/// The model as `validate()` reads it. Object identity (the C++ compares
/// pointers) is the record index. The map entries come in the order the C++
/// iterates the maps (the standard library's `unordered_map` order).
#[repr(C)]
pub struct VksInvSnapshot {
    pub root: Uuid,
    pub library_root: Uuid,
    pub categories: *const VksInvSnapCategory,
    pub category_count: usize,
    pub items: *const VksInvSnapItem,
    pub item_count: usize,
    pub category_map: *const VksInvSnapEntry,
    pub category_map_count: usize,
    pub item_map: *const VksInvSnapEntry,
    pub item_map_count: usize,
    pub category_tree: *const VksInvSnapTree,
    pub category_tree_count: usize,
    pub item_tree: *const VksInvSnapTree,
    pub item_tree_count: usize,
    pub children: *const i64,
    pub children_count: usize,
    /// `mBacklinkMMap`: (target, link) pairs, 32 bytes each.
    pub backlinks: *const u8,
    pub backlink_count: usize,
    /// The keys of `mCategoryLock` and `mItemLock` set to true, 16 bytes
    /// each.
    pub category_locks: *const u8,
    pub category_lock_count: usize,
    pub item_locks: *const u8,
    pub item_lock_count: usize,
    /// `mLastItem` as an item index, `-1` for none.
    pub last_item: i64,
    /// `LLGridManager::isInOpenSim()` (read by OpenSim builds only).
    pub in_open_sim: bool,
}

/// One `mWarnings` entry.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvWarning {
    pub key: VksInvBytes,
    pub count: u32,
}

/// What `validate()` returns (`LLInventoryValidationInfo`) and leaves
/// behind (`mLastItem`). Every pointer points into the block and lives
/// until `vks_inv_validation_free`.
#[repr(C)]
pub struct VksInvValidationView {
    pub initialized: bool,
    pub warning_count: i32,
    /// `mWarnings`, in key order.
    pub warnings: *const VksInvWarning,
    pub warnings_count: usize,
    pub loop_count: i32,
    pub orphaned_count: i32,
    pub fatal_error_count: i32,
    pub fatal_no_root_folder: bool,
    pub fatal_system_duplicate: i32,
    pub fatal_no_library_root_folder: bool,
    pub fatal_qa_debug_mode: bool,
    /// Folder types, in order.
    pub missing_required_system_folders: *const i32,
    pub missing_count: usize,
    pub duplicate_required_system_folders: *const i32,
    pub duplicate_count: usize,
    /// `mLog`'s text.
    pub log: VksInvBytes,
    /// `mLastItem` afterwards, an item index or `-1`.
    pub last_item: i64,
}

/// A validation result.
pub struct VksInvValidation {
    _keys: Vec<Vec<u8>>,
    _warnings: Vec<VksInvWarning>,
    _missing: Vec<i32>,
    _duplicate: Vec<i32>,
    _log: Vec<u8>,
    view: VksInvValidationView,
}

/// The engine of `QAModeFakeSystemFolderIssues`: the
/// `std::default_random_engine` of this build's standard library (`mt19937`
/// with MSVC, `minstd_rand0` with libstdc++). Its state carries over between
/// calls, as the C++ function-static engine's did.
pub struct VksInvQaRandom {
    random: QaRandom,
}

/// A new engine in its initial state. Never null.
#[unsafe(no_mangle)]
pub extern "C" fn vks_inv_qa_random_new() -> *mut VksInvQaRandom {
    Box::into_raw(Box::new(VksInvQaRandom {
        random: QaRandom::native(),
    }))
}

/// Releases an engine. Null is ignored.
///
/// # Safety
/// `r` must come from `vks_inv_qa_random_new` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_qa_random_free(r: *mut VksInvQaRandom) {
    if !r.is_null() {
        drop(unsafe { Box::from_raw(r) });
    }
}

fn index(i: i64, len: usize) -> Option<usize> {
    usize::try_from(i).ok().filter(|&i| i < len)
}

unsafe fn snapshot_from(s: &VksInvSnapshot) -> Snapshot {
    let categories: Vec<SnapCategory> = unsafe { views(s.categories, s.category_count) }
        .iter()
        .map(|c| SnapCategory {
            id: c.id,
            parent: c.parent,
            preferred_type: FolderType(c.preferred_type),
            version: c.version,
            descendent_count: c.descendent_count,
        })
        .collect();
    let items: Vec<SnapItem> = unsafe { views(s.items, s.item_count) }
        .iter()
        .map(|i| SnapItem {
            id: i.id,
            parent: i.parent,
            asset_type: AssetType(i.asset_type),
            asset: i.asset,
        })
        .collect();
    let (nc, ni) = (categories.len(), items.len());
    let cat_entries = unsafe { views(s.category_map, s.category_map_count) };
    let item_entries = unsafe { views(s.item_map, s.item_map_count) };
    let children = unsafe { views(s.children, s.children_count) };
    let tree = |trees: &[VksInvSnapTree], len: usize| -> BTreeMap<Uuid, Vec<Option<usize>>> {
        trees
            .iter()
            .map(|t| {
                let kids = children
                    .get(t.first..t.first.saturating_add(t.count))
                    .unwrap_or(&[]);
                (t.parent, kids.iter().map(|&k| index(k, len)).collect())
            })
            .collect()
    };
    let uuids =
        |p: *const u8, n: usize| -> Vec<Uuid> { unsafe { views(p as *const Uuid, n) }.to_vec() };
    let pairs = uuids(s.backlinks, s.backlink_count.saturating_mul(2));
    Snapshot {
        root: s.root,
        library_root: s.library_root,
        category_map: cat_entries
            .iter()
            .map(|e| (e.key, index(e.index, nc)))
            .collect(),
        item_map: item_entries
            .iter()
            .filter_map(|e| index(e.index, ni).map(|i| (e.key, i)))
            .collect(),
        category_order: cat_entries.iter().map(|e| e.key).collect(),
        item_order: item_entries
            .iter()
            .filter(|e| index(e.index, ni).is_some())
            .map(|e| e.key)
            .collect(),
        category_tree: tree(unsafe { views(s.category_tree, s.category_tree_count) }, nc),
        item_tree: tree(unsafe { views(s.item_tree, s.item_tree_count) }, ni),
        backlinks: pairs.chunks_exact(2).map(|p| (p[0], p[1])).collect(),
        category_locks: uuids(s.category_locks, s.category_lock_count)
            .into_iter()
            .collect(),
        item_locks: uuids(s.item_locks, s.item_lock_count).into_iter().collect(),
        last_item: index(s.last_item, ni),
        categories,
        items,
    }
}

/// `LLInventoryModel::validate()` over a snapshot of the model. `qa` is the
/// engine to draw from when `QAModeFakeSystemFolderIssues` is set, null
/// otherwise. Null on a panic (including map orders that do not name exactly
/// the map keys).
///
/// # Safety
/// `snapshot` must be valid with readable arrays; `qa` null or from
/// `vks_inv_qa_random_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_validate(
    snapshot: *const VksInvSnapshot,
    qa: *mut VksInvQaRandom,
) -> *mut VksInvValidation {
    guard(ptr::null_mut(), || {
        let Some(s) = (unsafe { snapshot.as_ref() }) else {
            return ptr::null_mut();
        };
        let snap = unsafe { snapshot_from(s) };
        let qa = unsafe { qa.as_mut() }.map(|q| &mut q.random);
        let v = validate::validate(
            &snap,
            validate::Options {
                qa,
                in_open_sim: s.in_open_sim,
            },
        );
        let info = v.info;
        let keys: Vec<Vec<u8>> = info
            .warnings
            .keys()
            .map(|k| k.as_bytes().to_vec())
            .collect();
        let warnings: Vec<VksInvWarning> = keys
            .iter()
            .zip(info.warnings.values())
            .map(|(k, &count)| VksInvWarning {
                key: VksInvBytes::of(k),
                count,
            })
            .collect();
        let missing: Vec<i32> = info
            .missing_required_system_folders
            .iter()
            .copied()
            .collect();
        let duplicate: Vec<i32> = info
            .duplicate_required_system_folders
            .iter()
            .copied()
            .collect();
        let log = info.log.into_bytes();
        let view = VksInvValidationView {
            initialized: info.initialized,
            warning_count: info.warning_count,
            warnings: warnings.as_ptr(),
            warnings_count: warnings.len(),
            loop_count: info.loop_count,
            orphaned_count: info.orphaned_count,
            fatal_error_count: info.fatal_error_count,
            fatal_no_root_folder: info.fatal_no_root_folder,
            fatal_system_duplicate: info.fatal_system_duplicate,
            fatal_no_library_root_folder: info.fatal_no_library_root_folder,
            fatal_qa_debug_mode: info.fatal_qa_debug_mode,
            missing_required_system_folders: missing.as_ptr(),
            missing_count: missing.len(),
            duplicate_required_system_folders: duplicate.as_ptr(),
            duplicate_count: duplicate.len(),
            log: VksInvBytes::of(&log),
            last_item: v.last_item.map_or(-1, |i| i as i64),
        };
        Box::into_raw(Box::new(VksInvValidation {
            _keys: keys,
            _warnings: warnings,
            _missing: missing,
            _duplicate: duplicate,
            _log: log,
            view,
        }))
    })
}

/// The view of a validation result.
///
/// # Safety
/// `v` must come from `vks_inv_validate` and be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_validation_view(
    v: *const VksInvValidation,
) -> *const VksInvValidationView {
    match unsafe { v.as_ref() } {
        Some(v) => &v.view,
        None => ptr::null(),
    }
}

/// Releases a validation result. Null is ignored.
///
/// # Safety
/// `v` must come from `vks_inv_validate` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_validation_free(v: *mut VksInvValidation) {
    if !v.is_null() {
        drop(unsafe { Box::from_raw(v) });
    }
}

const _: () = {
    use std::mem::size_of;
    if size_of::<usize>() == 8 {
        assert!(size_of::<VksInvSnapCategory>() == 44);
        assert!(size_of::<VksInvSnapItem>() == 52);
        assert!(size_of::<VksInvSnapEntry>() == 24);
        assert!(size_of::<VksInvSnapTree>() == 32);
        assert!(size_of::<VksInvSnapshot>() == 208);
        assert!(size_of::<VksInvWarning>() == 24);
    }
};
