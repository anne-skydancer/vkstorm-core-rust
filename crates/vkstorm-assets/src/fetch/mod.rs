//! `LLTextureFetchWorker`'s state machine and `LLTextureFetch`'s HTTP
//! resource accounting, as pure logic driven by the host's threads.
//!
//! The viewer keeps its worker objects (`LLWorkerClass` lifecycle, the
//! `LLImageFormatted` and `LLImageRaw` containers, the HTTP handler, the
//! cache and decode handles) and its threads (the fetch thread, the cache
//! and decode pools). Each C++ worker owns one [`Worker`]; its `doWork` is
//! [`Worker::do_work`], which makes every decision the C++ made (which cache
//! read to issue, which byte range to ask for and at what discard level,
//! when to decode, write back, retry, wait or give up) and asks the host to
//! carry them out through the [`Host`] trait. Completions come back through
//! the event methods (`cache_read_done`, `http_completed`, `decode_done`,
//! `cache_write_done`), as the C++ callbacks set the worker's flags.
//!
//! The fetcher-level HTTP semaphore, the resource waiters and the request
//! planning of `createRequest` live in [`Fetcher`] and [`plan`].

pub mod budget;
pub mod retry;

use std::time::{Duration, Instant};

use crate::{uuid_to_string, Uuid};
use budget::{FIRST_PACKET_SIZE, MAX_DISCARD_LEVEL};
#[cfg(feature = "opensim")]
use budget::MAX_IMG_PACKET_SIZE;
use retry::AdaptiveRetryPolicy;

/// `LLTextureFetchWorker::e_state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(i32)]
pub enum State {
    Invalid = 0,
    Init,
    LoadFromTextureCache,
    CachePost,
    LoadFromNetwork,
    LoadFromSimulator,
    WaitHttpResource,
    WaitHttpResource2,
    SendHttpReq,
    WaitHttpReq,
    DecodeImage,
    DecodeImageUpdate,
    WriteToCache,
    WaitOnWrite,
    Done,
}

impl State {
    pub const NAMES: [&'static str; 15] = [
        "INVALID",
        "INIT",
        "LOAD_FROM_TEXTURE_CACHE",
        "CACHE_POST",
        "LOAD_FROM_NETWORK",
        "LOAD_FROM_SIMULATOR",
        "WAIT_HTTP_RESOURCE",
        "WAIT_HTTP_RESOURCE2",
        "SEND_HTTP_REQ",
        "WAIT_HTTP_REQ",
        "DECODE_IMAGE",
        "DECODE_IMAGE_UPDATE",
        "WRITE_TO_CACHE",
        "WAIT_ON_WRITE",
        "DONE",
    ];

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    pub fn from_i32(v: i32) -> State {
        match v {
            1 => State::Init,
            2 => State::LoadFromTextureCache,
            3 => State::CachePost,
            4 => State::LoadFromNetwork,
            5 => State::LoadFromSimulator,
            6 => State::WaitHttpResource,
            7 => State::WaitHttpResource2,
            8 => State::SendHttpReq,
            9 => State::WaitHttpReq,
            10 => State::DecodeImage,
            11 => State::DecodeImageUpdate,
            12 => State::WriteToCache,
            13 => State::WaitOnWrite,
            14 => State::Done,
            _ => State::Invalid,
        }
    }

    /// `getStateString`.
    pub fn string_for(state: i32) -> String {
        if !(0..=15).contains(&state) {
            return state.to_string();
        }
        // The C++ indexed one past the table for state 15; map it to DONE's
        // neighbour defensively (the value never occurs).
        Self::NAMES[(state as usize).min(14)].to_string()
    }
}

/// The states whose time is recorded for the metrics tester.
const LOGGED_STATES: [State; 6] = [
    State::LoadFromTextureCache,
    State::LoadFromNetwork,
    State::LoadFromSimulator,
    State::WaitHttpReq,
    State::DecodeImageUpdate,
    State::WaitOnWrite,
];

/// `FTType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum FetchType {
    Unknown = -1,
    Default = 0,
    ServerBake = 1,
    HostBake = 2,
    MapTile = 3,
    LocalFile = 4,
}

impl FetchType {
    pub fn from_i32(v: i32) -> FetchType {
        match v {
            0 => FetchType::Default,
            1 => FetchType::ServerBake,
            2 => FetchType::HostBake,
            3 => FetchType::MapTile,
            4 => FetchType::LocalFile,
            _ => FetchType::Unknown,
        }
    }
}

/// `e_write_to_cache_state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum WriteToCacheState {
    NotWrite = 0,
    CanWrite = 1,
    ShouldWrite = 2,
}

/// `e_request_state` (`mSentRequest`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum RequestState {
    Unsent = 0,
    Queued = 1,
    SentSim = 2,
}

/// `HTTP_PIPE_REQUESTS_*` and `HTTP_NONPIPE_REQUESTS_*`.
pub const HTTP_PIPE_REQUESTS_HIGH_WATER: i32 = 100;
pub const HTTP_PIPE_REQUESTS_LOW_WATER: i32 = 50;
pub const HTTP_NONPIPE_REQUESTS_HIGH_WATER: i32 = 40;
pub const HTTP_NONPIPE_REQUESTS_LOW_WATER: i32 = 20;
/// `HTTP_REQUESTS_RANGE_END_MAX`: ranges ending past this ask for the rest.
pub const HTTP_REQUESTS_RANGE_END_MAX: i32 = 20_000_000;
/// `MAX_CAP_MISSING_RETRIES` and `CAP_MISSING_EXPIRATION_DELAY`.
pub const MAX_CAP_MISSING_RETRIES: i32 = 720;
pub const CAP_MISSING_EXPIRATION_DELAY: f32 = 1.0;
/// `F_ALMOST_ZERO`.
pub const F_ALMOST_ZERO: f32 = 0.0001;
/// `HttpStatus` codes the state machine switches on.
pub const HTTP_NOT_FOUND: i32 = 404;
pub const HTTP_SERVICE_UNAVAILABLE: i32 = 503;
pub const HTTP_REQUESTED_RANGE_NOT_SATISFIABLE: i32 = 416;
pub const HTTP_PARTIAL_CONTENT: i32 = 206;

/// The region the worker fetches from (`getRegion()`), as the host sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Region {
    pub id: Uuid,
    pub viewer_asset_url: String,
    pub http_url: String,
}

/// The environment of one `doWork` call: what the C++ read from globals,
/// settings and its own objects.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// `mFetcher->isQuitting()`.
    pub quitting: bool,
    /// `getFlags(WCF_DELETE_REQUESTED)`.
    pub delete_requested: bool,
    /// `mFetcher->mDebugPause`.
    pub debug_pause: bool,
    /// `gNonInteractive`.
    pub non_interactive: bool,
    /// `FSAssetBlacklist::isBlacklisted(mID, AT_TEXTURE)`.
    pub blacklisted: bool,
    /// `mFormattedImage.notNull()`.
    pub formatted_present: bool,
    /// `mFormattedImage->getDataSize()` (0 when absent).
    pub formatted_size: i32,
    /// `mFormattedImage->getDiscardLevel()`.
    pub formatted_discard: i32,
    /// `mFormattedImage->getCodec() == IMG_CODEC_J2C`.
    pub formatted_is_j2c: bool,
    /// `mFetcher->canLoadFromCache()`.
    pub can_load_from_cache: bool,
    /// `use_http_textures()`.
    pub use_http_textures: bool,
    /// `gDisconnected`.
    pub disconnected: bool,
    /// `HttpRangeRequestsDisable`.
    pub range_requests_disabled: bool,
    /// `TextureDecodeDisabled`.
    pub decode_disabled: bool,
    /// `mFetcher->getTextureBandwidth()` and `mMaxBandwidth`, for the log.
    pub texture_bandwidth: f32,
    pub max_bandwidth: f32,
    /// `mRawImage->getWidth()/getHeight()`, for the log after a decode.
    pub raw_width: i32,
    pub raw_height: i32,
    /// `mFormattedImage->getWidth()/getHeight()` for the CACHE_POST log.
    pub formatted_width: i32,
    pub formatted_height: i32,
}

/// The HTTP request the state machine asks the host to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpGet {
    pub url: String,
    /// The range (`offset`, `size`); `size` 0 asks for the rest of the file.
    pub offset: i32,
    pub size: i32,
    /// Whole-resource GET (`HttpRangeRequestsDisable`).
    pub no_range: bool,
    /// `mHttpOptionsWithHeaders` (server bakes) rather than `mHttpOptions`.
    pub with_headers: bool,
}

