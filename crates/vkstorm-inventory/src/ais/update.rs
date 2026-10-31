//! `AISUpdate`: the parse of an AIS reply (`llaisapi.cpp:1050-1641`) and the
//! ordered operations its applier `doUpdate()` performs (`:1643-1863`).
//!
//! # The split
//!
//! [`ParsedUpdate::parse`] is the constructor's `parseUpdate(update)`: it
//! reads the reply, queries the model through [`Model`] exactly where the
//! C++ queries `gInventory` and the model's objects, makes the parse's own
//! model calls through it (the stale-folder refetch), and collects what
//! `doUpdate()` applies. [`ParsedUpdate::apply_ops`] is `doUpdate()`'s walk
//! over those collections, flattened into [`ApplyOp`]s. A C++ executor
//! applies them in order; the two reads `doUpdate()` makes of the model
//! (the version and count copied back into an updated folder, and the
//! version rule) happen in the executor at apply time, as the C++ does,
//! because the model can change between the parse and the apply (the update
//! yields inside `doUpdate()`).
//!
//! # Queries and their order
//!
//! The corpus records every query (`q.getCategory`, `q.getItem`,
//! `q.getVersion`) between the model calls. The parse therefore asks the
//! host one query at a time, in the C++ order, rather than taking a model
//! snapshot as input: a snapshot would move or drop queries. Every
//! `getVersion()` the C++ makes on a model folder is a
//! [`Model::get_version`] call, also where the C++ only logs the
//! comparison (the warning in the fetch branch), and none where it is only
//! an argument of a log line (the corpus records with logging off).
//!
//! # Yields
//!
//! The C++ parse calls `checkTimeout()` at the start of every
//! `parseEmbedded()`, and may yield there. The parse calls
//! [`Model::checkpoint`] at the same points (and [`Model::begin_update`]
//! where the constructor arms the timers), so a host that runs
//! `checkTimeout()` there keeps the recorded yield points of the corpus's
//! `sched` runs; a host that does not yield there only moves yields into
//! `doUpdate()`. A stop at a checkpoint ends the parse ([`Flow::Stop`]);
//! the update is then neither applied nor called back.
//!
//! # Objects
//!
//! The parse builds new objects ([`ItemRecord`], [`CategoryRecord`]): fresh
//! ones, or copies of the model's current object ("default to current
//! values"), then `unpackMessage` over them. They are kept in two arenas
//! and the collections hold indices, so an object the C++ holds in two maps
//! (a lost item is in `mItemsCreated` and `mItemsLost`) is one index, and an
//! id parsed twice gets a second object as in the C++. Names are left as
//! `fromLLSD` wrote them: `unpackMessage`'s `localizeInventoryObjectName` is
//! the executor's, applied once when it materializes each object (nothing
//! in the parse reads a name).
//!
//! # Deltas
//!
//! `mCatDescendentDeltas` is a `std::map<LLUUID, size_t>` read back with
//! `static_cast<S32>`; here it is signed ([`i32`], wrapping), which reads
//! the same on every platform.

use std::collections::{BTreeMap, BTreeSet};

use vkstorm_llsd::Value;
use vkstorm_llsd::text::uuid_set;

use super::command::{CommandType, MAX_FOLDER_DEPTH_REQUEST};
use crate::category::{Category, VERSION_UNKNOWN};
use crate::item::Item;
use crate::llsd::coerce::{as_boolean, as_integer, as_uuid, get, has};
use crate::perms::{NULL_ID, PERM_NONE, Permissions, Uuid};
use crate::sale::SaleInfo;
use crate::types::{AssetType, FolderType};

/// `LLViewerInventoryCategory::DESCENDENT_COUNT_UNKNOWN`.
pub const DESCENDENT_COUNT_UNKNOWN: i32 = -1;
/// `MAX_UPDATE_BACKLOG` of `doUpdate()`: the observers are flushed when the
/// model's changed-id set holds more ids than this after a create.
pub const MAX_UPDATE_BACKLOG: usize = 50;
/// `fetch()`'s default `expiry_seconds`, the stale-folder refetch's.
pub const FETCH_DEFAULT_EXPIRY: i32 = 10;
/// `LONG_EXPIRY` of the version rule's refetch.
pub const FETCH_LONG_EXPIRY: i32 = 360;
/// `LLInventoryObserver::CREATE`, the mask of the create calls.
pub const MASK_CREATE: u32 = 512;

