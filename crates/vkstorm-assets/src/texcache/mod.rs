//! `LLTextureCache`'s data side: the entries index (`texture.entries`), the
//! header-data file (`texture.cache`, one 600-byte record per entry), the
//! body files (`texturecache/[0-f]/<uuid>.texture`), the fast cache
//! (`FastCache.cache`, one 1040-byte record per entry) and their purge,
//! corruption and LRU rules, byte-compatible with the files the C++ wrote.
//!
//! The on-disk format is unchanged by design: the cache version stays 1.71,
//! the entries header is the same 44 bytes (`F32` version, `U32` address
//! size, 32-byte encoder string, `U32` entry count, little-endian), an entry
//! the same 28 bytes (UUID, `S32` image size, `S32` body size, `U32` time),
//! and the engine string is checked as before (its change purges the cache
//! once). No migration is needed: a cache written by the C++ viewer is read
//! as it was, and vice versa.
//!
//! Threading follows the C++: the index is behind one lock (the header
//! mutex), the fast cache behind another; header-record and body-file
//! transfers run without either, as the worker thread did. The host's
//! worker threads drive [`TextureCache::read`] and [`TextureCache::write`];
//! the lifecycle of the C++ worker objects stays in the viewer.

pub mod files;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::Instant;

use crate::host::ClockFn;
use crate::{Uuid, ll_debugs, ll_infos, ll_warns, uuid_to_string};

/// `TEXTURE_CACHE_ENTRY_SIZE` (= `FIRST_PACKET_SIZE`): the header record.
pub const ENTRY_SIZE: i32 = 600;
/// `TEXTURE_CACHE_PURGE_AMOUNT`: the fraction removed when over the limit.
pub const PURGE_AMOUNT: f32 = 0.20;
/// `TEXTURE_CACHE_LRU_SIZE`: the fraction of entries kept in the LRU list.
pub const LRU_SIZE: f32 = 0.10;
/// The fast cache record: `w, h, c, level` then 16x16x4 bytes of pixels.
pub const FAST_ENTRY_OVERHEAD: i32 = 16;
pub const FAST_DATA_SIZE: i32 = 16 * 16 * 4;
pub const FAST_ENTRY_SIZE: i32 = FAST_DATA_SIZE + FAST_ENTRY_OVERHEAD;
/// `TEXTURE_LAZY_PURGE_TIME_LIMIT` in seconds.
pub const LAZY_PURGE_TIME_LIMIT: f32 = 0.004;
/// `TEXTURE_PRUNING_MAX_TIME` in seconds.
pub const PRUNING_MAX_TIME: f32 = 15.0;
/// `sHeaderEncoderStringSize`.
pub const HEADER_ENCODER_STRING_SIZE: usize = 32;
/// `sHeaderCacheVersion`.
pub const HEADER_CACHE_VERSION: f32 = 1.71;
/// `sizeof(EntriesInfo)` and `sizeof(Entry)` as the C++ laid them out.
pub const ENTRIES_INFO_SIZE: usize = 44;
pub const ENTRY_RECORD_SIZE: usize = 28;
/// The default `sCacheMaxEntries` (about a million textures).
pub const DEFAULT_MAX_ENTRIES: u32 = 1024 * 1024;
/// The fast cache handle's idle timeout (`closeFastCache`).
const FAST_CACHE_TIMEOUT_SECS: f32 = 10.0;

const SUBDIRS: &[u8; 16] = b"0123456789abcdef";

/// `LLTextureCache::EntriesInfo`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntriesInfo {
    pub version: f32,
    pub address_size: u32,
    pub encoder_version: [u8; HEADER_ENCODER_STRING_SIZE],
    pub entries: u32,
}

impl Default for EntriesInfo {
    fn default() -> Self {
        EntriesInfo {
            version: 0.0,
            address_size: 0,
            encoder_version: [0; HEADER_ENCODER_STRING_SIZE],
            entries: 0,
        }
    }
}

impl EntriesInfo {
    pub fn encode(&self) -> [u8; ENTRIES_INFO_SIZE] {
        let mut b = [0u8; ENTRIES_INFO_SIZE];
        b[0..4].copy_from_slice(&self.version.to_le_bytes());
        b[4..8].copy_from_slice(&self.address_size.to_le_bytes());
        b[8..40].copy_from_slice(&self.encoder_version);
        b[40..44].copy_from_slice(&self.entries.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8; ENTRIES_INFO_SIZE]) -> EntriesInfo {
        let mut enc = [0u8; HEADER_ENCODER_STRING_SIZE];
        enc.copy_from_slice(&b[8..40]);
        EntriesInfo {
            version: f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            address_size: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            encoder_version: enc,
            entries: u32::from_le_bytes([b[40], b[41], b[42], b[43]]),
        }
    }

    /// The encoder string up to its NUL, as `strcmp` compared it.
    pub fn encoder_str(&self) -> &[u8] {
        let n = self
            .encoder_version
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(HEADER_ENCODER_STRING_SIZE);
        &self.encoder_version[..n]
    }
}

/// `LLTextureCache::Entry`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub id: Uuid,
    /// Total size of the image if known; -1 marks a brand-new entry.
    pub image_size: i32,
    /// Size of the body file.
    pub body_size: i32,
    /// Seconds since 1970.
    pub time: u32,
}

impl Entry {
    pub fn encode(&self) -> [u8; ENTRY_RECORD_SIZE] {
        let mut b = [0u8; ENTRY_RECORD_SIZE];
        b[0..16].copy_from_slice(&self.id);
        b[16..20].copy_from_slice(&self.image_size.to_le_bytes());
        b[20..24].copy_from_slice(&self.body_size.to_le_bytes());
        b[24..28].copy_from_slice(&self.time.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8; ENTRY_RECORD_SIZE]) -> Entry {
        let mut id = [0u8; 16];
        id.copy_from_slice(&b[0..16]);
        Entry {
            id,
            image_size: i32::from_le_bytes([b[16], b[17], b[18], b[19]]),
            body_size: i32::from_le_bytes([b[20], b[21], b[22], b[23]]),
            time: u32::from_le_bytes([b[24], b[25], b[26], b[27]]),
        }
    }
}

/// The directories and files of one cache location (`setDirNames`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paths {
    /// `getExpandedFilename(location, "")`.
    pub parent_dir: String,
    /// `<location>/texturecache/texture.entries`.
    pub entries_file: String,
    /// `<location>/texturecache/texture.cache`.
    pub data_file: String,
    /// `<location>/texturecache`.
    pub textures_dir: String,
    /// `<location>/texturecache/FastCache.cache`.
    pub fast_file: String,
    /// `<location>/textures`: the legacy cache directory older viewers used.
    pub legacy_textures_dir: String,
    /// `<location>/texture.entries` and `<location>/texture.cache`, the
    /// legacy index files next to it.
    pub legacy_entries_file: String,
    pub legacy_data_file: String,
    /// The platform's directory delimiter (`\` or `/`).
    pub delim: String,
}

impl Paths {
    /// Builds the paths as `setDirNames` did from a location directory.
    pub fn for_location(location: &str, delim: &str) -> Paths {
        let join = |a: &str, b: &str| {
            if a.is_empty() {
                b.to_string()
            } else {
                format!("{a}{delim}{b}")
            }
        };
        let textures_dir = join(location, "texturecache");
        Paths {
            parent_dir: location.to_string(),
            entries_file: join(&textures_dir, "texture.entries"),
            data_file: join(&textures_dir, "texture.cache"),
            fast_file: join(&textures_dir, "FastCache.cache"),
            legacy_textures_dir: join(location, "textures"),
            legacy_entries_file: join(location, "texture.entries"),
            legacy_data_file: join(location, "texture.cache"),
            textures_dir,
            delim: delim.to_string(),
        }
    }
}

/// `getTextureFileName`: `<textures_dir>/<first hex digit>/<uuid>.texture`.
pub fn texture_file_name(paths: &Paths, id: &Uuid) -> String {
    let idstr = uuid_to_string(id);
    let first = &idstr[..1];
    format!(
        "{}{}{}{}{}.texture",
        paths.textures_dir, paths.delim, first, paths.delim, idstr
    )
}

