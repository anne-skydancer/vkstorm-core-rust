//! `LLDiskCache` (the asset disk cache: file naming, the purge by age with
//! high and low water marks, the clear and the scans) and the file I/O of
//! `LLFileSystem` and `LLLFSThread` ([`file`]).

pub mod file;

use std::fs;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::{ll_debugs, ll_infos, ll_warns, uuid_to_string, Uuid};

/// `CACHE_FILENAME_PREFIX`.
pub const CACHE_FILENAME_PREFIX: &str = "sl_cache";

/// `metaDataToFilepath`: `<dir>/<first hex digit>/sl_cache_<uuid>_0.asset`
/// (the asset type is not part of the name).
pub fn metadata_to_filepath(cache_dir: &str, delim: &str, id: &Uuid) -> String {
    let s = uuid_to_string(id);
    let first = &s[..1];
    format!("{cache_dir}{delim}{first}{delim}{CACHE_FILENAME_PREFIX}_{s}_0.asset")
}

/// The `sl_cache_<uuid>` part of a cache file name, as the purge reads it
/// back (`getBaseFileName(path, true).substr(9, 36)`).
pub fn uuid_text_of_cache_file(path: &str) -> String {
    let base = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let start = CACHE_FILENAME_PREFIX.len() + 1;
    if base.len() <= start {
        return String::new();
    }
    let end = (start + 36).min(base.len());
    base[start..end].to_string()
}

fn mtime_secs(m: &fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs() as i64).or_else(|| Some(-(UNIX_EPOCH.duration_since(t).ok()?.as_secs() as i64))))
        .unwrap_or(0)
}

/// Walks `dir` recursively calling `f` with each regular file whose path
/// contains `needle`, its size and its modification time.
fn walk(dir: &Path, needle: &str, f: &mut dyn FnMut(String, u64, i64)) {
    walk_while(dir, needle, &mut || true, f);
}

/// [`walk`] that stops, returning false, as soon as `running` says no
/// (checked before each directory entry, as upstream d527adf68c checks
/// `LLApp::isRunning()` in `purge`'s directory loop).
fn walk_while(dir: &Path, needle: &str, running: &mut dyn FnMut() -> bool, f: &mut dyn FnMut(String, u64, i64)) -> bool {
    let Ok(rd) = fs::read_dir(dir) else { return true };
    for e in rd.flatten() {
        if !running() {
            return false;
        }
        let path = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if !walk_while(&path, needle, running, f) {
                return false;
            }
        } else if ft.is_file() {
            let s = path.to_string_lossy().into_owned();
            if s.contains(needle) {
                if let Ok(m) = fs::metadata(&path) {
                    f(s, m.len(), mtime_secs(&m));
                }
            }
        }
    }
    true
}

/// What one purge did (`LLDiskCache::purge`'s accounting).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PurgeReport {
    pub files: usize,
    pub size_before: u64,
    pub deleted_size: u64,
    pub deleted: u32,
    pub skipped: u32,
    pub kept: usize,
    /// Whether the high water mark was reached (else nothing was done).
    pub purged: bool,
    pub took_ms: u128,
    /// The purge stopped early because the application is shutting down
    /// ([`DiskCache::purge_while`]): files removed so far stay removed, the
    /// stored cache size is not updated and the rest is not reported, as
    /// upstream d527adf68c returns from `purge`.
    pub interrupted: bool,
}

/// `LLDiskCache`'s state.
#[derive(Clone, Debug)]
pub struct DiskCache {
    pub cache_dir: String,
    pub delim: String,
    pub max_size_bytes: u64,
    pub high_percent: f32,
    pub low_percent: f32,
    pub enable_debug_info: bool,
    /// `mSkipList`: the UUID strings of the static assets never purged.
    pub skip_list: Vec<String>,
    stored_cache_size: u64,
    last_scan: Option<Instant>,
}

impl DiskCache {
    /// The constructor: creates the directory and its 16 subdirectories.
    /// `prepopulate` follows separately (it needs the host's static
    /// asset folders).
    pub fn new(cache_dir: &str, delim: &str, max_size_bytes: u64, enable_debug_info: bool, high: f32, low: f32) -> DiskCache {
        let _ = fs::create_dir(cache_dir);
        for c in b"0123456789abcdef" {
            let _ = fs::create_dir(format!("{cache_dir}{delim}{}", *c as char));
        }
        DiskCache {
            cache_dir: cache_dir.to_string(),
            delim: delim.to_string(),
            max_size_bytes,
            high_percent: high,
            low_percent: low,
            enable_debug_info,
            skip_list: Vec::new(),
            stored_cache_size: 0,
            last_scan: None,
        }
    }

