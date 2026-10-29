//! Rung four: the HTTP layer (`vks_http_*`), the C ABI over `vkstorm-http`
//! behind `LLCore::HttpRequest`. One service per process, as in the viewer:
//! `vks_http_service_create` builds it with the host's hooks (logging,
//! statistics, `LLProxy`, the SSL certificate callback, thread
//! registration), the `vks_http_request_*` calls queue operations on a
//! reply queue, and `vks_http_reply_fetch` hands back what completed, one
//! reply at a time, for the C++ `HttpRequest::update()` to notify the
//! handlers. Handles are the service's own 64-bit counter: never recycled.
//!
//! Panics are caught at the boundary and reported as failures.

use std::ffi::{CStr, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;
use std::sync::{Arc, Mutex, MutexGuard};

use vkstorm_http::hooks::{Hooks, LogLevel, Stat};
use vkstorm_http::request::{Reply, ReplyKind, ReplyQueue, Request};
use vkstorm_http::sys::{CURL, SendPtr, SslInfo, UserPtr};
use vkstorm_http::{Method, RequestOptions, Service, Status, he};

/// The hooks the host supplies; every function may be null.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksHttpHooks {
    pub ctx: *mut c_void,
    pub log: Option<unsafe extern "C" fn(*mut c_void, i32, *const c_char, usize)>,
    pub stat: Option<unsafe extern "C" fn(*mut c_void, i32, u64)>,
    pub apply_proxy: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    pub ssl_ctx: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *mut SslInfo)>,
    pub thread_start: Option<unsafe extern "C" fn(*mut c_void)>,
    pub thread_stop: Option<unsafe extern "C" fn(*mut c_void)>,
    pub release_user: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
}

struct CHooks(VksHttpHooks);

// SAFETY: the host's hook functions are called from the service thread and
// the consumer threads alike, as the C++ implementation called the same
// facilities (LLProxy, LLError, HTTPStats) from its worker thread.
unsafe impl Send for CHooks {}
unsafe impl Sync for CHooks {}

impl Hooks for CHooks {
    fn log(&self, level: LogLevel, message: &str) {
        if let Some(f) = self.0.log {
            unsafe {
                f(
                    self.0.ctx,
                    level as i32,
                    message.as_ptr() as *const c_char,
                    message.len(),
                )
            };
        }
    }

    fn stat(&self, stat: Stat, value: u64) {
        if let Some(f) = self.0.stat {
            unsafe { f(self.0.ctx, stat as i32, value) };
        }
    }

    fn apply_proxy(&self, curl: *mut CURL) {
        if let Some(f) = self.0.apply_proxy {
            unsafe { f(self.0.ctx, curl) };
        }
    }

    fn ssl_ctx(&self, ssl_ctx: *mut c_void, info: &mut SslInfo) {
        if let Some(f) = self.0.ssl_ctx {
            unsafe { f(self.0.ctx, ssl_ctx, info as *mut SslInfo) };
        }
    }

    fn thread_start(&self) {
        if let Some(f) = self.0.thread_start {
            unsafe { f(self.0.ctx) };
        }
    }

    fn thread_stop(&self) {
        if let Some(f) = self.0.thread_stop {
            unsafe { f(self.0.ctx) };
        }
    }

    fn release_user(&self, user: UserPtr) {
        if user.is_null() {
            return;
        }
        if let Some(f) = self.0.release_user {
            unsafe { f(self.0.ctx, user.0) };
        }
    }
}

/// `LLCore::HttpStatus` by value: the type and the code.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VksHttpStatus {
    pub kind: u16,
    pub code: i16,
}

impl From<Status> for VksHttpStatus {
    fn from(s: Status) -> Self {
        VksHttpStatus {
            kind: s.kind,
            code: s.code,
        }
    }
}

/// `LLCore::HttpOptions` by value.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksHttpOptions {
    pub want_headers: i32,
    pub trace: i32,
    pub timeout: u32,
    pub transfer_timeout: u32,
    pub retries: u32,
    pub min_backoff: u64,
    pub max_backoff: u64,
    pub use_retry_after: i32,
    pub follow_redirects: i32,
    pub verify_peer: i32,
    pub verify_host: i32,
    pub dns_cache_timeout: i32,
    pub headers_only: i32,
    pub last_modified: i64,
}

