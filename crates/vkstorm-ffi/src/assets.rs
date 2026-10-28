//! Rung five: the asset pipeline (`vks_texcache_*`, `vks_tfetch_*`,
//! `vks_retry_*`, `vks_diskcache_*`, `vks_asset_file_*`, `vks_lfs_*`,
//! `vks_mesh_*`, `vks_asset_*`), the C ABI over `vkstorm-assets` behind
//! `LLTextureCache`, `LLTextureFetchWorker`/`LLTextureFetch`,
//! `LLAdaptiveRetryPolicy`, `LLDiskCache`, `LLFileSystem`, `LLLFSThread`,
//! `LLVolume::unpackVolumeFaces`, `LLMeshSkinInfo`, `LLModel::Decomposition`,
//! `LLMeshHeader`, the mesh repository thread's planning and
//! `LLAssetStorage`'s bookkeeping.
//!
//! Panics are caught at the boundary and reported as failures. The host's
//! callbacks (the texture fetch host table, the log) must not throw.

use std::ffi::{c_char, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::slice;
use std::sync::Mutex;

use vkstorm_assets::asset::{self, RequestKey, ToxicMap, Verdict};
use vkstorm_assets::diskcache::{self, file as dfile, DiskCache};
use vkstorm_assets::fetch::{budget, plan, retry::AdaptiveRetryPolicy, Env, FetchType, Fetcher, Host, HttpGet, Region, Release, State, Worker};
use vkstorm_assets::host::{LogLevel, set_logger};
use vkstorm_assets::mesh::{self, header::{MeshHeader, NUM_LODS}, plan as mplan, skin::{Decomposition, SkinInfo}, unpack::Face};
use vkstorm_assets::texcache::{self, Config, FastEntry, Paths, TextureCache};
use vkstorm_assets::Uuid;

unsafe fn bytes<'a>(p: *const u8, len: usize) -> &'a [u8] {
    if p.is_null() || len == 0 { &[] } else { unsafe { slice::from_raw_parts(p, len) } }
}

unsafe fn str_arg<'a>(p: *const c_char, len: usize) -> &'a str {
    let b = unsafe { bytes(p as *const u8, len) };
    std::str::from_utf8(b).unwrap_or("")
}

unsafe fn uuid_arg(p: *const u8) -> Uuid {
    let mut u = [0u8; 16];
    if !p.is_null() {
        u.copy_from_slice(unsafe { slice::from_raw_parts(p, 16) });
    }
    u
}

/// Copies `s` into the caller's buffer (NUL-terminated when there is room)
/// and returns the full length.
unsafe fn copy_out(s: &[u8], buf: *mut c_char, cap: usize) -> usize {
    if !buf.is_null() && cap > 0 {
        let n = s.len().min(cap - 1);
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), buf as *mut u8, n);
            *buf.add(n) = 0;
        }
    }
    s.len()
}

// ---------------------------------------------------------------------------
// The host's log

/// Installs the log sink the crate reports through: `level` is 0 debug,
/// 1 info, 2 warn.
#[unsafe(no_mangle)]
pub extern "C" fn vks_assets_set_log(f: Option<unsafe extern "C" fn(i32, *const c_char, usize)>) {
    if let Some(f) = f {
        struct SendFn(unsafe extern "C" fn(i32, *const c_char, usize));
        unsafe impl Send for SendFn {}
        unsafe impl Sync for SendFn {}
        let f = SendFn(f);
        set_logger(Box::new(move |level: LogLevel, msg: &str| unsafe { (f.0)(level as i32, msg.as_ptr() as *const c_char, msg.len()) }));
    }
}

// ---------------------------------------------------------------------------
// The texture cache

pub struct VksTexCache {
    inner: TextureCache,
}

/// `LLTextureCache`'s directories for one location (`setDirNames`).
#[repr(C)]
pub struct VksTexCachePaths {
    pub location: *const c_char,
    pub location_len: usize,
    pub delim: *const c_char,
    pub delim_len: usize,
}

unsafe fn paths_arg(p: *const VksTexCachePaths) -> Paths {
    if p.is_null() {
        return Paths::default();
    }
    let p = unsafe { &*p };
    let location = unsafe { str_arg(p.location, p.location_len) };
    let delim = unsafe { str_arg(p.delim, p.delim_len) };
    Paths::for_location(location, delim)
}

/// Creates a cache with the engine string, the address size and the
/// clock (null: the system clock) the entries are stamped with.
///
/// # Safety
/// `encoder` must point to `encoder_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_texcache_new(encoder: *const c_char, encoder_len: usize, address_size: u32, clock: Option<extern "C" fn() -> u32>) -> *mut VksTexCache {
    let encoder = unsafe { str_arg(encoder, encoder_len) }.to_owned();
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mut config = Config::new(&encoder, address_size);
        if let Some(c) = clock {
            // A C clock has the same ABI as the Rust fn pointer on every
            // platform the viewer builds for; store it as such.
            config.clock = unsafe { std::mem::transmute::<extern "C" fn() -> u32, fn() -> u32>(c) };
        }
        TextureCache::new(config)
    }));
    match r {
        Ok(inner) => Box::into_raw(Box::new(VksTexCache { inner })),
        Err(_) => std::ptr::null_mut(),
    }
}

/// # Safety
/// `c` must come from `vks_texcache_new` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_texcache_free(c: *mut VksTexCache) {
    if !c.is_null() {
        drop(unsafe { Box::from_raw(c) });
    }
}

macro_rules! cache_fn {
    ($name:ident, |$c:ident $(, $arg:ident : $ty:ty)*| -> $ret:ty, $default:expr, $body:expr) => {
        /// See the C header.
        ///
        /// # Safety
        /// `c` must come from `vks_texcache_new` and be live; pointer
        /// arguments must be valid for their lengths.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(c: *mut VksTexCache $(, $arg: $ty)*) -> $ret {
            if c.is_null() {
                return $default;
            }
            let $c = &unsafe { &*c }.inner;
            catch_unwind(AssertUnwindSafe(|| $body)).unwrap_or($default)
        }
    };
}

cache_fn!(vks_texcache_set_read_only, |c, read_only: i32| -> i32, 0, { c.set_read_only(read_only != 0); 1 });
cache_fn!(vks_texcache_is_read_only, |c| -> i32, 1, c.is_read_only() as i32);
cache_fn!(vks_texcache_usage, |c| -> i64, 0, c.usage());
cache_fn!(vks_texcache_max_usage, |c| -> i64, 0, c.max_usage());
cache_fn!(vks_texcache_entries, |c| -> u32, 0, c.entries());
cache_fn!(vks_texcache_max_entries, |c| -> u32, 0, c.max_entries());
cache_fn!(vks_texcache_is_in_cache, |c, id: *const u8| -> i32, 0, c.is_in_cache(&unsafe { uuid_arg(id) }) as i32);
cache_fn!(vks_texcache_purge_pending, |c| -> i32, 0, c.purge_pending() as i32);
cache_fn!(vks_texcache_purge_cache, |c, paths: *const VksTexCachePaths, remove_dir: i32| -> i32, 0, {
    c.purge_cache(&unsafe { paths_arg(paths) }, remove_dir != 0);
    1
});
cache_fn!(vks_texcache_init, |c, paths: *const VksTexCachePaths, max_size: i64, mismatch: i32, validate_idx: i32| -> i64, 0, {
    c.init_cache(&unsafe { paths_arg(paths) }, max_size, mismatch != 0, if validate_idx < 0 { None } else { Some(validate_idx as u32) })
});
cache_fn!(vks_texcache_lazy_purge_tick, |c, time_limit: f32| -> i32, 0, c.lazy_purge_tick(time_limit) as i32);
cache_fn!(vks_texcache_write_updated_entries, |c| -> i32, 0, { c.write_updated_entries(); 1 });
cache_fn!(vks_texcache_shutdown, |c| -> i32, 0, { c.shutdown(); 1 });
cache_fn!(vks_texcache_remove, |c, id: *const u8| -> i32, 0, c.remove(&unsafe { uuid_arg(id) }) as i32);
cache_fn!(vks_texcache_texture_file_name, |c, id: *const u8, buf: *mut c_char, cap: usize| -> usize, 0, {
    let s = c.texture_file_name(&unsafe { uuid_arg(id) });
    unsafe { copy_out(s.as_bytes(), buf, cap) }
});

/// `doRead`: reads up to `size` bytes at `offset` of the texture into
/// `buf` (at least `size` bytes). Returns the data size (0: not cached,
/// -1: failed) and the entry's image size.
///
/// # Safety
/// `buf` must be writable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_texcache_read(c: *mut VksTexCache, id: *const u8, offset: i32, size: i32, buf: *mut u8, image_size: *mut i32) -> i32 {
    if c.is_null() || buf.is_null() || size < 0 {
        return -1;
    }
    let cache = &unsafe { &*c }.inner;
    let id = unsafe { uuid_arg(id) };
    let b = unsafe { slice::from_raw_parts_mut(buf, size as usize) };
    match catch_unwind(AssertUnwindSafe(|| cache.read_into(&id, offset, size, b))) {
        Ok((data_size, img)) => {
            if !image_size.is_null() {
                unsafe { *image_size = img };
            }
            data_size
        }
        Err(_) => -1,
    }
}

/// The fast-cache record a write carries: the raw image scaled by the host
/// to the level `vks_texcache_fast_level` gives.
#[repr(C)]
pub struct VksFastEntry {
    pub width: i32,
    pub height: i32,
    pub components: i32,
    pub discard_level: i32,
    pub data: *const u8,
    pub data_len: usize,
}

/// `writeToFastCache`'s level search.
#[unsafe(no_mangle)]
pub extern "C" fn vks_texcache_fast_level(width: i32, height: i32, components: i32) -> i32 {
    texcache::fast_cache_level(width, height, components)
}

/// `doWrite`. Returns 1 on success, 0 on failure (the host then removes
/// the entry).
///
/// # Safety
/// `data` must point to `len` bytes; `fast` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_texcache_write(c: *mut VksTexCache, id: *const u8, data: *const u8, len: usize, image_size: i32, discard_level: i32, fast: *const VksFastEntry) -> i32 {
    if c.is_null() || fast.is_null() {
        return 0;
    }
    let cache = &unsafe { &*c }.inner;
    let id = unsafe { uuid_arg(id) };
    let data = unsafe { bytes(data, len) };
    let f = unsafe { &*fast };
    let fast = FastEntry {
        width: f.width,
        height: f.height,
        components: f.components,
        discard_level: f.discard_level,
        data: unsafe { bytes(f.data, f.data_len) }.to_vec(),
    };
    catch_unwind(AssertUnwindSafe(|| cache.write(&id, data, image_size, discard_level, &fast) as i32)).unwrap_or(0)
}

