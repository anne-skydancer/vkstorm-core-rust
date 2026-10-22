//! HTTP(S) for the music stream and the Icecast status document, through
//! the viewer's libcurl.
//!
//! A stream request follows redirects, verifies TLS against the viewer's CA
//! bundle, sends the configured User-Agent and `Icy-MetaData: 1`, and
//! accepts SHOUTcast's non-HTTP `ICY 200 OK` status line (registered with
//! `CURLOPT_HTTP200ALIASES`, which libcurl 7.54.1 and 8.x both honour; the
//! mock server's tests cover it). The connection phase is bounded by
//! `CURLOPT_CONNECTTIMEOUT`; afterwards a transfer that receives nothing
//! for the stall timeout is aborted (from the progress callback, so a
//! decoder that holds the transfer back while its ring is full does not
//! count as a stall).
//!
//! # Linking
//! libcurl is called through a table of entry points ([`CurlApi`]). With
//! the `curl-symbols` feature (the viewer's FFI crate enables it; the
//! viewer links libcurl) or `link-curl` (the crate's own tests) the table
//! is built from libcurl's exported symbols. Without either, nothing in the
//! crate references libcurl, so the crate's other tests link without it,
//! and an embedder may supply the table with [`set_curl_api`]. The option
//! and info numbers are libcurl's stable ABI values (all exist in 7.54.1).

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use super::icy::IcyStripper;
use super::pipe::{Head, Pipe, PipeEnd};

// The crate's own tests: vkstorm-http's build script names libcurl for the
// linker, which takes effect only if the crate is linked.
#[cfg(feature = "link-curl")]
use vkstorm_http as _;

// libcurl's own type names.
#[allow(clippy::upper_case_acronyms)]
type CURL = c_void;
type CURLcode = c_int;
type CURLoption = c_int;
#[allow(clippy::upper_case_acronyms)]
type CURLINFO = c_int;
type CurlOffT = i64;

const LONG: CURLoption = 0;
const OBJECTPOINT: CURLoption = 10000;
const FUNCTIONPOINT: CURLoption = 20000;
const CURLOPT_WRITEDATA: CURLoption = OBJECTPOINT + 1;
const CURLOPT_URL: CURLoption = OBJECTPOINT + 2;
const CURLOPT_WRITEFUNCTION: CURLoption = FUNCTIONPOINT + 11;
const CURLOPT_TIMEOUT: CURLoption = LONG + 13;
const CURLOPT_USERAGENT: CURLoption = OBJECTPOINT + 18;
const CURLOPT_HTTPHEADER: CURLoption = OBJECTPOINT + 23;
const CURLOPT_HEADERDATA: CURLoption = OBJECTPOINT + 29;
const CURLOPT_NOPROGRESS: CURLoption = LONG + 43;
const CURLOPT_FOLLOWLOCATION: CURLoption = LONG + 52;
const CURLOPT_XFERINFODATA: CURLoption = OBJECTPOINT + 57;
const CURLOPT_SSL_VERIFYPEER: CURLoption = LONG + 64;
const CURLOPT_CAINFO: CURLoption = OBJECTPOINT + 65;
const CURLOPT_MAXREDIRS: CURLoption = LONG + 68;
const CURLOPT_CONNECTTIMEOUT: CURLoption = LONG + 78;
const CURLOPT_HEADERFUNCTION: CURLoption = FUNCTIONPOINT + 79;
const CURLOPT_SSL_VERIFYHOST: CURLoption = LONG + 81;
const CURLOPT_NOSIGNAL: CURLoption = LONG + 99;
const CURLOPT_HTTP200ALIASES: CURLoption = OBJECTPOINT + 104;
const CURLOPT_XFERINFOFUNCTION: CURLoption = FUNCTIONPOINT + 219;
const CURLINFO_EFFECTIVE_URL: CURLINFO = 0x100000 + 1;
const CURLINFO_RESPONSE_CODE: CURLINFO = 0x200000 + 2;
const CURLE_OK: CURLcode = 0;
const CURLE_WRITE_ERROR: CURLcode = 23;
const CURLE_ABORTED_BY_CALLBACK: CURLcode = 42;