/// One header, name and value as byte ranges.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksHttpHeader {
    pub name: *const c_char,
    pub name_len: usize,
    pub value: *const c_char,
    pub value_len: usize,
}

/// A request to submit. `method`: 0 GET, 1 POST, 2 PUT, 3 DELETE, 4 PATCH,
/// 5 COPY, 6 MOVE. `has_body` distinguishes an empty body from none.
#[repr(C)]
pub struct VksHttpRequestDesc {
    pub policy: u32,
    pub method: i32,
    pub url: *const c_char,
    pub url_len: usize,
    pub body: *const u8,
    pub body_len: usize,
    pub has_body: i32,
    pub offset: u64,
    pub length: u64,
    pub headers: *const VksHttpHeader,
    pub header_count: usize,
    pub options: *const VksHttpOptions,
}

/// A reply queue (`HttpReplyQueue`), one per `HttpRequest` instance.
pub struct VksHttpReplyQueue {
    inner: Arc<ReplyQueue>,
}

/// A completed operation, read through the accessors and freed with
/// `vks_http_reply_free`.
pub struct VksHttpReply {
    inner: Reply,
}

static SERVICE: Mutex<Option<Service>> = Mutex::new(None);

fn service() -> MutexGuard<'static, Option<Service>> {
    SERVICE.lock().unwrap_or_else(|e| e.into_inner())
}

unsafe fn bytes<'a>(p: *const c_char, len: usize) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(p as *const u8, len) }
    }
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn status_fail(out: *mut VksHttpStatus, s: Status) -> u64 {
    if !out.is_null() {
        unsafe { *out = s.into() };
    }
    0
}

fn status_ok(out: *mut VksHttpStatus, handle: u64) -> u64 {
    if !out.is_null() {
        unsafe { *out = Status::OK.into() };
    }
    handle
}

fn issue(out: *mut VksHttpStatus, r: Result<u64, Status>) -> u64 {
    match r {
        Ok(h) => status_ok(out, h),
        Err(e) => status_fail(out, e),
    }
}

/// Creates the service (`HttpRequest::createService`) with the host's
/// hooks and `User-Agent`. Returns 1, or 0 when one exists already or the
/// arguments are invalid. libcurl must have been initialised by the host.
///
/// # Safety
/// `hooks` must point to a valid table whose functions stay callable until
/// `vks_http_service_destroy`; `user_agent` must be NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_service_create(
    hooks: *const VksHttpHooks,
    user_agent: *const c_char,
) -> i32 {
    if hooks.is_null() {
        return 0;
    }
    let hooks = unsafe { *hooks };
    let ua = if user_agent.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(user_agent) }
            .to_string_lossy()
            .into_owned()
    };
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mut s = service();
        if s.is_some() {
            return 0;
        }
        *s = Some(Service::new(Arc::new(CHooks(hooks)), &ua));
        1
    }));
    r.unwrap_or(0)
}

/// `HttpRequest::destroyService`: stops a running thread (waiting up to a
/// second for it, as before) and releases the service.
#[unsafe(no_mangle)]
pub extern "C" fn vks_http_service_destroy() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let taken = service().take();
        if let Some(s) = taken {
            s.destroy();
        }
    }));
}

/// `HttpService::getState`: -1 not initialised, 0 initialised, 1 running,
/// 2 stopped.
#[unsafe(no_mangle)]
pub extern "C" fn vks_http_service_state() -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        service().as_ref().map_or(-1, |s| s.state() as i32)
    }))
    .unwrap_or(-1)
}

/// `HttpRequest::startThread`.
#[unsafe(no_mangle)]
pub extern "C" fn vks_http_service_start_thread() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(s) = service().as_mut() {
            s.start_thread();
        }
    }));
}

/// `HttpRequest::createPolicyClass`.
#[unsafe(no_mangle)]
pub extern "C" fn vks_http_policy_class_create() -> u32 {
    catch_unwind(AssertUnwindSafe(|| {
        service().as_ref().map_or(0, |s| s.create_policy_class())
    }))
    .unwrap_or(0)
}