/// What the state machine asks of its host. Every method corresponds to a
/// call the C++ worker made on the fetcher, the cache, the decode thread or
/// its own containers; none may throw across the boundary.
pub trait Host {
    /// The environment snapshot for this call.
    fn env(&mut self) -> Env;
    /// `getRegion()`.
    fn region(&mut self) -> Option<Region>;
    /// `readFromCache`: by local file name (the `file://` case) or by id.
    fn read_cache(&mut self, local_file: Option<&str>, offset: i32, size: i32);
    /// `readComplete(handle, false)`; clears the handle when it returns
    /// true.
    fn cache_read_complete(&mut self) -> bool;
    /// `requestGet`/`requestGetByteRange`; false when no handle came back
    /// (the host logs the status).
    fn send_http(&mut self, get: &HttpGet) -> bool;
    /// `mRawImage = NULL; mAuxImage = NULL`.
    fn drop_raw_images(&mut self);
    /// `LLImageDecodeThread::decodeImage` with a new responder; returns the
    /// decode handle (0 when it did not post).
    fn decode(&mut self, discard: i32, needs_aux: bool) -> u32;
    /// `writeToCache` with the formatted image's bytes and the raw image.
    fn write_cache(&mut self, file_size: i32, decoded_discard: i32);
    /// `writeComplete(handle)`; clears the handle when it returns true.
    fn cache_write_complete(&mut self) -> bool;
    /// `prioritizeWrite(handle)`.
    fn prioritize_write(&mut self);
    /// `removeFromCache` unless the texture is from a local file.
    fn remove_from_cache(&mut self);
    /// `resetFormattedData`'s container side: release the HTTP buffer and
    /// delete the formatted image's data.
    fn reset_formatted_data(&mut self);
    /// `mFormattedImage = NULL`.
    fn drop_formatted_image(&mut self);
    /// Release the HTTP reply buffer (`mHttpBufferArray->release()`).
    fn release_http_buffer(&mut self);
    /// Builds the formatted image from the bytes held plus the HTTP reply
    /// (`WAIT_HTTP_REQ`): creates the image from the URL's extension when
    /// absent (JPEG 2000 by default), copies `cur_size` existing bytes and
    /// `append_size` reply bytes from `src_offset`, sets `total_size`.
    /// False when the allocation failed. `url` is the request URL, whose
    /// extension picks the codec when no image exists yet.
    fn assemble_http_reply(&mut self, url: &str, src_offset: i32, cur_size: i32, append_size: i32, total_size: i32) -> bool;
    /// `mFetcher->addToHTTPQueue(mID)`.
    fn add_to_http_queue(&mut self);
    /// `recordTextureStart`/`recordTextureDone` (asset statistics).
    fn record_texture_start(&mut self, is_http: bool);
    fn record_texture_done(&mut self, is_http: bool, byte_count: f64);
    /// `add(sCacheAttempt, 1)`, `add(sCacheHit, 1)`, `record(sCacheHitRate, hit)`.
    fn stat_cache_attempt(&mut self);
    fn stat_cache_hit(&mut self);
    fn stat_cache_hit_rate(&mut self, hit: bool);
    /// A log line at the viewer's level (0 debug, 1 info, 2 warn); `once`
    /// marks `LL_INFOS_ONCE`.
    fn log(&mut self, level: i32, once: bool, msg: &str);
    /// OpenSim: `mFetcher->addToNetworkQueue(this)` and
    /// `removeFromNetworkQueue(this, cancel)`.
    #[cfg(feature = "opensim")]
    fn add_to_network_queue(&mut self);
    #[cfg(feature = "opensim")]
    fn remove_from_network_queue(&mut self, cancel: bool);
    /// OpenSim: `processSimulatorPackets`' container side: assemble
    /// `mFormattedImage` from the packets (the host owns the packet
    /// buffers). Returns the data size assembled.
    #[cfg(feature = "opensim")]
    fn assemble_packets(&mut self, first_packet: i32, last_packet: i32) -> i32;
}

/// The fetcher-level state the workers share: the HTTP resource semaphore,
/// its water marks, the resource waiters and the cumulative statistics.
#[derive(Debug)]
pub struct Fetcher {
    /// `mHttpSemaphore`: outstanding requests charged against the limit.
    pub http_semaphore: i32,
    pub http_high_water: i32,
    pub http_low_water: i32,
    /// `mHttpWaitResource`.
    pub http_wait_resource: std::collections::BTreeSet<Uuid>,
    /// `mTotalCacheReadCount`, `mTotalCacheWriteCount`, `mTotalResourceWaitCount`.
    pub total_cache_read_count: u32,
    pub total_cache_write_count: u32,
    pub total_resource_wait_count: u32,
    /// `mTotalHTTPRequests`.
    pub total_http_requests: u32,
    /// `mDebugPause`, `mDebugID`, `mDebugCount`.
    pub debug_pause: bool,
    pub debug_id: Uuid,
    pub debug_count: i32,
}

impl Default for Fetcher {
    fn default() -> Self {
        Fetcher::new()
    }
}

impl Fetcher {
    pub fn new() -> Fetcher {
        Fetcher {
            http_semaphore: 0,
            http_high_water: HTTP_NONPIPE_REQUESTS_HIGH_WATER,
            http_low_water: HTTP_NONPIPE_REQUESTS_LOW_WATER,
            http_wait_resource: Default::default(),
            total_cache_read_count: 0,
            total_cache_write_count: 0,
            total_resource_wait_count: 0,
            total_http_requests: 0,
            debug_pause: false,
            debug_id: [0; 16],
            debug_count: 0,
        }
    }

    /// `commonUpdate`'s water-mark selection.
    pub fn set_pipelined(&mut self, pipelined: bool) {
        if pipelined {
            self.http_high_water = HTTP_PIPE_REQUESTS_HIGH_WATER;
            self.http_low_water = HTTP_PIPE_REQUESTS_LOW_WATER;
        } else {
            self.http_high_water = HTTP_NONPIPE_REQUESTS_HIGH_WATER;
            self.http_low_water = HTTP_NONPIPE_REQUESTS_LOW_WATER;
        }
    }

    pub fn add_http_waiter(&mut self, id: &Uuid) {
        self.http_wait_resource.insert(*id);
    }

    pub fn remove_http_waiter(&mut self, id: &Uuid) {
        self.http_wait_resource.remove(id);
    }

    pub fn is_http_waiter(&self, id: &Uuid) -> bool {
        self.http_wait_resource.contains(id)
    }

    pub fn http_waiters_count(&self) -> i32 {
        self.http_wait_resource.len() as i32
    }

    pub fn cancel_http_waiters(&mut self) {
        self.http_wait_resource.clear();
    }

    /// `updateStateStats`.
    pub fn update_state_stats(&mut self, cache_read: u32, cache_write: u32, res_wait: u32) {
        self.total_cache_read_count += cache_read;
        self.total_cache_write_count += cache_write;
        self.total_resource_wait_count += res_wait;
    }

    /// `releaseHttpWaiters`, first half: the number of waiters to release
    /// now (0 when the semaphore is above the low water or nothing waits)
    /// and the waiter ids, for the host to look up.
    pub fn waiters_to_release(&self) -> (i32, Vec<Uuid>) {
        if self.http_semaphore >= self.http_low_water {
            return (0, Vec::new());
        }
        let needed = self.http_high_water - self.http_semaphore;
        if needed <= 0 || self.http_wait_resource.is_empty() {
            return (0, Vec::new());
        }
        (needed, self.http_wait_resource.iter().copied().collect())
    }

    /// `releaseHttpWaiters`, second half: orders the found waiters by
    /// priority (highest first) so that the first `needed` are the ones to
    /// release (`std::partial_sort` with `Compare`), returning the indices
    /// into `priorities` in release order.
    pub fn release_order(priorities: &[f32], needed: i32) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..priorities.len()).collect();
        if priorities.len() > needed.max(0) as usize {
            // partial_sort of the first `needed` by descending priority;
            // the remainder's order is unspecified in the C++ and they are
            // visited anyway, so a full stable sort gives the same release
            // set and the same order among the released.
            idx.sort_by(|&a, &b| priorities[b].partial_cmp(&priorities[a]).unwrap_or(std::cmp::Ordering::Equal));
        }
        idx
    }
}

/// The outcome of `Worker::setup_packet_data` (OpenSim).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupPackets {
    Ok,
    /// "Bad CACHED TEXTURE size": the host removes the texture from the
    /// cache and resets the formatted data.
    BadSize,
    /// Cached through HTTP: the host resets the formatted data so the
    /// first packet is fetched again.
    Refetch,
}