/// The static configuration the C++ kept in class statics.
#[derive(Clone, Debug)]
pub struct Config {
    /// `sHeaderCacheVersion`.
    pub version: f32,
    /// `sHeaderCacheAddressSize` (`ADDRESS_SIZE`).
    pub address_size: u32,
    /// `sHeaderCacheEncoderVersion` (`LLImageJ2C::getEngineInfo()`).
    pub encoder_version: String,
    /// `sCacheMaxEntries` before `initCache` reduces it.
    pub max_entries: u32,
    /// `sCacheMaxTexturesSize` (0 = no limit until `initCache`).
    pub max_textures_size: i64,
    /// The clock entries are stamped with.
    pub clock: ClockFn,
}

impl Config {
    pub fn new(encoder_version: &str, address_size: u32) -> Config {
        Config {
            version: HEADER_CACHE_VERSION,
            address_size,
            encoder_version: encoder_version.to_string(),
            max_entries: DEFAULT_MAX_ENTRIES,
            max_textures_size: 0,
            clock: crate::host::system_seconds,
        }
    }
}

/// The outcome of a cache read (`LLTextureCacheRemoteWorker::doRead`).
#[derive(Debug, Default)]
pub struct ReadOutcome {
    /// `mDataSize` after the read: the bytes in `data`, 0 when the texture
    /// is not cached, -1 on failure (the C++ then removed the entry).
    pub data_size: i32,
    /// The bytes read (header record bytes then body bytes).
    pub data: Vec<u8>,
    /// The entry's image size.
    pub image_size: i32,
}

/// What goes into the fast cache with a write: the raw image at a level
/// that fits 16x16x4 bytes (the host scales it; see [`fast_cache_level`]).
#[derive(Debug, Clone)]
pub struct FastEntry {
    pub width: i32,
    pub height: i32,
    pub components: i32,
    pub discard_level: i32,
    pub data: Vec<u8>,
}

/// A fast cache record as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastRecord {
    pub width: i32,
    pub height: i32,
    pub components: i32,
    pub discard_level: i32,
    pub data: Vec<u8>,
}

/// [`TextureCache::try_read_fast`]'s answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FastRead {
    /// Another thread holds the fast cache (a write is in progress); nothing
    /// was read and nothing waited.
    Busy,
    /// Not in the cache, or the record could not be read (as
    /// [`TextureCache::read_fast`] returning `None`).
    Miss,
    /// The record.
    Hit(FastRecord),
}

/// `writeToFastCache`'s search for the level that fits the record: the
/// number of halvings of the raw image's dimensions.
pub fn fast_cache_level(width: i32, height: i32, components: i32) -> i32 {
    let mut i = 0;
    while ((width >> i) * (height >> i) * components) > FAST_DATA_SIZE {
        i += 1;
    }
    i
}

/// The state behind the header mutex.
struct HeaderState {
    info: EntriesInfo,
    free_list: BTreeSet<i32>,
    lru: BTreeSet<Uuid>,
    header_id_map: BTreeMap<Uuid, i32>,
    textures_size_map: BTreeMap<Uuid, i32>,
    textures_size_total: i64,
    updated_entry_map: BTreeMap<i32, Entry>,
    purge_entry_list: Vec<(i32, Entry)>,
    /// `mHeaderAPRFile`: `Some` while the slot is open; the inner `None` is
    /// a handle whose open failed (every transfer on it returns 0).
    header_file: Option<Option<files::OpenFile>>,
    /// The function-local static of `updateEntryTimeStamp`, fixed at its
    /// first evaluation.
    max_entries_without_time_stamp: Option<u32>,
    read_only: bool,
    paths: Paths,
    max_entries: u32,
    max_textures_size: i64,
}

/// The fast cache's handle slot: `mFastCachep`, which may hold a file that
/// failed to open (every transfer on it then fails) until the timeout
/// closes it.
struct FastState {
    file: Option<Option<files::OpenFile>>,
    opened: Instant,
    first_time_done: bool,
}

/// The texture cache.
pub struct TextureCache {
    config: Config,
    header: Mutex<HeaderState>,
    fast: Mutex<FastState>,
    do_purge: AtomicBool,
}

