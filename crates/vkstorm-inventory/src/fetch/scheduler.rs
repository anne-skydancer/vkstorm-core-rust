//! `LLInventoryModelBackgroundFetch` (`newview/llinventorymodelbackgroundfetch.cpp`):
//! the folder and item queues, the AIS scheduler `bulkFetchViaAis`, the
//! legacy scheduler `bulkFetch`, the AIS completion handlers, the legacy
//! handlers' scheduling rules, and the public predicates.
//!
//! Every function keeps the C++ name in its doc comment and follows the C++
//! line by line, including its recorded defects (R3b README "Findings"):
//! the legacy 403 split's empty POST (1), legacy replies stranded by a
//! switch to AIS (2), a queue drained when the region has no inventory
//! capability (3), the forced set on the legacy path (8), version 0 after
//! an outage (9). `incrFetchFolderCount` resets the folder count on the
//! fetch count's underflow, as the C++ does.
//!
//! Not ported: the `#ifdef OPENSIM` UDP background loop of
//! `backgroundFetch()` (the recorded build has `OPENSIM` off; R3b "Not
//! covered"). With the `opensim` feature, `bulkFetch()`'s `#ifdef OPENSIM`
//! condition is followed.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::ais::AisFetch;
use super::legacy::{FolderBody, FolderRequest, ItemBody, ItemRequest};
use super::{
    CategoryFetch, FT_MARKETPLACE_LISTINGS, FetchType, NULL_ID, QueueEntry, Uuid, VERSION_UNKNOWN,
};

/// `ALEXANDRIA_LINDEN_ID`: the owner whose folders AIS fetches through
/// `LibraryAPIv3`.
pub const ALEXANDRIA_LINDEN_ID: Uuid = [
    0xba, 0x2a, 0x56, 0x4a, 0xf0, 0xf1, 0x4b, 0x82, 0x9c, 0x61, 0xb7, 0x52, 0x0b, 0xfc, 0xd0, 0x9f,
];

/// `bulkFetch()`'s `max_batch_size`.
pub const LEGACY_MAX_BATCH_SIZE: u32 = 10;
/// `bulkFetch()`'s `max_concurrent_fetches` (outstanding requests).
pub const LEGACY_MAX_CONCURRENT_FETCHES: i32 = 12;

/// A caller's callback, registered on the C++ side: a `nullary_func_t` or an
/// `AISAPI::completion_t`. The scheduler only passes the token back.
pub type CallerToken = u64;

/// What the scheduler reads of a category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CategoryInfo {
    pub version: i32,
    pub preferred_type: i8,
    pub owner_id: Uuid,
}

/// What the scheduler reads of an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemInfo {
    /// `mIsComplete` (`isFinished()`).
    pub complete: bool,
    /// `getPermissions().getOwner()`
    pub owner_id: Uuid,
}

/// The model as the scheduler sees it (`gInventory`, the categories' fetch
/// state, the appearance manager's base outfit). Reads are the C++ getters;
/// the three writes are the ones the scheduler makes, applied at once.
pub trait FetchModel {
    fn root_folder_id(&self) -> Uuid;
    fn library_root_folder_id(&self) -> Uuid;
    /// `getLibraryOwnerID()` (the legacy scheduler's library test).
    fn library_owner_id(&self) -> Uuid;
    fn marketplace_listings_id(&self) -> Uuid;
    /// `findCategoryUUIDForType(FT_CURRENT_OUTFIT)`
    fn current_outfit_id(&self) -> Uuid;
    /// `LLAppearanceMgr::getBaseOutfitUUID()`
    fn base_outfit_id(&self) -> Uuid;
    fn category(&self, id: &Uuid) -> Option<CategoryInfo>;
    /// `getDirectDescendentsOf`'s category array, in model order; `None` when
    /// the model has no array for the id.
    fn child_categories(&self, id: &Uuid) -> Option<Vec<Uuid>>;
    fn item(&self, id: &Uuid) -> Option<ItemInfo>;
    fn is_object_descendent_of(&self, obj: &Uuid, cat: &Uuid) -> bool;
    /// `LLViewerInventoryCategory::getFetching()` (may reset an expired mark).
    fn get_fetching(&mut self, id: &Uuid) -> CategoryFetch;
    /// `LLViewerInventoryCategory::setFetching()`
    fn set_fetching(&mut self, id: &Uuid, state: CategoryFetch);
    /// `LLViewerInventoryCategory::setVersion()`
    fn set_version(&mut self, id: &Uuid, version: i32);
}

/// The frame budget's clock: `LLTimer::getTotalSeconds()` in
/// `bulkFetchViaAis`, read before the dispatch loops and after each entry.
pub trait BudgetClock {
    fn total_seconds(&mut self) -> f64;
}

/// The environment the scheduler reads: settings, the region and its
/// capabilities, the session state.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// `use_http_inventory()` (forced on in Second Life).
    pub use_http_inventory: bool,
    /// `AISAPI::isAvailable()`, see [`ais_available`].
    pub ais_available: bool,
    /// `gAgent.getRegion()` is set.
    pub region: bool,
    /// `getRegion()->capabilitiesReceived()`
    pub capabilities_received: bool,
    /// `gDisconnected`
    pub disconnected: bool,
    /// `LLApp::isExiting()`
    pub exiting: bool,
    /// `LLGridManager::isInSecondLife()`
    pub in_second_life: bool,
    /// `PoolSizeAIS`
    pub pool_size_ais: u32,
    /// `BatchSizeAIS3`
    pub batch_size_ais3: i32,
    /// `LLStartUp::getStartupState() > STATE_WEARABLES_WAIT`
    pub past_wearables_wait: bool,
    /// `gAgent.getID()`
    pub agent_id: Uuid,
    /// `InventorySortOrder` (`LLInventoryPanel::DEFAULT_SORT_ORDER`)
    pub sort_order: u32,
    /// The region has `InventoryAPIv3` (`getInvCap()` is not empty).
    pub inventory_cap: bool,
    /// The region has `LibraryAPIv3` (`getLibCap()` is not empty).
    pub library_cap: bool,
    /// `FetchInventoryDescendents2`
    pub cap_fid2: String,
    /// `FetchLibDescendents2`
    pub cap_fld2: String,
    /// `FetchInventory2`
    pub cap_fi2: String,
    /// `FetchLib2`
    pub cap_fl2: String,
}

/// `AISAPI::isAvailable()`: a region, `FSUseAis3Api` or Second Life (the
/// setting matters only off Second Life), and the region's `InventoryAPIv3`.
pub fn ais_available(
    region: bool,
    fs_use_ais3_api: bool,
    in_second_life: bool,
    has_inventory_cap: bool,
) -> bool {
    region && (fs_use_ais3_api || in_second_life) && has_inventory_cap
}