/// `readFromFastCache`: fills the head fields and copies the pixels into
/// `data` (1024 bytes). Returns 1 when the record was read.
///
/// # Safety
/// `data` must be writable for 1024 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_texcache_read_fast(c: *mut VksTexCache, id: *const u8, width: *mut i32, height: *mut i32, components: *mut i32, discard_level: *mut i32, data: *mut u8) -> i32 {
    if c.is_null() || data.is_null() {
        return 0;
    }
    let cache = &unsafe { &*c }.inner;
    let id = unsafe { uuid_arg(id) };
    match catch_unwind(AssertUnwindSafe(|| cache.read_fast(&id))) {
        Ok(Some(r)) => {
            unsafe {
                if !width.is_null() { *width = r.width; }
                if !height.is_null() { *height = r.height; }
                if !components.is_null() { *components = r.components; }
                if !discard_level.is_null() { *discard_level = r.discard_level; }
                let n = r.data.len().min(texcache::FAST_DATA_SIZE as usize);
                std::ptr::copy_nonoverlapping(r.data.as_ptr(), data, n);
            }
            1
        }
        _ => 0,
    }
}

/// `readFromFastCache` without waiting for the fast cache (upstream #5356:
/// the texture list skips the frame while a worker writes it). Fills the
/// head fields and copies the pixels into `data` (1024 bytes) as
/// [`vks_texcache_read_fast`]. Returns 1 when the record was read, 0 when it
/// is not cached or could not be read, and -1 when the fast cache is busy
/// (nothing read, nothing waited).
///
/// # Safety
/// `data` must be writable for 1024 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_texcache_try_read_fast(c: *mut VksTexCache, id: *const u8, width: *mut i32, height: *mut i32, components: *mut i32, discard_level: *mut i32, data: *mut u8) -> i32 {
    if c.is_null() || data.is_null() {
        return 0;
    }
    let cache = &unsafe { &*c }.inner;
    let id = unsafe { uuid_arg(id) };
    match catch_unwind(AssertUnwindSafe(|| cache.try_read_fast(&id))) {
        Ok(texcache::FastRead::Hit(r)) => {
            unsafe {
                if !width.is_null() { *width = r.width; }
                if !height.is_null() { *height = r.height; }
                if !components.is_null() { *components = r.components; }
                if !discard_level.is_null() { *discard_level = r.discard_level; }
                let n = r.data.len().min(texcache::FAST_DATA_SIZE as usize);
                std::ptr::copy_nonoverlapping(r.data.as_ptr(), data, n);
            }
            1
        }
        Ok(texcache::FastRead::Busy) => -1,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Plain file operations (LLAPRFile by name, LLFileSystem, LLLFSThread)

/// `LLAPRFile::size`: 0 when absent.
///
/// # Safety
/// `path` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_file_size(path: *const c_char, len: usize) -> i32 {
    let p = unsafe { str_arg(path, len) };
    catch_unwind(AssertUnwindSafe(|| texcache::files::file_size(p))).unwrap_or(0)
}

/// `LLAPRFile::readEx`: bytes read into `buf` from `offset`.
///
/// # Safety
/// `buf` must be writable for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_file_read_at(path: *const c_char, len: usize, offset: i64, buf: *mut u8, cap: usize) -> i32 {
    if buf.is_null() {
        return 0;
    }
    let p = unsafe { str_arg(path, len) };
    let b = unsafe { slice::from_raw_parts_mut(buf, cap) };
    catch_unwind(AssertUnwindSafe(|| texcache::files::read_at(p, offset, b))).unwrap_or(0)
}

/// `LLFileSystem::getExists`: a regular file with a positive size.
///
/// # Safety
/// `path` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_asset_file_exists(path: *const c_char, len: usize) -> i32 {
    let p = unsafe { str_arg(path, len) };
    catch_unwind(AssertUnwindSafe(|| dfile::exists(p) as i32)).unwrap_or(0)
}

/// `LLFileSystem::getFileSize`.
///
/// # Safety
/// `path` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_asset_file_size(path: *const c_char, len: usize) -> i32 {
    let p = unsafe { str_arg(path, len) };
    catch_unwind(AssertUnwindSafe(|| dfile::file_size(p))).unwrap_or(0)
}

/// `LLFileSystem::read`: the bytes read at `position`, -1 when the file
/// cannot be opened or positioned.
///
/// # Safety
/// `buf` must be writable for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_asset_file_read(path: *const c_char, len: usize, position: i32, buf: *mut u8, cap: usize) -> i32 {
    if buf.is_null() {
        return -1;
    }
    let p = unsafe { str_arg(path, len) };
    let b = unsafe { slice::from_raw_parts_mut(buf, cap) };
    catch_unwind(AssertUnwindSafe(|| dfile::read_at(p, position, b).unwrap_or(-1))).unwrap_or(-1)
}

/// `LLFileSystem::write` in `mode` (1 read, 2 write, 3 read-write, 6
/// append): the new position, -1 on failure.
///
/// # Safety
/// `data` must point to `data_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_asset_file_write(path: *const c_char, len: usize, mode: i32, position: i32, data: *const u8, data_len: usize) -> i32 {
    let p = unsafe { str_arg(path, len) };
    let d = unsafe { bytes(data, data_len) };
    catch_unwind(AssertUnwindSafe(|| dfile::write_at(p, dfile::Mode::from_i32(mode), position, d).unwrap_or(-1))).unwrap_or(-1)
}

/// `LLFileSystem::seek`'s arithmetic: writes the new position; returns 1
/// when within the file, 0 when clamped.
///
/// # Safety
/// `new_position` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_asset_file_seek(position: i32, offset: i32, origin: i32, size: i32, new_position: *mut i32) -> i32 {
    let (ok, pos) = match dfile::seek_position(position, offset, origin, size) {
        Ok(p) => (1, p),
        Err(p) => (0, p),
    };
    if !new_position.is_null() {
        unsafe { *new_position = pos };
    }
    ok
}

/// `LLFileSystem::removeFile`.
///
/// # Safety
/// `path` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_asset_file_remove(path: *const c_char, len: usize) {
    let p = unsafe { str_arg(path, len) };
    let _ = catch_unwind(AssertUnwindSafe(|| dfile::remove_file(p)));
}

/// `LLFileSystem::renameFile`; 1 on success.
///
/// # Safety
/// Both paths must point to their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_asset_file_rename(from: *const c_char, from_len: usize, to: *const c_char, to_len: usize) -> i32 {
    let f = unsafe { str_arg(from, from_len) };
    let t = unsafe { str_arg(to, to_len) };
    catch_unwind(AssertUnwindSafe(|| dfile::rename_file(f, t) as i32)).unwrap_or(0)
}

/// `LLFileSystem::updateFileAccessTime`.
///
/// # Safety
/// `path` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_asset_file_touch(path: *const c_char, len: usize) {
    let p = unsafe { str_arg(path, len) };
    let _ = catch_unwind(AssertUnwindSafe(|| dfile::update_file_access_time(p)));
}

/// `LLLFSThread` read: bytes read (0 on failure); `offset` < 0 reads from
/// the end.
///
/// # Safety
/// `buf` must be writable for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_lfs_read(path: *const c_char, len: usize, offset: i32, buf: *mut u8, cap: usize) -> i32 {
    if buf.is_null() {
        return 0;
    }
    let p = unsafe { str_arg(path, len) };
    let b = unsafe { slice::from_raw_parts_mut(buf, cap) };
    catch_unwind(AssertUnwindSafe(|| dfile::lfs_read(p, offset, b))).unwrap_or(0)
}

/// `LLLFSThread` write: bytes written (0 on failure); `offset` < 0 appends.
///
/// # Safety
/// `data` must point to `data_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_lfs_write(path: *const c_char, len: usize, offset: i32, data: *const u8, data_len: usize) -> i32 {
    let p = unsafe { str_arg(path, len) };
    let d = unsafe { bytes(data, data_len) };
    catch_unwind(AssertUnwindSafe(|| dfile::lfs_write(p, offset, d))).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// The texture fetcher

pub struct VksTexFetcher {
    inner: Mutex<Fetcher>,
}

pub struct VksTexFetchWorker {
    inner: Worker,
}

/// The environment of one `doWork` call (see `fetch::Env`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VksTexFetchEnv {
    pub quitting: i32,
    pub delete_requested: i32,
    pub debug_pause: i32,
    pub non_interactive: i32,
    pub blacklisted: i32,
    pub formatted_present: i32,
    pub formatted_size: i32,
    pub formatted_discard: i32,
    pub formatted_is_j2c: i32,
    pub formatted_width: i32,
    pub formatted_height: i32,
    pub can_load_from_cache: i32,
    pub use_http_textures: i32,
    pub disconnected: i32,
    pub range_requests_disabled: i32,
    pub decode_disabled: i32,
    pub raw_width: i32,
    pub raw_height: i32,
    pub texture_bandwidth: f32,
    pub max_bandwidth: f32,
}

/// The region as the host reports it (`getRegion()`); `present` 0 when
/// there is none.
#[repr(C)]
pub struct VksTexFetchRegion {
    pub present: i32,
    pub id: [u8; 16],
    pub viewer_asset_url: *const c_char,
    pub viewer_asset_url_len: usize,
    pub http_url: *const c_char,
    pub http_url_len: usize,
}

/// The HTTP GET the state machine asks for.
#[repr(C)]
pub struct VksTexFetchHttpGet {
    pub url: *const c_char,
    pub url_len: usize,
    pub offset: i32,
    pub size: i32,
    pub no_range: i32,
    pub with_headers: i32,
}

/// The host's side of the state machine: the C++ worker's objects. Every
/// function receives `ctx` (the C++ worker). None may throw.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksTexFetchHost {
    pub ctx: *mut c_void,
    pub env: Option<unsafe extern "C" fn(*mut c_void, *mut VksTexFetchEnv)>,
    pub region: Option<unsafe extern "C" fn(*mut c_void, *mut VksTexFetchRegion)>,
    pub read_cache: Option<unsafe extern "C" fn(*mut c_void, *const c_char, usize, i32, i32)>,
    pub cache_read_complete: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
    pub send_http: Option<unsafe extern "C" fn(*mut c_void, *const VksTexFetchHttpGet) -> i32>,
    pub drop_raw_images: Option<unsafe extern "C" fn(*mut c_void)>,
    pub decode: Option<unsafe extern "C" fn(*mut c_void, i32, i32) -> u32>,
    pub write_cache: Option<unsafe extern "C" fn(*mut c_void, i32, i32)>,
    pub cache_write_complete: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
    pub prioritize_write: Option<unsafe extern "C" fn(*mut c_void)>,
    pub remove_from_cache: Option<unsafe extern "C" fn(*mut c_void)>,
    pub reset_formatted_data: Option<unsafe extern "C" fn(*mut c_void)>,
    pub drop_formatted_image: Option<unsafe extern "C" fn(*mut c_void)>,
    pub release_http_buffer: Option<unsafe extern "C" fn(*mut c_void)>,
    pub assemble_http_reply: Option<unsafe extern "C" fn(*mut c_void, *const c_char, usize, i32, i32, i32, i32) -> i32>,
    pub add_to_http_queue: Option<unsafe extern "C" fn(*mut c_void)>,
    pub record_texture_start: Option<unsafe extern "C" fn(*mut c_void, i32)>,
    pub record_texture_done: Option<unsafe extern "C" fn(*mut c_void, i32, f64)>,
    pub stat_cache_attempt: Option<unsafe extern "C" fn(*mut c_void)>,
    pub stat_cache_hit: Option<unsafe extern "C" fn(*mut c_void)>,
    pub stat_cache_hit_rate: Option<unsafe extern "C" fn(*mut c_void, i32)>,
    pub log: Option<unsafe extern "C" fn(*mut c_void, i32, i32, *const c_char, usize)>,
    pub add_to_network_queue: Option<unsafe extern "C" fn(*mut c_void)>,
    pub remove_from_network_queue: Option<unsafe extern "C" fn(*mut c_void, i32)>,
    pub assemble_packets: Option<unsafe extern "C" fn(*mut c_void, i32, i32) -> i32>,
}