/// `LLViewerInventoryCategory::mDescendentsRequested`, an `LLFrameTimer`:
/// carried from the model's folder into its parse copy and back into the
/// model by `updateCategory`, never read by the parse.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct FetchTimer {
    /// `mStartTime`.
    pub start_time: f64,
    /// `mExpiry`.
    pub expiry: f64,
    /// `mStarted`.
    pub started: bool,
}

/// The fields of an `LLViewerInventoryItem`: the item record, `mIsComplete`,
/// `mTransactionID`, `mIsRunning`, `mIsFaulted`.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemRecord {
    pub item: Item,
    pub complete: bool,
    pub transaction_id: Uuid,
    pub running: bool,
    pub faulted: bool,
}

impl Default for ItemRecord {
    /// `LLViewerInventoryItem()`: a default item, incomplete, null
    /// transaction id, not running, not faulted.
    fn default() -> Self {
        ItemRecord {
            item: Item::default(),
            complete: false,
            transaction_id: NULL_ID,
            running: false,
            faulted: false,
        }
    }
}

impl ItemRecord {
    /// `copyViewerItem`: `LLInventoryItem::copyItem`, then the viewer fields.
    pub fn copy_viewer_item(&mut self, other: &ItemRecord) {
        self.item.copy_item(&other.item);
        self.complete = other.complete;
        self.transaction_id = other.transaction_id;
        self.running = other.running;
        self.faulted = other.faulted;
    }

    /// `LLViewerInventoryItem::unpackMessage(const LLSD&)`:
    /// `LLInventoryItem::fromLLSD(sd)` (`is_new` true), `running` and
    /// `faulted` by `asBoolean`, then complete. The name localisation is
    /// left to the executor (see the module documentation).
    pub fn unpack_message(&mut self, sd: &Value, in_open_sim: bool) -> bool {
        let rv = self.item.from_llsd(sd, true, in_open_sim);
        self.running = as_boolean(get(sd, "running"));
        self.faulted = as_boolean(get(sd, "faulted"));
        self.complete = true;
        rv
    }
}

/// The fields of an `LLViewerInventoryCategory` the parse reads and writes:
/// the category record, `mOwnerID`, `mVersion`, `mDescendentCount` and
/// `mDescendentsRequested` (`mFetching` is `FETCH_NONE` in every object the
/// parse builds).
#[derive(Clone, Debug, PartialEq)]
pub struct CategoryRecord {
    pub category: Category,
    pub owner: Uuid,
    pub version: i32,
    pub descendents: i32,
    /// The request timer: `None` for a new object (`reset()` at
    /// construction, so expired), else the model folder's, copied.
    pub requested: Option<FetchTimer>,
}

impl CategoryRecord {
    /// `LLViewerInventoryCategory(owner_id)`: a default category, version
    /// and count unknown, a fresh request timer.
    pub fn new_owned(owner: &Uuid) -> CategoryRecord {
        CategoryRecord {
            category: Category::default(),
            owner: *owner,
            version: VERSION_UNKNOWN,
            descendents: DESCENDENT_COUNT_UNKNOWN,
            requested: None,
        }
    }

    /// The copy constructor `LLViewerInventoryCategory(other)`:
    /// `copyViewerCategory(other)` (`copyCategory`, which leaves the date at
    /// 0, the owner, `setVersion(other->getVersion())` with `version` the
    /// value that call returned, the count and the request timer).
    pub fn copy_of(other: &CategoryRecord, version: i32) -> CategoryRecord {
        CategoryRecord {
            category: Category::copy_from(&other.category),
            owner: other.owner,
            version,
            descendents: other.descendents,
            requested: other.requested,
        }
    }

    /// `LLViewerInventoryCategory::unpackMessage(const LLSD&)`:
    /// `LLInventoryCategory::fromLLSD`, which does not read the version or
    /// count. The name localisation is the executor's.
    pub fn unpack_message(&mut self, sd: &Value) -> bool {
        self.category.from_llsd(sd)
    }
}

/// The host's handle on a folder of the model, valid for one parse: the
/// executor's `LLViewerInventoryCategory*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModelRef(pub u64);