/// `HttpRequest::setStaticPolicyOption` with a long; `ret` (optional)
/// receives the value as stored.
///
/// # Safety
/// `ret` must be null or valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_policy_set_long(
    opt: i32,
    pclass: u32,
    value: i64,
    ret: *mut i64,
) -> VksHttpStatus {
    catch_unwind(AssertUnwindSafe(|| {
        let s = service();
        let Some(s) = s.as_ref() else {
            return Status::llcore(he::INVALID_ARG).into();
        };
        match s.set_option_long(opt, pclass, value) {
            Ok(v) => {
                if !ret.is_null() {
                    unsafe { *ret = v };
                }
                Status::OK.into()
            }
            Err(e) => e.into(),
        }
    }))
    .unwrap_or(Status::llcore(he::INVALID_ARG).into())
}

/// `HttpRequest::setStaticPolicyOption` with a string.
///
/// # Safety
/// `value` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_policy_set_string(
    opt: i32,
    pclass: u32,
    value: *const c_char,
    len: usize,
) -> VksHttpStatus {
    let v = lossy(unsafe { bytes(value, len) });
    catch_unwind(AssertUnwindSafe(|| {
        let s = service();
        let Some(s) = s.as_ref() else {
            return Status::llcore(he::INVALID_ARG).into();
        };
        match s.set_option_string(opt, pclass, &v) {
            Ok(_) => Status::OK.into(),
            Err(e) => e.into(),
        }
    }))
    .unwrap_or(Status::llcore(he::INVALID_ARG).into())
}

/// `HttpRequest::setStaticPolicyOption` with the SSL verification
/// callback: `set` says whether the host has one.
#[unsafe(no_mangle)]
pub extern "C" fn vks_http_policy_set_callback(opt: i32, pclass: u32, set: i32) -> VksHttpStatus {
    catch_unwind(AssertUnwindSafe(|| {
        let s = service();
        let Some(s) = s.as_ref() else {
            return Status::llcore(he::INVALID_ARG).into();
        };
        match s.set_option_callback(opt, pclass, set != 0) {
            Ok(_) => Status::OK.into(),
            Err(e) => e.into(),
        }
    }))
    .unwrap_or(Status::llcore(he::INVALID_ARG).into())
}

/// `HttpService::getPolicyOption` with a long.
///
/// # Safety
/// `ret` must be null or valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_policy_get_long(
    opt: i32,
    pclass: u32,
    ret: *mut i64,
) -> VksHttpStatus {
    catch_unwind(AssertUnwindSafe(|| {
        let s = service();
        let Some(s) = s.as_ref() else {
            return Status::llcore(he::INVALID_ARG).into();
        };
        match s.get_option_long(opt, pclass) {
            Ok(v) => {
                if !ret.is_null() {
                    unsafe { *ret = v };
                }
                Status::OK.into()
            }
            Err(e) => e.into(),
        }
    }))
    .unwrap_or(Status::llcore(he::INVALID_ARG).into())
}

/// `HttpService::getPolicyOption` with a string: copies up to `cap` bytes
/// into `buf` (no terminator) and sets `len` to the full length.
///
/// # Safety
/// `buf` must be writable for `cap` bytes; `len` must be null or valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_policy_get_string(
    opt: i32,
    pclass: u32,
    buf: *mut c_char,
    cap: usize,
    len: *mut usize,
) -> VksHttpStatus {
    catch_unwind(AssertUnwindSafe(|| {
        let s = service();
        let Some(s) = s.as_ref() else {
            return Status::llcore(he::INVALID_ARG).into();
        };
        match s.get_option_string(opt, pclass) {
            Ok(v) => {
                let n = v.len().min(cap);
                if !buf.is_null() && n > 0 {
                    unsafe { std::ptr::copy_nonoverlapping(v.as_ptr(), buf as *mut u8, n) };
                }
                if !len.is_null() {
                    unsafe { *len = v.len() };
                }
                Status::OK.into()
            }
            Err(e) => e.into(),
        }
    }))
    .unwrap_or(Status::llcore(he::INVALID_ARG).into())
}