struct CHost(VksTexFetchHost);

impl Host for CHost {
    fn env(&mut self) -> Env {
        let mut e = VksTexFetchEnv::default();
        if let Some(f) = self.0.env {
            unsafe { f(self.0.ctx, &mut e) };
        }
        Env {
            quitting: e.quitting != 0,
            delete_requested: e.delete_requested != 0,
            debug_pause: e.debug_pause != 0,
            non_interactive: e.non_interactive != 0,
            blacklisted: e.blacklisted != 0,
            formatted_present: e.formatted_present != 0,
            formatted_size: e.formatted_size,
            formatted_discard: e.formatted_discard,
            formatted_is_j2c: e.formatted_is_j2c != 0,
            can_load_from_cache: e.can_load_from_cache != 0,
            use_http_textures: e.use_http_textures != 0,
            disconnected: e.disconnected != 0,
            range_requests_disabled: e.range_requests_disabled != 0,
            decode_disabled: e.decode_disabled != 0,
            texture_bandwidth: e.texture_bandwidth,
            max_bandwidth: e.max_bandwidth,
            raw_width: e.raw_width,
            raw_height: e.raw_height,
            formatted_width: e.formatted_width,
            formatted_height: e.formatted_height,
        }
    }
    fn region(&mut self) -> Option<Region> {
        let mut r = VksTexFetchRegion { present: 0, id: [0; 16], viewer_asset_url: std::ptr::null(), viewer_asset_url_len: 0, http_url: std::ptr::null(), http_url_len: 0 };
        if let Some(f) = self.0.region {
            unsafe { f(self.0.ctx, &mut r) };
        }
        if r.present == 0 {
            return None;
        }
        Some(Region {
            id: r.id,
            viewer_asset_url: unsafe { str_arg(r.viewer_asset_url, r.viewer_asset_url_len) }.to_owned(),
            http_url: unsafe { str_arg(r.http_url, r.http_url_len) }.to_owned(),
        })
    }
    fn read_cache(&mut self, local_file: Option<&str>, offset: i32, size: i32) {
        if let Some(f) = self.0.read_cache {
            match local_file {
                Some(name) => unsafe { f(self.0.ctx, name.as_ptr() as *const c_char, name.len(), offset, size) },
                None => unsafe { f(self.0.ctx, std::ptr::null(), 0, offset, size) },
            }
        }
    }
    fn cache_read_complete(&mut self) -> bool {
        self.0.cache_read_complete.map(|f| unsafe { f(self.0.ctx) } != 0).unwrap_or(true)
    }
    fn send_http(&mut self, get: &HttpGet) -> bool {
        let Some(f) = self.0.send_http else { return false };
        let g = VksTexFetchHttpGet {
            url: get.url.as_ptr() as *const c_char,
            url_len: get.url.len(),
            offset: get.offset,
            size: get.size,
            no_range: get.no_range as i32,
            with_headers: get.with_headers as i32,
        };
        unsafe { f(self.0.ctx, &g) != 0 }
    }
    fn drop_raw_images(&mut self) {
        if let Some(f) = self.0.drop_raw_images {
            unsafe { f(self.0.ctx) };
        }
    }
    fn decode(&mut self, discard: i32, needs_aux: bool) -> u32 {
        self.0.decode.map(|f| unsafe { f(self.0.ctx, discard, needs_aux as i32) }).unwrap_or(0)
    }
    fn write_cache(&mut self, file_size: i32, decoded_discard: i32) {
        if let Some(f) = self.0.write_cache {
            unsafe { f(self.0.ctx, file_size, decoded_discard) };
        }
    }
    fn cache_write_complete(&mut self) -> bool {
        self.0.cache_write_complete.map(|f| unsafe { f(self.0.ctx) } != 0).unwrap_or(true)
    }
    fn prioritize_write(&mut self) {
        if let Some(f) = self.0.prioritize_write {
            unsafe { f(self.0.ctx) };
        }
    }
    fn remove_from_cache(&mut self) {
        if let Some(f) = self.0.remove_from_cache {
            unsafe { f(self.0.ctx) };
        }
    }
    fn reset_formatted_data(&mut self) {
        if let Some(f) = self.0.reset_formatted_data {
            unsafe { f(self.0.ctx) };
        }
    }
    fn drop_formatted_image(&mut self) {
        if let Some(f) = self.0.drop_formatted_image {
            unsafe { f(self.0.ctx) };
        }
    }
    fn release_http_buffer(&mut self) {
        if let Some(f) = self.0.release_http_buffer {
            unsafe { f(self.0.ctx) };
        }
    }
    fn assemble_http_reply(&mut self, url: &str, src_offset: i32, cur_size: i32, append_size: i32, total_size: i32) -> bool {
        self.0
            .assemble_http_reply
            .map(|f| unsafe { f(self.0.ctx, url.as_ptr() as *const c_char, url.len(), src_offset, cur_size, append_size, total_size) } != 0)
            .unwrap_or(false)
    }
    fn add_to_http_queue(&mut self) {
        if let Some(f) = self.0.add_to_http_queue {
            unsafe { f(self.0.ctx) };
        }
    }
    fn record_texture_start(&mut self, is_http: bool) {
        if let Some(f) = self.0.record_texture_start {
            unsafe { f(self.0.ctx, is_http as i32) };
        }
    }
    fn record_texture_done(&mut self, is_http: bool, byte_count: f64) {
        if let Some(f) = self.0.record_texture_done {
            unsafe { f(self.0.ctx, is_http as i32, byte_count) };
        }
    }
    fn stat_cache_attempt(&mut self) {
        if let Some(f) = self.0.stat_cache_attempt {
            unsafe { f(self.0.ctx) };
        }
    }
    fn stat_cache_hit(&mut self) {
        if let Some(f) = self.0.stat_cache_hit {
            unsafe { f(self.0.ctx) };
        }
    }
    fn stat_cache_hit_rate(&mut self, hit: bool) {
        if let Some(f) = self.0.stat_cache_hit_rate {
            unsafe { f(self.0.ctx, hit as i32) };
        }
    }
    fn log(&mut self, level: i32, once: bool, msg: &str) {
        if let Some(f) = self.0.log {
            unsafe { f(self.0.ctx, level, once as i32, msg.as_ptr() as *const c_char, msg.len()) };
        }
    }
    #[cfg(feature = "opensim")]
    fn add_to_network_queue(&mut self) {
        if let Some(f) = self.0.add_to_network_queue {
            unsafe { f(self.0.ctx) };
        }
    }
    #[cfg(feature = "opensim")]
    fn remove_from_network_queue(&mut self, cancel: bool) {
        if let Some(f) = self.0.remove_from_network_queue {
            unsafe { f(self.0.ctx, cancel as i32) };
        }
    }
    #[cfg(feature = "opensim")]
    fn assemble_packets(&mut self, first_packet: i32, last_packet: i32) -> i32 {
        self.0.assemble_packets.map(|f| unsafe { f(self.0.ctx, first_packet, last_packet) }).unwrap_or(0)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn vks_tfetch_fetcher_new() -> *mut VksTexFetcher {
    Box::into_raw(Box::new(VksTexFetcher { inner: Mutex::new(Fetcher::new()) }))
}

/// # Safety
/// `f` must come from `vks_tfetch_fetcher_new` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tfetch_fetcher_free(f: *mut VksTexFetcher) {
    if !f.is_null() {
        drop(unsafe { Box::from_raw(f) });
    }
}

macro_rules! fetcher_fn {
    ($name:ident, |$f:ident $(, $arg:ident : $ty:ty)*| -> $ret:ty, $default:expr, $body:expr) => {
        /// See the C header.
        ///
        /// # Safety
        /// `f` must come from `vks_tfetch_fetcher_new` and be live; the
        /// caller holds the fetcher's lock.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(f: *mut VksTexFetcher $(, $arg: $ty)*) -> $ret {
            if f.is_null() {
                return $default;
            }
            let mut guard = unsafe { &*f }.inner.lock().unwrap_or_else(|e| e.into_inner());
            let $f = &mut *guard;
            catch_unwind(AssertUnwindSafe(|| $body)).unwrap_or($default)
        }
    };
}

fetcher_fn!(vks_tfetch_fetcher_set_pipelined, |f, pipelined: i32| -> i32, 0, { f.set_pipelined(pipelined != 0); 1 });
fetcher_fn!(vks_tfetch_fetcher_add_waiter, |f, id: *const u8| -> i32, 0, { f.add_http_waiter(&unsafe { uuid_arg(id) }); 1 });
fetcher_fn!(vks_tfetch_fetcher_remove_waiter, |f, id: *const u8| -> i32, 0, { f.remove_http_waiter(&unsafe { uuid_arg(id) }); 1 });
fetcher_fn!(vks_tfetch_fetcher_is_waiter, |f, id: *const u8| -> i32, 0, f.is_http_waiter(&unsafe { uuid_arg(id) }) as i32);
fetcher_fn!(vks_tfetch_fetcher_waiters_count, |f| -> i32, 0, f.http_waiters_count());
fetcher_fn!(vks_tfetch_fetcher_cancel_waiters, |f| -> i32, 0, { f.cancel_http_waiters(); 1 });
fetcher_fn!(vks_tfetch_fetcher_semaphore, |f| -> i32, 0, f.http_semaphore);
fetcher_fn!(vks_tfetch_fetcher_total_http_requests, |f| -> u32, 0, f.total_http_requests);
fetcher_fn!(vks_tfetch_fetcher_update_state_stats, |f, reads: u32, writes: u32, waits: u32| -> i32, 0, { f.update_state_stats(reads, writes, waits); 1 });
fetcher_fn!(vks_tfetch_fetcher_get_state_stats, |f, reads: *mut u32, writes: *mut u32, waits: *mut u32| -> i32, 0, {
    unsafe {
        if !reads.is_null() { *reads = f.total_cache_read_count; }
        if !writes.is_null() { *writes = f.total_cache_write_count; }
        if !waits.is_null() { *waits = f.total_resource_wait_count; }
    }
    1
});
fetcher_fn!(vks_tfetch_fetcher_set_debug, |f, pause: i32, id: *const u8| -> i32, 0, {
    f.debug_pause = pause != 0;
    f.debug_id = unsafe { uuid_arg(id) };
    1
});
fetcher_fn!(vks_tfetch_fetcher_debug_count, |f| -> i32, 0, f.debug_count);