/// A folder of the model, as `gInventory.getCategory()` returns it.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelCategory {
    pub handle: ModelRef,
    /// Every field, read directly. The parse takes the version from
    /// [`Model::get_version`] wherever the C++ calls `getVersion()`.
    pub record: CategoryRecord,
}

/// What the host says at a parse checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Go on parsing.
    Continue,
    /// The coroutine is stopping (`LLCoros::Stopping` after the yield): the
    /// parse ends where it is and nothing of it is applied.
    Stop,
}

/// The inventory model as the parse sees it. Each method is one C++ call,
/// made where and as often as the C++ makes it.
pub trait Model {
    /// `gInventory.getCategory(id)`.
    fn get_category(&mut self, id: &Uuid) -> Option<ModelCategory>;
    /// `gInventory.getItem(id)`: the model's item, every field.
    fn get_item(&mut self, id: &Uuid) -> Option<ItemRecord>;
    /// `getVersion()` on a model folder.
    fn get_version(&mut self, cat: ModelRef) -> i32;
    /// `setVersion(version)` on a model folder.
    fn set_version(&mut self, cat: ModelRef, version: i32);
    /// `fetch(expiry_seconds)` on a model folder.
    fn fetch(&mut self, cat: ModelRef, expiry_seconds: i32) -> bool;
    /// `gInventory.fetchDescendentsOf(folder_id)` (the 410 branch).
    fn fetch_descendents_of(&mut self, folder_id: &Uuid) -> bool;
    /// `gInventory.onObjectDeletedFromServer(...)` (the 410 branch).
    fn on_object_deleted_from_server(
        &mut self,
        id: &Uuid,
        fix_broken_links: bool,
        update_parent_version: bool,
        do_notify_observers: bool,
    );
    /// `LLNotificationsUtil::add(name)` (the 403 branch).
    fn notify(&mut self, name: &'static str);
    /// `AISUpdate`'s constructor, before its parse: the C++ starts the
    /// update's task timer and, for the frame's first update, the shared
    /// batch timer. Called once, first, by [`ParsedUpdate::parse`]. The
    /// default does nothing.
    fn begin_update(&mut self) {}
    /// `checkTimeout()` at the start of each `parseEmbedded()`. The C++
    /// yields there when the update's budget is spent; a host may do so
    /// too, and returns [`Flow::Stop`] when the coroutine is stopping
    /// instead of letting `LLCoros::Stopping` cross the parse. The default
    /// does not yield.
    fn checkpoint(&mut self) -> Flow {
        Flow::Continue
    }
}

/// What the parse needs besides the reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseContext {
    /// `AISUpdate::mType`.
    pub command: CommandType,
    /// `gAgent.getID()`: the owner of the default permissions of a new link.
    pub agent_id: Uuid,
    /// `LLPermissions::getIsInOpenSim()` for `LLInventoryItem::fromLLSD`.
    pub in_open_sim: bool,
}

/// One step of `doUpdate()`, in order. Record indices are into
/// [`ParsedUpdate::items`] and [`ParsedUpdate::categories`]; the executor
/// materializes one viewer object per index (applying the name
/// localisation) and keeps it for the update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyOp {
    /// `checkTimeout()`: the first and the last call of `doUpdate()`.
    Checkpoint,
    /// The descendent accounting of one folder: `getCategory(category)`;
    /// if found, `accountForUpdate(LLCategoryUpdate(category, delta,
    /// false))` (FIRE-33455: the count only).
    AccountDescendents { category: Uuid, delta: i32 },
    /// `updateCategory(object, CREATE)`.
    CreateCategory { record: usize },
    /// `if (getChangedIDs().size() > MAX_UPDATE_BACKLOG) { notifyObservers();
    /// checkTimeout(); }`, after each create.
    FlushBacklog,
    /// `curr = getCategory(object id)`; if found, the object takes
    /// `curr->getVersion()` and `curr->getDescendentCount()` (read now), then
    /// `updateCategory(object)`; else nothing.
    UpdateCategory { record: usize },
    /// `lost = findCategoryUUIDForType(FT_LOST_AND_FOUND)`; if not null,
    /// for each object in order: `setParent(lost)`,
    /// `updateParentOnServer(false)`.
    MoveLostItems { records: Vec<usize> },
    /// `updateItem(object, CREATE)`.
    CreateItem { record: usize },
    /// `updateItem(object)`.
    UpdateItem { record: usize },
    /// `onObjectDeletedFromServer(id, false, false, false)`.
    DeleteObject { id: Uuid },
    /// The version rule against the model now: `cat = getCategory(category)`;
    /// if found and `cat->getVersion() != version`: for a known `version`,
    /// `setVersion(version)` when `cat->getVersion()` is unknown or lower
    /// (FIRE-33455: versions only rise), for `VERSION_UNKNOWN`
    /// `cat->fetch(FETCH_LONG_EXPIRY)`.
    VersionUpdate { category: Uuid, version: i32 },
    /// `notifyObservers()`, the last call of `doUpdate()`.
    NotifyObservers,
}