/// What `LLViewerInventoryCategory::fetch()` does once its gate passed
/// ([`super::timer::CategoryFetchState::fetch_gate`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchPath {
    /// `LLInventoryModelBackgroundFetch::start(id, false)`
    Start,
    /// The UDP `FetchInventoryDescendents` message (`<FS:Ansariel> [UDP-Msg]`):
    /// `SortOrder` = sort order & 1, `FetchFolders` false, `FetchItems` true.
    Udp,
}

/// `fetch()` after its gate: the background fetch when the region has
/// `FetchInventoryDescendents2` or AIS is available, else UDP.
pub fn fetch_path(region_cap_fid2: &str, ais_available: bool) -> FetchPath {
    if !region_cap_fid2.is_empty() || ais_available {
        FetchPath::Start
    } else {
        FetchPath::Udp
    }
}

/// Everything the scheduler asks the outside to do, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Call the `AISAPI` command function `command` (R3a's
    /// `ais::command::Command`, built and run by the AIS command layer, the
    /// coprocedure pool through [`super::ais::PostponedQueue`]); report the
    /// ids its completion is called with through
    /// [`BackgroundFetch::ais_complete`]. The scheduler only issues commands
    /// the command layer will not refuse ([`AisFetch::refusal`]).
    IssueAis { ticket: u64, command: AisFetch },
    /// Call a caller's `nullary_func_t`, then [`BackgroundFetch::resume`].
    /// The C++ calls it first thing in the completion lambda, and the
    /// callback may call back into the scheduler, so the scheduler stops
    /// here: it is always the last action of its list, and the rest of the
    /// completion (and of the call that issued the command, when it
    /// completed at once) runs in `resume`, after the callback.
    Callback { token: CallerToken },
    /// `markFetchStarted()` found the fetch inactive: `mFetchStartTimer.reset()`.
    FetchStarted,
    /// The first time the bulk fetch is complete with every recursive folder
    /// fetched: `mInitialFetchDuration = mFetchStartTimer.getElapsedTimeF32()`.
    InitialFetchDuration,
    /// `bulkFetch()` sent a batch: `mFetchTimer.reset()` (read only by the
    /// `#ifdef OPENSIM` UDP loop).
    FetchTimerReset,
    /// A warning the C++ logs (`LL_WARNS`), with the id it names.
    Warn { warning: Warning, id: Uuid },
    /// `LLNotificationsUtil::add(name)`
    Notify { name: &'static str },
    /// `mFoldersFetchedSignal()` (the fetch completion callback).
    FetchCompleted,
    /// `gIdleCallbacks.addFunction(backgroundFetchCB)`: from now on run
    /// [`BackgroundFetch::background_fetch`] every frame (never removed).
    AddIdle,
    /// `gInventory.requestPost(false, url, body, handler, label)` with a new
    /// `BGFolderHttpHandler` (`folder` true) or `BGItemHttpHandler`.
    LegacyPost {
        handler: u64,
        folder: bool,
        url: String,
        body: Vec<u8>,
        label: &'static str,
    },
    /// `gInventory.handleResponses(false)` (then `notifyObservers()` when
    /// `notify_observers`): pump the legacy handlers, then call
    /// [`BackgroundFetch::bulk_fetch_after_pump`].
    LegacyPump { notify_observers: bool },
    /// `gInventory.notifyObservers()` (a legacy handler off Second Life).
    NotifyObservers,
}

/// The warnings of the C++ scheduler ([`Action::Warn`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Warning {
    /// `incrFetchCount()`: "Inventory fetch count fell below zero (0)."
    /// (`LL_WARNS_ONCE`); the id is null.
    FetchCountBelowZero = 1,
    /// `onAISFolderCalback()`: "Unexpected folder response for <id>".
    UnexpectedFolderResponse = 2,
    /// `onAISFolderCalback()`: "Failed to download folder: <id> Requesting
    /// known content separately".
    FolderDownloadFailed = 3,
    /// `fetchFolderAndLinks()`'s lambda: "Failed to fetch category links <id>".
    CategoryLinksFailed = 4,
    /// `BGFolderHttpHandler::processFailure()` on a 403: "Failed to get AIS2
    /// cap"; the id is null.
    NoAis2Cap = 5,
}

/// The rest of a completion, or of the call that issued it, after an
/// [`Action::Callback`] ([`BackgroundFetch::resume`]).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Resume {
    /// `fetchCOF`'s lambda after `callback()`.
    Cof { response_id: Uuid },
    /// `fetchFolderAndLinks`'s lambda after `callback()`.
    FolderAndLinks { category: Uuid, response_id: Uuid },
    /// The end of `fetchCOF` / `fetchFolderAndLinks` after their AIS call
    /// completed at once: `markFetchStarted()`, `mFolderFetchActive`, the
    /// idle callback.
    Tail,
}

/// What [`BackgroundFetch::ais_call`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AisCall {
    Issued,
    /// Refused: the completion ran at once, inside the call.
    Completed,
    /// Refused, and the completion stopped at a caller's callback.
    Yielded,
}

/// What an AIS command's completion does.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Completion {
    /// `onAISFolderCalback(request, response, fetch_type)`; the null request
    /// is the orphans fetch.
    Folder {
        request: Uuid,
        fetch_type: FetchType,
    },
    /// `onAISContentCalback(category, children, response, fetch_type)`
    Content { children: Vec<Uuid> },
    /// `ais_simple_item_callback`
    Item,
    /// `fetchCOF`'s lambda.
    Cof { callback: Option<CallerToken> },
    /// `fetchFolderAndLinks`'s lambda.
    FolderAndLinks {
        category: Uuid,
        callback: Option<CallerToken>,
    },
}

#[derive(Clone, Debug)]
struct Pending {
    completion: Completion,
    command: AisFetch,
}

#[derive(Clone, Debug)]
struct LegacyHandler {
    body: FolderBody,
    recursive: Vec<Uuid>,
}

/// The scheduler's private state, as the R3b recorder prints it (`jrn sch`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedState {
    pub bg_active: bool,
    pub folder_active: bool,
    pub inv_started: bool,
    pub lib_started: bool,
    pub mkt_started: bool,
    pub all_fetched: bool,
    pub fetch_count: i32,
    pub folder_fetch_count: i32,
    pub folder_queue: Vec<QueueEntry>,
    pub item_queue: Vec<QueueEntry>,
    pub expected: Vec<Uuid>,
    pub forced: Vec<Uuid>,
}

/// `LLInventoryModelBackgroundFetch`, with its open AIS commands (the
/// completions of the lambdas it passes to `AISAPI`).
#[derive(Debug, Default)]
pub struct BackgroundFetch {
    recursive_inventory_fetch_started: bool,
    recursive_library_fetch_started: bool,
    recursive_marketplace_fetch_started: bool,
    all_recursive_folders_fetched: bool,
    background_fetch_active: bool,
    folder_fetch_active: bool,
    fetch_count: i32,
    last_fetch_count: i32,
    fetch_folder_count: i32,
    fetch_folder_queue: VecDeque<QueueEntry>,
    fetch_item_queue: VecDeque<QueueEntry>,
    force_fetch_set: BTreeSet<Uuid>,
    expected_folder_ids: Vec<Uuid>,
    idle_registered: bool,
    initial_fetch_duration_captured: bool,
    /// The continuations of the open [`Action::Callback`]s, innermost last;
    /// each frame runs in order on [`BackgroundFetch::resume`].
    resume: Vec<Vec<Resume>>,