/// A new reply queue. Null if the service does not exist.
#[unsafe(no_mangle)]
pub extern "C" fn vks_http_reply_queue_new() -> *mut VksHttpReplyQueue {
    catch_unwind(AssertUnwindSafe(|| match service().as_ref() {
        Some(s) => Box::into_raw(Box::new(VksHttpReplyQueue {
            inner: s.reply_queue(),
        })),
        None => std::ptr::null_mut(),
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// Releases a reply queue. The host fetches and frees the replies it still
/// holds first (their user data is the host's). Null is ignored.
///
/// # Safety
/// `q` must come from `vks_http_reply_queue_new` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_reply_queue_free(q: *mut VksHttpReplyQueue) {
    if !q.is_null() {
        drop(unsafe { Box::from_raw(q) });
    }
}

fn options_from(o: *const VksHttpOptions) -> Option<RequestOptions> {
    if o.is_null() {
        return None;
    }
    let o = unsafe { &*o };
    Some(RequestOptions {
        want_headers: o.want_headers != 0,
        trace: o.trace,
        timeout: o.timeout,
        transfer_timeout: o.transfer_timeout,
        retries: o.retries,
        min_backoff: o.min_backoff,
        max_backoff: o.max_backoff,
        use_retry_after: o.use_retry_after != 0,
        follow_redirects: o.follow_redirects != 0,
        verify_peer: o.verify_peer != 0,
        verify_host: o.verify_host != 0,
        dns_cache_timeout: o.dns_cache_timeout,
        headers_only: o.headers_only != 0,
        last_modified: o.last_modified,
    })
}

/// Queues a request (`HttpRequest::requestGet` and the other methods).
/// Returns the handle, or 0 with `status` set (`HE_SHUTTING_DOWN` once the
/// service stops). `user` rides along and comes back with the reply.
///
/// # Safety
/// `q` and `desc` must be valid; the pointers in `desc` must be readable
/// for their lengths for the duration of the call (everything is copied).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_request_submit(
    q: *mut VksHttpReplyQueue,
    desc: *const VksHttpRequestDesc,
    user: *mut c_void,
    status: *mut VksHttpStatus,
) -> u64 {
    if q.is_null() || desc.is_null() {
        return status_fail(status, Status::llcore(he::INVALID_ARG));
    }
    let q = unsafe { &*q };
    let d = unsafe { &*desc };
    let method = match d.method {
        0 => Method::Get,
        1 => Method::Post,
        2 => Method::Put,
        3 => Method::Delete,
        4 => Method::Patch,
        5 => Method::Copy,
        6 => Method::Move,
        _ => return status_fail(status, Status::llcore(he::INVALID_ARG)),
    };
    let url = lossy(unsafe { bytes(d.url, d.url_len) });
    let body = if d.has_body != 0 {
        Some(unsafe { bytes(d.body as *const c_char, d.body_len) }.to_vec())
    } else {
        None
    };
    let mut headers = Vec::with_capacity(d.header_count);
    if !d.headers.is_null() {
        for h in unsafe { slice::from_raw_parts(d.headers, d.header_count) } {
            headers.push((
                lossy(unsafe { bytes(h.name, h.name_len) }),
                lossy(unsafe { bytes(h.value, h.value_len) }),
            ));
        }
    }
    let req = Request {
        policy: d.policy,
        method,
        url,
        body,
        offset: d.offset,
        length: d.length,
        headers,
        options: options_from(d.options),
    };
    catch_unwind(AssertUnwindSafe(|| {
        let s = service();
        let Some(s) = s.as_ref() else {
            return status_fail(status, Status::llcore(he::SHUTTING_DOWN));
        };
        issue(status, s.submit(&q.inner, SendPtr(user), req))
    }))
    .unwrap_or_else(|_| status_fail(status, Status::llcore(he::INVALID_ARG)))
}

macro_rules! with_service {
    ($q:expr, $status:expr, |$s:ident| $body:expr) => {{
        if $q.is_null() {
            return status_fail($status, Status::llcore(he::INVALID_ARG));
        }
        let q = unsafe { &*$q };
        catch_unwind(AssertUnwindSafe(|| {
            let s = service();
            let Some($s) = s.as_ref() else {
                return status_fail($status, Status::llcore(he::SHUTTING_DOWN));
            };
            let q = &q.inner;
            issue($status, $body(q))
        }))
        .unwrap_or_else(|_| status_fail($status, Status::llcore(he::INVALID_ARG)))
    }};
}

/// `HttpRequest::requestCancel`.
///
/// # Safety
/// `q` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_request_cancel(
    q: *mut VksHttpReplyQueue,
    target: u64,
    user: *mut c_void,
    status: *mut VksHttpStatus,
) -> u64 {
    with_service!(q, status, |s| |q| s.cancel(q, SendPtr(user), target))
}

/// `HttpRequest::requestNoOp`.
///
/// # Safety
/// `q` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_request_noop(
    q: *mut VksHttpReplyQueue,
    user: *mut c_void,
    status: *mut VksHttpStatus,
) -> u64 {
    with_service!(q, status, |s| |q| s.noop(q, SendPtr(user)))
}