/// The ids an update touched, for the callers that follow AIS results
/// (the background fetch).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateSummary {
    pub categories_created: Vec<Uuid>,
    pub categories_updated: Vec<Uuid>,
    pub items_created: Vec<Uuid>,
    pub items_updated: Vec<Uuid>,
    pub items_lost: Vec<Uuid>,
    pub objects_deleted: Vec<Uuid>,
    /// `_updated_category_versions`: folder and version, as parsed.
    pub versions: Vec<(Uuid, i32)>,
}

/// The parse results of one `AISUpdate` (its member maps), in id order.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedUpdate {
    pub command: CommandType,
    /// `mFetch`.
    pub fetch: bool,
    /// `mFetchDepth`: the request body's `depth` for a fetch that names one,
    /// else [`MAX_FOLDER_DEPTH_REQUEST`].
    pub fetch_depth: i32,
    /// The parse's item objects.
    pub items: Vec<ItemRecord>,
    /// The parse's folder objects.
    pub categories: Vec<CategoryRecord>,
    /// `mCatDescendentDeltas`.
    pub descendent_deltas: BTreeMap<Uuid, i32>,
    /// `mCatDescendentsKnown`.
    pub descendents_known: BTreeMap<Uuid, i32>,
    /// `mCatVersionsUpdated`.
    pub versions_updated: BTreeMap<Uuid, i32>,
    /// `mItemsCreated`.
    pub items_created: BTreeMap<Uuid, usize>,
    /// `mItemsLost`.
    pub items_lost: BTreeMap<Uuid, usize>,
    /// `mItemsUpdated`.
    pub items_updated: BTreeMap<Uuid, usize>,
    /// `mCategoriesCreated`.
    pub categories_created: BTreeMap<Uuid, usize>,
    /// `mCategoriesUpdated`.
    pub categories_updated: BTreeMap<Uuid, usize>,
    /// `mObjectsDeletedIds`.
    pub objects_deleted: BTreeSet<Uuid>,
    /// `mItemIds` (`_created_items`): the write filter for embedded items
    /// and links.
    pub created_item_ids: BTreeSet<Uuid>,
    /// `mCategoryIds` (`_created_categories`): the write filter for
    /// embedded folders.
    pub created_category_ids: BTreeSet<Uuid>,
    /// The host stopped the parse at a checkpoint ([`Flow::Stop`]): the
    /// update must not be applied and no callback fires.
    pub stopped: bool,
}

/// `parseUUIDArray`: each element of `content[name]` (an array) by
/// `asUUID`, into a set.
fn parse_uuid_array(content: &Value, name: &str, ids: &mut BTreeSet<Uuid>) {
    if has(content, name)
        && let Value::Array(a) = get(content, name)
    {
        ids.extend(a.iter().map(as_uuid));
    }
}

/// `LLUUID(const std::string&)` of a map key.
fn key_id(key: &[u8]) -> Uuid {
    uuid_set(key)
}

/// The default permissions of a link: `init(agent, agent, null, null)`,
/// then `initMasks(PERM_NONE x 5)`.
fn link_permissions(agent: &Uuid) -> Permissions {
    let mut p = Permissions::DEFAULT;
    p.init(agent, agent, &NULL_ID, &NULL_ID);
    p.init_masks(PERM_NONE, PERM_NONE, PERM_NONE, PERM_NONE, PERM_NONE);
    p
}