type XferInfoFn = unsafe extern "C" fn(*mut c_void, CurlOffT, CurlOffT, CurlOffT, CurlOffT) -> c_int;

/// The libcurl entry points the stream code calls (`curl_easy_*`,
/// `curl_slist_*`), with libcurl's C signatures.
#[derive(Clone, Copy, Debug)]
pub struct CurlApi {
    pub easy_init: unsafe extern "C" fn() -> *mut c_void,
    pub easy_cleanup: unsafe extern "C" fn(*mut c_void),
    pub easy_setopt: unsafe extern "C" fn(*mut c_void, c_int, ...) -> c_int,
    pub easy_getinfo: unsafe extern "C" fn(*mut c_void, c_int, ...) -> c_int,
    pub easy_perform: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub easy_strerror: unsafe extern "C" fn(c_int) -> *const c_char,
    pub slist_append: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    pub slist_free_all: unsafe extern "C" fn(*mut c_void),
}

#[cfg(feature = "curl-symbols")]
mod symbols {
    use std::ffi::{c_char, c_int, c_void};
    unsafe extern "C" {
        pub fn curl_easy_init() -> *mut c_void;
        pub fn curl_easy_cleanup(h: *mut c_void);
        pub fn curl_easy_setopt(h: *mut c_void, option: c_int, ...) -> c_int;
        pub fn curl_easy_getinfo(h: *mut c_void, info: c_int, ...) -> c_int;
        pub fn curl_easy_perform(h: *mut c_void) -> c_int;
        pub fn curl_easy_strerror(code: c_int) -> *const c_char;
        pub fn curl_slist_append(list: *mut c_void, s: *const c_char) -> *mut c_void;
        pub fn curl_slist_free_all(list: *mut c_void);
        #[cfg(feature = "link-curl")]
        pub fn curl_global_init(flags: std::ffi::c_long) -> c_int;
    }
}

#[cfg(feature = "curl-symbols")]
impl CurlApi {
    /// The table of libcurl's exported symbols (resolved at link time).
    pub fn linked() -> CurlApi {
        CurlApi {
            easy_init: symbols::curl_easy_init,
            easy_cleanup: symbols::curl_easy_cleanup,
            easy_setopt: symbols::curl_easy_setopt,
            easy_getinfo: symbols::curl_easy_getinfo,
            easy_perform: symbols::curl_easy_perform,
            easy_strerror: symbols::curl_easy_strerror,
            slist_append: symbols::curl_slist_append,
            slist_free_all: symbols::curl_slist_free_all,
        }
    }
}

static API: OnceLock<CurlApi> = OnceLock::new();

/// Supplies the libcurl entry points (for an embedder that builds the
/// crate without `curl-symbols`). The first table set wins; returns false
/// when one was already in place.
pub fn set_curl_api(api: CurlApi) -> bool {
    API.set(api).is_ok()
}

fn api() -> Result<&'static CurlApi, String> {
    #[cfg(feature = "curl-symbols")]
    {
        Ok(API.get_or_init(|| {
            // The crate's tests initialise libcurl themselves; the viewer
            // did it at startup.
            #[cfg(feature = "link-curl")]
            // SAFETY: CURL_GLOBAL_ALL, before any handle of this process.
            unsafe {
                symbols::curl_global_init(3);
            }
            CurlApi::linked()
        }))
    }
    #[cfg(not(feature = "curl-symbols"))]
    {
        API.get().ok_or_else(|| "libcurl is not available (build with the curl-symbols feature or call stream::set_curl_api)".to_owned())
    }
}