/// The outcome of `Worker::try_release` (one waiter in `releaseHttpWaiters`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Release {
    /// Moved to `SEND_HTTP_REQ`; remove it from the waiters.
    Released,
    /// Not in `WAIT_HTTP_RESOURCE2`; remove it from the waiters and go on.
    NotWaiting,
    /// No semaphore slot: stop releasing.
    NoSlot,
}

/// OpenSim: one received UDP packet of a texture.
#[cfg(feature = "opensim")]
#[derive(Clone, Debug)]
pub struct PacketInfo {
    pub size: u32,
}

/// `LLTextureFetchWorker`'s state.
#[derive(Debug)]
pub struct Worker {
    pub id: Uuid,
    pub ftype: FetchType,
    pub url: String,
    /// `mHost.isOk()`: a simulator host was given (avatar bakes).
    pub host_is_ok: bool,
    pub state: State,
    pub write_to_cache_state: WriteToCacheState,
    pub image_priority: f32,
    pub requested_priority: f32,
    pub desired_discard: i32,
    pub sim_requested_discard: i32,
    pub requested_discard: i32,
    pub loaded_discard: i32,
    pub decoded_discard: i32,
    pub requested_size: i32,
    pub requested_offset: i32,
    pub desired_size: i32,
    pub file_size: i32,
    pub cached_size: i32,
    pub sent_request: RequestState,
    pub loaded: bool,
    pub decoded: bool,
    pub written: bool,
    pub needs_aux: bool,
    pub have_all_data: bool,
    pub in_local_cache: bool,
    pub in_cache: bool,
    pub can_use_http: bool,
    pub can_use_net: bool,
    pub retry_attempt: i32,
    pub active_count: i32,
    /// `mGetStatus` as (ok, type/code) and `mGetReason`.
    pub get_status_ok: bool,
    pub get_status_type: i32,
    pub get_reason: String,
    pub fetch_retry_policy: AdaptiveRetryPolicy,
    pub can_use_capability: bool,
    pub region_retry_attempt: i32,
    region_retry_deadline: Option<Instant>,
    pub last_region_id: Uuid,
    /// `mHttpActive`, `mHttpHasResource`, `mHttpReplySize`, `mHttpReplyOffset`.
    pub http_active: bool,
    pub http_has_resource: bool,
    pub http_reply_size: u32,
    pub http_reply_offset: u32,
    /// Whether the host holds an HTTP reply buffer and its size
    /// (`mHttpBufferArray`).
    pub http_buffer_present: bool,
    pub http_buffer_size: i32,
    /// `mCacheReadCount`, `mCacheWriteCount`, `mResourceWaitCount`.
    pub cache_read_count: u32,
    pub cache_write_count: u32,
    pub resource_wait_count: u32,
    /// Handles the host holds (`mCacheReadHandle`, `mCacheWriteHandle`,
    /// `mDecodeHandle`): tracked here as flags so the machine knows.
    pub cache_read_pending: bool,
    pub cache_write_pending: bool,
    pub decode_handle: u32,
    /// `mImageCodec` from the cache read.
    pub image_codec: i32,
    /// Timers.
    requested_delta: Instant,
    fetch_delta: Instant,
    cache_read_timer: Instant,
    decode_timer: Instant,
    cache_write_timer: Instant,
    fetch_timer: Instant,
    state_timer: Instant,
    pub cache_read_time: f32,
    pub decode_time: f32,
    pub cache_write_time: f32,
    pub fetch_time: f32,
    pub state_timers: [f32; 15],
    pub skipped_states_time: f32,
    /// `mMetricsStartTime` is kept by the host (it is an asset-stats type).
    #[cfg(feature = "opensim")]
    pub packets: Vec<Option<PacketInfo>>,
    #[cfg(feature = "opensim")]
    pub first_packet: i32,
    #[cfg(feature = "opensim")]
    pub last_packet: i32,
    #[cfg(feature = "opensim")]
    pub total_packets: u16,
}

impl Worker {
    /// The constructor: `in_second_life` is `LLGridManager::isInSecondLife()`
    /// (`mCanUseNET = !in_second_life && url.empty()`).
    pub fn new(ftype: FetchType, url: &str, id: Uuid, host_is_ok: bool, priority: f32, in_second_life: bool) -> Worker {
        let now = Instant::now();
        Worker {
            id,
            ftype,
            url: url.to_string(),
            host_is_ok,
            state: State::Init,
            write_to_cache_state: WriteToCacheState::NotWrite,
            image_priority: priority,
            requested_priority: 0.0,
            desired_discard: -1,
            sim_requested_discard: -1,
            requested_discard: -1,
            loaded_discard: -1,
            decoded_discard: -1,
            requested_size: 0,
            requested_offset: 0,
            desired_size: FIRST_PACKET_SIZE,
            file_size: 0,
            cached_size: 0,
            sent_request: RequestState::Unsent,
            loaded: false,
            decoded: false,
            written: false,
            needs_aux: false,
            have_all_data: false,
            in_local_cache: false,
            in_cache: false,
            can_use_http: true,
            can_use_net: !in_second_life && url.is_empty(),
            retry_attempt: 0,
            active_count: 0,
            get_status_ok: true,
            get_status_type: 0,
            get_reason: String::new(),
            fetch_retry_policy: AdaptiveRetryPolicy::new(10.0, 3600.0, 2.0, 10, false),
            can_use_capability: true,
            region_retry_attempt: 0,
            region_retry_deadline: None,
            last_region_id: [0; 16],
            http_active: false,
            http_has_resource: false,
            http_reply_size: 0,
            http_reply_offset: 0,
            http_buffer_present: false,
            http_buffer_size: 0,
            cache_read_count: 0,
            cache_write_count: 0,
            resource_wait_count: 0,
            cache_read_pending: false,
            cache_write_pending: false,
            decode_handle: 0,
            image_codec: -1,
            requested_delta: now,
            fetch_delta: now,
            cache_read_timer: now,
            decode_timer: now,
            cache_write_timer: now,
            fetch_timer: now,
            state_timer: now,
            cache_read_time: 0.0,
            decode_time: 0.0,
            cache_write_time: 0.0,
            fetch_time: 0.0,
            state_timers: [0.0; 15],
            skipped_states_time: 0.0,
            #[cfg(feature = "opensim")]
            packets: Vec::new(),
            #[cfg(feature = "opensim")]
            first_packet: 0,
            #[cfg(feature = "opensim")]
            last_packet: -1,
            #[cfg(feature = "opensim")]
            total_packets: 0,
        }
    }

    /// `setState`.
    pub fn set_state(&mut self, new_state: State) {
        let d_time = self.state_timer.elapsed().as_secs_f32();
        if d_time >= 0.0001 {
            if LOGGED_STATES.contains(&self.state) {
                self.state_timers[self.state as usize] = d_time;
            } else {
                self.skipped_states_time += d_time;
            }
        }
        self.state_timer = Instant::now();
        self.state = new_state;
    }

    /// `setDesiredDiscard(discard, size)`. Returns whether work must be
    /// added (`addWork(0)`): the host does so unless paused.
    pub fn set_desired_discard(&mut self, discard: i32, size: i32, have_work: bool) -> bool {
        let mut prioritize = false;
        let mut add_work = false;
        if self.desired_discard != discard {
            if !have_work {
                add_work = true;
            } else if self.desired_discard < discard {
                prioritize = true;
            }
            self.desired_discard = discard;
            self.desired_size = size;
        } else if size > self.desired_size {
            self.desired_size = size;
            prioritize = true;
        }
        self.desired_size = self.desired_size.max(FIRST_PACKET_SIZE);
        if (prioritize && self.state == State::Init) || self.state == State::Done {
            self.set_state(State::Init);
        }
        add_work
    }

    pub fn set_image_priority(&mut self, priority: f32) {
        self.image_priority = priority;
    }

    /// `mRequestedDeltaTimer.getElapsedTimeF32()` and `mFetchDeltaTimer`.
    pub fn requested_delta_secs(&self) -> f32 {
        self.requested_delta.elapsed().as_secs_f32()
    }

    pub fn fetch_delta_secs(&self) -> f32 {
        self.fetch_delta.elapsed().as_secs_f32()
    }

    /// `acquireHttpSemaphore`.
    fn acquire_http_semaphore(&mut self, fetcher: &mut Fetcher) -> bool {
        debug_assert!(!self.http_has_resource);
        if fetcher.http_semaphore >= fetcher.http_high_water {
            return false;
        }
        self.http_has_resource = true;
        fetcher.http_semaphore += 1;
        true
    }