impl TextureCache {
    pub fn new(config: Config) -> TextureCache {
        let state = HeaderState {
            info: EntriesInfo::default(),
            free_list: BTreeSet::new(),
            lru: BTreeSet::new(),
            header_id_map: BTreeMap::new(),
            textures_size_map: BTreeMap::new(),
            textures_size_total: 0,
            updated_entry_map: BTreeMap::new(),
            purge_entry_list: Vec::new(),
            header_file: None,
            max_entries_without_time_stamp: None,
            // Do not allow changes until set_read_only is called.
            read_only: true,
            paths: Paths::default(),
            max_entries: config.max_entries,
            max_textures_size: config.max_textures_size,
        };
        TextureCache {
            config,
            header: Mutex::new(state),
            fast: Mutex::new(FastState {
                file: None,
                opened: Instant::now(),
                first_time_done: false,
            }),
            do_purge: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HeaderState> {
        self.header.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now(&self) -> u32 {
        (self.config.clock)()
    }

    /// `setReadOnly`.
    pub fn set_read_only(&self, read_only: bool) {
        self.lock().read_only = read_only;
    }

    pub fn is_read_only(&self) -> bool {
        self.lock().read_only
    }

    /// `getUsage`.
    pub fn usage(&self) -> i64 {
        self.lock().textures_size_total
    }

    /// `getMaxUsage`.
    pub fn max_usage(&self) -> i64 {
        self.lock().max_textures_size
    }

    /// `getEntries`.
    pub fn entries(&self) -> u32 {
        self.lock().info.entries
    }

    /// `getMaxEntries`.
    pub fn max_entries(&self) -> u32 {
        self.lock().max_entries
    }

    /// `isInCache`.
    pub fn is_in_cache(&self, id: &Uuid) -> bool {
        self.lock().header_id_map.contains_key(id)
    }

    /// `getTextureFileName`.
    pub fn texture_file_name(&self, id: &Uuid) -> String {
        texture_file_name(&self.lock().paths, id)
    }

    /// The current paths (`setDirNames` result).
    pub fn paths(&self) -> Paths {
        self.lock().paths.clone()
    }

    /// Whether a purge is pending (`mDoPurge`).
    pub fn purge_pending(&self) -> bool {
        self.do_purge.load(Ordering::Relaxed)
    }

    /// `purgeCache(location, remove_dir)`: removes the legacy cache if it
    /// exists, then the current one.
    pub fn purge_cache(&self, paths: &Paths, remove_dir: bool) {
        let mut s = self.lock();
        if !s.read_only {
            s.paths = paths.clone();
            debug_assert!(s.header_file.is_none());
            let texture_dir = s.paths.textures_dir.clone();
            s.paths.textures_dir = paths.legacy_textures_dir.clone();
            if files::is_dir(&s.paths.textures_dir) {
                files::remove_file(&paths.legacy_entries_file);
                files::remove_file(&paths.legacy_data_file);
                self.purge_all_textures(&mut s, true);
            }
            s.paths.textures_dir = texture_dir;
        }
        self.purge_all_textures(&mut s, remove_dir);
    }

    /// `initCache`: sizes the cache, creates the directories, reads the
    /// index, purges (validating 1/256th of the bodies when `validate_idx`
    /// is given) and opens the fast cache. Returns the unused part of
    /// `max_size`.
    pub fn init_cache(
        &self,
        paths: &Paths,
        max_size: i64,
        texture_cache_mismatch: bool,
        validate_idx: Option<u32>,
    ) -> i64 {
        let mut max_size = max_size;
        {
            let mut s = self.lock();
            let mut entries_size = (max_size * 36) / 100;
            let max_entries = entries_size / (ENTRY_SIZE + FAST_ENTRY_SIZE) as i64;
            s.max_entries = (s.max_entries as i64).min(max_entries) as u32;
            entries_size = s.max_entries as i64 * (ENTRY_SIZE + FAST_ENTRY_SIZE) as i64;
            max_size -= entries_size;
            if s.max_textures_size > 0 {
                s.max_textures_size = s.max_textures_size.min(max_size);
            } else {
                s.max_textures_size = max_size;
            }
            max_size -= s.max_textures_size;
            ll_infos!(
                "Headers: {} Textures size: {} MB",
                s.max_entries,
                s.max_textures_size / (1024 * 1024)
            );
            s.paths = paths.clone();
            if texture_cache_mismatch {
                self.purge_all_textures(&mut s, true);
                if s.read_only {
                    return max_size;
                }
            }
            if !s.read_only {
                if !files::is_dir(&s.paths.parent_dir) {
                    files::mkdir(&s.paths.parent_dir);
                }
                files::mkdir(&s.paths.textures_dir);
                make_subdirs(&s.paths);
            }
        }
        self.read_header_cache();
        self.purge_textures(validate_idx);
        self.open_fast_cache(true);
        max_size
    }

    // ---- the header entries file --------------------------------------------
    // The header mutex is held by the caller of everything that takes a
    // `&mut HeaderState`.

    fn open_header_entries_file(s: &mut HeaderState, readonly: bool, offset: i64) {
        debug_assert!(s.header_file.is_none());
        let mut f = files::OpenFile::open(&s.paths.entries_file, readonly);
        if offset > 0 {
            if let Some(f) = f.as_mut() {
                f.seek_set(offset);
            }
        }
        s.header_file = Some(f);
    }

    fn close_header_entries_file(s: &mut HeaderState) {
        s.header_file = None;
    }

    fn header_file_open(s: &HeaderState) -> bool {
        s.header_file.is_some()
    }

    fn hf_write(s: &mut HeaderState, data: &[u8]) -> i32 {
        match s.header_file.as_mut().and_then(|f| f.as_mut()) {
            Some(f) => f.write(data),
            None => 0,
        }
    }

    fn hf_read(s: &mut HeaderState, buf: &mut [u8]) -> i32 {
        match s.header_file.as_mut().and_then(|f| f.as_mut()) {
            Some(f) => f.read(buf),
            None => 0,
        }
    }

    fn hf_seek_set(s: &mut HeaderState, offset: i64) {
        if let Some(f) = s.header_file.as_mut().and_then(|f| f.as_mut()) {
            f.seek_set(offset);
        }
    }

    fn hf_seek_cur(s: &mut HeaderState, delta: i64) {
        if let Some(f) = s.header_file.as_mut().and_then(|f| f.as_mut()) {
            f.seek_cur(delta);
        }
    }

    fn read_entries_header(&self, s: &mut HeaderState) {
        debug_assert!(s.header_file.is_none());
        if files::exists(&s.paths.entries_file) {
            // Read into the current info: bytes a short read does not cover
            // keep their previous values, as the C++ member did.
            let mut buf = s.info.encode();
            files::read_at(&s.paths.entries_file, 0, &mut buf);
            s.info = EntriesInfo::decode(&buf);
        } else {
            self.set_entries_header(s);
            Self::write_entries_header(s);
        }
    }

    fn set_entries_header(&self, s: &mut HeaderState) {
        let enc = self.config.encoder_version.as_bytes();
        if HEADER_ENCODER_STRING_SIZE < enc.len() + 1 {
            // LL_ERRS in the C++: the string cannot fit the field.
            panic!("Version string doesn't fit in header");
        }
        s.info.version = self.config.version;
        s.info.address_size = self.config.address_size;
        // strcpy: the string and its NUL; bytes after it keep their content.
        s.info.encoder_version[..enc.len()].copy_from_slice(enc);
        s.info.encoder_version[enc.len()] = 0;
        s.info.entries = 0;
    }

    fn write_entries_header(s: &mut HeaderState) {
        debug_assert!(s.header_file.is_none());
        if !s.read_only {
            files::write_at(&s.paths.entries_file, 0, &s.info.encode());
        }
    }

    /// `openAndReadEntry`: finds the entry's index, or creates one when
    /// asked (appending, from the free list, or by evicting the LRU).
    fn open_and_read_entry(
        &self,
        s: &mut HeaderState,
        id: &Uuid,
        entry: &mut Entry,
        create: bool,
    ) -> i32 {
        let mut idx = s.header_id_map.get(id).copied().unwrap_or(-1);
        if idx < 0 {
            if create && !s.read_only {
                if s.info.entries < s.max_entries {
                    idx = s.info.entries as i32;
                    s.info.entries += 1;
                } else if let Some(&first) = s.free_list.iter().next() {
                    s.free_list.remove(&first);
                    idx = first;
                } else {
                    // Look for a still valid entry in the LRU.
                    while let Some(&oldid) = s.lru.iter().next() {
                        s.lru.remove(&oldid);
                        if let Some(&old_idx) = s.header_id_map.get(&oldid) {
                            if old_idx >= 0 {
                                idx = old_idx;
                                self.remove_cached_texture(s, &oldid);
                                break;
                            }
                        }
                    }
                }
                if idx >= 0 {
                    entry.id = *id;
                    entry.image_size = -1; // brand-new entry
                    entry.body_size = 0;
                }
            }
        } else {
            s.lru.remove(id);
            match s.updated_entry_map.get(&idx) {
                Some(e) => *entry = *e,
                None => self.read_entry_from_header_immediately(s, &mut idx, entry),
            }
            if entry.image_size <= entry.body_size {
                ll_warns!(
                    "corrupted entry: {} entry image size: {} entry body size: {}",
                    uuid_to_string(id),
                    entry.image_size,
                    entry.body_size
                );
                let tex_filename = texture_file_name(&s.paths, id);
                Self::remove_entry(s, idx, entry, &tex_filename);
                s.updated_entry_map.remove(&idx);
                idx = -1;
            }
        }
        idx
    }

    fn write_entry_to_header_immediately(
        &self,
        s: &mut HeaderState,
        idx: &mut i32,
        entry: &Entry,
        write_header: bool,
    ) {
        let offset = ENTRIES_INFO_SIZE as i64 + *idx as i64 * ENTRY_RECORD_SIZE as i64;
        if write_header {
            Self::open_header_entries_file(s, false, 0);
            let info = s.info.encode();
            let bytes_written = Self::hf_write(s, &info);
            if bytes_written != ENTRIES_INFO_SIZE as i32 {
                self.clear_corrupted_cache(s);
                *idx = -1;
                return;
            }
            Self::hf_seek_set(s, offset);
        } else {
            Self::open_header_entries_file(s, false, offset);
        }
        let bytes_written = Self::hf_write(s, &entry.encode());
        if bytes_written != ENTRY_RECORD_SIZE as i32 {
            self.clear_corrupted_cache(s);
            *idx = -1;
            return;
        }
        Self::close_header_entries_file(s);
        s.updated_entry_map.remove(idx);
    }

    fn read_entry_from_header_immediately(
        &self,
        s: &mut HeaderState,
        idx: &mut i32,
        entry: &mut Entry,
    ) {
        let offset = ENTRIES_INFO_SIZE as i64 + *idx as i64 * ENTRY_RECORD_SIZE as i64;
        Self::open_header_entries_file(s, true, offset);
        let mut buf = entry.encode();
        let bytes_read = Self::hf_read(s, &mut buf);
        Self::close_header_entries_file(s);
        if bytes_read != ENTRY_RECORD_SIZE as i32 {
            self.clear_corrupted_cache(s);
            *idx = -1;
        } else {
            *entry = Entry::decode(&buf);
        }
    }

    /// `updateEntryTimeStamp`: stamps the entry's time (writing later) once
    /// the table is three quarters full.
    fn update_entry_time_stamp(&self, s: &mut HeaderState, idx: i32, entry: &mut Entry) {
        let limit = match s.max_entries_without_time_stamp {
            Some(v) => v,
            None => {
                let v = (s.max_entries as f32 * 0.75) as u32;
                s.max_entries_without_time_stamp = Some(v);
                v
            }
        };
        if s.info.entries < limit {
            return;
        }
        if idx >= 0 && !s.read_only {
            entry.time = self.now();
            s.updated_entry_map.insert(idx, *entry);
        }
    }

    /// `updateEntry`: records new sizes and writes the entry at once.
    /// Returns true when nothing changed ("already cached").
    fn update_entry(
        &self,
        idx: &mut i32,
        entry: &mut Entry,
        new_image_size: i32,
        new_data_size: i32,
    ) -> bool {
        let new_body_size = (new_data_size - ENTRY_SIZE).max(0);
        if new_image_size == entry.image_size && new_body_size == entry.body_size {
            return true;
        }
        let mut purge = false;
        {
            let mut s = self.lock();
            let mut update_header = false;
            if entry.image_size < 0 {
                s.header_id_map.insert(entry.id, *idx);
                s.textures_size_map.insert(entry.id, new_body_size);
                s.textures_size_total += new_body_size as i64;
                update_header = true;
            } else if entry.body_size != new_body_size {
                s.textures_size_map.insert(entry.id, new_body_size);
                s.textures_size_total -= entry.body_size as i64;
                s.textures_size_total += new_body_size as i64;
            }
            entry.time = self.now();
            entry.image_size = new_image_size;
            entry.body_size = new_body_size;
            self.write_entry_to_header_immediately(&mut s, idx, entry, update_header);
            if s.textures_size_total > s.max_textures_size {
                purge = true;
            }
        }
        if purge {
            self.do_purge.store(true, Ordering::Relaxed);
        }
        false
    }

    /// `openAndReadEntries`: rebuilds the maps from the file; returns the
    /// entry count read (0 on failure, with the maps cleared).
    fn open_and_read_entries(&self, s: &mut HeaderState, entries: &mut Vec<Entry>) -> u32 {
        let num_entries = s.info.entries;
        s.header_id_map.clear();
        s.textures_size_map.clear();
        s.free_list.clear();
        s.textures_size_total = 0;

        if s.updated_entry_map.is_empty() {
            Self::open_header_entries_file(s, true, ENTRIES_INFO_SIZE as i64);
        } else {
            Self::open_header_entries_file(s, false, 0);
            self.updated_header_entries_file(s);
            if !Self::header_file_open(s) {
                return 0;
            }
            Self::hf_seek_set(s, ENTRIES_INFO_SIZE as i64);
        }
        for idx in 0..num_entries {
            let mut buf = [0u8; ENTRY_RECORD_SIZE];
            let bytes_read = Self::hf_read(s, &mut buf);
            if bytes_read < ENTRY_RECORD_SIZE as i32 {
                ll_warns!(
                    "Corrupted header entries, failed at {} / {}",
                    idx,
                    num_entries
                );
                Self::close_header_entries_file(s);
                return 0;
            }
            let entry = Entry::decode(&buf);
            entries.push(entry);
            if entry.image_size > entry.body_size {
                s.header_id_map.insert(entry.id, idx as i32);
                s.textures_size_map.insert(entry.id, entry.body_size);
                s.textures_size_total += entry.body_size as i64;
            } else {
                s.free_list.insert(idx as i32);
            }
        }
        Self::close_header_entries_file(s);
        num_entries
    }

    fn write_entries_and_close(&self, s: &mut HeaderState, entries: &[Entry]) {
        debug_assert_eq!(entries.len() as u32, s.info.entries);
        if !s.read_only {
            Self::open_header_entries_file(s, false, ENTRIES_INFO_SIZE as i64);
            for e in entries {
                let bytes_written = Self::hf_write(s, &e.encode());
                if bytes_written != ENTRY_RECORD_SIZE as i32 {
                    self.clear_corrupted_cache(s);
                    return;
                }
            }
            Self::close_header_entries_file(s);
        }
    }

    /// `writeUpdatedEntries`: flushes the time-stamped entries.
    pub fn write_updated_entries(&self) {
        let mut s = self.lock();
        if !s.read_only && !s.updated_entry_map.is_empty() {
            Self::open_header_entries_file(&mut s, false, 0);
            self.updated_header_entries_file(&mut s);
            Self::close_header_entries_file(&mut s);
        }
    }

    /// `updatedHeaderEntriesFile`: with the file open, writes the header
    /// and every updated entry by seeking between them.
    fn updated_header_entries_file(&self, s: &mut HeaderState) {
        if !s.read_only && !s.updated_entry_map.is_empty() && Self::header_file_open(s) {
            Self::hf_seek_set(s, 0);
            let info = s.info.encode();
            let bytes_written = Self::hf_write(s, &info);
            if bytes_written != ENTRIES_INFO_SIZE as i32 {
                self.clear_corrupted_cache(s);
                return;
            }
            let entry_size = ENTRY_RECORD_SIZE as i64;
            let mut prev_idx: i64 = -1;
            let updated: Vec<(i32, Entry)> =
                s.updated_entry_map.iter().map(|(k, v)| (*k, *v)).collect();
            for (idx, entry) in updated {
                let delta_idx = idx as i64 - prev_idx - 1;
                prev_idx = idx as i64;
                if delta_idx != 0 {
                    Self::hf_seek_cur(s, delta_idx * entry_size);
                }
                let bytes_written = Self::hf_write(s, &entry.encode());
                if bytes_written != entry_size as i32 {
                    self.clear_corrupted_cache(s);
                    return;
                }
            }
            s.updated_entry_map.clear();
        }
    }

    /// `readHeaderCache`: reads the index, purging on a version or engine
    /// mismatch, rebuilding the LRU list and pruning when the table shrank.
    pub fn read_header_cache(&self) {
        let mut s = self.lock();
        s.lru.clear();
        self.read_entries_header(&mut s);

        let encoder_matches = s.info.encoder_str() == self.config.encoder_version.as_bytes();
        if s.info.version != self.config.version
            || s.info.address_size != self.config.address_size
            || !encoder_matches
        {
            if !s.read_only {
                ll_infos!("Texture Cache version mismatch, Purging.");
                self.purge_all_textures(&mut s, false);
            }
        } else {
            let mut entries: Vec<Entry> = Vec::new();
            let num_entries = self.open_and_read_entries(&mut s, &mut entries);
            if num_entries > 0 {
                let mut empty_entries: u32 = 0;
                let mut lru: BTreeSet<(u32, i32)> = BTreeSet::new();
                let mut purge_list: BTreeSet<u32> = BTreeSet::new();
                for (i, entry) in entries.iter().enumerate() {
                    if entry.image_size <= 0 {
                        empty_entries += 1;
                    } else {
                        lru.insert((entry.time, i as i32));
                        if entry.body_size > 0 && entry.body_size > entry.image_size {
                            ll_warns!(
                                "Bad entry: {}: {}: BodySize: {}",
                                i,
                                uuid_to_string(&entry.id),
                                entry.body_size
                            );
                            purge_list.insert(i as u32);
                        }
                    }
                }
                if num_entries - empty_entries > s.max_entries {
                    let entries_to_purge = (num_entries - empty_entries) - s.max_entries;
                    ll_infos!(
                        "Texture Cache Entries: {} Max: {} Empty: {} Purging: {}",
                        num_entries,
                        s.max_entries,
                        empty_entries,
                        entries_to_purge
                    );
                    let mut iter = lru.iter();
                    while (purge_list.len() as u32) < entries_to_purge {
                        match iter.next() {
                            Some(&(_, idx)) => {
                                purge_list.insert(idx as u32);
                            }
                            None => break,
                        }
                    }
                }
                {
                    let mut lru_entries = (s.max_entries as f32 * LRU_SIZE) as i32;
                    for &(_, idx) in lru.iter() {
                        s.lru.insert(entries[idx as usize].id);
                        lru_entries -= 1;
                        if lru_entries <= 0 {
                            break;
                        }
                    }
                }
                if !purge_list.is_empty() {
                    let timer = Instant::now();
                    for &i in purge_list.iter() {
                        let tex_filename = texture_file_name(&s.paths, &entries[i as usize].id);
                        let mut e = entries[i as usize];
                        Self::remove_entry(&mut s, i as i32, &mut e, &tex_filename);
                        entries[i as usize] = e;
                        if timer.elapsed().as_secs_f32() > PRUNING_MAX_TIME {
                            break;
                        }
                    }
                    self.write_entries_and_close(&mut s, &entries);
                }
            }
        }
    }

    /// `clearCorruptedCache`: closes the header file, wipes the cache and
    /// recreates the directory tree.
    fn clear_corrupted_cache(&self, s: &mut HeaderState) {
        ll_warns!("the texture cache is corrupted, need to be cleared.");
        Self::close_header_entries_file(s);
        self.purge_all_textures(s, false);
        if !s.read_only {
            files::mkdir(&s.paths.textures_dir);
            make_subdirs(&s.paths);
        }
    }

    /// `purgeAllTextures`.
    fn purge_all_textures(&self, s: &mut HeaderState, purge_directories: bool) {
        if !s.read_only {
            // Windows can be slow deleting a huge texture cache: on a full
            // purge the directory is renamed and left for a background thread
            // of the viewer to delete.
            #[cfg(windows)]
            let delete_contents = if purge_directories {
                let new_name = format!(
                    "{}{}{}.old_texturecache",
                    s.paths.parent_dir,
                    s.paths.delim,
                    uuid_to_string(&pseudo_random_uuid())
                );
                let mut old_purge = true;
                if files::is_dir(&s.paths.textures_dir) {
                    old_purge = !files::rename(&s.paths.textures_dir, &new_name);
                }
                if old_purge {
                    ll_warns!("Rename of {} to {} failed", s.paths.textures_dir, new_name);
                }
                old_purge
            } else {
                true
            };
            #[cfg(not(windows))]
            let delete_contents = true;
            if delete_contents {
                for c in SUBDIRS.iter() {
                    let dirname =
                        format!("{}{}{}", s.paths.textures_dir, s.paths.delim, *c as char);
                    ll_infos!("Deleting files in directory: {}", dirname);
                    if purge_directories {
                        files::delete_dir_and_contents(&dirname);
                    } else {
                        files::delete_files_in_dir(&dirname);
                    }
                }
                if files::is_dir(&s.paths.textures_dir) {
                    files::delete_files_in_dir(&s.paths.textures_dir); // headers, fast cache
                    if purge_directories {
                        files::rmdir(&s.paths.textures_dir);
                    }
                }
            }
        }
        s.header_id_map.clear();
        s.textures_size_map.clear();
        s.textures_size_total = 0;
        s.free_list.clear();
        s.updated_entry_map.clear();
        self.set_entries_header(s);
        Self::write_entries_header(s);
        ll_infos!("The entire texture cache is cleared.");
    }

    /// `purgeTexturesLazy`: forms the purge list on the first call and
    /// removes entries from it for at most `time_limit_sec` on later calls.
    pub fn purge_textures_lazy(&self, time_limit_sec: f32) {
        let mut s = self.lock();
        if s.read_only {
            return;
        }
        if s.purge_entry_list.is_empty() {
            let mut entries: Vec<Entry> = Vec::new();
            let num_entries = self.open_and_read_entries(&mut s, &mut entries);
            if num_entries == 0 {
                return;
            }
            let mut time_idx_set: BTreeSet<(u32, i32)> = BTreeSet::new();
            for (id, &size) in s.textures_size_map.iter() {
                if size > 0 {
                    match s.header_id_map.get(id) {
                        Some(&idx) => {
                            time_idx_set.insert((entries[idx as usize].time, idx));
                        }
                        None => panic!("mTexturesSizeMap / mHeaderIDMap corrupted."),
                    }
                }
            }
            let mut cache_size = s.textures_size_total;
            let purged_cache_size = purged_size(cache_size, s.max_textures_size);
            for &(_, idx) in time_idx_set.iter() {
                if cache_size >= purged_cache_size {
                    cache_size -= entries[idx as usize].body_size as i64;
                    s.purge_entry_list.push((idx, entries[idx as usize]));
                } else {
                    break;
                }
            }
            ll_debugs!("Formed Purge list of {} entries", s.purge_entry_list.len());
        } else {
            let timer = Instant::now();
            while !s.purge_entry_list.is_empty() && timer.elapsed().as_secs_f32() < time_limit_sec {
                let (mut idx, mut entry) = s.purge_entry_list.pop().unwrap();
                if s.header_id_map.get(&entry.id) == Some(&idx) {
                    let tex_filename = texture_file_name(&s.paths, &entry.id);
                    Self::remove_entry(&mut s, idx, &mut entry, &tex_filename);
                    self.write_entry_to_header_immediately(&mut s, &mut idx, &entry, false);
                }
            }
        }
    }

    /// The `writeToCache` prelude: one slice of the lazy purge while one is
    /// pending. Returns whether more remains.
    pub fn lazy_purge_tick(&self, time_limit_sec: f32) -> bool {
        if self.do_purge.load(Ordering::Relaxed) {
            self.purge_textures_lazy(time_limit_sec);
            let more = !self.lock().purge_entry_list.is_empty();
            self.do_purge.store(more, Ordering::Relaxed);
            more
        } else {
            false
        }
    }

    /// `purgeTextures(validate)`: trims the bodies to 80 % of the limit by
    /// age and, when `validate_idx` is given, checks the body files of the
    /// entries whose UUID starts with that byte.
    pub fn purge_textures(&self, validate_idx: Option<u32>) {
        let mut s = self.lock();
        if s.read_only {
            return;
        }
        ll_infos!("TEXTURE CACHE: Purging.");
        let mut entries: Vec<Entry> = Vec::new();
        let num_entries = self.open_and_read_entries(&mut s, &mut entries);
        if num_entries == 0 {
            return;
        }
        let mut time_idx_set: BTreeSet<(u32, i32)> = BTreeSet::new();
        for (id, &size) in s.textures_size_map.iter() {
            if size > 0 {
                match s.header_id_map.get(id) {
                    Some(&idx) => {
                        time_idx_set.insert((entries[idx as usize].time, idx));
                    }
                    None => panic!("mTexturesSizeMap / mHeaderIDMap corrupted."),
                }
            }
        }
        if let Some(v) = validate_idx {
            ll_debugs!("TEXTURE CACHE: Validating: {}", v);
        }
        let mut cache_size = s.textures_size_total;
        let purged_cache_size = purged_size(cache_size, s.max_textures_size);
        let mut purge_count = 0;
        for &(_, idx) in time_idx_set.iter() {
            let mut purge_entry = false;
            if cache_size >= purged_cache_size {
                purge_entry = true;
            } else if let Some(validate) = validate_idx {
                let uuididx = entries[idx as usize].id[0] as u32;
                if uuididx == validate {
                    let filename = texture_file_name(&s.paths, &entries[idx as usize].id);
                    ll_debugs!(
                        "Validating: {}Size: {}",
                        filename,
                        entries[idx as usize].body_size
                    );
                    let bodysize = files::file_size(&filename);
                    if bodysize != entries[idx as usize].body_size {
                        ll_warns!(
                            "TEXTURE CACHE BODY HAS BAD SIZE: {} != {}{}",
                            bodysize,
                            entries[idx as usize].body_size,
                            filename
                        );
                        purge_entry = true;
                    }
                }
            } else {
                break;
            }
            if purge_entry {
                purge_count += 1;
                let filename = texture_file_name(&s.paths, &entries[idx as usize].id);
                ll_debugs!("PURGING: {}", filename);
                cache_size -= entries[idx as usize].body_size as i64;
                let mut e = entries[idx as usize];
                Self::remove_entry(&mut s, idx, &mut e, &filename);
                entries[idx as usize] = e;
            }
        }
        ll_debugs!("TEXTURE CACHE: Writing Entries: {}", num_entries);
        self.write_entries_and_close(&mut s, &entries);
        ll_infos!(
            "TEXTURE CACHE: PURGED: {} ENTRIES: {} CACHE SIZE: {} MB",
            purge_count,
            num_entries,
            s.textures_size_total / (1024 * 1024)
        );
    }

    /// `getHeaderCacheEntry`: the entry's index with its time stamped.
    fn get_header_cache_entry(&self, id: &Uuid, entry: &mut Entry) -> i32 {
        let mut s = self.lock();
        let idx = self.open_and_read_entry(&mut s, id, entry, false);
        if idx >= 0 {
            self.update_entry_time_stamp(&mut s, idx, entry);
        }
        idx
    }

    /// `setHeaderCacheEntry`: creates the entry (retrying once after a
    /// re-read of the index) and records its sizes.
    fn set_header_cache_entry(
        &self,
        id: &Uuid,
        entry: &mut Entry,
        imagesize: i32,
        datasize: i32,
    ) -> i32 {
        let mut idx = {
            let mut s = self.lock();
            self.open_and_read_entry(&mut s, id, entry, true)
        };
        if idx < 0 {
            self.read_header_cache(); // refresh the LRU
            let mut s = self.lock();
            idx = self.open_and_read_entry(&mut s, id, entry, true);
        }
        if idx >= 0 {
            self.update_entry(&mut idx, entry, imagesize, datasize);
        } else {
            ll_warns!(
                "Failed to set cache entry for image: {}",
                uuid_to_string(id)
            );
            let mut s = self.lock();
            s.read_only = true;
            self.clear_corrupted_cache(&mut s);
        }
        idx
    }

    // ---- the worker operations -----------------------------------------------

    /// `LLTextureCacheRemoteWorker::doRead`: `size` bytes from `offset` of
    /// the texture's data (header record, then body).
    pub fn read(&self, id: &Uuid, offset: i32, size: i32) -> ReadOutcome {
        let mut data = vec![0u8; size.max(0) as usize];
        let (data_size, image_size) = self.read_into(id, offset, size, &mut data);
        data.truncate(data_size.max(0) as usize);
        ReadOutcome {
            data_size,
            data,
            image_size,
        }
    }

    /// `read` into the caller's buffer of at least `size` bytes (the C++
    /// worker allocated `mDataSize` up front; the host's buffer takes its
    /// place). Returns `(data_size, image_size)` as [`ReadOutcome`] carries
    /// them.
    pub fn read_into(&self, id: &Uuid, offset: i32, size: i32, buf: &mut [u8]) -> (i32, i32) {
        let mut data_size = size;
        let mut entry = Entry::default();
        let idx = self.get_header_cache_entry(id, &mut entry);
        if idx < 0 {
            return (0, 0);
        }
        let image_size = entry.image_size;
        if buf.len() < size.max(0) as usize {
            ll_warns!(
                "LLTextureCacheWorker: {} failed to allocate memory for reading: {}",
                uuid_to_string(id),
                data_size
            );
            return (-1, image_size);
        }
        let paths = self.paths();
        let mut header_bytes = 0i32;
        if offset < ENTRY_SIZE {
            let file_offset = idx as i64 * ENTRY_SIZE as i64 + offset as i64;
            let mut rsize = ENTRY_SIZE - offset;
            rsize = rsize.min(data_size);
            if rsize < 0 {
                ll_warns!(
                    "LLTextureCacheWorker: {} failed to allocate memory for reading: {}",
                    uuid_to_string(id),
                    data_size
                );
                return (-1, image_size);
            }
            let bytes_read =
                files::read_at(&paths.data_file, file_offset, &mut buf[..rsize as usize]);
            if bytes_read != rsize {
                ll_warns!(
                    "LLTextureCacheWorker: {} incorrect number of bytes read from header: {} / {}",
                    uuid_to_string(id),
                    bytes_read,
                    rsize
                );
                return (-1, image_size);
            }
            if data_size <= bytes_read {
                return (data_size, image_size);
            }
            header_bytes = bytes_read;
        }
        let filename = texture_file_name(&paths, id);
        let filesize = files::file_size(&filename);
        if filesize != 0 && (filesize + ENTRY_SIZE) > offset {
            let max_datasize = ENTRY_SIZE + filesize - offset;
            data_size = max_datasize.min(data_size);
            let (data_offset, file_offset, file_size) = if offset < ENTRY_SIZE {
                let data_offset = ENTRY_SIZE - offset;
                (data_offset, 0, data_size - data_offset)
            } else {
                (0, offset - ENTRY_SIZE, data_size)
            };
            debug_assert!(data_offset <= header_bytes.max(data_offset));
            let bytes_read = files::read_at(
                &filename,
                file_offset as i64,
                &mut buf[data_offset as usize..data_size as usize],
            );
            if bytes_read != file_size {
                ll_warns!(
                    "LLTextureCacheWorker: {} incorrect number of bytes read from body: {} / {}",
                    uuid_to_string(id),
                    bytes_read,
                    file_size
                );
                return (-1, image_size);
            }
            (data_size, image_size)
        } else {
            data_size = (ENTRY_SIZE - offset).max(0);
            ll_debugs!("No body file for: {}", filename);
            (data_size, image_size)
        }
    }

    /// `LLTextureCacheRemoteWorker::doWrite`: writes `data` (the first
    /// `image_size` bytes of a texture of which `data.len()` are known)
    /// with the fast-cache record for a new entry. Returns false on failure
    /// (the host then removes the entry, as `endWork` did).
    pub fn write(
        &self,
        id: &Uuid,
        data: &[u8],
        image_size: i32,
        discard_level: i32,
        fast: &FastEntry,
    ) -> bool {
        let data_size = data.len() as i32;
        if data_size <= 0 || image_size < data_size || discard_level < 0 {
            ll_warns!(
                "INIT state check failed for image: {} Size: {} DataSize: {} Discard:{}",
                uuid_to_string(id),
                image_size,
                data_size,
                discard_level
            );
            return false;
        }
        let mut entry = Entry::default();
        let mut already_cached = false;
        let mut idx = self.get_header_cache_entry(id, &mut entry);
        if idx < 0 {
            idx = self.set_header_cache_entry(id, &mut entry, image_size, data_size);
            if idx >= 0 && !self.write_to_fast_cache(idx, fast) {
                ll_warns!("writeToFastCache failed");
                return false;
            }
        } else {
            already_cached = self.update_entry(&mut idx, &mut entry, image_size, data_size);
        }
        if idx < 0 {
            ll_warns!(
                "LLTextureCacheWorker: {} Unable to create header entry for writing!",
                uuid_to_string(id)
            );
            return false;
        }
        if already_cached && data_size <= ENTRY_SIZE {
            return true;
        }
        let paths = self.paths();
        if !already_cached {
            let offset = idx as i64 * ENTRY_SIZE as i64;
            let bytes_written = if data_size < ENTRY_SIZE {
                let mut pad = vec![0u8; ENTRY_SIZE as usize];
                pad[..data.len()].copy_from_slice(data);
                files::write_at(&paths.data_file, offset, &pad)
            } else {
                files::write_at(&paths.data_file, offset, &data[..ENTRY_SIZE as usize])
            };
            if bytes_written <= 0 {
                ll_warns!(
                    "LLTextureCacheWorker: {} Unable to write header entry!",
                    uuid_to_string(id)
                );
                return false;
            }
            if data_size <= bytes_written {
                return true;
            }
        }
        if data_size <= ENTRY_SIZE {
            ll_warns!("mDataSize check failed");
            return false;
        }
        let file_size = data_size - ENTRY_SIZE;
        let filename = texture_file_name(&paths, id);
        let bytes_written = files::write_at(&filename, 0, &data[ENTRY_SIZE as usize..]);
        if bytes_written <= 0 {
            ll_warns!(
                "LLTextureCacheWorker: {} incorrect number of bytes written to body: {} / {}",
                uuid_to_string(id),
                bytes_written,
                file_size
            );
            return false;
        }
        true
    }

    /// `removeFromCache`.
    pub fn remove(&self, id: &Uuid) -> bool {
        let mut ret = false;
        let mut s = self.lock();
        if !s.read_only {
            let mut entry = Entry::default();
            let mut idx = self.open_and_read_entry(&mut s, id, &mut entry, false);
            let tex_filename = texture_file_name(&s.paths, id);
            Self::remove_entry(&mut s, idx, &mut entry, &tex_filename);
            if idx >= 0 {
                self.write_entry_to_header_immediately(&mut s, &mut idx, &entry, false);
                ret = true;
            }
        }
        ret
    }

    /// `removeCachedTexture` (under the header lock).
    fn remove_cached_texture(&self, s: &mut HeaderState, id: &Uuid) {
        if let Some(size) = s.textures_size_map.remove(id) {
            s.textures_size_total -= size as i64;
        }
        s.header_id_map.remove(id);
        files::remove_file(&texture_file_name(&s.paths, id));
    }

    /// `removeEntry` (under the header lock).
    fn remove_entry(s: &mut HeaderState, idx: i32, entry: &mut Entry, filename: &str) {
        let mut file_maybe_exists = true;
        if idx >= 0 {
            if entry.body_size == 0 {
                if files::exists(filename) {
                    ll_warns!(
                        "Entry has body size of zero but file {} exists. Deleting this file, too.",
                        filename
                    );
                } else {
                    file_maybe_exists = false;
                }
            }
            s.textures_size_total -= entry.body_size as i64;
            entry.image_size = -1;
            entry.body_size = 0;
            s.header_id_map.remove(&entry.id);
            s.textures_size_map.remove(&entry.id);
            s.free_list.insert(idx);
        }
        if file_maybe_exists {
            files::remove_file(filename);
        }
    }

    // ---- the fast cache --------------------------------------------------------

    fn open_fast_cache(&self, first_time: bool) {
        let paths = self.paths();
        let mut f = self.fast.lock().unwrap_or_else(|e| e.into_inner());
        Self::open_fast_cache_locked(&mut f, &paths, first_time);
    }

    fn open_fast_cache_locked(f: &mut FastState, paths: &Paths, first_time: bool) {
        if f.file.is_none() {
            let file = if first_time {
                f.first_time_done = true;
                if files::exists(&paths.fast_file) {
                    files::OpenFile::open(&paths.fast_file, false)
                } else {
                    std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .truncate(false)
                        .open(&paths.fast_file)
                        .ok()
                        .and_then(|_| files::OpenFile::open(&paths.fast_file, false))
                }
            } else {
                files::OpenFile::open(&paths.fast_file, false)
            };
            f.file = Some(file);
            f.opened = Instant::now();
        }
    }

    fn close_fast_cache_locked(f: &mut FastState, forced: bool) {
        if f.file.is_none() {
            return;
        }
        if !forced && f.opened.elapsed().as_secs_f32() < FAST_CACHE_TIMEOUT_SECS {
            return;
        }
        f.file = None;
    }

    /// `readFromFastCache`: the small raw image of a cached texture. Waits
    /// for the fast cache when another thread is writing it.
    pub fn read_fast(&self, id: &Uuid) -> Option<FastRecord> {
        let (offset, paths) = self.fast_offset(id)?;
        let mut f = self.fast.lock().unwrap_or_else(|e| e.into_inner());
        Self::read_fast_locked(&mut f, offset, &paths)
    }

    /// `readFromFastCache` without waiting for the fast cache: upstream
    /// #5356 lets `LLViewerTextureList::updateImagesLoadingFastCache`
    /// try-lock the fast cache mutex and skip the frame while a worker
    /// writes it, instead of stalling the main thread. [`FastRead::Busy`] is
    /// that case. The header lookup before it still waits, as in the C++,
    /// which locks `mHeaderIDMapMutex` normally.
    pub fn try_read_fast(&self, id: &Uuid) -> FastRead {
        let Some((offset, paths)) = self.fast_offset(id) else {
            return FastRead::Miss;
        };
        let mut f = match self.fast.try_lock() {
            Ok(f) => f,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => return FastRead::Busy,
        };
        match Self::read_fast_locked(&mut f, offset, &paths) {
            Some(r) => FastRead::Hit(r),
            None => FastRead::Miss,
        }
    }

    fn fast_offset(&self, id: &Uuid) -> Option<(i64, Paths)> {
        let s = self.lock();
        let idx = *s.header_id_map.get(id)?;
        Some((idx as i64 * FAST_ENTRY_SIZE as i64, s.paths.clone()))
    }

    fn read_fast_locked(f: &mut FastState, offset: i64, paths: &Paths) -> Option<FastRecord> {
        Self::open_fast_cache_locked(f, paths, false);
        let file = f.file.as_mut().and_then(|x| x.as_mut());
        let mut head = [0u8; FAST_ENTRY_OVERHEAD as usize];
        let read_ok = match file {
            Some(file) => file.seek_set(offset) && file.read(&mut head) == FAST_ENTRY_OVERHEAD,
            None => false,
        };
        if !read_ok {
            Self::close_fast_cache_locked(f, false);
            return None;
        }
        let w = i32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        let h = i32::from_le_bytes([head[4], head[5], head[6], head[7]]);
        let c = i32::from_le_bytes([head[8], head[9], head[10], head[11]]);
        let level = i32::from_le_bytes([head[12], head[13], head[14], head[15]]);
        let image_size = w.wrapping_mul(h).wrapping_mul(c);
        if image_size <= 0 || image_size > FAST_DATA_SIZE || level < 0 {
            Self::close_fast_cache_locked(f, false);
            return None;
        }
        let mut data = vec![0u8; image_size as usize];
        let file = f.file.as_mut().and_then(|x| x.as_mut());
        let ok = match file {
            Some(file) => file.read(&mut data) == image_size,
            None => false,
        };
        if !ok {
            Self::close_fast_cache_locked(f, false);
            return None;
        }
        Self::close_fast_cache_locked(f, false);
        Some(FastRecord {
            width: w,
            height: h,
            components: c,
            discard_level: level,
            data,
        })
    }

    /// `writeToFastCache` after the host scaled the raw image: writes the
    /// record at the entry's index.
    fn write_to_fast_cache(&self, idx: i32, fast: &FastEntry) -> bool {
        let mut record = vec![0u8; FAST_ENTRY_SIZE as usize];
        record[0..4].copy_from_slice(&fast.width.to_le_bytes());
        record[4..8].copy_from_slice(&fast.height.to_le_bytes());
        record[8..12].copy_from_slice(&fast.components.to_le_bytes());
        record[12..16].copy_from_slice(&fast.discard_level.to_le_bytes());
        let copy_size = fast.width * fast.height * fast.components;
        if copy_size > 0 {
            let copy_size = (copy_size.min(FAST_DATA_SIZE) as usize).min(fast.data.len());
            record[FAST_ENTRY_OVERHEAD as usize..FAST_ENTRY_OVERHEAD as usize + copy_size]
                .copy_from_slice(&fast.data[..copy_size]);
        }
        let offset = idx as i64 * FAST_ENTRY_SIZE as i64;
        let paths = self.paths();
        let mut f = self.fast.lock().unwrap_or_else(|e| e.into_inner());
        Self::open_fast_cache_locked(&mut f, &paths, false);
        if let Some(file) = f.file.as_mut().and_then(|x| x.as_mut()) {
            // A failed write is let go quietly: another viewer may have
            // removed the fast cache file while clearing the cache.
            if file.seek_set(offset) {
                file.write(&record);
            }
        }
        Self::close_fast_cache_locked(&mut f, true);
        true
    }

    /// The destructor's flush (`writeUpdatedEntries`) and the fast cache
    /// handle release.
    pub fn shutdown(&self) {
        self.write_updated_entries();
        let mut f = self.fast.lock().unwrap_or_else(|e| e.into_inner());
        f.file = None;
    }
}

fn make_subdirs(paths: &Paths) {
    for c in SUBDIRS.iter() {
        files::mkdir(&format!(
            "{}{}{}",
            paths.textures_dir, paths.delim, *c as char
        ));
    }
}

/// `(max(cache_size, limit) * (S64)((1 - PURGE_AMOUNT) * 100)) / 100`.
fn purged_size(cache_size: i64, max_textures_size: i64) -> i64 {
    (cache_size.max(max_textures_size) * ((1.0 - PURGE_AMOUNT) * 100.0) as i64) / 100
}

#[cfg(windows)]
fn pseudo_random_uuid() -> Uuid {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = [0u8; 16];
    for chunk in out.chunks_mut(8) {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(Instant::now().elapsed().as_nanos());
        let v = h.finish().to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
    out[6] = (out[6] & 0x0f) | 0x40;
    out[8] = (out[8] & 0x3f) | 0x80;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_layouts() {
        let info = EntriesInfo {
            version: 1.71,
            address_size: 64,
            encoder_version: *b"Rust j2c\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
            entries: 7,
        };
        let b = info.encode();
        assert_eq!(b.len(), 44);
        assert_eq!(EntriesInfo::decode(&b), info);
        assert_eq!(info.encoder_str(), b"Rust j2c");
        let e = Entry {
            id: [1; 16],
            image_size: 1234,
            body_size: 634,
            time: 99,
        };
        let b = e.encode();
        assert_eq!(b.len(), 28);
        assert_eq!(Entry::decode(&b), e);
    }

    #[test]
    fn fast_level() {
        assert_eq!(fast_cache_level(16, 16, 4), 0);
        assert_eq!(fast_cache_level(32, 32, 4), 1);
        assert_eq!(fast_cache_level(1024, 1024, 3), 6);
        assert_eq!(fast_cache_level(64, 16, 1), 0);
    }

    #[test]
    fn purge_target() {
        assert_eq!(purged_size(1000, 500), 800);
        assert_eq!(purged_size(100, 500), 400);
    }

    #[test]
    fn paths_layout() {
        let p = Paths::for_location("C:\\cache", "\\");
        assert_eq!(p.textures_dir, "C:\\cache\\texturecache");
        assert_eq!(p.entries_file, "C:\\cache\\texturecache\\texture.entries");
        assert_eq!(p.legacy_textures_dir, "C:\\cache\\textures");
        let id = crate::uuid_from_string("a1234567-89ab-cdef-1032-547698badcfe").unwrap();
        assert_eq!(
            texture_file_name(&p, &id),
            "C:\\cache\\texturecache\\a\\a1234567-89ab-cdef-1032-547698badcfe.texture"
        );
    }

    fn temp_dir(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("vks_texcache_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    }

    fn fixed_clock() -> u32 {
        1_700_000_000
    }

    #[test]
    fn write_read_round_trip() {
        let dir = temp_dir("rt");
        let delim = std::path::MAIN_SEPARATOR.to_string();
        let paths = Paths::for_location(&dir, &delim);
        let mut cfg = Config::new("test-engine", 64);
        cfg.clock = fixed_clock;
        let cache = TextureCache::new(cfg);
        cache.set_read_only(false);
        cache.init_cache(&paths, 50 * 1024 * 1024, false, Some(0));
        let id = [7u8; 16];
        let data: Vec<u8> = (0..2000u32).map(|i| (i * 7 % 251) as u8).collect();
        let fast = FastEntry {
            width: 16,
            height: 16,
            components: 3,
            discard_level: 3,
            data: vec![9; 768],
        };
        assert!(cache.write(&id, &data, 5000, 3, &fast));
        assert!(cache.is_in_cache(&id));
        let r = cache.read(&id, 0, 4096);
        assert_eq!(r.data_size, 2000);
        assert_eq!(r.image_size, 5000);
        assert_eq!(r.data, data);
        let r = cache.read(&id, 0, 300);
        assert_eq!(r.data_size, 300);
        assert_eq!(&r.data[..], &data[..300]);
        let fr = cache.read_fast(&id).unwrap();
        assert_eq!(
            (fr.width, fr.height, fr.components, fr.discard_level),
            (16, 16, 3, 3)
        );
        assert_eq!(fr.data, vec![9; 768]);
        // The entries file carries one entry with our sizes.
        let bytes = std::fs::read(&paths.entries_file).unwrap();
        assert_eq!(bytes.len(), 44 + 28);
        let e = Entry::decode(bytes[44..72].try_into().unwrap());
        assert_eq!(e.id, id);
        assert_eq!(e.image_size, 5000);
        assert_eq!(e.body_size, 1400);
        assert_eq!(e.time, 1_700_000_000);
        assert!(cache.remove(&id));
        assert!(!cache.is_in_cache(&id));
        assert_eq!(cache.read(&id, 0, 4096).data_size, 0);
        cache.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Upstream #5356: while a worker holds the fast cache, the main
    /// thread's non-blocking read answers Busy at once instead of waiting;
    /// once released, the same read finds the record.
    #[test]
    fn try_read_fast_is_busy_while_the_fast_cache_is_held() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        let dir = temp_dir("trylock");
        let delim = std::path::MAIN_SEPARATOR.to_string();
        let paths = Paths::for_location(&dir, &delim);
        let mut cfg = Config::new("test-engine", 64);
        cfg.clock = fixed_clock;
        let cache = TextureCache::new(cfg);
        cache.set_read_only(false);
        cache.init_cache(&paths, 50 * 1024 * 1024, false, Some(0));
        let id = [5u8; 16];
        let fast = FastEntry {
            width: 8,
            height: 8,
            components: 3,
            discard_level: 2,
            data: vec![4; 192],
        };
        assert!(cache.write(&id, &[1u8; 600], 1000, 2, &fast));
        assert_eq!(cache.try_read_fast(&[6u8; 16]), FastRead::Miss);

        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        std::thread::scope(|scope| {
            let cache = &cache;
            scope.spawn(move || {
                let _guard = cache.fast.lock().unwrap();
                held_tx.send(()).unwrap();
                // Bounded, so a read that blocks (the regression) still
                // returns and fails below instead of hanging the test.
                let _ = release_rx.recv_timeout(Duration::from_secs(10));
            });
            held_rx.recv().unwrap();
            let started = Instant::now();
            let read = cache.try_read_fast(&id);
            let waited = started.elapsed();
            let _ = release_tx.send(());
            assert_eq!(read, FastRead::Busy);
            assert!(
                waited < Duration::from_secs(1),
                "try_read_fast waited {waited:?}"
            );
        });
        match cache.try_read_fast(&id) {
            FastRead::Hit(r) => {
                assert_eq!(
                    (r.width, r.height, r.components, r.discard_level),
                    (8, 8, 3, 2)
                );
                assert_eq!(r.data, vec![4; 192]);
            }
            other => panic!("expected the record, got {other:?}"),
        }
        assert_eq!(cache.read_fast(&id).map(|r| r.data.len()), Some(192));
        cache.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_mismatch_purges() {
        let dir = temp_dir("mismatch");
        let delim = std::path::MAIN_SEPARATOR.to_string();
        let paths = Paths::for_location(&dir, &delim);
        let mut cfg = Config::new("engine-a", 64);
        cfg.clock = fixed_clock;
        {
            let cache = TextureCache::new(cfg.clone());
            cache.set_read_only(false);
            cache.init_cache(&paths, 50 * 1024 * 1024, false, None);
            let fast = FastEntry {
                width: 8,
                height: 8,
                components: 3,
                discard_level: 2,
                data: vec![1; 192],
            };
            assert!(cache.write(&[1; 16], &[5u8; 700], 700, 2, &fast));
            cache.shutdown();
        }
        cfg.encoder_version = "engine-b".into();
        let cache = TextureCache::new(cfg);
        cache.set_read_only(false);
        cache.init_cache(&paths, 50 * 1024 * 1024, false, None);
        assert_eq!(cache.entries(), 0);
        assert!(!cache.is_in_cache(&[1; 16]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
