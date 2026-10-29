//! The operations of the service (`HttpOperation` and its subclasses), the
//! request record (`HttpOpRequest`) with its transport and reply state, and
//! the reply delivered to the caller (`HttpResponse` plus the operation's
//! status), queued per `HttpRequest` instance.

use std::collections::VecDeque;
use std::ffi::CString;
use std::sync::{Arc, Mutex};

use crate::hooks::Hooks;
use crate::options::{
    PolicyOption, RETRY_BACKOFF_MAX, RETRY_BACKOFF_MAX_DEFAULT, RETRY_BACKOFF_MIN_DEFAULT,
    RETRY_COUNT_DEFAULT, RETRY_COUNT_MAX, RETRY_COUNT_MIN, RequestOptions, TRACE_MAX, TRACE_MIN,
};
use crate::status::Status;
use crate::sys::{SendPtr, SslInfo, UserPtr};

/// `HttpOpRequest::EMethod`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Copy,
    Move,
}

impl Method {
    /// `HttpOpRequest::methodToString`.
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
            Method::Copy => "COPY",
            Method::Move => "MOVE",
        }
    }
}

/// What the caller supplies for a request.
#[derive(Clone, Debug)]
pub struct Request {
    pub policy: u32,
    pub method: Method,
    pub url: String,
    pub body: Option<Vec<u8>>,
    /// Byte-range start (`requestGetByteRange`), else 0.
    pub offset: u64,
    /// Byte-range length, 0 for open-ended or none.
    pub length: u64,
    pub headers: Vec<(String, String)>,
    pub options: Option<RequestOptions>,
}

impl Request {
    pub fn new(policy: u32, method: Method, url: impl Into<String>) -> Request {
        Request {
            policy,
            method,
            url: url.into(),
            body: None,
            offset: 0,
            length: 0,
            headers: Vec::new(),
            options: None,
        }
    }

    pub fn body(mut self, body: Vec<u8>) -> Request {
        self.body = Some(body);
        self
    }

    pub fn range(mut self, offset: u64, length: u64) -> Request {
        self.offset = offset;
        self.length = length;
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Request {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn options(mut self, options: RequestOptions) -> Request {
        self.options = Some(options);
        self
    }
}

pub const PF_SCAN_RANGE_HEADER: u32 = 1;
pub const PF_SAVE_HEADERS: u32 = 2;
pub const PF_USE_RETRY_AFTER: u32 = 4;

/// Transfer statistics read from libcurl when the request completes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TransferStats {
    pub size_download: f64,
    pub total_time: f64,
    pub speed_download: f64,
}

/// What every operation carries.
pub struct OpCommon {
    pub handle: u64,
    pub user: UserPtr,
    pub reply: Option<Arc<ReplyQueue>>,
    pub tracing: i32,
}

/// `HttpOpRequest`: the request with its policy, transport and reply state.
pub struct RequestOp {
    pub common: OpCommon,
    pub req: Request,
    pub proc_flags: u32,
    pub retry_limit: u32,
    pub min_backoff: u64,
    pub max_backoff: u64,

    // Transport state (worker thread only).
    pub curl: SendPtr,
    pub slist: SendPtr,
    pub body_pos: usize,
    pub active: bool,
    pub url_c: CString,
    pub ssl: SslInfo,
    /// The service's hooks, for the libcurl callbacks.
    pub hooks: Option<Arc<dyn Hooks>>,

    // Reply state.
    pub status: Status,
    pub reply_body: Vec<u8>,
    pub reply_offset: u64,
    pub reply_length: u64,
    pub reply_full_length: u64,
    pub reply_headers: Option<Vec<(String, String)>>,
    pub content_type: String,
    pub retry_after: i32,
    pub xll_url: String,
    pub stats: TransferStats,

    // Policy state.
    pub retries: u32,
    pub retries_503: u32,
    pub retry_at: u64,
}

impl RequestOp {
    /// `HttpOpRequest::setupCommon` plus the method-specific setup.
    pub fn new(common: OpCommon, req: Request) -> RequestOp {
        let mut proc_flags = 0;
        let mut retry_limit = RETRY_COUNT_DEFAULT;
        let mut min_backoff = RETRY_BACKOFF_MIN_DEFAULT;
        let mut max_backoff = RETRY_BACKOFF_MAX_DEFAULT;
        let mut tracing = common.tracing;
        if let Some(o) = &req.options {
            if o.want_headers {
                proc_flags |= PF_SAVE_HEADERS;
            }
            if o.use_retry_after {
                proc_flags |= PF_USE_RETRY_AFTER;
            }
            retry_limit = o.retries.clamp(RETRY_COUNT_MIN, RETRY_COUNT_MAX);
            tracing = tracing.max(o.trace.clamp(TRACE_MIN, TRACE_MAX));
            min_backoff = o.min_backoff.clamp(0, RETRY_BACKOFF_MAX);
            max_backoff = o.max_backoff.clamp(min_backoff, RETRY_BACKOFF_MAX);
        }
        if req.method == Method::Get && (req.offset != 0 || req.length != 0) {
            proc_flags |= PF_SCAN_RANGE_HEADER;
        }
        let url_c = CString::new(req.url.replace('\0', "")).unwrap_or_default();
        let ssl = SslInfo {
            handle: common.handle,
            user: common.user,
            url: std::ptr::null(),
            verify_peer: req.options.as_ref().is_some_and(|o| o.verify_peer) as i32,
            result_type: 0,
            result_code: 0,
        };
        RequestOp {
            common: OpCommon { tracing, ..common },
            req,
            proc_flags,
            retry_limit,
            min_backoff,
            max_backoff,
            curl: SendPtr::null(),
            slist: SendPtr::null(),
            body_pos: 0,
            active: false,
            url_c,
            ssl,
            hooks: None,
            status: Status::OK,
            reply_body: Vec::new(),
            reply_offset: 0,
            reply_length: 0,
            reply_full_length: 0,
            reply_headers: None,
            content_type: String::new(),
            retry_after: 0,
            xll_url: String::new(),
            stats: TransferStats::default(),
            retries: 0,
            retries_503: 0,
            retry_at: 0,
        }
    }