    /// `releaseHttpSemaphore`.
    pub fn release_http_semaphore(&mut self, fetcher: &mut Fetcher) {
        debug_assert!(self.http_has_resource);
        self.http_has_resource = false;
        fetcher.http_semaphore -= 1;
        assert!(fetcher.http_semaphore >= 0);
    }

    /// One waiter's turn in `releaseHttpWaiters`.
    pub fn try_release(&mut self, fetcher: &mut Fetcher) -> Release {
        if self.state != State::WaitHttpResource2 {
            return Release::NotWaiting;
        }
        if !self.acquire_http_semaphore(fetcher) {
            return Release::NoSlot;
        }
        self.set_state(State::SendHttpReq);
        Release::Released
    }

    /// `resetFormattedData`'s state side (the host does the containers).
    fn reset_formatted_data(&mut self, host: &mut dyn Host) {
        host.reset_formatted_data();
        self.http_buffer_present = false;
        self.http_buffer_size = 0;
        self.http_reply_size = 0;
        self.http_reply_offset = 0;
        self.have_all_data = false;
    }

    /// The destructor's accounting: releases the semaphore if held and
    /// returns whether an HTTP request is active (the host cancels it).
    pub fn on_destroy(&mut self, fetcher: &mut Fetcher) -> bool {
        if self.http_has_resource {
            self.release_http_semaphore(fetcher);
        }
        fetcher.remove_http_waiter(&self.id);
        fetcher.update_state_stats(self.cache_read_count, self.cache_write_count, self.resource_wait_count);
        self.http_active
    }

    /// `doWork`: returns true when the request is finished (or aborted),
    /// false when it waits for an event.
    pub fn do_work(&mut self, fetcher: &mut Fetcher, host: &mut dyn Host) -> bool {
        let mut env = host.env();
        if env.non_interactive {
            return true;
        }
        if env.quitting || env.delete_requested {
            if self.state < State::DecodeImage {
                return true; // abort
            }
        }
        if self.image_priority < F_ALMOST_ZERO
            && (self.state == State::Init || self.state == State::LoadFromNetwork || self.state == State::LoadFromSimulator)
        {
            host.log(0, false, &format!("{} abort: mImagePriority < F_ALMOST_ZERO", uuid_to_string(&self.id)));
            return true; // abort
        }
        if self.state > State::CachePost && !self.can_use_capability && self.can_use_http {
            if self.region_retry_attempt > MAX_CAP_MISSING_RETRIES {
                self.can_use_http = false;
            } else if !self.region_retry_expired() {
                return false;
            }
        }
        if self.state > State::CachePost && !self.can_use_net && !self.can_use_http {
            host.log(2, false, &format!("{} abort, nowhere to get data", uuid_to_string(&self.id)));
            return true;
        }
        if env.debug_pause {
            return false; // debug: don't do any work
        }
        if self.id == fetcher.debug_id {
            fetcher.debug_count += 1; // for setting breakpoints
        }
        if self.state != State::Done {
            self.fetch_delta = Instant::now();
        }

        // The C++ `return doWork(param)` recursions are iterations here;
        // each iteration re-reads the environment as a fresh call did.
        let mut first = true;
        loop {
            if !first {
                env = host.env();
                if env.non_interactive {
                    return true;
                }
                if (env.quitting || env.delete_requested) && self.state < State::DecodeImage {
                    return true;
                }
                if self.image_priority < F_ALMOST_ZERO
                    && (self.state == State::Init || self.state == State::LoadFromNetwork || self.state == State::LoadFromSimulator)
                {
                    host.log(0, false, &format!("{} abort: mImagePriority < F_ALMOST_ZERO", uuid_to_string(&self.id)));
                    return true;
                }
                if self.state > State::CachePost && !self.can_use_capability && self.can_use_http {
                    if self.region_retry_attempt > MAX_CAP_MISSING_RETRIES {
                        self.can_use_http = false;
                    } else if !self.region_retry_expired() {
                        return false;
                    }
                }
                if self.state > State::CachePost && !self.can_use_net && !self.can_use_http {
                    host.log(2, false, &format!("{} abort, nowhere to get data", uuid_to_string(&self.id)));
                    return true;
                }
                if env.debug_pause {
                    return false;
                }
                if self.id == fetcher.debug_id {
                    fetcher.debug_count += 1;
                }
                if self.state != State::Done {
                    self.fetch_delta = Instant::now();
                }
            }
            first = false;

            match self.do_work_pass(fetcher, host, &env) {
                Pass::Return(r) => return r,
                Pass::Again => continue,
            }
        }
    }

    fn region_retry_expired(&self) -> bool {
        match self.region_retry_deadline {
            Some(d) => Instant::now() >= d,
            None => true,
        }
    }

    fn set_region_retry(&mut self) {
        self.region_retry_deadline = Some(Instant::now() + Duration::from_secs_f32(CAP_MISSING_EXPIRATION_DELAY));
    }