/// `HttpRequest::requestStopThread`.
///
/// # Safety
/// `q` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_request_stop(
    q: *mut VksHttpReplyQueue,
    user: *mut c_void,
    status: *mut VksHttpStatus,
) -> u64 {
    with_service!(q, status, |s| |q| s.stop_thread(q, SendPtr(user)))
}

/// `HttpRequest::requestSpin`.
///
/// # Safety
/// `q` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_request_spin(
    q: *mut VksHttpReplyQueue,
    mode: i32,
    status: *mut VksHttpStatus,
) -> u64 {
    with_service!(q, status, |s| |q| s.spin(q, mode))
}

/// `HttpRequest::setPolicyOption` with a long (dynamic, through the
/// service thread).
///
/// # Safety
/// `q` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_request_set_long(
    q: *mut VksHttpReplyQueue,
    opt: i32,
    pclass: u32,
    value: i64,
    user: *mut c_void,
    status: *mut VksHttpStatus,
) -> u64 {
    with_service!(q, status, |s| |q| s.set_option_long_dynamic(
        q,
        SendPtr(user),
        opt,
        pclass,
        value
    ))
}

/// `HttpRequest::setPolicyOption` with a string.
///
/// # Safety
/// `q` must be valid; `value` readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_request_set_string(
    q: *mut VksHttpReplyQueue,
    opt: i32,
    pclass: u32,
    value: *const c_char,
    len: usize,
    user: *mut c_void,
    status: *mut VksHttpStatus,
) -> u64 {
    let v = lossy(unsafe { bytes(value, len) });
    with_service!(q, status, |s| |q| s.set_option_string_dynamic(
        q,
        SendPtr(user),
        opt,
        pclass,
        &v
    ))
}

/// The next completed operation on the queue, or null.
///
/// # Safety
/// `q` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_reply_fetch(q: *mut VksHttpReplyQueue) -> *mut VksHttpReply {
    if q.is_null() {
        return std::ptr::null_mut();
    }
    let q = unsafe { &*q };
    catch_unwind(AssertUnwindSafe(|| match q.inner.fetch() {
        Some(r) => Box::into_raw(Box::new(VksHttpReply { inner: r })),
        None => std::ptr::null_mut(),
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// Releases a reply. Null is ignored.
///
/// # Safety
/// `r` must come from `vks_http_reply_fetch` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_reply_free(r: *mut VksHttpReply) {
    if !r.is_null() {
        drop(unsafe { Box::from_raw(r) });
    }
}

macro_rules! reply_get {
    ($name:ident, $ret:ty, |$r:ident| $body:expr) => {
        /// A field of a reply; see the C header.
        ///
        /// # Safety
        /// `r` must come from `vks_http_reply_fetch` and be live.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(r: *const VksHttpReply) -> $ret {
            let $r = &unsafe { &*r }.inner;
            $body
        }
    };
}

reply_get!(vks_http_reply_handle, u64, |r| r.handle);
reply_get!(vks_http_reply_user, *mut c_void, |r| r.user.0);
reply_get!(vks_http_reply_kind, i32, |r| match r.kind {
    ReplyKind::Request => 0,
    ReplyKind::Cancel => 1,
    ReplyKind::SetGet => 2,
    ReplyKind::Stop => 3,
    ReplyKind::NoOp => 4,
});
reply_get!(vks_http_reply_status, VksHttpStatus, |r| r.status.into());
reply_get!(vks_http_reply_has_headers, i32, |r| r.headers.is_some()
    as i32);
reply_get!(vks_http_reply_header_count, usize, |r| r
    .headers
    .as_ref()
    .map_or(0, |h| h.len()));
reply_get!(vks_http_reply_retries, u32, |r| r.retries);
reply_get!(vks_http_reply_retries_503, u32, |r| r.retries_503);
reply_get!(vks_http_reply_long, i64, |r| r.long_value);

/// The reply body: a pointer valid until the reply is freed, and its length.
///
/// # Safety
/// `r` must be live; `len` must be null or valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_reply_body(r: *const VksHttpReply, len: *mut usize) -> *const u8 {
    let r = &unsafe { &*r }.inner;
    if !len.is_null() {
        unsafe { *len = r.body.len() };
    }
    r.body.as_ptr()
}