struct Parser<'m, M: Model + ?Sized> {
    model: &'m mut M,
    ctx: ParseContext,
    out: ParsedUpdate,
}

impl<M: Model + ?Sized> Parser<'_, M> {
    fn delta(&mut self, folder: Uuid) -> &mut i32 {
        self.out.descendent_deltas.entry(folder).or_insert(0)
    }

    fn push_item(&mut self, record: ItemRecord) -> usize {
        self.out.items.push(record);
        self.out.items.len() - 1
    }

    fn push_category(&mut self, record: CategoryRecord) -> usize {
        self.out.categories.push(record);
        self.out.categories.len() - 1
    }

    /// `parseMeta`.
    fn parse_meta(&mut self, update: &Value) {
        let mut cat_ids = BTreeSet::new();
        parse_uuid_array(update, "_categories_removed", &mut cat_ids);
        for id in &cat_ids {
            if let Some(cat) = self.model.get_category(id) {
                let d = self.delta(cat.record.category.object.parent);
                *d = d.wrapping_sub(1);
                self.out.objects_deleted.insert(*id);
            }
        }

        let mut item_ids = BTreeSet::new();
        parse_uuid_array(update, "_category_items_removed", &mut item_ids);
        parse_uuid_array(update, "_removed_items", &mut item_ids);
        let mut broken_link_ids = BTreeSet::new();
        parse_uuid_array(update, "_broken_links_removed", &mut broken_link_ids);
        for ids in [&item_ids, &broken_link_ids] {
            for id in ids {
                if let Some(item) = self.model.get_item(id) {
                    let d = self.delta(item.item.object.parent);
                    *d = d.wrapping_sub(1);
                    self.out.objects_deleted.insert(*id);
                }
            }
        }

        parse_uuid_array(update, "_created_items", &mut self.out.created_item_ids);
        parse_uuid_array(
            update,
            "_created_categories",
            &mut self.out.created_category_ids,
        );

        const UCV: &str = "_updated_category_versions";
        if has(update, UCV)
            && let Value::Map(m) = get(update, UCV)
        {
            for (key, v) in m {
                self.out.versions_updated.insert(key_id(key), as_integer(v));
            }
        }
    }

    /// `parseContent`.
    fn parse_content(&mut self, update: &Value) {
        if has(update, "linked_id") && has(update, "parent_id") {
            self.parse_link(update, self.out.fetch_depth);
        } else if has(update, "item_id") && has(update, "parent_id") {
            self.parse_item(update);
        }
        if self.out.stopped {
            return;
        }

        if self.ctx.command == CommandType::FetchCategorySubset {
            // The initial folder is incomplete: only its content.
            if has(update, "_embedded") {
                self.parse_embedded(
                    get(update, "_embedded"),
                    self.out.fetch_depth.wrapping_sub(1),
                );
            }
        } else if has(update, "category_id") && has(update, "parent_id") {
            self.parse_category(update, self.out.fetch_depth);
        } else if has(update, "_embedded") {
            self.parse_embedded(get(update, "_embedded"), self.out.fetch_depth);
        }
    }

    /// `parseItem`.
    fn parse_item(&mut self, item_map: &Value) {
        let item_id = as_uuid(get(item_map, "item_id"));
        let mut new_item = ItemRecord::default();
        let curr = self.model.get_item(&item_id);
        if let Some(curr) = &curr {
            new_item.copy_viewer_item(curr);
        }
        if !new_item.unpack_message(item_map, self.ctx.in_open_sim) {
            // LL_ERRS("unpack failed"): unreachable from LLSD input (finding 8).
            return;
        }
        let parent = new_item.item.object.parent;
        if self.out.fetch {
            new_item.complete = true;
            let h = self.push_item(new_item);
            self.out.items_created.insert(item_id, h);
            if parent == NULL_ID {
                self.out.items_lost.insert(item_id, h);
            }
        } else if curr.is_some() {
            let h = self.push_item(new_item);
            self.out.items_updated.insert(item_id, h);
            self.delta(parent);
        } else {
            new_item.complete = true;
            // Finding 9 (fixed): `addItem()` refuses an item of no asset type,
            // so it is not counted; the delta entry is still made, with 0.
            // The C++ tests `getActualType()`, the stored type.
            let counted = new_item.item.object.asset_type.0 > AssetType::NONE.0;
            let h = self.push_item(new_item);
            self.out.items_created.insert(item_id, h);
            let d = self.delta(parent);
            if counted {
                *d = d.wrapping_add(1);
            }
        }
    }

    /// `parseLink`.
    fn parse_link(&mut self, link_map: &Value, depth: i32) {
        let item_id = as_uuid(get(link_map, "item_id"));
        let mut new_link = ItemRecord::default();
        let curr = self.model.get_item(&item_id);
        if let Some(curr) = &curr {
            new_link.copy_viewer_item(curr);
        }
        if !new_link.unpack_message(link_map, self.ctx.in_open_sim) {
            // LL_ERRS("unpack failed"): unreachable from LLSD input (finding 8).
            return;
        }
        let parent = new_link.item.object.parent;
        if self.out.fetch {
            new_link
                .item
                .set_permissions(&link_permissions(&self.ctx.agent_id));
            new_link.item.sale_info = SaleInfo::DEFAULT;
            new_link.complete = true;
            let h = self.push_item(new_link);
            self.out.items_created.insert(item_id, h);
            if parent == NULL_ID {
                self.out.items_lost.insert(item_id, h);
            }
        } else if curr.is_some() {
            let h = self.push_item(new_link);
            self.out.items_updated.insert(item_id, h);
            self.delta(parent);
        } else {
            new_link
                .item
                .set_permissions(&link_permissions(&self.ctx.agent_id));
            new_link.item.sale_info = SaleInfo::DEFAULT;
            new_link.complete = true;
            let h = self.push_item(new_link);
            self.out.items_created.insert(item_id, h);
            let d = self.delta(parent);
            *d = d.wrapping_add(1);
        }

        if has(link_map, "_embedded") {
            self.parse_embedded(get(link_map, "_embedded"), depth);
        }
    }

    /// `parseCategory`.
    fn parse_category(&mut self, category_map: &Value, depth: i32) {
        let category_id = as_uuid(get(category_map, "category_id"));
        let mut version = VERSION_UNKNOWN;
        if has(category_map, "version") {
            version = as_integer(get(category_map, "version"));
        }

        let curr = self.model.get_category(&category_id);

        if let Some(c) = &curr
            && self.model.get_version(c.handle) > VERSION_UNKNOWN
            && c.record.descendents != DESCENDENT_COUNT_UNKNOWN
            && version > VERSION_UNKNOWN
            && version < self.model.get_version(c.handle)
        {
            // "Got stale folder" (FIRE-33337): AIS is canonical, refetch.
            if version < self.model.get_version(c.handle) {
                self.model.set_version(c.handle, VERSION_UNKNOWN);
                self.model.fetch(c.handle, FETCH_DEFAULT_EXPIRY);
            }
            return;
        }

        let mut new_cat = match &curr {
            // Default to current values where not provided.
            Some(c) => {
                let v = self.model.get_version(c.handle);
                CategoryRecord::copy_of(&c.record, v)
            }
            None if has(category_map, "agent_id") => {
                CategoryRecord::new_owned(&as_uuid(get(category_map, "agent_id")))
            }
            None => CategoryRecord::new_owned(&NULL_ID),
        };
        // unpackMessage reads neither the version nor the count.
        if new_cat.unpack_message(category_map) {
            // The count first: a new folder may take it.
            if has(category_map, "_embedded") {
                let embedded = get(category_map, "_embedded");
                self.parse_descendent_count(category_id, new_cat.category.preferred_type, embedded);
            }

            if self.out.fetch {
                if let Some(&known) = self.out.descendents_known.get(&category_id) {
                    new_cat.descendents = known;
                    // The version only when this reply holds the full content.
                    if version > VERSION_UNKNOWN && depth >= 0 {
                        if let Some(c) = &curr {
                            // the "Version was ..." warning's test
                            let _ = self.model.get_version(c.handle) > version;
                        }
                        new_cat.version = version;
                    }
                } else if let Some(c) = &curr
                    && self.model.get_version(c.handle) > VERSION_UNKNOWN
                {
                    // the "is stale" debug line's test
                    let _ = version > self.model.get_version(c.handle);
                }
                let h = self.push_category(new_cat);
                self.out.categories_created.insert(category_id, h);
            } else if curr.is_some() {
                let parent = new_cat.category.object.parent;
                let h = self.push_category(new_cat);
                self.out.categories_updated.insert(category_id, h);
                self.delta(parent);
                self.delta(category_id);
            } else {
                if let Some(&known) = self.out.descendents_known.get(&category_id) {
                    new_cat.descendents = known;
                    if has(category_map, "version") {
                        new_cat.version = as_integer(get(category_map, "version"));
                    }
                }
                let parent = new_cat.category.object.parent;
                let h = self.push_category(new_cat);
                self.out.categories_created.insert(category_id, h);
                let d = self.delta(parent);
                *d = d.wrapping_add(1);
            }
        }
        // else LL_ERRS("unpack failed"): fromLLSD always returns true.

        if has(category_map, "_embedded") {
            self.parse_embedded(get(category_map, "_embedded"), depth.wrapping_sub(1));
        }
    }

    /// `parseDescendentCount`.
    fn parse_descendent_count(
        &mut self,
        category_id: Uuid,
        preferred_type: FolderType,
        embedded: &Value,
    ) {
        if has(embedded, "categories") && has(embedded, "links") && has(embedded, "items") {
            let n = get(embedded, "categories").len()
                + get(embedded, "links").len()
                + get(embedded, "items").len();
            self.out.descendents_known.insert(category_id, n as i32);
        } else if self.out.fetch
            && has(embedded, "links")
            && (preferred_type == FolderType::CURRENT_OUTFIT
                || preferred_type == FolderType::OUTFIT)
        {
            // COF and outfits hold links only.
            self.out
                .descendents_known
                .insert(category_id, get(embedded, "links").len() as i32);
        }
    }

    /// `parseEmbedded`.
    fn parse_embedded(&mut self, embedded: &Value, depth: i32) {
        if self.out.stopped {
            return;
        }
        if self.model.checkpoint() == Flow::Stop {
            self.out.stopped = true;
            return;
        }
        // A stop inside one part ends the parse (the C++ throws).
        if has(embedded, "links") {
            // _embedded in a category
            self.parse_embedded_links(get(embedded, "links"), depth);
        }
        if has(embedded, "items") && !self.out.stopped {
            // _embedded in a category
            self.parse_embedded_items(get(embedded, "items"));
        }
        if has(embedded, "item") && !self.out.stopped {
            // _embedded in a link
            self.parse_embedded_item(get(embedded, "item"));
        }
        if has(embedded, "categories") && !self.out.stopped {
            // _embedded in a category
            self.parse_embedded_categories(get(embedded, "categories"), depth);
        }
        if has(embedded, "category") && !self.out.stopped {
            // _embedded in a link
            self.parse_embedded_category(get(embedded, "category"), depth);
        }
    }

    /// `parseEmbeddedLinks`.
    fn parse_embedded_links(&mut self, links: &Value, depth: i32) {
        let Value::Map(m) = links else { return };
        for (key, link_map) in m {
            if self.out.stopped {
                return;
            }
            if self.out.fetch || self.out.created_item_ids.contains(&key_id(key)) {
                self.parse_link(link_map, depth);
            }
        }
    }

    /// `parseEmbeddedItem`: a single item (`_embedded` in a link).
    fn parse_embedded_item(&mut self, item: &Value) {
        if has(item, "item_id")
            && (self.out.fetch
                || self
                    .out
                    .created_item_ids
                    .contains(&as_uuid(get(item, "item_id"))))
        {
            self.parse_item(item);
        }
    }

    /// `parseEmbeddedItems`: a map of items (`_embedded` in a category).
    fn parse_embedded_items(&mut self, items: &Value) {
        let Value::Map(m) = items else { return };
        for (key, item_map) in m {
            if self.out.fetch || self.out.created_item_ids.contains(&key_id(key)) {
                self.parse_item(item_map);
            }
        }
    }

    /// `parseEmbeddedCategory`: a single folder (`_embedded` in a link).
    fn parse_embedded_category(&mut self, category: &Value, depth: i32) {
        if has(category, "category_id")
            && (self.out.fetch
                || self
                    .out
                    .created_category_ids
                    .contains(&as_uuid(get(category, "category_id"))))
        {
            self.parse_category(category, depth);
        }
    }

    /// `parseEmbeddedCategories`: a map of folders (`_embedded` in a
    /// category).
    fn parse_embedded_categories(&mut self, categories: &Value, depth: i32) {
        let Value::Map(m) = categories else { return };
        for (key, category_map) in m {
            if self.out.stopped {
                return;
            }
            if self.out.fetch || self.out.created_category_ids.contains(&key_id(key)) {
                self.parse_category(category_map, depth);
            }
        }
    }
}