/// `releaseHttpWaiters`, first half: the number of waiters to release and
/// their ids (up to `cap`, 16 bytes each). Returns the number of ids
/// written; `needed` receives the count to release (0: nothing to do).
///
/// # Safety
/// `ids` must be writable for `cap * 16` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tfetch_fetcher_waiters_to_release(f: *mut VksTexFetcher, needed: *mut i32, ids: *mut u8, cap: usize) -> usize {
    if f.is_null() {
        return 0;
    }
    let fetcher_guard = unsafe { &*f }.inner.lock().unwrap_or_else(|e| e.into_inner());
    let fetcher = &*fetcher_guard;
    let (n, list) = fetcher.waiters_to_release();
    if !needed.is_null() {
        unsafe { *needed = n };
    }
    let count = list.len().min(cap);
    for (i, id) in list.iter().take(count).enumerate() {
        unsafe { std::ptr::copy_nonoverlapping(id.as_ptr(), ids.add(i * 16), 16) };
    }
    count
}

/// `releaseHttpWaiters`, second half: the release order (indices into
/// `priorities`) for `needed` slots.
///
/// # Safety
/// `priorities` must point to `n` floats; `order` must be writable for `n`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tfetch_release_order(priorities: *const f32, n: usize, needed: i32, order: *mut usize) {
    if priorities.is_null() || order.is_null() {
        return;
    }
    let p = unsafe { slice::from_raw_parts(priorities, n) };
    let o = Fetcher::release_order(p, needed);
    for (i, v) in o.iter().enumerate() {
        unsafe { *order.add(i) = *v };
    }
}

/// A worker (`LLTextureFetchWorker`'s state): `ftype` is the `FTType`,
/// `host_is_ok` whether a simulator host was given, `in_second_life` the
/// grid.
///
/// # Safety
/// `url` must point to `url_len` bytes; `id` to 16.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tfetch_worker_new(ftype: i32, url: *const c_char, url_len: usize, id: *const u8, host_is_ok: i32, priority: f32, in_second_life: i32) -> *mut VksTexFetchWorker {
    let url = unsafe { str_arg(url, url_len) }.to_owned();
    let id = unsafe { uuid_arg(id) };
    let r = catch_unwind(AssertUnwindSafe(|| Worker::new(FetchType::from_i32(ftype), &url, id, host_is_ok != 0, priority, in_second_life != 0)));
    match r {
        Ok(inner) => Box::into_raw(Box::new(VksTexFetchWorker { inner })),
        Err(_) => std::ptr::null_mut(),
    }
}

/// # Safety
/// `w` must come from `vks_tfetch_worker_new` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tfetch_worker_free(w: *mut VksTexFetchWorker) {
    if !w.is_null() {
        drop(unsafe { Box::from_raw(w) });
    }
}

macro_rules! worker_fn {
    ($name:ident, |$w:ident $(, $arg:ident : $ty:ty)*| -> $ret:ty, $default:expr, $body:expr) => {
        /// See the C header.
        ///
        /// # Safety
        /// `w` must come from `vks_tfetch_worker_new` and be live; the
        /// caller holds the worker's lock.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(w: *mut VksTexFetchWorker $(, $arg: $ty)*) -> $ret {
            if w.is_null() {
                return $default;
            }
            let $w = &mut unsafe { &mut *w }.inner;
            catch_unwind(AssertUnwindSafe(|| $body)).unwrap_or($default)
        }
    };
}

/// `doWork`: 1 when finished (or aborted), 0 when waiting.
///
/// # Safety
/// `w`, `f` and `host` must be valid; the caller holds the worker's lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tfetch_worker_do_work(w: *mut VksTexFetchWorker, f: *mut VksTexFetcher, host: *const VksTexFetchHost) -> i32 {
    if w.is_null() || f.is_null() || host.is_null() {
        return 1;
    }
    let worker = &mut unsafe { &mut *w }.inner;
    let mut fetcher_guard = unsafe { &*f }.inner.lock().unwrap_or_else(|e| e.into_inner());
    let fetcher = &mut *fetcher_guard;
    let mut h = CHost(unsafe { *host });
    catch_unwind(AssertUnwindSafe(|| worker.do_work(fetcher, &mut h) as i32)).unwrap_or(1)
}

worker_fn!(vks_tfetch_worker_state, |w| -> i32, 0, w.state as i32);
worker_fn!(vks_tfetch_worker_set_state, |w, state: i32| -> i32, 0, { w.set_state(State::from_i32(state)); 1 });
worker_fn!(vks_tfetch_worker_set_desired_discard, |w, discard: i32, size: i32, have_work: i32| -> i32, 0, w.set_desired_discard(discard, size, have_work != 0) as i32);
worker_fn!(vks_tfetch_worker_set_image_priority, |w, priority: f32| -> i32, 0, { w.set_image_priority(priority); 1 });
worker_fn!(vks_tfetch_worker_set_can_use_http, |w, can: i32| -> i32, 0, { w.can_use_http = can != 0; 1 });
worker_fn!(vks_tfetch_worker_set_needs_aux, |w, needs: i32| -> i32, 0, { w.needs_aux = needs != 0; 1 });
worker_fn!(vks_tfetch_worker_set_url, |w, url: *const c_char, url_len: usize| -> i32, 0, { w.url = unsafe { str_arg(url, url_len) }.to_owned(); 1 });
worker_fn!(vks_tfetch_worker_url, |w, buf: *mut c_char, cap: usize| -> usize, 0, unsafe { copy_out(w.url.as_bytes(), buf, cap) });
worker_fn!(vks_tfetch_worker_inc_active, |w| -> i32, 0, { w.active_count += 1; w.active_count });
worker_fn!(vks_tfetch_worker_end_work, |w| -> i32, 0, { w.end_work(); 1 });
worker_fn!(vks_tfetch_worker_reset_timers, |w| -> i32, 0, { w.reset_timers(); 1 });
worker_fn!(vks_tfetch_worker_requested_delta_secs, |w| -> f32, 0.0, w.requested_delta_secs());
worker_fn!(vks_tfetch_worker_fetch_delta_secs, |w| -> f32, 0.0, w.fetch_delta_secs());
worker_fn!(vks_tfetch_worker_data_progress, |w, formatted_present: i32, formatted_size: i32| -> f32, 0.0, {
    w.data_progress(if formatted_present != 0 { Some(formatted_size) } else { None })
});
worker_fn!(vks_tfetch_worker_requested_priority_for_state, |w| -> f32, 0.0, w.requested_priority_for_state());
worker_fn!(vks_tfetch_worker_cache_read_done, |w, success: i32, imagesize: i32, data_size: i32, codec: i32, is_local: i32| -> i32, 0, {
    w.cache_read_done(success != 0, imagesize, data_size, codec, is_local != 0);
    1
});
worker_fn!(vks_tfetch_worker_cache_write_done, |w, success: i32| -> i32, 0, { w.cache_write_done(success != 0); 1 });

/// The worker's fields the viewer's accessors read (`getFetchState`,
/// `getLastFetchState`, `getRequestFinished`, `deleteOK`, the debugger).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VksTexFetchWorkerInfo {
    pub state: i32,
    pub write_to_cache_state: i32,
    pub image_priority: f32,
    pub requested_priority: f32,
    pub desired_discard: i32,
    pub requested_discard: i32,
    pub loaded_discard: i32,
    pub decoded_discard: i32,
    pub desired_size: i32,
    pub requested_size: i32,
    pub requested_offset: i32,
    pub file_size: i32,
    pub cached_size: i32,
    pub loaded: i32,
    pub decoded: i32,
    pub written: i32,
    pub needs_aux: i32,
    pub have_all_data: i32,
    pub in_local_cache: i32,
    pub in_cache: i32,
    pub can_use_http: i32,
    pub can_use_net: i32,
    pub retry_attempt: i32,
    pub active_count: i32,
    pub get_status_ok: i32,
    pub get_status_type: i32,
    pub http_active: i32,
    pub http_has_resource: i32,
    pub http_buffer_present: i32,
    pub cache_read_pending: i32,
    pub cache_write_pending: i32,
    pub decode_handle: u32,
    pub image_codec: i32,
    pub cache_read_count: u32,
    pub cache_write_count: u32,
    pub resource_wait_count: u32,
    pub cache_read_time: f32,
    pub decode_time: f32,
    pub cache_write_time: f32,
    pub fetch_time: f32,
    pub skipped_states_time: f32,
    pub state_timers: [f32; 15],
    pub sent_request: i32,
    pub sim_requested_discard: i32,
    pub total_packets: i32,
    pub last_packet: i32,
}

worker_fn!(vks_tfetch_worker_info, |w, out: *mut VksTexFetchWorkerInfo| -> i32, 0, {
    if out.is_null() {
        return 0;
    }
    let info = VksTexFetchWorkerInfo {
        state: w.state as i32,
        write_to_cache_state: w.write_to_cache_state as i32,
        image_priority: w.image_priority,
        requested_priority: w.requested_priority,
        desired_discard: w.desired_discard,
        requested_discard: w.requested_discard,
        loaded_discard: w.loaded_discard,
        decoded_discard: w.decoded_discard,
        desired_size: w.desired_size,
        requested_size: w.requested_size,
        requested_offset: w.requested_offset,
        file_size: w.file_size,
        cached_size: w.cached_size,
        loaded: w.loaded as i32,
        decoded: w.decoded as i32,
        written: w.written as i32,
        needs_aux: w.needs_aux as i32,
        have_all_data: w.have_all_data as i32,
        in_local_cache: w.in_local_cache as i32,
        in_cache: w.in_cache as i32,
        can_use_http: w.can_use_http as i32,
        can_use_net: w.can_use_net as i32,
        retry_attempt: w.retry_attempt,
        active_count: w.active_count,
        get_status_ok: w.get_status_ok as i32,
        get_status_type: w.get_status_type,
        http_active: w.http_active as i32,
        http_has_resource: w.http_has_resource as i32,
        http_buffer_present: w.http_buffer_present as i32,
        cache_read_pending: w.cache_read_pending as i32,
        cache_write_pending: w.cache_write_pending as i32,
        decode_handle: w.decode_handle,
        image_codec: w.image_codec,
        cache_read_count: w.cache_read_count,
        cache_write_count: w.cache_write_count,
        resource_wait_count: w.resource_wait_count,
        cache_read_time: w.cache_read_time,
        decode_time: w.decode_time,
        cache_write_time: w.cache_write_time,
        fetch_time: w.fetch_time,
        skipped_states_time: w.skipped_states_time,
        state_timers: w.state_timers,
        sent_request: w.sent_request as i32,
        sim_requested_discard: w.sim_requested_discard,
        total_packets: packets_total(w),
        last_packet: packets_last(w),
    };
    unsafe { *out = info };
    1
});

worker_fn!(vks_tfetch_worker_get_reason, |w, buf: *mut c_char, cap: usize| -> usize, 0, unsafe { copy_out(w.get_reason.as_bytes(), buf, cap) });

#[cfg(feature = "opensim")]
fn packets_total(w: &Worker) -> i32 {
    w.total_packets as i32
}
#[cfg(not(feature = "opensim"))]
fn packets_total(_w: &Worker) -> i32 {
    0
}
#[cfg(feature = "opensim")]
fn packets_last(w: &Worker) -> i32 {
    w.last_packet
}
#[cfg(not(feature = "opensim"))]
fn packets_last(_w: &Worker) -> i32 {
    -1
}