    next_ticket: u64,
    pending: BTreeMap<u64, Pending>,
    next_handler: u64,
    legacy: BTreeMap<u64, LegacyHandler>,
    legacy_item_handlers: BTreeSet<u64>,
    pump_pending: bool,

    actions: Vec<Action>,
}

/// The model and environment of one call.
pub struct Ctx<'a> {
    pub model: &'a mut dyn FetchModel,
    pub env: &'a Env,
}

impl<'a> Ctx<'a> {
    pub fn new(model: &'a mut dyn FetchModel, env: &'a Env) -> Self {
        Ctx { model, env }
    }
}

fn entry(id: Uuid, fetch_type: FetchType, is_category: bool) -> QueueEntry {
    QueueEntry {
        id,
        is_category,
        fetch_type,
    }
}

impl BackgroundFetch {
    pub fn new() -> Self {
        Self::default()
    }

    /// The actions produced since the last call, in order.
    pub fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    fn add_idle(&mut self) {
        if !self.idle_registered {
            self.idle_registered = true;
            self.actions.push(Action::AddIdle);
        }
    }

    /// Whether `backgroundFetchCB` is in the idle callbacks.
    pub fn idle_registered(&self) -> bool {
        self.idle_registered
    }

    /// `mBackgroundFetchActive`: the idle callback has work (the first test
    /// of `backgroundFetch()`).
    pub fn background_fetch_active(&self) -> bool {
        self.background_fetch_active
    }

    /// `markFetchStarted()`
    fn mark_fetch_started(&mut self) {
        if !self.background_fetch_active {
            self.actions.push(Action::FetchStarted);
        }
        self.background_fetch_active = true;
    }

    /// The bulk fetch is complete: the initial fetch duration, the first
    /// time every recursive folder is fetched, and the fetch goes inactive.
    fn bulk_fetch_complete(&mut self) {
        if self.all_recursive_folders_fetched && !self.initial_fetch_duration_captured {
            self.initial_fetch_duration_captured = true;
            self.actions.push(Action::InitialFetchDuration);
        }
        self.background_fetch_active = false;
    }

    fn warn(&mut self, warning: Warning, id: Uuid) {
        self.actions.push(Action::Warn { warning, id });
    }

    // ---- the folder queue, for the `#ifdef OPENSIM` UDP loop ---------------

    /// `mFetchFolderQueue.front()`
    pub fn folder_queue_front(&self) -> Option<QueueEntry> {
        self.fetch_folder_queue.front().copied()
    }

    /// `mFetchFolderQueue.pop_front()`
    pub fn folder_queue_pop_front(&mut self) {
        self.fetch_folder_queue.pop_front();
    }

    /// `mFetchFolderQueue.push_back(entry)`
    pub fn folder_queue_push_back(&mut self, entry: QueueEntry) {
        self.fetch_folder_queue.push_back(entry);
    }

    // ---- predicates ------------------------------------------------------

    /// `isBulkFetchProcessingComplete()`
    pub fn is_bulk_fetch_processing_complete(&self) -> bool {
        self.fetch_folder_queue.is_empty()
            && self.fetch_item_queue.is_empty()
            && self.fetch_count <= 0
    }

    /// `isFolderFetchProcessingComplete()`
    pub fn is_folder_fetch_processing_complete(&self) -> bool {
        self.fetch_folder_queue.is_empty() && self.fetch_folder_count <= 0
    }

    pub fn library_fetch_started(&self) -> bool {
        self.recursive_library_fetch_started
    }

    pub fn library_fetch_completed(&self, model: &dyn FetchModel) -> bool {
        self.library_fetch_started()
            && self.fetch_queue_contains_no_descendents_of(model, &model.library_root_folder_id())
    }

    pub fn library_fetch_in_progress(&self, model: &dyn FetchModel) -> bool {
        self.library_fetch_started() && !self.library_fetch_completed(model)
    }

    pub fn inventory_fetch_started(&self) -> bool {
        self.recursive_inventory_fetch_started
    }

    pub fn inventory_fetch_completed(&self, model: &dyn FetchModel) -> bool {
        self.inventory_fetch_started()
            && self.fetch_queue_contains_no_descendents_of(model, &model.root_folder_id())
    }

    pub fn inventory_fetch_in_progress(&self, model: &dyn FetchModel) -> bool {
        self.inventory_fetch_started() && !self.inventory_fetch_completed(model)
    }

    /// `isEverythingFetched()`
    pub fn is_everything_fetched(&self) -> bool {
        self.all_recursive_folders_fetched
    }

    /// `folderFetchActive()`
    pub fn folder_fetch_active(&self) -> bool {
        self.folder_fetch_active
    }

    /// `fetchQueueContainsNoDescendentsOf()`
    pub fn fetch_queue_contains_no_descendents_of(
        &self,
        model: &dyn FetchModel,
        cat_id: &Uuid,
    ) -> bool {
        !self
            .fetch_folder_queue
            .iter()
            .chain(self.fetch_item_queue.iter())
            .any(|e| model.is_object_descendent_of(&e.id, cat_id))
    }

    /// The private state, for the recorder's `sch` lines.
    pub fn state(&self) -> SchedState {
        SchedState {
            bg_active: self.background_fetch_active,
            folder_active: self.folder_fetch_active,
            inv_started: self.recursive_inventory_fetch_started,
            lib_started: self.recursive_library_fetch_started,
            mkt_started: self.recursive_marketplace_fetch_started,
            all_fetched: self.all_recursive_folders_fetched,
            fetch_count: self.fetch_count,
            folder_fetch_count: self.fetch_folder_count,
            folder_queue: self.fetch_folder_queue.iter().copied().collect(),
            item_queue: self.fetch_item_queue.iter().copied().collect(),
            expected: self.expected_folder_ids.clone(),
            forced: self.force_fetch_set.iter().copied().collect(),
        }
    }

    // ---- counts ----------------------------------------------------------

    /// `incrFetchCount()`
    pub fn incr_fetch_count(&mut self, fetching: i32) {
        self.fetch_count += fetching;
        if self.fetch_count < 0 {
            self.warn(Warning::FetchCountBelowZero, NULL_ID);
            self.fetch_count = 0;
        }
    }

    /// `incrFetchFolderCount()`: the underflow check reads the fetch count,
    /// as in the C++.
    pub fn incr_fetch_folder_count(&mut self, fetching: i32) {
        self.incr_fetch_count(fetching);
        self.fetch_folder_count += fetching;
        if self.fetch_count < 0 {
            self.fetch_folder_count = 0;
        }
    }