fn strerror(api: &CurlApi, code: CURLcode) -> String {
    // SAFETY: libcurl returns a static NUL-terminated string for any code.
    let p = unsafe { (api.easy_strerror)(code) };
    if p.is_null() {
        return format!("libcurl error {code}");
    }
    // SAFETY: as above.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// Connection settings shared by the stream and status requests.
#[derive(Clone, Debug)]
pub(crate) struct NetConfig {
    pub user_agent: String,
    pub ca_bundle: Option<PathBuf>,
    pub connect_timeout: Duration,
    pub stall_timeout: Duration,
}

/// Receives a transfer's response.
trait Body {
    /// The head (status, headers) at the first body bytes or at the end;
    /// false aborts.
    fn head(&mut self, head: &Head) -> bool;
    /// Body bytes; false aborts.
    fn data(&mut self, bytes: &[u8]) -> bool;
    /// Polled from the progress callback: true aborts.
    fn cancelled(&self) -> bool;
}

struct Transfer<'a> {
    api: &'static CurlApi,
    handle: *mut CURL,
    body: &'a mut dyn Body,
    status_line: String,
    headers: Vec<(String, String)>,
    head_sent: bool,
    head: Head,
    received: u64,
    last_data: Instant,
    stall: Duration,
    stalled: bool,
    aborted_by_body: bool,
}

impl Transfer<'_> {
    fn build_head(&mut self) {
        let mut code: c_long = 0;
        let mut url: *const c_char = std::ptr::null();
        // SAFETY: the handle is live for the duration of the transfer.
        unsafe {
            (self.api.easy_getinfo)(self.handle, CURLINFO_RESPONSE_CODE, &mut code as *mut c_long);
            (self.api.easy_getinfo)(self.handle, CURLINFO_EFFECTIVE_URL, &mut url as *mut *const c_char);
        }
        let header = |n: &str| self.headers.iter().rev().find(|(k, _)| k == n).map(|(_, v)| v.clone());
        let effective_url = if url.is_null() {
            String::new()
        } else {
            // SAFETY: libcurl returns a NUL-terminated string it owns.
            unsafe { CStr::from_ptr(url) }.to_string_lossy().into_owned()
        };
        self.head = Head {
            status: u32::try_from(code).unwrap_or(0),
            status_line: self.status_line.clone(),
            content_type: header("content-type").unwrap_or_default(),
            content_length: header("content-length").and_then(|v| v.parse().ok()),
            chunked: header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")),
            icy_metaint: header("icy-metaint").and_then(|v| v.parse().ok()).filter(|&n: &usize| n > 0),
            effective_url,
        };
    }

    fn send_head(&mut self) -> bool {
        self.head_sent = true;
        self.build_head();
        let head = self.head.clone();
        self.body.head(&head)
    }
}

unsafe extern "C" fn header_cb(ptr: *mut c_char, size: usize, n: usize, ud: *mut c_void) -> usize {
    let len = size * n;
    let r = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: libcurl passes `len` bytes and our Transfer.
        let (t, bytes) = unsafe { (&mut *(ud.cast::<Transfer<'_>>()), std::slice::from_raw_parts(ptr.cast::<u8>(), len)) };
        t.last_data = Instant::now();
        let line = String::from_utf8_lossy(bytes);
        let line = line.trim_end_matches(['\r', '\n']);
        if line.starts_with("HTTP/") || line.starts_with("ICY ") {
            // A new response (redirects and 100-continue come first).
            t.status_line = line.to_owned();
            t.headers.clear();
        } else if let Some((k, v)) = line.split_once(':') {
            t.headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
        len
    }));
    r.unwrap_or(0)
}

unsafe extern "C" fn write_cb(ptr: *mut c_char, size: usize, n: usize, ud: *mut c_void) -> usize {
    let len = size * n;
    let r = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: as above.
        let (t, bytes) = unsafe { (&mut *(ud.cast::<Transfer<'_>>()), std::slice::from_raw_parts(ptr.cast::<u8>(), len)) };
        if !t.head_sent && !t.send_head() {
            t.aborted_by_body = true;
            return 0;
        }
        t.received += len as u64;
        if !t.body.data(bytes) {
            t.aborted_by_body = true;
            return 0;
        }
        // After a write that waited for room, the time spent waiting is
        // not a stall.
        t.last_data = Instant::now();
        len
    }));
    r.unwrap_or(0)
}

unsafe extern "C" fn xferinfo_cb(ud: *mut c_void, _: CurlOffT, _: CurlOffT, _: CurlOffT, _: CurlOffT) -> c_int {
    let r = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: our Transfer.
        let t = unsafe { &mut *(ud.cast::<Transfer<'_>>()) };
        if t.body.cancelled() {
            return 1;
        }
        if t.last_data.elapsed() > t.stall {
            t.stalled = true;
            return 1;
        }
        0
    }));
    r.unwrap_or(1)
}