    /// One pass through the state chain of `doWork` (from the current
    /// state to the first return).
    fn do_work_pass(&mut self, fetcher: &mut Fetcher, host: &mut dyn Host, env: &Env) -> Pass {
        let id = uuid_to_string(&self.id);

        if self.state == State::Init {
            if env.blacklisted {
                host.log(1, false, "Blacklisted texture asset blocked.");
                self.state = State::Done;
                return Pass::Return(true);
            }
            self.state_timer = Instant::now();
            self.fetch_timer = Instant::now();
            for s in LOGGED_STATES.iter() {
                self.state_timers[*s as usize] = 0.0;
            }
            self.skipped_states_time = 0.0;
            host.drop_raw_images();
            self.requested_discard = -1;
            self.loaded_discard = -1;
            self.decoded_discard = -1;
            self.requested_size = 0;
            self.requested_offset = 0;
            self.file_size = 0;
            self.cached_size = 0;
            self.loaded = false;
            self.sent_request = RequestState::Unsent;
            self.decoded = false;
            self.written = false;
            if self.http_buffer_present {
                host.release_http_buffer();
                self.http_buffer_present = false;
                self.http_buffer_size = 0;
            }
            self.http_reply_size = 0;
            self.http_reply_offset = 0;
            self.have_all_data = false;
            #[cfg(feature = "opensim")]
            self.clear_packets();
            self.cache_read_pending = false;
            self.cache_write_pending = false;
            self.set_state(State::LoadFromTextureCache);
            self.in_cache = false;
            self.desired_size = self.desired_size.max(FIRST_PACKET_SIZE);
            host.log(
                0,
                false,
                &format!(
                    "{}: Priority: {:8.0} Desired Discard: {} Desired Size: {}",
                    id, self.image_priority, self.desired_discard, self.desired_size
                ),
            );
        }

        if self.state == State::LoadFromTextureCache {
            if !self.cache_read_pending {
                let offset = if env.formatted_present { env.formatted_size } else { 0 };
                let size = self.desired_size - offset;
                if size <= 0 {
                    self.set_state(State::CachePost);
                    return Pass::Again;
                }
                self.file_size = 0;
                self.loaded = false;
                host.stat_cache_attempt();
                if self.url.starts_with("file://") {
                    self.cache_read_count += 1;
                    let filename = self.url[7..].to_string();
                    self.cache_read_timer = Instant::now();
                    self.cache_read_pending = true;
                    host.read_cache(Some(&filename), offset, size);
                } else if (self.url.is_empty() || self.ftype == FetchType::ServerBake) && env.can_load_from_cache {
                    self.cache_read_count += 1;
                    self.cache_read_timer = Instant::now();
                    self.cache_read_pending = true;
                    host.read_cache(None, offset, size);
                } else if !self.url.is_empty() && self.can_use_http {
                    self.set_state(State::WaitHttpResource);
                } else {
                    self.set_state(State::LoadFromNetwork);
                }
            }
            if self.loaded {
                if host.cache_read_complete() {
                    self.cache_read_pending = false;
                    self.set_state(State::CachePost);
                    host.stat_cache_hit();
                    self.cache_read_time = self.cache_read_timer.elapsed().as_secs_f32();
                } else {
                    host.log(0, false, &format!("{} this should never happen", id));
                    return Pass::Return(false);
                }
            } else {
                // Not loaded: the C++ returned false here even when it had
                // just moved on to WAIT_HTTP_RESOURCE or LOAD_FROM_NETWORK
                // without issuing a read; the next doWork call carries on.
                return Pass::Return(false);
            }
        }

        if self.state == State::CachePost {
            self.cached_size = if env.formatted_present { env.formatted_size } else { 0 };
            if self.cached_size >= self.desired_size || self.have_all_data {
                assert!(env.formatted_size > 0);
                self.loaded_discard = self.desired_discard;
                if self.loaded_discard < 0 {
                    host.log(2, false, &format!("{} mLoadedDiscard is {}, should be >=0", id, self.loaded_discard));
                }
                self.set_state(State::DecodeImage);
                self.in_cache = true;
                self.write_to_cache_state = WriteToCacheState::NotWrite;
                host.log(
                    0,
                    false,
                    &format!(
                        "{}: Cached. Bytes: {} Size: {}x{} Desired Discard: {} Desired Size: {}",
                        id, env.formatted_size, env.formatted_width, env.formatted_height, self.desired_discard, self.desired_size
                    ),
                );
                host.stat_cache_hit_rate(true);
            } else {
                if self.url.starts_with("file://") {
                    host.log(2, false, &format!("{}: abort, failed to load local file {}", id, self.url));
                    return Pass::Return(true);
                } else {
                    host.log(0, false, &format!("{}: Not in Cache", id));
                    self.set_state(State::LoadFromNetwork);
                }
                host.stat_cache_hit_rate(false);
            }
        }

        if self.state == State::LoadFromNetwork {
            let (should_retry, wait_seconds) = self.fetch_retry_policy.should_retry();
            if should_retry {
                if wait_seconds <= 0.0 {
                    host.log(1, false, &format!("{} retrying now", id));
                } else {
                    return Pass::Return(false);
                }
            }

            if env.use_http_textures && self.can_use_http && self.url.is_empty() {
                match host.region() {
                    Some(region) => {
                        let mut http_url = region.viewer_asset_url.clone();
                        if http_url.is_empty() {
                            http_url = region.http_url.clone();
                        }
                        if !http_url.is_empty() {
                            if self.ftype != FetchType::Default {
                                host.log(2, false, "Trying to fetch a texture of non-default type by UUID. This probably won't work!");
                            }
                            self.url = format!("{}/?texture_id={}", http_url, id);
                            host.log(0, false, &format!("Texture URL: {}", self.url));
                            self.write_to_cache_state = WriteToCacheState::CanWrite; // fixed texture id
                            self.can_use_capability = true;
                            self.region_retry_attempt = 0;
                            self.last_region_id = region.id;
                        } else {
                            self.can_use_capability = false;
                            if env.disconnected {
                                self.can_use_http = false;
                                return Pass::Return(true); // abort
                            } else {
                                self.region_retry_attempt += 1;
                                self.set_region_retry();
                                host.log(1, true, "Texture not available via HTTP: empty URL.");
                            }
                        }
                    }
                    None => {
                        self.can_use_capability = false;
                        self.region_retry_attempt += 1;
                        self.set_region_retry();
                        host.log(1, true, &format!("Texture not available via HTTP: no region {}", self.url));
                    }
                }
            } else if self.ftype == FetchType::ServerBake {
                self.write_to_cache_state = WriteToCacheState::CanWrite;
            }

            if self.can_use_capability && self.can_use_http && !self.url.is_empty() {
                self.set_state(State::WaitHttpResource);
                if self.write_to_cache_state != WriteToCacheState::NotWrite {
                    self.write_to_cache_state = WriteToCacheState::CanWrite;
                }
            } else if self.sent_request == RequestState::Unsent && !self.can_use_http && self.can_use_net {
                #[cfg(feature = "opensim")]
                {
                    self.write_to_cache_state = WriteToCacheState::CanWrite;
                    self.requested_size = self.desired_size;
                    self.requested_discard = self.desired_discard;
                    self.sent_request = RequestState::Queued;
                    host.add_to_network_queue();
                    host.record_texture_start(false);
                    return Pass::Return(false);
                }
                #[cfg(not(feature = "opensim"))]
                {
                    // can_use_net is never set in a Second Life-only build.
                    return Pass::Return(false);
                }
            } else {
                return Pass::Return(false);
            }
        }

        #[cfg(feature = "opensim")]
        if self.state == State::LoadFromSimulator {
            return self.load_from_simulator(host, &id);
        }

        if self.state == State::WaitHttpResource {
            if !self.http_has_resource && (fetcher.http_waiters_count() > 0 || !self.acquire_http_semaphore(fetcher)) {
                self.set_state(State::WaitHttpResource2);
                fetcher.add_http_waiter(&self.id);
                self.resource_wait_count += 1;
                return Pass::Return(false);
            }
            self.set_state(State::SendHttpReq);
        }

        if self.state == State::WaitHttpResource2 {
            return Pass::Return(false);
        }

        if self.state == State::SendHttpReq {
            if !self.can_use_http {
                self.release_http_semaphore(fetcher);
                host.log(2, false, &format!("{} abort: SEND_HTTP_REQ but !mCanUseHTTP", id));
                return Pass::Return(true); // abort
            }
            #[cfg(feature = "opensim")]
            host.remove_from_network_queue(false);

            let mut cur_size = 0;
            if env.formatted_present {
                cur_size = env.formatted_size; // amount of data we already have
                if env.formatted_discard == 0 {
                    if cur_size > 0 {
                        self.loaded_discard = env.formatted_discard;
                        if self.loaded_discard < 0 {
                            host.log(2, false, &format!("{} mLoadedDiscard is {}, should be >=0", id, self.loaded_discard));
                        }
                        self.set_state(State::DecodeImage);
                        self.release_http_semaphore(fetcher);
                        return Pass::Again;
                    } else {
                        self.release_http_semaphore(fetcher);
                        host.log(2, false, &format!("{} SEND_HTTP_REQ abort: cur_size {} <=0", id, cur_size));
                        return Pass::Return(true); // abort
                    }
                }
            }
            self.requested_size = self.desired_size;
            self.requested_discard = self.desired_discard;
            self.requested_size -= cur_size;
            self.requested_offset = cur_size;
            if self.requested_offset != 0 {
                self.requested_offset -= 1;
                self.requested_size += 1;
            }
            if self.url.is_empty() {
                host.log(2, false, &format!("HTTP GET request failed for {} on empty URL.", id));
                self.reset_formatted_data(host);
                self.release_http_semaphore(fetcher);
                return Pass::Return(true); // failed
            }
            self.requested_delta = Instant::now();
            self.loaded = false;
            self.get_status_ok = true;
            self.get_status_type = 0;
            self.get_reason.clear();
            host.log(
                0,
                false,
                &format!(
                    "HTTP GET: {} Offset: {} Bytes: {} Bandwidth(kbps): {}/{}",
                    id, self.requested_offset, self.requested_size, env.texture_bandwidth, env.max_bandwidth
                ),
            );
            let get = HttpGet {
                url: self.url.clone(),
                offset: self.requested_offset,
                size: if (self.requested_offset + self.requested_size) > HTTP_REQUESTS_RANGE_END_MAX { 0 } else { self.requested_size },
                no_range: env.range_requests_disabled,
                with_headers: self.ftype == FetchType::ServerBake,
            };
            if !host.send_http(&get) {
                self.reset_formatted_data(host);
                self.release_http_semaphore(fetcher);
                return Pass::Return(true); // failed
            }
            self.http_active = true;
            host.add_to_http_queue();
            fetcher.total_http_requests += 1;
            host.record_texture_start(true);
            self.set_state(State::WaitHttpReq);
        }

        if self.state == State::WaitHttpReq {
            if !self.loaded {
                return Pass::Return(false);
            }
            let cur_size = if env.formatted_present { env.formatted_size } else { 0 };
            if self.requested_size < 0 {
                if self.get_status_type == HTTP_NOT_FOUND {
                    if self.write_to_cache_state == WriteToCacheState::NotWrite {
                        // map tiles or server bakes
                        self.set_state(State::Done);
                        self.release_http_semaphore(fetcher);
                        if self.ftype != FetchType::MapTile {
                            host.log(2, false, &format!("{}NOT_WRITE texture missing from server (404), abort: {}", id, self.url));
                        }
                        return Pass::Return(true);
                    }
                    if self.can_use_http && !self.url.is_empty() && cur_size <= 0 {
                        let region = host.region();
                        let region_changed = match &region {
                            None => true,
                            Some(r) => self.last_region_id != r.id,
                        };
                        if region_changed {
                            if self.ftype != FetchType::MapTile {
                                host.log(
                                    1,
                                    false,
                                    &format!(
                                        "Texture missing from server (404), retrying: {} mRetryAttempt {}",
                                        self.url, self.retry_attempt
                                    ),
                                );
                            }
                            self.url.clear();
                            self.retry_attempt += 1;
                            self.last_region_id = [0; 16];
                            self.set_state(State::Init);
                            return Pass::Return(false);
                        }
                    } else if self.can_use_net {
                        self.set_state(State::Init);
                        self.can_use_http = false;
                        self.url.clear();
                        self.release_http_semaphore(fetcher);
                        return Pass::Again;
                    }
                    if self.ftype != FetchType::MapTile {
                        host.log(2, false, &format!("Texture missing from server (404): {}", self.url));
                    }
                } else if self.get_status_type == HTTP_SERVICE_UNAVAILABLE {
                    host.log(1, true, &format!("Texture server busy (503): {}", self.url));
                    if self.can_use_http && !self.url.is_empty() && cur_size <= 0 {
                        let region = host.region();
                        let region_changed = match &region {
                            None => true,
                            Some(r) => self.last_region_id != r.id,
                        };
                        if region_changed {
                            self.url.clear();
                            self.retry_attempt += 1;
                            self.last_region_id = [0; 16];
                            self.set_state(State::Init);
                            return Pass::Return(false);
                        }
                    }
                } else if self.get_status_type == HTTP_REQUESTED_RANGE_NOT_SATISFIABLE {
                    self.have_all_data = true;
                } else {
                    host.log(
                        1,
                        false,
                        &format!("HTTP GET failed for: {} Status: {} Reason: '{}'", self.url, self.get_status_type, self.get_reason),
                    );
                }

                if self.ftype != FetchType::ServerBake && self.ftype != FetchType::MapTile {
                    self.url.clear();
                }
                if cur_size > 0 {
                    self.loaded_discard = env.formatted_discard;
                    if self.loaded_discard < 0 {
                        host.log(2, false, &format!("{} mLoadedDiscard is {}, should be >=0", id, self.loaded_discard));
                    }
                    self.set_state(State::DecodeImage);
                    self.release_http_semaphore(fetcher);
                    return Pass::Again;
                }
                self.reset_formatted_data(host);
                self.set_state(State::Done);
                self.release_http_semaphore(fetcher);
                host.log(2, false, &format!("{} abort: fail harder", id));
                return Pass::Return(true); // failed
            }

            if self.write_to_cache_state != WriteToCacheState::NotWrite && self.ftype != FetchType::ServerBake {
                self.url.clear();
            }

            if !self.http_buffer_present || self.http_buffer_size == 0 {
                if self.http_buffer_present {
                    host.release_http_buffer();
                    self.http_buffer_present = false;
                    self.http_buffer_size = 0;
                }
                self.set_state(State::Done);
                host.log(2, false, &format!("{} abort: no data received", id));
                self.release_http_semaphore(fetcher);
                return Pass::Return(true);
            }

            let mut append_size = self.http_buffer_size;
            let mut total_size = cur_size + append_size;
            let mut src_offset = 0;
            assert_eq!(append_size, self.requested_size);
            if self.http_reply_offset != 0 && self.http_reply_offset != cur_size as u32 {
                if (self.http_reply_offset as i32) > cur_size || cur_size > (self.http_reply_offset as i32 + append_size) {
                    host.log(
                        2,
                        false,
                        &format!("Partial HTTP response produces break in image data for texture {}.  Aborting load.", id),
                    );
                    self.set_state(State::Done);
                    self.release_http_semaphore(fetcher);
                    return Pass::Return(true);
                }
                src_offset = cur_size - self.http_reply_offset as i32;
                append_size -= src_offset;
                total_size -= src_offset;
                self.requested_size -= src_offset; // make requested values reflect the useful part
                self.requested_offset += src_offset;
            }

            // The file size: known when all the data arrived, else flagged
            // as not fully loaded (one more than the bytes held).
            self.file_size = if self.have_all_data { total_size } else { total_size + 1 };

            if !host.assemble_http_reply(&self.url, src_offset, cur_size, append_size, total_size) {
                self.set_state(State::Done);
                host.log(2, false, &format!("{} abort: out of memory", id));
                self.release_http_semaphore(fetcher);
                return Pass::Return(true);
            }
            self.http_buffer_present = false;
            self.http_buffer_size = 0;
            self.http_reply_size = 0;
            self.http_reply_offset = 0;

            self.loaded_discard = self.requested_discard;
            let env2 = host.env();
            if self.loaded_discard < 0 || (self.loaded_discard > MAX_DISCARD_LEVEL && env2.formatted_is_j2c) {
                host.log(
                    2,
                    false,
                    &format!("{} mLoadedDiscard is {}, should be >=0 and <={}", id, self.loaded_discard, MAX_DISCARD_LEVEL),
                );
            }
            self.set_state(State::DecodeImage);
            if self.write_to_cache_state != WriteToCacheState::NotWrite {
                self.write_to_cache_state = WriteToCacheState::ShouldWrite;
            }
            self.release_http_semaphore(fetcher);
            return Pass::Again;
        }

        if self.state == State::DecodeImage {
            if env.decode_disabled {
                self.set_state(State::Done);
                return Pass::Return(true);
            }
            if self.desired_discard < 0 {
                self.set_state(State::Done);
                host.log(0, false, &format!("{} DECODE_IMAGE abort: desired discard {}<0", id, self.desired_discard));
                return Pass::Return(true);
            }
            if env.formatted_size <= 0 {
                host.log(2, false, &format!("Decode entered with invalid mFormattedImage. ID = {}", id));
                self.set_state(State::Done);
                host.log(0, false, &format!("{} DECODE_IMAGE abort: (mFormattedImage->getDataSize() <= 0)", id));
                return Pass::Return(true);
            }
            if self.loaded_discard < 0 {
                host.log(2, false, &format!("Decode entered with invalid mLoadedDiscard. ID = {}", id));
                self.set_state(State::Done);
                host.log(0, false, &format!("{} DECODE_IMAGE abort: mLoadedDiscard < 0", id));
                return Pass::Return(true);
            }
            assert!(env.formatted_present);
            let discard = if self.have_all_data && !env.formatted_is_j2c { 0 } else { self.loaded_discard };
            if discard > MAX_DISCARD_LEVEL {
                host.log(2, false, &format!("Decode entered with invalid discard. ID = {}", id));
                self.set_state(State::Done);
                host.log(0, false, &format!("{} DECODE_IMAGE abort: mLoadedDiscard > MAX_DISCARD_LEVEL", id));
                return Pass::Return(true);
            }
            self.decode_timer = Instant::now();
            host.drop_raw_images();
            self.decoded = false;
            self.set_state(State::DecodeImageUpdate);
            host.log(
                0,
                false,
                &format!("{}: Decoding. Bytes: {} Discard: {} All Data: {}", id, env.formatted_size, discard, self.have_all_data),
            );
            self.decode_handle = host.decode(discard, self.needs_aux);
            if self.decode_handle == 0 {
                self.set_state(State::Done);
                host.log(0, false, &format!("{} DECODE_IMAGE abort: failed to post for decoding", id));
                return Pass::Return(true);
            }
        }

        if self.state == State::DecodeImageUpdate {
            if self.decoded {
                self.decode_time = self.decode_timer.elapsed().as_secs_f32();
                if self.decoded_discard < 0 {
                    if self.cached_size > 0 && !self.in_local_cache && self.retry_attempt == 0 {
                        host.log(0, false, &format!("{}: Decode of cached file failed (removed), retrying", id));
                        assert_eq!(self.decode_handle, 0);
                        host.drop_formatted_image();
                        self.retry_attempt += 1;
                        self.set_state(State::Init);
                        return Pass::Again;
                    } else {
                        host.log(0, false, &format!("Failed to Decode image {} after {} retries", id, self.retry_attempt));
                        self.set_state(State::Done); // failed
                    }
                } else {
                    host.log(
                        0,
                        false,
                        &format!("{}: Decoded. Discard: {} Raw Image: {}x{}", id, self.decoded_discard, env.raw_width, env.raw_height),
                    );
                    self.set_state(State::WriteToCache);
                }
            } else {
                return Pass::Return(false);
            }
        }

        if self.state == State::WriteToCache {
            let env = host.env();
            if self.write_to_cache_state != WriteToCacheState::ShouldWrite || !env.formatted_present {
                self.set_state(State::Done);
                return Pass::Again;
            }
            let datasize = env.formatted_size;
            if self.file_size < datasize {
                // HTTP fetching and sim fetching mixed.
                self.file_size = if self.have_all_data { datasize } else { datasize + 1 };
            }
            assert!(datasize != 0);
            self.written = false;
            self.set_state(State::WaitOnWrite);
            self.cache_write_count += 1;
            self.cache_write_timer = Instant::now();
            self.cache_write_pending = true;
            host.write_cache(self.file_size, self.decoded_discard);
        }

        if self.state == State::WaitOnWrite {
            if self.write_to_cache_complete(host) {
                self.cache_write_time = self.cache_write_timer.elapsed().as_secs_f32();
                self.set_state(State::Done);
            } else {
                if self.desired_discard < self.decoded_discard {
                    host.prioritize_write();
                }
                return Pass::Return(false);
            }
        }

        if self.state == State::Done {
            if self.decoded_discard >= 0 && self.desired_discard < self.decoded_discard {
                self.set_state(State::Init);
                host.log(
                    0,
                    false,
                    &format!(
                        "{} more data requested, returning to INIT:  mDecodedDiscard {}>= 0 && mDesiredDiscard {}< mDecodedDiscard {}",
                        id, self.decoded_discard, self.desired_discard, self.decoded_discard
                    ),
                );
                return Pass::Again;
            } else {
                self.fetch_time = self.fetch_timer.elapsed().as_secs_f32();
                return Pass::Return(true);
            }
        }

        Pass::Return(false)
    }