    pub fn handle(&self) -> u64 {
        self.common.handle
    }

    pub fn policy(&self) -> u32 {
        self.req.policy
    }

    /// The reply the caller receives (`HttpOpRequest::visitNotifier`).
    pub fn into_reply(mut self) -> Reply {
        let range = if self.reply_offset != 0 || self.reply_length != 0 {
            Some((self.reply_offset, self.reply_length, self.reply_full_length))
        } else {
            None
        };
        Reply {
            handle: self.common.handle,
            user: self.common.user,
            kind: ReplyKind::Request,
            status: self.status,
            body: std::mem::take(&mut self.reply_body),
            headers: self.reply_headers.take(),
            content_type: std::mem::take(&mut self.content_type),
            range,
            retries: self.retries,
            retries_503: self.retries_503,
            url: std::mem::take(&mut self.req.url),
            method: self.req.method.as_str(),
            stats: self.stats,
            long_value: 0,
            string_value: String::new(),
        }
    }
}

/// `HttpOpSetGet`.
pub struct SetGetOp {
    pub common: OpCommon,
    pub option: PolicyOption,
    pub pclass: u32,
    pub do_set: bool,
    pub long_value: i64,
    pub string_value: String,
}

/// The operations the service thread processes (`HttpOperation` subclasses).
pub enum Op {
    Request(Box<RequestOp>),
    Cancel {
        common: OpCommon,
        target: u64,
    },
    SetGet(SetGetOp),
    /// `HttpOpStop`.
    Stop(OpCommon),
    /// `HttpOpNull`.
    NoOp(OpCommon),
    /// `HttpOpSpin`: 0 spins forever, 1 requeues itself.
    Spin {
        common: OpCommon,
        mode: i32,
    },
}

impl Op {
    pub fn common(&self) -> &OpCommon {
        match self {
            Op::Request(r) => &r.common,
            Op::Cancel { common, .. } => common,
            Op::SetGet(s) => &s.common,
            Op::Stop(c) | Op::NoOp(c) => c,
            Op::Spin { common, .. } => common,
        }
    }

    pub fn common_mut(&mut self) -> &mut OpCommon {
        match self {
            Op::Request(r) => &mut r.common,
            Op::Cancel { common, .. } => common,
            Op::SetGet(s) => &mut s.common,
            Op::Stop(c) | Op::NoOp(c) => c,
            Op::Spin { common, .. } => common,
        }
    }
}

/// Which operation a reply answers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReplyKind {
    Request,
    Cancel,
    SetGet,
    Stop,
    NoOp,
}

/// A completed operation as the caller's `update()` receives it: the
/// `HttpResponse` fields and the handle and user data to route it.
pub struct Reply {
    pub handle: u64,
    pub user: UserPtr,
    pub kind: ReplyKind,
    pub status: Status,
    pub body: Vec<u8>,
    pub headers: Option<Vec<(String, String)>>,
    pub content_type: String,
    pub range: Option<(u64, u64, u64)>,
    pub retries: u32,
    pub retries_503: u32,
    pub url: String,
    pub method: &'static str,
    pub stats: TransferStats,
    pub long_value: i64,
    pub string_value: String,
}

impl Reply {
    /// A reply carrying only a status (`HttpOperation::visitNotifier`).
    pub fn plain(common: &OpCommon, kind: ReplyKind, status: Status) -> Reply {
        Reply {
            handle: common.handle,
            user: common.user,
            kind,
            status,
            body: Vec::new(),
            headers: None,
            content_type: String::new(),
            range: None,
            retries: 0,
            retries_503: 0,
            url: String::new(),
            method: "",
            stats: TransferStats::default(),
            long_value: 0,
            string_value: String::new(),
        }
    }
}

/// `HttpReplyQueue`: one per `HttpRequest` instance, filled by the service
/// thread, drained by the instance's `update()`.
#[derive(Default)]
pub struct ReplyQueue {
    queue: Mutex<VecDeque<Reply>>,
}

impl ReplyQueue {
    pub fn new() -> Arc<ReplyQueue> {
        Arc::new(ReplyQueue::default())
    }

    pub fn push(&self, reply: Reply) {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(reply);
    }

    pub fn fetch(&self) -> Option<Reply> {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }

    pub fn fetch_all(&self) -> Vec<Reply> {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Delivers a plain reply to the operation's queue, if it has one.
pub fn reply_plain(common: &OpCommon, kind: ReplyKind, status: Status) {
    if let Some(q) = &common.reply {
        q.push(Reply::plain(common, kind, status));
    }
}