// The UDP (simulator) texture path: live with the `opensim` feature, inert
// without it (the C++ compiles its callers out as well).
#[cfg(feature = "opensim")]
worker_fn!(vks_tfetch_worker_insert_packet, |w, index: i32, size: u32| -> i32, 0, w.insert_packet(index, size) as i32);
#[cfg(feature = "opensim")]
worker_fn!(vks_tfetch_worker_setup_packet_data, |w, data_size: i32| -> i32, 0, match w.setup_packet_data(data_size) {
    vkstorm_assets::fetch::SetupPackets::Ok => 1,
    vkstorm_assets::fetch::SetupPackets::BadSize => 0,
    vkstorm_assets::fetch::SetupPackets::Refetch => -1,
});
#[cfg(feature = "opensim")]
worker_fn!(vks_tfetch_worker_receive_header, |w, codec: i32, total_packets: u16, file_size: i32| -> i32, 0, {
    w.receive_header(codec, total_packets, file_size);
    1
});
#[cfg(feature = "opensim")]
worker_fn!(vks_tfetch_worker_mark_sent_sim, |w| -> i32, 0, {
    w.mark_sent_sim();
    1
});
/// Inert without the `opensim` feature.
#[cfg(not(feature = "opensim"))]
#[unsafe(no_mangle)]
pub extern "C" fn vks_tfetch_worker_insert_packet(_w: *mut VksTexFetchWorker, _index: i32, _size: u32) -> i32 {
    0
}
/// Inert without the `opensim` feature.
#[cfg(not(feature = "opensim"))]
#[unsafe(no_mangle)]
pub extern "C" fn vks_tfetch_worker_setup_packet_data(_w: *mut VksTexFetchWorker, _data_size: i32) -> i32 {
    1
}
/// Inert without the `opensim` feature.
#[cfg(not(feature = "opensim"))]
#[unsafe(no_mangle)]
pub extern "C" fn vks_tfetch_worker_receive_header(_w: *mut VksTexFetchWorker, _codec: i32, _total_packets: u16, _file_size: i32) -> i32 {
    0
}
/// Inert without the `opensim` feature.
#[cfg(not(feature = "opensim"))]
#[unsafe(no_mangle)]
pub extern "C" fn vks_tfetch_worker_mark_sent_sim(_w: *mut VksTexFetchWorker) -> i32 {
    0
}

/// `callbackDecoded`: 1 when the callback was accepted (the host keeps the
/// raw images), 0 when ignored.
///
/// # Safety
/// `w` and `host` must be valid; the caller holds the worker's lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tfetch_worker_decode_done(w: *mut VksTexFetchWorker, host: *const VksTexFetchHost, success: i32, formatted_discard: i32, decode_id: u32) -> i32 {
    if w.is_null() || host.is_null() {
        return 0;
    }
    let worker = &mut unsafe { &mut *w }.inner;
    let mut h = CHost(unsafe { *host });
    catch_unwind(AssertUnwindSafe(|| worker.decode_done(success != 0, formatted_discard, decode_id, &mut h) as i32)).unwrap_or(0)
}

/// `onCompleted` and `callbackHttpGet`: returns the data size for the HTTP
/// queue accounting, or -1 when the request is being retried (the host
/// then removes it from the HTTP queue with 0 bytes and returns).
///
/// # Safety
/// All pointers must be valid; the caller holds the worker's lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tfetch_worker_http_completed(
    w: *mut VksTexFetchWorker,
    f: *mut VksTexFetcher,
    host: *const VksTexFetchHost,
    status_ok: i32,
    status_type: i32,
    reason: *const c_char,
    reason_len: usize,
    partial: i32,
    body_size: i32,
    has_range: i32,
    range_offset: u32,
    range_length: u32,
    range_full: u32,
    has_retry_after: i32,
    retry_after: f32,
) -> i64 {
    if w.is_null() || f.is_null() || host.is_null() {
        return -1;
    }
    let worker = &mut unsafe { &mut *w }.inner;
    let mut fetcher_guard = unsafe { &*f }.inner.lock().unwrap_or_else(|e| e.into_inner());
    let fetcher = &mut *fetcher_guard;
    let mut h = CHost(unsafe { *host });
    let reason = unsafe { str_arg(reason, reason_len) }.to_owned();
    let range = if has_range != 0 { Some((range_offset, range_length, range_full)) } else { None };
    let ra = if has_retry_after != 0 { Some(retry_after) } else { None };
    catch_unwind(AssertUnwindSafe(|| {
        match worker.http_completed(fetcher, &mut h, status_ok != 0, status_type, &reason, partial != 0, body_size, range, ra) {
            Some(n) => n as i64,
            None => -1,
        }
    }))
    .unwrap_or(-1)
}

/// `acquireHttpSemaphore` through `try_release` (0 released, 1 not
/// waiting, 2 no slot).
///
/// # Safety
/// `w` and `f` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tfetch_worker_try_release(w: *mut VksTexFetchWorker, f: *mut VksTexFetcher) -> i32 {
    if w.is_null() || f.is_null() {
        return 1;
    }
    let worker = &mut unsafe { &mut *w }.inner;
    let mut fetcher_guard = unsafe { &*f }.inner.lock().unwrap_or_else(|e| e.into_inner());
    let fetcher = &mut *fetcher_guard;
    match catch_unwind(AssertUnwindSafe(|| worker.try_release(fetcher))) {
        Ok(Release::Released) => 0,
        Ok(Release::NotWaiting) => 1,
        _ => 2,
    }
}

/// The destructor's accounting: releases the semaphore, the waiter entry
/// and adds the counts; returns whether an HTTP request is active.
///
/// # Safety
/// `w` and `f` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tfetch_worker_on_destroy(w: *mut VksTexFetchWorker, f: *mut VksTexFetcher) -> i32 {
    if w.is_null() || f.is_null() {
        return 0;
    }
    let worker = &mut unsafe { &mut *w }.inner;
    let mut fetcher_guard = unsafe { &*f }.inner.lock().unwrap_or_else(|e| e.into_inner());
    let fetcher = &mut *fetcher_guard;
    catch_unwind(AssertUnwindSafe(|| worker.on_destroy(fetcher) as i32)).unwrap_or(0)
}

/// `deleteOK`'s state part.
///
/// # Safety
/// `w` and `f` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tfetch_worker_delete_ok_state(w: *mut VksTexFetchWorker, f: *mut VksTexFetcher, have_work: i32) -> i32 {
    if w.is_null() || f.is_null() {
        return 1;
    }
    let worker = &unsafe { &*w }.inner;
    let fetcher_guard = unsafe { &*f }.inner.lock().unwrap_or_else(|e| e.into_inner());
    let fetcher = &*fetcher_guard;
    catch_unwind(AssertUnwindSafe(|| worker.delete_ok_state(fetcher, have_work != 0) as i32)).unwrap_or(1)
}

/// `createRequest`'s desired size and discard.
///
/// # Safety
/// Output pointers must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tfetch_desired_size(ftype: i32, url_non_empty: i32, url_is_non_j2c: i32, w: i32, h: i32, c: i32, desired_discard: i32, size_out: *mut i32, discard_out: *mut i32) {
    let (size, discard) = plan::desired_size(FetchType::from_i32(ftype), url_non_empty != 0, url_is_non_j2c != 0, w, h, c, desired_discard);
    unsafe {
        if !size_out.is_null() { *size_out = size; }
        if !discard_out.is_null() { *discard_out = discard; }
    }
}

/// `createRequest`'s "already done" check.
#[unsafe(no_mangle)]
pub extern "C" fn vks_tfetch_already_done(worker_done: i32, worker_desired_size: i32, worker_desired_discard: i32, desired_size: i32, desired_discard: i32) -> i32 {
    plan::already_done(worker_done != 0, worker_desired_size, worker_desired_discard, desired_size, desired_discard) as i32
}

/// `getStateString`.
///
/// # Safety
/// `buf` must be writable for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tfetch_state_string(state: i32, buf: *mut c_char, cap: usize) -> usize {
    let s = State::string_for(state);
    unsafe { copy_out(s.as_bytes(), buf, cap) }
}

/// `LLImageJ2C::calcDataSizeJ2C`.
#[unsafe(no_mangle)]
pub extern "C" fn vks_j2c_calc_data_size(w: i32, h: i32, comp: i32, discard_level: i32, rate: f32) -> i32 {
    budget::calc_data_size_j2c(w, h, comp, discard_level, rate)
}

/// `LLImageJ2C::calcDiscardLevelBytes` with the reverse byte-range
/// percentage.
#[unsafe(no_mangle)]
pub extern "C" fn vks_j2c_calc_discard_level_bytes(w: i32, h: i32, comp: i32, bytes: i32, reverse_percent: i32) -> i32 {
    budget::calc_discard_level_bytes(w, h, comp, bytes, reverse_percent)
}

// ---------------------------------------------------------------------------
// The adaptive retry policy

pub struct VksRetryPolicy {
    inner: AdaptiveRetryPolicy,
}

#[unsafe(no_mangle)]
pub extern "C" fn vks_retry_policy_new(min_delay: f32, max_delay: f32, backoff_factor: f32, max_retries: u32, retry_on_4xx: i32) -> *mut VksRetryPolicy {
    Box::into_raw(Box::new(VksRetryPolicy { inner: AdaptiveRetryPolicy::new(min_delay, max_delay, backoff_factor, max_retries, retry_on_4xx != 0) }))
}

/// # Safety
/// `p` must come from `vks_retry_policy_new` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_retry_policy_free(p: *mut VksRetryPolicy) {
    if !p.is_null() {
        drop(unsafe { Box::from_raw(p) });
    }
}

macro_rules! retry_fn {
    ($name:ident, |$p:ident $(, $arg:ident : $ty:ty)*| -> $ret:ty, $default:expr, $body:expr) => {
        /// See the C header.
        ///
        /// # Safety
        /// `p` must come from `vks_retry_policy_new` and be live.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(p: *mut VksRetryPolicy $(, $arg: $ty)*) -> $ret {
            if p.is_null() {
                return $default;
            }
            let $p = &mut unsafe { &mut *p }.inner;
            catch_unwind(AssertUnwindSafe(|| $body)).unwrap_or($default)
        }
    };
}

retry_fn!(vks_retry_policy_reset, |p| -> i32, 0, { p.reset(); 1 });
retry_fn!(vks_retry_policy_on_success, |p| -> i32, 0, { p.on_success(); 1 });
retry_fn!(vks_retry_policy_on_failure, |p, status: i32, has_retry_after: i32, retry_after: f32| -> i32, 0, {
    p.on_failure(status, if has_retry_after != 0 { Some(retry_after) } else { None });
    1
});
retry_fn!(vks_retry_policy_should_retry, |p, seconds: *mut f32| -> i32, 0, {
    let (ok, secs) = p.should_retry();
    if !seconds.is_null() {
        unsafe { *seconds = secs };
    }
    ok as i32
});
retry_fn!(vks_retry_policy_retry_count, |p| -> u32, 0, p.retry_count());