    pub fn set_max_size_bytes(&mut self, size: u64) {
        self.max_size_bytes = size;
    }

    /// `setHighWaterPercentage`: clamped to `[low, 100]`.
    pub fn set_high_water_percentage(&mut self, pct: f32) {
        self.high_percent = pct.clamp(self.low_percent, 100.0);
    }

    /// `setLowWaterPercentage`: clamped to `[0, high]`.
    pub fn set_low_water_percentage(&mut self, pct: f32) {
        self.low_percent = pct.clamp(0.0, self.high_percent);
    }

    pub fn filepath(&self, id: &Uuid) -> String {
        metadata_to_filepath(&self.cache_dir, &self.delim, id)
    }

    /// `purge`: removes the oldest files until the cache is under the low
    /// water mark, once it is over the high water mark; static assets are
    /// skipped (and counted).
    pub fn purge(&mut self) -> PurgeReport {
        self.purge_while(&mut || true)
    }

    /// `purge` with upstream d527adf68c's shutdown early-out: `running` is
    /// `LLApp::isRunning()`, asked before each directory entry of the scan,
    /// before each file of the delete loop and before each line of the
    /// debug listing; when it says no, the purge returns at once
    /// ([`PurgeReport::interrupted`]).
    pub fn purge_while(&mut self, running: &mut dyn FnMut() -> bool) -> PurgeReport {
        if self.enable_debug_info {
            let size = self.dir_file_size(false);
            ll_infos!("Total dir size before purge is {}", size);
        }
        let start = Instant::now();
        let mut file_info: Vec<(i64, u64, String)> = Vec::new();
        let mut file_size_total: u64 = 0;
        let completed = walk_while(Path::new(&self.cache_dir), CACHE_FILENAME_PREFIX, running, &mut |path, size, time| {
            file_size_total += size;
            file_info.push((time, size, path));
        });
        let mut report = PurgeReport { files: file_info.len(), size_before: file_size_total, ..Default::default() };
        if !completed {
            report.interrupted = true;
            return report;
        }
        ll_debugs!("Cache is {}% full", ((file_size_total as f32) / self.max_size_bytes as f32 * 100.0) as i32);
        if (file_size_total as f64) < self.max_size_bytes as f64 * (self.high_percent as f64 / 100.0) {
            ll_debugs!("Not exceded high water - do nothing");
            return report;
        }
        report.purged = true;
        // Oldest first; files with the same time keep their listing order
        // (the C++ std::sort gave no order among them).
        file_info.sort_by_key(|e| e.0);
        let target_size = (self.max_size_bytes as f64 * (self.low_percent as f64 / 100.0)) as u64;
        ll_infos!("Purging cache to a maximum of {} bytes", target_size);
        let mut keep = file_info.len();
        let mut del = 0u32;
        let mut skip = 0u32;
        let mut deleted_size_total: u64 = 0;
        let mut actions: Vec<(String, &'static str)> = Vec::new();
        for (_, this_file_size, path) in file_info.iter() {
            if !running() {
                report.interrupted = true;
                report.deleted_size = deleted_size_total;
                report.deleted = del;
                report.skipped = skip;
                return report;
            }
            let mut should_remove = (file_size_total - deleted_size_total) > target_size;
            if !should_remove {
                break;
            }
            deleted_size_total += this_file_size;
            let uuid_as_string = uuid_text_of_cache_file(path);
            let action;
            if self.skip_list.iter().any(|s| *s == uuid_as_string) {
                should_remove = false;
                action = "STATIC";
                skip += 1;
            } else {
                action = "DELETE";
                del += 1;
            }
            keep -= 1;
            if self.enable_debug_info {
                actions.push((path.clone(), action));
            }
            if should_remove {
                if let Err(e) = fs::remove_file(path) {
                    ll_warns!("Failed to delete cache file {}: {}", path, e);
                }
            }
        }
        let execute_time = start.elapsed().as_millis();
        if self.enable_debug_info {
            let mut deleted_so_far: u64 = 0;
            for (time, size, path) in file_info.iter() {
                if !running() {
                    report.interrupted = true;
                    report.deleted_size = deleted_size_total;
                    report.deleted = del;
                    report.skipped = skip;
                    return report;
                }
                deleted_so_far += size;
                let action = match actions.iter().find(|(p, _)| p == path) {
                    Some((_, "DELETE")) => {
                        del += 1;
                        "DELETE"
                    }
                    Some((_, "STATIC")) => {
                        skip += 1;
                        "STATIC"
                    }
                    Some(_) => "UNKNOWN",
                    None => "KEEP",
                };
                ll_infos!(
                    "{}  {}  {}  {} ({}/{})",
                    action,
                    time,
                    size,
                    path,
                    file_size_total - deleted_so_far,
                    self.max_size_bytes
                );
            }
        }
        let new_cache_size = self.update_cache_size(file_size_total - deleted_size_total);
        ll_infos!("Total dir size after purge is {}", new_cache_size);
        ll_infos!("Cache purge took {} ms to execute for {} files", execute_time, file_info.len());
        ll_infos!("Deleted: {} Skipped: {} Kept: {}", del, skip, keep);
        ll_infos!("Total of {} bytes removed.", deleted_size_total);
        report.deleted_size = deleted_size_total;
        report.deleted = del;
        report.skipped = skip;
        report.kept = keep;
        report.took_ms = execute_time;
        report
    }

    /// `getCacheInfo`.
    pub fn cache_info(&mut self) -> String {
        let max_in_mb = self.max_size_bytes as f32 / (1024.0 * 1024.0);
        let percent_used = if self.stored_cache_size > 0 {
            (self.stored_cache_size as f32 / self.max_size_bytes as f32) * 100.0
        } else {
            (self.dir_file_size(false) as f32 / self.max_size_bytes as f32) * 100.0
        };
        format!("Max size {:.1} MB ({:.1}% used)", max_in_mb, percent_used)
    }

    /// `prepopulateCacheWithStatic`: copies the static assets of each
    /// folder into the cache (when absent) and records their ids in the
    /// skip list. `folders` are the existing source folders; each entry
    /// lists its files (name, full path).
    pub fn prepopulate(&mut self, folders: &[(String, Vec<String>)]) {
        self.skip_list.clear();
        for (from_folder, files) in folders {
            for from_asset_file in files {
                let uuid_as_string = Path::new(from_asset_file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let uuid = crate::uuid_from_string(&uuid_as_string).unwrap_or([0; 16]);
                let to_asset_file = self.filepath(&uuid);
                if !Path::new(&to_asset_file).exists() {
                    if self.enable_debug_info {
                        ll_infos!("Copying static asset {} to cache from {}", from_asset_file, from_folder);
                    }
                    if fs::copy(from_asset_file, &to_asset_file).is_err() {
                        ll_warns!("Failed to copy {} to {}", from_asset_file, to_asset_file);
                    }
                }
                if !self.skip_list.iter().any(|s| *s == uuid_as_string) {
                    if self.enable_debug_info {
                        ll_infos!("Adding {} to skip list", uuid_as_string);
                    }
                    self.skip_list.push(uuid_as_string);
                }
            }
        }
    }

    /// `clearCache`: removes every cache file (then the host prepopulates).
    pub fn clear_cache(&mut self) {
        ll_infos!("clearing cache {}", self.cache_dir);
        let mut paths = Vec::new();
        walk(Path::new(&self.cache_dir), CACHE_FILENAME_PREFIX, &mut |path, _, _| paths.push(path));
        for p in paths {
            if let Err(e) = fs::remove_file(&p) {
                ll_warns!("Failed to delete cache file {}: {}", p, e);
            }
        }
        ll_infos!("Cleared cache {}", self.cache_dir);
    }

    /// `removeOldVFSFiles`: removes the pre-2021 VFS files of the cache
    /// directory (`inv.llsd`, `db2.x`), not recursing.
    pub fn remove_old_vfs_files(cache_path: &str) {
        let Ok(rd) = fs::read_dir(cache_path) else { return };
        for e in rd.flatten() {
            if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let s = e.path().to_string_lossy().into_owned();
            if s.contains("inv.llsd") || s.contains("db2.x") {
                if let Err(err) = fs::remove_file(e.path()) {
                    ll_warns!("Failed to delete cache file {}: {}", s, err);
                }
            }
        }
    }

    /// Takes over the scan result (`mStoredCacheSize`, `mLastScanTime`) of
    /// a purge run on a copy of this cache.
    pub fn absorb_scan(&mut self, from: &DiskCache) {
        self.stored_cache_size = from.stored_cache_size;
        self.last_scan = from.last_scan;
    }

    fn update_cache_size(&mut self, newsize: u64) -> u64 {
        self.stored_cache_size = newsize;
        self.last_scan = Some(Instant::now());
        self.stored_cache_size
    }

    /// `dirFileSize`: the total size of the cache files, from the scan
    /// cached within the last 120 seconds unless `force`.
    pub fn dir_file_size(&mut self, force: bool) -> u64 {
        const CACHE_DURATION_SECS: u64 = 120;
        if !force {
            if let Some(last) = self.last_scan {
                if last.elapsed().as_secs() < CACHE_DURATION_SECS {
                    ll_debugs!("Using cached result: {}", self.stored_cache_size);
                    return self.stored_cache_size;
                }
            }
        }
        let mut total: u64 = 0;
        walk(Path::new(&self.cache_dir), CACHE_FILENAME_PREFIX, &mut |_, size, _| total += size);
        self.update_cache_size(total)
    }

    /// The current time in seconds, for callers that stamp files.
    pub fn now_secs() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming() {
        let id = crate::uuid_from_string("a1234567-89ab-cdef-1032-547698badcfe").unwrap();
        // The platform's delimiter: the name is read back through the
        // platform's path splitting, as getBaseFileName does.
        let d = std::path::MAIN_SEPARATOR.to_string();
        let p = metadata_to_filepath(&format!("{d}cache"), &d, &id);
        assert_eq!(p, format!("{d}cache{d}a{d}sl_cache_a1234567-89ab-cdef-1032-547698badcfe_0.asset"));
        assert_eq!(uuid_text_of_cache_file(&p), "a1234567-89ab-cdef-1032-547698badcfe");
    }

    #[test]
    fn purge_by_age() {
        let dir = std::env::temp_dir().join(format!("vks_diskcache_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let dirs = dir.to_string_lossy().into_owned();
        let delim = std::path::MAIN_SEPARATOR.to_string();
        let mut cache = DiskCache::new(&dirs, &delim, 1000, false, 90.0, 50.0);
        let mut ids = Vec::new();
        for i in 0..10u8 {
            let id = [i + 1; 16];
            let p = cache.filepath(&id);
            fs::write(&p, vec![0u8; 100]).unwrap();
            // Distinct, increasing modification times.
            let t = filetime_secs(1_600_000_000 + i as i64 * 10);
            fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
            ids.push(id);
        }
        cache.skip_list.push(uuid_to_string(&ids[0]));
        let r = cache.purge();
        assert!(r.purged);
        assert_eq!(r.files, 10);
        // 1000 bytes > 900 high water; target 500: the five oldest go, the
        // static one among them is skipped (and counted).
        assert_eq!(r.skipped, 1);
        assert_eq!(r.deleted, 4);
        assert_eq!(r.kept, 5);
        assert!(Path::new(&cache.filepath(&ids[0])).exists());
        assert!(!Path::new(&cache.filepath(&ids[1])).exists());
        assert!(Path::new(&cache.filepath(&ids[5])).exists());
        assert_eq!(cache.dir_file_size(true), 600);
        cache.clear_cache();
        assert_eq!(cache.dir_file_size(true), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Upstream d527adf68c: a purge asked to stop deletes nothing more,
    /// leaves the stored size alone and says it was interrupted.
    #[test]
    fn purge_stops_when_the_app_is_not_running() {
        let dir = std::env::temp_dir().join(format!("vks_diskcache_stop_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let dirs = dir.to_string_lossy().into_owned();
        let delim = std::path::MAIN_SEPARATOR.to_string();
        let mut cache = DiskCache::new(&dirs, &delim, 1000, false, 90.0, 50.0);
        let mut ids = Vec::new();
        for i in 0..10u8 {
            let id = [i + 1; 16];
            let p = cache.filepath(&id);
            fs::write(&p, vec![0u8; 100]).unwrap();
            let t = filetime_secs(1_600_000_000 + i as i64 * 10);
            fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
            ids.push(id);
        }
        // Stopped before the scan: nothing listed, nothing deleted.
        let r = cache.purge_while(&mut || false);
        assert!(r.interrupted && !r.purged && r.files == 0);
        assert_eq!(ids.iter().filter(|id| Path::new(&cache.filepath(id)).exists()).count(), 10);
        // Stopped after the scan, before the second deletion: one file goes.
        let mut scan_checks = 0;
        walk_while(Path::new(&dirs), CACHE_FILENAME_PREFIX, &mut || {
            scan_checks += 1;
            true
        }, &mut |_, _, _| {});
        let mut asked = 0;
        let mut running = || {
            asked += 1;
            asked <= scan_checks + 1
        };
        let r = cache.purge_while(&mut running);
        assert!(r.interrupted && r.purged);
        assert_eq!((r.files, r.deleted), (10, 1));
        assert!(!Path::new(&cache.filepath(&ids[0])).exists());
        assert!(Path::new(&cache.filepath(&ids[1])).exists());
        // Running throughout: the ordinary purge.
        let r = cache.purge_while(&mut || true);
        assert!(!r.interrupted);
        let _ = fs::remove_dir_all(&dir);
    }

    fn filetime_secs(s: i64) -> SystemTime {
        UNIX_EPOCH + std::time::Duration::from_secs(s as u64)
    }
}