    /// `writeToCacheComplete`.
    fn write_to_cache_complete(&mut self, host: &mut dyn Host) -> bool {
        if self.cache_write_pending {
            if !self.written {
                return false;
            }
            if host.cache_write_complete() {
                self.cache_write_pending = false;
            } else {
                return false;
            }
        }
        true
    }

    #[cfg(feature = "opensim")]
    fn clear_packets(&mut self) {
        self.packets.clear();
        self.total_packets = 0;
        self.last_packet = -1;
        self.first_packet = 0;
    }

    #[cfg(feature = "opensim")]
    fn load_from_simulator(&mut self, host: &mut dyn Host, id: &str) -> Pass {
        // mFormattedImage defaults to a J2C image on the host side.
        if self.process_simulator_packets(host) {
            let mut byte_count = 0.0f64;
            if self.last_packet >= self.first_packet {
                for i in self.first_packet..=self.last_packet {
                    if let Some(Some(p)) = self.packets.get(i as usize) {
                        byte_count += p.size as f64;
                    }
                }
            }
            let env = host.env();
            host.log(0, false, &format!("{}: Loaded from Sim. Bytes: {}", id, env.formatted_size));
            host.remove_from_network_queue(false);
            if !env.formatted_present || env.formatted_size == 0 {
                host.log(2, false, &format!("{} processSimulatorPackets() failed to load buffer", id));
                return Pass::Return(true); // failed
            }
            if self.loaded_discard < 0 {
                host.log(2, false, &format!("{} mLoadedDiscard is {}, should be >=0", id, self.loaded_discard));
            }
            self.set_state(State::DecodeImage);
            self.write_to_cache_state = WriteToCacheState::ShouldWrite;
            host.record_texture_done(false, byte_count);
            Pass::Again
        } else {
            host.add_to_network_queue(); // failsafe
            host.record_texture_start(false);
            Pass::Return(false)
        }
    }

