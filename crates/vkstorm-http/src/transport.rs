//! `HttpLibcurl`: the libcurl transport. One multi handle per policy class,
//! a cache of easy handles (the first one freed is the template the others
//! are duplicated from), the per-request option setup of
//! `HttpOpRequest::prepareRequest`, the libcurl callbacks, request
//! completion and cancellation, and the stall used to reconfigure a class's
//! multi handle while it has no active request.
//!
//! This module and [`crate::sys`] hold all of the crate's `unsafe`: every
//! libcurl call, and the callbacks that get their request record back from
//! libcurl's user-data pointer.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_long, c_void, CStr, CString};
use std::sync::Arc;

use crate::headers::{escape_debug, header_line, parse_content_range, parse_retry_after, range_line, split_header, ContentRange};
use crate::hooks::{Hooks, LogLevel, Stat};
use crate::options::{PolicyOption, REDIRECTS_DEFAULT, REQUEST_TIMEOUT_DEFAULT, REQUEST_TIMEOUT_MAX, REQUEST_TIMEOUT_MIN, REQUEST_XFER_TIMEOUT_DEFAULT, TRACE_CURL_BODIES, TRACE_CURL_HEADERS, TRACE_OFF};
use crate::policy::{LoopSpeed, Policy, CANCELED};
use crate::request::{Method, RequestOp, PF_SAVE_HEADERS, PF_SCAN_RANGE_HEADER, PF_USE_RETRY_AFTER};
use crate::status::{he, Status};
use crate::sys::{self, SendPtr, CURL, CURLM};

/// `HttpLibcurl::HandleCache`.
struct HandleCache {
    template: *mut CURL,
    cache: Vec<*mut CURL>,
}

impl HandleCache {
    fn new() -> HandleCache {
        HandleCache { template: std::ptr::null_mut(), cache: Vec::with_capacity(50) }
    }

    fn get(&mut self) -> *mut CURL {
        if let Some(h) = self.cache.pop() {
            h
        } else if !self.template.is_null() {
            unsafe { sys::curl_easy_duphandle(self.template) }
        } else {
            unsafe { sys::curl_easy_init() }
        }
    }

    fn free(&mut self, handle: *mut CURL) {
        if handle.is_null() {
            return;
        }
        unsafe { sys::curl_easy_reset(handle) };
        if self.template.is_null() {
            self.template = handle;
        } else {
            self.cache.push(handle);
        }
    }
}

impl Drop for HandleCache {
    fn drop(&mut self) {
        if !self.template.is_null() {
            unsafe { sys::curl_easy_cleanup(self.template) };
        }
        for h in self.cache.drain(..) {
            unsafe { sys::curl_easy_cleanup(h) };
        }
    }
}

pub struct Transport {
    hooks: Arc<dyn Hooks>,
    multis: Vec<*mut CURLM>,
    active_counts: Vec<i32>,
    dirty: Vec<bool>,
    active: HashMap<u64, Box<RequestOp>>,
    cache: HandleCache,
    user_agent: CString,
    warned_unexpected_msg: bool,
}

// SAFETY: the libcurl handles are created, used and freed by the service
// thread only; the transport moves to that thread with the worker state
// and never runs on two threads at once.
unsafe impl Send for Transport {}

fn check_easy(code: sys::CURLcode, option: sys::CURLoption, hooks: &dyn Hooks) {
    if code != sys::CURLE_OK {
        hooks.log(LogLevel::Warn, &format!("libcurl error detected:  {}, curl_easy_setopt option:  {}", sys::easy_strerror(code), option));
    }
}

fn check_multi(code: sys::CURLMcode, option: Option<sys::CURLMoption>, hooks: &dyn Hooks) {
    if code != sys::CURLM_OK {
        match option {
            Some(o) => hooks.log(LogLevel::Warn, &format!("libcurl multi error detected:  {}, curl_multi_setopt option:  {}", sys::multi_strerror(code), o)),
            None => hooks.log(LogLevel::Warn, &format!("libcurl multi error detected:  {}", sys::multi_strerror(code))),
        }
    }
}

unsafe fn setopt_long(h: *mut CURL, opt: sys::CURLoption, v: c_long, hooks: &dyn Hooks) {
    check_easy(unsafe { sys::curl_easy_setopt(h, opt, v) }, opt, hooks);
}

unsafe fn setopt_ptr(h: *mut CURL, opt: sys::CURLoption, v: *const c_void, hooks: &dyn Hooks) {
    check_easy(unsafe { sys::curl_easy_setopt(h, opt, v) }, opt, hooks);
}

unsafe fn setopt_str(h: *mut CURL, opt: sys::CURLoption, v: &CStr, hooks: &dyn Hooks) {
    check_easy(unsafe { sys::curl_easy_setopt(h, opt, v.as_ptr()) }, opt, hooks);
}