    // ---- queue entry points ----------------------------------------------

    /// `addRequestAtFront()`
    pub fn add_request_at_front(&mut self, id: Uuid, recursive: bool, is_category: bool) {
        let t = if recursive {
            FetchType::Recursive
        } else {
            FetchType::Default
        };
        if is_category {
            self.fetch_folder_queue.push_front(entry(id, t, true));
        } else {
            self.fetch_item_queue.push_front(entry(id, t, false));
        }
    }

    /// `addRequestAtBack()`
    pub fn add_request_at_back(&mut self, id: Uuid, recursive: bool, is_category: bool) {
        let t = if recursive {
            FetchType::Recursive
        } else {
            FetchType::Default
        };
        if is_category {
            self.fetch_folder_queue.push_back(entry(id, t, true));
        } else {
            self.fetch_item_queue.push_back(entry(id, t, false));
        }
    }

    /// `start(id, recursive)`; the null id starts the login walk.
    pub fn start(&mut self, cx: &mut Ctx<'_>, id: Uuid, recursive: bool) {
        let cat = cx.model.category(&id);
        let ais = cx.env.ais_available;
        if cat.is_some() || (id == NULL_ID && !self.is_everything_fetched()) {
            self.mark_fetch_started();
            self.folder_fetch_active = true;
            let recursion_type = if recursive {
                FetchType::Recursive
            } else {
                FetchType::Default
            };
            if id == NULL_ID {
                if !self.recursive_inventory_fetch_started {
                    self.recursive_inventory_fetch_started |= recursive;
                    let root = cx.model.root_folder_id();
                    if recursive && ais {
                        // root and content separately: most system folders
                        // are requested independently
                        self.fetch_folder_queue.push_front(entry(
                            root,
                            FetchType::FolderAndContent,
                            true,
                        ));
                    } else {
                        self.fetch_folder_queue
                            .push_back(entry(root, recursion_type, true));
                    }
                    self.add_idle();
                }
                if !self.recursive_library_fetch_started {
                    self.recursive_library_fetch_started |= recursive;
                    let lib = cx.model.library_root_folder_id();
                    self.fetch_folder_queue
                        .push_back(entry(lib, recursion_type, true));
                    self.add_idle();
                }
            } else if recursive && cat.is_some_and(|c| c.preferred_type == FT_MARKETPLACE_LISTINGS)
            {
                if self.fetch_folder_queue.back().is_none_or(|b| b.id != id) {
                    if recursive && ais {
                        self.fetch_folder_queue.push_front(entry(
                            id,
                            FetchType::FolderAndContent,
                            true,
                        ));
                    } else {
                        self.fetch_folder_queue
                            .push_front(entry(id, recursion_type, true));
                    }
                    self.add_idle();
                    self.recursive_marketplace_fetch_started = true;
                }
            } else {
                if ais {
                    if self.fetch_folder_queue.back().is_none_or(|b| b.id != id) {
                        // On AIS the root goes to the top; follow-ups go back
                        self.fetch_folder_queue
                            .push_back(entry(id, recursion_type, true));
                        self.add_idle();
                    }
                } else if self.fetch_folder_queue.front().is_none_or(|f| f.id != id) {
                    // Specific folder requests go to front of queue.
                    self.fetch_folder_queue
                        .push_front(entry(id, recursion_type, true));
                    self.add_idle();
                }
                if id == cx.model.library_root_folder_id() {
                    self.recursive_library_fetch_started |= recursive;
                }
                if id == cx.model.root_folder_id() {
                    self.recursive_inventory_fetch_started |= recursive;
                }
            }
        } else if let Some(item) = cx.model.item(&id) {
            if !item.complete {
                self.schedule_item_fetch(id, false);
            }
        }
    }

    /// `scheduleFolderFetch()`
    pub fn schedule_folder_fetch(&mut self, cat_id: Uuid, forced: bool) {
        if self
            .fetch_folder_queue
            .front()
            .is_none_or(|f| f.id != cat_id)
        {
            self.mark_fetch_started();
            self.folder_fetch_active = true;
            if forced {
                // check if already requested
                if self.force_fetch_set.insert(cat_id) {
                    self.fetch_folder_queue
                        .push_front(entry(cat_id, FetchType::Forced, true));
                }
            } else {
                // version presence acts as duplicate prevention for normal fetches
                self.fetch_folder_queue
                    .push_front(entry(cat_id, FetchType::Default, true));
            }
            self.add_idle();
        }
    }

    /// `scheduleItemFetch()`
    pub fn schedule_item_fetch(&mut self, item_id: Uuid, forced: bool) {
        if self
            .fetch_item_queue
            .front()
            .is_none_or(|f| f.id != item_id)
        {
            self.mark_fetch_started();
            if forced {
                if self.force_fetch_set.insert(item_id) {
                    self.fetch_item_queue
                        .push_front(entry(item_id, FetchType::Forced, false));
                }
            } else {
                // 'isFinished' being set acts as duplicate prevention
                self.fetch_item_queue
                    .push_front(entry(item_id, FetchType::Default, false));
            }
            self.add_idle();
        }
    }

    /// `fetchFolderAndLinks(cat_id, callback)` (AIS only); `None` is `no_op`.
    pub fn fetch_folder_and_links(
        &mut self,
        cx: &mut Ctx<'_>,
        cat_id: Uuid,
        callback: Option<CallerToken>,
    ) {
        if cx.model.category(&cat_id).is_some() {
            // Mark folder (update timer) so that background fetch won't request it
            cx.model.set_fetching(&cat_id, CategoryFetch::Recursive);
        }
        self.incr_fetch_folder_count(1);
        self.expected_folder_ids.push(cat_id);
        let call = self.ais_call(
            cx,
            AisFetch::FetchCategoryLinks {
                category_id: cat_id,
            },
            Completion::FolderAndLinks {
                category: cat_id,
                callback,
            },
        );
        self.issue_tail(call);
    }

    /// The end of `fetchCOF` and `fetchFolderAndLinks`: start the idle loop
    /// to track completion, at once or after the callback of a completion
    /// that ran inside the call.
    fn issue_tail(&mut self, call: AisCall) {
        if call == AisCall::Yielded {
            if let Some(frame) = self.resume.last_mut() {
                frame.push(Resume::Tail);
                return;
            }
        }
        self.tail();
    }

    fn tail(&mut self) {
        self.mark_fetch_started();
        self.folder_fetch_active = true;
        self.add_idle();
    }

    /// `fetchCOF(callback)` (AIS only).
    pub fn fetch_cof(&mut self, cx: &mut Ctx<'_>, callback: Option<CallerToken>) {
        let cat_id = cx.model.current_outfit_id();
        if cx.model.category(&cat_id).is_some() {
            cx.model.set_fetching(&cat_id, CategoryFetch::Recursive);
        }
        self.incr_fetch_folder_count(1);
        self.expected_folder_ids.push(cat_id);
        let call = self.ais_call(cx, AisFetch::FetchCof, Completion::Cof { callback });
        self.issue_tail(call);
    }