impl ParsedUpdate {
    /// `AISUpdate(update, type, request_body)`'s parse (the constructor's
    /// timer and batch-frame logic stays with the C++ `checkTimeout()`).
    pub fn parse<M: Model + ?Sized>(
        update: &Value,
        request_body: &Value,
        ctx: &ParseContext,
        model: &mut M,
    ) -> ParsedUpdate {
        let fetch = ctx.command.is_fetch();
        let mut fetch_depth = MAX_FOLDER_DEPTH_REQUEST;
        if fetch && has(request_body, "depth") {
            fetch_depth = as_integer(get(request_body, "depth"));
        }
        let out = ParsedUpdate {
            command: ctx.command,
            fetch,
            fetch_depth,
            items: Vec::new(),
            categories: Vec::new(),
            descendent_deltas: BTreeMap::new(),
            descendents_known: BTreeMap::new(),
            versions_updated: BTreeMap::new(),
            items_created: BTreeMap::new(),
            items_lost: BTreeMap::new(),
            items_updated: BTreeMap::new(),
            categories_created: BTreeMap::new(),
            categories_updated: BTreeMap::new(),
            objects_deleted: BTreeSet::new(),
            created_item_ids: BTreeSet::new(),
            created_category_ids: BTreeSet::new(),
            stopped: false,
        };
        model.begin_update();
        let mut p = Parser {
            model,
            ctx: *ctx,
            out,
        };
        p.parse_meta(update);
        p.parse_content(update);
        p.out
    }