    /// OpenSim: `processSimulatorPackets`.
    #[cfg(feature = "opensim")]
    fn process_simulator_packets(&mut self, host: &mut dyn Host) -> bool {
        let env = host.env();
        if !env.formatted_present || self.requested_size < 0 {
            assert_eq!(self.decode_handle, 0);
            host.drop_formatted_image();
            return true;
        }
        if self.last_packet >= self.first_packet {
            let mut buffer_size = env.formatted_size;
            for i in self.first_packet..=self.last_packet {
                let p = self.packets.get(i as usize).and_then(|p| p.as_ref()).expect("packet present");
                buffer_size += p.size as i32;
            }
            let have_all_data = self.last_packet >= self.total_packets as i32 - 1;
            if self.requested_size <= 0 {
                return true;
            }
            if buffer_size >= self.requested_size || have_all_data {
                if have_all_data {
                    self.have_all_data = true;
                }
                if buffer_size > env.formatted_size {
                    host.assemble_packets(self.first_packet, self.last_packet);
                }
                self.loaded_discard = self.requested_discard;
                return true;
            }
        }
        false
    }

    /// OpenSim: `setupPacketData` over the cached data's size. The host
    /// then does what the C++ did with its containers: on `BadSize` it
    /// removes the texture from the cache and resets the formatted data,
    /// on `Refetch` (cached through HTTP, the first packet is fetched
    /// again) it resets the formatted data; the packets are cleared here.
    #[cfg(feature = "opensim")]
    pub fn setup_packet_data(&mut self, data_size: i32) -> SetupPackets {
        if data_size > 0 {
            self.first_packet = (data_size - FIRST_PACKET_SIZE) / MAX_IMG_PACKET_SIZE + 1;
            if FIRST_PACKET_SIZE + (self.first_packet - 1) * MAX_IMG_PACKET_SIZE != data_size {
                self.clear_packets();
                return SetupPackets::BadSize;
            } else if self.file_size > 0 {
                self.last_packet = self.first_packet - 1;
                self.total_packets = ((self.file_size - FIRST_PACKET_SIZE + MAX_IMG_PACKET_SIZE - 1) / MAX_IMG_PACKET_SIZE + 1) as u16;
            } else {
                self.clear_packets();
                return SetupPackets::Refetch;
            }
        }
        SetupPackets::Ok
    }

    /// OpenSim: `receiveImageHeader`'s fields and `mSentRequest = SENT_SIM`
    /// bookkeeping of `sendRequestListToSimulators`.
    #[cfg(feature = "opensim")]
    pub fn receive_header(&mut self, codec: i32, total_packets: u16, file_size: i32) {
        self.image_codec = codec;
        self.total_packets = total_packets;
        self.file_size = file_size;
    }

    /// OpenSim: the request was sent to the simulator.
    #[cfg(feature = "opensim")]
    pub fn mark_sent_sim(&mut self) {
        self.sent_request = RequestState::SentSim;
        self.sim_requested_discard = self.desired_discard;
        self.requested_priority = self.image_priority;
        self.requested_delta = Instant::now();
    }

    /// OpenSim: `insertPacket`.
    #[cfg(feature = "opensim")]
    pub fn insert_packet(&mut self, index: i32, size: u32) -> bool {
        self.requested_delta = Instant::now();
        if index >= self.total_packets as i32 {
            return false;
        }
        if index > 0 && index < self.total_packets as i32 - 1 && size != MAX_IMG_PACKET_SIZE as u32 {
            return false;
        }
        if index >= self.packets.len() as i32 {
            self.packets.resize(index as usize + 1, None);
        } else if self.packets[index as usize].is_some() {
            return false;
        }
        self.packets[index as usize] = Some(PacketInfo { size });
        while self.last_packet + 1 < self.packets.len() as i32 && self.packets[(self.last_packet + 1) as usize].is_some() {
            self.last_packet += 1;
        }
        true
    }

    // ---- events ----------------------------------------------------------------

    /// `callbackCacheRead` (under the work mutex): `image_present` is
    /// whether the responder delivered a formatted image, `data_size` its
    /// byte count, `codec` its codec.
    pub fn cache_read_done(&mut self, success: bool, imagesize: i32, data_size: i32, codec: i32, is_local: bool) {
        if self.state != State::LoadFromTextureCache {
            return;
        }
        if success {
            assert!(imagesize >= 0);
            self.file_size = imagesize;
            self.image_codec = codec;
            self.in_local_cache = is_local;
            if self.file_size != 0 && data_size >= self.file_size {
                self.have_all_data = true;
            }
        }
        self.loaded = true;
    }

    /// `callbackCacheWrite`.
    pub fn cache_write_done(&mut self, _success: bool) {
        if self.state != State::WaitOnWrite {
            return;
        }
        self.written = true;
    }

    /// `callbackDecoded`: `success` with the formatted image's discard
    /// level (`mDecodedDiscard = mFormattedImage->getDiscardLevel()`), or a
    /// failure (the host removes the texture from the cache). Returns
    /// whether the callback was accepted (the host then keeps the raw
    /// images; otherwise it discards them).
    pub fn decode_done(&mut self, success: bool, formatted_discard: i32, decode_id: u32, host: &mut dyn Host) -> bool {
        if self.decode_handle == 0 {
            return false; // aborted, ignore
        }
        if self.decode_handle != decode_id {
            host.log(0, false, &format!("{} received obsolete decode's callback", uuid_to_string(&self.id)));
            return false; // ignore
        }
        if self.state != State::DecodeImageUpdate {
            host.log(0, false, &format!("Decode callback for {} with state = {}", uuid_to_string(&self.id), self.state as i32));
            self.decode_handle = 0;
            return false;
        }
        self.decode_handle = 0;
        if success {
            self.decoded_discard = formatted_discard;
            if self.decoded_discard < self.desired_discard {
                host.log(2, true, "Decoded higher resolution than requested");
            }
        } else {
            host.remove_from_cache();
            self.decoded_discard = -1;
        }
        self.decoded = true;
        true
    }