    /// `findLostItems()`
    pub fn find_lost_items(&mut self) {
        self.mark_fetch_started();
        self.folder_fetch_active = true;
        self.fetch_folder_queue
            .push_back(entry(NULL_ID, FetchType::Recursive, true));
        self.add_idle();
    }

    /// `setAllFoldersFetched()`
    pub fn set_all_folders_fetched(&mut self) {
        if self.recursive_inventory_fetch_started && self.recursive_library_fetch_started {
            self.all_recursive_folders_fetched = true;
        }
        self.folder_fetch_active = false;
        if self.is_bulk_fetch_processing_complete() {
            self.bulk_fetch_complete();
        }
        // For now only informs about initial fetch being done
        self.actions.push(Action::FetchCompleted);
    }

    // ---- AIS commands ----------------------------------------------------

    fn ais_call(&mut self, cx: &mut Ctx<'_>, command: AisFetch, completion: Completion) -> AisCall {
        if command
            .refusal(cx.env.inventory_cap, cx.env.library_cap)
            .is_some()
        {
            // No capability, or an empty subset: the completion with null,
            // at once, inside this call (the re-entrancy of s17).
            return if self.complete(cx, completion, NULL_ID) {
                AisCall::Yielded
            } else {
                AisCall::Completed
            };
        }
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.pending.insert(
            ticket,
            Pending {
                completion,
                command: command.clone(),
            },
        );
        self.actions.push(Action::IssueAis { ticket, command });
        AisCall::Issued
    }

    /// The command's completion was called (`InvokeAISCommandCoro` after the
    /// update was applied, or at once when `gDisconnected`): `callback_ids`
    /// are the ids the command layer calls it with (R3a's
    /// `CommandResult::callbacks`); the scheduler's completions are
    /// single-id, so the first one counts, null when there is none.
    pub fn ais_complete(&mut self, cx: &mut Ctx<'_>, ticket: u64, callback_ids: &[Uuid]) {
        let Some(p) = self.pending.remove(&ticket) else {
            return;
        };
        let id = callback_ids.first().copied().unwrap_or(NULL_ID);
        self.complete(cx, p.completion, id);
    }

    /// `LLCoros::checkStop()` threw inside the command's update: its
    /// completion never runs (the counts it would have released stay raised).
    pub fn ais_abandoned(&mut self, ticket: u64) {
        self.pending.remove(&ticket);
    }

    /// The command of an open ticket.
    pub fn ais_pending(&self, ticket: u64) -> Option<&AisFetch> {
        self.pending.get(&ticket).map(|p| &p.command)
    }

    /// Runs a completion; true when it stopped at a caller's callback
    /// ([`Action::Callback`]; the rest is in [`resume`](Self::resume)).
    fn complete(&mut self, cx: &mut Ctx<'_>, completion: Completion, response_id: Uuid) -> bool {
        match completion {
            Completion::Folder {
                request,
                fetch_type,
            } => self.on_ais_folder_callback(cx, request, response_id, fetch_type),
            Completion::Content { children } => {
                self.on_ais_content_callback(cx, &children, response_id)
            }
            Completion::Item => {
                // ais_simple_item_callback
                self.incr_fetch_count(-1);
            }
            Completion::Cof { callback } => {
                let rest = Resume::Cof { response_id };
                if let Some(token) = callback {
                    return self.yield_to(token, rest);
                }
                self.run_resume(cx, rest);
            }
            Completion::FolderAndLinks { category, callback } => {
                let rest = Resume::FolderAndLinks {
                    category,
                    response_id,
                };
                if let Some(token) = callback {
                    return self.yield_to(token, rest);
                }
                self.run_resume(cx, rest);
            }
        }
        false
    }

    /// Stops at a caller's callback: the C++ runs it, then [`resume`](Self::resume).
    fn yield_to(&mut self, token: CallerToken, rest: Resume) -> bool {
        self.actions.push(Action::Callback { token });
        self.resume.push(vec![rest]);
        true
    }

    /// The rest of the completion (and of the call that issued it) whose
    /// [`Action::Callback`] the caller has just run. Does nothing when no
    /// callback is open.
    pub fn resume(&mut self, cx: &mut Ctx<'_>) {
        let Some(frame) = self.resume.pop() else {
            return;
        };
        for rest in frame {
            self.run_resume(cx, rest);
        }
    }

    fn run_resume(&mut self, cx: &mut Ctx<'_>, rest: Resume) {
        match rest {
            Resume::Cof { response_id } => {
                let cat_id = cx.model.current_outfit_id();
                self.on_ais_folder_callback(cx, cat_id, response_id, FetchType::Default);
                if response_id != NULL_ID {
                    // COF might have fetched the base outfit folder through a
                    // link, but not its content: make sure it's up to date
                    let base = cx.model.base_outfit_id();
                    if base != NULL_ID {
                        let cat = cx.model.category(&base);
                        if cat.is_none_or(|c| c.version == VERSION_UNKNOWN) {
                            // no_op: never stops at a callback
                            self.fetch_folder_and_links(cx, base, None);
                        }
                    }
                }
            }
            Resume::FolderAndLinks {
                category,
                response_id,
            } => {
                if response_id == NULL_ID {
                    self.warn(Warning::CategoryLinksFailed, category);
                }
                self.on_ais_folder_callback(cx, category, response_id, FetchType::Default);
            }
            Resume::Tail => self.tail(),
        }
    }

    /// `onAISContentCalback()`
    fn on_ais_content_callback(
        &mut self,
        cx: &mut Ctx<'_>,
        content_ids: &[Uuid],
        response_id: Uuid,
    ) {
        // Don't emplace_front on failure - this may run inside bulkFetchViaAis
        self.incr_fetch_folder_count(-1);
        for folder in content_ids {
            if let Some(pos) = self.expected_folder_ids.iter().position(|e| e == folder) {
                self.expected_folder_ids.remove(pos);
            }
            if cx.model.category(folder).is_some() {
                cx.model.set_fetching(folder, CategoryFetch::None);
            }
            if response_id == NULL_ID {
                // Failed to fetch, get it individually
                self.fetch_folder_queue
                    .push_back(entry(*folder, FetchType::Recursive, true));
            } else if let Some(children) = cx.model.child_categories(folder) {
                // verify the descendants are fetched fully (depth limit)
                for c in children {
                    self.fetch_folder_queue
                        .push_back(entry(c, FetchType::Recursive, true));
                }
            }
        }
        if !self.fetch_folder_queue.is_empty() {
            self.background_fetch_active = true;
            self.folder_fetch_active = true;
            self.add_idle();
        }
    }