// ---------------------------------------------------------------------------
// The asset disk cache

pub struct VksDiskCache {
    inner: Mutex<DiskCache>,
}

/// `LLDiskCache`'s constructor (without the static prepopulation, which
/// the host drives file by file).
///
/// # Safety
/// `dir` and `delim` must point to their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_diskcache_new(dir: *const c_char, dir_len: usize, delim: *const c_char, delim_len: usize, max_size: u64, debug: i32, high: f32, low: f32) -> *mut VksDiskCache {
    let d = unsafe { str_arg(dir, dir_len) }.to_owned();
    let dl = unsafe { str_arg(delim, delim_len) }.to_owned();
    match catch_unwind(AssertUnwindSafe(|| DiskCache::new(&d, &dl, max_size, debug != 0, high, low))) {
        Ok(inner) => Box::into_raw(Box::new(VksDiskCache { inner: Mutex::new(inner) })),
        Err(_) => std::ptr::null_mut(),
    }
}

/// # Safety
/// `c` must come from `vks_diskcache_new` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_diskcache_free(c: *mut VksDiskCache) {
    if !c.is_null() {
        drop(unsafe { Box::from_raw(c) });
    }
}

macro_rules! diskcache_fn {
    ($name:ident, |$c:ident $(, $arg:ident : $ty:ty)*| -> $ret:ty, $default:expr, $body:expr) => {
        /// See the C header.
        ///
        /// # Safety
        /// `c` must come from `vks_diskcache_new` and be live; pointer
        /// arguments must be valid for their lengths.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(c: *mut VksDiskCache $(, $arg: $ty)*) -> $ret {
            if c.is_null() {
                return $default;
            }
            let mut guard = unsafe { &*c }.inner.lock().unwrap_or_else(|e| e.into_inner());
            let $c = &mut *guard;
            catch_unwind(AssertUnwindSafe(|| $body)).unwrap_or($default)
        }
    };
}

/// `purge`, on LLPurgeDiskCacheThread: works on a copy of the cache's
/// configuration so the main thread's calls do not wait for the file
/// system work, then records the scan result.
///
/// # Safety
/// `c` must come from `vks_diskcache_new` and be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_diskcache_purge(c: *mut VksDiskCache) -> i32 {
    if c.is_null() {
        return 0;
    }
    let cache = &unsafe { &*c }.inner;
    let mut work = cache.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if catch_unwind(AssertUnwindSafe(|| work.purge())).is_err() {
        return 0;
    }
    cache.lock().unwrap_or_else(|e| e.into_inner()).absorb_scan(&work);
    1
}
/// `purge` with upstream d527adf68c's shutdown early-out: `is_running` is
/// `LLApp::isRunning()` (returns non-zero while the application runs; null
/// means always running), asked during the scan, the delete loop and the
/// debug listing. Returns 1 when the purge ran to the end, 2 when it
/// stopped because the application is shutting down (the scan result is
/// then not recorded, as the C++ returns before `updateCacheSize`), and 0
/// on failure.
///
/// # Safety
/// `c` must come from `vks_diskcache_new` and be live; `is_running`, when
/// not null, must be callable from the purge thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_diskcache_purge_while(c: *mut VksDiskCache, is_running: Option<extern "C" fn() -> i32>) -> i32 {
    if c.is_null() {
        return 0;
    }
    let cache = &unsafe { &*c }.inner;
    let mut work = cache.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut running = || is_running.map(|f| f() != 0).unwrap_or(true);
    let Ok(report) = catch_unwind(AssertUnwindSafe(|| work.purge_while(&mut running))) else {
        return 0;
    };
    if report.interrupted {
        return 2;
    }
    cache.lock().unwrap_or_else(|e| e.into_inner()).absorb_scan(&work);
    1
}
diskcache_fn!(vks_diskcache_clear, |c| -> i32, 0, { c.clear_cache(); 1 });
diskcache_fn!(vks_diskcache_set_max_size, |c, size: u64| -> i32, 0, { c.set_max_size_bytes(size); 1 });
diskcache_fn!(vks_diskcache_set_high_water, |c, pct: f32| -> i32, 0, { c.set_high_water_percentage(pct); 1 });
diskcache_fn!(vks_diskcache_set_low_water, |c, pct: f32| -> i32, 0, { c.set_low_water_percentage(pct); 1 });
diskcache_fn!(vks_diskcache_dir_file_size, |c, force: i32| -> u64, 0, c.dir_file_size(force != 0));
diskcache_fn!(vks_diskcache_info, |c, buf: *mut c_char, cap: usize| -> usize, 0, {
    let s = c.cache_info();
    unsafe { copy_out(s.as_bytes(), buf, cap) }
});
diskcache_fn!(vks_diskcache_skip_list_clear, |c| -> i32, 0, { c.skip_list.clear(); 1 });
diskcache_fn!(vks_diskcache_prepopulate_file, |c, from_folder: *const c_char, from_folder_len: usize, from_file: *const c_char, from_file_len: usize| -> i32, 0, {
    let folder = unsafe { str_arg(from_folder, from_folder_len) }.to_owned();
    let file = unsafe { str_arg(from_file, from_file_len) }.to_owned();
    c.prepopulate(&[(folder, vec![file])]);
    1
});
diskcache_fn!(vks_diskcache_filepath, |c, id: *const u8, buf: *mut c_char, cap: usize| -> usize, 0, {
    let s = c.filepath(&unsafe { uuid_arg(id) });
    unsafe { copy_out(s.as_bytes(), buf, cap) }
});

/// `metaDataToFilepath` without a cache object.
///
/// # Safety
/// `dir` and `delim` must point to their lengths; `buf` to `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_diskcache_metadata_to_filepath(dir: *const c_char, dir_len: usize, delim: *const c_char, delim_len: usize, id: *const u8, buf: *mut c_char, cap: usize) -> usize {
    let d = unsafe { str_arg(dir, dir_len) };
    let dl = unsafe { str_arg(delim, delim_len) };
    let s = diskcache::metadata_to_filepath(d, dl, &unsafe { uuid_arg(id) });
    unsafe { copy_out(s.as_bytes(), buf, cap) }
}

/// `removeOldVFSFiles`.
///
/// # Safety
/// `path` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_diskcache_remove_old_vfs_files(path: *const c_char, len: usize) {
    let p = unsafe { str_arg(path, len) };
    let _ = catch_unwind(AssertUnwindSafe(|| DiskCache::remove_old_vfs_files(p)));
}

/// Fixes the prepopulate state that `vks_diskcache_prepopulate_file`
/// accumulates: `prepopulate` clears the skip list per call, so the host
/// clears it once (`vks_diskcache_skip_list_clear`) and adds each file
/// with this call, which keeps the list.
///
/// # Safety
/// As `vks_diskcache_prepopulate_file`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_diskcache_add_static(c: *mut VksDiskCache, from_folder: *const c_char, from_folder_len: usize, from_file: *const c_char, from_file_len: usize) -> i32 {
    if c.is_null() {
        return 0;
    }
    let mut guard = unsafe { &*c }.inner.lock().unwrap_or_else(|e| e.into_inner());
    let cache = &mut *guard;
    let folder = unsafe { str_arg(from_folder, from_folder_len) }.to_owned();
    let file = unsafe { str_arg(from_file, from_file_len) }.to_owned();
    catch_unwind(AssertUnwindSafe(|| {
        let mut keep = std::mem::take(&mut cache.skip_list);
        cache.prepopulate(&[(folder, vec![file])]);
        for s in cache.skip_list.drain(..) {
            if !keep.contains(&s) {
                keep.push(s);
            }
        }
        cache.skip_list = keep;
        1
    }))
    .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Mesh assets

/// `LLMeshHeader`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksMeshHeader {
    pub version: i32,
    pub skin_offset: i32,
    pub skin_size: i32,
    pub skin_in_cache: i32,
    pub physics_convex_offset: i32,
    pub physics_convex_size: i32,
    pub physics_convex_in_cache: i32,
    pub physics_mesh_offset: i32,
    pub physics_mesh_size: i32,
    pub physics_mesh_in_cache: i32,
    pub lod_offset: [i32; 4],
    pub lod_size: [i32; 4],
    pub lod_in_cache: [i32; 4],
    pub header_size: i32,
    pub not_found: i32,
    pub creator_id: [u8; 16],
}

fn header_out(h: &MeshHeader) -> VksMeshHeader {
    VksMeshHeader {
        version: h.version,
        skin_offset: h.skin_offset,
        skin_size: h.skin_size,
        skin_in_cache: h.skin_in_cache as i32,
        physics_convex_offset: h.physics_convex_offset,
        physics_convex_size: h.physics_convex_size,
        physics_convex_in_cache: h.physics_convex_in_cache as i32,
        physics_mesh_offset: h.physics_mesh_offset,
        physics_mesh_size: h.physics_mesh_size,
        physics_mesh_in_cache: h.physics_mesh_in_cache as i32,
        lod_offset: h.lod_offset,
        lod_size: h.lod_size,
        lod_in_cache: [h.lod_in_cache[0] as i32, h.lod_in_cache[1] as i32, h.lod_in_cache[2] as i32, h.lod_in_cache[3] as i32],
        header_size: h.header_size,
        not_found: h.not_found as i32,
        creator_id: h.creator_id,
    }
}

fn header_in(h: &VksMeshHeader) -> MeshHeader {
    MeshHeader {
        version: h.version,
        skin_offset: h.skin_offset,
        skin_size: h.skin_size,
        skin_in_cache: h.skin_in_cache != 0,
        physics_convex_offset: h.physics_convex_offset,
        physics_convex_size: h.physics_convex_size,
        physics_convex_in_cache: h.physics_convex_in_cache != 0,
        physics_mesh_offset: h.physics_mesh_offset,
        physics_mesh_size: h.physics_mesh_size,
        physics_mesh_in_cache: h.physics_mesh_in_cache != 0,
        lod_offset: h.lod_offset,
        lod_size: h.lod_size,
        lod_in_cache: [h.lod_in_cache[0] != 0, h.lod_in_cache[1] != 0, h.lod_in_cache[2] != 0, h.lod_in_cache[3] != 0],
        header_size: h.header_size,
        not_found: h.not_found != 0,
        creator_id: h.creator_id,
    }
}

/// What `headerReceived` derives from a header fetch.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksMeshHeaderReceipt {
    pub header: VksMeshHeader,
    pub combined_header_size: i64,
    pub data_size: i32,
    pub skin_in_data: i32,
    pub skin_offset: usize,
    pub skin_size: usize,
    pub lod_in_data: [i32; 4],
    pub lod_offset: [usize; 4],
    pub lod_size: [usize; 4],
}