unsafe fn msetopt_long(m: *mut CURLM, opt: sys::CURLMoption, v: c_long, hooks: &dyn Hooks) {
    check_multi(unsafe { sys::curl_multi_setopt(m, opt, v) }, Some(opt), hooks);
}

fn slist_append(list: *mut sys::curl_slist, line: &str) -> *mut sys::curl_slist {
    let c = CString::new(line.replace('\0', "")).unwrap_or_default();
    unsafe { sys::curl_slist_append(list, c.as_ptr()) }
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

impl Transport {
    pub fn new(hooks: Arc<dyn Hooks>, user_agent: &str) -> Transport {
        Transport {
            hooks,
            multis: Vec::new(),
            active_counts: Vec::new(),
            dirty: Vec::new(),
            active: HashMap::new(),
            cache: HandleCache::new(),
            user_agent: cstr(user_agent),
            warned_unexpected_msg: false,
        }
    }

    pub fn started(&self) -> bool {
        !self.multis.is_empty()
    }

    /// `HttpLibcurl::start`: one multi handle per policy class, configured
    /// from the class options.
    pub fn start(&mut self, policy_count: usize, policy: &mut Policy) {
        debug_assert!(self.multis.is_empty());
        self.multis.clear();
        self.active_counts = vec![0; policy_count];
        self.dirty = vec![false; policy_count];
        for pclass in 0..policy_count {
            let m = unsafe { sys::curl_multi_init() };
            if m.is_null() {
                self.hooks.log(LogLevel::Warn, "Failed to allocate multi handle in libcurl.");
            }
            self.multis.push(m);
            self.policy_updated(pclass as u32, policy);
        }
    }

    /// `HttpLibcurl::processTransport`: gives libcurl its cycles for every
    /// class with active requests and completes what finished.
    pub fn process(&mut self, policy: &mut Policy, now: u64) -> LoopSpeed {
        let mut ret = LoopSpeed::RequestSleep;
        for pclass in 0..self.multis.len() {
            let multi = self.multis[pclass];
            if multi.is_null() {
                continue;
            }
            if self.active_counts[pclass] == 0 {
                if self.dirty[pclass] {
                    self.policy_updated(pclass as u32, policy);
                }
                continue;
            }
            loop {
                let mut running: c_int = 0;
                let status = unsafe { sys::curl_multi_perform(multi, &mut running) };
                if !(running != 0 && status == sys::CURLM_CALL_MULTI_PERFORM) {
                    break;
                }
            }
            loop {
                let mut in_queue: c_int = 0;
                let msg = unsafe { sys::curl_multi_info_read(multi, &mut in_queue) };
                if msg.is_null() {
                    break;
                }
                let (kind, easy, result) = unsafe { ((*msg).msg, (*msg).easy_handle, (*msg).data.result) };
                if kind == sys::CURLMSG_DONE {
                    self.complete_request(multi, easy, result, policy, now);
                    ret = LoopSpeed::Normal;
                } else if !self.warned_unexpected_msg {
                    self.warned_unexpected_msg = true;
                    self.hooks.log(LogLevel::Warn, &format!("Unexpected message from libcurl.  Msg code:  {kind}"));
                }
            }
        }
        if !self.active.is_empty() {
            ret = LoopSpeed::Normal;
        }
        ret
    }

    /// `HttpLibcurl::addOp`: prepares the handle and makes the request
    /// live. A request that cannot be prepared or added is delivered as
    /// failed (the C++ dropped it without a reply).
    pub fn add_op(&mut self, mut op: Box<RequestOp>, policy: &Policy, hooks: &dyn Hooks) {
        let pclass = op.policy() as usize;
        if pclass >= self.multis.len() || self.multis[pclass].is_null() {
            op.status = Status::llcore(he::BAD_ALLOC);
            deliver(op, hooks);
            return;
        }
        let status = self.prepare(&mut op, policy);
        if !status.is_ok() {
            op.status = status;
            deliver(op, hooks);
            return;
        }
        let code = unsafe { sys::curl_multi_add_handle(self.multis[pclass], op.curl.0) };
        if code != sys::CURLM_OK {
            check_multi(code, None, hooks);
            self.cache.free(op.curl.0);
            op.curl = SendPtr::null();
            op.status = Status::multi(code);
            deliver(op, hooks);
            return;
        }
        op.active = true;
        self.active_counts[pclass] += 1;
        if op.common.tracing > TRACE_OFF {
            hooks.log(
                LogLevel::Info,
                &format!(
                    "TRACE, ToActiveQueue, Handle:  {}, Actives:  {}, Readies:  {}",
                    op.handle(),
                    self.active.len() + 1,
                    policy.ready_count(op.policy())
                ),
            );
        }
        self.active.insert(op.handle(), op);
    }

    /// `HttpOpRequest::prepareRequest`: every libcurl option of a request,
    /// in the order the C++ set them.
    fn prepare(&mut self, op: &mut RequestOp, policy: &Policy) -> Status {
        let hooks: &dyn Hooks = &*self.hooks;
        op.active = false;
        op.curl = SendPtr::null();
        if !op.slist.is_null() {
            unsafe { sys::curl_slist_free_all(op.slist.0 as *mut sys::curl_slist) };
            op.slist = SendPtr::null();
        }
        op.body_pos = 0;
        op.reply_body = Vec::new();
        op.reply_offset = 0;
        op.reply_length = 0;
        op.reply_full_length = 0;
        op.reply_headers = None;
        op.content_type.clear();
        // The status of a previous attempt is not the status of this one;
        // the C++ kept it, so a retry could not succeed unless the header
        // callback happened to reset it (see the rung notes).
        op.status = Status::OK;
        op.ssl.result_type = 0;
        op.ssl.result_code = 0;
        op.hooks = Some(self.hooks.clone());

        let global = &policy.global;
        let class = policy.class_options(op.policy());
        let h = self.cache.get();
        if h.is_null() {
            hooks.log(LogLevel::Warn, "Failed to allocate libcurl easy handle.  Continuing.");
            return Status::llcore(he::BAD_ALLOC);
        }
        op.curl = SendPtr(h);
        let op_ptr = op as *mut RequestOp as *mut c_void;
        op.ssl.url = op.url_c.as_ptr();
        unsafe {
            setopt_long(h, sys::CURLOPT_IPRESOLVE, sys::CURL_IPRESOLVE_V4, hooks);
            setopt_long(h, sys::CURLOPT_NOSIGNAL, 1, hooks);
            setopt_long(h, sys::CURLOPT_NOPROGRESS, 1, hooks);
            setopt_str(h, sys::CURLOPT_URL, &op.url_c, hooks);
            setopt_ptr(h, sys::CURLOPT_PRIVATE, op_ptr, hooks);
            // libcurl before 7.81 (the viewer's is 7.54.1): ask for every
            // supported content encoding, as the C++ did for that version.
            setopt_str(h, sys::CURLOPT_ACCEPT_ENCODING, c"", hooks);
            setopt_long(h, sys::CURLOPT_AUTOREFERER, 1, hooks);
            setopt_long(h, sys::CURLOPT_MAXREDIRS, REDIRECTS_DEFAULT as c_long, hooks);
            setopt_ptr(h, sys::CURLOPT_WRITEFUNCTION, write_callback as *const c_void, hooks);
            setopt_ptr(h, sys::CURLOPT_WRITEDATA, op_ptr, hooks);
            setopt_ptr(h, sys::CURLOPT_READFUNCTION, read_callback as *const c_void, hooks);
            setopt_ptr(h, sys::CURLOPT_READDATA, op_ptr, hooks);
            setopt_ptr(h, sys::CURLOPT_SEEKFUNCTION, seek_callback as *const c_void, hooks);
            setopt_ptr(h, sys::CURLOPT_SEEKDATA, op_ptr, hooks);
            setopt_str(h, sys::CURLOPT_COOKIEFILE, c"", hooks);
            setopt_str(h, sys::CURLOPT_USERAGENT, &self.user_agent, hooks);
            if global.ssl_ctx_callback {
                setopt_ptr(h, sys::CURLOPT_SSL_CTX_FUNCTION, ssl_ctx_callback as *const c_void, hooks);
                setopt_ptr(h, sys::CURLOPT_SSL_CTX_DATA, op_ptr, hooks);
            }
        }
        let mut follow_redirect: c_long = 1;
        let mut ssl_peer: c_long = 0;
        let mut ssl_host: c_long = 0;
        let mut dns_cache_timeout: c_long = -1;
        let mut nobody: c_long = 0;
        let mut last_modified: i64 = 0;
        if let Some(o) = &op.req.options {
            follow_redirect = o.follow_redirects as c_long;
            ssl_peer = o.verify_peer as c_long;
            ssl_host = if o.verify_host { 2 } else { 0 };
            dns_cache_timeout = o.dns_cache_timeout as c_long;
            nobody = o.headers_only as c_long;
            last_modified = o.last_modified;
        }
        unsafe {
            setopt_long(h, sys::CURLOPT_FOLLOWLOCATION, follow_redirect, hooks);
            setopt_long(h, sys::CURLOPT_SSL_VERIFYPEER, ssl_peer, hooks);
            setopt_long(h, sys::CURLOPT_SSL_VERIFYHOST, ssl_host, hooks);
            setopt_long(h, sys::CURLOPT_NOBODY, nobody, hooks);
            if last_modified != 0 {
                setopt_long(h, sys::CURLOPT_TIMECONDITION, sys::CURL_TIMECOND_IFMODSINCE, hooks);
                setopt_long(h, sys::CURLOPT_TIMEVALUE, last_modified as c_long, hooks);
            }
            setopt_long(h, sys::CURLOPT_DNS_CACHE_TIMEOUT, dns_cache_timeout, hooks);
        }
        if global.use_llproxy != 0 {
            hooks.apply_proxy(h);
        } else if !global.http_proxy.is_empty() {
            unsafe {
                setopt_str(h, sys::CURLOPT_PROXY, &cstr(&global.http_proxy), hooks);
                setopt_long(h, sys::CURLOPT_PROXYTYPE, sys::CURLPROXY_HTTP, hooks);
            }
        }
        if !global.ca_path.is_empty() {
            unsafe { setopt_str(h, sys::CURLOPT_CAPATH, &cstr(&global.ca_path), hooks) };
        }
        if !global.ca_file.is_empty() {
            unsafe { setopt_str(h, sys::CURLOPT_CAINFO, &cstr(&global.ca_file), hooks) };
        }
        let mut slist: *mut sys::curl_slist = std::ptr::null_mut();
        let body_size = op.req.body.as_ref().map_or(0, |b| b.len()) as c_long;
        match op.req.method {
            Method::Get => {
                if nobody == 0 {
                    unsafe { setopt_long(h, sys::CURLOPT_HTTPGET, 1, hooks) };
                }
            }
            Method::Post => unsafe {
                setopt_long(h, sys::CURLOPT_POST, 1, hooks);
                setopt_str(h, sys::CURLOPT_ACCEPT_ENCODING, c"", hooks);
                setopt_ptr(h, sys::CURLOPT_POSTFIELDS, std::ptr::null(), hooks);
                setopt_long(h, sys::CURLOPT_POSTFIELDSIZE, body_size, hooks);
                slist = slist_append(slist, "Expect:");
            },
            Method::Patch | Method::Put => unsafe {
                if op.req.method == Method::Patch {
                    setopt_str(h, sys::CURLOPT_CUSTOMREQUEST, c"PATCH", hooks);
                }
                setopt_long(h, sys::CURLOPT_UPLOAD, 1, hooks);
                setopt_long(h, sys::CURLOPT_INFILESIZE, body_size, hooks);
                slist = slist_append(slist, "Expect:");
            },
            Method::Delete => unsafe { setopt_str(h, sys::CURLOPT_CUSTOMREQUEST, c"DELETE", hooks) },
            Method::Copy => unsafe { setopt_str(h, sys::CURLOPT_CUSTOMREQUEST, c"COPY", hooks) },
            Method::Move => unsafe { setopt_str(h, sys::CURLOPT_CUSTOMREQUEST, c"MOVE", hooks) },
        }
        slist = slist_append(slist, "Connection: keep-alive");
        slist = slist_append(slist, "Keep-alive: 300");
        if op.common.tracing >= TRACE_CURL_HEADERS {
            unsafe {
                setopt_long(h, sys::CURLOPT_VERBOSE, 1, hooks);
                setopt_ptr(h, sys::CURLOPT_DEBUGDATA, op_ptr, hooks);
                setopt_ptr(h, sys::CURLOPT_DEBUGFUNCTION, debug_callback as *const c_void, hooks);
            }
        }
        if (op.req.offset != 0 || op.req.length != 0) && op.req.method == Method::Get {
            slist = slist_append(slist, &range_line(op.req.offset, op.req.length));
        }
        slist = slist_append(slist, "Pragma:");
        let mut timeout = REQUEST_TIMEOUT_DEFAULT;
        let mut xfer_timeout = REQUEST_XFER_TIMEOUT_DEFAULT;
        if let Some(o) = &op.req.options {
            timeout = o.timeout.clamp(REQUEST_TIMEOUT_MIN, REQUEST_TIMEOUT_MAX);
            xfer_timeout = o.transfer_timeout.clamp(REQUEST_TIMEOUT_MIN, REQUEST_TIMEOUT_MAX);
        }
        if xfer_timeout == 0 {
            xfer_timeout = timeout;
        }
        if class.pipelining > 1 {
            // Pipelined requests wait behind each other: twice the transfer
            // timeout. HTTP/2 only under the VIEWERASSET override, as before.
            xfer_timeout *= 2;
            if std::env::var_os("VIEWERASSET").is_some() {
                unsafe { setopt_long(h, sys::CURLOPT_HTTP_VERSION, sys::CURL_HTTP_VERSION_2_0, hooks) };
            }
        }
        unsafe {
            setopt_long(h, sys::CURLOPT_TIMEOUT, xfer_timeout as c_long, hooks);
            setopt_long(h, sys::CURLOPT_CONNECTTIMEOUT, timeout as c_long, hooks);
        }
        for (name, value) in &op.req.headers {
            slist = slist_append(slist, &header_line(name, value));
        }
        op.slist = SendPtr(slist as *mut c_void);
        unsafe { setopt_ptr(h, sys::CURLOPT_HTTPHEADER, slist as *const c_void, hooks) };
        if op.proc_flags & (PF_SCAN_RANGE_HEADER | PF_SAVE_HEADERS | PF_USE_RETRY_AFTER) != 0 {
            unsafe {
                setopt_ptr(h, sys::CURLOPT_HEADERFUNCTION, header_callback as *const c_void, hooks);
                setopt_ptr(h, sys::CURLOPT_HEADERDATA, op_ptr, hooks);
            }
        }
        Status::OK
    }

    /// `HttpLibcurl::cancel`: an active request is removed from libcurl and
    /// delivered as cancelled.
    pub fn cancel(&mut self, handle: u64) -> bool {
        let Some(op) = self.active.remove(&handle) else {
            return false;
        };
        let pclass = op.policy() as usize;
        self.active_counts[pclass] -= 1;
        self.cancel_request(op);
        true
    }

    fn cancel_request(&mut self, mut op: Box<RequestOp>) {
        op.active = false;
        let pclass = op.policy() as usize;
        unsafe { sys::curl_multi_remove_handle(self.multis[pclass], op.curl.0) };
        self.cache.free(op.curl.0);
        op.curl = SendPtr::null();
        if op.common.tracing > TRACE_OFF {
            self.hooks.log(LogLevel::Info, &format!("TRACE, RequestCanceled, Handle:  {}, Status:  {}", op.handle(), op.status.terse()));
        }
        op.status = CANCELED;
        deliver_cancelled(op);
    }

    /// `HttpLibcurl::completeRequest`.
    fn complete_request(&mut self, multi: *mut CURLM, handle: *mut CURL, result: sys::CURLcode, policy: &mut Policy, now: u64) {
        let hooks = self.hooks.clone();
        let mut priv_ptr: *mut c_void = std::ptr::null_mut();
        let ccode = unsafe { sys::curl_easy_getinfo(handle, sys::CURLINFO_PRIVATE, &mut priv_ptr) };
        if ccode != sys::CURLE_OK || priv_ptr.is_null() {
            hooks.log(LogLevel::Warn, &format!("libcurl error: {ccode} Unable to retrieve operation handle from CURL handle"));
            return;
        }
        // SAFETY: the pointer is the request record we gave libcurl, kept
        // alive and unaliased in the active map while the request runs.
        let id = unsafe { (*(priv_ptr as *const RequestOp)).handle() };
        let Some(mut op) = self.active.remove(&id) else {
            hooks.log(LogLevel::Warn, &format!("libcurl completion for request not on active list.  Continuing.  Handle:  {handle:?}"));
            return;
        };
        if op.curl.0 != handle || !op.active {
            hooks.log(LogLevel::Warn, &format!("libcurl handle and HttpOpRequest handle in disagreement or inactive request.  Handle:  {handle:?}"));
            self.active.insert(id, op);
            return;
        }
        let pclass = op.policy() as usize;
        self.active_counts[pclass] -= 1;
        op.active = false;

        // The verification callback's decision, when it failed the request.
        if op.ssl.result_code != 0 {
            op.status = Status { kind: op.ssl.result_type, code: op.ssl.result_code };
        }
        if op.status.is_ok() {
            op.status = Status::easy(result);
        }
        if op.status.is_ok() {
            let mut http_status: c_long = 200;
            let ccode = unsafe { sys::curl_easy_getinfo(handle, sys::CURLINFO_RESPONSE_CODE, &mut http_status) };
            if ccode == sys::CURLE_OK {
                if (100..=999).contains(&http_status) {
                    let mut cont_type: *const c_char = std::ptr::null();
                    let ccode = unsafe { sys::curl_easy_getinfo(handle, sys::CURLINFO_CONTENT_TYPE, &mut cont_type) };
                    if ccode == sys::CURLE_OK {
                        if !cont_type.is_null() {
                            op.content_type = unsafe { CStr::from_ptr(cont_type) }.to_string_lossy().into_owned();
                        }
                    } else {
                        hooks.log(LogLevel::Warn, &format!("CURL error:{ccode} Attempting to get content type."));
                    }
                    op.status = Status::http(http_status as i32);
                } else {
                    hooks.log(LogLevel::Warn, &format!("Invalid HTTP response code ({http_status}) received from server."));
                    op.status = Status::llcore(he::INVALID_HTTP_STATUS);
                }
            } else {
                op.status = Status::llcore(he::INVALID_HTTP_STATUS);
            }
        }
        // The transfer statistics, read while the handle still holds them.
        unsafe {
            sys::curl_easy_getinfo(handle, sys::CURLINFO_SIZE_DOWNLOAD, &mut op.stats.size_download);
            sys::curl_easy_getinfo(handle, sys::CURLINFO_TOTAL_TIME, &mut op.stats.total_time);
            sys::curl_easy_getinfo(handle, sys::CURLINFO_SPEED_DOWNLOAD, &mut op.stats.speed_download);
        }

        // Pipelining sanity checks: an X-LL-URL header naming another
        // request's URL, or a partial reply for a range other than the one
        // asked, mean the pipeline went out of sync; fail with a 503 (which
        // retries) and turn pipelining off on every class.
        let mut failed = false;
        if !op.xll_url.is_empty() {
            let mut uri = op.req.url.clone();
            if let Some(i) = uri.find("://")
                && let Some(j) = uri[i + 3..].find('/') {
                    uri = uri[i + 3 + j..].to_owned();
                }
            if uri != op.xll_url {
                hooks.log(LogLevel::Warn, &format!("HTTP pipelining out of sync! Asked for: {} got {}", uri, op.xll_url));
                op.status = Status::http(503);
                failed = true;
            }
        }
        if !failed && (op.req.offset != 0 || op.req.length != 0)
            && op.status == Status::http(206) && (op.req.offset != op.reply_offset || (op.req.length != 0 && op.req.length < op.reply_length)) {
                let wanted_end = if op.req.length != 0 { format!("{}", op.req.offset + op.req.length - 1) } else { String::new() };
                hooks.log(
                    LogLevel::Warn,
                    &format!(
                        "HTTP pipelining possibly out of sync, request wanted: {}-{} got: {}-{} url: {}",
                        op.req.offset,
                        wanted_end,
                        op.reply_offset,
                        op.reply_offset.wrapping_add(op.reply_length).wrapping_sub(1),
                        op.req.url
                    ),
                );
                op.status = Status::http(503);
                failed = true;
            }
        if failed {
            for i in 0..self.multis.len() {
                let options = policy.class_options_mut(i as u32);
                if options.pipelining != 0 {
                    options.set(PolicyOption::PipeliningDepth, 0);
                    self.dirty[i] = true;
                    self.policy_updated(i as u32, policy);
                }
            }
        }
        if !multi.is_null() && !handle.is_null() {
            unsafe { sys::curl_multi_remove_handle(multi, handle) };
            self.cache.free(op.curl.0);
        } else {
            hooks.log(LogLevel::Warn, &format!("Curl multi_handle or handle is NULL on remove! multi:{multi:?} h:{handle:?}"));
        }
        op.curl = SendPtr::null();
        if op.common.tracing > TRACE_OFF {
            hooks.log(LogLevel::Info, &format!("TRACE, RequestComplete, Handle:  {}, Status:  {}", op.handle(), op.status.terse()));
        }
        if let Some(op) = policy.stage_after_completion(op, now, &*hooks) {
            deliver(op, &*hooks);
        }
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// `HttpLibcurl::getActiveCountInClass`.
    pub fn active_count_in_class(&self, pclass: u32) -> i32 {
        self.active_counts.get(pclass as usize).copied().unwrap_or(0)
    }

    /// `HttpLibcurl::policyUpdated`: applies the class's connection and
    /// pipelining options to its multi handle when it has no active
    /// request; otherwise marks it dirty and stalls the class until it has
    /// none (libcurl 7.37 corrupted memory when a pipelining multi handle
    /// with active requests was switched off).
    pub fn policy_updated(&mut self, pclass: u32, policy: &mut Policy) {
        let i = pclass as usize;
        if i >= self.multis.len() {
            return;
        }
        let hooks = self.hooks.clone();
        if self.active_counts[i] == 0 {
            let options = policy.class_options(pclass).clone();
            let multi = self.multis[i];
            policy.stall_policy(pclass, false);
            self.dirty[i] = false;
            unsafe {
                if options.pipelining > 1 {
                    msetopt_long(multi, sys::CURLMOPT_PIPELINING, 1, &*hooks);
                    msetopt_long(multi, sys::CURLMOPT_MAX_PIPELINE_LENGTH, options.pipelining as c_long, &*hooks);
                    msetopt_long(multi, sys::CURLMOPT_MAX_HOST_CONNECTIONS, options.per_host_connection_limit as c_long, &*hooks);
                    msetopt_long(multi, sys::CURLMOPT_MAX_TOTAL_CONNECTIONS, options.connection_limit as c_long, &*hooks);
                } else {
                    msetopt_long(multi, sys::CURLMOPT_PIPELINING, 0, &*hooks);
                    msetopt_long(multi, sys::CURLMOPT_MAX_HOST_CONNECTIONS, 0, &*hooks);
                    msetopt_long(multi, sys::CURLMOPT_MAX_TOTAL_CONNECTIONS, options.connection_limit as c_long, &*hooks);
                }
            }
        } else if !self.dirty[i] {
            self.dirty[i] = true;
            policy.stall_policy(pclass, true);
        }
    }

    /// `HttpLibcurl::shutdown`: cancels every active request and frees the
    /// multi handles.
    pub fn shutdown(&mut self) {
        let ids: Vec<u64> = self.active.keys().copied().collect();
        for id in ids {
            if let Some(op) = self.active.remove(&id) {
                let pclass = op.policy() as usize;
                self.active_counts[pclass] -= 1;
                self.cancel_request(op);
            }
        }
        for m in self.multis.drain(..) {
            if !m.is_null() {
                unsafe { sys::curl_multi_cleanup(m) };
            }
        }
        self.active_counts.clear();
        self.dirty.clear();
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// `HttpOpRequest::stageFromActive` then `visitNotifier`: the reply of a
/// finished request, after the check that a `Content-Range` reply carried
/// as many bytes as it declared.
pub fn deliver(mut op: Box<RequestOp>, _hooks: &dyn Hooks) {
    if op.reply_length != 0 && !op.reply_body.is_empty() && op.reply_length != op.reply_body.len() as u64 {
        op.status = Status::llcore(he::INV_CONTENT_RANGE_HDR);
    }
    if !op.slist.is_null() {
        unsafe { sys::curl_slist_free_all(op.slist.0 as *mut sys::curl_slist) };
        op.slist = SendPtr::null();
    }
    if op.common.tracing > TRACE_OFF
        && let Some(h) = &op.hooks {
            h.log(LogLevel::Info, &format!("TRACE, ToReplyQueue, Handle:  {}", op.handle()));
        }
    push_reply(op);
}

/// `HttpOpRequest::cancel`: the reply of a cancelled request, no checks.
pub fn deliver_cancelled(mut op: Box<RequestOp>) {
    if !op.slist.is_null() {
        unsafe { sys::curl_slist_free_all(op.slist.0 as *mut sys::curl_slist) };
        op.slist = SendPtr::null();
    }
    op.status = CANCELED;
    push_reply(op);
}

fn push_reply(op: Box<RequestOp>) {
    match op.common.reply.clone() {
        Some(q) => q.push(op.into_reply()),
        None => {
            if let Some(h) = &op.hooks {
                h.release_user(op.common.user);
            }
        }
    }
}

impl Drop for RequestOp {
    fn drop(&mut self) {
        if !self.slist.is_null() {
            unsafe { sys::curl_slist_free_all(self.slist.0 as *mut sys::curl_slist) };
            self.slist = SendPtr::null();
        }
        if !self.curl.is_null() {
            // A record dropped while it still owns a handle (never while
            // active): the handle goes with it.
            unsafe { sys::curl_easy_cleanup(self.curl.0) };
            self.curl = SendPtr::null();
        }
    }
}

// --- libcurl callbacks --------------------------------------------------

unsafe fn op_from<'a>(userdata: *mut c_void) -> &'a mut RequestOp {
    // SAFETY: libcurl passes back the request record pointer set as the
    // callback data; the record is alive and unaliased during
    // curl_multi_perform, the only place these callbacks run.
    unsafe { &mut *(userdata as *mut RequestOp) }
}

unsafe extern "C" fn write_callback(data: *mut c_char, size: usize, nmemb: usize, userdata: *mut c_void) -> usize {
    let op = unsafe { op_from(userdata) };
    let n = size * nmemb;
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, n) };
    op.reply_body.extend_from_slice(bytes);
    if let Some(h) = &op.hooks {
        h.stat(Stat::DataDown, n as u64);
    }
    n
}

unsafe extern "C" fn read_callback(data: *mut c_char, size: usize, nmemb: usize, userdata: *mut c_void) -> usize {
    let op = unsafe { op_from(userdata) };
    let Some(body) = &op.req.body else {
        return 0;
    };
    let req_size = size * nmemb;
    if body.len() <= op.body_pos {
        if body.len() < op.body_pos
            && let Some(h) = &op.hooks {
                h.log(LogLevel::Warn, "Request body position beyond body size.  Truncating request body.");
            }
        return 0;
    }
    let do_size = req_size.min(body.len() - op.body_pos);
    unsafe { std::ptr::copy_nonoverlapping(body.as_ptr().add(op.body_pos), data as *mut u8, do_size) };
    if let Some(h) = &op.hooks {
        h.stat(Stat::DataUp, do_size as u64);
    }
    op.body_pos += do_size;
    do_size
}

unsafe extern "C" fn seek_callback(userdata: *mut c_void, offset: sys::curl_off_t, origin: c_int) -> c_int {
    let op = unsafe { op_from(userdata) };
    let Some(body) = &op.req.body else {
        return sys::CURL_SEEKFUNC_OK;
    };
    const SEEK_SET: c_int = 0;
    const SEEK_CUR: c_int = 1;
    const SEEK_END: c_int = 2;
    let new_pos: i64 = match origin {
        SEEK_SET => offset,
        SEEK_END => body.len() as i64 + offset,
        SEEK_CUR => op.body_pos as i64 + offset,
        _ => return sys::CURL_SEEKFUNC_CANTSEEK,
    };
    // The C++ compared through size_t, so a negative position is huge.
    if new_pos as u64 as usize >= body.len() {
        if let Some(h) = &op.hooks {
            h.log(LogLevel::Warn, "Attempt to seek to position outside post body.");
        }
        return sys::CURL_SEEKFUNC_CANTSEEK;
    }
    op.body_pos = new_pos as usize;
    sys::CURL_SEEKFUNC_OK
}

unsafe extern "C" fn header_callback(data: *mut c_char, size: usize, nmemb: usize, userdata: *mut c_void) -> usize {
    let op = unsafe { op_from(userdata) };
    let hdr_size = size * nmemb;
    let hdr = unsafe { std::slice::from_raw_parts(data as *const u8, hdr_size) };
    let mut is_header = true;
    if hdr.starts_with(b"HTTP/") {
        // A status line (one of possibly several): start over, taking the
        // results from the last header stanza received.
        op.reply_offset = 0;
        op.reply_length = 0;
        op.reply_full_length = 0;
        op.retry_after = 0;
        op.status = Status::OK;
        op.ssl.result_type = 0;
        op.ssl.result_code = 0;
        if let Some(h) = &mut op.reply_headers {
            h.clear();
        }
        is_header = false;
    }
    let Some((name, value)) = split_header(hdr) else {
        return hdr_size;
    };
    if let Some(v) = &value
        && name == "x-ll-url" {
            op.xll_url = v.clone();
        }
    if is_header && op.proc_flags & PF_SAVE_HEADERS != 0 {
        op.reply_headers.get_or_insert_with(Vec::new).push((name.clone(), value.clone().unwrap_or_default()));
    }
    if is_header && op.proc_flags & PF_SCAN_RANGE_HEADER != 0 && name == "content-range"
        && let Some(v) = value.as_deref().filter(|v| !v.is_empty()) {
            match parse_content_range(v) {
                ContentRange::Valid { first, last, length } => {
                    op.reply_offset = first as u64;
                    op.reply_length = (last - first + 1) as u64;
                    op.reply_full_length = length as u64;
                }
                ContentRange::Invalid => op.status = Status::llcore(he::INV_CONTENT_RANGE_HDR),
                ContentRange::Unparsed => {
                    if let Some(h) = &op.hooks {
                        h.log(LogLevel::Info, &format!("Problem parsing odd Content-Range header:  '{}'.  Ignoring.", String::from_utf8_lossy(hdr).trim_end()));
                    }
                }
            }
        }
    if is_header && op.proc_flags & PF_USE_RETRY_AFTER != 0 && name == "retry-after"
        && let Some(v) = value.as_deref().filter(|v| !v.is_empty())
            && let Some(t) = parse_retry_after(v) {
                op.retry_after = t;
            }
    hdr_size
}

unsafe extern "C" fn ssl_ctx_callback(_curl: *mut CURL, ssl_ctx: *mut c_void, userdata: *mut c_void) -> sys::CURLcode {
    let op = unsafe { op_from(userdata) };
    if let Some(h) = op.hooks.clone() {
        h.ssl_ctx(ssl_ctx, &mut op.ssl);
    }
    sys::CURLE_OK
}

unsafe extern "C" fn debug_callback(_curl: *mut CURL, info: sys::curl_infotype, buffer: *mut c_char, len: usize, userdata: *mut c_void) -> c_int {
    let op = unsafe { op_from(userdata) };
    let Some(hooks) = op.hooks.clone() else {
        return 0;
    };
    let log_len = len.min(256);
    let data = unsafe { std::slice::from_raw_parts(buffer as *const u8, log_len) };
    let tracing = op.common.tracing;
    let (tag, line) = match info {
        sys::CURLINFO_TEXT if tracing >= TRACE_CURL_HEADERS => ("TEXT", escape_debug(data, true)),
        sys::CURLINFO_HEADER_IN if tracing >= TRACE_CURL_HEADERS => ("HEADERIN", escape_debug(data, true)),
        sys::CURLINFO_HEADER_OUT if tracing >= TRACE_CURL_HEADERS => ("HEADEROUT", escape_debug(data, true)),
        sys::CURLINFO_DATA_IN if tracing >= TRACE_CURL_HEADERS => {
            ("DATAIN", if tracing >= TRACE_CURL_BODIES { escape_debug(data, false) } else { format!("{len} Bytes") })
        }
        sys::CURLINFO_DATA_OUT if tracing >= TRACE_CURL_HEADERS => {
            ("DATAOUT", if tracing >= TRACE_CURL_BODIES { escape_debug(data, false) } else { format!("{len} Bytes") })
        }
        _ => return 0,
    };
    hooks.log(LogLevel::Info, &format!("TRACE, LibcurlDebug, Handle:  {}, Type:  {}, Data:  {}", op.handle(), tag, line));
    0
}