    /// `onAISFolderCalback()`
    fn on_ais_folder_callback(
        &mut self,
        cx: &mut Ctx<'_>,
        request_id: Uuid,
        response_id: Uuid,
        fetch_type: FetchType,
    ) {
        self.incr_fetch_folder_count(-1);
        if let Some(pos) = self
            .expected_folder_ids
            .iter()
            .position(|e| *e == request_id)
        {
            self.expected_folder_ids.remove(pos);
        } else {
            // ais shouldn't respond twice (llassert, a warning)
            self.warn(Warning::UnexpectedFolderResponse, request_id);
        }

        if request_id == NULL_ID {
            // orphans, no other actions needed
            return;
        }

        let mut new_state = CategoryFetch::None;
        let mut request_descendants = false;
        if response_id == NULL_ID {
            // Failure
            if fetch_type == FetchType::Recursive {
                // A full recursive request failed: folder and content separately
                self.fetch_folder_queue.push_back(entry(
                    request_id,
                    FetchType::FolderAndContent,
                    true,
                ));
            } else if fetch_type == FetchType::FolderAndContent {
                self.warn(Warning::FolderDownloadFailed, request_id);
                self.fetch_folder_queue.push_back(entry(
                    request_id,
                    FetchType::ContentRecursive,
                    true,
                ));
                // the version stops the viewer requesting the folder forever
                if cx
                    .model
                    .category(&request_id)
                    .is_some_and(|c| c.version == VERSION_UNKNOWN)
                {
                    cx.model.set_version(&request_id, 0);
                }
                // back off for a bit in case something force-requests at once
                new_state = CategoryFetch::Failed;
            }
        } else if fetch_type == FetchType::Recursive {
            // Got the folder and content, verify the content
            request_descendants = true;
        } else if fetch_type == FetchType::FolderAndContent {
            // read folder for content request
            self.fetch_folder_queue.push_front(entry(
                request_id,
                FetchType::ContentRecursive,
                true,
            ));
        }

        if request_descendants {
            if let Some(children) = cx.model.child_categories(&request_id) {
                for c in children {
                    self.fetch_folder_queue
                        .push_back(entry(c, FetchType::Recursive, true));
                }
            }
        }

        if !self.fetch_folder_queue.is_empty() {
            self.background_fetch_active = true;
            self.folder_fetch_active = true;
            self.add_idle();
        }

        // done
        if cx.model.category(&request_id).is_some() {
            cx.model.set_fetching(&request_id, new_state);
        }
    }

    // ---- the idle callback -------------------------------------------------

    /// `backgroundFetch()`, the idle callback. On the legacy path with
    /// requests in flight this stops at [`Action::LegacyPump`]; the caller
    /// pumps the handlers and then calls [`bulk_fetch_after_pump`].
    ///
    /// [`bulk_fetch_after_pump`]: BackgroundFetch::bulk_fetch_after_pump
    pub fn background_fetch(&mut self, cx: &mut Ctx<'_>, clock: &mut dyn BudgetClock) {
        if !self.background_fetch_active {
            return;
        }
        if cx.env.use_http_inventory {
            if cx.env.ais_available {
                self.bulk_fetch_via_ais(cx, clock);
            } else if cx.env.region && cx.env.capabilities_received {
                self.bulk_fetch(cx);
            }
        }
        // #ifdef OPENSIM: the UDP loop (not in this build)
    }

    /// `bulkFetchViaAis()`
    fn bulk_fetch_via_ais(&mut self, cx: &mut Ctx<'_>, clock: &mut dyn BudgetClock) {
        if cx.env.disconnected {
            return;
        }
        // Don't have too many requests at once, AIS throttles. Reserve one
        // request for actions outside of fetch (like renames). U32
        // arithmetic: a pool of 0 wraps to the top and clamps to 50.
        let max_concurrent_fetches = cx.env.pool_size_ais.wrapping_sub(1).clamp(1, 50);
        if (self.fetch_count as u32) >= max_concurrent_fetches {
            return;
        }

        // Don't loop for too long (in case of large, fully loaded inventory)
        let mut curent_time = clock.total_seconds();
        let max_time: f64 = if cx.env.past_wearables_wait {
            0.006f32 as f64
        } else {
            1.0
        };
        let end_time = curent_time + max_time;
        let last_fetch_count = self.fetch_count;

        while let Some(&info) = self.fetch_folder_queue.front() {
            if !((self.fetch_count as u32) < max_concurrent_fetches && curent_time < end_time) {
                break;
            }
            self.bulk_fetch_via_ais_entry(cx, info);
            self.fetch_folder_queue.pop_front();
            curent_time = clock.total_seconds();
        }

        // Items even if the recursive fetch isn't done: a folder that keeps
        // timing out must not block them
        while let Some(&info) = self.fetch_item_queue.front() {
            if !((self.fetch_count as u32) < max_concurrent_fetches && curent_time < end_time) {
                break;
            }
            self.bulk_fetch_via_ais_entry(cx, info);
            self.fetch_item_queue.pop_front();
            curent_time = clock.total_seconds();
        }

        if last_fetch_count != self.fetch_count || self.last_fetch_count != self.fetch_count {
            // "Total active fetches" (debug log)
            self.last_fetch_count = self.fetch_count;
        }

        if self.is_folder_fetch_processing_complete() && self.folder_fetch_active {
            if !self.recursive_inventory_fetch_started || self.recursive_marketplace_fetch_started {
                self.set_all_folders_fetched();
            } else {
                // the marketplace after the main inventory, unless a floater
                // asked for it
                self.recursive_marketplace_fetch_started = true;
                let mkt = cx.model.marketplace_listings_id();
                if mkt != NULL_ID {
                    self.fetch_folder_queue.push_front(entry(
                        mkt,
                        FetchType::FolderAndContent,
                        true,
                    ));
                } else {
                    self.set_all_folders_fetched();
                }
            }
        }

        if self.is_bulk_fetch_processing_complete() {
            self.bulk_fetch_complete();
        }
    }

