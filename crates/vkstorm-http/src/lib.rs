//! The viewer's HTTP layer: the `LLCore::HttpRequest` service of
//! `indra/llcorehttp`, ported for the vkstorm-rust track (rung four).
//!
//! What is here: the request queue and the service thread
//! (`HttpService`), the per-class ready and retry queues with the
//! concurrency limits, request-rate throttle and retry backoff
//! (`HttpPolicy`, `HttpPolicyClass`, `HttpPolicyGlobal`, `HttpRetryQueue`),
//! the operations (`HttpOperation` and its subclasses, `HttpOpRequest`'s
//! libcurl setup and callbacks), the libcurl multi-handle transport with
//! its easy-handle cache and the pipelining stall (`HttpLibcurl`), the
//! reply queues, the status type (`HttpStatus`) and the header parsing.
//! Behaviour is that of the C++ it replaces, request for request on the
//! wire, with the differences stated in the rung notes of the viewer's
//! `doc/rust_port.md`.
//!
//! The transport is libcurl, the library the viewer stages (OpenSSL,
//! nghttp2, zlib-ng), through the hand-written bindings of [`sys`]; see
//! that module for why not `curl-sys`. The host supplies its proxy
//! settings, certificate verification, logging and statistics through
//! [`hooks::Hooks`]; the viewer does so over the C ABI of `vkstorm-ffi`.
//!
//! [`Client`] is the crate's own front door, used by its tests and benches:
//! one reply queue and the request methods of `HttpRequest`.

pub mod headers;
pub mod hooks;
pub mod options;
pub mod policy;
pub mod request;
pub mod service;
pub mod status;
pub mod sys;
pub mod transport;

use std::sync::Arc;

pub use hooks::{Hooks, LogLevel, NoHooks, Stat};
pub use options::{
    ClassOptions, DEFAULT_POLICY_ID, GLOBAL_POLICY_ID, GlobalOptions, INVALID_POLICY_ID,
    PolicyOption, RequestOptions,
};
pub use request::{Method, Reply, ReplyKind, ReplyQueue, Request, TransferStats};
pub use service::{Service, State};
pub use status::{EXT_CURL_EASY, EXT_CURL_MULTI, LLCORE, Status, he};
pub use sys::UserPtr;

/// The `User-Agent` the crate's own tests send.
pub const TEST_USER_AGENT: &str = "vkstorm-http";

/// An `HttpRequest` instance: its reply queue on a service.
pub struct Client<'a> {
    service: &'a Service,
    queue: Arc<ReplyQueue>,
    last_status: Status,
}

impl<'a> Client<'a> {
    pub fn new(service: &'a Service) -> Client<'a> {
        Client {
            service,
            queue: service.reply_queue(),
            last_status: Status::OK,
        }
    }

    /// `HttpRequest::getStatus`: the status of the last issue.
    pub fn status(&self) -> Status {
        self.last_status
    }

    fn issue(&mut self, r: Result<u64, Status>) -> Option<u64> {
        match r {
            Ok(h) => {
                self.last_status = Status::OK;
                Some(h)
            }
            Err(e) => {
                self.last_status = e;
                None
            }
        }
    }

    /// Any request method, by its [`Request`].
    pub fn request(&mut self, req: Request) -> Option<u64> {
        let r = self.service.submit(&self.queue, UserPtr::null(), req);
        self.issue(r)
    }

    pub fn get(&mut self, policy: u32, url: &str, options: Option<RequestOptions>) -> Option<u64> {
        let mut req = Request::new(policy, Method::Get, url);
        req.options = options;
        self.request(req)
    }

    pub fn get_range(
        &mut self,
        policy: u32,
        url: &str,
        offset: u64,
        length: u64,
        options: Option<RequestOptions>,
    ) -> Option<u64> {
        let mut req = Request::new(policy, Method::Get, url).range(offset, length);
        req.options = options;
        self.request(req)
    }

    pub fn post(
        &mut self,
        policy: u32,
        url: &str,
        body: Vec<u8>,
        options: Option<RequestOptions>,
    ) -> Option<u64> {
        let mut req = Request::new(policy, Method::Post, url).body(body);
        req.options = options;
        self.request(req)
    }

    pub fn cancel(&mut self, handle: u64) -> Option<u64> {
        let r = self.service.cancel(&self.queue, UserPtr::null(), handle);
        self.issue(r)
    }

    pub fn noop(&mut self) -> Option<u64> {
        let r = self.service.noop(&self.queue, UserPtr::null());
        self.issue(r)
    }

    pub fn stop_thread(&mut self) -> Option<u64> {
        let r = self.service.stop_thread(&self.queue, UserPtr::null());
        self.issue(r)
    }

    pub fn set_option_long(&mut self, opt: PolicyOption, pclass: u32, value: i64) -> Option<u64> {
        let r = self.service.set_option_long_dynamic(
            &self.queue,
            UserPtr::null(),
            opt as i32,
            pclass,
            value,
        );
        self.issue(r)
    }

    /// `HttpRequest::update(0)`: every reply delivered so far.
    pub fn update(&mut self) -> Vec<Reply> {
        self.queue.fetch_all()
    }

    /// One reply, if any.
    pub fn fetch(&mut self) -> Option<Reply> {
        self.queue.fetch()
    }

    /// Polls until `count` replies have arrived or `timeout` passed.
    pub fn wait_for(&mut self, count: usize, timeout: std::time::Duration) -> Vec<Reply> {
        let start = std::time::Instant::now();
        let mut out = Vec::new();
        while out.len() < count && start.elapsed() < timeout {
            out.extend(self.queue.fetch_all());
            if out.len() < count {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        out
    }
}