    /// `doUpdate()`'s steps, in order (empty for a stopped parse).
    pub fn apply_ops(&self) -> Vec<ApplyOp> {
        if self.stopped {
            return Vec::new();
        }
        let mut ops = vec![ApplyOp::Checkpoint];
        // Descendent accounting: not for folders this update creates, nor
        // for those AIS did not report as updated.
        for (category, &delta) in &self.descendent_deltas {
            if !self.categories_created.contains_key(category)
                && self.versions_updated.contains_key(category)
            {
                ops.push(ApplyOp::AccountDescendents {
                    category: *category,
                    delta,
                });
            }
        }
        for &record in self.categories_created.values() {
            ops.push(ApplyOp::CreateCategory { record });
            ops.push(ApplyOp::FlushBacklog);
        }
        for &record in self.categories_updated.values() {
            ops.push(ApplyOp::UpdateCategory { record });
        }
        if !self.items_lost.is_empty() {
            ops.push(ApplyOp::MoveLostItems {
                records: self.items_lost.values().copied().collect(),
            });
        }
        for &record in self.items_created.values() {
            ops.push(ApplyOp::CreateItem { record });
            ops.push(ApplyOp::FlushBacklog);
        }
        for &record in self.items_updated.values() {
            ops.push(ApplyOp::UpdateItem { record });
        }
        for id in &self.objects_deleted {
            ops.push(ApplyOp::DeleteObject { id: *id });
        }
        for (category, &version) in &self.versions_updated {
            ops.push(ApplyOp::VersionUpdate {
                category: *category,
                version,
            });
        }
        ops.push(ApplyOp::Checkpoint);
        ops.push(ApplyOp::NotifyObservers);
        ops
    }

    /// The ids the update touches.
    pub fn summary(&self) -> UpdateSummary {
        UpdateSummary {
            categories_created: self.categories_created.keys().copied().collect(),
            categories_updated: self.categories_updated.keys().copied().collect(),
            items_created: self.items_created.keys().copied().collect(),
            items_updated: self.items_updated.keys().copied().collect(),
            items_lost: self.items_lost.keys().copied().collect(),
            objects_deleted: self.objects_deleted.iter().copied().collect(),
            versions: self
                .versions_updated
                .iter()
                .map(|(k, v)| (*k, *v))
                .collect(),
        }
    }
}