/// `headerReceived`: 0 (`MESH_OK`) with the receipt, else the
/// `EMeshProcessingResult` (4 parse failure, 5 invalid).
///
/// # Safety
/// `data` must point to `len` bytes; `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_header_received(data: *const u8, len: usize, flags: u32, out: *mut VksMeshHeaderReceipt) -> i32 {
    if out.is_null() {
        return 6;
    }
    let d = unsafe { bytes(data, len) };
    match catch_unwind(AssertUnwindSafe(|| mesh::header_received(d, flags))) {
        Ok(Ok(r)) => {
            let mut o = VksMeshHeaderReceipt {
                header: header_out(&r.header),
                combined_header_size: r.combined_header_size,
                data_size: r.data_size,
                skin_in_data: r.skin_in_data.is_some() as i32,
                skin_offset: r.skin_in_data.map(|x| x.0).unwrap_or(0),
                skin_size: r.skin_in_data.map(|x| x.1).unwrap_or(0),
                lod_in_data: [0; 4],
                lod_offset: [0; 4],
                lod_size: [0; 4],
            };
            for i in 0..NUM_LODS {
                if let Some((off, size)) = r.lods_in_data[i] {
                    o.lod_in_data[i] = 1;
                    o.lod_offset[i] = off;
                    o.lod_size[i] = size;
                }
            }
            unsafe { *out = o };
            0
        }
        Ok(Err(e)) => e as i32,
        Err(_) => 6,
    }
}

/// `LLMeshHeader::getFlags`.
///
/// # Safety
/// `h` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_header_flags(h: *const VksMeshHeader) -> u32 {
    if h.is_null() {
        return 0;
    }
    header_in(unsafe { &*h }).flags()
}

/// `LLMeshHeader::setFromFlags`.
///
/// # Safety
/// `h` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_header_set_from_flags(h: *mut VksMeshHeader, flags: u32) {
    if h.is_null() {
        return;
    }
    let mut m = header_in(unsafe { &*h });
    m.set_from_flags(flags);
    unsafe { *h = header_out(&m) };
}

/// `LLMeshRepository::getActualMeshLOD(header, lod)`; marks the header 404
/// when no LOD exists.
///
/// # Safety
/// `h` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_header_actual_lod(h: *mut VksMeshHeader, lod: i32) -> i32 {
    if h.is_null() {
        return -1;
    }
    let mut m = header_in(unsafe { &*h });
    let r = m.actual_lod(lod);
    unsafe { *h = header_out(&m) };
    r
}

/// The cache file size the header handler reserves.
///
/// # Safety
/// `h` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_header_cache_file_size(h: *const VksMeshHeader) -> i32 {
    if h.is_null() {
        return 0;
    }
    header_in(unsafe { &*h }).cache_file_size()
}

/// `write_preamble`'s 12 bytes.
///
/// # Safety
/// `out` must be writable for 12 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_preamble_encode(header_bytes: i32, flags: u32, out: *mut u8) {
    if out.is_null() {
        return;
    }
    let p = mesh::header::encode_preamble(header_bytes, flags);
    unsafe { std::ptr::copy_nonoverlapping(p.as_ptr(), out, p.len()) };
}

/// The preamble read back: 1 when the buffer holds one.
///
/// # Safety
/// `buf` must point to `len` bytes; outputs must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_preamble_decode(buf: *const u8, len: usize, version: *mut u32, header_size: *mut i32, flags: *mut u32) -> i32 {
    let b = unsafe { bytes(buf, len) };
    match mesh::header::decode_preamble(b) {
        Some((v, h, f)) => {
            unsafe {
                if !version.is_null() { *version = v; }
                if !header_size.is_null() { *header_size = h; }
                if !flags.is_null() { *flags = f; }
            }
            1
        }
        None => 0,
    }
}

/// Whether a cached block's first kilobyte is all zeros (never written).
///
/// # Safety
/// `buf` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_looks_unwritten(buf: *const u8, len: usize) -> i32 {
    mesh::header::looks_unwritten(unsafe { bytes(buf, len) }) as i32
}

/// `constructUrl`: writes the URL (empty when no capability) and the
/// legacy version; returns the URL's length.
///
/// # Safety
/// Strings must point to their lengths; `buf` to `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_construct_url(
    mesh_cap: *const c_char,
    mesh_cap_len: usize,
    legacy1: *const c_char,
    legacy1_len: usize,
    legacy2: *const c_char,
    legacy2_len: usize,
    legacy_pref_version: i32,
    id: *const u8,
    buf: *mut c_char,
    cap: usize,
    legacy_version: *mut i32,
) -> usize {
    let u = mplan::construct_url(
        unsafe { str_arg(mesh_cap, mesh_cap_len) },
        unsafe { str_arg(legacy1, legacy1_len) },
        unsafe { str_arg(legacy2, legacy2_len) },
        legacy_pref_version,
        &unsafe { uuid_arg(id) },
    );
    if !legacy_version.is_null() {
        unsafe { *legacy_version = u.legacy_version };
    }
    unsafe { copy_out(u.url.as_bytes(), buf, cap) }
}

/// The policy class of a mesh byte range: 0 mesh, 1 legacy, 2 large.
#[unsafe(no_mangle)]
pub extern "C" fn vks_mesh_policy_for(len: u32, legacy_cap_version: i32) -> i32 {
    match mplan::policy_for(len, legacy_cap_version) {
        mplan::PolicyChoice::Mesh => 0,
        mplan::PolicyChoice::Legacy => 1,
        mplan::PolicyChoice::Large => 2,
    }
}

/// A part's byte range: `kind` 0..3 the render LODs, 4 the skin, 5 the
/// convex decomposition, 6 the physics mesh. Returns 1 with the range, 0
/// when the header has no such part.
///
/// # Safety
/// `h` and the outputs must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_part_request(h: *const VksMeshHeader, kind: i32, offset: *mut i32, size: *mut i32, in_cache: *mut i32, disk_offset: *mut i32) -> i32 {
    if h.is_null() {
        return 0;
    }
    let m = header_in(unsafe { &*h });
    let r = match kind {
        0..=3 => mplan::lod_request(&m, kind as usize),
        4 => mplan::skin_request(&m),
        5 => mplan::decomposition_request(&m),
        6 => mplan::physics_shape_request(&m),
        _ => None,
    };
    match r {
        Some(r) => {
            unsafe {
                if !offset.is_null() { *offset = r.offset; }
                if !size.is_null() { *size = r.size; }
                if !in_cache.is_null() { *in_cache = r.in_cache as i32; }
                if !disk_offset.is_null() { *disk_offset = r.disk_offset; }
            }
            1
        }
        None => 0,
    }
}

/// The HTTP response overlap check: 1 with the body offset of the
/// requested bytes, 0 when the response does not cover the request.
///
/// # Safety
/// `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_response_body_offset(request_offset: u32, response_offset: u32, data_size: usize, out: *mut usize) -> i32 {
    match mplan::response_body_offset(request_offset, response_offset, data_size) {
        Some(o) => {
            if !out.is_null() {
                unsafe { *out = o };
            }
            1
        }
        None => 0,
    }
}

/// Unpacked faces of one LOD.
pub struct VksMeshFaces {
    faces: Vec<Face>,
}

/// `unpackVolumeFaces`: null on failure with the reason in `err` (1 inflate
/// or parse failure, 2 no faces, 3 a buffer shorter than its vertex count).
///
/// # Safety
/// `data` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_unpack(data: *const u8, len: usize, sculpt_type: u8, err: *mut i32) -> *mut VksMeshFaces {
    let d = unsafe { bytes(data, len) };
    match catch_unwind(AssertUnwindSafe(|| mesh::unpack_volume_faces(d, sculpt_type))) {
        Ok(Ok(faces)) => Box::into_raw(Box::new(VksMeshFaces { faces })),
        Ok(Err(e)) => {
            if !err.is_null() {
                unsafe {
                    *err = match e {
                        mesh::UnpackError::Zip(_) => 1,
                        mesh::UnpackError::NoFaces => 2,
                        mesh::UnpackError::ShortBuffer(_) => 3,
                    }
                };
            }
            std::ptr::null_mut()
        }
        Err(_) => {
            if !err.is_null() {
                unsafe { *err = 4 };
            }
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `f` must come from `vks_mesh_unpack` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_faces_free(f: *mut VksMeshFaces) {
    if !f.is_null() {
        drop(unsafe { Box::from_raw(f) });
    }
}

/// # Safety
/// `f` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_faces_count(f: *const VksMeshFaces) -> usize {
    if f.is_null() { 0 } else { unsafe { &*f }.faces.len() }
}

/// One unpacked face: counts, the buffers (valid until the faces are
/// freed) and the extents. `texcoords` holds `num_vertices` rounded up to
/// an even count of pairs.
#[repr(C)]
pub struct VksMeshFace {
    pub num_vertices: i32,
    pub num_indices: i32,
    pub has_weights: i32,
    pub incomplete: i32,
    pub positions: *const f32,
    pub normals: *const f32,
    pub texcoords: *const f32,
    pub texcoord_count: usize,
    pub indices: *const u16,
    pub weights: *const f32,
    pub extents: [f32; 8],
    pub tc_extents: [f32; 4],
    pub normalized_scale: [f32; 3],
}

/// # Safety
/// `f` must be live; `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_face(f: *const VksMeshFaces, i: usize, out: *mut VksMeshFace) -> i32 {
    if f.is_null() || out.is_null() {
        return 0;
    }
    let faces = &unsafe { &*f }.faces;
    let Some(face) = faces.get(i) else { return 0 };
    let o = VksMeshFace {
        num_vertices: face.num_vertices,
        num_indices: face.num_indices,
        has_weights: face.weights.is_some() as i32,
        incomplete: face.incomplete as i32,
        positions: face.positions.as_ptr() as *const f32,
        normals: face.normals.as_ptr() as *const f32,
        texcoords: face.texcoords.as_ptr() as *const f32,
        texcoord_count: face.texcoords.len(),
        indices: face.indices.as_ptr(),
        weights: face.weights.as_ref().map(|w| w.as_ptr() as *const f32).unwrap_or(std::ptr::null()),
        extents: [
            face.extents[0][0], face.extents[0][1], face.extents[0][2], face.extents[0][3],
            face.extents[1][0], face.extents[1][1], face.extents[1][2], face.extents[1][3],
        ],
        tc_extents: [face.tc_extents[0][0], face.tc_extents[0][1], face.tc_extents[1][0], face.tc_extents[1][1]],
        normalized_scale: face.normalized_scale,
    };
    unsafe { *out = o };
    1
}

/// A parsed skin block.
pub struct VksMeshSkin {
    inner: SkinInfo,
}

/// `LLMeshSkinInfo::fromZippedData`: null when the block does not inflate
/// or parse (an empty block parses to the default skin info).
///
/// # Safety
/// `data` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_skin_parse(data: *const u8, len: usize) -> *mut VksMeshSkin {
    let d = unsafe { bytes(data, len) };
    match catch_unwind(AssertUnwindSafe(|| SkinInfo::from_zipped(d))) {
        Ok(Ok(inner)) => Box::into_raw(Box::new(VksMeshSkin { inner })),
        _ => std::ptr::null_mut(),
    }
}

/// # Safety
/// `s` must come from `vks_mesh_skin_parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_skin_free(s: *mut VksMeshSkin) {
    if !s.is_null() {
        drop(unsafe { Box::from_raw(s) });
    }
}

/// The skin's scalar fields and counts.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VksMeshSkinInfo {
    pub joint_count: usize,
    pub inv_bind_count: usize,
    pub alt_bind_count: usize,
    pub bind_pose_count: usize,
    pub pelvis_offset: f32,
    pub lock_scale_if_joint_position: i32,
    pub bind_shape_matrix: [f32; 16],
}