    /// `bulkFetchViaAis(const FetchQueueInfo&)`
    fn bulk_fetch_via_ais_entry(&mut self, cx: &mut Ctx<'_>, info: QueueEntry) {
        if info.is_category {
            let cat_id = info.id;
            if cat_id == NULL_ID {
                self.incr_fetch_folder_count(1);
                self.expected_folder_ids.push(cat_id);
                // Lost and found
                self.ais_call(
                    cx,
                    AisFetch::FetchOrphans,
                    Completion::Folder {
                        request: NULL_ID,
                        fetch_type: FetchType::Default,
                    },
                );
            } else if let Some(cat) = cx.model.category(&cat_id) {
                let library = cat.owner_id == ALEXANDRIA_LINDEN_ID;
                if info.fetch_type == FetchType::ContentRecursive {
                    // fetch content only, ignore cat itself
                    let categories = cx.model.child_categories(&cat_id);
                    let target_state = CategoryFetch::Recursive;
                    let mut content_done = true;
                    // Top limit is 'as many as you can put into url'
                    let batch_limit = cx.env.batch_size_ais3.clamp(1, 40) as usize;
                    let mut children = Vec::new();
                    if let Some(cats) = &categories {
                        for child in cats {
                            let Some(c) = cx.model.category(child) else {
                                continue;
                            };
                            if c.version != VERSION_UNKNOWN
                                || cx.model.get_fetching(child) >= target_state
                            {
                                continue;
                            }
                            if c.preferred_type == FT_MARKETPLACE_LISTINGS {
                                // special case, marketplace will fetch that as needed
                                continue;
                            }
                            children.push(*child);
                            self.expected_folder_ids.push(*child);
                            cx.model.set_fetching(child, target_state);
                            if children.len() >= batch_limit {
                                content_done = false;
                                break;
                            }
                        }
                    }

                    if !children.is_empty() {
                        // increment before call in case of immediate callback
                        self.incr_fetch_folder_count(1);
                        self.ais_call(
                            cx,
                            AisFetch::FetchCategorySubset {
                                category_id: cat_id,
                                children: children.clone(),
                                library,
                                recursive: true,
                                depth: 0,
                            },
                            Completion::Content { children },
                        );
                    }

                    if content_done {
                        // something else might have downloaded folders: verify
                        // every complete child has its children done as well
                        if let Some(cats) = &categories {
                            for child in cats {
                                if cx
                                    .model
                                    .category(child)
                                    .is_some_and(|c| c.version != VERSION_UNKNOWN)
                                {
                                    self.fetch_folder_queue.push_back(entry(
                                        *child,
                                        FetchType::Recursive,
                                        true,
                                    ));
                                }
                            }
                        }
                    } else {
                        // send it back to get the rest
                        self.fetch_folder_queue.push_back(entry(
                            cat_id,
                            FetchType::ContentRecursive,
                            true,
                        ));
                    }
                } else if cat.version == VERSION_UNKNOWN || info.fetch_type == FetchType::Forced {
                    let target_state = if info.fetch_type > FetchType::ContentRecursive {
                        CategoryFetch::Recursive
                    } else {
                        CategoryFetch::Normal
                    };
                    // start again if we did a non-recursive fetch before
                    if cx.model.get_fetching(&cat_id) < target_state {
                        self.incr_fetch_folder_count(1);
                        cx.model.set_fetching(&cat_id, target_state);
                        self.expected_folder_ids.push(cat_id);
                        self.ais_call(
                            cx,
                            AisFetch::FetchCategoryChildren {
                                category_id: cat_id,
                                library,
                                recursive: info.fetch_type == FetchType::Recursive,
                                depth: 0,
                            },
                            Completion::Folder {
                                request: cat_id,
                                fetch_type: info.fetch_type,
                            },
                        );
                    }
                } else if info.fetch_type == FetchType::Recursive
                    || info.fetch_type == FetchType::FolderAndContent
                {
                    // Already fetched, check if anything inside needs fetching
                    if let Some(cats) = cx.model.child_categories(&cat_id) {
                        for c in cats {
                            // not push_front, to not cause an infinite loop
                            self.fetch_folder_queue
                                .push_back(entry(c, FetchType::Recursive, true));
                        }
                    }
                }
            }
            // else: an unknown folder is dropped
        } else {
            match cx.model.item(&info.id) {
                Some(item) => {
                    if !item.complete || info.fetch_type == FetchType::Forced {
                        self.fetch_count += 1;
                        let library = item.owner_id != cx.env.agent_id;
                        self.ais_call(
                            cx,
                            AisFetch::FetchItem {
                                item_id: info.id,
                                library,
                            },
                            Completion::Item,
                        );
                    }
                }
                None => {
                    // unknown: assume the agent's inventory
                    self.fetch_count += 1;
                    self.ais_call(
                        cx,
                        AisFetch::FetchItem {
                            item_id: info.id,
                            library: false,
                        },
                        Completion::Item,
                    );
                }
            }
        }

        if info.fetch_type == FetchType::Forced {
            self.force_fetch_set.remove(&info.id);
        }
    }

    // ---- the legacy scheduler -------------------------------------------

    /// `bulkFetch()` up to `handleResponses`.
    fn bulk_fetch(&mut self, cx: &mut Ctx<'_>) {
        if !cx.env.region || cx.env.disconnected || cx.env.exiting {
            return;
        }
        if self.fetch_count != 0 {
            // Process completed background HTTP requests first.
            self.pump_pending = true;
            self.actions.push(Action::LegacyPump {
                notify_observers: cx.env.in_second_life,
            });
            return;
        }
        self.bulk_fetch_batches(cx);
    }

    /// The rest of `bulkFetch()` after the caller pumped the legacy handlers
    /// (only after an [`Action::LegacyPump`]).
    pub fn bulk_fetch_after_pump(&mut self, cx: &mut Ctx<'_>) {
        if !std::mem::take(&mut self.pump_pending) {
            return;
        }
        self.bulk_fetch_batches(cx);
    }