/// The outcome of a transfer.
struct Outcome {
    code: CURLcode,
    /// libcurl's text for `code`.
    error: String,
    head: Head,
    head_sent: bool,
    received: u64,
    stalled: bool,
    aborted_by_body: bool,
}

/// Runs one GET to completion on the calling thread.
fn perform(url: &str, cfg: &NetConfig, icy: bool, total_timeout: Option<Duration>, body: &mut dyn Body) -> Result<Outcome, String> {
    let api = api()?;
    let curl_url = CString::new(url).map_err(|_| "invalid URL".to_owned())?;
    let ua = CString::new(cfg.user_agent.as_str()).unwrap_or_default();
    let ca = cfg.ca_bundle.as_ref().and_then(|p| CString::new(p.to_string_lossy().as_bytes()).ok());
    let icy_header = c"Icy-MetaData: 1";
    let alias = c"ICY 200 OK";
    // SAFETY: a fresh easy handle configured with pointers that outlive the
    // perform call (CStrings, lists and the Transfer on this stack frame);
    // everything is released before returning.
    unsafe {
        let h = (api.easy_init)();
        if h.is_null() {
            return Err("libcurl: cannot create a handle".into());
        }
        let mut headers: *mut c_void = std::ptr::null_mut();
        if icy {
            headers = (api.slist_append)(headers, icy_header.as_ptr());
        }
        let aliases = (api.slist_append)(std::ptr::null_mut(), alias.as_ptr());
        let mut t = Transfer {
            api,
            handle: h,
            body,
            status_line: String::new(),
            headers: Vec::new(),
            head_sent: false,
            head: Head::default(),
            received: 0,
            last_data: Instant::now(),
            stall: cfg.stall_timeout,
            stalled: false,
            aborted_by_body: false,
        };
        let tp = (&mut t as *mut Transfer<'_>).cast::<c_void>();
        (api.easy_setopt)(h, CURLOPT_URL, curl_url.as_ptr());
        (api.easy_setopt)(h, CURLOPT_NOSIGNAL, 1 as c_long);
        (api.easy_setopt)(h, CURLOPT_FOLLOWLOCATION, 1 as c_long);
        (api.easy_setopt)(h, CURLOPT_MAXREDIRS, 10 as c_long);
        (api.easy_setopt)(h, CURLOPT_USERAGENT, ua.as_ptr());
        if !headers.is_null() {
            (api.easy_setopt)(h, CURLOPT_HTTPHEADER, headers);
        }
        (api.easy_setopt)(h, CURLOPT_HTTP200ALIASES, aliases);
        if let Some(ca) = &ca {
            (api.easy_setopt)(h, CURLOPT_CAINFO, ca.as_ptr());
        }
        (api.easy_setopt)(h, CURLOPT_SSL_VERIFYPEER, 1 as c_long);
        (api.easy_setopt)(h, CURLOPT_SSL_VERIFYHOST, 2 as c_long);
        (api.easy_setopt)(h, CURLOPT_CONNECTTIMEOUT, cfg.connect_timeout.as_secs().max(1) as c_long);
        if let Some(t) = total_timeout {
            (api.easy_setopt)(h, CURLOPT_TIMEOUT, t.as_secs().max(1) as c_long);
        }
        (api.easy_setopt)(h, CURLOPT_HEADERFUNCTION, header_cb as *const c_void);
        (api.easy_setopt)(h, CURLOPT_HEADERDATA, tp);
        (api.easy_setopt)(h, CURLOPT_WRITEFUNCTION, write_cb as *const c_void);
        (api.easy_setopt)(h, CURLOPT_WRITEDATA, tp);
        (api.easy_setopt)(h, CURLOPT_NOPROGRESS, 0 as c_long);
        (api.easy_setopt)(h, CURLOPT_XFERINFOFUNCTION, xferinfo_cb as XferInfoFn as *const c_void);
        (api.easy_setopt)(h, CURLOPT_XFERINFODATA, tp);
        let code = (api.easy_perform)(h);
        if !t.head_sent {
            t.build_head();
        }
        let out = Outcome {
            code,
            error: if code == CURLE_OK { String::new() } else { strerror(api, code) },
            head: t.head.clone(),
            head_sent: t.head_sent,
            received: t.received,
            stalled: t.stalled,
            aborted_by_body: t.aborted_by_body,
        };
        drop(t);
        (api.easy_cleanup)(h);
        if !headers.is_null() {
            (api.slist_free_all)(headers);
        }
        (api.slist_free_all)(aliases);
        Ok(out)
    }
}

struct StreamBody<'a> {
    pipe: &'a Pipe,
    icy: Option<IcyStripper>,
    on_title: &'a mut dyn FnMut(String),
    abort: &'a dyn Fn() -> bool,
}