/// # Safety
/// `s` must be live; `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_skin_info(s: *const VksMeshSkin, out: *mut VksMeshSkinInfo) -> i32 {
    if s.is_null() || out.is_null() {
        return 0;
    }
    let k = &unsafe { &*s }.inner;
    let mut bs = [0f32; 16];
    for i in 0..4 {
        for j in 0..4 {
            bs[i * 4 + j] = k.bind_shape_matrix[i][j];
        }
    }
    unsafe {
        *out = VksMeshSkinInfo {
            joint_count: k.joint_names.len(),
            inv_bind_count: k.inv_bind_matrix.len(),
            alt_bind_count: k.alternate_bind_matrix.len(),
            bind_pose_count: k.bind_pose_matrix.len(),
            pelvis_offset: k.pelvis_offset,
            lock_scale_if_joint_position: k.lock_scale_if_joint_position as i32,
            bind_shape_matrix: bs,
        }
    };
    1
}

/// Joint name `i` (bytes, not NUL-terminated); returns its length.
///
/// # Safety
/// `s` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_skin_joint_name(s: *const VksMeshSkin, i: usize, out: *mut *const c_char) -> usize {
    if s.is_null() {
        return 0;
    }
    let k = &unsafe { &*s }.inner;
    match k.joint_names.get(i) {
        Some(n) => {
            if !out.is_null() {
                unsafe { *out = n.as_ptr() as *const c_char };
            }
            n.len()
        }
        None => 0,
    }
}

/// Matrix `i` of `kind` (0 inverse bind, 1 alternate bind, 2 bind pose)
/// as 16 floats, row-major like `LLMatrix4a`.
///
/// # Safety
/// `s` must be live; `out` must be writable for 16 floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_skin_matrix(s: *const VksMeshSkin, kind: i32, i: usize, out: *mut f32) -> i32 {
    if s.is_null() || out.is_null() {
        return 0;
    }
    let k = &unsafe { &*s }.inner;
    let list = match kind {
        0 => &k.inv_bind_matrix,
        1 => &k.alternate_bind_matrix,
        _ => &k.bind_pose_matrix,
    };
    let Some(m) = list.get(i) else { return 0 };
    for r in 0..4 {
        for c in 0..4 {
            unsafe { *out.add(r * 4 + c) = m[r][c] };
        }
    }
    1
}

/// A parsed convex decomposition.
pub struct VksMeshDecomp {
    inner: Decomposition,
}

/// `LLModel::Decomposition::fromZippedData`: null when the block does not
/// inflate or parse.
///
/// # Safety
/// `data` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_decomp_parse(data: *const u8, len: usize) -> *mut VksMeshDecomp {
    let d = unsafe { bytes(data, len) };
    match catch_unwind(AssertUnwindSafe(|| Decomposition::from_zipped(d))) {
        Ok(Ok(inner)) => Box::into_raw(Box::new(VksMeshDecomp { inner })),
        _ => std::ptr::null_mut(),
    }
}

/// # Safety
/// `d` must come from `vks_mesh_decomp_parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_decomp_free(d: *mut VksMeshDecomp) {
    if !d.is_null() {
        drop(unsafe { Box::from_raw(d) });
    }
}

/// The hull count, and whether the base hull mesh was cleared (no
/// `BoundingVerts`).
///
/// # Safety
/// `d` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_decomp_info(d: *const VksMeshDecomp, hull_count: *mut usize, base_hull_count: *mut usize, base_mesh_cleared: *mut i32) {
    if d.is_null() {
        return;
    }
    let k = &unsafe { &*d }.inner;
    unsafe {
        if !hull_count.is_null() { *hull_count = k.hulls.len(); }
        if !base_hull_count.is_null() { *base_hull_count = k.base_hull.len(); }
        if !base_mesh_cleared.is_null() { *base_mesh_cleared = k.base_hull_mesh_cleared as i32; }
    }
}

/// Hull `i`'s points (`x y z` floats); `i` == SIZE_MAX gives the base hull.
///
/// # Safety
/// `d` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_mesh_decomp_hull(d: *const VksMeshDecomp, i: usize, points: *mut *const f32) -> usize {
    if d.is_null() {
        return 0;
    }
    let k = &unsafe { &*d }.inner;
    let list = if i == usize::MAX { Some(&k.base_hull) } else { k.hulls.get(i) };
    match list {
        Some(l) => {
            if !points.is_null() {
                unsafe { *points = l.as_ptr() as *const f32 };
            }
            l.len()
        }
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// The generic asset fetch

/// `LLViewerAssetStorage::getAssetURL`.
///
/// # Safety
/// Strings must point to their lengths; `buf` to `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_asset_url(cap_url: *const c_char, cap_len: usize, type_name: *const c_char, type_len: usize, id: *const u8, buf: *mut c_char, cap: usize) -> usize {
    let s = asset::asset_url(unsafe { str_arg(cap_url, cap_len) }, unsafe { str_arg(type_name, type_len) }, &unsafe { uuid_arg(id) });
    unsafe { copy_out(s.as_bytes(), buf, cap) }
}

/// `LLAssetStorage::getErrorString`: a static NUL-terminated string.
#[unsafe(no_mangle)]
pub extern "C" fn vks_asset_error_string(status: i32) -> *const c_char {
    static STRINGS: std::sync::OnceLock<Vec<std::ffi::CString>> = std::sync::OnceLock::new();
    let table = STRINGS.get_or_init(|| {
        [
            asset::LL_ERR_NOERR,
            asset::LL_ERR_ASSET_REQUEST_FAILED,
            asset::LL_ERR_ASSET_REQUEST_NONEXISTENT_FILE,
            asset::LL_ERR_ASSET_REQUEST_NOT_IN_DATABASE,
            asset::LL_ERR_NO_CAP,
            asset::LL_ERR_EOF,
            asset::LL_ERR_CANNOT_OPEN_FILE,
            asset::LL_ERR_FILE_NOT_FOUND,
            asset::LL_ERR_TCP_TIMEOUT,
            asset::LL_ERR_CIRCUIT_GONE,
            asset::LL_ERR_PRICE_MISMATCH,
            i32::MIN,
        ]
        .iter()
        .map(|&code| std::ffi::CString::new(asset::error_string(code)).unwrap_or_default())
        .collect()
    });
    let codes = [
        asset::LL_ERR_NOERR,
        asset::LL_ERR_ASSET_REQUEST_FAILED,
        asset::LL_ERR_ASSET_REQUEST_NONEXISTENT_FILE,
        asset::LL_ERR_ASSET_REQUEST_NOT_IN_DATABASE,
        asset::LL_ERR_NO_CAP,
        asset::LL_ERR_EOF,
        asset::LL_ERR_CANNOT_OPEN_FILE,
        asset::LL_ERR_FILE_NOT_FOUND,
        asset::LL_ERR_TCP_TIMEOUT,
        asset::LL_ERR_CIRCUIT_GONE,
        asset::LL_ERR_PRICE_MISMATCH,
    ];
    let idx = codes.iter().position(|&c| c == status).unwrap_or(codes.len());
    table[idx].as_ptr()
}

pub struct VksToxicMap {
    inner: ToxicMap,
}

#[unsafe(no_mangle)]
pub extern "C" fn vks_toxic_new() -> *mut VksToxicMap {
    Box::into_raw(Box::new(VksToxicMap { inner: ToxicMap::default() }))
}

/// # Safety
/// `m` must come from `vks_toxic_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_toxic_free(m: *mut VksToxicMap) {
    if !m.is_null() {
        drop(unsafe { Box::from_raw(m) });
    }
}

/// # Safety
/// `m` must be live; `id` must point to 16 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_toxic_is_toxic(m: *mut VksToxicMap, id: *const u8, now_us: u64) -> i32 {
    if m.is_null() {
        return 0;
    }
    unsafe { &mut *m }.inner.is_toxic(&unsafe { uuid_arg(id) }, now_us) as i32
}

/// # Safety
/// `m` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_toxic_flush_old(m: *mut VksToxicMap, now_us: u64, force: i32) {
    if !m.is_null() {
        unsafe { &mut *m }.inner.flush_old(now_us, force != 0);
    }
}

/// # Safety
/// `m` must be live; `id` must point to 16 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_toxic_mark(m: *mut VksToxicMap, id: *const u8, now_us: u64) {
    if !m.is_null() {
        unsafe { &mut *m }.inner.mark(&unsafe { uuid_arg(id) }, now_us);
    }
}

/// A pending request as the list logic sees it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksAssetRequestKey {
    pub id: [u8; 16],
    pub asset_type: i32,
    pub callback: u64,
    pub user_data: u64,
    pub time: f64,
}

unsafe fn keys_arg(keys: *const VksAssetRequestKey, n: usize) -> Vec<RequestKey> {
    if keys.is_null() {
        return Vec::new();
    }
    unsafe { slice::from_raw_parts(keys, n) }
        .iter()
        .map(|k| RequestKey { id: k.id, asset_type: k.asset_type, callback: k.callback, user_data: k.user_data, time: k.time })
        .collect()
}

/// `getAssetData`'s duplicate verdict: 0 discard, 1 duplicate, 2 new.
///
/// # Safety
/// `keys` must point to `n` entries; `id` to 16 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_asset_duplicate_verdict(keys: *const VksAssetRequestKey, n: usize, id: *const u8, asset_type: i32, callback: u64, user_data: u64) -> i32 {
    let k = unsafe { keys_arg(keys, n) };
    match asset::duplicate_verdict(&k, &unsafe { uuid_arg(id) }, asset_type, callback, user_data) {
        Verdict::Discard => 0,
        Verdict::Duplicate => 1,
        Verdict::New => 2,
    }
}

/// `removeAndCallbackPendingDownloads`'s selection: the matching indices
/// in callback order; returns the count written (up to `cap`).
///
/// # Safety
/// `keys` must point to `n` entries; `out` must be writable for `cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_asset_completion_order(keys: *const VksAssetRequestKey, n: usize, id: *const u8, asset_type: i32, out: *mut usize, cap: usize) -> usize {
    let k = unsafe { keys_arg(keys, n) };
    let order = asset::completion_order(&k, &unsafe { uuid_arg(id) }, asset_type);
    let count = order.len().min(cap);
    for (i, v) in order.iter().take(count).enumerate() {
        unsafe { *out.add(i) = *v };
    }
    count
}

/// `_cleanupRequests`'s selection for one list.
///
/// # Safety
/// As `vks_asset_completion_order`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_asset_timed_out_order(keys: *const VksAssetRequestKey, n: usize, is_download_list: i32, all: i32, now_secs: f64, timeout_secs: f64, out: *mut usize, cap: usize) -> usize {
    let k = unsafe { keys_arg(keys, n) };
    let order = asset::timed_out_order(&k, is_download_list != 0, all != 0, now_secs, timeout_secs);
    let count = order.len().min(cap);
    for (i, v) in order.iter().take(count).enumerate() {
        unsafe { *out.add(i) = *v };
    }
    count
}