    /// `endWork`: the decode handle is dropped and the formatted image
    /// released by the host.
    pub fn end_work(&mut self) {
        self.decode_handle = 0;
    }

    /// `getRequestFinished`'s timer resets after it read the times.
    pub fn reset_timers(&mut self) {
        let now = Instant::now();
        self.cache_read_timer = now;
        self.decode_timer = now;
        self.cache_write_timer = now;
        self.fetch_timer = now;
        self.state_timer = now;
    }

    /// `onCompleted` and `callbackHttpGet` (under the work mutex). The host
    /// has applied the fake-failure setting and logging; it passes the
    /// response's status (`ok`, its type for the policy: the HTTP status or
    /// the LLCore status type, `code`, its string), whether it is a 206, the
    /// body size (0 when none), the `Content-Range` triple when present and
    /// a parsed `Retry-After`. Returns the data size for the HTTP queue
    /// accounting, or `None` when the request is being retried (the host
    /// then removes it from the HTTP queue with 0 bytes and returns).
    #[allow(clippy::too_many_arguments)]
    pub fn http_completed(
        &mut self,
        fetcher: &mut Fetcher,
        host: &mut dyn Host,
        status_ok: bool,
        status_type: i32,
        reason: &str,
        partial: bool,
        body_size: i32,
        range: Option<(u32, u32, u32)>,
        retry_after: Option<f32>,
    ) -> Option<i32> {
        self.http_active = false;
        let id = uuid_to_string(&self.id);
        if !status_ok && self.ftype == FetchType::ServerBake {
            host.log(1, false, &format!("{} state {}", id, self.state.name()));
            self.fetch_retry_policy.on_failure(status_type, retry_after);
            let (should_retry, retry_after_secs) = self.fetch_retry_policy.should_retry();
            if should_retry {
                host.log(
                    1,
                    false,
                    &format!("{} will retry after {} seconds, resetting state to LOAD_FROM_NETWORK", id, retry_after_secs),
                );
                self.get_status_ok = status_ok;
                self.get_status_type = status_type;
                self.get_reason = reason.to_string();
                self.release_http_semaphore(fetcher);
                self.set_state(State::LoadFromNetwork);
                return None;
            } else {
                host.log(1, false, &format!("{} will not retry", id));
            }
        } else {
            self.fetch_retry_policy.on_success();
        }
        self.get_status_ok = status_ok;
        self.get_status_type = status_type;
        self.get_reason = reason.to_string();
        host.log(0, false, &format!("HTTP COMPLETE: {} status: {} '{}'", id, status_type, reason));
        let mut success = true;
        let mut is_partial = false;
        if !status_ok {
            success = false;
            if self.ftype != FetchType::MapTile {
                host.log(2, false, &format!("CURL GET FAILED, status: {} reason: {}", status_type, reason));
            }
        } else {
            is_partial = partial;
        }
        let data_size = self.callback_http_get(host, is_partial, success, body_size, range);
        host.record_texture_done(true, data_size as f64);
        Some(data_size)
    }

    fn callback_http_get(&mut self, host: &mut dyn Host, partial: bool, success: bool, body_size: i32, range: Option<(u32, u32, u32)>) -> i32 {
        let id = uuid_to_string(&self.id);
        let mut data_size = 0;
        if self.state != State::WaitHttpReq {
            host.log(
                2,
                false,
                &format!("callbackHttpGet for unrequested fetch worker: {} req={} state= {}", id, self.sent_request as i32, self.state as i32),
            );
            return data_size;
        }
        if self.loaded {
            host.log(2, false, &format!("Duplicate callback for {}", id));
            return data_size; // ignore duplicate callback
        }
        if success {
            data_size = body_size;
            host.log(0, false, &format!("HTTP RECEIVED: {} Bytes: {}", id, data_size));
            if data_size > 0 {
                assert!(!self.http_buffer_present);
                // The host keeps the body (addRef) as mHttpBufferArray.
                self.http_buffer_present = true;
                self.http_buffer_size = data_size;
                if partial {
                    match range {
                        Some((offset, length, _full)) if !(offset == 0 && length == 0) => {
                            self.http_reply_size = length;
                            self.http_reply_offset = offset;
                        }
                        _ => {
                            self.http_reply_size = data_size as u32;
                            self.http_reply_offset = self.requested_offset as u32;
                        }
                    }
                }
                if !partial {
                    if data_size <= self.requested_offset {
                        let formatted_size = host.env().formatted_size;
                        host.log(
                            2,
                            false,
                            &format!(
                                "Fetched entire texture {} when it was expected to be marked complete.  mImageSize:  {} datasize:  {}",
                                id, self.file_size, formatted_size
                            ),
                        );
                    }
                    self.have_all_data = true;
                    assert_eq!(self.decode_handle, 0);
                    host.drop_formatted_image(); // discard any previous data we had
                } else if data_size < self.requested_size {
                    self.have_all_data = true;
                } else if data_size > self.requested_size {
                    host.log(2, false, &format!("data_size = {} > requested: {}", data_size, self.requested_size));
                    self.have_all_data = true;
                    assert_eq!(self.decode_handle, 0);
                    host.drop_formatted_image();
                }
            } else {
                self.have_all_data = true;
            }
            self.requested_size = data_size;
        } else {
            self.requested_size = -1; // error
        }
        self.loaded = true;
        data_size
    }

    /// `deleteOK`'s state part; the host adds the handle checks.
    pub fn delete_ok_state(&self, fetcher: &Fetcher, have_work: bool) -> bool {
        let mut delete_ok = true;
        if self.http_active {
            delete_ok = false;
        }
        if self.state == State::WaitHttpResource2 && fetcher.is_http_waiter(&self.id) {
            delete_ok = false;
        }
        if have_work && self.state >= State::WriteToCache && self.state <= State::WaitOnWrite {
            delete_ok = false;
        }
        delete_ok
    }

    /// `getFetchState`'s data progress for the current state.
    pub fn data_progress(&self, formatted_size: Option<i32>) -> f32 {
        if self.file_size > 0 {
            #[cfg(feature = "opensim")]
            if self.state == State::LoadFromSimulator {
                let data_size = (FIRST_PACKET_SIZE + (self.last_packet - 1) * MAX_IMG_PACKET_SIZE).max(0);
                return data_size as f32 / self.file_size as f32;
            }
            if let Some(size) = formatted_size {
                return size as f32 / self.file_size as f32;
            }
        }
        0.0
    }

    /// `getFetchState`'s requested priority.
    pub fn requested_priority_for_state(&self) -> f32 {
        if self.state >= State::LoadFromNetwork && self.state <= State::WaitHttpReq {
            self.requested_priority
        } else {
            self.image_priority
        }
    }
}

enum Pass {
    Return(bool),
    Again,
}

/// `createRequest`'s planning: the desired size and discard for a request.
pub mod plan {
    use super::budget::{calc_data_size_j2c, DEFAULT_RATE, FIRST_PACKET_SIZE, MAX_DISCARD_LEVEL, MAX_IMAGE_DATA_SIZE};
    use super::FetchType;

    /// The desired size and discard level of a new request:
    /// `url_is_non_j2c` is whether the URL has an extension of another
    /// codec.
    pub fn desired_size(ftype: FetchType, url_non_empty: bool, url_is_non_j2c: bool, w: i32, h: i32, c: i32, desired_discard: i32) -> (i32, i32) {
        if ftype == FetchType::ServerBake && url_non_empty && url_is_non_j2c {
            (MAX_IMAGE_DATA_SIZE, 0)
        } else if url_non_empty && url_is_non_j2c {
            (MAX_IMAGE_DATA_SIZE, 0)
        } else if desired_discard == 0 {
            (MAX_IMAGE_DATA_SIZE, desired_discard)
        } else if w * h * c > 0 {
            (calc_data_size_j2c(w, h, c, desired_discard, DEFAULT_RATE), desired_discard)
        } else {
            (FIRST_PACKET_SIZE, MAX_DISCARD_LEVEL)
        }
    }

    /// The `createRequest` check on an existing, finished worker: nothing
    /// to do when it already delivered what is asked.
    pub fn already_done(worker_state_done: bool, worker_desired_size: i32, worker_desired_discard: i32, desired_size: i32, desired_discard: i32) -> bool {
        worker_state_done && worker_desired_size == desired_size.max(FIRST_PACKET_SIZE) && worker_desired_discard == desired_discard
    }
}

#[cfg(test)]
mod tests;