impl Body for StreamBody<'_> {
    fn head(&mut self, head: &Head) -> bool {
        self.icy = head.icy_metaint.map(IcyStripper::new);
        self.pipe.set_head(head.clone());
        // A failed request's body (an error page) is of no use.
        (200..300).contains(&head.status)
    }

    fn data(&mut self, bytes: &[u8]) -> bool {
        let pipe = self.pipe;
        match &mut self.icy {
            Some(s) => s.feed(bytes, &mut |a| pipe.write(a), &mut |t| (self.on_title)(t)),
            None => pipe.write(bytes),
        }
    }

    fn cancelled(&self) -> bool {
        self.pipe.is_cancelled() || (self.abort)()
    }
}

/// Streams `url` into `pipe` (ICY metadata stripped, each new StreamTitle
/// to `on_title`) until the body ends, the transfer fails or `abort`
/// returns true; then closes the pipe with the reason. Blocks.
pub(crate) fn stream(url: &str, cfg: &NetConfig, pipe: &Pipe, abort: &dyn Fn() -> bool, on_title: &mut dyn FnMut(String)) {
    let mut body = StreamBody { pipe, icy: None, on_title, abort };
    let end = match perform(url, cfg, true, None, &mut body) {
        Err(e) => PipeEnd::Error(e),
        Ok(o) => {
            if !o.head_sent {
                // No body at all: still let the reader see the status.
                if o.code == CURLE_OK {
                    pipe.set_head(o.head.clone());
                }
            }
            if pipe.is_cancelled() || abort() {
                PipeEnd::Cancelled
            } else if o.code == CURLE_OK {
                let complete = o.head.content_length.is_some_and(|l| o.received >= l) || o.head.chunked;
                if complete { PipeEnd::Complete } else { PipeEnd::Eof }
            } else if o.stalled {
                PipeEnd::Error(format!("no data for {} s", cfg.stall_timeout.as_secs()))
            } else if o.aborted_by_body || o.code == CURLE_WRITE_ERROR || o.code == CURLE_ABORTED_BY_CALLBACK {
                if (200..300).contains(&o.head.status) {
                    PipeEnd::Cancelled
                } else {
                    PipeEnd::Error(format!("HTTP {}", o.head.status))
                }
            } else {
                PipeEnd::Error(o.error)
            }
        }
    };
    pipe.close(end);
}

struct CollectBody<'a> {
    data: Vec<u8>,
    limit: usize,
    abort: &'a dyn Fn() -> bool,
}

impl Body for CollectBody<'_> {
    fn head(&mut self, _: &Head) -> bool {
        true
    }

    fn data(&mut self, bytes: &[u8]) -> bool {
        if self.data.len() + bytes.len() > self.limit {
            return false;
        }
        self.data.extend_from_slice(bytes);
        true
    }

    fn cancelled(&self) -> bool {
        (self.abort)()
    }
}

/// A small GET (the Icecast status document): (status, body).
pub(crate) fn get(url: &str, cfg: &NetConfig, limit: usize, timeout: Duration, abort: &dyn Fn() -> bool) -> Result<(u32, Vec<u8>), String> {
    let mut body = CollectBody { data: Vec::new(), limit, abort };
    let o = perform(url, cfg, false, Some(timeout), &mut body)?;
    if o.code != CURLE_OK {
        return Err(if o.stalled { "timed out".into() } else { o.error });
    }
    Ok((o.head.status, body.data))
}