    fn bulk_fetch_batches(&mut self, cx: &mut Ctx<'_>) {
        if self.fetch_count > LEGACY_MAX_CONCURRENT_FETCHES {
            return;
        }
        let mut item_count: u32 = 0;
        let mut folder_count: u32 = 0;
        let sort_order = (cx.env.sort_order & 0x1) as i32;
        let agent = cx.env.agent_id;
        let mut recursive_cats: Vec<Uuid> = Vec::new();
        let mut all_cats: Vec<Uuid> = Vec::new(); // duplicate avoidance
        let mut folders: Vec<FolderRequest> = Vec::new();
        let mut folders_lib: Vec<FolderRequest> = Vec::new();
        let mut items: Vec<ItemRequest> = Vec::new();
        let mut items_lib: Vec<ItemRequest> = Vec::new();

        while let Some(&info) = self.fetch_folder_queue.front() {
            if item_count + folder_count >= LEGACY_MAX_BATCH_SIZE {
                break;
            }
            if info.is_category {
                let cat_id = info.id;
                if cat_id == NULL_ID {
                    // DEV-17797 Lost and found
                    folders.push(FolderRequest {
                        folder_id: NULL_ID,
                        id_as_string: true,
                        owner_id: agent,
                        sort_order,
                        fetch_folders: false,
                    });
                    folder_count += 1;
                } else if let Some(cat) = cx.model.category(&cat_id) {
                    if cat.version == VERSION_UNKNOWN && !all_cats.contains(&cat_id) {
                        let f = FolderRequest {
                            folder_id: cat_id,
                            id_as_string: false,
                            owner_id: cat.owner_id,
                            sort_order,
                            fetch_folders: true,
                        };
                        // <FS:Beq> the library owner for OpenSim
                        if cx.model.library_owner_id() == cat.owner_id {
                            folders_lib.push(f);
                        } else {
                            folders.push(f);
                        }
                        folder_count += 1;
                    }
                    #[cfg(feature = "opensim")]
                    let descend = cat.version != VERSION_UNKNOWN || !cx.env.in_second_life;
                    #[cfg(not(feature = "opensim"))]
                    let descend = cat.version != VERSION_UNKNOWN;
                    if descend && info.fetch_type >= FetchType::ContentRecursive {
                        // May already have this folder, but append child folders
                        for c in cx.model.child_categories(&cat_id).unwrap_or_default() {
                            self.fetch_folder_queue
                                .push_back(entry(c, info.fetch_type, true));
                        }
                    }
                }
                if info.fetch_type >= FetchType::ContentRecursive {
                    recursive_cats.push(cat_id);
                }
                all_cats.push(cat_id);
            }
            self.fetch_folder_queue.pop_front();
        }

        while let Some(&info) = self.fetch_item_queue.front() {
            if item_count + folder_count >= LEGACY_MAX_BATCH_SIZE {
                break;
            }
            if let Some(item) = cx.model.item(&info.id) {
                let r = ItemRequest {
                    item_id: info.id,
                    owner_id: item.owner_id,
                };
                if item.owner_id == agent {
                    items.push(r);
                } else {
                    items_lib.push(r);
                }
                item_count += 1;
            }
            self.fetch_item_queue.pop_front();
        }

        if item_count + folder_count > 0 {
            if folder_count > 0 {
                if !folders.is_empty() && !cx.env.cap_fid2.is_empty() {
                    let body = FolderBody {
                        folders: Some(folders),
                    };
                    self.legacy_folder_post(
                        cx.env.cap_fid2.clone(),
                        body,
                        recursive_cats.clone(),
                        "Inventory Folder",
                    );
                }
                if !folders_lib.is_empty() && !cx.env.cap_fld2.is_empty() {
                    let body = FolderBody {
                        folders: Some(folders_lib),
                    };
                    self.legacy_folder_post(
                        cx.env.cap_fld2.clone(),
                        body,
                        recursive_cats.clone(),
                        "Library Folder",
                    );
                }
            }
            if item_count > 0 {
                if !items.is_empty() && !cx.env.cap_fi2.is_empty() {
                    let body = ItemBody {
                        agent_id: None,
                        items,
                    };
                    self.legacy_item_post(cx.env.cap_fi2.clone(), body.to_xml(), "Inventory Item");
                }
                if !items_lib.is_empty() && !cx.env.cap_fl2.is_empty() {
                    let body = ItemBody {
                        agent_id: None,
                        items: items_lib,
                    };
                    self.legacy_item_post(cx.env.cap_fl2.clone(), body.to_xml(), "Library Item");
                }
            }
            // read only by the OPENSIM UDP loop
            self.actions.push(Action::FetchTimerReset);
        } else if self.is_bulk_fetch_processing_complete() {
            self.set_all_folders_fetched();
        }
    }

    fn legacy_folder_post(
        &mut self,
        url: String,
        body: FolderBody,
        recursive: Vec<Uuid>,
        label: &'static str,
    ) {
        let handler = self.next_handler;
        self.next_handler += 1;
        // BGFolderHttpHandler's constructor
        self.incr_fetch_count(1);
        let xml = body.to_xml();
        self.legacy
            .insert(handler, LegacyHandler { body, recursive });
        self.actions.push(Action::LegacyPost {
            handler,
            folder: true,
            url,
            body: xml,
            label,
        });
    }

    fn legacy_item_post(&mut self, url: String, body: Vec<u8>, label: &'static str) {
        let handler = self.next_handler;
        self.next_handler += 1;
        // BGItemHttpHandler's constructor
        self.incr_fetch_count(1);
        self.legacy_item_handlers.insert(handler);
        self.actions.push(Action::LegacyPost {
            handler,
            folder: false,
            url,
            body,
            label,
        });
    }

    /// `BGFolderHttpHandler::getIsRecursive()`: the reply's applier asks it
    /// for every category of a folder whose parent the model has; a
    /// recursive one goes to [`add_request_at_back`] (recursive), any other
    /// one is applied (`updateCategory` when incomplete).
    ///
    /// [`add_request_at_back`]: BackgroundFetch::add_request_at_back
    pub fn legacy_is_recursive(&self, handler: u64, cat_id: &Uuid) -> bool {
        self.legacy
            .get(&handler)
            .is_some_and(|h| h.recursive.contains(cat_id))
    }

    /// The end of `processData()`, after the applier wrote the folders,
    /// categories and items.
    pub fn legacy_data_done(&mut self, cx: &mut Ctx<'_>) {
        if self.is_bulk_fetch_processing_complete() {
            self.set_all_folders_fetched();
        }
        if !cx.env.in_second_life {
            self.actions.push(Action::NotifyObservers);
        }
    }

    /// `processFailure(HttpStatus)`: a 403 splits the request in two when it
    /// can; anything else only checks for completion.
    pub fn legacy_http_failure(&mut self, cx: &mut Ctx<'_>, handler: u64, status: i32) {
        if status == 403 {
            // Too large, split into two if possible
            if cx.env.disconnected || cx.env.exiting {
                return;
            }
            if cx.env.cap_fid2.is_empty() {
                self.warn(Warning::NoAis2Cap, NULL_ID);
                return;
            }
            let Some(h) = self.legacy.get(&handler).cloned() else {
                return;
            };
            if h.body.len() > 1 {
                // Can split, assume that this isn't the library
                for (body, rec) in super::legacy::split_403(&h.body, &h.recursive) {
                    self.legacy_folder_post(cx.env.cap_fid2.clone(), body, rec, "Inventory Folder");
                }
                return;
            }
            // Can't split
            self.actions.push(Action::Notify {
                name: "InventoryLimitReachedAIS",
            });
        }
        if self.is_bulk_fetch_processing_complete() {
            self.set_all_folders_fetched();
        }
        if !cx.env.in_second_life {
            self.actions.push(Action::NotifyObservers);
        }
    }

    /// `processFailure(const char* reason)`: a missing body, malformed LLSD,
    /// a non-map or a 200-with-error. Every folder of the request is queued
    /// again at the front, without limit (survey risk "the legacy unbounded
    /// re-queue").
    pub fn legacy_reply_failure(&mut self, cx: &mut Ctx<'_>, handler: u64) {
        if let Some(h) = self.legacy.get(&handler).cloned() {
            for f in h.body.folders.iter().flatten() {
                let recursive = h.recursive.contains(&f.folder_id);
                self.add_request_at_front(f.folder_id, recursive, true);
            }
        }
        if !cx.env.in_second_life {
            self.actions.push(Action::NotifyObservers);
        }
    }

    /// A legacy handler was destroyed (after its `onCompleted`).
    pub fn legacy_handler_released(&mut self, handler: u64) {
        if self.legacy.remove(&handler).is_some() || self.legacy_item_handlers.remove(&handler) {
            self.incr_fetch_count(-1);
        }
    }
}