/// Header `i` of the reply (normalised name, value).
///
/// # Safety
/// `r` must be live; `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_reply_header(
    r: *const VksHttpReply,
    i: usize,
    out: *mut VksHttpHeader,
) -> i32 {
    let r = &unsafe { &*r }.inner;
    let Some((n, v)) = r.headers.as_ref().and_then(|h| h.get(i)) else {
        return 0;
    };
    unsafe {
        *out = VksHttpHeader {
            name: n.as_ptr() as *const c_char,
            name_len: n.len(),
            value: v.as_ptr() as *const c_char,
            value_len: v.len(),
        };
    }
    1
}

unsafe fn out_str(s: &str, len: *mut usize) -> *const c_char {
    if !len.is_null() {
        unsafe { *len = s.len() };
    }
    s.as_ptr() as *const c_char
}

/// The reply's content type (`CURLINFO_CONTENT_TYPE`), not NUL-terminated.
///
/// # Safety
/// `r` must be live; `len` must be null or valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_reply_content_type(
    r: *const VksHttpReply,
    len: *mut usize,
) -> *const c_char {
    unsafe { out_str(&(*r).inner.content_type, len) }
}

/// The request URL as submitted.
///
/// # Safety
/// `r` must be live; `len` must be null or valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_reply_url(
    r: *const VksHttpReply,
    len: *mut usize,
) -> *const c_char {
    unsafe { out_str(&(*r).inner.url, len) }
}

/// The request method name (`GET`, `POST`, ...), empty for other operations.
///
/// # Safety
/// `r` must be live; `len` must be null or valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_reply_method(
    r: *const VksHttpReply,
    len: *mut usize,
) -> *const c_char {
    unsafe { out_str((*r).inner.method, len) }
}

/// The string value of an option get/set reply.
///
/// # Safety
/// `r` must be live; `len` must be null or valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_reply_string(
    r: *const VksHttpReply,
    len: *mut usize,
) -> *const c_char {
    unsafe { out_str(&(*r).inner.string_value, len) }
}

/// The reply's `Content-Range` result: returns 1 and fills the three values
/// when the response carried one, else 0.
///
/// # Safety
/// `r` must be live; the out pointers must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_reply_range(
    r: *const VksHttpReply,
    offset: *mut u64,
    length: *mut u64,
    full: *mut u64,
) -> i32 {
    let r = &unsafe { &*r }.inner;
    match r.range {
        Some((o, l, f)) => {
            unsafe {
                *offset = o;
                *length = l;
                *full = f;
            }
            1
        }
        None => 0,
    }
}

/// The transfer statistics libcurl reported for the request.
///
/// # Safety
/// `r` must be live; the out pointers must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_http_reply_stats(
    r: *const VksHttpReply,
    size_download: *mut f64,
    total_time: *mut f64,
    speed_download: *mut f64,
) {
    let r = &unsafe { &*r }.inner;
    unsafe {
        *size_download = r.stats.size_download;
        *total_time = r.stats.total_time;
        *speed_download = r.stats.speed_download;
    }
}

/// `HttpStatus::isRetryable`.
#[unsafe(no_mangle)]
pub extern "C" fn vks_http_status_is_retryable(kind: u16, code: i16) -> i32 {
    Status { kind, code }.is_retryable() as i32
}

/// `curl_version()` as the crate's libcurl reports it, NUL-terminated and
/// static.
#[unsafe(no_mangle)]
pub extern "C" fn vks_http_curl_version() -> *const c_char {
    static V: std::sync::OnceLock<std::ffi::CString> = std::sync::OnceLock::new();
    V.get_or_init(|| std::ffi::CString::new(vkstorm_http::sys::version()).unwrap_or_default())
        .as_ptr()
}
